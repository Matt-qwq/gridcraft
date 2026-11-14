//! What a screen reader sees of a worksheet.
//!
//! The grid is painted as one surface (its cells are not widgets), so on its own AccessKit finds
//! a single node for the whole sheet and a reader can't walk it. While a reader is attached
//! ([`egui::Context::is_accesskit_enabled`], which the platform integration turns on) this module
//! gives the visible cells a `Grid` of their own: each one a `GridCell` whose value is the label
//! and whose address (and formula, when it has one) is the description. Nothing is built when no
//! reader is attached, so the normal path costs one branch a frame.
//!
//! Only the viewport is exposed (capped), because the tree is rebuilt every frame: a reader
//! reaches the rest of a sheet by scrolling it, as a sighted user does. Column and row headers
//! are not nodes of their own yet.

use egui::accesskit::Role;
use egui::{Id, Sense};
use gridcraft_engine::core::{CellRef, RangeRef};
use gridcraft_engine::model::{Sheet, Workbook};

use crate::grid::Geo;
use crate::i18n::Language;

/// Most header nodes exposed (a viewport never has more rows than cells).
const MAX_HEADERS: usize = 200;

/// Most cells exposed at once: about a 70 × 30 window at 100 % zoom, which is what a large monitor
/// shows. A reader reaches the rest of a sheet by scrolling it, as a sighted user does — walking
/// more than this per frame would cost more than it is worth.
const MAX_CELLS: usize = 2000;

/// Exposes the visible cells of `sh` as an AccessKit grid under `ui`, with the active cell marked
/// selected and the selection's range in the grid's description.
pub(crate) fn expose_grid(ui: &mut egui::Ui, wb: &Workbook, si: usize, sh: &Sheet, geo: &Geo, active: CellRef, range: RangeRef) {
    let ctx = ui.ctx().clone();
    let (sr0, sr1) = geo.scroll_rows(sh);
    let (sc0, sc1) = geo.scroll_cols(sh);
    // Frozen rows and columns stay visible above and left of the scrolling part.
    let mut rows: Vec<u32> = (0..geo.fr.min(sr0)).collect();
    rows.extend(sr0..=sr1);
    let mut cols: Vec<u32> = (0..geo.fc.min(sc0)).collect();
    cols.extend(sc0..=sc1);

    // The grid node: a child `Ui` whose node becomes the parent of the cells created below.
    let grid = ui.new_child(egui::UiBuilder::new().max_rect(geo.cells).id_salt(("gridcraft.a11y.grid", si)));
    let grid_id = grid.unique_id();
    let used = sh.used_range();
    let active_value = sh.value(active);
    let value = if active_value.is_empty() { String::new() } else { format!(", {}", active_value.display()) };
    // A chart sheet holds one chart and no cells: walking cell nodes there would describe an
    // editable grid that does not exist, so the node names the sheet and says what it holds.
    let language = crate::i18n::current(&ctx);
    let shown = wb.active_sheet == si;
    let description = if sh.is_chart_sheet() {
        chart_sheet_description(language, &sh.name, sh.charts.len(), shown)
    } else {
        worksheet_description(language, &sh.name, &active.a1(), &value, range.count(), shown)
    };
    // Physical pixels for the node's bounds. (Anything that reads the context must happen before
    // `accesskit_node_builder`: it holds the context lock while its closure runs.)
    let ppp = ctx.pixels_per_point();
    let bounds = egui::accesskit::Rect {
        x0: f64::from(geo.cells.left() * ppp),
        y0: f64::from(geo.cells.top() * ppp),
        x1: f64::from(geo.cells.right() * ppp),
        y1: f64::from(geo.cells.bottom() * ppp),
    };
    // Creating this node is also the probe: with no reader attached the builder is absent and no
    // cell nodes are made at all.
    let enabled = ctx
        .accesskit_node_builder(grid_id, |n| {
            n.set_role(Role::Grid);
            n.set_label(sh.name.clone());
            n.set_row_count(used.map(|r| r.end.row as usize + 1).unwrap_or(0));
            n.set_column_count(used.map(|r| r.end.col as usize + 1).unwrap_or(0));
            n.set_description(description);
            n.set_bounds(bounds);
        })
        .is_some();
    if !enabled {
        return;
    }

    // The header row and column, so a reader can name where a cell is.
    for &c in cols.iter().take(MAX_HEADERS) {
        let x0 = geo.x(sh, c);
        let rect = egui::Rect::from_min_size(egui::pos2(x0, geo.cells.top()), egui::vec2(sh.col_width(c) * geo.z, geo.header_h));
        if !rect.intersects(geo.cells) {
            continue;
        }
        let id = Id::new(("gridcraft.a11y.col", si, c));
        let _ = grid.interact(rect, id, Sense::hover());
        let name = gridcraft_engine::core::col_to_letters(c);
        ctx.accesskit_node_builder(id, |n| {
            n.set_role(Role::ColumnHeader);
            n.set_label(name);
            n.set_column_index(c as usize);
        });
    }
    for &r in rows.iter().take(MAX_HEADERS) {
        let y0 = geo.y(sh, r);
        let rect = egui::Rect::from_min_size(egui::pos2(geo.cells.left(), y0), egui::vec2(geo.header_w, sh.row_height(r) * geo.z));
        if !rect.intersects(geo.cells) {
            continue;
        }
        let id = Id::new(("gridcraft.a11y.row", si, r));
        let _ = grid.interact(rect, id, Sense::hover());
        ctx.accesskit_node_builder(id, |n| {
            n.set_role(Role::RowHeader);
            n.set_label((r + 1).to_string());
            n.set_row_index(r as usize);
        });
    }

    let mut shown = 0usize;
    let mut active_id: Option<Id> = None;
    'cells: for &r in &rows {
        for &c in &cols {
            if shown >= MAX_CELLS {
                break 'cells;
            }
            let cell = CellRef::new(r, c);
            let rect = geo.cell_rect(sh, cell);
            if !rect.intersects(geo.cells) {
                continue;
            }
            let id = Id::new(("gridcraft.a11y.cell", si, r, c));
            // A hover-only response gives the cell an AccessKit node under this `Ui` (the only
            // public way to parent a node) without taking clicks from the grid's own pointer
            // handling.
            let _ = grid.interact(rect, id, Sense::hover());
            let value = sh.value(cell);
            let label = if value.is_empty() { String::new() } else { value.display() };
            let formula = sh.cells.get(cell).and_then(|c| c.formula.clone()).map(|f| f.text.trim_start_matches('=').to_string());
            let address = cell.a1();
            ctx.accesskit_node_builder(id, |n| {
                n.set_role(Role::GridCell);
                n.set_value(label.clone());
                n.set_row_index(r as usize);
                n.set_column_index(c as usize);
                n.set_description(match formula.as_deref() {
                    Some(text) => format!("{address}, ={text}"),
                    None => address,
                });
                if cell == active {
                    n.set_selected(true);
                }
            });
            if cell == active {
                active_id = Some(id);
            }
            shown += 1;
        }
    }
    // The container keeps focus while the active cell moves (a reader announces it as the grid's
    // active descendant), which is how a spreadsheet is read cell by cell.
    ctx.accesskit_node_builder(grid_id, |n| match active_id {
        Some(id) => n.set_active_descendant(id.accesskit_id()),
        None => n.clear_active_descendant(),
    });
}

