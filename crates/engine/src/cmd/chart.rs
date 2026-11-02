//! Chart formatting commands: axis scaling, data labels, error bars and per-point formatting.
//!
//! Each command changes the model only; the chart crate draws what is stored and the xlsx writer
//! saves it, so an option that survives a save is the same one the UI draws.

use gridcraft_model::{
    AxisCrossing, AxisOptions, Chart, ChartKind, Color, DisplayUnits, ErrorBarDirection, ErrorBarKind, ErrorBars, LabelOptions, LabelPosition,
    PointFormat, PointLabel, Series,
};
use serde_json::{Value as Json, json};

use super::*;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "chart.setAxis",
            "Format Axis",
            ["Format", "Current Selection"],
            None,
            "{chart?, axis?: x|y|y2 (default y; y2 is the secondary axis of a combo chart), reset?: bool, min?, max?, majorUnit?, logBase?: 2..1000|0, reversed?: bool, crosses?: autoZero|min|max|<axis value>, displayUnits?: none|hundreds|thousands|tenThousands|hundredThousands|millions|tenMillions|hundredMillions|billions|trillions, numberFormat?: code (\"\" follows the plotted values)}",
            has_doc,
            set_axis
        ),
        cmd!(
            "chart.setDataLabels",
            "Format Data Labels",
            ["Chart Design", "Chart Layouts"],
            None,
            "{chart?, series?: index (default every series), point?: index (needs series: that point's own label), text?: reference or text for the point's label, on?: bool (default true), value?, percentage?, seriesName?, categoryName?, legendKey?, position?: auto|center|insideEnd|insideBase|outsideEnd|bestFit|left|right|top|bottom, separator?, numberFormat?, deletePoint?: index (hides that label), showPoint?: index}",
            has_doc,
            set_data_labels
        ),
        cmd!(
            "chart.setErrorBars",
            "Error Bars",
            ["Chart Design", "Chart Layouts"],
            None,
            "{chart?, series?: index (default every series), type: fixed|percentage|stdDev|stdErr|custom|none, value?: amount or deviation count, direction?: both|plus|minus, noEndCap?: bool, plus?: reference|{…}, minus?: reference|{…}}",
            has_doc,
            set_error_bars
        ),
        cmd!(
            "chart.formatPoint",
            "Format Data Point",
            ["Format", "Current Selection"],
            None,
            "{chart?, series: index, point: index, fill?: hex|\"none\", line?: hex|\"none\", marker?: hex|\"none\", markerSize?: 2..72, deleteLabel?: bool, clear?: bool}",
            has_doc,
            format_point
        ),
        cmd!(
            "chart.setSeriesAxis",
            "Secondary Axis",
            ["Chart Design", "Type"],
            None,
            "{chart?, series: index, secondary?: bool (default true), type?: chart type, subtype?: stacked|stacked100|markers|lines} (moves one series to the secondary value axis, optionally changing its kind, as Change Chart Type does) → {chart, series, secondary}",
            has_doc,
            set_series_axis
        ),
    ]
}

/// The chart a command names on the active sheet (the last one when `chart` is absent).
fn find_chart(s: &Session, p: &Json) -> Result<(usize, usize)> {
    let d = s.doc()?;
    let si = d.wb.active_sheet;
    let sh = d.wb.sheet(si).ok_or(EngineError::NoDocument)?;
    let idx = match u32_param(p, "chart") {
        Some(id) => sh.charts.iter().position(|c| c.id == id),
        None => sh.charts.len().checked_sub(1),
    };
    idx.map(|i| (si, i)).ok_or_else(|| bad("chart", "no chart on this sheet"))
}

