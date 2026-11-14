//! Floating objects on the sheet: charts (rendered by `gridcraft-chart` into primitives that
//! we paint with egui), pictures and shapes.

use egui::epaint::{PathShape, PathStroke};
use egui::{Align2, Color32, CursorIcon, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, pos2, vec2};
use gridcraft_chart::{HAlign, Prim, VAlign};
use gridcraft_engine::core::CellRef;
use gridcraft_engine::model::{Anchor, ShapeKind, Sheet, Workbook};

use crate::SheetApp;
use crate::grid::Geo;
use crate::theme;

fn rgba(c: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}

struct EguiMeasure<'a> {
    painter: &'a Painter,
    z: f32,
    /// The app's text engine, so a label's measured width is the width it is drawn at (the shaped
    /// engine reorders and joins where epaint's own layout would not).
    text: Option<&'a crate::text::GridText>,
}

impl gridcraft_chart::Measure for EguiMeasure<'_> {
    fn text_width(&self, text: &str, size: f32, bold: bool) -> f32 {
        let font = if bold { theme::ui_bold(size * self.z) } else { theme::ui_font(size * self.z) };
        let w = match self.text {
            Some(engine) => engine.measure(self.painter.ctx(), &font, text, crate::text::base_direction(text, false)),
            None => self.painter.layout_no_wrap(text.to_string(), font, Color32::BLACK).size().x,
        };
        w / self.z.max(0.01)
    }
}

/// Paints `chart` (not necessarily on the sheet: the Recommended Charts preview) scaled to fit
/// `rect`.
pub fn paint_chart(
    p: &Painter,
    rect: Rect,
    wb: &Workbook,
    sheet: usize,
    chart: &gridcraft_engine::model::Chart,
    text: Option<&crate::text::GridText>,
) {
    let (w, h) = (chart.anchor.width.max(1.0), chart.anchor.height.max(1.0));
    let z = (rect.width() / w).min(rect.height() / h);
    if !(z.is_finite() && z > 0.0) {
        return;
    }
    let origin = rect.center() - vec2(w * z, h * z) / 2.0;
    let data = gridcraft_chart::resolve(wb, sheet, chart);
    let m = EguiMeasure { painter: p, z, text };
    let prims = gridcraft_chart::render(chart, &data, w, h, &m);
    paint_prims(&p.with_clip_rect(rect.intersect(p.clip_rect())), origin, &prims, z, text);
}

/// What a click at `at` (screen coordinates) inside a chart's on-screen `rect` lands on, in the
/// Format pane's vocabulary. The chart is laid out at its anchor size and painted scaled, so the
/// click maps back by the same scale; the regions come from the same layout that painted it
/// (`hits`, kept across frames by [`crate::grid::PaintCache`]) or, for a chart that was not
/// painted this frame, from a layout made here.
#[allow(clippy::too_many_arguments)]
pub fn element_at(
    p: &Painter,
    wb: &Workbook,
    sheet: usize,
    chart: &gridcraft_engine::model::Chart,
    rect: Rect,
    at: Pos2,
    text: Option<&crate::text::GridText>,
    hits: Option<&[gridcraft_chart::HitRegion]>,
) -> Option<gridcraft_chart::Hit> {
    if !rect.contains(at) {
        return None;
    }
    let (w, h) = (chart.anchor.width.max(1.0), chart.anchor.height.max(1.0));
    // The chart is laid out at its own size and scaled to fit the rect, centered: on a worksheet
    // that is the zoom factor exactly, and on a chart sheet the fit that fills the window.
    let z = (rect.width() / w).min(rect.height() / h);
    if !(w.is_finite() && h.is_finite() && z.is_finite() && z > 0.0) {
        return None;
    }
    let origin = rect.center() - vec2(w * z, h * z) / 2.0;
    let (x, y) = ((at.x - origin.x) / z, (at.y - origin.y) / z);
    if let Some(hits) = hits {
        return gridcraft_chart::hit_at(hits, x, y);
    }
    let data = gridcraft_chart::resolve(wb, sheet, chart);
    let m = EguiMeasure { painter: p, z, text };
    let laid_out = gridcraft_chart::render_hits(chart, &data, w, h, &m);
    gridcraft_chart::hit_at(&laid_out, x, y)
}

