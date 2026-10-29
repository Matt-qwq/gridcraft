//! Slicers and timelines: the model ↔ package round trip, the drop-with-owner rule, and the
//! boundary between modelled and kept parts. Workbooks are built in code.

use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

use gridcraft_core::{CellRef, RangeRef, Value};
use gridcraft_model::*;

use crate::read_xlsx;
use crate::testing::check_package;
use crate::write_xlsx;

fn at(a: &str) -> CellRef {
    CellRef::parse(a).unwrap()
}

fn rr(a: &str) -> RangeRef {
    RangeRef::parse(a).unwrap()
}

/// Source data on "Data" (A1:C5) and a PivotTable on "Report", with a slicer on Region and a
/// timeline on Date — the shape an Insert ▸ Slicer / Insert ▸ Timeline produces.
fn workbook() -> Workbook {
    let mut wb = Workbook::new();
    {
        let data = wb.sheet_mut(0).unwrap();
        data.name = "Data".into();
        let headers = ["Region", "Amount", "Date"];
        for (c, h) in headers.iter().enumerate() {
            data.cells.set(CellRef::new(0, c as u32), Cell::value(Value::text(*h)));
        }
        let rows = [("East", 10.0, 45_000.0), ("West", 20.0, 45_031.0), ("East", 30.0, 45_059.0), ("North", 40.0, 45_090.0)];
        for (r, (region, amount, date)) in rows.iter().enumerate() {
            let r = r as u32 + 1;
            data.cells.set(CellRef::new(r, 0), Cell::value(Value::text(*region)));
            data.cells.set(CellRef::new(r, 1), Cell::value(Value::number(*amount)));
            data.cells.set(CellRef::new(r, 2), Cell::value(Value::number(*date)));
        }
    }
    let mut report = Sheet::new("Report");
    report.pivots.push(PivotTable {
        id: 1,
        name: "Report Pivot".into(),
        source: "Data!$A$1:$C$5".into(),
        anchor: at("A3"),
        rows: vec![PivotField { source_col: "Region".into(), ..PivotField::default() }],
        values: vec![PivotValue { source_col: "Amount".into(), func: PivotFunc::Sum, name: "Sum of Amount".into(), ..PivotValue::default() }],
        last_range: Some(rr("A2:B7")),
        ..PivotTable::default()
    });
    report.pivot_slicers.push(PivotSlicer {
        id: 1,
        name: "Region".into(),
        cache: "Slicer_Region".into(),
        caption: "Regions".into(),
        field: "Region".into(),
        pivot_tables: vec!["Report Pivot".into()],
        items: vec![
            SlicerItem::new(SlicerValue::Text("East".into()), true),
            SlicerItem::new(SlicerValue::Text("West".into()), false),
            SlicerItem::new(SlicerValue::Text("North".into()), true),
        ],
        style: "SlicerStyleLight2".into(),
        columns: 2,
        anchor: Anchor { cell: at("D3"), dx: 4.0, dy: 4.0, width: 168.0, height: 200.0, mode: AnchorMode::MoveOnly },
        ..PivotSlicer::default()
    });
    report.pivot_timelines.push(PivotTimeline {
        id: 2,
        name: "Date".into(),
        cache: "Timeline_Date".into(),
        caption: "By quarter".into(),
        field: "Date".into(),
        pivot_tables: vec!["Report Pivot".into()],
        style: String::new(),
        unit: TimeUnit::Quarters,
        anchor: Anchor { cell: at("H3"), dx: 4.0, dy: 4.0, width: 240.0, height: 96.0, mode: AnchorMode::MoveOnly },
        ..PivotTimeline::default()
    });
    wb.sheets.push(Arc::new(report));
    wb
}

fn unpack(bytes: &[u8]) -> HashMap<String, Vec<u8>> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut parts = HashMap::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let name = f.name().to_string();
        let mut data = vec![];
        f.read_to_end(&mut data).unwrap();
        parts.insert(name, data);
    }
    parts
}

fn text(parts: &HashMap<String, Vec<u8>>, prefix: &str) -> String {
    let hits: Vec<&String> = parts.keys().filter(|n| n.starts_with(prefix) && n.ends_with(".xml")).collect();
    assert_eq!(hits.len(), 1, "expected one {prefix}*.xml part, found {hits:?}");
    String::from_utf8_lossy(&parts[hits[0]]).into_owned()
}

