//! Shaped cell text: the one text engine the UI owns, the deliberate unshaped fast path, and the
//! measurements and caret geometry the grid and the in-cell editor need.
//!
//! [`gridcraft_engine::core::text`] does the Unicode work (bidi levels and reordering, Arabic
//! joining, Indic reordering, Thai mark positioning, Latin ligatures). This module is the wiring:
//! it owns the one [`TextEngine`], registers the very font faces egui draws cells with — read from
//! the active [`egui::FontDefinitions`], so both engines shape the same bytes, at the same size —
//! and turns a [`ShapedText`] into an [`egui::Galley`] the painter uses.
//!
//! # Why a shaped line is assembled from per-run galleys
//! epaint has no glyph-id drawing API and lays text out without the bidi algorithm (it guesses one
//! direction for the whole string), so a shaped glyph cannot be drawn on its own. What it *can* do
//! is lay out one bidi run's text in that run's own direction — exactly the input the core engine
//! shaped for the run. So each run is laid out here with the same face bytes, pixel size and
//! direction, its glyphs are checked against the engine's advances, and the per-run rows and meshes
//! are folded into **one** galley in the engine's visual run order. One galley per cell keeps every
//! caller working unchanged: the RTL-sheet mirror translates one text shape per cell (several
//! would be shuffled), and underlines, rotation, clipping and `galley.size()` keep meaning what
//! they meant.
//!
//! # Where the faces come from
//! A cell's font is a role family (`fnt:<family>:<weight><slant>`, see
//! [`crate::theme::cell_family`]) whose `FontDefinitions` chain lists the font files egui paints
//! with, primary first. The chain is read from the live [`egui::Context`] each time it changes (a
//! font rebuild, a newly bound workbook font), the first face in chain order that covers every
//! character of the text is registered with the engine, and its bytes are the very ones egui draws
//! with — otherwise the measure and the drawing would disagree.
//!
//! # What is not shaped here
//! Text boxes and the PDF writer's `text_width` still go through their own layout (the PDF
//! *drawing* shapes through the same core engine in its own writer). This module covers the
//! one-line and word-wrapped cell text the grid paints — including rotated cells, which paint the
//! shaped galley turned — chart labels, and the in-cell editor's shaped line.
//!
//! # Never crash
//! A family egui has no data for, a font file the engine refuses, a string over the engine's
//! limit, or a run whose shaping epaint would not reproduce all fall back to the unshaped galley
//! after one warning — the drawing path that existed before shaping. Nothing here panics.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use egui::{Color32, Context, FontData, FontId, Galley, Painter, Pos2};
use gridcraft_engine::core::text::{Direction, FaceId, FontSize, ShapedText, TextEngine};
use gridcraft_engine::model;

/// How far egui's own layout of a run may differ from the engine's advance for it before the
/// shaped line is abandoned. Both shape the same bytes with HarfBuzz, so the sum matches exactly in
/// practice; this catches a font or engine change that would make the drawing disagree with the
/// measure (the `####` test, AutoFit and the overflow test all measure with the engine).
const MAX_RUN_WIDTH_DRIFT: f32 = 0.05;

/// Characters of a cell's text probed for font coverage. Coverage picks the chain face, so a
/// hostile 32 767-character cell costs a bounded number of `cmap` lookups.
const MAX_COVERAGE_PROBE: usize = 64;

/// Font files one role's chain may contribute. Longer chains (a workbook font, the base faces,
/// eight script fallbacks) are cut: past this the last entries only add replacements.
const MAX_CHAIN_FACES: usize = 12;

/// Distinct shaping warnings remembered, so a repaint loop cannot spam the log.
const MAX_WARNINGS: usize = 32;

/// Whether the shaper can change how `text` draws, so it has to go through the shaped path.
///
/// Conservative on purpose: it answers `false` only for printable ASCII (plus tab) in a paragraph
/// whose base direction is not right-to-left. That text has no bidi controls, no right-to-left or
/// combining characters, and cannot take a contextual form the unshaped path would miss, so the
/// unshaped galley is bit-identical to the shaped one — existing layout, golden and screenshot
/// tests stay valid.
///
/// An RTL paragraph is *not* exempt even for ASCII: rule N2/L1 moves a trailing space or dash to
/// the other end of the line, so `"a "` on a right-to-left sheet is not drawn as stored. When in
/// doubt this answers `true` and the engine decides.
pub fn needs_shaping(text: &str, direction: Direction) -> bool {
    direction.is_rtl() || !text.bytes().all(|b| b == b'\t' || (0x20..=0x7E).contains(&b))
}

/// The base direction to shape a cell's text with: the text's own first strong character wins
/// (bidi rules P2/P3), and text with none (digits, punctuation, blanks) follows the sheet's
/// right-to-left flag.
pub fn base_direction(text: &str, sheet_rtl: bool) -> Direction {
    if sheet_rtl && first_strong_is_neutral(text) { Direction::Rtl } else { Direction::Auto }
}