/// Paints chart primitives at `origin`, scaled by `z`.
pub fn paint_prims(p: &Painter, origin: Pos2, prims: &[Prim], z: f32, text: Option<&crate::text::GridText>) {
    let pt = |x: f32, y: f32| pos2(origin.x + x * z, origin.y + y * z);
    for prim in prims {
        match prim {
            Prim::Rect { x, y, w, h, fill, stroke, radius } => {
                let r = Rect::from_min_size(pt(*x, *y), vec2(w * z, h * z));
                if let Some(f) = fill {
                    p.rect_filled(r, radius * z, rgba(*f));
                }
                if let Some((c, wd)) = stroke {
                    p.rect_stroke(r, radius * z, Stroke::new(wd * z.max(0.5), rgba(*c)), StrokeKind::Inside);
                }
            }
            Prim::Line { pts, color, width, dash } => {
                let v: Vec<Pos2> = pts.iter().map(|q| pt(q[0], q[1])).collect();
                if v.len() < 2 {
                    continue;
                }
                if *dash {
                    p.extend(egui::Shape::dashed_line(&v, Stroke::new(width * z.max(0.5), rgba(*color)), 4.0 * z, 3.0 * z));
                } else {
                    p.add(PathShape::line(v, PathStroke::new(width * z.max(0.5), rgba(*color))));
                }
            }
            Prim::Polygon { pts, fill, stroke } => {
                let v: Vec<Pos2> = pts.iter().map(|q| pt(q[0], q[1])).collect();
                if v.len() < 3 {
                    continue;
                }
                // Polygons may be concave (areas): triangulate as a fan from the first point
                // when convex, else draw through a mesh of the closed path.
                let shape = PathShape { points: v.clone(), closed: true, fill: rgba(*fill), stroke: PathStroke::NONE };
                if is_convex(&v) {
                    p.add(shape);
                } else {
                    p.add(egui::Shape::mesh(area_mesh(&v, rgba(*fill))));
                }
                if let Some((c, w)) = stroke {
                    p.add(PathShape::closed_line(v, PathStroke::new(w * z.max(0.5), rgba(*c))));
                }
            }
            Prim::Wedge { cx, cy, r_outer, r_inner, a0, a1, fill, stroke } => {
                let c = pt(*cx, *cy);
                let steps = (((a1 - a0).abs() / 0.05).ceil() as usize).clamp(2, 400);
                let at = |a: f32, r: f32| pos2(c.x + r * z * a.sin(), c.y - r * z * a.cos());
                let mut mesh = egui::Mesh::default();
                let col = rgba(*fill);
                for i in 0..=steps {
                    let a = a0 + (a1 - a0) * i as f32 / steps as f32;
                    mesh.colored_vertex(at(a, *r_outer), col);
                    mesh.colored_vertex(if *r_inner > 0.0 { at(a, *r_inner) } else { c }, col);
                }
                for i in 0..steps as u32 {
                    let b = i * 2;
                    mesh.add_triangle(b, b + 1, b + 2);
                    mesh.add_triangle(b + 1, b + 3, b + 2);
                }
                p.add(egui::Shape::mesh(mesh));
                if let Some((sc, w)) = stroke {
                    let mut outline: Vec<Pos2> = (0..=steps).map(|i| at(a0 + (a1 - a0) * i as f32 / steps as f32, *r_outer)).collect();
                    if *r_inner > 0.0 {
                        outline.extend((0..=steps).rev().map(|i| at(a0 + (a1 - a0) * i as f32 / steps as f32, *r_inner)));
                    } else {
                        outline.push(c);
                    }
                    p.add(PathShape::closed_line(outline, PathStroke::new(w * z.max(0.5), rgba(*sc))));
                }
            }
            Prim::Circle { cx, cy, r, fill, stroke } => {
                if let Some(f) = fill {
                    p.circle_filled(pt(*cx, *cy), r * z, rgba(*f));
                }
                if let Some((c, w)) = stroke {
                    p.circle_stroke(pt(*cx, *cy), r * z, Stroke::new(w * z.max(0.5), rgba(*c)));
                }
            }
            Prim::Text { x, y, text: label, size, color, bold, align, valign, rotation } => {
                let font: FontId = if *bold { theme::ui_bold(size * z) } else { theme::ui_font(size * z) };
                // A label the bidi/joining/mark engines can change is shaped like cell text; the
                // same galley is what the layout measured above.
                let galley = match text {
                    Some(engine) => engine.line(p.ctx(), &font, label, crate::text::base_direction(label, false), rgba(*color)).galley,
                    None => p.layout_no_wrap(label.clone(), font, rgba(*color)),
                };
                let sz = galley.size();
                let ax = match align {
                    HAlign::Left => 0.0,
                    HAlign::Center => 0.5,
                    HAlign::Right => 1.0,
                };
                let ay = match valign {
                    VAlign::Top => 0.0,
                    VAlign::Middle => 0.5,
                    VAlign::Bottom => 1.0,
                };
                let anchor = pt(*x, *y);
                if *rotation == 0.0 {
                    p.galley(pos2(anchor.x - sz.x * ax, anchor.y - sz.y * ay), galley, rgba(*color));
                } else {
                    // Rotate about the anchor: offset the galley origin by the rotated alignment.
                    let (s, c) = rotation.sin_cos();
                    let off = vec2(-sz.x * ax, -sz.y * ay);
                    let rot = vec2(off.x * c - off.y * s, off.x * s + off.y * c);
                    let mut shape = egui::epaint::TextShape::new(anchor + rot, galley, rgba(*color));
                    shape.angle = *rotation;
                    p.add(shape);
                }
            }
        }
    }
}

fn is_convex(v: &[Pos2]) -> bool {
    let n = v.len();
    let mut sign = 0.0f32;
    for i in 0..n {
        let a = v[i];
        let b = v[(i + 1) % n];
        let c = v[(i + 2) % n];
        let cross = (b.x - a.x) * (c.y - b.y) - (b.y - a.y) * (c.x - b.x);
        if cross.abs() > 1e-3 {
            if sign == 0.0 {
                sign = cross.signum();
            } else if cross.signum() != sign {
                return false;
            }
        }
    }
    true
}

/// Area charts produce "mountain" polygons whose bottom edge is a straight baseline: fill them
/// as vertical strips (exact for such shapes, acceptable for others).
fn area_mesh(v: &[Pos2], col: Color32) -> egui::Mesh {
    let mut mesh = egui::Mesh::default();
    let n = v.len();
    if n < 3 {
        return mesh;
    }
    let half = n / 2;
    for i in 0..half.saturating_sub(1) {
        let a = v[i];
        let b = v[i + 1];
        let c = v[n - 2 - i];
        let d = v[n - 1 - i];
        let base = mesh.vertices.len() as u32;
        for q in [a, b, c, d] {
            mesh.colored_vertex(q, col);
        }
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    }
    mesh
}

/// A shape's text with its formatting: size, bold/italic, typeface, colour, outline, horizontal
/// alignment and vertical anchor. Shapes other than text boxes default to white text centred in
/// the shape; text boxes to black text at the top left.
fn paint_shape_text(p: &Painter, wb: &Workbook, sp: &gridcraft_engine::model::Shape, r: Rect, z: f32, text: Option<&crate::text::GridText>) {
    paint_box_text(p, wb, &sp.text, sp.kind == ShapeKind::TextBox, &sp.format, r, z, text);
}

