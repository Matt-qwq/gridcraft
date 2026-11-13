//! PivotTable slicers and timelines on the sheet: a titled frame of value tiles, and a timeline's
//! caption, time unit and full-range track.
//!
//! This is the *PivotTable* widget ([`gridcraft_engine::model::PivotSlicer`]); a table slicer is
//! drawn by [`crate::chartview::paint_slicers`]. Everything here is drawing and hit-testing: the
//! commands (`slicer.toggle`, `slicer.clear`, `slicer.move`, `slicer.delete`, `slicer.setStyle`)
//! live in the engine, and the grid calls them in one undo step.
//!
//! A timeline's selected date *range* is stored on the model ([`TimelineRange`], the cache's
//! `<state><selection>`), so its two handles are draggable and the widget draws the selected band
//! on the field's own date axis. The axis (the field's first and last date) comes from the report's
//! source once per revision ([`ensure_timeline_axes`]); a drag maps a pointer position back to a
//! whole day and runs `slicer.setRange`, one undo step per drag.
//!
//! Tiles are drawn in code from [`crate::theme::Tokens`]; no image assets are involved. The
//! slicer style *names* are file-format identifiers, and each family (light / other / dark) picks
//! a different set of token colours.
//!
//! Everything read from the model is hostile input here: a file (or a command) can carry a NaN
//! anchor, the last cell, no items, a kilometre-long caption or a million tiles. The helper
//! [`widget_rect`] clamps the frame, text is elided before it is laid out, and only the visible
//! tiles are laid out and painted (never one rect per item).

use egui::{Align2, Color32, CursorIcon, Painter, Pos2, Rect, Stroke, StrokeKind, pos2, vec2};
use gridcraft_engine::core::{MAX_COLS, MAX_ROWS};
use gridcraft_engine::model::{Anchor, PivotSlicer, PivotTimeline, Sheet, TimeUnit, TimelineRange, Workbook};

use crate::SheetApp;
use crate::grid::Geo;
use crate::theme;

/// Height of a widget's caption band, in screen points at zoom 1.
const HEADER_H: f32 = 26.0;
/// Inner padding of a widget, in screen points at zoom 1.
const PAD: f32 = 6.0;
/// Gap between tile columns and rows.
const GAP: f32 = 3.0;
/// Height of one value tile.
const TILE_H: f32 = 22.0;
/// Height of the "N of M" footer shown when tiles do not all fit.
const FOOTER_H: f32 = 14.0;
/// The most tiles one slicer lays out, so a hostile item list cannot make a frame's paint quadratically slow.
const MAX_TILES: usize = 500;
/// The most characters of a caption or tile label that are laid out.
const MAX_TEXT: usize = 120;
/// The largest widget size a hostile anchor may ask for (sheet points), matching the commands.
const MAX_WIDGET: f32 = 10_000.0;
/// The furthest a stored pixel offset may put a widget from its cell (sheet points).
const MAX_OFFSET: f32 = 1_000_000.0;

fn finite(v: f32, fallback: f32) -> f32 {
    if v.is_finite() { v } else { fallback }
}

/// The on-screen frame of a widget, clamped so a hostile anchor (NaN, negative, past the sheet)
/// can only ever produce a finite rectangle that is clipped by the painter.
pub fn widget_rect(geo: &Geo, sh: &Sheet, a: &Anchor) -> Rect {
    let z = finite(geo.z, 1.0).clamp(0.01, 64.0);
    let col = a.cell.col.min(MAX_COLS - 1);
    let row = a.cell.row.min(MAX_ROWS - 1);
    let x = finite(geo.x(sh, col) + finite(a.dx, 0.0).clamp(-MAX_OFFSET, MAX_OFFSET) * z, geo.cells.left());
    let y = finite(geo.y(sh, row) + finite(a.dy, 0.0).clamp(-MAX_OFFSET, MAX_OFFSET) * z, geo.cells.top());
    let w = finite(a.width, 168.0).clamp(1.0, MAX_WIDGET) * z;
    let h = finite(a.height, 96.0).clamp(1.0, MAX_WIDGET) * z;
    Rect::from_min_size(pos2(x.clamp(-1.0e9, 1.0e9), y.clamp(-1.0e9, 1.0e9)), vec2(w, h))
}

/// The colours one slicer style family uses. The *names* are file-format identifiers; the looks
/// are ours, built only from theme tokens.
#[derive(Clone, Copy)]
struct Look {
    header: Color32,
    header_text: Color32,
    tile_on: Color32,
    tile_on_text: Color32,
    tile_off: Color32,
    tile_off_text: Color32,
    border: Color32,
}

fn look(style: &str, t: &theme::Tokens) -> Look {
    let s = style.to_ascii_lowercase();
    // On a light accent (dark mode) dark text reads; on the dark accents of light mode white does.
    let on_accent = if t.dark { Color32::BLACK } else { Color32::WHITE };
    if s.contains("dark") {
        Look {
            header: t.text_dim,
            header_text: t.window,
            tile_on: t.accent,
            tile_on_text: on_accent,
            tile_off: t.hover,
            tile_off_text: t.text,
            border: t.separator,
        }
    } else if s.contains("other") {
        Look {
            header: t.pressed,
            header_text: t.text,
            tile_on: t.accent_dark,
            tile_on_text: Color32::WHITE,
            tile_off: t.window,
            tile_off_text: t.text_dim,
            border: t.separator,
        }
    } else {
        Look {
            header: t.accent_soft,
            header_text: t.accent_dark,
            tile_on: t.accent,
            tile_on_text: on_accent,
            tile_off: t.hover,
            tile_off_text: t.text_dim,
            border: t.separator,
        }
    }
}

