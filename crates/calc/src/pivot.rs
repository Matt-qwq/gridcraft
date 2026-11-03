//! GETPIVOTDATA: reads one cell out of the report a PivotTable wrote.
//!
//! The report's body is static cells on its sheet, so the lookup reads the report's own header
//! and item labels instead of recomputing the aggregation. That keeps the answer right on a file
//! that is opened and read without a refresh (the model has no PivotTable cache to recompute
//! from), and it makes a formula depend on real cells: a refresh rewrites them, and the formula
//! recalculates (see `eval::precedents`).
//!
//! The geometry mirrors `crates/engine/src/pivot.rs::compute`: page filters above the body, a
//! caption row plus one row per column field, then one line per row item. A compact report writes
//! one label column with the depth in the cell's indent; outline and tabular write one column per
//! row level.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use gridcraft_core::{CellError, CellRef, Value};
use gridcraft_model::{PivotLayout, PivotTable, Sheet, StyleId, Workbook};
use gridcraft_numfmt::{NumberFormat, format_value};

/// The PivotTable whose last written report covers `at`.
pub fn pivot_at(wb: &Workbook, sheet: usize, at: CellRef) -> Option<&PivotTable> {
    wb.sheet(sheet)?.pivots.iter().find(|p| p.last_range.is_some_and(|r| r.contains(at)))
}

/// The report layout, in cell offsets from the top-left of the written area.
struct Layout {
    /// Page-filter rows plus their blank separator.
    body_offset: u32,
    /// The caption row and one row per column field (plus one for the value captions).
    header_rows: u32,
    /// Label columns before the data columns.
    label_cols: u32,
    row_levels: u32,
    col_levels: u32,
    values: u32,
    /// Σ Values sits in Rows (two or more value fields).
    on_rows: bool,
    /// Depth of the Σ Values row level.
    vpos: u32,
}

fn layout(pt: &PivotTable) -> Layout {
    let values = u32::try_from(pt.values.len()).unwrap_or(u32::MAX);
    let on_rows = pt.values_on_rows && values > 1;
    let on_axis = |name: &str| pt.rows.iter().chain(pt.columns.iter()).any(|f| f.source_col.eq_ignore_ascii_case(name));
    let pages = pt.filters.iter().filter(|f| f.is_page(on_axis(&f.source_col))).count();
    let body_offset = u32::try_from(pages).unwrap_or(u32::MAX).saturating_add(u32::from(pages > 0));
    let row_fields = u32::try_from(pt.rows.len()).unwrap_or(u32::MAX);
    let col_fields = u32::try_from(pt.columns.len()).unwrap_or(u32::MAX);
    let row_levels = row_fields.saturating_add(u32::from(on_rows));
    let header_rows = if pt.columns.is_empty() { 1 } else { 1u32.saturating_add(col_fields).saturating_add(u32::from(values > 1 && !on_rows)) };
    let label_cols = if row_levels == 0 && pt.columns.is_empty() {
        0
    } else if pt.layout == PivotLayout::Compact {
        1
    } else {
        row_levels.max(1)
    };
    let vpos = if on_rows { u32::try_from(pt.values_position.unwrap_or(pt.rows.len()).min(pt.rows.len())).unwrap_or(0) } else { 0 };
    Layout { body_offset, header_rows, label_cols, row_levels, col_levels: col_fields, values, on_rows, vpos }
}

/// A label cell read from the report: its value and the style that carries the number format (and,
/// in compact form, the indent that names the level).
#[derive(Clone)]
struct Label {
    value: Value,
    style: StyleId,
}

fn label(sh: &Sheet, r: u32, c: u32) -> Option<Label> {
    let cell = sh.cells.get(CellRef::new(r, c))?;
    (!cell.value.is_empty()).then_some(Label { value: cell.value, style: cell.style })
}