/// Moves one series to the secondary value axis (and optionally changes its kind), the model side
/// of Excel's Change Chart Type → Secondary Axis. The chart writer already groups series by kind
/// and axis, so this is the missing command, not new drawing.
fn set_series_axis(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "chart.setSeriesAxis";
    let (si, ci) = find_chart(s, p)?;
    let index = usize::try_from(u32_param(p, "series").ok_or_else(|| bad(C, "pass `series` (the index from 0)"))?)
        .map_err(|_| bad(C, "`series` is too large"))?;
    let secondary = match p.get("secondary") {
        None => true,
        Some(Json::Bool(b)) => *b,
        Some(_) => return Err(bad(C, "`secondary` must be true or false")),
    };
    let kind = match str_param(p, "type") {
        None => None,
        Some(t) => {
            let k = super::insert::chart_kind(t, str_param(p, "subtype").unwrap_or(""));
            if k == ChartKind::Stock {
                return Err(bad(C, "a stock chart cannot be one series of a combo chart"));
            }
            Some(k)
        }
    };
    edit(s, |cx| {
        let c = cx.sheet_mut(si)?.charts.get_mut(ci).ok_or_else(|| bad(C, "no such chart"))?;
        let count = c.series.len();
        let ser = c.series.get_mut(index).ok_or_else(|| bad(C, format!("no series {index}: the chart has {count}")))?;
        ser.secondary = secondary;
        if let Some(k) = kind {
            ser.kind = Some(k);
        }
        // A chart with a per-series kind or a secondary axis is a combo chart; the writer (and the
        // renderer's per-series kind lookup) only reads `secondary` for one.
        if secondary || kind.is_some() {
            c.kind = ChartKind::Combo;
        }
        Ok(json!({"chart": c.id, "series": index, "secondary": ser.secondary}))
    })
}

fn with_chart(s: &mut Session, p: &Json, f: impl FnOnce(&mut Chart) -> Result<()>) -> Result<Json> {
    let (si, ci) = find_chart(s, p)?;
    edit(s, |cx| {
        let c = cx.sheet_mut(si)?.charts.get_mut(ci).ok_or_else(|| bad("chart", "no such chart"))?;
        f(c)?;
        Ok(json!({"chart": c.id}))
    })
}

/// Which axis of a chart an option command addresses.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AxisSel {
    /// The category axis (or a scatter chart's X).
    X,
    /// The primary value axis.
    Y,
    /// The secondary value axis of a combo chart.
    Y2,
}

impl AxisSel {
    fn is_value(self) -> bool {
        self != AxisSel::X
    }
}

/// `axis` param: the category axis, the value axis or a combo chart's secondary value axis.
fn axis_param(p: &Json, cmd: &str) -> Result<AxisSel> {
    match str_param(p, "axis").unwrap_or("y") {
        "y" | "value" | "val" => Ok(AxisSel::Y),
        "x" | "category" | "cat" => Ok(AxisSel::X),
        "y2" | "secondary" | "right" => Ok(AxisSel::Y2),
        other => Err(bad(cmd, format!("unknown axis `{other}`: expected x, y or y2"))),
    }
}

fn finite(p: &Json, key: &str, cmd: &str) -> Result<Option<f64>> {
    match f64_param(p, key) {
        Some(v) if v.is_finite() => Ok(Some(v)),
        Some(_) => Err(bad(cmd, format!("`{key}` must be a finite number"))),
        None => Ok(None),
    }
}

/// `crosses`: `min`/`max`/`autoZero`, or an axis value (number or numeric string).
fn crossing_param(v: &Json, cmd: &str) -> Result<AxisCrossing> {
    let parsed = match v {
        Json::String(s) => match s.trim() {
            "" | "autoZero" | "automatic" => Some(AxisCrossing::AutoZero),
            "min" => Some(AxisCrossing::Min),
            "max" => Some(AxisCrossing::Max),
            other => other.parse::<f64>().ok().filter(|f| f.is_finite()).map(AxisCrossing::At),
        },
        Json::Number(n) => n.as_f64().filter(|f| f.is_finite()).map(AxisCrossing::At),
        _ => None,
    };
    parsed.ok_or_else(|| bad(cmd, "crosses must be autoZero, min, max or an axis value"))
}

