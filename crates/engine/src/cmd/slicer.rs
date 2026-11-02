//! Slicers and timelines: on-sheet widgets that filter by clicking value tiles.
//!
//! Two kinds live side by side. A **table slicer** ([`gridcraft_model::Slicer`]) is a thin
//! front-end over the sheet's [`AutoFilter`]: its selection is written as a
//! [`FilterCriterion::Values`] and [`super::data::apply_filter`] hides the rows. A **PivotTable
//! slicer** ([`gridcraft_model::PivotSlicer`]) is Excel's Insert ▸ Slicer: it owns a slicer cache,
//! is saved in the file, and filters every PivotTable that shares its cache by writing the
//! field's item selection into the PivotTable's own filter and re-rendering it. A timeline is the
//! same for a date field, with the time unit it groups by.
//!
//! Programmatic calls never open a dialog: empty params apply the documented default or return an
//! error naming what is missing. Every command is one undo step.

use std::collections::HashSet;

use gridcraft_core::{CellRef, DateSystem, RangeRef, Value};
use gridcraft_model::*;
use serde_json::{Value as Json, json};

use super::*;
use crate::pivot as pv;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "insert.slicer",
            "Slicer",
            ["Insert", "Filters"],
            None,
            "{table? | pivot?: name | id | a cell of the report, column?/field?: header, columns?: 1, at?: \"H2\", width?, height?, style?, name?, caption?} → {slicer, name, cache}; a PivotTable slicer filters its report and is saved, a table slicer filters the table's column",
            has_doc,
            insert_slicer
        ),
        cmd!(
            "insert.timeline",
            "Timeline",
            ["Insert", "Filters"],
            None,
            "{pivot?, field? (default: the report's first field), at?, width?, height?, unit?: years|quarters|months|days (default months), style?, name?, caption?} → {timeline, name, cache}",
            has_doc,
            insert_timeline
        ),
        cmd!(
            "table.insertSlicer",
            "Insert Slicer",
            ["Table", "Tools"],
            None,
            "{table?, column?: \"Region\", columns?: 1, at?, width?, height?}",
            has_doc,
            insert_slicer
        ),
        cmd!("slicer.toggle", "Toggle Slicer Value", [], None, "{slicer?: id | name, value: \"East\", multi?: false}", has_doc, slicer_toggle),
        cmd!("slicer.selectAll", "Clear Slicer Filter", [], None, "{slicer?: id | name}", has_doc, slicer_select_all),
        cmd!("slicer.clear", "Clear Slicer Filter", [], None, "{slicer?: id | name}", has_doc, slicer_select_all),
        cmd!(
            "slicer.setSelection",
            "Select Slicer Items",
            [],
            None,
            "{slicer?: id | name, selected?: [tile labels] | null (all)} → {slicer, selected, pivots}; stops with the first label that is not a tile",
            has_doc,
            set_selection
        ),
        cmd!(
            "slicer.setRange",
            "Set Timeline Range",
            [],
            None,
            "{slicer?: id | name | timeline?: name, start?: \"2024-01-01\", end?: \"2024-01-01\"} (a timeline's dates; both ends are included and both are clamped to the field's own first and last date, which `clamped` reports; null for both shows every date) → {timeline, start, end, pivots, clamped}",
            has_doc,
            set_range
        ),
        cmd!(
            "slicer.setStyle",
            "Slicer Styles",
            [],
            None,
            "{slicer?: id | name, style: \"SlicerStyleLight1\"} (a PivotTable slicer or timeline style the format defines)",
            has_doc,
            set_style
        ),
        cmd!(
            "slicer.move",
            "Move Slicer",
            [],
            None,
            "{slicer?: id | name, at?: cell, dx?, dy?, width?, height?} (a PivotTable slicer, a table slicer or a timeline; `dx`/`dy` are offsets inside the anchor cell, so a drag does not have to snap to a cell) → {slicer, at, dx, dy, width, height}",
            has_doc,
            move_slicer
        ),
        cmd!("slicer.delete", "Delete Slicer", [], None, "{slicer?: id | name}", has_doc, slicer_delete),
    ]
}

// ---------------------------------------------------------------- table slicers

/// Distinct, non-blank display values of a table column, in first-seen order. Used to populate a
/// table slicer's tiles and to detect "everything selected".
pub fn column_values(wb: &Workbook, sheet: usize, table: &str, column: &str) -> Vec<String> {
    let Some(sh) = wb.sheet(sheet) else { return vec![] };
    let Some(t) = sh.tables.iter().find(|t| t.name.eq_ignore_ascii_case(table)) else { return vec![] };
    let Some(ci) = t.column_index(column) else { return vec![] };
    let Some(dr) = t.data_range() else { return vec![] };
    let col = dr.start.col + ci as u32;
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for row in dr.start.row..=dr.end.row {
        let text = crate::display::cell_text(wb, sh, CellRef::new(row, col));
        if text.is_empty() {
            continue;
        }
        if seen.insert(text.to_lowercase()) {
            out.push(text);
        }
    }
    out
}

/// A table slicer's tiles as `(value, selected)`, for the UI. `selected` is true for every tile
/// when the slicer has no filter (`selected == None`).
pub fn slicer_items(wb: &Workbook, sheet: usize, sl: &Slicer) -> Vec<(String, bool)> {
    column_values(wb, sheet, &sl.table, &sl.column)
        .into_iter()
        .map(|v| {
            let on = match &sl.selected {
                None => true,
                Some(set) => set.iter().any(|x| x.eq_ignore_ascii_case(&v)),
            };
            (v, on)
        })
        .collect()
}

/// Writes a table slicer's current selection into the sheet filter and re-applies it.
fn reapply_slicer(wb: &mut Workbook, sheet: usize, id: u32) {
    let info = wb.sheet(sheet).and_then(|sh| {
        let sl = sh.slicers.iter().find(|s| s.id == id)?;
        let t = sh.tables.iter().find(|t| t.name.eq_ignore_ascii_case(&sl.table))?;
        let ci = t.column_index(&sl.column)?;
        Some((t.range, t.range.start.col + ci as u32, sl.selected.clone()))
    });
    let Some((table_range, abs_col, selected)) = info else { return };
    {
        let Some(sh) = wb.sheet_mut(sheet) else { return };
        let af = sh.autofilter.get_or_insert_with(|| AutoFilter { range: table_range, criteria: vec![] });
        let off = abs_col.saturating_sub(af.range.start.col);
        af.criteria.retain(|(o, _)| *o != off);
        if let Some(values) = selected {
            af.criteria.push((off, FilterCriterion::Values { values, blanks: false }));
        }
    }
    super::data::apply_filter(wb, sheet);
}

// ---------------------------------------------------------------- PivotTable slicers

