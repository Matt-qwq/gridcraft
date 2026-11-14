//! Sheet features beyond cells: tables, conditional formatting, data validation, comments,
//! hyperlinks, charts, images, autofilter, print settings.

use gridcraft_core::{CellRef, RangeRef};
use serde::{Deserialize, Serialize};

use crate::style::{Color, Style};

// ---------------------------------------------------------------- tables

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TotalsFn {
    #[default]
    None,
    Average,
    Count,
    CountNums,
    Max,
    Min,
    Sum,
    StdDev,
    Var,
    Custom,
}

impl TotalsFn {
    /// SUBTOTAL function number.
    pub fn subtotal_code(&self) -> Option<u32> {
        Some(match self {
            TotalsFn::Average => 101,
            TotalsFn::Count => 103,
            TotalsFn::CountNums => 102,
            TotalsFn::Max => 104,
            TotalsFn::Min => 105,
            TotalsFn::Sum => 109,
            TotalsFn::StdDev => 107,
            TotalsFn::Var => 110,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TableColumn {
    pub name: String,
    pub totals: TotalsFn,
    /// Totals row label (first column) or custom formula.
    pub totals_label: Option<String>,
    /// Calculated column formula.
    pub formula: Option<String>,
    /// `uniqueName`: the name XML maps know the column by.
    #[serde(default)]
    pub unique_name: Option<String>,
    /// Binding to an element of an XML map. It moves with the column and goes when the column is
    /// deleted; inserted columns are unmapped.
    #[serde(default)]
    pub xml: Option<XmlColumnPr>,
}

/// A table column's binding to an XML map (`<xmlColumnPr>`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct XmlColumnPr {
    pub map_id: u32,
    pub xpath: String,
    /// `xmlDataType` as written in the file (`string`, `boolean`, `integer`…).
    pub data_type: String,
    pub denormalized: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub id: u32,
    pub name: String,
    /// Whole table including header and totals rows.
    pub range: RangeRef,
    pub header_row: bool,
    pub totals_row: bool,
    pub columns: Vec<TableColumn>,
    pub style: String,
    pub banded_rows: bool,
    pub banded_cols: bool,
    pub first_col: bool,
    pub last_col: bool,
    pub filter_button: bool,
    /// Criteria of the table's AutoFilter (column offset within the table → criterion). With
    /// criteria present, hidden data rows are filtered out rather than hidden by hand, which
    /// SUBTOTAL distinguishes.
    #[serde(default)]
    pub filter: Vec<(u32, FilterCriterion)>,
    /// `tableType` when it isn't a plain worksheet table: `xml` for a table bound to an XML map.
    #[serde(default)]
    pub table_type: Option<String>,
    /// The connection (`xl/connections.xml`) an XML table is bound through.
    #[serde(default)]
    pub connection_id: Option<u32>,
}

impl Table {
    pub fn data_range(&self) -> Option<RangeRef> {
        let r0 = self.range.start.row + self.header_row as u32;
        let r1 = self.range.end.row.checked_sub(self.totals_row as u32)?;
        if r1 < r0 {
            return None;
        }
        Some(RangeRef::new(CellRef::new(r0, self.range.start.col), CellRef::new(r1, self.range.end.col)))
    }
    /// The range an AutoFilter covers: the header row through the last data row; a totals row is
    /// excluded, as the table part's `<autoFilter ref>` spells it.
    pub fn filter_range(&self) -> RangeRef {
        let end = if self.totals_row { self.range.end.row.saturating_sub(1) } else { self.range.end.row };
        RangeRef::new(self.range.start, CellRef::new(end.max(self.range.start.row), self.range.end.col))
    }
    pub fn header_range(&self) -> Option<RangeRef> {
        self.header_row.then(|| RangeRef::new(self.range.start, CellRef::new(self.range.start.row, self.range.end.col)))
    }
    pub fn totals_range(&self) -> Option<RangeRef> {
        self.totals_row.then(|| RangeRef::new(CellRef::new(self.range.end.row, self.range.start.col), self.range.end))
    }
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name.eq_ignore_ascii_case(name))
    }
}

// ---------------------------------------------------------------- conditional formatting

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CfOperator {
    Between,
    NotBetween,
    Equal,
    NotEqual,
    Greater,
    Less,
    GreaterOrEqual,
    LessOrEqual,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CfValueKind {
    Min,
    Max,
    Number(f64),
    Percent(f64),
    Percentile(f64),
    Formula(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CfRule {
    /// Cell value compared with one or two formulas.
    CellIs {
        op: CfOperator,
        a: String,
        b: Option<String>,
        style: Box<Style>,
    },
    /// Custom formula (relative to the top-left cell of the range).
    Expression {
        formula: String,
        style: Box<Style>,
    },
    ContainsText {
        text: String,
        style: Box<Style>,
    },
    NotContainsText {
        text: String,
        style: Box<Style>,
    },
    BeginsWith {
        text: String,
        style: Box<Style>,
    },
    EndsWith {
        text: String,
        style: Box<Style>,
    },
    Blanks {
        style: Box<Style>,
    },
    NoBlanks {
        style: Box<Style>,
    },
    Errors {
        style: Box<Style>,
    },
    NoErrors {
        style: Box<Style>,
    },
    Duplicate {
        style: Box<Style>,
    },
    Unique {
        style: Box<Style>,
    },
    /// Top/bottom N or N percent.
    Top10 {
        bottom: bool,
        percent: bool,
        rank: u32,
        style: Box<Style>,
    },
    AboveAverage {
        below: bool,
        equal: bool,
        std_dev: u8,
        style: Box<Style>,
    },
    /// `yesterday`, `today`, `tomorrow`, `last7Days`, `lastWeek`, `thisWeek`, `nextWeek`, `lastMonth`, `thisMonth`, `nextMonth`.
    TimePeriod {
        period: String,
        style: Box<Style>,
    },
    ColorScale {
        stops: Vec<(CfValueKind, Color)>,
    },
    DataBar {
        min: CfValueKind,
        max: CfValueKind,
        color: Color,
        gradient: bool,
        show_value: bool,
    },
    /// Icon set name (`3Arrows`, `3TrafficLights1`, `4Rating`, `5Quarters`…).
    IconSet {
        set: String,
        thresholds: Vec<CfValueKind>,
        reverse: bool,
        show_value: bool,
    },
}

/// Number of icons in an icon set (`3Arrows` → 3, `5Quarters` → 5), always 3 to 5.
pub fn icon_count(set: &str) -> usize {
    set.chars().next().and_then(|c| c.to_digit(10)).unwrap_or(3).clamp(3, 5) as usize
}

/// Excel's default icon-set thresholds: one per icon, the first being the lower bound,
/// then even percent steps (`0, 33, 67` for three icons).
pub fn default_icon_thresholds(set: &str) -> Vec<CfValueKind> {
    let k = icon_count(set);
    (0..k).map(|i| CfValueKind::Percent((i as f64 * 100.0 / k as f64).round())).collect()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CondFormat {
    pub ranges: Vec<RangeRef>,
    pub rule: CfRule,
    pub priority: u32,
    pub stop_if_true: bool,
}

// ---------------------------------------------------------------- data validation

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValidationKind {
    #[default]
    Any,
    Whole,
    Decimal,
    List,
    Date,
    Time,
    TextLength,
    Custom,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorStyle {
    #[default]
    Stop,
    Warning,
    Information,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Validation {
    pub ranges: Vec<RangeRef>,
    pub kind: ValidationKind,
    pub op: CfOperator,
    /// Formula 1 (list source: `"a,b,c"` literal list or `=$A$1:$A$5`).
    pub f1: String,
    pub f2: Option<String>,
    pub allow_blank: bool,
    pub in_cell_dropdown: bool,
    pub input_title: String,
    pub input_message: String,
    pub show_input: bool,
    pub error_title: String,
    pub error_message: String,
    pub error_style: ErrorStyle,
    pub show_error: bool,
}

impl Default for Validation {
    fn default() -> Self {
        Validation {
            ranges: vec![],
            kind: ValidationKind::Any,
            op: CfOperator::Between,
            f1: String::new(),
            f2: None,
            allow_blank: true,
            in_cell_dropdown: true,
            input_title: String::new(),
            input_message: String::new(),
            show_input: true,
            error_title: String::new(),
            error_message: String::new(),
            error_style: ErrorStyle::Stop,
            show_error: true,
        }
    }
}

// ---------------------------------------------------------------- comments, links

/// A note (classic comment) or a threaded comment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub author: String,
    pub text: String,
    /// Replies of a threaded comment (author, text).
    pub replies: Vec<(String, String)>,
    pub threaded: bool,
    pub resolved: bool,
    pub visible: bool,
    /// Threaded comments: when the thread (first entry) and each reply were written, ISO 8601 as
    /// read from the file (`dT`); empty when unknown, e.g. for comments made here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dates: Vec<String>,
    /// Where the note's box sits and how big it is, as read from the file; `None` (a note made
    /// here, or a file without the shape) writes the default box next to the cell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note_box: Option<NoteBox>,
}

/// A note box's place and size, from the note's VML shape: Excel's `<x:Anchor>` with its corners
/// stored relative to the note's cell, so the box moves with the note when rows or columns are
/// inserted or deleted, and the shape's width and height.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoteBox {
    /// Top-left corner: columns and rows from the note's cell (negative = left of or above it),
    /// then the pixel offset into that column and row.
    pub from: NoteCorner,
    /// Bottom-right corner, the same way.
    pub to: NoteCorner,
    /// The shape's width and height in points (the VML style's `width`/`height`), when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_pt: Option<(f64, f64)>,
}

/// One corner of a [`NoteBox`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteCorner {
    pub col: i64,
    pub col_px: u32,
    pub row: i64,
    pub row_px: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hyperlink {
    /// URL, `mailto:`, a file path (`Book.xlsx`, `Book.xlsx#Sheet1!A2`), or an in-workbook location
    /// like `Sheet2!A1`.
    pub target: String,
    pub tooltip: Option<String>,
    /// The target is outside this workbook (Excel's "Existing File or Web Page"): a relationship in
    /// the file, not a `location`. A bare file name can't be told from a defined name by its text.
    #[serde(default)]
    pub external: bool,
}

// ---------------------------------------------------------------- drawing objects

/// How a floating object is pinned to the grid when its cells move or resize. Mirrors Excel's
/// Format ▸ Size and Properties ▸ Properties, stored in the file as the anchor's `editAs`:
/// `twoCell` (absent `editAs`), `oneCell` and `absolute`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AnchorMode {
    /// Move and size with cells (Excel's default; `<xdr:twoCellAnchor>`).
    #[default]
    MoveAndSize,
    /// Move but don't size with cells (`<xdr:twoCellAnchor editAs="oneCell">` / `<xdr:oneCellAnchor>`).
    MoveOnly,
    /// Don't move or size with cells (`editAs="absolute"` / `<xdr:absoluteAnchor>`).
    Absolute,
}

/// Position of a floating object: top-left cell plus offset, size in points.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    pub cell: CellRef,
    pub dx: f32,
    pub dy: f32,
    pub width: f32,
    pub height: f32,
    /// How the object follows its anchor cell when rows/columns move (sort, insert, delete).
    #[serde(default)]
    pub mode: AnchorMode,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChartKind {
    #[default]
    ColumnClustered,
    ColumnStacked,
    ColumnStacked100,
    BarClustered,
    BarStacked,
    BarStacked100,
    Line,
    LineMarkers,
    LineStacked,
    Pie,
    Doughnut,
    Area,
    AreaStacked,
    Scatter,
    ScatterLines,
    Bubble,
    Radar,
    Histogram,
    Waterfall,
    Funnel,
    Treemap,
    Sunburst,
    BoxWhisker,
    Stock,
    Combo,
}

impl ChartKind {
    /// The Office 2016 types a file keeps as a `chartex` part (`xl/charts/chartExN.xml`) rather than
    /// a DrawingML chart part. They carry no classic style number, so reading one reports style 2.
    pub fn is_chartex(self) -> bool {
        matches!(self, Self::Histogram | Self::BoxWhisker | Self::Waterfall | Self::Funnel | Self::Treemap | Self::Sunburst)
    }
}

/// Where the other axis crosses this one (Format Axis ▸ Axis value).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum AxisCrossing {
    /// Automatic: zero on a value axis, the first category on a category axis.
    #[default]
    AutoZero,
    /// At the minimum: the lowest value, or before the first category.
    Min,
    /// At the maximum: the highest value, or after the last category.
    Max,
    /// A fixed value: a number on a value axis, a 1-based category number on a category axis.
    At(f64),
}

/// A built-in display unit (Format Axis ▸ Display units).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisplayUnits {
    Hundreds,
    Thousands,
    TenThousands,
    HundredThousands,
    Millions,
    TenMillions,
    HundredMillions,
    Billions,
    Trillions,
}

impl DisplayUnits {
    /// The divisor an axis label is shown in (Thousands → 1_000).
    pub fn factor(self) -> f64 {
        match self {
            Self::Hundreds => 1e2,
            Self::Thousands => 1e3,
            Self::TenThousands => 1e4,
            Self::HundredThousands => 1e5,
            Self::Millions => 1e6,
            Self::TenMillions => 1e7,
            Self::HundredMillions => 1e8,
            Self::Billions => 1e9,
            Self::Trillions => 1e12,
        }
    }

    /// The name shown beside the axis, as Excel labels it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Hundreds => "Hundreds",
            Self::Thousands => "Thousands",
            Self::TenThousands => "Ten Thousands",
            Self::HundredThousands => "Hundred Thousands",
            Self::Millions => "Millions",
            Self::TenMillions => "Ten Millions",
            Self::HundredMillions => "Hundred Millions",
            Self::Billions => "Billions",
            Self::Trillions => "Trillions",
        }
    }
}

