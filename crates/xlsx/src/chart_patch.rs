//! Patching an edit into a kept chart part.
//!
//! A chart read from a file keeps its part (`Chart::part`). Writing it back unchanged is byte for
//! byte; this module writes back an *edited* chart by patching only the elements the edit touched,
//! so every option GridCraft has no field for (manual layout, 3-D walls, leader lines, title run
//! formatting, `extLst`, print settings…) is still there afterwards. Only a change the part cannot
//! express — a different chart kind, a different set of series — falls back to regenerating the
//! part from the model.
//!
//! The patch works on byte spans of the kept XML: a changed element is replaced, a removed one is
//! dropped and a new one is inserted at its schema position; everything else is copied verbatim.
//! File-derived XML is hostile, so every offset is checked and the scan is depth- and size-capped.

use std::collections::BTreeMap;

use gridcraft_model::{AxisCrossing, AxisOptions, Chart, DisplayUnits, LabelOptions, PointFormat, PointLabel, Series, Workbook};

use crate::chart::{
    MAX_POINTS, cat_data, dlbls_xml, err_bars_children, label_children, num_data, point_xml, scaling_xml, series_sp_pr, series_tx_xml, title_xml,
    trendline_xml,
};
use crate::drawing::srgb;
use crate::xml::{esc, esc_attr, num};

/// Deepest element nesting the span scanner accepts in a chart part.
const MAX_DEPTH: usize = 64;
/// Most elements the span scanner accepts in a chart part.
const MAX_NODES: usize = 50_000;
/// Largest chart part the patcher works on. A real chart part is a few kilobytes; a huge one (a
/// hostile file, or a cache with a million points) is regenerated from the model instead, so the
/// span tree and the rendered copy stay bounded.
pub(crate) const MAX_PATCH_BYTES: usize = 4 * 1024 * 1024;

// ---------------------------------------------------------------- span tree

/// An element of the kept XML: where it starts and ends, so it can be copied or replaced byte for
/// byte. Children are the direct children only.
#[derive(Clone, Debug)]
pub(crate) struct Node {
    pub(crate) name: String,
    /// Offset of `<`.
    pub(crate) start: usize,
    /// Offset just past the element's name in the opening tag (its attributes start here).
    name_end: usize,
    /// Offset just past the opening tag's `>`.
    pub(crate) open_end: usize,
    /// Offset of the closing `</name>` (equal to `open_end` when self-closing).
    pub(crate) close_start: usize,
    /// Offset just past the element.
    pub(crate) end: usize,
    pub(crate) children: Vec<Node>,
}

fn local_name(raw: &str) -> &str {
    match raw.rfind(':') {
        Some(i) => raw.get(i + 1..).unwrap_or(raw),
        None => raw,
    }
}

/// Index of the `>` that ends the tag starting at `lt`, skipping quoted attribute values.
fn tag_end(xml: &str, lt: usize) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (off, c) in xml.get(lt + 1..)?.char_indices() {
        match (c, quote) {
            ('"' | '\'', None) => quote = Some(c),
            (q, Some(open)) if q == open => quote = None,
            ('>', None) => return Some(lt + 1 + off),
            _ => {}
        }
    }
    None
}

/// Splits a chart part into a span tree. `None` for anything the scanner does not fully
/// understand (an unbalanced or mismatched tag, a part nested too deeply or too large): the caller
/// then regenerates the part from the model.
pub(crate) fn scan(xml: &str) -> Option<Vec<Node>> {
    let mut roots: Vec<Node> = Vec::new();
    let mut stack: Vec<Node> = Vec::new();
    let mut nodes = 0usize;
    let mut i = 0usize;
    while i < xml.len() {
        let Some(off) = xml.get(i..)?.find('<') else { break };
        let lt = off + i;
        let rest = xml.get(lt..)?;
        if rest.starts_with("<!--") {
            i = lt + rest.find("-->")? + 3;
            continue;
        }
        if rest.starts_with("<![CDATA[") {
            i = lt + rest.find("]]>")? + 3;
            continue;
        }
        if rest.starts_with("<?") {
            i = lt + rest.find("?>")? + 2;
            continue;
        }
        if rest.starts_with("<!") {
            i = lt + rest.find('>')? + 1;
            continue;
        }
        if rest.starts_with("</") {
            let gt = lt + rest.find('>')?;
            let mut node = stack.pop()?;
            if local_name(xml.get(lt + 2..gt)?.trim()) != node.name {
                return None;
            }
            node.close_start = lt;
            node.end = gt + 1;
            match stack.last_mut() {
                Some(parent) => parent.children.push(node),
                None => roots.push(node),
            }
            i = gt + 1;
            continue;
        }
        let gt = tag_end(xml, lt)?;
        let self_closing = xml.get(lt..gt).unwrap_or("").trim_end().ends_with('/');
        let name_start = lt + 1;
        let name_len = xml.get(name_start..gt)?.find(|c: char| c.is_whitespace() || c == '/').unwrap_or(gt.saturating_sub(name_start));
        let name = local_name(xml.get(name_start..name_start + name_len)?).to_string();
        nodes += 1;
        if nodes > MAX_NODES {
            return None;
        }
        let node =
            Node { name, start: lt, name_end: name_start + name_len, open_end: gt + 1, close_start: gt + 1, end: gt + 1, children: Vec::new() };
        if self_closing {
            match stack.last_mut() {
                Some(parent) => parent.children.push(node),
                None => roots.push(node),
            }
        } else {
            if stack.len() >= MAX_DEPTH {
                return None;
            }
            stack.push(node);
        }
        i = gt + 1;
    }
    if !stack.is_empty() {
        return None;
    }
    Some(roots)
}

/// Value of attribute `key` of `n` (matched by local name).
pub(crate) fn attr<'a>(xml: &'a str, n: &Node, key: &str) -> Option<&'a str> {
    let body = xml.get(n.name_end..n.open_end)?;
    let mut i = 0usize;
    while let Some(eq) = body.get(i..)?.find('=') {
        let name = body.get(i..i + eq)?.trim();
        let after_eq = i + eq + 1;
        let rest = body.get(after_eq..)?;
        let (qpos, q) = rest.char_indices().find(|(_, c)| *c == '"' || *c == '\'')?;
        let value_start = after_eq + qpos + q.len_utf8();
        let len = body.get(value_start..)?.find(q)?;
        if local_name(name) == key {
            return body.get(value_start..value_start + len);
        }
        i = value_start + len + q.len_utf8();
    }
    None
}

