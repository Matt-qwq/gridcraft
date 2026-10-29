//! Reading the workbook part and everything hanging off it.

use std::sync::Arc;

use gridcraft_core::{DateSystem, RangeRef};
use gridcraft_model::{CalcMode, CellPicture, DefinedName, PhoneticString, Sheet, ShowObjects, Style, StyleId, StyleTable, Visibility, Workbook};

use crate::ReadOptions;
use crate::package::{Package, Rel};
use crate::sheet_read::{ReadPlan, SpillTarget};
use crate::styles::{StylesIn, read_styles};
use crate::xml::{self, El};
use crate::{IoError, ReadReport};

/// State shared while reading one package.
pub struct Ctx<'a> {
    pub pkg: Package<'a>,
    pub report: ReadReport,
    pub sst: Vec<Arc<str>>,
    /// Phonetic (furigana) data of the shared strings, one entry per base text.
    pub phonetics: Vec<PhoneticString>,
    /// Cell format index → model style.
    pub xf_map: Vec<StyleId>,
    pub dxfs: Vec<Style>,
    pub palette: Vec<u32>,
    pub date_system: DateSystem,
    pub styles: StyleTable,
    /// Threaded-comment person id → display name.
    pub persons: std::collections::HashMap<String, String>,
    pub next_id: u32,
    /// Custom number formats from the styles part, by id.
    pub num_fmts: std::collections::HashMap<u32, String>,
    /// Value metadata blocks, indexed by the worksheet's one-based `vm` attribute.
    pub cell_pictures: Vec<Option<Arc<CellPicture>>>,
    /// x14 conditional-format ids of the current sheet's data bars, with the index of the rule in `cond_formats`.
    pub cf_ext_ids: Vec<(String, usize)>,
    /// Whether the sheet part being read had an `<extLst>`: the pass-through scans the part again
    /// only when it has one.
    pub sheet_ext: bool,
}

impl Ctx<'_> {
    pub fn warn(&mut self, m: impl Into<String>) {
        self.report.warn(m);
    }
    /// Stores the phonetic data read from `xl/sharedStrings.xml`.
    ///
    /// One entry per base text: the writer rebuilds the shared string table from the values the
    /// sheets use and holds each text once (the format's `uniqueCount`), so only one reading can be
    /// written per text. A later `<si>` with a text that is already there is dropped — with a
    /// warning when its data differs, so nothing is lost silently.
    pub fn keep_phonetics(&mut self, list: Vec<PhoneticString>) {
        let mut kept: Vec<PhoneticString> = Vec::new();
        let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut conflicts = 0usize;
        for p in list {
            match index.get(p.text.as_str()) {
                Some(&i) => {
                    if kept.get(i).is_some_and(|k| *k != p) {
                        conflicts += 1;
                    }
                }
                None => {
                    index.insert(p.text.clone(), kept.len());
                    kept.push(p);
                }
            }
        }
        if conflicts > 0 {
            self.warn(format!("shared strings: {conflicts} phonetic readings were dropped because another <si> with the same text already had one"));
        }
        self.phonetics = kept;
    }
    /// A fresh id for a chart, picture, shape or table.
    pub fn next_object_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }
    pub fn style(&self, xf: Option<u32>) -> StyleId {
        xf.and_then(|i| self.xf_map.get(i as usize).copied()).unwrap_or_default()
    }
    /// Reads and parses an optional part: problems other than size limits become warnings.
    pub fn optional_xml(&mut self, part: &str) -> Result<Option<El>, IoError> {
        match self.pkg.read_xml(part) {
            Ok(Some(e)) => Ok(Some(e)),
            Ok(None) => {
                self.warn(format!("missing part {part}"));
                Ok(None)
            }
            Err(IoError::TooLarge(m)) => Err(IoError::TooLarge(m)),
            Err(e) => {
                self.warn(format!("skipped {part}: {e}"));
                Ok(None)
            }
        }
    }
}

pub fn find_rel<'r>(rels: &'r [Rel], kind: &str) -> Option<&'r Rel> {
    rels.iter().find(|r| r.kind == kind)
}

pub fn read_xlsx(bytes: &[u8]) -> Result<(Workbook, ReadReport), IoError> {
    read_xlsx_with(bytes, ReadPlan::default())
}

