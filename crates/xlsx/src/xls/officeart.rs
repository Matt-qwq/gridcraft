//! OfficeArt drawings in `.xls` files: pictures from `MSODRAWINGGROUP`/`MSODRAWING`.
//!
//! A sheet's drawing is the concatenation of its `MSODRAWING` record payloads ([MS-XLS] 2.4.174)
//! and holds an `OfficeArtDgContainer` tree ([MS-ODRAW] 2.2.16 onwards): a group of shapes, each
//! with an `OfficeArtFSP` (its type), an `OfficeArtFOPT` property table (the picture's `pib`, a
//! one-based index into the workgroup's BLIP store) and an `OfficeArtClientAnchorSheet` (the cells
//! and offsets it covers). The blips themselves live in the sheet's `MSODRAWINGGROUP`
//! (`OfficeArtDggContainer` → `OfficeArtBStoreContainer` → `OfficeArtFBSE`).
//!
//! Only raster pictures are imported: an EMF/WMF blip is a vector drawing the model cannot hold as
//! an [`Image`], so it is skipped with a warning, like charts and shapes (which the caller models
//! only as plain cells today).

use std::io::Read;

use gridcraft_core::{CellRef, MAX_COLS, MAX_ROWS};
use gridcraft_model::{Anchor, AnchorMode, Image, Shape, ShapeKind, Sheet};

/// Deepest nesting of OfficeArt containers followed (the real tree is four levels).
const MAX_DEPTH: usize = 8;
/// Records visited in one drawing, and shapes and blips imported, at most: a hostile file must not
/// turn into an unbounded walk or an unbounded list.
const MAX_RECORDS: usize = 100_000;
const MAX_PICTURES: usize = 10_000;

const DG_CONTAINER: u16 = 0xF002;
const DG_GROUP: u16 = 0xF003;
const SP_CONTAINER: u16 = 0xF004;
const SP: u16 = 0xF00A;
const OPT: u16 = 0xF00B;
const CLIENT_ANCHOR_SHEET: u16 = 0xF010;
const DGG_CONTAINER: u16 = 0xF000;
const BSTORE_CONTAINER: u16 = 0xF001;
const FBSE: u16 = 0xF007;
const FSP_PICTURE_FRAME: u16 = 0x004B;
/// `msosptChart`: the shape an embedded chart hangs from.
const FSP_CHART: u16 = 0x00C9;
const PIB: u16 = 0x0104;
/// `OfficeArtTertiaryFOPT`: vendor data; Excel 2007+ puts a DrawingML shape fragment here.
const TERTIARY_OPT: u16 = 0xF122;
const FSP_LINE: u16 = 0x0014;
const FSP_TEXT_BOX: u16 = 0x00CA;

/// One OfficeArt record's header and payload.
struct Record<'a> {
    inst: u16,
    ty: u16,
    data: &'a [u8],
}

/// The records of `buf`, in order; a record whose length would run past the buffer ends the walk
/// (a truncated drawing is skipped, not read as noise).
fn records<'a>(buf: &'a [u8], at: &mut usize) -> Option<Record<'a>> {
    let h = buf.get(*at..*at + 8)?;
    let vi = u16::from_le_bytes([h[0], h[1]]);
    let ty = u16::from_le_bytes([h[2], h[3]]);
    let len = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as usize;
    let start = *at + 8;
    // A drawing is split across `MSODRAWING`/`Continue` records, so a container's declared size
    // may run past the bytes gathered here; take what is present instead of losing the whole tree.
    // A leaf record that runs past the buffer still ends the walk (that is truncation).
    let data = match buf.get(start..start.checked_add(len)?) {
        Some(d) => d,
        None if matches!(ty, DG_CONTAINER | DG_GROUP | SP_CONTAINER | DGG_CONTAINER | BSTORE_CONTAINER) => buf.get(start..)?,
        None => return None,
    };
    *at = start + len;
    Some(Record { inst: vi >> 4, ty, data })
}