/// The text of a box — a shape's or an equation's — as [`paint_shape_text`] draws it.
#[allow(clippy::too_many_arguments)]
fn paint_box_text(
    p: &Painter,
    wb: &Workbook,
    body: &str,
    text_box: bool,
    f: &gridcraft_engine::model::ShapeFormat,
    r: Rect,
    z: f32,
    text: Option<&crate::text::GridText>,
) {
    use gridcraft_engine::model::{ShapeTextAlign, ShapeTextAnchor};
    let align = f.align.unwrap_or(if text_box { ShapeTextAlign::Left } else { ShapeTextAlign::Center });
    let anchor = f.anchor.unwrap_or(if text_box { ShapeTextAnchor::Top } else { ShapeTextAnchor::Middle });
    let default_color = if text_box { Color32::BLACK } else { Color32::WHITE };
    let color = f.text_color.and_then(|c| c.resolve(&wb.theme)).map(theme::color32).unwrap_or(default_color);
    // Default 14 px (as before); a set size is in points.
    let size = f.font_size.map_or(14.0, |pt| pt * 96.0 / 72.0).clamp(1.0, 2000.0) * z;
    let family = theme::cell_family(f.font.as_deref().unwrap_or(""), f.bold, f.italic);
    let inset = 4.0;
    let width = (r.width() - 2.0 * inset).max(1.0);
    let font = FontId::new(size, family);
    let clip = p.with_clip_rect(r.intersect(p.clip_rect()));
    // The outline is the text drawn in the outline colour, offset in eight directions by the
    // outline width, under the filled text (egui has no stroked text).
    let halo = f.text_outline.and_then(|(c, w)| c.resolve(&wb.theme).map(|oc| (theme::color32(oc), w)));
    let paint_line = |line: &crate::text::Line, x: f32, y: f32| {
        if let Some((oc, w)) = halo {
            let d = (w * 96.0 / 72.0 * z / 2.0).clamp(0.5, 20.0);
            for (dx, dy) in [(-1.0, -1.0), (0.0, -1.0), (1.0, -1.0), (-1.0, 0.0), (1.0, 0.0), (-1.0, 1.0), (0.0, 1.0), (1.0, 1.0)] {
                clip.galley_with_override_text_color(pos2(x + dx * d, y + dy * d), line.galley.clone(), oc);
            }
        }
        clip.galley(pos2(x, y), line.galley.clone(), color);
    };
    // Wrapped shape text is shaped the way a wrapped cell is (its bidi runs, joining and marks
    // resolved per line); printable ASCII keeps the unshaped layout below, bit for bit.
    if let Some(engine) = text {
        let direction = crate::text::base_direction(body, false);
        if let Some(lines) = engine.wrap(p.ctx(), &font, body, direction, color, width) {
            let total: f32 = lines.iter().map(|l| l.height()).sum();
            let mut y = match anchor {
                ShapeTextAnchor::Top => r.top() + inset,
                ShapeTextAnchor::Middle => r.center().y - total / 2.0,
                ShapeTextAnchor::Bottom => r.bottom() - inset - total,
            };
            for line in &lines {
                let lw = line.galley.size().x;
                let x = match align {
                    ShapeTextAlign::Left => r.left() + inset,
                    ShapeTextAlign::Center => r.center().x - lw / 2.0,
                    ShapeTextAlign::Right => r.right() - inset - lw,
                };
                paint_line(line, x, y);
                y += line.height();
            }
            return;
        }
    }
    let mut job = egui::text::LayoutJob::simple(body.to_string(), font, color, width);
    job.halign = match align {
        ShapeTextAlign::Left => egui::Align::LEFT,
        ShapeTextAlign::Center => egui::Align::Center,
        ShapeTextAlign::Right => egui::Align::RIGHT,
    };
    let g = p.layout_job(job);
    let x = match align {
        ShapeTextAlign::Left => r.left() + inset,
        ShapeTextAlign::Center => r.center().x,
        ShapeTextAlign::Right => r.right() - inset,
    };
    let y = match anchor {
        ShapeTextAnchor::Top => r.top() + inset,
        ShapeTextAnchor::Middle => r.center().y - g.size().y / 2.0,
        ShapeTextAnchor::Bottom => r.bottom() - inset - g.size().y,
    };
    let pos = pos2(x, y);
    if let Some((oc, w)) = halo {
        let d = (w * 96.0 / 72.0 * z / 2.0).clamp(0.5, 20.0);
        for (dx, dy) in [(-1.0, -1.0), (0.0, -1.0), (1.0, -1.0), (-1.0, 0.0), (1.0, 0.0), (-1.0, 1.0), (0.0, 1.0), (1.0, 1.0)] {
            clip.galley_with_override_text_color(pos + vec2(dx * d, dy * d), g.clone(), oc);
        }
    }
    clip.galley(pos, g, color);
}

pub(crate) fn anchor_rect(geo: &Geo, sh: &Sheet, a: &Anchor) -> Rect {
    let x = geo.x(sh, a.cell.col) + a.dx * geo.z;
    let y = geo.y(sh, a.cell.row) + a.dy * geo.z;
    Rect::from_min_size(pos2(x, y), vec2(a.width.max(1.0) * geo.z, a.height.max(1.0) * geo.z))
}

// ------------------------------------------------------------------ moving and resizing

/// The smallest width or height (sheet points) an object can be dragged to. Both ends are held
/// apart by this much, so a stray pointer can never turn a chart into a line.
pub const OBJECT_MIN_SIZE: f32 = 20.0;
/// The largest size `object.move` stores (it clamps to this), so a wild drag asks for nothing more.
pub const OBJECT_MAX_SIZE: f32 = 10_000.0;
/// Side of a resize handle's hit box (screen points): wide enough that a press a few pixels inside the
/// corner, as people aim, grabs the handle rather than moving the object.
const HANDLE_SIZE: f32 = 16.0;

/// A resize handle of a selected object: the corner or edge it grabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    Nw,
    N,
    Ne,
    E,
    Se,
    S,
    Sw,
    W,
}

/// Which edges of an object a drag moves. A move drags all four.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Edges {
    pub left: bool,
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
}

impl Edges {
    /// A whole-object move: every edge follows the pointer.
    pub const MOVE: Edges = Edges { left: true, top: true, right: true, bottom: true };
}

impl Handle {
    pub fn edges(self) -> Edges {
        let (l, t, r, b) = match self {
            Handle::Nw => (true, true, false, false),
            Handle::N => (false, true, false, false),
            Handle::Ne => (false, true, true, false),
            Handle::E => (false, false, true, false),
            Handle::Se => (false, false, true, true),
            Handle::S => (false, false, false, true),
            Handle::Sw => (true, false, false, true),
            Handle::W => (true, false, false, false),
        };
        Edges { left: l, top: t, right: r, bottom: b }
    }

    /// The resize cursor for the handle. On a right-to-left sheet the drawing is mirrored, so a
    /// top-left handle sits on the screen's top right: the diagonal cursors swap with it.
    pub fn cursor(self, rtl: bool) -> CursorIcon {
        match self {
            Handle::N | Handle::S => CursorIcon::ResizeVertical,
            Handle::E | Handle::W => CursorIcon::ResizeHorizontal,
            Handle::Nw | Handle::Se if rtl => CursorIcon::ResizeNeSw,
            Handle::Ne | Handle::Sw if rtl => CursorIcon::ResizeNwSe,
            Handle::Nw | Handle::Se => CursorIcon::ResizeNwSe,
            Handle::Ne | Handle::Sw => CursorIcon::ResizeNeSw,
        }
    }
}

