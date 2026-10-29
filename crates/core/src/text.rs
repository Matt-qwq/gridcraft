//! Text shaping and the Unicode bidirectional algorithm: a string plus a font in, positioned
//! glyphs out.
//!
//! Nothing here knows about egui, the grid or the PDF writer. `core` sits at L0, so both callers
//! use this module and the text a user sees on screen is the same text an export writes.
//!
//! # The pipeline
//! 1. The Unicode bidi algorithm ([`unicode_bidi`]) splits the text into paragraphs and gives
//!    every byte a directional level (rules P2/P3, X1–X10, W1–W7, N0–N2, I1–I2).
//! 2. Its visual runs reorder the level runs of a line into display order (rules L1/L2).
//! 3. Each run is shaped with `rustybuzz` in its own direction, which applies the script shaping:
//!    Arabic joining and contextual forms, Indic reordering, Thai mark positioning, Latin
//!    ligatures, and mirroring for characters such as brackets in RTL text.
//! 4. Every glyph keeps the byte range of the source cluster it came from and the positioning
//!    offsets a mark or a reordered cluster needs.
//!
//! Runs and glyphs are returned in **visual order, left to right**, with positive horizontal
//! advances: that is what a renderer draws and what a PDF content stream writes. A glyph's
//! [`cluster_start`](Glyph::cluster_start)/[`cluster_end`](Glyph::cluster_end) are **logical**
//! UTF-8 byte offsets into the source string, so a caller can map a hit position back to a
//! character.
//!
//! # Sizing
//! Glyph advances and offsets are returned in *drawing units*. [`FontSize`] says how many drawing
//! units one em is: a PDF writer passes the point size and a scale of `1.0` and gets points; the
//! grid passes pixels and the display scale and gets pixels.
//!
//! # Caching
//! [`TextEngine`] keeps the faces it was given (bounded, FIFO) and the results of recent shapings
//! keyed by `(text, face, direction, size)` (bounded by entries, glyphs and bytes). A cache hit is
//! an [`Arc`] clone. Strings over 4 KiB are shaped but not cached, so a cell holding a novel does
//! not push the working set out.
//!
//! ```
//! use gridcraft_core::text::{Direction, FontSize, TextEngine, TextError};
//!
//! let mut engine = TextEngine::new();
//! // The bytes are validated once, here: a phone book is not a font, and nothing panics.
//! assert_eq!(engine.add_face(b"not a font".to_vec()), Err(TextError::InvalidFont { index: 0 }));
//! assert_eq!(engine.cache_stats().faces, 0);
//! ```
//!
//! # What is not here
//! Line breaking, word wrapping, justification and vertical text. The caller decides where a line
//! ends; this module shapes what it is given as one line.

use std::{
    collections::{HashMap, VecDeque},
    fmt,
    hash::{DefaultHasher, Hash, Hasher},
    ops::Range,
    sync::Arc,
};

use rustybuzz::{BufferClusterLevel, Direction as HbDirection, Face as HbFace, UnicodeBuffer};
use thiserror::Error;
use unicode_bidi::{BidiInfo, Level};

/// Glyphs the shape cache may hold before the oldest entries are dropped.
const MAX_CACHED_GLYPHS: usize = 1 << 17;
/// Shape results (text + face + direction + size) the cache may hold.
const MAX_CACHED_SHAPES: usize = 512;
/// Source bytes of the cached strings together.
const MAX_CACHED_TEXT_BYTES: usize = 1 << 19;
/// Strings longer than this are shaped but never cached.
const MAX_CACHED_TEXT: usize = 4096;
/// Font faces one engine holds at a time.
const MAX_FACES: usize = 16;
/// Longest string one [`TextEngine::shape`] call accepts. Excel caps a cell at 32 767 characters,
/// so this only stops a hostile caller from making the engine allocate gigabytes.
pub const MAX_TEXT_BYTES: usize = 4 << 20;

