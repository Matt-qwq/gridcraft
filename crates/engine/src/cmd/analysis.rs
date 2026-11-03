//! Analysis ToolPak (Data › Analysis › Data Analysis): statistical tools that compute a report
//! from input ranges and write it as a static table (values, not formulas) to an output range or
//! a new worksheet.
//!
//! Every tool is one `tool` value of `data.analysisToolPak`. Distribution values (t, F) come
//! from the worksheet functions (`T.DIST.2T`, `F.DIST.RT`, `T.INV.2T`…), so the reports agree
//! with the formulas a user would write. Layouts follow Microsoft's public description of the
//! tools (row labels and section titles); exact spacing is GridCraft's reading of it.

use std::collections::{BTreeMap, HashMap};

use gridcraft_core::{CellError, CellRef, DateSystem, MAX_COLS, MAX_ROWS, RangeRef, Value};
use gridcraft_model::{Cell, NumFmt, Sheet};
use serde_json::{Value as Json, json};

use super::whatif::parse_range;
use super::*;

const CMD: &str = "data.analysisToolPak";
/// Most input cells a tool reads.
const MAX_INPUT_CELLS: u64 = 1_000_000;
/// Most cells a report may write.
const MAX_OUTPUT_CELLS: u64 = 4_000_000;
/// Most variables in a correlation or covariance matrix (work grows with its square).
const MAX_MATRIX_VARS: usize = 256;
/// Excel's limit on regression X columns.
const MAX_REGRESSORS: usize = 16;
/// Most histogram bins.
const MAX_BINS: usize = 100_000;

/// Part of the error a programmatic call gets when the output range already holds data; the
/// dialog recognises it and asks before running again with `overwrite: true`.
pub const OVERWRITE_ERROR: &str = "already holds data";

/// (id, label) of every tool, in the order of Excel's Analysis Tools list.
pub const TOOLS: &[(&str, &str)] = &[
    ("anovaSingle", "Anova: Single Factor"),
    ("anovaTwoFactor", "Anova: Two-Factor Without Replication"),
    ("anovaTwoFactorRep", "Anova: Two-Factor With Replication"),
    ("correlation", "Correlation"),
    ("covariance", "Covariance"),
    ("descriptive", "Descriptive Statistics"),
    ("exponentialSmoothing", "Exponential Smoothing"),
    ("fTest", "F-Test Two-Sample for Variances"),
    ("histogram", "Histogram"),
    ("movingAverage", "Moving Average"),
    ("rankPercentile", "Rank and Percentile"),
    ("regression", "Regression"),
    ("tTestPaired", "t-Test: Paired Two Sample for Means"),
    ("tTestEqual", "t-Test: Two-Sample Assuming Equal Variances"),
    ("tTestUnequal", "t-Test: Two-Sample Assuming Unequal Variances"),
    ("zTest", "z-Test: Two Sample for Means"),
];

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "data.analysisToolPak",
        "Data Analysis…",
        ["Data", "Analysis"],
        None,
        "{tool: anovaSingle|correlation|covariance|descriptive|exponentialSmoothing|fTest|histogram|movingAverage|rankPercentile|regression|tTestPaired|tTestEqual|tTestUnequal (or the tool's label), output?: \"E1\" (top-left of the report; default a new sheet), newSheet?: name, outputTo?: range|newSheet (picks between the two), overwrite?: bool (else non-empty output cells are refused), labels?: bool (first row/column holds names), groupedBy?: columns|rows, …tool params} — range tools take `range`; descriptive: summary? (default true), confidence?: 95, kthLargest?, kthSmallest?; histogram: bins?: range (default evenly spaced), pareto?, cumulative?; regression: inputY, inputX, constantZero?, confidence?: 95, residuals?, standardizedResiduals?, normalProbability?; movingAverage: interval? (3), standardErrors?; exponentialSmoothing: damping? (0.3), standardErrors?; t-tests and fTest: range1, range2, alpha? (0.05), hypothesizedDifference? (0); zTest: range1, range2, variance1, variance2 (the known variances), alpha? (0.05), hypothesizedDifference? (0); anovaSingle: alpha?; anovaTwoFactor: labels?, alpha? (two-factor without replication); anovaTwoFactorRep: range (factor A down the first column, factor B across the top, one block of rows per A level), rowsPerSample: r (the replicates per cell), labels?, alpha?. Writes static values. No `tool` → {tools} (the menu opens the dialog).",
        has_doc,
        run
    )]
}

// ---------------------------------------------------------------- parameters

fn num(p: &Json, key: &str) -> Option<f64> {
    match p.get(key)? {
        Json::Number(n) => n.as_f64(),
        Json::String(s) => s.trim().trim_end_matches('%').trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|v| v.is_finite())
}

fn flag(p: &Json, key: &str) -> bool {
    match p.get(key) {
        Some(Json::Bool(b)) => *b,
        Some(Json::String(s)) => s.eq_ignore_ascii_case("true") || s == "1",
        Some(Json::Number(n)) => n.as_f64().is_some_and(|v| v != 0.0),
        _ => false,
    }
}

fn text<'a>(p: &'a Json, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| str_param(p, k)).map(str::trim).filter(|t| !t.is_empty())
}

fn norm(s: &str) -> String {
    s.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase()
}

/// A tool id from its id or its label (case and punctuation ignored).
fn tool_id(t: &str) -> Option<&'static str> {
    let n = norm(t);
    TOOLS.iter().find(|(id, label)| norm(id) == n || norm(label) == n).map(|(id, _)| *id)
}

/// Confidence level as a percentage in (0, 100): `95`, `"95%"` or `0.95`.
fn confidence(p: &Json, key: &str) -> Result<Option<f64>> {
    match num(p, key) {
        None => Ok(None),
        Some(c) => {
            let c = if c > 0.0 && c < 1.0 { c * 100.0 } else { c };
            if c > 0.0 && c < 100.0 { Ok(Some(c)) } else { Err(bad(CMD, "the confidence level must be between 0 and 100")) }
        }
    }
}

fn alpha(p: &Json) -> Result<f64> {
    match num(p, "alpha") {
        None => Ok(0.05),
        Some(a) if a > 0.0 && a < 1.0 => Ok(a),
        Some(_) => Err(bad(CMD, "alpha must be between 0 and 1")),
    }
}

// ---------------------------------------------------------------- input

struct Var {
    name: String,
    vals: Vec<Value>,
}

fn non_numeric() -> EngineError {
    EngineError::Other("Input range contains non-numeric data.".into())
}

/// The cells of a range parameter, after checking its size.
fn input_range(s: &Session, p: &Json, keys: &[&str]) -> Result<(usize, RangeRef)> {
    let t = text(p, keys).ok_or_else(|| bad(CMD, format!("missing `{}` (the input range)", keys.first().copied().unwrap_or("range"))))?;
    let (si, r) = parse_range(&s.doc()?.wb, t, CMD)?;
    if r.count() > MAX_INPUT_CELLS {
        return Err(bad(CMD, format!("the input range is too large ({MAX_INPUT_CELLS} cells at most)")));
    }
    Ok((si, r))
}

/// Splits a range into variables (columns, or rows when `by_rows`); with `labels` the first
/// cell of each names it, else `default(i)` does.
fn variables(s: &Session, si: usize, r: RangeRef, by_rows: bool, labels: bool, default: &dyn Fn(usize) -> String) -> Result<Vec<Var>> {
    let d = s.doc()?;
    let sh = d.wb.sheet(si).ok_or(EngineError::NoDocument)?;
    let (outer, inner) = if by_rows { (r.height(), r.width()) } else { (r.width(), r.height()) };
    let mut out = Vec::with_capacity(outer as usize);
    for i in 0..outer {
        let cell = |j: u32| if by_rows { CellRef::new(r.start.row + i, r.start.col + j) } else { CellRef::new(r.start.row + j, r.start.col + i) };
        let mut vals: Vec<Value> = (0..inner).map(|j| sh.value(cell(j))).collect();
        let mut name = default(i as usize);
        if labels && !vals.is_empty() {
            let first = vals.remove(0);
            let t = first.display();
            if !t.trim().is_empty() {
                name = t;
            }
        }
        out.push(Var { name, vals });
    }
    Ok(out)
}

/// The numbers of a variable; blanks are skipped when `skip_blank`, anything else that isn't a
/// number is an error.
fn numbers(v: &[Value], skip_blank: bool) -> Result<Vec<f64>> {
    let mut out = Vec::with_capacity(v.len());
    for x in v {
        match x {
            Value::Number(n) => out.push(*n),
            Value::Empty if skip_blank => {}
            _ => return Err(non_numeric()),
        }
    }
    Ok(out)
}

fn by_rows(p: &Json) -> bool {
    str_param(p, "groupedBy").is_some_and(|g| g.to_ascii_lowercase().starts_with("row"))
}

/// One variable from a single row or column (`range1`/`range2` of the two-sample tests).
fn single(s: &Session, p: &Json, key: &str, fallback: &str) -> Result<Var> {
    let (si, r) = input_range(s, p, &[key])?;
    if r.width() > 1 && r.height() > 1 {
        return Err(EngineError::Other("Input range must be a single row or column.".into()));
    }
    let rows = r.width() > 1;
    let mut v = variables(s, si, r, rows, flag(p, "labels"), &|_| fallback.to_string())?;
    v.pop().ok_or_else(|| bad(CMD, "empty input range"))
}

// ---------------------------------------------------------------- statistics

fn mean(x: &[f64]) -> f64 {
    if x.is_empty() { f64::NAN } else { x.iter().sum::<f64>() / x.len() as f64 }
}

/// Sum of squared deviations from the mean.
fn ss(x: &[f64]) -> f64 {
    let m = mean(x);
    x.iter().map(|v| (v - m) * (v - m)).sum()
}

/// Sample variance (n − 1).
fn var_s(x: &[f64]) -> f64 {
    if x.len() < 2 { f64::NAN } else { ss(x) / (x.len() - 1) as f64 }
}

fn div0() -> Value {
    Value::Error(CellError::Div0)
}

/// `a / b`, `#DIV/0!` when `b` is zero or the inputs aren't finite.
fn ratio(a: f64, b: f64) -> Value {
    if b == 0.0 || !a.is_finite() || !b.is_finite() { div0() } else { Value::number(a / b) }
}

fn n_or_div0(x: f64) -> Value {
    if x.is_finite() { Value::number(x) } else { div0() }
}

/// Host services for calling worksheet functions directly.
struct FnHost;

impl gridcraft_functions::Ctx for FnHost {
    fn date_system(&self) -> DateSystem {
        DateSystem::D1900
    }
    fn now_serial(&self) -> f64 {
        0.0
    }
    fn random(&mut self) -> f64 {
        0.5
    }
}

/// Calls a worksheet function on numeric arguments (and `cumulative` flags as booleans).
fn call(name: &str, args: &[Value]) -> Value {
    let Some(spec) = gridcraft_functions::lookup(name) else { return Value::Error(CellError::Name) };
    if args.iter().any(|a| a.as_f64().is_some_and(|v| !v.is_finite())) {
        return Value::Error(CellError::Num);
    }
    let a: Vec<gridcraft_functions::Arg> = args.iter().cloned().map(gridcraft_functions::Arg::val).collect();
    gridcraft_functions::call(spec, &a, &mut FnHost)
}

fn callf(name: &str, args: &[f64]) -> Value {
    call(name, &args.iter().map(|v| Value::Number(*v)).collect::<Vec<_>>())
}

fn f64_of(v: &Value) -> f64 {
    v.as_f64().unwrap_or(f64::NAN)
}

// ---------------------------------------------------------------- report grid

/// A report: values by (row, col) relative to its top-left corner.
#[derive(Default)]
struct Grid {
    cells: BTreeMap<(u32, u32), Value>,
    /// Cells shown as percentages.
    pct: Vec<(u32, u32)>,
}

impl Grid {
    fn put(&mut self, r: u32, c: u32, v: Value) {
        if !v.is_empty() {
            self.cells.insert((r, c), v);
        }
    }
    fn txt(&mut self, r: u32, c: u32, t: impl AsRef<str>) {
        self.put(r, c, Value::text(t.as_ref()));
    }
    fn num(&mut self, r: u32, c: u32, x: f64) {
        self.put(r, c, Value::number(x));
    }
    /// Writes a row of labels starting at column `c`.
    fn labels(&mut self, r: u32, c: u32, items: &[&str]) {
        for (k, t) in items.iter().enumerate() {
            if !t.is_empty() {
                self.txt(r, c + k as u32, t);
            }
        }
    }
    /// (rows, cols) spanned.
    fn extent(&self) -> (u32, u32) {
        let rows = self.cells.keys().map(|(r, _)| r + 1).max().unwrap_or(0);
        let cols = self.cells.keys().map(|(_, c)| c + 1).max().unwrap_or(0);
        (rows, cols)
    }
}

fn idx(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(u32::MAX)
}

// ---------------------------------------------------------------- tools