/// Centre of each of the eight handles, in the order `Handle` is listed.
const HANDLES: [Handle; 8] = [Handle::Nw, Handle::N, Handle::Ne, Handle::E, Handle::Se, Handle::S, Handle::Sw, Handle::W];

pub fn handle_centres(r: Rect) -> [Pos2; 8] {
    [r.left_top(), r.center_top(), r.right_top(), r.right_center(), r.right_bottom(), r.center_bottom(), r.left_bottom(), r.left_center()]
}

/// The handle under `p`, if any. Only a selected object shows handles; the caller checks that.
pub fn handle_at(r: Rect, p: Pos2) -> Option<Handle> {
    let side = vec2(HANDLE_SIZE, HANDLE_SIZE);
    handle_centres(r).into_iter().zip(HANDLES).find(|(c, _)| Rect::from_center_size(*c, side).contains(p)).map(|(_, h)| h)
}

/// An object's frame in sheet units (points): its top-left corner and size.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub w: f32,
    pub h: f32,
}

pub fn frame_of(sh: &Sheet, a: &Anchor) -> Frame {
    Frame { x: sh.col_left(a.cell.col) + f64::from(a.dx), y: sh.row_top(a.cell.row) + f64::from(a.dy), w: a.width, h: a.height }
}

/// The anchor (top-left cell, its pixel offset, and the size) that describes `f`. A frame on or
/// past the sheet's left/top edge anchors at the first cell with a zero offset.
pub fn anchor_of(sh: &Sheet, f: Frame) -> (CellRef, f32, f32, f32, f32) {
    let x = if f.x.is_finite() { f.x.max(0.0) } else { 0.0 };
    let y = if f.y.is_finite() { f.y.max(0.0) } else { 0.0 };
    let col = sh.col_at(x);
    let row = sh.row_at(y);
    let dx = (x - sh.col_left(col)).max(0.0) as f32;
    let dy = (y - sh.row_top(row)).max(0.0) as f32;
    let w = finite_size(f.w);
    let h = finite_size(f.h);
    (CellRef::new(row, col), dx, dy, w, h)
}

fn finite_size(v: f32) -> f32 {
    if v.is_finite() { v.clamp(OBJECT_MIN_SIZE, OBJECT_MAX_SIZE) } else { OBJECT_MIN_SIZE }
}

/// The frame a drag leaves the object in: `orig` moved or resized by `delta` sheet points, with
/// the edges `e` names moving. Dragging a left or top edge holds the opposite edge still, so the
/// anchor moves with it. The minimum size is always kept, and with `keep_ratio` (Shift) the other
/// dimension follows the dragged one at the size's original proportions.
pub fn drag_frame(orig: Frame, e: Edges, delta: egui::Vec2, keep_ratio: bool) -> Frame {
    let (dx, dy) = if delta.x.is_finite() && delta.y.is_finite() { (f64::from(delta.x), f64::from(delta.y)) } else { (0.0, 0.0) };
    let min = f64::from(OBJECT_MIN_SIZE);
    let w0 = f64::from(orig.w.clamp(OBJECT_MIN_SIZE, OBJECT_MAX_SIZE));
    let h0 = f64::from(orig.h.clamp(OBJECT_MIN_SIZE, OBJECT_MAX_SIZE));
    let x0 = if orig.x.is_finite() { orig.x.max(0.0) } else { 0.0 };
    let y0 = if orig.y.is_finite() { orig.y.max(0.0) } else { 0.0 };
    let right = x0 + w0;
    let bottom = y0 + h0;
    let moves_x = e.left && e.right;
    let moves_y = e.top && e.bottom;
    let mut w = if e.right && !e.left {
        w0 + dx
    } else if e.left && !e.right {
        w0 - dx
    } else {
        w0
    };
    let mut h = if e.bottom && !e.top {
        h0 + dy
    } else if e.top && !e.bottom {
        h0 - dy
    } else {
        h0
    };
    // The minimum size wins over the pointer: the edge that is not dragged stays put.
    w = w.max(min);
    h = h.max(min);
    if keep_ratio {
        let ratio = w0 / h0;
        let (horizontal, vertical) = (e.left != e.right, e.top != e.bottom);
        if horizontal && vertical {
            if (w / w0 - 1.0).abs() >= (h / h0 - 1.0).abs() {
                h = (w / ratio).max(min);
            } else {
                w = (h * ratio).max(min);
            }
        } else if horizontal {
            h = (w / ratio).max(min);
        } else if vertical {
            w = (h * ratio).max(min);
        }
    }
    w = finite_size(w as f32) as f64;
    h = finite_size(h as f32) as f64;
    let mut x = if moves_x {
        x0 + dx
    } else if e.left {
        right - w
    } else {
        x0
    };
    let mut y = if moves_y {
        y0 + dy
    } else if e.top {
        bottom - h
    } else {
        y0
    };
    // Nothing goes above or left of the sheet: the near edge stops at the first cell's origin.
    if x < 0.0 {
        x = 0.0;
        if !moves_x {
            w = (right - x).max(min);
        }
    }
    if y < 0.0 {
        y = 0.0;
        if !moves_y {
            h = (bottom - y).max(min);
        }
    }
    Frame { x, y, w: w as f32, h: h as f32 }
}

/// The shapes one floating object painted (indices into the painter's layer) and its logical
/// rectangle, so a right-to-left sheet can move it as a whole instead of mirroring it.
pub(crate) struct PaintedObject {
    pub shapes: std::ops::Range<usize>,
    pub rect: Rect,
}

/// Ctrl+6 (Options › Advanced › "For objects, show: Nothing"): objects are neither drawn nor
/// clickable. Placeholders, which Excel no longer offers, show the objects.
pub(crate) fn objects_hidden(wb: &Workbook) -> bool {
    wb.show_objects == gridcraft_engine::model::ShowObjects::None
}