/// Base direction of a paragraph.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Take the direction from the first strong character (bidi rules P2/P3); LTR when there is
    /// none.
    #[default]
    Auto,
    /// Left to right.
    Ltr,
    /// Right to left.
    Rtl,
}

impl Direction {
    /// True for [`Direction::Rtl`] (and never for `Auto`: resolve it first).
    pub fn is_rtl(self) -> bool {
        matches!(self, Direction::Rtl)
    }

    /// The horizontal direction of a bidi level: even is left to right, odd is right to left.
    pub fn of_level(level: u8) -> Direction {
        if level.is_multiple_of(2) { Direction::Ltr } else { Direction::Rtl }
    }

    /// The bidi paragraph level this direction asks for (`None` = derive it from the text).
    fn level(self) -> Option<Level> {
        match self {
            Direction::Auto => None,
            Direction::Ltr => Some(Level::ltr()),
            Direction::Rtl => Some(Level::rtl()),
        }
    }
}

/// The size a string is drawn at.
///
/// One em is `size * scale` drawing units. The grid passes the font size in pixels and the
/// display's scale factor; the PDF writer passes the point size and `1.0`. Keeping them separate
/// means a zoom or a Retina scale never changes how text is shaped, only how big it is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontSize {
    /// Em size in the caller's units (points, pixels, …).
    pub size: f32,
    /// Extra scale on top, typically a display scale factor or a zoom.
    pub scale: f32,
}

impl FontSize {
    /// A size with an explicit scale.
    pub const fn new(size: f32, scale: f32) -> Self {
        Self { size, scale }
    }

    /// A size drawn at scale `1.0`.
    pub const fn em(size: f32) -> Self {
        Self { size, scale: 1.0 }
    }

    /// Drawing units per em.
    pub fn units_per_em(self) -> f32 {
        self.size * self.scale
    }
}

impl Default for FontSize {
    fn default() -> Self {
        Self { size: 11.0, scale: 1.0 }
    }
}

/// Handle to a font face registered with a [`TextEngine`].
///
/// Ids are never reused, so a face evicted from the cache keeps failing with
/// [`TextError::UnknownFace`] rather than silently pointing at another font.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FaceId(u64);

impl FaceId {
    /// The raw id, for logs and UI-side maps.
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for FaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "face#{}", self.0)
    }
}

/// What a registered face says about its own metrics, in font units (not scaled).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceMetrics {
    /// Font units per em; the divisor from font units to em fractions. Always positive.
    pub units_per_em: f32,
    /// Ascender in font units, above the baseline (positive up).
    pub ascender: f32,
    /// Descender in font units, below the baseline (negative).
    pub descender: f32,
    /// Glyphs in the face (`maxp`).
    pub glyphs: u16,
    /// Index of the face inside the file: a `.ttc` collection holds several.
    pub index: u32,
}

/// One positioned glyph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    /// Glyph id in the face. `0` is `.notdef`: the face has no glyph for that cluster.
    pub id: u16,
    /// First UTF-8 byte of the source cluster this glyph came from.
    pub cluster_start: u32,
    /// The byte just past that cluster. A mark shares its base's cluster, a ligature spans both of
    /// its characters.
    pub cluster_end: u32,
    /// How far the pen moves after drawing this glyph, in drawing units.
    pub advance: f32,
    /// Attachment shift before drawing, in drawing units (mark positioning, asymmetric forms).
    pub x_offset: f32,
    /// Vertical attachment shift in drawing units, positive up.
    pub y_offset: f32,
}

impl Glyph {
    /// The source cluster as a byte range.
    pub fn cluster(&self) -> Range<u32> {
        self.cluster_start..self.cluster_end
    }

    /// True when the face had no glyph for this cluster.
    pub fn is_missing(&self) -> bool {
        self.id == 0
    }
}

/// A run of glyphs with one bidi level, in visual order.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphRun {
    /// Bidi embedding level: even is LTR, odd is RTL. It can be higher than 1 inside an explicit
    /// embedding or an isolate.
    pub level: u8,
    /// The logical UTF-8 byte range of the source text this run covers.
    pub range: Range<u32>,
    /// Glyphs in visual order, left to right, whatever the level.
    pub glyphs: Vec<Glyph>,
}

