//! Resizing a chart by dragging its bottom-right corner or with the Format Chart Area Size fields, through real input.

use egui::{Event, Key, Modifiers, PointerButton, accesskit::Role, pos2, vec2};
use egui_kittest::kittest::Queryable as _;
use gridcraft_engine::Session;
use gridcraft_engine::core::CellRef;
use gridcraft_ui_egui::{SheetApp, grid::Geo};
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

fn fixture() -> (Harness, u32) {
    let mut session = Session::new();
    session.new_workbook();
    session.execute("cell.set", json!({"cell": "A1", "input": "1"})).unwrap();
    session.execute("cell.set", json!({"cell": "A2", "input": "2"})).unwrap();
    session.execute("selection.set", json!({"range": "A1:A2"})).unwrap();
    let chart = session.execute("insert.chart", json!({"type": "column", "at": "D3", "width": 300, "height": 200})).unwrap();
    let id = chart["chart"].as_u64().unwrap() as u32;
    let mut h = egui_kittest::Harness::builder().with_size(vec2(1200.0, 800.0)).build_ui_state(
        |ui, app: &mut SheetApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        SheetApp::new(session, Default::default()),
    );
    h.run_steps(4);
    (h, id)
}

fn size(h: &Harness, id: u32) -> (f32, f32) {
    let a = h.state().session.doc().unwrap().wb.active().unwrap().charts.iter().find(|c| c.id == id).unwrap().anchor;
    (a.width, a.height)
}

fn corner(h: &Harness, id: u32) -> egui::Pos2 {
    let app = h.state();
    let sh = app.session.doc().unwrap().wb.active().unwrap();
    let geo = Geo::new(sh, app.grid.rect.unwrap(), app.view().scroll);
    let a = sh.charts.iter().find(|c| c.id == id).unwrap().anchor;
    let min = geo.cell_rect(sh, CellRef::parse("D3").unwrap()).min + vec2(a.dx, a.dy) * geo.z;
    min + vec2(a.width, a.height) * geo.z
}