fn descriptive(s: &Session, p: &Json) -> Result<Grid> {
    let (si, r) = input_range(s, p, &["range", "input"])?;
    let vars = variables(s, si, r, by_rows(p), flag(p, "labels"), &|i| format!("Column{}", i + 1))?;
    let summary = p.get("summary").map(|_| flag(p, "summary")).unwrap_or(true);
    let conf = confidence(p, "confidence")?;
    let kl = num(p, "kthLargest").filter(|k| *k >= 1.0).map(|k| k as usize);
    let ks = num(p, "kthSmallest").filter(|k| *k >= 1.0).map(|k| k as usize);
    if !summary && conf.is_none() && kl.is_none() && ks.is_none() {
        return Err(bad(CMD, "choose at least one of summary, confidence, kthLargest or kthSmallest"));
    }
    let mut g = Grid::default();
    for (i, v) in vars.iter().enumerate() {
        let x = numbers(&v.vals, true)?;
        let c = idx(i).saturating_mul(2);
        g.txt(0, c, &v.name);
        let mut row = 2u32;
        let mut line = |g: &mut Grid, label: &str, val: Value| {
            g.txt(row, c, label);
            g.put(row, c + 1, val);
            row += 1;
        };
        let n = x.len() as f64;
        let mut sorted = x.clone();
        sorted.sort_by(f64::total_cmp);
        if summary {
            let m = mean(&x);
            let sd = var_s(&x).sqrt();
            line(&mut g, "Mean", n_or_div0(m));
            line(&mut g, "Standard Error", if x.len() < 2 { div0() } else { n_or_div0(sd / n.sqrt()) });
            line(&mut g, "Median", median(&sorted));
            line(&mut g, "Mode", mode(&x));
            line(&mut g, "Standard Deviation", n_or_div0(sd));
            line(&mut g, "Sample Variance", n_or_div0(var_s(&x)));
            line(&mut g, "Kurtosis", kurtosis(&x));
            line(&mut g, "Skewness", skewness(&x));
            let (lo, hi) = (sorted.first().copied(), sorted.last().copied());
            line(&mut g, "Range", lo.zip(hi).map(|(a, b)| Value::number(b - a)).unwrap_or(Value::Number(0.0)));
            line(&mut g, "Minimum", lo.map(Value::number).unwrap_or(Value::Number(0.0)));
            line(&mut g, "Maximum", hi.map(Value::number).unwrap_or(Value::Number(0.0)));
            line(&mut g, "Sum", Value::number(x.iter().sum()));
            line(&mut g, "Count", Value::number(n));
        }
        if let Some(k) = kl {
            let v = if k <= sorted.len() { sorted.get(sorted.len() - k).copied().map(Value::number) } else { None };
            line(&mut g, &format!("Largest({k})"), v.unwrap_or(Value::Error(CellError::Num)));
        }
        if let Some(k) = ks {
            let v = sorted.get(k - 1).copied().map(Value::number);
            line(&mut g, &format!("Smallest({k})"), v.unwrap_or(Value::Error(CellError::Num)));
        }
        if let Some(cl) = conf {
            let sd = var_s(&x).sqrt();
            let val = if x.len() < 2 {
                div0()
            } else {
                let t = f64_of(&callf("T.INV.2T", &[1.0 - cl / 100.0, n - 1.0]));
                n_or_div0(t * sd / n.sqrt())
            };
            line(&mut g, &format!("Confidence Level({cl:.1}%)"), val);
        }
    }
    Ok(g)
}

fn median(sorted: &[f64]) -> Value {
    let n = sorted.len();
    if n == 0 {
        return Value::Error(CellError::Num);
    }
    let v = if n % 2 == 1 { sorted.get(n / 2).copied() } else { sorted.get(n / 2 - 1).zip(sorted.get(n / 2)).map(|(a, b)| (a + b) / 2.0) };
    v.map(Value::number).unwrap_or(Value::Error(CellError::Num))
}

/// The most frequent value (the first one to reach the top count), `#N/A` when no value repeats.
fn mode(x: &[f64]) -> Value {
    let mut counts: HashMap<u64, usize> = HashMap::new();
    for v in x {
        *counts.entry(v.to_bits()).or_default() += 1;
    }
    let top = counts.values().copied().max().unwrap_or(0);
    if top < 2 {
        return Value::Error(CellError::NA);
    }
    x.iter().find(|v| counts.get(&v.to_bits()).copied() == Some(top)).map(|v| Value::number(*v)).unwrap_or(Value::Error(CellError::NA))
}

/// Sample excess kurtosis (Excel's KURT).
fn kurtosis(x: &[f64]) -> Value {
    let n = x.len() as f64;
    let sd = var_s(x).sqrt();
    if x.len() < 4 || sd.is_nan() || sd <= 0.0 {
        return div0();
    }
    let m = mean(x);
    let s4: f64 = x.iter().map(|v| ((v - m) / sd).powi(4)).sum();
    Value::number(n * (n + 1.0) / ((n - 1.0) * (n - 2.0) * (n - 3.0)) * s4 - 3.0 * (n - 1.0) * (n - 1.0) / ((n - 2.0) * (n - 3.0)))
}

/// Sample skewness (Excel's SKEW).
fn skewness(x: &[f64]) -> Value {
    let n = x.len() as f64;
    let sd = var_s(x).sqrt();
    if x.len() < 3 || sd.is_nan() || sd <= 0.0 {
        return div0();
    }
    let m = mean(x);
    let s3: f64 = x.iter().map(|v| ((v - m) / sd).powi(3)).sum();
    Value::number(n / ((n - 1.0) * (n - 2.0)) * s3)
}

fn histogram(s: &Session, p: &Json) -> Result<Grid> {
    let (si, r) = input_range(s, p, &["range", "input"])?;
    let labels = flag(p, "labels");
    let d = s.doc()?;
    let sh = d.wb.sheet(si).ok_or(EngineError::NoDocument)?;
    let mut cells: Vec<Value> = r.iter().map(|c| sh.value(c)).collect();
    if labels && !cells.is_empty() {
        cells.remove(0);
    }
    let data = numbers(&cells, true)?;
    let mut bins: Vec<f64> = match text(p, &["bins", "binRange"]) {
        Some(_) => {
            let (bsi, br) = input_range(s, p, &["bins", "binRange"])?;
            let bsh = d.wb.sheet(bsi).ok_or(EngineError::NoDocument)?;
            let mut b: Vec<Value> = br.iter().map(|c| bsh.value(c)).collect();
            if labels && !b.is_empty() {
                b.remove(0);
            }
            numbers(&b, true)?
        }
        None => {
            // Evenly spaced bins from the minimum to the maximum, √n intervals.
            let lo = data.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = data.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            if !lo.is_finite() || !hi.is_finite() {
                return Err(EngineError::Other("The input range holds no numbers.".into()));
            }
            let k = ((data.len() as f64).sqrt().floor() as usize).clamp(1, 1000);
            if hi == lo { vec![lo] } else { (0..=k).map(|i| lo + (hi - lo) * i as f64 / k as f64).collect() }
        }
    };
    if bins.is_empty() {
        return Err(EngineError::Other("The bin range holds no numbers.".into()));
    }
    if bins.len() > MAX_BINS {
        return Err(bad(CMD, format!("too many bins ({MAX_BINS} at most)")));
    }
    bins.sort_by(f64::total_cmp);
    let mut freq = vec![0u64; bins.len() + 1];
    for x in &data {
        let i = bins.partition_point(|b| b < x);
        if let Some(f) = freq.get_mut(i) {
            *f += 1;
        }
    }
    let rows: Vec<(Value, u64)> = bins.iter().map(|b| Value::number(*b)).chain(std::iter::once(Value::text("More"))).zip(freq).collect();
    let total = data.len() as f64;
    let cumulative = flag(p, "cumulative");
    let mut g = Grid::default();
    let write = |g: &mut Grid, c0: u32, rows: &[(Value, u64)]| {
        g.labels(0, c0, &["Bin", "Frequency"]);
        if cumulative {
            g.txt(0, c0 + 2, "Cumulative %");
        }
        let mut acc = 0u64;
        for (i, (b, f)) in rows.iter().enumerate() {
            let r = idx(i) + 1;
            g.put(r, c0, b.clone());
            g.num(r, c0 + 1, *f as f64);
            if cumulative {
                acc += f;
                g.put(r, c0 + 2, ratio(acc as f64, total));
                g.pct.push((r, c0 + 2));
            }
        }
    };
    write(&mut g, 0, &rows);
    if flag(p, "pareto") {
        let mut sorted = rows.clone();
        sorted.sort_by_key(|r| std::cmp::Reverse(r.1));
        write(&mut g, if cumulative { 3 } else { 2 }, &sorted);
    }
    Ok(g)
}

/// Variables of a range whose columns (rows) must all be numbers, for the matrix tools.
fn matrix_vars(s: &Session, p: &Json) -> Result<Vec<(String, Vec<f64>)>> {
    let (si, r) = input_range(s, p, &["range", "input"])?;
    let vars = variables(s, si, r, by_rows(p), flag(p, "labels"), &|i| format!("Column {}", i + 1))?;
    if vars.len() > MAX_MATRIX_VARS {
        return Err(bad(CMD, format!("too many variables ({MAX_MATRIX_VARS} at most)")));
    }
    vars.into_iter().map(|v| Ok((v.name, numbers(&v.vals, false)?))).collect()
}

fn matrix(s: &Session, p: &Json, f: impl Fn(&[f64], &[f64]) -> Value, diag: impl Fn(&[f64]) -> Value) -> Result<Grid> {
    let vars = matrix_vars(s, p)?;
    let mut g = Grid::default();
    for (i, (name, x)) in vars.iter().enumerate() {
        let k = idx(i) + 1;
        g.txt(0, k, name);
        g.txt(k, 0, name);
        for (j, (_, y)) in vars.iter().enumerate().take(i + 1) {
            g.put(k, idx(j) + 1, if i == j { diag(x) } else { f(x, y) });
        }
    }
    Ok(g)
}

/// Sum of cross products of deviations.
fn sxy(x: &[f64], y: &[f64]) -> f64 {
    let (mx, my) = (mean(x), mean(y));
    x.iter().zip(y).map(|(a, b)| (a - mx) * (b - my)).sum()
}

fn correlation(x: &[f64], y: &[f64]) -> Value {
    let d = (ss(x) * ss(y)).sqrt();
    if x.is_empty() || d.is_nan() || d <= 0.0 { div0() } else { Value::number(sxy(x, y) / d) }
}

fn covariance_p(x: &[f64], y: &[f64]) -> Value {
    if x.is_empty() { div0() } else { Value::number(sxy(x, y) / x.len() as f64) }
}

// ---------------------------------------------------------------- regression

/// Inverts a small symmetric matrix by Gauss–Jordan elimination with partial pivoting; `None`
/// when it is singular (collinear inputs).
fn invert(mut a: Vec<Vec<f64>>) -> Option<Vec<Vec<f64>>> {
    let n = a.len();
    let scale = (0..n).filter_map(|i| a.get(i).and_then(|r| r.get(i))).fold(0.0f64, |m, v| m.max(v.abs()));
    let mut inv: Vec<Vec<f64>> = (0..n).map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect()).collect();
    for col in 0..n {
        let piv = (col..n).max_by(|&x, &y| {
            let ax = a.get(x).and_then(|r| r.get(col)).map_or(0.0, |v| v.abs());
            let ay = a.get(y).and_then(|r| r.get(col)).map_or(0.0, |v| v.abs());
            ax.total_cmp(&ay)
        })?;
        let pv = *a.get(piv)?.get(col)?;
        if pv.is_nan() || pv.abs() <= 1e-12 * scale.max(f64::MIN_POSITIVE) {
            return None;
        }
        a.swap(col, piv);
        inv.swap(col, piv);
        for j in 0..n {
            *a.get_mut(col)?.get_mut(j)? /= pv;
            *inv.get_mut(col)?.get_mut(j)? /= pv;
        }
        let prow = a.get(col)?.clone();
        let pinv = inv.get(col)?.clone();
        for i in 0..n {
            if i == col {
                continue;
            }
            let f = *a.get(i)?.get(col)?;
            if f == 0.0 {
                continue;
            }
            for j in 0..n {
                *a.get_mut(i)?.get_mut(j)? -= f * prow.get(j)?;
                *inv.get_mut(i)?.get_mut(j)? -= f * pinv.get(j)?;
            }
        }
    }
    Some(inv)
}

struct Fit {
    /// Intercept first, then one coefficient per X column.
    coef: Vec<f64>,
    /// Standard errors in the same order (NaN for a forced-zero intercept).
    se: Vec<f64>,
    predicted: Vec<f64>,
    sse: f64,
    ssr: f64,
    dfr: f64,
    dfe: f64,
    /// Total degrees of freedom (n − 1, or n through the origin).
    dft: f64,
}

