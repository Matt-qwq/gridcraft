//! Text engine tests: bidi levels and visual order, script shaping, robustness, cache bounds.
//!
//! Font bytes: the bidi, Latin, emoji and robustness tests use the two faces compiled into
//! `epaint_default_fonts` (a dev-dependency), so they run on every machine with no installed font
//! and no committed binary. Script tests (Arabic, Devanagari, Thai) read the platform's faces
//! through [`fonts`], exactly as the app does; a machine without the script's font prints why and
//! skips that test instead of failing (AGENTS.md forbids committing font binaries — set
//! `CRAFT_FONTS_DIR` to a directory of Noto faces to run them in a bare container).

use super::*;

use crate::fonts;

/// The size every test shapes at, unless it is testing the size itself.
const SIZE: FontSize = FontSize::em(16.0);

/// A registered face plus the bytes it came from, so a test can read the font's own `cmap`.
struct Fixture {
    engine: TextEngine,
    face: FaceId,
    bytes: Arc<[u8]>,
    index: u32,
}

impl Fixture {
    fn new(bytes: Arc<[u8]>, index: u32) -> Self {
        let mut engine = TextEngine::new();
        let face = engine.add_face_at(bytes.clone(), index).expect("the fixture is a parseable face");
        Self { engine, face, bytes, index }
    }

    fn ubuntu() -> Self {
        Self::new(Arc::from(epaint_default_fonts::UBUNTU_LIGHT), 0)
    }

    fn shape(&mut self, text: &str, direction: Direction) -> Arc<ShapedText> {
        self.shape_at(text, direction, SIZE)
    }

    fn shape_at(&mut self, text: &str, direction: Direction, size: FontSize) -> Arc<ShapedText> {
        self.engine.shape(text, direction, self.face, size).expect("a registered face shapes")
    }

    /// The glyph the face's `cmap` gives `c`: what a per-character (unshaped) mapping would use.
    fn nominal(&self, c: char) -> u16 {
        rustybuzz::Face::from_slice(&self.bytes, self.index).and_then(|face| face.glyph_index(c)).map_or(0, |id| id.0)
    }
}

/// A face for `script` from the platform's fonts (or `CRAFT_FONTS_DIR`), trying the faces of a
/// collection until one has a glyph for `probe`. `None` when this machine has no such font.
fn script_fixture(script: fonts::Script, probe: char) -> Option<Fixture> {
    let font = fonts::resolve_for_script(script).ok()?;
    let bytes: Arc<[u8]> = Arc::from(font.data.into_boxed_slice());
    let index = (0..4).find(|&index| rustybuzz::Face::from_slice(&bytes, index).is_some_and(|face| face.glyph_index(probe).is_some()))?;
    Some(Fixture::new(bytes, index))
}

/// [`script_fixture`] as a statement: print why and return from the test when the font is absent.
macro_rules! script_fixture {
    ($script:expr, $probe:expr) => {
        match script_fixture($script, $probe) {
            Some(fixture) => fixture,
            None => {
                eprintln!("skipped: no installed {} face (set CRAFT_FONTS_DIR to test it)", $script.name());
                return;
            }
        }
    };
}

/// Distinct clusters tile `text` in logical order: no gap, no overlap, every byte claimed once.
fn assert_cluster_map(shaped: &ShapedText, text: &str) {
    let mut ranges: Vec<(u32, u32)> = shaped.glyphs().map(|glyph| (glyph.cluster_start, glyph.cluster_end)).collect();
    assert!(!ranges.is_empty(), "a non-empty string must produce glyphs");
    ranges.sort_unstable();
    ranges.dedup();
    let mut next = 0;
    for (start, end) in ranges {
        assert_eq!(start, next, "clusters must tile {text:?} without a gap or overlap");
        assert!(end > start, "empty cluster in {text:?}");
        next = end;
    }
    assert_eq!(next as usize, text.len(), "clusters must cover every byte of {text:?}");
}

/// Runs tile the text too, in logical order.
fn assert_run_map(shaped: &ShapedText, text: &str) {
    let mut ranges: Vec<(u32, u32)> = shaped.runs.iter().map(|run| (run.range.start, run.range.end)).collect();
    if ranges.is_empty() {
        assert!(text.is_empty(), "only an empty string has no runs");
        return;
    }
    ranges.sort_unstable();
    let mut next = 0;
    for (start, end) in ranges {
        assert_eq!(start, next, "runs must tile {text:?} without a gap or overlap");
        assert!(end > start, "empty run in {text:?}");
        next = end;
    }
    assert_eq!(next as usize, text.len(), "runs must cover every byte of {text:?}");
}