/// Whether an item label names `want`. Text compares case-insensitively (the report merges items
/// that way); a number matches its value or its formatted text, so a date can be named by serial
/// or as shown.
fn item_is(wb: &Workbook, label: &Label, want: &str) -> bool {
    match &label.value {
        Value::Text(t) => t.eq_ignore_ascii_case(want),
        Value::Number(n) => {
            if want.parse::<f64>().is_ok_and(|x| x == *n) {
                return true;
            }
            let code = wb.styles.get(label.style).num_fmt.as_str();
            format_value(&label.value, &NumberFormat::parse(code), wb.date_system).text.eq_ignore_ascii_case(want)
        }
        Value::Bool(b) => want.eq_ignore_ascii_case(if *b { "TRUE" } else { "FALSE" }),
        Value::Empty => want.eq_ignore_ascii_case("(blank)"),
        Value::Error(e) => want.eq_ignore_ascii_case(e.as_str()),
        Value::Array(_) => false,
    }
}

/// Whether a label is the *subtotal* line of `want` ("West Total", "West Sum of Sales"). A
/// layout that has no row of its own for a parent item (tabular) writes its figure on that line.
fn subtotal_of(label: &Label, want: &str, pt: &PivotTable) -> bool {
    let Value::Text(t) = &label.value else { return false };
    let (Some(head), Some(tail)) = (t.get(..want.len()), t.get(want.len()..)) else { return false };
    if !head.eq_ignore_ascii_case(want) {
        return false;
    }
    let rest = tail.trim_start();
    tail.len() > rest.len() && (rest.eq_ignore_ascii_case("Total") || pt.values.iter().any(|v| v.name.eq_ignore_ascii_case(rest)))
}

fn value_by_caption(pt: &PivotTable, caption: &str) -> Option<usize> {
    pt.values.iter().position(|v| v.name.eq_ignore_ascii_case(caption))
}

/// The index of the value field `data_field` names: its caption ("Sum of Sales") or, when only one
/// value field comes from that source column, the column's own name ("Sales").
fn value_index(pt: &PivotTable, data_field: &str) -> Option<usize> {
    if let Some(i) = value_by_caption(pt, data_field) {
        return Some(i);
    }
    let mut found = None;
    for (i, v) in pt.values.iter().enumerate() {
        if v.source_col.eq_ignore_ascii_case(data_field) {
            if found.is_some() {
                return None;
            }
            found = Some(i);
        }
    }
    found
}

fn find_field(fields: &[gridcraft_model::PivotField], name: &str) -> Option<usize> {
    fields.iter().position(|f| f.source_col.eq_ignore_ascii_case(name))
}

/// A cell of the report, as the report itself shows it.
fn shown(sh: &Sheet, at: CellRef) -> Option<Value> {
    let cell = sh.cells.get(at)?;
    (!cell.value.is_empty()).then_some(cell.value)
}

