//! Keyboard reachability of the context menus: Shift+F10 opens the cell menu at the selection,
//! arrows/Home/End move the highlight, Enter and Space activate, Escape closes, and the grid keeps
//! its selection and its keyboard while the menu is open. The row/column header menus navigate the
//! same way.
//!
//! The dedicated Menu key (VK_APPS) cannot be tested here: egui-winit drops `KeyCode::ContextMenu`
//! before egui sees it (there is no `egui::Key` variant), so Shift+F10 is the reachable pair. See
//! `docs/ui-parity.md` (keyboard audit).

use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2};
use gridcraft_engine::Session;
use gridcraft_engine::core::{CellRef, Value};
use gridcraft_ui_egui::{SheetApp, grid};
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

fn harness() -> Harness {
    let mut session = Session::new();
    session.new_workbook();
    session.execute("range.setValues", json!({"range": "A1", "values": [[1, 2], [3, 4], [5, 6]]})).unwrap();
    session.execute("selection.set", json!({"range": "B3"})).unwrap();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_ui_state(
        |ui, app: &mut SheetApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        SheetApp::new(session, Default::default()),
    );
    h.run_steps(4);
    h
}

fn key(h: &mut Harness, k: Key, m: Modifiers) {
    h.input_mut().events.push(Event::Key { key: k, physical_key: Some(k), pressed: true, repeat: false, modifiers: m });
    h.input_mut().events.push(Event::Key { key: k, physical_key: Some(k), pressed: false, repeat: false, modifiers: m });
    h.run_steps(2);
}

fn key_with_text(h: &mut Harness, k: Key, m: Modifiers, text: &str) {
    h.input_mut().events.push(Event::Key { key: k, physical_key: Some(k), pressed: true, repeat: false, modifiers: m });
    h.input_mut().events.push(Event::Text(text.to_string()));
    h.input_mut().events.push(Event::Key { key: k, physical_key: Some(k), pressed: false, repeat: false, modifiers: m });
    h.run_steps(2);
}

fn cell_center(h: &Harness, cell: &str) -> Pos2 {
    let app = h.state();
    let sh = app.session.active().unwrap().wb.active().unwrap();
    grid::Geo::new(sh, app.grid.rect.unwrap(), app.view().scroll).cell_rect(sh, CellRef::parse(cell).unwrap()).center()
}

fn active(h: &Harness) -> String {
    h.state().session.active().unwrap().selection.current().a1()
}

fn value(h: &Harness, cell: &str) -> Value {
    h.state().session.active().unwrap().wb.active().unwrap().value(CellRef::parse(cell).unwrap())
}

fn right_click_row_header(h: &mut Harness, cell: &str) {
    let app = h.state();
    let sh = app.session.active().unwrap().wb.active().unwrap();
    let geo = grid::Geo::new(sh, app.grid.rect.unwrap(), app.view().scroll);
    let y = geo.cell_rect(sh, CellRef::parse(cell).unwrap()).center().y;
    let pos = pos2((geo.rect.left() + geo.cells.left()) / 2.0, y);
    h.input_mut().events.push(Event::PointerMoved(pos));
    h.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Secondary, pressed: true, modifiers: Modifiers::NONE });
    h.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Secondary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

fn right_click_col_header(h: &mut Harness, cell: &str) {
    let app = h.state();
    let sh = app.session.active().unwrap().wb.active().unwrap();
    let geo = grid::Geo::new(sh, app.grid.rect.unwrap(), app.view().scroll);
    let x = geo.cell_rect(sh, CellRef::parse(cell).unwrap()).center().x;
    let pos = pos2(x, (geo.rect.top() + geo.cells.top()) / 2.0);
    h.input_mut().events.push(Event::PointerMoved(pos));
    h.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Secondary, pressed: true, modifiers: Modifiers::NONE });
    h.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Secondary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

#[test]
fn shift_f10_opens_the_cell_menu_at_the_selection() {
    let mut h = harness();
    key(&mut h, Key::F10, Modifiers::SHIFT);
    assert_eq!(h.state().grid.context_menu, Some(cell_center(&h, "B3")), "the menu opens at the active cell");
    assert_eq!(active(&h), "B3", "Shift+F10 does not move the selection");
    assert!(h.state().editor.is_none());
}