/// Glyphs inside a run follow the run's direction: logical cluster starts rise in an LTR run and
/// fall in an RTL one. That is what "visual order, left to right" means.
fn assert_visual_order(shaped: &ShapedText) {
    for run in &shaped.runs {
        let starts: Vec<u32> = run.glyphs.iter().map(|glyph| glyph.cluster_start).collect();
        let ordered = if run.direction() == Direction::Rtl {
            starts.windows(2).all(|pair| pair[0] >= pair[1])
        } else {
            starts.windows(2).all(|pair| pair[0] <= pair[1])
        };
        assert!(ordered, "glyphs out of visual order in run {:?}: {starts:?}", run.range);
    }
}

/// The non-blank source text of each run, in visual order: what the line looks like when the runs
/// are read left to right.
fn visual_chunks<'a>(shaped: &ShapedText, text: &'a str) -> Vec<&'a str> {
    shaped
        .runs
        .iter()
        .filter_map(|run| text.get(run.range.start as usize..run.range.end as usize))
        .map(str::trim)
        .filter(|chunk| !chunk.is_empty())
        .collect()
}

/// The run that covers `needle`'s first occurrence.
fn run_of<'a>(shaped: &'a ShapedText, text: &str, needle: &str) -> &'a GlyphRun {
    let at = text.find(needle).expect("the needle is in the text") as u32;
    shaped.runs.iter().find(|run| run.range.start <= at && at < run.range.end).expect("a run covers the needle")
}

/// The advance widths of the glyphs in visual order.
fn advances(shaped: &ShapedText) -> Vec<f32> {
    shaped.glyphs().map(|glyph| glyph.advance).collect()
}

// ---------------------------------------------------------------- bidi ------------------------------------------------

#[test]
fn bidi_mixed_ltr_text_puts_latin_then_digits_then_hebrew() {
    let mut fixture = Fixture::ubuntu();
    let text = "abc אבג 123";
    let shaped = fixture.shape(text, Direction::Auto);
    assert_eq!(shaped.base_direction, Direction::Ltr, "P2/P3: the first strong character is Latin");
    assert_run_map(&shaped, text);
    assert_cluster_map(&shaped, text);
    assert_visual_order(&shaped);

    // UBA: Hebrew level 1, the digits next to it level 2 (they stay an even, LTR run), Latin 0.
    let levels: Vec<u8> = shaped.runs.iter().map(|run| run.level).collect();
    assert_eq!(levels, [0, 2, 1]);

    // L2 reverses the RTL run only: abc, then the digits, then the Hebrew, left to right.
    assert_eq!(visual_chunks(&shaped, text), ["abc", "123", "אבג"]);
    let digits = run_of(&shaped, text, "123");
    assert_eq!(digits.glyphs.iter().map(|glyph| glyph.cluster_start).collect::<Vec<_>>(), [11, 12, 13]);
    assert_eq!(digits.glyphs.iter().map(|glyph| glyph.id).collect::<Vec<_>>(), [fixture.nominal('1'), fixture.nominal('2'), fixture.nominal('3')]);
    // The Hebrew glyphs come out in reverse logical order; the space that ended the logical
    // Hebrew chunk belongs to the same level-1 run and comes first in visual order.
    let hebrew = run_of(&shaped, text, "אבג");
    assert_eq!(hebrew.range, 4..11, "the trailing space shares the Hebrew's level");
    assert_eq!(hebrew.glyphs.iter().map(|glyph| glyph.cluster_start).collect::<Vec<_>>(), [10, 8, 6, 4]);
}

