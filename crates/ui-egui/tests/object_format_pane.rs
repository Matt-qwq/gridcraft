//! The Format pane's shape and picture sections: what they show, and that every control reaches
//! the engine's `object.format` (or `object.setAnchorMode`) instead of the model.

use egui::{Event, Key, Modifiers, PointerButton, Rect, accesskit::Role, vec2};
use egui_kittest::kittest::Queryable as _;
use gridcraft_engine::Session;
use gridcraft_engine::model::{AnchorMode, Color, Image, Shape, ShapeTextAlign, ShapeTextAnchor};
use gridcraft_ui_egui::SheetApp;
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

/// A 1x1 PNG: a picture to select without a binary fixture in the repo.
const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

fn harness(session: Session) -> Harness {
    let mut h = egui_kittest::Harness::builder().with_size(vec2(1300.0, 1000.0)).build_ui_state(
        |ui, app: &mut SheetApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        SheetApp::new(session, Default::default()),
    );
    h.run_steps(4);
    h
}

/// A workbook with one text box on the sheet, selected, with the Format pane open.
fn text_box() -> (Harness, u32) {
    let mut session = Session::new();
    session.new_workbook();
    let id = session.execute("insert.textBox", json!({"at": "C3", "width": 200, "height": 90, "text": "Summary"})).unwrap()["shape"]
        .as_u64()
        .unwrap_or(0) as u32;
    let mut h = harness(session);
    h.state_mut().selected_chart = Some(id);
    h.state_mut().grid.pane = Some("formatChart".into());
    h.run_steps(2);
    (h, id)
}

fn shape(h: &Harness, id: u32) -> Shape {
    h.state().session.doc().unwrap().wb.active().unwrap().shapes.iter().find(|s| s.id == id).cloned().expect("the shape")
}

fn picture(h: &Harness, id: u32) -> Image {
    h.state().session.doc().unwrap().wb.active().unwrap().images.iter().find(|i| i.id == id).cloned().expect("the picture")
}

/// Opens a collapsing section of the pane by its header.
fn expand(h: &mut Harness, title: &str) {
    h.get_by_role_and_label(Role::Button, title).click();
    h.run_steps(2);
}

fn click(h: &mut Harness, label: &str) {
    h.get_by_label(label).click();
    h.run_steps(2);
}

fn header(h: &Harness, title: &str) -> bool {
    h.query_by_role_and_label(Role::Button, title).is_some()
}

/// Every field the pane draws on its right-hand side, top to bottom.
fn pane_fields(h: &Harness, role: Role) -> Vec<Rect> {
    h.get_all_by_role(role).map(|n| n.rect()).filter(|r| r.left() > 650.0).collect()
}

