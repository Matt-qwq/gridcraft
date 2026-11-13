//! The formula bar (Name Box, ✕ ✓ fx, formula field) and the shared cell editor widget.

use egui::{Color32, FontId, Key, Rect, Sense, Stroke, StrokeKind, TextEdit, pos2, vec2};
use serde_json::json;

use crate::SheetApp;
use crate::editor::{REF_COLORS, formula_refs};
use crate::icons::{self, Icon};
use crate::text::Line;
use crate::theme::{self, Tokens};

pub fn show(app: &mut SheetApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let expanded = app.ui.formula_bar_expanded;
    let h = if expanded { 72.0 } else { 30.0 };
    egui::Panel::top("formula_bar")
        .exact_size(h)
        .frame(egui::Frame::NONE.fill(t.window).inner_margin(egui::Margin { left: 8, right: 8, top: 3, bottom: 3 }))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                name_box(app, ui, &t);
                ui.add_space(4.0);
                let editing = app.editor.is_some();
                let c = if editing { t.text } else { t.text_disabled };
                if crate::widgets::icon_button(ui, Icon::Close, c, "Cancel", vec2(22.0, 22.0)).clicked() && editing {
                    app.cancel_edit();
                }
                if crate::widgets::icon_button(ui, Icon::Check, c, "Enter", vec2(22.0, 22.0)).clicked() && editing {
                    app.commit_edit(0, 0, false, false);
                }
                if crate::widgets::icon_button(ui, Icon::Fx, t.text, "Insert Function", vec2(26.0, 22.0)).clicked() {
                    app.open_dialog("insertFunction", json!({}));
                }
                ui.add_space(4.0);
                // The formula field.
                let avail = ui.available_width() - 24.0;
                let field = Rect::from_min_size(ui.cursor().min, vec2(avail, h - 6.0));
                ui.painter().rect_filled(field, 4.0, t.input_bg);
                ui.painter().rect_stroke(field, 4.0, Stroke::new(1.0, t.input_border), StrokeKind::Inside);
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(field.shrink2(vec2(6.0, 3.0))));
                let id = egui::Id::new("gridcraft.formula_bar");
                let read_only = active_spill_anchor(app).is_some();
                if app.editor.is_some() {
                    editor_widget(app, &mut child, id, theme::ui_font(13.0), expanded, field.width() - 12.0);
                } else {
                    // Show the active cell's content; clicking starts editing in the bar. A picture
                    // in the cell shows its own placeholder, in the frame's language.
                    let text = match active_picture(app) {
                        Some(alt) if alt.is_empty() => crate::widgets::tr(ui, "Picture in cell").to_string(),
                        Some(alt) => crate::widgets::tr(ui, "Picture: {}").replace("{}", &alt),
                        None => active_input(app),
                    };
                    let color = if read_only { t.text_disabled } else { t.text };
                    let resp = child
                        .add(egui::Label::new(egui::RichText::new(text).font(theme::ui_font(13.0)).color(color)).truncate().sense(Sense::click()));
                    let click = child.interact(field, id.with("click"), Sense::click());
                    if !read_only && (resp.clicked() || click.clicked()) {
                        app.begin_edit(None, true);
                    }
                }
                ui.allocate_space(vec2(avail, h - 6.0));
                let chevron = if expanded { Icon::ChevronUp } else { Icon::Chevron };
                if crate::widgets::icon_button(ui, chevron, t.text_dim, "Expand formula bar", vec2(20.0, 22.0)).clicked() {
                    app.ui.formula_bar_expanded = !app.ui.formula_bar_expanded;
                }
            });
        });
}

/// Resolve ownership on every frame; never cache a spill range in presentation state.
pub(crate) fn active_spill_anchor(app: &SheetApp) -> Option<gridcraft_engine::core::CellRef> {
    let d = app.session.active()?;
    let sh = d.wb.active()?;
    let at = sh.merge_at(d.selection.active).map(|m| m.start).unwrap_or(d.selection.active);
    sh.spill_anchor(at)
}

fn active_input(app: &SheetApp) -> String {
    let Some(d) = app.session.active() else { return String::new() };
    let Some(sh) = d.wb.active() else { return String::new() };
    let a = sh.merge_at(d.selection.active).map(|m| m.start).unwrap_or(d.selection.active);
    if let Some(anchor) = sh.spill_anchor(a) {
        if gridcraft_engine::display::formula_hidden(&d.wb, sh, anchor) {
            return String::new();
        }
        return sh
            .cell(anchor)
            .map(|c| crate::formula_locale::display(&c.input_text(), app.ui.language.formula_locale(), &d.wb, d.wb.active_sheet))
            .unwrap_or_default();
    }
    if let Some(c) = sh.cell(a) {
        if c.formula.is_some() && d.wb.styles.get(c.style).protection.hidden && sh.is_protected() {
            return String::new();
        }
        return if let Some(f) = &c.formula {
            let shown = crate::formula_locale::display(&c.input_text(), app.ui.language.formula_locale(), &d.wb, d.wb.active_sheet);
            // A legacy array formula shows in braces (`{=A1:A2*2}`), as `Cell::bar_text` does.
            if f.array.is_some() { format!("{{{shown}}}") } else { shown }
        } else {
            gridcraft_engine::display::edit_text(&d.wb, sh, a)
        };
    }
    String::new()
}

