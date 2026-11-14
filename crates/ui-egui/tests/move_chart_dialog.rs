//! Chart Design ▸ Move Chart: the dialog Excel opens from Location. Its two choices put a floating
//! chart on a chart sheet of its own or a chart sheet's chart back on a worksheet as an object.

use egui::{Event, Key, Modifiers, accesskit::Role, vec2};
use egui_kittest::kittest::Queryable as _;
use gridcraft_engine::Session;
use gridcraft_ui_egui::SheetApp;
use gridcraft_ui_egui::control::{Outcome, handle};
use serde_json::{Value as Json, json};

type Harness = egui_kittest::Harness<'static, SheetApp>;

/// Two worksheets — Sheet1 with a floating column chart, and Notes — so "Object in" has a choice
/// to make. The chart is the one insert.chart makes, as the ribbon inserts it.
fn harness() -> (Harness, u32) {
    let mut session = Session::new();
    session.new_workbook();
    session.execute("range.setValues", json!({"range": "A1", "values": [["Region", "Q1"], ["North", 48], ["South", 39], ["East", 55]]})).unwrap();
    session.execute("selection.set", json!({"range": "A1:B4"})).unwrap();
    let chart = session.execute("insert.chart", json!({"type": "column", "at": "D3", "width": 320, "height": 220})).unwrap();
    let id = u32::try_from(chart["chart"].as_u64().unwrap()).unwrap();
    session.execute("home.insertSheet", json!({"name": "Notes"})).unwrap();
    session.execute("sheet.activate", json!({"sheet": "Sheet1"})).unwrap();
    let mut h = egui_kittest::Harness::builder().with_size(vec2(1200.0, 800.0)).build_ui_state(
        |ui, app: &mut SheetApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        SheetApp::new(session, Default::default()),
    );
    h.run_steps(6);
    (h, id)
}

fn control(h: &mut Harness, method: &str, params: Json) -> Json {
    let ctx = h.ctx.clone();
    match handle(h.state_mut(), &ctx, method, &params) {
        Outcome::Done(v) => v,
        Outcome::Screenshot { .. } => json!(null),
    }
}

/// Chart Design ▸ Location ▸ Move Chart, as a user reaches it: the chart selected, the contextual
/// tab open, the button clicked.
fn open_from_ribbon(h: &mut Harness, id: u32) {
    h.state_mut().selected_chart = Some(id);
    h.state_mut().ui.ribbon_tab = "Chart Design".into();
    h.run_steps(4);
    h.get_by_label("Move Chart").click();
    h.run_steps(4);
}

fn dialog_values(h: &Harness) -> Json {
    let d = h.state().dialog.as_ref().expect("the Move Chart dialog is open");
    assert_eq!(d.name, "moveChart");
    Json::Object(d.values.clone())
}

/// The dialog's text field: the only one inside the window (the grid's name box and formula bar
/// sit outside it).
fn dialog_field(h: &Harness) -> egui::Rect {
    let area = h.ctx.memory(|m| m.area_rect(egui::Id::new("dialog").with("moveChart"))).expect("the Move Chart dialog is open");
    h.get_all_by_role(Role::TextInput).map(|n| n.rect()).find(|r| area.contains(r.center())).expect("the name field")
}

/// Clicks the field, replaces its text with the argument, then presses Enter (which a refused name
/// swallows, so the test can look at both).
fn retype(h: &mut Harness, field: egui::Rect, text: &str) {
    h.input_mut().events.push(Event::PointerMoved(field.center()));
    h.run_steps(1);
    h.get_all_by_role(Role::TextInput).find(|n| n.rect() == field).unwrap().click();
    h.run_steps(2);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(1);
    for c in text.chars() {
        h.input_mut().events.push(Event::Text(c.to_string()));
        h.run_steps(2);
    }
    h.key_press(Key::Enter);
    h.run_steps(2);
}

#[test]
fn a_floating_chart_moves_onto_a_new_chart_sheet_with_the_name_the_dialog_holds() {
    let (mut h, id) = harness();
    open_from_ribbon(&mut h, id);
    // The chart floats on a worksheet, so the dialog proposes a chart sheet of its own, named
    // after the workbook's next ChartN.
    let values = dialog_values(&h);
    assert_eq!(values["where"], json!("newSheet"));
    assert_eq!(values["name"], json!("Chart1"));
    assert_eq!(values["chart"], json!(id));
    // "Object in" switches the command OK runs, and back.
    h.get_by_label("Object in").click();
    h.run_steps(2);
    assert_eq!(h.state().dialog.as_ref().unwrap().command, Some("chart.moveToObject"));
    h.get_by_label("New sheet").click();
    h.run_steps(2);
    assert_eq!(h.state().dialog.as_ref().unwrap().command, Some("chart.moveToSheet"));
    h.get_by_label("   OK   ").click();
    h.run_steps(4);
    assert!(h.state().dialog.is_none(), "OK closes the dialog");
    let wb = &h.state().session.doc().unwrap().wb;
    let sheet = wb.sheet(1).unwrap();
    assert!(sheet.is_chart_sheet(), "{:?} is not a chart sheet", sheet.name);
    assert_eq!(sheet.name, "Chart1");
    assert!(sheet.chart_sheet_chart().is_some(), "the chart sheet holds the chart");
    assert!(wb.sheet(0).unwrap().charts.is_empty(), "the chart left its worksheet");
    assert_eq!(wb.active_sheet, 1, "the new chart sheet is the active one");
}