#[test]
fn arrows_move_the_highlight_not_the_selection() {
    let mut h = harness();
    key(&mut h, Key::F10, Modifiers::SHIFT);
    assert!(h.state().grid.menu_nav.is_none(), "a fresh menu starts unhighlighted, as a mouse-opened one");
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(h.state().grid.menu_nav.map(|n| n.selected), Some(0), "the first ArrowDown highlights the first item");
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(h.state().grid.menu_nav.map(|n| n.selected), Some(1));
    key(&mut h, Key::ArrowUp, Modifiers::NONE);
    assert_eq!(h.state().grid.menu_nav.map(|n| n.selected), Some(0));
    // Up from the first item wraps to the last, as in Excel.
    key(&mut h, Key::ArrowUp, Modifiers::NONE);
    assert_eq!(h.state().grid.menu_nav.map(|n| n.selected), Some(19), "the last item (Link…) is item 19");
    key(&mut h, Key::Home, Modifiers::NONE);
    assert_eq!(h.state().grid.menu_nav.map(|n| n.selected), Some(0));
    key(&mut h, Key::End, Modifiers::NONE);
    assert_eq!(h.state().grid.menu_nav.map(|n| n.selected), Some(19));
    assert_eq!(active(&h), "B3", "the arrows never reach the grid while a menu is open");
    assert!(h.state().editor.is_none());
}

#[test]
fn enter_runs_the_highlighted_item() {
    let mut h = harness();
    key(&mut h, Key::F10, Modifiers::SHIFT);
    // Actionable rows: Cut, Copy, Paste, Paste Special…, Insert…, Delete…, Clear Contents.
    for _ in 0..7 {
        key(&mut h, Key::ArrowDown, Modifiers::NONE);
    }
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(value(&h, "B3"), Value::Empty, "Enter ran Clear Contents");
    assert!(h.state().grid.context_menu.is_none(), "the menu closes after the command");
    assert!(h.state().grid.menu_nav.is_none(), "and forgets the highlight");
}

#[test]
fn space_activates_and_does_not_start_editing() {
    let mut h = harness();
    key(&mut h, Key::F10, Modifiers::SHIFT);
    for _ in 0..7 {
        key(&mut h, Key::ArrowDown, Modifiers::NONE);
    }
    // The OS sends the space as text as well as the key: neither may reach the grid.
    key_with_text(&mut h, Key::Space, Modifiers::NONE, " ");
    assert_eq!(value(&h, "B3"), Value::Empty, "Space ran Clear Contents");
    assert!(h.state().editor.is_none(), "Space did not start an edit behind the menu");
    assert!(h.state().grid.context_menu.is_none());
}

#[test]
fn enter_takes_the_first_item_when_nothing_is_highlighted() {
    let mut h = harness();
    key(&mut h, Key::F10, Modifiers::SHIFT);
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert!(h.state().grid.context_menu.is_none(), "Enter chose the first item (Cut)");
    assert!(h.state().editor.is_none());
}

#[test]
fn escape_closes_the_menu_and_leaves_the_cell_alone() {
    let mut h = harness();
    key(&mut h, Key::F10, Modifiers::SHIFT);
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    key(&mut h, Key::Escape, Modifiers::NONE);
    assert!(h.state().grid.context_menu.is_none(), "Escape closes the menu");
    assert_eq!(value(&h, "B3"), Value::Number(6.0), "and runs nothing");
    assert_eq!(active(&h), "B3");
}

#[test]
fn row_header_menu_answers_to_the_keyboard() {
    let mut h = harness();
    right_click_row_header(&mut h, "A3");
    assert!(h.state().grid.header_menu.is_some());
    // Rows: Cut, Copy, Paste, Insert, Delete, Clear Contents, Format Cells…, Row Height…,
    // AutoFit Row Height, Hide — the tenth actionable row.
    for _ in 0..10 {
        key(&mut h, Key::ArrowDown, Modifiers::NONE);
    }
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert!(h.state().grid.header_menu.is_none(), "the menu closes after the command");
    let d = h.state().session.active().unwrap();
    assert!(d.wb.active().unwrap().is_row_hidden(2), "Enter ran Hide on the clicked row");
}

#[test]
fn column_header_menu_answers_to_the_keyboard() {
    let mut h = harness();
    right_click_col_header(&mut h, "B3");
    assert!(h.state().grid.header_menu.is_some());
    // Columns: Cut, Copy, Paste, Insert, Delete, Clear Contents, Format Cells…, Column Width…,
    // AutoFit Column Width, Hide — the tenth actionable row (Unhide and Sort follow it).
    for _ in 0..10 {
        key(&mut h, Key::ArrowDown, Modifiers::NONE);
    }
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert!(h.state().grid.header_menu.is_none());
    let d = h.state().session.active().unwrap();
    assert!(d.wb.active().unwrap().is_col_hidden(1), "Enter ran Hide on the clicked column");
}

#[test]
fn a_keyboard_activated_menu_item_runs_once() {
    // The item runs after the frame's menu was drawn: no double execution, no leftover highlight.
    let mut h = harness();
    key(&mut h, Key::F10, Modifiers::SHIFT);
    for _ in 0..7 {
        key(&mut h, Key::ArrowDown, Modifiers::NONE);
    }
    key(&mut h, Key::Enter, Modifiers::NONE);
    h.run_steps(2);
    assert_eq!(value(&h, "B3"), Value::Empty);
    let clears = h.state().session.journal.iter().filter(|(id, _)| id == "edit.clearContents").count();
    assert_eq!(clears, 1, "exactly one journal entry");
}
