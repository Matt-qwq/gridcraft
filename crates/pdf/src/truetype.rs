//! A small TrueType reader and subsetter for embedding text the standard 14 PDF fonts cannot
//! encode.
//!
//! Only what a PDF `CIDFontType2` needs is read: `head`, `hhea`, `maxp`, `hmtx`, `cmap`
//! (formats 0, 4, 6 and 12), `loca`, `glyf`, `name`, `post` and — when present — `OS/2`. CFF/OTF
//! outlines are not supported (`CIDFontType0` + `FontFile3` would be needed), so a CFF font is
//! rejected rather than embedded broken.
//!
//! # Hostile input
//!
//! Font files come from the user's machine and are parsed in-process, so every offset and length
//! is checked against the file it came from, `loca` must be non-decreasing and inside `glyf`, the
//! glyph count is capped, composite glyphs are followed iteratively (a cycle cannot recurse), and
//! table sizes computed from the file are capped before allocating. A malformed font returns
//! [`Error`]; nothing here panics or reads out of bounds.
//!
//! Portions of this file's table layouts follow the OpenType specification (Microsoft/Adobe,
//! OpenType 1.9): all offsets and meanings are taken from that public document.

use std::fmt;
use std::sync::Arc;

/// Something the font file cannot or will not provide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Not an SFNT (`0x00010000`, `true` or a `ttcf` collection with face 0).
    NotSfnt,
    /// A table the reader needs is missing.
    Missing(&'static str),
    /// A table is truncated, out of range or self-inconsistent.
    Bad(&'static str),
    /// More glyphs than a `CIDFontType2` can number.
    TooManyGlyphs,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotSfnt => f.write_str("not a TrueType font"),
            Error::Missing(t) => write!(f, "missing `{t}` table"),
            Error::Bad(t) => write!(f, "malformed `{t}` table"),
            Error::TooManyGlyphs => f.write_str("too many glyphs"),
        }
    }
}

impl std::error::Error for Error {}

/// Largest glyph count a subset can address (a `CIDFontType2` with 2-byte glyph ids).
pub const MAX_GLYPHS: usize = 65_535;
/// Components one composite glyph may reference (guards a hostile font).
const MAX_COMPONENTS: usize = 64;
/// Glyphs one composite closure may reach before the font is treated as unsound.
const MAX_COMPONENT_CLOSURE: usize = 4096;
/// Tables a TrueType face may declare.
const MAX_TABLES: usize = 512;

fn u16_at(d: &[u8], at: usize) -> Option<u16> {
    let b = d.get(at..at.checked_add(2)?)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}
fn i16_at(d: &[u8], at: usize) -> Option<i16> {
    u16_at(d, at).map(|v| v as i16)
}
fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    let b = d.get(at..at.checked_add(4)?)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// A table's tag, offset and length inside the file.
#[derive(Clone, Copy, Debug)]
struct Dir {
    tag: [u8; 4],
    off: usize,
    len: usize,
}

/// A parsed TrueType face (face 0 of a collection). Sharing is cheap: the file bytes live in an
/// [`Arc`], so the engine can keep one face for text measurement while the document owns another
/// handle to the same font.
#[derive(Clone)]
pub struct Face {
    data: Arc<[u8]>,
    dir: Arc<Vec<Dir>>,
    units_per_em: u16,
    num_glyphs: u16,
    loca_format: i16,
    num_h_metrics: u16,
    ascent: i16,
    descent: i16,
    bbox: [i16; 4],
    italic_angle: f32,
    /// Per glyph: 0 = not checked, 1 = its outline data can be read, 2 = it cannot. Shared by
    /// clones (the answer never changes for a given file) and atomic so a [`Face`] stays `Send`.
    sound: Arc<Vec<std::sync::atomic::AtomicU8>>,
}

impl fmt::Debug for Face {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Face")
            .field("family", &self.family_name())
            .field("glyphs", &self.num_glyphs)
            .field("units_per_em", &self.units_per_em)
            .finish()
    }
}

