//! Shape-and-place tests: glyph order, `TJ`/`Ts` placement, cluster extraction and the
//! byte-identical fast path.

use std::sync::Arc;

use gridcraft_core::text::{FontSize, ShapedText, TextEngine};

use super::*;

/// Ubuntu Light, compiled into epaint: a real TrueType face every machine has, with an `fi`
/// ligature. No font binary is committed here (AGENTS.md, craftrules `standards/fonts.md`).
fn ubuntu() -> Face {
    Face::parse(epaint_default_fonts::UBUNTU_LIGHT.to_vec()).expect("Ubuntu Light parses")
}

/// An embeddable face for `script` from the platform's fonts (or `CRAFT_FONTS_DIR`), if this
/// machine has one. Script tests skip with a message when it does not, like the core text tests.
fn script_face(script: gridcraft_core::fonts::Script) -> Option<Face> {
    let font = gridcraft_core::fonts::resolve_for_script(script).ok()?;
    Face::parse(font.data).ok()
}

macro_rules! script_face {
    ($script:expr) => {
        match script_face($script) {
            Some(face) => face,
            None => {
                eprintln!("skipped: no embeddable {} face (set CRAFT_FONTS_DIR to test it)", $script.name());
                return;
            }
        }
    };
}

/// The shaped form of `text`, with the same engine and size the writer uses.
fn shape_with(face: &Face, text: &str, direction: Direction, size: f32) -> Arc<ShapedText> {
    let mut engine = TextEngine::new();
    let id = engine.add_face(face.bytes()).expect("the face registers");
    engine.shape(text, direction, id, FontSize::em(size)).expect("the text shapes")
}

/// Every CID shown in a content stream, in order.
fn shown_cids(content: &str) -> Vec<u16> {
    let mut out = Vec::new();
    for token in content.split_whitespace() {
        let token = token.trim_matches(|c| c == '[' || c == ']');
        if token.len() < 3 || !token.starts_with('<') || !token.ends_with('>') {
            continue;
        }
        let hex = &token[1..token.len() - 1];
        if hex.len() % 4 != 0 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        for chunk in hex.as_bytes().chunks(4) {
            out.push(u16::from_str_radix(std::str::from_utf8(chunk).expect("ASCII hex"), 16).expect("four hex digits"));
        }
    }
    out
}

/// The pen positions of the shown glyphs, computed the way a PDF viewer does: a `TJ` number moves
/// the pen by `-number/1000 * size` points, and a glyph advances by its `/W` width.
fn placement(content: &str, size: f32, widths: &[u16]) -> Vec<(u16, f32, f32)> {
    let mut out = Vec::new();
    let mut x = 0.0f32;
    let mut rise = 0.0f32;
    for line in content.lines() {
        if line.ends_with(" Tm") {
            // The tests compare against the shaper's pen, which starts at the text origin.
            x = 0.0;
            continue;
        }
        if line.ends_with(" Ts") {
            if let Some(value) = line.split_whitespace().next().and_then(|t| t.parse::<f32>().ok()) {
                rise = value;
            }
            continue;
        }
        if !(line.ends_with(" Tj") || line.ends_with(" TJ")) {
            continue;
        }
        let body = line.trim_end_matches(" Tj").trim_end_matches(" TJ").trim_matches(|c| c == '[' || c == ']');
        for token in body.split_whitespace() {
            if token.starts_with('<') && token.ends_with('>') {
                let hex = &token[1..token.len() - 1];
                for chunk in hex.as_bytes().chunks(4) {
                    let cid = u16::from_str_radix(std::str::from_utf8(chunk).expect("ASCII hex"), 16).expect("four hex digits");
                    out.push((cid, x, rise));
                    x += f32::from(*widths.get(cid as usize).unwrap_or(&0)) * size / 1000.0;
                }
            } else if let Ok(number) = token.parse::<f32>() {
                x -= number * size / 1000.0;
            }
        }
    }
    out
}

/// The width table the content stream's `/W` array would hold, indexable by CID.
fn width_table(font: &FontUse) -> Vec<u16> {
    std::iter::once(font.width_1000(0)).chain(font.cid_width.iter().copied()).collect()
}

/// The source glyph and pen position the shaper asks for, in visual order.
fn shaper_positions(shaped: &ShapedText) -> Vec<(u16, f32, f32)> {
    let mut out = Vec::new();
    let mut pen = 0.0f32;
    for run in &shaped.runs {
        for glyph in &run.glyphs {
            out.push((glyph.id, pen + glyph.x_offset, glyph.y_offset));
            pen += glyph.advance;
        }
    }
    out
}

