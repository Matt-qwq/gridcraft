//! PivotTables: reading the source list, grouping and aggregating, laying out the report and
//! writing it onto the sheet as static, styled cells.
//!
//! Behaviour follows Excel's documented PivotTable model (rows/columns/values/filters areas,
//! Compact/Outline/Tabular layouts, subtotals, grand totals, show-values-as); the code and the
//! look (our own palette) are original.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use gridcraft_core::date::datetime_from_serial;
pub use gridcraft_core::quote_sheet;
use gridcraft_core::{CellError, CellRef, MAX_COLS, MAX_ROWS, RangeRef, Value, split_sheet};
use gridcraft_model::pivot_item;
use gridcraft_model::*;

use crate::pivot_calc::CalcSet;

/// Largest source block read (cells).
pub const MAX_SOURCE_CELLS: u64 = 10_000_000;
/// Largest report written (cells).
pub const MAX_OUTPUT_CELLS: u64 = 2_000_000;
/// Most fields in the Rows plus Columns areas.
pub const MAX_AXIS_FIELDS: usize = 32;
/// Most fields in the Values area.
pub const MAX_VALUE_FIELDS: usize = 256;
/// Most calculated items per PivotTable.
pub const MAX_CALC_ITEMS: usize = 256;
/// Most report paths a calculated item may add to an axis (a hostile document must not ask for
/// an unbounded report).
const MAX_CALC_PATHS: usize = 200_000;
/// Deepest chain of calculated items a lookup follows (they may not name each other, but a hand-
/// written document could still nest them).
const MAX_CALC_DEPTH: u32 = 8;

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

// ---------------------------------------------------------------- source

/// The source list: header names, per-column number formats and the records.
#[derive(Clone, Debug)]
pub struct Source {
    pub sheet: usize,
    pub range: RangeRef,
    pub headers: Vec<String>,
    pub formats: Vec<String>,
    /// Every non-blank value in the column is a number.
    pub numeric: Vec<bool>,
    /// Numeric with a date number format.
    pub dates: Vec<bool>,
    pub rows: Vec<Vec<Value>>,
}

impl Source {
    pub fn col(&self, name: &str) -> Option<usize> {
        self.headers.iter().position(|h| h.eq_ignore_ascii_case(name))
    }
}

/// Absolute reference text for a range on a sheet: `Sheet1!$A$1:$E$20`.
pub fn range_text(sheet_name: &str, r: RangeRef) -> String {
    let abs = |c: CellRef| format!("${}${}", gridcraft_core::col_to_letters(c.col), c.row as u64 + 1);
    format!("{}!{}:{}", quote_sheet(sheet_name), abs(r.start), abs(r.end))
}

/// Resolves source text (range, table name or defined name) to (sheet, range including header).
pub fn resolve_source(wb: &Workbook, text: &str, default_sheet: usize) -> Result<(usize, RangeRef), String> {
    resolve_inner(wb, text, default_sheet, 0)
}

fn resolve_inner(wb: &Workbook, text: &str, default_sheet: usize, depth: u32) -> Result<(usize, RangeRef), String> {
    const INVALID: &str = "The reference isn't valid.";
    let t = text.trim().trim_start_matches('=').trim();
    if t.is_empty() || depth > 4 {
        return Err(INVALID.into());
    }
    if let Some((si, ti)) = wb.table(t)
        && let Some(tb) = wb.sheet(si).and_then(|s| s.tables.get(ti))
    {
        if !tb.header_row {
            return Err("The table needs a header row to be used as a PivotTable source.".into());
        }
        let end_row = tb.range.end.row.saturating_sub(tb.totals_row as u32).max(tb.range.start.row);
        return Ok((si, RangeRef::new(tb.range.start, CellRef::new(end_row, tb.range.end.col))));
    }
    let (sheet, body) = split_sheet(t);
    if let Some(r) = RangeRef::parse(body) {
        let si = match sheet {
            Some(n) => wb.sheet_index(&n).ok_or_else(|| INVALID.to_string())?,
            None => default_sheet,
        };
        let sh = wb.sheet(si).ok_or_else(|| INVALID.to_string())?;
        let r = if r.is_full_cols() || r.is_full_rows() {
            let used = sh.used_range().ok_or_else(|| INVALID.to_string())?;
            r.intersection(&used).ok_or_else(|| INVALID.to_string())?
        } else {
            r
        };
        return Ok((si, r));
    }
    if sheet.is_none()
        && let Some(n) = wb.name(t, default_sheet)
    {
        let f = n.formula.clone();
        return resolve_inner(wb, &f, default_sheet, depth + 1);
    }
    Err(INVALID.into())
}

const FIELD_NAME_ERR: &str =
    "The PivotTable field name is not valid. To create a PivotTable, you must use data that is organized as a list with labeled columns.";

/// Reads the source list.
pub fn read_source(wb: &Workbook, text: &str, default_sheet: usize) -> Result<Source, String> {
    let (si, r) = resolve_source(wb, text, default_sheet)?;
    let sh = wb.sheet(si).ok_or("The reference isn't valid.")?;
    if r.count() > MAX_SOURCE_CELLS {
        return Err("The source range is too large for a PivotTable.".into());
    }
    let mut headers: Vec<String> = Vec::new();
    // Names taken so far (lower-case), and the next suffix to try for each repeated header, so
    // that thousands of equal headers don't take cubic time.
    let mut taken: HashSet<String> = HashSet::new();
    let mut next_suffix: HashMap<String, usize> = HashMap::new();
    let mut formats = Vec::new();
    for c in r.start.col..=r.end.col {
        let h = crate::display::cell_text(wb, sh, CellRef::new(r.start.row, c)).trim().to_string();
        if h.is_empty() {
            return Err(FIELD_NAME_ERR.into());
        }
        let key = h.to_ascii_lowercase();
        let mut n = h.clone();
        let mut k = next_suffix.get(&key).copied().unwrap_or(2);
        while taken.contains(&n.to_ascii_lowercase()) {
            n = format!("{h}{k}");
            k += 1;
        }
        next_suffix.insert(key, k);
        taken.insert(n.to_ascii_lowercase());
        headers.push(n);
        let fmt = if r.end.row > r.start.row {
            wb.styles.get(sh.style_id(CellRef::new(r.start.row + 1, c))).num_fmt.as_str().to_string()
        } else {
            "General".into()
        };
        formats.push(fmt);
    }
    let w = headers.len();
    let mut rows = Vec::new();
    let mut numeric = vec![true; w];
    let mut any = vec![false; w];
    if r.end.row > r.start.row {
        for row in r.start.row + 1..=r.end.row {
            let rec: Vec<Value> = (r.start.col..=r.end.col).map(|c| sh.value(CellRef::new(row, c)).scalar()).collect();
            if rec.iter().all(Value::is_empty) {
                continue;
            }
            for (i, v) in rec.iter().enumerate() {
                if !v.is_empty() {
                    if let Some(a) = any.get_mut(i) {
                        *a = true;
                    }
                    if !v.is_number()
                        && let Some(n) = numeric.get_mut(i)
                    {
                        *n = false;
                    }
                }
            }
            rows.push(rec);
        }
    }
    for (n, a) in numeric.iter_mut().zip(any.iter()) {
        *n = *n && *a;
    }
    let dates = numeric.iter().zip(formats.iter()).map(|(n, f)| *n && crate::display::number_format(f).is_date()).collect();
    Ok(Source { sheet: si, range: r, headers, formats, numeric, dates, rows })
}

// ---------------------------------------------------------------- items

#[derive(Clone, Debug)]
struct Item {
    value: Value,
    label: String,
    class: u8,
    num: f64,
    text: String,
    first: usize,
    /// The value is a number shown with the source format.
    formatted: bool,
}

#[derive(Default)]
struct FieldItems {
    items: Vec<Item>,
    index: HashMap<String, u32>,
}

fn cmp_items(a: &Item, b: &Item) -> Ordering {
    a.class.cmp(&b.class).then(a.num.total_cmp(&b.num)).then_with(|| a.text.cmp(&b.text))
}

/// How a field buckets its values: a date part, or a numeric range.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Grouping {
    pub date: PivotDateGroup,
    pub number: Option<PivotNumberGroup>,
}

impl Grouping {
    /// The grouping a field on an axis uses (an invalid numeric group is ignored).
    pub fn of(f: &PivotField) -> Grouping {
        Grouping { date: f.date_group, number: f.number_group.filter(PivotNumberGroup::is_valid) }
    }
    pub fn is_none(&self) -> bool {
        self.date == PivotDateGroup::None && self.number.is_none()
    }
}

/// (identity key, item) for a source value under a field's grouping.
fn make_item(wb: &Workbook, v: &Value, group: Grouping, fmt: &str, first: usize) -> (String, Item) {
    let it = |value: Value, label: String, class: u8, num: f64, formatted: bool| Item {
        text: label.to_lowercase(),
        value,
        label,
        class,
        num,
        first,
        formatted,
    };
    match v {
        Value::Empty => ("b".into(), it(Value::text("(blank)"), "(blank)".into(), 4, 0.0, false)),
        Value::Number(n) => {
            if let Some(ng) = group.number
                && let Some((label, key)) = ng.bucket(*n)
            {
                return (format!("r{}", label.to_lowercase()), it(Value::text(label.as_str()), label, 0, key, false));
            }
            if group.date != PivotDateGroup::None
                && let Some(dt) = datetime_from_serial(wb.date_system, *n)
            {
                let mon = MONTHS.get(dt.month.saturating_sub(1) as usize).copied().unwrap_or("Jan");
                let (label, key) = match group.date {
                    PivotDateGroup::Years => (dt.year.to_string(), dt.year as f64),
                    PivotDateGroup::Quarters => {
                        let q = (dt.month.saturating_sub(1)) / 3 + 1;
                        (format!("Qtr{q}"), q as f64)
                    }
                    PivotDateGroup::Months => (mon.to_string(), dt.month as f64),
                    _ => (format!("{}-{mon}", dt.day), (dt.month * 100 + dt.day) as f64),
                };
                return (format!("g{key}"), it(Value::text(label.as_str()), label, 0, key, false));
            }
            let label = crate::display::format(v, fmt, wb).text;
            (format!("n{}", n.to_bits()), it(v.clone(), label, 0, *n, true))
        }
        Value::Text(t) => (format!("t{}", t.to_lowercase()), it(v.clone(), t.to_string(), 1, 0.0, false)),
        Value::Bool(b) => {
            let l = if *b { "TRUE" } else { "FALSE" };
            (format!("l{l}"), it(Value::Bool(*b), l.into(), 2, *b as u8 as f64, false))
        }
        Value::Error(e) => (format!("e{}", e.as_str()), it(v.clone(), e.as_str().into(), 3, 0.0, false)),
        Value::Array(_) => make_item(wb, &v.scalar(), group, fmt, first),
    }
}

