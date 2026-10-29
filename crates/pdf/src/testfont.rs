//! A synthetic TrueType font built in code, for the reader and subsetter tests.
//!
//! The repository forbids committed binaries (AGENTS.md, craftrules `standards/test-corpora.md`),
//! so the fixture is generated: a few triangle glyphs, one composite glyph that references two of
//! them, a `cmap` in format 4 (plus an isolated format 12 face in `astral`), and the tables a
//! `CIDFontType2` needs. Nothing here is copied from a vendor font — the outlines are a triangle
//! written by hand.

/// A `glyf` glyph: a triangle with an integer triangle outline in font units.
pub(crate) fn simple_glyph(advance: i16) -> Vec<u8> {
    let mut g = Vec::new();
    g.extend_from_slice(&1i16.to_be_bytes()); // numberOfContours
    for v in [0i16, 0, advance, advance] {
        // xMin, yMin, xMax, yMax
        g.extend_from_slice(&v.to_be_bytes());
    }
    g[2..4].copy_from_slice(&0i16.to_be_bytes()); // xMin
    g.extend_from_slice(&3u16.to_be_bytes()); // endPtsOfContours[0] = 3
    g.extend_from_slice(&0u16.to_be_bytes()); // instructionLength
    g.extend_from_slice(&[0x01, 0x01, 0x01, 0x01]); // on-curve, 16-bit deltas
    for d in [0i16, advance, 0, -advance] {
        g.extend_from_slice(&d.to_be_bytes()); // x deltas
    }
    for d in [0i16, 0, advance, -advance] {
        g.extend_from_slice(&d.to_be_bytes()); // y deltas
    }
    g
}

/// A composite glyph: `components` placed at the origin, all standard flags set so the records
/// have the widest form (the reader must skip the argument words correctly).
pub(crate) fn composite_glyph(components: &[u16]) -> Vec<u8> {
    let mut g = Vec::new();
    g.extend_from_slice(&(-1i16).to_be_bytes());
    for v in [0i16, 0, 100, 100] {
        g.extend_from_slice(&v.to_be_bytes());
    }
    for (i, c) in components.iter().enumerate() {
        let last = i + 1 == components.len();
        let mut flags = 0x0001u16; // ARG_1_AND_2_ARE_WORDS
        if !last {
            flags |= 0x0020; // MORE_COMPONENTS
        }
        g.extend_from_slice(&flags.to_be_bytes());
        g.extend_from_slice(&c.to_be_bytes());
        g.extend_from_slice(&0i16.to_be_bytes()); // dx
        g.extend_from_slice(&0i16.to_be_bytes()); // dy
    }
    g
}

/// Builds a whole font file. Glyph 0 is `.notdef` (empty); every later glyph is a triangle.
#[derive(Clone, Debug)]
pub(crate) struct Builder {
    glyphs: Vec<Vec<u8>>,
    advances: Vec<u16>,
    cmap: Vec<(char, u16)>,
    pub(crate) loca_long: bool,
    pub(crate) family: String,
}

impl Default for Builder {
    fn default() -> Self {
        Builder { glyphs: vec![Vec::new()], advances: vec![600], cmap: Vec::new(), loca_long: true, family: "GridCraft Synthetic".to_string() }
    }
}

impl Builder {
    /// Adds a triangle glyph and returns its glyph id.
    pub(crate) fn glyph(&mut self, advance: u16) -> u16 {
        self.glyphs.push(simple_glyph(advance as i16));
        self.advances.push(advance);
        (self.glyphs.len() - 1) as u16
    }

    /// Adds a composite glyph of `components` and returns its glyph id.
    pub(crate) fn composite(&mut self, components: &[u16]) -> u16 {
        self.glyphs.push(composite_glyph(components));
        self.advances.push(800);
        (self.glyphs.len() - 1) as u16
    }

    /// Maps `c` to `gid` in the `cmap`.
    pub(crate) fn map(&mut self, c: char, gid: u16) {
        self.cmap.push((c, gid));
    }