/// A value or category axis's scaling (Format Axis ▸ Axis Options).
///
/// Bounds are data values, except on a category axis, where `min`/`max` are 1-based category
/// numbers and `major_unit` is the number of categories between labels. All of them are
/// automatic when `None`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AxisOptions {
    /// Fixed lower bound of the axis.
    pub min: Option<f64>,
    /// Fixed upper bound of the axis.
    pub max: Option<f64>,
    /// Distance between major gridlines and labels.
    pub major_unit: Option<f64>,
    /// Logarithmic scale base (2..=1000); `None` is a linear axis.
    pub log_base: Option<f64>,
    /// Values (or categories) run from high to low: `orientation maxMin`.
    pub reversed: bool,
    /// Where the other axis crosses this one.
    pub crosses: AxisCrossing,
    /// Labels are shown in these units, with the unit named beside the axis.
    pub display_units: Option<DisplayUnits>,
    /// Number format code of the axis labels; `None` follows the plotted values.
    pub number_format: Option<String>,
}

/// Where a data label sits relative to its point (`c:dLblPos`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LabelPosition {
    /// Excel's choice for the chart type (outside the end of a bar, best fit in a pie slice).
    #[default]
    Auto,
    /// Centre of the point (pie slices, stacked bars).
    Center,
    InsideEnd,
    InsideBase,
    OutsideEnd,
    /// Excel picks a free spot (pie, doughnut, bar of pie).
    BestFit,
    Left,
    Right,
    Top,
    Bottom,
}

/// A label's own text formatting (`c:txPr/a:p/a:pPr/a:defRPr`): its size, weight and colour.
///
/// The typeface, italic and the label's fill are not modelled: they stay with the part while the
/// element is untouched and are lost when the point's or series' label font is edited.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LabelFont {
    /// Size in points (`sz`, hundredths of a point in the file); `None` keeps the chart's size.
    pub size: Option<f32>,
    pub bold: Option<bool>,
    pub color: Option<Color>,
}

