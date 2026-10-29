//! PivotTable slicers and timelines on the sheet, driven through real pointer and key input.
//!
//! The engine commands (`slicer.toggle`, `slicer.move`, `slicer.delete`, …) are the only way the
//! widgets change the workbook; these tests prove the grid reaches them and that the drawing
//! survives hostile model content.

use std::sync::Arc;

use egui::{Event, Key, Modifiers, PointerButton, Pos2, Shape, vec2};
use gridcraft_engine::Session;
use gridcraft_engine::core::{CellRef, MAX_COLS, MAX_ROWS};
use gridcraft_engine::model::{AnchorMode, SlicerValue};
use gridcraft_ui_egui::grid::Geo;
use gridcraft_ui_egui::slicer_view::{slicer_chrome, timeline_chrome, widget_rect};
use gridcraft_ui_egui::{SheetApp, theme};
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

/// Data on Sheet1, a PivotTable on "Report" (rows Region, values Amount), the active cell in the
/// report. Dates are formatted so the source knows the column holds dates, as a real sheet would.
fn session() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s.execute(
        "range.setValues",
        json!({"range": "A1", "values": [
            ["Region", "Amount", "Date"],
            ["East", 10, 45658],
            ["West", 20, 45700],
            ["East", 30, 45750],
            ["North", 40, 45800],
        ]}),
    )
    .unwrap();
    s.execute("home.numberFormat", json!({"range": "C2:C5", "format": "yyyy-mm-dd"})).unwrap();
    s.execute("home.insertSheet", json!({"name": "Report"})).unwrap();
    s.execute(
        "insert.pivotTable",
        json!({"source": "Sheet1!A1:C5", "destination": "Report!A1", "name": "P1", "rows": ["Region"], "values": ["Amount"]}),
    )
    .unwrap();
    s
}

/// A slicer at D3 and a timeline at H3 on the report.
fn fixture() -> (Harness, u32, u32) {
    let mut s = session();
    let slicer = s.execute("insert.slicer", json!({"pivot": "P1", "field": "Region", "at": "D3", "width": 168, "height": 220})).unwrap();
    let slicer = slicer["slicer"].as_u64().unwrap() as u32;
    let timeline =
        s.execute("insert.timeline", json!({"pivot": "P1", "field": "Date", "unit": "quarters", "at": "H3", "width": 240, "height": 96})).unwrap();
    let timeline = timeline["timeline"].as_u64().unwrap() as u32;
    let mut h = Harness::builder().with_size(vec2(1200.0, 800.0)).build_ui_state(
        |ui, app: &mut SheetApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        SheetApp::new(s, Default::default()),
    );
    h.run_steps(4);
    (h, slicer, timeline)
}

fn geo(h: &Harness) -> Geo {
    let app = h.state();
    let sh = app.session.active().unwrap().wb.active().unwrap();
    Geo::new(sh, app.grid.rect.unwrap(), app.view().scroll)
}

/// The slicer's frame on screen.
fn frame(h: &Harness, id: u32) -> egui::Rect {
    let app = h.state();
    let sh = app.session.active().unwrap().wb.active().unwrap();
    let sl = sh.pivot_slicers.iter().find(|s| s.id == id).unwrap();
    widget_rect(&geo(h), sh, &sl.anchor)
}

/// The centre of the tile whose label is `label`.
fn tile(h: &Harness, id: u32, label: &str) -> Pos2 {
    let app = h.state();
    let sh = app.session.active().unwrap().wb.active().unwrap();
    let sl = sh.pivot_slicers.iter().find(|s| s.id == id).unwrap();
    let lay = slicer_chrome(&geo(h), frame(h, id), sl.columns, sl.items.len(), 0);
    let index = sl.items.iter().position(|i| i.label() == label).unwrap();
    lay.tiles.iter().find(|(i, _)| *i == index).unwrap().1.center()
}

fn move_to(h: &mut Harness, pos: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(pos));
    h.run_steps(2);
}