impl Face {
    /// Reads the tables a PDF embedding needs. `data` is the whole font file.
    pub fn parse(data: Vec<u8>) -> Result<Face, Error> {
        let data = Arc::<[u8]>::from(data);
        let dir = Arc::new(read_directory(&data)?);
        let table = |tag: &[u8; 4]| -> Option<&[u8]> {
            let d = dir.iter().find(|d| &d.tag == tag)?;
            data.get(d.off..d.off.checked_add(d.len)?)
        };
        let head = table(b"head").ok_or(Error::Missing("head"))?;
        let hhea = table(b"hhea").ok_or(Error::Missing("hhea"))?;
        let maxp = table(b"maxp").ok_or(Error::Missing("maxp"))?;
        if head.len() < 54 || hhea.len() < 36 || maxp.len() < 6 {
            return Err(Error::Bad("head"));
        }
        for t in [b"hmtx", b"cmap", b"loca", b"glyf"] {
            if table(t).is_none() {
                return Err(Error::Missing(match t {
                    b"hmtx" => "hmtx",
                    b"cmap" => "cmap",
                    b"loca" => "loca",
                    _ => "glyf",
                }));
            }
        }
        let units_per_em = u16_at(head, 18).filter(|u| *u >= 16 && *u <= 16_384).ok_or(Error::Bad("head"))?;
        let loca_format = i16_at(head, 50).ok_or(Error::Bad("head"))?;
        if !matches!(loca_format, 0 | 1) {
            return Err(Error::Bad("head"));
        }
        let num_glyphs = u16_at(maxp, 4).ok_or(Error::Bad("maxp"))?;
        if num_glyphs == 0 {
            return Err(Error::Bad("maxp"));
        }
        let num_h_metrics = u16_at(hhea, 34).ok_or(Error::Bad("hhea"))?;
        if num_h_metrics == 0 || num_h_metrics > num_glyphs {
            return Err(Error::Bad("hhea"));
        }
        let bbox = [
            i16_at(head, 36).ok_or(Error::Bad("head"))?,
            i16_at(head, 38).ok_or(Error::Bad("head"))?,
            i16_at(head, 40).ok_or(Error::Bad("head"))?,
            i16_at(head, 42).ok_or(Error::Bad("head"))?,
        ];
        let ascent = i16_at(hhea, 4).ok_or(Error::Bad("hhea"))?;
        let descent = i16_at(hhea, 6).ok_or(Error::Bad("hhea"))?;
        let italic_angle = table(b"post").and_then(|p| u32_at(p, 4)).map(|v| v as i32 as f32 / 65536.0).filter(|v| v.is_finite()).unwrap_or(0.0);
        let sound = Arc::new((0..num_glyphs).map(|_| std::sync::atomic::AtomicU8::new(0u8)).collect());
        let face = Face { data, dir, units_per_em, num_glyphs, loca_format, num_h_metrics, ascent, descent, bbox, italic_angle, sound };
        // `loca` is validated once here so every later glyph lookup is a slice, not a check.
        face.check_loca()?;
        let cmap = face.table(b"cmap").ok_or(Error::Missing("cmap"))?;
        if cmap.len() > 64 * 1024 * 1024 {
            return Err(Error::Bad("cmap"));
        }
        Ok(face)
    }

    fn table(&self, tag: &[u8; 4]) -> Option<&[u8]> {
        let d = self.dir.iter().find(|d| &d.tag == tag)?;
        self.data.get(d.off..d.off.checked_add(d.len)?)
    }

    /// The font file this face was parsed from, shared (no copy) so the text engine can register
    /// the same bytes for shaping that this subsetter embeds.
    pub fn bytes(&self) -> Arc<[u8]> {
        Arc::clone(&self.data)
    }

    /// Em size in font units.
    pub fn units_per_em(&self) -> u16 {
        self.units_per_em
    }
    /// Number of glyphs the font declares.
    pub fn glyph_count(&self) -> u16 {
        self.num_glyphs
    }
    /// `[xMin, yMin, xMax, yMax]` in font units.
    pub fn bbox(&self) -> [i16; 4] {
        self.bbox
    }
    /// Ascender and descender in font units (positive and negative).
    pub fn ascent_descent(&self) -> (i16, i16) {
        (self.ascent, self.descent)
    }
    /// Italic angle in degrees (from `post`).
    pub fn italic_angle(&self) -> f32 {
        self.italic_angle
    }

    /// The `name` table's family name (name ID 1), or `None`.
    pub fn family_name(&self) -> Option<String> {
        self.name(1)
    }
    /// The `name` table's PostScript name (name ID 6), or `None`.
    pub fn postscript_name(&self) -> Option<String> {
        self.name(6)
    }

    fn name(&self, id: u16) -> Option<String> {
        let t = self.table(b"name")?;
        let count = u16_at(t, 2)? as usize;
        let strings = u16_at(t, 4)? as usize;
        for i in 0..count.min(256) {
            let rec = 6 + i * 12;
            let platform = u16_at(t, rec)?;
            let encoding = u16_at(t, rec + 2)?;
            let language = u16_at(t, rec + 4)?;
            if u16_at(t, rec + 6)? != id {
                continue;
            }
            let len = u16_at(t, rec + 8)? as usize;
            let off = u16_at(t, rec + 10)? as usize;
            let bytes = t.get(strings.checked_add(off)?..strings.checked_add(off)?.checked_add(len)?)?;
            // Windows and Unicode records are UTF-16BE; Macintosh records are MacRoman, which is
            // ASCII for the letters of a font name.
            let text = if platform == 1 {
                bytes.iter().map(|b| *b as char).collect()
            } else {
                let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                String::from_utf16(&units).ok()?
            };
            let text: String = text.chars().filter(|c| !c.is_control()).collect();
            if !text.is_empty() {
                return Some(text);
            }
            // Prefer the Windows English record, but any name is better than none.
            let _ = (encoding, language);
        }
        None
    }

