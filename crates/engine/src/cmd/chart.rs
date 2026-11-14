//! Chart formatting commands: axis scaling, data labels, error bars and per-point formatting.
//!
//! Each command changes the model only; the chart crate draws what is stored and the xlsx writer
//! saves it, so an option that survives a save is the same one the UI draws.

use std::sync::Arc;

use gridcraft_model::{
    Anchor, AnchorMode, AxisCrossing, AxisOptions, Chart, ChartKind, Color, DisplayUnits, ErrorBarDirection, ErrorBarKind, ErrorBars, Gridlines,
    LabelFont, LabelOptions, LabelPosition, LegendPos, PointFormat, PointLabel, Series, Sheet,
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
            "{chart?, series?: index (default every series), point?: index (needs series: that point's own label), text?: reference or text for the point's label, on?: bool (default true), value?, percentage?, seriesName?, categoryName?, legendKey?, position?: auto|center|insideEnd|insideBase|outsideEnd|bestFit|left|right|top|bottom, separator?, numberFormat?, deletePoint?: index (hides that label), showPoint?: index, fontSize?: 1..400 (the label's own size), bold?: bool, color?: hex or none} (a chartex chart stores the value, seriesName and categoryName flags, the position, the font and each point's own flags; it has no percentage, legend key, separator, number format or per-point text, so those are refused)",
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
            "chart.setGridlines",
            "Gridlines",
            ["Chart Design", "Chart Layouts"],
            None,
            "{chart?, axis?: x|y (default y: the value axis; x is the category axis, or a scatter chart's numeric X), major?: bool, minor?: bool, on?: bool (both sets at once)}",
            has_doc,
            set_gridlines
        ),
        cmd!(
            "chart.setLegend",
            "Legend",
            ["Chart Design", "Chart Layouts"],
            None,
            "{chart?, position?: none|bottom|top|left|right, overlay?: bool (draw the legend over the plot area instead of in a band of its own), deleteEntry?: series index, showEntry?: series index} (a legend entry is a series' entry; a pie-like chart's entries are its points and a waterfall's its totals, which GridCraft does not model)",
            has_doc,
            set_legend
        ),
        cmd!(
            "chart.formatPoint",
            "Format Data Point",
            ["Format", "Current Selection"],
            None,
            "{chart?, series: index, point: index, fill?: hex|\"none\", line?: hex|\"none\", marker?: hex|\"none\", markerSize?: 2..72, deleteLabel?: bool, clear?: bool} (a chartex chart's points take the fill, the outline and the label deletion; no chartex kind draws markers)",
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
        cmd!(
            "chart.moveToSheet",
            "Move Chart to a New Sheet",
            ["Chart Design", "Location"],
            None,
            "{chart?: id, name?: sheet name (default ChartN)}: moves the chart onto a chart sheet of its own, right after the sheet it is on. One undo step; undo puts the chart back where it was → {chart, sheet, name}",
            has_doc,
            move_to_sheet
        ),
        cmd!(
            "chart.moveToObject",
            "Move Chart to a Sheet",
            ["Chart Design", "Location"],
            None,
            "{sheet?: name | index (default: the worksheet before it), at?: \"C3\", width?, height?: pixels, dx?, dy?: pixels}: puts the active chart sheet's chart back on a worksheet as a floating object, and removes the chart sheet → {chart, sheet}",
            has_doc,
            move_to_object
        ),
    ]
}

