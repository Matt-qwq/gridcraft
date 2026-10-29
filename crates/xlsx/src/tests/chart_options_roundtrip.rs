//! Chart options round-trip through XLSX: axis scaling, data labels, error bars and per-point
//! formatting are read, written and read back the same; unmodelled chart XML survives an
//! open → save byte for byte while the chart is untouched.

use std::io::Read;
use std::sync::Arc;

use gridcraft_core::CellRef;
use gridcraft_model::{
    Anchor, AxisCrossing, AxisOptions, Chart, ChartKind, Color, DisplayUnits, ErrorBarDirection, ErrorBarKind, ErrorBars, LabelOptions,
    LabelPosition, LegendPos, PointFormat, Series, Sheet, Workbook,
};

use crate::{read_xlsx, write_xlsx};

fn series(name: &str, values: &str) -> Series {
    Series { name: Some(format!("\"{name}\"")), categories: Some("Data!$A$2:$A$5".into()), values: values.into(), ..Series::default() }
}

fn chart(id: u32, kind: ChartKind) -> Chart {
    Chart {
        id,
        kind,
        anchor: Anchor { cell: CellRef::new(id * 16, 3), width: 480.0, height: 300.0, ..Anchor::default() },
        title: Some(format!("Chart {id}")),
        series: vec![series("Sales", "Data!$B$2:$B$5")],
        legend: LegendPos::Bottom,
        style: 2,
        ..Chart::default()
    }
}

/// Every modelled option in one chart: it must come back exactly as it went in.
fn full_chart() -> Chart {
    let mut c = chart(1, ChartKind::ColumnClustered);
    c.x_axis = AxisOptions {
        min: Some(1.0),
        max: Some(5.0),
        major_unit: Some(2.0),
        log_base: None,
        reversed: true,
        crosses: AxisCrossing::At(2.0),
        display_units: None,
        number_format: Some("0".into()),
    };
    c.y_axis = AxisOptions {
        min: Some(0.0),
        max: Some(40.0),
        major_unit: Some(5.0),
        log_base: None,
        reversed: false,
        crosses: AxisCrossing::Min,
        display_units: Some(DisplayUnits::Thousands),
        number_format: Some("$#,##0".into()),
    };
    c.data_labels = true;
    c.labels = LabelOptions {
        show_value: true,
        show_percentage: true,
        show_series: true,
        show_category: true,
        show_legend_key: true,
        position: LabelPosition::OutsideEnd,
        separator: Some(" / ".into()),
        number_format: Some("0.0".into()),
        hidden: false,
    };
    c.series[0].labels = Some(LabelOptions { show_value: false, show_category: true, position: LabelPosition::Center, ..LabelOptions::default() });
    c.series[0].error_bars = Some(ErrorBars { direction: ErrorBarDirection::Plus, kind: ErrorBarKind::Fixed(3.5), no_end_cap: true });
    let mut second = series("Cost", "Data!$C$2:$C$5");
    second.labels = Some(LabelOptions { hidden: true, ..LabelOptions::default() });
    second.error_bars = Some(ErrorBars { direction: ErrorBarDirection::Both, kind: ErrorBarKind::Percentage(10.0), no_end_cap: false });
    let mut third = series("Plan", "Data!$D$2:$D$5");
    third.error_bars = Some(ErrorBars { direction: ErrorBarDirection::Minus, kind: ErrorBarKind::StdDev(2.0), no_end_cap: false });
    let mut fourth = series("Forecast", "Data!$E$2:$E$5");
    fourth.error_bars = Some(ErrorBars {
        direction: ErrorBarDirection::Both,
        kind: ErrorBarKind::Custom { plus: Some("{1,2,3,4}".into()), minus: Some("{4,3,2,1}".into()) },
        no_end_cap: false,
    });
    fourth.points = vec![PointFormat { index: 1, fill: Some(Color::Rgb(0xC00000)), line: Some(Color::Rgb(0x00B050)), marker: None, no_label: true }];
    c.series.extend([second, third, fourth]);
    c
}

fn bar_workbook() -> Workbook {
    let mut s = Sheet::new("Data");
    for (i, name) in ["Name", "Sales", "Cost", "Plan", "Forecast"].iter().enumerate() {
        s.set_value(CellRef::new(0, i as u32), gridcraft_core::Value::Text((*name).into()));
    }
    for r in 0..4u32 {
        for col in 1..=4u32 {
            s.set_value(CellRef::new(r + 1, col), gridcraft_core::Value::Number(f64::from(col * 10 + r)));
        }
    }
    s.charts.push(full_chart());
    Workbook { sheets: vec![Arc::new(s)], ..Workbook::default() }
}

fn chart_part(bytes: &[u8], name: &str) -> String {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
    let mut s = String::new();
    z.by_name(name).unwrap().read_to_string(&mut s).unwrap();
    s
}

