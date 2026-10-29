//! Content owned by parts GridCraft rewrites: slicers and their caches, timelines and their
//! caches, query tables attached to a table, and the `extLst` children we don't model on the
//! workbook, a worksheet, a table, a PivotTable and a pivot cache definition.

use super::*;

const NS_X14: &str = "http://schemas.microsoft.com/office/spreadsheetml/2009/9/main";
const NS_X15: &str = "http://schemas.microsoft.com/office/spreadsheetml/2010/11/main";
const REL_SLICER: &str = "http://schemas.microsoft.com/office/2007/relationships/slicer";
const REL_SLICER_CACHE: &str = "http://schemas.microsoft.com/office/2007/relationships/slicerCache";
const REL_TIMELINE: &str = "http://schemas.microsoft.com/office/2011/relationships/timeline";
const REL_TIMELINE_CACHE: &str = "http://schemas.microsoft.com/office/2011/relationships/timelineCache";
const EXT_SLICER_CACHES: &str = "{BBE1A952-6BD2-4e30-A5A6-4D6A0A0D6F2B}";
const EXT_TIMELINES: &str = "{7E03D99C-DC04-49d9-9315-930204A7B6E9}";
const EXT_PIVOT_SLICER_CACHE: &str = "{A8765BA9-456A-4dab-B4F3-ACF838C121DE}";
const EXT_PIVOT_MODELLED: &str = "{962EF5D1-5CA2-4c93-8EF4-DBF5C05439D2}";

const SLICER_CACHE: &str = r#"<?xml version="1.0"?><slicerCacheDefinition xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" name="Slicer_Region" sourceName="Region"><pivotTables><pivotTable tabId="0" name="PivotTable1"/></pivotTables><data><tabular pivotCacheId="1"/></data><slicers><slicer r:id="rId1"/></slicers></slicerCacheDefinition>"#;
const SLICER: &str = r#"<?xml version="1.0"?><slicers xmlns="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><slicer name="Region" cache="Slicer_Region" caption="Region"/></slicers>"#;
const TIMELINE: &str = r#"<?xml version="1.0"?><timelines xmlns="http://schemas.microsoft.com/office/spreadsheetml/2010/11/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><timeline name="Date" cache="Timeline_Date" caption="Date"/></timelines>"#;
const TIMELINE_CACHE: &str = r#"<?xml version="1.0"?><timelineCacheDefinition xmlns="http://schemas.microsoft.com/office/spreadsheetml/2010/11/main" name="Timeline_Date" sourceName="Date"><pivotTables><pivotTable tabId="0" name="PivotTable1"/></pivotTables><state scrollPosition="0"/></timelineCacheDefinition>"#;
const QUERY_TABLE: &str = r#"<?xml version="1.0"?><queryTable xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" name="SalesQuery" connectionId="1" autoFormatId="16" applyNumberFormats="0" applyBorderFormats="0" applyFontFormats="0" applyPatternFormats="0" applyAlignmentFormats="0" applyWidthHeightFormats="0"/>"#;
const PIVOT_CACHE: &str = r#"<?xml version="1.0"?><pivotCacheDefinition xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" saveData="0" refreshOnLoad="1" createdVersion="6" refreshedVersion="6" minRefreshableVersion="3"><cacheSource type="worksheet"><worksheetSource ref="A1:B5" sheet="Data"/></cacheSource><cacheFields count="2"><cacheField name="Region" numFmtId="0"><sharedItems containsSemiMixedTypes="0" containsString="1"><s v="North"/><s v="South"/></sharedItems></cacheField><cacheField name="Amount" numFmtId="0"><sharedItems containsSemiMixedTypes="0" containsString="0"/></cacheField></cacheFields><extLst><ext uri="{33333333-4444-5555-6666-777777777777}"><x99:cacheThing xmlns:x99="urn:example:cache">keep the cache extension</x99:cacheThing></ext></extLst></pivotCacheDefinition>"#;
const PIVOT_TABLE: &str = r#"<?xml version="1.0"?><pivotTableDefinition xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" name="PivotTable1" cacheId="1" dataCaption="Values" updatedVersion="6" minRefreshableVersion="3"><location ref="E3:F6" firstHeaderRow="1" firstDataRow="1" firstDataCol="1"/><pivotFields count="2"><pivotField axis="axisRow" showAll="0"/><pivotField dataField="1" showAll="0"/></pivotFields><rowFields count="1"><field x="0"/></rowFields><rowItems count="1"><i><x/></i></rowItems><colItems count="1"><i/></colItems><dataFields count="1"><dataField name="Sum of Amount" fld="1" baseField="0" baseItem="0"/></dataFields><extLst><ext uri="{A8765BA9-456A-4dab-B4F3-ACF838C121DE}" xmlns:x14="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main"><x14:slicerCacheData><x14:slicerCache r:id="rId2"/></x14:slicerCacheData></ext><ext uri="{962EF5D1-5CA2-4c93-8EF4-DBF5C05439D2}" xmlns:x14="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main"><x14:pivotTableDefinition hideValuesRow="1"/></ext><ext uri="{22222222-3333-4444-5555-666666666666}"><x99:pivotThing xmlns:x99="urn:example:pivot">keep the pivot extension</x99:pivotThing></ext></extLst></pivotTableDefinition>"#;
const TABLE_WITH_EXT: &str = r#"<?xml version="1.0"?><table xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" id="1" name="Sales" displayName="Sales" ref="A1:B5" totalsRowShown="0"><autoFilter ref="A1:B5"/><tableColumns count="2"><tableColumn id="1" name="Region"/><tableColumn id="2" name="Amount"/></tableColumns><tableStyleInfo name="TableStyleMedium2" showFirstColumn="0" showLastColumn="0" showRowStripes="1" showColumnStripes="0"/><extLst><ext uri="{7A5B2C4D-1111-2222-3333-444455556666}"><x99:tableThing xmlns:x99="urn:example:table">keep the table extension</x99:tableThing></ext></extLst></table>"#;