/// Gridlines on one axis, major and/or minor set (Add Chart Element ▸ Gridlines). The value axis's
/// major set is the chart-level gridlines flag that chart.set has always written, so the two
/// spellings name the same thing.
fn set_gridlines(s: &mut Session, p: &Json) -> Result<Json> {
    const CMD: &str = "chart.setGridlines";
    let axis_sel = axis_param(p, CMD)?;
    if axis_sel == AxisSel::Y2 {
        return Err(bad(CMD, "gridlines are modelled on the category axis and the primary value axis, not on the secondary one"));
    }
    let on = bool_param(p, "on");
    let major = bool_param(p, "major");
    let minor = bool_param(p, "minor");
    if on.is_none() && major.is_none() && minor.is_none() {
        return Err(bad(CMD, "give major, minor or on (both sets at once)"));
    }
    with_chart(s, p, |c| {
        if c.kind.is_chartex() {
            // A chartex part has one value axis with a major-gridline element and nothing else: the
            // minor set and the category gridlines have no element to be stored in.
            if !chartex_value_axis(c) {
                return Err(bad(CMD, "this chart type has no axis frame to draw gridlines on"));
            }
            if axis_sel != AxisSel::Y || minor.is_some() || on.is_some() {
                return Err(bad(
                    CMD,
                    "a chartex part stores only the value axis's major gridlines: give major (chart.set gridlines is the same flag)",
                ));
            }
            c.gridlines = major.unwrap_or(c.gridlines);
            c.gridlines_minor = false;
            c.x_gridlines = Gridlines::default();
            return Ok(());
        }
        if !has_axis_frame(c.kind) {
            return Err(bad(CMD, "this chart type has no axis frame to draw gridlines on"));
        }
        if axis_sel == AxisSel::X && c.kind == ChartKind::Radar {
            return Err(bad(CMD, "a radar chart's category axis is its ring of spokes: it has no category gridlines"));
        }
        let apply = |g: &mut Gridlines| {
            if let Some(v) = on {
                g.major = v;
                g.minor = v;
            }
            if let Some(v) = major {
                g.major = v;
            }
            if let Some(v) = minor {
                g.minor = v;
            }
        };
        if axis_sel == AxisSel::X {
            apply(&mut c.x_gridlines);
        } else {
            let mut g = c.y_gridlines();
            apply(&mut g);
            c.set_y_gridlines(g);
        }
        Ok(())
    })
}

/// Whether a chart kind draws an axis frame (a pie-like chart draws no axes, so a gridline on it
/// would never be seen).
fn has_axis_frame(kind: ChartKind) -> bool {
    matches!(
        kind,
        ChartKind::ColumnClustered
            | ChartKind::ColumnStacked
            | ChartKind::ColumnStacked100
            | ChartKind::BarClustered
            | ChartKind::BarStacked
            | ChartKind::BarStacked100
            | ChartKind::Line
            | ChartKind::LineMarkers
            | ChartKind::LineStacked
            | ChartKind::Area
            | ChartKind::AreaStacked
            | ChartKind::Combo
            | ChartKind::Scatter
            | ChartKind::ScatterLines
            | ChartKind::Bubble
            | ChartKind::Radar
            | ChartKind::Stock
    )
}

/// The legend: where it sits, whether it lies over the plot area, and which series' entries are
/// deleted (c:legend/c:overlay, c:legendEntry/c:delete).
fn set_legend(s: &mut Session, p: &Json) -> Result<Json> {
    const CMD: &str = "chart.setLegend";
    let position = match str_param(p, "position") {
        None => None,
        Some("none") => Some(LegendPos::None),
        Some("bottom") => Some(LegendPos::Bottom),
        Some("top") => Some(LegendPos::Top),
        Some("left") => Some(LegendPos::Left),
        Some("right") => Some(LegendPos::Right),
        Some(other) => return Err(bad(CMD, format!("unknown legend position {other}: expected none, bottom, top, left or right"))),
    };
    let overlay = bool_param(p, "overlay");
    let delete = u32_param(p, "deleteEntry");
    let show = u32_param(p, "showEntry");
    if position.is_none() && overlay.is_none() && delete.is_none() && show.is_none() {
        return Err(bad(CMD, "give a position, overlay, deleteEntry or showEntry"));
    }
    if delete.is_some() && show.is_some() {
        return Err(bad(CMD, "give either deleteEntry or showEntry"));
    }
    with_chart(s, p, |c| {
        if let Some(pos) = position {
            c.legend = pos;
        }
        if let Some(v) = overlay {
            c.legend_overlay = v;
        }
        let entry = delete.map(|i| (i, true)).or(show.map(|i| (i, false)));
        if let Some((i, deleted)) = entry {
            if !c.kind.legend_entries_are_series() {
                return Err(bad(CMD, "this chart's legend entries are not series' entries, the only kind GridCraft can delete"));
            }
            let count = c.series.len();
            let se = c.series.get_mut(i as usize).ok_or_else(|| bad(CMD, format!("no series {i}: the chart has {count}")))?;
            se.legend_deleted = deleted;
        }
        Ok(())
    })
}

