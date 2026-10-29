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

/// A table, a table slicer on its `Region` column, and the sheet filter the slicer drives (the
/// engine writes the selection into the AutoFilter; the table slicer model is the widget).
fn table_workbook() -> Workbook {
    let mut wb = Workbook::new();
    {
        let sh = wb.sheet_mut(0).unwrap();
        sh.name = "Data".into();
        for (c, h) in ["Region", "Amount"].iter().enumerate() {
            sh.cells.set(CellRef::new(0, c as u32), Cell::value(Value::text(*h)));
        }
        for (r, (region, amount)) in [("East", 10.0), ("West", 20.0), ("East", 30.0)].iter().enumerate() {
            sh.cells.set(CellRef::new(r as u32 + 1, 0), Cell::value(Value::text(*region)));
            sh.cells.set(CellRef::new(r as u32 + 1, 1), Cell::value(Value::number(*amount)));
        }
        let column =
            |name: &str| TableColumn { name: name.into(), totals: TotalsFn::None, totals_label: None, formula: None, unique_name: None, xml: None };
        sh.tables.push(Table {
            id: 1,
            name: "Sales".into(),
            range: rr("A1:B4"),
            header_row: true,
            totals_row: false,
            columns: vec![column("Region"), column("Amount")],
            style: "TableStyleMedium2".into(),
            banded_rows: true,
            banded_cols: false,
            first_col: false,
            last_col: false,
            filter_button: true,
            filter: vec![],
            table_type: None,
            connection_id: None,
        });
        // The selection a table slicer keeps in the file: its column's value filter.
        sh.autofilter =
            Some(AutoFilter { range: rr("A1:B4"), criteria: vec![(0, FilterCriterion::Values { values: vec!["East".into()], blanks: false })] });
        sh.slicers.push(Slicer {
            id: 1,
            table: "Sales".into(),
            column: "Region".into(),
            caption: "Regions".into(),
            anchor: Anchor { cell: at("D2"), dx: 4.0, dy: 4.0, width: 168.0, height: 160.0, mode: AnchorMode::MoveOnly },
            selected: Some(vec!["East".into()]),
            columns: 1,
        });
    }
    wb
}

#[test]
fn a_slicer_and_a_timeline_are_placed_where_the_model_says() {
    let wb = workbook();
    let out = write_xlsx(&wb).unwrap();
    check_package(&out).unwrap();
    let drawing = text(&unpack(&out), "xl/drawings/");
    // Each view is an anchored graphic frame naming its drawing namespace and view.
    assert!(drawing.contains("<a:graphicData uri=\"http://schemas.microsoft.com/office/drawing/2010/slicer\">"), "{drawing}");
    assert!(drawing.contains("<sle:slicer ") && drawing.contains("name=\"Region\""), "{drawing}");
    assert!(drawing.contains("<a:graphicData uri=\"http://schemas.microsoft.com/office/drawing/2012/timeslicer\">"), "{drawing}");
    assert!(drawing.contains("<sle:timeslicer ") && drawing.contains("name=\"Date\""), "{drawing}");
    // The first frame is anchored at D3 (0-based col 3, row 2) + 4 px, 168 x 200 px.
    assert!(drawing.contains("<xdr:twoCellAnchor editAs=\"oneCell\"><xdr:from><xdr:col>3</xdr:col><xdr:colOff>38100</xdr:colOff><xdr:row>2</xdr:row><xdr:rowOff>38100</xdr:rowOff>"), "{drawing}");

    // The model reads the anchors back from the frames: the parts carry no position.
    let (back, rep) = read_xlsx(&out).unwrap();
    assert!(!rep.warnings.iter().any(|w| w.contains("kept as they came") || w.contains("not placed")), "{:?}", rep.warnings);
    let report = back.sheet_index("Report").and_then(|i| back.sheet(i)).unwrap();
    let sl = &report.pivot_slicers[0];
    assert_eq!((sl.anchor.cell, sl.anchor.dx, sl.anchor.dy, sl.anchor.width, sl.anchor.height), (at("D3"), 4.0, 4.0, 168.0, 200.0));
    assert_eq!(sl.anchor.mode, AnchorMode::MoveOnly);
    let tl = &report.pivot_timelines[0];
    assert_eq!((tl.anchor.cell, tl.anchor.width, tl.anchor.height), (at("H3"), 240.0, 96.0));

    // A second save draws the same frames: the anchor is the model's, not a fresh guess.
    let out2 = write_xlsx(&back).unwrap();
    check_package(&out2).unwrap();
    assert_eq!(text(&unpack(&out2), "xl/drawings/"), drawing);
}

