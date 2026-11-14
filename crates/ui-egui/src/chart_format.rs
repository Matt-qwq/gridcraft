//! The Format pane's chart sections: the chart area, axes, data labels, error bars and data
//! points. Every control calls one of the engine's chart commands through
//! [`SheetApp::run_or_alert`](crate::SheetApp::run_or_alert); the pane holds no copy of the chart
//! and never edits the model itself, so what a control offers is what the file, the renderer and
//! the command agree on.
//!
//! A control the current chart kind cannot store is disabled with the reason, and the reason is
//! the one `chart.setAxis`, `chart.setDataLabels`, `chart.setErrorBars` or `chart.formatPoint`
//! refuses the option with: the UI does not offer what the command would reject.

use std::ops::RangeInclusive;

use egui::{RichText, Ui, vec2};
use gridcraft_chart::ChartKind;
use gridcraft_engine::model::{
    Anchor, AnchorMode, AxisCrossing, AxisOptions, Chart, Color, DisplayUnits, ErrorBarDirection, ErrorBarKind, Image, LabelFont, LabelOptions,
    LabelPosition, Shape, ShapeTextAlign, ShapeTextAnchor,
};
use serde_json::{Map, Value as Json, json};

use crate::SheetApp;
use crate::theme::{self, Tokens};
use crate::widgets::tr;

/// Why a chartex chart offers no formatting here.
const CHARTEX: &str = "This chart type keeps its options in a chartex part, which GridCraft does not model.";
/// Why a pie has no axis options.
const NO_AXES: &str = "This chart type has no axes.";

/// What the Format pane is editing, from its "Current selection" list (Excel's Chart Elements
/// dropdown).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Selection {
    /// The chart itself: title, type, legend, size and (for all series at once) labels, error
    /// bars and points.
    #[default]
    Chart,
    /// The horizontal axis: the category axis, or the numeric X axis of a scatter or bubble.
    AxisX,
    /// The vertical (value) axis.
    AxisY,
    /// One series: its fill, its labels, its error bars and its points.
    Series(usize),
}

impl Selection {
    /// `chart`, `axisX`, `axisY`, `series1`… (the offscreen snapshot's `--format` and tests).
    pub fn parse(text: &str) -> Selection {
        match text.trim().to_ascii_lowercase().as_str() {
            "axisx" | "x" | "category" => Selection::AxisX,
            "axisy" | "y" | "value" => Selection::AxisY,
            other => match other.strip_prefix("series").and_then(|n| n.parse::<usize>().ok()) {
                Some(n) => Selection::Series(n.saturating_sub(1)),
                None => Selection::Chart,
            },
        }
    }

    fn label(self) -> &'static str {
        match self {
            Selection::Chart => "Chart Area",
            Selection::AxisX => "Horizontal (Category) Axis",
            Selection::AxisY => "Vertical (Value) Axis",
            Selection::Series(_) => "Series",
        }
    }

    fn text(self) -> String {
        match self {
            Selection::Series(i) => format!("Series {}", i + 1),
            other => other.label().to_string(),
        }
    }

    /// The label the pane draws, in the frame's language: `text()` keys the translation and a
    /// series number is substituted by hand, since `format!` needs a literal template.
    fn localized(self, ui: &Ui) -> String {
        match self {
            Selection::Series(i) => tr(ui, "Series {}").replace("{}", &(i + 1).to_string()),
            other => tr(ui, &other.text()).to_string(),
        }
    }
}

/// The `Data Point` section's selected point for `series`, kept in the UI's temporary store so a
/// click on a bar, slice or marker can point the section at it.
fn point_key(chart: u32, series: usize) -> egui::Id {
    egui::Id::new(("gridcraft.chart_point", chart, series))
}

/// Points the pane at what a click on the chart landed on: an axis, a series, or a bar/slice/marker
/// of a series (which also selects that point in the Data Point section). A click on the chart's
/// title, legend blank space or background selects the chart area, as Excel does.
pub fn select_element(app: &mut SheetApp, ui: &mut Ui, chart: u32, hit: gridcraft_chart::Hit) {
    use gridcraft_chart::Hit;
    app.format_selection = match hit {
        Hit::ChartArea => Selection::Chart,
        Hit::AxisX => Selection::AxisX,
        Hit::AxisY => Selection::AxisY,
        Hit::Series { index, point } => {
            if let Some(j) = point {
                ui.data_mut(|d| d.insert_temp(point_key(chart, index), j));
            }
            Selection::Series(index)
        }
    };
}

/// Why the whole axis-scaling group (bounds, log scale, display units, crossing) is unavailable,
/// when one reason covers them all. Individual options report their own reason through
/// [`axis_reason`].
fn axis_group_reason(chart: &Chart, value_axis: bool) -> Option<&'static str> {
    if chart.kind.is_chartex() {
        return Some(CHARTEX);
    }
    if matches!(chart.kind, ChartKind::Pie | ChartKind::Doughnut) {
        return Some(NO_AXES);
    }
    let numeric_cat = matches!(chart.kind, ChartKind::Scatter | ChartKind::ScatterLines | ChartKind::Bubble);
    (!value_axis && !numeric_cat).then_some(
        "A category axis has no bounds, logarithmic scale, display units or crossing; the label interval, reverse order and number format apply.",
    )
}

/// The axis options a control can set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxisOption {
    Bounds,
    MajorUnit,
    LogBase,
    Reversed,
    Crossing,
    DisplayUnits,
    NumberFormat,
}

/// Whether `option` of `chart`'s value (`value_axis`) or category axis can be stored, and if not,
/// why. Mirrors `chart.setAxis`'s refusals.
pub fn axis_reason(chart: &Chart, value_axis: bool, option: AxisOption) -> Option<&'static str> {
    use AxisOption::*;
    if chart.kind.is_chartex() {
        return Some(CHARTEX);
    }
    if matches!(chart.kind, ChartKind::Pie | ChartKind::Doughnut) {
        return Some(NO_AXES);
    }
    let numeric_cat = matches!(chart.kind, ChartKind::Scatter | ChartKind::ScatterLines | ChartKind::Bubble);
    if !value_axis && !numeric_cat {
        return match option {
            LogBase => Some("A category axis has no logarithmic scale."),
            DisplayUnits => Some("Display units apply to a value axis."),
            Bounds => Some("Category bounds are not drawn; only a numeric X axis takes them."),
            Crossing => Some("Where the value axis crosses a category axis is not drawn."),
            MajorUnit | Reversed | NumberFormat => None,
        };
    }
    if value_axis && chart.kind == ChartKind::Radar && option == Crossing {
        return Some("A radar chart has no axis crossing.");
    }
    None
}

/// Whether error bars can be stored and drawn on every series of `chart`. Mirrors
/// `chart.setErrorBars` (2-D column, bar, line, area, scatter and bubble series).
pub fn error_bar_reason(chart: &Chart) -> Option<&'static str> {
    use ChartKind::*;
    if chart.kind.is_chartex() {
        return Some(CHARTEX);
    }
    let drawn = !chart.series.is_empty()
        && (0..chart.series.len()).all(|i| {
            matches!(
                gridcraft_chart::series_plot_kind(chart, i),
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
        });
    (!drawn).then_some("Error bars are drawn on 2-D column, bar, line, area, scatter and bubble series only.")
}

/// Why one series' points cannot be formatted.
pub fn point_reason(chart: &Chart, series: usize) -> Option<&'static str> {
    if chart.kind.is_chartex() {
        return Some(CHARTEX);
    }
    (series >= chart.series.len()).then_some("Select a series first.")
}

/// Why a marker colour cannot be set on a point of `series`.
pub fn marker_reason(chart: &Chart, series: usize) -> Option<&'static str> {
    use ChartKind::*;
    if let Some(r) = point_reason(chart, series) {
        return Some(r);
    }
    let line_like =
        matches!(gridcraft_chart::series_plot_kind(chart, series), Line | LineMarkers | LineStacked | Scatter | ScatterLines | Bubble | Radar);
    (!line_like).then_some("A marker colour applies to line, scatter, bubble and radar points only.")
}

/// Why this chart's label options cannot be stored (only a chartex chart refuses them).
pub fn labels_reason(chart: &Chart) -> Option<&'static str> {
    chart.kind.is_chartex().then_some(CHARTEX)
}

/// A starting interval for "fix the major unit": 1, 2 or 5 times a power of ten.
fn nice_step(step: f64) -> f64 {
    if !step.is_finite() || step <= 0.0 {
        return 1.0;
    }
    let exp = step.log10().floor();
    let base = 10f64.powf(exp);
    let n = step / base;
    let nice = if n < 1.5 {
        1.0
    } else if n < 3.5 {
        2.0
    } else if n < 7.5 {
        5.0
    } else {
        10.0
    };
    nice * base
}