/// Draws `text` at 20 pt and returns the document and the page's content stream.
fn shaped_page(face: &Face, text: &str, direction: Direction) -> (PdfDoc, String) {
    let mut doc = PdfDoc::new();
    doc.set_font(face.clone());
    let content = {
        let page = doc.add_page(500.0, 120.0);
        page.text_dir(20.0, 60.0, 20.0, Font::Helvetica, [0, 0, 0], text, direction);
        page.content.clone()
    };
    (doc, content)
}

#[test]
fn plain_text_is_the_byte_identical_fast_path() {
    // WinAnsi text with no mark, bidi control or right-to-left base: a standard font has no
    // layout features, so shaping it could not change a glyph.
    assert!(plain_ltr("Invoice (draft) €5 — café", Direction::Auto));
    assert!(plain_ltr("Hello, world", Direction::Ltr));
    assert!(plain_ltr("", Direction::Ltr));
    assert!(plain_ltr("fi", Direction::Auto), "the fast path keeps fi two characters");

    // Every character outside WinAnsi, every combining mark and every bidi control shapes, and so
    // does any right-to-left base (it mirrors brackets and reverses neutral runs).
    assert!(!plain_ltr("Ж", Direction::Ltr));
    assert!(!plain_ltr("हिन्दी", Direction::Ltr));
    assert!(!plain_ltr("e\u{0301}", Direction::Ltr));
    assert!(!plain_ltr("abc\u{202E}123", Direction::Ltr));
    assert!(!plain_ltr("שלום", Direction::Ltr));
    assert!(!plain_ltr("Hello", Direction::Rtl));

    // Ubuntu Light ligates "fi", but a WinAnsi-only document must stay byte-identical: the
    // predicate, not the font, decides.
    let text = "Invoice fi (draft)";
    let plain = {
        let mut doc = PdfDoc::new();
        doc.add_page(300.0, 100.0).text(10.0, 40.0, 12.0, Font::Helvetica, [0, 0, 0], text);
        doc.finish()
    };
    let with_font = {
        let mut doc = PdfDoc::new();
        doc.set_font(ubuntu());
        doc.add_page(300.0, 100.0).text(10.0, 40.0, 12.0, Font::Helvetica, [0, 0, 0], text);
        doc.finish()
    };
    assert_eq!(plain, with_font);
    assert!(find(&with_font, br"(Invoice fi \(draft\)) Tj").is_some(), "stored order, no ligature");
}

#[test]
fn a_ligature_extracts_as_all_of_its_characters() {
    let face = ubuntu();
    let text = "Āfi"; // Ā is outside WinAnsi, so the whole item is shaped and "fi" ligates.
    let shaped = shape_with(&face, text, Direction::Ltr, 20.0);
    assert_eq!(shaped.glyph_count(), 2, "Ā plus the fi ligature");
    let ligature = shaped.glyphs().nth(1).expect("the ligature");
    assert_eq!(ligature.cluster(), 2..4, "the ligature still covers both characters");
    assert_ne!(ligature.id, face.glyph('f'));

    let (doc, content) = shaped_page(&face, text, Direction::Ltr);
    let cids = shown_cids(&content);
    assert_eq!(cids.len(), 2);
    {
        let font = doc.font.borrow();
        let font = font.as_ref().expect("the face was set");
        assert_eq!(font.cid_text[cids[0] as usize - 1].as_deref(), Some("Ā"));
        assert_eq!(font.cid_text[cids[1] as usize - 1].as_deref(), Some("fi"), "the ligature carries both characters");
        assert_eq!(font.cid_src[cids[1] as usize - 1], ligature.id);
    }
    let pdf = doc.finish();
    check_xref(&pdf);
    assert!(find(&pdf, b"<00660069>").is_some(), "ToUnicode maps the ligature to f + i");
}

#[test]
fn a_composed_accent_extracts_as_both_characters() {
    let face = ubuntu();
    let text = "e\u{0301}"; // e + COMBINING ACUTE; Ubuntu composes it to one glyph.
    let shaped = shape_with(&face, text, Direction::Ltr, 20.0);
    assert_eq!(shaped.glyph_count(), 1);
    let (doc, content) = shaped_page(&face, text, Direction::Ltr);
    assert!(!content.contains("(e?)"), "the unshaped path would have dropped the mark");
    assert_eq!(shown_cids(&content).len(), 1);
    let pdf = doc.finish();
    assert!(find(&pdf, b"<00650301>").is_some(), "the glyph extracts as e + U+0301");
}

