use std::sync::Arc;

use gridcraft_core::{CellRef, Value};
use gridcraft_model::{Anchor, Chart, ChartKind, Color, LegendPos, Series, SparklineKind, Style, Workbook};

use crate::*;

const ALL_KINDS: [ChartKind; 25] = [
    ChartKind::ColumnClustered,
    ChartKind::ColumnStacked,
    ChartKind::ColumnStacked100,
    ChartKind::BarClustered,
    ChartKind::BarStacked,
    ChartKind::BarStacked100,
    ChartKind::Line,
    ChartKind::LineMarkers,
    ChartKind::LineStacked,
    ChartKind::Pie,
    ChartKind::Doughnut,
    ChartKind::Area,
    ChartKind::AreaStacked,
    ChartKind::Scatter,
    ChartKind::ScatterLines,
    ChartKind::Bubble,
    ChartKind::Radar,
    ChartKind::Histogram,
    ChartKind::Waterfall,
    ChartKind::Funnel,
    ChartKind::Treemap,
    ChartKind::Sunburst,
    ChartKind::BoxWhisker,
    ChartKind::Stock,
    ChartKind::Combo,
];

fn chart(kind: ChartKind) -> Chart {
    Chart {
        id: 1,
        kind,
        anchor: Anchor::default(),
        title: Some("Quarterly Sales".into()),
        series: vec![],
        legend: LegendPos::Bottom,
        data_labels: true,
        gridlines: true,
        style: 0,
        x_title: Some("Quarter".into()),
        y_title: Some("Revenue".into()),
        source: None,
        by_rows: false,
        pivot: None,
        ..Chart::default()
    }
}

fn series(name: &str, values: &[f64], color: Rgba, kind: ChartKind) -> SeriesData {
    SeriesData {
        label_font: None,
        name: name.into(),
        values: values.iter().map(|v| Some(*v)).collect(),
        x: Some(values.iter().enumerate().map(|(i, _)| Some(i as f64 * 1.5 + 2.0)).collect()),
        sizes: Some(values.iter().map(|v| Some(v.abs() + 1.0)).collect()),
        color,
        kind,
        secondary: false,
        smooth: false,
        trendlines: Vec::new(),
        number_format: "$#,##0".into(),
        ..SeriesData::default()
    }
}

fn sample(kind: ChartKind) -> ChartData {
    let cats = ["Q1", "Q2", "Q3", "Q4", "Q5", "Q6"];
    let k2 = if kind == ChartKind::Combo { ChartKind::Line } else { kind };
    let k1 = if kind == ChartKind::Combo { ChartKind::ColumnClustered } else { kind };
    let mut b = series("East", &[38000.0, 30000.0, 45000.0, 61870.0, 52000.0, 41000.0], [200, 80, 40, 255], k2);
    b.secondary = kind == ChartKind::Combo;
    ChartData {
        label_font: None,
        category_levels: vec![],
        palette: vec![],
        categories: cats.iter().map(|s| s.to_string()).collect(),
        series: vec![
            series("North", &[12000.0, -4000.0, 23000.0, 31000.0, 18000.0, 27000.0], [20, 96, 130, 255], k1),
            b,
            series("South", &[9000.0, 11000.0, 8000.0, 15000.0, 21000.0, 17000.0], [25, 107, 36, 255], k2),
        ],
    }
}

fn coords(p: &Prim) -> Vec<f32> {
    match p {
        Prim::Rect { x, y, w, h, .. } => vec![*x, *y, x + w, y + h],
        Prim::Line { pts, width, .. } => pts.iter().flat_map(|p| [p[0], p[1]]).chain([*width]).collect(),
        Prim::Polygon { pts, .. } => pts.iter().flat_map(|p| [p[0], p[1]]).collect(),
        Prim::Wedge { cx, cy, r_outer, r_inner, a0, a1, .. } => vec![*cx, *cy, *r_outer, *r_inner, *a0, *a1],
        Prim::Circle { cx, cy, r, .. } => vec![*cx, *cy, *r],
        Prim::Text { x, y, size, rotation, .. } => vec![*x, *y, *size, *rotation],
    }
}

fn assert_sane(prims: &[Prim], w: f32, h: f32) {
    let eps = 0.01;
    for p in prims {
        assert!(coords(p).iter().all(|v| v.is_finite()), "non-finite: {p:?}");
        let inside = |x: f32, y: f32| x >= -eps && x <= w + eps && y >= -eps && y <= h + eps;
        match p {
            Prim::Rect { x, y, w: rw, h: rh, .. } => assert!(inside(*x, *y) && inside(x + rw, y + rh), "{p:?}"),
            Prim::Line { pts, .. } | Prim::Polygon { pts, .. } => assert!(pts.iter().all(|q| inside(q[0], q[1])), "{p:?}"),
            Prim::Wedge { cx, cy, r_outer, .. } => assert!(inside(cx - r_outer, cy - r_outer) && inside(cx + r_outer, cy + r_outer), "{p:?}"),
            Prim::Circle { cx, cy, r, .. } => assert!(inside(cx - r, cy - r) && inside(cx + r, cy + r), "{p:?}"),
            Prim::Text { x, y, .. } => assert!(inside(*x, *y), "{p:?}"),
        }
    }
}