    /// The glyph id `c` maps to (0 = `.notdef`, which means "no glyph").
    pub fn glyph(&self, c: char) -> u16 {
        self.glyph_opt(c).unwrap_or(0)
    }

    /// True when the font has a real glyph for `c`.
    pub fn covers(&self, c: char) -> bool {
        self.glyph(c) != 0
    }

    fn glyph_opt(&self, c: char) -> Option<u16> {
        let cmap = self.table(b"cmap")?;
        let n = u16_at(cmap, 2)? as usize;
        let u = c as u32;
        let mut best: Option<(u8, u16)> = None;
        for i in 0..n.min(64) {
            // A `cmap` subtable record is 8 bytes: platform, encoding, offset.
            let rec = 4 + i * 8;
            let platform = u16_at(cmap, rec)?;
            let encoding = u16_at(cmap, rec + 2)?;
            let at = u32_at(cmap, rec + 4)? as usize;
            let format = u16_at(cmap, at)?;
            if !matches!(format, 0 | 4 | 6 | 12) {
                continue;
            }
            let rank = match (platform, encoding, format) {
                (3, 10, 12) | (0, _, 12) => 0u8,
                (3, 1, 4) | (0, _, 4) => 1,
                (3, _, 6) | (0, _, 6) => 2,
                // A symbol cmap (3,0) maps the private-use range, not Unicode: ignore it.
                (3, 0, _) => continue,
                _ => 3,
            };
            if best.is_some_and(|(r, _)| r <= rank) {
                continue;
            }
            if let Some(g) = cmap_lookup(cmap, at, format, u) {
                best = Some((rank, g));
                if rank == 0 {
                    break;
                }
            }
        }
        best.map(|(_, g)| g)
    }

    /// Advance width of `gid` in font units (0 when the glyph id is out of range).
    pub fn advance(&self, gid: u16) -> u16 {
        let Some(hmtx) = self.table(b"hmtx") else { return 0 };
        let i = gid.min(self.num_h_metrics.saturating_sub(1)) as usize;
        u16_at(hmtx, i * 4).unwrap_or(0)
    }

    /// The `glyf` bytes of `gid` (empty for an empty glyph).
    fn glyph_data(&self, gid: u16) -> Option<&[u8]> {
        if gid >= self.num_glyphs {
            return None;
        }
        let loca = self.table(b"loca")?;
        let glyf = self.table(b"glyf")?;
        let (a, b) = if self.loca_format == 0 {
            (u16_at(loca, gid as usize * 2)? as usize * 2, u16_at(loca, gid as usize * 2 + 2)? as usize * 2)
        } else {
            (u32_at(loca, gid as usize * 4)? as usize, u32_at(loca, gid as usize * 4 + 4)? as usize)
        };
        if b < a {
            return None;
        }
        glyf.get(a..b)
    }

    fn check_loca(&self) -> Result<(), Error> {
        let loca = self.table(b"loca").ok_or(Error::Missing("loca"))?;
        let glyf = self.table(b"glyf").ok_or(Error::Missing("glyf"))?;
        let need = (self.num_glyphs as usize + 1) * if self.loca_format == 0 { 2 } else { 4 };
        if loca.len() < need {
            return Err(Error::Bad("loca"));
        }
        let mut prev = 0usize;
        for gid in 0..=self.num_glyphs {
            let off = if self.loca_format == 0 {
                u16_at(loca, gid as usize * 2).ok_or(Error::Bad("loca"))? as usize * 2
            } else {
                u32_at(loca, gid as usize * 4).ok_or(Error::Bad("loca"))? as usize
            };
            if off < prev || off > glyf.len() {
                return Err(Error::Bad("loca"));
            }
            prev = off;
        }
        Ok(())
    }