pub(crate) fn paint_objects(
    app: &SheetApp,
    p: &Painter,
    geo: &Geo,
    wb: &Workbook,
    si: usize,
    sh: &Sheet,
    cache: &mut crate::grid::PaintCache,
) -> Vec<PaintedObject> {
    let mut painted = Vec::new();
    if objects_hidden(wb) {
        return painted;
    }
    let mut done = |start: egui::layers::ShapeIdx, rect: Rect| painted.push(PaintedObject { shapes: start.0..p.add(egui::Shape::Noop).0, rect });
    // The grid paints the active workbook: its pictures are cached under it.
    let doc = app.session.active().map_or(0, |d| d.uid);
    for sp in &sh.shapes {
        let r = anchor_rect(geo, sh, &sp.anchor);
        if !r.intersects(p.clip_rect()) {
            continue;
        }
        let start = p.add(egui::Shape::Noop);
        let mut label = true;
        let fill = sp.fill.resolve(&wb.theme).map(theme::color32).unwrap_or(Color32::TRANSPARENT);
        let line =
            if sp.format.no_line { Color32::TRANSPARENT } else { sp.line.resolve(&wb.theme).map(theme::color32).unwrap_or(Color32::DARK_GRAY) };
        match sp.kind {
            ShapeKind::Rectangle | ShapeKind::TextBox => {
                p.rect_filled(r, 0.0, fill);
                p.rect_stroke(r, 0.0, Stroke::new(1.0, line), StrokeKind::Inside);
            }
            ShapeKind::RoundedRectangle => {
                p.rect_filled(r, 10.0 * geo.z, fill);
                p.rect_stroke(r, 10.0 * geo.z, Stroke::new(1.0, line), StrokeKind::Inside);
            }
            ShapeKind::Ellipse => {
                let pts: Vec<Pos2> = (0..64)
                    .map(|i| {
                        let a = i as f32 / 64.0 * std::f32::consts::TAU;
                        pos2(r.center().x + r.width() / 2.0 * a.cos(), r.center().y + r.height() / 2.0 * a.sin())
                    })
                    .collect();
                p.add(PathShape::convex_polygon(pts, fill, Stroke::new(1.0, line)));
            }
            ShapeKind::Triangle => {
                p.add(PathShape::convex_polygon(vec![pos2(r.center().x, r.top()), r.right_bottom(), r.left_bottom()], fill, Stroke::new(1.0, line)));
            }
            ShapeKind::Ink => {
                let w = gridcraft_engine::cmd::draw::ink_width(sp) * geo.z;
                let pts: Vec<Pos2> =
                    gridcraft_engine::cmd::draw::ink_points(sp).iter().map(|q| pos2(r.left() + q[0] * geo.z, r.top() + q[1] * geo.z)).collect();
                if pts.len() >= 2 {
                    let col = if w > 8.0 { line.gamma_multiply(0.45) } else { line };
                    p.add(PathShape::line(pts, PathStroke::new(w, col)));
                }
                label = false;
            }
            ShapeKind::Icon => {
                if let Some(icon) = crate::icons::from_name(&sp.text) {
                    crate::icons::paint(p, r, icon, if fill == Color32::TRANSPARENT { Color32::BLACK } else { fill });
                }
                label = false;
            }
            ShapeKind::Line | ShapeKind::Arrow => {
                p.line_segment([r.left_top(), r.right_bottom()], Stroke::new(2.0, line));
                if sp.kind == ShapeKind::Arrow {
                    let d = (r.right_bottom() - r.left_top()).normalized();
                    let n = vec2(-d.y, d.x);
                    let tip = r.right_bottom();
                    p.add(PathShape::convex_polygon(vec![tip, tip - d * 10.0 + n * 5.0, tip - d * 10.0 - n * 5.0], line, Stroke::NONE));
                }
            }
        }
        if label && !sp.text.is_empty() && !app.editing_text_box(sp.id) {
            paint_shape_text(p, wb, sp, r, geo.z, Some(&app.text));
        }
        if label && sp.kind == ShapeKind::TextBox && app.selected_chart == Some(sp.id) {
            selection_frame(p, r);
        }
        done(start, r);
    }
    // Equations: a box painted like a text box, holding the equation's linear text. The 2-D math
    // layout belongs to whoever reads the file (the markup is kept verbatim); here it reads aloud.
    for eq in &sh.equations {
        let r = anchor_rect(geo, sh, &eq.anchor);
        if !r.intersects(p.clip_rect()) {
            continue;
        }
        let start = p.add(egui::Shape::Noop);
        if let Some(fill) = eq.fill.resolve(&wb.theme).map(theme::color32) {
            p.rect_filled(r, 0.0, fill);
        }
        if let Some(line) = eq.line.resolve(&wb.theme).map(theme::color32) {
            p.rect_stroke(r, 0.0, Stroke::new(1.0, line), StrokeKind::Inside);
        }
        if !eq.source.is_empty() {
            // Centred in its box, as Excel lays an equation out.
            let format = gridcraft_engine::model::ShapeFormat {
                align: Some(gridcraft_engine::model::ShapeTextAlign::Center),
                anchor: Some(gridcraft_engine::model::ShapeTextAnchor::Middle),
                ..Default::default()
            };
            paint_box_text(p, wb, &eq.source, true, &format, r, geo.z, Some(&app.text));
        }
        if app.selected_chart == Some(eq.id) {
            selection_frame(p, r);
        }
        done(start, r);
    }
    for im in &sh.images {
        let r = anchor_rect(geo, sh, &im.anchor);
        if !r.intersects(p.clip_rect()) {
            continue;
        }
        let start = p.add(egui::Shape::Noop);
        if let Some(tex) = crate::floating_pictures::texture(p.ctx(), doc, im, r) {
            p.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        } else {
            p.rect_filled(r, 0.0, Color32::from_gray(230));
        }
        if app.selected_chart == Some(im.id) {
            selection_frame(p, r);
        }
        done(start, r);
    }
    // A chart sheet is its chart: one chart, filling the window instead of sitting in a
    // cell-anchored frame (Excel paints the chart sheet's chart over the whole page area).
    let chart_sheet = sh.is_chart_sheet();
    for (i, ch) in sh.charts.iter().enumerate() {
        let (w, h) = (ch.anchor.width, ch.anchor.height);
        let (r, z) = if chart_sheet {
            // Laid out at its own size and fitted to the window, keeping its proportions; the
            // drawing is laid out once per size, so the click regions stay the ones painted.
            let view = geo.cells;
            let (w, h) = (w.max(1.0), h.max(1.0));
            let fit = (view.width() / w).min(view.height() / h);
            let z = if fit.is_finite() && fit > 0.0 { fit } else { geo.z };
            (Rect::from_center_size(view.center(), vec2(w * z, h * z)), z)
        } else {
            (anchor_rect(geo, sh, &ch.anchor), geo.z)
        };
        if !r.intersects(p.clip_rect()) {
            continue;
        }
        let start = p.add(egui::Shape::Noop);
        // Soft shadow + white card.
        p.rect_filled(r.translate(vec2(0.0, 1.5)), 2.0, Color32::from_black_alpha(18));
        p.rect_filled(r, 0.0, Color32::WHITE);
        // Laying a chart out (axis ticks, measured labels) is far too slow for every frame: the
        // drawing — primitives and click regions — is kept until the document or the anchor size
        // changes.
        let size = (w.to_bits(), h.to_bits());
        if cache.chart_stale(i, size) {
            let data = gridcraft_chart::resolve(wb, si, ch);
            let m = EguiMeasure { painter: p, z, text: Some(&app.text) };
            let drawing = gridcraft_chart::draw(ch, &data, w, h, &m);
            cache.put_chart(i, size, drawing);
        }
        if let Some(drawing) = cache.chart(i) {
            paint_prims(&p.with_clip_rect(r.intersect(p.clip_rect())), r.min, &drawing.prims, z, Some(&app.text));
        }
        if app.selected_chart == Some(ch.id) {
            if chart_sheet {
                // The chart sheet's chart is the window's size: a resize frame would promise
                // something the sheet cannot do.
                p.rect_stroke(r, 0.0, Stroke::new(1.5, theme::Tokens::get(p.ctx()).accent), StrokeKind::Outside);
            } else {
                selection_frame(p, r);
            }
        }
        // The source-data highlight below is in cell space: mirrored with the cells.
        done(start, r);
        if app.selected_chart == Some(ch.id) {
            // Highlight the source data like Excel does.
            for s in &ch.series {
                if let Ok(gridcraft_engine::formula::Expr::Ref(rf)) = gridcraft_engine::formula::parse(&s.values)
                    && rf.sheet_name().is_none_or(|n| n.eq_ignore_ascii_case(&sh.name))
                {
                    let rr = geo.range_rect(sh, rf.range());
                    for pane in geo.range_panes(rf.range()) {
                        let pp = p.with_clip_rect(pane.intersect(p.clip_rect()));
                        pp.rect_stroke(rr, 0.0, Stroke::new(1.5, Color32::from_rgb(0x2E, 0x6F, 0xD8)), StrokeKind::Inside);
                    }
                }
            }
        }
    }
    paint_slicers(app, p, geo, wb, si, sh);
    // PivotTable slicers and timelines sit above the table slicers (their own drawings are last).
    painted.extend(crate::slicer_view::paint(app, p, geo, wb, sh));
    painted
}