/// [`read_xlsx`], choosing whether completed bands go to a page file while the sheets are read.
/// The engine passes true for a package large enough that the live model would dominate memory;
/// the plain entry point stays live so tests and small files pay nothing for paging. A workbook
/// opened from bytes has no folder, so its pages go to the temp directory.
pub fn read_xlsx_paged(bytes: &[u8], paging: bool) -> Result<(Workbook, ReadReport), IoError> {
    read_xlsx_with(bytes, ReadPlan { paging, ..ReadPlan::default() })
}

/// [`read_xlsx_with`]'s plan builder for a public [`ReadOptions`].
impl ReadPlan {
    fn from_options(options: ReadOptions) -> ReadPlan {
        let spill = options.paging.then_some(SpillTarget { dir: options.spill_dir, name: options.spill_name, temp: options.spill_temp });
        ReadPlan { paging: options.paging, spill, ..ReadPlan::default() }
    }
}

/// [`read_xlsx`] with the row-reading plan spelled out: tests use it to read one fixture whole, in
/// chunks and streamed, and to hold the three against each other.
pub(crate) fn read_xlsx_with(bytes: &[u8], plan: ReadPlan) -> Result<(Workbook, ReadReport), IoError> {
    read_package(Package::open(bytes)?, plan)
}

/// [`read_xlsx`] from a file, decompressing parts as they are needed instead of holding the file.
pub fn read_xlsx_file(path: &std::path::Path) -> Result<(Workbook, ReadReport), IoError> {
    read_xlsx_file_with(path, ReadOptions::default())
}

/// [`read_xlsx_file`] with the reader allowed to spill completed bands to a page file.
pub fn read_xlsx_file_paged(path: &std::path::Path, paging: bool) -> Result<(Workbook, ReadReport), IoError> {
    read_xlsx_file_with(path, ReadOptions { paging, ..ReadOptions::default() })
}

/// [`read_xlsx_file`] with everything the read may be told: whether to page, and where the page
/// file may live and what it may be called. The engine opens by path through this, so a spill goes
/// beside the document rather than into the system temp directory (which may be a tmpfs, where
/// paging would silently land in RAM).
pub fn read_xlsx_file_with(path: &std::path::Path, options: ReadOptions) -> Result<(Workbook, ReadReport), IoError> {
    let file = std::fs::File::open(path).map_err(|e| IoError::Format(format!("{}: {e}", path.display())))?;
    read_package(Package::open_file(file)?, ReadPlan::from_options(options))
}

