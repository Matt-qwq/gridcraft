//! Sheet tabs and the status bar.

use std::sync::Arc;

use egui::{Align2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use gridcraft_engine::model::{Sheet, Visibility};
use serde_json::json;

use crate::SheetApp;
use crate::icons::{self, Icon};
use crate::theme::{self, Tokens};

/// Space a chart sheet's tab gives its chart glyph before the name, in screen points.
const TAB_GLYPH: f32 = 16.0;

/// The rename field asks for focus only while it has not got it yet. Asking again on the frame it lost focus (Enter or a click elsewhere)
/// would keep the field focused after it is closed, so the keyboard stayed stuck in the sheet name.
fn rename_needs_focus(has_focus: bool, lost_focus: bool) -> bool {
    !has_focus && !lost_focus
}

/// The status bar's Accessibility entry: its label and the issue count from
/// `review.checkAccessibility`. The count is cached per document revision, because the check walks
/// every sheet's cells and objects and the status bar runs every frame.
fn accessibility_status(app: &mut SheetApp) -> (String, usize) {
    let Some(key) = app.session.active().map(|d| (d.uid, d.revision)) else {
        return (app.ui.language.tr("Accessibility").to_string(), 0);
    };
    if app.grid.a11y.as_ref().is_none_or(|(k, _)| *k != key) {
        let n = app.session.run("review.checkAccessibility", json!({})).ok().and_then(|v| v.as_array().map(Vec::len)).unwrap_or(0);
        app.grid.a11y = Some((key, n));
    }
    let n = app.grid.a11y.as_ref().map(|(_, n)| *n).unwrap_or(0);
    // No new string for the empty case: the entry reads "Accessibility" alone when there is
    // nothing to fix, and "Accessibility: N" otherwise (Excel shows "Good to go" instead).
    let label = if n == 0 { app.ui.language.tr("Accessibility").to_string() } else { format!("{}: {n}", app.ui.language.tr("Accessibility")) };
    (label, n)
}

/// Rows of the All Sheets navigator as `(workbook index, name)`. Hidden sheets stay out, like on the tab strip:
/// Unhide is the way to reach them.
fn navigator_items(sheets: &[Arc<Sheet>]) -> Vec<(String, String)> {
    sheets.iter().enumerate().filter(|(_, s)| s.visibility == Visibility::Visible).map(|(i, s)| (i.to_string(), s.name.clone())).collect()
}

/// Switch to sheet `i` from the tab strip or the navigator. In point mode the formula being typed keeps going on the
/// other sheet; otherwise the edit is committed first.
fn activate_sheet(app: &mut SheetApp, i: usize) {
    if app.editor.as_ref().is_some_and(|e| e.can_point()) {
        let _ = app.session.run("sheet.activate", json!({"sheet": i}));
    } else {
        if app.editor.is_some() && !app.commit_edit(0, 0, false, false) {
            return;
        }
        app.run_or_alert("sheet.activate", json!({"sheet": i}));
    }
}

/// A tab released with Ctrl held (Cmd on macOS, as for the fill handle) drops a copy of the sheet. Excel for Mac uses
/// Option for this, so Option copies there too.
fn drop_copies_sheet(m: egui::Modifiers, mac: bool) -> bool {
    m.command || (mac && m.alt)
}

/// `sheet.move` params for tab `i` dropped `steps` tabs away, or `None` when it lands where it started.
fn tab_drop(i: usize, steps: i64, copy: bool) -> Option<serde_json::Value> {
    if steps == 0 {
        return None;
    }
    let to = (i as i64 + steps).max(0) as usize;
    if copy {
        // A move indexes the list without the sheet, a copy the list with it: to the right, the copy goes one further.
        let to = if steps > 0 { to + 1 } else { to };
        return Some(json!({"sheet": i, "to": to, "copy": true}));
    }
    Some(json!({"sheet": i, "to": to}))
}

pub fn sheet_tabs(app: &mut SheetApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Panel::bottom("sheet_tabs").exact_size(32.0).frame(egui::Frame::NONE.fill(t.tab_bar)).show(ui, |ui| {
        let rect = ui.max_rect();
        ui.painter().line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, t.ribbon_border));
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.add_space(8.0);
            if crate::widgets::icon_button(ui, Icon::Left, t.text_dim, app.ui.language.tr("Previous sheet"), vec2(24.0, 24.0)).clicked() {
                app.run_or_alert("sheet.previous", json!({}));
            }
            if crate::widgets::icon_button(ui, Icon::Right, t.text_dim, app.ui.language.tr("Next sheet"), vec2(24.0, 24.0)).clicked() {
                app.run_or_alert("sheet.next", json!({}));
            }
            let all = crate::widgets::icon_button(ui, Icon::SheetList, t.text_dim, app.ui.language.tr("All sheets"), vec2(24.0, 24.0));
            all.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, app.ui.language.tr("All sheets")));
            let items = app.session.active().map(|d| navigator_items(&d.wb.sheets)).unwrap_or_default();
            let active = app.session.active().map(|d| d.wb.active_sheet.to_string()).unwrap_or_default();
            let rows: Vec<(&str, String)> = items.iter().map(|(i, name)| (i.as_str(), name.clone())).collect();
            if let crate::ribbon::ComboResult::Picked(i) = crate::ribbon::editable_combo(ui, &all, "sheet_navigator", &active, &rows, false)
                && let Ok(i) = i.parse::<usize>()
            {
                activate_sheet(app, i);
            }
            ui.add_space(8.0);
            let Some(d) = app.session.active() else { return };
            let sheets: Vec<(usize, String, Option<[u8; 3]>, bool, bool)> =
                d.wb.sheets
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.visibility == Visibility::Visible)
                    .map(|(i, s)| (i, s.name.clone(), s.tab_color.and_then(|c| c.resolve(&d.wb.theme)), i == d.wb.active_sheet, s.is_chart_sheet()))
                    .collect();
            // Keep worksheet tabs in their own horizontally scrollable viewport. As in Excel, the
            // tabs get up to 60% of the bar and the grid's horizontal scrollbar the rest; a strip
            // that needs less shrinks to its tabs, so New sheet (+) sits right after the last one.
            let tabs_width = (ui.available_width() * 0.6).max(100.0);
            // Reveal the selected sheet after navigation or layout changes, but
            // allow browsing other tabs without snapping back on every frame.
            let reveal_key = egui::Id::new((d.uid, d.wb.active_sheet, &sheets, tabs_width.to_bits()));
            let reveal_id = ui.id().with("worksheet_tabs_reveal");
            let reveal_active = ui.ctx().data_mut(|data| {
                let changed = data.get_temp::<egui::Id>(reveal_id) != Some(reveal_key);
                data.insert_temp(reveal_id, reveal_key);
                changed
            });
            ui.allocate_ui_with_layout(vec2(tabs_width, 30.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                // Translate vertical wheel movement into horizontal
                // worksheet-tab scrolling.
                let wheel_scroll = if ui.rect_contains_pointer(ui.max_rect()) {
                    ui.input(|i| {
                        let d = i.smooth_scroll_delta;
                        if d.x.abs() > d.y.abs() { d.x } else { d.y }
                    })
                } else {
                    0.0
                };

                egui::ScrollArea::horizontal()
                    .id_salt("worksheet_tabs_horizontal")
                    .auto_shrink([true, false])
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .show(ui, |ui| {
                        if wheel_scroll.abs() > 0.0 {
                            ui.scroll_with_delta(vec2(wheel_scroll, 0.0));
                        }
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            for (i, name, color, active, chart_sheet) in sheets {
                                let renaming = app.grid.renaming_tab == Some(i);
                                let font = if active { theme::ui_bold(13.0) } else { theme::ui_font(13.0) };
                                let tw = ui.painter().layout_no_wrap(name.clone(), font.clone(), t.text).size().x;
                                // A chart sheet (one chart, no cells) is marked like Excel marks it,
                                // with a chart glyph before its name.
                                let glyph = if chart_sheet { TAB_GLYPH } else { 0.0 };
                                let w = (tw + glyph + 32.0).max(72.0);
                                let (r, resp) = ui.allocate_exact_size(vec2(w, 30.0), Sense::click_and_drag());
                                // The tab's name is what a screen reader reads for it; a chart
                                // sheet says so, as its glyph does on screen.
                                let label = if chart_sheet { format!("{}, {}", name, app.ui.language.tr("Chart sheet")) } else { name.clone() };
                                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
                                if active && reveal_active {
                                    resp.scroll_to_me(None);
                                }
                                let r = r.shrink2(vec2(0.0, 1.0));
                                if active {
                                    ui.painter().rect_filled(Rect::from_min_max(pos2(r.left(), r.top() - 2.0), r.max), 4.0, t.tab_active);
                                    ui.painter().rect_stroke(
                                        Rect::from_min_max(pos2(r.left(), r.top() - 2.0), r.max),
                                        4.0,
                                        Stroke::new(1.0, t.ribbon_border),
                                        StrokeKind::Inside,
                                    );
                                } else if resp.hovered() {
                                    ui.painter().rect_filled(r, 4.0, t.hover);
                                }
                                if let Some(c) = color {
                                    let band = Rect::from_min_size(pos2(r.left() + 6.0, r.bottom() - 4.0), vec2(r.width() - 12.0, 3.0));
                                    ui.painter().rect_filled(band, 1.0, theme::color32(c));
                                } else if active {
                                    let band = Rect::from_center_size(pos2(r.center().x, r.bottom() - 3.0), vec2(tw.min(48.0), 2.5));
                                    ui.painter().rect_filled(band, 1.0, t.accent);
                                }
                                if renaming {
                                    let mut text = app.grid.rename_text.clone();
                                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink2(vec2(6.0, 5.0))));
                                    let ed =
                                        child.add(egui::TextEdit::singleline(&mut text).font(theme::ui_font(13.0)).desired_width(r.width() - 12.0));
                                    app.grid.rename_text = text.clone();
                                    if rename_needs_focus(ed.has_focus(), ed.lost_focus()) {
                                        ed.request_focus();
                                    }
                                    if ed.lost_focus() {
                                        if !ui.input(|i| i.key_pressed(egui::Key::Escape)) && text != name {
                                            app.run_or_alert("sheet.rename", json!({"sheet": i, "name": text}));
                                        }
                                        app.grid.renaming_tab = None;
                                    }
                                } else {
                                    let ink = if active { t.accent } else { t.text };
                                    if chart_sheet {
                                        let glyph_rect =
                                            Rect::from_center_size(pos2(r.center().x - tw / 2.0 - glyph / 4.0, r.center().y), vec2(14.0, 14.0));
                                        icons::paint(ui.painter(), glyph_rect, Icon::Chart, if active { t.accent } else { t.text_dim });
                                    }
                                    ui.painter().text(pos2(r.center().x + glyph / 4.0, r.center().y), Align2::CENTER_CENTER, &name, font, ink);
                                }
                                // A right-click activates the tab too, as in Excel, so the menu's sheet commands (Protect
                                // Sheet, Select All Sheets) act on the sheet that was clicked rather than the one that was active.
                                if resp.clicked() || (resp.secondary_clicked() && !active) {
                                    activate_sheet(app, i);
                                }
                                if resp.double_clicked() {
                                    app.grid.renaming_tab = Some(i);
                                    app.grid.rename_text = name.clone();
                                }
                                // Drag to reorder; with the copy modifier held, drop a copy instead.
                                let copy = drop_copies_sheet(ui.input(|inp| inp.modifiers), ui.ctx().os() == egui::os::OperatingSystem::Mac);
                                if resp.dragged() && copy {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::Copy);
                                }
                                if resp.drag_stopped()
                                    && let Some(p) = ui.input(|inp| inp.pointer.interact_pos())
                                {
                                    let dx = p.x - r.center().x;
                                    let steps = (dx / w).round() as i64;
                                    if let Some(params) = tab_drop(i, steps, copy) {
                                        app.run_or_alert("sheet.move", params);
                                    }
                                }
                                resp.context_menu(|ui| {
                                    ui.set_min_width(180.0);
                                    if ui.button(app.ui.language.tr("Insert Sheet")).clicked() {
                                        app.run_or_alert("home.insertSheet", json!({"before": i}));
                                    }
                                    if ui.button(app.ui.language.tr("Delete")).clicked() {
                                        app.run_or_alert("home.deleteSheet", json!({"sheet": i}));
                                    }
                                    if ui.button(app.ui.language.tr("Rename")).clicked() {
                                        app.grid.renaming_tab = Some(i);
                                        app.grid.rename_text = name.clone();
                                    }
                                    if ui.button(app.ui.language.tr("Move or Copy…")).clicked() {
                                        app.open_dialog("moveSheet", json!({"sheet": i}));
                                    }
                                    if ui.button(app.ui.language.tr("Duplicate")).clicked() {
                                        app.run_or_alert("sheet.move", json!({"sheet": i, "to": i + 1, "copy": true}));
                                    }
                                    ui.menu_button(app.ui.language.tr("Tab Color"), |ui| {
                                        let colors = app.session.active().map(|d| d.wb.theme.colors).unwrap_or_default();
                                        if let Some(c) = crate::widgets::color_palette_fixed(ui, &colors, app.ui.language.tr("No Color")) {
                                            app.run_or_alert("sheet.tabColor", json!({"sheet": i, "color": c}));
                                            ui.close();
                                        }
                                    });
                                    ui.separator();
                                    if ui.button(app.ui.language.tr("Hide")).clicked() {
                                        app.run_or_alert("sheet.hide", json!({"sheet": i}));
                                    }
                                    if ui.button(app.ui.language.tr("Unhide…")).clicked() {
                                        app.run_or_alert("sheet.unhide", json!({}));
                                    }
                                    ui.separator();
                                    let protected = app.active_sheet_protected();
                                    if ui
                                        .button(if protected {
                                            app.ui.language.tr("Unprotect Sheet…")
                                        } else {
                                            app.ui.language.tr("Protect Sheet…")
                                        })
                                        .clicked()
                                    {
                                        app.open_dialog("protectSheet", json!({}));
                                    }
                                    if ui.button(app.ui.language.tr("Select All Sheets")).clicked() {
                                        ui.close();
                                    }
                                });
                            }
                        });
                    });
            });
            ui.add_space(6.0);
            if crate::widgets::icon_button(ui, Icon::Plus, t.text_dim, app.ui.language.tr("New sheet (⇧F11)"), vec2(26.0, 26.0)).clicked() {
                app.run_or_alert("home.insertSheet", json!({}));
            }
            // The track ends where the grid does, so it stops under the vertical scrollbar.
            let avail = ui.available_rect_before_wrap();
            let right = app.grid.rect.map_or(avail.right(), |r| r.right()).min(avail.right());
            let track = Rect::from_min_max(pos2(avail.left() + 8.0, avail.center().y - 7.0), pos2(right, avail.center().y + 7.0));
            crate::grid::horizontal_scrollbar(app, ui, track, &t);
        });
    });
}