    /// True when `gid`'s outline data — and every composite component it needs — can be read.
    ///
    /// A subset built from unsound glyph data would fail at write time, after the text has
    /// already been drawn; checking here lets the caller keep the fallback for that character
    /// instead. The answer is cached per glyph (a component is checked once).
    pub fn sound(&self, gid: u16) -> bool {
        if gid >= self.num_glyphs {
            return false;
        }
        use std::sync::atomic::Ordering;
        match self.sound[gid as usize].load(Ordering::Relaxed) {
            1 => return true,
            2 => return false,
            _ => {}
        }
        let mut visited: Vec<u16> = Vec::new();
        let mut stack = vec![gid];
        let mut ok = true;
        while let Some(g) = stack.pop() {
            if visited.contains(&g) {
                continue;
            }
            if visited.len() >= MAX_COMPONENT_CLOSURE {
                // A font whose glyph graph is this large is not worth drawing: treat it as
                // unsound so the caller keeps the CID fallback instead of spending minutes here.
                ok = false;
                break;
            }
            visited.push(g);
            let Some(data) = self.glyph_data(g) else {
                ok = false;
                break;
            };
            if data.is_empty() {
                continue;
            }
            match self.components(data) {
                Ok(comps) => stack.extend(comps.into_iter().filter(|c| *c < self.num_glyphs)),
                Err(_) => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            // Every glyph walked is sound; `gid` at least.
            for g in visited {
                self.sound[g as usize].store(1, Ordering::Relaxed);
            }
        } else {
            self.sound[gid as usize].store(2, Ordering::Relaxed);
        }
        ok
    }

    /// The component glyph ids of a composite glyph, in file order.
    fn components(&self, glyph: &[u8]) -> Result<Vec<u16>, Error> {
        let mut out = Vec::new();
        if i16_at(glyph, 0).ok_or(Error::Bad("glyf"))? >= 0 {
            return Ok(out);
        }
        let mut at = 10usize;
        for _ in 0..MAX_COMPONENTS {
            let flags = u16_at(glyph, at).ok_or(Error::Bad("glyf"))?;
            let gid = u16_at(glyph, at + 2).ok_or(Error::Bad("glyf"))?;
            out.push(gid);
            let mut step = 4 + if flags & 0x0001 != 0 { 4 } else { 2 };
            if flags & 0x0008 != 0 {
                step += 2;
            } else if flags & 0x0040 != 0 {
                step += 4;
            } else if flags & 0x0080 != 0 {
                step += 8;
            }
            at = at.checked_add(step).ok_or(Error::Bad("glyf"))?;
            if flags & 0x0020 == 0 {
                return Ok(out);
            }
        }
        Err(Error::Bad("glyf"))
    }

    /// Copies a composite glyph, renumbering its components with `map` (source → subset glyph
    /// id). A component that is not in the map is an error: the closure must have added it.
    fn rewrite_composite(&self, glyph: &[u8], map: &[u16]) -> Result<Vec<u8>, Error> {
        let mut out = glyph.to_vec();
        if i16_at(glyph, 0).ok_or(Error::Bad("glyf"))? >= 0 {
            return Ok(out);
        }
        let mut at = 10usize;
        for _ in 0..MAX_COMPONENTS {
            let flags = u16_at(glyph, at).ok_or(Error::Bad("glyf"))?;
            let old = u16_at(glyph, at + 2).ok_or(Error::Bad("glyf"))? as usize;
            // A component the closure did not add (an out-of-range or cyclic reference in a
            // hostile font) becomes `.notdef` rather than an id that does not exist.
            let new = map.get(old).copied().filter(|m| *m != 0xFFFF).unwrap_or(0);
            match out.get_mut(at + 2..at + 4) {
                Some(slot) => slot.copy_from_slice(&new.to_be_bytes()),
                None => return Err(Error::Bad("glyf")),
            }
            let mut step = 4 + if flags & 0x0001 != 0 { 4 } else { 2 };
            if flags & 0x0008 != 0 {
                step += 2;
            } else if flags & 0x0040 != 0 {
                step += 4;
            } else if flags & 0x0080 != 0 {
                step += 8;
            }
            at = at.checked_add(step).ok_or(Error::Bad("glyf"))?;
            if flags & 0x0020 == 0 {
                return Ok(out);
            }
        }
        Err(Error::Bad("glyf"))
    }

    /// Builds a subset font: `.notdef`, then `glyphs` in order, then any composite components
    /// they need. `chars` maps characters to source glyph ids for the rebuilt `cmap`.
    ///
    /// The subset's glyph ids are the indices into the returned [`Subset::gids`], so subset gid 0
    /// is `.notdef`, subset gid 1 is `glyphs[0]`, and so on.
    pub fn subset(&self, glyphs: &[u16], chars: &[(char, u16)]) -> Result<Subset, Error> {
        // Subset order: .notdef, the used glyphs (deduplicated), then composite components.
        let mut order: Vec<u16> = vec![0];
        let mut map: Vec<u16> = vec![0xFFFF; self.num_glyphs as usize];
        map[0] = 0;
        for gid in glyphs {
            if *gid >= self.num_glyphs || map[*gid as usize] != 0xFFFF {
                continue;
            }
            map[*gid as usize] = order.len() as u16;
            order.push(*gid);
        }
        // Iterative closure: a component may itself be composite, and a hostile font may point at
        // itself, so walk a queue and stop when every glyph has been seen.
        let mut i = 1usize;
        while i < order.len() {
            let gid = order[i];
            i += 1;
            let Some(glyph) = self.glyph_data(gid) else { continue };
            if glyph.is_empty() {
                continue;
            }
            for comp in self.components(glyph)? {
                if comp >= self.num_glyphs || map[comp as usize] != 0xFFFF {
                    continue;
                }
                if order.len() >= MAX_GLYPHS {
                    return Err(Error::TooManyGlyphs);
                }
                map[comp as usize] = order.len() as u16;
                order.push(comp);
            }
        }
        // `glyf` and `loca`.
        let (glyf, loca) = self.build_outlines(&order, &map)?;
        // `hmtx`: every subset glyph gets its own advance, so `numberOfHMetrics` can be the glyph
        // count.
        let mut hmtx = Vec::with_capacity(order.len() * 4);
        for gid in &order {
            hmtx.extend_from_slice(&self.advance(*gid).to_be_bytes());
            let lsb = self.glyph_data(*gid).and_then(|g| i16_at(g, 6)).unwrap_or(0);
            hmtx.extend_from_slice(&lsb.to_be_bytes());
        }
        let remapped: Vec<(char, u16)> =
            chars.iter().filter_map(|(c, g)| map.get(*g as usize).copied().filter(|m| *m != 0xFFFF).map(|m| (*c, m))).collect();
        let cmap = build_cmap(&remapped);
        let head = self.build_head();
        let hhea = self.build_hhea(order.len());
        let maxp = self.build_maxp(order.len());
        let name = self.build_name();
        let post = self.build_post();
        let os2 = self.table(b"OS/2").filter(|t| t.len() >= 78).map(|t| t.to_vec());
        let mut tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
            (b"head", head),
            (b"hhea", hhea),
            (b"maxp", maxp),
            (b"hmtx", hmtx),
            (b"cmap", cmap),
            (b"loca", loca),
            (b"glyf", glyf),
            (b"name", name),
            (b"post", post),
        ];
        if let Some(os2) = os2 {
            tables.insert(3, (b"OS/2", os2));
        }
        let data = assemble(&tables)?;
        Ok(Subset { data, gids: order, chars: remapped })
    }