/// The PivotTable a slicer command targets: `pivot` (name, id or a cell of the report), else the
/// report under the active cell.
fn target_pivot(s: &Session, p: &Json) -> Result<(usize, usize)> {
    let d = s.doc()?;
    if let Some(v) = p.get("pivot").filter(|v| !v.is_null()) {
        for si in 0..d.wb.sheets.len() {
            let Some(sh) = d.wb.sheet(si) else { continue };
            let hit = sh.pivots.iter().position(|pt| match v {
                Json::String(n) => pt.name.eq_ignore_ascii_case(n),
                Json::Number(n) => n.as_u64() == Some(pt.id as u64),
                _ => false,
            });
            if let Some(i) = hit {
                return Ok((si, i));
            }
        }
        return Err(bad("slicer", format!("no PivotTable `{v}`")));
    }
    let active = d.wb.active_sheet;
    let at = d.selection.active;
    let sh = d.wb.active().ok_or(EngineError::NoDocument)?;
    if let Some(i) = sh.pivots.iter().position(|pt| pt.last_range.is_some_and(|r| r.contains(at)) || pt.anchor == at) {
        return Ok((active, i));
    }
    Err(EngineError::Disabled("slicer".into(), "select a cell in a PivotTable (or pass `pivot` and `field`)".into()))
}

/// Whether the active cell is in a PivotTable (so Insert ▸ Slicer should bind to it rather than to
/// a table under it).
fn pivot_at_selection(s: &Session) -> bool {
    s.doc().is_ok_and(|d| d.wb.active().is_some_and(|sh| sh.pivots.iter().any(|pt| pt.last_range.is_some_and(|r| r.contains(d.selection.active)))))
}

/// The field a slicer shows when the caller names none: the report's first row field, then its
/// first column field, its first filter, the field of its first value, and finally the source's
/// first header.
fn default_field(pt: &PivotTable, src: &pv::Source) -> Option<String> {
    pt.rows
        .first()
        .or_else(|| pt.columns.first())
        .map(|f| f.source_col.clone())
        .or_else(|| pt.filters.first().map(|f| f.source_col.clone()))
        .or_else(|| pt.values.first().map(|v| v.source_col.clone()))
        .or_else(|| src.headers.first().cloned())
}

/// The distinct values of a source column, in the cache's item order (rows of first appearance,
/// skipping rows that are wholly blank, as the pivot cache does). Every tile starts selected.
fn source_items(src: &pv::Source, col: usize) -> Vec<SlicerItem> {
    const MAX: usize = 100_000;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for row in &src.rows {
        if row.iter().all(|v| v.scalar().is_empty()) {
            continue;
        }
        let v = row.get(col).cloned().unwrap_or(Value::Empty);
        let value = SlicerValue::from_value(&v);
        if seen.insert(value.key()) {
            out.push(SlicerItem::new(value, true));
            if out.len() >= MAX {
                break;
            }
        }
    }
    out
}

/// A cell to the right of a PivotTable's report, where Excel would put a new slicer.
fn beside_pivot(pt: &PivotTable) -> CellRef {
    let r = pt.last_range.unwrap_or_else(|| RangeRef::new(pt.anchor, pt.anchor));
    CellRef::new(r.start.row, r.end.col.saturating_add(2).min(gridcraft_core::MAX_COLS - 1))
}

/// A slicer name unique in the workbook (the file's slicer parts are named by it).
fn unique_name(wb: &Workbook, wanted: &str) -> String {
    let taken = |n: &str| wb.sheets.iter().any(|s| s.pivot_slicers.iter().any(|x| x.name.eq_ignore_ascii_case(n)));
    let base: String = wanted.trim().chars().take(64).collect();
    let base = if base.is_empty() { "Slicer".to_string() } else { base };
    if !taken(&base) {
        return base;
    }
    (2..).map(|i| format!("{base}{i}")).find(|n| !taken(n)).unwrap_or(base)
}

fn known_style(cmd: &str, style: &str) -> Result<String> {
    if SLICER_STYLES.iter().any(|s| s.eq_ignore_ascii_case(style.trim())) {
        Ok(style.trim().to_string())
    } else {
        Err(bad(cmd, format!("unknown slicer style `{style}` (one of {})", SLICER_STYLES.join(", "))))
    }
}

/// The model's slicer a call targets: by id or name on any sheet, else the last one on the active
/// sheet.
fn find_pivot_slicer(s: &Session, p: &Json) -> Option<(usize, usize)> {
    let d = s.doc().ok()?;
    match p.get("slicer").filter(|v| !v.is_null()) {
        Some(Json::Number(n)) => {
            let id = n.as_u64()?;
            for si in 0..d.wb.sheets.len() {
                let Some(sh) = d.wb.sheet(si) else { continue };
                if let Some(i) = sh.pivot_slicers.iter().position(|x| x.id as u64 == id) {
                    return Some((si, i));
                }
            }
            None
        }
        Some(Json::String(name)) => {
            for si in 0..d.wb.sheets.len() {
                let Some(sh) = d.wb.sheet(si) else { continue };
                if let Some(i) = sh.pivot_slicers.iter().position(|x| x.name.eq_ignore_ascii_case(name)) {
                    return Some((si, i));
                }
            }
            None
        }
        Some(_) => None,
        None => {
            let si = d.wb.active_sheet;
            let n = d.wb.sheet(si)?.pivot_slicers.len();
            n.checked_sub(1).map(|i| (si, i))
        }
    }
}

/// The timeline a call targets: by id or name on any sheet, else the last one on the active sheet.
fn find_timeline(s: &Session, p: &Json) -> Option<(usize, usize)> {
    let d = s.doc().ok()?;
    match p.get("slicer").or_else(|| p.get("timeline")).filter(|v| !v.is_null()) {
        Some(Json::Number(n)) => {
            let id = n.as_u64()?;
            for si in 0..d.wb.sheets.len() {
                let Some(sh) = d.wb.sheet(si) else { continue };
                if let Some(i) = sh.pivot_timelines.iter().position(|x| x.id as u64 == id) {
                    return Some((si, i));
                }
            }
            None
        }
        Some(Json::String(name)) => {
            for si in 0..d.wb.sheets.len() {
                let Some(sh) = d.wb.sheet(si) else { continue };
                if let Some(i) = sh.pivot_timelines.iter().position(|x| x.name.eq_ignore_ascii_case(name)) {
                    return Some((si, i));
                }
            }
            None
        }
        Some(_) => None,
        None => {
            let si = d.wb.active_sheet;
            let n = d.wb.sheet(si)?.pivot_timelines.len();
            n.checked_sub(1).map(|i| (si, i))
        }
    }
}