fn button(h: &mut Harness, pos: Pos2, pressed: bool) {
    h.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

fn click(h: &mut Harness, pos: Pos2) {
    move_to(h, pos);
    button(h, pos, true);
    button(h, pos, false);
}

fn key(h: &mut Harness, k: Key, m: Modifiers) {
    h.input_mut().events.push(Event::Key { key: k, physical_key: Some(k), pressed: true, repeat: false, modifiers: m });
    h.input_mut().events.push(Event::Key { key: k, physical_key: Some(k), pressed: false, repeat: false, modifiers: m });
    h.run_steps(2);
}

/// Selects the slicer by clicking its caption band (a header click selects, it does not toggle).
fn select_slicer(h: &mut Harness, id: u32) {
    let f = frame(h, id);
    click(h, f.left_top() + vec2(20.0, 13.0));
    assert_eq!(h.state().selected_chart, Some(id), "the slicer is selected");
}

/// The report's non-empty cell texts.
fn report_texts(h: &Harness) -> Vec<String> {
    let d = h.state().session.active().unwrap();
    let si = d.wb.sheet_index("Report").unwrap();
    let sh = d.wb.sheet(si).unwrap();
    let Some(r) = sh.used_range() else { return vec![] };
    let mut out = vec![];
    for row in r.start.row..=r.end.row {
        for col in r.start.col..=r.end.col {
            let t = gridcraft_engine::display::cell_text(&d.wb, sh, CellRef::new(row, col));
            if !t.is_empty() {
                out.push(t);
            }
        }
    }
    out
}

fn labels(h: &Harness, id: u32) -> Vec<String> {
    let d = h.state().session.active().unwrap();
    let sh = d.wb.active().unwrap();
    sh.pivot_slicers.iter().find(|s| s.id == id).and_then(|s| s.selected_labels()).unwrap_or_default()
}

fn painted_texts(h: &Harness) -> Vec<String> {
    h.output()
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            Shape::Text(t) => Some(t.galley.text().to_string()),
            _ => None,
        })
        .collect()
}

#[test]
fn clicking_a_tile_filters_the_report_and_clicking_it_again_clears_the_filter() {
    let (mut h, slicer, _) = fixture();
    assert_eq!(labels(&h, slicer), Vec::<String>::new(), "every tile starts selected");

    // Clicking the West tile is one `slicer.toggle`: only West remains selected and the report
    // drops the East and North rows.
    let west = tile(&h, slicer, "West");
    click(&mut h, west);
    assert_eq!(labels(&h, slicer), vec!["West".to_string()]);
    let shown = report_texts(&h);
    assert!(shown.iter().any(|t| t == "West"), "{shown:?}");
    assert!(!shown.iter().any(|t| t == "East" || t == "North"), "the report is filtered: {shown:?}");

    // Clicking the only selected tile clears the filter again.
    let west = tile(&h, slicer, "West");
    click(&mut h, west);
    assert_eq!(labels(&h, slicer), Vec::<String>::new());
    let shown = report_texts(&h);
    assert!(shown.iter().any(|t| t == "East"), "{shown:?}");
    assert!(shown.iter().any(|t| t == "North"), "{shown:?}");
}

#[test]
fn dragging_the_frame_moves_the_anchor_in_one_undo_step() {
    let (mut h, slicer, _) = fixture();
    select_slicer(&mut h, slicer);
    let f = frame(&h, slicer);
    let start = f.left_top() + vec2(20.0, 13.0);
    let delta = vec2(200.0, 100.0);
    let end = start + delta;
    let (original_cell, orig, z) = {
        let app = h.state();
        let sh = app.session.active().unwrap().wb.active().unwrap();
        let sl = sh.pivot_slicers.iter().find(|s| s.id == slicer).unwrap();
        (sl.anchor.cell, gridcraft_ui_egui::chartview::frame_of(sh, &sl.anchor), geo(&h).z)
    };
    let want = {
        let app = h.state();
        let sh = app.session.active().unwrap().wb.active().unwrap();
        let moved = gridcraft_ui_egui::chartview::drag_frame(orig, gridcraft_ui_egui::chartview::Edges::MOVE, delta / z, false);
        gridcraft_ui_egui::chartview::anchor_of(sh, moved).0
    };
    assert_ne!(want, original_cell);

    let undo_before = h.state().session.active().unwrap().undo.len();
    move_to(&mut h, start);
    button(&mut h, start, true);
    for i in 1..=5 {
        move_to(&mut h, start + delta * (i as f32 / 5.0));
    }
    move_to(&mut h, end);
    button(&mut h, end, false);

    let anchor = {
        let app = h.state();
        let sh = app.session.active().unwrap().wb.active().unwrap();
        sh.pivot_slicers.iter().find(|s| s.id == slicer).unwrap().anchor
    };
    assert_eq!(anchor.cell, want, "the frame followed the drag to a new anchor cell");
    assert_eq!(h.state().session.active().unwrap().undo.len(), undo_before + 1, "one drag is one undo step");

    h.state_mut().session.run("edit.undo", serde_json::json!({})).unwrap();
    h.run_steps(2);
    let after = {
        let app = h.state();
        let sh = app.session.active().unwrap().wb.active().unwrap();
        sh.pivot_slicers.iter().find(|s| s.id == slicer).unwrap().anchor.cell
    };
    assert_eq!(after, original_cell, "one undo puts the slicer back");
}