/// A BLIP (one picture's bytes) from the drawing group.
pub(super) struct Blip {
    pub mime: &'static str,
    pub data: Vec<u8>,
}

/// Reads the BLIP store of a sheet's drawing group, in `pib` order (index 0 is `pib` 1).
pub(super) fn read_blips(group: &[u8]) -> Vec<Blip> {
    let mut out: Vec<Blip> = Vec::new();
    if group.is_empty() {
        return out;
    }
    let mut at = 0usize;
    while let Some(r) = records(group, &mut at) {
        if r.ty == DGG_CONTAINER {
            walk_group(r.data, &mut out);
            break;
        }
    }
    out
}

fn walk_group(buf: &[u8], out: &mut Vec<Blip>) {
    let mut at = 0usize;
    while let Some(r) = records(buf, &mut at) {
        if r.ty == BSTORE_CONTAINER {
            let mut i = 0usize;
            while let Some(b) = records(r.data, &mut i) {
                if b.ty == FBSE {
                    out.push(read_fbse(b.data));
                }
            }
            return;
        }
    }
}

/// `OfficeArtFBSE` ([MS-ODRAW] 2.2.32): a 36-byte header, an optional name, then the blip record.
fn read_fbse(p: &[u8]) -> Blip {
    const HEADER: usize = 36;
    let cb_name = p.get(33).copied().unwrap_or(0) as usize;
    let mut at = HEADER.saturating_add(cb_name.saturating_mul(2));
    // A blip is a record; its payload is the two UIDs and the picture.
    let Some(rec) = records(p, &mut at) else { return Blip { mime: "", data: vec![] } };
    let (mime, header) = match rec.inst {
        // OfficeArtBlipEMF/WMF: a metafile header before the bytes, and a vector format the model
        // cannot hold as an image.
        0x3D4 | 0x3D5 | 0x216 | 0x217 => return Blip { mime: "", data: vec![] },
        0x6E0 | 0x6E1 => ("image/png", 16),
        0x46A | 0x46B => ("image/jpeg", 16),
        0x7A8 | 0x7A9 => ("image/bmp", 16),
        0x6E4 | 0x6E5 => ("image/tiff", 16),
        _ => return Blip { mime: "", data: vec![] },
    };
    // The second UID is present in the `…1` variants, then a one-byte `tag` (0xFF for an external
    // file) before the picture ([MS-ODRAW] 2.2.28): recLen is the data plus 17 or 33.
    let skip = header + if rec.inst & 1 == 1 { 16 } else { 0 } + 1;
    let data = rec.data.get(skip..).or(rec.data.get(header..)).unwrap_or(&[]).to_vec();
    Blip { mime, data }
}

/// Reads the pictures of a sheet's drawing: every `msosptPictureFrame` shape whose `pib` names a
/// raster blip becomes an [`Image`]. Returns the imported images and how many drawings were
/// skipped (charts, shapes and vector pictures).
pub(super) fn read_images(drawing: &[u8], blips: &[Blip], sheet: &Sheet) -> (Vec<Image>, usize) {
    let mut out: Vec<Image> = Vec::new();
    let mut skipped = 0usize;
    if drawing.is_empty() {
        return (out, skipped);
    }
    let mut at = 0usize;
    let mut visits = 0usize;
    while let Some(r) = records(drawing, &mut at) {
        if r.ty == DG_CONTAINER {
            walk_shapes(r.data, blips, sheet, &mut out, &mut skipped, &mut visits, 0);
            break;
        }
    }
    (out, skipped)
}