pub(crate) fn child<'a>(n: &'a Node, name: &str) -> Option<&'a Node> {
    n.children.iter().find(|c| c.name == name)
}

/// `n`'s direct children named `name`, in document order.
pub(crate) fn kids<'a>(n: &'a Node, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
    n.children.iter().filter(move |c| c.name == name)
}

// ---------------------------------------------------------------- schema order
//
// A new element has to sit where the schema's sequence puts it, or Excel rejects the part. The
// lists below are the ones the writer already follows (ECMA-376 Part 1, 21.2.2).

const CHARTSPACE_ORDER: &[&str] = &[
    "date1904",
    "lang",
    "roundedCorners",
    "style",
    "clrMapOvr",
    "pivotSource",
    "protection",
    "chart",
    "spPr",
    "txPr",
    "externalData",
    "printSettings",
    "userShapes",
    "extLst",
];
const CHART_ORDER: &[&str] = &[
    "title",
    "autoTitleDeleted",
    "pivotFmts",
    "view3D",
    "floor",
    "sideWall",
    "backWall",
    "plotArea",
    "legend",
    "plotVisOnly",
    "dispBlanksAs",
    "showDLblsOverMax",
    "extLst",
];
const LEGEND_ORDER: &[&str] = &["legendPos", "legendEntry", "layout", "overlay", "spPr", "txPr", "extLst"];
const VALAX_ORDER: &[&str] = &[
    "axId",
    "scaling",
    "delete",
    "axPos",
    "majorGridlines",
    "minorGridlines",
    "title",
    "numFmt",
    "majorTickMark",
    "minorTickMark",
    "tickLblPos",
    "spPr",
    "txPr",
    "crossAx",
    "crosses",
    "crossesAt",
    "crossBetween",
    "majorUnit",
    "minorUnit",
    "dispUnits",
    "extLst",
];
const CATAX_ORDER: &[&str] = &[
    "axId",
    "scaling",
    "delete",
    "axPos",
    "majorGridlines",
    "minorGridlines",
    "title",
    "numFmt",
    "majorTickMark",
    "minorTickMark",
    "tickLblPos",
    "spPr",
    "txPr",
    "crossAx",
    "crosses",
    "crossesAt",
    "auto",
    "lblAlgn",
    "lblOffset",
    "tickLblSkip",
    "tickMarkSkip",
    "noMultiLvlLbl",
    "extLst",
];
const BAR_GROUP_ORDER: &[&str] = &["barDir", "grouping", "varyColors", "ser", "dLbls", "gapWidth", "overlap", "serLines", "axId", "extLst"];
const LINE_GROUP_ORDER: &[&str] =
    &["grouping", "varyColors", "ser", "dLbls", "dropLines", "hiLowLines", "upDownBars", "marker", "smooth", "axId", "extLst"];
const AREA_GROUP_ORDER: &[&str] = &["grouping", "varyColors", "ser", "dLbls", "dropLines", "axId", "extLst"];
const SCATTER_GROUP_ORDER: &[&str] = &["scatterStyle", "varyColors", "ser", "dLbls", "axId", "extLst"];
const PIE_GROUP_ORDER: &[&str] = &["varyColors", "ser", "dLbls", "firstSliceAng", "extLst"];
const DOUGHNUT_GROUP_ORDER: &[&str] = &["varyColors", "ser", "dLbls", "firstSliceAng", "holeSize", "extLst"];
const RADAR_GROUP_ORDER: &[&str] = &["radarStyle", "varyColors", "ser", "dLbls", "axId", "extLst"];
const BUBBLE_GROUP_ORDER: &[&str] = &["varyColors", "ser", "dLbls", "bubble3D", "bubbleScale", "showNegBubbles", "sizeRepresents", "axId", "extLst"];
const STOCK_GROUP_ORDER: &[&str] = &["ser", "dLbls", "dropLines", "hiLowLines", "upDownBars", "axId", "extLst"];
/// The CT_*Ser sequences merged: every kind's own subsequence keeps its order here.
const SER_ORDER: &[&str] = &[
    "idx",
    "order",
    "tx",
    "spPr",
    "invertIfNegative",
    "pictureOptions",
    "marker",
    "dPt",
    "dLbls",
    "trendline",
    "errBars",
    "cat",
    "xVal",
    "val",
    "yVal",
    "bubbleSize",
    "bubble3D",
    "smooth",
    "shape",
    "extLst",
];
const DPT_ORDER: &[&str] = &["idx", "invertIfNegative", "marker", "bubble3D", "explosion", "spPr", "pictureOptions", "extLst"];
const ERRBARS_ORDER: &[&str] = &["errBarType", "errValType", "noEndCap", "plus", "minus", "val", "spPr", "extLst"];
const MARKER_ORDER: &[&str] = &["symbol", "size", "spPr", "extLst"];
const DLBLS_ORDER: &[&str] = &[
    "dLbl",
    "delete",
    "numFmt",
    "spPr",
    "txPr",
    "dLblPos",
    "showLegendKey",
    "showVal",
    "showCatName",
    "showSerName",
    "showPercent",
    "showBubbleSize",
    "separator",
    "showLeaderLines",
    "leaderLines",
    "extLst",
];
/// CT_DLbl's sequence: the index, deletion, the text, the position and the show flags, then the
/// formatting and callout parts the model leaves alone.
const DLABEL_ORDER: &[&str] = &[
    "idx",
    "delete",
    "layout",
    "tx",
    "numFmt",
    "spPr",
    "txPr",
    "dLblPos",
    "showLegendKey",
    "showVal",
    "showCatName",
    "showSerName",
    "showPercent",
    "showBubbleSize",
    "separator",
    "showLeaderLines",
    "leaderLines",
    "extLst",
];

/// The point index a `dLbl` names, if it has one.
fn dlbl_index(xml: &str, n: &Node) -> Option<u32> {
    child(n, "idx").and_then(|i| attr(xml, i, "val")).and_then(|v| v.trim().parse::<u32>().ok())
}

/// A DrawingML shape's properties: the fill comes before the outline.
pub(crate) const SPPR_ORDER: &[&str] = &[
    "xfrm",
    "custGeom",
    "prstGeom",
    "noFill",
    "solidFill",
    "gradFill",
    "blipFill",
    "pattFill",
    "grpFill",
    "ln",
    "effectLst",
    "effectDag",
    "scene3d",
    "sp3d",
    "extLst",
];

/// Which group a series' data lives in, as the part spells it.
#[derive(Clone, Copy, PartialEq)]
enum Shape {
    Scatter,
    Bubble,
    Other,
}

