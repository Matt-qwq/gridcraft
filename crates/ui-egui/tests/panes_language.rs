//! The task panes, the Format pane and the object previews read Chinese on a Chinese screen: the
//! pivot field list (its areas, buttons and layout list), the Format pane's sections, and the
//! selection pane's slicers. The catalog answers each label; a pane whose label was never added
//! would show the English here.

use egui_kittest::kittest::Queryable as _;
use gridcraft_engine::Session;
use gridcraft_ui_egui::SheetApp;
use gridcraft_ui_egui::i18n::Language;
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

fn harness(session: Session, language: Language) -> Harness {
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_ui_state(
        |ui, app: &mut SheetApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        {
            let mut app = SheetApp::new(session, Default::default());
            app.ui.language = language;
            app
        },
    );
    h.run_steps(4);
    h
}

/// A worksheet with a PivotTable whose active cell is inside the report, so the field list shows.
fn pivot_session() -> Session {
    let mut session = Session::new();
    session.new_workbook();
    session.execute("range.setValues", json!({"range": "A1", "values": [["Region", "Sales"], ["East", 100], ["West", 80]]})).unwrap();
    session
        .execute("insert.pivotTable", json!({"source": "Sheet1!A1:B3", "destination": "Sheet1!D3", "rows": ["Region"], "values": ["Sales"]}))
        .unwrap();
    session.execute("selection.set", json!({"cell": "D3"})).unwrap();
    session
}

#[test]
fn the_pivot_field_list_speaks_chinese() {
    let h = harness(pivot_session(), Language::Zh);
    // The caption, its hint, the four areas and the pane's own buttons.
    for label in ["数据透视表字段", "选择要添加到报表的字段：", "用字段菜单在各区域之间移动：", "行", "列", "值", "筛选器", "刷新", "计算字段…"]
    {
        assert!(h.query_by_label(label).is_some(), "the pivot pane should show {label}");
    }
    for english in ["PivotTable Fields", "Choose fields to add to report:", "Calculated Field…", "Refresh"] {
        assert!(h.query_by_label(english).is_none(), "{english} is untranslated in the pivot pane");
    }
}

#[test]
fn the_format_pane_speaks_chinese() {
    let mut session = Session::new();
    session.new_workbook();
    session.execute("range.setValues", json!({"range": "A1", "values": [["Region", "Sales"], ["East", 10], ["West", 20]]})).unwrap();
    session.execute("selection.set", json!({"range": "A1:B3"})).unwrap();
    let id = session.execute("insert.chart", json!({"type": "column", "at": "D2", "width": 300, "height": 200})).unwrap()["chart"]
        .as_u64()
        .unwrap_or(0) as u32;
    let mut h = harness(session, Language::Zh);
    h.state_mut().selected_chart = Some(id);
    h.state_mut().grid.pane = Some("formatChart".into());
    h.run_steps(3);
    // The pane's title (named after the selected object) and the chart area's sections.
    for label in ["设置图表区格式", "图表标题", "图表类型", "图例", "元素"] {
        assert!(h.query_by_label(label).is_some(), "the Format pane should show {label}");
    }
    for english in ["Format Chart Area", "Chart Title", "Chart Type", "Legend"] {
        assert!(h.query_by_label(english).is_none(), "{english} is untranslated in the Format pane");
    }
}

#[test]
fn the_selection_pane_names_a_slicer_in_chinese() {
    let mut session = pivot_session();
    let name = session.execute("insert.slicer", json!({"pivot": "PivotTable1", "field": "Region"})).unwrap()["name"]
        .as_str()
        .unwrap_or("Region")
        .to_string();
    let mut h = harness(session, Language::Zh);
    h.state_mut().grid.pane = Some("selection".into());
    h.run_steps(3);
    assert!(h.query_by_label("选择").is_some(), "the selection pane's caption reads Chinese");
    let slicer = format!("切片器 {name}");
    assert!(h.query_by_label(&slicer).is_some(), "the pane should list {slicer}");
    assert!(h.query_by_label(&format!("Slicer {name}")).is_none(), "the slicer's row is untranslated");
}