impl FieldItems {
    fn intern(&mut self, wb: &Workbook, v: &Value, group: Grouping, fmt: &str, first: usize) -> u32 {
        let (key, item) = make_item(wb, v, group, fmt, first);
        if let Some(i) = self.index.get(&key) {
            return *i;
        }
        let i = self.items.len() as u32;
        self.items.push(item);
        self.index.insert(key, i);
        i
    }
    /// Appends a calculated item's caption: it sorts after the real items (`class` 4) and never
    /// shares an identity key with a source value.
    fn push_calc(&mut self, label: &str) -> u32 {
        let i = self.items.len() as u32;
        let text = label.to_lowercase();
        self.items.push(Item { value: Value::text(label), label: label.to_string(), class: 4, num: 0.0, text, first: usize::MAX, formatted: false });
        i
    }
    fn get(&self, i: u32) -> Option<&Item> {
        self.items.get(i as usize)
    }
    fn label(&self, i: u32) -> String {
        self.get(i).map(|x| x.label.clone()).unwrap_or_default()
    }
}

/// The label of a source value under a date grouping (for filters and field lists).
pub fn item_label(wb: &Workbook, v: &Value, group: Grouping, fmt: &str) -> String {
    make_item(wb, v, group, fmt, 0).1.label
}

/// Whether a record is hidden by the report's page filters and item selections. A label or value
/// filter is evaluated when it is set and stored as the field's hidden items (the same check), so
/// it needs no separate pass here.
pub fn record_passes(wb: &Workbook, src: &Source, pt: &PivotTable, rec: &[Value]) -> bool {
    for f in &pt.filters {
        let Some(sel) = &f.selected else { continue };
        let Some(c) = src.col(&f.source_col) else { continue };
        let group =
            pt.rows.iter().chain(pt.columns.iter()).find(|x| x.source_col.eq_ignore_ascii_case(&f.source_col)).map(Grouping::of).unwrap_or_default();
        let fmt = src.formats.get(c).map(String::as_str).unwrap_or("General");
        let label = item_label(wb, rec.get(c).unwrap_or(&Value::Empty), group, fmt);
        if !sel.iter().any(|s| s.eq_ignore_ascii_case(&label)) {
            return false;
        }
    }
    true
}

/// The value field a filter names, as (source column, function). A calculated field has no source
/// column to aggregate yet.
pub fn value_measure(src: &Source, pt: &PivotTable, name: &str) -> Result<(usize, PivotFunc), String> {
    let v = match pt.values.iter().find(|v| v.name.eq_ignore_ascii_case(name)) {
        Some(v) => v,
        None => {
            let mut candidates = pt.values.iter().filter(|v| v.source_col.eq_ignore_ascii_case(name));
            let first = candidates.next();
            if candidates.next().is_some() {
                return Err(format!("`{name}` names more than one value field; give its caption."));
            }
            first.ok_or_else(|| format!("There's no value field `{name}` to filter by."))?
        }
    };
    let c = src.col(&v.source_col).ok_or_else(|| "Value filters on a calculated field aren't supported yet.".to_string())?;
    Ok((c, v.func))
}

/// The labels of `field` whose aggregate of the filter's measure passes, over the records the
/// report's other filters keep. The result is stored as the field's hidden items.
pub fn value_filter_kept(wb: &Workbook, src: &Source, pt: &PivotTable, vf: &PivotValueFilter) -> Result<Vec<String>, String> {
    let c = src.col(&vf.source_col).ok_or_else(|| format!("no field `{}` in the source data", vf.source_col))?;
    let axis = pt
        .rows
        .iter()
        .chain(pt.columns.iter())
        .find(|f| f.source_col.eq_ignore_ascii_case(&vf.source_col))
        .ok_or("Value filters apply to a row or column field.")?;
    let group = Grouping::of(axis);
    let fmt = src.formats.get(c).cloned().unwrap_or_else(|| "General".into());
    let (vc, func) = value_measure(src, pt, &vf.measure)?;
    let mut acc: HashMap<String, Acc> = HashMap::new();
    let mut order: Vec<String> = vec![];
    for rec in &src.rows {
        if !record_passes(wb, src, pt, rec) {
            continue;
        }
        let label = item_label(wb, rec.get(c).unwrap_or(&Value::Empty), group, &fmt);
        acc.entry(label.clone()).or_default().add(rec.get(vc).unwrap_or(&Value::Empty));
        if !order.iter().any(|x| x.eq_ignore_ascii_case(&label)) {
            order.push(label);
            if order.len() > 100_000 {
                break;
            }
        }
    }
    Ok(order.into_iter().filter(|l| acc.get(l).and_then(|a| a.result(func).as_f64()).is_some_and(|x| vf.passes(x))).collect())
}

/// Distinct item labels of a source column, sorted like the field list shows them.
pub fn distinct_labels(wb: &Workbook, src: &Source, col: usize, group: Grouping, limit: usize) -> Vec<String> {
    let mut fi = FieldItems::default();
    let fmt = src.formats.get(col).map(String::as_str).unwrap_or("General");
    for (i, r) in src.rows.iter().enumerate() {
        if let Some(v) = r.get(col) {
            fi.intern(wb, v, group, fmt, i);
        }
        if fi.items.len() > 100_000 {
            break;
        }
    }
    let mut items = fi.items;
    items.sort_by(cmp_items);
    items.into_iter().take(limit).map(|i| i.label).collect()
}

// ---------------------------------------------------------------- calculated items

/// A calculated item appended to a field on an axis, ready to look up.
struct CalcItem {
    /// The field's depth among the axis's fields, and the position it holds in a report path.
    depth: usize,
    /// Its item index in the field's items (after the real ones).
    item: u32,
    /// The arithmetic over the item indexes in `refs`.
    term: pivot_item::Term,
    /// The item index of each caption the formula names, in `Compiled::items()` order; `None`
    /// for a caption that isn't an item of the field (it evaluates to `#REF!`).
    refs: Vec<Option<u32>>,
}

/// Parses a PivotTable's calculated items against the axis's fields and appends each one to its
/// field's items. Every caption is resolved before any item is appended, so a formula cannot
/// name another calculated item — references never form a cycle.
fn plan_calc_items(calc_items: &[PivotCalcItem], fields: &[(usize, &PivotField)], items: &mut [FieldItems]) -> Vec<CalcItem> {
    let mut pending: Vec<(usize, String, pivot_item::Compiled, Vec<Option<u32>>)> = Vec::new();
    for ci in calc_items.iter().take(MAX_CALC_ITEMS) {
        let field = ci.field.trim();
        let Some(depth) = fields.iter().position(|(_, f)| f.source_col.eq_ignore_ascii_case(field)) else { continue };
        let Some((_, f)) = fields.get(depth) else { continue };
        // A grouped field's items are ranges or dates, not captions a formula can name.
        if !Grouping::of(f).is_none() {
            continue;
        }
        let name = ci.name.trim();
        if name.is_empty() {
            continue;
        }
        let Some(fi) = items.get(depth) else { continue };
        // A caption already used by a real item (or by an earlier calculated item on this field).
        if fi.items.iter().any(|it| it.label.eq_ignore_ascii_case(name)) {
            continue;
        }
        if pending.iter().any(|(d, n, _, _)| *d == depth && n.eq_ignore_ascii_case(name)) {
            continue;
        }
        let Ok(c) = pivot_item::compile(field, &ci.formula) else { continue };
        let refs = c.items().iter().map(|n| fi.items.iter().position(|it| it.label.eq_ignore_ascii_case(n.trim())).map(|i| i as u32)).collect();
        pending.push((depth, name.to_string(), c, refs));
    }
    let mut out = Vec::with_capacity(pending.len());
    for (depth, name, c, refs) in pending {
        let Some(fi) = items.get_mut(depth) else { continue };
        let item = fi.push_calc(&name);
        out.push(CalcItem { depth, item, term: c.term().clone(), refs });
    }
    out
}

/// Adds a synthetic report path for every calculated item under the parent groups the axis
/// already has: `[calc]` at depth 0, `[parent, calc]` below each first-level item, and so on.
fn add_calc_paths(set: &mut HashSet<Vec<u32>>, calc: &[CalcItem]) {
    let mut by_depth: HashMap<usize, Vec<u32>> = HashMap::new();
    for c in calc {
        by_depth.entry(c.depth).or_default().push(c.item);
    }
    for (depth, items) in by_depth {
        // The parent groups, collected before anything is inserted into the set.
        let mut prefixes: HashSet<Vec<u32>> = HashSet::new();
        for p in set.iter() {
            if p.len() > depth {
                prefixes.insert(p[..depth].to_vec());
            }
        }
        for pre in prefixes {
            for item in &items {
                if set.len() >= MAX_CALC_PATHS {
                    return;
                }
                let mut q = pre.clone();
                q.push(*item);
                set.insert(q);
            }
        }
    }
}

// ---------------------------------------------------------------- aggregation

#[derive(Clone, Debug, Default)]
struct Acc {
    count: u64,
    nums: u64,
    sum: f64,
    prod: f64,
    min: f64,
    max: f64,
    mean: f64,
    m2: f64,
    err: Option<CellError>,
}

impl Acc {
    fn add(&mut self, v: &Value) {
        match v {
            Value::Empty => {}
            Value::Number(n) => {
                let n = *n;
                self.count += 1;
                self.nums += 1;
                self.sum += n;
                if self.nums == 1 {
                    self.prod = n;
                    self.min = n;
                    self.max = n;
                } else {
                    self.prod *= n;
                    self.min = self.min.min(n);
                    self.max = self.max.max(n);
                }
                let d = n - self.mean;
                self.mean += d / self.nums as f64;
                self.m2 += d * (n - self.mean);
            }
            Value::Error(e) => {
                self.count += 1;
                if self.err.is_none() {
                    self.err = Some(*e);
                }
            }
            _ => self.count += 1,
        }
    }
    fn result(&self, f: PivotFunc) -> Value {
        let n = self.nums as f64;
        match f {
            PivotFunc::Count => return Value::number(self.count as f64),
            PivotFunc::CountNumbers => return Value::number(n),
            _ => {}
        }
        if let Some(e) = self.err {
            return Value::Error(e);
        }
        let div0 = Value::Error(CellError::Div0);
        match f {
            PivotFunc::Sum => Value::number(self.sum),
            PivotFunc::Average => {
                if self.nums == 0 {
                    div0
                } else {
                    Value::number(self.sum / n)
                }
            }
            PivotFunc::Max => Value::number(if self.nums == 0 { 0.0 } else { self.max }),
            PivotFunc::Min => Value::number(if self.nums == 0 { 0.0 } else { self.min }),
            PivotFunc::Product => Value::number(if self.nums == 0 { 0.0 } else { self.prod }),
            PivotFunc::StdDev | PivotFunc::Var => {
                if self.nums < 2 {
                    div0
                } else {
                    let var = (self.m2 / (n - 1.0)).max(0.0);
                    Value::number(if f == PivotFunc::Var { var } else { var.sqrt() })
                }
            }
            PivotFunc::StdDevP | PivotFunc::VarP => {
                if self.nums < 1 {
                    div0
                } else {
                    let var = (self.m2 / n).max(0.0);
                    Value::number(if f == PivotFunc::VarP { var } else { var.sqrt() })
                }
            }
            PivotFunc::Count | PivotFunc::CountNumbers => Value::Empty,
        }
    }
}