fn group_shape(group: &Node) -> Shape {
    match group.name.as_str() {
        "scatterChart" => Shape::Scatter,
        "bubbleChart" => Shape::Bubble,
        _ => Shape::Other,
    }
}

fn group_order(group: &Node) -> &'static [&'static str] {
    match group.name.as_str() {
        "barChart" | "bar3DChart" => BAR_GROUP_ORDER,
        "lineChart" | "line3DChart" => LINE_GROUP_ORDER,
        "areaChart" | "area3DChart" => AREA_GROUP_ORDER,
        "scatterChart" => SCATTER_GROUP_ORDER,
        "pieChart" | "pie3DChart" | "ofPieChart" => PIE_GROUP_ORDER,
        "doughnutChart" => DOUGHNUT_GROUP_ORDER,
        "radarChart" => RADAR_GROUP_ORDER,
        "bubbleChart" => BUBBLE_GROUP_ORDER,
        "stockChart" => STOCK_GROUP_ORDER,
        _ => BAR_GROUP_ORDER,
    }
}

// ---------------------------------------------------------------- patch plan

/// The edits to one element's children.
#[derive(Default)]
pub(crate) struct ParentEdits {
    order: &'static [&'static str],
    /// Managed child elements: the new text (`Some`) or removal (`None`).
    edits: Vec<(&'static str, Option<String>)>,
    /// Repeated elements added at a schema position (per-point formats, trendlines, deleted labels).
    extra: Vec<(usize, String)>,
    /// Indices of direct children to drop.
    remove: Vec<usize>,
}

impl ParentEdits {
    fn new(order: &'static [&'static str]) -> Self {
        Self { order, ..Self::default() }
    }

    fn set(&mut self, tag: &'static str, text: Option<String>) {
        match self.edits.iter_mut().find(|(t, _)| *t == tag) {
            Some(slot) => slot.1 = text,
            None => self.edits.push((tag, text)),
        }
    }

    fn rank(&self, tag: &str) -> usize {
        self.order.iter().position(|t| *t == tag).unwrap_or(usize::MAX)
    }
}

/// The whole patch: elements to replace and elements whose children change.
#[derive(Default)]
pub(crate) struct Plan {
    replace: BTreeMap<usize, String>,
    children: BTreeMap<usize, ParentEdits>,
    /// Changed attributes (`None` removes one) of an element's opening tag.
    attrs: BTreeMap<usize, Vec<(String, Option<String>)>>,
}

impl Plan {
    pub(crate) fn edits(&mut self, parent: &Node, order: &'static [&'static str]) -> &mut ParentEdits {
        self.children.entry(parent.start).or_insert_with(|| ParentEdits::new(order))
    }

    /// Replaces (or removes) child element `tag` of `parent`.
    pub(crate) fn set(&mut self, parent: &Node, order: &'static [&'static str], tag: &'static str, text: Option<String>) {
        self.edits(parent, order).set(tag, text);
    }

    /// Adds repeated element `xml` at the schema position of `tag`.
    pub(crate) fn add(&mut self, parent: &Node, order: &'static [&'static str], tag: &str, xml: String) {
        let pe = self.edits(parent, order);
        let rank = pe.rank(tag);
        pe.extra.push((rank, xml));
    }

    /// Replaces the whole element `node`.
    pub(crate) fn replace(&mut self, node: &Node, text: String) {
        self.replace.insert(node.start, text);
    }

    /// Sets (or removes, with `None`) one attribute of `node`'s opening tag.
    pub(crate) fn set_attr(&mut self, node: &Node, key: &str, value: Option<String>) {
        let list = self.attrs.entry(node.start).or_default();
        match list.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(key)) {
            Some(slot) => slot.1 = value,
            None => {
                if let Some(v) = value {
                    list.push((key.to_string(), Some(v)));
                }
            }
        }
    }
}

fn push(out: &mut String, text: Option<&str>) {
    if let Some(t) = text {
        out.push_str(t);
    }
}

/// Records that `text` (an element of schema `rank`) goes before the first child after it.
fn place_extra<'a>(before: &mut BTreeMap<usize, Vec<(usize, &'a str)>>, n: &Node, pe: &ParentEdits, text: &'a str, rank: usize) {
    let at = n.children.iter().position(|c| pe.rank(&c.name) > rank).unwrap_or(n.children.len());
    before.entry(at).or_default().push((rank, text));
}

/// The opening tag of `n` with `edits` applied: attributes keep their order, a new one goes last.
/// Rebuilt only for an element whose attributes changed; every other tag is copied as it was.
fn open_tag(xml: &str, n: &Node, edits: &[(String, Option<String>)]) -> String {
    let Some((name, self_closing, mut attrs)) = raw_attrs(xml, n) else {
        // Attributes the parser does not understand (an unquoted value, say): keep the tag's bytes
        // and append the changes before its `>`, so the edit is not silently dropped.
        let raw = xml.get(n.start..n.open_end).unwrap_or_default();
        let self_closing = raw.trim_end().ends_with("/>");
        let mut out = raw.trim_end().trim_end_matches('>').trim_end_matches('/').trim_end().to_string();
        for (key, value) in edits {
            if let Some(v) = value {
                out.push(' ');
                out.push_str(key);
                out.push_str("=\"");
                out.push_str(v);
                out.push('\"');
            }
        }
        out.push_str(if self_closing { "/>" } else { ">" });
        return out;
    };
    let mut removed: Vec<&str> = Vec::new();
    for (key, value) in edits {
        match attrs.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(key)) {
            Some(slot) => match value {
                Some(v) => slot.1 = v.clone(),
                None => removed.push(key.as_str()),
            },
            None => {
                if let Some(v) = value {
                    attrs.push((key.clone(), v.clone()));
                }
            }
        }
    }
    attrs.retain(|(k, _)| !removed.iter().any(|r| r.eq_ignore_ascii_case(k)));
    let mut out = format!("<{name}");
    for (k, v) in attrs {
        out.push(' ');
        out.push_str(&k);
        out.push_str("=\"");
        out.push_str(&v);
        out.push('\"');
    }
    out.push_str(if self_closing { "/>" } else { ">" });
    out
}

