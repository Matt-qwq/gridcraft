//! Paging reads: where the page file goes, and what a read says when it cannot create one.
//!
//! The model owns the placement policy (directory, name, mode and lifetime); these tests hold the
//! reader to it: a read told to page puts its page file beside the document and still yields the
//! cells a live read yields, and a read with nowhere to spill opens anyway, with one warning.

use std::fmt::Write as _;
use std::path::PathBuf;

use super::minimal;
use crate::{ReadOptions, read_xlsx_file, read_xlsx_file_with};

/// A worksheet body of `rows` rows, two numbers each: enough to fill several 64-row bands.
fn body(rows: u32) -> String {
    let mut body = String::from("<sheetData>");
    for r in 1..=rows {
        let _ = write!(body, r#"<row r="{r}"><c r="A{r}"><v>{r}</v></c><c r="B{r}"><v>{}</v></c></row>"#, f64::from(r) / 2.0);
    }
    body.push_str("</sheetData>");
    body
}

/// A fresh directory under the system temp directory. The clock is not read (it traps on wasm),
/// and a counter plus the pid keeps parallel tests and processes apart.
#[cfg(not(target_arch = "wasm32"))]
fn temp_dir(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nonce = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("gridcraft-xlsx-{tag}-{}-{nonce}", std::process::id()))
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn a_paged_read_puts_the_spill_beside_the_document_and_leaves_nothing() {
    let dir = temp_dir("spill-beside");
    std::fs::create_dir(&dir).expect("a directory for the fixture");
    let path = dir.join("report.xlsx");
    std::fs::write(&path, minimal(&body(200), &[], "", "")).expect("the fixture");
    let (live, _) = read_xlsx_file(&path).expect("a live read");
    let options = ReadOptions { paging: true, spill_dir: Some(dir.clone()), spill_name: Some("report".into()), spill_temp: None };
    let (paged, report) = read_xlsx_file_with(&path, options).expect("a paged read");
    assert!(!report.warnings.iter().any(|w| w.contains("without paging")), "{:?}", report.warnings);
    {
        let sheet = paged.active().expect("a sheet");
        assert_eq!(sheet.cells.page_file_path().and_then(|p| p.parent()), Some(dir.as_path()), "the spill went beside the document");
        let name = sheet.cells.page_file_path().and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or_default();
        assert!(name.starts_with(".gridcraft-spill-report-"), "{name}");
        let live_cells: Vec<_> = live.active().expect("a sheet").cells.iter().collect();
        assert_eq!(sheet.cells.iter().collect::<Vec<_>>(), live_cells, "paging changes no cell");
    }
    drop(paged);
    assert_eq!(std::fs::read_dir(&dir).expect("the fixture's directory").count(), 1, "only the workbook is left");
    std::fs::remove_dir_all(&dir).expect("clean up the fixture's directory");
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn a_paged_read_with_nowhere_to_spill_warns_once_and_keeps_the_cells() {
    let dir = temp_dir("spill-nowhere");
    std::fs::create_dir(&dir).expect("a directory for the fixture");
    let path = dir.join("report.xlsx");
    std::fs::write(&path, minimal(&body(200), &[], "", "")).expect("the fixture");
    let (live, _) = read_xlsx_file(&path).expect("a live read");
    // Both candidates are missing, so no page file can be created anywhere.
    let missing = dir.join("missing");
    let options = ReadOptions { paging: true, spill_dir: Some(missing.clone()), spill_name: Some("report".into()), spill_temp: Some(missing) };
    let (paged, report) = read_xlsx_file_with(&path, options).expect("the workbook still opens");
    let spill_warnings: Vec<&String> = report.warnings.iter().filter(|w| w.contains("opened without paging")).collect();
    assert_eq!(spill_warnings.len(), 1, "{:?}", report.warnings);
    assert!(spill_warnings.first().is_some_and(|w| w.contains("missing")), "{:?}", report.warnings);
    {
        let sheet = paged.active().expect("a sheet");
        assert_eq!(sheet.cells.paged_band_count(), 0, "the store stayed live");
        assert_eq!(sheet.cells.page_file_path(), None);
        let live_cells: Vec<_> = live.active().expect("a sheet").cells.iter().collect();
        assert_eq!(sheet.cells.iter().collect::<Vec<_>>(), live_cells, "no cells were lost with the warning");
    }
    std::fs::remove_dir_all(&dir).expect("clean up the fixture's directory");
}
