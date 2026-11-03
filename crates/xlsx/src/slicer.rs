//! Slicers and timelines in `.xlsx`: the model side of Excel's Insert ▸ Slicer / Insert ▸ Timeline.
//!
//! Reading models the parts `passthrough` used to keep byte for byte: a *slicer cache* (the field
//! it shows, the PivotTable views it filters, and one `<i x s>` item per value with its selected
//! flag), the *slicers* part (one per worksheet, holding a view per slicer: name, caption, style,
//! column count) and the same pair for timelines. The model's tiles are read from the pivot cache's
//! shared items (`x` indexes them), so a save writes the same kind of value back (`<s>`, `<n>`,
//! `<b>`, `<e>`, `<m>` for a blank). A *table slicer* (a slicer on a table column, not a
//! PivotTable) is modelled too: its cache names the table and column by id in the cache's
//! `<extLst>` (`x15:tableSlicerCache`, [MS-XLSX] 2.2.4.8 / 2.3.2.1.3 / 2.6.97) and its selection is
//! the table's AutoFilter criterion, which is where Excel keeps it.
//!
//! Writing is from the model, once, and the pass-through is kept out of it — the same contract as
//! the model's PivotTable extensions (`owner_extras`):
//! - a slicer or timeline that was modelled is written by [`SlicerWriter`]; on read, the kept
//!   relationships that reached its parts (the workbook's `<x14:slicerCaches>` and
//!   `<x15:timelineCacheRefs>`, the sheet's `<x14:slicerList>` and `<x15:timelineRefs>`, the
//!   PivotTable's `<x14:slicerCacheData>` extension) are removed from the preserved content, so the
//!   kept path cannot write them a second time;
//! - content that was **not** modelled (an OLAP cache, item properties, an unknown attribute or
//!   relationship, a timeline cache whose state, range or refresh version this model does not
//!   carry, a sort order other than the default, a damaged part) is not touched: every part,
//!   relationship and extension stays as it is, and a save keeps it byte for byte;
//! - modelling is all-or-nothing for each kind: one cache GridCraft cannot fully understand leaves
//!   *every* slicer in that workbook on the kept path, and the same for timelines. That is stricter
//!   than per-part modelling, and it means a model is never a partial rewrite of the same lists;
//! - a modelled slicer/timeline whose PivotTable is gone is not written (its cache goes with it),
//!   matching the kept path's rule for a cache that names a deleted or renamed PivotTable. A table
//!   slicer goes with its table (or a column the table no longer has).
//!
//! Placement lives in the drawing part, not here: a slicer/timeline view is placed by an
//! `xdr:graphicFrame` whose `a:graphicData` names the widget and its name ([`crate::drawing`]).
//! Reading takes the anchor from that frame; writing emits it there.
//!
//! A timeline's selected date range is modelled ([`TimelineRange`]): the cache's
//! `<state><selection startDate endDate>` with `filterType="dateBetween"`, and no selection with
//! `filterType="unknown"`. The two are written together, so only those two pairs are read: a
//! state that says one and carries the other is contradictory and stays on the kept path. A date
//! written in any form other than the `YYYY-MM-DDThh:mm:ss` GridCraft writes (a fraction of a
//! second, a zone, a short date) also stays kept, because a save could not spell it back exactly;
//! so does a selection whose level differs from the level shown ([`timeline_widget`]).
//!
//! Not modelled (kept raw, and said so here so the next reader does not guess): OLAP slicer caches,
//! item properties (`<p>`), custom lists, slicer `extLst`s, a timeline cache's refresh versions
//! (the writer states 0), a timeline state's `singleRangeFilterState` other than the default, the
//! drawing shape's `mc:Fallback`, and the slicer sort order (a file's slicer part does not carry
//! it; the model keeps it as a UI hint only).
//!
//! Attribute provenance: the parts' names and attributes are [MS-XLSX]'s extension schemas
//! (sections 2.2.4.8, 2.2.4.11, 2.2.4.12, 2.4.36–2.4.50, 2.6.67–2.6.114), the drawing frames are
//! [MS-XLSX] 2.2.4.2 with the slicer/timeline drawing schemas of 5.2 and 5.7, and the frame's
//! shape is ECMA-376's `xdr:graphicFrame`. Nothing here is copied from Excel's artwork or theme
//! XML; a style is a name the file carries, and how it looks is GridCraft's own drawing code.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use gridcraft_core::CellRef;
use gridcraft_core::date::{datetime_from_serial, serial_from_ymd, time_fraction};
use gridcraft_model::{
    Anchor, AnchorMode, FilterCriterion, PivotSlicer, PivotTimeline, Slicer, SlicerItem, SlicerSort, SlicerValue, Table, TimeUnit, TimelineRange,
    Workbook,
};

use crate::package::normalize;
use crate::pivot::PivotWriter;
use crate::read::Ctx;
use crate::write::{NS_REL, Out, Rels, XML_DECL};
use crate::xml::{self, El, esc_attr};

pub const CT_SLICER_CACHE: &str = "application/vnd.ms-excel.slicerCache+xml";
pub const CT_SLICER: &str = "application/vnd.ms-excel.slicer+xml";
/// The timeline parts' content types, as [MS-XLSX] 2.1.7 and 2.1.8 spell them.
pub const CT_TIMELINE_CACHE: &str = "application/vnd.ms-excel.TimelineCache+xml";
pub const CT_TIMELINE: &str = "application/vnd.ms-excel.Timeline+xml";

pub const REL_SLICER_CACHE: &str = "http://schemas.microsoft.com/office/2007/relationships/slicerCache";
pub const REL_SLICER: &str = "http://schemas.microsoft.com/office/2007/relationships/slicer";
/// The timeline parts' relationship types, as [MS-XLSX] 2.1.7 and 2.1.8 spell them.
pub const REL_TIMELINE_CACHE: &str = "http://schemas.microsoft.com/office/2010/relationships/TimelineCache";
pub const REL_TIMELINE: &str = "http://schemas.microsoft.com/office/2010/relationships/Timeline";

/// The 2009 extension namespace slicer parts and their workbook/PivotTable extensions use.
pub const NS_X14: &str = "http://schemas.microsoft.com/office/spreadsheetml/2009/9/main";
/// The 2010 extension namespace timeline parts and their sheet/workbook extensions use.
pub const NS_X15: &str = "http://schemas.microsoft.com/office/spreadsheetml/2010/11/main";

/// The namespace a slicer's drawing frame names ([MS-XLSX] 5.2).
pub const NS_DRAWING_SLICER: &str = "http://schemas.microsoft.com/office/drawing/2010/slicer";
/// The namespace a timeline's drawing frame names ([MS-XLSX] 5.7).
pub const NS_DRAWING_TIMELINE: &str = "http://schemas.microsoft.com/office/drawing/2012/timeslicer";

/// The workbook extension listing the slicer caches ([MS-XLSX] 2.2.4.11).
pub const EXT_SLICER_CACHES: &str = "{BBE1A952-AA13-448E-AADC-164F8A28A991}";
/// The workbook extension listing the timeline caches ([MS-XLSX] 2.2.4.11).
pub const EXT_TIMELINE_CACHE_REFS: &str = "{D0CA8CA8-9F24-4464-BF8E-62219DCF47F9}";
/// The sheet extension naming its slicers part ([MS-XLSX] 2.2.4.12).
pub const EXT_SLICER_LIST: &str = "{A8765BA9-456A-4DAB-B4F3-ACF838C121DE}";
/// The sheet extension naming its timelines part ([MS-XLSX] 2.2.4.12).
pub const EXT_TIMELINE_REFS: &str = "{7E03D99C-DC04-49d9-9315-930204A7B6E9}";
/// The slicer cache extension that makes a table the cache's source ([MS-XLSX] 2.2.4.8).
pub const EXT_TABLE_SLICER_CACHE: &str = "{2F2917AC-EB37-4324-AD4E-5DD8C200BD13}";
/// The PivotTable extension that names the slicer caches filtering it. [MS-XLSX] 2.3.2.1.4 makes
/// the cache's own `<pivotTables>` list the association, so this is written for readers that look
/// for the extension (same URI as the sheet's `slicerList`); Excel ignores an unknown extension.
pub const EXT_PIVOT_SLICER_CACHE: &str = EXT_SLICER_LIST;

/// Most tiles read from one slicer cache, and most fields scanned in one pivot cache.
const MAX_ITEMS: usize = 100_000;
const MAX_CACHE_FIELDS: usize = 16_384;
/// Most slicer caches, slicers, timelines and timeline caches read or written.
const MAX_WIDGETS: usize = 4096;

/// A modelled slicer cache that a PivotTable is filtered by: the PivotTable part gets a
/// relationship to the cache part and the `<x14:slicerCacheData>` extension naming it.
#[derive(Clone, Debug)]
pub struct CacheLink {
    /// The PivotTable's name (the model's `PivotTable::name`).
    pub pivot: String,
    /// The cache part, e.g. `xl/slicerCaches/slicerCache1.xml`.
    pub cache_part: String,
}

/// The `<ext>` a PivotTable part writes for its modelled slicer caches, adding one relationship per
/// cache to `rels`. Empty when none of them filters that PivotTable.
pub fn pivot_ext(pivot: &str, links: &[CacheLink], rels: &mut Rels) -> String {
    let mine: Vec<&CacheLink> = links.iter().filter(|l| l.pivot.eq_ignore_ascii_case(pivot)).collect();
    if mine.is_empty() {
        return String::new();
    }
    // `xmlns:r` is declared here, on the element that uses it: the PivotTable part's root does not
    // carry it, and a strict parser rejects a part with an unbound prefix (Excel: repair prompt).
    let mut x = format!("<ext uri=\"{EXT_PIVOT_SLICER_CACHE}\" xmlns:r=\"{NS_REL}\" xmlns:x14=\"{NS_X14}\"><x14:slicerCacheData>");
    let mut seen = HashSet::new();
    for link in mine {
        if !seen.insert(link.cache_part.to_ascii_lowercase()) {
            continue;
        }
        let rid = rels.add_uri(REL_SLICER_CACHE, &from_part_dir(&link.cache_part));
        let _ = write!(x, "<x14:slicerCache r:id=\"{rid}\"/>");
    }
    x.push_str("</x14:slicerCacheData></ext>");
    x
}