fn units_param(v: &Json) -> Result<Option<DisplayUnits>> {
    let name = v.as_str().ok_or_else(|| bad("chart.setAxis", "displayUnits must be a unit name or \"none\""))?;
    Ok(Some(match name {
        "hundreds" => DisplayUnits::Hundreds,
        "thousands" => DisplayUnits::Thousands,
        "tenThousands" => DisplayUnits::TenThousands,
        "hundredThousands" => DisplayUnits::HundredThousands,
        "millions" => DisplayUnits::Millions,
        "tenMillions" => DisplayUnits::TenMillions,
        "hundredMillions" => DisplayUnits::HundredMillions,
        "billions" => DisplayUnits::Billions,
        "trillions" => DisplayUnits::Trillions,
        "none" | "" => return Ok(None),
        other => return Err(bad("chart.setAxis", format!("unknown display unit `{other}`"))),
    }))
}

fn position_param(v: &Json) -> Result<LabelPosition> {
    let name = v.as_str().ok_or_else(|| bad("chart.setDataLabels", "position must be a label position"))?;
    Ok(match name {
        "auto" | "" => LabelPosition::Auto,
        "center" | "ctr" => LabelPosition::Center,
        "insideEnd" => LabelPosition::InsideEnd,
        "insideBase" => LabelPosition::InsideBase,
        "outsideEnd" => LabelPosition::OutsideEnd,
        "bestFit" => LabelPosition::BestFit,
        "left" | "l" => LabelPosition::Left,
        "right" | "r" => LabelPosition::Right,
        "top" | "t" => LabelPosition::Top,
        "bottom" | "b" => LabelPosition::Bottom,
        other => return Err(bad("chart.setDataLabels", format!("unknown label position `{other}`"))),
    })
}

/// A colour param: a hex string, or `none`/`""` to clear it.
fn color_param(v: &Json, key: &str) -> Result<Option<Color>> {
    match v.as_str() {
        Some(s) if !s.is_empty() && !s.eq_ignore_ascii_case("none") => {
            Color::from_hex(s).map(Some).ok_or_else(|| bad("chart.formatPoint", format!("`{key}` is not a colour")))
        }
        _ => Ok(None),
    }
}