fn units_name(units: Option<DisplayUnits>) -> &'static str {
    match units {
        None => "none",
        Some(DisplayUnits::Hundreds) => "hundreds",
        Some(DisplayUnits::Thousands) => "thousands",
        Some(DisplayUnits::TenThousands) => "tenThousands",
        Some(DisplayUnits::HundredThousands) => "hundredThousands",
        Some(DisplayUnits::Millions) => "millions",
        Some(DisplayUnits::TenMillions) => "tenMillions",
        Some(DisplayUnits::HundredMillions) => "hundredMillions",
        Some(DisplayUnits::Billions) => "billions",
        Some(DisplayUnits::Trillions) => "trillions",
    }
}

/// The `chart.setAxis` params that describe `chart`'s axis as it stands, holding only what this
/// axis can store. Sending them with `reset: true` restores the axis, and dropping one key clears
/// that option (the command has no per-field "automatic").
pub fn axis_params(chart: &Chart, value_axis: bool) -> Map<String, Json> {
    let o: &AxisOptions = if value_axis { &chart.y_axis } else { &chart.x_axis };
    let mut m = Map::new();
    let numeric = value_axis || matches!(chart.kind, ChartKind::Scatter | ChartKind::ScatterLines | ChartKind::Bubble);
    if numeric && axis_reason(chart, value_axis, AxisOption::Bounds).is_none() {
        if let Some(v) = o.min {
            m.insert("min".into(), json!(v));
        }
        if let Some(v) = o.max {
            m.insert("max".into(), json!(v));
        }
    }
    if let Some(v) = o.major_unit {
        m.insert("majorUnit".into(), json!(v));
    }
    if axis_reason(chart, value_axis, AxisOption::LogBase).is_none() {
        // 0 turns the log scale off.
        m.insert("logBase".into(), json!(o.log_base.unwrap_or(0.0)));
    }
    m.insert("reversed".into(), json!(o.reversed));
    if axis_reason(chart, value_axis, AxisOption::Crossing).is_none() {
        m.insert("crosses".into(), crossing_json(o.crosses));
    }
    if numeric && axis_reason(chart, value_axis, AxisOption::DisplayUnits).is_none() {
        m.insert("displayUnits".into(), json!(units_name(o.display_units)));
    }
    if let Some(code) = o.number_format.as_deref().filter(|c| !c.is_empty()) {
        m.insert("numberFormat".into(), json!(code));
    }
    m
}

/// `chart.setAxis` params that reset `chart`'s axis and put every option back except `drop`.
pub fn clear_axis_params(chart: &Chart, value_axis: bool, axis_name: &str, drop: &str) -> Json {
    let mut params = Map::new();
    params.insert("axis".into(), json!(axis_name));
    params.insert("reset".into(), json!(true));
    for (key, value) in axis_params(chart, value_axis) {
        if key != drop {
            params.insert(key, value);
        }
    }
    Json::Object(params)
}

/// The pane: the Format Chart Area content.
pub fn show(app: &mut SheetApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let Some(id) = app.selected_chart else {
        ui.label(RichText::new(crate::widgets::tr(ui, "Select a chart, shape or picture to format it.")).italics().color(t.text_dim));
        return;
    };
    // A shape or a picture has its own sections; a chart keeps the ones below.
    match picked(app, id) {
        Some(Picked::Shape(sh)) => return shape(app, ui, &sh),
        Some(Picked::Image(im)) => return picture(app, ui, &im),
        None => {}
    }
    let Some(chart) = app.session.active().and_then(|d| d.wb.active().and_then(|s| s.charts.iter().find(|c| c.id == id).cloned())) else {
        ui.label(RichText::new(crate::widgets::tr(ui, "The object no longer exists.")).italics().color(t.text_dim));
        return;
    };
    // A series selection can go stale (Select Data removed one).
    if let Selection::Series(i) = app.format_selection
        && i >= chart.series.len()
    {
        app.format_selection = Selection::Chart;
    }
    let mut selection = app.format_selection;
    egui::ComboBox::from_id_salt(("gridcraft.format_selection", id))
        .selected_text(selection.localized(ui))
        .width(ui.available_width().min(260.0))
        .show_ui(ui, |ui| {
            for option in
                std::iter::once(Selection::Chart).chain([Selection::AxisX, Selection::AxisY]).chain((0..chart.series.len()).map(Selection::Series))
            {
                if ui.selectable_label(selection == option, option.localized(ui)).clicked() {
                    selection = option;
                }
            }
        });
    app.format_selection = selection;
    ui.add_space(2.0);
    match selection {
        Selection::Chart => {
            chart_area(app, ui, &chart, id);
            data_labels(app, ui, &chart, id, None);
            error_bars(app, ui, &chart, id, None);
            data_point(app, ui, &chart, id, None);
        }
        Selection::AxisX => axis(app, ui, &chart, id, false),
        Selection::AxisY => axis(app, ui, &chart, id, true),
        Selection::Series(i) => {
            series_axis(app, ui, &chart, id, i);
            series_fill(app, ui, &chart, id, i);
            data_labels(app, ui, &chart, id, Some(i));
            error_bars(app, ui, &chart, id, Some(i));
            data_point(app, ui, &chart, id, Some(i));
        }
    }
}

/// A collapsing section; a disabled one says why instead of offering controls that would be
/// stored and then ignored (or refused by the command).
fn section(ui: &mut Ui, title: &str, reason: Option<&'static str>, body: impl FnOnce(&mut Ui)) {
    let t = Tokens::get(ui.ctx());
    ui.collapsing(tr(ui, title), |ui| {
        if let Some(r) = reason {
            ui.label(RichText::new(tr(ui, r)).small().color(t.text_dim));
            return;
        }
        body(ui);
    });
}

/// One series' axis: the pane side of `chart.setSeriesAxis` (Change Chart Type → Secondary Axis).
fn series_axis(app: &mut SheetApp, ui: &mut Ui, chart: &Chart, id: u32, i: usize) {
    let Some(ser) = chart.series.get(i) else { return };
    ui.collapsing(tr(ui, "Series Options"), |ui| {
        let mut secondary = ser.secondary;
        if ui.checkbox(&mut secondary, crate::widgets::tr(ui, "Plot on secondary axis")).changed()
            && let Err(e) = app.run("chart.setSeriesAxis", json!({"chart": id, "series": i, "secondary": secondary}))
        {
            app.message = Some(("GridCraft".into(), crate::clean_error(&e)));
        }
        if secondary {
            ui.label(
                RichText::new(crate::widgets::tr(ui, "On the right-hand value axis; the chart becomes a combo chart."))
                    .small()
                    .color(Tokens::get(ui.ctx()).text_dim),
            );
        }
    });
}