    /// `glyf` and the long-format `loca` that indexes it.
    fn build_outlines(&self, order: &[u16], map: &[u16]) -> Result<(Vec<u8>, Vec<u8>), Error> {
        let mut glyf = Vec::new();
        let mut loca = Vec::with_capacity(order.len() * 4 + 4);
        loca.extend_from_slice(&0u32.to_be_bytes());
        for gid in order {
            let src = self.glyph_data(*gid).unwrap_or(&[]);
            let bytes = if src.is_empty() { Vec::new() } else { self.rewrite_composite(src, map)? };
            // Align each glyph to 4 bytes, as the specification recomments for `loca`.
            glyf.extend_from_slice(&bytes);
            while glyf.len() % 4 != 0 {
                glyf.push(0);
            }
            if glyf.len() > u32::MAX as usize {
                return Err(Error::TooManyGlyphs);
            }
            loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
        }
        Ok((glyf, loca))
    }

    fn build_head(&self) -> Vec<u8> {
        let mut head = self.table(b"head").map(|t| t.to_vec()).unwrap_or_else(|| vec![0; 54]);
        head.resize(54, 0);
        // Long `loca`; the checksum adjustment is written by `assemble`.
        head[50..52].copy_from_slice(&1i16.to_be_bytes());
        head[8..12].copy_from_slice(&0u32.to_be_bytes());
        head
    }

    fn build_hhea(&self, glyphs: usize) -> Vec<u8> {
        let mut hhea = self.table(b"hhea").map(|t| t.to_vec()).unwrap_or_else(|| vec![0; 36]);
        hhea.resize(36, 0);
        hhea[34..36].copy_from_slice(&(glyphs as u16).to_be_bytes());
        hhea
    }

    fn build_maxp(&self, glyphs: usize) -> Vec<u8> {
        let mut maxp = self.table(b"maxp").map(|t| t.to_vec()).unwrap_or_else(|| vec![0; 6]);
        if maxp.len() < 6 {
            maxp.resize(6, 0);
        }
        maxp[4..6].copy_from_slice(&(glyphs as u16).to_be_bytes());
        maxp
    }

