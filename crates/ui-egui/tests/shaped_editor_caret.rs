//! The in-cell editor's caret in mixed-direction text: it moves through the *visual* clusters,
//! a click lands on the cluster under the pointer, and typing still inserts at the logical offset.
//!
//! The buffer stays logical; only the mapping between a character index and an x goes through the
//! shaped line. These tests run the real widget through frames, so they cover the wiring and not
//! just the map: the caret index the editor ends up with is the one egui is handed.

use egui::{Color32, Context, Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, pos2, vec2};
use gridcraft_engine::Session;
use gridcraft_engine::core::text::Direction;
use gridcraft_ui_egui::{SheetApp, grid::Geo, theme};

fn frame(ctx: &Context, app: &mut SheetApp, time: f64, events: Vec<Event>) {
    let mut output = ctx.run_ui(
        RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0))), time: Some(time), events, ..Default::default() },
        |ui| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
    );
    output.textures_delta.clear();
}

fn key(key: Key, modifiers: Modifiers) -> Event {
    Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
}

/// A workbook whose A1 holds `text`, with the in-cell editor open and the caret at the end.
fn editing(text: &str) -> (Context, SheetApp, f64) {
    let ctx = Context::default();
    let mut session = Session::new();
    session.new_workbook();
    let mut app = SheetApp::new(session, Default::default());
    for t in 0..4 {
        frame(&ctx, &mut app, f64::from(t) * 0.01, vec![]);
    }
    app.run("range.setValues", serde_json::json!({"range": "A1", "values": [[text]]})).expect("set A1");
    // F2-style edit (not Enter-mode typing): the arrow keys move the caret instead of committing.
    app.begin_edit(None, false);
    for t in 4..8 {
        frame(&ctx, &mut app, f64::from(t) * 0.01, vec![]);
    }
    (ctx, app, 0.08)
}

/// The shaped line and caret map the in-cell editor uses for `text`, or `None` when this machine's
/// cell font cannot draw it (the editor then keeps its ordinary path and there is nothing to test).
fn caret_geometry(ctx: &Context, app: &SheetApp, text: &str) -> Option<(gridcraft_ui_egui::text::Line, gridcraft_ui_egui::text::CaretMap)> {
    let d = app.session.active()?;
    let wb = &d.wb;
    let sh = wb.sheet(wb.active_sheet)?;
    let id = sh.style_id(d.selection.active);
    let font = &wb.styles.get(id).font;
    let font_id = egui::FontId::new(font.size * 96.0 / 72.0, theme::cell_family(&font.name, font.bold, font.italic));
    let line = app.text.line(ctx, &font_id, text, Direction::Auto, Color32::BLACK);
    let visual = line.visual_order_differs();
    let map = app.text.caret_map(text, &line)?;
    (line.shaped.is_some() && visual).then_some((line, map))
}

#[test]
fn arrow_keys_move_the_caret_through_the_visual_clusters() {
    let text = "abc אבג";
    let (ctx, mut app, mut time) = editing(text);
    if caret_geometry(&ctx, &app, text).is_none() {
        return;
    }
    let map = {
        let d = app.session.active().expect("a document");
        let wb = &d.wb;
        let sh = wb.sheet(wb.active_sheet).expect("a sheet");
        let id = sh.style_id(d.selection.active);
        let font = &wb.styles.get(id).font;
        let font_id = egui::FontId::new(font.size * 96.0 / 72.0, theme::cell_family(&font.name, font.bold, font.italic));
        let line = app.text.line(&ctx, &font_id, text, Direction::Auto, Color32::BLACK);
        app.text.caret_map(text, &line).expect("a shaped line has a caret map")
    };
    // The caret starts at the end of the text (as `begin_edit` puts it).
    assert_eq!(app.editor.as_ref().expect("editing").caret, 7);
    // Walking left visits the clusters in the reverse of their visual order: the indices are not
    // descending, because the Hebrew half is drawn right to left.
    let mut seen = vec![7usize];
    for _ in 0..7 {
        time += 0.01;
        frame(&ctx, &mut app, time, vec![key(Key::ArrowLeft, Modifiers::NONE)]);
        let caret = app.editor.as_ref().expect("still editing").caret;
        if seen.last() == Some(&caret) {
            break;
        }
        seen.push(caret);
    }
    let mut sorted = seen.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), seen.len(), "the caret never repeats a cluster: {seen:?}");
    assert!(seen.contains(&0), "walking left reaches the start: {seen:?}");
    let xs: Vec<f32> = seen.iter().map(|i| map.caret_x(*i)).collect();
    assert!(xs.windows(2).all(|w| w[0] >= w[1]), "the caret moves leftwards on screen: {xs:?}");
    // Walking right through every cluster visits all of them and never moves backwards on screen.
    let mut seen_right = vec![0usize];
    for _ in 0..8 {
        time += 0.01;
        let before = app.editor.as_ref().expect("editing").caret;
        frame(&ctx, &mut app, time, vec![key(Key::ArrowRight, Modifiers::NONE)]);
        let at = app.editor.as_ref().expect("still editing").caret;
        if at == before {
            break;
        }
        seen_right.push(at);
    }
    let mut all = seen_right.clone();
    all.sort_unstable();
    all.dedup();
    assert_eq!(all, (0..=7).collect::<Vec<_>>(), "every cluster edge is a caret stop: {seen_right:?}");
    let xs: Vec<f32> = seen_right.iter().map(|i| map.caret_x(*i)).collect();
    assert!(xs.windows(2).all(|w| w[0] <= w[1]), "the caret moves rightwards on screen: {xs:?}");
}

