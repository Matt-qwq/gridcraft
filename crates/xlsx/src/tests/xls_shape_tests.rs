//! `.xls` shapes: OfficeArt autoshapes and text boxes become model shapes, from Excel 2007+'s
//! DrawingML fragment when present and from the OfficeArt type and anchor otherwise.

use super::xls_tests::{bof, cat, cell, compound, eof, rec, u16s};
use crate::read_xls;

/// One OfficeArt record: `inst`, type and payload. Container records use recVer 0xF.
fn oa(inst: u16, ty: u16, container: bool, payload: &[u8]) -> Vec<u8> {
    let mut v = ((inst << 4) | if container { 0x0F } else { 0x02 }).to_le_bytes().to_vec();
    v.extend_from_slice(&ty.to_le_bytes());
    v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    v.extend_from_slice(payload);
    v
}

/// A `ClientAnchorSheet`: move-and-size, A1 to C4 with no offsets.
fn anchor() -> Vec<u8> {
    oa(0, 0xF010, false, &u16s(&[0, 0, 0, 0, 0, 1, 0, 3, 0]))
}

/// An `OfficeArtSpContainer` holding one shape of OfficeArt type `inst`, plus optional extra
/// records (a tertiary opt with a DrawingML fragment).
fn sp_container(inst: u16, extra: &[u8]) -> Vec<u8> {
    let sp = oa(inst, 0xF00A, false, &cat(&[&u32s(&[1024]), &[0; 4]]));
    oa(0, 0xF004, true, &cat(&[&sp, &anchor(), extra]))
}

fn u32s(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// The sheet drawing: a `DgContainer` holding a `DgGroup` of `shapes`.
fn drawing(shapes: &[Vec<u8>]) -> Vec<u8> {
    let group = oa(0, 0xF003, true, &shapes.concat());
    let dg = oa(1, 0xF008, false, &[0; 8]);
    oa(0, 0xF002, true, &cat(&[&dg, &group]))
}

/// A workbook with one worksheet, one cell and the given drawing bytes.
fn workbook(drawing: &[u8]) -> Vec<u8> {
    let sheet = cat(&[&bof(0x0010), &rec(0x0203, &cat(&[&cell(0, 0, 0), &1.0f64.to_le_bytes()])), &rec(0x00EC, drawing), &eof()]);
    let globals = |pos: u32| cat(&[&bof(0x0005), &rec(0x0085, &cat(&[&pos.to_le_bytes(), &[0, 0, 6, 0], b"Sheet1"])), &eof()]);
    let pos = globals(0).len() as u32;
    cat(&[&globals(pos), &sheet])
}

/// An `OfficeArtTertiaryFOPT` record whose payload is a 12-byte header, then a zip holding
/// `drs/shapexml.xml`.
fn fragment(xml: &str) -> Vec<u8> {
    let zip = super::make_zip(&[("drs/shapexml.xml", xml.as_bytes())]);
    let mut payload = vec![0u8; 12];
    payload.extend_from_slice(&zip);
    oa(0, 0xF122, false, &payload)
}

const FRAGMENT: &str = r#"<xdr:anchor xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><xdr:twoCellAnchor><xdr:from><xdr:col>0</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>0</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>1</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>3</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><xdr:sp><xdr:nvSpPr><xdr:cNvPr id="2" name="TextBox 1"/><xdr:cNvSpPr txBox="1"/></xdr:nvSpPr><xdr:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1270000" cy="762000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></xdr:spPr><xdr:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" sz="1100"/><a:t>Hi</a:t></a:r></a:p></xdr:txBody></xdr:sp><xdr:clientData/></xdr:twoCellAnchor></xdr:anchor>"#;

#[test]
fn reads_a_primitive_shape_without_a_fragment() {
    // An older writer's file: no tertiary opt, only the OfficeArt shape type and the anchor.
    let file = compound("Workbook", &workbook(&drawing(&[sp_container(0x03, &[])])));
    let (wb, _) = read_xls(&file).unwrap();
    let shapes = &wb.sheets[0].shapes;
    assert_eq!(shapes.len(), 1);
    assert_eq!(shapes[0].kind, gridcraft_model::ShapeKind::Ellipse);
    assert!(shapes[0].anchor.width > 0.0 && shapes[0].anchor.height > 0.0);
    assert_eq!(shapes[0].id, 1);
}

#[test]
fn reads_a_shape_from_its_drawingml_fragment() {
    // Excel 2007+ writes a DrawingML fragment for a shape; it carries text, fill and geometry.
    let file = compound("Workbook", &workbook(&drawing(&[sp_container(0x01, &fragment(FRAGMENT))])));
    let (wb, _) = read_xls(&file).unwrap();
    let shapes = &wb.sheets[0].shapes;
    assert_eq!(shapes.len(), 1);
    let shape = &shapes[0];
    assert_eq!(shape.kind, gridcraft_model::ShapeKind::TextBox);
    assert_eq!(shape.text, "Hi");
    assert_eq!(shape.fill, gridcraft_model::Color::rgb(255, 0, 0));
}

#[test]
fn a_damaged_shape_record_does_not_panic() {
    let full = workbook(&drawing(&[sp_container(0x03, &fragment(FRAGMENT))]));
    for cut in (0..full.len()).step_by(23) {
        let _ = read_xls(&compound("Workbook", &full[..cut]));
    }
}