/// A part name relative to a part in a sibling folder (`xl/slicerCaches/x.xml` → `../slicerCaches/x.xml`).
fn from_part_dir(part: &str) -> String {
    format!("../{}", part.strip_prefix("xl/").unwrap_or(part))
}

/// A part name relative to the workbook part (`xl/slicerCaches/x.xml` → `slicerCaches/x.xml`).
fn from_workbook(part: &str) -> String {
    part.strip_prefix("xl/").unwrap_or(part).to_string()
}

// ================================================================ reading

/// Reads the workbook's modelled slicers and timelines into the sheets, and removes them from the
/// preserved content so the pass-through does not write them a second time.
///
/// Called once, after every sheet, PivotTable, table and kept part has been read. Both kinds are
/// modelled all-or-nothing: anything GridCraft does not fully understand leaves that kind raw.
pub fn read(wb: &mut Workbook, cx: &mut Ctx<'_>, wb_xml: &El, wb_rels: &[crate::package::Rel]) {
    let items = pivot_cache_items(cx, wb_xml, wb_rels);
    let slicers = read_slicers(wb, cx, &items);
    let timelines = read_timelines(wb, cx);
    let mut cache_parts: HashSet<String> = HashSet::new();
    let mut slicer_parts: HashSet<String> = HashSet::new();
    let mut timeline_parts: HashSet<String> = HashSet::new();
    let mut placed: HashSet<(String, bool)> = HashSet::new();
    if let Some(list) = slicers {
        placed.extend(list.placed.iter().cloned());
        for (si, mut widget) in list.pivot {
            widget.id = cx.next_object_id();
            if let Some(sheet) = wb.sheet_mut(si)
                && sheet.pivot_slicers.len() < MAX_WIDGETS
            {
                sheet.pivot_slicers.push(widget);
            }
        }
        for (si, widget) in list.tables {
            if let Some(sheet) = wb.sheet_mut(si)
                && sheet.slicers.len() < MAX_WIDGETS
            {
                let mut widget = widget;
                widget.id = cx.next_object_id();
                sheet.slicers.push(widget);
            }
        }
        cache_parts.extend(list.caches);
        slicer_parts.extend(list.parts);
    }
    if let Some(list) = timelines {
        placed.extend(list.placed.iter().cloned());
        for (si, mut widget) in list.views {
            widget.id = cx.next_object_id();
            if let Some(sheet) = wb.sheet_mut(si)
                && sheet.pivot_timelines.len() < MAX_WIDGETS
            {
                sheet.pivot_timelines.push(widget);
            }
        }
        cache_parts.extend(list.caches);
        timeline_parts.extend(list.parts);
    }
    let unplaced: Vec<(String, bool)> = cx
        .widget_frames
        .iter()
        .map(|f| (f.name.clone(), f.timeline))
        .filter(|(name, timeline)| !placed.contains(&(name.to_lowercase(), *timeline)))
        .collect();
    for (name, timeline) in unplaced {
        let what = if timeline { "timeline" } else { "slicer" };
        cx.warn(format!("the drawing frame for the {what} `{name}` is not written: its part is kept as it came"));
    }
    if cache_parts.is_empty() && timeline_parts.is_empty() {
        return;
    }
    strip(wb, &cache_parts, &slicer_parts, &timeline_parts);
}

/// Removes the kept relationships that reached a modelled cache or timeline part. Each preserved
/// `<ext>` fragment names its part by relationship id, and a fragment whose ids no longer resolve is
/// dropped whole by the writer (`passthrough::remap`), so this is what keeps the kept path from
/// writing a second `<x14:slicerCaches>` list or a second cache part.
fn strip(wb: &mut Workbook, caches: &HashSet<String>, slicers: &HashSet<String>, timelines: &HashSet<String>) {
    let drop_rel = |r: &gridcraft_model::PreservedRel| {
        let kind = r.rel_type.rsplit('/').next().unwrap_or("").to_ascii_lowercase();
        let target = normalize(&r.target);
        match kind.as_str() {
            "slicercache" | "timelinecache" => caches.contains(&target),
            "slicer" => slicers.contains(&target),
            "timeline" => timelines.contains(&target),
            _ => false,
        }
    };
    if let Some(p) = wb.preserved.as_mut() {
        let p = std::sync::Arc::make_mut(p);
        p.rels.retain(|r| !drop_rel(r));
    }
    for i in 0..wb.sheets.len() {
        let owned = wb.sheets.get(i).and_then(|s| s.preserved.as_ref()).is_some_and(|p| p.rels.iter().any(drop_rel));
        if !owned {
            continue;
        }
        let Some(sheet) = wb.sheet_mut(i) else { continue };
        if let Some(p) = sheet.preserved.as_mut() {
            std::sync::Arc::make_mut(p).rels.retain(|r| !drop_rel(r));
        }
    }
}

/// The shared items of every workbook pivot cache, by `cacheId` and lower-cased field name. This is
/// what a slicer cache's `<i x>` indexes. A field with a shared item kind we do not model (a date
/// `<d>`, anything not ECMA-376's list) is left out, which makes the cache using it unmodelled.
fn pivot_cache_items(cx: &mut Ctx<'_>, wb_xml: &El, wb_rels: &[crate::package::Rel]) -> HashMap<u32, HashMap<String, Vec<SlicerValue>>> {
    let mut out: HashMap<u32, HashMap<String, Vec<SlicerValue>>> = HashMap::new();
    let Some(list) = wb_xml.child("pivotCaches") else { return out };
    for pc in list.kids("pivotCache") {
        let (Some(id), Some(rid)) = (pc.attr_u32("cacheId"), pc.attr("id")) else { continue };
        let Some(rel) = wb_rels.iter().find(|r| r.id == rid && !r.external) else { continue };
        let root = match cx.optional_xml(&rel.target) {
            Ok(Some(x)) => x,
            _ => continue,
        };
        let Some(fields) = root.child("cacheFields") else { continue };
        let mut map: HashMap<String, Vec<SlicerValue>> = HashMap::new();
        for (n, f) in fields.kids("cacheField").enumerate() {
            if n >= MAX_CACHE_FIELDS {
                break;
            }
            let Some(name) = f.attr("name") else { continue };
            if let Some(values) = shared_items(f) {
                map.insert(name.to_lowercase(), values);
            }
        }
        out.insert(id, map);
    }
    out
}

/// One cache field's shared items as slicer values, in the order `<i x>` indexes them. `None` for a
/// kind we do not model (dates, and anything else).
fn shared_items(field: &El) -> Option<Vec<SlicerValue>> {
    let list = field.child("sharedItems")?;
    let mut out = Vec::new();
    for c in &list.children {
        if out.len() >= MAX_ITEMS {
            return None;
        }
        let v = match c.name.as_str() {
            "s" => SlicerValue::Text(c.attr("v").unwrap_or("").to_string()),
            "n" => SlicerValue::Number(c.attr_f64("v")?),
            "b" => SlicerValue::Bool(c.attr_bool("v")?),
            "e" => SlicerValue::Error(c.attr("v").unwrap_or("").to_string()),
            "m" => SlicerValue::Blank,
            _ => return None,
        };
        out.push(v);
    }
    // A shared item list that is not exactly `count` long would index wrongly.
    if let Some(n) = list.attr_u32("count")
        && n as usize != out.len()
    {
        return None;
    }
    Some(out)
}

/// Every kept part with content type `ct`, deduplicated across the workbook and all sheets.
fn kept_parts<'a>(wb: &'a Workbook, ct: &str) -> Vec<&'a gridcraft_model::PreservedPart> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let sources = wb.preserved.iter().map(|p| &**p).chain(wb.sheets.iter().filter_map(|s| s.preserved.as_deref()));
    for p in sources {
        for part in &p.parts {
            if !part.content_type.eq_ignore_ascii_case(ct) {
                continue;
            }
            if seen.insert(normalize(&part.name)) {
                out.push(part);
            }
        }
    }
    out
}

/// `(sheet, index)` of the PivotTable called `name`.
fn pivot_sheet(wb: &Workbook, name: &str) -> Option<usize> {
    wb.sheets.iter().position(|s| s.pivots.iter().any(|p| p.name.eq_ignore_ascii_case(name)))
}

/// The anchor a widget gets when a file carries no placement: to the right of its PivotTable's
/// report (the slicer part itself stores no position; only the drawing frame does).
fn anchor_beside(wb: &Workbook, si: usize, pivot: &str, width: f32, height: f32) -> Anchor {
    let cell = wb
        .sheet(si)
        .and_then(|s| s.pivots.iter().find(|p| p.name.eq_ignore_ascii_case(pivot)))
        .and_then(|p| p.last_range)
        .map(|r| CellRef::new(r.start.row, r.end.col.saturating_add(2)))
        .unwrap_or_default();
    Anchor { cell, dx: 4.0, dy: 4.0, width, height, mode: AnchorMode::MoveOnly }
}

/// The anchor a table slicer gets when a file carries no placement: to the right of its table.
fn anchor_beside_table(t: &Table, width: f32, height: f32) -> Anchor {
    let cell = CellRef::new(t.range.start.row, t.range.end.col.saturating_add(2));
    Anchor { cell, dx: 4.0, dy: 4.0, width, height, mode: AnchorMode::MoveOnly }
}