impl LabelFont {
    /// Whether the font says anything (a font with nothing set is no font).
    pub fn is_set(&self) -> bool {
        self.size.is_some() || self.bold.is_some() || self.color.is_some()
    }

    /// The font with its size forced into the range the file and the command accept.
    pub fn sane(&self) -> LabelFont {
        LabelFont { size: self.size.filter(|s| s.is_finite() && (1.0..=400.0).contains(s)), ..*self }
    }
}

/// What a data label shows and where (Add Chart Element ▸ Data Labels, Format Data Labels).
///
/// A chart's labels are on when [`Chart::data_labels`] is set; this says which parts of the label
/// are shown. A label with every part off falls back to the value, as the on/off flag alone meant.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LabelOptions {
    pub show_value: bool,
    pub show_percentage: bool,
    pub show_series: bool,
    pub show_category: bool,
    /// The legend key (a colour square) is drawn beside the label.
    pub show_legend_key: bool,
    pub position: LabelPosition,
    /// Separator between the parts (`, ` when absent).
    pub separator: Option<String>,
    /// Number format of the label text; `None` follows the plotted values.
    pub number_format: Option<String>,
    /// No labels at all for this series (`c:dLbls/c:delete`): Excel hides one series' labels
    /// while the others keep theirs.
    pub hidden: bool,
    /// The labels' own text formatting; a point's own font overrides it for that point.
    pub font: Option<LabelFont>,
}

impl LabelOptions {
    /// Whether any part of a label is shown.
    pub fn shows_anything(&self) -> bool {
        self.show_value || self.show_percentage || self.show_series || self.show_category || self.show_legend_key
    }

    /// The value-only label the single on/off flag used to mean.
    pub fn values() -> Self {
        Self { show_value: true, ..Self::default() }
    }
}

/// Which ends of an error bar are drawn (`c:errBarType`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorBarDirection {
    #[default]
    Both,
    Plus,
    Minus,
}

/// How long an error bar is (`c:errValType`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum ErrorBarKind {
    /// The standard error of the plotted values.
    #[default]
    StdErr,
    /// A fixed amount above and below each point.
    Fixed(f64),
    /// A percentage of each point's value.
    Percentage(f64),
    /// A multiple of the standard deviation of the plotted values.
    StdDev(f64),
    /// Custom amounts per point: references or array constants (`Sheet1!$C$2:$C$5`, `{1,2,3}`).
    Custom { plus: Option<String>, minus: Option<String> },
}

/// A series' error bars (Add Chart Element ▸ Error Bars).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ErrorBars {
    pub direction: ErrorBarDirection,
    pub kind: ErrorBarKind,
    /// No cap across the end of the bar (`noEndCap`); Excel draws caps by default.
    pub no_end_cap: bool,
}

/// Formatting that applies to one point of a series (`c:dPt`): a pie slice, a bar, a marker.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PointFormat {
    /// 0-based point index within the series.
    pub index: u32,
    /// The point's fill colour.
    pub fill: Option<Color>,
    /// The point's outline colour.
    pub line: Option<Color>,
    /// The point's marker colour (line and scatter series).
    pub marker: Option<Color>,
    /// The point's own marker size in points (2–72); `None` follows the series default (5).
    pub marker_size: Option<u8>,
    /// The point's data label is deleted (`c:dLbl` with `delete`).
    pub no_label: bool,
}

/// A point's own data label (`c:dLbl`): its text and what it shows. Excel stores one per point in
/// the series' `c:dLbls`, beside the series-wide options.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PointLabel {
    /// 0-based point index within the series (`c:idx`).
    pub index: u32,
    /// `c:tx`: a cell reference (`Data!$C$2`) or literal text; `None` composes the label from the
    /// show flags instead.
    pub text: Option<String>,
    /// `None` follows the series' or the chart's label options.
    pub show_value: Option<bool>,
    pub show_series: Option<bool>,
    pub show_category: Option<bool>,
    pub show_percentage: Option<bool>,
    /// `Auto` keeps the series' or the chart's position (`c:dLblPos` absent).
    pub position: LabelPosition,
    /// This label's own text formatting (`c:txPr`), overriding the series' or the chart's.
    pub font: Option<LabelFont>,
}

impl PointLabel {
    /// Whether any of the show flags is set (to either value).
    pub fn has_flags(&self) -> bool {
        self.show_value.is_some() || self.show_series.is_some() || self.show_category.is_some() || self.show_percentage.is_some()
    }

    /// Whether the label shows anything at all: a text, a flag switched on, a position or a font.
    pub fn shows_anything(&self) -> bool {
        self.text.as_deref().is_some_and(|t| !t.is_empty())
            || self.show_value == Some(true)
            || self.show_series == Some(true)
            || self.show_category == Some(true)
            || self.show_percentage == Some(true)
            || self.position != LabelPosition::Auto
            || self.font.is_some_and(|f| f.is_set())
    }

    /// Nothing but its index: the record carries no label and is dropped.
    pub fn is_empty(&self) -> bool {
        !self.shows_anything() && !self.has_flags()
    }
}

/// The chart part a chart was read from, kept so that everything GridCraft does not model in it
/// survives an open → save. Opaque to the model (not part of the JSON format); the xlsx crate
/// writes it back byte for byte while the chart still matches what was read.
#[derive(Clone, PartialEq)]
pub struct ChartPart {
    pub xml: std::sync::Arc<str>,
    /// The model as read, without this field: an edit makes the part stale.
    as_read: Box<Chart>,
    /// A chartex part's hidden `_xlchart.v1.N` names and the references they stood for, as the
    /// reading workbook defined them. A chartex part refers to its data through these names, so a
    /// kept part is only valid while the names are written again.
    names: Vec<(String, String)>,
    /// The chart's own style and colour parts (`xl/charts/styleN.xml`, `colorsN.xml`), as the file
    /// held them. They describe the chart's look, not its data, so a patched chart part keeps them.
    style: Option<std::sync::Arc<str>>,
    colors: Option<std::sync::Arc<str>>,
}

impl ChartPart {
    /// Most hidden chartex names kept from a file.
    const MAX_NAMES: usize = 4096;
    /// Longest chartex name or reference kept (characters).
    const MAX_NAME_CHARS: usize = 4096;
}

impl std::fmt::Debug for ChartPart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChartPart").field("bytes", &self.xml.len()).finish()
    }
}

#[derive(Clone, PartialEq)]
pub struct KeptChart {
    pub id: u32,
    pub anchor: Anchor,
    /// The chart part, written back byte for byte: its kind is not modelled (3-D, surface, pie of
    /// pie, a Pareto line, a filled map) and rebuilding it would silently lose the chart.
    pub xml: std::sync::Arc<str>,
    /// A `chartEx` part (Office 2016 and later), drawn through a `mc:AlternateContent` frame.
    pub chartex: bool,
    /// The chart's own style and colour parts (`xl/charts/styleN.xml`, `colorsN.xml`), written back
    /// with it byte for byte.
    pub style: Option<std::sync::Arc<str>>,
    pub colors: Option<std::sync::Arc<str>>,
    /// Alternative text a screen reader reads for the chart (`xdr:cNvPr/@descr`).
    pub alt: Option<String>,
}

impl std::fmt::Debug for KeptChart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeptChart").field("id", &self.id).field("bytes", &self.xml.len()).finish()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LegendPos {
    None,
    #[default]
    Bottom,
    Top,
    Left,
    Right,
}

