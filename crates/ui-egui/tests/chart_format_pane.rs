//! The Format pane drives the four chart commands, and disables what the chart kind cannot store.

use egui::{Event, Key, Modifiers, PointerButton, Rect, accesskit::Role, vec2};
use egui_kittest::kittest::Queryable as _;
use gridcraft_engine::Session;
use gridcraft_engine::model::{AxisCrossing, Chart, ChartKind, ErrorBarDirection, ErrorBarKind, LabelPosition};
use gridcraft_ui_egui::SheetApp;
use gridcraft_ui_egui::chart_format::Selection;
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

/// A workbook with one two-column block and a chart of it on the active sheet.
fn fixture(kind: &str) -> (Harness, u32) {
    let mut session = Session::new();
    session.new_workbook();
    session
        .execute("range.setValues", json!({"range": "A1", "values": [["Region", "Q1"], ["North", 48], ["South", 39], ["East", 55], ["West", 61]]}))
        .unwrap();
    session.execute("selection.set", json!({"range": "A1:B5"})).unwrap();
    let chart = session.execute("insert.chart", json!({"type": kind, "at": "D2", "width": 420, "height": 260})).unwrap();
    let id = chart["chart"].as_u64().unwrap() as u32;
    let mut h = egui_kittest::Harness::builder().with_size(vec2(1300.0, 1000.0)).build_ui_state(
        |ui, app: &mut SheetApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        SheetApp::new(session, Default::default()),
    );
    h.run_steps(4);
    h.state_mut().selected_chart = Some(id);
    h.state_mut().grid.pane = Some("formatChart".into());
    h.run_steps(2);
    (h, id)
}

fn chart(h: &Harness, id: u32) -> Chart {
    h.state().session.doc().unwrap().wb.active().unwrap().charts.iter().find(|c| c.id == id).cloned().expect("the chart")
}

fn select(h: &mut Harness, what: Selection) {
    h.state_mut().format_selection = what;
    h.run_steps(2);
}

/// Opens a collapsing section of the pane by its header.
fn expand(h: &mut Harness, title: &str) {
    h.get_by_role_and_label(Role::Button, title).click();
    h.run_steps(2);
}

fn click(h: &mut Harness, label: &str) {
    h.get_by_label(label).click();
    h.run_steps(2);
}

/// Clicks a combo box (found by its selected text) and then an entry in its list.
fn choose(h: &mut Harness, selected: &str, entry: &str) {
    h.get_by_value(selected).click();
    h.run_steps(2);
    click(h, entry);
}