/// The name, self-closing flag and attributes of `n`'s opening tag. Malformed attributes make the
/// caller copy the tag unchanged.
fn raw_attrs(xml: &str, n: &Node) -> Option<(String, bool, Vec<(String, String)>)> {
    // `name_end` is just past the qualified name, so the name starts after the `<`.
    let name: String = xml.get(n.start + 1..n.name_end)?.to_string();
    let body = xml.get(n.name_end..n.open_end)?;
    let self_closing = body.trim_end().trim_end_matches('>').trim_end().ends_with('/');
    let mut attrs: Vec<(String, String)> = Vec::new();
    let mut i = 0usize;
    loop {
        while body.as_bytes().get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        let tail = body.get(i..)?;
        if tail.is_empty() || tail.starts_with('/') || tail.starts_with('>') {
            break;
        }
        let key_end = tail.find(|c: char| c.is_whitespace() || c == '=' || c == '/' || c == '>')?;
        let key: String = tail.get(..key_end)?.to_string();
        i += key_end;
        while body.as_bytes().get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if body.as_bytes().get(i) != Some(&b'=') {
            return None;
        }
        i += 1;
        while body.as_bytes().get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        let q = *body.as_bytes().get(i)?;
        if q != b'"' && q != b'\'' {
            return None;
        }
        i += 1;
        let after = body.get(i..)?;
        let len = after.find(q as char)?;
        attrs.push((key, after.get(..len)?.to_string()));
        i += len + 1;
    }
    Some((name, self_closing, attrs))
}

/// [`render`] for a root the caller reached through [`scan`] itself.
pub(crate) fn render_node(xml: &str, n: &Node, plan: &Plan) -> String {
    render(xml, n, plan)
}

/// Renders `n` with the plan applied. Every byte outside a patched element is copied as it was:
/// the gaps between children, the opening tags and the text all come straight from the input.
fn render(xml: &str, n: &Node, plan: &Plan) -> String {
    if let Some(r) = plan.replace.get(&n.start) {
        return r.clone();
    }
    let open = match plan.attrs.get(&n.start) {
        Some(edits) => open_tag(xml, n, edits),
        None => xml.get(n.start..n.open_end).unwrap_or_default().to_string(),
    };
    let empty = ParentEdits::default();
    let pe = plan.children.get(&n.start).unwrap_or(&empty);
    // New elements go before the first child the schema puts after them.
    let mut before: BTreeMap<usize, Vec<(usize, &str)>> = BTreeMap::new();
    for (tag, text) in &pe.edits {
        if let Some(text) = text.as_deref()
            && !n.children.iter().any(|c| c.name == *tag)
        {
            place_extra(&mut before, n, pe, text, pe.rank(tag));
        }
    }
    for (rank, text) in &pe.extra {
        place_extra(&mut before, n, pe, text, *rank);
    }
    let mut out = String::with_capacity(n.end.saturating_sub(n.start) + 64);
    out.push_str(&open);
    let mut pos = n.open_end;
    for (i, c) in n.children.iter().enumerate() {
        if pe.remove.contains(&i) {
            pos = c.end;
            continue;
        }
        push(&mut out, xml.get(pos..c.start));
        if let Some(list) = before.get_mut(&i) {
            list.sort_by_key(|(r, _)| *r);
            for (_, t) in list.iter() {
                out.push_str(t);
            }
        }
        match pe.edits.iter().find(|(t, _)| *t == c.name) {
            Some((_, Some(text))) => out.push_str(text),
            Some((_, None)) => {}
            None => out.push_str(&render(xml, c, plan)),
        }
        pos = c.end;
    }
    push(&mut out, xml.get(pos..n.close_start));
    if let Some(list) = before.get_mut(&n.children.len()) {
        list.sort_by_key(|(r, _)| *r);
        for (_, t) in list.iter() {
            out.push_str(t);
        }
    }
    push(&mut out, xml.get(n.close_start..n.end));
    out
}

// ---------------------------------------------------------------- diffing

fn scaling_changed(a: &AxisOptions, b: &AxisOptions) -> bool {
    a.min != b.min || a.max != b.max || a.log_base != b.log_base || a.reversed != b.reversed
}

fn display_unit_name(u: DisplayUnits) -> &'static str {
    use DisplayUnits::*;
    match u {
        Hundreds => "hundreds",
        Thousands => "thousands",
        TenThousands => "tenThousands",
        HundredThousands => "hundredThousands",
        Millions => "millions",
        TenMillions => "tenMillions",
        HundredMillions => "hundredMillions",
        Billions => "billions",
        Trillions => "trillions",
    }
}

/// Patches the children of an axis element that `now` changed.
#[allow(clippy::too_many_arguments)]
fn patch_axis(
    plan: &mut Plan,
    a: &Node,
    then: &AxisOptions,
    now: &AxisOptions,
    order: &'static [&'static str],
    cat: bool,
    then_title: Option<&String>,
    now_title: Option<&String>,
) {
    if then_title != now_title {
        plan.set(a, order, "title", now_title.map(|t| title_xml(t)));
    }
    if scaling_changed(then, now) {
        plan.set(a, order, "scaling", Some(scaling_xml(now)));
    }
    if then.number_format != now.number_format {
        let text = match now.number_format.as_deref().filter(|c| !c.is_empty()) {
            Some(code) => format!("<c:numFmt formatCode=\"{}\" sourceLinked=\"0\"/>", esc_attr(code)),
            None => "<c:numFmt formatCode=\"General\" sourceLinked=\"1\"/>".to_string(),
        };
        plan.set(a, order, "numFmt", Some(text));
    }
    if then.crosses != now.crosses {
        let (crosses, at) = match now.crosses {
            AxisCrossing::Min => (Some("<c:crosses val=\"min\"/>".to_string()), None),
            AxisCrossing::Max => (Some("<c:crosses val=\"max\"/>".to_string()), None),
            AxisCrossing::At(v) if v.is_finite() => (None, Some(format!("<c:crossesAt val=\"{}\"/>", num(v)))),
            _ => (Some("<c:crosses val=\"autoZero\"/>".to_string()), None),
        };
        plan.set(a, order, "crosses", crosses);
        plan.set(a, order, "crossesAt", at);
    }
    if then.major_unit != now.major_unit {
        if cat {
            let text = now
                .major_unit
                .filter(|u| u.is_finite() && *u >= 1.0)
                .map(|u| format!("<c:tickLblSkip val=\"{}\"/>", u.round().clamp(1.0, 31_999.0) as u32));
            plan.set(a, order, "tickLblSkip", text);
        } else {
            let text = now.major_unit.filter(|u| u.is_finite() && *u > 0.0).map(|u| format!("<c:majorUnit val=\"{}\"/>", num(u)));
            plan.set(a, order, "majorUnit", text);
        }
    }
    if !cat && then.display_units != now.display_units {
        let text = now.display_units.map(|u| format!("<c:dispUnits><c:builtInUnit val=\"{}\"/></c:dispUnits>", display_unit_name(u)));
        plan.set(a, order, "dispUnits", text);
    }
}