impl GlyphRun {
    /// The direction this run is drawn in.
    pub fn direction(&self) -> Direction {
        Direction::of_level(self.level)
    }

    /// Sum of the run's advances, in drawing units.
    pub fn width(&self) -> f32 {
        self.glyphs.iter().map(|g| g.advance).sum()
    }
}

/// The shaped form of one string.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapedText {
    /// Runs in visual order, left to right.
    pub runs: Vec<GlyphRun>,
    /// The base direction actually used: `Auto` is resolved to `Ltr`/`Rtl` here (from the first
    /// paragraph's first strong character).
    pub base_direction: Direction,
    /// Sum of every advance, in drawing units.
    pub width: f32,
    /// Ascent for this face and size, in drawing units, positive up.
    pub ascent: f32,
    /// Descent for this face and size, in drawing units, negative (below the baseline).
    pub descent: f32,
}

impl ShapedText {
    /// Every glyph, in visual order.
    pub fn glyphs(&self) -> impl Iterator<Item = &Glyph> {
        self.runs.iter().flat_map(|run| run.glyphs.iter())
    }

    /// How many glyphs the string produced. Not the character count: ligatures, decomposed marks
    /// and unassigned characters all change it.
    pub fn glyph_count(&self) -> usize {
        self.runs.iter().map(|run| run.glyphs.len()).sum()
    }

    /// True when the string shaped to nothing (an empty string does).
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// The logical byte range the runs cover, or `None` when there are no runs.
    pub fn range(&self) -> Option<Range<u32>> {
        let start = self.runs.iter().map(|run| run.range.start).min()?;
        let end = self.runs.iter().map(|run| run.range.end).max()?;
        Some(start..end)
    }
}

/// Why a string could not be shaped.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TextError {
    /// The bytes are not an SFNT face, or the collection has no face at `index`.
    #[error("the font bytes are not a parseable OpenType/TrueType face (index {index})")]
    InvalidFont { index: u32 },
    /// The face was never registered with this engine, or it has been evicted.
    #[error("{0} is not registered with this text engine")]
    UnknownFace(FaceId),
    /// The drawing scale was `NaN`, infinite or negative.
    #[error("the drawing scale {0} is not a finite, non-negative number")]
    InvalidSize(String),
    /// The string is longer than [`MAX_TEXT_BYTES`].
    #[error("{len} bytes of text is more than the {MAX_TEXT_BYTES}-byte shaping limit")]
    TooLong { len: usize },
}

/// Counters for the caches, for tests and for tuning the bounds from the UI side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// Faces held.
    pub faces: usize,
    /// Shape results held.
    pub shapes: usize,
    /// Glyphs held by those results.
    pub glyphs: usize,
    /// Source bytes of the cached strings.
    pub text_bytes: usize,
    /// Lookups answered from the cache.
    pub hits: u64,
    /// Lookups that shaped.
    pub misses: u64,
}

/// The shaping engine: registered faces plus a bounded cache of shaped strings.
///
/// Create one per document (or per renderer) and reuse it; the faces stay parsed and repeated
/// strings stay shaped. It is not `Sync`: give each thread its own engine.
#[derive(Debug)]
pub struct TextEngine {
    faces: Vec<FaceEntry>,
    cache: ShapeCache,
    next_face: u64,
}

impl Default for TextEngine {
    fn default() -> Self {
        Self::new()
    }
}

struct FaceEntry {
    id: FaceId,
    data: Arc<[u8]>,
    index: u32,
    metrics: FaceMetrics,
    /// Content hash, so registering the same bytes twice reuses the face.
    hash: u64,
}

impl fmt::Debug for FaceEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FaceEntry").field("id", &self.id).field("index", &self.index).field("bytes", &self.data.len()).finish()
    }
}