#[test]
fn a_click_lands_on_the_cluster_under_the_pointer() {
    let text = "abc אבג";
    let (ctx, mut app, mut time) = editing(text);
    let Some((line, map)) = caret_geometry(&ctx, &app, text) else { return };
    let d = app.session.active().expect("a document");
    let wb = d.wb.clone();
    let sh = wb.active().expect("a sheet");
    let geo = Geo::new(sh, app.grid.rect.expect("the grid was painted"), egui::Vec2::ZERO);
    let cell = geo.cell_rect(sh, gridcraft_engine::core::CellRef::new(0, 0));
    let line_left = cell.left() + 2.0;
    // Click in the middle of the visually leftmost Hebrew cluster: the caret must land on its
    // first byte, not on the character egui's stored-order layout would put there.
    let hebrew = map.visual_positions().iter().rev().find(|(i, _)| *i <= 7).map(|(i, _)| *i).expect("a position");
    let x = line_left + map.caret_x(hebrew) - 0.5;
    let y = cell.center().y;
    for pressed in [true, false] {
        time += 0.01;
        frame(
            &ctx,
            &mut app,
            time,
            vec![
                Event::PointerMoved(pos2(x, y)),
                Event::PointerButton { pos: pos2(x, y), button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE },
            ],
        );
    }
    let caret = app.editor.as_ref().expect("still editing").caret;
    assert_eq!(caret, map.index_at_x(x - line_left), "the click selects the cluster under it ({caret})");
    assert_eq!(map.caret_x(caret), map.caret_x(hebrew), "the caret is drawn at the cluster it clicked");
    assert!(line.width > 0.0);
}

#[test]
fn typing_at_a_visual_caret_inserts_at_the_logical_offset() {
    let text = "abc אבג";
    let (ctx, mut app, mut time) = editing(text);
    if caret_geometry(&ctx, &app, text).is_none() {
        return;
    }
    // Walk the caret to the start, then right through the visual clusters: 0, 1, 2, 3 (end of
    // "abc"), 7 (left of the Hebrew block), 6, 5 and 4 — the right-hand end of the line, which is
    // logically *before* the first Hebrew character.
    for _ in 0..7 {
        time += 0.01;
        frame(&ctx, &mut app, time, vec![key(Key::ArrowLeft, Modifiers::NONE)]);
    }
    assert_eq!(app.editor.as_ref().expect("editing").caret, 0);
    for _ in 0..7 {
        time += 0.01;
        frame(&ctx, &mut app, time, vec![key(Key::ArrowRight, Modifiers::NONE)]);
    }
    assert_eq!(app.editor.as_ref().expect("editing").caret, 4);
    time += 0.01;
    frame(&ctx, &mut app, time, vec![Event::Text("X".into())]);
    let ed = app.editor.as_ref().expect("still editing");
    assert_eq!(ed.text, "abc Xאבג", "typing goes where the caret logically is, not where it is drawn");
    assert_eq!(ed.caret, 5);
    // And committing stores the logical text.
    time += 0.01;
    frame(&ctx, &mut app, time, vec![key(Key::Enter, Modifiers::NONE)]);
    let d = app.session.active().expect("a document");
    let cell = d.wb.sheet(d.wb.active_sheet).expect("a sheet").cell(gridcraft_engine::core::CellRef::new(0, 0));
    assert_eq!(cell.map(|c| c.input_text()).unwrap_or_default(), "abc Xאבג");
}

#[test]
fn the_editor_paints_a_caret_in_the_shaped_line() {
    let text = "אבג abc";
    let (ctx, mut app, mut time) = editing(text);
    if caret_geometry(&ctx, &app, text).is_none() {
        return;
    }
    time += 0.01;
    let mut output = ctx.run_ui(
        RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0))), time: Some(time), ..Default::default() },
        |ui| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
    );
    output.textures_delta.clear();
    // Our caret is a vertical line segment; egui's own caret is hidden for shaped editing, so the
    // only vertical segment inside the editor is ours.
    let vertical: Vec<f32> = output
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::LineSegment { points, .. } if (points[0].x - points[1].x).abs() < 0.01 && (points[0].y - points[1].y).abs() > 4.0 => {
                Some(points[0].x)
            }
            _ => None,
        })
        .collect();
    assert!(!vertical.is_empty(), "the shaped editor paints its caret");
}
