//! `.xls` embedded charts: the chart substream inside a worksheet is read into the model's chart
//! objects, and a kind the model cannot express is skipped with a warning instead of guessed.

use super::xls_tests::{bof, cat, cell, compound, eof, rec, u16s};
use crate::{Format, read_xls, sniff};

/// `BRAI` ([MS-XLS] 2.4.29): id, reference type, flags, number format, then the formula.
fn brai(id: u8, rt: u8, rgce: &[u8]) -> Vec<u8> {
    let mut p = vec![id, rt, 0, 0, 0, 0];
    p.extend_from_slice(&(rgce.len() as u16).to_le_bytes());
    p.extend_from_slice(rgce);
    rec(0x1051, &p)
}

/// A `SeriesText` with the record's two reserved bytes and a compressed ShortXLUnicodeString.
fn series_text(text: &str) -> Vec<u8> {
    let mut p = vec![0, 0, text.len() as u8, 0];
    p.extend_from_slice(text.as_bytes());
    rec(0x100D, &p)
}

/// A worksheet with one column chart: Sheet1!$B$1 (name), $B$2:$B$5 (values), $A$2:$A$5
/// (categories), a title and a legend. `kind_record` is `Bar` for a column chart; the caller may
/// pass `Chart3d` instead.
fn chart_file(kind_record: Vec<u8>, include_legend: bool) -> Vec<u8> {
    let series = cat(&[
        &rec(0x1003, &cat(&[&u16s(&[3, 4, 1, 4, 1, 0])])),
        &rec(0x1033, &[]),
        &brai(0, 2, &[0x3A, 0, 0, 0, 0, 1, 0]), // Sheet1!$B$1
        &series_text("Sales"),
        &brai(1, 2, &[0x3B, 0, 0, 1, 0, 4, 0, 1, 0, 1, 0]), // Sheet1!$B$2:$B$5
        &brai(2, 2, &[0x3B, 0, 0, 1, 0, 4, 0, 0, 0, 0, 0]), // Sheet1!$A$2:$A$5
        &rec(0x1034, &[]),
    ]);
    let mut chart = cat(&[
        &bof(0x0020),
        &rec(0x1002, &[0; 16]),      // Chart
        &rec(0x1033, &[]),           // Begin
        &rec(0x00A0, &[1, 0, 1, 0]), // Scl
        &rec(0x1064, &[0; 8]),       // PlotGrowth
        &rec(0x1014, &[0; 20]),      // ChartFormat
        &kind_record,
        &series,
        &rec(0x1044, &[0; 4]),             // ShtProps
        &series_text("Chart Title"),       // the chart's own title
        &rec(0x1027, &[1, 0, 0, 0, 0, 0]), // ObjectLink: wLinkObj = 1 (the title)
        &rec(0x1034, &[]),                 // End
    ]);
    if include_legend {
        chart.extend_from_slice(&rec(0x1015, &[0; 20]));
    }
    chart.extend_from_slice(&eof());
    let sheet = cat(&[
        &bof(0x0010),
        &rec(0x0203, &cat(&[&cell(0, 0, 0), &1.0f64.to_le_bytes()])),
        &rec(0x0203, &cat(&[&cell(1, 1, 0), &100.0f64.to_le_bytes()])),
        &chart,
        &eof(),
    ]);
    // A SUPBOOK and an EXTERNSHEET, so the 3-D reference tokens of the chart formulas resolve to
    // Sheet1 (the shape every real `.xls` workbook has).
    let globals = |pos: u32| {
        cat(&[
            &bof(0x0005),
            &rec(0x0085, &cat(&[&pos.to_le_bytes(), &[0, 0, 6, 0], b"Sheet1"])),
            &rec(0x01AE, &u16s(&[3, 0x0401])),
            &rec(0x0017, &cat(&[&u16s(&[1]), &u16s(&[0, 0, 0])])),
            &eof(),
        ])
    };
    let pos = globals(0).len() as u32;
    cat(&[&globals(pos), &sheet])
}

/// A clustered column `Bar` record: overlap 0, gap 50, no transpose/stacking.
fn column_group() -> Vec<u8> {
    rec(0x1017, &[0, 0, 50, 0, 0, 0])
}

#[test]
fn reads_an_embedded_column_chart() {
    let file = compound("Workbook", &chart_file(column_group(), true));
    assert_eq!(sniff(&file), Format::Xls);
    let (wb, report) = read_xls(&file).unwrap();
    let sheet = &wb.sheets[0];
    assert_eq!(sheet.charts.len(), 1, "{:?}", report.warnings);
    let chart = &sheet.charts[0];
    assert_eq!(chart.kind, gridcraft_model::ChartKind::ColumnClustered);
    assert_eq!(chart.title.as_deref(), Some("Chart Title"));
    assert_eq!(chart.legend, gridcraft_model::LegendPos::Bottom);
    assert!(chart.gridlines);
    assert_eq!(chart.series.len(), 1);
    let s = &chart.series[0];
    assert!(s.values.contains("$B$2:$B$5"), "values: {}", s.values);
    assert_eq!(s.categories.as_deref().map(|c| c.contains("$A$2:$A$5")), Some(true));
    assert!(s.name.as_deref().is_some_and(|n| n.contains("$B$1")), "name: {:?}", s.name);
}

#[test]
fn reads_a_doughnut_chart() {
    // Pie ([MS-XLS] 2.4.194): anStart, then pcDonut. A non-zero pcDonut is a doughnut; reading
    // anStart as pcDonut would make every pie whose first slice is rotated look like one.
    let pie = rec(0x1019, &[0, 0, 50, 0, 0, 0]);
    let file = compound("Workbook", &chart_file(pie, false));
    let (wb, _) = read_xls(&file).unwrap();
    assert_eq!(wb.sheets[0].charts[0].kind, gridcraft_model::ChartKind::Doughnut);

    let pie = rec(0x1019, &[0, 0, 0, 0, 0, 0]);
    let file = compound("Workbook", &chart_file(pie, false));
    let (wb, _) = read_xls(&file).unwrap();
    assert_eq!(wb.sheets[0].charts[0].kind, gridcraft_model::ChartKind::Pie);
}

#[test]
fn a_three_d_chart_is_skipped_with_a_warning_not_read_as_a_column_chart() {
    // `Chart3d` (0x103A) says the chart is 3-D; the model has no 3-D kind, so the chart is left
    // out instead of being imported as a flat column chart.
    let chart_records = cat(&[&rec(0x103A, &[0; 16]), &column_group()]);
    let file = compound("Workbook", &chart_file(chart_records, false));
    let (wb, report) = read_xls(&file).unwrap();
    assert!(wb.sheets[0].charts.is_empty());
    assert!(report.warnings.iter().any(|w| w.contains("chart")), "{:?}", report.warnings);
    // The sheet itself still opens.
    assert_eq!(wb.sheets[0].value(gridcraft_core::CellRef::parse("A1").unwrap()), gridcraft_core::Value::Number(1.0));
}

#[test]
fn a_damaged_chart_substream_does_not_panic() {
    // Every prefix of the chart records: a truncated record must be an error or a skipped chart,
    // never an index past the data.
    let full = chart_file(column_group(), false);
    for cut in (0..full.len()).step_by(29) {
        let bytes = compound("Workbook", &full[..cut]);
        let _ = read_xls(&bytes);
    }
}
