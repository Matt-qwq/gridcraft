//! Slicers and timelines in `.xlsx`: the model side of Excel's Insert ▸ Slicer / Insert ▸ Timeline.
//!
//! Reading models the parts `passthrough` used to keep byte for byte: a *slicer cache* (the field
//! it shows, the PivotTables it filters, and one `<i x s>` item per value with its selected flag),
//! the *slicer* parts (name, caption, style, column count) and the same pair for timelines. The
//! model's tiles are read from the pivot cache's shared items (`x` indexes them), so a save writes
//! the same kind of value back (`<s>`, `<n>`, `<b>`, `<e>`, `<m>` for a blank).
//!
//! Writing is from the model, once, and the pass-through is kept out of it — the same contract as
//! the model's PivotTable extensions (`owner_extras`):
//! - a slicer or timeline that was modelled is written by [`SlicerWriter`]; on read, the kept
//!   relationship that reached its cache (the workbook's `<x14:slicerCaches>` list, the
//!   PivotTable's `<x14:slicerCacheData>` extension, the sheet's `<x15:timelines>` list) is removed
//!   from the preserved content, so the kept path cannot write it a second time;
//! - content that was **not** modelled (an OLAP cache, an item list that is not exactly the field's
//!   shared items, a slicer attribute we do not know, a timeline with a selected date range, a
//!   damaged part) is not touched: every part, relationship and extension stays as it is, and a
//!   save keeps it byte for byte;
//! - modelling is all-or-nothing for each kind: one cache GridCraft cannot fully understand leaves
//!   *every* slicer in that workbook on the kept path, and the same for timelines. That is stricter
//!   than per-part modelling, and it means a model is never a partial rewrite of the same lists;
//! - a modelled slicer/timeline whose PivotTable is gone is not written (its cache goes with it),
//!   matching the kept path's rule for a cache that names a deleted or renamed PivotTable.
//!
//! The model is built from the parts the pass-through kept, so reading with the pass-through off
//! has no slicer content to model (nothing is written back either, as for every other kept part).
//!
//! Not modelled (kept raw, and said so here so the next reader does not guess): OLAP slicer caches,
//! item properties (`<p>`), custom lists, slicer `extLst`s, a timeline's date-range selection, the
//! drawing shape that places a slicer on the sheet, and the slicer sort order (a file's slicer part
//! does not carry it; the model keeps it as a UI hint only).
//!
//! Clean room: built from ECMA-376 / ISO/IEC 29500 and the Microsoft extension parts of a package
//! (names and attributes of the file format). Nothing here is copied from Excel's artwork or
//! theme XML; a style is a name the file carries, and how it looks is GridCraft's own drawing code.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use gridcraft_core::CellRef;
use gridcraft_model::{Anchor, PivotSlicer, PivotTimeline, SlicerItem, SlicerSort, SlicerValue, TimeUnit, Workbook};

use crate::package::normalize;
use crate::pivot::PivotWriter;
use crate::read::Ctx;
use crate::write::{NS_MAIN, NS_REL, Out, Rels, XML_DECL};
use crate::xml::{self, El, esc_attr};

pub const CT_SLICER_CACHE: &str = "application/vnd.ms-excel.slicerCache+xml";
pub const CT_SLICER: &str = "application/vnd.ms-excel.slicer+xml";
pub const CT_TIMELINE_CACHE: &str = "application/vnd.ms-excel.timelineCache+xml";
pub const CT_TIMELINE: &str = "application/vnd.ms-excel.timeline+xml";

pub const REL_SLICER_CACHE: &str = "http://schemas.microsoft.com/office/2007/relationships/slicerCache";
pub const REL_SLICER: &str = "http://schemas.microsoft.com/office/2007/relationships/slicer";
pub const REL_TIMELINE_CACHE: &str = "http://schemas.microsoft.com/office/2011/relationships/timelineCache";
pub const REL_TIMELINE: &str = "http://schemas.microsoft.com/office/2011/relationships/timeline";

/// The 2009 extension namespace slicer parts and their workbook/PivotTable extensions use.
pub const NS_X14: &str = "http://schemas.microsoft.com/office/spreadsheetml/2009/9/main";
/// The 2010 extension namespace timeline parts and their sheet/workbook extensions use.
pub const NS_X15: &str = "http://schemas.microsoft.com/office/spreadsheetml/2010/11/main";

