//! Patching an edit into a kept chartex part (`xl/charts/chartExN.xml`).
//!
//! A chartex part read from a file is kept (`Chart::part`) just like a classic chart part. An
//! edit rewrites only the elements the model owns — the chart and axis titles, the legend, the
//! value axis' scaling, each series' fill, data labels and box & whisker quartiles — and copies
//! everything else byte for byte: the plot area's layout, tick-label formatting, data-point
//! callouts, `cx:units`, `cx:subtotals`, `cx:extLst` and the style/colour parts around it.
//!
//! A change the chartex format cannot express (a different kind or set of series, a different data
//! reference) makes the caller regenerate the part from the model instead; the commands refuse the
//! options a chartex part has no element for.

use std::fmt::Write as _;

use gridcraft_model::{AxisOptions, Chart, LabelOptions, LabelPosition, QuartileMethod, Series, Workbook};

use crate::chart::label_position_file;
use crate::chart_patch::{MAX_PATCH_BYTES, Plan, SPPR_ORDER, attr, child, scan};
use crate::drawing::srgb;
use crate::xml::{esc, num};

// The schema orders a new element has to sit in (ECMA-376 Part 1, 21.2.2 chartex).

/// `cx:chart`.
const CHART_ORDER: &[&str] = &["title", "autoTitleDeleted", "plotArea", "legend", "plotVisOnly", "dispBlanksAs", "showDLblsOverMax", "extLst"];
/// `cx:title`.
const TITLE_ORDER: &[&str] = &["tx", "overlay", "spPr", "txPr", "extLst"];
/// `cx:axis`.
const AXIS_ORDER: &[&str] = &[
    "catScaling",
    "valScaling",
    "title",
    "units",
    "majorGridlines",
    "minorGridlines",
    "majorTickMarks",
    "minorTickMarks",
    "tickLabels",
    "spPr",
    "txPr",
    "extLst",
];
/// `cx:series`.
const SERIES_ORDER: &[&str] = &["tx", "spPr", "dataPt", "dataLabels", "dataId", "layoutPr", "axisId", "errBars", "extLst"];
/// `cx:dataLabels`.
const DATALABELS_ORDER: &[&str] = &["visibility", "dataLabel"];

/// `<cx:title>` around a literal text.
fn title_xml(t: &str) -> String {
    format!("<cx:title pos=\"t\" align=\"ctr\" overlay=\"0\">{}</cx:title>", tx_xml(t))
}

/// `<cx:tx>` around a literal text (a title's, not a data reference).
fn tx_xml(t: &str) -> String {
    format!("<cx:tx><cx:txData><cx:v>{}</cx:v></cx:txData></cx:tx>", esc(t))
}

/// `<cx:dataLabels>` outside a `pos` attribute: what the labels show.
pub(super) fn labels_xml(l: &LabelOptions) -> String {
    format!("<cx:dataLabels{}>{}</cx:dataLabels>", pos_attr(l.position), visibility_xml(l))
}

/// `pos="…"` for a label position, empty for automatic.
fn pos_attr(p: LabelPosition) -> String {
    if p == LabelPosition::Auto { String::new() } else { format!(" pos=\"{}\"", label_position_file(p)) }
}

/// `<cx:visibility>`: which parts a label shows. A label that asks for nothing falls back to the
/// value, as the classic writer does.
fn visibility_xml(l: &LabelOptions) -> String {
    let value = l.show_value || !l.shows_anything();
    format!("<cx:visibility seriesName=\"{}\" categoryName=\"{}\" value=\"{}\"/>", l.show_series as u8, l.show_category as u8, value as u8)
}

/// The label options in force for series `s`: its own, else the chart's; `None` when the chart's
/// labels are off or the series hides its own.
pub(super) fn effective<'a>(c: &'a Chart, s: &'a Series) -> Option<&'a LabelOptions> {
    let l = s.labels.as_ref().unwrap_or(&c.labels);
    (c.data_labels && !l.hidden).then_some(l)
}

/// `<cx:valScaling>` from the model's value-axis options.
pub(crate) fn val_scaling_xml(o: &AxisOptions) -> String {
    let mut s = String::from("<cx:valScaling");
    for (name, v) in [
        ("min", o.min.filter(|v| v.is_finite())),
        ("max", o.max.filter(|v| v.is_finite())),
        ("majorUnit", o.major_unit.filter(|u| u.is_finite() && *u > 0.0)),
        ("logBase", o.log_base.filter(|b| (2.0..=1000.0).contains(b))),
    ] {
        if let Some(v) = v {
            let _ = write!(s, " {name}=\"{}\"", num(v));
        }
    }
    if o.reversed {
        s.push_str(" orientation=\"maxMin\"");
    }
    s.push_str("/>");
    s
}

