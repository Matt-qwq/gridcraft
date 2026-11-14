//! Chart sheets: a sheet that is one chart and no cells (Excel's Move Chart ▸ New sheet). Its part
//! is read into the model, written back as a real `chartsheet` part, and a package that cannot
//! represent one keeps it whole rather than half-modelling it.

use std::io::Read;
use std::sync::Arc;

use gridcraft_core::{CellRef, Value};
use gridcraft_model::{Anchor, Chart, ChartKind, LegendPos, Series, Sheet, SheetKind, Visibility, Workbook};

use crate::testing::check_package;
use crate::{read_xlsx, write_xlsx};

fn chart(id: u32, title: &str) -> Chart {
    Chart {
        id,
        kind: ChartKind::ColumnClustered,
        anchor: Anchor::default(),
        title: Some(title.into()),
        series: vec![Series {
            name: Some("\"Q1\"".into()),
            categories: Some("Data!$A$2:$A$5".into()),
            values: "Data!$B$2:$B$5".into(),
            ..Series::default()
        }],
        legend: LegendPos::Bottom,
        ..Chart::default()
    }
}

fn data_sheet() -> Sheet {
    let mut s = Sheet::new("Data");
    for (row, (name, value)) in [("Region", 0), ("North", 48), ("South", 39), ("East", 55), ("West", 61)].iter().enumerate() {
        s.set_value(CellRef::new(row as u32, 0), Value::Text((*name).into()));
        if *value > 0 {
            s.set_value(CellRef::new(row as u32, 1), Value::Number(f64::from(*value)));
        }
    }
    s
}

/// A workbook mixing worksheets and chart sheets: Data, Chart1, Notes — the chart sheet in the
/// middle, so its position has to survive the file.
fn mixed_workbook() -> Workbook {
    let mut wb = Workbook::new();
    wb.sheets.clear();
    wb.sheets.push(Arc::new(data_sheet()));
    wb.sheets.push(Arc::new(Sheet::chart_sheet("Chart1", chart(9, "Quarterly sales"))));
    let mut notes = Sheet::new("Notes");
    notes.set_value(CellRef::new(0, 0), Value::Text("kept".into()));
    wb.sheets.push(Arc::new(notes));
    wb.active_sheet = 1;
    wb
}

fn packages(bytes: &[u8]) -> Vec<String> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
    let mut names: Vec<String> = (0..z.len()).filter_map(|i| z.by_index(i).ok().map(|f| f.name().to_string())).collect();
    names.sort();
    names
}

fn part(bytes: &[u8], name: &str) -> String {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
    let mut s = String::new();
    z.by_name(name).unwrap().read_to_string(&mut s).unwrap();
    s
}

fn count(names: &[String], prefix: &str) -> usize {
    names.iter().filter(|n| n.starts_with(prefix) && !n.contains("_rels")).count()
}