/// The workbook extension listing the slicer caches (`<x14:slicerCaches>`).
pub const EXT_SLICER_CACHES: &str = "{BBE1A952-6BD2-4e30-A5A6-4D6A0A0D6F2B}";
/// The workbook and sheet extension listing timelines and their caches.
pub const EXT_TIMELINES: &str = "{7E03D99C-DC04-49d9-9315-930204A7B6E9}";
/// The PivotTable extension that names the slicer caches filtering it.
pub const EXT_PIVOT_SLICER_CACHE: &str = "{A8765BA9-456A-4dab-B4F3-ACF838C121DE}";

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
    let mut x = format!("<ext uri=\"{EXT_PIVOT_SLICER_CACHE}\" xmlns:x14=\"{NS_X14}\"><x14:slicerCacheData>");
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
/// Called once, after every sheet, PivotTable and kept part has been read. Both kinds are modelled
/// all-or-nothing: anything GridCraft does not fully understand leaves that kind raw.
pub fn read(wb: &mut Workbook, cx: &mut Ctx<'_>, wb_xml: &El, wb_rels: &[crate::package::Rel]) {
    let items = pivot_cache_items(cx, wb_xml, wb_rels);
    let slicers = read_slicers(wb, cx, &items);
    let timelines = read_timelines(wb, cx);
    let mut cache_parts: HashSet<String> = HashSet::new();
    let mut slicer_parts: HashSet<String> = HashSet::new();
    let mut timeline_parts: HashSet<String> = HashSet::new();
    if let Some(list) = slicers {
        for (si, mut widget, cache_part, slicer_part) in list {
            // A widget's id is unique in the workbook, like every other on-sheet object.
            widget.id = cx.next_object_id();
            if let Some(sheet) = wb.sheet_mut(si)
                && sheet.pivot_slicers.len() < MAX_WIDGETS
            {
                sheet.pivot_slicers.push(widget);
            }
            cache_parts.insert(normalize(&cache_part));
            slicer_parts.insert(normalize(&slicer_part));
        }
    }
    if let Some(list) = timelines {
        for (si, mut widget, cache_part, timeline_part) in list {
            widget.id = cx.next_object_id();
            if let Some(sheet) = wb.sheet_mut(si)
                && sheet.pivot_timelines.len() < MAX_WIDGETS
            {
                sheet.pivot_timelines.push(widget);
            }
            cache_parts.insert(normalize(&cache_part));
            timeline_parts.insert(normalize(&timeline_part));
        }
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
/// report (the slicer part itself stores no position; the drawing shape does, and is not modelled).
fn anchor_beside(wb: &Workbook, si: usize, pivot: &str, width: f32, height: f32) -> Anchor {
    let cell = wb
        .sheet(si)
        .and_then(|s| s.pivots.iter().find(|p| p.name.eq_ignore_ascii_case(pivot)))
        .and_then(|p| p.last_range)
        .map(|r| CellRef::new(r.start.row, r.end.col.saturating_add(2)))
        .unwrap_or_default();
    Anchor { cell, dx: 4.0, dy: 4.0, width, height, mode: gridcraft_model::AnchorMode::MoveOnly }
}

/// A slicer cache read from a kept part, before its slicers are attached.
struct CacheRead {
    part: String,
    name: String,
    field: String,
    pivot_cache_id: u32,
    pivot_tables: Vec<String>,
    values: Vec<SlicerValue>,
    /// One per value: the selected flag. `None` = the file wrote no item list, i.e. all selected.
    selected: Option<Vec<bool>>,
}

fn read_slicers(
    wb: &mut Workbook,
    cx: &mut Ctx<'_>,
    items: &HashMap<u32, HashMap<String, Vec<SlicerValue>>>,
) -> Option<Vec<(usize, PivotSlicer, String, String)>> {
    let caches = kept_parts(wb, CT_SLICER_CACHE);
    if caches.is_empty() {
        return Some(vec![]);
    }
    let slicers = kept_parts(wb, CT_SLICER);
    let mut parsed: Vec<CacheRead> = Vec::new();
    let mut out = Vec::new();
    let mut why: Option<String> = None;
    'cache: for part in caches {
        if parsed.len() >= MAX_WIDGETS {
            why = Some("too many slicer caches".into());
            break;
        }
        let root = match xml::parse(&part.data) {
            Ok(x) if x.name == "slicerCacheDefinition" => x,
            _ => {
                why = Some(format!("{}: not a slicer cache definition", part.name));
                break;
            }
        };
        // Only a plain tabular cache is modelled: an OLAP one, an item property or any extension
        // would be lost.
        let mut tabular = None;
        for c in &root.children {
            match c.name.as_str() {
                "pivotTables" | "data" => {}
                "slicers" => {}
                _ => {
                    why = Some(format!("{}: <{}> is not modelled", part.name, c.name));
                    break 'cache;
                }
            }
        }
        if let Some(data) = root.child("data") {
            if data.children.len() != 1 || data.children.first().is_some_and(|t| t.name != "tabular") {
                why = Some(format!("{}: not a tabular cache", part.name));
                break;
            }
            tabular = data.child("tabular");
        }
        let (Some(name), Some(field), Some(tab)) = (root.attr("name"), root.attr("sourceName"), tabular) else {
            why = Some(format!("{}: a slicer cache needs a name, a source field and a table", part.name));
            break;
        };
        let Some(pivot_cache_id) = tab.attr_u32("pivotCacheId") else {
            why = Some(format!("{}: no pivotCacheId", part.name));
            break;
        };
        let Some(values) = items.get(&pivot_cache_id).and_then(|m| m.get(&field.to_lowercase())) else {
            why = Some(format!("{}: the pivot cache has no shared items for `{field}`", part.name));
            break;
        };
        let Some(pivot_tables) = pivot_table_names(&root) else {
            why = Some(format!("{}: its PivotTable list is empty or damaged", part.name));
            break;
        };
        if pivot_tables.iter().any(|n| pivot_sheet(wb, n).is_none()) {
            why = Some(format!("{}: it names a PivotTable this workbook does not have", part.name));
            break;
        }
        let selected = match read_selection(tab, values.len()) {
            Ok(s) => s,
            Err(e) => {
                why = Some(format!("{}: {e}", part.name));
                break;
            }
        };
        // The slicer parts this cache lists (its `<slicers><slicer r:id>` and its relationships).
        let mut slicer_els = 0usize;
        for target in slicer_part_rels(part) {
            let Some(sp) = parts_by_name(&slicers, &target) else {
                why = Some(format!("{}: {} is missing", part.name, target));
                break 'cache;
            };
            let Ok(sroot) = xml::parse(&sp.data) else {
                why = Some(format!("{}: not XML", sp.name));
                break 'cache;
            };
            if sroot.name != "slicers" {
                why = Some(format!("{}: not a slicers part", sp.name));
                break 'cache;
            }
            for el in sroot.kids("slicer") {
                slicer_els += 1;
                let Some(widget) = slicer_widget(el, name) else {
                    why = Some(format!("{}: a slicer has an attribute GridCraft does not model", sp.name));
                    break 'cache;
                };
                let Some(first) = pivot_tables.first() else { break };
                let Some(si) = pivot_sheet(wb, first) else { continue };
                out.push((si, widget, part.name.clone(), sp.name.clone()));
            }
        }
        if slicer_els == 0 {
            why = Some(format!("{}: no slicer part", part.name));
            break;
        }
        if out.len() > MAX_WIDGETS {
            why = Some("too many slicers".into());
            break;
        }
        parsed.push(CacheRead {
            part: part.name.clone(),
            name: name.to_string(),
            field: field.to_string(),
            pivot_cache_id,
            pivot_tables,
            values: values.clone(),
            selected,
        });
    }
    if let Some(why) = why {
        // All or nothing: one cache we cannot model leaves every slicer on the kept path.
        cx.warn(format!("slicers are kept as they came (they are not editable): {why}"));
        return None;
    }
    // Attach the cache's item list to each widget: a widget's items are the cache's values, in the
    // cache's order, with that cache's selection.
    for (_, widget, cache_part, _) in &mut out {
        let Some(cache) = parsed.iter().find(|c| same_part(&c.part, cache_part)) else { continue };
        widget.cache = cache.name.clone();
        widget.field = cache.field.clone();
        widget.pivot_cache_id = cache.pivot_cache_id;
        widget.pivot_tables = cache.pivot_tables.clone();
        widget.caption = if widget.caption.is_empty() { cache.field.clone() } else { widget.caption.clone() };
        widget.items = cache
            .values
            .iter()
            .enumerate()
            .map(|(i, v)| SlicerItem::new(v.clone(), cache.selected.as_ref().map(|s| s.get(i).copied().unwrap_or(true)).unwrap_or(true)))
            .collect();
        let Some(first) = cache.pivot_tables.first() else { continue };
        if let Some(si) = pivot_sheet(wb, first) {
            widget.anchor = anchor_beside(wb, si, first, widget.anchor.width, widget.anchor.height);
        }
    }
    Some(out)
}

fn same_part(a: &str, b: &str) -> bool {
    normalize(a) == normalize(b)
}

fn parts_by_name<'a>(parts: &[&'a gridcraft_model::PreservedPart], name: &str) -> Option<&'a gridcraft_model::PreservedPart> {
    let n = normalize(name);
    parts.iter().copied().find(|p| normalize(&p.name) == n)
}