#[test]
fn bidi_rtl_paragraph_reverses_the_chunks_but_not_the_number() {
    let mut fixture = Fixture::ubuntu();
    let text = "abc אב 123";
    let shaped = fixture.shape(text, Direction::Rtl);
    assert_eq!(shaped.base_direction, Direction::Rtl);
    assert_run_map(&shaped, text);
    assert_cluster_map(&shaped, text);
    assert_visual_order(&shaped);

    // A base-RTL paragraph reads from the right: the first logical chunk is drawn rightmost.
    assert_eq!(visual_chunks(&shaped, text), ["123", "אב", "abc"]);

    // The number is an even-level (LTR) island: its digits keep their reading order.
    let digits = run_of(&shaped, text, "123");
    assert!(digits.level.is_multiple_of(2), "a European number must keep an even level in an RTL paragraph");
    assert_eq!(digits.glyphs.iter().map(|glyph| glyph.cluster_start).collect::<Vec<_>>(), [9, 10, 11]);
    assert_eq!(digits.glyphs.iter().map(|glyph| glyph.id).collect::<Vec<_>>(), [fixture.nominal('1'), fixture.nominal('2'), fixture.nominal('3')]);
    // ... while the Hebrew run inside the same string is reversed. The two spaces around it share
    // its level (N2 takes the paragraph direction between L and R), so they are part of the run.
    let hebrew = run_of(&shaped, text, "אב");
    assert!(!hebrew.level.is_multiple_of(2), "Hebrew keeps an odd level");
    assert_eq!(hebrew.range, 3..9);
    assert_eq!(hebrew.glyphs.iter().map(|glyph| glyph.cluster_start).collect::<Vec<_>>(), [8, 6, 4, 3]);
}

#[test]
fn bidi_explicit_override_and_embedding_are_honoured() {
    let mut fixture = Fixture::ubuntu();

    // U+202E RIGHT-TO-LEFT OVERRIDE forces "123" to level 1, so it lays out (and reads) "321".
    let text = "abc \u{202E}123\u{202C}";
    let shaped = fixture.shape(text, Direction::Ltr);
    let digits = run_of(&shaped, text, "123");
    assert!(!digits.level.is_multiple_of(2), "the override makes the digits right-to-left");
    assert_eq!(digits.glyphs.iter().map(|glyph| glyph.cluster_start).collect::<Vec<_>>(), [9, 8, 7]);
    assert_run_map(&shaped, text);
    assert_cluster_map(&shaped, text);

    // U+202B RIGHT-TO-LEFT EMBEDDING raises the level of the embedded text (rule I2 lifts Latin
    // inside an RTL embedding to the next even level), so the Latin keeps its own direction but
    // is now a separate, embedded run at level 2.
    let text = "\u{202B}abc\u{202C}";
    let shaped = fixture.shape(text, Direction::Ltr);
    let latin = run_of(&shaped, text, "abc");
    assert_eq!(latin.level, 2, "the embedding is applied");
    assert_eq!(
        latin.glyphs.iter().map(|glyph| glyph.cluster_start).collect::<Vec<_>>(),
        [3, 4, 5],
        "Latin inside an RTL embedding stays left to right"
    );
    assert_eq!(fixture.shape("abc", Direction::Ltr).runs[0].level, 0, "without the control the same text is level 0");
}

#[test]
fn bidi_neutral_only_text_follows_the_base_direction() {
    let mut fixture = Fixture::ubuntu();
    let text = " - ";
    let ltr = fixture.shape(text, Direction::Ltr);
    assert_eq!(ltr.runs.len(), 1);
    assert_eq!(ltr.runs[0].level, 0);
    let rtl = fixture.shape(text, Direction::Rtl);
    assert_eq!(rtl.runs.len(), 1);
    assert_eq!(rtl.runs[0].level, 1);
    // P3: with no strong character, auto resolves to LTR.
    assert_eq!(fixture.shape(text, Direction::Auto).base_direction, Direction::Ltr);
}

#[test]
fn bidi_auto_direction_follows_the_first_strong_character() {
    let mut fixture = Fixture::ubuntu();
    assert_eq!(fixture.shape("אבג abc", Direction::Auto).base_direction, Direction::Rtl);
    assert_eq!(fixture.shape("abc אבג", Direction::Auto).base_direction, Direction::Ltr);
    // Digits and neutrals are not strong, so they fall back to LTR.
    assert_eq!(fixture.shape("123", Direction::Auto).base_direction, Direction::Ltr);
    assert_eq!(fixture.shape("", Direction::Auto).base_direction, Direction::Ltr);
}

// -------------------------------------------------------------- Arabic -----------------------------------------------