/// One slicer cache part as read, before its views are attached.
struct CacheRead {
    part: String,
    name: String,
    source: String,
    kind: CacheKind,
}

enum CacheKind {
    Pivot {
        pivot_cache_id: u32,
        pivot_tables: Vec<String>,
        values: Vec<SlicerValue>,
        /// One per value: the selected flag. `None` = the file wrote no item list, i.e. all selected.
        selected: Option<Vec<bool>>,
    },
    Table {
        table_id: u32,
        column: u32,
    },
}

/// The slicers read from one workbook: the views (with their sheet), and the parts to strip from
/// the kept path.
struct SlicersRead {
    pivot: Vec<(usize, PivotSlicer)>,
    tables: Vec<(usize, Slicer)>,
    caches: HashSet<String>,
    parts: HashSet<String>,
    /// The drawing frames consumed: `(view name, timeline)`.
    placed: HashSet<(String, bool)>,
}

/// A parsed `<slicer>` view of a slicers part: the fields of `CT_Slicer` the model holds.
struct SlicerView {
    name: String,
    cache: String,
    caption: String,
    style: String,
    columns: u32,
}

fn read_slicers(wb: &mut Workbook, cx: &mut Ctx<'_>, items: &HashMap<u32, HashMap<String, Vec<SlicerValue>>>) -> Option<SlicersRead> {
    let cache_parts = kept_parts(wb, CT_SLICER_CACHE);
    let view_parts = kept_parts(wb, CT_SLICER);
    if cache_parts.is_empty() || view_parts.is_empty() {
        return Some(SlicersRead { pivot: vec![], tables: vec![], caches: HashSet::new(), parts: HashSet::new(), placed: HashSet::new() });
    }
    let mut caches: Vec<CacheRead> = Vec::new();
    let mut why: Option<String> = None;
    for part in &cache_parts {
        if caches.len() >= MAX_WIDGETS {
            why = Some("too many slicer caches".into());
            break;
        }
        match read_cache(part, items) {
            Ok(c) => caches.push(c),
            Err(e) => {
                why = Some(e);
                break;
            }
        }
    }
    // The views belong to the worksheet whose relationship reaches their part, and each view names
    // its cache ([MS-XLSX] 2.1.5, 2.4.9).
    let mut out = SlicersRead { pivot: vec![], tables: vec![], caches: HashSet::new(), parts: HashSet::new(), placed: HashSet::new() };
    'sheets: for si in 0..wb.sheets.len() {
        let Some(p) = wb.sheets.get(si).and_then(|s| s.preserved.as_deref()) else { continue };
        let targets: Vec<String> = p.rels.iter().filter(|r| !r.external && rel_kind(&r.rel_type, "slicer")).map(|r| r.target.clone()).collect();
        for target in targets {
            let Some(part) = parts_by_name(&view_parts, &target) else {
                why = Some(format!("{target}: the slicers part is missing"));
                break 'sheets;
            };
            if !part.rels.is_empty() {
                why = Some(format!("{}: it has relationships GridCraft does not model", part.name));
                break 'sheets;
            }
            let Ok(root) = xml::parse(&part.data) else {
                why = Some(format!("{}: not XML", part.name));
                break 'sheets;
            };
            if root.name != "slicers" {
                why = Some(format!("{}: not a slicers part", part.name));
                break 'sheets;
            }
            let views = root.kids("slicer").count();
            if views == 0 {
                continue;
            }
            for el in root.kids("slicer") {
                let Some(view) = read_view(el) else {
                    why = Some(format!("{}: a slicer has an attribute GridCraft does not model", part.name));
                    break 'sheets;
                };
                let Some(cache) = caches.iter().find(|c| c.name.eq_ignore_ascii_case(&view.cache)) else {
                    why = Some(format!("{}: the slicer `{}` names no cache", part.name, view.name));
                    break 'sheets;
                };
                out.caches.insert(normalize(&cache.part));
                out.placed.insert((view.name.to_lowercase(), false));
                match &cache.kind {
                    CacheKind::Pivot { pivot_cache_id, pivot_tables, values, selected } => {
                        if pivot_tables.iter().any(|n| pivot_sheet(wb, n).is_none()) {
                            why = Some(format!("{}: it names a PivotTable this workbook does not have", cache.part));
                            break 'sheets;
                        }
                        let Some(first) = pivot_tables.first() else {
                            why = Some(format!("{}: its PivotTable list is empty", cache.part));
                            break 'sheets;
                        };
                        let Some(pivot_sheet_index) = pivot_sheet(wb, first) else {
                            why = Some(format!("{}: it names a PivotTable this workbook does not have", cache.part));
                            break 'sheets;
                        };
                        if out.pivot.len() + out.tables.len() >= MAX_WIDGETS {
                            why = Some("too many slicers".into());
                            break 'sheets;
                        }
                        let mut widget = PivotSlicer {
                            id: 0,
                            name: view.name.clone(),
                            cache: cache.name.clone(),
                            caption: if view.caption.is_empty() { cache.source.clone() } else { view.caption.clone() },
                            field: cache.source.clone(),
                            pivot_cache_id: *pivot_cache_id,
                            pivot_tables: pivot_tables.clone(),
                            items: values
                                .iter()
                                .enumerate()
                                .map(|(i, v)| {
                                    SlicerItem::new(v.clone(), selected.as_ref().map(|s| s.get(i).copied().unwrap_or(true)).unwrap_or(true))
                                })
                                .collect(),
                            style: view.style.clone(),
                            columns: view.columns,
                            sort: SlicerSort::default(),
                            anchor: anchor_beside(wb, pivot_sheet_index, first, 168.0, 200.0),
                        };
                        widget.anchor = frame_anchor(cx, &view.name, false).unwrap_or(widget.anchor);
                        out.pivot.push((si, widget));
                    }
                    CacheKind::Table { table_id, column } => {
                        if !view.style.is_empty() {
                            why = Some(format!("{}: a table slicer style is not modelled", part.name));
                            break 'sheets;
                        }
                        let Some(table) = wb.sheet(si).and_then(|s| s.tables.iter().find(|t| t.id == *table_id)) else {
                            why = Some(format!("{}: it names a table this sheet does not have", cache.part));
                            break 'sheets;
                        };
                        let Some(column_name) = table_column_name(wb, si, table, *column) else {
                            why = Some(format!("{}: the table has no column {}", cache.part, *column));
                            break 'sheets;
                        };
                        if out.pivot.len() + out.tables.len() >= MAX_WIDGETS {
                            why = Some("too many slicers".into());
                            break 'sheets;
                        }
                        let selected = table_selection(wb, si, table, (*column).saturating_sub(1) as usize);
                        let mut widget = Slicer {
                            id: 0,
                            table: table.name.clone(),
                            caption: if view.caption.is_empty() { column_name.clone() } else { view.caption.clone() },
                            column: column_name,
                            anchor: anchor_beside_table(table, 168.0, 160.0),
                            selected,
                            columns: view.columns,
                        };
                        widget.anchor = frame_anchor(cx, &view.name, false).unwrap_or(widget.anchor);
                        out.tables.push((si, widget));
                    }
                }
            }
            out.parts.insert(normalize(&part.name));
        }
    }
    if let Some(why) = why {
        // All or nothing: one cache we cannot model leaves every slicer on the kept path.
        cx.warn(format!("slicers are kept as they came (they are not editable): {why}"));
        return None;
    }
    Some(out)
}

