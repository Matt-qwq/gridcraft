//! Regressions from the 400-seed command-sequence soak: two `[structure]` failures (a legacy array
//! whose range no longer starts at its formula cell after `data.removeDuplicates`).
//!
//! The invariants are the fuzz harness's own, and so is the comparison ([`fuzz_common`]).

use gridcraft_core::CellRef;
use gridcraft_engine::Session;
use serde_json::json;

/// The array ranges on a sheet, as `(anchor, covered range)` for every legacy array formula.
fn arrays(s: &Session) -> Vec<(String, String)> {
    let d = s.doc().expect("document");
    let mut out = Vec::new();
    for sh in &d.wb.sheets {
        for (c, cell) in sh.cells.iter() {
            if let Some(a) = cell.formula.as_ref().and_then(|f| f.array) {
                out.push((format!("{}!{c}", sh.name), a.a1()));
            }
        }
    }
    out
}

/// The fuzz harness's `[structure]` rule for legacy arrays: the formula lives in the top-left cell
/// of the range it covers, on the sheet.
fn array_problem(s: &Session) -> Option<String> {
    for sh in &s.doc().expect("document").wb.sheets {
        for (c, cell) in sh.cells.iter() {
            if let Some(a) = cell.formula.as_ref().and_then(|f| f.array) {
                let on_sheet = a.start.is_valid() && a.end.is_valid() && a.start.row <= a.end.row && a.start.col <= a.end.col;
                if a.start != c || !on_sheet {
                    return Some(format!("{}!{c}: array formula covers {a}", sh.name));
                }
            }
        }
    }
    None
}

fn value(s: &Session, at: &str) -> String {
    let c = CellRef::parse(at).expect("cell reference");
    s.doc().expect("document").wb.active().expect("sheet").value(c).display()
}

/// A session on a blank workbook with column E holding keys and column F holding values, so
/// `data.removeDuplicates` has something to compact.
fn keys_and_values() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s.execute(
        "range.setValues",
        json!({"range": "E9:F16", "values": [
            ["k1", "a"], ["k1", "a"], ["k3", "c"], ["k4", "d"],
            ["k5", "e"], ["k6", "f"], ["k7", "g"], ["k8", "h"]
        ]}),
    )
    .expect("values");
    s
}

/// A one-cell legacy array at `at` (Ctrl+Shift+Enter over that single cell).
fn one_cell_array(s: &mut Session, at: &str) {
    s.execute("selection.set", json!({"range": at})).expect("selection");
    s.execute("cell.set", json!({"cell": at, "input": "=1+1", "array": true})).expect("array");
}

#[test]
fn remove_duplicates_moves_a_one_cell_array_with_its_anchor() {
    // Seed 260/285: the compaction moved the anchor of a legacy array up and left the range it
    // covers where the array used to be, so the formula covered an unrelated cell.
    let mut s = keys_and_values();
    one_cell_array(&mut s, "F14");
    assert_eq!(arrays(&s), [("Sheet1!F14".to_string(), "F14".to_string())]);

    // Row 10 is a duplicate of row 9, so rows 11.. move up one: F14's anchor lands on F13.
    let r = s.execute("data.removeDuplicates", json!({"range": "E9:F16", "header": false})).expect("remove duplicates");
    assert_eq!(r["removed"], 1);
    assert_eq!(array_problem(&s), None);
    assert_eq!(arrays(&s), [("Sheet1!F13".to_string(), "F13".to_string())]);
    // The formula and its value travel with the anchor.
    assert_eq!(value(&s, "F13"), "2");
}

#[test]
fn remove_duplicates_keeps_a_multi_cell_array_whole() {
    let mut s = keys_and_values();
    s.execute("selection.set", json!({"range": "F13:F14"})).expect("selection");
    s.execute("cell.set", json!({"cell": "F13", "input": "=SEQUENCE(2)", "array": true})).expect("array");
    assert_eq!(arrays(&s), [("Sheet1!F13".to_string(), "F13:F14".to_string())]);

    let r = s.execute("data.removeDuplicates", json!({"range": "E9:F16", "header": false})).expect("remove duplicates");
    assert_eq!(r["removed"], 1);
    assert_eq!(array_problem(&s), None);
    // Both rows of the array survive and stay adjacent, so the whole array moves up one row.
    assert_eq!(arrays(&s), [("Sheet1!F12".to_string(), "F12:F13".to_string())]);
    assert_eq!((value(&s, "F12"), value(&s, "F13")), ("1".to_string(), "2".to_string()));
}

#[test]
fn remove_duplicates_refuses_to_cut_an_array_apart() {
    // The array is cut by a removed row: keeping the formula's values would need an array that
    // covers a cell it does not own, so the compaction is refused and nothing changes (Excel:
    // "You can't change part of an array.").
    let mut s = Session::new();
    s.new_workbook();
    s.execute(
        "range.setValues",
        json!({"range": "E9:F16", "values": [
            ["k1", "a"], ["k2", "b"], ["k3", "c"], ["k3", "d"],
            ["k5", "e"], ["k6", "f"], ["k7", "g"], ["k8", "h"]
        ]}),
    )
    .expect("values");
    s.execute("selection.set", json!({"range": "F11:F13"})).expect("selection");
    s.execute("cell.set", json!({"cell": "F11", "input": "=SEQUENCE(3)", "array": true})).expect("array");

    // Comparing column E alone makes row 12 (F12, the array's middle row) a duplicate of row 11.
    let err = s.execute("data.removeDuplicates", json!({"range": "E9:F16", "header": false, "columns": ["E"]})).expect_err("refused");
    assert!(err.to_string().contains("part of an array"), "{err}");
    assert_eq!(array_problem(&s), None);
    assert_eq!(arrays(&s), [("Sheet1!F11".to_string(), "F11:F13".to_string())]);
    assert_eq!((value(&s, "F11"), value(&s, "F12"), value(&s, "F13")), ("1".to_string(), "2".to_string(), "3".to_string()));
}
