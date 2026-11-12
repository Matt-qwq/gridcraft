//! The worksheet grid: virtualised painting of only the visible cells, frozen panes,
//! headers, selection, fill handle, column/row resizing, text overflow, borders, conditional
//! formats, tables, comments, filters, sparklines, floating charts/pictures/shapes and the
//! in-cell editor.

use std::sync::Arc;

use egui::{Align2, Color32, CursorIcon, FontId, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use gridcraft_engine::cf::{CfCache, CfLook};
use gridcraft_engine::core::{CellRef, MAX_COLS, MAX_ROWS, RangeRef, Value, col_to_letters};
use gridcraft_engine::model::{BorderLine, BorderStyle, Color, HAlign, LineIndex, PatternType, Sheet, Style, Underline, VAlign, Workbook};
use serde_json::json;

use crate::SheetApp;
use crate::editor::{EditState, REF_COLORS};
use crate::theme::{self, Tokens};

pub const COL_HEADER_H: f32 = 21.0;
/// Sheet units are 96-dpi pixels (as in XLSX), drawn one for one on screen, so a default 20 px row
/// is 20 pt tall and 11 pt text sits in it exactly as it does in Excel at 100%. (This used to be
/// 0.8, which made the grid 20% denser than Excel while the text stayed 11 pt: the glyphs filled
/// the row and every cell looked cramped.)
pub const DISPLAY_SCALE: f32 = 1.0;

/// The zoom percentages a Ctrl+wheel notch steps between: four per doubling (about 20% apart),
/// so 25, 50, 100, 200 and 400 are on it. Excel steps 15 points a notch instead; the status bar's
/// Zoom In / Zoom Out buttons keep their 10% steps.
pub const WHEEL_ZOOM_LADDER: [u16; 22] = [10, 12, 15, 18, 21, 25, 30, 35, 42, 50, 60, 70, 85, 100, 120, 140, 170, 200, 240, 280, 340, 400];

/// Seconds without Ctrl+wheel input after which part of a notch turned so far is forgotten.
const WHEEL_ZOOM_IDLE: f64 = 0.75;

/// The zoom one Ctrl+wheel notch goes to from `zoom`: the next step on [`WHEEL_ZOOM_LADDER`] in
/// that direction, skipping a step less than 5% away (121% out goes to 100%, not 120%), but never
/// 100% or the 10% / 400% limits (99% or 101% towards 100% goes to 100%).
pub fn wheel_zoom_step(zoom: u16, zoom_in: bool) -> u16 {
    let zoom = zoom.clamp(10, 400);
    let z = u32::from(zoom);
    let far = |l: u32| if zoom_in { l * 100 >= z * 105 } else { l * 105 <= z * 100 };
    let mut ahead = WHEEL_ZOOM_LADDER.iter().copied().filter(|&l| if zoom_in { l > zoom } else { l < zoom }).collect::<Vec<_>>();
    if !zoom_in {
        ahead.reverse();
    }
    let limit = ahead.last().copied().unwrap_or(zoom);
    ahead.into_iter().find(|&l| l == 100 || far(u32::from(l))).unwrap_or(limit)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Drag {
    #[default]
    None,
    Select,
    Rows(u32),
    Cols(u32),
    /// Reordering one contiguous whole-column selection by its header.
    MoveCols {
        source: RangeRef,
        before: Option<u32>,
    },
    ResizeCol {
        col: u32,
        start: f32,
        orig: f32,
    },
    ResizeRow {
        row: u32,
        start: f32,
        orig: f32,
    },
    /// Dragging the fill handle; `menu` when the right button drags it (the fill menu opens on release).
    Fill {
        target: Option<RangeRef>,
        menu: bool,
    },
    /// Dragging a freeze handle: `cols` for the column bar, `to` the line count it would freeze.
    Freeze {
        cols: bool,
        to: Option<u32>,
    },
    /// Moving the selection by its border (cut & paste).
    Move {
        grab: CellRef,
        to: Option<CellRef>,
    },
    /// Moving or resizing a floating object (chart, picture, shape, slicer). `orig` is the frame
    /// when the drag began, `handle` the edge grabbed (`None` moves the whole object), and `steps`
    /// the drag's own undo entries so far (`0`: the first move is about to be recorded).
    Object(ObjectDrag),
    /// Dragging one end of a timeline's selected date range, or sliding the range itself.
    Timeline(crate::slicer_view::TimelineDrag),
}

/// A drag of a floating object in progress.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectDrag {
    pub kind: &'static str,
    pub id: u32,
    /// Where the pointer pressed, in sheet units (`Geo::mirror_pos` applied).
    pub start: Pos2,
    pub orig: crate::chartview::Frame,
    pub handle: Option<crate::chartview::Handle>,
    /// Undo entries this drag has recorded, so only its first one starts a new entry.
    pub steps: u32,
}

impl ObjectDrag {
    pub fn new(kind: &'static str, id: u32, start: Pos2, orig: crate::chartview::Frame, handle: Option<crate::chartview::Handle>) -> ObjectDrag {
        ObjectDrag { kind, id, start, orig, handle, steps: 0 }
    }
    fn edges(&self) -> crate::chartview::Edges {
        self.handle.map_or(crate::chartview::Edges::MOVE, |h| h.edges())
    }
}

impl Drag {
    /// Whether this drag holds a floating object or a timeline range: both coalesce their moves
    /// into one undo entry and both are cancelled by Esc.
    fn is_object(&self) -> bool {
        matches!(self, Drag::Object(_) | Drag::Timeline(_))
    }
    /// Undo entries this drag has recorded so far.
    fn steps(&self) -> u32 {
        match self {
            Drag::Object(d) => d.steps,
            Drag::Timeline(d) => d.steps,
            _ => 0,
        }
    }
}

#[derive(Default)]
pub struct GridState {
    pub drag: Drag,
    pub ensure_visible: bool,
    /// With `ensure_visible`: scroll to the moving end of the selection instead of the active cell
    /// (Shift/Ctrl+Shift+arrows keep the active cell on the anchor while the far corner moves).
    pub reveal_moving_end: bool,
    pub last_title: Option<String>,
    pub drag_select: bool,
    pub rect: Option<Rect>,
    pub cells_rect: Option<Rect>,
    /// Column and row of the clicked header cell, and where to open the menu.
    pub filter_menu: Option<(u32, u32, Pos2)>,
    pub context_menu: Option<Pos2>,
    /// The frame a right-click opened `context_menu` in: that click's own press doesn't close it.
    pub context_menu_frame: u64,
    /// After a right-drag of the fill handle: the source, the dragged range and where to open the menu.
    pub fill_menu: Option<(RangeRef, RangeRef, Pos2)>,
    /// The right-clicked chart, picture, shape or slicer: where to open its menu, its kind and id.
    pub object_menu: Option<(Pos2, &'static str, u32)>,
    pub last_click: Option<(CellRef, f64)>,
    pub hover_cell: Option<CellRef>,
    pub last_autosave: f64,
    pub last_border: String,
    pub last_fill: String,
    pub last_font_color: String,
    pub trace: Option<serde_json::Value>,
    pub renaming_tab: Option<usize>,
    pub rename_text: String,
    pub list_picker: Option<CellRef>,
    /// Open task pane: comments, watch, selection, formatChart.
    pub pane: Option<String>,
    /// Ink stroke being drawn (sheet points).
    pub ink: Vec<[f32; 2]>,
    /// Page ranges for Page Break Preview, cached per (doc uid, revision, sheet).
    pub pages: Option<((u64, u64, usize), Vec<(RangeRef, u32)>)>,
    /// The PivotTable Fields pane's `pivot.fields` answer, cached per (doc uid, revision, pivot):
    /// it scans the whole source list, far too slow to repeat every frame (#2113).
    pub pivot_fields: Option<((u64, u64, String), serde_json::Value)>,
    /// The grid's line geometry, cached per (doc uid, revision, sheet): a sheet with a custom
    /// height on every row would otherwise rebuild its prefix sums on every frame.
    pub line_geo: Option<LineGeo>,
    pub header_menu: Option<(Pos2, bool)>,
    /// Where the keyboard is in the open context menu (cell or row/column header), if any.
    pub menu_nav: Option<MenuNav>,
    /// Page Layout view pagination, cached per (doc uid, revision, sheet, rows, columns).
    pub paper: Option<crate::paper_view::PaperCache>,
    /// Header or footer being edited in place (Page Layout view).
    pub hf_edit: Option<crate::paper_view::HfEdit>,
    /// A page margin being dragged on a ruler (Page Layout view); committed on release.
    pub ruler_drag: Option<crate::paper_view::RulerDrag>,
    /// Insert › Header & Footer: start editing the first page's header once Page Layout view is up.
    pub hf_start: bool,
    /// Pivot slicer tile scroll offsets, by (document uid, slicer id): the first tile row shown.
    /// The widget elides tiles that do not fit; the wheel over a slicer moves this instead of the sheet.
    pub slicer_scroll: std::collections::HashMap<(u64, u32), usize>,
    /// The date axis of each timeline, by (document uid, timeline id), with the revision it was
    /// computed at: a scan of the report's source, cached until the document changes.
    pub timeline_axes: std::collections::HashMap<(u64, u32), (u64, Option<(f64, f64)>)>,
    /// The tile of the selected pivot slicer that Tab last focused, if any.
    pub slicer_focus: Option<(u32, usize)>,
    /// The grid geometry of the last frame (hit-testing for agents and tests).
    pub geo: Option<Geo>,
    /// The horizontal scrollbar's track in the sheet tab bar (hit-testing for tests).
    pub hscroll: Option<Rect>,
    /// Ctrl+wheel zoom in progress (see [`WheelZoom`]).
    pub wheel_zoom: Option<WheelZoom>,
    /// A one-finger touch drag in progress: it pans the sheet instead of selecting.
    pub touch_pan: bool,
    /// `review.checkAccessibility`'s issue count for the status bar, cached per document revision:
    /// the check walks every sheet's cells and objects, far too slow to repeat every frame.
    pub a11y: Option<((u64, u64), usize)>,
}

/// The grid's line geometry: prefix sums over custom row heights and column widths, and the
/// outline axes built from the same maps. It changes only with the document (an edit, undo/redo, a
/// sheet switch), so [`GridState::line_geo_for`] keeps it for as long as the document revision and
/// sheet stay the same — a sheet with a custom height on every row would otherwise rebuild it once
/// or twice every frame.
#[derive(Clone)]
pub struct LineGeo {
    /// (document uid, document revision, sheet index).
    key: (u64, u64, usize),
    ri: Arc<LineIndex>,
    ci: Arc<LineIndex>,
    row_outline: Arc<crate::outline::Axis>,
    col_outline: Arc<crate::outline::Axis>,
}

impl LineGeo {
    /// The line geometry of `sh`, built from its row and column maps.
    fn of(sh: &Sheet) -> LineGeo {
        LineGeo {
            key: (0, 0, 0),
            ri: Arc::new(LineIndex::new(&sh.rows, sh.default_row_height, MAX_ROWS)),
            ci: Arc::new(LineIndex::new(&sh.cols, sh.default_col_width, MAX_COLS)),
            row_outline: Arc::new(crate::outline::Axis::new(&sh.rows)),
            col_outline: Arc::new(crate::outline::Axis::new(&sh.cols)),
        }
    }
    /// The four `Arc`s, in [`Geo::new_with_lines`] order.
    fn into_arcs(self) -> (Arc<LineIndex>, Arc<LineIndex>, Arc<crate::outline::Axis>, Arc<crate::outline::Axis>) {
        (self.ri, self.ci, self.row_outline, self.col_outline)
    }
}

impl GridState {
    /// The line geometry for `key` — (document uid, revision, sheet) — rebuilt from `sh` when the
    /// document changed and reused (cloned `Arc`s) otherwise. The returned value owns its `Arc`s,
    /// so the borrow of `sh` ends with this call.
    pub fn line_geo_for(&mut self, key: (u64, u64, usize), sh: &Sheet) -> LineGeo {
        if let Some(cached) = self.line_geo.as_ref().filter(|g| g.key == key) {
            return cached.clone();
        }
        let g = LineGeo { key, ..LineGeo::of(sh) };
        self.line_geo = Some(g.clone());
        g
    }
}

/// Ctrl+wheel zoom for one (doc uid, sheet). A fine-grained wheel sends parts of a notch, so the
/// turning is counted in notches and each whole one steps [`WHEEL_ZOOM_LADDER`] at once.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WheelZoom {
    pub place: (u64, usize),
    /// Turned so far towards the next step, in notches (+ in, − out).
    pub notches: f32,
    /// When `notches` last changed.
    pub time: f64,
}

impl WheelZoom {
    /// The state for `place`: fresh for another document or sheet.
    fn at(app: &SheetApp, place: (u64, usize), now: f64) -> WheelZoom {
        app.grid.wheel_zoom.filter(|w| w.place == place).unwrap_or(WheelZoom { place, notches: 0.0, time: now })
    }
}

/// Geometry of the grid for one frame (handles zoom and frozen panes).
#[derive(Clone)]
pub struct Geo {
    pub rect: Rect,
    pub cells: Rect,
    pub z: f32,
    pub header_w: f32,
    pub header_h: f32,
    /// The outline bands (grouped rows / columns): left of the row headers and above the column
    /// headers; 0 without an outline (see `outline`).
    pub band_w: f32,
    pub band_h: f32,
    pub row_outline: Arc<crate::outline::Axis>,
    pub col_outline: Arc<crate::outline::Axis>,
    pub fr: u32,
    pub fc: u32,
    pub frozen_w: f32,
    pub frozen_h: f32,
    pub scroll: egui::Vec2,
    /// Sheet Right-to-Left: everything is laid out left-to-right as usual (these "logical"
    /// coordinates are what `x`, `cell_rect`, `col_at`… return) and mirrored across `rect`'s
    /// vertical centre line on screen: painted shapes by `mirror_painted`, widgets by
    /// `mirror_rect`/`mirror_pos`, and pointer input back by `mirror_pos` (an involution).
    pub rtl: bool,
    /// Prefix sums over custom row heights / column widths, cached per document revision.
    ri: Arc<LineIndex>,
    ci: Arc<LineIndex>,
    /// Page Layout view: cells sit on pages (`scroll` is then in page-layout units).
    pub paper: Option<Arc<gridcraft_engine::paper::PageGrid>>,
}

impl Geo {
    /// The uncached constructor: builds the line geometry and wraps it. Per-frame callers go
    /// through `GridState::line_geo_for` and [`Geo::new_with_lines`] instead.
    pub fn new(sh: &Sheet, rect: Rect, scroll: egui::Vec2) -> Geo {
        let (ri, ci, row_outline, col_outline) = LineGeo::of(sh).into_arcs();
        Geo::new_with_lines(sh, rect, scroll, ri, ci, row_outline, col_outline)
    }
    /// The geometry from line geometry already built (the per-revision cache holds it across
    /// frames): prefix sums over custom row heights and column widths and the outline axes.
    pub fn new_with_lines(
        sh: &Sheet,
        rect: Rect,
        scroll: egui::Vec2,
        ri: Arc<LineIndex>,
        ci: Arc<LineIndex>,
        row_outline: Arc<crate::outline::Axis>,
        col_outline: Arc<crate::outline::Axis>,
    ) -> Geo {
        let zoom = (sh.zoom as f32 / 100.0).clamp(0.1, 4.0);
        let z = zoom * DISPLAY_SCALE;
        let header_h = if sh.show_headings { (COL_HEADER_H * zoom.max(0.75)).round() } else { 0.0 };
        let last_row = ri.at((scroll.y + rect.height() / z) as f64 + 200.0) + 1;
        let digits = (last_row as f32).log10().floor() as i32 + 1;
        let header_w = if sh.show_headings { ((digits.max(2) as f32 * 7.0 + 14.0) * zoom.max(0.75)).round() } else { 0.0 };
        let (fr, fc) = sh.freeze.unwrap_or((0, 0));
        let frozen_w = (ci.start(fc) as f32) * z;
        let frozen_h = (ri.start(fr) as f32) * z;
        let mut band_w = crate::outline::band(row_outline.levels, zoom);
        let mut band_h = crate::outline::band(col_outline.levels, zoom);
        // Without headings the level buttons still need a corner to sit in.
        if row_outline.levels > 0 && header_h == 0.0 {
            band_h = band_h.max(crate::outline::lane(zoom) + 3.0);
        }
        if col_outline.levels > 0 && header_w == 0.0 {
            band_w = band_w.max(crate::outline::lane(zoom) + 3.0);
        }
        let cells = Rect::from_min_max(pos2(rect.left() + band_w + header_w, rect.top() + band_h + header_h), rect.max);
        Geo {
            rect,
            cells,
            z,
            header_w,
            header_h,
            band_w,
            band_h,
            row_outline,
            col_outline,
            fr,
            fc,
            frozen_w,
            frozen_h,
            scroll,
            rtl: sh.right_to_left,
            ri,
            ci,
            paper: None,
        }
    }
    /// Left edge of the row headers (right of the outline band).
    pub fn head_left(&self) -> f32 {
        self.cells.left() - self.header_w
    }
    /// Top edge of the column headers (below the outline band).
    pub fn head_top(&self) -> f32 {
        self.cells.top() - self.header_h
    }
    /// Page Layout view geometry: `rect` excludes the rulers; cells are drawn at the print scale
    /// on pages laid out by `grid`; frozen panes don't apply (as in Excel).
    pub fn paged(sh: &Sheet, rect: Rect, scroll: egui::Vec2, grid: Arc<gridcraft_engine::paper::PageGrid>) -> Geo {
        let (ri, ci, row_outline, col_outline) = LineGeo::of(sh).into_arcs();
        Geo::paged_with_lines(sh, rect, scroll, grid, ri, ci, row_outline, col_outline)
    }
    /// [`Geo::paged`] from line geometry already built (the per-revision cache).
    #[allow(clippy::too_many_arguments)]
    pub fn paged_with_lines(
        sh: &Sheet,
        rect: Rect,
        scroll: egui::Vec2,
        grid: Arc<gridcraft_engine::paper::PageGrid>,
        ri: Arc<LineIndex>,
        ci: Arc<LineIndex>,
        row_outline: Arc<crate::outline::Axis>,
        col_outline: Arc<crate::outline::Axis>,
    ) -> Geo {
        let mut g = Geo::new_with_lines(sh, rect, scroll, ri, ci, row_outline, col_outline);
        let zoom = (sh.zoom as f32 / 100.0).clamp(0.1, 4.0);
        g.z = zoom * DISPLAY_SCALE * grid.scale.clamp(0.1, 4.0);
        (g.fr, g.fc, g.frozen_w, g.frozen_h) = (0, 0, 0.0, 0.0);
        g.paper = Some(grid);
        g
    }
    /// The point is over cells (in Page Layout view: on a page, inside its margins).
    pub fn in_cells(&self, p: Pos2) -> bool {
        if !self.cells.contains(p) {
            return false;
        }
        match &self.paper {
            Some(pg) => pg.on_cells(
                self.scroll.x as f64 + ((p.x - self.cells.left()) / self.z) as f64,
                self.scroll.y as f64 + ((p.y - self.cells.top()) / self.z) as f64,
            ),
            None => true,
        }
    }
    /// Scroll offset (in `scroll` units) at which `row` reaches the top of the scrolled area.
    pub fn scroll_top_of(&self, sh: &Sheet, row: u32) -> f32 {
        match &self.paper {
            Some(pg) => pg.row_y(row) as f32,
            None => (sh.row_top(row) - sh.row_top(self.fr)) as f32,
        }
    }
    /// Scroll offset (in `scroll` units) at which `col` reaches the left of the scrolled area.
    pub fn scroll_left_of(&self, sh: &Sheet, col: u32) -> f32 {
        match &self.paper {
            Some(pg) => pg.col_x(col) as f32,
            None => (sh.col_left(col) - sh.col_left(self.fc)) as f32,
        }
    }
    /// Screen x for a logical x (and back): identity unless the sheet is right-to-left.
    pub fn mirror_x(&self, x: f32) -> f32 {
        if self.rtl { self.rect.left() + self.rect.right() - x } else { x }
    }
    pub fn mirror_pos(&self, p: Pos2) -> Pos2 {
        pos2(self.mirror_x(p.x), p.y)
    }
    pub fn mirror_rect(&self, r: Rect) -> Rect {
        if self.rtl { Rect::from_min_max(pos2(self.mirror_x(r.right()), r.top()), pos2(self.mirror_x(r.left()), r.bottom())) } else { r }
    }
    /// On-screen rectangle of a cell (`cell_rect` mirrored for right-to-left sheets).
    pub fn screen_cell_rect(&self, sh: &Sheet, c: CellRef) -> Rect {
        self.mirror_rect(self.cell_rect(sh, c))
    }
    pub fn x(&self, _sh: &Sheet, col: u32) -> f32 {
        if let Some(pg) = &self.paper {
            return self.cells.left() + (pg.col_x(col) - self.scroll.x as f64) as f32 * self.z;
        }
        if col < self.fc {
            self.cells.left() + self.ci.start(col) as f32 * self.z
        } else {
            self.cells.left() + self.frozen_w + (self.ci.start(col) as f32 - self.ci.start(self.fc) as f32 - self.scroll.x) * self.z
        }
    }
    pub fn y(&self, _sh: &Sheet, row: u32) -> f32 {
        if let Some(pg) = &self.paper {
            return self.cells.top() + (pg.row_y(row) - self.scroll.y as f64) as f32 * self.z;
        }
        if row < self.fr {
            self.cells.top() + self.ri.start(row) as f32 * self.z
        } else {
            self.cells.top() + self.frozen_h + (self.ri.start(row) as f32 - self.ri.start(self.fr) as f32 - self.scroll.y) * self.z
        }
    }
    pub fn cell_rect(&self, sh: &Sheet, c: CellRef) -> Rect {
        Rect::from_min_max(
            pos2(self.x(sh, c.col), self.y(sh, c.row)),
            pos2(self.x(sh, c.col) + sh.col_width(c.col) * self.z, self.y(sh, c.row) + sh.row_height(c.row) * self.z),
        )
    }
    pub fn range_rect(&self, sh: &Sheet, r: RangeRef) -> Rect {
        let a = self.cell_rect(sh, r.start);
        let b = self.cell_rect(sh, r.end);
        Rect::from_min_max(a.min, b.max)
    }
    pub fn col_at(&self, _sh: &Sheet, x: f32) -> u32 {
        if let Some(pg) = &self.paper {
            return pg.col_at(self.scroll.x as f64 + ((x - self.cells.left()) / self.z) as f64);
        }
        let dx = (x - self.cells.left()) / self.z;
        if dx < self.frozen_w / self.z {
            self.ci.at(dx.max(0.0) as f64)
        } else {
            self.ci.at(self.ci.start(self.fc) + self.scroll.x as f64 + (dx - self.frozen_w / self.z) as f64)
        }
    }
    pub fn row_at(&self, _sh: &Sheet, y: f32) -> u32 {
        if let Some(pg) = &self.paper {
            return pg.row_at(self.scroll.y as f64 + ((y - self.cells.top()) / self.z) as f64);
        }
        let dy = (y - self.cells.top()) / self.z;
        if dy < self.frozen_h / self.z {
            self.ri.at(dy.max(0.0) as f64)
        } else {
            self.ri.at(self.ri.start(self.fr) + self.scroll.y as f64 + (dy - self.frozen_h / self.z) as f64)
        }
    }
    pub fn cell_at(&self, sh: &Sheet, p: Pos2) -> CellRef {
        CellRef::new(self.row_at(sh, p.y), self.col_at(sh, p.x))
    }
    /// Visible scrolled rows (from the first visible row after the frozen ones).
    pub(crate) fn scroll_rows(&self, _sh: &Sheet) -> (u32, u32) {
        let top = self.ri.start(self.fr) + self.scroll.y as f64;
        let r0 = self.ri.at(top).max(self.fr);
        let r1 = self.ri.at(top + ((self.cells.height() - self.frozen_h) / self.z) as f64).min(MAX_ROWS - 1);
        (r0, r1)
    }
    pub(crate) fn scroll_cols(&self, _sh: &Sheet) -> (u32, u32) {
        let left = self.ci.start(self.fc) + self.scroll.x as f64;
        let c0 = self.ci.at(left).max(self.fc);
        let c1 = self.ci.at(left + ((self.cells.width() - self.frozen_w) / self.z) as f64).min(MAX_COLS - 1);
        (c0, c1)
    }
    /// Screen areas of the panes (frozen rows/columns or the scrolling part) that hold part of `r`.
    /// The panes share one coordinate system, so a range that is not clipped to these is also drawn
    /// over the frozen rows/columns when it is scrolled under them.
    pub fn range_panes(&self, r: RangeRef) -> Vec<Rect> {
        let split = pos2(self.cells.left() + self.frozen_w, self.cells.top() + self.frozen_h);
        let mut xs = Vec::new();
        if r.start.col < self.fc {
            xs.push((self.cells.left(), split.x));
        }
        if r.end.col >= self.fc {
            xs.push((split.x, self.cells.right()));
        }
        let mut ys = Vec::new();
        if r.start.row < self.fr {
            ys.push((self.cells.top(), split.y));
        }
        if r.end.row >= self.fr {
            ys.push((split.y, self.cells.bottom()));
        }
        ys.iter().flat_map(|&(y0, y1)| xs.iter().map(move |&(x0, x1)| Rect::from_min_max(pos2(x0, y0), pos2(x1, y1)))).collect()
    }
    /// True when cell `c` is in the scrolling pane but scrolled entirely under the frozen rows or columns.
    pub fn hidden_under_frozen(&self, sh: &Sheet, c: CellRef) -> bool {
        let r = self.cell_rect(sh, c);
        (self.fc > 0 && c.col >= self.fc && r.right() <= self.cells.left() + self.frozen_w)
            || (self.fr > 0 && c.row >= self.fr && r.bottom() <= self.cells.top() + self.frozen_h)
    }
}

/// Resolved look of a cell for painting.
struct Look {
    style: Style,
    fill: Option<Color32>,
    text_color: Color32,
}

fn col32(c: Color, wb: &Workbook, default: Color32) -> Color32 {
    c.resolve(&wb.theme).map(theme::color32).unwrap_or(default)
}

fn numfmt_color(c: gridcraft_engine::numfmt::FormatColor) -> Color32 {
    c.rgb().map(|[r, g, b]| Color32::from_rgb(r, g, b)).unwrap_or(Color32::BLACK)
}

/// Last column of a Center Across Selection run starting at `start`: it extends over following columns (up to `limit`) while `continues` holds.
fn center_across_end(start: u32, limit: u32, continues: impl Fn(u32) -> bool) -> u32 {
    let mut end = start;
    while end < limit && end + 1 < MAX_COLS && continues(end + 1) {
        end += 1;
    }
    end
}

fn look(wb: &Workbook, si: usize, sh: &Sheet, c: CellRef, cf: &mut CfCache) -> (Look, Option<CfLook>) {
    let mut style = wb.styles.get(sh.style_id(c)).clone();
    if let Some(t) = sh.table_at(c) {
        style = gridcraft_engine::tables::table_cell_style(wb, si, t, c);
    }
    let cfl = cf.look(wb, si, c);
    if let Some(l) = &cfl
        && let Some(st) = &l.style
    {
        if st.fill.pattern != PatternType::None {
            style.fill = st.fill;
        }
        if st.font.color != Color::Auto {
            style.font.color = st.font.color;
        }
        if st.font.bold {
            style.font.bold = true;
        }
        if st.font.italic {
            style.font.italic = true;
        }
        if st.font.underline != Underline::None {
            style.font.underline = st.font.underline;
        }
        if st.font.strike {
            style.font.strike = true;
        }
        // In a differential style General means "not set" (#1163).
        if st.num_fmt.as_str() != "General" {
            style.num_fmt = st.num_fmt.clone();
        }
        for (dst, src) in [
            (&mut style.borders.left, st.borders.left),
            (&mut style.borders.right, st.borders.right),
            (&mut style.borders.top, st.borders.top),
            (&mut style.borders.bottom, st.borders.bottom),
        ] {
            if !src.is_none() {
                *dst = src;
            }
        }
    }
    let mut fill = match style.fill.pattern {
        PatternType::None => None,
        PatternType::Solid => style.fill.fg.resolve(&wb.theme).map(theme::color32),
        _ => {
            // Patterns: approximate with a blend of foreground and background.
            let fg = style.fill.fg.resolve(&wb.theme).unwrap_or([0, 0, 0]);
            let bg = style.fill.bg.resolve(&wb.theme).unwrap_or([255, 255, 255]);
            Some(Color32::from_rgb(
                ((fg[0] as u16 + bg[0] as u16 * 3) / 4) as u8,
                ((fg[1] as u16 + bg[1] as u16 * 3) / 4) as u8,
                ((fg[2] as u16 + bg[2] as u16 * 3) / 4) as u8,
            ))
        }
    };
    if let Some(l) = &cfl
        && let Some(rgb) = l.scale_fill
    {
        fill = Some(theme::color32(rgb));
    }
    let text_color = col32(style.font.color, wb, Color32::BLACK);
    (Look { style, fill, text_color }, cfl)
}

/// Text for a value with General narrowing to the available width.
fn display_text(
    wb: &Workbook,
    sh: &Sheet,
    c: CellRef,
    v: &Value,
    st: &Style,
    width_px: f32,
    char_w: f32,
) -> (String, Option<Color32>, bool, Option<(char, usize)>) {
    if sh.show_formulas
        && let Some(f) = sh.cell(c).and_then(|x| x.formula)
    {
        return (format!("={}", f.text), None, false, None);
    }
    if !sh.show_zeros && v.as_f64() == Some(0.0) {
        return (String::new(), None, false, None);
    }
    let code = st.num_fmt.as_str();
    // A checkbox cell draws TRUE/FALSE (and empty) as a box; other values show as General.
    if code == "checkbox" && matches!(v, Value::Bool(_) | Value::Empty) {
        return (String::new(), None, false, None);
    }
    if code == "General"
        && let Value::Number(n) = v
    {
        let max_chars = ((width_px - 4.0) / char_w).floor().max(1.0) as usize;
        return match gridcraft_engine::numfmt::format_general_fit(*n, max_chars.min(11)) {
            Some(t) => (t, None, true, None),
            None => ("#".repeat(max_chars.max(1)), None, true, None),
        };
    }
    let f = gridcraft_engine::display::format(v, code, wb);
    (f.text, f.color.map(numfmt_color), f.numeric, f.fill)
}

pub fn show(app: &mut SheetApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let available = ui.available_rect_before_wrap();
    // A right-to-left sheet has its vertical scrollbar on the left.
    let rtl = app.session.active().and_then(|d| d.wb.active()).is_some_and(|s| s.right_to_left);
    let bar = 14.0_f32.min(available.width());
    let rect = if rtl {
        Rect::from_min_max(pos2(available.left() + bar, available.top()), available.max)
    } else {
        Rect::from_min_max(available.min, pos2(available.right() - bar, available.bottom()))
    };
    let resp = ui.allocate_rect(rect, Sense::click_and_drag());
    app.grid.rect = Some(rect);
    let Some(d) = app.session.active() else { return };
    let wb: Arc<Workbook> = d.wb.clone();
    let si = wb.active_sheet;
    let Some(sh) = wb.sheet(si) else { return };
    let sel = d.selection.clone();
    let place = (d.uid, si);
    // The line geometry changes only with the document: the revision is part of its cache key.
    let line_key = (d.uid, d.revision, si);
    // The timeline axis is a source scan; the grid caches it per revision before painting.
    crate::slicer_view::ensure_timeline_axes(app, &wb, sh);
    let mut view = app.view();
    // Page Layout view: pages on a desk, rulers above and left of the headings.
    let paper = sh.view == gridcraft_engine::model::SheetViewMode::PageLayout;
    let ruler = if paper && !app.session.hide_ruler { crate::paper_view::RULER } else { 0.0 };
    // Right-to-left sheets carry the vertical ruler on the right, beside the row headings.
    let grid_rect = if rtl {
        Rect::from_min_max(pos2(rect.left(), (rect.top() + ruler).min(rect.bottom())), pos2((rect.right() - ruler).max(rect.left()), rect.bottom()))
    } else {
        Rect::from_min_max(pos2((rect.left() + ruler).min(rect.right()), (rect.top() + ruler).min(rect.bottom())), rect.max)
    };
    let make_geo = |app: &mut SheetApp, scroll: egui::Vec2| -> Geo {
        let lines = app.grid.line_geo_for(line_key, sh);
        if paper {
            let (ri, ci, row_outline, col_outline) = lines.clone().into_arcs();
            let probe = Geo::new_with_lines(sh, grid_rect, scroll, ri, ci, row_outline, col_outline);
            if let Some(g) = crate::paper_view::grid_for(app, &wb, si, sh, probe.cells.size(), scroll) {
                let g = crate::paper_view::with_dragged_margin(app, &wb, si, g);
                let (ri, ci, row_outline, col_outline) = lines.into_arcs();
                return Geo::paged_with_lines(sh, grid_rect, scroll, g, ri, ci, row_outline, col_outline);
            }
        }
        let (ri, ci, row_outline, col_outline) = lines.into_arcs();
        Geo::new_with_lines(sh, rect, scroll, ri, ci, row_outline, col_outline)
    };
    // Scrolling.
    let (scroll_delta, zoom_delta, modifiers, now) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.modifiers, i.time));
    if (resp.hovered() || resp.contains_pointer()) && !crate::text_box::contains_pointer(app, ui.ctx()) {
        if zoom_delta != 1.0 {
            // egui's zoom factor is e^(speed × points); one notch is `line_scroll_speed` points.
            let notch = ui.ctx().options(|o| o.input_options.scroll_zoom_speed * o.input_options.line_scroll_speed);
            let turned = zoom_delta.ln() / notch;
            let mut zooming = WheelZoom::at(app, place, now);
            if now - zooming.time > WHEEL_ZOOM_IDLE {
                zooming.notches = 0.0;
            }
            if turned.is_finite() {
                zooming.notches = (zooming.notches + turned).clamp(-32.0, 32.0);
            }
            zooming.time = now;
            // A notch steps once 60% of it has turned, and the rest of it brings `notches` back to
            // about 0 (past half, so what's left after a step can't step back).
            let from = sh.zoom;
            let mut z = from;
            while zooming.notches >= 0.6 {
                z = wheel_zoom_step(z, true);
                zooming.notches -= 1.0;
            }
            while zooming.notches <= -0.6 {
                z = wheel_zoom_step(z, false);
                zooming.notches += 1.0;
            }
            if z != from {
                let _ = app.session.run("view.zoom", json!({"percent": z}));
            }
            app.grid.wheel_zoom = Some(zooming);
        } else if scroll_delta != egui::Vec2::ZERO {
            // A pivot slicer under the pointer takes the wheel for its own tile list (the tiles
            // beyond the frame are elided), so the sheet does not scroll underneath it.
            let used = app
                .grid
                .geo
                .clone()
                .zip(ui.input(|i| i.pointer.hover_pos()))
                .is_some_and(|(geo, p)| crate::slicer_view::wheel(app, sh, &geo, geo.mirror_pos(p), scroll_delta));
            if used {
                ui.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
            } else {
                let z = if paper { make_geo(app, view.scroll).z } else { sh.zoom as f32 / 100.0 * DISPLAY_SCALE };
                let mut d = scroll_delta;
                // A horizontal swipe moves the content the way the fingers go, which is towards
                // higher columns on a mirrored sheet.
                if sh.right_to_left {
                    d.x = -d.x;
                }
                if modifiers.shift && d.x == 0.0 {
                    d = vec2(d.y, 0.0);
                }
                view.scroll -= d / z;
            }
        }
    }
    // A one-finger touch drag pans the sheet, the content following the finger; it doesn't select.
    // (Two fingers pinch-zoom through `zoom_delta` above.)
    if resp.drag_started() && ui.input(|i| i.any_touches()) {
        app.grid.touch_pan = true;
    }
    if app.grid.touch_pan && resp.dragged() && ui.input(|i| i.multi_touch().is_none()) {
        let z = if paper { make_geo(app, view.scroll).z } else { sh.zoom as f32 / 100.0 * DISPLAY_SCALE };
        let mut d = resp.drag_delta();
        if sh.right_to_left {
            d.x = -d.x;
        }
        view.scroll -= d / z;
    }
    view.scroll = view.scroll.max(egui::Vec2::ZERO);
    let (mut max_x, mut max_y) = (sh.col_left(MAX_COLS - 20) as f32, sh.row_top(MAX_ROWS - 40) as f32);
    if paper {
        // Page layout units run past sheet units (margins and gaps); the end is known once the
        // pages reach the last row / column.
        let g = make_geo(app, view.scroll);
        if let Some(pg) = &g.paper {
            let (ex, ey) = pg.extent();
            max_x = if pg.cols_complete { (ex as f32 - g.cells.width() / g.z).max(0.0) } else { 1.0e9 };
            max_y = if pg.rows_complete { (ey as f32 - g.cells.height() / g.z).max(0.0) } else { 1.0e9 };
        }
    }
    view.scroll.x = view.scroll.x.min(max_x);
    view.scroll.y = view.scroll.y.min(max_y);
    let mut geo = make_geo(app, view.scroll);
    // Keep the active cell (or the moving end of an extension) visible after keyboard moves.
    if app.grid.ensure_visible {
        app.grid.ensure_visible = false;
        let cell = if std::mem::take(&mut app.grid.reveal_moving_end) { sel.moving_end() } else { sel.active };
        let target = app.editor.as_ref().and_then(|e| e.point_cell).unwrap_or(cell);
        if paper {
            crate::paper_view::ensure_covers(app, &wb, si, target);
            geo = make_geo(app, view.scroll);
        }
        let cr = geo.cell_rect(sh, target);
        let area = Rect::from_min_max(pos2(geo.cells.left() + geo.frozen_w, geo.cells.top() + geo.frozen_h), geo.cells.max);
        if target.row >= geo.fr {
            if cr.top() < area.top() {
                view.scroll.y -= (area.top() - cr.top()) / geo.z;
            } else if cr.bottom() > area.bottom() - 2.0 {
                view.scroll.y += (cr.bottom() - area.bottom() + 2.0) / geo.z;
            }
        }
        if target.col >= geo.fc {
            if cr.left() < area.left() {
                view.scroll.x -= (area.left() - cr.left()) / geo.z;
            } else if cr.right() > area.right() - 2.0 {
                view.scroll.x += ((cr.right() - area.right() + 2.0) / geo.z).min(cr.left() - area.left());
            }
        }
        view.scroll = view.scroll.max(egui::Vec2::ZERO);
        geo = make_geo(app, view.scroll);
    }
    // Snap scrolling to whole rows/columns at rest like Excel? Keep smooth (better on trackpads).
    if let Some(v) = app.view_mut() {
        *v = view;
    }
    app.grid.cells_rect = Some(geo.mirror_rect(geo.cells));
    app.grid.geo = Some(geo.clone());
    // Screen readers walk the cells that are on screen (nothing is built without one).
    crate::a11y::expose_grid(ui, &wb, si, sh, &geo, sel.active, sel.current());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, t.grid_bg);
    // Everything up to the headers is painted in logical (left-to-right) coordinates.
    let mirror_from = painter.add(egui::Shape::Noop);
    let mut cf = CfCache::new();
    let quads = if geo.paper.is_some() {
        crate::paper_view::paint_pages(app, &painter, &geo, &wb, si, sh, &sel, &mut cf, &t)
    } else {
        paint_panes(app, &painter, &geo, &wb, si, sh, &sel, &mut cf, &t)
    };
    // Objects above cells. A hidden object can't stay selected: Delete or the Format pane would act
    // on something the user can't see.
    if crate::chartview::objects_hidden(&wb) {
        app.selected_chart = None;
    }
    let objects = crate::chartview::paint_objects(app, &painter.with_clip_rect(geo.cells), &geo, &wb, si, sh);
    paint_overlays(app, ui, &painter.with_clip_rect(geo.cells), &geo, &wb, si, sh, &sel);
    if sh.view == gridcraft_engine::model::SheetViewMode::PageBreakPreview {
        paint_pages(app, &painter.with_clip_rect(geo.cells), &geo, sh);
    }
    if geo.paper.is_some() {
        crate::paper_view::paint_headers_footers(app, ui, &painter, &geo, &wb, sh);
    }
    // Headers.
    if sh.show_headings {
        let headings = table_headings(&geo, sh, sel.active);
        paint_headers(&painter, &geo, sh, &sel, &t, &quads, &headings);
        paint_freeze_handles(&painter, &geo, sh, &app.grid.drag, &t);
    }
    crate::outline::paint(&painter, &geo, sh, &t);
    if geo.rtl {
        let mirror_to = painter.add(egui::Shape::Noop);
        mirror_painted(ui.ctx(), painter.layer_id(), mirror_from.0..mirror_to.0, &geo, &objects);
    }
    if geo.paper.is_some() {
        crate::paper_view::paint_rulers(app, &painter, rect, &geo, &t);
    }
    // Interaction.
    let previous_popups = (app.grid.filter_menu, app.grid.list_picker, app.grid.header_menu, app.grid.fill_menu);
    // A pan the grid never saw end (it was hidden mid-drag) must not swallow the next press.
    if app.grid.touch_pan && !resp.dragged() && !resp.drag_stopped() {
        app.grid.touch_pan = false;
    }
    if app.grid.touch_pan {
        // Skip the frames of a touch pan, its release included.
        if !resp.dragged() {
            app.grid.touch_pan = false;
        }
    } else {
        crate::paper_view::interact(app, ui, &resp, &geo, sh, &wb);
        interact(app, ui, &resp, &geo, sh, &wb);
    }
    update_cursor(app, ui, &resp, &geo, sh, &wb);
    crate::text_box::show(app, ui, &geo, sh);
    if app.editor.as_ref().is_some_and(|e| !e.from_formula_bar) {
        in_cell_editor(app, ui, &geo, sh, &wb);
    }
    filter_menu(app, ui, &geo, previous_popups.0 == app.grid.filter_menu);
    list_picker(app, ui, &geo, previous_popups.1 == app.grid.list_picker);
    header_menu(app, ui, previous_popups.2 == app.grid.header_menu);
    context_menu(app, ui);
    fill_menu(app, ui, previous_popups.3 == app.grid.fill_menu);
    object_menu(app, ui);
    error_check_tag(app, ui, &geo, sh);
    let track_x = if rtl { (available.left(), rect.left()) } else { (rect.right(), available.right()) };
    // The track starts below the column headings and frozen rows; the bar's column beside them
    // takes its colour too instead of showing a light square in dark mode.
    ui.painter().rect_filled(Rect::from_min_max(pos2(track_x.0, rect.top()), pos2(track_x.1, rect.bottom())), 0.0, t.tab_bar);
    vertical_scrollbar(app, ui, sh, &geo, track_x, max_y, &t);
    if app.session.clipboard.is_some() {
        // The copied range keeps a dashed border that never moves; this repaint is what keeps the
        // clipboard on screen honest when something else changes it.
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(60));
    }
}

