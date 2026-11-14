//! Patching an edit into a kept chartex part (`xl/charts/chartExN.xml`).
//!
//! A chartex part read from a file is kept (`Chart::part`) just like a classic chart part. An
//! edit rewrites only the elements the model owns — the chart and axis titles, the legend, the
//! value axis' scaling, each series' fill and per-point fills, data labels (their visibility,
//! position, font and each point's own record) and box & whisker quartiles — and copies
//! everything else byte for byte: the plot area's layout, tick-label formatting, data-point
//! callouts, `cx:units`, `cx:subtotals`, `cx:extLst` and the style/colour parts around it.
//!
//! A change the chartex format cannot express (a different kind or set of series, a different data
//! reference) makes the caller regenerate the part from the model instead; the commands refuse the
//! options a chartex part has no element for.

use std::fmt::Write as _;

use gridcraft_model::{AxisOptions, Chart, LabelFont, LabelOptions, LabelPosition, PointFormat, QuartileMethod, Series, Workbook};

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
const SERIES_ORDER: &[&str] =
    &["tx", "spPr", "valueColors", "valueColorPositions", "dataPt", "dataLabels", "dataId", "layoutPr", "axisId", "errBars", "extLst"];
/// `cx:dataLabels`.
const DATALABELS_ORDER: &[&str] = &["visibility", "dataLabel", "leaderLines", "numFmt", "spPr", "txPr", "extLst"];
/// `cx:dataLabel` (one point's own label).
const DATALABEL_ORDER: &[&str] = &["visibility", "txPr", "extLst"];
/// `cx:dataPt` (one point's own fill and outline).
const DATAPT_ORDER: &[&str] = &["spPr", "extLst"];

/// `<cx:visibility>` switching every part off: how a hidden series' or point's labels are written.
const VISIBILITY_OFF: &str = "<cx:visibility seriesName=\"0\" categoryName=\"0\" value=\"0\"/>";

/// `<cx:title>` around a literal text.
fn title_xml(t: &str) -> String {
    format!("<cx:title pos=\"t\" align=\"ctr\" overlay=\"0\">{}</cx:title>", tx_xml(t))
}

/// `<cx:tx>` around a literal text (a title's, not a data reference).
fn tx_xml(t: &str) -> String {
    format!("<cx:tx><cx:txData><cx:v>{}</cx:v></cx:txData></cx:tx>", esc(t))
}