#[test]
fn an_rtl_run_is_drawn_in_visual_order() {
    let face = script_face!(gridcraft_core::fonts::Script::Arabic);
    let text = "مرحبا";
    let shaped = shape_with(&face, text, Direction::Auto, 20.0);
    assert_eq!(shaped.runs.len(), 1);
    assert!(shaped.base_direction.is_rtl());

    let (doc, content) = shaped_page(&face, text, Direction::Auto);
    let cids = shown_cids(&content);
    {
        let font = doc.font.borrow();
        let font = font.as_ref().expect("the face was set");
        let drawn: Vec<u16> = cids.iter().map(|cid| font.cid_src[*cid as usize - 1]).collect();
        let expected: Vec<u16> = shaped.glyphs().map(|g| g.id).collect();
        assert_eq!(drawn, expected, "the content stream follows the shaper's visual order");
        // Joining changed the glyphs: no letter is drawn in its isolated cmap form.
        let nominal: Vec<u16> = text.chars().map(|c| face.glyph(c)).collect();
        assert_ne!(drawn, nominal, "the letters must take contextual forms");
        // Read in drawn (visual) order the extraction text is the word reversed; a bidi-aware
        // extractor turns that back into the logical word (checked with pdftotext in the example).
        let extracted: String = cids.iter().filter_map(|cid| font.cid_text[*cid as usize - 1].as_deref()).collect();
        let logical: String = text.chars().rev().collect();
        assert_eq!(extracted, logical);
    }
    assert_eq!(content.matches(" Tm").count(), 1, "one text matrix; the pen moves with TJ/Ts");
    assert!(!content.contains(" Td"));
}

#[test]
fn a_right_to_left_base_direction_mirrors_neutral_text() {
    let face = script_face!(gridcraft_core::fonts::Script::Arabic);
    let (doc, content) = shaped_page(&face, "(", Direction::Rtl);
    let cids = shown_cids(&content);
    let font = doc.font.borrow();
    let font = font.as_ref().expect("the face was set");
    let drawn = font.cid_src[cids[0] as usize - 1];
    assert_ne!(drawn, face.glyph('('));
    assert_eq!(drawn, face.glyph(')'), "an opening bracket is mirrored in a right-to-left paragraph");
}

#[test]
fn a_multi_glyph_cluster_extracts_once_and_spans_its_whole_advance() {
    let face = script_face!(gridcraft_core::fonts::Script::Devanagari);
    let text = "नि";
    let size = 20.0;
    let shaped = shape_with(&face, text, Direction::Ltr, size);
    assert_eq!(shaped.glyph_count(), 2, "the i-matra is drawn before its base");
    assert_eq!(shaped.glyphs().next().expect("the matra").id, face.glyph('ि'), "the matra comes first visually");

    let (doc, content) = shaped_page(&face, text, Direction::Ltr);
    let cids = shown_cids(&content);
    {
        let font = doc.font.borrow();
        let font = font.as_ref().expect("the face was set");
        // Exactly one CID carries the cluster's text, and it is the leftmost glyph.
        assert_eq!(font.cid_text[cids[0] as usize - 1].as_deref(), Some("नि"));
        assert!(font.cid_text[cids[1] as usize - 1].is_none(), "the base must not repeat the text");
        // The carrier advances over the whole cluster; the component stands still. An extractor
        // that saw a narrow carrier followed by an untexted glyph would report a word space.
        let cluster: f32 = shaped.glyphs().map(|g| g.advance).sum();
        assert!((f32::from(font.cid_width[cids[0] as usize - 1]) - cluster * 1000.0 / size).abs() < 1.0);
        assert_eq!(font.cid_width[cids[1] as usize - 1], 0);
    }
    // The positions still match the shaper's pen.
    {
        let borrow = doc.font.borrow();
        let font = borrow.as_ref().expect("the face was set");
        let widths = width_table(font);
        let drawn = placement(&content, size, &widths);
        let expected = shaper_positions(&shaped);
        assert_eq!(drawn.len(), expected.len());
        for ((cid, x, rise), (gid, want_x, want_rise)) in drawn.iter().zip(expected) {
            assert_eq!(font.cid_src[*cid as usize - 1], gid);
            assert!((x - want_x).abs() < 0.02, "x {x} vs {want_x}");
            assert!((rise - want_rise).abs() < 0.01);
        }
    }
    let pdf = doc.finish();
    check_xref(&pdf);
}