/// Ordinary least squares of `y` on the columns `xs`, with an intercept unless `zero`.
fn ols(y: &[f64], xs: &[Vec<f64>], zero: bool) -> Option<Fit> {
    let n = y.len();
    let k = xs.len();
    let nf = n as f64;
    let (ym, xm): (f64, Vec<f64>) = if zero { (0.0, vec![0.0; k]) } else { (mean(y), xs.iter().map(|x| mean(x)).collect()) };
    let dev = |j: usize, i: usize| xs.get(j).and_then(|x| x.get(i)).copied().unwrap_or(0.0) - xm.get(j).copied().unwrap_or(0.0);
    let mut sxx = vec![vec![0.0; k]; k];
    let mut sxy = vec![0.0; k];
    for i in 0..n {
        let yi = y.get(i).copied().unwrap_or(0.0) - ym;
        for a in 0..k {
            let da = dev(a, i);
            if let Some(v) = sxy.get_mut(a) {
                *v += da * yi;
            }
            for b in 0..k {
                if let Some(v) = sxx.get_mut(a).and_then(|r| r.get_mut(b)) {
                    *v += da * dev(b, i);
                }
            }
        }
    }
    let inv = invert(sxx)?;
    let b: Vec<f64> = (0..k)
        .map(|a| (0..k).map(|c| inv.get(a).and_then(|r| r.get(c)).copied().unwrap_or(0.0) * sxy.get(c).copied().unwrap_or(0.0)).sum())
        .collect();
    let b0 = if zero { 0.0 } else { ym - b.iter().zip(&xm).map(|(bj, mj)| bj * mj).sum::<f64>() };
    let predicted: Vec<f64> = (0..n)
        .map(|i| b0 + (0..k).map(|j| b.get(j).copied().unwrap_or(0.0) * xs.get(j).and_then(|x| x.get(i)).copied().unwrap_or(0.0)).sum::<f64>())
        .collect();
    let sse: f64 = y.iter().zip(&predicted).map(|(a, p)| (a - p) * (a - p)).sum();
    let ssr: f64 = predicted.iter().map(|p| (p - ym) * (p - ym)).sum();
    let dfr = k as f64;
    let dfe = if zero { nf - dfr } else { nf - dfr - 1.0 };
    let dft = if zero { nf } else { nf - 1.0 };
    let s2 = sse / dfe;
    let mut se = Vec::with_capacity(k + 1);
    if zero {
        se.push(f64::NAN);
    } else {
        let q: f64 = (0..k)
            .map(|a| {
                (0..k)
                    .map(|c| {
                        xm.get(a).copied().unwrap_or(0.0)
                            * inv.get(a).and_then(|r| r.get(c)).copied().unwrap_or(0.0)
                            * xm.get(c).copied().unwrap_or(0.0)
                    })
                    .sum::<f64>()
            })
            .sum();
        se.push((s2 * (1.0 / nf + q)).sqrt());
    }
    for a in 0..k {
        se.push((s2 * inv.get(a).and_then(|r| r.get(a)).copied().unwrap_or(f64::NAN)).sqrt());
    }
    let mut coef = vec![b0];
    coef.extend(b);
    Some(Fit { coef, se, predicted, sse, ssr, dfr, dfe, dft })
}

fn regression(s: &Session, p: &Json) -> Result<Grid> {
    let labels = flag(p, "labels");
    let (ysi, yr) = input_range(s, p, &["inputY", "y"])?;
    let (xsi, xr) = input_range(s, p, &["inputX", "x"])?;
    if yr.width() != 1 {
        return Err(EngineError::Other("Input Y range must be a single column.".into()));
    }
    if xr.height() != yr.height() {
        return Err(EngineError::Other("Input Y range and input X range must have the same number of rows.".into()));
    }
    if xr.width() as usize > MAX_REGRESSORS {
        return Err(EngineError::Other(format!("Input X range must have {MAX_REGRESSORS} columns or fewer.")));
    }
    let yv = variables(s, ysi, yr, false, labels, &|_| "Y".into())?;
    let xv = variables(s, xsi, xr, false, labels, &|i| format!("X Variable {}", i + 1))?;
    let yvar = yv.first().ok_or_else(|| bad(CMD, "empty input Y range"))?;
    let y = numbers(&yvar.vals, false)?;
    let xs: Vec<Vec<f64>> = xv.iter().map(|v| numbers(&v.vals, false)).collect::<Result<_>>()?;
    let zero = flag(p, "constantZero");
    let k = xs.len();
    let n = y.len();
    let params = if zero { k } else { k + 1 };
    if n <= params {
        return Err(EngineError::Other("Too few observations for this many X variables.".into()));
    }
    let fit = ols(&y, &xs, zero).ok_or_else(|| EngineError::Other("The X variables are collinear; the regression can't be computed.".into()))?;
    let conf = confidence(p, "confidence")?.unwrap_or(95.0);
    let sst = fit.ssr + fit.sse;
    let r2 = if sst > 0.0 { fit.ssr / sst } else { f64::NAN };
    let mut g = Grid::default();
    g.txt(0, 0, "SUMMARY OUTPUT");
    g.txt(2, 0, "Regression Statistics");
    let stats: [(&str, Value); 5] = [
        ("Multiple R", n_or_div0(r2.sqrt())),
        ("R Square", n_or_div0(r2)),
        ("Adjusted R Square", n_or_div0(1.0 - (fit.sse / fit.dfe) / (sst / fit.dft))),
        ("Standard Error", n_or_div0((fit.sse / fit.dfe).sqrt())),
        ("Observations", Value::number(n as f64)),
    ];
    for (i, (label, v)) in stats.into_iter().enumerate() {
        g.txt(3 + idx(i), 0, label);
        g.put(3 + idx(i), 1, v);
    }
    g.txt(9, 0, "ANOVA");
    g.labels(10, 1, &["df", "SS", "MS", "F", "Significance F"]);
    let (msr, mse) = (fit.ssr / fit.dfr, fit.sse / fit.dfe);
    let f = msr / mse;
    g.txt(11, 0, "Regression");
    g.num(11, 1, fit.dfr);
    g.num(11, 2, fit.ssr);
    g.num(11, 3, msr);
    g.put(11, 4, n_or_div0(f));
    g.put(11, 5, if f.is_finite() { callf("F.DIST.RT", &[f, fit.dfr, fit.dfe]) } else { div0() });
    g.txt(12, 0, "Residual");
    g.num(12, 1, fit.dfe);
    g.num(12, 2, fit.sse);
    g.num(12, 3, mse);
    g.txt(13, 0, "Total");
    g.num(13, 1, fit.dft);
    g.num(13, 2, sst);
    let lo2 = format!("Lower {conf:.1}%");
    let hi2 = format!("Upper {conf:.1}%");
    g.labels(15, 1, &["Coefficients", "Standard Error", "t Stat", "P-value", "Lower 95%", "Upper 95%", &lo2, &hi2]);
    let t95 = f64_of(&callf("T.INV.2T", &[0.05, fit.dfe]));
    let tc = f64_of(&callf("T.INV.2T", &[1.0 - conf / 100.0, fit.dfe]));
    for (j, (b, se)) in fit.coef.iter().zip(&fit.se).enumerate() {
        let row = 16 + idx(j);
        g.txt(row, 0, if j == 0 { "Intercept" } else { xv.get(j - 1).map_or("", |v| v.name.as_str()) });
        g.num(row, 1, *b);
        if j == 0 && zero {
            for c in 2..=8 {
                g.put(row, c, Value::Error(CellError::NA));
            }
            continue;
        }
        let t = b / se;
        g.put(row, 2, n_or_div0(*se));
        g.put(row, 3, n_or_div0(t));
        g.put(row, 4, if t.is_finite() { callf("T.DIST.2T", &[t.abs(), fit.dfe]) } else { div0() });
        g.num(row, 5, b - t95 * se);
        g.num(row, 6, b + t95 * se);
        g.num(row, 7, b - tc * se);
        g.num(row, 8, b + tc * se);
    }
    let mut row = 16 + idx(fit.coef.len()) + 3;
    let residuals = flag(p, "residuals");
    let standardized = flag(p, "standardizedResiduals");
    if residuals || standardized {
        g.txt(row, 0, "RESIDUAL OUTPUT");
        let pred = format!("Predicted {}", yvar.name);
        let mut head = vec!["Observation", pred.as_str(), "Residuals"];
        if standardized {
            head.push("Standard Residuals");
        }
        g.labels(row + 2, 0, &head);
        // Residuals scaled by their standard deviation (n − 1).
        let sd = (fit.sse / (n as f64 - 1.0)).sqrt();
        for (i, (yi, pi)) in y.iter().zip(&fit.predicted).enumerate() {
            let r = row + 3 + idx(i);
            g.num(r, 0, (i + 1) as f64);
            g.num(r, 1, *pi);
            g.num(r, 2, yi - pi);
            if standardized {
                g.put(r, 3, ratio(yi - pi, sd));
            }
        }
    }
    if flag(p, "normalProbability") {
        let c = if residuals || standardized { if standardized { 5 } else { 4 } } else { 0 };
        if !(residuals || standardized) {
            row = 16 + idx(fit.coef.len()) + 3;
        }
        g.txt(row, c, "PROBABILITY OUTPUT");
        g.labels(row + 2, c, &["Percentile", &yvar.name]);
        let mut sorted = y.clone();
        sorted.sort_by(f64::total_cmp);
        for (i, v) in sorted.iter().enumerate() {
            let r = row + 3 + idx(i);
            g.num(r, c, 100.0 * (i as f64 + 0.5) / n as f64);
            g.num(r, c + 1, *v);
        }
    }
    Ok(g)
}

// ---------------------------------------------------------------- smoothing

fn series(s: &Session, p: &Json) -> Result<Vec<f64>> {
    let (si, r) = input_range(s, p, &["range", "input"])?;
    if r.width() > 1 && r.height() > 1 {
        return Err(EngineError::Other("Input range must be a single row or column.".into()));
    }
    let v = variables(s, si, r, r.width() > 1, flag(p, "labels"), &|_| String::new())?;
    numbers(&v.first().map(|v| v.vals.clone()).unwrap_or_default(), false)
}

/// Writes the forecasts (and with `standardErrors` the standard errors over `se_window`
/// points) down two columns.
fn forecast_grid(x: &[f64], f: &[Option<f64>], se_window: usize, p: &Json) -> Grid {
    let mut g = Grid::default();
    let na = || Value::Error(CellError::NA);
    for (i, fi) in f.iter().enumerate() {
        g.put(idx(i), 0, fi.map(Value::number).unwrap_or_else(na));
    }
    if flag(p, "standardErrors") {
        // A running sum of squared residuals over the last `se_window` points: O(n), not
        // O(n·window). A point without a forecast restarts it, so every window touching it is #N/A.
        let sq = |j: usize| Some((x.get(j)? - (*f.get(j)?)?).powi(2));
        let (mut acc, mut run) = (0.0, 0usize);
        for i in 0..f.len() {
            match sq(i) {
                Some(t) => {
                    acc += t;
                    run += 1;
                }
                None => (acc, run) = (0.0, 0),
            }
            if run > se_window {
                acc -= i.checked_sub(se_window).and_then(sq).unwrap_or(0.0);
                run = se_window;
            }
            // Rounding in the subtraction can leave a tiny negative sum.
            let v = (run == se_window).then(|| (acc.max(0.0) / se_window as f64).sqrt());
            g.put(idx(i), 1, v.map(Value::number).unwrap_or_else(na));
        }
    }
    g
}

fn moving_average(s: &Session, p: &Json) -> Result<Grid> {
    let x = series(s, p)?;
    let interval = num(p, "interval").unwrap_or(3.0);
    if interval < 1.0 || interval.fract() != 0.0 || interval > x.len() as f64 {
        return Err(EngineError::Other("The interval must be a whole number from 1 to the number of data points.".into()));
    }
    let k = interval as usize;
    let mut f = Vec::with_capacity(x.len());
    let mut acc = 0.0;
    for (i, v) in x.iter().enumerate() {
        acc += v;
        if let Some(old) = i.checked_sub(k).and_then(|j| x.get(j)) {
            acc -= old;
        }
        f.push(if i + 1 >= k { Some(acc / k as f64) } else { None });
    }
    Ok(forecast_grid(&x, &f, k, p))
}

fn exponential_smoothing(s: &Session, p: &Json) -> Result<Grid> {
    let x = series(s, p)?;
    let damp = num(p, "damping").unwrap_or(0.3);
    if !(0.0..=1.0).contains(&damp) {
        return Err(EngineError::Other("The damping factor must be between 0 and 1.".into()));
    }
    // F(1) = A(0); F(t) = (1 − damping)·A(t−1) + damping·F(t−1).
    let mut f: Vec<Option<f64>> = Vec::with_capacity(x.len());
    for i in 0..x.len() {
        let v = match i {
            0 => None,
            1 => x.first().copied(),
            _ => x.get(i - 1).zip(f.get(i - 1).copied().flatten()).map(|(a, prev)| (1.0 - damp) * a + damp * prev),
        };
        f.push(v);
    }
    Ok(forecast_grid(&x, &f, 3, p))
}

// ---------------------------------------------------------------- rank and percentile

fn rank_percentile(s: &Session, p: &Json) -> Result<Grid> {
    let (si, r) = input_range(s, p, &["range", "input"])?;
    let vars = variables(s, si, r, by_rows(p), flag(p, "labels"), &|i| format!("Column{}", i + 1))?;
    let mut g = Grid::default();
    for (vi, v) in vars.iter().enumerate() {
        let c = idx(vi).saturating_mul(4);
        g.labels(0, c, &["Point", &v.name, "Rank", "Percent"]);
        let mut pts: Vec<(usize, f64)> = Vec::new();
        for (i, val) in v.vals.iter().enumerate() {
            match val {
                Value::Number(n) => pts.push((i + 1, *n)),
                Value::Empty => {}
                _ => return Err(non_numeric()),
            }
        }
        let mut asc: Vec<f64> = pts.iter().map(|p| p.1).collect();
        asc.sort_by(f64::total_cmp);
        let n = asc.len();
        pts.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        for (k, (point, x)) in pts.iter().enumerate() {
            let row = idx(k) + 1;
            let below = asc.partition_point(|v| v < x);
            let above = n - asc.partition_point(|v| v <= x);
            g.num(row, c, *point as f64);
            g.num(row, c + 1, *x);
            g.num(row, c + 2, (above + 1) as f64);
            // PERCENTRANK.INC: share of values below, truncated to three digits.
            let pr = if n <= 1 { 1.0 } else { (below as f64 / (n - 1) as f64 * 1000.0 + 1e-9).floor() / 1000.0 };
            g.num(row, c + 3, pr);
            g.pct.push((row, c + 3));
        }
    }
    Ok(g)
}

