//! PivotTable commands (Insert › PivotTable, PivotTable Analyze, PivotTable Design, Data ›
//! Refresh All). Every change re-renders the report as one undo step.
//!
//! `pivot?` parameters name a PivotTable (name or numeric id); by default it is the pivot whose
//! report contains the active cell, else the last pivot on the active sheet.

use std::sync::Arc;

use gridcraft_core::{CellRef, RangeRef, Value};
use gridcraft_model::*;
use serde_json::{Value as Json, json};

use super::*;
use crate::pivot as pv;
use crate::selection::current_region;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "insert.pivotTable",
            "PivotTable",
            ["Insert", "Tables"],
            None,
            "{source?: \"Sheet1!$A$1:$E$200\" | table name (default: the table or current region of the selection), destination?: \"new\" (default: new sheet before the current one, report at A3) | \"Sheet1!H3\", name?, rows?: [field | {field, group?: years|quarters|months|days}], columns?: [..], values?: [field | {field, func?, name?}], filters?: [field], replace?: bool} → {id, name, sheet, anchor, fields}",
            has_doc,
            insert_pivot
        ),
        cmd!(
            "insert.recommendedPivotTables",
            "Recommended PivotTables",
            ["Insert", "Tables"],
            None,
            "{source?, apply?: index, destination?} → suggestions [{index, title, rows, columns, values}]; with apply, creates that one",
            has_doc,
            recommended
        ),
        cmd!(
            "table.summarizePivot",
            "Summarize with PivotTable",
            ["Table Design", "Tools"],
            None,
            "{table?, destination?} creates a PivotTable from the table",
            has_doc,
            summarize_table
        ),
        cmd!(
            "pivot.calculatedField",
            "Calculated Field",
            ["PivotTable Analyze", "Calculations"],
            None,
            "{pivot?, name, formula?: \"=Sales*0.1\" (fields bare or 'quoted'; evaluated on each cell's sums), delete?: bool, values?: bool (add a new field to Values, default true)} adds, changes or deletes a calculated field; without `name` opens the dialog → {name, formula, calculatedFields}",
            has_doc,
            calculated_field
        ),
        cmd!(
            "pivot.calculatedItem",
            "Calculated Item",
            [],
            None,
            "{pivot?, field: a row or column field, name, formula?: \"=North+South\" (items of that field, bare, 'quoted' or Field[item]), delete?: bool} adds, changes or deletes a calculated item: an item of the field computed from its other items and shown after them; it stays out of the grand totals → {name, field, formula, calculatedItems}",
            has_doc,
            calculated_item
        ),
        cmd!(
            "insert.pivotChart",
            "PivotChart",
            ["Insert", "Charts"],
            None,
            "{pivot?: name | id | a cell of the report (\"Sheet2!B5\"; default: the PivotTable at the active cell), type?: column|bar|line|pie|area|… (default column), subtype?, title?, at?: \"H2\", width?, height?} adds a chart bound to the PivotTable's report: categories are the innermost row items, series the column items × value fields; it follows refreshes and layout changes → {chart, pivot, series}",
            has_doc,
            insert_pivot_chart
        ),
        cmd!(query "pivot.fields", "PivotTable Fields", [], None, "{pivot?} → source fields (name, numeric, date, samples, areas) and the current layout", has_doc, fields),
        cmd!(query "pivot.fieldList", "Field List", ["PivotTable Analyze", "Show"], None, "{pivot?} (same as pivot.fields)", has_doc, fields),
        cmd!(query "pivot.list", "PivotTables", [], None, "{} → every PivotTable in the workbook", has_doc, list),
        cmd!(
            "pivot.addField",
            "Add Field",
            [],
            None,
            "{pivot?, field, area: rows|columns|values|filters, func?: sum|count|average|max|min|product|countNumbers|stdDev|stdDevP|var|varP, name?, position?: index, replace?}",
            has_doc,
            add_field
        ),
        cmd!(
            "pivot.removeField",
            "Remove Field",
            [],
            None,
            "{pivot?, field (or value caption), area?: rows|columns|values|filters, replace?}",
            has_doc,
            remove_field
        ),
        cmd!(
            "pivot.moveField",
            "Move Field",
            [],
            None,
            "{pivot?, field (\"Σ Values\" moves the value fields between rows and columns; in columns it stays after the last field), from: rows|columns|values|filters, to: rows|columns|values|filters, position?: index, replace?}",
            has_doc,
            move_field
        ),
        cmd!(
            "pivot.valueSettings",
            "Value Field Settings",
            ["PivotTable Analyze", "Active Field"],
            None,
            "{pivot?, field: caption or source field | index: n, func?, name?, showAs?: normal|percentOfGrandTotal|percentOfColumnTotal|percentOfRowTotal|runningTotal|rank, numberFormat?: \"#,##0\" | null, replace?}",
            has_doc,
            value_settings
        ),
        cmd!("pivot.filter", "Filter Items", [], None, "{pivot?, field, selected: [\"East\", ..] | null (all), replace?}", has_doc, filter),
        cmd!(
            "pivot.filterByLabel",
            "Label Filters",
            [],
            None,
            "{pivot?, field, op: equals|notEquals|beginsWith|endsWith|contains|doesNotContain|greaterThan|greaterThanOrEqual|lessThan|lessThanOrEqual|between|notBetween, value, value2? (between/notBetween), clear?: bool} keeps the items of a row or column field whose caption matches; the rule is evaluated now and stored as the field's hidden items → {field, items}",
            has_doc,
            filter_by_label
        ),
        cmd!(
            "pivot.filterByValue",
            "Value Filters",
            [],
            None,
            "{pivot?, field, measure?: a value field's caption or source column (default: the first), op: equals|notEquals|greaterThan|greaterThanOrEqual|lessThan|lessThanOrEqual|between|notBetween, value, value2?, clear?: bool} keeps the items of a row or column field whose aggregate passes; evaluated now and stored as hidden items → {field, items}",
            has_doc,
            filter_by_value
        ),
        cmd!("pivot.sort", "Sort Field", [], None, "{pivot?, field, order: asc|desc|none, replace?}", has_doc, sort),
        cmd!(
            "pivot.group",
            "Group Field",
            ["PivotTable Analyze", "Group"],
            None,
            "{pivot?, field, by: years|quarters|months|days|none (a date field), or start?, end?, interval? (a number field; start/end default to its extent), replace?}",
            has_doc,
            group
        ),
        cmd!(
            "pivot.collapse",
            "Expand/Collapse Field",
            ["PivotTable Analyze", "Active Field"],
            None,
            "{pivot?, field, items?: [labels] (default: all), collapse?: true, replace?}",
            has_doc,
            collapse
        ),
        cmd!(
            "pivot.layout",
            "PivotTable Layout",
            ["PivotTable Design", "Layout"],
            None,
            "{pivot?, layout?: compact|outline|tabular, grandTotalsRows?: bool, grandTotalsCols?: bool, subtotals?: bool|top|bottom|none, style?: \"PivotStyleLight16\", showHeaders?: bool, replace?}",
            has_doc,
            layout
        ),
        cmd!(
            "pivot.grandTotals",
            "Grand Totals",
            ["PivotTable Design", "Layout"],
            None,
            "{pivot?, mode?: off|on|rows|columns, rows?: bool, cols?: bool}",
            has_doc,
            grand_totals
        ),
        cmd!("pivot.subtotals", "Subtotals", ["PivotTable Design", "Layout"], None, "{pivot?, mode: none|top|bottom}", has_doc, |s, p| {
            let mode = str_param(p, "mode").unwrap_or("top");
            layout(s, &with(p, "subtotals", json!(mode)))
        }),
        cmd!(
            "pivot.reportLayout",
            "Report Layout",
            ["PivotTable Design", "Layout"],
            None,
            "{pivot?, layout: compact|outline|tabular}",
            has_doc,
            layout
        ),
        cmd!(
            "pivot.style",
            "PivotTable Styles",
            ["PivotTable Design", "PivotTable Styles"],
            None,
            "{pivot?, style: \"PivotStyleMedium9\"}",
            has_doc,
            layout
        ),
        cmd!("pivot.refresh", "Refresh", ["PivotTable Analyze", "Data"], Some("Alt+F5"), "{pivot?, replace?}", has_doc, refresh),
        cmd!(
            "data.refreshAll",
            "Refresh All",
            ["Data", "Queries & Connections"],
            Some("Cmd+Alt+F5"),
            "{replace?} refreshes every PivotTable → {refreshed, errors}",
            has_doc,
            refresh_all
        ),
        cmd!(
            "pivot.changeSource",
            "Change Data Source",
            ["PivotTable Analyze", "Data"],
            None,
            "{pivot?, source: \"Sheet1!$A$1:$E$300\" | table name, replace?} (fields missing from the new source are removed)",
            has_doc,
            change_source
        ),
        cmd!(
            "pivot.showDetails",
            "Show Details",
            ["PivotTable Analyze", "Active Field"],
            None,
            "{cell?: \"A5\" (a value cell of the report; default: the active cell when it is in the report, else the report's corner), pivot?} writes the source records behind that cell into a new sheet placed right after the PivotTable's, named from the cell's items (\"East\", \"East - Jan\", suffixed when taken), as one undo step → {sheet, name, title, rows, cells, skipped}; a leaf cell writes its records, a subtotal its group's and a grand total every record the filters pass, and the sheet holds at most 12,000,000 cells (a wide source writes fewer rows; `skipped` counts the records the cap left out)",
            has_doc,
            show_details
        ),
        cmd!(
            "pivot.getPivotData",
            "Generate GetPivotData",
            ["PivotTable Analyze", "PivotTable"],
            None,
            "{pivot?, on?: bool, cell?: \"B5\"} Excel's Options ▸ Generate GetPivotData (on by default): with `on`, clicking a value of the report while a formula is being typed inserts =GETPIVOTDATA(\"Sum of Sales\", $A$3, \"Region\", \"East\", ...) — the field/item pairs the report draws — instead of a plain reference; without `on` it reports {pivot, on, cell, formula?} (the formula a click on `cell`, default the report's corner, inserts). Kept on the PivotTable in our own document format; a .xlsx save/reopen brings back the default",
            has_doc,
            generate_get_pivot_data
        ),
        cmd!(
            "pivot.delete",
            "Delete PivotTable",
            ["PivotTable Analyze", "Actions"],
            None,
            "{pivot?} clears the report and removes the PivotTable",
            has_doc,
            delete
        ),
    ]
}

fn with(p: &Json, k: &str, v: Json) -> Json {
    let mut m = p.as_object().cloned().unwrap_or_default();
    m.insert(k.into(), v);
    Json::Object(m)
}

fn other(msg: impl Into<String>) -> EngineError {
    EngineError::Other(msg.into())
}

// ---------------------------------------------------------------- lookup

/// (sheet, index) of the pivot a call targets.
fn find_pivot(s: &Session, p: &Json) -> Result<(usize, usize)> {
    let d = s.doc()?;
    let active = d.wb.active_sheet;
    if let Some(v) = p.get("pivot").filter(|v| !v.is_null()) {
        let mut order: Vec<usize> = vec![active];
        order.extend((0..d.wb.sheets.len()).filter(|i| *i != active));
        for si in order {
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
        return Err(bad("pivot", format!("no PivotTable `{}`", v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))));
    }
    let sh = d.wb.active().ok_or(EngineError::NoDocument)?;
    let at = d.selection.active;
    if let Some(i) = sh.pivots.iter().position(|pt| pt.last_range.is_some_and(|r| r.contains(at))) {
        return Ok((active, i));
    }
    if !sh.pivots.is_empty() {
        return Ok((active, sh.pivots.len() - 1));
    }
    Err(EngineError::Disabled("pivot".into(), "select a cell in a PivotTable (or pass `pivot`)".into()))
}

fn area_param(cmd: &str, v: Option<&str>) -> Result<&'static str> {
    match v.map(|x| x.to_ascii_lowercase()) {
        Some(a) if a == "rows" || a == "row" => Ok("rows"),
        Some(a) if a == "columns" || a == "column" || a == "cols" => Ok("columns"),
        Some(a) if a == "values" || a == "value" || a == "data" => Ok("values"),
        Some(a) if a == "filters" || a == "filter" => Ok("filters"),
        Some(a) => Err(bad(cmd, format!("unknown area `{a}` (rows|columns|values|filters)"))),
        None => Err(bad(cmd, "missing `area` (rows|columns|values|filters)")),
    }
}

fn func_param(v: Option<&str>) -> Option<PivotFunc> {
    Some(match v?.to_ascii_lowercase().as_str() {
        "sum" => PivotFunc::Sum,
        "count" | "counta" => PivotFunc::Count,
        "average" | "avg" | "mean" => PivotFunc::Average,
        "max" => PivotFunc::Max,
        "min" => PivotFunc::Min,
        "product" => PivotFunc::Product,
        "countnumbers" | "countnums" | "count numbers" => PivotFunc::CountNumbers,
        "stddev" => PivotFunc::StdDev,
        "stddevp" => PivotFunc::StdDevP,
        "var" => PivotFunc::Var,
        "varp" => PivotFunc::VarP,
        _ => return None,
    })
}

fn func_name(f: PivotFunc) -> &'static str {
    match f {
        PivotFunc::Sum => "sum",
        PivotFunc::Count => "count",
        PivotFunc::Average => "average",
        PivotFunc::Max => "max",
        PivotFunc::Min => "min",
        PivotFunc::Product => "product",
        PivotFunc::CountNumbers => "countNumbers",
        PivotFunc::StdDev => "stdDev",
        PivotFunc::StdDevP => "stdDevP",
        PivotFunc::Var => "var",
        PivotFunc::VarP => "varP",
    }
}

fn show_as_param(v: &str) -> Option<PivotShowAs> {
    Some(match v.to_ascii_lowercase().as_str() {
        "normal" | "none" => PivotShowAs::Normal,
        "percentofgrandtotal" => PivotShowAs::PercentOfGrandTotal,
        "percentofcolumntotal" => PivotShowAs::PercentOfColumnTotal,
        "percentofrowtotal" => PivotShowAs::PercentOfRowTotal,
        "runningtotal" => PivotShowAs::RunningTotal,
        "rank" => PivotShowAs::Rank,
        _ => return None,
    })
}

fn show_as_name(v: PivotShowAs) -> &'static str {
    match v {
        PivotShowAs::Normal => "normal",
        PivotShowAs::PercentOfGrandTotal => "percentOfGrandTotal",
        PivotShowAs::PercentOfColumnTotal => "percentOfColumnTotal",
        PivotShowAs::PercentOfRowTotal => "percentOfRowTotal",
        PivotShowAs::RunningTotal => "runningTotal",
        PivotShowAs::Rank => "rank",
    }
}

fn source_kind_name(k: PivotSourceKind) -> &'static str {
    match k {
        PivotSourceKind::Range => "range",
        PivotSourceKind::DataModel => "dataModel",
    }
}

/// The error a command that would recompute or reshape a report GridCraft does not compute
/// returns, naming the reason the report is read-only.
fn read_only_error(pt: &PivotTable) -> EngineError {
    let why = match pt.read_only_text() {
        "" => "its source is one GridCraft does not query".to_string(),
        t => format!("it is {t}"),
    };
    other(format!("PivotTable \"{}\" is read-only: {why}. Its cached values are left as they are.", pt.name))
}

fn layout_name(l: PivotLayout) -> &'static str {
    match l {
        PivotLayout::Compact => "compact",
        PivotLayout::Outline => "outline",
        PivotLayout::Tabular => "tabular",
    }
}

fn group_name(g: PivotDateGroup) -> &'static str {
    match g {
        PivotDateGroup::None => "none",
        PivotDateGroup::Years => "years",
        PivotDateGroup::Quarters => "quarters",
        PivotDateGroup::Months => "months",
        PivotDateGroup::Days => "days",
    }
}

fn sort_name(o: PivotSort) -> &'static str {
    match o {
        PivotSort::Asc => "asc",
        PivotSort::Desc => "desc",
        PivotSort::None => "none",
    }
}

// ---------------------------------------------------------------- layout editing (pure)

fn in_axes(pt: &PivotTable, field: &str) -> bool {
    pt.rows.iter().chain(pt.columns.iter()).any(|f| f.source_col.eq_ignore_ascii_case(field))
}

/// Name of the Σ Values pseudo-field (where the value fields sit when there are two or more).
const VALUES_FIELD: &str = "Σ Values";

/// Whether `field` names Σ Values: `Σ Values`, or `Values` unless a source field has that name.
/// Drops a field's label/value filter rules: they only apply while it is on an axis.
fn drop_axis_rules(pt: &mut PivotTable, field: &str) {
    pt.label_filters.retain(|f| !f.source_col.eq_ignore_ascii_case(field));
    pt.value_filters.retain(|f| !f.source_col.eq_ignore_ascii_case(field));
    // A calculated item is an item of a field in Rows or Columns; it goes with the field.
    pt.calculated_items.retain(|c| !c.field.eq_ignore_ascii_case(field));
}

/// Drops the value filters whose measure no longer names a value field.
fn prune_value_filters(pt: &mut PivotTable) {
    let measures: Vec<String> = pt.values.iter().flat_map(|v| [v.name.clone(), v.source_col.clone()]).collect();
    pt.value_filters.retain(|f| measures.iter().any(|m| m.eq_ignore_ascii_case(&f.measure)));
}

fn is_values_field(src: &pv::Source, field: &str) -> bool {
    let f = field.trim();
    f == VALUES_FIELD || (f.eq_ignore_ascii_case("values") && src.col(f).is_none())
}

/// Keeps Σ Values in place after a field left (`inserted` false) or arrived at index `i` of
/// Rows (`rows`) or Columns.
fn shift_values(pt: &mut PivotTable, rows: bool, i: usize, inserted: bool) {
    if pt.values_on_rows != rows {
        return;
    }
    let Some(p) = pt.values_position else { return };
    // Σ Values stays just ahead of the field it preceded.
    let p = match inserted {
        true if i <= p => p + 1,
        false if i < p => p - 1,
        _ => p,
    };
    let n = if rows { pt.rows.len() } else { pt.columns.len() };
    pt.values_position = (p < n).then_some(p);
}

/// Takes `field` out of Rows and Columns (Σ Values keeps its place); returns its first entry.
fn take_from_axes(pt: &mut PivotTable, field: &str) -> Option<PivotField> {
    let mut first = None;
    for rows in [true, false] {
        loop {
            let list = if rows { &mut pt.rows } else { &mut pt.columns };
            let Some(i) = list.iter().position(|f| f.source_col.eq_ignore_ascii_case(field)) else { break };
            let f = list.remove(i);
            shift_values(pt, rows, i, false);
            first.get_or_insert(f);
        }
    }
    first
}

/// Inserts `f` into Rows (`rows`) or Columns at `position` (default: the end).
fn insert_axis_field(pt: &mut PivotTable, rows: bool, f: PivotField, position: Option<usize>) {
    let list = if rows { &mut pt.rows } else { &mut pt.columns };
    let at = position.unwrap_or(list.len()).min(list.len());
    list.insert(at, f);
    shift_values(pt, rows, at, true);
}

/// Moves Σ Values between Rows and Columns, or within Rows, to `position` (default: the end).
/// In Columns it always comes after the last column field: the column layout doesn't place it
/// anywhere else yet.
fn move_values_field(cmd: &str, pt: &mut PivotTable, from: &str, to: &str, position: Option<usize>) -> Result<Json> {
    let current = if pt.values_on_rows { "rows" } else { "columns" };
    if from != current {
        return Err(bad(cmd, format!("`{VALUES_FIELD}` isn't in the {from} area")));
    }
    if !matches!(to, "rows" | "columns") {
        return Err(other("The field you are moving cannot be placed in that PivotTable area."));
    }
    pt.values_on_rows = to == "rows";
    pt.values_position = position.filter(|p| pt.values_on_rows && *p < pt.rows.len());
    Ok(json!({"field": VALUES_FIELD, "to": to}))
}

/// The canonical source field name for `field`.
fn canonical(cmd: &str, src: &pv::Source, field: &str) -> Result<String> {
    src.col(field).and_then(|c| src.headers.get(c).cloned()).ok_or_else(|| bad(cmd, format!("no field `{field}` in the source data")))
}

/// The calculated field named `field`, if any.
fn calc_field<'a>(pt: &'a PivotTable, field: &str) -> Option<&'a PivotCalcField> {
    pt.calculated_fields.iter().find(|c| c.name.eq_ignore_ascii_case(field.trim()))
}

/// The canonical name of a source or calculated field.
fn canonical_any(cmd: &str, pt: &PivotTable, src: &pv::Source, field: &str) -> Result<String> {
    match calc_field(pt, field) {
        Some(c) if src.col(field).is_none() => Ok(c.name.clone()),
        _ => canonical(cmd, src, field),
    }
}

fn unique_value_name(pt: &PivotTable, base: &str, except: Option<usize>) -> String {
    let taken = |n: &str| pt.values.iter().enumerate().any(|(i, v)| Some(i) != except && v.name.eq_ignore_ascii_case(n));
    if !taken(base) {
        return base.to_string();
    }
    (2..10_000).map(|k| format!("{base}{k}")).find(|n| !taken(n)).unwrap_or_else(|| base.to_string())
}

fn auto_name(f: PivotFunc, field: &str) -> String {
    format!("{} of {field}", pv::func_caption(f))
}

