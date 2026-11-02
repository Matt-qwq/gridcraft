//! DrawingML charts (`xl/charts/chartN.xml`): reading the common 2-D/3-D chart groups into the
//! model, and writing model charts (with cached series values) from scratch.

use std::fmt::Write as _;

use gridcraft_core::{RangeRef, Value, category_levels};
use gridcraft_formula::{Expr, SheetSel};
use gridcraft_model::{
    AxisCrossing, AxisOptions, Chart, ChartKind, Color, DefinedName, DisplayUnits, ErrorBarDirection, ErrorBarKind, ErrorBars, LabelFont,
    LabelOptions, LabelPosition, LegendPos, LineDash, PointFormat, PointLabel, Series, Trendline, TrendlineKind, Workbook,
};

use crate::drawing::{dml_color, srgb};
use crate::xml::{El, esc, esc_attr, num};

// ---------------------------------------------------------------- reading

fn group_kind(g: &El) -> Option<ChartKind> {
    let grouping = g.child_val("grouping").unwrap_or("standard");
    let stacked = grouping == "stacked";
    let pct = grouping == "percentStacked";
    Some(match g.name.as_str() {
        "barChart" | "bar3DChart" => {
            let bar = g.child_val("barDir") == Some("bar");
            match (bar, stacked, pct) {
                (false, true, _) => ChartKind::ColumnStacked,
                (false, _, true) => ChartKind::ColumnStacked100,
                (false, _, _) => ChartKind::ColumnClustered,
                (true, true, _) => ChartKind::BarStacked,
                (true, _, true) => ChartKind::BarStacked100,
                (true, _, _) => ChartKind::BarClustered,
            }
        }
        "lineChart" | "line3DChart" => {
            if stacked || pct {
                ChartKind::LineStacked
            } else {
                let group_markers = g.child_val("marker").is_none_or(|v| v == "1" || v == "true");
                let all_none = g.kids("ser").all(|s| s.path(&["marker", "symbol"]).and_then(|m| m.attr("val")) == Some("none"));
                if !group_markers || (all_none && g.kids("ser").next().is_some()) { ChartKind::Line } else { ChartKind::LineMarkers }
            }
        }
        "pieChart" | "pie3DChart" | "ofPieChart" => ChartKind::Pie,
        "doughnutChart" => ChartKind::Doughnut,
        "areaChart" | "area3DChart" => {
            if stacked || pct {
                ChartKind::AreaStacked
            } else {
                ChartKind::Area
            }
        }
        "scatterChart" => {
            let no_lines = g.kids("ser").all(|s| s.path(&["spPr", "ln"]).is_some_and(|l| l.child("noFill").is_some()));
            if g.child_val("scatterStyle") == Some("marker") || (no_lines && g.kids("ser").next().is_some()) {
                ChartKind::Scatter
            } else {
                ChartKind::ScatterLines
            }
        }
        "bubbleChart" => ChartKind::Bubble,
        "radarChart" => ChartKind::Radar,
        "stockChart" => ChartKind::Stock,
        _ => return None,
    })
}

/// Excel's file notation for "this workbook" in front of a workbook-scoped defined name.
const THIS_BOOK: &str = "[0]!";

/// A chart reference (`<c:f>`) read from a file → model text: drops a leading `=` and the
/// `[0]!` in front of workbook-scoped names (`[0]!Sales` → `Sales`), which the formula parser
/// would take for a structured reference. External books (`[1]Sheet1!$A$1`), quoted sheet
/// names and string literals are kept as they are.
pub(crate) fn chart_ref_from_file(text: &str) -> String {
    let t = text.trim();
    let t = t.strip_prefix('=').unwrap_or(t);
    let mut out = String::with_capacity(t.len());
    let mut quote: Option<char> = None;
    // The last character kept: `[0]!` only counts at the start or after an operator,
    // never inside a structured reference (`T[[0]]`) or a longer name.
    let mut prev: Option<char> = None;
    let mut i = 0;
    while let Some(c) = t.get(i..).and_then(|r| r.chars().next()) {
        let rest = t.get(i..).unwrap_or("");
        if quote.is_none()
            && rest.starts_with(THIS_BOOK)
            && prev.is_none_or(|p| !(p.is_alphanumeric() || "_.\\[]'\"!$#".contains(p)))
            && rest.get(THIS_BOOK.len()..).and_then(|n| n.chars().next()).is_some_and(|n| n.is_alphabetic() || n == '_' || n == '\\')
        {
            i += THIS_BOOK.len();
            continue;
        }
        match (c, quote) {
            ('"' | '\'', None) => quote = Some(c),
            (q, Some(open)) if q == open => quote = None,
            _ => {}
        }
        out.push(c);
        prev = Some(c);
        i += c.len_utf8();
    }
    out
}

fn data_ref(e: Option<&El>) -> Option<String> {
    let e = e?;
    for k in ["numRef", "strRef", "multiLvlStrRef"] {
        if let Some(f) = e.child(k).and_then(|r| r.child("f")) {
            return Some(chart_ref_from_file(&f.text));
        }
    }
    // Literal values become an array constant.
    for k in ["numLit", "strLit"] {
        if let Some(l) = e.child(k) {
            let pts: Vec<String> = l
                .kids("pt")
                .filter_map(|p| p.child("v"))
                .map(|v| if k == "numLit" { v.text.trim().to_string() } else { format!("\"{}\"", v.text.replace('"', "\"\"")) })
                .collect();
            return Some(format!("{{{}}}", pts.join(",")));
        }
    }
    None
}

fn read_series(s: &El) -> Series {
    let name = s
        .child("tx")
        .and_then(|tx| tx.path(&["strRef", "f"]).map(|f| chart_ref_from_file(&f.text)).or_else(|| tx.child("v").map(|v| v.text.clone())));
    let sp = s.child("spPr");
    let color = sp
        .and_then(|p| p.child("solidFill"))
        .and_then(dml_color)
        .or_else(|| sp.and_then(|p| p.path(&["ln", "solidFill"])).and_then(dml_color))
        .or_else(|| s.path(&["marker", "spPr", "solidFill"]).and_then(dml_color));
    let mut points: Vec<PointFormat> = s.kids("dPt").take(MAX_POINTS).filter_map(read_point).collect();
    let mut point_labels: Vec<PointLabel> = Vec::new();
    // A per-point label stored inside its `dPt` (`c:dPt/c:dLbl`).
    for dpt in s.kids("dPt").take(MAX_POINTS) {
        let (Some(idx), Some(dl)) = (dpt.child_val("idx").and_then(|v| v.trim().parse::<u32>().ok()), dpt.child("dLbl")) else { continue };
        let label = read_dlbl(dl, idx);
        if !label.is_empty() {
            set_label(&mut point_labels, label);
        }
    }
    // Per-point labels live in the series' `<c:dLbls>`: Excel keeps deleted labels and custom
    // label text there.
    for dl in s.path(&["dLbls"]).into_iter().flat_map(|d| d.kids("dLbl")).take(MAX_POINTS) {
        let Some(idx) = dl.child_val("idx").and_then(|v| v.trim().parse::<u32>().ok()) else { continue };
        if bool_val(dl, "delete") {
            match points.iter_mut().find(|p| p.index == idx) {
                Some(p) => p.no_label = true,
                None => points.push(PointFormat { index: idx, no_label: true, ..PointFormat::default() }),
            }
        }
        let label = read_dlbl(dl, idx);
        if !label.is_empty() {
            set_label(&mut point_labels, label);
        }
    }
    points.sort_by_key(|p| p.index);
    point_labels.sort_by_key(|l| l.index);
    Series {
        name,
        categories: data_ref(s.child("cat")).or_else(|| data_ref(s.child("xVal"))),
        values: data_ref(s.child("val")).or_else(|| data_ref(s.child("yVal"))).unwrap_or_default(),
        bubble_sizes: data_ref(s.child("bubbleSize")),
        color,
        secondary: false,
        kind: None,
        quartiles: Default::default(),
        smooth: bool_val(s, "smooth"),
        trendlines: s.kids("trendline").filter_map(read_trendline).collect(),
        // A `dLbls` that only deletes points carries no options; keep `None` so an empty record
        // does not appear out of nowhere on the next save.
        labels: s.child("dLbls").map(read_labels).filter(|l| {
            l.hidden || l.shows_anything() || l.position != gridcraft_model::LabelPosition::Auto || l.separator.is_some() || l.number_format.is_some()
        }),
        error_bars: s.child("errBars").and_then(read_error_bars),
        points,
        point_labels,
    }
}

/// Stores a per-point label, replacing an entry for the same point (a file may split a point's
/// label over its `dPt` and its series `dLbls`).
fn set_label(labels: &mut Vec<PointLabel>, label: PointLabel) {
    match labels.iter_mut().find(|l| l.index == label.index) {
        Some(slot) => *slot = label,
        None => labels.push(label),
    }
}

/// Longest per-point label text read from a file or written back (characters).
const MAX_LABEL_TEXT: usize = 1024;