#[test]
fn every_axis_label_error_and_point_option_round_trips() {
    let bytes = write_xlsx(&bar_workbook()).unwrap();
    let (back, _rep) = read_xlsx(&bytes).unwrap();
    let got = &back.sheets[0].charts[0];
    let want = full_chart();
    assert_eq!(got.x_axis, want.x_axis, "category axis");
    assert_eq!(got.y_axis, want.y_axis, "value axis");
    assert_eq!(got.labels, want.labels, "chart labels");
    assert_eq!(got.data_labels, want.data_labels);
    for (a, b) in got.series.iter().zip(&want.series) {
        assert_eq!(a.labels, b.labels, "{}: labels", b.name.as_deref().unwrap_or(""));
        assert_eq!(a.error_bars, b.error_bars, "{}: error bars", b.name.as_deref().unwrap_or(""));
        assert_eq!(a.points, b.points, "{}: point formats", b.name.as_deref().unwrap_or(""));
    }
    // A series that hides its labels is written as the format spells it: a deleted series dLbls.
    let part = chart_part(&bytes, "xl/charts/chart1.xml");
    assert!(part.contains("<c:dLbls><c:delete val=\"1\"/></c:dLbls>"), "{part}");
    // A second save writes the same part: nothing drifts on the way round.
    let again = write_xlsx(&back).unwrap();
    assert_eq!(chart_part(&again, "xl/charts/chart1.xml"), chart_part(&bytes, "xl/charts/chart1.xml"));
}

#[test]
fn a_log_axis_and_a_reversed_value_axis_round_trip() {
    let mut wb = bar_workbook();
    let mut sheet = (*wb.sheets[0]).clone();
    sheet.charts[0].y_axis = AxisOptions { log_base: Some(10.0), reversed: true, crosses: AxisCrossing::Max, ..AxisOptions::default() };
    sheet.charts[0].y_axis.display_units = None;
    // A marker override only exists on line and scatter points; keep one here to prove it too.
    let mut line = chart(2, ChartKind::LineMarkers);
    line.series[0].points = vec![PointFormat { index: 2, marker: Some(Color::Rgb(0x4472C4)), ..PointFormat::default() }];
    sheet.charts.push(line);
    wb.sheets = vec![Arc::new(sheet)];
    let (back, _rep) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    let y = &back.sheets[0].charts[0].y_axis;
    assert_eq!(y.log_base, Some(10.0));
    assert!(y.reversed);
    assert_eq!(y.crosses, AxisCrossing::Max);
    assert_eq!(
        back.sheets[0].charts[1].series[0].points,
        vec![PointFormat { index: 2, marker: Some(Color::Rgb(0x4472C4)), ..PointFormat::default() }]
    );
}

/// A chart part with options GridCraft has no model field for: a theme-coloured plot area, a
/// manual layout, a title's run formatting and an extension block.
fn unmodelled_chart_xml() -> String {
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><c:roundedCorners val="1"/><c:style val="42"/><c:chart><c:title><c:tx><c:rich><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr sz="1600" b="1"/></a:pPr><a:r><a:rPr lang="en-US" b="1"/><a:t>Sales &amp; costs</a:t></a:r></a:p></c:rich></c:tx><c:overlay val="1"/><c:spPr><a:solidFill><a:schemeClr val="accent2"/></a:solidFill></c:spPr></c:title><c:plotArea><c:layout><c:manualLayout><c:layoutTarget val="inner"/><c:xMode val="edge"/><c:yMode val="edge"/><c:x val="0.1"/><c:y val="0.12"/></c:manualLayout></c:layout><c:barChart><c:barDir val="col"/><c:grouping val="clustered"/><c:varyColors val="0"/><c:ser><c:idx val="0"/><c:order val="0"/><c:cat><c:strRef><c:f>Data!$A$2:$A$5</c:f></c:strRef></c:cat><c:val><c:numRef><c:f>Data!$B$2:$B$5</c:f></c:numRef></c:val><c:extLst><c:ext uri="{12345678-1234-1234-1234-1234567890AB}"><c:unknownSerThing val="7"/></c:ext></c:extLst></c:ser><c:dLbls><c:showVal val="1"/><c:showLeaderLines val="1"/><c:leaderLines><c:spPr><a:ln w="9525"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:ln></c:spPr></c:leaderLines></c:dLbls><c:gapWidth val="150"/><c:axId val="1"/><c:axId val="2"/></c:barChart><c:catAx><c:axId val="1"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="b"/><c:numFmt formatCode="General" sourceLinked="1"/><c:majorTickMark val="out"/><c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/><c:crossAx val="2"/><c:crosses val="autoZero"/><c:auto val="1"/><c:lblAlgn val="ctr"/><c:lblOffset val="100"/><c:noMultiLvlLbl val="0"/></c:catAx><c:valAx><c:axId val="2"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="l"/><c:majorGridlines/><c:numFmt formatCode="General" sourceLinked="1"/><c:majorTickMark val="out"/><c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/><c:crossAx val="1"/><c:crosses val="autoZero"/><c:crossBetween val="between"/></c:valAx></c:plotArea><c:legend><c:legendPos val="b"/><c:overlay val="0"/></c:legend><c:plotVisOnly val="1"/><c:dispBlanksAs val="gap"/></c:chart><c:printSettings><c:headerFooter/><c:pageMargins b="0.75" l="0.7" r="0.7" t="0.75" header="0.3" footer="0.3"/><c:pageSetup/></c:printSettings><c:extLst><c:ext uri="{DEADBEEF-DEAD-BEEF-DEAD-BEEFDEADBEEF}"><c:unknownChartThing val="1"/></c:ext></c:extLst></c:chartSpace>"#
        .to_string()
}

fn chart_fixture() -> Vec<u8> {
    chart_package(&unmodelled_chart_xml())
}