// ---------------------------------------------------------------- tests of means and variances

fn two_samples(s: &Session, p: &Json, skip_blank: bool) -> Result<(Var, Var, Vec<f64>, Vec<f64>)> {
    let a = single(s, p, "range1", "Variable 1")?;
    let b = single(s, p, "range2", "Variable 2")?;
    let x = numbers(&a.vals, skip_blank)?;
    let y = numbers(&b.vals, skip_blank)?;
    if x.len() < 2 || y.len() < 2 {
        return Err(EngineError::Other("Each input range needs at least two numbers.".into()));
    }
    Ok((a, b, x, y))
}

/// Title, blank row and the "Variable 1 / Variable 2" header; returns the first data row.
fn test_head(g: &mut Grid, title: &str, a: &str, b: &str) -> u32 {
    g.txt(0, 0, title);
    g.txt(2, 1, a);
    g.txt(2, 2, b);
    3
}

/// The t statistic rows shared by the three t-tests.
fn t_rows(g: &mut Grid, mut row: u32, t: f64, df: f64, alpha: f64) {
    let ta = t.abs();
    let lines: [(&str, Value); 5] = [
        ("t Stat", n_or_div0(t)),
        ("P(T<=t) one-tail", if t.is_finite() { callf("T.DIST.RT", &[ta, df]) } else { div0() }),
        ("t Critical one-tail", callf("T.INV", &[1.0 - alpha, df])),
        ("P(T<=t) two-tail", if t.is_finite() { callf("T.DIST.2T", &[ta, df]) } else { div0() }),
        ("t Critical two-tail", callf("T.INV.2T", &[alpha, df])),
    ];
    for (label, v) in lines {
        g.txt(row, 0, label);
        g.put(row, 1, v);
        row += 1;
    }
}

fn pair_row(g: &mut Grid, row: u32, label: &str, a: Value, b: Value) {
    g.txt(row, 0, label);
    g.put(row, 1, a);
    g.put(row, 2, b);
}

fn t_test(s: &Session, p: &Json, kind: &str) -> Result<Grid> {
    let alpha = alpha(p)?;
    let hmd = num(p, "hypothesizedDifference").or_else(|| num(p, "difference")).unwrap_or(0.0);
    let paired = kind == "tTestPaired";
    let (a, b, x, y) = two_samples(s, p, !paired)?;
    let (n1, n2) = (x.len() as f64, y.len() as f64);
    let (m1, m2, v1, v2) = (mean(&x), mean(&y), var_s(&x), var_s(&y));
    let mut g = Grid::default();
    let title = match kind {
        "tTestPaired" => "t-Test: Paired Two Sample for Means",
        "tTestEqual" => "t-Test: Two-Sample Assuming Equal Variances",
        _ => "t-Test: Two-Sample Assuming Unequal Variances",
    };
    let mut row = test_head(&mut g, title, &a.name, &b.name);
    pair_row(&mut g, row, "Mean", Value::number(m1), Value::number(m2));
    pair_row(&mut g, row + 1, "Variance", Value::number(v1), Value::number(v2));
    pair_row(&mut g, row + 2, "Observations", Value::number(n1), Value::number(n2));
    row += 3;
    let (t, df) = match kind {
        "tTestPaired" => {
            if x.len() != y.len() {
                return Err(EngineError::Other("Input ranges must have the same number of values.".into()));
            }
            pair_row(&mut g, row, "Pearson Correlation", correlation(&x, &y), Value::Empty);
            row += 1;
            let dif: Vec<f64> = x.iter().zip(&y).map(|(a, b)| a - b).collect();
            let sd = var_s(&dif).sqrt();
            ((mean(&dif) - hmd) / (sd / n1.sqrt()), n1 - 1.0)
        }
        "tTestEqual" => {
            let pooled = ((n1 - 1.0) * v1 + (n2 - 1.0) * v2) / (n1 + n2 - 2.0);
            pair_row(&mut g, row, "Pooled Variance", Value::number(pooled), Value::Empty);
            row += 1;
            ((m1 - m2 - hmd) / (pooled * (1.0 / n1 + 1.0 / n2)).sqrt(), n1 + n2 - 2.0)
        }
        _ => {
            let (q1, q2) = (v1 / n1, v2 / n2);
            // Welch–Satterthwaite degrees of freedom, rounded to a whole number.
            let df = ((q1 + q2).powi(2) / (q1 * q1 / (n1 - 1.0) + q2 * q2 / (n2 - 1.0))).round();
            ((m1 - m2 - hmd) / (q1 + q2).sqrt(), df)
        }
    };
    pair_row(&mut g, row, "Hypothesized Mean Difference", Value::number(hmd), Value::Empty);
    pair_row(&mut g, row + 1, "df", n_or_div0(df), Value::Empty);
    t_rows(&mut g, row + 2, t, df, alpha);
    Ok(g)
}

fn f_test(s: &Session, p: &Json) -> Result<Grid> {
    let alpha = alpha(p)?;
    let (a, b, x, y) = two_samples(s, p, true)?;
    let (v1, v2) = (var_s(&x), var_s(&y));
    let (d1, d2) = (x.len() as f64 - 1.0, y.len() as f64 - 1.0);
    let mut g = Grid::default();
    let row = test_head(&mut g, "F-Test Two-Sample for Variances", &a.name, &b.name);
    pair_row(&mut g, row, "Mean", Value::number(mean(&x)), Value::number(mean(&y)));
    pair_row(&mut g, row + 1, "Variance", Value::number(v1), Value::number(v2));
    pair_row(&mut g, row + 2, "Observations", Value::number(d1 + 1.0), Value::number(d2 + 1.0));
    pair_row(&mut g, row + 3, "df", Value::number(d1), Value::number(d2));
    let f = v1 / v2;
    pair_row(&mut g, row + 4, "F", n_or_div0(f), Value::Empty);
    // The tail on the side of F: left when F < 1, right otherwise.
    let (pv, crit) = if !f.is_finite() {
        (div0(), div0())
    } else if f < 1.0 {
        (call("F.DIST", &[Value::Number(f), Value::Number(d1), Value::Number(d2), Value::Bool(true)]), callf("F.INV", &[alpha, d1, d2]))
    } else {
        (callf("F.DIST.RT", &[f, d1, d2]), callf("F.INV.RT", &[alpha, d1, d2]))
    };
    pair_row(&mut g, row + 5, "P(F<=f) one-tail", pv, Value::Empty);
    pair_row(&mut g, row + 6, "F Critical one-tail", crit, Value::Empty);
    Ok(g)
}

/// The known variance a z-Test takes from its parameters (`variance1`/`variance2`, or the
/// spelled-out `knownVariance1`/`knownVariance2`). A variance of exactly zero would divide by
/// zero; Excel refuses it too.
fn known_variance(p: &Json, which: usize) -> Result<f64> {
    let key = if which == 1 { "variance1" } else { "variance2" };
    let alt = if which == 1 { "knownVariance1" } else { "knownVariance2" };
    let v = num(p, key).or_else(|| num(p, alt)).ok_or_else(|| bad(CMD, format!("z-Test needs `{key}`, the known variance of variable {which}")))?;
    if v <= 0.0 || !v.is_finite() {
        return Err(bad(CMD, format!("`{key}` must be a number past zero")));
    }
    Ok(v)
}

fn z_test(s: &Session, p: &Json) -> Result<Grid> {
    let alpha = alpha(p)?;
    let hmd = num(p, "hypothesizedDifference").or_else(|| num(p, "difference")).unwrap_or(0.0);
    let v1 = known_variance(p, 1)?;
    let v2 = known_variance(p, 2)?;
    let (a, b, x, y) = two_samples(s, p, true)?;
    let (n1, n2) = (x.len() as f64, y.len() as f64);
    let (m1, m2) = (mean(&x), mean(&y));
    let mut g = Grid::default();
    let mut row = test_head(&mut g, "z-Test: Two Sample for Means", &a.name, &b.name);
    pair_row(&mut g, row, "Mean", Value::number(m1), Value::number(m2));
    pair_row(&mut g, row + 1, "Known Variance", Value::number(v1), Value::number(v2));
    pair_row(&mut g, row + 2, "Observations", Value::number(n1), Value::number(n2));
    row += 3;
    pair_row(&mut g, row, "Hypothesized Mean Difference", Value::number(hmd), Value::Empty);
    // z = (mean1 - mean2 - difference) / sqrt(var1/n1 + var2/n2), as Excel computes it.
    let se = (v1 / n1 + v2 / n2).sqrt();
    let z = (m1 - m2 - hmd) / se;
    let za = z.abs();
    // The one-tail p is 1 - Phi(|z|), so it never exceeds 0.5; the two-tail p is twice it.
    let p1 = call("NORM.S.DIST", &[Value::Number(-za), Value::Bool(true)]);
    let p2 = match p1 {
        Value::Number(p) => Value::number(2.0 * p),
        Value::Error(e) => Value::Error(e),
        _ => div0(),
    };
    let lines: [(&str, Value); 5] = [
        ("z", n_or_div0(z)),
        ("P(Z<=z) one-tail", p1),
        ("z Critical one-tail", callf("NORM.S.INV", &[1.0 - alpha])),
        ("P(Z<=z) two-tail", p2),
        ("z Critical two-tail", callf("NORM.S.INV", &[1.0 - alpha / 2.0])),
    ];
    for (label, v) in lines {
        g.txt(row + 1, 0, label);
        g.put(row + 1, 1, v);
        row += 1;
    }
    Ok(g)
}

fn anova_single(s: &Session, p: &Json) -> Result<Grid> {
    let alpha = alpha(p)?;
    let (si, r) = input_range(s, p, &["range", "input"])?;
    let vars = variables(s, si, r, by_rows(p), flag(p, "labels"), &|i| format!("Column {}", i + 1))?;
    let groups: Vec<(String, Vec<f64>)> = vars.into_iter().map(|v| Ok((v.name, numbers(&v.vals, true)?))).collect::<Result<_>>()?;
    if groups.len() < 2 || groups.iter().any(|(_, x)| x.is_empty()) {
        return Err(EngineError::Other("Anova needs at least two groups, each with at least one number.".into()));
    }
    let mut g = Grid::default();
    g.txt(0, 0, "Anova: Single Factor");
    g.txt(2, 0, "SUMMARY");
    g.labels(3, 0, &["Groups", "Count", "Sum", "Average", "Variance"]);
    let all: Vec<f64> = groups.iter().flat_map(|(_, x)| x.iter().copied()).collect();
    let grand = mean(&all);
    let (mut ssb, mut ssw) = (0.0, 0.0);
    for (i, (name, x)) in groups.iter().enumerate() {
        let row = 4 + idx(i);
        let m = mean(x);
        g.txt(row, 0, name);
        g.num(row, 1, x.len() as f64);
        g.num(row, 2, x.iter().sum());
        g.num(row, 3, m);
        g.put(row, 4, n_or_div0(var_s(x)));
        ssb += x.len() as f64 * (m - grand) * (m - grand);
        ssw += ss(x);
    }
    let k = groups.len() as f64;
    let n = all.len() as f64;
    let (dfb, dfw) = (k - 1.0, n - k);
    let row = 4 + idx(groups.len()) + 2;
    g.txt(row, 0, "ANOVA");
    g.labels(row + 1, 0, &["Source of Variation", "SS", "df", "MS", "F", "P-value", "F crit"]);
    let (msb, msw) = (ssb / dfb, ratio(ssw, dfw));
    let f = msw.as_f64().map_or(f64::NAN, |w| msb / w);
    g.txt(row + 2, 0, "Between Groups");
    g.num(row + 2, 1, ssb);
    g.num(row + 2, 2, dfb);
    g.num(row + 2, 3, msb);
    g.put(row + 2, 4, n_or_div0(f));
    g.put(row + 2, 5, if f.is_finite() && dfw > 0.0 { callf("F.DIST.RT", &[f, dfb, dfw]) } else { div0() });
    g.put(row + 2, 6, if dfw > 0.0 { callf("F.INV.RT", &[alpha, dfb, dfw]) } else { div0() });
    g.txt(row + 3, 0, "Within Groups");
    g.num(row + 3, 1, ssw);
    g.num(row + 3, 2, dfw);
    g.put(row + 3, 3, msw);
    g.txt(row + 5, 0, "Total");
    g.num(row + 5, 1, ssb + ssw);
    g.num(row + 5, 2, n - 1.0);
    Ok(g)
}

/// The cells of a range as a dense matrix (row-major), without any header handling.
fn read_matrix(s: &Session, si: usize, r: RangeRef) -> Result<Vec<Vec<Value>>> {
    let d = s.doc()?;
    let sh = d.wb.sheet(si).ok_or(EngineError::NoDocument)?;
    Ok((0..r.height()).map(|i| (0..r.width()).map(|j| sh.value(CellRef::new(r.start.row + i, r.start.col + j))).collect()).collect())
}