/// `GETPIVOTDATA(data_field, pivot_table, field1, item1, …)`.
///
/// `at` is the cell the second argument names (any cell of the report). Returns `#REF!` when the
/// report does not hold a single cell the arguments name, which is what Excel returns for an
/// unknown field, an unknown item or an ambiguous lookup.
pub fn get(wb: &Workbook, sheet: usize, at: CellRef, data_field: &str, pairs: &[(String, String)]) -> Value {
    let Some(sh) = wb.sheet(sheet) else { return Value::Error(CellError::Ref) };
    let Some(pt) = pivot_at(wb, sheet, at) else { return Value::Error(CellError::Ref) };
    let Some(area) = pt.last_range else { return Value::Error(CellError::Ref) };
    if pt.values.is_empty() || (pt.rows.is_empty() && pt.columns.is_empty()) {
        return Value::Error(CellError::Ref);
    }
    let Some(vi) = value_index(pt, data_field) else { return Value::Error(CellError::Ref) };
    let ly = layout(pt);

    let body_start = area.start.row.saturating_add(ly.body_offset).saturating_add(ly.header_rows);
    if body_start > area.end.row {
        return Value::Error(CellError::Ref);
    }
    let body_rows = area.end.row - body_start + 1;
    let data_width = area.width().saturating_sub(ly.label_cols);
    // Data columns of one column group: one per value field, or one when Σ Values is in Rows.
    let per = if ly.on_rows || ly.values == 0 { 1 } else { ly.values };
    if data_width < per || per == 0 {
        return Value::Error(CellError::Ref);
    }
    let grand_cols = u32::from(pt.grand_totals_rows && !pt.columns.is_empty());
    let grand_rows = if pt.grand_totals_cols && !pt.rows.is_empty() { if ly.on_rows { ly.values } else { 1 } } else { 0 };

    // The (field, item) pairs name a page filter, a row field or a column field.
    let mut row_want: Vec<Option<&str>> = vec![None; pt.rows.len()];
    let mut col_want: Vec<Option<&str>> = vec![None; pt.columns.len()];
    let mut want_grand_row = false;
    let mut want_grand_col = false;
    for (field, item) in pairs {
        let grand = item.eq_ignore_ascii_case("Grand Total");
        if let Some(i) = find_field(&pt.rows, field) {
            if grand {
                want_grand_row = true;
            } else if let Some(slot) = row_want.get_mut(i) {
                *slot = Some(item);
            }
        } else if let Some(i) = find_field(&pt.columns, field) {
            if grand {
                want_grand_col = true;
            } else if let Some(slot) = col_want.get_mut(i) {
                *slot = Some(item);
            }
        } else if let Some(f) = pt.filters.iter().find(|f| f.source_col.eq_ignore_ascii_case(field)) {
            // A filter only shows the items it selected; the report body is already narrowed to
            // them, so naming one names the same cells while naming another names none.
            let ok = match &f.selected {
                None => item.eq_ignore_ascii_case("(All)") || item.eq_ignore_ascii_case("All"),
                Some(sel) if sel.len() == 1 => sel.first().is_some_and(|s| s.eq_ignore_ascii_case(item)),
                Some(_) => item.eq_ignore_ascii_case("(Multiple Items)"),
            };
            if !ok {
                return Value::Error(CellError::Ref);
            }
        } else {
            return Value::Error(CellError::Ref);
        }
    }

    // No pairs: the grand total.
    if pairs.is_empty() {
        let row = if pt.rows.is_empty() {
            body_start
        } else if grand_rows > 0 {
            if ly.on_rows { body_start.saturating_add(body_rows.saturating_sub(grand_rows)).saturating_add(vi as u32) } else { area.end.row }
        } else {
            return Value::Error(CellError::Ref);
        };
        let col = if pt.columns.is_empty() {
            area.start.col.saturating_add(ly.label_cols)
        } else if grand_cols > 0 {
            let base = area.start.col.saturating_add(ly.label_cols).saturating_add(data_width - per);
            base + if ly.on_rows { 0 } else { vi as u32 }
        } else {
            return Value::Error(CellError::Ref);
        };
        return shown(sh, CellRef::new(row, col)).unwrap_or(Value::Error(CellError::Ref));
    }

    let row_constrained = row_want.iter().any(Option::is_some);
    let col_constrained = col_want.iter().any(Option::is_some);
    // A pair names the cell of the item itself, not of the group below it: on a report with
    // Region and Month, ("Region", "East") is East's subtotal row, never one of its months.
    let row_depth = row_want.iter().rposition(Option::is_some).map(|fi| if ly.on_rows && fi as u32 >= ly.vpos { fi + 1 } else { fi });
    let col_depth = col_want.iter().rposition(Option::is_some);

    // ---- column groups: the item labels are written at the first column of a group and inherited
    // to its right, and a label clears the deeper levels.
    let mut col_hits: Vec<u32> = Vec::new();
    let mut cur: Vec<Option<Label>> = vec![None; ly.col_levels as usize];
    // Depth of the group the column belongs to (the deepest label its first column carried).
    let mut group_depth: Option<usize> = None;
    for j in 0..data_width {
        let c = area.start.col.saturating_add(ly.label_cols).saturating_add(j);
        let mut own: Option<usize> = None;
        for lv in 0..ly.col_levels {
            let r = area.start.row.saturating_add(ly.body_offset).saturating_add(1).saturating_add(lv);
            if let Some(l) = label(sh, r, c) {
                own = Some(lv as usize);
                if let Some(slot) = cur.get_mut(lv as usize) {
                    *slot = Some(l);
                }
                for deeper in cur.iter_mut().skip(lv as usize + 1) {
                    *deeper = None;
                }
            }
        }
        if own.is_some() {
            group_depth = own;
        }
        let grand = grand_cols > 0 && j >= data_width - per;
        if want_grand_col {
            if !grand {
                continue;
            }
        } else if col_constrained {
            if grand || group_depth != col_depth {
                continue;
            }
            let mut ok = true;
            for (lv, want) in col_want.iter().enumerate() {
                let Some(want) = want else { continue };
                match cur.get(lv).and_then(|l| l.as_ref()) {
                    Some(l) if item_is(wb, l, want) || subtotal_of(l, want, pt) => {}
                    _ => {
                        ok = false;
                        break;
                    }
                }
            }
            if !ok {
                continue;
            }
        }
        // The value field a data column holds (Σ Values in Rows picks it on the line instead).
        if !ly.on_rows && ly.values > 1 && (j % per) as usize != vi {
            continue;
        }
        col_hits.push(j);
    }

    // ---- row lines
    let mut row_hits: Vec<u32> = Vec::new();
    let mut cur: Vec<Option<Label>> = vec![None; ly.row_levels as usize];
    for i in 0..body_rows {
        let r = body_start + i;
        let mut own: Option<usize> = None;
        if ly.row_levels > 0 {
            if ly.label_cols == 1 {
                // Compact: one label column, the depth is the cell's indent.
                if let Some(l) = label(sh, r, area.start.col) {
                    let d = wb.styles.get(l.style).align.indent as usize;
                    own = Some(d);
                    if let Some(slot) = cur.get_mut(d) {
                        *slot = Some(l);
                    }
                    for deeper in cur.iter_mut().skip(d + 1) {
                        *deeper = None;
                    }
                }
            } else {
                for d in 0..ly.row_levels.min(ly.label_cols) {
                    if let Some(l) = label(sh, r, area.start.col + d) {
                        own = Some(d as usize);
                        if let Some(slot) = cur.get_mut(d as usize) {
                            *slot = Some(l);
                        }
                        for deeper in cur.iter_mut().skip(d as usize + 1) {
                            *deeper = None;
                        }
                    }
                }
            }
        }
        let is_grand = grand_rows > 0 && i >= body_rows - grand_rows;
        // Σ Values in Rows: the line (or its value caption above it) names the value field.
        if ly.on_rows {
            let line_vi = if is_grand {
                Some((i - (body_rows - grand_rows)) as usize)
            } else {
                cur.get(ly.vpos as usize).and_then(|l| l.as_ref()).and_then(|l| match &l.value {
                    Value::Text(t) => value_by_caption(pt, t),
                    _ => None,
                })
            };
            if line_vi != Some(vi) {
                continue;
            }
        }
        if want_grand_row {
            if !is_grand {
                continue;
            }
        } else if row_constrained || grand_rows > 0 {
            // A grand line holds totals, not an item: only an explicit "Grand Total" names it.
            if is_grand {
                continue;
            }
            if row_constrained && own != row_depth {
                continue;
            }
            let mut ok = true;
            for (fi, want) in row_want.iter().enumerate() {
                let Some(want) = want else { continue };
                let d = if ly.on_rows && fi as u32 >= ly.vpos { fi + 1 } else { fi };
                match cur.get(d).and_then(|l| l.as_ref()) {
                    Some(l) if item_is(wb, l, want) || subtotal_of(l, want, pt) => {}
                    _ => {
                        ok = false;
                        break;
                    }
                }
            }
            if !ok {
                continue;
            }
        }
        row_hits.push(i);
    }

    // One cell has to answer the lookup; anything else is #REF!, as in Excel.
    let mut hit: Option<Value> = None;
    let mut looked = 0u32;
    for &i in &row_hits {
        for &j in &col_hits {
            looked = looked.saturating_add(1);
            if looked > 2_000_000 {
                return Value::Error(CellError::Ref);
            }
            let c = CellRef::new(body_start.saturating_add(i), area.start.col.saturating_add(ly.label_cols).saturating_add(j));
            let Some(v) = shown(sh, c) else { continue };
            if hit.is_some() {
                return Value::Error(CellError::Ref);
            }
            hit = Some(v);
        }
    }
    hit.unwrap_or(Value::Error(CellError::Ref))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_that_is_not_written_has_no_answer() {
        let wb = Workbook::default();
        assert_eq!(get(&wb, 0, CellRef::new(0, 0), "Sales", &[]), Value::Error(CellError::Ref));
    }
}
