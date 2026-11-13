//! The AutoFilter dropdown speaks the UI language: a user reported its sort buttons, search box
//! and Clear/Apply row in English. They were translated; the report came from an older installed
//! build, so this pins the menu itself — open it and read its labels.

use egui::{Event, Modifiers, PointerButton};
use egui_kittest::kittest::Queryable;
use gridcraft_engine::Session;
use gridcraft_engine::core::CellRef;
use gridcraft_ui_egui::{SheetApp, grid};
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

fn open_filter_menu(language: gridcraft_ui_egui::i18n::Language) -> Harness {
    let mut session = Session::new();
    session.new_workbook();
    session.execute("range.setValues", json!({"range": "A1", "values": [["Name"], ["Ana"], ["Bia"], ["Caio"]]})).unwrap();
    session.execute("data.filter", json!({"range": "A1:A4"})).unwrap();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_ui_state(
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
    let button = {
        let app = h.state();
        let sh = app.session.active().unwrap().wb.active().unwrap();
        let geo = grid::Geo::new(sh, app.grid.rect.unwrap(), app.view().scroll);
        grid::filter_button_rect(&geo, sh, CellRef::parse("A1").unwrap()).center()
    };
    h.input_mut().events.push(Event::PointerMoved(button));
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos: button, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    h.run_steps(4);
    assert!(h.state().grid.filter_menu.is_some(), "the filter menu opens");
    h
}

#[test]
fn the_filter_menu_speaks_chinese() {
    let h = open_filter_menu(gridcraft_ui_egui::i18n::Language::Zh);
    for label in ["↑  升序排序", "↓  降序排序", "筛选", "（全选）", "清除筛选", "应用"] {
        assert!(h.query_by_label(label).is_some(), "the filter menu should show {label}");
    }
    assert!(h.query_by_label("Clear Filter").is_none(), "no English left in the menu");
}

#[test]
fn the_filter_menu_follows_an_english_ui() {
    let h = open_filter_menu(gridcraft_ui_egui::i18n::Language::En);
    assert!(h.query_by_label("Clear Filter").is_some(), "an English UI keeps the English menu");
}