/// A package holding one sheet of data and one chart part.
fn chart_package(part: &str) -> Vec<u8> {
    let drawing = r#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><xdr:twoCellAnchor><xdr:from><xdr:col>3</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>10</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>15</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><xdr:graphicFrame><xdr:nvGraphicFramePr><xdr:cNvPr id="2" name="Chart 1"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" r:id="rId1"/></a:graphicData></a:graphic></xdr:graphicFrame><xdr:clientData/></xdr:twoCellAnchor></xdr:wsDr>"#;
    let drels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="../charts/chart1.xml"/></Relationships>"#;
    let srels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing" Target="../drawings/drawing1.xml"/></Relationships>"#;
    let sheet = r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Name</t></is></c><c r="B1" t="inlineStr"><is><t>Sales</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>One</t></is></c><c r="B2"><v>1</v></c></row><row r="3"><c r="A3" t="inlineStr"><is><t>Two</t></is></c><c r="B3"><v>4</v></c></row><row r="4"><c r="A4" t="inlineStr"><is><t>Three</t></is></c><c r="B4"><v>9</v></c></row><row r="5"><c r="A5" t="inlineStr"><is><t>Four</t></is></c><c r="B5"><v>16</v></c></row></sheetData><drawing r:id="rId1"/>"#;
    super::minimal(
        sheet,
        &[
            ("xl/worksheets/_rels/sheet1.xml.rels", srels),
            ("xl/drawings/drawing1.xml", drawing),
            ("xl/drawings/_rels/drawing1.xml.rels", drels),
            ("xl/charts/chart1.xml", part),
        ],
        "",
        "",
    )
}

#[test]
fn unknown_chart_xml_comes_back_byte_identical() {
    let bytes = chart_fixture();
    let (wb, _rep) = read_xlsx(&bytes).unwrap();
    assert_eq!(wb.sheets[0].charts.len(), 1);
    assert!(wb.sheets[0].charts[0].unchanged_part().is_some(), "the part was not kept");
    let back = write_xlsx(&wb).unwrap();
    // The chart part is written back exactly as read: the title's run formatting, the manual
    // layout, the leader lines and both extension blocks survive.
    assert_eq!(chart_part(&back, "xl/charts/chart1.xml").as_bytes(), unmodelled_chart_xml().as_bytes());
}

#[test]
fn an_axis_edit_is_patched_into_the_part_and_keeps_what_it_does_not_model() {
    let bytes = chart_fixture();
    let (mut wb, _rep) = read_xlsx(&bytes).unwrap();
    {
        let sheet = Arc::make_mut(&mut wb.sheets[0]);
        let c = &mut sheet.charts[0];
        // Read with its own labels and no explicit axis scale.
        assert!(c.data_labels);
        assert_eq!(c.y_axis, AxisOptions::default());
        c.y_axis.min = Some(2.0);
        c.y_axis.max = Some(20.0);
    }
    let back = write_xlsx(&wb).unwrap();
    let (again, _rep) = read_xlsx(&back).unwrap();
    let c = &again.sheets[0].charts[0];
    assert_eq!((c.y_axis.min, c.y_axis.max), (Some(2.0), Some(20.0)));
    assert!(c.data_labels, "the options we do model are written too");
    // The edit is patched into the part: the title's run formatting, the manual layout, the leader
    // lines and the extension blocks are still there, and only the scaling changed.
    let part = chart_part(&back, "xl/charts/chart1.xml");
    assert!(part.contains("printSettings"), "{part}");
    assert!(part.contains("manualLayout"), "{part}");
    assert!(part.contains("showLeaderLines"), "{part}");
    assert!(part.contains("unknownChartThing"), "{part}");
    assert!(part.contains("<c:max val=\"20\"/><c:min val=\"2\"/>"), "{part}");
    assert!(part.contains("<c:showVal val=\"1\"/>"), "{part}");
}