pub fn status_bar(app: &mut SheetApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Panel::bottom("status_bar").exact_size(26.0).frame(egui::Frame::NONE.fill(t.status_bar).inner_margin(egui::Margin::symmetric(12, 0))).show(
        ui,
        |ui| {
            ui.horizontal_centered(|ui| {
                let mode = match app.session.mode {
                    gridcraft_engine::Mode::Ready => "Ready",
                    gridcraft_engine::Mode::Enter => "Enter",
                    gridcraft_engine::Mode::Edit => "Edit",
                    gridcraft_engine::Mode::Point => "Point",
                };
                ui.label(egui::RichText::new(app.ui.language.tr(mode)).font(theme::ui_font(12.5)).color(t.text_dim));
                if let Some(d) = app.session.active() {
                    if d.wb.vba_project.is_some() {
                        ui.add_space(12.0);
                        ui.label(
                            egui::RichText::new(crate::widgets::tr(ui, "Macros preserved (not executed)")).font(theme::ui_font(12.5)).color(t.danger),
                        );
                    }
                    if d.wb.calc.mode == gridcraft_engine::model::CalcMode::Manual {
                        ui.add_space(12.0);
                        ui.label(egui::RichText::new(app.ui.language.tr("Calculate")).font(theme::ui_font(12.5)).color(t.text_dim));
                    }
                    if !d.calc.circular.is_empty() {
                        ui.add_space(12.0);
                        let (si, c) = d.calc.circular[0];
                        let name = d.wb.sheet(si).map(|s| s.name.clone()).unwrap_or_default();
                        let text = crate::widgets::tr(ui, "Circular References: {name}!{}").replace("{name}", &name).replace("{}", &c.a1());
                        ui.label(egui::RichText::new(text).font(theme::ui_font(12.5)).color(t.danger));
                    }
                    if app.session.clipboard.is_some() && app.editor.is_none() {
                        ui.add_space(12.0);
                        ui.label(
                            egui::RichText::new(app.ui.language.tr("Select destination and press Enter or choose Paste"))
                                .font(theme::ui_font(12.5))
                                .color(t.text_dim),
                        );
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Zoom percentage and slider.
                    let zoom = app.session.active().and_then(|d| d.wb.active().map(|s| s.zoom)).unwrap_or(100);
                    let zl = ui.add(
                        egui::Label::new(egui::RichText::new(format!("{zoom}%")).font(theme::ui_font(12.5)).color(t.text_dim)).sense(Sense::click()),
                    );
                    if zl.clicked() {
                        app.open_dialog("zoom", json!({}));
                    }
                    ui.add_space(4.0);
                    if crate::widgets::icon_button(ui, Icon::Plus, t.text_dim, "Zoom In", vec2(18.0, 18.0)).clicked() {
                        let z = ((zoom as f32 / 10.0).floor() * 10.0 + 10.0).min(400.0);
                        app.run_or_alert("view.zoom", json!({"percent": z}));
                    }
                    let mut z = zoom as f32;
                    let slider = ui.add(egui::Slider::new(&mut z, 10.0..=400.0).show_value(false).logarithmic(true));
                    if slider.changed() {
                        let _ = app.session.run("view.zoom", json!({"percent": z.round()}));
                    }
                    if crate::widgets::icon_button(ui, Icon::Minus, t.text_dim, "Zoom Out", vec2(18.0, 18.0)).clicked() {
                        let z = ((zoom as f32 / 10.0).ceil() * 10.0 - 10.0).max(10.0);
                        app.run_or_alert("view.zoom", json!({"percent": z}));
                    }
                    ui.add_space(10.0);
                    for (icon, tip, mode, cmd) in [
                        (Icon::PageBreak, "Page Break Preview", "pageBreakPreview", "view.pageBreakPreview"),
                        (Icon::PageLayout, "Page Layout", "pageLayout", "view.pageLayout"),
                        (Icon::Normal, "Normal", "normal", "view.normal"),
                    ] {
                        let (r, resp) = ui.allocate_exact_size(vec2(26.0, 20.0), Sense::click());
                        let cur = app.session.active().and_then(|d| d.wb.active().map(|s| s.view.as_str())).unwrap_or("normal");
                        if cur == mode {
                            ui.painter().rect_filled(r, 3.0, t.pressed);
                        } else if resp.hovered() {
                            ui.painter().rect_filled(r, 3.0, t.hover);
                        }
                        icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(14.0, 14.0)), icon, t.text_dim);
                        if resp.on_hover_text(crate::widgets::tr(ui, tip)).clicked() {
                            app.run_or_alert(cmd, json!({}));
                        }
                    }
                    ui.add_space(10.0);
                    // Accessibility: the issue count, opening the same Check Accessibility dialog
                    // the Review tab does (Excel shows this entry in its status bar too).
                    let (label, issues) = accessibility_status(app);
                    let color = if issues == 0 { t.text_dim } else { t.danger };
                    let entry = egui::Button::new(egui::RichText::new(label).font(theme::ui_font(12.5)).color(color)).frame(false);
                    if ui.add(entry).on_hover_text(app.ui.language.tr("Check Accessibility")).clicked() {
                        app.open_dialog("accessibility", json!({}));
                    }
                    ui.add_space(16.0);
                    // Average / Count / Sum.
                    if let Some(d) = app.session.active()
                        && let Some(sh) = d.wb.active()
                        && !d.selection.is_single_cell()
                    {
                        let st = gridcraft_engine::display::stats(sh, &d.selection.ranges);
                        let fmt = |v: f64| {
                            let code = d.wb.styles.get(sh.style_id(d.selection.active)).num_fmt.as_str().to_string();
                            let code = if code == "General" { "#,##0.##########".to_string() } else { code };
                            gridcraft_engine::display::format(&gridcraft_engine::core::Value::Number(v), &code, &d.wb)
                                .text
                                .trim_end_matches('.')
                                .to_string()
                        };
                        let mut parts = Vec::new();
                        if st.numeric > 0 {
                            parts.push(crate::widgets::tr(ui, "Sum: {}").replace("{}", &fmt(st.sum)));
                        }
                        if st.count > 0 {
                            parts.push(crate::widgets::tr(ui, "Count: {}").replace("{}", &st.count.to_string()));
                        }
                        if let Some(a) = st.average {
                            parts.push(crate::widgets::tr(ui, "Average: {}").replace("{}", &fmt(a)));
                        }
                        for p in parts {
                            ui.label(egui::RichText::new(p).font(theme::ui_font(12.5)).color(t.text_dim));
                            ui.add_space(10.0);
                        }
                    }
                });
            });
        },
    );
}