/// True when `text` has no strong (L, R or AL) character, so bidi rules P2/P3 derive nothing from
/// it. ASCII fast path: only the Latin letters are strong, every other ASCII character is not.
fn first_strong_is_neutral(text: &str) -> bool {
    !text.chars().any(is_strong)
}

/// A strong (left-to-right, right-to-left or Arabic-letter) character: the ones P2/P3 look at.
/// The ranges are the strong-direction scripts; everything else (digits, punctuation, spaces,
/// symbols) is neutral for the purpose of picking a paragraph direction.
fn is_strong(c: char) -> bool {
    let u = u32::from(c);
    (0x41..=0x5A).contains(&u) // A-Z
        || (0x61..=0x7A).contains(&u) // a-z
        || (0x0590..=0x08FF).contains(&u) // Hebrew, Arabic, Syriac, Thaana, NKo, …
        || (0xFB1D..=0xFDFF).contains(&u) // Hebrew/Arabic presentation forms
        || (0xFE70..=0xFEFF).contains(&u)
        || (0x10800..=0x10FFF).contains(&u) // Cypriot, Phoenician, Kharoshthi, …
        || (0x1E800..=0x1EFFF).contains(&u)
        || (0xC0..=0x24F).contains(&u) // Latin-1 Supplement and Latin Extended letters
        || (0x370..=0x3FF).contains(&u) // Greek
        || (0x400..=0x52F).contains(&u) // Cyrillic
        || (0x900..=0x97F).contains(&u) // Devanagari
}

/// One font face of a role's chain, as epaint holds it.
#[derive(Clone)]
struct ChainFace {
    /// egui's font key, for the warning message and the byte cache.
    key: String,
    /// Identity of the `Arc<FontData>` the bytes came from (a rebuilt font set changes it).
    source: usize,
    bytes: std::sync::Arc<[u8]>,
    index: u32,
}

/// The faces one role family resolves to, and the font definitions they were read from.
struct Role {
    /// Hash of the chain's `(key, Arc, index)`, so a font rebuild re-reads the files.
    signature: u64,
    faces: Vec<ChainFace>,
    /// Registered faces by chain position: `None` until shaped, `Some(None)` when the engine
    /// refused the bytes (never retried).
    registered: Vec<Option<Option<FaceId>>>,
}

#[derive(Default)]
struct Inner {
    engine: TextEngine,
    roles: HashMap<String, Role>,
    /// Font bytes by `(key, source)`, so a repaint does not copy a 20 MB font again.
    bytes: HashMap<(String, usize), std::sync::Arc<[u8]>>,
    /// Shaping warnings already printed.
    warned: HashSet<String>,
}

impl Inner {
    fn warn(&mut self, key: &str, message: &str) {
        if self.warned.len() >= MAX_WARNINGS {
            self.warned.clear();
        }
        if self.warned.insert(key.to_owned()) {
            log::warn!("text: {message}");
        }
    }
}

/// GridCraft's text engine and the faces egui draws cells with.
///
/// One per app ([`crate::SheetApp::text`]): the shaped lines the grid paints, the measures AutoFit
/// and the `####` test use, and the cluster maps the in-cell editor's caret moves through all come
/// from the same engine, so they cannot disagree.
pub struct GridText {
    inner: Mutex<Inner>,
}

impl Default for GridText {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for GridText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let stats = self.lock().engine.cache_stats();
        f.debug_struct("GridText").field("faces", &stats.faces).field("shapes", &stats.shapes).finish()
    }
}

/// One shaped cluster of a paragraph: its source bytes and the pen position after it.
struct Cluster {
    start: usize,
    end: usize,
    /// Pen position after this cluster, in drawing units, in logical order.
    cum: f32,
}

/// The paragraph's clusters in logical order, with the pen position after each. Glyphs inside a
/// cluster (a base and its marks) are summed into that cluster.
fn cluster_prefix(text: &str, shaped: &ShapedText) -> Vec<Cluster> {
    let mut by_start: std::collections::BTreeMap<usize, (usize, f32)> = std::collections::BTreeMap::new();
    for glyph in shaped.glyphs() {
        let start = glyph.cluster_start as usize;
        let end = glyph.cluster_end as usize;
        if start >= end || end > text.len() || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            continue;
        }
        let e = by_start.entry(start).or_insert((end, 0.0));
        e.0 = e.0.max(end);
        e.1 += glyph.advance;
    }
    let mut cum = 0.0;
    let mut out = Vec::with_capacity(by_start.len());
    for (start, (end, advance)) in by_start {
        cum += advance;
        out.push(Cluster { start, end, cum });
    }
    out
}