#[test]
fn excel_style_labels_and_error_bars_are_read() {
    let xml = r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><c:chart><c:plotArea><c:lineChart><c:grouping val="standard"/><c:ser><c:idx val="0"/><c:order val="0"/><c:dLbls><c:numFmt formatCode="0.0&quot;%&quot;" sourceLinked="0"/><c:dLblPos val="t"/><c:showLegendKey val="0"/><c:showVal val="1"/><c:showCatName val="1"/><c:showSerName val="1"/><c:showPercent val="1"/><c:separator> | </c:separator></c:dLbls><c:dPt><c:idx val="2"/><c:invertIfNegative val="0"/><c:marker><c:symbol val="circle"/><c:spPr><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></c:spPr></c:marker><c:spPr><a:solidFill><a:srgbClr val="00FF00"/></a:solidFill><a:ln w="12700"><a:solidFill><a:srgbClr val="0000FF"/></a:solidFill></a:ln></c:spPr></c:dPt><c:errBars><c:errBarType val="minus"/><c:errValType val="percentage"/><c:noEndCap val="0"/><c:val val="7.5"/></c:errBars><c:cat><c:strRef><c:f>Data!$A$2:$A$5</c:f></c:strRef></c:cat><c:val><c:numRef><c:f>Data!$B$2:$B$5</c:f></c:numRef></c:val></c:ser><c:dLbls><c:showVal val="0"/><c:showCatName val="1"/></c:dLbls><c:axId val="1"/><c:axId val="2"/></c:lineChart><c:catAx><c:axId val="1"/><c:scaling><c:orientation val="maxMin"/><c:max val="4"/><c:min val="1"/></c:scaling><c:majorTickMark val="out"/><c:crossAx val="2"/><c:crosses val="max"/><c:tickLblSkip val="2"/></c:catAx><c:valAx><c:axId val="2"/><c:scaling><c:logBase val="10"/><c:orientation val="minMax"/></c:scaling><c:crossAx val="1"/><c:crossesAt val="5"/><c:majorUnit val="3"/><c:dispUnits><c:builtInUnit val="millions"/></c:dispUnits></c:valAx></c:plotArea></c:chart></c:chartSpace>"#;
    let root = crate::xml::parse(xml.as_bytes()).unwrap();
    let c = crate::chart::read_chart(&root).unwrap();
    assert!(c.data_labels);
    // The group shows categories; this series overrides it with every part, top-placed.
    assert_eq!(c.labels, LabelOptions { show_category: true, ..LabelOptions::default() }, "group-level labels");
    let s = &c.series[0];
    assert_eq!(
        s.labels,
        Some(LabelOptions {
            show_value: true,
            show_percentage: true,
            show_series: true,
            show_category: true,
            show_legend_key: false,
            position: LabelPosition::Top,
            separator: Some(" | ".into()),
            number_format: Some("0.0\"%\"".into()),
            hidden: false,
        }),
        "series-level labels"
    );
    assert_eq!(s.error_bars, Some(ErrorBars { direction: ErrorBarDirection::Minus, kind: ErrorBarKind::Percentage(7.5), no_end_cap: false }));
    assert_eq!(
        s.points,
        vec![PointFormat {
            index: 2,
            fill: Some(Color::Rgb(0x00FF00)),
            line: Some(Color::Rgb(0x0000FF)),
            marker: Some(Color::Rgb(0xFF0000)),
            no_label: false,
        }]
    );
    assert_eq!(c.x_axis.min, Some(1.0));
    assert_eq!(c.x_axis.max, Some(4.0));
    assert_eq!(c.x_axis.major_unit, Some(2.0));
    assert!(c.x_axis.reversed);
    assert_eq!(c.x_axis.crosses, AxisCrossing::Max);
    assert_eq!(c.y_axis.log_base, Some(10.0));
    assert_eq!(c.y_axis.crosses, AxisCrossing::At(5.0));
    assert_eq!(c.y_axis.major_unit, Some(3.0));
    assert_eq!(c.y_axis.display_units, Some(DisplayUnits::Millions));
    assert_eq!(c.y_axis.number_format, None, "linked-to-source axis format is automatic");
}

#[test]
fn a_deleted_point_label_is_read_and_written_in_the_series_labels() {
    let mut c = chart(1, ChartKind::LineMarkers);
    c.data_labels = true;
    c.labels = LabelOptions::values();
    c.series[0].points = vec![PointFormat { index: 0, no_label: true, ..PointFormat::default() }];
    let mut s = Sheet::new("Data");
    for (i, v) in [1.0, 2.0, 3.0, 4.0].iter().enumerate() {
        s.set_value(CellRef::new(i as u32 + 1, 0), gridcraft_core::Value::Number(*v));
        s.set_value(CellRef::new(i as u32 + 1, 1), gridcraft_core::Value::Number(*v * 2.0));
    }
    s.charts.push(c);
    let wb = Workbook { sheets: vec![Arc::new(s)], ..Workbook::default() };
    let bytes = write_xlsx(&wb).unwrap();
    let part = chart_part(&bytes, "xl/charts/chart1.xml");
    assert!(part.contains("<c:dLbl><c:idx val=\"0\"/><c:delete val=\"1\"/></c:dLbl>"), "{part}");
    let (back, _rep) = read_xlsx(&bytes).unwrap();
    assert_eq!(back.sheets[0].charts[0].series[0].points, vec![PointFormat { index: 0, no_label: true, ..PointFormat::default() }]);
}

#[test]
fn a_chart_that_labels_only_one_series_hides_the_others() {
    let xml = r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart><c:plotArea><c:barChart><c:barDir val="col"/><c:ser><c:idx val="0"/><c:order val="0"/><c:dLbls><c:showVal val="1"/></c:dLbls><c:val><c:numRef><c:f>Data!$B$2:$B$5</c:f></c:numRef></c:val></c:ser><c:ser><c:idx val="1"/><c:order val="1"/><c:val><c:numRef><c:f>Data!$C$2:$C$5</c:f></c:numRef></c:val></c:ser><c:axId val="1"/><c:axId val="2"/></c:barChart><c:valAx><c:axId val="2"/></c:valAx><c:catAx><c:axId val="1"/></c:catAx></c:plotArea></c:chart></c:chartSpace>"#;
    let root = crate::xml::parse(xml.as_bytes()).unwrap();
    let c = crate::chart::read_chart(&root).unwrap();
    assert!(c.data_labels);
    assert_eq!(c.series[0].labels, Some(LabelOptions::values()));
    assert_eq!(c.series[1].labels, Some(LabelOptions { hidden: true, ..LabelOptions::default() }));
    // And it stays that way round the writer.
    let mut sheet = Sheet::new("Data");
    for (i, v) in [1.0, 2.0, 3.0, 4.0].iter().enumerate() {
        sheet.set_value(CellRef::new(i as u32 + 1, 1), gridcraft_core::Value::Number(*v));
        sheet.set_value(CellRef::new(i as u32 + 1, 2), gridcraft_core::Value::Number(*v * 3.0));
    }
    let mut chart = c;
    chart.anchor = Anchor { cell: CellRef::new(0, 4), width: 480.0, height: 300.0, ..Anchor::default() };
    sheet.charts.push(chart);
    let wb = Workbook { sheets: vec![Arc::new(sheet)], ..Workbook::default() };
    let bytes = write_xlsx(&wb).unwrap();
    let (back, _rep) = read_xlsx(&bytes).unwrap();
    assert_eq!(back.sheets[0].charts[0].series[1].labels, Some(LabelOptions { hidden: true, ..LabelOptions::default() }));
}

