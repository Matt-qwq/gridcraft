//! The dialogs read the interface language: every caption, field label, choice and button the
//! dialog surfaces draw goes through the catalog, so a Chinese build shows Chinese there.
//!
//! Two halves: what a dialog renders (open it in a kittest and read its labels), and the one place
//! a dialog takes its words from the command registry more than its own source -- the command
//! palette's menu path ("Home › Editing › Find & Select"), whose group names live in the engine's
//! command specs and are drawn by `dialogs::command_search`.

use egui::vec2;
use egui_kittest::kittest::{NodeT, Queryable};
use gridcraft_engine::Session;
use gridcraft_ui_egui::SheetApp;
use gridcraft_ui_egui::control::{Outcome, handle};
use gridcraft_ui_egui::i18n::Language;
use serde_json::{Value as Json, json};

fn harness(language: Language) -> egui_kittest::Harness<'static, SheetApp> {
    let mut session = Session::new();
    session.new_workbook();
    session.execute("range.setValues", json!({"range": "A1:B4", "values": [["Name", "Value"], ["Ana", 1], ["Bia", 2], ["Caio", 3]]})).unwrap();
    session.execute("selection.set", json!({"range": "A1:B4"})).unwrap();
    let mut app = SheetApp::new(session, Default::default());
    app.ui.language = language;
    let mut h = egui_kittest::Harness::builder().with_size(vec2(1400.0, 900.0)).build_ui_state(
        |ui, app: &mut SheetApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(6);
    h
}

fn control(h: &mut egui_kittest::Harness<'static, SheetApp>, method: &str, params: Json) -> Json {
    let ctx = h.ctx.clone();
    match handle(h.state_mut(), &ctx, method, &params) {
        Outcome::Done(v) => v,
        Outcome::Screenshot { .. } => json!(null),
    }
}

/// Every label the frame draws, once each.
fn labels(h: &egui_kittest::Harness<'static, SheetApp>) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for node in h.query_all_by(|n| n.label().is_some()) {
        if let Some(label) = node.accesskit_node().label()
            && seen.insert(label.clone())
        {
            out.push(label);
        }
    }
    out
}

/// Opens a dialog and returns its labels.
fn dialog_labels(name: &str, params: Json, language: Language) -> Vec<String> {
    let mut h = harness(language);
    control(&mut h, "ui.dialog", {
        let mut p = params;
        if let Some(m) = p.as_object_mut() {
            m.insert("name".into(), json!(name));
        }
        p
    });
    h.run_steps(4);
    assert!(h.state().dialog.is_some(), "{name} opens");
    labels(&h)
}

