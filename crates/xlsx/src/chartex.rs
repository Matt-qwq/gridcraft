//! Office 2016 chart types (histogram, box & whisker, waterfall, funnel, treemap, sunburst),
//! which files keep as `cx:chartSpace` "chartex" parts (`xl/charts/chartExN.xml`) rather than
//! DrawingML chart parts.

use std::fmt::Write as _;

use gridcraft_core::RangeRef;
use gridcraft_formula::{Expr, SheetSel};
use gridcraft_model::{Anchor, AxisOptions, Chart, ChartKind, LabelOptions, LegendPos, QuartileMethod, Series, Workbook};

use crate::drawing::{dml_color, srgb};
use crate::xml::{El, esc, esc_attr};

pub const CT_CHARTEX: &str = "application/vnd.ms-office.chartex+xml";
pub const REL_CHARTEX: &str = "http://schemas.microsoft.com/office/2014/relationships/chartEx";
pub const CT_STYLE: &str = "application/vnd.ms-office.chartstyle+xml";
pub const CT_COLORS: &str = "application/vnd.ms-office.chartcolorstyle+xml";
pub const REL_STYLE: &str = "http://schemas.microsoft.com/office/2011/relationships/chartStyle";
pub const REL_COLORS: &str = "http://schemas.microsoft.com/office/2011/relationships/chartColorStyle";

#[path = "chartex_patch.rs"]
mod chartex_patch;

/// The `mc:Choice` requirement for a kind's drawing frame: (prefix, namespace). Funnel charts
/// came in a later revision of the format.
pub fn requires(k: ChartKind) -> (&'static str, &'static str) {
    match k {
        ChartKind::Funnel => ("cx2", "http://schemas.microsoft.com/office/drawing/2015/10/21/chartex"),
        _ => ("cx1", "http://schemas.microsoft.com/office/drawing/2015/9/8/chartex"),
    }
}

// ---------------------------------------------------------------- reading

fn kind_of(layout: &str) -> Option<ChartKind> {
    Some(match layout {
        "clusteredColumn" | "paretoLine" => ChartKind::Histogram,
        "boxWhisker" => ChartKind::BoxWhisker,
        "waterfall" => ChartKind::Waterfall,
        "funnel" => ChartKind::Funnel,
        "treemap" => ChartKind::Treemap,
        "sunburst" => ChartKind::Sunburst,
        _ => return None,
    })
}

fn formula(dim: &El) -> Option<String> {
    let f = dim.child("f")?;
    Some(crate::fmla::from_file(&crate::chart::chart_ref_from_file(&f.text)))
}

fn text(tx: &El) -> Option<String> {
    if let Some(d) = tx.child("txData") {
        if let Some(f) = d.child("f") {
            return Some(crate::chart::chart_ref_from_file(&f.text));
        }
        return d.child("v").map(|v| v.text.clone());
    }
    let rich = tx.child("rich")?;
    let paras: Vec<String> = rich.kids("p").map(|p| p.kids("r").filter_map(|r| r.child("t")).map(|t| t.text.as_str()).collect()).collect();
    let s = paras.join("\n");
    if s.is_empty() { None } else { Some(s) }
}

/// A series' `<cx:dataLabels>`: what its labels show and where. A record with every part off means
/// the series' labels are switched off.
fn read_labels(d: &El) -> LabelOptions {
    let vis = d.child("visibility");
    let flag = |k: &str| vis.and_then(|v| v.attr_bool(k)).unwrap_or(false);
    let options = LabelOptions {
        show_value: flag("value"),
        show_category: flag("categoryName"),
        show_series: flag("seriesName"),
        position: d.attr("pos").and_then(crate::chart::label_position).unwrap_or_default(),
        ..LabelOptions::default()
    };
    if options.shows_anything() { options } else { LabelOptions { hidden: true, ..options } }
}