/// A `c:dLbl`'s text: rich-text runs, or a reference to the cell it takes its text from.
fn read_label_text(dl: &El) -> Option<String> {
    let tx = dl.child("tx")?;
    if let Some(rich) = tx.child("rich") {
        let mut s = String::new();
        for (p, para) in rich.kids("p").take(64).enumerate() {
            if p > 0 {
                s.push('\n');
            }
            for t in para.kids("r").filter_map(|r| r.child("t")).take(1024) {
                let room = MAX_LABEL_TEXT.saturating_sub(s.chars().count());
                if room == 0 {
                    break;
                }
                s.extend(t.text.chars().take(room));
            }
        }
        return Some(s).filter(|s| !s.is_empty());
    }
    if let Some(f) = tx.path(&["strRef", "f"]) {
        let text: String = f.text.trim().chars().take(MAX_LABEL_TEXT).collect();
        return Some(chart_ref_from_file(&text)).filter(|s| !s.is_empty());
    }
    // A reference without one (a cache only): show what the file cached.
    tx.path(&["strRef", "strCache", "pt", "v"]).map(|v| v.text.chars().take(MAX_LABEL_TEXT).collect())
}

/// A `c:dLbl` as the model's per-point label: its text, position and show flags.
fn read_dlbl(dl: &El, index: u32) -> PointLabel {
    PointLabel {
        index,
        text: read_label_text(dl),
        show_value: dl.child("showVal").map(|c| c.flag("val", true)),
        show_series: dl.child("showSerName").map(|c| c.flag("val", true)),
        show_category: dl.child("showCatName").map(|c| c.flag("val", true)),
        show_percentage: dl.child("showPercent").map(|c| c.flag("val", true)),
        position: dl.child_val("dLblPos").and_then(label_position).unwrap_or_default(),
        font: read_label_font(dl),
    }
}

/// `c:dLblPos` → [`LabelPosition`].
pub(crate) fn label_position(v: &str) -> Option<LabelPosition> {
    Some(match v {
        "ctr" => LabelPosition::Center,
        "inEnd" => LabelPosition::InsideEnd,
        "inBase" => LabelPosition::InsideBase,
        "outEnd" => LabelPosition::OutsideEnd,
        "bestFit" => LabelPosition::BestFit,
        "l" => LabelPosition::Left,
        "r" => LabelPosition::Right,
        "t" => LabelPosition::Top,
        "b" => LabelPosition::Bottom,
        _ => return None,
    })
}

pub(crate) fn label_position_file(p: LabelPosition) -> &'static str {
    match p {
        LabelPosition::Auto | LabelPosition::Center => "ctr",
        LabelPosition::InsideEnd => "inEnd",
        LabelPosition::InsideBase => "inBase",
        LabelPosition::OutsideEnd => "outEnd",
        LabelPosition::BestFit => "bestFit",
        LabelPosition::Left => "l",
        LabelPosition::Right => "r",
        LabelPosition::Top => "t",
        LabelPosition::Bottom => "b",
    }
}

/// A label's own text formatting (`c:txPr/a:p/a:pPr/a:defRPr`): size, bold and colour. A label
/// without one follows the chart's size and the contrast colour of what it sits on.
pub(crate) fn read_label_font(e: &El) -> Option<LabelFont> {
    let def = e.path(&["txPr", "p", "pPr", "defRPr"])?;
    let font = LabelFont {
        // `sz` is hundredths of a point; a hostile or absurd value is not a size.
        size: def.attr("sz").and_then(|v| v.trim().parse::<f32>().ok()).map(|v| v / 100.0).filter(|v| (1.0..=400.0).contains(v)),
        bold: def.attr_bool("b"),
        color: def.child("solidFill").and_then(dml_color),
    };
    font.is_set().then_some(font)
}

/// A `CT_DLbls`: what the labels show, where and with which separator.
fn read_labels(d: &El) -> LabelOptions {
    LabelOptions {
        font: read_label_font(d),
        show_value: bool_val(d, "showVal"),
        show_percentage: bool_val(d, "showPercent"),
        show_series: bool_val(d, "showSerName"),
        show_category: bool_val(d, "showCatName"),
        show_legend_key: bool_val(d, "showLegendKey"),
        position: d.child_val("dLblPos").and_then(label_position).unwrap_or_default(),
        separator: d.child("separator").map(|s| s.text.clone()).filter(|s| !s.is_empty()),
        number_format: d.child("numFmt").and_then(|n| n.attr("formatCode")).map(str::to_string).filter(|c| !c.is_empty()),
        hidden: bool_val(d, "delete"),
    }
}

/// A `CT_ErrBars`. Unknown error types are read as the standard error rather than dropped.
fn read_error_bars(e: &El) -> Option<ErrorBars> {
    let direction = match e.child_val("errBarType") {
        Some("plus") => ErrorBarDirection::Plus,
        Some("minus") => ErrorBarDirection::Minus,
        _ => ErrorBarDirection::Both,
    };
    let kind = match e.child_val("errValType") {
        Some("fixedVal") => ErrorBarKind::Fixed(f64_val(e, "val").unwrap_or(0.0)),
        Some("percentage") => ErrorBarKind::Percentage(f64_val(e, "val").unwrap_or(0.0)),
        Some("stdDev") => ErrorBarKind::StdDev(f64_val(e, "val").unwrap_or(1.0)),
        Some("cust") => ErrorBarKind::Custom { plus: data_ref(e.child("plus")), minus: data_ref(e.child("minus")) },
        _ => ErrorBarKind::StdErr,
    };
    Some(ErrorBars { direction, kind, no_end_cap: bool_val(e, "noEndCap") })
}

/// A `CT_DPt`: the fill, line and marker colour of one point.
fn read_point(e: &El) -> Option<PointFormat> {
    let index = e.child_val("idx")?.trim().parse::<u32>().ok()?;
    let sp = e.child("spPr");
    Some(PointFormat {
        index,
        fill: sp.and_then(|p| p.child("solidFill")).and_then(dml_color),
        line: sp.and_then(|p| p.path(&["ln", "solidFill"])).and_then(dml_color),
        marker: e.path(&["marker", "spPr", "solidFill"]).and_then(dml_color),
        marker_size: e
            .path(&["marker", "size"])
            .and_then(|n| n.attr("val"))
            .and_then(|v| v.trim().parse::<u8>().ok())
            .filter(|n| (2..=72).contains(n)),
        no_label: e.child("dLbl").is_some_and(|d| bool_val(d, "delete")),
    })
}

const DASHES: [(LineDash, &str); 11] = [
    (LineDash::Solid, "solid"),
    (LineDash::Dot, "dot"),
    (LineDash::Dash, "dash"),
    (LineDash::LgDash, "lgDash"),
    (LineDash::DashDot, "dashDot"),
    (LineDash::LgDashDot, "lgDashDot"),
    (LineDash::LgDashDotDot, "lgDashDotDot"),
    (LineDash::SysDash, "sysDash"),
    (LineDash::SysDot, "sysDot"),
    (LineDash::SysDashDot, "sysDashDot"),
    (LineDash::SysDashDotDot, "sysDashDotDot"),
];

const EMU_PER_PT: f64 = 12_700.0;

/// A `CT_Boolean` child (`val` defaults to true).
fn bool_val(e: &El, tag: &str) -> bool {
    e.child(tag).is_some_and(|c| c.flag("val", true))
}

fn f64_val(e: &El, tag: &str) -> Option<f64> {
    e.child(tag).and_then(|c| c.attr_f64("val"))
}

/// `<c:trendline>` (ECMA-376 Part 1, 21.2.2.211). Unknown types are dropped.
fn read_trendline(t: &El) -> Option<Trendline> {
    let kind = match t.child_val("trendlineType").unwrap_or("linear") {
        "linear" => TrendlineKind::Linear,
        "exp" => TrendlineKind::Exponential,
        "log" => TrendlineKind::Logarithmic,
        "power" => TrendlineKind::Power,
        "poly" => TrendlineKind::Polynomial(t.child_val("order").and_then(|v| v.trim().parse::<u8>().ok()).unwrap_or(2).clamp(2, 6)),
        "movingAvg" => TrendlineKind::MovingAverage(t.child_val("period").and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(2).clamp(2, 255)),
        _ => return None,
    };
    let ln = t.path(&["spPr", "ln"]);
    Some(Trendline {
        kind,
        name: t.child("name").map(|n| n.text.clone()),
        forward: Trendline::clamp_forecast(f64_val(t, "forward").unwrap_or(0.0)),
        backward: Trendline::clamp_forecast(f64_val(t, "backward").unwrap_or(0.0)),
        intercept: f64_val(t, "intercept"),
        display_equation: bool_val(t, "dispEq"),
        display_r_squared: bool_val(t, "dispRSqr"),
        color: ln.and_then(|l| l.child("solidFill")).and_then(dml_color),
        width: ln.and_then(|l| l.attr_f64("w")).map(|w| (w / EMU_PER_PT) as f32).filter(|w| w.is_finite() && *w >= 0.0),
        dash: ln.and_then(|l| l.child("prstDash")).and_then(|d| d.attr("val")).and_then(|v| DASHES.iter().find(|(_, n)| *n == v).map(|(d, _)| *d)),
    })
}

fn rich_title(t: &El) -> Option<String> {
    let tx = t.child("tx")?;
    if let Some(rich) = tx.child("rich") {
        let paras: Vec<String> =
            rich.kids("p").map(|p| p.kids("r").filter_map(|r| r.child("t")).map(|t| t.text.as_str()).collect::<String>()).collect();
        let s = paras.join("\n");
        return if s.is_empty() { None } else { Some(s) };
    }
    if let Some(sr) = tx.child("strRef") {
        if let Some(v) = sr.path(&["strCache", "pt", "v"]) {
            return Some(v.text.clone());
        }
        return sr.child("f").map(|f| f.text.trim().to_string());
    }
    None
}