#[test]
fn arabic_shapes_contextual_forms() {
    let mut fixture = script_fixture!(fonts::Script::Arabic, 'س');
    let text = "سلام";
    let shaped = fixture.shape(text, Direction::Auto);
    assert_eq!(shaped.base_direction, Direction::Rtl);
    assert_eq!(shaped.glyph_count(), 4);
    assert_run_map(&shaped, text);
    assert_cluster_map(&shaped, text);
    assert_visual_order(&shaped);

    // Every letter joins: the glyph at its cluster is not the isolated form a per-character
    // mapping would draw.
    let mut differing = 0;
    for (at, c) in text.char_indices() {
        let isolated = fixture.shape(&c.to_string(), Direction::Rtl).glyphs().next().expect("one glyph").id;
        let joined = shaped.glyphs().find(|glyph| glyph.cluster_start as usize == at).expect("a glyph starts at each letter").id;
        if joined != isolated {
            differing += 1;
        }
    }
    assert!(differing > 0, "no letter took a contextual form: {:?}", shaped.glyphs().map(|g| g.id).collect::<Vec<_>>());

    // And the word is not the naive per-character mapping (which is what the unshaped path drew).
    let naive: Vec<u16> = text.chars().map(|c| fixture.nominal(c)).collect();
    let mut logical: Vec<(u32, u16)> = shaped.glyphs().map(|glyph| (glyph.cluster_start, glyph.id)).collect();
    logical.sort_unstable();
    assert_ne!(logical.iter().map(|(_, id)| *id).collect::<Vec<_>>(), naive);
}

#[test]
fn arabic_ligature_collapses_letters_into_one_glyph() {
    let mut fixture = script_fixture!(fonts::Script::Arabic, 'ا');
    // The two required Arabic ligatures. A face carries at least one of them (the four Arabic
    // faces on the dev machine carry both), and shaping must then produce fewer glyphs than the
    // naive one-glyph-per-character mapping.
    let mut collapsed = Vec::new();
    for text in ["الله", "لا"] {
        let shaped = fixture.shape(text, Direction::Auto);
        assert_cluster_map(&shaped, text);
        if shaped.glyph_count() < text.chars().count() {
            collapsed.push(text);
        }
    }
    assert!(
        !collapsed.is_empty(),
        "the face must ligate الله or لا: {:?}",
        fixture.shape("الله", Direction::Auto).glyphs().map(|g| g.id).collect::<Vec<_>>()
    );
}

#[test]
fn arabic_zwnj_blocks_the_join() {
    let mut fixture = script_fixture!(fonts::Script::Arabic, 'س');
    let seen_isolated = fixture.shape("س", Direction::Rtl).glyphs().next().expect("seen").id;
    let lam_isolated = fixture.shape("ل", Direction::Rtl).glyphs().next().expect("lam").id;

    let joined = fixture.shape("سل", Direction::Rtl);
    let joined_ids: Vec<u16> = joined.glyphs().map(|glyph| glyph.id).collect();
    assert!(
        !joined_ids.contains(&seen_isolated) || !joined_ids.contains(&lam_isolated),
        "without a separator at least one letter must join: {joined_ids:?}"
    );

    let blocked = fixture.shape("س\u{200C}ل", Direction::Rtl);
    let blocked_ids: Vec<u16> = blocked.glyphs().map(|glyph| glyph.id).collect();
    assert!(blocked_ids.contains(&seen_isolated), "U+200C leaves seen isolated: {blocked_ids:?}");
    assert!(blocked_ids.contains(&lam_isolated), "U+200C leaves lam isolated: {blocked_ids:?}");
    assert_ne!(joined_ids, blocked_ids);
    assert_cluster_map(&blocked, "س\u{200C}ل");
    assert_visual_order(&blocked);
}

#[test]
fn arabic_keeps_numbers_left_to_right() {
    let mut fixture = script_fixture!(fonts::Script::Arabic, 'م');
    let text = "مبيعات 2024";
    let shaped = fixture.shape(text, Direction::Auto);
    assert_eq!(shaped.base_direction, Direction::Rtl);
    assert_cluster_map(&shaped, text);
    assert_visual_order(&shaped);

    let digits = run_of(&shaped, text, "2024");
    assert!(digits.level.is_multiple_of(2), "the number is an LTR island in RTL text");
    let starts: Vec<u32> = digits.glyphs.iter().map(|glyph| glyph.cluster_start).collect();
    assert_eq!(starts.len(), 4);
    assert!(starts.windows(2).all(|pair| pair[0] < pair[1]), "2024 must read left to right: {starts:?}");
    let expected: Vec<u16> = "2024".chars().map(|c| fixture.nominal(c)).collect();
    assert_eq!(digits.glyphs.iter().map(|glyph| glyph.id).collect::<Vec<_>>(), expected);

    // In an RTL paragraph the number sits at the left of the word.
    let first_digit = shaped.glyphs().position(|glyph| glyph.cluster_start == digits.range.start).expect("a digit");
    let first_letter = shaped.glyphs().position(|glyph| glyph.cluster_start == 0).expect("the first letter");
    assert!(first_digit < first_letter, "the number must be drawn left of the Arabic word");
}

