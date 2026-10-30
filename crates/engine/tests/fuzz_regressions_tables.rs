//! Regressions from the command-sequence soak's table-name corruption: a save and reload turned
//! `Sheet1!M12` from `Column3` into `Column32` (seed 346, after `table.resize` followed by
//! `home.deleteRows`; the earlier hard-coded replay reached it too).
//!
//! The shape of the hole: `table.resize` built the resized table's column list from the header
//! cells but generated a blank header's name from its position alone, so a table resized one
//! column to the right got two `Column3`s. Nothing in memory disagreed — the header cells held
//! the duplicate names too — and the XLSX writer, which must emit unique names, renamed the later
//! one on the way out (`Column3` → `Column32`) and wrote it into the header cell. So the rename
//! only happened at the file boundary, where nothing can undo it.
//!
//! The rule these tests pin down: a structural edit never leaves two table columns with the same
//! name, the header cells always hold the column names, and a rename (when it is unavoidable) is
//! made in memory, on the later column, by position — never silently on save.

use gridcraft_core::CellRef;
use gridcraft_engine::model::{Table, TableColumn, TotalsFn, Workbook};
use gridcraft_engine::{DocState, Session, io};
use serde_json::json;

fn session() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s
}

fn workbook(s: &Session) -> Workbook {
    (*s.doc().expect("document").wb).clone()
}

fn table(wb: &Workbook) -> Table {
    wb.active().and_then(|sh| sh.tables.first().cloned()).expect("a table")
}

fn names(t: &Table) -> Vec<String> {
    t.columns.iter().map(|c| c.name.clone()).collect()
}

/// The header row's cell texts, in column order.
fn header_cells(wb: &Workbook, t: &Table) -> Vec<String> {
    let Some(sh) = wb.active() else { return Vec::new() };
    (t.range.start.col..=t.range.end.col).map(|c| sh.value(CellRef::new(t.range.start.row, c)).display()).collect()
}

/// Everything a save and reload must give back about the tables' names.
fn header_state(wb: &Workbook) -> Vec<(Vec<String>, Vec<String>)> {
    let Some(sh) = wb.active() else { return Vec::new() };
    sh.tables.iter().map(|t| (names(t), header_cells(wb, t))).collect()
}

fn reload(wb: &Workbook) -> Workbook {
    let bytes = io::save_bytes(wb, "regression.xlsx").expect("save");
    // Every save in these tests goes through the package validator.
    gridcraft_xlsx::testing::check_package(&bytes).expect("a valid package");
    io::open_bytes("regression.xlsx", &bytes).expect("reload").0
}

/// One table column with every optional part at its default.
fn column(name: &str) -> TableColumn {
    TableColumn { name: name.into(), totals: TotalsFn::None, totals_label: None, formula: None, unique_name: None, xml: None }
}

/// A workbook whose first sheet holds a table over `A1:{last_col}3` with `header: true`, its
/// header cells and column names as given (a model the XLSX reader can produce from a file).
fn opened_with_table(last_col: &str, headers: &[&str]) -> Session {
    let mut wb = Workbook::new();
    if let Some(sh) = wb.sheet_mut(0) {
        for (i, h) in headers.iter().enumerate() {
            sh.set_value(CellRef::new(0, i as u32), gridcraft_core::Value::text(*h));
            sh.set_value(CellRef::new(1, i as u32), gridcraft_core::Value::Number(i as f64 + 1.0));
        }
        sh.tables.push(Table {
            id: 1,
            name: "T".into(),
            range: gridcraft_core::RangeRef::parse(&format!("A1:{last_col}3")).expect("range"),
            header_row: true,
            totals_row: false,
            columns: headers.iter().map(|h| column(h)).collect(),
            style: "TableStyleMedium2".into(),
            banded_rows: true,
            banded_cols: false,
            first_col: false,
            last_col: false,
            filter_button: true,
            filter: vec![],
            table_type: None,
            connection_id: None,
        });
    }
    let mut s = Session::new();
    s.add_document(DocState::new(wb, None, "tables".into()));
    s
}

/// The names are unique (case-insensitively, as the file format requires), non-empty, and the
/// header cells hold exactly them.
fn assert_named(wb: &Workbook) {
    let t = table(wb);
    let names = names(&t);
    assert!(names.iter().all(|n| !n.trim().is_empty()), "empty column name: {names:?}");
    let mut unique: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), names.len(), "two columns share a name: {names:?}");
    assert_eq!(header_cells(wb, &t), names, "the header cells hold the column names");
}

