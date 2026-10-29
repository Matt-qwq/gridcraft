//! Tests for the shaped-text wiring: the fast path, visual run order, measurement, and the caret
//! map the in-cell editor uses.
//!
//! Fonts: the bidi, Latin and robustness tests use the faces compiled into `epaint_default_fonts`
//! bound as a test family, so they run on every machine. Arabic, Hebrew, Devanagari and Thai read
//! the platform's own faces through [`gridcraft_engine::core::fonts`] (the same discovery the app
//! uses) and skip with a printed reason when this machine has no face for the script — never
//! committing a font binary (AGENTS.md).

use super::*;
use egui::FontFamily;
use gridcraft_engine::core::fonts;

/// A family name no test context binds, for the fallback tests.
const MISSING: &str = "GridCraft No Such Face";

fn ubuntu_context() -> Context {
    test_context("test-ubuntu", epaint_default_fonts::UBUNTU_LIGHT, 0)
}

fn ubuntu_engine() -> GridText {
    GridText::new()
}

fn font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("test-ubuntu".into()))
}

/// A context with a family bound to a script's installed face, or `None` when this machine has no
/// such face (`None` skips the test rather than failing it).
fn script_context(script: fonts::Script, probe: char) -> Option<(Context, FontId, GridText)> {
    let file = fonts::resolve_for_char(probe).ok()?;
    if fonts::Script::of(probe) != Some(script) {
        return None;
    }
    let index = (0..8).find(|i| face_has(&file.data, *i, probe))?;
    let ctx = test_context("test-script", &file.data, index);
    let id = FontId::new(32.0, FontFamily::Name("test-script".into()));
    Some((ctx, id, GridText::new()))
}

/// A context whose one family covers every character of `text`, resolved through the machine's
/// fonts: the face a workbook would really be drawn with, for the tests that assert the painted
/// order (a face without the script draws replacement glyphs and the grid keeps the unshaped
/// path, so those tests need real coverage).
fn covered_context(probe: char, text: &str) -> Option<(Context, FontId, GridText)> {
    let file = fonts::resolve_for_char(probe).ok()?;
    let chars: Vec<char> = text.chars().collect();
    let index = (0..8).find(|i| chars.iter().all(|c| face_has(&file.data, *i, *c)))?;
    let ctx = test_context("test-script", &file.data, index);
    let id = FontId::new(32.0, FontFamily::Name("test-script".into()));
    Some((ctx, id, GridText::new()))
}

/// Whether face `index` of `bytes` has a glyph for `c`.
fn face_has(bytes: &[u8], index: u32, c: char) -> bool {
    use skrifa::MetadataProvider;
    skrifa::FontRef::from_index(bytes, index).is_ok_and(|f| f.charmap().map(c).is_some())
}