/// The value axis' options from `<cx:valScaling>`. A chartex category axis carries only a gap width,
/// so nothing of `x_axis` is read.
fn read_val_scaling(plot: &El) -> AxisOptions {
    let Some(sc) = plot.kids("axis").find(|a| a.attr("id") == Some("1")).and_then(|a| a.child("valScaling")) else {
        return AxisOptions::default();
    };
    AxisOptions {
        min: sc.attr_f64("min"),
        max: sc.attr_f64("max"),
        major_unit: sc.attr_f64("majorUnit").filter(|u| *u > 0.0),
        log_base: sc.attr_f64("logBase").filter(|b| (2.0..=1000.0).contains(b)),
        reversed: sc.attr("orientation") == Some("maxMin"),
        ..AxisOptions::default()
    }
}

pub fn read_chartex(root: &El) -> Option<Chart> {
    let chart = root.child("chart")?;
    let plot = chart.child("plotArea")?;
    let region = plot.child("plotAreaRegion")?;
    // Data by id: (categories, values).
    let data: Vec<(String, Option<String>, Option<String>)> = root
        .child("chartData")
        .map(|cd| {
            cd.kids("data")
                .map(|d| {
                    let id = d.attr("id").unwrap_or("").to_string();
                    let cat = d.kids("strDim").chain(d.kids("numDim")).find(|x| matches!(x.attr("type"), Some("cat" | "x"))).and_then(formula);
                    let val = d.kids("numDim").find(|x| matches!(x.attr("type"), Some("val" | "size" | "y"))).and_then(formula);
                    (id, cat, val)
                })
                .collect()
        })
        .unwrap_or_default();
    let mut kind = None;
    let mut series = vec![];
    let mut data_labels = false;
    for s in region.kids("series") {
        let layout = s.attr("layoutId").unwrap_or("");
        // A Pareto line belongs to its histogram.
        if layout == "paretoLine" {
            continue;
        }
        let k = kind_of(layout)?;
        kind.get_or_insert(k);
        let id = s.child_val("dataId").unwrap_or("");
        let (cat, val) = data.iter().find(|(i, _, _)| i == id).map(|(_, c, v)| (c.clone(), v.clone())).unwrap_or_default();
        let labels = s.child("dataLabels").map(read_labels);
        data_labels |= labels.as_ref().is_some_and(|l| l.shows_anything());
        series.push(Series {
            quartiles: match s.path(&["layoutPr", "statistics"]).and_then(|el| el.attr("quartileMethod")) {
                Some("inclusive") => QuartileMethod::Inclusive,
                _ => QuartileMethod::Exclusive,
            },
            name: s.child("tx").and_then(text),
            categories: cat,
            values: val.unwrap_or_default(),
            labels,
            // The default colour (the series' theme accent, which the writer spells out) isn't kept.
            color: s
                .path(&["spPr", "solidFill"])
                .and_then(dml_color)
                .filter(|c| *c != gridcraft_model::Color::Theme(4 + (series.len() % 6) as u8, 0)),
            ..Default::default()
        });
    }
    let legend = match chart.child("legend") {
        None => LegendPos::None,
        Some(l) => match l.attr("pos").unwrap_or("r") {
            "b" => LegendPos::Bottom,
            "t" => LegendPos::Top,
            "l" => LegendPos::Left,
            _ => LegendPos::Right,
        },
    };
    let axis_title = |id: &str| plot.kids("axis").find(|a| a.attr("id") == Some(id)).and_then(|a| a.path(&["title", "tx"])).and_then(text);
    Some(Chart {
        id: 0,
        kind: kind?,
        anchor: Default::default(),
        title: chart.path(&["title", "tx"]).and_then(text),
        series,
        legend,
        data_labels,
        gridlines: plot.kids("axis").any(|a| a.child("majorGridlines").is_some()),
        style: 2,
        x_title: axis_title("0"),
        y_title: axis_title("1"),
        y_axis: read_val_scaling(plot),
        source: None,
        by_rows: false,
        pivot: None,
        ..Chart::default()
    })
}

