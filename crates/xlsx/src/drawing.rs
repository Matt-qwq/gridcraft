//! DrawingML spreadsheet drawings (`xl/drawings/drawingN.xml`): anchors, pictures, shapes, chart
//! frames, and the frames that place slicers and timelines.
//!
//! A slicer or timeline view lives in its own part but is placed by a drawing: an `xdr:graphicFrame`
//! whose `a:graphicData` names the drawing namespace of the widget and holds a `<slicer name>` /
//! `<timeslicer name>` element ([MS-XLSX] 2.2.4.2, 5.2, 5.7). The frame carries no relationship —
//! the name matches the view in the sheet's slicers/timelines part — so the model reads a widget's
//! anchor from here and writes it back here ([`WidgetFrame`]).

use std::fmt::Write as _;

use gridcraft_core::{CellRef, MAX_COLS, MAX_ROWS};
use gridcraft_model::{Anchor, AnchorMode, Color, Equation, Image, Shape, ShapeFormat, ShapeKind, ShapeTextAlign, ShapeTextAnchor, Sheet, Theme};
use quick_xml::events::Event;

use crate::IoError;
use crate::read::Ctx;
use crate::tables::EMU_PER_PX;
use crate::xml::{El, esc, esc_attr};

mod ink;

/// The anchor a slicer or timeline view is placed with, read from a sheet's drawing.
pub struct WidgetFrame {
    /// The view's name (a slicer's or timeline's `name`, unique per kind in the workbook).
    pub name: String,
    pub anchor: Anchor,
    /// `false` for a slicer view, `true` for a timeline view.
    pub timeline: bool,
}

/// The largest object side, in pixels, an anchor is written with: 1,000,000 px is wider than any
/// sheet. Model anchors come from files and commands, so their numbers are hostile — NaN, negative
/// or absurd values are clamped here rather than trusted.
const MAX_OBJECT_PX: f64 = 1_000_000.0;

/// The DrawingML 2010 extension namespace (`a14`), where Excel puts OMML inside a drawing.
const NS_A14: &str = "http://schemas.microsoft.com/office/drawing/2010/main";
/// The Office Math Markup namespace (`m`).
const NS_MATH: &str = "http://schemas.openxmlformats.org/officeDocument/2006/math";

/// Deepest match/wrapper nesting walked inside a shape's math paragraph.
const MAX_MATH_DEPTH: usize = 64;
/// Longest linear text an equation's runs are flattened into (a text box holds 32,767 characters;
/// this is that as bytes, with room for a character of four bytes).
const MAX_MATH_TEXT_BYTES: usize = 32_767 * 4;
/// Largest OMML kept for one equation.
const MAX_OMML_BYTES: usize = 1024 * 1024;
/// Most equations one drawing part may carry.
const MAX_EQUATIONS: usize = 10_000;

fn clamp_px(v: f64) -> f64 {
    if v.is_finite() { v.clamp(0.0, MAX_OBJECT_PX) } else { 0.0 }
}

// ---------------------------------------------------------------- reading

fn emu_px(v: Option<i64>) -> f32 {
    v.map(|e| (e as f64 / EMU_PER_PX).clamp(-1e7, 1e7) as f32).unwrap_or(0.0)
}

fn marker(e: &El) -> (CellRef, f32, f32) {
    let num = |n: &str| e.child(n).and_then(|c| c.text.trim().parse::<i64>().ok());
    let col = num("col").unwrap_or(0).clamp(0, MAX_COLS as i64 - 1) as u32;
    let row = num("row").unwrap_or(0).clamp(0, MAX_ROWS as i64 - 1) as u32;
    (CellRef::new(row, col), emu_px(num("colOff")).max(0.0), emu_px(num("rowOff")).max(0.0))
}

fn ext_px(e: Option<&El>) -> (f32, f32) {
    match e {
        Some(x) => (emu_px(x.attr_i64("cx")).max(0.0), emu_px(x.attr_i64("cy")).max(0.0)),
        None => (0.0, 0.0),
    }
}

fn read_anchor(a: &El, sheet: &Sheet) -> Option<Anchor> {
    match a.name.as_str() {
        "twoCellAnchor" => {
            let (cell, dx, dy) = marker(a.child("from")?);
            let (to, tdx, tdy) = a.child("to").map(marker).unwrap_or((cell, dx, dy));
            let x1 = sheet.col_left(cell.col) + dx as f64;
            let y1 = sheet.row_top(cell.row) + dy as f64;
            let x2 = sheet.col_left(to.col) + tdx as f64;
            let y2 = sheet.row_top(to.row) + tdy as f64;
            // Excel stores "Move and size with cells" as a plain twoCellAnchor, "Move but don't
            // size" as editAs="oneCell", and "Don't move or size" as editAs="absolute".
            let mode = match a.attr("editAs") {
                Some("absolute") => AnchorMode::Absolute,
                Some("oneCell") => AnchorMode::MoveOnly,
                _ => AnchorMode::MoveAndSize,
            };
            Some(Anchor { cell, dx, dy, width: (x2 - x1).max(0.0) as f32, height: (y2 - y1).max(0.0) as f32, mode })
        }
        "oneCellAnchor" => {
            let (cell, dx, dy) = marker(a.child("from")?);
            let (width, height) = ext_px(a.child("ext"));
            Some(Anchor { cell, dx, dy, width, height, mode: AnchorMode::MoveOnly })
        }
        "absoluteAnchor" => {
            let pos = a.child("pos");
            let x = emu_px(pos.and_then(|p| p.attr_i64("x"))).max(0.0) as f64;
            let y = emu_px(pos.and_then(|p| p.attr_i64("y"))).max(0.0) as f64;
            let col = sheet.col_at(x);
            let row = sheet.row_at(y);
            let (width, height) = ext_px(a.child("ext"));
            Some(Anchor {
                cell: CellRef::new(row, col),
                dx: (x - sheet.col_left(col)).max(0.0) as f32,
                dy: (y - sheet.row_top(row)).max(0.0) as f32,
                width,
                height,
                mode: AnchorMode::Absolute,
            })
        }
        _ => None,
    }
}

