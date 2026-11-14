//! Chart sheets in the UI: the tab strip marks one, activating it shows its chart, and the chart's
//! own Format pane still acts on it (the chart of a chart sheet is an ordinary chart model).

use egui::{Color32, Rect, Shape, vec2};
use egui_kittest::kittest::Queryable as _;
use gridcraft_engine::Session;
use gridcraft_ui_egui::SheetApp;
use gridcraft_ui_egui::chart_format::Selection;
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

/// A workbook with one worksheet holding a chart. The chart is moved onto a chart sheet by the
/// test, so both states can be looked at.
fn harness() -> (Harness, u32) {
    let mut session = Session::new();
    session.new_workbook();
    session
        .execute("range.setValues", json!({"range": "A1", "values": [["Region", "Q1"], ["North", 48], ["South", 39], ["East", 55], ["West", 61]]}))
        .unwrap();
    session.execute("selection.set", json!({"range": "A1:B5"})).unwrap();
    let chart = session.execute("insert.chart", json!({"type": "column", "at": "D2", "width": 420, "height": 260})).unwrap();
    let id = u32::try_from(chart["chart"].as_u64().unwrap()).unwrap();
    let mut fonts_ready = false;
    let mut h = egui_kittest::Harness::builder().with_step_dt(1.0 / 60.0).with_size(vec2(1300.0, 900.0)).build_ui_state(
        move |ui, app: &mut SheetApp| {
            if !fonts_ready {
                ui.ctx().set_fonts(gridcraft_ui_egui::theme::font_definitions());
                fonts_ready = true;
                return;
            }
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        SheetApp::new(session, Default::default()),
    );
    h.run_steps(6);
    (h, id)
}

/// The chart's card: the white rectangle whose drop shadow was painted under it (the grid paints
/// white cells too, the shadow is the chart's alone). On a chart sheet the chart fills the window,
/// on a worksheet it is the chart's own size.
fn chart_card(h: &Harness) -> Rect {
    let shadow = Color32::from_black_alpha(18);
    let s = h
        .output()
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            Shape::Rect(r) if r.fill == shadow => Some(r.rect),
            _ => None,
        })
        .max_by(|a, b| (a.width() * a.height()).total_cmp(&(b.width() * b.height())))
        .expect("a chart card was painted");
    Rect::from_min_size(s.min - vec2(0.0, 1.5), s.size())
}

#[test]
fn the_tab_strip_marks_a_chart_sheet_and_activating_it_fills_the_window() {
    let (mut h, id) = harness();
    assert!(h.state().session.doc().unwrap().wb.active_sheet == 0, "the worksheet is active to start with");
    // The worksheet's chart is 420 x 260 at zoom 1.
    let card = chart_card(&h);
    assert!((card.width() - 420.0).abs() < 2.0, "the worksheet's chart card is its anchor size: {card:?}");
    // Move Chart ▸ New sheet: the tab strip gains a chart sheet, marked as one.
    h.state_mut().run("chart.moveToSheet", json!({"chart": id, "name": "Chart1"})).unwrap();
    h.state_mut().run("sheet.activate", json!({"sheet": 0})).unwrap();
    h.run_steps(6);
    let tab = h.get_by_label("Chart1, Chart sheet");
    tab.click();
    h.run_steps(6);
    assert_eq!(h.state().session.doc().unwrap().wb.active_sheet, 1, "clicking the tab activates the chart sheet");
    let view = h.state().grid.cells_rect.expect("the grid was painted");
    let card = chart_card(&h);
    // Full-bleed: the chart is scaled up to fill the window, far past its 420 x 260 anchor size and
    // wider or taller than the whole grid area.
    assert!(card.width() >= view.width() - 1.0 || card.height() >= view.height() - 1.0, "chart {card:?} does not fill {view:?}");
    assert!(card.width() > 420.0 && card.height() > 260.0, "the chart did not grow: {card:?}");
}

#[test]
fn the_chart_sheets_chart_takes_the_format_pane() {
    let (mut h, id) = harness();
    h.state_mut().run("chart.moveToSheet", json!({"chart": id, "name": "Chart1"})).unwrap();
    h.run_steps(4);
    h.get_by_label("Chart1, Chart sheet").click();
    h.run_steps(6);
    h.state_mut().selected_chart = Some(id);
    h.state_mut().format_selection = Selection::Chart;
    h.state_mut().grid.pane = Some("formatChart".into());
    h.run_steps(4);
    // The Format pane's legend option acts on the chart the chart sheet holds.
    assert!(!h.state().session.doc().unwrap().wb.sheet(1).unwrap().charts.is_empty(), "the chart is still there");
    h.state_mut().run("chart.setLegend", json!({"position": "top"})).unwrap();
    h.run_steps(2);
    let wb = &h.state().session.doc().unwrap().wb;
    let ch = wb.sheet(1).unwrap().chart_sheet_chart().expect("the chart sheet's chart");
    assert_eq!(ch.legend, gridcraft_engine::model::LegendPos::Top);
    assert!(wb.sheet(1).unwrap().cells.is_empty(), "the chart sheet still has no cells");
}