/// Whether a chartex part is one GridCraft models: every plot-area series has a layout it can read
/// and write, and none of them is a Pareto line (which belongs to a histogram the model would
/// write back as a plain histogram, losing the line).
pub fn is_modelled(root: &El) -> bool {
    let Some(region) = root.path(&["chart", "plotArea", "plotAreaRegion"]) else { return false };
    let mut any = false;
    for s in region.kids("series") {
        any = true;
        let layout = s.attr("layoutId").unwrap_or("");
        // A Pareto line belongs to its histogram; the model would write the histogram back without
        // it. A layout the model does not know (a filled map) cannot be written at all.
        if layout == "paretoLine" || kind_of(layout).is_none() {
            return false;
        }
    }
    any
}

// ---------------------------------------------------------------- writing

/// Prefix of the hidden defined names chartex formulas go through: a chartex part refers to
/// its data by names (`_xlchart.v1.0`), which the workbook defines, not by references.
pub const NAME_PREFIX: &str = "_xlchart.v1.";

/// `<cx:f>` for a reference: a new hidden workbook name for it (collected in `names`, written
/// with the workbook's defined names). A reference along a row says so; otherwise each column
/// is one level.
fn f_xml(wb: &Workbook, si: usize, f: &str, names: &mut Vec<String>) -> String {
    let row = ref_range(wb, si, f).is_some_and(|(_, r)| r.height() == 1 && r.width() > 1);
    names.push(f.to_string());
    format!("<cx:f{}>{NAME_PREFIX}{}</cx:f>", if row { " dir=\"row\"" } else { "" }, names.len() - 1)
}

/// A reference's sheet and range (for the row/column direction).
fn ref_range(wb: &Workbook, si: usize, f: &str) -> Option<(usize, RangeRef)> {
    let Ok(Expr::Ref(r)) = gridcraft_formula::parse(f) else { return None };
    let sheet = match &r.sheet {
        SheetSel::Current => si,
        SheetSel::Named(n) => wb.sheet_index(n)?,
        SheetSel::Span(..) => return None,
    };
    Some((sheet, r.range()))
}

fn tx_xml(t: &str, wb: &Workbook, si: usize, names: &mut Vec<String>) -> String {
    if ref_range(wb, si, t).is_some() {
        let v = crate::chart::ref_values(wb, si, t).and_then(|v| v.into_iter().next()).map(|v| v.display()).unwrap_or_default();
        format!("<cx:tx><cx:txData>{}<cx:v>{}</cx:v></cx:txData></cx:tx>", f_xml(wb, si, t, names), esc(&v))
    } else {
        let lit = t.trim().strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(t);
        format!("<cx:tx><cx:txData><cx:v>{}</cx:v></cx:txData></cx:tx>", esc(lit))
    }
}

/// The chart style part a chartex part comes with: every element at its plain default.
pub fn style_xml() -> String {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<cs:chartStyle xmlns:cs=\"http://schemas.microsoft.com/office/drawing/2012/chartStyle\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" id=\"201\">",
    );
    for e in [
        "axisTitle",
        "categoryAxis",
        "chartArea",
        "dataLabel",
        "dataLabelCallout",
        "dataPoint",
        "dataPoint3D",
        "dataPointLine",
        "dataPointMarker",
        "dataPointMarkerLayout",
        "dataPointWireframe",
        "dataTable",
        "downBar",
        "dropLine",
        "errorBar",
        "floor",
        "gridlineMajor",
        "gridlineMinor",
        "hiLoLine",
        "leaderLine",
        "legend",
        "plotArea",
        "plotArea3D",
        "seriesAxis",
        "seriesLine",
        "title",
        "trendline",
        "trendlineLabel",
        "upBar",
        "valueAxis",
        "wall",
    ] {
        if e == "dataPointMarkerLayout" {
            s.push_str("<cs:dataPointMarkerLayout symbol=\"circle\" size=\"5\"/>");
        } else {
            let _ = write!(
                s,
                "<cs:{e}><cs:lnRef idx=\"0\"/><cs:fillRef idx=\"0\"/><cs:effectRef idx=\"0\"/><cs:fontRef idx=\"minor\"><a:schemeClr val=\"tx1\"/></cs:fontRef></cs:{e}>"
            );
        }
    }
    s.push_str("</cs:chartStyle>");
    s
}