impl FaceEntry {
    /// A `rustybuzz` view over the cached bytes.
    ///
    /// Reading the table directory per shaping call is cheap (microseconds, measured in the
    /// tests) and keeps the bytes owned without a self-referential struct. The costly parts —
    /// validation, metrics and the shaped results — are cached.
    fn face(&self) -> Option<HbFace<'_>> {
        HbFace::from_slice(&self.data, self.index)
    }
}

impl TextEngine {
    /// An empty engine.
    pub fn new() -> Self {
        Self { faces: Vec::new(), cache: ShapeCache::default(), next_face: 0 }
    }

    /// Register a font's bytes (face index `0`) and return its handle.
    ///
    /// The bytes are parsed once: an unparseable face is rejected here rather than at shaping
    /// time, and registering the same bytes again returns the same id.
    pub fn add_face(&mut self, data: impl Into<Arc<[u8]>>) -> Result<FaceId, TextError> {
        self.add_face_at(data, 0)
    }

    /// Register one face of a font file. `index` selects a face inside a `.ttc` collection.
    pub fn add_face_at(&mut self, data: impl Into<Arc<[u8]>>, index: u32) -> Result<FaceId, TextError> {
        let data = data.into();
        // Registering the same allocation again is the common case (a document re-opened, a face
        // handed to a fresh engine): answering it must not hash megabytes of font.
        if let Some(face) = self.faces.iter().find(|f| f.index == index && Arc::ptr_eq(&f.data, &data)) {
            return Ok(face.id);
        }
        let hash = hash_bytes(&data);
        // Same bytes in another allocation: the hash narrows the search and the bytes are compared
        // so a collision cannot hand back the wrong font.
        if let Some(face) = self.faces.iter().find(|f| f.index == index && f.hash == hash && f.data[..] == data[..]) {
            return Ok(face.id);
        }
        let metrics = face_metrics(&data, index)?;
        if self.faces.len() >= MAX_FACES {
            // Drop the face registered longest ago and its cached shapings: a stale shape would
            // still be correct, but keeping it only hides the eviction from the caller.
            if let Some(old) = self.faces.first() {
                let old = old.id;
                self.faces.remove(0);
                self.cache.forget_face(old);
            }
        }
        let id = FaceId(self.next_face);
        self.next_face = self.next_face.saturating_add(1);
        self.faces.push(FaceEntry { id, data, index, metrics, hash });
        Ok(id)
    }

    /// The metrics of a registered face, or `None` once it has been evicted.
    pub fn face_metrics(&self, face: FaceId) -> Option<FaceMetrics> {
        self.faces.iter().find(|f| f.id == face).map(|f| f.metrics)
    }

    /// Shape `text` with `face` at `size`, honouring `direction` for the paragraph.
    ///
    /// The result is cached for `(text, face, direction, size)`; a hit costs an [`Arc`] clone.
    /// `Ok` with no runs is what an empty (or all-blank and unassigned) string produces.
    pub fn shape(&mut self, text: &str, direction: Direction, face: FaceId, size: FontSize) -> Result<Arc<ShapedText>, TextError> {
        let units_per_em = size.units_per_em();
        if !units_per_em.is_finite() || units_per_em < 0.0 {
            return Err(TextError::InvalidSize(format!("{units_per_em}")));
        }
        if text.len() > MAX_TEXT_BYTES {
            return Err(TextError::TooLong { len: text.len() });
        }
        let entry = self.faces.iter().find(|f| f.id == face).ok_or(TextError::UnknownFace(face))?;
        let cacheable = text.len() <= MAX_CACHED_TEXT;
        let key =
            cacheable.then(|| ShapeKey { face, direction, size_bits: size.size.to_bits(), scale_bits: size.scale.to_bits(), text: text.into() });
        if let Some(key) = &key
            && let Some(hit) = self.cache.get(key)
        {
            return Ok(hit);
        }
        let shaped = Arc::new(shape_text(entry, text, direction, units_per_em)?);
        if let Some(key) = key {
            self.cache.insert(key, Arc::clone(&shaped));
        }
        Ok(shaped)
    }