/// The `<c:dLbls>` children the model sets from its label options (everything except the repeated
/// `dLbl` markers and the callouts' own formatting).
fn label_option_tags() -> [&'static str; 9] {
    ["numFmt", "dLblPos", "showLegendKey", "showVal", "showCatName", "showSerName", "showPercent", "showBubbleSize", "separator"]
}

/// The `dLbl` children the model owns: the point's index, its deletion and its label. Anything else
/// in the element (layout, text formatting, a callout's leader lines) is somebody else's and stays.
fn is_own_label_child(name: &str) -> bool {
    matches!(name, "idx" | "delete" | "tx" | "dLblPos" | "showVal" | "showCatName" | "showSerName" | "showPercent")
}

/// [`is_own_label_child`] for a whole element: nothing in it would be lost by rebuilding it.
fn is_own_label(n: &Node) -> bool {
    n.children.iter().all(|c| is_own_label_child(&c.name))
}

/// What the model says about one point's label: deletion (the point format) and the label record.
#[derive(Clone, Default, PartialEq)]
struct PointLabelState {
    index: u32,
    no_label: bool,
    label: Option<PointLabel>,
}

/// A group's or a series' label state: the options, plus each point's own label.
#[derive(Clone, Default, PartialEq)]
struct LabelState {
    /// The chart-wide switch is off: the part's label element is marked deleted.
    labels_off: bool,
    options: Option<LabelOptions>,
    points: Vec<PointLabelState>,
}

impl LabelState {
    fn point(&self, index: u32) -> Option<&PointLabelState> {
        self.points.iter().find(|p| p.index == index)
    }
}

/// The label options and per-point labels each side writes for a group (`series: None`) or a
/// series.
fn label_state(c: &Chart, s: Option<&Series>) -> Option<LabelState> {
    match s {
        None => {
            let options = (c.data_labels && !c.labels.hidden).then(|| c.labels.clone());
            let state = LabelState { labels_off: !c.data_labels, options, points: Vec::new() };
            (state.options.is_some()).then_some(state)
        }
        Some(s) => {
            let options = s.labels.as_ref().filter(|_| c.data_labels).cloned();
            let mut points: Vec<PointLabelState> = Vec::new();
            let mut indices: Vec<u32> = s.points.iter().filter(|p| p.no_label).map(|p| p.index).collect();
            indices.extend(s.point_labels.iter().map(|l| l.index));
            indices.sort_unstable();
            indices.dedup();
            indices.truncate(MAX_POINTS);
            for idx in indices {
                let no_label = s.points.iter().any(|p| p.index == idx && p.no_label);
                let label = s.point_labels.iter().find(|l| l.index == idx).cloned();
                points.push(PointLabelState { index: idx, no_label, label });
            }
            (options.is_some() || !points.is_empty()).then_some(LabelState { labels_off: !c.data_labels, options, points })
        }
    }
}

/// Patches the `<c:dLbls>` of a group or a series.
fn patch_dlbls(
    plan: &mut Plan,
    xml: &str,
    parent: &Node,
    parent_order: &'static [&'static str],
    then: Option<&LabelState>,
    now: Option<&LabelState>,
    wb: &Workbook,
    si: usize,
) {
    let existing = child(parent, "dLbls");
    match (existing, now) {
        (None, None) => {}
        (Some(d), None) => {
            // Labels switched off: the element keeps what GridCraft does not model (leader lines,
            // a callout's text) and hides them the way Excel does, instead of dropping them.
            let pe = plan.edits(d, DLBLS_ORDER);
            pe.set("delete", Some("<c:delete val=\"1\"/>".to_string()));
            for tag in label_option_tags() {
                pe.set(tag, None);
            }
        }
        (None, Some(state)) => {
            let (deleted, labels): (Vec<u32>, Vec<PointLabel>) = own_points(state);
            let mut text = dlbls_xml(state.options.as_ref(), &deleted, &labels, wb, si);
            if state.labels_off {
                // The element the writer builds carries no options; the switch-off flag goes first.
                text = text.replacen("<c:dLbls>", "<c:dLbls><c:delete val=\"1\"/>", 1);
            }
            plan.set(parent, parent_order, "dLbls", Some(text));
        }
        (Some(d), Some(state)) => {
            let pe = plan.edits(d, DLBLS_ORDER);
            match state.options.as_ref() {
                Some(l) => {
                    for (tag, text) in label_children(l) {
                        pe.set(tag, text);
                    }
                    if l.hidden {
                        for tag in label_option_tags() {
                            pe.set(tag, None);
                        }
                    }
                }
                None => {
                    pe.set("delete", state.labels_off.then(|| "<c:delete val=\"1\"/>".to_string()));
                    for tag in label_option_tags() {
                        pe.set(tag, None);
                    }
                }
            }
            let before = then.cloned().unwrap_or_default();
            patch_point_labels(plan, xml, d, &before, state, wb, si);
        }
    }
}

/// The deleted indices and label records of a state, as the writer spells them.
fn own_points(state: &LabelState) -> (Vec<u32>, Vec<PointLabel>) {
    let mut deleted = Vec::new();
    let mut labels = Vec::new();
    for p in &state.points {
        if p.no_label {
            deleted.push(p.index);
        }
        if let Some(l) = &p.label {
            labels.push(l.clone());
        }
    }
    (deleted, labels)
}