/// The colour style part a chartex part comes with: the theme accents in turn.
pub fn colors_xml() -> String {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<cs:colorStyle xmlns:cs=\"http://schemas.microsoft.com/office/drawing/2012/chartStyle\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" meth=\"cycle\" id=\"10\">",
    );
    for i in 1..=6 {
        let _ = write!(s, "<a:schemeClr val=\"accent{i}\"/>");
    }
    s.push_str("<cs:variation/></cs:colorStyle>");
    s
}

/// A solid fill (and outline, for line-drawn parts like whiskers) as `cx:spPr`.
fn fill_xml(color: &str, line: bool) -> String {
    let ln = if line { format!("<a:ln w=\"9525\"><a:solidFill>{color}</a:solidFill></a:ln>") } else { String::new() };
    format!("<cx:spPr><a:solidFill>{color}</a:solidFill>{ln}</cx:spPr>")
}

/// Light grey for gridlines and the chart border: the text colour at a quarter strength.
const GREY: &str = "<a:schemeClr val=\"tx1\"><a:lumMod val=\"25000\"/><a:lumOff val=\"75000\"/></a:schemeClr>";

pub(crate) fn accent(i: usize) -> String {
    format!("<a:schemeClr val=\"accent{}\"/>", i % 6 + 1)
}