#[test]
fn a_chart_sheet_round_trips_with_its_chart_and_position() {
    let bytes = write_xlsx(&mixed_workbook()).unwrap();
    check_package(&bytes).expect("the package with a chart sheet is valid");

    // Exactly one chart sheet part, one drawing and one chart: a sheet kept as it was *and*
    // modelled would give the package two of each.
    let names = packages(&bytes);
    assert_eq!(count(&names, "xl/chartsheets/sheet"), 1, "{names:?}");
    assert_eq!(count(&names, "xl/drawings/drawing"), 1, "{names:?}");
    assert_eq!(count(&names, "xl/charts/chart"), 1, "{names:?}");
    assert!(names.iter().any(|n| n == "xl/chartsheets/sheet1.xml"));
    assert!(names.iter().any(|n| n == "xl/chartsheets/_rels/sheet1.xml.rels"));
    let cs = part(&bytes, "xl/chartsheets/sheet1.xml");
    assert!(cs.contains("<chartsheet"), "{cs}");
    assert!(cs.contains("<drawing r:id=\"rId1\"/>"), "{cs}");
    // The workbook names it a chart sheet, between the two worksheets.
    let wb = part(&bytes, "xl/workbook.xml");
    let chart_at = wb.find("name=\"Chart1\"").expect("the chart sheet entry");
    assert!(chart_at > wb.find("name=\"Data\"").unwrap_or(0), "{wb}");
    assert!(chart_at < wb.find("name=\"Notes\"").unwrap_or(0), "{wb}");
    let wb_rels = part(&bytes, "xl/_rels/workbook.xml.rels");
    assert!(wb_rels.contains("chartsheets/sheet1.xml"), "{wb_rels}");

    // Read back: same sheets in the same order, the chart sheet still one chart and no cells.
    let (back, report) = read_xlsx(&bytes).unwrap();
    assert!(report.warnings.iter().all(|w| !w.contains("Chart1")), "{:?}", report.warnings);
    let kinds: Vec<(&str, SheetKind)> = back.sheets.iter().map(|s| (s.name.as_str(), s.kind)).collect();
    assert_eq!(kinds, vec![("Data", SheetKind::Worksheet), ("Chart1", SheetKind::ChartSheet), ("Notes", SheetKind::Worksheet)]);
    assert_eq!(back.active_sheet, 1);
    let cs = back.sheet(1).expect("the chart sheet");
    assert!(cs.cells.is_empty(), "a chart sheet has no cells");
    let ch = cs.chart_sheet_chart().expect("the chart sheet's chart");
    assert_eq!(ch.kind, ChartKind::ColumnClustered);
    assert_eq!(ch.title.as_deref(), Some("Quarterly sales"));
    assert_eq!(ch.legend, LegendPos::Bottom);
    assert_eq!(ch.series.len(), 1);
    assert_eq!(ch.series[0].values, "Data!$B$2:$B$5");
    assert_eq!(ch.series[0].categories.as_deref(), Some("Data!$A$2:$A$5"));
    // The worksheets kept their values.
    assert_eq!(back.sheets[0].value(CellRef::new(1, 1)), Value::Number(48.0));
    assert_eq!(back.sheets[2].value(CellRef::new(0, 0)), Value::Text("kept".into()));

    // And a second save is stable: still one of each part.
    let again = write_xlsx(&back).unwrap();
    check_package(&again).expect("the saved chart sheet package is valid");
    let names = packages(&again);
    assert_eq!(count(&names, "xl/chartsheets/sheet"), 1, "{names:?}");
    assert_eq!(count(&names, "xl/drawings/drawing"), 1, "{names:?}");
    assert_eq!(count(&names, "xl/charts/chart"), 1, "{names:?}");
}

#[test]
fn chart_sheet_names_stay_unique_beside_worksheets() {
    let mut wb = Workbook::new();
    wb.sheets.clear();
    wb.sheets.push(Arc::new(Sheet::chart_sheet("Chart1", chart(1, "A"))));
    wb.sheets.push(Arc::new(Sheet::new("Sheet1")));
    wb.sheets.push(Arc::new(Sheet::chart_sheet("Chart2", chart(2, "B"))));
    let bytes = write_xlsx(&wb).unwrap();
    let (back, _) = read_xlsx(&bytes).unwrap();
    assert_eq!(back.sheets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["Chart1", "Sheet1", "Chart2"]);
    assert_eq!(back.sheets.iter().filter(|s| s.is_chart_sheet()).count(), 2);
    assert_eq!(back.next_chart_sheet_name(), "Chart3");
    assert_eq!(back.next_sheet_name(), "Sheet2");
    // A chart sheet can be hidden, renamed and moved like any other sheet.
    let mut wb = back.clone();
    let mut moved = (*wb.sheets[0]).clone();
    moved.name = "Sales chart".into();
    moved.visibility = Visibility::Hidden;
    wb.sheets[0] = Arc::new(moved);
    wb.sheets.swap(0, 1);
    let bytes = write_xlsx(&wb).unwrap();
    check_package(&bytes).expect("the moved chart sheet package is valid");
    let (back, _) = read_xlsx(&bytes).unwrap();
    assert_eq!(back.sheets[0].name, "Sheet1");
    assert_eq!(back.sheets[1].name, "Sales chart");
    assert!(back.sheets[1].is_chart_sheet());
    assert_eq!(back.sheets[1].visibility, Visibility::Hidden);
    assert_eq!(back.sheets[1].chart_sheet_chart().and_then(|c| c.title.clone()).as_deref(), Some("A"));
}

