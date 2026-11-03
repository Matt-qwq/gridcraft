# Target-app parity: GridCraft vs Microsoft Excel

> **Last reviewed:** 2026-11-02 · **Last updated:** 2026-11-02 · **Change:** `.xls` reads pictures (OfficeArt blip store, `pib`, client anchors and Excel's placement bits); before it, `.xls` reads colour scales, data bars and icon sets (`CondFmt12`/`CF12`, settled against Excel 16-authored files); before it, Excel 16 is activated and the oracle verifies every fixture in it; the number-group encoding it found is fixed; PivotTable label and value filters work (items hidden by caption or aggregate); PivotTable number grouping works (half-open buckets, `rangePr` in the cache, save/reopen); `GETPIVOTDATA` works (the evaluator reads the report's own labels, in every layout, and a refresh recalculates the formula; functions 514 / 523); gate #1 recorded as owner-blocked, with `cargo xtask oracle` (openpyxl + WPS) as the accepted non-Excel substitute — it found a PivotTable part with an undeclared `r:` prefix, now fixed and guarded; minor (alternative text for charts and shapes with a checker that navigates to each issue; a PivotTable filter records its area, so a page filter on an axis field keeps its row; a data label's own font is modelled at three levels, a click on a chart element selects it, the chart point's own marker size is settable, and custom error-bar references are editable in the pane; gate #4 met, no score change because the chart row already counted these) · **Target:** Microsoft Excel (Microsoft 365)

This is the authoritative parity assessment. [`ROADMAP.md`](../ROADMAP.md) summarizes it,
[`gaps.md`](gaps.md) lists every shortfall, and the area checklists hold the detail:
[functions](function-parity.md), [charts](chart-parity.md), [file formats](file-format-parity.md),
[UI](ui-parity.md), [hardware](hardware-parity.md), [localization](localization-parity.md). The
generated ribbon checklist is [`parity-checklist.md`](parity-checklist.md).

## Summary