/// The chart object itself: title, type, legend, elements, palettes and size.
fn chart_area(app: &mut SheetApp, ui: &mut Ui, chart: &Chart, id: u32) {
    ui.collapsing(tr(ui, "Chart Title"), |ui| {
        let field = egui::Id::new(("gridcraft.chart_title", id));
        if let Some(title) = crate::widgets::committed_text(ui, field, chart.title.as_deref().unwrap_or(""), None) {
            app.run_or_alert("chart.set", json!({"chart": id, "title": title}));
        }
    });
    ui.collapsing(tr(ui, "Chart Type"), |ui| {
        ui.horizontal_wrapped(|ui| {
            // The chart type's name is the label; the kind and subtype stay the command's ids.
            for (l, k, s) in [
                ("Column", "column", ""),
                ("Stacked", "column", "stacked"),
                ("Bar", "bar", ""),
                ("chart|Line", "line", "markers"),
                ("Area", "area", ""),
                ("Pie", "pie", ""),
                ("Doughnut", "doughnut", ""),
                ("Scatter", "scatter", ""),
                ("Radar", "radar", ""),
            ] {
                if ui.button(tr(ui, l)).clicked() {
                    app.run_or_alert("chart.set", json!({"chart": id, "type": k, "subtype": s}));
                }
            }
        });
    });
    ui.collapsing(tr(ui, "Legend"), |ui| {
        ui.horizontal_wrapped(|ui| {
            for (l, v) in [("None", "none"), ("Bottom", "bottom"), ("Top", "top"), ("Left", "left"), ("Right", "right")] {
                let cur = format!("{:?}", chart.legend).eq_ignore_ascii_case(v);
                if ui.selectable_label(cur, position_text(ui, l)).clicked() {
                    app.run_or_alert("chart.set", json!({"chart": id, "legend": v}));
                }
            }
        });
    });
    ui.collapsing(tr(ui, "Elements"), |ui| {
        let mut dl = chart.data_labels;
        if ui.checkbox(&mut dl, crate::widgets::tr(ui, "Data Labels")).changed() {
            app.run_or_alert("chart.set", json!({"chart": id, "dataLabels": dl}));
        }
        let mut gl = chart.gridlines;
        if ui.checkbox(&mut gl, crate::widgets::tr(ui, "Gridlines")).changed() {
            app.run_or_alert("chart.set", json!({"chart": id, "gridlines": gl}));
        }
        for (label, current, key) in
            [("Horizontal axis title:", chart.x_title.as_deref(), "xTitle"), ("Vertical axis title:", chart.y_title.as_deref(), "yTitle")]
        {
            ui.horizontal(|ui| {
                ui.label(tr(ui, label));
                let field = egui::Id::new(("gridcraft.chart_axis_title", id, key));
                if let Some(text) = crate::widgets::committed_text(ui, field, current.unwrap_or(""), None) {
                    app.run_or_alert("chart.set", json!({"chart": id, key: if text.is_empty() { Json::Null } else { json!(text) }}));
                }
            });
        }
    });
    ui.collapsing(tr(ui, "Colors & Layout"), |ui| {
        ui.horizontal_wrapped(|ui| {
            // The palette's name is the label; the id is what the command receives.
            for p in ["colorful", "monochrome", "accent1", "accent2", "accent3", "accent4", "accent5", "accent6"] {
                if ui.button(tr(ui, p)).clicked() {
                    app.run_or_alert("chart.changeColors", json!({"chart": id, "palette": p}));
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            for n in 1..=6 {
                let label = tr(ui, "Layout {n}").replace("{n}", &n.to_string());
                if ui.button(label).clicked() {
                    app.run_or_alert("chart.quickLayout", json!({"chart": id, "layout": n}));
                }
            }
        });
    });
    ui.collapsing(tr(ui, "chart|Size"), |ui| {
        // Keep the value being typed or dragged between frames; reloading it from the chart every
        // frame threw the edit away.
        let draft = egui::Id::new(("gridcraft.chart_size_draft", id));
        let min = crate::chartview::OBJECT_MIN_SIZE;
        let (mut w, mut h) = ui.data_mut(|d| d.get_temp::<(f32, f32)>(draft)).unwrap_or((chart.anchor.width, chart.anchor.height));
        let a = ui.add(egui::DragValue::new(&mut w).prefix(tr(ui, "Width ")).range(min..=4000.0));
        let b = ui.add(egui::DragValue::new(&mut h).prefix(tr(ui, "Height ")).range(min..=4000.0));
        if a.drag_stopped() || a.lost_focus() || b.drag_stopped() || b.lost_focus() {
            ui.data_mut(|d| d.remove::<(f32, f32)>(draft));
            if !ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                app.run_or_alert("chart.set", json!({"chart": id, "width": w, "height": h}));
            }
        } else if a.dragged() || a.has_focus() || b.dragged() || b.has_focus() {
            ui.data_mut(|d| d.insert_temp(draft, (w, h)));
        } else {
            ui.data_mut(|d| d.remove::<(f32, f32)>(draft));
        }
    });
}

/// One series' own fill, as the Chart Area's Series list had it.
fn series_fill(app: &mut SheetApp, ui: &mut Ui, chart: &Chart, id: u32, i: usize) {
    let theme = app.session.active().map(|d| d.wb.theme.clone()).unwrap_or_default();
    let Some(s) = chart.series.get(i) else { return };
    ui.collapsing(tr(ui, "Series Fill"), |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&s.values).small().color(Tokens::get(ui.ctx()).text_dim));
            ui.menu_button(tr(ui, "Fill…"), |ui| {
                if let Some(c) = crate::widgets::color_palette_fixed(ui, &theme.colors, tr(ui, "Automatic")) {
                    app.run_or_alert(
                        "chart.formatSelection",
                        json!({"chart": id, "series": i, "color": if c == "none" { Json::Null } else { json!(c) }}),
                    );
                    ui.close();
                }
            });
        });
    });
}

/// What a bound row decided: make the option automatic, or fix it at a value.
enum Bound {
    Auto,
    Set(f64),
}

/// Format Axis for the axis the pane's selection names.
fn axis(app: &mut SheetApp, ui: &mut Ui, chart: &Chart, id: u32, value_axis: bool) {
    let axis_name = if value_axis { "y" } else { "x" };
    let o: AxisOptions = if value_axis { chart.y_axis.clone() } else { chart.x_axis.clone() };
    let reason = |option| axis_reason(chart, value_axis, option);
    let group = axis_group_reason(chart, value_axis);
    let t = Tokens::get(ui.ctx());
    let (lo, hi) = data_bounds(app, chart);

    ui.collapsing(tr(ui, "Axis Options"), |ui| {
        if let Some(r) = group {
            ui.label(RichText::new(tr(ui, r)).small().color(t.text_dim));
        }
        if group.is_none() {
            if let Some(b) = bound(ui, egui::Id::new(("chart_axis_min", id, value_axis)), o.min, "Minimum", lo.floor(), -1e15..=1e15) {
                match b {
                    Bound::Auto => app.run_or_alert("chart.setAxis", clear_axis_params(chart, value_axis, axis_name, "min")),
                    Bound::Set(v) => run_axis(app, id, axis_name, json!({"min": v})),
                }
            }
            if let Some(b) = bound(ui, egui::Id::new(("chart_axis_max", id, value_axis)), o.max, "Maximum", hi.ceil(), -1e15..=1e15) {
                match b {
                    Bound::Auto => app.run_or_alert("chart.setAxis", clear_axis_params(chart, value_axis, axis_name, "max")),
                    Bound::Set(v) => run_axis(app, id, axis_name, json!({"max": v})),
                }
            }
        }
        if reason(AxisOption::MajorUnit).is_none() {
            let step = if hi > lo { nice_step((hi - lo) / 5.0) } else { 1.0 };
            let label = if value_axis { "Major unit" } else { "Labels every" };
            if let Some(b) = bound(ui, egui::Id::new(("chart_axis_unit", id, value_axis)), o.major_unit, label, step, 1e-9..=1e15) {
                match b {
                    Bound::Auto => app.run_or_alert("chart.setAxis", clear_axis_params(chart, value_axis, axis_name, "majorUnit")),
                    Bound::Set(v) => run_axis(app, id, axis_name, json!({"majorUnit": v})),
                }
            }
        }
        // Log scale: unchecking sends 0, which the command reads as "linear".
        if group.is_none() {
            ui.horizontal(|ui| {
                let mut log = o.log_base.is_some();
                if ui.checkbox(&mut log, crate::widgets::tr(ui, "Logarithmic scale")).changed() {
                    run_axis(app, id, axis_name, json!({"logBase": if log { 10.0 } else { 0.0 }}));
                }
                if let Some(base) = o.log_base
                    && let Some(v) = number(ui, egui::Id::new(("chart_axis_log", id, value_axis)), base, 2.0..=1000.0, "base")
                {
                    run_axis(app, id, axis_name, json!({"logBase": v}));
                }
            });
        }
        if reason(AxisOption::Reversed).is_none() {
            let mut rev = o.reversed;
            if ui.checkbox(&mut rev, crate::widgets::tr(ui, "Values in reverse order")).changed() {
                run_axis(app, id, axis_name, json!({"reversed": rev}));
            }
        }
        if reason(AxisOption::Crossing).is_none() {
            ui.horizontal(|ui| {
                ui.label(tr(ui, "Crosses at"));
                let current = match o.crosses {
                    AxisCrossing::AutoZero => "Automatic",
                    AxisCrossing::Min => "Minimum",
                    AxisCrossing::Max => "Maximum",
                    AxisCrossing::At(_) => "Axis value",
                };
                egui::ComboBox::from_id_salt(("chart_axis_crosses", id, value_axis)).selected_text(crate::widgets::tr(ui, current)).show_ui(
                    ui,
                    |ui| {
                        for (label, params) in [
                            ("Automatic", json!({"crosses": "autoZero"})),
                            ("Minimum", json!({"crosses": "min"})),
                            ("Maximum", json!({"crosses": "max"})),
                        ] {
                            if ui.selectable_label(current == label, crate::widgets::tr(ui, label)).clicked() {
                                run_axis(app, id, axis_name, params);
                            }
                        }
                        let at = match o.crosses {
                            AxisCrossing::At(v) => v,
                            _ => 0.0,
                        };
                        if ui.selectable_label(current == crate::widgets::tr(ui, "Axis value"), crate::widgets::tr(ui, "Axis value")).clicked() {
                            run_axis(app, id, axis_name, json!({"crosses": at}));
                        }
                    },
                );
            });
            if let AxisCrossing::At(v) = o.crosses
                && let Some(v) = number(ui, egui::Id::new(("chart_axis_cross_value", id, value_axis)), v, -1e15..=1e15, "value")
            {
                run_axis(app, id, axis_name, json!({"crosses": v}));
            }
        } else if group.is_none()
            && let Some(r) = reason(AxisOption::Crossing)
        {
            ui.label(RichText::new(tr(ui, r)).small().color(t.text_dim));
        }
        if group.is_none() {
            ui.horizontal(|ui| {
                ui.label(tr(ui, "Display units"));
                let current = o.display_units.map_or("None", DisplayUnits::label);
                egui::ComboBox::from_id_salt(("chart_axis_units", id, value_axis)).selected_text(crate::widgets::tr(ui, current)).show_ui(ui, |ui| {
                    if ui.selectable_label(o.display_units.is_none(), crate::widgets::tr(ui, "None")).clicked() {
                        run_axis(app, id, axis_name, json!({"displayUnits": "none"}));
                    }
                    for (label, name) in [
                        ("Hundreds", "hundreds"),
                        ("Thousands", "thousands"),
                        ("Ten Thousands", "tenThousands"),
                        ("Hundred Thousands", "hundredThousands"),
                        ("Millions", "millions"),
                        ("Ten Millions", "tenMillions"),
                        ("Hundred Millions", "hundredMillions"),
                        ("Billions", "billions"),
                        ("Trillions", "trillions"),
                    ] {
                        if ui.selectable_label(current == label, crate::widgets::tr(ui, label)).clicked() {
                            run_axis(app, id, axis_name, json!({"displayUnits": name}));
                        }
                    }
                });
            });
        }
    });
    ui.collapsing(tr(ui, "Axis Number Format"), |ui| {
        if let Some(r) = reason(AxisOption::NumberFormat) {
            ui.label(RichText::new(tr(ui, r)).small().color(t.text_dim));
            return;
        }
        ui.horizontal(|ui| {
            ui.label(tr(ui, "Format code"));
            let field = egui::Id::new(("gridcraft.chart_axis_numfmt", id, value_axis));
            if let Some(code) = crate::widgets::committed_text(ui, field, o.number_format.as_deref().unwrap_or(""), None) {
                run_axis(app, id, axis_name, json!({"numberFormat": code}));
            }
        });
        ui.label(RichText::new(crate::widgets::tr(ui, "Empty follows the plotted values, as in Excel.")).small().color(t.text_dim));
    });
}