/// Normal view: up to four panes (frozen rows/columns), each painted with its own clip.
#[allow(clippy::too_many_arguments)]
fn paint_panes(
    app: &SheetApp,
    painter: &Painter,
    geo: &Geo,
    wb: &Workbook,
    si: usize,
    sh: &Sheet,
    sel: &gridcraft_engine::Selection,
    cf: &mut CfCache,
    t: &Tokens,
) -> Vec<((u32, u32), (u32, u32), Rect)> {
    // Quadrants: (rows, cols, clip).
    let (sr0, sr1) = geo.scroll_rows(sh);
    let (sc0, sc1) = geo.scroll_cols(sh);
    let fr_rows = (0, geo.fr.saturating_sub(1));
    let fr_cols = (0, geo.fc.saturating_sub(1));
    let split = pos2(geo.cells.left() + geo.frozen_w, geo.cells.top() + geo.frozen_h);
    let mut quads = vec![((sr0, sr1), (sc0, sc1), Rect::from_min_max(split, geo.cells.max))];
    if geo.fr > 0 {
        quads.push((fr_rows, (sc0, sc1), Rect::from_min_max(pos2(split.x, geo.cells.top()), pos2(geo.cells.right(), split.y))));
    }
    if geo.fc > 0 {
        quads.push(((sr0, sr1), fr_cols, Rect::from_min_max(pos2(geo.cells.left(), split.y), pos2(split.x, geo.cells.bottom()))));
    }
    if geo.fr > 0 && geo.fc > 0 {
        quads.push((fr_rows, fr_cols, Rect::from_min_max(geo.cells.min, split)));
    }
    for (rows, cols, clip) in &quads {
        if rows.0 > rows.1 || cols.0 > cols.1 {
            continue;
        }
        let p = painter.with_clip_rect(*clip);
        paint_quadrant(&p, geo, wb, si, sh, *rows, *cols, cf, t, &app.text);
        paint_selection(&p, geo, sh, sel, t, app, *rows, *cols);
    }
    // Freeze lines.
    if geo.fr > 0 {
        painter.line_segment([pos2(geo.cells.left(), split.y), pos2(geo.cells.right(), split.y)], Stroke::new(1.0, Color32::from_gray(150)));
    }
    if geo.fc > 0 {
        painter.line_segment([pos2(split.x, geo.cells.top()), pos2(split.x, geo.cells.bottom())], Stroke::new(1.0, Color32::from_gray(150)));
    }
    quads
}

/// The worksheet is painted virtually, so it needs its own scrollbar rather than a
/// ScrollArea containing all million rows. Keep the scroll extent stable during a drag.
fn vertical_scrollbar(app: &mut SheetApp, ui: &mut egui::Ui, sh: &Sheet, geo: &Geo, track_x: (f32, f32), limit: f32, t: &Tokens) {
    let track = Rect::from_min_max(pos2(track_x.0, geo.cells.top() + geo.frozen_h), pos2(track_x.1, geo.cells.bottom()));
    if track.height() <= 0.0 || track.width() <= 0.0 {
        return;
    }
    let visible = track.height() / geo.z;
    let scroll = app.view().scroll.y;
    let last = sh.used_range().map(|r| r.end.row.saturating_add(10)).unwrap_or(100).clamp(100, MAX_ROWS - 1);
    let content = geo.scroll_top_of(sh, last);
    let extent = (content - visible).max(scroll).max(0.0).min(limit);
    let id = ui.id().with("vscroll");
    let resp = ui.interact(track, id, Sense::click_and_drag());
    let drag_id = id.with("extent");
    if resp.drag_started() {
        ui.ctx().data_mut(|d| d.insert_temp(drag_id, extent));
    }
    let extent = if resp.dragged() { ui.ctx().data(|d| d.get_temp::<f32>(drag_id)).unwrap_or(extent) } else { extent };
    let thumb_h = (track.height() * visible / (extent + visible)).clamp(24.0_f32.min(track.height()), track.height());
    let travel = track.height() - thumb_h;
    let thumb_top = track.top() + if extent > 0.0 { travel * (scroll / extent).clamp(0.0, 1.0) } else { 0.0 };
    let thumb = Rect::from_min_size(pos2(track.left() + 3.0, thumb_top), vec2((track.width() - 6.0).max(0.0), thumb_h));
    ui.painter().rect_filled(track, 0.0, t.tab_bar);
    ui.painter().rect_filled(thumb, 4.0, if resp.hovered() || resp.dragged() { t.text_dim } else { t.text_disabled });
    let mut next = scroll;
    if resp.dragged() && travel > 0.0 {
        next += resp.drag_delta().y / travel * extent;
    } else if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        if p.y < thumb.top() {
            next -= visible;
        } else if p.y > thumb.bottom() {
            next += visible;
        }
    }
    if resp.hovered() {
        next -= ui.input(|i| i.smooth_scroll_delta.y) / geo.z;
    }
    next = next.clamp(0.0, limit);
    if next != scroll {
        if let Some(v) = app.view_mut() {
            v.scroll.y = next;
        }
        ui.ctx().request_repaint();
    }
}

/// The horizontal counterpart of `vertical_scrollbar`, drawn in the sheet tab bar as in Excel. The tab
/// bar is laid out before the grid, so this reads the last frame's geometry and leaves clamping the
/// far end to the grid, which runs later in the same frame.
pub(crate) fn horizontal_scrollbar(app: &mut SheetApp, ui: &mut egui::Ui, track: Rect, t: &Tokens) {
    app.grid.hscroll = Some(track);
    if track.height() <= 0.0 || track.width() <= 0.0 {
        return;
    }
    let Some(geo) = app.grid.geo.clone() else { return };
    let Some(wb) = app.session.active().map(|d| d.wb.clone()) else { return };
    let Some(sh) = wb.active() else { return };
    let visible = ((geo.cells.width() - geo.frozen_w) / geo.z).max(0.0);
    let scroll = app.view().scroll.x;
    let last = sh.used_range().map(|r| r.end.col.saturating_add(10)).unwrap_or(26).clamp(26, MAX_COLS - 1);
    let content = geo.scroll_left_of(sh, last);
    let extent = (content - visible).max(scroll).max(0.0);
    let id = ui.id().with("hscroll");
    let resp = ui.interact(track, id, Sense::click_and_drag());
    let drag_id = id.with("extent");
    if resp.drag_started() {
        ui.ctx().data_mut(|d| d.insert_temp(drag_id, extent));
    }
    let extent = if resp.dragged() { ui.ctx().data(|d| d.get_temp::<f32>(drag_id)).unwrap_or(extent) } else { extent };
    let thumb_w = (track.width() * visible / (extent + visible).max(1.0)).clamp(24.0_f32.min(track.width()), track.width());
    let travel = track.width() - thumb_w;
    let offset = if extent > 0.0 { travel * (scroll / extent).clamp(0.0, 1.0) } else { 0.0 };
    // A right-to-left sheet scrolls towards the left, so its thumb starts at the right end.
    let dir = if geo.rtl { -1.0 } else { 1.0 };
    let thumb_left = if geo.rtl { track.right() - thumb_w - offset } else { track.left() + offset };
    let thumb = Rect::from_min_size(pos2(thumb_left, track.top() + 3.0), vec2(thumb_w, (track.height() - 6.0).max(0.0)));
    ui.painter().rect_filled(track, 0.0, t.tab_bar);
    ui.painter().rect_filled(thumb, 4.0, if resp.hovered() || resp.dragged() { t.text_dim } else { t.text_disabled });
    let mut next = scroll;
    if resp.dragged() && travel > 0.0 {
        next += dir * resp.drag_delta().x / travel * extent;
    } else if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        if p.x < thumb.left() {
            next -= dir * visible;
        } else if p.x > thumb.right() {
            next += dir * visible;
        }
    }
    if resp.hovered() {
        let d = ui.input(|i| i.smooth_scroll_delta);
        next -= (if d.x != 0.0 { dir * d.x } else { d.y }) / geo.z;
    }
    next = next.max(0.0);
    if next != scroll {
        if let Some(v) = app.view_mut() {
            v.scroll.x = next;
        }
        ui.ctx().request_repaint();
    }
}

pub(crate) fn wrapped_cell_text_y(valign: VAlign, rect: Rect, gh: f32, z: f32) -> f32 {
    let pad_v = 1.5 * z;
    let y_top = rect.top() + pad_v;
    // Not clamped to `y_top`: text taller than the row overflows upward (Bottom) or both ways
    // (Center), and the cell's clip rect trims it, so Middle and Bottom stay distinct from Top.
    let y_bottom = rect.bottom() - gh - pad_v;
    match valign {
        VAlign::Top => y_top,
        VAlign::Center | VAlign::Justify | VAlign::Distributed => (y_top + y_bottom) / 2.0,
        VAlign::Bottom => y_bottom,
    }
}

pub(crate) fn cell_text_y(valign: VAlign, rect: Rect, gh: f32, z: f32) -> f32 {
    let top_inset = 1.0 * z;
    let y_top = rect.top() - top_inset;
    let y_bottom = (rect.bottom() - gh - 0.5 * z).max(y_top);
    match valign {
        VAlign::Top => y_top,
        VAlign::Center | VAlign::Justify | VAlign::Distributed => (y_top + y_bottom) / 2.0,
        VAlign::Bottom => y_bottom,
    }
}

/// Where to put a `TextShape` holding a galley of `size` turned by `angle` (egui radians, so
/// negative for upward text) for it to sit in `rect` per the cell's alignment. egui turns the
/// galley about `pos`, its top-left corner, so the turned box is placed first and `pos` follows.
/// General keeps the text at the left. Bottom (the default) puts upward text at the bottom, but
/// downward text at the top, where it starts reading.
pub(crate) fn rotated_text_pos(rect: Rect, size: egui::Vec2, angle: f32, h: HAlign, v: VAlign, pad: f32, indent: f32) -> Pos2 {
    let rot = egui::emath::Rot2::from_angle(angle);
    let turned = Rect::from_points(&[vec2(0.0, 0.0), vec2(size.x, 0.0), vec2(0.0, size.y), size].map(|c| (rot * c).to_pos2()));
    let x = match h {
        HAlign::Center | HAlign::CenterAcross | HAlign::Distributed => rect.center().x - turned.width() / 2.0,
        HAlign::Right => rect.right() - pad - indent - turned.width(),
        _ => rect.left() + pad + indent,
    };
    let y = match v {
        VAlign::Center | VAlign::Justify | VAlign::Distributed => rect.center().y - turned.height() / 2.0,
        VAlign::Bottom if angle < 0.0 => rect.bottom() - pad - turned.height(),
        _ => rect.top() + pad,
    };
    pos2(x, y) - turned.min.to_vec2()
}

/// Mirror what the grid painted into `layer` (the shape indices in `range`) for a right-to-left
/// sheet. Geometry is reflected; text and meshes (glyphs, pictures) keep their reading direction
/// and only move so that their box lands mirrored. Floating objects (`objects`: shape index
/// ranges with their logical rect) move as a whole, so a chart keeps its axes and a picture
/// isn't flipped.
fn mirror_painted(ctx: &egui::Context, layer: egui::LayerId, range: std::ops::Range<usize>, geo: &Geo, objects: &[crate::chartview::PaintedObject]) {
    let axis = geo.rect.left() + geo.rect.right();
    ctx.graphics_mut(|g| {
        let list = g.entry(layer);
        for i in range {
            let object = objects.iter().find(|o| o.shapes.contains(&i));
            list.mutate_shape(egui::layers::ShapeIdx(i), |cs| {
                cs.clip_rect = geo.mirror_rect(cs.clip_rect);
                match object {
                    Some(o) => cs.shape.translate(vec2(axis - o.rect.right() - o.rect.left(), 0.0)),
                    None => mirror_shape(&mut cs.shape, axis),
                }
            });
        }
    });
}