/// The part names of the cache's slicer relationships.
fn slicer_part_rels(part: &gridcraft_model::PreservedPart) -> Vec<String> {
    part.rels
        .iter()
        .filter(|r| !r.external && r.rel_type.rsplit('/').next().is_some_and(|k| k.eq_ignore_ascii_case("slicer")))
        .map(|r| r.target.clone())
        .collect()
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

/// A slicer element, or `None` when it carries anything the model would lose.
fn slicer_widget(el: &El, cache: &str) -> Option<PivotSlicer> {
    let mut widget = PivotSlicer::default();
    // A file that names no style keeps none: the default look is the UI's choice, not a rewrite.
    widget.style.clear();
    let mut cache_seen = false;
    for (k, v) in &el.attrs {
        match k.as_str() {
            "name" => widget.name = v.clone(),
            "cache" => {
                cache_seen = true;
                if !v.eq_ignore_ascii_case(cache) {
                    return None;
                }
            }
            "caption" => widget.caption = v.clone(),
            "style" => widget.style = v.clone(),
            "columnCount" => widget.columns = v.trim().parse::<u32>().ok().filter(|c| (1..=64).contains(c))?,
            "sortOrder" => widget.sort = SlicerSort::parse(v)?,
            _ => return None,
        }
    }
    if !cache_seen || !el.children.is_empty() || widget.name.trim().is_empty() {
        return None;
    }
    Some(widget)
}

fn read_timelines(wb: &mut Workbook, cx: &mut Ctx<'_>) -> Option<Vec<(usize, PivotTimeline, String, String)>> {
    let caches = kept_parts(wb, CT_TIMELINE_CACHE);
    let parts = kept_parts(wb, CT_TIMELINE);
    if caches.is_empty() || parts.is_empty() {
        return Some(vec![]);
    }
    // A timeline part is related from the sheet it is drawn on (the sheet's `<extLst>` holds its
    // `r:id`), and names its cache. Cache parts name the PivotTables they filter.
    let mut out = Vec::new();
    let mut why: Option<String> = None;
    for si in 0..wb.sheets.len() {
        let Some(p) = wb.sheets.get(si).and_then(|s| s.preserved.as_deref()) else { continue };
        let targets: Vec<String> = p
            .rels
            .iter()
            .filter(|r| !r.external && r.rel_type.rsplit('/').next().is_some_and(|k| k.eq_ignore_ascii_case("timeline")))
            .map(|r| r.target.clone())
            .collect();
        for target in targets {
            let Some(part) = parts_by_name(&parts, &target) else {
                why = Some(format!("{target}: the timeline part is missing"));
                continue;
            };
            let Ok(root) = xml::parse(&part.data) else {
                why = Some(format!("{}: not XML", part.name));
                continue;
            };
            if root.name != "timelines" {
                why = Some(format!("{}: not a timelines part", part.name));
                continue;
            }
            for el in root.kids("timeline") {
                let Some(cache_name) = el.attr("cache") else {
                    why = Some(format!("{}: a timeline names no cache", part.name));
                    continue;
                };
                let cache = caches
                    .iter()
                    .find(|c| xml::parse(&c.data).ok().and_then(|x| x.attr("name").map(|n| n.eq_ignore_ascii_case(cache_name))).unwrap_or(false));
                let Some(cache) = cache else {
                    why = Some(format!("{cache_name}: the timeline cache is missing"));
                    continue;
                };
                let Ok(croot) = xml::parse(&cache.data) else {
                    why = Some(format!("{}: not XML", cache.name));
                    continue;
                };
                if let Some(e) = timeline_cache_problem(&croot) {
                    why = Some(format!("{}: {e}", cache.name));
                    continue;
                }
                let Some(mut widget) = timeline_widget(el, cache_name) else {
                    why = Some(format!("{}: a timeline has an attribute GridCraft does not model", part.name));
                    continue;
                };
                let Some(pivots) = pivot_table_names(&croot) else {
                    why = Some(format!("{}: its PivotTable list is empty or damaged", cache.name));
                    continue;
                };
                if pivots.iter().any(|n| pivot_sheet(wb, n).is_none()) {
                    why = Some(format!("{}: it names a PivotTable this workbook does not have", cache.name));
                    continue;
                }
                widget.pivot_tables = pivots.clone();
                widget.field = croot.attr("sourceName").unwrap_or("").to_string();
                if widget.caption.is_empty() {
                    widget.caption = widget.field.clone();
                }
                if let Some(first) = pivots.first() {
                    widget.anchor = anchor_beside(wb, si, first, widget.anchor.width, widget.anchor.height);
                }
                out.push((si, widget, cache.name.clone(), part.name.clone()));
                if out.len() > MAX_WIDGETS {
                    why = Some("too many timelines".into());
                    break;
                }
            }
        }
    }
    if let Some(why) = why {
        cx.warn(format!("timelines are kept as they came (they are not editable): {why}"));
        return None;
    }
    Some(out)
}

/// Why a timeline cache cannot be modelled, if it cannot: a selected date range, an extension, or
/// a child we do not know.
fn timeline_cache_problem(root: &El) -> Option<String> {
    if root.name != "timelineCacheDefinition" {
        return Some("not a timeline cache definition".into());
    }
    for c in &root.children {
        match c.name.as_str() {
            "pivotTables" => {}
            "state" => {
                if c.attr("selectionStartDate").is_some() || c.attr("selectionEndDate").is_some() {
                    return Some("its selected date range is not modelled".into());
                }
            }
            other => return Some(format!("<{other}> is not modelled")),
        }
    }
    if root.attr("name").is_none() || root.attr("sourceName").is_none() {
        return Some("it needs a name and a source field".into());
    }
    None
}

fn timeline_widget(el: &El, cache: &str) -> Option<PivotTimeline> {
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
            _ => return None,
        }
    }
    if !cache_seen || !el.children.is_empty() || widget.name.trim().is_empty() {
        return None;
    }
    Some(widget)
}