/// The visual order of the source text's runs: what the line reads like when the runs are read
/// left to right (the runs' own edge blanks are trimmed away).
fn visual_slices(text: &str, shaped: &ShapedText) -> Vec<String> {
    shaped
        .runs
        .iter()
        .filter_map(|run| text.get(run.range.start as usize..run.range.end as usize))
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

/// `(char, x)` of the glyphs the painter really draws (a ligature's continuation glyphs are not
/// drawn, so they are left out of a comparison with the source characters).
fn drawn(galley: &Galley) -> Vec<(char, f32)> {
    galley_glyphs(galley).into_iter().filter(|(_, _, real)| *real).map(|(c, x, _)| (c, x)).collect()
}

#[test]
fn the_fast_path_is_printable_ascii_at_left_to_right() {
    // Bit-identical to the unshaped path: nothing the shaper could change.
    for text in ["", "Hello", "SUM(A1:A9)", " 123.5 ", "a\tb", "~!@#$%^&*()_+{}|:\"<>?"] {
        assert!(!needs_shaping(text, Direction::Auto), "{text:?} must keep the unshaped path");
        assert!(!needs_shaping(text, Direction::Ltr), "{text:?} must keep the unshaped path");
    }
    // Combinations the shaper may reorder, join or re-shape: always shaped.
    for text in ["café", "naïve", "אבג", "سلام", "नमस्ते", "ก่", "abc\u{202E}123", "a\u{0301}", "①②"] {
        assert!(needs_shaping(text, Direction::Auto), "{text:?} must be shaped");
    }
    // Control characters are not exempt even inside ASCII.
    assert!(needs_shaping("a\u{0}b", Direction::Auto));
    // A right-to-left paragraph can move even an ASCII blank (rule L1), so ASCII alone is not
    // enough there.
    assert!(needs_shaping("Hello", Direction::Rtl));
    assert!(needs_shaping(" ", Direction::Rtl));
}

#[test]
fn a_neutral_cell_follows_the_sheets_direction() {
    // Text with a strong character decides for itself; digits and punctuation follow the flag.
    assert_eq!(base_direction("abc", false), Direction::Auto);
    assert_eq!(base_direction("abc", true), Direction::Auto);
    assert_eq!(base_direction("אבג", true), Direction::Auto);
    assert_eq!(base_direction("123", false), Direction::Auto);
    assert_eq!(base_direction("123", true), Direction::Rtl);
    assert_eq!(base_direction(" - ", true), Direction::Rtl);
    assert_eq!(base_direction("", true), Direction::Rtl);
}

#[test]
fn bidi_mixed_cell_draws_the_runs_in_visual_order() {
    // The engine's run order is font independent; drawing the whole string needs one face that
    // covers Latin *and* Hebrew, so this asks the machine's fonts for one (Arial, Segoe UI, Tahoma
    // … all cover both) and skips where there is none.
    let text = "abc אבג 123";
    let Some((ctx, id, engine)) = covered_context('א', text) else {
        eprintln!("skipped: no installed face covers {text:?} (set CRAFT_FONTS_DIR to test it)");
        return;
    };
    let line = engine.line(&ctx, &id, text, base_direction(text, false), Color32::BLACK);
    let shaped = line.shaped.clone().expect("mixed-direction text is shaped");
    // The runs the engine returns: Latin, then the digits (an LTR island), then the Hebrew.
    assert_eq!(visual_slices(text, &shaped), ["abc", "123", "אבג"]);
    assert!((line.width - shaped.width).abs() < 0.01, "the line is measured by the shaper");
    // Runs advance by the engine's widths: the digit run starts where the Latin run ended, and
    // the line ends where the engine says it does.
    let latin = shaped.runs.first().expect("a run").width();
    assert!(latin > 0.0);
    // The drawn glyph order is the visual one: the Latin letters, then the digits, then the
    // Hebrew, left to right. The Hebrew glyphs come out in reverse logical order.
    let glyphs = drawn(&line.galley);
    let latin_letters: Vec<char> = glyphs.iter().take(3).map(|(c, _)| *c).collect();
    assert_eq!(latin_letters, ['a', 'b', 'c'], "{glyphs:?}");
    let digits: Vec<char> = glyphs.iter().skip(3).take(3).map(|(c, _)| *c).collect();
    assert_eq!(digits, ['1', '2', '3'], "digits keep their left-to-right reading order: {glyphs:?}");
    let hebrew: Vec<char> = glyphs.iter().skip(6).map(|(c, _)| *c).collect();
    assert_eq!(hebrew, ['ג', 'ב', 'א'], "the Hebrew words are drawn in reverse logical order: {glyphs:?}");
    // Every glyph sits at its engine pen position: the galley is one line, runs in order.
    let xs: Vec<f32> = glyphs.iter().map(|(_, x)| *x).collect();
    assert!(xs.windows(2).all(|w| w[0] <= w[1]), "glyphs advance left to right: {xs:?}");
    assert!((xs.last().copied().unwrap_or(0.0)) < line.width + 0.01);
}

#[test]
fn bidi_rtl_sheet_keeps_latin_and_numbers_reading_left_to_right() {
    let ctx = ubuntu_context();
    let text = "abc 123";
    let engine = ubuntu_engine();
    let line = engine.line(&ctx, &font(24.0), text, base_direction("123", true), Color32::BLACK);
    let shaped = line.shaped.clone().expect("a right-to-left base direction shapes neutral text");
    assert_eq!(shaped.base_direction, Direction::Rtl, "the sheet's flag chose the base direction");
    // Even in a right-to-left paragraph the number and the Latin keep their own (even) levels, so
    // they are drawn left to right; only the order of the chunks changes.
    let digits = shaped.runs.iter().find(|r| text.get(r.range.start as usize..r.range.end as usize).is_some_and(|s| s.contains('1')));
    let digits = digits.expect("a run holding the digits");
    assert!(digits.level.is_multiple_of(2), "a number must stay an LTR island");
    let starts: Vec<u32> = digits.glyphs.iter().map(|g| g.cluster_start).collect();
    assert!(starts.windows(2).all(|w| w[0] < w[1]), "2024 must read left to right: {starts:?}");
    // The drawn line still starts with the visually-leftmost run and advances rightwards.
    let glyphs = drawn(&line.galley);
    let xs: Vec<f32> = glyphs.iter().map(|(_, x)| *x).collect();
    assert!(xs.windows(2).all(|w| w[0] <= w[1]), "runs are painted left to right: {xs:?}");
}

#[test]
fn arabic_word_joins_and_is_measured_shaped() {
    let Some((ctx, id, engine)) = script_context(fonts::Script::Arabic, 'س') else {
        eprintln!("skipped: no installed Arabic face (set CRAFT_FONTS_DIR to test it)");
        return;
    };
    let text = "سلام";
    let line = engine.line(&ctx, &id, text, base_direction(text, false), Color32::BLACK);
    let shaped = line.shaped.clone().expect("Arabic is shaped");
    assert_eq!(shaped.base_direction, Direction::Rtl);
    // Joined forms: the glyph at a letter's cluster is not the isolated form a per-character
    // mapping (the face's cmap) would draw, so the shaper really did the work.
    let face = engine.chain(&ctx, &id).and_then(|c| c.first().cloned()).expect("the test face is in the chain");
    let mut differing = 0;
    for (at, c) in text.char_indices() {
        let joined = shaped.glyphs().find(|g| g.cluster_start as usize == at).map(|g| g.id);
        let isolated = nominal_glyph(&face, c);
        if joined.is_some_and(|j| Some(j) != isolated) {
            differing += 1;
        }
    }
    assert!(differing > 0, "no letter took a contextual form");
    // And the measurement the grid uses is the shaped one — the joined word is narrower than the
    // isolated letters are apart.
    let measured = engine.measure(&ctx, &id, text, Direction::Auto);
    assert!((measured - shaped.width).abs() < 0.01, "measured {measured} vs shaped {}", shaped.width);
    let naive: f32 = text.chars().map(|c| ctx.fonts_mut(|f| f.glyph_width(&id, c))).sum();
    assert!(naive > measured, "the joined word is narrower than the sum of its isolated letters ({measured} vs {naive})");
    // The painted line has one glyph per engine glyph.
    assert_eq!(drawn(&line.galley).len(), shaped.glyph_count());
}

#[test]
fn devanagari_and_thai_are_reordered_and_measured_shaped() {
    let Some((ctx, id, engine)) = script_context(fonts::Script::Devanagari, 'क') else {
        eprintln!("skipped: no installed Devanagari face (set CRAFT_FONTS_DIR to test it)");
        return;
    };
    let text = "क्षि";
    let line = engine.line(&ctx, &id, text, Direction::Ltr, Color32::BLACK);
    let shaped = line.shaped.clone().expect("Devanagari is shaped");
    // The vowel sign is drawn before its base: the drawn width is not the per-character sum.
    let naive: f32 = text.chars().map(|c| ctx.fonts_mut(|f| f.glyph_width(&id, c))).sum();
    assert!((line.width - shaped.width).abs() < 0.01);
    assert!((line.width - naive).abs() > 0.01, "the reordered cluster measures {} but the characters sum to {naive}", line.width);
    // The cluster map is monotonic: the clusters tile the text and caret positions rise to the
    // right.
    let map = engine.caret_map(text, &line).expect("a shaped line has a caret map");
    let xs: Vec<f32> = map.visual_positions().iter().map(|(_, x)| *x).collect();
    assert!(xs.windows(2).all(|w| w[0] <= w[1]), "caret positions rise visually: {xs:?}");
    let mut indexes: Vec<usize> = map.visual_positions().iter().map(|(i, _)| *i).collect();
    indexes.sort_unstable();
    indexes.dedup();
    assert_eq!(indexes.last().copied().unwrap_or(0), text.chars().count());

    // Thai: a tone mark advances nothing, so the drawn width is the base consonant's alone.
    let Some((ctx, id, engine)) = script_context(fonts::Script::Thai, 'ก') else {
        eprintln!("skipped: no installed Thai face (set CRAFT_FONTS_DIR to test it)");
        return;
    };
    let line = engine.line(&ctx, &id, "ก่", Direction::Ltr, Color32::BLACK);
    let shaped = line.shaped.clone().expect("Thai is shaped");
    let advances: Vec<f32> = shaped.glyphs().map(|g| g.advance).collect();
    assert_eq!(advances.len(), 2);
    assert_eq!(advances[1], 0.0, "GPOS positions the tone mark instead of advancing");
    assert!((line.width - advances[0]).abs() < 0.01, "the mark adds no width");
}

#[test]
fn caret_moves_through_visual_clusters_of_mixed_direction_text() {
    let text = "abc אבג";
    let Some((ctx, id, engine)) = covered_context('א', text) else {
        eprintln!("skipped: no installed face covers {text:?} (set CRAFT_FONTS_DIR to test it)");
        return;
    };
    let line = engine.line(&ctx, &id, text, Direction::Auto, Color32::BLACK);
    let map = engine.caret_map(text, &line).expect("a shaped line has a caret map");
    assert!(line.visual_order_differs(), "a bidi mix is not laid out in logical order");
    let chars = text.chars().count();
    // Walking right through the visual clusters reaches the end and never moves backwards on
    // screen; walking left comes back.
    let mut at = 0;
    let mut seen = vec![map.caret_x(at)];
    let mut visited = vec![at];
    for _ in 0..chars {
        let next = map.step(at, true);
        if next == at {
            break;
        }
        at = next;
        visited.push(at);
        seen.push(map.caret_x(at));
    }
    let mut all = visited.clone();
    all.sort_unstable();
    all.dedup();
    assert_eq!(all, (0..=chars).collect::<Vec<_>>(), "every cluster edge is a caret stop: {visited:?}");
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "the caret moves rightwards through the line: {seen:?}");
    assert!((seen.last().copied().unwrap_or(0.0) - map.caret_x(visited[visited.len() - 1])).abs() < 0.01);
    assert!(seen.last().copied().unwrap_or(0.0) >= line.width - 0.01, "the walk ends at the line's right edge: {seen:?} of {}", line.width);
    let mut back = at;
    for _ in 0..chars {
        let prev = map.step(back, false);
        if prev == back {
            break;
        }
        back = prev;
    }
    assert_eq!(back, 0, "and back to the start");
    // A click lands on the cluster under it: the x of every caret position maps back to that same
    // logical position (a click selects from the cluster's first byte).
    for (index, x) in map.visual_positions() {
        assert_eq!(map.index_at_x(x + 0.1), *index, "a click at {x} must land on {index}");
    }
}

