# Chart parity

> **Last reviewed:** 2026-10-31 · **Last updated:** 2026-10-31 · **Change:** major (edits are patched into the kept chart part, so unmodelled XML survives an edit; unmodelled chart *kinds* keep their frame and part; secondary-axis options modelled and drawn; category-axis bounds and crossing drawn) · **Target:** Microsoft Excel (Microsoft 365), Excel for Mac 16.113.4

Excel's chart types and chart elements against GridCraft. Part of
[target-app-parity.md](target-app-parity.md). **Breadth ~50%, ready ~40%, 25–40 h** (estimated).

Sources: `crates/model/src/features.rs` (`ChartKind`, `Chart`, `Series`, `AxisOptions`,
`LabelOptions`, `ErrorBars`, `PointFormat`), `crates/chart/src` (layout and rendering),
`crates/xlsx/src/chart.rs` (DrawingML chart parts) and `crates/xlsx/src/chart_patch.rs` (patching an
edit into a kept part), the option matrix below, merged PRs #62, #116,
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
| Axis options | min/max/units, log scale, reverse order, crossing point, display units, date axis, number format | min/max, major unit, log base, reverse order, crossing point, display units (nine built-ins) and number format; read, written and drawn on the value axis, the category axis and the secondary value axis. Date-axis units are kept, not editable |
| Secondary axis | any series | line series in combo charts; its own bounds, unit, log scale, reverse, crossing, display units and number format are modelled (`y2_axis`; `chart.setAxis` `axis: "y2"`), written and drawn |
| Multi-level category axis (a category range two or more columns wide beside the values: year beside month) | the outer levels grouped under the labels with separators | read (`multiLvlStrRef`), drawn (rows under the labels; columns beside them in bar charts; up to three outer levels), written back with Excel's level caches |
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
| Category-axis bounds | `x_axis.min`, `max` | yes | `c:scaling` | yes | bounds are 1-based category numbers: they crop the shown categories and extend the axis with empty slots past the data (up to 2000 extra); `min ≥ max` is ignored |
| Category-axis crossing | `x_axis.crosses` | yes | `c:crosses`, `c:crossesAt` | yes | a `crossesAt` category puts the value-axis line at that category (Excel's "Crosses at category"), on column and bar charts too; a scatter chart moves its value axis along the numeric X axis |
| Secondary value axis | `y2_axis` (all the value-axis rows above) | yes | the second group's `c:valAx` | yes | labels drawn on the right, its display unit named beside them. Only a chart with a series on the secondary axis has one (`chart.setAxis` refuses otherwise) |
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
| Unmodelled chart XML | — | kept whole | patched: only the elements an edit touched are rewritten (`crates/xlsx/src/chart_patch.rs`) | — | leader lines, manual layout, title run formatting, date-axis units, minor units and gridline formatting, `extLst`, `printSettings`, `dTable` and error-bar line styles survive an edit as well as an open → save |
| Unmodelled chart kinds | 3-D column/bar/line/area/pie, surface, pie of pie, bar of pie, Pareto, filled map | kept whole (`Sheet::kept_charts`) | the part byte for byte, with its drawing frame | no (the frame is kept, the plot is not drawn) | rebuilding one from the 2-D model would silently lose it; opening warns |

Everything else in a classic chart part survives a save, edited or not. What an edit still costs:
a changed chart title, legend, series name, category or value reference, trendline, error-bar
amount, or a re-coloured series rewrites that one element from the model, so formatting stored
*inside* it (rich title runs, legend entry layout, a trendline label's formatting, an error bar's
line style) goes with it. A change the part cannot express — a different chart kind, a different set
of series — regenerates the whole part, which is where unmodelled XML is still lost. Per-point label
text and position, prefix/suffix, marker size and per-point label formatting are not modelled: they
are kept while their element is untouched and are lost when that point's format is edited. chartex
(`chartExN.xml`) options are not modelled at all: a chartex chart is regenerated from the model, so
its options are lost on save unless the chart is of a kind GridCraft cannot model, which is kept
whole (see `KeptChart`).

A part is patched only when it is a classic chart whose root declares the `c:` prefix, whose series
still match the model one for one, and that is under 4 MB (a hostile or huge part is regenerated
instead). A chart whose groups carry no `axId` (rare) is regenerated when an axis option changes.

A kept part also keeps its caches: cell edits do not refresh the `numCache`/`strCache` in a part
written back unchanged or patched, so a viewer that trusts caches (Word, PowerPoint) can show older
values until the chart is edited or Excel recalculates. Excel recalculates chart data on open. A
chart of an unmodelled *kind* keeps its caches the same way: if the sheet's data changed, they are
stale in the saved file, and Excel refreshes them when it opens the workbook.

## XLSX round-trip

Charts are read from and written to DrawingML chart parts. The options in the matrix above are
modelled in both directions. A classic chart part read from a file is kept (`Chart::part`) and
written back from the file: byte for byte while the chart is untouched, and by patching only the
elements an edit changed when it is edited (`crates/xlsx/src/chart_patch.rs`; tests in
`crates/xlsx/src/tests/chart_options_roundtrip.rs`). chartex parts (`chartExN.xml`) are regenerated
from the model, because their hidden defined names are not kept. A chart of a kind GridCraft cannot
model is not read into the model at all: its part and drawing frame are kept whole
(`Sheet::kept_charts`, `crates/xlsx/src/tests/chart_unmodelled_kinds.rs`).

| Difference after save and reopen (kept parts) | Why | Status |
|---|---|---|
| a chartex chart's options | the part is regenerated; its hidden `_xlchart.v1.N` names are not kept | by design |
| a secondary value axis left automatic reads back as `crosses: max` | that is Excel's own crossing for a secondary value axis (it keeps the hidden secondary category axis on the right), and it is what a combo chart created here writes | by design (same picture) |

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
2. ~~Preserve unmodelled chart XML when the chart *is* edited~~ (landed 10-31: field-level
   patching, unmodelled kinds kept whole, secondary-axis options, category bounds and crossing).
3. Remaining axis and label work: date-axis units as an option, minor units and gridlines,
   gridline and tick-label formatting, callouts and from-cells labels: 4–6 h.
4. Error-bar line formatting; Format Trendline pane: 2–4 h.
5. Per-element Format pane parity, smoothed lines, exploded pie, pie/bar of pie, 100% variants: 8–12 h.
6. 3-D column/bar/pie/area, surface and contour, Pareto: 8–12 h.
7. Chart sheets, templates and PivotChart field buttons: 5–8 h. Filled map: owner decision on boundary data.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-31 | major | Field-level pass-through: an edit is patched into the kept chart part (`crates/xlsx/src/chart_patch.rs`), so manual layouts, date-axis units, minor units and gridline formatting, label callouts and leader lines, error-bar line styles, title run formatting, `dTable`, `printSettings` and `extLst` survive an edit as well as an open → save; only the changed element is rewritten, and turning labels off keeps the label element with Excel's delete flag. Charts of unmodelled kinds (3-D, surface, pie of pie, bar of pie, Pareto, filled map) keep their frame and their part (`Sheet::kept_charts`) instead of being dropped. Secondary value axis modelled (`Chart::y2_axis`, `chart.setAxis` `axis: "y2"`), written and drawn; category-axis bounds crop/extend the drawn axis and a category crossing is drawn, so `chart.setAxis` accepts both |
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