/// The pen position just before byte `at` (or of the last cluster that starts before it).
fn width_at(clusters: &[Cluster], at: usize) -> f32 {
    let i = clusters.partition_point(|c| c.start < at);
    if i == 0 { 0.0 } else { clusters.get(i - 1).map(|c| c.cum).unwrap_or(0.0) }
}

/// Longest wrapped cell text the shaper wraps itself; longer text keeps the unshaped wrap (the
/// wrap is recomputed every frame, and a 32,000-character cell must not cost more than it did).
const MAX_WRAP_BYTES: usize = 16_384;

/// Where a paragraph may break: after a space, tab or hyphen, and on either side of a wide
/// (CJK, Hangul, kana, fullwidth) character, which needs no space to break. Positions are byte
/// offsets just past the character, and the end of the paragraph is not a break.
fn break_positions(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut prev_wide = false;
    for (i, ch) in text.char_indices() {
        let end = i + ch.len_utf8();
        let wide = is_wide(ch);
        if end < text.len() && (prev_wide || wide || matches!(ch, ' ' | '\t' | '-' | '\u{2010}' | '\u{2013}')) {
            out.push(end);
        }
        prev_wide = wide;
    }
    out
}

/// A character that may be broken between (CJK ideographs, kana, Hangul and fullwidth forms).
fn is_wide(c: char) -> bool {
    let u = u32::from(c);
    matches!(
        u,
        0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x20000..=0x2FA1F
    )
}

/// One line of cell text, ready to measure and to paint.
pub struct Line {
    /// Width in points: what the `####` overflow test, AutoFit and the editor's size use.
    pub width: f32,
    /// The single galley the painter draws. For a shaped line its glyphs are in visual order.
    pub galley: std::sync::Arc<Galley>,
    /// The engine's shaping, when the line went through it; `None` on the fast path.
    pub shaped: Option<std::sync::Arc<ShapedText>>,
}

impl Line {
    /// The line's height (one row of the font at this size).
    pub fn height(&self) -> f32 {
        self.galley.size().y
    }

    /// Paint the line with its top-left corner at `pos`.
    pub fn paint(&self, painter: &Painter, pos: Pos2, color: Color32) {
        painter.galley(pos, self.galley.clone(), color);
    }

    /// True when the glyphs are not in logical order — right-to-left text, a bidi mix, or a
    /// reordered/merged cluster — so a caret character index does not map straight to an x the way
    /// egui's own text widget assumes, and [`CaretMap`] has to do the mapping.
    pub fn visual_order_differs(&self) -> bool {
        let Some(shaped) = &self.shaped else { return false };
        if shaped.base_direction.is_rtl() || shaped.runs.len() > 1 {
            return true;
        }
        let mut last: Option<u32> = None;
        shaped.glyphs().any(|g| {
            let differs = last.is_some_and(|l| g.cluster_start <= l);
            last = Some(g.cluster_start);
            differs
        })
    }
}

impl GridText {
    pub fn new() -> Self {
        Self { inner: Mutex::new(Inner::default()) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Shape `text` as one line for `font`, in the base direction `direction`, and lay it out for
    /// painting. Never fails: text the shaper cannot help with comes back as the unshaped galley.
    pub fn line(&self, ctx: &Context, font: &FontId, text: &str, direction: Direction, color: Color32) -> Line {
        if needs_shaping(text, direction)
            && let Some(line) = self.shaped_line(ctx, font, text, direction, color)
        {
            return line;
        }
        let galley = ctx.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font.clone(), color));
        Line { width: galley.size().x, galley, shaped: None }
    }