/// A chart part carrying options GridCraft has no model field for: a manual plot layout, a themed
/// plot-area fill, date-axis units, minor units and gridline formatting, a custom label callout
/// with leader lines, an error-bar line style, title run formatting, a legend-entry delete, a
/// series extension, a data table, print settings and chart/chartSpace extension blocks.
const RICH_PART: &str = r##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:c15="http://schemas.microsoft.com/office/drawing/2012/chart">
  <c:roundedCorners val="1"/>
  <c:style val="42"/>
  <c:chart>
    <c:title><c:tx><c:rich><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr sz="1600" b="1"/></a:pPr><a:r><a:rPr lang="en-US" b="1"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:rPr><a:t>Sales &amp; costs</a:t></a:r></a:p></c:rich></c:tx><c:overlay val="1"/><c:spPr><a:solidFill><a:schemeClr val="accent2"/></a:solidFill></c:spPr></c:title>
    <c:autoTitleDeleted val="0"/>
    <c:plotArea>
      <c:layout><c:manualLayout><c:layoutTarget val="inner"/><c:xMode val="edge"/><c:yMode val="edge"/><c:x val="0.1"/><c:y val="0.12"/></c:manualLayout></c:layout>
      <c:barChart>
        <c:barDir val="col"/><c:grouping val="clustered"/><c:varyColors val="0"/>
        <c:ser>
          <c:idx val="0"/><c:order val="0"/>
          <c:tx><c:strRef><c:f>Data!$B$1</c:f><c:strCache><c:ptCount val="1"/><c:pt idx="0"><c:v>Sales</c:v></c:pt></c:strCache></c:strRef></c:tx>
          <c:spPr><a:solidFill><a:srgbClr val="4472C4"/></a:solidFill><a:ln w="19050"><a:solidFill><a:srgbClr val="203864"/></a:solidFill></a:ln></c:spPr>
          <c:invertIfNegative val="0"/>
          <c:dPt><c:idx val="1"/><c:invertIfNegative val="1"/><c:explosion val="25"/><c:spPr><a:solidFill><a:srgbClr val="C00000"/></a:solidFill><a:ln w="28575"><a:solidFill><a:srgbClr val="00B050"/></a:solidFill></a:ln></c:spPr><c:pictureOptions><c:applyToFront val="0"/></c:pictureOptions></c:dPt>
          <c:dLbls>
            <c:dLbl><c:idx val="2"/><c:layout><c:manualLayout><c:x val="0.3"/><c:y val="0.4"/></c:manualLayout></c:layout><c:tx><c:rich><a:bodyPr/><a:p><a:r><a:rPr lang="en-US"/><a:t>Peak</a:t></a:r></a:p></c:rich></c:tx></c:dLbl>
            <c:numFmt formatCode="0.0&quot;%&quot;" sourceLinked="0"/>
            <c:showLegendKey val="0"/><c:showVal val="1"/><c:showCatName val="0"/><c:showSerName val="0"/><c:showPercent val="0"/><c:showBubbleSize val="0"/>
            <c:showLeaderLines val="1"/>
            <c:leaderLines><c:spPr><a:ln w="9525"><a:solidFill><a:srgbClr val="7F7F7F"/></a:solidFill></a:ln></c:spPr></c:leaderLines>
          </c:dLbls>
          <c:trendline><c:name>Forecast</c:name><c:trendlineType val="linear"/><c:dispRSqr val="1"/><c:dispEq val="1"/><c:trendlineLbl><c:numFmt formatCode="General" sourceLinked="0"/></c:trendlineLbl></c:trendline>
          <c:errBars><c:errBarType val="both"/><c:errValType val="fixedVal"/><c:val val="2"/><c:spPr><a:ln w="9525" cap="flat"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill><a:prstDash val="sysDash"/></a:ln></c:spPr></c:errBars>
          <c:cat><c:strRef><c:f>Data!$A$2:$A$5</c:f></c:strRef></c:cat>
          <c:val><c:numRef><c:f>Data!$B$2:$B$5</c:f></c:numRef></c:val>
          <c:extLst><c:ext uri="{11111111-1111-1111-1111-111111111111}"><c15:unknownSerThing val="7"/></c:ext></c:extLst>
        </c:ser>
        <c:dLbls><c:showVal val="1"/></c:dLbls>
        <c:gapWidth val="150"/><c:overlap val="25"/>
        <c:serLines><c:spPr><a:ln w="12700"/></c:spPr></c:serLines>
        <c:axId val="1"/><c:axId val="2"/>
      </c:barChart>
      <c:lineChart>
        <c:grouping val="standard"/><c:varyColors val="0"/>
        <c:ser><c:idx val="1"/><c:order val="1"/><c:tx><c:v>Cost</c:v></c:tx><c:marker><c:symbol val="circle"/><c:size val="7"/></c:marker><c:cat><c:strRef><c:f>Data!$A$2:$A$5</c:f></c:strRef></c:cat><c:val><c:numRef><c:f>Data!$C$2:$C$5</c:f></c:numRef></c:val><c:smooth val="0"/></c:ser>
        <c:marker val="1"/><c:axId val="3"/><c:axId val="4"/>
      </c:lineChart>
      <c:dateAx>
        <c:axId val="1"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="b"/>
        <c:title><c:tx><c:rich><a:bodyPr/><a:p><a:r><a:t>Month</a:t></a:r></a:p></c:rich></c:tx><c:overlay val="0"/></c:title>
        <c:numFmt formatCode="mmm" sourceLinked="0"/><c:majorTickMark val="out"/><c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/>
        <c:crossAx val="2"/><c:crosses val="autoZero"/><c:auto val="1"/><c:lblOffset val="100"/>
        <c:baseTimeUnit val="days"/><c:majorUnit val="1"/><c:majorTimeUnit val="months"/><c:minorUnit val="1"/><c:minorTimeUnit val="days"/>
      </c:dateAx>
      <c:valAx>
        <c:axId val="2"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="l"/>
        <c:majorGridlines><c:spPr><a:ln w="6350"><a:solidFill><a:srgbClr val="D9D9D9"/></a:solidFill></a:ln></c:spPr></c:majorGridlines>
        <c:title><c:tx><c:rich><a:bodyPr/><a:p><a:r><a:t>Amount</a:t></a:r></a:p></c:rich></c:tx><c:overlay val="0"/></c:title>
        <c:numFmt formatCode="#,##0" sourceLinked="0"/><c:majorTickMark val="out"/><c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/>
        <c:spPr><a:ln><a:noFill/></a:ln></c:spPr>
        <c:txPr><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr sz="900"/></a:pPr><a:endParaRPr lang="en-US"/></a:p></c:txPr>
        <c:crossAx val="1"/><c:crosses val="autoZero"/><c:crossBetween val="between"/>
        <c:minorUnit val="0.5"/>
        <c:dispUnits><c:builtInUnit val="thousands"/><c:dispUnitsLbl><c:layout/><c:tx><c:rich><a:bodyPr/><a:p><a:r><a:t>k</a:t></a:r></a:p></c:rich></c:tx></c:dispUnitsLbl></c:dispUnits>
      </c:valAx>
      <c:catAx><c:axId val="3"/><c:delete val="1"/><c:axPos val="b"/><c:crossAx val="4"/><c:crosses val="max"/></c:catAx>
      <c:valAx>
        <c:axId val="4"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="r"/>
        <c:minorGridlines><c:spPr><a:ln w="3175"><a:solidFill><a:srgbClr val="EEEEEE"/></a:solidFill></a:ln></c:spPr></c:minorGridlines>
        <c:numFmt formatCode="General" sourceLinked="1"/><c:majorTickMark val="out"/><c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/>
        <c:crossAx val="3"/><c:crosses val="max"/><c:crossBetween val="between"/>
        <c:minorUnit val="0.25"/>
      </c:valAx>
      <c:dTable><c:showHorzBorder val="1"/><c:showVertBorder val="1"/><c:showOutline val="1"/></c:dTable>
      <c:spPr><a:solidFill><a:schemeClr val="bg2"/></a:solidFill></c:spPr>
    </c:plotArea>
    <c:legend><c:legendPos val="r"/><c:legendEntry><c:idx val="1"/><c:delete val="1"/></c:legendEntry><c:layout/><c:overlay val="1"/></c:legend>
    <c:plotVisOnly val="1"/><c:dispBlanksAs val="gap"/>
    <c:extLst><c:ext uri="{22222222-2222-2222-2222-222222222222}"><c15:unknownChartThing val="1"/></c:ext></c:extLst>
  </c:chart>
  <c:printSettings><c:headerFooter/><c:pageMargins b="0.75" l="0.7" r="0.7" t="0.75" header="0.3" footer="0.3"/><c:pageSetup/></c:printSettings>
  <c:extLst><c:ext uri="{33333333-3333-3333-3333-333333333333}"><c15:chartSpaceThing val="2"/></c:ext></c:extLst>