/// Colour of a DrawingML colour choice inside `e` (e.g. a `solidFill`).
pub fn dml_color(e: &El) -> Option<Color> {
    if let Some(c) = e.child("srgbClr").and_then(|c| c.attr("val")) {
        return Color::from_hex(c);
    }
    if let Some(c) = e.child("sysClr").and_then(|c| c.attr("lastClr")) {
        return Color::from_hex(c);
    }
    if let Some(c) = e.child("schemeClr").and_then(|c| c.attr("val")) {
        let i = match c {
            "bg1" | "lt1" => 0,
            "tx1" | "dk1" => 1,
            "bg2" | "lt2" => 2,
            "tx2" | "dk2" => 3,
            "accent1" => 4,
            "accent2" => 5,
            "accent3" => 6,
            "accent4" => 7,
            "accent5" => 8,
            "accent6" => 9,
            "hlink" => 10,
            "folHlink" => 11,
            _ => return None,
        };
        return Some(Color::Theme(i, 0));
    }
    None
}

fn mime_for(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" | "jpe" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "svg" => "image/svg+xml",
        "emf" => "image/x-emf",
        "wmf" => "image/x-wmf",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

pub fn read_drawing(cx: &mut Ctx<'_>, part: &str, sheet: &mut Sheet) -> Result<(), IoError> {
    // The part's text as well as its tree: an equation's OMML is kept byte for byte, so it comes
    // from the source rather than from a re-serialization of the parsed tree.
    let Some((x, text)) = cx.optional_xml_text(part)? else { return Ok(()) };
    let rels = cx.pkg.rels(part)?;
    let maths = math_fragments(&text);
    let mut math_i = 0usize;
    for a in &x.children {
        let Some(anchor) = read_anchor(a, sheet) else { continue };
        // Objects in markup-compatibility blocks: the first choice (chartex frames), else the
        // fallback.
        let objs: Vec<&El> = a
            .children
            .iter()
            .flat_map(|o| match o.name.as_str() {
                "AlternateContent" => o
                    .child("Choice")
                    // The preferred branch is the one we can read: a chartex frame, or the shape an
                    // equation (or a shape a newer writer draws) lives in. The fallback is the
                    // dumbed-down picture of it.
                    .filter(|c| c.children.iter().any(|k| matches!(k.name.as_str(), "graphicFrame" | "sp" | "cxnSp")))
                    .or_else(|| o.child("Fallback"))
                    .map(|c| c.children.iter().collect())
                    .unwrap_or_default(),
                _ => vec![o],
            })
            .collect();
        for obj in objs {
            match obj.name.as_str() {
                "pic" => {
                    let Some(rid) = obj.path(&["blipFill", "blip"]).and_then(|b| b.attr("embed")) else { continue };
                    let Some(rel) = rels.iter().find(|r| r.id == rid && !r.external) else {
                        cx.warn("a linked (not embedded) picture was skipped");
                        continue;
                    };
                    let target = rel.target.clone();
                    let Some(data) = cx.pkg.read(&target)? else {
                        cx.warn(format!("missing picture {target}"));
                        continue;
                    };
                    let alt = obj.path(&["nvPicPr", "cNvPr"]).and_then(|c| c.attr("descr")).unwrap_or("").to_string();
                    let id = cx.next_object_id();
                    sheet.images.push(Image { id, anchor, data: data.into(), mime: mime_for(&target).to_string(), alt });
                }
                "graphicFrame" => {
                    let Some(gd) = obj.path(&["graphic", "graphicData"]) else { continue };
                    let uri = gd.attr("uri").unwrap_or("");
                    // A slicer/timeline frame names its view; the part is read separately and the
                    // anchor is what this drawing contributes.
                    if uri == crate::slicer::NS_DRAWING_SLICER || uri == crate::slicer::NS_DRAWING_TIMELINE {
                        let timeline = uri == crate::slicer::NS_DRAWING_TIMELINE;
                        let tag = if timeline { "timeslicer" } else { "slicer" };
                        match gd.child(tag).and_then(|e| e.attr("name")) {
                            Some(name) if !name.trim().is_empty() => {
                                cx.widget_frames.push(WidgetFrame { name: name.to_string(), anchor, timeline });
                            }
                            _ => cx.warn("a slicer/timeline drawing frame that names no view was skipped"),
                        }
                        continue;
                    }
                    if let Some(rid) = gd.child("chart").and_then(|c| c.attr("id")) {
                        let Some(rel) = rels.iter().find(|r| r.id == rid) else { continue };
                        let target = rel.target.clone();
                        let chartex = uri.contains("chartex");
                        // A chart part with relationships of its own (external data, user shapes) can't
                        // be written back on its own: those parts are not kept, and its `r:id`s
                        // would dangle.
                        let rels = cx.pkg.rels(&target)?;
                        // A kept part is written back on its own, so it may only carry relationships
                        // GridCraft writes again: a chart's own style and colour parts are, for a
                        // chartex part and a classic one alike.
                        let kept_ok =
                            rels.is_empty() || rels.iter().all(|r| !r.external && matches!(r.kind.as_str(), "chartStyle" | "chartColorStyle"));
                        let part = cx.optional_xml_text(&target)?;
                        if let Some((cx_xml, text)) = part {
                            let modelled = if chartex { crate::chartex::is_modelled(&cx_xml) } else { crate::chart::is_modelled(&cx_xml) };
                            let read = if !modelled {
                                None
                            } else if chartex {
                                crate::chartex::read_chartex(&cx_xml)
                            } else {
                                crate::chart::read_chart(&cx_xml)
                            };
                            match read {
                                Some(mut ch) => {
                                    ch.id = cx.next_object_id();
                                    ch.anchor = anchor;
                                    ch.alt = read_alt(obj.path(&["nvGraphicFramePr", "cNvPr"]));
                                    // The part is kept so the options GridCraft does not model survive
                                    // a save: a classic part is patched element by element, a
                                    // chartex part too (its hidden `_xlchart` names are recorded on
                                    // it by the reader and written again).
                                    if kept_ok {
                                        ch.keep_part(text.into());
                                        // A file's chart keeps its look: its own style and colour
                                        // parts go back with the chart part, byte for byte.
                                        let (style, colors) = crate::chartex::read_style_parts(cx, &rels);
                                        ch.keep_style_parts(style, colors);
                                    }
                                    sheet.charts.push(ch);
                                }
                                // A kind GridCraft cannot model (3-D, surface, pie of pie, Pareto, a
                                // filled map) is kept whole with its frame: rebuilding it from the
                                // model would silently lose the chart.
                                None if kept_ok => {
                                    let id = cx.next_object_id();
                                    cx.warn(format!("chart {target} is a kind GridCraft does not model: kept as it was, not drawn"));
                                    let (style, colors) = crate::chartex::read_style_parts(cx, &rels);
                                    sheet.kept_charts.push(gridcraft_model::KeptChart {
                                        id,
                                        anchor,
                                        xml: text.into(),
                                        chartex,
                                        style,
                                        colors,
                                        alt: read_alt(obj.path(&["nvGraphicFramePr", "cNvPr"])),
                                    });
                                }
                                None => cx.warn(format!("chart {target} has no supported plot and was skipped")),
                            }
                        }
                    } else if uri.contains("chartex") || uri.contains("chartEx") {
                        cx.warn("a histogram, waterfall, treemap… chart without its chart part was skipped");
                    } else {
                        cx.warn("an embedded object (SmartArt, OLE…) was skipped");
                    }
                }
                "sp" | "cxnSp" => {
                    // A shape whose text body carries an OMML math paragraph is an equation, not a
                    // shape: what the file holds is the math, and the runs are only its linear text.
                    if let Some(math) = obj.child("txBody").and_then(math_element) {
                        let omml = maths.get(math_i).filter(|f| valid_omml(f)).cloned();
                        if omml.is_none() {
                            cx.warn(format!("equation {math_i} in {part} could not be kept byte for byte: its math was read as text"));
                        }
                        math_i = math_i.saturating_add(1);
                        let id = cx.next_object_id();
                        sheet.equations.push(Equation { id, ..read_equation(obj, math, anchor, omml) });
                    } else if let Some(s) = read_shape(obj, anchor) {
                        let id = cx.next_object_id();
                        sheet.shapes.push(Shape { id, ..s });
                    }
                }
                "grpSp" => cx.warn("grouped shapes are not supported and were skipped"),
                _ => {}
            }
        }
    }
    Ok(())
}

fn read_shape(obj: &El, anchor: Anchor) -> Option<Shape> {
    let sp = obj.child("spPr")?;
    if let Some(shape) = ink::read(obj, anchor) {
        return Some(shape);
    }
    let prst = sp.child("prstGeom").and_then(|g| g.attr("prst")).unwrap_or("rect");
    let tx_box = obj.path(&["nvSpPr", "cNvSpPr"]).is_some_and(|c| c.flag("txBox", false));
    let ln = sp.child("ln");
    let arrow = ln.is_some_and(|l| l.child("tailEnd").or_else(|| l.child("headEnd")).is_some_and(|t| t.attr("type").is_some_and(|v| v != "none")));
    let kind = match prst {
        _ if tx_box => ShapeKind::TextBox,
        "roundRect" => ShapeKind::RoundedRectangle,
        "ellipse" => ShapeKind::Ellipse,
        "triangle" => ShapeKind::Triangle,
        "line" | "straightConnector1" if arrow => ShapeKind::Arrow,
        "line" | "straightConnector1" => ShapeKind::Line,
        "rightArrow" => ShapeKind::Arrow,
        _ => ShapeKind::Rectangle,
    };
    let fill = sp.child("solidFill").and_then(dml_color).unwrap_or_default();
    let line = ln.and_then(|l| l.child("solidFill")).and_then(dml_color).unwrap_or_default();
    let text = obj
        .child("txBody")
        .map(|tb| {
            tb.kids("p").map(|p| p.kids("r").filter_map(|r| r.child("t")).map(|t| t.text.as_str()).collect::<String>()).collect::<Vec<_>>().join("\n")
        })
        .unwrap_or_default();
    let mut format = read_text_format(obj.child("txBody"));
    format.no_line = ln.is_some_and(|l| l.child("noFill").is_some());
    let alt = read_alt(obj.path(&["nvSpPr", "cNvPr"]));
    Some(Shape { id: 0, kind, anchor, fill, line, text, alt, format })
}

/// The `a14:m` element inside a shape's text body (Excel's equation), found anywhere below it:
/// a file may put it straight in an `a:p` or wrap the paragraph in a compatibility block.
fn math_element(body: &El) -> Option<&El> {
    fn find(e: &El, depth: usize) -> Option<&El> {
        if depth > MAX_MATH_DEPTH {
            return None;
        }
        e.children.iter().find(|c| c.name == "m").or_else(|| e.children.iter().find_map(|c| find(c, depth + 1)))
    }
    find(body, 0)
}

/// The equation's linear text: every `m:t` run of the math paragraph in document order. The
/// structure is flattened (a fraction reads as its numerator's and denominator's runs side by side)
/// because the layout belongs to the file, not to the model; the file's own markup is what is kept.
fn math_text(e: &El, out: &mut String, depth: usize) {
    if depth > MAX_MATH_DEPTH || out.len() > MAX_MATH_TEXT_BYTES {
        return;
    }
    for c in &e.children {
        if c.name == "t" {
            out.push_str(&c.text);
        } else {
            math_text(c, out, depth + 1);
        }
    }
}

/// An equation from a shape that carries math: its box (fill, outline, anchor), its linear text,
/// and the file's own markup when it could be kept. `omml` arrives separately because it is cut
/// out of the part's text, not built from the tree.
fn read_equation(obj: &El, math: &El, anchor: Anchor, omml: Option<String>) -> Equation {
    let sp = obj.child("spPr");
    let ln = sp.and_then(|s| s.child("ln"));
    let fill = sp.and_then(|s| s.child("solidFill")).and_then(dml_color).unwrap_or_default();
    let line = ln.and_then(|l| l.child("solidFill")).and_then(dml_color).unwrap_or_default();
    let mut source = String::new();
    math_text(math, &mut source, 0);
    // A run may push the text past the cap; cut on a character boundary, never through one.
    if source.len() > MAX_MATH_TEXT_BYTES {
        let mut cut = MAX_MATH_TEXT_BYTES;
        while cut > 0 && !source.is_char_boundary(cut) {
            cut -= 1;
        }
        source.truncate(cut);
    }
    Equation { id: 0, anchor, fill, line, source, omml, alt: read_alt(obj.path(&["nvSpPr", "cNvPr"])) }
}

/// The content of every `<a14:m>` element of a drawing part, in document order, exactly as the
/// file wrote it: what an equation's model keeps so a save writes its math back unchanged. Bounded
/// in count and size, since a part came from a file.
fn math_fragments(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut reader = quick_xml::Reader::from_str(text);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = e.name().as_ref().to_vec();
                if e.local_name().as_ref() != b"m" || !name.starts_with(b"a14:") {
                    continue;
                }
                let start = usize::try_from(reader.buffer_position()).unwrap_or(0);
                if reader.read_to_end(e.name()).is_err() {
                    return out;
                }
                let end = usize::try_from(reader.buffer_position()).unwrap_or(0);
                // Less the closing tag (`</a14:m>`, the name plus `</` and `>`).
                let close = name.len().saturating_add(3);
                if let Some(frag) = end.checked_sub(close).and_then(|stop| text.get(start..stop))
                    && frag.len() <= MAX_OMML_BYTES
                {
                    out.push(frag.to_string());
                }
                if out.len() >= MAX_EQUATIONS {
                    return out;
                }
            }
            Ok(Event::Eof) | Err(_) => return out,
            _ => {}
        }
    }
}

