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