// ----------------------------------------------------------- Devanagari ------------------------------------------------

#[test]
fn devanagari_reorders_the_vowel_sign_and_merges_the_cluster() {
    let mut fixture = script_fixture!(fonts::Script::Devanagari, 'क');
    let text = "क्षि";
    let shaped = fixture.shape(text, Direction::Ltr);
    assert_eq!(shaped.runs.len(), 1);
    assert_run_map(&shaped, text);
    assert_cluster_map(&shaped, text);
    assert_visual_order(&shaped);

    // क + ् + ष + ि is four characters; shaping forms the क्ष conjunct and moves the i-matra in
    // front of its base, so neither the count nor the order matches the stored text.
    assert!(shaped.glyph_count() < text.chars().count(), "{} glyphs for {} characters", shaped.glyph_count(), text.chars().count());
    assert!(shaped.glyph_count() >= 2, "the matra and the conjunct are separate glyphs");
    let ids: Vec<u16> = shaped.glyphs().map(|glyph| glyph.id).collect();
    assert_eq!(ids[0], fixture.nominal('ि'), "the i-matra is drawn before its base: {ids:?}");
    assert_ne!(ids[1], fixture.nominal('क'), "the base is a conjunct, not a lone क: {ids:?}");
    // The reordered sign belongs to one cluster that covers the whole string.
    let whole = 0..text.len() as u32;
    assert_eq!(shaped.glyphs().map(Glyph::cluster).collect::<Vec<_>>(), vec![whole.clone(), whole]);
}

// -------------------------------------------------------------- Thai ---------------------------------------------------

#[test]
fn thai_tone_mark_has_zero_advance() {
    let mut fixture = script_fixture!(fonts::Script::Thai, 'ก');
    let text = "ก่";
    let shaped = fixture.shape(text, Direction::Ltr);
    assert_eq!(shaped.glyph_count(), 2);
    assert_cluster_map(&shaped, text);
    let base = shaped.glyphs().next().expect("the base consonant");
    let mark = shaped.glyphs().nth(1).expect("the tone mark");
    assert!(base.advance > 0.0, "the base consonant advances the pen");
    assert_eq!(mark.advance, 0.0, "GPOS positions the tone mark instead of advancing");
    assert_eq!(advances(&shaped), [base.advance, 0.0]);
    assert_eq!(shaped.width, base.advance);
}

// -------------------------------------------------------------- Latin --------------------------------------------------

#[test]
fn latin_fi_ligature_collapses_two_characters() {
    let mut fixture = Fixture::ubuntu();
    let shaped = fixture.shape("fi", Direction::Ltr);
    assert_eq!(shaped.glyph_count(), 1, "Ubuntu Light ligates fi");
    let ligature = shaped.glyphs().next().expect("the ligature");
    assert_eq!(ligature.cluster(), 0..2, "the ligature still covers both characters");
    assert_ne!(ligature.id, fixture.nominal('f'));
    assert_ne!(ligature.id, fixture.nominal('i'));

    let shaped = fixture.shape("fix", Direction::Ltr);
    assert_eq!(shaped.glyph_count(), 2);
    assert_eq!(shaped.glyphs().next().expect("the ligature").cluster(), 0..2);
    assert_eq!(shaped.glyphs().nth(1).expect("x").cluster(), 2..3);
}

#[test]
fn latin_size_and_scale_multiply_the_advances() {
    let mut fixture = Fixture::ubuntu();
    let plain = fixture.shape_at("Hello", Direction::Ltr, FontSize::em(16.0));
    let doubled = fixture.shape_at("Hello", Direction::Ltr, FontSize::new(16.0, 2.0));
    assert!((doubled.width - 2.0 * plain.width).abs() < 0.01, "{} vs {}", doubled.width, plain.width);
    assert!((doubled.ascent - 2.0 * plain.ascent).abs() < 0.01);
    assert!(plain.ascent > 0.0);
    assert!(plain.descent < 0.0, "a descent is reported below the baseline");
}