/// Bounds the text laid out for a caption or a tile: `max` characters and an ellipsis when cut.
/// A model value can be arbitrarily long (a text tile survives from a file), and laying one out in
/// full would allocate a galley the size of the string.
fn elide(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let mut out: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        out.push('…');
    }
    if out.is_empty() { " ".to_string() } else { out }
}

/// The visible parts of a slicer's frame.
#[derive(Debug)]
pub struct SlicerChrome {
    /// The frame on screen.
    pub frame: Rect,
    /// The caption band.
    pub header: Rect,
    /// The clear-filter button, when the frame is wide enough for one.
    pub clear: Option<Rect>,
    /// The area the tiles are clipped to.
    pub body: Rect,
    /// The status line below the tiles, when they do not all fit.
    pub footer: Option<Rect>,
    /// The visible tiles and their indices in the item list.
    pub tiles: Vec<(usize, Rect)>,
    /// The first visible row of tiles.
    pub first_row: usize,
    /// How many rows fit in the body.
    pub rows: usize,
    /// How many rows the item list needs.
    pub total_rows: usize,
    /// How many items the slicer holds.
    pub total: usize,
}

/// Lays out a slicer's frame, given the first tile row currently scrolled into view. Only the
/// visible tiles are laid out, at most [`MAX_TILES`] of them.
pub fn slicer_chrome(geo: &Geo, frame: Rect, columns: u32, n: usize, first_row: usize) -> SlicerChrome {
    let z = finite(geo.z, 1.0).clamp(0.01, 64.0);
    let cols = columns.clamp(1, 64) as usize;
    let header_h = (HEADER_H * z).min(frame.height()).max(0.0);
    let header = Rect::from_min_size(frame.min, vec2(frame.width(), header_h));
    let cb = 16.0 * z;
    let pad = PAD * z;
    let clear = (header.width() >= cb + 3.0 * pad + 8.0 * z)
        .then(|| Rect::from_min_size(pos2(header.right() - pad - cb, header.top() + ((header_h - cb) / 2.0).max(0.0)), vec2(cb, cb)));
    let body_top = frame.top() + header_h;
    let body_full = Rect::from_min_max(pos2(frame.left(), body_top), frame.max);
    let tile_h = (TILE_H * z).max(1.0);
    let gap = GAP * z;
    let step = tile_h + gap;
    let total_rows = n.div_ceil(cols);
    let content_h = |footer: f32| (body_full.height() - 2.0 * pad - footer).max(0.0);
    let mut rows = (content_h(0.0) / step).floor() as usize;
    let mut footer_h = 0.0;
    if n > rows.saturating_mul(cols) && body_full.height() > FOOTER_H * z + 2.0 * pad {
        footer_h = FOOTER_H * z;
        rows = (content_h(footer_h) / step).floor() as usize;
    }
    rows = rows.min(MAX_TILES / cols.max(1));
    let first_row = if rows == 0 { 0 } else { first_row.min(total_rows.saturating_sub(1)) };
    let footer = (footer_h > 0.0).then(|| {
        let top = (body_full.bottom() - pad - footer_h).max(body_full.top());
        Rect::from_min_max(pos2(body_full.left() + pad, top), pos2(body_full.right() - pad, body_full.bottom()))
    });
    let body = Rect::from_min_max(body_full.min, pos2(body_full.right(), footer.map_or(body_full.bottom(), |f| f.top())));
    let inner_left = frame.left() + pad;
    let inner_top = body_top + pad;
    let inner_w = (frame.width() - 2.0 * pad).max(1.0);
    let tile_w = ((inner_w - (cols.saturating_sub(1)) as f32 * gap) / cols as f32).max(1.0);
    let first = first_row.saturating_mul(cols);
    let take = rows.saturating_mul(cols).min(n.saturating_sub(first)).min(MAX_TILES);
    let mut tiles = Vec::with_capacity(take);
    for k in 0..take {
        let i = first + k;
        let col = (i % cols) as f32;
        let row = (i / cols - first_row) as f32;
        tiles.push((i, Rect::from_min_size(pos2(inner_left + col * (tile_w + gap), inner_top + row * step), vec2(tile_w, tile_h))));
    }
    SlicerChrome { frame, header, clear, body, footer, tiles, first_row, rows, total_rows, total: n }
}

/// The visible parts of a timeline's frame.
#[derive(Debug)]
pub struct TimelineChrome {
    pub frame: Rect,
    pub header: Rect,
    /// The time unit under the caption.
    pub unit: Rect,
    /// The horizontal range track.
    pub track: Rect,
    /// The two range handles at the track's ends (the full axis; a selected range moves them).
    pub handles: [Rect; 2],
}

