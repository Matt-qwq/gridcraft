//! Reader and subsetter tests, all against the synthetic font in [`crate::testfont`].

use super::*;
use crate::testfont;

fn basic_face() -> Face {
    Face::parse(testfont::basic().build()).expect("the synthetic font parses")
}

#[test]
fn reads_the_tables_the_pdf_writer_needs() {
    let face = basic_face();
    assert_eq!(face.glyph_count(), 8);
    assert_eq!(face.units_per_em(), 1000);
    assert_eq!(face.family_name().as_deref(), Some("GridCraft Synthetic"));
    assert_eq!(face.postscript_name().as_deref(), Some("GridCraftSynthetic"));
    assert_eq!(face.bbox()[2], 1000);
    assert_eq!(face.ascent_descent(), (800, -200));
    // cmap (format 4) for BMP characters, format 12 for the astral one.
    assert_eq!(face.glyph('A'), 1);
    assert_eq!(face.glyph('Ж'), 2);
    assert_eq!(face.glyph('Ω'), 3);
    assert_eq!(face.glyph('字'), 4);
    assert_eq!(face.glyph('ก'), 6);
    assert_eq!(face.glyph('😀'), 7);
    assert_eq!(face.glyph('Z'), 0, "an unmapped character is .notdef");
    assert!(face.covers('字') && !face.covers('Z'));
    // hmtx advances in font units.
    assert_eq!(face.advance(1), 600);
    assert_eq!(face.advance(2), 650);
    assert_eq!(face.advance(4), 1000);
    // Glyph ids past `numberOfHMetrics` reuse the last advance (the specification's rule), and
    // an id past the glyph count is clamped the same way: nothing is indexed out of bounds.
    assert_eq!(face.advance(999), 900);
}

#[test]
fn short_loca_files_parse_too() {
    let mut b = testfont::basic();
    b.loca_long = false;
    let face = Face::parse(b.build()).expect("short loca parses");
    assert_eq!(face.glyph('Ж'), 2);
    assert_eq!(face.advance(2), 650);
}

#[test]
fn subset_keeps_only_the_used_glyphs() {
    let face = basic_face();
    let glyphs = [face.glyph('Ж'), face.glyph('字')];
    let chars = [('Ж', face.glyph('Ж')), ('字', face.glyph('字'))];
    let subset = face.subset(&glyphs, &chars).expect("subset");
    assert_eq!(subset.gids, vec![0, 2, 4], ".notdef, then the used glyphs in order");
    assert_eq!(subset.chars, vec![('Ж', 1), ('字', 2)]);
    // The subset is a font in its own right.
    let sub = Face::parse(subset.data.clone()).expect("the subset parses");
    assert_eq!(sub.glyph_count(), 3);
    assert_eq!(sub.glyph('Ж'), 1);
    assert_eq!(sub.glyph('字'), 2);
    assert_eq!(sub.advance(1), 650, "the advance width came from hmtx");
    assert_eq!(sub.advance(2), 1000);
    assert!(subsubset_is_consistent(&subset.data));
}

/// Every glyph offset in `loca` is non-decreasing and inside `glyf`.
fn subsubset_is_consistent(data: &[u8]) -> bool {
    let face = match Face::parse(data.to_vec()) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let Some(loca) = face.table(b"loca") else { return false };
    let Some(glyf) = face.table(b"glyf") else { return false };
    let n = face.glyph_count() as usize;
    let mut prev = 0u32;
    for i in 0..=n {
        let Some(off) = u32_at(loca, i * 4) else { return false };
        if off < prev || off as usize > glyf.len() {
            return false;
        }
        prev = off;
    }
    // The glyph count in maxp matches the loca length.
    loca.len() == (n + 1) * 4
}

#[test]
fn composite_glyphs_pull_in_their_components() {
    let face = Face::parse(testfont::with_composite().build()).expect("parse");
    let comp = face.glyph('é');
    assert_eq!(comp, 3);
    let subset = face.subset(&[comp], &[('é', comp)]).expect("subset");
    // .notdef, the composite, then its two components.
    assert_eq!(subset.gids, vec![0, 3, 1, 2]);
    assert_eq!(subset.chars, vec![('é', 1)]);
    let sub = Face::parse(subset.data.clone()).expect("the subset parses");
    assert_eq!(sub.glyph_count(), 4);
    // The composite in the subset points at its renumbered components.
    assert!(subsubset_is_consistent(&subset.data));
    let components = sub.components(sub.glyph_data(1).expect("composite data")).expect("components");
    assert_eq!(components, vec![2, 3], "component glyph ids were renumbered");
}

#[test]
fn a_composite_that_points_at_itself_terminates() {
    let mut b = testfont::Builder::default();
    let a = b.glyph(600);
    // A composite that references itself and `a` twice.
    let comp = b.composite(&[a]);
    let mut tables = b.tables();
    // Patch the composite glyph's component id to its own glyph id.
    if let Some((_, glyf)) = tables.iter_mut().find(|(t, _)| *t == b"glyf") {
        let at = glyf.len() - 18 + 12; // last glyph, component glyphIndex
        glyf[at..at + 2].copy_from_slice(&comp.to_be_bytes());
    }
    let face = Face::parse(testfont::assemble(&tables)).expect("parse");
    let subset = face.subset(&[comp], &[]).expect("a self-referencing composite still subsets");
    assert!(subset.gids.contains(&comp));
    assert!(Face::parse(subset.data).is_ok());
}