/// Patches each point's own `<c:dLbl>`: its deletion, its text, position and show flags. Only a
/// point whose model changed is touched, and a `dLbl` the model does not own (a callout layout,
/// rich-text formatting, leader lines) keeps everything outside the model's children.
fn patch_point_labels(plan: &mut Plan, xml: &str, dlbls: &Node, then: &LabelState, now: &LabelState, wb: &Workbook, si: usize) {
    let mut indices: Vec<u32> = then.points.iter().chain(&now.points).map(|p| p.index).collect();
    indices.sort_unstable();
    indices.dedup();
    indices.truncate(MAX_POINTS);
    for idx in indices {
        let a = then.point(idx);
        let b = now.point(idx);
        if a == b {
            continue;
        }
        let node = dlbls.children.iter().find(|n| n.name == "dLbl" && dlbl_index(xml, n) == Some(idx));
        match (node, b) {
            (Some(n), Some(b)) => {
                let pe = plan.edits(n, DLABEL_ORDER);
                pe.set("delete", b.no_label.then(|| "<c:delete val=\"1\"/>".to_string()));
                let label = b.label.as_ref().filter(|_| !b.no_label);
                pe.set("tx", label.and_then(|l| l.text.as_deref()).map(|t| crate::chart::label_tx_xml(t, wb, si)));
                pe.set(
                    "dLblPos",
                    label
                        .map(|l| l.position)
                        .filter(|p| *p != gridcraft_model::LabelPosition::Auto)
                        .map(|p| format!("<c:dLblPos val=\"{}\"/>", crate::chart::label_position_file(p))),
                );
                for (tag, value) in [
                    ("showVal", label.and_then(|l| l.show_value)),
                    ("showCatName", label.and_then(|l| l.show_category)),
                    ("showSerName", label.and_then(|l| l.show_series)),
                    ("showPercent", label.and_then(|l| l.show_percentage)),
                ] {
                    pe.set(tag, value.map(|v| format!("<c:{tag} val=\"{}\"/>", v as u8)));
                }
            }
            (Some(n), None) => {
                // The point no longer carries a label: drop the children the model owns. When
                // nothing else is in the element, the whole `dLbl` goes.
                if is_own_label(n) {
                    if let Some(i) = dlbls.children.iter().position(|c| c.start == n.start) {
                        plan.edits(dlbls, DLBLS_ORDER).remove.push(i);
                    }
                } else {
                    let pe = plan.edits(n, DLABEL_ORDER);
                    for tag in ["delete", "tx", "dLblPos", "showVal", "showCatName", "showSerName", "showPercent"] {
                        pe.set(tag, None);
                    }
                }
            }
            (None, Some(b)) => {
                let label = b.label.as_ref();
                plan.add(dlbls, DLBLS_ORDER, "dLbl", crate::chart::dlbl_xml(idx, b.no_label, label, wb, si));
            }
            (None, None) => {}
        }
    }
}

/// Recolours a shape's properties in place: its own fill, or the fill of its outline when the fill
/// is what the outline carries (a line series). Everything else in `spPr` — width, dash, joins,
/// effects — is kept.
fn patch_sp_pr_color(plan: &mut Plan, sp: &Node, color: &str) {
    let fill = format!("<a:solidFill>{color}</a:solidFill>");
    if let Some(f) = child(sp, "solidFill") {
        plan.replace(f, fill);
        return;
    }
    if let Some(ln) = child(sp, "ln") {
        if let Some(f) = child(ln, "solidFill") {
            plan.replace(f, fill);
        } else {
            plan.set(ln, SPPR_ORDER, "noFill", None);
            plan.set(ln, SPPR_ORDER, "solidFill", Some(fill));
        }
        return;
    }
    plan.set(sp, SPPR_ORDER, "solidFill", Some(fill));
}

/// The series colour, patched where the format keeps it: a marker's fill on a series whose colour
/// is its marker, the outline (line series) or the shape fill otherwise.
fn patch_series_color(plan: &mut Plan, xml: &str, ser: &Node, now: &Series, wb: &Workbook) {
    let Some(color) = now.color.as_ref().and_then(|c| srgb(c, &wb.theme)) else { return };
    let marker = child(ser, "marker").filter(|m| child(m, "symbol").and_then(|s| attr(xml, s, "val")).is_some_and(|v| v != "none"));
    if let Some(m) = marker {
        match child(m, "spPr") {
            Some(sp) => patch_sp_pr_color(plan, sp, &color),
            None => plan.set(m, MARKER_ORDER, "spPr", Some(format!("<c:spPr><a:solidFill>{color}</a:solidFill></c:spPr>"))),
        }
        return;
    }
    match child(ser, "spPr") {
        Some(sp) => patch_sp_pr_color(plan, sp, &color),
        None => {
            let line_like = child(ser, "marker").is_some();
            if let Some(text) = series_sp_pr(now, line_like, wb) {
                plan.set(ser, SER_ORDER, "spPr", Some(text));
            }
        }
    }
}

/// A point format that the part records (`dPt`): one with a colour. A format-less entry is how a
/// deleted label is stored, and belongs to the series' `dLbls` instead.
fn has_format(p: &PointFormat) -> bool {
    p.fill.is_some() || p.line.is_some() || p.marker.is_some() || p.marker_size.is_some()
}

fn point_sp_pr(p: &PointFormat, wb: &Workbook) -> Option<String> {
    let fill = p.fill.as_ref().and_then(|c| srgb(c, &wb.theme));
    let line = p.line.as_ref().and_then(|c| srgb(c, &wb.theme));
    if fill.is_none() && line.is_none() {
        return None;
    }
    let mut s = String::from("<c:spPr>");
    if let Some(c) = fill {
        s.push_str(&format!("<a:solidFill>{c}</a:solidFill>"));
    }
    if let Some(c) = line {
        s.push_str(&format!("<a:ln w=\"12700\"><a:solidFill>{c}</a:solidFill></a:ln>"));
    }
    s.push_str("</c:spPr>");
    Some(s)
}

fn patch_points(plan: &mut Plan, xml: &str, ser: &Node, then: &[PointFormat], now: &[PointFormat], wb: &Workbook) {
    if then == now {
        return;
    }
    let marker_capable = child(ser, "marker").and_then(|m| child(m, "symbol")).and_then(|s| attr(xml, s, "val")).is_some_and(|v| v != "none");
    let nodes: Vec<&Node> = ser.children.iter().filter(|c| c.name == "dPt").collect();
    for n in &nodes {
        let Some(idx) = child(n, "idx").and_then(|x| attr(xml, x, "val")).and_then(|v| v.trim().parse::<u32>().ok()) else { continue };
        let before = then.iter().find(|p| p.index == idx);
        let after = now.iter().find(|p| p.index == idx);
        let (Some(before), Some(after)) = (before, after) else {
            // The point's format was cleared: drop the colours the model owns and keep the rest of
            // the element (invert-if-negative, explosion, picture options). A label-only entry
            // never had a `dPt` to begin with.
            if before.is_some_and(has_format) {
                match child(n, "spPr") {
                    Some(sp) if sp.children.iter().all(|c| matches!(c.name.as_str(), "solidFill" | "noFill" | "ln")) => {
                        plan.set(n, DPT_ORDER, "spPr", None);
                    }
                    Some(sp) => {
                        plan.set(sp, SPPR_ORDER, "solidFill", None);
                        plan.set(sp, SPPR_ORDER, "ln", None);
                    }
                    None => {}
                }
                plan.set(n, DPT_ORDER, "marker", None);
            }
            continue;
        };
        if before.fill != after.fill || before.line != after.line {
            match child(n, "spPr") {
                Some(sp) => {
                    if before.fill != after.fill {
                        let fill = after.fill.as_ref().and_then(|c| srgb(c, &wb.theme));
                        plan.set(sp, SPPR_ORDER, "solidFill", fill.map(|c| format!("<a:solidFill>{c}</a:solidFill>")));
                    }
                    if before.line != after.line {
                        let line = after.line.as_ref().and_then(|c| srgb(c, &wb.theme));
                        plan.set(sp, SPPR_ORDER, "ln", line.map(|c| format!("<a:ln w=\"12700\"><a:solidFill>{c}</a:solidFill></a:ln>")));
                    }
                }
                None => {
                    if let Some(text) = point_sp_pr(after, wb) {
                        plan.set(n, DPT_ORDER, "spPr", Some(text));
                    }
                }
            }
        }
        if before.marker != after.marker || before.marker_size != after.marker_size {
            let marker = point_marker(after, wb).filter(|_| marker_capable);
            plan.set(n, DPT_ORDER, "marker", marker);
        }
    }
    for p in now.iter().filter(|p| has_format(p)) {
        if then.iter().any(|x| x.index == p.index && has_format(x)) {
            continue;
        }
        plan.add(ser, SER_ORDER, "dPt", point_xml(p, marker_capable, wb));
    }
}