/// The name of a header cell, or `fallback` when it is blank.
fn name_of(v: Option<&Value>, fallback: impl FnOnce() -> String) -> String {
    let t = v.map(Value::display).unwrap_or_default();
    if t.trim().is_empty() { fallback() } else { t.trim().to_string() }
}

/// The numbers of a matrix, refusing anything that is not a number (a two-factor Anova's cells
/// are replicates, so a blank would change the design).
fn matrix_numbers(m: &[Vec<Value>]) -> Result<Vec<Vec<f64>>> {
    let mut out = Vec::with_capacity(m.len());
    for row in m {
        let mut r = Vec::with_capacity(row.len());
        for v in row {
            match v {
                Value::Number(n) => r.push(*n),
                _ => return Err(non_numeric()),
            }
        }
        out.push(r);
    }
    Ok(out)
}

/// The Anova table row of an F test: the label, its sums of squares, df, mean square, F, p and
/// the critical value. `f` (with its two degrees of freedom) is omitted for the error and within
/// rows, which have none.
fn anova_row(g: &mut Grid, row: u32, label: &str, ss: f64, df: f64, ms: Value, f: Option<(f64, f64, f64)>, alpha: f64) {
    g.txt(row, 0, label);
    g.num(row, 1, ss);
    g.num(row, 2, df);
    g.put(row, 3, ms);
    if let Some((f, df1, df2)) = f {
        g.put(row, 4, n_or_div0(f));
        g.put(row, 5, if f.is_finite() && df1 > 0.0 && df2 > 0.0 { callf("F.DIST.RT", &[f, df1, df2]) } else { div0() });
        g.put(row, 6, if df1 > 0.0 && df2 > 0.0 { callf("F.INV.RT", &[alpha, df1, df2]) } else { div0() });
    }
}

fn row_mean(m: &[f64], i: usize) -> f64 {
    m.get(i).copied().unwrap_or(f64::NAN)
}

fn col_mean(m: &[f64], j: usize) -> f64 {
    m.get(j).copied().unwrap_or(f64::NAN)
}

/// Two-factor Anova, with or without replication.
///
/// Without replication each matrix cell is one observation: the row means give the Rows effect,
/// the column means the Columns effect, and the residual is the remainder. With replication each
/// cell is `r` observations (`rowsPerSample`), so the design also has an interaction term and a
/// within-cells error.
fn anova_two_factor(s: &Session, p: &Json, replication: bool) -> Result<Grid> {
    let alpha = alpha(p)?;
    let (si, r) = input_range(s, p, &["range", "input"])?;
    let labels = flag(p, "labels");
    let raw = read_matrix(s, si, r)?;
    let mut g = Grid::default();
    if !replication {
        // The matrix rows and columns are the two factors; the labels name them.
        let (row_names, col_names, cells): (Vec<String>, Vec<String>, Vec<Vec<Value>>) = if labels {
            let row_names: Vec<String> = (1..raw.len()).map(|i| name_of(raw.get(i).and_then(|r| r.first()), || format!("Row {i}"))).collect();
            let col_names: Vec<String> =
                (1..raw.first().map_or(0, Vec::len)).map(|j| name_of(raw.first().and_then(|r| r.get(j)), || format!("Column {j}"))).collect();
            let cells = raw.iter().skip(1).map(|row| row.iter().skip(1).cloned().collect()).collect();
            (row_names, col_names, cells)
        } else {
            let a = raw.len();
            let b = raw.first().map_or(0, Vec::len);
            ((1..=a).map(|i| format!("Row {i}")).collect(), (1..=b).map(|j| format!("Column {j}")).collect(), raw)
        };
        let x = matrix_numbers(&cells)?;
        let a = x.len();
        let b = x.first().map_or(0, Vec::len);
        if a < 2 || b < 2 || x.iter().any(|row| row.len() != b) {
            return Err(EngineError::Other("A two-factor Anova needs a rectangle of at least two rows and two columns.".into()));
        }
        let grand = mean(&x.iter().flat_map(|row| row.iter().copied()).collect::<Vec<_>>());
        let row_means: Vec<f64> = x.iter().map(|row| mean(row)).collect();
        let col_means: Vec<f64> = (0..b).map(|j| mean(&x.iter().map(|row| *row.get(j).unwrap_or(&0.0)).collect::<Vec<_>>())).collect();
        let ss_total: f64 = x.iter().flat_map(|row| row.iter()).map(|v| (v - grand) * (v - grand)).sum();
        let ss_rows: f64 = b as f64 * row_means.iter().map(|m| (m - grand) * (m - grand)).sum::<f64>();
        let ss_cols: f64 = a as f64 * col_means.iter().map(|m| (m - grand) * (m - grand)).sum::<f64>();
        let ss_err = ss_total - ss_rows - ss_cols;
        let (df_rows, df_cols, df_err) = (a as f64 - 1.0, b as f64 - 1.0, (a as f64 - 1.0) * (b as f64 - 1.0));
        let ms_err = ratio(ss_err, df_err);
        // SUMMARY: the rows, then the columns, each with count, sum, average and variance.
        g.txt(0, 0, "Anova: Two-Factor Without Replication");
        g.txt(2, 0, "SUMMARY");
        g.labels(2, 1, &["Count", "Sum", "Average", "Variance"]);
        let mut row = 3u32;
        for (i, values) in x.iter().enumerate() {
            g.txt(row, 0, row_names.get(i).cloned().unwrap_or_default());
            g.num(row, 1, values.len() as f64);
            g.num(row, 2, values.iter().sum());
            g.num(row, 3, row_means.get(i).copied().unwrap_or(f64::NAN));
            g.put(row, 4, n_or_div0(var_s(values)));
            row += 1;
        }
        for j in 0..b {
            let values: Vec<f64> = x.iter().map(|r| *r.get(j).unwrap_or(&0.0)).collect();
            g.txt(row, 0, col_names.get(j).cloned().unwrap_or_default());
            g.num(row, 1, values.len() as f64);
            g.num(row, 2, values.iter().sum());
            g.num(row, 3, col_means.get(j).copied().unwrap_or(f64::NAN));
            g.put(row, 4, n_or_div0(var_s(&values)));
            row += 1;
        }
        row += 1;
        g.txt(row, 0, "ANOVA");
        g.labels(row + 1, 0, &["Source of Variation", "SS", "df", "MS", "F", "P-value", "F crit"]);
        row += 2;
        let f = |ss: f64, df: f64| ms_err.as_f64().map(|e| ratio(ss, df).as_f64().map_or(f64::NAN, |m| m / e));
        anova_row(&mut g, row, "Rows", ss_rows, df_rows, ratio(ss_rows, df_rows), f(ss_rows, df_rows).map(|f| (f, df_rows, df_err)), alpha);
        anova_row(&mut g, row + 1, "Columns", ss_cols, df_cols, ratio(ss_cols, df_cols), f(ss_cols, df_cols).map(|f| (f, df_cols, df_err)), alpha);
        anova_row(&mut g, row + 2, "Error", ss_err, df_err, ms_err, None, alpha);
        g.txt(row + 4, 0, "Total");
        g.num(row + 4, 1, ss_total);
        g.num(row + 4, 2, a as f64 * b as f64 - 1.0);
        return Ok(g);
    }

    // With replication: `r` rows per row level, one column per column level.
    let reps = num(p, "rowsPerSample").or_else(|| num(p, "replicates")).map(|v| v.round()).filter(|v| *v >= 2.0 && v.fract() == 0.0);
    let Some(reps) = reps else {
        return Err(bad(CMD, "anovaTwoFactorRep needs `rowsPerSample`, the number of rows in each sample (at least 2)"));
    };
    let reps = reps as usize;
    let (header_rows, header_cols) = if labels { (1, 1) } else { (0, 0) };
    let data_rows = raw.len().saturating_sub(header_rows);
    let data_cols = raw.first().map_or(0, Vec::len).saturating_sub(header_cols);
    if data_rows == 0 || data_cols == 0 || data_rows % reps != 0 {
        return Err(EngineError::Other(format!("The input range must hold whole {reps}-row blocks of replicates.")));
    }
    let a = data_rows / reps;
    let b = data_cols;
    if a < 2 || b < 2 {
        return Err(EngineError::Other("A two-factor Anova needs at least two rows and two columns of samples.".into()));
    }
    let a_names: Vec<String> = (0..a).map(|i| name_of(raw.get(header_rows + i * reps).and_then(|r| r.first()), || format!("Row {i}"))).collect();
    let b_names: Vec<String> = (0..b).map(|j| name_of(raw.first().and_then(|r| r.get(header_cols + j)), || format!("Column {j}"))).collect();
    // cells[i][j] = the r observations of (row level i, column level j).
    let mut cells: Vec<Vec<Vec<f64>>> = Vec::with_capacity(a);
    for i in 0..a {
        let mut row = Vec::with_capacity(b);
        for j in 0..b {
            let mut block = Vec::with_capacity(reps);
            for ri in 0..reps {
                let r0 = header_rows + i * reps + ri;
                let c0 = header_cols + j;
                match raw.get(r0).and_then(|r| r.get(c0)) {
                    Some(Value::Number(n)) => block.push(*n),
                    _ => return Err(non_numeric()),
                }
            }
            row.push(block);
        }
        cells.push(row);
    }
    let all: Vec<f64> = cells.iter().flat_map(|row| row.iter()).flat_map(|b| b.iter().copied()).collect();
    let grand = mean(&all);
    let cell_means: Vec<Vec<f64>> = cells.iter().map(|row| row.iter().map(|b| mean(b)).collect()).collect();
    let row_means: Vec<f64> = cell_means.iter().map(|row| mean(row)).collect();
    let col_means: Vec<f64> = (0..b).map(|j| mean(&cell_means.iter().map(|row| *row.get(j).unwrap_or(&0.0)).collect::<Vec<_>>())).collect();
    let ss_total: f64 = all.iter().map(|v| (v - grand) * (v - grand)).sum();
    let ss_a: f64 = (b * reps) as f64 * row_means.iter().map(|m| (m - grand) * (m - grand)).sum::<f64>();
    let ss_b: f64 = (a * reps) as f64 * col_means.iter().map(|m| (m - grand) * (m - grand)).sum::<f64>();
    let mut ss_ab = 0.0;
    for (i, row) in cell_means.iter().enumerate() {
        for (j, m) in row.iter().enumerate() {
            let v = m - row_mean(&row_means, i) - col_mean(&col_means, j) + grand;
            ss_ab += v * v;
        }
    }
    ss_ab *= reps as f64;
    let mut ss_within = 0.0;
    for (i, row) in cells.iter().enumerate() {
        for (j, block) in row.iter().enumerate() {
            let m = cell_means.get(i).and_then(|r| r.get(j)).copied().unwrap_or(f64::NAN);
            ss_within += block.iter().map(|v| (v - m) * (v - m)).sum::<f64>();
        }
    }
    let (df_a, df_b, df_ab, df_w) = (a as f64 - 1.0, b as f64 - 1.0, (a as f64 - 1.0) * (b as f64 - 1.0), (a * b * (reps - 1)) as f64);
    let ms_w = ratio(ss_within, df_w);
    // SUMMARY: one block per row level, its column levels and a Total column.
    g.txt(0, 0, "Anova: Two-Factor With Replication");
    g.txt(2, 0, "SUMMARY");
    let mut row = 3u32;
    for i in 0..a {
        g.txt(row, 0, a_names.get(i).cloned().unwrap_or_default());
        for j in 0..b {
            g.txt(row, 1 + idx(j), b_names.get(j).cloned().unwrap_or_default());
        }
        g.txt(row, 1 + idx(b), "Total");
        row += 1;
        let base = row;
        for (k, label) in ["Count", "Sum", "Average", "Variance"].iter().enumerate() {
            g.txt(base + idx(k), 0, *label);
        }
        let mut total: Vec<f64> = Vec::new();
        for j in 0..b {
            let block = cells.get(i).and_then(|r| r.get(j)).cloned().unwrap_or_default();
            total.extend(block.iter().copied());
            let col = 1 + idx(j);
            g.num(base, col, block.len() as f64);
            g.num(base + 1, col, block.iter().sum());
            g.num(base + 2, col, mean(&block));
            g.put(base + 3, col, n_or_div0(var_s(&block)));
        }
        let col = 1 + idx(b);
        g.num(base, col, total.len() as f64);
        g.num(base + 1, col, total.iter().sum());
        g.num(base + 2, col, mean(&total));
        g.put(base + 3, col, n_or_div0(var_s(&total)));
        row = base + 5;
    }
    row += 1;
    g.txt(row, 0, "ANOVA");
    g.labels(row + 1, 0, &["Source of Variation", "SS", "df", "MS", "F", "P-value", "F crit"]);
    row += 2;
    let f = |ss: f64, df: f64| ms_w.as_f64().map(|e| ratio(ss, df).as_f64().map_or(f64::NAN, |m| m / e));
    anova_row(&mut g, row, "Sample", ss_a, df_a, ratio(ss_a, df_a), f(ss_a, df_a).map(|f| (f, df_a, df_w)), alpha);
    anova_row(&mut g, row + 1, "Columns", ss_b, df_b, ratio(ss_b, df_b), f(ss_b, df_b).map(|f| (f, df_b, df_w)), alpha);
    anova_row(&mut g, row + 2, "Interaction", ss_ab, df_ab, ratio(ss_ab, df_ab), f(ss_ab, df_ab).map(|f| (f, df_ab, df_w)), alpha);
    anova_row(&mut g, row + 3, "Within", ss_within, df_w, ms_w, None, alpha);
    g.txt(row + 5, 0, "Total");
    g.num(row + 5, 1, ss_total);
    g.num(row + 5, 2, (a * b * reps) as f64 - 1.0);
    Ok(g)
}

