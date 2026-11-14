//! Chart options round-trip through XLSX: axis scaling, data labels, error bars and per-point
//! formatting are read, written and read back the same; unmodelled chart XML survives an
//! open → save byte for byte while the chart is untouched.

use std::io::Read;
use std::sync::Arc;

use gridcraft_core::CellRef;
use gridcraft_model::{
    Anchor, AxisCrossing, AxisOptions, Chart, ChartKind, Color, DisplayUnits, ErrorBarDirection, ErrorBarKind, ErrorBars, Gridlines, LabelFont,
    LabelOptions, LabelPosition, LegendPos, PointFormat, PointLabel, Series, Sheet, Workbook,
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
        font: Some(LabelFont { size: Some(11.0), bold: Some(true), color: Some(Color::Rgb(0xC00000)) }),
    };
    c.series[0].labels = Some(LabelOptions {
        show_value: false,
        show_category: true,
        position: LabelPosition::Center,
        font: Some(LabelFont { size: Some(8.0), ..LabelFont::default() }),
        ..LabelOptions::default()
    });
    c.series[0].point_labels = vec![PointLabel {
        index: 2,
        text: Some("Check".into()),
        position: LabelPosition::Top,
        font: Some(LabelFont { bold: Some(true), color: Some(Color::Rgb(0x0070C0)), ..LabelFont::default() }),
        ..PointLabel::default()
    }];
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
    fourth.points = vec![PointFormat {
        index: 1,
        fill: Some(Color::Rgb(0xC00000)),
        line: Some(Color::Rgb(0x00B050)),
        marker: None,
        marker_size: None,
        no_label: true,
    }];
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
        assert_eq!(a.point_labels, b.point_labels, "{}: point labels", b.name.as_deref().unwrap_or(""));
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
    line.series[0].points = vec![PointFormat { index: 2, marker: Some(Color::Rgb(0x4472C4)), marker_size: Some(9), ..PointFormat::default() }];
    sheet.charts.push(line);
    wb.sheets = vec![Arc::new(sheet)];
    let (back, _rep) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    let y = &back.sheets[0].charts[0].y_axis;
    assert_eq!(y.log_base, Some(10.0));
    assert!(y.reversed);
    assert_eq!(y.crosses, AxisCrossing::Max);
    assert_eq!(
        back.sheets[0].charts[1].series[0].points,
        vec![PointFormat { index: 2, marker: Some(Color::Rgb(0x4472C4)), marker_size: Some(9), ..PointFormat::default() }]
    );
}

/// A chart part with options GridCraft has no model field for: a theme-coloured plot area, a
/// manual layout, a title's run formatting and an extension block.
fn unmodelled_chart_xml() -> String {
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><c:roundedCorners val="1"/><c:style val="42"/><c:chart><c:title><c:tx><c:rich><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr sz="1600" b="1"/></a:pPr><a:r><a:rPr lang="en-US" b="1"/><a:t>Sales &amp; costs</a:t></a:r></a:p></c:rich></c:tx><c:overlay val="1"/><c:spPr><a:solidFill><a:schemeClr val="accent2"/></a:solidFill></c:spPr></c:title><c:plotArea><c:layout><c:manualLayout><c:layoutTarget val="inner"/><c:xMode val="edge"/><c:yMode val="edge"/><c:x val="0.1"/><c:y val="0.12"/></c:manualLayout></c:layout><c:barChart><c:barDir val="col"/><c:grouping val="clustered"/><c:varyColors val="0"/><c:ser><c:idx val="0"/><c:order val="0"/><c:cat><c:strRef><c:f>Data!$A$2:$A$5</c:f></c:strRef></c:cat><c:val><c:numRef><c:f>Data!$B$2:$B$5</c:f></c:numRef></c:val><c:extLst><c:ext uri="{12345678-1234-1234-1234-1234567890AB}"><c:unknownSerThing val="7"/></c:ext></c:extLst></c:ser><c:dLbls><c:showVal val="1"/><c:showLeaderLines val="1"/><c:leaderLines><c:spPr><a:ln w="9525"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:ln></c:spPr></c:leaderLines></c:dLbls><c:gapWidth val="150"/><c:axId val="1"/><c:axId val="2"/></c:barChart><c:catAx><c:axId val="1"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="b"/><c:numFmt formatCode="General" sourceLinked="1"/><c:majorTickMark val="out"/><c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/><c:crossAx val="2"/><c:crosses val="autoZero"/><c:auto val="1"/><c:lblAlgn val="ctr"/><c:lblOffset val="100"/><c:noMultiLvlLbl val="0"/></c:catAx><c:valAx><c:axId val="2"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="l"/><c:majorGridlines/><c:numFmt formatCode="General" sourceLinked="1"/><c:majorTickMark val="out"/><c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/><c:crossAx val="1"/><c:crosses val="autoZero"/><c:crossBetween val="between"/></c:valAx></c:plotArea><c:legend><c:legendPos val="b"/><c:overlay val="0"/></c:legend><c:plotVisOnly val="1"/><c:dispBlanksAs val="gap"/></c:chart><c:printSettings><c:headerFooter/><c:pageMargins b="0.75" l="0.7" r="0.7" t="0.75" header="0.3" footer="0.3"/><c:pageSetup/></c:printSettings><c:extLst><c:ext uri="{DEADBEEF-DEAD-BEEF-DEAD-BEEFDEADBEEF}"><c:unknownChartThing val="1"/></c:ext></c:extLst></c:chartSpace>"#
        .to_string()
}

/// A chart style part as Excel writes one after the style gallery applies a style: a filled title
/// and a layered point line. GridCraft models none of it, so the part is the whole of the look.
const CUSTOM_STYLE: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
    "<cs:chartStyle xmlns:cs=\"http://schemas.microsoft.com/office/drawing/2012/chartStyle\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" id=\"201\">",
    "<cs:title><cs:lnRef idx=\"0\"/><cs:fillRef idx=\"3\"><a:schemeClr val=\"accent1\"/></cs:fillRef><cs:effectRef idx=\"0\"/><cs:fontRef idx=\"minor\"><a:schemeClr val=\"lt1\"/></cs:fontRef></cs:title>",
    "<cs:dataPointLine><cs:lnRef idx=\"2\"><a:schemeClr val=\"accent1\"><a:lumMod val=\"80000\"/></a:schemeClr></cs:lnRef><cs:fillRef idx=\"0\"/><cs:effectRef idx=\"0\"/><cs:fontRef idx=\"minor\"><a:schemeClr val=\"tx1\"/></cs:fontRef></cs:dataPointLine>",
    "</cs:chartStyle>"
);

/// A colour style part with its own cycling colours and variations.
const CUSTOM_COLORS: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
    "<cs:colorStyle xmlns:cs=\"http://schemas.microsoft.com/office/drawing/2012/chartStyle\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" meth=\"cycle\" id=\"10\">",
    "<a:schemeClr val=\"accent2\"/><a:schemeClr val=\"accent4\"/><a:schemeClr val=\"accent6\"/>",
    "<cs:variation><a:lumMod val=\"60000\"/></cs:variation><cs:variation><a:lumMod val=\"80000\"/><a:lumOff val=\"20000\"/></cs:variation>",
    "</cs:colorStyle>"
);

/// The chart part's own relationships to its style and colour parts.
const STYLE_RELS: &str = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.microsoft.com/office/2011/relationships/chartStyle" Target="style1.xml"/><Relationship Id="rId2" Type="http://schemas.microsoft.com/office/2011/relationships/chartColorStyle" Target="colors1.xml"/></Relationships>"#;

fn chart_fixture() -> Vec<u8> {
    chart_package(&unmodelled_chart_xml())
}

/// A package holding one sheet of data and one chart part.
fn chart_package(part: &str) -> Vec<u8> {
    chart_package_with(part, &[])
}