/// Patches a series' `<c:errBars>`: only the type, direction, caps and amount the model owns, so
/// the bar's own line formatting (`spPr`) survives.
fn patch_err_bars(plan: &mut Plan, ser: &Node, then: &Series, now: &Series, wb: &Workbook, si: usize) {
    let existing = child(ser, "errBars");
    match (existing, now.error_bars.as_ref()) {
        (None, None) => {}
        (Some(_), None) => plan.set(ser, SER_ORDER, "errBars", None),
        (None, Some(e)) => plan.set(ser, SER_ORDER, "errBars", Some(crate::chart::err_bars_xml(e, wb, si))),
        (Some(n), Some(e)) => {
            let pe = plan.edits(n, ERRBARS_ORDER);
            for (tag, text) in err_bars_children(e, wb, si) {
                pe.set(tag, text);
            }
            if then.error_bars.as_ref().is_some_and(|b| matches!(b.kind, gridcraft_model::ErrorBarKind::Custom { .. }))
                && !matches!(e.kind, gridcraft_model::ErrorBarKind::Custom { .. })
            {
                pe.set("plus", None);
                pe.set("minus", None);
            }
        }
    }
}

fn point_marker(p: &PointFormat, wb: &Workbook) -> Option<String> {
    let c = p.marker.as_ref().and_then(|c| srgb(c, &wb.theme));
    if c.is_none() && p.marker_size.is_none() {
        return None;
    }
    let size = p.marker_size.unwrap_or(5).clamp(2, 72);
    let sp = c.map(|c| format!("<c:spPr><a:solidFill>{c}</a:solidFill></c:spPr>")).unwrap_or_default();
    Some(format!("<c:marker><c:symbol val=\"circle\"/><c:size val=\"{size}\"/>{sp}</c:marker>"))
}

fn patch_trendlines(plan: &mut Plan, xml: &str, ser: &Node, then: &Series, now: &Series, wb: &Workbook) {
    let nodes: Vec<(usize, &Node)> = ser.children.iter().enumerate().filter(|(_, c)| c.name == "trendline").collect();
    let _ = xml;
    if then.trendlines.len() == now.trendlines.len() && nodes.len() == now.trendlines.len() {
        for ((_, n), (a, b)) in nodes.iter().zip(then.trendlines.iter().zip(&now.trendlines)) {
            if a != b {
                plan.replace(n, trendline_xml(b, wb));
            }
        }
        return;
    }
    for (i, _) in &nodes {
        plan.edits(ser, SER_ORDER).remove.push(*i);
    }
    for t in &now.trendlines {
        plan.add(ser, SER_ORDER, "trendline", trendline_xml(t, wb));
    }
}

#[allow(clippy::too_many_arguments)]
fn patch_series(
    plan: &mut Plan,
    xml: &str,
    ser: &Node,
    shape: Shape,
    then_chart: &Chart,
    now_chart: &Chart,
    then: &Series,
    now: &Series,
    wb: &Workbook,
    si: usize,
) {
    if then.name != now.name {
        let text = series_tx_xml(now, wb, si);
        plan.set(ser, SER_ORDER, "tx", (!text.is_empty()).then_some(text));
    }
    if then.color != now.color {
        patch_series_color(plan, xml, ser, now, wb);
    }
    if then.categories != now.categories {
        match shape {
            Shape::Scatter | Shape::Bubble => {
                plan.set(ser, SER_ORDER, "xVal", now.categories.as_deref().map(|c| num_data("xVal", Some(c), wb, si)));
            }
            Shape::Other => {
                plan.set(ser, SER_ORDER, "cat", now.categories.as_deref().map(|c| cat_data(c, &now.values, wb, si)));
            }
        }
    }
    if then.values != now.values {
        let tag = if shape == Shape::Other { "val" } else { "yVal" };
        plan.set(ser, SER_ORDER, tag, Some(num_data(tag, Some(&now.values), wb, si)));
    }
    if then.bubble_sizes != now.bubble_sizes && shape == Shape::Bubble {
        plan.set(ser, SER_ORDER, "bubbleSize", Some(num_data("bubbleSize", now.bubble_sizes.as_deref().or(Some(&now.values)), wb, si)));
    }
    if then.smooth != now.smooth && shape != Shape::Bubble {
        plan.set(ser, SER_ORDER, "smooth", Some(format!("<c:smooth val=\"{}\"/>", now.smooth as u8)));
    }
    if then.trendlines != now.trendlines {
        patch_trendlines(plan, xml, ser, then, now, wb);
    }
    if then.error_bars != now.error_bars {
        patch_err_bars(plan, ser, then, now, wb, si);
    }
    patch_points(plan, xml, ser, &then.points, &now.points, wb);
    let before = label_state(then_chart, Some(then));
    let after = label_state(now_chart, Some(now));
    if before != after {
        patch_dlbls(plan, xml, ser, SER_ORDER, before.as_ref(), after.as_ref(), wb, si);
    }
}

// ---------------------------------------------------------------- entry point