#[test]
fn caret_in_a_right_to_left_word_starts_at_the_right() {
    let Some((ctx, id, engine)) = script_context(fonts::Script::Hebrew, 'א') else {
        eprintln!("skipped: no installed Hebrew face (set CRAFT_FONTS_DIR to test it)");
        return;
    };
    let text = "אבג";
    let line = engine.line(&ctx, &id, text, Direction::Auto, Color32::BLACK);
    let map = engine.caret_map(text, &line).expect("a shaped line has a caret map");
    assert!(line.visual_order_differs());
    // The first logical character is drawn rightmost, so the caret before it is at the right end;
    // the caret after the last character is at the left end.
    assert!((map.caret_x(0) - line.width).abs() < 0.01, "caret 0 at the right: {} vs {}", map.caret_x(0), line.width);
    assert!(map.caret_x(text.chars().count()).abs() < 0.01, "the end of the text is on the left");
    // Typing at caret 0 goes before the first character, however far right that is on screen.
    assert_eq!(map.index_at_x(line.width - 0.5), 0);
}

#[test]
fn autofit_measures_shaped_text_and_ascii_stays_bit_identical() {
    let ctx = ubuntu_context();
    let engine = ubuntu_engine();
    // ASCII: the golden measure is egui's own galley width, unchanged by this wiring, and the
    // shaped path is not even consulted.
    let plain = ctx.fonts_mut(|f| f.layout_no_wrap("Plain ASCII 123".to_owned(), font(24.0), Color32::PLACEHOLDER)).size().x;
    assert_eq!(engine.measure(&ctx, &font(24.0), "Plain ASCII 123", Direction::Auto).to_bits(), plain.to_bits(), "bit-identical");
    assert_eq!(engine.measure_for_autofit(&ctx, &font(24.0), "Plain ASCII 123", true), None, "the fast path keeps egui's measure");

    // A non-Latin cell: AutoFit measures the shaped line, not the sum of the per-character
    // advances (and not the engine's average-glyph estimate).
    let Some((ctx, id, engine)) = script_context(fonts::Script::Arabic, 'س') else {
        eprintln!("skipped: no installed Arabic face (set CRAFT_FONTS_DIR to test it)");
        return;
    };
    let text = "مبيعات 2024";
    let line = engine.line(&ctx, &id, text, Direction::Auto, Color32::BLACK);
    let exact = engine.measure_for_autofit(&ctx, &id, text, true).expect("the family has a face");
    assert!((exact - line.width).abs() < 0.01, "AutoFit measures the drawn width: {exact} vs {}", line.width);
    assert_eq!(engine.measure_for_autofit(&ctx, &id, text, false), Some(exact + 0.5), "the quick path keeps a little slack");
    let naive: f32 = text.chars().map(|c| ctx.fonts_mut(|f| f.glyph_width(&id, c))).sum();
    assert!((exact - naive).abs() > 0.01, "the shaped measure is the shaped one: {exact} vs the naive {naive}");
}

