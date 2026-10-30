//! Regression for the remaining recalc finding of the 400-seed command-sequence soak: an empty
//! cell read as a value by the incremental pass (`=N25&"-"&Q21` giving `"0-"`, seed 297).
//!
//! The mechanism is not a store slot left behind by `edit.clearContents`: an array formula spilled
//! into a cell that a formula in its *own source range* read, so the array's value depended on a
//! cell it wrote — a circular reference the static dependency graph cannot name, because the
//! spilled area is only known once the array is computed. The pass alternated between the array
//! spilling (the reader saw `"-"` in the spill and became `"--"`) and the array being blocked by
//! that `"--"` (`#SPILL!`, the reader seeing an empty cell again), and stopped after the round
//! limit on whichever state it was in. Neither state is a fixed point, so the full recalculation
//! was as wrong as memory: the right answer is `#CIRC!` for the array, no spill, and an empty
//! cell for every formula that read the would-be spill area.
//!
//! The first test is a hand-built session small enough to see the loop; the second is the sequence
//! the fuzzer shrank seed 297 to, replayed verbatim (a seed's generated sequence changes whenever
//! the command set does, so the shrink is pinned here rather than left in the harness).

use std::collections::BTreeSet;

use gridcraft_core::{CellError, CellRef, Value};
use gridcraft_engine::Session;
use gridcraft_engine::calc::Calc;
use gridcraft_engine::model::Workbook;
use serde_json::{Value as Json, json};

mod fuzz_common;

fn run(s: &mut Session, id: &str, params: Json) {
    s.execute(id, params.clone()).unwrap_or_else(|e| panic!("`{id}` {params} failed: {e}"));
}

fn wb(s: &Session) -> Workbook {
    (*s.doc().expect("document").wb).clone()
}

fn value(s: &Session, at: &str) -> Value {
    s.doc().expect("document").wb.active().expect("sheet").value(CellRef::parse(at).expect("cell"))
}

fn formula(s: &Session, at: &str) -> Option<String> {
    let wb = s.doc().expect("document").wb.clone();
    wb.active().and_then(|sh| sh.cell(CellRef::parse(at).expect("cell"))).and_then(|c| c.formula).map(|f| f.text.clone())
}

fn spill_ranges(s: &Session) -> usize {
    s.doc().expect("document").wb.active().map_or(0, |sh| sh.spill_ranges.len())
}

/// The first cell where the incrementally maintained values and a full recalc disagree.
fn recalc_diff(wb: &Workbook) -> Option<String> {
    let mut fresh = wb.clone();
    Calc::new().recalc_all(&mut fresh);
    for (x, y) in wb.sheets.iter().zip(&fresh.sheets) {
        let mut keys: BTreeSet<CellRef> = BTreeSet::new();
        for s in [x, y] {
            keys.extend(s.cells.iter().filter(|(_, c)| !c.value.is_empty() || c.formula.is_some()).map(|(c, _)| c));
            keys.extend(s.spill.keys().copied());
        }
        for c in keys {
            let (va, vb) = (x.value(c), y.value(c));
            if va != vb {
                let f = x.cell(c).and_then(|c| c.formula).map(|f| format!(" (formula ={})", f.text)).unwrap_or_default();
                return Some(format!("{}!{c}{f}: {va:?} in memory, {vb:?} after a full recalc", x.name));
            }
        }
    }
    None
}

fn assert_recalc_agrees(s: &Session, what: &str) {
    let wb = wb(s);
    if let Some(diff) = recalc_diff(&wb) {
        panic!("{what}: incremental recalculation disagrees with a full one: {diff}");
    }
}