fn set_axis(s: &mut Session, p: &Json) -> Result<Json> {
    const CMD: &str = "chart.setAxis";
    let axis_sel = axis_param(p, CMD)?;
    let value_axis = axis_sel.is_value();
    let reset = bool_param(p, "reset").unwrap_or(false);
    let changes = ["min", "max", "majorUnit", "logBase", "reversed", "crosses", "displayUnits", "numberFormat"].iter().any(|k| p.get(*k).is_some());
    if !reset && !changes {
        return Err(bad(CMD, "give at least one axis option (min, max, majorUnit, logBase, reversed, crosses, displayUnits, numberFormat)"));
    }
    let min = finite(p, "min", CMD)?;
    let max = finite(p, "max", CMD)?;
    let major_unit = finite(p, "majorUnit", CMD)?;
    if let Some(u) = major_unit
        && u <= 0.0
    {
        return Err(bad(CMD, "majorUnit must be greater than zero"));
    }
    let log_base = finite(p, "logBase", CMD)?;
    if let Some(b) = log_base
        && b != 0.0
        && !(2.0..=1000.0).contains(&b)
    {
        return Err(bad(CMD, "logBase must be between 2 and 1000 (0 turns the log scale off)"));
    }
    let crosses = p.get("crosses").map(|v| crossing_param(v, CMD)).transpose()?;
    let units = p.get("displayUnits").map(units_param).transpose()?;
    let number_format = p.get("numberFormat").and_then(Json::as_str).map(str::to_string);
    with_chart(s, p, |c| {
        require_axes(c, CMD)?;
        if c.kind.is_chartex() {
            // A chartex value axis carries bounds, a unit, a log base and a direction; a category
            // axis carries only a gap width, and there is no crossing, display-unit or number-format
            // element at all.
            if !chartex_value_axis(c) {
                return Err(bad(CMD, "this chart type has no value axis GridCraft can save"));
            }
            if axis_sel != AxisSel::Y {
                return Err(bad(CMD, "a chartex category axis has no options GridCraft can save"));
            }
            if crosses.is_some() {
                return Err(bad(CMD, "a chartex value axis has no crossing point"));
            }
            if units.is_some() {
                return Err(bad(CMD, "a chartex value axis has no display units GridCraft can save"));
            }
            if number_format.is_some() {
                return Err(bad(CMD, "a chartex value axis has no number format GridCraft can save"));
            }
        }
        // A category axis of labels has no log scale, display units or axis-value crossing, and
        // cropping its categories by bound is not drawn: refuse rather than store a dead option.
        if value_axis && c.kind == ChartKind::Radar && crosses.is_some() {
            return Err(bad(CMD, "a radar chart has no axis crossing"));
        }
        let numeric_cat = matches!(c.kind, ChartKind::Scatter | ChartKind::ScatterLines | ChartKind::Bubble);
        if !value_axis && !numeric_cat {
            if log_base.is_some() {
                return Err(bad(CMD, "a label category axis has no logarithmic scale"));
            }
            if units.is_some_and(|u| u.is_some()) {
                return Err(bad(CMD, "display units apply to a value axis"));
            }
            // Bounds crop (or extend) the categories, and a crossing puts the value axis at a
            // category; both are drawn.
        }
        if axis_sel == AxisSel::Y2 && !c.series.iter().any(|s| s.secondary) {
            return Err(bad(CMD, "this chart has no secondary axis: no series is plotted against one"));
        }
        let o: &mut AxisOptions = match axis_sel {
            AxisSel::X => &mut c.x_axis,
            AxisSel::Y => &mut c.y_axis,
            AxisSel::Y2 => &mut c.y2_axis,
        };
        if reset {
            *o = AxisOptions::default();
        }
        if let Some(v) = min {
            o.min = Some(v);
        }
        if let Some(v) = max {
            o.max = Some(v);
        }
        if let (Some(a), Some(b)) = (o.min, o.max)
            && a >= b
        {
            return Err(bad(CMD, "min must be less than max"));
        }
        if let Some(v) = major_unit {
            o.major_unit = Some(v);
        }
        if let Some(v) = log_base {
            o.log_base = (v != 0.0).then_some(v);
        }
        if let Some(v) = bool_param(p, "reversed") {
            o.reversed = v;
        }
        if let Some(v) = crosses {
            o.crosses = v;
        }
        if let Some(v) = units {
            o.display_units = v;
        }
        if let Some(v) = number_format {
            o.number_format = Some(v).filter(|c| !c.is_empty());
        }
        Ok(())
    })
}

/// Refuses an option on a chart whose kind cannot store it. A chartex part is patched for the
/// options it has an element for (bounds, unit, log base, direction, label parts and position,
/// colours, quartiles); the commands refuse the rest with a reason the pane shows, rather than
/// storing an option the next save drops.
fn require_classic(c: &Chart, cmd: &str) -> Result<()> {
    if c.kind.is_chartex() {
        return Err(bad(cmd, "this chart type keeps its options in a chartex part, which has no element for it"));
    }
    Ok(())
}

/// Refuses an axis option on a chart that has no axes.
fn require_axes(c: &Chart, cmd: &str) -> Result<()> {
    if matches!(c.kind, ChartKind::Pie | ChartKind::Doughnut) {
        return Err(bad(cmd, "this chart type has no axes"));
    }
    Ok(())
}

/// Whether a chartex chart has the value axis the option commands address.
fn chartex_value_axis(c: &Chart) -> bool {
    matches!(c.kind, ChartKind::Histogram | ChartKind::BoxWhisker | ChartKind::Waterfall)
}

