//! A tiny, self-contained PDF 1.7 writer for GridCraft's print and PDF export.
//!
//! - The 14 standard fonts with WinAnsiEncoding, plus approximate width tables ([`text_width`]) so
//!   callers can measure, align and wrap text.
//! - An optional **embedded TrueType subset** ([`PdfDoc::set_font`]) for characters
//!   WinAnsiEncoding cannot set: the font is read and subset in [`truetype`], embedded as a
//!   `CIDFontType2` with `FontFile2`, a `ToUnicode` CMap and a `/W` widths array taken from
//!   `hmtx`, and text is drawn with two-byte glyph ids (`/Identity-H`). A document that sets no
//!   font, or whose text stays inside WinAnsi, is byte-for-byte what it was: the embedded font is
//!   only written when a page actually used it.
//! - Characters outside WinAnsiEncoding that the embedded font does not cover (and every such
//!   character when no font is set) are set in a predefined, non-embedded Adobe-GB1 CID font with
//!   the `UniGB-UTF16-H` CMap instead of being replaced by `?`.
//! - Vector drawing: rectangles, lines (solid or dashed), polygons, circles (Bézier), clipping.
//! - RGBA images as RGB image XObjects with a soft mask for alpha (run-length encoded).
//! - Coordinates are points with a **top-left origin** (y grows down); text `y` is the baseline.
//!
//! Streams are written uncompressed (images use the trivial RunLengthDecode filter) so the crate
//! needs no dependencies.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod metrics;
pub mod truetype;

#[cfg(test)]
mod testfont;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::rc::Rc;

pub use truetype::{Error as FontError, Face, Subset};

/// RGB colour, 0–255 per channel.
pub type Rgb = [u8; 3];

/// The standard 14 fonts minus Symbol and ZapfDingbats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Font {
    #[default]
    Helvetica,
    HelveticaBold,
    HelveticaOblique,
    HelveticaBoldOblique,
    TimesRoman,
    TimesBold,
    TimesItalic,
    TimesBoldItalic,
    Courier,
    CourierBold,
    CourierOblique,
    CourierBoldOblique,
}

/// A font family for [`Font::styled`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Family {
    #[default]
    Sans,
    Serif,
    Mono,
}

impl Font {
    pub const ALL: [Font; 12] = [
        Font::Helvetica,
        Font::HelveticaBold,
        Font::HelveticaOblique,
        Font::HelveticaBoldOblique,
        Font::TimesRoman,
        Font::TimesBold,
        Font::TimesItalic,
        Font::TimesBoldItalic,
        Font::Courier,
        Font::CourierBold,
        Font::CourierOblique,
        Font::CourierBoldOblique,
    ];

    /// PostScript base font name.
    pub fn base_name(self) -> &'static str {
        match self {
            Font::Helvetica => "Helvetica",
            Font::HelveticaBold => "Helvetica-Bold",
            Font::HelveticaOblique => "Helvetica-Oblique",
            Font::HelveticaBoldOblique => "Helvetica-BoldOblique",
            Font::TimesRoman => "Times-Roman",
            Font::TimesBold => "Times-Bold",
            Font::TimesItalic => "Times-Italic",
            Font::TimesBoldItalic => "Times-BoldItalic",
            Font::Courier => "Courier",
            Font::CourierBold => "Courier-Bold",
            Font::CourierOblique => "Courier-Oblique",
            Font::CourierBoldOblique => "Courier-BoldOblique",
        }
    }

    fn index(self) -> usize {
        Font::ALL.iter().position(|f| *f == self).unwrap_or(0)
    }

    /// The face of `family` with the given weight and slant.
    pub fn styled(family: Family, bold: bool, italic: bool) -> Font {
        match (family, bold, italic) {
            (Family::Sans, false, false) => Font::Helvetica,
            (Family::Sans, true, false) => Font::HelveticaBold,
            (Family::Sans, false, true) => Font::HelveticaOblique,
            (Family::Sans, true, true) => Font::HelveticaBoldOblique,
            (Family::Serif, false, false) => Font::TimesRoman,
            (Family::Serif, true, false) => Font::TimesBold,
            (Family::Serif, false, true) => Font::TimesItalic,
            (Family::Serif, true, true) => Font::TimesBoldItalic,
            (Family::Mono, false, false) => Font::Courier,
            (Family::Mono, true, false) => Font::CourierBold,
            (Family::Mono, false, true) => Font::CourierOblique,
            (Family::Mono, true, true) => Font::CourierBoldOblique,
        }
    }

    /// Picks a standard family for a font name: serif faces map to Times, monospaced ones to
    /// Courier, everything else to Helvetica.
    pub fn family_for(name: &str) -> Family {
        let n = name.to_ascii_lowercase();
        const SERIF: [&str; 10] = ["times", "cambria", "georgia", "garamond", "serif", "book", "palatino", "baskerville", "tinos", "caladea"];
        const MONO: [&str; 8] = ["courier", "consolas", "mono", "menlo", "lucida console", "cousine", "code", "typewriter"];
        if MONO.iter().any(|m| n.contains(m)) {
            Family::Mono
        } else if SERIF.iter().any(|s| n.contains(s)) && !n.contains("sans") {
            Family::Serif
        } else {
            Family::Sans
        }
    }

    /// Advance width of a character in 1/1000 em.
    pub fn char_width(self, c: char) -> u16 {
        let table: Option<&[u16; 95]> = match self {
            Font::Helvetica | Font::HelveticaOblique => Some(&metrics::HELVETICA),
            Font::HelveticaBold | Font::HelveticaBoldOblique => Some(&metrics::HELVETICA_BOLD),
            Font::TimesRoman | Font::TimesItalic => Some(&metrics::TIMES),
            Font::TimesBold | Font::TimesBoldItalic => Some(&metrics::TIMES_BOLD),
            _ => None,
        };
        if needs_cid(c) {
            return CID_WIDTH;
        }
        let Some(table) = table else { return 600 };
        let ascii = |c: char| -> Option<u16> {
            let i = (c as u32).checked_sub(32)? as usize;
            table.get(i).copied()
        };
        if let Some(w) = ascii(c) {
            return w;
        }
        match c {
            '—' | '…' | '‰' | '™' => 1000,
            _ => metrics::fold(c).and_then(ascii).unwrap_or(match self {
                Font::TimesRoman | Font::TimesItalic | Font::TimesBold | Font::TimesBoldItalic => 500,
                _ => 556,
            }),
        }
    }
}