#[test]
fn latin_rtl_mirrors_brackets() {
    let mut fixture = Fixture::ubuntu();
    let open = fixture.shape("(", Direction::Ltr).glyphs().next().expect("open").id;
    let close = fixture.shape(")", Direction::Ltr).glyphs().next().expect("close").id;
    assert_ne!(open, close);
    assert_eq!(fixture.shape("(", Direction::Rtl).glyphs().next().expect("mirrored").id, close, "an RTL paragraph mirrors (");
    assert_eq!(fixture.shape(")", Direction::Rtl).glyphs().next().expect("mirrored").id, open, "an RTL paragraph mirrors )");
}

// ----------------------------------------------------------- robustness -----------------------------------------------

#[test]
fn robustness_empty_string() {
    let mut fixture = Fixture::ubuntu();
    let shaped = fixture.shape("", Direction::Auto);
    assert!(shaped.is_empty());
    assert_eq!(shaped.glyph_count(), 0);
    assert_eq!(shaped.width, 0.0);
    assert_eq!(shaped.range(), None);
    assert_eq!(shaped.base_direction, Direction::Ltr);
}

#[test]
fn robustness_lone_combining_mark() {
    let mut fixture = Fixture::ubuntu();
    let text = "\u{0301}";
    let shaped = fixture.shape(text, Direction::Ltr);
    assert_eq!(shaped.glyph_count(), 1);
    assert_cluster_map(&shaped, text);
    assert_eq!(shaped.glyphs().next().expect("the mark").cluster(), 0..2);
}

#[test]
fn robustness_unassigned_codepoints() {
    let mut fixture = Fixture::ubuntu();
    let text = "\u{0378}\u{0379}";
    let shaped = fixture.shape(text, Direction::Ltr);
    assert_eq!(shaped.glyph_count(), 2);
    assert!(shaped.glyphs().all(Glyph::is_missing), "no face has a glyph for an unassigned code point");
    assert_cluster_map(&shaped, text);
}

#[test]
fn robustness_astral_characters() {
    // An emoji is one char in four UTF-8 bytes: the cluster is a byte range, not a char index.
    let mut fixture = Fixture::new(Arc::from(epaint_default_fonts::NOTO_EMOJI_REGULAR), 0);
    let text = "a😀b";
    let shaped = fixture.shape(text, Direction::Ltr);
    assert_eq!(shaped.glyph_count(), 3);
    assert_eq!(shaped.glyphs().nth(1).expect("the emoji").cluster(), 1..5);
    assert_cluster_map(&shaped, text);
}

#[test]
fn robustness_cut_clusters_shape_without_panic() {
    let mut fixture = Fixture::ubuntu();
    // Nothing rejects a string that begins or ends mid-grapheme: an isolated combining mark, a
    // consonant with no base, a lone joiner, a lone bidi control, a lone variation selector.
    for text in ["\u{0301}", "\u{0301}a\u{0301}", "\u{094D}ष", "\u{200D}", "\u{200C}", "\u{FE0F}", "\u{202E}", "क"] {
        let shaped = fixture.shape(text, Direction::Auto);
        assert_cluster_map(&shaped, text);
    }
    // Same with a real Indic face: a cut conjunct (virama + consonant, no base) must not panic.
    if let Some(mut devanagari) = script_fixture(fonts::Script::Devanagari, 'क') {
        for text in ["\u{0937}\u{093F}", "\u{094D}ष", "\u{093F}"] {
            let shaped = devanagari.shape(text, Direction::Ltr);
            assert_cluster_map(&shaped, text);
        }
    }
}

#[test]
fn robustness_long_text_is_complete() {
    // 100 000 characters through a single shaping call: the glyph stream and the cluster map stay
    // consistent. (Timing: `long_text_scales_linearly`, next.)
    let mut fixture = Fixture::ubuntu();
    let text: String = "The quick brown fox jumps over the lazy dog. ".chars().cycle().take(100_000).collect();
    let shaped = fixture.shape(&text, Direction::Ltr);
    assert_eq!(shaped.glyph_count(), 100_000);
    assert_eq!(shaped.runs.len(), 1);
    assert_eq!(shaped.range(), Some(0..text.len() as u32));
    assert_visual_order(&shaped);
    // Checking every cluster is O(n) and proves the map has no gap.
    let mut next = 0;
    for glyph in shaped.glyphs() {
        assert_eq!(glyph.cluster_start, next);
        next = glyph.cluster_end;
    }
    assert_eq!(next as usize, text.len());
}