#[cfg(test)]
mod tests {
    use super::{activate_sheet, drop_copies_sheet, navigator_items, rename_needs_focus, tab_drop};
    use crate::SheetApp;
    use egui::Modifiers;
    use gridcraft_engine::model::{Sheet, Visibility};
    use serde_json::json;
    use std::sync::Arc;

    fn workbook_abc() -> SheetApp {
        let mut session = gridcraft_engine::Session::default();
        session.new_workbook();
        let mut app = SheetApp::new(session, Default::default());
        app.run("home.insertSheet", json!({})).unwrap();
        app.run("home.insertSheet", json!({})).unwrap();
        for (i, name) in ["A", "B", "C"].into_iter().enumerate() {
            app.run("sheet.rename", json!({"sheet": i, "name": name})).unwrap();
        }
        app
    }

    fn names(app: &SheetApp) -> Vec<String> {
        app.session.active().unwrap().wb.sheets.iter().map(|s| s.name.clone()).collect()
    }

    #[test]
    fn copy_drop_modifier_is_ctrl_or_cmd_or_option_on_mac() {
        // egui reports Ctrl as `command` too off macOS, and Cmd as `mac_cmd` and `command` on macOS.
        assert!(drop_copies_sheet(Modifiers::CTRL | Modifiers::COMMAND, false));
        assert!(!drop_copies_sheet(Modifiers::ALT, false));
        assert!(drop_copies_sheet(Modifiers::MAC_CMD | Modifiers::COMMAND, true));
        assert!(drop_copies_sheet(Modifiers::ALT, true));
        assert!(!drop_copies_sheet(Modifiers::CTRL, true), "Ctrl+click is the context menu on macOS");
        assert!(!drop_copies_sheet(Modifiers::NONE, false));
    }

