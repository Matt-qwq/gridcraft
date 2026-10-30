//! Table styles: how a cell inside a table looks (our own palettes, keyed by the familiar
//! `TableStyleLight1…21 / Medium1…28 / Dark1…11` names so files round-trip).

use gridcraft_core::{CellRef, RangeRef};
use gridcraft_model::*;

/// (family, number) from a style name, e.g. `TableStyleMedium2` → ("Medium", 2).
pub fn parse_style_name(name: &str) -> (&'static str, u32) {
    let rest = name.strip_prefix("TableStyle").unwrap_or(name);
    for fam in ["Light", "Medium", "Dark"] {
        if let Some(n) = rest.strip_prefix(fam) {
            let k = n.parse::<u32>().unwrap_or(2).max(1); // the gallery starts at 1; 0 would underflow `n - 1`
            return (fam, k);
        }
    }
    ("Medium", 2)
}

/// Accent theme slot (4..=9) for a style number; 1 and every 7th style are neutral (dk1).
fn accent_for(n: u32) -> Option<u8> {
    let k = (n.saturating_sub(1)) % 7;
    if k == 0 { None } else { Some(3 + k as u8) }
}

/// Header/band/body colours for a table style.
pub struct TableLook {
    pub header_fill: Option<Color>,
    pub header_font: Color,
    pub header_bold: bool,
    pub band_fill: Option<Color>,
    pub body_fill: Option<Color>,
    pub line: Option<Color>,
    pub total_top: Option<Color>,
}

pub fn look(style: &str) -> TableLook {
    let (fam, n) = parse_style_name(style);
    let acc = accent_for(n);
    let base = |tint: i16| match acc {
        Some(a) => Color::Theme(a, tint),
        None => Color::Theme(1, if tint == 0 { 0 } else { tint.clamp(-1000, 1000) }),
    };
    let neutral_light = |tint: i16| match acc {
        Some(a) => Color::Theme(a, tint),
        None => Color::Theme(0, -((1000 - tint).clamp(0, 1000) / 6)),
    };
    match fam {
        "Light" => {
            let group = (n - 1) / 7; // 0: lines only, 1: banded header line, 2: grid
            TableLook {
                header_fill: None,
                header_font: if group == 0 { base(-250) } else { Color::Auto },
                header_bold: true,
                band_fill: Some(neutral_light(800)),
                body_fill: None,
                line: Some(base(0)),
                total_top: Some(base(0)),
            }
        }
        "Dark" => TableLook {
            header_fill: Some(Color::Theme(1, 0)),
            header_font: Color::Theme(0, 0),
            header_bold: true,
            band_fill: Some(base(-250)),
            body_fill: Some(base(0)),
            line: None,
            total_top: Some(Color::Theme(0, 0)),
        },
        _ => TableLook {
            header_fill: Some(base(0)),
            header_font: Color::Theme(0, 0),
            header_bold: true,
            band_fill: Some(neutral_light(800)),
            body_fill: None,
            line: if (n - 1) / 7 == 0 { None } else { Some(base(400)) },
            total_top: Some(base(0)),
        },
    }
}

/// The style a table gives a cell (merged over the cell's own style: explicit fills/fonts win).
pub fn table_cell_style(wb: &Workbook, sheet: usize, t: &Table, c: CellRef) -> Style {
    let own = wb.sheet(sheet).map(|sh| wb.styles.get(sh.style_id(c)).clone()).unwrap_or_default();
    let mut s = own.clone();
    let lk = look(&t.style);
    let is_header = t.header_row && c.row == t.range.start.row;
    let is_total = t.totals_row && c.row == t.range.end.row;
    let data_row = c.row.saturating_sub(t.range.start.row + t.header_row as u32);
    let data_col = c.col - t.range.start.col;
    let explicit_fill = own.fill.pattern != PatternType::None;
    if is_header {
        if let Some(f) = lk.header_fill
            && !explicit_fill
        {
            s.fill = Fill::solid(f);
        }
        if own.font.color == Color::Auto {
            s.font.color = lk.header_font;
        }
        s.font.bold = s.font.bold || lk.header_bold;
        if let Some(l) = lk.line {
            s.borders.bottom = BorderLine { style: BorderStyle::Thin, color: l };
        }
    } else if is_total {
        s.font.bold = true;
        if let Some(l) = lk.total_top {
            s.borders.top = BorderLine { style: BorderStyle::Double, color: l };
        }
    } else {
        let band = (t.banded_rows && data_row.is_multiple_of(2)) || (t.banded_cols && data_col.is_multiple_of(2));
        if !explicit_fill {
            if band {
                if let Some(f) = lk.band_fill {
                    s.fill = Fill::solid(f);
                }
            } else if let Some(f) = lk.body_fill {
                s.fill = Fill::solid(f);
            }
        }
        if let Some(l) = lk.line
            && t.style.contains("Light")
            && c.row == t.range.end.row
        {
            s.borders.bottom = BorderLine { style: BorderStyle::Thin, color: l };
        }
    }
    if (t.first_col && c.col == t.range.start.col) || (t.last_col && c.col == t.range.end.col) {
        s.font.bold = true;
    }
    if t.style.starts_with("TableStyleDark") && !is_header && own.font.color == Color::Auto {
        s.font.color = Color::Theme(0, 0);
    }
    s
}

