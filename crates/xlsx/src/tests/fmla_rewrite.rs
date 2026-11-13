//! What a formula is written as: the fast path that keeps the canonical text and the rewrite
//! that spells a file's form of it must agree, formula by formula.

use gridcraft_model::Formula;

use crate::fmla::{self, to_file};

/// The rewrite as it stood when every formula cell went through it. Kept as the oracle: the
/// fast path is an assertion that this function is not needed.
fn rewritten(f: &Formula) -> String {
    match f.expr() {
        Some(e) => crate::fmla::expr_to_file_for_test(e),
        None => f.text.clone(),
    }
}

/// Every kind of formula a file spells differently, plus ordinary ones the fast path must leave
/// alone. The scan behind the fast path is only correct if it agrees with the rewrite on all of
/// them.
const FORMULAS: &[&str] = &[
    // A file says what the model says: the fast path writes the canonical text.
    "=1+2",
    "=A1",
    "=SUM(A1:A3)",
    "=IF(A1>0,\"yes\",\"no\")",
    "=Sheet2!$B$4*0.5",
    "=A1+A2+A3+A4+A5+A6+A7+A8",
    "=VLOOKUP(A1,B:C,2,FALSE)",
    "=CONCAT(A1,\" - \",B1)",
    "={1,2;3,4}",
    // A file spells these out.
    "=XLOOKUP(A1,B:B,C:C)",
    "=LET(x,1,x+1)",
    "=LAMBDA(x,x*2)(3)",
    "=A1#",
    "=@A1:B2",
    "=SUM(A1:.E10)",
    "=Table1[Sales]",
    "=SUM(Table1[Sales])",
    "=Table1[[#Headers],[Sales]]",
    "=Table1[[#All],[Sales]]",
    "=Table1[[#Data],[Sales]:[Cost]]",
    "=Table1[]",
    "=Table1[[#This Row],[Sales]]",
    "=Table1[@Sales]",
    "=Table1[@]",
    "=SUM(Table1[Sales])+A1#",
];

#[test]
fn the_fast_path_writes_what_the_rewrite_writes() {
    for text in FORMULAS {
        let f = Formula::new(text);
        assert!(f.expr_arc().is_some(), "{text} must parse for this test to mean anything");
        assert_eq!(to_file(&f), rewritten(&f), "{text}");
    }
}

/// The scan must not be conservative in the wrong direction: every formula it lets through has to
/// be one the rewrite leaves alone, and every one it sends to the rewrite has to be one the
/// rewrite actually changes.
#[test]
fn the_rewrite_is_skipped_exactly_when_it_changes_nothing() {
    for text in FORMULAS {
        let f = Formula::new(text);
        let Some(e) = f.expr_arc() else { continue };
        // A file spells a formula differently exactly when the rewrite's text differs from the
        // canonical text.
        let changes = fmla::expr_to_file_for_test((*e).clone()) != gridcraft_formula::print(&e);
        assert_eq!(fmla::needs_file_rewrite_for_test(&e), changes, "{text}: the scan and the rewrite disagree (the file text would be wrong)");
    }
}

/// An unparseable formula is written as it stands, both ways.
#[test]
fn text_that_does_not_parse_is_kept() {
    let f = Formula::new("=SUM(A1:A3");
    assert!(f.expr_arc().is_none());
    assert_eq!(to_file(&f), f.text);
    assert_eq!(to_file(&f), rewritten(&f));
}