// ---------------------------------------------------------------- output

/// Writes the report at `output` (refusing to overwrite data unless `overwrite`) or on a new
/// sheet.
fn write_report(s: &mut Session, p: &Json, tool: &str, g: Grid) -> Result<Json> {
    let (rows, cols) = g.extent();
    let (rows, cols) = (rows.max(1), cols.max(1));
    if u64::from(rows) * u64::from(cols) > MAX_OUTPUT_CELLS {
        return Err(bad(CMD, "the report is too large"));
    }
    let d = s.doc()?;
    let target = match str_param(p, "outputTo").map(str::to_ascii_lowercase).as_deref() {
        Some("newsheet") => None,
        Some("range") => Some(text(p, &["output", "outputRange"]).ok_or_else(|| bad(CMD, "missing `output` (the output range)"))?),
        _ => text(p, &["output", "outputRange"]),
    };
    let (si, at, new_sheet) = match target {
        Some(t) => {
            let (si, r) = parse_range(&d.wb, t, CMD)?;
            (si, r.start, None)
        }
        None => {
            if d.wb.protected_structure {
                return Err(EngineError::Other("The workbook is protected and cannot be changed.".into()));
            }
            let name = match p.get("newSheet") {
                Some(Json::String(n)) if !n.trim().is_empty() => n.trim().to_string(),
                _ => d.wb.next_sheet_name(),
            };
            d.wb.check_sheet_name(&name, None).map_err(EngineError::Other)?;
            (d.wb.sheets.len(), CellRef::new(0, 0), Some(name))
        }
    };
    let end_row = at.row.checked_add(rows - 1).filter(|r| *r < MAX_ROWS);
    let end_col = at.col.checked_add(cols - 1).filter(|c| *c < MAX_COLS);
    let (Some(end_row), Some(end_col)) = (end_row, end_col) else {
        return Err(EngineError::Other("The report doesn't fit on the sheet below and right of the output cell.".into()));
    };
    let rect = RangeRef::new(at, CellRef::new(end_row, end_col));
    // Existing content inside the report's rectangle.
    let mut occupied: Vec<CellRef> = Vec::new();
    if new_sheet.is_none()
        && let Some(sh) = d.wb.sheet(si)
    {
        for row in rect.start.row..=rect.end.row {
            for (c, cell) in sh.cells.row(row, rect.start.col, rect.end.col) {
                if !cell.value.is_empty() || cell.formula.is_some() {
                    occupied.push(CellRef::new(row, c));
                }
            }
        }
        if sh.spill.keys().any(|c| rect.contains(*c)) && occupied.is_empty() {
            occupied.push(rect.start);
        }
    }
    if !occupied.is_empty() && !flag(p, "overwrite") {
        return Err(EngineError::Other(format!("The output range {} {OVERWRITE_ERROR}; pass overwrite: true to replace it.", rect.a1())));
    }
    let sheet_name = new_sheet.clone().or_else(|| d.wb.sheet(si).map(|sh| sh.name.clone())).unwrap_or_default();
    let new_index = edit(s, |cx| {
        let pct_fmt = NumFmt::new("0.00%");
        if let Some(name) = &new_sheet {
            cx.wb.sheets.push(std::sync::Arc::new(Sheet::new(name.clone())));
            cx.structural = true;
        }
        let pct_styles: Vec<(CellRef, gridcraft_model::StyleId)> = {
            let sh = cx.wb.sheet(si).ok_or(EngineError::NoDocument)?;
            g.pct.iter().map(|(r, c)| CellRef::new(at.row + r, at.col + c)).map(|c| (c, sh.style_id(c))).collect()
        };
        let pct_styles: HashMap<CellRef, gridcraft_model::StyleId> =
            pct_styles.into_iter().map(|(c, st)| (c, cx.wb.styles.derive(st, |s| s.num_fmt = pct_fmt.clone()))).collect();
        let sh = cx.sheet_mut(si)?;
        for c in &occupied {
            let style = sh.style_id(*c);
            sh.set_cell(*c, Cell { value: Value::Empty, formula: None, style });
        }
        let mut touched = occupied.clone();
        for ((r, c), v) in &g.cells {
            let cell = CellRef::new(at.row + r, at.col + c);
            let style = pct_styles.get(&cell).copied().unwrap_or_else(|| sh.style_id(cell));
            sh.set_cell(cell, Cell { value: v.clone(), formula: None, style });
            touched.push(cell);
        }
        for c in touched {
            cx.touch(si, c);
        }
        Ok(new_sheet.as_ref().map(|_| si))
    })?;
    if let Some(i) = new_index {
        s.doc_mut()?.wb_switch_sheet(i);
    }
    Ok(json!({"tool": tool, "sheet": sheet_name, "range": rect.a1(), "rows": rows, "cols": cols}))
}

fn run(s: &mut Session, p: &Json) -> Result<Json> {
    let Some(t) = text(p, &["tool"]) else {
        s.ui_requests.push(crate::UiRequest::Dialog("analysisToolPak".into(), json!({})));
        return Ok(json!({"tools": TOOLS.iter().map(|(id, label)| json!({"id": id, "label": label})).collect::<Vec<_>>()}));
    };
    let tool = tool_id(t).ok_or_else(|| bad(CMD, format!("unknown tool `{t}`")))?;
    let g = match tool {
        "descriptive" => descriptive(s, p)?,
        "histogram" => histogram(s, p)?,
        "correlation" => matrix(s, p, correlation, |_| Value::Number(1.0))?,
        "covariance" => matrix(s, p, covariance_p, |x| covariance_p(x, x))?,
        "regression" => regression(s, p)?,
        "movingAverage" => moving_average(s, p)?,
        "exponentialSmoothing" => exponential_smoothing(s, p)?,
        "rankPercentile" => rank_percentile(s, p)?,
        "tTestPaired" | "tTestEqual" | "tTestUnequal" => t_test(s, p, tool)?,
        "zTest" => z_test(s, p)?,
        "fTest" => f_test(s, p)?,
        "anovaSingle" => anova_single(s, p)?,
        "anovaTwoFactor" => anova_two_factor(s, p, false)?,
        "anovaTwoFactorRep" => anova_two_factor(s, p, true)?,
        _ => return Err(bad(CMD, format!("unknown tool `{t}`"))),
    };
    write_report(s, p, tool, g)
}

#[cfg(test)]
mod tests {
    use gridcraft_core::{CellRef, Value};
    use serde_json::json;

    use crate::Session;

