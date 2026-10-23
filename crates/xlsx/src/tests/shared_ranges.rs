//! Shared formulas, array formulas and their ranges across many rows.
//!
//! These are the only rules that make reading a worksheet order-dependent: a `<f t="shared" si="0"/>`
//! repeats the master's formula shifted by its own offset, a master can sit far above its uses, and
//! cells inside an array range keep their cached value only at the anchor. Any change that reads a
//! sheet in more than one piece (parallel row chunks, streaming) must reproduce all of it exactly,
//! so this file pins the behaviour down.

use gridcraft_core::{CellRef, Value};
use gridcraft_model::Workbook;

use crate::{read_xlsx, write_xlsx};

fn at(s: &str) -> CellRef {
    CellRef::parse(s).expect("address")
}

/// A sheet with two shared-formula groups (one with a quoted literal), a legacy array formula over
/// two rows and a dynamic one, spread over rows that cross the 64-row cell-store band size.
fn body() -> String {
    let mut s = String::from("<sheetData>");
    for r in 1..=300u32 {
        s.push_str(&format!("<row r=\"{r}\"><c r=\"A{r}\"><v>{}</v></c>", r as f64 * 1.5));
        // Shared group 1: master at B10, uses up to B120.
        if r == 10 {
            s.push_str(&format!(r#"<c r="B{r}"><f t="shared" ref="B10:B120" si="0">A{r}*2</f><v>{}</v></c>"#, r as f64 * 3.0));
        } else if (10..=120).contains(&r) {
            s.push_str(&format!(r#"<c r="B{r}"><f t="shared" si="0"/><v>{}</v></c>"#, r as f64 * 3.0));
        }
        // Shared group 2: master at C30, a literal with quotes in the formula.
        if r == 30 {
            s.push_str(&format!(r#"<c r="C{r}" t="str"><f t="shared" ref="C30:C60" si="1">IF(A{r}&gt;1,"big","small")</f><v>big</v></c>"#));
        } else if (30..=60).contains(&r) {
            s.push_str(&format!(r#"<c r="C{r}" t="str"><f t="shared" si="1"/><v>{}</v></c>"#, if r > 1 { "big" } else { "small" }));
        }
        // Legacy array formula over two rows: only the anchor keeps a value.
        if r == 20 {
            s.push_str(r#"<c r="D20"><f t="array" ref="D20:E21">A20:A21+1</f><v>2.5</v></c>"#);
            s.push_str(r#"<c r="E20"><v>3.5</v></c>"#);
        }
        if r == 21 {
            s.push_str(r#"<c r="D21"><v>4.5</v></c><c r="E21"><v>5.5</v></c>"#);
        }
        // Dynamic array: `cm` marks it, so the spill range must not be filled in from the file.
        if r == 20 {
            s.push_str(r#"<c r="F20" cm="1"><f t="array" ref="F20:F22">SEQUENCE(3)</f><v>1</v></c>"#);
        }
        if r == 21 || r == 22 {
            s.push_str(&format!(r#"<c r="F{r}" cm="1"><v>{}</v></c>"#, r - 19));
        }
        s.push_str("</row>");
    }
    s.push_str("</sheetData>");
    s
}

fn open() -> Workbook {
    let bytes = super::minimal(&body(), &[], "", "");
    let (wb, report) = read_xlsx(&bytes).expect("read");
    for w in &report.warnings {
        assert!(!w.contains("shared formula") && !w.contains("damaged") && !w.contains("array"), "{w}");
    }
    wb
}

#[test]
fn shared_formula_uses_follow_their_master_across_bands() {
    let wb = open();
    let sheet = wb.sheet(0).expect("sheet");
    let text = |a: &str| -> String {
        sheet.cell(at(a)).and_then(|c| c.formula.as_ref()).map(|f| f.text.clone()).unwrap_or_else(|| panic!("no formula at {a}"))
    };
    assert_eq!(text("B10"), "A10*2");
    // Shifted by the row offset, not copied verbatim.
    assert_eq!(text("B11"), "A11*2");
    assert_eq!(text("B119"), "A119*2");
    assert_eq!(text("B120"), "A120*2");
    // Outside the group there is no formula.
    assert!(sheet.cell(at("B9")).is_none());
    assert!(sheet.cell(at("B121")).is_none());

    // A literal with quotes survives the master and the shifted uses.
    assert_eq!(text("C30"), r#"IF(A30>1,"big","small")"#);
    assert_eq!(text("C31"), r#"IF(A31>1,"big","small")"#);
    assert_eq!(text("C60"), r#"IF(A60>1,"big","small")"#);
}

#[test]
fn array_ranges_keep_only_the_anchor() {
    let wb = open();
    let sheet = wb.sheet(0).expect("sheet");

    let d20 = sheet.cell(at("D20")).expect("D20");
    let f = d20.formula.as_ref().expect("formula");
    assert_eq!(f.text, "A20:A21+1");
    assert_eq!(f.array.map(|r| (r.start, r.end)), Some((at("D20"), at("E21"))));
    assert_eq!(d20.value, Value::Number(2.5));
    // Cached values inside a legacy array range are not kept: the formula fills them again.
    assert!(sheet.cell(at("E20")).is_none(), "spill cell E20");
    assert!(sheet.cell(at("D21")).is_none(), "spill cell D21");
    assert!(sheet.cell(at("E21")).is_none(), "spill cell E21");

    let f20 = sheet.cell(at("F20")).expect("F20");
    let f = f20.formula.as_ref().expect("formula");
    assert_eq!(f.text, "SEQUENCE(3)");
    // A dynamic array keeps its metadata (no legacy `array` range) and drops the spill cache.
    assert_eq!(f.array, None);
    assert!(f.dynamic);
    assert!(sheet.cell(at("F21")).is_none(), "spill cell F21");
    assert!(sheet.cell(at("F22")).is_none(), "spill cell F22");
}

#[test]
fn shared_and_array_formulas_round_trip() {
    let wb = open();
    let saved = write_xlsx(&wb).expect("write");
    let (back, report) = read_xlsx(&saved).expect("read back");
    for w in &report.warnings {
        assert!(!w.contains("shared formula") && !w.contains("damaged") && !w.contains("array"), "{w}");
    }

    let (a, b) = (wb.sheet(0).expect("a"), back.sheet(0).expect("b"));
    assert_eq!(a.cells.len(), b.cells.len(), "cell count");
    for (at_, cell) in a.cells.iter() {
        let got = b.cell(at_).unwrap_or_else(|| panic!("missing {at_}"));
        assert_eq!(got.value, cell.value, "value at {at_}");
        let f = |c: &gridcraft_model::Cell| c.formula.as_ref().map(|f| (f.text.clone(), f.array, f.dynamic));
        assert_eq!(f(got), f(cell), "formula at {at_}");
    }
}