/// Reflect one shape across the vertical line `x = axis / 2`.
fn mirror_shape(shape: &mut egui::Shape, axis: f32) {
    use egui::Shape;
    let flip = |p: &mut Pos2| p.x = axis - p.x;
    match shape {
        Shape::Noop | Shape::Callback(_) => {}
        Shape::Vec(v) => v.iter_mut().for_each(|s| mirror_shape(s, axis)),
        Shape::Circle(c) => flip(&mut c.center),
        Shape::Ellipse(e) => flip(&mut e.center),
        Shape::LineSegment { points, .. } => points.iter_mut().for_each(flip),
        Shape::Path(p) => {
            p.points.iter_mut().for_each(flip);
            // Keep the preferred (clockwise) winding for filled polygons.
            p.points.reverse();
        }
        Shape::Rect(r) => {
            r.rect = Rect::from_min_max(pos2(axis - r.rect.right(), r.rect.top()), pos2(axis - r.rect.left(), r.rect.bottom()));
            let c = r.corner_radius;
            r.corner_radius = egui::CornerRadius { nw: c.ne, ne: c.nw, sw: c.se, se: c.sw };
        }
        Shape::QuadraticBezier(b) => b.points.iter_mut().for_each(flip),
        Shape::CubicBezier(b) => b.points.iter_mut().for_each(flip),
        _ => {
            let b = shape.visual_bounding_rect();
            if b.is_finite() {
                shape.translate(vec2(axis - b.right() - b.left(), 0.0));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_quadrant(
    p: &Painter,
    geo: &Geo,
    wb: &Workbook,
    si: usize,
    sh: &Sheet,
    rows: (u32, u32),
    cols: (u32, u32),
    cf: &mut CfCache,
    t: &Tokens,
    engine: &crate::text::GridText,
) {
    let z = geo.z;
    let (r0, r1) = rows;
    let (c0, c1) = cols;
    let range = RangeRef::new(CellRef::new(r0, c0), CellRef::new(r1, c1));
    // Merges are looked up 20 columns beyond the quadrant too: text overflowing in from off-screen cells stops at them.
    let near = RangeRef::new(CellRef::new(r0, c0.saturating_sub(20)), CellRef::new(r1, c1.saturating_add(20).min(MAX_COLS - 1)));
    let merges: Vec<RangeRef> = sh.merges.iter().filter(|m| m.intersects(&near)).copied().collect();
    let in_merge = |c: CellRef| merges.iter().find(|m| m.contains(c)).copied();
    // Gridlines.
    if sh.show_gridlines {
        let stroke = Stroke::new(1.0, t.gridline);
        let top = geo.y(sh, r0);
        let bottom = geo.y(sh, r1 + 1);
        let left = geo.x(sh, c0);
        let right = geo.x(sh, c1 + 1);
        for c in c0..=c1 + 1 {
            if c <= c1 && sh.col_width(c) == 0.0 {
                continue;
            }
            let x = geo.x(sh, c).round() - 0.5;
            p.line_segment([pos2(x, top), pos2(x, bottom)], stroke);
        }
        for r in r0..=r1 + 1 {
            if r <= r1 && sh.row_height(r) == 0.0 {
                continue;
            }
            let y = geo.y(sh, r).round() - 0.5;
            p.line_segment([pos2(left, y), pos2(right, y)], stroke);
        }
    }
    // Merged areas: blank them (no inner gridlines) and paint as one cell.
    let mut cells: Vec<CellRef> = Vec::new();
    for r in r0..=r1 {
        if sh.row_height(r) == 0.0 {
            continue;
        }
        for c in c0..=c1 {
            if sh.col_width(c) == 0.0 {
                continue;
            }
            cells.push(CellRef::new(r, c));
        }
    }
    let empty_ok = |c: CellRef| -> bool { sh.value_ref(c).is_none_or(|v| v.is_empty()) && in_merge(c).is_none() };
    // Backgrounds.
    let mut looks: Vec<(CellRef, Rect, Look, Option<CfLook>)> = Vec::with_capacity(cells.len().min(20_000));
    for &c in &cells {
        let (rect, anchor) = match in_merge(c) {
            Some(m) if m.start != c && !(c.row == r0.max(m.start.row) && c.col == c0.max(m.start.col)) => continue,
            Some(m) => (geo.range_rect(sh, m), m.start),
            None => (geo.cell_rect(sh, c), c),
        };
        let (lk, cfl) = look(wb, si, sh, anchor, cf);
        if let Some(f) = lk.fill {
            p.rect_filled(rect.expand(0.0), 0.0, f);
        } else if in_merge(c).is_some() {
            p.rect_filled(rect.shrink(0.5), 0.0, Color32::WHITE);
        }
        looks.push((anchor, rect, lk, cfl));
    }
    // Text cells just outside the quadrant whose text may overflow into it: the nearest non-empty
    // cell left of `c0` and right of `c1` in each row (within the same frozen / scrolled pane).
    let mut outside: Vec<(CellRef, Rect, Look, Option<CfLook>)> = Vec::new();
    let same_pane = |col: u32| (col < geo.fc) == (c0 < geo.fc);
    for r in r0..=r1 {
        if sh.row_height(r) == 0.0 {
            continue;
        }
        let left = (c0.saturating_sub(20)..c0).rev().map(|col| CellRef::new(r, col)).find(|&n| !empty_ok(n));
        let right = (c1.saturating_add(1)..=c1.saturating_add(20).min(MAX_COLS - 1)).map(|col| CellRef::new(r, col)).find(|&n| !empty_ok(n));
        for n in [left, right].into_iter().flatten() {
            if same_pane(n.col) && sh.col_width(n.col) > 0.0 && in_merge(n).is_none() {
                let (lk, cfl) = look(wb, si, sh, n, cf);
                outside.push((n, geo.cell_rect(sh, n), lk, cfl));
            }
        }
    }
    // Text and per-cell decorations.
    for (c, rect, lk, cfl) in looks.iter().chain(&outside) {
        let c = *c;
        if let Some(picture) = sh.cell_pictures.get(&c) {
            crate::cell_pictures::paint(p, *rect, picture);
            continue;
        }
        let v = sh.value(c);
        if let Some(l) = cfl
            && let Some((frac, rgb, gradient)) = l.bar
        {
            let bar = Rect::from_min_size(pos2(rect.left() + 2.0, rect.top() + 2.0), vec2((rect.width() - 4.0) * frac, rect.height() - 4.0));
            let col = theme::color32(rgb);
            if gradient {
                let mut mesh = egui::Mesh::default();
                let light = Color32::from_rgb(
                    ((rgb[0] as u16 + 255 * 3) / 4) as u8,
                    ((rgb[1] as u16 + 255 * 3) / 4) as u8,
                    ((rgb[2] as u16 + 255 * 3) / 4) as u8,
                );
                let i = mesh.vertices.len() as u32;
                for (pt, cc) in [(bar.left_top(), col), (bar.right_top(), light), (bar.right_bottom(), light), (bar.left_bottom(), col)] {
                    mesh.colored_vertex(pt, cc);
                }
                mesh.add_triangle(i, i + 1, i + 2);
                mesh.add_triangle(i, i + 2, i + 3);
                p.add(mesh);
                p.rect_stroke(bar, 0.0, Stroke::new(1.0, col), StrokeKind::Inside);
            } else {
                p.rect_filled(bar, 0.0, col);
            }
        }
        let mut text_left = rect.left();
        if let Some(l) = cfl
            && let Some((set, idx)) = &l.icon
        {
            let r = Rect::from_min_size(pos2(rect.left() + 2.0, rect.center().y - 6.0 * z), vec2(12.0 * z, 12.0 * z));
            paint_cf_icon(p, r, set, *idx);
            text_left += 15.0 * z;
        }
        if lk.style.num_fmt.as_str() == "checkbox" && matches!(v, Value::Bool(_) | Value::Empty) {
            let s = (11.0 * z * 1.2).clamp(8.0, 24.0);
            let b = Rect::from_center_size(rect.center(), vec2(s, s));
            let on = v == Value::Bool(true);
            p.rect_filled(b, 2.0, if on { Color32::from_rgb(0x10, 0x7C, 0x41) } else { Color32::WHITE });
            p.rect_stroke(
                b,
                2.0,
                Stroke::new(1.0, if on { Color32::from_rgb(0x10, 0x7C, 0x41) } else { Color32::from_gray(110) }),
                StrokeKind::Inside,
            );
            if on {
                crate::icons::paint(p, b.shrink(1.5), crate::icons::Icon::Check, Color32::WHITE);
            }
            continue;
        }
        if cfl.as_ref().is_some_and(|l| l.hide_value) || v.is_empty() {
            continue;
        }
        // Align and clip header text inside the content area, away from the dropdown.
        let filter_space = sh.filter_button_space(c) * z;
        let text_rect = Rect::from_min_max(rect.min, pos2((rect.right() - filter_space).max(rect.left()), rect.bottom()));
        let rect = &text_rect;
        let st = &lk.style;
        let size = st.font.size * 96.0 / 72.0 * z;
        let fam = theme::cell_family(&st.font.name, st.font.bold, st.font.italic);
        let font = FontId::new(size, fam);
        // Digits fit by the cell's own font and size, not the default 11pt (9pt marks in narrow columns).
        let char_w = p.layout_no_wrap("0".to_string(), font.clone(), Color32::PLACEHOLDER).size().x.max(1.0);
        let avail_w = rect.right() - text_left;
        let (mut text, ncolor, numeric, fill_char) = display_text(wb, sh, c, &v, st, avail_w / z.max(0.1) * z, char_w);
        if sh.show_formulas && sh.cell(c).is_some_and(|cell| cell.formula.is_some()) {
            text = crate::formula_locale::display(&text, crate::i18n::current(p.ctx()).formula_locale(), wb, si);
        }
        if text.is_empty() {
            continue;
        }
        let color = ncolor.unwrap_or(lk.text_color);
        let mut halign = match st.align.h {
            HAlign::General => match v {
                Value::Number(_) => HAlign::Right,
                Value::Bool(_) | Value::Error(_) => HAlign::Center,
                _ if numeric => HAlign::Right,
                _ => HAlign::Left,
            },
            h => painted_halign(h, geo.rtl),
        };
        let indent = st.align.indent as f32 * 9.0 * z;
        let pad = 2.5 * z;
        let rotated = st.align.rotation != 0 && st.align.rotation != 255;
        // Rotated text wraps along its own direction (below), not across the column width.
        if !rotated && (st.align.wrap || st.align.h == HAlign::Justify) {
            let wrap_w = (rect.width() - 2.0 * pad - indent).max(4.0);
            // Shaped wrapping for text the bidi/joining/mark engines can change, with each line
            // shaped on its own; printable ASCII keeps epaint's wrap, which is bit-identical.
            let direction = crate::text::base_direction(&text, sh.right_to_left);
            if let Some(lines) = engine.wrap(p.ctx(), &font, &text, direction, color, wrap_w) {
                // General-aligned text follows its own reading direction, as the one-line path
                // does: a right-to-left paragraph in a left-to-right sheet aligns right.
                if !geo.rtl
                    && st.align.h == HAlign::General
                    && !numeric
                    && halign == HAlign::Left
                    && engine.direction_of(p.ctx(), &font, &text, direction).is_some_and(|d| d.is_rtl())
                {
                    halign = HAlign::Right;
                }
                let total: f32 = lines.iter().map(|l| l.height()).sum();
                let mut y = wrapped_cell_text_y(st.align.v, *rect, total, z);
                let clip = p.with_clip_rect(rect.intersect(p.clip_rect()));
                for line in &lines {
                    // Each line is aligned on its own, as the LayoutJob's halign did: the shaped
                    // galley has no alignment of its own, so the whole line's width is taken off
                    // for right and half of it for centre.
                    let lw = line.galley.size().x;
                    let lx = match halign {
                        HAlign::Center | HAlign::CenterAcross => rect.center().x - lw / 2.0,
                        HAlign::Right => rect.right() - pad - lw,
                        _ => text_left + pad + indent,
                    };
                    line.paint(&clip, pos2(lx, y), color);
                    y += line.height();
                }
                continue;
            }
            let mut job = egui::text::LayoutJob::simple(text.clone(), font.clone(), color, wrap_w);
            job.halign = match halign {
                HAlign::Center | HAlign::CenterAcross => egui::Align::Center,
                HAlign::Right => egui::Align::RIGHT,
                _ => egui::Align::LEFT,
            };
            let galley = p.layout_job(job);
            let x = match halign {
                HAlign::Center | HAlign::CenterAcross => rect.center().x,
                HAlign::Right => rect.right() - pad,
                _ => text_left + pad + indent,
            };
            let y = wrapped_cell_text_y(st.align.v, *rect, galley.size().y, z);
            p.with_clip_rect(rect.intersect(p.clip_rect())).galley(pos2(x, y), galley, color);
            continue;
        }
        // Wrap Text off: Excel ignores the line breaks and draws the whole string on one line.
        let first_line: String = text.chars().filter(|&ch| ch != '\n' && ch != '\r').collect();
        // The fill position indexes `text`; move it back by the breaks removed before it.
        let fill_char = fill_char.map(|(ch, pos)| (ch, pos.saturating_sub(text.get(..pos).map_or(0, |s| s.matches(['\n', '\r']).count()))));
        // One line of shaped text: bidi runs in visual order, Arabic joined, Indic reordered. The
        // measure it carries is the width the line is drawn at, so the #### test, Shrink to fit
        // and overflow all agree with the drawing (and with AutoFit, which asks the same engine).
        let direction = crate::text::base_direction(&first_line, sh.right_to_left);
        let mut line = engine.line(p.ctx(), &font, &first_line, direction, color);
        // General-aligned text follows its own reading direction: right-to-left text aligns
        // right, as in Excel. Numbers and dates keep the alignment their format asks for, and a
        // right-to-left *sheet* already puts general text on the right once the grid is mirrored.
        if !geo.rtl
            && st.align.h == HAlign::General
            && !numeric
            && halign == HAlign::Left
            && line.shaped.as_ref().is_some_and(|s| s.base_direction.is_rtl())
        {
            halign = HAlign::Right;
        }
        let mut w = line.width;
        // Shrink to fit: too-wide content is drawn in a smaller font so it fits the cell (or the
        // merged area): it neither spills, nor is clipped, nor turns into ####.
        let small = (st.align.shrink && !rotated && st.align.h != HAlign::Fill && fill_char.is_none())
            .then(|| shrink_font(p, engine, &first_line, &font, avail_w - 2.0 * pad - indent, direction))
            .flatten();
        let shrunk = small.is_some();
        let font = small.unwrap_or(font);
        if shrunk {
            line = engine.line(p.ctx(), &font, &first_line, direction, color);
            w = line.width;
        }
        // Numbers and dates never overflow: they show #### when too narrow.
        if !shrunk && (numeric || matches!(v, Value::Number(_))) && w > avail_w - 2.0 * pad && st.align.h != HAlign::Fill {
            let n = ((avail_w - 2.0 * pad) / char_w).floor().max(1.0) as usize;
            let hashes = "#".repeat(n);
            w = engine.measure(p.ctx(), &font, &hashes, direction);
            line = engine.line(p.ctx(), &font, &hashes, direction, color);
        }
        // Repeat fill (`*` in number formats) / Fill alignment.
        if let Some((ch, pos)) = fill_char
            && pos <= first_line.len()
        {
            let base_w = w;
            let cw = p.layout_no_wrap(ch.to_string(), font.clone(), color).size().x.max(1.0);
            let n = (((avail_w - 2.0 * pad) - base_w) / cw).floor().max(0.0) as usize;
            let mut s = first_line.clone();
            if first_line.is_char_boundary(pos) {
                s.insert_str(pos, &ch.to_string().repeat(n));
            }
            line = engine.line(p.ctx(), &font, &s, direction, color);
            w = line.width;
        }
        // Center Across Selection: the text is centred over this cell and the empty cells to its right that also use it.
        let across_end = if halign == HAlign::CenterAcross && !rotated && in_merge(c).is_none() {
            center_across_end(c.col, c1 + 20, |col| {
                let n = CellRef::new(c.row, col);
                empty_ok(n) && wb.styles.get(sh.style_id(n)).align.h == HAlign::CenterAcross
            })
        } else {
            c.col
        };
        // Overflow into empty neighbours for text (not upright text, whose width is its line height).
        let mut clip = *rect;
        let overflows = !numeric && !matches!(v, Value::Number(_)) && st.align.rotation.abs() != 90 && w > rect.width() - 2.0 * pad;
        if filter_space == 0.0 && (overflows || across_end > c.col) && in_merge(c).is_none() {
            let mut right = rect.right();
            let mut left = rect.left();
            if across_end > c.col {
                right = geo.x(sh, across_end + 1);
            }
            if matches!(halign, HAlign::Left | HAlign::Center | HAlign::Justify) {
                let mut cc = c.col + 1;
                while right - rect.left() < w + 2.0 * pad && cc < MAX_COLS && empty_ok(CellRef::new(c.row, cc)) && cc <= c1 + 20 {
                    right = geo.x(sh, cc + 1);
                    cc += 1;
                }
            }
            if matches!(halign, HAlign::Right | HAlign::Center) {
                let mut cc = c.col;
                while rect.right() - left < w + 2.0 * pad && cc > 0 && empty_ok(CellRef::new(c.row, cc - 1)) && cc + 20 > c0 {
                    cc -= 1;
                    left = geo.x(sh, cc);
                }
            }
            clip = Rect::from_min_max(pos2(left, rect.top()), pos2(right, rect.bottom()));
            // Paint over the gridlines the text passes through.
            if lk.fill.is_none() {
                let inner = Rect::from_min_max(pos2(clip.left() + 1.0, clip.top() + 0.5), pos2(clip.right() - 1.0, clip.bottom() - 1.0));
                let extra_l = Rect::from_min_max(inner.min, pos2(rect.left() + 1.0, inner.bottom()));
                let extra_r = Rect::from_min_max(pos2(rect.right() - 1.0, inner.top()), inner.max);
                if extra_l.width() > 0.0 {
                    p.rect_filled(extra_l, 0.0, t.grid_bg);
                }
                if extra_r.width() > 0.0 {
                    p.rect_filled(extra_r, 0.0, t.grid_bg);
                }
            }
        }
        let x = match halign {
            HAlign::CenterAcross => clip.center().x - w / 2.0,
            HAlign::Center | HAlign::Distributed => rect.center().x - w / 2.0,
            HAlign::Right => rect.right() - pad - w - indent,
            _ => text_left + pad + indent,
        };
        let gh = line.height();
        let y = cell_text_y(st.align.v, *rect, gh, z);
        let cp = p.with_clip_rect(clip.intersect(p.clip_rect()));
        if rotated {
            let angle = -(st.align.rotation as f32).to_radians();
            // Upright (±90°) wrapped text wraps to the row height: lines then stack across the column.
            let galley = if st.align.wrap && st.align.rotation.abs() == 90 {
                let job = egui::text::LayoutJob::simple(text.clone(), font.clone(), color, (rect.height() - 2.0 * pad).max(4.0));
                p.layout_job(job)
            } else {
                line.galley.clone()
            };
            let pos = rotated_text_pos(*rect, galley.size(), angle, painted_halign(st.align.h, geo.rtl), st.align.v, pad, indent);
            let mut shape = egui::epaint::TextShape::new(pos, galley, color);
            shape.angle = angle;
            cp.add(shape);
        } else {
            line.paint(&cp, pos2(x, y), color);
        }
        let baseline = y + gh * 0.82;
        match st.font.underline {
            Underline::None => {}
            Underline::Double | Underline::DoubleAccounting => {
                cp.line_segment([pos2(x, baseline + 1.0), pos2(x + w, baseline + 1.0)], Stroke::new(1.0, color));
                cp.line_segment([pos2(x, baseline + 3.0), pos2(x + w, baseline + 3.0)], Stroke::new(1.0, color));
            }
            _ => {
                cp.line_segment([pos2(x, baseline + 1.0), pos2(x + w, baseline + 1.0)], Stroke::new(1.0, color));
            }
        }
        if st.font.strike {
            cp.line_segment([pos2(x, y + gh * 0.55), pos2(x + w, y + gh * 0.55)], Stroke::new(1.0, color));
        }
    }
    // Borders (over fills and text).
    for (c, rect, lk, _) in &looks {
        let b = &lk.style.borders;
        let merge = merges.iter().find(|m| m.start == *c);
        let edges = match merge {
            // A merged area's outline lives on the cells along each of its edges (Excel writes the
            // right edge on the last column's cells and the bottom edge on the last row's), so each
            // edge is drawn cell by cell from the cell it belongs to.
            Some(m) => merged_edges(wb, si, sh, geo, *m, *rect, (r0, r1), (c0, c1), cf),
            None => vec![
                (b.top, [rect.left_top(), rect.right_top()]),
                (b.bottom, [rect.left_bottom(), rect.right_bottom()]),
                (b.left, [rect.left_top(), rect.left_bottom()]),
                (b.right, [rect.right_top(), rect.right_bottom()]),
            ],
        };
        for (line, pts) in edges {
            if line.is_none() {
                continue;
            }
            let col = col32(line.color, wb, Color32::BLACK);
            paint_border(p, pts, line.style, col);
        }
        if !b.diag_down.is_none() {
            paint_border(p, [rect.left_top(), rect.right_bottom()], b.diag_down.style, col32(b.diag_down.color, wb, Color32::BLACK));
        }
        if !b.diag_up.is_none() {
            paint_border(p, [rect.left_bottom(), rect.right_top()], b.diag_up.style, col32(b.diag_up.color, wb, Color32::BLACK));
        }
        // Comment / note indicator.
        if let Some(cm) = sh.comments.get(c) {
            let tri_col = if cm.threaded { Color32::from_rgb(0x88, 0x55, 0xC8) } else { Color32::from_rgb(0xD0, 0x2E, 0x2E) };
            let s = 6.0 * z.max(0.7);
            p.add(egui::Shape::convex_polygon(
                vec![rect.right_top(), pos2(rect.right(), rect.top() + s), pos2(rect.right() - s, rect.top())],
                tri_col,
                Stroke::NONE,
            ));
        }
        // Background error checking: a number stored as text gets Excel's green corner (#1994).
        if !sh.show_formulas && sh.cell(*c).as_ref().and_then(gridcraft_engine::cmd::formulas::number_stored_as_text).is_some() {
            let s = 6.0 * z.max(0.7);
            p.add(egui::Shape::convex_polygon(
                vec![rect.left_top(), pos2(rect.left() + s, rect.top()), pos2(rect.left(), rect.top() + s)],
                t.error_check_green,
                Stroke::NONE,
            ));
        }
    }
    // Sparklines.
    for sp in &sh.sparklines {
        if !range.contains(sp.cell) {
            continue;
        }
        let rect = geo.cell_rect(sh, sp.cell).shrink(3.0);
        let vals: Vec<Option<f64>> = match gridcraft_engine::calc::evaluate(wb, si, sp.cell, &sp.source) {
            Value::Array(a) => a.data.iter().map(Value::as_f64).collect(),
            v => vec![v.as_f64()],
        };
        let col = sp.color.resolve(&wb.theme).unwrap_or([0x2B, 0x7C, 0xD3]);
        let prims = gridcraft_chart::render_sparkline(sp.kind, &vals, [col[0], col[1], col[2], 255], sp.markers, rect.width(), rect.height());
        crate::chartview::paint_prims(p, rect.min, &prims, 1.0, None);
    }
    // Filter buttons.
    let mut buttons: Vec<(CellRef, bool)> = Vec::new();
    if let Some(af) = &sh.autofilter {
        for col in af.range.start.col..=af.range.end.col {
            let active = af.criteria.iter().any(|(o, _)| af.range.start.col + o == col);
            buttons.push((CellRef::new(af.range.start.row, col), active));
        }
    }
    for tb in &sh.tables {
        if tb.filter_button && tb.header_row {
            for col in tb.range.start.col..=tb.range.end.col {
                buttons.push((CellRef::new(tb.range.start.row, col), false));
            }
        }
    }
    for (c, active) in buttons {
        if !range.contains(c) {
            continue;
        }
        paint_filter_button(p, filter_button_rect(geo, sh, c), active);
    }
}

pub fn filter_button_rect(geo: &Geo, sh: &Sheet, c: CellRef) -> Rect {
    let rect = geo.cell_rect(sh, c);
    // Keep the button within the same header slot that AutoFit reserves.
    let pad = 2.0 * geo.z;
    let s = (rect.height() - 4.0).clamp(10.0, 18.0).min((sh.filter_button_space(c) * geo.z - pad).max(0.0));
    Rect::from_min_size(pos2(rect.right() - s - pad, rect.bottom() - s - pad), vec2(s, s))
}

/// A Table column heading that shows the column's header name in place of its letters.
#[derive(Clone, Debug)]
pub struct TableHeading {
    pub col: u32,
    /// The table's header row, which the filter menu is keyed by.
    pub header_row: u32,
    pub name: String,
    /// The heading slot over the column (logical coordinates).
    pub rect: Rect,
    /// The filter button, when the table shows filter buttons.
    pub button: Option<Rect>,
    /// The column is filtered (the button shows the funnel).
    pub filtered: bool,
}

/// The Table whose header names replace the column letters: the one holding the active cell,
/// with its header row scrolled out of view and some of its rows still in view (Excel, #1423).
pub fn heading_table<'a>(geo: &Geo, sh: &'a Sheet, active: CellRef) -> Option<&'a gridcraft_engine::model::Table> {
    // Page Layout view keeps the letters.
    if geo.paper.is_some() || geo.header_h <= 0.0 {
        return None;
    }
    let tb = sh.tables.iter().find(|t| t.header_row && t.range.contains(active))?;
    let header = tb.range.start.row;
    // A header row in the frozen rows is always on screen.
    if header < geo.fr {
        return None;
    }
    let top = geo.cells.top() + geo.frozen_h;
    let bottom = |r: u32| geo.y(sh, r) + sh.row_height(r) * geo.z;
    (bottom(header) <= top + 0.5 && bottom(tb.range.end.row) > top + 0.5).then_some(tb)
}

/// The headings of `heading_table`'s visible columns.
pub fn table_headings(geo: &Geo, sh: &Sheet, active: CellRef) -> Vec<TableHeading> {
    let Some(tb) = heading_table(geo, sh, active) else { return Vec::new() };
    let af = sh.autofilter.as_ref().filter(|af| af.range == tb.range);
    let mut out = Vec::new();
    for col in tb.range.start.col..=tb.range.end.col {
        let w = sh.col_width(col) * geo.z;
        let off = col - tb.range.start.col;
        let Some(column) = tb.columns.get(off as usize) else { continue };
        if w <= 0.0 {
            continue;
        }
        let rect = Rect::from_min_size(pos2(geo.x(sh, col), geo.head_top()), vec2(w, geo.header_h));
        // Clear of the 4-point column resize edge, and never wider than the column.
        let s = (rect.height() - 6.0).clamp(10.0, 16.0).min(w - 7.0);
        let button = (tb.filter_button && s > 0.0).then(|| Rect::from_min_size(pos2(rect.right() - s - 5.0, rect.center().y - s / 2.0), vec2(s, s)));
        let filtered = tb.filter.iter().any(|(o, _)| *o == off) || af.is_some_and(|af| af.criteria.iter().any(|(o, _)| *o == off));
        out.push(TableHeading { col, header_row: tb.range.start.row, name: column.name.clone(), rect, button, filtered });
    }
    out
}

fn paint_filter_button(p: &Painter, r: Rect, filtered: bool) {
    p.rect_filled(r, 2.0, Color32::from_gray(250));
    p.rect_stroke(r, 2.0, Stroke::new(1.0, Color32::from_gray(170)), StrokeKind::Inside);
    crate::icons::paint(p, r.shrink(1.5), if filtered { crate::icons::Icon::Filter } else { crate::icons::Icon::Chevron }, Color32::from_gray(70));
}

/// The outline segments of merged area `m` drawn at `rect`: the top and left edges come from the
/// first row's and first column's cells, the bottom and right edges from the last row's and last
/// column's cells, one segment per visible cell (limited to the visible `rows` × `cols`).
#[allow(clippy::too_many_arguments)]
fn merged_edges(
    wb: &Workbook,
    si: usize,
    sh: &Sheet,
    geo: &Geo,
    m: RangeRef,
    rect: Rect,
    rows: (u32, u32),
    cols: (u32, u32),
    cf: &mut CfCache,
) -> Vec<(BorderLine, [Pos2; 2])> {
    let mut out = Vec::new();
    let (cs, ce) = (m.start.col.max(cols.0), m.end.col.min(cols.1));
    for col in cs..=ce {
        if sh.col_width(col) == 0.0 {
            continue;
        }
        let (x0, x1) = (geo.x(sh, col), geo.x(sh, col.saturating_add(1)));
        let top = look(wb, si, sh, CellRef::new(m.start.row, col), cf).0.style.borders.top;
        let bottom = look(wb, si, sh, CellRef::new(m.end.row, col), cf).0.style.borders.bottom;
        out.push((top, [pos2(x0, rect.top()), pos2(x1, rect.top())]));
        out.push((bottom, [pos2(x0, rect.bottom()), pos2(x1, rect.bottom())]));
    }
    let (rs, re) = (m.start.row.max(rows.0), m.end.row.min(rows.1));
    for row in rs..=re {
        if sh.row_height(row) == 0.0 {
            continue;
        }
        let (y0, y1) = (geo.y(sh, row), geo.y(sh, row.saturating_add(1)));
        let left = look(wb, si, sh, CellRef::new(row, m.start.col), cf).0.style.borders.left;
        let right = look(wb, si, sh, CellRef::new(row, m.end.col), cf).0.style.borders.right;
        out.push((left, [pos2(rect.left(), y0), pos2(rect.left(), y1)]));
        out.push((right, [pos2(rect.right(), y0), pos2(rect.right(), y1)]));
    }
    out
}

/// The smaller font that makes `text` fit in `room` points of width (Shrink to fit), or `None`
/// when it already fits at its own size.
fn shrink_font(
    p: &Painter,
    engine: &crate::text::GridText,
    text: &str,
    font: &FontId,
    room: f32,
    direction: gridcraft_engine::core::text::Direction,
) -> Option<FontId> {
    let natural = engine.measure(p.ctx(), font, text, direction);
    let room = room.max(1.0);
    if natural.is_nan() || natural <= room {
        return None;
    }
    // Glyph advances don't scale exactly linearly (rounding), so leave a little slack.
    let size = (font.size * room / natural * 0.97).max(1.0);
    Some(FontId::new(size, font.family.clone()))
}

pub(crate) fn paint_border(p: &Painter, pts: [Pos2; 2], style: BorderStyle, col: Color32) {
    let w = style.width();
    let (a, b) = (pts[0], pts[1]);
    match style {
        BorderStyle::Double => {
            let horiz = (a.y - b.y).abs() < 0.5;
            let off = if horiz { vec2(0.0, 1.5) } else { vec2(1.5, 0.0) };
            p.line_segment([a - off, b - off], Stroke::new(1.0, col));
            p.line_segment([a + off, b + off], Stroke::new(1.0, col));
        }
        BorderStyle::Dashed
        | BorderStyle::MediumDashed
        | BorderStyle::DashDot
        | BorderStyle::MediumDashDot
        | BorderStyle::DashDotDot
        | BorderStyle::MediumDashDotDot
        | BorderStyle::SlantDashDot => {
            p.extend(egui::Shape::dashed_line(&[a, b], Stroke::new(w, col), 4.0, 2.0));
        }
        BorderStyle::Dotted | BorderStyle::Hair => {
            p.extend(egui::Shape::dotted_line(&[a, b], col, 2.0, w * 0.5));
        }
        _ => {
            p.line_segment([a, b], Stroke::new(w, col));
        }
    }
}

fn paint_cf_icon(p: &Painter, r: Rect, set: &str, idx: usize) {
    let n = gridcraft_engine::cf::icon_count(set);
    let palette3 = [Color32::from_rgb(0xD1, 0x3B, 0x2F), Color32::from_rgb(0xF2, 0xB8, 0x2E), Color32::from_rgb(0x2E, 0x9E, 0x4F)];
    let col = if n <= 3 {
        palette3.get(idx).copied().unwrap_or(Color32::GRAY)
    } else {
        let t = idx as f32 / (n - 1).max(1) as f32;
        Color32::from_rgb(
            (0xD1 as f32 * (1.0 - t) + 0x2E as f32 * t) as u8,
            (0x3B as f32 * (1.0 - t) + 0x9E as f32 * t) as u8,
            (0x2F as f32 * (1.0 - t) + 0x4F as f32 * t) as u8,
        )
    };
    let lower = set.to_ascii_lowercase();
    if lower.contains("arrow") {
        let c = r.center();
        let s = r.width() / 2.0;
        let pts = if idx + 1 == n {
            vec![pos2(c.x, c.y - s), pos2(c.x + s, c.y + 0.1 * s), pos2(c.x - s, c.y + 0.1 * s)]
        } else if idx == 0 {
            vec![pos2(c.x, c.y + s), pos2(c.x - s, c.y - 0.1 * s), pos2(c.x + s, c.y - 0.1 * s)]
        } else {
            vec![pos2(c.x + s, c.y), pos2(c.x - 0.1 * s, c.y - s), pos2(c.x - 0.1 * s, c.y + s)]
        };
        p.add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
    } else if lower.contains("rating") || lower.contains("quarter") {
        p.circle_stroke(r.center(), r.width() / 2.0 - 0.5, Stroke::new(1.0, Color32::GRAY));
        let frac = idx as f32 / (n - 1).max(1) as f32;
        p.rect_filled(
            Rect::from_min_size(pos2(r.left() + 2.0, r.bottom() - 2.0 - (r.height() - 4.0) * frac), vec2(r.width() - 4.0, (r.height() - 4.0) * frac)),
            1.0,
            Color32::from_rgb(0x2B, 0x7C, 0xD3),
        );
    } else {
        p.circle_filled(r.center(), r.width() / 2.0 - 0.5, col);
    }
}

/// Where the active cell is on screen: the whole merged cell when it is part of one.
fn active_rect(geo: &Geo, sh: &Sheet, sel: &gridcraft_engine::Selection) -> Rect {
    match sh.merge_at(sel.active) {
        Some(m) => geo.range_rect(sh, m),
        None => geo.cell_rect(sh, sel.active),
    }
}

/// The rectangles shaded for a selection: every area except the active cell, which stays clear so
/// it is plain where typing goes. A single selected cell has no fill.
pub fn selection_fill(geo: &Geo, sh: &Sheet, sel: &gridcraft_engine::Selection) -> Vec<Rect> {
    selection_fill_where(geo, sh, sel, |_| true)
}

/// `selection_fill` for the areas `keep` accepts (a frozen pane draws only the areas it shows).
fn selection_fill_where(geo: &Geo, sh: &Sheet, sel: &gridcraft_engine::Selection, keep: impl Fn(&RangeRef) -> bool) -> Vec<Rect> {
    if sel.is_single_cell() {
        return vec![];
    }
    let active = active_rect(geo, sh, sel);
    let area = |r: &RangeRef| geo.range_rect(sh, *r);
    sel.ranges.iter().filter(|r| keep(r)).flat_map(|r| rect_around(area(r), active)).collect()
}

/// `area` without `hole`: the strips above and below the hole, then what is left beside it. The
/// selection fill is translucent, so the active cell can't be cleared by painting over it; the
/// pieces never overlap for the same reason. Pieces with no size, or whose numbers aren't finite,
/// are dropped.
fn rect_around(area: Rect, hole: Rect) -> impl Iterator<Item = Rect> {
    // `intersect` takes the sane side of a NaN, which would make a broken hole cover the area.
    let hole = if hole.is_finite() { area.intersect(hole) } else { Rect::NOTHING };
    let parts = if hole.is_positive() {
        [
            Rect::from_min_max(area.min, pos2(area.max.x, hole.min.y)),
            Rect::from_min_max(pos2(area.min.x, hole.max.y), area.max),
            Rect::from_min_max(pos2(area.min.x, hole.min.y), pos2(hole.min.x, hole.max.y)),
            Rect::from_min_max(pos2(hole.max.x, hole.min.y), pos2(area.max.x, hole.max.y)),
        ]
    } else {
        [area, Rect::NOTHING, Rect::NOTHING, Rect::NOTHING]
    };
    parts.into_iter().filter(|r| r.is_positive() && r.is_finite())
}

/// True when `r` covers at least one cell of the pane that shows `rows` x `cols` (inclusive).
/// The panes of a frozen sheet share one coordinate system, so a range that lives only in the
/// scrolling pane would otherwise be drawn over the frozen pane as well.
fn range_in_pane(r: RangeRef, rows: (u32, u32), cols: (u32, u32)) -> bool {
    r.start.row <= rows.1 && r.end.row >= rows.0 && r.start.col <= cols.1 && r.end.col >= cols.0
}

/// The four sides of a `width` wide outline centred on `rect`, cut to `clip` (a little beyond it).
///
/// A stroked rectangle that spans a whole column (20 million points tall) loses its precision in
/// the tessellator and its left and right sides vanish; sides filled from rectangles that stay
/// near the viewport do not. Sides wholly outside `clip` are left out.
pub(crate) fn outline_bands(rect: Rect, width: f32, clip: Rect) -> Vec<Rect> {
    if !rect.is_finite() || !clip.is_finite() {
        return Vec::new();
    }
    let h = width / 2.0;
    let clip = clip.expand(width * 2.0);
    let sides = [
        Rect::from_min_max(pos2(rect.left() - h, rect.top() - h), pos2(rect.left() + h, rect.bottom() + h)),
        Rect::from_min_max(pos2(rect.right() - h, rect.top() - h), pos2(rect.right() + h, rect.bottom() + h)),
        Rect::from_min_max(pos2(rect.left() - h, rect.top() - h), pos2(rect.right() + h, rect.top() + h)),
        Rect::from_min_max(pos2(rect.left() - h, rect.bottom() - h), pos2(rect.right() + h, rect.bottom() + h)),
    ];
    sides.into_iter().map(|s| s.intersect(clip)).filter(|s| s.is_finite() && s.width() > 0.0 && s.height() > 0.0).collect()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_selection(
    p: &Painter,
    geo: &Geo,
    sh: &Sheet,
    sel: &gridcraft_engine::Selection,
    t: &Tokens,
    app: &SheetApp,
    rows: (u32, u32),
    cols: (u32, u32),
) {
    let outline = geo.range_rect(sh, sel.current());
    // Fill all areas except the active cell.
    for rect in selection_fill_where(geo, sh, sel, |r| range_in_pane(*r, rows, cols)) {
        p.rect_filled(rect, 0.0, t.sel_fill);
    }
    let active_range = sh.merge_at(sel.active).unwrap_or_else(|| RangeRef::cell(sel.active));
    let active = active_rect(geo, sh, sel);
    let active_here = range_in_pane(active_range, rows, cols);
    let cur_here = range_in_pane(sel.current(), rows, cols);
    // Editing a formula: outline its references instead of the selection.
    if let Some(ed) = &app.editor
        && ed.is_formula()
    {
        let origin = app.session.active().and_then(|d| d.wb.sheet(ed.sheet)).map_or(sh.name.as_str(), |o| o.name.as_str());
        for (_, _, r, sheet, idx) in crate::editor::formula_refs(&ed.text) {
            if !crate::editor::ref_is_on_sheet(sheet.as_deref(), origin, &sh.name) {
                continue;
            }
            if !range_in_pane(r, rows, cols) {
                continue;
            }
            let rr = geo.range_rect(sh, r);
            let col = REF_COLORS[idx];
            p.rect_filled(rr, 0.0, col.gamma_multiply(0.12));
            p.rect_stroke(rr, 0.0, Stroke::new(1.5, col), StrokeKind::Inside);
            for corner in [rr.left_top(), rr.right_top(), rr.left_bottom(), rr.right_bottom()] {
                p.rect_filled(Rect::from_center_size(corner, vec2(4.0, 4.0)), 0.0, col);
            }
        }
    }
    if cur_here {
        for side in outline_bands(outline.expand(0.5), 2.0, p.clip_rect()) {
            p.rect_filled(side, 0.0, t.sel_border);
        }
    }
    if active_here && sel.ranges.len() > 1 {
        p.rect_stroke(active, 0.0, Stroke::new(1.0, t.sel_border), StrokeKind::Inside);
    }
    // Fill handle (bottom-right of the current area).
    if app.editor.is_none() && range_in_pane(RangeRef::cell(sel.current().end), rows, cols) {
        let h = Rect::from_center_size(outline.right_bottom(), vec2(6.0, 6.0));
        p.rect_filled(h.expand(1.0), 0.0, Color32::WHITE);
        p.rect_filled(h, 0.0, t.sel_border);
    }
    // Fill-handle drag preview.
    if let crate::grid::Drag::Fill { target: Some(tr), .. } = &app.grid.drag
        && range_in_pane(*tr, rows, cols)
    {
        let rr = geo.range_rect(sh, *tr);
        p.extend(egui::Shape::dashed_line(
            &[rr.left_top(), rr.right_top(), rr.right_bottom(), rr.left_bottom(), rr.left_top()],
            Stroke::new(1.5, Color32::from_gray(90)),
            4.0,
            3.0,
        ));
    }
    if let crate::grid::Drag::Move { to: Some(to), .. } = &app.grid.drag {
        let r = sel.current();
        let moved = RangeRef::new(*to, to.offset_clamped(r.height() as i64 - 1, r.width() as i64 - 1));
        if range_in_pane(moved, rows, cols) {
            let rr = geo.range_rect(sh, moved);
            p.rect_stroke(rr, 0.0, Stroke::new(2.0, Color32::from_gray(110)), StrokeKind::Middle);
        }
    }
    if let crate::grid::Drag::MoveCols { before: Some(before), .. } = &app.grid.drag {
        let x = geo.x(sh, *before);
        p.line_segment([pos2(x, geo.rect.top()), pos2(x, geo.rect.bottom())], Stroke::new(3.0, t.accent));
    }
    // Marching ants around the copied range.
    if let Some(clip) = &app.session.clipboard
        && app.session.active().is_some_and(|d| d.uid == clip.doc_uid && d.wb.active_sheet == clip.sheet)
        && range_in_pane(clip.range, rows, cols)
    {
        // A multi-area copy marks each area as selected.
        let areas = if clip.areas.is_empty() { std::slice::from_ref(&clip.range) } else { clip.areas.as_slice() };
        for area in areas {
            let rr = geo.range_rect(sh, *area).expand(0.5);
            let pts = [rr.left_top(), rr.right_top(), rr.right_bottom(), rr.left_bottom(), rr.left_top()];
            p.extend(egui::Shape::dashed_line_with_offset(&pts, Stroke::new(2.0, t.sel_border), &[4.0], &[4.0], 0.0));
        }
    }
}

pub(crate) fn paint_headers(
    p: &Painter,
    geo: &Geo,
    sh: &Sheet,
    sel: &gridcraft_engine::Selection,
    t: &Tokens,
    quads: &[((u32, u32), (u32, u32), Rect)],
    headings: &[TableHeading],
) {
    let font = theme::ui_font(heading_font_size(geo.z));
    // The headers' own area: inside the outline bands.
    let r = Rect::from_min_max(pos2(geo.head_left(), geo.head_top()), geo.rect.max);
    let top = Rect::from_min_max(pos2(geo.cells.left(), r.top()), pos2(r.right(), geo.cells.top()));
    let left = Rect::from_min_max(pos2(r.left(), geo.cells.top()), pos2(geo.cells.left(), r.bottom()));
    p.rect_filled(top, 0.0, t.header_bg);
    p.rect_filled(left, 0.0, t.header_bg);
    let bounds = sel.bounds();
    let col_sel = |c: u32| sel.ranges.iter().any(|x| c >= x.start.col && c <= x.end.col);
    let row_sel = |rr: u32| sel.ranges.iter().any(|x| rr >= x.start.row && rr <= x.end.row);
    let full_col = |c: u32| sel.ranges.iter().any(|x| x.is_full_cols() && c >= x.start.col && c <= x.end.col);
    let full_row = |rr: u32| sel.ranges.iter().any(|x| x.is_full_rows() && rr >= x.start.row && rr <= x.end.row);
    let _ = bounds;
    // Hidden columns and rows leave a double divider where they sit, as in Excel.
    let hidden_mark = Stroke::new(1.0, t.header_text.gamma_multiply(0.6));
    let mut done_cols = std::collections::HashSet::new();
    let mut done_rows = std::collections::HashSet::new();
    for (rows, cols, clip) in quads {
        let pc = p.with_clip_rect(Rect::from_min_max(pos2(clip.left(), top.top()), pos2(clip.right(), top.bottom())));
        if cols.0 <= cols.1 {
            for c in cols.0..=cols.1 {
                if !done_cols.insert((c, clip.left() as i32)) {
                    continue;
                }
                let w = sh.col_width(c) * geo.z;
                if w <= 0.0 {
                    continue;
                }
                let x = geo.x(sh, c);
                let cr = Rect::from_min_size(pos2(x, top.top()), vec2(w, top.height()));
                if col_sel(c) {
                    pc.rect_filled(cr, 0.0, if full_col(c) { t.header_all_bg } else { t.header_sel_bg });
                    pc.line_segment([pos2(cr.left(), cr.bottom() - 1.0), pos2(cr.right(), cr.bottom() - 1.0)], Stroke::new(2.0, t.accent));
                }
                pc.line_segment([pos2(cr.right() - 0.5, cr.top() + 3.0), pos2(cr.right() - 0.5, cr.bottom())], Stroke::new(1.0, t.header_line));
                let (before, after) = hidden_around(c, |i| sh.col_width(i));
                for x in [before.then_some(cr.left()), after.then_some(cr.right())].into_iter().flatten() {
                    for dx in [-1.5, 1.5] {
                        pc.line_segment([pos2(x + dx, cr.top() + 3.0), pos2(x + dx, cr.bottom())], hidden_mark);
                    }
                }
                let color = if col_sel(c) { t.header_sel_text } else { t.header_text };
                match headings.iter().find(|h| h.col == c) {
                    Some(h) => {
                        // The name is cut off at the button, as a cell's text is at its neighbour.
                        let end = h.button.map_or(cr.right(), |b| b.left()) - 2.0;
                        let room = Rect::from_min_max(cr.min, pos2(end.max(cr.left()), cr.bottom())).intersect(pc.clip_rect());
                        let pad = 4.0 * (geo.z / DISPLAY_SCALE).max(0.75);
                        pc.with_clip_rect(room).text(pos2(cr.left() + pad, cr.center().y), Align2::LEFT_CENTER, &h.name, font.clone(), color);
                        if let Some(b) = h.button {
                            paint_filter_button(&pc, b, h.filtered);
                        }
                    }
                    None => {
                        pc.text(cr.center(), Align2::CENTER_CENTER, col_to_letters(c), font.clone(), color);
                    }
                }
            }
        }
        let pr = p.with_clip_rect(Rect::from_min_max(pos2(left.left(), clip.top()), pos2(left.right(), clip.bottom())));
        if rows.0 <= rows.1 {
            for rr in rows.0..=rows.1 {
                if !done_rows.insert((rr, clip.top() as i32)) {
                    continue;
                }
                let h = sh.row_height(rr) * geo.z;
                if h <= 0.0 {
                    continue;
                }
                let y = geo.y(sh, rr);
                let cr = Rect::from_min_size(pos2(left.left(), y), vec2(left.width(), h));
                if row_sel(rr) {
                    pr.rect_filled(cr, 0.0, if full_row(rr) { t.header_all_bg } else { t.header_sel_bg });
                    pr.line_segment([pos2(cr.right() - 1.0, cr.top()), pos2(cr.right() - 1.0, cr.bottom())], Stroke::new(2.0, t.accent));
                }
                pr.line_segment([pos2(cr.left() + 3.0, cr.bottom() - 0.5), pos2(cr.right(), cr.bottom() - 0.5)], Stroke::new(1.0, t.header_line));
                let (before, after) = hidden_around(rr, |i| sh.row_height(i));
                for y in [before.then_some(cr.top()), after.then_some(cr.bottom())].into_iter().flatten() {
                    for dy in [-1.5, 1.5] {
                        pr.line_segment([pos2(cr.left() + 3.0, y + dy), pos2(cr.right(), y + dy)], hidden_mark);
                    }
                }
                let color = if row_sel(rr) { t.header_sel_text } else { t.header_text };
                pr.text(cr.center(), Align2::CENTER_CENTER, (rr as u64 + 1).to_string(), font.clone(), color);
            }
        }
    }
    // Corner (select all) and edges.
    let corner = Rect::from_min_max(r.min, geo.cells.min);
    p.rect_filled(corner, 0.0, t.header_bg);
    let tri = vec![
        pos2(corner.right() - 3.0, corner.top() + 5.0),
        pos2(corner.right() - 3.0, corner.bottom() - 3.0),
        pos2(corner.left() + 7.0, corner.bottom() - 3.0),
    ];
    p.add(egui::Shape::convex_polygon(tri, Color32::from_gray(190), Stroke::NONE));
    p.line_segment([pos2(r.left(), geo.cells.top() - 0.5), pos2(r.right(), geo.cells.top() - 0.5)], Stroke::new(1.0, t.header_line));
    p.line_segment([pos2(geo.cells.left() - 0.5, r.top()), pos2(geo.cells.left() - 0.5, r.bottom())], Stroke::new(1.0, t.header_line));
}

/// Whether the line before / after visible line `i` is hidden (zero size), so its header edges
/// get the hidden marker. Line 0 has nothing before it.
fn hidden_around(i: u32, size: impl Fn(u32) -> f32) -> (bool, bool) {
    (i > 0 && size(i - 1) <= 0.0, i < u32::MAX && size(i + 1) <= 0.0)
}

// ------------------------------------------------------------------ interaction

/// The toast shown while an object is dragged: its size, or its position.
fn object_toast(kind: &str, f: &crate::chartview::Frame, moving: bool) -> String {
    if moving {
        format!("{} at {:.0}, {:.0}", object_name(kind), f.x.round(), f.y.round())
    } else {
        format!("{} {:.0} × {:.0}", object_name(kind), f.w.round(), f.h.round())
    }
}

fn object_name(kind: &str) -> &'static str {
    match kind {
        "chart" => "Chart",
        "image" => "Picture",
        "slicer" | "pivotSlicer" => "Slicer",
        "timeline" => "Timeline",
        _ => "Shape",
    }
}

/// Decide after interaction so drag start and release use the new state in the same frame.
fn update_cursor(app: &mut SheetApp, ui: &egui::Ui, resp: &egui::Response, geo: &Geo, sh: &Sheet, wb: &Workbook) {
    if app.text_box_editor.is_some() || app.grid.hf_edit.is_some() {
        return;
    }
    let ctx = ui.ctx();
    let pointer = ctx.input(|i| i.pointer.clone());
    let pos = pointer.interact_pos().or(pointer.hover_pos()).map(|p| geo.mirror_pos(p));
    let in_cells = pos.is_some_and(|p| geo.in_cells(p));
    let (freeze_cols, freeze_rows) = freeze_handles(geo);
    // Hover bookkeeping also serves cell notes, independently of who owns the cursor.
    if resp.hovered()
        && let Some(p) = pos
        && !freeze_cols.contains(p)
        && !freeze_rows.contains(p)
        && !matches!(app.grid.drag, Drag::Freeze { .. })
    {
        app.grid.hover_cell = in_cells.then(|| geo.cell_at(sh, p));
    }
    // Draw tools own the cursor over cells, including while recording a stroke.
    if !app.session.draw_tool.is_empty() && in_cells {
        ctx.set_cursor_icon(CursorIcon::Crosshair);
        return;
    }
    // A held drag owns the cursor even outside the grid; a press below egui's threshold still
    // has Drag::None and uses hover. A click can briefly leave a drag state after release.
    if resp.dragged() && pointer.primary_down() {
        let cursor = match app.grid.drag {
            Drag::None => None,
            Drag::Select => Some(CursorIcon::Cell),
            Drag::Rows(_) | Drag::Cols(_) => Some(CursorIcon::Default),
            Drag::Fill { .. } => Some(CursorIcon::Crosshair),
            Drag::Move { .. } | Drag::MoveCols { .. } => Some(CursorIcon::Move),
            Drag::ResizeCol { .. } => Some(CursorIcon::ResizeColumn),
            Drag::ResizeRow { .. } => Some(CursorIcon::ResizeRow),
            Drag::Freeze { cols, .. } => Some(if cols { CursorIcon::ResizeHorizontal } else { CursorIcon::ResizeVertical }),
            Drag::Object(ref d) => Some(d.handle.map_or(CursorIcon::Move, |h| h.cursor(geo.rtl))),
            Drag::Timeline(ref d) => {
                Some(if d.grab == crate::slicer_view::TimelineGrab::Move { CursorIcon::Move } else { CursorIcon::ResizeHorizontal })
            }
        };
        if let Some(cursor) = cursor {
            ctx.set_cursor_icon(cursor);
            return;
        }
    }
    if !resp.hovered() {
        return;
    }
    let Some(p) = pos else { return };
    // A timeline's handles and selected band are draggable and say so with their cursor; the
    // caption, the unit and the rest of the frame stay cell cursor.
    if app.grid.drag == Drag::None
        && let Some(icon) = crate::slicer_view::timeline_cursor(app, geo, sh, wb, p)
    {
        ctx.set_cursor_icon(icon);
        return;
    }
    if freeze_cols.contains(p) || freeze_rows.contains(p) {
        let cols = freeze_cols.contains(p);
        ctx.set_cursor_icon(if cols { CursorIcon::ResizeHorizontal } else { CursorIcon::ResizeVertical });
        if app.grid.drag == Drag::None && !pointer.any_down() {
            let tip = if cols { "Drag to freeze columns" } else { "Drag to freeze rows" };
            resp.clone().on_hover_text_at_pointer(crate::i18n::current(ctx).tr(tip));
        }
        return;
    }
    let in_col_header = p.y < geo.cells.top() && p.y >= geo.head_top() && p.x >= geo.cells.left();
    let in_row_header = p.x < geo.cells.left() && p.x >= geo.head_left() && p.y >= geo.cells.top();
    let sel = app.session.active().map(|d| d.selection.clone()).unwrap_or_default();
    let cur_rect = geo.range_rect(sh, sel.current());
    let handle = Rect::from_center_size(cur_rect.right_bottom(), vec2(9.0, 9.0));
    let handle_panes = geo.range_panes(RangeRef::cell(sel.current().end));
    let border_panes = geo.range_panes(sel.current());
    let on_handle = handle.contains(p) && handle_panes.iter().any(|r| r.contains(p));
    let on_cur_border = on_border(cur_rect, p) && border_panes.iter().any(|r| r.contains(p));
    // A timeline shows no resize handles (it has no move command), so it must not show their cursors either.
    let selected_object = crate::chartview::hit(app, geo, sh, p).filter(|&(kind, id, _)| kind != "timeline" && app.selected_chart == Some(id));
    let handle = selected_object.and_then(|(_, _, rect)| crate::chartview::handle_at(rect, p));
    if app.grid.drag == Drag::None
        && let Some(h) = handle
    {
        ctx.set_cursor_icon(h.cursor(geo.rtl));
    } else if (in_col_header && col_edge(geo, sh, p.x).is_some()) || (in_row_header && row_edge(geo, sh, p.y).is_some()) {
        let near_col_edge = in_col_header && col_edge(geo, sh, p.x).is_some();
        // Just past a hidden column or row, Excel shows a split cursor: dragging or
        // double-clicking there unhides it.
        let hidden = app.grid.drag == Drag::None
            && if near_col_edge { hidden_col_edge(geo, sh, p.x).is_some() } else { hidden_row_edge(geo, sh, p.y).is_some() };
        match pointer.hover_pos() {
            Some(at) if hidden => paint_split_cursor(ctx, at, near_col_edge),
            _ => ctx.set_cursor_icon(if near_col_edge { CursorIcon::ResizeColumn } else { CursorIcon::ResizeRow }),
        }
    } else if on_handle && app.editor.is_none() {
        ctx.set_cursor_icon(CursorIcon::Crosshair);
        if app.grid.drag == Drag::None && !pointer.any_down() {
            resp.clone().on_hover_text_at_pointer("Drag to fill cells. Drag from inside a cell to select a range.");
        }
    } else if in_cells && app.editor.is_none() && on_cur_border {
        ctx.set_cursor_icon(CursorIcon::Move);
    } else if in_cells {
        ctx.set_cursor_icon(CursorIcon::Cell);
    } else if in_col_header {
        let col = geo.col_at(sh, p.x);
        let selected = sel.ranges.len() == 1 && sel.current().is_full_cols() && sel.current().contains(CellRef::new(0, col));
        ctx.set_cursor_icon(if selected { CursorIcon::Move } else { CursorIcon::Default });
    }
}

fn interact(app: &mut SheetApp, ui: &mut egui::Ui, resp: &egui::Response, geo: &Geo, sh: &Sheet, wb: &Workbook) {
    // Text selection and editing must not also move the object or edit worksheet cells.
    if app.text_box_editor.is_some() || app.grid.hf_edit.is_some() {
        return;
    }
    let ctx = ui.ctx().clone();
    // Enter/Space synthesize egui clicks; only real pointer clicks use sheet coordinates.
    let primary_clicked = resp.clicked_by(egui::PointerButton::Primary);
    let pointer = ctx.input(|i| i.pointer.clone());
    let mods = ctx.input(|i| i.modifiers);
    // Hit-testing works in logical coordinates (see `Geo::rtl`).
    let pos = pointer.interact_pos().or(pointer.hover_pos()).map(|p| geo.mirror_pos(p));
    let in_cells = pos.is_some_and(|p| geo.in_cells(p));
    let in_col_header = pos.is_some_and(|p| p.y < geo.cells.top() && p.y >= geo.head_top() && p.x >= geo.cells.left());
    let in_row_header = pos.is_some_and(|p| p.x < geo.cells.left() && p.x >= geo.head_left() && p.y >= geo.cells.top());
    let in_corner = pos.is_some_and(|p| p.x < geo.cells.left() && p.y < geo.cells.top() && p.x >= geo.head_left() && p.y >= geo.head_top());
    let sel = app.session.active().map(|d| d.selection.clone()).unwrap_or_default();
    let cur_rect = geo.range_rect(sh, sel.current());
    let handle = Rect::from_center_size(cur_rect.right_bottom(), vec2(9.0, 9.0));
    // The fill handle and the move border only react in the panes where they are painted, not
    // over the frozen rows/columns that a scrolled selection lies under.
    let handle_panes = geo.range_panes(RangeRef::cell(sel.current().end));
    let border_panes = geo.range_panes(sel.current());
    let on_handle = |p: Pos2| handle.contains(p) && handle_panes.iter().any(|r| r.contains(p));
    let on_cur_border = |p: Pos2| on_border(cur_rect, p) && border_panes.iter().any(|r| r.contains(p));

    let (freeze_cols, freeze_rows) = freeze_handles(geo);
    // Draw tab tools: the pen records a stroke, the eraser deletes ink under the pointer.
    if !app.session.draw_tool.is_empty() && in_cells {
        ctx.set_cursor_icon(CursorIcon::Crosshair);
        if app.session.draw_tool == "pen" {
            if let Some(p) = pos
                && resp.dragged()
            {
                // Through the cell under the pointer, so frozen panes and pages map right.
                let c = geo.cell_at(sh, p);
                let sx = sh.col_left(c.col) as f32 + (p.x - geo.x(sh, c.col)) / geo.z;
                let sy = sh.row_top(c.row) as f32 + (p.y - geo.y(sh, c.row)) / geo.z;
                app.grid.ink.push([sx, sy]);
            }
            if !app.grid.ink.is_empty() {
                // Live preview.
                let pts: Vec<Pos2> = app
                    .grid
                    .ink
                    .iter()
                    .map(|q| {
                        let (col, row) = (sh.col_at(q[0].max(0.0) as f64), sh.row_at(q[1].max(0.0) as f64));
                        geo.mirror_pos(pos2(
                            geo.x(sh, col) + (q[0] - sh.col_left(col) as f32) * geo.z,
                            geo.y(sh, row) + (q[1] - sh.row_top(row) as f32) * geo.z,
                        ))
                    })
                    .collect();
                ui.painter().add(egui::epaint::PathShape::line(pts, egui::epaint::PathStroke::new(2.0 * geo.z, Color32::from_rgb(0x1F, 0x5F, 0xC9))));
            }
            if resp.drag_stopped() && app.grid.ink.len() >= 2 {
                let pts: Vec<[f32; 2]> = std::mem::take(&mut app.grid.ink);
                let _ = app.run("draw.stroke", json!({"points": pts}));
            }
            return;
        }
        if app.session.draw_tool == "eraser" && (primary_clicked || resp.dragged()) {
            if let Some(p) = pos
                && let Some(("shape", id, _)) = crate::chartview::hit(app, geo, sh, p)
                && sh.shapes.iter().any(|s| s.id == id && s.kind == gridcraft_engine::model::ShapeKind::Ink)
            {
                let _ = app.run("object.delete", json!({"kind": "shape", "id": id}));
            }
            return;
        }
    }
    // A plain click on a pivot slicer's value tile toggles it (Ctrl/Cmd for multi-select); the
    // clear button selects every tile again. One click is one command, so one undo step.
    if primary_clicked
        && !resp.drag_started()
        && let Some(p) = pos
        && let Some((id, act)) = crate::slicer_view::click(app, geo, sh, wb, p)
    {
        app.selected_chart = Some(id);
        match act {
            crate::slicer_view::SlicerAction::Toggle { index, label } => {
                app.grid.slicer_focus = Some((id, index));
                let _ = app.run("slicer.toggle", json!({"slicer": id, "value": label, "multi": mods.command || mods.ctrl}));
            }
            crate::slicer_view::SlicerAction::Clear => {
                let _ = app.run("slicer.clear", json!({"slicer": id}));
            }
        }
        return;
    }
    // A plain click on a table slicer value tile toggles it (Ctrl/Cmd for multi-select); the clear
    // button resets it. Runs before the generic object select/drag so tiles stay clickable.
    if primary_clicked
        && !resp.drag_started()
        && let Some(p) = pos
        && let Some((id, act)) = crate::chartview::slicer_click(geo, sh, wb, wb.active_sheet, p)
    {
        app.selected_chart = Some(id);
        match act {
            crate::chartview::SlicerAction::Toggle(value) => {
                let _ = app.run("slicer.toggle", json!({"slicer": id, "value": value, "multi": mods.command || mods.ctrl}));
            }
            crate::chartview::SlicerAction::Clear => {
                let _ = app.run("slicer.clear", json!({"slicer": id}));
            }
        }
        return;
    }
    // Objects (charts, pictures, shapes, slicers) take clicks first. Only the primary button moves
    // them: a right-click that wobbles into a drag still opens the object's menu (#2067).
    let primary_drag = resp.drag_started_by(egui::PointerButton::Primary);
    if primary_drag || primary_clicked {
        // A drag is recognised only after the pointer has moved, so grab the object where the press began.
        let at = if primary_drag { pointer.press_origin().or(pos) } else { pos };
        if let Some(p) = at
            && let Some((kind, id, rect)) = crate::chartview::hit(app, geo, sh, p)
        {
            let already = app.selected_chart == Some(id);
            let handle = if already { crate::chartview::handle_at(rect, p) } else { None };
            app.selected_chart = Some(id);
            if kind == "shape"
                && resp.double_clicked()
                && sh.shapes.iter().any(|s| s.id == id && s.kind == gridcraft_engine::model::ShapeKind::TextBox)
            {
                app.begin_text_box_edit(id);
                return;
            }
            if kind == "chart" && resp.double_clicked() {
                app.grid.pane = Some("formatChart".into());
            }
            // A fresh click starts at the chart area, then a click on a selected chart picks the
            // element under the pointer (Excel: the first click selects the chart, the next one
            // the axis, series, point or chart area).
            if kind == "chart" && primary_clicked && !already {
                app.format_selection = crate::chart_format::Selection::Chart;
            }
            if kind == "chart"
                && primary_clicked
                && already
                && let Some(ch) = sh.charts.iter().find(|c| c.id == id)
                && let Some(hit) = crate::chartview::element_at(ui.painter(), wb, wb.active_sheet, ch, rect, p, Some(&app.text))
            {
                crate::chart_format::select_element(app, ui, id, hit);
            }
            if primary_drag {
                // A handle belongs to the object that was selected before this press: the first
                // press on an object selects it, the next one moves or resizes it. A timeline is
                // grabbed by its range handles or selected band instead of being moved itself.
                if app.grid.drag == Drag::None {
                    if kind == "timeline" {
                        if let Some(d) = crate::slicer_view::begin_timeline_drag(app, geo, sh, wb, p) {
                            app.grid.drag = Drag::Timeline(d);
                        }
                    } else if let Some(a) = object_anchor(sh, kind, id) {
                        app.grid.drag = Drag::Object(ObjectDrag::new(kind, id, p, crate::chartview::frame_of(sh, &a), handle));
                    }
                }
            }
            return;
        } else {
            app.selected_chart = None;
        }
    }
    // A right-click on a chart, picture, shape or slicer opens the object's menu, not the cell's
    // under it; a right press that wobbles into a drag counts too (#2067).
    let secondary_drag = resp.drag_started_by(egui::PointerButton::Secondary);
    if (resp.secondary_clicked() || secondary_drag)
        && let Some(p) = if secondary_drag { pointer.press_origin().map(|p| geo.mirror_pos(p)).or(pos) } else { pos }
        && let Some((kind, id, _)) = crate::chartview::hit(app, geo, sh, p)
    {
        app.selected_chart = Some(id);
        app.grid.object_menu = Some((geo.mirror_pos(p), kind, id));
        return;
    }

    // A double-click is also a click: handle header AutoFit before resize setup returns.
    if resp.double_clicked()
        && let Some(p) = pos
    {
        let unhide = if in_col_header {
            hidden_col_edge(geo, sh, p.x).map(|col| ("home.unhideColumns", json!({"cols": RangeRef::cols(col, col).a1()})))
        } else if in_row_header {
            hidden_row_edge(geo, sh, p.y).map(|row| ("home.unhideRows", json!({"rows": RangeRef::rows(row, row).a1()})))
        } else {
            None
        };
        let autofit = if unhide.is_some() {
            unhide
        } else if in_col_header {
            col_edge(geo, sh, p.x).map(|(col, _)| {
                let selected = sel.ranges.iter().any(|r| r.is_full_cols() && col >= r.start.col && col <= r.end.col);
                ("home.autofitColumnWidth", if selected { json!({}) } else { json!({"cols": RangeRef::cols(col, col).a1()}) })
            })
        } else if in_row_header {
            row_edge(geo, sh, p.y).map(|(row, _)| {
                let selected = sel.ranges.iter().any(|r| r.is_full_rows() && row >= r.start.row && row <= r.end.row);
                ("home.autofitRowHeight", if selected { json!({}) } else { json!({"rows": RangeRef::rows(row, row).a1()}) })
            })
        } else {
            None
        };
        if let Some((command, params)) = autofit {
            app.grid.drag = Drag::None;
            app.grid.drag_select = false;
            app.run_or_alert(command, params);
            resp.request_focus();
            return;
        }
    }

    // Double-click the fill handle: a double-click is also a click, so run it before the handle press returns.
    if resp.double_clicked()
        && let Some(p) = pos
        && on_handle(p)
    {
        // Fill down as far as the neighbouring column goes.
        let src = sel.current();
        let neighbour = if src.start.col > 0 { src.start.col - 1 } else { src.end.col + 1 };
        let cap = src.end.row.saturating_add(1_000_000).min(MAX_ROWS - 1);
        let last = if src.end.row < cap { sh.last_contiguous_nonempty(neighbour, src.end.row + 1, cap) } else { src.end.row };
        if last > src.end.row {
            let target = RangeRef::new(src.start, CellRef::new(last, src.end.col));
            let _ = app.run("edit.autoFill", json!({"source": src.a1(), "target": target.a1()}));
        }
        app.grid.drag = Drag::None;
        app.grid.drag_select = false;
        resp.request_focus();
        return;
    }

    // Press: decide the drag mode.
    if resp.drag_started() || (primary_clicked && app.grid.drag == Drag::None) {
        // Where the press began (a drag is only recognised after the pointer has moved).
        let Some(p) = pointer.press_origin().map(|p| geo.mirror_pos(p)).or(pos) else { return };
        // The outline band's − / + and level buttons act on release, like buttons.
        if let Some(action) = crate::outline::hit(geo, sh, p) {
            if primary_clicked {
                let (command, params) = action.command();
                app.run_or_alert(command, params);
            }
            return;
        }
        // Freeze handles sit on the header edges (and the corner's edges when nothing is frozen).
        if freeze_cols.contains(p) || freeze_rows.contains(p) {
            app.grid.drag = Drag::Freeze { cols: freeze_cols.contains(p), to: None };
            return;
        }
        if in_corner {
            let _ = app.session.run("selection.set", json!({"range": RangeRef::all().a1()}));
            return;
        }
        if in_col_header {
            if let Some((c, x)) = col_edge(geo, sh, p.x) {
                app.grid.drag = Drag::ResizeCol { col: c, start: p.x, orig: sh.col_width(c).max(0.0) * 0.0 + (x - geo.x(sh, c)) / geo.z };
                return;
            }
            let c = geo.col_at(sh, p.x);
            // A Table heading's filter button, while the header names stand in for the letters.
            if let Some(h) = table_headings(geo, sh, sel.active).into_iter().find(|h| h.col == c && h.button.is_some_and(|b| b.contains(p))) {
                app.grid.filter_menu = Some((c, h.header_row, geo.mirror_pos(p)));
                return;
            }
            if point_header(app, wb, sh, RangeRef::cols(c, c), mods.shift, mods.command) {
                return;
            }
            // An armed Format Painter paints the columns dragged over instead of moving them.
            if resp.drag_started()
                && !mods.any()
                && app.session.format_painter.is_none()
                && sel.ranges.len() == 1
                && sel.current().is_full_cols()
                && sel.current().contains(CellRef::new(0, c))
            {
                app.grid.drag = Drag::MoveCols { source: sel.current(), before: None };
                return;
            }
            let range = if mods.shift { RangeRef::cols(sel.anchor.col.min(c), sel.anchor.col.max(c)) } else { RangeRef::cols(c, c) };
            select_ranges(app, range, mods.command, CellRef::new(geo.row_at(sh, geo.cells.top() + 1.0), c));
            app.grid.drag = Drag::Cols(if mods.shift { sel.anchor.col } else { c });
            return;
        }
        if in_row_header {
            if let Some((r, y)) = row_edge(geo, sh, p.y) {
                app.grid.drag = Drag::ResizeRow { row: r, start: p.y, orig: (y - geo.y(sh, r)) / geo.z };
                return;
            }
            let r = geo.row_at(sh, p.y);
            if point_header(app, wb, sh, RangeRef::rows(r, r), mods.shift, mods.command) {
                return;
            }
            let range = if mods.shift { RangeRef::rows(sel.anchor.row.min(r), sel.anchor.row.max(r)) } else { RangeRef::rows(r, r) };
            select_ranges(app, range, mods.command, CellRef::new(r, geo.col_at(sh, geo.cells.left() + 1.0)));
            app.grid.drag = Drag::Rows(if mods.shift { sel.anchor.row } else { r });
            return;
        }
        if !in_cells {
            return;
        }
        let c = geo.cell_at(sh, p);
        // Data validation list arrow next to the active cell.
        if let Some(r) = validation_arrow(sh, geo, sel.active)
            && r.contains(p)
        {
            app.grid.list_picker = Some(sel.active);
            return;
        }
        // Filter dropdown buttons.
        if is_filter_button(sh, geo, p) {
            app.grid.filter_menu = Some((c.col, c.row, geo.mirror_pos(p)));
            return;
        }
        // Point mode while editing a formula: clicking inserts a reference.
        if let Some(ed) = app.editor.as_mut()
            && ed.can_point()
        {
            let anchor = if mods.shift && ed.point_at_caret() {
                ed.point_anchor.filter(|(sheet, _)| *sheet == wb.active_sheet).map(|(_, cell)| cell).unwrap_or(c)
            } else {
                c
            };
            let range = RangeRef::new(anchor, c);
            let text = if range.is_single() { c.a1() } else { range.a1() };
            let sheet_name = (ed.sheet != wb.active_sheet).then(|| sh.name.clone());
            let r = match sheet_name {
                Some(n) => format!("{}!{text}", gridcraft_engine::formula::quote_sheet(&n)),
                None => text,
            };
            if mods.command && !mods.shift {
                ed.append_ref(&r);
            } else {
                ed.insert_ref(&r);
            }
            ed.request_focus = true;
            ed.point_cell = Some(c);
            ed.point_anchor = Some((wb.active_sheet, anchor));
            app.grid.drag = Drag::Select;
            app.grid.drag_select = true;
            if resp.drag_started()
                && let Some(p) = pos
            {
                select_to_pointer(app, geo, sh, wb, p);
            }
            return;
        }
        if app.editor.is_some() && !app.commit_edit(0, 0, false, false) {
            return;
        }
        if on_handle(p) {
            app.grid.drag = Drag::Fill { target: None, menu: resp.drag_started_by(egui::PointerButton::Secondary) };
            return;
        }
        if on_cur_border(p) && resp.drag_started() {
            app.grid.drag = Drag::Move { grab: c, to: None };
            return;
        }
        // Double-click edits.
        let now = crate::now_ms();
        if let Some((last, at)) = app.grid.last_click
            && last == c
            && now - at < 450.0
            && !mods.shift
        {
            app.grid.last_click = None;
            // Following a hyperlink requires a single click in Excel; double-click edits.
            app.begin_edit(None, false);
            return;
        }
        app.grid.last_click = Some((c, now));
        if mods.shift {
            let _ = app.session.run("selection.set", json!({"range": RangeRef::new(sel.anchor, c).a1(), "active": sel.active.a1()}));
            if let Some(d) = app.session.active_mut() {
                d.selection.anchor = sel.anchor;
            }
        } else if mods.command {
            select_ranges(app, RangeRef::cell(c), true, c);
        } else {
            let _ = app.session.run("selection.set", json!({"cell": c.a1()}));
            if primary_clicked && wb.styles.get(sh.style_id(c)).num_fmt.as_str() == "checkbox" {
                let _ = app.run("cell.toggleCheckbox", json!({"cell": c.a1()}));
            }
            // A click on a hyperlink's text follows it (like Excel); elsewhere in the cell selects.
            if let Some(h) = sh.hyperlinks.get(&c).cloned()
                && primary_clicked
                && !mods.any()
            {
                let r = geo.cell_rect(sh, c);
                if p.x < r.left() + r.width() * 0.8 {
                    follow_link(app, &h.target);
                }
            }
        }
        app.grid.drag = Drag::Select;
        app.grid.drag_select = true;
        if resp.drag_started()
            && let Some(p) = pos
        {
            // The first recognised drag already includes movement away from the press.
            select_to_pointer(app, geo, sh, wb, p);
        }
        return;
    }

    // Dragging.
    if resp.dragged()
        && let Some(p) = pos
    {
        // Auto-scroll near edges.
        let edge = 24.0;
        let mut auto = egui::Vec2::ZERO;
        if p.y > geo.cells.bottom() - edge {
            auto.y = 20.0;
        }
        if p.y < geo.cells.top() + edge * 0.5 && !matches!(app.grid.drag, Drag::ResizeRow { .. }) {
            auto.y = -20.0;
        }
        if p.x > geo.cells.right() - edge {
            auto.x = 30.0;
        }
        if p.x < geo.cells.left() + edge * 0.5 && !matches!(app.grid.drag, Drag::ResizeCol { .. }) {
            auto.x = -30.0;
        }
        let pc = CellRef::new(
            geo.row_at(sh, p.y.clamp(geo.cells.top() + 1.0, geo.cells.bottom() - 1.0)),
            geo.col_at(sh, p.x.clamp(geo.cells.left() + 1.0, geo.cells.right() - 1.0)),
        );
        // Esc ends an object drag where it started. The drag records one undo entry, so one undo
        // puts the object back and the workbook is not left dirty by a cancelled drag.
        if app.grid.drag.is_object() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            let steps = std::mem::take(&mut app.grid.drag).steps();
            if steps > 0 {
                let _ = app.session.run("edit.undo", json!({}));
            }
            app.toast = None;
            return;
        }
        match app.grid.drag.clone() {
            Drag::Select => {
                select_to_pointer(app, geo, sh, wb, p);
                scroll_by(app, auto);
            }
            Drag::Rows(anchor) => {
                let r = geo.row_at(sh, p.y);
                let _ =
                    app.session.run("selection.set", json!({"range": RangeRef::rows(anchor.min(r), anchor.max(r)).a1(), "active": sel.active.a1()}));
                scroll_by(app, vec2(0.0, auto.y));
            }
            Drag::Cols(anchor) => {
                let c = geo.col_at(sh, p.x);
                let _ =
                    app.session.run("selection.set", json!({"range": RangeRef::cols(anchor.min(c), anchor.max(c)).a1(), "active": sel.active.a1()}));
                scroll_by(app, vec2(auto.x, 0.0));
            }
            Drag::MoveCols { source, .. } => {
                let col = geo.col_at(sh, p.x.clamp(geo.cells.left() + 1.0, geo.cells.right() - 1.0));
                let midpoint = geo.x(sh, col) + sh.col_width(col) * geo.z * 0.5;
                let before = (col + u32::from(p.x >= midpoint)).min(MAX_COLS - 1);
                let before = (!(source.start.col..=source.end.col + 1).contains(&before)).then_some(before);
                app.grid.drag = Drag::MoveCols { source, before };
                scroll_by(app, vec2(auto.x, 0.0));
            }
            Drag::ResizeCol { col, start, orig } => {
                let w = (orig + (p.x - start) / geo.z).max(0.0);
                app.toast =
                    Some((format!("Width: {:.2} ({} pixels)", gridcraft_engine::cmd::format::points_to_chars(w as f64), w.round()), crate::now_ms()));
                // Inside a selection of whole columns, every selected column takes the new width
                // (Ctrl-selected areas included); otherwise only the dragged one.
                let cols = if sel.ranges.iter().any(|r| r.is_full_cols() && col >= r.start.col && col <= r.end.col) {
                    sel.ranges.iter().filter(|r| r.is_full_cols()).map(|r| r.a1()).collect::<Vec<_>>().join(",")
                } else {
                    RangeRef::cols(col, col).a1()
                };
                let _ = app.session.run("home.columnWidth", json!({"cols": cols, "width": w.round()}));
                coalesce_last_undo(app);
            }
            Drag::ResizeRow { row, start, orig } => {
                let h = (orig + (p.y - start) / geo.z).max(0.0);
                app.toast = Some((format!("Height: {:.2} ({} pixels)", h * 0.75, h.round()), crate::now_ms()));
                let rows = if sel.ranges.iter().any(|r| r.is_full_rows() && row >= r.start.row && row <= r.end.row) {
                    sel.ranges
                        .iter()
                        .filter(|r| r.is_full_rows())
                        .map(|r| format!("{}:{}", r.start.row + 1, r.end.row + 1))
                        .collect::<Vec<_>>()
                        .join(",")
                } else {
                    format!("{}:{}", row + 1, row + 1)
                };
                let _ = app.session.run("home.rowHeight", json!({"rows": rows, "height": h.round()}));
                coalesce_last_undo(app);
            }
            Drag::Freeze { cols, .. } => {
                app.grid.drag = Drag::Freeze { cols, to: Some(freeze_boundary(geo, sh, p, cols)) };
            }
            Drag::Fill { menu, .. } => {
                let src = sel.current();
                let target = fill_target(src, pc);
                app.grid.drag = Drag::Fill { target: Some(target), menu };
                scroll_by(app, auto);
            }
            Drag::Move { grab, .. } => {
                let src = sel.current();
                let dr = pc.row as i64 - grab.row as i64;
                let dc = pc.col as i64 - grab.col as i64;
                app.grid.drag = Drag::Move { grab, to: Some(src.start.offset_clamped(dr, dc)) };
                scroll_by(app, auto);
            }
            Drag::Object(d) => {
                let shift = ctx.input(|i| i.modifiers.shift);
                let delta = (p - d.start) / geo.z;
                let f = crate::chartview::drag_frame(d.orig, d.edges(), delta, shift);
                let (cell, dx, dy, w, h) = crate::chartview::anchor_of(sh, f);
                let (command, params) = object_move_call(d.kind, d.id, cell, dx, dy, w, h);
                let before = app.session.active().map(|x| x.revision).unwrap_or(0);
                // A pivot slicer moves through `slicer.move`, which stores only whole-cell anchors:
                // skip a frame whose target its command cannot represent rather than recording an
                // undo entry that changes nothing.
                let unchanged = object_anchor(sh, d.kind, d.id).is_some_and(|a| {
                    a.cell == cell
                        && (a.width - w).abs() < 0.01
                        && (a.height - h).abs() < 0.01
                        && (d.kind == "pivotSlicer" || ((a.dx - dx).abs() < 0.01 && (a.dy - dy).abs() < 0.01))
                });
                if !unchanged {
                    let _ = app.session.run(command, params);
                }
                let changed = app.session.active().map(|x| x.revision).unwrap_or(0) != before;
                let mut d = d;
                if changed {
                    // One drag is one undo step: its first change opens the entry, later changes
                    // join it rather than piling up a step per frame.
                    if d.steps > 0 {
                        coalesce_last_undo(app);
                    }
                    d.steps = d.steps.saturating_add(1);
                }
                app.toast = Some((object_toast(d.kind, &f, d.handle.is_none()), crate::now_ms()));
                app.grid.drag = Drag::Object(d);
                scroll_by(app, auto);
            }
            Drag::None => {}
            Drag::Timeline(d) => {
                let range = crate::slicer_view::dragged_range(&d, p);
                let current = sh.pivot_timelines.iter().find(|t| t.id == d.id).and_then(|t| t.range);
                // A handle dragged back to the axis's own end on a timeline that shows every date
                // says nothing new: no command, no empty undo entry.
                let full = range.start == d.bounds.0 && range.end == d.bounds.1;
                if current != Some(range)
                    && !(current.is_none() && full)
                    && let Some(params) = crate::slicer_view::range_params(wb, d.id, range)
                {
                    let before = app.session.active().map(|x| x.revision).unwrap_or(0);
                    let _ = app.session.run("slicer.setRange", params);
                    let changed = app.session.active().map(|x| x.revision).unwrap_or(0) != before;
                    let mut d = d;
                    if changed {
                        if d.steps > 0 {
                            coalesce_last_undo(app);
                        }
                        d.steps = d.steps.saturating_add(1);
                    }
                    app.toast = Some((crate::slicer_view::range_toast(wb, range), crate::now_ms()));
                    app.grid.drag = Drag::Timeline(d);
                }
            }
        }
    }

    // Release.
    if resp.drag_stopped() || (!pointer.any_down() && app.grid.drag != Drag::None && !resp.dragged()) {
        // Esc mid-drag (egui ends the drag on Esc too): the object goes back, and the drag's own
        // undo entry is dropped with it, so a cancelled drag leaves no step behind.
        if app.grid.drag.is_object() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            let steps = std::mem::take(&mut app.grid.drag).steps();
            if steps > 0 {
                let _ = app.session.run("edit.undo", json!({}));
            }
            app.toast = None;
            app.grid.drag_select = false;
            return;
        }
        match std::mem::take(&mut app.grid.drag) {
            Drag::Select => {
                if resp.drag_stopped()
                    && let Some(p) = pos
                {
                    select_to_pointer(app, geo, sh, wb, p);
                }
                format_painter_apply(app);
            }
            // Whole rows or columns clicked or dragged over by their headers.
            Drag::Rows(_) | Drag::Cols(_) => format_painter_apply(app),
            // Esc mid-drag lands here too (egui ends the drag on Esc): no menu, and the key is
            // spent on the drag so the grid's Esc doesn't also drop the copy.
            Drag::Fill { target: Some(t), menu: true } => {
                if let Some(p) = pos
                    && t != sel.current()
                    && !ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
                {
                    app.grid.fill_menu = Some((sel.current(), t, geo.mirror_pos(p)));
                }
            }
            Drag::Fill { target: Some(t), menu: false } => {
                let src = sel.current();
                if t != src {
                    // Ctrl (Cmd on macOS) swaps copying and extending, as in Excel.
                    let mode = if mods.command { "alternate" } else { "series" };
                    if let Err(e) = app.run("edit.autoFill", json!({"source": src.a1(), "target": t.a1(), "mode": mode})) {
                        app.message = Some(("GridCraft".into(), crate::clean_error(&e)));
                    }
                }
            }
            Drag::Freeze { cols, to: Some(to) } => {
                let (rows_now, cols_now) = (geo.fr, geo.fc);
                let (rows, cols) = if cols { (rows_now, to) } else { (to, cols_now) };
                if (rows, cols) != (rows_now, cols_now) {
                    // A1 (nothing left frozen) unfreezes.
                    app.run_or_alert("view.freezePanes", json!({"cell": CellRef::new(rows, cols).a1()}));
                }
            }
            Drag::Move { to: Some(to), .. } => {
                let src = sel.current();
                if to != src.start {
                    let _ = app.session.run("edit.cut", json!({"range": src.a1()}));
                    let _ = app.run("edit.paste", json!({"at": to.a1()}));
                }
            }
            Drag::MoveCols { source, before: Some(before) } => {
                if let Err(e) = app.run("sheet.moveColumns", json!({"cols": source.a1(), "before": before})) {
                    app.message = Some(("GridCraft".into(), crate::clean_error(&e)));
                }
            }
            _ => {}
        }
        app.grid.drag_select = false;
    }

    // Right-click: context menu.
    if resp.secondary_clicked()
        && let Some(p) = pos
    {
        if in_cells && !sel.contains(geo.cell_at(sh, p)) {
            let _ = app.session.run("selection.set", json!({"cell": geo.cell_at(sh, p).a1()}));
        }
        if in_col_header || in_row_header {
            let (rows, idx) = if in_row_header { (true, geo.row_at(sh, p.y)) } else { (false, geo.col_at(sh, p.x)) };
            let inside = sel.ranges.iter().any(|r| {
                if rows {
                    r.is_full_rows() && idx >= r.start.row && idx <= r.end.row
                } else {
                    r.is_full_cols() && idx >= r.start.col && idx <= r.end.col
                }
            });
            if !inside {
                let range = if rows { RangeRef::rows(idx, idx) } else { RangeRef::cols(idx, idx) };
                let _ = app.session.run("selection.set", json!({"range": range.a1()}));
            }
            app.grid.menu_nav = None;
            app.grid.header_menu = Some((geo.mirror_pos(p), rows));
        } else {
            app.grid.menu_nav = None;
            app.grid.context_menu = Some(geo.mirror_pos(p));
            app.grid.context_menu_frame = ui.ctx().cumulative_frame_nr();
        }
    }

    if primary_clicked || resp.drag_started() {
        resp.request_focus();
    }
    keyboard(app, &ctx, resp, geo, sh);
}