fn walk_shapes(buf: &[u8], blips: &[Blip], sheet: &Sheet, out: &mut Vec<Image>, skipped: &mut usize, visits: &mut usize, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    let mut at = 0usize;
    while let Some(r) = records(buf, &mut at) {
        *visits += 1;
        if *visits > MAX_RECORDS || out.len() >= MAX_PICTURES {
            return;
        }
        if r.ty == DG_GROUP || r.ty == SP_CONTAINER {
            walk_shapes(r.data, blips, sheet, out, skipped, visits, depth + 1);
            continue;
        }
        if r.ty != SP {
            continue;
        }
        // A group shape's own SP record has `spid`/`flags`, not the picture frame type; it holds
        // the children the walk reaches through the group container, so it is not a skipped
        // object. Any other non-picture leaf (a chart, an autoshape, a control) is.
        // Charts and the shapes are read by their own readers; only pictures count here. A
        // picture frame without a usable raster blip (a vector picture) is what is skipped.
        if r.inst != FSP_PICTURE_FRAME {
            continue;
        }
        // The rest of the shape follows in the same container; look ahead for its properties and
        // anchor instead of walking sibling records again.
        let (pib, anchor) = shape_head(buf, at, sheet);
        let Some(anchor) = anchor else {
            *skipped += 1;
            continue;
        };
        let Some(blip) =
            pib.checked_sub(1).and_then(|i| usize::try_from(i).ok()).and_then(|i| blips.get(i)).filter(|b| !b.data.is_empty() && !b.mime.is_empty())
        else {
            *skipped += 1;
            continue;
        };
        let id = next_id(out);
        out.push(Image { id, anchor, data: blip.data.clone().into(), mime: blip.mime.to_string(), alt: String::new() });
    }
}

/// The anchors of the `msosptChart` shapes in a sheet's drawing, in document order. An embedded
/// chart has no pointer to its shape: the chart substreams appear in the same order as the chart
/// shapes in the drawing ([MS-XLS] 2.2.3.1), so the caller pairs the n-th chart with the n-th
/// anchor. A drawing split across several `MSODRAWING` records is one tree: the caller passes the
/// concatenated payloads.
pub(super) fn chart_anchors(drawing: &[u8], sheet: &Sheet) -> Vec<Anchor> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while let Some(r) = records(drawing, &mut at) {
        if r.ty == DG_CONTAINER {
            collect_charts(r.data, sheet, 0, &mut out);
        }
    }
    out
}

fn collect_charts(buf: &[u8], sheet: &Sheet, depth: usize, out: &mut Vec<Anchor>) {
    if depth > MAX_DEPTH || out.len() >= MAX_PICTURES {
        return;
    }
    let mut at = 0usize;
    while let Some(r) = records(buf, &mut at) {
        if r.ty == DG_GROUP || r.ty == SP_CONTAINER {
            collect_charts(r.data, sheet, depth + 1, out);
            continue;
        }
        if r.ty == SP && r.inst == FSP_CHART {
            let (_, anchor) = shape_head(buf, at, sheet);
            if let Some(anchor) = anchor {
                out.push(anchor);
            }
        }
    }
}

/// Reads the autoshapes and text boxes of a sheet's drawing. Excel 2007+ writes a DrawingML
/// fragment (`drs/shapexml.xml` in an `OfficeArtTertiaryFOPT` record) for each shape, which keeps
/// its geometry, fill, line, text and text formatting; a shape without one (an older writer) is
/// rebuilt from its `OfficeArtFSP` type and anchor. Returns the shapes and how many drawings were
/// skipped (OLE objects, controls and shape types the model cannot express).
pub(super) fn read_shapes(drawing: &[u8], sheet: &Sheet) -> (Vec<Shape>, usize) {
    let mut out = Vec::new();
    let mut skipped = 0usize;
    let mut at = 0usize;
    while let Some(r) = records(drawing, &mut at) {
        if r.ty == DG_CONTAINER {
            walk_shape_containers(r.data, sheet, &mut out, &mut skipped);
        }
    }
    (out, skipped)
}