/// Width of `text` in points when set in `font` at `size` points.
pub fn text_width(font: Font, size: f32, text: &str) -> f32 {
    text_width_with(None, font, size, text)
}

/// Width of `text` in points, measured with `face` for the characters it covers (the advance
/// widths come from the font's `hmtx`) and with the standard font's table for the rest. Pass the
/// same face the document draws with so centring, right alignment and wrapping match the glyphs.
pub fn text_width_with(face: Option<&Face>, font: Font, size: f32, text: &str) -> f32 {
    // u64: a u32 sum overflows for ~8 million characters.
    let mut units: u64 = 0;
    for c in text.chars() {
        if needs_cid(c)
            && let Some(face) = face
            && face.covers(c)
        {
            // Scale the face's units-per-em to the 1/1000 em the table uses.
            let advance = u64::from(face.advance(face.glyph(c)));
            units += advance.saturating_mul(1000) / u64::from(face.units_per_em().max(1));
        } else {
            units += u64::from(font.char_width(c));
        }
    }
    units as f32 * size / 1000.0
}

/// Unicode → WinAnsiEncoding (CP1252) byte; unmappable characters become `?`.
pub fn win_ansi(c: char) -> u8 {
    let u = c as u32;
    if (0x20..0x7F).contains(&u) || (0xA0..=0xFF).contains(&u) {
        return u as u8;
    }
    match c {
        '€' => 0x80,
        '‚' => 0x82,
        'ƒ' => 0x83,
        '„' => 0x84,
        '…' => 0x85,
        '†' => 0x86,
        '‡' => 0x87,
        'ˆ' => 0x88,
        '‰' => 0x89,
        'Š' => 0x8A,
        '‹' => 0x8B,
        'Œ' => 0x8C,
        'Ž' => 0x8E,
        '‘' => 0x91,
        '’' => 0x92,
        '“' => 0x93,
        '”' => 0x94,
        '•' => 0x95,
        '–' => 0x96,
        '—' => 0x97,
        '˜' => 0x98,
        '™' => 0x99,
        'š' => 0x9A,
        '›' => 0x9B,
        'œ' => 0x9C,
        'ž' => 0x9E,
        'Ÿ' => 0x9F,
        '\t' => b' ',
        _ => b'?',
    }
}

/// Index of the CID font in a page's font flags (after the [`Font::ALL`] faces).
const CID_FONT: usize = Font::ALL.len();
/// Index of the embedded TrueType subset in a page's font flags.
const EMBED_FONT: usize = CID_FONT + 1;
/// Number of font resources a page can use.
const FONT_SLOTS: usize = EMBED_FONT + 1;
/// Most characters one embedded font maps (a `CIDFontType2` numbers glyphs with two bytes).
const MAX_EMBEDDED_GLYPHS: usize = 65_534;
/// Base font of the CID font used for characters WinAnsiEncoding lacks.
const CID_BASE_FONT: &str = "STSong-Light";
/// Default glyph width of the CID font (full-width), in 1/1000 em.
const CID_WIDTH: u16 = 1000;

/// True when `c` has no WinAnsiEncoding byte and is drawn with the CID font.
fn needs_cid(c: char) -> bool {
    !c.is_control() && c != '?' && win_ansi(c) == b'?'
}

/// The font a document embeds for text WinAnsiEncoding cannot set, and the glyphs it has handed
/// out so far.
///
/// Glyph ids are assigned the first time a character is drawn, so the content stream written
/// while drawing already carries the subset's glyph ids: subset glyph 1 is the first character
/// the document set, and so on. Composite components are appended when the subset is built (they
/// have no character of their own), which is why the ids of drawn glyphs never move.
struct FontUse {
    face: Face,
    /// Character → subset glyph id (1-based; 0 is `.notdef`).
    chars: BTreeMap<char, u16>,
    /// Subset glyph id - 1 → source glyph id.
    order: Vec<u16>,
}

impl FontUse {
    fn new(face: Face) -> FontUse {
        FontUse { face, chars: BTreeMap::new(), order: Vec::new() }
    }

    /// The subset glyph id for `c`, assigning one on first use. `None` when the font has no glyph
    /// for `c`, or when its outline data cannot be read (the caller then keeps the CID fallback,
    /// so a broken font cannot produce a broken subset).
    fn glyph_for(&mut self, c: char) -> Option<u16> {
        if let Some(gid) = self.chars.get(&c) {
            return Some(*gid);
        }
        let src = self.face.glyph(c);
        if src == 0 || !self.face.sound(src) {
            return None;
        }
        if self.order.len() >= MAX_EMBEDDED_GLYPHS {
            return None;
        }
        let gid = self.order.len() as u16 + 1;
        self.order.push(src);
        self.chars.insert(c, gid);
        Some(gid)
    }

    /// The font program, widths and mappings for the subset this document used. If the used
    /// glyphs cannot be subset (a font that parsed but whose outlines are inconsistent), a
    /// `.notdef`-only subset keeps the PDF valid instead of leaving the pages pointing at a font
    /// that was never written.
    fn build(&self) -> Result<BuiltFont, FontError> {
        self.build_of(&self.order).or_else(|_| self.build_of(&[]))
    }