/// An armed Format Painter pastes onto the cells just clicked or dragged over (not while
/// pointing a formula reference, which also drags a Select).
fn format_painter_apply(app: &mut SheetApp) {
    if app.session.format_painter.is_some() && !app.editor.as_ref().is_some_and(|ed| ed.can_point()) {
        let dest = app.session.active().map(|d| d.selection.current().a1());
        if let Some(dest) = dest
            && let Err(e) = app.run("edit.formatPainter", json!({"apply": dest}))
        {
            app.message = Some(("GridCraft".into(), crate::clean_error(&e)));
        }
    }
}

/// Point mode on a header: inserts a whole-row or whole-column reference instead of selecting.
fn point_header(app: &mut SheetApp, wb: &Workbook, sh: &Sheet, range: RangeRef, extend: bool, add: bool) -> bool {
    let Some(ed) = app.editor.as_mut().filter(|ed| ed.can_point()) else { return false };
    let edit_sheet = ed.sheet;
    let active_sheet = wb.active_sheet;
    let range = if extend {
        ed.point
            .and_then(|point| crate::editor::formula_refs(&ed.text).into_iter().find(|(start, end, ..)| (*start, *end) == point))
            .and_then(|(_, _, previous, previous_sheet, _)| {
                let previous_sheet = previous_sheet.as_deref().map(|name| wb.sheet_index(name)).unwrap_or(Some(edit_sheet));
                if previous_sheet != Some(active_sheet) {
                    return None;
                }
                if range.is_full_cols() && previous.is_full_cols() {
                    Some(RangeRef::cols(previous.start.col.min(range.start.col), previous.end.col.max(range.end.col)))
                } else if range.is_full_rows() && previous.is_full_rows() {
                    Some(RangeRef::rows(previous.start.row.min(range.start.row), previous.end.row.max(range.end.row)))
                } else {
                    None
                }
            })
            .unwrap_or(range)
    } else {
        range
    };
    let reference = range.a1();
    let reference = if ed.sheet != wb.active_sheet { format!("{}!{reference}", gridcraft_engine::formula::quote_sheet(&sh.name)) } else { reference };
    if add {
        ed.append_ref(&reference);
    } else {
        ed.insert_ref(&reference);
    }
    ed.point_anchor = None;
    ed.point_cell = Some(range.start);
    ed.request_focus = true;
    true
}

