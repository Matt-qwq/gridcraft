//! Charts of kinds GridCraft cannot model (3-D, surface, pie of pie, Pareto) keep their drawing
//! frame and their part: saving a workbook never silently loses them.

use std::io::Read;

use crate::read_xlsx;
use crate::write_xlsx;

const SURFACE_PART: &str = r##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><c:roundedCorners val="0"/><c:chart><c:plotArea><c:surfaceChart><c:ser><c:idx val="0"/><c:order val="0"/><c:val><c:numRef><c:f>Data!$B$2:$B$5</c:f></c:numRef></c:val></c:ser><c:axId val="1"/><c:axId val="2"/><c:axId val="3"/></c:surfaceChart><c:catAx><c:axId val="1"/><c:axPos val="b"/></c:catAx><c:valAx><c:axId val="2"/><c:axPos val="l"/></c:valAx><c:serAx><c:axId val="3"/><c:axPos val="b"/></c:serAx></c:plotArea></c:chart><c:extLst><c:ext uri="{AAAA0000-0000-0000-0000-000000000001}"><c15:surfaceThing xmlns:c15="http://schemas.microsoft.com/office/drawing/2012/chart" val="1"/></c:ext></c:extLst></c:chartSpace>
"##;

const PIE3D_PART: &str = r##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><c:roundedCorners val="0"/><c:chart><c:view3D><c:rotX val="30"/><c:rotY val="50"/><c:rAngAx val="0"/></c:view3D><c:plotArea><c:pie3DChart><c:varyColors val="1"/><c:ser><c:idx val="0"/><c:order val="0"/><c:cat><c:strRef><c:f>Data!$A$2:$A$5</c:f></c:strRef></c:cat><c:val><c:numRef><c:f>Data!$B$2:$B$5</c:f></c:numRef></c:val></c:ser></c:pie3DChart></c:plotArea></c:chart></c:chartSpace>
"##;

const PARETO_PART: &str = r##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cx:chartSpace xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:cx="http://schemas.microsoft.com/office/drawing/2014/chartex"><cx:chartData><cx:data id="0"><cx:numDim type="val"><cx:f>_xlchart.v1.0</cx:f></cx:numDim></cx:data><cx:data id="1"><cx:numDim type="val"><cx:f>_xlchart.v1.1</cx:f></cx:numDim></cx:data></cx:chartData><cx:chart><cx:plotArea><cx:plotAreaRegion><cx:series layoutId="clusteredColumn" uniqueId="{00000000-0000-0000-0000-000000000000}"><cx:dataId val="0"/></cx:series><cx:series layoutId="paretoLine" uniqueId="{00000000-0000-0000-0000-000000000001}"><cx:dataId val="1"/></cx:series></cx:plotAreaRegion></cx:plotArea></cx:chart></cx:chartSpace>
"##;

fn frame(rid: &str, row: i32) -> String {
    format!(
        r#"<xdr:twoCellAnchor><xdr:from><xdr:col>3</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{row}</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>10</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{}</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><xdr:graphicFrame><xdr:nvGraphicFramePr><xdr:cNvPr id="{row}" name="Chart {row}"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" r:id="{rid}"/></a:graphicData></a:graphic></xdr:graphicFrame><xdr:clientData/></xdr:twoCellAnchor>"#,
        row + 10
    )
}

fn chartex_frame(rid: &str, row: i32) -> String {
    format!(
        r#"<xdr:twoCellAnchor><xdr:from><xdr:col>3</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{row}</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>10</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{}</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice xmlns:cx1="http://schemas.microsoft.com/office/drawing/2015/9/8/chartex" Requires="cx1"><xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="{row}" name="Chart {row}"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm/><a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/drawing/2014/chartex"><cx:chart xmlns:cx="http://schemas.microsoft.com/office/drawing/2014/chartex" r:id="{rid}"/></a:graphicData></a:graphic></xdr:graphicFrame></mc:Choice><mc:Fallback><xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="0" name=""/><xdr:cNvSpPr><a:spLocks noTextEdit="1"/></xdr:cNvSpPr></xdr:nvSpPr><xdr:spPr/><xdr:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>x</a:t></a:r></a:p></xdr:txBody></xdr:sp></mc:Fallback></mc:AlternateContent><xdr:clientData/></xdr:twoCellAnchor>"#,
        row + 10
    )
}