/// Gallery list of style names.
pub fn gallery() -> Vec<String> {
    let mut v = Vec::new();
    for n in 1..=21 {
        v.push(format!("TableStyleLight{n}"));
    }
    for n in 1..=28 {
        v.push(format!("TableStyleMedium{n}"));
    }
    for n in 1..=11 {
        v.push(format!("TableStyleDark{n}"));
    }
    v
}

/// The first `ColumnN` at or after `from` that `used` (lower-case names) does not hold.
///
/// At most `used.len()` names are taken, so one of the next `used.len() + 1` is free; the loop is
/// bounded by that even when a table holds hostile names such as `Column18446744073709551615`.
fn first_free_column(used: &std::collections::HashSet<String>, from: usize) -> String {
    let start = from.max(1);
    let end = start.saturating_add(used.len()).saturating_add(1);
    (start..end).map(|n| format!("Column{n}")).find(|n| !used.contains(&n.to_lowercase())).unwrap_or_else(|| format!("Column{start}"))
}

/// The column names a table's header row must have, in column order, from the header cells' texts
/// (`None` where a cell is blank). Names are unique and non-empty, and this is the rule the engine
/// and the XLSX writer share, so the table in memory and the table in the file can never disagree:
/// - a blank header takes the first free `ColumnN` at or after its position, so a table resized or
///   grown over blank cells keeps counting (Column1, Column2, Column3…) even when it no longer
///   starts at A, and never lands on a name an earlier column already has;
/// - a name an earlier column already has takes the first free `{name}{k}` from 2 (`x`, `x2`,
///   `x3`…), so a typed duplicate is renamed on the column that repeats it, by position — never
///   silently on save, matched by some other rule.
///
/// `crates/xlsx/src/sheet_write.rs`'s `table_columns` applies the same repeated-name suffix for a
/// table that arrives from a file; it repairs a malformed model, it is not how the engine names a
/// table.
pub(crate) fn header_names(texts: &[Option<String>]) -> Vec<String> {
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::with_capacity(texts.len());
    // The next suffix to try for a repeated name, so a wide header row stays linear.
    let mut next: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut out = Vec::with_capacity(texts.len());
    for (i, text) in texts.iter().enumerate() {
        let base = match text.as_deref() {
            Some(t) if !t.trim().is_empty() => t.to_string(),
            _ => first_free_column(&used, i.saturating_add(1)),
        };
        let key = base.to_lowercase();
        let mut name = base.clone();
        let mut k = next.get(&key).copied().unwrap_or(2);
        if used.contains(&key) {
            // At most `i` names are used, so one of the next `i + 1` suffixes is free.
            let last = k.saturating_add(i as u32).saturating_add(2);
            loop {
                let candidate = format!("{base}{k}");
                k = k.saturating_add(1);
                if !used.contains(&candidate.to_lowercase()) || k > last {
                    name = candidate;
                    break;
                }
            }
        }
        next.insert(key, k.max(2));
        used.insert(name.to_lowercase());
        out.push(name);
    }
    out
}