fn select_to_pointer(app: &mut SheetApp, geo: &Geo, sh: &Sheet, wb: &Workbook, p: Pos2) {
    let cell = CellRef::new(
        geo.row_at(sh, p.y.clamp(geo.cells.top() + 1.0, geo.cells.bottom() - 1.0)),
        geo.col_at(sh, p.x.clamp(geo.cells.left() + 1.0, geo.cells.right() - 1.0)),
    );
    if let Some(ed) = app.editor.as_mut()
        && ed.point.is_some()
    {
        if let Some(start) = ed.point_anchor.filter(|(sheet, _)| *sheet == wb.active_sheet).map(|(_, cell)| cell).or(ed.point_cell) {
            let range = RangeRef::new(start, cell);
            let text = if range.is_single() { range.start.a1() } else { range.a1() };
            let text = if ed.sheet != wb.active_sheet { format!("{}!{text}", gridcraft_engine::formula::quote_sheet(&sh.name)) } else { text };
            ed.insert_ref(&text);
            ed.point_cell = Some(cell);
            ed.point_anchor = Some((wb.active_sheet, start));
        }
    } else if let Some(d) = app.session.active() {
        // Read the current anchor: starting a drag may have just replaced the selection.
        let anchor = d.selection.anchor;
        let active = d.selection.active;
        let mut ranges: Vec<String> = d.selection.ranges.iter().map(|r| r.a1()).collect();
        ranges.pop();
        ranges.push(RangeRef::new(anchor, cell).a1());
        let _ = app.session.run("selection.set", json!({"range": ranges.join(","), "active": active.a1()}));
        if let Some(d) = app.session.active_mut() {
            d.selection.anchor = anchor;
        }
    }
}

fn coalesce_last_undo(app: &mut SheetApp) {
    // Merge consecutive drag steps into one undo entry.
    if let Some(d) = app.session.active_mut()
        && d.undo.len() >= 2
    {
        let n = d.undo.len();
        if d.undo[n - 1].label == d.undo[n - 2].label {
            d.undo.remove(n - 1);
        }
    }
}

fn scroll_by(app: &mut SheetApp, d: egui::Vec2) {
    if d != egui::Vec2::ZERO
        && let Some(v) = app.view_mut()
    {
        v.scroll = (v.scroll + d).max(egui::Vec2::ZERO);
    }
}

fn fill_target(src: RangeRef, pc: CellRef) -> RangeRef {
    let below = pc.row as i64 - src.end.row as i64;
    let above = src.start.row as i64 - pc.row as i64;
    let right = pc.col as i64 - src.end.col as i64;
    let left = src.start.col as i64 - pc.col as i64;
    let vert = below.max(above);
    let horiz = right.max(left);
    if vert <= 0 && horiz <= 0 {
        return src;
    }
    if vert >= horiz {
        if below > 0 {
            RangeRef::new(src.start, CellRef::new(pc.row, src.end.col))
        } else {
            RangeRef::new(CellRef::new(pc.row, src.start.col), src.end)
        }
    } else if right > 0 {
        RangeRef::new(src.start, CellRef::new(src.end.row, pc.col))
    } else {
        RangeRef::new(CellRef::new(src.start.row, pc.col), src.end)
    }
}

fn on_border(r: Rect, p: Pos2) -> bool {
    let outer = r.expand(3.0);
    let inner = r.shrink(3.0);
    outer.contains(p) && !inner.contains(p)
}

/// The freeze handles, as in Excel for the web: a bar in the upper part of the column-header strip
/// on the frozen columns' right edge, and one in the left part of the row-header strip on the
/// frozen rows' bottom edge. With nothing frozen they sit on the corner's edges. The rest of the
/// header edge keeps resizing the column or row.
pub fn freeze_handles(geo: &Geo) -> (Rect, Rect) {
    let x = geo.cells.left() + geo.frozen_w;
    let y = geo.cells.top() + geo.frozen_h;
    let cols = Rect::from_min_max(pos2(x - 3.0, geo.head_top()), pos2(x + 3.0, geo.head_top() + geo.header_h * 0.4));
    let rows = Rect::from_min_max(pos2(geo.head_left(), y - 3.0), pos2(geo.head_left() + geo.header_w * 0.4, y + 3.0));
    (cols, rows)
}

/// How many columns (or rows) a freeze handle dropped at `p` freezes: the line boundary nearest
/// the pointer, limited so the frozen pane leaves room for the scrolling one.
fn freeze_boundary(geo: &Geo, sh: &Sheet, p: Pos2, cols: bool) -> u32 {
    if cols {
        if p.x <= geo.cells.left() {
            return 0;
        }
        let c = geo.col_at(sh, p.x);
        let (l, r) = (geo.x(sh, c), geo.x(sh, c.saturating_add(1).min(MAX_COLS - 1)));
        let b = if p.x - l > r - p.x { c.saturating_add(1) } else { c };
        let room = geo.ci.at((((geo.cells.width() - 40.0) / geo.z).max(0.0)) as f64);
        b.min(room).min(MAX_COLS - 1)
    } else {
        if p.y <= geo.cells.top() {
            return 0;
        }
        let rr = geo.row_at(sh, p.y);
        let (t, b2) = (geo.y(sh, rr), geo.y(sh, rr.saturating_add(1).min(MAX_ROWS - 1)));
        let b = if p.y - t > b2 - p.y { rr.saturating_add(1) } else { rr };
        let room = geo.ri.at((((geo.cells.height() - 40.0) / geo.z).max(0.0)) as f64);
        b.min(room).min(MAX_ROWS - 1)
    }
}

/// Paints the freeze handles and, while one is dragged, where the freeze line will go.
fn paint_freeze_handles(p: &Painter, geo: &Geo, sh: &Sheet, drag: &Drag, t: &Tokens) {
    let (cols, rows) = freeze_handles(geo);
    let bar = Color32::from_gray(150);
    p.rect_filled(Rect::from_center_size(cols.center(), vec2(3.0, cols.height())), 1.0, bar);
    p.rect_filled(Rect::from_center_size(rows.center(), vec2(rows.width(), 3.0)), 1.0, bar);
    if let Drag::Freeze { cols, to: Some(to) } = *drag {
        let stroke = Stroke::new(2.0, t.accent);
        if cols {
            let x = geo.cells.left() + sh.col_left(to) as f32 * geo.z;
            p.line_segment([pos2(x, geo.head_top()), pos2(x, geo.cells.bottom())], stroke);
        } else {
            let y = geo.cells.top() + sh.row_top(to) as f32 * geo.z;
            p.line_segment([pos2(geo.head_left(), y), pos2(geo.cells.right(), y)], stroke);
        }
    }
}