/// Adds `field` to an area (moving it out of the other axis areas).
fn add_to_area(
    cmd: &str,
    pt: &mut PivotTable,
    src: &pv::Source,
    field: &str,
    area: &str,
    func: Option<PivotFunc>,
    name: Option<&str>,
    position: Option<usize>,
) -> Result<()> {
    let is_calc = src.col(field).is_none() && calc_field(pt, field).is_some();
    if is_calc && area != "values" {
        return Err(other("A calculated field can only be placed in the Values area."));
    }
    let field = canonical_any(cmd, pt, src, field)?;
    match area {
        "rows" | "columns" => {
            if pt.rows.len() + pt.columns.len() >= pv::MAX_AXIS_FIELDS && !in_axes(pt, &field) {
                return Err(other(format!("A PivotTable can have at most {} row and column fields.", pv::MAX_AXIS_FIELDS)));
            }
            let existing = take_from_axes(pt, &field).unwrap_or_else(|| PivotField { source_col: field.clone(), ..Default::default() });
            // A report filter that selects nothing in particular goes away; a selection stays
            // as the field's hidden items (a field is in one area at a time, so it is no longer
            // drawn as a report-filter row).
            pt.filters.retain(|f| !(f.source_col.eq_ignore_ascii_case(&field) && f.selected.is_none()));
            for f in pt.filters.iter_mut().filter(|f| f.source_col.eq_ignore_ascii_case(&field)) {
                f.area = PivotFilterArea::Axis;
            }
            insert_axis_field(pt, area == "rows", existing, position);
        }
        "values" => {
            if pt.values.len() >= pv::MAX_VALUE_FIELDS {
                return Err(other(format!("A PivotTable can have at most {} value fields.", pv::MAX_VALUE_FIELDS)));
            }
            let numeric = is_calc || src.col(&field).and_then(|c| src.numeric.get(c).copied()).unwrap_or(false);
            let f = func.unwrap_or(if numeric { PivotFunc::Sum } else { PivotFunc::Count });
            let base = match name {
                Some(n) if !n.trim().is_empty() => {
                    if src.col(n).is_some() {
                        return Err(other("PivotTable field name already exists."));
                    }
                    n.to_string()
                }
                _ => auto_name(f, &field),
            };
            let nm = unique_value_name(pt, &base, None);
            drop_axis_rules(pt, &field);
            let v = PivotValue { source_col: field, func: f, name: nm, show_as: PivotShowAs::Normal, number_format: None };
            let at = position.unwrap_or(pt.values.len()).min(pt.values.len());
            pt.values.insert(at, v);
        }
        _ => {
            take_from_axes(pt, &field);
            drop_axis_rules(pt, &field);
            let existing = pt.filters.iter().position(|f| f.source_col.eq_ignore_ascii_case(&field));
            let entry = match existing {
                Some(i) => pt.filters.remove(i),
                None => PivotFilter { source_col: field, selected: None, area: PivotFilterArea::Page },
            };
            let at = position.unwrap_or(pt.filters.len()).min(pt.filters.len());
            pt.filters.insert(at, entry);
        }
    }
    Ok(())
}

/// Removes `field` from an area; returns the source field it referred to.
fn remove_from_area(cmd: &str, pt: &mut PivotTable, field: &str, area: &str) -> Result<String> {
    match area {
        "rows" | "columns" => {
            let rows = area == "rows";
            let list = if rows { &mut pt.rows } else { &mut pt.columns };
            let i = list
                .iter()
                .position(|f| f.source_col.eq_ignore_ascii_case(field))
                .ok_or_else(|| bad(cmd, format!("`{field}` isn't in the {area} area")))?;
            let f = list.remove(i);
            shift_values(pt, rows, i, false);
            pt.filters.retain(|x| !x.source_col.eq_ignore_ascii_case(&f.source_col));
            if !in_axes(pt, &f.source_col) {
                drop_axis_rules(pt, &f.source_col);
            }
            Ok(f.source_col)
        }
        "values" => {
            let i = pt
                .values
                .iter()
                .position(|v| v.name.eq_ignore_ascii_case(field))
                .or_else(|| pt.values.iter().position(|v| v.source_col.eq_ignore_ascii_case(field)))
                .ok_or_else(|| bad(cmd, format!("`{field}` isn't in the values area")))?;
            let v = pt.values.remove(i);
            prune_value_filters(pt);
            Ok(v.source_col)
        }
        _ => {
            let i = pt
                .filters
                .iter()
                .position(|f| f.source_col.eq_ignore_ascii_case(field) && !in_axes(pt, &f.source_col))
                .ok_or_else(|| bad(cmd, format!("`{field}` isn't in the filters area")))?;
            Ok(pt.filters.remove(i).source_col)
        }
    }
}

// ---------------------------------------------------------------- shared update

/// Applies `f` to the targeted pivot and re-renders it, as one undo step.
fn update(s: &mut Session, p: &Json, f: impl FnOnce(&mut PivotTable, &pv::Source, &Workbook) -> Result<Json>) -> Result<Json> {
    let (si, pi) = find_pivot(s, p)?;
    // Every command here changes the report and has it recomputed from its source. A report
    // GridCraft does not compute has no source to read here: refusing keeps its cached values
    // intact instead of overwriting them from a range that isn't there.
    {
        let d = s.doc()?;
        if let Some(pt) = d.wb.sheet(si).and_then(|sh| sh.pivots.get(pi))
            && pt.is_read_only()
        {
            return Err(read_only_error(pt));
        }
    }
    let replace = bool_param(p, "replace").unwrap_or(false);
    edit(s, |cx| {
        let pt = cx.wb.sheet(si).and_then(|sh| sh.pivots.get(pi)).cloned().ok_or_else(|| other("There's no such PivotTable."))?;
        let src = pv::read_source(&cx.wb, &pt.source, si).map_err(other)?;
        let mut new = pt.clone();
        let mut out = f(&mut new, &src, &cx.wb)?;
        if let Some(slot) = cx.sheet_mut(si)?.pivots.get_mut(pi) {
            *slot = new;
        }
        let area = pv::refresh(&mut cx.wb, si, pi, replace).map_err(other)?;
        // A filter changed from the report itself is the same field selection a slicer shows, so
        // an already-inserted widget follows it (and the tiles cannot disagree with the cells).
        super::slicer::sync_slicers_from_pivot(&mut cx.wb, si, pi);
        cx.structural = true;
        if out.is_null() {
            out = json!({});
        }
        if let Some(m) = out.as_object_mut() {
            m.insert("range".into(), json!(area.a1()));
        }
        Ok(out)
    })
}

// ---------------------------------------------------------------- insert

/// The default source: the table at the selection, else the selected block / current region.
fn default_source(s: &Session) -> Result<String> {
    let d = s.doc()?;
    let sh = d.wb.active().ok_or(EngineError::NoDocument)?;
    if let Some(t) = sh.table_at(d.selection.active) {
        return Ok(t.name.clone());
    }
    let sel = d.selection.current();
    let r = if sel.is_single() {
        current_region(sh, d.selection.active)
    } else {
        let used = sh.used_range().unwrap_or(sel);
        sel.intersection(&used).unwrap_or(sel)
    };
    if r.is_single() && sh.value(r.start).is_empty() {
        return Err(other("The reference isn't valid. Select a cell in a range with labeled columns, or pass `source`."));
    }
    Ok(pv::range_text(&sh.name, r))
}

fn source_param(s: &Session, p: &Json) -> Result<String> {
    match str_param(p, "source").or_else(|| str_param(p, "range")) {
        Some(t) if !t.trim().is_empty() => {
            // Bare ranges are on the active sheet; store them with the sheet name.
            let d = s.doc()?;
            let t = t.trim().trim_start_matches('=');
            if !t.contains('!')
                && RangeRef::parse(t).is_some()
                && let Some(sh) = d.wb.active()
                && let Some(r) = RangeRef::parse(t)
            {
                return Ok(pv::range_text(&sh.name, r));
            }
            Ok(t.to_string())
        }
        _ => default_source(s),
    }
}

fn next_pivot_name(wb: &Workbook) -> String {
    (1..)
        .map(|n| format!("PivotTable{n}"))
        .find(|n| !wb.sheets.iter().any(|s| s.pivots.iter().any(|p| p.name.eq_ignore_ascii_case(n))))
        .unwrap_or_default()
}

fn next_pivot_id(wb: &Workbook) -> u32 {
    let m = wb.sheets.iter().flat_map(|s| s.pivots.iter().map(|p| p.id)).max().unwrap_or(0);
    wb.next_object_id().max(m.saturating_add(1))
}

fn field_list(v: Option<&Json>) -> Vec<Json> {
    match v {
        Some(Json::Array(a)) => a.iter().take(512).cloned().collect(),
        Some(Json::String(s)) => vec![Json::String(s.clone())],
        _ => vec![],
    }
}

fn insert_pivot(s: &mut Session, p: &Json) -> Result<Json> {
    if s.doc()?.wb.protected_structure && matches!(str_param(p, "destination"), None | Some("new")) {
        return Err(other("The workbook is protected and cannot be changed."));
    }
    let source = source_param(s, p)?;
    let active = s.doc()?.wb.active_sheet;
    // Validate the source now (header names, size).
    let src = pv::read_source(&s.doc()?.wb, &source, active).map_err(other)?;
    let name = match str_param(p, "name") {
        Some(n) if !n.trim().is_empty() => {
            let n = n.trim().to_string();
            if s.doc()?.wb.sheets.iter().any(|sh| sh.pivots.iter().any(|x| x.name.eq_ignore_ascii_case(&n))) {
                return Err(other("A PivotTable with that name already exists."));
            }
            n
        }
        _ => next_pivot_name(&s.doc()?.wb),
    };
    let dest = str_param(p, "destination").or_else(|| str_param(p, "location")).unwrap_or("new").trim().to_string();
    // Existing sheet destination: (sheet index, anchor).
    let target: Option<(usize, CellRef)> = if dest.eq_ignore_ascii_case("new") || dest.is_empty() {
        None
    } else {
        let d = s.doc()?;
        let (sh, body) = super::split_sheet(&dest);
        let si = match sh {
            Some(n) => d.wb.sheet_index(&n).ok_or_else(|| bad("insert.pivotTable", format!("no sheet named `{n}`")))?,
            None => active,
        };
        let at = RangeRef::parse(body).map(|r| r.start).ok_or_else(|| bad("insert.pivotTable", format!("not a cell: `{dest}`")))?;
        Some((si, at))
    };
    let replace = bool_param(p, "replace").unwrap_or(false);
    let rows = field_list(p.get("rows"));
    let columns = field_list(p.get("columns"));
    let values = field_list(p.get("values"));
    let filters = field_list(p.get("filters"));
    let mut new_sheet_at: Option<usize> = None;
    let res = edit(s, |cx| {
        let id = next_pivot_id(&cx.wb);
        let (si, anchor) = match target {
            Some(t) => t,
            None => {
                let at = cx.wb.active_sheet.min(cx.wb.sheets.len());
                let nm = cx.wb.next_sheet_name();
                cx.wb.sheets.insert(at, Arc::new(Sheet::new(nm)));
                for n in cx.wb.names.iter_mut() {
                    if let Some(sc) = n.scope.as_mut()
                        && *sc >= at
                    {
                        *sc += 1;
                    }
                }
                new_sheet_at = Some(at);
                (at, CellRef::new(2, 0))
            }
        };
        let mut pt = PivotTable { id, name: name.clone(), source: source.clone(), anchor, ..Default::default() };
        let add = |pt: &mut PivotTable, list: &[Json], area: &str| -> Result<()> {
            for f in list {
                match f {
                    Json::String(n) => add_to_area("insert.pivotTable", pt, &src, n, area, None, None, None)?,
                    Json::Object(_) => {
                        let n = str_param(f, "field").ok_or_else(|| bad("insert.pivotTable", "a field object needs `field`"))?;
                        let func = match str_param(f, "func") {
                            Some(fname) => {
                                Some(func_param(Some(fname)).ok_or_else(|| bad("insert.pivotTable", format!("unknown function `{fname}`")))?)
                            }
                            None => None,
                        };
                        add_to_area("insert.pivotTable", pt, &src, n, area, func, str_param(f, "name"), None)?;
                        let by = group_param(str_param(f, "group"));
                        if let Some(by) = by.filter(|b| *b != PivotDateGroup::None)
                            && matches!(area, "rows" | "columns")
                            && src.col(n).and_then(|c| src.numeric.get(c).copied()).unwrap_or(false)
                            && let Some(af) = pt.rows.iter_mut().chain(pt.columns.iter_mut()).find(|x| x.source_col.eq_ignore_ascii_case(n))
                        {
                            af.date_group = by;
                        }
                    }
                    _ => return Err(bad("insert.pivotTable", "fields are names or {field, func?, name?}")),
                }
            }
            Ok(())
        };
        add(&mut pt, &rows, "rows")?;
        add(&mut pt, &columns, "columns")?;
        add(&mut pt, &values, "values")?;
        add(&mut pt, &filters, "filters")?;
        let sh = cx.sheet_mut(si)?;
        sh.pivots.push(pt);
        let pi = sh.pivots.len() - 1;
        let area = pv::refresh(&mut cx.wb, si, pi, replace).map_err(other)?;
        cx.wb.active_sheet = si;
        *cx.sel = crate::Selection::at(anchor);
        cx.structural = true;
        let sheet_name = cx.wb.sheet(si).map(|s| s.name.clone()).unwrap_or_default();
        Ok(json!({
            "id": id,
            "name": name,
            "sheet": sheet_name,
            "anchor": anchor.a1(),
            "range": area.a1(),
            "source": source,
            "fields": src.headers,
        }))
    })?;
    if let Some(at) = new_sheet_at
        && let Ok(d) = s.doc_mut()
    {
        d.sheet_selections.insert(at.min(d.sheet_selections.len()), crate::Selection::default());
    }
    Ok(res)
}

fn summarize_table(s: &mut Session, p: &Json) -> Result<Json> {
    let d = s.doc()?;
    let name = match str_param(p, "table") {
        Some(n) => d.wb.table(n).and_then(|(si, ti)| d.wb.sheet(si).and_then(|sh| sh.tables.get(ti))).map(|t| t.name.clone()),
        None => d.wb.active().and_then(|sh| sh.table_at(d.selection.active)).map(|t| t.name.clone()),
    }
    .ok_or_else(|| EngineError::Disabled("table.summarizePivot".into(), "select a cell in a table".into()))?;
    let mut q = with(p, "source", json!(name));
    if let Some(m) = q.as_object_mut() {
        m.remove("table");
    }
    insert_pivot(s, &q)
}

// ---------------------------------------------------------------- queries

/// A row or column field: its area, sort, grouping and collapsed items.
fn axis_json(f: &PivotField) -> Json {
    json!({
        "field": f.source_col,
        "sort": sort_name(f.sort),
        "group": group_name(f.date_group),
        "numberGroup": f.number_group.map(|g| json!({"start": g.start, "end": g.end, "interval": g.interval})),
        "collapsed": f.collapsed_items,
    })
}

fn label_op_name(op: PivotLabelOp) -> &'static str {
    match op {
        PivotLabelOp::Equals => "equals",
        PivotLabelOp::NotEquals => "notEquals",
        PivotLabelOp::BeginsWith => "beginsWith",
        PivotLabelOp::EndsWith => "endsWith",
        PivotLabelOp::Contains => "contains",
        PivotLabelOp::DoesNotContain => "doesNotContain",
        PivotLabelOp::GreaterThan => "greaterThan",
        PivotLabelOp::GreaterThanOrEqual => "greaterThanOrEqual",
        PivotLabelOp::LessThan => "lessThan",
        PivotLabelOp::LessThanOrEqual => "lessThanOrEqual",
        PivotLabelOp::Between => "between",
        PivotLabelOp::NotBetween => "notBetween",
    }
}

fn value_op_name(op: PivotValueOp) -> &'static str {
    match op {
        PivotValueOp::Equals => "equals",
        PivotValueOp::NotEquals => "notEquals",
        PivotValueOp::GreaterThan => "greaterThan",
        PivotValueOp::GreaterThanOrEqual => "greaterThanOrEqual",
        PivotValueOp::LessThan => "lessThan",
        PivotValueOp::LessThanOrEqual => "lessThanOrEqual",
        PivotValueOp::Between => "between",
        PivotValueOp::NotBetween => "notBetween",
    }
}

fn pivot_json(wb: &Workbook, si: usize, pt: &PivotTable) -> Json {
    json!({
        "id": pt.id,
        "name": pt.name,
        "sheet": wb.sheet(si).map(|s| s.name.clone()).unwrap_or_default(),
        "anchor": pt.anchor.a1(),
        "range": pt.last_range.map(|r| r.a1()),
        "source": pt.source,
        "kind": source_kind_name(pt.source_kind),
        "readOnly": pt.is_read_only(),
        "readOnlyReason": pt.read_only_reason,
        "rows": pt.rows.iter().map(axis_json).collect::<Vec<_>>(),
        "columns": pt.columns.iter().map(axis_json).collect::<Vec<_>>(),
        "values": pt.values.iter().map(|v| json!({"field": v.source_col, "func": func_name(v.func), "name": v.name, "showAs": show_as_name(v.show_as), "numberFormat": v.number_format})).collect::<Vec<_>>(),
        "filters": pt.filters.iter().map(|f| json!({"field": f.source_col, "selected": f.selected, "report": !in_axes(pt, &f.source_col)})).collect::<Vec<_>>(),
        "layout": layout_name(pt.layout),
        "grandTotalsRows": pt.grand_totals_rows,
        "grandTotalsCols": pt.grand_totals_cols,
        "subtotals": if !pt.subtotals { "none" } else if pt.subtotals_top { "top" } else { "bottom" },
        "style": pt.style,
        "showHeaders": pt.show_headers,
        "calculatedFields": pt.calculated_fields.iter().map(|c| json!({"name": c.name, "formula": c.formula})).collect::<Vec<_>>(),
        "calculatedItems": calc_items_json(pt),
        "labelFilters": pt.label_filters.iter().map(|f| json!({"field": f.source_col, "op": label_op_name(f.op), "value": f.value, "value2": f.value2})).collect::<Vec<_>>(),
        "valueFilters": pt.value_filters.iter().map(|f| json!({"field": f.source_col, "measure": f.measure, "op": value_op_name(f.op), "value": f.value, "value2": f.value2})).collect::<Vec<_>>(),
        "valuesOnRows": pt.values_on_rows,
        "valuesPosition": pt.values_position,
    })
}

fn fields(s: &mut Session, p: &Json) -> Result<Json> {
    let (si, pi) = find_pivot(s, p)?;
    let d = s.doc()?;
    let pt = d.wb.sheet(si).and_then(|sh| sh.pivots.get(pi)).ok_or_else(|| other("There's no such PivotTable."))?;
    let mut out = pivot_json(&d.wb, si, pt);
    // A report GridCraft does not compute has no source to read: its fields are the ones the file
    // says it is built on, without record counts or samples. `kind`/`readOnlyReason` say why.
    if pt.is_read_only() {
        let list = read_only_fields(pt);
        let mut list = list;
        for c in &pt.calculated_fields {
            let areas: Vec<&str> = if pt.values.iter().any(|v| v.source_col.eq_ignore_ascii_case(&c.name)) { vec!["values"] } else { vec![] };
            list.push(
                json!({"name": c.name, "numeric": true, "date": false, "samples": [], "areas": areas, "calculated": true, "formula": c.formula}),
            );
        }
        if let Some(m) = out.as_object_mut() {
            m.insert("fields".into(), Json::Array(list));
        }
        return Ok(out);
    }
    let src = pv::read_source(&d.wb, &pt.source, si);
    let list: Vec<Json> = match &src {
        Ok(src) => src
            .headers
            .iter()
            .enumerate()
            .map(|(c, h)| {
                let mut areas = vec![];
                if pt.rows.iter().any(|f| f.source_col.eq_ignore_ascii_case(h)) {
                    areas.push("rows");
                }
                if pt.columns.iter().any(|f| f.source_col.eq_ignore_ascii_case(h)) {
                    areas.push("columns");
                }
                if pt.values.iter().any(|f| f.source_col.eq_ignore_ascii_case(h)) {
                    areas.push("values");
                }
                if pt.filters.iter().any(|f| f.source_col.eq_ignore_ascii_case(h)) && !in_axes(pt, h) {
                    areas.push("filters");
                }
                json!({
                    "name": h,
                    "numeric": src.numeric.get(c).copied().unwrap_or(false),
                    "date": src.dates.get(c).copied().unwrap_or(false),
                    "samples": pv::distinct_labels(&d.wb, src, c, crate::pivot::Grouping::default(), 5),
                    "areas": areas,
                })
            })
            .collect(),
        Err(_) => vec![],
    };
    let mut list = list;
    for c in &pt.calculated_fields {
        let areas: Vec<&str> = if pt.values.iter().any(|v| v.source_col.eq_ignore_ascii_case(&c.name)) { vec!["values"] } else { vec![] };
        list.push(json!({"name": c.name, "numeric": true, "date": false, "samples": [], "areas": areas, "calculated": true, "formula": c.formula}));
    }
    if let Some(m) = out.as_object_mut() {
        m.insert("fields".into(), Json::Array(list));
        if let Ok(src) = &src {
            m.insert("records".into(), json!(src.rows.len()));
        }
        if let Err(e) = src {
            m.insert("error".into(), json!(e));
        }
    }
    Ok(out)
}

/// The fields a report GridCraft does not compute names, with the area each is in. The source
/// itself is never read, so there are no samples and no record count.
fn read_only_fields(pt: &PivotTable) -> Vec<Json> {
    let mut names: Vec<&str> = Vec::new();
    fn push<'a>(n: &'a str, names: &mut Vec<&'a str>) {
        if !n.trim().is_empty() && !names.iter().any(|x| x.eq_ignore_ascii_case(n)) {
            names.push(n);
        }
    }
    for f in pt.rows.iter().chain(pt.columns.iter()) {
        push(&f.source_col, &mut names);
    }
    for v in &pt.values {
        push(&v.source_col, &mut names);
    }
    for f in &pt.filters {
        push(&f.source_col, &mut names);
    }
    names
        .into_iter()
        .map(|h| {
            let mut areas = vec![];
            if pt.rows.iter().any(|f| f.source_col.eq_ignore_ascii_case(h)) {
                areas.push("rows");
            }
            if pt.columns.iter().any(|f| f.source_col.eq_ignore_ascii_case(h)) {
                areas.push("columns");
            }
            if pt.values.iter().any(|f| f.source_col.eq_ignore_ascii_case(h)) {
                areas.push("values");
            }
            if pt.filters.iter().any(|f| f.source_col.eq_ignore_ascii_case(h)) && !in_axes(pt, h) {
                areas.push("filters");
            }
            json!({"name": h, "numeric": false, "date": false, "samples": [], "areas": areas})
        })
        .collect()
}

