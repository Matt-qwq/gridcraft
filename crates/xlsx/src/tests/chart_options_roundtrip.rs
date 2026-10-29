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
            ("xl/charts/chart1.xml", &unmodelled_chart_xml()),
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
fn editing_one_option_rewrites_only_the_model_and_only_that_option_changes() {
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
    // Unmodelled content is replaced when the chart is edited: our writer cannot patch it.
    let part = chart_part(&back, "xl/charts/chart1.xml");
    assert!(!part.contains("printSettings"), "{part}");
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