fn texts(prims: &[Prim]) -> Vec<&str> {
    prims
        .iter()
        .filter_map(|p| match p {
            Prim::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn every_kind_renders_sanely() {
    let m = ApproxMeasure;
    for kind in ALL_KINDS {
        for legend in [LegendPos::Bottom, LegendPos::Top, LegendPos::Left, LegendPos::Right] {
            let mut c = chart(kind);
            c.legend = legend;
            let d = sample(kind);
            let (w, h) = (640.0, 400.0);
            let prims = render(&c, &d, w, h, &m);
            assert!(prims.len() > 3, "{kind:?}: too few prims");
            assert_sane(&prims, w, h);
            let t = texts(&prims);
            assert!(t.contains(&"Quarterly Sales"), "{kind:?}: no title");
            let entries = legend_entries(&c, &d);
            for (name, _, _) in &entries {
                assert!(t.contains(&name.as_str()), "{kind:?} {legend:?}: legend entry {name} missing");
            }
            let n_shapes = prims.iter().filter(|p| !matches!(p, Prim::Text { .. })).count();
            assert!(n_shapes > 2, "{kind:?}: nothing drawn");
        }
    }
}

#[test]
fn legend_counts() {
    let d = sample(ChartKind::ColumnClustered);
    assert_eq!(legend_entries(&chart(ChartKind::ColumnClustered), &d).len(), 3);
    assert_eq!(legend_entries(&chart(ChartKind::Pie), &d).len(), 6);
    assert_eq!(legend_entries(&chart(ChartKind::Waterfall), &d).len(), 3);
    assert_eq!(legend_entries(&chart(ChartKind::Histogram), &d).len(), 0);
    let mut c = chart(ChartKind::Line);
    c.legend = LegendPos::None;
    assert!(legend_entries(&c, &d).is_empty());
}

#[test]
fn nice_axis() {
    let s = nice_scale(0.0, 61870.0, true);
    assert_eq!((s.min, s.max, s.step), (0.0, 70000.0, 10000.0));
    let s = nice_scale(0.0, 100.0, true);
    assert_eq!((s.min, s.max, s.step), (0.0, 120.0, 20.0));
    let s = nice_scale(-4000.0, 31000.0, true);
    assert!(s.min < -4000.0 && s.min >= -10000.0 && s.max >= 31000.0);
    // All zero.
    let s = nice_scale(0.0, 0.0, true);
    assert_eq!((s.min, s.max), (0.0, 1.0));
    // Far from zero: no forced zero.
    let s = nice_scale(50.0, 55.0, false);
    assert!(s.min >= 45.0 && s.max <= 60.0, "{s:?}");
    // Close to zero: zero included.
    assert_eq!(nice_scale(10.0, 90.0, false).min, 0.0);
    // Negative only.
    let s = nice_scale(-80.0, -10.0, false);
    assert_eq!(s.max, 0.0);
    assert!(s.min <= -80.0);
    for (lo, hi) in [(0.0, 1e-300), (-1e308, 1e308), (f64::NAN, 5.0), (0.001, 0.0013), (3.0, 3.0), (-7.0, -7.0), (1e15, 1e15 + 1.0)] {
        let s = nice_scale(lo, hi, false);
        assert!(s.min.is_finite() && s.max.is_finite() && s.step.is_finite() && s.max > s.min && s.step > 0.0, "{lo} {hi} {s:?}");
        let n = s.ticks().len();
        assert!((2..=51).contains(&n));
    }
    // Ticks count in 4..=10 for typical data.
    for hi in [1.0, 7.0, 13.0, 99.0, 250.0, 1234.0, 61870.0, 999999.0] {
        let n = nice_scale(0.0, hi, true).ticks().len();
        assert!((4..=10).contains(&n), "{hi}: {n}");
    }
}

#[test]
fn axis_labels_use_number_format() {
    assert_eq!(format_number(40000.0, "$#,##0", Some(10000.0)), "$40,000");
    assert_eq!(format_number(0.30000000000000004, "General", Some(0.1)), "0.3");
    assert_eq!(format_number(1e-17, "General", Some(0.2)), "0");
    let c = chart(ChartKind::ColumnClustered);
    let prims = render(&c, &sample(ChartKind::ColumnClustered), 640.0, 400.0, &ApproxMeasure);
    let t = texts(&prims);
    assert!(t.contains(&"$70,000"), "{t:?}");
    assert!(t.contains(&"$40,000"));
}

#[test]
fn resolve_from_workbook() {
    let mut wb = Workbook::new();
    let cur = wb.styles.intern(Style { num_fmt: gridcraft_model::NumFmt(Arc::from("$#,##0")), ..Style::default() });
    {
        let s = wb.sheet_mut(0).unwrap();
        s.name = "Sales".into();
        s.set_value(CellRef::new(0, 1), Value::from("Revenue"));
        for (i, (q, v)) in [("Q1", 100.0), ("Q2", 250.0), ("Q3", 175.0)].iter().enumerate() {
            let r = i as u32 + 1;
            s.set_value(CellRef::new(r, 0), Value::from(*q));
            s.set_value(CellRef::new(r, 1), Value::Number(*v));
            s.set_style(CellRef::new(r, 1), cur);
        }
        s.set_value(CellRef::new(2, 2), Value::Number(5.0));
    }
    let mut c = chart(ChartKind::ColumnClustered);
    c.series = vec![
        Series {
            name: Some("Sales!$B$1".into()),
            categories: Some("Sales!$A$2:$A$4".into()),
            values: "Sales!$B$2:$B$4".into(),
            bubble_sizes: None,
            color: None,
            secondary: false,
            kind: None,
            quartiles: Default::default(),
            smooth: false,
            trendlines: Vec::new(),
            ..Series::default()
        },
        Series {
            name: None,
            categories: None,
            values: "=Sales!$C$2:$C$4".into(),
            bubble_sizes: None,
            color: Some(Color::rgb(1, 2, 3)),
            secondary: true,
            kind: None,
            quartiles: Default::default(),
            smooth: false,
            trendlines: Vec::new(),
            ..Series::default()
        },
        Series {
            name: Some("Literal".into()),
            categories: None,
            values: "{1,2,3,4}".into(),
            bubble_sizes: None,
            color: None,
            secondary: false,
            kind: None,
            quartiles: Default::default(),
            smooth: false,
            trendlines: Vec::new(),
            ..Series::default()
        },
    ];
    let d = resolve(&wb, 0, &c);
    assert_eq!(d.categories, vec!["Q1", "Q2", "Q3"]);
    assert_eq!(d.series.len(), 3);
    let s0 = &d.series[0];
    assert_eq!(s0.name, "Revenue");
    assert_eq!(s0.values, vec![Some(100.0), Some(250.0), Some(175.0)]);
    assert_eq!(s0.number_format, "$#,##0");
    assert_eq!(s0.color, series_color(&wb.theme, 0));
    let t = wb.theme.colors[4];
    assert_eq!(s0.color, [(t >> 16) as u8, (t >> 8) as u8, t as u8, 255]);
    let s1 = &d.series[1];
    assert_eq!(s1.name, "Series2");
    assert_eq!(s1.values, vec![None, Some(5.0), None]);
    assert_eq!(s1.color, [1, 2, 3, 255]);
    assert!(s1.secondary);
    assert_eq!(d.series[2].name, "Literal");
    assert_eq!(d.series[2].values.len(), 4);

    // Default categories.
    c.series.remove(0);
    let d = resolve(&wb, 0, &c);
    assert_eq!(d.categories, vec!["1", "2", "3", "4"]);

    // Combo kinds and scatter X values.
    let mut c = chart(ChartKind::Combo);
    c.series = vec![
        Series {
            name: None,
            categories: None,
            values: "Sales!B2:B4".into(),
            bubble_sizes: None,
            color: None,
            secondary: false,
            kind: None,
            quartiles: Default::default(),
            smooth: false,
            trendlines: Vec::new(),
            ..Series::default()
        },
        Series {
            name: None,
            categories: None,
            values: "Sales!B2:B4".into(),
            bubble_sizes: None,
            color: None,
            secondary: true,
            kind: None,
            quartiles: Default::default(),
            smooth: false,
            trendlines: Vec::new(),
            ..Series::default()
        },
    ];
    let d = resolve(&wb, 0, &c);
    assert_eq!(d.series[0].kind, ChartKind::ColumnClustered);
    assert_eq!(d.series[1].kind, ChartKind::Line);
    let mut c = chart(ChartKind::Bubble);
    c.series = vec![Series {
        name: None,
        categories: Some("Sales!B2:B4".into()),
        values: "Sales!B2:B4".into(),
        bubble_sizes: Some("{1,2,3}".into()),
        color: None,
        secondary: false,
        kind: None,
        quartiles: Default::default(),
        smooth: false,
        trendlines: Vec::new(),
        ..Series::default()
    }];
    let d = resolve(&wb, 0, &c);
    assert_eq!(d.series[0].x.as_ref().map(|x| x.len()), Some(3));
    assert_eq!(d.series[0].sizes, Some(vec![Some(1.0), Some(2.0), Some(3.0)]));

    // Bad formulas never panic.
    let mut c = chart(ChartKind::Line);
    c.series = vec![Series {
        name: Some("=Nope!A1".into()),
        categories: Some("((".into()),
        values: "Missing!A1:A3".into(),
        bubble_sizes: Some("".into()),
        color: None,
        secondary: false,
        kind: None,
        quartiles: Default::default(),
        smooth: false,
        trendlines: Vec::new(),
        ..Series::default()
    }];
    let d = resolve(&wb, 7, &c);
    assert_eq!(d.series.len(), 1);
    let _ = render(&c, &d, 300.0, 200.0, &ApproxMeasure);
}

#[test]
fn palette_beyond_six_series() {
    let theme = gridcraft_model::Theme::default();
    let p = default_palette(&theme, 14);
    assert_eq!(p.len(), 14);
    assert_ne!(p[0], p[6]);
    assert_ne!(p[6], p[12]);
}

#[test]
fn scatter_with_text_x_values_numbers_the_points() {
    let mut wb = Workbook::new();
    let s = wb.sheet_mut(0).unwrap();
    for (i, (label, v)) in [("North", 3.0), ("South", 5.0), ("East", 4.0)].iter().enumerate() {
        s.set_value(CellRef::new(i as u32, 0), Value::from(*label));
        s.set_value(CellRef::new(i as u32, 1), Value::Number(*v));
        s.set_value(CellRef::new(i as u32, 2), Value::Number(i as f64 * 10.0));
    }
    for kind in [ChartKind::Scatter, ChartKind::ScatterLines, ChartKind::Bubble] {
        let mut c = chart(kind);
        c.series = vec![Series { categories: Some("Sheet1!$A$1:$A$3".into()), values: "Sheet1!$B$1:$B$3".into(), ..Default::default() }];
        let d = resolve(&wb, 0, &c);
        assert_eq!(d.series[0].x, None, "{kind:?}");
        let prims = render(&c, &d, 400.0, 300.0, &ApproxMeasure);
        let empty = render(
            &c,
            &ChartData { label_font: None, category_levels: vec![], palette: vec![], categories: vec![], series: vec![] },
            400.0,
            300.0,
            &ApproxMeasure,
        );
        assert!(prims.len() > empty.len() + 2, "{kind:?} draws its points");
        // Numeric X values are kept.
        c.series[0].categories = Some("Sheet1!$C$1:$C$3".into());
        assert_eq!(resolve(&wb, 0, &c).series[0].x, Some(vec![Some(0.0), Some(10.0), Some(20.0)]));
    }
}

fn pixel(img: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2], img[i + 3]]
}

#[test]
fn rasterize_bar_center_has_bar_color() {
    let mut c = chart(ChartKind::ColumnClustered);
    c.data_labels = false;
    let d = sample(ChartKind::ColumnClustered);
    let (w, h) = (640u32, 400u32);
    let prims = render(&c, &d, w as f32, h as f32, &ApproxMeasure);
    let img = rasterize(&prims, w, h, [255, 255, 255, 255]);
    assert_eq!(img.len(), (w * h * 4) as usize);
    // Find the bars of the East series (tallest) and check their centres.
    let east = d.series[1].color;
    let bars: Vec<_> = prims
        .iter()
        .filter_map(|p| match p {
            Prim::Rect { x, y, w, h, fill: Some(f), .. } if *f == east && *w > 10.0 => Some((*x, *y, *w, *h)),
            _ => None,
        })
        .collect();
    assert_eq!(bars.len(), 6);
    for (x, y, bw, bh) in bars {
        let px = pixel(&img, w, (x + bw / 2.0) as u32, (y + bh / 2.0) as u32);
        assert_eq!(px, east);
    }
    // Primitive-level checks.
    let img = rasterize(&[Prim::Circle { cx: 10.0, cy: 10.0, r: 5.0, fill: Some([255, 0, 0, 255]), stroke: None }], 20, 20, [0, 0, 0, 0]);
    assert_eq!(pixel(&img, 20, 10, 10), [255, 0, 0, 255]);
    assert_eq!(pixel(&img, 20, 0, 0)[3], 0);
    let wedge = Prim::Wedge {
        cx: 50.0,
        cy: 50.0,
        r_outer: 40.0,
        r_inner: 20.0,
        a0: 0.0,
        a1: std::f32::consts::FRAC_PI_2,
        fill: [0, 0, 255, 255],
        stroke: None,
    };
    let img = rasterize(&[wedge], 100, 100, [255, 255, 255, 255]);
    assert_eq!(pixel(&img, 100, 79, 30), [0, 0, 255, 255]); // upper-right quadrant ring
    assert_eq!(pixel(&img, 100, 50, 50), [255, 255, 255, 255]); // hole
    assert_eq!(pixel(&img, 100, 25, 70), [255, 255, 255, 255]); // other quadrant
    assert!(rasterize(&[], 0, 10, [0; 4]).is_empty());
    assert!(rasterize(&[], 100_000, 100_000, [0; 4]).is_empty());
}

#[test]
fn rasterize_does_not_panic_on_negative_rect_extents() {
    let red = [255, 0, 0, 255];
    for (w, h) in [(-10.0, 10.0), (10.0, -10.0), (-10.0, -10.0), (f32::NAN, f32::NAN)] {
        let rect = Prim::Rect { x: 5.0, y: 5.0, w, h, fill: Some(red), stroke: Some((red, 2.0)), radius: 0.0 };
        let img = rasterize(&[rect], 20, 20, [0, 0, 0, 0]);
        assert_eq!(img.len(), 20 * 20 * 4);
    }
}

#[test]
fn pie_starts_at_twelve_clockwise() {
    let c = chart(ChartKind::Pie);
    let d = sample(ChartKind::Pie);
    let prims = render(&c, &d, 400.0, 400.0, &ApproxMeasure);
    let wedges: Vec<_> = prims
        .iter()
        .filter_map(|p| match p {
            Prim::Wedge { a0, a1, r_inner, stroke, .. } => Some((*a0, *a1, *r_inner, *stroke)),
            _ => None,
        })
        .collect();
    assert_eq!(wedges.len(), 6);
    assert_eq!(wedges[0].0, 0.0);
    assert!(wedges.windows(2).all(|w| w[1].0 >= w[0].1 - 1e-4));
    assert!((wedges[5].1 - std::f32::consts::TAU).abs() < 1e-3);
    assert!(wedges.iter().all(|w| w.2 == 0.0 && w.3.map(|s| s.0) == Some([255; 4])));
    let prims = render(&chart(ChartKind::Doughnut), &d, 400.0, 400.0, &ApproxMeasure);
    let ok = prims.iter().any(|p| matches!(p, Prim::Wedge { r_outer, r_inner, .. } if (r_inner / r_outer - 0.5).abs() < 1e-3));
    assert!(ok);
}

#[test]
fn negative_values_put_axis_below_zero() {
    let c = chart(ChartKind::ColumnClustered);
    let d = ChartData {
        label_font: None,
        category_levels: vec![],
        palette: vec![],
        categories: vec![],
        series: vec![series("N", &[-5.0, 3.0, -2.0], [1, 2, 3, 255], ChartKind::ColumnClustered)],
    };
    let prims = render(&c, &d, 400.0, 300.0, &ApproxMeasure);
    let t = texts(&prims);
    assert!(t.iter().any(|s| s.starts_with('-') || s.starts_with("($") || s.starts_with("-$")), "{t:?}");
    assert!(t.contains(&"1") && t.contains(&"3"), "default categories: {t:?}");
}

#[test]
fn edge_cases_never_panic() {
    let m = ApproxMeasure;
    let empty = ChartData::default();
    let zeros = ChartData {
        label_font: None,
        category_levels: vec![],
        palette: vec![],
        categories: vec!["a".into(), "b".into()],
        series: vec![series("z", &[0.0, 0.0], [0, 0, 0, 255], ChartKind::Line)],
    };
    let weird = ChartData {
        label_font: None,
        category_levels: vec![],
        palette: vec![],
        categories: vec![String::new(); 3],
        series: vec![SeriesData {
            label_font: None,
            name: "w".repeat(500),
            values: vec![Some(f64::NAN), Some(f64::INFINITY), None, Some(-1e308), Some(1e308)],
            x: Some(vec![Some(f64::NAN)]),
            sizes: Some(vec![Some(-1.0), Some(0.0)]),
            ..SeriesData::default()
        }],
    };
    for kind in ALL_KINDS {
        let c = chart(kind);
        for d in [&empty, &zeros, &weird] {
            for (w, h) in [(0.0, 0.0), (-5.0, 100.0), (f32::NAN, 10.0), (1.0, 1.0), (20.0, 15.0), (640.0, 400.0), (5000.0, 30.0), (1e6, 1e6)] {
                let p = render(&c, d, w, h, &m);
                if !(w > 0.0 && h > 0.0) || w.is_nan() {
                    assert!(p.is_empty());
                } else {
                    assert_sane(&p, w, h);
                }
            }
        }
    }
}

#[test]
fn thousands_of_points_are_decimated() {
    let n = 50_000;
    let vals: Vec<f64> = (0..n).map(|i| ((i as f64) * 0.01).sin() * 100.0 + i as f64 * 0.001).collect();
    let mut c = chart(ChartKind::Line);
    c.data_labels = true;
    let d = ChartData {
        label_font: None,
        category_levels: vec![],
        palette: vec![],
        categories: vec![],
        series: vec![series("big", &vals, [9, 9, 9, 255], ChartKind::Line)],
    };
    let prims = render(&c, &d, 800.0, 400.0, &ApproxMeasure);
    let max_line = prims
        .iter()
        .filter_map(|p| match p {
            Prim::Line { pts, .. } => Some(pts.len()),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    assert!(max_line > 10 && max_line <= 4 * 2000, "{max_line}");
    assert!(prims.len() < 10_000);
    for kind in [ChartKind::Scatter, ChartKind::Histogram, ChartKind::AreaStacked, ChartKind::BoxWhisker] {
        let p = render(&chart(kind), &d, 800.0, 400.0, &ApproxMeasure);
        assert_sane(&p, 800.0, 400.0);
    }
}

#[test]
fn smooth_line_adds_interpolated_points_and_keeps_endpoints() {
    let m = ApproxMeasure;
    let cats = ["a", "b", "c", "d"].map(String::from).to_vec();
    let line_pts = |smooth: bool, m: &ApproxMeasure| -> usize {
        let mut s = series("L", &[1.0, 5.0, 2.0, 6.0], [10, 20, 30, 255], ChartKind::Line);
        s.smooth = smooth;
        let d = ChartData { label_font: None, category_levels: vec![], palette: vec![], categories: cats.clone(), series: vec![s] };
        render(&chart(ChartKind::Line), &d, 600.0, 400.0, m)
            .iter()
            .filter_map(|p| match p {
                Prim::Line { pts, .. } => Some(pts.len()),
                _ => None,
            })
            .max()
            .unwrap_or(0)
    };
    let straight = line_pts(false, &m);
    let curved = line_pts(true, &m);
    assert_eq!(straight, 4, "straight polyline keeps one point per datum");
    assert!(curved > straight, "smooth line interpolates extra points: {curved} vs {straight}");
    // Rendering either way must stay finite and inside the canvas.
    let mut s = series("L", &[1.0, 5.0, 2.0, 6.0], [10, 20, 30, 255], ChartKind::Line);
    s.smooth = true;
    let d = ChartData { label_font: None, category_levels: vec![], palette: vec![], categories: cats, series: vec![s] };
    assert_sane(&render(&chart(ChartKind::Line), &d, 600.0, 400.0, &m), 600.0, 400.0);
}

#[test]
fn smooth_line_splits_at_gaps() {
    let m = ApproxMeasure;
    let mut s = series("L", &[1.0, 5.0, 2.0, 6.0], [10, 20, 30, 255], ChartKind::Line);
    s.smooth = true;
    s.values[1] = None; // gap between two data points
    let d = ChartData {
        label_font: None,
        category_levels: vec![],
        palette: vec![],
        categories: vec!["a".into(), "b".into(), "c".into(), "d".into()],
        series: vec![s],
    };
    let prims = render(&chart(ChartKind::Line), &d, 600.0, 400.0, &m);
    // Two separate runs (before/after the gap) → at least two line primitives.
    let lines = prims.iter().filter(|p| matches!(p, Prim::Line { .. })).count();
    assert!(lines >= 2, "a gap must split the smoothed line into runs, got {lines}");
    assert_sane(&prims, 600.0, 400.0);
}

/// Small deterministic PRNG (xorshift) for the fuzz test.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn f(&mut self) -> f64 {
        (self.next() % 1_000_000) as f64 / 1_000_000.0
    }
}

#[test]
fn fuzz_random_data_and_sizes() {
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    let m = ApproxMeasure;
    for iter in 0..400 {
        let kind = ALL_KINDS[(r.next() % ALL_KINDS.len() as u64) as usize];
        let nser = (r.next() % 6) as usize;
        let npts = (r.next() % 40) as usize;
        let mag = 10f64.powi((r.next() % 30) as i32 - 10);
        let val = |r: &mut Rng| -> Option<f64> {
            match r.next() % 12 {
                0 => None,
                1 => Some(f64::NAN),
                2 => Some(0.0),
                3 => Some(-r.f() * mag),
                _ => Some(r.f() * mag),
            }
        };
        let series: Vec<SeriesData> = (0..nser)
            .map(|i| SeriesData {
                label_font: None,
                name: format!("S{i}"),
                values: (0..npts).map(|_| val(&mut r)).collect(),
                x: if r.next().is_multiple_of(2) { Some((0..npts).map(|_| val(&mut r)).collect()) } else { None },
                sizes: if r.next().is_multiple_of(2) { Some((0..npts).map(|_| val(&mut r)).collect()) } else { None },
                color: [(r.next() % 256) as u8, 100, 50, 255],
                kind: ALL_KINDS[(r.next() % ALL_KINDS.len() as u64) as usize],
                secondary: r.next().is_multiple_of(3),
                smooth: r.next().is_multiple_of(4),
                trendlines: (0..r.next() % 3)
                    .map(|_| TrendlineData {
                        spec: Trendline {
                            kind: [
                                TrendlineKind::Linear,
                                TrendlineKind::Exponential,
                                TrendlineKind::Logarithmic,
                                TrendlineKind::Polynomial((r.next() % 8) as u8),
                                TrendlineKind::Power,
                                TrendlineKind::MovingAverage((r.next() % 6) as u32),
                            ][(r.next() % 6) as usize],
                            forward: [0.0, 1.5, -2.0, f64::NAN, 1e300][(r.next() % 5) as usize],
                            backward: [0.0, 0.5, f64::INFINITY][(r.next() % 3) as usize],
                            intercept: [None, Some(0.0), Some(-5.0), Some(f64::NAN)][(r.next() % 4) as usize],
                            display_equation: r.next().is_multiple_of(2),
                            display_r_squared: r.next().is_multiple_of(2),
                            width: [None, Some(0.0), Some(f32::NAN), Some(3.0)][(r.next() % 4) as usize],
                            ..Trendline::default()
                        },
                        color: [0, 0, 0, 255],
                    })
                    .collect(),
                number_format: ["General", "$#,##0", "0.00%", "yyyy-mm-dd", "#,##0.0;[Red]-#,##0.0", "@"][(r.next() % 6) as usize].into(),
                ..SeriesData::default()
            })
            .collect();
        let ncat = (r.next() % 50) as usize;
        let d = ChartData {
            label_font: None,
            category_levels: vec![],
            palette: vec![],
            categories: (0..ncat).map(|i| "c".repeat(i % 7) + &i.to_string()).collect(),
            series,
        };
        let mut c = chart(kind);
        c.legend = [LegendPos::None, LegendPos::Bottom, LegendPos::Top, LegendPos::Left, LegendPos::Right][(r.next() % 5) as usize];
        c.data_labels = r.next().is_multiple_of(2);
        c.gridlines = r.next().is_multiple_of(2);
        if r.next().is_multiple_of(3) {
            c.title = None;
        }
        let w = [0.0, -3.0, 1.0, 12.0, 80.0, 320.0, 1200.0][(r.next() % 7) as usize] as f32 + r.f() as f32 * 10.0;
        let h = [0.0, 2.0, 30.0, 200.0, 900.0][(r.next() % 5) as usize] as f32 + r.f() as f32 * 10.0;
        let p = render(&c, &d, w, h, &m);
        if w >= 1.0 && h >= 1.0 {
            assert_sane(&p, w, h);
        }
        if iter % 20 == 0 && w >= 1.0 && h >= 1.0 && w < 2000.0 && h < 2000.0 {
            let img = rasterize(&p, w as u32, h as u32, [255; 4]);
            assert_eq!(img.len(), (w as u32 * h as u32 * 4) as usize);
        }
        let vals: Vec<Option<f64>> = (0..npts).map(|_| val(&mut r)).collect();
        for k in [SparklineKind::Line, SparklineKind::Column, SparklineKind::WinLoss] {
            let sp = render_sparkline(k, &vals, [0, 0, 0, 255], r.next().is_multiple_of(2), w, h);
            if w >= 1.0 && h >= 1.0 {
                assert_sane(&sp, w.max(1.0), h.max(1.0));
            }
        }
    }
}

#[test]
fn sparklines() {
    let v = [Some(1.0), Some(-2.0), None, Some(4.0), Some(0.0), Some(3.0)];
    let line = render_sparkline(SparklineKind::Line, &v, [1, 2, 3, 255], true, 60.0, 16.0);
    assert_eq!(line.iter().filter(|p| matches!(p, Prim::Line { .. })).count(), 2);
    assert_eq!(line.iter().filter(|p| matches!(p, Prim::Circle { .. })).count(), 5);
    assert_sane(&line, 60.0, 16.0);
    let col = render_sparkline(SparklineKind::Column, &v, [1, 2, 3, 255], false, 60.0, 16.0);
    assert_eq!(col.len(), 5);
    assert_sane(&col, 60.0, 16.0);
    let wl = render_sparkline(SparklineKind::WinLoss, &v, [1, 2, 3, 255], false, 60.0, 16.0);
    assert_eq!(wl.len(), 4);
    assert!(render_sparkline(SparklineKind::Line, &v, [0; 4], false, 0.0, 10.0).is_empty());
    assert!(render_sparkline(SparklineKind::Line, &[], [0; 4], false, 10.0, 10.0).is_empty());
}

// ---------------------------------------------------------------- trendlines

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * (1.0 + b.abs())
}