/// Parses one slicer cache part. `Err` explains why it has to stay on the kept path.
fn read_cache(part: &gridcraft_model::PreservedPart, items: &HashMap<u32, HashMap<String, Vec<SlicerValue>>>) -> Result<CacheRead, String> {
    let root = xml::parse(&part.data).map_err(|_| format!("{}: not XML", part.name))?;
    if root.name != "slicerCacheDefinition" {
        return Err(format!("{}: not a slicer cache definition", part.name));
    }
    let (Some(name), Some(source)) = (root.attr("name"), root.attr("sourceName")) else {
        return Err(format!("{}: a slicer cache needs a name and a source field", part.name));
    };
    // `Ignorable` is MC's list of prefixes a reader may ignore, not data.
    if !attrs_match(&root, &["name", "sourceName", "Ignorable"], &[]) {
        return Err(format!("{}: an attribute of the cache is not modelled", part.name));
    }
    // Only the children and attributes GridCraft writes again are modelled; anything else would be
    // lost on save.
    let mut table_ext: Option<&El> = None;
    for c in &root.children {
        match c.name.as_str() {
            "pivotTables" | "data" => {}
            "extLst" => {
                for e in c.kids("ext") {
                    let uri = e.attr("uri").unwrap_or("");
                    if uri.eq_ignore_ascii_case(EXT_TABLE_SLICER_CACHE) {
                        if table_ext.is_some() || e.children.len() != 1 {
                            return Err(format!("{}: its table source extension is damaged", part.name));
                        }
                        table_ext = e.children.first();
                    } else {
                        return Err(format!("{}: <ext {uri}> is not modelled", part.name));
                    }
                }
            }
            other => return Err(format!("{}: <{other}> is not modelled", part.name)),
        }
    }
    if let Some(table) = table_ext {
        if table.name != "tableSlicerCache" {
            return Err(format!("{}: <{}> is not modelled", part.name, table.name));
        }
        // A table slicer cache names a table and a column; a PivotTable list or an item block would
        // be dropped, so such a cache stays raw.
        if root.child("data").is_some() || root.child("pivotTables").is_some() {
            return Err(format!("{}: a table slicer cache with a data or pivotTables block is not modelled", part.name));
        }
        let defaults: [(&str, &str); 3] = [("sortOrder", "ascending"), ("customListSort", "true"), ("crossFilter", "showItemsWithDataAtTop")];
        if !attrs_match(table, &["tableId", "column"], &defaults) {
            return Err(format!("{}: its tableSlicerCache attributes are not modelled", part.name));
        }
        let (Some(table_id), Some(column)) = (table.attr_u32("tableId"), table.attr_u32("column")) else {
            return Err(format!("{}: it needs a tableId and a column", part.name));
        };
        if column == 0 {
            return Err(format!("{}: column ids start at 1", part.name));
        }
        return Ok(CacheRead {
            part: part.name.clone(),
            name: name.to_string(),
            source: source.to_string(),
            kind: CacheKind::Table { table_id, column },
        });
    }
    let Some(data) = root.child("data") else {
        return Err(format!("{}: no data block", part.name));
    };
    if data.children.len() != 1 || data.children.first().is_some_and(|t| t.name != "tabular") {
        return Err(format!("{}: not a tabular cache", part.name));
    }
    let Some(tab) = data.child("tabular") else {
        return Err(format!("{}: not a tabular cache", part.name));
    };
    if tab.children.iter().any(|c| c.name != "items") || tab.kids("items").count() > 1 {
        return Err(format!("{}: a child of its tabular block is not modelled", part.name));
    }
    if !attrs_match(
        tab,
        &["pivotCacheId"],
        &[("sortOrder", "ascending"), ("customListSort", "true"), ("showMissing", "true"), ("crossFilter", "showItemsWithDataAtTop")],
    ) {
        return Err(format!("{}: its tabular attributes are not modelled", part.name));
    }
    let Some(pivot_cache_id) = tab.attr_u32("pivotCacheId") else {
        return Err(format!("{}: no pivotCacheId", part.name));
    };
    let Some(values) = items.get(&pivot_cache_id).and_then(|m| m.get(&source.to_lowercase())) else {
        return Err(format!("{}: the pivot cache has no shared items for `{source}`", part.name));
    };
    let Some(pivot_tables) = pivot_table_names(&root) else {
        return Err(format!("{}: its PivotTable list is empty or damaged", part.name));
    };
    let selected = read_selection(tab, values.len()).map_err(|e| format!("{}: {e}", part.name))?;
    Ok(CacheRead {
        part: part.name.clone(),
        name: name.to_string(),
        source: source.to_string(),
        kind: CacheKind::Pivot { pivot_cache_id, pivot_tables, values: values.clone(), selected },
    })
}

/// A `<slicer>` view, or `None` when it carries anything the model would lose. `startItem`,
/// `showCaption` and `lockedPosition` are accepted at their documented defaults; `rowHeight`,
/// `level` (OLAP only) and an `extLst` are not modelled.
fn read_view(el: &El) -> Option<SlicerView> {
    if !el.children.is_empty() {
        return None;
    }
    let defaults: [(&str, &str); 3] = [("startItem", "0"), ("showCaption", "true"), ("lockedPosition", "false")];
    if !attrs_match(el, &["name", "cache", "caption", "style", "columnCount"], &defaults) {
        return None;
    }
    let (Some(name), Some(cache)) = (el.attr("name"), el.attr("cache")) else { return None };
    if name.trim().is_empty() {
        return None;
    }
    let columns = match el.attr("columnCount") {
        Some(v) => v.trim().parse::<u32>().ok().filter(|c| (1..=64).contains(c))?,
        None => 1,
    };
    Some(SlicerView {
        name: name.to_string(),
        cache: cache.to_string(),
        caption: el.attr("caption").unwrap_or("").to_string(),
        style: el.attr("style").unwrap_or("").to_string(),
        columns,
    })
}

/// The anchor the sheet's drawing gives the view called `name`, if it has one.
fn frame_anchor(cx: &Ctx<'_>, name: &str, timeline: bool) -> Option<Anchor> {
    cx.widget_frames.iter().find(|f| f.timeline == timeline && f.name.eq_ignore_ascii_case(name)).map(|f| f.anchor)
}

fn parts_by_name<'a>(parts: &[&'a gridcraft_model::PreservedPart], name: &str) -> Option<&'a gridcraft_model::PreservedPart> {
    let n = normalize(name);
    parts.iter().copied().find(|p| normalize(&p.name) == n)
}

/// Whether a relationship type's last segment is `kind` (case-insensitive: the Microsoft
/// extension relationship types differ in case between documents).
fn rel_kind(rel_type: &str, kind: &str) -> bool {
    rel_type.rsplit('/').next().is_some_and(|k| k.eq_ignore_ascii_case(kind))
}

/// The PivotTables a cache's `<pivotTables>` lists, all named.
fn pivot_table_names(root: &El) -> Option<Vec<String>> {
    let list = root.child("pivotTables")?;
    let mut names = Vec::new();
    for p in list.kids("pivotTable") {
        let n = p.attr("name")?.trim();
        if n.is_empty() {
            return None;
        }
        names.push(n.to_string());
    }
    if names.is_empty() { None } else { Some(names) }
}

/// A tabular cache's `<items>`: one selected flag per shared item, in shared-item order, or `None`
/// when the file wrote no list (all selected).
fn read_selection(tab: &El, n: usize) -> Result<Option<Vec<bool>>, String> {
    let Some(list) = tab.child("items") else { return Ok(None) };
    if list.children.len() != n {
        return Err(format!("its item list has {} entries for {n} values", list.children.len()));
    }
    let mut seen = vec![false; n];
    let mut selected = vec![false; n];
    for it in &list.children {
        if it.name != "i" {
            return Err(format!("<{}> is not an item", it.name));
        }
        let x = it.attr_u32("x").ok_or("an item has no `x`")? as usize;
        let Some(slot) = seen.get_mut(x) else { return Err(format!("item x={x} is out of range")) };
        if *slot {
            return Err(format!("item x={x} appears twice"));
        }
        *slot = true;
        if it.attr_bool("s").unwrap_or(false)
            && let Some(v) = selected.get_mut(x)
        {
            *v = true;
        }
        if !it.children.is_empty() {
            return Err("an item carries a property GridCraft does not model".into());
        }
    }
    Ok(Some(selected))
}

/// A table column's name by its one-based `tableColumn/@id`, falling back to the header cell when
/// the model's table carries no column list.
fn table_column_name(wb: &Workbook, si: usize, t: &Table, id: u32) -> Option<String> {
    let idx = id.checked_sub(1)?;
    if let Some(name) = t.columns.get(idx as usize).map(|c| c.name.trim()).filter(|n| !n.is_empty()) {
        return Some(name.to_string());
    }
    let sh = wb.sheet(si)?;
    let text = sh.value(CellRef::new(t.range.start.row, t.range.start.col.saturating_add(idx))).scalar().display();
    if text.is_empty() { None } else { Some(text) }
}

/// A table slicer's tile selection: the table column's value filter, from the sheet's AutoFilter
/// (the live filter state the commands keep) or the table's own. `None` means every tile is on —
/// no filter, a filter of another kind, or a values filter that also shows blanks.
fn table_selection(wb: &Workbook, si: usize, t: &Table, column: usize) -> Option<Vec<String>> {
    let sh = wb.sheet(si)?;
    let from_sheet = sh.autofilter.as_ref().filter(|af| af.range.contains_range(&t.range)).and_then(|af| {
        let abs = t.range.start.col.checked_add(column as u32)?;
        let off = abs.checked_sub(af.range.start.col)?;
        af.criteria.iter().find(|(o, _)| *o == off).map(|(_, c)| c)
    });
    let crit = from_sheet.or_else(|| t.filter.iter().find(|(o, _)| *o as usize == column).map(|(_, c)| c))?;
    match crit {
        FilterCriterion::Values { values, blanks: false } => Some(values.clone()),
        _ => None,
    }
}

/// The date bounds a timeline shows, from the source column its PivotTable reads: the minimum and
/// maximum serial number. `None` when the column has no numeric value, which is a field a timeline
/// cannot display — such a timeline is not written.
fn timeline_bounds(wb: &Workbook, sheet: usize, pivots: &[String], field: &str) -> Option<(f64, f64)> {
    for name in pivots {
        let Some((si, pi)) =
            wb.sheets.iter().enumerate().find_map(|(si, s)| s.pivots.iter().position(|p| p.name.eq_ignore_ascii_case(name)).map(|pi| (si, pi)))
        else {
            continue;
        };
        let Some(pt) = wb.sheet(si).and_then(|s| s.pivots.get(pi)) else { continue };
        let Some((src, range, col)) = crate::pivot::source_column(wb, pt, sheet, field) else { continue };
        let Some(sh) = wb.sheet(src) else { continue };
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for row in range.start.row.saturating_add(1)..=range.end.row {
            if let gridcraft_core::Value::Number(n) = sh.value(CellRef::new(row, col)).scalar()
                && n.is_finite()
            {
                lo = lo.min(n);
                hi = hi.max(n);
            }
        }
        if lo <= hi && datetime_from_serial(wb.date_system, lo).is_some() && datetime_from_serial(wb.date_system, hi).is_some() {
            return Some((lo, hi));
        }
    }
    None
}

/// Whether every attribute of `e` is one of `known`, or the file format's documented default for it
/// (writing the element without the attribute then says the same thing).
fn attrs_match(e: &El, known: &[&str], defaults: &[(&str, &str)]) -> bool {
    e.attrs.iter().all(|(k, v)| known.contains(&k.as_str()) || defaults.iter().any(|(d, dv)| *d == k.as_str() && default_value(v, dv)))
}

/// Whether `value` says what the document's `default` says (booleans are spelled `1`/`0` or
/// `true`/`false`).
fn default_value(value: &str, default: &str) -> bool {
    match (bool_of(value), bool_of(default)) {
        (Some(a), Some(b)) => a == b,
        _ => value.trim().eq_ignore_ascii_case(default),
    }
}

