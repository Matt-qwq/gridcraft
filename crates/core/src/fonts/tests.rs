//! Font discovery tests. The SFNT fixtures are built in code — no font binary is committed (the
//! repository forbids that, and a font file would be a vendored asset besides).

use super::*;

/// Big-endian writers for the fixture.
fn be16(v: u16) -> [u8; 2] {
    v.to_be_bytes()
}
fn be32(v: u32) -> [u8; 4] {
    v.to_be_bytes()
}

/// A `cmap` format 4 subtable mapping each `(char, gid)` with idDelta arithmetic; contiguous runs
/// with a constant `gid - char` share one segment, everything else gets its own.
fn cmap4(entries: &[(u16, u16)]) -> Vec<u8> {
    let mut sorted = entries.to_vec();
    sorted.sort_unstable();
    let mut segs: Vec<(u16, u16, u16)> = Vec::new();
    for (c, g) in sorted {
        let d = g.wrapping_sub(c);
        match segs.last_mut() {
            Some((s, e, dd)) if *dd == d && e.wrapping_add(1) == c => *e = c,
            _ => segs.push((c, c, d)),
        }
    }
    segs.push((0xFFFF, 0xFFFF, 1));
    let n = segs.len() as u16;
    let len = 16 + n as usize * 8;
    let mut out = Vec::with_capacity(len);
    out.extend_from_slice(&be16(4));
    out.extend_from_slice(&be16(len as u16));
    out.extend_from_slice(&be16(0)); // language
    out.extend_from_slice(&be16(n * 2));
    out.extend_from_slice(&be16(2)); // searchRange
    out.extend_from_slice(&be16(1)); // entrySelector
    out.extend_from_slice(&be16(0)); // rangeShift
    for (_, e, _) in &segs {
        out.extend_from_slice(&be16(*e));
    }
    out.extend_from_slice(&be16(0)); // reservedPad
    for (s, _, _) in &segs {
        out.extend_from_slice(&be16(*s));
    }
    for (s, _, d) in &segs {
        // The 0xFFFF segment uses idDelta 1 so it maps to gid 0.
        let d = if *s == 0xFFFF { 1u16 } else { *d };
        out.extend_from_slice(&be16(d));
    }
    for _ in &segs {
        out.extend_from_slice(&be16(0)); // idRangeOffset, so idDelta decides
    }
    out
}

/// A `cmap` format 12 subtable (all of Unicode), one group per entry.
fn cmap12(entries: &[(u32, u32)]) -> Vec<u8> {
    let n = entries.len() as u32;
    let mut out = Vec::new();
    out.extend_from_slice(&be16(12));
    out.extend_from_slice(&be16(0));
    out.extend_from_slice(&be32(16 + n * 12));
    out.extend_from_slice(&be32(0)); // language
    out.extend_from_slice(&be32(n));
    for (c, g) in entries {
        out.extend_from_slice(&be32(*c));
        out.extend_from_slice(&be32(*c));
        out.extend_from_slice(&be32(*g));
    }
    out
}

/// An SFNT with one `cmap` table (platform 3, encoding 1 for BMP subtables / 10 for full).
fn sfnt_with_cmap(sub: &[u8], encoding: u16) -> Vec<u8> {
    let mut cmap = Vec::new();
    cmap.extend_from_slice(&be16(0)); // version
    cmap.extend_from_slice(&be16(1)); // one record
    cmap.extend_from_slice(&be16(3));
    cmap.extend_from_slice(&be16(encoding));
    cmap.extend_from_slice(&be32(12)); // one 8-byte record follows the 4-byte header
    cmap.extend_from_slice(sub);

    let mut out = Vec::new();
    out.extend_from_slice(&be32(0x0001_0000));
    out.extend_from_slice(&be16(1)); // numTables
    out.extend_from_slice(&be16(16));
    out.extend_from_slice(&be16(0));
    out.extend_from_slice(&be16(0));
    out.extend_from_slice(b"cmap");
    out.extend_from_slice(&be32(0)); // checksum (unchecked by the probe)
    out.extend_from_slice(&be32(28)); // table data starts after the 12-byte header + one 16-byte record
    out.extend_from_slice(&be32(cmap.len() as u32));
    out.extend_from_slice(&cmap);
    out
}

#[test]
fn format_4_cmap_answers_coverage() {
    let font = sfnt_with_cmap(&cmap4(&[(b'A' as u16, 5), (b'B' as u16, 6), (0x0416, 30), (0x03A9, 40)]), 1);
    assert!(covers(&font, 'A'));
    assert!(covers(&font, 'B'));
    assert!(covers(&font, 'Ж'), "Cyrillic Zhe is in the cmap");
    assert!(covers(&font, 'Ω'));
    assert!(!covers(&font, 'C'), "C has no segment");
    assert!(!covers(&font, '字'));
    assert!(!covers(&font, '\u{1F600}'), "an astral character is out of a format 4 cmap");
}