fn axis_ids(g: &El) -> Vec<String> {
    g.kids("axId").filter_map(|a| a.attr("val")).map(str::to_string).collect()
}

/// The `<c:valAx>`, `<c:catAx>` or `<c:dateAx>` with relationship id `id`.
fn axis_by_id<'a>(plot: &'a El, id: &str) -> Option<&'a El> {
    plot.children.iter().find(|e| matches!(e.name.as_str(), "valAx" | "catAx" | "dateAx") && e.child_val("axId") == Some(id))
}

fn display_units(v: &str) -> Option<DisplayUnits> {
    Some(match v {
        "hundreds" => DisplayUnits::Hundreds,
        "thousands" => DisplayUnits::Thousands,
        "tenThousands" => DisplayUnits::TenThousands,
        "hundredThousands" => DisplayUnits::HundredThousands,
        "millions" => DisplayUnits::Millions,
        "tenMillions" => DisplayUnits::TenMillions,
        "hundredMillions" => DisplayUnits::HundredMillions,
        "billions" => DisplayUnits::Billions,
        "trillions" => DisplayUnits::Trillions,
        _ => return None,
    })
}

/// Axis scaling, crossing and labels from a `<c:valAx>`, `<c:catAx>` or `<c:dateAx>`.
fn read_axis(a: &El) -> AxisOptions {
    let scaling = a.child("scaling");
    let bound = |tag: &str| scaling.and_then(|s| s.child(tag)).and_then(|e| e.attr_f64("val"));
    let crosses = match a.child_val("crosses") {
        Some("min") => AxisCrossing::Min,
        Some("max") => AxisCrossing::Max,
        _ => match a.child("crossesAt").and_then(|e| e.attr_f64("val")) {
            Some(v) => AxisCrossing::At(v),
            None => AxisCrossing::AutoZero,
        },
    };
    // A category axis carries the interval between labels as `tickLblSkip`.
    let cat = matches!(a.name.as_str(), "catAx" | "dateAx");
    let skip = a.child_val("tickLblSkip").and_then(|v| v.trim().parse::<f64>().ok());
    AxisOptions {
        min: bound("min"),
        max: bound("max"),
        major_unit: a.child("majorUnit").and_then(|e| e.attr_f64("val")).or(if cat { skip } else { None }),
        log_base: scaling.and_then(|s| s.child("logBase")).and_then(|e| e.attr_f64("val")).filter(|b| (2.0..=1000.0).contains(b)),
        reversed: scaling.and_then(|s| s.child("orientation")).and_then(|o| o.attr("val")) == Some("maxMin"),
        crosses,
        display_units: a.path(&["dispUnits", "builtInUnit"]).and_then(|e| e.attr("val")).and_then(display_units),
        // `sourceLinked` means "linked to source": the format follows the cells, which the model
        // spells as no explicit format.
        number_format: a
            .child("numFmt")
            .filter(|n| !n.flag("sourceLinked", false))
            .and_then(|n| n.attr("formatCode"))
            .map(str::to_string)
            .filter(|c| !c.is_empty()),
    }
}

/// Whether every chart group in a classic part is a kind GridCraft models. A part with any other
/// group (3-D bars, lines, areas or pies, a surface, a pie of pie or bar of pie) is kept as it was
/// instead of being read into the 2-D model and written back as something else.
pub fn is_modelled(root: &El) -> bool {
    let Some(plot) = root.path(&["chart", "plotArea"]) else { return false };
    let mut groups = 0usize;
    for g in &plot.children {
        if !g.name.ends_with("Chart") {
            continue;
        }
        groups += 1;
        if !matches!(
            g.name.as_str(),
            "barChart" | "lineChart" | "pieChart" | "doughnutChart" | "areaChart" | "scatterChart" | "bubbleChart" | "radarChart" | "stockChart"
        ) {
            return false;
        }
    }
    groups > 0
}

pub fn read_chart(root: &El) -> Option<Chart> {
    let chart = root.child("chart")?;
    let plot = chart.child("plotArea")?;
    let groups: Vec<(&El, ChartKind)> = plot.children.iter().filter_map(|g| group_kind(g).map(|k| (g, k))).collect();
    let (first, first_kind) = *groups.first()?;
    let first_axes = axis_ids(first);
    // A combo chart's series are split across plot groups, and the groups are written in drawing
    // order (bars, lines, secondary), not in the chart's series order. `c:order` (or `c:idx` on
    // older files) is the order the model and every series-indexed command use, so read it back and
    // sort; without this a save/reopen renamed which series a secondary-axis or point command hit.
    let mut series: Vec<(u32, Series)> = vec![];
    for (gi, (g, k)) in groups.iter().enumerate() {
        for (position, s) in g.kids("ser").enumerate() {
            let mut ser = read_series(s);
            if groups.len() > 1 {
                // Every series of a combo chart keeps its own group's kind, the first group's included.
                ser.kind = Some(*k);
            }
            if gi > 0 {
                ser.secondary = axis_ids(g) != first_axes;
            }
            let order = s.child_val("order").or_else(|| s.child_val("idx")).and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(position as u32);
            series.push((order, ser));
        }
    }
    series.sort_by_key(|(order, _)| *order);
    let mut series: Vec<Series> = series.into_iter().map(|(_, s)| s).collect();
    let kind = if groups.len() > 1 { ChartKind::Combo } else { first_kind };
    let group_labels = groups.iter().find_map(|(g, _)| g.child("dLbls")).map(read_labels);
    // Labels are on when any group or series shows any part of them.
    let shows = |d: &El| read_labels(d).shows_anything();
    let data_labels = groups.iter().any(|(g, _)| g.child("dLbls").is_some_and(shows) || g.kids("ser").any(|s| s.child("dLbls").is_some_and(shows)));
    // Labels only on some series: the chart-level record says "no group labels" so the next save
    // writes that back (with every unlabelled series hidden), instead of turning on value labels.
    let labels = match &group_labels {
        Some(l) => l.clone(),
        None if data_labels => LabelOptions { hidden: true, ..LabelOptions::default() },
        None => LabelOptions::default(),
    };
    // Series without a record of their own show no labels when the group shows none: Excel writes
    // their `<c:dLbls><c:delete val="1"/></c:dLbls>`, and the writer below does the same.
    if data_labels && !group_labels.as_ref().is_some_and(|l| l.shows_anything()) {
        for se in &mut series {
            if se.labels.is_none() {
                se.labels = Some(LabelOptions { hidden: true, ..LabelOptions::default() });
            }
        }
    }
    let scatter = matches!(first_kind, ChartKind::Scatter | ChartKind::ScatterLines | ChartKind::Bubble);
    // The secondary axis pair: the axes of the first group whose ids differ from the first group's
    // (a combo chart's right-hand value axis).
    let second_axes = groups.iter().skip(1).map(|(g, _)| axis_ids(g)).find(|ids| !ids.is_empty() && *ids != first_axes);
    let y2_ax = second_axes.as_ref().and_then(|ids| ids.get(1)).and_then(|id| axis_by_id(plot, id));
    let (x_ax, y_ax) = {
        let x = first_axes.first().and_then(|id| axis_by_id(plot, id));
        let y = first_axes.get(1).and_then(|id| axis_by_id(plot, id));
        if x.is_some() || y.is_some() {
            (x, y)
        } else {
            // Few files leave out the group's axis ids: fall back to the axes by position.
            let val_axes: Vec<&El> = plot.kids("valAx").collect();
            let cat_ax = plot.child("catAx").or_else(|| plot.child("dateAx"));
            if scatter {
                let x = val_axes.iter().find(|a| matches!(a.child_val("axPos"), Some("b") | Some("t"))).copied();
                let y = val_axes.iter().find(|a| matches!(a.child_val("axPos"), Some("l") | Some("r"))).copied();
                (x, y)
            } else {
                (cat_ax, val_axes.first().copied())
            }
        }
    };
    let legend = match chart.child("legend") {
        None => LegendPos::None,
        Some(l) => match l.child_val("legendPos").unwrap_or("r") {
            "b" => LegendPos::Bottom,
            "t" => LegendPos::Top,
            "l" => LegendPos::Left,
            _ => LegendPos::Right,
        },
    };
    Some(Chart {
        id: 0,
        kind,
        anchor: Default::default(),
        title: chart.child("title").and_then(rich_title),
        series,
        legend,
        data_labels,
        gridlines: plot.kids("valAx").any(|a| a.child("majorGridlines").is_some()),
        style: root.child_val("style").and_then(|v| v.parse().ok()).unwrap_or(2),
        x_title: x_ax.and_then(|a| a.child("title")).and_then(rich_title),
        y_title: y_ax.and_then(|a| a.child("title")).and_then(rich_title),
        source: None,
        by_rows: false,
        pivot: root.path(&["pivotSource", "name"]).map(|n| strip_book(n.text.trim()).to_string()).filter(|n| !n.is_empty()),
        x_axis: x_ax.map(read_axis).unwrap_or_default(),
        y_axis: y_ax.map(read_axis).unwrap_or_default(),
        y2_axis: y2_ax.map(read_axis).unwrap_or_default(),
        labels,
        part: None,
    })
}

// ---------------------------------------------------------------- writing