/// Quartile calculation for a box & whisker series.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuartileMethod {
    #[default]
    Exclusive,
    Inclusive,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Series {
    /// Formula text for the series name (`Sheet1!$B$1`) or a literal.
    pub name: Option<String>,
    pub categories: Option<String>,
    pub values: String,
    /// X values for scatter / bubble sizes.
    pub bubble_sizes: Option<String>,
    pub color: Option<Color>,
    /// Combo charts: this series drawn as a line on the secondary axis.
    pub secondary: bool,
    pub kind: Option<ChartKind>,
    /// Draw line segments as a smoothed (spline) curve.
    #[serde(default)]
    pub smooth: bool,
    /// Box & whisker quartiles; Excel defaults to exclusive.
    #[serde(default)]
    pub quartiles: QuartileMethod,
    /// Trendlines fitted to this series (Excel allows several per series).
    #[serde(default)]
    pub trendlines: Vec<Trendline>,
    /// Data labels for this series only (Excel's Format Data Labels on a selected series).
    #[serde(default)]
    pub labels: Option<LabelOptions>,
    /// Error bars drawn on this series.
    #[serde(default)]
    pub error_bars: Option<ErrorBars>,
    /// Per-point formatting overrides (a slice, a bar, a marker), sorted by index.
    #[serde(default)]
    pub points: Vec<PointFormat>,
    /// Per-point data labels (text, position, what they show), sorted by index.
    #[serde(default)]
    pub point_labels: Vec<PointLabel>,
}

impl Series {
    /// The formatting override for point `j`, if the file carried one.
    pub fn point(&self, j: usize) -> Option<&PointFormat> {
        let j = u32::try_from(j).ok()?;
        self.points.iter().find(|p| p.index == j)
    }

    /// Stores an override, replacing an entry for the same point.
    pub fn set_point(&mut self, p: PointFormat) {
        match self.points.iter_mut().find(|x| x.index == p.index) {
            Some(slot) => *slot = p,
            None => self.points.push(p),
        }
        self.points.sort_by_key(|x| x.index);
    }

    /// The per-point data label for point `j`, if the file or an edit stored one.
    pub fn point_label(&self, j: usize) -> Option<&PointLabel> {
        let j = u32::try_from(j).ok()?;
        self.point_labels.iter().find(|l| l.index == j)
    }

    /// Stores a point's label, replacing an entry for the same point; a record that carries nothing
    /// is dropped instead.
    pub fn set_point_label(&mut self, label: PointLabel) {
        match self.point_labels.iter_mut().find(|x| x.index == label.index) {
            Some(_) if label.is_empty() => {
                let index = label.index;
                self.point_labels.retain(|x| x.index != index);
                return;
            }
            Some(slot) => *slot = label,
            None if !label.is_empty() => self.point_labels.push(label),
            None => return,
        }
        self.point_labels.sort_by_key(|x| x.index);
    }
}

/// Regression a trendline fits (`c:trendlineType`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrendlineKind {
    #[default]
    Linear,
    /// `y = c·e^(b·x)`.
    Exponential,
    /// `y = c·ln(x) + b`.
    Logarithmic,
    /// Polynomial of this order (2..=6).
    Polynomial(u8),
    /// `y = c·x^b`.
    Power,
    /// Average of this many consecutive points (2..=255).
    MovingAverage(u32),
}

/// DrawingML preset line dash (`a:prstDash`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineDash {
    #[default]
    Solid,
    Dot,
    Dash,
    LgDash,
    DashDot,
    LgDashDot,
    LgDashDotDot,
    SysDash,
    SysDot,
    SysDashDot,
    SysDashDotDot,
}

/// A chart trendline (Add Chart Element ▸ Trendline, Format Trendline pane).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Trendline {
    pub kind: TrendlineKind,
    /// Custom name shown in the legend; `None` = automatic ("Linear (Sales)").
    pub name: Option<String>,
    /// Forecast past the last point, in x units (categories on a category axis).
    pub forward: f64,
    /// Forecast before the first point, in x units.
    pub backward: f64,
    /// Set Intercept: fixed y at x = 0 (linear, exponential, polynomial).
    pub intercept: Option<f64>,
    pub display_equation: bool,
    pub display_r_squared: bool,
    /// Line colour; `None` = the series colour.
    pub color: Option<Color>,
    /// Line width in points; `None` = default.
    pub width: Option<f32>,
    /// Line dash; `None` = default.
    pub dash: Option<LineDash>,
}

impl Trendline {
    /// Longest forward / backward forecast kept from a file or a command, in x units. Excel's own
    /// limit is unconfirmed; the renderer additionally bounds the forecast by the plot's slot budget.
    pub const MAX_FORECAST: f64 = 100_000.0;