    fn s() -> Session {
        let mut s = Session::new();
        s.new_workbook();
        s
    }
    fn v(s: &Session, a: &str) -> Value {
        let d = s.doc().unwrap();
        let (sh, body) = crate::cmd::split_sheet(a);
        let si = sh.map(|n| d.wb.sheet_index(&n).unwrap()).unwrap_or(d.wb.active_sheet);
        d.wb.sheet(si).unwrap().value(CellRef::parse(body).unwrap())
    }
    fn n(s: &Session, a: &str) -> f64 {
        v(s, a).as_f64().unwrap_or(f64::NAN)
    }
    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }
    /// Evaluates a formula in a scratch cell.
    fn eval(s: &mut Session, f: &str) -> f64 {
        s.execute("cell.set", json!({"cell": "Z1000", "input": f})).unwrap();
        let x = n(s, "Z1000");
        s.execute("cell.set", json!({"cell": "Z1000", "input": ""})).unwrap();
        x
    }

    #[test]
    fn forecast_standard_errors_match_the_full_window_rescan() {
        // #1043: the running sum must give what re-adding every window gave, gaps included.
        let x: [f64; 11] = [2.0, 4.0, 5.0, 4.0, 5.0, 7.0, 1.0, 3.0, 8.0, 6.0, 2.0];
        let f = [None, Some(3.0), Some(4.5), None, Some(4.0), Some(6.5), Some(2.0), Some(2.5), Some(7.0), Some(5.0), Some(4.0)];
        let rescan = |i: usize, w: usize| -> Value {
            let v = (i + 1).checked_sub(w).and_then(|s0| {
                let mut acc = 0.0;
                for j in s0..=i {
                    acc += (x.get(j)? - (*f.get(j)?)?).powi(2);
                }
                Some((acc / w as f64).sqrt())
            });
            v.map(Value::number).unwrap_or(Value::Error(gridcraft_core::CellError::NA))
        };
        for w in 1..=5 {
            let g = super::forecast_grid(&x, &f, w, &json!({"standardErrors": true}));
            for i in 0..f.len() {
                let (got, want) = (g.cells.get(&(i as u32, 1)).cloned().unwrap_or(Value::Empty), rescan(i, w));
                match (got.as_f64(), want.as_f64()) {
                    (Some(a), Some(b)) => assert!(close(a, b, 1e-12), "window {w}, row {i}: {a} vs {b}"),
                    _ => assert_eq!(got, want, "window {w}, row {i}"),
                }
            }
        }
        // Row 3 has no forecast: windows through it are #N/A, the next full window after it is not.
        let g = super::forecast_grid(&x, &f, 3, &json!({"standardErrors": true}));
        assert_eq!(g.cells.get(&(5, 1)), Some(&Value::Error(gridcraft_core::CellError::NA)));
        assert!(g.cells.get(&(6, 1)).and_then(Value::as_f64).is_some());
    }

    #[test]
    fn descriptive_statistics_of_one_to_five() {
        // 1..5: mean 3, s = √2.5, kurtosis −1.2, skewness 0 (textbook values).
        let mut s = s();
        s.execute("range.setValues", json!({"range": "A1", "values": [["Score"], [1], [2], [3], [4], [5]]})).unwrap();
        let r = s
            .execute("data.analysisToolPak", json!({"tool": "Descriptive Statistics", "range": "A1:A6", "labels": true, "output": "C1", "confidence": 95, "kthLargest": 2, "kthSmallest": 1}))
            .unwrap();
        assert_eq!(r["range"], "C1:D18");
        assert_eq!(v(&s, "C1"), Value::text("Score"));
        assert_eq!(v(&s, "C3"), Value::text("Mean"));
        assert_eq!(n(&s, "D3"), 3.0);
        assert!(close(n(&s, "D4"), (2.5f64 / 5.0).sqrt(), 1e-12));
        assert_eq!(n(&s, "D5"), 3.0);
        assert_eq!(v(&s, "D6"), Value::Error(gridcraft_core::CellError::NA));
        assert!(close(n(&s, "D7"), 2.5f64.sqrt(), 1e-12));
        assert_eq!(n(&s, "D8"), 2.5);
        assert!(close(n(&s, "D9"), -1.2, 1e-12));
        assert!(close(n(&s, "D10"), 0.0, 1e-12));
        assert_eq!((n(&s, "D11"), n(&s, "D12"), n(&s, "D13"), n(&s, "D14"), n(&s, "D15")), (4.0, 1.0, 5.0, 15.0, 5.0));
        assert_eq!(v(&s, "C16"), Value::text("Largest(2)"));
        assert_eq!(n(&s, "D16"), 4.0);
        assert_eq!(n(&s, "D17"), 1.0);
        assert_eq!(v(&s, "C18"), Value::text("Confidence Level(95.0%)"));
        let want = eval(&mut s, "=CONFIDENCE.T(0.05,STDEV.S(A2:A6),5)");
        assert!(close(n(&s, "D18"), want, 1e-9), "{} vs {want}", n(&s, "D18"));
        // The output range now holds data: a second run is refused unless it may overwrite.
        let again = json!({"tool": "descriptive", "range": "A2:A6", "output": "C1"});
        let e = s.execute("data.analysisToolPak", again.clone()).unwrap_err().to_string();
        assert!(e.contains(super::OVERWRITE_ERROR), "{e}");
        let mut again = again;
        again["overwrite"] = json!(true);
        s.execute("data.analysisToolPak", again).unwrap();
        assert_eq!(v(&s, "C1"), Value::text("Column1"));
    }

    #[test]
    fn regression_matches_textbook_and_linest() {
        // x = 1..5, y = 2,4,5,4,5: b = 0.6, a = 2.2, R² = 0.6, F = 4.5 (textbook example).
        let mut s = s();
        s.execute("range.setValues", json!({"range": "A1", "values": [["y", "x"], [2, 1], [4, 2], [5, 3], [4, 4], [5, 5]]})).unwrap();
        s.execute(
            "data.analysisToolPak",
            json!({"tool": "regression", "inputY": "A1:A6", "inputX": "B1:B6", "labels": true, "newSheet": "Fit", "residuals": true, "standardizedResiduals": true}),
        )
        .unwrap();
        assert!(close(n(&s, "Fit!B5"), 0.6, 1e-12), "R²");
        assert!(close(n(&s, "Fit!B6"), 1.0 - 0.4 * 4.0 / 3.0, 1e-12), "adjusted R²");
        assert!(close(n(&s, "Fit!B7"), 0.8f64.sqrt(), 1e-12), "standard error");
        assert!(close(n(&s, "Fit!E12"), 4.5, 1e-12), "F");
        assert!(close(n(&s, "Fit!B17"), 2.2, 1e-12), "intercept");
        assert!(close(n(&s, "Fit!B18"), 0.6, 1e-12), "slope");
        assert_eq!(v(&s, "Fit!A18"), Value::text("x"));
        assert!(close(n(&s, "Fit!C18"), (0.8f64 / 10.0).sqrt(), 1e-12), "SE of slope");
        // Simple regression: Significance F equals the slope's p-value.
        assert!(close(n(&s, "Fit!F12"), n(&s, "Fit!E18"), 1e-9));
        assert!(n(&s, "Fit!E18") > 0.12 && n(&s, "Fit!E18") < 0.13);
        assert_eq!(v(&s, "Fit!A22"), Value::text("RESIDUAL OUTPUT"));
        assert!(close(n(&s, "Fit!B25"), 2.8, 1e-12), "first prediction");
        assert!(close(n(&s, "Fit!C25"), -0.8, 1e-12), "first residual");
        // Multiple regression agrees with LINEST.
        s.execute(
            "range.setValues",
            json!({"range": "Sheet1!D1", "values": [[1, 2, 3], [3, 1, 5], [4, 5, 2], [7, 3, 4], [6, 6, 1], [9, 4, 6], [8, 7, 3]]}),
        )
        .unwrap();
        s.execute("data.analysisToolPak", json!({"tool": "regression", "inputY": "Sheet1!D1:D7", "inputX": "Sheet1!E1:F7", "newSheet": "Multi"}))
            .unwrap();
        let slope2 = eval(&mut s, "=INDEX(LINEST(Sheet1!D1:D7,Sheet1!E1:F7,TRUE,TRUE),1,1)");
        let slope1 = eval(&mut s, "=INDEX(LINEST(Sheet1!D1:D7,Sheet1!E1:F7,TRUE,TRUE),1,2)");
        let se1 = eval(&mut s, "=INDEX(LINEST(Sheet1!D1:D7,Sheet1!E1:F7,TRUE,TRUE),2,2)");
        let icpt = eval(&mut s, "=INDEX(LINEST(Sheet1!D1:D7,Sheet1!E1:F7,TRUE,TRUE),1,3)");
        assert!(close(n(&s, "Multi!B17"), icpt, 1e-9));
        assert!(close(n(&s, "Multi!B18"), slope1, 1e-9));
        assert!(close(n(&s, "Multi!B19"), slope2, 1e-9));
        assert!(close(n(&s, "Multi!C18"), se1, 1e-9));
        assert_eq!(v(&s, "Multi!A19"), Value::text("X Variable 2"));
    }

    #[test]
    fn tests_of_means_and_variances_match_worksheet_functions() {
        let mut s = s();
        s.execute("range.setValues", json!({"range": "A1", "values": [[12, 10], [15, 14], [11, 9], [18, 15], [14, 13], [16, 12]]})).unwrap();
        s.execute("data.analysisToolPak", json!({"tool": "tTestEqual", "range1": "A1:A6", "range2": "B1:B6", "output": "D1"})).unwrap();
        assert_eq!(v(&s, "D1"), Value::text("t-Test: Two-Sample Assuming Equal Variances"));
        assert_eq!(n(&s, "E9"), 10.0, "df");
        let p2 = eval(&mut s, "=T.TEST(A1:A6,B1:B6,2,2)");
        assert!(close(n(&s, "E13"), p2, 1e-9), "two-tail p");
        s.execute("data.analysisToolPak", json!({"tool": "tTestPaired", "range1": "A1:A6", "range2": "B1:B6", "output": "H1"})).unwrap();
        let pp = eval(&mut s, "=T.TEST(A1:A6,B1:B6,2,1)");
        assert_eq!(v(&s, "H13"), Value::text("P(T<=t) two-tail"));
        assert!(close(n(&s, "I13"), pp, 1e-9), "paired two-tail p");
        s.execute("data.analysisToolPak", json!({"tool": "fTest", "range1": "A1:A6", "range2": "B1:B6", "output": "L1"})).unwrap();
        let fp = eval(&mut s, "=F.TEST(A1:A6,B1:B6)");
        assert!(close(n(&s, "M9") * 2.0, fp, 1e-9), "one-tail p is half of F.TEST");
        // Anova on {1,2,3}, {4,5,6}, {7,8,9}: SSB 54, SSW 6, F 27.
        s.execute("range.setValues", json!({"range": "A20", "values": [[1, 4, 7], [2, 5, 8], [3, 6, 9]]})).unwrap();
        s.execute("data.analysisToolPak", json!({"tool": "anovaSingle", "range": "A20:C22", "output": "E20"})).unwrap();
        assert_eq!(v(&s, "E31"), Value::text("Between Groups"));
        assert_eq!((n(&s, "F31"), n(&s, "F32"), n(&s, "I31")), (54.0, 6.0, 27.0));
        let p = eval(&mut s, "=F.DIST.RT(27,2,6)");
        assert!(close(n(&s, "J31"), p, 1e-12));
    }

    #[test]
    fn histogram_matrices_smoothing_and_ranks() {
        let mut s = s();
        s.execute("range.setValues", json!({"range": "A1", "values": [[1], [2], [2], [3], [5], [7]]})).unwrap();
        s.execute("range.setValues", json!({"range": "B1", "values": [[2], [4], [6]]})).unwrap();
        s.execute(
            "data.analysisToolPak",
            json!({"tool": "histogram", "range": "A1:A6", "bins": "B1:B3", "output": "D1", "cumulative": true, "pareto": true}),
        )
        .unwrap();
        assert_eq!((n(&s, "E2"), n(&s, "E3"), n(&s, "E4"), n(&s, "E5")), (3.0, 1.0, 1.0, 1.0));
        assert_eq!(v(&s, "D5"), Value::text("More"));
        assert_eq!(n(&s, "F5"), 1.0);
        assert_eq!((n(&s, "G2"), n(&s, "H2")), (2.0, 3.0), "Pareto puts the largest bin first");
        // Correlation and covariance against CORREL and COVARIANCE.P.
        s.execute("range.setValues", json!({"range": "J1", "values": [[1, 2], [2, 4], [3, 5], [4, 4], [5, 5]]})).unwrap();
        s.execute("data.analysisToolPak", json!({"tool": "correlation", "range": "J1:K5", "output": "M1"})).unwrap();
        assert_eq!(n(&s, "N2"), 1.0);
        assert!(close(n(&s, "N3"), eval(&mut s, "=CORREL(J1:J5,K1:K5)"), 1e-12));
        assert!(v(&s, "O2").is_empty(), "upper triangle is blank");
        s.execute("data.analysisToolPak", json!({"tool": "covariance", "range": "J1:K5", "output": "M10"})).unwrap();
        assert!(close(n(&s, "N12"), eval(&mut s, "=COVARIANCE.P(J1:J5,K1:K5)"), 1e-12));
        assert!(close(n(&s, "N11"), eval(&mut s, "=VAR.P(J1:J5)"), 1e-12));
        // Moving average of 3 and exponential smoothing with damping 0.3.
        s.execute("data.analysisToolPak", json!({"tool": "movingAverage", "range": "K1:K5", "output": "Q1", "standardErrors": true})).unwrap();
        assert_eq!(v(&s, "Q2"), Value::Error(gridcraft_core::CellError::NA));
        assert!(close(n(&s, "Q3"), 11.0 / 3.0, 1e-12));
        assert!(close(n(&s, "Q5"), 14.0 / 3.0, 1e-12));
        assert_eq!(v(&s, "R4"), Value::Error(gridcraft_core::CellError::NA));
        assert!(n(&s, "R5").is_finite());
        s.execute("data.analysisToolPak", json!({"tool": "exponentialSmoothing", "range": "K1:K5", "output": "S1"})).unwrap();
        assert_eq!(n(&s, "S2"), 2.0);
        assert!(close(n(&s, "S3"), 0.7 * 4.0 + 0.3 * 2.0, 1e-12));
        // Rank and Percentile of 2,4,5,4,5: the two 5s share rank 1 (3 of 4 others below them).
        s.execute("data.analysisToolPak", json!({"tool": "rankPercentile", "range": "K1:K5", "output": "U1"})).unwrap();
        assert_eq!((n(&s, "U2"), n(&s, "V2"), n(&s, "W2"), n(&s, "X2")), (3.0, 5.0, 1.0, 0.75));
        assert_eq!((n(&s, "U3"), n(&s, "W3")), (5.0, 1.0));
        assert_eq!((n(&s, "V6"), n(&s, "X6")), (2.0, 0.0));
        assert_eq!(n(&s, "X4"), 0.25, "4 has one value below it among five");
    }

    #[test]
    fn hostile_inputs_are_refused_without_panicking() {
        let mut s = s();
        let r = s.execute("data.analysisToolPak", json!({})).unwrap();
        assert!(r["tools"].as_array().is_some_and(|t| t.len() == super::TOOLS.len()));
        assert!(s.take_ui_requests().iter().any(|q| matches!(q, crate::UiRequest::Dialog(n, _) if n == "analysisToolPak")));
        s.execute("range.setValues", json!({"range": "A1", "values": [[1, "x"], [2, 3]]})).unwrap();
        for p in [
            json!({"tool": "nope"}),
            json!({"tool": "descriptive"}),
            json!({"tool": "descriptive", "range": "A1:XFD1048576"}),
            json!({"tool": "descriptive", "range": "A1:B2"}),
            json!({"tool": "descriptive", "range": "A1:A2", "output": "XFD1048576"}),
            json!({"tool": "descriptive", "range": "A1:A2", "newSheet": "bad[name]"}),
            json!({"tool": "regression", "inputY": "A1:A2", "inputX": "A1:A2"}),
            json!({"tool": "regression", "inputY": "A1:A2", "inputX": "A1:Q2"}),
            json!({"tool": "movingAverage", "range": "A1:A2", "interval": 1e300}),
            json!({"tool": "movingAverage", "range": "A1:A2", "interval": -3}),
            json!({"tool": "exponentialSmoothing", "range": "A1:A2", "damping": 7}),
            json!({"tool": "histogram", "range": "C1:C3"}),
            json!({"tool": "tTestPaired", "range1": "A1", "range2": "A2"}),
            json!({"tool": "anovaSingle", "range": "A1:A2"}),
            json!({"tool": "correlation", "range": "A1:B2", "alpha": "x", "groupedBy": 5}),
            json!({"tool": "descriptive", "range": "A1:A2", "confidence": 250}),
        ] {
            assert!(s.execute("data.analysisToolPak", p.clone()).is_err(), "{p}");
        }
        // Collinear X columns are an error, not a NaN report.
        s.execute("range.setValues", json!({"range": "D1", "values": [[1, 1, 2], [2, 2, 4], [3, 3, 6], [5, 4, 8]]})).unwrap();
        assert!(s.execute("data.analysisToolPak", json!({"tool": "regression", "inputY": "D1:D4", "inputX": "E1:F4"})).is_err());
        // Constant data gives #DIV/0! cells, not a crash.
        s.execute("range.setValues", json!({"range": "H1", "values": [[4], [4], [4]]})).unwrap();
        s.execute("data.analysisToolPak", json!({"tool": "descriptive", "range": "H1:H3", "output": "J1"})).unwrap();
        assert_eq!(v(&s, "K9"), Value::Error(gridcraft_core::CellError::Div0));
    }
}

#[cfg(test)]
mod z_test_tests {
    use gridcraft_core::Value;
    use serde_json::{Value as Json, json};

    use crate::Session;