/// A package with content GridCraft does not model: a slicer and its cache (behind the
/// PivotTable's `extLst` and the workbook's), a timeline with its cache, a table with a query
/// table, and unknown `extLst` children on the workbook, the worksheet, the chart, the table and
/// the pivot cache definition.
fn extension_package() -> Vec<u8> {
    let workbook = format!(
        r#"<?xml version="1.0"?><workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}" xmlns:x14="{NS_X14}" xmlns:x15="{NS_X15}"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets><pivotCaches><pivotCache cacheId="1" r:id="rId2"/></pivotCaches><extLst><ext uri="{EXT_SLICER_CACHES}" xmlns:x14="{NS_X14}"><x14:slicerCaches><x14:slicerCache r:id="rId3"/></x14:slicerCaches></ext><ext uri="{EXT_TIMELINES}" xmlns:x15="{NS_X15}"><x15:timelineCacheRefs><x15:timelineCacheRef r:id="rId4"/></x15:timelineCacheRefs></ext><ext uri="{{11111111-2222-3333-4444-555555555555}}"><x99:workbookThing xmlns:x99="urn:example:workbook">keep the workbook extension</x99:workbookThing></ext></extLst></workbook>"#
    );
    let sheet = format!(
        r#"<?xml version="1.0"?><worksheet xmlns="{NS_MAIN}" xmlns:r="{NS_R}" xmlns:x14="{NS_X14}" xmlns:x15="{NS_X15}"><dimension ref="A1:B5"/><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Region</t></is></c><c r="B1" t="inlineStr"><is><t>Amount</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>North</t></is></c><c r="B2"><v>10</v></c></row><row r="3"><c r="A3" t="inlineStr"><is><t>South</t></is></c><c r="B3"><v>20</v></c></row><row r="4"><c r="A4" t="inlineStr"><is><t>North</t></is></c><c r="B4"><v>30</v></c></row><row r="5"><c r="A5" t="inlineStr"><is><t>South</t></is></c><c r="B5"><v>40</v></c></row></sheetData><drawing r:id="rId6"/><tableParts count="1"><tablePart r:id="rId1"/></tableParts><extLst><ext uri="{EXT_TIMELINES}" xmlns:x15="{NS_X15}"><x15:timelines><x15:timeline r:id="rId5"/></x15:timelines></ext><ext uri="{{AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE}}"><x99:sheetThing xmlns:x99="urn:example:sheet">keep the sheet extension</x99:sheetThing></ext></extLst></worksheet>"#
    );
    let drawing = r#"<?xml version="1.0"?><xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing"><xdr:twoCellAnchor><xdr:from><xdr:col>3</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>8</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>10</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><xdr:graphicFrame><xdr:nvGraphicFramePr><xdr:cNvPr id="2" name="Chart 1"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm/><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" r:id="rId1"/></a:graphicData></a:graphic></xdr:graphicFrame><xdr:clientData/></xdr:twoCellAnchor></xdr:wsDr>"#;
    let chart = format!(
        r#"<?xml version="1.0"?><c:chartSpace xmlns:c="{NS_C}"><c:chart><c:plotArea><c:barChart><c:barDir val="col"/><c:ser><c:idx val="0"/><c:order val="0"/><c:val><c:numRef><c:f>Data!$B$2:$B$5</c:f></c:numRef></c:val></c:ser></c:barChart></c:plotArea></c:chart><c:extLst><c:ext uri="{{C3380CC4-5D21-4A18-A83A-4E5A6B4C5A11}}"><c15:unknownChartThing xmlns:c15="http://schemas.microsoft.com/office/drawing/2012/chart">keep the chart extension</c15:unknownChartThing></c:ext></c:extLst></c:chartSpace>"#
    );
    let content_types = format!(
        r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/xl/workbook.xml" ContentType="{CT_PREFIX}.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="{CT_PREFIX}.spreadsheetml.worksheet+xml"/><Override PartName="/xl/tables/table1.xml" ContentType="{CT_PREFIX}.spreadsheetml.table+xml"/><Override PartName="/xl/queryTables/queryTable2.xml" ContentType="{CT_PREFIX}.spreadsheetml.queryTable+xml"/><Override PartName="/xl/pivotTables/pivotTable1.xml" ContentType="{CT_PREFIX}.spreadsheetml.pivotTable+xml"/><Override PartName="/xl/pivotCache/pivotCacheDefinition1.xml" ContentType="{CT_PREFIX}.spreadsheetml.pivotCacheDefinition+xml"/><Override PartName="/xl/slicerCaches/slicerCache1.xml" ContentType="application/vnd.ms-excel.slicerCache+xml"/><Override PartName="/xl/slicers/slicer1.xml" ContentType="application/vnd.ms-excel.slicer+xml"/><Override PartName="/xl/timelines/timeline1.xml" ContentType="application/vnd.ms-excel.timeline+xml"/><Override PartName="/xl/timelineCaches/timelineCache1.xml" ContentType="application/vnd.ms-excel.timelineCache+xml"/><Override PartName="/xl/drawings/drawing1.xml" ContentType="{CT_PREFIX}.drawing+xml"/><Override PartName="/xl/charts/chart1.xml" ContentType="{CT_PREFIX}.drawingml.chart+xml"/></Types>"#
    );
    make_zip(&[
        ("[Content_Types].xml", content_types.as_bytes()),
        ("_rels/.rels", rels(&[("rId1", "officeDocument", "xl/workbook.xml")]).as_bytes()),
        ("xl/workbook.xml", workbook.as_bytes()),
        (
            "xl/_rels/workbook.xml.rels",
            rels(&[
                ("rId1", "worksheet", "worksheets/sheet1.xml"),
                ("rId2", "pivotCacheDefinition", "pivotCache/pivotCacheDefinition1.xml"),
                ("rId3", REL_SLICER_CACHE, "slicerCaches/slicerCache1.xml"),
                ("rId4", REL_TIMELINE_CACHE, "timelineCaches/timelineCache1.xml"),
            ])
            .as_bytes(),
        ),
        ("xl/worksheets/sheet1.xml", sheet.as_bytes()),
        (
            "xl/worksheets/_rels/sheet1.xml.rels",
            rels(&[
                ("rId1", "table", "../tables/table1.xml"),
                ("rId2", "pivotTable", "../pivotTables/pivotTable1.xml"),
                ("rId5", REL_TIMELINE, "../timelines/timeline1.xml"),
                ("rId6", "drawing", "../drawings/drawing1.xml"),
            ])
            .as_bytes(),
        ),
        ("xl/tables/table1.xml", TABLE_WITH_EXT.as_bytes()),
        ("xl/tables/_rels/table1.xml.rels", rels(&[("rId1", "queryTable", "../queryTables/queryTable2.xml")]).as_bytes()),
        ("xl/queryTables/queryTable2.xml", QUERY_TABLE.as_bytes()),
        ("xl/pivotTables/pivotTable1.xml", PIVOT_TABLE.as_bytes()),
        (
            "xl/pivotTables/_rels/pivotTable1.xml.rels",
            rels(&[
                ("rId1", "pivotCacheDefinition", "../pivotCache/pivotCacheDefinition1.xml"),
                ("rId2", REL_SLICER_CACHE, "../slicerCaches/slicerCache1.xml"),
            ])
            .as_bytes(),
        ),
        ("xl/pivotCache/pivotCacheDefinition1.xml", PIVOT_CACHE.as_bytes()),
        ("xl/slicerCaches/slicerCache1.xml", SLICER_CACHE.as_bytes()),
        ("xl/slicerCaches/_rels/slicerCache1.xml.rels", rels(&[("rId1", REL_SLICER, "../slicers/slicer1.xml")]).as_bytes()),
        ("xl/slicers/slicer1.xml", SLICER.as_bytes()),
        ("xl/timelines/timeline1.xml", TIMELINE.as_bytes()),
        ("xl/timelineCaches/timelineCache1.xml", TIMELINE_CACHE.as_bytes()),
        ("xl/drawings/drawing1.xml", drawing.as_bytes()),
        ("xl/drawings/_rels/drawing1.xml.rels", rels(&[("rId1", "chart", "../charts/chart1.xml")]).as_bytes()),
        ("xl/charts/chart1.xml", chart.as_bytes()),
    ])
}