fn list(s: &mut Session, _: &Json) -> Result<Json> {
    let d = s.doc()?;
    let mut v = vec![];
    for (si, sh) in d.wb.sheets.iter().enumerate() {
        for pt in &sh.pivots {
            v.push(pivot_json(&d.wb, si, pt));
        }
    }
    Ok(Json::Array(v))
}

// ---------------------------------------------------------------- field commands

fn position_param(p: &Json) -> Option<usize> {
    u32_param(p, "position").map(|v| v as usize)
}

fn add_field(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "pivot.addField";
    let field = str_param(p, "field").ok_or_else(|| bad(C, "missing `field`"))?.to_string();
    let area = area_param(C, str_param(p, "area"))?;
    let func = match str_param(p, "func") {
        Some(f) => Some(func_param(Some(f)).ok_or_else(|| bad(C, format!("unknown function `{f}`")))?),
        None => None,
    };
    let name = str_param(p, "name").map(str::to_string);
    let pos = position_param(p);
    update(s, p, |pt, src, _| {
        add_to_area(C, pt, src, &field, area, func, name.as_deref(), pos)?;
        Ok(json!({"field": canonical_any(C, pt, src, &field)?, "area": area}))
    })
}

fn calculated_field(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "pivot.calculatedField";
    let Some(name) = str_param(p, "name").map(|n| n.trim().to_string()) else {
        // Menu-style call: the Insert Calculated Field dialog.
        let (si, pi) = find_pivot(s, p)?;
        let d = s.doc()?;
        let pt = d.wb.sheet(si).and_then(|sh| sh.pivots.get(pi)).ok_or_else(|| other("There's no such PivotTable."))?;
        let n = (1..).map(|k| format!("Field{k}")).find(|n| calc_field(pt, n).is_none()).unwrap_or_else(|| "Field1".into());
        let pivot = pt.name.clone();
        s.ui_requests.push(crate::UiRequest::Dialog("pivotCalculatedField".into(), json!({"pivot": pivot, "name": n, "formula": "= 0"})));
        return ok();
    };
    if name.is_empty() || name.chars().count() > 255 {
        return Err(bad(C, "`name` must be 1 to 255 characters"));
    }
    let delete = bool_param(p, "delete").unwrap_or(false);
    let formula = str_param(p, "formula").map(|f| {
        let f = f.trim();
        if f.starts_with('=') { f.to_string() } else { format!("={f}") }
    });
    let to_values = bool_param(p, "values").unwrap_or(true);
    if !delete && formula.is_none() {
        return Err(bad(C, "missing `formula`"));
    }
    update(s, p, |pt, src, _| {
        let existing = pt.calculated_fields.iter().position(|c| c.name.eq_ignore_ascii_case(&name));
        if delete {
            let i = existing.ok_or_else(|| bad(C, format!("no calculated field `{name}`")))?;
            let gone = pt.calculated_fields.remove(i);
            pt.values.retain(|v| !v.source_col.eq_ignore_ascii_case(&gone.name));
            return Ok(json!({"name": gone.name, "deleted": true}));
        }
        let formula = formula.clone().unwrap_or_default();
        if src.col(&name).is_some() {
            return Err(other("PivotTable field name already exists."));
        }
        let canon = match existing {
            Some(i) => {
                let c = pt.calculated_fields.get_mut(i).ok_or_else(|| other("There's no such calculated field."))?;
                c.formula = formula.clone();
                c.name.clone()
            }
            None => {
                if pt.calculated_fields.len() >= crate::pivot_calc::MAX_CALC_FIELDS {
                    return Err(other(format!("A PivotTable can have at most {} calculated fields.", crate::pivot_calc::MAX_CALC_FIELDS)));
                }
                pt.calculated_fields.push(PivotCalcField { name: name.clone(), formula: formula.clone() });
                name.clone()
            }
        };
        let set = crate::pivot_calc::CalcSet::new(pt, &src.headers);
        let i = set.index(&canon).ok_or_else(|| other("There's no such calculated field."))?;
        if let Some(e) = set.error(i) {
            return Err(bad(C, e.to_string()));
        }
        if set.is_cyclic(i) {
            return Err(bad(C, format!("`{canon}` refers to itself through its formula")));
        }
        if existing.is_none() && to_values && !pt.values.iter().any(|v| v.source_col.eq_ignore_ascii_case(&canon)) {
            add_to_area(C, pt, src, &canon, "values", Some(PivotFunc::Sum), None, None)?;
        }
        Ok(json!({
            "name": canon,
            "formula": formula,
            "calculatedFields": pt.calculated_fields.iter().map(|c| json!({"name": c.name, "formula": c.formula})).collect::<Vec<_>>(),
        }))
    })
}

/// The pivot's calculated items, for command results.
fn calc_items_json(pt: &PivotTable) -> Json {
    Json::Array(pt.calculated_items.iter().map(|c| json!({"field": c.field, "name": c.name, "formula": c.formula})).collect())
}

fn calculated_item(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "pivot.calculatedItem";
    let name = str_param(p, "name").map(|n| n.trim().to_string());
    let field = str_param(p, "field").map(|f| f.trim().to_string());
    let delete = bool_param(p, "delete").unwrap_or(false);
    let formula = str_param(p, "formula").map(|f| {
        let f = f.trim();
        if f.starts_with('=') { f.to_string() } else { format!("={f}") }
    });
    if !delete {
        let Some(n) = name.as_deref() else { return Err(bad(C, "missing `name`")) };
        if n.is_empty() || n.chars().count() > 255 {
            return Err(bad(C, "`name` must be 1 to 255 characters"));
        }
        if n.contains(['[', ']']) {
            return Err(bad(C, "`name` cannot contain `[` or `]` (Excel's calculated-item syntax uses them)"));
        }
        if formula.is_none() {
            return Err(bad(C, "missing `formula`"));
        }
        if field.is_none() {
            return Err(bad(C, "missing `field`"));
        }
    }
    update(s, p, move |pt, src, wb| {
        if delete {
            let name = name.as_deref().ok_or_else(|| bad(C, "missing `name`"))?;
            let i = pt
                .calculated_items
                .iter()
                .position(|c| c.name.eq_ignore_ascii_case(name) && field.as_deref().is_none_or(|f| c.field.eq_ignore_ascii_case(f)))
                .ok_or_else(|| bad(C, format!("no calculated item `{name}`")))?;
            let gone = pt.calculated_items.remove(i);
            return Ok(json!({"name": gone.name, "field": gone.field, "deleted": true, "calculatedItems": calc_items_json(pt)}));
        }
        let name = name.as_deref().unwrap_or_default();
        let asked = field.as_deref().unwrap_or_default();
        // A calculated item is an item of a field in Rows or Columns: Excel adds it to the field the
        // selected item belongs to.
        let f = pt
            .rows
            .iter()
            .chain(pt.columns.iter())
            .find(|f| f.source_col.eq_ignore_ascii_case(asked))
            .ok_or_else(|| other(format!("`{asked}` isn't in the Rows or Columns area; a calculated item belongs to a field on an axis.")))?;
        let canon = f.source_col.clone();
        let group = crate::pivot::Grouping::of(f);
        if !group.is_none() {
            return Err(other(format!("`{canon}` is grouped; a calculated item needs its items, so ungroup the field first.")));
        }
        let c = src.col(&canon).ok_or_else(|| other(format!("no field `{asked}` in the source data")))?;
        let labels = pv::distinct_labels(wb, src, c, group, 100_000);
        if labels.iter().any(|l| l.eq_ignore_ascii_case(name)) {
            return Err(other(format!("There's already an item named `{name}` in `{canon}`.")));
        }
        let formula = formula.clone().unwrap_or_default();
        let compiled = gridcraft_model::pivot_item::compile(&canon, &formula).map_err(|e| bad(C, e))?;
        for n in compiled.items() {
            if !labels.iter().any(|l| l.eq_ignore_ascii_case(n.trim())) {
                return Err(other(format!("`{n}` isn't an item of `{canon}`; a calculated item can only use its field's own items.")));
            }
        }
        let existing = pt.calculated_items.iter().position(|x| x.field.eq_ignore_ascii_case(&canon) && x.name.eq_ignore_ascii_case(name));
        let on_field = pt.calculated_items.iter().filter(|x| x.field.eq_ignore_ascii_case(&canon)).count();
        if existing.is_none() && on_field >= crate::pivot::MAX_CALC_ITEMS {
            return Err(other(format!("A PivotTable field can have at most {} calculated items.", crate::pivot::MAX_CALC_ITEMS)));
        }
        let stored = compiled.dialog_form(&canon);
        match existing {
            Some(i) => {
                if let Some(x) = pt.calculated_items.get_mut(i) {
                    x.formula = stored.clone();
                }
            }
            None => pt.calculated_items.push(PivotCalcItem { field: canon.clone(), name: name.to_string(), formula: stored.clone() }),
        }
        Ok(json!({"name": name, "field": canon, "formula": stored, "calculatedItems": calc_items_json(pt)}))
    })
}

fn remove_field(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "pivot.removeField";
    let field = str_param(p, "field").ok_or_else(|| bad(C, "missing `field`"))?.to_string();
    let area = match str_param(p, "area") {
        Some(a) => Some(area_param(C, Some(a))?),
        None => None,
    };
    update(s, p, |pt, _, _| {
        match area {
            Some(a) => {
                remove_from_area(C, pt, &field, a)?;
            }
            None => {
                let mut any = false;
                for a in ["rows", "columns", "values", "filters"] {
                    any |= remove_from_area(C, pt, &field, a).is_ok();
                }
                if !any {
                    return Err(bad(C, format!("`{field}` isn't in the PivotTable")));
                }
            }
        }
        Ok(json!({}))
    })
}

fn move_field(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "pivot.moveField";
    let field = str_param(p, "field").ok_or_else(|| bad(C, "missing `field`"))?.to_string();
    let from = area_param(C, str_param(p, "from"))?;
    let to = area_param(C, str_param(p, "to"))?;
    let pos = position_param(p);
    update(s, p, |pt, src, _| {
        if is_values_field(src, &field) {
            return move_values_field(C, pt, from, to, pos);
        }
        if from == to {
            // Reorder within the area.
            match from {
                "rows" | "columns" => {
                    let rows = from == "rows";
                    let list = if rows { &mut pt.rows } else { &mut pt.columns };
                    let i = list
                        .iter()
                        .position(|f| f.source_col.eq_ignore_ascii_case(&field))
                        .ok_or_else(|| bad(C, format!("`{field}` isn't in the {from} area")))?;
                    let f = list.remove(i);
                    shift_values(pt, rows, i, false);
                    insert_axis_field(pt, rows, f, pos);
                }
                "values" => {
                    let i = pt
                        .values
                        .iter()
                        .position(|v| v.name.eq_ignore_ascii_case(&field))
                        .or_else(|| pt.values.iter().position(|v| v.source_col.eq_ignore_ascii_case(&field)))
                        .ok_or_else(|| bad(C, format!("`{field}` isn't in the values area")))?;
                    let v = pt.values.remove(i);
                    let at = pos.unwrap_or(pt.values.len()).min(pt.values.len());
                    pt.values.insert(at, v);
                }
                _ => {
                    let i = pt
                        .filters
                        .iter()
                        .position(|f| f.source_col.eq_ignore_ascii_case(&field))
                        .ok_or_else(|| bad(C, format!("`{field}` isn't in the filters area")))?;
                    let f = pt.filters.remove(i);
                    let at = pos.unwrap_or(pt.filters.len()).min(pt.filters.len());
                    pt.filters.insert(at, f);
                }
            }
            return Ok(json!({}));
        }
        // Keep axis settings (sort, grouping, calculated items) when moving between rows and
        // columns: the field leaves every axis for a moment, which drops its axis-only rules.
        let kept = pt.rows.iter().chain(pt.columns.iter()).find(|f| f.source_col.eq_ignore_ascii_case(&field)).cloned();
        let hidden = pt.filters.iter().find(|f| f.source_col.eq_ignore_ascii_case(&field)).cloned();
        let calcs: Vec<PivotCalcItem> = pt.calculated_items.iter().filter(|c| c.field.eq_ignore_ascii_case(&field)).cloned().collect();
        let src_field = remove_from_area(C, pt, &field, from)?;
        add_to_area(C, pt, src, &src_field, to, None, None, pos)?;
        if matches!(to, "rows" | "columns") {
            let list = if to == "rows" { &mut pt.rows } else { &mut pt.columns };
            if let (Some(k), Some(slot)) = (kept, list.iter_mut().find(|f| f.source_col.eq_ignore_ascii_case(&src_field))) {
                *slot = k;
            }
            for c in calcs {
                if !pt.calculated_items.iter().any(|x| x.field.eq_ignore_ascii_case(&c.field) && x.name.eq_ignore_ascii_case(&c.name)) {
                    pt.calculated_items.push(c);
                }
            }
            if let Some(h) = hidden.filter(|h| h.selected.is_some())
                && !pt.filters.iter().any(|f| f.source_col.eq_ignore_ascii_case(&src_field))
            {
                pt.filters.push(h);
            }
        }
        Ok(json!({"field": src_field, "to": to}))
    })
}

fn value_settings(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "pivot.valueSettings";
    let field = str_param(p, "field").map(str::to_string);
    let index = u32_param(p, "index").map(|v| v as usize);
    let func = match str_param(p, "func") {
        Some(f) => Some(func_param(Some(f)).ok_or_else(|| bad(C, format!("unknown function `{f}`")))?),
        None => None,
    };
    let show = match str_param(p, "showAs") {
        Some(v) => Some(show_as_param(v).ok_or_else(|| bad(C, format!("unknown showAs `{v}`")))?),
        None => None,
    };
    let name = str_param(p, "name").map(str::to_string);
    let fmt = p.get("numberFormat").cloned();
    update(s, p, |pt, src, _| {
        let i = match (index, &field) {
            (Some(i), _) => i,
            (None, Some(f)) => pt
                .values
                .iter()
                .position(|v| v.name.eq_ignore_ascii_case(f))
                .or_else(|| pt.values.iter().position(|v| v.source_col.eq_ignore_ascii_case(f)))
                .ok_or_else(|| bad(C, format!("`{f}` isn't in the values area")))?,
            (None, None) => {
                if pt.values.len() == 1 {
                    0
                } else {
                    return Err(bad(C, "pass `field` or `index`"));
                }
            }
        };
        let cur = pt.values.get(i).cloned().ok_or_else(|| bad(C, "no value field at that index"))?;
        let mut v = cur.clone();
        if let Some(f) = func {
            v.func = f;
            // An automatic caption follows the function.
            let auto = auto_name(cur.func, &cur.source_col);
            if cur.name.eq_ignore_ascii_case(&auto) || cur.name.strip_prefix(auto.as_str()).is_some_and(|r| r.chars().all(|c| c.is_ascii_digit())) {
                v.name = unique_value_name(pt, &auto_name(f, &cur.source_col), Some(i));
            }
        }
        if let Some(n) = &name {
            let n = n.trim();
            if n.is_empty() {
                return Err(other("The name isn't valid."));
            }
            if src.col(n).is_some() {
                return Err(other("PivotTable field name already exists."));
            }
            if pt.values.iter().enumerate().any(|(j, x)| j != i && x.name.eq_ignore_ascii_case(n)) {
                return Err(other("PivotTable field name already exists."));
            }
            v.name = n.to_string();
        }
        if let Some(sa) = show {
            v.show_as = sa;
        }
        match &fmt {
            Some(Json::String(f)) if !f.is_empty() && f.len() <= 255 => v.number_format = Some(f.clone()),
            Some(Json::Null) => v.number_format = None,
            Some(Json::String(f)) if f.is_empty() => v.number_format = None,
            Some(_) => return Err(bad(C, "`numberFormat` is a format code or null")),
            None => {}
        }
        let name = v.name.clone();
        if let Some(slot) = pt.values.get_mut(i) {
            *slot = v;
        }
        Ok(json!({"name": name}))
    })
}

fn filter(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "pivot.filter";
    let field = str_param(p, "field").ok_or_else(|| bad(C, "missing `field`"))?.to_string();
    let selected: Option<Vec<String>> = match p.get("selected") {
        None | Some(Json::Null) => None,
        Some(Json::Array(a)) => {
            if a.len() > 100_000 {
                return Err(bad(C, "too many items"));
            }
            let v: Vec<String> = a
                .iter()
                .map(|x| match x {
                    Json::String(s) => s.clone(),
                    Json::Null => "(blank)".into(),
                    other => other.to_string(),
                })
                .collect();
            if v.is_empty() {
                return Err(other("You must select at least one item."));
            }
            Some(v)
        }
        Some(Json::String(s)) => Some(vec![s.clone()]),
        Some(_) => return Err(bad(C, "`selected` is a list of item labels or null")),
    };
    update(s, p, |pt, src, _| {
        let field = canonical(C, src, &field)?;
        pt.label_filters.retain(|f| !f.source_col.eq_ignore_ascii_case(&field));
        pt.value_filters.retain(|f| !f.source_col.eq_ignore_ascii_case(&field));
        let i = pt.filters.iter().position(|f| f.source_col.eq_ignore_ascii_case(&field));
        match (i, selected) {
            (Some(i), None) => {
                if in_axes(pt, &field) {
                    pt.filters.remove(i);
                } else if let Some(f) = pt.filters.get_mut(i) {
                    f.selected = None;
                }
            }
            (Some(i), Some(sel)) => {
                if let Some(f) = pt.filters.get_mut(i) {
                    f.selected = Some(sel);
                }
            }
            (None, Some(sel)) => pt.filters.push(PivotFilter { source_col: field.clone(), selected: Some(sel), area: PivotFilterArea::Auto }),
            (None, None) => {}
        }
        Ok(json!({"field": field}))
    })
}

fn label_op(v: &str) -> Option<PivotLabelOp> {
    Some(match v.to_ascii_lowercase().as_str() {
        "equals" | "eq" | "=" => PivotLabelOp::Equals,
        "notequals" | "ne" | "<>" => PivotLabelOp::NotEquals,
        "beginswith" | "startswith" => PivotLabelOp::BeginsWith,
        "endswith" => PivotLabelOp::EndsWith,
        "contains" => PivotLabelOp::Contains,
        "doesnotcontain" | "notcontains" => PivotLabelOp::DoesNotContain,
        "greaterthan" | ">" => PivotLabelOp::GreaterThan,
        "greaterthanorequal" | ">=" => PivotLabelOp::GreaterThanOrEqual,
        "lessthan" | "<" => PivotLabelOp::LessThan,
        "lessthanorequal" | "<=" => PivotLabelOp::LessThanOrEqual,
        "between" => PivotLabelOp::Between,
        "notbetween" => PivotLabelOp::NotBetween,
        _ => return None,
    })
}

fn value_op(v: &str) -> Option<PivotValueOp> {
    Some(match v.to_ascii_lowercase().as_str() {
        "equals" | "eq" | "=" => PivotValueOp::Equals,
        "notequals" | "ne" | "<>" => PivotValueOp::NotEquals,
        "greaterthan" | ">" => PivotValueOp::GreaterThan,
        "greaterthanorequal" | ">=" => PivotValueOp::GreaterThanOrEqual,
        "lessthan" | "<" => PivotValueOp::LessThan,
        "lessthanorequal" | "<=" => PivotValueOp::LessThanOrEqual,
        "between" => PivotValueOp::Between,
        "notbetween" => PivotValueOp::NotBetween,
        _ => return None,
    })
}

/// Stores `selected` as the field's item filter (hidden items on its axis).
fn set_item_filter(pt: &mut PivotTable, field: &str, selected: Vec<String>) {
    match pt.filters.iter_mut().find(|f| f.source_col.eq_ignore_ascii_case(field)) {
        Some(f) => {
            f.selected = Some(selected);
            f.area = PivotFilterArea::Axis;
        }
        None => pt.filters.push(PivotFilter { source_col: field.to_string(), selected: Some(selected), area: PivotFilterArea::Axis }),
    }
}

fn clear_item_filter(pt: &mut PivotTable, field: &str) {
    if let Some(f) = pt.filters.iter_mut().find(|f| f.source_col.eq_ignore_ascii_case(field))
        && f.area == PivotFilterArea::Axis
    {
        f.selected = None;
    }
}

fn filter_by_label(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "pivot.filterByLabel";
    let field = str_param(p, "field").ok_or_else(|| bad(C, "missing `field`"))?.to_string();
    let clear = bool_param(p, "clear").unwrap_or(false) || p.get("op").is_some_and(Json::is_null);
    let raw = str_param(p, "op").unwrap_or("equals");
    let op = if clear { None } else { Some(label_op(raw).ok_or_else(|| bad(C, format!("unknown operator `{raw}`")))?) };
    let value = str_param(p, "value").unwrap_or_default().to_string();
    let value2 = str_param(p, "value2").map(str::to_string);
    if matches!(op, Some(PivotLabelOp::Between | PivotLabelOp::NotBetween)) && value2.is_none() {
        return Err(bad(C, "`between`/`notBetween` need `value2`"));
    }
    update(s, p, move |pt, src, wb| {
        let field = canonical(C, src, &field)?;
        axis_field(C, pt, &field)?.collapsed_items.clear();
        pt.value_filters.retain(|f| !f.source_col.eq_ignore_ascii_case(&field));
        pt.label_filters.retain(|f| !f.source_col.eq_ignore_ascii_case(&field));
        let Some(op) = op else {
            clear_item_filter(pt, &field);
            return Ok(json!({"field": field, "items": Json::Null}));
        };
        let rule = PivotLabelFilter { source_col: field.clone(), op, value, value2 };
        let c = src.col(&field).ok_or_else(|| bad(C, format!("no field `{field}` in the source data")))?;
        let group = pt
            .rows
            .iter()
            .chain(pt.columns.iter())
            .find(|f| f.source_col.eq_ignore_ascii_case(&field))
            .map(crate::pivot::Grouping::of)
            .unwrap_or_default();
        let kept: Vec<String> = pv::distinct_labels(wb, src, c, group, 100_000).into_iter().filter(|l| rule.passes(l)).collect();
        let n = kept.len();
        set_item_filter(pt, &field, kept);
        pt.label_filters.push(rule);
        Ok(json!({"field": field, "items": n}))
    })
}