    #[test]
    fn tab_dropped_with_the_copy_modifier_leaves_a_copy_where_a_move_would_land() {
        let mut app = workbook_abc();
        app.run("sheet.move", tab_drop(0, 1, true).unwrap()).unwrap();
        assert_eq!(names(&app), ["A", "B", "A (2)", "C"]);

        let mut app = workbook_abc();
        app.run("sheet.move", tab_drop(2, -1, true).unwrap()).unwrap();
        assert_eq!(names(&app), ["A", "C (2)", "B", "C"]);

        let mut app = workbook_abc();
        app.run("sheet.move", tab_drop(0, 1, false).unwrap()).unwrap();
        assert_eq!(names(&app), ["B", "A", "C"]);
        assert!(tab_drop(1, 0, true).is_none());
    }

    #[test]
    fn rename_field_takes_focus_once_and_lets_go_on_commit() {
        assert!(rename_needs_focus(false, false), "first frame: grab focus");
        assert!(!rename_needs_focus(true, false), "already focused: do not ask again");
        assert!(!rename_needs_focus(false, true), "Enter or click-away: must not take focus back");
    }

    #[test]
    fn navigator_lists_visible_sheets_by_workbook_index() {
        let mut hidden = Sheet::new("Hidden");
        hidden.visibility = Visibility::Hidden;
        let mut very_hidden = Sheet::new("VeryHidden");
        very_hidden.visibility = Visibility::VeryHidden;
        let sheets = [Sheet::new("Data"), hidden, very_hidden, Sheet::new("Summary")].map(Arc::new);
        assert_eq!(navigator_items(&sheets), vec![("0".to_string(), "Data".to_string()), ("3".to_string(), "Summary".to_string())]);
    }