/// The part the first relationship of `kind` on `owner` points at.
fn target_of_kind(u: &Unpacked, owner: &str, kind: &str) -> (String, String) {
    let r = u.rels(owner).into_iter().find(|r| r.1.ends_with(&format!("/{kind}"))).unwrap_or_else(|| panic!("{owner}: no {kind} relationship"));
    (r.0, r.2)
}

/// Parts that only the unmodelled content of this fixture produces.
fn is_kept_part(name: &str) -> bool {
    name.starts_with("xl/slicerCaches/")
        || name.starts_with("xl/slicers/")
        || name.starts_with("xl/timelines/")
        || name.starts_with("xl/timelineCaches/")
        || name.starts_with("xl/queryTables/")
}

fn kept_parts(u: &Unpacked) -> Vec<String> {
    let mut v: Vec<String> = u.parts.keys().filter(|n| is_kept_part(n)).cloned().collect();
    v.sort();
    v
}

#[test]
fn slicers_timelines_and_owner_extensions_survive_a_round_trip() {
    let (wb, rep) = read_xlsx(&extension_package()).unwrap();
    assert_eq!(wb.sheet(0).unwrap().pivots.len(), 1, "the PivotTable is modelled");
    assert_eq!(wb.sheet(0).unwrap().tables.len(), 1, "the table is modelled");
    assert!(!rep.warnings.iter().any(|w| w.contains("dropped")), "{:?}", rep.warnings);

    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();

    // The slicer cache and its slicer, the timeline and its cache, and the table's query table:
    // part bytes unchanged, each still where a reader looks for it.
    for data in [SLICER_CACHE.as_bytes(), SLICER.as_bytes(), TIMELINE.as_bytes(), TIMELINE_CACHE.as_bytes(), QUERY_TABLE.as_bytes()] {
        assert_eq!(u.find(data).len(), 1, "kept part missing or duplicated: {}", String::from_utf8_lossy(data));
    }
    for name in [
        "xl/slicerCaches/slicerCache1.xml",
        "xl/slicers/slicer1.xml",
        "xl/timelines/timeline1.xml",
        "xl/timelineCaches/timelineCache1.xml",
        "xl/queryTables/queryTable2.xml",
    ] {
        assert!(u.parts.contains_key(name), "{name} lost its name: {:?}", u.parts.keys());
    }
    let wbx = u.text("xl/workbook.xml");
    let (_, cache) = target_of_kind(&u, "xl/workbook.xml", "slicerCache");
    assert_eq!(u.parts.get(&cache).unwrap(), SLICER_CACHE.as_bytes());
    let (_, timeline_cache) = target_of_kind(&u, "xl/workbook.xml", "timelineCache");
    assert_eq!(u.parts.get(&timeline_cache).unwrap(), TIMELINE_CACHE.as_bytes());
    // The workbook extensions that name them, plus the one we don't model.
    assert!(wbx.contains(EXT_SLICER_CACHES) && wbx.contains("slicerCaches"), "{wbx}");
    assert!(wbx.contains(EXT_TIMELINES) && wbx.contains("timelineCacheRefs"), "{wbx}");
    assert!(wbx.contains("keep the workbook extension"), "{wbx}");
    let slicer_id = attr_of(&wbx[wbx.find("<x14:slicerCache ").unwrap()..], "x14:slicerCache", "r:id");
    assert_eq!(u.target("xl/workbook.xml", &slicer_id).1, cache);

    // The worksheet's timeline and its own unknown extension.
    let sheet = sheet_part(&u, "Data");
    let sx = u.text(&sheet);
    assert!(sx.contains("timelines") && sx.contains("keep the sheet extension"), "{sx}");
    let timeline_id = attr_of(&sx[sx.find("<x15:timeline ").unwrap()..], "x15:timeline", "r:id");
    let (ty, timeline) = u.target(&sheet, &timeline_id);
    assert!(ty.ends_with("/timeline"), "{ty}");
    assert_eq!(u.parts.get(&timeline).unwrap(), TIMELINE.as_bytes());

    // The PivotTable keeps the extension that names its slicer cache and its own unknown one; the
    // extension GridCraft models is never kept from the source too (it is written from the model,
    // and only when the model asks for it — so at most one, never two).
    let (_, pivot) = target_of_kind(&u, &sheet, "pivotTable");
    let px = u.text(&pivot);
    assert!(px.contains(EXT_PIVOT_SLICER_CACHE) && px.contains("slicerCacheData"), "{px}");
    assert!(px.matches(EXT_PIVOT_MODELLED).count() <= 1, "the modelled extension is written twice: {px}");
    assert!(px.contains("keep the pivot extension"), "{px}");
    let slicer_id = attr_of(&px[px.find("<x14:slicerCache ").unwrap()..], "x14:slicerCache", "r:id");
    assert_eq!(u.target(&pivot, &slicer_id).1, cache);
    // The pivot cache definition keeps its own extension.
    let (_, pivot_cache) = target_of_kind(&u, &pivot, "pivotCacheDefinition");
    assert!(u.text(&pivot_cache).contains("keep the cache extension"));

    // The table keeps its extension and the query table hanging off it.
    let (_, table) = target_of_kind(&u, &sheet, "table");
    assert!(u.text(&table).contains("keep the table extension"));
    let (_, query_table) = target_of_kind(&u, &table, "queryTable");
    assert_eq!(u.parts.get(&query_table).unwrap(), QUERY_TABLE.as_bytes());

    // Unknown chart extensions survive as well now that gap #4 landed: the chart part is kept
    // byte-identically while the chart is untouched. Editing the chart rewrites the part from the
    // model, which is where the unmodelled content still goes (documented in chart.rs).
    let (_, drawing) = target_of_kind(&u, &sheet, "drawing");
    let (_, chart) = target_of_kind(&u, &drawing, "chart");
    assert!(u.text(&chart).contains("unknownChartThing"));

    // Reading the output keeps the same content again: the pass-through is stable.
    let (wb2, rep2) = read_xlsx(&out).unwrap();
    assert!(!rep2.warnings.iter().any(|w| w.contains("dropped")), "{:?}", rep2.warnings);
    let u2 = Unpacked::new(&write_xlsx(&wb2).unwrap());
    u2.assert_valid();
    for data in [SLICER_CACHE.as_bytes(), SLICER.as_bytes(), TIMELINE.as_bytes(), TIMELINE_CACHE.as_bytes(), QUERY_TABLE.as_bytes()] {
        assert_eq!(u2.find(data).len(), 1, "kept part lost on the second save: {}", String::from_utf8_lossy(data));
    }
    assert!(u2.text("xl/workbook.xml").contains("keep the workbook extension"));
    assert!(u2.text(&sheet_part(&u2, "Data")).contains("keep the sheet extension"));
    // The kept chart part is stable across a second save too.
    let (_, drawing2) = target_of_kind(&u2, &sheet_part(&u2, "Data"), "drawing");
    let (_, chart2) = target_of_kind(&u2, &drawing2, "chart");
    assert!(u2.text(&chart2).contains("unknownChartThing"));
}