fn filter_by_value(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "pivot.filterByValue";
    let field = str_param(p, "field").ok_or_else(|| bad(C, "missing `field`"))?.to_string();
    let clear = bool_param(p, "clear").unwrap_or(false) || p.get("op").is_some_and(Json::is_null);
    let raw = str_param(p, "op").unwrap_or("greaterThan");
    let op = if clear { None } else { Some(value_op(raw).ok_or_else(|| bad(C, format!("unknown operator `{raw}`")))?) };
    if !clear && f64_param(p, "value").is_none() {
        return Err(bad(C, "missing `value`"));
    }
    let value = f64_param(p, "value").unwrap_or(0.0);
    let value2 = f64_param(p, "value2");
    if matches!(op, Some(PivotValueOp::Between | PivotValueOp::NotBetween)) && value2.is_none() {
        return Err(bad(C, "`between`/`notBetween` need `value2`"));
    }
    let measure = str_param(p, "measure").map(str::to_string);
    update(s, p, move |pt, src, wb| {
        let field = canonical(C, src, &field)?;
        axis_field(C, pt, &field)?.collapsed_items.clear();
        pt.label_filters.retain(|f| !f.source_col.eq_ignore_ascii_case(&field));
        pt.value_filters.retain(|f| !f.source_col.eq_ignore_ascii_case(&field));
        let Some(op) = op else {
            clear_item_filter(pt, &field);
            return Ok(json!({"field": field, "items": Json::Null}));
        };
        let measure = match measure {
            Some(m) => m,
            None => pt.values.first().map(|v| v.name.clone()).ok_or_else(|| other("Add a value field before filtering by value."))?,
        };
        let rule = PivotValueFilter { source_col: field.clone(), measure, op, value, value2 };
        // The new filter replaces this field's own selection; the other fields still narrow the
        // records it aggregates over.
        clear_item_filter(pt, &field);
        let kept = pv::value_filter_kept(wb, src, pt, &rule).map_err(other)?;
        let n = kept.len();
        set_item_filter(pt, &field, kept);
        pt.value_filters.push(rule);
        Ok(json!({"field": field, "items": n}))
    })
}

fn axis_field<'a>(cmd: &str, pt: &'a mut PivotTable, field: &str) -> Result<&'a mut PivotField> {
    pt.rows
        .iter_mut()
        .chain(pt.columns.iter_mut())
        .find(|f| f.source_col.eq_ignore_ascii_case(field))
        .ok_or_else(|| bad(cmd, format!("`{field}` isn't in the rows or columns area")))
}

fn sort(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "pivot.sort";
    let field = str_param(p, "field").ok_or_else(|| bad(C, "missing `field`"))?.to_string();
    let order = match str_param(p, "order").unwrap_or("asc").to_ascii_lowercase().as_str() {
        "asc" | "ascending" | "a-z" => PivotSort::Asc,
        "desc" | "descending" | "z-a" => PivotSort::Desc,
        "none" | "manual" | "source" => PivotSort::None,
        o => return Err(bad(C, format!("unknown order `{o}` (asc|desc|none)"))),
    };
    update(s, p, |pt, _, _| {
        axis_field(C, pt, &field)?.sort = order;
        Ok(json!({}))
    })
}

fn group_param(v: Option<&str>) -> Option<PivotDateGroup> {
    Some(match v?.to_ascii_lowercase().as_str() {
        "years" | "year" => PivotDateGroup::Years,
        "quarters" | "quarter" => PivotDateGroup::Quarters,
        "months" | "month" => PivotDateGroup::Months,
        "days" | "day" => PivotDateGroup::Days,
        "none" | "ungroup" => PivotDateGroup::None,
        _ => return None,
    })
}

fn group(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "pivot.group";
    let field = str_param(p, "field").ok_or_else(|| bad(C, "missing `field`"))?.to_string();
    // A numeric range is asked for by any of its three numbers; it replaces a date grouping.
    let numeric = ["start", "end", "interval"].iter().any(|k| p.get(*k).is_some_and(|v| !v.is_null()));
    let raw = match str_param(p, "by") {
        Some(b) => b,
        None if numeric => "none",
        None => "months",
    };
    let by = group_param(Some(raw)).ok_or_else(|| bad(C, format!("unknown grouping `{raw}` (years|quarters|months|days|none)")))?;
    if numeric && by != PivotDateGroup::None {
        return Err(bad(C, "give either `by` (a date grouping) or a numeric range (`start`/`end`/`interval`)"));
    }
    if let Some(interval) = f64_param(p, "interval")
        && !(interval.is_finite() && interval > 0.0)
    {
        return Err(bad(C, "`interval` must be a number past zero"));
    }
    update(s, p, |pt, src, _| {
        let c = src.col(&field).ok_or_else(|| bad(C, format!("no field `{field}` in the source data")))?;
        if !src.numeric.get(c).copied().unwrap_or(false) {
            return Err(other("Cannot group that selection."));
        }
        let f = axis_field(C, pt, &field)?;
        let grouped_field = f.source_col.clone();
        f.date_group = PivotDateGroup::None;
        f.number_group = None;
        if numeric {
            // Starting at/Ending at default to the field's own extent, floored/ceiled to whole
            // numbers the way Excel's dialog fills them in.
            let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
            for row in &src.rows {
                if let Some(Value::Number(n)) = row.get(c).map(Value::scalar)
                    && n.is_finite()
                {
                    lo = lo.min(n);
                    hi = hi.max(n);
                }
            }
            let (default_start, default_end) = if lo.is_finite() && hi.is_finite() { (lo.floor(), hi.ceil()) } else { (0.0, 0.0) };
            // Excel anchors a number group at the data's own extent and re-anchors it there when
            // the pivot is refreshed; a start below the minimum (or an end above it) only asks for
            // empty leading/trailing buckets Excel drops. Clamp, and say so.
            let asked_start = f64_param(p, "start");
            let asked_end = f64_param(p, "end");
            let start = asked_start.unwrap_or(default_start).max(default_start);
            let end = asked_end.unwrap_or(default_end).min(default_end);
            let clamped = asked_start.is_some_and(|s| s < default_start) || asked_end.is_some_and(|e| e > default_end);
            if start > end {
                return Err(other(format!("The grouping range does not include the field's values ({default_start}..{default_end}).")));
            }
            let interval = f64_param(p, "interval").ok_or_else(|| bad(C, "missing `interval` for a numeric grouping"))?;
            let g = PivotNumberGroup { start, end, interval };
            if !g.is_valid() {
                return Err(other(format!(
                    "The grouping range is not usable: `start` must be at most `end`, `interval` past zero, both finite, and at most {} groups.",
                    PivotNumberGroup::MAX_GROUPS
                )));
            }
            f.number_group = Some(g);
            f.collapsed_items.clear();
            // A grouped field's items are ranges, so its calculated items no longer apply; Excel
            // drops them when the field is grouped.
            pt.calculated_items.retain(|c| !c.field.eq_ignore_ascii_case(&grouped_field));
            return Ok(json!({"field": field, "numberGroup": {"start": start, "end": end, "interval": interval}, "clamped": clamped}));
        } else {
            f.date_group = by;
        }
        f.collapsed_items.clear();
        if f.date_group != PivotDateGroup::None {
            pt.calculated_items.retain(|c| !c.field.eq_ignore_ascii_case(&grouped_field));
        }
        Ok(json!({}))
    })
}

fn collapse(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "pivot.collapse";
    let field = str_param(p, "field").ok_or_else(|| bad(C, "missing `field`"))?.to_string();
    let on = bool_param(p, "collapse").unwrap_or(true);
    let items: Option<Vec<String>> =
        p.get("items").and_then(Json::as_array).map(|a| a.iter().take(100_000).filter_map(|x| x.as_str().map(str::to_string)).collect());
    update(s, p, |pt, src, wb| {
        let all = match &items {
            Some(v) => v.clone(),
            None => {
                let f = pt.rows.iter().chain(pt.columns.iter()).find(|f| f.source_col.eq_ignore_ascii_case(&field));
                let g = f.map(crate::pivot::Grouping::of).unwrap_or_default();
                match src.col(&field) {
                    Some(c) => pv::distinct_labels(wb, src, c, g, 100_000),
                    None => vec![],
                }
            }
        };
        let f = axis_field(C, pt, &field)?;
        if on {
            for it in all {
                if !f.collapsed_items.iter().any(|x| x.eq_ignore_ascii_case(&it)) {
                    f.collapsed_items.push(it);
                }
            }
        } else if items.is_none() {
            f.collapsed_items.clear();
        } else {
            f.collapsed_items.retain(|x| !all.iter().any(|a| a.eq_ignore_ascii_case(x)));
        }
        Ok(json!({"collapsed": f.collapsed_items}))
    })
}

fn layout(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "pivot.layout";
    let lay = match str_param(p, "layout").map(|x| x.to_ascii_lowercase()) {
        Some(l) if l == "compact" => Some(PivotLayout::Compact),
        Some(l) if l == "outline" => Some(PivotLayout::Outline),
        Some(l) if l == "tabular" => Some(PivotLayout::Tabular),
        Some(l) => return Err(bad(C, format!("unknown layout `{l}` (compact|outline|tabular)"))),
        None => None,
    };
    let subtotals: Option<(bool, Option<bool>)> = match p.get("subtotals") {
        Some(Json::Bool(b)) => Some((*b, None)),
        Some(Json::String(m)) => match m.to_ascii_lowercase().as_str() {
            "top" => Some((true, Some(true))),
            "bottom" => Some((true, Some(false))),
            "none" | "off" => Some((false, None)),
            o => return Err(bad(C, format!("unknown subtotals `{o}` (top|bottom|none)"))),
        },
        _ => None,
    };
    let style = str_param(p, "style").map(str::to_string);
    if style.as_ref().is_some_and(|s| s.len() > 64) {
        return Err(bad(C, "style name too long"));
    }
    let gr = bool_param(p, "grandTotalsRows");
    let gc = bool_param(p, "grandTotalsCols");
    let hd = bool_param(p, "showHeaders");
    update(s, p, |pt, _, _| {
        if let Some(l) = lay {
            pt.layout = l;
            if l == PivotLayout::Tabular {
                pt.subtotals_top = false;
            }
        }
        if let Some((on, top)) = subtotals {
            pt.subtotals = on;
            if let Some(t) = top {
                pt.subtotals_top = t;
            }
        }
        if let Some(st) = &style {
            pt.style = st.clone();
        }
        if let Some(v) = gr {
            pt.grand_totals_rows = v;
        }
        if let Some(v) = gc {
            pt.grand_totals_cols = v;
        }
        if let Some(v) = hd {
            pt.show_headers = v;
        }
        Ok(json!({"layout": layout_name(pt.layout)}))
    })
}

fn grand_totals(s: &mut Session, p: &Json) -> Result<Json> {
    let (r, c) = match str_param(p, "mode").map(|m| m.to_ascii_lowercase()) {
        Some(m) if m == "off" => (Some(false), Some(false)),
        Some(m) if m == "on" => (Some(true), Some(true)),
        Some(m) if m == "rows" => (Some(true), Some(false)),
        Some(m) if m == "columns" || m == "cols" => (Some(false), Some(true)),
        Some(m) => return Err(bad("pivot.grandTotals", format!("unknown mode `{m}` (off|on|rows|columns)"))),
        None => (bool_param(p, "rows"), bool_param(p, "cols")),
    };
    let mut q = p.as_object().cloned().unwrap_or_default();
    if let Some(v) = r {
        q.insert("grandTotalsRows".into(), json!(v));
    }
    if let Some(v) = c {
        q.insert("grandTotalsCols".into(), json!(v));
    }
    layout(s, &Json::Object(q))
}

fn refresh(s: &mut Session, p: &Json) -> Result<Json> {
    update(s, p, |_, src, _| Ok(json!({"records": src.rows.len()})))
}

fn refresh_all(s: &mut Session, p: &Json) -> Result<Json> {
    let replace = bool_param(p, "replace").unwrap_or(false);
    edit(s, |cx| {
        let mut n = 0;
        let mut errors = vec![];
        let mut skipped = vec![];
        for si in 0..cx.wb.sheets.len() {
            let count = cx.wb.sheet(si).map(|sh| sh.pivots.len()).unwrap_or(0);
            for pi in 0..count {
                let pivot = cx.wb.sheet(si).and_then(|sh| sh.pivots.get(pi)).cloned();
                let Some(pt) = pivot else { continue };
                let name = pt.name.clone();
                // A report GridCraft does not compute has no source to read: it is left as it is,
                // and said so rather than counted as refreshed.
                if pt.is_read_only() {
                    skipped.push(json!({"pivot": name, "reason": pt.read_only_text()}));
                    continue;
                }
                match pv::refresh(&mut cx.wb, si, pi, replace) {
                    Ok(_) => n += 1,
                    Err(e) => errors.push(json!({"pivot": name, "error": e})),
                }
            }
        }
        cx.structural = n > 0;
        Ok(json!({"refreshed": n, "errors": errors, "skipped": skipped}))
    })
}

/// Drops the calculated items whose field left the source or whose formula names an item the
/// source no longer has (Excel removes them with the field).
fn prune_calc_items(wb: &Workbook, src: &pv::Source, pt: &mut PivotTable) {
    let mut labels: Vec<(String, Vec<String>)> = Vec::new();
    let mut keep: Vec<PivotCalcItem> = Vec::new();
    for c in std::mem::take(&mut pt.calculated_items) {
        let Some(col) = src.col(&c.field) else { continue };
        let list = match labels.iter().find(|(f, _)| f.eq_ignore_ascii_case(&c.field)) {
            Some((_, l)) => l.clone(),
            None => {
                let l = pv::distinct_labels(wb, src, col, crate::pivot::Grouping::default(), 100_000);
                labels.push((c.field.clone(), l.clone()));
                l
            }
        };
        let Ok(compiled) = gridcraft_model::pivot_item::compile(&c.field, &c.formula) else { continue };
        if compiled.items().iter().all(|n| list.iter().any(|l| l.eq_ignore_ascii_case(n.trim()))) {
            keep.push(c);
        }
    }
    pt.calculated_items = keep;
}

fn change_source(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "pivot.changeSource";
    if str_param(p, "source").or_else(|| str_param(p, "range")).is_none() {
        return Err(bad(C, "missing `source`"));
    }
    let source = source_param(s, p)?;
    let (si, _) = find_pivot(s, p)?;
    let src = pv::read_source(&s.doc()?.wb, &source, si).map_err(other)?;
    update(s, p, move |pt, _, wb| {
        pt.source = source.clone();
        let mut dropped: Vec<String> = vec![];
        let keep = |name: &str, dropped: &mut Vec<String>| {
            let ok = src.col(name).is_some();
            if !ok {
                dropped.push(name.to_string());
            }
            ok
        };
        // Axis fields leave one by one so Σ Values keeps its place among the rest.
        for rows in [true, false] {
            let mut i = 0;
            loop {
                let list = if rows { &mut pt.rows } else { &mut pt.columns };
                let Some(f) = list.get(i) else { break };
                if keep(&f.source_col, &mut dropped) {
                    i += 1;
                } else {
                    list.remove(i);
                    shift_values(pt, rows, i, false);
                }
            }
        }
        pt.values.retain(|f| keep(&f.source_col, &mut dropped));
        pt.filters.retain(|f| keep(&f.source_col, &mut dropped));
        pt.label_filters.retain(|f| src.col(&f.source_col).is_some());
        prune_value_filters(pt);
        prune_calc_items(wb, &src, pt);
        Ok(json!({"source": source, "dropped": dropped}))
    })
}

fn delete(s: &mut Session, p: &Json) -> Result<Json> {
    let (si, pi) = find_pivot(s, p)?;
    edit(s, |cx| {
        // Its PivotCharts become ordinary charts.
        let link = cx.wb.sheet(si).and_then(|sh| sh.pivots.get(pi).map(|pt| (sh.name.clone(), pt.name.clone())));
        if let Some((sheet, name)) = link {
            for i in 0..cx.wb.sheets.len() {
                let linked = |c: &Chart| c.pivot.as_deref().is_some_and(|l| pv::links_to(l, &sheet, &name));
                if cx.wb.sheet(i).is_some_and(|sh| sh.charts.iter().any(linked)) {
                    for c in cx.sheet_mut(i)?.charts.iter_mut().filter(|c| linked(c)) {
                        c.pivot = None;
                    }
                }
            }
        }
        pv::clear_output(&mut cx.wb, si, pi);
        let sh = cx.sheet_mut(si)?;
        if pi < sh.pivots.len() {
            sh.pivots.remove(pi);
        }
        cx.structural = true;
        Ok(json!({}))
    })
}

// ---------------------------------------------------------------- PivotChart

/// The PivotTable a PivotChart call targets: `pivot` may also be a cell of its report.
fn chart_pivot(s: &Session, p: &Json) -> Result<(usize, usize)> {
    if let Some(t) = str_param(p, "pivot") {
        let d = s.doc()?;
        let (sheet, addr) = match t.rsplit_once('!') {
            Some((sh, a)) => {
                let sh = sh.trim();
                let sh = gridcraft_core::unquote_sheet(sh);
                (d.wb.sheet_index(&sh), a)
            }
            None => (Some(d.wb.active_sheet), t),
        };
        if let (Some(si), Some(c)) = (sheet, CellRef::parse(addr.trim().replace('$', "").as_str())) {
            let hit = d.wb.sheet(si).and_then(|sh| sh.pivots.iter().position(|pt| pt.last_range.is_some_and(|r| r.contains(c)) || pt.anchor == c));
            return hit.map(|pi| (si, pi)).ok_or_else(|| bad("insert.pivotChart", format!("{t} is not in a PivotTable")));
        }
    }
    find_pivot(s, p)
}

fn insert_pivot_chart(s: &mut Session, p: &Json) -> Result<Json> {
    let (si, pi) = chart_pivot(s, p)?;
    let d = s.doc()?;
    let sh = d.wb.sheet(si).ok_or(EngineError::NoDocument)?;
    let pt = sh.pivots.get(pi).ok_or_else(|| other("There's no such PivotTable."))?;
    let area = pt.last_range.ok_or_else(|| other("Refresh the PivotTable before adding a PivotChart."))?;
    let out = pv::compute(&d.wb, pt, si).map_err(other)?;
    if out.chart_rows.is_empty() || out.chart_cols.is_empty() {
        return Err(other("Add a field to the Values area and one to Rows or Columns before creating a PivotChart."));
    }
    let series = pv::chart_series(&sh.name, area.start, &out);
    let t = str_param(p, "type").unwrap_or("column");
    let kind = super::insert::chart_kind(t, str_param(p, "subtype").unwrap_or(""));
    let at =
        cell_param(s, p, "at")?.unwrap_or_else(|| CellRef::new(area.start.row, area.end.col.saturating_add(2).min(gridcraft_core::MAX_COLS - 1)));
    let title = match p.get("title") {
        Some(Json::String(t)) => Some(t.clone()),
        Some(Json::Null) => None,
        _ => Some(if series.len() == 1 { "Total".to_string() } else { "Chart Title".to_string() }),
    };
    let id = d.wb.next_object_id();
    let link = pv::chart_link(&sh.name, &pt.name);
    let pivot_name = pt.name.clone();
    let n = series.len();
    let chart = Chart {
        id,
        kind,
        anchor: Anchor {
            cell: at,
            dx: 0.0,
            dy: 0.0,
            width: f64_param(p, "width").unwrap_or(480.0).clamp(10.0, 10_000.0) as f32,
            height: f64_param(p, "height").unwrap_or(288.0).clamp(10.0, 10_000.0) as f32,
            mode: anchor_mode_param(p).unwrap_or_default(),
        },
        title,
        series,
        legend: if matches!(kind, ChartKind::Pie | ChartKind::Doughnut) { LegendPos::Right } else { LegendPos::Bottom },
        data_labels: false,
        gridlines: !matches!(kind, ChartKind::Pie | ChartKind::Doughnut | ChartKind::Treemap | ChartKind::Sunburst | ChartKind::Funnel),
        style: 1,
        x_title: None,
        y_title: None,
        source: None,
        by_rows: false,
        pivot: Some(link),
        ..Chart::default()
    };
    edit(s, |cx| {
        cx.sheet_mut(si)?.charts.push(chart);
        Ok(json!({"chart": id, "pivot": pivot_name, "series": n}))
    })
}

// ---------------------------------------------------------------- recommended

