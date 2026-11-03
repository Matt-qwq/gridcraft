//! `cargo xtask corpus <dir>`: open every `.xlsx` in a directory through GridCraft, report each
//! workbook's warnings, save and reopen a sample to prove the writer keeps what the reader read,
//! and (when python + openpyxl are installed) read the saved copy back through a foreign parser.
//!
//! The run never writes into the corpus directory: saved copies go to a temporary directory that
//! is removed afterwards. A directory over the caps is refused, not sampled.
//!
//! `--no-openpyxl` skips the foreign readback; `--all` saves and reopens every file (the default is
//! one in five); `--recursive` descends into subdirectories (eight levels at most).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

use gridcraft_engine::core::Value;
use gridcraft_engine::model::Workbook;

/// Most workbooks one run reads, and the most bytes it will read in total.
const MAX_FILES: usize = 2_000;
const MAX_TOTAL_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// A workbook bigger than this is reported and skipped (the engine has its own limits).
const MAX_FILE_BYTES: u64 = 200 * 1024 * 1024;
/// Deepest subdirectory `--recursive` descends.
const MAX_DEPTH: usize = 8;

#[derive(Default)]
struct FileResult {
    name: String,
    size: u64,
    sheets: usize,
    cells: usize,
    warnings: usize,
    /// `Some(Ok(()))` when the file was saved and reopened and the digest matched.
    round_trip: Option<Result<(), String>>,
    /// The openpyxl readback of the saved copy, when it ran.
    foreign: Option<Result<usize, String>>,
    error: Option<String>,
}

impl FileResult {
    fn ok(&self) -> bool {
        self.error.is_none() && self.round_trip.as_ref().is_none_or(|r| r.is_ok())
    }
}

fn main_usage() -> &'static str {
    "usage: cargo xtask corpus <dir> [--recursive] [--all] [--no-openpyxl]"
}

pub fn run(args: &[&str]) -> Result<(), String> {
    let mut dir: Option<&str> = None;
    let mut recursive = false;
    let mut all = false;
    let mut openpyxl = true;
    for a in args {
        match *a {
            "--recursive" => recursive = true,
            "--all" => all = true,
            "--no-openpyxl" => openpyxl = false,
            s if s.starts_with('-') => return Err(format!("unknown flag `{s}`\n{}", main_usage())),
            s if dir.is_none() => dir = Some(s),
            s => return Err(format!("unexpected argument `{s}`\n{}", main_usage())),
        }
    }
    let dir = dir.ok_or_else(|| format!("missing the corpus directory\n{}", main_usage()))?;
    let path = Path::new(dir);
    if !path.is_dir() {
        return Err(format!("{dir} is not a directory"));
    }
    let python = if openpyxl { python_with_openpyxl() } else { None };
    let mut files = Vec::new();
    collect(path, recursive, 0, &mut files)?;
    files.sort();
    if files.is_empty() {
        return Err(format!("{dir} has no .xlsx workbooks (the corpus standard expects .xlsx)"));
    }
    if files.len() > MAX_FILES {
        return Err(format!("{dir} has {} workbooks; the cap is {MAX_FILES} — split the corpus", files.len()));
    }
    let mut total = 0u64;
    for f in &files {
        let size = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
        if size > MAX_FILE_BYTES {
            return Err(format!("{} is {size} bytes, over the {MAX_FILE_BYTES}-byte per-file cap", f.display()));
        }
        total = total.saturating_add(size);
        if total > MAX_TOTAL_BYTES {
            return Err(format!("the directory is over the {MAX_TOTAL_BYTES}-byte cap"));
        }
    }
    let temp = std::env::temp_dir().join(format!("gridcraft-corpus-{}", std::process::id()));
    if openpyxl {
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).map_err(|e| format!("can't create {}: {e}", temp.display()))?;
    }
    println!("corpus: {} workbook(s) in {}\n", files.len(), path.display());
    println!(
        "{:<44} {:>9} {:>6} {:>9} {:>8} {:>11} {:>8}",
        "file",
        "size",
        "sheets",
        "cells",
        "warnings",
        "save/reopen",
        if python.is_some() { "openpyxl" } else { "" }
    );
    let mut results = Vec::new();
    for (i, f) in files.iter().enumerate() {
        let save = all || i % 5 == 0;
        let r = check(f, save, python.as_deref(), &temp);
        print_row(&r);
        results.push(r);
    }
    if openpyxl {
        let _ = std::fs::remove_dir_all(&temp);
    }
    let opened = results.iter().filter(|r| r.error.is_none()).count();
    let failed: Vec<&FileResult> = results.iter().filter(|r| !r.ok()).collect();
    let warnings: usize = results.iter().map(|r| r.warnings).sum();
    let saved = results.iter().filter(|r| r.round_trip.is_some()).count();
    let mismatched = results.iter().filter(|r| r.round_trip.as_ref().is_some_and(|x| x.is_err())).count();
    println!();
    println!(
        "{} file(s): {} opened, {} failed, {} warning(s); {} save/reopen, {} mismatch(es){}",
        results.len(),
        opened,
        results.len() - opened,
        warnings,
        saved,
        mismatched,
        match &python {
            Some(p) => format!("; foreign readback with {p}"),
            None => String::new(),
        }
    );
    for r in failed.iter().take(10) {
        if let Some(e) = &r.error {
            println!("  {}: {e}", r.name);
        }
        if let Some(Err(e)) = &r.round_trip {
            println!("  {}: save/reopen: {e}", r.name);
        }
    }
    if failed.is_empty() { Ok(()) } else { Err(format!("{} workbook(s) failed", failed.len())) }
}