/// What a reader hears for a worksheet: "Worksheet Sheet1, active cell B2, 42, 3 cells selected".
///
/// The singular and the plural are templates of their own, because a language need not spell the
/// plural by adding a letter; the count stays a placeholder in both. The sheet name, the address
/// and the value are document data and go into the slots untouched.
fn worksheet_description(language: Language, name: &str, active: &str, value: &str, cells: u64, shown: bool) -> String {
    let template = if cells == 1 {
        "Worksheet {name}{hidden}, active cell {cell}{value}, {cells} cell selected"
    } else {
        "Worksheet {name}{hidden}, active cell {cell}{value}, {cells} cells selected"
    };
    language
        .tr(template)
        .replace("{name}", name)
        .replace("{hidden}", &hidden_suffix(language, shown))
        .replace("{cell}", active)
        .replace("{value}", value)
        .replace("{cells}", &cells.to_string())
}

/// What a reader hears for a chart sheet, which holds one chart and no cells.
fn chart_sheet_description(language: Language, name: &str, charts: usize, shown: bool) -> String {
    let template = if charts == 1 { "Chart sheet {name}{hidden}, {charts} chart" } else { "Chart sheet {name}{hidden}, {charts} charts" };
    language.tr(template).replace("{name}", name).replace("{hidden}", &hidden_suffix(language, shown)).replace("{charts}", &charts.to_string())
}

/// The suffix a sheet that is not the one on screen carries; its words come from the catalog too.
fn hidden_suffix(language: Language, shown: bool) -> String {
    if shown { String::new() } else { format!(" {}", language.tr("(not shown)")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Chinese build describes the sheet in Chinese, keeps the sheet name, the address and the
    /// numbers as they are, and never leaves an English word of the template behind.
    #[test]
    fn the_grid_description_reads_chinese() {
        let zh = worksheet_description(Language::Zh, "Sheet1", "B2", ", 42", 3, true);
        assert_eq!(zh, "工作表 Sheet1，活动单元格 B2, 42，已选择 3 个单元格");
        for english in ["Worksheet", "active", "cell", "selected", "cells"] {
            assert!(!zh.contains(english), "{english:?} survived in {zh:?}");
        }
        // The singular is its own template, and the count is still the count.
        let one = worksheet_description(Language::Zh, "Sheet1", "A1", "", 1, true);
        assert!(one.contains('1'), "{one:?}");
        assert!(!one.contains("单元格 selected"), "{one:?}");
        // A sheet that is not on screen says so in Chinese too.
        let hidden = worksheet_description(Language::Zh, "Sheet1", "B2", "", 2, false);
        assert!(hidden.contains("未显示"), "{hidden:?}");
        // A chart sheet has its own wording, in both languages.
        assert_eq!(chart_sheet_description(Language::Zh, "Chart1", 1, true), "图表工作表 Chart1，共 1 个图表");
        assert_eq!(chart_sheet_description(Language::En, "Chart1", 2, false), "Chart sheet Chart1 (not shown), 2 charts");
        // The English build is unchanged: nothing of this wave moves English text.
        assert_eq!(worksheet_description(Language::En, "Sheet1", "B2", ", 42", 3, true), "Worksheet Sheet1, active cell B2, 42, 3 cells selected");
        assert_eq!(worksheet_description(Language::En, "Sheet1", "B2", ", 42", 1, true), "Worksheet Sheet1, active cell B2, 42, 1 cell selected");
    }
}