fn run_axis(app: &mut SheetApp, id: u32, axis: &str, params: Json) {
    let mut all = Map::new();
    all.insert("chart".into(), json!(id));
    all.insert("axis".into(), json!(axis));
    if let Json::Object(m) = params {
        all.extend(m);
    }
    app.run_or_alert("chart.setAxis", Json::Object(all));
}

fn crossing_json(crosses: AxisCrossing) -> Json {
    match crosses {
        AxisCrossing::AutoZero => json!("autoZero"),
        AxisCrossing::Min => json!("min"),
        AxisCrossing::Max => json!("max"),
        AxisCrossing::At(v) if v.is_finite() => json!(v),
        AxisCrossing::At(_) => json!("autoZero"),
    }
}

/// The plotted values' lowest and highest finite numbers, so turning an automatic bound off starts
/// somewhere sensible (the data's own edge) rather than at zero.
fn data_bounds(app: &SheetApp, chart: &Chart) -> (f64, f64) {
    let Some(d) = app.session.active() else { return (0.0, 1.0) };
    let data = gridcraft_chart::resolve(&d.wb, d.wb.active_sheet, chart);
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for v in data.series.iter().flat_map(|s| s.values.iter().flatten()) {
        if v.is_finite() {
            lo = lo.min(*v);
            hi = hi.max(*v);
        }
    }
    if lo.is_finite() && hi.is_finite() && lo < hi { (lo, hi) } else { (0.0, 1.0) }
}

/// An axis bound: a checkbox that makes it automatic, and a field that is there only when the
/// bound is fixed. The field keeps its draft until the pointer or focus lets go.
fn bound(ui: &mut Ui, id: egui::Id, value: Option<f64>, label: &str, start: f64, range: RangeInclusive<f64>) -> Option<Bound> {
    let mut action = None;
    let mut auto = value.is_none();
    ui.horizontal(|ui| {
        ui.label(tr(ui, label));
        let boxed = ui.checkbox(&mut auto, crate::widgets::tr(ui, "Auto"));
        if boxed.changed() {
            action = Some(if auto { Bound::Auto } else { Bound::Set(start) });
        }
        // Automatic: no field until the box is unticked, which fixes the bound at a starting value.
        if let Some(v) = value
            && let Some(v) = number(ui, id, v, range, "")
        {
            action = Some(Bound::Set(v));
        }
    });
    action
}

/// A number field that keeps its draft until the pointer or focus lets go, then returns the value
/// to commit (`None` while typing). Escape cancels.
fn number(ui: &mut Ui, id: egui::Id, value: f64, range: RangeInclusive<f64>, label: &str) -> Option<f64> {
    if !label.is_empty() {
        ui.label(tr(ui, label));
    }
    let mut commit = None;
    let mut draft = ui.data_mut(|d| d.get_temp::<f64>(id)).unwrap_or(value);
    let r = ui.add(egui::DragValue::new(&mut draft).range(range).speed(1.0).max_decimals(6));
    if r.drag_stopped() || r.lost_focus() {
        ui.data_mut(|d| d.remove::<f64>(id));
        if !ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            commit = Some(draft);
        }
    } else if r.dragged() || r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, draft));
    } else {
        ui.data_mut(|d| d.remove::<f64>(id));
    }
    commit
}

/// Format Data Labels: what a label shows, where, its separator and format. `series` scopes it
/// to one series (Excel's per-series labels); `None` sets the chart's labels for every series.
fn data_labels(app: &mut SheetApp, ui: &mut Ui, chart: &Chart, id: u32, series: Option<usize>) {
    let t = Tokens::get(ui.ctx());
    let o: LabelOptions = match series {
        Some(i) => chart.series.get(i).and_then(|s| s.labels.clone()).unwrap_or_else(|| chart.labels.clone()),
        None => chart.labels.clone(),
    };
    let shown = chart.data_labels && !o.hidden;
    section(ui, "Data Labels", labels_reason(chart), |ui| {
        let mut on = shown;
        if ui.checkbox(&mut on, tr(ui, if series.is_some() { "Show labels for this series" } else { "Show labels" })).changed() {
            run_labels(app, id, series, json!({"on": on}));
        }
        if !shown {
            ui.label(RichText::new(crate::widgets::tr(ui, "Turn the labels on to choose what they show.")).small().color(t.text_dim));
        }
        ui.add_enabled_ui(shown, |ui| {
            for (label, key, on) in [
                ("Value", "value", o.show_value),
                ("Percentage", "percentage", o.show_percentage),
                ("Series name", "seriesName", o.show_series),
                ("Category name", "categoryName", o.show_category),
                ("Legend key", "legendKey", o.show_legend_key),
            ] {
                let mut checked = on;
                if ui.checkbox(&mut checked, tr(ui, label)).changed() {
                    run_labels(app, id, series, json!({key: checked}));
                }
            }
            ui.horizontal(|ui| {
                ui.label(tr(ui, "Position"));
                let current = position_label(o.position);
                egui::ComboBox::from_id_salt(("chart_label_pos", id, series)).selected_text(position_text(ui, current)).show_ui(ui, |ui| {
                    for (label, params) in [
                        ("Automatic", "auto"),
                        ("Center", "center"),
                        ("Inside End", "insideEnd"),
                        ("Inside Base", "insideBase"),
                        ("Outside End", "outsideEnd"),
                        ("Best Fit", "bestFit"),
                        ("Left", "left"),
                        ("Right", "right"),
                        ("Above", "top"),
                        ("Below", "bottom"),
                    ] {
                        if ui.selectable_label(current == label, position_text(ui, label)).clicked() {
                            run_labels(app, id, series, json!({"position": params}));
                        }
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label(tr(ui, "Separator"));
                let field = egui::Id::new(("gridcraft.chart_label_sep", id, series));
                if let Some(text) = crate::widgets::committed_text(ui, field, o.separator.as_deref().unwrap_or(", "), Some(60.0)) {
                    run_labels(app, id, series, json!({"separator": text}));
                }
            });
            ui.horizontal(|ui| {
                ui.label(tr(ui, "Number format"));
                let field = egui::Id::new(("gridcraft.chart_label_numfmt", id, series));
                if let Some(text) = crate::widgets::committed_text(ui, field, o.number_format.as_deref().unwrap_or(""), Some(90.0)) {
                    run_labels(app, id, series, json!({"numberFormat": text}));
                }
            });
            label_font_controls(app, ui, id, series, None, o.font);
        });
    });
}

fn position_label(p: LabelPosition) -> &'static str {
    match p {
        LabelPosition::Auto => "Automatic",
        LabelPosition::Center => "Center",
        LabelPosition::InsideEnd => "Inside End",
        LabelPosition::InsideBase => "Inside Base",
        LabelPosition::OutsideEnd => "Outside End",
        LabelPosition::BestFit => "Best Fit",
        LabelPosition::Left => "Left",
        LabelPosition::Right => "Right",
        LabelPosition::Top => "Above",
        LabelPosition::Bottom => "Below",
    }
}

/// A legend or label position's name. "Left" and "Right" carry their own keys: the ribbon's
/// alignment "Left"/"Right" read 左对齐/右对齐 in Chinese, a position 靠左/靠右.
fn position_text<'a>(ui: &Ui, label: &'a str) -> &'a str {
    match label {
        "Left" => tr(ui, "chart|Left"),
        "Right" => tr(ui, "chart|Right"),
        other => tr(ui, other),
    }
}