    fn build_of(&self, glyphs: &[u16]) -> Result<BuiltFont, FontError> {
        let pairs: Vec<(char, u16)> = if glyphs.is_empty() {
            Vec::new()
        } else {
            self.chars.iter().filter_map(|(c, gid)| glyphs.get((*gid as usize).checked_sub(1)?).map(|src| (*c, *src))).collect()
        };
        let subset = self.face.subset(glyphs, &pairs)?;
        let upem = self.face.units_per_em().max(1);
        let to_em = |v: i16| -> i32 { (i32::from(v).saturating_mul(1000)) / i32::from(upem) };
        Ok(BuiltFont {
            data: subset.data,
            // A PDF subset name is six uppercase letters and a `+` prefix.
            name: subset_name(self.face.postscript_name().or_else(|| self.face.family_name()).as_deref()),
            widths: subset.gids.iter().map(|g| to_em(self.face.advance(*g) as i16).clamp(0, 30_000) as u16).collect(),
            unicode: subset.chars,
            bbox: self.face.bbox().map(to_em),
            ascent: to_em(self.face.ascent_descent().0),
            descent: to_em(self.face.ascent_descent().1),
            italic: self.face.italic_angle(),
        })
    }
}

/// Everything the PDF objects need from a used font.
struct BuiltFont {
    data: Vec<u8>,
    name: String,
    /// Advance width per subset glyph id, in 1/1000 em.
    widths: Vec<u16>,
    /// Subset glyph id → character, for the `ToUnicode` CMap.
    unicode: Vec<(char, u16)>,
    /// `[xMin, yMin, xMax, yMax]` in 1/1000 em.
    bbox: [i32; 4],
    ascent: i32,
    descent: i32,
    italic: f32,
}

/// `BASENAME+` as a PDF subset name: six uppercase letters (ASCII for the font's own name, so
/// `/Name` needs no escaping) and a deterministic tag.
fn subset_name(name: Option<&str>) -> String {
    let base: String = name.unwrap_or("Embedded").chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).take(48).collect();
    let base = if base.is_empty() { "Embedded".to_string() } else { base };
    // The tag must be six uppercase letters; deriving it from the name keeps the output stable.
    let seed: u32 = base.bytes().fold(0x811C_9DC5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193));
    let mut tag = String::new();
    let mut v = seed;
    for _ in 0..6 {
        tag.push((b'A' + (v % 26) as u8) as char);
        v /= 26;
        if v == 0 {
            v = seed.rotate_left(3) | 1;
        }
    }
    format!("{tag}+{base}")
}

/// A PDF hex string `< … >` of `text` in UTF-16BE (the `UniGB-UTF16-H` encoding).
fn pdf_utf16_string(text: &str) -> String {
    let mut s = String::with_capacity(text.len() * 4 + 2);
    s.push('<');
    for unit in text.encode_utf16() {
        let _ = write!(s, "{unit:04X}");
    }
    s.push('>');
    s
}

/// A PDF hex string `< … >` of two-byte glyph ids for an `/Identity-H` embedded font.
fn pdf_glyph_string(glyphs: &[u16]) -> String {
    let mut s = String::with_capacity(glyphs.len() * 4 + 2);
    s.push('<');
    for g in glyphs {
        let _ = write!(s, "{g:04X}");
    }
    s.push('>');
    s
}

/// A PDF literal string `( … )` for `text` in WinAnsiEncoding.
fn pdf_string(text: &str) -> String {
    let mut s = String::with_capacity(text.len() + 2);
    s.push('(');
    for c in text.chars() {
        let b = win_ansi(c);
        match b {
            b'(' | b')' | b'\\' => {
                s.push('\\');
                s.push(b as char);
            }
            0x20..=0x7E => s.push(b as char),
            _ => {
                let _ = write!(s, "\\{b:03o}");
            }
        }
    }
    s.push(')');
    s
}

/// URI actions use byte strings. Percent-encode non-ASCII UTF-8 before PDF string escaping.
fn pdf_uri_string(uri: &str) -> String {
    let mut ascii = String::with_capacity(uri.len());
    for byte in uri.bytes() {
        if byte.is_ascii() {
            ascii.push(byte as char);
        } else {
            let _ = write!(ascii, "%{byte:02X}");
        }
    }
    pdf_string(&ascii)
}

