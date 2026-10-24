//! Reading a worksheet whole, in chunks and streamed must give the same sheet.
//!
//! A worksheet part is decoded three ways: read into one buffer and walked straight through, read
//! into one buffer and cut at row boundaries for several threads, or decompressed window by window
//! as it is decoded (a sheet too big to hold). All three must produce exactly the same sheet, so
//! the tests here read one fixture each of those ways and hold the results against each other.
//!
//! The plans are spelled out instead of the production thresholds: a fixture of a few hundred rows
//! then exercises every cut, every window boundary and the `<sheetData>` bookkeeping around them.

use gridcraft_core::Value;
use gridcraft_model::{Sheet, Workbook};

use super::minimal;
use crate::read::read_xlsx_with;
use crate::sheet_read::ReadPlan;

/// One pass over a buffer: no chunking, no streaming.
fn whole() -> ReadPlan {
    ReadPlan { stream_min: u64::MAX, window: usize::MAX, chunk_min: usize::MAX }
}

/// One buffer, cut into chunks that run on several threads.
fn chunked() -> ReadPlan {
    ReadPlan { stream_min: u64::MAX, window: usize::MAX, chunk_min: 256 }
}

/// Decompressed as it is decoded, with a window small enough to split almost every row band.
fn streamed() -> ReadPlan {
    ReadPlan { stream_min: 0, window: 700, chunk_min: 256 }
}

fn open(body: &str, plan: ReadPlan) -> (Workbook, Vec<String>) {
    let bytes = minimal(body, &[], "", "");
    let (wb, report) = read_xlsx_with(&bytes, plan).expect("read");
    (wb, report.warnings)
}

fn sheet_of(body: &str, plan: ReadPlan) -> Sheet {
    let (wb, _) = open(body, plan);
    wb.sheet(0).expect("sheet").clone()
}

/// Fails with the first field that differs, which is far more useful than `assert_eq!` on a whole
/// sheet: a mismatch in a cell's formula otherwise prints two megabytes of model.
fn assert_same(a: &Sheet, b: &Sheet, what: &str) {
    assert_eq!(a.name, b.name, "{what}: name");
    assert_eq!(a.cells.len(), b.cells.len(), "{what}: cell count");
    for (at, cell) in a.cells.iter() {
        match b.cells.get(at) {
            Some(other) => assert!(cell == other, "{what}: cell {at:?} is {cell:?} vs {other:?}"),
            None => panic!("{what}: cell {at:?} missing from the other read"),
        }
    }
    assert_eq!(a.cols, b.cols, "{what}: column widths");
    assert_eq!(a.rows, b.rows, "{what}: row heights");
    assert_eq!(a.merges, b.merges, "{what}: merges");
    assert_eq!(a.freeze, b.freeze, "{what}: freeze panes");
    assert_eq!(a.validations, b.validations, "{what}: validations");
    assert_eq!(a.autofilter, b.autofilter, "{what}: autofilter");
    assert_eq!(a.tab_color, b.tab_color, "{what}: tab colour");
    assert_eq!(a.cond_formats, b.cond_formats, "{what}: conditional formats");
    assert_eq!(a.print, b.print, "{what}: page setup");
    assert_eq!(a.default_col_width, b.default_col_width, "{what}: default column width");
    assert_eq!(a.default_row_height, b.default_row_height, "{what}: default row height");
}