    /// A minimal `name` table: family and full font name, UTF-16BE for Windows and MacRoman.
    fn build_name(&self) -> Vec<u8> {
        let family = self.family_name().unwrap_or_else(|| "GridCraft Embedded".to_string());
        let full = self.postscript_name().unwrap_or_else(|| family.clone());
        let clip = |s: &str| -> String { s.chars().take(120).collect() };
        let (family, full) = (clip(&family), clip(&full));
        let mut strings: Vec<u8> = Vec::new();
        // platform, encoding, language, name id, length, offset
        let mut records: Vec<(u16, u16, u16, u16, u16, u16)> = Vec::new();
        for (id, text) in [(1u16, &family), (4u16, &full), (6u16, &full)] {
            let mut utf16 = Vec::with_capacity(text.len() * 2);
            for unit in text.encode_utf16() {
                utf16.extend_from_slice(&unit.to_be_bytes());
            }
            records.push((3, 1, 0x0409, id, utf16.len() as u16, strings.len() as u16));
            strings.extend_from_slice(&utf16);
            let mac: Vec<u8> = text.chars().map(|c| if c.is_ascii() { c as u8 } else { b'?' }).collect();
            records.push((1, 0, 0, id, mac.len() as u16, strings.len() as u16));
            strings.extend_from_slice(&mac);
        }
        let mut out = Vec::with_capacity(6 + records.len() * 12 + strings.len());
        out.extend_from_slice(&0u16.to_be_bytes()); // format
        out.extend_from_slice(&(records.len() as u16).to_be_bytes());
        out.extend_from_slice(&((6 + records.len() * 12) as u16).to_be_bytes()); // stringOffset
        for (platform, encoding, language, id, len, off) in &records {
            out.extend_from_slice(&platform.to_be_bytes());
            out.extend_from_slice(&encoding.to_be_bytes());
            out.extend_from_slice(&language.to_be_bytes());
            out.extend_from_slice(&id.to_be_bytes());
            out.extend_from_slice(&len.to_be_bytes());
            out.extend_from_slice(&off.to_be_bytes());
        }
        out.extend_from_slice(&strings);
        out
    }

    /// A `post` version 3.0 header (no glyph names: a subset renumbers them away).
    fn build_post(&self) -> Vec<u8> {
        let mut post = vec![0u8; 32];
        post[0..4].copy_from_slice(&0x0003_0000u32.to_be_bytes());
        post[4..8].copy_from_slice(&((self.italic_angle * 65536.0) as i32).to_be_bytes());
        post[8..10].copy_from_slice(&(-100i16).to_be_bytes()); // underlinePosition
        post[10..12].copy_from_slice(&50i16.to_be_bytes()); // underlineThickness
        post
    }
}

/// A subset font program.
#[derive(Clone, Debug)]
pub struct Subset {
    /// The standalone TrueType file.
    pub data: Vec<u8>,
    /// Source glyph id per subset glyph id (`gids[0]` is `.notdef`).
    pub gids: Vec<u16>,
    /// Characters the subset maps, with their subset glyph ids.
    pub chars: Vec<(char, u16)>,
}

/// The table directory of face 0.
fn read_directory(data: &[u8]) -> Result<Vec<Dir>, Error> {
    let base = if data.get(0..4) == Some(b"ttcf") {
        let off = u32_at(data, 12).ok_or(Error::NotSfnt)? as usize;
        if off >= data.len() {
            return Err(Error::NotSfnt);
        }
        off
    } else {
        0
    };
    if !matches!(u32_at(data, base), Some(0x0001_0000 | 0x7472_7565)) {
        return Err(Error::NotSfnt);
    }
    let n = u16_at(data, base + 4).ok_or(Error::NotSfnt)? as usize;
    if n == 0 || n > MAX_TABLES {
        return Err(Error::NotSfnt);
    }
    let mut dir = Vec::with_capacity(n);
    for i in 0..n {
        let rec = base + 12 + i.checked_mul(16).ok_or(Error::NotSfnt)?;
        let tag = data.get(rec..rec + 4).ok_or(Error::NotSfnt)?;
        let off = u32_at(data, rec + 8).ok_or(Error::NotSfnt)? as usize;
        let len = u32_at(data, rec + 12).ok_or(Error::NotSfnt)? as usize;
        if off.checked_add(len).is_none_or(|end| end > data.len()) {
            return Err(Error::Bad("table directory"));
        }
        dir.push(Dir { tag: [tag[0], tag[1], tag[2], tag[3]], off, len });
    }
    Ok(dir)
}