/// A chart sheet's own page layout — margins, page setup, header and footer, and the printer
/// settings behind \`<pageSetup r:id>\` — is kept through a save even though the model has no room
/// for it on a chart sheet.
#[test]
fn a_chart_sheets_page_setup_and_header_are_kept() {
    let bytes = write_xlsx(&mixed_workbook()).unwrap();
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut parts: Vec<(String, Vec<u8>)> = vec![];
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let name = f.name().to_string();
        let mut data = vec![];
        f.read_to_end(&mut data).unwrap();
        match name.as_str() {
            "xl/chartsheets/sheet1.xml" => {
                data = concat!(
                    "<chartsheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" ",
                    "xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">",
                    "<sheetPr><tabColor rgb=\"FFFF0000\"/></sheetPr>",
                    "<sheetViews><sheetView tabSelected=\"1\" zoomScale=\"85\" workbookViewId=\"0\"/></sheetViews>",
                    "<pageMargins left=\"1\" right=\"1\" top=\"1\" bottom=\"1\" header=\"0.5\" footer=\"0.5\"/>",
                    "<pageSetup r:id=\"rId2\" paperSize=\"9\" orientation=\"landscape\"/>",
                    "<headerFooter><oddHeader>&amp;LChart sheet</oddHeader></headerFooter>",
                    "<drawing r:id=\"rId1\"/></chartsheet>"
                )
                .as_bytes()
                .to_vec();
            }
            "xl/chartsheets/_rels/sheet1.xml.rels" => {
                data = concat!(
                    "<?xml version=\"1.0\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
                    "<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing\" Target=\"../drawings/drawing1.xml\"/>",
                    "<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/printerSettings\" Target=\"../printerSettings/printerSettings1.bin\"/>",
                    "</Relationships>"
                )
                .as_bytes()
                .to_vec();
            }
            "[Content_Types].xml" => {
                let text = String::from_utf8(data).unwrap().replace(
                    "</Types>",
                    "<Default Extension=\"bin\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.printerSettings\"/></Types>",
                );
                data = text.into_bytes();
            }
            _ => {}
        }
        parts.push((name, data));
    }
    drop(z);
    parts.push(("xl/printerSettings/printerSettings1.bin".into(), b"printer bytes".to_vec()));
    let file = super::make_zip(&parts.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect::<Vec<_>>());

    let (wb, report) = read_xlsx(&file).unwrap();
    assert!(!report.warnings.iter().any(|w| w.contains("page setup") || w.contains("pageSetup")), "{:?}", report.warnings);
    assert!(wb.sheet(1).is_some_and(|s| s.is_chart_sheet()));
    let out = write_xlsx(&wb).unwrap();
    check_package(&out).expect("the package keeping the chart sheet's page layout is valid");
    let cs = part(&out, "xl/chartsheets/sheet1.xml");
    assert!(cs.contains("<pageMargins") && cs.contains("left=\"1\""), "{cs}");
    assert!(cs.contains("<pageSetup") && cs.contains("orientation=\"landscape\""), "{cs}");
    assert!(cs.contains("<oddHeader>&amp;LChart sheet</oddHeader>"), "{cs}");
    assert!(cs.contains("zoomScale=\"85\""), "{cs}");
    let rels = part(&out, "xl/chartsheets/_rels/sheet1.xml.rels");
    assert!(rels.contains("printerSettings"), "the printer settings part is related: {rels}");
    assert!(packages(&out).iter().any(|n| n.contains("printerSettings")), "the printer settings part is written");
    // And reading it back still gives the chart sheet, with its chart.
    let (back, _) = read_xlsx(&out).unwrap();
    assert_eq!(back.sheets.len(), 3);
    assert_eq!(back.sheet(1).and_then(|s| s.chart_sheet_chart()).map(|c| c.title.clone()), Some(Some("Quarterly sales".into())));

    // A tab colour set on the chart sheet in the model reaches the file: the sheet's own
    // <sheetPr> is the model's, as it is for a worksheet.
    let mut wb = back.clone();
    let mut cs = (*wb.sheets[1]).clone();
    cs.tab_color = gridcraft_model::Color::from_hex("00FF00");
    wb.sheets[1] = Arc::new(cs);
    let out = write_xlsx(&wb).unwrap();
    check_package(&out).expect("the package with a chart sheet tab colour is valid");
    let part_xml = part(&out, "xl/chartsheets/sheet1.xml");
    assert!(part_xml.contains("<tabColor"), "the tab colour is written: {part_xml}");
    let (back, _) = read_xlsx(&out).unwrap();
    assert!(back.sheet(1).is_some_and(|s| s.is_chart_sheet()));
}