fn suggestions(wb: &Workbook, src: &pv::Source) -> Vec<Json> {
    let n = src.headers.len();
    let h = |i: usize| src.headers.get(i).cloned().unwrap_or_default();
    let distinct = |i: usize| pv::distinct_labels(wb, src, i, crate::pivot::Grouping::default(), 1001).len();
    let records = src.rows.len().max(1);
    let is_num = |i: usize| src.numeric.get(i).copied().unwrap_or(false);
    let is_date = |i: usize| src.dates.get(i).copied().unwrap_or(false);
    // Category columns: text with repeats.
    let cats: Vec<usize> = (0..n)
        .filter(|&i| {
            !is_num(i) && {
                let d = distinct(i);
                d >= 1 && (d < records || records <= 2) && d <= 1000
            }
        })
        .collect();
    let nums: Vec<usize> = (0..n).filter(|&i| is_num(i) && !is_date(i)).collect();
    let dates: Vec<usize> = (0..n).filter(|&i| is_date(i)).collect();
    let mut out = vec![];
    let mut push = |title: String, rows: Vec<Json>, cols: Vec<Json>, vals: Vec<Json>| {
        if out.len() < 5 {
            let idx = out.len();
            out.push(json!({"index": idx, "title": title, "rows": rows, "columns": cols, "values": vals}));
        }
    };
    if let (Some(&c), Some(&v)) = (cats.first(), nums.first()) {
        push(format!("Sum of {} by {}", h(v), h(c)), vec![json!(h(c))], vec![], vec![json!({"field": h(v), "func": "sum"})]);
    }
    if let (Some(&c), Some(&c2), Some(&v)) = (cats.first(), cats.get(1), nums.first()) {
        push(
            format!("Sum of {} by {} and {}", h(v), h(c), h(c2)),
            vec![json!(h(c))],
            vec![json!(h(c2))],
            vec![json!({"field": h(v), "func": "sum"})],
        );
    }
    if let (Some(&d), Some(&v)) = (dates.first(), nums.first()) {
        push(
            format!("Sum of {} by {} (quarters)", h(v), h(d)),
            vec![json!({"field": h(d), "group": "quarters"})],
            vec![],
            vec![json!({"field": h(v), "func": "sum"})],
        );
    }
    if let (Some(&c), Some(&v), Some(&v2)) = (cats.first(), nums.first(), nums.get(1)) {
        push(
            format!("Sum of {} and {} by {}", h(v), h(v2), h(c)),
            vec![json!(h(c))],
            vec![],
            vec![json!({"field": h(v), "func": "sum"}), json!({"field": h(v2), "func": "sum"})],
        );
    }
    if let Some(&c) = cats.first() {
        push(format!("Count of {} by {}", h(c), h(c)), vec![json!(h(c))], vec![], vec![json!({"field": h(c), "func": "count"})]);
    }
    if let (Some(&c), Some(&v)) = (cats.get(1), nums.first()) {
        push(format!("Average of {} by {}", h(v), h(c)), vec![json!(h(c))], vec![], vec![json!({"field": h(v), "func": "average"})]);
    }
    if cats.is_empty()
        && dates.is_empty()
        && let Some(&v) = nums.first()
    {
        push(format!("Sum of {}", h(v)), vec![], vec![], vec![json!({"field": h(v), "func": "sum"})]);
    }
    out
}

fn recommended(s: &mut Session, p: &Json) -> Result<Json> {
    let source = source_param(s, p)?;
    let active = s.doc()?.wb.active_sheet;
    let src = pv::read_source(&s.doc()?.wb, &source, active).map_err(other)?;
    let list = suggestions(&s.doc()?.wb, &src);
    let Some(apply) = p.get("apply").and_then(Json::as_u64) else {
        return Ok(json!({"source": source, "suggestions": list}));
    };
    let pick = list.get(apply as usize).cloned().ok_or_else(|| bad("insert.recommendedPivotTables", "no suggestion with that index"))?;
    let mut q = json!({
        "source": source,
        "rows": pick.get("rows").cloned().unwrap_or(json!([])),
        "columns": pick.get("columns").cloned().unwrap_or(json!([])),
        "values": pick.get("values").cloned().unwrap_or(json!([])),
    });
    if let (Some(m), Some(d)) = (q.as_object_mut(), p.get("destination")) {
        m.insert("destination".into(), d.clone());
    }
    if let (Some(m), Some(r)) = (q.as_object_mut(), p.get("replace")) {
        m.insert("replace".into(), r.clone());
    }
    let mut res = insert_pivot(s, &q)?;
    if let Some(m) = res.as_object_mut() {
        m.insert("suggestion".into(), pick);
    }
    Ok(res)
}

// ---------------------------------------------------------------- Show Details / GetPivotData

/// A fresh sheet name for a Show Details sheet: the item path under Excel's naming rules, with a
/// numeric suffix when that name is taken (`East`, `East (2)`, ...).
fn detail_sheet_name(wb: &Workbook, title: &str) -> String {
    let clean = |s: &str| -> String {
        // Excel's invalid sheet-name characters, plus the apostrophe rule.
        let kept: String =
            s.chars().take(31).map(|c| if matches!(c, ':' | '/' | '?' | '*' | '[' | ']') || c as u32 == 92 { '-' } else { c }).collect();
        kept.trim().trim_matches('\'').to_string()
    };
    let base = clean(title);
    let base = if base.is_empty() { "Details".to_string() } else { base };
    // "History" is a reserved sheet name, so it counts as taken.
    let taken = |n: &str| wb.sheet_index(n).is_some() || n.eq_ignore_ascii_case("History");
    if !taken(&base) {
        return base;
    }
    for k in 2..10_000u32 {
        let suffix = format!(" ({k})");
        let room = 31usize.saturating_sub(suffix.chars().count()).max(1);
        let head: String = base.chars().take(room).collect();
        let n = format!("{}{suffix}", head.trim_end());
        if !taken(&n) {
            return n;
        }
    }
    wb.next_sheet_name()
}

/// Excel's Show Details: writes the source records behind one report cell into a new sheet right
/// after the PivotTable's, as one undo step.
fn show_details(s: &mut Session, p: &Json) -> Result<Json> {
    if s.doc()?.wb.protected_structure {
        return Err(other("The workbook is protected and cannot be changed."));
    }
    let (si, pi) = find_pivot(s, p)?;
    let (detail, name) = {
        let d = s.doc()?;
        let wb = &d.wb;
        let sh = wb.sheet(si).ok_or(EngineError::NoDocument)?;
        let pt = sh.pivots.get(pi).ok_or_else(|| other("There's no such PivotTable."))?;
        // Show Details needs the records behind a cell, and a report GridCraft does not compute has
        // no source to read them from.
        if pt.is_read_only() {
            return Err(read_only_error(pt));
        }
        // The cell: `cell`, else the active cell when it is in the report, else its corner.
        let active = d.selection.active;
        let at = cell_param_on(s, p, "cell", si)?.unwrap_or_else(|| {
            if pt.last_range.is_some_and(|r| r.contains(active)) { active } else { pt.last_range.map(|r| r.start).unwrap_or(pt.anchor) }
        });
        let detail = pv::details(wb, pt, si, at).map_err(other)?;
        let name = detail_sheet_name(wb, &detail.title);
        (detail, name)
    };
    let rows = detail.rows.len();
    let cells = (rows as u64 + 1).saturating_mul(detail.headers.len() as u64);
    let skipped = detail.skipped;
    let title = detail.title.clone();
    let idx = edit(s, |cx| {
        if cx.wb.protected_structure {
            return Err(other("The workbook is protected and cannot be changed."));
        }
        let at = si.saturating_add(1).min(cx.wb.sheets.len());
        cx.wb.sheets.insert(at, Arc::new(Sheet::new(name.clone())));
        // Names scoped to later sheets shift.
        for n in cx.wb.names.iter_mut() {
            if let Some(sc) = n.scope.as_mut()
                && *sc >= at
            {
                *sc += 1;
            }
        }
        // One style per source column, so dates and currency look like the source list.
        let styles: Vec<StyleId> = detail
            .formats
            .iter()
            .map(|f| if f == "General" { StyleId::default() } else { cx.wb.styles.intern(Style { num_fmt: NumFmt::new(f), ..Default::default() }) })
            .collect();
        let sh = cx.sheet_mut(at)?;
        for (c, h) in detail.headers.iter().enumerate() {
            let Ok(col) = u32::try_from(c) else { break };
            sh.set_cell(CellRef::new(0, col), Cell { value: Value::text(h.as_str()), formula: None, style: StyleId::default() });
        }
        for (r, rec) in detail.rows.iter().enumerate() {
            let Ok(row) = u32::try_from(r) else { break };
            for (c, v) in rec.iter().enumerate() {
                if v.is_empty() {
                    continue;
                }
                let Ok(col) = u32::try_from(c) else { break };
                let style = styles.get(c).copied().unwrap_or_default();
                sh.set_cell(CellRef::new(row.saturating_add(1), col), Cell { value: v.clone(), formula: None, style });
            }
        }
        cx.wb.active_sheet = at;
        *cx.sel = crate::Selection::default();
        cx.structural = true;
        Ok(at)
    })?;
    {
        let d = s.doc_mut()?;
        d.sheet_selections.insert(idx.min(d.sheet_selections.len()), crate::Selection::default());
    }
    Ok(json!({"sheet": idx, "name": name, "title": title, "rows": rows, "cells": cells, "skipped": skipped}))
}