/// Whether a kept (or caller-supplied) OMML fragment is XML we can write inside `<a14:m>`: it
/// must parse as a run of balanced elements and close before its wrapper does. A fragment that is
/// not is replaced by markup generated from the equation's text, so a damaged — or hostile — value
/// never reaches a file.
fn valid_omml(frag: &str) -> bool {
    if frag.len() > MAX_OMML_BYTES || frag.contains("<?") {
        return false;
    }
    let wrapped = format!("<x xmlns:m=\"{NS_MATH}\">{frag}</x>");
    let mut reader = quick_xml::Reader::from_str(&wrapped);
    // Mismatched end tags are refused here, unlike the lenient reader the import path uses.
    reader.config_mut().check_end_names = true;
    let mut depth = 0usize;
    let mut closed = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(_)) if !closed => depth = depth.saturating_add(1),
            Ok(Event::End(_)) if !closed => {
                depth = depth.saturating_sub(1);
                closed = depth == 0;
            }
            Ok(Event::Eof) => return closed,
            // Content inside the math: text, escaped references, comments and empty elements.
            Ok(Event::Empty(_) | Event::Text(_) | Event::CData(_) | Event::Comment(_) | Event::GeneralRef(_)) if !closed => {}
            // Anything after the wrapper closed is a fragment that broke out of it; a declaration,
            // doctype or processing instruction never belongs inside math.
            Ok(_) | Err(_) => return false,
        }
    }
}