pub fn timeline_chrome(geo: &Geo, frame: Rect) -> TimelineChrome {
    let z = finite(geo.z, 1.0).clamp(0.01, 64.0);
    let pad = PAD * z;
    let header_h = (HEADER_H * z).min(frame.height()).max(0.0);
    let header = Rect::from_min_size(frame.min, vec2(frame.width(), header_h));
    let body_top = frame.top() + header_h;
    let unit_h = (12.0 * z).min((frame.bottom() - body_top).max(0.0));
    let unit = Rect::from_min_size(pos2(frame.left() + pad, body_top + pad), vec2((frame.width() - 2.0 * pad).max(1.0), unit_h));
    let handle_w = (8.0 * z).max(2.0);
    let handle_h = (20.0 * z).max(4.0);
    let track_top = unit.bottom() + pad;
    let track_h = (8.0 * z).max(2.0);
    let left = frame.left() + pad + handle_w;
    let right = (frame.right() - pad - handle_w).max(left + 1.0);
    // Keep the track inside a frame that is too short for its parts; the painter clips to the
    // frame as well, so a hostile 1-point timeline cannot draw over the sheet.
    let track_top = track_top.min((frame.bottom() - pad - track_h).max(unit.bottom()));
    let track = Rect::from_min_size(pos2(left, track_top), vec2(right - left, track_h));
    let at = |x: f32| Rect::from_center_size(pos2(x, track.center().y), vec2(handle_w, handle_h));
    TimelineChrome { frame, header, unit, track, handles: [at(track.left()), at(track.right())] }
}

/// The screen x's of a timeline's selected band on its track: the range's two ends mapped onto
/// the field's date axis. `None` when the axis has no span, so the band cannot be placed honestly.
pub(crate) fn band_xs(track: Rect, axis: Option<(f64, f64)>, range: Option<TimelineRange>) -> Option<(f32, f32)> {
    let (lo, hi) = axis?;
    if !lo.is_finite() || !hi.is_finite() || hi <= lo || !track.width().is_finite() {
        return None;
    }
    let r = range.unwrap_or(TimelineRange::new(lo, hi));
    let x = |day: f64| track.left() + (((day - lo) / (hi - lo)).clamp(0.0, 1.0) as f32) * track.width();
    let (a, b) = (x(r.start), x(r.end));
    Some((a.min(b), a.max(b)))
}

/// The handles a timeline draws and hit-tests: at the selected band's ends when the axis can place
/// them, otherwise at the track's own ends.
pub(crate) fn timeline_handles(lay: &TimelineChrome, axis: Option<(f64, f64)>, range: Option<TimelineRange>) -> [Rect; 2] {
    let at = |x: f32| Rect::from_center_size(pos2(x, lay.track.center().y), vec2(lay.handles[0].width(), lay.handles[0].height()));
    match band_xs(lay.track, axis, range) {
        Some((a, b)) => [at(a), at(b)],
        None => [at(lay.track.left()), at(lay.track.right())],
    }
}

/// Which part of a timeline a drag took hold of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineGrab {
    Start,
    End,
    /// The selected band itself: slides the whole range, keeping its length.
    Move,
}

/// A timeline range drag in progress: the axis it maps onto and the range it started from.
#[derive(Clone, Debug, PartialEq)]
pub struct TimelineDrag {
    pub id: u32,
    pub grab: TimelineGrab,
    /// The press position, in the logical screen points `timeline_chrome` works in.
    pub start: Pos2,
    /// The full-range track when the drag began, in screen points.
    pub track: Rect,
    /// The range when the drag began; `None` = every date.
    pub orig: Option<TimelineRange>,
    /// The field's first and last date, as serial days.
    pub bounds: (f64, f64),
    /// Undo entries this drag has recorded, so only its first one starts a new entry.
    pub steps: u32,
}

/// The day at screen `x` on `track`: clamped to the axis and rounded to a whole day. A track too
/// narrow to divide, or an axis with no span, puts every position on the first day.
fn day_at(track: Rect, bounds: (f64, f64), x: f32) -> f64 {
    let (lo, hi) = bounds;
    if !lo.is_finite() || !hi.is_finite() || hi <= lo || !track.width().is_finite() || track.width() <= 0.0 {
        return lo;
    }
    let t = f64::from(((x - track.left()) / track.width()).clamp(0.0, 1.0));
    (lo + (hi - lo) * t).round().clamp(lo, hi)
}

/// The range a timeline drag shows for pointer `p`. A grabbed end follows the pointer one whole
/// day at a time; the body slides by the days the pointer crossed, keeping its length and never
/// leaving the axis. A range keeps at least one day, so a handle cannot pass the other one.
pub(crate) fn dragged_range(d: &TimelineDrag, p: Pos2) -> TimelineRange {
    let (lo, hi) = d.bounds;
    let orig = d.orig.unwrap_or(TimelineRange::new(lo, hi));
    let day = day_at(d.track, d.bounds, p.x);
    match d.grab {
        TimelineGrab::Start => TimelineRange::new(day.min(orig.end), orig.end),
        TimelineGrab::End => TimelineRange::new(orig.start, day.max(orig.start)),
        TimelineGrab::Move => {
            let len = orig.end - orig.start;
            if !lo.is_finite() || !hi.is_finite() || hi <= lo || !len.is_finite() || len >= hi - lo {
                return TimelineRange::new(lo, hi);
            }
            let from = day_at(d.track, d.bounds, d.start.x);
            let mut start = orig.start + (day - from);
            if start < lo {
                start = lo;
            }
            if start + len > hi {
                start = hi - len;
            }
            TimelineRange::new(start, start + len)
        }
    }
}