fn walk_shape_containers(buf: &[u8], sheet: &Sheet, out: &mut Vec<Shape>, skipped: &mut usize) {
    let mut at = 0usize;
    while let Some(r) = records(buf, &mut at) {
        if out.len() >= MAX_PICTURES {
            return;
        }
        if r.ty == DG_GROUP {
            walk_shape_containers(r.data, sheet, out, skipped);
        } else if r.ty == SP_CONTAINER {
            read_shape_container(r.data, sheet, out, skipped);
        }
    }
}

/// One `OfficeArtSpContainer`: the shape's type (`OfficeArtFSP`), its anchor, and its DrawingML
/// fragment when the writer left one.
fn read_shape_container(container: &[u8], sheet: &Sheet, out: &mut Vec<Shape>, skipped: &mut usize) {
    let mut inst = 0u16;
    let mut anchor = None;
    let mut fragment: Option<Shape> = None;
    let mut at = 0usize;
    while let Some(r) = records(container, &mut at) {
        match r.ty {
            SP => inst = r.inst,
            CLIENT_ANCHOR_SHEET if anchor.is_none() => anchor = read_anchor(r.data, sheet),
            TERTIARY_OPT if fragment.is_none() => fragment = shape_fragment(r.data, sheet),
            _ => {}
        }
    }
    if inst == FSP_PICTURE_FRAME || inst == FSP_CHART {
        return;
    }
    // A group shape's own `OfficeArtFSP` is type 0 and has no anchor; its children are the shapes.
    if inst == 0 && anchor.is_none() {
        return;
    }
    if let Some(shape) = fragment {
        out.push(shape);
        return;
    }
    let Some(anchor) = anchor else {
        *skipped += 1;
        return;
    };
    let Some(kind) = primitive_kind(inst) else {
        *skipped += 1;
        return;
    };
    out.push(Shape {
        id: 0,
        kind,
        anchor,
        fill: gridcraft_model::Color::default(),
        line: gridcraft_model::Color::default(),
        text: String::new(),
        alt: None,
        format: gridcraft_model::ShapeFormat::default(),
    });
}

/// A shape type the model has a kind for, when a file has no DrawingML fragment.
fn primitive_kind(inst: u16) -> Option<ShapeKind> {
    Some(match inst {
        0x01 => ShapeKind::Rectangle,
        0x02 => ShapeKind::RoundedRectangle,
        0x03 => ShapeKind::Ellipse,
        0x05 => ShapeKind::Triangle,
        FSP_LINE => ShapeKind::Line,
        0x0D => ShapeKind::Arrow,
        FSP_TEXT_BOX => ShapeKind::TextBox,
        _ => return None,
    })
}

/// The DrawingML fragment in an `OfficeArtTertiaryFOPT` record: a small zip holding
/// `drs/shapexml.xml` after a 12-byte header (the zip may start at any offset; its signature is
/// what identifies it). Bounded so a hostile record cannot inflate without limit.
fn shape_fragment(payload: &[u8], sheet: &Sheet) -> Option<Shape> {
    const MAX_FRAGMENT: usize = 1 << 20;
    let start = payload.windows(4).take(64).position(|w| w == b"PK")?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(payload.get(start..)?)).ok()?;
    let file = zip.by_name("drs/shapexml.xml").ok()?;
    let mut xml = String::new();
    file.take(MAX_FRAGMENT as u64).read_to_string(&mut xml).ok()?;
    crate::drawing::read_xls_shape_fragment(&xml, sheet)
}

/// The remaining records of the same `OfficeArtSpContainer`: the property table's `pib` and the
/// client anchor. `at` is just past the shape's `OfficeArtFSP`.
fn shape_head(buf: &[u8], from: usize, sheet: &Sheet) -> (u32, Option<Anchor>) {
    let mut pib = 0u32;
    let mut anchor = None;
    let mut at = from;
    while let Some(r) = records(buf, &mut at) {
        if r.ty == SP_CONTAINER || r.ty == DG_GROUP {
            break;
        }
        if r.ty == OPT {
            if pib == 0 {
                pib = read_pib(r.data).unwrap_or(0);
            }
        } else if r.ty == CLIENT_ANCHOR_SHEET && anchor.is_none() {
            anchor = read_anchor(r.data, sheet);
        }
    }
    (pib, anchor)
}