/// Patches `now`'s edits into the kept chartex part `xml`, which was read as `then` (with its
/// hidden `_xlchart` names resolved). `None` when the change is not one a patch can express, so
/// the caller regenerates the part from the model.
pub fn patch_chartex(xml: &str, then: &Chart, now: &Chart, wb: &Workbook, _si: usize) -> Option<String> {
    if xml.len() > MAX_PATCH_BYTES || then.kind != now.kind || then.series.len() != now.series.len() {
        return None;
    }
    for (a, b) in then.series.iter().zip(&now.series) {
        // A chartex chart has no per-series kind, no secondary axis and no data of its own: its
        // series keep their references (a changed one regenerates the part).
        if a.secondary != b.secondary || a.kind != b.kind || a.name != b.name || a.categories != b.categories || a.values != b.values {
            return None;
        }
    }
    let roots = scan(xml)?;
    let space = roots.iter().find(|n| n.name == "chartSpace")?;
    // Everything written here is `cx:`-prefixed: only patch a part whose root declares it.
    let root_tag = xml.get(space.start..space.open_end)?;
    if !root_tag.contains("xmlns:cx=") && !root_tag.contains("xmlns:cx1=") {
        return None;
    }
    let chart = child(space, "chart")?;
    let plot = child(chart, "plotArea")?;
    let region = child(plot, "plotAreaRegion")?;
    let sers: Vec<&crate::chart_patch::Node> = region.children.iter().filter(|n| n.name == "series").collect();
    if sers.len() != now.series.len() {
        return None;
    }
    let mut plan = Plan::default();

    if then.title != now.title {
        let text = now.title.as_deref().filter(|t| !t.is_empty()).map(title_xml);
        match child(chart, "title") {
            // Keep Excel's own title alignment and formatting: only the text's element changes.
            Some(t) if text.is_some() => plan.set(t, TITLE_ORDER, "tx", Some(tx_xml(now.title.as_deref().unwrap_or("")))),
            // An empty title: the element goes, which is how chartex spells "no title".
            Some(_) => plan.set(chart, CHART_ORDER, "title", None),
            None => plan.set(chart, CHART_ORDER, "title", text),
        }
    }
    if then.legend != now.legend || then.legend_overlay != now.legend_overlay {
        let pos = match now.legend {
            gridcraft_model::LegendPos::None => None,
            gridcraft_model::LegendPos::Bottom => Some("b"),
            gridcraft_model::LegendPos::Top => Some("t"),
            gridcraft_model::LegendPos::Left => Some("l"),
            gridcraft_model::LegendPos::Right => Some("r"),
        };
        match (child(chart, "legend"), pos) {
            // A chartex legend's position and overlay are both attributes, so patching either
            // keeps the element's alignment and whatever else the file put on it.
            (Some(l), Some(p)) => {
                if then.legend != now.legend {
                    plan.set_attr(l, "pos", Some(p.to_string()));
                }
                if then.legend_overlay != now.legend_overlay {
                    plan.set_attr(l, "overlay", Some((now.legend_overlay as u8).to_string()));
                }
            }
            (Some(_), None) => plan.set(chart, CHART_ORDER, "legend", None),
            (None, Some(p)) => plan.set(
                chart,
                CHART_ORDER,
                "legend",
                Some(format!("<cx:legend pos=\"{p}\" align=\"ctr\" overlay=\"{}\"/>", now.legend_overlay as u8)),
            ),
            (None, None) => {}
        }
    }

    let axis = |id: &str| plot.children.iter().find(|n| n.name == "axis" && attr(xml, n, "id") == Some(id));

    // The value axis: bounds, unit, log base and direction live on `cx:valScaling`.
    if then.y_axis != now.y_axis || then.y_title != now.y_title {
        let a = axis("1")?;
        if then.y_title != now.y_title {
            patch_axis_title(&mut plan, a, now.y_title.as_deref());
        }
        if then.y_axis != now.y_axis {
            let sc = child(a, "valScaling")?;
            patch_val_scaling(&mut plan, sc, &now.y_axis);
        }
    }
    // A category axis has no scaling the model can change (only a gap width, which is not
    // modelled): a difference means the part cannot express the edit.
    if then.x_axis != now.x_axis {
        return None;
    }
    if then.x_title != now.x_title {
        let a = axis("0")?;
        patch_axis_title(&mut plan, a, now.x_title.as_deref());
    }
    if then.gridlines != now.gridlines {
        let a = axis("1")?;
        plan.set(a, AXIS_ORDER, "majorGridlines", now.gridlines.then(|| "<cx:majorGridlines/>".to_string()));
    }

    for (i, ser) in sers.iter().enumerate() {
        let then_ser = then.series.get(i)?;
        let now_ser = now.series.get(i)?;
        if then_ser.color != now_ser.color {
            patch_series_fill(&mut plan, ser, now_ser, i, wb)?;
        }
        if then_ser.quartiles != now_ser.quartiles && now.kind == gridcraft_model::ChartKind::BoxWhisker {
            let lp = child(ser, "layoutPr")?;
            let st = child(lp, "statistics")?;
            let method = match now_ser.quartiles {
                QuartileMethod::Exclusive => "exclusive",
                QuartileMethod::Inclusive => "inclusive",
            };
            plan.set_attr(st, "quartileMethod", Some(method.to_string()));
        }
        let before = effective(then, then_ser);
        let after = effective(now, now_ser);
        if before == after {
            continue;
        }
        match (child(ser, "dataLabels"), after) {
            (None, None) => {}
            (Some(_), None) => plan.set(ser, SERIES_ORDER, "dataLabels", None),
            (None, Some(l)) => plan.set(ser, SERIES_ORDER, "dataLabels", Some(labels_xml(l))),
            (Some(d), Some(l)) => {
                plan.set(d, DATALABELS_ORDER, "visibility", Some(visibility_xml(l)));
                plan.set_attr(d, "pos", (l.position != LabelPosition::Auto).then(|| label_position_file(l.position).to_string()));
            }
        }
    }

    let mut out = String::with_capacity(xml.len() + 256);
    out.push_str(xml.get(..space.start)?);
    out.push_str(&crate::chart_patch::render_node(xml, space, &plan));
    out.push_str(xml.get(space.end..)?);
    Some(out)
}

