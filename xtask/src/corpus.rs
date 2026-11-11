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
    /// The warning texts (capped): the first ones are worth reading when a run finds a new class.
    warning_texts: Vec<String>,
    /// `Some(Ok(()))` when the file was saved and reopened and the digest matched.
    round_trip: Option<Result<(), String>>,
    /// The openpyxl readback of the saved copy, when it ran.
    foreign: Option<Result<usize, String>>,
    /// The saved copy's file name, when one was written (for `--keep`).
    saved: Option<String>,
    error: Option<String>,
}

impl FileResult {
    fn ok(&self) -> bool {
        self.error.is_none() && self.round_trip.as_ref().is_none_or(|r| r.is_ok())
    }
}

fn main_usage() -> &'static str {
    "usage: cargo xtask corpus <dir> [--recursive] [--all] [--no-openpyxl] [--keep <dir>]"
}

pub fn run(args: &[&str]) -> Result<(), String> {
    let mut dir: Option<&str> = None;
    let mut recursive = false;
    let mut all = false;
    let mut openpyxl = true;
    let mut keep: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args.get(i).copied().unwrap_or_default() {
            "--recursive" => recursive = true,
            "--all" => all = true,
            "--no-openpyxl" => openpyxl = false,
            "--keep" => {
                i += 1;
                let dir = args.get(i).ok_or_else(|| {
                    format!(
                        "--keep needs a directory
{}",
                        main_usage()
                    )
                })?;
                keep = Some(PathBuf::from(dir));
            }
            s if s.starts_with('-') => {
                return Err(format!(
                    "unknown flag `{s}`
{}",
                    main_usage()
                ));
            }
            s if dir.is_none() => dir = Some(s),
            s => {
                return Err(format!(
                    "unexpected argument `{s}`
{}",
                    main_usage()
                ));
            }
        }
        i += 1;
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
    // A synced folder (OneDrive, Baidu Sync and the like) can hold online-only placeholders: the
    // file is listed but reading it fails with "cloud file provider is not running". They are not
    // corpus evidence, so say how many were left out rather than reporting them as broken.
    let before = files.len();
    files.retain(|f| !is_cloud_placeholder(f));
    let cloud = before - files.len();
    if files.is_empty() {
        return Err(format!("{dir} has no readable .xlsx workbooks (the corpus standard expects .xlsx)"));
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
    // Saved copies go to a temporary directory that is removed at the end. `--keep` puts them
    // somewhere the caller can open in Excel instead, and leaves them there.
    let keep_dir = keep.as_ref();
    let temp = match keep_dir {
        Some(dir) => dir.clone(),
        None => std::env::temp_dir().join(format!("gridcraft-corpus-{}", std::process::id())),
    };
    if openpyxl || keep_dir.is_some() {
        if keep_dir.is_none() {
            let _ = std::fs::remove_dir_all(&temp);
        }
        std::fs::create_dir_all(&temp).map_err(|e| format!("can't create {}: {e}", temp.display()))?;
    }
    println!("corpus: {} workbook(s) in {}", files.len(), path.display());
    if cloud > 0 {
        println!("corpus: {cloud} cloud-only file(s) skipped (make them available offline to include them)");
    }
    println!();
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
        let r = check(f, save, python.as_deref(), &temp, i);
        print_row(&r);
        if let Some(error) = &r.error {
            println!("     x {error}");
        }
        for warning in &r.warning_texts {
            println!("     ! {warning}");
        }
        results.push(r);
    }
    if openpyxl && keep_dir.is_none() {
        let _ = std::fs::remove_dir_all(&temp);
    }
    if let Some(dir) = keep_dir {
        let mut manifest = String::from(
            "source	saved
",
        );
        for r in &results {
            if let Some(saved) = &r.saved {
                manifest.push_str(&format!(
                    "{}	{}
",
                    r.name, saved
                ));
            }
        }
        let _ = std::fs::write(dir.join("saved-copies.tsv"), manifest);
        println!();
        println!("saved copies kept in {} (see saved-copies.tsv)", dir.display());
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

fn check(path: &Path, save: bool, python: Option<&str>, temp: &Path, index: usize) -> FileResult {
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
            r.warning_texts = warnings.iter().take(20).cloned().collect();
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
        r.round_trip = Some(round_trip(&wb, &name, temp, python, &mut r.foreign, index, &mut r.saved));
    }
    r
}

/// Save, reopen and compare a digest; the saved copy is also what openpyxl reads.
fn round_trip(
    wb: &Workbook,
    name: &str,
    temp: &Path,
    python: Option<&str>,
    foreign: &mut Option<Result<usize, String>>,
    index: usize,
    saved: &mut Option<String>,
) -> Result<(), String> {
    let before = digest(wb);
    let bytes = gridcraft_engine::io::save_bytes(wb, name).map_err(|e| e.to_string())?;
    let (back, _) = gridcraft_engine::io::open_bytes(name, &bytes).map_err(|e| format!("reopen: {e}"))?;
    let after = digest(&back);
    let file = temp.join(saved_name(index, name));
    if let Err(e) = std::fs::write(&file, &bytes) {
        if let Some(py) = python {
            *foreign = Some(Err(format!("write: {e}")));
        }
    } else {
        *saved = Some(file.display().to_string());
        if let Some(py) = python {
            *foreign = Some(read_foreign(py, &file));
        }
    }
    if before == after { Ok(()) } else { Err("the saved copy reads back differently".into()) }
}

/// What a save must keep: sheet names and state, the cell grid (values, formula text, styles),
/// merges, comments, hyperlinks, validation and conditional-format ranges, tables, defined names,
/// pictures, charts and shapes. Volatile or regenerated details (style ids' definitions, chart part
/// bytes, image bytes) are counted, not hashed byte for byte.
fn digest(wb: &Workbook) -> (usize, usize, u64) {
    let mut hasher = DefaultHasher::new();
    for n in &wb.names {
        (n.name.as_str(), n.scope, n.formula.as_str(), n.hidden).hash(&mut hasher);
    }
    let mut cells = 0usize;
    for sheet in &wb.sheets {
        sheet.name.hash(&mut hasher);
        format!("{:?}", sheet.visibility).hash(&mut hasher);
        sheet.freeze.hash(&mut hasher);
        (sheet.view_top_left.row, sheet.view_top_left.col).hash(&mut hasher);
        let mut merges: Vec<(u32, u32, u32, u32)> = sheet.merges.iter().map(|r| (r.start.row, r.start.col, r.end.row, r.end.col)).collect();
        merges.sort_unstable();
        merges.hash(&mut hasher);
        for (cell, comment) in &sheet.comments {
            (cell.row, cell.col, comment.author.as_str(), comment.text.as_str(), comment.threaded).hash(&mut hasher);
        }
        for (cell, link) in &sheet.hyperlinks {
            (cell.row, cell.col, link.target.as_str(), link.tooltip.as_deref(), link.external).hash(&mut hasher);
        }
        for v in &sheet.validations {
            (v.ranges.len(), format!("{:?}", v.kind), format!("{:?}", v.op), v.f1.as_str(), v.f2.as_deref()).hash(&mut hasher);
            for r in &v.ranges {
                (r.start.row, r.start.col, r.end.row, r.end.col).hash(&mut hasher);
            }
        }
        // Priorities are ranks: the writer renumbers them 1..n on save, which keeps the relative
        // order (what Excel applies) but not the absolute numbers a file happened to use.
        let mut cfs: Vec<&gridcraft_engine::model::CondFormat> = sheet.cond_formats.iter().collect();
        cfs.sort_by_key(|c| c.priority);
        for (rank, cf) in cfs.iter().enumerate() {
            (rank, cf.stop_if_true, cf.ranges.len()).hash(&mut hasher);
            for r in &cf.ranges {
                (r.start.row, r.start.col, r.end.row, r.end.col).hash(&mut hasher);
            }
            format!("{:?}", cf.rule).hash(&mut hasher);
        }
        for t in &sheet.tables {
            (t.name.as_str(), t.range.start.row, t.range.start.col, t.range.end.row, t.range.end.col, t.header_row, t.totals_row).hash(&mut hasher);
            for c in &t.columns {
                (c.name.as_str(), format!("{:?}", c.totals), c.formula.as_deref()).hash(&mut hasher);
            }
        }
        for c in &sheet.charts {
            (format!("{:?}", c.kind), c.title.as_deref(), c.legend as u8, c.series.len()).hash(&mut hasher);
            for s in &c.series {
                (s.values.as_str(), s.categories.as_deref(), s.name.as_deref()).hash(&mut hasher);
            }
        }
        (sheet.images.len(), sheet.shapes.len(), sheet.kept_charts.len()).hash(&mut hasher);
        for s in &sheet.shapes {
            (format!("{:?}", s.kind), s.text.as_str(), s.anchor.cell.row, s.anchor.cell.col).hash(&mut hasher);
        }
        for image in &sheet.images {
            (image.mime.as_str(), image.data.len(), image.anchor.cell.row, image.anchor.cell.col).hash(&mut hasher);
        }
        for (c, cell) in sheet.cells.iter() {
            cells += 1;
            (c.row, c.col).hash(&mut hasher);
            value_text(&cell.value).hash(&mut hasher);
            cell.formula.as_ref().map(|f| f.text.as_str()).hash(&mut hasher);
            sheet.style_id(c).hash(&mut hasher);
        }
    }
    (wb.sheets.len(), cells, hasher.finish())
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

/// A saved copy's file name: the row's index (unique even when two paths sanitize alike) and the
/// end of the sanitized name, long enough to recognise it by eye and short enough for a path limit.
fn saved_name(index: usize, name: &str) -> String {
    let base = sanitize(name);
    let base = base.trim_end_matches("_xlsx");
    let tail: String = base.chars().rev().take(40).collect::<String>().chars().rev().collect();
    format!("{index:04}-{tail}.xlsx")
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

/// The file name a table row shows: the last 43 characters, with an ellipsis when it had to be
/// shortened. Characters, not bytes: a path with Chinese (or any non-ASCII) names must not be cut
/// inside a character.
fn short_name(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= 44 {
        return name.to_string();
    }
    let tail: String = chars[chars.len() - 43..].iter().collect();
    format!("…{tail}")
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
    let name = short_name(&r.name);
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

/// Whether `path` is a cloud-sync placeholder that is not stored locally. Reading one fails while
/// the provider is not running; on other platforms the attribute does not exist.
fn is_cloud_placeholder(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const OFFLINE: u32 = 0x1000;
        const RECALL_ON_DATA_ACCESS: u32 = 0x400000;
        return std::fs::metadata(path).is_ok_and(|m| m.file_attributes() & (OFFLINE | RECALL_ON_DATA_ACCESS) != 0);
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
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

    use super::{check, collect, digest, short_name};

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
        let r = check(&files[0], true, None, &temp, 0);
        assert!(r.error.is_none(), "{:?}", r.error);
        assert!(r.sheets >= 1 && r.cells > 0);
        assert!(r.round_trip.as_ref().is_some_and(|x| x.is_ok()), "{:?}", r.round_trip);
        // The corpus directory is untouched: no saved copy, the same bytes.
        assert_eq!(std::fs::read(&files[0]).unwrap(), before);
        assert!(!temp.exists() || std::fs::read_dir(&temp).map(|d| d.count()).unwrap_or(0) == 0);
    }

    #[test]
    fn saved_names_are_short_and_unique_per_row() {
        let a = super::saved_name(0, "D:/数据/佛山/结果.xlsx");
        let b = super::saved_name(1, "D:/数据/佛山/结果.xlsx");
        assert_ne!(a, b);
        assert!(a.len() < 80, "{a}");
        assert!(a.ends_with(".xlsx"), "{a}");
    }

    #[test]
    fn a_local_file_is_not_a_cloud_placeholder() {
        let dir = temp_dir("cloud");
        let f = dir.join("local.xlsx");
        std::fs::write(&f, b"x").unwrap();
        assert!(!super::is_cloud_placeholder(&f));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn long_non_ascii_names_are_cut_on_a_character_boundary() {
        let name = "D:/百度同步盘/佛山/一个很长的目录名字/另一个很长很长的目录名字/备份数据表格.xlsx";
        let short = short_name(name);
        assert!(short.starts_with('…'), "{short}");
        assert_eq!(short.chars().count(), 44);
        assert!(short.ends_with("备份数据表格.xlsx"), "{short}");
        assert_eq!(short_name("short.xlsx"), "short.xlsx");
    }

    #[test]
    fn a_broken_workbook_is_reported_not_fatal() {
        let dir = temp_dir("bad");
        std::fs::write(dir.join("broken.xlsx"), b"").unwrap();
        std::fs::write(dir.join("~$locked.xlsx"), b"temporary").unwrap();
        let mut files = Vec::new();
        collect(&dir, false, 0, &mut files).unwrap();
        assert_eq!(files.len(), 1, "the temporary file is skipped");
        let r = check(&files[0], false, None, &dir, 0);
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