#[test]
fn a_table_resized_over_a_blank_column_gets_a_free_name() {
    // The minimal replay the soak's shrinker found: a table over blank cells (generated names
    // Column1, Column2, Column3 at J12, K12, L12) resized one column right. The new column's
    // blank header used to be given the third position's "Column3", which the second column (the
    // old Column3) already had.
    let mut s = session();
    s.execute("insert.table", json!({"range": "J12:L15", "header": true})).expect("insert table");
    s.execute("table.resize", json!({"range": "K12:M17", "table": "Table1"})).expect("resize");

    let wb = workbook(&s);
    assert_named(&wb);
    // The first two columns keep the names they had; the blank one takes the next free number.
    assert_eq!(names(&table(&wb)), ["Column2", "Column3", "Column4"]);
    assert_eq!(header_state(&wb), header_state(&reload(&wb)));
}

#[test]
fn the_soak_sequence_resize_then_delete_rows_keeps_its_names() {
    // Seed 346's last three relevant commands (`insert.table` on blank cells, `table.resize`, then
    // `home.deleteRows 17:18`), which is where the save reported
    // `Sheet1!M12: Column3 in memory, Column32 after save and reload`.
    let mut s = session();
    s.execute("insert.table", json!({"range": "J12:L15", "header": true})).expect("insert table");
    s.execute("table.resize", json!({"range": "K12:M17", "table": "Table1"})).expect("resize");
    s.execute("home.deleteRows", json!({"rows": "17:18"})).expect("delete rows");

    let wb = workbook(&s);
    assert_named(&wb);
    assert_eq!(header_state(&wb), header_state(&reload(&wb)));
    assert_eq!(wb.active().expect("sheet").value(CellRef::new(11, 12)).display(), "Column4");
    assert_eq!(reload(&wb).active().expect("sheet").value(CellRef::new(11, 12)).display(), "Column4");
}

#[test]
fn typing_a_header_keeps_every_name_unique() {
    // Editing a header cell names the column as it is typed; a name the row already uses is taken
    // by position, so the later column gets the suffix (the writer's rule) — and it happens
    // in memory, not on save.
    let mut s = session();
    s.execute("range.setValues", json!({"range": "A1", "values": [["a", "b", "c"], [1, 2, 3]]})).expect("values");
    s.execute("insert.table", json!({"range": "A1:C2", "header": true})).expect("insert table");
    s.execute("cell.set", json!({"cell": "C1", "input": "a"})).expect("type a duplicate");

    let wb = workbook(&s);
    assert_named(&wb);
    assert_eq!(names(&table(&wb)), ["a", "b", "a2"]);
    assert_eq!(header_state(&wb), header_state(&reload(&wb)));

    // A duplicate the workbook already had (as a file with repeated `<tableColumn name>` gives
    // it) is repaired by the same edit, even when the edited cell is another column: before this,
    // only the edited cells were looked at, so both halves of the duplicate matched their own
    // column and the writer renamed one of them on save.
    let mut s = opened_with_table("D", &["x", "y", "y", "z"]);
    s.execute("cell.set", json!({"cell": "D1", "input": "w"})).expect("rename another header");
    let wb = workbook(&s);
    assert_named(&wb);
    assert_eq!(names(&table(&wb)), ["x", "y", "y2", "w"]);
    assert_eq!(header_state(&wb), header_state(&reload(&wb)));
}

#[test]
fn a_structural_edit_repairs_a_duplicate_and_its_structured_reference() {
    // A table whose model holds a duplicate (as a file with repeated `<tableColumn name>` gives
    // it) and a structured reference to the repeated name. A structural edit elsewhere used to
    // leave the duplicate alone — both header cells matched their own column's name — and the
    // save renamed the later one, so the reloaded sheet no longer matched the sheet in memory.
    let mut s = opened_with_table("C", &["x", "y", "y"]);
    s.execute("cell.set", json!({"cell": "E1", "input": "=SUM(T[y])"})).expect("formula");
    s.execute("home.insertRows", json!({"rows": "5:5"})).expect("insert rows");

    let wb = workbook(&s);
    assert_named(&wb);
    assert_eq!(names(&table(&wb)), ["x", "y", "y2"]);
    let sh = wb.active().expect("sheet");
    let formula = sh.cell(CellRef::new(0, 4)).and_then(|c| c.formula.map(|f| f.text.clone()));
    assert_eq!(formula.as_deref(), Some("SUM(T[y2])"));
    assert_eq!(header_state(&wb), header_state(&reload(&wb)));
}

#[test]
fn hostile_column_names_do_not_panic_or_stay_duplicated() {
    // Names a hostile file can hold: one as long as a number can be, and a blank that would make
    // the "next free ColumnN" search walk to the end of `u64` if it were unbounded.
    let huge = format!("Column{}", u64::MAX);
    let mut s = opened_with_table("C", &[huge.as_str(), "", "Column2"]);
    s.execute("home.insertRows", json!({"rows": "5:5"})).expect("insert rows");

    let wb = workbook(&s);
    assert_named(&wb);
    assert_eq!(names(&table(&wb)), [huge, "Column2".to_string(), "Column22".to_string()]);
    assert_eq!(header_state(&wb), header_state(&reload(&wb)));
}