fn bool_of(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    }
}

/// The timelines read from one workbook: the views (with their sheet), and the parts to strip.
struct TimelinesRead {
    views: Vec<(usize, PivotTimeline)>,
    caches: HashSet<String>,
    parts: HashSet<String>,
    /// The drawing frames consumed: `(view name, timeline)`.
    placed: HashSet<(String, bool)>,
}

fn read_timelines(wb: &mut Workbook, cx: &mut Ctx<'_>) -> Option<TimelinesRead> {
    let cache_parts = kept_parts(wb, CT_TIMELINE_CACHE);
    let parts = kept_parts(wb, CT_TIMELINE);
    if cache_parts.is_empty() || parts.is_empty() {
        return Some(TimelinesRead { views: vec![], caches: HashSet::new(), parts: HashSet::new(), placed: HashSet::new() });
    }
    // A timeline part is related from the sheet it is drawn on (the sheet's `<extLst>` holds its
    // `r:id`), and names its cache. Cache parts name the PivotTables they filter.
    let mut out = TimelinesRead { views: vec![], caches: HashSet::new(), parts: HashSet::new(), placed: HashSet::new() };
    let mut why: Option<String> = None;
    'sheets: for si in 0..wb.sheets.len() {
        let Some(p) = wb.sheets.get(si).and_then(|s| s.preserved.as_deref()) else { continue };
        let targets: Vec<String> = p.rels.iter().filter(|r| !r.external && rel_kind(&r.rel_type, "timeline")).map(|r| r.target.clone()).collect();
        for target in targets {
            let Some(part) = parts_by_name(&parts, &target) else {
                why = Some(format!("{target}: the timeline part is missing"));
                break 'sheets;
            };
            if !part.rels.is_empty() {
                why = Some(format!("{}: it has relationships GridCraft does not model", part.name));
                break 'sheets;
            }
            let Ok(root) = xml::parse(&part.data) else {
                why = Some(format!("{}: not XML", part.name));
                break 'sheets;
            };
            if root.name != "timelines" {
                why = Some(format!("{}: not a timelines part", part.name));
                break 'sheets;
            }
            if root.kids("timeline").count() == 0 {
                continue;
            }
            for el in root.kids("timeline") {
                let Some(cache_name) = el.attr("cache") else {
                    why = Some(format!("{}: a timeline names no cache", part.name));
                    break 'sheets;
                };
                let Some(cache) = cache_parts
                    .iter()
                    .find(|c| xml::parse(&c.data).ok().and_then(|x| x.attr("name").map(|n| n.eq_ignore_ascii_case(cache_name))).unwrap_or(false))
                else {
                    why = Some(format!("{cache_name}: the timeline cache is missing"));
                    break 'sheets;
                };
                if !cache.rels.is_empty() {
                    why = Some(format!("{}: it has relationships GridCraft does not model", cache.name));
                    break 'sheets;
                }
                let Ok(croot) = xml::parse(&cache.data) else {
                    why = Some(format!("{}: not XML", cache.name));
                    break 'sheets;
                };
                let state = match timeline_cache_problem(wb, &croot) {
                    Ok(s) => s,
                    Err(e) => {
                        why = Some(format!("{}: {e}", cache.name));
                        break 'sheets;
                    }
                };
                let Some(mut widget) = timeline_widget(el, cache_name) else {
                    why = Some(format!("{}: a timeline has an attribute GridCraft does not model", part.name));
                    break 'sheets;
                };
                let Some(pivots) = pivot_table_names(&croot) else {
                    why = Some(format!("{}: its PivotTable list is empty or damaged", cache.name));
                    break 'sheets;
                };
                if pivots.iter().any(|n| pivot_sheet(wb, n).is_none()) {
                    why = Some(format!("{}: it names a PivotTable this workbook does not have", cache.name));
                    break 'sheets;
                }
                if out.views.len() >= MAX_WIDGETS {
                    why = Some("too many timelines".into());
                    break 'sheets;
                }
                widget.pivot_tables = pivots.clone();
                widget.field = croot.attr("sourceName").unwrap_or("").to_string();
                if let Some(id) = state.state.attr_u32("pivotCacheId") {
                    widget.pivot_cache_id = id;
                }
                widget.range = state.range;
                if widget.caption.is_empty() {
                    widget.caption = widget.field.clone();
                }
                if let Some(first) = pivots.first() {
                    widget.anchor = anchor_beside(wb, si, first, widget.anchor.width, widget.anchor.height);
                }
                widget.anchor = frame_anchor(cx, &widget.name, true).unwrap_or(widget.anchor);
                out.placed.insert((widget.name.to_lowercase(), true));
                out.caches.insert(normalize(&cache.name));
                out.views.push((si, widget));
            }
            out.parts.insert(normalize(&part.name));
        }
    }
    if let Some(why) = why {
        cx.warn(format!("timelines are kept as they came (they are not editable): {why}"));
        return None;
    }
    Some(out)
}

/// What a timeline cache's `<state>` says, besides the PivotTable list: the element (for its
/// `pivotCacheId`) and the selected date range, if it has one.
struct TimelineState<'a> {
    state: &'a El,
    range: Option<TimelineRange>,
}

/// Why a timeline cache cannot be modelled, if it cannot: a state, filter or range this model does
/// not carry, an extension, or a child we do not know.
fn timeline_cache_problem<'a>(wb: &Workbook, root: &'a El) -> Result<TimelineState<'a>, String> {
    if root.name != "timelineCacheDefinition" {
        return Err("not a timeline cache definition".into());
    }
    let mut state: Option<&El> = None;
    for c in &root.children {
        match c.name.as_str() {
            "pivotTables" => {}
            "state" => state = Some(c),
            other => return Err(format!("<{other}> is not modelled")),
        }
    }
    if root.attr("name").is_none() || root.attr("sourceName").is_none() {
        return Err("it needs a name and a source field".into());
    }
    if !attrs_match(root, &["name", "sourceName", "Ignorable"], &[]) {
        return Err("an attribute of the cache is not modelled".into());
    }
    if root.child("extLst").is_some() {
        return Err("its extensions are not modelled".into());
    }
    let Some(state) = state else { return Err("it has no state".into()) };
    // The refresh versions and the single-range flag are not modelled: only the values GridCraft
    // writes itself can be written back.
    let defaults: [(&str, &str); 3] = [("minimalRefreshVersion", "0"), ("lastRefreshVersion", "0"), ("singleRangeFilterState", "true")];
    if !attrs_match(state, &["pivotCacheId", "filterType"], &defaults) {
        return Err("its state carries a refresh version or a filter state GridCraft does not model".into());
    }
    // `filterType` and the selection are one statement: `dateBetween` means a range, `unknown`
    // means none. Any other type, or one contradicting the children, is not ours to rewrite.
    let filter = state.attr("filterType").unwrap_or("unknown").trim().to_ascii_lowercase();
    let mut range: Option<TimelineRange> = None;
    for c in &state.children {
        match c.name.as_str() {
            "bounds" => {}
            "selection" => {
                if range.is_some() {
                    return Err("its state has more than one selection".into());
                }
                if filter != "datebetween" {
                    return Err("its state has a selection its filter type does not name".into());
                }
                range = Some(timeline_range(wb, c)?);
            }
            other => return Err(format!("<{other}> in its state is not modelled")),
        }
    }
    match (filter.as_str(), range.is_some()) {
        ("datebetween", true) | ("unknown", false) => {}
        ("datebetween", false) => return Err("its filter type names a date range it does not carry".into()),
        ("unknown", true) => return Err("its state says no date filter but carries a selection".into()),
        (other, _) => return Err(format!("its date filter type `{other}` is not modelled")),
    }
    Ok(TimelineState { state, range })
}

/// The range a `<selection>` states. Its two dates are required and the start must not be after
/// the end; a date written in a form a save could not spell back exactly is refused so the part
/// stays byte-identical rather than being rewritten in another spelling.
fn timeline_range(wb: &Workbook, el: &El) -> Result<TimelineRange, String> {
    if !el.children.is_empty() {
        return Err("its selection has children that are not modelled".into());
    }
    if !attrs_match(el, &["startDate", "endDate"], &[]) {
        return Err("an attribute of its selection is not modelled".into());
    }
    let (Some(start), Some(end)) = (el.attr("startDate"), el.attr("endDate")) else {
        return Err("its selection needs a start and an end date".into());
    };
    let (Some(start), Some(end)) = (timeline_date(wb, start), timeline_date(wb, end)) else {
        return Err("a date of its selection is not one GridCraft writes back verbatim".into());
    };
    if start > end {
        return Err("its selection ends before it starts".into());
    }
    Ok(TimelineRange::new(start, end))
}

/// The serial a timeline cache's date attribute holds, or `None` unless the text is exactly the
/// `YYYY-MM-DDThh:mm:ss` form [`iso_datetime`] writes for it (a fraction of a second, a zone or a
/// short date would be lost by writing it back, so such a file stays on the kept path).
fn timeline_date(wb: &Workbook, text: &str) -> Option<f64> {
    let (date, time) = text.split_once('T')?;
    let mut d = date.split('-');
    let (y, m, day) = (d.next()?.parse::<i64>().ok()?, d.next()?.parse::<i64>().ok()?, d.next()?.parse::<i64>().ok()?);
    if d.next().is_some() {
        return None;
    }
    let mut t = time.split(':');
    let (h, min, sec) = (t.next()?.parse::<i64>().ok()?, t.next()?.parse::<i64>().ok()?, t.next()?.parse::<i64>().ok()?);
    if t.next().is_some() || !(0..=23).contains(&h) || !(0..=59).contains(&min) || !(0..=59).contains(&sec) {
        return None;
    }
    let serial = serial_from_ymd(wb.date_system, y, m, day)? + time_fraction(h as f64, min as f64, sec as f64);
    (iso_datetime(wb, serial).as_deref() == Some(text)).then_some(serial)
}

