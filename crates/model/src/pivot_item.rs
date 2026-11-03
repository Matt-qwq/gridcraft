//! Calculated PivotTable items: a formula over the items of one field (`=North+South`).
//!
//! Excel stores a calculated item's formula in the field/item form
//! `Category[North]+Category[South]` and shows it in the Calculated Item dialog as
//! `=North+South`; both are accepted here, and the two printers produce one from the other.
//! The grammar is Excel's arithmetic over one field's item captions: numbers, `+ - * / ^`,
//! parentheses and items of the same field — no functions, cell references or other fields.
//! An item caption that cannot be written bare (a number, or one holding an operator) is always
//! written `Field[item]`, which is also how Excel stores it.
//!
//! A calculated item may not refer to another calculated item: [`compile`] only knows captions,
//! and the caller resolves them against the field's real items (so a self-reference is simply an
//! unknown name).

use gridcraft_core::{CellError, Value};
use gridcraft_formula::{BinOp, Expr, UnOp};

/// Longest formula accepted, in characters (Excel's limit for formula contents).
pub const MAX_FORMULA_LEN: usize = 8_192;
/// Most nodes a formula may hold; it bounds the recursion of parsing, printing and evaluation.
pub const MAX_TERMS: usize = 4_096;

const PREFIX: &str = "__pci";

/// The arithmetic a calculated item computes over its field's item aggregates.
#[derive(Clone, Debug, PartialEq)]
pub enum Term {
    Number(f64),
    /// The value of `items()[i]` (an item of the calculated item's own field).
    Item(usize),
    Neg(Box<Term>),
    Binary(BinOp, Box<Term>, Box<Term>),
}

/// A parsed calculated-item formula.
#[derive(Clone, Debug, PartialEq)]
pub struct Compiled {
    term: Term,
    /// Item captions in first-appearance order.
    items: Vec<String>,
}

impl Compiled {
    /// What the formula computes.
    pub fn term(&self) -> &Term {
        &self.term
    }
    /// The item captions the formula names, in first-appearance order.
    pub fn items(&self) -> &[String] {
        &self.items
    }
    /// Excel's stored form: `Category[North]+Category[South]`.
    pub fn stored_form(&self, field: &str) -> String {
        self.print(field, false)
    }
    /// The dialog form: `=North+South` — captions that cannot be written bare stay
    /// `Field[caption]`, so the text always parses back.
    pub fn dialog_form(&self, field: &str) -> String {
        self.print(field, true)
    }
    fn print(&self, field: &str, dialog: bool) -> String {
        let mut s = String::new();
        if dialog {
            s.push('=');
        }
        print_term(&self.term, field, dialog, &self.items, 0, &mut s);
        s
    }
}

/// Parses a calculated item's formula for the field `field` (its source column name).
pub fn compile(field: &str, formula: &str) -> Result<Compiled, String> {
    let field = field.trim();
    let text = formula.trim();
    let text = text.strip_prefix('=').unwrap_or(text);
    if text.trim().is_empty() {
        return Err("The formula is empty.".into());
    }
    if text.chars().count() > MAX_FORMULA_LEN {
        return Err("The formula is too long.".into());
    }
    let chars: Vec<char> = text.chars().collect();
    let at = |i: usize| chars.get(i).copied();
    let mut out = String::with_capacity(text.len() + 16);
    let mut items: Vec<String> = Vec::new();
    let slot = |name: &str, items: &mut Vec<String>| -> String {
        let i = items.iter().position(|r| r.eq_ignore_ascii_case(name)).unwrap_or_else(|| {
            items.push(name.to_string());
            items.len() - 1
        });
        format!("{PREFIX}{i}")
    };
    let mut i = 0;
    while let Some(c) = at(i) {
        match c {
            c if matches!(c, '(' | ')' | '+' | '-' | '*' | '/' | '^') => {
                out.push(c);
                i += 1;
            }
            '\'' => {
                // A quoted caption (`='North America'+2023`), as a calculated field writes it.
                i += 1;
                let mut name = String::new();
                loop {
                    match at(i) {
                        None => return Err("The formula has an unterminated item name.".into()),
                        Some('\'') if at(i + 1) == Some('\'') => {
                            name.push('\'');
                            i += 2;
                        }
                        Some('\'') => {
                            i += 1;
                            break;
                        }
                        Some(x) => {
                            name.push(x);
                            i += 1;
                        }
                    }
                }
                out.push_str(&slot(name.trim(), &mut items));
            }
            c if c.is_ascii_digit() || (c == '.' && at(i + 1).is_some_and(|d| d.is_ascii_digit())) => {
                while at(i).is_some_and(|d| d.is_ascii_digit() || d == '.') {
                    out.push(at(i).unwrap_or('0'));
                    i += 1;
                }
                if matches!(at(i), Some('e' | 'E')) {
                    let sign = usize::from(matches!(at(i + 1), Some('+' | '-')));
                    if at(i + 1 + sign).is_some_and(|d| d.is_ascii_digit()) {
                        for _ in 0..=sign {
                            out.push(at(i).unwrap_or('0'));
                            i += 1;
                        }
                        while at(i).is_some_and(|d| d.is_ascii_digit()) {
                            out.push(at(i).unwrap_or('0'));
                            i += 1;
                        }
                    }
                }
            }
            _ => {
                // A caption: a bare name up to an operator, or `Field[item]`.
                let start = i;
                while at(i).is_some_and(|c| !matches!(c, '(' | ')' | '+' | '-' | '*' | '/' | '^' | '[' | ']')) {
                    i += 1;
                }
                let word: String = chars.get(start..i).map(|w| w.iter().collect()).unwrap_or_default();
                let word = word.trim().to_string();
                if at(i) == Some('[') {
                    i += 1;
                    let ns = i;
                    while at(i).is_some_and(|c| c != ']') {
                        i += 1;
                    }
                    if at(i) != Some(']') {
                        return Err("The formula has an unterminated item reference.".into());
                    }
                    let name: String = chars.get(ns..i).map(|w| w.iter().collect()).unwrap_or_default();
                    i += 1;
                    let name = name.trim();
                    if name.is_empty() {
                        return Err("The formula has an empty item reference.".into());
                    }
                    if !word.eq_ignore_ascii_case(field) {
                        return Err(format!("`{word}[{name}]` names another field; a calculated item can only use items of `{field}`."));
                    }
                    out.push_str(&slot(name, &mut items));
                } else if word.is_empty() {
                    // A character the grammar has no use for: let the parser reject it.
                    if let Some(c) = chars.get(start) {
                        out.push(*c);
                    }
                    i = start + 1;
                } else {
                    out.push_str(&slot(&word, &mut items));
                }
            }
        }
    }
    let body = gridcraft_formula::parse(&out).map_err(|e| format!("The formula is not valid: {e}"))?;
    let mut nodes = 0usize;
    body.walk(&mut |_| nodes += 1);
    if nodes > MAX_TERMS {
        return Err("The formula is too complex.".into());
    }
    let term = term_of(&body)?;
    Ok(Compiled { term, items })
}