/// Chartex part XML for a model chart on sheet `si`. The references it uses are appended to
/// `names` (see [`NAME_PREFIX`]).
pub fn write_chartex(wb: &Workbook, si: usize, ch: &Chart, names: &mut Vec<String>) -> String {
    // A part read from a file is written back as it was when nothing changed, and patched field by
    // field when something the part stores changed: everything GridCraft does not model in it
    // survives. A change it cannot express regenerates the part from the model.
    if let Some(text) = kept_part(wb, si, ch, names) {
        return text;
    }
    let hierarchy = matches!(ch.kind, ChartKind::Treemap | ChartKind::Sunburst);
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<cx:chartSpace xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:cx=\"http://schemas.microsoft.com/office/drawing/2014/chartex\"><cx:chartData>",
    );
    for (i, ser) in ch.series.iter().enumerate() {
        let _ = write!(s, "<cx:data id=\"{i}\">");
        if let Some(c) = ser.categories.as_deref().filter(|_| ch.kind != ChartKind::Histogram) {
            let _ = write!(s, "<cx:strDim type=\"cat\">{}</cx:strDim>", f_xml(wb, si, c, names));
        }
        let _ = write!(s, "<cx:numDim type=\"{}\">{}</cx:numDim>", if hierarchy { "size" } else { "val" }, f_xml(wb, si, &ser.values, names));
        s.push_str("</cx:data>");
    }
    s.push_str("</cx:chartData><cx:chart>");
    if let Some(t) = ch.title.as_deref().filter(|t| !t.is_empty()) {
        let _ = write!(s, "<cx:title pos=\"t\" align=\"ctr\" overlay=\"0\"><cx:tx><cx:txData><cx:v>{}</cx:v></cx:txData></cx:tx></cx:title>", esc(t));
    }
    s.push_str("<cx:plotArea><cx:plotAreaRegion>");
    let layout = match ch.kind {
        ChartKind::BoxWhisker => "boxWhisker",
        ChartKind::Waterfall => "waterfall",
        ChartKind::Funnel => "funnel",
        ChartKind::Treemap => "treemap",
        ChartKind::Sunburst => "sunburst",
        _ => "clusteredColumn",
    };
    for (i, ser) in ch.series.iter().enumerate() {
        let _ = write!(s, "<cx:series layoutId=\"{layout}\" uniqueId=\"{{00000000-0000-0000-0000-{:08X}{i:04X}}}\">", ch.id);
        if let Some(n) = &ser.name {
            s.push_str(&tx_xml(n, wb, si, names));
        }
        // Fills are written out: they don't come from the style part. Waterfall decreases and
        // treemap/sunburst boxes get their own colours.
        let color = ser.color.as_ref().and_then(|c| srgb(c, &wb.theme)).unwrap_or_else(|| accent(i));
        s.push_str(&fill_xml(&color, ch.kind == ChartKind::BoxWhisker));
        let points = || crate::chart::ref_values(wb, si, &ser.values).unwrap_or_default().into_iter().take(10_000).enumerate();
        match ch.kind {
            ChartKind::Waterfall => {
                for (j, v) in points() {
                    if v.as_f64().is_some_and(|n| n < 0.0) {
                        let _ = write!(s, "<cx:dataPt idx=\"{j}\">{}</cx:dataPt>", fill_xml(&accent(i + 1), false));
                    }
                }
            }
            ChartKind::Treemap | ChartKind::Sunburst if ser.color.is_none() => {
                for (j, _) in points() {
                    let _ = write!(s, "<cx:dataPt idx=\"{j}\">{}</cx:dataPt>", fill_xml(&accent(j), false));
                }
            }
            _ => {}
        }
        if let Some(l) = chartex_patch::effective(ch, ser) {
            s.push_str(&chartex_patch::labels_xml(l));
        }
        let _ = write!(s, "<cx:dataId val=\"{i}\"/>");
        match ch.kind {
            ChartKind::Histogram => s.push_str("<cx:layoutPr><cx:binning intervalClosed=\"r\"/></cx:layoutPr>"),
            ChartKind::BoxWhisker => {
                let method = match ser.quartiles {
                    QuartileMethod::Exclusive => "exclusive",
                    QuartileMethod::Inclusive => "inclusive",
                };
                let _ = write!(
                    s,
                    "<cx:layoutPr><cx:visibility meanLine=\"0\" meanMarker=\"1\" nonoutliers=\"0\" outliers=\"1\"/><cx:statistics quartileMethod=\"{method}\"/></cx:layoutPr>"
                );
            }
            ChartKind::Treemap => s.push_str("<cx:layoutPr><cx:parentLabelLayout val=\"overlapping\"/></cx:layoutPr>"),
            _ => {}
        }
        s.push_str("</cx:series>");
    }
    s.push_str("</cx:plotAreaRegion>");
    let gap = match ch.kind {
        ChartKind::Histogram => Some("0"),
        ChartKind::BoxWhisker => Some("1"),
        ChartKind::Waterfall => Some("0.5"),
        ChartKind::Funnel => Some("0.06"),
        _ => None,
    };
    let title = |t: Option<&str>| {
        t.filter(|t| !t.is_empty())
            .map(|t| format!("<cx:title><cx:tx><cx:txData><cx:v>{}</cx:v></cx:txData></cx:tx></cx:title>", esc(t)))
            .unwrap_or_default()
    };
    if let Some(gap) = gap {
        let _ =
            write!(s, "<cx:axis id=\"0\"><cx:catScaling gapWidth=\"{}\"/>{}<cx:tickLabels/></cx:axis>", esc_attr(gap), title(ch.x_title.as_deref()));
        if ch.kind != ChartKind::Funnel {
            let _ = write!(
                s,
                "<cx:axis id=\"1\">{}{}{}<cx:tickLabels/></cx:axis>",
                chartex_patch::val_scaling_xml(&ch.y_axis),
                title(ch.y_title.as_deref()),
                if ch.gridlines {
                    format!("<cx:majorGridlines><cx:spPr><a:ln w=\"9525\"><a:solidFill>{GREY}</a:solidFill></a:ln></cx:spPr></cx:majorGridlines>")
                } else {
                    String::new()
                }
            );
        }
    }
    s.push_str("</cx:plotArea>");
    let pos = match ch.legend {
        LegendPos::None => None,
        LegendPos::Bottom => Some("b"),
        LegendPos::Top => Some("t"),
        LegendPos::Left => Some("l"),
        LegendPos::Right => Some("r"),
    };
    if let Some(p) = pos {
        let _ = write!(s, "<cx:legend pos=\"{p}\" align=\"ctr\" overlay=\"0\"/>");
    }
    // A white chart area with a light border, like other charts.
    let _ = write!(
        s,
        "</cx:chart><cx:spPr><a:solidFill><a:schemeClr val=\"bg1\"/></a:solidFill><a:ln w=\"9525\"><a:solidFill>{GREY}</a:solidFill></a:ln></cx:spPr></cx:chartSpace>"
    );
    s
}