/// Excel's caption verb for a function ("Sum of Sales").
pub fn func_caption(f: PivotFunc) -> &'static str {
    match f {
        PivotFunc::Sum => "Sum",
        PivotFunc::Count => "Count",
        PivotFunc::Average => "Average",
        PivotFunc::Max => "Max",
        PivotFunc::Min => "Min",
        PivotFunc::Product => "Product",
        PivotFunc::CountNumbers => "Count Numbers",
        PivotFunc::StdDev => "StdDev",
        PivotFunc::StdDevP => "StdDevp",
        PivotFunc::Var => "Var",
        PivotFunc::VarP => "Varp",
    }
}

// ---------------------------------------------------------------- trees

#[derive(Clone, Debug)]
struct Node {
    item: u32,
    children: Vec<Node>,
}

fn build_tree(paths: &[&[u32]], depth: usize, fields: &[&PivotField], items: &[FieldItems]) -> Vec<Node> {
    let (Some(field), Some(fi)) = (fields.get(depth), items.get(depth)) else { return vec![] };
    let mut order: Vec<u32> = Vec::new();
    let mut groups: HashMap<u32, Vec<&[u32]>> = HashMap::new();
    for p in paths {
        if let Some(&it) = p.get(depth) {
            let g = groups.entry(it).or_default();
            if g.is_empty() {
                order.push(it);
            }
            g.push(p);
        }
    }
    let mut nodes: Vec<Node> = order
        .into_iter()
        .map(|it| {
            let ps = groups.remove(&it).unwrap_or_default();
            Node { item: it, children: build_tree(&ps, depth + 1, fields, items) }
        })
        .collect();
    let by = |a: &Node, b: &Node| match (fi.get(a.item), fi.get(b.item)) {
        (Some(x), Some(y)) => match field.sort {
            PivotSort::Asc => cmp_items(x, y),
            PivotSort::Desc => cmp_items(y, x),
            PivotSort::None => x.first.cmp(&y.first),
        },
        _ => Ordering::Equal,
    };
    nodes.sort_by(by);
    nodes
}

fn is_collapsed(field: &PivotField, label: &str) -> bool {
    field.collapsed_items.iter().any(|c| c.eq_ignore_ascii_case(label))
}

fn collect_siblings(nodes: &[Node], prefix: &mut Vec<u32>, out: &mut HashMap<Vec<u32>, Vec<u32>>) {
    out.insert(prefix.clone(), nodes.iter().map(|n| n.item).collect());
    for n in nodes {
        prefix.push(n.item);
        collect_siblings(&n.children, prefix, out);
        prefix.pop();
    }
}

// ---------------------------------------------------------------- output

/// What a report cell is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum CellKind {
    Empty,
    Header,
    RowLabel,
    ColLabel,
    Data,
    Subtotal,
    GrandTotal,
    Filter,
    Placeholder,
}

/// What a report row is (for row-wide styling).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum RowKind {
    Filter,
    Blank,
    Header,
    /// Item row; `true` for a parent item (bold).
    Item(bool),
    Subtotal,
    Grand,
    Placeholder,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OutCell {
    pub value: Value,
    pub kind: CellKind,
    pub indent: u8,
    pub num_fmt: Option<String>,
}

impl OutCell {
    fn empty() -> OutCell {
        OutCell { value: Value::Empty, kind: CellKind::Empty, indent: 0, num_fmt: None }
    }
}

/// A computed report: a dense grid relative to its top-left, with row kinds.
#[derive(Clone, Debug, Default)]
pub struct Output {
    pub grid: Vec<Vec<OutCell>>,
    pub row_kinds: Vec<RowKind>,
    /// Rows above the body (report filters plus a blank row).
    pub body_offset: u32,
    pub width: u32,
    /// Grid rows of the innermost (or collapsed) items: a PivotChart's categories.
    pub chart_rows: Vec<u32>,
    /// Grid columns of item (not total) values with their series names: a PivotChart's series.
    pub chart_cols: Vec<(u32, String)>,
    /// Grid column holding the innermost row labels (`None` without row fields).
    pub label_col: Option<u32>,
    /// Columns of row labels before the data columns.
    pub label_cols: u32,
    /// Source column of each row field and column field the report kept from the source.
    pub row_fields: Vec<String>,
    pub col_fields: Vec<String>,
    /// Captions of each field's items; a report path holds these indexes.
    pub row_items: Vec<Vec<String>>,
    pub col_items: Vec<Vec<String>>,
    /// Per grid row: the row-field items the line stands for (empty on a total line).
    pub row_paths: Vec<Vec<u32>>,
    /// Per grid row: the value field the line holds when Σ Values sits in Rows.
    pub row_values: Vec<Option<usize>>,
    /// Per data column (the columns right of `label_cols`): the column-field items it stands for.
    pub col_paths: Vec<Vec<u32>>,
    /// Per data column: the value field it holds (Σ Values in Rows names it on the line instead).
    pub col_values: Vec<Option<usize>>,
    /// Per data column: it is the grand-total column, not an item of the column fields.
    pub col_grand: Vec<bool>,
    /// Caption of each value field, in data-column order.
    pub value_names: Vec<String>,
}

impl Output {
    pub fn height(&self) -> u32 {
        self.grid.len() as u32
    }
    /// Value at (row, col) relative to the top-left.
    pub fn value(&self, r: usize, c: usize) -> Value {
        self.grid.get(r).and_then(|row| row.get(c)).map(|x| x.value.clone()).unwrap_or_default()
    }
    /// (grid row, grid column, data column) of a cell of the report written at `origin` (its
    /// top-left). `None` left of the data columns or outside the grid.
    fn grid_pos(&self, origin: CellRef, at: CellRef) -> Option<(usize, usize, usize)> {
        let r = at.row.checked_sub(origin.row)? as usize;
        let c = at.col.checked_sub(origin.col)? as usize;
        let j = c.checked_sub(self.label_cols as usize)?;
        Some((r, c, j))
    }
}

struct Grid {
    rows: Vec<Vec<OutCell>>,
    kinds: Vec<RowKind>,
    width: usize,
}