/// A number for a content stream: finite, short.
fn n(v: f32) -> String {
    let v = if v.is_finite() { v.clamp(-1.0e7, 1.0e7) } else { 0.0 };
    let r = (v * 1000.0).round() / 1000.0;
    if r == r.trunc() {
        format!("{}", r as i64)
    } else {
        let s = format!("{r:.3}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// True when every value is finite (NaN and infinities are rejected).
fn finite(v: &[f32]) -> bool {
    v.iter().all(|x| x.is_finite())
}

/// A positive, finite extent (a width, height or radius).
fn extent(v: f32) -> bool {
    v > 0.0 && v.is_finite()
}

/// A stroke width: negative, NaN and infinite become 0 (the thinnest line).
fn stroke_width(w: f32) -> f32 {
    if w.is_finite() { w.max(0.0) } else { 0.0 }
}

/// The entries of a dash array (`[on, off, …]`, at most 8). A pattern the PDF specification
/// forbids (any non-finite entry, or no entry that survives rounding to a positive number) is
/// replaced by an empty array, i.e. a solid line.
fn dash_values(dash: &[f32]) -> String {
    let d: Vec<f32> = dash.iter().take(8).map(|v| if v.is_finite() { v.max(0.0) } else { f32::NAN }).collect();
    if !finite(&d) || !d.iter().any(|v| (v.min(1.0e7) * 1000.0).round() > 0.0) {
        return String::new();
    }
    d.iter().map(|v| n(*v)).collect::<Vec<_>>().join(" ")
}

fn rgb_op(c: Rgb, stroke: bool) -> String {
    let f = |v: u8| n(v as f32 / 255.0);
    format!("{} {} {} {}", f(c[0]), f(c[1]), f(c[2]), if stroke { "RG" } else { "rg" })
}

struct ImageData {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
    alpha: Option<Vec<u8>>,
}

enum LinkTarget {
    Uri(String),
    Page { index: usize, x: f32, y: f32 },
}

struct LinkAnnotation {
    /// PDF coordinates, with the origin at the bottom left.
    rect: [f32; 4],
    target: LinkTarget,
}

/// A page under construction. Obtain one with [`PdfDoc::add_page`].
pub struct Page {
    width: f32,
    height: f32,
    content: String,
    fonts: [bool; FONT_SLOTS],
    images: Vec<ImageData>,
    links: Vec<LinkAnnotation>,
    depth: u32,
    /// Set when a run was drawn with the embedded font (its objects are only then written).
    embedded_used: bool,
    /// The document's embedded font: glyph ids are handed out as text is drawn.
    font: Rc<RefCell<Option<FontUse>>>,
}

/// Most bytes of content per page before further drawing is dropped (keeps hostile inputs from
/// exhausting memory).
const MAX_PAGE_CONTENT: usize = 64 * 1024 * 1024;
/// Largest image (pixels) embedded.
const MAX_IMAGE_PIXELS: u64 = 40_000_000;
const MAX_PAGE_LINKS: usize = 4096;
const MAX_LINK_URI_BYTES: usize = 4096;

impl Page {
    pub fn width(&self) -> f32 {
        self.width
    }
    pub fn height(&self) -> f32 {
        self.height
    }
    fn y(&self, y: f32) -> f32 {
        self.height - y
    }
    fn full(&self) -> bool {
        self.content.len() > MAX_PAGE_CONTENT
    }
    fn op(&mut self, s: &str) {
        if !self.full() {
            self.content.push_str(s);
            self.content.push('\n');
        }
    }

    fn link_rect(&self, x: f32, y: f32, w: f32, h: f32) -> Option<[f32; 4]> {
        if !finite(&[x, y, w, h]) || !extent(w) || !extent(h) || self.links.len() >= MAX_PAGE_LINKS {
            return None;
        }
        let (right, bottom) = (x + w, y + h);
        if !finite(&[right, bottom]) {
            return None;
        }
        let (left, right) = (x.max(0.0), right.min(self.width));
        let (top, bottom) = (y.max(0.0), bottom.min(self.height));
        (right > left && bottom > top).then(|| [left, self.y(bottom), right, self.y(top)])
    }

    /// Adds an invisible clickable area for an external URI. Invalid or excessive links are ignored.
    pub fn link_uri(&mut self, x: f32, y: f32, w: f32, h: f32, uri: &str) {
        let allowed = uri
            .split_once(':')
            .is_some_and(|(scheme, rest)| matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https" | "mailto") && !rest.is_empty());
        if !allowed || uri.len() > MAX_LINK_URI_BYTES || uri.chars().any(char::is_control) {
            return;
        }
        if let Some(rect) = self.link_rect(x, y, w, h) {
            self.links.push(LinkAnnotation { rect, target: LinkTarget::Uri(uri.to_string()) });
        }
    }

    /// Adds a link to a location on another page of this document (zero-based page index).
    pub fn link_page(&mut self, x: f32, y: f32, w: f32, h: f32, page: usize, target_x: f32, target_y: f32) {
        if !finite(&[target_x, target_y]) {
            return;
        }
        if let Some(rect) = self.link_rect(x, y, w, h) {
            self.links.push(LinkAnnotation { rect, target: LinkTarget::Page { index: page, x: target_x, y: target_y } });
        }
    }

    /// Saves the graphics state (pair with [`Page::restore`]).
    pub fn save(&mut self) {
        self.depth = self.depth.saturating_add(1);
        self.content.push_str("q\n");
    }
    /// Restores the graphics state saved by [`Page::save`]; extra calls are ignored.
    pub fn restore(&mut self) {
        if self.depth > 0 {
            self.depth -= 1;
            self.content.push_str("Q\n");
        }
    }
    /// Intersects the clip with a rectangle (until the next [`Page::restore`]).
    pub fn clip_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let s = format!("{} {} {} {} re W n", n(x), n(self.y(y + h)), n(w.max(0.0)), n(h.max(0.0)));
        self.op(&s);
    }

    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Rgb) {
        if !(extent(w) && extent(h) && finite(&[x, y])) {
            return;
        }
        let s = format!("{}\n{} {} {} {} re f", rgb_op(color, false), n(x), n(self.y(y + h)), n(w), n(h));
        self.op(&s);
    }

    pub fn rect_stroke(&mut self, x: f32, y: f32, w: f32, h: f32, width: f32, color: Rgb) {
        if !finite(&[x, y, w, h]) {
            return;
        }
        let s = format!("{}\n{} w [] 0 d\n{} {} {} {} re S", rgb_op(color, true), n(stroke_width(width)), n(x), n(self.y(y + h)), n(w), n(h));
        self.op(&s);
    }

    pub fn stroke_line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, color: Rgb) {
        self.stroke_line_dashed(x0, y0, x1, y1, width, color, &[]);
    }

    /// A line with a dash pattern (`[on, off, …]` in points; empty = solid).
    pub fn stroke_line_dashed(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, color: Rgb, dash: &[f32]) {
        if !finite(&[x0, y0, x1, y1]) {
            return;
        }
        let s = format!(
            "{}\n{} w [{}] 0 d\n{} {} m {} {} l S",
            rgb_op(color, true),
            n(stroke_width(width)),
            dash_values(dash),
            n(x0),
            n(self.y(y0)),
            n(x1),
            n(self.y(y1))
        );
        self.op(&s);
    }

    /// An open polyline.
    pub fn polyline(&mut self, pts: &[[f32; 2]], width: f32, color: Rgb, dash: &[f32]) {
        if pts.len() < 2 || !pts.iter().all(|p| finite(p)) {
            return;
        }
        let mut s = format!("{}\n{} w [{}] 0 d 1 j\n", rgb_op(color, true), n(stroke_width(width)), dash_values(dash));
        self.path(&mut s, pts, false);
        s.push_str(" S");
        self.op(&s);
    }

    fn path(&self, s: &mut String, pts: &[[f32; 2]], close: bool) {
        for (i, p) in pts.iter().enumerate() {
            let _ = write!(s, "{} {} {} ", n(p[0]), n(self.y(p[1])), if i == 0 { "m" } else { "l" });
        }
        if close {
            s.push('h');
        }
    }

    /// A closed polygon, filled and/or stroked.
    pub fn polygon(&mut self, pts: &[[f32; 2]], fill: Option<Rgb>, stroke: Option<(Rgb, f32)>) {
        if pts.len() < 2 || (fill.is_none() && stroke.is_none()) || !pts.iter().all(|p| finite(p)) {
            return;
        }
        let mut s = String::new();
        if let Some(f) = fill {
            s.push_str(&rgb_op(f, false));
            s.push('\n');
        }
        if let Some((c, w)) = stroke {
            let _ = writeln!(s, "{}\n{} w [] 0 d 1 j", rgb_op(c, true), n(stroke_width(w)));
        }
        self.path(&mut s, pts, true);
        s.push_str(match (fill.is_some(), stroke.is_some()) {
            (true, true) => " B",
            (true, false) => " f",
            _ => " S",
        });
        self.op(&s);
    }

    /// A circle approximated by four cubic Béziers.
    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, fill: Option<Rgb>, stroke: Option<(Rgb, f32)>) {
        if !extent(r) || !finite(&[cx, cy]) || (fill.is_none() && stroke.is_none()) {
            return;
        }
        const K: f32 = 0.552_284_8;
        let k = r * K;
        let y = self.y(cy);
        let mut s = String::new();
        if let Some(f) = fill {
            s.push_str(&rgb_op(f, false));
            s.push('\n');
        }
        if let Some((c, w)) = stroke {
            let _ = writeln!(s, "{}\n{} w [] 0 d", rgb_op(c, true), n(stroke_width(w)));
        }
        let _ = write!(
            s,
            "{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c h",
            n(cx + r),
            n(y),
            n(cx + r),
            n(y + k),
            n(cx + k),
            n(y + r),
            n(cx),
            n(y + r),
            n(cx - k),
            n(y + r),
            n(cx - r),
            n(y + k),
            n(cx - r),
            n(y),
            n(cx - r),
            n(y - k),
            n(cx - k),
            n(y - r),
            n(cx),
            n(y - r),
            n(cx + k),
            n(y - r),
            n(cx + r),
            n(y - k),
            n(cx + r),
            n(y)
        );
        s.push_str(match (fill.is_some(), stroke.is_some()) {
            (true, true) => " B",
            (true, false) => " f",
            _ => " S",
        });
        self.op(&s);
    }

    /// Text with its baseline starting at `(x, y)`.
    pub fn text(&mut self, x: f32, y: f32, size: f32, font: Font, color: Rgb, text: &str) {
        self.text_rotated(x, y, size, font, color, text, 0.0);
    }

    /// Text rotated by `angle` radians (clockwise on the page, like the y-down coordinate
    /// system) about its baseline origin.
    pub fn text_rotated(&mut self, x: f32, y: f32, size: f32, font: Font, color: Rgb, text: &str, angle: f32) {
        if text.is_empty() || !extent(size) || !finite(&[x, y]) {
            return;
        }
        let fi = font.index();
        let size = n(size.min(10_000.0));
        let (sn, cs) = if angle.is_finite() { angle.sin_cos() } else { (0.0, 1.0) };
        // Clockwise in y-down = counter-clockwise negated in PDF's y-up space.
        let mut s = format!("BT\n{}\n{} {} {} {} {} {} Tm\n", rgb_op(color, false), n(cs), n(-sn), n(sn), n(cs), n(x), n(self.y(y)));
        // Runs of WinAnsi text use `font`; runs it cannot encode use the embedded subset when the
        // document set one that covers them, and the predefined CID font otherwise. Each `Tj`
        // advances the text position, so the runs follow each other on the baseline.
        let mut rest = text;
        while !rest.is_empty() {
            let (class, end, glyphs) = self.next_run(rest);
            let end = end.clamp(1, rest.len());
            let (run, tail) = rest.split_at(end);
            // A run the embedded font could not map keeps the non-embedded CID fallback.
            let (slot, string) = match class {
                1 if !glyphs.is_empty() => (EMBED_FONT, pdf_glyph_string(&glyphs)),
                2 => (CID_FONT, pdf_utf16_string(run)),
                _ => (fi, pdf_string(run)),
            };
            if let Some(f) = self.fonts.get_mut(slot) {
                *f = true;
            }
            if slot == EMBED_FONT {
                self.embedded_used = true;
            }
            let _ = writeln!(s, "/F{} {} Tf\n{} Tj", slot + 1, size, string);
            rest = tail;
        }
        s.push_str("ET");
        self.op(&s);
    }

    /// Splits the next text run off `rest`: `(class, byte length, glyph ids)` where class 0 is a
    /// standard-font run, 1 an embedded-font run (with one glyph id per character) and 2 a run for
    /// the predefined CID font.
    fn next_run(&self, rest: &str) -> (u8, usize, Vec<u16>) {
        let Some(first) = rest.chars().next() else { return (0, 0, Vec::new()) };
        if !needs_cid(first) {
            let end = rest.char_indices().find(|(_, c)| needs_cid(*c)).map_or(rest.len(), |(i, _)| i);
            return (0, end, Vec::new());
        }
        // Try the embedded font first: it hands out subset glyph ids as it goes, and stops at the
        // first character it cannot set (that character starts the next run).
        let mut glyphs = Vec::new();
        let mut end = rest.len();
        {
            let mut bank = self.font.try_borrow_mut().ok();
            if let Some(font) = bank.as_mut().and_then(|b| b.as_mut()) {
                for (i, c) in rest.char_indices() {
                    if !needs_cid(c) {
                        end = i;
                        break;
                    }
                    match font.glyph_for(c) {
                        Some(g) => glyphs.push(g),
                        None => {
                            end = i;
                            break;
                        }
                    }
                }
            }
        }
        if !glyphs.is_empty() {
            return (1, end, glyphs);
        }
        // No embedded font (or no glyph for the first character): the CID font takes the run.
        let end = rest.char_indices().find(|(_, c)| !needs_cid(*c)).map_or(rest.len(), |(i, _)| i);
        (2, end, Vec::new())
    }

    /// Draws an RGBA image (`pw`×`ph` pixels, row-major, straight alpha) into the box
    /// `(x, y, w, h)`. Malformed or oversized buffers are ignored.
    pub fn image_rgba(&mut self, x: f32, y: f32, w: f32, h: f32, rgba: &[u8], pw: u32, ph: u32) {
        let px = pw as u64 * ph as u64;
        if pw == 0 || ph == 0 || px > MAX_IMAGE_PIXELS || (rgba.len() as u64) < px * 4 || !(extent(w) && extent(h) && finite(&[x, y])) {
            return;
        }
        let px = px as usize;
        let mut rgb = Vec::with_capacity(px * 3);
        let mut alpha = Vec::with_capacity(px);
        for p in rgba.as_chunks::<4>().0.iter().take(px) {
            rgb.extend_from_slice(&[p[0], p[1], p[2]]);
            alpha.push(p[3]);
        }
        let alpha = if alpha.iter().all(|a| *a == 255) { None } else { Some(alpha) };
        let idx = self.images.len();
        self.images.push(ImageData { width: pw, height: ph, rgb, alpha });
        let s = format!("q {} 0 0 {} {} {} cm /Im{} Do Q", n(w), n(h), n(x), n(self.y(y + h)), idx + 1);
        self.op(&s);
    }
}