    /// Drop every face and cached shaping (after a document switch, or to reclaim memory).
    pub fn clear(&mut self) {
        self.faces.clear();
        self.cache = ShapeCache::default();
    }

    /// What the caches hold right now.
    pub fn cache_stats(&self) -> CacheStats {
        CacheStats {
            faces: self.faces.len(),
            shapes: self.cache.entries.len(),
            glyphs: self.cache.glyphs,
            text_bytes: self.cache.text_bytes,
            hits: self.cache.hits,
            misses: self.cache.misses,
        }
    }
}

/// One cached shaping, keyed by everything that can change its result.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ShapeKey {
    face: FaceId,
    direction: Direction,
    /// `f32::to_bits`, so `NaN` sizes (which `shape` rejects) and `-0.0` still hash consistently.
    size_bits: u32,
    scale_bits: u32,
    text: Box<str>,
}

#[derive(Debug, Default)]
struct ShapeCache {
    entries: HashMap<ShapeKey, Arc<ShapedText>>,
    /// Insertion order, for FIFO eviction. A hit does not move an entry: FIFO is enough to bound
    /// the memory, and a renderer that repeats a string hits it long before it ages out.
    order: VecDeque<ShapeKey>,
    glyphs: usize,
    text_bytes: usize,
    hits: u64,
    misses: u64,
}

impl ShapeCache {
    fn get(&mut self, key: &ShapeKey) -> Option<Arc<ShapedText>> {
        match self.entries.get(key) {
            Some(hit) => {
                self.hits = self.hits.saturating_add(1);
                Some(Arc::clone(hit))
            }
            None => {
                self.misses = self.misses.saturating_add(1);
                None
            }
        }
    }

    fn insert(&mut self, key: ShapeKey, shaped: Arc<ShapedText>) {
        if self.entries.contains_key(&key) {
            return;
        }
        self.glyphs = self.glyphs.saturating_add(shaped.glyph_count());
        self.text_bytes = self.text_bytes.saturating_add(key.text.len());
        self.order.push_back(key.clone());
        self.entries.insert(key, shaped);
        while self.order.len() > MAX_CACHED_SHAPES || self.glyphs > MAX_CACHED_GLYPHS || self.text_bytes > MAX_CACHED_TEXT_BYTES {
            let Some(oldest) = self.order.pop_front() else { break };
            if let Some(evicted) = self.entries.remove(&oldest) {
                self.glyphs = self.glyphs.saturating_sub(evicted.glyph_count());
                self.text_bytes = self.text_bytes.saturating_sub(oldest.text.len());
            }
        }
    }

    /// Drop everything shaped with `face` (it is being evicted).
    fn forget_face(&mut self, face: FaceId) {
        let mut glyphs = 0;
        let mut text_bytes = 0;
        self.entries.retain(|key, shaped| {
            if key.face == face {
                glyphs += shaped.glyph_count();
                text_bytes += key.text.len();
                false
            } else {
                true
            }
        });
        self.order.retain(|key| key.face != face);
        self.glyphs = self.glyphs.saturating_sub(glyphs);
        self.text_bytes = self.text_bytes.saturating_sub(text_bytes);
    }
}

/// A stable, cheap content hash for face lookup. Collisions are resolved by comparing the bytes.
fn hash_bytes(data: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    hasher.finish()
}

/// Parse `data` once to validate it and read the metrics the shaping code needs.
fn face_metrics(data: &[u8], index: u32) -> Result<FaceMetrics, TextError> {
    let face = HbFace::from_slice(data, index).ok_or(TextError::InvalidFont { index })?;
    let units_per_em = face.units_per_em();
    if units_per_em <= 0 {
        return Err(TextError::InvalidFont { index });
    }
    Ok(FaceMetrics {
        units_per_em: units_per_em as f32,
        ascender: f32::from(face.ascender()),
        descender: f32::from(face.descender()),
        glyphs: face.number_of_glyphs(),
        index,
    })
}