/// The model as the chartex part read it, with its hidden `_xlchart` names resolved to references
/// (the reading workbook kept the names, not the references).
fn resolve_names(as_read: &Chart, names: &[(String, String)]) -> Chart {
    let resolve = |f: &mut String| {
        if let Some((_, r)) = names.iter().find(|(n, _)| n.eq_ignore_ascii_case(f)) {
            *f = r.clone();
        }
    };
    let mut then = as_read.clone();
    for s in &mut then.series {
        resolve(&mut s.values);
        if let Some(c) = &mut s.categories {
            resolve(c);
        }
        if let Some(n) = &mut s.name {
            resolve(n);
        }
    }
    then
}

/// Whether `a` and `b` are the same chart apart from the kept part, the object id and the anchor
/// (which live in the drawing part, not in the chart part).
fn same_chart(a: &Chart, b: &Chart) -> bool {
    let mut a = a.clone();
    let mut b = b.clone();
    for c in [&mut a, &mut b] {
        c.id = 0;
        c.anchor = Anchor::default();
        c.part = None;
    }
    a == b
}

/// The index of a `_xlchart.v1.N` name, when it is one.
fn name_index(name: &str) -> Option<usize> {
    let prefix = NAME_PREFIX.to_ascii_lowercase();
    if !name.to_ascii_lowercase().starts_with(&prefix) {
        return None;
    }
    let n: usize = name.get(prefix.len()..)?.trim().parse().ok()?;
    (n <= 100_000).then_some(n)
}