#[test]
fn a_pivottable_slicer_and_timeline_round_trip_through_the_model() {
    let wb = workbook();
    let out = write_xlsx(&wb).unwrap();
    check_package(&out).unwrap();
    let parts = unpack(&out);
    // One part of each kind, and the field the slicer shows is itemized in the pivot cache.
    let cache = text(&parts, "xl/slicerCaches/");
    assert!(cache.contains("name=\"Slicer_Region\"") && cache.contains("sourceName=\"Region\""), "{cache}");
    assert!(cache.contains("<items count=\"3\"><i x=\"0\" s=\"1\"/><i x=\"1\"/><i x=\"2\" s=\"1\"/></items>"), "{cache}");
    assert!(text(&parts, "xl/slicers/").contains("columnCount=\"2\""), "{}", text(&parts, "xl/slicers/"));
    assert!(text(&parts, "xl/timelines/").contains("level=\"1\""), "{}", text(&parts, "xl/timelines/"));
    text(&parts, "xl/timelineCaches/");
    let pivot_cache = text(&parts, "xl/pivotCache/pivotCacheDefinition");
    assert!(pivot_cache.contains("<cacheField name=\"Region\""), "{pivot_cache}");
    assert!(pivot_cache.contains("<s v=\"East\"/><s v=\"West\"/><s v=\"North\"/>"), "the slicer's field is itemized in cache order: {pivot_cache}");

    let (back, rep) = read_xlsx(&out).unwrap();
    assert!(!rep.warnings.iter().any(|w| w.contains("dropped") || w.contains("kept as they came")), "{:?}", rep.warnings);
    let sh = back.sheet_index("Report").and_then(|i| back.sheet(i)).unwrap();
    assert_eq!(sh.pivot_slicers.len(), 1);
    let sl = &sh.pivot_slicers[0];
    assert_eq!(sl.name, "Region");
    assert_eq!(sl.cache, "Slicer_Region");
    assert_eq!(sl.caption, "Regions");
    assert_eq!(sl.field, "Region");
    assert_eq!(sl.style, "SlicerStyleLight2");
    assert_eq!(sl.columns, 2);
    assert_eq!(sl.pivot_cache_id, 1);
    // Item order and selection come back: West is the only tile switched off.
    let labels: Vec<String> = sl.items.iter().map(|i| i.label()).collect();
    assert_eq!(labels, ["East", "West", "North"]);
    let on: Vec<bool> = sl.items.iter().map(|i| i.selected).collect();
    assert_eq!(on, [true, false, true]);
    assert_eq!(sl.selected_labels(), Some(vec!["East".into(), "North".into()]));
    assert_eq!(sh.pivot_timelines.len(), 1);
    let tl = &sh.pivot_timelines[0];
    assert_eq!((tl.name.as_str(), tl.caption.as_str(), tl.field.as_str()), ("Date", "By quarter", "Date"));
    assert_eq!(tl.unit, TimeUnit::Quarters);
    assert_eq!(tl.style, "", "a timeline written without a style comes back without one");

    // A second save writes the same model, so the parts are identical and no part is duplicated.
    let out2 = write_xlsx(&back).unwrap();
    check_package(&out2).unwrap();
    let parts2 = unpack(&out2);
    assert_eq!(text(&parts2, "xl/slicerCaches/"), cache);
    assert_eq!(
        text(&parts2, "xl/slicers/"),
        text(&unpack(&write_xlsx(&wb).unwrap()), "xl/slicers/"),
        "the slicer part is written from the model both times"
    );
}

#[test]
fn deleting_the_pivottable_or_its_sheet_takes_the_modelled_slicer_and_caches() {
    // The PivotTable goes: nothing may name a report the file no longer has, and no part may be
    // left orphaned (the strict package validator runs on every save).
    let mut wb = workbook();
    wb.sheet_mut(1).unwrap().pivots.clear();
    let parts = unpack(&write_xlsx(&wb).unwrap());
    for prefix in ["xl/slicers/", "xl/slicerCaches/", "xl/timelines/", "xl/timelineCaches/"] {
        assert!(parts.keys().all(|n| !n.starts_with(prefix)), "{prefix} survived its PivotTable: {:?}", parts.keys());
    }
    let workbook_xml = String::from_utf8_lossy(&parts["xl/workbook.xml"]).into_owned();
    assert!(!workbook_xml.contains("slicerCaches") && !workbook_xml.contains("timelineCacheRefs"), "{workbook_xml}");

    // The sheet goes.
    let mut wb = workbook();
    wb.sheets.remove(1);
    let parts = unpack(&write_xlsx(&wb).unwrap());
    for prefix in ["xl/slicers/", "xl/slicerCaches/", "xl/timelines/", "xl/timelineCaches/"] {
        assert!(parts.keys().all(|n| !n.starts_with(prefix)), "{prefix} survived its sheet: {:?}", parts.keys());
    }
}

#[test]
fn a_slicer_on_a_field_the_source_lost_is_not_written() {
    // A tile list that no longer names a source field cannot be bound to a cache item; the widget
    // stays in the model (a rename back brings it back) but nothing dangling reaches the file.
    let mut wb = workbook();
    wb.sheet_mut(1).unwrap().pivot_slicers[0].field = "Nope".into();
    let parts = unpack(&write_xlsx(&wb).unwrap());
    assert!(parts.keys().all(|n| !n.starts_with("xl/slicers/") && !n.starts_with("xl/slicerCaches/")), "{:?}", parts.keys());
}

#[test]
fn a_widget_with_no_tiles_gets_the_fields_values() {
    // A brand-new slicer before any tile is known has no item list; the cache still needs one per
    // shared item, and every value is selected (Excel selects a new tile).
    let mut wb = workbook();
    {
        let sl = &mut wb.sheet_mut(1).unwrap().pivot_slicers[0];
        sl.items.clear();
    }
    let out = write_xlsx(&wb).unwrap();
    let parts = unpack(&out);
    let cache = text(&parts, "xl/slicerCaches/");
    assert!(cache.contains("<items count=\"3\"><i x=\"0\" s=\"1\"/><i x=\"1\" s=\"1\"/><i x=\"2\" s=\"1\"/></items>"), "{cache}");
    let (back, rep) = read_xlsx(&out).unwrap();
    assert!(!rep.warnings.iter().any(|w| w.contains("kept as they came")), "{:?}", rep.warnings);
    let sl = &back.sheet(1).unwrap().pivot_slicers[0];
    // The cache's item list is the field's values, all selected (an empty list in the file means
    // "every value"), which is what a fresh slicer shows.
    assert_eq!(sl.items.iter().map(|i| i.label()).collect::<Vec<_>>(), ["East", "West", "North"]);
    assert!(sl.all_selected());
}