fn fit(kind: TrendlineKind, pts: &[(f64, f64)]) -> crate::trend::TrendFit {
    crate::trend::fit_trendline(&Trendline { kind, ..Trendline::default() }, pts).expect("fits")
}

#[test]
fn linear_trendline_matches_least_squares() {
    // Slope 0.6, intercept 2.2, R² = 1 - 2.4 / 6.
    let f = fit(TrendlineKind::Linear, &[(1.0, 2.0), (2.0, 4.0), (3.0, 5.0), (4.0, 4.0), (5.0, 5.0)]);
    assert!(close(f.coef[0], 2.2) && close(f.coef[1], 0.6), "{:?}", f.coef);
    assert!(close(f.r2.unwrap(), 0.6));
    assert_eq!(f.equation(), "y = 0.6x + 2.2");
    assert_eq!(f.r_squared(), "R² = 0.6");
    assert!(close(f.eval(10.0).unwrap(), 8.2));
    let down = fit(TrendlineKind::Linear, &[(0.0, 1.0), (1.0, -1.0), (2.0, -3.0)]);
    assert_eq!(down.equation(), "y = -2x + 1");
}

#[test]
fn set_intercept_fits_through_it() {
    // Through the origin: slope Σxy / Σx² = 30 / 14; R² against the uncentred total.
    let pts = [(1.0, 2.0), (2.0, 5.0), (3.0, 6.0)];
    let f = crate::trend::fit_trendline(&Trendline { intercept: Some(0.0), ..Trendline::default() }, &pts).unwrap();
    assert!(close(f.coef[0], 0.0) && close(f.coef[1], 15.0 / 7.0), "{:?}", f.coef);
    let ss_res: f64 = pts.iter().map(|(x, y)| (y - 15.0 / 7.0 * x).powi(2)).sum();
    assert!(close(f.r2.unwrap(), 1.0 - ss_res / 65.0));
    let q = crate::trend::fit_trendline(
        &Trendline { kind: TrendlineKind::Polynomial(2), intercept: Some(2.0), ..Trendline::default() },
        &[(1.0, 0.0), (2.0, 0.0), (3.0, 2.0), (4.0, 6.0)],
    )
    .unwrap();
    assert!(close(q.coef[0], 2.0) && close(q.coef[1], -3.0) && close(q.coef[2], 1.0), "{:?}", q.coef);
}

#[test]
fn transformed_trendlines_recover_their_curves() {
    let xs: [f64; 6] = [0.5, 1.0, 2.0, 3.0, 4.5, 6.0];
    let e = fit(TrendlineKind::Exponential, &xs.map(|x| (x, 3.0 * (0.5 * x).exp())));
    assert!(close(e.coef[0].exp(), 3.0) && close(e.coef[1], 0.5));
    assert_eq!(e.equation(), "y = 3e^(0.5x)");
    let l = fit(TrendlineKind::Logarithmic, &xs.map(|x| (x, 2.0 * x.ln() + 1.0)));
    assert!(close(l.coef[1], 2.0) && close(l.coef[0], 1.0));
    assert_eq!(l.equation(), "y = 2ln(x) + 1");
    let p = fit(TrendlineKind::Power, &xs.map(|x| (x, 4.0 * x.powf(1.5))));
    assert!(close(p.coef[0].exp(), 4.0) && close(p.coef[1], 1.5));
    assert_eq!(p.equation(), "y = 4x^1.5");
    assert!(close(p.r2.unwrap(), 1.0));
    assert_eq!(p.eval(-1.0), None, "power is undefined left of zero");
    // Points the transform can't take are skipped, as Excel documents.
    let skip = fit(TrendlineKind::Exponential, &[(1.0, 2.0f64.exp()), (2.0, 0.0), (3.0, -4.0), (4.0, 8.0f64.exp())]);
    assert!(close(skip.coef[1], 2.0) && close(skip.coef[0], 0.0), "{:?}", skip.coef);
    assert!(
        crate::trend::fit_trendline(&Trendline { kind: TrendlineKind::Power, ..Trendline::default() }, &[(-1.0, 1.0), (0.0, 2.0), (3.0, 3.0)])
            .is_none()
    );
}

#[test]
fn polynomial_trendlines_are_stable() {
    let q = fit(TrendlineKind::Polynomial(2), &[0.0, 1.0, 2.0, 3.0, 4.0].map(|x| (x, x * x - 3.0 * x + 2.0)));
    assert!(close(q.coef[0], 2.0) && close(q.coef[1], -3.0) && close(q.coef[2], 1.0), "{:?}", q.coef);
    assert_eq!(q.equation(), "y = x² - 3x + 2");
    // Year-valued x would make the raw normal equations hopelessly ill-conditioned.
    let curve = |x: f64| {
        let t = x - 2000.0;
        0.001 * t.powi(6) - 0.02 * t.powi(5) + 0.1 * t.powi(3) - t + 7.0
    };
    let years: Vec<(f64, f64)> = (2001..=2012).map(|x| (f64::from(x), curve(f64::from(x)))).collect();
    let six = fit(TrendlineKind::Polynomial(6), &years);
    for x in [2001.0, 2006.5, 2012.0, 2014.0] {
        let (got, want) = (six.eval(x).unwrap(), curve(x));
        assert!((got - want).abs() <= 1e-6 * (1.0 + want.abs()), "{x}: {got} vs {want}");
    }
    assert!(six.r2.unwrap() > 0.999_999);
}

#[test]
fn degenerate_trendlines_fit_nothing() {
    let lin = |pts: &[(f64, f64)]| crate::trend::fit_trendline(&Trendline::default(), pts);
    assert!(lin(&[]).is_none());
    assert!(lin(&[(1.0, 1.0)]).is_none());
    assert!(lin(&[(2.0, 1.0), (2.0, 5.0), (2.0, 3.0)]).is_none(), "all x equal is singular");
    assert!(lin(&[(f64::NAN, 1.0), (1.0, f64::INFINITY), (2.0, 3.0)]).is_none());
    assert!(
        crate::trend::fit_trendline(&Trendline { kind: TrendlineKind::Polynomial(3), ..Trendline::default() }, &[(1.0, 1.0), (2.0, 4.0), (3.0, 9.0)])
            .is_none()
    );
    assert!(
        crate::trend::fit_trendline(&Trendline { kind: TrendlineKind::MovingAverage(2), ..Trendline::default() }, &[(1.0, 1.0), (2.0, 2.0)])
            .is_none()
    );
    let flat = lin(&[(1.0, 4.0), (2.0, 4.0), (3.0, 4.0)]).unwrap();
    assert_eq!((flat.equation(), flat.r2), ("y = 4".to_string(), None));
    assert!(lin(&[(0.0, 1e300), (1.0, -1e300), (2.0, 1e300)]).is_none_or(|f| f.coef.iter().all(|c| c.is_finite())));
}

#[test]
fn moving_average_and_labels() {
    let pts = [(1.0, 1.0), (2.0, 2.0), (3.0, 6.0), (4.0, f64::NAN), (5.0, 4.0)];
    assert_eq!(crate::trend::moving_average(&pts, 2), vec![(2.0, 1.5), (3.0, 4.0), (5.0, 5.0)]);
    assert_eq!(crate::trend::moving_average(&pts, 3), vec![(3.0, 3.0), (5.0, 4.0)]);
    assert!(crate::trend::moving_average(&pts, 9).is_empty());
    assert_eq!(crate::trend::num_text(0.000012345), "1.2345E-05");
    assert_eq!(crate::trend::num_text(-0.00001), "-1E-05");
    assert_eq!(crate::trend::num_text(1234.56789), "1234.5679");
    assert_eq!(crate::trend::auto_name(TrendlineKind::MovingAverage(3), "Sales"), "3 per. Mov. Avg. (Sales)");
    assert_eq!(crate::trend::auto_name(TrendlineKind::Exponential, "Sales"), "Expon. (Sales)");
}

fn lines_of(prims: &[Prim], color: Rgba) -> Vec<&Vec<[f32; 2]>> {
    prims
        .iter()
        .filter_map(|p| match p {
            Prim::Line { pts, color: c, .. } if *c == color => Some(pts),
            _ => None,
        })
        .collect()
}

#[test]
fn line_chart_draws_trendline_forecast_and_labels() {
    let m = ApproxMeasure;
    let tc: Rgba = [1, 2, 3, 255];
    let mut s = series("Sales", &[9.0, 7.0, 5.0, 3.0], [10, 20, 30, 255], ChartKind::Line);
    s.trendlines =
        vec![TrendlineData { spec: Trendline { forward: 1.0, display_equation: true, display_r_squared: true, ..Trendline::default() }, color: tc }];
    let d = ChartData {
        label_font: None,
        category_levels: vec![],
        palette: vec![],
        categories: ["a", "b", "c", "d"].map(String::from).to_vec(),
        series: vec![s.clone()],
    };
    let mut c = chart(ChartKind::Line);
    c.legend = LegendPos::None; // keeps legend swatches out of the line counts
    let prims = render(&c, &d, 600.0, 400.0, &m);
    assert_sane(&prims, 600.0, 400.0);
    let data = lines_of(&prims, [10, 20, 30, 255]);
    let trend = lines_of(&prims, tc);
    assert_eq!((data.len(), trend.len()), (1, 1));
    let (d0, dn) = (data[0][0], data[0][3]);
    let (t0, tn) = (trend[0][0], *trend[0].last().unwrap());
    // Exact data: the trendline starts on the first point and runs through the last,
    // then on one more category (the forecast slot).
    assert!((t0[0] - d0[0]).abs() < 0.01 && (t0[1] - d0[1]).abs() < 0.01, "{t0:?} vs {d0:?}");
    let step = (dn[0] - d0[0]) / 3.0;
    assert!((tn[0] - (dn[0] + step)).abs() < 0.01, "forecast end {tn:?}, last point {dn:?}");
    assert!(tn[1] > dn[1], "keeps falling past the data");
    let t = texts(&prims);
    assert!(t.contains(&"y = -2x + 11") && t.contains(&"R² = 1"), "{t:?}");
    assert!(legend_entries(&chart(ChartKind::Line), &d).iter().any(|(n, col, sw)| n == "Linear (Sales)" && *col == tc && *sw == Swatch::Line));
    // Not on chart kinds Excel doesn't offer trendlines on.
    let pie = render(&chart(ChartKind::Pie), &d, 600.0, 400.0, &m);
    assert!(lines_of(&pie, tc).is_empty());
    // One point: nothing to fit, nothing drawn.
    s.values.truncate(1);
    let one = ChartData { label_font: None, category_levels: vec![], palette: vec![], categories: vec!["a".into()], series: vec![s] };
    assert!(lines_of(&render(&c, &one, 600.0, 400.0, &m), tc).is_empty());
}

#[test]
fn scatter_trendlines_use_x_values_and_stay_in_the_plot() {
    let m = ApproxMeasure;
    let xs = [1.0, 2.0, 4.0, 8.0];
    let mut s = series("P", &xs.map(|x| 4.0 * x * x), [10, 20, 30, 255], ChartKind::Scatter);
    s.x = Some(xs.iter().map(|x| Some(*x)).collect());
    let colors: [Rgba; 3] = [[1, 0, 0, 255], [2, 0, 0, 255], [3, 0, 0, 255]];
    s.trendlines = vec![
        TrendlineData {
            spec: Trendline { kind: TrendlineKind::Power, backward: 0.5, display_equation: true, ..Trendline::default() },
            color: colors[0],
        },
        // A steep exponential leaves the plot box and is clipped to it.
        TrendlineData { spec: Trendline { kind: TrendlineKind::Exponential, forward: 30.0, ..Trendline::default() }, color: colors[1] },
        TrendlineData { spec: Trendline { kind: TrendlineKind::MovingAverage(2), ..Trendline::default() }, color: colors[2] },
    ];
    let d = ChartData { label_font: None, category_levels: vec![], palette: vec![], categories: vec![], series: vec![s] };
    let mut c = chart(ChartKind::Scatter);
    c.legend = LegendPos::None;
    let prims = render(&c, &d, 600.0, 400.0, &m);
    assert_sane(&prims, 600.0, 400.0);
    for c in colors {
        assert!(!lines_of(&prims, c).is_empty(), "{c:?} not drawn");
    }
    assert_eq!(lines_of(&prims, colors[2])[0].len(), 3, "moving average: one point per full window");
    assert!(texts(&prims).contains(&"y = 4x^2"));
}

