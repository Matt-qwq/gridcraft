//! GridCraft charts (layer L3, toolkit-free).
//!
//! - [`resolve`] pulls series names, categories and values out of a workbook,
//! - [`render`] lays a chart out into a list of drawing primitives ([`Prim`]) that any UI
//!   toolkit (or the built-in [`rasterize`]) can paint,
//! - [`render_sparkline`] draws in-cell sparklines,
//! - [`rasterize`] turns primitives into RGBA pixels for PNG export and tests.
//!
//! The look (palette, greys, spacing) is GridCraft's own design.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod axis;
mod raster;
mod render;
mod resolve;
mod sparkline;
mod trend;

#[cfg(test)]
mod tests;

pub use gridcraft_model::{
    AxisCrossing, AxisOptions, Chart, ChartKind, DisplayUnits, ErrorBarDirection, ErrorBarKind, ErrorBars, LabelOptions, LabelPosition, LegendPos,
    PointFormat, SparklineKind, Trendline, TrendlineKind,
};
use serde::{Deserialize, Serialize};

pub use axis::{Scale, format_number, nice_scale};
pub use raster::rasterize;
pub use render::{Swatch, draw, legend_entries, render, render_hits, supports_trendline};
pub use resolve::{default_palette, resolve, series_color, series_plot_kind};
pub use sparkline::render_sparkline;

/// Straight (non-premultiplied) RGBA colour.
pub type Rgba = [u8; 4];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HAlign {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VAlign {
    #[default]
    Top,
    Middle,
    Bottom,
}

/// A drawing primitive in chart coordinates (origin top-left, y down, units = points/pixels).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Prim {
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        fill: Option<Rgba>,
        stroke: Option<(Rgba, f32)>,
        radius: f32,
    },
    Line {
        pts: Vec<[f32; 2]>,
        color: Rgba,
        width: f32,
        dash: bool,
    },
    Polygon {
        pts: Vec<[f32; 2]>,
        fill: Rgba,
        stroke: Option<(Rgba, f32)>,
    },
    /// Pie/doughnut sector: centre, radii, angles in radians (0 = 12 o'clock, clockwise).
    Wedge {
        cx: f32,
        cy: f32,
        r_outer: f32,
        r_inner: f32,
        a0: f32,
        a1: f32,
        fill: Rgba,
        stroke: Option<(Rgba, f32)>,
    },
    Circle {
        cx: f32,
        cy: f32,
        r: f32,
        fill: Option<Rgba>,
        stroke: Option<(Rgba, f32)>,
    },
    /// Text anchored at `(x, y)` per `align`/`valign`, rotated by `rotation` radians about the anchor.
    Text {
        x: f32,
        y: f32,
        text: String,
        size: f32,
        color: Rgba,
        bold: bool,
        align: HAlign,
        valign: VAlign,
        rotation: f32,
    },
}

/// What a click on a drawn chart lands on, in the Format pane's vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// The chart itself: the background, the title, an empty part of the plot, a legend's blank
    /// space.
    ChartArea,
    /// The horizontal axis: its labels or its line (the category axis, or a numeric X axis).
    AxisX,
    /// The vertical (value) axis: its labels or its line.
    AxisY,
    /// One series, from a bar, slice, marker or legend entry; `point` is the index when a single
    /// point (a bar, a slice, a marker) was hit rather than the series as a whole.
    Series { index: usize, point: Option<usize> },
}

/// The shape of one clickable box, in chart coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HitShape {
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    },
    Circle {
        cx: f32,
        cy: f32,
        r: f32,
    },
    /// A pie/doughnut sector, angles in radians (0 = 12 o'clock, clockwise), as [`Prim::Wedge`].
    Wedge {
        cx: f32,
        cy: f32,
        r_outer: f32,
        r_inner: f32,
        a0: f32,
        a1: f32,
    },
}

impl HitShape {
    /// Whether `(x, y)` falls inside the shape. Hostile (non-finite) input is outside everything.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        if !(x.is_finite() && y.is_finite()) {
            return false;
        }
        match *self {
            HitShape::Rect { x: rx, y: ry, w, h } => {
                let (w, h) = (w.max(0.0), h.max(0.0));
                x >= rx && x <= rx + w && y >= ry && y <= ry + h
            }
            HitShape::Circle { cx, cy, r } => {
                let r = r.max(0.0);
                let (dx, dy) = (x - cx, y - cy);
                dx * dx + dy * dy <= r * r
            }
            HitShape::Wedge { cx, cy, r_outer, r_inner, a0, a1 } => {
                let (dx, dy) = (x - cx, y - cy);
                let d = (dx * dx + dy * dy).sqrt();
                if !(d.is_finite() && d >= r_inner.max(0.0) && d <= r_outer.max(0.0)) {
                    return false;
                }
                let sweep = (a1 - a0).max(0.0);
                if sweep >= TAU_HIT {
                    return true;
                }
                // The angle in the drawing's own convention: 0 at 12 o'clock, clockwise.
                let mut ang = dx.atan2(-dy);
                while ang < a0 {
                    ang += TAU_HIT;
                }
                ang <= a1
            }
        }
    }
}

/// Full turn; a sector at least this wide contains every angle.
const TAU_HIT: f32 = std::f32::consts::TAU;

/// One region a click can land on, with what it selects.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HitRegion {
    pub hit: Hit,
    pub shape: HitShape,
}