/// Resolve bidi levels, reorder into visual runs and shape each run.
fn shape_text(face: &FaceEntry, text: &str, base: Direction, units_per_em: f32) -> Result<ShapedText, TextError> {
    let metrics = face.metrics;
    let hb_face = face.face().ok_or(TextError::InvalidFont { index: face.index })?;
    let unit = units_per_em / metrics.units_per_em;
    let bidi = BidiInfo::new(text, base.level());
    let base_direction = match base {
        Direction::Auto => match bidi.paragraphs.first() {
            Some(para) if para.level.is_rtl() => Direction::Rtl,
            _ => Direction::Ltr,
        },
        fixed => fixed,
    };
    let mut runs = Vec::new();
    // A paragraph is a hard line break: each one gets its own base level and its own L1/L2 pass.
    // Wrapping inside a paragraph is the caller's job (see the module docs).
    for para in &bidi.paragraphs {
        let (levels, visual) = bidi.visual_runs(para, para.range.clone());
        for range in visual {
            let Some(slice) = text.get(range.clone()) else { continue };
            if slice.is_empty() {
                continue;
            }
            // `visual_runs` returns byte-aligned runs; take the L1-adjusted level it used.
            let level = levels.get(range.start).copied().unwrap_or(para.level).number();
            let glyphs = shape_run(&hb_face, slice, range.start, Direction::of_level(level), unit, range.end);
            if glyphs.is_empty() {
                continue;
            }
            runs.push(GlyphRun { level, range: range.start as u32..range.end as u32, glyphs });
        }
    }
    let width = runs.iter().map(GlyphRun::width).sum();
    Ok(ShapedText { runs, base_direction, width, ascent: metrics.ascender * unit, descent: metrics.descender * unit })
}

/// Shape one bidi run.
///
/// `slice` is the run's text, `base` its first byte in the full string and `end` the byte just past
/// it; returned clusters are absolute offsets into the full string. Text is always sliced on a
/// character boundary (the bidi algorithm works per byte but only breaks runs between characters),
/// but a caller can hand us a string that starts mid-cluster — an isolated combining mark — and
/// that shapes without complaint.
fn shape_run(face: &HbFace, slice: &str, base: usize, direction: Direction, unit: f32, end: usize) -> Vec<Glyph> {
    let mut buffer = UnicodeBuffer::new();
    buffer.set_direction(if direction.is_rtl() { HbDirection::RightToLeft } else { HbDirection::LeftToRight });
    // Monotone grapheme clusters: a mark or a reordered vowel merges into its base's cluster, so
    // the cluster map is monotonic and every byte of the string belongs to exactly one glyph
    // cluster. That is what a hit test or a text selection needs.
    buffer.set_cluster_level(BufferClusterLevel::MonotoneGraphemes);
    buffer.push_str(slice);
    let shaped = rustybuzz::shape(face, &[], buffer);
    let infos = shaped.glyph_infos();
    let positions = shaped.glyph_positions();
    let run_end = end as u32;
    // Cluster starts in logical order (the monotone level guarantees non-decreasing values), so
    // each glyph's cluster ends where the next one starts.
    let mut starts: Vec<u32> = infos.iter().map(|info| info.cluster).collect();
    starts.sort_unstable();
    starts.dedup();
    let mut glyphs = Vec::with_capacity(infos.len());
    for (info, position) in infos.iter().zip(positions.iter()) {
        let cluster = info.cluster;
        let next = starts.partition_point(|&start| start <= cluster);
        let cluster_end = starts.get(next).map_or(run_end, |&start| base as u32 + start);
        glyphs.push(Glyph {
            // rustybuzz documents glyph ids as <= u16::MAX; fall back to .notdef rather than
            // truncating if a face ever breaks that promise.
            id: u16::try_from(info.glyph_id).unwrap_or(0),
            cluster_start: base as u32 + cluster,
            cluster_end: cluster_end.min(run_end),
            advance: position.x_advance as f32 * unit,
            x_offset: position.x_offset as f32 * unit,
            y_offset: position.y_offset as f32 * unit,
        });
    }
    glyphs
}

#[cfg(test)]
mod tests;