#[test]
fn treemap_and_histogram_shapes() {
    let d = ChartData {
        label_font: None,
        category_levels: vec![],
        palette: vec![],
        categories: vec!["a".into(), "b".into(), "c".into(), "d".into()],
        series: vec![series("t", &[6.0, 6.0, 4.0, 3.0], [10, 20, 30, 255], ChartKind::Treemap)],
    };
    let mut c = chart(ChartKind::Treemap);
    c.title = None;
    c.legend = LegendPos::None;
    let prims = render(&c, &d, 600.0, 400.0, &ApproxMeasure);
    let rects: Vec<f32> = prims
        .iter()
        .skip(1)
        .filter_map(|p| match p {
            Prim::Rect { w, h, .. } => Some(w * h),
            _ => None,
        })
        .collect();
    assert_eq!(rects.len(), 4);
    // Areas proportional to values.
    assert!((rects[0] / rects[3] - 2.0).abs() < 0.05, "{rects:?}");

    let vals: Vec<f64> = (0..100).map(|i| i as f64).collect();
    let d = ChartData {
        label_font: None,
        category_levels: vec![],
        palette: vec![],
        categories: vec![],
        series: vec![series("h", &vals, [10, 20, 30, 255], ChartKind::Histogram)],
    };
    let prims = render(&chart(ChartKind::Histogram), &d, 600.0, 400.0, &ApproxMeasure);
    let bins = prims.iter().filter(|p| matches!(p, Prim::Rect { fill: Some([10, 20, 30, 255]), .. })).count();
    assert_eq!(bins, 10);
}

#[test]
fn histogram_boundary_values_match_interval_labels() {
    let heights = |vals: &[f64]| -> Vec<f32> {
        let d = ChartData {
            label_font: None,
            category_levels: vec![],
            palette: vec![],
            categories: vec![],
            series: vec![series("h", vals, [10, 20, 30, 255], ChartKind::Histogram)],
        };
        let prims = render(&chart(ChartKind::Histogram), &d, 600.0, 400.0, &ApproxMeasure);
        prims.iter().filter_map(|p| if let Prim::Rect { h, fill: Some([10, 20, 30, 255]), .. } = p { Some(*h) } else { None }).collect()
    };
    // Labels are "[0, 7.5]" and "(7.5, 15.0]": 7.5 belongs to the first bar, so the counts are 2 and 1.
    let h = heights(&[0.0, 7.5, 15.0]);
    assert_eq!(h.len(), 2);
    assert!((h[0] / h[1] - 2.0).abs() < 0.01, "{h:?}");
    let h = heights(&[-15.0, -7.5, 0.0]);
    assert!((h[0] / h[1] - 2.0).abs() < 0.01, "{h:?}");
    // A value off the boundary still goes right: [0, 7.5] holds 1, (7.5, 15.0] holds 2.
    let h = heights(&[0.0, 7.75, 15.0]);
    assert!((h[1] / h[0] - 2.0).abs() < 0.01, "{h:?}");
    // Five values, three bins [0,5] (5,10] (10,15]: counts 3, 1, 1.
    let h = heights(&[0.0, 5.0, 5.0, 10.0, 15.0]);
    assert_eq!(h.len(), 3);
    assert!((h[0] / h[1] - 3.0).abs() < 0.01 && (h[1] - h[2]).abs() < 0.01, "{h:?}");
}

#[test]
fn union_series_references() {
    let mut wb = Workbook::new();
    {
        let s = wb.sheet_mut(0).unwrap();
        for (r, (l, v)) in [("a", 1.0), ("b", 2.0), ("Total", 3.0), ("d", 4.0)].iter().enumerate() {
            s.set_value(CellRef::new(r as u32, 0), Value::from(*l));
            s.set_value(CellRef::new(r as u32, 1), Value::Number(*v));
        }
    }
    let mut c = chart(ChartKind::ColumnClustered);
    c.series = vec![Series {
        name: Some("\"Sum of X\"".into()),
        categories: Some("(Sheet1!$A$1:$A$2,Sheet1!$A$4)".into()),
        values: "(Sheet1!$B$1:$B$2,Sheet1!$B$4)".into(),
        ..Series::default()
    }];
    let d = resolve(&wb, 0, &c);
    assert_eq!(d.series[0].values, vec![Some(1.0), Some(2.0), Some(4.0)]);
    assert_eq!(d.series[0].name, "Sum of X");
    assert_eq!(d.categories, vec!["a", "b", "d"]);
    // Deeply nested parentheses from a file are bounded, not a crash.
    c.series[0].values = format!("{}Sheet1!$B$1,Sheet1!$B$2{}", "(".repeat(5000), ")".repeat(5000));
    let _ = resolve(&wb, 0, &c);
}

#[test]
fn box_and_stock_charts_honour_the_gridlines_flag() {
    // Horizontal full-width lines in the gridline colour are the value-axis gridlines.
    let grid: Rgba = [217, 217, 217, 255];
    let is_grid = |p: &Prim| match p {
        Prim::Line { color, pts, .. } => *color == grid && pts.len() == 2 && pts[0][1] == pts[1][1] && pts[1][0] - pts[0][0] > 200.0,
        _ => false,
    };
    let grids = |prims: &[Prim]| prims.iter().filter(|p| is_grid(p)).count();
    for kind in [ChartKind::BoxWhisker, ChartKind::Stock, ChartKind::ColumnClustered] {
        let mut d = sample(kind);
        d.series.push(series("Close", &[10000.0, 12000.0, 9000.0, 14000.0, 20000.0, 16000.0], [90, 90, 90, 255], kind));
        let mut c = chart(kind);
        let on = render(&c, &d, 500.0, 350.0, &ApproxMeasure);
        c.gridlines = false;
        let off = render(&c, &d, 500.0, 350.0, &ApproxMeasure);
        assert!(grids(&on) > 0, "{kind:?}: gridlines on must draw lines");
        assert_eq!(grids(&off), 0, "{kind:?}: gridlines off must hide them");
    }
}

#[test]
fn minor_gridlines_are_dashed_and_lighter_than_the_major_ones() {
    let d = sample(ChartKind::ColumnClustered);
    let m = ApproxMeasure;
    let mut c = chart(ChartKind::ColumnClustered);
    let major_only = render(&c, &d, 640.0, 400.0, &m);
    assert!(major_only.iter().any(|p| matches!(p, Prim::Line { dash: false, color, .. } if *color == [0xD9, 0xD9, 0xD9, 0xFF])));
    assert!(!major_only.iter().any(|p| matches!(p, Prim::Line { dash: true, .. })), "no minor gridlines with them off");
    // The value axis's minor set: dashes halfway between the labelled ticks.
    c.gridlines_minor = true;
    let with_minor = render(&c, &d, 640.0, 400.0, &m);
    let dashed = with_minor.iter().filter(|p| matches!(p, Prim::Line { dash: true, color, .. } if *color == [0xE9, 0xE9, 0xE9, 0xFF])).count();
    let majors = with_minor.iter().filter(|p| matches!(p, Prim::Line { dash: false, color, .. } if *color == [0xD9, 0xD9, 0xD9, 0xFF])).count();
    assert!(dashed > 0, "minor gridlines on must draw dashed lines");
    assert_eq!(dashed, majors.saturating_sub(1).max(1), "one minor line per gap between two major ones: {dashed} minors, {majors} majors");
    // Halfway between the major lines, and never where a major one is.
    let ys = |prims: &[Prim], dash: bool| -> Vec<f32> {
        prims
            .iter()
            .filter_map(|p| match p {
                Prim::Line { pts, dash: d, color, .. }
                    if *d == dash && (*color == [0xD9, 0xD9, 0xD9, 0xFF] || *color == [0xE9, 0xE9, 0xE9, 0xFF]) && pts[0][1] == pts[1][1] =>
                {
                    Some(pts[0][1])
                }
                _ => None,
            })
            .collect()
    };
    let (major, minor) = (ys(&with_minor, false), ys(&with_minor, true));
    for m in &minor {
        assert!(major.iter().all(|y| (y - m).abs() > 0.5), "a minor line sits on a major one");
    }
    for w in major.windows(2) {
        let (a, b) = (w[0], w[1]);
        assert!(minor.iter().any(|m| (m - (a + b) / 2.0).abs() < 0.5), "no minor line between {a} and {b}");
    }
    // Off again: back to the major set alone.
    c.gridlines_minor = false;
    assert!(!render(&c, &d, 640.0, 400.0, &m).iter().any(|p| matches!(p, Prim::Line { dash: true, .. })));
    // A radar chart's value-axis gridlines are its rings, so its minor set is the rings between them.
    let mut r = chart(ChartKind::Radar);
    let rings = |prims: &[Prim], dash: bool| prims.iter().filter(|p| matches!(p, Prim::Line { dash: d, .. } if *d == dash)).count();
    let major_only = render(&r, &sample(ChartKind::Radar), 640.0, 400.0, &m);
    assert!(rings(&major_only, false) > 0 && rings(&major_only, true) == 0, "radar rings are solid");
    r.gridlines_minor = true;
    assert!(rings(&render(&r, &sample(ChartKind::Radar), 640.0, 400.0, &m), true) > 0, "radar minor rings");
}

#[test]
fn category_gridlines_are_vertical_lines_at_the_boundaries() {
    let d = sample(ChartKind::ColumnClustered);
    let m = ApproxMeasure;
    let mut c = chart(ChartKind::ColumnClustered);
    let verticals = |prims: &[Prim], color: Rgba| -> Vec<(f32, f32, f32)> {
        prims
            .iter()
            .filter_map(|p| match p {
                Prim::Line { pts, color: col, .. } if *col == color && pts.len() == 2 && pts[0][0] == pts[1][0] && pts[1][1] - pts[0][1] > 50.0 => {
                    Some((pts[0][0], pts[0][1], pts[1][1]))
                }
                _ => None,
            })
            .collect()
    };
    assert!(verticals(&render(&c, &d, 640.0, 400.0, &m), [0xD9, 0xD9, 0xD9, 0xFF]).is_empty(), "off by default");
    c.x_gridlines = Gridlines::MAJOR;
    let prims = render(&c, &d, 640.0, 400.0, &m);
    let lines = verticals(&prims, [0xD9, 0xD9, 0xD9, 0xFF]);
    // One line per category boundary: six categories, seven boundaries.
    assert_eq!(lines.len(), 7, "one major line per boundary");
    // They run down the whole plot and are evenly spaced.
    let mut xs: Vec<f32> = lines.iter().map(|l| l.0).collect();
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let step = xs[1] - xs[0];
    assert!(step > 10.0, "categories are {step} apart");
    for w in xs.windows(2) {
        assert!((w[1] - w[0] - step).abs() < 0.5, "uneven category lines");
    }
    // A bar chart turns the axes: its category gridlines are horizontal.
    let mut h = chart(ChartKind::BarClustered);
    h.x_gridlines = Gridlines::MAJOR;
    let prims = render(&h, &sample(ChartKind::BarClustered), 640.0, 400.0, &m);
    let horizontals = prims
        .iter()
        .filter(|p| matches!(p, Prim::Line { color, pts, dash: false, .. } if *color == [0xD9, 0xD9, 0xD9, 0xFF] && pts[0][1] == pts[1][1] && pts[1][0] - pts[0][0] > 100.0))
        .count();
    assert!(horizontals > 0, "a bar chart draws its category gridlines horizontally");
}

#[test]
fn an_overlaid_legend_sits_inside_the_plot_area() {
    let d = sample(ChartKind::ColumnClustered);
    let m = ApproxMeasure;
    let mut c = chart(ChartKind::ColumnClustered);
    c.legend = LegendPos::Bottom;
    // The "East" entry's text anchor: the legend is laid out before the plot is drawn.
    let entry_at = |prims: &[Prim]| -> Option<[f32; 2]> {
        prims.iter().find_map(|p| match p {
            Prim::Text { text, x, y, .. } if text == "East" => Some([*x, *y]),
            _ => None,
        })
    };
    // "Q1" is the first category label, drawn just under the plot area: its y is the plot's
    // bottom edge plus the label gap, so it says where the plot ends and the labels begin.
    let cat_label_y = |prims: &[Prim]| -> Option<f32> {
        prims.iter().find_map(|p| match p {
            Prim::Text { text, y, .. } if text == "Q1" => Some(*y),
            _ => None,
        })
    };
    let beside = render(&c, &d, 640.0, 400.0, &m);
    let apart = cat_label_y(&beside).expect("the category labels are drawn");
    let ly = entry_at(&beside).expect("the legend entry is drawn")[1];
    assert!(ly > apart, "a legend in its own band sits below the plot area ({ly} vs {apart})");
    c.legend_overlay = true;
    let over = render(&c, &d, 640.0, 400.0, &m);
    let inside = cat_label_y(&over).expect("the category labels are still drawn");
    let [ex, ey] = entry_at(&over).expect("the overlaid legend entry is drawn");
    assert!(ey < inside, "an overlaid legend sits inside the plot area ({ey} vs {inside})");
    // The plot keeps the room the legend would have taken, so its bottom edge is lower than before.
    assert!(inside > apart, "the overlaid legend frees the plot area ({inside} vs {apart})");
    // The entries are still there, and a click on one still selects its series.
    let hits = render_hits(&c, &d, 640.0, 400.0, &m);
    assert!(
        hits.iter().any(|r| matches!(r.hit, Hit::Series { index: 1, point: None }) && r.shape.contains(ex, ey)),
        "the overlaid entry keeps its click target"
    );
}

#[test]
fn funnel_value_labels_follow_the_data_labels_flag() {
    let data = ChartData {
        label_font: None,
        category_levels: vec![],
        palette: vec![],
        categories: vec!["a".into(), "b".into(), "c".into()],
        series: vec![series("S", &[11111.0, 22222.0, 33333.0], [20, 96, 130, 255], ChartKind::Funnel)],
    };
    let mut c = chart(ChartKind::Funnel);
    c.data_labels = false;
    let off = render(&c, &data, 500.0, 350.0, &ApproxMeasure);
    c.data_labels = true;
    let on = render(&c, &data, 500.0, 350.0, &ApproxMeasure);
    let values = ["$11,111", "$22,222", "$33,333"];
    for v in values {
        assert!(texts(&on).contains(&v), "{v} missing with labels on");
        assert!(!texts(&off).contains(&v), "{v} drawn with labels off");
    }
    for cat in ["a", "b", "c"] {
        assert!(texts(&off).contains(&cat));
        assert!(texts(&on).contains(&cat));
    }
}