/// Patches `now`'s edits into the kept chart part `xml`, which was read as `then`. `None` when the
/// change is not one a patch can express (a different kind or set of series), so the caller writes
/// the part from the model instead.
pub fn patch_chart(xml: &str, then: &Chart, now: &Chart, wb: &Workbook, si: usize, book_name: &str) -> Option<String> {
    if xml.len() > MAX_PATCH_BYTES || then.kind != now.kind || then.series.len() != now.series.len() {
        return None;
    }
    for (a, b) in then.series.iter().zip(now.series.iter()) {
        if a.secondary != b.secondary || a.kind != b.kind {
            return None;
        }
    }
    let roots = scan(xml)?;
    let space = roots.iter().find(|n| n.name == "chartSpace")?;
    // Everything written here is `c:`-prefixed: only patch a part whose root declares it.
    if !xml.get(space.start..space.open_end)?.contains("xmlns:c=") {
        return None;
    }
    let chart = child(space, "chart")?;
    let plot = child(chart, "plotArea")?;
    let mut plan = Plan::default();

    if then.style != now.style {
        let text = (1..=48).contains(&now.style).then(|| format!("<c:style val=\"{}\"/>", now.style));
        plan.set(space, CHARTSPACE_ORDER, "style", text);
    }
    if then.pivot != now.pivot {
        let text = now.pivot.as_deref().map(str::trim).filter(|l| !l.is_empty()).map(|link| {
            let link = crate::chart::strip_book(link);
            format!("<c:pivotSource><c:name>[{}]{}</c:name><c:fmtId val=\"0\"/></c:pivotSource>", esc(book_name), esc(link))
        });
        plan.set(space, CHARTSPACE_ORDER, "pivotSource", text);
    }
    if then.title != now.title {
        let text = now.title.as_deref().filter(|t| !t.is_empty()).map(title_xml);
        let deleted = text.is_none();
        plan.set(chart, CHART_ORDER, "title", text);
        plan.set(chart, CHART_ORDER, "autoTitleDeleted", Some(format!("<c:autoTitleDeleted val=\"{}\"/>", deleted as u8)));
    }
    if then.legend != now.legend {
        let pos = crate::chart::legend_pos_file(now.legend);
        match (child(chart, "legend"), pos) {
            (Some(l), Some(p)) => plan.set(l, LEGEND_ORDER, "legendPos", Some(format!("<c:legendPos val=\"{p}\"/>"))),
            (Some(_), None) => plan.set(chart, CHART_ORDER, "legend", None),
            (None, Some(p)) => {
                plan.set(chart, CHART_ORDER, "legend", Some(format!("<c:legend><c:legendPos val=\"{p}\"/><c:overlay val=\"0\"/></c:legend>")))
            }
            (None, None) => {}
        }
    }

    // Series, group by group, in the order the reader flattened them.
    let groups: Vec<&Node> = plot
        .children
        .iter()
        .filter(|n| {
            matches!(
                n.name.as_str(),
                "barChart"
                    | "bar3DChart"
                    | "lineChart"
                    | "line3DChart"
                    | "areaChart"
                    | "area3DChart"
                    | "pieChart"
                    | "pie3DChart"
                    | "ofPieChart"
                    | "doughnutChart"
                    | "scatterChart"
                    | "bubbleChart"
                    | "radarChart"
                    | "stockChart"
            )
        })
        .collect();
    let sers: Vec<(&Node, Shape)> = groups.iter().flat_map(|g| kids(g, "ser").map(move |s| (s, group_shape(g)))).collect();
    if sers.len() != now.series.len() {
        return None;
    }
    for (i, (ser, shape)) in sers.iter().enumerate() {
        patch_series(&mut plan, xml, ser, *shape, then, now, then.series.get(i)?, now.series.get(i)?, wb, si);
    }

    // Chart-level labels: one `<c:dLbls>` per group.
    let then_group = label_state(then, None);
    let now_group = label_state(now, None);
    if then_group.as_ref().map(|s| s.options.as_ref().map(label_children)) != now_group.as_ref().map(|s| s.options.as_ref().map(label_children)) {
        for g in &groups {
            patch_dlbls(&mut plan, xml, g, group_order(g), then_group.as_ref(), now_group.as_ref(), wb, si);
        }
    }

    // Axes. The primary pair is the first group's; the secondary pair is the first later group
    // plotting against other axes (a combo chart's right-hand axis).
    let ax_ids = |g: &Node| -> Vec<String> { kids(g, "axId").filter_map(|a| attr(xml, a, "val")).map(str::to_string).collect() };
    let first_axes = groups.first().map(|g| ax_ids(g)).unwrap_or_default();
    let axis = |id: &str| -> Option<&Node> {
        plot.children
            .iter()
            .find(|n| matches!(n.name.as_str(), "valAx" | "catAx" | "dateAx") && child(n, "axId").and_then(|a| attr(xml, a, "val")) == Some(id))
    };
    let x_ax = first_axes.first().and_then(|id| axis(id));
    let y_ax = first_axes.get(1).and_then(|id| axis(id));
    let second_axes = groups.iter().skip(1).map(|g| ax_ids(g)).find(|ids| !ids.is_empty() && *ids != first_axes);
    let y2_ax = second_axes.as_ref().and_then(|ids| ids.get(1)).and_then(|id| axis(id));
    if then.x_axis != now.x_axis || then.x_title != now.x_title {
        let a = x_ax?;
        let cat = matches!(a.name.as_str(), "catAx" | "dateAx");
        patch_axis(
            &mut plan,
            a,
            &then.x_axis,
            &now.x_axis,
            if cat { CATAX_ORDER } else { VALAX_ORDER },
            cat,
            then.x_title.as_ref(),
            now.x_title.as_ref(),
        );
    }
    if then.y_axis != now.y_axis || then.y_title != now.y_title {
        let a = y_ax?;
        patch_axis(&mut plan, a, &then.y_axis, &now.y_axis, VALAX_ORDER, false, then.y_title.as_ref(), now.y_title.as_ref());
    }
    if then.y2_axis != now.y2_axis {
        let a = y2_ax?;
        patch_axis(&mut plan, a, &then.y2_axis, &now.y2_axis, VALAX_ORDER, false, None, None);
    }
    if then.gridlines != now.gridlines {
        let a = y_ax?;
        plan.set(a, VALAX_ORDER, "majorGridlines", now.gridlines.then(|| "<c:majorGridlines/>".to_string()));
    }

    let mut out = String::with_capacity(xml.len() + 256);
    push(&mut out, xml.get(..space.start));
    out.push_str(&render(xml, space, &plan));
    push(&mut out, xml.get(space.end..));
    Some(out)
}
