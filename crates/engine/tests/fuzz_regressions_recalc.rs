//! Regression tests for the recalc findings of the 400-seed command-sequence soak: after an
//! incremental edit the value in memory disagreed with a full recalculation.
//!
//! Each test replays the sequence the fuzzer shrank to (or a smaller hand-built one with the same
//! mechanism), then asserts the two paths agree *and* that the value they agree on is the right
//! one.

use std::collections::BTreeSet;

use gridcraft_core::{CellError, CellRef, Value};
use gridcraft_engine::Session;
use gridcraft_engine::calc::Calc;
use gridcraft_engine::model::Workbook;
use serde_json::{Value as Json, json};

fn session(sample: &str) -> Session {
    let mut s = Session::new();
    s.execute("file.new", json!({"sample": sample})).expect("sample opens");
    s
}

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

/// A full recalculation from a fresh dependency graph.
fn recalc_all(wb: &Workbook) -> Workbook {
    let mut fresh = wb.clone();
    Calc::new().recalc_all(&mut fresh);
    fresh
}

/// The first cell where the incrementally maintained values and a full recalc disagree.
fn recalc_diff(wb: &Workbook) -> Option<String> {
    let fresh = recalc_all(wb);
    for (x, y) in wb.sheets.iter().zip(&fresh.sheets) {
        let mut keys: BTreeSet<CellRef> = BTreeSet::new();
        for s in [x, y] {
            keys.extend(s.cells.iter().filter(|(_, c)| !c.value.is_empty() || c.formula.is_some()).map(|(c, _)| c));
            keys.extend(s.spill.keys().copied());
        }
        for c in keys {
            let (va, vb) = (x.value(c), y.value(c));
            if va != vb {
                let f = x
                    .cell(c)
                    .and_then(|c| c.formula)
                    .map(|f| format!(" (formula ={})", f.text))
                    .or_else(|| {
                        let (anchor, _) = x.spill_ranges.iter().find(|(_, r)| r.contains(c))?;
                        let f = x.cell(*anchor).and_then(|c| c.formula)?;
                        Some(format!(" (spilled from {} ={})", anchor.a1(), f.text))
                    })
                    .unwrap_or_default();
                return Some(format!("{}!{c}{f}: {va:?} in memory, {vb:?} after a full recalc", x.name));
            }
        }
    }
    None
}

fn assert_recalc_agrees(s: &Session) -> Workbook {
    let wb = wb(s);
    if let Some(diff) = recalc_diff(&wb) {
        panic!("incremental recalculation disagrees with a full one: {diff}");
    }
    wb
}

