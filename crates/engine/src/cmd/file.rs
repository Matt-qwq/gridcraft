//! File menu: new, open, save, close, export, properties, options.

use serde_json::{Value as Json, json};

use super::*;
use crate::DocState;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(noundo "file.new", "New Workbook", ["File"], Some("Cmd+N"), "{sample?: \"budget\"|\"sales\"|\"grades\"}", always, new_workbook),
        cmd!(noundo "file.open", "Open…", ["File"], Some("Cmd+O"), "{path} | {name, base64}, plus {password} for a password-protected workbook and {csvAsText?: bool} to keep every csv/tsv/txt field as text, with no number or date conversion (xlsx, xlsm, xls import, xlsb and ods data import, csv, tsv, txt, json)", always, open),
        cmd!(noundo "file.save", "Save", ["File"], Some("Cmd+S"), "{path?, password?, encryption?} (xlsx by default; .csv/.tsv/.json/.html/.pdf by extension; without `password` the workbook keeps the one it was opened with, and `\"\"` removes it; `encryption` picks `agile` — the default — or `standard` for the AES-128/SHA-1 form Excel 2007 wrote, and without it a workbook opened from a Standard file stays Standard)", has_doc, save),
        cmd!(noundo "file.saveAs", "Save As…", ["File"], Some("Cmd+Shift+S"), "{path, password?, encryption?} (.xlsx, .csv, .tsv, .json, .html, or .pdf: a PDF of the active sheet; a password encrypts an .xlsx, with `encryption`: `agile` — the default — or `standard`)", has_doc, save_as),
        cmd!(query "file.saveBytes", "Encode Workbook", [], None, "{format?: xlsx|csv|tsv|json|html, password?, encryption?: agile|standard} → {base64} (without `encryption` the document's scheme is used)", has_doc, save_bytes),
        cmd!(noundo "file.close", "Close", ["File"], Some("Cmd+W"), "{force?: bool}", has_doc, close),
        cmd!(noundo "file.exportCsv", "Export as CSV", ["File", "Export"], None, "{path, sheet?}", has_doc, |s, p| export(s, p, "csv")),
        cmd!(noundo "file.exportHtml", "Save as Web Page", ["File", "Export"], None, "{path}", has_doc, |s, p| export(s, p, "html")),
        cmd!(
            noundo "file.exportParquet",
            "Export as Parquet",
            ["File", "Export"],
            None,
            "{path, sheet?, header?: bool = true, range?} — the sheet (or the range) as a typed columnar file for DuckDB, pandas and Spark: whole numbers become INT64, other numbers DOUBLE, TRUE/FALSE BOOLEAN, everything else STRING, blanks nulls",
            has_doc,
            export_parquet
        ),
        cmd!("file.properties", "Properties", ["File"], None, "{title?, subject?, author?, company?, keywords?, description?}", has_doc, properties),
        cmd!(
            query "file.options",
            "Options",
            ["File"],
            None,
            "{} → {calculation: {mode, iterative, maxIterations, maxChange}} for the active workbook (the interface opens File › Options; change settings with their own commands, e.g. formulas.calculationOptions, view.theme, app.language.set)",
            always,
            options
        ),
        cmd!(noundo "window.activate", "Switch Window", ["Window"], None, "{index}", has_doc, activate_window),
    ]
}

fn new_workbook(s: &mut Session, p: &Json) -> Result<Json> {
    let i = match str_param(p, "sample") {
        Some(name) => {
            let wb = crate::sample::build(name).ok_or_else(|| bad("file.new", format!("unknown sample `{name}`")))?;
            let title = crate::sample::title(name);
            s.add_document(DocState::new(wb, None, title))
        }
        None => s.new_workbook(),
    };
    Ok(json!({"index": i, "title": s.doc()?.display_title()}))
}