    /// The shaped lines a cell's text wraps into at `width` points — one per explicit line break
    /// and one per wrap — or `None` when the text needs no shaping.
    ///
    /// `None` keeps the caller's unshaped `LayoutJob` wrap: printable ASCII (line breaks aside)
    /// has no bidi reordering, no joining and no marks, so its wrap is bit-identical to this one
    /// and existing layout and screenshot tests stay valid.
    ///
    /// A paragraph is shaped once to measure its clusters, then each line is shaped on its own, so
    /// a wrapped line gets the same bidi resolution, joining and mark positioning as a one-line
    /// cell. Line breaks follow spaces, tabs and hyphens, and are allowed between CJK characters;
    /// a single word wider than the cell breaks at the furthest cluster boundary that fits, so a
    /// line always makes progress.
    pub fn wrap(&self, ctx: &Context, font: &FontId, text: &str, direction: Direction, color: Color32, width: f32) -> Option<Vec<Line>> {
        let flat: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
        if width <= 0.0 || flat.is_empty() || text.len() > MAX_WRAP_BYTES || !needs_shaping(&flat, direction) {
            return None;
        }
        let mut out: Vec<Line> = Vec::new();
        for para in text.split('\n') {
            let para = para.strip_suffix('\r').unwrap_or(para);
            if para.is_empty() {
                out.push(self.line(ctx, font, "", direction, color));
                continue;
            }
            // One shaped line when it fits: no wrapping work at all.
            if self.measure(ctx, font, para, direction) <= width {
                out.push(self.line(ctx, font, para, direction, color));
                continue;
            }
            let shaped = self.shape(ctx, font, para, direction)?;
            let clusters = cluster_prefix(para, &shaped);
            let breaks = break_positions(para);
            let mut start = 0usize;
            while start < para.len() {
                let start_w = width_at(&clusters, start);
                let from = breaks.partition_point(|p| *p <= start);
                let fitting = breaks[from..].iter().copied().take_while(|p| width_at(&clusters, *p) - start_w <= width).last();
                let end = match fitting {
                    Some(b) => b,
                    None => {
                        // No break opportunity fits: the furthest cluster end that does. One
                        // cluster always goes on the line, even a cluster wider than the cell.
                        let bounded = clusters
                            .iter()
                            .filter(|c| c.start >= start)
                            .map(|c| c.end)
                            .take_while(|e| width_at(&clusters, *e) - start_w <= width)
                            .last();
                        bounded.or_else(|| clusters.iter().find(|c| c.start >= start).map(|c| c.end)).unwrap_or(para.len())
                    }
                };
                let end = end.clamp(start, para.len());
                out.push(self.line(ctx, font, para.get(start..end).unwrap_or(""), direction, color));
                if end <= start {
                    break;
                }
                start = end;
            }
        }
        Some(out)
    }

    /// The base direction the shaper resolved for the first paragraph of `text`, when it shapes
    /// it. The wrapped path uses it to align general text by its own reading direction.
    pub fn direction_of(&self, ctx: &Context, font: &FontId, text: &str, direction: Direction) -> Option<Direction> {
        let first = text.split('\n').next().unwrap_or(text);
        self.shape(ctx, font, first, direction).map(|s| s.base_direction)
    }