/// Types into a number field and commits it with Enter.
fn type_number(h: &mut Harness, at: Rect, text: &str) {
    let at = at.center();
    h.input_mut().events.push(Event::PointerMoved(at));
    h.run_steps(1);
    h.input_mut().events.push(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(2);
    h.input_mut().events.push(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(1);
    // Clicking a DragValue turns it into a text field with its text selected.
    h.input_mut().events.push(Event::Text(text.into()));
    h.run_steps(1);
    h.input_mut().events.push(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.input_mut().events.push(Event::Key { key: Key::Enter, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

/// Replaces a text field's contents and commits them with Enter.
fn retype(h: &mut Harness, at: Rect, text: &str) {
    h.get_all_by_role(Role::TextInput).find(|n| n.rect() == at).expect("the field").click();
    h.run_steps(2);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(1);
    for c in text.chars() {
        h.input_mut().events.push(Event::Text(c.to_string()));
        h.run_steps(1);
    }
    h.key_press(Key::Enter);
    h.run_steps(2);
}

/// Clicks a combo box (found by its selected text) and then an entry in its list.
fn choose(h: &mut Harness, selected: &str, entry: &str) {
    h.get_by_value(selected).click();
    h.run_steps(2);
    click(h, entry);
}

#[test]
fn selecting_a_shape_offers_the_sections_it_can_store() {
    let (mut h, id) = text_box();
    for title in ["Fill", "Line", "Text", "Position & Size", "Alt Text"] {
        assert!(header(&h, title), "the pane offers the shape's {title} section");
    }
    // The chart sections belong to a chart.
    assert!(!header(&h, "Chart Type"), "a shape is not formatted as a chart");
    assert!(!header(&h, "Axis Options"));

    // The fill row shows what the shape holds (a text box's white) and changes it through the
    // command: the palette's own "No Fill" entry clears it.
    expand(&mut h, "Fill");
    assert_eq!(shape(&h, id).fill, Color::rgb(255, 255, 255), "a new text box is white");
    let undo = h.state().session.doc().unwrap().undo.len();
    click(&mut h, "Colour…");
    click(&mut h, "No Fill");
    assert_eq!(shape(&h, id).fill, Color::Auto, "No Fill reached the engine");
    assert!(h.state().session.journal.iter().any(|(cmd, _)| cmd == "object.format"), "the pane ran the command");
    assert_eq!(h.state().session.doc().unwrap().undo.len(), undo + 1, "one undo step");
}

#[test]
fn the_shape_line_changes_through_the_command() {
    let (mut h, id) = text_box();
    expand(&mut h, "Line");
    assert!(!shape(&h, id).format.no_line);
    click(&mut h, "No line");
    assert!(shape(&h, id).format.no_line, "the outline is turned off");
    click(&mut h, "No line");
    assert!(!shape(&h, id).format.no_line, "and on again");
}

#[test]
fn the_text_section_formats_the_shape_text() {
    let (mut h, id) = text_box();
    expand(&mut h, "Text");
    click(&mut h, "Bold");
    click(&mut h, "Italic");
    let f = shape(&h, id).format;
    assert!(f.bold && f.italic, "the checkboxes reach the command");

    // The size field sits on the row its label names.
    let label = h.get_by_label("Text size").rect();
    let field = h.get_all_by_role(Role::SpinButton).map(|n| n.rect()).find(|r| (r.center().y - label.center().y).abs() < 6.0).expect("a size field");
    type_number(&mut h, field, "20");
    assert_eq!(shape(&h, id).format.font_size, Some(20.0));

    // Both the alignment and the vertical anchor read "Default"; the alignment is the first.
    h.get_all_by_value("Default").next().expect("the alignment combo").click();
    h.run_steps(2);
    click(&mut h, "Center");
    assert_eq!(shape(&h, id).format.align, Some(ShapeTextAlign::Center));
    choose(&mut h, "Default", "Bottom");
    assert_eq!(shape(&h, id).format.anchor, Some(ShapeTextAnchor::Bottom));
}

#[test]
fn the_box_section_moves_resizes_and_anchors_the_shape() {
    let (mut h, id) = text_box();
    expand(&mut h, "Position & Size");
    let sizes = pane_fields(&h, Role::SpinButton);
    assert_eq!(sizes.len(), 2, "a width and a height field: {sizes:?}");
    type_number(&mut h, sizes[0], "260");
    assert_eq!(shape(&h, id).anchor.width, 260.0);
    type_number(&mut h, sizes[1], "120");
    assert_eq!(shape(&h, id).anchor.height, 120.0);

    let at = *pane_fields(&h, Role::TextInput).first().expect("the From field");
    retype(&mut h, at, "B5");
    assert_eq!(shape(&h, id).anchor.cell.a1(), "B5");

    click(&mut h, "Move but don't size with cells");
    assert_eq!(shape(&h, id).anchor.mode, AnchorMode::MoveOnly);
}

#[test]
fn the_alt_text_section_sets_the_description() {
    let (mut h, id) = text_box();
    expand(&mut h, "Alt Text");
    let at = *pane_fields(&h, Role::TextInput).first().expect("the description field");
    retype(&mut h, at, "Summary of the region totals");
    assert_eq!(shape(&h, id).alt.as_deref(), Some("Summary of the region totals"));
}

#[test]
fn a_picture_offers_its_box_and_alt_text_only() {
    let mut session = Session::new();
    session.new_workbook();
    let id = session.execute("insert.picture", json!({"base64": PNG, "mime": "image/png", "at": "A10", "width": 60, "height": 60})).unwrap()["image"]
        .as_u64()
        .unwrap_or(0) as u32;
    let mut h = harness(session);
    h.state_mut().selected_chart = Some(id);
    h.state_mut().grid.pane = Some("formatChart".into());
    h.run_steps(2);
    assert!(header(&h, "Position & Size"));
    assert!(header(&h, "Alt Text"));
    // A picture's pixels are its fill and outline; the model stores none of them.
    assert!(!header(&h, "Fill"));
    assert!(!header(&h, "Line"));
    assert!(!header(&h, "Text"));

    expand(&mut h, "Position & Size");
    let sizes = pane_fields(&h, Role::SpinButton);
    type_number(&mut h, sizes[0], "150");
    assert_eq!(picture(&h, id).anchor.width, 150.0);
    click(&mut h, "Don't move or size with cells");
    assert_eq!(picture(&h, id).anchor.mode, AnchorMode::Absolute);
}

#[test]
fn a_chart_still_opens_the_chart_sections() {
    let mut session = Session::new();
    session.new_workbook();
    session.execute("range.setValues", json!({"range": "A1", "values": [["Region", "Q1"], ["North", 48], ["South", 39]]})).unwrap();
    session.execute("selection.set", json!({"range": "A1:B3"})).unwrap();
    let id = session.execute("insert.chart", json!({"type": "column", "at": "D2", "width": 420, "height": 260})).unwrap()["chart"]
        .as_u64()
        .unwrap_or(0) as u32;
    let mut h = harness(session);
    h.state_mut().selected_chart = Some(id);
    h.state_mut().grid.pane = Some("formatChart".into());
    h.run_steps(2);
    assert!(header(&h, "Chart Type"), "a chart keeps the chart area's sections");
    assert!(!header(&h, "Position & Size"), "the shape sections are not offered for a chart");
}