fn timeline_widget(el: &El, cache: &str) -> Option<PivotTimeline> {
    if !el.children.is_empty() {
        return None;
    }
    // `selectionLevel` is written from the model (the level of a selection, which is the level
    // shown when nothing narrower was picked).
    let defaults: [(&str, &str); 4] =
        [("showHeader", "true"), ("showSelectionLabel", "true"), ("showTimeLevel", "true"), ("showHorizontalScrollbar", "true")];
    if !attrs_match(el, &["name", "cache", "caption", "style", "level", "selectionLevel"], &defaults) {
        return None;
    }
    let mut widget = PivotTimeline::default();
    let mut cache_seen = false;
    for (k, v) in &el.attrs {
        match k.as_str() {
            "name" => widget.name = v.clone(),
            "cache" => {
                cache_seen = true;
                widget.cache = v.clone();
                if !v.eq_ignore_ascii_case(cache) {
                    return None;
                }
            }
            "caption" => widget.caption = v.clone(),
            "style" => widget.style = v.clone(),
            "level" => widget.unit = TimeUnit::from_level(v.trim().parse::<u32>().ok()?)?,
            // The level a selection was made at; the model has no range selection, so it must be the
            // level shown.
            "selectionLevel" if TimeUnit::from_level(v.trim().parse::<u32>().ok()?)? != widget.unit => return None,
            _ => {}
        }
    }
    if !cache_seen || widget.name.trim().is_empty() {
        return None;
    }
    Some(widget)
}

// ================================================================ writing

/// A pivot slicer that survives to the file.
struct OutSlicer {
    sheet: usize,
    widget: PivotSlicer,
}

/// A table slicer that survives to the file: the view name and cache name are derived from the
/// table and column (the table slicer model names neither), so both are stable across saves.
struct OutTableSlicer {
    sheet: usize,
    widget: Slicer,
    name: String,
    cache: String,
}

/// A timeline that survives to the file.
struct OutTimeline {
    sheet: usize,
    widget: PivotTimeline,
    /// The source column's minimum and maximum, in serial days: the timeline cache's `bounds`.
    bounds: (f64, f64),
    /// Part names, e.g. `xl/timelineCaches/timelineCache1.xml`.
    cache_part: String,
}

/// One slicer cache written: the model's widgets that share its name, and the field's values.
struct OutCache {
    name: String,
    field: String,
    pivots: Vec<String>,
    items: Vec<SlicerItem>,
    part: String,
}

/// A table slicer cache written: its table (by the id the table writer gave it) and column (by the
/// one-based `tableColumn` id, which is the column's position).
struct OutTableCache {
    sheet: usize,
    name: String,
    table: String,
    source: String,
    column: u32,
    part: String,
}

/// A frame the sheet's drawing part places: a slicer or timeline view by name.
pub struct Frame<'a> {
    pub name: &'a str,
    pub anchor: &'a Anchor,
    pub timeline: bool,
}

/// Writes the modelled slicers and timelines of a workbook.
///
/// Constructed before the sheet loop (it decides which widgets can be written and claims their part
/// names), asked for each sheet's drawing frames ([`Self::frames`]) and its `<extLst>` entry
/// ([`Self::sheet_ext`]), then finished before the workbook XML is assembled ([`Self::finish`]
/// writes the parts, the workbook relationships and the workbook extension, and returns the
/// extension XML). The PivotTable writer takes the cache links afterwards ([`Self::links`]).
///
/// Every widget [`Self::new`] keeps is one [`Self::finish`] can write: the PivotTable's cache
/// resolves (the same check the cache writer makes), the table exists in the written file, and a
/// timeline's source has dates to bound it. That is what keeps a drawing frame from naming a view
/// whose part was not written.
#[derive(Default)]
pub struct SlicerWriter {
    /// Per sheet: the part holding its slicer views (the format requires exactly one per sheet).
    slicer_parts: Vec<Option<String>>,
    /// Per sheet: the part holding its timeline views.
    timeline_parts: Vec<Option<String>>,
    slicers: Vec<OutSlicer>,
    table_slicers: Vec<OutTableSlicer>,
    timelines: Vec<OutTimeline>,
    caches: Vec<OutCache>,
    table_caches: Vec<OutTableCache>,
    /// (PivotTable name, field) pairs whose pivot cache must itemize the field.
    pivot_fields: Vec<(String, String)>,
    links: Vec<CacheLink>,
    /// The workbook `<ext>` children written from the model.
    wb_ext: String,
    /// The `sheetId` each model sheet is written with, for a cache's `tabId`.
    sheet_ids: Vec<u32>,
}

impl SlicerWriter {
    /// Collects the widgets that can be written. A widget goes when its PivotTables are all gone or
    /// its PivotTable's source (or that source's field) can no longer be written: the kept path
    /// drops such a slicer too, and a cache naming a report the file lacks is a repair prompt. A
    /// table slicer goes when its table is not written or no longer has the column.
    pub fn new(wb: &Workbook, out: &mut Out) -> SlicerWriter {
        let mut w =
            SlicerWriter { slicer_parts: vec![None; wb.sheets.len()], timeline_parts: vec![None; wb.sheets.len()], ..SlicerWriter::default() };
        // A view's name is unique in the workbook, across slicers and timelines.
        let mut view_names: HashSet<String> = wb
            .sheets
            .iter()
            .flat_map(|s| s.pivot_slicers.iter().map(|x| x.name.to_lowercase()).chain(s.pivot_timelines.iter().map(|x| x.name.to_lowercase())))
            .collect();
        for (si, sheet) in wb.sheets.iter().enumerate() {
            for sl in &sheet.pivot_slicers {
                let Some(pivots) = live_pivots(wb, &sl.pivot_tables, Some(&sl.field)) else { continue };
                if w.slicers.len() + w.table_slicers.len() >= MAX_WIDGETS {
                    break;
                }
                let mut widget = sl.clone();
                widget.pivot_tables = pivots.clone();
                for p in &pivots {
                    if !w.pivot_fields.iter().any(|(name, f)| name.eq_ignore_ascii_case(p) && f.eq_ignore_ascii_case(&sl.field)) {
                        w.pivot_fields.push((p.clone(), sl.field.clone()));
                    }
                }
                // One cache per distinct name; the first widget's items are the cache's selection.
                if !w.caches.iter().any(|c| c.name.eq_ignore_ascii_case(&sl.cache))
                    && !w.table_caches.iter().any(|c| c.name.eq_ignore_ascii_case(&sl.cache))
                {
                    let n = w.caches.len() + w.table_caches.len() + 1;
                    let part = out.kept.claim(&format!("xl/slicerCaches/slicerCache{n}.xml"));
                    let items = if sl.items.is_empty() { Vec::new() } else { sl.items.clone() };
                    w.caches.push(OutCache { name: sl.cache.clone(), field: sl.field.clone(), pivots, items, part });
                }
                w.slicers.push(OutSlicer { sheet: si, widget });
            }
            for sl in &sheet.slicers {
                let Some(table) = sheet.tables.iter().find(|t| t.name.eq_ignore_ascii_case(&sl.table)) else { continue };
                if !crate::sheet_write::table_is_written(table) {
                    continue;
                }
                let Some(ci) = table.column_index(&sl.column) else { continue };
                if w.slicers.len() + w.table_slicers.len() >= MAX_WIDGETS {
                    break;
                }
                let Some(name) = unique_name(&format!("{}_{}", table.name, sl.column), &mut view_names) else { continue };
                // The cache's `sourceName` is the column name the written table part has (a table
                // with an empty or duplicated column name gets a derived one).
                let source = crate::sheet_write::table_columns(table).get(ci).cloned().unwrap_or_else(|| sl.column.clone());
                // Two views of one column share their cache, as Excel shares one cache per field.
                let cache =
                    match w.table_caches.iter().find(|c| c.sheet == si && c.table.eq_ignore_ascii_case(&table.name) && c.column == ci as u32 + 1) {
                        Some(c) => c.name.clone(),
                        None => {
                            let Some(cache) = unique_cache_name(&format!("Slicer_{}_{}", table.name, sl.column), &w) else { continue };
                            let n = w.caches.len() + w.table_caches.len() + 1;
                            let part = out.kept.claim(&format!("xl/slicerCaches/slicerCache{n}.xml"));
                            w.table_caches.push(OutTableCache {
                                sheet: si,
                                name: cache.clone(),
                                table: table.name.clone(),
                                source,
                                column: ci as u32 + 1,
                                part,
                            });
                            cache
                        }
                    };
                w.table_slicers.push(OutTableSlicer { sheet: si, widget: sl.clone(), name, cache });
            }
            for tl in &sheet.pivot_timelines {
                let Some(pivots) = live_pivots(wb, &tl.pivot_tables, None) else { continue };
                if w.timelines.len() >= MAX_WIDGETS {
                    break;
                }
                let Some(bounds) = timeline_bounds(wb, si, &pivots, &tl.field) else { continue };
                let n = w.timelines.len() + 1;
                let cache_part = out.kept.claim(&format!("xl/timelineCaches/timelineCache{n}.xml"));
                let mut widget = tl.clone();
                widget.pivot_tables = pivots;
                w.timelines.push(OutTimeline { sheet: si, widget, bounds, cache_part });
            }
            if w.slicers.iter().any(|s| s.sheet == si) || w.table_slicers.iter().any(|s| s.sheet == si) {
                let n = w.slicer_parts.iter().filter(|p| p.is_some()).count() + 1;
                w.slicer_parts[si] = Some(out.kept.claim(&format!("xl/slicers/slicer{n}.xml")));
            }
            if w.timelines.iter().any(|t| t.sheet == si) {
                let n = w.timeline_parts.iter().filter(|p| p.is_some()).count() + 1;
                w.timeline_parts[si] = Some(out.kept.claim(&format!("xl/timelines/timeline{n}.xml")));
            }
        }
        w
    }

