//! A filter closes the rows it hides, on screen too: the row it keeps sits under the header, not
//! a thousand points below it. The grid places rows through the line index, which used to read
//! the hidden state off one line info per row; with the filter bitmap it must ask the bitmap
//! (a filtered million-row sheet drew blank before this).

use egui::{Rect, pos2, vec2};
use gridcraft_engine::Session;
use gridcraft_engine::core::CellRef;
use gridcraft_ui_egui::grid;
use serde_json::json;

#[test]
fn a_filtered_row_sits_under_the_header() {
    let mut s = Session::new();
    s.new_workbook();
    let mut values = vec![vec![json!("Name")]];
    values.extend((0..600).map(|i| vec![json!(if i == 500 { "KEPT" } else { "gone" })]));
    s.execute("range.setValues", json!({"range": "A1", "values": values})).unwrap();
    s.execute("data.filter", json!({"range": "A1:A601"})).unwrap();
    s.execute("data.filterBy", json!({"column": "A", "values": ["KEPT"]})).unwrap();
    let sh = s.doc().unwrap().wb.active().unwrap().clone();
    // The rows the filter hides close up: the header, then the one row it keeps.
    let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0));
    let geo = grid::Geo::new(&sh, rect, vec2(0.0, 0.0));
    let kept = geo.cell_rect(&sh, CellRef::parse("A501").unwrap()).top();
    assert!(kept < 100.0, "the row the filter keeps is drawn at {kept}, under 499 hidden rows");
    // Every other data row stays hidden, so it keeps its place in the sheet.
    assert!(geo.cell_rect(&sh, CellRef::parse("A601").unwrap()).top() < 200.0, "the sheet still ends one row below");
}