/// Applies a PivotTable slicer's tile selection to every report it filters: the selected tiles
/// become the field's item filter (an empty selection hides every row, "all selected" removes the
/// filter), and each report is re-rendered. Returns how many reports were refreshed.
///
/// The labels are the report's own item labels (`pv::item_label`), derived from the source rows
/// whose value is one of the selected tiles, so a filter the engine applies on the next refresh
/// compares equal to the labels the report shows.
fn apply_pivot_slicer(wb: &mut Workbook, widget: &PivotSlicer) -> Result<usize> {
    let keys: Option<HashSet<String>> =
        if widget.all_selected() { None } else { Some(widget.items.iter().filter(|i| i.selected).map(|i| i.value.key()).collect()) };
    let mut refreshed = 0;
    for (si, pi) in widget_targets(wb, &widget.pivot_tables) {
        let Some(pt) = wb.sheet(si).and_then(|s| s.pivots.get(pi)).cloned() else { continue };
        let Ok(src) = pv::read_source(wb, &pt.source, si) else { continue };
        let Some(col) = src.col(&widget.field) else { continue };
        let fmt = src.formats.get(col).cloned().unwrap_or_default();
        let labels: Option<Vec<String>> = match &keys {
            None => None,
            Some(keys) => {
                let mut out: Vec<String> = vec![];
                for row in &src.rows {
                    let v = row.get(col).cloned().unwrap_or(Value::Empty);
                    if !keys.contains(&SlicerValue::from_value(&v).key()) {
                        continue;
                    }
                    let label = pv::item_label(wb, &v, PivotDateGroup::None, &fmt);
                    if !out.iter().any(|x| x.eq_ignore_ascii_case(&label)) {
                        out.push(label);
                    }
                }
                Some(out)
            }
        };
        filter_pivot_field(wb, si, pi, &widget.field, labels)?;
        refreshed += 1;
    }
    Ok(refreshed)
}

/// Applies a timeline's date range to every report it filters, through the same filter path a
/// slicer's tiles use: the source rows whose date is inside the range contribute their item
/// labels, and that label set becomes the field's [`PivotFilter`] before the report is re-rendered.
/// No range removes the filter (`None` = every date). A period the range only partly covers still
/// shows whole, because the filter is the report's own items — the representation the rest of the
/// pivot model filters by — not a per-row date test.
fn apply_pivot_timeline(wb: &mut Workbook, widget: &PivotTimeline) -> Result<usize> {
    let mut refreshed = 0;
    for (si, pi) in widget_targets(wb, &widget.pivot_tables) {
        let Some(pt) = wb.sheet(si).and_then(|s| s.pivots.get(pi)).cloned() else { continue };
        let Ok(src) = pv::read_source(wb, &pt.source, si) else { continue };
        let Some(col) = src.col(&widget.field) else { continue };
        let labels = match widget.range {
            None => None,
            Some(range) => {
                // The grouping `pivot.refresh` labels this field with: its axis grouping when it is
                // in Rows or Columns, nothing otherwise (the cell's own text).
                let group = pt
                    .rows
                    .iter()
                    .chain(pt.columns.iter())
                    .find(|f| f.source_col.eq_ignore_ascii_case(&widget.field))
                    .map(|f| f.date_group)
                    .unwrap_or_default();
                Some(range_labels(wb, &src, col, group, range))
            }
        };
        filter_pivot_field(wb, si, pi, &widget.field, labels)?;
        refreshed += 1;
    }
    Ok(refreshed)
}

/// The item labels a range selects: one per distinct label of the source rows whose date is inside
/// the range, labelled the way the report labels that field ([`PivotDateGroup`] when it is on an
/// axis). Bounded like every other item list.
fn range_labels(wb: &Workbook, src: &pv::Source, col: usize, group: PivotDateGroup, range: TimelineRange) -> Vec<String> {
    const MAX: usize = 100_000;
    let fmt = src.formats.get(col).cloned().unwrap_or_default();
    let mut out: Vec<String> = vec![];
    for row in &src.rows {
        let Some(v) = row.get(col) else { continue };
        let Value::Number(n) = v.scalar() else { continue };
        if !range.contains(n) {
            continue;
        }
        let label = pv::item_label(wb, v, group, &fmt);
        if !out.iter().any(|x| x.eq_ignore_ascii_case(&label)) {
            out.push(label);
            if out.len() >= MAX {
                break;
            }
        }
    }
    out
}

/// The reports a widget filters, as `(sheet, index)` into its sheet's `pivots`.
fn widget_targets(wb: &Workbook, pivots: &[String]) -> Vec<(usize, usize)> {
    let mut out = vec![];
    for (si, sheet) in wb.sheets.iter().enumerate() {
        for (pi, pt) in sheet.pivots.iter().enumerate() {
            if pivots.iter().any(|n| n.eq_ignore_ascii_case(&pt.name)) {
                out.push((si, pi));
            }
        }
    }
    out
}

/// Whether two item selections name the same set of labels (case-insensitive, order-free). The
/// report stores labels as the caller typed them; the widget compares tile labels the same way.
fn same_selection(a: &[String], b: &[String]) -> bool {
    let key = |v: &[String]| {
        let mut k: Vec<String> = v.iter().map(|s| s.trim().to_lowercase()).collect();
        k.sort();
        k.dedup();
        k
    };
    key(a) == key(b)
}

/// The selection every report connected to a slicer agrees on for `field`: `Some(None)` when they
/// all show every item, `Some(Some(labels))` when they agree on a filter, `None` when they
/// disagree, when a connected report is gone, or when there is no report at all. A slicer shows a
/// selection that really applies to its reports; it never mirrors one report while another differs.
fn agreed_selection(wb: &Workbook, pivots: &[String], field: &str) -> Option<Option<Vec<String>>> {
    let mut agreed: Option<Option<Vec<String>>> = None;
    let mut seen = 0usize;
    for name in pivots {
        let pt = wb.sheets.iter().flat_map(|s| s.pivots.iter()).find(|p| p.name.eq_ignore_ascii_case(name))?;
        let selection = pt.filters.iter().find(|f| f.source_col.eq_ignore_ascii_case(field)).and_then(|f| f.selected.clone());
        if seen == 0 {
            agreed = Some(selection);
        } else {
            match (agreed.as_ref().and_then(|x| x.as_ref()), selection.as_ref()) {
                (None, None) => {}
                (Some(a), Some(b)) if same_selection(a, b) => {}
                _ => return None,
            }
        }
        seen = seen.saturating_add(1);
    }
    if seen == 0 { None } else { agreed }
}

/// Mirrors a filter changed on the PivotTable itself onto a slicer connected to it, so the tiles
/// and the report cannot disagree (Excel's slicer shows the field's item selection, however it was
/// changed). Only a label set that every connected report agrees on is applied, and only when every
/// label names a tile; a grouped date label or a number-format-dependent label has no tile to
/// select, so the widget keeps its state instead of showing a filter the model does not hold.
/// Timelines keep their own range: a date range is not derivable from item labels.
pub(crate) fn sync_slicers_from_pivot(wb: &mut Workbook, si: usize, pi: usize) {
    let Some(name) = wb.sheet(si).and_then(|s| s.pivots.get(pi)).map(|p| p.name.clone()) else { return };
    // Collect the work first: `agreed_selection` reads the whole workbook through `&`.
    let mut plan: Vec<(usize, usize, Option<Vec<String>>)> = vec![];
    for (sheet_i, sh) in wb.sheets.iter().enumerate() {
        for (wi, sl) in sh.pivot_slicers.iter().enumerate() {
            if !sl.pivot_tables.iter().any(|p| p.eq_ignore_ascii_case(&name)) {
                continue;
            }
            if let Some(selection) = agreed_selection(wb, &sl.pivot_tables, &sl.field) {
                plan.push((sheet_i, wi, selection));
            }
        }
    }
    for (sheet_i, wi, selection) in plan {
        let Some(sl) = wb.sheet_mut(sheet_i).and_then(|s| s.pivot_slicers.get_mut(wi)) else { continue };
        match selection {
            None => {
                sl.select(None);
            }
            Some(labels) => {
                let mut trial = sl.clone();
                if trial.select(Some(&labels)).is_empty() {
                    *sl = trial;
                }
            }
        }
    }
}