fn button(h: &mut Harness, pos: egui::Pos2, pressed: bool) {
    h.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

/// The chart's frame in sheet points: left, top, width, height.
fn frame(h: &Harness, id: u32) -> (f64, f64, f32, f32) {
    let app = h.state();
    let sh = app.session.doc().unwrap().wb.active().unwrap();
    let a = sh.charts.iter().find(|c| c.id == id).unwrap().anchor;
    let f = gridcraft_ui_egui::chartview::frame_of(sh, &a);
    (f.x, f.y, f.w, f.h)
}

/// Where a handle sits on screen.
fn handle_pos(h: &Harness, id: u32, want: gridcraft_ui_egui::chartview::Handle) -> egui::Pos2 {
    use gridcraft_ui_egui::chartview::{Handle::*, handle_centres};
    let app = h.state();
    let sh = app.session.doc().unwrap().wb.active().unwrap();
    let geo = Geo::new(sh, app.grid.rect.unwrap(), app.view().scroll);
    let a = sh.charts.iter().find(|c| c.id == id).unwrap().anchor;
    let min = geo.cell_rect(sh, CellRef::parse("D3").unwrap()).min + vec2(a.dx, a.dy) * geo.z;
    let rect = egui::Rect::from_min_size(min, vec2(a.width, a.height) * geo.z);
    let all = handle_centres(rect);
    match want {
        Nw => all[0],
        N => all[1],
        Ne => all[2],
        E => all[3],
        Se => all[4],
        S => all[5],
        Sw => all[6],
        W => all[7],
    }
}

fn select_chart(h: &mut Harness, id: u32) {
    let inside = handle_pos(h, id, gridcraft_ui_egui::chartview::Handle::Nw) + vec2(40.0, 40.0);
    h.input_mut().events.push(Event::PointerMoved(inside));
    h.run_steps(2);
    button(h, inside, true);
    button(h, inside, false);
    assert_eq!(h.state().selected_chart, Some(id));
}

/// Drags `handle` of the selected chart by `delta` screen points, in `moves` steps, and releases.
fn drag(h: &mut Harness, id: u32, handle: gridcraft_ui_egui::chartview::Handle, delta: egui::Vec2, moves: usize, modifiers: Modifiers) {
    let start = handle_pos(h, id, handle);
    h.input_mut().events.push(Event::PointerMoved(start));
    h.run_steps(2);
    button(h, start, true);
    if !modifiers.is_none() {
        h.input_mut().events.push(Event::ModifiersChanged(modifiers));
    }
    for i in 1..=moves {
        let at = start + delta * (i as f32 / moves as f32);
        h.input_mut().events.push(Event::PointerMoved(at));
        h.run_steps(1);
    }
    let end = start + delta;
    h.input_mut().events.push(Event::PointerMoved(end));
    h.run_steps(2);
    button(h, end, false);
    if !modifiers.is_none() {
        h.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
        h.run_steps(1);
    }
}

#[test]
fn every_handle_resizes_the_edges_it_grabs() {
    use gridcraft_ui_egui::chartview::Handle::*;
    // (handle, left moves, top moves, right moves, bottom moves) as seen in logical sheet space.
    for (handle, l, t, r, b) in [
        (Nw, true, true, false, false),
        (N, false, true, false, false),
        (Ne, false, true, true, false),
        (E, false, false, true, false),
        (Se, false, false, true, true),
        (S, false, false, false, true),
        (Sw, true, false, false, true),
        (W, true, false, false, false),
    ] {
        let (mut h, id) = fixture();
        select_chart(&mut h, id);
        let (x0, y0, w0, h0) = frame(&h, id);
        let z = {
            let app = h.state();
            let sh = app.session.doc().unwrap().wb.active().unwrap();
            Geo::new(sh, app.grid.rect.unwrap(), app.view().scroll).z
        };
        let (dx, dy) = (40.0 / z, 20.0 / z);
        drag(&mut h, id, handle, vec2(40.0, 20.0), 4, Modifiers::NONE);
        let (x1, y1, w1, h1) = frame(&h, id);
        let near = |a: f64, b: f64| (a - b).abs() < 1.5;
        assert!(near(x1, x0 + if l { f64::from(dx) } else { 0.0 }), "{handle:?}: left edge {x0} -> {x1}");
        assert!(near(y1, y0 + if t { f64::from(dy) } else { 0.0 }), "{handle:?}: top edge {y0} -> {y1}");
        assert!(near(x1 + f64::from(w1), x0 + f64::from(w0) + if r { f64::from(dx) } else { 0.0 }), "{handle:?}: right edge");
        assert!(near(y1 + f64::from(h1), y0 + f64::from(h0) + if b { f64::from(dy) } else { 0.0 }), "{handle:?}: bottom edge");
        if !l && !r {
            assert!(near(f64::from(w1), f64::from(w0)), "{handle:?}: the width stays {w0} -> {w1}");
        }
        if !t && !b {
            assert!(near(f64::from(h1), f64::from(h0)), "{handle:?}: the height stays {h0} -> {h1}");
        }
    }
}

#[test]
fn a_drag_is_one_undo_step() {
    let (mut h, id) = fixture();
    select_chart(&mut h, id);
    let before = frame(&h, id);
    let undo0 = h.state().session.active().map(|d| d.undo.len()).unwrap_or(0);
    drag(&mut h, id, gridcraft_ui_egui::chartview::Handle::Se, vec2(120.0, 60.0), 6, Modifiers::NONE);
    let after = frame(&h, id);
    assert!(after.2 > before.2 && after.3 > before.3, "the chart grew {before:?} -> {after:?}");
    assert_eq!(h.state().session.active().map(|d| d.undo.len()), Some(undo0 + 1), "one drag, one undo entry");
    let _ = h.state_mut().session.run("edit.undo", json!({}));
    h.run_steps(2);
    assert_eq!(frame(&h, id), before, "one undo puts the size back");
}

#[test]
fn shift_keeps_the_aspect_ratio() {
    let (mut h, id) = fixture();
    select_chart(&mut h, id);
    let (_, _, w0, h0) = frame(&h, id);
    let ratio = w0 / h0;
    // A wide, flat drag: without Shift the height would not follow.
    drag(&mut h, id, gridcraft_ui_egui::chartview::Handle::Se, vec2(120.0, 0.0), 4, Modifiers::SHIFT);
    let (_, _, w1, h1) = frame(&h, id);
    assert!(w1 > w0 + 50.0, "the width followed the pointer {w0} -> {w1}");
    assert!((w1 / h1 - ratio).abs() < 0.01, "the shape is kept: {ratio} -> {}", w1 / h1);
}

#[test]
fn a_chart_never_shrinks_past_the_minimum_size() {
    let (mut h, id) = fixture();
    select_chart(&mut h, id);
    let (x0, y0, _, _) = frame(&h, id);
    drag(&mut h, id, gridcraft_ui_egui::chartview::Handle::Se, vec2(-500.0, -400.0), 5, Modifiers::NONE);
    let (x1, y1, w1, h1) = frame(&h, id);
    assert_eq!((w1, h1), (gridcraft_ui_egui::chartview::OBJECT_MIN_SIZE, gridcraft_ui_egui::chartview::OBJECT_MIN_SIZE));
    assert_eq!((x1, y1), (x0, y0), "the top-left corner stays put");
    // The opposite handles shrink towards the top-left too.
    let (mut h, id) = fixture();
    select_chart(&mut h, id);
    drag(&mut h, id, gridcraft_ui_egui::chartview::Handle::Nw, vec2(500.0, 400.0), 5, Modifiers::NONE);
    let (_, _, w2, h2) = frame(&h, id);
    assert_eq!((w2, h2), (gridcraft_ui_egui::chartview::OBJECT_MIN_SIZE, gridcraft_ui_egui::chartview::OBJECT_MIN_SIZE));
}

#[test]
fn escape_cancels_a_drag() {
    let (mut h, id) = fixture();
    select_chart(&mut h, id);
    let before = frame(&h, id);
    let undo0 = h.state().session.active().map(|d| d.undo.len()).unwrap_or(0);
    let start = handle_pos(&h, id, gridcraft_ui_egui::chartview::Handle::Se);
    h.input_mut().events.push(Event::PointerMoved(start));
    h.run_steps(2);
    button(&mut h, start, true);
    let end = start + vec2(150.0, 80.0);
    h.input_mut().events.push(Event::PointerMoved(end));
    h.run_steps(2);
    assert_ne!(frame(&h, id), before, "the drag resized the chart while it ran");
    h.input_mut().events.push(Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.input_mut().events.push(Event::Key { key: Key::Escape, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    assert_eq!(frame(&h, id), before, "Esc puts the chart back");
    assert_eq!(h.state().session.active().map(|d| d.undo.len()), Some(undo0), "the cancelled drag leaves no undo step");
    // The release after Esc must not resize it again.
    button(&mut h, end, false);
    assert_eq!(frame(&h, id), before);
}

#[test]
fn arrow_keys_move_and_resize_the_selected_chart() {
    let (mut h, id) = fixture();
    select_chart(&mut h, id);
    let (x0, y0, w0, h0) = frame(&h, id);
    let key = |h: &mut Harness, k: Key, m: Modifiers| {
        h.input_mut().events.push(Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m });
        h.input_mut().events.push(Event::Key { key: k, physical_key: None, pressed: false, repeat: false, modifiers: m });
        h.run_steps(2);
    };
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    let (x1, y1, w1, h1) = frame(&h, id);
    assert!(x1 > x0 + 5.0 && y1 > y0 + 5.0, "the arrows moved the chart by cells? {x0},{y0} -> {x1},{y1}");
    assert_eq!((w1, h1), (w0, h0), "moving does not resize");
    // The cell cursor stays where it was: the object took the keys.
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    assert_eq!(h.state().session.active().map(|d| d.selection.active.a1()).as_deref(), Some("A1"));
    key(&mut h, Key::ArrowRight, Modifiers::SHIFT);
    let (_, _, w2, _) = frame(&h, id);
    assert!(w2 > w1 + 5.0, "Shift+arrow grows the width {w1} -> {w2}");
    key(&mut h, Key::Escape, Modifiers::NONE);
    assert_eq!(h.state().selected_chart, None, "Esc drops the object selection");
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    assert_eq!(h.state().session.active().map(|d| d.selection.active.a1()).as_deref(), Some("B1"), "the arrows move cells again");
}

#[test]
fn a_resized_chart_keeps_its_size_across_save_and_reopen() {
    let (mut h, id) = fixture();
    select_chart(&mut h, id);
    drag(&mut h, id, gridcraft_ui_egui::chartview::Handle::Se, vec2(90.0, 45.0), 3, Modifiers::NONE);
    let (x, y, w, hh) = frame(&h, id);
    let bytes = h.state_mut().session.run("file.saveBytes", json!({"format": "xlsx"})).unwrap_or_default();
    let b64 = bytes.get("base64").and_then(serde_json::Value::as_str).unwrap_or_default().to_string();
    assert!(!b64.is_empty(), "the workbook saved");
    let mut s = Session::new();
    s.execute("file.open", json!({"name": "resized.xlsx", "base64": b64})).unwrap();
    let sh = s.doc().unwrap().wb.active().unwrap().clone();
    let a = sh.charts.iter().find(|c| c.id == id).map(|c| c.anchor).expect("the chart came back");
    let f = gridcraft_ui_egui::chartview::frame_of(&sh, &a);
    assert!((f.w - w).abs() < 0.5 && (f.h - hh).abs() < 0.5, "size {w}x{hh} came back as {}x{}", f.w, f.h);
    assert!((f.x - x).abs() < 0.5 && (f.y - y).abs() < 0.5, "position {x},{y} came back as {},{} ", f.x, f.y);
}

#[test]
fn dragging_the_corner_handle_resizes_the_chart() {
    let (mut h, id) = fixture();
    let (w0, h0) = size(&h, id);
    let start = corner(&h, id);
    // Select the chart first, as the user does before grabbing a handle.
    let inside = start - vec2(40.0, 40.0);
    h.input_mut().events.push(Event::PointerMoved(inside));
    h.run_steps(2);
    button(&mut h, inside, true);
    button(&mut h, inside, false);
    assert_eq!(h.state().selected_chart, Some(id));

    h.input_mut().events.push(Event::PointerMoved(start));
    h.run_steps(2);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ResizeNwSe, "the corner shows a resize cursor");
    button(&mut h, start, true);
    // The pointer moves well past the drag threshold in one step, like a quick drag.
    let end = start + vec2(60.0, 30.0);
    h.input_mut().events.push(Event::PointerMoved(pos2(end.x, end.y)));
    h.run_steps(3);
    button(&mut h, end, false);
    let (w1, h1) = size(&h, id);
    assert!(w1 > w0 + 40.0 && h1 > h0 + 20.0, "chart grew from {w0}x{h0} to {w1}x{h1}");
}

/// Opens Format Chart Area for the chart and expands its Size section; returns the Width field's rect.
fn open_size_fields(h: &mut Harness, id: u32) -> egui::Rect {
    h.state_mut().selected_chart = Some(id);
    h.state_mut().grid.pane = Some("formatChart".into());
    h.run_steps(2);
    h.get_by_role_and_label(Role::Button, "Size").click();
    h.run_steps(2);
    h.get_all_by_role(Role::SpinButton).next().unwrap().rect()
}

#[test]
fn typing_a_width_in_the_size_field_resizes_the_chart() {
    let (mut h, id) = fixture();
    let (_, h0) = size(&h, id);
    let field = open_size_fields(&mut h, id);
    h.input_mut().events.push(Event::PointerMoved(field.center()));
    button(&mut h, field.center(), true);
    button(&mut h, field.center(), false);
    // Clicking the field selects its text; each keystroke must survive the next frame.
    for digit in ["6", "4", "0"] {
        h.input_mut().events.push(Event::Text(digit.into()));
        h.run_steps(2);
    }
    h.key_press(Key::Enter);
    h.run_steps(2);
    assert_eq!(size(&h, id), (640.0, h0));
}

#[test]
fn dragging_the_size_field_resizes_the_chart() {
    let (mut h, id) = fixture();
    let (w0, _) = size(&h, id);
    let field = open_size_fields(&mut h, id);
    h.input_mut().events.push(Event::PointerMoved(field.center()));
    button(&mut h, field.center(), true);
    for step in 1..=6 {
        h.input_mut().events.push(Event::PointerMoved(field.center() + vec2(10.0 * step as f32, 0.0)));
        h.run_steps(1);
    }
    button(&mut h, field.center() + vec2(60.0, 0.0), false);
    let (w1, _) = size(&h, id);
    assert!(w1 > w0 + 20.0, "width went from {w0} to {w1}");
}

fn right_click(h: &mut Harness, pos: egui::Pos2, wobble: egui::Vec2) {
    h.input_mut().events.push(Event::PointerMoved(pos));
    h.run_steps(2);
    h.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Secondary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(2);
    let end = pos + wobble;
    if wobble != vec2(0.0, 0.0) {
        h.input_mut().events.push(Event::PointerMoved(end));
        h.run_steps(2);
    }
    h.input_mut().events.push(Event::PointerButton { pos: end, button: PointerButton::Secondary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

#[test]
fn right_clicking_a_chart_opens_its_menu_not_the_cells() {
    // #2067: a right-click on a chart opened the menu of the cell under it, or nothing at all.
    for wobble in [vec2(0.0, 0.0), vec2(20.0, 10.0)] {
        let (mut h, id) = fixture();
        let (w0, h0) = size(&h, id);
        let inside = corner(&h, id) - vec2(100.0, 80.0);
        right_click(&mut h, inside, wobble);
        assert_eq!(h.state().selected_chart, Some(id), "the right-clicked chart is selected ({wobble:?})");
        assert!(matches!(h.state().grid.object_menu, Some((_, "chart", i)) if i == id), "the chart's menu opens ({wobble:?})");
        assert!(h.state().grid.context_menu.is_none(), "the cell menu stays closed ({wobble:?})");
        assert_eq!(size(&h, id), (w0, h0));
        let a = h.state().session.doc().unwrap().wb.active().unwrap().charts.iter().find(|c| c.id == id).unwrap().anchor;
        assert_eq!(a.cell, CellRef::parse("D3").unwrap(), "a right-button drag does not move the chart ({wobble:?})");
    }
    // Delete from the menu removes the chart.
    let (mut h, id) = fixture();
    let inside = corner(&h, id) - vec2(100.0, 80.0);
    right_click(&mut h, inside, vec2(0.0, 0.0));
    h.get_by_label("Delete").click();
    h.run_steps(3);
    assert!(h.state().session.doc().unwrap().wb.active().unwrap().charts.is_empty());
    assert!(h.state().grid.object_menu.is_none());
    // Outside the chart a right-click still opens the cell menu.
    let (mut h, _) = fixture();
    let cell = {
        let app = h.state();
        let sh = app.session.doc().unwrap().wb.active().unwrap();
        Geo::new(sh, app.grid.rect.unwrap(), app.view().scroll).cell_rect(sh, CellRef::parse("A10").unwrap()).center()
    };
    right_click(&mut h, cell, vec2(0.0, 0.0));
    assert!(h.state().grid.context_menu.is_some() && h.state().grid.object_menu.is_none());
}