    /// A forecast distance as stored: finite, not negative (NaN and infinity become 0) and at most
    /// [`Self::MAX_FORECAST`].
    pub fn clamp_forecast(v: f64) -> f64 {
        if v.is_finite() && v > 0.0 { v.min(Self::MAX_FORECAST) } else { 0.0 }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Chart {
    pub id: u32,
    pub kind: ChartKind,
    pub anchor: Anchor,
    pub title: Option<String>,
    pub series: Vec<Series>,
    pub legend: LegendPos,
    pub data_labels: bool,
    pub gridlines: bool,
    pub style: u32,
    pub x_title: Option<String>,
    pub y_title: Option<String>,
    /// Source range when created from a selection (for Switch Row/Column).
    pub source: Option<String>,
    pub by_rows: bool,
    /// A PivotChart's PivotTable (`Sheet2!PivotTable1`): its series are rebuilt from the report
    /// whenever the PivotTable is refreshed or its layout changes.
    #[serde(default)]
    pub pivot: Option<String>,
    /// Alternative text a screen reader reads for the chart (`xdr:cNvPr/@descr`); empty is what
    /// the accessibility check reports.
    #[serde(default)]
    pub alt: Option<String>,
    /// Category axis scaling and labels.
    #[serde(default)]
    pub x_axis: AxisOptions,
    /// Value axis scaling and labels (the primary axis of a combo chart).
    #[serde(default)]
    pub y_axis: AxisOptions,
    /// The secondary (right) value axis of a combo chart, for the series plotted against it.
    #[serde(default)]
    pub y2_axis: AxisOptions,
    /// What data labels show; `data_labels` turns them on and off.
    #[serde(default)]
    pub labels: LabelOptions,
    /// The chart part as read from a file, for options GridCraft does not model. Not serialized.
    #[serde(skip)]
    pub part: Option<Box<ChartPart>>,
}

impl Chart {
    /// Keeps the chart part `xml`, read into `self`, for a later save.
    pub fn keep_part(&mut self, xml: std::sync::Arc<str>) {
        let mut as_read = self.clone();
        as_read.part = None;
        self.part = Some(Box::new(ChartPart { xml, as_read: Box::new(as_read), names: Vec::new(), style: None, colors: None }));
    }

    /// Keeps the chart's own style and colour parts (`styleN.xml`, `colorsN.xml`) with its chart
    /// part. They do not depend on the chart's data, so an edit that only patches the part keeps
    /// them; a chart part regenerated from the model drops them.
    pub fn keep_style_parts(&mut self, style: Option<std::sync::Arc<str>>, colors: Option<std::sync::Arc<str>>) {
        let Some(part) = self.part.as_deref_mut() else { return };
        part.style = style;
        part.colors = colors;
    }

    /// The kept style and colour parts, as the file held them (`None` for one the file had not).
    pub fn kept_style_parts(&self) -> (Option<&str>, Option<&str>) {
        match self.part.as_deref() {
            Some(part) => (part.style.as_deref(), part.colors.as_deref()),
            None => (None, None),
        }
    }

    /// Records the workbook's hidden chartex names (`_xlchart.v1.3` → `Data!$B$2:$B$5`) on the kept
    /// part, so a chartex part can be written back with its data references intact.
    pub fn keep_chart_names(&mut self, names: &[(String, String)]) {
        let Some(part) = self.part.as_deref_mut() else { return };
        // Only a chartex part refers to data through these names.
        if !part.xml.contains("chartex") {
            return;
        }
        part.names = names
            .iter()
            .take(ChartPart::MAX_NAMES)
            .map(|(n, r)| (n.chars().take(ChartPart::MAX_NAME_CHARS).collect(), r.chars().take(ChartPart::MAX_NAME_CHARS).collect()))
            .collect();
    }

    /// The chartex names recorded on the kept part (empty for a classic part).
    pub fn part_chart_names(&self) -> &[(String, String)] {
        match self.part.as_deref() {
            Some(part) => &part.names,
            None => &[],
        }
    }

    /// The chart part as read and the model it was read as, so an edit can patch the fields it
    /// changes into the kept part instead of regenerating it.
    pub fn kept_part(&self) -> Option<(&str, &Chart)> {
        let part = self.part.as_deref()?;
        Some((part.xml.as_ref(), &part.as_read))
    }

    /// The chart part to write back unchanged: only while nothing in the model changed since it was
    /// read. When only modelled fields changed, [`Self::kept_part`] lets the writer patch them in.
    pub fn unchanged_part(&self) -> Option<&str> {
        let part = self.part.as_deref()?;
        let mut now = self.clone();
        let mut then = (*part.as_read).clone();
        now.part = None;
        then.part = None;
        // The anchor and the object id live in the drawing part, not in the chart part.
        now.anchor = Anchor::default();
        then.anchor = Anchor::default();
        now.id = 0;
        then.id = 0;
        (now == then).then_some(part.xml.as_ref())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Image {
    pub id: u32,
    pub anchor: Anchor,
    /// Encoded bytes (PNG/JPEG), base64 in JSON. Shared, so a copy of the sheet (an undo step)
    /// doesn't copy the picture.
    #[serde(with = "crate::b64::shared")]
    pub data: std::sync::Arc<Vec<u8>>,
    pub mime: String,
    pub alt: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeKind {
    #[default]
    Rectangle,
    RoundedRectangle,
    Ellipse,
    Triangle,
    Line,
    Arrow,
    TextBox,
    /// An icon from GridCraft's own icon set; `Shape::text` holds the icon name.
    Icon,
    /// A freehand ink stroke; `Shape::text` holds the points as `x,y;x,y;…` relative to the
    /// anchor (sheet points), `Shape::line` the ink colour and `fill` unused.
    Ink,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub id: u32,
    pub kind: ShapeKind,
    pub anchor: Anchor,
    pub fill: Color,
    pub line: Color,
    pub text: String,
    /// Alternative text a screen reader reads for the shape (`xdr:cNvPr/@descr`); a shape with
    /// text of its own needs none.
    #[serde(default)]
    pub alt: Option<String>,
    /// Outline visibility and text formatting (DrawingML `a:ln`, `a:bodyPr`, `a:pPr`, `a:rPr`).
    #[serde(default)]
    pub format: ShapeFormat,
}

/// Horizontal alignment of a shape's text (`a:pPr algn`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeTextAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// Vertical anchor of a shape's text in its box (`a:bodyPr anchor`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeTextAnchor {
    #[default]
    Top,
    Middle,
    Bottom,
}

/// How a shape's outline and text look. The text properties apply to the whole text (the first
/// paragraph's and first run's properties when a file has several).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShapeFormat {
    /// No outline (`a:ln/a:noFill`), whatever `Shape::line` says.
    pub no_line: bool,
    /// Text size in points (`a:rPr sz` / 100); `None` is the default size.
    pub font_size: Option<f32>,
    pub bold: bool,
    pub italic: bool,
    /// Typeface (`a:latin typeface`); `None` is the default font.
    pub font: Option<String>,
    /// Text fill colour; `None` is the default (black in text boxes).
    pub text_color: Option<Color>,
    /// Text outline (`a:rPr/a:ln`): colour and width in points.
    pub text_outline: Option<(Color, f32)>,
    /// `None`: the shape's default (left for text boxes, centred for other shapes).
    pub align: Option<ShapeTextAlign>,
    /// `None`: the shape's default (top for text boxes, middle for other shapes).
    pub anchor: Option<ShapeTextAnchor>,
    /// Paragraphs run right to left (`a:pPr rtl`).
    pub rtl: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SparklineKind {
    #[default]
    Line,
    Column,
    WinLoss,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sparkline {
    pub cell: CellRef,
    pub source: String,
    pub kind: SparklineKind,
    pub color: Color,
    pub markers: bool,
}

// ---------------------------------------------------------------- filter & sort

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum FilterCriterion {
    /// Show rows whose display text is one of these (case-insensitive); `blanks` includes empty.
    Values {
        values: Vec<String>,
        blanks: bool,
    },
    /// Custom: one or two comparisons (`op` like `>=`, `=*abc*`), joined by and/or.
    Custom {
        a: (String, String),
        b: Option<(String, String)>,
        and: bool,
    },
    Top10 {
        bottom: bool,
        percent: bool,
        count: u32,
    },
    AboveAverage(bool),
    FillColor(Color),
    FontColor(Color),
    /// Rows whose cell shows icon `id` (0-based; `None` = no icon) of conditional-format icon set
    /// `set` (`iconFilter`). Kept from files; re-applying the filter leaves those rows as they are.
    Icon {
        set: String,
        id: Option<u32>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutoFilter {
    pub range: RangeRef,
    /// Column offset within `range` → criterion.
    pub criteria: Vec<(u32, FilterCriterion)>,
}

/// A slicer: an on-sheet filter widget bound to one column of a table. Clicking its value tiles
/// sets that column's `FilterCriterion::Values` in the sheet's [`AutoFilter`] and re-applies it,
/// so a slicer is just a visual front-end over the existing filter machinery.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Slicer {
    pub id: u32,
    /// Name of the table this slicer filters.
    pub table: String,
    /// Header name of the filtered column within the table.
    pub column: String,
    /// Caption shown in the slicer header (defaults to the column name).
    pub caption: String,
    pub anchor: Anchor,
    /// Selected display values. `None` means every value (no filter from this slicer); an empty
    /// list means nothing is selected (all rows hidden), matching Excel's "clear all buttons".
    pub selected: Option<Vec<String>>,
    /// Number of button columns in the layout (Excel default 1).
    pub columns: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Orientation {
    #[default]
    Portrait,
    Landscape,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrintSettings {
    pub orientation: Orientation,
    /// Paper size name (`Letter`, `A4`, `Legal`…).
    pub paper: String,
    /// Margins in inches: left, right, top, bottom, header, footer.
    pub margins: [f32; 6],
    pub print_area: Option<RangeRef>,
    pub title_rows: Option<(u32, u32)>,
    pub title_cols: Option<(u32, u32)>,
    /// Percent scale, or fit to pages (wide, tall).
    pub scale: u16,
    pub fit_to: Option<(u16, u16)>,
    pub gridlines: bool,
    pub headings: bool,
    pub center_h: bool,
    pub center_v: bool,
    /// The header and footer of odd pages, and of every page unless the variants below apply.
    pub header: String,
    pub footer: String,
    pub row_breaks: Vec<u32>,
    pub col_breaks: Vec<u32>,
    /// Different First Page: the sheet's first printed page uses `first_header`/`first_footer`.
    #[serde(default)]
    pub different_first: bool,
    /// Different Odd & Even Pages: even-numbered pages use `even_header`/`even_footer`.
    #[serde(default)]
    pub different_odd_even: bool,
    #[serde(default)]
    pub first_header: String,
    #[serde(default)]
    pub first_footer: String,
    #[serde(default)]
    pub even_header: String,
    #[serde(default)]
    pub even_footer: String,
    /// Scale with Document / Align with Page Margins (both on unless a file turned them off).
    #[serde(default = "yes")]
    pub hf_scale_with_doc: bool,
    #[serde(default = "yes")]
    pub hf_align_with_margins: bool,
}

fn yes() -> bool {
    true
}

/// Which header/footer pair a printed page uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HfPage {
    Odd,
    Even,
    First,
}

impl HfPage {
    pub fn id(self) -> &'static str {
        match self {
            HfPage::Odd => "odd",
            HfPage::Even => "even",
            HfPage::First => "first",
        }
    }
    pub fn parse(id: &str) -> Option<HfPage> {
        match id {
            "odd" | "all" => Some(HfPage::Odd),
            "even" => Some(HfPage::Even),
            "first" => Some(HfPage::First),
            _ => None,
        }
    }
}

impl PrintSettings {
    /// The pair used by page `number` (1-based, as `&P` prints it); `first` is the sheet's first page.
    pub fn hf_page(&self, first: bool, number: usize) -> HfPage {
        if self.different_first && first {
            HfPage::First
        } else if self.different_odd_even && number.is_multiple_of(2) {
            HfPage::Even
        } else {
            HfPage::Odd
        }
    }
    /// The header (`footer` false) or footer of a page kind.
    pub fn hf(&self, page: HfPage, footer: bool) -> &String {
        match (page, footer) {
            (HfPage::Odd, false) => &self.header,
            (HfPage::Odd, true) => &self.footer,
            (HfPage::Even, false) => &self.even_header,
            (HfPage::Even, true) => &self.even_footer,
            (HfPage::First, false) => &self.first_header,
            (HfPage::First, true) => &self.first_footer,
        }
    }
    pub fn hf_mut(&mut self, page: HfPage, footer: bool) -> &mut String {
        match (page, footer) {
            (HfPage::Odd, false) => &mut self.header,
            (HfPage::Odd, true) => &mut self.footer,
            (HfPage::Even, false) => &mut self.even_header,
            (HfPage::Even, true) => &mut self.even_footer,
            (HfPage::First, false) => &mut self.first_header,
            (HfPage::First, true) => &mut self.first_footer,
        }
    }
    /// Any header or footer text at all (on any kind of page).
    pub fn has_header_footer(&self) -> bool {
        [&self.header, &self.footer, &self.first_header, &self.first_footer, &self.even_header, &self.even_footer].iter().any(|t| !t.is_empty())
    }
}

impl Default for PrintSettings {
    fn default() -> Self {
        PrintSettings {
            orientation: Orientation::Portrait,
            paper: "Letter".into(),
            margins: [0.7, 0.7, 0.75, 0.75, 0.3, 0.3],
            print_area: None,
            title_rows: None,
            title_cols: None,
            scale: 100,
            fit_to: None,
            gridlines: false,
            headings: false,
            center_h: false,
            center_v: false,
            header: String::new(),
            footer: String::new(),
            row_breaks: vec![],
            col_breaks: vec![],
            different_first: false,
            different_odd_even: false,
            first_header: String::new(),
            first_footer: String::new(),
            even_header: String::new(),
            even_footer: String::new(),
            hf_scale_with_doc: true,
            hf_align_with_margins: true,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SheetProtection {
    /// Hash of the password (Excel-compatible, written to XLSX `sheetProtection`; never the
    /// password itself).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<crate::PasswordHash>,
    /// The internal hash older GridCraft JSON workbooks stored ([`password_hash`]); still
    /// verified when unprotecting, never written for new passwords.
    #[serde(rename = "password_hash", default, skip_serializing_if = "Option::is_none")]
    pub legacy_password_hash: Option<u64>,
    pub select_locked: bool,
    pub select_unlocked: bool,
    pub format_cells: bool,
    pub format_columns: bool,
    pub format_rows: bool,
    pub insert_columns: bool,
    pub insert_rows: bool,
    pub delete_columns: bool,
    pub delete_rows: bool,
    pub sort: bool,
    pub autofilter: bool,
}

/// Review › Allow Edit Ranges: cells that stay editable on a protected sheet, with or without a
/// password (XLSX `<protectedRange>`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EditRange {
    pub title: String,
    pub ranges: Vec<RangeRef>,
    pub password: Option<crate::PasswordHash>,
    /// Windows permissions (`securityDescriptor`), kept as read from the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_descriptor: Option<String>,
}

impl EditRange {
    pub fn contains(&self, c: CellRef) -> bool {
        self.ranges.iter().any(|r| r.contains(c))
    }
}

/// The non-cryptographic password hash older GridCraft versions stored for sheet protection
/// (only verified now, see [`SheetProtection::legacy_password_hash`]).
pub fn password_hash(pw: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325 ^ 0x5348_4545_5443;
    for b in pw.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Outline grouping info for a row or column.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LineInfo {
    /// Size in pixels at 96 dpi; `None` = default (rows auto-fit to content).
    pub size: Option<f32>,
    pub hidden: bool,
    pub outline: u8,
    pub collapsed: bool,
    pub style: Option<crate::style::StyleId>,
    /// The size was set by the user (rows): automatic heights follow the content.
    #[serde(default)]
    pub custom: bool,
}

// ---------------------------------------------------------------- pivot tables

/// How a PivotTable lays out its row fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PivotLayout {
    /// All row fields in one column, inner items indented ("Row Labels" header).
    #[default]
    Compact,
    /// One column per row field; each item on its own row.
    Outline,
    /// One column per row field; inner items start on their parent's row, totals below.
    Tabular,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PivotSort {
    #[default]
    Asc,
    Desc,
    /// Source order (first appearance).
    None,
}

/// Grouping of a date field (values are date serials).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PivotDateGroup {
    #[default]
    None,
    Years,
    Quarters,
    Months,
    Days,
}

/// Numeric grouping of a number field: buckets `interval` wide from `start` to `end` (inclusive),
/// as Excel's "Group Field ▸ Starting at / Ending at / By" does. A value outside the range falls in
/// the `<start` or `>end` group, as Excel shows it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PivotNumberGroup {
    pub start: f64,
    pub end: f64,
    pub interval: f64,
}

impl PivotNumberGroup {
    /// The most groups one field may have: a hostile `start`/`end`/`interval` must not ask for an
    /// unbounded caption or group-item list.
    pub const MAX_GROUPS: u64 = 100_000;

    /// Whether the group can be used: finite bounds, a positive interval, an ordered range and a
    /// bounded number of buckets.
    pub fn is_valid(&self) -> bool {
        self.start.is_finite()
            && self.end.is_finite()
            && self.interval.is_finite()
            && self.interval > 0.0
            && self.start <= self.end
            && self.groups().is_some_and(|n| n <= Self::MAX_GROUPS)
    }

    /// The number of buckets: the boundaries run `start + k x interval`, and `end` sits in the
    /// last bucket, so the count is `ceil((end - start) / interval)`, at least one. `None` when
    /// that count would exceed [`Self::MAX_GROUPS`] (or the bounds are unusable).
    pub fn groups(&self) -> Option<u64> {
        let span = (self.end - self.start) / self.interval;
        if !span.is_finite() || span < 0.0 || span >= Self::MAX_GROUPS as f64 {
            return None;
        }
        Some(span.ceil().max(1.0) as u64)
    }

    /// The first value past the last bucket. Excel names the upper tail after this boundary, not
    /// after `end` (`...53740-78739`, `>78740` for 28740..72410 by 25000).
    pub fn next_boundary(&self) -> f64 {
        self.start + self.groups().unwrap_or(1) as f64 * self.interval
    }

    /// The caption of a value's group and a key that sorts the groups in order.
    pub fn bucket(&self, v: f64) -> Option<(String, f64)> {
        if !self.is_valid() || !v.is_finite() {
            return None;
        }
        if v < self.start {
            return Some((format!("<{}", number_text(self.start)), f64::NEG_INFINITY));
        }
        let i = ((v - self.start) / self.interval).floor();
        if !i.is_finite() || i < 0.0 {
            return None;
        }
        // The tail holds the next boundary and everything past `end` (the last bucket's end).
        if i >= self.groups().unwrap_or(1) as f64 || v > self.end {
            return Some((format!(">{}", number_text(self.next_boundary())), f64::INFINITY));
        }
        let lo = self.start + i * self.interval;
        Some((self.span_caption(lo), lo))
    }

    /// Every group caption in order: `<start`, the buckets, `>next boundary`.
    pub fn captions(&self) -> Vec<String> {
        let mut v = vec![format!("<{}", number_text(self.start))];
        if self.is_valid() {
            for i in 0..self.groups().unwrap_or(0) {
                let lo = self.start + i as f64 * self.interval;
                v.push(self.span_caption(lo));
            }
        }
        v.push(format!(">{}", number_text(self.next_boundary())));
        v
    }

    /// "0-9" for whole steps (the last value the bucket holds), "0-0.5" for a fractional one (the
    /// bucket's half-open end), the way Excel names a group by its range.
    fn span_caption(&self, lo: f64) -> String {
        let hi = lo + self.interval;
        if self.interval.fract() == 0.0 && lo.fract() == 0.0 {
            format!("{}-{}", number_text(lo), number_text(hi - 1.0))
        } else {
            format!("{}-{}", number_text(lo), number_text(hi))
        }
    }
}

/// A number as a group caption writes it: whole numbers without a trailing `.0`, everything else
/// in Rust's shortest round-trip form.
fn number_text(x: f64) -> String {
    if !x.is_finite() {
        return format!("{x}");
    }
    if x.fract() == 0.0 && x.abs() < 1e15 { format!("{}", x as i64) } else { format!("{x}") }
}

/// A field placed in the Rows or Columns area.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PivotField {
    /// Source column header name.
    pub source_col: String,
    pub sort: PivotSort,
    pub date_group: PivotDateGroup,
    /// Numeric grouping; set instead of `date_group` (a field has one grouping).
    pub number_group: Option<PivotNumberGroup>,
    /// Item labels whose detail is hidden (collapsed).
    pub collapsed_items: Vec<String>,
}

/// Summarize-values-by function.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PivotFunc {
    #[default]
    Sum,
    Count,
    Average,
    Max,
    Min,
    Product,
    CountNumbers,
    StdDev,
    StdDevP,
    Var,
    VarP,
}

/// Show-values-as transform.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PivotShowAs {
    #[default]
    Normal,
    PercentOfGrandTotal,
    PercentOfColumnTotal,
    PercentOfRowTotal,
    /// Running total in the first row field.
    RunningTotal,
    /// Rank largest to smallest among sibling row items.
    Rank,
}

/// A field in the Values area.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PivotValue {
    pub source_col: String,
    pub func: PivotFunc,
    /// Caption, e.g. "Sum of Sales".
    pub name: String,
    pub show_as: PivotShowAs,
    pub number_format: Option<String>,
}

/// Where a filter's field sits, which decides how the filter is shown and written: a report filter
/// gets a row above the table and a `pageFields` entry, an axis filter only hides items.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PivotFilterArea {
    /// Not recorded (a document from before the area was modelled): a field on an axis filters
    /// that axis, any other field is a report filter.
    #[default]
    Auto,
    /// The Filters area: drawn as its own row above the table (`c:pageFields`).
    Page,
    /// Hidden items on a field that is on an axis (`c:pivotField/item/@h`).
    Axis,
}

/// A report filter (or, for a field also in Rows/Columns, its hidden-items filter).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PivotFilter {
    pub source_col: String,
    /// Item labels shown; `None` = all.
    pub selected: Option<Vec<String>>,
    pub area: PivotFilterArea,
}

impl PivotFilter {
    /// Whether the filter is drawn as a report-filter row: what the file said, else a field that is
    /// not on an axis.
    pub fn is_page(&self, on_axis: bool) -> bool {
        match self.area {
            PivotFilterArea::Page => true,
            PivotFilterArea::Axis => false,
            PivotFilterArea::Auto => !on_axis,
        }
    }
}

/// How a label filter compares a field's item captions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PivotLabelOp {
    #[default]
    Equals,
    NotEquals,
    BeginsWith,
    EndsWith,
    Contains,
    DoesNotContain,
    GreaterThan,
    GreaterThanOrEqual,
    LessThan,
    LessThanOrEqual,
    Between,
    NotBetween,
}