/// The alt text of the picture the active cell holds, when the bar shows a placeholder instead of
/// a value. The wording is translated where it is drawn.
fn active_picture(app: &SheetApp) -> Option<String> {
    let d = app.session.active()?;
    let sh = d.wb.active()?;
    let a = sh.merge_at(d.selection.active).map(|m| m.start).unwrap_or(d.selection.active);
    sh.cell_pictures.get(&a).map(|picture| picture.alt.clone())
}

fn name_box(app: &mut SheetApp, ui: &mut egui::Ui, t: &Tokens) {
    let id = egui::Id::new("gridcraft.name_box");
    let shown = app.name_box_text();
    let mut text = app.name_box.clone().unwrap_or(shown.clone());
    let rect = Rect::from_min_size(ui.cursor().min, vec2(118.0, 24.0));
    ui.painter().rect_filled(rect, 4.0, t.input_bg);
    ui.painter().rect_stroke(rect, 4.0, Stroke::new(1.0, t.input_border), StrokeKind::Inside);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(rect.min + vec2(6.0, 3.0), vec2(96.0, 18.0))));
    let resp = child.add(TextEdit::singleline(&mut text).id(id).frame(egui::Frame::NONE).font(theme::ui_font(13.0)).desired_width(96.0));
    if resp.gained_focus() {
        app.name_box = Some(shown.clone());
    }
    if resp.changed() {
        app.name_box = Some(text.clone());
    }
    if resp.lost_focus() {
        if ui.input(|i| i.key_pressed(Key::Enter)) {
            // The Enter that commits the Name Box is used up here. Left in the frame's input, the
            // grid (drawn after the formula bar, and focused again now the box let go) would read
            // it as its own Enter and move the active cell one row down from the jump target (#247).
            ui.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Key { key: Key::Enter, .. })));
            let target = text.trim().to_string();
            if !target.is_empty() && target != shown {
                // A new name for the selection, or a place to go.
                let is_ref = gridcraft_engine::core::RangeRef::parse(target.split('!').next_back().unwrap_or("")).is_some();
                let known = app.session.active().is_some_and(|d| d.wb.name(&target, d.wb.active_sheet).is_some() || d.wb.table(&target).is_some());
                let r = if is_ref || known {
                    app.run("edit.goTo", json!({"reference": target}))
                } else {
                    app.run("formulas.defineName", json!({"name": target}))
                };
                if let Err(e) = r {
                    app.message = Some(("GridCraft".into(), crate::clean_error(&e)));
                }
                app.grid.ensure_visible = true;
            }
        }
        app.name_box = None;
    }
    ui.allocate_space(vec2(118.0, 24.0));
    // Dropdown of names.
    let names: Vec<String> =
        app.session.active().map(|d| d.wb.names.iter().filter(|n| !n.hidden).map(|n| n.name.clone()).collect()).unwrap_or_default();
    let mb = Rect::from_min_size(pos2(rect.right() - 16.0, rect.top() + 4.0), vec2(14.0, 16.0));
    icons::paint(ui.painter(), mb, Icon::Chevron, t.text_dim);
    let r = ui.interact(mb, id.with("drop"), Sense::click());
    egui::Popup::menu(&r).show(|ui| {
        if names.is_empty() {
            ui.label(egui::RichText::new(crate::widgets::tr(ui, "No names defined")).italics());
        }
        for n in names {
            if ui.button(&n).clicked() {
                let _ = app.run("edit.goTo", json!({"reference": n}));
                app.grid.ensure_visible = true;
            }
        }
    });
}

/// Backspace and Delete take back the AutoComplete proposal (Excel's selected rest), not a typed character.
fn dismiss_completion(i: &mut egui::InputState, completion: &mut Option<String>) {
    if completion.is_some() && (i.consume_key(egui::Modifiers::NONE, Key::Backspace) || i.consume_key(egui::Modifiers::NONE, Key::Delete)) {
        *completion = None;
    }
}

/// Column AutoComplete proposes only after typing: deleting characters (Backspace, Delete) brings no proposal back, as in Excel.
fn entry_shrank(chars_before: usize, text: &str) -> bool {
    text.chars().count() < chars_before
}