#[test]
fn a_glyph_that_extracts_differently_gets_its_own_cid() {
    let face = script_face!(gridcraft_core::fonts::Script::Devanagari);
    // The base न stands alone (carrier, full advance) and again inside नि (component, no advance).
    let text = "न नि";
    let (doc, content) = shaped_page(&face, text, Direction::Ltr);
    let cids = shown_cids(&content);
    {
        let font = doc.font.borrow();
        let font = font.as_ref().expect("the face was set");
        let mut by_source: std::collections::BTreeMap<u16, usize> = std::collections::BTreeMap::new();
        for cid in &cids {
            *by_source.entry(font.cid_src[*cid as usize - 1]).or_default() += 1;
        }
        assert!(by_source.values().any(|n| *n > 1), "one source glyph is two CIDs: {by_source:?}");
        let texts: Vec<Option<&str>> = cids.iter().map(|cid| font.cid_text[*cid as usize - 1].as_deref()).collect();
        assert!(texts.contains(&Some("न")) && texts.contains(&Some("नि")), "{texts:?}");
    }
    let pdf = doc.finish();
    check_xref(&pdf);
    assert!(find(&pdf, b"/CIDToGIDMap /Identity").is_none(), "duplicated CIDs need a map stream");
    assert!(find(&pdf, b"/CIDToGIDMap ").is_some());
}

#[test]
fn a_thai_mark_is_raised_without_advancing() {
    let face = script_face!(gridcraft_core::fonts::Script::Thai);
    let text = "ก่";
    let size = 20.0;
    let shaped = shape_with(&face, text, Direction::Ltr, size);
    assert_eq!(shaped.glyph_count(), 2);
    let base = shaped.glyphs().next().expect("the base");
    let mark = shaped.glyphs().nth(1).expect("the tone mark");
    assert!(base.advance > 0.0);
    assert_eq!(mark.advance, 0.0, "the mark is positioned, not advanced");

    let (doc, content) = shaped_page(&face, text, Direction::Ltr);
    let cids = shown_cids(&content);
    {
        let font = doc.font.borrow();
        let font = font.as_ref().expect("the face was set");
        assert_eq!(font.cid_text[cids[0] as usize - 1].as_deref(), Some("ก่"));
        assert!(font.cid_text[cids[1] as usize - 1].is_none());
        assert_eq!(font.cid_width[cids[1] as usize - 1], 0, "a mark adds no width");
        if mark.y_offset != 0.0 {
            assert!(content.contains(" Ts"), "a raised mark uses Ts, not a second text matrix");
        }
        // The mark's rise and every glyph's pen position match the shaper, including the sign.
        let widths = width_table(font);
        let drawn = placement(&content, size, &widths);
        let expected = shaper_positions(&shaped);
        assert_eq!(drawn.len(), expected.len());
        for ((cid, x, rise), (gid, want_x, want_rise)) in drawn.iter().zip(expected) {
            assert_eq!(font.cid_src[*cid as usize - 1], gid);
            assert!((x - want_x).abs() < 0.02, "x {x} vs {want_x}");
            assert!((rise - want_rise).abs() < 0.01, "rise {rise} vs {want_rise}");
        }
    }
    assert_eq!(content.matches(" Tm").count(), 1);
    let pdf = doc.finish();
    check_xref(&pdf);
}

#[test]
fn a_string_over_the_shape_limit_keeps_the_stored_order_path() {
    let mut font = FontUse::new(ubuntu());
    assert!(font.shape("Ж", Direction::Ltr, 12.0).is_some());
    let over = "Ж".repeat(gridcraft_core::text::MAX_TEXT_BYTES / 2 + 1);
    assert!(over.len() > gridcraft_core::text::MAX_TEXT_BYTES);
    assert!(font.shape(&over, Direction::Ltr, 12.0).is_none(), "over the engine limit: fall back");
}

#[test]
fn shaped_text_survives_hostile_inputs() {
    let mut doc = PdfDoc::new();
    doc.set_font(ubuntu());
    let page = doc.add_page(300.0, 300.0);
    for &a in &[f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -7.0, 0.0, 1.0e30, -1.0e30] {
        page.text_dir(a, a, a, Font::Helvetica, [0, 0, 0], "Жe\u{0301}---", Direction::Rtl);
        page.text_rotated_dir(a, a, 10.0, Font::TimesRoman, [0, 0, 0], "Ж", a, Direction::Auto);
        page.text_dir(10.0, 20.0, 10.0, Font::Helvetica, [0, 0, 0], "\u{202E}abc\u{202C}", Direction::Rtl);
    }
    let pdf = doc.finish();
    check_xref(&pdf);
}