/// A label filter: keeps the items of a row or column field whose caption matches. Text compares
/// case-insensitively, and `>`/`<` are a plain case-insensitive text order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PivotLabelFilter {
    pub source_col: String,
    pub op: PivotLabelOp,
    pub value: String,
    pub value2: Option<String>,
}

impl PivotLabelFilter {
    pub fn passes(&self, label: &str) -> bool {
        let l = label.to_lowercase();
        let a = self.value.to_lowercase();
        let b = self.value2.as_deref().unwrap_or_default().to_lowercase();
        match self.op {
            PivotLabelOp::Equals => l == a,
            PivotLabelOp::NotEquals => l != a,
            PivotLabelOp::BeginsWith => l.starts_with(&a),
            PivotLabelOp::EndsWith => l.ends_with(&a),
            PivotLabelOp::Contains => l.contains(&a),
            PivotLabelOp::DoesNotContain => !l.contains(&a),
            PivotLabelOp::GreaterThan => l > a,
            PivotLabelOp::GreaterThanOrEqual => l >= a,
            PivotLabelOp::LessThan => l < a,
            PivotLabelOp::LessThanOrEqual => l <= a,
            PivotLabelOp::Between => l >= a && l <= b,
            PivotLabelOp::NotBetween => l < a || l > b,
        }
    }
}