/// The smallest version of the loop: `C1` spills `UNIQUE(A1:B2)` and `B2` — a formula in that
/// source — reads `D1` from the spill. With `A1` cleared the two source rows collide, so the
/// array becomes one row (`C1:D1`), `D1` turns `"-"` and `B2` `"--"`; that second row makes the
/// source two distinct rows again, the two-row array `C1:D2` is blocked by `C2`, `B2` drops back to
/// `"-"` — and around. That is the loop the fuzzer found through `UNIQUE` and
/// `edit.clearContents`.
#[test]
fn an_array_that_reads_its_own_spill_is_circular_and_sheds_no_values() {
    let mut s = Session::new();
    s.new_workbook();
    run(&mut s, "cell.set", json!({"cell": "A1", "input": "a"}));
    run(&mut s, "cell.set", json!({"cell": "B1", "input": "-"}));
    run(&mut s, "cell.set", json!({"cell": "B2", "input": "=D1&\"-\"&F1"}));
    run(&mut s, "cell.set", json!({"cell": "C1", "input": "=UNIQUE(A1:B2)"}));
    assert_recalc_agrees(&s, "the array spills and B2 reads its second column");
    run(&mut s, "cell.set", json!({"cell": "C2", "input": "x"}));
    assert_recalc_agrees(&s, "a blocker in the second row of the spill");
    // Two distinct rows: `C2` holds `x`, so the two-row array is blocked and spills nothing.
    assert_eq!(formula(&s, "C1").as_deref(), Some("UNIQUE(A1:B2)"));
    assert_eq!(value(&s, "C1"), Value::Error(CellError::Spill));
    assert_eq!(value(&s, "B2"), Value::Text("-".into()));

    // Clearing `A1` collapses the source rows, which is what started the alternation.
    run(&mut s, "edit.clearContents", json!({"range": "A1"}));

    assert_recalc_agrees(&s, "after clearing the source cell");
    // The array now reads a cell it would spill over: circular, and it spills nothing, so `D1` is
    // empty and `B2` concatenates two empty cells.
    assert_eq!(value(&s, "C1"), Value::Error(CellError::Circ));
    assert_eq!(value(&s, "D1"), Value::Empty);
    assert_eq!(value(&s, "B2"), Value::Text("-".into()));
    assert_eq!(spill_ranges(&s), 0, "a circular array leaves no spilled values behind");

    // A second full recalculation must land on the same answer, not on the other side of the loop.
    let before = wb(&s);
    let mut again = before.clone();
    Calc::new().recalc_all(&mut again);
    assert!(fuzz_common::content_diff(&before, &again).is_none(), "a recalculation changes values, not content");
    assert_eq!(again.active().expect("sheet").value(CellRef::parse("B2").expect("cell")), Value::Text("-".into()));
    assert_eq!(again.active().expect("sheet").value(CellRef::parse("C1").expect("cell")), Value::Error(CellError::Circ));
}

/// Seed 297 as the soak found it, shrunk by the fuzzer to seven steps: `edit.clearContents` was the
/// last step checked, but `edit.fillDown` had already left `F20` on the wrong side of the loop
/// (`"--"` in memory, `"-"` after a full recalc). Every step is checked, as the harness does, and
/// the value they must agree on is `"-"`: `J25` and `M21` are empty.
#[test]
fn seed_297_clearing_cells_leaves_an_empty_operand_empty() {
    let mut s = Session::new();
    s.execute("file.new", json!({"sample": "sales"})).expect("sample opens");

    let steps: Vec<(&str, Json)> = vec![
        ("selection.set", json!({"range": "A14:D17"})),
        ("home.deleteRows", json!({"rows": "7:7"})),
        ("range.fill", json!({"input": "=E19&\"-\"&H15", "range": "F8:J15"})),
        ("range.setValues", json!({"range": "G19", "values": [[null], ["#N/A"], ["=UNIQUE(D11:G16)"], ["x y"]]})),
        ("home.insertRows", json!({"count": 4, "rows": "1:2"})),
        ("edit.fillDown", json!({"range": "F17:I21"})),
        ("edit.clearContents", json!({"range": "B15:F15"})),
    ];
    for (i, (id, params)) in steps.iter().enumerate() {
        run(&mut s, id, params.clone());
        assert_recalc_agrees(&s, &format!("step {i} `{id}`"));
    }

    assert_eq!(formula(&s, "F20").as_deref(), Some("J25&\"-\"&M21"));
    assert_eq!(value(&s, "J25"), Value::Empty);
    assert_eq!(value(&s, "M21"), Value::Empty);
    assert_eq!(value(&s, "F20"), Value::Text("-".into()), "two empty operands concatenate to just the separator");
    assert_eq!(spill_ranges(&s), 0, "the array that fed the loop left no spilled values behind");

    // The full recalculation must not change content either: only formula results are derived.
    let before = wb(&s);
    let mut again = before.clone();
    Calc::new().recalc_all(&mut again);
    assert!(fuzz_common::content_diff(&before, &again).is_none());
}