/// The chart a command names on the active sheet (the last one when chart is absent).
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

/// Excel's Move Chart ▸ New sheet: the chart leaves its worksheet and becomes a chart sheet of its
/// own, right after the sheet it was on, which becomes active. One undo step: the model goes back
/// with the chart back on its worksheet where it was.
fn move_to_sheet(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "chart.moveToSheet";
    if s.doc()?.wb.protected_structure {
        return Err(EngineError::Other("The workbook is protected and cannot be changed.".into()));
    }
    let (si, ci) = find_chart(s, p)?;
    let d = s.doc()?;
    let sh = d.wb.sheet(si).ok_or(EngineError::NoDocument)?;
    if sh.is_chart_sheet() {
        return Err(bad(C, "the chart is already on a chart sheet"));
    }
    let name = match str_param(p, "name") {
        Some(n) => n.trim().to_string(),
        None => d.wb.next_chart_sheet_name(),
    };
    d.wb.check_sheet_name(&name, None).map_err(EngineError::Other)?;
    let chart_id = sh.charts.get(ci).map(|c| c.id).unwrap_or_default();
    let at = edit(s, |cx| {
        let chart = {
            let sheet = cx.sheet_mut(si)?;
            let chart = sheet.charts.get(ci).cloned().ok_or_else(|| bad(C, "no such chart"))?;
            sheet.charts.remove(ci);
            chart
        };
        // Right after the chart's own sheet, as Excel places it.
        let at = si.saturating_add(1).min(cx.wb.sheets.len());
        cx.wb.sheets.insert(at, Arc::new(Sheet::chart_sheet(name.clone(), chart)));
        // Names scoped to later sheets shift.
        for n in cx.wb.names.iter_mut() {
            if let Some(sc) = n.scope.as_mut()
                && *sc >= at
            {
                *sc += 1;
            }
        }
        cx.wb.active_sheet = at;
        *cx.sel = crate::Selection::default();
        cx.structural = true;
        Ok(at)
    })?;
    let d = s.doc_mut()?;
    d.sheet_selections.insert(at.min(d.sheet_selections.len()), crate::Selection::default());
    Ok(json!({"chart": chart_id, "sheet": at, "name": name}))
}