#[test]
fn delete_removes_the_selected_slicer() {
    let (mut h, slicer, _) = fixture();
    select_slicer(&mut h, slicer);
    key(&mut h, Key::Delete, Modifiers::NONE);
    let sh = h.state().session.active().unwrap().wb.active().unwrap();
    assert!(sh.pivot_slicers.is_empty(), "Delete runs slicer.delete");
    assert_eq!(h.state().selected_chart, None, "the selection goes with it");
}

#[test]
fn tab_moves_between_tiles_and_enter_toggles_the_focused_one() {
    let (mut h, slicer, _) = fixture();
    select_slicer(&mut h, slicer);
    key(&mut h, Key::Tab, Modifiers::NONE);
    assert_eq!(h.state().grid.slicer_focus, Some((slicer, 0)));
    key(&mut h, Key::Tab, Modifiers::NONE);
    assert_eq!(h.state().grid.slicer_focus, Some((slicer, 1)));
    // The second tile (source order) is West; Enter toggles exactly that tile.
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(labels(&h, slicer), vec!["West".to_string()]);
    let shown = report_texts(&h);
    assert!(!shown.iter().any(|t| t == "East" || t == "North"), "{shown:?}");
    // Shift+Tab walks back.
    key(&mut h, Key::Tab, Modifiers::SHIFT);
    assert_eq!(h.state().grid.slicer_focus, Some((slicer, 0)));
}

#[test]
fn a_hostile_anchor_and_a_kilometre_caption_still_draw() {
    let (mut h, slicer, timeline) = fixture();
    {
        let d = h.state_mut().session.doc_mut().unwrap();
        let wb = Arc::make_mut(&mut d.wb);
        let si = wb.active_sheet;
        let sh = wb.sheet_mut(si).unwrap();
        // Off the sheet with NaN offsets and a negative size: the drawing clamps every number.
        let bad = sh.pivot_slicers.iter().find(|s| s.id == slicer).map(|s| s.anchor).unwrap();
        if let Some(sl) = sh.pivot_slicers.iter_mut().find(|s| s.id == slicer) {
            sl.anchor = gridcraft_engine::model::Anchor {
                cell: CellRef::new(MAX_ROWS - 1, MAX_COLS - 1),
                dx: f32::NAN,
                dy: f32::NEG_INFINITY,
                width: f32::NAN,
                height: -5.0,
                mode: AnchorMode::MoveOnly,
            };
            sl.caption = "C".repeat(5000);
            if let Some(item) = sl.items.first_mut() {
                item.value = SlicerValue::Text("L".repeat(5000));
            }
        }
        // A second slicer on screen whose offsets and size are NaN, so the frame is painted too;
        // its caption and one tile label are a kilometre long and must be elided before layout.
        let mut on_screen = sh.pivot_slicers.first().cloned().unwrap_or_default();
        on_screen.id = slicer.saturating_add(100);
        on_screen.caption = "C".repeat(5000);
        on_screen.anchor = gridcraft_engine::model::Anchor {
            cell: CellRef::parse("D20").unwrap(),
            // A negative offset and a negative size, both clamped instead of drawn off the sheet.
            dx: -30.0,
            dy: -10.0,
            width: f32::NAN,
            height: -5.0,
            mode: AnchorMode::MoveOnly,
        };
        if let Some(item) = on_screen.items.first_mut() {
            item.value = SlicerValue::Text("L".repeat(5000));
        }
        sh.pivot_slicers.push(on_screen);
        if let Some(tl) = sh.pivot_timelines.iter_mut().find(|t| t.id == timeline) {
            tl.anchor = gridcraft_engine::model::Anchor {
                cell: bad.cell,
                dx: f32::NAN,
                dy: f32::NAN,
                width: f32::INFINITY,
                height: f32::NAN,
                mode: AnchorMode::MoveOnly,
            };
            tl.caption = "T".repeat(5000);
        }
    }
    h.run_steps(3);
    // Painting happened, and every frame the model can describe has a finite on-screen rect.
    assert!(!h.output().shapes.is_empty());
    let app = h.state();
    let sh = app.session.active().unwrap().wb.active().unwrap();
    let g = geo(&h);
    for sl in &sh.pivot_slicers {
        assert!(widget_rect(&g, sh, &sl.anchor).is_finite(), "slicer {} has a finite rect", sl.name);
    }
    for tl in &sh.pivot_timelines {
        assert!(widget_rect(&g, sh, &tl.anchor).is_finite(), "timeline {} has a finite rect", tl.name);
    }
    assert!(painted_texts(&h).iter().any(|t| t.starts_with("CCCC")), "the long caption is elided, not dropped");
}