/// A monotonic clock, or `None` on wasm where reading one traps. The timing tests return early
/// there; they are about the native shaping path, which is the one the app ships.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::disallowed_methods)] // the one place the engine's tests read the clock
fn clock() -> Option<std::time::Instant> {
    Some(std::time::Instant::now())
}

#[cfg(target_arch = "wasm32")]
fn clock() -> Option<std::time::Instant> {
    None
}

#[test]
fn long_text_scales_linearly() {
    let unit = "The quick brown fox jumps over the lazy dog. ";
    let mut fixture = Fixture::ubuntu();
    let mut timed = |chars: usize| -> Option<std::time::Duration> {
        let text: String = unit.chars().cycle().take(chars).collect();
        let start = clock()?;
        let shaped = fixture.shape(&text, Direction::Ltr);
        let elapsed = start.elapsed();
        assert_eq!(shaped.glyph_count(), chars);
        Some(elapsed)
    };
    let (Some(ten_thousand), Some(hundred_thousand)) = (timed(10_000), timed(100_000)) else { return };
    eprintln!("shaping: 10k chars {ten_thousand:?}, 100k chars {hundred_thousand:?}");
    // Ten times the text; quadratic shaping would be a hundred times slower. The slack covers
    // timer noise on a busy machine and the four-fold growth of the bidi level vector.
    let budget = ten_thousand * 40 + std::time::Duration::from_millis(50);
    assert!(
        hundred_thousand < budget,
        "100k chars took {hundred_thousand:?}, more than the {budget:?} budget for a linear pass (10k took {ten_thousand:?})"
    );
}

// ---------------------------------------------------------------- cache ------------------------------------------------

#[test]
fn cache_reuses_shapings_and_stays_bounded() {
    let mut fixture = Fixture::ubuntu();
    let first = fixture.shape("cache me", Direction::Ltr);
    let second = fixture.shape("cache me", Direction::Ltr);
    assert_eq!(first, second, "the same request returns the same shaping");
    let stats = fixture.engine.cache_stats();
    assert_eq!((stats.shapes, stats.hits, stats.misses), (1, 1, 1));

    // A different size is a different entry, and advances scale with it.
    let doubled = fixture.shape_at("cache me", Direction::Ltr, FontSize::em(32.0));
    assert!((doubled.width - 2.0 * first.width).abs() < 0.01);
    assert_eq!(fixture.engine.cache_stats().shapes, 2);

    // Fill the cache past its bounds; every bound must hold.
    for row in 0..(MAX_CACHED_SHAPES + 64) {
        fixture.shape(&format!("row {row} of the sheet"), Direction::Ltr);
    }
    let stats = fixture.engine.cache_stats();
    assert!(stats.shapes <= MAX_CACHED_SHAPES, "{} shapes", stats.shapes);
    assert!(stats.glyphs <= MAX_CACHED_GLYPHS, "{} glyphs", stats.glyphs);
    assert!(stats.text_bytes <= MAX_CACHED_TEXT_BYTES, "{} text bytes", stats.text_bytes);
    assert!(stats.misses > stats.hits, "the fills were misses");

    // A long string is shaped but never cached, so one big cell cannot flush the working set.
    let long = "x".repeat(MAX_CACHED_TEXT + 1);
    let before = fixture.engine.cache_stats();
    assert_eq!(fixture.shape(&long, Direction::Ltr).glyph_count(), long.len());
    let after = fixture.engine.cache_stats();
    assert_eq!((after.shapes, after.glyphs, after.text_bytes), (before.shapes, before.glyphs, before.text_bytes));
}

#[test]
fn faces_are_deduplicated_and_bounded() {
    let mut engine = TextEngine::new();
    let ubuntu: Arc<[u8]> = Arc::from(epaint_default_fonts::UBUNTU_LIGHT);
    let first = engine.add_face(ubuntu.clone()).expect("Ubuntu Light");
    assert_eq!(engine.add_face(ubuntu).expect("the same bytes"), first, "the same bytes are the same face");
    assert_eq!(engine.cache_stats().faces, 1);

    let metrics = engine.face_metrics(first).expect("registered");
    assert!(metrics.units_per_em > 0.0);
    assert!(metrics.ascender > 0.0, "an ascender is above the baseline");
    assert!(metrics.descender < 0.0, "a descender is below the baseline");
    assert!(metrics.glyphs > 0);
    assert_eq!(metrics.index, 0);

    // Register distinct byte strings (trailing padding changes the content) until the face cache
    // overflows. The bytes are still a valid font, so each one parses.
    let mut ids = vec![first];
    for pad in 1..=(MAX_FACES + 4) {
        let mut bytes = ubuntu_bytes();
        bytes.extend_from_slice(&vec![0u8; pad]);
        let id = engine.add_face(bytes).expect("padding does not break the font");
        assert!(!ids.contains(&id), "face ids must never be reused");
        ids.push(id);
    }
    assert_eq!(engine.cache_stats().faces, MAX_FACES);
    // The oldest face is gone, and shaping with it reports the eviction instead of drawing with
    // another font.
    assert!(engine.face_metrics(first).is_none());
    assert_eq!(engine.shape("a", Direction::Ltr, first, SIZE), Err(TextError::UnknownFace(first)));
    // The newest face still works.
    let newest = *ids.last().expect("at least one face");
    assert!(engine.shape("a", Direction::Ltr, newest, SIZE).is_ok());
}