#[test]
fn a_deleted_pivottable_takes_its_slicer_and_timeline_caches_with_it() {
    let (mut wb, _) = read_xlsx(&extension_package()).unwrap();
    wb.sheet_mut(0).unwrap().pivots.clear();
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    // Nothing that names PivotTable1 is left: no cache parts, no extension referring to them.
    assert_eq!(kept_parts(&u), ["xl/queryTables/queryTable2.xml"], "a PivotTable's caches went with it");
    assert!(!u.text("xl/workbook.xml").contains("slicerCaches"));
    assert!(!u.text("xl/workbook.xml").contains("timelineCache"));
    assert!(!u.text(&sheet_part(&u, "Data")).contains("timelines"));
    // An extension that names no part is kept: it may be anything.
    assert!(u.text("xl/workbook.xml").contains("keep the workbook extension"));
    assert!(u.text(&sheet_part(&u, "Data")).contains("keep the sheet extension"));
}

#[test]
fn a_renamed_table_loses_its_query_table_and_extensions() {
    let (mut wb, _) = read_xlsx(&extension_package()).unwrap();
    wb.sheet_mut(0).unwrap().tables[0].name = "Renamed".into();
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    assert_eq!(u.find(QUERY_TABLE.as_bytes()).len(), 0, "the query table named the table it was attached to");
    let sheet = sheet_part(&u, "Data");
    let (_, table) = target_of_kind(&u, &sheet, "table");
    let tx = u.text(&table);
    assert!(!tx.contains("keep the table extension"), "{tx}");
    assert!(!tx.contains("<extLst"), "{tx}");
}