/// [`chart_package`] plus `extra` parts: a chart's own style and colour parts go behind its
/// relationships, as Excel writes them for a chart whose style was customised.
fn chart_package_with<'a>(part: &'a str, extra: &[(&'a str, &'a str)]) -> Vec<u8> {
    let drawing = r#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><xdr:twoCellAnchor><xdr:from><xdr:col>3</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>10</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>15</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><xdr:graphicFrame><xdr:nvGraphicFramePr><xdr:cNvPr id="2" name="Chart 1"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" r:id="rId1"/></a:graphicData></a:graphic></xdr:graphicFrame><xdr:clientData/></xdr:twoCellAnchor></xdr:wsDr>"#;
    let drels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="../charts/chart1.xml"/></Relationships>"#;
    let srels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing" Target="../drawings/drawing1.xml"/></Relationships>"#;
    let sheet = r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Name</t></is></c><c r="B1" t="inlineStr"><is><t>Sales</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>One</t></is></c><c r="B2"><v>1</v></c></row><row r="3"><c r="A3" t="inlineStr"><is><t>Two</t></is></c><c r="B3"><v>4</v></c></row><row r="4"><c r="A4" t="inlineStr"><is><t>Three</t></is></c><c r="B4"><v>9</v></c></row><row r="5"><c r="A5" t="inlineStr"><is><t>Four</t></is></c><c r="B5"><v>16</v></c></row></sheetData><drawing r:id="rId1"/>"#;
    let mut parts = vec![
        ("xl/worksheets/_rels/sheet1.xml.rels", srels),
        ("xl/drawings/drawing1.xml", drawing),
        ("xl/drawings/_rels/drawing1.xml.rels", drels),
        ("xl/charts/chart1.xml", part),
    ];
    parts.extend_from_slice(extra);
    super::minimal(sheet, &parts, "", "")
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
            font: None,
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
            marker_size: None,
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

// ---------------------------------------------------------------- gridlines and the legend

#[test]
fn gridlines_on_both_axes_and_the_legend_options_round_trip() {
    let mut wb = bar_workbook();
    let mut sheet = (*wb.sheets[0]).clone();
    {
        let c = &mut sheet.charts[0];
        // The value axis keeps the legacy flag as its major set; the category axis has its own pair.
        c.gridlines = true;
        c.gridlines_minor = true;
        c.x_gridlines = Gridlines { major: true, minor: true };
        c.legend_overlay = true;
        c.series[1].legend_deleted = true;
        c.series[3].legend_deleted = true;
    }
    wb.sheets = vec![Arc::new(sheet)];
    let bytes = write_xlsx(&wb).unwrap();
    let part = chart_part(&bytes, "xl/charts/chart1.xml");
    // CT_*Ax order: major, then minor, on the value axis (left) and the category axis (bottom).
    assert!(part.contains(r#"<c:axPos val="l"/><c:majorGridlines/><c:minorGridlines/><c:numFmt"#), "the value axis: {part}");
    assert!(part.contains(r#"<c:axPos val="b"/><c:majorGridlines/><c:minorGridlines/><c:numFmt"#), "the category axis: {part}");
    // CT_Legend order: position, one entry per deleted series, then the overlay flag.
    assert!(
        part.contains(
            r#"<c:legend><c:legendPos val="b"/><c:legendEntry><c:idx val="1"/><c:delete val="1"/></c:legendEntry><c:legendEntry><c:idx val="3"/><c:delete val="1"/></c:legendEntry><c:overlay val="1"/></c:legend>"#
        ),
        "the legend: {part}"
    );
    let (back, _rep) = read_xlsx(&bytes).unwrap();
    let c = &back.sheets[0].charts[0];
    assert!(c.gridlines, "the legacy flag is the value axis's major set");
    assert_eq!(c.y_gridlines(), Gridlines { major: true, minor: true });
    assert_eq!(c.x_gridlines, Gridlines { major: true, minor: true });
    assert!(c.legend_overlay);
    assert_eq!(c.series.iter().map(|s| s.legend_deleted).collect::<Vec<_>>(), vec![false, true, false, true]);
    // A second save writes the same part: nothing drifts on the way round.
    assert_eq!(chart_part(&write_xlsx(&back).unwrap(), "xl/charts/chart1.xml"), part);
}

#[test]
fn gridlines_and_a_deleted_legend_entry_are_read_from_excel_shaped_xml() {
    let xml = r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart><c:plotArea><c:barChart><c:barDir val="col"/><c:grouping val="clustered"/><c:ser><c:idx val="0"/><c:order val="0"/><c:val><c:numRef><c:f>Data!$B$2:$B$5</c:f></c:numRef></c:val></c:ser><c:ser><c:idx val="1"/><c:order val="1"/><c:val><c:numRef><c:f>Data!$C$2:$C$5</c:f></c:numRef></c:val></c:ser><c:axId val="1"/><c:axId val="2"/></c:barChart><c:catAx><c:axId val="1"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="b"/><c:majorGridlines/><c:minorGridlines/><c:crossAx val="2"/></c:catAx><c:valAx><c:axId val="2"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="l"/><c:majorGridlines><c:spPr><a:ln w="6350"><a:solidFill><a:srgbClr val="D9D9D9"/></a:solidFill></a:ln></c:spPr></c:majorGridlines><c:crossAx val="1"/></c:valAx></c:plotArea><c:legend><c:legendPos val="r"/><c:legendEntry><c:idx val="1"/><c:delete val="1"/></c:legendEntry><c:legendEntry><c:idx val="7"/><c:delete val="1"/></c:legendEntry><c:overlay val="1"/></c:legend></c:chart></c:chartSpace>"#;
    let root = crate::xml::parse(xml.as_bytes()).unwrap();
    let c = crate::chart::read_chart(&root).unwrap();
    assert!(c.gridlines, "the value axis's major gridlines are the legacy flag");
    assert!(!c.gridlines_minor, "the value axis has no minor element here");
    assert_eq!(c.x_gridlines, Gridlines { major: true, minor: true });
    assert!(c.legend_overlay);
    // The deletion names series 1. An index past the series (7) belongs to a trendline or to a
    // chart whose entries are not series at all: the model leaves it to the kept part.
    assert_eq!(c.series.iter().map(|s| s.legend_deleted).collect::<Vec<_>>(), vec![false, true]);
}

#[test]
fn a_pie_legends_entries_are_its_points_not_its_series() {
    // A pie chart's legend entry index is a slice, so reading it as a series index would hide the
    // whole series' entry: the deletion is left to the part instead.
    let xml = r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart><c:plotArea><c:pieChart><c:varyColors val="1"/><c:ser><c:idx val="0"/><c:order val="0"/><c:val><c:numRef><c:f>Data!$B$2:$B$5</c:f></c:numRef></c:val></c:ser></c:pieChart></c:plotArea><c:legend><c:legendPos val="r"/><c:legendEntry><c:idx val="0"/><c:delete val="1"/></c:legendEntry></c:legend></c:chart></c:chartSpace>"#;
    let root = crate::xml::parse(xml.as_bytes()).unwrap();
    let c = crate::chart::read_chart(&root).unwrap();
    assert_eq!(c.kind, ChartKind::Pie);
    assert!(c.series.iter().all(|s| !s.legend_deleted));
    assert!(!c.kind.legend_entries_are_series());
}

#[test]
fn gridline_and_legend_edits_keep_the_kept_parts_unknown_xml() {
    let (mut wb, _rep) = read_xlsx(&rich_fixture()).unwrap();
    {
        let sheet = Arc::make_mut(&mut wb.sheets[0]);
        let c = &mut sheet.charts[0];
        // As the file has it: the value axis has a formatted major gridline, the legend is overlaid
        // and series 1's entry is deleted.
        assert!(c.gridlines);
        assert!(!c.gridlines_minor, "the secondary axis's minor gridlines are not the primary one's");
        assert_eq!(c.x_gridlines, Gridlines::default());
        assert!(c.legend_overlay);
        assert!(c.series[1].legend_deleted);
        // The edit: gridlines on both axes and both sets, the legend back in its own band, the
        // deleted entry moved from series 1 to series 0.
        c.gridlines_minor = true;
        c.x_gridlines = Gridlines { major: true, minor: true };
        c.legend_overlay = false;
        c.series[1].legend_deleted = false;
        c.series[0].legend_deleted = true;
    }
    let back = write_xlsx(&wb).unwrap();
    let part = part_of(&back);
    for needle in SURVIVORS {
        assert!(
            part.contains(needle),
            "the edit dropped {needle}:
{part}"
        );
    }
    // The category axis here is a date axis: its gridlines go where the schema wants them, after
    // axPos and before the title, and its date units are untouched.
    let start = part.find("<c:dateAx>").unwrap_or_default();
    let end = part.find("</c:dateAx>").unwrap_or(start);
    let date_axis = part.get(start..end).unwrap_or_default();
    assert!(date_axis.contains("<c:majorGridlines/><c:minorGridlines/><c:title>"), "the date axis gets both sets: {date_axis}");
    assert!(date_axis.contains("<c:baseTimeUnit val=\"days\"/>"), "the date axis keeps its units: {date_axis}");
    // The primary value axis gains a minor element after its own formatted major one, which it keeps.
    assert!(part.contains("</a:ln></c:spPr></c:majorGridlines>"), "{part}");
    assert!(part.contains("<c:minorGridlines/><c:title>"), "{part}");
    assert_eq!(part.matches("<c:majorGridlines").count(), 2, "the value axis and the category axis: {part}");
    assert_eq!(part.matches("<c:minorGridlines").count(), 3, "both axes and the secondary axis's own: {part}");
    assert!(part.contains(r#"<a:srgbClr val="EEEEEE"/>"#), "the secondary axis keeps its formatted minor gridlines");
    // The legend: the layout stays, the overlay flag flips, the entries move.
    assert!(part.contains(r#"<c:legendEntry><c:idx val="0"/><c:delete val="1"/></c:legendEntry><c:layout/><c:overlay val="0"/>"#), "{part}");
    assert!(!part.contains(r#"<c:legendEntry><c:idx val="1"/><c:delete val="1"/></c:legendEntry>"#), "series 1's entry is back: {part}");
    // And back through the reader: the model says what the edit said.
    let (again, _rep) = read_xlsx(&back).unwrap();
    let c = &again.sheets[0].charts[0];
    assert_eq!(c.y_gridlines(), Gridlines { major: true, minor: true });
    assert_eq!(c.x_gridlines, Gridlines { major: true, minor: true });
    assert!(!c.legend_overlay);
    assert_eq!(c.series.iter().map(|s| s.legend_deleted).collect::<Vec<_>>(), vec![true, false]);
    // A second save of the reopened workbook changes nothing, nor does saving the edit twice.
    assert_eq!(part_of(&write_xlsx(&again).unwrap()), part);
    assert_eq!(part_of(&write_xlsx(&wb).unwrap()), part);
}

#[test]
fn a_deleted_legend_entry_of_a_regenerated_part_is_written_from_the_model() {
    let mut wb = bar_workbook();
    let mut sheet = (*wb.sheets[0]).clone();
    sheet.charts[0].series[2].legend_deleted = true;
    sheet.charts[0].legend_overlay = true;
    wb.sheets = vec![Arc::new(sheet)];
    let part = chart_part(&write_xlsx(&wb).unwrap(), "xl/charts/chart1.xml");
    assert!(
        part.contains(r#"<c:legendPos val="b"/><c:legendEntry><c:idx val="2"/><c:delete val="1"/></c:legendEntry><c:overlay val="1"/>"#),
        "{part}"
    );
    assert_eq!(part.matches("<c:legendEntry>").count(), 1);
}

// ---------------------------------------------------------------- chartex parts

/// A modelled chartex part (a histogram) carrying options GridCraft has no field for: a title fill,
/// a per-point label format, binning, display units, formatted gridlines and tick labels, and an
/// extension block. Everything here has to survive a patched edit byte for byte.
const CHARTEX_HISTOGRAM: &str = r##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cx:chartSpace xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:cx="http://schemas.microsoft.com/office/drawing/2014/chartex"><cx:chartData><cx:data id="0"><cx:numDim type="val"><cx:f>_xlchart.v1.0</cx:f></cx:numDim></cx:data></cx:chartData><cx:chart><cx:title pos="t" align="ctr" overlay="0"><cx:tx><cx:txData><cx:v>Bins</cx:v></cx:txData></cx:tx><cx:spPr><a:solidFill><a:schemeClr val="accent2"/></a:solidFill></cx:spPr></cx:title><cx:plotArea><cx:plotAreaRegion><cx:series layoutId="clusteredColumn" uniqueId="{00000000-0000-0000-0000-000000000001}"><cx:tx><cx:txData><cx:f>_xlchart.v1.1</cx:f><cx:v>Sales</cx:v></cx:txData></cx:tx><cx:spPr><a:solidFill><a:schemeClr val="accent1"/></a:solidFill></cx:spPr><cx:dataLabels pos="outEnd"><cx:visibility seriesName="0" categoryName="0" value="1"/><cx:dataLabel idx="2"><cx:visibility value="0"/><cx:spPr><a:ln w="9525"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:ln></cx:spPr></cx:dataLabel></cx:dataLabels><cx:dataId val="0"/><cx:layoutPr><cx:binning intervalClosed="r"/></cx:layoutPr></cx:series></cx:plotAreaRegion><cx:axis id="0"><cx:catScaling gapWidth="0"/><cx:tickLabels/></cx:axis><cx:axis id="1"><cx:valScaling min="0" max="50" majorUnit="10"/><cx:units><cx:builtInUnit val="thousands"/></cx:units><cx:majorGridlines><cx:spPr><a:ln w="6350"><a:solidFill><a:srgbClr val="D9D9D9"/></a:solidFill></a:ln></cx:spPr></cx:majorGridlines><cx:tickLabels><cx:spPr><a:ln><a:noFill/></a:ln></cx:spPr></cx:tickLabels></cx:axis></cx:plotArea><cx:legend pos="b" align="ctr" overlay="0"/></cx:chart><cx:spPr><a:solidFill><a:schemeClr val="bg1"/></a:solidFill></cx:spPr><cx:extLst><cx:ext uri="{AAAAAAAA-0000-0000-0000-000000000001}"><cx15:unknownThing xmlns:cx15="http://schemas.microsoft.com/office/drawing/2015/9/8/chartex" val="1"/></cx:ext></cx:extLst></cx:chartSpace>"##;

/// A modelled chartex part for `kind`, with the axes that kind has.
fn chartex_part(kind: ChartKind) -> String {
    let (layout, axes) = match kind {
        ChartKind::Histogram => (
            "clusteredColumn",
            r#"<cx:axis id="0"><cx:catScaling gapWidth="0"/><cx:tickLabels/></cx:axis><cx:axis id="1"><cx:valScaling/><cx:tickLabels/></cx:axis>"#,
        ),
        ChartKind::Waterfall => (
            "waterfall",
            r#"<cx:axis id="0"><cx:catScaling gapWidth="0.5"/><cx:tickLabels/></cx:axis><cx:axis id="1"><cx:valScaling/><cx:tickLabels/></cx:axis>"#,
        ),
        ChartKind::BoxWhisker => (
            "boxWhisker",
            r#"<cx:axis id="0"><cx:catScaling gapWidth="1"/><cx:tickLabels/></cx:axis><cx:axis id="1"><cx:valScaling/><cx:tickLabels/></cx:axis>"#,
        ),
        ChartKind::Funnel => ("funnel", r#"<cx:axis id="0"><cx:catScaling gapWidth="0.06"/><cx:tickLabels/></cx:axis>"#),
        ChartKind::Treemap => ("treemap", ""),
        _ => ("sunburst", ""),
    };
    let dims = if kind == ChartKind::Histogram {
        r#"<cx:numDim type="val"><cx:f>_xlchart.v1.0</cx:f></cx:numDim>"#
    } else {
        r#"<cx:strDim type="cat"><cx:f>_xlchart.v1.0</cx:f></cx:strDim><cx:numDim type="val"><cx:f>_xlchart.v1.2</cx:f></cx:numDim>"#
    };
    let layout_pr = match kind {
        ChartKind::Histogram => r#"<cx:layoutPr><cx:binning intervalClosed="r"/></cx:layoutPr>"#,
        ChartKind::BoxWhisker => {
            r#"<cx:layoutPr><cx:visibility meanLine="0" meanMarker="1" nonoutliers="0" outliers="1"/><cx:statistics quartileMethod="exclusive"/></cx:layoutPr>"#
        }
        ChartKind::Treemap => r#"<cx:layoutPr><cx:parentLabelLayout val="overlapping"/></cx:layoutPr>"#,
        _ => "",
    };
    format!(
        r##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cx:chartSpace xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:cx="http://schemas.microsoft.com/office/drawing/2014/chartex"><cx:chartData><cx:data id="0">{dims}</cx:data></cx:chartData><cx:chart><cx:title pos="t" align="ctr" overlay="0"><cx:tx><cx:txData><cx:v>Old</cx:v></cx:txData></cx:tx></cx:title><cx:plotArea><cx:plotAreaRegion><cx:series layoutId="{layout}" uniqueId="{{00000000-0000-0000-0000-000000000001}}"><cx:tx><cx:txData><cx:f>_xlchart.v1.1</cx:f><cx:v>Sales</cx:v></cx:txData></cx:tx><cx:dataId val="0"/>{layout_pr}</cx:series></cx:plotAreaRegion>{axes}</cx:plotArea></cx:chart></cx:chartSpace>"##
    )
}

/// A package with one sheet of data and one chartex part framed through `mc:AlternateContent`.
fn chartex_package(part: &str) -> Vec<u8> {
    let drawing = r#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><xdr:twoCellAnchor><xdr:from><xdr:col>3</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>10</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>15</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice xmlns:cx1="http://schemas.microsoft.com/office/drawing/2015/9/8/chartex" Requires="cx1"><xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="2" name="Chart 1"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm/><a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/drawing/2014/chartex"><cx:chart xmlns:cx="http://schemas.microsoft.com/office/drawing/2014/chartex" r:id="rId1"/></a:graphicData></a:graphic></xdr:graphicFrame></mc:Choice><mc:Fallback><xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="0" name=""/><xdr:cNvSpPr><a:spLocks noTextEdit="1"/></xdr:cNvSpPr></xdr:nvSpPr><xdr:spPr/><xdr:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>x</a:t></a:r></a:p></xdr:txBody></xdr:sp></mc:Fallback></mc:AlternateContent><xdr:clientData/></xdr:twoCellAnchor></xdr:wsDr>"#;
    let drels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.microsoft.com/office/2014/relationships/chartEx" Target="../charts/chartEx1.xml"/></Relationships>"#;
    let srels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing" Target="../drawings/drawing1.xml"/></Relationships>"#;
    let sheet = r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Name</t></is></c><c r="B1" t="inlineStr"><is><t>Sales</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>One</t></is></c><c r="B2"><v>1</v></c></row><row r="3"><c r="A3" t="inlineStr"><is><t>Two</t></is></c><c r="B3"><v>4</v></c></row><row r="4"><c r="A4" t="inlineStr"><is><t>Three</t></is></c><c r="B4"><v>9</v></c></row><row r="5"><c r="A5" t="inlineStr"><is><t>Four</t></is></c><c r="B5"><v>16</v></c></row></sheetData><drawing r:id="rId1"/>"#;
    let names = r#"<definedNames><definedName name="_xlchart.v1.0" hidden="1">Data!$B$2:$B$5</definedName><definedName name="_xlchart.v1.1" hidden="1">Data!$B$1</definedName><definedName name="_xlchart.v1.2" hidden="1">Data!$A$2:$A$5</definedName></definedNames>"#;
    super::minimal(
        sheet,
        &[
            ("xl/worksheets/_rels/sheet1.xml.rels", srels),
            ("xl/drawings/drawing1.xml", drawing),
            ("xl/drawings/_rels/drawing1.xml.rels", drels),
            ("xl/charts/chartEx1.xml", part),
            ("xl/charts/style1.xml", CUSTOM_STYLE),
            ("xl/charts/colors1.xml", CUSTOM_COLORS),
            (
                "xl/charts/_rels/chartEx1.xml.rels",
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.microsoft.com/office/2011/relationships/chartStyle" Target="style1.xml"/><Relationship Id="rId2" Type="http://schemas.microsoft.com/office/2011/relationships/chartColorStyle" Target="colors1.xml"/></Relationships>"#,
            ),
        ],
        "",
        names,
    )
}

fn chartex_part_of(bytes: &[u8]) -> String {
    chart_part(bytes, "xl/charts/chartEx1.xml")
}

#[test]
fn a_chartex_legend_overlay_is_read_and_patched_in_place() {
    let (mut wb, _rep) = read_xlsx(&chartex_package(CHARTEX_HISTOGRAM)).unwrap();
    {
        let sheet = Arc::make_mut(&mut wb.sheets[0]);
        let c = &mut sheet.charts[0];
        assert_eq!(c.legend, LegendPos::Bottom);
        assert!(!c.legend_overlay, "the file's legend says overlay=0");
        // The edit: the legend goes over the plot area.
        c.legend_overlay = true;
    }
    let back = write_xlsx(&wb).unwrap();
    let part = chartex_part_of(&back);
    assert!(part.contains(r#"<cx:legend pos="b" align="ctr" overlay="1"/>"#), "{part}");
    // Only the overlay attribute changed: the alignment and everything else is the file's.
    assert_eq!(
        part.replace("overlay=\"1\"", "overlay=\"0\""),
        CHARTEX_HISTOGRAM,
        "the patch touched more than the flag:
{part}"
    );
    let (again, _rep) = read_xlsx(&back).unwrap();
    assert!(again.sheets[0].charts[0].legend_overlay);
    // A second save is stable.
    assert_eq!(chartex_part_of(&write_xlsx(&again).unwrap()), part);
}
#[test]
fn a_chartex_chart_read_from_a_file_keeps_its_options_through_an_edit() {
    let bytes = chartex_package(CHARTEX_HISTOGRAM);
    let (mut wb, _rep) = read_xlsx(&bytes).unwrap();
    {
        let sheet = Arc::make_mut(&mut wb.sheets[0]);
        assert_eq!(sheet.charts.len(), 1, "a modelled chartex chart is read, not kept whole");
        let c = &mut sheet.charts[0];
        assert_eq!(c.kind, ChartKind::Histogram);
        assert_eq!(c.series[0].values, "Data!$B$2:$B$5", "the hidden name resolved to its reference");
        assert_eq!(c.y_axis.min, Some(0.0));
        assert_eq!(c.y_axis.max, Some(50.0));
        assert_eq!(c.y_axis.major_unit, Some(10.0));
        assert!(c.data_labels);
        assert_eq!(c.series[0].labels.as_ref().map(|l| l.position), Some(LabelPosition::OutsideEnd));
        // The edit: one value-axis option.
        c.y_axis.min = Some(8.0);
    }
    let back = write_xlsx(&wb).unwrap();
    let part = chartex_part_of(&back);
    // The element that carries the edit changed; every other byte is the file's.
    assert_eq!(part.replace("min=\"8\"", "min=\"0\""), CHARTEX_HISTOGRAM, "the patch touched more than the edited attribute:\n{part}");
    for needle in [
        "<cx:units><cx:builtInUnit val=\"thousands\"/></cx:units>",
        "<cx:binning intervalClosed=\"r\"/>",
        "<cx:dataLabel idx=\"2\">",
        "<a:srgbClr val=\"FF0000\"/>",
        "<a:schemeClr val=\"accent2\"/>",
        "<a:srgbClr val=\"D9D9D9\"/>",
        "<a:noFill/>",
        "unknownThing",
    ] {
        assert!(part.contains(needle), "the edit dropped `{needle}`:\n{part}");
    }
    // The hidden names the part refers to are defined again, and the chart reads back the same.
    let book = chart_part(&back, "xl/workbook.xml");
    assert!(book.contains(r#"_xlchart.v1.0" hidden="1">Data!$B$2:$B$5"#), "{book}");
    let (again, _rep) = read_xlsx(&back).unwrap();
    let c = &again.sheets[0].charts[0];
    assert_eq!(c.kind, ChartKind::Histogram);
    assert_eq!((c.y_axis.min, c.y_axis.max, c.y_axis.major_unit), (Some(8.0), Some(50.0), Some(10.0)));
    assert_eq!(c.series[0].values, "Data!$B$2:$B$5");
    assert_eq!(c.series[0].name.as_deref(), Some("Data!$B$1"));
    assert!(c.data_labels);
    // A second save is stable.
    assert_eq!(chartex_part_of(&write_xlsx(&again).unwrap()), part);
    // Saving the edited workbook again changes nothing either.
    assert_eq!(chartex_part_of(&write_xlsx(&wb).unwrap()), part);
}

#[test]
fn every_chartex_kind_patches_only_the_edited_element() {
    for kind in [ChartKind::Histogram, ChartKind::Waterfall, ChartKind::BoxWhisker, ChartKind::Funnel, ChartKind::Treemap, ChartKind::Sunburst] {
        let original = chartex_part(kind);
        let (mut wb, _rep) = read_xlsx(&chartex_package(&original)).unwrap();
        {
            let sheet = Arc::make_mut(&mut wb.sheets[0]);
            let c = &mut sheet.charts[0];
            assert_eq!(c.kind, kind, "{kind:?} did not read back");
            // The title is the one option every kind stores.
            c.title = Some("Edited".into());
        }
        let part = chartex_part_of(&write_xlsx(&wb).unwrap());
        assert_eq!(part.replace("<cx:v>Edited</cx:v>", "<cx:v>Old</cx:v>"), original, "{kind:?}: the patch touched more than the title:\n{part}");
        let (again, _rep) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
        assert_eq!(again.sheets[0].charts[0].title.as_deref(), Some("Edited"), "{kind:?}");
    }
}

#[test]
fn a_chartex_series_label_position_and_visibilities_patch_only_the_label_element() {
    let original = chartex_part(ChartKind::Waterfall);
    let (mut wb, _rep) = read_xlsx(&chartex_package(&original)).unwrap();
    {
        let sheet = Arc::make_mut(&mut wb.sheets[0]);
        let c = &mut sheet.charts[0];
        c.data_labels = true;
        c.series[0].labels = Some(LabelOptions { show_value: true, show_category: true, position: LabelPosition::Center, ..LabelOptions::default() });
    }
    let part = chartex_part_of(&write_xlsx(&wb).unwrap());
    assert!(part.contains(r#"<cx:dataLabels pos="ctr"><cx:visibility seriesName="0" categoryName="1" value="1"/></cx:dataLabels>"#), "{part}");
    // Everything else is the original's.
    let then = r##"<cx:series layoutId="waterfall" uniqueId="{00000000-0000-0000-0000-000000000001}"><cx:tx><cx:txData><cx:f>_xlchart.v1.1</cx:f><cx:v>Sales</cx:v></cx:txData></cx:tx><cx:dataLabels pos="ctr"><cx:visibility seriesName="0" categoryName="1" value="1"/></cx:dataLabels><cx:dataId val="0"/>"##;
    let before = r##"<cx:series layoutId="waterfall" uniqueId="{00000000-0000-0000-0000-000000000001}"><cx:tx><cx:txData><cx:f>_xlchart.v1.1</cx:f><cx:v>Sales</cx:v></cx:txData></cx:tx><cx:dataId val="0"/>"##;
    assert_eq!(part.replace(then, before), original, "{part}");
    // The labels come back from the saved file.
    let (again, _rep) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    let l = again.sheets[0].charts[0].series[0].labels.clone().expect("series labels");
    assert!(l.show_value && l.show_category && l.position == LabelPosition::Center);
}

#[test]
fn a_chartex_box_whisker_quartile_edit_patches_the_statistics_element() {
    let original = chartex_part(ChartKind::BoxWhisker);
    let (mut wb, _rep) = read_xlsx(&chartex_package(&original)).unwrap();
    {
        let sheet = Arc::make_mut(&mut wb.sheets[0]);
        assert_eq!(sheet.charts[0].series[0].quartiles, gridcraft_model::QuartileMethod::Exclusive);
        sheet.charts[0].series[0].quartiles = gridcraft_model::QuartileMethod::Inclusive;
    }
    let part = chartex_part_of(&write_xlsx(&wb).unwrap());
    assert_eq!(
        part.replace("quartileMethod=\"inclusive\"", "quartileMethod=\"exclusive\""),
        original,
        "the quartile edit touched more than the attribute:\n{part}"
    );
    let (again, _rep) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    assert_eq!(again.sheets[0].charts[0].series[0].quartiles, gridcraft_model::QuartileMethod::Inclusive);
}

// ---------------------------------------------------------------- per-point labels

#[test]
fn a_point_label_text_and_position_round_trip() {
    let mut c = chart(1, ChartKind::ColumnClustered);
    c.data_labels = true;
    c.labels = LabelOptions::values();
    c.series[0].point_labels = vec![
        PointLabel { index: 1, text: Some("Data!$C$2".into()), show_value: Some(false), position: LabelPosition::Top, ..PointLabel::default() },
        PointLabel { index: 2, text: Some("Peak".into()), show_category: Some(true), position: LabelPosition::InsideEnd, ..PointLabel::default() },
    ];
    let mut s = Sheet::new("Data");
    for (i, v) in [1.0, 2.0, 3.0, 4.0].iter().enumerate() {
        s.set_value(CellRef::new(i as u32 + 1, 0), gridcraft_core::Value::Number(*v));
        s.set_value(CellRef::new(i as u32 + 1, 1), gridcraft_core::Value::Number(*v * 2.0));
        s.set_value(CellRef::new(i as u32 + 1, 2), gridcraft_core::Value::Text("note".into()));
    }
    let wanted = c.series[0].point_labels.clone();
    s.charts.push(c);
    let wb = Workbook { sheets: vec![Arc::new(s)], ..Workbook::default() };
    let bytes = write_xlsx(&wb).unwrap();
    let part = chart_part(&bytes, "xl/charts/chart1.xml");
    // A reference goes through a strRef with its cache; literal text is one rich-text run.
    assert!(
        part.contains(
            "<c:tx><c:strRef><c:f>Data!$C$2</c:f><c:strCache><c:ptCount val=\"1\"/><c:pt idx=\"0\"><c:v>note</c:v></c:pt></c:strCache></c:strRef></c:tx>"
        ),
        "{part}"
    );
    assert!(part.contains("<a:t>Peak</a:t>"), "{part}");
    assert!(part.contains("<c:dLbl><c:idx val=\"1\"/><c:tx>"), "{part}");
    assert!(part.contains("<c:dLblPos val=\"t\"/>"), "{part}");
    assert!(part.contains("<c:showVal val=\"0\"/>"), "{part}");
    let (back, _rep) = read_xlsx(&bytes).unwrap();
    assert_eq!(back.sheets[0].charts[0].series[0].point_labels, wanted);
    // A second save writes the same part.
    assert_eq!(chart_part(&write_xlsx(&back).unwrap(), "xl/charts/chart1.xml"), part);
}

/// A chart part with two per-point labels (one rich text, one taken from a cell) beside the series
/// options and an unmodelled callout layout.
fn point_labels_part() -> String {
    r##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><c:chart><c:plotArea><c:barChart><c:barDir val="col"/><c:grouping val="clustered"/><c:varyColors val="0"/><c:ser><c:idx val="0"/><c:order val="0"/><c:dLbls><c:dLbl><c:idx val="1"/><c:layout><c:manualLayout><c:x val="0.3"/><c:y val="0.4"/></c:manualLayout></c:layout><c:tx><c:rich><a:bodyPr/><a:p><a:r><a:rPr lang="en-US" b="1"/><a:t>Peak</a:t></a:r></a:p></c:rich></c:tx><c:dLblPos val="t"/><c:showVal val="0"/></c:dLbl><c:dLbl><c:idx val="2"/><c:tx><c:strRef><c:f>Data!$D$2</c:f><c:strCache><c:ptCount val="1"/><c:pt idx="0"><c:v>From a cell</c:v></c:pt></c:strCache></c:strRef></c:tx><c:showCatName val="1"/></c:dLbl><c:dLbl><c:idx val="3"/><c:delete val="1"/></c:dLbl><c:showVal val="1"/></c:dLbls><c:cat><c:strRef><c:f>Data!$A$2:$A$5</c:f></c:strRef></c:cat><c:val><c:numRef><c:f>Data!$B$2:$B$5</c:f></c:numRef></c:val><c:extLst><c:ext uri="{44444444-4444-4444-4444-444444444444}"><c15:pointThing xmlns:c15="http://schemas.microsoft.com/office/drawing/2012/chart" val="1"/></c:ext></c:extLst></c:ser><c:dLbls><c:showVal val="1"/></c:dLbls><c:gapWidth val="150"/><c:axId val="1"/><c:axId val="2"/></c:barChart><c:catAx><c:axId val="1"/><c:crossAx val="2"/></c:catAx><c:valAx><c:axId val="2"/><c:crossAx val="1"/></c:valAx></c:plotArea></c:chart></c:chartSpace>"##
        .to_string()
}

#[test]
fn per_point_labels_in_a_file_survive_and_patch() {
    let original = point_labels_part();
    let (mut wb, _rep) = read_xlsx(&chart_package(&original)).unwrap();
    {
        let sheet = Arc::make_mut(&mut wb.sheets[0]);
        let c = &mut sheet.charts[0];
        let p = &c.series[0].point_labels;
        assert_eq!(p.len(), 2, "{p:?}");
        assert_eq!(p[0].index, 1);
        assert_eq!(p[0].text.as_deref(), Some("Peak"), "rich text concatenated");
        assert_eq!(p[0].position, LabelPosition::Top);
        assert_eq!(p[0].show_value, Some(false));
        assert_eq!(p[1].index, 2);
        assert_eq!(p[1].text.as_deref(), Some("Data!$D$2"), "a cell reference is kept as one");
        assert_eq!(p[1].show_category, Some(true));
        // A deleted label stays in the point formats.
        assert!(c.series[0].points.iter().any(|x| x.index == 3 && x.no_label));
    }
    // An unrelated edit patches the axis and leaves the labels (and their layout) alone.
    {
        let sheet = Arc::make_mut(&mut wb.sheets[0]);
        sheet.charts[0].y_axis.min = Some(2.0);
    }
    let part = chart_part(&write_xlsx(&wb).unwrap(), "xl/charts/chart1.xml");
    for needle in ["<a:t>Peak</a:t>", "<c:manualLayout>", "<c:f>Data!$D$2</c:f>", "pointThing", "<c:delete val=\"1\"/>"] {
        assert!(part.contains(needle), "the edit dropped `{needle}`:\n{part}");
    }
    // Changing one point's text rewrites that `dLbl` and keeps the other point's.
    let (mut again, _rep) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    {
        let sheet = Arc::make_mut(&mut again.sheets[0]);
        let c = &mut sheet.charts[0];
        let mut l = c.series[0].point_labels[0].clone();
        l.text = Some("Renamed".into());
        c.series[0].set_point_label(l);
    }
    let part = chart_part(&write_xlsx(&again).unwrap(), "xl/charts/chart1.xml");
    assert!(part.contains("<a:t>Renamed</a:t>"), "{part}");
    assert!(part.contains("<c:f>Data!$D$2</c:f>"), "the other point's label stays: {part}");
    assert!(part.contains("<c:manualLayout>"), "the callout layout stays: {part}");
    let (back, _rep) = read_xlsx(&write_xlsx(&again).unwrap()).unwrap();
    assert_eq!(back.sheets[0].charts[0].series[0].point_labels[0].text.as_deref(), Some("Renamed"));
}

// ---------------------------------------------------------------- custom error bars from a file

/// A chart part whose series carries custom error bars (a reference on each side) with their own
/// line formatting.
fn custom_error_bars_part() -> String {
    r##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><c:chart><c:plotArea><c:barChart><c:barDir val="col"/><c:grouping val="clustered"/><c:varyColors val="0"/><c:ser><c:idx val="0"/><c:order val="0"/><c:errBars><c:errBarType val="both"/><c:errValType val="cust"/><c:noEndCap val="0"/><c:plus><c:numRef><c:f>Data!$C$2:$C$5</c:f><c:numCache><c:ptCount val="4"/></c:numCache></c:numRef></c:plus><c:minus><c:numRef><c:f>Data!$D$2:$D$5</c:f><c:numCache><c:ptCount val="4"/></c:numCache></c:numRef></c:minus><c:spPr><a:ln w="9525"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:ln></c:spPr></c:errBars><c:cat><c:strRef><c:f>Data!$A$2:$A$5</c:f></c:strRef></c:cat><c:val><c:numRef><c:f>Data!$B$2:$B$5</c:f></c:numRef></c:val></c:ser><c:gapWidth val="150"/><c:axId val="1"/><c:axId val="2"/></c:barChart><c:catAx><c:axId val="1"/><c:crossAx val="2"/></c:catAx><c:valAx><c:axId val="2"/><c:crossAx val="1"/></c:valAx></c:plotArea></c:chart></c:chartSpace>"##
        .to_string()
}

#[test]
fn custom_error_bars_read_from_a_file_survive_an_unrelated_edit() {
    let original = custom_error_bars_part();
    let (mut wb, _rep) = read_xlsx(&chart_package(&original)).unwrap();
    {
        let sheet = Arc::make_mut(&mut wb.sheets[0]);
        let e = sheet.charts[0].series[0].error_bars.clone().expect("error bars");
        assert_eq!(
            e.kind,
            ErrorBarKind::Custom { plus: Some("Data!$C$2:$C$5".into()), minus: Some("Data!$D$2:$D$5".into()) },
            "the custom references are read"
        );
        sheet.charts[0].y_axis.min = Some(1.0);
    }
    let part = chart_part(&write_xlsx(&wb).unwrap(), "xl/charts/chart1.xml");
    assert!(part.contains("<c:f>Data!$C$2:$C$5</c:f>"), "{part}");
    assert!(part.contains("<c:f>Data!$D$2:$D$5</c:f>"), "{part}");
    assert!(part.contains("<a:srgbClr val=\"FF0000\"/>"), "the bar's line style stays: {part}");
    // The references survive a save of a model written from scratch too.
    let (back, _rep) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    let e = back.sheets[0].charts[0].series[0].error_bars.clone().expect("error bars");
    assert_eq!(e.kind, ErrorBarKind::Custom { plus: Some("Data!$C$2:$C$5".into()), minus: Some("Data!$D$2:$D$5".into()) });
    // Changing the bars themselves keeps both references.
    let mut from_scratch = (*back.sheets[0]).clone();
    from_scratch.charts[0].series[0].error_bars = Some(ErrorBars {
        direction: ErrorBarDirection::Minus,
        kind: ErrorBarKind::Custom { plus: Some("Data!$C$2:$C$5".into()), minus: Some("Data!$D$2:$D$5".into()) },
        no_end_cap: true,
    });
    let wb2 = Workbook { sheets: vec![Arc::new(from_scratch)], ..Workbook::default() };
    let part2 = chart_part(&write_xlsx(&wb2).unwrap(), "xl/charts/chart1.xml");
    assert!(part2.contains("<c:f>Data!$C$2:$C$5</c:f>") && part2.contains("<c:f>Data!$D$2:$D$5</c:f>"), "{part2}");
}

#[test]
fn a_chartex_series_colour_edit_patches_the_series_fill() {
    let original = chartex_part(ChartKind::Histogram);
    let (mut wb, _rep) = read_xlsx(&chartex_package(&original)).unwrap();
    {
        let sheet = Arc::make_mut(&mut wb.sheets[0]);
        sheet.charts[0].series[0].color = Some(Color::Rgb(0xC00000));
    }
    let part = chartex_part_of(&write_xlsx(&wb).unwrap());
    // The series had no fill of its own: the patch adds one where the schema puts it.
    assert_eq!(
        part.replace("<cx:spPr><a:solidFill><a:srgbClr val=\"C00000\"/></a:solidFill></cx:spPr>", ""),
        original,
        "the colour edit touched more than the series fill:\n{part}"
    );
    let (again, _rep) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    assert_eq!(again.sheets[0].charts[0].series[0].color, Some(Color::Rgb(0xC00000)));
}

#[test]
fn an_untouched_chartex_part_is_written_back_byte_identical() {
    let (wb, _rep) = read_xlsx(&chartex_package(CHARTEX_HISTOGRAM)).unwrap();
    let back = write_xlsx(&wb).unwrap();
    assert_eq!(chartex_part_of(&back), CHARTEX_HISTOGRAM);
    // Its hidden names are defined again, at the indices the part uses.
    let book = chart_part(&back, "xl/workbook.xml");
    for (name, reference) in [("_xlchart.v1.0", "Data!$B$2:$B$5"), ("_xlchart.v1.1", "Data!$B$1")] {
        assert!(book.contains(&format!("name=\"{name}\" hidden=\"1\">{reference}</definedName>")), "{name}: {book}");
    }
    // And a second save changes nothing.
    let (again, _rep) = read_xlsx(&back).unwrap();
    assert_eq!(chartex_part_of(&write_xlsx(&again).unwrap()), CHARTEX_HISTOGRAM);
}

#[test]
fn a_chartex_data_change_regenerates_the_part_with_readable_names() {
    let (mut wb, _rep) = read_xlsx(&chartex_package(CHARTEX_HISTOGRAM)).unwrap();
    {
        let sheet = Arc::make_mut(&mut wb.sheets[0]);
        sheet.charts[0].series[0].values = "Data!$B$2:$B$3".into();
    }
    let back = write_xlsx(&wb).unwrap();
    let part = chartex_part_of(&back);
    // The data reference moved, which a patched part cannot express: the part is regenerated, with
    // its data behind hidden names and no stale reference left.
    assert!(part.contains("<cx:f>_xlchart.v1.0</cx:f>"), "{part}");
    assert!(!part.contains("Data!"), "{part}");
    let book = chart_part(&back, "xl/workbook.xml");
    assert!(book.contains(">Data!$B$2:$B$3</definedName>"), "{book}");
    let (again, _rep) = read_xlsx(&back).unwrap();
    assert_eq!(again.sheets[0].charts[0].series[0].values, "Data!$B$2:$B$3");
}

/// A package with two chartex parts, the second numbered from 5 (as a workbook whose earlier
/// chartex parts were deleted would be).
fn chartex_pair_package(a: &str, b: &str) -> Vec<u8> {
    let frame = |rid: &str, id: u32| {
        format!(
            r#"<xdr:twoCellAnchor><xdr:from><xdr:col>3</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{id}</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>10</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{}</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice xmlns:cx1="http://schemas.microsoft.com/office/drawing/2015/9/8/chartex" Requires="cx1"><xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="{id}" name="Chart {id}"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm/><a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/drawing/2014/chartex"><cx:chart xmlns:cx="http://schemas.microsoft.com/office/drawing/2014/chartex" r:id="{rid}"/></a:graphicData></a:graphic></xdr:graphicFrame></mc:Choice><mc:Fallback><xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="0" name=""/><xdr:cNvSpPr><a:spLocks noTextEdit="1"/></xdr:cNvSpPr></xdr:nvSpPr><xdr:spPr/><xdr:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>x</a:t></a:r></a:p></xdr:txBody></xdr:sp></mc:Fallback></mc:AlternateContent><xdr:clientData/></xdr:twoCellAnchor>"#,
            id + 10
        )
    };
    let drawing = format!(
        r#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">{}{}</xdr:wsDr>"#,
        frame("rId1", 1),
        frame("rId2", 20)
    );
    let drels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.microsoft.com/office/2014/relationships/chartEx" Target="../charts/chartEx1.xml"/><Relationship Id="rId2" Type="http://schemas.microsoft.com/office/2014/relationships/chartEx" Target="../charts/chartEx2.xml"/></Relationships>"#;
    let srels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing" Target="../drawings/drawing1.xml"/></Relationships>"#;
    let sheet = r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Name</t></is></c><c r="B1" t="inlineStr"><is><t>Sales</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>One</t></is></c><c r="B2"><v>1</v></c></row><row r="3"><c r="A3" t="inlineStr"><is><t>Two</t></is></c><c r="B3"><v>4</v></c></row><row r="4"><c r="A4" t="inlineStr"><is><t>Three</t></is></c><c r="B4"><v>9</v></c></row><row r="5"><c r="A5" t="inlineStr"><is><t>Four</t></is></c><c r="B5"><v>16</v></c></row></sheetData><drawing r:id="rId1"/>"#;
    let names = r#"<definedNames><definedName name="_xlchart.v1.0" hidden="1">Data!$B$2:$B$5</definedName><definedName name="_xlchart.v1.1" hidden="1">Data!$B$1</definedName><definedName name="_xlchart.v1.5" hidden="1">Data!$A$2:$A$5</definedName><definedName name="_xlchart.v1.6" hidden="1">Data!$B$1</definedName><definedName name="_xlchart.v1.7" hidden="1">Data!$B$2:$B$5</definedName></definedNames>"#;
    super::minimal(
        sheet,
        &[
            ("xl/worksheets/_rels/sheet1.xml.rels", srels),
            ("xl/drawings/drawing1.xml", &drawing),
            ("xl/drawings/_rels/drawing1.xml.rels", drels),
            ("xl/charts/chartEx1.xml", a),
            ("xl/charts/chartEx2.xml", b),
        ],
        "",
        names,
    )
}

#[test]
fn two_chartex_parts_keep_their_own_names_at_their_own_indices() {
    // The second part numbers its hidden names 5, 7 and 6: the writer fills the gaps and keeps the
    // indices, and neither part disturbs the other's data.
    let second = r##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cx:chartSpace xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:cx="http://schemas.microsoft.com/office/drawing/2014/chartex"><cx:chartData><cx:data id="0"><cx:strDim type="cat"><cx:f>_xlchart.v1.5</cx:f></cx:strDim><cx:numDim type="val"><cx:f>_xlchart.v1.7</cx:f></cx:numDim></cx:data></cx:chartData><cx:chart><cx:title pos="t" align="ctr" overlay="0"><cx:tx><cx:txData><cx:v>Second</cx:v></cx:txData></cx:tx></cx:title><cx:plotArea><cx:plotAreaRegion><cx:series layoutId="waterfall" uniqueId="{00000000-0000-0000-0000-000000000002}"><cx:tx><cx:txData><cx:f>_xlchart.v1.6</cx:f><cx:v>Sales</cx:v></cx:txData></cx:tx><cx:dataId val="0"/></cx:series></cx:plotAreaRegion><cx:axis id="0"><cx:catScaling gapWidth="0.5"/><cx:tickLabels/></cx:axis><cx:axis id="1"><cx:valScaling/><cx:tickLabels/></cx:axis></cx:plotArea></cx:chart></cx:chartSpace>"##.to_string();
    let (mut wb, _rep) = read_xlsx(&chartex_pair_package(&chartex_part(ChartKind::Histogram), &second)).unwrap();
    {
        let sheet = Arc::make_mut(&mut wb.sheets[0]);
        assert_eq!(sheet.charts.len(), 2);
        assert_eq!(sheet.charts[1].series[0].values, "Data!$B$2:$B$5");
        assert_eq!(sheet.charts[1].series[0].categories.as_deref(), Some("Data!$A$2:$A$5"));
        // Edit both: one axis option on each.
        sheet.charts[0].y_axis.max = Some(40.0);
        sheet.charts[1].y_axis.min = Some(1.0);
    }
    let back = write_xlsx(&wb).unwrap();
    let a = chartex_part_of(&back);
    assert!(a.contains("<cx:valScaling max=\"40\"/>"), "{a}");
    // The first part's names stay 0 and 1.
    assert!(a.contains("<cx:f>_xlchart.v1.0</cx:f>") && a.contains("<cx:f>_xlchart.v1.1</cx:f>"), "{a}");
    let b = chart_part(&back, "xl/charts/chartEx2.xml");
    // The second part keeps the indices it had (5, 7 and 6), all defined again.
    for name in ["_xlchart.v1.5", "_xlchart.v1.6", "_xlchart.v1.7"] {
        assert!(b.contains(&format!("<cx:f>{name}</cx:f>")), "{name} lost from the second part:\n{b}");
    }
    let book = chart_part(&back, "xl/workbook.xml");
    for (i, reference) in [(5, "Data!$A$2:$A$5"), (6, "Data!$B$1"), (7, "Data!$B$2:$B$5")] {
        assert!(
            book.contains(&format!("name=\"_xlchart.v1.{i}\" hidden=\"1\">{reference}</definedName>")),
            "name {i} is not defined as {reference}:\n{book}"
        );
    }
    let (again, _rep) = read_xlsx(&back).unwrap();
    assert_eq!(again.sheets[0].charts[0].y_axis.max, Some(40.0));
    assert_eq!(again.sheets[0].charts[1].y_axis.min, Some(1.0));
    assert_eq!(again.sheets[0].charts[1].series[0].values, "Data!$B$2:$B$5");
    assert_eq!(chart_part(&write_xlsx(&again).unwrap(), "xl/charts/chartEx2.xml"), b);
}

// ---------------------------------------------------------------- the chart's own style parts

/// How many `[Content_Types].xml` overrides the package declares for `part`: one, or none when the
/// part is not written.
fn content_type_count(types: &str, part: &str) -> usize {
    types.matches(&format!("PartName=\"/{part}\"")).count()
}

/// A chart's own style and colour parts as saved.
fn style_parts(output: &[u8], style: &str, colors: &str) -> (String, String) {
    (chart_part(output, style), chart_part(output, colors))
}

#[test]
fn a_classic_chart_keeps_its_own_style_and_colour_parts_byte_for_byte() {
    let bytes = chart_package_with(
        &unmodelled_chart_xml(),
        &[("xl/charts/_rels/chart1.xml.rels", STYLE_RELS), ("xl/charts/style1.xml", CUSTOM_STYLE), ("xl/charts/colors1.xml", CUSTOM_COLORS)],
    );
    let (wb, _rep) = read_xlsx(&bytes).unwrap();
    let back = write_xlsx(&wb).unwrap();
    // Neither the part nor its look changed, byte for byte.
    assert_eq!(part_of(&back), unmodelled_chart_xml());
    let (style, colors) = style_parts(&back, "xl/charts/style1.xml", "xl/charts/colors1.xml");
    assert_eq!((style.as_str(), colors.as_str()), (CUSTOM_STYLE, CUSTOM_COLORS));
    // The package is consistent: the chart part owns the two parts, and each is typed once.
    crate::testing::check_package(&back).expect("the package with style parts is valid");
    let rels = chart_part(&back, "xl/charts/_rels/chart1.xml.rels");
    assert!(rels.contains("relationships/chartStyle") && rels.contains(r#"Target="style1.xml""#), "{rels}");
    assert!(rels.contains("relationships/chartColorStyle") && rels.contains(r#"Target="colors1.xml""#), "{rels}");
    let types = chart_part(&back, "[Content_Types].xml");
    assert_eq!(content_type_count(&types, "xl/charts/style1.xml"), 1, "{types}");
    assert_eq!(content_type_count(&types, "xl/charts/colors1.xml"), 1, "{types}");
    // A second save is stable.
    let (again, _rep) = read_xlsx(&back).unwrap();
    let twice = write_xlsx(&again).unwrap();
    assert_eq!(part_of(&twice), unmodelled_chart_xml());
    assert_eq!(style_parts(&twice, "xl/charts/style1.xml", "xl/charts/colors1.xml"), (style, colors));
}

#[test]
fn a_classic_edit_keeps_the_style_and_colour_parts() {
    let bytes = chart_package_with(
        &unmodelled_chart_xml(),
        &[("xl/charts/_rels/chart1.xml.rels", STYLE_RELS), ("xl/charts/style1.xml", CUSTOM_STYLE), ("xl/charts/colors1.xml", CUSTOM_COLORS)],
    );
    let (mut wb, _rep) = read_xlsx(&bytes).unwrap();
    {
        let sheet = Arc::make_mut(&mut wb.sheets[0]);
        sheet.charts[0].y_axis.min = Some(5.0);
    }
    let back = write_xlsx(&wb).unwrap();
    // The value axis' scaling changed and nothing else: a style part does not depend on the data.
    let then = r#"<c:scaling><c:orientation val="minMax"/></c:scaling>"#;
    let now = r#"<c:scaling><c:orientation val="minMax"/><c:min val="5"/></c:scaling>"#;
    let part = part_of(&back);
    assert_eq!(part.matches(now).count(), 1, "{part}");
    assert_eq!(part.replace(now, then), unmodelled_chart_xml(), "the edit touched more than the axis:\n{part}");
    assert_eq!(style_parts(&back, "xl/charts/style1.xml", "xl/charts/colors1.xml"), (CUSTOM_STYLE.into(), CUSTOM_COLORS.into()));
    crate::testing::check_package(&back).expect("the package with style parts is valid");
}

#[test]
fn a_classic_kind_change_drops_the_style_and_colour_parts() {
    let bytes = chart_package_with(
        &unmodelled_chart_xml(),
        &[("xl/charts/_rels/chart1.xml.rels", STYLE_RELS), ("xl/charts/style1.xml", CUSTOM_STYLE), ("xl/charts/colors1.xml", CUSTOM_COLORS)],
    );
    let (mut wb, _rep) = read_xlsx(&bytes).unwrap();
    {
        let sheet = Arc::make_mut(&mut wb.sheets[0]);
        sheet.charts[0].kind = ChartKind::BarClustered;
    }
    let back = write_xlsx(&wb).unwrap();
    // The part had to be regenerated from the model, so the file's look no longer applies: it is
    // dropped rather than left behind as an orphan.
    let part = part_of(&back);
    assert!(!part.contains("unknownChartThing") && !part.contains("printSettings"), "the part was patched, not regenerated:\n{part}");
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(back.clone())).unwrap();
    for name in ["xl/charts/style1.xml", "xl/charts/colors1.xml", "xl/charts/_rels/chart1.xml.rels"] {
        assert!(z.by_name(name).is_err(), "{name} outlived the part it styled");
    }
    // A default applies instead, and the package stays consistent.
    let types = chart_part(&back, "[Content_Types].xml");
    assert_eq!(content_type_count(&types, "xl/charts/style1.xml"), 0, "{types}");
    crate::testing::check_package(&back).expect("the regenerated package is valid");
    let (again, _rep) = read_xlsx(&back).unwrap();
    assert_eq!(again.sheets[0].charts[0].kind, ChartKind::BarClustered);
}

#[test]
fn a_chartex_chart_keeps_its_own_style_and_colour_parts_byte_for_byte() {
    let (wb, _rep) = read_xlsx(&chartex_package(CHARTEX_HISTOGRAM)).unwrap();
    let back = write_xlsx(&wb).unwrap();
    assert_eq!(chartex_part_of(&back), CHARTEX_HISTOGRAM);
    let (style, colors) = style_parts(&back, "xl/charts/style1.xml", "xl/charts/colors1.xml");
    assert_eq!((style.as_str(), colors.as_str()), (CUSTOM_STYLE, CUSTOM_COLORS), "the chart's look was replaced by a default");
    crate::testing::check_package(&back).expect("the package with style parts is valid");
    let rels = chart_part(&back, "xl/charts/_rels/chartEx1.xml.rels");
    assert!(rels.contains("relationships/chartStyle") && rels.contains(r#"Target="style1.xml""#), "{rels}");
    assert!(rels.contains("relationships/chartColorStyle") && rels.contains(r#"Target="colors1.xml""#), "{rels}");
    // A second save is stable.
    let (again, _rep) = read_xlsx(&back).unwrap();
    assert_eq!(style_parts(&write_xlsx(&again).unwrap(), "xl/charts/style1.xml", "xl/charts/colors1.xml"), (style, colors));
}

#[test]
fn a_chartex_title_edit_keeps_the_style_and_colour_parts() {
    let original = chartex_part(ChartKind::Waterfall);
    let (mut wb, _rep) = read_xlsx(&chartex_package(&original)).unwrap();
    {
        let sheet = Arc::make_mut(&mut wb.sheets[0]);
        sheet.charts[0].title = Some("Edited".into());
    }
    let back = write_xlsx(&wb).unwrap();
    let part = chartex_part_of(&back);
    assert_eq!(part.replace("<cx:v>Edited</cx:v>", "<cx:v>Old</cx:v>"), original, "the patch touched more than the title:\n{part}");
    assert_eq!(style_parts(&back, "xl/charts/style1.xml", "xl/charts/colors1.xml"), (CUSTOM_STYLE.into(), CUSTOM_COLORS.into()));
    crate::testing::check_package(&back).expect("the package with style parts is valid");
    // The look survives reopening, byte for byte.
    let (again, _rep) = read_xlsx(&back).unwrap();
    let twice = write_xlsx(&again).unwrap();
    assert_eq!(style_parts(&twice, "xl/charts/style1.xml", "xl/charts/colors1.xml"), (CUSTOM_STYLE.into(), CUSTOM_COLORS.into()));
}

#[test]
fn a_chartex_kind_change_drops_the_custom_style_and_colour_parts() {
    let (mut wb, _rep) = read_xlsx(&chartex_package(&chartex_part(ChartKind::Waterfall))).unwrap();
    {
        let sheet = Arc::make_mut(&mut wb.sheets[0]);
        sheet.charts[0].kind = ChartKind::Histogram;
    }
    let back = write_xlsx(&wb).unwrap();
    // The part is regenerated from the model: the file's style described a waterfall, and keeping
    // it would style the histogram with the wrong chart's look. The defaults apply instead.
    let part = chartex_part_of(&back);
    assert!(part.contains(r#"<cx:series layoutId="clusteredColumn""#), "{part}");
    assert_eq!(style_parts(&back, "xl/charts/style1.xml", "xl/charts/colors1.xml"), (crate::chartex::style_xml(), crate::chartex::colors_xml()));
    // Still a chartex part: it gets its style and colour parts, and the package is consistent.
    let rels = chart_part(&back, "xl/charts/_rels/chartEx1.xml.rels");
    assert!(rels.contains("chartStyle") && rels.contains("chartColorStyle"), "{rels}");
    let types = chart_part(&back, "[Content_Types].xml");
    assert_eq!(content_type_count(&types, "xl/charts/style1.xml"), 1, "{types}");
    crate::testing::check_package(&back).expect("the regenerated package is valid");
    let (again, _rep) = read_xlsx(&back).unwrap();
    assert_eq!(again.sheets[0].charts[0].kind, ChartKind::Histogram);
}

#[test]
fn a_charts_alt_text_round_trips_and_is_escaped() {
    let mut wb = bar_workbook();
    let mut sheet = (*wb.sheets[0]).clone();
    // A quote, an angle bracket and an ampersand live in an XML attribute: they must be escaped
    // and come back exactly as written.
    let tricky = "Sales by \"region\" <chart> & totals";
    sheet.charts[0].alt = Some(tricky.into());
    sheet.shapes.push(gridcraft_model::Shape {
        id: 7,
        kind: gridcraft_model::ShapeKind::Rectangle,
        anchor: Anchor::default(),
        fill: Color::default(),
        line: Color::default(),
        text: String::new(),
        alt: Some("Blue status box".into()),
        format: Default::default(),
    });
    wb.sheets = vec![Arc::new(sheet)];
    let bytes = write_xlsx(&wb).unwrap();
    let part = chart_part(&bytes, "xl/drawings/drawing1.xml");
    assert!(part.contains("descr=\"Sales by &quot;region&quot; &lt;chart&gt; &amp; totals\""), "{part}");
    assert!(part.contains("descr=\"Blue status box\""), "{part}");
    let (back, _) = read_xlsx(&bytes).unwrap();
    let sh = back.sheets[0].as_ref();
    assert_eq!(sh.charts[0].alt.as_deref(), Some(tricky));
    assert_eq!(sh.shapes.first().and_then(|s| s.alt.as_deref()), Some("Blue status box"));
    // A second save writes the same drawing: nothing drifts on the way round.
    let again = write_xlsx(&back).unwrap();
    assert_eq!(chart_part(&again, "xl/drawings/drawing1.xml"), part);
}