#[test]
fn a_table_slicer_round_trips_through_its_table_slicer_cache() {
    let wb = table_workbook();
    let out = write_xlsx(&wb).unwrap();
    check_package(&out).unwrap();
    let parts = unpack(&out);
    let cache = text(&parts, "xl/slicerCaches/");
    // The cache is the table's, named by the table and column ids the file writes.
    assert!(cache.contains("<ext uri=\"{2F2917AC-EB37-4324-AD4E-5DD8C200BD13}\""), "{cache}");
    assert!(cache.contains("<x15:tableSlicerCache tableId=\"1\" column=\"1\"/>"), "{cache}");
    assert!(cache.contains("name=\"Slicer_Sales_Region\" sourceName=\"Region\""), "{cache}");
    assert!(!cache.contains("<items"), "a table slicer keeps its selection in the table filter: {cache}");
    let slicer = text(&parts, "xl/slicers/");
    assert!(slicer.contains("name=\"Sales_Region\" cache=\"Slicer_Sales_Region\" caption=\"Regions\""), "{slicer}");
    // It is placed, and the sheet lists the part it lives in.
    let drawing = text(&parts, "xl/drawings/");
    assert!(
        drawing.contains("uri=\"http://schemas.microsoft.com/office/drawing/2010/slicer\"") && drawing.contains("name=\"Sales_Region\""),
        "{drawing}"
    );
    let sheet = String::from_utf8_lossy(&parts["xl/worksheets/sheet1.xml"]).into_owned();
    assert!(sheet.contains("slicerList"), "{sheet}");

    // Reading models the table slicer again, with the anchor and the table filter's selection.
    let (back, rep) = read_xlsx(&out).unwrap();
    assert!(!rep.warnings.iter().any(|w| w.contains("kept as they came") || w.contains("not placed")), "{:?}", rep.warnings);
    let sh = back.sheet(0).unwrap();
    assert_eq!(sh.slicers.len(), 1, "{:?}", rep.warnings);
    let sl = &sh.slicers[0];
    assert_eq!((sl.table.as_str(), sl.column.as_str(), sl.caption.as_str(), sl.columns), ("Sales", "Region", "Regions", 1));
    assert_eq!(sl.selected, Some(vec!["East".to_string()]));
    assert_eq!((sl.anchor.cell, sl.anchor.width, sl.anchor.height), (at("D2"), 168.0, 160.0));

    let out2 = write_xlsx(&back).unwrap();
    check_package(&out2).unwrap();
    let parts2 = unpack(&out2);
    assert_eq!(text(&parts2, "xl/slicerCaches/"), cache);
    assert_eq!(text(&parts2, "xl/drawings/"), drawing);
}

#[test]
fn a_slicer_cache_the_model_does_not_understand_keeps_its_bytes() {
    // A cache with an attribute the model does not carry must stay exactly as it came: the whole
    // slicer set is kept, not half-rewritten.
    let out = write_xlsx(&workbook()).unwrap();
    let mut parts = unpack(&out);
    let patched = text(&parts, "xl/slicerCaches/").replace("name=\"Slicer_Region\"", "name=\"Slicer_Region\" sortOrder=\"descending\"");
    parts.insert("xl/slicerCaches/slicerCache1.xml".into(), patched.clone().into_bytes());
    let slicer_before = text(&parts, "xl/slicers/").into_bytes();
    let repacked = super::make_zip(&parts.iter().map(|(k, v)| (k.as_str(), v.as_slice())).collect::<Vec<_>>());

    let (back, rep) = read_xlsx(&repacked).unwrap();
    assert!(rep.warnings.iter().any(|w| w.contains("kept as they came")), "{:?}", rep.warnings);
    assert!(back.sheet(1).unwrap().pivot_slicers.is_empty());
    let out2 = write_xlsx(&back).unwrap();
    check_package(&out2).unwrap();
    let parts2 = unpack(&out2);
    let kept = parts2.keys().filter(|n| n.starts_with("xl/slicerCaches/")).collect::<Vec<_>>();
    assert_eq!(kept.len(), 1, "{:?}", parts2.keys());
    assert_eq!(parts2[kept[0]], patched.as_bytes(), "the kept cache is written byte for byte");
    assert_eq!(text(&parts2, "xl/slicers/").as_bytes(), slicer_before.as_slice(), "the kept slicer part is written byte for byte");
    // The timeline of the same file is independent of the slicers and stays modelled.
    assert_eq!(parts2.keys().filter(|n| n.starts_with("xl/timelines/")).count(), 1);
}

#[test]
fn a_timeline_on_a_field_without_dates_is_not_written() {
    // A timeline needs a date range to show; a text field has none, so nothing names a timeline the
    // file could not draw (and no drawing frame is left behind for it).
    let mut wb = workbook();
    wb.sheet_mut(1).unwrap().pivot_timelines[0].field = "Region".into();
    let parts = unpack(&write_xlsx(&wb).unwrap());
    assert!(parts.keys().all(|n| !n.starts_with("xl/timelines/") && !n.starts_with("xl/timelineCaches/")), "{:?}", parts.keys());
    assert!(!text(&parts, "xl/drawings/").contains("timeslicer"));
    assert!(text(&parts, "xl/slicers/").contains("name=\"Region\""), "the slicer is unaffected");
}

#[test]
fn a_hostile_anchor_is_clamped_not_trusted() {
    let mut wb = workbook();
    wb.sheet_mut(1).unwrap().pivot_slicers[0].anchor =
        Anchor { cell: CellRef::new(u32::MAX, u32::MAX), dx: f32::NAN, dy: -12.0, width: f32::INFINITY, height: 1e30, mode: AnchorMode::MoveOnly };
    let out = write_xlsx(&wb).unwrap();
    check_package(&out).unwrap();
    let drawing = text(&unpack(&out), "xl/drawings/");
    assert!(!drawing.contains("NaN") && !drawing.contains("inf"), "{drawing}");
    // The start cell is the sheet's last cell and the offsets are finite: a number a reader can use.
    assert!(
        drawing.contains(&format!(
            "<xdr:from><xdr:col>{}</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{}</xdr:row><xdr:rowOff>0</xdr:rowOff>",
            gridcraft_core::MAX_COLS - 1,
            gridcraft_core::MAX_ROWS - 1
        )),
        "{drawing}"
    );
    let (back, _) = read_xlsx(&out).unwrap();
    let a = back.sheet(1).unwrap().pivot_slicers[0].anchor;
    assert!(a.dx.is_finite() && a.dy.is_finite() && a.width.is_finite() && a.height.is_finite());
    assert!(a.dx >= 0.0 && a.dy >= 0.0 && a.width >= 0.0 && a.height >= 0.0);
}