fn term_of(e: &Expr) -> Result<Term, String> {
    match e {
        Expr::Number(n) => Ok(Term::Number(*n)),
        Expr::Name(n) => n
            .strip_prefix(PREFIX)
            .and_then(|k| k.parse::<usize>().ok())
            .map(Term::Item)
            .ok_or_else(|| "A calculated item's formula can only use items of its field.".to_string()),
        Expr::Unary(UnOp::Neg, e) => Ok(Term::Neg(Box::new(term_of(e)?))),
        Expr::Unary(UnOp::Plus, e) => term_of(e),
        Expr::Binary(op @ (BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Pow), a, b) => {
            Ok(Term::Binary(*op, Box::new(term_of(a)?), Box::new(term_of(b)?)))
        }
        Expr::Paren(e) => term_of(e),
        _ => Err("A calculated item's formula can only use item names, numbers, + - * / ^ and parentheses.".into()),
    }
}

/// Evaluates a term, asking `item` for each item's aggregate.
pub fn eval(term: &Term, item: &mut dyn FnMut(usize) -> Value) -> Value {
    eval_term(term, item, 0)
}

fn eval_term(t: &Term, item: &mut dyn FnMut(usize) -> Value, depth: u32) -> Value {
    if depth as usize > MAX_TERMS {
        return Value::Error(CellError::Num);
    }
    match t {
        Term::Number(n) => Value::number(*n),
        Term::Item(i) => item(*i),
        Term::Neg(e) => match eval_term(e, item, depth + 1) {
            Value::Number(n) => Value::number(-n),
            v @ Value::Error(_) => v,
            _ => Value::Error(CellError::Value),
        },
        Term::Binary(op, a, b) => {
            let a = eval_term(a, item, depth + 1);
            if let Value::Error(e) = a {
                return Value::Error(e);
            }
            let b = eval_term(b, item, depth + 1);
            if let Value::Error(e) = b {
                return Value::Error(e);
            }
            let (Value::Number(x), Value::Number(y)) = (a, b) else {
                return Value::Error(CellError::Value);
            };
            match op {
                BinOp::Add => Value::number(x + y),
                BinOp::Sub => Value::number(x - y),
                BinOp::Mul => Value::number(x * y),
                BinOp::Div if y == 0.0 => Value::Error(CellError::Div0),
                BinOp::Div => Value::number(x / y),
                BinOp::Pow => Value::number(x.powf(y)),
                // `term_of` admits no other operator.
                _ => Value::Error(CellError::Value),
            }
        }
    }
}

// ---------------------------------------------------------------- printing

fn op_prec(op: &BinOp) -> u8 {
    match op {
        BinOp::Add | BinOp::Sub => 1,
        BinOp::Mul | BinOp::Div => 2,
        BinOp::Pow => 3,
        _ => 0,
    }
}

fn op_text(op: &BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Pow => "^",
        _ => "?",
    }
}