/// Geometry of a slicer's parts within its screen rect.
pub struct SlicerLayout {
    pub header: Rect,
    /// Clear-filter button in the header.
    pub clear: Rect,
    /// One rect per value tile, in order.
    pub tiles: Vec<Rect>,
}

pub fn slicer_layout(rect: Rect, z: f32, n: usize, columns: u32) -> SlicerLayout {
    let header_h = 26.0 * z;
    let pad = 6.0 * z;
    let gap = 3.0 * z;
    let tile_h = 22.0 * z;
    let header = Rect::from_min_size(rect.min, vec2(rect.width(), header_h));
    let cb = 16.0 * z;
    let clear = Rect::from_min_size(pos2(header.right() - pad - cb, header.top() + (header_h - cb) / 2.0), vec2(cb, cb));
    let cols = columns.max(1) as f32;
    let inner_left = rect.left() + pad;
    let inner_top = header.bottom() + pad;
    let inner_w = (rect.width() - 2.0 * pad).max(1.0);
    let tile_w = ((inner_w - (cols - 1.0) * gap) / cols).max(1.0);
    let mut tiles = Vec::with_capacity(n);
    for i in 0..n {
        let col = (i as f32) % cols;
        let row = (i as f32 / cols).floor();
        let x = inner_left + col * (tile_w + gap);
        let y = inner_top + row * (tile_h + gap);
        tiles.push(Rect::from_min_size(pos2(x, y), vec2(tile_w, tile_h)));
    }
    SlicerLayout { header, clear, tiles }
}

fn paint_slicers(app: &SheetApp, p: &Painter, geo: &Geo, wb: &Workbook, si: usize, sh: &Sheet) {
    let tk = theme::Tokens::get(p.ctx());
    for sl in &sh.slicers {
        let r = anchor_rect(geo, sh, &sl.anchor);
        if !r.intersects(p.clip_rect()) {
            continue;
        }
        let items = gridcraft_engine::cmd::slicer::slicer_items(wb, si, sl);
        let lay = slicer_layout(r, geo.z, items.len(), sl.columns);
        let radius = 4.0 * geo.z;
        // Card + soft shadow.
        p.rect_filled(r.translate(vec2(0.0, 1.5)), radius, Color32::from_black_alpha(18));
        p.rect_filled(r, radius, tk.window);
        p.rect_stroke(r, radius, Stroke::new(1.0, tk.separator), StrokeKind::Inside);
        // Header.
        p.rect_filled(lay.header, radius, tk.accent_soft);
        p.rect_filled(Rect::from_min_max(pos2(lay.header.left(), lay.header.bottom() - radius), lay.header.max), 0.0, tk.accent_soft);
        let cap = if sl.caption.is_empty() { sl.column.clone() } else { sl.caption.clone() };
        p.text(lay.header.left_center() + vec2(8.0 * geo.z, 0.0), Align2::LEFT_CENTER, cap, theme::ui_bold(12.5 * geo.z), tk.accent_dark);
        // Clear-filter button (tinted when a filter is active).
        let filtered = sl.selected.is_some();
        if let Some(icon) = crate::icons::from_name("filter") {
            crate::icons::paint(p, lay.clear, icon, if filtered { tk.accent } else { tk.text_dim });
        }
        // Value tiles.
        for ((val, on), tr) in items.iter().zip(&lay.tiles) {
            if !r.contains(tr.left_top()) {
                continue; // overflowed the card; skip (no scroll yet)
            }
            let (fill, fg) = if *on { (tk.accent, Color32::WHITE) } else { (tk.hover, tk.text_dim) };
            p.rect_filled(*tr, 3.0 * geo.z, fill);
            let cp = p.with_clip_rect(tr.intersect(p.clip_rect()));
            cp.text(tr.left_center() + vec2(6.0 * geo.z, 0.0), Align2::LEFT_CENTER, val, theme::ui_font(12.0 * geo.z), fg);
        }
        if app.selected_chart == Some(sl.id) {
            selection_frame(p, r);
        }
    }
}