/// Editing a table's header row renames the column, as in Excel: an empty or repeated header
/// gets a unique name written back into the cell, and structured references follow the rename.
/// The whole header row is reconciled at once, because uniqueness is a property of the row: had
/// this only looked at the cells that changed, a duplicate made by a structural edit (a resized
/// table's blank column taking the name a neighbour already had) survived here and was renamed
/// silently when the file was written, so `Column3` came back as `Column32`.
pub(crate) fn sync_headers(cx: &mut crate::cmd::Ctx) {
    if cx.wb.sheets.iter().all(|s| s.tables.is_empty()) {
        return;
    }
    // (sheet, table name, table range, old column name, new column name)
    let mut renames: Vec<(usize, String, RangeRef, String, String)> = Vec::new();
    // Every table whose header row to reconcile: all of them after a structural edit (it can move
    // anything into a header row), else the ones a changed cell falls in.
    let mut targets: Vec<(usize, usize)> = Vec::new();
    for si in 0..cx.wb.sheets.len() {
        let Some(sh) = cx.wb.sheet(si) else { continue };
        for (ti, t) in sh.tables.iter().enumerate() {
            let hit = cx.structural || cx.changed.iter().any(|(s, c)| *s == si && c.row == t.range.start.row && t.range.contains(*c));
            if hit && t.header_row {
                targets.push((si, ti));
            }
        }
    }
    for (si, ti) in targets {
        sync_table_headers(cx, si, ti, &mut renames);
    }
    if renames.is_empty() {
        return;
    }
    // Structured references follow the new names.
    for i in 0..cx.wb.sheets.len() {
        let formulas: Vec<(CellRef, std::sync::Arc<Formula>)> =
            cx.wb.sheets.get(i).map(|s| s.cells.iter().filter_map(|(c, x)| x.formula.clone().map(|f| (c, f))).collect()).unwrap_or_default();
        for (c, f) in formulas {
            let Some(e) = f.expr() else { continue };
            let mut hit = false;
            let ne = e.map(&mut |x| match x {
                gridcraft_formula::Expr::Struct(mut st) => {
                    for (si, table, range, old, name) in &renames {
                        let this = if st.table.is_empty() { i == *si && range.contains(c) } else { st.table.eq_ignore_ascii_case(table) };
                        if this {
                            for end in [&mut st.col_start, &mut st.col_end].into_iter().flatten() {
                                if end.eq_ignore_ascii_case(old) {
                                    *end = name.clone();
                                    hit = true;
                                }
                            }
                        }
                    }
                    gridcraft_formula::Expr::Struct(st)
                }
                o => o,
            });
            if hit
                && let Some(sh) = cx.wb.sheet_mut(i)
                && let Some(mut cell) = sh.cells.get(c)
            {
                cell.formula = Some(std::sync::Arc::new(Formula::from_expr(ne)));
                sh.cells.set(c, cell);
            }
        }
    }
    cx.structural = true;
}

/// Reconciles one table's column names with its header cells and records the renames, so
/// structured references can follow them.
fn sync_table_headers(cx: &mut crate::cmd::Ctx, si: usize, ti: usize, renames: &mut Vec<(usize, String, RangeRef, String, String)>) {
    let Some(sh) = cx.wb.sheet(si) else { return };
    let Some(t) = sh.tables.get(ti) else { return };
    if !t.header_row {
        return;
    }
    let (range, table) = (t.range, t.name.clone());
    let old: Vec<String> = t.columns.iter().map(|c| c.name.clone()).collect();
    // Each header's text and whether it is constant text. A number format only changes how a text
    // header looks (an accounting format pads it with spaces); anything else — a number, a formula
    // or a spilled value — becomes its text, because a header holds constant text.
    let mut texts: Vec<Option<String>> = Vec::with_capacity(range.width() as usize);
    let mut is_text: Vec<bool> = Vec::with_capacity(range.width() as usize);
    for c in range.start.col..=range.end.col {
        let value = sh.value(CellRef::new(range.start.row, c));
        let constant = value.is_text() && sh.cell(CellRef::new(range.start.row, c)).is_some_and(|x| x.formula.is_none());
        texts.push(match value.as_text() {
            Some(t) => Some(t.to_string()),
            None => Some(crate::display::cell_text(&cx.wb, sh, CellRef::new(range.start.row, c))),
        });
        is_text.push(constant);
    }
    let names = header_names(&texts);
    // (column offset, name, whether the cell must be written back). A column the model does not
    // have (a hostile file's table part can name fewer columns than its range is wide) keeps its
    // name out of the table; the writer then treats it as blank.
    let mut plan: Vec<(usize, String, bool)> = Vec::new();
    for (i, name) in names.into_iter().enumerate() {
        let Some(old_name) = old.get(i) else { continue };
        let text = texts.get(i).and_then(|t| t.as_deref()).unwrap_or_default();
        let write = name != text || !is_text.get(i).copied().unwrap_or(false);
        if *old_name == name && !write {
            continue;
        }
        plan.push((i, name, write));
    }
    if plan.is_empty() {
        return;
    }
    let Some(sh) = cx.wb.sheet_mut(si) else { return };
    if let Some(t) = sh.tables.get_mut(ti) {
        for (i, name, _) in &plan {
            if let Some(c) = t.columns.get_mut(*i) {
                c.name = name.clone();
            }
        }
    }
    for (i, name, write) in &plan {
        if !*write {
            continue;
        }
        let cell = CellRef::new(range.start.row, range.start.col.saturating_add(*i as u32));
        let style = sh.style_id(cell);
        sh.cells.set(cell, Cell { value: gridcraft_core::Value::text(name.as_str()), style, ..Default::default() });
    }
    for (i, name, _) in &plan {
        if let Some(old_name) = old.get(*i)
            && !old_name.eq_ignore_ascii_case(name)
        {
            renames.push((si, table.clone(), range, old_name.clone(), name.clone()));
        }
    }
}