/// `<cx:dataLabels>`: what a series' labels show, where, their own font and the records of the
/// points that carry one. `None` writes the element that shows nothing (a series whose labels are
/// switched off but whose points keep a record).
pub(super) fn labels_xml(l: Option<&LabelOptions>, points: &[PointLabelState], wb: &Workbook) -> String {
    let mut x = String::from("<cx:dataLabels");
    if let Some(l) = l.filter(|l| l.position != LabelPosition::Auto) {
        x.push_str(&pos_attr(l.position));
    }
    x.push('>');
    match l {
        Some(l) => x.push_str(&visibility_xml(l)),
        None => x.push_str(VISIBILITY_OFF),
    }
    for p in points.iter().filter(|p| !p.is_empty()) {
        x.push_str(&point_state_xml(p, l, wb));
    }
    // `cx:txPr` comes after the points' `cx:dataLabel` elements.
    if let Some(f) = l.and_then(|l| l.font).filter(|f| f.is_set()) {
        x.push_str(&label_font_xml(&f, wb));
    }
    x.push_str("</cx:dataLabels>");
    x
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

/// `<cx:txPr>` for a label font, in the same DrawingML body the classic part uses
/// (`a:CT_TextBody`: size in hundredths of a point, weight and colour).
pub(super) fn label_font_xml(f: &LabelFont, wb: &Workbook) -> String {
    let f = f.sane();
    let mut attrs = String::new();
    if let Some(size) = f.size {
        let _ = write!(attrs, " sz=\"{}\"", (size * 100.0).round().clamp(100.0, 40_000.0) as i64);
    }
    if let Some(b) = f.bold {
        let _ = write!(attrs, " b=\"{}\"", b as u8);
    }
    let fill = f.color.as_ref().and_then(|c| srgb(c, &wb.theme)).map(|c| format!("<a:solidFill>{c}</a:solidFill>")).unwrap_or_default();
    format!("<cx:txPr><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr{attrs}>{fill}</a:defRPr></a:pPr><a:endParaRPr lang=\"en-US\"/></a:p></cx:txPr>")
}

/// A point's own data label as a chartex part spells it. The part has one `cx:dataLabel` per point
/// and it carries what the label shows (three flags) and its font — no text, position or
/// percentage, which a classic part's `c:dLbl` holds and this one has no element for.
#[derive(Clone, Default, PartialEq)]
pub(super) struct PointLabelState {
    pub index: u32,
    /// The label is switched off for this point: every flag is 0 and nothing else is set.
    pub hidden: bool,
    /// A flag the point leaves unset follows the series' options; the part always writes all three.
    pub show_value: Option<bool>,
    pub show_series: Option<bool>,
    pub show_category: Option<bool>,
    pub font: Option<LabelFont>,
}

impl PointLabelState {
    /// Nothing the part can hold: no element is written for the point.
    fn is_empty(&self) -> bool {
        !self.hidden && !self.shows_anything() && !self.font.is_some_and(|f| f.is_set())
    }

    /// Whether the point switches any part of the label on or off.
    fn shows_anything(&self) -> bool {
        self.show_value.is_some() || self.show_series.is_some() || self.show_category.is_some()
    }

    /// The flags the point sets, with the ones it leaves unset as the series' options give them.
    fn flags(&self, o: Option<&LabelOptions>) -> (bool, bool, bool) {
        if self.hidden {
            return (false, false, false);
        }
        let o = o.cloned().unwrap_or_default();
        let value = self.show_value.unwrap_or(o.show_value || !o.shows_anything());
        let series = self.show_series.unwrap_or(o.show_series);
        let category = self.show_category.unwrap_or(o.show_category);
        (series, category, value)
    }
}

/// `<cx:visibility>` for a point's own label.
fn point_visibility_xml(p: &PointLabelState, o: Option<&LabelOptions>) -> String {
    let (series, category, value) = p.flags(o);
    format!("<cx:visibility seriesName=\"{}\" categoryName=\"{}\" value=\"{}\"/>", series as u8, category as u8, value as u8)
}

/// `<cx:dataLabel idx="…">` for one point: its flags and its font.
fn point_state_xml(p: &PointLabelState, o: Option<&LabelOptions>, wb: &Workbook) -> String {
    let mut x = format!("<cx:dataLabel idx=\"{}\">", p.index);
    x.push_str(&point_visibility_xml(p, o));
    if let Some(f) = p.font.filter(|f| f.is_set()) {
        x.push_str(&label_font_xml(&f, wb));
    }
    x.push_str("</cx:dataLabel>");
    x
}

/// The points of a series that carry a record of their own: the ones whose label is switched off
/// (the model's `no_label`) and the ones with their own flags or font. Sorted by index.
pub(super) fn point_states(ser: &Series) -> Vec<PointLabelState> {
    let mut out: Vec<PointLabelState> = Vec::new();
    for p in ser.points.iter().filter(|p| p.no_label).take(crate::chart::MAX_POINTS) {
        out.push(PointLabelState { index: p.index, hidden: true, ..PointLabelState::default() });
    }
    for l in ser.point_labels.iter().take(crate::chart::MAX_POINTS) {
        let state = PointLabelState {
            index: l.index,
            hidden: false,
            show_value: l.show_value,
            show_series: l.show_series,
            show_category: l.show_category,
            font: l.font,
        };
        match out.iter_mut().find(|s| s.index == l.index) {
            Some(slot) => *slot = state,
            None => out.push(state),
        }
    }
    out.sort_by_key(|s| s.index);
    out
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
        if then_ser.points != now_ser.points {
            patch_points(&mut plan, xml, ser, &then_ser.points, &now_ser.points, wb);
        }
        let before = effective(then, then_ser);
        let after = effective(now, now_ser);
        let then_pts = point_states(then_ser);
        let now_pts = point_states(now_ser);
        if before != after || then_pts != now_pts {
            patch_labels(&mut plan, xml, ser, before, after, &then_pts, &now_pts, wb);
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

/// A point's `cx:spPr` (its fill and outline) as the part spells it; `None` when the point sets
/// neither.
pub(super) fn point_sp_pr(fill: Option<&str>, line: Option<&str>) -> Option<String> {
    if fill.is_none() && line.is_none() {
        return None;
    }
    let mut s = String::from("<cx:spPr>");
    if let Some(c) = fill {
        let _ = write!(s, "<a:solidFill>{c}</a:solidFill>");
    }
    if let Some(c) = line {
        let _ = write!(s, "<a:ln w=\"12700\"><a:solidFill>{c}</a:solidFill></a:ln>");
    }
    s.push_str("</cx:spPr>");
    Some(s)
}

/// Whether a `cx:dataPt` holds nothing but the children the model owns.
fn is_own_point(n: &crate::chart_patch::Node) -> bool {
    n.children.iter().all(|c| c.name == "spPr")
}

/// Whether a point's `cx:spPr` holds nothing but the fill and outline the model owns.
fn is_own_sp_pr(n: &crate::chart_patch::Node) -> bool {
    n.children.iter().all(|c| matches!(c.name.as_str(), "solidFill" | "noFill" | "ln"))
}

/// Patches each point's own `cx:dataPt`: only a point whose fill or outline changed is touched, so
/// the parts of the element GridCraft does not model stay.
fn patch_points(plan: &mut Plan, xml: &str, ser: &crate::chart_patch::Node, then: &[PointFormat], now: &[PointFormat], wb: &Workbook) {
    let index = |n: &crate::chart_patch::Node| attr(xml, n, "idx").and_then(|v| v.trim().parse::<u32>().ok());
    let nodes: Vec<(&crate::chart_patch::Node, u32)> =
        ser.children.iter().filter(|c| c.name == "dataPt").filter_map(|n| Some((n, index(n)?))).collect();
    for (n, idx) in &nodes {
        let a = then.iter().find(|p| p.index == *idx);
        let b = now.iter().find(|p| p.index == *idx);
        // Only the colour that changed is written: the point's outline keeps its width and dash, and
        // its fill keeps whatever the file put beside it.
        let fill_changed = a.and_then(|p| p.fill.as_ref()) != b.and_then(|p| p.fill.as_ref());
        let line_changed = a.and_then(|p| p.line.as_ref()) != b.and_then(|p| p.line.as_ref());
        if !fill_changed && !line_changed {
            continue;
        }
        let fill = b.and_then(|p| p.fill.as_ref()).and_then(|c| srgb(c, &wb.theme));
        let line = b.and_then(|p| p.line.as_ref()).and_then(|c| srgb(c, &wb.theme));
        match child(n, "spPr") {
            Some(sp) => {
                if fill_changed {
                    plan.set(sp, SPPR_ORDER, "solidFill", fill.clone().map(|c| format!("<a:solidFill>{c}</a:solidFill>")));
                }
                if line_changed {
                    plan.set(sp, SPPR_ORDER, "ln", line.clone().map(|c| format!("<a:ln w=\"12700\"><a:solidFill>{c}</a:solidFill></a:ln>")));
                }
                // The point's colours were cleared: its own properties go with them, and the element
                // too when nothing else is in it.
                if fill.is_none() && line.is_none() && is_own_sp_pr(sp) {
                    plan.set(n, DATAPT_ORDER, "spPr", None);
                    if is_own_point(n)
                        && let Some(i) = ser.children.iter().position(|c| c.start == n.start)
                    {
                        plan.edits(ser, SERIES_ORDER).remove.push(i);
                    }
                }
            }
            None => {
                if let Some(text) = point_sp_pr(fill.as_deref(), line.as_deref()) {
                    plan.set(n, DATAPT_ORDER, "spPr", Some(text));
                }
            }
        }
    }
    for p in now.iter().filter(|p| p.fill.is_some() || p.line.is_some()) {
        if nodes.iter().any(|(_, idx)| *idx == p.index) {
            continue;
        }
        let fill = p.fill.as_ref().and_then(|c| srgb(c, &wb.theme));
        let line = p.line.as_ref().and_then(|c| srgb(c, &wb.theme));
        if let Some(sp) = point_sp_pr(fill.as_deref(), line.as_deref()) {
            plan.add(ser, SERIES_ORDER, "dataPt", format!("<cx:dataPt idx=\"{}\">{sp}</cx:dataPt>", p.index));
        }
    }
}

/// Whether a `cx:dataLabel` holds nothing but the children the model owns.
fn is_own_label(n: &crate::chart_patch::Node) -> bool {
    n.children.iter().all(|c| matches!(c.name.as_str(), "visibility" | "txPr"))
}

/// Patches a series' `cx:dataLabels`: its own visibility flags, position and font when they
/// changed, and the record of each point whose own label changed. Only the changed children are
/// written, so everything the model does not own (a callout outline, leader lines, an extension)
/// stays where it was.
#[allow(clippy::too_many_arguments)]
fn patch_labels(
    plan: &mut Plan,
    xml: &str,
    ser: &crate::chart_patch::Node,
    before: Option<&LabelOptions>,
    after: Option<&LabelOptions>,
    then: &[PointLabelState],
    now: &[PointLabelState],
    wb: &Workbook,
) {
    let Some(d) = child(ser, "dataLabels") else {
        // A series that had no label element: the whole record is new.
        if after.is_some() || now.iter().any(|p| !p.is_empty()) {
            plan.set(ser, SERIES_ORDER, "dataLabels", Some(labels_xml(after, now, wb)));
        }
        return;
    };
    if before.map(|b| (b.show_value, b.show_series, b.show_category)) != after.map(|a| (a.show_value, a.show_series, a.show_category)) {
        let text = match after {
            Some(a) => visibility_xml(a),
            None => VISIBILITY_OFF.to_string(),
        };
        plan.set(d, DATALABELS_ORDER, "visibility", Some(text));
    }
    if before.and_then(|b| b.font) != after.and_then(|a| a.font) {
        let text = after.and_then(|a| a.font).filter(|f| f.is_set()).map(|f| label_font_xml(&f, wb));
        plan.set(d, DATALABELS_ORDER, "txPr", text);
    }
    if before.map(|b| b.position) != after.map(|a| a.position) {
        let pos = after.filter(|a| a.position != LabelPosition::Auto).map(|a| label_position_file(a.position).to_string());
        plan.set_attr(d, "pos", pos);
    }
    let mut indices: Vec<u32> = then.iter().chain(now).map(|p| p.index).collect();
    indices.sort_unstable();
    indices.dedup();
    for idx in indices {
        let a = then.iter().find(|p| p.index == idx);
        let b = now.iter().find(|p| p.index == idx);
        if a == b {
            continue;
        }
        let node = d.children.iter().find(|n| n.name == "dataLabel" && attr(xml, n, "idx").and_then(|v| v.trim().parse::<u32>().ok()) == Some(idx));
        let Some(b) = b.filter(|b| !b.is_empty()) else {
            // The point no longer carries a record: drop the children the model owns, and the
            // element itself when there is nothing else in it.
            if let Some(n) = node {
                if is_own_label(n) {
                    if let Some(i) = d.children.iter().position(|c| c.start == n.start) {
                        plan.edits(d, DATALABELS_ORDER).remove.push(i);
                    }
                } else {
                    let pe = plan.edits(n, DATALABEL_ORDER);
                    pe.set("visibility", None);
                    pe.set("txPr", None);
                }
            }
            continue;
        };
        match node {
            Some(n) => {
                let flags_changed = a.map(|s| (s.hidden, s.show_value, s.show_series, s.show_category))
                    != Some((b.hidden, b.show_value, b.show_series, b.show_category));
                let font_changed = a.and_then(|s| s.font) != b.font;
                let pe = plan.edits(n, DATALABEL_ORDER);
                if flags_changed {
                    pe.set("visibility", Some(point_visibility_xml(b, after)));
                }
                if font_changed {
                    pe.set("txPr", b.font.filter(|f| f.is_set()).map(|f| label_font_xml(&f, wb)));
                }
            }
            None => plan.add(d, DATALABELS_ORDER, "dataLabel", point_state_xml(b, after, wb)),
        }
    }
}