fn read_package(pkg: Package<'_>, plan: ReadPlan) -> Result<(Workbook, ReadReport), IoError> {
    let mut cx = Ctx {
        pkg,
        report: ReadReport::default(),
        sst: vec![],
        phonetics: vec![],
        xf_map: vec![],
        dxfs: vec![],
        palette: crate::tables::INDEXED.to_vec(),
        date_system: DateSystem::D1900,
        styles: StyleTable::default(),
        persons: Default::default(),
        next_id: 1,
        num_fmts: Default::default(),
        cell_pictures: vec![],
        cf_ext_ids: vec![],
        sheet_ext: false,
    };
    let root_rels = cx.pkg.rels("")?;
    let wb_part = find_rel(&root_rels, "officeDocument").map(|r| r.target.clone()).unwrap_or_else(|| "xl/workbook.xml".into());
    let wb_xml = match cx.pkg.read_xml(&wb_part)? {
        Some(x) => x,
        None => {
            if cx.pkg.has("xl/workbook.xml") && wb_part != "xl/workbook.xml" {
                cx.pkg.read_xml("xl/workbook.xml")?.ok_or_else(|| IoError::Format("no workbook part".into()))?
            } else if cx.pkg.has("content.xml") {
                return Err(IoError::Format("this is an OpenDocument file, not XLSX".into()));
            } else {
                return Err(IoError::Format("no workbook part".into()));
            }
        }
    };
    if wb_xml.name != "workbook" {
        return Err(IoError::Format(format!("unexpected root element <{}> in workbook part", wb_xml.name)));
    }
    let wb_rels = cx.pkg.rels(&wb_part)?;
    let mut kept = crate::passthrough::KeptReader::new(&mut cx);
    cx.cell_pictures = match crate::richdata::read(&mut cx, &wb_rels) {
        Ok(pictures) => pictures,
        Err(error) => {
            cx.warn(format!("cell-picture data could not be read ({error}); scalar fallbacks were retained"));
            vec![]
        }
    };
    let mut wb = Workbook::new();
    wb.sheets.clear();

    // Styles and theme. Cell checkboxes live in the feature property bags, which styles refer to.
    let mut checkboxes = Vec::new();
    if let Some(r) = find_rel(&wb_rels, "FeaturePropertyBag").cloned()
        && let Some(x) = cx.optional_xml(&r.target)?
    {
        checkboxes = crate::feature_bags::checkbox_complements(&x);
    }
    if let Some(r) = find_rel(&wb_rels, "styles").cloned() {
        if let Some(x) = cx.optional_xml(&r.target)? {
            let StylesIn { xfs, dxfs, palette, named, num_fmts } = read_styles(&x, &checkboxes);
            cx.num_fmts = num_fmts;
            // Cell format 0 is the Normal cell format: everything on it (font, number format,
            // fill, borders, alignment, protection) is what a cell without its own format shows
            // (#1013).
            if let Some(normal) = xfs.first() {
                cx.styles = StyleTable::with_default(normal.clone());
            }
            cx.xf_map = xfs.into_iter().enumerate().map(|(i, s)| if i == 0 { StyleId::DEFAULT } else { cx.styles.intern(s) }).collect();
            cx.dxfs = dxfs;
            cx.palette = palette;
            wb.cell_styles = named;
        }
    } else {
        cx.warn("no styles part");
    }
    if let Some(r) = find_rel(&wb_rels, "theme").cloned()
        && let Some(x) = cx.optional_xml(&r.target)?
    {
        wb.theme = crate::theme::read_theme(&x);
    }
    // Shared strings, streamed item by item. Phonetic (furigana) runs are kept next to the text
    // they annotate, so an open + save does not delete them.
    if let Some(r) = find_rel(&wb_rels, "sharedStrings").cloned() {
        match cx.pkg.read(&r.target)? {
            Some(b) => {
                let mut sst = Vec::new();
                let mut phonetics = Vec::new();
                let mut dropped = 0usize;
                let res = xml::parse_streaming(&b, &["si"], &mut |si| {
                    let text = xml::rich_text(si);
                    let raw = si.kids("rPh").count();
                    let (runs, props) = xml::phonetic_data(si, &text);
                    dropped += raw.saturating_sub(runs.len());
                    if !runs.is_empty() || props.is_some() {
                        phonetics.push(PhoneticString { text: text.clone(), runs, props });
                    }
                    sst.push(Arc::from(text));
                    Ok(())
                });
                if let Err(e) = res {
                    cx.warn(format!("shared strings are damaged ({e}); kept {} strings", sst.len()));
                }
                if dropped > 0 {
                    cx.warn(format!("shared strings: {dropped} phonetic runs were dropped (bad sb/eb or no reading)"));
                }
                cx.sst = sst;
                cx.keep_phonetics(phonetics);
            }
            None => cx.warn(format!("missing part {}", r.target)),
        }
    }

    if let Some(r) = find_rel(&wb_rels, "person").cloned()
        && let Ok(Some(x)) = cx.pkg.read_xml(&r.target)
    {
        for p in x.kids("person") {
            if let (Some(id), Some(n)) = (p.attr("id"), p.attr("displayName")) {
                cx.persons.insert(id.to_string(), n.to_string());
            }
        }
    }

    // Workbook properties.
    if let Some(p) = wb_xml.child("workbookPr") {
        if p.flag("date1904", false) {
            cx.date_system = DateSystem::D1904;
        }
        wb.show_objects = match p.attr("showObjects") {
            Some("none") => ShowObjects::None,
            Some("placeholders") => ShowObjects::Placeholders,
            _ => ShowObjects::All,
        };
    }
    wb.date_system = cx.date_system;
    if let Some(c) = wb_xml.child("calcPr") {
        wb.calc.mode = match c.attr("calcMode") {
            Some("manual") => CalcMode::Manual,
            Some("autoNoTable") => CalcMode::AutomaticExceptTables,
            _ => CalcMode::Automatic,
        };
        wb.calc.iterative = c.flag("iterate", false);
        wb.calc.full_calc_on_load = c.flag("fullCalcOnLoad", false);
        if let Some(n) = c.attr_u32("iterateCount") {
            wb.calc.set_max_iterations(n);
        }
        if let Some(d) = c.attr_f64("iterateDelta") {
            wb.calc.set_max_change(d);
        }
        wb.calc.precision_as_displayed = !c.flag("fullPrecision", true);
    }
    if let Some(p) = wb_xml.child("workbookProtection") {
        wb.protected_structure = p.flag("lockStructure", false);
        wb.structure_password = if wb.protected_structure { crate::sheet_read::read_password(p, "workbook") } else { None };
    }
    let active_tab = wb_xml.path(&["bookViews", "workbookView"]).and_then(|v| v.attr_u32("activeTab")).unwrap_or(0) as usize;

    // Sheets. `file_to_model[i]` maps the i-th <sheet> to a model sheet index.
    let mut file_to_model: Vec<Option<usize>> = vec![];
    let mut pivot_caches = crate::pivot::CachesIn::new(&wb_xml, &wb_rels);
    let sheet_els: Vec<El> = wb_xml.child("sheets").map(|s| s.kids("sheet").cloned().collect()).unwrap_or_default();
    for (i, se) in sheet_els.iter().enumerate() {
        let name = se.attr("name").unwrap_or("").to_string();
        let rel = se.attr("id").and_then(|id| wb_rels.iter().find(|r| r.id == id)).cloned();
        let Some(rel) = rel else {
            cx.warn(format!("sheet \"{name}\" has no part; skipped"));
            file_to_model.push(None);
            continue;
        };
        if rel.kind == "chartsheet" {
            // Kept as it is and written back in place (see `passthrough`); nothing to model.
            file_to_model.push(None);
            continue;
        }
        if rel.kind != "worksheet" {
            cx.warn(format!("sheet \"{name}\" is a {} and was skipped", rel.kind));
            file_to_model.push(None);
            continue;
        }
        let name = unique_name(&wb, &name, i);
        cx.sheet_ext = false;
        let mut sheet = match crate::sheet_read::read_sheet_with(&mut cx, &rel.target, &name, &plan) {
            Ok(s) => s,
            Err(IoError::TooLarge(m)) => return Err(IoError::TooLarge(m)),
            Err(e) => {
                cx.warn(format!("sheet \"{name}\" could not be read: {e}"));
                Sheet::new(name.clone())
            }
        };
        match crate::pivot::read_pivots(&mut cx, &mut pivot_caches, &rel.target, &name) {
            Ok(p) => sheet.pivots = p,
            Err(IoError::TooLarge(m)) => return Err(IoError::TooLarge(m)),
            Err(e) => cx.warn(format!("PivotTables on sheet \"{name}\" could not be read: {e}")),
        }
        sheet.preserved = kept.sheet(&mut cx, &rel.target, &name);
        sheet.visibility = match se.attr("state") {
            Some("hidden") => Visibility::Hidden,
            Some("veryHidden") => Visibility::VeryHidden,
            _ => Visibility::Visible,
        };
        file_to_model.push(Some(wb.sheets.len()));
        wb.sheets.push(Arc::new(sheet));
    }
    if wb.sheets.is_empty() {
        cx.warn("the workbook has no worksheets; added an empty sheet");
        wb.sheets.push(Arc::new(Sheet::new("Sheet1")));
    }
    wb.active_sheet = file_to_model.get(active_tab).copied().flatten().unwrap_or(0);

    // Defined names. Chartex parts refer to their data through hidden `_xlchart.*` names.
    let mut chart_names: Vec<(String, String)> = vec![];
    if let Some(dn) = wb_xml.child("definedNames") {
        for d in dn.kids("definedName") {
            let Some(name) = d.attr("name") else { continue };
            let scope = match d.attr_u32("localSheetId") {
                Some(i) => match file_to_model.get(i as usize).copied().flatten() {
                    Some(m) => Some(m),
                    None => continue,
                },
                None => None,
            };
            let text = d.text.trim();
            let lname = name.to_ascii_lowercase();
            if lname == "_xlnm._filterdatabase" {
                continue;
            }
            if lname.starts_with("_xlchart.") {
                chart_names.push((lname, crate::fmla::from_file(&crate::chart::chart_ref_from_file(text))));
                continue;
            }
            if lname == "_xlnm.print_area" || lname == "_xlnm.print_titles" {
                if let Some(si) = scope
                    && let Some(sheet) = wb.sheet_mut(si)
                {
                    if lname == "_xlnm.print_area" {
                        sheet.print.print_area = parse_print_area(text);
                    } else {
                        let (rows, cols) = parse_print_titles(text);
                        sheet.print.title_rows = rows;
                        sheet.print.title_cols = cols;
                    }
                }
                continue;
            }
            wb.names.push(DefinedName {
                name: name.to_string(),
                scope,
                formula: crate::fmla::from_file(text),
                comment: d.attr("comment").unwrap_or("").to_string(),
                hidden: d.flag("hidden", false),
            });
        }
    }

    if !chart_names.is_empty() {
        let resolve = |f: &mut String| {
            if let Some((_, r)) = chart_names.iter().find(|(n, _)| f.eq_ignore_ascii_case(n)) {
                *f = r.clone();
            }
        };
        for sheet in wb.sheets.iter_mut().filter(|s| !s.charts.is_empty()) {
            for ch in Arc::make_mut(sheet).charts.iter_mut() {
                for s in ch.series.iter_mut() {
                    resolve(&mut s.values);
                    s.categories.iter_mut().for_each(&resolve);
                    s.name.iter_mut().for_each(&resolve);
                }
            }
        }
    }

    // Document properties.
    if let Some(r) = find_rel(&root_rels, "core-properties").cloned()
        && let Ok(Some(x)) = cx.pkg.read_xml(&r.target)
    {
        let get = |n: &str| x.child(n).map(|e| e.text.clone()).unwrap_or_default();
        wb.props.title = get("title");
        wb.props.subject = get("subject");
        wb.props.author = get("creator");
        wb.props.keywords = get("keywords");
        wb.props.description = get("description");
    }
    if let Some(r) = find_rel(&root_rels, "extended-properties").cloned()
        && let Ok(Some(x)) = cx.pkg.read_xml(&r.target)
    {
        wb.props.company = x.child("Company").map(|e| e.text.clone()).unwrap_or_default();
    }

    // A VBA project is carried through unchanged: GridCraft never runs macros, but preserving the
    // binary keeps macro-enabled workbooks from being silently stripped on save.
    if let Some(r) = find_rel(&wb_rels, "vbaProject").cloned() {
        match cx.pkg.read(&r.target) {
            Ok(Some(data)) => {
                wb.vba_project = Some(data.into());
                cx.warn("macros (VBA project) were kept but not executed");
            }
            Ok(None) => cx.warn("macros (VBA project) reference a missing part and were dropped"),
            Err(IoError::TooLarge(m)) => return Err(IoError::TooLarge(m)),
            Err(e) => cx.warn(format!("macros (VBA project) could not be read and were dropped: {e}")),
        }
    }
    wb.preserved = kept.workbook(&mut cx, &wb_part, &wb_xml, &file_to_model);
    // Calls of LAMBDA names are spelled like the names (the parser upper-cases them).
    wb.apply_name_call_case();
    wb.styles = cx.styles;
    wb.phonetics = cx.phonetics;
    Ok((wb, cx.report))
}