/// Writes `labels` as a report's filter on `field` (`None` removes the field's filter) and
/// re-renders it: the one filter path a slicer's tiles and a timeline's range both use.
fn filter_pivot_field(wb: &mut Workbook, si: usize, pi: usize, field: &str, labels: Option<Vec<String>>) -> Result<()> {
    if let Some(p) = wb.sheet_mut(si).and_then(|s| s.pivots.get_mut(pi)) {
        let at = p.filters.iter().position(|f| f.source_col.eq_ignore_ascii_case(field));
        match (&labels, at) {
            (Some(sel), Some(i)) => {
                if let Some(f) = p.filters.get_mut(i) {
                    f.selected = Some(sel.clone());
                }
            }
            (Some(sel), None) => p.filters.push(PivotFilter { source_col: field.to_string(), selected: Some(sel.clone()) }),
            (None, Some(i)) => {
                p.filters.remove(i);
            }
            (None, None) => {}
        }
    }
    pv::refresh(wb, si, pi, false).map(|_| ()).map_err(EngineError::Other)
}

/// Removes a PivotTable slicer's filter from the reports it filtered (deleting the slicer leaves
/// the report unfiltered, as deleting a table slicer clears its criterion).
fn clear_pivot_slicer_filter(wb: &mut Workbook, widget: &PivotSlicer) {
    let mut targets = vec![];
    for (si, sheet) in wb.sheets.iter().enumerate() {
        for (pi, pt) in sheet.pivots.iter().enumerate() {
            if widget.pivot_tables.iter().any(|n| n.eq_ignore_ascii_case(&pt.name)) {
                targets.push((si, pi));
            }
        }
    }
    for (si, pi) in targets {
        let cleared = match wb.sheet_mut(si).and_then(|s| s.pivots.get_mut(pi)) {
            Some(p) => {
                let before = p.filters.len();
                p.filters.retain(|f| !f.source_col.eq_ignore_ascii_case(&widget.field));
                p.filters.len() != before
            }
            None => false,
        };
        if cleared {
            let _ = pv::refresh(wb, si, pi, false);
        }
    }
}

// ---------------------------------------------------------------- insert

fn insert_slicer(s: &mut Session, p: &Json) -> Result<Json> {
    // A table named by `table` is found on any sheet, as the other table commands find it, and the slicer goes on that
    // table's sheet. A PivotTable under the active cell (or named by `pivot`) takes precedence only when no table was
    // named: Excel's Insert ▸ Slicer binds to the PivotTable you are in.
    if str_param(p, "table").is_none() && (str_param(p, "pivot").is_some() || pivot_at_selection(s)) {
        return insert_pivot_slicer(s, p);
    }
    let named = match str_param(p, "table") {
        Some(name) => Some(s.doc()?.wb.table(name).ok_or_else(|| bad("insert.slicer", format!("no table named `{name}`")))?),
        None => None,
    };
    let sheet = match named {
        Some((si, _)) => si,
        None => target_sheet(s, p)?,
    };
    let d = s.doc()?;
    let sh = d.wb.sheet(sheet).ok_or(EngineError::NoDocument)?;
    let t = match named {
        Some((_, ti)) => sh.tables.get(ti),
        None => sh.table_at(d.selection.active),
    }
    .ok_or_else(|| EngineError::Other("Select a cell inside a table first, or pass `table`.".into()))?;
    let column = match str_param(p, "column") {
        Some(c) => t
            .columns
            .iter()
            .find(|tc| tc.name.eq_ignore_ascii_case(c))
            .map(|tc| tc.name.clone())
            .ok_or_else(|| bad("insert.slicer", format!("table has no column `{c}`")))?,
        None => {
            // Default to the active cell's column within the table, else the first column.
            let idx = d.selection.active.col.checked_sub(t.range.start.col).map(|o| o as usize).filter(|o| *o < t.columns.len()).unwrap_or(0);
            t.columns.get(idx).map(|tc| tc.name.clone()).ok_or_else(|| bad("insert.slicer", "the table has no columns"))?
        }
    };
    let table = t.name.clone();
    let at = cell_param_on(s, p, "at", sheet)?
        .unwrap_or_else(|| CellRef::new(t.range.start.row, t.range.end.col.saturating_add(2).min(gridcraft_core::MAX_COLS - 1)));
    let columns = u32_param(p, "columns").unwrap_or(1).clamp(1, 10);
    let width = f64_param(p, "width").unwrap_or(168.0).clamp(60.0, 10000.0) as f32;
    let height = f64_param(p, "height").unwrap_or(200.0).clamp(60.0, 10000.0) as f32;
    let id = d.wb.next_object_id();
    let caption = str_param(p, "caption").map(str::to_string).unwrap_or_else(|| column.clone());
    let slicer = Slicer {
        id,
        table,
        column,
        caption,
        anchor: Anchor { cell: at, dx: 4.0, dy: 4.0, width, height, mode: AnchorMode::MoveOnly },
        selected: None,
        columns,
    };
    edit(s, |cx| {
        cx.sheet_mut(sheet)?.slicers.push(slicer);
        Ok(json!({"slicer": id}))
    })
}

fn insert_pivot_slicer(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "insert.slicer";
    let (psi, pi) = target_pivot(s, p)?;
    let d = s.doc()?;
    let pt = d.wb.sheet(psi).and_then(|sh| sh.pivots.get(pi)).cloned().ok_or(EngineError::NoDocument)?;
    let src = pv::read_source(&d.wb, &pt.source, psi).map_err(|e| bad(C, e))?;
    let field = match str_param(p, "field").or_else(|| str_param(p, "column")) {
        Some(f) => src
            .headers
            .iter()
            .find(|h| h.eq_ignore_ascii_case(f))
            .cloned()
            .ok_or_else(|| bad(C, format!("the PivotTable's source has no field `{f}`")))?,
        None => default_field(&pt, &src).ok_or_else(|| bad(C, "the PivotTable has no fields"))?,
    };
    let col = src.col(&field).ok_or_else(|| bad(C, format!("no column for `{field}`")))?;
    let items = source_items(&src, col);
    let columns = u32_param(p, "columns").unwrap_or(1).clamp(1, 16);
    let style = match str_param(p, "style") {
        Some(style) => known_style(C, style)?,
        None => DEFAULT_SLICER_STYLE.to_string(),
    };
    let at = cell_param_on(s, p, "at", psi)?.unwrap_or_else(|| beside_pivot(&pt));
    let width = f64_param(p, "width").unwrap_or(168.0).clamp(60.0, 10000.0) as f32;
    let height = f64_param(p, "height").unwrap_or(200.0).clamp(60.0, 10000.0) as f32;
    let name = unique_name(&d.wb, str_param(p, "name").unwrap_or(&field));
    let cache = unique_cache_name(&d.wb, &name);
    let caption = str_param(p, "caption").map(str::to_string).unwrap_or_else(|| field.clone());
    let id = d.wb.next_object_id();
    let widget = PivotSlicer {
        id,
        name: name.clone(),
        cache: cache.clone(),
        caption,
        field,
        pivot_cache_id: 0,
        pivot_tables: vec![pt.name.clone()],
        items,
        style,
        columns,
        sort: SlicerSort::Ascending,
        anchor: Anchor { cell: at, dx: 4.0, dy: 4.0, width, height, mode: AnchorMode::MoveOnly },
    };
    edit(s, |cx| {
        cx.sheet_mut(psi)?.pivot_slicers.push(widget);
        Ok(json!({"slicer": id, "name": name, "cache": cache}))
    })
}