#[test]
fn autofit_falls_back_to_the_plain_measure_without_a_face() {
    // AutoFit's entry point (`Session::text_width`) goes through `cell_text_width`, which looks the
    // role family up in the app's font set: a family the set does not bind answers with the plain
    // measure instead of failing or shaping with bytes the renderer does not have.
    let ctx = ubuntu_context();
    let engine = ubuntu_engine();
    let missing = model::Font { name: "GridCraft No Such Family".into(), size: 11.0, ..Default::default() };
    let text = "مبيعات 2024";
    assert_eq!(
        engine.cell_text_width(&ctx, text, &missing, true),
        crate::theme::cell_text_width(&ctx, text, &missing, true),
        "the font-less answer is the existing measure"
    );
    assert!(engine.cell_text_width(&ctx, text, &missing, true) > 0.0);
}

#[test]
fn latin_with_accents_keeps_the_plain_editor_path() {
    let ctx = ubuntu_context();
    let engine = ubuntu_engine();
    let line = engine.line(&ctx, &font(24.0), "café", Direction::Auto, Color32::BLACK);
    assert!(line.shaped.is_some(), "non-ASCII text is shaped for painting");
    assert!(!line.visual_order_differs(), "but it is one LTR run in logical order, so the caret needs no map");
    assert!(engine.caret_map("café", &line).is_some());
}

