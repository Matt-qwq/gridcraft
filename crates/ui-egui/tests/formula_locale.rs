//! Language selection must affect UI entry, never the canonical engine/file boundary. Both
//! shipped interface languages (English, Simplified Chinese) use the canonical English formula
//! syntax, so a switch never changes what a formula means or what the file stores.

use gridcraft_engine::{
    Session,
    core::{CellError, CellRef, Value},
};
use gridcraft_ui_egui::{SheetApp, UiState, i18n::Language};
use serde_json::json;

fn app() -> SheetApp {
    let mut session = Session::new();
    session.new_workbook();
    let mut app = SheetApp::new(session, Default::default());
    app.ui.language = Language::En;
    app
}
fn formula(app: &SheetApp) -> String {
    app.session.active().unwrap().wb.active().unwrap().cell(CellRef::new(0, 0)).unwrap().formula.as_ref().unwrap().text.to_string()
}
fn enter(app: &mut SheetApp, text: &str) -> bool {
    app.begin_edit(Some(text.into()), false);
    app.commit_edit(0, 0, false, false)
}

#[test]
fn the_interface_language_never_changes_the_formula_syntax() {
    // The Chinese interface translates labels only: function names, `,` separators and `.`
    // decimals stay canonical, and a decimal-comma formula is not a formula.
    let mut app = app();
    app.run("app.language.set", json!({"language":"zh"})).unwrap();
    assert_eq!(app.ui.language, Language::Zh);
    assert_eq!(app.ui.language.tr("Data"), "数据");
    assert!(enter(&mut app, "=SUM(1.5,2.25)"));
    assert_eq!(formula(&app), "SUM(1.5,2.25)");
    let saved = app.run("file.save", json!({"format":"xlsx"})).unwrap();
    app.run("file.open", json!({"name":"locale.xlsx", "base64":saved["base64"]})).unwrap();
    app.begin_edit(None, true);
    assert_eq!(app.editor.as_ref().unwrap().text, "=SUM(1.5,2.25)");
    assert!(app.commit_edit(0, 0, false, false));
    assert_eq!(formula(&app), "SUM(1.5,2.25)");
    let state: UiState = serde_json::from_str(&serde_json::to_string(&app.ui).unwrap()).unwrap();
    assert_eq!(state.language, Language::Zh);
    app.run("app.language.set", json!({"language":"en"})).unwrap();
    app.begin_edit(None, false);
    assert_eq!(app.editor.as_ref().unwrap().text, "=SUM(1.5,2.25)");
}

#[test]
fn language_does_not_change_api_syntax_or_an_edit_already_in_progress() {
    let mut app = app();
    app.run("app.language.set", json!({"language":"zh"})).unwrap();
    assert!(app.run("cell.set", json!({"cell":"A1","input":"=SUM(1,2)"})).is_ok());
    app.begin_edit(Some("=SUM(1.5,2)".into()), false);
    app.run("app.language.set", json!({"language":"en"})).unwrap();
    assert!(app.commit_edit(0, 0, false, false));
    assert_eq!(formula(&app), "SUM(1.5,2)");
    // An engine-generated template is canonical in both languages.
    app.run("formulas.insertFunction", json!({"name":"SUM"})).unwrap();
    assert_eq!(app.editor.as_ref().unwrap().text, "=SUM(");
}

#[test]
fn defined_and_local_names_survive_a_language_switch() {
    let mut app = app();
    app.run("formulas.defineName", json!({"name":"TOTAL", "refersTo":"=7"})).unwrap();
    app.run("app.language.set", json!({"language":"zh"})).unwrap();
    assert!(app.run("cell.set", json!({"cell":"A1","input":"=TOTAL+1"})).is_ok());
    app.begin_edit(None, false);
    assert_eq!(app.editor.as_ref().unwrap().text, "=TOTAL+1");
    assert!(app.commit_edit(0, 0, false, false));
    assert_eq!(app.session.active().unwrap().wb.active().unwrap().value(CellRef::new(0, 0)), Value::Number(8.0));
}

#[test]
fn chinese_interface_retains_its_canonical_formula_fallback() {
    // The Chinese table translates interface labels and function help only: formulas keep the
    // canonical English names, `,` separators and `.` decimals.
    let mut app = app();
    app.run("app.language.set", json!({"code":"zh-CN"})).unwrap();
    assert_eq!(app.ui.language, Language::Zh);
    assert_eq!(app.ui.language.tr("Formulas"), "公式");
    assert!(enter(&mut app, "=SUM(1.5,2.25)"));
    app.begin_edit(None, true);
    assert_eq!(app.editor.as_ref().unwrap().text, "=SUM(1.5,2.25)");
    let restored: UiState = serde_json::from_str(&serde_json::to_string(&app.ui).unwrap()).unwrap();
    assert_eq!(restored.language, Language::Zh);
}

#[test]
fn a_language_this_build_no_longer_ships_is_refused_and_keeps_the_interface() {
    let mut app = app();
    for removed in ["es", "de", "pt-BR", "cs-CZ", "tr-TR"] {
        assert!(app.run("app.language.set", json!({"code": removed})).is_err(), "{removed} is not a language this build ships");
        assert_eq!(app.ui.language, Language::En);
    }
    // An edit already in progress keeps the syntax it started with (#726): the switch never
    // reinterprets typed text.
    app.begin_edit(Some("=SUM(1.5,2)".into()), false);
    app.run("app.language.set", json!({"language":"zh"})).unwrap();
    assert!(app.commit_edit(0, 0, false, false));
    assert_eq!(formula(&app), "SUM(1.5,2)");
}

#[test]
fn user_defined_prefix_is_hidden_while_editing_and_kept_on_commit() {
    // Excel shows `_xludf.EOMONTH()` as `=EOMONTH()`; the cell still holds the user-defined call
    // (#726), and committing the edit unchanged must not turn it into the built-in.
    let mut app = app();
    app.run("cell.set", json!({"cell":"A1","input":"=_xludf.EOMONTH()&_xludf.CONCAT(\"_xludf.x\")"})).unwrap();
    app.begin_edit(None, true);
    assert_eq!(app.editor.as_ref().unwrap().text, "=EOMONTH()&CONCAT(\"_xludf.x\")");
    assert!(app.commit_edit(0, 0, false, false));
    assert_eq!(formula(&app), "_xludf.EOMONTH()&_xludf.CONCAT(\"_xludf.x\")");
    assert_eq!(app.session.active().unwrap().wb.active().unwrap().value(CellRef::new(0, 0)), Value::Error(CellError::Name));
    app.run("app.language.set", json!({"language":"zh"})).unwrap();
    app.begin_edit(None, true);
    assert_eq!(app.editor.as_ref().unwrap().text, "=EOMONTH()&CONCAT(\"_xludf.x\")");
    assert!(app.commit_edit(0, 0, false, false));
    assert_eq!(formula(&app), "_xludf.EOMONTH()&_xludf.CONCAT(\"_xludf.x\")");
}