fn open(s: &mut Session, p: &Json) -> Result<Json> {
    // A workbook read straight from its file never comes back as bytes: a package of a gigabyte
    // whose sheets are small then costs the parts it reads instead of a gigabyte of buffer.
    let mut from_file: Option<(Workbook, Vec<String>)> = None;
    let password = str_param(p, "password");
    let (name, bytes) = if let Some(b) = str_param(p, "base64") {
        (str_param(p, "name").unwrap_or("Book.xlsx").to_string(), crate::io::base64_decode(b).ok_or_else(|| bad("file.open", "invalid base64"))?)
    } else if let Some(path) = str_param(p, "path") {
        // Already open? Switch to it.
        if let Some(i) = s.documents().iter().position(|d| d.path.as_deref() == Some(path)) {
            s.set_active(i);
            return Ok(json!({"index": i, "alreadyOpen": true}));
        }
        match crate::io::open_from_path(path, path, password)? {
            Some((wb, warnings)) => {
                from_file = Some((wb, warnings));
                (path.to_string(), Vec::new())
            }
            None => (path.to_string(), crate::io::read_file(path)?),
        }
    } else {
        s.ui_requests.push(crate::UiRequest::Dialog("open".into(), json!({})));
        return ok();
    };
    let encrypted = from_file.is_none() && gridcraft_xlsx::sniff(&bytes) == gridcraft_xlsx::Format::Encrypted;
    let (mut wb, warnings) = match from_file {
        Some((wb, warnings)) => (wb, warnings),
        None => crate::io::open_bytes_with(&name, &bytes, password, bool_param(p, "csvAsText").unwrap_or(false))?,
    };
    wb.source_dir = str_param(p, "path").and_then(crate::io::source_dir);
    // What `CELL("filename")` reports (#1489). A workbook opened from bytes knows its name but
    // never a folder, so it reports `[name]Sheet` with none; an import reports the file it came
    // from, as Excel does, even though Save must still ask where to put the .xlsx.
    wb.source_path = match str_param(p, "path") {
        Some(path) => crate::io::source_path(path),
        None => Some(std::path::PathBuf::from(&name)),
    };
    let imported =
        matches!(crate::io::FileKind::from_path(&name), Some(crate::io::FileKind::Ods | crate::io::FileKind::Xlsb | crate::io::FileKind::Xls))
            || matches!(gridcraft_xlsx::sniff(&bytes), gridcraft_xlsx::Format::Ods | gridcraft_xlsx::Format::Xlsb | gridcraft_xlsx::Format::Xls);
    let title = if imported {
        format!("{} (imported).xlsx", std::path::Path::new(&name).file_stem().and_then(|n| n.to_str()).unwrap_or("Book"))
    } else {
        std::path::Path::new(&name).file_name().and_then(|n| n.to_str()).unwrap_or("Book").to_string()
    };
    // An import has no save target: ordinary Save must ask for a new destination. An encrypted
    // workbook keeps its file (and its password, below), so Save writes the protection back.
    let path = str_param(p, "path").filter(|_| !imported).map(str::to_string);
    // Replace an untouched blank Book1 like Excel does.
    if s.documents().len() == 1
        && s.active().is_some_and(|d| d.path.is_none() && !d.is_dirty() && d.undo.is_empty() && d.wb.sheets.iter().all(|sh| sh.cells.is_empty()))
    {
        s.close_document(0);
    }
    let mut doc = DocState::new(wb, path, title);
    // An encrypted workbook is saved back with the password it was opened with, and with the
    // scheme it was written in: a Standard file stays Standard instead of silently changing form.
    doc.password = if encrypted { password.map(str::to_string) } else { None };
    doc.encryption = if encrypted { gridcraft_xlsx::encryption_scheme(&bytes).unwrap_or_default() } else { Default::default() };
    if crate::io::FileKind::from_path(&name).is_none() && gridcraft_xlsx::sniff(&bytes) == gridcraft_xlsx::Format::Xlsx {
        doc.content_kind = Some(crate::io::FileKind::Xlsx);
    }
    let i = s.add_document(doc);
    // The decrypted-workbook notice is for the info bar, not a message box.
    let shown: Vec<&str> = warnings.iter().map(String::as_str).filter(|w| *w != crate::io::DECRYPTED_NOTICE).collect();
    if !shown.is_empty() {
        let mut message = shown.iter().take(5).copied().collect::<Vec<_>>().join("\n\n");
        if shown.len() > 5 {
            message.push_str(&format!("\n\n{} additional import warnings.", shown.len() - 5));
        }
        s.ui_requests.push(crate::UiRequest::Message(message));
    }
    let d = s.doc()?;
    let out = json!({"index": i, "title": d.display_title(), "sheets": d.wb.sheets.iter().map(|s| s.name.clone()).collect::<Vec<_>>(), "warnings": warnings});
    Ok(out)
}

/// The scheme a password-protected save writes: the `encryption` param (`"agile"` — the default —
/// or `"standard"`), else the one the document's file was written with. An unknown name is an
/// error that lists the two accepted values.
fn scheme_param(cmd: &str, p: &Json, d: &DocState) -> Result<gridcraft_xlsx::EncryptionScheme> {
    use gridcraft_xlsx::EncryptionScheme;
    match str_param(p, "encryption") {
        Some(name) => EncryptionScheme::from_name(name).ok_or_else(|| {
            bad(
                cmd,
                format!("unknown encryption scheme `{name}`: use `{}` (the default) or `{}`", EncryptionScheme::NAMES[0], EncryptionScheme::NAMES[1]),
            )
        }),
        None => Ok(d.encryption),
    }
}