#[test]
fn a_family_the_font_set_does_not_bind_falls_back_to_the_unshaped_galley() {
    let ctx = ubuntu_context();
    let engine = ubuntu_engine();
    // A bound family whose one face has no Arabic: the renderer would draw replacement glyphs at
    // other advances, so the grid must keep the unshaped path instead of the engine's positions.
    let line = engine.line(&ctx, &font(24.0), "سلام", Direction::Auto, Color32::BLACK);
    assert!(line.shaped.is_none(), "no covered face, no shaping");
    assert_eq!(line.galley.rows.len(), 1, "the fallback is a normal one-row galley");
    assert!(line.galley.size().x >= 0.0);
    assert!(engine.measure_for_autofit(&ctx, &font(24.0), "سلام", true).is_none());
    // An unbound family never reaches the engine either (egui itself would panic on it, which is
    // why `theme::cell_family` never names one).
    assert!(engine.shape(&ctx, &FontId::new(24.0, FontFamily::Name(MISSING.into())), "abc", Direction::Auto).is_none());
}

#[test]
fn text_over_the_engines_limit_is_refused_not_shaped() {
    let ctx = ubuntu_context();
    let engine = ubuntu_engine();
    let huge = "س".repeat(gridcraft_engine::core::text::MAX_TEXT_BYTES / 2 + 1);
    // The engine refuses it (never allocates gigabytes) and the UI falls back to the unshaped
    // path; asking for the shape directly is what a `line` call does first.
    assert!(engine.shape(&ctx, &font(11.0), &huge, Direction::Auto).is_none());
}

#[test]
fn the_shaping_warning_log_is_bounded() {
    let mut inner = Inner::default();
    for i in 0..(MAX_WARNINGS * 3) {
        inner.warn(&format!("key-{i}"), "a warning");
    }
    assert!(inner.warned.len() <= MAX_WARNINGS);
}

/// The face's `cmap` glyph for `c`: what a per-character (unshaped) mapping would draw.
fn nominal_glyph(face: &ChainFace, c: char) -> Option<u16> {
    use skrifa::MetadataProvider;
    let font = skrifa::FontRef::from_index(&face.bytes, face.index).ok()?;
    font.charmap().map(c).map(|g| u16::try_from(g.to_u32()).unwrap_or(0))
}