/// The OMML of an equation made of plain text: one run, the math an equation of typed text is in
/// Excel (a paragraph of `m:oMath` holding a single `m:r`). Control characters that XML cannot
/// carry are dropped, and the text is escaped, so the part always parses.
pub fn equation_omml(text: &str) -> String {
    let clean: String = text.chars().filter(|c| matches!(c, '\t' | '\n' | '\r') || ('\u{20}'..='\u{d7ff}').contains(c) || *c >= '\u{e000}').collect();
    format!(
        "<m:oMathPara><m:oMath><m:r><m:rPr><m:sty m:val=\"p\"/></m:rPr><m:t xml:space=\"preserve\">{}</m:t></m:r></m:oMath></m:oMathPara>",
        esc(&clean)
    )
}

/// Reads one DrawingML shape fragment that Excel 2007+ writes into a legacy `.xls` drawing
/// (`drs/shapexml.xml` inside a tertiary-opt record): an `xdr:anchor` holding one
/// `xdr:twoCellAnchor` and an `xdr:sp`. Returns the model shape (id 0; the caller assigns the
/// sheet's next object id).
pub(crate) fn read_xls_shape_fragment(xml: &str, sheet: &Sheet) -> Option<Shape> {
    let root = crate::xml::parse(xml.as_bytes()).ok()?;
    let anchor_el = root.child("twoCellAnchor")?;
    let anchor = read_anchor(anchor_el, sheet)?;
    read_shape(anchor_el.child("sp")?, anchor)
}

/// A shape's text formatting: vertical anchor (`a:bodyPr anchor`), the first paragraph's
/// alignment and direction (`a:pPr algn`, `rtl`) and the first run's properties (`a:rPr`: size,
/// bold, italic, fill, outline, `a:latin` typeface).
fn read_text_format(body: Option<&El>) -> ShapeFormat {
    let mut f = ShapeFormat::default();
    let Some(body) = body else { return f };
    f.anchor = body.child("bodyPr").and_then(|b| b.attr("anchor")).map(|a| match a {
        "ctr" => ShapeTextAnchor::Middle,
        "b" => ShapeTextAnchor::Bottom,
        _ => ShapeTextAnchor::Top,
    });
    let ppr = body.kids("p").next().and_then(|p| p.child("pPr"));
    f.align = ppr.and_then(|p| p.attr("algn")).map(|a| match a {
        "ctr" => ShapeTextAlign::Center,
        "r" => ShapeTextAlign::Right,
        _ => ShapeTextAlign::Left,
    });
    f.rtl = ppr.is_some_and(|p| p.flag("rtl", false));
    let rpr = body.kids("p").flat_map(|p| p.kids("r")).find_map(|r| r.child("rPr"));
    if let Some(r) = rpr {
        // `sz` is in hundredths of a point (100..=400000 in the schema).
        f.font_size = r.attr_f64("sz").filter(|v| v.is_finite()).map(|v| (v / 100.0).clamp(1.0, 4000.0) as f32);
        f.bold = r.flag("b", false);
        f.italic = r.flag("i", false);
        f.font = r.child("latin").and_then(|l| l.attr("typeface")).filter(|t| !t.is_empty() && !t.starts_with('+')).map(str::to_string);
        f.text_color = r.child("solidFill").and_then(dml_color);
        f.text_outline = r.child("ln").and_then(|l| {
            let c = l.child("solidFill").and_then(dml_color)?;
            // Width in EMU (12700 per point); Excel's default hairline is 0.75 pt.
            let w = l.attr_f64("w").filter(|w| w.is_finite()).map_or(0.75, |w| (w / 12700.0).clamp(0.0, 100.0)) as f32;
            Some((c, w))
        });
    }
    f
}

// ---------------------------------------------------------------- writing