#[test]
fn deleting_the_sheet_takes_its_slicers_table_and_timeline() {
    let (mut wb, _) = read_xlsx(&extension_package()).unwrap();
    wb.sheets = vec![Arc::new(Sheet::new("Other"))];
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    assert!(kept_parts(&u).is_empty(), "{:?}", kept_parts(&u));
    assert!(!u.text("xl/workbook.xml").contains("slicerCache"));
    assert!(!u.text("xl/workbook.xml").contains("timelineCache"));
}

#[test]
fn a_renamed_pivottable_loses_its_slicer_and_timeline_caches() {
    let (mut wb, _) = read_xlsx(&extension_package()).unwrap();
    wb.sheet_mut(0).unwrap().pivots[0].name = "Renamed".into();
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    // Both the caches and the slicers and timelines behind them go: a slicer for a report that no
    // longer exists is worse than no slicer, and a part nothing relates to is a repair prompt.
    assert_eq!(kept_parts(&u), ["xl/queryTables/queryTable2.xml"], "{:?}", kept_parts(&u));
    assert!(!u.text("xl/workbook.xml").contains("slicerCaches"));
    assert!(!u.text(&sheet_part(&u, "Data")).contains("timelines"));
    // The PivotTable itself still saves, with no extension pointing at a part the file lacks.
    let sheet = sheet_part(&u, "Data");
    let (_, pivot) = target_of_kind(&u, &sheet, "pivotTable");
    let px = u.text(&pivot);
    assert!(!px.contains("slicerCacheData"), "{px}");
    // Extras hang off the owner name, so a renamed PivotTable loses its own extensions too: an
    // extension may name the report it belongs to, and attaching it to another one is worse.
    assert!(!px.contains("pivotThing"), "{px}");
}

#[test]
fn editing_a_cell_keeps_the_slicers_and_pivot_extensions() {
    let (mut wb, _) = read_xlsx(&extension_package()).unwrap();
    wb.sheet_mut(0).unwrap().set_value(gridcraft_core::CellRef::new(9, 0), gridcraft_core::Value::Number(7.0));
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    for data in [SLICER_CACHE.as_bytes(), SLICER.as_bytes(), TIMELINE.as_bytes(), TIMELINE_CACHE.as_bytes(), QUERY_TABLE.as_bytes()] {
        assert_eq!(u.find(data).len(), 1, "an edit dropped a kept part: {}", String::from_utf8_lossy(data));
    }
    let sheet = sheet_part(&u, "Data");
    let (_, pivot) = target_of_kind(&u, &sheet, "pivotTable");
    assert!(u.text(&pivot).contains("slicerCacheData"));
}