impl Grid {
    fn push_row(&mut self, kind: RowKind) -> Result<usize, String> {
        if (self.rows.len() as u64 + 1) * self.width.max(1) as u64 > MAX_OUTPUT_CELLS || self.rows.len() as u64 >= MAX_ROWS as u64 {
            return Err("The PivotTable report is too large.".into());
        }
        self.rows.push(vec![OutCell::empty(); self.width]);
        self.kinds.push(kind);
        Ok(self.rows.len() - 1)
    }
    fn set(&mut self, r: usize, c: usize, value: Value, kind: CellKind, indent: u8, fmt: Option<String>) {
        if let Some(cell) = self.rows.get_mut(r).and_then(|row| row.get_mut(c)) {
            *cell = OutCell { value, kind, indent, num_fmt: fmt };
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LineKind {
    Item(bool),
    Subtotal,
    Grand,
}

struct Line {
    kind: LineKind,
    /// A category of a PivotChart: an innermost (or collapsed) item.
    leaf: bool,
    path: Vec<u32>,
    values: bool,
    /// (column, value, indent, number format)
    labels: Vec<(usize, Value, u8, Option<String>)>,
    /// The value field of the line when Σ Values sits in Rows (`None`: one per data column).
    vi: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ColKind {
    Leaf,
    Subtotal,
    Grand,
}

struct ColSpec {
    kind: ColKind,
    path: Vec<u32>,
}

struct Ctx<'a> {
    pt: &'a PivotTable,
    rows: Vec<&'a PivotField>,
    row_items: Vec<FieldItems>,
    row_fmts: Vec<String>,
    /// Depth of Σ Values among the row fields when it sits in Rows (two or more value fields).
    vpos: Option<usize>,
    /// Value field captions.
    val_names: Vec<String>,
}

impl Ctx<'_> {
    fn label_cell(&self, depth: usize, item: u32) -> (Value, Option<String>, String) {
        let Some(it) = self.row_items.get(depth).and_then(|fi| fi.get(item)) else { return (Value::Empty, None, String::new()) };
        let fmt = if it.formatted { self.row_fmts.get(depth).cloned() } else { None };
        (it.value.clone(), fmt, it.label.clone())
    }

    /// (column, indent) of a row level (Σ Values counts as a level) in Compact/Outline form.
    fn level_cell(&self, level: usize) -> (usize, u8) {
        if self.pt.layout == PivotLayout::Outline { (level, 0) } else { (0, level.min(250) as u8) }
    }

    /// Σ Values comes right here, at row field depth `depth` (`vi`: the value field already chosen above).
    fn values_here(&self, depth: usize, vi: Option<usize>) -> bool {
        vi.is_none() && self.vpos == Some(depth)
    }

    /// Per-value subtotal lines of an item with Σ Values below it ("East Sum of Sales").
    fn value_subtotals(&self, col: usize, indent: u8, label: &str, prefix: &[u32], out: &mut Vec<Line>) {
        for (vi, name) in self.val_names.iter().enumerate() {
            out.push(Line {
                kind: LineKind::Subtotal,
                leaf: false,
                path: prefix.to_vec(),
                values: true,
                labels: vec![(col, Value::text(format!("{label} {name}")), indent, None)],
                vi: Some(vi),
            });
        }
    }

    /// The Σ Values level at row field depth `depth`: one item per value field, each followed by
    /// the row fields below it (`nodes`) or, innermost, holding that value.
    fn flatten_values(&self, nodes: &[Node], depth: usize, prefix: &mut Vec<u32>, out: &mut Vec<Line>) {
        let (col, indent) = self.level_cell(depth);
        for (vi, name) in self.val_names.iter().enumerate() {
            let labels = vec![(col, Value::text(name.as_str()), indent, None)];
            let leaf = nodes.is_empty();
            out.push(Line { kind: LineKind::Item(!leaf), leaf, path: prefix.clone(), values: leaf, labels, vi: Some(vi) });
            if !leaf {
                self.flatten(nodes, depth, Some(vi), prefix, out);
            }
        }
    }

    fn flatten(&self, nodes: &[Node], depth: usize, vi: Option<usize>, prefix: &mut Vec<u32>, out: &mut Vec<Line>) {
        if self.values_here(depth, vi) {
            self.flatten_values(nodes, depth, prefix, out);
            return;
        }
        let n = self.rows.len();
        // Σ Values still to come below this level: totals need one line per value field.
        let values_below = vi.is_none() && self.vpos.is_some();
        let (col, indent) = self.level_cell(depth + usize::from(vi.is_some() && self.vpos.is_some()));
        for node in nodes {
            prefix.push(node.item);
            let (v, fmt, label) = self.label_cell(depth, node.item);
            let collapsed = depth + 1 < n && self.rows.get(depth).is_some_and(|f| is_collapsed(f, &label));
            let leaf = depth + 1 >= n || collapsed || node.children.is_empty();
            if leaf && !values_below {
                out.push(Line {
                    kind: LineKind::Item(depth + 1 < n),
                    leaf: true,
                    path: prefix.clone(),
                    values: true,
                    labels: vec![(col, v, indent, fmt)],
                    vi,
                });
            } else if leaf {
                out.push(Line {
                    kind: LineKind::Item(true),
                    leaf: false,
                    path: prefix.clone(),
                    values: false,
                    labels: vec![(col, v, indent, fmt)],
                    vi,
                });
                // Outline gives each level its own column, so the captions go in the Values
                // column even when the levels between are collapsed away.
                let level = if self.pt.layout == PivotLayout::Outline { self.vpos.unwrap_or(depth + 1) } else { depth + 1 };
                self.flatten_values(&[], level, prefix, out);
            } else if values_below {
                // Subtotals per value field, below the group (one line can't hold them all).
                out.push(Line {
                    kind: LineKind::Item(true),
                    leaf: false,
                    path: prefix.clone(),
                    values: false,
                    labels: vec![(col, v, indent, fmt)],
                    vi,
                });
                self.flatten(&node.children, depth + 1, None, prefix, out);
                if self.pt.subtotals {
                    self.value_subtotals(col, indent, &label, prefix, out);
                }
            } else {
                let top = self.pt.subtotals && self.pt.subtotals_top;
                out.push(Line {
                    kind: LineKind::Item(true),
                    leaf: false,
                    path: prefix.clone(),
                    values: top,
                    labels: vec![(col, v, indent, fmt)],
                    vi,
                });
                self.flatten(&node.children, depth + 1, vi, prefix, out);
                if self.pt.subtotals && !self.pt.subtotals_top {
                    out.push(Line {
                        kind: LineKind::Subtotal,
                        leaf: false,
                        path: prefix.clone(),
                        values: true,
                        labels: vec![(col, Value::text(format!("{label} Total")), indent, None)],
                        vi,
                    });
                }
            }
            prefix.pop();
        }
    }

    /// Tabular form of [`Self::flatten_values`]: the first line also carries the parents' labels.
    fn flatten_tabular_values(
        &self,
        nodes: &[Node],
        depth: usize,
        prefix: &mut Vec<u32>,
        pending: &[(usize, Value, u8, Option<String>)],
        out: &mut Vec<Line>,
    ) {
        for (vi, name) in self.val_names.iter().enumerate() {
            let mut labels: Vec<(usize, Value, u8, Option<String>)> = if vi == 0 { pending.to_vec() } else { vec![] };
            labels.push((depth, Value::text(name.as_str()), 0, None));
            if nodes.is_empty() {
                out.push(Line { kind: LineKind::Item(false), leaf: true, path: prefix.clone(), values: true, labels, vi: Some(vi) });
            } else {
                self.flatten_tabular(nodes, depth, Some(vi), prefix, &labels, out);
            }
        }
    }

    fn flatten_tabular(
        &self,
        nodes: &[Node],
        depth: usize,
        vi: Option<usize>,
        prefix: &mut Vec<u32>,
        pending: &[(usize, Value, u8, Option<String>)],
        out: &mut Vec<Line>,
    ) {
        if self.values_here(depth, vi) {
            self.flatten_tabular_values(nodes, depth, prefix, pending, out);
            return;
        }
        let n = self.rows.len();
        let values_below = vi.is_none() && self.vpos.is_some();
        let col = depth + usize::from(vi.is_some() && self.vpos.is_some());
        for (i, node) in nodes.iter().enumerate() {
            prefix.push(node.item);
            let (v, fmt, label) = self.label_cell(depth, node.item);
            let mut labels: Vec<(usize, Value, u8, Option<String>)> = if i == 0 { pending.to_vec() } else { vec![] };
            labels.push((col, v, 0, fmt));
            let collapsed = depth + 1 < n && self.rows.get(depth).is_some_and(|f| is_collapsed(f, &label));
            if depth + 1 >= n || collapsed || node.children.is_empty() {
                if values_below {
                    self.flatten_tabular_values(&[], self.vpos.unwrap_or(depth + 1), prefix, &labels, out);
                } else {
                    out.push(Line { kind: LineKind::Item(false), leaf: true, path: prefix.clone(), values: true, labels, vi });
                }
            } else {
                self.flatten_tabular(&node.children, depth + 1, vi, prefix, &labels, out);
                if self.pt.subtotals && values_below {
                    self.value_subtotals(col, 0, &label, prefix, out);
                } else if self.pt.subtotals {
                    out.push(Line {
                        kind: LineKind::Subtotal,
                        leaf: false,
                        path: prefix.clone(),
                        values: true,
                        labels: vec![(col, Value::text(format!("{label} Total")), 0, None)],
                        vi,
                    });
                }
            }
            prefix.pop();
        }
    }
}

fn flatten_cols(
    nodes: &[Node],
    depth: usize,
    fields: &[&PivotField],
    items: &[FieldItems],
    subtotals: bool,
    prefix: &mut Vec<u32>,
    out: &mut Vec<ColSpec>,
) {
    let n = fields.len();
    for node in nodes {
        prefix.push(node.item);
        let label = items.get(depth).map(|fi| fi.label(node.item)).unwrap_or_default();
        let collapsed = depth + 1 < n && fields.get(depth).is_some_and(|f| is_collapsed(f, &label));
        if depth + 1 >= n || collapsed || node.children.is_empty() {
            out.push(ColSpec { kind: ColKind::Leaf, path: prefix.clone() });
        } else {
            flatten_cols(&node.children, depth + 1, fields, items, subtotals, prefix, out);
            if subtotals {
                out.push(ColSpec { kind: ColKind::Subtotal, path: prefix.clone() });
            }
        }
        prefix.pop();
    }
}

type AggMap = HashMap<(Vec<u32>, Vec<u32>), Vec<Acc>>;

/// Where a value field's numbers come from.
#[derive(Clone, Copy)]
enum ValSrc {
    /// A source column.
    Col(usize),
    /// A calculated field (index into the PivotTable's calculated fields).
    Calc(usize),
}

/// True when `col_groups` distinct column leaves (plus the grand-total column) times `values` value fields
/// (at least one data column per group) already exceed the sheet width. This is a lower bound on the final column count.
fn data_cols_exceed_sheet(col_groups: usize, grand_total: bool, values: usize) -> bool {
    (col_groups + usize::from(grand_total)).saturating_mul(values.max(1)) > MAX_COLS as usize
}

/// One report cell's value: the aggregates, with a calculated item in the row or column path
/// replaced by its formula over the items it names.
struct Lookup<'a> {
    wb: &'a Workbook,
    src: &'a Source,
    vals: &'a [(ValSrc, &'a PivotValue)],
    /// Source columns summed for the calculated fields.
    extra: &'a [usize],
    calcset: &'a CalcSet,
    aggs: &'a AggMap,
    /// Number of value fields.
    k: usize,
    rows: &'a [CalcItem],
    cols: &'a [CalcItem],
}

impl Lookup<'_> {
    /// The aggregate of the records of one cell (no calculated item in the path).
    fn raw(&self, rp: &[u32], cp: &[u32], vi: usize) -> Option<Value> {
        let (vs, v) = self.vals.get(vi)?;
        let accs = self.aggs.get(&(rp.to_vec(), cp.to_vec()))?;
        match vs {
            ValSrc::Col(_) => accs.get(vi).map(|a| a.result(v.func)),
            ValSrc::Calc(i) => {
                let sum = |name: &str| -> Value {
                    self.src
                        .col(name)
                        .and_then(|c| self.extra.iter().position(|x| *x == c))
                        .and_then(|j| accs.get(self.k + j))
                        .map(|a| a.result(PivotFunc::Sum))
                        .unwrap_or(Value::Number(0.0))
                };
                Some(self.calcset.value(self.wb, *i, &sum))
            }
        }
    }

    /// A cell's value, evaluating a calculated item in the row or column path.
    fn get(&self, rp: &[u32], cp: &[u32], vi: usize, depth: u32) -> Option<Value> {
        if depth > MAX_CALC_DEPTH {
            return Some(Value::Error(CellError::Num));
        }
        if let Some(c) = self.rows.iter().find(|c| rp.get(c.depth) == Some(&c.item)) {
            let mut p = rp.to_vec();
            let mut item = |i: usize| -> Value {
                let Some(j) = c.refs.get(i).copied().flatten() else { return Value::Error(CellError::Ref) };
                if let Some(slot) = p.get_mut(c.depth) {
                    *slot = j;
                }
                self.get(&p, cp, vi, depth + 1).unwrap_or(Value::Error(CellError::Ref))
            };
            return Some(pivot_item::eval(&c.term, &mut item));
        }
        if let Some(c) = self.cols.iter().find(|c| cp.get(c.depth) == Some(&c.item)) {
            let mut p = cp.to_vec();
            let mut item = |i: usize| -> Value {
                let Some(j) = c.refs.get(i).copied().flatten() else { return Value::Error(CellError::Ref) };
                if let Some(slot) = p.get_mut(c.depth) {
                    *slot = j;
                }
                self.get(rp, &p, vi, depth + 1).unwrap_or(Value::Error(CellError::Ref))
            };
            return Some(pivot_item::eval(&c.term, &mut item));
        }
        self.raw(rp, cp, vi)
    }
}

/// Computes a PivotTable report from the workbook.
pub fn compute(wb: &Workbook, pt: &PivotTable, pivot_sheet: usize) -> Result<Output, String> {
    compute_with_source(wb, pt, pivot_sheet).map(|(out, _)| out)
}