fn collect(dir: &Path, recursive: bool, depth: usize, out: &mut Vec<PathBuf>) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Ok(());
    }
    let entries = std::fs::read_dir(dir).map_err(|e| format!("can't read {}: {e}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('~') || name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            if recursive {
                collect(&path, recursive, depth + 1, out)?;
            }
            continue;
        }
        if name.to_ascii_lowercase().ends_with(".xlsx") {
            if out.len() >= MAX_FILES {
                return Err(format!("more than {MAX_FILES} workbooks; split the corpus"));
            }
            out.push(path);
        }
    }
    Ok(())
}

fn check(path: &Path, save: bool, python: Option<&str>, temp: &Path) -> FileResult {
    let name = path.display().to_string();
    let mut r = FileResult { name: name.clone(), size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0), ..FileResult::default() };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            r.error = Some(format!("read: {e}"));
            return r;
        }
    };
    let wb = match gridcraft_engine::io::open_bytes(&name, &bytes) {
        Ok((wb, warnings)) => {
            r.warnings = warnings.len();
            wb
        }
        Err(e) => {
            r.error = Some(e.to_string());
            return r;
        }
    };
    r.sheets = wb.sheets.len();
    r.cells = wb.sheets.iter().map(|s| s.cells.iter().count()).sum();
    if save {
        r.round_trip = Some(round_trip(&wb, &name, temp, python, &mut r.foreign));
    }
    r
}

/// Save, reopen and compare a digest; the saved copy is also what openpyxl reads.
fn round_trip(wb: &Workbook, name: &str, temp: &Path, python: Option<&str>, foreign: &mut Option<Result<usize, String>>) -> Result<(), String> {
    let before = digest(wb);
    let bytes = gridcraft_engine::io::save_bytes(wb, name).map_err(|e| e.to_string())?;
    let (back, _) = gridcraft_engine::io::open_bytes(name, &bytes).map_err(|e| format!("reopen: {e}"))?;
    let after = digest(&back);
    if let Some(py) = python {
        let file = temp.join(format!("{}.xlsx", sanitize(name)));
        *foreign = Some(match std::fs::write(&file, &bytes) {
            Ok(()) => read_foreign(py, &file),
            Err(e) => Err(format!("write: {e}")),
        });
    }
    if before == after { Ok(()) } else { Err("the saved copy reads back differently".into()) }
}

/// (sheets, cells, hash of names, values, formulas and number formats) — what a save must keep.
fn digest(wb: &Workbook) -> (usize, usize, u64) {
    let mut hasher = DefaultHasher::new();
    for sheet in &wb.sheets {
        sheet.name.hash(&mut hasher);
        for (c, cell) in sheet.cells.iter() {
            (c.row, c.col).hash(&mut hasher);
            value_text(&cell.value).hash(&mut hasher);
            cell.formula.is_some().hash(&mut hasher);
            sheet.style_id(c).hash(&mut hasher);
        }
    }
    (wb.sheets.len(), wb.sheets.iter().map(|s| s.cells.iter().count()).sum(), hasher.finish())
}

fn value_text(v: &Value) -> String {
    match v {
        Value::Number(n) => format!("n{n:?}"),
        Value::Text(t) => format!("t{t}"),
        Value::Bool(b) => format!("b{b}"),
        Value::Error(e) => format!("e{}", e.as_str()),
        Value::Empty => "e".into(),
        Value::Array(_) => value_text(&v.scalar()),
    }
}