fn cmap_lookup(cmap: &[u8], at: usize, format: u16, u: u32) -> Option<u16> {
    match format {
        0 => {
            if u > 0xFF {
                return None;
            }
            u16_at(cmap, at + 6 + u as usize)
        }
        4 => {
            if u > 0xFFFF {
                return None;
            }
            let u = u as u16;
            let seg_x2 = u16_at(cmap, at + 6)? as usize;
            let segs = seg_x2 / 2;
            if segs == 0 {
                return None;
            }
            let end = at + 14;
            let start = end + seg_x2 + 2;
            let delta = start + seg_x2;
            let range = delta + seg_x2;
            for i in 0..segs {
                let e = u16_at(cmap, end + i * 2)?;
                let s = u16_at(cmap, start + i * 2)?;
                if u < s || u > e {
                    continue;
                }
                let d = u16_at(cmap, delta + i * 2)?;
                let ro = u16_at(cmap, range + i * 2)? as usize;
                if ro == 0 {
                    return Some(u.wrapping_add(d));
                }
                let addr = range + i * 2 + ro + (u - s) as usize * 2;
                let g = u16_at(cmap, addr)?;
                return Some(if g == 0 { 0 } else { g.wrapping_add(d) });
            }
            None
        }
        6 => {
            let first = u16_at(cmap, at + 6)? as u32;
            let count = u16_at(cmap, at + 8)? as usize;
            let i = u.checked_sub(first)? as usize;
            if i >= count {
                return None;
            }
            u16_at(cmap, at + 10 + i * 2)
        }
        12 => {
            let groups = u32_at(cmap, at + 12)? as usize;
            // A group is 12 bytes; reject a count the table cannot hold before walking it.
            if at.checked_add(16 + groups.checked_mul(12)?)? > cmap.len() {
                return None;
            }
            for i in 0..groups {
                let g = at + 16 + i * 12;
                let start = u32_at(cmap, g)?;
                let end = u32_at(cmap, g + 4)?;
                if u < start || u > end {
                    continue;
                }
                let base = u32_at(cmap, g + 8)?;
                return Some((base.checked_add(u - start)? & 0xFFFF) as u16);
            }
            None
        }
        _ => None,
    }
}

/// A `cmap` table for the subset: format 12 for every character, plus format 4 for the BMP when
/// it fits (a format 4 subtable's length is a `u16`, so a very large subset gets format 12 only).
fn build_cmap(chars: &[(char, u16)]) -> Vec<u8> {
    let mut all: Vec<(u32, u16)> = chars.iter().map(|(c, g)| (*c as u32, *g)).collect();
    all.sort_unstable();
    all.dedup_by_key(|(c, _)| *c);
    let bmp: Vec<(u16, u16)> = all.iter().filter(|(c, _)| *c < 0xFFFF).map(|(c, g)| (*c as u16, *g)).collect();
    let segs = cmap4_segments(&bmp).len() + 1;
    let format4 = (16 + segs * 8 <= usize::from(u16::MAX)).then(|| build_cmap4(&bmp));
    let format12 = build_cmap12(&all);
    let n = 1 + usize::from(format4.is_some());
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_be_bytes()); // version
    out.extend_from_slice(&(n as u16).to_be_bytes());
    let header = 4 + n * 8;
    let mut body = Vec::new();
    if let Some(f4) = &format4 {
        out.extend_from_slice(&3u16.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&(header as u32).to_be_bytes());
        body.extend_from_slice(f4);
    }
    out.extend_from_slice(&3u16.to_be_bytes());
    out.extend_from_slice(&10u16.to_be_bytes());
    out.extend_from_slice(&((header + body.len()) as u32).to_be_bytes());
    body.extend_from_slice(&format12);
    out.extend_from_slice(&body);
    out
}

/// Runs of consecutive code points whose `glyph - code` delta is constant, one per format 4
/// segment.
fn cmap4_segments(chars: &[(u16, u16)]) -> Vec<(u16, u16, u16)> {
    let mut segs: Vec<(u16, u16, u16)> = Vec::new();
    for (c, g) in chars {
        let d = g.wrapping_sub(*c);
        match segs.last_mut() {
            Some((_, end, dd)) if *dd == d && end.wrapping_add(1) == *c => *end = *c,
            _ => segs.push((*c, *c, d)),
        }
    }
    segs
}