fn col_edge(geo: &Geo, sh: &Sheet, x: f32) -> Option<(u32, f32)> {
    let c = geo.col_at(sh, x);
    for cc in [c, c.saturating_sub(1)] {
        let edge = geo.x(sh, cc) + sh.col_width(cc) * geo.z;
        if (x - edge).abs() <= 4.0 {
            return Some((cc, edge));
        }
    }
    None
}

fn row_edge(geo: &Geo, sh: &Sheet, y: f32) -> Option<(u32, f32)> {
    let r = geo.row_at(sh, y);
    for rr in [r, r.saturating_sub(1)] {
        let edge = geo.y(sh, rr) + sh.row_height(rr) * geo.z;
        if (y - edge).abs() <= 3.0 {
            return Some((rr, edge));
        }
    }
    None
}

/// The hidden column whose (zero-width) edge is at `x`: only just right of the boundary, as the
/// edge just left of it belongs to the visible column there.
fn hidden_col_edge(geo: &Geo, sh: &Sheet, x: f32) -> Option<u32> {
    col_edge(geo, sh, x).map(|(c, _)| c).filter(|&c| sh.col_width(c) <= 0.0)
}

fn hidden_row_edge(geo: &Geo, sh: &Sheet, y: f32) -> Option<u32> {
    row_edge(geo, sh, y).map(|(r, _)| r).filter(|&r| sh.row_height(r) <= 0.0)
}

/// A split cursor (two bars with arrows either side) painted in place of the OS cursor, which
/// has no such shape. Black on a white outline, like the system cursors.
fn paint_split_cursor(ctx: &egui::Context, at: Pos2, cols: bool) {
    ctx.set_cursor_icon(CursorIcon::None);
    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("gridcraft.split_cursor")));
    // Drawn for columns (bars vertical); rows swap the axes.
    let pt = |x: f32, y: f32| if cols { pos2(at.x + x, at.y + y) } else { pos2(at.x + y, at.y + x) };
    let arrow = |dir: f32| vec![pt(dir * 10.0, 0.0), pt(dir * 6.0, -4.0), pt(dir * 6.0, 4.0)];
    for (width, color) in [(3.0, Color32::WHITE), (1.0, Color32::BLACK)] {
        let stroke = Stroke::new(width, color);
        for bx in [-1.5, 1.5] {
            p.line_segment([pt(bx, -8.0), pt(bx, 8.0)], stroke);
        }
        p.line_segment([pt(-7.0, 0.0), pt(7.0, 0.0)], stroke);
        for dir in [-1.0, 1.0] {
            p.add(egui::Shape::convex_polygon(arrow(dir), color, Stroke::new(width - 1.0, color)));
        }
    }
}

fn is_filter_button(sh: &Sheet, geo: &Geo, p: Pos2) -> bool {
    let c = geo.cell_at(sh, p);
    sh.has_filter_button(c) && filter_button_rect(geo, sh, c).contains(p)
}

fn object_anchor(sh: &Sheet, kind: &str, id: u32) -> Option<gridcraft_engine::model::Anchor> {
    match kind {
        "chart" => sh.charts.iter().find(|c| c.id == id).map(|c| c.anchor),
        "image" => sh.images.iter().find(|c| c.id == id).map(|c| c.anchor),
        "slicer" => sh.slicers.iter().find(|c| c.id == id).map(|c| c.anchor),
        "pivotSlicer" => sh.pivot_slicers.iter().find(|c| c.id == id).map(|c| c.anchor),
        "timeline" => sh.pivot_timelines.iter().find(|c| c.id == id).map(|c| c.anchor),
        _ => sh.shapes.iter().find(|c| c.id == id).map(|c| c.anchor),
    }
}

/// The object `Shift+F10` targets when one is selected: its kind, id and anchor, else none. With
/// no object selected the key opens the cell's context menu instead.
pub(crate) fn selected_object(sh: &Sheet, selected: Option<u32>) -> Option<(&'static str, u32, gridcraft_engine::model::Anchor)> {
    let id = selected?;
    let kind = object_kind(sh, id)?;
    let anchor = object_anchor(sh, kind, id)?;
    Some((kind, id, anchor))
}

/// The kind of object `id` names on `sh`, as `object.move` spells it.
fn object_kind(sh: &Sheet, id: u32) -> Option<&'static str> {
    if sh.charts.iter().any(|o| o.id == id) {
        Some("chart")
    } else if sh.images.iter().any(|o| o.id == id) {
        Some("image")
    } else if sh.slicers.iter().any(|o| o.id == id) {
        Some("slicer")
    } else if sh.pivot_slicers.iter().any(|o| o.id == id) {
        Some("pivotSlicer")
    } else if sh.pivot_timelines.iter().any(|o| o.id == id) {
        Some("timeline")
    } else if sh.shapes.iter().any(|o| o.id == id) {
        Some("shape")
    } else {
        None
    }
}

/// The command that writes a dragged or nudged object's new place. A PivotTable slicer moves
/// through `slicer.move`; that command has no `dx`/`dy`, so only the anchor cell and the size are
/// written and a drag lands on a cell boundary. `timeline` never gets here: it has no move path.
fn object_move_call(kind: &str, id: u32, cell: CellRef, dx: f32, dy: f32, w: f32, h: f32) -> (&'static str, serde_json::Value) {
    if kind == "pivotSlicer" {
        ("slicer.move", json!({"slicer": id, "at": cell.a1(), "width": w, "height": h}))
    } else {
        ("object.move", json!({"kind": kind, "id": id, "at": cell.a1(), "dx": dx, "dy": dy, "width": w, "height": h}))
    }
}

/// Arrow keys on a selected object: moves it by 10 points (1 with Ctrl/Cmd), or resizes it from
/// the edge they point at with Shift. One key press is one undo step. Returns whether an object
/// took the key, so the cells' cursor stays put.
fn nudge_selected_object(app: &mut SheetApp, sh: &Sheet, key: egui::Key, shift: bool, fine: bool) -> bool {
    let Some(id) = app.selected_chart else { return false };
    let Some(kind) = object_kind(sh, id) else { return false };
    // A timeline has no move command (`slicer.move` handles slicers only), so arrows keep their
    // cell meaning for it instead of pretending to move it.
    if kind == "timeline" {
        return false;
    }
    let Some(anchor) = object_anchor(sh, kind, id) else { return false };
    let (dx, dy) = match key {
        egui::Key::ArrowLeft => (-1.0, 0.0),
        egui::Key::ArrowRight => (1.0, 0.0),
        egui::Key::ArrowUp => (0.0, -1.0),
        egui::Key::ArrowDown => (0.0, 1.0),
        _ => return false,
    };
    // Arrows follow the screen: on a right-to-left sheet the drawing is mirrored.
    let delta = if sh.right_to_left { vec2(-dx, dy) } else { vec2(dx, dy) } * if fine { 1.0 } else { 10.0 };
    let edges = if !shift {
        crate::chartview::Edges::MOVE
    } else if delta.x != 0.0 {
        // Resizing from the edge the arrow points at: grow towards it, shrink away from it.
        if delta.x > 0.0 {
            crate::chartview::Edges { right: true, ..Default::default() }
        } else {
            crate::chartview::Edges { left: true, ..Default::default() }
        }
    } else if delta.y > 0.0 {
        crate::chartview::Edges { bottom: true, ..Default::default() }
    } else {
        crate::chartview::Edges { top: true, ..Default::default() }
    };
    let f = crate::chartview::drag_frame(crate::chartview::frame_of(sh, &anchor), edges, delta, false);
    let (cell, adx, ady, w, h) = crate::chartview::anchor_of(sh, f);
    // `slicer.move` stores the anchor cell and the size but has no dx/dy, so a nudge that stays
    // inside a cell has nothing to write: keep the key and add no empty undo step.
    if kind == "pivotSlicer" && anchor.cell == cell && (anchor.width - w).abs() < 0.01 && (anchor.height - h).abs() < 0.01 {
        return true;
    }
    let (command, params) = object_move_call(kind, id, cell, adx, ady, w, h);
    let _ = app.session.run(command, params);
    app.toast = Some((object_toast(kind, &f, !shift), crate::now_ms()));
    true
}

fn delete_selected_object(app: &mut SheetApp, sh: &Sheet) -> bool {
    let Some(id) = app.selected_chart else { return false };
    let Some(kind) = object_kind(sh, id) else { return false };
    if matches!(kind, "pivotSlicer" | "timeline") {
        // A PivotTable widget has its own delete command, which also clears the report's filter.
        app.run_or_alert("slicer.delete", json!({"slicer": id}));
    } else {
        app.run_or_alert("object.delete", json!({"kind": kind, "id": id}));
    }
    app.selected_chart = None;
    true
}

/// A click on a hyperlink. References select; web and e-mail addresses ask first (the dialog's
/// OK runs `link.open`); every other kind is refused with a message. The decision is the
/// engine's ([`gridcraft_engine::links::classify`]), the same one `link.open` applies.
pub fn follow_link(app: &mut SheetApp, target: &str) {
    use gridcraft_engine::links::{LinkKind, classify};
    match classify(target) {
        LinkKind::Internal(r) => {
            let _ = app.session.run("edit.goTo", json!({"reference": r.trim_start_matches('#')}));
            app.grid.ensure_visible = true;
        }
        LinkKind::External(url) => app.open_dialog("openLink", json!({"url": url})),
        LinkKind::Refused(why) => app.message = Some(("Hyperlink".into(), why)),
    }
}

/// The areas after a click: with `add` (Ctrl) the clicked area is added, or removed when it is already
/// selected (unless it is the only one, so the selection never ends up empty).
fn toggled_ranges(mut ranges: Vec<String>, clicked: String, add: bool) -> Vec<String> {
    if !add {
        return vec![clicked];
    }
    if ranges.len() > 1 && ranges.contains(&clicked) {
        ranges.retain(|x| *x != clicked);
    } else if !ranges.contains(&clicked) {
        ranges.push(clicked);
    }
    ranges
}

fn select_ranges(app: &mut SheetApp, r: RangeRef, add: bool, active: CellRef) {
    let current: Vec<String> =
        if add { app.session.active().map(|d| d.selection.ranges.iter().map(|x| x.a1()).collect()).unwrap_or_default() } else { vec![] };
    let clicked = r.a1();
    let ranges = toggled_ranges(current, clicked.clone(), add);
    // A removed area takes the active cell with it; `selection.set` then puts it at the start of the last area.
    let params =
        if ranges.contains(&clicked) { json!({"range": ranges.join(","), "active": active.a1()}) } else { json!({"range": ranges.join(",")}) };
    let _ = app.session.run("selection.set", params);
}

/// Grid keyboard handling when no text field has focus.
fn keyboard(app: &mut SheetApp, ctx: &egui::Context, resp: &egui::Response, geo: &Geo, sh: &Sheet) {
    // Focus left on an editor that no longer exists goes back to the grid.
    let stale = [egui::Id::new("gridcraft.cell_editor"), egui::Id::new("gridcraft.formula_bar"), crate::text_box::editor_id()];
    let editing = app.editor.is_some() || app.text_box_editor.is_some();
    if !editing && ctx.memory(|m| m.focused()).is_some_and(|f| stale.contains(&f)) {
        resp.request_focus();
    }
    let other_focus = ctx.memory(|m| m.focused()).is_some_and(|f| f != resp.id && (editing || !stale.contains(&f)));
    // An open filter menu owns the keyboard even when a click inside it left focus on nothing (#1516).
    // So do the fill menu (its Esc closes it, not the copy) and the right-drag that opens it (an
    // arrow key would move the selection the menu fills from).
    if editing
        || app.dialog.is_some()
        || other_focus
        || app.message.is_some()
        || app.grid.filter_menu.is_some()
        || app.grid.fill_menu.is_some()
        // An open context menu owns the keyboard for its own navigation (grid::menu_keys).
        || app.grid.context_menu.is_some()
        || app.grid.header_menu.is_some()
        || matches!(app.grid.drag, Drag::Fill { menu: true, .. })
    {
        return;
    }
    if ctx.memory(|m| m.focused()).is_none() {
        resp.request_focus();
    }
    // The grid owns arrows, Tab and Escape (egui would otherwise move focus between widgets).
    ctx.memory_mut(|m| {
        m.set_focus_lock_filter(resp.id, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true })
    });
    let events = ctx.input(|i| i.events.clone());
    // A shortcut key that also types a character (Shift+Space sends Text(" ")) must not start an edit.
    let shortcut_pressed = events.iter().any(|e| {
        matches!(e, egui::Event::Key { pressed: true, modifiers: m, key, .. }
            if !m.alt && (m.ctrl || m.command || (m.shift && *key == egui::Key::Space)))
    });
    let page = ((geo.cells.height() / geo.z) / sh.default_row_height).floor().max(1.0) as i64;
    // Alt shortcuts type their character too on Windows and Linux (Alt+= sends Text("=")): once
    // a shortcut ran, that text must not start an edit over what it just did.
    let mut shortcut_ran = false;
    for ev in events {
        match ev {
            egui::Event::Copy => {
                app.copy_to_clipboard(ctx, "edit.copy");
            }
            egui::Event::Cut => {
                app.copy_to_clipboard(ctx, "edit.cut");
            }
            egui::Event::Paste(text) => app.paste_external(text),
            egui::Event::Text(_) if shortcut_pressed || shortcut_ran => {}
            egui::Event::Text(text) if text.chars().all(|c| !c.is_control()) && !text.is_empty() => {
                app.begin_edit(Some(text), false);
                if let Some(ed) = app.editor.as_mut() {
                    ed.caret = ed.text.chars().count();
                }
                return;
            }
            egui::Event::Key { key, physical_key, pressed: true, modifiers: m, .. } => {
                use egui::Key;
                if crate::keymap::dispatch(app, ctx, key, physical_key, m, geo, sh) {
                    shortcut_ran = true;
                    continue;
                }
                let cmd = m.command;
                let shift = m.shift;
                // Tab moves between the value tiles of a selected pivot slicer (Shift+Tab back),
                // keeping grid focus where it is; every other selection keeps Tab for cells.
                // Arrows are not used for tiles: a selected object takes them for nudging.
                if key == Key::Tab && crate::slicer_view::tab(app, sh, if shift { -1 } else { 1 }) {
                    continue;
                }
                // A selected object takes the arrows: move it, or resize it with Shift (Ctrl or
                // Cmd for one-point steps), as Excel does. Escape below drops the selection.
                if matches!(key, Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight)
                    && nudge_selected_object(app, sh, key, shift, cmd || m.ctrl)
                {
                    continue;
                }
                let mv = |app: &mut SheetApp, dr: i64, dc: i64| {
                    let _ = app.session.run("selection.move", json!({"dr": dr, "dc": dc, "extend": shift, "jump": cmd}));
                    app.grid.ensure_visible = true;
                    app.grid.reveal_moving_end = shift;
                };
                match key {
                    Key::ArrowUp => mv(app, -1, 0),
                    Key::ArrowDown => mv(app, 1, 0),
                    // Arrows follow the screen: on a right-to-left sheet Left goes to the next column.
                    Key::ArrowLeft => mv(app, 0, if sh.right_to_left { 1 } else { -1 }),
                    Key::ArrowRight => mv(app, 0, if sh.right_to_left { -1 } else { 1 }),
                    Key::PageDown if !cmd => mv(app, page, 0),
                    Key::PageUp if !cmd => mv(app, -page, 0),
                    Key::PageDown => app.run_or_alert("sheet.next", json!({})),
                    Key::PageUp => app.run_or_alert("sheet.previous", json!({})),
                    Key::Home if cmd => {
                        let r = sh.freeze.map(|f| f.0).unwrap_or(0);
                        let c = sh.freeze.map(|f| f.1).unwrap_or(0);
                        let _ = app.session.run("selection.set", json!({"cell": CellRef::new(r, c).a1()}));
                        app.grid.ensure_visible = true;
                    }
                    Key::End if cmd => {
                        let end = sh.used_range().map(|u| u.end).unwrap_or_default();
                        let _ = app.session.run("selection.set", json!({"cell": end.a1()}));
                        app.grid.ensure_visible = true;
                    }
                    Key::Home => {
                        let a = app.session.active().map(|d| d.selection.active).unwrap_or_default();
                        let _ = app.session.run("selection.set", json!({"cell": CellRef::new(a.row, 0).a1()}));
                        app.grid.ensure_visible = true;
                    }
                    // A focused tile of the selected slicer toggles through `slicer.toggle`.
                    Key::Enter if crate::slicer_view::toggle_focused(app, sh) => {}
                    // Excel: Enter while a copied range is marked pastes it and ends copy mode.
                    Key::Enter if !m.alt && !m.ctrl && !shift && !cmd && app.session.clipboard.is_some() => {
                        app.run_or_alert("edit.paste", json!({}));
                        app.session.clipboard = None;
                        app.grid.ensure_visible = true;
                    }
                    Key::Enter => {
                        if m.alt || m.ctrl {
                            app.begin_edit(None, false);
                        } else {
                            app.move_after_enter(if shift { -1 } else { 1 }, 0);
                        }
                    }
                    Key::Tab => app.move_after_enter(0, if shift { -1 } else { 1 }),
                    Key::F2 => app.begin_edit(None, false),
                    Key::Delete if delete_selected_object(app, sh) => {}
                    Key::Delete => app.run_or_alert("edit.clearContents", json!({})),
                    Key::Backspace if delete_selected_object(app, sh) => {}
                    Key::Backspace => {
                        // Excel for Mac: Delete clears the cell and starts editing it.
                        app.run_or_alert("edit.clearContents", json!({}));
                        if app.session.active().is_some_and(|d| d.selection.is_single_cell()) {
                            app.begin_edit(Some(String::new()), false);
                        }
                    }
                    Key::Escape => {
                        app.session.clipboard = None;
                        app.session.format_painter = None;
                        // Excel: Escape leaves the object and returns to the cells.
                        app.selected_chart = None;
                    }
                    Key::Space if !cmd && !shift && is_checkbox(sh, app) => app.run_or_alert("cell.toggleCheckbox", json!({})),
                    Key::F8 if m.alt => {
                        app.open_dialog("macros", json!({}));
                    }
                    Key::F9 => app.run_or_alert("formulas.calculateNow", json!({})),
                    Key::F4 if !cmd => {}
                    _ if cmd && !crate::keymap::altgr_text(ctx, m, ctx.os() == egui::os::OperatingSystem::Mac) => {
                        crate::ribbon::shortcut(app, key, m)
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn in_cell_editor(app: &mut SheetApp, ui: &mut egui::Ui, geo: &Geo, sh: &Sheet, wb: &Workbook) {
    let Some(ed) = app.editor.as_ref() else { return };
    if ed.sheet != wb.active_sheet {
        return;
    }
    let cell = ed.cell;
    let rect = match sh.merge_at(cell) {
        Some(m) => geo.range_rect(sh, m),
        None => geo.cell_rect(sh, cell),
    };
    let st = wb.styles.get(sh.style_id(cell)).clone();
    let size = st.font.size * 96.0 / 72.0 * geo.z;
    let font = FontId::new(size, theme::cell_family(&st.font.name, st.font.bold, st.font.italic));
    let lines = ed.text.lines().count().max(1) as f32 + if ed.text.ends_with('\n') { 1.0 } else { 0.0 };
    // Wrap Text: the editor keeps the column's width and wraps, growing down as Excel's does.
    // Otherwise it widens to the text's one-line width.
    let wrap = st.align.wrap;
    let (w, h) = if wrap {
        // The field wraps at the width it is given (the editor's less its 2 px inset a side): its
        // frame is `Frame::NONE`, so no text margin is taken off.
        let wrapped = ui.painter().layout(ed.text.clone(), font.clone(), Color32::BLACK, (rect.width() - 4.0).max(1.0));
        (rect.width(), (wrapped.size().y.max(size * 1.3 * lines) + 4.0).max(rect.height()))
    } else {
        let text_w = app.text.measure(ui.ctx(), &font, &ed.text, crate::text::base_direction(&ed.text, sh.right_to_left));
        // Excel keeps the cell's height for one line and only grows for extra lines.
        let h = if lines > 1.0 { (size * 1.3 * lines + 4.0).max(rect.height()) } else { rect.height() };
        ((text_w + 14.0).max(rect.width()).min(geo.cells.right() - rect.left()), h)
    };
    // Grows to the left on a right-to-left sheet.
    let erect = geo.mirror_rect(Rect::from_min_size(rect.min, vec2(w, h)));
    let fill = match st.fill.pattern {
        PatternType::Solid => st.fill.fg.resolve(&wb.theme).map(theme::color32).unwrap_or(Color32::WHITE),
        _ => Color32::WHITE,
    };
    let split = pos2(geo.cells.left() + geo.frozen_w, geo.cells.top() + geo.frozen_h);
    let quadrant = Rect::from_min_max(
        pos2(if cell.col >= geo.fc { split.x } else { geo.cells.left() }, if cell.row >= geo.fr { split.y } else { geo.cells.top() }),
        pos2(if cell.col >= geo.fc { geo.cells.right() } else { split.x }, if cell.row >= geo.fr { geo.cells.bottom() } else { split.y }),
    );
    let clip = ui.clip_rect().intersect(quadrant);
    let painter = ui.painter().with_clip_rect(clip);
    painter.rect_filled(erect, 0.0, fill);
    painter.rect_stroke(erect.expand(1.0), 0.0, Stroke::new(2.0, Tokens::get(ui.ctx()).sel_border), StrokeKind::Middle);
    let id = egui::Id::new("gridcraft.cell_editor");
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(erect.shrink2(vec2(2.0, 1.0))));
    child.set_clip_rect(clip);
    // The theme's caret is meant for the UI chrome and vanishes on some fills (and, in the dark
    // theme, on the white paper): pick black or white against the fill, like Excel.
    child.visuals_mut().text_cursor.stroke.color = ink_on(fill);
    crate::formula_bar::editor_widget(app, &mut child, id, font, wrap, erect.width() - 4.0);
}

/// Test the popup itself, not whether the pointer is over any part of the app.
/// A trigger can sit outside its popup, so its opening click must be ignored.
fn popup_dismissed(response: &egui::Response, was_open: bool) -> bool {
    response.ctx.input(|i| !i.focused || i.key_pressed(egui::Key::Escape)) || (was_open && response.clicked_elsewhere())
}

/// Black or white, whichever reads on `fill` (the threshold chart labels use).
fn ink_on(fill: Color32) -> Color32 {
    let l = 0.299 * fill.r() as f32 + 0.587 * fill.g() as f32 + 0.114 * fill.b() as f32;
    if l < 150.0 { Color32::WHITE } else { Color32::BLACK }
}

/// The table whose header cell (`row`, `col`) was clicked, not just the first table in the column.
fn filter_table_range(sh: &Sheet, col: u32, row: u32) -> Option<RangeRef> {
    sh.tables.iter().find(|t| t.range.start.row == row && col >= t.range.start.col && col <= t.range.end.col).map(|t| t.range)
}

fn filter_menu(app: &mut SheetApp, ui: &mut egui::Ui, geo: &Geo, was_open: bool) {
    let Some((col, row, at)) = app.grid.filter_menu else { return };
    let Some(d) = app.session.active() else { return };
    let wb = d.wb.clone();
    let si = wb.active_sheet;
    let Some(sh) = wb.sheet(si) else { return };
    let _ = geo;
    // Tables get an autofilter on demand.
    let table_range = filter_table_range(sh, col, row);
    if sh.autofilter.is_none()
        && let Some(r) = table_range
    {
        let _ = app.session.run("data.filter", json!({"range": r.a1()}));
        let _ = app.session.run("data.filter", json!({"range": r.a1()}));
        if let Some(sh2) = app.session.active().and_then(|d| d.wb.active()).map(|s| s.autofilter.is_some())
            && !sh2
        {
            let d = app.session.active_mut();
            if let Some(d) = d {
                let wbm = Arc::make_mut(&mut d.wb);
                if let Some(s) = wbm.sheet_mut(si) {
                    s.autofilter = Some(gridcraft_engine::model::AutoFilter { range: r, criteria: vec![] });
                }
            }
        }
    }
    let wb = app.session.active().map(|d| d.wb.clone()).unwrap_or(wb);
    let values = gridcraft_engine::cmd::data::filter_values(&wb, si, col);
    let key = egui::Id::new(("filter_sel", col));
    let search_key = egui::Id::new(("filter_search", col));
    let buttons_key = egui::Id::new(("filter_buttons", col));
    // Enter is the menu's OK, as in Excel: taken before the search box (which would only drop its
    // focus) and the value checkboxes (which would toggle) see it. A focused button keeps it as its click.
    let buttons: Vec<egui::Id> = ui.ctx().data_mut(|d| d.get_temp(buttons_key)).unwrap_or_default();
    let on_button = ui.ctx().memory(|m| m.focused()).is_some_and(|f| buttons.contains(&f));
    let enter = !on_button && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    let mut close = false;
    let area = egui::Area::new(egui::Id::new("filter_menu")).fixed_pos(at + vec2(-180.0, 10.0)).order(egui::Order::Foreground);
    let response = area.show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_width(230.0);
            let ascending = ui.button("↑  Sort Ascending");
            if ascending.clicked() {
                let _ = app.run("data.sortAscending", json!({"column": col_to_letters(col)}));
                close = true;
            }
            let descending = ui.button("↓  Sort Descending");
            if descending.clicked() {
                let _ = app.run("data.sortDescending", json!({"column": col_to_letters(col)}));
                close = true;
            }
            ui.separator();
            ui.label(egui::RichText::new("Filter").strong());
            let mut checks: Vec<(String, bool)> = ui.ctx().data_mut(|d| d.get_temp::<Vec<(String, bool)>>(key)).unwrap_or(values.clone());
            // A search narrows the list; Select All and Apply then only cover the matches.
            let mut search: String = ui.ctx().data_mut(|d| d.get_temp(search_key)).unwrap_or_default();
            let field = ui.add(egui::TextEdit::singleline(&mut search).hint_text("Search").desired_width(f32::INFINITY));
            if !was_open {
                field.request_focus();
            }
            let needle = search.trim().to_lowercase();
            let matches = |v: &str| needle.is_empty() || v.to_lowercase().contains(&needle);
            let all = checks.iter().filter(|(v, _)| matches(v)).all(|(_, b)| *b);
            let mut all_new = all;
            let label = if needle.is_empty() { "(Select All)" } else { "(Select All Search Results)" };
            if ui.checkbox(&mut all_new, label).changed() {
                for c in checks.iter_mut().filter(|(v, _)| matches(v)) {
                    c.1 = all_new;
                }
            }
            // Full width: shrunk to the longest value, the scroll bar sat mid-menu and the wheel only
            // worked over the values themselves (#432).
            egui::ScrollArea::vertical().max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
                for (v, on) in checks.iter_mut().filter(|(v, _)| matches(v)) {
                    ui.checkbox(on, v.as_str());
                }
            });
            ui.ctx().data_mut(|d| {
                d.insert_temp(key, checks.clone());
                d.insert_temp(search_key, search.clone());
            });
            let checks: Vec<(String, bool)> = checks.iter().map(|(v, on)| (v.clone(), *on && matches(v))).collect();
            ui.separator();
            ui.horizontal(|ui| {
                let clear = ui.button("Clear Filter");
                if clear.clicked() {
                    let _ = app.run("data.filterBy", json!({"column": col_to_letters(col), "clear": true}));
                    close = true;
                }
                // Nothing checked would hide every row, so Enter (unlike a click on Apply) does nothing then.
                let any = checks.iter().any(|(_, on)| *on);
                let apply = ui.button("Apply");
                ui.ctx().data_mut(|d| d.insert_temp(buttons_key, vec![ascending.id, descending.id, clear.id, apply.id]));
                if apply.clicked() || (enter && any) {
                    let blanks = checks.iter().any(|(v, on)| v == "(Blanks)" && *on);
                    let vals: Vec<String> = checks.iter().filter(|(v, on)| *on && v != "(Blanks)").map(|(v, _)| v.clone()).collect();
                    let p = if checks.iter().all(|(_, b)| *b) {
                        json!({"column": col_to_letters(col), "clear": true})
                    } else {
                        json!({"column": col_to_letters(col), "values": vals, "blanks": blanks})
                    };
                    let _ = app.run("data.filterBy", p);
                    close = true;
                }
            });
        });
    });
    if close || popup_dismissed(&response.response, was_open) {
        ui.ctx().data_mut(|d| {
            d.remove::<Vec<(String, bool)>>(key);
            d.remove::<String>(search_key);
            d.remove::<Vec<egui::Id>>(buttons_key);
        });
        app.grid.filter_menu = None;
    }
}

/// Where the keyboard is in an open context menu. `items` records the list it was moved in, so a
/// menu whose items changed (Insert Copied Cells appeared) starts unhighlighted instead of
/// activating the wrong row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuNav {
    pub items: usize,
    pub selected: usize,
}