/// The opening `<xdr:twoCellAnchor>` for a model anchor, carrying Excel's `editAs` when the mode
/// is not the default: omitted for "Move and size with cells", `editAs="oneCell"` for "Move but
/// don't size", `editAs="absolute"` for "Don't move or size". (Excel writes pictures this way;
/// the standalone `oneCellAnchor`/`absoluteAnchor` elements are read on import but not written.)
/// Returns the opening tag and the matching closing tag.
fn anchor_open(sheet: &Sheet, a: &Anchor) -> (String, &'static str) {
    let px = |v: f64| (clamp_px(v) * EMU_PER_PX).round() as i64;
    let col = a.cell.col.min(MAX_COLS - 1);
    let row = a.cell.row.min(MAX_ROWS - 1);
    let dx = clamp_px(a.dx as f64);
    let dy = clamp_px(a.dy as f64);
    let x1 = sheet.col_left(col) + dx;
    let y1 = sheet.row_top(row) + dy;
    let x2 = x1 + clamp_px(a.width as f64);
    let y2 = y1 + clamp_px(a.height as f64);
    let tc = sheet.col_at(x2);
    let tr = sheet.row_at(y2);
    let tdx = clamp_px(x2 - sheet.col_left(tc));
    let tdy = clamp_px(y2 - sheet.row_top(tr));
    let edit = match a.mode {
        AnchorMode::MoveAndSize => "",
        AnchorMode::MoveOnly => " editAs=\"oneCell\"",
        AnchorMode::Absolute => " editAs=\"absolute\"",
    };
    let open = format!(
        "<xdr:twoCellAnchor{edit}><xdr:from><xdr:col>{col}</xdr:col><xdr:colOff>{}</xdr:colOff><xdr:row>{row}</xdr:row><xdr:rowOff>{}</xdr:rowOff></xdr:from><xdr:to><xdr:col>{tc}</xdr:col><xdr:colOff>{}</xdr:colOff><xdr:row>{tr}</xdr:row><xdr:rowOff>{}</xdr:rowOff></xdr:to>",
        px(dx),
        px(dy),
        px(tdx),
        px(tdy)
    );
    (open, "</xdr:twoCellAnchor>")
}

/// A drawing object's alternative text (`xdr:cNvPr/@descr`), trimmed and capped; empty is `None`.
fn read_alt(cnv_pr: Option<&El>) -> Option<String> {
    let text = cnv_pr?.attr("descr")?.trim();
    (!text.is_empty()).then(|| text.chars().take(1024).collect())
}

/// The `descr` attribute for a drawing object, escaped, or nothing when there is no alt text.
fn alt_attr(alt: Option<&str>) -> String {
    match alt.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => format!(" descr=\"{}\"", esc_attr(&t.chars().take(1024).collect::<String>())),
        None => String::new(),
    }
}

fn xfrm(a: &Anchor, tag: &str) -> String {
    let e = |v: f32| ((v.max(0.0) as f64) * EMU_PER_PX).round() as i64;
    format!("<{tag}><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{}\" cy=\"{}\"/></{tag}>", e(a.width), e(a.height))
}

pub fn srgb(c: &Color, theme: &Theme) -> Option<String> {
    let [r, g, b] = c.resolve(theme)?;
    Some(format!("<a:srgbClr val=\"{r:02X}{g:02X}{b:02X}\"/>"))
}

