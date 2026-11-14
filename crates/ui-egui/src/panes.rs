//! Right-hand task panes: Comments, Watch Window, Selection Pane, Format Chart.

use egui::{Color32, Ui, vec2};
use serde_json::{Value as Json, json};

use crate::SheetApp;
use crate::theme::{self, Tokens};

pub fn show(app: &mut SheetApp, ui: &mut Ui) {
    // The Comments pane is engine state (`review.showComments`, so agents can drive it); the other
    // panes are UI state. One pane shows at a time: opening another one closes Comments.
    if app.grid.pane.as_deref() == Some("comments") {
        app.grid.pane = None;
        app.run_or_alert("review.showComments", json!({"on": true}));
    } else if app.grid.pane.is_some() && app.session.comments_pane {
        app.run_or_alert("review.showComments", json!({"on": false}));
    }
    let Some(pane) = app.grid.pane.clone().or_else(|| app.session.comments_pane.then(|| "comments".to_string())) else { return };
    let t = Tokens::get(ui.ctx());
    // Capped at half the window so a pane can never cover the sheet, whatever its content asks for.
    let max = (ui.available_width() / 2.0).max(300.0);
    let panel = egui::Panel::right("task_pane").default_size(300.0).max_size(max).resizable(true);
    panel.frame(egui::Frame::NONE.fill(t.ribbon).inner_margin(10)).show(ui, |ui| {
        ui.horizontal(|ui| {
            let title = match pane.as_str() {
                "comments" => "Comments",
                "watch" => "Watch Window",
                "selection" => "Selection",
                "formatChart" => format_pane_title(app),
                _ => "",
            };
            ui.label(egui::RichText::new(crate::widgets::tr(ui, title)).font(theme::ui_bold(15.0)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if crate::widgets::icon_button(ui, crate::icons::Icon::Close, t.text_dim, "Close", vec2(22.0, 22.0)).clicked() {
                    if pane == "comments" {
                        app.run_or_alert("review.showComments", json!({"on": false}));
                    }
                    app.grid.pane = None;
                }
            });
        });
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| match pane.as_str() {
            "comments" => comments(app, ui),
            "watch" => watch(app, ui),
            "selection" => selection(app, ui),
            "formatChart" => format_chart(app, ui),
            _ => {}
        });
    });
}

/// The Format pane's title: the same pane serves a chart, a shape and a picture, and Excel names it
/// after the selected object.
fn format_pane_title(app: &SheetApp) -> &'static str {
    let kind =
        app.session.active().and_then(|d| d.wb.active()).and_then(|sh| crate::grid::selected_object(sh, app.selected_chart)).map(|(kind, _, _)| kind);
    title_of(kind)
}

/// The title for a selected object kind (`grid::selected_object`'s first field).
fn title_of(kind: Option<&str>) -> &'static str {
    match kind {
        Some("shape") => "Format Shape",
        Some("image") => "Format Picture",
        _ => "Format Chart Area",
    }
}

fn comments(app: &mut SheetApp, ui: &mut Ui) {
    let Some(d) = app.session.active() else { return };
    let Some(sh) = d.wb.active() else { return };
    let list: Vec<(String, gridcraft_engine::model::Comment)> = sh.comments.iter().map(|(c, m)| (c.a1(), m.clone())).collect();
    if list.is_empty() {
        ui.label(egui::RichText::new(crate::widgets::tr(ui, "No comments on this sheet yet. Select a cell and choose New Comment.")).italics());
    }
    for (cell, c) in list {
        egui::Frame::NONE
            .fill(if c.resolved { Color32::from_gray(240) } else { Color32::WHITE })
            .corner_radius(6.0)
            .inner_margin(8.0)
            .stroke(egui::Stroke::new(1.0, Color32::from_gray(215)))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    if ui.link(egui::RichText::new(&cell).strong()).clicked() {
                        let _ = app.run("selection.set", json!({"cell": cell}));
                        app.grid.ensure_visible = true;
                    }
                    ui.label(egui::RichText::new(&c.author).color(Color32::from_gray(90)));
                    if c.resolved {
                        ui.label(egui::RichText::new(crate::widgets::tr(ui, "Resolved")).small().color(Color32::from_rgb(0x10, 0x7C, 0x41)));
                    }
                });
                ui.label(&c.text);
                for (a, txt) in &c.replies {
                    ui.label(egui::RichText::new(format!("↳ {a}: {txt}")).color(Color32::from_gray(60)));
                }
                let key = egui::Id::new(("reply", &cell));
                let mut draft: String = ui.ctx().data_mut(|m| m.get_temp(key)).unwrap_or_default();
                // Buttons first, right to left, so the reply field gets exactly the width they leave:
                // a field sized by a guess wider than the row pushes the resizable pane wider every frame.
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button(crate::widgets::tr(ui, "Delete")).clicked() {
                            app.run_or_alert("review.deleteComment", json!({"cell": cell}));
                        }
                        if ui.small_button(crate::widgets::tr(ui, if c.resolved { "Reopen" } else { "Resolve" })).clicked() {
                            app.run_or_alert("review.resolveComment", json!({"cell": cell, "resolved": !c.resolved}));
                        }
                        let send = ui.small_button(crate::widgets::tr(ui, "Send")).clicked();
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut draft).hint_text(crate::widgets::tr(ui, "Reply…")).desired_width(ui.available_width()),
                        );
                        if (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) || send) && !draft.is_empty() {
                            app.run_or_alert("review.replyComment", json!({"cell": cell, "text": draft}));
                            draft.clear();
                        }
                    });
                });
                ui.ctx().data_mut(|m| m.insert_temp(key, draft));
            });
        ui.add_space(6.0);
    }
}