/// Whether error bars can be saved and drawn on every group of `c` (the writer and renderer draw
/// them on 2-D column, bar, line, area, scatter and bubble series).
fn supports_error_bars(c: &Chart) -> bool {
    use ChartKind::*;
    (0..c.series.len()).all(|i| {
        matches!(
            gridcraft_chart::series_plot_kind(c, i),
            ColumnClustered
                | ColumnStacked
                | ColumnStacked100
                | BarClustered
                | BarStacked
                | BarStacked100
                | Line
                | LineMarkers
                | LineStacked
                | Area
                | AreaStacked
                | Scatter
                | ScatterLines
                | Bubble
        )
    })
}

/// The series a command applies to: one index, or every series.
fn series_targets(c: &Chart, p: &Json, cmd: &str) -> Result<Vec<usize>> {
    match u32_param(p, "series") {
        Some(i) => {
            let i = i as usize;
            if i >= c.series.len() {
                return Err(bad(cmd, "no such series"));
            }
            Ok(vec![i])
        }
        None => Ok((0..c.series.len()).collect()),
    }
}

fn set_data_labels(s: &mut Session, p: &Json) -> Result<Json> {
    let cmd = "chart.setDataLabels";
    let on = bool_param(p, "on").unwrap_or(true);
    let series = u32_param(p, "series").map(|i| i as usize);
    let point = u32_param(p, "point");
    let text = p.get("text").and_then(Json::as_str).map(str::to_string);
    let position = p.get("position").map(position_param).transpose()?;
    let separator = p.get("separator").and_then(Json::as_str).map(str::to_string);
    let number_format = p.get("numberFormat").and_then(Json::as_str).map(str::to_string);
    let delete = u32_param(p, "deletePoint");
    let show = u32_param(p, "showPoint");
    let flags = ["value", "percentage", "seriesName", "categoryName", "legendKey"].iter().map(|k| (*k, bool_param(p, k))).collect::<Vec<_>>();
    let any_flag = flags.iter().any(|(_, v)| v.is_some());
    let other_options =
        position.is_some() || separator.is_some() || number_format.is_some() || any_flag || delete.is_some() || show.is_some() || text.is_some();
    if !on && other_options {
        return Err(bad(cmd, "`on: false` turns labels off; remove the other options"));
    }
    if point.is_some() {
        if series.is_none() {
            return Err(bad(cmd, "`point` needs `series`: a label belongs to one point of one series"));
        }
        if delete.is_some() || show.is_some() {
            return Err(bad(cmd, "give either `point` or `deletePoint`/`showPoint`"));
        }
        if p.get("legendKey").is_some() {
            return Err(bad(cmd, "a point's label has no legend key in GridCraft's model"));
        }
        if separator.is_some() {
            return Err(bad(cmd, "a point's label has no separator of its own in GridCraft's model"));
        }
        if number_format.is_some() {
            return Err(bad(cmd, "a point's label has no number format of its own in GridCraft's model"));
        }
    } else if text.is_some() {
        return Err(bad(cmd, "`text` needs `point`: it sets one point's label"));
    }
    with_chart(s, p, |c| {
        if let Some(i) = series
            && i >= c.series.len()
        {
            return Err(bad(cmd, "no such series"));
        }
        if c.kind.is_chartex() {
            if delete.is_some() || show.is_some() {
                return Err(bad(cmd, "a chartex chart has no per-point labels GridCraft can save"));
            }
            return set_chartex_labels(c, cmd, on, series, point, &flags, position, separator.clone(), number_format.clone());
        }
        if let (Some(i), Some(pi)) = (series, point) {
            set_one_point_label(c, i, pi, on, text, position, &flags, any_flag);
            return Ok(());
        }
        // A series can hide its labels while the chart keeps them: `on: false` with a series.
        if let Some(i) = series {
            if !on {
                let mut o = c.series.get(i).and_then(|se| se.labels.clone()).unwrap_or_default();
                o.hidden = true;
                if let Some(se) = c.series.get_mut(i) {
                    se.labels = Some(o);
                }
                return Ok(());
            }
            let mut o = c.series.get(i).and_then(|se| se.labels.clone()).unwrap_or_default();
            o.hidden = false;
            apply_label_options(&mut o, &flags, position, separator, number_format);
            if !o.shows_anything() && !any_flag {
                o = LabelOptions::values();
            }
            if let Some(se) = c.series.get_mut(i) {
                se.labels = Some(o);
            }
            if !c.data_labels {
                // Labels were off: only the series just labelled shows them, and the file must
                // record no group-level labels (the other series are hidden).
                c.data_labels = true;
                if !c.labels.shows_anything() {
                    c.labels.hidden = true;
                }
                for (j, se) in c.series.iter_mut().enumerate() {
                    if j == i || se.labels.as_ref().is_some_and(|l| l.shows_anything() && !l.hidden) {
                        continue;
                    }
                    let mut l = se.labels.clone().unwrap_or_default();
                    l.hidden = true;
                    se.labels = Some(l);
                }
            }
            return Ok(());
        }
        let mut o = c.labels.clone();
        if on {
            o.hidden = false;
        }
        apply_label_options(&mut o, &flags, position, separator, number_format);
        if on && !o.shows_anything() && !any_flag {
            // No part asked for: the plain value label, as the on/off flag alone meant.
            o = LabelOptions::values();
        }
        c.labels = o;
        c.data_labels = on;
        // Per-point label visibility applies to one series (or all of them).
        for target in (0..c.series.len()).filter(|i| series.is_none_or(|s| s == *i)) {
            if let Some(se) = c.series.get_mut(target) {
                if let Some(idx) = delete {
                    let mut pt = se.point(idx as usize).copied().unwrap_or(PointFormat { index: idx, ..PointFormat::default() });
                    pt.index = idx;
                    pt.no_label = true;
                    se.set_point(pt);
                }
                if let Some(idx) = show
                    && let Some(pt) = se.point(idx as usize).copied()
                {
                    se.set_point(PointFormat { no_label: false, ..pt });
                }
            }
        }
        Ok(())
    })
}