/// One drawing object to write.
pub enum Obj<'a> {
    Image(&'a Image, String),
    Chart(&'a gridcraft_model::Chart, String),
    /// A chart of a kind GridCraft does not model: its frame only (the part is written as it was).
    RawChart(&'a gridcraft_model::KeptChart, String),
    Shape(&'a Shape),
    /// An equation: a text-box shape whose paragraph is the `a14:m` math extension.
    Equation(&'a Equation),
    /// A slicer or timeline view's frame: the part is written from the model, the frame places it.
    Widget {
        name: &'a str,
        anchor: &'a Anchor,
        timeline: bool,
    },
}

/// The drawing part for a sheet's objects (relationship ids already assigned).
pub fn write_drawing(sheet: &Sheet, theme: &Theme, objs: &[Obj<'_>]) -> String {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<xdr:wsDr xmlns:xdr=\"http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"",
    );
    // An equation's math lives in the DrawingML 2010 extension (`a14`) and in the math namespace
    // (`m`); both are declared on the root, once, when the part holds one.
    if objs.iter().any(|o| matches!(o, Obj::Equation(_))) {
        let _ = write!(s, " xmlns:a14=\"{NS_A14}\" xmlns:m=\"{NS_MATH}\"");
    }
    s.push('>');
    for (i, o) in objs.iter().enumerate() {
        let id = i + 2;
        let close_tag;
        match o {
            Obj::Image(img, rid) => {
                let (open, close) = anchor_open(sheet, &img.anchor);
                close_tag = close;
                s.push_str(&open);
                let _ = write!(
                    s,
                    "<xdr:pic><xdr:nvPicPr><xdr:cNvPr id=\"{id}\" name=\"Picture {}\" descr=\"{}\"/><xdr:cNvPicPr><a:picLocks noChangeAspect=\"1\"/></xdr:cNvPicPr></xdr:nvPicPr><xdr:blipFill><a:blip r:embed=\"{rid}\"/><a:stretch><a:fillRect/></a:stretch></xdr:blipFill><xdr:spPr>{}<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></xdr:spPr></xdr:pic>",
                    id - 1,
                    esc_attr(&img.alt),
                    xfrm(&img.anchor, "a:xfrm")
                );
            }
            Obj::RawChart(ch, rid) if ch.chartex => {
                let (open, close) = anchor_open(sheet, &ch.anchor);
                close_tag = close;
                s.push_str(&open);
                let _ = write!(
                    s,
                    "<mc:AlternateContent xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\"><mc:Choice xmlns:cx1=\"http://schemas.microsoft.com/office/drawing/2015/9/8/chartex\" Requires=\"cx1\"><xdr:graphicFrame macro=\"\"><xdr:nvGraphicFramePr><xdr:cNvPr id=\"{id}\" name=\"Chart {}\"{}/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr>{}<a:graphic><a:graphicData uri=\"http://schemas.microsoft.com/office/drawing/2014/chartex\"><cx:chart xmlns:cx=\"http://schemas.microsoft.com/office/drawing/2014/chartex\" r:id=\"{rid}\"/></a:graphicData></a:graphic></xdr:graphicFrame></mc:Choice><mc:Fallback><xdr:sp macro=\"\" textlink=\"\"><xdr:nvSpPr><xdr:cNvPr id=\"0\" name=\"\"/><xdr:cNvSpPr><a:spLocks noTextEdit=\"1\"/></xdr:cNvSpPr></xdr:nvSpPr><xdr:spPr>{}<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></xdr:spPr><xdr:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang=\"en-US\"/><a:t>This chart type can't be shown by this application.</a:t></a:r></a:p></xdr:txBody></xdr:sp></mc:Fallback></mc:AlternateContent>",
                    id - 1,
                    alt_attr(ch.alt.as_deref()),
                    xfrm(&ch.anchor, "xdr:xfrm"),
                    xfrm(&ch.anchor, "a:xfrm")
                );
            }
            Obj::RawChart(ch, rid) => {
                let (open, close) = anchor_open(sheet, &ch.anchor);
                close_tag = close;
                s.push_str(&open);
                let _ = write!(
                    s,
                    "<xdr:graphicFrame macro=\"\"><xdr:nvGraphicFramePr><xdr:cNvPr id=\"{id}\" name=\"Chart {}\"{}/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr>{}<a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"><c:chart r:id=\"{rid}\"/></a:graphicData></a:graphic></xdr:graphicFrame>",
                    id - 1,
                    alt_attr(ch.alt.as_deref()),
                    xfrm(&ch.anchor, "xdr:xfrm")
                );
            }
            Obj::Chart(ch, rid) if ch.kind.is_chartex() => {
                // Chartex frames come with a fallback for readers that don't know them.
                let (open, close) = anchor_open(sheet, &ch.anchor);
                close_tag = close;
                s.push_str(&open);
                let (prefix, ns) = crate::chartex::requires(ch.kind);
                let _ = write!(
                    s,
                    "<mc:AlternateContent xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\"><mc:Choice xmlns:{prefix}=\"{ns}\" Requires=\"{prefix}\"><xdr:graphicFrame macro=\"\"><xdr:nvGraphicFramePr><xdr:cNvPr id=\"{id}\" name=\"Chart {}\"{}/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr>{}<a:graphic><a:graphicData uri=\"http://schemas.microsoft.com/office/drawing/2014/chartex\"><cx:chart xmlns:cx=\"http://schemas.microsoft.com/office/drawing/2014/chartex\" r:id=\"{rid}\"/></a:graphicData></a:graphic></xdr:graphicFrame></mc:Choice><mc:Fallback><xdr:sp macro=\"\" textlink=\"\"><xdr:nvSpPr><xdr:cNvPr id=\"0\" name=\"\"/><xdr:cNvSpPr><a:spLocks noTextEdit=\"1\"/></xdr:cNvSpPr></xdr:nvSpPr><xdr:spPr>{}<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></xdr:spPr><xdr:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang=\"en-US\"/><a:t>This chart type can't be shown by this application.</a:t></a:r></a:p></xdr:txBody></xdr:sp></mc:Fallback></mc:AlternateContent>",
                    id - 1,
                    alt_attr(ch.alt.as_deref()),
                    xfrm(&ch.anchor, "xdr:xfrm"),
                    xfrm(&ch.anchor, "a:xfrm")
                );
            }
            Obj::Chart(ch, rid) => {
                let (open, close) = anchor_open(sheet, &ch.anchor);
                close_tag = close;
                s.push_str(&open);
                let _ = write!(
                    s,
                    "<xdr:graphicFrame macro=\"\"><xdr:nvGraphicFramePr><xdr:cNvPr id=\"{id}\" name=\"Chart {}\"{}/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr>{}<a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"><c:chart r:id=\"{rid}\"/></a:graphicData></a:graphic></xdr:graphicFrame>",
                    id - 1,
                    alt_attr(ch.alt.as_deref()),
                    xfrm(&ch.anchor, "xdr:xfrm")
                );
            }
            Obj::Widget { name, anchor, timeline } => {
                let (open, close) = anchor_open(sheet, anchor);
                close_tag = close;
                s.push_str(&open);
                let (uri, tag) =
                    if *timeline { (crate::slicer::NS_DRAWING_TIMELINE, "timeslicer") } else { (crate::slicer::NS_DRAWING_SLICER, "slicer") };
                // MS-XLSX 5.2/5.7: the view is named, not related; the frame's transform is the
                // anchor's (Excel writes a zero ext for a frame in a twoCellAnchor).
                let _ = write!(
                    s,
                    "<xdr:graphicFrame macro=\"\"><xdr:nvGraphicFramePr><xdr:cNvPr id=\"{id}\" name=\"{}\"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/></xdr:xfrm><a:graphic><a:graphicData uri=\"{uri}\"><sle:{tag} xmlns:sle=\"{uri}\" name=\"{}\"/></a:graphicData></a:graphic></xdr:graphicFrame>",
                    esc_attr(name),
                    esc_attr(name)
                );
            }
            Obj::Equation(eq) => {
                let (open, close) = anchor_open(sheet, &eq.anchor);
                close_tag = close;
                s.push_str(&open);
                // A text box whose paragraph is the math: the shape Excel writes for an equation.
                let _ = write!(
                    s,
                    "<xdr:sp macro=\"\" textlink=\"\"><xdr:nvSpPr><xdr:cNvPr id=\"{id}\" name=\"Equation {}\"{}/><xdr:cNvSpPr txBox=\"1\"/></xdr:nvSpPr><xdr:spPr>{}<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom>",
                    id - 1,
                    alt_attr(eq.alt.as_deref()),
                    xfrm(&eq.anchor, "a:xfrm")
                );
                if let Some(c) = srgb(&eq.fill, theme) {
                    let _ = write!(s, "<a:solidFill>{c}</a:solidFill>");
                }
                // An equation is outlined only when the model gives it a colour: `Color::Auto` is
                // Excel's no outline, which is explicit in the file (`a:ln/a:noFill`).
                match srgb(&eq.line, theme) {
                    Some(c) => {
                        let _ = write!(s, "<a:ln w=\"9525\"><a:solidFill>{c}</a:solidFill></a:ln>");
                    }
                    None => s.push_str("<a:ln><a:noFill/></a:ln>"),
                }
                s.push_str("</xdr:spPr>");
                // The file's own math goes back byte for byte; anything else (a new equation, or one
                // whose text was edited) is written as the plain runs its text is.
                let omml = eq.omml.as_deref().filter(|f| valid_omml(f)).map(str::to_string).unwrap_or_else(|| equation_omml(&eq.source));
                let _ = write!(
                    s,
                    "<xdr:txBody><a:bodyPr wrap=\"square\" lIns=\"91440\" tIns=\"45720\" rIns=\"91440\" bIns=\"45720\" anchor=\"ctr\"/><a:lstStyle/><a:p><a:pPr algn=\"ctr\"/><a14:m>{omml}</a14:m><a:endParaRPr lang=\"en-US\"/></a:p></xdr:txBody></xdr:sp>"
                );
            }
            Obj::Shape(sh) => {
                let (open, close) = anchor_open(sheet, &sh.anchor);
                close_tag = close;
                s.push_str(&open);
                if let Some(xml) = ink::write(sh, theme, id) {
                    s.push_str(&xml);
                    s.push_str("<xdr:clientData/>");
                    s.push_str(close_tag);
                    continue;
                }
                let (prst, tx) = match sh.kind {
                    ShapeKind::Rectangle => ("rect", false),
                    ShapeKind::RoundedRectangle => ("roundRect", false),
                    ShapeKind::Ellipse => ("ellipse", false),
                    ShapeKind::Triangle => ("triangle", false),
                    ShapeKind::Line | ShapeKind::Arrow => ("line", false),
                    ShapeKind::TextBox => ("rect", true),
                    ShapeKind::Icon | ShapeKind::Ink => ("rect", false),
                };
                let _ = write!(
                    s,
                    "<xdr:sp macro=\"\" textlink=\"\"><xdr:nvSpPr><xdr:cNvPr id=\"{id}\" name=\"Shape {}\"{}/><xdr:cNvSpPr{}/></xdr:nvSpPr><xdr:spPr>{}<a:prstGeom prst=\"{prst}\"><a:avLst/></a:prstGeom>",
                    id - 1,
                    alt_attr(sh.alt.as_deref()),
                    if tx { " txBox=\"1\"" } else { "" },
                    xfrm(&sh.anchor, "a:xfrm")
                );
                if let Some(c) = srgb(&sh.fill, theme) {
                    let _ = write!(s, "<a:solidFill>{c}</a:solidFill>");
                }
                let line_c = srgb(&sh.line, theme);
                if sh.format.no_line {
                    s.push_str("<a:ln><a:noFill/></a:ln>");
                } else if line_c.is_some() || sh.kind == ShapeKind::Arrow {
                    s.push_str("<a:ln w=\"9525\">");
                    match line_c {
                        Some(c) => {
                            let _ = write!(s, "<a:solidFill>{c}</a:solidFill>");
                        }
                        None => s.push_str("<a:solidFill><a:srgbClr val=\"000000\"/></a:solidFill>"),
                    }
                    if sh.kind == ShapeKind::Arrow {
                        s.push_str("<a:tailEnd type=\"triangle\"/>");
                    }
                    s.push_str("</a:ln>");
                }
                s.push_str("</xdr:spPr>");
                if !sh.text.is_empty() {
                    let f = &sh.format;
                    let anchor = match f.anchor {
                        Some(ShapeTextAnchor::Top) => " anchor=\"t\"",
                        Some(ShapeTextAnchor::Middle) => " anchor=\"ctr\"",
                        Some(ShapeTextAnchor::Bottom) => " anchor=\"b\"",
                        None if sh.kind == ShapeKind::TextBox => " anchor=\"t\"",
                        None => "",
                    };
                    let _ = write!(s, "<xdr:txBody><a:bodyPr wrap=\"square\" rtlCol=\"0\"{anchor}/><a:lstStyle/>");
                    let algn = match f.align {
                        None => "",
                        Some(ShapeTextAlign::Left) => " algn=\"l\"",
                        Some(ShapeTextAlign::Center) => " algn=\"ctr\"",
                        Some(ShapeTextAlign::Right) => " algn=\"r\"",
                    };
                    let rtl = if f.rtl { " rtl=\"1\"" } else { "" };
                    let ppr = if algn.is_empty() && rtl.is_empty() { String::new() } else { format!("<a:pPr{algn}{rtl}/>") };
                    let (rpr_attrs, rpr_body) = run_props(f, theme);
                    for line in sh.text.split('\n') {
                        if line.is_empty() {
                            let _ = write!(s, "<a:p>{ppr}<a:endParaRPr lang=\"en-US\"{rpr_attrs}/></a:p>");
                        } else if rpr_body.is_empty() {
                            let _ = write!(s, "<a:p>{ppr}<a:r><a:rPr lang=\"en-US\"{rpr_attrs}/><a:t>{}</a:t></a:r></a:p>", esc(line));
                        } else {
                            let _ =
                                write!(s, "<a:p>{ppr}<a:r><a:rPr lang=\"en-US\"{rpr_attrs}>{rpr_body}</a:rPr><a:t>{}</a:t></a:r></a:p>", esc(line));
                        }
                    }
                    s.push_str("</xdr:txBody>");
                }
                s.push_str("</xdr:sp>");
            }
        }
        s.push_str("<xdr:clientData/>");
        s.push_str(close_tag);
    }
    s.push_str("</xdr:wsDr>");
    s
}

/// A text run's `a:rPr` attributes (size, bold, italic) and children in schema order (outline,
/// fill, typeface).
fn run_props(f: &ShapeFormat, theme: &Theme) -> (String, String) {
    let mut attrs = String::new();
    if let Some(sz) = f.font_size.filter(|v| v.is_finite()) {
        let _ = write!(attrs, " sz=\"{}\"", (sz.clamp(1.0, 4000.0) * 100.0).round() as u32);
    }
    if f.bold {
        attrs.push_str(" b=\"1\"");
    }
    if f.italic {
        attrs.push_str(" i=\"1\"");
    }
    let mut body = String::new();
    if let Some((c, w)) = &f.text_outline
        && let Some(c) = srgb(c, theme)
    {
        let emu = (w.clamp(0.0, 100.0) * 12700.0).round() as u32;
        let _ = write!(body, "<a:ln w=\"{emu}\"><a:solidFill>{c}</a:solidFill></a:ln>");
    }
    if let Some(c) = f.text_color.as_ref().and_then(|c| srgb(c, theme)) {
        let _ = write!(body, "<a:solidFill>{c}</a:solidFill>");
    }
    if let Some(font) = &f.font {
        let _ = write!(body, "<a:latin typeface=\"{}\"/>", esc(font));
    }
    (attrs, body)
}

/// File extension for an image MIME type.
pub fn image_ext(mime: &str, data: &[u8]) -> &'static str {
    match mime.to_ascii_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpeg",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        "image/svg+xml" => "svg",
        "image/x-emf" => "emf",
        "image/x-wmf" => "wmf",
        "image/webp" => "webp",
        _ => {
            if data.starts_with(b"\x89PNG") {
                "png"
            } else if data.starts_with(&[0xFF, 0xD8]) {
                "jpeg"
            } else if data.starts_with(b"GIF8") {
                "gif"
            } else {
                "png"
            }
        }
    }
}

pub fn image_content_type(ext: &str) -> &'static str {
    match ext {
        "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tiff" => "image/tiff",
        "svg" => "image/svg+xml",
        "emf" => "image/x-emf",
        "wmf" => "image/x-wmf",
        "webp" => "image/webp",
        _ => "image/png",
    }
}

#[cfg(test)]
mod tests {
    use crate::tests::minimal;
    use crate::{read_xlsx, write_xlsx};

    /// The math a file writes for "1/2": an OMML math paragraph inside the `a14:m` extension.
    const FRACTION: &str =
        "<m:oMathPara><m:oMath><m:f><m:num><m:r><m:t>1</m:t></m:r></m:num><m:den><m:r><m:t>2</m:t></m:r></m:den></m:f></m:oMath></m:oMathPara>";

    /// A drawing part holding one equation (a shape whose text body is math) and one plain text box,
    /// written by hand in code: an equation is invisible to GridCraft only when nothing reads it.
    fn drawing() -> String {
        format!(
            r##"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:a14="http://schemas.microsoft.com/office/drawing/2010/main" xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math">
<xdr:twoCellAnchor><xdr:from><xdr:col>1</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>3</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>3</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to>
<xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="2" name="Equation 1" descr="a half"/><xdr:cNvSpPr txBox="1"/></xdr:nvSpPr>
<xdr:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1828800" cy="571500"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:ln><a:noFill/></a:ln></xdr:spPr>
<xdr:txBody><a:bodyPr wrap="square" anchor="ctr"/><a:lstStyle/>
<a:p><a:pPr algn="ctr"/><a14:m>{FRACTION}</a14:m><a:endParaRPr lang="en-US"/></a:p>
</xdr:txBody></xdr:sp><xdr:clientData/></xdr:twoCellAnchor>
<xdr:twoCellAnchor><xdr:from><xdr:col>5</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>7</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>3</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to>
<xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="3" name="TextBox 2"/><xdr:cNvSpPr txBox="1"/></xdr:nvSpPr>
<xdr:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1828800" cy="571500"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></xdr:spPr>
<xdr:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>Summary</a:t></a:r></a:p></xdr:txBody></xdr:sp><xdr:clientData/></xdr:twoCellAnchor>
</xdr:wsDr>"##
        )
    }

    fn with_drawing(drawing: &str) -> Vec<u8> {
        let rels = r##"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing" Target="../drawings/drawing1.xml"/></Relationships>"##;
        minimal(
            r##"<sheetData/><drawing r:id="rId1"/>"##,
            &[("xl/worksheets/_rels/sheet1.xml.rels", rels), ("xl/drawings/drawing1.xml", drawing)],
            "",
            "",
        )
    }

    #[test]
    fn a_shape_that_carries_math_opens_as_an_equation() {
        let wb = read_xlsx(&with_drawing(&drawing())).unwrap().0;
        let sheet = wb.sheet(0).unwrap();
        assert_eq!(sheet.equations.len(), 1, "the math is an equation");
        assert_eq!(sheet.shapes.len(), 1, "the shape with no math stays a shape");
        let eq = &sheet.equations[0];
        assert_eq!(eq.source, "12", "the linear form is the runs, flattened");
        assert_eq!(eq.omml.as_deref(), Some(FRACTION), "the file's math is kept verbatim");
        assert_eq!(eq.alt.as_deref(), Some("a half"), "alt text comes with it");
        assert_eq!(eq.line, gridcraft_model::Color::Auto, "a:ln/a:noFill is no outline");
        assert_eq!(eq.anchor.width, 128.0, "the box is the cells it spans: two default columns wide");
    }

    #[test]
    fn an_equation_is_written_back_with_its_own_markup() {
        let wb = read_xlsx(&with_drawing(&drawing())).unwrap().0;
        let bytes = write_xlsx(&wb).unwrap();
        crate::testing::check_package(&bytes).expect("the package validates");
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.clone())).unwrap();
        let mut part = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("xl/drawings/drawing1.xml").unwrap(), &mut part).unwrap();
        assert!(part.contains("<a14:m>"), "{part}");
        assert!(part.contains(FRACTION), "the math goes back byte for byte: {part}");
        // The part parses: a prefix the drawing never declared would make Excel repair the file.
        crate::xml::parse(part.as_bytes()).expect("the written drawing is XML");
        // And reopening reads the same math back.
        let again = read_xlsx(&bytes).unwrap().0;
        let eq = &again.sheet(0).unwrap().equations[0];
        assert_eq!(eq.omml.as_deref(), Some(FRACTION));
        assert_eq!(eq.source, "12");
    }

    #[test]
    fn a_plain_run_equation_is_written_as_valid_omml() {
        let omml = super::equation_omml("a<b & c");
        assert_eq!(
            omml,
            r##"<m:oMathPara><m:oMath><m:r><m:rPr><m:sty m:val="p"/></m:rPr><m:t xml:space="preserve">a&lt;b &amp; c</m:t></m:r></m:oMath></m:oMathPara>"##
        );
        // Namespace-declared, so the part holding it parses wherever it is written.
        let wrapped = format!(
            r##"<a14:m xmlns:a14="http://schemas.microsoft.com/office/drawing/2010/main" xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math">{omml}</a14:m>"##
        );
        crate::xml::parse(wrapped.as_bytes()).expect("generated markup is XML");
        // Control characters XML cannot carry are dropped rather than written.
        assert!(!super::equation_omml("ab").contains(''));
    }

    #[test]
    fn a_kept_fragment_that_is_not_xml_is_refused() {
        assert!(super::valid_omml(FRACTION));
        // Escaped references are what an equation's text is: they are content, not structure.
        assert!(super::valid_omml("<m:oMath><m:r><m:t>a &lt; b</m:t></m:r></m:oMath>"));
        assert!(!super::valid_omml("<m:oMath>"), "an unclosed element is refused");
        assert!(!super::valid_omml("</a14:m><xdr:sp>"), "a fragment cannot close what holds it");
        assert!(!super::valid_omml("<?xml version=\"1.0\"?><m:oMath/>"));
    }
}