    /// The width of one line, in points, as the grid draws it. This is the measure the `####`
    /// overflow test, Shrink to fit, the editor's size and AutoFit (`theme::cell_text_width`) use.
    pub fn measure(&self, ctx: &Context, font: &FontId, text: &str, direction: Direction) -> f32 {
        if needs_shaping(text, direction)
            && let Some(width) = self.shaped_width(ctx, font, text, direction)
        {
            return width;
        }
        ctx.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font.clone(), Color32::PLACEHOLDER).size().x)
    }

    /// Cell text width for AutoFit: the shaped width of the line the grid would draw, in points.
    ///
    /// Falls back to [`crate::theme::cell_text_width`] (the unshaped measure) when the role family
    /// has no face this engine can use, so AutoFit still answers on a machine without the font.
    pub fn cell_text_width(&self, ctx: &Context, text: &str, font: &model::Font, exact: bool) -> f32 {
        let family = crate::theme::cell_family(&font.name, font.bold, font.italic);
        let id = FontId::new(font.size * 96.0 / 72.0, family);
        self.measure_for_autofit(ctx, &id, text, exact).unwrap_or_else(|| crate::theme::cell_text_width(ctx, text, font, exact))
    }

    /// The shaped width AutoFit uses for a font family this engine has a face for, or `None` when
    /// the family is not bound to font data (the caller falls back to the plain measure).
    ///
    /// `exact` false adds half a point: AutoFit's quick path promises a measure that is never
    /// narrower than the drawn line, and the shaped width is what the line is drawn at.
    pub fn measure_for_autofit(&self, ctx: &Context, font: &FontId, text: &str, exact: bool) -> Option<f32> {
        // AutoFit has no sheet: a neutral cell is measured as left to right. Only the base direction
        // differs, not the glyphs, so the width is the one the drawn line has up to a trailing blank.
        let direction = base_direction(text, false);
        if !needs_shaping(text, direction) {
            return None;
        }
        self.shaped_width(ctx, font, text, direction).map(|width| if exact { width } else { width + 0.5 })
    }

    /// The engine's width for `text`, or `None` when the UI cannot shape it (no face for the
    /// family, a font the engine refuses, a string over the engine's limit).
    fn shaped_width(&self, ctx: &Context, font: &FontId, text: &str, direction: Direction) -> Option<f32> {
        self.shape(ctx, font, text, direction).map(|s| s.width)
    }

    /// Shape `text` with the face egui would draw the cell with.
    ///
    /// `None` when the line cannot be drawn from the engine's shaping: no family bound to the font
    /// set, no chain face that covers the text (the renderer would substitute a replacement glyph
    /// at another advance, so measure and drawing would disagree), a font the engine refuses, or a
    /// string over the engine's limit.
    fn shape(&self, ctx: &Context, font: &FontId, text: &str, direction: Direction) -> Option<std::sync::Arc<ShapedText>> {
        if text.is_empty() {
            return None;
        }
        let chain = self.chain(ctx, font)?;
        let pick = pick_face(&chain, text)?;
        let mut inner = self.lock();
        let face = inner.register(font, &chain[..], pick)?;
        let size = FontSize::em(font.size.max(0.0));
        match inner.engine.shape(text, direction, face, size) {
            Ok(shaped) => Some(shaped),
            Err(err) => {
                inner.warn("shape", &format!("cannot shape {:?} in {font:?}: {err}; drawing it unshaped", clip(text)));
                None
            }
        }
    }

    /// The font faces of `font`'s role family, read from egui's live font definitions.
    fn chain(&self, ctx: &Context, font: &FontId) -> Option<Vec<ChainFace>> {
        let key = font.family.to_string();
        let data: Vec<(String, std::sync::Arc<FontData>)> = ctx.fonts(|f| {
            let defs = f.definitions();
            let keys = defs.families.get(&font.family)?;
            Some(keys.iter().take(MAX_CHAIN_FACES).filter_map(|k| defs.font_data.get(k).map(|d| (k.clone(), d.clone()))).collect())
        })?;
        if data.is_empty() {
            return None;
        }
        let signature = chain_signature(&data);
        let mut inner = self.lock();
        if let Some(role) = inner.roles.get(&key)
            && role.signature == signature
        {
            return Some(role.faces.clone());
        }
        let mut faces: Vec<ChainFace> = Vec::with_capacity(data.len());
        for (key, fd) in data {
            let source = std::sync::Arc::as_ptr(&fd) as usize;
            let bytes = match inner.bytes.get(&(key.clone(), source)) {
                Some(b) => b.clone(),
                None => {
                    let bytes: std::sync::Arc<[u8]> = std::sync::Arc::from(fd.font.as_ref());
                    inner.bytes.insert((key.clone(), source), bytes.clone());
                    bytes
                }
            };
            faces.push(ChainFace { key, source, bytes, index: fd.index });
        }
        // A font rebuild changes every `Arc`, so the byte cache would keep the old files for ever.
        if inner.bytes.len() > 4 * MAX_CHAIN_FACES {
            let live: HashSet<(String, usize)> = faces.iter().map(|f| (f.key.clone(), f.source)).collect();
            inner.bytes.retain(|k, _| live.contains(k));
        }
        inner.roles.insert(key.clone(), Role { signature, faces: faces.clone(), registered: Vec::new() });
        if inner.roles.len() > 8 {
            let keep = key.clone();
            inner.roles.retain(|k, _| k == &keep);
        }
        Some(faces)
    }

    /// Shape `text` and lay its runs out as one galley, in visual order.
    fn shaped_line(&self, ctx: &Context, font: &FontId, text: &str, direction: Direction, color: Color32) -> Option<Line> {
        let shaped = self.shape(ctx, font, text, direction)?;
        if shaped.is_empty() {
            return None;
        }
        // The runs, their slices, their widths and whether epaint would shape each the same way.
        let mut runs: Vec<(f32, String, f32, bool)> = Vec::with_capacity(shaped.runs.len());
        let mut x = 0.0;
        for run in &shaped.runs {
            let slice = text.get(run.range.start as usize..run.range.end as usize)?;
            runs.push((x, slice.to_owned(), run.width(), epaint_direction_matches(slice, run.direction())));
            x += run.width();
        }
        let mut laid_out: Vec<(f32, std::sync::Arc<Galley>)> = Vec::with_capacity(runs.len());
        for (x, slice, engine_width, same_direction) in runs {
            if !same_direction {
                self.lock().warn("direction", &format!("epaint would shape {:?} in another direction; drawing it unshaped", clip(&slice)));
                return None;
            }
            let galley = ctx.fonts_mut(|f| f.layout_no_wrap(slice.clone(), font.clone(), color));
            if (galley.size().x - engine_width).abs() > MAX_RUN_WIDTH_DRIFT {
                self.lock().warn(
                    "width",
                    &format!(
                        "the renderer measures {:?} at {} pt but the engine at {engine_width} pt; drawing it unshaped",
                        clip(&slice),
                        galley.size().x
                    ),
                );
                return None;
            }
            laid_out.push((x, galley));
        }
        let base = ctx.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font.clone(), color));
        let galley = combine(base, &laid_out, shaped.width).or_else(|| {
            self.lock().warn("combine", "the shaped runs cannot be folded into one galley; drawing the text unshaped");
            None
        })?;
        Some(Line { width: shaped.width, galley, shaped: Some(shaped) })
    }

    /// The caret geometry of `line` (the shaped one: a plain line needs none).
    pub fn caret_map(&self, text: &str, line: &Line) -> Option<CaretMap> {
        let shaped = line.shaped.clone()?;
        Some(CaretMap::new(text, &shaped))
    }

    /// What the cache holds right now, for tests and for the control channel.
    pub fn cache_stats(&self) -> gridcraft_engine::core::text::CacheStats {
        self.lock().engine.cache_stats()
    }
}