/// A chartex series' own point fills reach the drawn primitives: a funnel band, a treemap box, a
/// waterfall bar and a box & whisker box take the point's colour over the series' one.
#[test]
fn a_point_fill_overrides_the_series_fill_for_chartex_kinds() {
    let fill = [0xC0, 0x00, 0x00, 0xFF];
    let line = [0x00, 0x70, 0xC0, 0xFF];
    for kind in [ChartKind::Funnel, ChartKind::Treemap, ChartKind::Waterfall, ChartKind::BoxWhisker] {
        let mut c = chart(kind);
        c.data_labels = false;
        let mut d = ChartData {
            label_font: None,
            category_levels: vec![],
            palette: vec![],
            categories: vec!["a".into(), "b".into(), "c".into()],
            series: vec![series("S", &[10.0, 20.0, 30.0], [20, 96, 130, 255], kind)],
        };
        let before = render(&c, &d, 500.0, 350.0, &ApproxMeasure);
        let series_colour = d.series[0].color;
        assert!(
            before.iter().any(|p| matches!(p, Prim::Rect { fill: Some(f), .. } if *f == series_colour)),
            "{kind:?}: the series colour is not drawn to begin with"
        );
        let bars = |prims: &[Prim]| prims.iter().filter(|p| matches!(p, Prim::Rect { fill: Some(f), .. } if *f == series_colour)).count();
        // A box & whisker draws one box per point (one per series), so its first point is index 0.
        let pi = if kind == ChartKind::BoxWhisker { 0 } else { 1 };
        d.series[0].points = vec![PointStyle { index: pi, fill: Some(fill), line: Some(line), ..PointStyle::default() }];
        let after = render(&c, &d, 500.0, 350.0, &ApproxMeasure);
        assert!(after.iter().any(|p| matches!(p, Prim::Rect { fill: Some(f), .. } if *f == fill)), "{kind:?}: the point's own fill is not drawn");
        // Its outline is the point's too.
        assert!(
            after.iter().any(|p| matches!(p, Prim::Rect { fill: Some(f), stroke: Some((s, _)), .. } if *f == fill && *s == line)),
            "{kind:?}: the point's own outline is not drawn"
        );
        // Where the kind paints every band in the series colour, the point's colour replaces one
        // rather than being drawn over it (a treemap colours its boxes from the palette instead).
        if matches!(kind, ChartKind::Funnel | ChartKind::BoxWhisker) {
            assert!(bars(&after) < bars(&before), "{kind:?}: the series colour is still drawn under the point's");
        }
    }
}