/// A unary minus binds tighter than `^` (Excel's order: `=-2^2` is `4`).
const NEG_PREC: u8 = 4;

/// Writes a term, parenthesising a child whose precedence is below `parent`.
fn print_term(t: &Term, field: &str, dialog: bool, items: &[String], parent: u8, out: &mut String) {
    match t {
        Term::Number(n) => out.push_str(&formula_number(*n)),
        Term::Item(i) => {
            let name = items.get(*i).map(String::as_str).unwrap_or("");
            if dialog && bare_ok(name) {
                out.push_str(name);
            } else {
                out.push_str(field);
                out.push('[');
                out.push_str(name);
                out.push(']');
            }
        }
        Term::Neg(e) => {
            let need = NEG_PREC < parent;
            if need {
                out.push('(');
            }
            out.push('-');
            print_term(e, field, dialog, items, NEG_PREC + 1, out);
            if need {
                out.push(')');
            }
        }
        Term::Binary(op, a, b) => {
            let p = op_prec(op);
            let need = p < parent;
            if need {
                out.push('(');
            }
            // Left associative, except `^`: `a-(b-c)` and `(a^b)^c` keep their parentheses.
            let (pl, pr) = if *op == BinOp::Pow { (p + 1, p) } else { (p, p + 1) };
            print_term(a, field, dialog, items, pl, out);
            out.push_str(op_text(op));
            print_term(b, field, dialog, items, pr, out);
            if need {
                out.push(')');
            }
        }
    }
}

/// Whether a caption can be written bare in the dialog form and read back unchanged: an
/// identifier (`North`, `Q1`, `Sales_2024`) — anything else stays `Field[caption]`.
fn bare_ok(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else { return false };
    let word = |c: char| c.is_alphanumeric() || c == '_';
    (first.is_alphabetic() || first == '_')
        && chars.all(|c| word(c) || c == '.')
        && !name.eq_ignore_ascii_case("true")
        && !name.eq_ignore_ascii_case("false")
}

fn formula_number(n: f64) -> String {
    if !n.is_finite() {
        return "0".into();
    }
    if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", n as i64) } else { format!("{n}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(c: &Compiled) -> Vec<&str> {
        c.items().iter().map(String::as_str).collect()
    }

    #[test]
    fn dialog_and_stored_forms_are_one_formula() {
        let c = compile("Category", "=a+b").unwrap();
        assert_eq!(items(&c), vec!["a", "b"]);
        assert_eq!(c.dialog_form("Category"), "=a+b");
        assert_eq!(c.stored_form("Category"), "Category[a]+Category[b]");
        // Excel's own form reads back to the same thing.
        let d = compile("Category", "Category[a]+Category[b]").unwrap();
        assert_eq!(d, c);
        // A field-qualified reference to another field is refused, not silently dropped.
        assert!(compile("Category", "Region[a]+b").is_err());
    }

    #[test]
    fn captions_with_spaces_or_numbers_stay_bracketed() {
        let c = compile("Region", "='North America'+Region[2023]").unwrap();
        assert_eq!(items(&c), vec!["North America", "2023"]);
        assert_eq!(c.dialog_form("Region"), "=Region[North America]+Region[2023]");
        assert_eq!(c.stored_form("Region"), "Region[North America]+Region[2023]");
        assert!(compile("Region", "Region[North America]+Region[2023]").is_ok());
    }

    #[test]
    fn operators_keep_their_grouping() {
        for text in ["=(a+b)*c", "=a*(b+c)", "=a-b-c", "=a-(b-c)", "=a/b/c", "=(a/b)/c", "=-a^b", "=(-2^a)^b", "=a^b^c"] {
            let c = compile("F", text).unwrap();
            let again = compile("F", &c.dialog_form("F")).unwrap();
            assert_eq!(again, c, "{text}");
            let stored = compile("F", &c.stored_form("F")).unwrap();
            assert_eq!(stored, c, "{text}");
        }
    }

    #[test]
    fn evaluation_is_excels_arithmetic() {
        let c = compile("F", "=(a+b)*2-c/2").unwrap();
        let v = eval(c.term(), &mut |i| Value::number([10.0, 5.0, 4.0].get(i).copied().unwrap_or(0.0)));
        assert_eq!(v, Value::number(28.0));
        let c = compile("F", "=a/b").unwrap();
        assert_eq!(eval(c.term(), &mut |_| Value::number(0.0)), Value::Error(CellError::Div0));
        let c = compile("F", "=a+1").unwrap();
        assert_eq!(eval(c.term(), &mut |_| Value::Error(CellError::Ref)), Value::Error(CellError::Ref));
    }

    #[test]
    fn only_item_arithmetic_is_accepted() {
        assert!(compile("F", "=SUM(a,b)").is_err());
        assert!(compile("F", "=a*").is_err());
        assert!(compile("F", "=").is_err());
        assert!(compile("F", "=F[a").is_err());
        assert!(compile("F", "=F[]").is_err());
    }
}