#[derive(Clone, Copy, PartialEq)]
enum G {
    Bar { bar: bool, grouping: &'static str },
    Line { stacked: bool, markers: bool },
    Pie,
    Doughnut,
    Area { stacked: bool },
    Scatter { lines: bool },
    Bubble,
    Radar,
    Stock,
}

fn group_for(k: ChartKind) -> G {
    match k {
        ChartKind::ColumnStacked => G::Bar { bar: false, grouping: "stacked" },
        ChartKind::ColumnStacked100 => G::Bar { bar: false, grouping: "percentStacked" },
        ChartKind::BarClustered => G::Bar { bar: true, grouping: "clustered" },
        ChartKind::BarStacked => G::Bar { bar: true, grouping: "stacked" },
        ChartKind::BarStacked100 => G::Bar { bar: true, grouping: "percentStacked" },
        ChartKind::Line => G::Line { stacked: false, markers: false },
        ChartKind::LineMarkers => G::Line { stacked: false, markers: true },
        ChartKind::LineStacked => G::Line { stacked: true, markers: false },
        ChartKind::Pie => G::Pie,
        ChartKind::Doughnut => G::Doughnut,
        ChartKind::Area => G::Area { stacked: false },
        ChartKind::AreaStacked => G::Area { stacked: true },
        ChartKind::Scatter => G::Scatter { lines: false },
        ChartKind::ScatterLines => G::Scatter { lines: true },
        ChartKind::Bubble => G::Bubble,
        ChartKind::Radar => G::Radar,
        ChartKind::Stock => G::Stock,
        // Combo charts start from clustered columns; chartex kinds have their own part.
        _ => G::Bar { bar: false, grouping: "clustered" },
    }
}

const MAX_CACHE: u64 = 100_000;

/// Most per-point formatting records written for one series (a file or a command may carry more
/// than the model caps; the rest are dropped rather than growing the part without bound).
pub(crate) const MAX_POINTS: usize = 4096;

/// The sheet and range a same-workbook reference (`Sheet1!$B$2:$B$9`) names.
fn ref_target(wb: &Workbook, sheet_idx: usize, f: &str) -> Option<(usize, RangeRef)> {
    let e = gridcraft_formula::parse(f).ok()?;
    let Expr::Ref(r) = e else { return None };
    let si = match &r.sheet {
        SheetSel::Current => sheet_idx,
        SheetSel::Named(n) => wb.sheet_index(n)?,
        SheetSel::Span(..) => return None,
    };
    Some((si, r.range()))
}

/// Values of a same-workbook reference (`Sheet1!$B$2:$B$9`), row-major.
pub(crate) fn ref_values(wb: &Workbook, sheet_idx: usize, f: &str) -> Option<Vec<Value>> {
    let (si, mut range) = ref_target(wb, sheet_idx, f)?;
    let sheet = wb.sheet(si)?;
    // Whole columns/rows: cut the far end at the used range. The start stays put, so point indices keep matching the reference.
    if let Some(u) = sheet.used_range() {
        if range.is_full_cols() {
            range.end.row = range.end.row.min(u.end.row.max(range.start.row));
        }
        if range.is_full_rows() {
            range.end.col = range.end.col.min(u.end.col.max(range.start.col));
        }
    }
    if range.count() > MAX_CACHE {
        return None;
    }
    Some(range.iter().map(|c| sheet.value(c)).collect())
}

fn is_ref(f: &str) -> bool {
    matches!(gridcraft_formula::parse(f), Ok(Expr::Ref(_)))
}

/// A chart reference that is a name: its sheet when qualified (`Sheet1!Sales`), the bare
/// name, and the definition formulas would find from sheet `si` (none when undefined). `None`
/// when it isn't a name, or names a sheet the workbook doesn't have.
fn chart_name<'a>(wb: &'a Workbook, si: usize, f: &str) -> Option<(Option<usize>, String, Option<&'a DefinedName>)> {
    let Ok(Expr::Name(n)) = gridcraft_formula::parse(f.trim()) else { return None };
    match n.split_once('!') {
        Some((s, bare)) => {
            let i = wb.sheet_index(s.trim_matches('\''))?;
            Some((Some(i), bare.to_string(), wb.name(bare, i)))
        }
        None => {
            let def = wb.name(&n, si);
            Some((None, n, def))
        }
    }
}

/// A chart reference for `<c:f>`. Excel needs a name qualified there: with a bare `Name` it
/// draws the whole chart blank, with no repair prompt (observed in Excel for Windows,
/// Microsoft 365). A workbook-scoped name is written `[0]!Name`, as Excel writes it, a
/// sheet-scoped one `Sheet!Name`; an undefined name gets `[0]!`, as the file it came from had.
fn chart_ref_to_file(wb: &Workbook, si: usize, f: &str) -> String {
    let on_sheet = |i: usize, n: &str| wb.sheet(i).map(|sh| format!("{}!{n}", gridcraft_formula::quote_sheet(&sh.name)));
    match chart_name(wb, si, f) {
        Some((Some(i), n, _)) => on_sheet(i, &n).unwrap_or_else(|| crate::fmla::text_to_file(f)),
        Some((None, n, def)) => def.and_then(|d| d.scope).and_then(|i| on_sheet(i, &n)).unwrap_or_else(|| format!("{THIS_BOOK}{n}")),
        None => crate::fmla::text_to_file(f),
    }
}

/// A series name that refers to a defined name (as opposed to literal text that parses as one).
fn is_defined_name(wb: &Workbook, si: usize, f: &str) -> bool {
    chart_name(wb, si, f).is_some_and(|(_, _, d)| d.is_some())
}

pub(crate) fn num_data(tag: &str, f: Option<&str>, wb: &Workbook, si: usize) -> String {
    let f = f.unwrap_or("").trim();
    if f.is_empty() {
        return format!("<c:{tag}><c:numLit><c:ptCount val=\"0\"/></c:numLit></c:{tag}>");
    }
    if f.starts_with('{') {
        let vals: Vec<&str> = f.trim_matches(|c| c == '{' || c == '}').split([',', ';']).collect();
        let mut s = format!("<c:{tag}><c:numLit><c:formatCode>General</c:formatCode><c:ptCount val=\"{}\"/>", vals.len());
        for (i, v) in vals.iter().enumerate() {
            if let Ok(n) = v.trim().parse::<f64>() {
                let _ = write!(s, "<c:pt idx=\"{i}\"><c:v>{}</c:v></c:pt>", num(n));
            }
        }
        let _ = write!(s, "</c:numLit></c:{tag}>");
        return s;
    }
    let mut s = format!("<c:{tag}><c:numRef><c:f>{}</c:f>", esc(&chart_ref_to_file(wb, si, f)));
    if let Some(vals) = ref_values(wb, si, f) {
        let _ = write!(s, "<c:numCache><c:formatCode>General</c:formatCode><c:ptCount val=\"{}\"/>", vals.len());
        for (i, v) in vals.iter().enumerate() {
            if let Value::Number(n) = v {
                let _ = write!(s, "<c:pt idx=\"{i}\"><c:v>{}</c:v></c:pt>", num(*n));
            }
        }
        s.push_str("</c:numCache>");
    }
    let _ = write!(s, "</c:numRef></c:{tag}>");
    s
}

/// Splits the inside of an array constant on `,` and `;` outside double quotes; inside quotes `""` is a literal quote.
/// Unquoted items are trimmed; quoted ones keep the text between the quotes and drop the spaces around them.
fn split_array_items(inner: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut in_quote = false;
    let mut chars = inner.chars().peekable();
    let finish = |cur: &mut String, quoted: &mut bool| {
        let text = std::mem::take(cur);
        let item = if *quoted { text } else { text.trim().to_string() };
        *quoted = false;
        item
    };
    while let Some(c) = chars.next() {
        match c {
            '"' if in_quote && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => {
                if !quoted && cur.trim().is_empty() {
                    cur.clear();
                }
                in_quote = !in_quote;
                quoted = true;
            }
            ',' | ';' if !in_quote => items.push(finish(&mut cur, &mut quoted)),
            c if quoted && !in_quote && c.is_whitespace() => {}
            _ => cur.push(c),
        }
    }
    items.push(finish(&mut cur, &mut quoted));
    items
}

pub(crate) fn str_data(tag: &str, f: &str, wb: &Workbook, si: usize) -> String {
    let f = f.trim();
    if f.starts_with('{') {
        let vals = split_array_items(f.trim_matches(|c| c == '{' || c == '}'));
        let mut s = format!("<c:{tag}><c:strLit><c:ptCount val=\"{}\"/>", vals.len());
        for (i, v) in vals.iter().enumerate() {
            let _ = write!(s, "<c:pt idx=\"{i}\"><c:v>{}</c:v></c:pt>", esc(v));
        }
        let _ = write!(s, "</c:strLit></c:{tag}>");
        return s;
    }
    let mut s = format!("<c:{tag}><c:strRef><c:f>{}</c:f>", esc(&chart_ref_to_file(wb, si, f)));
    if let Some(vals) = ref_values(wb, si, f) {
        let _ = write!(s, "<c:strCache><c:ptCount val=\"{}\"/>", vals.len());
        for (i, v) in vals.iter().enumerate() {
            if !v.is_empty() {
                let _ = write!(s, "<c:pt idx=\"{i}\"><c:v>{}</c:v></c:pt>", esc(&v.display()));
            }
        }
        s.push_str("</c:strCache>");
    }
    let _ = write!(s, "</c:strRef></c:{tag}>");
    s
}