fn package() -> Vec<u8> {
    let drawing = format!(
        r#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">{}{}{}</xdr:wsDr>"#,
        frame("rId1", 1),
        frame("rId2", 15),
        chartex_frame("rId3", 29)
    );
    let drels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="../charts/chart1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="../charts/chart2.xml"/><Relationship Id="rId3" Type="http://schemas.microsoft.com/office/2014/relationships/chartEx" Target="../charts/chartEx3.xml"/></Relationships>"#;
    let srels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing" Target="../drawings/drawing1.xml"/></Relationships>"#;
    let sheet = r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Name</t></is></c><c r="B1" t="inlineStr"><is><t>Sales</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>One</t></is></c><c r="B2"><v>1</v></c></row><row r="3"><c r="A3" t="inlineStr"><is><t>Two</t></is></c><c r="B3"><v>4</v></c></row><row r="4"><c r="A4" t="inlineStr"><is><t>Three</t></is></c><c r="B4"><v>9</v></c></row><row r="5"><c r="A5" t="inlineStr"><is><t>Four</t></is></c><c r="B5"><v>16</v></c></row></sheetData><drawing r:id="rId1"/>"#;
    super::minimal(
        sheet,
        &[
            ("xl/worksheets/_rels/sheet1.xml.rels", srels),
            ("xl/drawings/drawing1.xml", &drawing),
            ("xl/drawings/_rels/drawing1.xml.rels", drels),
            ("xl/charts/chart1.xml", SURFACE_PART),
            ("xl/charts/chart2.xml", PIE3D_PART),
            ("xl/charts/chartEx3.xml", PARETO_PART),
        ],
        "",
        "",
    )
}

fn part(bytes: &[u8], name: &str) -> String {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
    let mut s = String::new();
    z.by_name(name).unwrap().read_to_string(&mut s).unwrap();
    s
}

#[test]
fn unmodelled_chart_kinds_keep_their_frame_and_their_part() {
    let (wb, rep) = read_xlsx(&package()).unwrap();
    assert!(wb.sheets[0].charts.is_empty(), "a 3-D, surface or Pareto chart is not read into the 2-D model");
    assert_eq!(wb.sheets[0].kept_charts.len(), 3);
    assert!(wb.sheets[0].kept_charts.iter().any(|c| c.chartex), "the Pareto chart is a chartex part");
    assert!(rep.warnings.iter().any(|w| w.contains("does not model")), "{:?}", rep.warnings);
    let back = write_xlsx(&wb).unwrap();
    // Every part is written back byte for byte, 3-D walls, surface and Pareto line included.
    assert_eq!(part(&back, "xl/charts/chart1.xml"), SURFACE_PART);
    assert_eq!(part(&back, "xl/charts/chart2.xml"), PIE3D_PART);
    assert_eq!(part(&back, "xl/charts/chartEx3.xml"), PARETO_PART);
    // The drawing still frames all three, the chartex one through mc:AlternateContent.
    let drawing = part(&back, "xl/drawings/drawing1.xml");
    assert_eq!(drawing.matches("<xdr:graphicFrame").count(), 3, "{drawing}");
    assert!(drawing.contains("AlternateContent"), "{drawing}");
    assert!(drawing.contains("chartex"), "{drawing}");
    // A second save is stable, and the frames come back with their parts.
    let (again, _rep) = read_xlsx(&back).unwrap();
    assert_eq!(again.sheets[0].kept_charts.len(), 3);
    let twice = write_xlsx(&again).unwrap();
    assert_eq!(part(&twice, "xl/charts/chart1.xml"), SURFACE_PART);
    assert_eq!(part(&twice, "xl/charts/chartEx3.xml"), PARETO_PART);
    assert_eq!(part(&twice, "xl/drawings/drawing1.xml"), drawing);
    // The chartex part comes with the style and colour parts Excel expects.
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(twice.clone())).unwrap();
    for name in ["xl/charts/chartEx3.xml", "xl/charts/style3.xml", "xl/charts/colors3.xml", "xl/charts/_rels/chartEx3.xml.rels"] {
        assert!(z.by_name(name).is_ok(), "{name} is missing");
    }
}