/// Sets (or hides) one point's own label: its text, position and show flags (`c:dLbl`).
#[allow(clippy::too_many_arguments)]
fn set_one_point_label(
    c: &mut Chart,
    series: usize,
    pi: u32,
    on: bool,
    text: Option<String>,
    position: Option<LabelPosition>,
    flags: &[(&str, Option<bool>)],
    any_flag: bool,
) {
    let Some(se) = c.series.get_mut(series) else { return };
    if !on {
        // `on: false` for one point hides its label, the model's per-point deletion.
        let mut pt = se.point(pi as usize).copied().unwrap_or(PointFormat { index: pi, ..PointFormat::default() });
        pt.index = pi;
        pt.no_label = true;
        se.set_point(pt);
        return;
    }
    let mut label = se.point_label(pi as usize).cloned().unwrap_or(PointLabel { index: pi, ..PointLabel::default() });
    label.index = pi;
    if let Some(t) = text {
        label.text = Some(t).filter(|t| !t.is_empty());
    }
    if let Some(v) = position {
        label.position = v;
    }
    for (key, value) in flags {
        let Some(v) = value else { continue };
        match *key {
            "value" => label.show_value = Some(*v),
            "percentage" => label.show_percentage = Some(*v),
            "seriesName" => label.show_series = Some(*v),
            "categoryName" => label.show_category = Some(*v),
            _ => {}
        }
    }
    let given = any_flag || label.text.is_some() || label.position != LabelPosition::Auto;
    if !given {
        // Nothing asked for: the point's label shows again (the way `showPoint` does).
        if let Some(pt) = se.point(pi as usize).copied()
            && pt.no_label
        {
            se.set_point(PointFormat { no_label: false, ..pt });
        }
        c.data_labels = true;
        return;
    }
    se.set_point_label(label);
    if let Some(pt) = se.point(pi as usize).copied()
        && pt.no_label
    {
        se.set_point(PointFormat { no_label: false, ..pt });
    }
    c.data_labels = true;
}