    #[test]
    fn a_rejected_commit_keeps_the_sheet_and_the_edit() {
        let mut session = gridcraft_engine::Session::new();
        session.new_workbook();
        session.execute("home.insertSheet", json!({})).unwrap();
        session.execute("sheet.activate", json!({"sheet": 0})).unwrap();
        let rule = json!({"range": "A1", "type": "whole", "operator": "between", "formula1": "1", "formula2": "10"});
        session.execute("data.validation", rule).unwrap();
        let mut app = crate::SheetApp::new(session, Default::default());
        app.begin_edit(Some("99".into()), false);
        activate_sheet(&mut app, 1);
        assert_eq!(app.session.active().unwrap().wb.active_sheet, 0, "a value the validation stops must not leave its sheet");
        assert!(app.editor.is_some(), "the edit stays open to be corrected");
        app.editor.as_mut().unwrap().text = "7".into();
        activate_sheet(&mut app, 1);
        assert_eq!(app.session.active().unwrap().wb.active_sheet, 1, "the corrected value commits and the sheet changes");
        let a1 = gridcraft_engine::core::CellRef::parse("A1").unwrap();
        assert!(app.session.active().unwrap().wb.sheet(0).unwrap().cell(a1).is_some(), "the value lands on the first sheet");
    }
}