/// The keys an open context menu owns, consumed before the grid's own keyboard pass: arrows,
/// Home/End, Enter/Space and Escape. Nothing is focused, so the grid keeps the keyboard for
/// everything else and loses nothing when the menu closes. Returns `(close, activate)` where
/// `activate` is an index into `items`.
fn menu_keys(ui: &egui::Ui, items: &[(&str, &str)], nav: &mut Option<MenuNav>) -> (bool, Option<usize>) {
    let actionable: Vec<usize> = items.iter().enumerate().filter(|(_, (label, _))| *label != "-").map(|(at, _)| at).collect();
    let Some(first) = actionable.first().copied() else { return (false, None) };
    let (down, up, home, end, enter, space, escape) = ui.input_mut(|i| {
        let plain = egui::Modifiers::NONE;
        (
            i.consume_key(plain, egui::Key::ArrowDown),
            i.consume_key(plain, egui::Key::ArrowUp),
            i.consume_key(plain, egui::Key::Home),
            i.consume_key(plain, egui::Key::End),
            i.consume_key(plain, egui::Key::Enter),
            i.consume_key(plain, egui::Key::Space),
            i.consume_key(plain, egui::Key::Escape),
        )
    });
    // Space also arrives as text: it must not start editing the cell behind the menu.
    ui.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(text) if text.trim().is_empty())));
    if escape {
        return (true, None);
    }
    let current = nav.filter(|n| n.items == items.len()).map(|n| n.selected).filter(|at| actionable.contains(at));
    if down || up || home || end {
        let at = current.and_then(|sel| actionable.iter().position(|item| *item == sel));
        let next = if home {
            actionable.first().copied()
        } else if end {
            actionable.last().copied()
        } else if down {
            let step = at.map_or(0, |i| i.saturating_add(1) % actionable.len());
            actionable.get(step).copied()
        } else {
            let step = at.map_or(actionable.len().saturating_sub(1), |i| (i + actionable.len().saturating_sub(1)) % actionable.len());
            actionable.get(step).copied()
        };
        if let Some(next) = next {
            *nav = Some(MenuNav { items: items.len(), selected: next });
        }
    }
    if enter || space {
        // Excel opens the menu unhighlighted and Enter takes the first item.
        return (false, current.or(Some(first)));
    }
    (false, None)
}

/// One context-menu row: highlighted when the keyboard is on it, frameless otherwise.
fn menu_row(ui: &mut egui::Ui, label: &str, selected: bool, width: f32) -> egui::Response {
    let mut button = egui::Button::new(label).min_size(vec2(width, 20.0));
    if selected {
        button = button.fill(Tokens::get(ui.ctx()).hover);
    } else {
        button = button.frame_when_inactive(false);
    }
    ui.add(button)
}

/// Run the action a context-menu row names. Shared by mouse clicks and keyboard activation.
fn run_menu_item(app: &mut SheetApp, ui: &egui::Ui, id: &str) {
    match id {
        "ui:pasteSpecial" => app.open_dialog("pasteSpecial", json!({})),
        "ui:insertPaste" => app.open_dialog("insertPaste", json!({})),
        "ui:insertCells" => app.open_dialog("insertCells", json!({})),
        "ui:deleteCells" => app.open_dialog("deleteCells", json!({})),
        "ui:formatCells" => app.open_dialog("formatCells", json!({})),
        "ui:newComment" => app.open_dialog("comment", json!({"threaded": true})),
        "ui:newNote" => app.open_dialog("comment", json!({"threaded": false})),
        "ui:defineName" => app.open_dialog("defineName", json!({})),
        "ui:insertLink" => app.open_dialog("insertLink", json!({})),
        "ui:pickList" => app.open_dialog("pickList", json!({})),
        "ui:rowHeight" => app.open_dialog("rowHeight", json!({})),
        "ui:columnWidth" => app.open_dialog("columnWidth", json!({})),
        "ui:filterValue" => {
            if let Some(d) = app.session.active() {
                let a = d.selection.active;
                let text = d.wb.active().map(|sh| gridcraft_engine::display::cell_text(&d.wb, sh, a)).unwrap_or_default();
                let has = d.wb.active().is_some_and(|sh| sh.autofilter.is_some());
                if !has {
                    let _ = app.run("data.filter", json!({}));
                }
                app.run_or_alert("data.filterBy", json!({"column": col_to_letters(a.col), "values": [text]}));
            }
        }
        "edit.copy" | "edit.cut" => app.copy_to_clipboard(ui.ctx(), id),
        "edit.paste" => app.paste_from_clipboard(),
        other => app.run_or_alert(other, json!({})),
    }
}

fn context_menu(app: &mut SheetApp, ui: &mut egui::Ui) {
    let Some(at) = app.grid.context_menu else { return };
    let insert_copied = insert_copied_item(app);
    let mut items: Vec<(&str, &str)> = vec![
        ("Cut", "edit.cut"),
        ("Copy", "edit.copy"),
        ("Paste", "edit.paste"),
        ("Paste Special…", "ui:pasteSpecial"),
        ("-", ""),
        ("Insert…", "ui:insertCells"),
        ("Delete…", "ui:deleteCells"),
        ("Clear Contents", "edit.clearContents"),
        ("-", ""),
        ("Filter by Selected Cell's Value", "ui:filterValue"),
        ("Sort A to Z", "data.sortAscending"),
        ("Sort Z to A", "data.sortDescending"),
        ("-", ""),
        ("New Comment", "ui:newComment"),
        ("New Note", "ui:newNote"),
        ("-", ""),
        ("Format Cells…", "ui:formatCells"),
        ("Pick From Drop-down List…", "ui:pickList"),
        ("Define Name…", "ui:defineName"),
        ("Link…", "ui:insertLink"),
    ];
    if let Some(item) = insert_copied {
        items.insert(6, item);
    }
    let (keys_closed, activate) = menu_keys(ui, &items, &mut app.grid.menu_nav);
    let mut close = keys_closed;
    let mut run = None;
    let hover_moved = ui.input(|i| i.pointer.delta().length_sq() > 0.0);
    let resp = egui::Area::new(egui::Id::new("cell_context")).fixed_pos(at).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_min_width(200.0);
            let lang = crate::i18n::current(ui.ctx());
            for (index, (label, id)) in items.iter().enumerate() {
                if *label == "-" {
                    ui.separator();
                    continue;
                }
                let selected = app.grid.menu_nav.is_some_and(|n| n.items == items.len() && n.selected == index);
                let row = menu_row(ui, lang.tr(label), selected, 190.0);
                if hover_moved && row.hovered() {
                    app.grid.menu_nav = Some(MenuNav { items: items.len(), selected: index });
                }
                if row.clicked() || activate == Some(index) {
                    close = true;
                    run = Some(*id);
                }
            }
        });
    });
    if let Some(id) = run {
        run_menu_item(app, ui, id);
    }
    // A quick right-click presses and releases in one frame, and the menu's corner snaps to the
    // nearest physical pixel, which can put the pointer just outside it: that press opened the
    // menu, so it doesn't count as a click elsewhere.
    let opened_now = app.grid.context_menu_frame == ui.ctx().cumulative_frame_nr();
    if close
        || (!opened_now
            && ui.input(|i| i.pointer.any_pressed())
            && !resp.response.contains_pointer()
            && !resp.response.hovered()
            && ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| !resp.response.rect.contains(p)))
    {
        app.grid.context_menu = None;
        app.grid.menu_nav = None;
    }
}

/// The menu a right-drag of the fill handle opens on release, as in Excel.
fn fill_menu(app: &mut SheetApp, ui: &mut egui::Ui, was_open: bool) {
    let Some((src, target, at)) = app.grid.fill_menu else { return };
    let mut close = false;
    let response = egui::Area::new(egui::Id::new("fill_menu")).fixed_pos(at).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_min_width(190.0);
            let items = [
                ("Copy Cells", "copy"),
                ("Fill Series", "extend"),
                ("Fill Formatting Only", "formats"),
                ("Fill Without Formatting", "values"),
                ("-", ""),
                ("Series…", "ui:series"),
            ];
            let lang = crate::i18n::current(ui.ctx());
            for (label, mode) in items {
                if label == "-" {
                    ui.separator();
                    continue;
                }
                if ui.add(egui::Button::new(lang.tr(label)).frame(false).min_size(vec2(180.0, 20.0))).clicked() {
                    close = true;
                    if mode == "ui:series" {
                        let _ = app.session.run("selection.set", json!({"range": target.union(&src).a1(), "active": src.start.a1()}));
                        let direction = if target.width() != src.width() { "rows" } else { "columns" };
                        app.open_dialog("series", json!({"direction": direction}));
                    } else if let Err(e) = app.run("edit.autoFill", json!({"source": src.a1(), "target": target.a1(), "mode": mode})) {
                        app.message = Some(("GridCraft".into(), crate::clean_error(&e)));
                    }
                }
            }
        });
    });
    if close || popup_dismissed(&response.response, was_open) {
        app.grid.fill_menu = None;
    }
}

/// The right-click menu of a chart, picture, shape, slicer or timeline.
fn object_menu(app: &mut SheetApp, ui: &mut egui::Ui) {
    let Some((at, kind, id)) = app.grid.object_menu else { return };
    let mut close = false;
    let lang = crate::i18n::current(ui.ctx());
    let widget = matches!(kind, "pivotSlicer" | "timeline");
    let resp = egui::Area::new(egui::Id::new("object_context")).fixed_pos(at).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_min_width(200.0);
            // A PivotTable widget deletes through `slicer.delete` (it also clears the report's
            // filter); a table slicer and the drawing objects use `object.delete`.
            let mut items: Vec<(&str, &str)> = vec![("Delete", if widget { "slicer.delete" } else { "object.delete" })];
            if kind == "chart" {
                items.extend([("-", ""), ("Select Data…", "ui:selectData"), ("Format Chart Area…", "ui:formatChart")]);
            }
            if kind == "pivotSlicer" {
                items.extend([("-", ""), ("Select All", "slicer.selectAll"), ("Clear Filter", "slicer.clear")]);
            }
            for (label, action) in items {
                if label == "-" {
                    ui.separator();
                    continue;
                }
                if ui.add(egui::Button::new(lang.tr(label)).frame(false).min_size(vec2(190.0, 20.0))).clicked() {
                    close = true;
                    match action {
                        "ui:selectData" => app.open_dialog("selectData", json!({"chart": id})),
                        "ui:formatChart" => app.grid.pane = Some("formatChart".into()),
                        "slicer.selectAll" => {
                            app.run_or_alert("slicer.selectAll", json!({"slicer": id}));
                        }
                        "slicer.clear" => {
                            app.run_or_alert("slicer.clear", json!({"slicer": id}));
                        }
                        "slicer.delete" => {
                            app.run_or_alert("slicer.delete", json!({"slicer": id}));
                            app.selected_chart = None;
                        }
                        _ => {
                            app.run_or_alert("object.delete", json!({"kind": kind, "id": id}));
                            app.selected_chart = None;
                        }
                    }
                }
            }
            // Slicer styles are a submenu: the file's style names are the only choices (an
            // unknown name is refused by the command, not written into the file).
            if kind == "pivotSlicer" {
                ui.separator();
                ui.menu_button(lang.tr("Slicer Styles"), |ui| {
                    for style in gridcraft_engine::model::SLICER_STYLES {
                        if ui.button(*style).clicked() {
                            app.run_or_alert("slicer.setStyle", json!({"slicer": id, "style": style}));
                            close = true;
                        }
                    }
                });
            }
        });
    });
    // A click inside an open submenu (Slicer Styles) lands outside this area and must not close
    // the menu before the submenu's item handles it. The frame the menu opens is never a dismissal
    // either: its area is placed at the click point and rounds its top-left by a fraction of a
    // point, so the press that opened it can sit a hair outside and close it at once.
    let first_frame = {
        let key = egui::Id::new("gridcraft.object_menu_frame");
        let frame = ui.ctx().cumulative_frame_nr();
        let fresh = ui.data(|d| d.get_temp::<u64>(key)).is_none_or(|shown| shown != frame);
        ui.data_mut(|d| d.insert_temp(key, frame));
        fresh
    };
    let submenu_open = egui::Popup::is_any_open(ui.ctx());
    if close
        || ui.input(|i| i.key_pressed(egui::Key::Escape))
        || (!first_frame
            && !submenu_open
            && ui.input(|i| i.pointer.any_pressed())
            && !resp.response.contains_pointer()
            && !resp.response.hovered()
            && ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| !resp.response.rect.contains(p)))
    {
        app.grid.object_menu = None;
        ui.data_mut(|d| d.remove_temp::<u64>(egui::Id::new("gridcraft.object_menu_frame")));
    }
}

/// The warning button beside an active cell holding a number stored as text, as in Excel: its
/// menu names the problem and offers Convert to Number for the selection (#1994).
fn error_check_tag(app: &mut SheetApp, ui: &mut egui::Ui, geo: &Geo, sh: &Sheet) {
    if app.editor.is_some() || app.dialog.is_some() || sh.show_formulas || app.grid.drag != Drag::None {
        return;
    }
    let Some(active) = app.session.active().map(|d| d.selection.active) else { return };
    if sh.cell(active).as_ref().and_then(gridcraft_engine::cmd::formulas::number_stored_as_text).is_none() {
        return;
    }
    let cell = geo.cell_rect(sh, active);
    let size = vec2(18.0, 18.0);
    // Left of the cell, or right of it when that would cover the row headings.
    let left = if cell.left() - size.x - 2.0 >= geo.cells.left() { cell.left() - size.x - 2.0 } else { cell.right() + 2.0 };
    let tag = geo.mirror_rect(Rect::from_min_size(pos2(left, cell.top()), size));
    if !geo.cells.intersects(tag) {
        return;
    }
    let lang = crate::i18n::current(ui.ctx());
    let resp = ui.interact(tag, egui::Id::new("gridcraft.error_check_tag"), Sense::click()).on_hover_text(lang.tr("Number Stored as Text"));
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, lang.tr("Error checking options")));
    let t = Tokens::get(ui.ctx());
    let p = ui.painter();
    p.rect_filled(tag, 3.0, if resp.hovered() { t.hover } else { t.window });
    p.rect_stroke(tag, 3.0, Stroke::new(1.0, t.separator), egui::StrokeKind::Inside);
    // A warning sign: an amber diamond with an exclamation mark.
    let c = tag.center();
    let r = 6.0;
    p.add(egui::Shape::convex_polygon(
        vec![pos2(c.x, c.y - r), pos2(c.x + r, c.y), pos2(c.x, c.y + r), pos2(c.x - r, c.y)],
        t.warning_fill,
        Stroke::NONE,
    ));
    p.text(c, egui::Align2::CENTER_CENTER, "!", egui::FontId::proportional(10.0), t.warning_ink);
    egui::Popup::menu(&resp).show(|ui| {
        ui.label(egui::RichText::new(lang.tr("Number Stored as Text")).strong());
        ui.separator();
        if ui.button(lang.tr("Convert to Number")).clicked() {
            app.run_or_alert("formulas.convertToNumber", json!({}));
            ui.close();
        }
    });
}

/// Used by `EditState` users to keep the active cell visible while pointing.
pub fn point_move(app: &mut SheetApp, ed: &mut EditState, dr: i64, dc: i64, extend: bool) {
    let Some(d) = app.session.active() else { return };
    let from = ed.point_cell.unwrap_or(ed.cell);
    let to = from.offset_clamped(dr, dc);
    let anchor = if extend {
        // The anchor is the corner opposite the moving end: the written range is normalized, so
        // after extending up or left its first cell is the moving end, not the anchor.
        let pointed = ed.point.and_then(|(s, e)| ed.text.get(s..e)).and_then(RangeRef::parse);
        let inferred_anchor = pointed
            .map(|r| {
                CellRef::new(
                    if from.row == r.start.row { r.end.row } else { r.start.row },
                    if from.col == r.start.col { r.end.col } else { r.start.col },
                )
            })
            .unwrap_or(from);
        ed.point_anchor.filter(|(sheet, _)| *sheet == d.wb.active_sheet).map(|(_, cell)| cell).unwrap_or(inferred_anchor)
    } else {
        to
    };
    let text = if extend { RangeRef::new(anchor, to).a1() } else { to.a1() };
    let sheet = d.wb.active_sheet;
    ed.insert_ref(&text);
    ed.point_cell = Some(to);
    ed.point_anchor = Some((sheet, anchor));
    app.grid.ensure_visible = true;
}

/// The dropdown arrow beside a cell with an in-cell list validation.
pub fn validation_arrow(sh: &Sheet, geo: &Geo, c: CellRef) -> Option<Rect> {
    let dv = sh.validations.iter().find(|d| d.ranges.iter().any(|r| r.contains(c)))?;
    if dv.kind != gridcraft_engine::model::ValidationKind::List || !dv.in_cell_dropdown || geo.hidden_under_frozen(sh, c) {
        return None;
    }
    let r = geo.cell_rect(sh, c);
    let s = r.height().clamp(14.0, 22.0);
    Some(Rect::from_min_size(pos2(r.right() + 1.0, r.bottom() - s), vec2(s, s)))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_overlays(
    app: &mut SheetApp,
    ui: &egui::Ui,
    p: &Painter,
    geo: &Geo,
    wb: &Workbook,
    si: usize,
    sh: &Sheet,
    sel: &gridcraft_engine::Selection,
) {
    // Validation dropdown arrow for the active cell.
    if app.editor.is_none()
        && let Some(r) = validation_arrow(sh, geo, sel.active)
    {
        p.rect_filled(r, 2.0, Color32::from_gray(245));
        p.rect_stroke(r, 2.0, Stroke::new(1.0, Color32::from_gray(160)), StrokeKind::Inside);
        crate::icons::paint(p, r.shrink(3.0), crate::icons::Icon::Chevron, Color32::from_gray(60));
    }
    // Input message of the active cell's validation, hidden while its list is open: it would cover the items.
    if app.editor.is_none()
        && app.grid.list_picker != Some(sel.active)
        && let Some(dv) = sh.validations.iter().find(|d| d.ranges.iter().any(|r| r.contains(sel.active)))
        && dv.show_input
        && !(dv.input_title.is_empty() && dv.input_message.is_empty())
        && !geo.hidden_under_frozen(sh, sel.active)
    {
        let r = geo.cell_rect(sh, sel.active);
        let (at, pivot) = (geo.mirror_pos(pos2(r.left() + 8.0, r.bottom() + 6.0)), if geo.rtl { Align2::RIGHT_TOP } else { Align2::LEFT_TOP });
        egui::Area::new(egui::Id::new("dv_input")).fixed_pos(at).pivot(pivot).order(egui::Order::Tooltip).interactable(false).show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).fill(Color32::from_rgb(0xFF, 0xFF, 0xE1)).show(ui, |ui| {
                ui.set_max_width(220.0);
                if !dv.input_title.is_empty() {
                    ui.label(egui::RichText::new(&dv.input_title).strong().color(Color32::BLACK));
                }
                ui.label(egui::RichText::new(&dv.input_message).color(Color32::BLACK));
            });
        });
    }
    // Notes: shown on hover (or always when marked visible).
    let hover = app.grid.hover_cell;
    let all_notes = app.session.show_all_notes;
    for (c, cm) in &sh.comments {
        let show = cm.visible || (all_notes && !cm.threaded) || hover == Some(*c);
        if !show || app.grid.drag != Drag::None {
            continue;
        }
        let r = geo.cell_rect(sh, *c);
        if !p.clip_rect().intersects(r) || geo.hidden_under_frozen(sh, *c) {
            continue;
        }
        let at = pos2(r.right() + 10.0, r.top());
        p.line_segment([r.right_top(), at + vec2(0.0, 6.0)], Stroke::new(1.0, Color32::from_gray(90)));
        let (at, pivot) = (geo.mirror_pos(at), if geo.rtl { Align2::RIGHT_TOP } else { Align2::LEFT_TOP });
        egui::Area::new(egui::Id::new(("note", c.row, c.col))).fixed_pos(at).pivot(pivot).order(egui::Order::Tooltip).interactable(false).show(
            ui.ctx(),
            |ui| {
                egui::Frame::popup(ui.style()).fill(if cm.threaded { Color32::WHITE } else { Color32::from_rgb(0xFF, 0xFF, 0xE1) }).show(ui, |ui| {
                    ui.set_max_width(240.0);
                    ui.label(egui::RichText::new(format!("{}:", cm.author)).strong().color(Color32::BLACK));
                    ui.label(egui::RichText::new(&cm.text).color(Color32::BLACK));
                    for (a, t) in &cm.replies {
                        ui.separator();
                        ui.label(egui::RichText::new(format!("{a}: {t}")).color(Color32::BLACK));
                    }
                });
            },
        );
    }
    // Hyperlink tooltip.
    if let Some(h) = hover.and_then(|c| sh.hyperlinks.get(&c))
        && app.editor.is_none()
    {
        let tip = h.tooltip.clone().unwrap_or_else(|| format!("{}\nClick once to follow. Click and hold to select this cell.", h.target));
        egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), egui::Id::new("link_tip"), egui::PopupAnchor::Pointer).show(|ui| {
            ui.label(tip);
        });
    }
    // Trace precedents / dependents arrows.
    if let Some(trace) = app.grid.trace.clone()
        && let Some(items) = trace.as_array()
    {
        let blue = Color32::from_rgb(0x1F, 0x5F, 0xC9);
        let active = geo.cell_rect(sh, sel.active).center();
        for it in items {
            if it["sheet"].as_str().is_some_and(|s| !s.eq_ignore_ascii_case(&sh.name)) {
                continue;
            }
            let target = it["range"].as_str().or(it["cell"].as_str()).and_then(RangeRef::parse);
            let Some(t) = target else { continue };
            let tr = geo.range_rect(sh, t);
            if !t.is_single() {
                for pane in geo.range_panes(t) {
                    p.with_clip_rect(pane.intersect(p.clip_rect())).rect_stroke(tr, 0.0, Stroke::new(1.5, blue), StrokeKind::Inside);
                }
            }
            let (from, to) = if it.get("range").is_some() { (tr.center(), active) } else { (active, tr.center()) };
            p.line_segment([from, to], Stroke::new(1.5, blue));
            p.circle_filled(from, 3.0, blue);
            let d = (to - from).normalized();
            let n = vec2(-d.y, d.x);
            p.add(egui::Shape::convex_polygon(vec![to, to - d * 9.0 + n * 4.0, to - d * 9.0 - n * 4.0], blue, Stroke::NONE));
        }
    }
    let _ = (wb, si);
}

/// Rows the list-validation dropdown shows before it scrolls (Excel shows 8).
const LIST_PICKER_MAX_ROWS: usize = 8;
const LIST_PICKER_ROW_H: f32 = 18.0;
/// Popup frame around the rows: 2 px inner margin and the window stroke on each side, rounded up.
const LIST_PICKER_CHROME: f32 = 8.0;

/// Popup height for `n` items: every item up to `LIST_PICKER_MAX_ROWS`, then a scrollbar.
fn list_picker_height(n: usize, row_h: f32, gap: f32) -> f32 {
    let rows = n.clamp(1, LIST_PICKER_MAX_ROWS) as f32;
    rows * row_h + (rows - 1.0) * gap
}

fn list_picker(app: &mut SheetApp, ui: &mut egui::Ui, geo: &Geo, was_open: bool) {
    let Some(c) = app.grid.list_picker else { return };
    let Some(d) = app.session.active() else { return };
    let wb = d.wb.clone();
    let si = wb.active_sheet;
    let Some(sh) = wb.sheet(si) else { return };
    let Some(dv) = sh.validations.iter().find(|x| x.ranges.iter().any(|r| r.contains(c))).cloned() else {
        app.grid.list_picker = None;
        return;
    };
    let items = gridcraft_engine::cmd::data::list_items(&wb, si, &dv);
    let r = geo.cell_rect(sh, c);
    // Fit the popup to its rows (up to LIST_PICKER_MAX_ROWS); without a minimum the scroll area collapses to its 64 px default.
    let height = list_picker_height(items.len(), LIST_PICKER_ROW_H, ui.spacing().item_spacing.y);
    // Open below the cell, or above it when the rows would run past the bottom of the window (as Excel does).
    let screen = ui.ctx().content_rect();
    let above = r.bottom() + height + LIST_PICKER_CHROME > screen.bottom() && r.top() - height - LIST_PICKER_CHROME >= screen.top();
    let (at, pivot) = match (above, geo.rtl) {
        (false, false) => (geo.mirror_pos(r.left_bottom()), Align2::LEFT_TOP),
        (false, true) => (geo.mirror_pos(r.left_bottom()), Align2::RIGHT_TOP),
        (true, false) => (geo.mirror_pos(r.left_top()), Align2::LEFT_BOTTOM),
        (true, true) => (geo.mirror_pos(r.left_top()), Align2::RIGHT_BOTTOM),
    };
    let mut close = false;
    let response = egui::Area::new(egui::Id::new("dv_list")).fixed_pos(at).pivot(pivot).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).inner_margin(2.0).show(ui, |ui| {
            ui.set_min_width(r.width().max(100.0));
            egui::ScrollArea::vertical().max_height(height).min_scrolled_height(height).show(ui, |ui| {
                for it in &items {
                    if ui
                        .add(egui::Button::new(it.as_str()).frame_when_inactive(false).min_size(vec2(r.width().max(100.0), LIST_PICKER_ROW_H)))
                        .clicked()
                    {
                        app.run_or_alert("cell.set", json!({"cell": c.a1(), "input": it}));
                        close = true;
                    }
                }
            });
        });
    });
    if close || popup_dismissed(&response.response, was_open) {
        app.grid.list_picker = None;
    }
}

fn header_menu(app: &mut SheetApp, ui: &mut egui::Ui, was_open: bool) {
    let Some((at, rows)) = app.grid.header_menu else { return };
    let insert_copied = insert_copied_item(app);
    let mut items: Vec<(&str, &str)> = if rows {
        vec![
            ("Cut", "edit.cut"),
            ("Copy", "edit.copy"),
            ("Paste", "edit.paste"),
            ("-", ""),
            ("Insert", "home.insertRows"),
            ("Delete", "home.deleteRows"),
            ("Clear Contents", "edit.clearContents"),
            ("-", ""),
            ("Format Cells…", "ui:formatCells"),
            ("Row Height…", "ui:rowHeight"),
            ("AutoFit Row Height", "home.autofitRowHeight"),
            ("Hide", "home.hideRows"),
            ("Unhide", "home.unhideRows"),
            ("Group", "data.group"),
        ]
    } else {
        vec![
            ("Cut", "edit.cut"),
            ("Copy", "edit.copy"),
            ("Paste", "edit.paste"),
            ("-", ""),
            ("Insert", "home.insertColumns"),
            ("Delete", "home.deleteColumns"),
            ("Clear Contents", "edit.clearContents"),
            ("-", ""),
            ("Format Cells…", "ui:formatCells"),
            ("Column Width…", "ui:columnWidth"),
            ("AutoFit Column Width", "home.autofitColumnWidth"),
            ("Hide", "home.hideColumns"),
            ("Unhide", "home.unhideColumns"),
            ("Sort A to Z", "data.sortAscending"),
            ("Group", "data.group"),
        ]
    };
    if let Some(item) = insert_copied {
        items.insert(5, item);
    }
    let (keys_closed, activate) = menu_keys(ui, &items, &mut app.grid.menu_nav);
    let mut close = keys_closed;
    let mut run = None;
    let hover_moved = ui.input(|i| i.pointer.delta().length_sq() > 0.0);
    let response = egui::Area::new(egui::Id::new("header_menu")).fixed_pos(at).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_min_width(190.0);
            let lang = crate::i18n::current(ui.ctx());
            for (index, (label, id)) in items.iter().enumerate() {
                if *label == "-" {
                    ui.separator();
                    continue;
                }
                let selected = app.grid.menu_nav.is_some_and(|n| n.items == items.len() && n.selected == index);
                let row = menu_row(ui, lang.tr(label), selected, 180.0);
                if hover_moved && row.hovered() {
                    app.grid.menu_nav = Some(MenuNav { items: items.len(), selected: index });
                }
                if row.clicked() || activate == Some(index) {
                    close = true;
                    run = Some(*id);
                }
            }
        });
    });
    if let Some(id) = run {
        run_menu_item(app, ui, id);
    }
    if close || popup_dismissed(&response.response, was_open) {
        app.grid.header_menu = None;
        app.grid.menu_nav = None;
    }
}

/// The context-menu entry for Insert Copied Cells / Insert Cut Cells, when the clipboard holds a
/// copy or cut of this app: whole rows or columns insert at once, a block asks which way to shift
/// (Insert Paste). A cut block can only be pasted, so it gets no entry.
fn insert_copied_item(app: &SheetApp) -> Option<(&'static str, &'static str)> {
    let clip = app.session.clipboard.as_ref()?;
    let lines = clip.selected.is_full_rows() != clip.selected.is_full_cols();
    let cut = clip.cut && app.session.active().is_some_and(|d| d.uid == clip.doc_uid);
    match (cut, lines) {
        (true, true) => Some(("Insert Cut Cells", "home.insertCopiedCells")),
        (true, false) => None,
        (false, true) => Some(("Insert Copied Cells", "home.insertCopiedCells")),
        (false, false) => Some(("Insert Copied Cells…", "ui:insertPaste")),
    }
}

fn is_checkbox(sh: &Sheet, app: &SheetApp) -> bool {
    let Some(d) = app.session.active() else { return false };
    d.wb.styles.get(sh.style_id(d.selection.active)).num_fmt.as_str() == "checkbox"
}

fn paint_pages(app: &mut SheetApp, p: &Painter, geo: &Geo, sh: &Sheet) {
    let Some(d) = app.session.active() else { return };
    let key = (d.uid, d.revision, d.wb.active_sheet);
    if app.grid.pages.as_ref().is_none_or(|(k, _)| *k != key) {
        let pages: Vec<(RangeRef, u32)> = app
            .session
            .run("file.printPreview", json!({}))
            .ok()
            .and_then(|v| {
                v["pageList"].as_array().map(|a| {
                    a.iter()
                        .filter(|pg| pg["sheet"].as_str() == Some(sh.name.as_str()))
                        .filter_map(|pg| Some((RangeRef::parse(pg["range"].as_str()?)?, pg["page"].as_u64()? as u32)))
                        .collect()
                })
            })
            .unwrap_or_default();
        app.grid.pages = Some((key, pages));
    }
    let Some((_, pages)) = &app.grid.pages else { return };
    if pages.is_empty() {
        return;
    }
    // Grey everything, then clear the printed pages.
    let mut printed = pages[0].0;
    for (r, _) in pages {
        printed = printed.union(r);
    }
    let pr = geo.range_rect(sh, printed);
    let grey = Color32::from_black_alpha(70);
    let c = p.clip_rect();
    for r in [
        Rect::from_min_max(c.min, pos2(c.right(), pr.top())),
        Rect::from_min_max(pos2(c.left(), pr.bottom()), c.max),
        Rect::from_min_max(pos2(c.left(), pr.top()), pos2(pr.left(), pr.bottom())),
        Rect::from_min_max(pos2(pr.right(), pr.top()), pos2(c.right(), pr.bottom())),
    ] {
        if r.is_positive() {
            p.rect_filled(r, 0.0, grey);
        }
    }
    let blue = Color32::from_rgb(0x2E, 0x6F, 0xD8);
    for (r, n) in pages {
        let rr = geo.range_rect(sh, *r);
        if !rr.intersects(c) {
            continue;
        }
        p.extend(egui::Shape::dashed_line(
            &[rr.left_top(), rr.right_top(), rr.right_bottom(), rr.left_bottom(), rr.left_top()],
            Stroke::new(2.0, blue),
            6.0,
            3.0,
        ));
        p.text(
            rr.center(),
            Align2::CENTER_CENTER,
            format!("Page {n}"),
            theme::ui_bold((rr.height().min(rr.width()) / 6.0).clamp(14.0, 72.0)),
            Color32::from_black_alpha(45),
        );
    }
    p.rect_stroke(pr, 0.0, Stroke::new(3.0, blue), StrokeKind::Outside);
}

