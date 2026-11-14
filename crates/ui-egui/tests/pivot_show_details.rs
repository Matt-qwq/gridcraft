//! Double-clicking a PivotTable value is Excel's Show Details: the source rows behind that number
//! land in a sheet of their own. A label cell keeps the ordinary double-click behaviour.

use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use gridcraft_engine::Session;
use gridcraft_engine::core::{CellRef, Value};
use gridcraft_ui_egui::{SheetApp, grid::Geo};
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

fn harness() -> Harness {
    let mut session = Session::new();
    session.new_workbook();
    session
        .execute("range.setValues", json!({"range": "A1", "values": [["Region", "Sales"], ["East", 10], ["West", 20], ["East", 30], ["North", 40]]}))
        .unwrap();
    session
        .execute(
            "insert.pivotTable",
            json!({"source": "Sheet1!$A$1:$B$5", "destination": "Sheet1!D2", "name": "Report", "rows": ["Region"], "values": ["Sales"]}),
        )
        .unwrap();
    let mut h = egui_kittest::Harness::builder().with_step_dt(1.0 / 60.0).with_size(vec2(1200.0, 800.0)).build_ui_state(
        |ui, app: &mut SheetApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        SheetApp::new(session, Default::default()),
    );
    h.run_steps(4);
    h
}

/// The report's area, from the pivot that wrote it.
fn report(h: &Harness) -> gridcraft_engine::core::RangeRef {
    let app = h.state();
    let sh = app.session.active().expect("a document").wb.active().expect("a sheet");
    sh.pivots.first().and_then(|p| p.last_range).expect("the pivot wrote a report")
}

/// The first cell of the report that holds a number: a value cell, not a label.
fn value_cell(h: &Harness) -> CellRef {
    let app = h.state();
    let sh = app.session.active().expect("a document").wb.active().expect("a sheet");
    let range = report(h);
    for row in range.start.row..=range.end.row {
        for col in range.start.col..=range.end.col {
            let at = CellRef::new(row, col);
            if matches!(sh.value(at), Value::Number(_)) {
                return at;
            }
        }
    }
    panic!("the report holds no value cell");
}

fn label_cell(h: &Harness) -> CellRef {
    let app = h.state();
    let sh = app.session.active().expect("a document").wb.active().expect("a sheet");
    let range = report(h);
    for row in range.start.row..=range.end.row {
        for col in range.start.col..=range.end.col {
            let at = CellRef::new(row, col);
            if matches!(sh.value(at), Value::Text(t) if t.as_ref() == "East") {
                return at;
            }
        }
    }
    panic!("the report holds no East label");
}

fn centre(h: &Harness, at: CellRef) -> Pos2 {
    let app = h.state();
    let sheet = app.session.active().expect("a document").wb.active().expect("a sheet");
    let geo = Geo::new(sheet, app.grid.rect.expect("the grid was painted"), app.view().scroll);
    geo.cell_rect(sheet, at).center()
}

fn button(h: &mut Harness, pos: Pos2, pressed: bool) {
    h.input_mut()
        .events
        .extend([Event::PointerMoved(pos), Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }]);
    h.step();
}

fn double_click(h: &mut Harness, pos: Pos2) {
    button(h, pos, true);
    button(h, pos, false);
    h.step();
    button(h, pos, true);
    button(h, pos, false);
    h.run_steps(2);
}

#[test]
fn double_clicking_a_value_writes_the_source_rows_to_a_new_sheet() {
    let mut h = harness();
    let at = value_cell(&h);
    let pos = centre(&h, at);
    double_click(&mut h, pos);

    let app = h.state();
    let wb = &app.session.doc().expect("a document").wb;
    assert_eq!(wb.sheets.len(), 2, "Show Details adds a sheet");
    let details = wb.sheets.get(1).expect("the details sheet");
    assert!(details.name.starts_with("East"), "named after the item: {}", details.name);
    // The source header row, then the two East records in source order.
    assert_eq!(details.value(CellRef::new(0, 0)), Value::from("Region"));
    assert_eq!(details.value(CellRef::new(0, 1)), Value::from("Sales"));
    assert_eq!(details.value(CellRef::new(1, 0)), Value::from("East"));
    assert_eq!(details.value(CellRef::new(1, 1)), Value::Number(10.0));
    assert_eq!(details.value(CellRef::new(2, 1)), Value::Number(30.0));
    assert_eq!(details.value(CellRef::new(3, 0)), Value::Empty);
}

#[test]
fn double_clicking_a_label_does_not_drill() {
    let mut h = harness();
    let at = label_cell(&h);
    let pos = centre(&h, at);
    double_click(&mut h, pos);
    let app = h.state();
    assert_eq!(app.session.doc().expect("a document").wb.sheets.len(), 1, "a label cell has no records of its own");
}