/// The parts of a valid package, minus a few, as a zip: what a hostile file looks like.
fn without(bytes: &[u8], gone: &[&str]) -> Vec<u8> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
    let mut parts: Vec<(String, Vec<u8>)> = vec![];
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let name = f.name().to_string();
        if gone.contains(&name.as_str()) {
            continue;
        }
        let mut data = vec![];
        f.read_to_end(&mut data).unwrap();
        parts.push((name, data));
    }
    drop(z);
    super::make_zip(&parts.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect::<Vec<_>>())
}

/// A chart sheet whose chart part is missing: the reader warns and does not model the sheet, and
/// nothing panics (the sheet is one the reader cannot represent).
#[test]
fn a_chart_sheet_with_a_missing_chart_part_is_a_warning_not_a_panic() {
    let bytes = write_xlsx(&mixed_workbook()).unwrap();
    let hostile = without(&bytes, &["xl/charts/chart1.xml"]);
    let (wb, report) = read_xlsx(&hostile).expect("a missing chart part is a warning, not a failure");
    assert!(report.warnings.iter().any(|w| w.contains("Chart1")), "{:?}", report.warnings);
    assert_eq!(wb.sheets.len(), 2, "the chart sheet is not modelled: {:?}", wb.sheets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>());
    assert!(wb.sheets.iter().all(|s| !s.is_chart_sheet()));
    // The workbook still saves: whatever could not be modelled is either kept or warned about,
    // never half-written.
    let out = write_xlsx(&wb).unwrap();
    check_package(&out).expect("the package is still valid");
}

/// A chart sheet part with no `<drawing>`: the sheet is warned about and kept whole through the
/// pass-through instead of being modelled with no chart.
#[test]
fn a_chart_sheet_without_a_drawing_is_kept_whole() {
    let bytes = write_xlsx(&mixed_workbook()).unwrap();
    let mut hostile = without(&bytes, &[]);
    // Replace the chart sheet part with one that shows no chart.
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(std::mem::take(&mut hostile))).unwrap();
    let mut parts: Vec<(String, Vec<u8>)> = vec![];
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let name = f.name().to_string();
        let mut data = vec![];
        f.read_to_end(&mut data).unwrap();
        if name == "xl/chartsheets/sheet1.xml" {
            data = b"<chartsheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetViews><sheetView workbookViewId=\"0\"/></sheetViews></chartsheet>".to_vec();
        }
        parts.push((name, data));
    }
    drop(z);
    let hostile = super::make_zip(&parts.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect::<Vec<_>>());
    let (wb, report) = read_xlsx(&hostile).expect("a chart sheet without a drawing is a warning");
    assert!(report.warnings.iter().any(|w| w.contains("shows no drawing")), "{:?}", report.warnings);
    assert_eq!(wb.sheets.len(), 2);
    assert!(wb.sheets.iter().all(|s| !s.is_chart_sheet()));
    // Kept as it was byte for byte: the chart sheet still has its part and its chart on save.
    let out = write_xlsx(&wb).unwrap();
    check_package(&out).expect("the kept chart sheet package is valid");
    let names = packages(&out);
    assert!(names.iter().any(|n| n.starts_with("xl/chartsheets/")), "the chart sheet part is kept: {names:?}");
    assert!(names.iter().any(|n| n.starts_with("xl/charts/")), "its chart part is kept: {names:?}");
    let wb_xml = part(&out, "xl/workbook.xml");
    assert!(wb_xml.contains("name=\"Chart1\""), "the sheet entry is kept: {wb_xml}");
    let wb_rels = part(&out, "xl/_rels/workbook.xml.rels");
    assert!(wb_rels.contains("chartsheet"), "the sheet is related as a chart sheet: {wb_rels}");
}