/// A chartex chart's labels: one `<cx:dataLabels>` per series, holding the value, category and
/// series-name flags and a position. The part has no percentage, legend key, separator, number
/// format or per-point label, so those are refused.
#[allow(clippy::too_many_arguments)]
fn set_chartex_labels(
    c: &mut Chart,
    cmd: &str,
    on: bool,
    series: Option<usize>,
    point: Option<u32>,
    flags: &[(&str, Option<bool>)],
    position: Option<LabelPosition>,
    separator: Option<String>,
    number_format: Option<String>,
) -> Result<()> {
    if point.is_some() {
        return Err(bad(cmd, "a chartex chart has no per-point labels GridCraft can save"));
    }
    if let Some((key, _)) = flags.iter().find(|(k, v)| v.is_some() && matches!(*k, "percentage" | "legendKey")) {
        return Err(bad(cmd, format!("a chartex data label has no `{key}`: Excel's chartex part stores the value, category and series name only")));
    }
    if separator.is_some() {
        return Err(bad(cmd, "a chartex data label has no separator"));
    }
    if number_format.is_some() {
        return Err(bad(cmd, "a chartex data label has no number format"));
    }
    let targets: Vec<usize> = match series {
        Some(i) => vec![i],
        None => (0..c.series.len()).collect(),
    };
    for i in targets {
        let Some(se) = c.series.get_mut(i) else { continue };
        let mut o = se.labels.clone().unwrap_or_else(|| c.labels.clone());
        o.hidden = !on;
        if on {
            apply_label_options(&mut o, flags, position, separator.clone(), number_format.clone());
            if !o.shows_anything() && !flags.iter().any(|(_, v)| v.is_some()) {
                o = LabelOptions::values();
            }
        }
        se.labels = Some(o);
    }
    match series {
        Some(_) if on => c.data_labels = true,
        Some(_) => {}
        None => {
            c.data_labels = on;
            if let Some(o) = c.series.first().and_then(|se| se.labels.clone()) {
                c.labels = o;
            }
        }
    }
    Ok(())
}

/// Applies the show flags, position, separator and format to one label options record.
fn apply_label_options(
    o: &mut LabelOptions,
    flags: &[(&str, Option<bool>)],
    position: Option<LabelPosition>,
    separator: Option<String>,
    number_format: Option<String>,
) {
    for (key, value) in flags {
        let Some(v) = value else { continue };
        match *key {
            "value" => o.show_value = *v,
            "percentage" => o.show_percentage = *v,
            "seriesName" => o.show_series = *v,
            "categoryName" => o.show_category = *v,
            "legendKey" => o.show_legend_key = *v,
            _ => {}
        }
    }
    if let Some(v) = position {
        o.position = v;
    }
    if let Some(v) = separator {
        o.separator = Some(v).filter(|s| !s.is_empty());
    }
    if let Some(v) = number_format {
        o.number_format = Some(v).filter(|s| !s.is_empty());
    }
}