/// `compute`, also handing back the source list it read: Show Details writes the records the
/// source holds, so it needs them and not a second read of the sheet.
pub fn compute_with_source(wb: &Workbook, pt: &PivotTable, pivot_sheet: usize) -> Result<(Output, Source), String> {
    let src = read_source(wb, &pt.source, pivot_sheet)?;
    let fmt_of = |c: usize| src.formats.get(c).cloned().unwrap_or_else(|| "General".into());
    let rows: Vec<(usize, &PivotField)> = pt.rows.iter().filter_map(|f| src.col(&f.source_col).map(|c| (c, f))).collect();
    let cols: Vec<(usize, &PivotField)> = pt.columns.iter().filter_map(|f| src.col(&f.source_col).map(|c| (c, f))).collect();
    // Value fields: a source column, or a calculated field evaluated on sums (see `pivot_calc`).
    let calc = CalcSet::new(pt, &src.headers);
    let vals: Vec<(ValSrc, &PivotValue)> = pt
        .values
        .iter()
        .filter_map(|v| match src.col(&v.source_col) {
            Some(c) => Some((ValSrc::Col(c), v)),
            None => calc.index(&v.source_col).map(|i| (ValSrc::Calc(i), v)),
        })
        .collect();
    // Source columns summed for the calculated fields, accumulated after the value fields.
    let mut extra: Vec<usize> = Vec::new();
    for (vs, _) in &vals {
        if let ValSrc::Calc(i) = vs {
            for name in calc.source_refs(*i) {
                if let Some(c) = src.col(&name)
                    && !extra.contains(&c)
                {
                    extra.push(c);
                }
            }
        }
    }
    if rows.len() + cols.len() > MAX_AXIS_FIELDS {
        return Err(format!("A PivotTable can have at most {MAX_AXIS_FIELDS} row and column fields."));
    }
    if vals.len() > MAX_VALUE_FIELDS {
        return Err(format!("A PivotTable can have at most {MAX_VALUE_FIELDS} value fields."));
    }
    let axis_group = |name: &str| -> Option<Grouping> {
        pt.rows.iter().chain(pt.columns.iter()).find(|f| f.source_col.eq_ignore_ascii_case(name)).map(Grouping::of)
    };
    // (column, filter, grouping used for labels, shown as a report filter)
    let filters: Vec<(usize, &PivotFilter, Grouping, bool)> = pt
        .filters
        .iter()
        .filter_map(|f| {
            let c = src.col(&f.source_col)?;
            let g = axis_group(&f.source_col);
            // A report filter draws its own row; a filter on an axis only hides items (the file's
            // own answer wins over the guess, so a field in both the Filters and Rows areas keeps
            // its row).
            Some((c, f, g.unwrap_or_default(), f.is_page(g.is_some())))
        })
        .collect();
    let shown_filters: Vec<&(usize, &PivotFilter, Grouping, bool)> = filters.iter().filter(|f| f.3).collect();

    let k = vals.len();
    let empty = rows.is_empty() && cols.is_empty() && vals.is_empty();
    // Σ Values in Rows (it exists with two or more value fields): its depth among the row fields
    // found in the source.
    let vpos: Option<usize> = (pt.values_on_rows && k > 1).then(|| {
        let p = pt.values_position.unwrap_or(pt.rows.len());
        pt.rows.iter().take(p).filter(|f| src.col(&f.source_col).is_some()).count()
    });
    let on_rows = vpos.is_some();
    let row_levels = rows.len() + usize::from(on_rows);

    // ---- aggregate
    let mut row_items: Vec<FieldItems> = rows.iter().map(|_| FieldItems::default()).collect();
    let mut col_items: Vec<FieldItems> = cols.iter().map(|_| FieldItems::default()).collect();
    let mut aggs: AggMap = HashMap::new();
    let mut row_set: HashSet<Vec<u32>> = HashSet::new();
    let mut col_set: HashSet<Vec<u32>> = HashSet::new();
    let mut col_leaves: HashSet<Vec<u32>> = HashSet::new();
    let row_fmts: Vec<String> = rows.iter().map(|(c, _)| fmt_of(*c)).collect();
    let col_fmts: Vec<String> = cols.iter().map(|(c, _)| fmt_of(*c)).collect();
    'rec: for (ri, rec) in src.rows.iter().enumerate() {
        if !record_passes(wb, &src, pt, rec) {
            continue 'rec;
        }
        if empty {
            continue;
        }
        let mut rp = Vec::with_capacity(rows.len());
        for (((c, f), fi), fmt) in rows.iter().zip(row_items.iter_mut()).zip(row_fmts.iter()) {
            rp.push(fi.intern(wb, rec.get(*c).unwrap_or(&Value::Empty), Grouping::of(f), fmt, ri));
        }
        let mut cp = Vec::with_capacity(cols.len());
        for (((c, f), fi), fmt) in cols.iter().zip(col_items.iter_mut()).zip(col_fmts.iter()) {
            cp.push(fi.intern(wb, rec.get(*c).unwrap_or(&Value::Empty), Grouping::of(f), fmt, ri));
        }
        for a in 0..=rp.len() {
            for b in 0..=cp.len() {
                let e = aggs.entry((rp[..a].to_vec(), cp[..b].to_vec())).or_insert_with(|| vec![Acc::default(); k + extra.len()]);
                for (acc, (vs, _)) in e.iter_mut().zip(vals.iter()) {
                    if let ValSrc::Col(c) = vs {
                        acc.add(rec.get(*c).unwrap_or(&Value::Empty));
                    }
                }
                for (acc, c) in e.iter_mut().skip(k).zip(extra.iter()) {
                    acc.add(rec.get(*c).unwrap_or(&Value::Empty));
                }
            }
        }
        // The column leaf this record lands in: its path cut after the first collapsed item (as in `flatten_cols`).
        let shown = cols
            .iter()
            .zip(col_items.iter())
            .zip(cp.iter())
            .position(|(((_, f), fi), &it)| !f.collapsed_items.is_empty() && is_collapsed(f, &fi.label(it)))
            .map_or(cp.len(), |d| d + 1);
        if !col_leaves.contains(&cp[..shown]) {
            col_leaves.insert(cp[..shown].to_vec());
        }
        row_set.insert(rp);
        col_set.insert(cp);
        // Refuse a report that cannot fit before the accumulators for more column groups are allocated.
        if !cols.is_empty() && data_cols_exceed_sheet(col_leaves.len(), pt.grand_totals_rows, k) {
            return Err("The PivotTable report will not fit on the sheet.".into());
        }
        if aggs.len() > 5_000_000 {
            return Err("The PivotTable report is too large.".into());
        }
    }

    // ---- axes
    // Calculated items first: they extend their field's items and get a report path under every
    // parent group the axis already has.
    let row_calc = plan_calc_items(&pt.calculated_items, &rows, &mut row_items);
    let col_calc = plan_calc_items(&pt.calculated_items, &cols, &mut col_items);
    add_calc_paths(&mut row_set, &row_calc);
    add_calc_paths(&mut col_set, &col_calc);
    let row_fields: Vec<&PivotField> = rows.iter().map(|(_, f)| *f).collect();
    let col_fields: Vec<&PivotField> = cols.iter().map(|(_, f)| *f).collect();
    let row_paths: Vec<&[u32]> = row_set.iter().map(Vec::as_slice).collect();
    let col_paths: Vec<&[u32]> = col_set.iter().map(Vec::as_slice).collect();
    let row_tree = build_tree(&row_paths, 0, &row_fields, &row_items);
    let col_tree = build_tree(&col_paths, 0, &col_fields, &col_items);
    let mut siblings: HashMap<Vec<u32>, Vec<u32>> = HashMap::new();
    collect_siblings(&row_tree, &mut Vec::new(), &mut siblings);
    let level0: Vec<u32> = row_tree.iter().map(|n| n.item).collect();

    let val_names: Vec<String> = vals.iter().map(|(_, v)| v.name.clone()).collect();
    let ctx = Ctx { pt, rows: row_fields.clone(), row_items, row_fmts: row_fmts.clone(), vpos, val_names: val_names.clone() };
    let lk = Lookup { wb, src: &src, vals: &vals, extra: &extra, calcset: &calc, aggs: &aggs, k, rows: &row_calc, cols: &col_calc };
    let mut lines: Vec<Line> = Vec::new();
    if row_levels == 0 {
        lines.push(Line { kind: LineKind::Item(false), leaf: true, path: vec![], values: true, labels: vec![], vi: None });
    } else {
        if pt.layout == PivotLayout::Tabular {
            ctx.flatten_tabular(&row_tree, 0, None, &mut Vec::new(), &[], &mut lines);
        } else {
            ctx.flatten(&row_tree, 0, None, &mut Vec::new(), &mut lines);
        }
        // With only Σ Values in Rows its items are the totals already.
        if pt.grand_totals_cols && !rows.is_empty() {
            let grand = |text: String, vi: Option<usize>| Line {
                kind: LineKind::Grand,
                leaf: false,
                path: vec![],
                values: true,
                labels: vec![(0, Value::text(text), 0, None)],
                vi,
            };
            if on_rows {
                lines.extend(val_names.iter().enumerate().map(|(vi, name)| grand(format!("Total {name}"), Some(vi))));
            } else {
                lines.push(grand("Grand Total".into(), None));
            }
        }
    }
    let mut specs: Vec<ColSpec> = Vec::new();
    if cols.is_empty() {
        specs.push(ColSpec { kind: ColKind::Leaf, path: vec![] });
    } else {
        flatten_cols(&col_tree, 0, &col_fields, &col_items, pt.subtotals, &mut Vec::new(), &mut specs);
        if pt.grand_totals_rows {
            specs.push(ColSpec { kind: ColKind::Grand, path: vec![] });
        }
    }
    let mut data_cols: Vec<(usize, Option<usize>)> = Vec::new();
    for (si, _) in specs.iter().enumerate() {
        if on_rows {
            // Each line names its value field; one data column per column item.
            data_cols.push((si, None));
        } else if k == 0 {
            if !cols.is_empty() {
                data_cols.push((si, None));
            }
        } else {
            for vi in 0..k {
                data_cols.push((si, Some(vi)));
            }
        }
        if data_cols.len() > MAX_COLS as usize {
            return Err("The PivotTable report will not fit on the sheet.".into());
        }
    }

    let compact = pt.layout == PivotLayout::Compact;
    let l = if row_levels == 0 && cols.is_empty() {
        0
    } else if compact {
        1
    } else {
        row_levels.max(1)
    };
    let mut width = l + data_cols.len();
    if !shown_filters.is_empty() {
        width = width.max(2);
    }
    if empty {
        width = width.max(1);
    }
    let width = width.max(1);
    let mut g = Grid { rows: vec![], kinds: vec![], width };

    // ---- report filters
    for (c, f, _, _) in &shown_filters {
        let r = g.push_row(RowKind::Filter)?;
        let name = src.headers.get(*c).cloned().unwrap_or_default();
        let shown = match &f.selected {
            None => "(All)".to_string(),
            Some(v) if v.len() == 1 => v.first().cloned().unwrap_or_default(),
            Some(_) => "(Multiple Items)".to_string(),
        };
        g.set(r, 0, Value::text(name), CellKind::Filter, 0, None);
        g.set(r, 1, Value::text(shown), CellKind::Filter, 0, None);
    }
    if !shown_filters.is_empty() {
        g.push_row(RowKind::Blank)?;
    }
    let body_offset = g.rows.len() as u32;

    if empty {
        let r = g.push_row(RowKind::Placeholder)?;
        g.set(r, 0, Value::text(format!("{} — choose fields", pt.name)), CellKind::Placeholder, 0, None);
        return Ok((Output { width: g.width as u32, grid: g.rows, row_kinds: g.kinds, body_offset, ..Output::default() }, src));
    }

    let val_name = |vi: usize| vals.get(vi).map(|(_, v)| v.name.clone()).unwrap_or_default();
    let headers_on = pt.show_headers;
    let row_header = |g: &mut Grid, r: usize| {
        if row_levels == 0 || !headers_on {
            return;
        }
        if compact {
            g.set(r, 0, Value::text("Row Labels"), CellKind::Header, 0, None);
        } else {
            // One column per row field, Σ Values ("Values") at its place.
            let mut names: Vec<&str> = row_fields.iter().map(|f| f.source_col.as_str()).collect();
            if let Some(p) = vpos {
                names.insert(p.min(names.len()), "Values");
            }
            for (i, name) in names.into_iter().enumerate() {
                g.set(r, i, Value::text(name), CellKind::Header, 0, None);
            }
        }
    };

    // ---- header rows
    if cols.is_empty() {
        let r = g.push_row(RowKind::Header)?;
        row_header(&mut g, r);
        for (j, (_, vi)) in data_cols.iter().enumerate() {
            match vi {
                Some(vi) => g.set(r, l + j, Value::text(val_name(*vi)), CellKind::Header, 0, None),
                // Values on rows, no column field: the one data column is the grand total.
                None if on_rows => g.set(r, l + j, Value::text("Total"), CellKind::Header, 0, None),
                None => {}
            }
        }
    } else {
        let cap = g.push_row(RowKind::Header)?;
        if l > 0 && k == 1 {
            g.set(cap, 0, Value::text(val_name(0)), CellKind::Header, 0, None);
        }
        if headers_on {
            let caption = if compact { "Column Labels".to_string() } else { col_fields.first().map(|f| f.source_col.clone()).unwrap_or_default() };
            g.set(cap, l, Value::text(caption), CellKind::Header, 0, None);
        }
        let levels = cols.len() + usize::from(k > 1 && !on_rows);
        let first_level = g.rows.len();
        for _ in 0..levels {
            g.push_row(RowKind::Header)?;
        }
        let mut prev: Option<&ColSpec> = None;
        for (j, (si, vi)) in data_cols.iter().enumerate() {
            let Some(spec) = specs.get(*si) else { continue };
            let c = l + j;
            match spec.kind {
                ColKind::Leaf => {
                    for (lv, &it) in spec.path.iter().enumerate() {
                        let same = prev.is_some_and(|p| p.kind != ColKind::Grand && p.path.get(..=lv) == spec.path.get(..=lv));
                        if !same {
                            let Some(item) = col_items.get(lv).and_then(|fi| fi.get(it)) else { continue };
                            let fmt = if item.formatted { cols.get(lv).map(|(cc, _)| fmt_of(*cc)) } else { None };
                            g.set(first_level + lv, c, item.value.clone(), CellKind::ColLabel, 0, fmt);
                        }
                    }
                    if k > 1
                        && let Some(vi) = vi
                    {
                        g.set(first_level + cols.len(), c, Value::text(val_name(*vi)), CellKind::ColLabel, 0, None);
                    }
                }
                ColKind::Subtotal => {
                    let lv = spec.path.len().saturating_sub(1);
                    let label = spec.path.last().and_then(|it| col_items.get(lv).map(|fi| fi.label(*it))).unwrap_or_default();
                    let text = match vi {
                        Some(vi) if k > 1 => format!("{label} {}", val_name(*vi)),
                        _ => format!("{label} Total"),
                    };
                    g.set(first_level + lv, c, Value::text(text), CellKind::ColLabel, 0, None);
                }
                ColKind::Grand => {
                    let text = match vi {
                        Some(vi) if k > 1 => format!("Total {}", val_name(*vi)),
                        _ => "Grand Total".to_string(),
                    };
                    g.set(first_level, c, Value::text(text), CellKind::ColLabel, 0, None);
                }
            }
            prev = Some(spec);
        }
        let last = g.rows.len() - 1;
        row_header(&mut g, last);
    }

    // ---- body
    let get = |rp: &[u32], cp: &[u32], vi: usize| -> Option<Value> { lk.get(rp, cp, vi, 0) };
    let num = |v: &Option<Value>| v.as_ref().and_then(Value::as_f64);
    let ratio = |a: Value, b: Option<Value>| -> Value {
        match (a, b) {
            (Value::Error(e), _) => Value::Error(e),
            (_, Some(Value::Error(e))) => Value::Error(e),
            (Value::Number(x), Some(Value::Number(y))) => {
                if y == 0.0 {
                    Value::Error(CellError::Div0)
                } else {
                    Value::number(x / y)
                }
            }
            (v, _) => v,
        }
    };
    let data_fmt: Vec<Option<String>> = vals
        .iter()
        .map(|(vs, v)| match v.show_as {
            PivotShowAs::PercentOfGrandTotal | PivotShowAs::PercentOfColumnTotal | PivotShowAs::PercentOfRowTotal => {
                Some(v.number_format.clone().unwrap_or_else(|| "0.00%".into()))
            }
            PivotShowAs::Rank => v.number_format.clone(),
            _ => v.number_format.clone().or_else(|| match v.func {
                PivotFunc::Count | PivotFunc::CountNumbers => None,
                _ => {
                    let f = match vs {
                        ValSrc::Col(c) => fmt_of(*c),
                        ValSrc::Calc(_) => "General".to_string(),
                    };
                    (f != "General").then_some(f)
                }
            }),
        })
        .collect();
    let mut chart_rows: Vec<u32> = Vec::new();
    let chart_cols: Vec<(u32, String)> = data_cols
        .iter()
        .enumerate()
        .filter_map(|(j, (si, vi))| {
            let spec = specs.get(*si)?;
            if spec.kind != ColKind::Leaf {
                return None;
            }
            let mut parts: Vec<String> =
                spec.path.iter().enumerate().map(|(lv, it)| col_items.get(lv).map(|fi| fi.label(*it)).unwrap_or_default()).collect();
            if let Some(vi) = vi
                && (k > 1 || parts.is_empty())
            {
                parts.push(val_name(*vi));
            } else if parts.is_empty() {
                parts.push("Total".into());
            }
            Some(((l + j) as u32, parts.join(" - ")))
        })
        .filter(|_| k > 0)
        .collect();
    let label_col = if row_levels == 0 {
        None
    } else if compact {
        Some(0)
    } else {
        Some(row_levels.saturating_sub(1) as u32)
    };
    let first_line = g.rows.len();
    for line in &lines {
        let rk = match line.kind {
            LineKind::Item(b) => RowKind::Item(b),
            LineKind::Subtotal => RowKind::Subtotal,
            LineKind::Grand => RowKind::Grand,
        };
        let ck = match line.kind {
            LineKind::Item(_) => (CellKind::RowLabel, CellKind::Data),
            LineKind::Subtotal => (CellKind::Subtotal, CellKind::Subtotal),
            LineKind::Grand => (CellKind::GrandTotal, CellKind::GrandTotal),
        };
        let r = g.push_row(rk)?;
        if line.leaf && k > 0 {
            chart_rows.push(r as u32);
        }
        for (c, v, indent, fmt) in &line.labels {
            g.set(r, *c, v.clone(), ck.0, *indent, fmt.clone());
        }
        if !line.values {
            continue;
        }
        let rp = line.path.as_slice();
        for (j, (si, col_vi)) in data_cols.iter().enumerate() {
            let line_vi = col_vi.or(line.vi);
            let (Some(spec), Some(vi)) = (specs.get(*si), &line_vi) else { continue };
            let Some((_, vdef)) = vals.get(*vi) else { continue };
            let cp = spec.path.as_slice();
            let Some(raw) = get(rp, cp, *vi) else { continue };
            let shown = match vdef.show_as {
                PivotShowAs::Normal => raw,
                PivotShowAs::PercentOfGrandTotal => ratio(raw, get(&[], &[], *vi)),
                PivotShowAs::PercentOfColumnTotal => ratio(raw, get(&[], cp, *vi)),
                PivotShowAs::PercentOfRowTotal => ratio(raw, get(rp, &[], *vi)),
                PivotShowAs::RunningTotal => match (rp.first(), &raw) {
                    (Some(first), Value::Number(_)) => {
                        let mut acc = 0.0;
                        for it in &level0 {
                            let mut p = rp.to_vec();
                            if let Some(x) = p.first_mut() {
                                *x = *it;
                            }
                            acc += num(&get(&p, cp, *vi)).unwrap_or(0.0);
                            if it == first {
                                break;
                            }
                        }
                        Value::number(acc)
                    }
                    _ => raw,
                },
                PivotShowAs::Rank => match (rp.split_last(), raw.as_f64()) {
                    (Some((_, parent)), Some(me)) => {
                        let sibs = siblings.get(parent).cloned().unwrap_or_default();
                        let mut rank = 1u64;
                        for sib in sibs {
                            let mut p = parent.to_vec();
                            p.push(sib);
                            if num(&get(&p, cp, *vi)).is_some_and(|x| x > me) {
                                rank += 1;
                            }
                        }
                        Value::number(rank as f64)
                    }
                    (None, _) => Value::Empty,
                    _ => raw,
                },
            };
            g.set(r, l + j, shown, ck.1, 0, data_fmt.get(*vi).cloned().flatten());
        }
    }
    // ---- what each written cell stands for (Show Details, Generate GetPivotData)
    // One entry per grid row: a body line's path, and the value field it names when Σ Values sits
    // in Rows (the rows above the body hold none).
    let mut row_paths: Vec<Vec<u32>> = vec![Vec::new(); g.rows.len()];
    let mut row_values: Vec<Option<usize>> = vec![None; g.rows.len()];
    for (i, line) in lines.iter().enumerate() {
        let at = first_line.saturating_add(i);
        if let (Some(p), Some(v)) = (row_paths.get_mut(at), row_values.get_mut(at)) {
            *p = line.path.clone();
            *v = line.vi;
        }
    }
    let col_paths: Vec<Vec<u32>> = data_cols.iter().map(|(si, _)| specs.get(*si).map(|s| s.path.clone()).unwrap_or_default()).collect();
    let col_values: Vec<Option<usize>> = data_cols.iter().map(|(_, vi)| *vi).collect();
    let col_grand: Vec<bool> = data_cols.iter().map(|(si, _)| specs.get(*si).is_some_and(|s| s.kind == ColKind::Grand)).collect();
    // The item captions, moved out of the renderer (calculated items are already appended).
    let Ctx { row_items: row_item_list, .. } = ctx;
    let captions =
        |items: Vec<FieldItems>| -> Vec<Vec<String>> { items.into_iter().map(|fi| fi.items.into_iter().map(|it| it.label).collect()).collect() };
    let row_items = captions(row_item_list);
    let col_items = captions(col_items);
    Ok((
        Output {
            width: g.width as u32,
            grid: g.rows,
            row_kinds: g.kinds,
            body_offset,
            chart_rows,
            chart_cols,
            label_col,
            label_cols: l as u32,
            row_fields: row_fields.iter().map(|f| f.source_col.clone()).collect(),
            col_fields: col_fields.iter().map(|f| f.source_col.clone()).collect(),
            row_items,
            col_items,
            row_paths,
            row_values,
            col_paths,
            col_values,
            col_grand,
            value_names: val_names,
        },
        src,
    ))
}

