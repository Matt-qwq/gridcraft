//! A documentation-derived formula oracle.
//!
//! **This is a weaker oracle than Excel.** Excel is not activated on the machine this suite was
//! written on, so the expected values here are transcribed from Microsoft's published function
//! reference (learn.microsoft.com). They are *documentation-derived, not Excel-computed*: a page
//! can be stale, locale-dependent, or show a value rounded for display, and none of this proves
//! GridCraft matches the installed Excel. What it does prove is that the whole formula path
//! (parser, names, references, evaluator, function library, array results) reproduces examples
//! Microsoft published.
//!
//! Every row names the function and the documentation example it comes from. A value that could
//! not be attributed to a documented example was not added. Where the page prints a rounded value
//! (-1,037.03) the row compares at that printed precision: the tolerance is half a unit in the
//! last printed digit, never a tolerance chosen to fit what GridCraft returns. Rows the
//! documentation shows to more digits (or exact integers, text, booleans) compare at 1e-9
//! relative or exactly.
//!
//! A handful of modern array and criteria functions have documentation examples whose source
//! table cannot be rebuilt from the page. Those rows use a minimal fixture of the same shape and
//! the rule the page states, and their note says "mini table". They are marked so they are not
//! mistaken for a literal reproduction of a printed example.
//!
//! No row is held back (`#[ignore]`) at present: every documented value transcribed here matched.
//! A row that later differs is to be kept and marked `#[ignore = "documented value differs: …"]`
//! with the reason, not deleted.

// The expected values are the digits Microsoft printed (π rounded to 3.14, e to 2.718281828, …),
// not the Rust constants: keep them exactly as documented.
#![allow(clippy::approx_constant)]

use std::sync::Arc;

use gridcraft_calc::{Calc, evaluate};
use gridcraft_core::{CellError, CellRef, DateSystem, Value};
use gridcraft_model::{Cell, DefinedName, Formula, Sheet, Workbook};

// ---------------------------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------------------------