/// `OfficeArtFOPT` ([MS-ODRAW] 2.2.9): `recInstance` entries of 6 bytes (`OfficeArtFOPTE`: a
/// property id and its 4-byte value), then the complex properties' data in entry order. Complex
/// entries are skipped: `pib` is a plain one.
fn read_pib(p: &[u8]) -> Option<u32> {
    let mut at = 0usize;
    while at + 6 <= p.len() {
        let opid = u16::from_le_bytes([p[at], p[at + 1]]);
        let op = u32::from_le_bytes([p[at + 2], p[at + 3], p[at + 4], p[at + 5]]);
        at += 6;
        if opid & 0x8000 == 0 && opid & 0x3FFF == PIB {
            return Some(op);
        }
    }
    None
}

/// `OfficeArtClientAnchorSheet` ([MS-XLS] 2.5.193): the move/size bits, the top-left cell and its
/// offsets in 1/1024 of a column width and 1/256 of a row height, then the bottom-right cell.
fn read_anchor(p: &[u8], sheet: &Sheet) -> Option<Anchor> {
    if p.len() < 18 {
        return None;
    }
    let u = |i: usize| u16::from_le_bytes([p[i], p[i + 1]]);
    let (flags, col_l, dx_l, row_t, dy_t, col_r, dx_r, row_b, dy_b) = (u(0), u(2), u(4), u(6), u(8), u(10), u(12), u(14), u(16));
    let col_l = u32::from(col_l).min(MAX_COLS - 1);
    let row_t = u32::from(row_t).min(MAX_ROWS - 1);
    let col_r = u32::from(col_r).min(MAX_COLS - 1);
    let row_b = u32::from(row_b).min(MAX_ROWS - 1);
    let x1 = sheet.col_left(col_l) + f64::from(dx_l) / 1024.0 * f64::from(sheet.col_width(col_l));
    let y1 = sheet.row_top(row_t) + f64::from(dy_t) / 256.0 * f64::from(sheet.row_height(row_t));
    let x2 = sheet.col_left(col_r) + f64::from(dx_r) / 1024.0 * f64::from(sheet.col_width(col_r));
    let y2 = sheet.row_top(row_b) + f64::from(dy_b) / 256.0 * f64::from(sheet.row_height(row_b));
    let cell = CellRef::new(row_t, col_l);
    let dx = clamp((x1 - sheet.col_left(col_l)) as f32);
    let dy = clamp((y1 - sheet.row_top(row_t)) as f32);
    // A bottom-right corner before the top-left one is damage: keep a zero-sized object.
    let width = clamp((x2 - x1).max(0.0) as f32);
    let height = clamp((y2 - y1).max(0.0) as f32);
    // The two bits are "do not move" and "do not size" (Excel wrote 0x0000 for a shape set to
    // move and size with cells, 0x0002 for move only and 0x0003 for free-floating).
    let mode = if flags & 0x0001 != 0 {
        AnchorMode::Absolute
    } else if flags & 0x0002 != 0 {
        AnchorMode::MoveOnly
    } else {
        AnchorMode::MoveAndSize
    };
    Some(Anchor { cell, dx, dy, width, height, mode })
}

/// An anchor offset, side or the like: finite, non-negative and no wider than any sheet.
fn clamp(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1_000_000.0) } else { 0.0 }
}

/// A picture id not already used in this sheet (ids share a space with charts and shapes; the
/// reader starts at 1 and the sheet holds no other objects yet).
fn next_id(taken: &[Image]) -> u32 {
    taken.iter().map(|i| i.id).max().unwrap_or(0).saturating_add(1)
}