impl Inner {
    /// The engine's handle for `chain[pick]`, registered on first use.
    fn register(&mut self, font: &FontId, chain: &[ChainFace], pick: usize) -> Option<FaceId> {
        let key = font.family.to_string();
        let source = chain.get(pick)?;
        if let Some(done) = self.roles.get(&key).and_then(|r| r.registered.get(pick).copied().flatten()) {
            return done;
        }
        let registered = match self.engine.add_face_at(source.bytes.clone(), source.index) {
            Ok(face) => Some(face),
            Err(err) => {
                let message = format!("cannot register the font file for {:?} ({}): {err}", source.key, font.family);
                self.warn("face", &message);
                None
            }
        };
        if let Some(role) = self.roles.get_mut(&key) {
            while role.registered.len() < chain.len() {
                role.registered.push(None);
            }
            if let Some(slot) = role.registered.get_mut(pick) {
                *slot = Some(registered);
            }
        }
        registered
    }
}

/// Whether epaint, laying `slice` out on its own, would shape it in the run's `direction`.
///
/// epaint derives a run's direction from its first strong character and takes left-to-right when
/// there is none (HarfBuzz segment guessing). The bidi rules guarantee that a run at an even level
/// holds no right-to-left strong character and one at an odd level holds no left-to-right one, so
/// the guess only differs for a run of nothing but neutrals at an odd level — which epaint would
/// draw left to right. Answering `false` there falls back to the unshaped galley instead of drawing
/// that run mirrored.
fn epaint_direction_matches(slice: &str, direction: Direction) -> bool {
    !(direction.is_rtl() && first_strong_is_neutral(slice))
}

/// Hash of a chain's identity: the same font files in the same order hash the same.
fn chain_signature(data: &[(String, std::sync::Arc<FontData>)]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (key, fd) in data {
        key.hash(&mut hasher);
        (std::sync::Arc::as_ptr(fd) as usize).hash(&mut hasher);
        fd.index.hash(&mut hasher);
    }
    hasher.finish()
}

/// The chain face that covers every character of `text`, the earliest on a tie; `None` when no
/// face covers them all (the renderer would substitute glyphs the engine cannot position).
///
/// The probe is bounded ([`MAX_COVERAGE_PROBE`] characters spread over the string), so a hostile
/// 32 767-character cell costs a few dozen `cmap` lookups and not a scan of the whole string.
fn pick_face(chain: &[ChainFace], text: &str) -> Option<usize> {
    let total = text.chars().count();
    let probe: Vec<char> = if total <= MAX_COVERAGE_PROBE {
        text.chars().collect()
    } else {
        let step = total / MAX_COVERAGE_PROBE;
        text.chars().step_by(step.max(1)).take(MAX_COVERAGE_PROBE).collect()
    };
    let mut best: Option<(usize, usize)> = None;
    for (i, face) in chain.iter().enumerate() {
        let covered = coverage(face, &probe);
        if best.is_none_or(|(_, c)| covered > c) {
            best = Some((i, covered));
        }
        if covered == probe.len() {
            return Some(i);
        }
    }
    None
}

/// How many of `probe` the face maps to a real glyph. A file that does not parse covers nothing.
fn coverage(face: &ChainFace, probe: &[char]) -> usize {
    use skrifa::MetadataProvider;
    let Ok(font) = skrifa::FontRef::from_index(&face.bytes, face.index) else { return 0 };
    let charmap = font.charmap();
    probe.iter().filter(|c| charmap.map(**c).is_some()).count()
}