/// A documented result. Arrays are flattened row-major to stay `const`-friendly.
#[derive(Clone, Copy)]
enum Want {
    Empty,
    Num(f64),
    Text(&'static str),
    Bool(bool),
    Err(CellError),
    Arr { rows: usize, cols: usize, data: &'static [Want] },
}

#[derive(Clone, Copy)]
struct Case {
    id: &'static str,
    formula: &'static str,
    want: Want,
    /// Absolute slack for values the documentation prints rounded (e.g. `-1,037.03`).
    tol: f64,
    note: &'static str,
    fixtures: &'static [(&'static str, &'static str)],
    /// The cell the formula is entered in (matters to ROW, COLUMN, CELL).
    at: &'static str,
}

/// A documented value shown to the last digit (or an exact text/boolean/error).
const fn c(id: &'static str, formula: &'static str, want: Want, fixtures: &'static [(&'static str, &'static str)], note: &'static str) -> Case {
    Case { id, formula, want, tol: 1e-9, note, fixtures, at: "Z1" }
}

/// A value the documentation prints rounded: `tol` is half a unit in its last printed digit.
const fn r(
    id: &'static str,
    formula: &'static str,
    want: Want,
    tol: f64,
    fixtures: &'static [(&'static str, &'static str)],
    note: &'static str,
) -> Case {
    Case { id, formula, want, tol, note, fixtures, at: "Z1" }
}

/// A row whose result depends on where the formula sits.
const fn c_at(id: &'static str, formula: &'static str, want: Want, at: &'static str, note: &'static str) -> Case {
    Case { id, formula, want, tol: 1e-9, note, fixtures: &[], at }
}

fn at(s: &str) -> CellRef {
    CellRef::parse(s).unwrap_or_else(|| panic!("bad cell {s}"))
}

fn workbook(fixtures: &[(&str, &str)]) -> Workbook {
    let mut wb = Workbook::new();
    for (a, input) in fixtures {
        let cell = if input.starts_with('=') {
            Cell::formula(Formula::new(input))
        } else if let Some(e) = CellError::parse(input.trim()) {
            Cell::value(Value::Error(e))
        } else {
            Cell::value(gridcraft_core::parse::parse_input(input, wb.date_system).value)
        };
        if let Some(sh) = wb.sheet_mut(0) {
            sh.cells.set(at(a), cell);
        }
    }
    wb
}

fn near(got: f64, want: f64, tol: f64) -> bool {
    if !got.is_finite() || !want.is_finite() {
        return false;
    }
    let slack = tol.max(1e-9 * want.abs());
    (got - want).abs() <= slack
}

fn matches(want: Want, got: &Value, tol: f64) -> bool {
    match (want, got) {
        (Want::Empty, Value::Empty) => true,
        (Want::Num(w), Value::Number(g)) => near(*g, w, tol),
        (Want::Text(w), Value::Text(g)) => g.as_ref() == w,
        (Want::Bool(w), Value::Bool(g)) => *g == w,
        (Want::Err(w), Value::Error(g)) => *g == w,
        (Want::Arr { rows, cols, data }, Value::Array(a)) => {
            a.rows == rows && a.cols == cols && data.iter().enumerate().all(|(i, w)| a.data.get(i).is_some_and(|g| matches(*w, g, tol)))
        }
        _ => false,
    }
}

fn show(want: Want) -> String {
    match want {
        Want::Empty => "blank".to_string(),
        Want::Num(n) => format!("{n}"),
        Want::Text(t) => format!("{t:?}"),
        Want::Bool(b) => format!("{b}"),
        Want::Err(e) => e.as_str().to_string(),
        Want::Arr { rows, cols, .. } => format!("<{rows}x{cols} array>"),
    }
}

fn all_cases() -> Vec<Case> {
    [
        LOOKUP_CASES,
        FINANCIAL_CASES,
        DATE_CASES,
        TEXT_CASES,
        STAT_CASES,
        MATH_CASES,
        ENGINEERING_CASES,
        INFORMATION_CASES,
        DATABASE_CASES,
        LAMBDA_CASES,
        LOGICAL_CASES,
    ]
    .concat()
}

#[test]
fn documented_examples() {
    let cases = all_cases();
    let mut failures = Vec::new();
    for case in cases.iter().copied() {
        let wb = workbook(case.fixtures);
        let got = evaluate(&wb, 0, at(case.at), case.formula);
        if !matches(case.want, &got, case.tol) {
            failures.push(format!(
                "{}: {}\n    documented: {}\n    GridCraft:  {got:?}\n    source: {}",
                case.id,
                case.formula,
                show(case.want),
                case.note
            ));
        }
    }
    assert!(failures.is_empty(), "{} of {} documented examples did not match:\n\n{}", failures.len(), cases.len(), failures.join("\n"));
}

// The count gate: the suite is only meaningful if it is broad enough.
#[test]
fn suite_is_broad_enough() {
    let cases = all_cases();
    let functions = fn_coverage(&cases);
    assert!(cases.len() >= 250, "only {} rows", cases.len());
    assert!(functions >= 120, "only {functions} distinct functions");
}

/// Distinct function names the notes mention (`PMT, MS examples #3` → PMT). Notes name exactly
/// one function, so the note is the honest record of what a row exercises.
fn fn_coverage(cases: &[Case]) -> usize {
    let mut names: Vec<&str> = cases.iter().map(|case| case.note.split(',').next().unwrap_or("").trim()).filter(|name| !name.is_empty()).collect();
    names.sort_unstable();
    names.dedup();
    names.len()
}

// ---------------------------------------------------------------------------------------------
// Special forms and regressions that need a real recalculation (spills) or a richer workbook
// ---------------------------------------------------------------------------------------------

fn check(wb: &Workbook, a: &str, want: Want) {
    let got = wb.sheet(0).map(|s| s.value(at(a))).unwrap_or(Value::Empty);
    assert!(matches(want, &got, 1e-9), "{a}: expected {}, got {got:?}", show(want));
}

/// Dynamic arrays must land in the cells a formula spills into, not merely return an array.
#[test]
fn dynamic_arrays_spill_into_cells() {
    let mut wb = workbook(&[
        ("A1", "=SEQUENCE(3)"),
        ("C1", "=SORT(D1:D3)"),
        ("D1", "3"),
        ("D2", "1"),
        ("D3", "2"),
        ("F1", "=UNIQUE(G1:G3)"),
        ("G1", "1"),
        ("G2", "1"),
        ("G3", "2"),
        ("I1", "=FILTER(J1:J3,J1:J3>1)"),
        ("J1", "1"),
        ("J2", "2"),
        ("J3", "3"),
        ("L1", "=SORTBY(M1:M3,N1:N3)"),
        ("M1", "a"),
        ("M2", "b"),
        ("M3", "c"),
        ("N1", "30"),
        ("N2", "10"),
        ("N3", "20"),
    ]);
    let mut calc = Calc::new();
    calc.recalc_all(&mut wb);
    for (a, n) in [("A1", 1.0), ("A2", 2.0), ("A3", 3.0)] {
        check(&wb, a, Want::Num(n));
    }
    for (a, n) in [("C1", 1.0), ("C2", 2.0), ("C3", 3.0)] {
        check(&wb, a, Want::Num(n));
    }
    check(&wb, "F1", Want::Num(1.0));
    check(&wb, "F2", Want::Num(2.0));
    check(&wb, "F3", Want::Empty);
    check(&wb, "I1", Want::Num(2.0));
    check(&wb, "I2", Want::Num(3.0));
    for (a, t) in [("L1", "b"), ("L2", "c"), ("L3", "a")] {
        check(&wb, a, Want::Text(t));
    }
}

/// #139: clearing the cell that blocks a spill must let the array spill on recalculation.
#[test]
fn bug_139_clearing_a_blocker_unblocks_a_spill() {
    let mut wb = workbook(&[("A1", "1"), ("A2", "2"), ("B1", "=A1:A2")]);
    wb.sheet_mut(0).unwrap().cells.set(at("B2"), Cell::value(Value::Number(9.0)));
    let mut calc = Calc::new();
    calc.recalc_all(&mut wb);
    assert_eq!(wb.sheet(0).unwrap().value(at("B1")), Value::Error(CellError::Spill));
    wb.sheet_mut(0).unwrap().cells.remove(at("B2"));
    calc.cells_changed(&mut wb, &[(0, at("B2"))]);
    check(&wb, "B1", Want::Num(1.0));
    check(&wb, "B2", Want::Num(2.0));
}

/// #133: a full recalculation re-lays spills from the formulas' current positions instead of
/// leaving the values they had spilled earlier behind. (The structural edit itself is the
/// engine's; this reproduces the state it leaves for calc.)
#[test]
fn bug_133_a_full_recalculation_relays_a_moved_spill() {
    let mut wb = workbook(&[("A5", "=SEQUENCE(3)")]);
    let mut calc = Calc::new();
    calc.recalc_all(&mut wb);
    for (a, n) in [("A5", 1.0), ("A6", 2.0), ("A7", 3.0)] {
        check(&wb, a, Want::Num(n));
    }
    // The formula moves one row up; the old spilled values stay where they were.
    let moved = wb.sheet_mut(0).unwrap().cells.get(at("A5")).unwrap();
    wb.sheet_mut(0).unwrap().cells.remove(at("A5"));
    wb.sheet_mut(0).unwrap().cells.set(at("A4"), moved);
    calc.recalc_all(&mut wb);
    for (a, n) in [("A4", 1.0), ("A5", 2.0), ("A6", 3.0)] {
        check(&wb, a, Want::Num(n));
    }
    assert_eq!(wb.sheet(0).unwrap().value(at("A7")), Value::Empty);
}

/// #57: Excel compares at 15 significant digits, so 0.1 + 0.2 equals 0.3 (though the arithmetic
/// keeps the binary tail, as the documented 15-digit storage rule says).
#[test]
fn bug_57_comparisons_use_fifteen_significant_digits() {
    let mut wb = workbook(&[("A1", "0.1"), ("A2", "0.2"), ("A3", "=A1+A2")]);
    let mut calc = Calc::new();
    calc.recalc_all(&mut wb);
    assert_eq!(evaluate(&wb, 0, at("D1"), "=A3=0.3"), Value::Bool(true));
    assert_eq!(evaluate(&wb, 0, at("D1"), "=MATCH(0.3,A1:A3,0)"), Value::Number(3.0));
    assert_eq!(evaluate(&wb, 0, at("D1"), "=COUNTIF(A1:A3,0.3)"), Value::Number(1.0));
    // Arithmetic still keeps the full binary result: Excel does not round the stored value.
    let tail = evaluate(&wb, 0, at("D1"), "=A3-0.3").as_f64().unwrap_or(0.0);
    assert!((tail - 5.551115123125783e-17).abs() < 1e-30, "tail was {tail}");
}

/// The 1904 date system (Microsoft's "Date systems in Excel"): serial 0 is 1-Jan-1904, and the
/// two systems differ by 1,462 days, so 1-Jan-2020 is 42,369 rather than 43,831.
#[test]
fn date_system_1904() {
    let mut wb = workbook(&[]);
    wb.date_system = DateSystem::D1904;
    assert_eq!(evaluate(&wb, 0, at("A1"), "=DATE(2020,1,1)"), Value::Number(42369.0));
    assert_eq!(evaluate(&wb, 0, at("A1"), "=TEXT(DATE(2020,1,1),\"yyyy-mm-dd\")"), Value::from("2020-01-01"));
    // 1-Jan-1904 was a Friday: return_type 1 gives 6.
    assert_eq!(evaluate(&wb, 0, at("A1"), "=WEEKDAY(DATE(1904,1,1))"), Value::Number(6.0));
}

/// #140: a 3-D reference over a sheet span sums every sheet in it. (Deleting an end sheet and
/// shrinking the span is the engine's job; the engine suite pins that side.)
#[test]
fn three_d_reference_spans_sheets() {
    let mut wb = Workbook::new();
    wb.sheets.push(Arc::new(Sheet::new("Sheet2")));
    wb.sheets.push(Arc::new(Sheet::new("Sheet3")));
    for (i, v) in [1.0, 2.0, 3.0].iter().enumerate() {
        wb.sheet_mut(i).unwrap().cells.set(at("A1"), Cell::value(Value::Number(*v)));
    }
    assert_eq!(evaluate(&wb, 0, at("B1"), "=SUM(Sheet1:Sheet3!A1)"), Value::Number(6.0));
    assert_eq!(evaluate(&wb, 0, at("B1"), "=AVERAGE(Sheet1:Sheet3!A1)"), Value::Number(2.0));
    assert_eq!(evaluate(&wb, 0, at("B1"), "=Sheet2!A1"), Value::Number(2.0));
}

/// #134: a sheet-scoped name keeps working when it is written with its sheet qualifier.
#[test]
fn bug_134_qualified_defined_name_resolves() {
    let mut wb = Workbook::new();
    let mut data = Sheet::new("Data");
    data.cells.set(at("A1"), Cell::value(Value::Number(0.05)));
    wb.sheets.push(Arc::new(data));
    wb.names.push(DefinedName { name: "Rate".into(), scope: Some(1), formula: "Data!$A$1".into(), comment: String::new(), hidden: false });
    assert_eq!(evaluate(&wb, 0, at("B1"), "=Data!Rate"), Value::Number(0.05));
    assert_eq!(evaluate(&wb, 0, at("B1"), "=Data!Rate*2"), Value::Number(0.1));
}

// ---------------------------------------------------------------------------------------------
// Lookup and reference, and the dynamic arrays they feed
// ---------------------------------------------------------------------------------------------

const VLOOKUP_TABLE: &[(&str, &str)] = &[
    ("A2", "0.457"),
    ("B2", "2.17"),
    ("C2", "100"),
    ("A3", "0.525"),
    ("B3", "2.28"),
    ("C3", "110"),
    ("A4", "0.606"),
    ("B4", "2.43"),
    ("C4", "120"),
    ("A5", "0.684"),
    ("B5", "2.67"),
    ("C5", "130"),
    ("A6", "0.746"),
    ("B6", "3.02"),
    ("C6", "140"),
    ("A7", "0.824"),
    ("B7", "3.45"),
    ("C7", "150"),
    ("A8", "0.855"),
    ("B8", "3.83"),
    ("C8", "160"),
    ("A9", "0.921"),
    ("B9", "4.25"),
    ("C9", "170"),
    ("A10", "1.02"),
    ("B10", "4.7"),
    ("C10", "180"),
];

const HLOOKUP_TABLE: &[(&str, &str)] = &[
    ("A1", "Axles"),
    ("B1", "Bearings"),
    ("C1", "Bolts"),
    ("A2", "4"),
    ("B2", "4"),
    ("C2", "9"),
    ("A3", "5"),
    ("B3", "7"),
    ("C3", "10"),
    ("A4", "6"),
    ("B4", "8"),
    ("C4", "11"),
];

const LOOKUP_TABLE: &[(&str, &str)] = &[
    ("A2", "4.14"),
    ("B2", "2.17"),
    ("A3", "4.19"),
    ("B3", "2.28"),
    ("A4", "5.17"),
    ("B4", "2.43"),
    ("A5", "5.77"),
    ("B5", "3.02"),
    ("A6", "6.39"),
    ("B6", "3.45"),
];

const MATCH_TABLE: &[(&str, &str)] = &[("B2", "25"), ("B3", "38"), ("B4", "40"), ("B5", "41")];

const LOOKUP_CASES: &[Case] = &[
    // VLOOKUP, Microsoft VLOOKUP examples 1–5 (density/viscosity/temperature table).
    c("vlookup.approx.column2", "=VLOOKUP(1,A2:C10,2)", Want::Num(4.25), VLOOKUP_TABLE, "VLOOKUP, MS examples #1"),
    c("vlookup.approx.column3", "=VLOOKUP(1,A2:C10,3,TRUE)", Want::Num(170.0), VLOOKUP_TABLE, "VLOOKUP, MS examples #2"),
    c("vlookup.exact.miss", "=VLOOKUP(0.7,A2:C10,3,FALSE)", Want::Err(CellError::NA), VLOOKUP_TABLE, "VLOOKUP, MS examples #3"),
    c("vlookup.below.first", "=VLOOKUP(0.1,A2:C10,2,TRUE)", Want::Err(CellError::NA), VLOOKUP_TABLE, "VLOOKUP, MS examples #4"),
    c("vlookup.approx.largest", "=VLOOKUP(2,A2:C10,2,TRUE)", Want::Num(4.7), VLOOKUP_TABLE, "VLOOKUP, MS examples #5"),
    // HLOOKUP, Microsoft HLOOKUP examples 1–5 (axles/bearings/bolts table).
    c("hlookup.approx", "=HLOOKUP(\"Axles\",A1:C4,2,TRUE)", Want::Num(4.0), HLOOKUP_TABLE, "HLOOKUP, MS examples #1"),
    c("hlookup.exact", "=HLOOKUP(\"Bearings\",A1:C4,3,FALSE)", Want::Num(7.0), HLOOKUP_TABLE, "HLOOKUP, MS examples #2"),
    c("hlookup.partial", "=HLOOKUP(\"B\",A1:C4,3,TRUE)", Want::Num(5.0), HLOOKUP_TABLE, "HLOOKUP, MS examples #3"),
    c("hlookup.row4", "=HLOOKUP(\"Bolts\",A1:C4,4)", Want::Num(11.0), HLOOKUP_TABLE, "HLOOKUP, MS example (row_num 4)"),
    c(
        "hlookup.array",
        "=HLOOKUP(3,{1,2,3;\"a\",\"b\",\"c\";\"d\",\"e\",\"f\"},2,TRUE)",
        Want::Text("c"),
        &[],
        "HLOOKUP, MS examples #4 (array form)",
    ),
    // MATCH, Microsoft MATCH examples 1–3.
    c("match.approx", "=MATCH(39,B2:B5,1)", Want::Num(2.0), MATCH_TABLE, "MATCH, MS examples #1"),
    c("match.exact", "=MATCH(41,B2:B5,0)", Want::Num(4.0), MATCH_TABLE, "MATCH, MS examples #2"),
    c("match.descending.wrong.order", "=MATCH(40,B2:B5,-1)", Want::Err(CellError::NA), MATCH_TABLE, "MATCH, MS examples #3"),
    c("match.wildcard", "=MATCH(\"b*\",{\"apple\",\"banana\",\"cherry\"},0)", Want::Num(2.0), &[], "MATCH, MS wildcard example (\"b*\")"),
    // INDEX: the page's printed examples are table-based; the rule (row_num, column_num) is exact.
    c("index.rowcol", "=INDEX({1,2;3,4},2,2)", Want::Num(4.0), &[], "INDEX, MS reference form (row_num 2, column_num 2; mini table)"),
    c(
        "index.whole.row",
        "=INDEX({1,2;3,4},2,0)",
        Want::Arr { rows: 1, cols: 2, data: &[Want::Num(3.0), Want::Num(4.0)] },
        &[],
        "INDEX, MS reference form (column_num 0; mini table)",
    ),
    c(
        "index.whole.column",
        "=INDEX({1,2;3,4},0,1)",
        Want::Arr { rows: 2, cols: 1, data: &[Want::Num(1.0), Want::Num(3.0)] },
        &[],
        "INDEX, MS reference form (row_num 0; mini table)",
    ),
    // CHOOSE, Microsoft CHOOSE examples 1–2 (literal arguments).
    c("choose.second", "=CHOOSE(2,\"1st\",\"2nd\",\"3rd\",\"Finished\")", Want::Text("2nd"), &[], "CHOOSE, MS examples #1"),
    c("choose.mixed", "=CHOOSE(3,\"Wide\",115,\"world\")", Want::Text("world"), &[], "CHOOSE, MS examples #2"),
    c("choose.last", "=CHOOSE(4,\"1st\",\"2nd\",\"3rd\",\"Finished\")", Want::Text("Finished"), &[], "CHOOSE, MS example #1 extended (index 4)"),
    // LOOKUP, Microsoft LOOKUP examples 1–3.
    c("lookup.exact", "=LOOKUP(4.14,A2:A6,B2:B6)", Want::Num(2.17), LOOKUP_TABLE, "LOOKUP, MS examples #1"),
    c("lookup.approx", "=LOOKUP(5.75,A2:A6,B2:B6)", Want::Num(2.43), LOOKUP_TABLE, "LOOKUP, MS examples #2"),
    c("lookup.above.max", "=LOOKUP(7.66,A2:A6,B2:B6)", Want::Num(3.45), LOOKUP_TABLE, "LOOKUP, MS examples #3 (above the largest lookup value)"),
    // XLOOKUP / XMATCH: pages print table-based examples; these use mini fixtures of the same
    // shape and the documented match/search modes.
    c("xlookup.exact", "=XLOOKUP(\"b\",{\"a\";\"b\";\"c\"},{10;20;30})", Want::Num(20.0), &[], "XLOOKUP, MS exact-match example (mini table)"),
    c("xlookup.notfound", "=XLOOKUP(\"z\",{\"a\";\"b\"},{10;20},\"none\")", Want::Text("none"), &[], "XLOOKUP, MS if_not_found example (mini table)"),
    c(
        "xlookup.next.larger",
        "=XLOOKUP(25,{10;20;30},{100;200;300},,1)",
        Want::Num(300.0),
        &[],
        "XLOOKUP, MS match_mode 1 example (next larger; mini table)",
    ),
    c(
        "xlookup.last.to.first",
        "=XLOOKUP(2,{1;2;3},{10;20;30},,-1,-1)",
        Want::Num(20.0),
        &[],
        "XLOOKUP, MS search_mode -1 example (last to first; mini table)",
    ),
    c("xmatch.exact", "=XMATCH(\"b\",{\"a\";\"b\";\"c\"})", Want::Num(2.0), &[], "XMATCH, MS exact-match example (mini table)"),
    c("xmatch.next.larger", "=XMATCH(25,{10;20;30},1)", Want::Num(3.0), &[], "XMATCH, MS match_mode 1 example (mini table)"),
    // SORT / SORTBY / FILTER / UNIQUE / SEQUENCE, MS examples with mini tables.
    c(
        "sort.ascending",
        "=SORT({3;1;2})",
        Want::Arr { rows: 3, cols: 1, data: &[Want::Num(1.0), Want::Num(2.0), Want::Num(3.0)] },
        &[],
        "SORT, MS example #1 (ascending; mini table)",
    ),
    c(
        "sort.by.second.desc",
        "=SORT({3,1;2,4},2,-1)",
        Want::Arr { rows: 2, cols: 2, data: &[Want::Num(2.0), Want::Num(4.0), Want::Num(3.0), Want::Num(1.0)] },
        &[],
        "SORT, MS example #2 (sort_index 2, descending; mini table)",
    ),
    c(
        "sortby.ascending",
        "=SORTBY({1;2;3},{30;10;20})",
        Want::Arr { rows: 3, cols: 1, data: &[Want::Num(2.0), Want::Num(3.0), Want::Num(1.0)] },
        &[],
        "SORTBY, MS example #1 (mini table)",
    ),
    c(
        "sortby.descending",
        "=SORTBY({\"a\";\"b\";\"c\"},{1;2;3},-1)",
        Want::Arr { rows: 3, cols: 1, data: &[Want::Text("c"), Want::Text("b"), Want::Text("a")] },
        &[],
        "SORTBY, MS example #2 (sort_order -1; mini table)",
    ),
    c(
        "filter.include",
        "=FILTER({1;2;3;4},{TRUE;FALSE;TRUE;FALSE})",
        Want::Arr { rows: 2, cols: 1, data: &[Want::Num(1.0), Want::Num(3.0)] },
        &[],
        "FILTER, MS example #1 (include filter; mini table)",
    ),
    c("filter.empty", "=FILTER({1;2;3},{FALSE;FALSE;FALSE},\"none\")", Want::Text("none"), &[], "FILTER, MS if_empty example (mini table)"),
    c(
        "unique.column",
        "=UNIQUE({\"a\";\"a\";\"b\"})",
        Want::Arr { rows: 2, cols: 1, data: &[Want::Text("a"), Want::Text("b")] },
        &[],
        "UNIQUE, MS example #1 (unique rows; mini table)",
    ),
    c(
        "unique.rows",
        "=UNIQUE({1,2;1,2;3,4})",
        Want::Arr { rows: 2, cols: 2, data: &[Want::Num(1.0), Want::Num(2.0), Want::Num(3.0), Want::Num(4.0)] },
        &[],
        "UNIQUE, MS example #2 (unique rows; mini table)",
    ),
    c(
        "sequence.grid",
        "=SEQUENCE(4,5)",
        Want::Arr {
            rows: 4,
            cols: 5,
            data: &[
                Want::Num(1.0),
                Want::Num(2.0),
                Want::Num(3.0),
                Want::Num(4.0),
                Want::Num(5.0),
                Want::Num(6.0),
                Want::Num(7.0),
                Want::Num(8.0),
                Want::Num(9.0),
                Want::Num(10.0),
                Want::Num(11.0),
                Want::Num(12.0),
                Want::Num(13.0),
                Want::Num(14.0),
                Want::Num(15.0),
                Want::Num(16.0),
                Want::Num(17.0),
                Want::Num(18.0),
                Want::Num(19.0),
                Want::Num(20.0),
            ],
        },
        &[],
        "SEQUENCE, MS example #1 (4 rows, 5 columns)",
    ),
    // Reference-shaped helpers: the examples are literal formulas on the page.
    c("address.a1", "=ADDRESS(2,3)", Want::Text("$C$2"), &[], "ADDRESS, MS examples #1"),
    c("address.relative.row", "=ADDRESS(2,3,2)", Want::Text("C$2"), &[], "ADDRESS, MS examples #2"),
    c("address.r1c1", "=ADDRESS(2,3,2,FALSE)", Want::Text("R2C[3]"), &[], "ADDRESS, MS examples #3"),
    c("address.sheet", "=ADDRESS(2,3,1,FALSE,\"EXCEL SHEET\")", Want::Text("'EXCEL SHEET'!R2C3"), &[], "ADDRESS, MS examples #5"),
    c_at("row.empty", "=ROW()", Want::Num(5.0), "Z5", "ROW, MS example #1 (row of the formula)"),
    c("row.reference", "=ROW(C10)", Want::Num(10.0), &[], "ROW, MS example #2"),
    c_at("column.empty", "=COLUMN()", Want::Num(26.0), "Z5", "COLUMN, MS example #1 (column of the formula)"),
    c("column.reference", "=COLUMN(C10)", Want::Num(3.0), &[], "COLUMN, MS example #2"),
    c("rows.range", "=ROWS(A1:C4)", Want::Num(4.0), &[], "ROWS, MS example #1"),
    c("rows.array", "=ROWS({1,2;3,4})", Want::Num(2.0), &[], "ROWS, MS example #2 (array)"),
    c("columns.range", "=COLUMNS(A1:C4)", Want::Num(3.0), &[], "COLUMNS, MS example #1"),
    c("columns.array", "=COLUMNS({1,2,3})", Want::Num(3.0), &[], "COLUMNS, MS example #2 (array)"),
    c("offset.value", "=OFFSET(A1,1,1)", Want::Text("x"), &[("A1", "2"), ("B2", "x")], "OFFSET, MS example #1"),
    c("offset.sum", "=SUM(OFFSET(A1,0,0,3,1))", Want::Num(6.0), &[("A1", "1"), ("A2", "2"), ("A3", "3")], "OFFSET, MS example #2 (height/width)"),
    c("indirect.a1", "=INDIRECT(\"A1\")", Want::Num(2.0), &[("A1", "2")], "INDIRECT, MS examples #1"),
    c(
        "indirect.range",
        "=SUM(INDIRECT(\"A1:A3\"))",
        Want::Num(6.0),
        &[("A1", "1"), ("A2", "2"), ("A3", "3")],
        "INDIRECT, MS example #2 (range text)",
    ),
    c("indirect.r1c1", "=INDIRECT(\"R1C1\",FALSE)", Want::Num(2.0), &[("A1", "2")], "INDIRECT, MS examples #3 (a1 = FALSE)"),
    c(
        "transpose.array",
        "=TRANSPOSE({1,2;3,4})",
        Want::Arr { rows: 2, cols: 2, data: &[Want::Num(1.0), Want::Num(3.0), Want::Num(2.0), Want::Num(4.0)] },
        &[],
        "TRANSPOSE, MS example #1 (mini table)",
    ),
    c(
        "take.rows",
        "=TAKE({1;2;3;4;5},2)",
        Want::Arr { rows: 2, cols: 1, data: &[Want::Num(1.0), Want::Num(2.0)] },
        &[],
        "TAKE, MS example #1 (first 2 rows)",
    ),
    c(
        "drop.rows",
        "=DROP({1;2;3;4;5},2)",
        Want::Arr { rows: 3, cols: 1, data: &[Want::Num(3.0), Want::Num(4.0), Want::Num(5.0)] },
        &[],
        "DROP, MS example #1 (drop first 2 rows)",
    ),
    c(
        "hstack.arrays",
        "=HSTACK({1;2},{3;4})",
        Want::Arr { rows: 2, cols: 2, data: &[Want::Num(1.0), Want::Num(3.0), Want::Num(2.0), Want::Num(4.0)] },
        &[],
        "HSTACK, MS example #1 (mini table)",
    ),
    c(
        "vstack.arrays",
        "=VSTACK({1,2},{3,4})",
        Want::Arr { rows: 2, cols: 2, data: &[Want::Num(1.0), Want::Num(2.0), Want::Num(3.0), Want::Num(4.0)] },
        &[],
        "VSTACK, MS example #1 (mini table)",
    ),
];

// ---------------------------------------------------------------------------------------------
// Financial
// ---------------------------------------------------------------------------------------------

const XIRR_TABLE: &[(&str, &str)] = &[
    ("A2", "-10000"),
    ("B2", "1/1/2008"),
    ("A3", "2750"),
    ("B3", "3/1/2008"),
    ("A4", "4250"),
    ("B4", "10/30/2008"),
    ("A5", "3250"),
    ("B5", "2/15/2009"),
    ("A6", "2750"),
    ("B6", "4/1/2009"),
];

const FINANCIAL_CASES: &[Case] = &[
    // PMT, PV, FV, RATE, NPER: Microsoft's worked examples, printed to cents (or 0.0077).
    r("pmt.monthly.loan", "=PMT(0.08/12,10,10000)", Want::Num(-1037.03), 5e-3, &[], "PMT, MS examples #1"),
    r("pmt.savings.goal", "=PMT(0.06/12,18*12,0,50000)", Want::Num(-129.08), 5e-3, &[], "PMT, MS examples #3"),
    r("pv.annuity", "=PV(0.08/12,12*20,500)", Want::Num(-59777.15), 5e-3, &[], "PV, MS examples #1"),
    r("fv.annuity", "=FV(0.06/12,10,-200,-500,1)", Want::Num(2581.40), 5e-3, &[], "FV, MS examples #1"),
    r("fv.monthly.deposit", "=FV(0.12/12,12,-1000)", Want::Num(12682.50), 5e-3, &[], "FV, MS examples #2"),
    r("rate.monthly", "=RATE(4*12,-200,8000)", Want::Num(0.0077), 5e-5, &[], "RATE, MS examples #1"),
    r("rate.annual", "=RATE(4*12,-200,8000)*12", Want::Num(0.0924), 5e-5, &[], "RATE, MS examples #2"),
    r("nper.with.fv", "=NPER(0.12/12,-100,-1000,10000,1)", Want::Num(59.6738657), 5e-8, &[], "NPER, MS examples #1"),
    r("nper.without.type", "=NPER(0.12/12,-100,-1000,10000)", Want::Num(60.0821229), 5e-8, &[], "NPER, MS examples #2"),
    r("nper.negative", "=NPER(0.12/12,-100,-1000)", Want::Num(-9.57859404), 5e-8, &[], "NPER, MS examples #3"),
    r("ipmt.first.month", "=IPMT(0.1/12,1,36,8000)", Want::Num(-66.67), 5e-3, &[], "IPMT, MS examples #1"),
    r("ipmt.third.year", "=IPMT(0.1,3,3,8000)", Want::Num(-292.45), 5e-3, &[], "IPMT, MS examples #2"),
    r("ppmt.first.month", "=PPMT(0.1/12,1,24,2000)", Want::Num(-75.62), 5e-3, &[], "PPMT, MS examples #1"),
    r("ppmt.tenth.year", "=PPMT(0.08,10,10,200000)", Want::Num(-27598.05), 5e-3, &[], "PPMT, MS examples #2"),
    r("cumipmt.months.13.24", "=CUMIPMT(0.09/12,30*12,125000,13,24,0)", Want::Num(-11135.23), 5e-3, &[], "CUMIPMT, MS examples #1"),
    r("cumipmt.first.month", "=CUMIPMT(0.09/12,30*12,125000,1,1,0)", Want::Num(-937.50), 5e-3, &[], "CUMIPMT, MS examples #2"),
    r("cumprinc.months.13.24", "=CUMPRINC(0.09/12,30*12,125000,13,24,0)", Want::Num(-934.11), 5e-3, &[], "CUMPRINC, MS examples #1"),
    r("cumprinc.first.month", "=CUMPRINC(0.09/12,30*12,125000,1,1,0)", Want::Num(-68.28), 5e-3, &[], "CUMPRINC, MS examples #2"),
    // NPV / IRR / XIRR / XNPV / MIRR.
    r("npv.flows", "=NPV(0.1,-10000,3000,4200,6800)", Want::Num(1188.44), 5e-3, &[], "NPV, MS examples #1"),
    r("npv.range", "=NPV(0.08,8000,9200,10000,12000,14500)", Want::Num(41922.06), 5e-3, &[], "NPV, MS examples #2"),
    r("irr.five.flows", "=IRR({-70000,12000,15000,18000,21000})", Want::Num(-0.0212), 5e-4, &[], "IRR, MS examples #1"),
    r("irr.six.flows", "=IRR({-70000,12000,15000,18000,21000,26000})", Want::Num(0.0866), 5e-4, &[], "IRR, MS examples #2"),
    r("irr.guess", "=IRR({-70000,12000,15000},-0.1)", Want::Num(-0.4435), 5e-4, &[], "IRR, MS examples #3"),
    r("xirr.dated.flows", "=XIRR(A2:A6,B2:B6,0.1)", Want::Num(0.373362535), 5e-9, XIRR_TABLE, "XIRR, MS examples #1"),
    r("xnpv.dated.flows", "=XNPV(0.09,A2:A6,B2:B6)", Want::Num(2086.65), 5e-3, XIRR_TABLE, "XNPV, MS examples #1"),
    r("mirr.flows", "=MIRR({-120000,39000,30000,21000,37000,46000},0.1,0.12)", Want::Num(0.1261), 5e-5, &[], "MIRR, MS examples #1"),
    // Depreciation: DB, DDB, SLN, SYD (Microsoft's printed schedules).
    r("db.year1", "=DB(1000000,100000,6,1,7)", Want::Num(186083.33), 5e-3, &[], "DB, MS examples #1"),
    r("db.year2", "=DB(1000000,100000,6,2,7)", Want::Num(259639.42), 5e-3, &[], "DB, MS examples #2"),
    r("db.year3", "=DB(1000000,100000,6,3,7)", Want::Num(176814.44), 5e-3, &[], "DB, MS examples #3"),
    r("db.year4", "=DB(1000000,100000,6,4,7)", Want::Num(120410.64), 5e-3, &[], "DB, MS examples #4"),
    r("db.year7", "=DB(1000000,100000,6,7,7)", Want::Num(15845.10), 5e-3, &[], "DB, MS examples #7"),
    r("ddb.year1", "=DDB(2400,300,10,1)", Want::Num(480.00), 5e-3, &[], "DDB, MS examples #1"),
    r("ddb.year2.factor", "=DDB(2400,300,10,2,1.5)", Want::Num(306.00), 5e-3, &[], "DDB, MS examples #2"),
    r("ddb.year10", "=DDB(2400,300,10,10)", Want::Num(22.12), 5e-3, &[], "DDB, MS examples #3 (last period)"),
    r("sln.annual", "=SLN(30000,7500,10)", Want::Num(2250.00), 5e-3, &[], "SLN, MS examples #1"),
    r("syd.year1", "=SYD(30000,7500,10,1)", Want::Num(4090.91), 5e-3, &[], "SYD, MS examples #1"),
    r("syd.year10", "=SYD(30000,7500,10,10)", Want::Num(409.09), 5e-3, &[], "SYD, MS examples #2"),
    // Coupon dates: settlement 25-Jan-2011, maturity 15-Nov-2011, semiannual, basis 1.
    c("coupdays.semiannual", "=COUPDAYS(DATE(2011,1,25),DATE(2011,11,15),2,1)", Want::Num(181.0), &[], "COUPDAYS, MS examples #1"),
    c("coupnum.semiannual", "=COUPNUM(DATE(2011,1,25),DATE(2011,11,15),2,1)", Want::Num(2.0), &[], "COUPNUM, MS examples #1"),
    c(
        "couppcd.semiannual",
        "=TEXT(COUPPCD(DATE(2011,1,25),DATE(2011,11,15),2,1),\"yyyy-mm-dd\")",
        Want::Text("2010-11-15"),
        &[],
        "COUPPCD, MS examples #1",
    ),
    c(
        "coupncd.semiannual",
        "=TEXT(COUPNCD(DATE(2011,1,25),DATE(2011,11,15),2,1),\"yyyy-mm-dd\")",
        Want::Text("2011-05-15"),
        &[],
        "COUPNCD, MS examples #1",
    ),
    c("coupdaybs.semiannual", "=COUPDAYBS(DATE(2011,1,25),DATE(2011,11,15),2,1)", Want::Num(71.0), &[], "COUPDAYBS, MS examples #1"),
    c("coupdaysnc.semiannual", "=COUPDAYSNC(DATE(2011,1,25),DATE(2011,11,15),2,1)", Want::Num(110.0), &[], "COUPDAYSNC, MS examples #1"),
    // Effective/nominal rates and other financial primitives.
    r("effect.quarterly", "=EFFECT(0.0525,4)", Want::Num(0.053543), 5e-7, &[], "EFFECT, MS examples #1"),
    r("nominal.quarterly", "=NOMINAL(0.053543,4)", Want::Num(0.0525), 5e-5, &[], "NOMINAL, MS examples #1"),
    r("pduration.periods", "=PDURATION(0.025,2000,2200)", Want::Num(3.859866163), 5e-9, &[], "PDURATION, MS examples #1"),
    r("rri.periods", "=RRI(96,10000,11000)", Want::Num(0.0009933), 5e-7, &[], "RRI, MS examples #1"),
    c("dollarde.16", "=DOLLARDE(1.02,16)", Want::Num(1.125), &[], "DOLLARDE, MS examples #1"),
    c("dollarde.32", "=DOLLARDE(1.1,32)", Want::Num(1.3125), &[], "DOLLARDE, MS examples #2"),
    c("dollarfr.16", "=DOLLARFR(1.125,16)", Want::Num(1.02), &[], "DOLLARFR, MS examples #1"),
    c("dollarfr.32", "=DOLLARFR(1.125,32)", Want::Num(1.04), &[], "DOLLARFR, MS examples #2"),
    c("fvschedule.steps", "=FVSCHEDULE(1,{0.09,0.11,0.1})", Want::Num(1.33089), &[], "FVSCHEDULE, MS examples #1"),
    r("ispmt.first.period", "=ISPMT(0.1/12,0,36,8000000)", Want::Num(-66666.67), 5e-3, &[], "ISPMT, MS examples #1"),
    r("price.bond", "=PRICE(DATE(2008,2,15),DATE(2017,11,15),0.0575,0.065,100,2,0)", Want::Num(94.63), 5e-3, &[], "PRICE, MS examples #1"),
    r("yield.bond", "=YIELD(DATE(2008,2,15),DATE(2016,11,15),0.0575,95,100,2,0)", Want::Num(0.0651), 5e-5, &[], "YIELD, MS examples #1"),
    r("tbillprice.days", "=TBILLPRICE(DATE(2008,3,31),DATE(2008,6,1),0.09)", Want::Num(98.45), 5e-3, &[], "TBILLPRICE, MS examples #1"),
    r("tbillyield.days", "=TBILLYIELD(DATE(2008,3,31),DATE(2008,6,1),98.45)", Want::Num(0.0914), 5e-5, &[], "TBILLYIELD, MS examples #1"),
];

// ---------------------------------------------------------------------------------------------
// Date and time
// ---------------------------------------------------------------------------------------------

const DATE_CASES: &[Case] = &[
    // DATE: the page's four worked examples (dates printed in the page's locale; y-m-d via TEXT).
    c("date.leap.year.span", "=DATE(2008,7,8)-DATE(2007,7,8)", Want::Num(366.0), &[], "DATE, MS examples #3 (2008 is a leap year)"),
    c(
        "date.month.overflow",
        "=TEXT(DATE(2008,14,2),\"yyyy-mm-dd\")",
        Want::Text("2009-02-02"),
        &[],
        "DATE, MS examples #4 (month > 12 rolls into the next year)",
    ),
    c(
        "date.day.overflow",
        "=TEXT(DATE(2008,1,35),\"yyyy-mm-dd\")",
        Want::Text("2008-02-04"),
        &[],
        "DATE, MS examples #5 (day > month length rolls over)",
    ),
    c("date.negative.month", "=TEXT(DATE(2008,-3,2),\"yyyy-mm-dd\")", Want::Text("2007-09-02"), &[], "DATE, MS examples #6 (negative month)"),
    // EDATE / EOMONTH, Microsoft's 15-Jan-2011 example.
    c("edate.plus.one", "=TEXT(EDATE(DATE(2011,1,15),1),\"yyyy-mm-dd\")", Want::Text("2011-02-15"), &[], "EDATE, MS examples #1"),
    c("edate.minus.one", "=TEXT(EDATE(DATE(2011,1,15),-1),\"yyyy-mm-dd\")", Want::Text("2010-12-15"), &[], "EDATE, MS examples #2"),
    c("edate.plus.two", "=TEXT(EDATE(DATE(2011,1,15),2),\"yyyy-mm-dd\")", Want::Text("2011-03-15"), &[], "EDATE, MS examples #3"),
    c(
        "edate.month.end",
        "=TEXT(EDATE(DATE(2011,1,31),1),\"yyyy-mm-dd\")",
        Want::Text("2011-02-28"),
        &[],
        "EDATE, MS month-end rule (31-Jan rolls to the month's last day)",
    ),
    c("eomonth.next", "=TEXT(EOMONTH(DATE(2011,1,1),1),\"yyyy-mm-dd\")", Want::Text("2011-02-28"), &[], "EOMONTH, MS examples #1"),
    c("eomonth.minus.three", "=TEXT(EOMONTH(DATE(2011,1,1),-3),\"yyyy-mm-dd\")", Want::Text("2010-10-31"), &[], "EOMONTH, MS examples #2"),
    c("eomonth.month.end", "=TEXT(EOMONTH(DATE(2011,1,31),1),\"yyyy-mm-dd\")", Want::Text("2011-02-28"), &[], "EOMONTH, MS month-end rule"),
    // WORKDAY / NETWORKDAYS, Microsoft's 1-Oct-2008 and 1-Oct-2012 examples.
    c("workday.151", "=TEXT(WORKDAY(DATE(2008,10,1),151),\"yyyy-mm-dd\")", Want::Text("2009-04-30"), &[], "WORKDAY, MS examples #1"),
    c(
        "workday.151.holidays",
        "=TEXT(WORKDAY(DATE(2008,10,1),151,DATE(2008,11,26)),\"yyyy-mm-dd\")",
        Want::Text("2009-05-01"),
        &[],
        "WORKDAY, MS examples #2 (one holiday)",
    ),
    c(
        "workday.151.three.holidays",
        "=TEXT(WORKDAY(DATE(2008,10,1),151,A1:A3),\"yyyy-mm-dd\")",
        Want::Text("2009-05-05"),
        &[("A1", "11/26/2008"), ("A2", "12/4/2008"), ("A3", "1/21/2009")],
        "WORKDAY, MS examples #2 (three holidays)",
    ),
    c("networkdays.all", "=NETWORKDAYS(DATE(2012,10,1),DATE(2013,3,1))", Want::Num(110.0), &[], "NETWORKDAYS, MS examples #1"),
    c(
        "networkdays.one.holiday",
        "=NETWORKDAYS(DATE(2012,10,1),DATE(2013,3,1),DATE(2012,11,22))",
        Want::Num(109.0),
        &[],
        "NETWORKDAYS, MS examples #2",
    ),
    c(
        "networkdays.two.holidays",
        "=NETWORKDAYS(DATE(2012,10,1),DATE(2013,3,1),DATE(2012,11,22))+0*DATE(2012,12,24)+NETWORKDAYS(DATE(2012,10,1),DATE(2013,3,1),DATE(2012,12,24))-NETWORKDAYS(DATE(2012,10,1),DATE(2013,3,1))",
        Want::Num(108.0),
        &[],
        "NETWORKDAYS, MS examples #3 (two holidays)",
    ),
    c(
        "networkdays.intl.holidays",
        "=NETWORKDAYS.INTL(DATE(2006,1,1),DATE(2006,2,1),7,A1:A2)",
        Want::Num(22.0),
        &[("A1", "1/2/2006"), ("A2", "1/16/2006")],
        "NETWORKDAYS.INTL, MS examples #1 (Friday/Saturday weekend, two holidays)",
    ),
    // YEARFRAC, Microsoft's 1-Jan-2012 .. 30-Jul-2012 example for bases 0, 1 and 3.
    r(
        "yearfrac.basis0",
        "=YEARFRAC(DATE(2012,1,1),DATE(2012,7,30))",
        Want::Num(0.58055556),
        5e-9,
        &[],
        "YEARFRAC, MS examples #1 (basis 0, US 30/360)",
    ),
    r(
        "yearfrac.basis1",
        "=YEARFRAC(DATE(2012,1,1),DATE(2012,7,30),1)",
        Want::Num(0.57650273),
        5e-9,
        &[],
        "YEARFRAC, MS examples #2 (basis 1, actual/actual)",
    ),
    r(
        "yearfrac.basis3",
        "=YEARFRAC(DATE(2012,1,1),DATE(2012,7,30),3)",
        Want::Num(0.57808219),
        5e-9,
        &[],
        "YEARFRAC, MS examples #3 (basis 3, actual/365)",
    ),
    // DATEDIF, Microsoft's 1-Jan-2001 .. 1-Jan-2003 example.
    c("datedif.years", "=DATEDIF(DATE(2001,1,1),DATE(2003,1,1),\"Y\")", Want::Num(2.0), &[], "DATEDIF, MS examples #1 (\"Y\")"),
    c("datedif.months", "=DATEDIF(DATE(2001,1,1),DATE(2003,1,1),\"M\")", Want::Num(24.0), &[], "DATEDIF, MS examples #2 (\"M\")"),
    c("datedif.days", "=DATEDIF(DATE(2001,1,1),DATE(2003,1,1),\"D\")", Want::Num(730.0), &[], "DATEDIF, MS examples #3 (\"D\")"),
    c("datedif.yd", "=DATEDIF(DATE(2001,6,1),DATE(2002,8,15),\"YD\")", Want::Num(75.0), &[], "DATEDIF, MS examples #4 (\"YD\")"),
    c("datedif.md", "=DATEDIF(DATE(2001,6,1),DATE(2002,8,15),\"MD\")", Want::Num(14.0), &[], "DATEDIF, MS examples #5 (\"MD\")"),
    c("datedif.ym", "=DATEDIF(DATE(2001,6,1),DATE(2002,8,15),\"YM\")", Want::Num(2.0), &[], "DATEDIF, MS examples #6 (\"YM\")"),
    // WEEKDAY / WEEKNUM / ISOWEEKNUM, Microsoft's 14-Feb-2008 and 9-Mar-2012 examples.
    c("weekday.default", "=WEEKDAY(DATE(2008,2,14))", Want::Num(5.0), &[], "WEEKDAY, MS examples #1 (return_type 1)"),
    c("weekday.monday1", "=WEEKDAY(DATE(2008,2,14),2)", Want::Num(4.0), &[], "WEEKDAY, MS examples #2 (return_type 2)"),
    c("weekday.monday0", "=WEEKDAY(DATE(2008,2,14),3)", Want::Num(3.0), &[], "WEEKDAY, MS examples #3 (return_type 3)"),
    c("weeknum.default", "=WEEKNUM(DATE(2012,3,9))", Want::Num(10.0), &[], "WEEKNUM, MS examples #1"),
    c("weeknum.monday", "=WEEKNUM(DATE(2012,3,9),2)", Want::Num(11.0), &[], "WEEKNUM, MS examples #2 (return_type 2)"),
    c("isoweeknum.default", "=ISOWEEKNUM(DATE(2012,3,9))", Want::Num(10.0), &[], "ISOWEEKNUM, MS examples #1"),
    c(
        "weeknum.monday.year.end",
        "=WEEKNUM(DATE(2012,12,31),2)",
        Want::Num(54.0),
        &[],
        "WEEKNUM, MS return_type 2 rule (year with a partial first week)",
    ),
    c("isoweeknum.year.end", "=ISOWEEKNUM(DATE(2012,12,31))", Want::Num(1.0), &[], "ISOWEEKNUM, MS ISO week rule (31-Dec-2012 is week 1 of 2013)"),
    // Day/month/year, DATEVALUE, DAYS.
    c("day.number", "=DAY(DATE(2011,4,15))", Want::Num(15.0), &[], "DAY, MS examples #1"),
    c("month.number", "=MONTH(DATE(2011,4,15))", Want::Num(4.0), &[], "MONTH, MS examples #1"),
    c("year.number", "=YEAR(DATE(2011,4,15))", Want::Num(2011.0), &[], "YEAR, MS examples #1"),
    c("datevalue.roundtrip", "=TEXT(DATEVALUE(\"8/22/2008\"),\"yyyy-mm-dd\")", Want::Text("2008-08-22"), &[], "DATEVALUE, MS examples #1"),
    c("days.difference", "=DAYS(DATE(2011,3,15),DATE(2011,2,1))", Want::Num(42.0), &[], "DAYS, MS examples #1"),
    // TIME and its parts.
    c("time.noon", "=TIME(12,0,0)", Want::Num(0.5), &[], "TIME, MS examples #1"),
    c("time.afternoon", "=TIME(16,48,10)", Want::Num(0.70011574), &[], "TIME, MS examples #2"),
    c("hour.of.time", "=HOUR(TIME(15,30,0))", Want::Num(15.0), &[], "HOUR, MS examples #1"),
    c("minute.of.time", "=MINUTE(TIME(15,30,0))", Want::Num(30.0), &[], "MINUTE, MS examples #1"),
    c("second.of.time", "=SECOND(TIME(15,30,45))", Want::Num(45.0), &[], "SECOND, MS examples #1"),
    c("timevalue.2.24am", "=TIMEVALUE(\"2:24 AM\")", Want::Num(0.1), &[], "TIMEVALUE, MS examples #1"),
    c("timevalue.datetime", "=TIMEVALUE(\"22-Aug-2011 6:35 AM\")", Want::Num(0.274305556), &[], "TIMEVALUE, MS examples #2"),
    // DAYS360, Microsoft's 30-Jan-2011 .. 1-Feb-2011 example.
    c("days360.us", "=DAYS360(DATE(2011,1,30),DATE(2011,2,1))", Want::Num(1.0), &[], "DAYS360, MS examples #1 (US method)"),
    c("days360.european", "=DAYS360(DATE(2011,1,30),DATE(2011,2,1),TRUE)", Want::Num(1.0), &[], "DAYS360, MS examples #2 (European method)"),
    // The 1900 leap-year bug, documented by Microsoft ("Excel incorrectly assumes that the year
    // 1900 is a leap year"): serial 60 is 29-Feb-1900 and WEEKDAY(60) is 4 (Wednesday).
    c("date.1900.first.day", "=DATE(1900,1,1)", Want::Num(1.0), &[], "DATE, 1900 date system (1-Jan-1900 is serial 1)"),
    c("weekday.1900.first.day", "=WEEKDAY(DATE(1900,1,1))", Want::Num(1.0), &[], "WEEKDAY, 1900 date system (1-Jan-1900 is a Sunday)"),
    c("date.1900.after.fake.day", "=DATE(1900,3,1)", Want::Num(61.0), &[], "DATE, 1900 date system leap-year bug (1-Mar-1900 is serial 61)"),
    c("date.1900.fake.leap.day", "=DATE(1900,2,29)", Want::Num(60.0), &[], "DATE, 1900 date system leap-year bug (serial 60)"),
    c(
        "date.1900.fake.leap.text",
        "=TEXT(60,\"yyyy-mm-dd\")",
        Want::Text("1900-02-29"),
        &[],
        "TEXT, 1900 date system leap-year bug (serial 60 formats as 29-Feb-1900)",
    ),
    c("weekday.1900.fake.leap.day", "=WEEKDAY(DATE(1900,2,29))", Want::Num(4.0), &[], "WEEKDAY, 1900 date system leap-year bug (Wednesday)"),
];

// ---------------------------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------------------------

const TEXTJOIN_TABLE: &[(&str, &str)] = &[("A1", "Amanda"), ("A2", ""), ("A3", "Charlotte")];

const TEXT_CASES: &[Case] = &[
    // TEXT with documented format codes (the format-code reference and the TEXT page's examples).
    c("text.currency", "=TEXT(1234.567,\"$#,##0.00\")", Want::Text("$1,234.57"), &[], "TEXT, MS examples #1 (currency)"),
    c("text.percent.one.decimal", "=TEXT(0.285,\"0.0%\")", Want::Text("28.5%"), &[], "TEXT, MS examples #2 (percentage)"),
    c("text.thousands", "=TEXT(1234.567,\"#,##0.00\")", Want::Text("1,234.57"), &[], "TEXT, MS number-format example (#,##0.00)"),
    c("text.two.decimals", "=TEXT(1234.567,\"0.00\")", Want::Text("1234.57"), &[], "TEXT, MS number-format example (0.00)"),
    c("text.zero.padded", "=TEXT(1,\"00000\")", Want::Text("00001"), &[], "TEXT, MS number-format example (00000)"),
    c("text.percent.zero", "=TEXT(0.1,\"0%\")", Want::Text("10%"), &[], "TEXT, MS number-format example (0%)"),
    c("text.round.half.away", "=TEXT(2.5,\"0\")", Want::Text("3"), &[], "TEXT, MS number-format example (0, half away from zero)"),
    c("text.round.negative.half", "=TEXT(-2.5,\"0\")", Want::Text("-3"), &[], "TEXT, MS number-format example (0, half away from zero)"),
    c("text.integer.separator", "=TEXT(1234567,\"#,##0\")", Want::Text("1,234,567"), &[], "TEXT, MS number-format example (#,##0)"),
    c("text.time.hmm", "=TEXT(0.5,\"h:mm\")", Want::Text("12:00"), &[], "TEXT, MS time-format example (h:mm)"),
    c("text.time.ampm", "=TEXT(0.75,\"h:mm AM/PM\")", Want::Text("6:00 PM"), &[], "TEXT, MS time-format example (h:mm AM/PM)"),
    c("text.date.iso", "=TEXT(43831,\"yyyy-mm-dd\")", Want::Text("2020-01-01"), &[], "TEXT, MS date-format example (yyyy-mm-dd)"),
    c("text.date.long", "=TEXT(43831,\"mmmm d, yyyy\")", Want::Text("January 1, 2020"), &[], "TEXT, MS date-format example (mmmm d, yyyy)"),
    c("text.fraction", "=TEXT(1.5,\"# ?/?\")", Want::Text("1 1/2"), &[], "TEXT, MS fraction-format example (# ?/?)"),
    c("text.scientific", "=TEXT(1234567,\"0.0E+00\")", Want::Text("1.2E+06"), &[], "TEXT, MS scientific-format example (0.0E+00)"),
    c("text.percent.rounds", "=TEXT(0.285,\"0%\")", Want::Text("29%"), &[], "TEXT, MS number-format rounding (0%)"),
    c("text.rounds.decimal.tail", "=TEXT(2.675,\"0.00\")", Want::Text("2.68"), &[], "TEXT, MS 15-significant-digit display rule (0.00)"),
    // TEXTJOIN, Microsoft's Charlotte/Amanda example table.
    c("textjoin.ignore.empty.true", "=TEXTJOIN(\", \",TRUE,A1:A3)", Want::Text("Amanda, Charlotte"), TEXTJOIN_TABLE, "TEXTJOIN, MS examples #1"),
    c("textjoin.ignore.empty.false", "=TEXTJOIN(\", \",FALSE,A1:A3)", Want::Text("Amanda, , Charlotte"), TEXTJOIN_TABLE, "TEXTJOIN, MS examples #2"),
    c(
        "textjoin.all",
        "=TEXTJOIN(\", \",TRUE,\"Charlotte\",\"Charlotte\",\"Charlotte\")",
        Want::Text("Charlotte, Charlotte, Charlotte"),
        &[],
        "TEXTJOIN, MS examples #3",
    ),
    // CONCAT / CONCATENATE, the page's literal example.
    c(
        "concat.parts",
        "=CONCAT(\"Stream population for\",\" brook trout\",\" species is\",\" 32/Mile\")",
        Want::Text("Stream population for brook trout species is 32/Mile"),
        &[],
        "CONCAT, MS examples #1",
    ),
    c(
        "concatenate.parts",
        "=CONCATENATE(\"Stream population for\",\" brook trout\",\" species is\",\" 32/Mile\")",
        Want::Text("Stream population for brook trout species is 32/Mile"),
        &[],
        "CONCATENATE, MS examples #1",
    ),
    // SUBSTITUTE, REPLACE, MID: literal examples on the pages.
    c("substitute.word", "=SUBSTITUTE(\"Sales Data\",\"Sales\",\"Cost\")", Want::Text("Cost Data"), &[], "SUBSTITUTE, MS examples #1"),
    c("substitute.instance1", "=SUBSTITUTE(\"Quarter 1, 2011\",\"1\",\"2\",1)", Want::Text("Quarter 2, 2011"), &[], "SUBSTITUTE, MS examples #2"),
    c("substitute.instance3", "=SUBSTITUTE(\"Quarter 1, 2011\",\"1\",\"2\",3)", Want::Text("Quarter 1, 2012"), &[], "SUBSTITUTE, MS examples #3"),
    c("replace.middle", "=REPLACE(\"abcdefghijk\",6,5,\"*\")", Want::Text("abcde*k"), &[], "REPLACE, MS examples #1"),
    c("replace.year", "=REPLACE(\"2009\",3,2,\"10\")", Want::Text("2010"), &[], "REPLACE, MS examples #2"),
    c("replace.front", "=REPLACE(\"123456\",1,3,\"@\")", Want::Text("@456"), &[], "REPLACE, MS examples #3"),
    c("mid.fluid", "=MID(\"Fluid Flow\",1,5)", Want::Text("Fluid"), &[], "MID, MS examples #1"),
    c("mid.flow", "=MID(\"Fluid Flow\",7,20)", Want::Text("Flow"), &[], "MID, MS examples #2"),
    c("mid.past.end", "=MID(\"Fluid Flow\",20,5)", Want::Text(""), &[], "MID, MS examples #3"),
    // FIND / SEARCH, including SEARCH's case-insensitivity.
    c("find.case.sensitive", "=FIND(\"M\",\"Miriam McGovern\")", Want::Num(1.0), &[], "FIND, MS examples #1"),
    c("find.lowercase", "=FIND(\"m\",\"Miriam McGovern\")", Want::Num(6.0), &[], "FIND, MS examples #2"),
    c("find.start", "=FIND(\"M\",\"Miriam McGovern\",3)", Want::Num(8.0), &[], "FIND, MS examples #3"),
    c("search.case.insensitive", "=SEARCH(\"e\",\"Statements\",6)", Want::Num(7.0), &[], "SEARCH, MS examples #1"),
    c("search.word", "=SEARCH(\"margin\",\"Profit Margin\")", Want::Num(8.0), &[], "SEARCH, MS examples #2"),
    c("search.not.found", "=SEARCH(\"b\",\"Milwaukee\")", Want::Err(CellError::Value), &[], "SEARCH, MS examples #3 (#VALUE! when not found)"),
    // TRIM / CLEAN / PROPER / EXACT: literal examples.
    c("trim.spaces", "=TRIM(\" First Quarter Earnings \")", Want::Text("First Quarter Earnings"), &[], "TRIM, MS examples #1"),
    c("clean.control.chars", "=CLEAN(CHAR(9)&\"Monthly report\"&CHAR(10))", Want::Text("Monthly report"), &[], "CLEAN, MS examples #1"),
    c("proper.title", "=PROPER(\"this is a TITLE\")", Want::Text("This Is A Title"), &[], "PROPER, MS examples #1"),
    c("proper.apostrophe", "=PROPER(\"2-cent's worth\")", Want::Text("2-Cent'S Worth"), &[], "PROPER, MS examples #2"),
    c("proper.camel", "=PROPER(\"76BudGet\")", Want::Text("76Budget"), &[], "PROPER, MS examples #3"),
    c("exact.same", "=EXACT(\"word\",\"word\")", Want::Bool(true), &[], "EXACT, MS examples #1"),
    c("exact.case", "=EXACT(\"Word\",\"word\")", Want::Bool(false), &[], "EXACT, MS examples #2"),
    c("exact.space", "=EXACT(\"w ord\",\"word\")", Want::Bool(false), &[], "EXACT, MS examples #3"),
    // LEFT / RIGHT / LEN / case / repetition.
    c("left.word", "=LEFT(\"Sale Price\",4)", Want::Text("Sale"), &[], "LEFT, MS examples #1"),
    c("left.default", "=LEFT(\"Sweden\")", Want::Text("S"), &[], "LEFT, MS examples #2"),
    c("right.word", "=RIGHT(\"Sale Price\",5)", Want::Text("Price"), &[], "RIGHT, MS examples #1"),
    c("right.default", "=RIGHT(\"Stock Number\")", Want::Text("r"), &[], "RIGHT, MS examples #2"),
    c("len.text", "=LEN(\"Phoenix, AZ\")", Want::Num(11.0), &[], "LEN, MS examples #1"),
    c("len.empty", "=LEN(\"\")", Want::Num(0.0), &[], "LEN, MS examples #2"),
    c("upper.word", "=UPPER(\"total\")", Want::Text("TOTAL"), &[], "UPPER, MS examples #1"),
    c("lower.poem", "=LOWER(\"E. E. Cummings\")", Want::Text("e. e. cummings"), &[], "LOWER, MS examples #1"),
    c("rept.stars", "=REPT(\"*-\",3)", Want::Text("*-*-*-"), &[], "REPT, MS examples #1"),
    c("rept.dashes", "=REPT(\"-\",10)", Want::Text("----------"), &[], "REPT, MS examples #2"),
    // VALUE, CHAR, CODE, UNICHAR, UNICODE, T.
    c("value.currency", "=VALUE(\"$1,000\")", Want::Num(1000.0), &[], "VALUE, MS examples #1"),
    c("value.times.difference", "=VALUE(\"16:48:00\")-VALUE(\"12:00:00\")", Want::Num(0.2), &[], "VALUE, MS examples #2"),
    c("char.letter", "=CHAR(65)", Want::Text("A"), &[], "CHAR, MS examples #1"),
    c("char.bang", "=CHAR(33)", Want::Text("!"), &[], "CHAR, MS examples #2"),
    c("code.letter", "=CODE(\"A\")", Want::Num(65.0), &[], "CODE, MS examples #1"),
    c("code.bang", "=CODE(\"!\")", Want::Num(33.0), &[], "CODE, MS examples #2"),
    c("unichar.letter", "=UNICHAR(66)", Want::Text("B"), &[], "UNICHAR, MS examples #1"),
    c("unichar.space", "=UNICHAR(32)", Want::Text(" "), &[], "UNICHAR, MS examples #2"),
    c("unicode.letter", "=UNICODE(\"B\")", Want::Num(66.0), &[], "UNICODE, MS examples #1"),
    c("t.text", "=T(\"Rain\")", Want::Text("Rain"), &[], "T, MS examples #1"),
    c("t.number", "=T(19)", Want::Text(""), &[], "T, MS examples #2 (a number gives text \"\")"),
    // DOLLAR, NUMBERVALUE, TEXTBEFORE, TEXTAFTER: literal examples on the pages.
    c("dollar.two.decimals", "=DOLLAR(1234.567,2)", Want::Text("$1,234.57"), &[], "DOLLAR, MS examples #1"),
    c("dollar.negative.two", "=DOLLAR(-1234.567,-2)", Want::Text("($1,200)"), &[], "DOLLAR, MS examples #2"),
    c("dollar.negative.four", "=DOLLAR(-0.123,4)", Want::Text("($0.1230)"), &[], "DOLLAR, MS examples #3"),
    c("dollar.default", "=DOLLAR(99.888)", Want::Text("$99.89"), &[], "DOLLAR, MS examples #4"),
    c("numbervalue.decimal", "=NUMBERVALUE(\"2.500,27\",\",\",\".\")", Want::Num(2500.27), &[], "NUMBERVALUE, MS examples #1"),
    c("numbervalue.percent", "=NUMBERVALUE(\"3.5%\")", Want::Num(0.035), &[], "NUMBERVALUE, MS examples #2"),
    c("textbefore.word", "=TEXTBEFORE(\"Red riding hood\",\" \")", Want::Text("Red"), &[], "TEXTBEFORE, MS examples #1"),
    c("textbefore.second", "=TEXTBEFORE(\"Red riding hood\",\" \",2)", Want::Text("Red riding"), &[], "TEXTBEFORE, MS examples #2"),
    c("textafter.word", "=TEXTAFTER(\"Red riding hood\",\" \")", Want::Text("riding hood"), &[], "TEXTAFTER, MS examples #1"),
    c("textafter.second", "=TEXTAFTER(\"Red riding hood\",\" \",2)", Want::Text("hood"), &[], "TEXTAFTER, MS examples #2"),
];

// ---------------------------------------------------------------------------------------------
// Statistical
// ---------------------------------------------------------------------------------------------

const STDEV_TABLE: &[(&str, &str)] = &[
    ("A2", "1345"),
    ("A3", "1301"),
    ("A4", "1368"),
    ("A5", "1322"),
    ("A6", "1310"),
    ("A7", "1370"),
    ("A8", "1318"),
    ("A9", "1350"),
    ("A10", "1303"),
    ("A11", "1299"),
];

const AVERAGE_TABLE: &[(&str, &str)] = &[("A2", "10"), ("A3", "7"), ("A4", "9"), ("A5", "27"), ("A6", "2")];

const MEDIAN_TABLE: &[(&str, &str)] = &[("A2", "1"), ("A3", "2"), ("A4", "3"), ("A5", "4"), ("A6", "5"), ("A7", "6")];

const QUARTILE_TABLE: &[(&str, &str)] = &[("A2", "1"), ("A3", "2"), ("A4", "4"), ("A5", "7"), ("A6", "8"), ("A7", "9"), ("A8", "10"), ("A9", "12")];

const LARGE_TABLE: &[(&str, &str)] =
    &[("A2", "3"), ("A3", "5"), ("A4", "3"), ("A5", "5"), ("A6", "4"), ("A7", "4"), ("A8", "2"), ("A9", "4"), ("A10", "6"), ("A11", "7")];

const RANK_TABLE: &[(&str, &str)] = &[("A2", "7"), ("A3", "3.5"), ("A4", "3.5"), ("A5", "1"), ("A6", "2")];

const PERCENTRANK_TABLE: &[(&str, &str)] =
    &[("A2", "13"), ("A3", "12"), ("A4", "11"), ("A5", "8"), ("A6", "4"), ("A7", "3"), ("A8", "2"), ("A9", "1"), ("A10", "1"), ("A11", "1")];

const COUNTIF_TABLE: &[(&str, &str)] =
    &[("A2", "apples"), ("B2", "32"), ("A3", "oranges"), ("B3", "54"), ("A4", "peaches"), ("B4", "75"), ("A5", "apples"), ("B5", "86")];

const CRITERIA_TABLE: &[(&str, &str)] =
    &[("A1", "apples"), ("B1", "32"), ("A2", "oranges"), ("B2", "54"), ("A3", "peaches"), ("B3", "75"), ("A4", "apples"), ("B4", "86")];

const SUBTOTAL_TABLE: &[(&str, &str)] = &[("A2", "120"), ("A3", "10"), ("A4", "150"), ("A5", "23")];

const AGGREGATE_ERRORS: &[(&str, &str)] = &[("A1", "1"), ("A2", "#DIV/0!"), ("A3", "3")];

const STAT_CASES: &[Case] = &[
    // AVERAGE and MEDIAN, the pages' small tables.
    c("average.range", "=AVERAGE(A2:A6)", Want::Num(11.0), AVERAGE_TABLE, "AVERAGE, MS examples #1"),
    c("average.range.and.literal", "=AVERAGE(A2:A6,5)", Want::Num(10.0), AVERAGE_TABLE, "AVERAGE, MS examples #2"),
    c("median.odd", "=MEDIAN(A2:A6)", Want::Num(3.0), MEDIAN_TABLE, "MEDIAN, MS examples #1"),
    c("median.even", "=MEDIAN(A2:A7)", Want::Num(3.5), MEDIAN_TABLE, "MEDIAN, MS examples #2"),
    c("median.with.literal", "=MEDIAN(A2:A6,10)", Want::Num(3.5), MEDIAN_TABLE, "MEDIAN, MS examples #3"),
    // STDEV.S / STDEV.P / VAR.S / VAR.P, Microsoft's 1,345..1,299 table.
    r("stdev.s.sample", "=STDEV.S(A2:A11)", Want::Num(27.46391572), 5e-9, STDEV_TABLE, "STDEV.S, MS examples #1"),
    r("stdev.p.population", "=STDEV.P(A2:A11)", Want::Num(26.05455814), 5e-9, STDEV_TABLE, "STDEV.P, MS examples #1"),
    r("var.s.sample", "=VAR.S(A2:A11)", Want::Num(754.2666667), 5e-8, STDEV_TABLE, "VAR.S, MS examples #1"),
    r("var.p.population", "=VAR.P(A2:A11)", Want::Num(678.84), 5e-9, STDEV_TABLE, "VAR.P, MS examples #1"),
    // QUARTILE / PERCENTILE, Microsoft's 1,2,4,7,8,9,10,12 table.
    c("quartile.first", "=QUARTILE(A2:A9,1)", Want::Num(3.5), QUARTILE_TABLE, "QUARTILE, MS examples #1 (quart 1)"),
    c("quartile.third", "=QUARTILE(A2:A9,3)", Want::Num(9.25), QUARTILE_TABLE, "QUARTILE, MS examples #2 (quart 3)"),
    c("quartile.minimum", "=QUARTILE(A2:A9,0)", Want::Num(1.0), QUARTILE_TABLE, "QUARTILE, MS examples (quart 0)"),
    c("quartile.maximum", "=QUARTILE(A2:A9,4)", Want::Num(12.0), QUARTILE_TABLE, "QUARTILE, MS examples (quart 4)"),
    c("quartile.exc.first", "=QUARTILE.EXC(A2:A9,1)", Want::Num(2.5), QUARTILE_TABLE, "QUARTILE.EXC, MS examples #1 (quart 1)"),
    c("quartile.exc.third", "=QUARTILE.EXC(A2:A9,3)", Want::Num(9.75), QUARTILE_TABLE, "QUARTILE.EXC, MS examples #2 (quart 3)"),
    c("percentile.exc.50", "=PERCENTILE.EXC(A2:A9,0.5)", Want::Num(7.5), QUARTILE_TABLE, "PERCENTILE.EXC, MS 0.5 example (mini table)"),
    c("percentile.inc.95", "=PERCENTILE.INC(A2:A9,0.95)", Want::Num(11.3), QUARTILE_TABLE, "PERCENTILE.INC, MS 0.95 example (mini table)"),
    c("percentile.inc.25", "=PERCENTILE.INC(A2:A9,0.25)", Want::Num(3.5), QUARTILE_TABLE, "PERCENTILE.INC, MS 0.25 example (mini table)"),
    // LARGE / SMALL, the page's ten-value table.
    c("large.third", "=LARGE(A2:A11,3)", Want::Num(5.0), LARGE_TABLE, "LARGE, MS examples #1"),
    c("large.sixth", "=LARGE(A2:A11,6)", Want::Num(4.0), LARGE_TABLE, "LARGE, MS examples #2"),
    c("small.second", "=SMALL(A2:A11,2)", Want::Num(3.0), LARGE_TABLE, "SMALL, MS examples #1"),
    c("small.sixth", "=SMALL(A2:A11,6)", Want::Num(4.0), LARGE_TABLE, "SMALL, MS examples #2"),
    // MODE.SNGL / MODE.MULT.
    c("mode.sngl", "=MODE.SNGL({5.6,4,4,3,2,4})", Want::Num(4.0), &[], "MODE.SNGL, MS examples #1"),
    c(
        "mode.mult",
        "=MODE.MULT({1,2,3,4,3,2,1,2,3,5,6,1})",
        Want::Arr { rows: 3, cols: 1, data: &[Want::Num(1.0), Want::Num(2.0), Want::Num(3.0)] },
        &[],
        "MODE.MULT, MS examples #1 (three modes)",
    ),
    // RANK, Microsoft's 7/3.5/3.5/1/2 table.
    c("rank.ascending.third", "=RANK(A3,A2:A6,1)", Want::Num(3.0), RANK_TABLE, "RANK, MS examples #1"),
    c("rank.ascending.last", "=RANK(A2,A2:A6,1)", Want::Num(5.0), RANK_TABLE, "RANK, MS examples #2"),
    c("rank.descending.first", "=RANK(A2,A2:A6)", Want::Num(1.0), RANK_TABLE, "RANK, MS examples #3"),
    c("rank.eq.ascending.third", "=RANK.EQ(A3,A2:A6,1)", Want::Num(3.0), RANK_TABLE, "RANK.EQ, MS examples #1 (ties share the rank)"),
    // PERCENTRANK, the page's 13..1 table (values are truncated to 3 significant digits).
    c("percentrank.2", "=PERCENTRANK(A2:A11,2)", Want::Num(0.333), PERCENTRANK_TABLE, "PERCENTRANK, MS examples #1"),
    c("percentrank.4", "=PERCENTRANK(A2:A11,4)", Want::Num(0.555), PERCENTRANK_TABLE, "PERCENTRANK, MS examples #2"),
    c("percentrank.8", "=PERCENTRANK(A2:A11,8)", Want::Num(0.666), PERCENTRANK_TABLE, "PERCENTRANK, MS examples #3"),
    c("percentrank.5", "=PERCENTRANK(A2:A11,5)", Want::Num(0.583), PERCENTRANK_TABLE, "PERCENTRANK, MS examples #4"),
    // COUNT / COUNTA / COUNTBLANK / COUNTIF / COUNTIFS.
    c("count.numbers", "=COUNT(1,2,\"x\",4)", Want::Num(3.0), &[], "COUNT, MS examples #1 (numbers only)"),
    c("count.blank", "=COUNT(1,\"\",3)", Want::Num(2.0), &[], "COUNT, MS examples #2 (empty argument)"),
    c("counta.mixed", "=COUNTA(1,\"x\",TRUE)", Want::Num(3.0), &[], "COUNTA, MS examples #1"),
    c("countblank.one", "=COUNTBLANK(A1:A3)", Want::Num(1.0), &[("A1", "1"), ("A2", ""), ("A3", "x")], "COUNTBLANK, MS examples #1"),
    c("countif.text", "=COUNTIF(A2:A5,\"apples\")", Want::Num(2.0), COUNTIF_TABLE, "COUNTIF, MS examples #1"),
    c("countif.cell.reference", "=COUNTIF(A2:A5,A4)", Want::Num(1.0), COUNTIF_TABLE, "COUNTIF, MS examples #2"),
    c("countif.greater", "=COUNTIF(B2:B5,\">55\")", Want::Num(2.0), COUNTIF_TABLE, "COUNTIF, MS examples #3"),
    c("countif.not.equal", "=COUNTIF(B2:B5,\"<>\"&B4)", Want::Num(3.0), COUNTIF_TABLE, "COUNTIF, MS examples #4"),
    c("countif.range.difference", "=COUNTIF(B2:B5,\">=32\")-COUNTIF(B2:B5,\">85\")", Want::Num(3.0), COUNTIF_TABLE, "COUNTIF, MS examples #5"),
    c(
        "countifs.two.criteria",
        "=COUNTIFS(A1:A4,\"apples\",B1:B4,\">50\")",
        Want::Num(1.0),
        CRITERIA_TABLE,
        "COUNTIFS, MS documented criteria form (mini table)",
    ),
    // SUMIF / SUMIFS / AVERAGEIF / AVERAGEIFS / MAXIFS / MINIFS, documented criteria forms.
    c("sumif.apples", "=SUMIF(A1:A4,\"apples\",B1:B4)", Want::Num(118.0), CRITERIA_TABLE, "SUMIF, MS documented criteria form (mini table)"),
    c(
        "sumif.greater",
        "=SUMIF(B1:B4,\">50\")",
        Want::Num(215.0),
        CRITERIA_TABLE,
        "SUMIF, MS documented criteria form, sum_range omitted (mini table)",
    ),
    c(
        "sumifs.two.criteria",
        "=SUMIFS(B1:B4,A1:A4,\"apples\",B1:B4,\">50\")",
        Want::Num(86.0),
        CRITERIA_TABLE,
        "SUMIFS, MS documented criteria form (mini table)",
    ),
    c(
        "averageif.apples",
        "=AVERAGEIF(A1:A4,\"apples\",B1:B4)",
        Want::Num(59.0),
        CRITERIA_TABLE,
        "AVERAGEIF, MS documented criteria form (mini table)",
    ),
    c(
        "averageifs.apples.greater",
        "=AVERAGEIFS(B1:B4,A1:A4,\"apples\",B1:B4,\">50\")",
        Want::Num(86.0),
        CRITERIA_TABLE,
        "AVERAGEIFS, MS documented criteria form (mini table)",
    ),
    c("maxifs.apples", "=MAXIFS(B1:B4,A1:A4,\"apples\")", Want::Num(86.0), CRITERIA_TABLE, "MAXIFS, MS documented criteria form (mini table)"),
    c("minifs.oranges", "=MINIFS(B1:B4,A1:A4,\"oranges\")", Want::Num(54.0), CRITERIA_TABLE, "MINIFS, MS documented criteria form (mini table)"),
    // SUBTOTAL / AGGREGATE, with function-number and options rules.
    c("subtotal.sum", "=SUBTOTAL(9,A2:A5)", Want::Num(303.0), SUBTOTAL_TABLE, "SUBTOTAL, MS examples #1 (function 9, SUM)"),
    c("subtotal.average", "=SUBTOTAL(1,A2:A5)", Want::Num(75.75), SUBTOTAL_TABLE, "SUBTOTAL, MS examples #2 (function 1, AVERAGE)"),
    c("subtotal.max", "=SUBTOTAL(4,A2:A5)", Want::Num(150.0), SUBTOTAL_TABLE, "SUBTOTAL, MS function 4 (MAX)"),
    c(
        "aggregate.sum.ignore.errors",
        "=AGGREGATE(9,6,A1:A3)",
        Want::Num(4.0),
        AGGREGATE_ERRORS,
        "AGGREGATE, MS examples #1 (options 6, ignore errors)",
    ),
    c("aggregate.sum.propagate", "=AGGREGATE(9,0,A1:A3)", Want::Err(CellError::Div0), AGGREGATE_ERRORS, "AGGREGATE, MS examples #2 (options 0)"),
    c("aggregate.max.ignore.errors", "=AGGREGATE(4,6,A1:A3)", Want::Num(3.0), AGGREGATE_ERRORS, "AGGREGATE, MS examples #3 (function 4, MAX)"),
    c(
        "aggregate.average.ignore.errors",
        "=AGGREGATE(1,6,A1:A3)",
        Want::Num(2.0),
        AGGREGATE_ERRORS,
        "AGGREGATE, MS function 1 (AVERAGE) with options 6",
    ),
    // FREQUENCY / TRIMMEAN.
    c(
        "frequency.bins",
        "=FREQUENCY({79,85,78,85,50,81,95,88,97},{70,79,89})",
        Want::Arr { rows: 4, cols: 1, data: &[Want::Num(1.0), Want::Num(2.0), Want::Num(4.0), Want::Num(2.0)] },
        &[],
        "FREQUENCY, MS examples #1 (70/79/89 bins)",
    ),
    c("trimmean.20.percent", "=TRIMMEAN({1,2,3,4,5,6,7,8,9,10},0.2)", Want::Num(5.5), &[], "TRIMMEAN, MS 20% trim example (mini table)"),
    // Descriptive statistics with literal arguments.
    c("devsq.values", "=DEVSQ(4,5,8,7,11,4,3)", Want::Num(48.0), &[], "DEVSQ, MS examples #1"),
    c("avedev.values", "=AVEDEV(4,5,8,7,11,4,3)", Want::Num(2.285714286), &[], "AVEDEV, MS examples #1"),
    r("geomean.values", "=GEOMEAN(4,5,8,7,11,4,3)", Want::Num(5.476987), 5e-7, &[], "GEOMEAN, MS examples #1"),
    r("harmmean.values", "=HARMEAN(4,5,8,7,11,4,3)", Want::Num(5.028376), 5e-7, &[], "HARMEAN, MS examples #1"),
    c("kurt.small.set", "=KURT(1,2,3,4,5)", Want::Num(-1.2), &[], "KURT, MS documented sample-excess formula (1..5)"),
    c("skew.symmetric", "=SKEW(1,2,3,4,5)", Want::Num(0.0), &[], "SKEW, MS documented symmetry at the mean (1..5)"),
    c("covariance.p", "=COVARIANCE.P({3,2,4,5,6},{9,7,12,15,17})", Want::Num(5.2), &[], "COVARIANCE.P, MS examples #1"),
    c("covariance.s", "=COVARIANCE.S({3,2,4,5,6},{9,7,12,15,17})", Want::Num(6.5), &[], "COVARIANCE.S, MS examples #1"),
    c("max.values", "=MAX(1,2,3)", Want::Num(3.0), &[], "MAX, MS examples #1"),
    c("min.values", "=MIN(1,2,3)", Want::Num(1.0), &[], "MIN, MS examples #1"),
    // Distributions with literal published examples.
    c("binom.dist.pmf", "=BINOM.DIST(6,10,0.5,FALSE)", Want::Num(0.205078125), &[], "BINOM.DIST, MS examples #1 (pmf)"),
    c("binom.dist.cdf", "=BINOM.DIST(6,10,0.5,TRUE)", Want::Num(0.828125), &[], "BINOM.DIST, MS examples #2 (cumulative)"),
    r("norm.s.dist", "=NORM.S.DIST(1.333333,TRUE)", Want::Num(0.908789), 5e-7, &[], "NORM.S.DIST, MS examples #1"),
    r("norm.s.inv", "=NORM.S.INV(0.908789)", Want::Num(1.3333), 5e-5, &[], "NORM.S.INV, MS examples #1"),
    r("expon.dist.cdf", "=EXPON.DIST(0.2,10,TRUE)", Want::Num(0.86466472), 5e-9, &[], "EXPON.DIST, MS examples #1"),
    r("expon.dist.pdf", "=EXPON.DIST(0.2,10,FALSE)", Want::Num(1.35335283), 5e-9, &[], "EXPON.DIST, MS examples #2"),
    r("poisson.dist.cdf", "=POISSON.DIST(2,5,TRUE)", Want::Num(0.124652), 5e-7, &[], "POISSON.DIST, MS examples #1"),
    r("poisson.dist.pmf", "=POISSON.DIST(2,5,FALSE)", Want::Num(0.084224), 5e-7, &[], "POISSON.DIST, MS examples #2"),
    c("gamma.value", "=GAMMA(2.5)", Want::Num(1.329340388), &[], "GAMMA, MS examples #1"),
];

// ---------------------------------------------------------------------------------------------
// Math and trigonometry
// ---------------------------------------------------------------------------------------------

const MATH_CASES: &[Case] = &[
    // SUM / PRODUCT / SUMSQ / SUMPRODUCT, the pages' literal examples.
    c("sum.literals", "=SUM(3,2)", Want::Num(5.0), &[], "SUM, MS examples #1"),
    c("sum.coercion", "=SUM(\"5\",15,TRUE)", Want::Num(21.0), &[], "SUM, MS examples #2 (text and logical arguments)"),
    c("sum.array", "=SUM({1,2,3})", Want::Num(6.0), &[], "SUM, MS array example (mini array)"),
    c("product.literals", "=PRODUCT(5,15,30)", Want::Num(2250.0), &[], "PRODUCT, MS examples #1"),
    c("product.range", "=PRODUCT({1,2,3,4})", Want::Num(24.0), &[], "PRODUCT, MS range example (mini array)"),
    c("sumsq.values", "=SUMSQ(3,4)", Want::Num(25.0), &[], "SUMSQ, MS examples #1"),
    c("sumproduct.matrices", "=SUMPRODUCT({3,4;8,6;1,9},{2,7;6,7;5,3})", Want::Num(156.0), &[], "SUMPRODUCT, MS examples #1 (two 3x2 arrays)"),
    // Matrix functions: MDETERM's example is literal; MMULT/MINVERSE use mini matrices of the
    // documented shape (the pages' own examples are table-based).
    c("mdeterm.four.by.four", "=MDETERM({1,3,8,5;1,3,6,1;1,1,1,0;7,3,10,2})", Want::Num(88.0), &[], "MDETERM, MS examples #1"),
    c(
        "mmult.two.by.two",
        "=MMULT({1,2;3,4},{5,6;7,8})",
        Want::Arr { rows: 2, cols: 2, data: &[Want::Num(19.0), Want::Num(22.0), Want::Num(43.0), Want::Num(50.0)] },
        &[],
        "MMULT, MS example #1 (mini matrix)",
    ),
    c(
        "minverse.two.by.two",
        "=MINVERSE({4,-1;2,0})",
        Want::Arr { rows: 2, cols: 2, data: &[Want::Num(0.0), Want::Num(0.5), Want::Num(-1.0), Want::Num(2.0)] },
        &[],
        "MINVERSE, MS example #1 (mini matrix)",
    ),
    // MOD and QUOTIENT, literal examples.
    c("mod.positive", "=MOD(3,2)", Want::Num(1.0), &[], "MOD, MS examples #1"),
    c("mod.negative.dividend", "=MOD(-3,2)", Want::Num(1.0), &[], "MOD, MS examples #2"),
    c("mod.negative.divisor", "=MOD(3,-2)", Want::Num(-1.0), &[], "MOD, MS examples #3"),
    c("mod.both.negative", "=MOD(-3,-2)", Want::Num(-1.0), &[], "MOD, MS examples #4"),
    c("quotient.integers", "=QUOTIENT(5,2)", Want::Num(2.0), &[], "QUOTIENT, MS examples #1"),
    c("quotient.decimals", "=QUOTIENT(4.5,3.1)", Want::Num(1.0), &[], "QUOTIENT, MS examples #2"),
    // GCD / LCM, literal examples.
    c("gcd.coprime", "=GCD(5,2)", Want::Num(1.0), &[], "GCD, MS examples #1"),
    c("gcd.common", "=GCD(24,36)", Want::Num(12.0), &[], "GCD, MS examples #2"),
    c("gcd.one", "=GCD(7,1)", Want::Num(1.0), &[], "GCD, MS examples #3"),
    c("gcd.three", "=GCD(3,5,7)", Want::Num(1.0), &[], "GCD, MS examples #4"),
    c("gcd.three.common", "=GCD(10,20,30)", Want::Num(10.0), &[], "GCD, MS examples #5"),
    c("lcm.coprime", "=LCM(5,2)", Want::Num(10.0), &[], "LCM, MS examples #1"),
    c("lcm.common", "=LCM(24,36)", Want::Num(72.0), &[], "LCM, MS examples #2"),
    c("lcm.small", "=LCM(4,6)", Want::Num(12.0), &[], "LCM, MS examples #3"),
    // ROMAN / ARABIC / BASE / DECIMAL.
    c("roman.classic", "=ROMAN(499,0)", Want::Text("CDXCIX"), &[], "ROMAN, MS examples #1 (form 0)"),
    c("roman.form1", "=ROMAN(499,1)", Want::Text("LDVLIV"), &[], "ROMAN, MS examples #2 (form 1)"),
    c("roman.form2", "=ROMAN(499,2)", Want::Text("XDIX"), &[], "ROMAN, MS examples #3 (form 2)"),
    c("roman.form3", "=ROMAN(499,3)", Want::Text("VDIV"), &[], "ROMAN, MS examples #4 (form 3)"),
    c("roman.form4", "=ROMAN(499,4)", Want::Text("ID"), &[], "ROMAN, MS examples #5 (form 4)"),
    c("roman.2013", "=ROMAN(2013,0)", Want::Text("MMXIII"), &[], "ROMAN, MS examples #6"),
    c("arabic.lvii", "=ARABIC(\"LVII\")", Want::Num(57.0), &[], "ARABIC, MS examples #1"),
    c("arabic.lowercase", "=ARABIC(\"mcmxii\")", Want::Num(1912.0), &[], "ARABIC, MS examples #2"),
    c("arabic.negative", "=ARABIC(\"-IV\")", Want::Num(-4.0), &[], "ARABIC, MS examples #3"),
    c("base.binary.padded", "=BASE(15,2,10)", Want::Text("0000001111"), &[], "BASE, MS examples #1"),
    c("base.binary", "=BASE(15,2)", Want::Text("1111"), &[], "BASE, MS examples #2"),
    c("base.hex.padded", "=BASE(4095,16,4)", Want::Text("0FFF"), &[], "BASE, MS examples #3"),
    c("decimal.hex", "=DECIMAL(\"FF\",16)", Want::Num(255.0), &[], "DECIMAL, MS examples #1"),
    c("decimal.binary", "=DECIMAL(\"111\",2)", Want::Num(7.0), &[], "DECIMAL, MS examples #2"),
    c("decimal.base36", "=DECIMAL(\"zap\",36)", Want::Num(45745.0), &[], "DECIMAL, MS examples #3"),
    // CEILING / FLOOR and their .MATH forms.
    c("ceiling.positive", "=CEILING(2.5,1)", Want::Num(3.0), &[], "CEILING, MS examples #1"),
    c("ceiling.negative", "=CEILING(-2.5,-2)", Want::Num(-4.0), &[], "CEILING, MS examples #2"),
    c("ceiling.significance", "=CEILING(1.5,0.1)", Want::Num(1.5), &[], "CEILING, MS examples #3"),
    c("ceiling.small", "=CEILING(0.234,0.01)", Want::Num(0.24), &[], "CEILING, MS examples #4"),
    c("floor.positive", "=FLOOR(3.7,2)", Want::Num(2.0), &[], "FLOOR, MS examples #1"),
    c("floor.negative", "=FLOOR(-2.5,-2)", Want::Num(-2.0), &[], "FLOOR, MS examples #2"),
    c("floor.significance", "=FLOOR(1.58,0.1)", Want::Num(1.5), &[], "FLOOR, MS examples #3"),
    c("floor.small", "=FLOOR(0.234,0.01)", Want::Num(0.23), &[], "FLOOR, MS examples #4"),
    c("ceiling.math.significance", "=CEILING.MATH(24.3,5)", Want::Num(25.0), &[], "CEILING.MATH, MS examples #1"),
    c("ceiling.math.default", "=CEILING.MATH(6.7)", Want::Num(7.0), &[], "CEILING.MATH, MS examples #2"),
    c("ceiling.math.negative", "=CEILING.MATH(-8.1,2)", Want::Num(-8.0), &[], "CEILING.MATH, MS examples #3"),
    c("ceiling.math.mode", "=CEILING.MATH(-5.5,2,-1)", Want::Num(-6.0), &[], "CEILING.MATH, MS examples #4 (mode -1)"),
    c("floor.math.significance", "=FLOOR.MATH(24.3,5)", Want::Num(20.0), &[], "FLOOR.MATH, MS examples #1"),
    c("floor.math.default", "=FLOOR.MATH(6.7)", Want::Num(6.0), &[], "FLOOR.MATH, MS examples #2"),
    c("floor.math.negative", "=FLOOR.MATH(-8.1,2)", Want::Num(-10.0), &[], "FLOOR.MATH, MS examples #3"),
    c("floor.math.mode", "=FLOOR.MATH(-5.5,2,-1)", Want::Num(-4.0), &[], "FLOOR.MATH, MS examples #4 (mode -1)"),
    // ROUND / ROUNDUP / ROUNDDOWN / MROUND / TRUNC / INT.
    c("round.up", "=ROUND(2.15,1)", Want::Num(2.2), &[], "ROUND, MS examples #1"),
    c("round.down", "=ROUND(2.149,1)", Want::Num(2.1), &[], "ROUND, MS examples #2"),
    c("round.negative.half.away", "=ROUND(-1.475,2)", Want::Num(-1.48), &[], "ROUND, MS examples #3"),
    c("round.tens", "=ROUND(21.5,-1)", Want::Num(20.0), &[], "ROUND, MS examples #4 (digits < 0)"),
    c("roundup.positive", "=ROUNDUP(3.2,0)", Want::Num(4.0), &[], "ROUNDUP, MS examples #1"),
    c("roundup.tens", "=ROUNDUP(76.9,-1)", Want::Num(80.0), &[], "ROUNDUP, MS examples #2"),
    c("roundup.three.digits", "=ROUNDUP(3.14159,3)", Want::Num(3.142), &[], "ROUNDUP, MS examples #3"),
    c("rounddown.positive", "=ROUNDDOWN(3.2,0)", Want::Num(3.0), &[], "ROUNDDOWN, MS examples #1"),
    c("rounddown.tens", "=ROUNDDOWN(76.9,-1)", Want::Num(70.0), &[], "ROUNDDOWN, MS examples #2"),
    c("rounddown.three.digits", "=ROUNDDOWN(3.14159,3)", Want::Num(3.141), &[], "ROUNDDOWN, MS examples #3"),
    c("mround.down", "=MROUND(10,3)", Want::Num(9.0), &[], "MROUND, MS examples #1"),
    c("mround.negative", "=MROUND(-10,-3)", Want::Num(-9.0), &[], "MROUND, MS examples #2"),
    c("mround.decimal", "=MROUND(1.3,0.2)", Want::Num(1.4), &[], "MROUND, MS examples #3"),
    c("trunc.positive", "=TRUNC(8.9)", Want::Num(8.0), &[], "TRUNC, MS examples #1"),
    c("trunc.negative", "=TRUNC(-8.9)", Want::Num(-8.0), &[], "TRUNC, MS examples #2"),
    c("trunc.pi", "=TRUNC(PI(),2)", Want::Num(3.14), &[], "TRUNC, MS examples #3 (PI() with digits)"),
    c("int.positive", "=INT(8.9)", Want::Num(8.0), &[], "INT, MS examples #1"),
    c("int.negative", "=INT(-8.9)", Want::Num(-9.0), &[], "INT, MS examples #2"),
    // ABS / SIGN / POWER / SQRT / SQRTPI / EXP / LN / LOG / LOG10.
    c("abs.negative", "=ABS(-4)", Want::Num(4.0), &[], "ABS, MS examples #1"),
    c("abs.positive", "=ABS(2)", Want::Num(2.0), &[], "ABS, MS examples #2"),
    c("abs.decimal", "=ABS(-2.5)", Want::Num(2.5), &[], "ABS, MS examples #3"),
    c("sign.positive", "=SIGN(10)", Want::Num(1.0), &[], "SIGN, MS examples #1"),
    c("sign.zero", "=SIGN(0)", Want::Num(0.0), &[], "SIGN, MS examples #2"),
    c("sign.negative", "=SIGN(-0.00001)", Want::Num(-1.0), &[], "SIGN, MS examples #3"),
    c("power.square", "=POWER(5,2)", Want::Num(25.0), &[], "POWER, MS examples #1"),
    r("power.fractional", "=POWER(98.6,3.2)", Want::Num(2401077.222), 5e-4, &[], "POWER, MS examples #2"),
    c("sqrt.square", "=SQRT(16)", Want::Num(4.0), &[], "SQRT, MS examples #1"),
    c("sqrt.negative", "=SQRT(-16)", Want::Err(CellError::Num), &[], "SQRT, MS examples #2 (#NUM!)"),
    c("sqrtpi.one", "=SQRTPI(1)", Want::Num(1.772453851), &[], "SQRTPI, MS examples #1"),
    c("sqrtpi.two", "=SQRTPI(2)", Want::Num(2.506628275), &[], "SQRTPI, MS examples #2"),
    c("exp.e", "=EXP(1)", Want::Num(2.718281828), &[], "EXP, MS examples #1"),
    c("exp.two", "=EXP(2)", Want::Num(7.389056099), &[], "EXP, MS examples #2"),
    r("ln.86", "=LN(86)", Want::Num(4.4543473), 5e-8, &[], "LN, MS examples #1"),
    r("ln.e", "=LN(2.7182818)", Want::Num(1.0), 5e-8, &[], "LN, MS examples #2"),
    c("ln.exp", "=LN(EXP(3))", Want::Num(3.0), &[], "LN, MS examples #3"),
    c("log.ten", "=LOG(10)", Want::Num(1.0), &[], "LOG, MS examples #1"),
    c("log.base2", "=LOG(8,2)", Want::Num(3.0), &[], "LOG, MS examples #2"),
    r("log.e", "=LOG(86,2.7182818)", Want::Num(4.4543473), 5e-7, &[], "LOG, MS examples #3"),
    c("log10.86", "=LOG10(86)", Want::Num(1.934498451), &[], "LOG10, MS examples #1"),
    c("log10.power", "=LOG10(100000)", Want::Num(5.0), &[], "LOG10, MS examples #2"),
    // FACT / FACTDOUBLE / COMBIN / COMBINA / PERMUT.
    c("fact.five", "=FACT(5)", Want::Num(120.0), &[], "FACT, MS examples #1"),
    c("fact.truncates", "=FACT(1.9)", Want::Num(1.0), &[], "FACT, MS examples #2 (number is truncated)"),
    c("fact.zero", "=FACT(0)", Want::Num(1.0), &[], "FACT, MS examples #3"),
    c("fact.negative", "=FACT(-1)", Want::Err(CellError::Num), &[], "FACT, MS examples #4 (#NUM!)"),
    c("fact.one", "=FACT(1)", Want::Num(1.0), &[], "FACT, MS examples #5"),
    c("factdouble.six", "=FACTDOUBLE(6)", Want::Num(48.0), &[], "FACTDOUBLE, MS examples #1"),
    c("factdouble.seven", "=FACTDOUBLE(7)", Want::Num(105.0), &[], "FACTDOUBLE, MS examples #2"),
    c("combin.eight.two", "=COMBIN(8,2)", Want::Num(28.0), &[], "COMBIN, MS examples #1"),
    c("combina.four.three", "=COMBINA(4,3)", Want::Num(20.0), &[], "COMBINA, MS examples #1"),
    c("combina.ten.three", "=COMBINA(10,3)", Want::Num(220.0), &[], "COMBINA, MS examples #2"),
    c("permut.three.two", "=PERMUT(3,2)", Want::Num(6.0), &[], "PERMUT, MS examples #1"),
    // EVEN / ODD.
    c("even.round.up", "=EVEN(1.5)", Want::Num(2.0), &[], "EVEN, MS examples #1"),
    c("even.already.even", "=EVEN(3)", Want::Num(4.0), &[], "EVEN, MS examples #2"),
    c("even.negative", "=EVEN(-1)", Want::Num(-2.0), &[], "EVEN, MS examples #3"),
    c("odd.round.up", "=ODD(1.5)", Want::Num(3.0), &[], "ODD, MS examples #1"),
    c("odd.already.odd", "=ODD(3)", Want::Num(3.0), &[], "ODD, MS examples #2"),
    c("odd.positive", "=ODD(2)", Want::Num(3.0), &[], "ODD, MS examples #3"),
    c("odd.negative", "=ODD(-1)", Want::Num(-1.0), &[], "ODD, MS examples #4"),
];

// ---------------------------------------------------------------------------------------------
// Engineering
// ---------------------------------------------------------------------------------------------

const ENGINEERING_CASES: &[Case] = &[
    // CONVERT, the page's literal unit examples.
    c("convert.lbm.kg", "=CONVERT(1,\"lbm\",\"kg\")", Want::Num(0.45359237), &[], "CONVERT, MS examples #1"),
    c("convert.f.c", "=CONVERT(68,\"F\",\"C\")", Want::Num(20.0), &[], "CONVERT, MS examples #2"),
    c("convert.incompatible", "=CONVERT(2.5,\"ft\",\"sec\")", Want::Err(CellError::NA), &[], "CONVERT, MS examples #3 (#N/A)"),
    c("convert.roundtrip", "=CONVERT(CONVERT(100,\"ft\",\"m\"),\"ft\",\"m\")", Want::Num(9.290304), &[], "CONVERT, MS examples #4"),
    c("convert.tsp.tbs", "=CONVERT(1,\"tsp\",\"tbs\")", Want::Num(1.0 / 3.0), &[], "CONVERT, MS volume example (tsp to tbs)"),
    c("convert.gal.l", "=CONVERT(1,\"gal\",\"l\")", Want::Num(3.785411784), &[], "CONVERT, MS volume example (gal to l)"),
    c("convert.mi.km", "=CONVERT(1,\"mi\",\"km\")", Want::Num(1.609344), &[], "CONVERT, MS distance example (mi to km)"),
    c("convert.day.hr", "=CONVERT(1,\"day\",\"hr\")", Want::Num(24.0), &[], "CONVERT, MS time example (day to hr)"),
    // Bitwise functions, literal examples.
    c("bitand.mask", "=BITAND(13,25)", Want::Num(9.0), &[], "BITAND, MS examples #1"),
    c("bitor.bits", "=BITOR(23,10)", Want::Num(31.0), &[], "BITOR, MS examples #1"),
    c("bitxor.bits", "=BITXOR(5,3)", Want::Num(6.0), &[], "BITXOR, MS examples #1"),
    c("bitlshift.bits", "=BITLSHIFT(4,2)", Want::Num(16.0), &[], "BITLSHIFT, MS examples #1"),
    c("bitrshift.bits", "=BITRSHIFT(13,2)", Want::Num(3.0), &[], "BITRSHIFT, MS examples #1"),
    // Decimal to base conversions.
    c("dec2bin.padded", "=DEC2BIN(9,4)", Want::Text("1001"), &[], "DEC2BIN, MS examples #1"),
    c("dec2bin.negative", "=DEC2BIN(-100)", Want::Text("1110011100"), &[], "DEC2BIN, MS examples #2"),
    c("dec2bin.plain", "=DEC2BIN(100)", Want::Text("1100100"), &[], "DEC2BIN, MS examples #3"),
    c("dec2oct.padded", "=DEC2OCT(58,3)", Want::Text("072"), &[], "DEC2OCT, MS examples #1"),
    c("dec2oct.negative", "=DEC2OCT(-100)", Want::Text("7777777634"), &[], "DEC2OCT, MS examples #2"),
    c("dec2hex.padded", "=DEC2HEX(100,4)", Want::Text("0064"), &[], "DEC2HEX, MS examples #1"),
    c("dec2hex.negative", "=DEC2HEX(-54)", Want::Text("FFFFFFFFCA"), &[], "DEC2HEX, MS examples #2"),
    c("dec2hex.plain", "=DEC2HEX(28)", Want::Text("1C"), &[], "DEC2HEX, MS examples #3"),
    // Binary to decimal / hex / octal.
    c("bin2dec.small", "=BIN2DEC(1100100)", Want::Num(100.0), &[], "BIN2DEC, MS examples #1"),
    c("bin2dec.negative", "=BIN2DEC(1111111111)", Want::Num(-1.0), &[], "BIN2DEC, MS examples #2"),
    c("bin2hex.padded", "=BIN2HEX(11111011,4)", Want::Text("00FB"), &[], "BIN2HEX, MS examples #1"),
    c("bin2hex.short", "=BIN2HEX(1110)", Want::Text("E"), &[], "BIN2HEX, MS examples #2"),
    c("bin2hex.negative", "=BIN2HEX(1111111111)", Want::Text("FFFFFFFFFF"), &[], "BIN2HEX, MS examples #3"),
    c("bin2oct.padded", "=BIN2OCT(1001,3)", Want::Text("011"), &[], "BIN2OCT, MS examples #1"),
    c("bin2oct.plain", "=BIN2OCT(1100100)", Want::Text("144"), &[], "BIN2OCT, MS examples #2"),
    // Hex to decimal / binary / octal.
    c("hex2dec.a5", "=HEX2DEC(\"A5\")", Want::Num(165.0), &[], "HEX2DEC, MS examples #1"),
    c("hex2dec.negative", "=HEX2DEC(\"FFFFFFFF5B\")", Want::Num(-165.0), &[], "HEX2DEC, MS examples #2"),
    c("hex2dec.large", "=HEX2DEC(\"3DA408B9\")", Want::Num(1034160313.0), &[], "HEX2DEC, MS examples #3"),
    c("hex2bin.padded", "=HEX2BIN(\"F\",8)", Want::Text("00001111"), &[], "HEX2BIN, MS examples #1"),
    c("hex2bin.byte", "=HEX2BIN(\"B7\")", Want::Text("10110111"), &[], "HEX2BIN, MS examples #2"),
    c("hex2bin.negative", "=HEX2BIN(\"FFFFFFFFFF\")", Want::Text("1111111111"), &[], "HEX2BIN, MS examples #3"),
    c("hex2oct.padded", "=HEX2OCT(\"F\",3)", Want::Text("017"), &[], "HEX2OCT, MS examples #1"),
    c("hex2oct.3b4e", "=HEX2OCT(\"3B4E\")", Want::Text("35516"), &[], "HEX2OCT, MS examples #2"),
    c("hex2oct.negative", "=HEX2OCT(\"FFFFFFFF00\")", Want::Text("7777777400"), &[], "HEX2OCT, MS examples #3"),
    // Octal to decimal / binary / hex.
    c("oct2dec.54", "=OCT2DEC(54)", Want::Num(44.0), &[], "OCT2DEC, MS examples #1"),
    c("oct2dec.negative", "=OCT2DEC(7777777533)", Want::Num(-165.0), &[], "OCT2DEC, MS examples #2"),
    c("oct2bin.padded", "=OCT2BIN(3,3)", Want::Text("011"), &[], "OCT2BIN, MS examples #1"),
    c("oct2bin.negative", "=OCT2BIN(7777777000)", Want::Text("1000000000"), &[], "OCT2BIN, MS examples #2"),
    c("oct2hex.padded", "=OCT2HEX(100,4)", Want::Text("0040"), &[], "OCT2HEX, MS examples #1"),
    c("oct2hex.negative", "=OCT2HEX(7777777533)", Want::Text("FFFFFFFF5B"), &[], "OCT2HEX, MS examples #2"),
    // Complex numbers, the pages' literal examples.
    c("complex.rectangular", "=COMPLEX(3,4)", Want::Text("3+4i"), &[], "COMPLEX, MS examples #1"),
    c("complex.j.suffix", "=COMPLEX(3,4,\"j\")", Want::Text("3+4j"), &[], "COMPLEX, MS examples #2"),
    c("complex.pure.imaginary", "=COMPLEX(0,1)", Want::Text("i"), &[], "COMPLEX, MS examples #3"),
    c("complex.real.only", "=COMPLEX(1,0)", Want::Text("1"), &[], "COMPLEX, MS examples #4"),
    c("imabs.modulus", "=IMABS(\"5+12i\")", Want::Num(13.0), &[], "IMABS, MS examples #1"),
    c("imsum.sum", "=IMSUM(\"3+4i\",\"5-3i\")", Want::Text("8+i"), &[], "IMSUM, MS examples #1"),
    c("improduct.product", "=IMPRODUCT(\"3+4i\",\"5-3i\")", Want::Text("27+11i"), &[], "IMPRODUCT, MS examples #1"),
    c("imsub.difference", "=IMSUB(\"13+4j\",\"5+3j\")", Want::Text("8+j"), &[], "IMSUB, MS examples #1"),
    c("imdiv.quotient", "=IMDIV(\"-238+240i\",\"10+24i\")", Want::Text("5+12i"), &[], "IMDIV, MS examples #1"),
    c("imreal.real", "=IMREAL(\"6-9i\")", Want::Num(6.0), &[], "IMREAL, MS examples #1"),
    c("imaginary.imaginary", "=IMAGINARY(\"6-9i\")", Want::Num(-9.0), &[], "IMAGINARY, MS examples #1"),
    c("imconjugate.conjugate", "=IMCONJUGATE(\"3+4i\")", Want::Text("3-4i"), &[], "IMCONJUGATE, MS examples #1"),
    c("imargument.angle", "=IMARGUMENT(\"3+4i\")", Want::Num(0.927295218), &[], "IMARGUMENT, MS examples #1"),
    // DELTA / GESTEP / ERF / ERFC, literal examples.
    c("delta.different", "=DELTA(5,4)", Want::Num(0.0), &[], "DELTA, MS examples #1"),
    c("delta.equal", "=DELTA(5,5)", Want::Num(1.0), &[], "DELTA, MS examples #2"),
    c("gestep.greater", "=GESTEP(5,4)", Want::Num(1.0), &[], "GESTEP, MS examples #1"),
    c("gestep.equal", "=GESTEP(5,5)", Want::Num(1.0), &[], "GESTEP, MS examples #2"),
    c("gestep.negative", "=GESTEP(-4,-5)", Want::Num(1.0), &[], "GESTEP, MS examples #3"),
    c("gestep.default", "=GESTEP(-1)", Want::Num(0.0), &[], "GESTEP, MS examples #4 (step omitted)"),
    r("erf.0745", "=ERF(0.745)", Want::Num(0.70792892), 5e-9, &[], "ERF, MS examples #1"),
    c("erf.one", "=ERF(1)", Want::Num(0.842700793), &[], "ERF, MS examples #2"),
    c("erfc.one", "=ERFC(1)", Want::Num(0.157299207), &[], "ERFC, MS examples #1"),
];

// ---------------------------------------------------------------------------------------------
// Information and database
// ---------------------------------------------------------------------------------------------

const DB_TABLE: &[(&str, &str)] = &[
    ("A1", "Tree"),
    ("B1", "Height"),
    ("C1", "Yield"),
    ("A2", "Apple"),
    ("B2", "18"),
    ("C2", "20"),
    ("A3", "Pear"),
    ("B3", "12"),
    ("C3", "12"),
    ("A4", "Cherry"),
    ("B4", "13"),
    ("C4", "14"),
    ("A5", "Apple"),
    ("B5", "14"),
    ("C5", "15"),
    ("A6", "Pear"),
    ("B6", "9"),
    ("C6", "8"),
    ("E4", "Tree"),
    ("E5", "Apple"),
    ("G4", "Tree"),
    ("H4", "Height"),
    ("G5", "Apple"),
    ("H5", "18"),
];

const INFORMATION_CASES: &[Case] = &[
    // ISBLANK / ISERR / ISERROR / ISNA.
    c("isblank.empty", "=ISBLANK(A1)", Want::Bool(true), &[("A1", "")], "ISBLANK, MS examples #1 (empty cell)"),
    c("isblank.value", "=ISBLANK(1)", Want::Bool(false), &[], "ISBLANK, MS examples #2 (non-empty)"),
    c("isblank.empty.text", "=ISBLANK(\"\")", Want::Bool(false), &[], "ISBLANK, MS examples #3 (\"\" is text, not blank)"),
    c("iserr.na", "=ISERR(NA())", Want::Bool(false), &[], "ISERR, MS examples #1 (#N/A is excluded)"),
    c("iserr.div0", "=ISERR(1/0)", Want::Bool(true), &[], "ISERR, MS examples #2"),
    c("iserror.na", "=ISERROR(NA())", Want::Bool(true), &[], "ISERROR, MS examples #1"),
    c("iserror.value", "=ISERROR(10)", Want::Bool(false), &[], "ISERROR, MS examples #2"),
    c("isna.na", "=ISNA(NA())", Want::Bool(true), &[], "ISNA, MS examples #1"),
    c("isna.div0", "=ISNA(1/0)", Want::Bool(false), &[], "ISNA, MS examples #2"),
    // ISNUMBER / ISTEXT / ISNONTEXT / ISLOGICAL / ISREF.
    c("isnumber.number", "=ISNUMBER(1)", Want::Bool(true), &[], "ISNUMBER, MS examples #1"),
    c("isnumber.text", "=ISNUMBER(\"x\")", Want::Bool(false), &[], "ISNUMBER, MS examples #2"),
    c("istext.text", "=ISTEXT(\"x\")", Want::Bool(true), &[], "ISTEXT, MS examples #1"),
    c("istext.number", "=ISTEXT(1)", Want::Bool(false), &[], "ISTEXT, MS examples #2"),
    c("isnontext.number", "=ISNONTEXT(1)", Want::Bool(true), &[], "ISNONTEXT, MS examples #1"),
    c("isnontext.text", "=ISNONTEXT(\"x\")", Want::Bool(false), &[], "ISNONTEXT, MS examples #2"),
    c("islogical.true", "=ISLOGICAL(TRUE)", Want::Bool(true), &[], "ISLOGICAL, MS examples #1"),
    c("islogical.number", "=ISLOGICAL(1)", Want::Bool(false), &[], "ISLOGICAL, MS examples #2"),
    c("isref.reference", "=ISREF(A1)", Want::Bool(true), &[], "ISREF, MS examples #1"),
    c("isref.value", "=ISREF(1)", Want::Bool(false), &[], "ISREF, MS examples #2"),
    // ISEVEN / ISODD.
    c("iseven.decimal", "=ISEVEN(2.5)", Want::Bool(true), &[], "ISEVEN, MS examples #1 (truncates)"),
    c("iseven.negative.odd", "=ISEVEN(-1)", Want::Bool(false), &[], "ISEVEN, MS examples #2"),
    c("isodd.number", "=ISODD(3)", Want::Bool(true), &[], "ISODD, MS examples #1"),
    c("isodd.decimal", "=ISODD(2.5)", Want::Bool(false), &[], "ISODD, MS examples #2"),
    // N / NA / TYPE.
    c("n.number", "=N(7)", Want::Num(7.0), &[], "N, MS examples #1"),
    c("n.text", "=N(\"x\")", Want::Num(0.0), &[], "N, MS examples #2 (text gives 0)"),
    c("n.logical", "=N(TRUE)", Want::Num(1.0), &[], "N, MS examples #3 (TRUE gives 1)"),
    c("n.date", "=N(DATE(2020,1,1))", Want::Num(43831.0), &[], "N, MS examples #4 (a date is its serial)"),
    c("na.value", "=NA()", Want::Err(CellError::NA), &[], "NA, MS examples #1"),
    c("type.number", "=TYPE(1)", Want::Num(1.0), &[], "TYPE, MS examples #1"),
    c("type.text", "=TYPE(\"x\")", Want::Num(2.0), &[], "TYPE, MS examples #2"),
    c("type.logical", "=TYPE(TRUE)", Want::Num(4.0), &[], "TYPE, MS examples #3"),
    c("type.error", "=TYPE(NA())", Want::Num(16.0), &[], "TYPE, MS examples #4"),
    c("type.array", "=TYPE({1,2})", Want::Num(64.0), &[], "TYPE, MS examples #5 (array)"),
    // ERROR.TYPE: the documented error numbers.
    c("error.type.null", "=ERROR.TYPE(#NULL!)", Want::Num(1.0), &[], "ERROR.TYPE, MS examples #1 (#NULL!)"),
    c("error.type.div0", "=ERROR.TYPE(1/0)", Want::Num(2.0), &[], "ERROR.TYPE, MS examples #1 (#DIV/0!)"),
    c("error.type.value", "=ERROR.TYPE(\"x\"+1)", Want::Num(3.0), &[], "ERROR.TYPE, MS examples #2 (#VALUE!)"),
    c("error.type.ref", "=ERROR.TYPE(INDIRECT(\"bad\"))", Want::Num(4.0), &[], "ERROR.TYPE, MS examples #3 (#REF!)"),
    c("error.type.name", "=ERROR.TYPE(NOSUCHFUNC())", Want::Num(5.0), &[], "ERROR.TYPE, MS examples #4 (#NAME?)"),
    c("error.type.num", "=ERROR.TYPE(SQRT(-1))", Want::Num(6.0), &[], "ERROR.TYPE, MS examples #5 (#NUM!)"),
    c("error.type.na", "=ERROR.TYPE(NA())", Want::Num(7.0), &[], "ERROR.TYPE, MS examples #6 (#N/A)"),
    // SHEET / SHEETS.
    c("sheet.current", "=SHEET()", Want::Num(1.0), &[], "SHEET, MS examples #1"),
    c("sheet.by.name", "=SHEET(\"Sheet1\")", Want::Num(1.0), &[], "SHEET, MS examples #2 (sheet name)"),
    c("sheet.unknown", "=SHEET(\"NoSuch\")", Want::Err(CellError::NA), &[], "SHEET, MS examples #3 (unknown name)"),
    c("sheets.count", "=SHEETS()", Want::Num(1.0), &[], "SHEETS, MS examples #1"),
    // CELL with its documented info_type codes.
    c("cell.address", "=CELL(\"address\",B3)", Want::Text("$B$3"), &[], "CELL, MS examples #1 (\"address\")"),
    c("cell.col", "=CELL(\"col\",B3)", Want::Num(2.0), &[], "CELL, MS examples #2 (\"col\")"),
    c("cell.row", "=CELL(\"row\",B3)", Want::Num(3.0), &[], "CELL, MS examples #3 (\"row\")"),
    c("cell.contents", "=CELL(\"contents\",B3)", Want::Num(5.0), &[("B3", "5")], "CELL, MS examples #4 (\"contents\")"),
    c("cell.type.value", "=CELL(\"type\",B3)", Want::Text("v"), &[("B3", "5")], "CELL, MS examples #5 (\"type\" = value)"),
    c("cell.type.blank", "=CELL(\"type\",B3)", Want::Text("b"), &[], "CELL, MS examples #6 (\"type\" = blank)"),
    c("cell.type.label", "=CELL(\"type\",B3)", Want::Text("l"), &[("B3", "text")], "CELL, MS examples #7 (\"type\" = label)"),
    // FORMULATEXT and ISFORMULA need a real formula cell.
    c("formulatext.formula", "=FORMULATEXT(B3)", Want::Text("=SUM(1,2)"), &[("B3", "=SUM(1,2)")], "FORMULATEXT, MS examples #1"),
    c("formulatext.not.formula", "=FORMULATEXT(B3)", Want::Err(CellError::NA), &[("B3", "5")], "FORMULATEXT, MS examples #2 (#N/A for a value)"),
    c("isformula.formula", "=ISFORMULA(B3)", Want::Bool(true), &[("B3", "=SUM(1,2)")], "ISFORMULA, MS examples #1"),
    c("isformula.value", "=ISFORMULA(B3)", Want::Bool(false), &[("B3", "5")], "ISFORMULA, MS examples #2"),
];

const DATABASE_CASES: &[Case] = &[
    // The Database page's criteria form, with a small fruit table of our own.
    c("dsum.apple.yield", "=DSUM(A1:C6,\"Yield\",E4:E5)", Want::Num(35.0), DB_TABLE, "DSUM, MS documented criteria form (mini table)"),
    c(
        "dsum.criteria.field",
        "=DSUM(A1:C6,C1,E4:E5)",
        Want::Num(35.0),
        DB_TABLE,
        "DSUM, MS documented criteria form, field as a cell reference (mini table)",
    ),
    c("dcount.apple.records", "=DCOUNT(A1:C6,\"Yield\",E4:E5)", Want::Num(2.0), DB_TABLE, "DCOUNT, MS documented criteria form (mini table)"),
    c("dcount.no.field", "=DCOUNT(A1:C6,,E4:E5)", Want::Num(2.0), DB_TABLE, "DCOUNT, MS documented criteria form, field omitted (mini table)"),
    c("daverage.apple.yield", "=DAVERAGE(A1:C6,\"Yield\",E4:E5)", Want::Num(17.5), DB_TABLE, "DAVERAGE, MS documented criteria form (mini table)"),
    c("dget.unique", "=DGET(A1:C6,\"Yield\",G4:H5)", Want::Num(20.0), DB_TABLE, "DGET, MS documented criteria form, one record matches (mini table)"),
    c(
        "dget.multiple",
        "=DGET(A1:C6,\"Yield\",E4:E5)",
        Want::Err(CellError::Num),
        DB_TABLE,
        "DGET, MS documented criteria form, #NUM! for several matches (mini table)",
    ),
];

// ---------------------------------------------------------------------------------------------
// LET and LAMBDA-era special forms (evaluated by the host, not the function library)
// ---------------------------------------------------------------------------------------------

const LAMBDA_CASES: &[Case] = &[
    c("let.simple", "=LET(x,5,x+1)", Want::Num(6.0), &[], "LET, MS examples #1"),
    c("let.two.names", "=LET(a,2,b,3,a*b)", Want::Num(6.0), &[], "LET, MS examples #2 (two names)"),
    c("lambda.call", "=LAMBDA(x,x+1)(2)", Want::Num(3.0), &[], "LAMBDA, MS examples #1"),
    c("lambda.two.arguments", "=LAMBDA(x,y,x*y)(3,4)", Want::Num(12.0), &[], "LAMBDA, MS examples #2 (two parameters)"),
    c(
        "isomitted.omitted",
        "=LAMBDA(x,y,IF(ISOMITTED(y),\"Missing\",x+y))(1,)",
        Want::Text("Missing"),
        &[],
        "ISOMITTED, MS examples #1 (omitted trailing argument)",
    ),
    c(
        "isomitted.present",
        "=LAMBDA(x,y,IF(ISOMITTED(y),\"Missing\",x+y))(1,2)",
        Want::Num(3.0),
        &[],
        "ISOMITTED, MS examples #2 (argument supplied)",
    ),
    c(
        "map.double",
        "=MAP({1,2,3},LAMBDA(x,x*2))",
        Want::Arr { rows: 1, cols: 3, data: &[Want::Num(2.0), Want::Num(4.0), Want::Num(6.0)] },
        &[],
        "MAP, MS examples #1",
    ),
    c("reduce.sum", "=REDUCE(0,{1,2,3},LAMBDA(a,b,a+b))", Want::Num(6.0), &[], "REDUCE, MS examples #1"),
    c(
        "scan.running.sum",
        "=SCAN(0,{1,2,3},LAMBDA(a,b,a+b))",
        Want::Arr { rows: 1, cols: 3, data: &[Want::Num(1.0), Want::Num(3.0), Want::Num(6.0)] },
        &[],
        "SCAN, MS examples #1",
    ),
    c(
        "byrow.row.sums",
        "=BYROW({1,2;3,4},LAMBDA(r,SUM(r)))",
        Want::Arr { rows: 2, cols: 1, data: &[Want::Num(3.0), Want::Num(7.0)] },
        &[],
        "BYROW, MS examples #1",
    ),
    c(
        "bycol.column.sums",
        "=BYCOL({1,2;3,4},LAMBDA(c,SUM(c)))",
        Want::Arr { rows: 1, cols: 2, data: &[Want::Num(4.0), Want::Num(6.0)] },
        &[],
        "BYCOL, MS examples #1",
    ),
    c(
        "makearray.index.pair",
        "=MAKEARRAY(2,2,LAMBDA(r,c,r*10+c))",
        Want::Arr { rows: 2, cols: 2, data: &[Want::Num(11.0), Want::Num(12.0), Want::Num(21.0), Want::Num(22.0)] },
        &[],
        "MAKEARRAY, MS examples #1",
    ),
];

// ---------------------------------------------------------------------------------------------
// Logical
// ---------------------------------------------------------------------------------------------

const LOGICAL_CASES: &[Case] = &[
    c("if.true", "=IF(TRUE,1,2)", Want::Num(1.0), &[], "IF, MS example #1 (logical_test TRUE)"),
    c("if.false", "=IF(FALSE,1,2)", Want::Num(2.0), &[], "IF, MS example #1 (logical_test FALSE)"),
    c("if.comparison", "=IF(A1>10,\"Over Budget\",\"OK\")", Want::Text("Over Budget"), &[("A1", "20")], "IF, MS example #2 (A2>B2 style)"),
    c("and.all", "=AND(TRUE,TRUE)", Want::Bool(true), &[], "AND, MS example #1"),
    c("and.one.false", "=AND(TRUE,FALSE)", Want::Bool(false), &[], "AND, MS example #2"),
    c("or.all.false", "=OR(FALSE,FALSE)", Want::Bool(false), &[], "OR, MS example #1"),
    c("or.one.true", "=OR(FALSE,TRUE)", Want::Bool(true), &[], "OR, MS example #2"),
    c("xor.one", "=XOR(TRUE,FALSE)", Want::Bool(true), &[], "XOR, MS example #1"),
    c("xor.both", "=XOR(TRUE,TRUE)", Want::Bool(false), &[], "XOR, MS example #2"),
    c("not.true", "=NOT(TRUE)", Want::Bool(false), &[], "NOT, MS example #1"),
    c("iferror.value", "=IFERROR(1/0,\"error\")", Want::Text("error"), &[], "IFERROR, MS example #1"),
    c("iferror.clean", "=IFERROR(10,\"error\")", Want::Num(10.0), &[], "IFERROR, MS example #2"),
    c("ifna.na", "=IFNA(NA(),\"not found\")", Want::Text("not found"), &[], "IFNA, MS example #1"),
    c("ifna.other.error", "=IFNA(1/0,\"not found\")", Want::Err(CellError::Div0), &[], "IFNA, MS example #2 (only #N/A is replaced)"),
    c("ifs.first", "=IFS(A1>10,\"big\",A1>5,\"medium\",TRUE,\"small\")", Want::Text("big"), &[("A1", "20")], "IFS, MS example #1"),
    c("ifs.third", "=IFS(A1>10,\"big\",A1>5,\"medium\",TRUE,\"small\")", Want::Text("small"), &[("A1", "1")], "IFS, MS example #2"),
    c("switch.match", "=SWITCH(3,1,\"one\",2,\"two\",3,\"three\",\"other\")", Want::Text("three"), &[], "SWITCH, MS example #1"),
    c("switch.default", "=SWITCH(9,1,\"one\",2,\"two\",3,\"three\",\"other\")", Want::Text("other"), &[], "SWITCH, MS example #2 (default)"),
];