    /// The `sheetId` each model sheet was written with (the cache's `pivotTable/@tabId`).
    pub fn set_sheet_ids(&mut self, ids: Vec<u32>) {
        self.sheet_ids = ids;
    }

    /// The sheet's drawing frames: the slicer views placed on it, then its timeline views.
    pub fn frames(&self, si: usize) -> Vec<Frame<'_>> {
        let mut out = Vec::new();
        for s in self.slicers.iter().filter(|s| s.sheet == si) {
            out.push(Frame { name: &s.widget.name, anchor: &s.widget.anchor, timeline: false });
        }
        for s in self.table_slicers.iter().filter(|s| s.sheet == si) {
            out.push(Frame { name: &s.name, anchor: &s.widget.anchor, timeline: false });
        }
        for t in self.timelines.iter().filter(|t| t.sheet == si) {
            out.push(Frame { name: &t.widget.name, anchor: &t.widget.anchor, timeline: true });
        }
        out
    }

    /// The fields a PivotTable's cache must itemize for the modelled slicers that filter it.
    pub fn fields_for_pivot(&self, pivot: &str) -> Vec<String> {
        self.pivot_fields.iter().filter(|(p, _)| p.eq_ignore_ascii_case(pivot)).map(|(_, f)| f.clone()).collect()
    }

    /// The sheet's extension entries naming its slicers and timelines parts, adding a sheet
    /// relationship per part written on it.
    pub fn sheet_ext(&self, si: usize, rels: &mut Rels) -> String {
        let mut x = String::new();
        if let Some(part) = self.slicer_parts.get(si).and_then(|p| p.as_deref()) {
            let rid = rels.add_uri(REL_SLICER, &from_part_dir(part));
            let _ = write!(
                x,
                "<ext uri=\"{EXT_SLICER_LIST}\" xmlns:x14=\"{NS_X14}\"><x14:slicerList><x14:slicer r:id=\"{rid}\"/></x14:slicerList></ext>"
            );
        }
        if let Some(part) = self.timeline_parts.get(si).and_then(|p| p.as_deref()) {
            let rid = rels.add_uri(REL_TIMELINE, &from_part_dir(part));
            let _ = write!(
                x,
                "<ext uri=\"{EXT_TIMELINE_REFS}\" xmlns:x15=\"{NS_X15}\"><x15:timelineRefs><x15:timelineRef r:id=\"{rid}\"/></x15:timelineRefs></ext>"
            );
        }
        x
    }

    /// The cache links for the PivotTable writer (one per PivotTable a written cache filters).
    pub fn links(&self) -> &[CacheLink] {
        &self.links
    }

    /// Writes the slicer/timeline parts and their caches, adds the workbook relationships, and
    /// returns the workbook `<ext>` children (slicer caches, timeline cache references).
    pub fn finish(&mut self, wb: &Workbook, out: &mut Out, wb_rels: &mut Rels, pivots: &PivotWriter) -> String {
        let mut slicer_ext = String::new();
        let mut timeline_ext = String::new();
        let mut links: Vec<CacheLink> = Vec::new();
        // The caches that reached the file: a view is only written when its cache was, so no part
        // names a cache the package does not have.
        let mut written_caches: HashSet<String> = HashSet::new();
        let mut written_timelines: HashSet<String> = HashSet::new();
        // Slicer caches: a widget whose PivotTable lost its cache is not written.
        for cache in &self.caches {
            let widgets: Vec<&OutSlicer> = self.slicers.iter().filter(|s| s.widget.cache.eq_ignore_ascii_case(&cache.name)).collect();
            if widgets.is_empty() {
                continue;
            }
            written_caches.insert(cache.name.to_lowercase());
            // The cache and its item list when the PivotTable's cache exists and itemizes the field;
            // otherwise a cache with only its PivotTable list, which the format allows and which
            // shows every tile (a view must never name a cache the package does not have).
            let cache_id = cache.pivots.iter().find_map(|p| pivots.cache_id(wb, p));
            let values = cache_id.and_then(|id| pivots.field_items(id, &cache.field));
            let selected: HashMap<String, bool> = cache.items.iter().map(|i| (i.value.key(), i.selected)).collect();
            let mut part_xml = format!(
                "{XML_DECL}<slicerCacheDefinition xmlns=\"{NS_X14}\" name=\"{}\" sourceName=\"{}\"><pivotTables>",
                esc_attr(&cache.name),
                esc_attr(&cache.field)
            );
            for p in &cache.pivots {
                let _ = write!(part_xml, "<pivotTable tabId=\"{}\" name=\"{}\"/>", self.tab_id(wb, p), esc_attr(p));
            }
            part_xml.push_str("</pivotTables>");
            if let (Some(cache_id), Some(values)) = (cache_id, values) {
                part_xml.push_str("<data>");
                if values.is_empty() {
                    let _ = write!(part_xml, "<tabular pivotCacheId=\"{cache_id}\"/>");
                } else {
                    let _ = write!(part_xml, "<tabular pivotCacheId=\"{cache_id}\"><items count=\"{}\">", values.len());
                    for (x, v) in values.iter().enumerate() {
                        // A value added to the source since the slicer was made is selected, as Excel
                        // selects a new item; only a tile the model marks off is written off.
                        let on = selected.get(&SlicerValue::from_value(v).key()).copied().unwrap_or(true);
                        if on {
                            let _ = write!(part_xml, "<i x=\"{x}\" s=\"1\"/>");
                        } else {
                            let _ = write!(part_xml, "<i x=\"{x}\"/>");
                        }
                    }
                    part_xml.push_str("</items></tabular>");
                }
                part_xml.push_str("</data>");
            }
            part_xml.push_str("</slicerCacheDefinition>");
            out.part(&cache.part, Some(CT_SLICER_CACHE), part_xml.into_bytes());
            let rid = wb_rels.add_uri(REL_SLICER_CACHE, &from_workbook(&cache.part));
            let _ = write!(slicer_ext, "<x14:slicerCache r:id=\"{rid}\"/>");
            for p in &cache.pivots {
                links.push(CacheLink { pivot: p.clone(), cache_part: cache.part.clone() });
            }
        }
        // Table slicer caches: the table's id is the one its part got, so the cache names a table
        // this file has.
        for cache in &self.table_caches {
            let Some(table_id) =
                out.table_ids.iter().find(|(si, name, _)| *si == cache.sheet && name.eq_ignore_ascii_case(&cache.table)).map(|(_, _, id)| *id)
            else {
                continue;
            };
            let part_xml = format!(
                "{XML_DECL}<slicerCacheDefinition xmlns=\"{NS_X14}\" name=\"{}\" sourceName=\"{}\"><extLst><ext uri=\"{EXT_TABLE_SLICER_CACHE}\" xmlns:x15=\"{NS_X15}\"><x15:tableSlicerCache tableId=\"{table_id}\" column=\"{}\"/></ext></extLst></slicerCacheDefinition>",
                esc_attr(&cache.name),
                esc_attr(&cache.source),
                cache.column
            );
            out.part(&cache.part, Some(CT_SLICER_CACHE), part_xml.into_bytes());
            let rid = wb_rels.add_uri(REL_SLICER_CACHE, &from_workbook(&cache.part));
            let _ = write!(slicer_ext, "<x14:slicerCache r:id=\"{rid}\"/>");
            written_caches.insert(cache.name.to_lowercase());
        }
        // The views, one part per sheet.
        for (si, part) in self.slicer_parts.iter().enumerate() {
            let Some(part) = part else { continue };
            let mut views = String::new();
            for s in self.slicers.iter().filter(|s| s.sheet == si && written_caches.contains(&s.widget.cache.to_lowercase())) {
                views.push_str(&slicer_view_xml(&s.widget.name, &s.widget.cache, &s.widget.caption, &s.widget.style, s.widget.columns));
            }
            for s in self.table_slicers.iter().filter(|s| s.sheet == si && written_caches.contains(&s.cache.to_lowercase())) {
                views.push_str(&slicer_view_xml(&s.name, &s.cache, &s.widget.caption, "", s.widget.columns));
            }
            let xml = format!("{XML_DECL}<slicers xmlns=\"{NS_X14}\">{views}</slicers>");
            out.part(part, Some(CT_SLICER), xml.into_bytes());
        }
        for tl in &self.timelines {
            let pivot_names = tl.widget.pivot_tables.clone();
            // The cache id the pivot cache was written with; a timeline read from a file keeps the
            // id it came with as the fallback.
            let Some(pivot_cache_id) =
                pivot_names.iter().find_map(|p| pivots.cache_id(wb, p)).or(Some(tl.widget.pivot_cache_id).filter(|id| *id > 0))
            else {
                continue;
            };
            let mut cache_xml = format!(
                "{XML_DECL}<timelineCacheDefinition xmlns=\"{NS_X15}\" name=\"{}\" sourceName=\"{}\"><pivotTables>",
                esc_attr(&tl.widget.cache),
                esc_attr(&tl.widget.field)
            );
            for p in &pivot_names {
                let _ = write!(cache_xml, "<pivotTable tabId=\"{}\" name=\"{}\"/>", self.tab_id(wb, p), esc_attr(p));
            }
            // The state: the date range the model holds (if any), no refresh version, and the
            // bounds the source data gives the timeline. `filterType` and the selection are written
            // together, as the format pairs them.
            let (start, end) = (iso_date(wb, tl.bounds.0), iso_date(wb, tl.bounds.1));
            let selection = tl.widget.range.and_then(|r| timeline_selection_xml(wb, r));
            let filter_type = if selection.is_some() { "dateBetween" } else { "unknown" };
            let _ = write!(
                cache_xml,
                "</pivotTables><state minimalRefreshVersion=\"0\" lastRefreshVersion=\"0\" pivotCacheId=\"{pivot_cache_id}\" filterType=\"{filter_type}\" singleRangeFilterState=\"1\">{}<bounds startDate=\"{start}\" endDate=\"{end}\"/></state></timelineCacheDefinition>",
                selection.unwrap_or_default()
            );
            out.part(&tl.cache_part, Some(CT_TIMELINE_CACHE), cache_xml.into_bytes());
            let rid = wb_rels.add_uri(REL_TIMELINE_CACHE, &from_workbook(&tl.cache_part));
            let _ = write!(timeline_ext, "<x15:timelineCacheRef r:id=\"{rid}\"/>");
            written_timelines.insert(tl.widget.cache.to_lowercase());
        }
        for (si, part) in self.timeline_parts.iter().enumerate() {
            let Some(part) = part else { continue };
            let mut views = String::new();
            for t in self.timelines.iter().filter(|t| t.sheet == si && written_timelines.contains(&t.widget.cache.to_lowercase())) {
                views.push_str(&timeline_view_xml(&t.widget));
            }
            let xml = format!("{XML_DECL}<timelines xmlns=\"{NS_X15}\">{views}</timelines>");
            out.part(part, Some(CT_TIMELINE), xml.into_bytes());
        }
        self.links = links;
        self.wb_ext = String::new();
        if !slicer_ext.is_empty() {
            let _ = write!(
                self.wb_ext,
                "<ext uri=\"{EXT_SLICER_CACHES}\" xmlns:x14=\"{NS_X14}\"><x14:slicerCaches>{slicer_ext}</x14:slicerCaches></ext>"
            );
        }
        if !timeline_ext.is_empty() {
            let _ = write!(
                self.wb_ext,
                "<ext uri=\"{EXT_TIMELINE_CACHE_REFS}\" xmlns:x15=\"{NS_X15}\"><x15:timelineCacheRefs>{timeline_ext}</x15:timelineCacheRefs></ext>"
            );
        }
        self.wb_ext.clone()
    }

    /// The `sheetId` (a one-based id of the workbook's `<sheet>` entries) of the sheet a PivotTable
    /// is on; `tabId` names a sheet by that id ([MS-XLSX] 2.6.73).
    fn tab_id(&self, wb: &Workbook, pivot: &str) -> u32 {
        let si = wb.sheets.iter().position(|s| s.pivots.iter().any(|p| p.name.eq_ignore_ascii_case(pivot))).unwrap_or(0);
        self.sheet_ids.get(si).copied().unwrap_or(si as u32 + 1)
    }
}