/// The date axis of a timeline, from the grid's per-revision cache.
fn axis_of(app: &SheetApp, id: u32) -> Option<(f64, f64)> {
    let uid = app.session.active().map_or(0, |d| d.uid);
    app.grid.timeline_axes.get(&(uid, id)).and_then(|(_, b)| *b)
}

/// Fills the grid's timeline-axis cache for `sh`, recomputing only entries whose document revision
/// changed. Bounds come from a scan of the report's source, so they are not computed per frame.
/// A drag changes the revision without changing the source, so the axis a drag started on stays.
pub(crate) fn ensure_timeline_axes(app: &mut SheetApp, wb: &Workbook, sh: &Sheet) {
    let (uid, rev) = app.session.active().map_or((0, 0), |d| (d.uid, d.revision));
    for tl in &sh.pivot_timelines {
        if matches!(&app.grid.drag, crate::grid::Drag::Timeline(d) if d.id == tl.id) {
            continue;
        }
        if app.grid.timeline_axes.get(&(uid, tl.id)).is_some_and(|(cached, _)| *cached == rev) {
            continue;
        }
        let axis = gridcraft_engine::cmd::slicer::timeline_axis(wb, tl);
        app.grid.timeline_axes.insert((uid, tl.id), (rev, axis));
    }
    // Timelines deleted from the sheet, and documents switched away from, must not pile up.
    if app.grid.timeline_axes.len() > 256 {
        app.grid.timeline_axes.retain(|(cached, _), _| *cached == uid);
    }
}

/// The part of a timeline under `p` on this sheet, if any, and the axis a drag would map onto.
/// Used when a primary drag starts; a press on a handle wins over the band and the band over the
/// rest of the frame (the caption and the unit do not move anything).
pub(crate) fn begin_timeline_drag(app: &SheetApp, geo: &Geo, sh: &Sheet, wb: &Workbook, p: Pos2) -> Option<TimelineDrag> {
    if !geo.cells.contains(p) || crate::chartview::objects_hidden(wb) {
        return None;
    }
    let z = finite(geo.z, 1.0).clamp(0.01, 64.0);
    for tl in sh.pivot_timelines.iter().rev() {
        let frame = widget_rect(geo, sh, &tl.anchor);
        if !frame.contains(p) {
            continue;
        }
        let bounds = axis_of(app, tl.id).or_else(|| gridcraft_engine::cmd::slicer::timeline_axis(wb, tl))?;
        if !bounds.0.is_finite() || !bounds.1.is_finite() || bounds.1 <= bounds.0 {
            return None;
        }
        let lay = timeline_chrome(geo, frame);
        let handles = timeline_handles(&lay, Some(bounds), tl.range);
        let reach = 3.0 * z;
        let grab = if handles[0].expand(reach).contains(p) {
            TimelineGrab::Start
        } else if handles[1].expand(reach).contains(p) {
            TimelineGrab::End
        } else if lay.track.expand(4.0 * z).contains(p) {
            TimelineGrab::Move
        } else {
            return None;
        };
        return Some(TimelineDrag { id: tl.id, grab, start: p, track: lay.track, orig: tl.range, bounds, steps: 0 });
    }
    None
}

/// The cursor over a draggable part of a timeline: resize on a handle, move on a selected band.
/// `None` everywhere else, so the cell cursor keeps its meaning over the caption and the unit.
pub(crate) fn timeline_cursor(app: &SheetApp, geo: &Geo, sh: &Sheet, wb: &Workbook, p: Pos2) -> Option<CursorIcon> {
    if !geo.cells.contains(p) || crate::chartview::objects_hidden(wb) {
        return None;
    }
    let z = finite(geo.z, 1.0).clamp(0.01, 64.0);
    for tl in sh.pivot_timelines.iter().rev() {
        let frame = widget_rect(geo, sh, &tl.anchor);
        if !frame.contains(p) {
            continue;
        }
        // Without an axis the handles are only decoration: no resize cursor, no drag.
        let axis = axis_of(app, tl.id)?;
        let lay = timeline_chrome(geo, frame);
        let handles = timeline_handles(&lay, Some(axis), tl.range);
        if handles[0].expand(3.0 * z).contains(p) || handles[1].expand(3.0 * z).contains(p) {
            return Some(CursorIcon::ResizeHorizontal);
        }
        if tl.range.is_some() && lay.track.expand(4.0 * z).contains(p) {
            return Some(CursorIcon::Move);
        }
        return None;
    }
    None
}

/// A serial day as the ISO text `slicer.setRange` accepts.
fn iso_date(wb: &Workbook, serial: f64) -> Option<String> {
    let d = gridcraft_engine::core::date::datetime_from_serial(wb.date_system, serial)?;
    Some(format!("{:04}-{:02}-{:02}", d.year, d.month, d.day))
}

/// The command params for a dragged range.
pub(crate) fn range_params(wb: &Workbook, id: u32, range: TimelineRange) -> Option<serde_json::Value> {
    let start = iso_date(wb, range.start)?;
    let end = iso_date(wb, range.end)?;
    Some(serde_json::json!({"slicer": id, "start": start, "end": end}))
}