#[test]
fn format_12_cmap_answers_astral_coverage() {
    let font = sfnt_with_cmap(&cmap12(&[(0x41, 5), (0x4E2D, 9), (0x1F600, 12)]), 10);
    assert!(covers(&font, 'A'));
    assert!(covers(&font, '中'));
    assert!(covers(&font, '😀'));
    assert!(!covers(&font, 'B'));
}

#[test]
fn a_truncated_font_covers_nothing_and_never_panics() {
    let font = sfnt_with_cmap(&cmap4(&[(b'A' as u16, 5), (0x0416, 30)]), 1);
    assert!(!covers(&[], 'A'));
    for n in 0..font.len() {
        let prefix = &font[..n];
        // Every prefix parses or does not; none may panic.
        let _ = covers(prefix, 'A');
        let _ = covers(prefix, 'Ж');
        let _ = is_embeddable(prefix);
    }
    // Corrupting the cmap record count does not panic either.
    let mut bad = font.clone();
    if let Some(b) = bad.get_mut(28 + 2) {
        *b = 0xFF;
    }
    let _ = covers(&bad, 'A');
}

#[test]
fn embeddable_needs_outlines_not_just_a_cmap() {
    let font = sfnt_with_cmap(&cmap4(&[(b'A' as u16, 5)]), 1);
    assert!(!is_embeddable(&font), "a cmap-only font has no glyf");
    // A TrueType collection header is recognised, but this one points nowhere.
    let mut ttc = Vec::new();
    ttc.extend_from_slice(b"ttcf");
    ttc.extend_from_slice(&be32(0x0001_0000));
    ttc.extend_from_slice(&be32(1));
    ttc.extend_from_slice(&be32(0xFFFF_FFF0));
    assert!(!is_embeddable(&ttc));
}

#[test]
fn scripts_are_classified_for_font_selection() {
    assert_eq!(Script::of('A'), None);
    assert_eq!(Script::of('\n'), None);
    assert_eq!(Script::of('é'), Some(Script::Latin));
    assert_eq!(Script::of('Ω'), Some(Script::Greek));
    assert_eq!(Script::of('Ж'), Some(Script::Cyrillic));
    assert_eq!(Script::of('ש'), Some(Script::Hebrew));
    assert_eq!(Script::of('م'), Some(Script::Arabic));
    assert_eq!(Script::of('अ'), Some(Script::Devanagari));
    assert_eq!(Script::of('ก'), Some(Script::Thai));
    assert_eq!(Script::of('中'), Some(Script::Cjk));
    assert_eq!(Script::of('あ'), Some(Script::Cjk));
    assert_eq!(Script::of('한'), Some(Script::Hangul));
    assert_eq!(Script::of('☃'), Some(Script::Other));
    assert_eq!(Script::Cjk.name(), "CJK");
    // Every script has at least one candidate face, and none is empty.
    for s in [
        Script::Latin,
        Script::Greek,
        Script::Cyrillic,
        Script::Hebrew,
        Script::Arabic,
        Script::Devanagari,
        Script::Thai,
        Script::Cjk,
        Script::Hangul,
        Script::Other,
    ] {
        assert!(!stems(s).is_empty(), "{s:?} has no candidate face");
        assert!(
            stems(s).iter().any(|f| f.starts_with("segoeui") || f.starts_with("arial") || f.starts_with("notosans") || f.starts_with("dejavu")),
            "{s:?} has no general-purpose face"
        );
    }
}

#[test]
fn file_names_match_stems_ignoring_weight_suffixes() {
    assert!(name_matches("segoeui.ttf", "segoeui"));
    assert!(name_matches("segoeui.ttf", "Segoe UI"));
    assert!(name_matches("arialunicode.ttf", "arial unicode"));
    assert!(!name_matches("segoeuib.ttf", "segoeui.ttf"), "a literal name matches only itself");
    assert!(!name_matches("times.ttf", "arial"));

    let mut p: Vec<&Path> = vec![
        Path::new("c:/fonts/segoeuib.ttf"),
        Path::new("c:/fonts/segoeuisb.ttf"),
        Path::new("c:/fonts/segoeui.ttf"),
        Path::new("c:/fonts/arial.ttf"),
    ];
    regular_first(&mut p, "Segoe UI");
    assert_eq!(p[0], Path::new("c:/fonts/segoeui.ttf"), "the regular face is tried first");
    assert_eq!(p[1], Path::new("c:/fonts/segoeuisb.ttf"), "a plain suffix keeps rank 1");
    assert_eq!(p[2], Path::new("c:/fonts/segoeuib.ttf"), "a weight suffix ranks last among matches");
    assert_eq!(p[3], Path::new("c:/fonts/arial.ttf"), "an unrelated name sorts after every match");
}

#[test]
fn missing_files_are_an_error_not_a_panic() {
    let path = Path::new("definitely/not/a/font.ttf");
    assert!(matches!(load(path), Err(FontError::NoFont(_))));
    assert!(load_named("").is_err());
    #[cfg(target_arch = "wasm32")]
    assert!(matches!(resolve_for_char('字'), Err(FontError::Unsupported)));
}
