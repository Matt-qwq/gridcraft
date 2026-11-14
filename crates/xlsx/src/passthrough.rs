//! Pass-through of package content GridCraft does not model, so that open → edit → save keeps it.
//!
//! On read, the raw bytes of unmodelled parts are kept with their content type and their
//! relationship graph, per owner: the package root (custom document properties, ribbon
//! customisation, add-in task panes), the workbook (custom XML, links to external workbooks with
//! their `<externalReferences>`, data connections, XML maps, its unmodelled `<extLst>` children and
//! the slicer and timeline caches they or a relationship name), each sheet (query tables, cells
//! mapped to XML elements, form controls, ActiveX controls, OLE objects, the sheet background and
//! printer settings, with the sheet's `<picture>`, `<oleObjects>` and `<controls>` XML, the `r:id`
//! of `<pageSetup>`, the legacy VML shapes those controls need and the pictures of the headers and
//! footers, `&G`, with their `<legacyDrawingHF>`; and the sheet's unmodelled `<extLst>` children,
//! such as a timeline) and the parts GridCraft rewrites (a table's `<extLst>` and the query table
//! attached to it, a PivotTable's `<extLst>` — the slicer cache reference among them — and the
//! `<extLst>` of its cache definition). The workbook also keeps its `<fileVersion>` and the
//! calcFeatures extension, which Excel needs to trust the saved values.
//!
//! On write, kept parts get fresh part names where needed and new relationship ids; the `r:id`s in
//! kept XML are rewritten to match. Kept content is conservative:
//! - Anything whose relationship graph reaches a part GridCraft reads and rewrites (worksheets,
//!   drawings, charts, tables, PivotTables, comments…) is not kept: it would point at content that
//!   no longer exists in the same form. Slicers, slicer caches, timelines and timeline caches are
//!   not rewritten, so their graphs are kept whole — they are never edited (gap #8).
//! - Extensions GridCraft writes itself are never kept from the source: the calc-features
//!   extension, the worksheet's data bars, sparklines and x14 validations, and the PivotTable's own
//!   x14 extensions are written from the model, once.
//! - Content a part GridCraft rewrites owns hangs off that owner's name (`table:<name>`,
//!   `pivot:<name>`, `pivotcache:<name>`), stored in the owning sheet's or workbook's kept content;
//!   its relationships carry the key in their id (see `KS`). A table or PivotTable that is
//!   deleted, renamed or replaced therefore takes its extensions and the parts only they reach
//!   with it, instead of having them attached to another one. An extension whose relationship ids
//!   can't all be resolved is dropped whole.
//! - A slicer or timeline cache that names a PivotTable the workbook no longer has is dropped, with
//!   the slicers and timelines behind it: a filter for a report that is gone is a repair prompt, and
//!   a part nothing relates to is one too.
//! - External relationship targets (the path of a linked workbook) are kept as text and never
//!   opened, followed or refreshed. A relative path to a linked workbook is rewritten when the
//!   workbook is saved into another folder ([`rebase_links`]), so it still names the same file.
//! - Sheet content lives on the sheet: deleting the sheet drops it, Copy Sheet doesn't copy it.
//!   Cell and sheet references inside kept parts (a control's linked cell, a query table's range, a
//!   cell mapped to an XML element, a slicer cache's `pivotCacheId`) are kept as they were, not
//!   adjusted to inserted or deleted rows or reordered sheets.
//! - A PivotTable GridCraft reads but does not compute (a data-model or OLAP cache) is carried
//!   whole: its own part, its cache definition, its records and the data model behind them, byte
//!   for byte, keyed to the report's name. The workbook's data model (`xl/model/*`) is kept with
//!   the workbook on its own, so it survives even every report built on it being deleted. The
//!   model is never read, only carried.
//! - Not kept: chart `<extLst>` children and chart details, and Strict Open XML files (their
//!   namespaces differ from what we write).
//! - Sizes are bounded: per part, per owner and per workbook.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::path::{Component, Path};
use std::sync::Arc;

use gridcraft_model::{PivotTable, Preserved, PreservedPart, PreservedRel, PreservedSheet, PreservedXml, Sheet, Visibility, Workbook};
use quick_xml::events::attributes::Attribute;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::QName;
use quick_xml::{Reader, Writer};

use crate::IoError;
use crate::package::{self, normalize};
use crate::read::Ctx;
use crate::write::{NS_REL, Out, Rels, XML_DECL};
use crate::xml::{self, esc_attr};

/// Most bytes kept for one workbook.
pub const MAX_KEPT_TOTAL: usize = 256 * 1024 * 1024;
/// Largest single kept part.
pub const MAX_KEPT_PART: u64 = 64 * 1024 * 1024;
/// Most parts kept for one workbook.
const MAX_KEPT_PARTS: usize = 4096;
/// Most relationships read from one rels part.
const MAX_RELS: usize = 10_000;
/// Largest relationships, content-types or VML part read here.
const MAX_META: u64 = 16 * 1024 * 1024;
/// Longest chain of relationships followed from an owner.
const MAX_CHAIN: usize = 16;
/// Largest kept XML fragment of a sheet.
const MAX_FRAGMENT: usize = 4 * 1024 * 1024;
/// Deepest element nesting accepted in a kept fragment.
const MAX_FRAGMENT_DEPTH: usize = 256;
/// Most `extLst` children kept for one owner.
const MAX_EXTS: usize = 256;
/// Most bytes of `extLst` children kept for one owner.
const MAX_EXTS_BYTES: usize = 8 * 1024 * 1024;
/// Separator between an owner key and the source relationship id in [`PreservedRel::id`]. XML 1.0
/// forbids it in a part, so it can't come out of a file and can't be confused with a real id.
const KS: char = '\u{1}';
/// The slot of an `extLst` child of the workbook or a worksheet that GridCraft does not model.
const EXT_SLOT: &str = "ext";
/// The slot of the workbook's calc-features extension (written by [`workbook_elements`]).
pub(crate) const CALC_SLOT: &str = "calcFeatures";

#[cfg(test)]
thread_local! {
    /// A smaller kept-bytes allowance for tests on this thread.
    pub(crate) static KEPT_TOTAL_FOR_TEST: std::cell::Cell<usize> = const { std::cell::Cell::new(MAX_KEPT_TOTAL) };
}

fn kept_total() -> usize {
    #[cfg(test)]
    {
        KEPT_TOTAL_FOR_TEST.with(std::cell::Cell::get)
    }
    #[cfg(not(test))]
    {
        MAX_KEPT_TOTAL
    }
}

const NS_REL_STRICT: &str = "http://purl.oclc.org/ooxml/officeDocument/relationships";
const CT_VML: &str = "application/vnd.openxmlformats-officedocument.vmlDrawing";
const CT_SLICER_CACHE: &str = "application/vnd.ms-excel.slicerCache+xml";
const CT_TIMELINE_CACHE: &str = "application/vnd.ms-excel.timelineCache+xml";
const CT_SLICER: &str = "application/vnd.ms-excel.slicer+xml";
const CT_TIMELINE: &str = "application/vnd.ms-excel.timeline+xml";

/// Relationship kinds (last segment of the type) kept from the package root.
const PACKAGE_ROOTS: &[&str] = &["custom-properties", "extensibility", "webextensiontaskpanes"];
/// Relationship kinds kept from the workbook without a workbook element. Slicer and timeline
/// caches are related to the workbook; the extensions naming them are kept as well when the file
/// has them.
const WORKBOOK_ROOTS: &[&str] = &["customXml", "connections", "xmlMaps"];
/// Workbook relationship kinds whose parts are kept even when no XML element names them.
const WORKBOOK_CACHES: &[&str] = &["slicerCache", "timelineCache"];
/// Relationship kinds a table part owns that GridCraft doesn't write (its query table).
const TABLE_OWNED: &[&str] = &["queryTable"];
/// The relationship type that ties a pivot cache definition to the workbook's data model
/// (Power Pivot), and the model part to the workbook when GridCraft keeps it on its own.
const POWER_PIVOT_KINDS: &[&str] = &["powerPivotData"];
/// The relationships inside a PivotTable's kept content. GridCraft writes these parts from the
/// model, so they are normally rewritten; a report it does not compute is carried whole instead,
/// and the walk goes through them to the cache, its records and the data model behind them.
const PIVOT_GRAPH: &[&str] = &["pivotTable", "pivotCacheDefinition", "pivotCacheRecords", "powerPivotData"];
/// Relationship kinds kept from a sheet without a sheet element.
const SHEET_ROOTS: &[&str] = &["queryTable", "tableSingleCells"];
/// Sheet relationship kinds that mean the sheet XML may hold elements worth keeping.
const SHEET_ELEMENT_KINDS: &[&str] = &["oleObject", "ctrlProp", "control", "image", "package", "printerSettings"];
/// Attributes of `<fileVersion>` (CT_FileVersion).
const FILE_VERSION_ATTRS: &[&str] = &["appName", "lastEdited", "lowestEdited", "rupBuild", "codeName"];
/// The workbook extension listing the calculation features the saved values were computed with.
const CALC_FEATURES_URI: &str = "{B58B0392-4F1F-4190-BB64-5DF3571DCE5F}";
const NS_CALC_FEATURES: &str = "http://schemas.microsoft.com/office/spreadsheetml/2018/calcfeatures";
/// Worksheet extensions GridCraft models and writes itself (solid data bars, sparklines, the
/// validations of Excel 2010, and the id that links a data bar to its rule): keeping the source
/// ones too would duplicate them or contradict the model.
const MODELED_SHEET_EXTS: &[&str] =
    &["{78C0D931-6437-407d-A8A1-0F90B4350D7D}", "{05C60535-1F16-4fd2-B633-F4F36F0B64E0}", "{B025F937-C7B1-47D3-B67F-A62EFF666E3E}"];
const MODELED_SHEET_EXT_ELEMENTS: &[&str] = &["conditionalFormattings", "sparklineGroups", "dataValidations"];
/// The PivotTable extensions GridCraft writes itself (the show-values-as rank, `hideValuesRow`).
const MODELED_PIVOT_EXTS: &[&str] = &[crate::pivot::EXT_X14_DATA_FIELD, crate::pivot::EXT_X14_PIVOT_TABLE];

/// Whether a worksheet `<ext>` holds something the model has no room for. Read with the sheet, so
/// the pass-through only reads the part again (it is the biggest part of the file) when there is
/// something to keep there. The list is the one `keep_exts` uses: the same extensions GridCraft
/// writes itself are skipped on the way back.
pub(crate) fn sheet_ext_unknown(ext: &xml::El) -> bool {
    let uri = ext.attr("uri").unwrap_or("");
    if MODELED_SHEET_EXTS.iter().any(|m| m.eq_ignore_ascii_case(uri)) {
        return false;
    }
    ext.children.iter().any(|c| !MODELED_SHEET_EXT_ELEMENTS.contains(&c.name.as_str()))
}
/// Kept sheet elements, in schema order (all after `<legacyDrawing>`, before `<tableParts>`).
const SHEET_SLOTS: &[&str] = &["legacyDrawingHF", "picture", "oleObjects", "controls"];

/// Relationship kinds GridCraft reads and rewrites, or that depend on content it rewrites:
/// kept content whose graph reaches one of them is dropped.
const REWRITTEN: &[&str] = &[
    "officedocument",
    "core-properties",
    "extended-properties",
    "worksheet",
    "chartsheet",
    "dialogsheet",
    "xlmacrosheet",
    "drawing",
    "chart",
    "chartex",
    "chartstyle",
    "chartcolorstyle",
    "chartusershapes",
    "table",
    "pivottable",
    "pivotcachedefinition",
    "pivotcacherecords",
    "comments",
    "vmldrawing",
    "threadedcomment",
    "person",
    "sharedstrings",
    "styles",
    "theme",
    "sheetmetadata",
    "calcchain",
    "vbaproject",
    "rdrichvalue",
    "rdrichvaluestructure",
    "rdrichvaluetypes",
    "rdrichvaluewebimage",
    "rdarray",
    "rdsupportingpropertybag",
    "rdsupportingpropertybagstructure",
    "richvaluerel",
    "powerpivotdata",
];