// ---------------------------------------------------------------- styling and writing

/// Colours for a PivotTable style name (our own palette, keyed by the familiar
/// `PivotStyleLight1…28 / Medium1…28 / Dark1…28` names).
struct Look {
    header_fill: Color,
    header_font: Color,
    line: Color,
    filter_fill: Color,
}

fn look(style: &str) -> Look {
    let rest = style.strip_prefix("PivotStyle").unwrap_or(style);
    let (fam, n) = ["Light", "Medium", "Dark"]
        .iter()
        .find_map(|f| rest.strip_prefix(f).map(|num| (*f, num.parse::<u32>().unwrap_or(16))))
        .unwrap_or(("Light", 16));
    let k = n.saturating_sub(1) % 7;
    let acc = if k == 0 { None } else { Some(3 + k as u8) };
    let tone = |tint: i16| match acc {
        Some(a) => Color::Theme(a, tint),
        None => Color::Theme(1, tint.clamp(-1000, 1000)),
    };
    match fam {
        "Dark" => Look { header_fill: Color::Theme(1, 0), header_font: Color::Theme(0, 0), line: tone(0), filter_fill: tone(600) },
        "Medium" => Look { header_fill: tone(0), header_font: Color::Theme(0, 0), line: tone(0), filter_fill: tone(800) },
        _ => Look { header_fill: tone(800), header_font: Color::Auto, line: tone(400), filter_fill: tone(800) },
    }
}