// ================================================================ writing

/// A slicer that survives to the file: its sheet and a copy of the model's widget.
struct OutSlicer {
    widget: PivotSlicer,
    /// Part name, e.g. `xl/slicers/slicer1.xml`.
    part: String,
}

/// A timeline that survives to the file.
struct OutTimeline {
    sheet: usize,
    widget: PivotTimeline,
    /// Part names, e.g. `xl/timelines/timeline1.xml` and `xl/timelineCaches/timelineCache1.xml`.
    part: String,
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

/// Writes the modelled slicers and timelines of a workbook.
///
/// Constructed before the sheet loop (it decides which widgets can be written and claims their part
/// names), asked for each sheet's `<extLst>` timeline entry ([`Self::sheet_ext`]), then finished
/// before the workbook XML is assembled ([`Self::finish`] writes the parts, the workbook
/// relationships and the workbook extension, and returns the extension XML). The PivotTable writer
/// takes the cache links afterwards ([`Self::links`]) to write its own relationship and extension.
#[derive(Default)]
pub struct SlicerWriter {
    slicers: Vec<OutSlicer>,
    timelines: Vec<OutTimeline>,
    caches: Vec<OutCache>,
    /// (PivotTable name, field) pairs whose pivot cache must itemize the field.
    pivot_fields: Vec<(String, String)>,
    links: Vec<CacheLink>,
    /// The workbook `<ext>` children written from the model.
    wb_ext: String,
}

impl SlicerWriter {
    /// Collects the widgets that can be written. A widget goes when its PivotTables are all gone or
    /// its PivotTable's source (or that source's field) can no longer be written: the kept path
    /// drops such a slicer too, and a cache naming a report the file lacks is a repair prompt.
    pub fn new(wb: &Workbook, out: &mut Out) -> SlicerWriter {
        let mut w = SlicerWriter::default();
        for (si, sheet) in wb.sheets.iter().enumerate() {
            for sl in &sheet.pivot_slicers {
                let Some(pivots) = live_pivots(wb, &sl.pivot_tables, Some(&sl.field)) else { continue };
                if w.slicers.len() >= MAX_WIDGETS {
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
                if !w.caches.iter().any(|c| c.name.eq_ignore_ascii_case(&sl.cache)) {
                    let n = w.caches.len() + 1;
                    let part = out.kept.claim(&format!("xl/slicerCaches/slicerCache{n}.xml"));
                    let items = if sl.items.is_empty() { Vec::new() } else { sl.items.clone() };
                    w.caches.push(OutCache { name: sl.cache.clone(), field: sl.field.clone(), pivots, items, part });
                }
                let n = w.slicers.len() + 1;
                let part = out.kept.claim(&format!("xl/slicers/slicer{n}.xml"));
                w.slicers.push(OutSlicer { widget, part });
            }
            for tl in &sheet.pivot_timelines {
                let Some(pivots) = live_pivots(wb, &tl.pivot_tables, None) else { continue };
                if w.timelines.len() >= MAX_WIDGETS {
                    break;
                }
                let n = w.timelines.len() + 1;
                let part = out.kept.claim(&format!("xl/timelines/timeline{n}.xml"));
                let cache_part = out.kept.claim(&format!("xl/timelineCaches/timelineCache{n}.xml"));
                let mut widget = tl.clone();
                widget.pivot_tables = pivots;
                w.timelines.push(OutTimeline { sheet: si, widget, part, cache_part });
            }
        }
        w
    }

    /// The fields a PivotTable's cache must itemize for the modelled slicers that filter it.
    pub fn fields_for_pivot(&self, pivot: &str) -> Vec<String> {
        self.pivot_fields.iter().filter(|(p, _)| p.eq_ignore_ascii_case(pivot)).map(|(_, f)| f.clone()).collect()
    }

    /// The sheet's timeline `<ext>` entry, adding a sheet relationship per timeline written on it.
    pub fn sheet_ext(&self, si: usize, rels: &mut Rels) -> String {
        let mine: Vec<&OutTimeline> = self.timelines.iter().filter(|t| t.sheet == si).collect();
        if mine.is_empty() {
            return String::new();
        }
        let mut x = format!("<ext uri=\"{EXT_TIMELINES}\" xmlns:x15=\"{NS_X15}\"><x15:timelines>");
        for t in mine {
            let rid = rels.add_uri(REL_TIMELINE, &from_part_dir(&t.part));
            let _ = write!(x, "<x15:timeline r:id=\"{rid}\"/>");
        }
        x.push_str("</x15:timelines></ext>");
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
        // Slicer caches: a widget whose PivotTable lost its cache is not written.
        for ci in 0..self.caches.len() {
            let Some((name, field, pivots_named, items, part)) =
                self.caches.get(ci).map(|c| (c.name.clone(), c.field.clone(), c.pivots.clone(), c.items.clone(), c.part.clone()))
            else {
                continue;
            };
            let Some(cache_id) = pivots_named.iter().find_map(|p| pivots.cache_id(wb, p)) else { continue };
            let Some(values) = pivots.field_items(cache_id, &field) else { continue };
            let widgets: Vec<&OutSlicer> = self.slicers.iter().filter(|s| s.widget.cache.eq_ignore_ascii_case(&name)).collect();
            if widgets.is_empty() {
                continue;
            }
            let selected: HashMap<String, bool> = items.iter().map(|i| (i.value.key(), i.selected)).collect();
            let mut cache_rels = Rels::default();
            let mut refs = String::new();
            let mut part_xml = format!(
                "{XML_DECL}<slicerCacheDefinition xmlns=\"{NS_MAIN}\" xmlns:r=\"{NS_REL}\" name=\"{}\" sourceName=\"{}\"><pivotTables>",
                esc_attr(&name),
                esc_attr(&field)
            );
            for p in &pivots_named {
                let tab = table_sheet(wb, p);
                let _ = write!(part_xml, "<pivotTable tabId=\"{tab}\" name=\"{}\"/>", esc_attr(p));
            }
            part_xml.push_str("</pivotTables><data>");
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
            part_xml.push_str("</data><slicers>");
            for slicer in &widgets {
                let rid = cache_rels.add_uri(REL_SLICER, &from_part_dir(&slicer.part));
                let _ = write!(refs, "<slicer r:id=\"{rid}\"/>");
                out.part(&slicer.part, Some(CT_SLICER), slicer_xml(&slicer.widget).into_bytes());
            }
            part_xml.push_str(&refs);
            part_xml.push_str("</slicers></slicerCacheDefinition>");
            out.part(&part, Some(CT_SLICER_CACHE), part_xml.into_bytes());
            if !cache_rels.is_empty() {
                let rels_part = crate::passthrough::rels_part_name(&part);
                out.part(&rels_part, None, cache_rels.xml().into_bytes());
            }
            let rid = wb_rels.add_uri(REL_SLICER_CACHE, &from_workbook(&part));
            let _ = write!(slicer_ext, "<x14:slicerCache r:id=\"{rid}\"/>");
            for p in &pivots_named {
                links.push(CacheLink { pivot: p.clone(), cache_part: part.clone() });
            }
        }
        for tl in &self.timelines {
            let pivots = tl.widget.pivot_tables.clone();
            if pivots.is_empty() {
                continue;
            }
            let mut cache_xml = format!(
                "{XML_DECL}<timelineCacheDefinition xmlns=\"{NS_X15}\" name=\"{}\" sourceName=\"{}\"><pivotTables>",
                esc_attr(&tl.widget.cache),
                esc_attr(&tl.widget.field)
            );
            for p in &pivots {
                let tab = table_sheet(wb, p);
                let _ = write!(cache_xml, "<pivotTable tabId=\"{tab}\" name=\"{}\"/>", esc_attr(p));
            }
            cache_xml.push_str("</pivotTables><state scrollPosition=\"0\"/></timelineCacheDefinition>");
            out.part(&tl.cache_part, Some(CT_TIMELINE_CACHE), cache_xml.into_bytes());
            out.part(&tl.part, Some(CT_TIMELINE), timeline_xml(&tl.widget).into_bytes());
            let rid = wb_rels.add_uri(REL_TIMELINE_CACHE, &from_workbook(&tl.cache_part));
            let _ = write!(timeline_ext, "<x15:timelineCacheRef r:id=\"{rid}\"/>");
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
                "<ext uri=\"{EXT_TIMELINES}\" xmlns:x15=\"{NS_X15}\"><x15:timelineCacheRefs>{timeline_ext}</x15:timelineCacheRefs></ext>"
            );
        }
        self.wb_ext.clone()
    }
}

/// The 0-based sheet index of a PivotTable by name (a file's `tabId`).
fn table_sheet(wb: &Workbook, pivot: &str) -> usize {
    wb.sheets.iter().position(|s| s.pivots.iter().any(|pt| pt.name.eq_ignore_ascii_case(pivot))).unwrap_or(0)
}

fn slicer_xml(sl: &PivotSlicer) -> String {
    let mut s = format!(
        "{XML_DECL}<slicers xmlns=\"{NS_X14}\" xmlns:r=\"{NS_REL}\"><slicer name=\"{}\" cache=\"{}\"",
        esc_attr(&sl.name),
        esc_attr(&sl.cache)
    );
    if !sl.caption.is_empty() {
        let _ = write!(s, " caption=\"{}\"", esc_attr(&sl.caption));
    }
    if !sl.style.is_empty() {
        let _ = write!(s, " style=\"{}\"", esc_attr(&sl.style));
    }
    let _ = write!(s, " columnCount=\"{}\"/></slicers>", sl.columns.clamp(1, 64));
    s
}

fn timeline_xml(tl: &PivotTimeline) -> String {
    let mut s = format!("{XML_DECL}<timelines xmlns=\"{NS_X15}\"><timeline name=\"{}\" cache=\"{}\"", esc_attr(&tl.name), esc_attr(&tl.cache));
    if !tl.caption.is_empty() {
        let _ = write!(s, " caption=\"{}\"", esc_attr(&tl.caption));
    }
    let _ = write!(s, " level=\"{}\"", tl.unit.level());
    if !tl.style.is_empty() {
        let _ = write!(s, " style=\"{}\"", esc_attr(&tl.style));
    }
    s.push_str("/></timelines>");
    s
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_links_are_relativised_from_the_pivot_table_part() {
        assert_eq!(from_part_dir("xl/slicerCaches/slicerCache1.xml"), "../slicerCaches/slicerCache1.xml");
        assert_eq!(from_part_dir("other/x.xml"), "../other/x.xml");
    }
}