/// Relationship kinds a chart sheet reaches that are otherwise rewritten: its drawing, chart and
/// their style, colour and shape parts are kept byte for byte (the chart sheet is not modelled).
const CHART_SHEET_GRAPH: &[&str] = &["chartsheet", "drawing", "chart", "chartex", "chartstyle", "chartcolorstyle", "chartusershapes"];

/// Part-name prefixes (lower case) GridCraft's writer never produces: kept parts there keep their
/// original names. Anything else is renamed so it can't collide with written parts.
const OWN_DIRS: &[&str] = &[
    "customxml/",
    "xl/slicers/",
    "xl/slicercaches/",
    "xl/timelines/",
    "xl/timelinecaches/",
    "docprops/custom.xml",
    "customui/",
    "xl/externallinks/",
    "xl/connections.xml",
    "xl/xmlmaps.xml",
    "xl/querytables/",
    "xl/ctrlprops/",
    "xl/embeddings/",
    "xl/activex/",
    "xl/webextensions/",
    "xl/printersettings/",
    "xl/model/",
];

fn kind(rel_type: &str) -> &str {
    rel_type.rsplit('/').next().unwrap_or("")
}

fn is_kind(r: &PreservedRel, kinds: &[&str]) -> bool {
    let k = kind(&r.rel_type);
    kinds.iter().any(|x| x.eq_ignore_ascii_case(k))
}

fn rewritten(r: &PreservedRel) -> bool {
    !r.external && REWRITTEN.contains(&kind(&r.rel_type).to_ascii_lowercase().as_str())
}

pub(crate) fn rels_part_name(part: &str) -> String {
    let (dir, file) = package::split_dir(part);
    if dir.is_empty() { format!("_rels/{file}.rels") } else { format!("{dir}/_rels/{file}.rels") }
}

/// The key of a part GridCraft rewrites whose kept extras hang off its name (a table, PivotTable
/// or pivot cache), or `None` when the name can't be used as one.
fn owner_key(kind: &str, name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() || name.contains(KS) {
        return None;
    }
    Some(format!("{kind}:{}", name.to_lowercase()))
}

/// The key of a table's kept content: its name, which Excel keeps unique in a workbook.
pub fn table_key(name: &str) -> Option<String> {
    owner_key("table", name)
}

/// The key of a PivotTable's kept content (its slicer cache, its extensions).
pub fn pivot_key(name: &str) -> Option<String> {
    owner_key("pivot", name)
}

/// The key of the kept extensions of a PivotTable's cache definition.
pub fn pivot_cache_key(name: &str) -> Option<String> {
    owner_key("pivotcache", name)
}

/// The owner key of a kept relationship of a rewritten part, if it has one.
fn split_owner(id: &str) -> Option<(&str, &str)> {
    id.split_once(KS)
}

/// A kept relationship id of a rewritten part, prefixed with its owner key.
fn prefixed(key: &str, id: &str) -> String {
    format!("{key}{KS}{id}")
}

/// The model's PivotTable for the pivotTable part `part`, when GridCraft reads it but does not
/// compute it (a data-model or OLAP cache). Its parts are carried whole instead of rewritten.
fn read_only_pivot<'a>(cx: &mut Ctx<'_>, part: &str, pivots: &'a [PivotTable]) -> Option<&'a PivotTable> {
    if !pivots.iter().any(PivotTable::is_read_only) {
        return None;
    }
    let bytes = cx.pkg.read_limited(part, MAX_META).ok().flatten()?;
    let name = xml::parse(&bytes).ok()?.attr("name").map(str::to_string)?;
    pivots.iter().find(|p| p.is_read_only() && p.name.eq_ignore_ascii_case(name.trim()))
}