/// Fold per-run galleys into one galley, in the order given, with the engine's total width.
///
/// The runs were laid out as single-direction slices, so their rows are already in visual order
/// left to right; translating each one's glyphs and mesh by the run's x gives the whole line.
fn combine(base: std::sync::Arc<Galley>, runs: &[(f32, std::sync::Arc<Galley>)], width: f32) -> Option<std::sync::Arc<Galley>> {
    let mut galley = (*base).clone();
    if galley.rows.len() != 1 {
        return None;
    }
    let first = galley.rows.first()?;
    let mut row = (*first.row).clone();
    if row.glyphs.is_empty() {
        return None;
    }
    let height = row.size.y;
    row.glyphs.clear();
    let mut mesh = egui::Mesh::default();
    let mut bounds = egui::Rect::NOTHING;
    let mut glyph_vertices = 0usize;
    for (dx, run) in runs {
        let Some(run_row) = run.rows.first() else { continue };
        let vert_base = mesh.vertices.len() as u32;
        for v in &run_row.visuals.mesh.vertices {
            mesh.vertices.push(egui::epaint::Vertex { pos: v.pos + egui::vec2(*dx, 0.0), uv: v.uv, color: v.color });
        }
        glyph_vertices += run_row.visuals.mesh.vertices.len();
        for i in &run_row.visuals.mesh.indices {
            mesh.indices.push(i + vert_base);
        }
        for glyph in &run_row.glyphs {
            let mut shifted = *glyph;
            shifted.pos.x += dx;
            shifted.first_vertex += vert_base;
            row.glyphs.push(shifted);
        }
        bounds = bounds.union(run_row.visuals.mesh_bounds.translate(egui::vec2(*dx, 0.0)));
    }
    let vertices = mesh.vertices.len();
    let indices = mesh.indices.len();
    row.size = egui::vec2(width.max(0.0), height);
    row.visuals = egui::epaint::text::RowVisuals { mesh, mesh_bounds: bounds, glyph_index_start: 0, glyph_vertex_range: 0..glyph_vertices };
    let pos = first.pos;
    galley.rows = vec![egui::epaint::text::PlacedRow { pos, row: std::sync::Arc::new(row), ends_with_newline: false }];
    galley.rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width.max(0.0), height));
    galley.mesh_bounds = bounds;
    galley.num_vertices = vertices;
    galley.num_indices = indices;
    galley.elided = false;
    Some(std::sync::Arc::new(galley))
}

/// A short, printable version of `text` for a log message.
fn clip(text: &str) -> String {
    text.chars().take(24).collect()
}

// ---------------------------------------------------------------------------- caret mapping

/// Where the caret sits in a shaped line, and what a click at an x selects.
///
/// The editor's buffer stays logical: `caret` is a character index into the text. In a shaped line
/// the glyphs are in *visual* order, so character index and x do not agree any more — that is what
/// this map is for. A caret only sits at a cluster edge (a ligature or a base with its mark is one
/// cluster), which is also where a click lands: on the cluster's first byte.
#[derive(Clone, Debug, Default)]
pub struct CaretMap {
    /// Caret positions in visual order: `(character index, x)`, x rising.
    positions: Vec<(usize, f32)>,
    /// The drawn span of every cluster, in visual order: `(first character, one past the last,
    /// x0, x1)`. A selection in mixed-direction text is a set of visual islands built from these.
    spans: Vec<(usize, usize, f32, f32)>,
    /// The text's character count, so the caret can reach the end.
    chars: usize,
    width: f32,
}

impl CaretMap {
    /// Build the map from one shaping: every cluster edge in visual order.
    pub fn new(text: &str, shaped: &ShapedText) -> Self {
        let mut positions: Vec<(usize, f32)> = Vec::new();
        let mut spans: Vec<(usize, usize, f32, f32)> = Vec::new();
        let mut pen = 0.0f32;
        for run in &shaped.runs {
            let rtl = run.direction().is_rtl();
            // A cluster's glyphs are consecutive and share its byte range (a mark, a ligature).
            let mut cluster: Option<(u32, u32, f32, f32)> = None;
            for glyph in &run.glyphs {
                let end = pen + glyph.advance.max(0.0);
                let same = cluster.is_some_and(|(start, _, _, _)| start == glyph.cluster_start);
                if !same {
                    if let Some((start, stop, x0, x1)) = cluster {
                        push_cluster(&mut positions, &mut spans, text, start, stop, x0, x1, rtl);
                    }
                    cluster = Some((glyph.cluster_start, glyph.cluster_end, pen, end));
                } else if let Some((_, stop, x0, x1)) = cluster.as_mut() {
                    *stop = (*stop).max(glyph.cluster_end);
                    *x0 = x0.min(pen);
                    *x1 = x1.max(end);
                }
                pen = end;
            }
            if let Some((start, stop, x0, x1)) = cluster {
                push_cluster(&mut positions, &mut spans, text, start, stop, x0, x1, rtl);
            }
        }
        positions.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        // One x per character index: at a directional boundary the same logical position has two
        // visual places (the end of the run on the left and the start of the run on the right);
        // the caret takes the right-hand one, so moving rightwards into a run lands at the end of
        // the run's visual span rather than at its far edge.
        let mut rightmost: HashMap<usize, f32> = HashMap::new();
        for (index, x) in positions {
            let slot = rightmost.entry(index).or_insert(x);
            if x > *slot {
                *slot = x;
            }
        }
        let mut positions: Vec<(usize, f32)> = rightmost.into_iter().collect();
        positions.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        Self { positions, spans, chars: text.chars().count(), width: shaped.width.max(0.0) }
    }

    /// Every caret position in visual order: `(character index, x)` with x rising.
    pub fn visual_positions(&self) -> &[(usize, f32)] {
        &self.positions
    }