fn set_error_bars(s: &mut Session, p: &Json) -> Result<Json> {
    let cmd = "chart.setErrorBars";
    let kind = match str_param(p, "type") {
        Some("fixed") => ErrorBarKind::Fixed(finite(p, "value", cmd)?.unwrap_or(1.0)),
        Some("percentage") => ErrorBarKind::Percentage(finite(p, "value", cmd)?.unwrap_or(10.0)),
        Some("stdDev") => ErrorBarKind::StdDev(finite(p, "value", cmd)?.unwrap_or(1.0)),
        Some("stdErr") | Some("standardError") => ErrorBarKind::StdErr,
        Some("custom") => {
            let plus = str_param(p, "plus").map(str::to_string);
            let minus = str_param(p, "minus").map(str::to_string);
            if plus.is_none() && minus.is_none() {
                return Err(bad(cmd, "custom error bars need `plus` and/or `minus` references"));
            }
            ErrorBarKind::Custom { plus, minus }
        }
        Some("none") => {
            let series = u32_param(p, "series").map(|i| i as usize);
            return with_chart(s, p, |c| {
                require_classic(c, cmd)?;
                if !supports_error_bars(c) {
                    return Err(bad(cmd, "error bars are only drawn on 2-D column, bar, line, area, scatter and bubble charts"));
                }
                for target in (0..c.series.len()).filter(|i| series.is_none_or(|s| s == *i)) {
                    if let Some(se) = c.series.get_mut(target) {
                        se.error_bars = None;
                    }
                }
                Ok(())
            });
        }
        Some(other) => return Err(bad(cmd, format!("unknown error bar type `{other}`"))),
        None => return Err(bad(cmd, "give a type: fixed, percentage, stdDev, stdErr, custom or none")),
    };
    let direction = match str_param(p, "direction") {
        None | Some("both") => ErrorBarDirection::Both,
        Some("plus") => ErrorBarDirection::Plus,
        Some("minus") => ErrorBarDirection::Minus,
        Some(other) => return Err(bad(cmd, format!("direction must be both, plus or minus (got `{other}`)"))),
    };
    let bars = ErrorBars { direction, kind, no_end_cap: bool_param(p, "noEndCap").unwrap_or(false) };
    with_chart(s, p, |c| {
        require_classic(c, cmd)?;
        if !supports_error_bars(c) {
            return Err(bad(cmd, "error bars are only drawn on 2-D column, bar, line, area, scatter and bubble charts"));
        }
        let targets = series_targets(c, p, cmd)?;
        for i in targets {
            if let Some(se) = c.series.get_mut(i) {
                se.error_bars = Some(bars.clone());
            }
        }
        Ok(())
    })
}

fn format_point(s: &mut Session, p: &Json) -> Result<Json> {
    let cmd = "chart.formatPoint";
    let si = u32_param(p, "series").ok_or_else(|| bad(cmd, "give a series index"))? as usize;
    let pi = u32_param(p, "point").ok_or_else(|| bad(cmd, "give a point index"))?;
    let clear = bool_param(p, "clear").unwrap_or(false);
    let fill = p.get("fill").map(|v| color_param(v, "fill")).transpose()?.flatten();
    let line = p.get("line").map(|v| color_param(v, "line")).transpose()?.flatten();
    let marker = p.get("marker").map(|v| color_param(v, "marker")).transpose()?.flatten();
    let marker_size = match u32_param(p, "markerSize") {
        Some(n) if (2..=72).contains(&n) => Some(n as u8),
        Some(_) => return Err(bad(cmd, "`markerSize` must be between 2 and 72 points")),
        None => None,
    };
    let delete_label = bool_param(p, "deleteLabel");
    let given = clear || fill.is_some() || line.is_some() || marker.is_some() || marker_size.is_some() || delete_label.is_some();
    if !given {
        return Err(bad(cmd, "give a fill, line, marker colour, markerSize, deleteLabel or clear"));
    }
    with_chart(s, p, |c| {
        require_classic(c, cmd)?;
        if marker.is_some() || marker_size.is_some() {
            use ChartKind::*;
            let plot = gridcraft_chart::series_plot_kind(c, si);
            if !matches!(plot, Line | LineMarkers | LineStacked | Scatter | ScatterLines | Bubble | Radar) {
                return Err(bad(cmd, "a marker colour or size applies to line, scatter, bubble and radar points only"));
            }
        }
        let se: &mut Series = c.series.get_mut(si).ok_or_else(|| bad(cmd, "no such series"))?;
        if clear {
            se.points.retain(|x| x.index != pi);
            return Ok(());
        }
        let mut p = se.point(pi as usize).copied().unwrap_or(PointFormat { index: pi, ..PointFormat::default() });
        p.index = pi;
        if let Some(c) = fill {
            p.fill = Some(c);
        }
        if let Some(c) = line {
            p.line = Some(c);
        }
        if let Some(c) = marker {
            p.marker = Some(c);
        }
        if let Some(n) = marker_size {
            p.marker_size = Some(n);
        }
        if let Some(v) = delete_label {
            p.no_label = v;
        }
        se.set_point(p);
        Ok(())
    })
}