/// A part's relationships with full type URIs. `Ok(vec![])` when it has none; an error when the
/// rels part is damaged or too large (its part then can't be kept faithfully).
fn raw_rels(cx: &mut Ctx<'_>, part: &str) -> Result<Vec<PreservedRel>, String> {
    let name = rels_part_name(part);
    let bytes = match cx.pkg.read_limited(&name, MAX_META) {
        Ok(Some(b)) => b,
        Ok(None) => return Ok(vec![]),
        Err(e) => return Err(e.to_string()),
    };
    let root = xml::parse(&bytes).map_err(|e| format!("{name}: {e}"))?;
    let (dir, _) = package::split_dir(part);
    let mut out = vec![];
    for r in root.kids("Relationship") {
        if out.len() >= MAX_RELS {
            return Err(format!("{name}: too many relationships"));
        }
        let (Some(id), Some(target)) = (r.attr("Id"), r.attr("Target")) else { continue };
        let external = r.attr("TargetMode").is_some_and(|m| m.eq_ignore_ascii_case("External"));
        out.push(PreservedRel {
            id: id.to_string(),
            rel_type: r.attr("Type").unwrap_or("").to_string(),
            target: if external { target.to_string() } else { package::resolve(dir, target) },
            external,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------- reading

/// The root element's `name` attribute and the raw `<ext>` children of a part's `<extLst>`.
struct ExtScan {
    name: Option<String>,
    exts: Vec<String>,
}

/// Reads the `extLst` children of a part that GridCraft rewrites (a table, PivotTable or pivot
/// cache definition), with the root's namespace declarations added to each fragment. `None` when
/// the part is not XML.
fn scan_exts(bytes: &[u8]) -> Option<ExtScan> {
    let data = xml::to_utf8(bytes);
    let text = std::str::from_utf8(&data).ok()?;
    let mut reader = Reader::from_str(text);
    let mut decls: Vec<(String, String)> = vec![];
    let mut out = ExtScan { name: None, exts: vec![] };
    let mut in_root = false;
    let mut total = 0usize;
    loop {
        match reader.read_event().ok()? {
            Event::Eof => break,
            Event::Start(e) if !in_root => {
                in_root = true;
                for a in e.attributes().with_checks(false).flatten() {
                    if a.key.as_ref() == b"name" {
                        out.name = String::from_utf8(a.value.to_vec()).ok();
                    } else if let Some(p) = a.key.as_ref().strip_prefix(b"xmlns:")
                        && let (Ok(p), Ok(v)) = (std::str::from_utf8(p), std::str::from_utf8(&a.value))
                        && !v.contains('"')
                    {
                        decls.push((p.to_string(), v.to_string()));
                    }
                }
            }
            Event::Start(e) => {
                let local = e.local_name().as_ref().to_vec();
                if local == b"extLst" {
                    read_ext_list(&mut reader, text, &decls, &mut out.exts, &mut total)?;
                } else {
                    reader.read_to_end(e.name()).ok()?;
                }
            }
            Event::End(_) => break,
            _ => {}
        }
    }
    Some(out)
}

/// Reads the `<ext>` children of the `<extLst>` the reader is positioned in, appending their raw
/// XML to `out`. Bounded: at most [`MAX_EXTS`] fragments of [`MAX_FRAGMENT`] bytes.
fn read_ext_list(reader: &mut Reader<&[u8]>, text: &str, decls: &[(String, String)], out: &mut Vec<String>, total: &mut usize) -> Option<()> {
    loop {
        let start = usize::try_from(reader.buffer_position()).ok()?;
        match reader.read_event().ok()? {
            Event::Eof => return None,
            Event::Start(e) => {
                let is_ext = e.local_name().as_ref() == b"ext";
                reader.read_to_end(e.name()).ok()?;
                if is_ext {
                    let end = usize::try_from(reader.buffer_position()).ok()?;
                    push_ext(text, start, end, decls, out, total);
                }
            }
            Event::Empty(e) => {
                if e.local_name().as_ref() == b"ext" {
                    let end = usize::try_from(reader.buffer_position()).ok()?;
                    push_ext(text, start, end, decls, out, total);
                }
            }
            Event::End(_) => return Some(()),
            _ => {}
        }
    }
}

fn push_ext(text: &str, start: usize, end: usize, decls: &[(String, String)], out: &mut Vec<String>, total: &mut usize) {
    if out.len() >= MAX_EXTS || end.saturating_sub(start) > MAX_FRAGMENT {
        return;
    }
    let Some(frag) = text.get(start..end) else { return };
    let frag = with_decls(frag, decls);
    if total.saturating_add(frag.len()) > MAX_EXTS_BYTES {
        return;
    }
    *total = total.saturating_add(frag.len());
    out.push(frag);
}

/// Every relationship id a fragment names, in order; `None` when the fragment is damaged or a
/// namespace prefix it uses is undeclared.
fn rel_ids(x: &str) -> Option<Vec<String>> {
    let mut ids = vec![];
    map_rel_ids(x, &mut |id| {
        ids.push(id.to_string());
        Some(id.to_string())
    })
    .map(|_| ids)
}

/// A part GridCraft rewrites, as far as pass-through needs it: its root `name`, its `<extLst>`
/// children and its relationships.
fn read_owner_part(cx: &mut Ctx<'_>, part: &str) -> Option<(String, Vec<String>, Vec<PreservedRel>)> {
    let bytes = cx.pkg.read_limited(part, MAX_META).ok().flatten()?;
    let scan = scan_exts(&bytes)?;
    let rels = raw_rels(cx, part).ok()?;
    Some((scan.name.unwrap_or_default(), scan.exts, rels))
}
/// The `<sheet>` entries of the workbook that are chart sheets, each with the number of worksheets
/// (model sheets) before it.
fn chart_sheet_entries<'a>(wb_xml: &'a xml::El, wb_rels: &[PreservedRel], file_to_model: &[Option<usize>]) -> Vec<(usize, &'a xml::El)> {
    let mut out = vec![];
    let mut worksheets = 0usize;
    for (i, se) in wb_xml.child("sheets").into_iter().flat_map(|s| s.kids("sheet")).enumerate() {
        if file_to_model.get(i).copied().flatten().is_some() {
            worksheets += 1;
        } else if se.attr("id").and_then(|id| wb_rels.iter().find(|r| r.id == id)).is_some_and(|r| is_kind(r, &["chartsheet"])) {
            out.push((worksheets, se));
        }
    }
    out
}

/// Collects kept content while a package is read.
pub struct KeptReader {
    on: bool,
    strict: bool,
    defaults: HashMap<String, String>,
    overrides: HashMap<String, String>,
    budget: usize,
    parts_left: usize,
}

impl KeptReader {
    pub fn new(cx: &mut Ctx<'_>) -> KeptReader {
        let mut k = KeptReader {
            on: true,
            strict: false,
            defaults: HashMap::new(),
            overrides: HashMap::new(),
            budget: kept_total(),
            parts_left: MAX_KEPT_PARTS,
        };
        if let Ok(root) = raw_rels(cx, "")
            && root.iter().any(|r| r.rel_type.starts_with("http://purl.oclc.org/"))
        {
            k.on = false;
            k.strict = true;
        }
        match cx.pkg.read_limited("[Content_Types].xml", MAX_META).ok().flatten().and_then(|b| xml::parse(&b).ok()) {
            Some(types) => {
                for d in types.kids("Default") {
                    if let (Some(e), Some(t)) = (d.attr("Extension"), d.attr("ContentType")) {
                        k.defaults.insert(e.to_ascii_lowercase(), t.to_string());
                    }
                }
                for o in types.kids("Override") {
                    if let (Some(p), Some(t)) = (o.attr("PartName"), o.attr("ContentType")) {
                        k.overrides.insert(normalize(p), t.to_string());
                    }
                }
            }
            None => k.on = false,
        }
        k
    }

    fn content_type(&self, part: &str) -> Option<String> {
        let key = normalize(part);
        if let Some(t) = self.overrides.get(&key) {
            return Some(t.clone());
        }
        let (_, file) = package::split_dir(&key);
        let ext = file.rsplit_once('.').map(|(_, e)| e)?;
        self.defaults.get(ext).cloned()
    }

    /// The parts reachable from `root` that aren't in `seen` yet. Fails (and keeps nothing) when the
    /// graph reaches rewritten content, a missing or damaged part, or a size limit.
    fn closure(&mut self, cx: &mut Ctx<'_>, root: &PreservedRel, seen: &HashSet<String>, through: &[&str]) -> Result<Vec<PreservedPart>, String> {
        if root.external {
            return Ok(vec![]);
        }
        let blocked = |r: &PreservedRel| rewritten(r) && !through.iter().any(|k| k.eq_ignore_ascii_case(kind(&r.rel_type)));
        if blocked(root) {
            return Err(format!("it refers to {}", kind(&root.rel_type)));
        }
        let mut parts = vec![];
        let mut bytes = 0usize;
        let mut local: HashSet<String> = HashSet::new();
        let mut queue = VecDeque::from([(root.target.clone(), 0usize)]);
        while let Some((name, depth)) = queue.pop_front() {
            let key = normalize(&name);
            if seen.contains(&key) || !local.insert(key) {
                continue;
            }
            if depth > MAX_CHAIN {
                return Err("its relationships are nested too deeply".into());
            }
            if parts.len() >= self.parts_left {
                return Err("it has too many parts".into());
            }
            let content_type = self.content_type(&name).ok_or_else(|| format!("{name} has no content type"))?;
            let data = match cx.pkg.read_limited(&name, MAX_KEPT_PART) {
                Ok(Some(d)) => d,
                Ok(None) => return Err(format!("{name} is missing")),
                Err(e) => return Err(e.to_string()),
            };
            bytes = bytes.saturating_add(data.len());
            if bytes > self.budget {
                return Err("it is too large".into());
            }
            let rels = raw_rels(cx, &name)?;
            for r in rels.iter().filter(|r| !r.external) {
                if blocked(r) {
                    return Err(format!("it refers to {}", kind(&r.rel_type)));
                }
                queue.push_back((r.target.clone(), depth + 1));
            }
            parts.push(PreservedPart { name, content_type, data: Arc::from(data), rels });
        }
        self.budget = self.budget.saturating_sub(bytes);
        self.parts_left = self.parts_left.saturating_sub(parts.len());
        Ok(parts)
    }

    /// Keeps a set of relationships (all or nothing) and the parts behind them.
    fn keep(&mut self, cx: &mut Ctx<'_>, p: &mut Preserved, roots: &[PreservedRel], seen: &mut HashSet<String>) -> Result<(), String> {
        self.keep_through(cx, p, roots, seen, &[])
    }

    /// [`Self::keep`] for roots whose graph may reach the rewritten kinds in `through`.
    fn keep_through(
        &mut self,
        cx: &mut Ctx<'_>,
        p: &mut Preserved,
        roots: &[PreservedRel],
        seen: &mut HashSet<String>,
        through: &[&str],
    ) -> Result<(), String> {
        let mut new_parts: Vec<PreservedPart> = vec![];
        let mut scratch = seen.clone();
        for r in roots {
            let parts = self.closure(cx, r, &scratch, through)?;
            scratch.extend(parts.iter().map(|q| normalize(&q.name)));
            new_parts.extend(parts);
        }
        *seen = scratch;
        p.parts.extend(new_parts);
        for r in roots {
            // The same part is often named twice: by an extension and by a root kind (a slicer
            // cache is related to the workbook and listed in its `<extLst>`). One relationship is
            // written, with the id the kept XML uses.
            if !p.rels.iter().any(|x| x.id == r.id || (x.rel_type == r.rel_type && x.target == r.target && x.external == r.external)) {
                p.rels.push(r.clone());
            }
        }
        Ok(())
    }

    /// Keeps a part GridCraft rewrites and what it owns that the model doesn't: the `<extLst>`
    /// children it has (except `modeled`, which GridCraft writes itself), the relationships in
    /// `owned` and everything behind them. Extras hang off the owner key (`owner_key`), so a
    /// renamed or deleted owner drops them instead of attaching them elsewhere. `key_name`
    /// overrides the part's root `name` attribute (a pivot cache definition has none).
    #[allow(clippy::too_many_arguments)]
    fn keep_owner(
        &mut self,
        cx: &mut Ctx<'_>,
        part: &str,
        kind_name: &str,
        key_name: Option<&str>,
        modeled: &[&str],
        owned: &[&str],
        p: &mut Preserved,
    ) -> Option<String> {
        let (name, exts, rels) = match read_owner_part(cx, part) {
            Some(x) => x,
            None => {
                cx.warn(format!("{part}: its unmodelled content could not be read and was dropped"));
                return None;
            }
        };
        let name = key_name.map(str::to_string).unwrap_or(name);
        let Some(key) = owner_key(kind_name, &name) else {
            cx.warn(format!("{part} has no usable name; its unmodelled content was dropped"));
            return None;
        };
        // A fresh set: a slicer cache can hang off two PivotTables (or the workbook too), and
        // each owner must get every part its own extensions name. The writer dedupes by part name.
        let mut seen = HashSet::new();
        self.keep_exts(cx, part, &rels, &exts, modeled, &key, owned, p, &mut seen);
        Some(key)
    }

    /// Keeps the `extLst` children of an owner that GridCraft does not model, and the
    /// relationships they name. `key` is empty for the package's own owners (the workbook and a
    /// worksheet, whose relationships are written as they are) and the owner key for a rewritten
    /// part. A fragment that names a relationship GridCraft can't keep is dropped whole: it would
    /// point at a part the file no longer has.
    #[allow(clippy::too_many_arguments)]
    fn keep_exts(
        &mut self,
        cx: &mut Ctx<'_>,
        owner: &str,
        rels: &[PreservedRel],
        frags: &[String],
        modeled: &[&str],
        key: &str,
        owned: &[&str],
        p: &mut Preserved,
        seen: &mut HashSet<String>,
    ) {
        let mut wanted: Vec<(&str, Vec<String>)> = vec![];
        for frag in frags {
            let Ok(el) = xml::parse(frag.as_bytes()) else {
                cx.warn(format!("{owner}: an extension was dropped: its XML is damaged"));
                continue;
            };
            let uri = el.attr("uri").unwrap_or("");
            let modeled_uri = modeled.iter().any(|m| m.eq_ignore_ascii_case(uri));
            let modeled_child = el.children.iter().any(|c| MODELED_SHEET_EXT_ELEMENTS.contains(&c.name.as_str()));
            if modeled_uri || modeled_child {
                continue;
            }
            let Some(ids) = rel_ids(frag) else {
                cx.warn(format!("{owner}: an extension was dropped: its XML is damaged"));
                continue;
            };
            let keepable = |id: &str| rels.iter().any(|r| r.id == id && !rewritten(r));
            if let Some(bad) = ids.iter().find(|id| !keepable(id)) {
                cx.warn(format!("{owner}: an extension was dropped: it names {bad}, which GridCraft cannot keep"));
                continue;
            }
            wanted.push((frag, ids));
        }
        let mut keep_ids: HashSet<String> = rels.iter().filter(|r| !rewritten(r) && is_kind(r, owned)).map(|r| r.id.clone()).collect();
        for (_, ids) in &wanted {
            keep_ids.extend(ids.iter().cloned());
        }
        // An extension that names no part is kept on its own: it may hold anything.
        if keep_ids.is_empty() && wanted.is_empty() {
            return;
        }
        let mut kept: HashSet<String> = HashSet::new();
        for r in rels.iter().filter(|r| keep_ids.contains(&r.id)) {
            let mut q = Preserved::default();
            match self.keep(cx, &mut q, std::slice::from_ref(r), seen) {
                Ok(()) => {
                    kept.insert(r.id.clone());
                    p.parts.extend(q.parts);
                    let id = if key.is_empty() { r.id.clone() } else { prefixed(key, &r.id) };
                    if !p.rels.iter().any(|x| x.id == id) {
                        p.rels.push(PreservedRel { id, ..r.clone() });
                    }
                }
                Err(why) => cx.warn(format!("{owner}: {} was dropped: {why}", kind(&r.rel_type))),
            }
        }
        for (frag, ids) in wanted {
            if ids.iter().all(|id| kept.contains(id)) {
                p.elements.push(PreservedXml { slot: if key.is_empty() { EXT_SLOT.into() } else { key.to_string() }, xml: frag.to_string() });
            } else {
                cx.warn(format!("{owner}: an extension was dropped: a part it names could not be kept"));
            }
        }
    }

    /// Package-root and workbook-level content.
    pub fn workbook(&mut self, cx: &mut Ctx<'_>, wb_part: &str, wb_xml: &xml::El, file_to_model: &[Option<usize>]) -> Option<Arc<Preserved>> {
        if !self.on {
            let wb_rels = raw_rels(cx, wb_part).unwrap_or_default();
            for (_, se) in chart_sheet_entries(wb_xml, &wb_rels, file_to_model) {
                cx.warn(format!("chart sheet \"{}\" was skipped and is not kept on save", se.attr("name").unwrap_or("")));
            }
            if self.strict {
                cx.warn("unmodelled content of a Strict Open XML workbook (custom XML, links, controls…) is not kept on save");
            }
            return None;
        }
        let mut p = Preserved::default();
        let mut seen = HashSet::new();
        // Package root: custom document properties, ribbon customisation, task panes.
        if let Ok(root) = raw_rels(cx, "") {
            for r in root.iter().filter(|r| is_kind(r, PACKAGE_ROOTS)) {
                let mut q = Preserved::default();
                match self.keep(cx, &mut q, std::slice::from_ref(r), &mut seen) {
                    Ok(()) => {
                        p.parts.extend(q.parts);
                        p.package_rels.extend(q.rels);
                    }
                    Err(why) => cx.warn(format!("{} was dropped: {why}", kind(&r.rel_type))),
                }
            }
        }
        let wb_rels = raw_rels(cx, wb_part).unwrap_or_default();
        for r in wb_rels.iter().filter(|r| is_kind(r, WORKBOOK_ROOTS)) {
            if let Err(why) = self.keep(cx, &mut p, std::slice::from_ref(r), &mut seen) {
                cx.warn(format!("{} part {} was dropped: {why}", kind(&r.rel_type), r.target));
            }
        }
        // Workbook extensions GridCraft does not model (slicer caches, timeline cache references,
        // defined names of extensions…), and the slicer and timeline caches they or a relationship
        // name. Their part bytes, relationships and content types are kept as they are.
        let wb_exts = cx.pkg.read_limited(wb_part, MAX_META).ok().flatten().and_then(|b| scan_exts(&b)).map(|s| s.exts).unwrap_or_default();
        self.keep_exts(cx, wb_part, &wb_rels, &wb_exts, &[CALC_FEATURES_URI], "", WORKBOOK_CACHES, &mut p, &mut seen);
        // The data model (Power Pivot): the parts hold the user's own model — the tables, measures
        // and relationships of Power Pivot — and are worth more than the reports that point at
        // them, so they hang off the workbook instead of a PivotTable. They come back byte for
        // byte even after every report built on them is deleted.
        self.keep_data_model(cx, wb_xml, &wb_rels, &mut p, &mut seen);
        // Chart sheets: the sheet entry (name, state, position) plus the chart sheet part and the
        // drawing, chart, style and colour parts behind it.
        for (worksheets, se) in chart_sheet_entries(wb_xml, &wb_rels, file_to_model) {
            let name = se.attr("name").unwrap_or("").to_string();
            let Some(r) = se.attr("id").and_then(|id| wb_rels.iter().find(|r| r.id == id)) else { continue };
            match self.keep_through(cx, &mut p, std::slice::from_ref(r), &mut seen, CHART_SHEET_GRAPH) {
                Ok(()) => p.sheets.push(PreservedSheet {
                    name,
                    visibility: match se.attr("state") {
                        Some("hidden") => Visibility::Hidden,
                        Some("veryHidden") => Visibility::VeryHidden,
                        _ => Visibility::Visible,
                    },
                    rel_id: r.id.clone(),
                    before: worksheets,
                }),
                Err(why) => cx.warn(format!("chart sheet \"{name}\" was dropped: {why}")),
            }
        }
        // Links to external workbooks: `[1]` in formulas is the position in <externalReferences>,
        // so they are kept all together or not at all.
        let ids: Vec<&str> =
            wb_xml.child("externalReferences").map(|e| e.kids("externalReference").filter_map(|x| x.attr("id")).collect()).unwrap_or_default();
        if !ids.is_empty() {
            let roots: Option<Vec<PreservedRel>> =
                ids.iter().map(|id| wb_rels.iter().find(|r| r.id == *id && is_kind(r, &["externalLink"])).cloned()).collect();
            let kept = match roots {
                Some(roots) => self.keep(cx, &mut p, &roots, &mut seen),
                None => Err("a link has no part".into()),
            };
            match kept {
                Ok(()) => {
                    let mut x = format!("<externalReferences xmlns:r=\"{NS_REL}\">");
                    for id in &ids {
                        let _ = write!(x, "<externalReference r:id=\"{}\"/>", esc_attr(id));
                    }
                    x.push_str("</externalReferences>");
                    p.elements.push(PreservedXml { slot: "externalReferences".into(), xml: x });
                    cx.warn("links to external workbooks were kept; they are not followed or refreshed");
                }
                Err(why) => cx.warn(format!("links to external workbooks were dropped: {why}")),
            }
        }
        // Excel trusts the saved cell values only when both are present; without them it
        // recalculates everything on open, and links into closed workbooks come back as errors.
        if let Some(v) = wb_xml.child("fileVersion") {
            let mut x = String::from("<fileVersion");
            for k in FILE_VERSION_ATTRS {
                if let Some(a) = v.attr(k) {
                    let _ = write!(x, " {k}=\"{}\"", esc_attr(a));
                }
            }
            x.push_str("/>");
            p.elements.push(PreservedXml { slot: "fileVersion".into(), xml: x });
        }
        let calc_features = wb_xml
            .child("extLst")
            .and_then(|l| l.kids("ext").find(|e| e.attr("uri").is_some_and(|u| u.eq_ignore_ascii_case(CALC_FEATURES_URI))))
            .and_then(|e| e.child("calcFeatures"));
        if let Some(c) = calc_features {
            let mut x = format!("<ext uri=\"{CALC_FEATURES_URI}\" xmlns:xcalcf=\"{NS_CALC_FEATURES}\"><xcalcf:calcFeatures>");
            for name in c.kids("feature").filter_map(|f| f.attr("name")) {
                let _ = write!(x, "<xcalcf:feature name=\"{}\"/>", esc_attr(name));
            }
            x.push_str("</xcalcf:calcFeatures></ext>");
            p.elements.push(PreservedXml { slot: CALC_SLOT.into(), xml: x });
        }
        (!p.is_empty()).then(|| Arc::new(p))
    }

    /// The parts of the workbook's data model (Power Pivot), kept with the workbook however the
    /// reports that name them fare. A workbook whose cache definitions relate a `powerPivotData`
    /// part has one; its content is opaque to GridCraft and is never read.
    fn keep_data_model(&mut self, cx: &mut Ctx<'_>, wb_xml: &xml::El, wb_rels: &[PreservedRel], p: &mut Preserved, seen: &mut HashSet<String>) {
        let caches: Vec<String> = wb_xml
            .child("pivotCaches")
            .map(|l| l.kids("pivotCache").filter_map(|c| c.attr("id")).map(str::to_string).collect::<Vec<_>>())
            .unwrap_or_default();
        let mut roots: Vec<PreservedRel> = Vec::new();
        for id in caches {
            let Some(cache) = wb_rels.iter().find(|r| r.id == id && !r.external && is_kind(r, &["pivotCacheDefinition"])) else { continue };
            roots.extend(raw_rels(cx, &cache.target).unwrap_or_default().into_iter().filter(|r| !r.external && is_kind(r, POWER_PIVOT_KINDS)));
        }
        if roots.is_empty() {
            return;
        }
        // One root per part: a model shared by two caches is written once (the writer dedupes).
        let mut seen_parts: HashSet<String> = HashSet::new();
        roots.retain(|r| seen_parts.insert(normalize(&r.target)));
        match self.keep_through(cx, p, &roots, seen, PIVOT_GRAPH) {
            Ok(()) => cx.warn("the workbook's data model was kept; GridCraft carries it and does not read it"),
            Err(why) => cx.warn(format!("the workbook's data model was dropped: {why}")),
        }
    }

    /// Sheet-level content of the sheet part `part`. `pivots` are the PivotTables the model read
    /// from it: a report GridCraft does not compute is carried whole, not rewritten.
    pub fn sheet(&mut self, cx: &mut Ctx<'_>, part: &str, name: &str, pivots: &[PivotTable]) -> Option<Arc<Preserved>> {
        if !self.on {
            return None;
        }
        let rels = match raw_rels(cx, part) {
            Ok(r) => r,
            Err(e) => {
                cx.warn(format!("sheet \"{name}\": relationships could not be read ({e}); unmodelled content was dropped"));
                return None;
            }
        };
        let mut p = Preserved::default();
        let mut seen = HashSet::new();
        for r in rels.iter().filter(|r| is_kind(r, SHEET_ROOTS)) {
            if let Err(why) = self.keep(cx, &mut p, std::slice::from_ref(r), &mut seen) {
                cx.warn(format!("sheet \"{name}\": {} was dropped: {why}", kind(&r.rel_type)));
            }
        }
        // Parts GridCraft rewrites keep what the model has no room for: a table's extensions and
        // the query table attached to it, a PivotTable's slicer cache and the extensions of its
        // cache definition. Each hangs off the owner's name; a renamed or deleted owner drops it.
        for r in rels.iter().filter(|r| is_kind(r, &["table"]) && !r.external) {
            self.keep_owner(cx, &r.target, "table", None, &[], TABLE_OWNED, &mut p);
        }
        for r in rels.iter().filter(|r| is_kind(r, &["pivotTable"]) && !r.external) {
            // A report GridCraft does not compute (a data-model or OLAP cache) is carried whole:
            // its own part, its cache definition, its records and the data model behind them are
            // kept byte for byte, keyed to the report's name so a deleted or renamed report takes
            // them with it. Its cached values stay on the sheet as the cells they already are.
            //
            // The pivot's relationships that name another part (the cache definition, then its
            // records) are followed through the kinds GridCraft normally rewrites: keeping the
            // whole graph is the point, and the graph is never recomputed.
            if let Some(read_only) = read_only_pivot(cx, &r.target, pivots) {
                let root = owner_key("pivot", &read_only.name).map(|key| PreservedRel { id: prefixed(&key, &r.id), ..r.clone() });
                match root.map(|root| self.keep_through(cx, &mut p, std::slice::from_ref(&root), &mut seen, PIVOT_GRAPH)) {
                    Some(Ok(())) => continue,
                    Some(Err(why)) => cx.warn(format!("the read-only PivotTable \"{}\" was dropped: {why}", read_only.name)),
                    None => cx.warn(format!("PivotTable \"{}\" has no usable name; its parts were dropped", read_only.name)),
                }
            }
            let key = self.keep_owner(cx, &r.target, "pivot", None, MODELED_PIVOT_EXTS, &[], &mut p);
            if let Some(key) = key
                && let Some(cache) =
                    raw_rels(cx, &r.target).unwrap_or_default().into_iter().find(|q| is_kind(q, &["pivotCacheDefinition"]) && !q.external)
            {
                let pivot_name = key.split_once(':').map(|(_, n)| n).unwrap_or("");
                self.keep_owner(cx, &cache.target, "pivotcache", Some(pivot_name), &[], &[], &mut p);
            }
        }
        // Notes have one VML drawing per sheet; another one holds the header and footer pictures.
        let vml_drawings = rels.iter().filter(|r| is_kind(r, &["vmlDrawing"])).count();
        let notes = rels.iter().filter(|r| is_kind(r, &["comments"])).count();
        if rels.iter().any(|r| is_kind(r, SHEET_ELEMENT_KINDS)) || vml_drawings > notes || cx.sheet_ext {
            self.sheet_elements(cx, part, name, &rels, &mut p, &mut seen);
        }
        (!p.is_empty()).then(|| Arc::new(p))
    }

    /// A chart sheet's own content that the model has no room for: page margins, page setup, header
    /// and footer, protection and its extensions. Each child is kept as a fragment, its relationship
    /// ids rewritten on save (printer settings, the only part a chart sheet may reference that
    /// GridCraft does not write, are kept with it). The drawing and the chart behind it are
    /// modelled, so they are not kept here: writing them twice would give the package two charts.
    pub fn chart_sheet(&mut self, cx: &mut Ctx<'_>, part: &str, name: &str) -> Option<Arc<Preserved>> {
        if !self.on {
            return None;
        }
        let bytes = match cx.pkg.read_limited(part, MAX_META) {
            Ok(Some(b)) => b,
            Ok(None) => return None,
            Err(IoError::TooLarge(_)) => {
                cx.warn(format!("chart sheet \"{name}\": its page setup and header could not be read (the part is too large) and were dropped"));
                return None;
            }
            Err(e) => {
                cx.warn(format!("chart sheet \"{name}\": its page setup and header could not be read ({e}) and were dropped"));
                return None;
            }
        };
        let children = match scan_chart_sheet(&bytes) {
            Some(c) => c,
            None => {
                cx.warn(format!("chart sheet \"{name}\": its page setup and header could not be read and were dropped"));
                return None;
            }
        };
        let rels = match raw_rels(cx, part) {
            Ok(r) => r,
            Err(e) => {
                cx.warn(format!("chart sheet \"{name}\": its relationships could not be read ({e}); unmodelled content was dropped"));
                return None;
            }
        };
        let mut p = Preserved::default();
        let mut seen = HashSet::new();
        for (tag, xml, ids) in children {
            // The drawing (and the chart it frames) is written from the model, and so is <sheetPr>
            // (tab colour and fit-to-page are model fields).
            if tag == "drawing" || tag == "sheetPr" {
                continue;
            }
            let roots: Option<Vec<PreservedRel>> =
                ids.iter().map(|id| rels.iter().find(|r| r.id == *id && !r.external && is_kind(r, &["printerSettings"])).cloned()).collect();
            let Some(roots) = roots else {
                cx.warn(format!("chart sheet \"{name}\": its <{tag}> names a part GridCraft does not keep, so it was dropped"));
                continue;
            };
            if !roots.is_empty()
                && let Err(why) = self.keep(cx, &mut p, &roots, &mut seen)
            {
                cx.warn(format!("chart sheet \"{name}\": its <{tag}> was dropped: {why}"));
                continue;
            }
            p.elements.push(PreservedXml { slot: format!("{CHART_SHEET_SLOT}{tag}"), xml });
        }
        (!p.is_empty()).then(|| Arc::new(p))
    }

    fn sheet_elements(&mut self, cx: &mut Ctx<'_>, part: &str, name: &str, rels: &[PreservedRel], p: &mut Preserved, seen: &mut HashSet<String>) {
        let scan = match cx.pkg.read(part) {
            Ok(Some(bytes)) => scan_sheet(&bytes),
            _ => None,
        };
        let Some(scan) = scan else {
            cx.warn(format!("sheet \"{name}\": controls and embedded objects could not be read and were dropped"));
            return;
        };
        // The sheet's unmodelled extensions (timelines, filters, anything else): kept with the
        // parts their relationship ids name.
        self.keep_exts(cx, part, rels, &scan.exts, MODELED_SHEET_EXTS, "", &[], p, seen);
        // Printer settings (a binary part per sheet) hang off <pageSetup r:id>.
        if let Some(id) = scan.page_setup.as_deref()
            && let Some(r) = rels.iter().find(|r| r.id == id && is_kind(r, &["printerSettings"]))
        {
            match self.keep(cx, p, std::slice::from_ref(r), seen) {
                Ok(()) => p.elements.push(PreservedXml { slot: "pageSetup".into(), xml: r.id.clone() }),
                Err(why) => cx.warn(format!("sheet \"{name}\": printer settings were dropped: {why}")),
            }
        }
        // Header and footer pictures (`&G`): a VML drawing of their own and the images it shows,
        // kept byte for byte (GridCraft writes the header text, not the pictures).
        if let Some(id) = scan.legacy_drawing_hf.as_deref()
            && scan.legacy_drawing.as_deref() != Some(id)
            && let Some(r) = rels.iter().find(|r| r.id == id && is_kind(r, &["vmlDrawing"]))
        {
            match self.keep_through(cx, p, std::slice::from_ref(r), seen, &["vmlDrawing"]) {
                Ok(()) => {
                    let xml = format!("<legacyDrawingHF xmlns:r=\"{NS_REL}\" r:id=\"{}\"/>", esc_attr(&r.id));
                    p.elements.push(PreservedXml { slot: "legacyDrawingHF".into(), xml });
                    cx.warn(format!("sheet \"{name}\": pictures in the header and footer are not shown; they are kept for saving"));
                }
                Err(why) => cx.warn(format!("sheet \"{name}\": header and footer pictures were dropped: {why}")),
            }
        }
        let mut frags = scan.fragments;
        // Controls and OLE objects are drawn from shapes in the legacy VML drawing.
        if frags.iter().any(|(slot, _)| slot != "picture") {
            match self.vml(cx, rels, scan.legacy_drawing.as_deref(), seen) {
                Ok((vml, parts)) => {
                    p.parts.extend(parts);
                    p.vml = Some(vml);
                }
                Err(why) => {
                    cx.warn(format!("sheet \"{name}\": controls and embedded objects were dropped: {why}"));
                    frags.retain(|(slot, _)| slot == "picture");
                }
            }
        }
        for (slot, x) in frags {
            let mut ids = vec![];
            let ok = map_rel_ids(&x, &mut |id| {
                ids.push(id.to_string());
                Some(id.to_string())
            })
            .is_some();
            let roots: Option<Vec<PreservedRel>> = ids.iter().map(|id| rels.iter().find(|r| r.id == *id).cloned()).collect();
            let kept = match (ok, roots) {
                (true, Some(roots)) => self.keep(cx, p, &roots, seen),
                (false, _) => Err("its XML is damaged".into()),
                (_, None) => Err("it refers to a missing relationship".into()),
            };
            match kept {
                Ok(()) => p.elements.push(PreservedXml { slot, xml: x }),
                Err(why) => cx.warn(format!("sheet \"{name}\": {slot} were dropped: {why}")),
            }
        }
        if p.elements.iter().any(|e| e.slot == "oleObjects" || e.slot == "controls") {
            cx.warn(format!("sheet \"{name}\": embedded objects and form controls are not shown; they are kept for saving"));
        } else {
            p.vml = None;
        }
    }

    /// The sheet's VML drawing without its notes (GridCraft writes those), and the parts its
    /// shapes refer to (preview images).
    fn vml(
        &mut self,
        cx: &mut Ctx<'_>,
        rels: &[PreservedRel],
        rid: Option<&str>,
        seen: &mut HashSet<String>,
    ) -> Result<(PreservedPart, Vec<PreservedPart>), String> {
        let rel = rid.and_then(|id| rels.iter().find(|r| r.id == id && !r.external)).ok_or("the sheet has no VML drawing")?;
        // Charged to the kept total like any other part: many sheets can point at one highly
        // compressed VML drawing, and each keeps its own copy.
        if self.budget == 0 {
            return Err("it is too large".into());
        }
        let limit = MAX_META.min(u64::try_from(self.budget).unwrap_or(u64::MAX));
        let data = match cx.pkg.read_limited(&rel.target, limit) {
            Ok(Some(d)) => d,
            Ok(None) => return Err(format!("{} is missing", rel.target)),
            Err(IoError::TooLarge(_)) => return Err("it is too large".into()),
            Err(e) => return Err(e.to_string()),
        };
        self.budget = self.budget.saturating_sub(data.len());
        let text = String::from_utf8(data).map_err(|_| "the VML drawing is not UTF-8".to_string())?;
        let text = strip_notes(&text);
        let vrels = raw_rels(cx, &rel.target)?;
        let mut q = Preserved::default();
        let roots: Vec<PreservedRel> = vrels.iter().filter(|r| !r.external).cloned().collect();
        self.keep(cx, &mut q, &roots, seen)?;
        let vml = PreservedPart { name: rel.target.clone(), content_type: CT_VML.into(), data: Arc::from(text.into_bytes()), rels: vrels };
        Ok((vml, q.parts))
    }
}

/// What a sheet part holds for pass-through.
struct SheetScan {
    /// (slot, XML with the namespace declarations it needs).
    fragments: Vec<(String, String)>,
    /// The raw `<ext>` children of the sheet's `<extLst>`.
    exts: Vec<String>,
    ext_bytes: usize,
    legacy_drawing: Option<String>,
    /// The relationship id on `<legacyDrawingHF>` (header and footer pictures).
    legacy_drawing_hf: Option<String>,
    /// The relationship id on `<pageSetup>` (printer settings).
    page_setup: Option<String>,
}

/// The slot prefix of a chart sheet's kept children (`chartSheet:pageSetup`), so they cannot be
/// confused with a worksheet's kept sheet elements.
const CHART_SHEET_SLOT: &str = "chartSheet:";

/// One kept child of a chart sheet part: its element name, its XML with the namespace declarations
/// it needs, and the relationship ids it names.
type ChartSheetChild = (String, String, Vec<String>);

/// A chart sheet's top-level children, in document order: everything the model has no room for
/// (page margins, page setup, header and footer, protection, extensions) is written back around the
/// chart the model does keep. `None` when the part is damaged.
fn scan_chart_sheet(bytes: &[u8]) -> Option<Vec<ChartSheetChild>> {
    let data = xml::to_utf8(bytes);
    let text = std::str::from_utf8(&data).ok()?;
    let mut reader = Reader::from_str(text);
    let mut decls: Vec<(String, String)> = vec![];
    let mut out: Vec<ChartSheetChild> = vec![];
    let mut in_root = false;
    loop {
        let start = usize::try_from(reader.buffer_position()).ok()?;
        match reader.read_event().ok()? {
            Event::Eof => break,
            Event::Start(e) if !in_root => {
                in_root = true;
                for a in e.attributes().with_checks(false).flatten() {
                    if let Some(p) = a.key.as_ref().strip_prefix(b"xmlns:")
                        && let (Ok(p), Ok(v)) = (std::str::from_utf8(p), std::str::from_utf8(&a.value))
                        && !v.contains('"')
                    {
                        decls.push((p.to_string(), v.to_string()));
                    }
                }
            }
            Event::Start(e) if in_root => {
                let tag = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                reader.read_to_end(e.name()).ok()?;
                let end = usize::try_from(reader.buffer_position()).ok()?;
                out.push(chart_sheet_child(text, start, end, &decls, tag)?);
            }
            Event::Empty(e) if in_root => {
                let tag = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                let end = usize::try_from(reader.buffer_position()).ok()?;
                out.push(chart_sheet_child(text, start, end, &decls, tag)?);
            }
            _ => {}
        }
    }
    Some(out)
}

fn chart_sheet_child(text: &str, start: usize, end: usize, decls: &[(String, String)], tag: String) -> Option<ChartSheetChild> {
    if end.saturating_sub(start) > MAX_FRAGMENT {
        return None;
    }
    let frag = with_decls(text.get(start..end)?, decls);
    let ids = rel_ids(&frag)?;
    Some((tag, frag, ids))
}

/// Finds the sheet's `<picture>`, `<oleObjects>`, `<controls>` (also inside
/// `<mc:AlternateContent>`) and the `<legacyDrawing>` and `<legacyDrawingHF>` ids. `None` when the XML is damaged.
fn scan_sheet(bytes: &[u8]) -> Option<SheetScan> {
    let data = xml::to_utf8(bytes);
    let text = std::str::from_utf8(&data).ok()?;
    let mut reader = Reader::from_str(text);
    let mut decls: Vec<(String, String)> = vec![];
    let mut out = SheetScan { fragments: vec![], exts: vec![], ext_bytes: 0, legacy_drawing: None, legacy_drawing_hf: None, page_setup: None };
    let mut in_root = false;
    loop {
        let start = usize::try_from(reader.buffer_position()).ok()?;
        match reader.read_event().ok()? {
            Event::Eof => break,
            Event::Start(e) if !in_root => {
                in_root = true;
                for a in e.attributes().with_checks(false).flatten() {
                    if let Some(p) = a.key.as_ref().strip_prefix(b"xmlns:")
                        && let (Ok(p), Ok(v)) = (std::str::from_utf8(p), std::str::from_utf8(&a.value))
                        && !v.contains('"')
                    {
                        decls.push((p.to_string(), v.to_string()));
                    }
                }
            }
            Event::Start(e) => {
                let local = e.local_name().as_ref().to_vec();
                match local.as_slice() {
                    b"pageSetup" => out.page_setup = rel_id_attr(&e),
                    b"legacyDrawing" => out.legacy_drawing = rel_id_attr(&e),
                    b"legacyDrawingHF" => out.legacy_drawing_hf = rel_id_attr(&e),
                    b"extLst" => {
                        read_ext_list(&mut reader, text, &decls, &mut out.exts, &mut out.ext_bytes)?;
                        continue;
                    }
                    _ => {}
                }
                reader.read_to_end(e.name()).ok()?;
                let end = usize::try_from(reader.buffer_position()).ok()?;
                let wanted = matches!(local.as_slice(), b"picture" | b"oleObjects" | b"controls" | b"AlternateContent");
                if wanted && end.saturating_sub(start) <= MAX_FRAGMENT {
                    let frag = text.get(start..end)?;
                    push_fragment(&mut out, frag, &local, &decls);
                }
            }
            Event::Empty(e) if in_root => {
                let local = e.local_name().as_ref().to_vec();
                if local == b"legacyDrawing" || local == b"legacyDrawingHF" || local == b"pageSetup" {
                    let id = rel_id_attr(&e);
                    match local.as_slice() {
                        b"pageSetup" => out.page_setup = id,
                        b"legacyDrawing" => out.legacy_drawing = id,
                        _ => out.legacy_drawing_hf = id,
                    }
                } else if local == b"picture"
                    && let Some(frag) = text.get(start..usize::try_from(reader.buffer_position()).ok()?)
                {
                    push_fragment(&mut out, frag, &local, &decls);
                }
            }
            Event::End(_) => break,
            _ => {}
        }
    }
    Some(out)
}

/// The `r:id` of an element (an `id` attribute with a namespace prefix).
fn rel_id_attr(e: &BytesStart<'_>) -> Option<String> {
    e.attributes()
        .with_checks(false)
        .flatten()
        .find(|a| a.key.local_name().as_ref() == b"id" && a.key.prefix().is_some())
        .and_then(|a| String::from_utf8(a.value.to_vec()).ok())
}

fn push_fragment(out: &mut SheetScan, frag: &str, local: &[u8], decls: &[(String, String)]) {
    let frag = with_decls(frag, decls);
    let slot = if local == b"AlternateContent" {
        // The kept element is the one the first choice holds.
        let Ok(el) = xml::parse(frag.as_bytes()) else { return };
        match el.child("Choice").and_then(|c| c.children.first()) {
            Some(c) => c.name.clone(),
            None => return,
        }
    } else {
        String::from_utf8_lossy(local).into_owned()
    };
    if SHEET_SLOTS.contains(&slot.as_str()) && !out.fragments.iter().any(|(s, _)| *s == slot) {
        out.fragments.push((slot, frag));
    }
}

/// Adds the root's namespace declarations a fragment doesn't declare itself.
fn with_decls(frag: &str, decls: &[(String, String)]) -> String {
    let Some(name_end) = frag.char_indices().skip(1).find(|(_, c)| c.is_whitespace() || *c == '/' || *c == '>').map(|(i, _)| i) else {
        return frag.to_string();
    };
    let head = frag.get(..frag.find('>').unwrap_or(frag.len())).unwrap_or("");
    let mut add = String::new();
    for (p, v) in decls {
        if !head.contains(&format!("xmlns:{p}=")) {
            let _ = write!(add, " xmlns:{p}=\"{v}\"");
        }
    }
    format!("{}{add}{}", frag.get(..name_end).unwrap_or(""), frag.get(name_end..).unwrap_or(""))
}

/// Byte ranges of the `<v:shape …>…</v:shape>` (or self-closed) elements in VML text.
fn shape_spans(vml: &str) -> Vec<(usize, usize)> {
    let mut spans = vec![];
    let mut from = 0;
    while let Some(i) = vml.get(from..).and_then(|s| s.find("<v:shape")).map(|i| i + from) {
        let after = i + "<v:shape".len();
        let is_shape = vml.get(after..).and_then(|s| s.chars().next()).is_some_and(|c| c.is_whitespace() || c == '>' || c == '/');
        if !is_shape {
            from = after;
            continue;
        }
        let Some(tag_end) = vml.get(i..).and_then(|s| s.find('>')).map(|j| i + j + 1) else { break };
        let end = if vml.get(..tag_end).is_some_and(|t| t.ends_with("/>")) {
            tag_end
        } else {
            match vml.get(tag_end..).and_then(|s| s.find("</v:shape>")) {
                Some(j) => tag_end + j + "</v:shape>".len(),
                None => break,
            }
        };
        spans.push((i, end));
        from = end;
    }
    spans
}

/// VML without its note shapes (`ObjectType="Note"`).
fn strip_notes(vml: &str) -> String {
    let mut out = String::with_capacity(vml.len());
    let mut last = 0;
    for (a, b) in shape_spans(vml) {
        let shape = vml.get(a..b).unwrap_or("");
        if shape.contains("ObjectType=\"Note\"") {
            out.push_str(vml.get(last..a).unwrap_or(""));
            last = b;
        }
    }
    out.push_str(vml.get(last..).unwrap_or(""));
    out
}

/// Shape numbers (`_x0000_s1025` → 1025) used in VML text.
fn shape_numbers(vml: &str) -> Vec<u64> {
    vml.match_indices("_x0000_s")
        .filter_map(|(i, m)| {
            let digits: String = vml.get(i + m.len()..)?.chars().take_while(char::is_ascii_digit).take(18).collect();
            digits.parse().ok()
        })
        .collect()
}

/// The highest shape block VML text claims in `<o:idmap data>` or uses in a shape id.
fn vml_block_max(vml: &str) -> u64 {
    let used = shape_numbers(vml).into_iter().max().unwrap_or(0) / 1024;
    let claimed = vml
        .find("<o:idmap")
        .and_then(|i| vml.get(i..)?.find("data=\"").map(|d| i + d + "data=\"".len()))
        .and_then(|d| Some((d, d + vml.get(d..)?.find('"')?)))
        .and_then(|(d, e)| vml.get(d..e))
        .map(|list| list.split(',').filter_map(|b| b.trim().parse::<u64>().ok()).max().unwrap_or(0))
        .unwrap_or(0);
    used.max(claimed)
}

/// Kept VML (controls, OLE objects) plus GridCraft's note shapes: the notes get ids in a fresh
/// shape block so they can't collide with the kept shapes the sheet XML refers to.
fn merge_vml(kept: &str, ours: &str) -> String {
    let Some(close) = kept.rfind("</xml>") else { return ours.to_string() };
    let block = shape_numbers(kept).into_iter().max().unwrap_or(0) / 1024 + 1;
    let mut add = String::new();
    if !kept.contains("id=\"_x0000_t202\"")
        && let Some(i) = ours.find("<v:shapetype")
        && let Some(j) = ours.get(i..).and_then(|s| s.find("</v:shapetype>"))
    {
        add.push_str(ours.get(i..i + j + "</v:shapetype>".len()).unwrap_or(""));
    }
    for (k, (a, b)) in shape_spans(ours).into_iter().enumerate() {
        let shape = ours.get(a..b).unwrap_or("");
        let id = block.saturating_mul(1024).saturating_add(1 + k as u64);
        match (shape.find("id=\"_x0000_s"), shape.find("id=\"_x0000_s").and_then(|i| shape.get(i + 4..)?.find('"').map(|j| i + 4 + j))) {
            (Some(i), Some(j)) => {
                let _ = write!(add, "{}id=\"_x0000_s{id}\"{}", shape.get(..i).unwrap_or(""), shape.get(j + 1..).unwrap_or(""));
            }
            _ => add.push_str(shape),
        }
    }
    let mut head = kept.get(..close).unwrap_or("").to_string();
    // Claim the block in the drawing's id map.
    if let Some(i) = head.find("<o:idmap")
        && let Some(d) = head.get(i..).and_then(|s| s.find("data=\"")).map(|d| i + d + "data=\"".len())
        && let Some(e) = head.get(d..).and_then(|s| s.find('"')).map(|e| d + e)
    {
        head.insert_str(e, &format!(",{block}"));
    }
    format!("{head}{add}{}", kept.get(close..).unwrap_or(""))
}

/// Rewrites relationship-id attributes (`r:id`, `r:embed`…: any attribute in the relationships
/// namespace) of an XML fragment through `map`. `None` when the fragment is damaged or `map` has
/// no id for one of them.
pub fn map_rel_ids(x: &str, map: &mut dyn FnMut(&str) -> Option<String>) -> Option<String> {
    let mut reader = Reader::from_str(x);
    let mut writer = Writer::new(Vec::with_capacity(x.len()));
    let mut prefixes: HashSet<Vec<u8>> = HashSet::new();
    let mut depth = 0usize;
    loop {
        match reader.read_event().ok()? {
            Event::Eof => break,
            Event::Start(e) => {
                depth += 1;
                if depth > MAX_FRAGMENT_DEPTH {
                    return None;
                }
                let e = map_start(&e, &mut prefixes, map)?;
                writer.write_event(Event::Start(e)).ok()?;
            }
            Event::Empty(e) => {
                let e = map_start(&e, &mut prefixes, map)?;
                writer.write_event(Event::Empty(e)).ok()?;
            }
            Event::End(e) => {
                depth = depth.saturating_sub(1);
                writer.write_event(Event::End(e)).ok()?;
            }
            ev => writer.write_event(ev).ok()?,
        }
    }
    String::from_utf8(writer.into_inner()).ok()
}

fn map_start(e: &BytesStart<'_>, prefixes: &mut HashSet<Vec<u8>>, map: &mut dyn FnMut(&str) -> Option<String>) -> Option<BytesStart<'static>> {
    for a in e.attributes().with_checks(false) {
        let a = a.ok()?;
        if let Some(p) = a.key.as_ref().strip_prefix(b"xmlns:")
            && (a.value.as_ref() == NS_REL.as_bytes() || a.value.as_ref() == NS_REL_STRICT.as_bytes())
        {
            prefixes.insert(p.to_vec());
        }
    }
    let name = String::from_utf8(e.name().as_ref().to_vec()).ok()?;
    let mut out = BytesStart::new(name);
    for a in e.attributes().with_checks(false) {
        let a = a.ok()?;
        let key = a.key.as_ref();
        let is_rel = !key.starts_with(b"xmlns") && a.key.prefix().is_some_and(|p| prefixes.contains(p.as_ref()));
        if is_rel {
            let old = std::str::from_utf8(&a.value).ok()?;
            let new = map(old)?;
            out.push_attribute(Attribute { key: QName(key), value: Cow::Owned(esc_attr(&new).into_bytes()) });
        } else {
            out.push_attribute(Attribute { key: QName(key), value: a.value.clone() });
        }
    }
    Some(out)
}

// ---------------------------------------------------------------- writing

/// What the workbook still has, for the check a kept part has to pass: a slicer cache names the
/// PivotTables it filters, a slicer or timeline names its cache.
#[derive(Default)]
struct PartFacts {
    pivots: HashSet<String>,
    caches: HashSet<String>,
    /// Keep every part, whatever it names. Content carried for a PivotTable GridCraft does not
    /// compute is never repaired: the parts are written as they came, and a part dropped behind
    /// one of them would leave the relationship ids its own bytes name pointing at nothing.
    keep_all: bool,
}

/// Names given to kept parts while a package is written.
#[derive(Default)]
pub struct KeptOut {
    used: HashSet<String>,
    n: u32,
    /// Every part written by name (normalized original → written name), across all owners: a part
    /// reached from two owners is written once and related twice.
    names: NameMap,
    /// Part names (normalized) already written, so a part two owners keep is one zip entry.
    written: HashSet<String>,
    /// The PivotTable and cache names the workbook still has (see [`part_usable`]).
    facts: PartFacts,
    /// The highest VML shape block a kept header and footer drawing uses: notes go after it.
    vml_blocks: u32,
    /// Package-root relationships (type, target, external), added when `_rels/.rels` is written.
    root: Vec<(String, String, bool)>,
}

impl KeptOut {
    /// Records the PivotTables and caches the kept content may refer to, so a part that names a
    /// deleted or renamed owner is dropped instead of written pointing at something the workbook
    /// no longer has.
    pub fn set_workbook_facts(&mut self, wb: &Workbook) {
        self.facts.pivots = wb.sheets.iter().flat_map(|s| s.pivots.iter()).map(|p| p.name.to_lowercase()).collect();
        let caches = wb.preserved.iter().map(|p| &**p).chain(wb.sheets.iter().filter_map(|s| s.preserved.as_deref())).flat_map(|p| &p.parts);
        let names: Vec<String> = caches
            .filter(|q| is_cache_part(q) && pivot_names_ok(&self.facts.pivots, q))
            .filter_map(|q| xml::parse(&q.data).ok().and_then(|x| x.attr("name").map(|n| n.to_lowercase())))
            .collect();
        self.facts.caches = names.into_iter().collect();
    }

    /// Reserves the VML shape blocks the kept header and footer drawings of `wb` use, so that the
    /// notes drawings GridCraft writes don't reuse their shape ids.
    pub fn reserve_vml_blocks(&mut self, wb: &Workbook) {
        let vmls =
            wb.sheets.iter().filter_map(|s| s.preserved.as_deref()).flat_map(|p| &p.parts).filter(|q| q.content_type.eq_ignore_ascii_case(CT_VML));
        let max = vmls.map(|q| vml_block_max(&String::from_utf8_lossy(&q.data))).max().unwrap_or(0);
        self.vml_blocks = u32::try_from(max).unwrap_or(u32::MAX);
    }

    /// The shape block of the `n`th notes drawing.
    pub fn notes_block(&self, n: u32) -> u32 {
        n.saturating_add(self.vml_blocks)
    }

    /// Gives the kept part `original` the name `preferred` when nothing has taken it, else the
    /// name [`Self::claim`] would give it. Idempotent: a part reached from two owners keeps the
    /// first name it was given.
    pub fn name_as(&mut self, original: &str, preferred: &str) -> String {
        let key = normalize(original);
        if let Some(n) = self.names.get(&key) {
            return n.clone();
        }
        let name = if self.used.insert(preferred.to_ascii_lowercase()) { preferred.to_string() } else { self.claim(original) };
        self.names.insert(key, name.clone());
        name
    }

    /// A part name for a kept part: its original name when that can't collide with anything
    /// written, otherwise a fresh one in the same folder.
    pub(crate) fn claim(&mut self, original: &str) -> String {
        let lower = original.to_ascii_lowercase();
        let own = OWN_DIRS.iter().any(|d| lower.starts_with(d));
        if own && self.used.insert(lower) {
            return original.to_string();
        }
        let (dir, file) = package::split_dir(original);
        let (stem, ext) = match file.rsplit_once('.') {
            Some((s, e)) => (s, format!(".{e}")),
            None => (file, String::new()),
        };
        let stem = if own { format!("{stem}_") } else { "kept".to_string() };
        loop {
            self.n = self.n.saturating_add(1);
            let file = format!("{stem}{}{ext}", self.n);
            let cand = if dir.is_empty() { file } else { format!("{dir}/{file}") };
            if self.used.insert(cand.to_ascii_lowercase()) || self.n == u32::MAX {
                return cand;
            }
        }
    }
}

/// Original part name (normalized) → written part name.
type NameMap = HashMap<String, String>;

fn rel_target(r: &PreservedRel, names: &NameMap) -> Option<String> {
    if r.external { Some(r.target.clone()) } else { names.get(&normalize(&r.target)).map(|n| format!("/{n}")) }
}

/// A kept part's relationships, ids unchanged, targets renamed.
fn part_rels_xml(rels: &[PreservedRel], names: &NameMap) -> String {
    let mut s = format!("{XML_DECL}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">");
    for r in rels {
        let Some(target) = rel_target(r, names) else { continue };
        let _ = write!(
            s,
            "<Relationship Id=\"{}\" Type=\"{}\" Target=\"{}\"{}/>",
            esc_attr(&r.id),
            esc_attr(&r.rel_type),
            esc_attr(&target),
            if r.external { " TargetMode=\"External\"" } else { "" }
        );
    }
    s.push_str("</Relationships>");
    s
}

fn write_parts(out: &mut Out, parts: &[PreservedPart]) -> NameMap {
    let facts = std::mem::take(&mut out.kept.facts);
    let usable: Vec<&PreservedPart> = parts.iter().filter(|q| part_usable(&facts, q)).collect();
    let mut names = NameMap::new();
    for p in &usable {
        let key = normalize(&p.name);
        let name = match out.kept.names.get(&key) {
            Some(n) => n.clone(),
            None => {
                let n = out.kept.claim(&p.name);
                out.kept.names.insert(key.clone(), n.clone());
                n
            }
        };
        names.insert(key, name);
    }
    let mut written = HashSet::new();
    for p in &usable {
        let Some(new) = names.get(&normalize(&p.name)).cloned() else { continue };
        if !written.insert(new.clone()) || !out.kept.written.insert(new.to_ascii_lowercase()) {
            continue;
        }
        out.part(&new, Some(&p.content_type), p.data.to_vec());
        if !p.rels.is_empty() {
            out.part(&rels_part_name(&new), None, part_rels_xml(&p.rels, &names).into_bytes());
        }
    }
    out.kept.facts = facts;
    names
}

fn is_cache_part(p: &PreservedPart) -> bool {
    p.content_type.eq_ignore_ascii_case(CT_SLICER_CACHE) || p.content_type.eq_ignore_ascii_case(CT_TIMELINE_CACHE)
}

/// Whether a slicer or timeline cache still names the PivotTables the workbook has.
fn pivot_names_ok(pivots: &HashSet<String>, p: &PreservedPart) -> bool {
    let Ok(x) = xml::parse(&p.data) else { return false };
    let names: Vec<&str> = x.child("pivotTables").map(|l| l.kids("pivotTable").filter_map(|q| q.attr("name")).collect()).unwrap_or_default();
    names.iter().all(|n| n.trim().is_empty() || pivots.contains(&n.to_lowercase()))
}

/// Whether a kept part can still be written. A slicer or timeline cache names the PivotTables it
/// filters, and a slicer or timeline names its cache; an owner the workbook no longer has (deleted,
/// renamed) would leave the part pointing at nothing. The part goes, and with it whatever only it
/// reaches — no relationship to a part that isn't written is ever left behind.
fn part_usable(facts: &PartFacts, p: &PreservedPart) -> bool {
    if facts.keep_all {
        return true;
    }
    if is_cache_part(p) {
        return pivot_names_ok(&facts.pivots, p);
    }
    let slicer = p.content_type.eq_ignore_ascii_case(CT_SLICER) || p.content_type.eq_ignore_ascii_case(CT_TIMELINE);
    if !slicer {
        return true;
    }
    let Ok(x) = xml::parse(&p.data) else { return false };
    let names: Vec<&str> = x.children.iter().filter(|c| c.name == "slicer" || c.name == "timeline").filter_map(|c| c.attr("cache")).collect();
    names.iter().all(|n| n.trim().is_empty() || facts.caches.contains(&n.to_lowercase()))
}

/// The parts and relationships [`Keep::sheet`] kept for a PivotTable GridCraft does not compute
/// (its own part, its cache definition, its records, the data model behind them).
pub fn owner_kept(owner: Option<&Preserved>, key: Option<&str>) -> (Vec<PreservedPart>, Vec<PreservedRel>) {
    let (Some(p), Some(key)) = (owner, key) else { return (vec![], vec![]) };
    let (parts, sources, _frags) = owner_parts(p, key, &PartFacts { keep_all: true, ..PartFacts::default() });
    (parts, sources)
}

/// Writes the parts of a PivotTable GridCraft does not compute, under the names `name_of` gives
/// them (the writer's own: a kept report keeps its `pivotTableN.xml` numbering and its cache the
/// id it was written with, so the report's `cacheId` still names its cache, and every part comes
/// back byte for byte). Returns the kept relationships with their targets rewritten to the written
/// parts: the report's own relationship to its cache definition among them — and the name each
/// part was written under (original part name, normalized, to written name). Both are empty when
/// the report kept nothing.
pub fn write_pivot_parts(
    owner: Option<&Preserved>,
    key: Option<&str>,
    out: &mut Out,
    name_of: &dyn Fn(&PreservedPart) -> String,
) -> (Vec<PreservedRel>, NameMap) {
    let (Some(p), Some(key)) = (owner, key) else { return (vec![], NameMap::new()) };
    // Nothing behind the report is dropped: its parts come back as they came.
    let keep_all = std::mem::replace(&mut out.kept.facts.keep_all, true);
    let (parts, sources, _frags) = owner_parts(p, key, &out.kept.facts);
    for q in &parts {
        out.kept.name_as(&q.name, &name_of(q));
    }
    let names = write_parts(out, &parts);
    out.kept.facts.keep_all = keep_all;
    let rels = sources
        .iter()
        .filter_map(|r| {
            let written = names.get(&normalize(&r.target))?.clone();
            Some(PreservedRel { id: r.id.clone(), rel_type: r.rel_type.clone(), target: written, external: r.external })
        })
        .collect();
    (rels, names)
}

/// Adds the owner's relationships; returns old id → new id.
fn attach(rels: &mut Rels, list: &[PreservedRel], names: &NameMap) -> HashMap<String, String> {
    let mut ids = HashMap::new();
    for r in list {
        if let Some(target) = rel_target(r, names) {
            let id = rels.add_raw(&r.rel_type, &target, r.external);
            ids.insert(r.id.clone(), id);
        }
    }
    ids
}

fn remap(x: &str, ids: &HashMap<String, String>) -> Option<String> {
    map_rel_ids(x, &mut |id| ids.get(id).cloned())
}

/// A kept chart sheet being written: its `<sheet>` entry's fields and its new relationship id.
pub struct KeptSheet {
    before: usize,
    name: String,
    visibility: Visibility,
    rid: String,
}

/// The workbook's kept content as written.
#[derive(Default)]
pub struct WorkbookKept {
    /// Workbook XML elements that go right after `<sheets>` (`<externalReferences>`).
    pub elements: String,
    /// The workbook `<ext>` children GridCraft does not model, relationship ids rewritten.
    pub ext: String,
    sheets: Vec<KeptSheet>,
}

impl WorkbookKept {
    /// Where the model's sheet `i` is in the file's sheet list: chart sheets come between worksheets.
    pub fn file_index(&self, i: usize) -> usize {
        i + self.sheets.iter().filter(|s| s.before <= i).count()
    }

    /// The `<sheet>` entries of the chart sheets that go right before the model's sheet `i`
    /// (`i` = the number of sheets for those that come last); `id` numbers them from `first_id`.
    pub fn sheet_entries(&self, i: usize, last: bool, mut id: usize) -> (String, usize) {
        let mut x = String::new();
        for s in self.sheets.iter().filter(|s| if last { s.before >= i } else { s.before == i }) {
            let state = match s.visibility {
                Visibility::Visible => "",
                Visibility::Hidden => " state=\"hidden\"",
                Visibility::VeryHidden => " state=\"veryHidden\"",
            };
            let name: String = s.name.chars().take(31).collect();
            let _ = write!(x, "<sheet name=\"{}\" sheetId=\"{id}\"{state} r:id=\"{}\"/>", esc_attr(&name), esc_attr(&s.rid));
            id += 1;
        }
        (x, id)
    }
}

/// The part names reachable from `root` through kept parts' relationships (normalized).
fn reachable(p: &Preserved, root: &PreservedRel) -> HashSet<String> {
    let by_name: HashMap<String, &PreservedPart> = p.parts.iter().map(|q| (normalize(&q.name), q)).collect();
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from([normalize(&root.target)]);
    while let Some(n) = queue.pop_front() {
        if !seen.insert(n.clone()) {
            continue;
        }
        if let Some(q) = by_name.get(&n) {
            queue.extend(q.rels.iter().filter(|r| !r.external).map(|r| normalize(&r.target)));
        }
    }
    seen
}

/// The kept parts of `p` that one of `roots` reaches. Parts that only a rewritten owner's
/// relationships reach are written with that owner (`owner_extras`), so an owner that is gone
/// takes them with it instead of leaving an orphan part behind.
fn live_parts(p: &Preserved, roots: &[&PreservedRel], facts: &PartFacts) -> Vec<PreservedPart> {
    // Only usable parts are walked, so a part reached only through one that is dropped with its
    // owner (a slicer behind a dropped cache) is dropped too instead of being left orphaned.
    let by_name: HashMap<String, &PreservedPart> = p.parts.iter().filter(|q| part_usable(facts, q)).map(|q| (normalize(&q.name), q)).collect();
    let mut live: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = roots.iter().map(|r| normalize(&r.target)).collect();
    while let Some(n) = queue.pop_front() {
        if !live.insert(n.clone()) {
            continue;
        }
        if let Some(q) = by_name.get(&n) {
            queue.extend(q.rels.iter().filter(|r| !r.external).map(|r| normalize(&r.target)));
        }
    }
    p.parts.iter().filter(|q| live.contains(&normalize(&q.name)) && part_usable(facts, q)).cloned().collect()
}

/// The kept parts of `p` an owner GridCraft writes reaches through its own relationships.
fn owner_parts(p: &Preserved, key: &str, facts: &PartFacts) -> (Vec<PreservedPart>, Vec<PreservedRel>, Vec<String>) {
    let prefix = format!("{key}{KS}");
    let mut sources: Vec<PreservedRel> = vec![];
    for r in p.rels.iter().filter(|r| r.id.starts_with(&prefix)) {
        if let Some(id) = r.id.get(prefix.len()..) {
            sources.push(PreservedRel { id: id.to_string(), ..r.clone() });
        }
    }
    let frags: Vec<String> = p.elements.iter().filter(|e| e.slot == key).map(|e| e.xml.clone()).collect();
    let roots: Vec<&PreservedRel> = sources.iter().collect();
    (live_parts(p, &roots, facts), sources, frags)
}

/// What a part GridCraft rewrites keeps: its `<ext>` children and the relationships and parts
/// only it owns. Writes the parts and adds the relationships to `rels` (the part's own), then
/// returns the `<ext>` XML with fresh relationship ids; empty when `key` matches no kept owner
/// (the table or PivotTable was deleted or renamed) or it had nothing.
pub fn owner_extras(owner: Option<&Preserved>, key: Option<&str>, rels: &mut Rels, out: &mut Out) -> String {
    let (Some(p), Some(key)) = (owner, key) else { return String::new() };
    let facts = std::mem::take(&mut out.kept.facts);
    let (parts, sources, frags) = owner_parts(p, key, &facts);
    out.kept.facts = facts;
    if sources.is_empty() && frags.is_empty() {
        return String::new();
    }
    let names = write_parts(out, &parts);
    let ids = attach(rels, &sources, &names);
    let mut x = String::new();
    for f in &frags {
        if let Some(m) = remap(f, &ids) {
            x.push_str(&m);
        }
    }
    x
}

/// Every formula (`<c:f>`) of a chart part, depth first.
fn chart_formulas(el: &xml::El, out: &mut Vec<String>) {
    if el.name == "f" && !el.text.trim().is_empty() {
        out.push(el.text.trim().to_string());
    }
    for c in &el.children {
        chart_formulas(c, out);
    }
}

/// The sheet names a formula such as `'My Sheet'!$A$1:$A$9,Data!$B$1` refers to. References to
/// other workbooks (`[1]Sheet!A1`) are not sheets of this one and are left out.
pub(crate) fn sheet_names_in(formula: &str) -> Vec<String> {
    let mut names = vec![];
    let chars: Vec<char> = formula.chars().collect();
    let at = |i: usize| chars.get(i).copied();
    let mut i = 0;
    while let Some(c) = at(i) {
        let mut name = String::new();
        let start = i;
        if c == '\'' {
            i += 1;
            while let Some(c) = at(i) {
                if c == '\'' {
                    if at(i + 1) == Some('\'') {
                        name.push('\'');
                        i += 2;
                        continue;
                    }
                    break;
                }
                name.push(c);
                i += 1;
            }
            i += 1;
        } else {
            while let Some(c) = at(i).filter(|c| !matches!(c, '!' | ',' | '(' | ')' | '\'' | ' ')) {
                name.push(c);
                i += 1;
            }
        }
        if at(i) == Some('!') {
            if !name.is_empty() && !name.starts_with('[') {
                names.push(name);
            }
            i += 1;
        } else if i == start {
            i += 1;
        }
    }
    names
}

/// Whether a kept chart sheet can be written back: its chart's series must still refer to sheets
/// that exist under the same names (the chart is kept as it was, so a series on a deleted or
/// renamed sheet would be left pointing at nothing), and no worksheet may have taken its name.
fn chart_sheet_usable(wb: &Workbook, p: &Preserved, s: &PreservedSheet) -> bool {
    let same = |a: &str, b: &str| a.chars().take(31).collect::<String>().eq_ignore_ascii_case(&b.chars().take(31).collect::<String>());
    if wb.sheets.iter().any(|w| same(&w.name, &s.name)) {
        return false;
    }
    let Some(root) = p.rels.iter().find(|r| r.id == s.rel_id) else { return false };
    let parts = reachable(p, root);
    let mut formulas = vec![];
    for q in p.parts.iter().filter(|q| parts.contains(&normalize(&q.name)) && q.content_type.contains("chart")) {
        if let Ok(x) = xml::parse(&q.data) {
            chart_formulas(&x, &mut formulas);
        }
    }
    formulas.iter().flat_map(|f| sheet_names_in(f)).all(|n| wb.sheets.iter().any(|w| same(&w.name, &n)))
}

/// Writes the workbook's kept parts and relationships. Chart sheets that can't be written back
/// (see [`chart_sheet_usable`]) are left out with the parts only they reach.
pub fn write_workbook(wb: &Workbook, wb_rels: &mut Rels, out: &mut Out) -> WorkbookKept {
    let Some(p) = wb.preserved.as_deref() else { return WorkbookKept::default() };
    let (live, dropped): (Vec<&PreservedSheet>, Vec<&PreservedSheet>) = p.sheets.iter().partition(|s| chart_sheet_usable(wb, p, s));
    let root_of = |s: &PreservedSheet| p.rels.iter().find(|r| r.id == s.rel_id);
    let dropped_ids: HashSet<&str> = dropped.iter().map(|s| s.rel_id.as_str()).collect();
    // Parts reached only by dropped chart sheets go with them.
    let mut gone: HashSet<String> = dropped.iter().filter_map(|s| root_of(s)).flat_map(|r| reachable(p, r)).collect();
    if !gone.is_empty() {
        let others = p.rels.iter().filter(|r| !dropped_ids.contains(r.id.as_str())).flat_map(|r| reachable(p, r));
        for n in others {
            gone.remove(&n);
        }
    }
    let rels: Vec<PreservedRel> = p.rels.iter().filter(|r| !dropped_ids.contains(r.id.as_str()) && split_owner(&r.id).is_none()).cloned().collect();
    // A rewritten part's own parts (a PivotTable's slicer cache) are written with it.
    let facts = std::mem::take(&mut out.kept.facts);
    let mut roots: Vec<&PreservedRel> = rels.iter().collect();
    roots.extend(p.package_rels.iter());
    let mut kept_parts = live_parts(p, &roots, &facts);
    out.kept.facts = facts;
    kept_parts.retain(|q| !gone.contains(&normalize(&q.name)));
    let names = write_parts(out, &kept_parts);
    let ids = attach(wb_rels, &rels, &names);
    for r in &p.package_rels {
        if let Some(target) = rel_target(r, &names) {
            out.kept.root.push((r.rel_type.clone(), target, r.external));
        }
    }
    WorkbookKept {
        elements: p.elements.iter().filter(|e| e.slot == "externalReferences").filter_map(|e| remap(&e.xml, &ids)).collect(),
        ext: p.elements.iter().filter(|e| e.slot == EXT_SLOT).filter_map(|e| remap(&e.xml, &ids)).collect(),
        sheets: live
            .iter()
            .filter_map(|s| Some(KeptSheet { before: s.before, name: s.name.clone(), visibility: s.visibility, rid: ids.get(&s.rel_id)?.clone() }))
            .collect(),
    }
}

/// The workbook's kept elements of `slot` that refer to no part (`fileVersion`, the calc-features
/// extension).
pub fn workbook_elements(wb: &Workbook, slot: &str) -> String {
    let Some(p) = wb.preserved.as_deref() else { return String::new() };
    p.elements.iter().filter(|e| e.slot == slot).map(|e| e.xml.as_str()).collect()
}

/// Adds the kept package-root relationships (custom properties…) to `_rels/.rels`.
pub fn write_package_rels(root: &mut Rels, out: &mut Out) {
    for (ty, target, external) in std::mem::take(&mut out.kept.root) {
        root.add_raw(&ty, &target, external);
    }
}

/// A sheet's kept content being written.
pub struct SheetKept<'a> {
    p: Option<&'a Preserved>,
    names: NameMap,
    ids: HashMap<String, String>,
}

impl<'a> SheetKept<'a> {
    /// Writes the sheet's kept parts and adds their relationships to the sheet's.
    pub fn write(sheet: &'a Sheet, rels: &mut Rels, out: &mut Out) -> SheetKept<'a> {
        let Some(p) = sheet.preserved.as_deref() else { return SheetKept { p: None, names: NameMap::new(), ids: HashMap::new() } };
        // Parts a rewritten owner (a table, a PivotTable) owns are written with that owner, so
        // they leave with it when it is renamed or deleted.
        let mut roots: Vec<&PreservedRel> = p.rels.iter().filter(|r| split_owner(&r.id).is_none()).collect();
        roots.extend(p.vml.iter().flat_map(|v| v.rels.iter()));
        let facts = std::mem::take(&mut out.kept.facts);
        let parts = live_parts(p, &roots, &facts);
        out.kept.facts = facts;
        let names = write_parts(out, &parts);
        let rels_kept: Vec<PreservedRel> = p.rels.iter().filter(|r| split_owner(&r.id).is_none()).cloned().collect();
        let ids = attach(rels, &rels_kept, &names);
        SheetKept { p: Some(p), names, ids }
    }

    /// The sheet `<ext>` children GridCraft does not model, relationship ids rewritten.
    pub fn ext_elements(&self) -> String {
        let Some(p) = self.p else { return String::new() };
        p.elements.iter().filter(|e| e.slot == EXT_SLOT).filter_map(|e| remap(&e.xml, &self.ids)).collect()
    }

    fn vml(&self) -> Option<&'a PreservedPart> {
        self.p?.vml.as_ref()
    }

    fn write_vml_rels(&self, vml: &PreservedPart, name: &str, out: &mut Out) {
        if !vml.rels.is_empty() {
            out.part(&rels_part_name(name), None, part_rels_xml(&vml.rels, &self.names).into_bytes());
        }
    }

    /// GridCraft's notes VML for part `name`, merged with the kept control and object shapes.
    pub fn notes_vml(&self, ours: String, name: &str, out: &mut Out) -> String {
        let Some(vml) = self.vml() else { return ours };
        self.write_vml_rels(vml, name, out);
        merge_vml(&String::from_utf8_lossy(&vml.data), &ours)
    }

    /// The kept VML drawing of a sheet without notes; its new relationship id.
    pub fn lone_vml(&self, rels: &mut Rels, out: &mut Out) -> Option<String> {
        let vml = self.vml()?;
        let name = out.kept.claim(&vml.name);
        out.part(&name, Some(CT_VML), vml.data.to_vec());
        self.write_vml_rels(vml, &name, out);
        Some(rels.add("vmlDrawing", &format!("/{name}")))
    }

    /// The new relationship id for `<pageSetup r:id>` (printer settings).
    pub fn page_setup_id(&self) -> Option<&str> {
        let old = &self.p?.elements.iter().find(|e| e.slot == "pageSetup")?.xml;
        self.ids.get(old).map(String::as_str)
    }

    /// A chart sheet's kept child `tag` (`pageMargins`, `pageSetup`, `headerFooter`, `extLst`…),
    /// relationship ids rewritten. `None` when the file had none, or when an id it names is not
    /// one of the parts kept with it (a dangling reference is never written).
    pub fn chart_sheet_element(&self, tag: &str) -> Option<String> {
        let slot = format!("{CHART_SHEET_SLOT}{tag}");
        let xml = self.p?.elements.iter().find(|e| e.slot == slot)?.xml.clone();
        remap(&xml, &self.ids)
    }

    /// The kept sheet elements, in schema order.
    pub fn elements(&self) -> String {
        let Some(p) = self.p else { return String::new() };
        let mut s = String::new();
        for slot in SHEET_SLOTS {
            for e in p.elements.iter().filter(|e| e.slot == *slot) {
                if let Some(x) = remap(&e.xml, &self.ids) {
                    s.push_str(&x);
                }
            }
        }
        s
    }
}

// ---------------------------------------------------------------- linked workbooks

/// `wb` as it is saved into the folder `to` (absolute): the relative paths of its linked
/// workbooks, relative to the folder it was read from, are rewritten relative to `to`, as Excel
/// does on Save As, or made absolute when no relative path exists (another drive). `None` when
/// nothing changes.
pub fn rebase_links(wb: &Workbook, to: &Path) -> Option<Workbook> {
    let p = wb.preserved.as_deref()?;
    let from = Folder::of(wb.source_dir.as_deref()?)?;
    let to = Folder::of(to)?;
    if from == to {
        return None;
    }
    let mut parts = p.parts.clone();
    let mut changed = false;
    for r in parts.iter_mut().flat_map(|part| part.rels.iter_mut()) {
        if r.external
            && is_kind(r, &["externalLinkPath"])
            && let Some(target) = rebase(&r.target, &from, &to)
        {
            r.target = target;
            changed = true;
        }
    }
    if !changed {
        return None;
    }
    let mut out = wb.clone();
    out.preserved = Some(Arc::new(Preserved { parts, ..p.clone() }));
    Some(out)
}

/// An absolute folder, lexically normalized.
#[derive(PartialEq)]
struct Folder {
    /// Drive or share and root separator (`/`, `C:\`, `\\server\share\`).
    root: String,
    names: Vec<String>,
}

impl Folder {
    fn of(dir: &Path) -> Option<Folder> {
        if !dir.is_absolute() {
            return None;
        }
        let mut f = Folder { root: String::new(), names: vec![] };
        for c in dir.components() {
            match c {
                Component::Prefix(_) | Component::RootDir => f.root.push_str(&c.as_os_str().to_string_lossy()),
                Component::CurDir => {}
                Component::ParentDir => {
                    f.names.pop();
                }
                Component::Normal(n) => f.names.push(n.to_string_lossy().into_owned()),
            }
        }
        Some(f)
    }
}

/// A relative link target read from `from`, rewritten for `to`. `None` keeps it as it is: an
/// absolute path or URL, or a path that climbs above the root.
fn rebase(target: &str, from: &Folder, to: &Folder) -> Option<String> {
    if !is_relative_path(target) {
        return None;
    }
    // Each name as the file system knows it, and as it is written in a target. Names taken from
    // the original target keep their spelling.
    let mut path: Vec<(String, String)> = from.names.iter().map(|n| (n.clone(), encode_name(n))).collect();
    for seg in target.split(['/', '\\']) {
        match seg {
            "" | "." => {}
            ".." => {
                path.pop()?;
            }
            _ => path.push((decode_name(seg), seg.to_string())),
        }
    }
    let folders = path.len().checked_sub(1)?;
    if !from.root.eq_ignore_ascii_case(&to.root) {
        let sep = if from.root.contains('\\') { "\\" } else { "/" };
        let names: Vec<&str> = path.iter().map(|(name, _)| name.as_str()).collect();
        return Some(format!("file:///{}{}", from.root, names.join(sep)));
    }
    let common = to.names.iter().zip(&path).take(folders).take_while(|(a, (b, _))| *a == b).count();
    let up = "../".repeat(to.names.len().saturating_sub(common));
    let down: Vec<&str> = path.iter().skip(common).map(|(_, written)| written.as_str()).collect();
    Some(format!("{up}{}", down.join("/")))
}

/// Not empty, no root, no scheme or drive (`file:`, `http:`, `C:`).
fn is_relative_path(target: &str) -> bool {
    let scheme = target.split_once(':').is_some_and(|(s, _)| {
        s.starts_with(|c: char| c.is_ascii_alphabetic()) && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    });
    !target.is_empty() && !target.starts_with(['/', '\\']) && !scheme
}

/// A folder name as a segment of a relationship target (a URI): ASCII characters a URI path
/// can't hold are percent-encoded, other characters are written as they are.
fn encode_name(name: &str) -> String {
    let mut s = String::with_capacity(name.len());
    for c in name.chars() {
        if !c.is_ascii() || c.is_ascii_alphanumeric() || "-._~!$&'()+,;=@".contains(c) {
            s.push(c);
        } else {
            let _ = write!(s, "%{:02X}", c as u32);
        }
    }
    s
}

/// A target segment with its percent-escapes decoded; as written when they don't decode to UTF-8.
fn decode_name(seg: &str) -> String {
    let b = seg.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        let hex = b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match hex {
            Some(byte) if c == b'%' => {
                out.push(byte);
                i += 3;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| seg.to_string())
}