/// The toast text while a range is dragged: its two end dates.
pub(crate) fn range_toast(wb: &Workbook, range: TimelineRange) -> String {
    match (iso_date(wb, range.start), iso_date(wb, range.end)) {
        (Some(a), Some(b)) => format!("{a} – {b}"),
        _ => String::new(),
    }
}

fn unit_name(u: TimeUnit) -> &'static str {
    match u {
        TimeUnit::Years => "Years",
        TimeUnit::Quarters => "Quarters",
        TimeUnit::Months => "Months",
        TimeUnit::Days => "Days",
    }
}

/// Draws every pivot slicer and timeline on `sh`, in the logical (left-to-right) coordinates the
/// grid paints in. Returns one painted range per widget so a right-to-left sheet can mirror the
/// whole widget without flipping its text.
pub(crate) fn paint(app: &SheetApp, p: &Painter, geo: &Geo, wb: &Workbook, sh: &Sheet) -> Vec<crate::chartview::PaintedObject> {
    let mut painted = Vec::new();
    if crate::chartview::objects_hidden(wb) {
        return painted;
    }
    let t = theme::Tokens::get(p.ctx());
    let hover = p.ctx().input(|i| i.pointer.hover_pos()).map(|q| geo.mirror_pos(q));
    for sl in &sh.pivot_slicers {
        let frame = widget_rect(geo, sh, &sl.anchor);
        if !frame.is_finite() || !frame.intersects(p.clip_rect()) {
            continue;
        }
        let start = p.add(egui::Shape::Noop);
        let scroll = scroll_of(app, sl.id);
        paint_slicer(app, p, geo, &t, sl, frame, scroll, hover);
        painted.push(crate::chartview::PaintedObject { shapes: start.0..p.add(egui::Shape::Noop).0, rect: frame });
    }
    for tl in &sh.pivot_timelines {
        let frame = widget_rect(geo, sh, &tl.anchor);
        if !frame.is_finite() || !frame.intersects(p.clip_rect()) {
            continue;
        }
        let start = p.add(egui::Shape::Noop);
        paint_timeline(app, p, geo, &t, tl, frame);
        painted.push(crate::chartview::PaintedObject { shapes: start.0..p.add(egui::Shape::Noop).0, rect: frame });
    }
    painted
}

fn paint_slicer(app: &SheetApp, p: &Painter, geo: &Geo, t: &theme::Tokens, sl: &PivotSlicer, frame: Rect, scroll: usize, hover: Option<Pos2>) {
    let z = finite(geo.z, 1.0).clamp(0.01, 64.0);
    let lay = slicer_chrome(geo, frame, sl.columns, sl.items.len(), scroll);
    let radius = 4.0 * z;
    let look = look(&sl.style, t);
    // Card + a soft shadow, clipped by the caller.
    p.rect_filled(frame.translate(vec2(0.0, 1.5)), radius, Color32::from_black_alpha(18));
    p.rect_filled(frame, radius, t.window);
    p.rect_stroke(frame, radius, Stroke::new(1.0, look.border), StrokeKind::Inside);
    // Header band with square bottom corners.
    p.rect_filled(lay.header, radius, look.header);
    p.rect_filled(
        Rect::from_min_max(pos2(lay.header.left(), (lay.header.bottom() - radius).max(lay.header.top())), lay.header.max),
        0.0,
        look.header,
    );
    // Caption, or the field when a file carries no caption, clipped to the band.
    let caption = if sl.caption.trim().is_empty() { sl.field.as_str() } else { sl.caption.as_str() };
    let caption = elide(caption, MAX_TEXT);
    let cap_right = lay.clear.map_or(lay.header.right() - PAD * z, |c| c.left() - PAD * z);
    let cap_rect = Rect::from_min_max(lay.header.min, pos2(cap_right, lay.header.bottom()));
    let cp = p.with_clip_rect(cap_rect.intersect(p.clip_rect()));
    cp.text(lay.header.left_center() + vec2(PAD * z, 0.0), Align2::LEFT_CENTER, caption, theme::ui_bold(12.5 * z), look.header_text);
    // Clear-filter button: tinted while a filter is active.
    if let Some(c) = lay.clear {
        let filtered = !sl.items.iter().all(|i| i.selected);
        if let Some(icon) = crate::icons::from_name("filter") {
            crate::icons::paint(p, c.shrink(2.0 * z), icon, if filtered { t.accent } else { t.text_dim });
        }
    }
    // Tiles, clipped to the body so an item that does not fit is elided rather than overdrawing.
    let bp = p.with_clip_rect(lay.body.intersect(p.clip_rect()));
    let focused = app.grid.slicer_focus.filter(|(id, _)| *id == sl.id).map(|(_, i)| i);
    for &(i, tr) in &lay.tiles {
        if !tr.is_finite() || !tr.intersects(lay.body) {
            continue;
        }
        let Some(item) = sl.items.get(i) else { continue };
        let on = item.selected;
        let under = hover.is_some_and(|q| tr.contains(q));
        let fill = if on {
            look.tile_on
        } else if under {
            t.pressed
        } else {
            look.tile_off
        };
        let fg = if on { look.tile_on_text } else { look.tile_off_text };
        bp.rect_filled(tr, 3.0 * z, fill);
        bp.rect_stroke(tr, 3.0 * z, Stroke::new(1.0, if on { look.tile_on } else { look.border }), StrokeKind::Inside);
        let label = elide(&item.label(), MAX_TEXT);
        let mut text_left = tr.left() + PAD * z;
        if on {
            // A check mark on the selected tiles, drawn from the icon set (no image asset).
            let side = (12.0 * z).min(tr.width() * 0.4);
            let box_rect = Rect::from_min_size(pos2(tr.left() + 4.0 * z, tr.center().y - side / 2.0), vec2(side, side));
            if let Some(icon) = crate::icons::from_name("check") {
                crate::icons::paint(&bp, box_rect, icon, fg);
            }
            text_left = box_rect.right() + 3.0 * z;
        }
        let text_rect = Rect::from_min_max(pos2(text_left, tr.top()), pos2((tr.right() - 3.0 * z).max(text_left), tr.bottom()));
        let tp = bp.with_clip_rect(text_rect.intersect(bp.clip_rect()));
        tp.text(pos2(text_left, tr.center().y), Align2::LEFT_CENTER, label, theme::ui_font(12.0 * z), fg);
        if focused == Some(i) {
            bp.rect_stroke(tr.expand(1.0), 3.0 * z, Stroke::new(2.0, t.accent), StrokeKind::Outside);
        }
    }
    // "3–12 of 40" when the list overflows: the tiles that did not fit are elided, not silently lost.
    if let Some(f) = lay.footer {
        let first = lay.first_row.saturating_mul(sl.columns.clamp(1, 64) as usize);
        let last = lay.tiles.last().map_or(first, |(i, _)| i + 1);
        let msg = crate::i18n::current(p.ctx())
            .tr("{first}–{last} of {}")
            .replacen("{first}", &first.to_string(), 1)
            .replacen("{last}", &last.to_string(), 1)
            .replacen("{}", &lay.total.to_string(), 1);
        let fp = p.with_clip_rect(f.intersect(p.clip_rect()));
        fp.text(f.left_bottom(), Align2::LEFT_BOTTOM, msg, theme::ui_font(10.5 * z), t.text_dim);
    }
    if app.selected_chart == Some(sl.id) {
        crate::chartview::selection_frame(p, frame);
    }
}