/// The text editor used in the cell and in the formula bar (they share `app.editor`).
pub fn editor_widget(app: &mut SheetApp, ui: &mut egui::Ui, id: egui::Id, font: FontId, multiline: bool, width: f32) {
    let Some(mut ed) = app.editor.take() else { return };
    let mine = ed.from_formula_bar == (id == egui::Id::new("gridcraft.formula_bar"));
    // Keys the editor handles itself (before the TextEdit sees them), only where it has focus.
    let focused = ui.ctx().memory(|m| m.focused());
    let has_focus = focused == Some(id) || (mine && (ed.request_focus || focused.is_none()));
    let mut action: Option<(i64, i64, bool, bool)> = None;
    let mut arrow = false;
    let mut cancel = false;
    // Mixed-direction text in the in-cell editor: egui lays text out without the bidi algorithm,
    // so its glyphs, caret and selection highlight would sit in stored order. The field keeps the
    // buffer, the keyboard and the undo stack; the shaped line, the selection and the caret are
    // drawn here from the same engine the grid paints cells with. Text the shaper cannot reorder
    // (Latin, Cyrillic, CJK, a wrapped or multi-line cell) keeps egui's own editing untouched.
    let sheet_rtl = app.session.active().and_then(|d| d.wb.sheet(ed.sheet)).is_some_and(|s| s.right_to_left);
    let in_cell = id == egui::Id::new("gridcraft.cell_editor");
    let cell_text_color = if in_cell { Color32::BLACK } else { Tokens::get(ui.ctx()).text };
    let direction = crate::text::base_direction(&ed.text, sheet_rtl);
    let engine = std::sync::Arc::clone(&app.text);
    let shaped_line = (in_cell && !multiline && !ed.text.contains('\n') && crate::text::needs_shaping(&ed.text, direction))
        .then(|| engine.line(ui.ctx(), &font, &ed.text, direction, cell_text_color));
    let caret_map = shaped_line.as_ref().and_then(|line| engine.caret_map(&ed.text, line));
    let visual = shaped_line.as_ref().is_some_and(Line::visual_order_differs) && caret_map.is_some();
    let selection_color = ui.visuals().selection.bg_fill;
    let caret_color = ui.visuals().text_cursor.stroke.color;
    if visual {
        // egui's own highlight and caret would sit at the stored-order positions: hide them.
        ui.visuals_mut().selection.bg_fill = Color32::TRANSPARENT;
        ui.visuals_mut().text_cursor.stroke.color = Color32::TRANSPARENT;
    }
    // A message box over the editor (a data validation alert, a formula error) gets Enter and Esc;
    // the editor used to take them, so the first Esc cancelled the edit behind the message.
    if has_focus && mine && app.message.is_none() {
        let (enter, tab, esc, alt_enter, cmd_enter, ctrl_shift_enter, shift) = ui.input_mut(|i| {
            let m = i.modifiers;
            let alt_enter =
                i.consume_key(egui::Modifiers::ALT, Key::Enter) || i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::ALT, Key::Enter);
            let ctrl_shift_enter = i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::Enter);
            let cmd_enter = i.consume_key(egui::Modifiers::COMMAND, Key::Enter);
            let enter = i.consume_key(egui::Modifiers::NONE, Key::Enter) || i.consume_key(egui::Modifiers::SHIFT, Key::Enter);
            let tab = i.consume_key(egui::Modifiers::NONE, Key::Tab) || i.consume_key(egui::Modifiers::SHIFT, Key::Tab);
            let esc = i.consume_key(egui::Modifiers::NONE, Key::Escape);
            dismiss_completion(i, &mut ed.completion);
            (enter, tab, esc, alt_enter, cmd_enter, ctrl_shift_enter, m.shift)
        });
        if alt_enter {
            let b = ed.text.char_indices().nth(ed.caret).map(|(i, _)| i).unwrap_or(ed.text.len());
            ed.text.insert(b, '\n');
            ed.caret += 1;
            ed.caret_to_end = false;
            ed.sync_caret = true;
        }
        if !ed.autocomplete.is_empty() && tab {
            ed.accept_autocomplete();
        } else if enter {
            action = Some((if shift { -1 } else { 1 }, 0, false, false));
        } else if tab {
            action = Some((0, if shift { -1 } else { 1 }, false, false));
        }
        if cmd_enter {
            action = Some((0, 0, false, true));
        }
        if ctrl_shift_enter {
            action = Some((0, 0, true, false));
        }
        if esc {
            if !ed.autocomplete.is_empty() {
                ed.autocomplete.clear();
            } else if ed.completion.is_some() {
                ed.completion = None;
            } else {
                cancel = true;
            }
        }
        // Arrow keys: autocomplete navigation, point mode, or (Enter mode) commit and move.
        let arrows = ui.input(|i| {
            (
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::ArrowLeft),
                i.key_pressed(Key::ArrowRight),
                i.modifiers.shift,
            )
        });
        let (up, down, left, right, ashift) = arrows;
        if (up || down) && !ed.autocomplete.is_empty() {
            ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, Key::ArrowUp);
                i.consume_key(egui::Modifiers::NONE, Key::ArrowDown);
            });
            let n = ed.autocomplete.len();
            ed.ac_index = if down { (ed.ac_index + 1) % n } else { (ed.ac_index + n - 1) % n };
        } else if ed.enter_mode && (up || down || left || right) && !ed.from_formula_bar {
            let (dr, dc) = if up {
                (-1, 0)
            } else if down {
                (1, 0)
            } else if left {
                (0, -1)
            } else {
                (0, 1)
            };
            // Left/Right follow the screen on a right-to-left sheet.
            let rtl = app.session.active().and_then(|d| d.wb.active()).is_some_and(|s| s.right_to_left);
            let dc = if rtl { -dc } else { dc };
            if ed.can_point() {
                ui.input_mut(|i| {
                    for k in [Key::ArrowUp, Key::ArrowDown, Key::ArrowLeft, Key::ArrowRight] {
                        i.consume_key(egui::Modifiers::NONE, k);
                        i.consume_key(egui::Modifiers::SHIFT, k);
                    }
                });
                crate::grid::point_move(app, &mut ed, dr, dc, ashift);
            } else {
                action = Some((dr, dc, false, false));
                arrow = true;
            }
        }
        // F4 cycles $ anchors of the reference before the caret.
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::F4)) {
            cycle_anchor(&mut ed);
        }
        // Left/Right in mixed-direction text: the caret moves through the *visual* clusters, so it
        // follows what the reader sees instead of the stored byte order. Shift extends from where
        // the selection started. Only the caret moves; the buffer and the insertion point stay
        // logical (typing still inserts at character `ed.caret`).
        if visual && !ed.enter_mode {
            let (left, right, shift) = ui.input_mut(|i| {
                let shift = i.modifiers.shift;
                let left = i.consume_key(egui::Modifiers::NONE, Key::ArrowLeft) || i.consume_key(egui::Modifiers::SHIFT, Key::ArrowLeft);
                let right = i.consume_key(egui::Modifiers::NONE, Key::ArrowRight) || i.consume_key(egui::Modifiers::SHIFT, Key::ArrowRight);
                (left, right, shift)
            });
            if (left || right)
                && let Some(map) = &caret_map
            {
                let from = ed.caret;
                let focus = map.step(from, right);
                if focus != from || shift {
                    // Shift keeps the anchor where the selection started; a plain move collapses it.
                    ed.caret_anchor = if shift { Some(ed.caret_anchor.unwrap_or(from)) } else { None };
                    ed.caret = focus;
                    ed.caret_to_end = false;
                    ed.sync_caret = true;
                }
            }
        }
        // Ctrl+Shift+A (Control+Shift+A on a Mac) inserts the argument names. Consumed anywhere,
        // as in Excel, so it never falls through to the text's select-all.
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::A)) {
            let (text, caret) = (ed.text.clone(), ed.caret);
            if ed.insert_argument_names(app.ui.language)
                && let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id)
            {
                // The text edit records an undo point only once typing pauses: mark the text
                // before the shortcut so one Undo takes back just the inserted names.
                let mut undoer = state.undoer();
                let cursor = state.cursor.char_range().unwrap_or(egui::text::CCursorRange::one(egui::text::CCursor::new(caret)));
                undoer.add_undo(&(cursor, text));
                state.set_undoer(undoer);
                state.store(ui.ctx(), id);
            }
        }
    }
    // The text widget.
    let refs = formula_refs(&ed.text);
    let text_color = Tokens::get(ui.ctx()).text;
    let in_cell = id == egui::Id::new("gridcraft.cell_editor");
    let cell_text_color = if in_cell { Color32::BLACK } else { text_color };
    let font2 = font.clone();
    let glyph_color = if visual { Color32::TRANSPARENT } else { cell_text_color };
    let mut layouter = move |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
        let s = buf.as_str();
        let mut job = egui::text::LayoutJob::default();
        let mut pos = 0usize;
        let mut spans: Vec<(usize, usize, usize)> = refs.iter().filter(|r| r.1 <= s.len()).map(|r| (r.0, r.1, r.4)).collect();
        spans.sort();
        for (a, b, idx) in spans {
            if a < pos || !s.is_char_boundary(a) || !s.is_char_boundary(b) {
                continue;
            }
            job.append(s.get(pos..a).unwrap_or(""), 0.0, egui::text::TextFormat::simple(font2.clone(), glyph_color));
            job.append(s.get(a..b).unwrap_or(""), 0.0, egui::text::TextFormat::simple(font2.clone(), REF_COLORS[idx]));
            pos = b;
        }
        job.append(s.get(pos..).unwrap_or(""), 0.0, egui::text::TextFormat::simple(font2.clone(), glyph_color));
        job.wrap.max_width = if multiline { wrap_width } else { f32::INFINITY };
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let chars_before = ed.text.chars().count();
    // The in-cell editor is as tall as its lines (a multiline field reserves four rows).
    let edit = if multiline || ed.text.contains('\n') {
        TextEdit::multiline(&mut ed.text).desired_rows(if in_cell { 1 } else { 4 })
    } else {
        TextEdit::singleline(&mut ed.text)
    };
    let mut output = edit
        .id(id)
        .frame(egui::Frame::NONE)
        .font(font)
        .desired_width(width)
        .layouter(&mut layouter)
        .lock_focus(true)
        .event_filter(egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true })
        .show(ui);
    let resp = output.response.clone();
    let synced = mine && (ed.request_focus || ed.sync_caret);
    let mut range = output.cursor_range.unwrap_or_else(|| egui::text::CCursorRange::one(egui::text::CCursor::new(ed.caret)));
    let mut store_cursor = false;
    if synced {
        if ed.request_focus {
            resp.request_focus();
        }
        ed.request_focus = false;
        ed.sync_caret = false;
        range = egui::text::CCursorRange::one(egui::text::CCursor::new(ed.caret));
        if let Some(anchor) = ed.caret_anchor.filter(|a| *a != ed.caret) {
            range.secondary = egui::text::CCursor::new(anchor);
        }
        output.state.cursor.set_char_range(Some(range));
        store_cursor = true;
    } else if resp.clicked() || resp.gained_focus() {
        // Clicking into the other editor moves editing there.
        ed.from_formula_bar = id == egui::Id::new("gridcraft.formula_bar");
        ed.enter_mode = false;
        app.session.mode = gridcraft_engine::Mode::Edit;
    }
    if !synced && let Some(r) = output.cursor_range {
        ed.caret = r.primary.index.into();
        ed.caret_anchor = (r.primary != r.secondary).then(|| r.secondary.index.into());
    }
    // A click or a drag into mixed-direction text selects the cluster under the pointer: the
    // stored-order glyph x egui would pick is not the one on screen.
    if visual
        && let Some(map) = &caret_map
        && (resp.clicked() || resp.dragged())
        && let Some(p) = resp.interact_pointer_pos()
    {
        let focus = map.index_at_x(p.x - output.galley_pos.x);
        let anchor = if resp.dragged() { ed.caret_anchor.unwrap_or(focus) } else { focus };
        ed.caret = focus;
        ed.caret_anchor = (anchor != focus).then_some(anchor);
        range = egui::text::CCursorRange::one(egui::text::CCursor::new(focus));
        if let Some(anchor) = ed.caret_anchor {
            range.secondary = egui::text::CCursor::new(anchor);
        }
        output.state.cursor.set_char_range(Some(range));
        store_cursor = true;
    }
    if store_cursor {
        output.state.store(ui.ctx(), id);
    }
    if resp.changed() {
        ed.caret_anchor = None;
        ed.completion = None;
        if app.ui.autocomplete
            && !ed.is_formula()
            && !entry_shrank(chars_before, &ed.text)
            && ed.caret == ed.text.chars().count()
            && let Some(sh) = app.session.active().and_then(|d| d.wb.sheet(ed.sheet))
        {
            ed.completion = crate::editor::column_completion(sh, ed.cell, &ed.text);
        }
        ed.point = None;
        ed.point_anchor = None;
        // Typing ends the pointing: the next arrow starts again from the edited cell, as in Excel.
        ed.point_cell = None;
        let names: Vec<String> = function_names()
            .iter()
            .map(|name| {
                app.session.active().map(|d| crate::formula_locale::completion_name(name, ed.locale, &d.wb, ed.sheet)).unwrap_or(name).to_string()
            })
            .collect();
        ed.update_autocomplete(&names);
        app.session.mode = if ed.can_point() {
            gridcraft_engine::Mode::Point
        } else if ed.enter_mode {
            gridcraft_engine::Mode::Enter
        } else {
            gridcraft_engine::Mode::Edit
        };
    }
    // The shaped line, its selection islands and its caret. egui's own glyphs are transparent and
    // its highlight and caret hidden (see above), so this is the whole picture of the text.
    if visual && let (Some(line), Some(map)) = (&shaped_line, &caret_map) {
        let focus: usize = range.primary.index.into();
        let anchor: usize = range.secondary.index.into();
        let (lo, hi) = (focus.min(anchor), focus.max(anchor));
        let painter = ui.painter().with_clip_rect(resp.rect);
        let top = output.galley_pos.y;
        let bottom = top + output.galley.rect.height();
        for (x0, x1) in map.selection_spans(lo, hi) {
            let r = egui::Rect::from_min_max(pos2(output.galley_pos.x + x0, top), pos2(output.galley_pos.x + x1, bottom));
            painter.rect_filled(r, 0.0, selection_color);
        }
        line.paint(&painter, output.galley_pos, cell_text_color);
        let x = output.galley_pos.x + map.caret_x(focus);
        painter.line_segment([pos2(x, top), pos2(x, bottom)], Stroke::new(1.0, caret_color));
    }
    // Column AutoComplete: the rest of the suggested entry, shown selected after the caret.
    if let Some(full) = &ed.completion
        && let Some(rest) = crate::editor::completion_rest(full, &ed.text)
    {
        let pos = resp.rect.left_top() + egui::vec2(output.galley.rect.width(), 0.0);
        let g = ui.painter().layout_no_wrap(
            rest.to_string(),
            output.galley.job.sections.first().map(|s| s.format.font_id.clone()).unwrap_or_default(),
            Color32::WHITE,
        );
        let r = egui::Rect::from_min_size(pos, g.size());
        ui.painter().rect_filled(r, 0.0, Color32::from_rgb(0x2E, 0x6F, 0xD8));
        ui.painter().galley(pos, g, Color32::WHITE);
    }
    // Autocomplete list and argument hint under the editor.
    if mine && ui.is_rect_visible(resp.rect) && (!ed.autocomplete.is_empty() || ed.current_function().is_some()) {
        let below = resp.rect.left_bottom() + vec2(0.0, 4.0);
        let language = app.ui.language;
        // Areas have their own clip rect: keep cell helpers inside the worksheet.
        let bounds = if ed.from_formula_bar { ui.ctx().content_rect() } else { ui.clip_rect() };
        egui::Area::new(id.with("ac")).fixed_pos(below).constrain_to(bounds).order(egui::Order::Tooltip).show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).inner_margin(4.0).show(ui, |ui| {
                if !ed.autocomplete.is_empty() {
                    let mut pick = None;
                    for (i, n) in ed.autocomplete.iter().enumerate() {
                        let sel = i == ed.ac_index;
                        let r = ui.add(
                            egui::Button::selectable(sel, egui::RichText::new(format!("ƒ  {n}")).font(theme::ui_font(12.5)))
                                .min_size(vec2(220.0, 20.0)),
                        );
                        if r.clicked() {
                            pick = Some(i);
                        }
                        if sel && let Some(desc) = crate::formula_locale::description(n, ed.locale, language) {
                            r.on_hover_text(desc);
                        }
                    }
                    if let Some(desc) = ed.autocomplete.get(ed.ac_index).and_then(|n| crate::formula_locale::description(n, ed.locale, language)) {
                        ui.separator();
                        ui.add(egui::Label::new(egui::RichText::new(desc).small()).wrap());
                    }
                    if let Some(i) = pick {
                        ed.ac_index = i;
                        ed.accept_autocomplete();
                        ed.request_focus = true;
                    }
                } else if let Some((name, arg)) = ed.current_function()
                    && let Some(sig) = crate::formula_locale::signature(&name, ed.locale, language)
                {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let (head, rest) = sig.split_once('(').unwrap_or((&sig, ""));
                        ui.label(egui::RichText::new(format!("{head}(")).font(theme::ui_font(12.0)));
                        let inner = rest.trim_end_matches(')');
                        let parts: Vec<&str> = inner.split(&format!("{} ", ed.locale.list_separator())).collect();
                        for (i, part) in parts.iter().enumerate() {
                            let is_cur = i == arg.min(parts.len().saturating_sub(1)) || (part.contains("...") && arg >= i);
                            let txt = egui::RichText::new(*part).font(if is_cur { theme::ui_bold(12.0) } else { theme::ui_font(12.0) });
                            ui.label(txt);
                            if i + 1 < parts.len() {
                                ui.label(egui::RichText::new(format!("{} ", ed.locale.list_separator())).font(theme::ui_font(12.0)));
                            }
                        }
                        ui.label(egui::RichText::new(")").font(theme::ui_font(12.0)));
                    });
                    // The description too, so values an argument takes (DATEDIF's units, #363) show
                    // while it's typed; wrapped to the hint's width.
                    if let Some(desc) = crate::formula_locale::description(&name, ed.locale, language) {
                        ui.set_max_width(ui.min_rect().width().max(220.0));
                        ui.separator();
                        ui.add(egui::Label::new(egui::RichText::new(desc).small()).wrap());
                    }
                }
            });
        });
    }
    app.editor = Some(ed);
    if cancel {
        app.cancel_edit();
    } else if let Some((dr, dc, array, fill)) = action {
        if arrow {
            app.commit_edit_arrow(dr, dc);
        } else {
            app.commit_edit(dr, dc, array, fill);
        }
    }
}