/// How a value filter compares an item's aggregate with the bounds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PivotValueOp {
    #[default]
    Equals,
    NotEquals,
    GreaterThan,
    GreaterThanOrEqual,
    LessThan,
    LessThanOrEqual,
    Between,
    NotBetween,
}

/// A value filter: keeps the items of a row or column field whose aggregate of `measure` passes
/// (Excel's "Value Filters"). The measure is a value field's caption ("Sum of Sales") or, when
/// unambiguous, its source column.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PivotValueFilter {
    pub source_col: String,
    pub measure: String,
    pub op: PivotValueOp,
    pub value: f64,
    pub value2: Option<f64>,
}

impl PivotValueFilter {
    pub fn passes(&self, x: f64) -> bool {
        match self.op {
            PivotValueOp::Equals => x == self.value,
            PivotValueOp::NotEquals => x != self.value,
            PivotValueOp::GreaterThan => x > self.value,
            PivotValueOp::GreaterThanOrEqual => x >= self.value,
            PivotValueOp::LessThan => x < self.value,
            PivotValueOp::LessThanOrEqual => x <= self.value,
            PivotValueOp::Between => x >= self.value && x <= self.value2.unwrap_or(self.value),
            PivotValueOp::NotBetween => x < self.value || x > self.value2.unwrap_or(self.value),
        }
    }
}

/// A PivotTable: its definition plus where its output was last written. The output itself is
/// static cell values and styles on the sheet, rewritten on every refresh.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PivotTable {
    pub id: u32,
    pub name: String,
    /// Source: range formula text (`Sheet1!$A$1:$E$200`), a table name or a defined name.
    pub source: String,
    /// Top-left of the output body on the pivot's sheet.
    pub anchor: CellRef,
    pub rows: Vec<PivotField>,
    pub columns: Vec<PivotField>,
    pub values: Vec<PivotValue>,
    pub filters: Vec<PivotFilter>,
    pub layout: PivotLayout,
    /// Grand total for each row (the "Grand Total" column), like Excel's "On for Rows".
    pub grand_totals_rows: bool,
    /// Grand total for each column (the "Grand Total" row), like Excel's "On for Columns".
    pub grand_totals_cols: bool,
    pub subtotals: bool,
    /// Subtotals at the top of each group (Compact/Outline); otherwise below it.
    pub subtotals_top: bool,
    pub style: String,
    pub show_headers: bool,
    /// Area written by the last refresh.
    pub last_range: Option<RangeRef>,
    /// Calculated fields, usable in the Values area by name.
    pub calculated_fields: Vec<PivotCalcField>,
    /// Calculated items: extra items of a row or column field, computed from its other items
    /// (`=North+South`). They sort after the real items and stay out of the grand totals.
    pub calculated_items: Vec<PivotCalcItem>,
    /// Label filters on row or column fields (at most one per field).
    pub label_filters: Vec<PivotLabelFilter>,
    /// Value filters on row or column fields (at most one per field).
    pub value_filters: Vec<PivotValueFilter>,
    /// The Σ Values pseudo-field (it exists with two or more value fields) sits in Rows rather
    /// than Columns (Excel's "values on rows").
    pub values_on_rows: bool,
    /// Position of Σ Values among the row fields (`None` = after the last). Only used with
    /// `values_on_rows`: in Columns Σ Values always comes after the last column field.
    pub values_position: Option<usize>,
    /// Excel's "PivotTable Analyze ▸ Options ▸ Generate GetPivotData": while a formula is being
    /// typed, clicking a value of this report inserts a `GETPIVOTDATA` call naming the cell's
    /// items instead of a plain reference. On by default, as in Excel. It is remembered in our own
    /// document (the JSON format) and in the in-memory model; the XLSX writer has no slot for it,
    /// so a `.xlsx` save and reopen brings back the default.
    pub generate_get_pivot_data: bool,
}