/// What a click inside a slicer hit.
pub enum SlicerAction {
    Toggle(String),
    Clear,
}

/// A click on a slicer's value tile or clear button, if any. A click elsewhere in a slicer returns
/// `None` so the caller can treat it as a select/drag on the object.
pub fn slicer_click(geo: &Geo, sh: &Sheet, wb: &Workbook, si: usize, p: Pos2) -> Option<(u32, SlicerAction)> {
    if !geo.cells.contains(p) || objects_hidden(wb) {
        return None;
    }
    for sl in sh.slicers.iter().rev() {
        let r = anchor_rect(geo, sh, &sl.anchor);
        if !r.contains(p) {
            continue;
        }
        let items = gridcraft_engine::cmd::slicer::slicer_items(wb, si, sl);
        let lay = slicer_layout(r, geo.z, items.len(), sl.columns);
        if lay.clear.contains(p) {
            return Some((sl.id, SlicerAction::Clear));
        }
        for ((val, _on), tr) in items.iter().zip(&lay.tiles) {
            if r.contains(tr.left_top()) && tr.contains(p) {
                return Some((sl.id, SlicerAction::Toggle(val.clone())));
            }
        }
        return None; // inside this (topmost) slicer, but on its header/background → select/drag
    }
    None
}

/// The accent border of a selected object that has no resize handles. A timeline is selected like
/// any object, but `slicer.move` has no timeline path, so drawing handles would advertise a drag
/// that does not exist.
pub(crate) fn selection_outline(p: &Painter, r: Rect) {
    let t = theme::Tokens::get(p.ctx());
    p.rect_stroke(r, 0.0, Stroke::new(1.0, t.accent), StrokeKind::Outside);
}

/// The frame and eight drag handles of the selected object, in theme colours.
pub(crate) fn selection_frame(p: &Painter, r: Rect) {
    let t = theme::Tokens::get(p.ctx());
    p.rect_stroke(r, 0.0, Stroke::new(1.0, t.accent), StrokeKind::Outside);
    for c in handle_centres(r) {
        p.circle_filled(c, 4.0, t.window);
        p.circle_stroke(c, 4.0, Stroke::new(1.0, t.accent));
    }
    let _ = Align2::CENTER_CENTER;
}