fn cycle_anchor(ed: &mut crate::editor::EditState) {
    let caret_b = ed.text.char_indices().nth(ed.caret).map(|(i, _)| i).unwrap_or(ed.text.len());
    for (a, b, ..) in formula_refs(&ed.text) {
        if a <= caret_b && caret_b <= b {
            let Some(r) = ed.text.get(a..b) else { return };
            let cycled: String = r
                .split(':')
                .map(|part| {
                    let p = part.split('!').next_back().unwrap_or(part);
                    let prefix = &part[..part.len() - p.len()];
                    let bare = p.replace('$', "");
                    let letters: String = bare.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
                    let digits = &bare[letters.len()..];
                    let state = (p.starts_with('$'), p[letters.len().min(p.len())..].contains('$') || p.trim_start_matches('$').contains('$'));
                    let next = match state {
                        (false, false) => format!("${letters}${digits}"),
                        (true, true) => format!("{letters}${digits}"),
                        (false, true) => format!("${letters}{digits}"),
                        (true, false) => format!("{letters}{digits}"),
                    };
                    format!("{prefix}{next}")
                })
                .collect::<Vec<_>>()
                .join(":");
            let (Some(x), Some(y)) = (ed.text.get(..a), ed.text.get(b..)) else { return };
            ed.text = format!("{x}{cycled}{y}");
            ed.caret = ed.text.get(..a + cycled.len()).map(|s| s.chars().count()).unwrap_or(ed.caret);
            ed.caret_to_end = false;
            ed.request_focus = true;
            ed.sync_caret = true;
            return;
        }
    }
}

