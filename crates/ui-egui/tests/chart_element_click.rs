//! Clicking a chart element selects it in the Format pane, as Excel does: the first click
//! selects the chart, the next one the axis, series or point under the pointer.

use egui::{Event, Modifiers, PointerButton, Rect, accesskit::Role, pos2, vec2};
use egui_kittest::kittest::Queryable as _;
use gridcraft_chart::{Hit, HitShape};
use gridcraft_engine::Session;
use gridcraft_engine::core::CellRef;
use gridcraft_ui_egui::SheetApp;
use gridcraft_ui_egui::chart_format::Selection;
use gridcraft_ui_egui::grid::Geo;
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

/// A workbook with three categories and one column series, and a chart of it.
fn fixture() -> (Harness, u32) {
    let mut session = Session::new();
    session.new_workbook();
    session.execute("range.setValues", json!({"range": "A1", "values": [["Region", "Q1"], ["North", 48], ["South", 39], ["East", 55]]})).unwrap();
    session.execute("selection.set", json!({"range": "A1:B4"})).unwrap();
    let chart = session.execute("insert.chart", json!({"type": "column", "at": "D3", "width": 320, "height": 220})).unwrap();
    let id = chart["chart"].as_u64().unwrap() as u32;
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

/// The chart's rect on screen.
fn chart_rect(h: &Harness, id: u32) -> Rect {
    let app = h.state();
    let sh = app.session.doc().unwrap().wb.active().unwrap();
    let geo = Geo::new(sh, app.grid.rect.unwrap(), app.view().scroll);
    let a = sh.charts.iter().find(|c| c.id == id).unwrap().anchor;
    let min = geo.cell_rect(sh, CellRef::parse("D3").unwrap()).min + vec2(a.dx, a.dy) * geo.z;
    Rect::from_min_size(min, vec2(a.width, a.height) * geo.z)
}

/// Where the middle of the leftmost bar sits on screen: taken from the drawing's own click
/// regions (the UI measures with real fonts, the test with fixed widths, so only a bar's own
/// middle is safe to use — it is tens of pixels across).
fn a_bar(h: &Harness, id: u32) -> egui::Pos2 {
    let app = h.state();
    let wb = &app.session.doc().unwrap().wb;
    let sh = wb.active().unwrap();
    let chart = sh.charts.iter().find(|c| c.id == id).unwrap();
    let (w, ht) = (chart.anchor.width.max(1.0), chart.anchor.height.max(1.0));
    let data = gridcraft_chart::resolve(wb, wb.active_sheet, chart);
    let drawing = gridcraft_chart::draw(chart, &data, w, ht, &gridcraft_chart::ApproxMeasure);
    let (x, y) = drawing
        .hits
        .iter()
        .find_map(|r| match (r.hit, r.shape) {
            (Hit::Series { point: Some(_), .. }, HitShape::Rect { x, y, w, h }) => Some((x + w / 2.0, y + h / 2.0)),
            _ => None,
        })
        .expect("a bar to click");
    let rect = chart_rect(h, id);
    let z = rect.width() / w;
    pos2(rect.left() + x * z, rect.top() + y * z)
}

fn click(h: &mut Harness, at: egui::Pos2) {
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.run_steps(2);
    }
}

#[test]
fn clicking_a_bar_selects_its_series_and_that_point() {
    let (mut h, id) = fixture();
    // Another chart's element was selected: a click on this chart starts again at its area.
    h.state_mut().format_selection = Selection::AxisY;
    let at = a_bar(&h, id);
    click(&mut h, at);
    assert_eq!(h.state().selected_chart, Some(id), "the first click selects the chart");
    assert_eq!(h.state().format_selection, Selection::Chart, "and the chart area, as Excel does");

    // Not a double click: the second click is a separate gesture.
    h.run_steps(40);
    click(&mut h, at);
    assert_eq!(h.state().format_selection, Selection::Series(0), "the next click picks the bar's series");

    // The Data Point section is pointed at the bar that was clicked.
    h.state_mut().grid.pane = Some("formatChart".into());
    h.run_steps(2);
    h.get_by_role_and_label(Role::Button, "Data Point").click();
    h.run_steps(2);
    let point = h.get_all_by_role(Role::SpinButton).next().expect("the point field").value();
    assert_eq!(point.as_deref(), Some("1"), "the first bar is point 1 of 3");
}

#[test]
fn clicking_the_background_goes_back_to_the_chart_area() {
    let (mut h, id) = fixture();
    let at = a_bar(&h, id);
    click(&mut h, at);
    h.run_steps(40);
    click(&mut h, at);
    assert_eq!(h.state().format_selection, Selection::Series(0));
    h.run_steps(40);
    // The corner inside the chart's frame is its own background, past the padding.
    let rect = chart_rect(&h, id);
    click(&mut h, pos2(rect.left() + 3.0, rect.top() + 3.0));
    assert_eq!(h.state().format_selection, Selection::Chart, "a click on the frame's padding is the chart area");
    assert_eq!(h.state().selected_chart, Some(id), "the chart stays selected");
}