</c:chartSpace>
"##;

/// Everything in [`RICH_PART`] that GridCraft has no field for. Each must still be in the part
/// after an edit and a save.
const SURVIVORS: &[&str] = &[
    "<c:manualLayout>",
    "<c:x val=\"0.1\"/>",
    "<a:schemeClr val=\"accent2\"/>",
    "<a:schemeClr val=\"bg2\"/>",
    "<c:baseTimeUnit val=\"days\"/>",
    "<c:majorTimeUnit val=\"months\"/>",
    "<c:minorTimeUnit val=\"days\"/>",
    "<c:minorUnit val=\"0.5\"/>",
    "<a:srgbClr val=\"D9D9D9\"/>",
    "<a:srgbClr val=\"EEEEEE\"/>",
    "<c:dispUnitsLbl>",
    "<c:showLeaderLines val=\"1\"/>",
    "<c:leaderLines>",
    "<a:prstDash val=\"sysDash\"/>",
    "<c:explosion val=\"25\"/>",
    "<c:pictureOptions>",
    "<a:t>Peak</a:t>",
    "<c:trendlineLbl>",
    "<a:defRPr sz=\"1600\" b=\"1\"/>",
    "<c:legendEntry>",
    "<c:dTable>",
    "<c:overlap val=\"25\"/>",
    "<c:serLines>",
    "<c:printSettings>",
    "unknownSerThing",
    "unknownChartThing",
    "chartSpaceThing",
];

