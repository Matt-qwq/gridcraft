# Chart parity

> **Last reviewed:** 2026-10-30 · **Last updated:** 2026-10-30 · **Change:** major (axis scaling, data-label options, error bars, per-point formatting modelled, round-tripped and drawn; unmodelled chart parts passed through) · **Target:** Microsoft Excel (Microsoft 365), Excel for Mac 16.113.4

Excel's chart types and chart elements against GridCraft. Part of
[target-app-parity.md](target-app-parity.md). **Breadth ~50%, ready ~30%, 40–60 h** (estimated).

Sources: `crates/model/src/features.rs` (`ChartKind`, `Chart`, `Series`, `AxisOptions`,
`LabelOptions`, `ErrorBars`, `PointFormat`), `crates/chart/src` (layout and rendering),
`crates/xlsx/src/chart.rs` (DrawingML chart parts), the option matrix below, merged PRs #62, #116,
#158 and #161, and issue #190.

## Chart types

| Excel type | Excel subtypes | GridCraft | Notes |
|---|---|---|---|
| Column | clustered, stacked, 100% stacked, 3-D clustered/stacked/100%, 3-D column | 3 of 7 | no 3-D |
| Bar | clustered, stacked, 100%, 3-D ×3 | 3 of 6 | no 3-D |
| Line | line, stacked, 100% stacked, with markers ×3, 3-D line | 3 of 7 (line, with markers, stacked) | no 100% stacked; smoothed lines landed (#62) |
| Pie | pie, 3-D pie, pie of pie, bar of pie, doughnut | 2 of 5 (pie, doughnut) | no exploded, pie of pie, bar of pie |
| Area | area, stacked, 100%, 3-D ×3 | 2 of 6 | no 100%, no 3-D |
| X Y (scatter) | markers, smooth lines (±markers), straight lines (±markers) | 2 of 5 | no smooth lines |
| Bubble | bubble, 3-D bubble | 1 of 2 | |
| Stock | high-low-close, open-high-low-close, volume-HLC, volume-OHLC | 1 kind | |
| Surface | 3-D surface, wireframe, contour, wireframe contour | 0 of 4 | |
| Radar | radar, with markers, filled | 1 of 3 | |
| Treemap, Sunburst | 1 each | 2 of 2 | |
| Histogram, Pareto | 2 | 1 of 2 | no Pareto |
| Box & Whisker | 1 | 1 of 1 | exclusive (default) and inclusive quartiles per series; XLSX round-trip; `chart.set {quartiles}` changes all series |
| Waterfall, Funnel | 2 | 2 of 2 | written as chartex parts (PR #161); kind survives a round-trip |
| Filled map | 1 | 0 | needs geographic boundary data (licensing decision) |
| Combo | custom combinations, secondary axis | yes (line on secondary axis) | |
| **Total** | ~60 subtypes | **25 (~42%)** | usage-weighted breadth higher (~65%): column, bar, line, pie and scatter dominate |

## Chart elements and formatting

| Element | Excel | GridCraft |
|---|---|---|
| Move and resize the chart object | yes | yes (corner handle, Format Chart Area Size fields) |
| Chart title, axis titles | text, linked to cells, full formatting | text only |
| Legend | position, overlay, formatting, per-entry delete | position only |
| Data labels | value, category, series, percentage, from cells, callouts, position | value, percentage, series, category and legend key; position, separator, number format; per series; per point shown/deleted. No from-cells or callouts |
| Gridlines | major/minor per axis, formatting | one on/off flag (major, value axis) |
| Axis options | min/max/units, log scale, reverse order, crossing point, display units, date axis, number format | min/max, major unit, log base, reverse order, crossing point, display units (nine built-ins) and number format; read, written and drawn. Date-axis units and secondary-axis scaling are missing |
| Secondary axis | any series | line series in combo charts; its scaling is not modelled |
| Multi-level category axis (a category range two or more columns wide beside the values: year beside month) | the outer levels grouped under the labels with separators | read (`multiLvlStrRef`), drawn (rows under the labels; columns beside them in bar charts; up to three outer levels), written back with Excel's level caches |
| Secondary axis | any series | line series in combo charts |
| Trendlines | linear, exponential, logarithmic, polynomial, power, moving average, equation and R² | all six types, several per series, name, forward/backward forecast, set intercept, equation and R² label, line colour/width/dash; read, written and drawn on 2-D column, bar, line, area, scatter and bubble series; `chart.addElement` `trendline` adds/removes them. No Format Trendline pane, label position/number format not kept |
| Error bars | fixed, %, standard deviation/error, custom | each series: fixed, percentage, standard deviation, standard error and custom (plus/minus references or array constants), both/plus/minus directions, end caps on/off; read, written and drawn on 2-D column/bar/line/area/scatter/bubble. Line colour/width is not modelled (drawn in the series colour) |
| Up/down bars, high-low lines, drop lines, series lines | yes | **missing** |
| Data table under the chart | yes | **missing** |
| Per-series / per-point fill, line, marker, effects | Format pane for every element | series colour; per-point fill, line and marker colour plus label deletion, read, written and drawn (pie, doughnut, treemap, sunburst, column, bar, line, scatter, bubble). No effects, pattern fills or a per-element Format pane |
| Chart styles and colour palettes | ~16 styles × colour sets | style number stored, own palettes |
| Chart sheets | yes | **kept, not editable**: not shown, and written back unchanged in their place on save (#727); left out when a sheet they plot is deleted or renamed. No F11 / Move Chart > New sheet |
| Chart templates (`.crtx`) | yes | **missing** |
| Recommended Charts | yes | dialog with recommendations from the data's shape (own heuristic) and an All Charts list, live previews (#215) |
| Switch Row/Column, Select Data | yes | yes (Switch on charts read from files landed in #116); Select Data dialog: data range, remove series, Horizontal (Category) Axis Labels (#287). No series add/edit or hidden/empty cell options |
| Sparklines (line, column, win/loss, markers, axis) | yes | line, column, win/loss |
| PivotCharts | yes | Insert ▸ PivotChart (`insert.pivotChart`): bound to a PivotTable's report, rebuilt on refresh and layout changes, `c:pivotSource` round trip; no field buttons, innermost category level only |
| Animation on data change | yes (Windows) | no |


## Option matrix

What GridCraft models, reads, writes and draws. "Survives" means open → save without editing →
reopen gives the same option (`crates/xlsx/src/tests/chart_options_roundtrip.rs`); "drawn" means
`crates/chart` uses it, so a saved option and the picture agree.

| Option | Fields | Read | Written | Drawn | Notes |
|---|---|---|---|---|---|
| Value-axis bounds | `min`, `max` | yes | `c:scaling/c:min`, `c:max` | yes | a bound on the wrong side, or min ≥ max, is ignored rather than drawn upside down |
| Value-axis major unit | `major_unit` | yes | `c:majorUnit` | yes | a data step, or powers of the base on a log axis; capped at 51 ticks |
| Value-axis log scale | `log_base` (2..=1000) | yes | `c:scaling/c:logBase` | yes | powers of the base become the ticks; non-positive points sit at the low end |
| Value-axis reverse | `reversed` | yes | `c:orientation maxMin` | yes | |
| Value-axis crossing | `crosses`: auto zero / min / max / value | yes | `c:crosses`, `c:crossesAt` | yes | moves the category axis line |
| Value-axis display units | `display_units` (hundreds … trillions) | yes | `c:dispUnits/c:builtInUnit` | yes | labels divided by the factor, the unit named beside the axis |
| Value-axis number format | `number_format` | yes | `c:numFmt sourceLinked="0"` | yes | `sourceLinked="1"` reads as automatic |
| Category-axis reverse | `x_axis.reversed` | yes | `c:orientation maxMin` | yes | mirrors the categories and their labels |
| Category-axis label interval | `x_axis.major_unit` | yes | `c:tickLblSkip` | yes (labels every n-th) | |
| Category-axis bounds | `x_axis.min`, `max` | yes | `c:scaling` | no | kept for a faithful round trip; cropping categories is not drawn |
| Category-axis crossing | `x_axis.crosses` | yes | `c:crosses`, `c:crossesAt` | scatter only | for a label axis Excel offers "crosses at category" too |
| Category-axis display units / log | — | read as none | not written | no | Excel does not offer these on a category axis |
| Data-label parts | `show_value`, `show_percentage`, `show_series`, `show_category`, `show_legend_key` | yes | `c:dLbls/c:show*` | yes | chart-level and per series; whichever flags are set are composed |
| Data-label position | `position` (auto, centre, inside/outside end, base, best fit, four sides) | yes | `c:dLblPos` | yes | `auto` keeps Excel's per-chart default |
| Data-label separator | `separator` | yes | `c:separator` | yes | default `, ` |
| Data-label number format | `number_format` | yes | `c:numFmt` | yes | else the series' own format |
| Series label visibility | `Series::labels.hidden` | yes (`c:dLbls/c:delete`) | yes | yes | one series hides its labels while the chart keeps theirs; a chart that labels only some series is read this way |
| Per-point label deletion | `Series::points[].no_label` | yes (`dLbls/c:dLbl` and `dPt/c:dLbl`) | `c:dLbls/c:dLbl/c:delete` | yes | written even when labels are off |
| Error bars: kind | fixed, percentage, standard deviation (sample, n−1), standard error, custom | yes | `c:errBars/c:errValType`, `c:val`, `c:plus`, `c:minus` | yes | custom refs and `{…}` constants are resolved like series values |
| Error bars: direction, caps | both / plus / minus, `no_end_cap` | yes | `c:errBarType`, `c:noEndCap` | yes | |
| Error bars: scope | per series | yes | on that series | yes | column, bar, line, area, scatter, bubble (as Excel allows) |
| Per-point fill / line / marker | `fill`, `line`, `marker` | yes | `c:dPt/c:spPr`, `c:marker` | yes | the right slice, bar or marker takes the colour; markers only on line/scatter/bubble/radar points, as the format stores them |
| Unmodelled chart XML | — | kept whole | written back byte for byte while the chart is untouched | — | an edited chart is written from the model and unmodelled XML (leader lines, manual layout, title run formatting, `extLst`, `printSettings`, 3-D) is dropped |

Not modelled, therefore lost from a chart that is edited (and kept only through the byte-for-byte
part above): secondary-axis scaling, date-axis units, minor units and gridlines, gridline and
tick-label formatting, error-bar line colour/width, label callouts/leader lines and from-cells
labels, per-point label text/position (deletion only), per-group label sets in a combo chart, rich
text, chart-area/plot-area fills, and chartex (`chartExN.xml`) options. A chart whose *kind* is not modelled at all (3-D, surface, pie of
pie, bar of pie, Pareto) is skipped on open: its drawing frame belongs to the drawing part, which
GridCraft rewrites.

The kept part also keeps its caches: cell edits do not refresh the `numCache`/`strCache` in a part
written back unchanged, so a viewer that trusts caches (Word, PowerPoint) can show older values
until the chart is edited or Excel recalculates. Excel recalculates chart data on open.

## XLSX round-trip

Charts are read from and written to DrawingML chart parts. The options in the matrix above are
modelled in both directions; everything else in a classic chart part is kept byte for byte while the
chart is untouched (the chart's raw part is stored and written back), and is lost only when the
chart is edited, because editing regenerates the part from the model. chartex parts
(`chartExN.xml`) are regenerated from the model: their hidden defined names are not kept, so they
cannot be passed through. Unmodelled chart *kinds* are still skipped on open.

**Measured (10-26).** Each of the 25 kinds created by `insert.chart`, saved, reopened and compared
field by field (`_gridcraft-bench/chart_rt`). Kind, title, legend, data labels, gridlines, axis
titles and every series' name, categories, values and colour survive. What still differs:

| Difference after save and reopen | Why | Status |
|---|---|---|
| a stock chart of 2 series came back as a line chart | the format stores stock only for 3–4 series, and the writer had degraded the rest silently | **fixed**: `insert.chart` and `chart.set` refuse a stock chart without 3 or 4 columns |
| histogram, waterfall, funnel, treemap, sunburst and box & whisker changed style 1 → 2 | they are chartex parts, which have no classic style number; reading one reports 2 | **fixed**: a chartex chart is created at style 2, so the first save changes nothing |
| `source` is `None` after reopening | files don't carry the selection a chart was made from; Switch Row/Column rebuilds it from the series | by design |
| a bubble chart's empty `bubble_sizes` reads back as its values | the writer fills the required bubble size from the values when no size column was given | by design (same picture) |
| a combo chart's series carry no explicit `kind` until they are read back | the reader records the group each series was written as (column, then line) | by design (same picture) |
| a histogram reads back without categories | a histogram bins its values; it has no category axis | by design |

Series data, categories and names may refer to defined names, as dynamic (OFFSET) ranges do:
Excel writes a workbook-scoped name as `[0]!Name` in `<c:f>`. GridCraft reads it as the bare
name and writes it back with `[0]!`; a sheet-scoped name is written `Sheet!Name`. Excel needs the
qualifier: with a bare name in `<c:f>` it draws the whole chart blank, without a repair prompt
(observed in Excel for Windows, Microsoft 365). External books (`[1]Sheet1!…`) are kept as they
are (#1686). Such series get no `numCache`/`strCache` on save (#1690).

## Work (40–60 h)

1. Chart resizing (#190): 1–2 h.
2. Preserve unmodelled chart XML on round-trip when the chart *is* edited (patch only edited
   fields instead of regenerating the part): 6–10 h. The unedited case landed 10-30.
3. Remaining axis and label work: category cropping, date-axis units, minor units/gridlines,
   gridline and tick-label formatting, secondary-axis scaling, callouts/from-cells labels: 6–9 h.
4. Error-bar line formatting; Format Trendline pane: 2–4 h.
5. Per-element Format pane parity, smoothed lines, exploded pie, pie/bar of pie, 100% variants: 8–12 h.
6. 3-D column/bar/pie/area, surface and contour, Pareto: 8–12 h.
7. Chart sheets, templates and PivotChart field buttons: 5–8 h. Filled map: owner decision on boundary data.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-30 | major | Axis scaling (bounds, major unit, log, reverse, crossing, display units, number format), data-label options (five parts, position, separator, format, per series and per point), error bars (fixed, %, standard deviation/error, custom; both directions; caps) and per-point fill/line/marker model, round-trip and draw; `chart.setAxis`, `chart.setDataLabels`, `chart.setErrorBars`, `chart.formatPoint`; unedited chart parts are written back byte for byte so unmodelled XML survives; option matrix added |
| 2026-10-26 | minor | Round-trip measured across 25 kinds: a stock chart without 3–4 series is refused instead of saving as a line chart, chartex kinds no longer change style 1 → 2 on the first save; the rest of the diffs are representation or file-format limits, tabulated |
| 2026-10-18 | trivial | Stale notes: stock/combo, treemap/sunburst, box and waterfall kinds survive XLSX round-trip (PRs #158, #161) |
| 2026-10-16 | trivial | Series that refer to defined names (`[0]!Name`, `Sheet!Name`) resolve instead of showing `#NAME?`, and are saved qualified, as Excel needs (#1686) |
| 2026-10-16 | minor | Box & whisker quartiles default to exclusive, honor each series on draw and XLSX round-trip; `chart.set {quartiles}` changes the method (#1453) |
| 2026-10-13 | minor | Multi-level category axes: read, drawn (outer levels under or beside the labels), written back with level caches |
| 2026-10-12 | trivial | chart sheets are kept on save, still not editable (#727) |
| 2026-10-11 | minor | Trendlines: all six types modelled, round-tripped through `c:trendline`, drawn with forecast and equation/R² labels; `chart.addElement` `trendline` |
| 2026-10-11 | minor | PivotCharts (`insert.pivotChart`) |
| 2026-10-11 | minor | Recommended Charts dialog with previews (#215) |
| 2026-10-11 | minor | Select Data dialog with category axis labels; a blank top-left cell makes the first column the categories (#287) |
| 2026-10-10 | minor | Smooth lines (#62) and Switch Row/Column on file charts (#116) landed; chart resize bug (#190) |
| 2026-10-10 | major | Created from a source audit: 25 kinds (25 of ~60 subtypes), element gaps, round-trip loss |
