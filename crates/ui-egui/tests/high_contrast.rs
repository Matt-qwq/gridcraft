//! View ▸ Show ▸ High Contrast: the switch changes the chrome palette and survives a restart.

use egui::Color32;
use egui::vec2;
use gridcraft_engine::Session;
use gridcraft_ui_egui::SheetApp;
use gridcraft_ui_egui::theme::Tokens;
use serde_json::json;

use egui_kittest::kittest::Queryable as _;

type Harness = egui_kittest::Harness<'static, SheetApp>;

fn harness() -> Harness {
    let mut session = Session::new();
    session.new_workbook();
    egui_kittest::Harness::builder().with_size(vec2(1200.0, 800.0)).build_ui_state(
        |ui, app: &mut SheetApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        SheetApp::new(session, Default::default()),
    )
}

#[test]
fn the_switch_turns_the_window_high_contrast() {
    let mut h = harness();
    h.run_steps(3);
    assert!(!h.state().ui.high_contrast, "off by default");

    // The command is the switch (the View tab checkbox and its keytip call it).
    h.state_mut().run("view.highContrast", json!({})).unwrap();
    assert!(h.state().ui.high_contrast, "on after the command");
    h.run_steps(2);
    // The palette the frame publishes is the high-contrast one.
    let tokens = Tokens::get(&h.ctx);
    assert_eq!(tokens.window, Color32::BLACK, "black chrome");
    assert_eq!(tokens.text, Color32::WHITE, "white text");

    // Turning it off goes back to the ordinary palette.
    h.state_mut().run("view.highContrast", json!({"on": false})).unwrap();
    assert!(!h.state().ui.high_contrast);
    h.run_steps(2);
    assert_ne!(Tokens::get(&h.ctx).window, Color32::BLACK, "the ordinary chrome is back");
}

#[test]
fn the_view_tab_checkbox_is_the_same_switch() {
    let mut h = harness();
    h.state_mut().run("ui.ribbonTab", json!({"tab": "View"})).unwrap();
    h.run_steps(3);
    let check = h.get_by_label("High Contrast").rect();
    for pressed in [true, false] {
        h.input_mut().events.push(egui::Event::PointerButton {
            pos: check.center(),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        h.run_steps(2);
    }
    assert!(h.state().ui.high_contrast, "the checkbox turned it on");
}