#[test]
fn a_component_out_of_range_is_ignored() {
    let mut b = testfont::Builder::default();
    let comp = b.composite(&[0xFFFF]);
    let face = Face::parse(b.build()).expect("parse");
    let subset = face.subset(&[comp], &[]).expect("subset");
    assert_eq!(subset.gids, vec![0, comp], "the missing component is not added");
}

#[test]
fn malformed_fonts_are_errors_not_panics() {
    assert_eq!(Face::parse(Vec::new()).err(), Some(Error::NotSfnt));
    assert_eq!(Face::parse(vec![0u8; 100]).err(), Some(Error::NotSfnt));

    let font = testfont::basic().build();
    for n in [12, 60, 100, font.len() / 2, font.len() - 1] {
        // Any prefix either parses or errors; neither may panic.
        let _ = Face::parse(font[..n].to_vec());
    }

    // A table record pointing outside the file.
    let mut bad = font.clone();
    bad[12 + 8..12 + 12].copy_from_slice(&0xFFFF_0000u32.to_be_bytes());
    assert!(Face::parse(bad).is_err());

    // A glyph count that the `loca` cannot hold.
    let mut b = testfont::Builder::default();
    let _ = b.glyph(600);
    let mut tables = b.tables();
    if let Some((_, maxp)) = tables.iter_mut().find(|(t, _)| *t == b"maxp") {
        maxp[4..6].copy_from_slice(&4000u16.to_be_bytes());
    }
    assert_eq!(Face::parse(testfont::assemble(&tables)).err(), Some(Error::Bad("loca")));

    // A `loca` whose offsets go backwards.
    let mut tables = testfont::basic().tables();
    if let Some((_, loca)) = tables.iter_mut().find(|(t, _)| *t == b"loca") {
        loca[4..8].copy_from_slice(&0x00FF_0000u32.to_be_bytes());
    }
    assert_eq!(Face::parse(testfont::assemble(&tables)).err(), Some(Error::Bad("loca")));

    // A truncated table directory.
    let mut tables = testfont::basic().tables();
    tables.retain(|(t, _)| *t != b"hhea");
    assert_eq!(Face::parse(testfont::assemble(&tables)).err(), Some(Error::Missing("hhea")));

    // A CFF/OTF font is refused (the PDF writer can only embed TrueType outlines).
    let mut otf = font.clone();
    otf[0..4].copy_from_slice(b"OTTO");
    assert_eq!(Face::parse(otf).err(), Some(Error::NotSfnt));
}

#[test]
fn names_and_tables_survive_hostile_values() {
    let face = basic_face();
    // A glyph id past the end has no data and does not panic.
    assert!(face.glyph_data(9999).is_none());
    assert_eq!(face.components(&[]), return_bad());
    // A cmap with a huge record count and no subtable bytes behind the records does not walk
    // past the table.
    let b = testfont::basic();
    let mut tables = b.tables();
    if let Some((_, cmap)) = tables.iter_mut().find(|(t, _)| *t == b"cmap") {
        cmap[2..4].copy_from_slice(&0xFFFFu16.to_be_bytes());
        cmap.truncate(20);
    }
    let face = Face::parse(testfont::assemble(&tables)).expect("parse");
    assert_eq!(face.glyph('A'), 0, "a broken cmap maps nothing");
}

fn return_bad() -> Result<Vec<u16>, Error> {
    Err(Error::Bad("glyf"))
}

#[test]
fn empty_and_huge_subset_requests_are_bounded() {
    let face = basic_face();
    // No glyphs: a valid font with only `.notdef`.
    let subset = face.subset(&[], &[]).expect("subset");
    assert_eq!(subset.gids, vec![0]);
    let sub = Face::parse(subset.data).expect("parse");
    assert_eq!(sub.glyph_count(), 1);
    // Glyph ids out of range are dropped, not indexed.
    let subset = face.subset(&[0, 8, 9, u16::MAX], &[('A', u16::MAX)]).expect("subset");
    assert_eq!(subset.gids, vec![0], "only .notdef survives");
    assert!(subset.chars.is_empty());
}

/// The subset is a real font, not only something our own reader accepts: skrifa (the parser
/// epaint uses for system fonts, already in the workspace) reads it, finds the character and
/// draws its outline.
#[test]
fn an_independent_parser_accepts_the_subset() {
    use skrifa::prelude::*;
    let face = Face::parse(testfont::with_composite().build()).expect("parse");
    let chars = [('A', face.glyph('A')), ('Ж', face.glyph('Ж')), ('é', face.glyph('é'))];
    let glyphs: Vec<u16> = chars.iter().map(|(_, g)| *g).collect();
    let subset = face.subset(&glyphs, &chars).expect("subset");
    let font = FontRef::new(&subset.data).expect("skrifa accepts the subset");
    let cmap = font.charmap();
    assert_eq!(cmap.map('A'), Some(GlyphId::new(1)));
    assert_eq!(cmap.map('Ж'), Some(GlyphId::new(2)));
    assert_eq!(cmap.map('é'), Some(GlyphId::new(3)), "a composite glyph");
    assert_eq!(cmap.map('B'), None);
    let outlines = font.outline_glyphs();
    for gid in 1..=3u16 {
        let glyph = outlines.get(GlyphId::new(u32::from(gid))).expect("an outline");
        let mut pen = skrifa::outline::pen::SvgPen::new();
        glyph.draw(Size::unscaled(), &mut pen).expect("draws");
        assert!(!pen.to_string().is_empty(), "glyph {gid} has an outline");
    }
    // The metrics agree with `hmtx`.
    let metrics = font.glyph_metrics(Size::unscaled(), LocationRef::default());
    assert_eq!(metrics.advance_width(GlyphId::new(2)), Some(650.0));
}