/// `<c:cat>` of a series: a string reference, or a multi-level one (`multiLvlStrRef`) when the
/// category range has more columns than a column of values (or more rows than a row of values:
/// year beside month). Its cache lists the innermost level first, then each outer level with
/// only the cells that start a group, as Excel writes it.
pub(crate) fn cat_data(f: &str, values: &str, wb: &Workbook, si: usize) -> String {
    multi_level_cat(f.trim(), values, wb, si).unwrap_or_else(|| str_data("cat", f, wb, si))
}

fn multi_level_cat(f: &str, values: &str, wb: &Workbook, si: usize) -> Option<String> {
    let (sheet, range) = ref_target(wb, si, f)?;
    let value_range = ref_target(wb, si, values).map(|(_, r)| r);
    let levels = category_levels(&range, value_range.as_ref())?;
    if range.count() > MAX_CACHE {
        return None;
    }
    let sheet = wb.sheet(sheet)?;
    let (w, h) = (range.width() as usize, range.height() as usize);
    let text = |row: usize, col: usize| -> String {
        let v = range.start.offset(row as i64, col as i64).map(|c| sheet.value(c));
        v.filter(|v| !v.is_empty()).map(|v| v.display()).unwrap_or_default()
    };
    // The innermost level (nearest the values), then the outer ones.
    let (n, lvls): (usize, Vec<Vec<String>>) = if levels.by_columns {
        (h, (0..=levels.outer as usize).filter_map(|k| (w - 1).checked_sub(k)).map(|c| (0..h).map(|r| text(r, c)).collect()).collect())
    } else {
        (w, (0..=levels.outer as usize).filter_map(|k| (h - 1).checked_sub(k)).map(|r| (0..w).map(|c| text(r, c)).collect()).collect())
    };
    let mut s = format!("<c:cat><c:multiLvlStrRef><c:f>{}</c:f><c:multiLvlStrCache><c:ptCount val=\"{n}\"/>", esc(&crate::fmla::text_to_file(f)));
    for lvl in &lvls {
        s.push_str("<c:lvl>");
        for (i, v) in lvl.iter().enumerate() {
            if !v.is_empty() {
                let _ = write!(s, "<c:pt idx=\"{i}\"><c:v>{}</c:v></c:pt>", esc(v));
            }
        }
        s.push_str("</c:lvl>");
    }
    s.push_str("</c:multiLvlStrCache></c:multiLvlStrRef></c:cat>");
    Some(s)
}

pub(crate) fn series_tx_xml(s: &Series, wb: &Workbook, si: usize) -> String {
    let Some(n) = &s.name else { return String::new() };
    if is_ref(n) || is_defined_name(wb, si, n) {
        str_data("tx", n, wb, si)
    } else {
        let lit = n.trim().strip_prefix('"').and_then(|t| t.strip_suffix('"')).unwrap_or(n);
        format!("<c:tx><c:v>{}</c:v></c:tx>", esc(lit))
    }
}

/// A series' `spPr`: for a line-like series an outline in the series colour (line and radar charts),
/// for anything else a fill.
pub(crate) fn series_sp_pr(s: &Series, line_like: bool, wb: &Workbook) -> Option<String> {
    let c = s.color.as_ref().and_then(|c: &Color| srgb(c, &wb.theme))?;
    Some(if line_like {
        format!("<c:spPr><a:ln w=\"28575\" cap=\"rnd\"><a:solidFill>{c}</a:solidFill><a:round/></a:ln></c:spPr>")
    } else {
        format!("<c:spPr><a:solidFill>{c}</a:solidFill></c:spPr>")
    })
}

fn series_xml(s: &Series, idx: usize, g: G, labels_on: bool, wb: &Workbook, si: usize) -> String {
    let mut x = format!("<c:ser><c:idx val=\"{idx}\"/><c:order val=\"{idx}\"/>");
    x.push_str(&series_tx_xml(s, wb, si));
    let color = s.color.as_ref().and_then(|c: &Color| srgb(c, &wb.theme));
    let line_like = matches!(g, G::Line { .. } | G::Radar | G::Scatter { lines: true });
    match (&color, line_like) {
        (Some(c), true) => {
            let _ = write!(x, "<c:spPr><a:ln w=\"28575\" cap=\"rnd\"><a:solidFill>{c}</a:solidFill><a:round/></a:ln></c:spPr>");
        }
        (Some(c), false) if g == (G::Scatter { lines: false }) => {
            let _ = write!(
                x,
                "<c:spPr><a:ln w=\"19050\"><a:noFill/></a:ln></c:spPr><c:marker><c:symbol val=\"circle\"/><c:size val=\"5\"/><c:spPr><a:solidFill>{c}</a:solidFill></c:spPr></c:marker>"
            );
        }
        (Some(c), false) => {
            let _ = write!(x, "<c:spPr><a:solidFill>{c}</a:solidFill></c:spPr>");
        }
        (None, _) if g == (G::Scatter { lines: false }) || g == G::Stock => x.push_str("<c:spPr><a:ln w=\"19050\"><a:noFill/></a:ln></c:spPr>"),
        _ => {}
    }
    match g {
        G::Bar { .. } => x.push_str("<c:invertIfNegative val=\"0\"/>"),
        G::Line { markers: false, .. } | G::Scatter { lines: true } | G::Stock => x.push_str("<c:marker><c:symbol val=\"none\"/></c:marker>"),
        G::Bubble => x.push_str("<c:invertIfNegative val=\"0\"/>"),
        _ => {}
    }
    // CT_*Ser order after spPr and marker: dPt*, dLbls?, trendline*, errBars?, then the data.
    let marker_points = matches!(g, G::Line { .. } | G::Scatter { .. } | G::Bubble | G::Radar);
    for p in s.points.iter().take(MAX_POINTS) {
        x.push_str(&point_xml(p, marker_points, wb));
    }
    let no_label: Vec<u32> = s.points.iter().filter(|p| p.no_label).map(|p| p.index).collect();
    // Labels are a chart-wide switch: a series' own options are written only while they are on.
    let labels = s.labels.as_ref().filter(|_| labels_on);
    if labels.is_some() || !no_label.is_empty() || !s.point_labels.is_empty() {
        x.push_str(&dlbls_xml(labels, &no_label, &s.point_labels, wb, si));
    }
    if !matches!(g, G::Pie | G::Doughnut | G::Radar) {
        for t in &s.trendlines {
            x.push_str(&trendline_xml(t, wb));
        }
    }
    if let Some(e) = &s.error_bars
        && !matches!(g, G::Pie | G::Doughnut | G::Radar | G::Stock)
    {
        x.push_str(&err_bars_xml(e, wb, si));
    }
    match g {
        G::Scatter { .. } | G::Bubble => {
            if let Some(c) = &s.categories {
                x.push_str(&num_data("xVal", Some(c), wb, si));
            }
            x.push_str(&num_data("yVal", Some(&s.values), wb, si));
            if g == G::Bubble {
                x.push_str(&num_data("bubbleSize", s.bubble_sizes.as_deref().or(Some(&s.values)), wb, si));
                x.push_str("<c:bubble3D val=\"0\"/>");
            } else {
                let _ = write!(x, "<c:smooth val=\"{}\"/>", s.smooth as u8);
            }
        }
        _ => {
            if let Some(c) = &s.categories {
                x.push_str(&cat_data(c, &s.values, wb, si));
            }
            x.push_str(&num_data("val", Some(&s.values), wb, si));
            if matches!(g, G::Line { .. }) {
                let _ = write!(x, "<c:smooth val=\"{}\"/>", s.smooth as u8);
            }
        }
    }
    x.push_str("</c:ser>");
    x
}

/// `<c:trendline>` with children in schema order: name, spPr, trendlineType, order, period,
/// forward, backward, intercept, dispRSqr, dispEq, trendlineLbl.
pub(crate) fn trendline_xml(t: &Trendline, wb: &Workbook) -> String {
    let mut x = String::from("<c:trendline>");
    if let Some(n) = &t.name {
        let _ = write!(x, "<c:name>{}</c:name>", esc(n));
    }
    let color = t.color.as_ref().and_then(|c| srgb(c, &wb.theme));
    if color.is_some() || t.width.is_some() || t.dash.is_some() {
        x.push_str("<c:spPr><a:ln");
        if let Some(w) = t.width.filter(|w| w.is_finite()) {
            // ST_LineWidth: 0..=20116800 EMU.
            let _ = write!(x, " w=\"{}\"", (f64::from(w) * EMU_PER_PT).round().clamp(0.0, 20_116_800.0) as i64);
        }
        x.push_str(" cap=\"rnd\">");
        if let Some(c) = &color {
            let _ = write!(x, "<a:solidFill>{c}</a:solidFill>");
        }
        if let Some(d) = t.dash.and_then(|d| DASHES.iter().find(|(k, _)| *k == d)) {
            let _ = write!(x, "<a:prstDash val=\"{}\"/>", d.1);
        }
        x.push_str("</a:ln></c:spPr>");
    }
    let ty = match t.kind {
        TrendlineKind::Linear => "linear",
        TrendlineKind::Exponential => "exp",
        TrendlineKind::Logarithmic => "log",
        TrendlineKind::Polynomial(_) => "poly",
        TrendlineKind::Power => "power",
        TrendlineKind::MovingAverage(_) => "movingAvg",
    };
    let _ = write!(x, "<c:trendlineType val=\"{ty}\"/>");
    match t.kind {
        TrendlineKind::Polynomial(o) => {
            let _ = write!(x, "<c:order val=\"{}\"/>", o.clamp(2, 6));
        }
        TrendlineKind::MovingAverage(p) => {
            let _ = write!(x, "<c:period val=\"{}\"/>", p.clamp(2, 255));
        }
        _ => {}
    }
    for (tag, v) in [("forward", t.forward), ("backward", t.backward)] {
        if v.is_finite() && v > 0.0 {
            let _ = write!(x, "<c:{tag} val=\"{}\"/>", num(v));
        }
    }
    if let Some(i) = t.intercept.filter(|i| i.is_finite()) {
        let _ = write!(x, "<c:intercept val=\"{}\"/>", num(i));
    }
    let _ = write!(x, "<c:dispRSqr val=\"{}\"/><c:dispEq val=\"{}\"/>", t.display_r_squared as u8, t.display_equation as u8);
    if t.display_r_squared || t.display_equation {
        x.push_str("<c:trendlineLbl><c:numFmt formatCode=\"General\" sourceLinked=\"0\"/></c:trendlineLbl>");
    }
    x.push_str("</c:trendline>");
    x
}