/// Registers a kept chartex part's hidden `_xlchart` names again and renumbers them to the global
/// sequence the writer allocates (`names`), so the part's data references stay valid. A part whose
/// names are in ascending order — how both Excel and GridCraft write them — keeps its bytes.
fn resolve_part_names(part: &str, table: &[(String, String)], names: &mut Vec<String>) -> Option<String> {
    let roots = crate::chart_patch::scan(part)?;
    let space = roots.iter().find(|n| n.name == "chartSpace")?;
    let prefix = NAME_PREFIX.to_ascii_lowercase();
    // The raw `<cx:f>` texts, in document order, with the byte spans of the element and its text.
    let mut refs: Vec<(usize, usize, usize, usize, &str)> = Vec::new();
    let mut stack: Vec<&crate::chart_patch::Node> = vec![space];
    while let Some(n) = stack.pop() {
        if n.name == "f"
            && let Some(text) = part.get(n.open_end..n.close_start)
            && text.to_ascii_lowercase().starts_with(&prefix)
        {
            refs.push((n.start, n.open_end, n.close_start, n.end, text));
        }
        stack.extend(&n.children);
    }
    refs.sort_by_key(|r| r.0);
    if refs.is_empty() {
        return Some(part.to_string());
    }
    // Each name keeps the index it had (a file numbers its names in any order, and Excel's own
    // order is not the document's); a name whose index is taken by a different reference, or lacks
    // one, is renumbered.
    let mut assigned: Vec<(&str, String)> = Vec::new();
    for (_, _, _, _, old) in &refs {
        if assigned.iter().any(|(o, _)| o.eq_ignore_ascii_case(old)) {
            continue;
        }
        let reference = table.iter().find(|(n, _)| n.eq_ignore_ascii_case(old)).map(|(_, r)| r.clone())?;
        let new = match name_index(old) {
            Some(n) if n >= names.len() && n <= names.len().saturating_add(4096) => {
                // Fill the gap with the references those names stood for, when the file has them.
                while names.len() < n {
                    let gap = format!("{NAME_PREFIX}{}", names.len());
                    let filler = table.iter().find(|(name, _)| name.eq_ignore_ascii_case(&gap)).map(|(_, r)| r.clone());
                    names.push(filler.unwrap_or_else(|| reference.clone()));
                }
                names.push(reference);
                format!("{NAME_PREFIX}{n}")
            }
            Some(n) if names.get(n) == Some(&reference) => format!("{NAME_PREFIX}{n}"),
            _ => {
                let n = names.len();
                names.push(reference);
                format!("{NAME_PREFIX}{n}")
            }
        };
        assigned.push((old, new));
    }
    // Replace the texts that moved (or only the ones that differ).
    let mut out = String::with_capacity(part.len() + 32);
    let mut pos = 0usize;
    for (_, open_end, close_start, end, old) in refs {
        let Some((_, new)) = assigned.iter().find(|(o, _)| o.eq_ignore_ascii_case(old)) else { continue };
        if new == old {
            continue;
        }
        out.push_str(part.get(pos..open_end)?);
        out.push_str(new);
        out.push_str(part.get(close_start..end)?);
        pos = end;
    }
    out.push_str(part.get(pos..)?);
    Some(out)
}

/// A chartex part read from a file, written back unchanged or patched, with its hidden names
/// registered again. `None` when the kept part cannot be used, so the caller regenerates it.
fn kept_part(wb: &Workbook, si: usize, ch: &Chart, names: &mut Vec<String>) -> Option<String> {
    let (part, as_read) = ch.kept_part()?;
    // A chart switched from a classic kind keeps that chart's part; it is not a chartex part.
    if !part.contains("office/drawing/2014/chartex") {
        return None;
    }
    let table = ch.part_chart_names();
    let then = resolve_names(as_read, table);
    let text = if same_chart(&then, ch) { part.to_string() } else { chartex_patch::patch_chartex(part, &then, ch, wb, si)? };
    // A name with no reference (a name the reader skipped) regenerates the part instead; drop any
    // indices reserved before that was found.
    let before = names.len();
    match resolve_part_names(&text, table, names) {
        Some(out) => Some(out),
        None => {
            names.truncate(before);
            None
        }
    }
}

#[cfg(test)]
mod quartile_tests {
    use super::*;

    #[test]
    fn box_whisker_reads_quartile_method_and_defaults() {
        for (statistics, expected) in [
            ("", QuartileMethod::Exclusive),
            ("<cx:statistics/>", QuartileMethod::Exclusive),
            (r#"<cx:statistics quartileMethod="unknown"/>"#, QuartileMethod::Exclusive),
            (r#"<cx:statistics quartileMethod="exclusive"/>"#, QuartileMethod::Exclusive),
            (r#"<cx:statistics quartileMethod="inclusive"/>"#, QuartileMethod::Inclusive),
        ] {
            let xml = format!(
                r#"<cx:chartSpace xmlns:cx="http://schemas.microsoft.com/office/drawing/2014/chartex"><cx:chart><cx:plotArea><cx:plotAreaRegion><cx:series layoutId="boxWhisker"><cx:layoutPr>{statistics}</cx:layoutPr></cx:series></cx:plotAreaRegion></cx:plotArea></cx:chart></cx:chartSpace>"#
            );
            let root = crate::xml::parse(xml.as_bytes()).unwrap();
            let chart = read_chartex(&root).unwrap();
            assert_eq!(chart.series[0].quartiles, expected);
        }
    }
}