/// A slicer cache name unique in the workbook (a cache is shared by every slicer naming it).
fn unique_cache_name(wb: &Workbook, name: &str) -> String {
    let taken = |n: &str| {
        wb.sheets.iter().any(|s| s.pivot_slicers.iter().any(|x| x.cache.eq_ignore_ascii_case(n)))
            || wb.sheets.iter().any(|s| s.pivot_timelines.iter().any(|x| x.cache.eq_ignore_ascii_case(n)))
    };
    let base = format!("Slicer_{name}");
    if !taken(&base) {
        return base;
    }
    (2..).map(|i| format!("Slicer_{name}{i}")).find(|n| !taken(n)).unwrap_or(base)
}

fn insert_timeline(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "insert.timeline";
    let (psi, pi) = target_pivot(s, p)?;
    let d = s.doc()?;
    let pt = d.wb.sheet(psi).and_then(|sh| sh.pivots.get(pi)).cloned().ok_or(EngineError::NoDocument)?;
    let src = pv::read_source(&d.wb, &pt.source, psi).map_err(|e| bad(C, e))?;
    let field = match str_param(p, "field").or_else(|| str_param(p, "column")) {
        Some(f) => src
            .headers
            .iter()
            .find(|h| h.eq_ignore_ascii_case(f))
            .cloned()
            .ok_or_else(|| bad(C, format!("the PivotTable's source has no field `{f}`")))?,
        None => default_field(&pt, &src).ok_or_else(|| bad(C, "the PivotTable has no fields"))?,
    };
    let unit = match str_param(p, "unit") {
        Some(u) => TimeUnit::parse(u).ok_or_else(|| bad(C, format!("unknown time unit `{u}` (years|quarters|months|days)")))?,
        None => TimeUnit::Months,
    };
    // A timeline is a date axis: its cache needs the field's date bounds, so a field that holds no
    // dates is refused here rather than held in the model and dropped when the file is written.
    let is_date = src.col(&field).and_then(|i| src.dates.get(i)).copied().unwrap_or(false);
    if !is_date {
        return Err(bad(C, format!("a timeline needs a field of dates; `{field}` holds none")));
    }
    let at = cell_param_on(s, p, "at", psi)?.unwrap_or_else(|| beside_pivot(&pt));
    let width = f64_param(p, "width").unwrap_or(240.0).clamp(60.0, 10000.0) as f32;
    let height = f64_param(p, "height").unwrap_or(96.0).clamp(40.0, 10000.0) as f32;
    let name = unique_name(&d.wb, str_param(p, "name").unwrap_or(&field));
    let cache = unique_cache_name(&d.wb, &name);
    let caption = str_param(p, "caption").map(str::to_string).unwrap_or_else(|| field.clone());
    let style = str_param(p, "style").unwrap_or("").to_string();
    let id = d.wb.next_object_id();
    let widget = PivotTimeline {
        id,
        name: name.clone(),
        cache: cache.clone(),
        caption,
        field,
        pivot_cache_id: 0,
        pivot_tables: vec![pt.name.clone()],
        style,
        unit,
        range: None,
        anchor: Anchor { cell: at, dx: 4.0, dy: 4.0, width, height, mode: AnchorMode::MoveOnly },
    };
    edit(s, |cx| {
        cx.sheet_mut(psi)?.pivot_timelines.push(widget);
        Ok(json!({"timeline": id, "name": name, "cache": cache}))
    })
}

// ---------------------------------------------------------------- selection

/// The selected labels of a `selected` param: a list of tile labels, one label, or `null` (all).
fn selected_param(cmd: &str, p: &Json) -> Result<Option<Vec<String>>> {
    Ok(match p.get("selected") {
        None | Some(Json::Null) => None,
        Some(Json::Array(a)) => {
            if a.len() > 100_000 {
                return Err(bad(cmd, "too many labels"));
            }
            Some(
                a.iter()
                    .map(|x| match x {
                        Json::String(s) => s.clone(),
                        Json::Number(n) => n.to_string(),
                        Json::Bool(b) => {
                            if *b {
                                "TRUE".into()
                            } else {
                                "FALSE".into()
                            }
                        }
                        other => other.to_string(),
                    })
                    .collect(),
            )
        }
        Some(Json::String(s)) => Some(vec![s.clone()]),
        Some(_) => return Err(bad(cmd, "`selected` is a list of tile labels or null")),
    })
}

fn set_selection(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "slicer.setSelection";
    let labels = selected_param(C, p)?;
    if let Some((si, wi)) = find_pivot_slicer(s, p) {
        let d = s.doc()?;
        let mut widget = d.wb.sheet(si).and_then(|sh| sh.pivot_slicers.get(wi)).cloned().ok_or_else(|| bad(C, "no slicer"))?;
        let unmatched = widget.select(labels.as_deref());
        if let Some(first) = unmatched.first() {
            return Err(bad(C, format!("`{first}` is not a tile of `{}`", widget.name)));
        }
        let selected = widget.selected_labels();
        let name = widget.name.clone();
        return edit(s, |cx| {
            if let Some(slot) = cx.sheet_mut(si)?.pivot_slicers.get_mut(wi) {
                *slot = widget.clone();
            }
            let pivots = apply_pivot_slicer(&mut cx.wb, &widget)?;
            cx.structural = pivots > 0;
            Ok(json!({"slicer": name, "selected": selected, "pivots": pivots}))
        });
    }
    // A table slicer: the same selection becomes the column's filter.
    let (idx, id) = find_slicer(s, p)?;
    let sheet = s.doc()?.wb.active_sheet;
    let (table, column) = {
        let d = s.doc()?;
        let sl = d.wb.active().and_then(|sh| sh.slicers.get(idx)).ok_or_else(|| bad(C, "no slicer"))?;
        (sl.table.clone(), sl.column.clone())
    };
    let all = column_values(&s.doc()?.wb, sheet, &table, &column);
    let new_selected = match &labels {
        None => None,
        Some(l) => {
            if let Some(bad_label) = l.iter().find(|x| !all.iter().any(|a| a.eq_ignore_ascii_case(x.trim()))) {
                return Err(bad(C, format!("`{bad_label}` is not a value of the column")));
            }
            if all.iter().all(|a| l.iter().any(|x| x.eq_ignore_ascii_case(a))) { None } else { Some(l.clone()) }
        }
    };
    edit(s, |cx| {
        if let Some(sl) = cx.sheet_mut(sheet)?.slicers.get_mut(idx) {
            sl.selected = new_selected.clone();
        }
        reapply_slicer(&mut cx.wb, sheet, id);
        cx.structural = true;
        Ok(json!({"slicer": id}))
    })
}