/// `c:tx` of a point's data label: a reference to the cell it takes its text from, or literal
/// text as one rich-text run.
pub(crate) fn label_tx_xml(t: &str, wb: &Workbook, si: usize) -> String {
    let t = t.trim();
    let plain: String = t.chars().take(MAX_LABEL_TEXT).collect();
    if is_ref(t) || is_defined_name(wb, si, t) {
        let mut s = format!("<c:tx><c:strRef><c:f>{}</c:f>", esc(&chart_ref_to_file(wb, si, t)));
        if let Some(v) = ref_values(wb, si, t).and_then(|v| v.into_iter().next()) {
            let _ = write!(s, "<c:strCache><c:ptCount val=\"1\"/><c:pt idx=\"0\"><c:v>{}</c:v></c:pt></c:strCache>", esc(&v.display()));
        }
        s.push_str("</c:strRef></c:tx>");
        return s;
    }
    let lit = plain.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(&plain);
    format!("<c:tx><c:rich><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang=\"en-US\"/><a:t>{}</a:t></a:r></a:p></c:rich></c:tx>", esc(lit))
}

/// `<c:txPr>` for a label font, in CT_TextBody order: body properties, the paragraph and its
/// default run properties (size in hundredths of a point, bold, colour).
pub(crate) fn label_tx_pr_xml(f: &LabelFont, wb: &Workbook) -> String {
    let f = f.sane();
    let mut attrs = String::new();
    if let Some(size) = f.size {
        let _ = write!(attrs, " sz=\"{}\"", (size * 100.0).round().clamp(100.0, 40_000.0) as i64);
    }
    if let Some(b) = f.bold {
        let _ = write!(attrs, " b=\"{}\"", b as u8);
    }
    let fill = f.color.as_ref().and_then(|c| srgb(c, &wb.theme)).map(|c| format!("<a:solidFill>{c}</a:solidFill>")).unwrap_or_default();
    format!("<c:txPr><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr{attrs}>{fill}</a:defRPr></a:pPr><a:endParaRPr lang=\"en-US\"/></a:p></c:txPr>")
}

/// `<c:dLbl>` in CT_DLbl order: idx, delete, tx, dLblPos, the show flags.
pub(crate) fn dlbl_xml(index: u32, no_label: bool, label: Option<&PointLabel>, wb: &Workbook, si: usize) -> String {
    let index = index.min(MAX_POINTS as u32);
    let mut x = format!("<c:dLbl><c:idx val=\"{index}\"/>");
    if no_label {
        x.push_str("<c:delete val=\"1\"/>");
    } else if let Some(l) = label {
        if let Some(t) = l.text.as_deref().filter(|t| !t.trim().is_empty()) {
            x.push_str(&label_tx_xml(t, wb, si));
        }
        if let Some(f) = l.font.filter(|f| f.is_set()) {
            x.push_str(&label_tx_pr_xml(&f, wb));
        }
        if l.position != LabelPosition::Auto {
            let _ = write!(x, "<c:dLblPos val=\"{}\"/>", label_position_file(l.position));
        }
        for (tag, v) in
            [("showVal", l.show_value), ("showCatName", l.show_category), ("showSerName", l.show_series), ("showPercent", l.show_percentage)]
        {
            if let Some(v) = v {
                let _ = write!(x, "<c:{tag} val=\"{}\"/>", v as u8);
            }
        }
    }
    x.push_str("</c:dLbl>");
    x
}

/// `<c:dLbls>` in CT_DLbls order: per-point labels, then the options. `labels` absent writes the
/// per-point labels only (a series with no options of its own).
pub(crate) fn dlbls_xml(labels: Option<&LabelOptions>, deleted: &[u32], point_labels: &[PointLabel], wb: &Workbook, si: usize) -> String {
    let mut x = String::from("<c:dLbls>");
    // Deleted points and per-point labels share the `dLbl` elements: one per point, in index order.
    let deleted: std::collections::BTreeSet<u32> = deleted.iter().copied().collect();
    let mut indices: Vec<u32> = deleted.iter().copied().chain(point_labels.iter().map(|l| l.index)).collect();
    indices.sort_unstable();
    indices.dedup();
    indices.truncate(MAX_POINTS);
    for idx in indices {
        let no_label = deleted.contains(&idx);
        let label = point_labels.iter().find(|l| l.index == idx);
        x.push_str(&dlbl_xml(idx, no_label, label, wb, si));
    }
    if let Some(l) = labels {
        for (_, text) in label_children(l, wb) {
            if let Some(t) = text {
                x.push_str(&t);
            }
        }
    }
    x.push_str("</c:dLbls>");
    x
}

/// `<c:dPt>` in CT_DPt order: idx, marker?, spPr?.
pub(crate) fn point_xml(p: &PointFormat, marker: bool, wb: &Workbook) -> String {
    let mut x = format!("<c:dPt><c:idx val=\"{}\"/>", p.index.min(MAX_POINTS as u32));
    if marker && (p.marker.is_some() || p.marker_size.is_some()) {
        let size = p.marker_size.unwrap_or(5).clamp(2, 72);
        let sp = p
            .marker
            .as_ref()
            .and_then(|c: &Color| srgb(c, &wb.theme))
            .map(|c| format!("<c:spPr><a:solidFill>{c}</a:solidFill></c:spPr>"))
            .unwrap_or_default();
        let _ = write!(x, "<c:marker><c:symbol val=\"circle\"/><c:size val=\"{size}\"/>{sp}</c:marker>");
    }
    let fill = p.fill.as_ref().and_then(|c: &Color| srgb(c, &wb.theme));
    let line = p.line.as_ref().and_then(|c: &Color| srgb(c, &wb.theme));
    if fill.is_some() || line.is_some() {
        x.push_str("<c:spPr>");
        if let Some(c) = &fill {
            let _ = write!(x, "<a:solidFill>{c}</a:solidFill>");
        }
        if let Some(c) = &line {
            let _ = write!(x, "<a:ln w=\"12700\"><a:solidFill>{c}</a:solidFill></a:ln>");
        }
        x.push_str("</c:spPr>");
    }
    x.push_str("</c:dPt>");
    x
}

/// The children of a `<c:errBars>` in CT_ErrBars order: the type pair, end caps, the custom
/// references and the amount. The writer emits them all; the patcher changes a subset and leaves
/// the element's own formatting (`spPr`: the bar's line colour and width) alone.
pub(crate) fn err_bars_children(e: &ErrorBars, wb: &Workbook, si: usize) -> Vec<(&'static str, Option<String>)> {
    let direction = match e.direction {
        ErrorBarDirection::Both => "both",
        ErrorBarDirection::Plus => "plus",
        ErrorBarDirection::Minus => "minus",
    };
    let kind = match e.kind {
        ErrorBarKind::Fixed(_) => "fixedVal",
        ErrorBarKind::Percentage(_) => "percentage",
        ErrorBarKind::StdDev(_) => "stdDev",
        ErrorBarKind::Custom { .. } => "cust",
        ErrorBarKind::StdErr => "stdErr",
    };
    let (plus, minus, val) = match &e.kind {
        ErrorBarKind::Custom { plus, minus } => {
            (plus.as_deref().map(|f| num_data("plus", Some(f), wb, si)), minus.as_deref().map(|f| num_data("minus", Some(f), wb, si)), None)
        }
        ErrorBarKind::Fixed(v) | ErrorBarKind::Percentage(v) => (None, None, Some(format!("<c:val val=\"{}\"/>", num(*v)))),
        ErrorBarKind::StdDev(v) => (None, None, Some(format!("<c:val val=\"{}\"/>", num(v.max(0.0))))),
        ErrorBarKind::StdErr => (None, None, None),
    };
    vec![
        ("errBarType", Some(format!("<c:errBarType val=\"{direction}\"/>"))),
        ("errValType", Some(format!("<c:errValType val=\"{kind}\"/>"))),
        ("noEndCap", e.no_end_cap.then(|| "<c:noEndCap val=\"1\"/>".to_string())),
        ("plus", plus),
        ("minus", minus),
        ("val", val),
    ]
}

/// `<c:errBars>` in CT_ErrBars order: errBarType, errValType, noEndCap?, plus/minus?, val?.
pub(crate) fn err_bars_xml(e: &ErrorBars, wb: &Workbook, si: usize) -> String {
    let mut x = String::from("<c:errBars>");
    for (_, text) in err_bars_children(e, wb, si) {
        if let Some(t) = text {
            x.push_str(&t);
        }
    }
    let _ = write!(x, "</c:errBars>");
    x
}