    fn session() -> Session {
        let mut s = Session::new();
        s.new_workbook();
        s.execute("range.setValues", json!({"range": "A1", "values": [[12, 10], [15, 14], [11, 9], [18, 15], [14, 13], [16, 12]]})).unwrap();
        s
    }
    fn value(s: &Session, a: &str) -> Value {
        let d = s.doc().unwrap();
        let (sh, body) = crate::cmd::split_sheet(a);
        let si = sh.map(|n| d.wb.sheet_index(&n).unwrap()).unwrap_or(d.wb.active_sheet);
        d.wb.sheet(si).unwrap().value(gridcraft_core::CellRef::parse(body).unwrap())
    }
    fn num(s: &Session, a: &str) -> f64 {
        value(s, a).as_f64().unwrap_or(f64::NAN)
    }
    fn eval(s: &mut Session, f: &str) -> f64 {
        s.execute("cell.set", json!({"cell": "Z1000", "input": f})).unwrap();
        let x = num(s, "Z1000");
        s.execute("cell.set", json!({"cell": "Z1000", "input": ""})).unwrap();
        x
    }
    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9
    }

    #[test]
    fn a_z_test_reports_the_same_numbers_as_the_worksheet_functions() {
        let mut s = session();
        let v1 = 6.266_666_666_666_667;
        let v2 = 5.366_666_666_666_666;
        s.execute(
            "data.analysisToolPak",
            json!({"tool": "zTest", "range1": "A1:A6", "range2": "B1:B6", "variance1": v1, "variance2": v2, "output": "D1"}),
        )
        .unwrap();
        // The layout is the t-tests': title, the two variable names, then paired rows and the
        // statistic rows under Variable 1.
        assert_eq!(value(&s, "D1"), Value::text("z-Test: Two Sample for Means"));
        assert_eq!((value(&s, "E3"), value(&s, "F3")), (Value::text("Variable 1"), Value::text("Variable 2")));
        assert!((num(&s, "E4") - eval(&mut s, "=AVERAGE(A1:A6)")).abs() < 1e-12);
        assert!((num(&s, "F4") - eval(&mut s, "=AVERAGE(B1:B6)")).abs() < 1e-12);
        assert_eq!((num(&s, "E5"), num(&s, "F5")), (v1, v2));
        assert_eq!((num(&s, "E6"), num(&s, "F6")), (6.0, 6.0));
        assert_eq!(value(&s, "D7"), Value::text("Hypothesized Mean Difference"));
        assert_eq!(num(&s, "E7"), 0.0);
        // z = (mean1 - mean2 - difference) / sqrt(v1/n1 + v2/n2).
        let z = eval(&mut s, "=(AVERAGE(A1:A6)-AVERAGE(B1:B6))/SQRT(E5/6+F5/6)");
        assert!(close(num(&s, "E8"), z), "z {} vs {z}", num(&s, "E8"));
        assert_eq!(value(&s, "D8"), Value::text("z"));
        // The one-tail p is 1 - Phi(|z|); the two-tail p is twice it, and the critical values are
        // the normal inverses at 1-alpha and 1-alpha/2.
        let p1 = eval(&mut s, "=1-NORM.S.DIST(ABS(E8),TRUE)");
        assert!(close(num(&s, "E9"), p1));
        assert_eq!(value(&s, "D9"), Value::text("P(Z<=z) one-tail"));
        assert!(close(num(&s, "E10"), eval(&mut s, "=NORM.S.INV(0.95)")));
        assert!(close(num(&s, "E11"), 2.0 * p1));
        assert!(close(num(&s, "E12"), eval(&mut s, "=NORM.S.INV(0.975)")));
        // A non-zero hypothesized difference shifts z.
        s.execute(
            "data.analysisToolPak",
            json!({"tool": "zTest", "range1": "A1:A6", "range2": "B1:B6", "variance1": v1, "variance2": v2, "hypothesizedDifference": 1, "output": "H1"}),
        )
        .unwrap();
        assert!(close(num(&s, "I8"), (z * (v1 / 6.0 + v2 / 6.0).sqrt() - 1.0) / (v1 / 6.0 + v2 / 6.0).sqrt()));
    }

    #[test]
    fn a_z_test_refuses_missing_or_unusable_variances() {
        fn err(s: &mut Session, p: Json) -> String {
            s.execute("data.analysisToolPak", p).unwrap_err().to_string()
        }
        let mut s = session();
        assert!(err(&mut s, json!({"tool": "zTest", "range1": "A1:A6", "range2": "B1:B6", "variance2": 1, "output": "H1"})).contains("variance1"));
        assert!(err(&mut s, json!({"tool": "zTest", "range1": "A1:A6", "range2": "B1:B6", "variance1": 1, "output": "H1"})).contains("variance2"));
        assert!(
            err(&mut s, json!({"tool": "zTest", "range1": "A1:A6", "range2": "B1:B6", "variance1": 0, "variance2": 1, "output": "H1"}))
                .contains("past zero")
        );
        assert!(
            err(&mut s, json!({"tool": "zTest", "range1": "A1:A6", "range2": "B1:B6", "variance1": -2, "variance2": 1, "output": "H1"}))
                .contains("past zero")
        );
        assert!(err(&mut s, json!({"tool": "zTest", "range1": "A1:A6", "variance1": 1, "variance2": 1, "output": "H1"})).contains("range2"));
        // The tool list names it (the dialog lists what the command reports).
        let tools = s.execute("data.analysisToolPak", json!({})).unwrap();
        assert!(tools["tools"].as_array().is_some_and(|a| a.iter().any(|t| t["id"] == "zTest")));
    }
}

#[cfg(test)]
mod anova_two_factor_tests {
    use gridcraft_core::Value;
    use serde_json::{Value as Json, json};

    use crate::Session;

    fn session() -> Session {
        let mut s = Session::new();
        s.new_workbook();
        s
    }
    fn value(s: &Session, a: &str) -> Value {
        let d = s.doc().unwrap();
        let (sh, body) = crate::cmd::split_sheet(a);
        let si = sh.map(|n| d.wb.sheet_index(&n).unwrap()).unwrap_or(d.wb.active_sheet);
        d.wb.sheet(si).unwrap().value(gridcraft_core::CellRef::parse(body).unwrap())
    }
    fn num(s: &Session, a: &str) -> f64 {
        value(s, a).as_f64().unwrap_or(f64::NAN)
    }
    fn eval(s: &mut Session, f: &str) -> f64 {
        s.execute("cell.set", json!({"cell": "Z1000", "input": f})).unwrap();
        let x = num(s, "Z1000");
        s.execute("cell.set", json!({"cell": "Z1000", "input": ""})).unwrap();
        x
    }
    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9
    }

    #[test]
    fn a_two_factor_anova_without_replication_reports_rows_columns_and_error() {
        let mut s = session();
        s.execute("range.setValues", json!({"range": "A1", "values": [[1, 2], [2, 4], [5, 6]]})).unwrap();
        s.execute("data.analysisToolPak", json!({"tool": "anovaTwoFactor", "range": "A1:B3", "output": "I1"})).unwrap();
        assert_eq!(value(&s, "I1"), Value::text("Anova: Two-Factor Without Replication"));
        // SUMMARY: rows then columns.
        assert_eq!((value(&s, "I3"), value(&s, "J3"), value(&s, "M3")), (Value::text("SUMMARY"), Value::text("Count"), Value::text("Variance")));
        assert_eq!(
            (value(&s, "I4"), value(&s, "J4"), value(&s, "K4"), num(&s, "L4")),
            (Value::text("Row 1"), Value::number(2.0), Value::number(3.0), 1.5)
        );
        assert!(close(num(&s, "M4"), 0.5));
        assert_eq!((value(&s, "I7"), value(&s, "K7")), (Value::text("Column 1"), Value::number(8.0)));
        assert!(close(num(&s, "L7"), 8.0 / 3.0));
        assert!(close(num(&s, "M7"), 13.0 / 3.0));
        // ANOVA: SS_rows 49/3, SS_columns 8/3, SS_error 1/3, SS_total 58/3; F rows 98, columns 16.
        assert_eq!((value(&s, "I10"), value(&s, "I12")), (Value::text("ANOVA"), Value::text("Rows")));
        assert!(close(num(&s, "J12"), 49.0 / 3.0));
        assert_eq!((num(&s, "K12"), num(&s, "K13"), num(&s, "K14"), num(&s, "K16")), (2.0, 1.0, 2.0, 5.0));
        assert!(close(num(&s, "M12"), 49.0));
        assert!(close(num(&s, "J13"), 8.0 / 3.0));
        assert!(close(num(&s, "M13"), 16.0));
        assert!(close(num(&s, "J14"), 1.0 / 3.0));
        assert!(close(num(&s, "J16"), 58.0 / 3.0));
        // P-values and critical values agree with the worksheet functions.
        assert!(close(num(&s, "N12"), eval(&mut s, "=F.DIST.RT(M12,K12,K14)")));
        assert!(close(num(&s, "O12"), eval(&mut s, "=F.INV.RT(0.05,K12,K14)")));
        assert!(close(num(&s, "N13"), eval(&mut s, "=F.DIST.RT(M13,K13,K14)")));
    }

    #[test]
    fn a_two_factor_anova_without_replication_reads_labels() {
        let mut s = session();
        s.execute("range.setValues", json!({"range": "A20", "values": [["", "B1", "B2"], ["A1", 1, 2], ["A2", 3, 5]]})).unwrap();
        s.execute("data.analysisToolPak", json!({"tool": "anovaTwoFactor", "range": "A20:C22", "labels": true, "output": "A24"})).unwrap();
        assert_eq!((value(&s, "A24"), value(&s, "A26")), (Value::text("Anova: Two-Factor Without Replication"), Value::text("SUMMARY")));
        // The row and column names come from the labels.
        assert_eq!((value(&s, "A27"), value(&s, "A29")), (Value::text("A1"), Value::text("B1")));
        assert_eq!((value(&s, "A32"), value(&s, "A34")), (Value::text("ANOVA"), Value::text("Rows")));
        // SS_rows 6.25, SS_columns 2.25, SS_error 0.25; F rows 25, columns 9.
        assert!(close(num(&s, "B34"), 6.25));
        assert!(close(num(&s, "B35"), 2.25));
        assert!(close(num(&s, "B36"), 0.25));
        assert!(close(num(&s, "E34"), 25.0));
        assert!(close(num(&s, "E35"), 9.0));
        assert!(close(num(&s, "F34"), eval(&mut s, "=F.DIST.RT(E34,C34,C36)")));
        assert!(close(num(&s, "G34"), eval(&mut s, "=F.INV.RT(0.05,C34,C36)")));
    }

    #[test]
    fn a_two_factor_anova_with_replication_reports_sample_columns_interaction_within() {
        let mut s = session();
        s.execute("range.setValues", json!({"range": "A1", "values": [["", "B1", "B2"], ["A1", 1, 3], ["", 2, 4], ["A2", 5, 7], ["", 6, 8]]}))
            .unwrap();
        s.execute("data.analysisToolPak", json!({"tool": "anovaTwoFactorRep", "range": "A1:C5", "rowsPerSample": 2, "labels": true, "output": "G1"}))
            .unwrap();
        assert_eq!(value(&s, "G1"), Value::text("Anova: Two-Factor With Replication"));
        // One SUMMARY block per row level, with the column levels and a Total column.
        assert_eq!(
            (value(&s, "G3"), value(&s, "G4"), value(&s, "H4"), value(&s, "J4")),
            (Value::text("SUMMARY"), Value::text("A1"), Value::text("B1"), Value::text("Total"))
        );
        assert_eq!((value(&s, "G5"), value(&s, "H5"), value(&s, "J5")), (Value::text("Count"), Value::number(2.0), Value::number(4.0)));
        assert_eq!((num(&s, "H6"), num(&s, "J6")), (3.0, 10.0));
        assert_eq!((num(&s, "H7"), num(&s, "J7")), (1.5, 2.5));
        assert!(close(num(&s, "J8"), 5.0 / 3.0));
        assert_eq!(value(&s, "G10"), Value::text("A2"));
        assert_eq!((num(&s, "H11"), num(&s, "H12")), (2.0, 11.0));
        assert_eq!(value(&s, "G17"), Value::text("ANOVA"));
        // Sample SS 32 (df 1, F 64); Columns SS 8 (df 1, F 16); Interaction SS 0; Within SS 2
        // (df 4, MS 0.5); Total SS 42 (df 7).
        assert_eq!((value(&s, "G19"), value(&s, "G20")), (Value::text("Sample"), Value::text("Columns")));
        assert!(close(num(&s, "H19"), 32.0));
        assert_eq!((num(&s, "I19"), num(&s, "J19"), num(&s, "K19")), (1.0, 32.0, 64.0));
        assert!(close(num(&s, "H20"), 8.0));
        assert!(close(num(&s, "K20"), 16.0));
        assert!(close(num(&s, "H21"), 0.0));
        assert_eq!(num(&s, "K21"), 0.0);
        assert!(close(num(&s, "H22"), 2.0));
        assert_eq!((num(&s, "I22"), num(&s, "J22")), (4.0, 0.5));
        assert_eq!((num(&s, "H24"), num(&s, "I24")), (42.0, 7.0));
        assert!(close(num(&s, "L19"), eval(&mut s, "=F.DIST.RT(K19,I19,I22)")));
        assert!(close(num(&s, "M19"), eval(&mut s, "=F.INV.RT(0.05,I19,I22)")));
        assert!(close(num(&s, "L20"), eval(&mut s, "=F.DIST.RT(K20,I20,I22)")));
        assert!(close(num(&s, "L21"), eval(&mut s, "=F.DIST.RT(K21,I21,I22)")));
    }

    #[test]
    fn two_factor_anova_refuses_broken_shapes() {
        fn err(s: &mut Session, p: Json) -> String {
            s.execute("data.analysisToolPak", p).unwrap_err().to_string()
        }
        let mut s = session();
        s.execute("range.setValues", json!({"range": "A1", "values": [[1, 2]]})).unwrap();
        assert!(err(&mut s, json!({"tool": "anovaTwoFactor", "range": "A1:B1", "output": "D1"})).contains("rectangle"));
        s.execute("range.setValues", json!({"range": "A1", "values": [["x", 2], [2, 4]]})).unwrap();
        assert!(err(&mut s, json!({"tool": "anovaTwoFactor", "range": "A1:B2", "output": "D1"})).contains("non-numeric"));
        s.execute("range.setValues", json!({"range": "A1", "values": [[1, 3], [2, 4], [5, 7], [6, 8]]})).unwrap();
        // The replicate count is required and must divide the data rows.
        assert!(err(&mut s, json!({"tool": "anovaTwoFactorRep", "range": "A1:B4", "output": "D1"})).contains("rowsPerSample"));
        assert!(err(&mut s, json!({"tool": "anovaTwoFactorRep", "range": "A1:B4", "rowsPerSample": 3, "output": "D1"})).contains("blocks"));
        assert!(err(&mut s, json!({"tool": "anovaTwoFactorRep", "range": "A1:B4", "rowsPerSample": 1, "output": "D1"})).contains("rowsPerSample"));
        // The tool list names both.
        let tools = s.execute("data.analysisToolPak", json!({})).unwrap();
        let ids: Vec<&str> = tools["tools"].as_array().map(|a| a.iter().filter_map(|t| t["id"].as_str()).collect()).unwrap_or_default();
        assert!(ids.contains(&"anovaTwoFactor") && ids.contains(&"anovaTwoFactorRep"), "{ids:?}");
    }
}