/// Document metadata.
#[derive(Clone, Debug, Default)]
pub struct Info {
    pub title: String,
    pub author: String,
    pub creator: String,
}

/// A PDF document under construction.
#[derive(Default)]
pub struct PdfDoc {
    pages: Vec<Page>,
    pub info: Info,
    /// The face embedded for characters WinAnsiEncoding cannot set (see [`PdfDoc::set_font`]).
    font: Rc<RefCell<Option<FontUse>>>,
}

/// PackBits-style run-length encoding (PDF `RunLengthDecode`).
fn run_length(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() / 2 + 16);
    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        let mut run = 1;
        while i + run < data.len() && run < 128 && data[i + run] == b {
            run += 1;
        }
        if run >= 2 {
            out.push((257 - run) as u8);
            out.push(b);
            i += run;
            continue;
        }
        // Literal run until the next repeat of 3+ or 128 bytes.
        let start = i;
        let mut len = 0;
        while i < data.len() && len < 128 {
            if i + 2 < data.len() && data[i] == data[i + 1] && data[i] == data[i + 2] {
                break;
            }
            i += 1;
            len += 1;
        }
        if len == 0 {
            // Next is a repeat; loop will handle it.
            continue;
        }
        out.push((len - 1) as u8);
        out.extend_from_slice(&data[start..start + len]);
    }
    out.push(128);
    out
}