fn cell_style(lk: &Look, rk: RowKind, cell: &OutCell, last_header: bool) -> Style {
    let mut s = Style::default();
    match rk {
        RowKind::Header => {
            s.font.bold = true;
            s.fill = Fill::solid(lk.header_fill);
            s.font.color = lk.header_font;
            if last_header {
                s.borders.bottom = BorderLine { style: BorderStyle::Thin, color: lk.line };
            }
        }
        RowKind::Filter => {
            s.fill = Fill::solid(lk.filter_fill);
            s.font.bold = last_header;
        }
        RowKind::Item(parent) => s.font.bold = parent,
        RowKind::Subtotal => s.font.bold = true,
        RowKind::Grand => {
            s.font.bold = true;
            s.borders.top = BorderLine { style: BorderStyle::Thin, color: lk.line };
        }
        RowKind::Placeholder => {
            s.font.italic = true;
            s.font.color = Color::Theme(1, 500);
        }
        RowKind::Blank => {}
    }
    if matches!(cell.kind, CellKind::RowLabel | CellKind::ColLabel | CellKind::Subtotal | CellKind::GrandTotal) && !cell.value.is_number() {
        s.align.h = HAlign::Left;
    }
    if matches!(cell.kind, CellKind::RowLabel | CellKind::ColLabel) && cell.value.is_number() {
        s.align.h = HAlign::Left;
    }
    s.align.indent = cell.indent.min(15);
    if let Some(f) = &cell.num_fmt {
        s.num_fmt = NumFmt::new(f);
    }
    s
}

/// Recomputes a pivot and writes its report. With `replace` false, refuses when the target area
/// holds other data. Returns the written area.
pub fn refresh(wb: &mut Workbook, sheet: usize, idx: usize, replace: bool) -> Result<RangeRef, String> {
    let pt = wb.sheet(sheet).and_then(|s| s.pivots.get(idx)).cloned().ok_or("There's no such PivotTable.")?;
    let out = compute(wb, &pt, sheet)?;
    let h = out.height().max(1);
    let w = out.width.max(1);
    let k = out.body_offset;
    let origin_row = if pt.anchor.row >= k { pt.anchor.row - k } else { pt.anchor.row };
    let origin = CellRef::new(origin_row, pt.anchor.col);
    let end_row = origin.row as u64 + h as u64 - 1;
    let end_col = origin.col as u64 + w as u64 - 1;
    if end_row >= MAX_ROWS as u64 || end_col >= MAX_COLS as u64 {
        return Err("The PivotTable report will not fit on the sheet. Choose a different location.".into());
    }
    let area = RangeRef::new(origin, CellRef::new(end_row as u32, end_col as u32));
    let sh = wb.sheet(sheet).ok_or("There's no such sheet.")?;
    for (j, other) in sh.pivots.iter().enumerate() {
        if j != idx && other.last_range.is_some_and(|r| r.intersects(&area)) {
            return Err("A PivotTable report cannot overlap another PivotTable report.".into());
        }
    }
    if sh.is_protected() {
        return Err("The cell or chart you're trying to change is on a protected sheet.".into());
    }
    if let Ok((ssi, sr)) = resolve_source(wb, &pt.source, sheet)
        && ssi == sheet
        && sr.intersects(&area)
    {
        return Err("The PivotTable report can't overlap its source data. Choose a different location.".into());
    }
    if sh.tables.iter().any(|t| t.range.intersects(&area)) {
        return Err("A PivotTable report cannot overlap a table.".into());
    }
    if !replace {
        for (c, cell) in sh.cells.iter_range(area) {
            if (!cell.value.is_empty() || cell.formula.is_some()) && !pt.last_range.is_some_and(|r| r.contains(c)) {
                return Err(format!(
                    "There's already data in {}!{}. Do you want to replace it? (Run again with replace: true to overwrite.)",
                    quote_sheet(&sh.name),
                    c.a1()
                ));
            }
        }
        if let Some(c) = sh.spill.keys().find(|c| area.contains(**c) && !pt.last_range.is_some_and(|r| r.contains(**c))) {
            return Err(format!(
                "There's already data in {}!{}. Do you want to replace it? (Run again with replace: true to overwrite.)",
                quote_sheet(&sh.name),
                c.a1()
            ));
        }
    }
    // Styles first (the style table is workbook-wide).
    let lk = look(&pt.style);
    let header_last = out.row_kinds.iter().rposition(|k| *k == RowKind::Header);
    let mut cache: HashMap<Style, StyleId> = HashMap::new();
    let mut cells: Vec<(CellRef, Cell)> = Vec::new();
    for (ri, row) in out.grid.iter().enumerate() {
        let rk = out.row_kinds.get(ri).copied().unwrap_or(RowKind::Blank);
        if rk == RowKind::Blank {
            continue;
        }
        for (ci, oc) in row.iter().enumerate() {
            if rk == RowKind::Filter && oc.value.is_empty() {
                continue;
            }
            if rk == RowKind::Placeholder && oc.value.is_empty() {
                continue;
            }
            let flag = match rk {
                RowKind::Header => Some(ri) == header_last,
                RowKind::Filter => ci == 0,
                _ => false,
            };
            let st = cell_style(&lk, rk, oc, flag);
            let id = match cache.get(&st) {
                Some(id) => *id,
                None => {
                    let id = wb.styles.intern(st.clone());
                    cache.insert(st, id);
                    id
                }
            };
            if oc.value.is_empty() && id == StyleId::default() {
                continue;
            }
            cells.push((CellRef::new(origin.row + ri as u32, origin.col + ci as u32), Cell { value: oc.value.clone(), formula: None, style: id }));
        }
    }
    // Column widths that fit the labels.
    let mut widths: HashMap<u32, f32> = HashMap::new();
    for (c, cell) in &cells {
        if cell.value.is_empty() {
            continue;
        }
        let st = wb.styles.get(cell.style);
        let text = crate::display::format(&cell.value, st.num_fmt.as_str(), wb).text;
        let wpt = crate::cmd::format::approx_text_width(&text, st.font.size, st.font.bold) + st.align.indent as f32 * 9.0 + 14.0;
        let e = widths.entry(c.col).or_insert(0.0);
        *e = e.max(wpt.min(320.0));
    }
    let shm = wb.sheet_mut(sheet).ok_or("There's no such sheet.")?;
    if let Some(old) = pt.last_range {
        shm.take_cells(old);
    }
    shm.take_cells(area);
    for (c, cell) in cells {
        shm.set_cell(c, cell);
    }
    for (col, wpt) in widths {
        if wpt > shm.col_width(col) && !shm.is_col_hidden(col) {
            shm.cols.entry(col).or_default().size = Some(wpt);
        }
    }
    if let Some(p) = shm.pivots.get_mut(idx) {
        p.last_range = Some(area);
    }
    let sheet_name = shm.name.clone();
    rebind_charts(wb, &sheet_name, &pt.name, origin, &out);
    Ok(area)
}

// ---------------------------------------------------------------- PivotCharts

/// The link a PivotChart stores to its PivotTable: `Sheet2!PivotTable1`.
pub fn chart_link(sheet: &str, pivot: &str) -> String {
    format!("{}!{pivot}", quote_sheet(sheet))
}

/// (sheet, PivotTable name) of a PivotChart link; a leading `[Book.xlsx]` is ignored.
pub fn parse_chart_link(link: &str) -> Option<(String, String)> {
    let t = link.trim();
    let t = match t.strip_prefix('[') {
        Some(rest) => rest.split_once(']').map(|(_, r)| r).unwrap_or(rest),
        None => t,
    };
    let (sheet, name) = t.rsplit_once('!')?;
    let sheet = sheet.trim();
    let sheet = gridcraft_core::unquote_sheet(sheet);
    let name = name.trim();
    if sheet.is_empty() || name.is_empty() {
        return None;
    }
    Some((sheet, name.to_string()))
}

/// Whether a PivotChart link points at `pivot` on `sheet`.
pub fn links_to(link: &str, sheet: &str, pivot: &str) -> bool {
    parse_chart_link(link).is_some_and(|(s, n)| s.eq_ignore_ascii_case(sheet) && n.eq_ignore_ascii_case(pivot))
}

/// An absolute reference to `col` over grid `rows` (contiguous runs joined into a union).
fn rows_ref(sheet: &str, origin: CellRef, col: u32, rows: &[u32]) -> Option<String> {
    let q = quote_sheet(sheet);
    let c = origin.col.checked_add(col)?;
    let letters = gridcraft_core::col_to_letters(c);
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while let Some(&start) = rows.get(i) {
        let mut end = start;
        while rows.get(i + 1).is_some_and(|n| Some(*n) == end.checked_add(1)) {
            end += 1;
            i += 1;
        }
        i += 1;
        let (a, b) = (origin.row.checked_add(start)? + 1, origin.row.checked_add(end)? + 1);
        parts.push(if a == b { format!("{q}!${letters}${a}") } else { format!("{q}!${letters}${a}:${letters}${b}") });
    }
    match parts.len() {
        0 => None,
        1 => parts.pop(),
        _ => Some(format!("({})", parts.join(","))),
    }
}

/// Most category rows a PivotChart plots.
const MAX_CHART_ROWS: usize = 10_000;

/// A PivotChart's series for a report written at `origin`: categories are the innermost row
/// items, one series per item column (column items × value fields); totals are left out.
pub fn chart_series(sheet: &str, origin: CellRef, out: &Output) -> Vec<Series> {
    let rows: Vec<u32> = out.chart_rows.iter().copied().take(MAX_CHART_ROWS).collect();
    let categories = out.label_col.and_then(|c| rows_ref(sheet, origin, c, &rows));
    out.chart_cols
        .iter()
        .filter_map(|(c, name)| {
            Some(Series {
                name: Some(format!("\"{}\"", name.replace('"', "\"\""))),
                categories: categories.clone(),
                values: rows_ref(sheet, origin, *c, &rows)?,
                bubble_sizes: None,
                color: None,
                secondary: false,
                kind: None,
                quartiles: Default::default(),
                smooth: false,
                trendlines: Vec::new(),
                ..Series::default()
            })
        })
        .collect()
}

/// Rebuilds the series of every PivotChart of a PivotTable after its report was written.
fn rebind_charts(wb: &mut Workbook, sheet: &str, pivot: &str, origin: CellRef, out: &Output) {
    let linked = |ch: &Chart| ch.pivot.as_deref().is_some_and(|l| links_to(l, sheet, pivot));
    let hits: Vec<usize> = (0..wb.sheets.len()).filter(|i| wb.sheet(*i).is_some_and(|sh| sh.charts.iter().any(linked))).collect();
    if hits.is_empty() {
        return;
    }
    let series = chart_series(sheet, origin, out);
    for i in hits {
        let Some(sh) = wb.sheet_mut(i) else { continue };
        for ch in sh.charts.iter_mut().filter(|c| linked(c)) {
            ch.series = series.clone();
        }
    }
}

/// Clears a pivot's last output area.
pub fn clear_output(wb: &mut Workbook, sheet: usize, idx: usize) {
    let Some(shm) = wb.sheet_mut(sheet) else { return };
    let Some(r) = shm.pivots.get(idx).and_then(|p| p.last_range) else { return };
    shm.take_cells(r);
    if let Some(p) = shm.pivots.get_mut(idx) {
        p.last_range = None;
    }
}

