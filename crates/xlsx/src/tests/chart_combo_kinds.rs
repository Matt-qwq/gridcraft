//! Importing a combination chart keeps each series' own group kind, the first group's included.

use gridcraft_model::ChartKind;

use crate::chart::read_chart;
use crate::xml::parse;

fn ser(i: usize) -> String {
    format!("<c:ser><c:idx val=\"{i}\"/><c:order val=\"{i}\"/><c:val><c:numRef><c:f>Data!$A$1:$A$5</c:f></c:numRef></c:val></c:ser>")
}

const AX: &str = "<c:axId val=\"1\"/><c:axId val=\"2\"/>";

fn bar(sers: &str) -> String {
    format!("<c:barChart><c:barDir val=\"col\"/><c:grouping val=\"clustered\"/>{sers}{AX}</c:barChart>")
}

fn line(sers: &str) -> String {
    format!("<c:lineChart><c:grouping val=\"standard\"/>{sers}{AX}</c:lineChart>")
}

fn kinds(first: &str, second: &str) -> Vec<Option<ChartKind>> {
    let xml = format!(
        "<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\">\
         <c:chart><c:plotArea>{first}{second}</c:plotArea></c:chart></c:chartSpace>"
    );
    let root = parse(xml.as_bytes()).unwrap();
    let ch = read_chart(&root).unwrap();
    assert_eq!(ch.kind, ChartKind::Combo);
    ch.series.iter().map(|s| s.kind).collect()
}

#[test]
fn first_group_with_two_columns_stays_columns() {
    let k = kinds(&bar(&(ser(0) + &ser(1))), &line(&ser(2)));
    assert_eq!(k[0], Some(ChartKind::ColumnClustered));
    assert_eq!(k[1], Some(ChartKind::ColumnClustered));
    assert!(matches!(k[2], Some(ChartKind::Line | ChartKind::LineMarkers)));
}

#[test]
fn line_first_group_stays_a_line() {
    let k = kinds(&line(&ser(0)), &bar(&ser(1)));
    assert!(matches!(k[0], Some(ChartKind::Line | ChartKind::LineMarkers)));
    assert_eq!(k[1], Some(ChartKind::ColumnClustered));
}

fn named(i: usize) -> String {
    format!(
        "<c:ser><c:idx val=\"{i}\"/><c:order val=\"{i}\"/><c:tx><c:v>s{i}</c:v></c:tx>\
         <c:val><c:numRef><c:f>Data!$A$1:$A$5</c:f></c:numRef></c:val></c:ser>"
    )
}

#[test]
fn series_keep_their_order_when_the_groups_are_written_out_of_order() {
    // The writer emits a combo chart's groups in drawing order (primary bars, primary lines,
    // secondary), so the file's group order is not the chart's series order. `c:order` is; reading
    // it back must keep every series-indexed command pointing at the same series after reopen.
    let bar = bar(&named(0));
    let lines = line(&(named(2) + &named(3)));
    let secondary = line(&named(1));
    let xml = format!(
        "<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\">\
         <c:chart><c:plotArea>{bar}{lines}{secondary}</c:plotArea></c:chart></c:chartSpace>"
    );
    let root = parse(xml.as_bytes()).unwrap();
    let ch = read_chart(&root).unwrap();
    let names: Vec<&str> = ch.series.iter().filter_map(|s| s.name.as_deref()).collect();
    assert_eq!(names, ["s0", "s1", "s2", "s3"], "series are read in c:order, not group order");
}

/// A combo whose groups are bars is written as horizontal bars (barDir="bar") and its secondary
/// value axis along the top of the plot, opposite the primary one at the bottom; a column combo
/// keeps the column orientation and the right-hand secondary axis. Both read back as they were
/// written.
#[test]
fn a_combo_of_bar_groups_is_written_as_horizontal_bars() {
    use gridcraft_model::{Chart, Series, Workbook};

    use crate::chart::write_chart;

    let combo = |kind: ChartKind, second: ChartKind| {
        let mut ch = Chart { id: 1, kind: ChartKind::Combo, ..Chart::default() };
        ch.series = vec![
            Series { values: "Sheet1!$B$2:$B$4".into(), kind: Some(kind), ..Series::default() },
            Series { values: "Sheet1!$C$2:$C$4".into(), kind: Some(second), secondary: true, ..Series::default() },
        ];
        write_chart(&Workbook::new(), 0, &ch, "Book1").xml
    };
    // The axis position of the axis with an id, and the bar direction of each group.
    let axes = |xml: &str| -> Vec<(String, String)> {
        let root = parse(xml.as_bytes()).unwrap();
        let plot = root.path(&["chart", "plotArea"]).unwrap();
        plot.children
            .iter()
            .filter(|n| matches!(n.name.as_str(), "catAx" | "valAx"))
            .filter_map(|n| Some((n.child_val("axId")?.trim().to_string(), n.child_val("axPos")?.to_string())))
            .collect()
    };
    let dirs = |xml: &str| -> Vec<String> {
        let root = parse(xml.as_bytes()).unwrap();
        let plot = root.path(&["chart", "plotArea"]).unwrap();
        plot.kids("barChart").filter_map(|g| g.child_val("barDir").map(str::to_string)).collect()
    };

    // Bars: both groups horizontal, the primary pair on the left/bottom, the secondary on the top.
    let xml = combo(ChartKind::BarClustered, ChartKind::BarClustered);
    assert_eq!(dirs(&xml), ["bar", "bar"], "both groups are horizontal bars: {xml}");
    assert_eq!(
        axes(&xml),
        [
            ("1".to_string(), "l".to_string()),
            ("2".to_string(), "b".to_string()),
            ("3".to_string(), "l".to_string()),
            ("4".to_string(), "t".to_string())
        ],
        "a bar chart's secondary value axis is the one at the top: {xml}"
    );
    let root = parse(xml.as_bytes()).unwrap();
    let ch = read_chart(&root).unwrap();
    assert_eq!(ch.kind, ChartKind::Combo);
    assert_eq!(ch.series.iter().map(|s| s.kind).collect::<Vec<_>>(), [Some(ChartKind::BarClustered), Some(ChartKind::BarClustered)]);
    assert_eq!(ch.series.iter().map(|s| s.secondary).collect::<Vec<_>>(), [false, true]);

    // Columns: unchanged — columns, and the secondary axis on the right.
    let xml = combo(ChartKind::ColumnClustered, ChartKind::LineMarkers);
    assert_eq!(dirs(&xml), ["col"], "the combo's one bar group is a column group: {xml}");
    assert_eq!(
        axes(&xml),
        [
            ("1".to_string(), "b".to_string()),
            ("2".to_string(), "l".to_string()),
            ("3".to_string(), "b".to_string()),
            ("4".to_string(), "r".to_string())
        ],
        "a column chart's secondary value axis is the one on the right: {xml}"
    );
}
