//! Review ▸ Check Accessibility: the dialog lists where each issue is, goes to it, and sets an
//! object's alternative text.

use egui::{Event, Modifiers, PointerButton, accesskit::Role, vec2};
use egui_kittest::kittest::Queryable as _;
use gridcraft_engine::Session;
use gridcraft_ui_egui::SheetApp;
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

/// A workbook with a chart that has no alternative text.
fn fixture() -> (Harness, u32) {
    let mut session = Session::new();
    session.new_workbook();
    session.execute("range.setValues", json!({"range": "A1", "values": [["Region", "Sales"], ["North", 10], ["South", 20]]})).unwrap();
    session.execute("selection.set", json!({"range": "A1:B3"})).unwrap();
    let chart = session.execute("insert.chart", json!({"type": "column", "at": "D2", "width": 300, "height": 200})).unwrap();
    let id = chart["chart"].as_u64().unwrap() as u32;
    session.execute("selection.set", json!({"range": "A1"})).unwrap();
    let mut h = egui_kittest::Harness::builder().with_size(vec2(1200.0, 800.0)).build_ui_state(
        |ui, app: &mut SheetApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        SheetApp::new(session, Default::default()),
    );
    h.run_steps(4);
    (h, id)
}

fn click(h: &mut Harness, at: egui::Pos2) {
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.run_steps(2);
    }
}

#[test]
fn the_dialog_goes_to_an_object_and_takes_its_alt_text() {
    let (mut h, id) = fixture();
    h.state_mut().open_dialog("accessibility", json!({}));
    h.run_steps(4);
    assert!(h.query_by_label_contains("Missing alternative text").is_some(), "the issue is listed");

    // "Go to" selects the chart that needs describing (the first issue is the chart's alt text).
    let go = h.get_all_by_label("Go to").next().expect("a Go to button").rect();
    click(&mut h, go.center());
    assert_eq!(h.state().selected_chart, Some(id), "the chart is selected");

    // The field takes a description and Set writes it; the issue list refreshes without it. The
    // name box and the formula bar are text inputs too, so the field is found beside its label.
    let label = h.get_by_label("Alt text").rect();
    let field =
        h.get_all_by_role(Role::TextInput).map(|n| n.rect()).find(|r| (r.center().y - label.center().y).abs() < 8.0).expect("the alt text field");
    click(&mut h, field.center());
    h.input_mut().events.push(Event::Text("Sales by region".into()));
    h.run_steps(2);
    let set = h.get_by_label("Set").rect();
    click(&mut h, set.center());
    let alt = {
        let d = h.state().session.doc().unwrap();
        d.wb.active().unwrap().charts.iter().find(|c| c.id == id).and_then(|c| c.alt.clone())
    };
    assert_eq!(alt.as_deref(), Some("Sales by region"), "the chart was described");
    assert!(h.query_by_label_contains("Missing alternative text").is_none(), "the issue is gone");
    // The workbook's default sheet name is still an issue, so the list is not empty: it says
    // nothing about alternative text any more.
    assert!(h.query_by_label_contains("Default sheet name").is_some(), "the other issue stays");
}

#[test]
fn the_dialog_names_a_merged_range_and_selects_it() {
    let (mut h, _) = fixture();
    h.state_mut().session.execute("selection.set", json!({"range": "A8:B9"})).unwrap();
    h.state_mut().session.execute("home.mergeCells", json!({})).unwrap();
    h.state_mut().open_dialog("accessibility", json!({}));
    h.run_steps(4);
    assert!(h.query_by_label_contains("A8:B9").is_some(), "the merged range is named");
    // The merged-cells row's Go to button is the second one (after the chart's alt text).
    let gotos: Vec<egui::Rect> = h.get_all_by_label("Go to").map(|n| n.rect()).collect();
    assert!(gotos.len() >= 2, "one per issue: {}", gotos.len());
    click(&mut h, gotos[1].center());
    let sel = h.state().session.active().unwrap().selection.current();
    assert_eq!(sel.start.a1(), "A8", "the merged range is selected");
    assert_eq!(sel.end.a1(), "B9");
}