fn group_xml(g: G, series: &[(usize, &Series)], ch: &Chart, axes: (u32, u32), wb: &Workbook, si: usize) -> String {
    let sers: String = series.iter().map(|(i, s)| series_xml(s, *i, g, ch.data_labels, wb, si)).collect();
    // Group-level labels: the chart's options when they show something, else the plain value label.
    // `labels.hidden` means the file had no group-level labels (only some series had them):
    // writing no group record keeps it that way.
    let dl = if ch.data_labels && !ch.labels.hidden { dlbls_xml(Some(&ch.labels), &[], &[], wb, si) } else { String::new() };
    let ax = format!("<c:axId val=\"{}\"/><c:axId val=\"{}\"/>", axes.0, axes.1);
    match g {
        G::Bar { bar, grouping } => {
            let overlap = if grouping == "clustered" { String::new() } else { "<c:overlap val=\"100\"/>".into() };
            format!(
                "<c:barChart><c:barDir val=\"{}\"/><c:grouping val=\"{grouping}\"/><c:varyColors val=\"0\"/>{sers}{dl}<c:gapWidth val=\"150\"/>{overlap}{ax}</c:barChart>",
                if bar { "bar" } else { "col" }
            )
        }
        G::Line { stacked, .. } => format!(
            "<c:lineChart><c:grouping val=\"{}\"/><c:varyColors val=\"0\"/>{sers}{dl}<c:marker val=\"1\"/>{ax}</c:lineChart>",
            if stacked { "stacked" } else { "standard" }
        ),
        G::Pie => format!("<c:pieChart><c:varyColors val=\"1\"/>{sers}{dl}<c:firstSliceAng val=\"0\"/></c:pieChart>"),
        G::Doughnut => {
            format!("<c:doughnutChart><c:varyColors val=\"1\"/>{sers}{dl}<c:firstSliceAng val=\"0\"/><c:holeSize val=\"50\"/></c:doughnutChart>")
        }
        G::Area { stacked } => format!(
            "<c:areaChart><c:grouping val=\"{}\"/><c:varyColors val=\"0\"/>{sers}{dl}{ax}</c:areaChart>",
            if stacked { "stacked" } else { "standard" }
        ),
        G::Scatter { .. } => format!("<c:scatterChart><c:scatterStyle val=\"lineMarker\"/><c:varyColors val=\"0\"/>{sers}{dl}{ax}</c:scatterChart>"),
        G::Bubble => {
            format!("<c:bubbleChart><c:varyColors val=\"0\"/>{sers}{dl}<c:bubbleScale val=\"100\"/><c:showNegBubbles val=\"0\"/>{ax}</c:bubbleChart>")
        }
        G::Radar => format!("<c:radarChart><c:radarStyle val=\"marker\"/><c:varyColors val=\"0\"/>{sers}{dl}{ax}</c:radarChart>"),
        G::Stock => format!("<c:stockChart>{sers}{dl}<c:hiLowLines/>{ax}</c:stockChart>"),
    }
}

pub(crate) fn title_xml(t: &str) -> String {
    let mut s = String::from("<c:title><c:tx><c:rich><a:bodyPr/><a:lstStyle/>");
    for line in t.split('\n') {
        let _ = write!(s, "<a:p><a:pPr><a:defRPr/></a:pPr><a:r><a:t>{}</a:t></a:r></a:p>", esc(line));
    }
    s.push_str("</c:rich></c:tx><c:overlay val=\"0\"/></c:title>");
    s
}

struct Ax<'a> {
    cat: bool,
    id: u32,
    cross: u32,
    pos: &'a str,
    delete: bool,
    grid: bool,
    title: Option<&'a str>,
    options: &'a AxisOptions,
    /// A category axis that plots numbers (a scatter chart's X axis) is written `valAx`.
    numeric_cat: bool,
}

/// CT_Scaling child order: logBase, orientation, max, min.
pub(crate) fn legend_pos_file(p: LegendPos) -> Option<&'static str> {
    match p {
        LegendPos::None => None,
        LegendPos::Bottom => Some("b"),
        LegendPos::Top => Some("t"),
        LegendPos::Left => Some("l"),
        LegendPos::Right => Some("r"),
    }
}

/// A group's `<c:dLbls>` children in CT_DLbls order: the delete flag, number format, position, the
/// show flags, the separator. The writer emits them all; the patcher changes only those an edit
/// touched, and leaves the rest of the element (per-point labels, leader lines, text formatting)
/// alone.
pub(crate) fn label_children(l: &LabelOptions, wb: &Workbook) -> Vec<(&'static str, Option<String>)> {
    if l.hidden {
        return vec![("delete", Some("<c:delete val=\"1\"/>".into()))];
    }
    // A chart label with no part shown would show nothing; the flag alone always meant values.
    let show_value = l.show_value || !l.shows_anything();
    vec![
        ("delete", None),
        (
            "numFmt",
            l.number_format
                .as_deref()
                .filter(|c| !c.is_empty())
                .map(|code| format!("<c:numFmt formatCode=\"{}\" sourceLinked=\"0\"/>", esc_attr(code))),
        ),
        ("txPr", l.font.as_ref().filter(|f| f.is_set()).map(|f| label_tx_pr_xml(f, wb))),
        ("dLblPos", (l.position != LabelPosition::Auto).then(|| format!("<c:dLblPos val=\"{}\"/>", label_position_file(l.position)))),
        ("showLegendKey", Some(format!("<c:showLegendKey val=\"{}\"/>", l.show_legend_key as u8))),
        ("showVal", Some(format!("<c:showVal val=\"{}\"/>", show_value as u8))),
        ("showCatName", Some(format!("<c:showCatName val=\"{}\"/>", l.show_category as u8))),
        ("showSerName", Some(format!("<c:showSerName val=\"{}\"/>", l.show_series as u8))),
        ("showPercent", Some(format!("<c:showPercent val=\"{}\"/>", l.show_percentage as u8))),
        ("showBubbleSize", Some("<c:showBubbleSize val=\"0\"/>".into())),
        ("separator", l.separator.as_deref().filter(|s| !s.is_empty()).map(|s| format!("<c:separator>{}</c:separator>", esc(s)))),
    ]
}

pub(crate) fn scaling_xml(o: &AxisOptions) -> String {
    let mut s = String::from("<c:scaling>");
    if let Some(b) = o.log_base.filter(|b| (2.0..=1000.0).contains(b)) {
        let _ = write!(s, "<c:logBase val=\"{}\"/>", num(b));
    }
    let _ = write!(s, "<c:orientation val=\"{}\"/>", if o.reversed { "maxMin" } else { "minMax" });
    for (tag, v) in [("max", o.max), ("min", o.min)] {
        if let Some(v) = v.filter(|v| v.is_finite()) {
            let _ = write!(s, "<c:{tag} val=\"{}\"/>", num(v));
        }
    }
    s.push_str("</c:scaling>");
    s
}

fn axis_xml(a: &Ax<'_>) -> String {
    let o = a.options;
    let tag = if a.cat && !a.numeric_cat { "catAx" } else { "valAx" };
    let mut s = format!("<c:{tag}><c:axId val=\"{}\"/>{}<c:delete val=\"{}\"/><c:axPos val=\"{}\"/>", a.id, scaling_xml(o), a.delete as u8, a.pos);
    if a.grid {
        s.push_str("<c:majorGridlines/>");
    }
    if let Some(t) = a.title {
        s.push_str(&title_xml(t));
    }
    match o.number_format.as_deref().filter(|c| !c.is_empty()) {
        Some(code) => {
            let _ = write!(s, "<c:numFmt formatCode=\"{}\" sourceLinked=\"0\"/>", esc_attr(code));
        }
        None => s.push_str("<c:numFmt formatCode=\"General\" sourceLinked=\"1\"/>"),
    }
    s.push_str("<c:majorTickMark val=\"out\"/><c:minorTickMark val=\"none\"/><c:tickLblPos val=\"nextTo\"/>");
    let _ = write!(s, "<c:crossAx val=\"{}\"/>", a.cross);
    match o.crosses {
        AxisCrossing::Min => s.push_str("<c:crosses val=\"min\"/>"),
        AxisCrossing::Max => s.push_str("<c:crosses val=\"max\"/>"),
        AxisCrossing::At(v) if v.is_finite() => {
            let _ = write!(s, "<c:crossesAt val=\"{}\"/>", num(v));
        }
        _ => s.push_str("<c:crosses val=\"autoZero\"/>"),
    }
    if a.cat && !a.numeric_cat {
        s.push_str("<c:auto val=\"1\"/><c:lblAlgn val=\"ctr\"/><c:lblOffset val=\"100\"/>");
        if let Some(u) = o.major_unit.filter(|u| u.is_finite() && *u >= 1.0) {
            let _ = write!(s, "<c:tickLblSkip val=\"{}\"/>", u.round().clamp(1.0, 31_999.0) as u32);
        }
        s.push_str("<c:noMultiLvlLbl val=\"0\"/>");
    } else {
        s.push_str("<c:crossBetween val=\"between\"/>");
        if let Some(u) = o.major_unit.filter(|u| u.is_finite() && *u > 0.0) {
            let _ = write!(s, "<c:majorUnit val=\"{}\"/>", num(u));
        }
        if let Some(unit) = o.display_units {
            let name = match unit {
                DisplayUnits::Hundreds => "hundreds",
                DisplayUnits::Thousands => "thousands",
                DisplayUnits::TenThousands => "tenThousands",
                DisplayUnits::HundredThousands => "hundredThousands",
                DisplayUnits::Millions => "millions",
                DisplayUnits::TenMillions => "tenMillions",
                DisplayUnits::HundredMillions => "hundredMillions",
                DisplayUnits::Billions => "billions",
                DisplayUnits::Trillions => "trillions",
            };
            let _ = write!(s, "<c:dispUnits><c:builtInUnit val=\"{name}\"/></c:dispUnits>");
        }
    }
    let _ = write!(s, "</c:{tag}>");
    s
}