| Number | Value | Kind |
|---|---|---|
| Feature breadth (weighted by use) | **~78%** | estimated from the area table below; anchored on two measurements |
| Ribbon and menu catalog | **280 / 295 (94.9%)** | measured: `cargo xtask parity` (`crates/engine/src/catalog.rs` ∩ registered command ids) |
| Worksheet functions, by name | **513 / 523 (98.1%)** | measured: registered functions vs Microsoft's published function list (method below) |
| Worksheet functions that do real work | **504 / 523 (96.4%)** | measured: excludes 9 stubs (7 CUBE functions, WEBSERVICE, FILTERXML) |
| **Ready for real work** | **~55%** | estimated; written weights below (54.8 computed, re-measured 2026-10-30) |
| **Mainstream practitioner** | **~56%** | estimated; [method](#mainstream-practitioner) (73.6 depth × 0.92 × 0.92 × 0.88 = 50.6) |
| **Essentials user** | **~67%** | estimated; [method](#essentials-user) (82.0 depth × 0.92 × 0.96 × 0.92 = 66.7) |
| Stage | **alpha** | see [Stage](#stage) |
| Remaining to beta | **~150–240 Opus 5.5 agent-hours** | estimated; [roadmap.md](roadmap.md#beta-gates); large-workbook gate met, base polish in wave 0 |
| Remaining to full parity | **~700–1,100 Opus 5.5 agent-hours** | estimated; sum of the rows below |

### Readiness by audience

| Audience | Ready % | Opus 5.5 agent wall-clock hours to ~95% | Work that dominates |
|---|---|---|---|
| Full target (ready for real work) | ~55% | ~600–980 | Get & Transform and the data model, localization (twelve languages), charts depth, file fidelity (`.xls`, corpus, pass-through), PivotTables, collaboration, mobile |
| Mainstream practitioner | ~56% | ~180–285 | File exchange with Excel users (corpus and Excel verification, pass-through, large files, `.xls`, encryption: 63–97 h), common-chart depth (30–45 h), PivotTables (20–30 h), keyboard and interaction (18–28 h), calc correctness and functions (20–33 h) |
| Essentials user | ~67% | ~45–80 | Opening files people send (`.xls` import and saving with a password 24–36 h; macOS Finder open 2–4 h), launch stability (6–10 h), basic chart resize and polish (6–10 h), print and Page Layout (8–12 h), small editing and UI fixes (10–16 h) |

Each tier is a subset of the one above, so its hours are too. Calibrated as in [Effort and
calibration](#effort-and-calibration) (~6–10 h per readiness point for the remaining, verification-heavy
work; single features measured from PRs #66, #100, #101). Parallelism: the full and mainstream
work splits ~65–70% across 4–6 agents by area; the essentials work is mostly small fixes plus one
long `.xls` track, so about half parallelizes (≈30–50 wall-clock hours with three agents).

## What we measured against, and how

- **Target:** Microsoft Excel (Microsoft 365 channel). Version measured: **Excel for Mac
  16.113.4**, installed on the owner's Mac (version read from Spotlight metadata with `mdls`).
  Where Excel for Windows has more (Power Pivot, VBA editor depth, COM add-ins, 3D Maps), the
  Windows feature counts as part of the target.
- **Clean-room limits.** This repo's rule (AGENTS.md, "Non-negotiables") forbids reading anything
  inside the Office bundle, so the bundle's `Info.plist`, `.lproj` folders and resources were
  deliberately **not** inspected, and Excel was not launched for this pass. Excel's file formats,
  functions and features come from Microsoft's public documentation, the owner's earlier
  black-box observation (`plan/excel/observed-ui.md`, local only), and knowledge of the product.
- **Function list:** Microsoft's "Excel functions (alphabetical)" support page, fetched
  2026-10-10 and parsed (506 rows), plus 15 names the page lists under combined entries (FIND/FINDB,
  LEFT/LEFTB, LEN/LENB, MID/MIDB, REPLACE/REPLACEB, RIGHT/RIGHTB, SEARCH/SEARCHB, BETA.INV) and
  COPILOT and PY: **523 functions**. Ours: every `f!(…)` spec in `crates/functions/src/*.rs` plus
  `SPECIAL_FUNCTIONS` in `crates/calc/src/eval.rs` (evaluator special forms), minus the test-only
  `ADD2`: **505**, all of them Excel names.
- **Ribbon catalog:** `crates/engine/src/catalog.rs` (290 unique ids across 15 tabs) against the
  313 command ids registered in `crates/engine/src/cmd/*.rs`, recomputed from source by script and
  identical to the committed `parity-checklist.md`. The catalog is our own transcription of
  Excel's main ribbon tabs; it leaves out most contextual tabs (Picture Format, Shape Format,
  Header & Footer, Sparkline, Slicer, Timeline), the Developer and Help tabs and sub-menu items, so
  89% of it is roughly **60–65% of Excel's full command surface** (estimated).
- **Depth and readiness:** source reading (the model types in `crates/model/src/features.rs`,
  readers and writers in `crates/xlsx/src`, the evaluator), 478 `#[test]` functions, and the
  issue tracker: 60 open issues and 50 open PRs on 2026-10-10, 82 issues filed in total.
- No cargo build or test ran for this pass (disk constraints); numbers come from source.

## Weights

Percentages are weighted by what Excel users actually do, not by item count. Feature-area
weights (sum 100) reflect a typical business, finance and analyst workload: formulas, functions,
editing and formatting dominate; Power Query, VBA and the data model matter to fewer users but
are decisive for those who use them.

Dimension weights for **ready for real work**: features 45, file formats 15, UI/UX 12,
performance 8, stability 8, localization 4, hardware 2, platforms 2, ecosystem 2, AI 2.

## Feature areas

| Area | Weight | Breadth | Ready | Hours | Evidence |
|---|---|---|---|---|---|
| Formula language and calculation | 18 | ~93% | ~78% | 16–24 | A1/R1C1 parser, reference adjustment, dependency-graph recalc, dynamic arrays and `#SPILL!`, `A1#`, LET/LAMBDA and helpers, structured references, names, 3-D references. The five correctness reports (#57 15-digit equality, #133/#139 stale spills, #140 3-D span endpoint deletion, #134 sheet rename and qualified names) are fixed, and recalc shards independent formulas across the machine's threads (100k-dependent edit 20–25 ms). A 595-example, 275-function **documented-value** oracle suite runs through the real calc path (`crates/calc/tests/formula_oracle.rs`, 10-30); no Excel-computed corpus yet, so it pins documented behaviour rather than installed-Excel parity (gap #6) |
| Worksheet functions | 12 | 98.3 (measured) | ~86% | 12–20 | 514 / 523 names, 505 working. Missing: IMAGE plus cloud functions. A documentation-derived oracle suite covers 595 documented examples over 275 functions (10-30); no Excel-computed corpus, because Excel 16 is activated now: the oracle opens/refreshes/re-saves every fixture in it (11-02), and only the corpus half of gate #1 is missing. [function-parity.md](function-parity.md) |
| Editing, navigation, clipboard, fill | 12 | ~92% | ~73% | 14–23 | Point mode, F4, AutoComplete, Paste Special, AutoFill, Flash Fill, Find/Replace, Go To Special. Whole-ribbon key tips landed 10-29 (417 sequences, `Shift+F10` context menus). Bugs: rectangular drag-select (#28), monthly fill (#132), cut across sheets (#128), Insert Copied Cells missing (#21). [ui-parity.md](ui-parity.md) |
| Cell formatting, styles, conditional formatting | 10 | ~95% | ~80% | 8–14 | Full number-format language, borders, 47 cell styles, 60 table styles, all CF rule kinds including data bars, colour scales and icon sets. Gaps: font weights beyond regular/bold (#168), system fonts list (#4), themes |
| Sort, filter, tables, validation, outline | 8 | ~82% | ~70% | 10–16 | Multi-level sort, AutoFilter, Advanced Filter, tables with totals, validation, outline, subtotals, consolidate. Advanced Filter copy-to preserves cells outside its result and rejects worksheet overflow (#1031). Bugs: subtotal header (#131), validation edits (#129), table style panic caught (#130); no slicers on tables |
| PivotTables | 7 | ~70% | ~56% | 20–32 | Insert/Recommended, field list, three layouts, values on rows or columns, date grouping, show-values-as, refresh, calculated fields, XLSX round-trip.**Slicers and timelines landed 10-30:** read into the model (cache field, filtered PivotTables, item values with their selected tiles, caption, style, column count, timeline unit, anchor), written once from the model and driven by `insert.slicer`, `insert.timeline`, `slicer.setSelection/setRange/setStyle/move/delete`, and a selection filters the pivot through its own refresh path in one undo step. A written widget is **placed**: an anchored drawing frame naming the view, one slicers/timelines part per sheet listed in the worksheet, the caches in the workbook, and table slicers written with `x15:tableSlicerCache`; the widgets are **drawn and driven by real input**, a table slicer follows a table/column rename, a report-side filter mirrors onto its slicer, and **a timeline's range handles drag** (either end resizes, the band slides, one undo step). PivotCharts (`insert.pivotChart`) have been in since 10-11. A filter now records its **area** (`PivotFilterArea`), so a page filter on a field that is also on an axis keeps its row above the table. **`GETPIVOTDATA` landed 11-02**: it reads the report's own labels — case-insensitively, numbers by value or shown text, `Grand Total` on either axis, no pairs = the grand total — in every layout, with Σ Values in Rows, subtotals on top or off, and page filters, and the formula depends on the report's written area so a refresh recalculates it. **Number grouping landed 11-02**: a number field groups into half-open buckets from `pivot.group` start/end/interval (whole steps read `0-9`, `<start`/`>end` hold what falls outside), and the cache writes `rangePr startNum`/`endNum`/`groupInterval` with `groupBy="range"`. **Label and value filters landed 11-02**: `pivot.filterByLabel` and `pivot.filterByValue` keep the items whose caption or aggregate matches (twelve and eight operators), evaluated when set and stored as the field's hidden items so report, reopen, refresh and Excel agree. **Calculated items landed 11-02** (`pivot.calculatedItem`: an item of a row or column field computed from that field's own items, sorted last and out of the grand totals, stored in the model and written in Excel's own encoding — the caption as the field's last shared item with `f="1"`, `<item f="1" x>` and `<calculatedItems>` on the cache with the `Field[item]` formula). **Verified in Excel 16 (11-02)**: a grouped-number pivot with a value filter, a written slicer and `Region[East]+Region[West]` opens, refreshes and re-saves with every cell agreeing (localized chrome excepted); Excel keeps the calculated item and recomputes 116,320, and our reader reads Excel's own copy back. Missing: data-model (OLAP) pivots, the "Generate GetPivotData" click option, and a real-world corpus |
| Charts and sparklines | 8 | ~52% | ~55% | 28–42 | 25 chart kinds are created and edited; seven more (3-D bar/line/area/pie, surface, pie of pie/bar of pie, Pareto, filled map) are **kept but not drawn** rather than dropped. Options round-trip, draw and are commands (axis scaling, display units, data labels, error bars, per-point fill/line/marker, trendlines) and an **edit patches the kept part**, so unmodelled chart XML survives instead of being regenerated away; the secondary axis is modelled, written and drawn, and `chart.setSeriesAxis` (plus the Format pane checkbox) moves a series onto it, with combo series read back in `c:order` so an index survives save/reopen. The **Format pane** drives those options per element (chart area, either axis, each series; per-field Auto), including a point's fill, line, marker and marker size and a custom error bar's referenced ranges, and charts, pictures, shapes and slicers **resize from eight handles** (Shift keeps the ratio, Esc cancels, one undo step, the size survives save/reopen). A click on a bar, slice, marker, legend entry or axis label **selects that element** in the pane (the first click selects the chart), because the renderer returns its click regions with its primitives, and a data label's **own font** (size, bold, colour) is modelled at chart, series and point level and set from the pane. Missing: creating the unmodelled kinds, chartex `styleN`/`colorsN` (only the chartEx part is kept), a label's typeface/italic/fill/callout, chart sheets. [chart-parity.md](chart-parity.md) |
| Page layout, print, PDF | 5 | ~86% | ~62% | 10–17 | Page setup, print titles, breaks, Page Break Preview, headers/footers, PDF export and printing. Paper sizes match Japanese Excel: B4/B5/B6 are the JIS sizes, ISO B4/B5/B6 stay reachable by name, and Japanese Postcard (はがき), Japanese Double Postcard (往復はがき), the chou/kaku envelopes and A6 are available. PDF export embeds a subset TrueType face, so Latin Ext, Greek, Cyrillic, Hebrew, Arabic, Devanagari, Thai, CJK and Hangul print (shaping and bidi are still missing). No true Page Layout view; pictures in headers/footers and Different First Page/Odd & Even remain; web print only exports (#141, #143) |
| What-if and analysis | 4 | ~50% | ~40% | 22–35 | Goal Seek, Scenario Manager, data tables, Analysis ToolPak (14 of 19 tools, no charts; the z-Test landed 11-02), Solver (Simplex LP, a local nonlinear method and Evolutionary; Answer and Limits reports; no Sensitivity report or options dialog). Missing: Forecast Sheet, Analyze Data |
| Get & Transform, external data, data model | 5 | ~15% | ~10% | 60–100 | Import of CSV/TSV, JSON, HTML. No Power Query editor, no From Web, database or folder sources, no connections or refresh, no relationships or data model, no external workbook links (dropped with a warning) |
| Insert objects | 3 | ~55% | ~45% | 25–40 | Pictures, 8 shape kinds (Excel has ~170), text boxes, own icon set, ink with Ink to Shape, hyperlinks, checkboxes. Missing: SmartArt, WordArt, equations, signature line, objects, 3-D models, screenshot, pictures in cells (PR #105) |
| Review, comments, protection, collaboration | 4 | ~55% | ~44% | 32–50 | Threaded comments, notes, spelling (system dictionary), thesaurus, sheet and workbook protection, Allow Edit Ranges with Excel-compatible password hashes (#307). The **accessibility check** lists charts, pictures and shapes without alt text (set from the dialog or `object.setAltText`, written as `xdr:cNvPr/@descr`), tables without a header row, merged cells and default sheet names, and goes to each. Missing: comment navigation, Show Changes, notes-to-comments conversion, an accessible grid tree for screen readers, co-authoring. Password-encrypted workbooks **open** (Agile and Standard are read) and Agile is **written** back (#11, #7) |
| Automation | 4 | ~30% | ~20% | 12–20 | Recorded, editable GridCraft scripts (Automate tab). VBA is out of scope by owner decision and is **dropped** when an `.xlsm` is opened (warning shown). No Office Scripts compatibility |
| **Weighted** | 100 | **~78%** | **~64.2%** | **269–433** | |

## Dimensions

| Dimension | Weight | Ready | Hours | Evidence |
|---|---|---|---|---|
| Features | 45 | ~64.2% | 269–433 | Table above |
| File formats | 15 | ~54% | 65–98 | XLSX read/write is broad and large files now open (100M cells with cold paging); output is proven in **Excel 16** on five fixtures ([oracle.md](oracle.md)) but not yet on a real corpus (gate #1); a foreign reader (openpyxl) and a foreign application (WPS) read it back intact and already forced one real fix — PivotTable parts written without the `xmlns:r` declaration; `.xls` reads values, formulas, styles, comments, validation, conditional formats (including CF12 colour scales, data bars and icon sets), hyperlinks and panes; XLSB and ODS are values-only import; no XLS writing, XML Spreadsheet 2003, SYLK, DIF, PRN; VBA dropped. [file-format-parity.md](file-format-parity.md) |
| UI/UX fidelity | 12 | ~58% | 45–72 | Excel-style ribbon, galleries, dialogs, formula bar, status bar; 59 commands with shortcuts vs Excel's 200+; key tips on part of the Home tab only; short context menu; single window. [ui-parity.md](ui-parity.md) |
| Performance | 8 | ~35% (conservative; large-workbook gate met 10-28, re-scored at beta exit) | 20–35 | [Performance](#performance) |
| Stability | 8 | ~62% | 13–22 | Never-crash rules enforced by lint, every command fuzzed with hostile params, random command sequences fuzzed against undo/redo, XLSX round-trip and full-recalc invariants, last-resort guard in `Session::execute`. Field reports: Windows Intel GPU crash (#120), error on close (#117), freeze opening from Finder (#166), screen flashing (#76) |
| Localization | 4 | ~15% | 100–165 | Ribbon-only catalog in five languages (ja, zh, ko, pt-BR, ru; ~12% of strings each), CJK/Thai/Hebrew font fallback, no shaping or RTL. [localization-parity.md](localization-parity.md) |
| Hardware | 2 | ~40% | 25–40 | GPU UI via wgpu; recalc uses up to 8 threads for independent formulas; no pen pressure; one window. [hardware-parity.md](hardware-parity.md) |
| Platforms | 2 | ~80% | 30–60 | [Platforms](#platforms) |
| Ecosystem / add-ins | 2 | ~10% | 25–40 | [Ecosystem and AI](#ecosystem-and-ai) |
| AI features | 2 | ~15% | 30–50 | [Ecosystem and AI](#ecosystem-and-ai) |
| **Ready for real work** | 100 | **~55%** (54.8 computed: 0.45 × 64.2 features + the other rows × their weights) | **~627–1,020** | |

How the full number is built: features ready is the weighted mean of the area table (64.0 now);
ready for real work is the weighted mean of this table with the weights written under
[Weights](#weights). **Re-measured 2026-10-30**, the first re-score since 10-11: charts rose from
30% to 55% (options round-trip and are command-driven, an edit patches the kept part instead of
regenerating it, unmodelled kinds are kept, the secondary axis is modelled, and the Format pane
plus eight-handle resize landed), PivotTables from 45% to 56% (slicers and timelines modelled,
written from the model and command-driven), formula language from 72% to 78% (the five correctness
reports closed, recalc is threaded, a documented-value oracle suite runs), functions from 82% to
86% (that suite covers 275 of them), editing from 70% to 73% (key tips cover the whole ribbon),
PDF/print from 55% to 58% (font subsetting exports every script) and review from 40% to 44%
(encryption is written and its integrity verified); file formats, UI/UX and stability each moved
two points on the same evidence. The old 51.6 was 51.6 before and after the 10-10 correction; this
one is a real move.

## Mainstream practitioner

The typical professional Excel user: an analyst, accountant or manager who builds and maintains
models, cleans and summarizes data, makes charts and pivots, and exchanges workbooks with Excel
users every week. Left out: Power Query and the data model, VBA and add-ins, Copilot and cloud
functions, co-authoring and admin, specialist hardware, and languages beyond the user's own
(localization is reported separately).

| Area (weekly use) | Weight | Depth | Basis |
|---|---|---|---|
| Formula language and calculation | 20 | 78 | Feature-area table |
| Common worksheet functions | 15 | 88 | Missing functions are niche (ETS, odd-period bonds, cloud); edge bugs in common ones (#137, #138, #57) |
| Editing, navigation, clipboard, fill | 15 | 73 | Feature-area table |
| Cell formatting, styles, conditional formatting | 13 | 80 | Feature-area table |
| Sort, filter, tables, validation | 10 | 70 | Feature-area table |
| PivotTables (core: fields, layouts, grouping, refresh) | 8 | 64 | Field list, layouts, grouping and refresh; slicers are model- and command-driven but not drawn (10-30) |
| Charts (column, bar, line, pie, scatter, combo) | 9 | 62 | Common kinds work, survive save, are option-editable from the Format pane and resize from eight handles; unmodelled kinds are kept but not drawn |
| Page layout, print, PDF | 5 | 63 | Latin text exports |
| Comments, notes, protection | 3 | 58 | Feature-area table |
| Pictures and shapes | 2 | 45 | Feature-area table |
| **Average depth** | 100 | **73.6** | |

Discounts for what still stops real work:

| Discount | Factor | Evidence |
|---|---|---|
| Interaction fidelity | × 0.92 | 59 command shortcuts vs Excel's 200+; rectangular selection bug (#28); charts can't be resized (#190); one window (#162); missing horizontal scrollbar report (#176); no extend mode or pane cycling |
| Stability on real machines | × 0.92 | Never-crash lints, fuzzed commands and the `Session::execute` guard; but field reports of a startup crash on Intel UHD (#120), an error on close (#117), a freeze opening from Finder (#166) and screen flashing (#76) |
| File exchange with Excel users | × 0.90 | Excel opening our output is unverified across features (gate #1, blocked on Excel itself: the dev machine's install is unlicensed, so only a foreign reader and WPS stand in); the data model/Power Pivot parts are still dropped on save; chartex options and chart details are regenerated; `.xls` opens without charts/shapes and not for BIFF5 (validation, conditional formats including colour scales/data bars/icon sets, hyperlinks, panes and pictures are read), and writes as `.xlsx`; Standard/RC4 encryption is read but not written. Large files now open (100M cells with cold paging), unmodelled package parts are kept, and encrypted packages are written, but the corpus/repair proof is gate #1. Harsher than a drawing app's discount because spreadsheets are exchanged constantly |

**Mainstream practitioner: 73.6 × 0.92 × 0.92 × 0.90 = 56.1 → ~56%** (estimated, re-measured 2026-10-30). Unlike most
apps it is not higher than the full number: GridCraft's breadth is wide (functions, formatting,
ribbon), so excluding the long tail removes little, while the discounts land on exactly what
mainstream users do every week (exchange files with Excel users).

## Essentials user

Someone who only touches the core: opens a workbook someone sent, types data and simple formulas,
formats, sorts, makes a basic chart, saves and prints. Advanced options, pro workflows and exchange
edge cases are left out, along with everything excluded from the mainstream number.

| Core feature | Weight | Depth | Basis |
|---|---|---|---|
| Typing data, simple formulas, AutoSum (SUM, AVERAGE, arithmetic) | 20 | 89 | Solid; 15-digit equality (#57) is rare at this level |
| Basic formatting: fonts, bold, fills, borders, currency/percent/date formats | 15 | 85 | |
| Copy, paste, fill handle, undo/redo | 15 | 78 | Selection bug (#28), monthly fill (#132) |
| Rows and columns: insert, delete, widths, AutoFit | 10 | 82 | AutoFit fixed (#42, #88); rows auto-fit oversized fonts on open and row formatting (#833) |
| Sort and filter | 10 | 82 | |
| Save, Save As, reopen XLSX | 10 | 85 | |
| A basic chart | 8 | 70 | Created easily; can't be resized (#190) |
| Print and PDF | 7 | 72 | Desktop works; web PDF gives no file or feedback (#141) |
| Find and replace | 5 | 85 | |
| **Average depth** | 100 | **82.0** | |

| Discount | Factor | Evidence |
|---|---|---|
| Launch and stability | × 0.92 | Startup crash on some Intel GPUs (#120), "Issue executing" (#59), error on close (#117) |
| Discoverability and UI clarity | × 0.96 | The Excel-style ribbon is familiar and labelled; system language by default (#31); small gaps (#176 scrollbar, #170 macOS window controls) |
| Opening files people send them | × 0.92 | Double-clicking a workbook in Finder freezes or errors on macOS (#166, #69; drag-and-drop works; fixes in PRs #164, #167); BIFF8 `.xls` files open (charts and pictures are dropped) and encrypted `.xlsx` opens with its password, while BIFF5 and RC4-encrypted `.xls` are refused with a message |

**Essentials user: 82.0 × 0.92 × 0.96 × 0.92 = 66.7 → ~67%** (re-measured 2026-10-30) (estimated).

## User evidence

Counted from all 83 GitHub issues and 109 PRs on 2026-10-10; none of the issues were filed by
the maintainer.

- **Praise:** 3 explicit ("this is brilliant. Tiny fast native binaries", #16; "Thanks for the
  great work on GridCraft", #61; "i didnt have excel kind app on my macos", #69), plus polite
  thanks in feature requests (#39, #45, #142).
- **"Switched from Excel" reports:** 0. One counter-signal: "without macro support most
  professionals cannot switch from excel" (#6).
- **Issues by kind:** 50 core-path bugs (30 open, 20 closed) and 33 niche or feature requests
  (25 open, 8 closed). 18 of the core-path bugs came from one auditing contributor (coygeek).
- **Contributors:** 104 PRs from 28 people outside the maintainer in five days, which shows
  strong interest but not yet daily use.

## Performance

Excel recalculates on every core (multi-threaded recalculation), streams large files, and handles
1,048,576 × 16,384 sheets with millions of formulas.

| Measure | GridCraft | Excel | Kind |
|---|---|---|---|
| Sheet size limits | 1,048,576 × 16,384 | same | measured (`crates/core/src/addr.rs`) |
| Recalc threads | up to 8 for independent formulas (native; wasm single-threaded) | all cores | measured (`crates/calc`): leaves wave by wave; arrays, volatile formulas and cycles stay sequential |
| Edit → recalc, 100k dependents | **20–25 ms** (2026-10-27); target < 100 ms met | tens of ms | measured on the owner's machine, `_gridcraft-bench/formula_bench`, release: 65–75 ms before threading, 245 ms before the round; 1M dependents are 208–277 ms in `calcbench`; a full recalc of the 100k sheet is 89–114 ms |
| Grid scrolling | virtualised, O(log n) geometry with custom row heights | smooth | measured in code |
| Open a large workbook | 100M-cell / 551 MB export: **16.7 MiB live, 183 MiB peak**, 729 MiB compressed pages; 31.7M-cell / 118 MB report: 49.7 MiB live, 1.47 s | opens | measured 2026-10-28, `session_breakdown` / `cell_digest` |
| Row / column structural insert | 1M×20: row **3.7 ms** command / 35.8 ms commit; column **9.8 ms** / 42.5 ms; zero cells moved | immediate | measured 2026-10-28, `gridcraft-cli` |
| Line metadata, 1M custom heights | `Sheet::clone` **0.13 ms**, insert/delete **0.013/0.007 ms**, 10k inherited lines 0.1 ms; `LineIndex` build 6.6 ms (cached per revision) | immediate | measured 2026-10-29, `_gridcraft-bench/linebench` |
| Mapped value write-back | 1M values: identity 36–42 ms; after a row insert **35 ms** (was 131–135), after a column insert **33 ms** (was 116–142) | immediate | measured 2026-10-29, `_gridcraft-bench` release probe |
| Packed walk / owned gate | Dense block **2.5–3.3 ns/cell** on identity or a mapped column (was 30.6); owned/formula gate over 1M paged rows **0.84 ms** (was 9.5) | fine | measured 2026-10-29, `_gridcraft-bench` release probe |
| Save a large workbook | 31.7M cells: **3.48 s, 1,283.9 MiB peak**; 100M-cell export: **28.42 s, 1,814.3 MiB peak** | should stay flat | measured 2026-10-29, `session_breakdown` release with counting allocator; per-sheet deflate, bytes identical |
| Whole-sheet operations | Bounded: Remove Duplicates on 1M×10 falls from 91 s to 12 s, Sort 3.2 s, Fill Down of 1M rows 1.45 s; sort/remove duplicates/setValues cap at 12M cells (1M×20 refused in 1.3 s) | fine | measured on `max_1048576.xlsx` / `wide_1m_x20.xlsx`, `gridcraft-cli` |

Work: the deferred page-file rewrite and post-beta streaming save, then M17 zone maps and the M21
Parquet read half after beta. The
old streaming/threading/whole-column items are landed; keep the measured workbooks and
`cargo xtask bench` as the regression net.

## Platforms

| Platform | Excel | GridCraft |
|---|---|---|
| Windows x64 / arm64 | yes | yes (signed MSI and portable zip; also x86) |
| macOS (Apple silicon, Intel) | yes | yes (signed, notarized universal DMG) |
| Web | Excel for the web | yes (WASM; local files, no cloud) |
| iOS / iPadOS / Android | yes | no |
| Linux | no | yes (AppImage, Flatpak, deb, rpm, tarball; x86_64, aarch64, riscv64) |
| FreeBSD | no | yes |

Mobile is the gap (30–60 h for a touch layout over the same engine). GridCraft is ahead on
Linux, BSD and RISC-V, where Excel doesn't run.

## Ecosystem and AI

- **Add-ins:** Excel has Office JavaScript add-ins, COM/XLL add-ins, VBA, Office Scripts and
  Power Automate. GridCraft has its own recorded scripts, plus a control channel, MCP server and
  CLI that reach every command. VBA is out of scope (owner decision). A plugin or custom-function
  API (for example LAMBDA libraries and WASM custom functions) is the open-equivalent path: 25–40 h.
- **AI:** Excel has Copilot, the COPILOT() function, Analyze Data, Python in Excel and Flash
  Fill. GridCraft has Flash Fill and best-in-class agent control (any MCP client can drive every
  command headless or live), but no in-app assistant, no natural-language formulas and no
  Analyze Data. Local-model equivalents: 30–50 h; model choice needs the owner.

## Stage

**Alpha.** All six core workflows in the [alpha gate](roadmap.md#alpha-gate) pass end to end on macOS with save and reopen (two pass with non-blocking fidelity loss). Core workflows exist end to end, but depth, fidelity and file compatibility are rough
(ready ~55%, inside the 40–75% alpha band). The large-workbook gate is met: 100M cells open with
16.7 MiB live, 100k-dependent edits are 20–25 ms, and row/column structural maps are in. Not beta:
XLSX has not been proven on real-world files or confirmed to open in Excel without repair, `.xls`
is missing, charts/text/Pivot/keyboard gates remain, and the M20/M19 base still needs compaction
wiring, spill placement and flat-memory save. Distance to beta: ~25 points of readiness and
~150–240 h, itemized in [roadmap.md](roadmap.md#beta-gates).

## Effort and calibration

Hours are **Opus 5.5 agent wall-clock hours**, one agent working sequentially, including tests
and verification.

Calibrated against this repo's history:

- **First pass, 2026-10-05 → 10-07:** M0–M13 (about 72,000 lines of Rust: engine, 478 functions,
  XLSX, UI, charts, PivotTables, PDF, MCP, CLI, web, packaging) landed in commits from 22:09 on
  10-05 to 02:08 on 10-06, built by several parallel agents with work that preceded the first
  commits; we put it at **~50–80 agent-hours** for 0 → ~50% ready, so **~1–1.5 h per point**
  for the broad first pass.
- **Recent single features:** ODS import (#100, +2,232 lines with tests) and XLSB import (#101,
  +1,212 lines) each fit in one agent session of roughly 2–3 h; the linear-recalc fix (#66,
  +477 lines) about 1 h; single-function fixes (ROUND, DATE 1900, SUMIF sizing) 0.3–0.5 h each.
  That is roughly **600–1,000 lines per agent-hour** including tests.
- **Tail factor:** what remains is verification-heavy (Excel as the oracle for files, functions
  and rendering), and much of it is large self-contained subsystems (Power Query, Solver,
  localization, `.xls`). We apply **5–7× the first-pass rate**: ~6–10 h per point of readiness.

**Parallelism:** ~70% of the work splits cleanly by area (functions, charts, pivots, formats,
localization, Power Query) across 4–6 agents with one integrator; with five agents, beta is about
40–60 wall-clock hours away. **Needs a human:** checking our files in Excel (the owner's Mac),
native-speaker review of every language, scope decisions (co-authoring, cloud functions,
in-app AI models, mobile), and Windows hardware for GPU-driver crashes.

## What works today

The inventory carried over from the 2026-10-07 ROADMAP.md, updated for what landed since. Presence, not parity: the tables above grade depth.

- Engine: A1/R1C1 formula parser with reference adjustment, dependency-graph recalculation with
  dynamic arrays (spill + `#SPILL!`), cycle detection, LET/LAMBDA/MAP/REDUCE/SCAN/BYROW/BYCOL/
  MAKEARRAY, structured table references, defined names, INDIRECT/OFFSET/INDEX references,
  505 worksheet functions (478 at M0) (math, statistics and distributions, text and regex, lookup incl.
  XLOOKUP/FILTER/SORT/UNIQUE/VSTACK, dates, financial incl. bonds, engineering incl. complex
  numbers and Bessel, database, information).
- Number formats: Excel's full format-code language (sections, conditions, colours, dates and
  elapsed time, fractions, scientific, accounting fill), General narrowing to column width.
- Files: XLSX read/write (styles, themes, merges, CF, validation, tables, comments, hyperlinks,
  charts, pictures, sparklines, print settings, defined names, shared/array/dynamic formulas),
  CSV/TSV with encoding and delimiter detection, HTML export, JSON; XLSB and ODS data import (values only, 10-10).
- Editing: cell entry with type detection and auto-formats, in-cell and formula-bar editing,
  point mode (click/drag/arrow references), reference colouring, F4 anchor cycling, function
  autocomplete and argument hints, copy/cut/paste and Paste Special (values, formats, transpose,
  operations, link, skip blanks), AutoFill series (numbers, dates, months, weekdays, custom lists,
  text+number), Fill Series, Flash Fill, find/replace with wildcards, Go To / Go To Special,
  undo/redo with history, format painter. Quick selection drags retain their final endpoint;
  the fill handle explains how to fill versus select cells.
- Formatting: fonts, fills, all border styles, alignment (wrap, indent, rotation, merge), number
  formats, 47 cell styles, 60 table styles, conditional formatting (cell rules, text, dates,
  duplicates, top/bottom, averages, data bars, colour scales, icon sets, formulas), automatic
  row heights, autofit, hide/unhide, freeze panes.
  Border presets include visual diagrams; Format Cells previews draft borders before applying.
- Data: sort (multi-level, by colour, custom lists), AutoFilter (values, custom, top 10,
  average, colour), tables with totals rows, remove duplicates, text to columns, data validation
  with error alerts, grouping/outline, subtotals.
- PivotTables: Insert/Recommended PivotTables, field list pane, compact/outline/tabular layouts,
  values on rows or columns, date grouping, sort, filters, show-values-as, refresh, and XLSX round-trip.
- Page layout & print: Page Break Preview, print settings, PDF export and printing.
- What-if: Goal Seek, scenarios, data tables. Spelling (system word list) and thesaurus.
- Draw: pen, highlighter, eraser, ink strokes and Ink to Shape. Task panes: Comments (threaded
  replies, resolve), Watch Window, Selection, Format Chart.
- Charts: column, bar, line, area, pie, doughnut, scatter, bubble, radar, histogram, waterfall,
  funnel, treemap, sunburst, box & whisker, stock and combo charts; sparklines.
- UI (egui): Excel-style title bar with Quick Access Toolbar and AutoSave, the full ribbon with
  galleries (cell styles, table styles, colour palettes, conditional formatting menus),
  contextual Table/Chart Design tabs, formula bar with Name Box, virtualised grid (smooth
  scrolling, zoom, frozen panes, text overflow), sheet tabs (rename, reorder, colour, hide),
  status bar with Sum/Count/Average and zoom, dialogs (Format Cells, Formula Builder, Find and
  Replace, Sort, Name Manager, Data Validation, Paste Special…), command palette, dark mode.
- Agents: every action is an engine command; JSON control channel in the desktop app (pointer,
  keyboard, dialogs, screenshots); MCP server (headless or bridged); CLI (`info`, `convert`,
  `eval`, `cat`, `run`, `commands`, `functions`, `mcp`, `send`).
- Platforms: macOS, Windows, Linux, FreeBSD; web (WASM via trunk). Release workflows for
  signed/notarized macOS universal DMG, signed Windows x64/x86 MSI + zip, Linux AppImage/deb/rpm/
  tar.gz + Flatpak manifest, FreeBSD tarball and a web zip.

## Not measured yet

- Pixel comparison against Excel screenshots (planned; screenshots stay in `plan/excel/`).
- Function results against Excel-computed oracles (a corpus repo is the planned home).
- Recalc and open-time benchmarks on fixed workbooks (no `cargo xtask bench` yet).
- Whether Excel opens our XLSX output without repair, across features.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-02 | trivial | Analysis ToolPak: the z-Test (two sample for means) landed with its report layout and dialog fields (14 of 19 tools). No score change yet — the tool row keeps its depth estimate until the two-factor ANOVAs and the sampling tools land |
| 2026-11-02 | trivial | `.xls` pictures landed: OfficeArt drawings and the workbook's blip store are read into `Image`s with their cell/offset anchors (Excel's placement bits mapped), and a picture round-trips through the `.xlsx` writer with its media. Charts, shapes, controls and vector pictures are skipped with one warning. No score change (the file-formats depth estimate already counted pictures) |
| 2026-11-02 | trivial | `.xls` conditional formats of the CF12 kinds landed: colour scales, data bars and icon sets are read (`CondFmt12` + `CF12`, `CFGradient`/`CFDatabar`/`CFMultistate`, thresholds as `CfValueKind`, colours as `Color`, the 17 ECMA icon-set names), settled against three Excel 16-authored files (which showed Excel's percentile-as-type-7 threshold and its inverted data-bar show-value bit) and compared with the `.xlsx` copies Excel wrote. File formats' depth estimate falls by the CF12 hours (68–102 → 65–98); the percentage stays 54 (a rounding step, not a readiness point) |
| 2026-11-02 | trivial | PivotTable calculated items landed (`pivot.calculatedItem`: a formula over the field's own items, sorted last, out of the grand totals, written in Excel's `f="1"`/`<calculatedItems>` encoding and read back). Verified in Excel 16 — the oracle pivot keeps the definition across open/refresh, recomputes 116,320 and re-saves with every cell equal. No score change: the pivot row keeps its depth estimate until the real-world corpus run (gate #1) and data-model pivots, and the "~70% / ~56% / 20–32" numbers already counted calculated items as missing depth |
| 2026-11-02 | trivial | Gate #1's Excel half is evidence: the owner activated Excel 16 and `cargo xtask oracle` opens/refreshes/re-saves every fixture in it (all five agree; openpyxl readback and WPS remain). No score change yet — the file-exchange row waits for the corpus, and the pivot row for calculated items |
| 2026-11-02 | trivial | PivotTable label and value filters landed (`pivot.filterByLabel`/`pivot.filterByValue`; evaluated when set and stored as the field's hidden items, so the visible report is consistent across reopen, refresh and Excel). No score change — the pivot row keeps its depth estimate until calculated items land |
| 2026-11-02 | trivial | PivotTable number grouping landed (`PivotNumberGroup`, `pivot.group` start/end/interval, half-open buckets, `<start`/`>end` items, `rangePr startNum/endNum/groupInterval groupBy="range"` written and read back). No score change — the pivot row keeps its depth estimate until calculated items and value/label filters land |
| 2026-11-02 | trivial | `GETPIVOTDATA` landed (worksheet functions 514 / 523 names, 505 working; PivotTables lose it from the missing list): it reads the report's own label cells in every layout, matches numbers by value or shown text, names either grand total, depends on the report's written area so a refresh recalculates it, and is in the Insert Function list. No score change — the pivot row keeps its depth estimate until the remaining items land |
| 2026-11-02 | trivial | Gate #1 is owner-blocked (Excel unlicensed) and its non-Excel half now runs through [oracle.md](oracle.md): a foreign reader and a foreign application accept our packages and found a PivotTable part with an undeclared `r:` prefix. No score change — the file-exchange discount already assumed the repair proof is missing |
| 2026-11-02 | trivial | Alt text for charts and shapes with `object.setAltText`, and an accessibility check that covers them (plus tables without headers) and navigates to each issue; a PivotTable filter records its area, so a page filter on an axis field keeps its row; a data label's own font (size, bold, colour) is modelled at the chart, series and point level and set from the pane; a chart element is selected by clicking it; per-point marker size is settable; custom error-bar references are editable in the pane; gate #4 met, no score change |
| 2026-11-02 | trivial | Timeline range handles drag on the sheet (resize either end or slide the band, days snap, one undo step); no score change because the row already counted the modelled range |
| 2026-10-30 | trivial | Corrected the PivotTables row: the widget UI, an editable timeline range and PivotCharts (`insert.pivotChart`, 10-11) are landed, so the remaining list is the timeline handles' drag, calculated items, value/label filters, grouping, GETPIVOTDATA and the page-filter row |
| 2026-10-30 | minor | `.xls` gained validation, conditional formats, hyperlinks and frozen panes (matched to the `.xlsx` reader, spec-derived only); file formats 52% → 54%, hours 70–105 → 68–102 |
| 2026-10-30 | minor | Text is shaped on **both** paths: the grid and in-cell editor draw bidi runs in visual order with a visual caret, and PDF export shapes with cluster-exact `ToUnicode`. Page/print/PDF ready 58% → 62%, localization 10% → 15%, feature breadth 77% → 78%, ready for real work 54.6% → 54.8%; arithmetic recomputed from the tables |
| 2026-10-30 | major | Consolidated re-score after the 10-29/30 wave: ready for real work 51.6 → 54.6 (charts 30 → 55, PivotTables 45 → 56, formula 72 → 78, functions 82 → 86, editing 70 → 73, PDF 55 → 58, review 40 → 44; file formats, UI/UX and stability +2 each), mainstream practitioner 50.6 → 56.1, essentials 63.1 → 66.7, feature breadth 75 → 77, catalog 280/295. Arithmetics recomputed from the tables, not adjusted by hand |
| 2026-10-29 | minor | Large-workbook beta gate met: 100M cells 16.7 MiB live / 183 MiB peak with cold paging, 100k edit 20–25 ms, 1M rows, row and column structural maps; the performance readiness score is left conservative until M20/M19 polish closes and beta re-scores |
| 2026-10-28 | minor | Whole-sheet operations: bulk edits scan blocked arrays once for the union of changed cells; Remove Duplicates on 1M×10 is 12 s (was 91), Sort 3.2 s, Fill Down 1M rows 1.45 s, and sort/remove duplicates/setValues cap at 12M cells (1M×20 refused in 1.3 s) |
| 2026-10-27 | minor | Recalculation performance: independent formulas now run on up to eight threads (leaves wave by wave); the 100k-dependent edit is 20–25 ms end to end (was 65–75 single-threaded, 245 before the round) and 1M is 208–277 ms — the recalculation-threads and `< 100 ms` rows are updated |
| 2026-10-27 | minor | Recalculation performance: the 100k-dependent edit is 65–75 ms (was 245) and a full recalc 134–147 ms (was 281–289), single-threaded — the `< 100 ms` target in the performance table is met |
| 2026-10-21 | minor | Page Setup paper sizes: `B4`/`B5`/`B6` are the JIS sizes Japanese Excel uses (ISO B4/B5/B6 by name), and Japanese Postcard, Japanese Double Postcard, the chou/kaku envelopes and A6 are added; PDF MediaBox and `file.print` follow, and the Size menu lists them |
| 2026-10-18 | trivial | Stale chart notes: all chart kinds survive XLSX round-trip (PRs #158, #161) and trendlines landed |
| 2026-10-16 | minor | Advanced Filter copy-to clears only its actual output and rejects overflow without changing cells (#1031) |
| 2026-10-15 | minor | Row auto-fit: line breaks count only in wrapping cells (Excel: wrap off shows one line, AutoFit gives one line); unwrapped multi-line cells from files no longer grow rows on open (no score change) |
| 2026-10-12 | minor | Essentials AutoFit evidence: rows auto-fit oversized fonts on open and row formatting (#833) |
| 2026-10-12 | minor | Sheet tabs: reveal the selected tab on navigation and resize while preserving manual scrolling (#681); no score change |
| 2026-10-12 | minor | PivotTables: values on rows (Σ Values in Rows at any position, all three layouts; `pivot.moveField` and the field pane move it; `dataOnRows` round-trips) (no score change) |
| 2026-10-11 | minor | Stability: command-sequence fuzz harness (`crates/engine/tests/command_sequence_fuzz.rs`) checks random command sequences against undo/redo, XLSX round-trip, full-recalc and structure invariants; the sequence bugs it found are fixed (no score change) |
| 2026-10-11 | minor | Password-encrypted workbooks open (#11): Essentials file-opening work 24–36 h |
| 2026-10-11 | minor | Worksheet functions 505 / 523 (96.6%; 496 working): ODDFPRICE, ODDFYIELD, ODDLPRICE, ODDLYIELD |
| 2026-10-11 | minor | Worksheet functions 503 / 523 (96.2%; 494 working): PERCENTOF, EUROCONVERT |
| 2026-10-11 | minor | Worksheet functions 502 / 523 (96.0%; 493 working): BAHTTEXT |
| 2026-10-11 | minor | Worksheet functions 502 / 523 (96.0%; 493 working): TRIMRANGE |
| 2026-10-11 | minor | View › Formula Bar and Zoom 100% are engine commands (CLI/MCP); catalog 258 → 260 / 290 (#289) |
| 2026-10-11 | minor | Review › Allow Edit Ranges; catalog 258 → 259 / 290 (#307) |
| 2026-10-11 | minor | PivotTables: calculated fields (`pivot.calculatedField`) |
| 2026-10-11 | minor | PivotTables: PivotCharts (`insert.pivotChart`) |
| 2026-10-10 | minor | Readiness-by-audience table: hours to ~95% for each of the three numbers (full 650–1,050, mainstream 200–310, essentials 60–100). Full number confirmed as the additive weighted sum over the written dimension weights (no change) |
| 2026-10-10 | minor | Added Mainstream practitioner (~50%) and Essentials user (~63%) with written weights and discounts, and user-evidence counts; re-checked the full number after the 10-10 merges (charts 38→30, localization 5→10; still 51.6, ~50%) |
| 2026-10-10 | minor | Stage checked against the new alpha gate (six core workflows, all pass): stays alpha |
| 2026-10-10 | major | Created. Full re-measure against Excel for Mac 16.113.4: functions 501 / 523 measured against Microsoft's list, catalog 258 / 290, breadth ~75%, ready ~50%, alpha; replaces the estimate tables previously in ROADMAP.md and keeps its "Working today" inventory |