#[cfg(test)]
mod selection_fill_tests {
    use super::*;

    #[test]
    fn rect_around_leaves_the_hole_and_never_overlaps() {
        let area = Rect::from_min_max(pos2(100.0, 50.0), pos2(400.0, 250.0));
        let check = |hole: Rect, pieces: usize| {
            let parts: Vec<Rect> = rect_around(area, hole).collect();
            assert_eq!(parts.len(), pieces, "{hole:?}: {parts:?}");
            let cut = area.intersect(hole);
            let cut_area = if cut.is_positive() { cut.area() } else { 0.0 };
            let shaded: f32 = parts.iter().map(Rect::area).sum();
            assert!((shaded - (area.area() - cut_area)).abs() < 0.5, "everything but the hole is shaded: {hole:?}");
            for (i, a) in parts.iter().enumerate() {
                assert!(area.contains_rect(*a), "nothing outside the area: {a:?}");
                assert!(!a.shrink(0.01).intersects(hole), "the hole stays clear: {a:?} vs {hole:?}");
                // The fill is translucent: a part shaded twice would show darker.
                assert!(parts.iter().skip(i + 1).all(|b| !a.shrink(0.01).intersects(*b)), "{parts:?}");
            }
        };
        let cell = |x: f32, y: f32| Rect::from_min_size(pos2(x, y), vec2(64.0, 16.0));
        // In the middle, in each corner (where a drag began), along an edge.
        check(cell(200.0, 100.0), 4);
        check(cell(100.0, 50.0), 2);
        check(cell(336.0, 50.0), 2);
        check(cell(100.0, 234.0), 2);
        check(cell(336.0, 234.0), 2);
        check(cell(100.0, 100.0), 3);
        // A merged cell as wide, or as tall, as the area; one that is all of it, or more.
        check(Rect::from_min_max(pos2(100.0, 100.0), pos2(400.0, 132.0)), 2);
        check(Rect::from_min_max(pos2(200.0, 50.0), pos2(264.0, 250.0)), 2);
        check(area, 0);
        check(area.expand(30.0), 0);
        // The active cell is in another area, touches this one, or is only partly inside it.
        check(cell(500.0, 300.0), 1);
        check(cell(400.0, 50.0), 1);
        check(cell(380.0, 240.0), 2);
    }

    #[test]
    fn rect_around_survives_hostile_numbers() {
        let sane = Rect::from_min_max(pos2(0.0, 0.0), pos2(64.0, 16.0));
        let bad = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MAX, f32::MIN, -1.0, 0.0];
        for a in bad {
            for b in bad {
                let weird = Rect::from_min_max(pos2(a, b), pos2(b, a));
                for (area, hole) in [(sane, weird), (weird, sane), (weird, weird)] {
                    for part in rect_around(area, hole) {
                        assert!(part.is_positive() && part.is_finite(), "{area:?} around {hole:?}: {part:?}");
                    }
                }
                // A hole that makes no sense leaves the whole area shaded.
                if !weird.intersects(sane) {
                    assert_eq!(rect_around(sane, weird).collect::<Vec<_>>(), vec![sane]);
                }
            }
        }
    }
}

/// The horizontal alignment to paint with. Cells are painted left to right and a right-to-left
/// sheet is mirrored afterwards, so an explicit Left or Right is swapped first to end up on the
/// side the user chose: Align Right stays on the right in either sheet direction, as in Excel.
/// General alignment is resolved from the value before this and follows the sheet direction.
fn painted_halign(h: HAlign, rtl: bool) -> HAlign {
    match h {
        HAlign::Left if rtl => HAlign::Right,
        HAlign::Right if rtl => HAlign::Left,
        h => h,
    }
}

/// Row and column heading labels grow with the zoom as the cells do, as in Excel (at 400% the
/// labels are four times their 100% size), and never get smaller than a readable 8.5 points.
pub(crate) fn heading_font_size(zoom: f32) -> f32 {
    (11.5 * (zoom / DISPLAY_SCALE)).max(8.5)
}

#[cfg(test)]
mod tests {

    #[test]
    fn explicit_left_and_right_keep_their_side_on_a_right_to_left_sheet() {
        use super::{HAlign, painted_halign};
        // Painted before the mirror: swapped, so they land where the user put them.
        assert_eq!(painted_halign(HAlign::Right, true), HAlign::Left);
        assert_eq!(painted_halign(HAlign::Left, true), HAlign::Right);
        // Left-to-right sheets and the other alignments are painted as they are.
        assert_eq!(painted_halign(HAlign::Right, false), HAlign::Right);
        assert_eq!(painted_halign(HAlign::Left, false), HAlign::Left);
        for h in [HAlign::Center, HAlign::Justify, HAlign::Fill] {
            assert_eq!(painted_halign(h, true), h);
        }
    }

    #[test]
    fn heading_labels_scale_with_the_zoom_without_a_cap() {
        use super::{DISPLAY_SCALE, heading_font_size};
        let at = |percent: f32| heading_font_size(percent / 100.0 * DISPLAY_SCALE);
        assert!((at(400.0) / at(100.0) - 4.0).abs() < 1e-4, "four times as large at 400%");
        assert!(at(200.0) > at(100.0) && at(400.0) > at(200.0));
        assert_eq!(at(10.0), 8.5, "never below a readable size");
    }
    use super::*;

    #[test]
    fn list_picker_fits_up_to_eight_rows_then_scrolls() {
        assert_eq!(list_picker_height(5, 18.0, 3.0), 5.0 * 18.0 + 4.0 * 3.0);
        assert_eq!(list_picker_height(8, 18.0, 3.0), 8.0 * 18.0 + 7.0 * 3.0);
        assert_eq!(list_picker_height(30, 18.0, 3.0), list_picker_height(8, 18.0, 3.0));
        assert_eq!(list_picker_height(0, 18.0, 3.0), 18.0);
    }

    #[test]
    fn number_format_colors_resolve_named_and_indexed_rgb() {
        use gridcraft_engine::numfmt::FormatColor as F;
        assert_eq!(numfmt_color(F::Green), Color32::from_rgb(0, 255, 0));
        assert_eq!(numfmt_color(F::Indexed(1)), Color32::BLACK);
        assert_eq!(numfmt_color(F::Indexed(10)), Color32::from_rgb(0, 128, 0));
        assert_eq!(numfmt_color(F::Indexed(46)), Color32::from_rgb(255, 102, 0));
        assert_eq!(numfmt_color(F::Indexed(56)), Color32::from_rgb(51, 51, 51));
        for index in [0, 57, 255] {
            assert_eq!(numfmt_color(F::Indexed(index)), Color32::BLACK);
        }
    }

    #[test]
    fn wheel_zoom_steps_along_the_ladder_and_stops_at_100() {
        for (from, zoom_in, to) in [
            (100, true, 120),
            (120, false, 100),
            (100, false, 85),
            // Between steps, the next one each way (Excel: 112% goes to 115% or 100%).
            (112, true, 120),
            (112, false, 100),
            // A step less than 5% away is skipped…
            (121, false, 100),
            (86, false, 70),
            (139, true, 170),
            // …but never 100% or a limit.
            (99, true, 100),
            (101, false, 100),
            (96, true, 100),
            (385, true, 400),
            (11, false, 10),
            (400, true, 400),
            (10, false, 10),
            // Out-of-range zooms (from a file) come into range first.
            (5, true, 12),
            (1000, false, 340),
            (0, false, 10),
            (u16::MAX, true, 400),
        ] {
            assert_eq!(wheel_zoom_step(from, zoom_in), to, "{from}% {}", if zoom_in { "in" } else { "out" });
        }
        // Every step on the ladder is 15–25% from the next, and a notch in then out returns.
        for w in WHEEL_ZOOM_LADDER.windows(2) {
            let ratio = f32::from(w[1]) / f32::from(w[0]);
            assert!((1.15..=1.25).contains(&ratio), "{w:?}");
            assert_eq!(wheel_zoom_step(w[0], true), w[1]);
            assert_eq!(wheel_zoom_step(w[1], false), w[0]);
        }
    }

    #[test]
    fn center_across_run_stops_at_first_non_continuing_column() {
        assert_eq!(center_across_end(0, 20, |c| c <= 2), 2);
        assert_eq!(center_across_end(0, 20, |_| false), 0);
        assert_eq!(center_across_end(0, 1, |_| true), 1);
    }

    #[test]
    fn conditional_number_format_reaches_the_cell_text() {
        // #1163: a rule's number format replaces the cell's own; unmatched cells keep theirs.
        let mut session = gridcraft_engine::Session::new();
        session.new_workbook();
        session.execute("range.setValues", json!({"range": "A1", "values": [[1], [3], [4]]})).unwrap();
        session.execute("home.numberFormat", json!({"range": "A3", "code": "0.0"})).unwrap();
        let rule = json!({"type": "cellIs", "operator": "greater", "value": "2", "style": {"num_fmt": "0.00"}});
        session.execute("home.conditionalFormat", json!({"range": "A1:A3", "rule": rule})).unwrap();
        let wb = session.active().unwrap().wb.clone();
        let sh = wb.sheet(0).unwrap();
        let mut cf = CfCache::new();
        let text = |row: u32, cf: &mut CfCache| {
            let c = CellRef::new(row, 0);
            let (lk, _) = look(&wb, 0, sh, c, cf);
            display_text(&wb, sh, c, &sh.value(c), &lk.style, 200.0, 7.0).0
        };
        assert_eq!(text(0, &mut cf), "1");
        assert_eq!(text(1, &mut cf), "3.00");
        assert_eq!(text(2, &mut cf), "4.00");
    }

    #[test]
    fn filter_menu_uses_the_table_whose_header_was_clicked() {
        use gridcraft_engine::model::Table;
        let table = |id: u32, r0: u32, r1: u32| Table {
            id,
            name: format!("T{id}"),
            range: RangeRef::new(CellRef::new(r0, 0), CellRef::new(r1, 0)),
            header_row: true,
            totals_row: false,
            columns: vec![],
            style: String::new(),
            banded_rows: true,
            banded_cols: false,
            first_col: false,
            last_col: false,
            filter_button: true,
            filter: vec![],
            table_type: None,
            connection_id: None,
        };
        let mut sh = Sheet::new("S");
        sh.tables.push(table(1, 0, 2));
        sh.tables.push(table(2, 4, 7));
        assert_eq!(filter_table_range(&sh, 0, 4), Some(sh.tables[1].range));
        assert_eq!(filter_table_range(&sh, 0, 0), Some(sh.tables[0].range));
        assert_eq!(filter_table_range(&sh, 1, 4), None);
    }

    #[test]
    fn cell_text_y_moves_monotonically_in_standard_and_tall_rows() {
        let rect_std = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 20.0));
        let gh = 18.5;
        let z = 1.0;

        let y_top_std = cell_text_y(VAlign::Top, rect_std, gh, z);
        let y_center_std = cell_text_y(VAlign::Center, rect_std, gh, z);
        let y_bottom_std = cell_text_y(VAlign::Bottom, rect_std, gh, z);

        assert!(y_top_std < y_center_std, "Top ({y_top_std}) must be higher than Center ({y_center_std})");
        assert!(y_center_std < y_bottom_std, "Center ({y_center_std}) must be higher than Bottom ({y_bottom_std})");

        // Verify tall row (e.g. 40px)
        let rect_tall = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 40.0));
        let y_top_tall = cell_text_y(VAlign::Top, rect_tall, gh, z);
        let y_center_tall = cell_text_y(VAlign::Center, rect_tall, gh, z);
        let y_bottom_tall = cell_text_y(VAlign::Bottom, rect_tall, gh, z);

        assert!(y_top_tall < y_center_tall, "Top ({y_top_tall}) must be higher than Center ({y_center_tall})");
        assert!(y_center_tall < y_bottom_tall, "Center ({y_center_tall}) must be higher than Bottom ({y_bottom_tall})");
    }

    #[test]
    fn wrapped_cell_text_y_positions_correctly_and_safeguards_overflow() {
        let rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 40.0));
        let gh = 30.0;
        let z = 1.0;

        let y_top = wrapped_cell_text_y(VAlign::Top, rect, gh, z);
        let y_center = wrapped_cell_text_y(VAlign::Center, rect, gh, z);
        let y_bottom = wrapped_cell_text_y(VAlign::Bottom, rect, gh, z);

        assert!(y_top < y_center);
        assert!(y_center < y_bottom);

        // Overflow case: gh exceeds rect height; Top still starts inside the cell.
        let gh_overflow = 60.0;
        let y_top_ovf = wrapped_cell_text_y(VAlign::Top, rect, gh_overflow, z);
        assert!(y_top_ovf >= rect.top());
    }

    #[test]
    fn wrapped_cell_text_y_keeps_vertical_alignment_when_text_is_taller_than_the_row() {
        // A default-height row holding three wrapped lines.
        let rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 20.0));
        let (gh, z) = (60.0, 1.0);
        let y_top = wrapped_cell_text_y(VAlign::Top, rect, gh, z);
        let y_center = wrapped_cell_text_y(VAlign::Center, rect, gh, z);
        let y_bottom = wrapped_cell_text_y(VAlign::Bottom, rect, gh, z);
        // Taller text overflows upward for Bottom and both ways for Center (the cell clips it).
        assert!((y_top - 1.5).abs() < 0.01, "Top starts at the top edge, got {y_top}");
        assert!((y_center + gh / 2.0 - rect.center().y).abs() < 0.01, "Center is centred, got {y_center}");
        assert!((y_bottom + gh - (rect.bottom() - 1.5)).abs() < 0.01, "Bottom ends at the bottom edge, got {y_bottom}");
    }

    /// The screen box of a galley of `size` drawn at `pos` turned by `angle` about `pos`, as egui
    /// draws a rotated `TextShape`.
    fn rotated_box(pos: Pos2, size: egui::Vec2, angle: f32) -> Rect {
        let rot = egui::emath::Rot2::from_angle(angle);
        let pts = [vec2(0.0, 0.0), vec2(size.x, 0.0), vec2(0.0, size.y), size].map(|v| pos + rot * v);
        Rect::from_points(&pts)
    }

    #[test]
    fn rotated_text_box_follows_the_cell_alignment() {
        // A tall, narrow merged block (one 3-character column over four 60pt rows).
        let rect = Rect::from_min_max(pos2(10.0, 20.0), pos2(40.0, 340.0));
        let size = vec2(90.0, 15.0);
        let pad = 2.5;
        let near = |a: f32, b: f32| (a - b).abs() < 0.01;
        for deg in [90i16, -90, 45, -45] {
            let angle = -(deg as f32).to_radians();
            for v in [VAlign::Top, VAlign::Center, VAlign::Bottom] {
                for h in [HAlign::Left, HAlign::Center, HAlign::Right] {
                    let bb = rotated_box(rotated_text_pos(rect, size, angle, h, v, pad, 0.0), size, angle);
                    let what = format!("{deg}° {h:?}/{v:?}: {bb:?} in {rect:?}");
                    assert!(near(bb.height(), rotated_box(Pos2::ZERO, size, angle).height()), "{what}");
                    assert!(bb.top() >= rect.top() && bb.bottom() <= rect.bottom(), "{what}");
                    // A narrow column can't hold a 45° line across: only check where it fits.
                    let fits_x = bb.width() <= rect.width() - 2.0 * pad;
                    match h {
                        HAlign::Left => assert!(near(bb.left(), rect.left() + pad), "{what}"),
                        HAlign::Right => assert!(near(bb.right(), rect.right() - pad), "{what}"),
                        _ => assert!(near(bb.center().x, rect.center().x), "{what}"),
                    }
                    if fits_x {
                        assert!(bb.left() >= rect.left() && bb.right() <= rect.right(), "{what}");
                    }
                    match v {
                        VAlign::Top => assert!(near(bb.top(), rect.top() + pad), "{what}"),
                        VAlign::Center => assert!(near(bb.center().y, rect.center().y), "{what}"),
                        // Bottom is the default: downward text keeps starting at the top, as before.
                        _ if deg < 0 => assert!(near(bb.top(), rect.top() + pad), "{what}"),
                        _ => assert!(near(bb.bottom(), rect.bottom() - pad), "{what}"),
                    }
                }
            }
        }
    }

    #[test]
    fn upright_text_with_default_alignment_keeps_its_place() {
        let rect = Rect::from_min_max(pos2(10.0, 20.0), pos2(40.0, 340.0));
        let size = vec2(90.0, 15.0);
        let pad = 2.5;
        let near = |a: Pos2, b: Pos2| (a - b).length() < 0.01;
        // Upward text starts at the bottom-left corner, downward text one block-height in from the top-left.
        let up = rotated_text_pos(rect, size, -std::f32::consts::FRAC_PI_2, HAlign::General, VAlign::Bottom, pad, 0.0);
        assert!(near(up, pos2(rect.left() + pad, rect.bottom() - pad)), "{up:?}");
        let down = rotated_text_pos(rect, size, std::f32::consts::FRAC_PI_2, HAlign::General, VAlign::Bottom, pad, 0.0);
        assert!(near(down, pos2(rect.left() + pad + size.y, rect.top() + pad)), "{down:?}");
    }

    #[test]
    fn upward_text_with_middle_align_is_centred_in_a_merged_block() {
        // Issue #435: "Licht Strom (?)" turned up in A1:A4 with Middle Align was drawn at the bottom.
        let rect = Rect::from_min_max(pos2(10.0, 20.0), pos2(40.0, 340.0));
        let size = vec2(90.0, 15.0);
        let angle = -std::f32::consts::FRAC_PI_2;
        let bb = rotated_box(rotated_text_pos(rect, size, angle, HAlign::Center, VAlign::Center, 2.5, 0.0), size, angle);
        assert!((bb.center().y - rect.center().y).abs() < 0.01, "{bb:?} not centred in {rect:?}");
        assert!((bb.center().x - rect.center().x).abs() < 0.01, "{bb:?} not centred in {rect:?}");
    }
}

#[cfg(test)]
mod pane_tests {
    use super::*;

    fn rng(r0: u32, c0: u32, r1: u32, c1: u32) -> RangeRef {
        RangeRef::new(CellRef::new(r0, c0), CellRef::new(r1, c1))
    }

    #[test]
    fn selection_in_scrolling_pane_is_not_drawn_in_frozen_pane() {
        // Frozen: 2 rows and 2 columns; the scrolling pane shows rows 2..=20 and columns 2..=10.
        let frozen_cols = ((2, 20), (0, 1));
        let frozen_rows = ((0, 1), (2, 10));
        let corner = ((0, 1), (0, 1));
        let scrolled = ((2, 20), (2, 10));
        let sel = rng(5, 4, 7, 6);
        assert!(range_in_pane(sel, scrolled.0, scrolled.1));
        assert!(!range_in_pane(sel, frozen_cols.0, frozen_cols.1));
        assert!(!range_in_pane(sel, frozen_rows.0, frozen_rows.1));
        assert!(!range_in_pane(sel, corner.0, corner.1));
    }

    #[test]
    fn ctrl_click_on_a_selected_header_deselects_it() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(toggled_ranges(s(&["A:A", "C:C"]), "C:C".into(), true), s(&["A:A"]));
        assert_eq!(toggled_ranges(s(&["A:A"]), "C:C".into(), true), s(&["A:A", "C:C"]));
        assert_eq!(toggled_ranges(s(&["A:A"]), "A:A".into(), true), s(&["A:A"]));
        assert_eq!(toggled_ranges(s(&["A1", "B2", "3:3"]), "B2".into(), true), s(&["A1", "3:3"]));
        assert_eq!(toggled_ranges(s(&["A:A", "C:C"]), "C:C".into(), false), s(&["C:C"]));
    }

    #[test]
    fn selection_across_the_freeze_line_is_drawn_in_both_panes() {
        let sel = rng(5, 0, 7, 6);
        assert!(range_in_pane(sel, (2, 20), (0, 1)));
        assert!(range_in_pane(sel, (2, 20), (2, 10)));
    }

    /// Two frozen rows and columns, scrolled right so that columns C:E lie under the frozen columns.
    fn frozen_geo(sh: &mut Sheet) -> Geo {
        sh.freeze = Some((2, 2));
        let scroll = vec2(3.0 * sh.default_col_width + 8.0, 0.0);
        Geo::new(sh, Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0)), scroll)
    }

    #[test]
    fn the_edge_just_past_a_hidden_line_unhides_it() {
        let mut sh = Sheet::new("S");
        sh.cols.entry(11).or_default().hidden = true; // L
        sh.rows.entry(26).or_default().hidden = true; // 27
        let geo = Geo::new(&sh, Rect::from_min_size(pos2(0.0, 0.0), vec2(2000.0, 1200.0)), vec2(0.0, 0.0));
        let x = geo.x(&sh, 12); // between K and M
        assert_eq!(hidden_col_edge(&geo, &sh, x + 2.0), Some(11));
        assert_eq!(hidden_col_edge(&geo, &sh, x - 2.0), None); // K's own edge
        assert_eq!(hidden_col_edge(&geo, &sh, geo.x(&sh, 5)), None);
        let y = geo.y(&sh, 27); // between 26 and 28
        assert_eq!(hidden_row_edge(&geo, &sh, y + 2.0), Some(26));
        assert_eq!(hidden_row_edge(&geo, &sh, y - 2.0), None);
    }

    #[test]
    fn headers_mark_the_edges_next_to_hidden_lines() {
        let mut sh = Sheet::new("S");
        sh.cols.entry(0).or_default().hidden = true; // A
        sh.cols.entry(11).or_default().hidden = true; // L
        sh.rows.entry(26).or_default().hidden = true; // 27
        let col = |c| hidden_around(c, |i| sh.col_width(i));
        assert_eq!(col(1), (true, false)); // B, after hidden A
        assert_eq!(col(10), (false, true)); // K
        assert_eq!(col(12), (true, false)); // M
        assert_eq!(col(5), (false, false));
        let row = |r| hidden_around(r, |i| sh.row_height(i));
        assert_eq!(row(25), (false, true));
        assert_eq!(row(27), (true, false));
        assert_eq!(row(0), (false, false));
    }

    #[test]
    fn overlays_skip_cells_scrolled_under_the_frozen_columns() {
        let mut sh = Sheet::new("S");
        let geo = frozen_geo(&mut sh);
        assert!(geo.hidden_under_frozen(&sh, CellRef::new(5, 2)));
        assert!(geo.hidden_under_frozen(&sh, CellRef::new(5, 4)));
        assert!(!geo.hidden_under_frozen(&sh, CellRef::new(5, 5)));
        assert!(!geo.hidden_under_frozen(&sh, CellRef::new(5, 0)));
        let unscrolled = Geo::new(&sh, geo.rect, vec2(0.0, 0.0));
        assert!(!unscrolled.hidden_under_frozen(&sh, CellRef::new(5, 2)));
    }

    #[test]
    fn ranges_are_clipped_to_the_panes_that_hold_them() {
        let mut sh = Sheet::new("S");
        let geo = frozen_geo(&mut sh);
        let split = pos2(geo.cells.left() + geo.frozen_w, geo.cells.top() + geo.frozen_h);
        assert_eq!(geo.range_panes(rng(5, 3, 7, 6)), [Rect::from_min_max(split, geo.cells.max)]);
        assert_eq!(geo.range_panes(rng(5, 0, 7, 6)).len(), 2);
        assert_eq!(geo.range_panes(rng(0, 0, 7, 6)).len(), 4);
        assert_eq!(geo.range_panes(rng(0, 0, 1, 1)), [Rect::from_min_max(geo.cells.min, split)]);
        // The fill handle of D6:E7 sits over the frozen columns, where nothing of it is shown.
        let handle = geo.range_rect(&sh, rng(5, 3, 6, 4)).right_bottom();
        assert!(handle.x < split.x);
        assert!(!geo.range_panes(rng(6, 4, 6, 4)).iter().any(|r| r.contains(handle)));
    }
}

#[cfg(test)]
mod outline_band_tests {
    use super::*;

    #[test]
    fn whole_column_outline_keeps_left_and_right_sides_near_viewport() {
        let clip = Rect::from_min_max(pos2(0.0, 0.0), pos2(800.0, 600.0));
        let col = Rect::from_min_max(pos2(100.0, 40.0), pos2(180.0, 20_971_540.0));
        let sides = outline_bands(col, 2.0, clip);
        // Left, right and top (in view); the bottom is 20 million points away.
        assert_eq!(sides.len(), 3);
        assert!(sides.iter().all(|s| s.bottom() <= clip.bottom() + 4.0 + f32::EPSILON));
        assert!(sides.iter().any(|s| s.left() < 100.0 && s.right() > 100.0 && s.height() > 500.0));
        assert!(sides.iter().any(|s| s.left() < 180.0 && s.right() > 180.0 && s.height() > 500.0));
    }

    #[test]
    fn outline_outside_the_view_and_non_finite_draw_nothing() {
        let clip = Rect::from_min_max(pos2(0.0, 0.0), pos2(800.0, 600.0));
        assert!(outline_bands(Rect::from_min_max(pos2(900.0, 0.0), pos2(950.0, 20.0)), 2.0, clip).is_empty());
        assert!(outline_bands(Rect::from_min_max(pos2(f32::NAN, 0.0), pos2(1.0, 1.0)), 2.0, clip).is_empty());
    }
}

#[cfg(test)]
mod line_geo_tests {
    use super::*;

    #[test]
    fn line_geometry_is_reused_until_the_revision_changes() {
        let mut sh = Sheet::new("S");
        let mut grid = GridState::default();
        let key = (7, 1, 0);
        let first = grid.line_geo_for(key, &sh);
        let second = grid.line_geo_for(key, &sh);
        assert!(Arc::ptr_eq(&first.ri, &second.ri), "the same revision reuses the row prefix sums");
        assert!(Arc::ptr_eq(&first.ci, &second.ci), "the same revision reuses the column prefix sums");
        assert!(Arc::ptr_eq(&first.row_outline, &second.row_outline));
        assert!(Arc::ptr_eq(&first.col_outline, &second.col_outline));
        // Scrolling, selecting and zooming leave the revision alone: the cached lines survive them.
        let scrolled = grid.line_geo_for(key, &sh);
        assert!(Arc::ptr_eq(&first.ri, &scrolled.ri));

        // A row height is an edit: the revision moves, so the geometry is rebuilt from the new map.
        let before = first.ri.start(2);
        sh.rows.entry(1).or_default().size = Some(60.0);
        let after = grid.line_geo_for((7, 2, 0), &sh);
        assert!(!Arc::ptr_eq(&first.ri, &after.ri), "a new revision rebuilds the row prefix sums");
        assert!(after.ri.start(2) > before, "the added height shifts the rows below it");
        assert_eq!(after.ri.start(2), before + 40.0);

        // The rebuilt geometry is cached in turn, and another sheet is a different key.
        let again = grid.line_geo_for((7, 2, 0), &sh);
        assert!(Arc::ptr_eq(&after.ri, &again.ri));
        let other_sheet = grid.line_geo_for((7, 2, 1), &sh);
        assert!(!Arc::ptr_eq(&after.ri, &other_sheet.ri), "the key tells sheets apart");
    }
}

#[cfg(test)]
mod selected_object_tests {
    use super::selected_object;
    use gridcraft_engine::core::CellRef;
    use gridcraft_engine::model::{Anchor, AnchorMode, Chart, Image, Shape, ShapeKind, Sheet};

    fn anchor() -> Anchor {
        Anchor { cell: CellRef::new(2, 1), dx: 1.0, dy: 2.0, width: 30.0, height: 40.0, mode: AnchorMode::MoveOnly }
    }

    #[test]
    fn shift_f10_targets_the_selected_chart_picture_or_shape() {
        let mut sh = Sheet::new("Data");
        sh.charts.push(Chart { id: 7, anchor: anchor(), ..Chart::default() });
        let im = Image { id: 8, anchor: anchor(), data: std::sync::Arc::new(vec![]), mime: "image/png".into(), alt: String::new() };
        sh.images.push(im);
        sh.shapes.push(Shape {
            id: 9,
            anchor: anchor(),
            kind: ShapeKind::Rectangle,
            fill: Default::default(),
            line: Default::default(),
            text: String::new(),
            alt: None,
            format: Default::default(),
        });
        // Each kind comes back with the object's own anchor; an id nothing holds (or no selection
        // at all) leaves the key to the cell's context menu.
        assert_eq!(selected_object(&sh, Some(7)).map(|(k, i, _)| (k, i)), Some(("chart", 7)));
        assert_eq!(selected_object(&sh, Some(8)).map(|(k, i, _)| (k, i)), Some(("image", 8)));
        assert_eq!(selected_object(&sh, Some(9)).map(|(k, i, _)| (k, i)), Some(("shape", 9)));
        assert_eq!(selected_object(&sh, Some(7)).map(|(_, _, a)| a), Some(anchor()));
        assert_eq!(selected_object(&sh, Some(99)), None);
        assert_eq!(selected_object(&sh, None), None);
    }
}