/// A label's font is resolved field by field: the chart's size and colour, the series' weight and a
/// point's own colour or size all reach the drawn label.
#[test]
fn a_label_font_is_resolved_field_by_field_for_a_chartex_kind() {
    let chart_colour = [0x11, 0x22, 0x33, 0xFF];
    let series_colour = [0x44, 0x55, 0x66, 0xFF];
    let point_colour = [0x77, 0x88, 0x99, 0xFF];
    let mut c = chart(ChartKind::Funnel);
    c.data_labels = true;
    c.labels = LabelOptions::values();
    let mut d = ChartData {
        label_font: Some(LabelStyle { size: Some(18.0), bold: None, color: Some(chart_colour) }),
        category_levels: vec![],
        palette: vec![],
        categories: vec!["a".into(), "b".into(), "c".into()],
        series: vec![series("S", &[10.0, 20.0, 30.0], [20, 96, 130, 255], ChartKind::Funnel)],
    };
    // The series sets the weight and a colour; the size stays the chart's.
    d.series[0].label_font = Some(LabelStyle { size: None, bold: Some(true), color: Some(series_colour) });
    // Point 1 sets a colour of its own, point 2 a size of its own.
    d.series[0].point_labels = vec![
        PointLabelData { index: 1, font: Some(LabelStyle { size: None, bold: None, color: Some(point_colour) }), ..PointLabelData::default() },
        PointLabelData { index: 2, font: Some(LabelStyle { size: Some(7.0), bold: None, color: None }), ..PointLabelData::default() },
    ];
    let prims = render(&c, &d, 520.0, 360.0, &ApproxMeasure);
    let label = |text: &str| {
        prims
            .iter()
            .find_map(|p| match p {
                Prim::Text { text: t, size, color, bold, .. } if t == text => Some((*size, *color, *bold)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no label {text}"))
    };
    // The values, drawn as the funnel's own labels: the chart's size, the series' weight and then
    // the point's own field over one of them.
    assert_eq!(label("$10"), (18.0, series_colour, true), "the series' weight and colour, the chart's size");
    assert_eq!(label("$20"), (18.0, point_colour, true), "the point's colour over the series' one");
    assert_eq!(label("$30"), (7.0, series_colour, true), "the point's size over the chart's");
    // The chart's own colour is the one neither the series nor the point overrode: a third point
    // with no font at all falls back to it.
    let mut d2 = d.clone();
    d2.series[0].label_font = None;
    d2.series[0].point_labels.clear();
    let prims = render(&c, &d2, 520.0, 360.0, &ApproxMeasure);
    assert!(
        prims
            .iter()
            .any(|p| matches!(p, Prim::Text { text, color, size, bold: false, .. } if text == "$10" && *color == chart_colour && *size == 18.0)),
        "the chart's font is the last level"
    );
}

#[test]
fn pie_points_follow_workbook_theme() {
    let mut wb = Workbook::new();
    wb.theme.colors[4] = 0x4F6D8A;
    wb.theme.colors[5] = 0x8AA1B8;
    let mut c = chart(ChartKind::Pie);
    c.series = vec![Series {
        name: None,
        categories: None,
        values: "{5,3}".into(),
        bubble_sizes: None,
        color: None,
        secondary: false,
        kind: None,
        quartiles: Default::default(),
        smooth: false,
        trendlines: Vec::new(),
        ..Series::default()
    }];
    let d = resolve(&wb, 0, &c);
    let expected = vec![series_color(&wb.theme, 0), series_color(&wb.theme, 1)];
    assert_eq!(expected, vec![[0x4F, 0x6D, 0x8A, 0xFF], [0x8A, 0xA1, 0xB8, 0xFF]]);
    let legend: Vec<Rgba> = legend_entries(&c, &d).iter().map(|e| e.1).collect();
    assert_eq!(legend, expected);
    let fills: Vec<Rgba> = render(&c, &d, 480.0, 288.0, &ApproxMeasure)
        .iter()
        .filter_map(|p| match p {
            Prim::Wedge { fill, .. } => Some(*fill),
            _ => None,
        })
        .collect();
    assert_eq!(fills, expected);
    for kind in [ChartKind::Doughnut, ChartKind::Treemap, ChartKind::Sunburst] {
        c.kind = kind;
        let legend: Vec<Rgba> = legend_entries(&c, &d).iter().map(|e| e.1).collect();
        assert_eq!(legend, expected, "{kind:?}");
    }
    // Past the 36-colour cycle the points keep following the workbook theme.
    c.kind = ChartKind::Pie;
    c.series[0].values = format!("{{{}}}", ["1"; 40].join(","));
    let d = resolve(&wb, 0, &c);
    let legend: Vec<Rgba> = legend_entries(&c, &d).iter().map(|e| e.1).collect();
    assert_eq!(legend.len(), 40);
    assert_eq!(legend.get(37), Some(&series_color(&wb.theme, 37)));
    assert_ne!(series_color(&wb.theme, 37), series_color(&gridcraft_model::Theme::default(), 37));
}

#[test]
fn area_charts_draw_source_value_labels_only_when_enabled() {
    for kind in [ChartKind::Area, ChartKind::AreaStacked] {
        let d = sample(kind);
        let mut c = chart(kind);
        c.data_labels = false;
        let off = render(&c, &d, 640.0, 400.0, &ApproxMeasure);
        c.data_labels = true;
        let on = render(&c, &d, 640.0, 400.0, &ApproxMeasure);
        let count = |p: &[Prim], v: &str| texts(p).iter().filter(|t| **t == v).count();
        // One label per point: 3 series x 6 categories.
        assert_eq!(texts(&on).len(), texts(&off).len() + 18, "{kind:?}");
        for v in ["$12,000", "$38,000", "$9,000", "$27,000"] {
            assert_eq!(count(&on, v), count(&off, v) + 1, "{kind:?}: {v}");
        }
        // Stacked areas label the source values, not cumulative totals (12,000 + 38,000).
        assert_eq!(count(&on, "$50,000"), count(&off, "$50,000"), "{kind:?}: cumulative total shown");
        let polys = |p: &[Prim]| p.iter().filter(|p| matches!(p, Prim::Polygon { .. })).cloned().collect::<Vec<_>>();
        assert_eq!(polys(&on), polys(&off));
        // The first series' labels are drawn after every area, so the next area can't cover them.
        let last_poly = on.iter().rposition(|p| matches!(p, Prim::Polygon { .. }));
        let first_label = on.iter().position(|p| matches!(p, Prim::Text { text, .. } if text == "$12,000"));
        assert!(first_label > last_poly, "{kind:?}: label drawn under a later area");
    }
}

#[test]
fn two_level_categories_resolve_and_draw() {
    let years = ["2023", "", "", "2024", "", ""];
    let months = ["Oct", "Nov", "Dec", "Jan", "Feb", "Mar"];
    let mut wb = Workbook::new();
    {
        let s = wb.sheet_mut(0).unwrap();
        s.name = "Sales".into();
        for (i, (y, m)) in years.iter().zip(months).enumerate() {
            // Year beside month beside the values, points down the rows…
            let r = i as u32 + 1;
            if !y.is_empty() {
                s.set_value(CellRef::new(r, 0), Value::from(*y));
                s.set_value(CellRef::new(10, i as u32), Value::from(*y));
            }
            s.set_value(CellRef::new(r, 1), Value::from(m));
            s.set_value(CellRef::new(r, 2), Value::Number(10.0 * (i as f64 + 1.0)));
            // …and the same table in rows, points along the columns.
            s.set_value(CellRef::new(11, i as u32), Value::from(m));
            s.set_value(CellRef::new(12, i as u32), Value::Number(10.0 * (i as f64 + 1.0)));
        }
    }
    let mut c = chart(ChartKind::Line);
    c.series = vec![Series { categories: Some("Sales!$A$2:$B$7".into()), values: "Sales!$C$2:$C$7".into(), ..Default::default() }];
    let d = resolve(&wb, 0, &c);
    assert_eq!(d.categories, months);
    assert_eq!(d.category_levels, vec![years.map(String::from).to_vec()]);

    let texts = |prims: &[Prim]| -> Vec<(String, f32, f32)> {
        prims.iter().filter_map(|p| if let Prim::Text { text, x, y, .. } = p { Some((text.clone(), *x, *y)) } else { None }).collect()
    };
    let prims = render(&c, &d, 600.0, 300.0, &ApproxMeasure);
    assert_sane(&prims, 600.0, 300.0);
    let t = texts(&prims);
    let at = |t: &[(String, f32, f32)], s: &str| {
        t.iter().find(|(x, _, _)| x == s).map(|(_, x, y)| (*x, *y)).unwrap_or_else(|| panic!("no {s:?} in {t:?}"))
    };
    // Six evenly spaced month labels across the plot, not twelve slots for a two-column range.
    let xs: Vec<f32> = months.iter().map(|m| at(&t, m).0).collect();
    let step = xs[1] - xs[0];
    assert!(step > 40.0 && xs.windows(2).all(|w| (w[1] - w[0] - step).abs() < 0.5), "{xs:?}");
    // The years sit in a row below the months, each under its own group.
    let (x23, y23) = at(&t, "2023");
    let (x24, y24) = at(&t, "2024");
    assert!(y23 > at(&t, "Oct").1 && (y23 - y24).abs() < 0.01, "{t:?}");
    assert!(x23 < at(&t, "Dec").0 && x24 > at(&t, "Dec").0 && x24 < at(&t, "Mar").0, "{t:?}");

    // Bars: the levels stand in a column left of the labels.
    c.kind = ChartKind::BarClustered;
    let prims = render(&c, &d, 600.0, 300.0, &ApproxMeasure);
    assert_sane(&prims, 600.0, 300.0);
    let t = texts(&prims);
    let (x23, y23) = at(&t, "2023");
    let (x_oct, y_oct) = at(&t, "Oct");
    // Categories run up the axis: the group's name sits left of its labels, between Oct and Dec.
    assert!(x23 < x_oct && y23 < y_oct && y23 > at(&t, "Dec").1, "{t:?}");
    let (x24, y24) = at(&t, "2024");
    assert!((x23 - x24).abs() < 0.01 && y24 < y23, "same column, 2024 above: {t:?}");

    // A row of values: the levels are rows.
    c.series[0].categories = Some("Sales!$A$11:$F$12".into());
    c.series[0].values = "Sales!$A$13:$F$13".into();
    let d = resolve(&wb, 0, &c);
    assert_eq!(d.categories, months);
    assert_eq!(d.category_levels, vec![years.map(String::from).to_vec()]);

    // One column of categories: a plain axis, as before.
    c.series[0].categories = Some("Sales!$B$2:$B$7".into());
    c.series[0].values = "Sales!$C$2:$C$7".into();
    let d = resolve(&wb, 0, &c);
    assert_eq!(d.categories, months);
    assert!(d.category_levels.is_empty());
}

#[test]
fn a_huge_trendline_forecast_does_not_collapse_the_columns() {
    let m = ApproxMeasure;
    let tc: Rgba = [1, 2, 3, 255];
    let col: Rgba = [10, 20, 30, 255];
    let mut s = series("Sales", &[1.0, 3.0, 2.0, 5.0, 4.0], col, ChartKind::ColumnClustered);
    s.trendlines = vec![TrendlineData { spec: Trendline { forward: 1e6, backward: 1e6, ..Trendline::default() }, color: tc }];
    let d = ChartData {
        label_font: None,
        palette: vec![],
        category_levels: vec![],
        categories: ["a", "b", "c", "d", "e"].map(String::from).to_vec(),
        series: vec![s],
    };
    let mut c = chart(ChartKind::ColumnClustered);
    c.legend = LegendPos::None;
    let prims = render(&c, &d, 600.0, 400.0, &m);
    assert_sane(&prims, 600.0, 400.0);
    let widths: Vec<f32> =
        prims.iter().filter_map(|p| if let Prim::Rect { w, fill: Some(f), .. } = p { (*f == col).then_some(*w) } else { None }).collect();
    assert_eq!(widths.len(), 5);
    assert!(widths.iter().all(|w| *w > 4.0), "columns stay visible: {widths:?}");
    let trend = lines_of(&prims, tc);
    assert_eq!(trend.len(), 1);
    assert!(trend[0].len() >= 20, "the trendline is sampled over the plot, not the forecast: {}", trend[0].len());
}

#[test]
fn box_whisker_draws_each_series_quartile_method() {
    use gridcraft_model::QuartileMethod;
    let mut c = chart(ChartKind::BoxWhisker);
    c.legend = LegendPos::None;
    c.series = vec![Series::default(), Series { quartiles: QuartileMethod::Inclusive, ..Default::default() }];
    let color = [12, 34, 56, 255];
    let d = ChartData {
        label_font: None,
        series: vec![series("A", &[1., 2., 3., 4., 5., 6.], color, c.kind), series("B", &[1., 2., 3., 4., 5., 6.], color, c.kind)],
        ..Default::default()
    };
    let prims = render(&c, &d, 640., 400., &ApproxMeasure);
    let heights: Vec<f32> = prims
        .iter()
        .filter_map(|p| match p {
            Prim::Rect { h, fill, .. } if *fill == Some(color) => Some(*h),
            _ => None,
        })
        .collect();
    assert_eq!(heights.len(), 2);
    // The boxes span 3.5 and 2.5 data units on the same value axis.
    assert!((heights[0] / heights[1] - 1.4).abs() < 0.001);
}

#[test]
fn axis_bounds_and_major_unit_set_the_scale() {
    let m = ApproxMeasure;
    let mut c = chart(ChartKind::ColumnClustered);
    c.y_axis = AxisOptions { min: Some(10000.0), max: Some(70000.0), major_unit: Some(20000.0), ..AxisOptions::default() };
    let prims = render(&c, &sample(ChartKind::ColumnClustered), 640.0, 400.0, &m);
    let t = texts(&prims);
    for want in ["$10,000", "$30,000", "$50,000", "$70,000"] {
        assert!(t.contains(&want), "no {want} label: {t:?}");
    }
    assert!(!t.contains(&"$0"), "the automatic zero bound is still there: {t:?}");
    // The same options on a log axis: powers of the base, no zero.
    let mut log = chart(ChartKind::Line);
    log.y_axis = AxisOptions { log_base: Some(10.0), ..AxisOptions::default() };
    let log_data = ChartData {
        label_font: None,
        categories: vec!["a".into(), "b".into(), "c".into()],
        series: vec![SeriesData { label_font: None, values: vec![Some(1.0), Some(100.0), Some(1000.0)], ..SeriesData::default() }],
        ..ChartData::default()
    };
    let prims = render(&log, &log_data, 640.0, 400.0, &m);
    let t = texts(&prims).join(" ");
    assert!(t.contains("1000"), "no top power: {t}");
    assert!(t.contains("10"), "no power 10: {t}");
}

#[test]
fn display_units_divide_the_axis_labels() {
    let m = ApproxMeasure;
    let mut c = chart(ChartKind::ColumnClustered);
    c.y_axis = AxisOptions { display_units: Some(DisplayUnits::Thousands), ..AxisOptions::default() };
    let prims = render(&c, &sample(ChartKind::ColumnClustered), 640.0, 400.0, &m);
    let t = texts(&prims);
    assert!(t.contains(&"Thousands"), "the unit is named beside the axis: {t:?}");
    assert!(t.contains(&"$70"), "70 000 shows as $70: {t:?}");
    assert!(!t.contains(&"$70,000"), "raw value still shown: {t:?}");
}

#[test]
fn a_reversed_axis_maps_high_values_to_the_bottom() {
    let s = crate::axis::axis_scale(0.0, 100.0, false, &AxisOptions { reversed: true, ..AxisOptions::default() });
    assert!(s.frac(s.min) > 0.99 && s.frac(s.max) < 0.01, "{s:?}");
    // A major unit on a log axis spaces the ticks by powers of the base.
    let log = crate::axis::axis_scale(1.0, 1000.0, false, &AxisOptions { log_base: Some(10.0), major_unit: Some(2.0), ..AxisOptions::default() });
    assert_eq!(log.ticks(), vec![1.0, 100.0], "{log:?}");
    // Crossing the value axis at a fixed value moves the category axis line away from zero.
    let m = ApproxMeasure;
    let axis_lines = |prims: &[Prim]| -> Vec<f32> {
        prims
            .iter()
            .filter_map(|p| match p {
                Prim::Line { pts, width, color, .. }
                    if (*width - 0.75).abs() < 1e-6
                        && *color == [0xBF, 0xBF, 0xBF, 0xFF]
                        && pts.len() == 2
                        && (pts[0][1] - pts[1][1]).abs() < 1e-6 =>
                {
                    Some(pts[0][1])
                }
                _ => None,
            })
            .collect()
    };
    let d = sample(ChartKind::ColumnClustered);
    let auto = render(&chart(ChartKind::ColumnClustered), &d, 640.0, 400.0, &m);
    let mut c = chart(ChartKind::ColumnClustered);
    c.y_axis = AxisOptions { crosses: AxisCrossing::At(30000.0), ..AxisOptions::default() };
    let crossed = render(&c, &d, 640.0, 400.0, &m);
    let y_auto = *axis_lines(&auto).first().unwrap_or(&0.0);
    let Some(y_cross) = axis_lines(&crossed).first().copied() else { panic!("no category axis line") };
    // Positive data puts the automatic category axis at the bottom; crossing at 30 000 (of a
    // 0..70 000 scale) moves it well up the plot.
    assert!(y_cross < y_auto - 40.0, "crossing did not move the axis: {y_cross} vs {y_auto}");
}

#[test]
fn data_labels_show_the_parts_asked_for() {
    let m = ApproxMeasure;
    let mut c = chart(ChartKind::ColumnClustered);
    c.labels = LabelOptions {
        font: None,
        show_value: true,
        show_category: true,
        show_series: true,
        separator: Some(" / ".into()),
        ..LabelOptions::default()
    };
    let prims = render(&c, &sample(ChartKind::ColumnClustered), 640.0, 400.0, &m);
    let t: Vec<String> = texts(&prims).iter().map(|s| (*s).to_string()).collect();
    assert!(t.iter().any(|s| s == "North / Q1 / $12,000"), "no composed label: {t:?}");
    // Only the percentage on a pie: a share of the whole.
    let mut pie = chart(ChartKind::Pie);
    pie.labels = LabelOptions { font: None, show_percentage: true, ..LabelOptions::default() };
    let prims = render(&pie, &sample(ChartKind::Pie), 640.0, 400.0, &m);
    let t = texts(&prims);
    assert!(t.iter().any(|s| s.ends_with('%')), "no percentage label: {t:?}");
}

#[test]
fn a_deleted_point_label_is_not_drawn() {
    let m = ApproxMeasure;
    let mut c = chart(ChartKind::ColumnClustered);
    c.labels = LabelOptions { font: None, show_value: true, ..LabelOptions::default() };
    let mut d = sample(ChartKind::ColumnClustered);
    d.series[0].points = vec![PointStyle { index: 0, no_label: true, ..PointStyle::default() }];
    let prims = render(&c, &d, 640.0, 400.0, &m);
    let t = texts(&prims);
    assert!(!t.contains(&"$12,000"), "the deleted label is still drawn: {t:?}");
    assert!(t.contains(&"$9,000"), "another series' label is missing: {t:?}");
}

#[test]
fn a_series_can_hide_its_labels_while_the_chart_keeps_them() {
    let m = ApproxMeasure;
    let mut c = chart(ChartKind::ColumnClustered);
    c.labels = LabelOptions { font: None, show_value: true, ..LabelOptions::default() };
    let mut d = sample(ChartKind::ColumnClustered);
    d.series[1].labels = Some(LabelOptions { font: None, hidden: true, ..LabelOptions::default() });
    let prims = render(&c, &d, 640.0, 400.0, &m);
    let t = texts(&prims);
    assert!(t.contains(&"$12,000"), "the other series keeps its labels: {t:?}");
    assert!(!t.contains(&"$38,000"), "the hidden series still shows labels: {t:?}");
}

#[test]
fn error_bars_are_drawn_with_caps() {
    let m = ApproxMeasure;
    let mut c = chart(ChartKind::ColumnClustered);
    c.data_labels = false;
    let mut d = sample(ChartKind::ColumnClustered);
    d.series[0].error_bars = Some(ErrorBarsData {
        spec: ErrorBars { direction: ErrorBarDirection::Both, kind: ErrorBarKind::Fixed(5000.0), no_end_cap: false },
        plus: vec![],
        minus: vec![],
    });
    let prims = render(&c, &d, 640.0, 400.0, &m);
    // Six points: a stem and two 6-point caps each.
    let thin: Vec<&Prim> = prims.iter().filter(|p| matches!(p, Prim::Line { width, .. } if (*width - 1.0).abs() < 1e-6)).collect();
    assert!(thin.len() >= 6 * 3, "{} error-bar lines: {thin:?}", thin.len());
    // Without caps the stems stay.
    d.series[0].error_bars.as_mut().unwrap().spec.no_end_cap = true;
    let prims = render(&c, &d, 640.0, 400.0, &m);
    let thin = prims.iter().filter(|p| matches!(p, Prim::Line { width, .. } if (*width - 1.0).abs() < 1e-6)).count();
    assert!((6..=6 * 2).contains(&thin), "{thin} lines without caps");
}

#[test]
fn per_point_fill_colours_the_right_point() {
    let m = ApproxMeasure;
    let mut c = chart(ChartKind::ColumnClustered);
    c.data_labels = false;
    let mut d = sample(ChartKind::ColumnClustered);
    let red = [0xC0, 0x00, 0x00, 0xFF];
    d.series[0].points =
        vec![gridcraft_model::PointFormat { index: 1, fill: Some(gridcraft_model::Color::Rgb(0xC00000)), ..gridcraft_model::PointFormat::default() }]
            .into_iter()
            .map(|p| PointStyle { index: p.index as usize, fill: Some(red), line: None, marker: None, marker_size: None, no_label: false })
            .collect();
    let prims = render(&c, &d, 640.0, 400.0, &m);
    let reds = prims.iter().filter(|p| matches!(p, Prim::Rect { fill: Some(f), .. } if *f == red)).count();
    assert_eq!(reds, 1, "the override must colour exactly one bar: {prims:?}");
}

#[test]
fn every_kind_renders_sanely_with_every_option_set() {
    let m = ApproxMeasure;
    for kind in ALL_KINDS {
        let mut c = chart(kind);
        c.x_axis = AxisOptions {
            min: Some(1.0),
            max: Some(5.0),
            major_unit: Some(2.0),
            reversed: true,
            crosses: AxisCrossing::At(2.0),
            number_format: Some("0".into()),
            ..AxisOptions::default()
        };
        c.y_axis = AxisOptions {
            min: Some(-100_000.0),
            max: Some(100_000.0),
            major_unit: Some(5_000.0),
            log_base: Some(10.0),
            reversed: true,
            crosses: AxisCrossing::Max,
            display_units: Some(DisplayUnits::Millions),
            number_format: Some("0.0".into()),
        };
        c.labels = LabelOptions {
            font: None,
            show_value: true,
            show_percentage: true,
            show_series: true,
            show_category: true,
            show_legend_key: true,
            position: LabelPosition::OutsideEnd,
            separator: Some(" | ".into()),
            number_format: Some("0.0".into()),
            hidden: false,
        };
        let mut d = sample(kind);
        for (i, s) in d.series.iter_mut().enumerate() {
            s.labels = Some(LabelOptions { font: None, show_category: true, position: LabelPosition::Center, ..LabelOptions::default() });
            s.error_bars = Some(ErrorBarsData {
                spec: ErrorBars {
                    direction: match i % 3 {
                        0 => ErrorBarDirection::Both,
                        1 => ErrorBarDirection::Plus,
                        _ => ErrorBarDirection::Minus,
                    },
                    kind: match i % 4 {
                        0 => ErrorBarKind::Fixed(1_000.0),
                        1 => ErrorBarKind::Percentage(50.0),
                        2 => ErrorBarKind::StdDev(2.0),
                        _ => ErrorBarKind::StdErr,
                    },
                    no_end_cap: i % 2 == 0,
                },
                plus: vec![Some(1.0), None, Some(2.0)],
                minus: vec![Some(2.0), Some(1.0)],
            });
            s.points = vec![PointStyle {
                index: i,
                fill: Some([0xC0, 0x00, 0x00, 0xFF]),
                line: Some([0x00, 0xB0, 0x50, 0xFF]),
                marker: Some([0x00, 0x70, 0xC0, 0xFF]),
                marker_size: Some(9),
                no_label: i % 2 == 0,
            }];
        }
        let (w, h) = (640.0, 400.0);
        let prims = render(&c, &d, w, h, &m);
        assert_sane(&prims, w, h);
    }
}

#[test]
fn hostile_chart_options_do_not_panic_or_go_non_finite() {
    let m = ApproxMeasure;
    let mut c = chart(ChartKind::Scatter);
    c.y_axis = AxisOptions {
        min: Some(f64::NAN),
        max: Some(f64::INFINITY),
        major_unit: Some(f64::NAN),
        log_base: Some(1e300),
        reversed: true,
        crosses: AxisCrossing::At(f64::NAN),
        display_units: None,
        number_format: Some("General".into()),
    };
    c.x_axis = AxisOptions { min: Some(-1e300), max: Some(1e300), major_unit: Some(1e-300), ..AxisOptions::default() };
    c.labels = LabelOptions { font: None, show_value: true, ..LabelOptions::default() };
    let mut d = sample(ChartKind::Scatter);
    d.series[0].error_bars = Some(ErrorBarsData {
        spec: ErrorBars { direction: ErrorBarDirection::Both, kind: ErrorBarKind::Fixed(1e300), no_end_cap: false },
        plus: vec![],
        minus: vec![],
    });
    d.series[0].points = vec![PointStyle { index: usize::MAX, fill: Some([1, 2, 3, 255]), ..PointStyle::default() }];
    let prims = render(&c, &d, 640.0, 400.0, &m);
    assert_sane(&prims, 640.0, 400.0);
}

#[test]
fn category_bounds_crop_and_extend_the_axis() {
    let m = ApproxMeasure;
    let d = sample(ChartKind::ColumnClustered);
    let plain = render(&chart(ChartKind::ColumnClustered), &d, 640.0, 400.0, &m);
    assert!(texts(&plain).contains(&"Q1"), "the whole axis by default");
    let mut c = chart(ChartKind::ColumnClustered);
    // Excel numbers categories from 1: bounds 2..4 show Q2, Q3 and Q4 only.
    c.x_axis = AxisOptions { min: Some(2.0), max: Some(4.0), ..AxisOptions::default() };
    let prims = render(&c, &d, 640.0, 400.0, &m);
    let t = texts(&prims);
    assert!(!t.contains(&"Q1") && !t.contains(&"Q5") && !t.contains(&"Q6"), "categories outside the bounds: {t:?}");
    assert!(t.contains(&"Q2") && t.contains(&"Q3") && t.contains(&"Q4"), "the shown categories: {t:?}");
    assert_sane(&prims, 640.0, 400.0);
    // Every bar sits inside the plot: the three shown ones are drawn, the rest are not.
    let bars = prims.iter().filter(|p| matches!(p, Prim::Rect { .. })).count();
    let wide = render(&chart(ChartKind::ColumnClustered), &d, 640.0, 400.0, &m).iter().filter(|p| matches!(p, Prim::Rect { .. })).count();
    assert!(bars < wide, "cropping drew as many bars as the full axis: {bars} vs {wide}");
    // A range that leaves no categories is ignored rather than drawn as an empty plot.
    let mut bad = chart(ChartKind::ColumnClustered);
    bad.x_axis = AxisOptions { min: Some(4.0), max: Some(2.0), ..AxisOptions::default() };
    assert!(texts(&render(&bad, &d, 640.0, 400.0, &m)).contains(&"Q1"));
    // Extending the axis past the data leaves blank slots, and the frame stays inside its box.
    let mut ext = chart(ChartKind::ColumnClustered);
    ext.x_axis = AxisOptions { min: Some(5.0), max: Some(9.0), ..AxisOptions::default() };
    let p = render(&ext, &d, 640.0, 400.0, &m);
    assert_sane(&p, 640.0, 400.0);
    // Categories 7..9 of the extended axis are empty slots; 5 and 6 are still shown.
    assert!(texts(&p).contains(&"Q5") && texts(&p).contains(&"Q6") && !texts(&p).contains(&"Q1"), "{:?}", texts(&p));
}

#[test]
fn the_secondary_axis_takes_its_own_options() {
    let m = ApproxMeasure;
    let d = sample(ChartKind::Combo);
    let mut c = chart(ChartKind::Combo);
    c.y2_axis = AxisOptions { min: Some(0.0), max: Some(10.0), major_unit: Some(2.5), number_format: Some("0.0".into()), ..AxisOptions::default() };
    let prims = render(&c, &d, 640.0, 400.0, &m);
    let t = texts(&prims);
    for want in ["0.0", "2.5", "5.0", "7.5", "10.0"] {
        assert!(t.contains(&want), "no right-axis label {want}: {t:?}");
    }
    // The primary axis keeps its own scale (a $0..$70,000 axis on the left).
    assert!(t.contains(&"$30,000"), "the left axis lost its scale: {t:?}");
    assert!(!t.contains(&"$10"), "the secondary scale leaked into the primary labels: {t:?}");
    // Units on the secondary axis are named beside it.
    let mut units = chart(ChartKind::Combo);
    units.y2_axis = AxisOptions { display_units: Some(DisplayUnits::Thousands), ..AxisOptions::default() };
    let t2 = render(&units, &d, 640.0, 400.0, &m);
    let names = texts(&t2).iter().filter(|t| **t == "Thousands").count();
    assert_eq!(names, 1, "the secondary unit is named once: {:?}", texts(&t2));
}

#[test]
fn a_category_crossing_moves_the_value_axis_to_a_category() {
    let m = ApproxMeasure;
    let d = sample(ChartKind::ColumnClustered);
    let verticals = |prims: &[Prim]| -> Vec<f32> {
        prims
            .iter()
            .filter_map(|p| match p {
                Prim::Line { pts, width, .. } if (*width - 0.75).abs() < 1e-6 && pts.len() == 2 && (pts[0][0] - pts[1][0]).abs() < 1e-6 => {
                    Some(pts[0][0])
                }
                _ => None,
            })
            .collect()
    };
    let auto = render(&chart(ChartKind::ColumnClustered), &d, 640.0, 400.0, &m);
    assert!(verticals(&auto).is_empty(), "an automatic category axis draws no value-axis line");
    let mut c = chart(ChartKind::ColumnClustered);
    c.x_axis = AxisOptions { crosses: AxisCrossing::At(3.0), ..AxisOptions::default() };
    let prims = render(&c, &d, 640.0, 400.0, &m);
    let xs = verticals(&prims);
    assert_eq!(xs.len(), 1, "one value-axis line at the crossing: {xs:?}");
    assert_sane(&prims, 640.0, 400.0);
    // Crossing after the last category puts it at the right edge.
    let mut last = chart(ChartKind::ColumnClustered);
    last.x_axis = AxisOptions { crosses: AxisCrossing::At(6.0), ..AxisOptions::default() };
    let far = verticals(&render(&last, &d, 640.0, 400.0, &m));
    assert!(far.first().is_some_and(|x| *x > xs.first().copied().unwrap_or(0.0)), "crossing at 6 is right of 3: {far:?}");
}

#[test]
fn point_labels_draw_their_own_text_and_position() {
    let mut c = chart(ChartKind::LineMarkers);
    c.data_labels = true;
    c.labels = LabelOptions::values();
    // The two labelled points share a value, so only the label position moves one.
    let mut s = series("East", &[10.5, 12.5, 12.5, 16.5], [200, 80, 40, 255], ChartKind::LineMarkers);
    s.number_format = "0.0".into();
    s.point_labels = vec![
        // The series shows values, so a point with its own text keeps its value unless it says not.
        PointLabelData {
            font: None,
            index: 1,
            text: Some("Above".into()),
            show_value: Some(false),
            position: LabelPosition::Top,
            ..PointLabelData::default()
        },
        PointLabelData {
            font: None,
            index: 2,
            text: Some("Below".into()),
            show_value: Some(false),
            position: LabelPosition::Bottom,
            ..PointLabelData::default()
        },
        // Every part switched off for this point: its label shows nothing.
        PointLabelData { font: None, index: 3, text: None, show_value: Some(false), ..PointLabelData::default() },
    ];
    let d = ChartData { label_font: None, categories: vec!["A".into(), "B".into(), "C".into(), "D".into()], series: vec![s], ..ChartData::default() };
    let prims = render(&c, &d, 640.0, 400.0, &ApproxMeasure);
    let t = texts(&prims);
    assert!(t.contains(&"Above") && t.contains(&"Below"), "both point texts are drawn: {t:?}");
    // The point without an override still shows the value label the options ask for.
    assert!(t.contains(&"10.5"), "{t:?}");
    let pos = |needle: &str| {
        prims
            .iter()
            .find_map(|p| match p {
                Prim::Text { text, x, y, .. } if text == needle => Some((*x, *y)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("`{needle}` not drawn"))
    };
    let (ax, ay) = pos("Above");
    let (bx, by) = pos("Below");
    assert!(ax < bx, "each label sits at its own point: {ax} {bx}");
    assert!(by > ay + 4.0, "a Bottom label is drawn below a Top one: {ay} {by}");
    // The point whose parts are switched off shows no value at all.
    assert!(!t.contains(&"16.5"), "a point with every part off still draws a label: {t:?}");
}

#[test]
fn a_point_flag_overrides_the_series_labels() {
    let mut c = chart(ChartKind::ColumnClustered);
    c.data_labels = true;
    c.labels = LabelOptions { font: None, show_value: true, ..LabelOptions::default() };
    let mut s = series("East", &[7.0, 9.0, 11.0], [200, 80, 40, 255], ChartKind::ColumnClustered);
    s.number_format = "General".into();
    s.point_labels =
        vec![PointLabelData { font: None, index: 1, text: None, show_value: Some(false), show_category: Some(true), ..PointLabelData::default() }];
    let d = ChartData { label_font: None, categories: vec!["A".into(), "B".into(), "C".into()], series: vec![s], ..ChartData::default() };
    let prims = render(&c, &d, 640.0, 400.0, &ApproxMeasure);
    let t = texts(&prims);
    // The overridden point shows its category instead of its value; the others keep the value.
    assert!(t.contains(&"B"), "{t:?}");
    assert!(!t.contains(&"9"), "the point's value is switched off: {t:?}");
    assert!(t.contains(&"7") && t.contains(&"11"), "{t:?}");
}

// ---------------------------------------------------------------- click targets

/// Every region is sane and inside the canvas, and the whole canvas is the chart area.
fn assert_hits_sane(d: &Drawing, w: f32, h: f32) {
    let (w, h) = (f64::from(w), f64::from(h));
    let inside = |x: f64, y: f64| x >= -1.0 && x <= w + 1.0 && y >= -1.0 && y <= h + 1.0;
    for r in &d.hits {
        match r.shape {
            HitShape::Rect { x, y, w: rw, h: rh } => {
                assert!(f64::from(x).is_finite() && f64::from(rw).is_finite(), "non-finite rect: {:?}", r.shape);
                assert!(rw > 0.0 && rh > 0.0, "empty rect: {:?}", r.shape);
                assert!(inside(f64::from(x), f64::from(y)) && inside(f64::from(x + rw), f64::from(y + rh)), "{:?}", r.shape);
            }
            HitShape::Circle { cx, cy, r: rr } => {
                assert!(rr > 0.0 && rr.is_finite());
                assert!(inside(f64::from(cx), f64::from(cy)), "{:?}", r.shape);
            }
            HitShape::Wedge { cx, cy, r_outer, r_inner, a0, a1 } => {
                assert!(r_outer > 0.0 && r_inner >= 0.0 && a1 >= a0 && a1.is_finite(), "{:?}", r.shape);
                assert!(inside(f64::from(cx), f64::from(cy)), "{:?}", r.shape);
            }
        }
    }
    // The background is always the chart area, whatever the kind.
    assert_eq!(hit_at(&d.hits, 1.0, 1.0), Some(Hit::ChartArea), "the corner is the chart area");
}

#[test]
fn every_kind_records_sane_click_regions() {
    let m = ApproxMeasure;
    for kind in ALL_KINDS {
        let c = chart(kind);
        let d = sample(kind);
        let (w, h) = (640.0, 400.0);
        let drawing = draw(&c, &d, w, h, &m);
        assert_hits_sane(&drawing, w, h);
        assert_eq!(drawing.prims, render(&c, &d, w, h, &m), "{kind:?}: the primitives are the same either way");
    }
}

#[test]
fn a_bar_click_names_its_series_and_point() {
    let m = ApproxMeasure;
    let (w, h) = (640.0, 400.0);
    let c = chart(ChartKind::ColumnClustered);
    let d = sample(ChartKind::ColumnClustered);
    let drawing = draw(&c, &d, w, h, &m);
    // A bar's own region: clicking its middle must name that series and that point.
    let bars: Vec<(Hit, f32, f32)> = drawing
        .hits
        .iter()
        .filter(|r| matches!(r.hit, Hit::Series { point: Some(_), .. }))
        .filter_map(|r| match r.shape {
            HitShape::Rect { x, y, w: rw, h: rh } => Some((r.hit, x + rw / 2.0, y + rh / 2.0)),
            _ => None,
        })
        .collect();
    assert!(bars.len() >= 3, "one region per bar: {} regions", bars.len());
    for (hit, x, y) in &bars {
        assert_eq!(hit_at(&drawing.hits, *x, *y), Some(*hit), "a click on a bar at ({x}, {y})");
    }
    // The bars of two different series are told apart.
    let hits: Vec<Hit> = bars.iter().map(|(h, _, _)| *h).collect();
    assert!(hits.contains(&(Hit::Series { index: 0, point: Some(0) })), "{hits:?}");
    assert!(hits.contains(&(Hit::Series { index: 1, point: Some(0) })), "{hits:?}");
}

#[test]
fn the_axes_and_the_pie_slices_are_where_they_are_drawn() {
    let m = ApproxMeasure;
    let (w, h) = (640.0, 400.0);
    // The category labels sit under the plot: a click on them is the horizontal axis.
    let mut c = chart(ChartKind::ColumnClustered);
    c.legend = LegendPos::None;
    let d = sample(ChartKind::ColumnClustered);
    let drawing = draw(&c, &d, w, h, &m);
    let plot_bottom = drawing
        .hits
        .iter()
        .filter_map(|r| match (r.hit, r.shape) {
            (Hit::AxisX, HitShape::Rect { y, h: rh, .. }) => Some(y + rh),
            _ => None,
        })
        .fold(0.0f32, f32::max);
    assert!(plot_bottom < h, "the label band ends before the canvas does: {plot_bottom}");
    assert_eq!(hit_at(&drawing.hits, w / 2.0, plot_bottom - 1.0), Some(Hit::AxisX), "the label band is the category axis");
    // The value axis' label band (past the rotated title) is the vertical axis.
    let band = drawing
        .hits
        .iter()
        .find_map(|r| match (r.hit, r.shape) {
            (Hit::AxisY, HitShape::Rect { x, y, h: rh, .. }) if rh > 50.0 => Some((x, y, rh)),
            _ => None,
        })
        .expect("a value axis label band");
    assert_eq!(hit_at(&drawing.hits, band.0 + 2.0, band.1 + band.2 / 2.0), Some(Hit::AxisY), "the label band is the value axis");

    // A pie's slice region is the wedge the renderer drew: its bisector at half the radius hits it.
    let mut c = chart(ChartKind::Pie);
    c.legend = LegendPos::Right;
    let d = sample(ChartKind::Pie);
    let drawing = draw(&c, &d, w, h, &m);
    let wedges: Vec<(f32, f32, f32, f32, f32)> = drawing
        .prims
        .iter()
        .filter_map(|p| match p {
            Prim::Wedge { cx, cy, r_outer, a0, a1, .. } => Some((*cx, *cy, *r_outer, *a0, *a1)),
            _ => None,
        })
        .collect();
    assert!(wedges.len() >= 3, "a slice per value: {}", wedges.len());
    for (j, &(cx, cy, r, a0, a1)) in wedges.iter().enumerate().take(6) {
        let mid = (a0 + a1) / 2.0;
        let (x, y) = (cx + r * 0.5 * mid.sin(), cy - r * 0.5 * mid.cos());
        assert_eq!(hit_at(&drawing.hits, x, y), Some(Hit::Series { index: 0, point: Some(j) }), "slice {j}");
    }
}

#[test]
fn a_hostile_click_is_outside_every_region() {
    let m = ApproxMeasure;
    let drawing = draw(&chart(ChartKind::ColumnClustered), &sample(ChartKind::ColumnClustered), 640.0, 400.0, &m);
    for (x, y) in [(f32::NAN, 10.0), (10.0, f32::NAN), (f32::INFINITY, 0.0), (-1e9, -1e9), (1e9, 1e9)] {
        assert_eq!(hit_at(&drawing.hits, x, y), None, "({x}, {y}) is outside every region");
    }
    assert_eq!(draw(&chart(ChartKind::Pie), &ChartData::default(), 0.0, 0.0, &m), Drawing::default(), "no size, no drawing");
}

#[test]
fn a_label_font_sets_its_size_weight_and_colour() {
    let m = ApproxMeasure;
    let red = [0xFFu8, 0x00, 0x00, 0xFF];
    let mut c = chart(ChartKind::ColumnClustered);
    c.data_labels = true;
    c.labels = LabelOptions { font: None, show_value: true, ..LabelOptions::default() };
    let mut d = sample(ChartKind::ColumnClustered);
    // The chart's labels: 14 pt, bold.
    d.label_font = Some(LabelStyle { size: Some(14.0), bold: Some(true), ..LabelStyle::default() });
    // Series 1 overrides the size only; its weight still comes from the chart.
    d.series[1].label_font = Some(LabelStyle { size: Some(6.0), ..LabelStyle::default() });
    // Point 0 of series 0 is its own label: red, and the chart's size is replaced.
    d.series[0].point_labels = vec![PointLabelData {
        index: 0,
        font: Some(LabelStyle { size: Some(20.0), color: Some(red), ..LabelStyle::default() }),
        ..PointLabelData::default()
    }];
    let prims = render(&c, &d, 640.0, 400.0, &m);
    let labels: Vec<(f32, bool, Rgba)> = prims
        .iter()
        .filter_map(|p| match p {
            Prim::Text { size, bold, color, text, .. } if text.contains(',') || text.contains("000") => Some((*size, *bold, *color)),
            _ => None,
        })
        .collect();
    // The point sets a size and a colour; its weight still comes from the chart.
    assert!(labels.iter().any(|(s, b, col)| *s == 20.0 && *b && *col == red), "the point's own label is 20 pt, bold and red: {labels:?}");
    assert!(labels.iter().any(|(s, b, _)| *s == 14.0 && *b), "the chart's labels are 14 pt bold: {labels:?}");
    assert!(labels.iter().any(|(s, b, _)| *s == 6.0 && *b), "the series takes the chart's weight with its own size: {labels:?}");

    // A hostile size in a file is not drawn: it is clamped to a drawable one.
    d.series[0].point_labels[0].font = Some(LabelStyle { size: Some(1e30), ..LabelStyle::default() });
    let prims = render(&c, &d, 640.0, 400.0, &m);
    assert!(prims.iter().all(|p| !matches!(p, Prim::Text { size, .. } if !size.is_finite() || *size > 400.0)), "no absurd text");
}

// ---------------------------------------------------------------- combo charts: one category axis, two value axes

/// A chart with no title, legend, data labels or gridlines: the numbers these tests pin belong to
/// the plot, not to its decoration.
fn bare(kind: ChartKind) -> Chart {
    let mut c = chart(kind);
    c.kind = kind;
    c.title = None;
    c.legend = LegendPos::None;
    c.data_labels = false;
    c.gridlines = false;
    c.x_title = None;
    c.y_title = None;
    c
}

/// First series colour of the combo tests.
const P: Rgba = [10, 20, 30, 255];
/// Second series colour of the combo tests.
const S: Rgba = [200, 30, 40, 255];

/// Series of values drawn as one kind in one colour, optionally against the secondary value axis.
fn drawn(name: &str, values: &[f64], colour: Rgba, kind: ChartKind, secondary: bool) -> SeriesData {
    let mut s = series(name, values, colour, kind);
    s.number_format = "General".into();
    s.secondary = secondary;
    s
}

fn cats(n: usize) -> Vec<String> {
    ["A", "B", "C", "D"].iter().take(n).map(|s| s.to_string()).collect()
}

/// The plot rectangle (left, width), read from the band a click on the category axis selects: it
/// runs along the bottom of the plot and is as wide as it.
fn plot_span(d: &Drawing) -> (f32, f32) {
    d.hits
        .iter()
        .filter_map(|r| match (&r.hit, &r.shape) {
            (Hit::AxisX, HitShape::Rect { x, w, h, .. }) if *h > 4.0 && *w > 4.0 => Some((*x, *w)),
            _ => None,
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .expect("the category axis is a click target")
}

/// The (left, width) of every bar filled in a colour, in draw order.
fn bars(prims: &[Prim], colour: Rgba) -> Vec<(f32, f32)> {
    prims
        .iter()
        .filter_map(|p| match p {
            Prim::Rect { x, w, fill: Some(f), .. } if *f == colour && *w > 1.0 => Some((*x, *w)),
            _ => None,
        })
        .collect()
}

/// The points of the polyline drawn in a colour (a line series' path, one point per category).
fn polyline(prims: &[Prim], colour: Rgba) -> Vec<[f32; 2]> {
    prims
        .iter()
        .find_map(|p| match p {
            Prim::Line { pts, color, dash: false, .. } if *color == colour && pts.len() > 2 => Some(pts.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// The centres of the markers drawn in a colour.
fn markers(prims: &[Prim], colour: Rgba) -> Vec<f32> {
    prims
        .iter()
        .filter_map(|p| match p {
            Prim::Circle { cx, fill: Some(f), .. } if *f == colour => Some(*cx),
            _ => None,
        })
        .collect()
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

fn assert_near(what: &str, a: f32, b: f32) {
    assert!(near(a, b), "{what}: {a} != {b}");
}

/// (a) Two column series, one of them on the secondary axis: one category slot per category, both
/// groups inside it, the second group beside the first (never on top of it), the pair centred on
/// the slot. Excel's default gap width (150%) is what makes each bar cw / 3.5 wide for two bars.
#[test]
fn combo_bar_groups_share_the_category_slot() {
    let m = ApproxMeasure;
    let d = ChartData {
        categories: cats(3),
        series: vec![
            drawn("primary", &[1.0, 2.0, 3.0], P, ChartKind::ColumnClustered, false),
            drawn("secondary", &[10.0, 20.0, 30.0], S, ChartKind::ColumnClustered, true),
        ],
        ..ChartData::default()
    };
    let drawing = draw(&bare(ChartKind::Combo), &d, 640.0, 400.0, &m);
    let (px, pw) = plot_span(&drawing);
    let cw = pw / 3.0;
    let bw = cw / 3.5;
    let (p, s) = (bars(&drawing.prims, P), bars(&drawing.prims, S));
    assert_eq!(p.len(), 3, "three primary bars: {p:?}");
    assert_eq!(s.len(), 3, "three secondary bars: {s:?}");
    for j in 0..3 {
        let centre = px + (j as f32 + 0.5) * cw;
        let (left, right) = (px + j as f32 * cw, px + (j as f32 + 1.0) * cw);
        assert_near("bar width", p[j].1, bw);
        assert_near("bar width", s[j].1, bw);
        // Side by side: the primary bar ends exactly where the secondary one starts.
        assert_near("the groups meet, they do not overlap", p[j].0 + p[j].1, s[j].0);
        // And the pair is centred on the slot, so neither group is half-offset.
        assert_near("the pair is centred on the slot", (p[j].0 + s[j].0 + s[j].1) / 2.0, centre);
        assert!(p[j].0 >= left - 0.01 && s[j].0 + s[j].1 <= right + 0.01, "inside slot {j}: {:?} {:?}", p[j], s[j]);
    }
    // The exact geometry of the 640 x 400 chart: moving it moves every two-group column chart.
    assert_near("plot left", px, 30.85);
    assert_near("plot width", pw, 583.25);
    assert!(near(p[0].0, 72.51) && near(p[0].1, 55.55), "primary bar 0: {:?}", p[0]);
    assert!(near(s[0].0, 128.06) && near(s[0].1, 55.55), "secondary bar 0: {:?}", s[0]);
    assert!(near(p[2].0, 461.34), "primary bar 2: {:?}", p[2]);
    assert!(near(s[2].0, 516.89), "secondary bar 2: {:?}", s[2]);
}

/// (b) A column series and a line series on the secondary axis: the single column group fills the
/// slot (a bar is cw / 2.5 wide) and the line's points sit on the slot centres, read against the
/// secondary scale (reversed here, so the heights prove which axis they come from).
#[test]
fn combo_line_on_the_secondary_axis_meets_the_bar_centre() {
    let m = ApproxMeasure;
    let mut c = bare(ChartKind::Combo);
    c.y2_axis.reversed = true;
    let d = ChartData {
        categories: cats(3),
        series: vec![
            drawn("primary", &[1.0, 2.0, 3.0], P, ChartKind::ColumnClustered, false),
            drawn("secondary", &[10.0, 20.0, 30.0], S, ChartKind::LineMarkers, true),
        ],
        ..ChartData::default()
    };
    let drawing = draw(&c, &d, 640.0, 400.0, &m);
    let (px, pw) = plot_span(&drawing);
    let cw = pw / 3.0;
    let b = bars(&drawing.prims, P);
    let pts = polyline(&drawing.prims, S);
    assert_eq!(b.len(), 3, "three bars: {b:?}");
    assert_eq!(pts.len(), 3, "three line points: {pts:?}");
    for j in 0..3 {
        let centre = px + (j as f32 + 0.5) * cw;
        assert_near("bar width", b[j].1, cw / 2.5);
        assert_near("the bar is centred on the slot", b[j].0 + b[j].1 / 2.0, centre);
        assert_near("the line point is on the slot centre", pts[j][0], centre);
    }
    // The exact x of the line's points in the 640 x 400 chart.
    assert!(near(pts[0][0], 128.06) && near(pts[2][0], 516.89), "{pts:?}");
    // A reversed secondary axis draws 30 below 10: the line is plotted against y2, not the primary.
    assert!(pts[0][1] < pts[2][1], "the line reads the secondary axis: {pts:?}");
    // Markers land on the same centres.
    let mut mk = markers(&drawing.prims, S);
    let mut want: Vec<f32> = (0..3).map(|j| px + (j as f32 + 0.5) * cw).collect();
    mk.sort_by(f32::total_cmp);
    want.sort_by(f32::total_cmp);
    assert_eq!(mk.len(), 3, "a marker per point: {mk:?}");
    for (a, b) in mk.iter().zip(&want) {
        assert_near("marker centre", *a, *b);
    }
}

/// (c) A scatter series on the secondary axis under a category primary. Excel refuses to pair the
/// two, so the series is drawn as a category line on the secondary scale: its points keep the
/// primary's slot centres and its numeric X values are not used.
#[test]
fn combo_scatter_on_the_secondary_axis_keeps_the_category_slots() {
    let m = ApproxMeasure;
    let c = bare(ChartKind::Combo);
    let with_x = |xs: [f64; 3]| {
        let dots = SeriesData { x: Some(xs.iter().map(|x| Some(*x)).collect()), ..drawn("dots", &[10.0, 20.0, 30.0], S, ChartKind::Scatter, true) };
        ChartData {
            categories: cats(3),
            series: vec![drawn("primary", &[1.0, 2.0, 3.0], P, ChartKind::ColumnClustered, false), dots],
            ..ChartData::default()
        }
    };
    let drawing = draw(&c, &with_x([10.0, 20.0, 30.0]), 640.0, 400.0, &m);
    let (px, pw) = plot_span(&drawing);
    let cw = pw / 3.0;
    let pts = polyline(&drawing.prims, S);
    assert_eq!(pts.len(), 3, "one point per category: {pts:?}");
    for (j, p) in pts.iter().enumerate() {
        assert_near("the scatter point is on the category slot centre", p[0], px + (j as f32 + 0.5) * cw);
    }
    // The X values do not move it: the same picture comes from any of them.
    let other = draw(&c, &with_x([1.0, 2.0, 3.0]), 640.0, 400.0, &m);
    assert_eq!(polyline(&other.prims, S), pts, "a category plot ignores the scatter's X values");
}

/// A reversed and cropped category axis moves both axes' groups together: the secondary bar keeps
/// the slot the primary one is in, wherever that slot is.
#[test]
fn combo_bar_groups_keep_their_slot_on_a_reversed_cropped_axis() {
    let m = ApproxMeasure;
    let mut c = bare(ChartKind::Combo);
    c.x_axis.reversed = true;
    c.x_axis.min = Some(2.0);
    c.x_axis.max = Some(4.0);
    let d = ChartData {
        categories: cats(4),
        series: vec![
            drawn("primary", &[1.0, 2.0, 3.0, 4.0], P, ChartKind::ColumnClustered, false),
            drawn("secondary", &[10.0, 20.0, 30.0, 40.0], S, ChartKind::ColumnClustered, true),
        ],
        ..ChartData::default()
    };
    let drawing = draw(&c, &d, 640.0, 400.0, &m);
    let (px, pw) = plot_span(&drawing);
    let cw = pw / 3.0;
    let (p, s) = (bars(&drawing.prims, P), bars(&drawing.prims, S));
    assert_eq!(p.len(), 3, "categories 2..4 only: {p:?}");
    assert_eq!(s.len(), 3);
    for i in 0..3 {
        // Categories 2..4 with the axis reversed: category 2 is the right-hand slot.
        let centre = px + (2.0 - i as f32 + 0.5) * cw;
        assert_near("the pair is centred on the slot", (p[i].0 + s[i].0 + s[i].1) / 2.0, centre);
        assert_near("side by side", p[i].0 + p[i].1, s[i].0);
    }
}

/// The single-axis charts the combo layout sits on: one column group of one, two and three series,
/// and a line's points. These are the numbers a regression would move.
#[test]
fn single_axis_geometry_is_unchanged() {
    let m = ApproxMeasure;
    let data = |n: usize| ChartData {
        categories: cats(4),
        series: (0..n).map(|i| drawn(&format!("s{i}"), &[1.0, 2.0, 3.0, 4.0], P, ChartKind::ColumnClustered, false)).collect(),
        ..ChartData::default()
    };
    // One series: a bar is cw / 2.5 wide (gap width 150%) and centred on the slot.
    let one = draw(&bare(ChartKind::ColumnClustered), &data(1), 640.0, 400.0, &m);
    let (px, pw) = plot_span(&one);
    let cw = pw / 4.0;
    assert_near("plot left", px, 20.95);
    assert_near("plot width", pw, 605.05);
    let b = bars(&one.prims, P);
    assert_eq!(b.len(), 4);
    for (j, bar) in b.iter().enumerate() {
        assert_near("bar width", bar.1, cw / 2.5);
        assert_near("centred", bar.0 + bar.1 / 2.0, px + (j as f32 + 0.5) * cw);
    }
    assert!(near(b[0].0, 66.33) && near(b[0].1, 60.50), "first bar: {:?}", b[0]);
    // More series narrow the bars in the same slot: 3.5 and 4.5 bar widths per slot.
    for (n, per) in [(2usize, 3.5f32), (3, 4.5)] {
        let d = draw(&bare(ChartKind::ColumnClustered), &data(n), 640.0, 400.0, &m);
        let (px, pw) = plot_span(&d);
        let (cw, bw) = (pw / 4.0, pw / 4.0 / per);
        let b = bars(&d.prims, P);
        assert_eq!(b.len(), 4 * n, "{n} series: {b:?}");
        for j in 0..4 {
            let centre = px + (j as f32 + 0.5) * cw;
            for k in 0..n {
                // Bars are drawn series by series, a whole series before the next one.
                let bar = b[k * 4 + j];
                assert_near("bar width", bar.1, bw);
                assert_near("bars do not overlap", bar.0, centre - n as f32 * bw / 2.0 + k as f32 * bw);
            }
        }
    }
    // A line: the points are the slot centres.
    let line = ChartData { series: vec![drawn("line", &[1.0, 2.0, 3.0, 4.0], P, ChartKind::LineMarkers, false)], ..data(1) };
    let d = draw(&bare(ChartKind::LineMarkers), &line, 640.0, 400.0, &m);
    let (px, pw) = plot_span(&d);
    let cw = pw / 4.0;
    let pts = polyline(&d.prims, P);
    assert_eq!(pts.len(), 4);
    for (j, p) in pts.iter().enumerate() {
        assert_near("line point on the slot centre", p[0], px + (j as f32 + 0.5) * cw);
    }
}