pub(crate) fn unique_name(wb: &Workbook, name: &str, i: usize) -> String {
    let mut base: String = name.chars().filter(|c| !matches!(c, ':' | '\\' | '/' | '?' | '*' | '[' | ']')).take(31).collect();
    if base.trim().is_empty() {
        base = format!("Sheet{}", i + 1);
    }
    if wb.sheet_index(&base).is_none() {
        return base;
    }
    (2..).map(|n| format!("{} ({n})", base.chars().take(25).collect::<String>())).find(|n| wb.sheet_index(n).is_none()).unwrap_or(base)
}

/// Strips a sheet qualifier (`'My Sheet'!$A$1` → `$A$1`).
fn strip_sheet(s: &str) -> &str {
    match s.rfind('!') {
        Some(i) => s.get(i + 1..).unwrap_or(""),
        None => s,
    }
}

pub fn parse_print_area(text: &str) -> Option<RangeRef> {
    let first = text.split(',').next()?;
    let r = strip_sheet(first.trim()).replace('$', "");
    RangeRef::parse(&r)
}

pub fn parse_print_titles(text: &str) -> (Option<(u32, u32)>, Option<(u32, u32)>) {
    let mut rows = None;
    let mut cols = None;
    for part in text.split(',') {
        let p = strip_sheet(part.trim()).replace('$', "");
        let Some((a, b)) = p.split_once(':') else { continue };
        if let (Ok(r0), Ok(r1)) = (a.parse::<u32>(), b.parse::<u32>()) {
            if r0 >= 1 && r1 >= r0 && r1 <= gridcraft_core::MAX_ROWS {
                rows = Some((r0 - 1, r1 - 1));
            }
        } else if let (Some(c0), Some(c1)) = (gridcraft_core::letters_to_col(a), gridcraft_core::letters_to_col(b))
            && c1 >= c0
        {
            cols = Some((c0, c1));
        }
    }
    (rows, cols)
}