/// Patches an axis title: only the text's `cx:tx` changes, so the title's alignment and formatting
/// stay; a new title is added where the schema puts it.
fn patch_axis_title(plan: &mut Plan, axis: &crate::chart_patch::Node, title: Option<&str>) {
    match (child(axis, "title"), title) {
        (Some(t), Some(text)) => plan.set(t, TITLE_ORDER, "tx", Some(tx_xml(text))),
        (Some(_), None) => plan.set(axis, AXIS_ORDER, "title", None),
        (None, Some(text)) => plan.set(axis, AXIS_ORDER, "title", Some(format!("<cx:title>{}</cx:title>", tx_xml(text)))),
        (None, None) => {}
    }
}

/// Patches the attributes of `cx:valScaling`.
fn patch_val_scaling(plan: &mut Plan, sc: &crate::chart_patch::Node, o: &AxisOptions) {
    let finite = |v: Option<f64>| v.filter(|v| v.is_finite()).map(num);
    plan.set_attr(sc, "min", finite(o.min));
    plan.set_attr(sc, "max", finite(o.max));
    plan.set_attr(sc, "majorUnit", o.major_unit.filter(|u| u.is_finite() && *u > 0.0).map(num));
    plan.set_attr(sc, "logBase", o.log_base.filter(|b| (2.0..=1000.0).contains(b)).map(num));
    plan.set_attr(sc, "orientation", o.reversed.then(|| "maxMin".to_string()));
}

/// Patches a series' fill: its own `cx:spPr` when the part has one, else a new one after `cx:tx`.
/// A cleared colour goes back to the series' theme accent.
fn patch_series_fill(plan: &mut Plan, ser: &crate::chart_patch::Node, now: &Series, i: usize, wb: &Workbook) -> Option<()> {
    let color = match &now.color {
        Some(c) => srgb(c, &wb.theme)?,
        None => crate::chartex::accent(i),
    };
    let fill = format!("<a:solidFill>{color}</a:solidFill>");
    match child(ser, "spPr") {
        Some(sp) => {
            match child(sp, "solidFill") {
                Some(f) => plan.replace(f, fill.clone()),
                None => plan.set(sp, SPPR_ORDER, "solidFill", Some(fill.clone())),
            }
            // A box & whisker series also draws its whiskers in the series colour (an outline).
            if let Some(ln) = child(sp, "ln") {
                match child(ln, "solidFill") {
                    Some(f) => plan.replace(f, fill),
                    None => plan.set(ln, SPPR_ORDER, "solidFill", Some(fill)),
                }
            }
        }
        None => plan.set(ser, SERIES_ORDER, "spPr", Some(format!("<cx:spPr>{fill}</cx:spPr>"))),
    }
    Some(())
}