fn save(s: &mut Session, p: &Json) -> Result<Json> {
    let path = match str_param(p, "path").map(str::to_string).or_else(|| s.active().and_then(|d| d.path.clone())) {
        Some(p) => p,
        None => {
            s.ui_requests.push(crate::UiRequest::Dialog("saveAs".into(), json!({})));
            return ok();
        }
    };
    // A PDF is a print of the sheet (Page Setup applies), not a copy of the workbook: the
    // document keeps its file and its unsaved changes.
    if std::path::Path::new(&path).extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
        return super::print::export_pdf(s, &json!({"path": path}));
    }
    let d = s.doc_mut()?;
    // The password the file keeps: a `password` param changes it (`""` removes the protection),
    // and without one the document is saved with the password it was opened with.
    let asked = str_param(p, "password").map(|pw| if pw.is_empty() { None } else { Some(pw.to_string()) });
    let password = asked.clone().unwrap_or_else(|| d.password.clone());
    let scheme = scheme_param("file.save", p, d)?;
    // The document's own file keeps the format its content had when opened; any other path
    // goes by its extension. Relative paths to linked workbooks are rewritten for the new folder.
    let kind = crate::io::FileKind::from_path(&path).or(d.content_kind.filter(|_| d.path.as_deref() == Some(path.as_str())));
    let rebased = crate::io::source_dir(&path).and_then(|f| gridcraft_xlsx::rebase_links(&d.wb, &f));
    let wb = rebased.as_ref().unwrap_or(&d.wb);
    let bytes = match kind {
        Some(kind) => crate::io::save_bytes_as_with(wb, &path, kind, password.as_deref(), scheme)?,
        None => crate::io::save_bytes_with(wb, &path, password.as_deref(), scheme)?,
    };
    crate::io::write_file(&path, &bytes)?;
    // Only a full-fidelity format clears the dirty flag and becomes the document's file.
    if matches!(kind, Some(crate::io::FileKind::Xlsx | crate::io::FileKind::Json)) {
        // The new file is the workbook's own now, so `CELL("filename")` reports it (#1489). Set
        // before the `saved` snapshot, or the workbook would read as dirty the moment it was
        // saved; and only when it changes, so an ordinary Save does not clone the workbook.
        let file = crate::io::source_path(&path);
        if d.wb.source_path != file {
            std::sync::Arc::make_mut(&mut d.wb).source_path = file;
        }
        // Fold away cells no logical row or column can reach before this file becomes the
        // document's baseline. Compaction never changes what a cell holds, so the bytes just
        // written still describe the workbook; the baseline then starts out compacted.
        super::inspect::compact_document(d, None);
        d.saved = d.wb.clone();
        d.path = Some(path.clone());
        d.password = password;
        // The document remembers the scheme it was just written with, so the next ordinary Save
        // does not change the file's form behind the caller's back.
        d.encryption = scheme;
    }
    Ok(json!({"path": path, "bytes": bytes.len()}))
}

fn save_as(s: &mut Session, p: &Json) -> Result<Json> {
    if str_param(p, "path").is_none() {
        s.ui_requests.push(crate::UiRequest::Dialog("saveAs".into(), json!({})));
        return ok();
    }
    // Checked here as well, so a bad `encryption` names `file.saveAs` and not `file.save`.
    scheme_param("file.saveAs", p, s.doc()?)?;
    save(s, p)
}

fn save_bytes(s: &mut Session, p: &Json) -> Result<Json> {
    let fmt = str_param(p, "format").unwrap_or("xlsx");
    let d = s.doc()?;
    let name = format!("{}.{fmt}", std::path::Path::new(&d.display_title()).file_stem().and_then(|x| x.to_str()).unwrap_or("Book"));
    // The file name goes into a PivotChart's link to its PivotTable. A password encrypts an
    // `.xlsx`; any other format refuses one rather than writing it unprotected.
    let asked = str_param(p, "password").map(|pw| if pw.is_empty() { None } else { Some(pw.to_string()) });
    let password = match asked {
        // An explicit `""` removes the protection; no param keeps the document's password.
        Some(asked) => asked,
        None => d.password.clone(),
    };
    let scheme = scheme_param("file.saveBytes", p, d)?;
    let bytes = crate::io::save_bytes_with(&d.wb, &name, password.as_deref(), scheme)?;
    Ok(json!({"name": name, "base64": crate::io::base64_encode(&bytes), "bytes": bytes.len()}))
}