#[test]
fn cached_shapings_are_reused_without_reparsing_the_face() {
    // The engine builds a `rustybuzz` view per call, not per glyph; a cache hit must not parse the
    // font's tables again. This measures 100 hits against one fresh shaping.
    let mut fixture = Fixture::ubuntu();
    let text = "the quick brown fox jumps over the lazy dog";
    fixture.shape(text, Direction::Ltr);
    let mut slowest = std::time::Duration::ZERO;
    for _ in 0..100 {
        let Some(start) = clock() else { return };
        fixture.shape(text, Direction::Ltr);
        slowest = slowest.max(start.elapsed());
    }
    let stats = fixture.engine.cache_stats();
    assert_eq!(stats.shapes, 1);
    assert_eq!(stats.hits, 100);
    // A hit is an `Arc` clone plus a hash of the key: microseconds, not a re-shape.
    assert!(slowest < std::time::Duration::from_millis(2), "the slowest cache hit took {slowest:?}");
}

fn ubuntu_bytes() -> Vec<u8> {
    epaint_default_fonts::UBUNTU_LIGHT.to_vec()
}

// --------------------------------------------------------------- errors ------------------------------------------------

#[test]
fn invalid_fonts_sizes_and_ids_are_errors() {
    let mut engine = TextEngine::new();
    for bytes in [Vec::new(), b"not a font".to_vec(), b"OTTO\x00\x00\x00\x00".to_vec(), epaint_default_fonts::UBUNTU_LIGHT[..40].to_vec()] {
        assert_eq!(engine.add_face(bytes), Err(TextError::InvalidFont { index: 0 }));
    }
    // Ubuntu Light is a single-face file: index 7 does not exist.
    assert_eq!(engine.add_face_at(ubuntu_bytes(), 7), Err(TextError::InvalidFont { index: 7 }));
    assert_eq!(engine.cache_stats().faces, 0);

    let face = engine.add_face(ubuntu_bytes()).expect("Ubuntu Light");
    assert_eq!(engine.shape("a", Direction::Ltr, FaceId(9_999), SIZE), Err(TextError::UnknownFace(FaceId(9_999))));
    for bad in [f32::NAN, f32::INFINITY, -16.0] {
        assert!(matches!(engine.shape("a", Direction::Ltr, face, FontSize::new(bad, 1.0)), Err(TextError::InvalidSize(_))));
    }
    // A 0-size face is legal (it draws nothing); a negative scale is not.
    assert!(engine.shape("a", Direction::Ltr, face, FontSize::new(0.0, 1.0)).is_ok());
    assert!(matches!(engine.shape("a", Direction::Ltr, face, FontSize::new(16.0, -1.0)), Err(TextError::InvalidSize(_))));

    let long = "a".repeat(MAX_TEXT_BYTES + 1);
    assert_eq!(engine.shape(&long, Direction::Ltr, face, SIZE), Err(TextError::TooLong { len: long.len() }));
}

#[test]
fn clear_drops_faces_and_shapings() {
    let mut fixture = Fixture::ubuntu();
    fixture.shape("hello", Direction::Ltr);
    assert_eq!(fixture.engine.cache_stats().shapes, 1);
    fixture.engine.clear();
    let stats = fixture.engine.cache_stats();
    assert_eq!((stats.faces, stats.shapes, stats.glyphs, stats.text_bytes), (0, 0, 0, 0));
    assert_eq!(fixture.engine.shape("hello", Direction::Ltr, fixture.face, SIZE), Err(TextError::UnknownFace(fixture.face)));
}