/// The way back (Excel's Move Chart ▸ Object in): the chart of the active chart sheet becomes a
/// floating object on worksheet `sheet`, and the chart sheet goes away with it. One undo step.
fn move_to_object(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "chart.moveToObject";
    if s.doc()?.wb.protected_structure {
        return Err(EngineError::Other("The workbook is protected and cannot be changed.".into()));
    }
    let d = s.doc()?;
    let si = d.wb.active_sheet;
    let sh = d.wb.sheet(si).ok_or(EngineError::NoDocument)?;
    if !sh.is_chart_sheet() {
        return Err(bad(C, "the active sheet is not a chart sheet (chart.moveToSheet puts a chart on one)"));
    }
    let mut chart = sh.charts.first().cloned().ok_or_else(|| bad(C, "the chart sheet shows no chart to move"))?;
    // Where it lands: the sheet named, or the nearest worksheet before the chart sheet — a chart
    // sheet is never a place for an object.
    let target = match p.get("sheet") {
        Some(_) => Some(super::target_sheet(s, p)?),
        None => (0..si).rev().find(|i| d.wb.sheet(*i).is_some_and(|s| !s.is_chart_sheet())),
    };
    let Some(target) = target else { return Err(bad(C, "there is no worksheet to put the chart on")) };
    if d.wb.sheet(target).is_some_and(|s| s.is_chart_sheet()) {
        return Err(bad(C, "a chart sheet holds its own chart and no objects: choose a worksheet"));
    }
    let at = cell_param_on(s, p, "at", target)?.unwrap_or_default();
    let size = |v: f32, fallback: f32| f64::from(if v >= 1.0 { v } else { fallback });
    let width = f64_param(p, "width").unwrap_or(size(chart.anchor.width, 480.0)).clamp(1.0, 10_000.0) as f32;
    let height = f64_param(p, "height").unwrap_or(size(chart.anchor.height, 300.0)).clamp(1.0, 10_000.0) as f32;
    let dx = f64_param(p, "dx").unwrap_or(0.0).clamp(0.0, 10_000.0) as f32;
    let dy = f64_param(p, "dy").unwrap_or(0.0).clamp(0.0, 10_000.0) as f32;
    let chart_id = chart.id;
    let placed = edit(s, |cx| {
        // The chart sheet goes first, so the target's index is the one it has afterwards.
        super::sheet::drop_sheet(cx, si);
        let target = if target > si { target.saturating_sub(1) } else { target };
        chart.anchor = Anchor { cell: at, dx, dy, width, height, mode: AnchorMode::MoveAndSize };
        cx.sheet_mut(target)?.charts.push(chart);
        cx.wb.active_sheet = target;
        *cx.sel = crate::Selection::default();
        cx.structural = true;
        Ok(target)
    })?;
    let d = s.doc_mut()?;
    if si < d.sheet_selections.len() {
        d.sheet_selections.remove(si);
    }
    Ok(json!({"chart": chart_id, "sheet": placed}))
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
fn color_param_for(cmd: &str, v: &Json, key: &str) -> Result<Option<Color>> {
    match v.as_str() {
        Some(s) if !s.is_empty() && !s.eq_ignore_ascii_case("none") => {
            Color::from_hex(s).map(Some).ok_or_else(|| bad(cmd, format!("`{key}` is not a colour")))
        }
        _ => Ok(None),
    }
}

/// [`color_param_for`] for `chart.formatPoint`.
fn color_param(v: &Json, key: &str) -> Result<Option<Color>> {
    color_param_for("chart.formatPoint", v, key)
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
    // The label's own font: size in points, weight and colour.
    let font_size = match f64_param(p, "fontSize") {
        Some(s) if s.is_finite() && (1.0..=400.0).contains(&s) => Some(s as f32),
        Some(_) => return Err(bad(cmd, "`fontSize` must be between 1 and 400 points")),
        None => None,
    };
    let font_bold = bool_param(p, "bold");
    let font_color = p.get("color").map(|v| color_param_for(cmd, v, "color")).transpose()?.flatten();
    let font_given = font_size.is_some() || font_bold.is_some() || p.get("color").is_some();
    let font = font_given.then_some(LabelFont { size: font_size, bold: font_bold, color: font_color });
    let flags = ["value", "percentage", "seriesName", "categoryName", "legendKey"].iter().map(|k| (*k, bool_param(p, k))).collect::<Vec<_>>();
    let any_flag = flags.iter().any(|(_, v)| v.is_some());
    let other_options = position.is_some()
        || separator.is_some()
        || number_format.is_some()
        || any_flag
        || delete.is_some()
        || show.is_some()
        || text.is_some()
        || font_given;
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
            // A chartex point label is a record of what it shows and its font: the part has no text
            // element for one point's label.
            if text.is_some() {
                return Err(bad(cmd, "a chartex point label has no text of its own: give its visibility flags or a font"));
            }
            return set_chartex_labels(c, cmd, on, series, point, &flags, position, separator.clone(), number_format.clone(), font, delete, show);
        }
        if let (Some(i), Some(pi)) = (series, point) {
            set_one_point_label(c, i, pi, on, text, position, &flags, any_flag, font);
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
            if let Some(f) = font {
                o.font = Some(merge_font(o.font, f));
            }
            if !o.shows_anything() && !any_flag && !font_given {
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
        if let Some(f) = font {
            o.font = Some(merge_font(o.font, f));
        }
        if on && !o.shows_anything() && !any_flag && !font_given {
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
    font: Option<LabelFont>,
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
    if let Some(f) = font {
        label.font = Some(merge_font(label.font, f));
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
    let given = any_flag || label.text.is_some() || label.position != LabelPosition::Auto || label.font.is_some();
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
/// series-name flags, a position and the labels' font, plus one `<cx:dataLabel idx="…">` per point
/// that carries its own flags or font. The part has no percentage, legend key, separator, number
/// format or per-point text, so those are refused.
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
    font: Option<LabelFont>,
    delete: Option<u32>,
    show: Option<u32>,
) -> Result<()> {
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
    // One point's own record: its flags and its font, in the series' `cx:dataLabels`.
    if let Some(pi) = point {
        if position.is_some() {
            return Err(bad(cmd, "a chartex chart keeps one labels' position for the whole series, not one per point"));
        }
        let Some(i) = series else { return Err(bad(cmd, "`point` needs `series`: a label belongs to one point of one series")) };
        let Some(se) = c.series.get_mut(i) else { return Err(bad(cmd, "no such series")) };
        if !on {
            // `on: false` for one point hides its label, the model's per-point deletion.
            let mut pt = se.point(pi as usize).copied().unwrap_or(PointFormat { index: pi, ..PointFormat::default() });
            pt.index = pi;
            pt.no_label = true;
            se.set_point(pt);
            return Ok(());
        }
        let mut label = se.point_label(pi as usize).cloned().unwrap_or(PointLabel { index: pi, ..PointLabel::default() });
        label.index = pi;
        if let Some(f) = font {
            label.font = Some(merge_font(label.font, f));
        }
        for (key, value) in flags {
            let Some(v) = value else { continue };
            match *key {
                "value" => label.show_value = Some(*v),
                "seriesName" => label.show_series = Some(*v),
                "categoryName" => label.show_category = Some(*v),
                _ => {}
            }
        }
        let given = flags.iter().any(|(_, v)| v.is_some()) || label.font.is_some();
        if given {
            se.set_point_label(label);
        }
        // Either way the point's label shows again.
        if let Some(pt) = se.point(pi as usize).copied()
            && pt.no_label
        {
            se.set_point(PointFormat { no_label: false, ..pt });
        }
        c.data_labels = true;
        return Ok(());
    }
    // Per-point visibility applies to one series (or all of them).
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
    for i in targets {
        let Some(se) = c.series.get_mut(i) else { continue };
        let mut o = se.labels.clone().unwrap_or_else(|| c.labels.clone());
        o.hidden = !on;
        if on {
            apply_label_options(&mut o, flags, position, separator.clone(), number_format.clone());
            if let Some(f) = font {
                o.font = Some(merge_font(o.font, f));
            }
            if !o.shows_anything() && !flags.iter().any(|(_, v)| v.is_some()) && o.font.is_none() {
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
/// A label font with `now` applied over `old`: a field the caller left out keeps what the label
/// had (setting a colour alone does not clear its size).
fn merge_font(old: Option<LabelFont>, now: LabelFont) -> LabelFont {
    let old = old.unwrap_or_default();
    LabelFont { size: now.size.or(old.size), bold: now.bold.or(old.bold), color: now.color.or(old.color) }.sane()
}

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
        if c.kind.is_chartex() {
            // A chartex part keeps a point's fill and outline (`cx:dataPt`) and its label's state,
            // but no chartex kind draws markers.
            if marker.is_some() || marker_size.is_some() {
                return Err(bad(cmd, "a chartex chart's points have no markers: give a fill, an outline or `deleteLabel`"));
            }
        }
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;

    /// A workbook with a two-column block and one chart on it.
    fn session() -> (Session, u32) {
        let mut s = Session::new();
        s.new_workbook();
        s.execute("range.setValues", json!({"range": "A1", "values": [["Region", "Q1"], ["North", 48], ["South", 39], ["East", 55], ["West", 61]]}))
            .unwrap();
        s.execute("selection.set", json!({"range": "A1:B5"})).unwrap();
        let chart = s.execute("insert.chart", json!({"type": "column", "at": "D2", "width": 420, "height": 260})).unwrap();
        let id = u32::try_from(chart["chart"].as_u64().unwrap()).unwrap();
        (s, id)
    }

    /// Move Chart ▸ New sheet, and undo: the chart is back on its worksheet, where it was, in one
    /// step, and the chart sheet is gone.
    #[test]
    fn move_to_sheet_puts_the_chart_on_a_chart_sheet_and_undo_puts_it_back() {
        let (mut s, id) = session();
        assert_eq!(s.doc().unwrap().wb.sheets.len(), 1);
        let r = s.execute("chart.moveToSheet", json!({"chart": id})).unwrap();
        assert_eq!(r["sheet"], json!(1));
        assert_eq!(r["name"], json!("Chart1"));
        {
            let d = s.doc().unwrap();
            assert_eq!(d.wb.sheets.len(), 2);
            assert_eq!(d.wb.active_sheet, 1);
            let cs = d.wb.sheet(1).unwrap();
            assert!(cs.is_chart_sheet());
            assert_eq!(cs.name, "Chart1");
            assert_eq!(cs.charts.len(), 1);
            assert_eq!(cs.charts[0].id, id);
            assert!(cs.charts[0].series[0].values.contains("Sheet1!"));
            // The worksheet no longer holds it, and its cells are untouched.
            assert!(d.wb.sheet(0).unwrap().charts.is_empty());
            assert_eq!(d.wb.sheet(0).unwrap().value(gridcraft_core::CellRef::new(1, 1)), gridcraft_core::Value::Number(48.0));
        }
        // One undo step takes the whole move back.
        s.execute("edit.undo", json!({})).unwrap();
        {
            let d = s.doc().unwrap();
            assert_eq!(d.wb.sheets.len(), 1);
            assert_eq!(d.wb.active_sheet, 0);
            assert!(!d.wb.sheet(0).unwrap().is_chart_sheet());
            assert_eq!(d.wb.sheet(0).unwrap().charts.iter().map(|c| c.id).collect::<Vec<_>>(), vec![id]);
        }
    }

    /// The chart sheet can be renamed, moved, hidden and deleted like any other sheet.
    #[test]
    fn a_chart_sheet_is_a_sheet_for_the_sheet_commands() {
        let (mut s, _id) = session();
        s.execute("chart.moveToSheet", json!({"name": "Sales chart"})).unwrap();
        s.execute("sheet.rename", json!({"sheet": 1, "name": "Revenue"})).unwrap();
        s.execute("sheet.move", json!({"sheet": 1, "to": 0})).unwrap();
        {
            let d = s.doc().unwrap();
            assert_eq!(d.wb.sheets[0].name, "Revenue");
            assert!(d.wb.sheets[0].is_chart_sheet());
            assert_eq!(d.wb.active_sheet, 0);
        }
        s.execute("sheet.hide", json!({"sheet": 0})).unwrap();
        assert_eq!(s.doc().unwrap().wb.sheets[0].visibility, gridcraft_model::Visibility::Hidden);
        // The last visible sheet cannot go (Excel's rule); unhide, then delete the chart sheet.
        s.execute("sheet.unhide", json!({"sheet": 0})).unwrap();
        s.execute("home.deleteSheet", json!({"sheet": 0})).unwrap();
        let d = s.doc().unwrap();
        assert_eq!(d.wb.sheets.len(), 1);
        assert!(!d.wb.sheet(0).unwrap().is_chart_sheet());
    }

    /// The way back: chart.moveToObject puts the chart on a worksheet as a floating object and
    /// removes the chart sheet, in one undoable step.
    #[test]
    fn move_to_object_brings_the_chart_back_and_undo_returns_the_chart_sheet() {
        let (mut s, id) = session();
        s.execute("chart.moveToSheet", json!({})).unwrap();
        let r = s.execute("chart.moveToObject", json!({"sheet": 0, "at": "F4", "width": 300, "height": 180})).unwrap();
        assert_eq!(r["sheet"], json!(0));
        {
            let d = s.doc().unwrap();
            assert_eq!(d.wb.sheets.len(), 1);
            let sh = d.wb.sheet(0).unwrap();
            let ch = sh.charts.iter().find(|c| c.id == id).unwrap();
            assert_eq!(ch.anchor.cell, gridcraft_core::CellRef::new(3, 5));
            assert_eq!((ch.anchor.width, ch.anchor.height), (300.0, 180.0));
            assert!(!sh.is_chart_sheet());
        }
        s.execute("edit.undo", json!({})).unwrap();
        {
            let d = s.doc().unwrap();
            assert_eq!(d.wb.sheets.len(), 2);
            assert!(d.wb.sheet(1).unwrap().is_chart_sheet());
            assert!(d.wb.sheet(0).unwrap().charts.is_empty());
        }
    }

    /// A chart sheet never takes a floating chart, and a chart sheet's chart cannot be deleted on
    /// its own: the sheet is what goes.
    #[test]
    fn a_chart_sheet_takes_no_objects_and_its_chart_is_deleted_with_it() {
        let (mut s, _id) = session();
        s.execute("chart.moveToSheet", json!({})).unwrap();
        assert!(s.execute("chart.moveToObject", json!({"sheet": 1})).is_err(), "a chart sheet holds no objects");
        s.execute("sheet.activate", json!({"sheet": 1})).unwrap();
        assert!(s.execute("chart.delete", json!({})).is_err(), "the chart is the sheet");
        s.execute("home.deleteSheet", json!({"sheet": 1})).unwrap();
        let d = s.doc().unwrap();
        assert_eq!(d.wb.sheets.len(), 1);
        assert!(!d.wb.sheet(0).unwrap().is_chart_sheet());
    }

    /// Printing and PDF export walk every sheet: a chart sheet is a page, not a crash.
    #[test]
    fn a_chart_sheet_prints_and_exports_pdf() {
        let (mut s, _id) = session();
        s.execute("chart.moveToSheet", json!({})).unwrap();
        // The chart sheet is the active sheet now.
        let r = s.execute("file.exportPdf", json!({})).unwrap();
        assert!(r["pages"].as_u64().unwrap_or(0) >= 1, "a chart sheet is a page: {r:?}");
        let r = s.execute("file.exportPdf", json!({"sheets": "all"})).unwrap();
        assert!(r["pages"].as_u64().unwrap_or(0) >= 2, "both sheets print: {r:?}");
        assert!(s.execute("review.checkAccessibility", json!({})).is_ok());
        assert!(s.execute("file.printPreview", json!({})).is_ok());
    }

    /// Cells typed on a chart sheet are refused at the model's door: nothing is stored where a
    /// save would drop it.
    #[test]
    fn a_chart_sheet_never_stores_cells() {
        let (mut s, _id) = session();
        s.execute("chart.moveToSheet", json!({})).unwrap();
        let _ = s.execute("cell.set", json!({"cell": "A1", "input": "12"}));
        let d = s.doc().unwrap();
        let cs = d.wb.sheet(1).unwrap();
        assert!(cs.is_chart_sheet());
        assert!(cs.cells.is_empty(), "a chart sheet holds no cells");
        assert_eq!(cs.used_range(), None);
    }
}