/// A date parameter of a timeline range: an ISO date text that names its year (`2024-01-31`,
/// `1/31/2024`), or null/absent for "no date". Serial numbers and year-less dates are refused:
/// a range is a statement about a calendar, not an offset.
fn date_param(cmd: &str, name: &str, v: Option<&Json>, sys: DateSystem) -> Result<Option<f64>> {
    match v {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(s)) => {
            let parsed = gridcraft_core::parse::parse_input(s, sys);
            match parsed.value {
                Value::Number(n) if n.is_finite() && parsed.format.is_some_and(|f| f.contains('y')) => Ok(Some(n)),
                _ => Err(bad(cmd, format!("`{name}` is not a date with a year (e.g. \"2024-01-31\"): `{s}`"))),
            }
        }
        Some(_) => Err(bad(cmd, format!("`{name}` is an ISO date string, or null for every date"))),
    }
}

/// A date as the command reports it: `2024-01-31`, with the time when the date has one.
fn iso_text(wb: &Workbook, serial: f64) -> Option<String> {
    let d = gridcraft_core::date::datetime_from_serial(wb.date_system, serial)?;
    Some(if d.hour == 0 && d.minute == 0 && d.second == 0 {
        format!("{:04}-{:02}-{:02}", d.year, d.month, d.day)
    } else {
        format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", d.year, d.month, d.day, d.hour, d.minute, d.second)
    })
}

/// The first and last date the timeline's field holds, in its report's source: what the timeline
/// cache's `bounds` are written from, so a range is clamped to dates the file can show.
fn field_bounds(wb: &Workbook, src: &pv::Source, col: usize) -> Option<(f64, f64)> {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for row in &src.rows {
        let Some(Value::Number(n)) = row.get(col).map(|v| v.scalar()) else { continue };
        if !n.is_finite() || gridcraft_core::date::datetime_from_serial(wb.date_system, n).is_none() {
            continue;
        }
        lo = lo.min(n);
        hi = hi.max(n);
    }
    (lo <= hi).then_some((lo, hi))
}

/// The field's own date axis for `tl`: its first and last date as serial days, found through the
/// reports the timeline filters. The UI draws the selected range on this axis and maps a dragged
/// handle back to a date; `slicer.setRange` clamps to the same bounds. `None` when no report is
/// left, the field is gone, or it holds no dates.
///
/// One scan per call: the grid caches the answer per document revision, because the source can be
/// large and a timeline does not move between edits.
pub fn timeline_axis(wb: &Workbook, tl: &PivotTimeline) -> Option<(f64, f64)> {
    for name in &tl.pivot_tables {
        let Some((si, pi)) =
            wb.sheets.iter().enumerate().find_map(|(si, s)| s.pivots.iter().position(|p| p.name.eq_ignore_ascii_case(name)).map(|pi| (si, pi)))
        else {
            continue;
        };
        let Some(pt) = wb.sheet(si).and_then(|s| s.pivots.get(pi)) else { continue };
        let Ok(src) = pv::read_source(wb, &pt.source, si) else { continue };
        let Some(col) = src.col(&tl.field) else { continue };
        if let Some(bounds) = field_bounds(wb, &src, col) {
            return Some(bounds);
        }
    }
    None
}

/// The dates a timeline shows. One undo step: the model's range, the bound reports' filters and
/// their cells move together.
///
/// Refused: no timeline (a pivot slicer has tiles, not a range), only one of the two dates, a
/// start after its end, a field that holds no dates, and a date text without a year. A range that
/// runs past the field's own first or last date is **clamped** to it (reported by `clamped`) rather
/// than refused, so a drag past the end of the axis still lands on a date the file can show.
/// `start: null, end: null` clears the range and removes the report's filter.
fn set_range(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "slicer.setRange";
    let Some((si, wi)) = find_timeline(s, p) else {
        return Err(if find_pivot_slicer(s, p).is_some() {
            bad(C, "a date range needs a timeline, not a slicer")
        } else {
            bad(C, "no timeline on this sheet (pass `slicer` or `timeline`)")
        });
    };
    let d = s.doc()?;
    let mut widget = d.wb.sheet(si).and_then(|sh| sh.pivot_timelines.get(wi)).cloned().ok_or_else(|| bad(C, "no timeline"))?;
    let start = date_param(C, "start", p.get("start"), d.wb.date_system)?;
    let end = date_param(C, "end", p.get("end"), d.wb.date_system)?;
    let wanted = match (start, end) {
        (None, None) => None,
        (Some(a), Some(b)) => {
            if a > b {
                return Err(bad(C, "`start` is after `end`"));
            }
            Some((a, b))
        }
        _ => return Err(bad(C, "pass both `start` and `end`, or null for both to show every date")),
    };
    let mut clamped = false;
    let range = match wanted {
        None => None,
        Some((a, b)) => {
            // The date axis the range is drawn on: the timeline's own field in its report's source.
            let (psi, pi) = widget
                .pivot_tables
                .iter()
                .find_map(|name| {
                    d.wb.sheets
                        .iter()
                        .enumerate()
                        .find_map(|(si, sh)| sh.pivots.iter().position(|pt| pt.name.eq_ignore_ascii_case(name)).map(|pi| (si, pi)))
                })
                .ok_or_else(|| bad(C, "the timeline's PivotTable is gone"))?;
            let pt = d.wb.sheet(psi).and_then(|sh| sh.pivots.get(pi)).cloned().ok_or_else(|| bad(C, "the timeline's PivotTable is gone"))?;
            let src = pv::read_source(&d.wb, &pt.source, psi).map_err(|e| bad(C, e))?;
            let col = src.col(&widget.field).ok_or_else(|| bad(C, format!("the report's source has no field `{}`", widget.field)))?;
            if !src.dates.get(col).copied().unwrap_or(false) {
                return Err(bad(C, format!("a timeline needs a field of dates; `{}` holds none", widget.field)));
            }
            let (lo, hi) = field_bounds(&d.wb, &src, col).ok_or_else(|| bad(C, format!("`{}` holds no dates to filter", widget.field)))?;
            let (start, end) = (a.clamp(lo, hi), b.clamp(lo, hi));
            clamped = start != a || end != b;
            Some(TimelineRange::new(start, end))
        }
    };
    let name = widget.name.clone();
    widget.range = range;
    edit(s, |cx| {
        if let Some(slot) = cx.sheet_mut(si)?.pivot_timelines.get_mut(wi) {
            *slot = widget.clone();
        }
        let pivots = apply_pivot_timeline(&mut cx.wb, &widget)?;
        cx.structural = pivots > 0;
        let shown = widget.range.map(|r| (iso_text(&cx.wb, r.start), iso_text(&cx.wb, r.end)));
        let (start, end) = shown.unwrap_or((None, None));
        Ok(json!({"timeline": name, "start": start, "end": end, "pivots": pivots, "clamped": clamped}))
    })
}