/// Types into a number field and commits it with Enter.
fn type_number(h: &mut Harness, at: Rect, text: &str) {
    let at = at.center();
    h.input_mut().events.push(Event::PointerMoved(at));
    h.run_steps(1);
    h.input_mut().events.push(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(2);
    h.input_mut().events.push(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(1);
    // Clicking a DragValue turns it into a text field with its text selected.
    h.input_mut().events.push(Event::Text(text.into()));
    h.run_steps(1);
    h.input_mut().events.push(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.input_mut().events.push(Event::Key { key: Key::Enter, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

#[test]
fn the_series_sections_drive_the_label_and_error_bar_commands() {
    let (mut h, id) = fixture("column");
    assert_eq!(chart(&h, id).series.len(), 1);
    select(&mut h, Selection::Series(0));
    expand(&mut h, "Data Labels");
    assert!(!chart(&h, id).data_labels);
    click(&mut h, "Show labels for this series");
    let c = chart(&h, id);
    assert!(c.data_labels, "labels turn on");
    assert_eq!(c.series[0].labels.as_ref().map(|l| l.show_value), Some(true), "the value is the default part");
    click(&mut h, "Percentage");
    click(&mut h, "Category name");
    let l = chart(&h, id).series[0].labels.clone().expect("series labels");
    assert!(l.show_percentage && l.show_category && l.show_value);
    choose(&mut h, "Automatic", "Outside End");
    assert_eq!(chart(&h, id).series[0].labels.as_ref().unwrap().position, LabelPosition::OutsideEnd);

    expand(&mut h, "Error Bars");
    choose(&mut h, "None", "Standard deviation");
    let c = chart(&h, id);
    assert_eq!(c.series[0].error_bars.as_ref().map(|b| b.kind.clone()), Some(ErrorBarKind::StdDev(1.0)));
    assert_eq!(c.series[0].error_bars.as_ref().unwrap().direction, ErrorBarDirection::Both);
    click(&mut h, "Plus");
    assert_eq!(chart(&h, id).series[0].error_bars.as_ref().unwrap().direction, ErrorBarDirection::Plus);
    click(&mut h, "End caps");
    assert!(chart(&h, id).series[0].error_bars.as_ref().unwrap().no_end_cap, "unticking End caps turns the caps off");
}

#[test]
fn the_point_section_formats_and_hides_one_point() {
    let (mut h, id) = fixture("column");
    select(&mut h, Selection::Series(0));
    expand(&mut h, "Data Point");
    click(&mut h, "Hide this point's label");
    let pts = chart(&h, id).series[0].points.clone();
    assert_eq!(pts.len(), 1);
    assert!(pts[0].no_label && pts[0].index == 0, "the first point's label is hidden: {pts:?}");
    click(&mut h, "Hide this point's label");
    assert!(!chart(&h, id).series[0].points[0].no_label, "showing it again keeps the point record");
    // A marker colour is refused on columns: the control is disabled and says why.
    assert!(h.query_by_label_contains("marker colour applies").is_some(), "the marker control says why it cannot be used");
    click(&mut h, "Clear point formatting");
    assert!(chart(&h, id).series[0].points.is_empty());
}

#[test]
fn the_axis_section_sets_and_clears_the_value_axis_options() {
    let (mut h, id) = fixture("column");
    select(&mut h, Selection::AxisY);
    expand(&mut h, "Axis Options");
    // Automatic bounds have no field; the Auto box fixes one at the data's edge.
    assert!(h.query_by_label("Minimum").is_some());
    assert_eq!(chart(&h, id).y_axis.min, None);
    assert!(h.get_all_by_label("Auto").count() >= 3, "a box for the minimum, the maximum and the major unit");
    h.get_all_by_label("Auto").next().expect("the minimum's box").click();
    h.run_steps(2);
    let c = chart(&h, id);
    assert_eq!(c.y_axis.min, Some(39.0), "the minimum starts at the data's own edge: {:?}", c.y_axis);
    // Its field is now there; type a new minimum into it.
    let field = h.get_all_by_role(Role::SpinButton).next().expect("a minimum field").rect();
    type_number(&mut h, field, "10");
    assert_eq!(chart(&h, id).y_axis.min, Some(10.0));
    click(&mut h, "Values in reverse order");
    assert!(chart(&h, id).y_axis.reversed);
    choose(&mut h, "Automatic", "Axis value");
    assert_eq!(chart(&h, id).y_axis.crosses, AxisCrossing::At(0.0));
    choose(&mut h, "None", "Thousands");
    assert_eq!(chart(&h, id).y_axis.display_units.map(|u| u.label()), Some("Thousands"));
    // Ticking the minimum's Auto again clears it while the other options stay.
    h.get_all_by_label("Auto").next().expect("the minimum's box").click();
    h.run_steps(2);
    let c = chart(&h, id).y_axis;
    assert_eq!(c.min, None, "the minimum is automatic again");
    assert!(c.reversed, "reversing stays");
    assert_eq!(c.crosses, AxisCrossing::At(0.0));
    assert_eq!(c.display_units.map(|u| u.label()), Some("Thousands"));
}

#[test]
fn what_the_chart_kind_cannot_store_is_disabled_with_its_reason() {
    // A pie has no axes at all.
    let (mut h, _) = fixture("pie");
    select(&mut h, Selection::AxisY);
    expand(&mut h, "Axis Options");
    assert!(h.query_by_label_contains("no axes").is_some(), "the pane says a pie has no axes");
    assert!(h.query_by_label("Minimum").is_none(), "no bound field is offered");
    assert!(h.query_by_label("Logarithmic scale").is_none());

    // A label category axis takes neither a log scale, display units, bounds nor a crossing.
    let (mut h, _) = fixture("column");
    select(&mut h, Selection::AxisX);
    expand(&mut h, "Axis Options");
    assert!(h.query_by_label_contains("A category axis has no bounds").is_some());
    assert!(h.query_by_label("Logarithmic scale").is_none());
    assert!(h.query_by_label("Display units").is_none());
    assert!(h.query_by_label("Minimum").is_none());
    // But the label interval and reverse order are real category options.
    assert!(h.query_by_label("Labels every").is_some());
    assert!(h.query_by_label("Values in reverse order").is_some());

    // A chartex chart stores none of these options in a part GridCraft models.
    let (mut h, id) = fixture("treemap");
    select(&mut h, Selection::Series(0));
    expand(&mut h, "Data Labels");
    assert!(h.query_by_label_contains("chartex part").is_some(), "the pane says why");
    assert!(h.query_by_label("Show labels for this series").is_none());
    expand(&mut h, "Error Bars");
    expand(&mut h, "Data Point");
    assert!(h.query_all_by_label_contains("chartex part").count() >= 2, "each open section says why");
    assert!(h.query_by_label("Hide this point's label").is_none());
    assert!(chart(&h, id).kind.is_chartex());
}

#[test]
fn the_series_pane_moves_a_series_to_the_secondary_axis_and_back() {
    let (mut h, id) = fixture("column");
    select(&mut h, Selection::Series(0));
    expand(&mut h, "Series Options");
    assert!(!chart(&h, id).series[0].secondary);
    click(&mut h, "Plot on secondary axis");
    let c = chart(&h, id);
    assert!(c.series[0].secondary, "the series is on the secondary axis");
    assert_eq!(c.kind, ChartKind::Combo, "a per-series axis makes a combo chart");
    click(&mut h, "Plot on secondary axis");
    assert!(!chart(&h, id).series[0].secondary, "the checkbox moves it back");
}