fn function_names() -> Vec<String> {
    static NAMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    NAMES
        .get_or_init(|| gridcraft_engine::cmd::formulas::function_list().iter().filter_map(|f| f["name"].as_str().map(str::to_string)).collect())
        .clone()
}

fn function_info(name: &str) -> Option<serde_json::Value> {
    static LIST: std::sync::OnceLock<Vec<serde_json::Value>> = std::sync::OnceLock::new();
    LIST.get_or_init(gridcraft_engine::cmd::formulas::function_list)
        .iter()
        .find(|f| f["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(name)))
        .cloned()
}

pub fn function_signature(name: &str) -> Option<String> {
    function_info(name).and_then(|f| f["signature"].as_str().map(str::to_string))
}

pub fn function_description(name: &str) -> Option<String> {
    function_info(name).and_then(|f| f["description"].as_str().map(str::to_string))
}

#[cfg(test)]
mod spill_tests {
    use super::*;
    use gridcraft_engine::Session;

    fn app() -> SheetApp {
        let mut s = Session::new();
        s.new_workbook();
        SheetApp::new(s, Default::default())
    }

    fn set(app: &mut SheetApp, cell: &str, input: &str) {
        app.run("cell.set", json!({"cell": cell, "input": input})).unwrap();
    }

    fn select(app: &mut SheetApp, cell: &str) {
        app.cancel_edit();
        app.run("selection.set", json!({"cell": cell})).unwrap();
    }

    fn check(app: &mut SheetApp, cell: &str, text: &str, read_only: bool) {
        select(app, cell);
        assert_eq!(active_input(app), text);
        assert_eq!(active_spill_anchor(app).is_some(), read_only);
        app.begin_edit(None, true);
        assert_eq!(app.editor.is_none(), read_only);
        app.cancel_edit();
        if read_only {
            app.begin_edit(Some("=SUM(1,2)".into()), true);
            assert!(app.editor.is_none());
        }
    }

    #[test]
    fn selection_restores_editing_and_keeps_spill_formulas_only_at_anchor() {
        let mut app = app();
        set(&mut app, "A1", "=SEQUENCE(5,1,10,10)");
        set(&mut app, "D1", "=SUM(1,2)");
        set(&mut app, "D2", "hello");
        for cell in ["A2", "A3", "A4", "A5"] {
            check(&mut app, cell, "=SEQUENCE(5,1,10,10)", true);
        }
        check(&mut app, "A1", "=SEQUENCE(5,1,10,10)", false);
        check(&mut app, "D1", "=SUM(1,2)", false);
        check(&mut app, "D2", "hello", false);
        check(&mut app, "E1", "", false);
        let sh = app.session.active().unwrap().wb.active().unwrap();
        assert!(sh.cell(gridcraft_engine::core::CellRef::parse("A2").unwrap()).and_then(|c| c.formula).is_none());
        // Intentional grid replacement retains the existing entry behavior.
        select(&mut app, "A2");
        app.begin_edit(Some("replacement".into()), false);
        assert!(app.editor.is_some());
    }

    #[test]
    fn spill_ownership_follows_recalculation_and_obstructions() {
        let mut app = app();
        set(&mut app, "D1", "2");
        set(&mut app, "A1", "=SEQUENCE(D1,2)");
        check(&mut app, "B2", "=SEQUENCE(D1,2)", true);
        set(&mut app, "D1", "4");
        check(&mut app, "B4", "=SEQUENCE(D1,2)", true);
        set(&mut app, "D1", "1");
        check(&mut app, "B4", "", false);
        check(&mut app, "B1", "=SEQUENCE(D1,2)", true);
        set(&mut app, "A1", "");
        check(&mut app, "B1", "", false);
        set(&mut app, "B2", "blocked");
        set(&mut app, "A1", "=SEQUENCE(3,2)");
        check(&mut app, "A2", "", false);
        check(&mut app, "B2", "blocked", false);
        set(&mut app, "B2", "");
        check(&mut app, "B2", "=SEQUENCE(3,2)", true);
    }

    #[test]
    fn formatted_children_and_other_dynamic_array_functions() {
        let mut app = app();
        app.run("home.bold", json!({"range": "A2"})).unwrap();
        set(&mut app, "D1", "3");
        set(&mut app, "D2", "1");
        set(&mut app, "D3", "3");
        for formula in ["=SORT(D1:D3)", "=UNIQUE(D1:D3)", "=FILTER(D1:D3,D1:D3>0)"] {
            set(&mut app, "A1", formula);
            check(&mut app, "A2", formula, true);
        }
        app.run("home.insertSheet", json!({"name": "Other"})).unwrap();
        set(&mut app, "A1", "=SORT(Sheet1!D1:D3)");
        check(&mut app, "A2", "=SORT(Sheet1!D1:D3)", true);
        app.run("sheet.activate", json!({"sheet": "Sheet1"})).unwrap();
        check(&mut app, "A2", "=FILTER(D1:D3,D1:D3>0)", true);
    }
    #[test]
    fn formula_bar_click_and_theme_presentation() {
        use egui_kittest::kittest::Queryable as _;
        for dark in [false, true] {
            let mut app = app();
            set(&mut app, "A1", "=SEQUENCE(3)");
            select(&mut app, "A2");
            let mut fonts_ready = false;
            let mut h = egui_kittest::Harness::builder().with_size(vec2(900.0, 100.0)).build_ui_state(
                move |ui, app: &mut SheetApp| {
                    if !fonts_ready {
                        ui.ctx().set_fonts(theme::font_definitions());
                        fonts_ready = true;
                        return;
                    }
                    ui.ctx().set_visuals(if dark { egui::Visuals::dark() } else { egui::Visuals::light() });
                    show(app, ui);
                },
                app,
            );
            h.run_steps(3);
            let disabled = Tokens::get(&h.ctx).text_disabled;
            assert!(h.output().shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t)
                if t.galley.job.text == "=SEQUENCE(3)" && t.galley.job.sections.iter().all(|section| section.format.color == disabled))));
            h.get_by_label("=SEQUENCE(3)").click();
            h.run_steps(3);
            assert!(h.state().editor.is_none());
            select(h.state_mut(), "A1");
            h.run_steps(3);
            h.get_by_label("=SEQUENCE(3)").click();
            h.run_steps(3);
            assert!(h.state().editor.as_ref().is_some_and(|e| e.from_formula_bar));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(key: Key) -> egui::RawInput {
        let mut raw = egui::RawInput::default();
        raw.events.push(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE });
        raw
    }

    #[test]
    fn backspace_and_delete_dismiss_completion() {
        let ctx = egui::Context::default();
        for key in [Key::Backspace, Key::Delete] {
            let mut completion = Some("SomeText".to_string());
            let mut out = ctx.run_ui(press(key), |ui| ui.input_mut(|i| dismiss_completion(i, &mut completion)));
            out.textures_delta.clear(); // nothing consumes the atlas in a headless test
            assert_eq!(completion, None, "{key:?} must drop the proposal");
        }
        let mut completion = Some("SomeText".to_string());
        let mut out = ctx.run_ui(press(Key::A), |ui| ui.input_mut(|i| dismiss_completion(i, &mut completion)));
        out.textures_delta.clear();
        assert!(completion.is_some());
    }

    #[test]
    fn only_typing_proposes_completion() {
        assert!(!entry_shrank(4, "SomeT"));
        // Backspacing "Some" to "Som" must not bring "SomeText" back.
        assert!(entry_shrank(4, "Som"));
        assert!(!entry_shrank(4, "Same"));
    }
}