fn paint_timeline(app: &SheetApp, p: &Painter, geo: &Geo, t: &theme::Tokens, tl: &PivotTimeline, frame: Rect) {
    let z = finite(geo.z, 1.0).clamp(0.01, 64.0);
    let lay = timeline_chrome(geo, frame);
    let radius = 4.0 * z;
    // Everything is clipped to the frame: a file can carry a frame smaller than the widget's parts.
    let fp = p.with_clip_rect(frame.intersect(p.clip_rect()));
    fp.rect_filled(frame.translate(vec2(0.0, 1.5)), radius, Color32::from_black_alpha(18));
    fp.rect_filled(frame, radius, t.window);
    fp.rect_stroke(frame, radius, Stroke::new(1.0, t.separator), StrokeKind::Inside);
    fp.rect_filled(lay.header, radius, t.accent_soft);
    fp.rect_filled(
        Rect::from_min_max(pos2(lay.header.left(), (lay.header.bottom() - radius).max(lay.header.top())), lay.header.max),
        0.0,
        t.accent_soft,
    );
    let caption = if tl.caption.trim().is_empty() { tl.field.as_str() } else { tl.caption.as_str() };
    let caption = elide(caption, MAX_TEXT);
    let cp = fp.with_clip_rect(lay.header.intersect(fp.clip_rect()));
    cp.text(lay.header.left_center() + vec2(PAD * z, 0.0), Align2::LEFT_CENTER, caption, theme::ui_bold(12.5 * z), t.accent_dark);
    // The time unit the timeline groups by ("Months", "Quarters", …).
    let up = fp.with_clip_rect(lay.unit.intersect(fp.clip_rect()));
    up.text(lay.unit.left_center(), Align2::LEFT_CENTER, crate::i18n::current(p.ctx()).tr(unit_name(tl.unit)), theme::ui_font(11.0 * z), t.text_dim);
    // The track is the field's full date axis; the selected range is a band on it. A band is only
    // drawn when the axis is known (the grid's per-revision cache), because without it its place
    // would be made up.
    let track_rect = lay.track;
    fp.rect_filled(track_rect, track_rect.height() / 2.0, t.hover);
    fp.rect_stroke(track_rect, track_rect.height() / 2.0, Stroke::new(1.0, t.separator), StrokeKind::Inside);
    let axis = axis_of(app, tl.id);
    let band = band_xs(track_rect, axis, tl.range);
    let handles = timeline_handles(&lay, axis, tl.range);
    if let Some((a, b)) = band {
        let band_rect = Rect::from_min_max(pos2(a, track_rect.top()), pos2(b, track_rect.bottom()));
        let radius = band_rect.height() / 2.0;
        fp.rect_filled(band_rect, radius, t.accent_soft);
        fp.rect_stroke(band_rect, radius, Stroke::new(1.0, t.accent), StrokeKind::Inside);
    }
    for h in handles {
        if !h.is_finite() {
            continue;
        }
        fp.rect_filled(h, h.width() / 2.0, t.accent);
        fp.rect_stroke(h, h.width() / 2.0, Stroke::new(1.0, t.window), StrokeKind::Inside);
    }
    if app.selected_chart == Some(tl.id) {
        crate::chartview::selection_outline(p, frame);
    }
}