/// Everything a chart draws, plus the boxes a click on it can land on **in draw order** (a later
/// region is drawn over an earlier one, so the topmost match is the one a click hits).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Drawing {
    pub prims: Vec<Prim>,
    pub hits: Vec<HitRegion>,
}

/// What a click at `(x, y)` in chart coordinates lands on: the last (topmost) region containing it.
pub fn hit_at(hits: &[HitRegion], x: f32, y: f32) -> Option<Hit> {
    hits.iter().rev().find(|r| r.shape.contains(x, y)).map(|r| r.hit)
}

/// Measures text width (the UI passes real font metrics; tests use a fixed-width approximation).
pub trait Measure {
    fn text_width(&self, text: &str, size: f32, bold: bool) -> f32;
}

/// Fixed-width approximation: `0.55 × size` per character (bold 10% wider).
#[derive(Clone, Copy, Debug, Default)]
pub struct ApproxMeasure;

impl Measure for ApproxMeasure {
    fn text_width(&self, text: &str, size: f32, bold: bool) -> f32 {
        let n = text.chars().count() as f32;
        n * 0.55 * size * if bold { 1.1 } else { 1.0 }
    }
}

/// Resolved data: series names, categories and values pulled from the workbook.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ChartData {
    pub categories: Vec<String>,
    /// Outer levels of a multi-level category axis (a category range two columns wide beside a
    /// column of values: year beside month), nearest the labels first. One entry per category,
    /// non-empty where a group starts: `["2023", "", "", "2024", "", ""]`.
    #[serde(default)]
    pub category_levels: Vec<Vec<String>>,
    pub series: Vec<SeriesData>,
    /// Workbook-theme colours for the points of pie-like charts, repeating (point `j` uses entry `j % len`); empty uses the default theme.
    #[serde(default)]
    pub palette: Vec<Rgba>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeriesData {
    pub name: String,
    pub values: Vec<Option<f64>>,
    /// Numeric X values (scatter, bubble).
    pub x: Option<Vec<Option<f64>>>,
    /// Bubble sizes.
    pub sizes: Option<Vec<Option<f64>>>,
    pub color: Rgba,
    /// Per-series kind (combo charts).
    pub kind: ChartKind,
    /// Plotted against the secondary (right) value axis.
    pub secondary: bool,
    /// Draw line segments as a smoothed (spline) curve.
    pub smooth: bool,
    /// Trendlines to fit and draw over this series.
    pub trendlines: Vec<TrendlineData>,
    /// Number format code of the source values ("General" by default).
    pub number_format: String,
    /// Data labels for this series only; `None` uses the chart's.
    pub labels: Option<LabelOptions>,
    /// Error bars with their amounts resolved.
    pub error_bars: Option<ErrorBarsData>,
    /// Per-point formatting overrides, with colours resolved, by point index.
    pub points: Vec<PointStyle>,
    /// Per-point data labels (text, position, what they show), by point index.
    #[serde(default)]
    pub point_labels: Vec<PointLabelData>,
}

/// One point's own data label, with a cell reference already resolved to its text.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PointLabelData {
    pub index: usize,
    /// The label's text (a literal, or what the referenced cell holds); empty draws nothing.
    pub text: Option<String>,
    /// `Auto` keeps the series' or the chart's position.
    pub position: LabelPosition,
    /// Each flag overrides the series' or the chart's option when it is set.
    pub show_value: Option<bool>,
    pub show_series: Option<bool>,
    pub show_category: Option<bool>,
    pub show_percentage: Option<bool>,
}

impl PointLabelData {
    /// Whether any show flag is set (to either value).
    pub fn has_flags(&self) -> bool {
        self.show_value.is_some() || self.show_series.is_some() || self.show_category.is_some() || self.show_percentage.is_some()
    }
}

impl SeriesData {
    /// The own label of point `j`, if any. The list is sorted by index.
    pub fn point_label(&self, j: usize) -> Option<&PointLabelData> {
        self.point_labels.binary_search_by_key(&j, |l| l.index).ok().and_then(|i| self.point_labels.get(i))
    }
}

/// A series' error bars: the model's settings plus the custom amounts resolved against the
/// workbook, one entry per point (empty for the other kinds).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ErrorBarsData {
    pub spec: ErrorBars,
    pub plus: Vec<Option<f64>>,
    pub minus: Vec<Option<f64>>,
}

/// One point's formatting, colours resolved against the theme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PointStyle {
    pub index: usize,
    pub fill: Option<Rgba>,
    pub line: Option<Rgba>,
    pub marker: Option<Rgba>,
    /// The point's own marker size in points; `None` uses the series default.
    pub marker_size: Option<u8>,
    /// The point's data label is deleted.
    pub no_label: bool,
}

/// A trendline to draw: the model settings with the line colour resolved against the theme.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrendlineData {
    pub spec: Trendline,
    pub color: Rgba,
}

impl Default for SeriesData {
    fn default() -> Self {
        SeriesData {
            name: String::new(),
            values: vec![],
            x: None,
            sizes: None,
            color: [0x15, 0x60, 0x82, 0xFF],
            kind: ChartKind::ColumnClustered,
            secondary: false,
            smooth: false,
            trendlines: vec![],
            number_format: "General".into(),
            labels: None,
            error_bars: None,
            points: vec![],
            point_labels: vec![],
        }
    }
}