#[cfg(test)]
mod a11y_status_tests {
    use super::accessibility_status;
    use crate::SheetApp;
    use serde_json::json;

    fn app_with_sheets() -> SheetApp {
        let mut session = gridcraft_engine::Session::default();
        session.new_workbook();
        SheetApp::new(session, Default::default())
    }

    #[test]
    fn the_status_bar_runs_the_accessibility_check() {
        let mut app = app_with_sheets();
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        let mut out = ctx.run_ui(Default::default(), |ui| super::status_bar(&mut app, ui));
        out.textures_delta.clear(); // nothing consumes the atlas in a headless test
        assert!(app.grid.a11y.is_some(), "the bar asked for the issue count");
    }

    #[test]
    fn the_status_bar_entry_counts_issues_once_per_revision() {
        let mut app = app_with_sheets();
        // A default sheet name is an issue of its own; name it first.
        app.run("sheet.rename", json!({"sheet": 0, "name": "Data"})).unwrap();
        // A merged range is one of the issues `review.checkAccessibility` reports.
        app.run("selection.set", json!({"range": "A1:B2"})).unwrap();
        app.run("home.mergeCells", json!({})).unwrap();
        let (label, issues) = accessibility_status(&mut app);
        assert!(issues >= 1, "{label}");
        assert!(label.contains(&issues.to_string()), "{label}");
        // The count is cached: a second frame without an edit does not run the command again.
        let revision = app.session.active().unwrap().revision;
        let cached = app.grid.a11y;
        assert!(cached.is_some());
        assert_eq!(accessibility_status(&mut app).1, issues);
        assert_eq!(app.grid.a11y, cached);
        assert_eq!(app.session.active().unwrap().revision, revision);
        // An edit invalidates it, and a sheet with no issues says so instead of "0".
        app.run("home.unmergeCells", json!({})).unwrap();
        let (label, issues) = accessibility_status(&mut app);
        assert_eq!(issues, 0, "{label}");
        assert_eq!(label, "Accessibility", "nothing to fix: no count");
    }
}