/// Seed 378. The full recalc was the wrong side: the store's per-row wide table aliased values
/// once it grew past 65,535 entries, so rewriting `A23` read another cell's text back.
#[test]
fn seed_378_insert_columns_keeps_a_concatenation_with_an_empty_cell_empty() {
    let mut s = session("sales");
    run(&mut s, "selection.set", json!({"range": "D21:G28"}));
    run(&mut s, "range.fill", json!({"input": "=I23&\"-\"&H6", "range": "23:23"}));
    run(&mut s, "home.deleteColumns", json!({"cols": "I:J"}));
    run(&mut s, "data.filterBy", json!({"column": "F", "values": ["275", "2026-01-02"]}));
    run(&mut s, "home.deleteCells", json!({"range": "B2:B9", "shift": "column"}));
    run(&mut s, "home.deleteColumns", json!({"cols": "K:L"}));
    run(&mut s, "home.insertColumns", json!({"cols": "B:B"}));
    run(&mut s, "home.insertColumns", json!({"cols": "G:H"}));

    assert_recalc_agrees(&s);
    assert_eq!(formula(&s, "A23").as_deref(), Some("F25&\"-\"&E8"));
    assert_eq!(value(&s, "F25"), Value::Empty);
    assert_eq!(value(&s, "E8"), Value::Number(70150.0));
    assert_eq!(value(&s, "A23"), Value::Text("-70150".into()));
}
/// Seed 229. The full recalc was the wrong side: the `SORT` anchor turned out circular after its
/// array had been read, and the values read from the discarded array stayed in the reader.
#[test]
fn seed_229_a_spilled_sort_that_is_circular_leaves_no_values_behind() {
    let mut s = session("budget");
    run(&mut s, "range.fill", json!({"input": "=COUNTIF(G:G,\">0\")", "range": "B8:F14"}));
    run(&mut s, "home.insertColumns", json!({"cols": "D:E"}));
    run(&mut s, "home.deleteColumns", json!({"cols": "B:B"}));
    run(&mut s, "cell.set", json!({"cell": "K16", "input": "=SORT(D7:G12)"}));

    assert_recalc_agrees(&s);
    assert_eq!(formula(&s, "F13").as_deref(), Some("COUNTIF(L:L,\">0\")"));
    // The anchor is circular, so nothing it would have spilled exists: column L holds no number.
    assert_eq!(value(&s, "K16"), Value::Error(CellError::Circ));
    let wb = wb(&s);
    assert!(wb.active().expect("sheet").spill_ranges.is_empty(), "a circular array spills nothing");
    assert_eq!(value(&s, "F13"), Value::Number(0.0));
}
/// Seed 320. The incremental pass was the wrong side: `=23:26` is read through the sheet's used
/// range, and the last edit moved that range's right edge, so the array no longer fits.
#[test]
fn seed_320_offset_into_a_whole_row_reference_spills_like_a_full_recalc_says() {
    let mut s = session("grades");
    run(
        &mut s,
        "range.setValues",
        json!({"range": "K5", "values": [[null, false], ["=IF(B14>2,\"big\",B14)", "=A23:A26"], [true, "=SUM(H11:L17)"], [153, false]]}),
    );
    run(&mut s, "edit.replace", json!({"all": true, "what": "A", "with": ""}));
    run(&mut s, "cell.set", json!({"cell": "XFB6", "input": "=OFFSET(F22,1,1)"}));

    assert_recalc_agrees(&s);
    assert_eq!(formula(&s, "L6").as_deref(), Some("23:26"));
    assert_eq!(value(&s, "L6"), Value::Error(CellError::Spill));
}
/// Seed 229's replay at the current generator (the fuzzer's 6-step shrink): the `SORT`'s spill
/// moved under a defined name, then the `SORT` turned circular. The name hides its range from the
/// graph, so nothing seeded the sum; a spill change has to evaluate the sheet's dynamic formulas
/// again.
#[test]
fn seed_229_a_defined_name_reading_a_discarded_spill_is_evaluated_again() {
    let mut s = session("budget");
    run(&mut s, "formulas.defineName", json!({"name": "Totals", "refersTo": "='Budget'!$K$10:$N$17"}));
    run(&mut s, "home.insertColumns", json!({"cols": "I:I"}));
    run(&mut s, "cell.set", json!({"cell": "F56", "input": "=SUM(Totals)"}));
    run(&mut s, "cell.set", json!({"cell": "K16", "input": "=SORT(D7:G12)"}));
    run(&mut s, "home.deleteRows", json!({"rows": "8:9"}));
    run(&mut s, "edit.replace", json!({"all": true, "what": "1", "with": ""}));

    assert_recalc_agrees(&s);
    assert_eq!(formula(&s, "F54").as_deref(), Some("SUM(Totals)"));
    assert_eq!(value(&s, "F54"), Value::Number(0.0));
}

/// Seed 229 as the soak found it: the fill wrote `COUNTIF(D:D)` into D8:D14, counting the column
/// that holds the formulas themselves. A formula that reads its own cell is circular, and it has
/// to be circular for the formulas evaluated after it in the same pass too: the `SORT` and the
/// `COUNTIF`s fed each other and the pass ended with the `SORT`'s array from the previous round.
#[test]
fn seed_229_a_formula_that_reads_its_own_cell_is_circular_in_the_same_pass() {
    let mut s = session("budget");
    run(&mut s, "edit.cut", json!({"range": "C21:E22"}));
    run(&mut s, "edit.paste", json!({"at": "G5"}));
    run(&mut s, "range.fill", json!({"input": "=COUNTIF(G:G,\">0\")", "range": "B8:F14"}));
    run(&mut s, "home.deleteRows", json!({"rows": "14:14"}));
    run(&mut s, "home.mergeAcross", json!({}));
    run(&mut s, "edit.clearComments", json!({}));
    run(&mut s, "edit.undo", json!({"steps": 3}));
    run(&mut s, "home.insertRows", json!({"rows": "15:15"}));
    run(&mut s, "cell.set", json!({"cell": "L12", "input": "=INDIRECT(\"I15\")"}));
    run(&mut s, "cell.set", json!({"cell": "K16", "input": "=SORT(D7:G12)"}));

    assert_recalc_agrees(&s);
    assert_eq!(value(&s, "D8"), Value::Error(CellError::Circ));
    assert_eq!(value(&s, "K16"), Value::Number(212.8));
}