fn rich_fixture() -> Vec<u8> {
    chart_package(RICH_PART)
}

fn part_of(bytes: &[u8]) -> String {
    chart_part(bytes, "xl/charts/chart1.xml")
}

#[test]
fn an_edit_patches_only_its_own_field_into_the_kept_part() {
    let (mut wb, _rep) = read_xlsx(&rich_fixture()).unwrap();
    {
        let sheet = Arc::make_mut(&mut wb.sheets[0]);
        assert_eq!(sheet.charts.len(), 1);
        let c = &mut sheet.charts[0];
        assert_eq!(c.kind, ChartKind::Combo);
        // Read from the part: what the fixture sets and what it leaves automatic.
        assert_eq!(c.y_axis.major_unit, None);
        assert_eq!(c.y_axis.number_format.as_deref(), Some("#,##0"));
        assert_eq!(c.y_axis.display_units, Some(DisplayUnits::Thousands));
        assert_eq!(c.y2_axis.crosses, AxisCrossing::Max);
        assert_eq!(c.x_axis.number_format.as_deref(), Some("mmm"));
        assert!(c.gridlines);
        assert!(c.data_labels);
        // The edit: one option, on the primary value axis.
        c.y_axis.major_unit = Some(3.0);
    }
    let back = write_xlsx(&wb).unwrap();
    let part = part_of(&back);
    for needle in SURVIVORS {
        assert!(part.contains(needle), "the edit dropped `{needle}`:\n{part}");
    }
    // The only difference from the part as read is the patched element itself.
    assert_eq!(part.replace("<c:majorUnit val=\"3\"/>", ""), RICH_PART, "the patch touched more than the edited field:\n{part}");
    // The secondary axis did not get one, and the date axis keeps its own units.
    assert_eq!(part.matches("<c:majorUnit val=\"3\"/>").count(), 1);
    assert!(part.contains("<c:majorUnit val=\"1\"/><c:majorTimeUnit val=\"months\"/>"), "date-axis units");
    // Read back: the option is there and everything else the model knows is unchanged.
    let (again, _rep) = read_xlsx(&back).unwrap();
    let c = &again.sheets[0].charts[0];
    assert_eq!(c.y_axis.major_unit, Some(3.0));
    assert_eq!(c.y_axis.number_format.as_deref(), Some("#,##0"));
    assert_eq!(c.y2_axis.crosses, AxisCrossing::Max);
    assert_eq!(c.x_axis.number_format.as_deref(), Some("mmm"));
    // A second save of the reopened workbook changes nothing.
    let twice = write_xlsx(&again).unwrap();
    assert_eq!(part_of(&twice), part);
    // Nor does saving the edited workbook a second time.
    assert_eq!(part_of(&write_xlsx(&wb).unwrap()), part);
}

#[test]
fn label_error_bar_and_point_edits_keep_the_rest_of_the_part() {
    let (mut wb, _rep) = read_xlsx(&rich_fixture()).unwrap();
    {
        let sheet = Arc::make_mut(&mut wb.sheets[0]);
        let c = &mut sheet.charts[0];
        let s = &mut c.series[0];
        let labels = s.labels.as_mut().expect("the series has its own label options");
        labels.position = LabelPosition::OutsideEnd;
        labels.show_category = true;
        s.error_bars.as_mut().expect("error bars").no_end_cap = true;
        assert_eq!(s.points.len(), 1);
        s.points[0].fill = Some(Color::Rgb(0x123456));
    }
    let back = write_xlsx(&wb).unwrap();
    let part = part_of(&back);
    for needle in SURVIVORS {
        assert!(part.contains(needle), "the edit dropped `{needle}`:\n{part}");
    }
    // The label callout and its leader lines are still where they were.
    assert!(part.contains("<a:t>Peak</a:t>"), "custom label text");
    assert!(part.contains("<a:srgbClr val=\"7F7F7F\"/>"), "leader line colour");
    // The edited options are the ones the model holds now.
    assert!(part.contains("<c:dLblPos val=\"outEnd\"/>"), "{part}");
    assert!(part.contains("<c:showCatName val=\"1\"/>"), "{part}");
    assert!(part.contains("<c:noEndCap val=\"1\"/>"), "{part}");
    assert!(part.contains("<a:srgbClr val=\"123456\"/>"), "the point's new fill");
    // The point keeps its explosion and picture options, and its outline.
    assert!(part.contains("<c:explosion val=\"25\"/>"));
    assert!(part.contains("<a:ln w=\"28575\"><a:solidFill><a:srgbClr val=\"00B050\"/>"), "the point's outline");
    // Round trip: the same model comes back.
    let (again, _rep) = read_xlsx(&back).unwrap();
    let s = &again.sheets[0].charts[0].series[0];
    let l = s.labels.as_ref().expect("labels");
    assert_eq!(l.position, LabelPosition::OutsideEnd);
    assert!(l.show_category);
    assert!(s.error_bars.as_ref().expect("error bars").no_end_cap);
    assert_eq!(s.points[0].fill, Some(Color::Rgb(0x123456)));
    // A second save is stable.
    assert_eq!(part_of(&write_xlsx(&again).unwrap()), part);
}