/// Longest Info string written, in characters; longer values are cut on a character boundary.
const MAX_INFO_CHARS: usize = 1024;

/// A PDF text string (document Info entries) for `text`: a literal `( … )` for printable ASCII,
/// otherwise UTF-16BE with a byte-order mark as a hex string (ISO 32000-1 7.9.2.2).
fn pdf_text_string(text: &str) -> String {
    let text: String = text.chars().take(MAX_INFO_CHARS).collect();
    if text.chars().all(|c| matches!(c, ' '..='~')) {
        return pdf_string(&text);
    }
    let mut s = String::with_capacity(text.len() * 4 + 6);
    s.push_str("<FEFF");
    for unit in text.encode_utf16() {
        let _ = write!(s, "{unit:04X}");
    }
    s.push('>');
    s
}

/// Writes the five objects of an embedded `CIDFontType2`: the Type0 font, its descendant, the
/// font descriptor, the font program and the `ToUnicode` CMap. `id` is the Type0 font's object
/// number; the rest follow it.
fn push_embedded_font(objects: &mut Vec<(u32, Vec<u8>)>, id: u32, font: &BuiltFont) {
    let (cid_id, desc_id, file_id, unicode_id) = (id + 1, id + 2, id + 3, id + 4);
    let name = &font.name;
    objects.push((
        id,
        format!(
            "<< /Type /Font /Subtype /Type0 /BaseFont /{name} /Encoding /Identity-H /DescendantFonts [{cid_id} 0 R] /ToUnicode {unicode_id} 0 R >>"
        )
        .into_bytes(),
    ));
    let widths: Vec<String> = font.widths.iter().map(u16::to_string).collect();
    objects.push((
        cid_id,
        format!(
            "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /{name} /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor {desc_id} 0 R /CIDToGIDMap /Identity /DW 1000 /W [0 [{}]] >>",
            widths.join(" ")
        )
        .into_bytes(),
    ));
    let [x0, y0, x1, y1] = font.bbox;
    let cap = (font.ascent * 7 / 10).max(0);
    objects.push((
        desc_id,
        format!(
            "<< /Type /FontDescriptor /FontName /{name} /Flags 4 /FontBBox [{x0} {y0} {x1} {y1}] /ItalicAngle {} /Ascent {} /Descent {} /CapHeight {cap} /StemV 80 /FontFile2 {file_id} 0 R >>",
            n(font.italic),
            font.ascent,
            font.descent
        )
        .into_bytes(),
    ));
    let mut program = format!("<< /Length {} /Length1 {} >>\nstream\n", font.data.len(), font.data.len()).into_bytes();
    program.extend_from_slice(&font.data);
    program.extend_from_slice(b"\nendstream");
    objects.push((file_id, program));
    // `ToUnicode`: subset glyph id → the character it came from, so text can be copied and
    // searched. At most 100 entries per `bfchar` block, as the CMap syntax requires.
    let mut entries: Vec<(char, u16)> = font.unicode.clone();
    entries.sort_unstable_by_key(|(c, gid)| (*gid, *c));
    entries.dedup_by_key(|(_, gid)| *gid);
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    for chunk in entries.chunks(100) {
        let _ = writeln!(cmap, "{} beginbfchar", chunk.len());
        for (c, gid) in chunk {
            let _ = write!(cmap, "<{gid:04X}> <");
            let mut units = [0u16; 2];
            for unit in c.encode_utf16(&mut units) {
                let _ = write!(cmap, "{unit:04X}");
            }
            cmap.push_str(">\n");
        }
        cmap.push_str("endbfchar\n");
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend");
    let mut stream = format!("<< /Length {} >>\nstream\n", cmap.len()).into_bytes();
    stream.extend_from_slice(cmap.as_bytes());
    stream.extend_from_slice(b"\nendstream");
    objects.push((unicode_id, stream));
}

impl PdfDoc {
    pub fn new() -> PdfDoc {
        PdfDoc::default()
    }

    /// Embeds `face` and sets its glyphs for every character WinAnsiEncoding cannot encode.
    ///
    /// One font per document: characters it does not cover keep the non-embedded CID fallback. A
    /// document that sets a font but only draws WinAnsi text is unchanged — the font program is
    /// written only when a page used it.
    pub fn set_font(&mut self, face: Face) {
        if let Ok(mut slot) = self.font.try_borrow_mut() {
            *slot = Some(FontUse::new(face));
        }
    }

    /// The embedded face, if any (for measuring text with [`text_width_with`]).
    pub fn embedded_face(&self) -> Option<Face> {
        self.font.try_borrow().ok().and_then(|f| f.as_ref().map(|u| u.face.clone()))
    }

    /// Adds a page of `width_pt` × `height_pt` points and returns it for drawing.
    pub fn add_page(&mut self, width_pt: f32, height_pt: f32) -> &mut Page {
        let width = if width_pt.is_finite() { width_pt.clamp(3.0, 14_400.0) } else { 612.0 };
        let height = if height_pt.is_finite() { height_pt.clamp(3.0, 14_400.0) } else { 792.0 };
        self.pages.push(Page {
            width,
            height,
            content: String::new(),
            fonts: [false; FONT_SLOTS],
            images: Vec::new(),
            links: Vec::new(),
            depth: 0,
            embedded_used: false,
            font: Rc::clone(&self.font),
        });
        let i = self.pages.len() - 1;
        &mut self.pages[i]
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// The page at `i`, to keep drawing on it.
    pub fn page_mut(&mut self, i: usize) -> Option<&mut Page> {
        self.pages.get_mut(i)
    }

    /// Serializes the document. A document without pages gets one blank Letter page.
    pub fn finish(mut self) -> Vec<u8> {
        if self.pages.is_empty() {
            self.add_page(612.0, 792.0);
        }
        // Object numbering: 1 catalog, 2 page tree, 3 info, then fonts, then per page.
        let mut used = [false; FONT_SLOTS];
        for p in &self.pages {
            for (u, f) in used.iter_mut().zip(p.fonts.iter()) {
                *u |= *f;
            }
        }
        // The embedded subset, built once: glyph ids were assigned while pages were drawn.
        let embedded = if used[EMBED_FONT] { self.font.try_borrow().ok().and_then(|f| f.as_ref().and_then(|u| u.build().ok())) } else { None };
        // A subset that could not be built (a font that parsed but whose outlines are
        // inconsistent) leaves the slot as a standard font: every `/F` reference in a content
        // stream must resolve, so those runs print as garbage but the document stays a valid PDF.
        let mut next = 4u32;
        let mut font_ids = [0u32; FONT_SLOTS];
        for (i, u) in used.iter().enumerate() {
            if *u {
                font_ids[i] = next;
                // The CID font also needs its descendant font and font descriptor; the embedded
                // font adds its font program and `ToUnicode` CMap (and needs none of them when
                // the subset could not be built).
                next += match i {
                    CID_FONT => 3,
                    EMBED_FONT if embedded.is_some() => 5,
                    _ => 1,
                };
            }
        }
        let mut objects: Vec<(u32, Vec<u8>)> = Vec::new();
        let mut kids = Vec::new();
        let mut annotations = Vec::new();
        let page_count = self.pages.len();
        let mut font_dict = String::new();
        for (i, id) in font_ids.iter().enumerate() {
            if *id != 0 {
                let _ = write!(font_dict, "/F{} {} 0 R ", i + 1, id);
                if i == CID_FONT {
                    let (cid_id, desc_id) = (id + 1, id + 2);
                    objects.push((
                        *id,
                        format!(
                            "<< /Type /Font /Subtype /Type0 /BaseFont /{CID_BASE_FONT} /Encoding /UniGB-UTF16-H /DescendantFonts [{cid_id} 0 R] >>"
                        )
                        .into_bytes(),
                    ));
                    objects.push((
                        cid_id,
                        format!(
                            "<< /Type /Font /Subtype /CIDFontType0 /BaseFont /{CID_BASE_FONT} /CIDSystemInfo << /Registry (Adobe) /Ordering (GB1) /Supplement 4 >> /FontDescriptor {desc_id} 0 R /DW {CID_WIDTH} >>"
                        )
                        .into_bytes(),
                    ));
                    objects.push((
                        desc_id,
                        format!(
                            "<< /Type /FontDescriptor /FontName /{CID_BASE_FONT} /Flags 6 /FontBBox [-25 -254 1000 880] /ItalicAngle 0 /Ascent 880 /Descent -120 /CapHeight 880 /StemV 93 >>"
                        )
                        .into_bytes(),
                    ));
                    continue;
                }
                if i == EMBED_FONT {
                    match &embedded {
                        Some(font) => push_embedded_font(&mut objects, *id, font),
                        None => objects.push((*id, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec())),
                    }
                    continue;
                }
                let name = Font::ALL.get(i).copied().unwrap_or_default().base_name();
                let enc = if name.starts_with("Symbol") { "" } else { " /Encoding /WinAnsiEncoding" };
                objects.push((*id, format!("<< /Type /Font /Subtype /Type1 /BaseFont /{name}{enc} >>").into_bytes()));
            }
        }
        for page in &mut self.pages {
            while page.depth > 0 {
                page.restore();
            }
            let mut xobjects = String::new();
            for (k, img) in page.images.iter().enumerate() {
                let img_id = next;
                next += 1;
                let smask = match &img.alpha {
                    Some(a) => {
                        let id = next;
                        next += 1;
                        let data = run_length(a);
                        let mut o = format!(
                            "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /RunLengthDecode /Length {} >>\nstream\n",
                            img.width,
                            img.height,
                            data.len()
                        )
                        .into_bytes();
                        o.extend_from_slice(&data);
                        o.extend_from_slice(b"\nendstream");
                        objects.push((id, o));
                        format!(" /SMask {id} 0 R")
                    }
                    None => String::new(),
                };
                let data = run_length(&img.rgb);
                let mut o = format!(
                    "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8{} /Filter /RunLengthDecode /Length {} >>\nstream\n",
                    img.width,
                    img.height,
                    smask,
                    data.len()
                )
                .into_bytes();
                o.extend_from_slice(&data);
                o.extend_from_slice(b"\nendstream");
                objects.push((img_id, o));
                let _ = write!(xobjects, "/Im{} {} 0 R ", k + 1, img_id);
            }
            let content_id = next;
            let page_id = next + 1;
            next += 2;
            let body = std::mem::take(&mut page.content);
            let mut o = format!("<< /Length {} >>\nstream\n", body.len()).into_bytes();
            o.extend_from_slice(body.as_bytes());
            o.extend_from_slice(b"endstream");
            objects.push((content_id, o));
            let xo = if xobjects.is_empty() { String::new() } else { format!(" /XObject << {xobjects}>>") };
            let mut link_refs = String::new();
            for link in std::mem::take(&mut page.links) {
                if matches!(&link.target, LinkTarget::Page { index, .. } if *index >= page_count) {
                    continue;
                }
                let id = next;
                next += 1;
                let _ = write!(link_refs, "{id} 0 R ");
                annotations.push((id, link));
            }
            let annots = if link_refs.is_empty() { String::new() } else { format!(" /Annots [{link_refs}]") };
            objects.push((
                page_id,
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Resources << /ProcSet [/PDF /Text /ImageB /ImageC] /Font << {}>>{} >> /Contents {} 0 R{} >>",
                    n(page.width),
                    n(page.height),
                    font_dict,
                    xo,
                    content_id,
                    annots
                )
                .into_bytes(),
            ));
            kids.push(page_id);
        }
        for (id, link) in annotations {
            let [x0, y0, x1, y1] = link.rect;
            let action = match link.target {
                LinkTarget::Uri(uri) => format!("/A << /S /URI /URI {} >>", pdf_uri_string(&uri)),
                LinkTarget::Page { index, x, y } => {
                    let Some(&page_id) = kids.get(index) else { continue };
                    let height = self.pages.get(index).map_or(0.0, |p| p.height);
                    format!("/Dest [{page_id} 0 R /XYZ {} {} null]", n(x), n(height - y))
                }
            };
            objects.push((
                id,
                format!("<< /Type /Annot /Subtype /Link /Rect [{} {} {} {}] /Border [0 0 0] {action} >>", n(x0), n(y0), n(x1), n(y1)).into_bytes(),
            ));
        }
        objects.push((1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()));
        let kids_s: Vec<String> = kids.iter().map(|k| format!("{k} 0 R")).collect();
        objects.push((2, format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids_s.join(" "), kids.len()).into_bytes()));
        let creator = if self.info.creator.is_empty() { "GridCraft" } else { &self.info.creator };
        let mut info = format!("<< /Producer {} /Creator {}", pdf_text_string("GridCraft PDF"), pdf_text_string(creator));
        if !self.info.title.is_empty() {
            let _ = write!(info, " /Title {}", pdf_text_string(&self.info.title));
        }
        if !self.info.author.is_empty() {
            let _ = write!(info, " /Author {}", pdf_text_string(&self.info.author));
        }
        info.push_str(" >>");
        objects.push((3, info.into_bytes()));
        objects.sort_by_key(|(id, _)| *id);

        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");
        let mut offsets = vec![0usize; next as usize];
        for (id, body) in &objects {
            if let Some(slot) = offsets.get_mut(*id as usize) {
                *slot = out.len();
            }
            out.extend_from_slice(format!("{id} 0 obj\n").as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        let mut x = format!("xref\n0 {}\n0000000000 65535 f \n", next);
        for off in offsets.iter().skip(1) {
            let _ = writeln!(x, "{off:010} 00000 n ");
        }
        let _ = write!(x, "trailer\n<< /Size {} /Root 1 0 R /Info 3 0 R >>\nstartxref\n{}\n%%EOF\n", next, xref);
        out.extend_from_slice(x.as_bytes());
        out
    }
}

#[cfg(test)]
mod tests;