#[test]
fn an_empty_item_list_draws_the_frame_only() {
    let (mut h, slicer, _) = fixture();
    {
        let d = h.state_mut().session.doc_mut().unwrap();
        let wb = Arc::make_mut(&mut d.wb);
        let si = wb.active_sheet;
        let sh = wb.sheet_mut(si).unwrap();
        if let Some(sl) = sh.pivot_slicers.iter_mut().find(|s| s.id == slicer) {
            sl.items.clear();
            sl.caption = "Empty slicer".into();
        }
    }
    h.run_steps(3);
    let app = h.state();
    let sh = app.session.active().unwrap().wb.active().unwrap();
    let sl = sh.pivot_slicers.iter().find(|s| s.id == slicer).unwrap();
    let lay = slicer_chrome(&geo(&h), frame(&h, slicer), sl.columns, sl.items.len(), 0);
    assert!(lay.tiles.is_empty(), "no tiles are laid out");
    assert!(painted_texts(&h).iter().any(|t| t == "Empty slicer"), "the caption band still draws");
}

#[test]
fn a_timeline_draws_its_caption_unit_and_range_track() {
    let (h, _, timeline) = fixture();
    let app = h.state();
    let sh = app.session.active().unwrap().wb.active().unwrap();
    let tl = sh.pivot_timelines.iter().find(|t| t.id == timeline).unwrap();
    let rect = widget_rect(&geo(&h), sh, &tl.anchor);
    let lay = timeline_chrome(&geo(&h), rect);
    assert!(rect.is_finite() && lay.track.width() > 0.0, "the track renders");
    let texts = painted_texts(&h);
    assert!(texts.iter().any(|t| t == "Date"), "the caption is drawn: {texts:?}");
    assert!(texts.iter().any(|t| t == "Quarters"), "the time unit is drawn: {texts:?}");
    // Two range handles, in the accent colour.
    let accent = theme::Tokens::light().accent;
    for handle in lay.handles {
        let found = h
            .output()
            .shapes
            .iter()
            .any(|s| matches!(&s.shape, Shape::Rect(r) if (r.rect.center() - handle.center()).length() < 1.0 && r.fill == accent));
        assert!(found, "a handle is drawn at {handle:?}");
    }
}

#[test]
fn the_selection_pane_lists_the_slicer_and_timeline() {
    let (mut h, _, _) = fixture();
    h.state_mut().grid.pane = Some("selection".into());
    h.run_steps(3);
    let texts = painted_texts(&h);
    assert!(texts.iter().any(|t| t.contains("Slicer Region")), "{texts:?}");
    assert!(texts.iter().any(|t| t.contains("Timeline Date")), "{texts:?}");
}

#[test]
fn right_click_opens_the_slicer_menu() {
    let (mut h, slicer, _) = fixture();
    let f = frame(&h, slicer);
    let pos = f.left_top() + vec2(20.0, 13.0);
    move_to(&mut h, pos);
    h.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Secondary, pressed: true, modifiers: Modifiers::NONE });
    h.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Secondary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    let menu = h.state().grid.object_menu;
    assert_eq!(menu.map(|(_, kind, id)| (kind, id)), Some(("pivotSlicer", slicer)));
    let texts = painted_texts(&h);
    for item in ["Delete", "Select All", "Clear Filter", "Slicer Styles"] {
        assert!(texts.iter().any(|t| t == item), "{item} is in the menu: {texts:?}");
    }
}