fn run_labels(app: &mut SheetApp, id: u32, series: Option<usize>, params: Json) {
    let mut all = Map::new();
    all.insert("chart".into(), json!(id));
    if let Some(i) = series {
        all.insert("series".into(), json!(i));
    }
    if let Json::Object(m) = params {
        all.extend(m);
    }
    app.run_or_alert("chart.setDataLabels", Json::Object(all));
}

/// Format Error Bars: kind, amount, direction, caps. `series` scopes them to one series; `None`
/// applies them to every series, as `chart.setErrorBars` does.
fn error_bars(app: &mut SheetApp, ui: &mut Ui, chart: &Chart, id: u32, series: Option<usize>) {
    let t = Tokens::get(ui.ctx());
    let bars = match series {
        Some(i) => chart.series.get(i).and_then(|s| s.error_bars.clone()),
        None => chart.series.iter().find_map(|s| s.error_bars.clone()),
    };
    let (kind_name, amount) = match bars.as_ref().map(|b| &b.kind) {
        None => ("none", None),
        Some(ErrorBarKind::Fixed(v)) => ("fixed", Some(*v)),
        Some(ErrorBarKind::Percentage(v)) => ("percentage", Some(*v)),
        Some(ErrorBarKind::StdDev(v)) => ("stdDev", Some(*v)),
        Some(ErrorBarKind::StdErr) => ("stdErr", None),
        Some(ErrorBarKind::Custom { .. }) => ("custom", None),
    };
    let direction = bars.as_ref().map_or(ErrorBarDirection::Both, |b| b.direction);
    let caps = bars.as_ref().is_none_or(|b| !b.no_end_cap);
    let (mut plus_ref, mut minus_ref) = match bars.as_ref().map(|b| &b.kind) {
        Some(ErrorBarKind::Custom { plus, minus }) => (plus.clone().unwrap_or_default(), minus.clone().unwrap_or_default()),
        _ => (String::new(), String::new()),
    };
    section(ui, "Error Bars", error_bar_reason(chart), |ui| {
        ui.horizontal(|ui| {
            ui.label(crate::widgets::tr(ui, "Type"));
            let current = match kind_name {
                "fixed" => "Fixed amount",
                "percentage" => "Percentage",
                "stdDev" => "Standard deviation",
                "stdErr" => "Standard error",
                "custom" => "Custom",
                _ => "None",
            };
            egui::ComboBox::from_id_salt(("chart_errbars_kind", id, series)).selected_text(crate::widgets::tr(ui, current)).show_ui(ui, |ui| {
                for (label, params) in [
                    ("None", json!({"type": "none"})),
                    ("Fixed amount", json!({"type": "fixed", "value": amount.unwrap_or(1.0)})),
                    ("Percentage", json!({"type": "percentage", "value": amount.unwrap_or(10.0)})),
                    ("Standard deviation", json!({"type": "stdDev", "value": amount.unwrap_or(1.0)})),
                    ("Standard error", json!({"type": "stdErr"})),
                ] {
                    if ui.selectable_label(current == label, crate::widgets::tr(ui, label)).clicked() {
                        run_error_bars(app, id, series, params);
                    }
                }
                // Custom bars need a plus or minus reference; the fields below supply them.
                if plus_ref.trim().is_empty() && minus_ref.trim().is_empty() {
                    ui.add_enabled(false, egui::Button::new(crate::widgets::tr(ui, "Custom"))).on_disabled_hover_text(crate::widgets::tr(
                        ui,
                        "Custom error bars need a plus or minus reference; type one below (a range or an array constant).",
                    ));
                } else if ui.selectable_label(current == "Custom", crate::widgets::tr(ui, "Custom")).clicked() {
                    run_error_bars(app, id, series, json!({"type": "custom", "plus": plus_ref, "minus": minus_ref}));
                }
            });
        });
        if kind_name == "custom" {
            ui.label(RichText::new(crate::widgets::tr(ui, "Custom amounts come from these references.")).small().color(t.text_dim));
        }
        ui.add_enabled_ui(kind_name != "none" && kind_name != "custom", |ui| {
            if let Some(amount) = amount {
                let label = if kind_name == "percentage" { "Amount (%)" } else { "Amount" };
                if let Some(v) = number(ui, egui::Id::new(("chart_errbars_amount", id, series)), amount, 0.0..=1e12, label) {
                    run_error_bars(
                        app,
                        id,
                        series,
                        json!({"type": kind_name, "value": v, "direction": direction_name(direction), "noEndCap": !caps}),
                    );
                }
            }
            ui.horizontal(|ui| {
                ui.label(tr(ui, "Direction"));
                for (label, value) in [("Both", ErrorBarDirection::Both), ("Plus", ErrorBarDirection::Plus), ("Minus", ErrorBarDirection::Minus)] {
                    if ui.selectable_label(direction == value, crate::widgets::tr(ui, label)).clicked() {
                        run_error_bars(
                            app,
                            id,
                            series,
                            json!({"type": kind_name, "value": amount.unwrap_or(1.0), "direction": direction_name(value), "noEndCap": !caps}),
                        );
                    }
                }
            });
            let mut caps_on = caps;
            if ui.checkbox(&mut caps_on, crate::widgets::tr(ui, "End caps")).changed() {
                run_error_bars(
                    app,
                    id,
                    series,
                    json!({"type": kind_name, "value": amount.unwrap_or(1.0), "direction": direction_name(direction), "noEndCap": !caps_on}),
                );
            }
        });
        ui.separator();
        ui.label(RichText::new(crate::widgets::tr(ui, "Custom references (a range or an array constant)")).small().color(t.text_dim));
        ui.horizontal(|ui| {
            ui.label(tr(ui, "Plus reference"));
            if let Some(v) = crate::widgets::committed_text(ui, egui::Id::new(("chart_errbars_ref", id, series, "plus")), &plus_ref, None) {
                plus_ref = v;
                if !plus_ref.trim().is_empty() || !minus_ref.trim().is_empty() {
                    run_error_bars(app, id, series, json!({"type": "custom", "plus": plus_ref, "minus": minus_ref}));
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label(tr(ui, "Minus reference"));
            if let Some(v) = crate::widgets::committed_text(ui, egui::Id::new(("chart_errbars_ref", id, series, "minus")), &minus_ref, None) {
                minus_ref = v;
                if !plus_ref.trim().is_empty() || !minus_ref.trim().is_empty() {
                    run_error_bars(app, id, series, json!({"type": "custom", "plus": plus_ref, "minus": minus_ref}));
                }
            }
        });
    });
}

fn direction_name(direction: ErrorBarDirection) -> &'static str {
    match direction {
        ErrorBarDirection::Both => "both",
        ErrorBarDirection::Plus => "plus",
        ErrorBarDirection::Minus => "minus",
    }
}

fn run_error_bars(app: &mut SheetApp, id: u32, series: Option<usize>, params: Json) {
    let mut all = Map::new();
    all.insert("chart".into(), json!(id));
    if let Some(i) = series {
        all.insert("series".into(), json!(i));
    }
    if let Json::Object(m) = params {
        all.extend(m);
    }
    app.run_or_alert("chart.setErrorBars", Json::Object(all));
}

/// Format Data Point: the selected point's fill, line and marker, and its label. At chart level
/// the series is picked here; a `Selection::Series` fixes it.
fn data_point(app: &mut SheetApp, ui: &mut Ui, chart: &Chart, id: u32, series: Option<usize>) {
    let t = Tokens::get(ui.ctx());
    let reason = point_reason(chart, series.unwrap_or(0));
    section(ui, "Data Point", reason, |ui| {
        let scope = series.unwrap_or_else(|| chart_scope(ui, chart, id));
        let Some(s) = chart.series.get(scope) else { return };
        let count = point_count(app, chart, scope);
        let key = point_key(id, scope);
        let index = ui.data(|d| d.get_temp::<usize>(key)).unwrap_or(0).min(count.saturating_sub(1));
        ui.horizontal(|ui| {
            ui.label(tr(ui, "chart|Point"));
            let mut shown = index + 1;
            if ui.add(egui::DragValue::new(&mut shown).range(1..=count as i64)).changed() {
                ui.data_mut(|d| d.insert_temp(key, shown.saturating_sub(1).min(count - 1)));
            }
            ui.label(RichText::new(crate::widgets::tr(ui, "of {count}").replace("{count}", &count.to_string())).small().color(t.text_dim));
        });
        let pt = s.points.iter().find(|p| p.index as usize == index).copied().unwrap_or_default();
        let theme = app.session.active().map(|d| d.wb.theme.clone()).unwrap_or_default();
        for (label, key) in [("Fill", "fill"), ("point|Line", "line")] {
            point_colour(app, ui, &theme, id, scope, index, label, key, colour_of(&pt, key));
        }
        let marker = marker_reason(chart, scope);
        ui.add_enabled_ui(marker.is_none(), |ui| {
            point_colour(app, ui, &theme, id, scope, index, "Marker", "marker", pt.marker);
            ui.horizontal(|ui| {
                ui.label(tr(ui, "Marker size"));
                if let Some(v) = number(
                    ui,
                    egui::Id::new(("gridcraft.chart_point_marker_size", id, scope, index)),
                    f64::from(pt.marker_size.unwrap_or(5)),
                    2.0..=72.0,
                    "",
                ) {
                    let mut m = Map::new();
                    m.insert("chart".into(), json!(id));
                    m.insert("series".into(), json!(scope));
                    m.insert("point".into(), json!(index));
                    m.insert("markerSize".into(), json!(v.round() as u32));
                    app.run_or_alert("chart.formatPoint", Json::Object(m));
                }
                ui.label(RichText::new(tr(ui, "pt")).small().color(t.text_dim));
            });
        });
        if let Some(r) = marker {
            ui.label(RichText::new(tr(ui, r)).small().color(t.text_dim));
        }
        let mut no_label = pt.no_label;
        if ui.checkbox(&mut no_label, crate::widgets::tr(ui, "Hide this point's label")).changed() {
            let mut m = Map::new();
            m.insert("chart".into(), json!(id));
            m.insert("series".into(), json!(scope));
            m.insert("point".into(), json!(index));
            m.insert("deleteLabel".into(), json!(no_label));
            app.run_or_alert("chart.formatPoint", Json::Object(m));
        }
        label_font_controls(app, ui, id, Some(scope), Some(index as u32), s.point_label(index).and_then(|l| l.font));
        if ui.button(tr(ui, "Clear point formatting")).clicked() {
            let mut m = Map::new();
            m.insert("chart".into(), json!(id));
            m.insert("series".into(), json!(scope));
            m.insert("point".into(), json!(index));
            m.insert("clear".into(), json!(true));
            app.run_or_alert("chart.formatPoint", Json::Object(m));
        }
    });
}

fn colour_of(pt: &gridcraft_engine::model::PointFormat, key: &str) -> Option<Color> {
    match key {
        "fill" => pt.fill,
        "line" => pt.line,
        _ => pt.marker,
    }
}

/// A colour row: the current colour (or Automatic) and a palette menu that runs `cmd` with
/// `params` plus the chosen colour under `key`. `none` is the palette's no-colour entry, if it has
/// one; a point has none, since `chart.formatPoint` can clear the whole point, not one colour.
#[allow(clippy::too_many_arguments)]
fn colour_row(
    app: &mut SheetApp,
    ui: &mut Ui,
    wb_theme: &gridcraft_engine::model::Theme,
    label: &str,
    current: Option<Color>,
    cmd: &str,
    params: &Map<String, Json>,
    key: &str,
    none: &str,
) {
    ui.horizontal(|ui| {
        ui.label(tr(ui, label));
        if let Some(c) = colour_choice(ui, wb_theme, current, none) {
            let mut m = params.clone();
            m.insert(key.into(), json!(c));
            app.run_or_alert(cmd, Json::Object(m));
        }
    });
}

/// The swatch of the current colour (or a dim word when there is none) and the palette menu beside
/// it: returns the colour that was picked, `"none"` for the palette's no-colour entry.
fn colour_choice(ui: &mut Ui, wb_theme: &gridcraft_engine::model::Theme, current: Option<Color>, none: &str) -> Option<String> {
    let t = Tokens::get(ui.ctx());
    let mut picked = None;
    match current.and_then(|c| c.resolve(wb_theme)).map(theme::color32) {
        Some(c) => {
            let (r, _) = ui.allocate_exact_size(vec2(16.0, 14.0), egui::Sense::hover());
            ui.painter().rect_filled(r, 1.0, c);
            ui.painter().rect_stroke(r, 1.0, egui::Stroke::new(1.0, t.separator), egui::StrokeKind::Inside);
        }
        None => {
            // Not "Automatic": that word is the position combo's own value in this pane.
            ui.label(RichText::new(crate::widgets::tr(ui, "Automatic colour")).small().color(t.text_dim));
        }
    }
    ui.menu_button(tr(ui, "Colour…"), |ui| {
        if let Some(c) = crate::widgets::color_palette_fixed(ui, &wb_theme.colors, none) {
            ui.close();
            picked = Some(c);
        }
    });
    picked
}

/// A point's fill, line or marker colour through `chart.formatPoint`.
#[allow(clippy::too_many_arguments)]
fn point_colour(
    app: &mut SheetApp,
    ui: &mut Ui,
    wb_theme: &gridcraft_engine::model::Theme,
    id: u32,
    series: usize,
    point: usize,
    label: &str,
    key: &str,
    current: Option<Color>,
) {
    let mut m = Map::new();
    m.insert("chart".into(), json!(id));
    m.insert("series".into(), json!(series));
    m.insert("point".into(), json!(point));
    // `chart.formatPoint` keys its colour by the option's own name (fill, line, marker).
    colour_row(app, ui, wb_theme, label, current, "chart.formatPoint", &m, key, "");
}

/// A label's own font — size, weight and colour — for the chart's, a series' or one point's
/// labels, through `chart.setDataLabels`.
fn label_font_controls(app: &mut SheetApp, ui: &mut Ui, id: u32, series: Option<usize>, point: Option<u32>, font: Option<LabelFont>) {
    let f = font.unwrap_or_default();
    let mut params = Map::new();
    params.insert("chart".into(), json!(id));
    if let Some(s) = series {
        params.insert("series".into(), json!(s));
    }
    if let Some(p) = point {
        params.insert("point".into(), json!(p));
    }
    ui.horizontal(|ui| {
        ui.label(tr(ui, "Label size"));
        let field = egui::Id::new(("gridcraft.chart_label_font_size", id, series, point));
        if let Some(v) = number(ui, field, f64::from(f.size.unwrap_or(9.0)), 1.0..=400.0, "") {
            let mut m = params.clone();
            m.insert("fontSize".into(), json!(v));
            app.run_or_alert("chart.setDataLabels", Json::Object(m));
        }
        ui.label(RichText::new(tr(ui, "pt")).small().color(Tokens::get(ui.ctx()).text_dim));
    });
    let mut bold = f.bold.unwrap_or(false);
    if ui.checkbox(&mut bold, crate::widgets::tr(ui, "Bold label")).changed() {
        let mut m = params.clone();
        m.insert("bold".into(), json!(bold));
        app.run_or_alert("chart.setDataLabels", Json::Object(m));
    }
    let theme = app.session.active().map(|d| d.wb.theme.clone()).unwrap_or_default();
    colour_row(app, ui, &theme, "Label colour", f.color, "chart.setDataLabels", &params, "color", "");
}

/// The series a chart-level section works on, remembered per chart.
fn chart_scope(ui: &mut Ui, chart: &Chart, id: u32) -> usize {
    let key = egui::Id::new(("gridcraft.chart_scope", id));
    let mut scope: usize = ui.data(|d| d.get_temp(key)).unwrap_or(0);
    let n = chart.series.len();
    ui.horizontal(|ui| {
        ui.label(crate::widgets::tr(ui, "Series"));
        let mut shown = scope + 1;
        if ui.add(egui::DragValue::new(&mut shown).range(1..=n.max(1) as i64)).changed() {
            scope = shown.saturating_sub(1).min(n.saturating_sub(1));
            ui.data_mut(|d| d.insert_temp(key, scope));
        }
        if let Some(s) = chart.series.get(scope) {
            ui.label(RichText::new(&s.values).small().color(Tokens::get(ui.ctx()).text_dim));
        }
    });
    scope.min(n.saturating_sub(1))
}

// ---------------------------------------------------------------- shapes & pictures

/// What the pane formats when the selected object is not a chart: a shape or a picture, copied out
/// of the workbook so the pane never holds a borrow of it.
enum Picked {
    Shape(Shape),
    Image(Image),
}

/// The shape or picture the selected object id names on the active sheet, if it is one.
fn picked(app: &SheetApp, id: u32) -> Option<Picked> {
    let sh = app.session.active()?.wb.active()?;
    if let Some(shape) = sh.shapes.iter().find(|s| s.id == id) {
        return Some(Picked::Shape(shape.clone()));
    }
    sh.images.iter().find(|i| i.id == id).map(|i| Picked::Image(i.clone()))
}

/// The `object.format` params for one object; a section adds the field it changes.
fn object_params(kind: &str, id: u32) -> Map<String, Json> {
    let mut m = Map::new();
    m.insert("kind".into(), json!(kind));
    m.insert("id".into(), json!(id));
    m
}

/// Runs `object.format` with the object's params and the fields one control sets.
fn run_object(app: &mut SheetApp, base: &Map<String, Json>, fields: Json) {
    let mut m = base.clone();
    if let Json::Object(f) = fields {
        m.extend(f);
    }
    app.run_or_alert("object.format", Json::Object(m));
}

/// A shape: its fill, its outline, its text and its box — exactly what the model stores for one and
/// `object.format` writes, so no control here offers a value the command would refuse.
fn shape(app: &mut SheetApp, ui: &mut Ui, sh: &Shape) {
    let wb_theme = app.session.active().map(|d| d.wb.theme.clone()).unwrap_or_default();
    let t = Tokens::get(ui.ctx());
    let params = object_params("shape", sh.id);
    ui.collapsing(tr(ui, "Fill"), |ui| {
        colour_row(app, ui, &wb_theme, "Fill", Some(sh.fill), "object.format", &params, "fill", tr(ui, "No Fill"));
    });
    ui.collapsing(tr(ui, "Line"), |ui| {
        let mut drawn = !sh.format.no_line;
        if ui.checkbox(&mut drawn, crate::widgets::tr(ui, "No line")).changed() {
            run_object(app, &params, json!({"noLine": !drawn}));
        }
        ui.add_enabled_ui(drawn, |ui| {
            colour_row(app, ui, &wb_theme, "Line colour", Some(sh.line), "object.format", &params, "line", tr(ui, "Automatic"));
        });
    });
    // A shape's text formatting is stored with its text, so it is offered where there is text.
    if !sh.text.is_empty() {
        let f = &sh.format;
        ui.collapsing(tr(ui, "Text"), |ui| {
            ui.horizontal(|ui| {
                ui.label(tr(ui, "Text size"));
                let field = egui::Id::new(("gridcraft.object_font_size", sh.id));
                if let Some(v) = number(ui, field, f64::from(f.font_size.unwrap_or(SHAPE_TEXT_PT)), 1.0..=400.0, "") {
                    run_object(app, &params, json!({"fontSize": v}));
                }
                ui.label(RichText::new(tr(ui, "pt")).small().color(t.text_dim));
            });
            let mut bold = f.bold;
            if ui.checkbox(&mut bold, crate::widgets::tr(ui, "Bold")).changed() {
                run_object(app, &params, json!({"bold": bold}));
            }
            let mut italic = f.italic;
            if ui.checkbox(&mut italic, crate::widgets::tr(ui, "Italic")).changed() {
                run_object(app, &params, json!({"italic": italic}));
            }
            ui.horizontal(|ui| {
                ui.label(tr(ui, "Typeface"));
                let field = egui::Id::new(("gridcraft.object_font", sh.id));
                if let Some(name) = crate::widgets::committed_text(ui, field, f.font.as_deref().unwrap_or(""), Some(120.0)) {
                    run_object(app, &params, json!({"font": name}));
                }
            });
            ui.horizontal(|ui| {
                ui.label(tr(ui, "Text colour"));
                if let Some(c) = colour_choice(ui, &wb_theme, f.text_color, tr(ui, "Automatic")) {
                    run_object(app, &params, json!({"textColor": c}));
                }
            });
            ui.horizontal(|ui| {
                ui.label(tr(ui, "Text outline"));
                if let Some(c) = colour_choice(ui, &wb_theme, f.text_outline.map(|(c, _)| c), tr(ui, "No outline")) {
                    // A picked colour keeps the width it already had; 0.75 pt is Excel's hairline.
                    let width = f.text_outline.map_or(0.75, |(_, w)| w);
                    let fields = if c == "none" { json!({"textOutline": Json::Null}) } else { json!({"textOutline": {"color": c, "width": width}}) };
                    run_object(app, &params, fields);
                }
                if let Some((colour, width)) = f.text_outline
                    && let Some(hex) = colour.hex(&wb_theme)
                    && let Some(v) = number(ui, egui::Id::new(("gridcraft.object_text_outline", sh.id)), f64::from(width), 0.0..=100.0, "")
                {
                    run_object(app, &params, json!({"textOutline": {"color": hex, "width": v}}));
                }
            });
            ui.horizontal(|ui| {
                ui.label(tr(ui, "Alignment"));
                let current = match f.align {
                    Some(ShapeTextAlign::Left) => "Left",
                    Some(ShapeTextAlign::Center) => "Center",
                    Some(ShapeTextAlign::Right) => "Right",
                    None => "Default",
                };
                egui::ComboBox::from_id_salt(("gridcraft.object_align", sh.id)).selected_text(tr(ui, current)).show_ui(ui, |ui| {
                    for (label, value) in [("Default", Json::Null), ("Left", json!("left")), ("Center", json!("center")), ("Right", json!("right"))] {
                        if ui.selectable_label(current == label, tr(ui, label)).clicked() {
                            run_object(app, &params, json!({"align": value}));
                        }
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label(tr(ui, "Vertical anchor"));
                let current = match f.anchor {
                    Some(ShapeTextAnchor::Top) => "Top",
                    Some(ShapeTextAnchor::Middle) => "Middle",
                    Some(ShapeTextAnchor::Bottom) => "Bottom",
                    None => "Default",
                };
                egui::ComboBox::from_id_salt(("gridcraft.object_anchor", sh.id)).selected_text(tr(ui, current)).show_ui(ui, |ui| {
                    for (label, value) in [("Default", Json::Null), ("Top", json!("top")), ("Middle", json!("middle")), ("Bottom", json!("bottom"))] {
                        if ui.selectable_label(current == label, tr(ui, label)).clicked() {
                            run_object(app, &params, json!({"anchor": value}));
                        }
                    }
                });
            });
        });
    }
    object_box(app, ui, "shape", sh.id, &sh.anchor);
    object_alt(app, ui, "shape", sh.id, sh.alt.as_deref().unwrap_or(""));
}

/// A picture: the box it sits in and its alt text, the two things the model stores for one. Its
/// pixels are its own fill and outline, so there is nothing here to paint.
fn picture(app: &mut SheetApp, ui: &mut Ui, im: &Image) {
    object_box(app, ui, "image", im.id, &im.anchor);
    object_alt(app, ui, "image", im.id, &im.alt);
}

/// Excel's 18 pt default for a shape's text: what the size field shows before a size is stored.
const SHAPE_TEXT_PT: f32 = 18.0;

/// Where the object sits and how big it is (`object.format`), and how it follows the cells it covers
/// (`object.setAnchorMode`, the same choices a chart has).
fn object_box(app: &mut SheetApp, ui: &mut Ui, kind: &str, id: u32, anchor: &Anchor) {
    let params = object_params(kind, id);
    ui.collapsing(tr(ui, "Position & Size"), |ui| {
        ui.horizontal(|ui| {
            ui.label(tr(ui, "From"));
            if let Some(at) = crate::widgets::committed_text(ui, egui::Id::new(("gridcraft.object_at", id)), &anchor.cell.a1(), Some(70.0)) {
                run_object(app, &params, json!({"at": at}));
            }
        });
        // Keep the value being typed or dragged between frames; reloading it from the object every
        // frame threw the edit away.
        let draft = egui::Id::new(("gridcraft.object_size", id));
        let min = crate::chartview::OBJECT_MIN_SIZE;
        let (mut w, mut h) = ui.data_mut(|d| d.get_temp::<(f32, f32)>(draft)).unwrap_or((anchor.width, anchor.height));
        let a = ui.add(egui::DragValue::new(&mut w).prefix(tr(ui, "Width ")).range(min..=4000.0));
        let b = ui.add(egui::DragValue::new(&mut h).prefix(tr(ui, "Height ")).range(0.0..=4000.0));
        if a.drag_stopped() || a.lost_focus() || b.drag_stopped() || b.lost_focus() {
            ui.data_mut(|d| d.remove::<(f32, f32)>(draft));
            if !ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                run_object(app, &params, json!({"width": w, "height": h}));
            }
        } else if a.dragged() || a.has_focus() || b.dragged() || b.has_focus() {
            ui.data_mut(|d| d.insert_temp(draft, (w, h)));
        } else {
            ui.data_mut(|d| d.remove::<(f32, f32)>(draft));
        }
        ui.horizontal_wrapped(|ui| {
            for (label, mode) in [
                ("Move and size with cells", AnchorMode::MoveAndSize),
                ("Move but don't size with cells", AnchorMode::MoveOnly),
                ("Don't move or size with cells", AnchorMode::Absolute),
            ] {
                if ui.selectable_label(anchor.mode == mode, tr(ui, label)).clicked() {
                    app.run_or_alert("object.setAnchorMode", json!({"kind": kind, "id": id, "mode": anchor_mode_name(mode)}));
                }
            }
        });
    });
}

/// The wire name of an anchor mode, as `object.setAnchorMode` takes it.
fn anchor_mode_name(mode: AnchorMode) -> &'static str {
    match mode {
        AnchorMode::MoveAndSize => "moveAndSize",
        AnchorMode::MoveOnly => "moveOnly",
        AnchorMode::Absolute => "absolute",
    }
}

/// What a screen reader reads for the object, through the command that has always set it.
fn object_alt(app: &mut SheetApp, ui: &mut Ui, kind: &str, id: u32, alt: &str) {
    ui.collapsing(tr(ui, "Alt Text"), |ui| {
        ui.horizontal(|ui| {
            ui.label(tr(ui, "Description"));
            if let Some(text) = crate::widgets::committed_text(ui, egui::Id::new(("gridcraft.object_alt", id)), alt, None) {
                app.run_or_alert("object.setAltText", json!({"kind": kind, "id": id, "text": text}));
            }
        });
        ui.label(RichText::new(tr(ui, "Read out by a screen reader in place of the object.")).small().color(Tokens::get(ui.ctx()).text_dim));
    });
}

/// How many points a series plots (its value count, at least one).
pub fn point_count(app: &SheetApp, chart: &Chart, series: usize) -> usize {
    let Some(d) = app.session.active() else { return 1 };
    let data = gridcraft_chart::resolve(&d.wb, d.wb.active_sheet, chart);
    data.series.get(series).map_or(1, |s| s.values.len()).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gridcraft_engine::Session;

    fn chart(kind: &str) -> Session {
        let mut s = Session::new();
        s.new_workbook();
        for (c, v) in [("A1", "Category"), ("B1", "Value"), ("A2", "One"), ("B2", "3"), ("A3", "Two"), ("B3", "5")] {
            s.execute("cell.set", json!({"cell": c, "input": v})).unwrap();
        }
        s.execute("selection.set", json!({"range": "A1:B3"})).unwrap();
        s.execute("insert.chart", json!({"type": kind, "at": "D2", "width": 400, "height": 250})).unwrap();
        s
    }

    fn only_chart(s: &Session) -> Chart {
        s.doc().unwrap().wb.active().unwrap().charts.first().cloned().expect("a chart")
    }

    #[test]
    fn a_pie_offers_no_axis_options() {
        let s = chart("pie");
        let c = only_chart(&s);
        for option in [AxisOption::Bounds, AxisOption::LogBase, AxisOption::Crossing, AxisOption::DisplayUnits, AxisOption::NumberFormat] {
            assert_eq!(axis_reason(&c, true, option), Some(NO_AXES), "{option:?}");
        }
    }

    #[test]
    fn a_category_axis_disables_only_what_it_cannot_store() {
        let s = chart("column");
        let c = only_chart(&s);
        assert!(axis_reason(&c, false, AxisOption::Bounds).is_some());
        assert!(axis_reason(&c, false, AxisOption::LogBase).is_some());
        assert!(axis_reason(&c, false, AxisOption::DisplayUnits).is_some());
        assert!(axis_reason(&c, false, AxisOption::Crossing).is_some());
        assert_eq!(axis_reason(&c, false, AxisOption::MajorUnit), None, "the label interval is a category option");
        assert_eq!(axis_reason(&c, false, AxisOption::Reversed), None);
        assert_eq!(axis_reason(&c, false, AxisOption::NumberFormat), None);
        for option in [AxisOption::Bounds, AxisOption::LogBase, AxisOption::Crossing, AxisOption::DisplayUnits] {
            assert_eq!(axis_reason(&c, true, option), None, "{option:?}");
        }
        // One reason covers the scaling group on the label axis; the value axis has none.
        assert!(axis_group_reason(&c, false).is_some_and(|r| r.contains("category axis")));
        assert_eq!(axis_group_reason(&c, true), None);
    }

    #[test]
    fn a_scatter_x_axis_is_numeric_and_a_radar_has_no_crossing() {
        let s = chart("scatter");
        let c = only_chart(&s);
        assert_eq!(axis_group_reason(&c, false), None, "a numeric X axis takes the scaling options");
        assert_eq!(axis_reason(&c, false, AxisOption::Bounds), None);
        assert_eq!(axis_reason(&c, false, AxisOption::LogBase), None);
        assert_eq!(axis_reason(&c, false, AxisOption::Crossing), None);
        let s = chart("radar");
        let c = only_chart(&s);
        assert!(axis_reason(&c, true, AxisOption::Crossing).is_some());
        assert_eq!(axis_reason(&c, true, AxisOption::Bounds), None);
    }

    #[test]
    fn a_chartex_kind_refuses_every_chart_format_option() {
        for kind in ["treemap", "sunburst", "waterfall", "funnel", "histogram"] {
            let s = chart(kind);
            let c = only_chart(&s);
            assert_eq!(axis_reason(&c, true, AxisOption::Bounds), Some(CHARTEX), "{kind}");
            assert_eq!(labels_reason(&c), Some(CHARTEX), "{kind}");
            assert_eq!(error_bar_reason(&c), Some(CHARTEX), "{kind}");
            assert_eq!(point_reason(&c, 0), Some(CHARTEX), "{kind}");
        }
    }

    #[test]
    fn error_bars_are_offered_where_they_are_drawn() {
        for (kind, ok) in [("column", true), ("bar", true), ("line", true), ("area", true), ("scatter", true), ("pie", false), ("radar", false)] {
            let s = chart(kind);
            let c = only_chart(&s);
            assert_eq!(error_bar_reason(&c).is_none(), ok, "{kind}");
        }
    }

    #[test]
    fn a_marker_colour_needs_a_line_like_series() {
        for (kind, ok) in [("line", true), ("scatter", true), ("column", false), ("pie", false)] {
            let s = chart(kind);
            let c = only_chart(&s);
            assert_eq!(marker_reason(&c, 0).is_none(), ok, "{kind}");
        }
    }

    #[test]
    fn clearing_a_bound_keeps_the_rest_of_the_axis() {
        let mut s = chart("column");
        s.execute("chart.setAxis", json!({"axis": "y", "min": 1, "max": 9, "majorUnit": 2, "logBase": 10, "reversed": true})).unwrap();
        let c = only_chart(&s);
        assert!(c.y_axis.min == Some(1.0) && c.y_axis.log_base == Some(10.0));
        // The pane's Auto checkbox: reset the axis and put back everything but the minimum.
        let params = clear_axis_params(&c, true, "y", "min");
        s.execute("chart.setAxis", params).unwrap();
        let after = only_chart(&s).y_axis;
        assert_eq!(after.min, None, "the minimum is automatic again");
        assert_eq!(after.max, Some(9.0), "the maximum stays");
        assert_eq!(after.major_unit, Some(2.0));
        assert_eq!(after.log_base, Some(10.0));
        assert!(after.reversed);
    }

    #[test]
    fn clearing_an_option_on_a_category_axis_sends_only_its_options() {
        let mut s = chart("column");
        s.execute("chart.setAxis", json!({"axis": "x", "majorUnit": 3, "reversed": true})).unwrap();
        let c = only_chart(&s);
        let params = clear_axis_params(&c, false, "x", "majorUnit");
        let Json::Object(m) = &params else { panic!("params must be an object") };
        for key in ["min", "max", "logBase", "displayUnits", "crosses"] {
            assert!(!m.contains_key(key), "{key} must not be sent to a label category axis");
        }
        s.execute("chart.setAxis", params).unwrap();
        let after = only_chart(&s).x_axis;
        assert_eq!(after.major_unit, None);
        assert!(after.reversed, "the other options stay");
    }

    #[test]
    fn the_selection_parses_and_labels() {
        assert_eq!(Selection::parse("chart"), Selection::Chart);
        assert_eq!(Selection::parse("x"), Selection::AxisX);
        assert_eq!(Selection::parse("axisY"), Selection::AxisY);
        assert_eq!(Selection::parse("series2"), Selection::Series(1));
        assert_eq!(Selection::parse("nonsense"), Selection::Chart);
        assert_eq!(Selection::Series(0).text(), "Series 1");
    }
}