fn watch(app: &mut SheetApp, ui: &mut Ui) {
    if ui.button(crate::widgets::tr(ui, "Add Watch (active cell)")).clicked()
        && let Some(d) = app.session.active()
        && let Some(sh) = d.wb.active()
    {
        let r = format!("{}!{}", gridcraft_engine::formula::quote_sheet(&sh.name), d.selection.active.a1());
        let _ = app.session.run("formulas.watchWindow", json!({"add": r}));
    }
    let list = app.session.run("formulas.watchWindow", json!({})).unwrap_or(Json::Null);
    egui::Grid::new("watch_grid").striped(true).num_columns(4).show(ui, |ui| {
        ui.strong(crate::widgets::tr(ui, "Cell"));
        ui.strong(crate::widgets::tr(ui, "Value"));
        ui.strong(crate::widgets::tr(ui, "Formula"));
        ui.label("");
        ui.end_row();
        for w in list.as_array().cloned().unwrap_or_default() {
            let cell = w["cell"].as_str().unwrap_or("").to_string();
            ui.label(&cell);
            ui.label(w["value"].as_str().unwrap_or(""));
            ui.label(w["formula"].as_str().unwrap_or(""));
            if ui.small_button("✕").clicked() {
                let _ = app.session.run("formulas.watchWindow", json!({"remove": cell}));
            }
            ui.end_row();
        }
    });
}

fn selection(app: &mut SheetApp, ui: &mut Ui) {
    let list = app.session.run("arrange.selectionPane", json!({})).unwrap_or(Json::Null);
    let items = list.as_array().cloned().unwrap_or_default();
    // PivotTable slicers and timelines live in their own sheet lists, which `arrange.selectionPane`
    // does not carry (the arrange commands stack charts, pictures and shapes only). They are read
    // from the sheet here so they can be selected and deleted from the same pane; bring forward /
    // send backward has no meaning for a widget outside that stacking order.
    let widgets: Vec<(String, u32)> = app
        .session
        .active()
        .and_then(|d| d.wb.active())
        .map(|sh| {
            sh.pivot_slicers
                .iter()
                .map(|s| (crate::widgets::tr(ui, "Slicer {}").replace("{}", &s.name), s.id))
                .chain(sh.pivot_timelines.iter().map(|t| (crate::widgets::tr(ui, "Timeline {}").replace("{}", &t.name), t.id)))
                .collect()
        })
        .unwrap_or_default();
    if items.is_empty() && widgets.is_empty() {
        ui.label(egui::RichText::new(crate::widgets::tr(ui, "There are no shapes, pictures, charts or slicers on this sheet.")).italics());
    }
    for it in items.iter().rev() {
        let kind = it["kind"].as_str().unwrap_or("").to_string();
        let id = it["id"].as_u64().unwrap_or(0) as u32;
        ui.horizontal(|ui| {
            let sel = app.selected_chart == Some(id);
            if ui.selectable_label(sel, it["name"].as_str().unwrap_or("")).clicked() {
                app.selected_chart = Some(id);
            }
            if ui.small_button("▲").on_hover_text(crate::widgets::tr(ui, "Bring Forward")).clicked() {
                app.run_or_alert("arrange.bringForward", json!({"kind": kind, "id": id}));
            }
            if ui.small_button("▼").on_hover_text(crate::widgets::tr(ui, "Send Backward")).clicked() {
                app.run_or_alert("arrange.sendBackward", json!({"kind": kind, "id": id}));
            }
            if ui.small_button("✕").on_hover_text(crate::widgets::tr(ui, "Delete")).clicked() {
                app.run_or_alert("object.delete", json!({"kind": kind, "id": id}));
            }
        });
    }
    for (name, id) in widgets {
        ui.horizontal(|ui| {
            if ui.selectable_label(app.selected_chart == Some(id), name).clicked() {
                app.selected_chart = Some(id);
            }
            if ui.small_button("✕").on_hover_text(crate::widgets::tr(ui, "Delete")).clicked() {
                app.run_or_alert("slicer.delete", json!({"slicer": id}));
                if app.selected_chart == Some(id) {
                    app.selected_chart = None;
                }
            }
        });
    }
}

fn format_chart(app: &mut SheetApp, ui: &mut Ui) {
    crate::chart_format::show(app, ui);
}

#[cfg(test)]
mod tests {
    use super::title_of;

    #[test]
    fn the_format_pane_is_titled_after_the_object() {
        assert_eq!(title_of(Some("shape")), "Format Shape");
        assert_eq!(title_of(Some("image")), "Format Picture");
        // A chart, a slicer and an empty selection all keep the chart title the pane shipped with.
        for other in [Some("chart"), Some("slicer"), Some("timeline"), None] {
            assert_eq!(title_of(other), "Format Chart Area", "{other:?}");
        }
    }
}