#[test]
fn turning_labels_off_and_on_again_patches_the_label_elements() {
    let (mut wb, _rep) = read_xlsx(&rich_fixture()).unwrap();
    {
        let sheet = Arc::make_mut(&mut wb.sheets[0]);
        sheet.charts[0].data_labels = false;
    }
    let off_bytes = write_xlsx(&wb).unwrap();
    let off = part_of(&off_bytes);
    // Labels are switched off the way Excel does it, with a delete flag, so the label formatting and
    // the callout text around it stay in the part.
    assert!(
        !off.contains("showVal val=\"1\""),
        "labels are still on:
{off}"
    );
    // Two label elements gain the flag; the legend entry and the hidden secondary category axis
    // already had one.
    assert_eq!(
        off.matches("delete val=\"1\"").count(),
        4,
        "group and series labels are switched off:
{off}"
    );
    assert!(off.contains("delete val=\"1\"/></c:dLbls>"), "{off}");
    for needle in SURVIVORS {
        assert!(
            off.contains(needle),
            "turning labels off dropped `{needle}`:
{off}"
        );
    }
    let (again, _rep) = read_xlsx(&off_bytes).unwrap();
    assert!(!again.sheets[0].charts[0].data_labels);
    // Back on: the group record gets its flags where the schema wants them, and the option survives.
    let mut wb = again;
    {
        let sheet = Arc::make_mut(&mut wb.sheets[0]);
        let c = &mut sheet.charts[0];
        c.data_labels = true;
        c.labels = LabelOptions { show_value: true, show_category: true, ..LabelOptions::default() };
    }
    let on = part_of(&write_xlsx(&wb).unwrap());
    assert!(on.contains("showCatName val=\"1\""), "{on}");
    assert!(on.contains("gapWidth val=\"150\""), "the group still has its own children: {on}");
    assert!(!on.contains("delete val=\"1\"/></c:dLbls>"), "the flag is gone again: {on}");
}

#[test]
fn adding_and_removing_trendlines_and_point_formats_patches_the_repeated_elements() {
    let (mut wb, _rep) = read_xlsx(&rich_fixture()).unwrap();
    {
        let sheet = Arc::make_mut(&mut wb.sheets[0]);
        let s = &mut sheet.charts[0].series[0];
        assert_eq!(s.trendlines.len(), 1);
        s.trendlines.clear();
        // A second point format, then the first one dropped again.
        s.set_point(PointFormat { index: 3, fill: Some(Color::Rgb(0x00B0F0)), ..PointFormat::default() });
    }
    let part = part_of(&write_xlsx(&wb).unwrap());
    assert!(!part.contains("<c:trendline>"), "the trendline element is gone:\n{part}");
    assert!(part.contains("<c:dPt><c:idx val=\"3\"/><c:spPr><a:solidFill><a:srgbClr val=\"00B0F0\"/>"), "{part}");
    // The new point sit before the label element, the error bars after it: the schema order holds.
    let pt = part.find("<c:idx val=\"3\"/>").unwrap_or_default();
    let dlbls = part.find("<c:dLbls>").unwrap_or_default();
    let err = part.find("<c:errBars>").unwrap_or_default();
    assert!(pt < dlbls && dlbls < err, "dPt/d:dLbls/errBars order:\n{part}");
    // And back: a trendline added to the model is written where the schema wants it.
    let (mut again, _rep) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    {
        let sheet = Arc::make_mut(&mut again.sheets[0]);
        let s = &mut sheet.charts[0].series[0];
        s.trendlines.push(gridcraft_model::Trendline {
            kind: gridcraft_model::TrendlineKind::Linear,
            name: Some("Plan".into()),
            forward: 2.0,
            ..gridcraft_model::Trendline::default()
        });
        s.points.retain(|p| p.index != 1);
    }
    let back = part_of(&write_xlsx(&again).unwrap());
    assert!(back.contains("<c:trendline><c:name>Plan</c:name>"), "{back}");
    // The cleared point keeps the parts GridCraft does not model and loses its colours.
    assert!(!back.contains("C00000") && !back.contains("00B050"), "the cleared point's colours are gone: {back}");
    assert!(back.contains("<c:idx val=\"1\"/><c:invertIfNegative val=\"1\"/><c:explosion val=\"25\"/>"), "the point's other options stay: {back}");
    // The trendline sits between the label element and the error bars.
    let tl = back.find("<c:trendline>").unwrap_or_default();
    let dlbls = back.find("<c:dLbls>").unwrap_or_default();
    let err = back.find("<c:errBars>").unwrap_or_default();
    assert!(dlbls < tl && tl < err, "dLbls/trendline/errBars order:\n{back}");
    for needle in SURVIVORS {
        // The trendline was removed in this test, and its label with it.
        if *needle == "<c:trendlineLbl>" {
            continue;
        }
        assert!(back.contains(needle), "the edit dropped `{needle}`:\n{back}");
    }
}
