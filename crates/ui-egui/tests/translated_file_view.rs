//! The File view, the formula bar, the print dialog and the About credits read the interface
//! language. A Chinese build showed "Household Budget" on the New page forever: the template
//! tiles were drawn from the engine's English sample titles, never through `tr`.

use std::sync::mpsc::channel;

use egui_kittest::kittest::Queryable as _;
use gridcraft_engine::Session;
use gridcraft_ui_egui::{Services, SheetApp, backstage::Page, printing};
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

fn harness(services: Services) -> Harness {
    let mut session = Session::new();
    session.new_workbook();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1200.0, 900.0)).build_ui_state(
        |ui, app: &mut SheetApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        SheetApp::new(session, services),
    );
    h.run_steps(4);
    h
}

/// A language, set the way the Options page sets it.
fn speak(h: &mut Harness, code: &str) {
    h.state_mut().run("app.language.set", json!({"language": code})).unwrap();
    h.run_steps(3);
}

/// Every label drawn right now, as text (a tooltip is a label in its own layer).
fn drawn(h: &Harness) -> Vec<String> {
    fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(t) => out.push(t.galley.job.text.clone()),
            egui::Shape::Vec(v) => {
                for s in v {
                    walk(s, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for s in &h.output().shapes {
        walk(&s.shape, &mut out);
    }
    out
}

/// What a page shows in `translated`, and what its English twin must not be in. The text shapes
/// are read rather than the accessibility tree: a combo box's selected value (the print scope) and
/// a table header are drawn text, and one of them may be on screen twice.
fn shows(h: &Harness, translated: &[&str], english: &[&str]) {
    let text = drawn(h);
    for wanted in translated {
        assert!(text.iter().any(|t| t.contains(wanted)), "no {wanted:?} on screen: {text:?}");
    }
    for english in english {
        assert!(!text.iter().any(|t| t.contains(english)), "{english:?} is still English: {text:?}");
    }
}

/// The formula bar's buttons, found by hovering along the bar: the bar sits on top of the grid,
/// so grid_top - 15 is its middle.
fn formula_bar_tooltips(h: &mut Harness) -> Vec<String> {
    h.ctx.all_styles_mut(|s| s.interaction.tooltip_delay = 0.0);
    let Some(grid) = h.state().grid.rect else { return vec![] };
    // The row centres its buttons on the field, which is as tall as the bar (3px above and below).
    let bar = if h.state().ui.formula_bar_expanded { 72.0 } else { 30.0 };
    let y = grid.top() - bar + 3.0 + (bar - 6.0) / 2.0;
    let right = h.ctx.content_rect().right() as i32;
    let mut seen: Vec<String> = Vec::new();
    for x in (8..right - 8).step_by(2) {
        h.hover_at(egui::pos2(x as f32 + 1.0, y));
        h.run_steps(1);
        for text in drawn(h) {
            if !seen.contains(&text) {
                seen.push(text);
            }
        }
    }
    seen
}

#[test]
fn the_backstage_pages_speak_chinese() {
    // A caption of each page, and the English it replaces.
    let pages: [(Page, &str, &str); 10] = [
        (Page::Home, "空白工作簿", "Blank workbook"),
        (Page::Home, "家庭预算", "Household Budget"),
        (Page::New, "季度销售看板", "Quarterly Sales Dashboard"),
        (Page::Open, "浏览…", "Browse…"),
        (Page::Info, "属性", "Properties"),
        (Page::SaveAs, "保存类型", "Save as type"),
        (Page::Print, "打印活动工作表", "Print Active Sheets"),
        (Page::Export, "更改文件类型", "Change File Type"),
        (Page::Options, "界面语言", "Interface language"),
        (Page::Account, "产品信息", "Product Information"),
    ];
    for (page, chinese, english) in pages {
        let mut h = harness(Services::default());
        speak(&mut h, "zh");
        h.state_mut().open_backstage(page);
        h.run_steps(4);
        shows(&h, &[chinese], &[english]);
        // The same page reads English again when the interface is English.
        speak(&mut h, "en");
        h.run_steps(4);
        shows(&h, &[english], &[chinese]);
    }
}

#[test]
fn every_template_tile_is_named_in_the_interface_language() {
    for (english, chinese) in [("Household Budget", "家庭预算"), ("Quarterly Sales Dashboard", "季度销售看板"), ("Gradebook", "成绩表")]
    {
        let mut h = harness(Services::default());
        speak(&mut h, "zh");
        h.state_mut().open_backstage(Page::New);
        h.run_steps(4);
        shows(&h, &[chinese], &[english]);
        let mut h = harness(Services::default());
        h.state_mut().open_backstage(Page::New);
        h.run_steps(4);
        shows(&h, &[english], &[chinese]);
    }
}

#[test]
fn the_formula_bar_tooltips_speak_chinese() {
    let mut h = harness(Services::default());
    speak(&mut h, "zh");
    let tips = formula_bar_tooltips(&mut h);
    for chinese in ["取消", "输入", "插入函数", "展开编辑栏"] {
        assert!(tips.iter().any(|t| t.contains(chinese)), "no {chinese:?} in the formula bar's tooltips: {tips:?}");
    }
    for english in ["Cancel", "Enter", "Insert Function"] {
        assert!(!tips.iter().any(|t| t == english), "{english:?} is still English: {tips:?}");
    }
    // Open, the same button folds the bar away: its tip says so, in Chinese too.
    h.state_mut().ui.formula_bar_expanded = true;
    h.run_steps(3);
    let tips = formula_bar_tooltips(&mut h);
    assert!(tips.iter().any(|t| t.contains("折叠编辑栏")), "no {t:?} while the bar is open", t = tips);

    let mut h = harness(Services::default());
    let tips = formula_bar_tooltips(&mut h);
    for english in ["Cancel", "Enter", "Insert Function", "Expand formula bar"] {
        assert!(tips.iter().any(|t| t == english), "no {english:?} in the English build: {tips:?}");
    }
    h.state_mut().ui.formula_bar_expanded = true;
    h.run_steps(3);
    let tips = formula_bar_tooltips(&mut h);
    assert!(tips.iter().any(|t| t == "Collapse formula bar"), "no Collapse formula bar while open: {tips:?}");
}

#[test]
fn the_print_controls_speak_chinese() {
    let discover = || Services {
        printers: Some(Box::new(|| {
            let (send, receive) = channel();
            let _ = send.send(vec![printing::Printer { name: "Paper Maker".into(), default: true }]);
            receive
        })),
        ..Services::default()
    };
    let mut h = harness(discover());
    speak(&mut h, "zh");
    h.state_mut().open_backstage(Page::Print);
    h.run_steps(10);
    shows(
        &h,
        &["打印机", "份数", "双面打印", "灰度", "打印活动工作表", "将工作表调整为一页", "页面设置…"],
        &["Printer", "Copies", "Two-sided printing", "Print Active Sheets"],
    );
    // The printer's own name is the user's printer, not chrome: it stays as it is.
    assert!(drawn(&h).iter().any(|t| t == "Paper Maker"), "the chosen printer is not shown: {:?}", drawn(&h));

    let mut h = harness(discover());
    h.state_mut().open_backstage(Page::Print);
    h.run_steps(10);
    shows(&h, &["Printer", "Copies", "Print Active Sheets", "Fit Sheet on One Page"], &["打印机", "份数"]);

    // No printer at all: the notice under the controls is translated too.
    let none = || Services {
        printers: Some(Box::new(|| {
            let (send, receive) = channel();
            let _ = send.send(vec![]);
            receive
        })),
        ..Services::default()
    };
    let mut h = harness(none());
    speak(&mut h, "zh");
    h.state_mut().open_backstage(Page::Print);
    h.run_steps(10);
    shows(&h, &["未找到打印机，请配置系统打印机或导出为 PDF。"], &["No printers found. Configure a system printer or export a PDF."]);
}

#[test]
fn the_about_credits_speak_chinese() {
    let mut h = harness(Services::default());
    speak(&mut h, "zh");
    h.state_mut().open_dialog("about", json!({}));
    h.run_steps(4);
    shows(&h, &["贡献者"], &["Contributors"]);
    let Some(tab) = h.query_by_label_contains("贡献者") else { panic!("no Contributors tab") };
    tab.click();
    h.run_steps(4);
    // The credit tables: the name toggle, the sort menu and the count line.
    shows(&h, &["显示", "用户名", "显示名称", "真实姓名", "排序", "杂项", "表格"], &["Username", "Display name", "Real name", "Grab bag"]);
    assert!(drawn(&h).iter().any(|t| t.contains("位贡献者") && t.contains("次提交")), "the count line is not Chinese: {:?}", drawn(&h));

    let mut h = harness(Services::default());
    h.state_mut().open_dialog("about", json!({}));
    h.run_steps(4);
    let Some(tab) = h.query_by_label_contains("Contributors") else { panic!("no Contributors tab") };
    tab.click();
    h.run_steps(4);
    shows(&h, &["Username", "Grab bag"], &["用户名", "杂项"]);
}