// ------------------------------------------------------------------ state: scroll and tile focus

/// Key of a slicer's scroll offset: the document uid and the slicer id (ids are workbook-wide).
fn scroll_key(app: &SheetApp, id: u32) -> (u64, u32) {
    (app.session.active().map_or(0, |d| d.uid), id)
}

pub(crate) fn scroll_of(app: &SheetApp, id: u32) -> usize {
    app.grid.slicer_scroll.get(&scroll_key(app, id)).copied().unwrap_or(0)
}

fn set_scroll(app: &mut SheetApp, id: u32, row: usize) {
    let key = scroll_key(app, id);
    app.grid.slicer_scroll.insert(key, row);
}

/// Adjusts a slicer's scroll offset so item `index` is inside the visible window. Called when Tab
/// moves the tile focus past the edge.
fn reveal_tile(app: &mut SheetApp, sh: &Sheet, sl: &PivotSlicer, index: usize) {
    let Some(geo) = app.grid.geo.clone() else { return };
    let frame = widget_rect(&geo, sh, &sl.anchor);
    let cols = sl.columns.clamp(1, 64) as usize;
    let total_rows = sl.items.len().div_ceil(cols);
    let lay = slicer_chrome(&geo, frame, sl.columns, sl.items.len(), scroll_of(app, sl.id));
    if lay.rows == 0 {
        return;
    }
    let row = index / cols;
    let first = if row < lay.first_row {
        row
    } else if row >= lay.first_row + lay.rows {
        row.saturating_sub(lay.rows.saturating_sub(1))
    } else {
        lay.first_row
    };
    set_scroll(app, sl.id, first.min(total_rows.saturating_sub(1)));
}

/// A Tab press on a selected pivot slicer moves the tile focus instead of the cell cursor
/// (Shift+Tab goes back). Returns `false` when no slicer is selected, so Tab keeps its cell
/// meaning. Arrow keys are not used for tiles: a selected object takes them for nudging, as every
/// other on-sheet object does (and Excel does too).
pub(crate) fn tab(app: &mut SheetApp, sh: &Sheet, step: isize) -> bool {
    let Some(id) = app.selected_chart else { return false };
    let Some(sl) = sh.pivot_slicers.iter().find(|s| s.id == id) else { return false };
    if sl.items.is_empty() {
        return false;
    }
    let n = sl.items.len();
    let current = app.grid.slicer_focus.filter(|(fid, _)| *fid == id).map(|(_, i)| i);
    let next = match current {
        None => {
            if step >= 0 {
                0
            } else {
                n - 1
            }
        }
        Some(i) => (i as isize + step).rem_euclid(n as isize) as usize,
    };
    app.grid.slicer_focus = Some((id, next));
    reveal_tile(app, sh, sl, next);
    true
}

/// Enter or Space on a focused tile toggles it through `slicer.toggle` (one command, one undo
/// step). Returns `false` when no tile is focused, so the key keeps its cell meaning.
pub(crate) fn toggle_focused(app: &mut SheetApp, sh: &Sheet) -> bool {
    let Some(id) = app.selected_chart else { return false };
    let Some((_, index)) = app.grid.slicer_focus.filter(|(fid, _)| *fid == id) else { return false };
    let Some(item) = sh.pivot_slicers.iter().find(|s| s.id == id).and_then(|s| s.items.get(index)) else { return false };
    let value = item.label();
    let _ = app.run("slicer.toggle", serde_json::json!({"slicer": id, "value": value}));
    true
}

// ------------------------------------------------------------------ hit-testing

/// What a click inside a pivot slicer hit.
pub(crate) enum SlicerAction {
    /// A value tile: toggle it (`index` is its position in the item list).
    Toggle { index: usize, label: String },
    /// The clear-filter button: select every tile again.
    Clear,
}

/// The pivot slicer part under `p` on this sheet. A click on the header or the background returns
/// `None`, so the caller treats it as a select/drag of the object.
pub(crate) fn click(app: &SheetApp, geo: &Geo, sh: &Sheet, wb: &Workbook, p: Pos2) -> Option<(u32, SlicerAction)> {
    if !geo.cells.contains(p) || crate::chartview::objects_hidden(wb) {
        return None;
    }
    for sl in sh.pivot_slicers.iter().rev() {
        let frame = widget_rect(geo, sh, &sl.anchor);
        if !frame.contains(p) {
            continue;
        }
        let lay = slicer_chrome(geo, frame, sl.columns, sl.items.len(), scroll_of(app, sl.id));
        if lay.clear.is_some_and(|c| c.contains(p)) {
            return Some((sl.id, SlicerAction::Clear));
        }
        for &(i, tr) in &lay.tiles {
            if tr.contains(p)
                && let Some(item) = sl.items.get(i)
            {
                return Some((sl.id, SlicerAction::Toggle { index: i, label: item.label() }));
            }
        }
        return None; // header/background: select or drag the widget
    }
    None
}