fn slicer_toggle(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "slicer.toggle";
    let value = str_param(p, "value").ok_or_else(|| bad(C, "missing `value`"))?.to_string();
    let multi = bool_param(p, "multi").unwrap_or(false);
    if let Some((si, wi)) = find_pivot_slicer(s, p) {
        let d = s.doc()?;
        let mut widget = d.wb.sheet(si).and_then(|sh| sh.pivot_slicers.get(wi)).cloned().ok_or_else(|| bad(C, "no slicer"))?;
        let Some(pos) = widget.items.iter().position(|i| i.label().eq_ignore_ascii_case(&value)) else {
            return Err(bad(C, "value is not in the field"));
        };
        let all_on = widget.all_selected();
        if multi {
            let mut on: Vec<bool> = if all_on { vec![true; widget.items.len()] } else { widget.items.iter().map(|i| i.selected).collect() };
            if let Some(o) = on.get_mut(pos) {
                *o = !*o;
            }
            for (item, on) in widget.items.iter_mut().zip(on.iter()) {
                item.selected = *on;
            }
        } else if all_on {
            // Clicking one tile of an unfiltered slicer shows only that tile.
            for (i, item) in widget.items.iter_mut().enumerate() {
                item.selected = i == pos;
            }
        } else if widget.items.iter().enumerate().filter(|(_, i)| i.selected).count() == 1 && widget.items.get(pos).is_some_and(|i| i.selected) {
            // Clicking the only selected tile clears the filter.
            for item in &mut widget.items {
                item.selected = true;
            }
        } else {
            for (i, item) in widget.items.iter_mut().enumerate() {
                item.selected = i == pos;
            }
        }
        let selected = widget.selected_labels();
        let name = widget.name.clone();
        return edit(s, |cx| {
            if let Some(slot) = cx.sheet_mut(si)?.pivot_slicers.get_mut(wi) {
                *slot = widget.clone();
            }
            let pivots = apply_pivot_slicer(&mut cx.wb, &widget)?;
            cx.structural = pivots > 0;
            Ok(json!({"slicer": name, "selected": selected}))
        });
    }
    // Table slicer: the existing filter path.
    let (idx, id) = find_slicer(s, p)?;
    let sheet = s.doc()?.wb.active_sheet;
    let (table, column, current) = {
        let d = s.doc()?;
        let sl = d.wb.active().and_then(|sh| sh.slicers.get(idx)).ok_or_else(|| bad(C, "no slicer"))?;
        (sl.table.clone(), sl.column.clone(), sl.selected.clone())
    };
    let all = column_values(&s.doc()?.wb, sheet, &table, &column);
    if !all.iter().any(|v| v.eq_ignore_ascii_case(&value)) {
        return Err(bad(C, "value is not in the column"));
    }
    let new_selected: Option<Vec<String>> = if multi {
        let mut set: Vec<String> = current.unwrap_or_else(|| all.clone());
        match set.iter().position(|x| x.eq_ignore_ascii_case(&value)) {
            Some(pos) => {
                set.remove(pos);
            }
            None => set.push(value.clone()),
        }
        if set.len() == all.len() && all.iter().all(|a| set.iter().any(|x| x.eq_ignore_ascii_case(a))) { None } else { Some(set) }
    } else {
        match &current {
            Some(set) if set.len() == 1 && set.iter().any(|x| x.eq_ignore_ascii_case(&value)) => None,
            _ => Some(vec![value.clone()]),
        }
    };
    edit(s, |cx| {
        if let Some(sl) = cx.sheet_mut(sheet)?.slicers.get_mut(idx) {
            sl.selected = new_selected;
        }
        reapply_slicer(&mut cx.wb, sheet, id);
        cx.structural = true;
        Ok(Json::Null)
    })
}

fn slicer_select_all(s: &mut Session, p: &Json) -> Result<Json> {
    if let Some((si, wi)) = find_pivot_slicer(s, p) {
        let d = s.doc()?;
        let mut widget = d.wb.sheet(si).and_then(|sh| sh.pivot_slicers.get(wi)).cloned().ok_or_else(|| bad("slicer", "no slicer"))?;
        for item in &mut widget.items {
            item.selected = true;
        }
        let name = widget.name.clone();
        return edit(s, |cx| {
            if let Some(slot) = cx.sheet_mut(si)?.pivot_slicers.get_mut(wi) {
                *slot = widget.clone();
            }
            let pivots = apply_pivot_slicer(&mut cx.wb, &widget)?;
            cx.structural = pivots > 0;
            Ok(json!({"slicer": name, "selected": Json::Null}))
        });
    }
    let (idx, id) = find_slicer(s, p)?;
    let sheet = s.doc()?.wb.active_sheet;
    edit(s, |cx| {
        if let Some(sl) = cx.sheet_mut(sheet)?.slicers.get_mut(idx) {
            sl.selected = None;
        }
        reapply_slicer(&mut cx.wb, sheet, id);
        cx.structural = true;
        Ok(Json::Null)
    })
}

// ---------------------------------------------------------------- look and place

fn set_style(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "slicer.setStyle";
    let style = known_style(C, str_param(p, "style").ok_or_else(|| bad(C, "missing `style`"))?)?;
    // A PivotTable slicer or a timeline: both carry a style in the model.
    if let Some((si, wi)) = find_pivot_slicer(s, p) {
        let name = s.doc()?.wb.sheet(si).and_then(|sh| sh.pivot_slicers.get(wi)).map(|sl| sl.name.clone()).ok_or_else(|| bad(C, "no slicer"))?;
        return edit(s, |cx| {
            if let Some(sl) = cx.sheet_mut(si)?.pivot_slicers.get_mut(wi) {
                sl.style = style.clone();
            }
            Ok(json!({"slicer": name, "style": style}))
        });
    }
    let (si, wi) = find_timeline(s, p).ok_or_else(|| bad(C, "no PivotTable slicer or timeline on this sheet"))?;
    let name = s.doc()?.wb.sheet(si).and_then(|sh| sh.pivot_timelines.get(wi)).map(|tl| tl.name.clone()).ok_or_else(|| bad(C, "no timeline"))?;
    edit(s, |cx| {
        if let Some(tl) = cx.sheet_mut(si)?.pivot_timelines.get_mut(wi) {
            tl.style = style.clone();
        }
        Ok(json!({"slicer": name, "style": style}))
    })
}

