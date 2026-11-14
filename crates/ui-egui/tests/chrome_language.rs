//! The chrome a user looks at all day speaks the interface language: the grid's cell menu, the
//! filter list's blank-cells entry, the status bar, the sheet tabs' menu and a ribbon tooltip.
//!
//! Each case renders the real window (not just the catalog) and reads what a screen reader and a
//! test can both see, so a label that stops going through the translation lookup fails here.

use egui_kittest::kittest::Queryable;
use gridcraft_engine::Session;
use gridcraft_ui_egui::SheetApp;
use gridcraft_ui_egui::i18n::Language;
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

/// A window with a small filtered table: a column with a blank cell (the filter list's (Blanks)
/// entry), a chart (the object menu) and one sheet (the tab strip).
fn build(language: Language) -> Harness {
    let mut session = Session::new();
    session.new_workbook();
    session.execute("range.setValues", json!({"range": "A1:B4", "values": [["Name", "Qty"], ["Ana", "3"], ["Bia", ""], ["Caio", "5"]]})).unwrap();
    session.execute("data.filter", json!({"range": "A1:B4"})).unwrap();
    session.execute("insert.chart", json!({"type": "column", "at": "D2", "width": 300, "height": 200})).unwrap();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_ui_state(
        |ui, app: &mut SheetApp| {
            app.logic(ui.ctx());
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

/// Whether any node carries `label` (a painted button and the menu item of the same command
/// share one, so a query for a single node would be ambiguous).
fn has_label(h: &Harness, label: &str) -> bool {
    h.query_all_by_label(label).next().is_some()
}

/// Whether any node carries `value` as its accessible value: plain label text arrives that way.
fn has_value(h: &Harness, value: &str) -> bool {
    h.query_all_by_value(value).next().is_some()
}

fn open_cell_menu(h: &mut Harness) {
    h.state_mut().grid.context_menu = Some(egui::pos2(200.0, 300.0));
    h.run_steps(3);
}

fn open_filter_menu(h: &mut Harness) {
    // Column B has a blank cell, so the list offers the (Blanks) group beside the values.
    h.state_mut().grid.filter_menu = Some((1, 0, egui::pos2(300.0, 200.0)));
    h.run_steps(3);
}

#[test]
fn the_cell_context_menu_speaks_chinese() {
    let mut h = build(Language::Zh);
    open_cell_menu(&mut h);
    for label in ["剪切", "复制", "粘贴", "选择性粘贴…", "清除内容", "按所选单元格的值筛选", "设置单元格格式…", "链接…"]
    {
        assert!(has_label(&h, label), "the cell menu should show {label}");
    }
    assert!(!has_label(&h, "Cut"), "no English is left in the cell menu");
    assert!(!has_label(&h, "Clear Contents"), "no English is left in the cell menu");
}

#[test]
fn an_english_ui_keeps_the_cell_menu_english() {
    let mut h = build(Language::En);
    open_cell_menu(&mut h);
    assert!(has_label(&h, "Clear Contents"), "an English UI keeps the English menu");
    assert!(!has_label(&h, "清除内容"));
}

#[test]
fn the_filter_list_draws_the_blank_cells_entry_in_the_interface_language() {
    let mut h = build(Language::Zh);
    open_filter_menu(&mut h);
    assert!(has_label(&h, "（空白）"), "the blank-cells group reads Chinese");
    assert!(!has_label(&h, "(Blanks)"), "no English is left in the value list");
    // The values themselves are the cells' own text, which reads the same in both languages.
    assert!(has_label(&h, "3"), "a cell's text stays as it is");

    let mut h = build(Language::En);
    open_filter_menu(&mut h);
    assert!(has_label(&h, "(Blanks)"), "an English UI keeps the English word");
}

#[test]
fn the_status_bar_speaks_chinese() {
    let h = build(Language::Zh);
    assert!(has_value(&h, "就绪"), "the mode reads Chinese");
    assert!(!has_value(&h, "Ready"), "no English mode is left");
    assert!(has_label(&h, "辅助功能：2"), "the Accessibility entry that opens the check");

    let h = build(Language::En);
    assert!(has_value(&h, "Ready"));
    assert!(has_label(&h, "Accessibility: 2"));
}

#[test]
fn the_sheet_tabs_menu_speaks_chinese() {
    let mut h = build(Language::Zh);
    // The tab strip is the bottom-most node labelled by the sheet's name.
    let mut tabs: Vec<_> = h.query_all_by_label("Sheet1").collect();
    tabs.sort_by(|a, b| b.rect().top().total_cmp(&a.rect().top()));
    let tab = tabs.first().expect("the tab strip shows the sheet's tab");
    tab.click_secondary();
    h.run_steps(3);
    for label in ["插入工作表", "删除", "重命名", "移动或复制…", "复制", "保护工作表…", "隐藏", "选择所有工作表"] {
        assert!(has_label(&h, label), "the tab menu should show {label}");
    }
    assert!(h.query_all_by_label_contains("标签颜色").next().is_some(), "the Tab Color sub-menu");
    assert!(!has_label(&h, "Insert Sheet"), "no English is left in the tab menu");
}

#[test]
fn a_ribbon_tooltip_names_the_command_and_its_shortcut_in_chinese() {
    // Hovering a painted button in a test is timing-dependent, so the tooltip text is read from
    // the same function the ribbon's buttons call, in a frame drawn in each language.
    let keys = gridcraft_ui_egui::widgets::tooltip_shortcut("Paste", cfg!(target_os = "macos")).expect("Paste has a shortcut");
    assert_eq!(tooltip(Language::En, "Paste"), format!("Paste ({keys})"));
    // The brackets are the language's own: Chinese draws them full width.
    assert_eq!(tooltip(Language::Zh, "Paste"), format!("粘贴（{keys}）"));
    // A command without a shortcut shows its name alone.
    assert_eq!(tooltip(Language::Zh, "Borders"), "边框");
}

/// The ribbon tooltip text for a frame drawn in `language`.
fn tooltip(language: Language, tip: &str) -> String {
    let ctx = egui::Context::default();
    let mut text = String::new();
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
        gridcraft_ui_egui::i18n::set_current(ui.ctx(), language);
        text = gridcraft_ui_egui::widgets::tooltip_text(ui, tip);
    });
    out.textures_delta.clear(); // nothing consumes the atlas in a headless test
    text
}