/// The sheets a kept chart part's formulas (`<c:f>Data!$A$2:$A$5</c:f>`) name.
fn part_sheet_names(xml: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<c:f>") {
        let after = rest.get(start + 5..).unwrap_or("");
        let Some(end) = after.find("</c:f>") else { break };
        names.extend(crate::passthrough::sheet_names_in(after.get(..end).unwrap_or("")));
        rest = after.get(end + 6..).unwrap_or("");
    }
    names
}

/// A PivotChart link without the workbook in brackets: `[Book.xlsx]Sheet2!PT` → `Sheet2!PT`.
pub(crate) fn strip_book(link: &str) -> &str {
    link.strip_prefix('[').and_then(|r| r.split_once(']')).map_or(link, |(_, rest)| rest)
}

/// A chart part to write, and the chart's own style and colour parts (`styleN.xml`, `colorsN.xml`)
/// that go beside it. `style` and `colors` hold the file's parts as read: a chart part that is kept
/// or patched keeps them, because they describe the chart's look, not its data. Both are `None`
/// when the part is regenerated from the model — a chart whose kind or series changed no longer
/// matches them — and a default style applies instead.
pub struct ChartXml {
    pub xml: String,
    pub style: Option<String>,
    pub colors: Option<String>,
}

/// Chart part XML for a model chart on sheet `si`.
pub fn write_chart(wb: &Workbook, si: usize, ch: &Chart, book_name: &str) -> ChartXml {
    // A part read from a file whose sheets still exist is written back from the file, not from the
    // model: unchanged it is byte-identical, and an edit is patched into it field by field so every
    // option GridCraft does not model survives the edit too.
    if let Some((part, as_read)) = ch.kept_part()
        && part_sheet_names(part).iter().all(|n| wb.sheet_index(n).is_some())
    {
        let kept = || {
            let (style, colors) = ch.kept_style_parts();
            (style.map(str::to_string), colors.map(str::to_string))
        };
        if let Some(unchanged) = ch.unchanged_part() {
            let (style, colors) = kept();
            return ChartXml { xml: unchanged.to_string(), style, colors };
        }
        if let Some(patched) = crate::chart_patch::patch_chart(part, as_read, ch, wb, si, book_name) {
            let (style, colors) = kept();
            return ChartXml { xml: patched, style, colors };
        }
    }
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><c:roundedCorners val=\"0\"/>",
    );
    if (1..=48).contains(&ch.style) {
        let _ = write!(s, "<c:style val=\"{}\"/>", ch.style);
    }
    // A PivotChart names its PivotTable behind the workbook in brackets (`[Book.xlsx]Sheet2!PivotTable1`);
    // without the brackets Excel shows an ordinary chart.
    if let Some(link) = ch.pivot.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
        let link = strip_book(link);
        let _ = write!(s, "<c:pivotSource><c:name>[{}]{}</c:name><c:fmtId val=\"0\"/></c:pivotSource>", esc(book_name), esc(link));
    }
    s.push_str("<c:chart>");
    match &ch.title {
        Some(t) if !t.is_empty() => {
            s.push_str(&title_xml(t));
            s.push_str("<c:autoTitleDeleted val=\"0\"/>");
        }
        _ => s.push_str("<c:autoTitleDeleted val=\"1\"/>"),
    }
    s.push_str("<c:plotArea><c:layout/>");
    let all: Vec<(usize, &Series)> = ch.series.iter().enumerate().collect();
    let primary_g;
    let mut secondary: Option<G> = None;
    if ch.kind == ChartKind::Combo {
        // Series without a kind of their own: the first is a column, the others lines (as drawn).
        let kind = |i: usize, x: &Series| x.kind.filter(|k| *k != ChartKind::Combo).or((i > 0).then_some(ChartKind::Line));
        let is_line = |k: Option<ChartKind>| matches!(k.map(group_for), Some(G::Line { .. } | G::Area { .. } | G::Radar | G::Scatter { .. }));
        let bars: Vec<(usize, &Series)> = all.iter().filter(|(i, x)| !x.secondary && !is_line(kind(*i, x))).copied().collect();
        let lines: Vec<(usize, &Series)> = all.iter().filter(|(i, x)| !x.secondary && is_line(kind(*i, x))).copied().collect();
        let sec: Vec<(usize, &Series)> = all.iter().filter(|(_, x)| x.secondary).copied().collect();
        primary_g = G::Bar { bar: false, grouping: "clustered" };
        if !bars.is_empty() || (lines.is_empty() && sec.is_empty()) {
            s.push_str(&group_xml(primary_g, &bars, ch, (1, 2), wb, si));
        }
        if !lines.is_empty() {
            s.push_str(&group_xml(G::Line { stacked: false, markers: false }, &lines, ch, (1, 2), wb, si));
        }
        if !sec.is_empty() {
            let g = match sec.first().and_then(|(_, x)| x.kind).map(group_for) {
                Some(g @ (G::Bar { .. } | G::Line { .. } | G::Area { .. } | G::Scatter { .. } | G::Radar)) => g,
                _ => G::Line { stacked: false, markers: false },
            };
            s.push_str(&group_xml(g, &sec, ch, (3, 4), wb, si));
            secondary = Some(g);
        }
    } else {
        primary_g = match group_for(ch.kind) {
            // A stock chart has 3 or 4 series (high-low-close, open-high-low-close).
            G::Stock if !(3..=4).contains(&ch.series.len()) => G::Line { stacked: false, markers: false },
            g => g,
        };
        s.push_str(&group_xml(primary_g, &all, ch, (1, 2), wb, si));
    }
    let no_options = AxisOptions::default();
    // Excel's own secondary value axis crosses at max, which keeps the (hidden) secondary category
    // axis on the right; the model's automatic crossing means exactly that.
    let mut secondary_y = ch.y2_axis.clone();
    if secondary_y.crosses == AxisCrossing::AutoZero {
        secondary_y.crosses = AxisCrossing::Max;
    }
    match primary_g {
        G::Pie | G::Doughnut => {}
        G::Scatter { .. } | G::Bubble => {
            s.push_str(&axis_xml(&Ax {
                cat: false,
                id: 1,
                cross: 2,
                pos: "b",
                delete: false,
                grid: false,
                title: ch.x_title.as_deref(),
                options: &ch.x_axis,
                numeric_cat: true,
            }));
            s.push_str(&axis_xml(&Ax {
                cat: false,
                id: 2,
                cross: 1,
                pos: "l",
                delete: false,
                grid: ch.gridlines,
                title: ch.y_title.as_deref(),
                options: &ch.y_axis,
                numeric_cat: true,
            }));
        }
        G::Bar { bar: true, .. } => {
            s.push_str(&axis_xml(&Ax {
                cat: true,
                id: 1,
                cross: 2,
                pos: "l",
                delete: false,
                grid: false,
                title: ch.x_title.as_deref(),
                options: &ch.x_axis,
                numeric_cat: false,
            }));
            s.push_str(&axis_xml(&Ax {
                cat: false,
                id: 2,
                cross: 1,
                pos: "b",
                delete: false,
                grid: ch.gridlines,
                title: ch.y_title.as_deref(),
                options: &ch.y_axis,
                numeric_cat: true,
            }));
        }
        _ => {
            s.push_str(&axis_xml(&Ax {
                cat: true,
                id: 1,
                cross: 2,
                pos: "b",
                delete: false,
                grid: false,
                title: ch.x_title.as_deref(),
                options: &ch.x_axis,
                numeric_cat: false,
            }));
            s.push_str(&axis_xml(&Ax {
                cat: false,
                id: 2,
                cross: 1,
                pos: "l",
                delete: false,
                grid: ch.gridlines,
                title: ch.y_title.as_deref(),
                options: &ch.y_axis,
                numeric_cat: true,
            }));
        }
    }
    if secondary.is_some() {
        s.push_str(&axis_xml(&Ax {
            cat: true,
            id: 3,
            cross: 4,
            pos: "b",
            delete: true,
            grid: false,
            title: None,
            options: &no_options,
            numeric_cat: false,
        }));
        s.push_str(&axis_xml(&Ax {
            cat: false,
            id: 4,
            cross: 3,
            pos: "r",
            delete: false,
            grid: false,
            title: None,
            options: &secondary_y,
            numeric_cat: true,
        }));
    }
    s.push_str("</c:plotArea>");
    let pos = legend_pos_file(ch.legend);
    if let Some(p) = pos {
        let _ = write!(s, "<c:legend><c:legendPos val=\"{p}\"/><c:overlay val=\"0\"/></c:legend>");
    }
    s.push_str("<c:plotVisOnly val=\"1\"/><c:dispBlanksAs val=\"gap\"/></c:chart></c:chartSpace>");
    // A part regenerated from the model gets no kept style: the file's style and colour parts
    // described a chart whose kind or series changed, so they no longer fit it.
    ChartXml { xml: s, style: None, colors: None }
}