/// The topmost object under `p`: (kind, id, rect).
pub fn hit(app: &SheetApp, geo: &Geo, sh: &Sheet, p: Pos2) -> Option<(&'static str, u32, Rect)> {
    if app.session.active().is_some_and(|d| objects_hidden(&d.wb)) {
        return None;
    }
    // Pivot slicers and timelines are painted last, so they are hit first.
    for tl in sh.pivot_timelines.iter().rev() {
        let r = crate::slicer_view::widget_rect(geo, sh, &tl.anchor);
        if r.contains(p) && geo.cells.contains(p) {
            return Some(("timeline", tl.id, r));
        }
    }
    for sl in sh.pivot_slicers.iter().rev() {
        let r = crate::slicer_view::widget_rect(geo, sh, &sl.anchor);
        if r.contains(p) && geo.cells.contains(p) {
            return Some(("pivotSlicer", sl.id, r));
        }
    }
    for sl in sh.slicers.iter().rev() {
        let r = anchor_rect(geo, sh, &sl.anchor);
        if r.contains(p) && geo.cells.contains(p) {
            return Some(("slicer", sl.id, r));
        }
    }
    for ch in sh.charts.iter().rev() {
        let r = anchor_rect(geo, sh, &ch.anchor);
        if r.expand(4.0).contains(p) && geo.cells.contains(p) {
            return Some(("chart", ch.id, r));
        }
    }
    for im in sh.images.iter().rev() {
        let r = anchor_rect(geo, sh, &im.anchor);
        if r.contains(p) && geo.cells.contains(p) {
            return Some(("image", im.id, r));
        }
    }
    for eq in sh.equations.iter().rev() {
        let r = anchor_rect(geo, sh, &eq.anchor);
        if r.contains(p) && geo.cells.contains(p) {
            return Some(("equation", eq.id, r));
        }
    }
    for s in sh.shapes.iter().rev() {
        let r = anchor_rect(geo, sh, &s.anchor);
        if r.expand(3.0).contains(p) && geo.cells.contains(p) {
            return Some(("shape", s.id, r));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Frame {
        Frame { x: 300.0, y: 200.0, w: 300.0, h: 200.0 }
    }

    #[test]
    fn each_handle_grabs_its_own_edges() {
        let r = Rect::from_min_size(pos2(100.0, 100.0), vec2(200.0, 100.0));
        for (h, at) in [
            (Handle::Nw, r.left_top()),
            (Handle::N, r.center_top()),
            (Handle::Ne, r.right_top()),
            (Handle::E, r.right_center()),
            (Handle::Se, r.right_bottom()),
            (Handle::S, r.center_bottom()),
            (Handle::Sw, r.left_bottom()),
            (Handle::W, r.left_center()),
        ] {
            assert_eq!(handle_at(r, at), Some(h), "{h:?}");
        }
        // The middle of the object is a move, not a resize.
        assert_eq!(handle_at(r, r.center()), None);
    }

    #[test]
    fn a_corner_drag_holds_the_opposite_corner() {
        let f = drag_frame(frame(), Handle::Nw.edges(), vec2(40.0, 20.0), false);
        assert_eq!((f.x, f.y), (340.0, 220.0), "the dragged corner follows the pointer");
        assert!((f.w - 260.0).abs() < 0.01 && (f.h - 180.0).abs() < 0.01, "the far corner stays: {f:?}");
        let r = drag_frame(frame(), Handle::Se.edges(), vec2(40.0, 20.0), false);
        assert_eq!((r.x, r.y), (300.0, 200.0));
        assert!((r.w - 340.0).abs() < 0.01 && (r.h - 220.0).abs() < 0.01);
    }

    #[test]
    fn a_move_keeps_the_size() {
        let f = drag_frame(frame(), Edges::MOVE, vec2(-50.0, 30.0), false);
        assert_eq!((f.x, f.y), (250.0, 230.0));
        assert_eq!((f.w, f.h), (300.0, 200.0));
    }

    #[test]
    fn shift_keeps_the_ratio_in_the_dragged_direction() {
        let f = drag_frame(frame(), Handle::Se.edges(), vec2(120.0, 0.0), true);
        assert!((f.w / f.h - 1.5).abs() < 1e-4, "{f:?}");
        assert!(f.w > 300.0 && f.h > 200.0);
        // An edge handle takes the other dimension with it.
        let f = drag_frame(frame(), Handle::S.edges(), vec2(0.0, 40.0), true);
        assert!((f.w / f.h - 1.5).abs() < 1e-4, "{f:?}");
    }

    #[test]
    fn the_minimum_size_wins_over_the_pointer() {
        let f = drag_frame(frame(), Handle::Se.edges(), vec2(-1000.0, -1000.0), false);
        assert_eq!((f.x, f.y), (300.0, 200.0));
        assert_eq!((f.w, f.h), (OBJECT_MIN_SIZE, OBJECT_MIN_SIZE));
        let f = drag_frame(frame(), Handle::Nw.edges(), vec2(1000.0, 1000.0), false);
        assert_eq!((f.w, f.h), (OBJECT_MIN_SIZE, OBJECT_MIN_SIZE));
        // Held at the opposite corner.
        assert!((f.x + f64::from(f.w) - 600.0).abs() < 0.01 && (f.y + f64::from(f.h) - 400.0).abs() < 0.01, "{f:?}");
    }

    #[test]
    fn hostile_pointer_numbers_are_rejected() {
        let f = drag_frame(Frame { x: f64::NAN, y: f64::INFINITY, ..frame() }, Handle::Se.edges(), vec2(f32::NAN, f32::INFINITY), false);
        assert!(f.x.is_finite() && f.y.is_finite() && f.w.is_finite() && f.h.is_finite(), "{f:?}");
        assert_eq!((f.x, f.y), (0.0, 0.0));
        // A squeezed or wild size is normalised, never stored as NaN or past the command's limit.
        for (w, h) in [(0.0, -5.0), (f32::NAN, f32::INFINITY), (1e12, 1e12)] {
            let f = drag_frame(Frame { w, h, ..frame() }, Edges::MOVE, vec2(0.0, 0.0), false);
            assert!(f.w >= OBJECT_MIN_SIZE && f.w <= OBJECT_MAX_SIZE && f.h >= OBJECT_MIN_SIZE && f.h <= OBJECT_MAX_SIZE, "{f:?}");
        }
    }

    #[test]
    fn nothing_goes_off_the_top_left_of_the_sheet() {
        let f = drag_frame(frame(), Edges::MOVE, vec2(-500.0, -500.0), false);
        assert_eq!((f.x, f.y), (0.0, 0.0));
        assert_eq!((f.w, f.h), (300.0, 200.0), "a move keeps the size");
        // Dragging the left edge past the sheet start pins it there instead of growing.
        let f = drag_frame(Frame { x: 50.0, y: 50.0, w: 300.0, h: 200.0 }, Handle::W.edges(), vec2(-100.0, 0.0), false);
        assert_eq!(f.x, 0.0);
        assert!((f.w - 350.0).abs() < 0.01, "the right edge stays at 350: {f:?}");
    }

    #[test]
    fn a_frame_round_trips_through_its_anchor() {
        let sh = Sheet::new("S");
        let a = Anchor { cell: CellRef::new(2, 3), dx: 7.0, dy: 5.0, width: 320.0, height: 180.0, ..Anchor::default() };
        let f = frame_of(&sh, &a);
        let (cell, dx, dy, w, h) = anchor_of(&sh, f);
        assert_eq!(cell, a.cell);
        assert!((dx - a.dx).abs() < 0.01 && (dy - a.dy).abs() < 0.01 && (w - a.width).abs() < 0.01 && (h - a.height).abs() < 0.01);
        // A frame left of the first column anchors at A1 with a zero offset.
        let (cell, dx, dy, w, _) = anchor_of(&sh, Frame { x: 0.0, y: 0.0, ..f });
        assert_eq!((cell, dx, dy, w), (CellRef::new(0, 0), 0.0, 0.0, 320.0));
    }

    use super::EguiMeasure;
    use gridcraft_chart::Measure;

    /// A chart label is measured by the shaped engine when one is handed in, so the layout that
    /// places it and the galley that draws it agree; without an engine the old epaint measure is
    /// kept (the Recommended Charts preview's and the tests' path).
    #[test]
    fn a_label_is_measured_with_the_shaped_engine() {
        let Some(file) = gridcraft_engine::core::fonts::resolve_for_char('م').ok() else {
            eprintln!("skipped: no installed Arabic face (set CRAFT_FONTS_DIR to test it)");
            return;
        };
        let Some(index) = (0..8).find(|i| {
            use skrifa::MetadataProvider;
            skrifa::FontRef::from_index(&file.data, *i).is_ok_and(|f| f.charmap().map('م').is_some())
        }) else {
            return;
        };
        // The label's font is the UI family (`theme::ui_font`), so bind the face to that name.
        let ctx = crate::text::test_context(crate::theme::UI, &file.data, index);
        let engine = crate::text::GridText::new();
        let painter = egui::Painter::new(ctx.clone(), egui::LayerId::background(), egui::Rect::EVERYTHING);
        let label = "مرحبا";
        let font = crate::theme::ui_font(32.0);
        let shaped = engine.measure(&ctx, &font, label, crate::text::base_direction(label, false));
        let with = EguiMeasure { painter: &painter, z: 1.0, text: Some(&engine) };
        assert!((with.text_width(label, 32.0, false) - shaped).abs() < 0.01, "the label is measured shaped");
        let without = EguiMeasure { painter: &painter, z: 1.0, text: None };
        assert!(without.text_width(label, 32.0, false) > 0.0, "the unshaped fallback still measures");
    }
}