/// A calculated field: a formula over other fields (`=Sales*0.1`, `='Unit Price'*Units`). As in
/// Excel, it is evaluated on the sums of the fields it names for each report cell, not per record.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PivotCalcField {
    pub name: String,
    /// Formula text, starting with `=`.
    pub formula: String,
}

/// A calculated item: an extra item of a row or column field, computed from other items of the
/// same field (`=North+South`). It is shown after the real items and, as Excel does, left out of
/// the grand totals and of record-level filters.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PivotCalcItem {
    /// The row or column field the item belongs to (its source column name).
    pub field: String,
    /// The item's caption, the way the report shows it.
    pub name: String,
    /// Formula text in the dialog form (`=North+South`); items of other fields are not allowed.
    pub formula: String,
}

impl Default for PivotTable {
    fn default() -> Self {
        PivotTable {
            id: 0,
            name: "PivotTable1".into(),
            source: String::new(),
            anchor: CellRef::default(),
            rows: vec![],
            columns: vec![],
            values: vec![],
            filters: vec![],
            layout: PivotLayout::Compact,
            grand_totals_rows: true,
            grand_totals_cols: true,
            subtotals: true,
            subtotals_top: true,
            style: "PivotStyleLight16".into(),
            show_headers: true,
            last_range: None,
            calculated_fields: vec![],
            calculated_items: vec![],
            label_filters: vec![],
            value_filters: vec![],
            values_on_rows: false,
            values_position: None,
            generate_get_pivot_data: true,
        }
    }
}

#[cfg(test)]
mod pivot_group_tests {
    use super::PivotNumberGroup;

    fn group(start: f64, end: f64, interval: f64) -> PivotNumberGroup {
        PivotNumberGroup { start, end, interval }
    }

    #[test]
    fn a_whole_step_names_the_last_value_it_holds() {
        let g = group(1.0, 100.0, 10.0);
        assert!(g.is_valid());
        assert_eq!(g.groups(), Some(10));
        assert_eq!(g.bucket(1.0).map(|x| x.0), Some("1-10".to_string()));
        assert_eq!(g.bucket(10.0).map(|x| x.0), Some("1-10".to_string()));
        assert_eq!(g.bucket(11.0).map(|x| x.0), Some("11-20".to_string()));
        assert_eq!(g.bucket(100.0).map(|x| x.0), Some("91-100".to_string()));
        assert_eq!(g.captions().first(), Some(&"<1".to_string()));
        // Excel names the upper tail after the next boundary (1 + 10 x 10 = 101), not after `end`.
        assert_eq!(g.captions().last(), Some(&">101".to_string()));
        assert_eq!(g.captions().len(), 12);
    }

    #[test]
    fn values_outside_the_range_are_their_own_groups() {
        let g = group(0.0, 100.0, 10.0);
        let (low, low_key) = g.bucket(-5.0).unwrap_or_default();
        assert_eq!(low, "<0");
        assert!(low_key < 0.0);
        let (high, high_key) = g.bucket(1000.0).unwrap_or_default();
        assert_eq!(high, ">100");
        assert!(high_key > 0.0);
    }

    #[test]
    fn a_fractional_step_shows_the_half_open_end() {
        let g = group(0.0, 2.0, 0.5);
        assert_eq!(g.bucket(0.25).map(|x| x.0), Some("0-0.5".to_string()));
        // The end itself sits on the next boundary, so it is the upper tail (Excel's `>2`).
        assert_eq!(g.bucket(2.0).map(|x| x.0), Some(">2".to_string()));
    }

    #[test]
    fn hostile_bounds_are_refused_not_allocated() {
        assert!(!group(0.0, 1.0, 0.0).is_valid(), "zero interval");
        assert!(!group(10.0, 0.0, 1.0).is_valid(), "end before start");
        assert!(!group(f64::NAN, 1.0, 1.0).is_valid(), "NaN");
        assert!(!group(0.0, 1e9, 0.001).is_valid(), "too many groups");
        assert!(group(0.0, 9_999.0, 1.0).is_valid());
        assert_eq!(group(0.0, 1e9, 0.001).groups(), None);
    }
}

#[cfg(test)]
mod pivot_filter_tests {
    use super::*;

    #[test]
    fn a_label_filter_compares_case_insensitively() {
        let f = |op, value: &str, value2: Option<&str>| PivotLabelFilter {
            source_col: "Region".into(),
            op,
            value: value.into(),
            value2: value2.map(str::to_string),
        };
        assert!(f(PivotLabelOp::Equals, "east", None).passes("East"));
        assert!(!f(PivotLabelOp::NotEquals, "east", None).passes("East"));
        assert!(f(PivotLabelOp::BeginsWith, "ea", None).passes("East"));
        assert!(f(PivotLabelOp::EndsWith, "ST", None).passes("East"));
        assert!(f(PivotLabelOp::Contains, "as", None).passes("East"));
        assert!(f(PivotLabelOp::DoesNotContain, "we", None).passes("East"));
        assert!(f(PivotLabelOp::Between, "a", Some("f")).passes("East"));
        assert!(!f(PivotLabelOp::Between, "a", Some("d")).passes("East"));
        assert!(f(PivotLabelOp::NotBetween, "a", Some("d")).passes("East"));
        assert!(f(PivotLabelOp::GreaterThan, "east", None).passes("West"));
    }

    #[test]
    fn a_value_filter_compares_the_aggregate() {
        let f =
            |op, value: f64, value2: Option<f64>| PivotValueFilter { source_col: "Region".into(), measure: "Sum of Sales".into(), op, value, value2 };
        assert!(f(PivotValueOp::GreaterThan, 100.0, None).passes(250.0));
        assert!(!f(PivotValueOp::GreaterThan, 250.0, None).passes(250.0));
        assert!(f(PivotValueOp::GreaterThanOrEqual, 250.0, None).passes(250.0));
        assert!(f(PivotValueOp::Equals, 250.0, None).passes(250.0));
        assert!(f(PivotValueOp::NotEquals, 0.0, None).passes(250.0));
        assert!(f(PivotValueOp::Between, 200.0, Some(300.0)).passes(250.0));
        assert!(!f(PivotValueOp::NotBetween, 200.0, Some(300.0)).passes(250.0));
        assert!(f(PivotValueOp::LessThan, 300.0, None).passes(250.0));
        assert!(f(PivotValueOp::LessThanOrEqual, 250.0, None).passes(250.0));
    }
}

#[cfg(test)]
mod trendline_forecast_tests {
    use super::Trendline;

    #[test]
    fn clamp_forecast_bounds_hostile_values() {
        assert_eq!(Trendline::clamp_forecast(2.5), 2.5);
        assert_eq!(Trendline::clamp_forecast(0.0), 0.0);
        assert_eq!(Trendline::clamp_forecast(-4.0), 0.0);
        assert_eq!(Trendline::clamp_forecast(f64::NAN), 0.0);
        assert_eq!(Trendline::clamp_forecast(f64::INFINITY), 0.0);
        assert_eq!(Trendline::clamp_forecast(f64::NEG_INFINITY), 0.0);
        assert_eq!(Trendline::clamp_forecast(1e300), Trendline::MAX_FORECAST);
    }
}