#[cfg(test)]
mod tests {
    use gridcraft_core::Value;
    use serde_json::json;

    use crate::Session;

    #[test]
    fn editing_a_header_renames_the_column() {
        let mut s = Session::new();
        s.new_workbook();
        s.execute("range.setValues", json!({"range": "A1", "values": [["a", "b", "c"], [1, 2, 3]]})).unwrap();
        s.execute("insert.table", json!({"range": "A1:C2", "header": true})).unwrap();
        s.execute("cell.set", json!({"cell": "E1", "input": "=SUM(Table1[a])"})).unwrap();
        s.execute("cell.set", json!({"cell": "A1", "input": "zz"})).unwrap();
        s.execute("edit.clearContents", json!({"range": "B1"})).unwrap();
        s.execute("cell.set", json!({"cell": "C1", "input": "ZZ"})).unwrap();
        let get = |s: &mut Session, a: &str| s.execute("cell.get", json!({"cell": a})).unwrap();
        assert_eq!(get(&mut s, "E1")["formula"], "=SUM(Table1[zz])");
        assert_eq!(get(&mut s, "E1")["value"], json!(1.0));
        let d = s.doc().unwrap();
        let sh = d.wb.sheet(0).unwrap();
        let names: Vec<&str> = sh.tables[0].columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["zz", "Column2", "ZZ2"]);
        assert_eq!(sh.value(gridcraft_core::CellRef::new(0, 1)), Value::from("Column2"));
        assert_eq!(sh.value(gridcraft_core::CellRef::new(0, 2)), Value::from("ZZ2"));
        s.execute("range.setValues", json!({"range": "A1", "values": [["x", "x", 5]]})).unwrap();
        let sh = s.doc().unwrap().wb.sheet(0).unwrap();
        let names: Vec<&str> = sh.tables[0].columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["x", "x2", "5"]);
        assert_eq!(sh.value(gridcraft_core::CellRef::new(0, 2)), Value::from("5"));
    }

    #[test]
    fn go_to_a_table_selects_its_data() {
        let mut s = Session::new();
        s.new_workbook();
        s.execute("range.setValues", json!({"range": "A1", "values": [["a", "b"], [1, 2], [3, 4]]})).unwrap();
        s.execute("insert.table", json!({"range": "A1:B3", "header": true})).unwrap();
        assert_eq!(s.execute("edit.goTo", json!({"reference": "Table1"})).unwrap()["selection"], "A2:B3");
    }
}

#[cfg(test)]
mod style_tests {
    use super::*;

    #[test]
    fn zero_numbered_styles_do_not_underflow() {
        assert_eq!(parse_style_name("TableStyleLight0"), ("Light", 1));
        assert_eq!(parse_style_name("TableStyleMedium0"), ("Medium", 1));
        for name in ["TableStyleLight0", "TableStyleMedium0", "TableStyleDark0"] {
            let _ = look(name);
        }
    }
}