/// The dialogs a Chinese build reads in Chinese, with a label only that dialog draws (the caption,
/// a field, a choice, a button) and the same for the English one.
fn samples() -> Vec<(&'static str, Json, &'static str, &'static str)> {
    vec![
        // A form's caption (the window title) and its widgets: a field label alone is not an
        // accessibility node of its own, so what is asserted here is what a reader can query.
        ("rowHeight", json!({}), "行高", "Row Height"),
        ("columnWidth", json!({}), "列宽", "Column Width"),
        ("renameSheet", json!({}), "重命名工作表", "Rename Sheet"),
        ("moveSheet", json!({}), "创建副本", "Create a copy"),
        ("protectSheet", json!({}), "选定锁定单元格", "Select locked cells"),
        ("allowEditRanges", json!({}), "删除密码（修改）", "Remove the password (Modify)"),
        ("pageSetup", json!({}), "页面设置", "Page Setup"),
        ("textToColumns", json!({}), "连续分隔符号视为单个处理", "Treat consecutive delimiters as one"),
        ("removeDuplicates", json!({}), "数据包含标题", "My data has headers"),
        ("advancedFilter", json!({}), "仅显示唯一行", "Unique rows only"),
        ("goToSpecial", json!({}), "仅可见单元格", "Visible cells only"),
        ("zoom", json!({}), "恰好容纳选定区域", "Fit selection"),
        ("defineName", json!({}), "新建名称", "New Name"),
        ("insertLink", json!({}), "插入超链接", "Insert Hyperlink"),
        ("sparkline", json!({}), "数据标记", "Markers"),
        ("subtotal", json!({}), "分类汇总", "Subtotal"),
        ("series", json!({}), "工作日", "Weekday"),
        ("insertPaste", json!({}), "插入粘贴", "Insert Paste"),
        ("goalSeek", json!({}), "单变量求解", "Goal Seek"),
        ("solver", json!({}), "使未约束变量为非负数", "Make Unconstrained Variables Non-Negative"),
        ("sort", json!({}), "列表包含标题", "My list has headers"),
        ("find", json!({}), "区分大小写", "Match case"),
        ("pasteSpecial", json!({}), "转置 (E)", "Transpose (E)"),
        ("saveChanges", json!({"title": "Book1.xlsx"}), "不保存", "Don't Save"),
        (
            "cfQuick",
            json!({"type": "cellIs", "operator": "greater", "title": "Greater Than…"}),
            "浅红填充色深红色文本",
            "Light Red Fill with Dark Red Text",
        ),
        ("dataValidation", json!({}), "单元格内下拉列表", "In-cell dropdown"),
        // Format Cells, which is its own body per tab: the fields a Chinese build used to read wrong
        // (a paper size where the type size goes, a group level where the outer border goes).
        ("formatCells", json!({"tab": "Border"}), "上框线 细线", "Top thin"),
        ("formatCells", json!({"tab": "Border"}), "外边框", "Outline"),
    ]
}

#[test]
fn the_dialogs_read_as_chinese_and_stay_english_in_english() {
    for (name, params, chinese, english) in samples() {
        let zh = dialog_labels(name, params.clone(), Language::Zh);
        assert!(zh.iter().any(|l| l == chinese), "{name} should show {chinese:?} in a Chinese build: {zh:?}");
        assert!(!zh.iter().any(|l| l == english), "{name} still shows {english:?} in a Chinese build");
        let en = dialog_labels(name, params.clone(), Language::En);
        assert!(en.iter().any(|l| l == english), "{name} should show {english:?} in an English build: {en:?}");
        assert!(!en.iter().any(|l| l == chinese), "{name} shows {chinese:?} in an English build");
    }
}

/// Format Cells' words that another surface already uses for something else: the catalog keys
/// them `Format Cells|…`, so the Font tab's size is a type size (字号), not Page Setup's paper
/// size (纸张大小), and the Border tab's sides are border sides (上框线), not cell alignment
/// (靠上). The value can be asserted only here: a slider's caption and a plain label are not
/// accessibility nodes of their own.
#[test]
fn format_cells_terms_are_keyed_by_their_context() {
    for (key, chinese, english) in [
        ("Format Cells|Size", "字号", "Size"),
        ("Format Cells|Sample", "示例", "Sample"),
        ("Format Cells|Outline", "外边框", "Outline"),
        ("Format Cells|Top", "上框线", "Top"),
        ("Format Cells|Bottom", "下框线", "Bottom"),
        ("Format Cells|Left", "左边框", "Left"),
        ("Format Cells|Right", "右边框", "Right"),
    ] {
        assert_eq!(Language::Zh.tr(key), chinese, "{key}");
        // English shows the label after the context prefix, never the key.
        assert_eq!(Language::En.tr(key), english, "{key}");
    }
}

/// The one dialog whose words come from the registry: the palette's menu path. A group name with no
/// entry turns "开始 › Editing › 查找和选择" back into English inside a Chinese dialog.
#[test]
fn every_command_palette_menu_segment_reads_chinese() {
    let mut untranslated = Vec::new();
    for spec in gridcraft_engine::cmd::command_specs() {
        for segment in spec.menu.iter().copied() {
            if Language::Zh.tr(segment) == segment {
                untranslated.push(segment);
            }
        }
    }
    untranslated.sort_unstable();
    untranslated.dedup();
    assert!(untranslated.is_empty(), "{} command menu group(s) have no Simplified Chinese: {untranslated:?}", untranslated.len());
}
