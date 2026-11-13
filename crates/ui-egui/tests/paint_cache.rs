//! What the sheet is painted from — conditional-format statistics, charts, sparklines — is far
//! too slow to rebuild every frame, so the grid keeps it until the document changes: a repaint
//! that follows no change lays nothing out, and an edit lays it out again.

use egui::vec2;
use gridcraft_engine::Session;
use gridcraft_engine::cf::stats_built;
use gridcraft_ui_egui::SheetApp;
use gridcraft_ui_egui::grid::object_layouts;
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

fn harness(session: Session) -> Harness {
    let mut h = egui_kittest::Harness::builder().with_size(vec2(1200.0, 800.0)).build_ui_state(
        |ui, app: &mut SheetApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        SheetApp::new(session, Default::default()),
    );
    h.run_steps(4);
    h
}

/// A data bar reads its range's smallest and largest number: the statistics behind it scan and
/// sort every cell of the range, so they are built on the first look and kept afterwards.
#[test]
fn a_data_bars_statistics_survive_a_repaint_and_follow_an_edit() {
    let mut session = Session::new();
    session.new_workbook();
    let rows: Vec<serde_json::Value> = (1..=500).map(|i| json!([i])).collect();
    session.execute("range.setValues", json!({"range": "A1", "values": rows})).unwrap();
    session.execute("home.conditionalFormat", json!({"range": "A1:A500", "rule": {"type": "dataBar"}})).unwrap();
    let mut h = harness(session);
    let first = stats_built();
    assert!(first > 0, "the visible cells' bars needed the range's statistics");
    h.run_steps(5);
    assert_eq!(stats_built(), first, "a repaint of an unchanged document builds no statistics");
    // Moving the selection is not a document change either: the bars keep their statistics.
    h.state_mut().run("selection.set", json!({"range": "A20:A60"})).unwrap();
    h.run_steps(3);
    assert_eq!(stats_built(), first, "a selection change repaints without rescanning the range");
    // An edit changes the numbers the statistics come from.
    h.state_mut().run("cell.set", json!({"cell": "A1", "input": "9999"})).unwrap();
    h.run_steps(2);
    let after_edit = stats_built();
    assert!(after_edit > first, "an edit builds the statistics again, so the bars follow it");
    h.run_steps(3);
    assert_eq!(stats_built(), after_edit, "and the next repaint keeps them");
}

/// Laying a chart out (axis ticks, measured labels) is the expensive part of painting one, and it
/// depends on the document, not on the frame.
#[test]
fn a_chart_is_laid_out_once_and_again_after_a_change() {
    let mut session = Session::new();
    session.new_workbook();
    session.execute("range.setValues", json!({"range": "A1", "values": [["Region", "Q1"], ["North", 48], ["South", 39]]})).unwrap();
    session.execute("selection.set", json!({"range": "A1:B3"})).unwrap();
    let id = session.execute("insert.chart", json!({"type": "column", "at": "D3", "width": 320, "height": 220})).unwrap()["chart"].as_u64().unwrap()
        as u32;
    let mut h = harness(session);
    let first = object_layouts();
    assert!(first >= 1, "the chart was laid out to be painted");
    h.run_steps(5);
    assert_eq!(object_layouts(), first, "a repaint of an unchanged document lays nothing out");
    // Selecting it paints a frame around the drawing the cache already holds.
    h.state_mut().selected_chart = Some(id);
    h.run_steps(3);
    assert_eq!(object_layouts(), first, "selecting a chart does not lay it out again");
    // The data changed: the chart is laid out again, and kept afterwards.
    h.state_mut().run("cell.set", json!({"cell": "B2", "input": "50"})).unwrap();
    h.run_steps(2);
    let after_edit = object_layouts();
    assert!(after_edit > first, "an edit lays the chart out again");
    h.run_steps(3);
    assert_eq!(object_layouts(), after_edit, "and the next repaint keeps it");
    // A resize changes the picture without the data moving.
    h.state_mut().run("object.move", json!({"kind": "chart", "id": id, "width": 420, "height": 260})).unwrap();
    h.run_steps(2);
    assert!(object_layouts() > after_edit, "a resize lays the chart out again");
}

/// A sparkline parses and evaluates its source formula, which is far too slow for every frame.
#[test]
fn a_sparkline_is_drawn_once_and_again_after_a_change() {
    let mut session = Session::new();
    session.new_workbook();
    let rows: Vec<serde_json::Value> = (1..=12).map(|i| json!([i * 3, i * 2])).collect();
    session.execute("range.setValues", json!({"range": "A1", "values": rows})).unwrap();
    session.execute("insert.sparkline", json!({"range": "A1:A12", "location": "D1"})).unwrap();
    let mut h = harness(session);
    let first = object_layouts();
    assert!(first >= 1, "the visible sparkline was drawn");
    h.run_steps(5);
    assert_eq!(object_layouts(), first, "a repaint of an unchanged document draws nothing new");
    h.state_mut().run("cell.set", json!({"cell": "A1", "input": "100"})).unwrap();
    h.run_steps(2);
    let after_edit = object_layouts();
    assert!(after_edit > first, "an edit to the source draws the sparkline again");
    h.run_steps(3);
    assert_eq!(object_layouts(), after_edit, "and the next repaint keeps it");
}