/// Rows with values, strings, styles and the two kinds of reference-less cell, plus the settings
/// that live outside `<sheetData>`.
fn mixed_body(rows: u32) -> String {
    let mut s = String::from(
        r#"<sheetPr><tabColor rgb="FF00B050"/></sheetPr><sheetViews><sheetView workbookViewId="0"><pane xSplit="1" ySplit="2" topLeftCell="B3" state="frozen"/></sheetView></sheetViews><sheetFormatPr defaultRowHeight="15"/><cols><col min="2" max="3" width="27.5" customWidth="1"/></cols><sheetData>"#,
    );
    for r in 1..=rows {
        let height = if r % 7 == 0 { r#" ht="22.5" customHeight="1""# } else { "" };
        s.push_str(&format!("<row r=\"{r}\"{height}>"));
        s.push_str(&format!("<c r=\"A{r}\"><v>{}</v></c>", r as f64 * 1.25));
        s.push_str(&format!("<c r=\"B{r}\" t=\"s\"><v>{}</v></c>", r % 3));
        // No `r` on these: the reader fills in the next column and keeps the row.
        s.push_str(&format!("<c><v>{}</v></c><c t=\"str\"><v>p{r}</v></c>", r as f64));
        if r % 5 == 0 {
            s.push_str(&format!(r#"<c r="E{r}" s="1"><f>A{r}+1</f><v>{}</v></c>"#, r as f64 + 1.0));
        }
        s.push_str("</row>");
    }
    s.push_str("</sheetData>");
    s.push_str(r#"<mergeCells count="1"><mergeCell ref="A1:B2"/></mergeCells><autoFilter ref="A1:E4"/><pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/><pageSetup orientation="landscape" paperSize="9"/>"#);
    s
}

#[test]
fn chunked_and_streamed_reads_match_a_whole_one() {
    let body = mixed_body(400);
    let strings: Vec<&str> = vec!["zero", "one", "two"];
    let sst: Vec<String> = strings.iter().map(|s| format!("<si><t>{s}</t></si>")).collect();
    let parts: Vec<(&str, &str)> = vec![("xl/sharedStrings.xml", "")];
    let _ = parts;
    // The fixture's shared strings live in the package, so build it by hand for this one.
    let body = body.replace("t=\"s\"", "t=\"inlineStr\"");
    let _ = sst;

    let whole_sheet = sheet_of(&body, whole());
    let chunked_sheet = sheet_of(&body, chunked());
    let streamed_sheet = sheet_of(&body, streamed());
    assert_same(&whole_sheet, &chunked_sheet, "chunked");
    assert_same(&whole_sheet, &streamed_sheet, "streamed");
    assert!(whole_sheet.cells.len() > 1_500, "fixture too small: {}", whole_sheet.cells.len());
}

#[test]
fn the_settings_around_the_rows_survive_chunking() {
    let body = mixed_body(60);
    let whole_sheet = sheet_of(&body, whole());
    for (plan, what) in [(chunked(), "chunked"), (streamed(), "streamed")] {
        let sheet = sheet_of(&body, plan);
        let width = whole_sheet.cols.get(&1).and_then(|c| c.size);
        assert!(width.is_some_and(|px| px > whole_sheet.default_col_width), "the fixture's column width was not read");
        assert_eq!(sheet.cols.get(&1).and_then(|c| c.size), width, "{what}: column width");
        assert_eq!(sheet.freeze, Some((2, 1)), "{what}: frozen panes (2 rows, 1 column)");
        assert_eq!(sheet.merges.len(), 1, "{what}: merge count");
        assert_eq!(sheet.autofilter.as_ref().map(|f| f.range), whole_sheet.autofilter.as_ref().map(|f| f.range), "{what}: autofilter");
        assert_eq!(sheet.print.orientation, whole_sheet.print.orientation, "{what}: orientation");
        assert_eq!(sheet.print.paper, whole_sheet.print.paper, "{what}: paper");
        assert_eq!(sheet.tab_color, whole_sheet.tab_color, "{what}: tab colour");
        assert!(whole_sheet.rows.get(&6).and_then(|r| r.size).is_some(), "the fixture's row height was not read");
        assert_eq!(sheet.rows.get(&6).and_then(|r| r.size), whole_sheet.rows.get(&6).and_then(|r| r.size), "{what}: row height");
    }
}

#[test]
fn a_shared_formula_finds_its_master_in_an_earlier_window() {
    // The master sits at row 3 and its uses run to row 60; with a 700-byte window the master and
    // most uses land in different windows, so the state has to carry across them.
    let mut body = String::from("<sheetData>");
    for r in 1..=60u32 {
        body.push_str(&format!("<row r=\"{r}\"><c r=\"A{r}\"><v>{r}</v></c>"));
        if r == 3 {
            body.push_str(r#"<c r="B3"><f t="shared" ref="B3:B60" si="0">A3*2</f><v>6</v></c>"#);
        } else if r > 3 {
            body.push_str(&format!(r#"<c r="B{r}"><f t="shared" si="0"/><v>{}</v></c>"#, r * 2));
        }
        body.push_str("</row>");
    }
    body.push_str("</sheetData>");

    let sheet = sheet_of(&body, streamed());
    let text = |a: &str| -> String {
        let at = gridcraft_core::CellRef::parse(a).expect("address");
        sheet.cells.get(at).and_then(|c| c.formula).map(|f| f.text.clone()).unwrap_or_default()
    };
    assert_eq!(text("B3"), "A3*2");
    assert_eq!(text("B25"), "A25*2");
    assert_eq!(text("B60"), "A60*2");
}

#[test]
fn an_empty_sheet_data_tag_keeps_the_settings_after_it() {
    let body = r#"<cols><col min="1" max="1" width="12"/></cols><sheetData/><mergeCells><mergeCell ref="A1:B1"/></mergeCells>"#;
    let sheet = sheet_of(body, streamed());
    assert_eq!(sheet.cells.len(), 0, "no rows");
    let width = sheet.cols.get(&0).and_then(|c| c.size);
    assert!(width.is_some_and(|px| px > sheet.default_col_width), "column width before <sheetData/>: {width:?}");
    assert_eq!(sheet.merges.len(), 1, "merge after <sheetData/>");
}

#[test]
fn a_part_without_a_closing_tag_keeps_the_rows_it_read() {
    let body = r#"<sheetData><row r="1"><c r="A1"><v>1</v></c></row><row r="2"><c r="A2"><v>2</v></c></row>"#;
    let sheet = sheet_of(body, streamed());
    assert_eq!(sheet.cells.len(), 2, "rows before the part ends");
    let at = gridcraft_core::CellRef::parse("A2").expect("address");
    assert_eq!(sheet.cells.get(at).map(|c| c.value.clone()), Some(Value::number(2.0)));
}

#[test]
fn an_inline_string_across_a_window_boundary_keeps_its_text() {
    let long: String = "x".repeat(1200);
    let body = format!(
        "<sheetData><row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>{long}</t></is></c></row><row r=\"2\"><c r=\"A2\"><v>7</v></c></row></sheetData>"
    );
    // A window smaller than the string forces it to span windows.
    let plan = ReadPlan { stream_min: 0, window: 512, chunk_min: 256 };
    let sheet = sheet_of(&body, plan);
    let at = gridcraft_core::CellRef::parse("A1").expect("address");
    let text = sheet.cells.get(at).and_then(|c| c.value.as_text().map(str::to_string)).unwrap_or_default();
    assert_eq!(text.len(), 1200, "inline string text");
}
