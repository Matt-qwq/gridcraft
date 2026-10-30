//! Structural edits and fills are timed: the work has to follow the cells the command moves, not
//! the sheet's stored cells or the bounding box of its used range.
//!
//! Seed 104 of `command_sequence_fuzz` built a sheet whose used range is a million rows tall (one
//! stray cell at the bottom of column Z) and then filled `B:C`, which wrote 2,097,102 cells for the
//! 26 rows those two columns hold; every command after that ran on a two-million-cell workbook.
//! Cutting a column and pasting it was quadratic in the cells moved (`paste_special` looked each
//! source cell up with a linear scan of the moved block).
//!
//! The budgets are wall clock on the development machine (a debug `cargo test`), about ten times the
//! measured cost, so a loaded machine has room and a change that turns a linear pass into a scan of
//! the sheet (or a quadratic one) still fails. A failure prints the measured milliseconds.
//!
//! These use `Instant`, which the workspace bans on wasm (it traps there); they never run on wasm.

use std::time::{Duration, Instant};

use gridcraft_core::CellRef;
use gridcraft_engine::Session;
use serde_json::json;

fn session() -> Session {
    let mut s = Session::new();
    s.execute("file.new", json!({"sample": "grades"})).expect("sample opens");
    s
}

fn cells(s: &Session) -> usize {
    s.doc().expect("document").wb.sheets.iter().map(|sh| sh.cells.len()).sum()
}

fn millis(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// A fill over whole columns covers the content in those columns, not the sheet's used-range box.
/// One cell at the bottom of another column used to stretch it to 1,048,576 rows.
#[test]
#[allow(clippy::disallowed_methods)] // the clock, never on wasm
fn a_whole_column_fill_stays_inside_its_own_lines() {
    let mut s = session();
    s.execute("range.setValues", json!({"range": "B1", "values": [["x", "y"]]})).expect("B1:C1");
    s.execute("cell.set", json!({"cell": "B26", "input": "end"})).expect("B26");
    // The fuzz's stray cell: it makes the used range a million rows tall without putting anything
    // in the columns being filled.
    s.execute("cell.set", json!({"cell": "Z1048576", "input": "stray"})).expect("Z1048576");
    let before = cells(&s);

    let t = Instant::now();
    s.execute("edit.fillDown", json!({"range": "B:C"})).expect("fill down");
    let ms = millis(t.elapsed());
    let after = cells(&s);

    // B1 and C1 down to row 26: 50 cells, not two million.
    assert!(after < before + 64, "Fill Down on B:C wrote {} cells for the 26 rows those columns hold ({} ms)", after - before, ms);
    assert!(ms < 300.0, "Fill Down on B:C took {ms:.1} ms; the 50 cells it fills measured under 1 ms");
    let wb = &s.doc().expect("document").wb;
    let sh = &wb.sheets[0];
    assert_eq!(sh.cell(CellRef::new(25, 1)).map(|c| c.value.clone()), sh.cell(CellRef::new(0, 1)).map(|c| c.value.clone()), "B26 takes B1's value");
    assert!(sh.cell(CellRef::new(26, 1)).is_none(), "nothing is written past the content of the filled columns");
}

/// Deleting a column on a million-row sheet moves that column: it does not walk every cell twice
/// over, and it does not re-evaluate an empty column through the whole sheet.
#[test]
#[allow(clippy::disallowed_methods)] // the clock, never on wasm
fn deleting_a_column_is_proportional_to_the_column_not_the_sheet() {
    let mut s = session();
    s.execute("range.fill", json!({"range": "A1:A1048576", "input": "1"})).expect("a million rows");
    assert!(cells(&s) >= 1_048_576, "the sheet holds a million cells");

    let t = Instant::now();
    s.execute("home.deleteColumns", json!({"cols": "F:F"})).expect("delete an empty column");
    let empty_ms = millis(t.elapsed());
    assert!(empty_ms < 500.0, "deleting the empty column F on a million-row sheet took {empty_ms:.1} ms (measured 32 ms)");
    assert!(cells(&s) >= 1_048_576, "deleting an empty column keeps the sheet's cells");

    let t = Instant::now();
    s.execute("home.deleteColumns", json!({"cols": "A:A"})).expect("delete the filled column");
    let filled_ms = millis(t.elapsed());
    assert!(filled_ms < 2_500.0, "deleting a column of a million cells took {filled_ms:.1} ms (measured 277 ms)");
    assert!(cells(&s) < 100, "the deleted column's cells are gone ({} left)", cells(&s));
}

/// Cutting a column and pasting it scales with the cells moved. The paste looked every source cell
/// up with a linear scan of the moved block, so 200,000 rows took seconds and a real column of a
/// million rows did not finish.
#[test]
#[allow(clippy::disallowed_methods)] // the clock, never on wasm
fn cutting_and_pasting_a_column_is_linear_in_the_cells_it_moves() {
    let mut s = session();
    s.execute("range.fill", json!({"range": "A1:A200000", "input": "7"})).expect("200,000 rows");

    let t = Instant::now();
    s.execute("edit.cut", json!({"range": "A:A"})).expect("cut the column");
    let cut_ms = millis(t.elapsed());
    assert!(cut_ms < 1_500.0, "cutting a column of 200,000 cells took {cut_ms:.1} ms (measured 139 ms)");

    let t = Instant::now();
    s.execute("edit.paste", json!({"at": "B1"})).expect("paste it one column over");
    let paste_ms = millis(t.elapsed());
    assert!(paste_ms < 2_000.0, "pasting 200,000 cut cells took {paste_ms:.1} ms (measured 171 ms; the quadratic scan took about 8 s)");

    let wb = &s.doc().expect("document").wb;
    let sh = &wb.sheets[0];
    assert!(sh.cell(CellRef::new(0, 1)).is_some(), "the first row landed in B1");
    assert!(sh.cell(CellRef::new(199_999, 1)).is_some(), "the last row landed in B200000");
}