/// Excel's "Options > Generate GetPivotData": a toggle (`on`) and a getter, remembered on the
/// PivotTable, so a click on a report value while a formula is being typed inserts a
/// `GETPIVOTDATA` call instead of a plain reference.
fn generate_get_pivot_data(s: &mut Session, p: &Json) -> Result<Json> {
    if let Some(on) = p.get("on").and_then(Json::as_bool) {
        let (si, pi) = find_pivot(s, p)?;
        return edit(s, |cx| {
            let pt = cx.sheet_mut(si)?.pivots.get_mut(pi).ok_or_else(|| other("There's no such PivotTable."))?;
            pt.generate_get_pivot_data = on;
            Ok(json!({"on": on, "pivot": pt.name.clone()}))
        });
    }
    let (si, pi) = find_pivot(s, p)?;
    let d = s.doc()?;
    let wb = &d.wb;
    let sh = wb.sheet(si).ok_or(EngineError::NoDocument)?;
    let pt = sh.pivots.get(pi).ok_or_else(|| other("There's no such PivotTable."))?;
    let at = cell_param_on(s, p, "cell", si)?.or_else(|| pt.last_range.map(|r| r.start)).unwrap_or(pt.anchor);
    let mut out = json!({"pivot": pt.name, "on": pt.generate_get_pivot_data, "cell": at.a1()});
    if pt.generate_get_pivot_data
        && let Some(f) = pv::get_pivot_data(wb, si, at)
    {
        out["formula"] = json!(format!("={f}"));
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use gridcraft_core::{CellError, CellRef, Value};
    use serde_json::{Value as Json, json};

    use crate::Session;

    /// Region, Product, Date, Units, Sales — 12 records over 2026.
    fn sales() -> Session {
        let mut s = Session::new();
        s.new_workbook();
        let rows = json!([
            ["Region", "Product", "Date", "Units", "Sales"],
            ["East", "Apples", "1/15/2026", 10, 100],
            ["West", "Apples", "2/10/2026", 5, 50],
            ["East", "Pears", "3/3/2026", 8, 120],
            ["North", "Pears", "4/20/2026", 2, 30],
            ["West", "Plums", "5/5/2026", 7, 70],
            ["East", "Plums", "6/30/2026", 4, 60],
            ["North", "Apples", "7/4/2026", 9, 90],
            ["West", "Pears", "8/8/2026", 3, 45],
            ["East", "Apples", "9/9/2026", 6, 60],
            ["North", "Plums", "10/10/2026", 1, 15],
            ["West", "Apples", "11/11/2026", 12, 120],
            ["East", "Pears", "12/12/2026", 5, 75]
        ]);
        s.execute("range.setValues", json!({"range": "A1", "values": rows})).unwrap();
        s.execute("selection.set", json!({"range": "A1"})).unwrap();
        s
    }
    const TOTAL: f64 = 835.0;

    fn val(s: &Session, a: &str) -> Value {
        s.doc().unwrap().wb.active().unwrap().value(CellRef::parse(a).unwrap())
    }
    fn text(s: &Session, a: &str) -> String {
        val(s, a).display()
    }
    fn pt(s: &Session) -> gridcraft_model::PivotTable {
        s.doc().unwrap().wb.active().unwrap().pivots.last().unwrap().clone()
    }
    /// Finds the row (0-based) whose first report column holds `label`.
    fn find_row(s: &Session, label: &str) -> Option<u32> {
        let p = pt(s);
        let r = p.last_range.unwrap();
        (r.start.row..=r.end.row).find(|row| val(s, &CellRef::new(*row, r.start.col).a1()).display() == label)
    }

    #[test]
    fn insert_creates_placeholder_on_new_sheet() {
        let mut s = sales();
        let r = s.execute("insert.pivotTable", json!({})).unwrap();
        assert_eq!(r["name"], "PivotTable1");
        assert_eq!(r["anchor"], "A3");
        assert_eq!(r["fields"], json!(["Region", "Product", "Date", "Units", "Sales"]));
        let d = s.doc().unwrap();
        assert_eq!(d.wb.sheets.len(), 2);
        assert_eq!(d.wb.active_sheet, 0, "new sheet goes before the current one");
        assert_eq!(d.wb.active().unwrap().name, "Sheet2");
        assert!(text(&s, "A3").starts_with("PivotTable1"));
        assert_eq!(pt(&s).source, "Sheet1!$A$1:$E$13");
        // One undo step removes it.
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc().unwrap().wb.sheets.len(), 1);
    }

    #[test]
    fn region_by_product_sum_of_sales() {
        let mut s = sales();
        s.execute("insert.pivotTable", json!({})).unwrap();
        s.execute("pivot.addField", json!({"field": "region", "area": "rows"})).unwrap();
        s.execute("pivot.addField", json!({"field": "Product", "area": "columns"})).unwrap();
        let r = s.execute("pivot.addField", json!({"field": "Sales", "area": "values"})).unwrap();
        assert_eq!(r["range"], "A3:E8");
        assert_eq!(text(&s, "A3"), "Sum of Sales");
        assert_eq!(text(&s, "B3"), "Column Labels");
        assert_eq!(text(&s, "A4"), "Row Labels");
        assert_eq!(text(&s, "B4"), "Apples");
        assert_eq!(text(&s, "C4"), "Pears");
        assert_eq!(text(&s, "D4"), "Plums");
        assert_eq!(text(&s, "E4"), "Grand Total");
        assert_eq!(text(&s, "A5"), "East");
        assert_eq!(text(&s, "A6"), "North");
        assert_eq!(text(&s, "A7"), "West");
        assert_eq!(text(&s, "A8"), "Grand Total");
        assert_eq!(val(&s, "B5"), Value::Number(160.0));
        assert_eq!(val(&s, "C5"), Value::Number(195.0));
        assert_eq!(val(&s, "E5"), Value::Number(415.0));
        assert_eq!(val(&s, "E8"), Value::Number(TOTAL));
        // Column grand totals add up too.
        let sum: f64 = ["B8", "C8", "D8"].iter().map(|a| val(&s, a).as_f64().unwrap()).sum();
        assert_eq!(sum, TOTAL);
        let rows: f64 = ["E5", "E6", "E7"].iter().map(|a| val(&s, a).as_f64().unwrap()).sum();
        assert_eq!(rows, TOTAL);
        // Styles: header bold, grand total bold with a top border.
        let d = s.doc().unwrap();
        let sh = d.wb.active().unwrap();
        assert!(d.wb.styles.get(sh.style_id(CellRef::parse("A4").unwrap())).font.bold);
        let g = d.wb.styles.get(sh.style_id(CellRef::parse("B8").unwrap()));
        assert!(g.font.bold && !g.borders.top.is_none());
        // Listing.
        let l = s.execute("pivot.list", json!({})).unwrap();
        assert_eq!(l[0]["values"][0]["name"], "Sum of Sales");
    }

    #[test]
    fn multiple_value_fields_make_a_values_dimension() {
        let mut s = sales();
        s.execute("insert.pivotTable", json!({"rows": ["Region"], "values": ["Sales", {"field": "Units", "func": "average"}]})).unwrap();
        assert_eq!(text(&s, "A3"), "Row Labels");
        assert_eq!(text(&s, "B3"), "Sum of Sales");
        assert_eq!(text(&s, "C3"), "Average of Units");
        assert_eq!(text(&s, "A7"), "Grand Total");
        assert_eq!(val(&s, "B7"), Value::Number(TOTAL));
        assert_eq!(val(&s, "C7"), Value::Number(72.0 / 12.0));
        // With a column field the values become the innermost column level.
        s.execute("pivot.addField", json!({"field": "Product", "area": "columns"})).unwrap();
        assert_eq!(text(&s, "B4"), "Apples");
        assert_eq!(text(&s, "B5"), "Sum of Sales");
        assert_eq!(text(&s, "C5"), "Average of Units");
        assert_eq!(text(&s, "D4"), "Pears");
        assert_eq!(text(&s, "H4"), "Total Sum of Sales");
        assert_eq!(text(&s, "I4"), "Total Average of Units");
        let last = pt(&s).last_range.unwrap();
        assert_eq!(val(&s, &CellRef::new(last.end.row, 7).a1()), Value::Number(TOTAL));
    }

    #[test]
    fn date_grouping_by_quarter_and_month() {
        let mut s = sales();
        s.execute("insert.pivotTable", json!({"rows": ["Date"], "values": ["Sales"]})).unwrap();
        // Ungrouped: one row per date, shown with the source date format.
        assert_eq!(pt(&s).last_range.unwrap().height(), 14);
        s.execute("pivot.group", json!({"field": "Date", "by": "quarters"})).unwrap();
        assert_eq!(text(&s, "A4"), "Qtr1");
        assert_eq!(text(&s, "A7"), "Qtr4");
        assert_eq!(val(&s, "B4"), Value::Number(270.0));
        assert_eq!(val(&s, "B5"), Value::Number(160.0));
        assert_eq!(val(&s, "B8"), Value::Number(TOTAL));
        s.execute("pivot.group", json!({"field": "Date", "by": "months"})).unwrap();
        assert_eq!(text(&s, "A4"), "Jan");
        assert_eq!(text(&s, "A15"), "Dec");
        s.execute("pivot.group", json!({"field": "Date", "by": "years"})).unwrap();
        assert_eq!(text(&s, "A4"), "2026");
        assert!(s.execute("pivot.group", json!({"field": "Region", "by": "years"})).is_err());
    }

    #[test]
    fn filters_and_report_filter_rows() {
        let mut s = sales();
        s.execute("insert.pivotTable", json!({"rows": ["Product"], "values": ["Sales"], "filters": ["Region"]})).unwrap();
        // Report filter above the body; the body stays at A3.
        assert_eq!(text(&s, "A1"), "Region");
        assert_eq!(text(&s, "B1"), "(All)");
        assert_eq!(text(&s, "A3"), "Row Labels");
        s.execute("pivot.filter", json!({"field": "Region", "selected": ["East"]})).unwrap();
        assert_eq!(text(&s, "B1"), "East");
        let gt = find_row(&s, "Grand Total").unwrap();
        assert_eq!(val(&s, &CellRef::new(gt, 1).a1()), Value::Number(415.0));
        s.execute("pivot.filter", json!({"field": "Region", "selected": ["East", "west"]})).unwrap();
        assert_eq!(text(&s, "B1"), "(Multiple Items)");
        s.execute("pivot.filter", json!({"field": "Region", "selected": null})).unwrap();
        let gt = find_row(&s, "Grand Total").unwrap();
        assert_eq!(val(&s, &CellRef::new(gt, 1).a1()), Value::Number(TOTAL));
        // Hidden items on a row field.
        s.execute("pivot.filter", json!({"field": "Product", "selected": ["Apples"]})).unwrap();
        assert_eq!(text(&s, "A4"), "Apples");
        assert_eq!(text(&s, "A5"), "Grand Total");
        assert_eq!(val(&s, "B5"), Value::Number(420.0));
        assert!(s.execute("pivot.filter", json!({"field": "Product", "selected": []})).is_err());
    }

    #[test]
    fn a_page_filter_on_an_axis_field_draws_its_row() {
        let mut s = sales();
        s.execute("insert.pivotTable", json!({"rows": ["Product"], "values": ["Sales"]})).unwrap();
        // A file can hold a field in the Filters area *and* on an axis; the filter record then says
        // it is a report filter, and it keeps its row above the table.
        {
            let sh = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap();
            sh.pivots[0].filters = vec![gridcraft_model::PivotFilter {
                source_col: "Product".into(),
                selected: Some(vec!["Apples".into()]),
                area: gridcraft_model::PivotFilterArea::Page,
            }];
        }
        s.execute("pivot.refresh", json!({})).unwrap();
        assert_eq!(text(&s, "A1"), "Product", "the report filter's row");
        assert_eq!(text(&s, "B1"), "Apples");
        assert_eq!(text(&s, "A3"), "Row Labels", "the body moved down by the page row");
        // The same record as an axis filter draws no row: it only hides items.
        let mut s = sales();
        s.execute("insert.pivotTable", json!({"rows": ["Product"], "values": ["Sales"]})).unwrap();
        {
            let sh = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap();
            sh.pivots[0].filters = vec![gridcraft_model::PivotFilter {
                source_col: "Product".into(),
                selected: Some(vec!["Apples".into()]),
                area: gridcraft_model::PivotFilterArea::Axis,
            }];
        }
        s.execute("pivot.refresh", json!({})).unwrap();
        assert_eq!(text(&s, "A1"), "", "no page row for an axis filter");
        assert_eq!(text(&s, "A3"), "Row Labels");
        assert_eq!(text(&s, "A4"), "Apples");
        assert_eq!(text(&s, "A5"), "Grand Total");
    }

    #[test]
    fn show_values_as() {
        let mut s = sales();
        s.execute("insert.pivotTable", json!({"rows": ["Region"], "values": ["Sales"]})).unwrap();
        s.execute("pivot.valueSettings", json!({"field": "Sum of Sales", "showAs": "percentOfGrandTotal"})).unwrap();
        let east = val(&s, "B4").as_f64().unwrap();
        assert!((east - 415.0 / TOTAL).abs() < 1e-12);
        assert_eq!(val(&s, "B7"), Value::Number(1.0));
        assert_eq!(text(&s, "B7"), "1");
        let d = s.doc().unwrap();
        let sh = d.wb.active().unwrap();
        assert_eq!(d.wb.styles.get(sh.style_id(CellRef::parse("B4").unwrap())).num_fmt.as_str(), "0.00%");
        s.execute("pivot.valueSettings", json!({"index": 0, "showAs": "runningTotal"})).unwrap();
        assert_eq!(val(&s, "B4"), Value::Number(415.0));
        assert_eq!(val(&s, "B5"), Value::Number(415.0 + 135.0));
        assert_eq!(val(&s, "B6"), Value::Number(TOTAL));
        s.execute("pivot.valueSettings", json!({"index": 0, "showAs": "rank"})).unwrap();
        assert_eq!(val(&s, "B4"), Value::Number(1.0));
        assert_eq!(val(&s, "B5"), Value::Number(3.0));
        assert_eq!(val(&s, "B6"), Value::Number(2.0));
        s.execute("pivot.valueSettings", json!({"index": 0, "showAs": "normal", "func": "max", "numberFormat": "#,##0"})).unwrap();
        assert_eq!(text(&s, "B3"), "Max of Sales");
        assert_eq!(val(&s, "B4"), Value::Number(120.0));
        assert!(s.execute("pivot.valueSettings", json!({"index": 0, "name": "Sales"})).is_err());
        s.execute("pivot.valueSettings", json!({"index": 0, "name": "Biggest sale"})).unwrap();
        assert_eq!(text(&s, "B3"), "Biggest sale");
    }

    #[test]
    fn compact_vs_tabular_shapes() {
        let mut s = sales();
        s.execute("insert.pivotTable", json!({"rows": ["Region", "Product"], "values": ["Sales"]})).unwrap();
        // Compact: one label column; parents carry subtotals at the top; children indented.
        let r = pt(&s).last_range.unwrap();
        assert_eq!(r.width(), 2);
        // header + 3 regions + 9 (region, product) pairs present + grand total
        assert_eq!(r.height(), 1 + 3 + 9 + 1);
        assert_eq!(text(&s, "A4"), "East");
        assert_eq!(val(&s, "B4"), Value::Number(415.0));
        assert_eq!(text(&s, "A5"), "Apples");
        let d = s.doc().unwrap();
        let sh = d.wb.active().unwrap();
        assert_eq!(d.wb.styles.get(sh.style_id(CellRef::parse("A5").unwrap())).align.indent, 1);
        assert!(d.wb.styles.get(sh.style_id(CellRef::parse("A4").unwrap())).font.bold);
        // Outline: one column per field.
        s.execute("pivot.layout", json!({"layout": "outline"})).unwrap();
        let r = pt(&s).last_range.unwrap();
        assert_eq!(r.width(), 3);
        assert_eq!(text(&s, "A3"), "Region");
        assert_eq!(text(&s, "B3"), "Product");
        assert_eq!(text(&s, "A4"), "East");
        assert_eq!(text(&s, "B5"), "Apples");
        // Tabular: first child on its parent's row; "East Total" rows below each group.
        s.execute("pivot.layout", json!({"layout": "tabular"})).unwrap();
        let r = pt(&s).last_range.unwrap();
        assert_eq!(r.width(), 3);
        assert_eq!(r.height(), 1 + 9 + 3 + 1);
        assert_eq!(text(&s, "A4"), "East");
        assert_eq!(text(&s, "B4"), "Apples");
        assert_eq!(text(&s, "A5"), "");
        assert_eq!(text(&s, "B5"), "Pears");
        assert_eq!(text(&s, "A7"), "East Total");
        assert_eq!(val(&s, "C7"), Value::Number(415.0));
        // No subtotals, no grand totals.
        s.execute("pivot.layout", json!({"subtotals": false, "grandTotalsCols": false})).unwrap();
        assert_eq!(pt(&s).last_range.unwrap().height(), 1 + 9);
        // Collapse East in compact layout: its children disappear.
        s.execute("pivot.layout", json!({"layout": "compact", "subtotals": "top"})).unwrap();
        s.execute("pivot.collapse", json!({"field": "Region", "items": ["East"]})).unwrap();
        assert_eq!(text(&s, "A4"), "East");
        assert_eq!(text(&s, "A5"), "North");
        s.execute("pivot.collapse", json!({"field": "Region", "collapse": false})).unwrap();
        assert_eq!(text(&s, "A5"), "Apples");
    }

    #[test]
    fn values_on_rows_layouts() {
        let mut s = sales();
        s.execute("insert.pivotTable", json!({"rows": ["Region"], "values": ["Sales", "Units"]})).unwrap();
        assert_eq!(text(&s, "C3"), "Sum of Units");
        // Σ Values after Region: each region holds one line per value field.
        s.execute("pivot.moveField", json!({"field": "Σ Values", "from": "columns", "to": "rows"})).unwrap();
        let p = pt(&s);
        assert!(p.values_on_rows);
        assert_eq!(p.values_position, None);
        let r = p.last_range.unwrap();
        assert_eq!(r.width(), 2);
        // header + 3 × (region + 2 values) + 2 grand totals
        assert_eq!(r.height(), 1 + 9 + 2);
        assert_eq!(text(&s, "A4"), "East");
        assert_eq!(val(&s, "B4"), Value::Empty);
        assert_eq!(text(&s, "A5"), "Sum of Sales");
        assert_eq!(val(&s, "B5"), Value::Number(415.0));
        assert_eq!(text(&s, "A6"), "Sum of Units");
        assert_eq!(val(&s, "B6"), Value::Number(33.0));
        assert_eq!(text(&s, "A13"), "Total Sum of Sales");
        assert_eq!(val(&s, "B13"), Value::Number(TOTAL));
        assert_eq!(text(&s, "A14"), "Total Sum of Units");
        assert_eq!(val(&s, "B14"), Value::Number(72.0));
        // Tabular, no column field: "Total" heads the data column, as Excel saves it.
        s.execute("pivot.layout", json!({"layout": "tabular"})).unwrap();
        assert_eq!(text(&s, "A3"), "Region");
        assert_eq!(text(&s, "B3"), "Values");
        assert_eq!(text(&s, "C3"), "Total");
        s.execute("pivot.layout", json!({"layout": "compact"})).unwrap();
        // Σ Values before Region: the value fields are the outer items.
        s.execute("pivot.moveField", json!({"field": "Values", "from": "rows", "to": "rows", "position": 0})).unwrap();
        assert_eq!(pt(&s).values_position, Some(0));
        assert_eq!(text(&s, "A4"), "Sum of Sales");
        assert_eq!(val(&s, "B4"), Value::Empty);
        assert_eq!(text(&s, "A5"), "East");
        assert_eq!(val(&s, "B5"), Value::Number(415.0));
        assert_eq!(text(&s, "A8"), "Sum of Units");
        assert_eq!(text(&s, "A9"), "East");
        assert_eq!(val(&s, "B9"), Value::Number(33.0));
        assert_eq!(text(&s, "A12"), "Total Sum of Sales");
        assert_eq!(val(&s, "B13"), Value::Number(72.0));
        // Adding a row field in front keeps Σ Values ahead of Region.
        s.execute("pivot.addField", json!({"field": "Product", "area": "rows", "position": 0})).unwrap();
        assert_eq!(pt(&s).values_position, Some(1));
        s.execute("pivot.removeField", json!({"field": "Product", "area": "rows"})).unwrap();
        assert_eq!(pt(&s).values_position, Some(0));
        // No row field: one line per value field and no grand total.
        s.execute("pivot.removeField", json!({"field": "Region", "area": "rows"})).unwrap();
        let r = pt(&s).last_range.unwrap();
        assert_eq!(r.height(), 1 + 2);
        assert_eq!(text(&s, "A4"), "Sum of Sales");
        assert_eq!(val(&s, "B4"), Value::Number(TOTAL));
        assert_eq!(text(&s, "A5"), "Sum of Units");
        assert_eq!(val(&s, "B5"), Value::Number(72.0));
        // Back to Columns.
        s.execute("pivot.moveField", json!({"field": "Σ Values", "from": "rows", "to": "columns"})).unwrap();
        assert!(!pt(&s).values_on_rows);
        assert_eq!(text(&s, "A3"), "Sum of Sales");
        assert_eq!(text(&s, "B3"), "Sum of Units");
        assert!(s.execute("pivot.moveField", json!({"field": "Σ Values", "from": "columns", "to": "filters"})).is_err());
        assert!(s.execute("pivot.moveField", json!({"field": "Σ Values", "from": "rows", "to": "columns"})).is_err());
    }

    #[test]
    fn collapsed_item_puts_values_in_the_values_column() {
        let mut s = sales();
        s.execute("insert.pivotTable", json!({"rows": ["Region", "Product"], "values": ["Sales", "Units"]})).unwrap();
        s.execute("pivot.moveField", json!({"field": "Σ Values", "from": "columns", "to": "rows"})).unwrap();
        s.execute("pivot.collapse", json!({"field": "Region", "items": ["East"]})).unwrap();
        for layout in ["tabular", "outline"] {
            s.execute("pivot.layout", json!({"layout": layout})).unwrap();
            assert_eq!(text(&s, "C3"), "Values", "{layout}");
            let east = (4..20).find(|r| text(&s, &format!("A{r}")) == "East").unwrap();
            let first = if layout == "tabular" { east } else { east + 1 };
            assert_eq!(text(&s, &format!("B{first}")), "", "{layout}");
            assert_eq!(text(&s, &format!("C{first}")), "Sum of Sales", "{layout}");
            assert_eq!(val(&s, &format!("D{first}")), Value::Number(415.0), "{layout}");
            assert_eq!(text(&s, &format!("C{}", first + 1)), "Sum of Units", "{layout}");
        }
    }

    #[test]
    fn values_stay_last_in_columns() {
        let mut s = sales();
        s.execute("insert.pivotTable", json!({"rows": ["Product"], "columns": ["Region"], "values": ["Sales", "Units"]})).unwrap();
        s.execute("pivot.moveField", json!({"field": "Σ Values", "from": "columns", "to": "columns", "position": 0})).unwrap();
        let p = pt(&s);
        assert!(!p.values_on_rows);
        assert_eq!(p.values_position, None);
        // Σ Values to Rows and back with a position: still last in Columns.
        s.execute("pivot.moveField", json!({"field": "Σ Values", "from": "columns", "to": "rows"})).unwrap();
        s.execute("pivot.moveField", json!({"field": "Σ Values", "from": "rows", "to": "columns", "position": 0})).unwrap();
        assert_eq!(pt(&s).values_position, None);
        assert_eq!(text(&s, "B5"), "Sum of Sales");
        assert_eq!(text(&s, "C5"), "Sum of Units");
    }

    #[test]
    fn change_source_keeps_values_ahead_of_its_field() {
        let mut s = sales();
        s.execute("insert.pivotTable", json!({"rows": ["Region", "Product"], "values": ["Sales", "Units"]})).unwrap();
        s.execute("pivot.moveField", json!({"field": "Σ Values", "from": "columns", "to": "rows", "position": 1})).unwrap();
        // The new source has no Region: Σ Values stays ahead of Product.
        s.execute("pivot.changeSource", json!({"source": "Sheet1!B1:E13"})).unwrap();
        let p = pt(&s);
        assert_eq!(p.rows.len(), 1);
        assert_eq!(p.values_position, Some(0));
        assert_eq!(text(&s, "A4"), "Sum of Sales");
        assert_eq!(text(&s, "A5"), "Apples");
    }

    #[test]
    fn values_on_rows_between_row_fields() {
        let mut s = sales();
        s.execute("insert.pivotTable", json!({"rows": ["Region", "Product"], "values": ["Sales", "Units"]})).unwrap();
        s.execute("pivot.moveField", json!({"field": "Σ Values", "from": "columns", "to": "rows", "position": 1})).unwrap();
        // Compact: East, then per value field its products, then per-value subtotals of East.
        assert_eq!(text(&s, "A4"), "East");
        assert_eq!(text(&s, "A5"), "Sum of Sales");
        assert_eq!(text(&s, "A6"), "Apples");
        assert_eq!(val(&s, "B6"), Value::Number(160.0));
        assert_eq!(text(&s, "A9"), "Sum of Units");
        assert_eq!(text(&s, "A10"), "Apples");
        assert_eq!(val(&s, "B10"), Value::Number(16.0));
        assert_eq!(text(&s, "A13"), "East Sum of Sales");
        assert_eq!(val(&s, "B13"), Value::Number(415.0));
        assert_eq!(text(&s, "A14"), "East Sum of Units");
        assert_eq!(val(&s, "B14"), Value::Number(33.0));
        let d = s.doc().unwrap();
        let sh = d.wb.active().unwrap();
        assert_eq!(d.wb.styles.get(sh.style_id(CellRef::parse("A5").unwrap())).align.indent, 1);
        assert_eq!(d.wb.styles.get(sh.style_id(CellRef::parse("A6").unwrap())).align.indent, 2);
        // Outline: one column per level, "Values" heads the Σ Values column.
        s.execute("pivot.layout", json!({"layout": "outline"})).unwrap();
        assert_eq!(pt(&s).last_range.unwrap().width(), 4);
        assert_eq!(text(&s, "A3"), "Region");
        assert_eq!(text(&s, "B3"), "Values");
        assert_eq!(text(&s, "C3"), "Product");
        assert_eq!(text(&s, "B5"), "Sum of Sales");
        assert_eq!(text(&s, "C6"), "Apples");
        assert_eq!(val(&s, "D6"), Value::Number(160.0));
        // Tabular: labels start on their parent's row; totals per value field below each group.
        s.execute("pivot.layout", json!({"layout": "tabular"})).unwrap();
        assert_eq!(text(&s, "A4"), "East");
        assert_eq!(text(&s, "B4"), "Sum of Sales");
        assert_eq!(text(&s, "C4"), "Apples");
        assert_eq!(val(&s, "D4"), Value::Number(160.0));
        assert_eq!(text(&s, "B7"), "Sum of Units");
        assert_eq!(text(&s, "C7"), "Apples");
        assert_eq!(val(&s, "D7"), Value::Number(16.0));
        assert_eq!(text(&s, "A10"), "East Sum of Sales");
        assert_eq!(val(&s, "D10"), Value::Number(415.0));
        assert_eq!(text(&s, "A11"), "East Sum of Units");
        let r = pt(&s).last_range.unwrap();
        let last = r.end.row + 1;
        assert_eq!(text(&s, &format!("A{}", last - 1)), "Total Sum of Sales");
        assert_eq!(text(&s, &format!("A{last}")), "Total Sum of Units");
        assert_eq!(val(&s, &format!("D{last}")), Value::Number(72.0));
    }

    #[test]
    fn refresh_after_source_edits() {
        let mut s = sales();
        s.execute("insert.pivotTable", json!({"rows": ["Region"], "values": ["Sales"]})).unwrap();
        s.execute("sheet.activate", json!({"sheet": "Sheet1"})).unwrap();
        s.execute("cell.set", json!({"cell": "E2", "input": "1100"})).unwrap();
        // Refresh All picks the change up.
        let r = s.execute("data.refreshAll", json!({})).unwrap();
        assert_eq!(r["refreshed"], 1);
        let d = s.doc().unwrap();
        let si = d.wb.sheet_index("Sheet2").unwrap();
        let sh = d.wb.sheet(si).unwrap();
        assert_eq!(sh.value(CellRef::parse("B4").unwrap()), Value::Number(1415.0));
        assert_eq!(sh.value(CellRef::parse("B7").unwrap()), Value::Number(TOTAL + 1000.0));
        // pivot.refresh by name; change source to fewer rows.
        s.execute("pivot.changeSource", json!({"pivot": "PivotTable1", "source": "Sheet1!A1:E3"})).unwrap();
        let d = s.doc().unwrap();
        let sh = d.wb.sheet(si).unwrap();
        assert_eq!(sh.value(CellRef::parse("A6").unwrap()).display(), "Grand Total");
        assert_eq!(sh.value(CellRef::parse("B6").unwrap()), Value::Number(1150.0));
        // Old rows were cleared.
        assert_eq!(sh.value(CellRef::parse("A7").unwrap()), Value::Empty);
        s.execute("pivot.refresh", json!({"pivot": "PivotTable1"})).unwrap();
    }

    fn pivot_chart(s: &Session) -> gridcraft_model::Chart {
        s.doc().unwrap().wb.active().unwrap().charts.last().unwrap().clone()
    }
    fn plotted(s: &Session) -> Vec<Vec<Option<f64>>> {
        let d = s.doc().unwrap();
        let data = gridcraft_chart::resolve(&d.wb, d.wb.active_sheet, &pivot_chart(s));
        data.series.into_iter().map(|x| x.values).collect()
    }

    #[test]
    fn pivot_chart_follows_the_report() {
        let mut s = sales();
        s.execute("insert.pivotTable", json!({"rows": ["Region"], "values": ["Sales"]})).unwrap();
        // Any cell of the report picks the PivotTable.
        let r = s.execute("insert.pivotChart", json!({"pivot": "B5"})).unwrap();
        assert_eq!(r["series"], 1);
        let ch = pivot_chart(&s);
        assert_eq!(ch.pivot.as_deref(), Some("Sheet2!PivotTable1"));
        assert_eq!(ch.title.as_deref(), Some("Total"));
        assert_eq!(ch.series[0].values, "Sheet2!$B$4:$B$6");
        assert_eq!(ch.series[0].categories.as_deref(), Some("Sheet2!$A$4:$A$6"));
        assert_eq!(plotted(&s), vec![vec![Some(415.0), Some(135.0), Some(285.0)]]);
        // A column field: one series per item, rebound by the layout change.
        s.execute("pivot.addField", json!({"field": "Product", "area": "columns"})).unwrap();
        let ch = pivot_chart(&s);
        let names: Vec<_> = ch.series.iter().map(|x| x.name.clone().unwrap_or_default()).collect();
        assert_eq!(names, vec!["\"Apples\"", "\"Pears\"", "\"Plums\""]);
        assert_eq!(plotted(&s)[0], vec![Some(160.0), Some(90.0), Some(170.0)]);
        // Nested row fields: the innermost items only, as a union around the parent rows.
        s.execute("pivot.moveField", json!({"field": "Product", "from": "columns", "to": "rows"})).unwrap();
        let ch = pivot_chart(&s);
        assert!(ch.series[0].values.starts_with('('), "{}", ch.series[0].values);
        let pts = plotted(&s);
        assert_eq!(pts[0].len(), 9);
        assert_eq!(pts[0].iter().flatten().sum::<f64>(), TOTAL);
        // Survives an XLSX round trip as a PivotChart.
        let wb = (*s.doc().unwrap().wb).clone();
        let (back, _) = gridcraft_xlsx::read_xlsx(&gridcraft_xlsx::write_xlsx(&wb).unwrap()).unwrap();
        let si = back.sheet_index("Sheet2").unwrap();
        assert_eq!(back.sheet(si).unwrap().charts.first().and_then(|c| c.pivot.clone()).as_deref(), Some("Sheet2!PivotTable1"));
        // Renaming the sheet keeps the link; a refresh still rebinds.
        s.execute("sheet.rename", json!({"name": "Report"})).unwrap();
        assert_eq!(pivot_chart(&s).pivot.as_deref(), Some("Report!PivotTable1"));
        s.execute("pivot.moveField", json!({"field": "Region", "from": "rows", "to": "columns"})).unwrap();
        assert_eq!(pivot_chart(&s).series.len(), 3);
        // Deleting the PivotTable leaves an ordinary chart.
        s.execute("pivot.delete", json!({})).unwrap();
        assert_eq!(pivot_chart(&s).pivot, None);
    }

    #[test]
    fn pivot_chart_errors() {
        let mut s = sales();
        // No PivotTable yet.
        assert!(s.execute("insert.pivotChart", json!({})).is_err());
        s.execute("insert.pivotTable", json!({"rows": ["Region"]})).unwrap();
        // Not in the report; no values.
        assert!(s.execute("insert.pivotChart", json!({"pivot": "Z99"})).is_err());
        assert!(s.execute("insert.pivotChart", json!({})).unwrap_err().to_string().contains("Values"));
        s.execute("pivot.addField", json!({"field": "Units", "area": "values"})).unwrap();
        let r = s.execute("insert.pivotChart", json!({"pivot": "PivotTable1", "type": "line", "title": null})).unwrap();
        assert_eq!(r["series"], 1);
        let ch = pivot_chart(&s);
        assert_eq!(ch.kind, gridcraft_model::ChartKind::Line);
        assert_eq!(ch.title, None);
        // One undo step.
        s.execute("edit.undo", json!({})).unwrap();
        assert!(s.doc().unwrap().wb.active().unwrap().charts.is_empty());
    }

    #[test]
    fn calculated_field_on_sums() {
        let mut s = sales();
        s.execute("insert.pivotTable", json!({"rows": ["Region"], "values": ["Sales"]})).unwrap();
        let r = s.execute("pivot.calculatedField", json!({"name": "Price", "formula": "Sales / Units"})).unwrap();
        assert_eq!(r["formula"], "=Sales / Units");
        assert_eq!(text(&s, "C3"), "Sum of Price");
        assert_eq!(text(&s, "A4"), "East");
        // East: 415 sales over 33 units, from the sums.
        let east = val(&s, "C4").as_f64().unwrap();
        assert!((east - 415.0 / 33.0).abs() < 1e-12, "{east}");
        // The grand total applies the formula to the totals, not the sum of the rows.
        assert_eq!(text(&s, "A7"), "Grand Total");
        let total = val(&s, "C7").as_f64().unwrap();
        assert!((total - TOTAL / 72.0).abs() < 1e-12, "{total}");
        // Quoted names, functions, and another calculated field.
        s.execute("pivot.calculatedField", json!({"name": "Rounded", "formula": "=ROUND('Price'*2,1)"})).unwrap();
        assert_eq!(val(&s, "D4"), Value::Number(25.2));
        // Division by zero shows #DIV/0! cells.
        s.execute("pivot.calculatedField", json!({"name": "Zero", "formula": "=Sales/(Units-Units)"})).unwrap();
        assert_eq!(val(&s, "E4"), Value::Error(CellError::Div0));
        // Change and delete.
        s.execute("pivot.calculatedField", json!({"name": "zero", "formula": "=Units*2"})).unwrap();
        assert_eq!(val(&s, "E4"), Value::Number(66.0));
        s.execute("pivot.calculatedField", json!({"name": "Zero", "delete": true})).unwrap();
        assert_eq!(pt(&s).values.len(), 3);
        assert_eq!(pt(&s).calculated_fields.len(), 2);
        // The field list shows calculated fields.
        let f = s.execute("pivot.fields", json!({})).unwrap();
        assert!(f["fields"].as_array().unwrap().iter().any(|x| x["name"] == "Price" && x["calculated"] == true));
    }

    #[test]
    fn calculated_field_errors() {
        let mut s = sales();
        s.execute("insert.pivotTable", json!({"rows": ["Region"], "values": ["Sales"]})).unwrap();
        let mut e = |p: Json| s.execute("pivot.calculatedField", p).unwrap_err().to_string();
        assert!(e(json!({"name": "Bad", "formula": "=Sales*Bogus"})).contains("Bogus"));
        assert!(e(json!({"name": "Bad", "formula": "=A1*2"})).contains("cells"));
        assert!(e(json!({"name": "Bad", "formula": "=Sales*("})).contains("error"));
        assert!(e(json!({"name": "Sales", "formula": "=1"})).contains("already exists"));
        assert!(e(json!({"name": "Bad"})).contains("formula"));
        assert!(e(json!({"name": "Gone", "delete": true})).contains("Gone"));
        // A cycle is refused.
        s.execute("pivot.calculatedField", json!({"name": "X", "formula": "=Sales"})).unwrap();
        s.execute("pivot.calculatedField", json!({"name": "Y", "formula": "=X*2"})).unwrap();
        let err = s.execute("pivot.calculatedField", json!({"name": "X", "formula": "=Y"})).unwrap_err().to_string();
        assert!(err.contains("itself"), "{err}");
        // Calculated fields only go to Values; nothing changed after the failures.
        assert!(s.execute("pivot.addField", json!({"field": "Y", "area": "rows"})).is_err());
        assert_eq!(pt(&s).calculated_fields.len(), 2);
        // Without a name the dialog opens (menu-style call).
        s.ui_requests.clear();
        s.execute("pivot.calculatedField", json!({})).unwrap();
        assert!(matches!(s.ui_requests.last(), Some(crate::UiRequest::Dialog(n, _)) if n == "pivotCalculatedField"));
    }

    #[test]
    fn calculated_item_adds_an_item_from_other_items() {
        let mut s = sales();
        s.execute("insert.pivotTable", json!({"rows": ["Region"], "values": ["Sales"]})).unwrap();
        // Region order (ascending), the item after the real ones, Grand Total last.
        assert_eq!(text(&s, "A4"), "East");
        assert_eq!(text(&s, "A5"), "North");
        assert_eq!(text(&s, "A6"), "West");
        assert_eq!(text(&s, "A7"), "Grand Total");
        let r = s.execute("pivot.calculatedItem", json!({"field": "Region", "name": "East+West", "formula": "=East+West"})).unwrap();
        assert_eq!(r["formula"], "=East+West");
        assert_eq!(r["calculatedItems"][0]["name"], "East+West");
        // The calculated item sorts after the real items, before the grand total.
        assert_eq!(text(&s, "A7"), "East+West");
        assert_eq!(text(&s, "A8"), "Grand Total");
        // Its value is the formula over the items' aggregates: East 415 + West 285.
        assert_eq!(val(&s, "B7"), Value::Number(700.0));
        // The grand total stays the source total, calculated item left out.
        assert_eq!(val(&s, "B8"), Value::Number(TOTAL));
        // Arithmetic, another value field, and a caption that needs brackets.
        s.execute("pivot.addField", json!({"field": "Units", "area": "values"})).unwrap();
        s.execute("pivot.calculatedItem", json!({"field": "Region", "name": "Double", "formula": "=2*East-1"})).unwrap();
        let row = find_row(&s, "Double").unwrap();
        // East has 33 units: the formula applies to each value field's aggregate.
        assert_eq!(val(&s, &CellRef::new(row, 2).a1()), Value::Number(2.0 * 33.0 - 1.0));
        assert_eq!(val(&s, &CellRef::new(row, 1).a1()), Value::Number(2.0 * 415.0 - 1.0));
        // Delete removes it and the report shrinks back.
        s.execute("pivot.calculatedItem", json!({"name": "double", "delete": true})).unwrap();
        assert_eq!(text(&s, "A7"), "East+West");
        assert_eq!(text(&s, "A8"), "Grand Total");
        assert_eq!(pt(&s).calculated_items.len(), 1);
    }

    #[test]
    fn calculated_item_survives_save_and_recomputes_the_same_numbers() {
        let mut s = sales();
        s.execute("insert.pivotTable", json!({"rows": ["Region"], "values": ["Sales"]})).unwrap();
        s.execute("pivot.calculatedItem", json!({"field": "Region", "name": "East+West", "formula": "=East+West"})).unwrap();
        let report = |s: &Session| -> Vec<(String, String)> {
            let r = pt(s).last_range.unwrap();
            let sh = s.doc().unwrap().wb.active().unwrap();
            (r.start.row..=r.end.row)
                .flat_map(|row| (r.start.col..=r.end.col).map(move |col| CellRef::new(row, col)))
                .map(|c| (c.a1(), sh.value(c).display()))
                .collect()
        };
        let before = report(&s);
        assert!(before.iter().any(|(_, v)| v == "East+West"));
        let wb = (*s.doc().unwrap().wb).clone();
        let bytes = crate::io::save_bytes(&wb, "book.xlsx").unwrap();
        let (back, warnings) = crate::io::open_bytes("book.xlsx", &bytes).unwrap();
        assert!(warnings.iter().all(|w| !w.to_lowercase().contains("calculated")), "{warnings:?}");
        let mut t = Session::new();
        t.add_document(crate::DocState::new(back, None, "book.xlsx".into()));
        // A refresh after reopening recomputes the item from the cache's items.
        t.execute("pivot.refresh", json!({"pivot": "PivotTable1", "replace": true})).unwrap();
        let after = report(&t);
        assert_eq!(before, after);
    }

    #[test]
    fn calculated_item_errors() {
        let mut s = sales();
        s.execute("insert.pivotTable", json!({"rows": ["Region"], "values": ["Units"]})).unwrap();
        fn err(s: &mut Session, p: Json) -> String {
            s.execute("pivot.calculatedItem", p).unwrap_err().to_string()
        }
        // Unknown item, another field's item, a real item's caption, a bad formula, no name.
        assert!(err(&mut s, json!({"field": "Region", "name": "X", "formula": "=East+Bogus"})).contains("Bogus"));
        assert!(err(&mut s, json!({"field": "Region", "name": "X", "formula": "=Product[Apples]"})).contains("another field"));
        assert!(err(&mut s, json!({"field": "Region", "name": "East", "formula": "=West"})).contains("already an item"));
        assert!(err(&mut s, json!({"field": "Region", "name": "X", "formula": "=East*"})).contains("formula"));
        assert!(err(&mut s, json!({"field": "Region", "formula": "=East"})).contains("name"));
        // The field must be on an axis and ungrouped.
        assert!(err(&mut s, json!({"field": "Sales", "name": "X", "formula": "=1"})).contains("Rows or Columns"));
        s.execute("pivot.addField", json!({"field": "Date", "area": "rows"})).unwrap();
        s.execute("pivot.group", json!({"field": "Date", "by": "months"})).unwrap();
        let e = err(&mut s, json!({"field": "Date", "name": "X", "formula": "=Jan"}));
        assert!(e.contains("grouped"), "{e}");
        s.execute("pivot.removeField", json!({"field": "Date", "area": "rows"})).unwrap();
        // Nothing was stored by the failed calls.
        assert!(pt(&s).calculated_items.is_empty());
        // A cycle is refused: a calculated item cannot name another one.
        s.execute("pivot.addField", json!({"field": "Region", "area": "rows", "replace": true})).unwrap();
        s.execute("pivot.calculatedItem", json!({"field": "Region", "name": "Pair", "formula": "=East+West"})).unwrap();
        let e = err(&mut s, json!({"field": "Region", "name": "Twice", "formula": "=Pair*2"}));
        assert!(e.contains("Pair"), "{e}");
        // Removing the field from the axes drops its calculated items.
        s.execute("pivot.removeField", json!({"field": "Region", "area": "rows"})).unwrap();
        assert!(pt(&s).calculated_items.is_empty());
        // A change of source keeps them when the field and its items are still there.
        s.execute("pivot.addField", json!({"field": "Region", "area": "rows"})).unwrap();
        s.execute("pivot.calculatedItem", json!({"field": "Region", "name": "Pair", "formula": "=East+West"})).unwrap();
        s.execute("pivot.changeSource", json!({"source": "Sheet1!A1:E13"})).unwrap();
        assert_eq!(pt(&s).calculated_items.len(), 1);
        // Moving the field to the other axis keeps the item (Excel keeps it too).
        s.execute("pivot.moveField", json!({"field": "Region", "from": "rows", "to": "columns"})).unwrap();
        assert_eq!(pt(&s).calculated_items.len(), 1);
        s.execute("pivot.moveField", json!({"field": "Region", "from": "columns", "to": "rows"})).unwrap();
        assert_eq!(pt(&s).calculated_items.len(), 1);
        s.execute("pivot.removeField", json!({"field": "Region"})).unwrap();
        assert!(pt(&s).calculated_items.is_empty());
    }

    #[test]
    fn calculated_item_on_a_column_field() {
        let mut s = sales();
        s.execute("insert.pivotTable", json!({"columns": ["Product"], "rows": ["Region"], "values": ["Sales"]})).unwrap();
        s.execute("pivot.calculatedItem", json!({"field": "Product", "name": "Fruit", "formula": "=Apples+Pears"})).unwrap();
        let p = pt(&s);
        let r = p.last_range.unwrap();
        // The header spells the calculated item out and its column sums both.
        let mut found = None;
        for row in r.start.row..=r.start.row.saturating_add(3) {
            for c in r.start.col..=r.end.col {
                if val(&s, &CellRef::new(row, c).a1()).display() == "Fruit" {
                    found = Some((row, c));
                }
            }
        }
        let (_, c) = found.expect("the calculated item's column");
        let east = find_row(&s, "East").unwrap();
        assert_eq!(val(&s, &CellRef::new(east, c).a1()), Value::Number(160.0 + 195.0));
        // Grand total for the column is the source total (Apples 420 + Pears 270).
        let gt = find_row(&s, "Grand Total").unwrap();
        assert_eq!(val(&s, &CellRef::new(gt, c).a1()), Value::Number(420.0 + 270.0));
    }

    #[test]
    fn replace_protection() {
        let mut s = sales();
        s.execute("cell.set", json!({"cell": "H5", "input": "keep me"})).unwrap();
        let e = s.execute("insert.pivotTable", json!({"destination": "Sheet1!H3", "rows": ["Region"], "values": ["Sales"]})).unwrap_err().to_string();
        assert!(e.contains("There's already data in Sheet1!H5"), "{e}");
        assert_eq!(s.doc().unwrap().wb.active().unwrap().pivots.len(), 0);
        // Growing into data is refused as well.
        s.execute("insert.pivotTable", json!({"destination": "Sheet1!H3"})).unwrap();
        assert!(s.execute("pivot.addField", json!({"field": "Region", "area": "rows"})).is_err());
        s.execute("pivot.addField", json!({"field": "Region", "area": "rows", "replace": true})).unwrap();
        assert_eq!(text(&s, "H5"), "North");
        // A second pivot can't overlap the first.
        let e =
            s.execute("insert.pivotTable", json!({"source": "Sheet1!A1:E13", "destination": "Sheet1!H6", "replace": true})).unwrap_err().to_string();
        assert!(e.contains("overlap"), "{e}");
        // Nor its own source data.
        let e =
            s.execute("insert.pivotTable", json!({"source": "Sheet1!A1:E13", "destination": "Sheet1!B2", "replace": true})).unwrap_err().to_string();
        assert!(e.contains("source data"), "{e}");
        // Delete clears the report.
        s.execute("pivot.delete", json!({})).unwrap();
        assert_eq!(val(&s, "H5"), Value::Empty);
        assert!(s.doc().unwrap().wb.active().unwrap().pivots.is_empty());
    }

    #[test]
    fn fields_move_remove_sort() {
        let mut s = sales();
        s.execute("insert.pivotTable", json!({"rows": ["Region"], "values": ["Sales"]})).unwrap();
        let f = s.execute("pivot.fields", json!({})).unwrap();
        assert_eq!(f["fields"][0]["name"], "Region");
        assert_eq!(f["fields"][0]["samples"], json!(["East", "North", "West"]));
        assert_eq!(f["fields"][0]["areas"], json!(["rows"]));
        assert_eq!(f["fields"][4]["numeric"], true);
        assert_eq!(f["fields"][2]["date"], true);
        s.execute("pivot.sort", json!({"field": "Region", "order": "desc"})).unwrap();
        assert_eq!(text(&s, "A4"), "West");
        s.execute("pivot.moveField", json!({"field": "Region", "from": "rows", "to": "columns"})).unwrap();
        assert_eq!(text(&s, "B4"), "West");
        s.execute("pivot.removeField", json!({"field": "Sum of Sales", "area": "values"})).unwrap();
        s.execute("pivot.addField", json!({"field": "Region", "area": "values"})).unwrap();
        assert_eq!(pt(&s).values[0].name, "Count of Region");
        s.execute("pivot.removeField", json!({"field": "Region"})).unwrap();
        assert!(pt(&s).columns.is_empty() && pt(&s).values.is_empty());
        assert!(text(&s, "A3").contains("choose fields"));
        assert!(s.execute("pivot.addField", json!({"field": "Nope", "area": "rows"})).is_err());
        assert!(s.execute("pivot.addField", json!({"field": "Region", "area": "sideways"})).is_err());
    }

    #[test]
    fn recommended_and_summarize() {
        let mut s = sales();
        let r = s.execute("insert.recommendedPivotTables", json!({})).unwrap();
        let list = r["suggestions"].as_array().unwrap();
        assert!((3..=5).contains(&list.len()), "{list:?}");
        assert_eq!(list[0]["title"], "Sum of Units by Region");
        let r = s.execute("insert.recommendedPivotTables", json!({"apply": 2})).unwrap();
        assert!(r["suggestion"]["title"].as_str().unwrap().contains("quarters"));
        assert_eq!(text(&s, "A4"), "Qtr1");
        // From a table.
        s.execute("sheet.activate", json!({"sheet": "Sheet1"})).unwrap();
        s.execute("insert.table", json!({"range": "A1:E13"})).unwrap();
        s.execute("selection.set", json!({"range": "B2"})).unwrap();
        let r = s.execute("table.summarizePivot", json!({"rows": ["Product"], "values": ["Units"]})).unwrap();
        assert_eq!(pt(&s).source, "Table1");
        assert_eq!(r["name"], "PivotTable2");
        let gt = find_row(&s, "Grand Total").unwrap();
        assert_eq!(val(&s, &CellRef::new(gt, 1).a1()), Value::Number(72.0));
    }

    #[test]
    fn repeated_headers_get_numbered_names() {
        let mut s = Session::new();
        s.new_workbook();
        let mut header = vec![json!("a"), json!("A"), json!("a"), json!("a2")];
        // Thousands of equal headers (a row of check boxes, say) used to take cubic time; this would
        // hang rather than fail a wall-clock check, which is flaky on a loaded machine.
        header.extend((0..3000).map(|_| json!("x")));
        let data: Vec<_> = (0..header.len()).map(|i| json!(i)).collect();
        let end = CellRef::new(1, header.len() as u32 - 1).a1();
        s.execute("range.setValues", json!({"range": "A1", "values": [header, data]})).unwrap();
        let src = crate::pivot::read_source(&s.doc().unwrap().wb, &format!("Sheet1!A1:{end}"), 0).unwrap();
        assert_eq!(&src.headers[..4], ["a", "A2", "a3", "a22"]);
        assert_eq!(src.headers[4], "x");
        assert_eq!(src.headers.last().map(String::as_str), Some("x3000"));
        // Table column names follow the same rule.
        s.execute("insert.table", json!({"range": format!("A1:{end}"), "header": true})).unwrap();
        let text = |s: &mut Session, a: &str| s.execute("cell.get", json!({"cell": a})).unwrap()["text"].clone();
        assert_eq!(text(&mut s, "B1"), "A2");
        let last = CellRef::new(0, 3003).a1();
        assert_eq!(text(&mut s, &last), "x3000");
    }

    #[test]
    fn hostile_params_never_panic() {
        let mut s = sales();
        let cmds = [
            "insert.pivotTable",
            "insert.recommendedPivotTables",
            "table.summarizePivot",
            "pivot.fields",
            "pivot.fieldList",
            "pivot.list",
            "pivot.addField",
            "pivot.removeField",
            "pivot.moveField",
            "pivot.valueSettings",
            "pivot.filter",
            "pivot.sort",
            "pivot.group",
            "pivot.collapse",
            "pivot.layout",
            "pivot.grandTotals",
            "pivot.subtotals",
            "pivot.reportLayout",
            "pivot.style",
            "pivot.refresh",
            "data.refreshAll",
            "pivot.changeSource",
            "pivot.delete",
        ];
        let hostile = [
            json!({}),
            json!([1, 2]),
            json!({"pivot": 99999, "field": 5, "area": null}),
            json!({"source": "ZZZ999999999", "destination": "Nope!A1"}),
            json!({"source": "A:XFD", "destination": "Sheet1!XFD1048576"}),
            json!({"source": "''!A1", "destination": "!"}),
            json!({"field": "Sales", "area": "values", "func": "bogus", "position": 4294967295u64}),
            json!({"field": "Region", "selected": [null, 1, {"x": 1}], "index": 18446744073709551615u64, "showAs": "rank"}),
            json!({"apply": 99, "layout": "weird", "subtotals": 7, "style": "x".repeat(500)}),
            json!({"field": "Date", "by": "decades", "items": "nope", "order": "sideways"}),
        ];
        for c in cmds {
            for h in &hostile {
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let spec = crate::find_command(c).unwrap();
                    let _ = (spec.run)(&mut s, h);
                }));
                assert!(r.is_ok(), "{c} panicked on {h}");
            }
        }
        // And on a real pivot.
        s.execute("insert.pivotTable", json!({"source": "Sheet1!A1:E13", "rows": ["Region"], "values": ["Sales"]})).unwrap();
        for c in cmds {
            for h in &hostile {
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let spec = crate::find_command(c).unwrap();
                    let _ = (spec.run)(&mut s, h);
                }));
                assert!(r.is_ok(), "{c} panicked on {h}");
            }
        }
    }

    #[test]
    fn empty_header_is_refused() {
        let mut s = Session::new();
        s.new_workbook();
        s.execute("range.setValues", json!({"range": "A1", "values": [["Region", ""], ["East", 1]]})).unwrap();
        let e = s.execute("insert.pivotTable", json!({"source": "A1:B2"})).unwrap_err().to_string();
        assert!(e.contains("field name is not valid"), "{e}");
        assert!(s.execute("insert.pivotTable", json!({"source": "NoSuchTable"})).is_err());
    }

    #[test]
    fn too_many_value_columns_are_refused() {
        let mut s = Session::new();
        s.new_workbook();
        let mut records: Vec<Json> = vec![json!(["Cat", "Sub", "V"])];
        for r in 0..64 {
            for c in 0..64 {
                records.push(json!([format!("c{r}"), format!("s{c}"), 1.0]));
            }
        }
        s.execute("range.setValues", json!({"range": "A1", "values": records})).unwrap();
        let req = |n: usize| {
            let values: Vec<Json> = (0..n).map(|_| json!({"field": "V"})).collect();
            json!({"source": "A1:C4097", "rows": ["Cat"], "columns": ["Sub"], "values": values})
        };
        let e = s.execute("insert.pivotTable", req(256)).unwrap_err().to_string();
        assert!(e.contains("will not fit"), "{e}");
        assert!(s.execute("insert.pivotTable", req(128)).is_ok());
    }

    #[test]
    fn collapsed_column_items_count_as_one_column_group() {
        let mut s = Session::new();
        s.new_workbook();
        let mut records: Vec<Json> = vec![json!(["Grp", "Sub", "V"])];
        records.extend((0..64).map(|c| json!(["g", format!("s{c}"), 1.0])));
        s.execute("range.setValues", json!({"range": "A1", "values": records})).unwrap();
        let values: Vec<Json> = (0..256).map(|_| json!({"field": "V"})).collect();
        s.execute("insert.pivotTable", json!({"source": "A1:C65", "columns": ["Grp"], "values": values})).unwrap();
        s.execute("pivot.collapse", json!({"field": "Grp"})).unwrap();
        // 64 Sub items under the collapsed "g" show as one column group: (1 + grand total) x 256 columns fit.
        s.execute("pivot.addField", json!({"field": "Sub", "area": "columns"})).unwrap();
    }

    #[test]
    fn inserting_or_deleting_lines_moves_the_pivot_with_its_cells() {
        let mut s = sales();
        s.execute("insert.pivotTable", json!({"source": "Sheet1!A1:E13", "destination": "Sheet1!H10", "rows": ["Region"], "values": ["Sales"]}))
            .unwrap();
        assert_eq!(pt(&s).last_range.unwrap().a1(), "H10:I14");
        // Rows above the report (and below the source) move the report only.
        s.execute("home.insertRows", json!({"rows": "16:17"})).unwrap();
        s.execute("home.insertRows", json!({"rows": "6:7"})).unwrap();
        assert_eq!(text(&s, "H12"), "Row Labels");
        let l = s.execute("pivot.list", json!({})).unwrap();
        assert_eq!(l[0]["anchor"], "H12");
        assert_eq!(l[0]["range"], "H12:I16");
        assert_eq!(l[0]["source"], "Sheet1!A1:E15");
        // A row above the source moves both.
        s.execute("home.insertRows", json!({"rows": "1:1"})).unwrap();
        let p = pt(&s);
        assert_eq!(p.source, "Sheet1!A2:E16");
        assert_eq!(p.anchor.a1(), "H13");
        s.execute("pivot.refresh", json!({})).unwrap();
        assert_eq!(pt(&s).last_range.unwrap().a1(), "H13:I17");
        assert_eq!(text(&s, "H13"), "Row Labels");
        assert_eq!(val(&s, "I17"), Value::Number(TOTAL));
        // Deleting a column left of the report moves it left.
        s.execute("home.deleteColumns", json!({"cols": "F:F"})).unwrap();
        let p = pt(&s);
        assert_eq!(p.anchor.a1(), "G13");
        assert_eq!(p.last_range.unwrap().a1(), "G13:H17");
        // Deleting rows above moves it up.
        s.execute("home.deleteRows", json!({"rows": "1:1"})).unwrap();
        assert_eq!(pt(&s).source, "Sheet1!A1:E15");
        assert_eq!(pt(&s).anchor.a1(), "G12");
        s.execute("pivot.refresh", json!({})).unwrap();
        assert_eq!(text(&s, "G12"), "Row Labels");
        assert_eq!(val(&s, "H16"), Value::Number(TOTAL));
    }

    #[test]
    fn inserting_rows_on_the_source_sheet_moves_another_sheets_pivot_source() {
        let mut s = sales();
        s.execute("insert.pivotTable", json!({"source": "Sheet1!A1:E13", "rows": ["Region"], "values": ["Sales"]})).unwrap();
        s.execute("sheet.activate", json!({"sheet": "Sheet1"})).unwrap();
        s.execute("home.insertRows", json!({"rows": "1:2"})).unwrap();
        let l = s.execute("pivot.list", json!({})).unwrap();
        assert_eq!(l[0]["source"], "Sheet1!A3:E15");
        assert_eq!(l[0]["anchor"], "A3");
        s.execute("pivot.refresh", json!({"pivot": "PivotTable1"})).unwrap();
    }

    // ------------------------------------------------------ Show Details / GetPivotData

    /// Region, Month, Product, Sales — 4 records, and a "Report" sheet to pivot onto.
    fn drill_session() -> Session {
        let mut s = Session::new();
        s.new_workbook();
        s.execute(
            "range.setValues",
            json!({"range": "A1", "values": [
                ["Region", "Month", "Product", "Sales"],
                ["East", "Jan", "Apples", 100.0],
                ["East", "Feb", "Pears", 150.0],
                ["West", "Jan", "Apples", 200.0],
                ["West", "Feb", "Pears", 250.0],
            ]}),
        )
        .unwrap();
        s.execute("selection.set", json!({"range": "A1"})).unwrap();
        s.execute("home.insertSheet", json!({"name": "Report"})).unwrap();
        s
    }

    /// The sheet names, in order.
    fn names(s: &Session) -> Vec<String> {
        s.doc().unwrap().wb.sheets.iter().map(|sh| sh.name.clone()).collect()
    }

    /// Every cell of a sheet's used range, as text.
    fn sheet_text(s: &Session, name: &str) -> Vec<Vec<String>> {
        let d = s.doc().unwrap();
        let Some(sh) = d.wb.sheet_index(name).and_then(|i| d.wb.sheet(i)) else { return vec![] };
        let Some(r) = sh.used_range() else { return vec![] };
        (r.start.row..=r.end.row).map(|row| (r.start.col..=r.end.col).map(|col| sh.value(CellRef::new(row, col)).display()).collect()).collect()
    }

    /// A cell of the pivot's own sheet, whatever the active sheet is.
    fn report_val(s: &Session, cell: &str) -> Value {
        let d = s.doc().unwrap();
        let sh = d.wb.sheet_index("Report").and_then(|i| d.wb.sheet(i)).unwrap();
        sh.value(CellRef::parse(cell).unwrap())
    }

    /// The report row whose first label cell reads `label`.
    fn report_row(s: &Session, label: &str) -> u32 {
        let d = s.doc().unwrap();
        let sh = d.wb.sheet_index("Report").and_then(|i| d.wb.sheet(i)).unwrap();
        let r = sh.pivots.first().and_then(|p| p.last_range).unwrap();
        let row = (r.start.row..=r.end.row).find(|row| sh.value(CellRef::new(*row, r.start.col)).display() == label);
        row.unwrap()
    }

    #[test]
    fn show_details_writes_the_source_rows_behind_a_value_cell() {
        let mut s = drill_session();
        s.execute(
            "insert.pivotTable",
            json!({"source": "Sheet1!A1:D5", "destination": "Report!A3", "name": "P1", "rows": ["Region"], "columns": ["Month"], "values": ["Sales"]}),
        )
        .unwrap();
        // The items sort by their labels (our default is ascending), so Feb comes before Jan.
        assert_eq!(report_val(&s, "A5"), Value::text("East"));
        assert_eq!(report_val(&s, "B5"), Value::Number(150.0));
        let r = s.execute("pivot.showDetails", json!({"cell": "B5"})).unwrap();
        assert_eq!(r["name"], "East - Feb");
        assert_eq!(r["rows"], 1);
        assert_eq!(r["cells"], 8);
        // The new sheet follows the pivot's, the source's header row first, then the record.
        assert_eq!(names(&s), ["Sheet1", "Report", "East - Feb"]);
        assert_eq!(
            sheet_text(&s, "East - Feb"),
            vec![
                vec!["Region".to_string(), "Month".into(), "Product".into(), "Sales".into()],
                vec!["East".into(), "Feb".into(), "Pears".into(), "150".into()],
            ]
        );
        // The new sheet is active now, so a second call names its PivotTable.
        // A label cell is not a value cell: the command says so instead of writing a guess.
        let e = s.execute("pivot.showDetails", json!({"pivot": "P1", "cell": "A5"})).unwrap_err().to_string();
        assert!(e.contains("value cell"), "{e}");
        let e = s.execute("pivot.showDetails", json!({"pivot": "P1", "cell": "A1"})).unwrap_err().to_string();
        assert!(e.contains("outside"), "{e}");
    }

    #[test]
    fn show_details_emits_a_subtotal_group_and_the_records_a_filter_passes() {
        let mut s = drill_session();
        s.execute(
            "insert.pivotTable",
            json!({"source": "Sheet1!A1:D5", "destination": "Report!A3", "name": "P1", "rows": ["Region", "Month"], "values": ["Sales"]}),
        )
        .unwrap();
        s.execute("pivot.subtotals", json!({"pivot": "P1", "mode": "bottom"})).unwrap();
        let total = report_row(&s, "East Total");
        let cell = CellRef::new(total, 1).a1();
        assert_eq!(report_val(&s, &cell), Value::Number(250.0));
        let r = s.execute("pivot.showDetails", json!({"cell": cell.clone()})).unwrap();
        assert_eq!(r["name"], "East");
        assert_eq!(r["rows"], 2, "the subtotal holds East's two records");
        assert_eq!(
            sheet_text(&s, "East")[1..],
            [
                vec!["East".to_string(), "Jan".into(), "Apples".into(), "100".into()],
                vec!["East".to_string(), "Feb".into(), "Pears".into(), "150".into()]
            ]
        );
        // A page filter narrows the grand total to the records it passes.
        s.execute("pivot.filter", json!({"pivot": "P1", "field": "Product", "selected": ["Apples"]})).unwrap();
        s.execute("pivot.refresh", json!({"pivot": "P1"})).unwrap();
        let grand = report_row(&s, "Grand Total");
        let cell = CellRef::new(grand, 1).a1();
        assert_eq!(report_val(&s, &cell), Value::Number(300.0));
        let r = s.execute("pivot.showDetails", json!({"pivot": "P1", "cell": cell})).unwrap();
        assert_eq!(r["name"], "Details");
        assert_eq!(r["rows"], 2);
        assert_eq!(
            sheet_text(&s, "Details")[1..],
            [
                vec!["East".to_string(), "Jan".into(), "Apples".into(), "100".into()],
                vec!["West".to_string(), "Jan".into(), "Apples".into(), "200".into()]
            ]
        );
    }

    #[test]
    fn a_show_details_sheet_is_capped_by_cells() {
        let mut s = drill_session();
        s.execute(
            "insert.pivotTable",
            json!({"source": "Sheet1!A1:D5", "destination": "Report!A3", "name": "P1", "rows": ["Region"], "values": ["Sales"]}),
        )
        .unwrap();
        let cell = CellRef::new(report_row(&s, "Grand Total"), 1);
        let d = s.doc().unwrap();
        let pt = d.wb.sheet_index("Report").and_then(|i| d.wb.sheet(i)).and_then(|sh| sh.pivots.first()).unwrap();
        // The cap counts cells: an eight-cell sheet over four columns holds two records.
        let detail = crate::pivot::details_with_cap(&d.wb, pt, d.wb.sheet_index("Report").unwrap(), cell, 8).unwrap();
        assert_eq!(detail.rows.len(), 2);
        assert_eq!(detail.skipped, 2);
        // The command's own cap: 12,000,000 cells, and never more rows than a sheet has.
        assert_eq!(crate::pivot::detail_row_cap(4, crate::pivot::MAX_DETAIL_CELLS), gridcraft_core::MAX_ROWS as u64 - 1);
        assert_eq!(crate::pivot::detail_row_cap(160, crate::pivot::MAX_DETAIL_CELLS), 75_000);
    }

    #[test]
    fn show_details_is_one_undo_step_and_does_not_reuse_a_name() {
        let mut s = drill_session();
        s.execute(
            "insert.pivotTable",
            json!({"source": "Sheet1!A1:D5", "destination": "Report!A3", "name": "P1", "rows": ["Region"], "columns": ["Month"], "values": ["Sales"]}),
        )
        .unwrap();
        s.execute("pivot.showDetails", json!({"cell": "B5"})).unwrap();
        assert_eq!(names(&s), ["Sheet1", "Report", "East - Feb"]);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(names(&s), ["Sheet1", "Report"]);
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(names(&s), ["Sheet1", "Report", "East - Feb"]);
        // The sheet name is taken now: the next one gets a suffix.
        s.execute("pivot.showDetails", json!({"pivot": "P1", "cell": "B5"})).unwrap();
        assert_eq!(names(&s), ["Sheet1", "Report", "East - Feb (2)", "East - Feb"]);
    }

    /// Sets `formula` on Sheet1!F1 and returns what it evaluated to.
    fn gpd(s: &mut Session, formula: &str) -> Value {
        s.execute("cell.set", json!({"sheet": "Sheet1", "cell": "F1", "input": format!("={formula}")})).unwrap();
        let d = s.doc().unwrap();
        let sh = d.wb.sheet_index("Sheet1").and_then(|i| d.wb.sheet(i)).unwrap();
        sh.value(CellRef::parse("F1").unwrap())
    }

    #[test]
    fn a_click_on_a_report_value_composes_a_getpivotdata_that_shows_the_same_number() {
        let mut s = drill_session();
        s.execute(
            "insert.pivotTable",
            json!({"source": "Sheet1!A1:D5", "destination": "Report!A3", "name": "P1", "rows": ["Region"], "columns": ["Month"], "values": ["Sales"]}),
        )
        .unwrap();
        let formula = |s: &Session, cell: &str| -> String {
            let d = s.doc().unwrap();
            crate::pivot::get_pivot_data(&d.wb, d.wb.sheet_index("Report").unwrap(), CellRef::parse(cell).unwrap()).unwrap()
        };
        // The pairs are the labels the report draws.
        assert_eq!(formula(&s, "B5"), "GETPIVOTDATA(\"Sum of Sales\", Report!$A$3, \"Region\", \"East\", \"Month\", \"Feb\")");
        // A leaf, a grand-total column, a grand-total row and the corner: same number, through calc.
        for cell in ["B5", "C6", "D5", "B7", "D7"] {
            let f = formula(&s, cell);
            assert_eq!(gpd(&mut s, &f), report_val(&s, cell), "{cell}: {f}");
        }
        // With the option off the click writes a plain reference instead.
        s.execute("pivot.getPivotData", json!({"pivot": "P1", "on": false})).unwrap();
        // The option is remembered in our own document format (the XLSX writer has no slot for
        // it, so that format brings back Excel's default, on).
        {
            let wb = (*s.doc().unwrap().wb).clone();
            let bytes = crate::io::save_bytes(&wb, "book.json").unwrap();
            let (back, _) = crate::io::open_bytes("book.json", &bytes).unwrap();
            assert!(!back.sheet(1).unwrap().pivots[0].generate_get_pivot_data);
        }
        {
            let d = s.doc().unwrap();
            let pt = d.wb.sheet_index("Report").and_then(|i| d.wb.sheet(i)).and_then(|sh| sh.pivots.first()).unwrap();
            assert!(!pt.generate_get_pivot_data);
            let si = d.wb.sheet_index("Report").unwrap();
            assert!(crate::pivot::click_reference(&d.wb, si, CellRef::parse("B5").unwrap()).is_none());
        }
        let r = s.execute("pivot.getPivotData", json!({"pivot": "P1", "on": true})).unwrap();
        assert_eq!(r["on"], true);
        // The getter answers what a click would insert (the plain reference is the UI's fallback).
        let r = s.execute("pivot.getPivotData", json!({"pivot": "P1", "cell": "B5"})).unwrap();
        assert_eq!(r["on"], true);
        assert_eq!(r["formula"], format!("={}", formula(&s, "B5")));
        {
            let d = s.doc().unwrap();
            let si = d.wb.sheet_index("Report").unwrap();
            // A label cell of the report is not a value cell.
            assert!(crate::pivot::click_reference(&d.wb, si, CellRef::parse("A5").unwrap()).is_none());
        }
        // A nested report: every value cell — a leaf, a subtotal, a grand total — composes a call
        // that evaluates to the number it shows.
        s.execute(
            "insert.pivotTable",
            json!({"source": "Sheet1!A1:D5", "destination": "Report!K3", "name": "P2", "rows": ["Region", "Month"], "columns": ["Product"], "values": ["Sales"]}),
        )
        .unwrap();
        s.execute("pivot.subtotals", json!({"pivot": "P2", "mode": "bottom"})).unwrap();
        let sweep = {
            let d = s.doc().unwrap();
            let si = d.wb.sheet_index("Report").unwrap();
            let sh = d.wb.sheet(si).unwrap();
            let pt = sh.pivots.iter().find(|p| p.name == "P2").unwrap();
            let area = pt.last_range.unwrap();
            let mut v = Vec::new();
            for row in area.start.row..=area.end.row {
                for col in area.start.col..=area.end.col {
                    let at = CellRef::new(row, col);
                    if let Some(f) = crate::pivot::get_pivot_data(&d.wb, si, at) {
                        v.push((at.a1(), f, sh.value(at)));
                    }
                }
            }
            v
        };
        let live: Vec<(String, String, Value)> = sweep.into_iter().filter(|(_, _, v)| !v.is_empty()).collect();
        assert!(live.len() >= 8, "a report with subtotals and totals has values: {live:?}");
        for (cell, f, shown) in &live {
            assert_eq!(&gpd(&mut s, f), shown, "{cell}: {f}");
        }
    }

    #[test]
    fn a_show_details_sheet_survives_a_save_and_reopen() {
        let mut s = drill_session();
        s.execute(
            "insert.pivotTable",
            json!({"source": "Sheet1!A1:D5", "destination": "Report!A3", "name": "P1", "rows": ["Region"], "columns": ["Month"], "values": ["Sales"]}),
        )
        .unwrap();
        s.execute("pivot.showDetails", json!({"cell": "B5"})).unwrap();
        let before = sheet_text(&s, "East - Feb");
        let wb = (*s.doc().unwrap().wb).clone();
        let bytes = crate::io::save_bytes(&wb, "book.xlsx").unwrap();
        let (back, _) = crate::io::open_bytes("book.xlsx", &bytes).unwrap();
        let i = back.sheet_index("East - Feb").unwrap();
        assert_eq!(i, 2, "the sheet stays right after the pivot's");
        let sh = back.sheet(i).unwrap();
        assert_eq!(sh.value(CellRef::new(0, 0)), Value::text("Region"));
        assert_eq!(sh.value(CellRef::new(1, 0)), Value::text("East"));
        assert_eq!(sh.value(CellRef::new(1, 3)), Value::Number(150.0));
        assert_eq!(before.len(), 2);
        assert_eq!(before[1][2], "Pears");
    }

    // ------------------------------------------------ reports GridCraft does not compute

    /// A report carried from a file: its cached values on the sheet, and a source GridCraft does
    /// not query (a data model). What the reader builds for a Power Pivot PivotTable.
    fn carried() -> Session {
        let mut s = sales();
        s.execute("insert.pivotTable", json!({"rows": ["Region"], "values": ["Sales"]})).unwrap();
        {
            let d = s.doc_mut().unwrap();
            let si = d.wb.active_sheet;
            let wb = std::sync::Arc::make_mut(&mut d.wb);
            let sh = std::sync::Arc::make_mut(&mut wb.sheets[si]);
            let p = &mut sh.pivots[0];
            p.source = String::new();
            p.source_kind = gridcraft_model::PivotSourceKind::DataModel;
            p.read_only_reason = Some("built on the workbook's data model (Power Pivot), which GridCraft keeps but does not query".into());
        }
        s
    }

    /// Every cell of the report, as the file wrote them.
    fn report_cells(s: &Session) -> Vec<(String, String)> {
        let p = pt(s);
        let Some(r) = p.last_range else { return vec![] };
        let mut v = vec![];
        for row in r.start.row..=r.end.row {
            for col in r.start.col..=r.end.col {
                let a = CellRef::new(row, col).a1();
                v.push((a.clone(), val(s, &a).display()));
            }
        }
        v
    }

    #[test]
    fn a_carried_report_refuses_every_command_that_would_recompute_it() {
        let mut s = carried();
        let before = report_cells(&s);
        assert!(before.iter().any(|(a, v)| a == "B4" && v == "415"), "{before:?}");
        let attempts = [
            ("pivot.refresh", json!({})),
            ("pivot.addField", json!({"field": "Product", "area": "rows"})),
            ("pivot.removeField", json!({"field": "Region"})),
            ("pivot.moveField", json!({"field": "Region", "from": "rows", "to": "columns"})),
            ("pivot.group", json!({"field": "Region", "by": "years"})),
            ("pivot.filter", json!({"field": "Region", "selected": ["East"]})),
            ("pivot.sort", json!({"field": "Region", "order": "desc"})),
            ("pivot.collapse", json!({"field": "Region"})),
            ("pivot.layout", json!({"layout": "tabular"})),
            ("pivot.grandTotals", json!({"rows": false})),
            ("pivot.valueSettings", json!({"index": 0, "func": "average"})),
            ("pivot.changeSource", json!({"source": "Sheet1!A1:E13"})),
            ("pivot.showDetails", json!({"cell": "B4"})),
            ("pivot.calculatedField", json!({"name": "Double", "formula": "=Sales*2"})),
            ("pivot.calculatedItem", json!({"field": "Region", "name": "Both", "formula": "=East+West"})),
        ];
        for (cmd, params) in attempts {
            let e = s.execute(cmd, params).unwrap_err().to_string();
            assert!(e.contains("read-only"), "{cmd}: {e}");
            assert!(e.contains("data model"), "{cmd} names the reason: {e}");
            assert!(e.contains("PivotTable1"), "{cmd} names the report: {e}");
        }
        // The cached values are still exactly the file's: nothing was recomputed over a source
        // that isn't there, and nothing was reshaped.
        assert_eq!(report_cells(&s), before);
        assert_eq!(pt(&s).read_only_text(), "built on the workbook's data model (Power Pivot), which GridCraft keeps but does not query");
    }

    #[test]
    fn the_fields_of_a_carried_report_say_why_it_is_read_only() {
        let mut s = carried();
        let r = s.execute("pivot.fields", json!({})).unwrap();
        assert_eq!(r["name"], "PivotTable1");
        assert_eq!(r["kind"], "dataModel");
        assert_eq!(r["readOnly"], true);
        let why = r["readOnlyReason"].as_str().unwrap_or_default();
        assert!(why.contains("data model"), "{why}");
        assert_eq!(r["source"], "", "there is no range to name");
        let fields = r["fields"].as_array().cloned().unwrap_or_default();
        let region = fields.iter().find(|f| f["name"] == "Region").unwrap_or_else(|| panic!("{r}"));
        assert_eq!(region["areas"], json!(["rows"]));
        let sales = fields.iter().find(|f| f["name"] == "Sales").unwrap_or_else(|| panic!("{r}"));
        assert_eq!(sales["areas"], json!(["values"]));
        // No source was read, so there is no record count and no error to report.
        assert!(r.get("records").is_none(), "{r}");
        assert!(r.get("error").is_none(), "{r}");
        // The list command says the same thing about it.
        let list = s.execute("pivot.list", json!({})).unwrap();
        assert_eq!(list[0]["kind"], "dataModel");
        assert_eq!(list[0]["readOnly"], true);
    }

    #[test]
    fn refresh_all_leaves_a_carried_report_as_it_is() {
        let mut s = carried();
        let before = report_cells(&s);
        let r = s.execute("data.refreshAll", json!({})).unwrap();
        assert_eq!(r["refreshed"], 0);
        assert_eq!(r["skipped"][0]["pivot"], "PivotTable1");
        assert!(r["skipped"][0]["reason"].as_str().unwrap_or_default().contains("data model"), "{r}");
        assert_eq!(report_cells(&s), before);
    }
}