#[test]
fn a_chart_sheets_chart_goes_back_onto_the_chosen_worksheet_as_an_object() {
    let (mut h, id) = harness();
    h.state_mut().run("chart.moveToSheet", json!({"chart": id, "name": "Chart1"})).unwrap();
    h.run_steps(2);
    // A chart sheet's chart is the state "Object in" is for, so that is what the dialog proposes.
    open_from_ribbon(&mut h, id);
    let values = dialog_values(&h);
    assert_eq!(values["where"], json!("object"));
    assert_eq!(values["sheet"], json!("Sheet1"), "the nearest worksheet before the chart sheet, as the command defaults to");
    assert_eq!(h.state().session.doc().unwrap().wb.sheets.len(), 3);
    // The dropdown offers worksheets only: the chart sheet that is going away is not on it.
    assert_eq!(dialog_values(&h)["sheets"], json!(["Sheet1", "Notes"]));
    control(&mut h, "ui.dialog.set", json!({"field": "sheet", "value": "Notes"}));
    h.get_by_label("   OK   ").click();
    h.run_steps(4);
    assert!(h.state().dialog.is_none(), "OK closes the dialog");
    let wb = &h.state().session.doc().unwrap().wb;
    assert_eq!(wb.sheets.len(), 2, "the chart sheet went with the chart");
    assert!(wb.sheets.iter().all(|s| !s.is_chart_sheet()), "no chart sheet is left");
    let notes = wb.sheet_index("Notes").expect("the chosen worksheet");
    assert_eq!(wb.active_sheet, notes, "the chart lands on the worksheet it was sent to");
    let chart = wb.sheet(notes).unwrap().charts.first().expect("the chart is a floating object there");
    assert_eq!(chart.id, id);
    assert!(wb.sheet(0).unwrap().charts.is_empty());
}

#[test]
fn a_blank_name_falls_back_to_the_next_chart_sheet_name() {
    let (mut h, id) = harness();
    // The same dialog the ribbon opens, through the control channel so the blank field can be set
    // as typed: OK still runs the choice's command.
    control(&mut h, "ui.dialog", json!({"name": "moveChart", "chart": id}));
    control(&mut h, "ui.dialog.set", json!({"field": "name", "value": ""}));
    let r = control(&mut h, "ui.dialog.confirm", json!({}));
    assert_eq!(r["ok"], json!(true), "{r}");
    assert!(h.state().dialog.is_none(), "the dialog closed");
    let wb = &h.state().session.doc().unwrap().wb;
    assert_eq!(wb.sheet(1).map(|s| s.name.as_str()), Some("Chart1"), "the workbook's next ChartN");
    assert!(wb.sheet(1).is_some_and(|s| s.is_chart_sheet()));
}

#[test]
fn a_name_another_sheet_has_is_refused_inline_and_ok_changes_nothing() {
    let (mut h, id) = harness();
    open_from_ribbon(&mut h, id);
    // Excel's rule: a chart sheet cannot take a name the workbook already has.
    let field = dialog_field(&h);
    retype(&mut h, field, "Notes");
    assert_eq!(h.state().dialog.as_ref().unwrap().values["name"], json!("Notes"));
    assert!(h.query_by_label_contains("already taken").is_some(), "the refusal is shown in the dialog");
    // OK is disabled, so the click leaves the workbook alone and the dialog open to be fixed.
    h.get_by_label("   OK   ").click();
    h.run_steps(4);
    assert!(h.state().dialog.is_some(), "the dialog stays open");
    let wb = &h.state().session.doc().unwrap().wb;
    assert_eq!(wb.sheets.len(), 2, "no sheet was added");
    assert!(wb.sheets.iter().all(|s| !s.is_chart_sheet()));
    assert!(wb.sheet(0).unwrap().charts.iter().any(|c| c.id == id), "the chart is still on its worksheet");
}

#[test]
fn a_choice_the_chart_cannot_make_shows_the_engine_message_and_keeps_the_dialog() {
    let (mut h, id) = harness();
    open_from_ribbon(&mut h, id);
    // Both choices are drawn as in Excel; a floating chart has no chart sheet whose chart could
    // come back, so the engine refuses and the dialog stays open to pick the other choice.
    h.get_by_label("Object in").click();
    h.run_steps(2);
    h.get_by_label("   OK   ").click();
    h.run_steps(4);
    assert!(h.state().dialog.is_some(), "the dialog stays open");
    let message = h.state().message.clone().expect("the engine's message is shown");
    assert!(message.1.contains("chart sheet"), "{message:?}");
    let wb = &h.state().session.doc().unwrap().wb;
    assert_eq!(wb.sheets.len(), 2, "nothing moved");
    assert!(wb.sheet(0).unwrap().charts.iter().any(|c| c.id == id));
}

#[test]
fn cancel_changes_nothing() {
    let (mut h, id) = harness();
    open_from_ribbon(&mut h, id);
    h.get_by_label(" Cancel ").click();
    h.run_steps(4);
    assert!(h.state().dialog.is_none(), "Cancel closes the dialog");
    let wb = &h.state().session.doc().unwrap().wb;
    assert_eq!(wb.sheets.len(), 2);
    assert!(wb.sheets.iter().all(|s| !s.is_chart_sheet()));
    assert!(wb.sheet(0).unwrap().charts.iter().any(|c| c.id == id));
    assert_eq!(wb.active_sheet, 0);
}