    /// The font's tables, in directory order. Tests mutate this to build a hostile font.
    pub(crate) fn tables(&self) -> Vec<(&'static [u8; 4], Vec<u8>)> {
        let n = self.glyphs.len();
        let mut glyf = Vec::new();
        let mut loca = Vec::new();
        if self.loca_long {
            loca.extend_from_slice(&0u32.to_be_bytes());
        } else {
            loca.extend_from_slice(&0u16.to_be_bytes());
        }
        for g in &self.glyphs {
            glyf.extend_from_slice(g);
            while glyf.len() % 4 != 0 {
                glyf.push(0);
            }
            if self.loca_long {
                loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
            } else {
                loca.extend_from_slice(&((glyf.len() / 2) as u16).to_be_bytes());
            }
        }
        let mut hmtx = Vec::new();
        for a in &self.advances {
            hmtx.extend_from_slice(&a.to_be_bytes());
            hmtx.extend_from_slice(&0i16.to_be_bytes());
        }
        // head. The top eight bytes of the version are 0x00010000.
        let mut head = vec![0u8; 54];
        head[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        head[4..8].copy_from_slice(&0x0001_0000u32.to_be_bytes()); // fontRevision
        head[12..16].copy_from_slice(&0x5F0F_3CF5u32.to_be_bytes()); // magicNumber
        head[16..18].copy_from_slice(&0x000Bu16.to_be_bytes()); // flags
        head[18..20].copy_from_slice(&1000u16.to_be_bytes()); // unitsPerEm
        head[36..38].copy_from_slice(&0i16.to_be_bytes());
        head[38..40].copy_from_slice(&0i16.to_be_bytes());
        head[40..42].copy_from_slice(&1000i16.to_be_bytes());
        head[42..44].copy_from_slice(&1000i16.to_be_bytes());
        head[50..52].copy_from_slice(&(if self.loca_long { 1i16 } else { 0i16 }).to_be_bytes());
        // hhea
        let mut hhea = vec![0u8; 36];
        hhea[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        hhea[4..6].copy_from_slice(&800i16.to_be_bytes()); // ascender
        hhea[6..8].copy_from_slice(&(-200i16).to_be_bytes()); // descender
        hhea[10..12].copy_from_slice(&1000u16.to_be_bytes()); // advanceWidthMax
        hhea[34..36].copy_from_slice(&(n as u16).to_be_bytes()); // numberOfHMetrics
        // maxp 1.0
        let mut maxp = vec![0u8; 32];
        maxp[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        maxp[4..6].copy_from_slice(&(n as u16).to_be_bytes());
        maxp[6..8].copy_from_slice(&4u16.to_be_bytes()); // maxPoints
        // name: family (1), full (4), PostScript (6), Windows UTF-16BE.
        let mut name = Vec::new();
        let mut strings = Vec::new();
        let ps: String = self.family.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        let names = [(1u16, self.family.clone()), (4u16, self.family.clone()), (6u16, ps)];
        name.extend_from_slice(&0u16.to_be_bytes());
        name.extend_from_slice(&(names.len() as u16).to_be_bytes());
        name.extend_from_slice(&((6 + names.len() * 12) as u16).to_be_bytes());
        for (id, text) in &names {
            let off = strings.len() as u16;
            let mut utf16 = Vec::new();
            for u in text.encode_utf16() {
                utf16.extend_from_slice(&u.to_be_bytes());
            }
            name.extend_from_slice(&3u16.to_be_bytes());
            name.extend_from_slice(&1u16.to_be_bytes());
            name.extend_from_slice(&0x0409u16.to_be_bytes());
            name.extend_from_slice(&id.to_be_bytes());
            name.extend_from_slice(&(utf16.len() as u16).to_be_bytes());
            name.extend_from_slice(&off.to_be_bytes());
            strings.extend_from_slice(&utf16);
        }
        name.extend_from_slice(&strings);
        // post 3.0
        let mut post = vec![0u8; 32];
        post[0..4].copy_from_slice(&0x0003_0000u32.to_be_bytes());
        vec![
            (b"cmap", self.cmap_table()),
            (b"glyf", glyf),
            (b"head", head),
            (b"hhea", hhea),
            (b"hmtx", hmtx),
            (b"loca", loca),
            (b"maxp", maxp),
            (b"name", name),
            (b"post", post),
        ]
    }

    /// A `cmap` with one format 4 subtable (plus a format 12 one for astral characters).
    pub(crate) fn cmap_table(&self) -> Vec<u8> {
        let mut bmp: Vec<(u16, u16)> = self.cmap.iter().filter(|(c, _)| (*c as u32) < 0xFFFF).map(|(c, g)| (*c as u16, *g)).collect();
        bmp.sort_unstable();
        let mut segs: Vec<(u16, u16, u16)> = Vec::new();
        for (c, g) in &bmp {
            let d = g.wrapping_sub(*c);
            match segs.last_mut() {
                Some((_, end, dd)) if *dd == d && end.wrapping_add(1) == *c => *end = *c,
                _ => segs.push((*c, *c, d)),
            }
        }
        segs.push((0xFFFF, 0xFFFF, 1));
        let n = segs.len() as u16;
        let mut f4 = Vec::new();
        f4.extend_from_slice(&4u16.to_be_bytes());
        f4.extend_from_slice(&((16 + n as usize * 8) as u16).to_be_bytes());
        f4.extend_from_slice(&0u16.to_be_bytes());
        f4.extend_from_slice(&(n * 2).to_be_bytes());
        let search = 2 * (1u16 << (15 - n.leading_zeros()));
        f4.extend_from_slice(&search.to_be_bytes());
        f4.extend_from_slice(&((15 - n.leading_zeros()) as u16).to_be_bytes());
        f4.extend_from_slice(&(n * 2 - search).to_be_bytes());
        for (_, e, _) in &segs {
            f4.extend_from_slice(&e.to_be_bytes());
        }
        f4.extend_from_slice(&0u16.to_be_bytes());
        for (s, _, _) in &segs {
            f4.extend_from_slice(&s.to_be_bytes());
        }
        for (_, _, d) in &segs {
            f4.extend_from_slice(&d.to_be_bytes());
        }
        for _ in &segs {
            f4.extend_from_slice(&0u16.to_be_bytes());
        }
        // A face with a (3, 10) format 12 subtable must list *every* character there: a shaper
        // picks the best subtable by platform/encoding and does not fall back to format 4 (real
        // fonts write both complete). The format 4 table below stays the BMP-only twin.
        let has_astral = self.cmap.iter().any(|(c, _)| (*c as u32) >= 0xFFFF);
        let f12 = has_astral.then(|| {
            let mut all: Vec<(u32, u16)> = self.cmap.iter().map(|(c, g)| (*c as u32, *g)).collect();
            all.sort_unstable();
            let cnt = all.len() as u32;
            let mut f = Vec::new();
            f.extend_from_slice(&12u16.to_be_bytes());
            f.extend_from_slice(&0u16.to_be_bytes());
            f.extend_from_slice(&(16 + cnt * 12).to_be_bytes());
            f.extend_from_slice(&0u32.to_be_bytes());
            f.extend_from_slice(&cnt.to_be_bytes());
            for (c, g) in &all {
                f.extend_from_slice(&c.to_be_bytes());
                f.extend_from_slice(&c.to_be_bytes());
                f.extend_from_slice(&(*g as u32).to_be_bytes());
            }
            f
        });
        let subtables = if f12.is_some() { 2usize } else { 1 };
        let mut out = Vec::new();
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&(subtables as u16).to_be_bytes());
        let header = 4 + subtables * 8;
        out.extend_from_slice(&3u16.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&(header as u32).to_be_bytes());
        let mut body = f4;
        if let Some(f) = f12 {
            out.extend_from_slice(&3u16.to_be_bytes());
            out.extend_from_slice(&10u16.to_be_bytes());
            out.extend_from_slice(&((header + body.len()) as u32).to_be_bytes());
            body.extend_from_slice(&f);
        }
        out.extend_from_slice(&body);
        out
    }

    /// The finished font file.
    pub(crate) fn build(&self) -> Vec<u8> {
        assemble(&self.tables())
    }
}

/// Lays the tables out behind a directory, with the `head` checksum adjustment.
pub(crate) fn assemble(tables: &[(&'static [u8; 4], Vec<u8>)]) -> Vec<u8> {
    let n = tables.len();
    let mut search = 1usize;
    let mut entry = 0u16;
    while search * 2 <= n {
        search *= 2;
        entry += 1;
    }
    let mut out = Vec::new();
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    out.extend_from_slice(&(n as u16).to_be_bytes());
    out.extend_from_slice(&((search * 16) as u16).to_be_bytes());
    out.extend_from_slice(&entry.to_be_bytes());
    out.extend_from_slice(&(((n * 16) - search * 16) as u16).to_be_bytes());
    let mut offset = 12 + n * 16;
    let mut records = Vec::new();
    for (tag, data) in tables {
        records.push((*tag, offset, data.len()));
        offset += data.len() + (4 - data.len() % 4) % 4;
    }
    for (tag, off, len) in &records {
        let sum = checksum(tables.iter().find(|(t, _)| t == tag).map(|(_, d)| d.as_slice()).unwrap_or(&[]));
        out.extend_from_slice(*tag);
        out.extend_from_slice(&sum.to_be_bytes());
        out.extend_from_slice(&(*off as u32).to_be_bytes());
        out.extend_from_slice(&(*len as u32).to_be_bytes());
    }
    let mut head_at = 0usize;
    for (tag, data) in tables {
        if *tag == b"head" {
            head_at = out.len();
        }
        out.extend_from_slice(data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    let adjust = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out)).to_be_bytes();
    out[head_at + 8..head_at + 12].copy_from_slice(&adjust);
    out
}

fn checksum(table: &[u8]) -> u32 {
    let mut sum = 0u32;
    for chunk in table.chunks(4) {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum = sum.wrapping_add(u32::from_be_bytes(word));
    }
    sum
}

/// A font that covers Latin, Greek, Cyrillic, CJK, Thai and one astral character, with glyph ids
/// stable across tests:
///
/// | gid | character | kind |
/// | --- | --- | --- |
/// | 0 | — | `.notdef` |
/// | 1 | `A` | triangle |
/// | 2 | `Ж` | triangle |
/// | 3 | `Ω` | triangle |
/// | 4 | `字` | triangle |
/// | 5 | — | composite of 1 + 2 |
/// | 6 | `ก` | triangle |
/// | 7 | `😀` | triangle |
pub(crate) fn basic() -> Builder {
    let mut b = Builder::default();
    let a = b.glyph(600);
    let zhe = b.glyph(650);
    let omega = b.glyph(700);
    let han = b.glyph(1000);
    let comp = b.composite(&[a, zhe]);
    let thai = b.glyph(480);
    let astral = b.glyph(900);
    b.map('A', a);
    b.map('Ж', zhe);
    b.map('Ω', omega);
    b.map('字', han);
    b.map('ก', thai);
    b.map('😀', astral);
    // The composite has no character of its own; it is pulled in as a component in a test.
    let _ = comp;
    b
}

/// A font with a composite glyph that maps to `é` so a subset test can pull it in by character.
pub(crate) fn with_composite() -> Builder {
    let mut b = Builder::default();
    let a = b.glyph(600);
    let zhe = b.glyph(650);
    let comp = b.composite(&[a, zhe]);
    b.map('A', a);
    b.map('Ж', zhe);
    b.map('é', comp);
    b
}