/// One `<slicer>` view: the attributes the model holds ([MS-XLSX] 2.6.68).
fn slicer_view_xml(name: &str, cache: &str, caption: &str, style: &str, columns: u32) -> String {
    let mut s = format!("<slicer name=\"{}\" cache=\"{}\"", esc_attr(name), esc_attr(cache));
    if !caption.is_empty() {
        let _ = write!(s, " caption=\"{}\"", esc_attr(caption));
    }
    if !style.is_empty() {
        let _ = write!(s, " style=\"{}\"", esc_attr(style));
    }
    // 1..=20000 in the schema; the model keeps the smaller Excel UI range.
    let _ = write!(s, " columnCount=\"{}\"/>", columns.clamp(1, 64));
    s
}

/// One `<timeline>` view ([MS-XLSX] 2.6.111). `level` and `selectionLevel` are required; the
/// model has one time unit, so a selection is made at the level shown (a file whose
/// `selectionLevel` differs is not modelled — see [`timeline_widget`]).
fn timeline_view_xml(tl: &PivotTimeline) -> String {
    let mut s = format!("<timeline name=\"{}\" cache=\"{}\"", esc_attr(&tl.name), esc_attr(&tl.cache));
    if !tl.caption.is_empty() {
        let _ = write!(s, " caption=\"{}\"", esc_attr(&tl.caption));
    }
    let level = tl.unit.level();
    let _ = write!(s, " level=\"{level}\" selectionLevel=\"{level}\"");
    if !tl.style.is_empty() {
        let _ = write!(s, " style=\"{}\"", esc_attr(&tl.style));
    }
    s.push_str("/>");
    s
}

/// The `<selection>` a timeline's range writes, or `None` when the model's range is not a date
/// range a reader could use (reversed, non-finite, or a serial no date system can show); the
/// cache then states no filter, which is what `filterType="unknown"` means.
fn timeline_selection_xml(wb: &Workbook, range: TimelineRange) -> Option<String> {
    if !range.is_valid() {
        return None;
    }
    let start = iso_datetime(wb, range.start)?;
    let end = iso_datetime(wb, range.end)?;
    Some(format!("<selection startDate=\"{start}\" endDate=\"{end}\"/>"))
}

/// The serial `serial` as an ISO 8601 date and time, the form a timeline cache's dates use.
/// `None` when no date can be written for it — including serial 0, the 1900 system's
/// day-before-the-epoch that would spell as the impossible `1900-01-00`.
fn iso_datetime(wb: &Workbook, serial: f64) -> Option<String> {
    let d = datetime_from_serial(wb.date_system, serial)?;
    if d.day == 0 {
        return None;
    }
    Some(format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", d.year, d.month, d.day, d.hour, d.minute, d.second))
}

/// [`iso_datetime`] for a value a timeline's `bounds` must have (a finite serial from a cell).
fn iso_date(wb: &Workbook, serial: f64) -> String {
    // Unreachable for a bounds value (the source's finite minimum and maximum), and a date is
    // required by the schema, so the 1900 epoch stands in rather than writing nothing.
    iso_datetime(wb, serial).unwrap_or_else(|| "1900-01-01T00:00:00".to_string())
}

/// The PivotTables of `names` whose source (and, when `field` is given, that field) can be
/// written, in the model's order.
fn live_pivots(wb: &Workbook, names: &[String], field: Option<&str>) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for name in names {
        let Some((si, pi)) =
            wb.sheets.iter().enumerate().find_map(|(si, s)| s.pivots.iter().position(|p| p.name.eq_ignore_ascii_case(name)).map(|pi| (si, pi)))
        else {
            continue;
        };
        let Some(pt) = wb.sheet(si).and_then(|s| s.pivots.get(pi)) else { continue };
        let Some((_, fields)) = crate::pivot::source_fields(wb, pt, si) else { continue };
        if field.is_some_and(|f| !fields.iter().any(|x| x.eq_ignore_ascii_case(f))) {
            continue;
        }
        out.push(name.clone());
    }
    if out.is_empty() { None } else { Some(out) }
}

/// A view name that is not already taken (case-insensitively), sanitised so it is a legal defined
/// name (letters, digits and `_`, not a cell reference). `None` when every candidate is taken.
fn unique_name(wanted: &str, taken: &mut HashSet<String>) -> Option<String> {
    let mut base: String = wanted.chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' }).take(200).collect();
    if base.is_empty() || !base.starts_with(|c: char| c.is_alphabetic() || c == '_') || gridcraft_core::CellRef::parse(&base).is_some() {
        base = format!("Slicer{base}");
    }
    if taken.insert(base.to_lowercase()) {
        return Some(base);
    }
    // More candidates than names can be taken (`taken` holds at most two per widget).
    for k in 2..=MAX_WIDGETS as u32 * 2 + 2 {
        let candidate = format!("{base}{k}");
        if taken.insert(candidate.to_lowercase()) {
            return Some(candidate);
        }
    }
    None
}

/// A cache name not already used by a pivot or table slicer cache; `None` when every candidate is.
fn unique_cache_name(wanted: &str, w: &SlicerWriter) -> Option<String> {
    let base: String = wanted.trim().chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' }).take(200).collect();
    let base = if base.is_empty() || !base.starts_with(|c: char| c.is_alphabetic() || c == '_') || gridcraft_core::CellRef::parse(&base).is_some() {
        format!("Slicer_{base}")
    } else {
        base
    };
    let taken = |n: &str| w.caches.iter().any(|c| c.name.eq_ignore_ascii_case(n)) || w.table_caches.iter().any(|c| c.name.eq_ignore_ascii_case(n));
    if !taken(&base) {
        return Some(base);
    }
    (2..=MAX_WIDGETS as u32 * 2 + 2).map(|i| format!("{base}{i}")).find(|n| !taken(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_links_are_relativised_from_the_pivot_table_part() {
        assert_eq!(from_part_dir("xl/slicerCaches/slicerCache1.xml"), "../slicerCaches/slicerCache1.xml");
        assert_eq!(from_part_dir("other/x.xml"), "../other/x.xml");
    }

    #[test]
    fn a_derived_name_is_legal_and_unique() {
        let mut taken: HashSet<String> = ["Region_Table1".to_lowercase()].into_iter().collect();
        assert_eq!(unique_name("Region_Table1", &mut taken).as_deref(), Some("Region_Table12"));
        assert_eq!(unique_name("My Table/1", &mut taken).as_deref(), Some("My_Table_1"));
        assert_eq!(unique_name("1", &mut taken).as_deref(), Some("Slicer1"));
        assert_eq!(unique_name("A1", &mut taken).as_deref(), Some("SlicerA1"));
    }

    #[test]
    fn documented_defaults_count_as_absent() {
        assert!(default_value("1", "true") && default_value("TRUE", "true"));
        assert!(default_value("0", "false"));
        assert!(!default_value("0", "true"));
        assert!(default_value("ascending", "ascending"));
        assert!(!default_value("natural", "ascending"));
    }
}