fn move_slicer(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "slicer.move";
    let sheet = target_sheet(s, p)?;
    // Offsets inside the anchor cell, so a drag does not have to snap to a cell edge; bounded like
    // every other host of hostile numbers.
    let offset = |name: &str| f64_param(p, name).map(|v| v.clamp(0.0, 10000.0) as f32);
    let at = cell_param_on(s, p, "at", sheet)?;
    let dx = offset("dx");
    let dy = offset("dy");
    let width = f64_param(p, "width").map(|w| w.clamp(60.0, 10000.0) as f32);
    let height = f64_param(p, "height").map(|h| h.clamp(40.0, 10000.0) as f32);
    if at.is_none() && dx.is_none() && dy.is_none() && width.is_none() && height.is_none() {
        return Err(bad(C, "nothing to move: pass `at`, `dx`, `dy`, `width` or `height`"));
    }
    // A PivotTable slicer, a table slicer or a timeline with this id/name on the sheet.
    let (pivot_at, table_at, timeline_at) = {
        let d = s.doc()?;
        let Some(sh) = d.wb.sheet(sheet) else { return Err(EngineError::NoDocument) };
        let pivot = match p.get("slicer").filter(|v| !v.is_null()) {
            Some(Json::Number(n)) => sh.pivot_slicers.iter().position(|x| n.as_u64() == Some(x.id as u64)),
            Some(Json::String(name)) => sh.pivot_slicers.iter().position(|x| x.name.eq_ignore_ascii_case(name)),
            Some(_) => None,
            None => sh.pivot_slicers.len().checked_sub(1),
        };
        let table = match p.get("slicer").filter(|v| !v.is_null()) {
            Some(Json::Number(n)) => sh.slicers.iter().position(|x| n.as_u64() == Some(x.id as u64)),
            Some(_) => None,
            None if sh.pivot_slicers.is_empty() => sh.slicers.len().checked_sub(1),
            None => None,
        };
        let timeline = match p.get("slicer").or_else(|| p.get("timeline")).filter(|v| !v.is_null()) {
            Some(Json::Number(n)) => sh.pivot_timelines.iter().position(|x| n.as_u64() == Some(x.id as u64)),
            Some(Json::String(name)) => sh.pivot_timelines.iter().position(|x| x.name.eq_ignore_ascii_case(name)),
            Some(_) => None,
            None if pivot.is_none() && table.is_none() => sh.pivot_timelines.len().checked_sub(1),
            None => None,
        };
        (pivot, table, timeline)
    };
    let id = {
        let d = s.doc()?;
        let sh = d.wb.sheet(sheet).ok_or(EngineError::NoDocument)?;
        match (pivot_at, table_at, timeline_at) {
            (Some(i), ..) => sh.pivot_slicers.get(i).map(|x| x.id),
            (None, Some(i), _) => sh.slicers.get(i).map(|x| x.id),
            (None, None, Some(i)) => sh.pivot_timelines.get(i).map(|x| x.id),
            _ => None,
        }
        .ok_or_else(|| bad(C, "no slicer or timeline on this sheet"))?
    };
    edit(s, |cx| {
        let sh = cx.sheet_mut(sheet)?;
        let anchor = match (pivot_at, table_at, timeline_at) {
            (Some(i), ..) => sh.pivot_slicers.get_mut(i).map(|x| &mut x.anchor),
            (None, Some(i), _) => sh.slicers.get_mut(i).map(|x| &mut x.anchor),
            (None, None, Some(i)) => sh.pivot_timelines.get_mut(i).map(|x| &mut x.anchor),
            _ => None,
        }
        .ok_or_else(|| bad(C, "no slicer or timeline on this sheet"))?;
        if let Some(at) = at {
            anchor.cell = at;
        }
        if let Some(v) = dx {
            anchor.dx = v;
        }
        if let Some(v) = dy {
            anchor.dy = v;
        }
        if let Some(w) = width {
            anchor.width = w;
        }
        if let Some(h) = height {
            anchor.height = h;
        }
        let out = json!({"slicer": id, "at": anchor.cell.a1(), "dx": anchor.dx, "dy": anchor.dy, "width": anchor.width, "height": anchor.height});
        Ok(out)
    })
}

// ---------------------------------------------------------------- delete

fn slicer_delete(s: &mut Session, p: &Json) -> Result<Json> {
    if let Some((si, wi)) = find_pivot_slicer(s, p) {
        let widget = s.doc()?.wb.sheet(si).and_then(|sh| sh.pivot_slicers.get(wi)).cloned().ok_or_else(|| bad("slicer.delete", "no slicer"))?;
        return edit(s, |cx| {
            let sh = cx.sheet_mut(si)?;
            sh.pivot_slicers.retain(|x| x.id != widget.id);
            clear_pivot_slicer_filter(&mut cx.wb, &widget);
            cx.structural = true;
            Ok(json!({"slicer": widget.name}))
        });
    }
    if let Some((si, ti)) = find_timeline(s, p) {
        let name = s
            .doc()?
            .wb
            .sheet(si)
            .and_then(|sh| sh.pivot_timelines.get(ti))
            .map(|t| t.name.clone())
            .ok_or_else(|| bad("slicer.delete", "no timeline"))?;
        return edit(s, |cx| {
            if let Ok(sh) = cx.sheet_mut(si) {
                sh.pivot_timelines.retain(|x| x.name != name);
            }
            Ok(json!({"timeline": name}))
        });
    }
    let (idx, id) = find_slicer(s, p)?;
    let sheet = s.doc()?.wb.active_sheet;
    // Absolute column of the slicer, so its filter criterion can be removed too.
    let abs_col = s.doc().ok().and_then(|d| {
        let sh = d.wb.active()?;
        let sl = sh.slicers.get(idx)?;
        let t = sh.tables.iter().find(|t| t.name.eq_ignore_ascii_case(&sl.table))?;
        let ci = t.column_index(&sl.column)?;
        Some(t.range.start.col + ci as u32)
    });
    edit(s, |cx| {
        let sh = cx.sheet_mut(sheet)?;
        sh.slicers.retain(|x| x.id != id);
        if let (Some(abs), Some(af)) = (abs_col, sh.autofilter.as_mut()) {
            let off = abs.saturating_sub(af.range.start.col);
            af.criteria.retain(|(o, _)| *o != off);
        }
        super::data::apply_filter(&mut cx.wb, sheet);
        cx.structural = true;
        Ok(Json::Null)
    })
}

// ---------------------------------------------------------------- target table slicer

/// The table slicer a call targets: by `slicer` id, else the last one on the active sheet.
fn find_slicer(s: &Session, p: &Json) -> Result<(usize, u32)> {
    let d = s.doc()?;
    let sh = d.wb.active().ok_or(EngineError::NoDocument)?;
    let idx = match u32_param(p, "slicer") {
        Some(id) => sh.slicers.iter().position(|x| x.id == id),
        None => sh.slicers.len().checked_sub(1),
    };
    idx.and_then(|i| sh.slicers.get(i).map(|x| (i, x.id))).ok_or_else(|| bad("slicer", "no slicer on this sheet"))
}