fn sanitize(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

/// The installed python with openpyxl, for the foreign readback.
fn python_with_openpyxl() -> Option<String> {
    for candidate in ["python", "python3", "py"] {
        let ok = Command::new(candidate).args(["-c", "import openpyxl"]).output().map(|o| o.status.success()).unwrap_or(false);
        if ok {
            return Some(candidate.to_string());
        }
    }
    None
}

/// How many non-empty cells openpyxl sees in the saved copy.
fn read_foreign(python: &str, file: &Path) -> Result<usize, String> {
    let script = "import sys, openpyxl\nwb = openpyxl.load_workbook(sys.argv[1], data_only=True)\nprint(sum(1 for ws in wb.worksheets for row in ws.iter_rows() for c in row if c.value is not None))";
    let out = Command::new(python).args(["-c", script]).arg(file).output().map_err(|e| format!("can't run {python}: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("openpyxl refused the file").to_string());
    }
    String::from_utf8_lossy(&out.stdout).trim().parse::<usize>().map_err(|e| format!("unexpected openpyxl output: {e}"))
}

fn print_row(r: &FileResult) {
    let round = match &r.round_trip {
        None => "-",
        Some(Ok(())) => "ok",
        Some(Err(_)) => "MISMATCH",
    };
    let foreign = match &r.foreign {
        None => "",
        Some(Ok(n)) => {
            if *n > 0 {
                "ok"
            } else {
                "EMPTY"
            }
        }
        Some(Err(_)) => "FAILED",
    };
    let name = if r.name.len() > 44 { format!("…{}", &r.name[r.name.len().saturating_sub(43)..]) } else { r.name.clone() };
    println!(
        "{:<44} {:>9} {:>6} {:>9} {:>8} {:>11} {:>8}",
        name,
        human(r.size),
        r.sheets,
        r.cells,
        r.warnings,
        if r.error.is_some() { "FAILED" } else { round },
        foreign
    );
}

fn human(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} kB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{check, collect, digest};

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gridcraft-corpus-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sales() -> Vec<u8> {
        let mut s = gridcraft_engine::Session::new();
        s.new_workbook();
        s.execute("file.new", serde_json::json!({"sample": "sales"})).unwrap();
        let wb = (*s.doc().unwrap().wb).clone();
        gridcraft_engine::io::save_bytes(&wb, "sales.xlsx").unwrap()
    }

    #[test]
    fn a_corpus_round_trip_keeps_its_files_and_reopens_its_workbooks() {
        let dir = temp_dir("ok");
        let bytes = sales();
        std::fs::write(dir.join("sales.xlsx"), &bytes).unwrap();
        let mut files = Vec::new();
        collect(&dir, false, 0, &mut files).unwrap();
        assert_eq!(files.len(), 1);
        let before = std::fs::read(&files[0]).unwrap();
        let temp = dir.join("out");
        let r = check(&files[0], true, None, &temp);
        assert!(r.error.is_none(), "{:?}", r.error);
        assert!(r.sheets >= 1 && r.cells > 0);
        assert!(r.round_trip.as_ref().is_some_and(|x| x.is_ok()), "{:?}", r.round_trip);
        // The corpus directory is untouched: no saved copy, the same bytes.
        assert_eq!(std::fs::read(&files[0]).unwrap(), before);
        assert!(!temp.exists() || std::fs::read_dir(&temp).map(|d| d.count()).unwrap_or(0) == 0);
    }

    #[test]
    fn a_broken_workbook_is_reported_not_fatal() {
        let dir = temp_dir("bad");
        std::fs::write(dir.join("broken.xlsx"), b"").unwrap();
        std::fs::write(dir.join("~$locked.xlsx"), b"temporary").unwrap();
        let mut files = Vec::new();
        collect(&dir, false, 0, &mut files).unwrap();
        assert_eq!(files.len(), 1, "the temporary file is skipped");
        let r = check(&files[0], false, None, &dir);
        assert!(r.error.is_some());
        assert!(!r.ok());
        // A digest of the same model does not depend on the run.
        let mut s = gridcraft_engine::Session::new();
        s.new_workbook();
        s.execute("file.new", serde_json::json!({"sample": "sales"})).unwrap();
        let wb = (*s.doc().unwrap().wb).clone();
        assert_eq!(digest(&wb), digest(&wb));
    }
}
