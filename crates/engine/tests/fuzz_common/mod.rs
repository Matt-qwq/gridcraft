//! The workbook comparison the command-sequence fuzzer uses, shared with the regression tests it
//! produced (`tests/fuzz_regressions_structure.rs`): an invariant is only meaningful if the two
//! sides are compared the same way everywhere.
//!
//! Two workbooks are compared as a whole ([`workbook_diff`]), view state and the Watch Window list
//! aside, because a difference the cell-level checks miss — a defined name, a registered style, a
//! workbook setting — is exactly what "undoing everything gives back the starting workbook" has to
//! catch.

// Each test target that includes this module uses the part it needs (the harness keeps the Watch
// Window list, the regression tests compare whole workbooks), so the rest is unused there.
#![allow(dead_code)]

use std::sync::Arc;

use gridcraft_core::CellRef;
use gridcraft_engine::model::{Sheet, Workbook};

/// The hidden defined name `formulas.watchWindow` keeps its list in.
pub const WATCH_NAME: &str = "_sc_watch";

pub fn has_content(c: &gridcraft_engine::model::Cell) -> bool {
    c.formula.is_some() || !c.value.is_empty()
}

/// Cells whose formula or constant differs (computed values of formulas aside).
pub fn content_diff(a: &Workbook, b: &Workbook) -> Option<String> {
    let names = |wb: &Workbook| wb.sheets.iter().map(|s| s.name.clone()).collect::<Vec<_>>();
    if names(a) != names(b) {
        return Some(format!("sheets {:?} became {:?}", names(a), names(b)));
    }
    for (x, y) in a.sheets.iter().zip(&b.sheets) {
        if Arc::ptr_eq(x, y) {
            continue;
        }
        // Logical, not stored, positions: the fuzz compares two workbooks that may have been
        // remapped by different structural edits, and `diff` is a physical comparison.
        for c in x.cells.diff_logical(&y.cells) {
            let content = |s: &Sheet| {
                s.cell(c).filter(has_content).map(|c| match &c.formula {
                    Some(f) => format!("={}", f.text),
                    None => format!("{:?}", c.value),
                })
            };
            if content(x) != content(y) {
                return Some(format!("{}!{c}: {:?} became {:?}", x.name, content(x), content(y)));
            }
        }
    }
    None
}

/// The Watch Window list, which is workbook state but not an edit (undo keeps it).
pub fn watch_list(wb: &Workbook) -> Option<&str> {
    wb.names.iter().find(|n| n.name == WATCH_NAME).map(|n| n.formula.as_str())
}

/// The workbook with view state (active sheet, scroll, active cell) and the Watch Window list
/// reset: undo restores view state as it was, but switching sheets isn't an undoable edit.
pub fn canonical(wb: &Workbook) -> Workbook {
    let mut wb = wb.clone();
    wb.active_sheet = 0;
    wb.names.retain(|n| n.name != WATCH_NAME);
    for sh in wb.sheets.iter_mut() {
        let sh = Arc::make_mut(sh);
        sh.view_active = CellRef::default();
        sh.view_top_left = CellRef::default();
        // A view command (page layout, page break preview) is not content and is not undoable.
        sh.view = gridcraft_engine::model::SheetViewMode::Normal;
        // Split (not undoable) shows its split as frozen panes; a view setting is not content.
        sh.freeze = None;
    }
    wb
}

pub fn workbook_diff(a: &Workbook, b: &Workbook) -> Option<String> {
    let (a, b) = (canonical(a), canonical(b));
    if a == b {
        return None;
    }
    if let Some(d) = content_diff(&a, &b) {
        return Some(d);
    }
    for (x, y) in a.sheets.iter().zip(&b.sheets) {
        if x != y {
            if let Some(c) = x.cells.diff_logical(&y.cells).first() {
                return Some(format!("{}!{c}: {:?} vs {:?}", x.name, x.cell(*c), y.cell(*c)));
            }
            let field = [
                ("merges", x.merges != y.merges),
                ("tables", x.tables != y.tables),
                ("charts", x.charts != y.charts),
                ("rows", x.rows != y.rows),
                ("cols", x.cols != y.cols),
                ("autofilter", x.autofilter != y.autofilter),
                ("cond_formats", x.cond_formats != y.cond_formats),
                ("validations", x.validations != y.validations),
                ("spill", x.spill != y.spill || x.spill_ranges != y.spill_ranges),
            ]
            .into_iter()
            .find(|(_, d)| *d)
            .map_or_else(|| differing_sheet_field(x, y), |(n, _)| n.to_string());
            return Some(format!("{}: {field} differ", x.name));
        }
    }
    Some("workbook-level properties (names, styles, settings) differ".into())
}

/// The first sheet field that differs, named the way the document JSON spells it.
fn differing_sheet_field(x: &Sheet, y: &Sheet) -> String {
    let (Ok(a), Ok(b)) = (serde_json::to_value(x), serde_json::to_value(y)) else { return "other sheet properties".into() };
    let (Some(a), Some(b)) = (a.as_object(), b.as_object()) else { return "other sheet properties".into() };
    for (key, value) in a {
        if b.get(key) != Some(value) {
            return key.clone();
        }
    }
    "other sheet properties".into()
}