fn build_cmap4(chars: &[(u16, u16)]) -> Vec<u8> {
    let mut segs = cmap4_segments(chars);
    segs.push((0xFFFF, 0xFFFF, 1));
    let n = segs.len() as u16;
    let len = 16 + n as usize * 8;
    let mut out = Vec::with_capacity(len);
    // searchRange = 2 * the largest power of two not above the segment count.
    let mut pow = 1u16;
    let mut selector = 0u16;
    while usize::from(pow) * 2 <= usize::from(n) {
        pow = pow.saturating_mul(2);
        selector += 1;
    }
    let search = pow.saturating_mul(2);
    out.extend_from_slice(&4u16.to_be_bytes());
    out.extend_from_slice(&(len as u16).to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes()); // language
    out.extend_from_slice(&(n * 2).to_be_bytes());
    out.extend_from_slice(&search.to_be_bytes());
    out.extend_from_slice(&selector.to_be_bytes());
    out.extend_from_slice(&(n * 2 - search).to_be_bytes());
    for (_, e, _) in &segs {
        out.extend_from_slice(&e.to_be_bytes());
    }
    out.extend_from_slice(&0u16.to_be_bytes()); // reservedPad
    for (s, _, _) in &segs {
        out.extend_from_slice(&s.to_be_bytes());
    }
    for (_, _, d) in &segs {
        out.extend_from_slice(&d.to_be_bytes());
    }
    for _ in &segs {
        out.extend_from_slice(&0u16.to_be_bytes()); // idRangeOffset
    }
    out
}

fn build_cmap12(chars: &[(u32, u16)]) -> Vec<u8> {
    let mut groups: Vec<(u32, u32, u32)> = Vec::new();
    for (c, g) in chars {
        match groups.last_mut() {
            // A format 12 group needs a constant glyph run too.
            Some((start, end, gs)) if end.wrapping_add(1) == *c && gs.wrapping_add(*end - *start + 1) == *g as u32 => *end = *c,
            _ => groups.push((*c, *c, *g as u32)),
        }
    }
    let n = groups.len() as u32;
    let mut out = Vec::new();
    out.extend_from_slice(&12u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&(16 + n * 12).to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes()); // language
    out.extend_from_slice(&n.to_be_bytes());
    for (s, e, g) in &groups {
        out.extend_from_slice(&s.to_be_bytes());
        out.extend_from_slice(&e.to_be_bytes());
        out.extend_from_slice(&g.to_be_bytes());
    }
    out
}

/// Sum of a table's big-endian `u32` words, zero-padded.
fn checksum(table: &[u8]) -> u32 {
    let mut sum = 0u32;
    for chunk in table.chunks(4) {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum = sum.wrapping_add(u32::from_be_bytes(word));
    }
    sum
}

/// Lays out the tables, their directory, checksums and the `head` checksum adjustment.
fn assemble(tables: &[(&[u8; 4], Vec<u8>)]) -> Result<Vec<u8>, Error> {
    let n = tables.len();
    if n == 0 || n > MAX_TABLES {
        return Err(Error::TooManyGlyphs);
    }
    let mut out = Vec::new();
    let search = {
        let mut pow = 1usize;
        while pow * 2 <= n {
            pow *= 2;
        }
        (pow as u16) * 16
    };
    let entry = (16 - (n as u16).leading_zeros() - 1) as u16;
    let range = (n as u16) * 16 - search;
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    out.extend_from_slice(&(n as u16).to_be_bytes());
    out.extend_from_slice(&search.to_be_bytes());
    out.extend_from_slice(&entry.to_be_bytes());
    out.extend_from_slice(&range.to_be_bytes());
    let header = 12 + n * 16;
    let mut offset = header;
    let mut records = Vec::with_capacity(n);
    for (tag, data) in tables {
        records.push((**tag, checksum(data), offset, data.len()));
        offset = offset.checked_add(data.len() + (4 - data.len() % 4) % 4).ok_or(Error::TooManyGlyphs)?;
    }
    let head_at = tables.iter().position(|(t, _)| *t == b"head").ok_or(Error::Missing("head"))?;
    for (tag, sum, off, len) in &records {
        out.extend_from_slice(tag);
        out.extend_from_slice(&sum.to_be_bytes());
        out.extend_from_slice(&(*off as u32).to_be_bytes());
        out.extend_from_slice(&(*len as u32).to_be_bytes());
    }
    let mut head_offset = 0usize;
    for (i, (_, data)) in tables.iter().enumerate() {
        if i == head_at {
            head_offset = out.len();
        }
        out.extend_from_slice(data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    // `head.checkSumAdjustment` = 0xB1B0AFBA - checksum(whole font with the field zero).
    let sum = checksum(&out);
    let adjust = 0xB1B0_AFBAu32.wrapping_sub(sum).to_be_bytes();
    match out.get_mut(head_offset + 8..head_offset + 12) {
        Some(slot) => slot.copy_from_slice(&adjust),
        None => return Err(Error::Bad("head")),
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