// ---------------------------------------------------------------- Show Details / GetPivotData

/// Most cells a Show Details sheet may hold: the bulk-edit cap, so a wide source writes fewer
/// rows (12,000,000 cells is a million rows of twelve columns).
pub const MAX_DETAIL_CELLS: u64 = 12_000_000;

/// Why a click on the cell is not the value cell it has to be.
const NOT_A_VALUE_CELL: &str =
    "That isn't a value cell of the PivotTable report: show details and generate GetPivotData work on its numbers, not on its labels.";

/// What Show Details writes into its new sheet: the source's header row and the records behind
/// one report cell.
#[derive(Clone, Debug, Default)]
pub struct Detail {
    /// Sheet name made from the cell's item path (`East`, `East - Jan`); it may still collide.
    pub title: String,
    /// The source's header row.
    pub headers: Vec<String>,
    /// The source's column number formats, so dates and currency look like the source.
    pub formats: Vec<String>,
    /// The records, in source order.
    pub rows: Vec<Vec<Value>>,
    /// Records the cell cap left out.
    pub skipped: u64,
}

/// The PivotTable whose last written report covers `at`.
pub fn pivot_at(wb: &Workbook, sheet: usize, at: CellRef) -> Option<&PivotTable> {
    wb.sheet(sheet)?.pivots.iter().find(|p| p.last_range.is_some_and(|r| r.contains(at)))
}

/// The records behind one report cell (Excel's Show Details): every record the report's filters
/// pass whose own item path starts with the cell's, in source order. A leaf cell gets its records,
/// a subtotal its group's and a grand total all of them. `Err` when the cell is not a value cell
/// of the report, or when the path cannot be derived (a calculated item holds no records).
pub fn details(wb: &Workbook, pt: &PivotTable, sheet: usize, cell: CellRef) -> Result<Detail, String> {
    details_with_cap(wb, pt, sheet, cell, MAX_DETAIL_CELLS)
}

/// Rows a Show Details sheet may hold over a source `width` columns wide: `max_cells` cells, and
/// never more than a sheet's rows (one of them is the header row).
pub fn detail_row_cap(width: usize, max_cells: u64) -> u64 {
    (max_cells / width.max(1) as u64).max(1).min(MAX_ROWS as u64 - 1)
}

/// [`details`] with the cell cap given (the command uses [`MAX_DETAIL_CELLS`]).
pub fn details_with_cap(wb: &Workbook, pt: &PivotTable, sheet: usize, cell: CellRef, max_cells: u64) -> Result<Detail, String> {
    let area = pt.last_range.ok_or("Refresh the PivotTable before showing details.")?;
    if !area.contains(cell) {
        return Err(format!("{} is outside the PivotTable report {}.", cell.a1(), area.a1()));
    }
    let (out, src) = compute_with_source(wb, pt, sheet)?;
    let (r, c, j) = out.grid_pos(area.start, cell).ok_or(NOT_A_VALUE_CELL)?;
    let kind = out.grid.get(r).and_then(|row| row.get(c)).map(|x| x.kind);
    if !matches!(kind, Some(CellKind::Data | CellKind::Subtotal | CellKind::GrandTotal)) {
        return Err(NOT_A_VALUE_CELL.into());
    }
    let row_path = out.row_paths.get(r).cloned().unwrap_or_default();
    let col_path = out.col_paths.get(j).cloned().ok_or(NOT_A_VALUE_CELL)?;
    // The sheet's name: the items the line and the column stand for.
    let title = {
        let mut parts: Vec<String> = Vec::new();
        for (d, id) in row_path.iter().enumerate() {
            if let Some(l) = out.row_items.get(d).and_then(|items| items.get(*id as usize)) {
                parts.push(l.clone());
            }
        }
        for (d, id) in col_path.iter().enumerate() {
            if let Some(l) = out.col_items.get(d).and_then(|items| items.get(*id as usize)) {
                parts.push(l.clone());
            }
        }
        if parts.is_empty() { "Details".to_string() } else { parts.join(" - ") }
    };
    // The items of each field, interned in source order exactly as `compute` does: a report path
    // holds the position of a value in that order, so walking the records the same way gives the
    // ids the report used. The fields kept are the ones the report kept.
    let rows_f: Vec<(usize, &PivotField)> = pt.rows.iter().filter_map(|f| src.col(&f.source_col).map(|c| (c, f))).collect();
    let cols_f: Vec<(usize, &PivotField)> = pt.columns.iter().filter_map(|f| src.col(&f.source_col).map(|c| (c, f))).collect();
    let fmt_of = |c: usize| src.formats.get(c).cloned().unwrap_or_else(|| "General".into());
    let row_fmts: Vec<String> = rows_f.iter().map(|(c, _)| fmt_of(*c)).collect();
    let col_fmts: Vec<String> = cols_f.iter().map(|(c, _)| fmt_of(*c)).collect();
    let mut row_items: Vec<FieldItems> = rows_f.iter().map(|_| FieldItems::default()).collect();
    let mut col_items: Vec<FieldItems> = cols_f.iter().map(|_| FieldItems::default()).collect();
    let cap = detail_row_cap(src.headers.len(), max_cells);
    let mut kept: Vec<Vec<Value>> = Vec::new();
    let mut skipped = 0u64;
    for (ri, rec) in src.rows.iter().enumerate() {
        if !record_passes(wb, &src, pt, rec) {
            continue;
        }
        let rp: Vec<u32> = rows_f
            .iter()
            .zip(row_items.iter_mut())
            .zip(row_fmts.iter())
            .map(|(((c, f), fi), fmt)| fi.intern(wb, rec.get(*c).unwrap_or(&Value::Empty), Grouping::of(f), fmt, ri))
            .collect();
        let cp: Vec<u32> = cols_f
            .iter()
            .zip(col_items.iter_mut())
            .zip(col_fmts.iter())
            .map(|(((c, f), fi), fmt)| fi.intern(wb, rec.get(*c).unwrap_or(&Value::Empty), Grouping::of(f), fmt, ri))
            .collect();
        if !rp.starts_with(&row_path) || !cp.starts_with(&col_path) {
            continue;
        }
        if kept.len() as u64 >= cap {
            skipped = skipped.saturating_add(1);
            continue;
        }
        kept.push(rec.clone());
    }
    // An item past the field's real items is a calculated item: it has no records of its own.
    let calc =
        |items: &[FieldItems], path: &[u32]| path.iter().enumerate().any(|(d, id)| items.get(d).is_none_or(|fi| *id as usize >= fi.items.len()));
    if kept.is_empty() && (calc(&row_items, &row_path) || calc(&col_items, &col_path)) {
        return Err("Show Details isn't available for a calculated item.".into());
    }
    Ok(Detail { title, headers: src.headers.clone(), formats: src.formats.clone(), rows: kept, skipped })
}

/// A text argument of a formula.
fn quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// The `GETPIVOTDATA(...)` text for one report cell (without the leading `=`): the value field's
/// caption, an absolute reference to the report, then the field/item pairs the report draws, so
/// that it evaluates to the number the cell shows. `None` for a cell that is not a value of a
/// PivotTable report.
pub fn get_pivot_data(wb: &Workbook, sheet: usize, at: CellRef) -> Option<String> {
    let pt = pivot_at(wb, sheet, at)?;
    let out = compute(wb, pt, sheet).ok()?;
    formula_for(wb, sheet, pt, &out, at).ok()
}

/// What a click on `at` inserts into a formula being typed: the `GETPIVOTDATA` call when the
/// PivotTable's "Generate GetPivotData" is on (Excel's default), nothing otherwise — the caller
/// then inserts a plain reference.
pub fn click_reference(wb: &Workbook, sheet: usize, at: CellRef) -> Option<String> {
    let pt = pivot_at(wb, sheet, at)?;
    if !pt.generate_get_pivot_data {
        return None;
    }
    get_pivot_data(wb, sheet, at)
}

fn formula_for(wb: &Workbook, sheet: usize, pt: &PivotTable, out: &Output, at: CellRef) -> Result<String, String> {
    let area = pt.last_range.ok_or("Refresh the PivotTable first: the cell isn't part of a written report.")?;
    let (r, c, j) = out.grid_pos(area.start, at).ok_or(NOT_A_VALUE_CELL)?;
    if !matches!(out.grid.get(r).and_then(|row| row.get(c)).map(|x| x.kind), Some(CellKind::Data | CellKind::Subtotal | CellKind::GrandTotal)) {
        return Err(NOT_A_VALUE_CELL.into());
    }
    let vi = out.col_values.get(j).copied().flatten().or_else(|| out.row_values.get(r).copied().flatten()).ok_or(NOT_A_VALUE_CELL)?;
    let value = out.value_names.get(vi).filter(|n| !n.is_empty()).ok_or(NOT_A_VALUE_CELL)?;
    let name = wb.sheet(sheet).map(|s| s.name.clone()).ok_or("There's no such sheet.")?;
    let abs = |c: CellRef| format!("\u{24}{}\u{24}{}", gridcraft_core::col_to_letters(c.col), c.row as u64 + 1);
    let corner = format!("{}!{}", quote_sheet(&name), abs(area.start));
    let mut args: Vec<String> = vec![quoted(value), corner];
    // The pairs of one axis: its items, or "Grand Total" for a total line (which stands for every
    // item of the axis's first field).
    let pairs = |fields: &[String], items: &[Vec<String>], path: &[u32]| -> Result<Vec<(String, String)>, String> {
        path.iter()
            .enumerate()
            .map(|(d, id)| {
                let f = fields.get(d).ok_or(NOT_A_VALUE_CELL)?;
                let l = items.get(d).and_then(|l| l.get(*id as usize)).ok_or(NOT_A_VALUE_CELL)?;
                Ok((f.clone(), l.clone()))
            })
            .collect()
    };
    let rp = out.row_paths.get(r).map(Vec::as_slice).unwrap_or(&[]);
    let mut want: Vec<(String, String)> = if rp.is_empty() {
        out.row_fields.first().map(|f| vec![(f.clone(), "Grand Total".to_string())]).unwrap_or_default()
    } else {
        pairs(&out.row_fields, &out.row_items, rp)?
    };
    let cp = out.col_paths.get(j).map(Vec::as_slice).unwrap_or(&[]);
    if cp.is_empty() {
        if out.col_grand.get(j).copied().unwrap_or(false)
            && let Some(f) = out.col_fields.first()
        {
            want.push((f.clone(), "Grand Total".to_string()));
        }
    } else {
        want.extend(pairs(&out.col_fields, &out.col_items, cp)?);
    }
    for (field, item) in &want {
        args.push(quoted(field));
        args.push(quoted(item));
    }
    Ok(format!("GETPIVOTDATA({})", args.join(", ")))
}

#[cfg(test)]
mod fit_tests {
    use super::data_cols_exceed_sheet;

    #[test]
    fn column_budget_counts_value_fields_and_grand_total() {
        assert!(!data_cols_exceed_sheet(64, true, 128));
        assert!(data_cols_exceed_sheet(64, true, 256));
        assert!(!data_cols_exceed_sheet(64, false, 256));
        assert!(!data_cols_exceed_sheet(16_384, false, 0));
        assert!(data_cols_exceed_sheet(16_385, false, 0));
    }
}
