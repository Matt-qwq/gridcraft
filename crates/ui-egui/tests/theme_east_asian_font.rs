//! A workbook's theme names its East Asian typeface separately from its Latin one, and Excel draws
//! the Chinese in a Calibri cell with the former: the report behind this test was a workbook whose
//! `styles.xml` says Calibri with `<scheme val="minor"/>` and whose `theme1.xml` says
//! `<a:font script="Hans" typeface="宋体"/>` — WPS showed 宋体, GridCraft showed another face.

use std::sync::Arc;

use gridcraft_engine::Session;
use gridcraft_ui_egui::SheetApp;

/// A workbook whose body font follows the theme, as a Chinese Excel file's does.
fn chinese_theme_session() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    let d = s.doc_mut().expect("a document");
    let wb = Arc::make_mut(&mut d.wb);
    wb.theme.minor_font = "Calibri".into();
    wb.theme.minor_font_scripts = vec![("Jpan".into(), "游ゴシック".into()), ("Hans".into(), "宋体".into())];
    s
}

#[test]
fn the_theme_east_asian_face_is_the_one_the_app_binds() {
    let mut app = SheetApp::new(chinese_theme_session(), Default::default());
    app.ui.language = gridcraft_ui_egui::i18n::Language::Zh;
    assert_eq!(app.ea_fonts(), ["宋体"], "the theme's script=\"Hans\" typeface");
    let used = app.used_fonts();
    assert!(used.contains(&"宋体".to_string()), "bound with the workbook's fonts: {used:?}");
    assert!(used.contains(&"Calibri".to_string()), "the Latin face is still bound: {used:?}");
}

#[test]
fn every_shipped_language_reads_the_hans_entry() {
    // Both interface languages read Simplified Chinese glyph shapes; only a Japanese interface
    // (which no longer ships) would have asked for the Jpan entry.
    let mut app = SheetApp::new(chinese_theme_session(), Default::default());
    for language in gridcraft_ui_egui::i18n::Language::ALL {
        app.ui.language = language;
        assert_eq!(app.ea_fonts(), ["宋体"], "{language:?} reads the theme's Hans entry");
    }
}

#[test]
fn a_workbook_that_names_no_east_asian_face_binds_none() {
    let mut s = Session::new();
    s.new_workbook();
    let mut app = SheetApp::new(s, Default::default());
    app.ui.language = gridcraft_ui_egui::i18n::Language::Zh;
    assert!(app.ea_fonts().is_empty(), "GridCraft's own theme names none");
}