fn close(s: &mut Session, p: &Json) -> Result<Json> {
    let force = bool_param(p, "force").unwrap_or(false);
    let d = s.doc()?;
    if d.is_dirty() && !force {
        s.ui_requests.push(crate::UiRequest::Dialog("saveChanges".into(), json!({"title": d.display_title()})));
        return Ok(json!({"closed": false, "dirty": true}));
    }
    let i = s.active_index();
    s.close_document(i);
    Ok(json!({"closed": true}))
}

fn export(s: &mut Session, p: &Json, ext: &str) -> Result<Json> {
    let path = str_param(p, "path").ok_or_else(|| bad("file.export", "missing `path`"))?.to_string();
    let d = s.doc()?;
    let mut wb = (*d.wb).clone();
    if p.get("sheet").is_some() {
        wb.active_sheet = target_sheet(s, p)?;
    }
    let target = if path.ends_with(&format!(".{ext}")) { path.clone() } else { format!("{path}.{ext}") };
    let bytes = crate::io::save_bytes(&wb, &target)?;
    crate::io::write_file(&target, &bytes)?;
    Ok(json!({"path": target, "bytes": bytes.len()}))
}

/// `file.exportParquet`: the sheet as a Parquet file, typed by what its columns hold.
fn export_parquet(s: &mut Session, p: &Json) -> Result<Json> {
    let path = str_param(p, "path").ok_or_else(|| bad("file.exportParquet", "missing `path`"))?.to_string();
    let sheet = target_sheet(s, p)?;
    let range = match str_param(p, "range") {
        Some(text) => Some(RangeRef::parse(text).ok_or_else(|| bad("file.exportParquet", "`range` is not a range"))?),
        None => None,
    };
    let header = bool_param(p, "header").unwrap_or(true);
    let (bytes, report) = {
        let d = s.doc()?;
        let sh = d.wb.sheet(sheet).ok_or_else(|| bad("file.exportParquet", "no such sheet"))?;
        gridcraft_parquet::export_sheet(sh, gridcraft_parquet::Options { header, range }).map_err(|e| bad("file.exportParquet", e.to_string()))?
    };
    let target = if path.to_ascii_lowercase().ends_with(".parquet") { path } else { format!("{path}.parquet") };
    crate::io::write_file(&target, &bytes)?;
    Ok(json!({
        "path": target,
        "bytes": bytes.len(),
        "rows": report.rows,
        "columns": report.columns.iter().map(|(name, kind)| json!({"name": name, "type": kind.as_str()})).collect::<Vec<_>>(),
    }))
}

fn properties(s: &mut Session, p: &Json) -> Result<Json> {
    edit(s, |cx| {
        let pr = &mut cx.wb.props;
        for (k, slot) in [
            ("title", &mut pr.title),
            ("subject", &mut pr.subject),
            ("author", &mut pr.author),
            ("company", &mut pr.company),
            ("keywords", &mut pr.keywords),
            ("description", &mut pr.description),
        ] {
            if let Some(v) = str_param(p, k) {
                *slot = v.to_string();
            }
        }
        Ok(serde_json::to_value(&cx.wb.props).unwrap_or_default())
    })
}

/// File › Options: reports the workbook-level settings and asks the interface to show its Options
/// page. Nothing changes here; every setting has its own command.
fn options(s: &mut Session, _: &Json) -> Result<Json> {
    s.ui_requests.push(crate::UiRequest::Dialog("options".into(), json!({})));
    let calculation = s.active().map(|d| {
        let c = &d.wb.calc;
        let mode = match c.mode {
            gridcraft_model::CalcMode::Automatic => "automatic",
            gridcraft_model::CalcMode::AutomaticExceptTables => "automaticExceptTables",
            gridcraft_model::CalcMode::Manual => "manual",
        };
        json!({"mode": mode, "iterative": c.iterative, "maxIterations": c.max_iterations, "maxChange": c.max_change})
    });
    Ok(json!({"calculation": calculation}))
}

fn activate_window(s: &mut Session, p: &Json) -> Result<Json> {
    let i = u32_param(p, "index").ok_or_else(|| bad("window.activate", "missing `index`"))? as usize;
    if i >= s.documents().len() {
        return Err(bad("window.activate", "no such window"));
    }
    s.set_active(i);
    ok()
}