/// The wheel over a pivot slicer whose tile list overflows scrolls the tiles, not the sheet.
/// Returns whether the delta was used.
pub(crate) fn wheel(app: &mut SheetApp, sh: &Sheet, geo: &Geo, p: Pos2, delta: egui::Vec2) -> bool {
    if delta == egui::Vec2::ZERO {
        return false;
    }
    for sl in sh.pivot_slicers.iter().rev() {
        let frame = widget_rect(geo, sh, &sl.anchor);
        if !frame.contains(p) {
            continue;
        }
        let lay = slicer_chrome(geo, frame, sl.columns, sl.items.len(), scroll_of(app, sl.id));
        if lay.rows == 0 || lay.total_rows <= lay.rows {
            return false;
        }
        let max_first = lay.total_rows - lay.rows;
        let step = if delta.y > 0.0 { -1 } else { 1 };
        let next = (lay.first_row as isize + step).clamp(0, max_first as isize) as usize;
        set_scroll(app, sl.id, next);
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> Rect {
        Rect::from_min_max(pos2(100.0, 50.0), pos2(300.0, 60.0))
    }

    fn drag(grab: TimelineGrab, start_x: f32, orig: Option<TimelineRange>, bounds: (f64, f64)) -> TimelineDrag {
        TimelineDrag { id: 7, grab, start: pos2(start_x, 55.0), track: track(), orig, bounds, steps: 0 }
    }

    #[test]
    fn a_start_handle_follows_the_pointer_and_cannot_pass_the_end() {
        // 200 screen points span 100 days: two points a day.
        let mut d = drag(TimelineGrab::Start, 100.0, Some(TimelineRange::new(10.0, 50.0)), (0.0, 100.0));
        assert_eq!(dragged_range(&d, pos2(150.0, 55.0)), TimelineRange::new(25.0, 50.0));
        // Past the axis's right end the start lands on the other end: one day is still a range.
        assert_eq!(dragged_range(&d, pos2(400.0, 55.0)), TimelineRange::new(50.0, 50.0));
        // Left of the axis it stops at the first day.
        d.orig = Some(TimelineRange::new(10.0, 50.0));
        assert_eq!(dragged_range(&d, pos2(0.0, 55.0)), TimelineRange::new(0.0, 50.0));
    }

    #[test]
    fn an_end_handle_clamps_to_the_start() {
        let d = drag(TimelineGrab::End, 300.0, Some(TimelineRange::new(10.0, 50.0)), (0.0, 100.0));
        assert_eq!(dragged_range(&d, pos2(200.0, 55.0)), TimelineRange::new(10.0, 50.0));
        assert_eq!(dragged_range(&d, pos2(100.0, 55.0)), TimelineRange::new(10.0, 10.0));
        assert_eq!(dragged_range(&d, pos2(400.0, 55.0)), TimelineRange::new(10.0, 100.0));
    }

    #[test]
    fn the_band_slides_without_changing_length_or_leaving_the_axis() {
        // A 20-day band grabbed at day 20 and carried to day 90 would end past the axis: it stops
        // at the last date and keeps its length.
        let d = drag(TimelineGrab::Move, 140.0, Some(TimelineRange::new(10.0, 30.0)), (0.0, 100.0));
        assert_eq!(dragged_range(&d, pos2(280.0, 55.0)), TimelineRange::new(80.0, 100.0));
        // Carried left it stops at the first date.
        assert_eq!(dragged_range(&d, pos2(0.0, 55.0)), TimelineRange::new(0.0, 20.0));
        // A band exactly as wide as the axis has nowhere to go.
        let full = drag(TimelineGrab::Move, 140.0, Some(TimelineRange::new(0.0, 100.0)), (0.0, 100.0));
        assert_eq!(dragged_range(&full, pos2(260.0, 55.0)), TimelineRange::new(0.0, 100.0));
        // With no stored range the band is the whole axis and cannot slide either.
        let none = drag(TimelineGrab::Move, 140.0, None, (0.0, 100.0));
        assert_eq!(dragged_range(&none, pos2(260.0, 55.0)), TimelineRange::new(0.0, 100.0));
    }

    #[test]
    fn handles_on_an_axis_with_no_span_stay_on_its_one_day() {
        let d = drag(TimelineGrab::End, 100.0, None, (45658.0, 45658.0));
        assert_eq!(dragged_range(&d, pos2(300.0, 55.0)), TimelineRange::new(45_658.0, 45_658.0));
        // A zero-width track cannot divide the axis; the first day is the answer, not NaN.
        let mut flat = drag(TimelineGrab::Start, 100.0, None, (10.0, 20.0));
        flat.track = Rect::from_min_max(pos2(100.0, 50.0), pos2(100.0, 60.0));
        assert_eq!(dragged_range(&flat, pos2(150.0, 55.0)), TimelineRange::new(10.0, 20.0));
    }

    #[test]
    fn a_dragged_range_writes_iso_dates_for_the_command() {
        let wb = Workbook::default();
        let params = range_params(&wb, 3, TimelineRange::new(45_658.0, 45_716.0)).unwrap();
        assert_eq!(params["slicer"], 3);
        assert_eq!(params["start"], "2025-01-01");
        assert_eq!(params["end"], "2025-02-28");
        assert_eq!(range_toast(&wb, TimelineRange::new(45_658.0, 45_716.0)), "2025-01-01 – 2025-02-28");
    }
}