    /// The visual spans the selected characters `[lo, hi)` cover, in visual order: the cluster
    /// rectangles a selection highlight is painted into. Empty for an empty selection.
    pub fn selection_spans(&self, lo: usize, hi: usize) -> Vec<(f32, f32)> {
        if lo >= hi {
            return Vec::new();
        }
        self.spans.iter().filter(|(a, b, _, _)| *a < hi && lo < *b).map(|(_, _, x0, x1)| (*x0, *x1)).collect()
    }

    /// The x of the caret at character index `index` (clamped to the text).
    pub fn caret_x(&self, index: usize) -> f32 {
        let index = index.min(self.chars);
        match self.positions.iter().find(|(i, _)| *i == index) {
            Some((_, x)) => *x,
            None => self.nearest(index).1,
        }
    }

    /// The character index the caret moves to one step left or right of `index` through the *visual*
    /// clusters: `→` in Arabic moves towards what reads next, not towards the next stored byte.
    ///
    /// A caret only stops at cluster edges, so a step never lands inside a ligature or between a
    /// base and its mark; at a directional boundary it takes the right-hand side of the boundary
    /// (see [`CaretMap::new`]), which is where a reader moving rightwards arrives.
    pub fn step(&self, index: usize, forward: bool) -> usize {
        let index = index.min(self.chars);
        if self.positions.is_empty() {
            return index;
        }
        let Some(at) = self.positions.iter().position(|(i, _)| *i == index) else {
            // Not on a cluster edge (a caret inside a cluster): step from the nearest one.
            let (i, _) = self.nearest(index);
            return self.step(i, forward);
        };
        let next = if forward { self.positions.get(at + 1) } else { at.checked_sub(1).and_then(|p| self.positions.get(p)) };
        next.map_or(index, |(i, _)| *i)
    }

    /// The character index a click at visual x lands on: the first byte of the cluster under it.
    pub fn index_at_x(&self, x: f32) -> usize {
        let Some((index, _)) = self.positions.iter().min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs())) else {
            return 0;
        };
        *index
    }

    /// The line's width in points.
    pub fn width(&self) -> f32 {
        self.width
    }

    /// The caret position nearest character index `index` by visual order (for a caret that is
    /// inside a cluster: between a base and its mark).
    fn nearest(&self, index: usize) -> (usize, f32) {
        self.positions
            .iter()
            .min_by(|a, b| a.0.abs_diff(index).cmp(&b.0.abs_diff(index)).then(a.1.total_cmp(&b.1)))
            .map_or((0, 0.0), |(i, x)| (*i, *x))
    }
}

/// Add one cluster's caret edges and its drawn span. The edges go in *visual* order: the left
/// edge first, then the right one. A left-to-right cluster starts at its left edge; a
/// right-to-left one starts at its right edge.
#[allow(clippy::too_many_arguments)]
fn push_cluster(
    out: &mut Vec<(usize, f32)>,
    spans: &mut Vec<(usize, usize, f32, f32)>,
    text: &str,
    start_byte: u32,
    end_byte: u32,
    x0: f32,
    x1: f32,
    rtl: bool,
) {
    let start = text.get(..start_byte as usize).map_or(0, |s| s.chars().count());
    let end = text.get(..end_byte as usize).map_or(start, |s| s.chars().count()).max(start);
    spans.push((start, end, x0, x1));
    if rtl {
        out.push((end, x0));
        out.push((start, x1));
    } else {
        out.push((start, x0));
        out.push((end, x1));
    }
}

/// The glyphs a galley holds as `(char, x, real)`, where `real` is true for a drawn glyph (a
/// zero-width continuation glyph for the rest of a ligature's characters is not drawn).
#[cfg(test)]
fn galley_glyphs(galley: &Galley) -> Vec<(char, f32, bool)> {
    galley.rows.iter().flat_map(|r| r.glyphs.iter().map(|g| (g.chr, g.pos.x, !g.uv_rect.is_nothing()))).collect()
}

/// Font definitions with one extra family bound to `bytes`, for tests that must not depend on an
/// installed face.
#[cfg(test)]
pub(crate) fn test_context(family: &str, bytes: &[u8], index: u32) -> Context {
    let ctx = Context::default();
    let mut defs = egui::FontDefinitions::default();
    let mut fd = FontData::from_owned(bytes.to_vec());
    fd.index = index;
    defs.font_data.insert(family.to_owned(), std::sync::Arc::new(fd));
    let fam = egui::FontFamily::Name(family.into());
    defs.families.insert(fam, vec![family.to_owned()]);
    ctx.set_fonts(defs);
    let mut out = ctx.run_ui(Default::default(), |_ui| {});
    out.textures_delta.clear();
    ctx
}

#[cfg(test)]
mod tests;
