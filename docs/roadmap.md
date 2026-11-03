# Roadmap detail

> **Last reviewed:** 2026-11-02 · **Last updated:** 2026-11-02 · **Change:** major (Excel 16 is activated; the oracle opens/refreshes/re-saves every fixture in it and all agree, finding and fixing a number-group encoding bug; PivotTable label and value filters keep items by caption or by aggregate, stored as hidden items; PivotTable number grouping — `pivot.group` start/end/interval, half-open buckets, `rangePr` in the cache; GETPIVOTDATA reads a report cell by its field items; high contrast from View ▸ Show; a screen reader can walk the sheet — the visible cells are an AccessKit grid; alternative text for charts and shapes with a checker that navigates to each issue; a PivotTable filter records its area, so a page filter on an axis field keeps its row; a data label's own font — size, bold, colour — is modelled at the chart, series and point level; a click on a chart element selects it; a chart point carries its own marker size; custom error-bar references are editable in the pane; a timeline's range handles drag on the sheet; a chart series moves to the secondary axis; **gate #4 met and Excel activated**, so only the corpus (3–6 h) and gates #7 7–16 h and #8 1–3 h remain; beta ~70–124 h) · **Target:** Microsoft Excel (Microsoft 365)

Forward-looking plan. The one-page summary is [`ROADMAP.md`](../ROADMAP.md); the ranked work
list is [`gaps.md`](gaps.md); numbers and method are in
[`target-app-parity.md`](target-app-parity.md). Estimates are Opus 5.5 agent-hours.

## Current focus (2026-10-29)

**Stage: alpha, with the large-workbook beta gate met.** The 100M-cell export opens with 16.7 MiB
live and 183 MiB peak (M19), row and column inserts/deletes change logical→physical maps instead of
moving cells (M20), independent formulas recalculate on the machine's threads, and bulk data
commands are bounded. What remains is a sequenced push to beta: finish the M20/M19 base, prove XLSX
in Excel, then close the fidelity, format and interaction gates. The old open-PR queue is no longer
the plan; if PRs are still open when picking up a gap, land/rebase them into that gap's task rather
than scheduling a separate review wave.

### Execution queue

Ordered; each wave can overlap with the next where noted. Estimates are Opus 5.5 agent-hours.

| Order | Work | Gap | Hours | Depends / notes |
|---|---|---|---|---|
| **0** | **Close the base**: page-file rewrite (deferred); save streaming (S4c) is post-beta | M19 follow-up | **0–8** | S1–S4b landed 10-29; wave 0's base is closed. Start wave 1 (XLSX corpus/Excel) next. |
| **0b** | Stability soak and the field crash/freeze list | #10 | 8–12 | Parallel with 0; Windows GPU machines for the driver cases. |
| **1** | **XLSX corpus + Excel acceptance**: run a real-world corpus; Excel opens our output without repair | #1 | 3–6 + owner | **Split 11-02.** The corpus repo run and the foreign-reader/application oracle (`cargo xtask oracle`, [oracle.md](oracle.md)) need no Excel and are 3–6 h; the Excel acceptance itself is the owner's half day and cannot be dropped. #3's pass-through half landed 10-29. |
| **1b** | Calculation correctness: 15-digit equality, stale spills, 3-D/name edge cases, iterative calculation | #6 | 2–5 | **met 10-30**: the five reports are closed, iterative calculation is in, and a 595-example / 275-function documented oracle suite runs through the real calc path. What remains is re-running it against an Excel-computed corpus, which belongs to #1 |
| **2** | Charts: options round-trip, axes, labels, error bars, per-point formatting, resize | #4 | 0 | **Closed 11-02.** A1 landed 10-29 (model, round-trip, render, four commands), A7 10-29 (an edit patches the kept part; unmodelled kinds kept whole; secondary axis modelled, written, drawn), the Format pane and eight-handle resize 10-30, and on 11-02 per-point marker size, click-to-select of any drawn element and a label's own font at the chart, series and point level. Chartex granularity (its label font, per-point options) and a pane per shape/picture remain, and neither is part of this gate. |
| **2b** | All-script text: Arabic/Devanagari/Thai shaping, bidi | #5 | 0–3 | **Shaping engine landed 10-30** (`core::text`: unicode-bidi levels + rustybuzz, visual-order runs, logical clusters, 26 tests). What remains is wiring it into the grid and the PDF writer — two tasks in flight. |
| **2c** | Keyboard: key tips, shortcut audit, context menus | #9 | 0–6 | **met 10-29**: 417 key-tip sequences, keyboard context menus, six shortcuts; leftovers are the Menu key (upstream) and menus for non-cell objects |
| **3** | `.xls` import and password-protected workbooks | #7 | 4–11 | **Encryption write (Agile and Standard, with a verified integrity field and an OS CSPRNG) and `.xls` styles, comments, validation, conditional formats (including CF12 colour scales, data bars and icon sets), hyperlinks and frozen panes landed 10-29/30/11-02**; charts/pictures, BIFF5 (not in [MS-XLS]) and RC4 writing remain. |
| **3b** | PivotTables: verified in Excel | #8 | 0–1 | **Slicers and timelines landed 10-30** (model, XLSX, six commands, drawn on the sheet, a selection filters the pivot, a timeline range filters through `slicer.setRange`), **the timeline's range handles drag (11-02)** — both ends and the band itself, one undo step, axis cached per revision. **GETPIVOTDATA reads a report cell by its field items** (every layout, Σ Values in Rows, subtotals, page filters, grand totals; a refresh recalculates it). **Number grouping landed 11-02** (`PivotNumberGroup`: `pivot.group` start/end/interval, half-open buckets with `<start`/`>end` items, `rangePr`/`groupBy="range"` in the cache, save/reopen). **Label and value filters landed 11-02** (`pivot.filterByLabel`/`pivot.filterByValue`, stored as the field's hidden items so report, reopen, refresh and Excel agree). **Calculated items landed 11-02** (`pivot.calculatedItem`: a formula over the field's own items, written as Excel does — the field's last shared item with `f="1"`, `<item f="1" x>` and `<calculatedItems>` on the cache — and verified in Excel 16, which keeps the definition and recomputes the item). What is left is the real-world corpus for #1; data-model pivots are full scope. |
| **4** | **Beta exit**: stability soak, all gates green, full `cargo xtask ci`, docs, release, corpus re-run | all | 8–12 | After 0–3b. |
| **Post-beta** | M21 Parquet read half and linked tables (20–30 h) · M17 zone maps (8–12 h) · localization, twelve languages (100–165 h) · Get & Transform/data model (60–100 h) · full chart depth, collaboration, mobile, AI · M22 only if a measured trigger fires | M17/M21/M22 | — | Not beta blockers. Keep the 100M/paging base stable; no native format before a trigger. |

**Parallel plan.** With 4–6 agents: A owns wave 0 (M20/M19 base); B owns wave 1 (XLSX); C owns
charts (wave 2); D owns text + keyboard (2b/2c); E owns `.xls`/encryption (3); the integrator owns
stability, the beta gate table, and the release. The owner's Excel/corpus half-day is the critical
external dependency; schedule it before wave 1 finishes.

## Alpha gate

The core workflows a typical Excel user runs every day, checked end to end on the main platform
(macOS desktop), including saving and reopening the work. Any "no", or a "partial" that blocks the
workflow, would make GridCraft pre-alpha.

| Core workflow | Works end to end? | Evidence | Hours to pass |
|---|---|---|---|
| Build a model: enter data and formulas, recalc, save XLSX, reopen | yes | 492 working functions, dynamic arrays, LET/LAMBDA, names, tables; XLSX round-trip tests (`crates/xlsx/src/tests/roundtrip.rs`). Edge-case bugs (#57 15-digit equality, stale spills #133/#139) give wrong results in specific cases, not in the workflow | 0 (fixes are beta work, gap #6) |
| Open a colleague's XLSX, edit, save, hand it back | yes for the workflow | Typical workbooks open and save with cells, styles, CF, validation, tables, names and pivots; large files now open (100M cells at 16.7 MiB live with cold paging); unmodelled parts are still dropped with a warning (gap #3) and Excel-side repair prompts are unverified (gap #1). These are beta gates, not alpha blockers | 0 for alpha (gaps #1–#3 are beta gates) |
| Format and present: number formats, styles, conditional formatting, print or PDF | yes | Full number-format language, 47 cell styles, all CF kinds, Page Break Preview, PDF export and printing. PDF can't carry non-Latin text (gap #5) | 0 |
| Analyze: sort, filter, tables, PivotTable | yes | Multi-level sort, AutoFilter, tables with totals, PivotTables with field list, layouts, grouping, refresh and XLSX round-trip. Calculated fields done; slicers missing (gap #8) | 0 |
| Chart the data | partial, not blocking | Column, bar, line, pie, area, scatter, combo, histogram, box, waterfall, funnel, treemap, sunburst and stock charts are created, edited and survive save and reopen; trendlines are drawn; no axis options (gap #4) | 0 for alpha |
| Import and export CSV | yes | Encoding and delimiter detection on read, CSV/TSV write; exports raw rather than displayed values (gap #19) | 0 |

**Passes:** every core workflow works end to end on macOS with save and reopen; the two partial
rows lose fidelity only on less common content (unmodelled XLSX parts, advanced chart options),
which are beta gates, not alpha blockers. Large files now open with cold paging. Ready for real
work ~50% is inside the 40–75% alpha band.

## Beta gates

Beta means: ~75% ready for real work, opens and saves XLSX reliably, remaining gaps are known bugs
and edge features. Each gate links its gap.

| Gate | Gap | Status (2026-10-29) | Hours left |
|---|---|---|---|
| XLSX proven on a real-world corpus; Excel opens our output without repair | #1 | **Excel half met 11-02** (the owner activated Excel 16): `cargo xtask oracle` opens every fixture in **Excel**, refreshes every PivotTable, recalculates, saves and re-reads the copy, comparing every cell/format/merge/table/CF/DV/chart count and name — all five fixtures pass in Excel and WPS with no repair and no loss (only Excel's localized PivotTable chrome differs). The package validator still refuses any undeclared namespace prefix. **What is left: the pinned real-world corpus** (3–6 h; no URL in the repo, no checkout, no network here) | 3–6 |
| Unmodelled XLSX parts preserved on save | #3 | **met 10-29** (slicers, timelines, table query tables, unknown `extLst`, strict package validator); data model and chart `extLst` remain, Excel check folded into #1 | 0–4 |
| 100 MB workbooks open; 1M rows usable; multi-threaded recalc; 100k-edit recalc < 100 ms | #2 | **met 10-28** (M19/M20); M20/M19 polish in wave 0 | 0 for the gate; 20–30 polish |
| Charts: kinds and options survive round trip; axes, trendlines, error bars | #4 | **met 11-02**: axis scaling, data labels (including a label's own size, weight and colour at the chart, series and point level), error bars and per-point formatting modelled, round-tripped, drawn and command-driven; an edit patches the kept chart part so unmodelled XML (manual layout, callouts, `dTable`, date-axis units, `extLst`) survives; unmodelled kinds are kept whole; secondary axis modelled, written and drawn, with combo series read back in `c:order`; clicking a bar, slice, marker, legend entry or axis label selects it in the Format pane. What is left is chartex granularity (its label font and per-point options stay with the kept part) and a pane per shape/picture, neither of which the gate asks for | 0 |
| Text in every script renders and exports | #5 | **met 10-30, with a font-coverage caveat**: `gridcraft_core::text` shapes every run (bidi levels, Arabic joining, Devanagari reordering, Thai marks) and **both paths use it** — the grid draws visual-order runs with a visual caret and shaped AutoFit, PDF places glyphs with cluster-exact `ToUnicode` so `pdftotext` returns logical order — while WinAnsi text stays byte-identical. What remains is coverage, not shaping: CJK and Arabic in one document, per-platform font distribution, per-run fallback, wrapped cells, text boxes, chart labels and mirrored printing | 0–3 |
| Calculation correctness bugs closed; iterative calculation | #6 | **met 10-30, with the oracle caveat**: the five correctness reports are closed, iterative calculation landed, and `crates/calc/tests/formula_oracle.rs` pins 595 documented examples over 275 functions through the real calc path — **documentation-derived, not Excel-computed**, so an Excel corpus run stays part of #1 | 2–5 |
| `.xls` import, password-protected workbooks | #7 | **partial 11-02**: **Agile and Standard** encryption are both written (Agile with OS-CSPRNG keys and a verified integrity HMAC; Standard as the [MS-OFFCRYPTO] 2.3.2 binary header + AES-128-ECB with no integrity field, which is why damage shows up downstream), the scheme is the caller's choice and is remembered per document; `.xls` reads styles, comments, validation, conditional formats (including **CF12** colour scales, data bars and icon sets, settled against Excel 16-authored files), hyperlinks and frozen panes. Charts/pictures, RC4 writing and an Excel run over a `.xls` remain | 4–11 |
| PivotTables: slicers, calculated items, verified in Excel | #8 | **met for beta 11-02 (calculated items are the last feature; the real-world corpus is #1's)**: **partial 10-30; timeline drag, GETPIVOTDATA, grouping, filters and calculated items 11-02; the Excel check now runs**: slicers and timelines are modelled, written from the model, placed on the sheet, **drawn and driven by real input** (tiles through `slicer.toggle`, filter button, drag/nudge through `slicer.move` with sub-cell `dx`/`dy`, Delete, context menu, style submenu), a selection filters the pivot through its own refresh path, and a timeline's range handles drag (both ends, or the band, one undo step). A filter records its area now (a page filter on a field that is also on an axis keeps its row; `PivotFilterArea`). **`GETPIVOTDATA` reads a report cell by its field items** — case-insensitive, numbers by value or shown text, `Grand Total` on either axis, no pairs = the grand total — in every layout, with Σ Values in Rows, subtotals and page filters; it depends on the report's written area, so a refresh recalculates it. **Number grouping landed 11-02**: a number field groups into half-open buckets (whole steps read `0-9`, `<start`/`>end` capture what falls outside) set by `pivot.group` start/end/interval, and the cache writes `rangePr startNum`/`endNum`/`groupInterval` with `groupBy="range"`. **Label and value filters landed 11-02**: the field's items are kept or hidden by a caption rule or by an aggregate of a chosen value field, evaluated when set and stored as the field's hidden items. **Calculated items landed 11-02** (`pivot.calculatedItem`: an item of a row or column field computed from that field's own items, sorted last and out of the grand totals, written in Excel's own encoding — the caption as the field's last shared item with `f="1"`, `<item f="1" x>` and `<calculatedItems>` on the cache definition). **Verified in Excel 16 (11-02)**: the grouped-number pivot with a value filter, a slicer and `Region[East]+Region[West]` opens, refreshes and re-saves with every cell agreeing (localized chrome excepted), Excel keeping the calculated item and recomputing 116,320. Missing: data-model (OLAP) pivots, the "Generate GetPivotData" click option and the real-world corpus (#1) | 0–1 |
| Keyboard: key tips, shortcut audit, context menus | #9 | **met 10-29** (417 sequences, keyboard context menus, six shortcuts; the Menu key itself is blocked upstream) | 0–6 |
| Field crashes and freezes closed; autosave/recovery soak | #10 | **partial 10-27/11-02**: the command-sequence soak is green at 500 seeds (release, 10-27) and 100 × 200 steps after the chart/timeline work (debug, 68.3 s, 0 failures, 0 slow); the Windows GPU (#120, #76) and macOS (#117, #166) field reports still need the machines they were reported on | 8–12 |
| **Total** | | gate #10 is open (field hardware reports), #1 waits only for the corpus (Excel accepted every fixture); pass-through #3, charts #4, keyboard #9, calc #6, all-script text #5 and PivotTables #8 met for beta; `.xls`/encryption #7 partly | **~45–85 + 20–30 base = ~65–115** |

With 4–6 agents and an integrator, about 40–60 wall-clock hours. Needs the owner: running Excel
to check our files (gate #1/#8), and Windows machines with the reported GPU drivers (gate #10). Excel
is not licensed on the dev machine (recorded 2026-11-02), so the Excel checks stay open until the
owner provides one; [oracle.md](oracle.md) is what runs in the meantime and is not a substitute for them.

## Milestones

| # | Milestone | Status |
|---|---|---|
| M0 | Foundation: core, number formats, formula language, functions, model, calc, XLSX/CSV, engine | done |
| M1 | Excel look: chrome, ribbon, formula bar, grid, tabs, status bar, control channel | done (polish continues) |
| M2 | Formatting completeness | done |
| M3 | Editing power (AutoComplete, spelling, drag-move/copy) | done |
| M4 | Data (advanced filter, consolidate, what-if) | most; Solver, ToolPak and Power Query remain |
| M5 | Formulas tab (trace arrows, watch window, evaluate, error checking) | done |
| M6 | Conditional formatting manager parity | done |
| M7 | Charts parity | partial; Format pane in; axes, trendlines, error bars, round-trip remain |
| M8 | Insert & Review | partial; ink, icons, comments pane in |
| M9 | Page layout & print | most; Page Layout view remains |
| M10 | PivotTables, Solver, Analysis ToolPak | PivotTables in; slicers, Solver, ToolPak remain |
| M11 | Automation (record actions, scripts) | done |
| M12 | Performance, accessibility, i18n | performance gate met (10-28); accessibility and i18n remain |
| M13 | Release & polish | releases shipping (v0.2.0–v0.4.0, signed) |
| M14 | **Beta gates** (table above) | **in progress**: large-workbook gate met; execution queue below drives the rest |
| M15 | Localization infrastructure and the twelve languages | started: ribbon catalog in ja, zh, ko, pt-BR, ru (#31, #104, #82); German in PR #163 |
| M16 | Get & Transform, data model | not started |
| M17 | Block statistics (zone maps) for skips and whole-column bounds | planned |
| M18 | Vectorized aggregation kernels | landed (10-24/27): whole-column folds and recalc kernels; remaining work in gaps |
| M19 | Cold paging: bands leave memory when unused | **landed (10-28/29)**: compressed pages, byte-budget cache, uncached save scan, spill beside the document, per-sheet save; page-file rewrite deferred |
| M20 | Append-style structural edits (segments, no whole-sheet shifts) | **complete (10-29)**: logical→physical maps, 3.7 ms row / 9.8 ms column on 1M×20, chunked line metadata, cached UI geometry, page-aware `compact` with `document.compact`/save caller, mapped value writes (35/33 ms per 1M), packed column walk and `diff_logical`; only M19's page-file rewrite/byte budget remain |
| M21 | Parquet / Arrow interchange and linked tables | export half done (10-24); reading/linked tables are post-beta (not a beta gate) |
| M22 | A native working format | deferred with four measured triggers (10-26) |

## Large workbooks: what big-data engineering offers a spreadsheet

Baseline, measured 2026-10-25 on the deployable build, on the machine the work is done on (8
threads, loaded): a **118 MB report of 31,709,565 cells** opens in **1.9 s** without the interface,
holds **191 MiB** of model (**6.0 bytes a cell**) and **549 MB** of working set, and **saves in
10.8 s**; a full recalculation of it takes 3.7 s. (`open` was 4.1 s on 10-24.) A **1.28 GB** workbook of 220,715 cells opens in
3.8 s. (On 10-24 the same report opened in 4.1 s and the 1.28 GB one in 5.2 s; the read path, the
row fitting and the recalculation are what moved.) An empty window is 293 MB of that working set —
GPU driver and interface, not data.

The instruments live beside the fork in `_gridcraft-bench/`: `cell_digest` (every cell's value,
formula and style as one hash — the correctness net for any storage change), `save_probe`,
`session_breakdown`, `memprobe`, `deploy_check.py`, `bench.ps1`.

### Already migrated — do not re-buy it

| Big-data idea | Where it already is |
|---|---|
| Parquet row groups (a block of rows, columnar inside) | `Band`: 64 rows behind an `Arc`, one page |
| Dictionary encoding (values once, small indexes per cell) | per band number and text tables; a cell's payload is a `u16` index |
| Run-length encoding | `Runs` for kinds (0.74 B a cell) and styles (0.14 B a cell) |
| Immutable snapshots and a manifest (Iceberg/Delta) | copy-on-write bands; undo is a few `Arc` pointers |
| Batch-parallel scan | a worksheet decoded window by window on eight threads; bands packed in the workers (`BandBatch`) |
| Compression level as a speed/size dial | zip level 3: a 599 MB worksheet packs in 1.25 s into 74 MB (level 6: 4.29 s, 65 MB) |

### Landed on the way to M17 (10-24)

Two things the numbers asked for before the phase itself, both measured on a
million-row sheet (`_gridcraft-bench/formula_bench`):

| Formula over 1,000,000 cells | before | after |
|---|---|---|
| `SUM(A:A)` | 154 ms | 52 ms |
| `COUNT(A:A)` | 286 ms | 50 ms |
| `AVERAGE(A:A)` | 414 ms | 53 ms |

Of the 52 ms, 6 is the recalculation pass itself (`=1` costs that). The fold now
reads the stored pages directly (`CellStore::visit_packed`,
`Host::visit_aggregate`) instead of building a `Cell` an iteration. The rest is
memory: a row is four heap allocations apart from its neighbours, so the walk is
pointer chasing at DRAM speed — which is what the layout item below is for.

`XLOOKUP` now reads a blank as a zero for an exact search, which took the 1.28 GB
report from 2,463 cells disagreeing with its cached values to 159 (see the
`2026-10-24` revision).

### Landed on the open path (10-25)

Three findings, each from measuring rather than guessing:

| Change | Effect on that report |
|---|---|
| A cell's own `r=` is parsed by a tight loop instead of the general address parser (one per cell) | read 2.2 → 1.9 s |
| `fit_imported_rows` skips a workbook whose styles can never grow a row (no wrap, no larger font) | open 3.0 → 1.9 s |
| The rows region scanned instead of walked event by event (590 MiB/s against 226) | open 1.9 → 1.7 s |
| — | open, all told: **4.1 → 1.7 s**; the 1.28 GB report 5.2 → 3.8 s |

The row fitting was the surprise: it walked every cell of every sheet and called `auto_row_height`
per row on every open, and for a file whose styles cannot make a row taller it could not change
anything.

The decode's XML walk was the next one: a generic event per tag runs at 226 MiB/s a thread, and a
worksheet has one tag per cell. `crates/xlsx/src/rows.rs` now scans the rows region itself —
`memchr` for `<`, a name and attribute loop, text straight into the buffers — at **590 MiB/s**
(measured by `examples/scan_probe.rs` on a 255 MiB sheet, same shape as the report's). The general
walker is kept as `parse_general` and a differential test holds the two against each other on
attribute order, quoting, entities, inline strings, comments, CDATA, prefixed names, a row that never
closes, and the forty-row mixed fixture. Open **1.9 → 1.7 s** for the 118 MB report; the scanner is
2.6× faster but the per-cell work now dominates, which is where the next second is.

`examples/cell_probe.rs` splits that per-cell work on a 255 MiB sheet of the report's shape,
single-threaded, 6M cells:

| step | time | per cell |
|---|---|---|
| scan only | 0.43 s | — |
| + what `decode_cell` does (attribute, number, `Cell`) | 0.59 s | 28 ns |
| + the band packing `pack_row` does | 0.99 s | **68 ns** |

Two guesses died on the measurement, and are recorded so nobody repeats them: a one-entry memo on a
band's number, style and text dictionaries (values repeat across *rows*, not within one, so the memo
almost never hits — the report opened 1.69 → 1.79 s) and skipping the post-row `Runs` compaction
(no measurable difference). What is left is allocation: a row with six cells builds eight small
`Vec`s, so six million cells means seven million allocations. That is the band-stripe layout above,
not a patch.

**First increment (10-25)**: `put_row` builds the row in one pass instead of collecting a
`Vec<(u32, u8, Payload, u16)>` first and walking it — one allocation and one copy per cell less.
Packing 68 → 59 ns a cell in `cell_probe` (0.400 → 0.345 s over 6M cells), the report opens in
1.55 s instead of 1.69, and its cells still digest to `f5b186062e525ec8`. The rest of the 59 ns is
the row's own buffers, which is the stripe layout.

### M17 — Block statistics (zone maps) · 8–12 h

**Scope.** Every band records, per column it touches, `(min, max, last row with a value, count)`.
Two uses: skip bands whose range cannot match a predicate (`COUNTIF`, `SUMIFS`, filters) when a
sheet is sparse in rows or columns, and keep `used_range` in O(bands).

Measured first, and the plan changed: a dense million-row sheet has no band that can be skipped,
and a whole-column reference is *already* clipped to the used range (`Evaluator::trim`), so the
"≥5× on SUMIF" claim in the first draft of this phase was wrong. What zone maps still buy is the
sparse case — the 31.7M-cell report holds 62 cells a row over 16,384 columns — and skipping
predicates on columns that are empty or constant over a band.

**Acceptance.**
- `=SUMIF(A:A,">0",B:B)` on a sheet where most bands cannot match: ≥3× faster than the same
  formula on the build before it, with the dense-sheet case no slower.
- Whole-column semantics match the values Excel cached: the 2,463 differing cells documented for
  the 1.28 GB file go to 0 (`_gridcraft-bench/recalc_check`).
- Memory: ≤ 2 MiB added to the 31.7M-cell model; open time within 5%.
- Digest of that report unchanged.

**Risk.** Computing statistics on read costs time; measure it and, if it does, compute them lazily
for numeric columns only. Rollback is a one-line switch in the band builder.

### M18 — Vectorized aggregation kernels · 30–45 h

**Landed (10-24).** Two changes and one finding.

- `MAX` and `MIN` fold a reference the way `SUM` already did: 105 → 47 ms over a million cells.
- A block of the sheet that this pass can no longer change is built once and shared
  (`Host::range_array`) — no formula in it still to be worked out, no spill over it. Two cuts of
  this guard were wrong and the measurement said so both times: refusing to cache anything on a
  sheet where a `FILTER` had spilled disabled it on every sheet of that report, and treating a
  formula the pass isn't evaluating as "not settled" kept every lookup table unshared.
- A text-key column the pass reads more than once is indexed (case-folded, first and last position)
  so an exact `MATCH` is a lookup instead of a scan of two thousand keys: 0.6 s of that report's
  recalculation.

Together: full recalculation of the 1.28 GB report **8.5 → 3.7 s**.

**Measured, worth keeping.** Only 23,400 of the report's 79,552 formulas are evaluated per pass, and
the function calls inside them total ~1.1 s (`IFERROR(INDEX(…MATCH…MATCH))`; a `MATCH` over 2,000
text keys is 24 µs, a linear scan, which is what an unsorted exact match costs). The rest of the pass
is evaluator plumbing, and a full recalculation runs four to five passes — each re-evaluating the
same formulas because `results` and `pending` start empty every pass. Making later passes incremental
is the next real win here, and it is a change to `Calc::calculate`'s loop, not to a kernel.

**Measured (10-26).** The case behind the <100 ms gate: cells of `=$A$1*2`, then an edit of A1.
The cost is linear and about a microsecond a formula, so the gate sits at ~50,000 dependents today:

| dependents | full recalc | edit A1 | a formula |
|---|---|---|---|
| 10,000 | 19 ms | 11 ms | 1.1 µs |
| 25,000 | 59 ms | 37 ms | 1.5 µs |
| 50,000 | 124 ms | **97 ms** | 1.9 µs |
| 100,000 | 281 ms | **222 ms** | 2.2 µs |

`cells_changed` already recalculates only the dependents, so the cost is the pass itself (graph
lookup, evaluation, storing the value), not the dependency walk. Whole-column references are not part
of it: `SUM(A:A)` folds a million cells in 21.5 ms, and a ten-row used range in 0.0 ms.

**Landed (10-27): the gate is met, single-threaded.** Same harness (`_gridcraft-bench/formula_bench`,
release), same machine, before and after this round:

At **100,000 dependents**: full recalc **134–147 ms** (was 281–289) and an edit of A1 **65–75 ms**
(was 245), **0.65–0.75 µs a formula** (was 2.5); the 50,000-dependents edit is ~33–49 ms.

- The pass reads each formula's parsed expression and legacy array range from the dependency graph
  (`Node::expr`, `Node::legacy`), where `rebuild`/`cells_changed` already put them, instead of
  rebuilding every cell for `expr_arc()`: the pass's `exprs` map is built in ~5.5 ms (was ~25).
  `results` is one `Slot` per formula whose empty value *is* the in-progress state — done and being
  evaluated are a single lookup, and an `in_flight` counter lets a top-level pass skip the cycle
  lookup entirely; the `pending` set is gone because the expression map says what is left.
- Write-back sorts the results row-major and calls `CellStore::set_values`, which rewrites the packed
  value slot in place and leaves the formula and style alone; a kind run moves only when the kind
  changes (number → number does not). A band copied for undo shares each row's `Arc<RowExtras>`
  (formulas and arrays) with the current workbook, which took the band-copy part of the write-back
  from ~17 ms to ~0. Of the 66 ms, 37.6 is evaluation, 13.7 write-back, 5.4 the expression map, 4.5
  the dependency closure, 2.2 undo accounting.
- Undo accounting charges a formula's AST from `Formula::expr_bytes`, counted once when the formula
  is parsed, instead of walking every formula of every band an edit touched: 22 → 2 ms.
- `check_arrays` diffs first and looks for legacy array anchors only at or above the lowest edited
  row: a row-1 edit on a 100k-formula sheet scans one formula instead of 100,000 (6.2 → 0.2 ms).
- The evaluator reads a single-cell reference straight from the host (no one-area `Vec`) and folds a
  one-operator binary without allocating the left-chain work list.

A full recalculation of the same sheet fell from 281–289 to 134–147 ms for the same reasons.

**Landed (10-27, second round): independent formulas on the machine's threads.** An edit of A1 with
100,000 `=$A$1*2` cells is now **20–25 ms** end to end (was 65–75 after the single-threaded round)
and a full recalculation of that sheet **89–114 ms** (was 134–147).

- `Calc::run` first evaluates **leaves** — formulas whose precedent blocks contain no formula the
  pass is also recalculating — in up to eight shards, each with its own expression and result maps;
  the shards return their values in row order, which is the write order. Writing a wave makes its
  dependents leaves, so the pass repeats (up to 16 waves): a layered sheet parallelises level by
  level. A formula with a whole-column precedent, a dynamic/volatile formula, an array or legacy
  formula in the dirty set, or iterative calculation sends the whole pass to the sequential path; a
  spill or cycle seen despite that rolls the parallel results back and disables the parallel path
  until a formula changes.
- `CellStore::set_values` writes a large batch (above 16k values, up to 4M) by cloning the touched
  bands in workers and replacing them only when every worker finishes. On 1M formulas this took the
  write-back from ~113 ms to ~55 ms.
- The dependency closure reserves room for the first seed's dependents (a column of 1M formulas),
  and write values are no longer re-sorted: the shards already produce row order.
- `Graph` remembers which formulas may return arrays (`Node::may_spill`, computed once when the
  formula is parsed) and which are legacy array anchors; the safety check is a set intersection, not
  an AST walk per formula.

Measured in `_gridcraft-bench/calcbench` (release, edit A1; `=$A$1`, `=$A$1*2`, `SUM($A$1:$B$1)`
and `IF($A$1>0,1,2)`):

| dependents | edit before | edit now |
|---|---|---|
| 100,000 | 31–36 ms | **17–23 ms** |
| 200,000 | 86–150 ms | **35–42 ms** |
| 500,000 | 244–404 ms | **100–132 ms** |
| 1,000,000 | 517–843 ms | **208–277 ms** |

A full recalculation of the 1M sheet fell from 1.1–1.8 s to 0.85–1.29 s; its graph rebuild dominates.
The parallel path is not used on the 1.28 GB report (its `FILTER` is an array formula), and the
report checks are unchanged: `cell_digest real_117m.xlsx` is still `f5b186062e525ec8` over
31,709,565 cells, and `recalc_check big_1281m.xlsx` still reports the same 159 cells whose cache
Excel left stale (79,552 formulas; 0 cells only after recalc), with a warm open of 2.5 s and a full
recalculation of 3.6 s.

Remaining: dependent formulas that are not leaves (a chain, or a wave below the 16k threshold after
the last wave) are still evaluated sequentially; a chain cannot be parallelised, and a wide array
formula keeps its whole pass sequential by design.

#### Bulk changes and whole-sheet data commands (10-28)

`Calc::cells_changed` used to look for blocked arrays in the 64×64 block above and to the left of
**every** changed cell. A 10M-cell Remove Duplicates therefore made ~40 billion block visits after
the command itself had finished; that, not the command, was the 91 s. It now scans the union of the
changed blocks once (or keeps the per-cell scan for a few scattered edits), which took the
recalculation phase from ~80 s to **1.4 s**. The same edit, measured on the 1M×10
`max_1048576.xlsx` through `gridcraft-cli` (peak working set in brackets):

| operation, 1M×10 | before | after |
|---|---|---|
| Remove Duplicates, A:J | 91 s (1.28 GB) | **12 s** (1.29 GB) — 1M×3: 15.4 → **5.3 s** |
| Sort, B descending | 3.2 s now (recalc 0.02 s) | |
| Fill Down, column A, 1M rows | 1.45 s (588 MB) | |
| Remove Duplicates / Sort on 1M×20 | — | **refused in 1.3 s** (12M-cell cap) |

`data.sort`, `data.removeDuplicates` and `range.setValues` share a **12M-cell cap** (a 1M×10 sheet
is 10.49M and fits; 1M×20 is 20.97M and is refused with the count and the limit). Fill keeps its
own 5M cap. `CellStore::style` reads a cell's packed style without rebuilding its value and formula,
and Remove Duplicates parses each style's number format once instead of once a cell. Whole-sheet
clear/copy/paste still clone the stored cells; they scale with the data rather than the file, and
are the remaining bulk path to measure.

**Scope.** A range read as column stripes (`&[f64]` plus a validity bitmap, taken straight from a
band's payload stripe) and typed loops for `SUM`, `SUMPRODUCT`, `COUNTIF`, `SUMIFS`, `AVERAGE`,
`MIN`, `MAX`. The scalar evaluator stays the fallback for everything else; nothing changes about
what a formula means.

**Acceptance.**
- `=SUM(A:A)` over 1M numeric cells: ≥10× the scalar path.
- Differential test: 1,000 random ranges, identical results (bit for bit) against the scalar path.
- A 1M-row sheet with 100k formulas: full recalc ≤ 2 s (today: minutes at the per-cell rate).

**Depends on** M17 for the stripes being worth walking.

### The save path, measured (10-25)

Saving the 118 MB report takes **~4.2 s** without the interface (a 1.7 s open, then a save), and it
is two sequential CPU loops and nothing else:

| phase | time | what would move it |
|---|---|---|
| XML generation (`sheet4` 1.53 s, `sheet3` 0.50, `sheet2` 0.22) | **2.25 s** | rows are independent — but `write_cell` mutates `Out` in four places, so the generation cannot run on several threads until each is either filled in a pass of its own (in the same order) or turned into a per-chunk result: `out.sst.index(t)` (line 892), `out.cell_pictures.index(picture)` (813), `out.dynamic_arrays = true` (841) and `out.needs_calc` (879). The first two need a `slot()` read path and a prefill pass over the merged row/cell order; the last two return from the chunk and are OR-ed. Tried, and it lost — see below. |
| deflate (`sheet4` 1.15 s, `sheet3` 0.41, `sheet2` 0.19) | **1.76 s** | **Done, partly**: `ZipOut` writes the container itself and compresses the parts on the machine's threads. 1.76 → 1.18 s — the parts that are not the big sheet (0.6 s together) compress in parallel; `sheet4` is 85% of the bytes and is *one* deflate stream, so it stays serial. Splitting a stream is not possible inside one zip entry, which is where the rest of this second is. |
| everything else (workbook, styles, theme, the zip directory) | ~0.2 s | — |

**Tried: a two-mode walk.** `write_cell` was made read-only (`sst.slot`, `cell_pictures.slot`, two
`&mut bool` flags), the tables were filled by the *same* walk in a "tables only" mode (so the two
passes cannot disagree about which string belongs to which cell), and the rows were then generated
in eight chunks: the big sheet's XML fell from **1.53 s to 0.48 s**, the file came out **byte
identical** (`cmp` against the previous build's own output), and every xlsx test passed.

It was **slower**: 2.5 s against 1.53 s for that sheet, and the whole save did not move. The reason
is in the numbers: the serial walk is 1.3–2 s of the 2.25 s (31.7M cells at ~40 ns each, before any
formatting), so paying it twice to parallelise the cheaper half loses. Parallelising the walk itself
means each worker needs its own shared-string table, whose ids then differ from the serial build's —
an id remap over 628 MB of XML would take back most of the 0.7 s that is left. Reverted; the
measurement is the result.

Both blockers are structural, not micro-optimisation, and each is a round of its own. The same
breakdown re-run after any change here is what to hold it against (a byte-for-byte `cmp` against
the file the previous build wrote is the cleanest acceptance test there is, and the report's cells
must still digest to `f5b186062e525ec8`).

### M19 — Cold paging · 60–90 h

The layout a page is made of was measured on the way to M17: a 64-row band is a vector of `Row`s,
and each `Row` owns four separately allocated vectors. Reading a million single-cell rows therefore
chases four million pointers at ~46 ns a cell, against ~2 ns for the arithmetic. A band that kept
its kinds, payloads and bitmaps in one stripe each would make the same walk sequential, cut the
model by ~90 MiB (the per-row headers are 58 MiB of 191), and make paging a single copy instead of
a thousand. It touches every read and write path, so it is a phase of its own, before or with M19.

**Landed (10-26): the stripes.** `crates/model/src/store.rs` builds a band the way it was
blueprinted: `Band` holds `bits`, `kinds`, `payloads` and `styles` as four band-wide vectors with a
slice per row, and `BandRow` is that slice — `key`, `first`, `bits_at`/`words`,
`cells_at`/`cells`, `kinds_at`/`kind_runs`, `styles_at`/`style_runs` — 48 bytes where the four
`Vec`s cost 108. `kind_at`, `style_at`, `payload_at`, `col_at`, `index_of`, `rank_before` and
`column_span` take `&Band` and read the stripes; `put_row` appends to them; `put` and `take` insert
and remove inside them and move every other row's offsets by what the change added; `finish` gives
the slack back once a band is complete instead of once a row. The kinds and styles stay
run-length encoded, as `(at, len)` slices of the same stripe, and the walks carry a `RunCursor` so a
row's kinds cost one pass rather than one search a cell.

Measured on a 1,000,000-cell probe (20,000 rows × 50 columns, 20% text, a style every 7th cell), one
machine, one profile, both stores built from the same tree:

| | per-row `Vec`s | stripes | |
|---|---|---|---|
| `set_row` (packing) | 56.2 ns/cell | **50.6** | −10% |
| `BandBatch::set_row` | — | **49.3** | the reader's path |
| `visit_packed` (a range scan) | 3.94 ns/cell | **2.86** | −27% |
| `iter_range` | 13.3 ns/cell | **11.9** | −11% |
| row headers | 2.40 MB | **0.96 MB** | −1.44 MB a million cells, ≈ −45 MiB on the report |
| packed total | 9.32 B/cell | **7.83** | −16% |
| `set` into an existing row | 422 ns | 429 | +2% — the band-wide shift |
| `remove` one cell | 92 ns | 104 | +13%, the same shift |
| clone (an undo snapshot) | 20.5 µs | 16.5 | −20% |

The packing win is smaller than the 59 → 20–25 ns/cell this section hoped for, and the reason is
worth keeping: of the 4 allocations a row used to make and give back, about 6 ns a cell went to them;
the rest of packing is the column sort `set_row` does, `Cell` construction and the band's number
hash, which a layout does not touch. The memory (−45 MiB on the report) and the range scan (−27%)
are what the stripes buy, and the two write paths pay 2–13 ns for the shift — which is what paging
was going to charge anyway.

Then the report itself (the bench crates beside the fork, `_gridcraft-bench/`; a release build):

| | before | after | |
|---|---|---|---|
| `cell_digest` on `real_117m.xlsx` | `f5b186062e525ec8` | **`f5b186062e525ec8`** | 4 sheets, 31,709,565 cells — unchanged |
| 118 MB report: open | 1.55 s | **1.28** | −18% |
| … its model | 191 MiB | **155** | −36 MiB (154.9 MiB of cells; 162.8 was the breakdown counting two dictionaries twice) |
| … peak working set | — | **293** MiB | |
| 1.28 GB report: open / live | 3.9 s / 238 MiB | **3.77 s / 237** | the packing win is CPU, so mostly flat |

The open time moved more than the estimate expected, because packing is 10% of a loop that runs on
eight threads: 0.27 s of the 1.55 s. Verified with the model's 31 tests (including the differential
`packed_walk_matches_iter_range` and a new `stripes_hold_exactly_the_cells_after_an_edit_soup`,
which keeps a `BTreeMap` as the oracle and checks the stripe bookkeeping after each of 3,000 random
edits), `gridcraft-xlsx`'s 262 and `gridcraft-engine`'s 396.

**Measured (10-26) at 100 million cells.** Ten sheets of 1,048,576 × 10 cells — 104,857,600 cells,
551 MiB of XLSX, built by `make_many.py` beside the fork — in a data-export shape: a unique id and
an amount on every row, four text columns drawing on 255 values.

| | |
|---|---|
| model | **2,404 MiB of cells (24.0 B/cell)**; live 2,411 MiB, peak 2,487 MiB |
| open / reopen | 6.4 s / 6.6 s |
| breakdown | row headers 481 MiB, kind stripe 201, payload stripe 201, style stripe 120, bitmaps 80, text table 640, number table and its slots 679, band structs and extras 4 |
| bands | 163,840 |

The difference from the report is not the layout but whether values repeat inside a band of 64 rows.
The report's numbers repeat (6% distinct a band) and cost 4.9 B/cell; this export's do not, so each
band pays its number table in full (4.3 KiB a band against ~385 distinct numbers) and its text table
too (~7 KiB a band against 255 distinct texts: an `Arc` pointer and an index entry each). The
dictionaries are 1,319 MiB of the 2.4 GB; the four stripes are 602, the row headers 481.

So at the report's density 100M cells extrapolate to ~490 MiB — inside the acceptance — and at the
export's density they are 2.4 GB, four times over it. The acceptance is met for report-shaped
workbooks today and not for unique-key exports, which is what paging is for. Two break-evens are
worth taking before it: a band whose numbers are nearly all distinct should keep them inline
(`NUMBER_WIDE` exists, but its trigger is 65,535 distinct in a band, not cost), and the same for
texts (`TEXT_WIDE`). Neither closes a 4× gap on its own.

**Landed (10-28): a finished band drops its text pointer index.** The text table itself has to stay
(a cell's payload is a `u16` slot into it), but the sorted pointer→slot index only exists to find a
text while cells are being added. `Band::finish` now drops it, like the number hash, and the first
edit of that band rebuilds it from the texts it still holds; it also shrinks the number table (its
capacity was rounded up to a power of two, about 1 KiB of slack a band). A completed band therefore
keeps one `Arc<str>` per distinct text instead of also a 16-byte index entry per text. On the
100M-cell export:

| | before | after |
|---|---|---|
| live after open | 2,411 MiB | **1,911 MiB** |
| cells | 2,404 MiB | **1,905** |
| … texts | 640 MiB | **301** |
| open / reopen | 7.50 s / 7.60 s | **6.63 s / 7.06 s** |

The rest of the breakdown after both shrinks: rows 481, numbers 519 (was 679 before the table was
shrunk to fit), kinds and payloads 201 each, styles 120, bitmaps 80. The text table is still 8 bytes
a distinct text for every band, and the number table is 8 bytes a distinct number; paging is what
takes the remaining 1.9 GB below 600 MB. While fixing the table overflow path, a text past the
`u16` slots now stores its own slot in the row's wide table instead of the last text added
(`TEXT_WIDE` had only been right when the text was the one just inserted). Tests: 42 model,
including a finished band rebuilt on edit and the 65,536th text.

**Landed (10-28): bands page to a compressed spill file under an LRU.** A completed band now has a
second home. `CellStore` keeps `bands` (live, edited) and `pages` (spilled); a page is a
self-describing `GPB1` record with the four stripes, the number table, the text table, the style
table and the formulas/arrays side tables, compressed with zlib at the fast level. `enable_paging`
creates one temporary file per sheet; the reader turns it on before the first row of a sheet in a
package larger than 200 MB (`read_xlsx_file_paged`, `read_xlsx_paged`; the engine's 100M export is
551 MB, while the 55 MB `max_1048576.xlsx` and the 118 MB report never enter this path), and every
`insert_batch` spills the bands it just completed. A read goes through `band_arc`: it decodes the
page once and keeps it in a 4,096-band cache (about 2.6M cells, roughly 50 MiB at this density); the
queue evicts the oldest decoded page, so a full-sheet scan cannot grow the working set back to the
live model. An edit materializes only the band it touches. `used_range`, `formulas` and
`has_owned_cells` answer from bounds/flags in the page metadata, without decoding a page that cannot
contain what they ask for.

| measured through `session_breakdown` (release, counting allocator) | 100M-cell export | 31.7M-cell report |
|---|---|---|
| live after open | **16.7 MiB** | **49.7 MiB** |
| peak | **183.1 MiB** | 232.3 MiB |
| open / reopen | 11.58 s / 11.94 s | 1.47 s / 1.49 s |
| page file | 729.5 MiB compressed (1,727.4 raw), 163,840 pages | none (package ≤ 200 MB) |

The 100M digest is `8964dde87c902c00` both with and without paging, cell for cell; the 31.7M
report's `f5b186062e525ec8` is unchanged. A page that does not parse is treated as missing rather
than trusted; the temporary file is removed with the last store that owns it, and a crash leaves a
file that no reopen reads. The B2 1M×10 sheet stays live (it is 55 MB), so Remove Duplicates is still
12 s; only the package-size threshold crosses into paging.

**Byte budget (10-29).** The decoded-band cache now has a byte budget instead of a page count:
**64 MiB** by default (`GRIDCRAFT_PAGE_CACHE_MB`, clamped 8–512), shared by every clone and undo
snapshot through the page file. A band is charged its measured heap bytes only by the call that
decodes it; `Weak<Page>` + generation make an eviction subtract once, a dead page's charge is
released when it reaches the front, and a single oversize band is kept and reported rather than
thrashing. `PageCacheStats` exposes budget/bytes/entries/hits/misses/evictions/corrupt/
spill-failures/max-entry. Compaction-primed pages are charged and evictable; `Page::decode` stays
uncached. Model tests are **93** (64 store), engine **422**. No peak-memory benchmark was rerun.

**Uncached save scan (10-29).** `CellStore::iter_scan`/`visit_scan` walk the row and column maps
one band and one packed row at a time, decoding paged bands through `Page::decode` instead of the
cache; the XLSX writer now uses it. A save no longer thrashes the viewport's page cache or leaves
the sheet's tail resident, and a 16,384-column band no longer allocates a band-sized `Vec`. A model
test pins that a full scan decodes each band once and leaves `PageCacheStats` exactly unchanged; an
XLSX test pins byte-identical output and unchanged stats after save. Model tests **97**, xlsx
**266** (2 ignored), engine **422**. No save-peak benchmark was run.

**Spill beside the document (10-29).** A paging read now tries the document's folder first and
the system temp directory second, by creating the file (no writability probe); `GRIDCRAFT_SPILL`
accepts `temp`/`off`, and a Windows UNC folder is skipped. The name is a sanitized stem and a
nonce; Unix creates it `0600` and unlinks the name immediately (the fd survives, so a crash leaves
nothing), Windows uses `FILE_DELETE_ON_CLOSE` with a retry. A first write failure latches spilling
off while pages already written stay readable; if no file can be created the workbook still opens
live with one info-bar warning. `GRIDCRAFT_PAGE_MB` pins the size threshold for tests. Model tests
**104**, xlsx **268** (2 ignored), engine **423**. The Unix-only assertions are written but were not
run on this Windows machine; no peak-memory benchmark was run.

**Writer ownership (10-29).** The save no longer copies every part into the zip or keeps the whole
uncompressed set alive while the archive is assembled: `ZipOut::add` takes the `Vec`, each part
moves into its deflate worker and is dropped there, and the zip64 decision is taken up front from
a bound that covers both headers, worst-case deflate and the end record. A full-feature workbook
still saves to the same **37,153 bytes** (`sha256 ae15eeb1…`), pinned by a test; a worker panic now
fails the save instead of writing an empty part. `Out` still retains the sheet XMLs until layout
(and `image_part` reads them back), so the writer still holds every sheet's XML: the per-sheet
release (S4b) is the remaining peak, to be done after a measured save peak. Xlsx **271**, engine
**423** tests.

**Per-sheet save (10-29).** Worksheet XML is handed to a background compression worker as each
sheet is written (`Out::part_precompressed`), so only the worker's current XML and the compressed
parts are alive; `write_zip` assembles mixed raw/pre-deflated parts in the same order, with the
zip64 decision taken from uncompressed sizes. On wasm the packer and the parallel deflate are
compiled out (inline deflate, sequential archive), and an end-to-end Node run produced the same
21,491 bytes as the native build. Output is byte-identical: the 100M package hash
`c7aff2d0…` and the 31.7M hash `11174305…` match the base.

Measured (`session_breakdown`, release, counting allocator):

| file | save | save peak | before |
|---|---:|---:|---:|
| `real_117m.xlsx` (31.7M cells) | **3.48 s** | **1,283.9 MiB** | 3.92 s / 1,655.8 MiB |
| `hundred_m.xlsx` (100M cells) | **28.42 s** | **1,814.3 MiB** | 26.37 s / 6,049.8 MiB |

**Scope (remaining).** A page-file rewrite (bytes appended by compaction) is deferred; streaming
the archive (S4c) and a zip64 streaming writer are post-beta. Bands outside the working set do
leave memory, decompressed when the viewport, calculation or a command touches them, which is the
phase's core. Bands outside the working set
do leave memory, decompressed when the viewport, calculation or a command touches them, which is
the phase's core.

**Acceptance.**
- Working set after opening a 100M-cell workbook ≤ 600 MB, whatever the file size: **16.7 MiB live,
  183.1 MiB peak**, with 729.5 MiB of compressed pages on disk.
- The 31.7M-cell report: open time within 15% of the baseline (1.47 s against 1.37 s on this run),
  paint and edit within 5%.
- Digest unchanged after a cold-then-warm round trip of every sheet: the paged 100M digest matches
  the live one, and the report digest is unchanged.
- A spill file never outlives the document, and a crash leaves no half-written state that a reopen
  would trust: the model test drops the store and checks the file is gone; the codec rejects a
  truncated or corrupt page.

**Risk.** I/O errors, eviction bugs, and a slow first paint after switching sheets. The store treats
a failed page write as “stay live” and a corrupt page as missing, never as cells to overwrite; the
reader's threshold keeps small workbooks out of the path entirely. The remaining work above is the
polish, not the core.

### M20 — Append-style structural edits · 80–120 h

**Scope.** Inserting or deleting rows stops rewriting the sheet. Each sheet keeps a row-mapping
segment list (immutable runs plus a delta), reads resolve a logical row to a physical one, and a
compaction pass folds segments on save or while idle. Today `shift_rows` walks every cell below the
insertion point.

**Landed (10-28): row inserts and deletes are a map.** `CellStore` has a `RowMap` — a sorted list of
logical→physical runs, with an identity fast path that costs no lookup until the first structural
edit. `shift_rows` now changes the map (split the run, shift its logical start, leave every band in
place), so an insert above a million rows is a handful of segments. New rows have no physical row
until something is written into them; that write allocates a fresh physical key above the sheet's
existing rows, and the explicit mapping is kept for reads, `used_range`, `formulas`, iteration and
`visit_packed`. Deletes drop the destination run; the caller (`delete_rows`) has already removed the
cells. `set`, `set_value`, `set_row`, `remove`, `get`, `style`, `contains`, `has`, `iter`,
`iter_range`, `row`, `used_range`, `formula_cells`, `formulas`, `has_owned_cells` and
`visit_packed` all resolve through the map; `serialize`/`Deserialize` save the logical cells, so a
mapped sheet reopens on an identity map. Paged bands stay keyed physically, so a structural edit
does not decode a page.

The other half of the insert cost was unrelated: `rewrite_all_formulas` built its formula list by
walking every cell of every sheet (`cells.iter().filter_map(...)`), so a 20M-cell sheet with no
formulas spent ~300 ms finding none. It now asks the store's formula side tables (`cells.formulas()`),
and `CellStore::iter` walks only bands that exist rather than every logical row.

**Landed (10-28, same round): columns.** `ColMap` is the column sibling: identity until the first
whole-column edit, then sorted logical→physical runs. Unlike rows, its initial run covers only the
sheet's used width, so an inserted column at A allocates physical column 20 on a 20-column sheet
rather than 16,384; a deleted run's physical columns go on a free list. `shift_cols` changes the map;
`get/style/contains/set/set_row/remove/used_range/iter/row/formula_cells/formulas/has_owned_cells`
map through it. Cells in inserted columns are written at a fresh physical key; a column-mapped sheet
uses the value-at-a-time `set_values` path (the logical and physical orders differ), and
`visit_packed` falls back to the logical walk until a physical-column packed walk lands. A column
insert at A in `wide_1m_x20.xlsx` is **9.8 ms command / 42.5 ms commit**, with no cell moved.

Measured on `wide_1m_x20.xlsx` (1M × 20 = 20M cells) through `gridcraft-cli`, deleting the last row
to leave room and then inserting a row at 1:

| | before this round | now |
|---|---|---|
| `home.deleteRows` edit | — | **3.3 ms** (35.8 ms commit incl. recalc) |
| `home.insertRows` edit | — | **3.7 ms** (35.8 ms commit incl. recalc) |
| cells moved by the shift | 20M | **0** (bands unchanged) |

The 1M×10 case is 3.9 / 4.0 ms and 36.8 / 35.7 ms commit. A save/reopen test keeps the values and the
shifted formula, and the model test checks the map, insertion into a gap, delete, iteration and JSON
round trip. Full 414 engine tests, 52 model, 265 xlsx, 86 calc and release fuzz 100×200 stay green.

**Compaction (10-29).** `CellStore::compact()` returns `Compacted` and is **O(1)** when both maps
are identity, covering every clean and freshly opened store. Otherwise it drops live orphan
rows/columns, then visits only the pages a gap can leave an orphan in: row gaps map to band keys,
column gaps test the page's bounds. Each candidate is decoded without priming the page cache;
only a page that actually held an orphan is cloned, re-encoded, appended and replaced by a **new**
`Arc<Page>`, so undo snapshots keep reading their old offsets and cells. Bounds and formula/array
flags are recomputed, `len` is exacted once, and the maps return to identity only when every
candidate was handled. `document.compact` (`noundo`, no menu) and the `file.save` hook (after a
successful Xlsx/Json write, before the saved baseline) call it; other formats and exports do not.
A clean 100M paged store pays the early-out; a decode counter proves a no-op pass visits zero
pages. Model tests are **71** (42 store), engine **422**, xlsx 265. A page-file rewrite, byte budget
and spill placement remain M19.

**Line metadata (10-29).** `Sheet.rows`/`cols` are a `LineMap`: the cell store's logical→physical
runs, with the entries themselves in 128-line `Arc` chunks. An insert or delete moves runs and
renumbers no entry; an undo snapshot clones chunk pointers; a line written into an inserted gap
gets a fresh physical id. JSON and XLSX still read and write logical lines, so files are unchanged;
inserts still inherit the line above (size/style/custom, not hidden/outline/collapsed), deletes drop
their entries, and `moveColumns` follows its insert/move/delete sequence. `LineIndex`, outline,
geometry, sort/filter, clipboard and the file reader/writer all take the logical view; undo
accounting charges only chunks a snapshot does not share. Model tests are 62, engine 419, xlsx 265;
`cargo check --workspace --all-targets` compiles (the UI test suite was not run). Measured on
1M custom row heights (`_gridcraft-bench/linebench`, opt-level 3): `Sheet::clone` **0.13 ms**,
`insert_lines`/`delete_lines` **0.013/0.007 ms**, 10k inherited lines 0.1 ms, and a `LineIndex`
build 6.6 ms (now cached by the UI); a `LineMap::get` on the same map is 0.48 µs.

**UI geometry (10-29).** `GridState` caches the derived `LineIndex` prefix sums and outline axes
per `(document uid, revision, sheet)`; `Geo::new_with_lines`/`Geo::paged_with_lines` consume them.
Scrolling, selection and zoom keep the cache; edits, undo/redo and sheet switches rebuild it. A
sheet with a custom height on every row no longer walks a million lines once or twice a frame.

**Mapped value writes (10-29).** `set_values` now streams through the row map's logical runs
(affine physical rows, one band borrow and one page materialize per run, gaps skipped) and keeps a
run-merged parallel plan; an unsorted batch falls back to value-at-a-time. On 1M formula cells a
1M-value write-back is **35 ms after a row insert** and **33 ms after a column insert** (was
131–135 / 116–142); identity is 36–42 ms. `visit_packed` drives both axes from segment runs, so a
dense block is **2.5–3.3 ns/cell** on identity or a mapped column (was 30.6); a narrow column is
unchanged (per-row bound). `has_owned_cells`/`formula_cells` resolve a band once: the 1M-row paged
gate is **0.84 ms** (was 9.5). `diff` keeps its documented physical contract; the new
`diff_logical` walks logical cells for tests/fuzz only. Model **87**, engine **422**, xlsx 265,
calc 86 tests.

**Remaining.** The M20 base is complete; page-file rewrite and byte accounting defer to M19, beside
spill-beside-document, a byte-budget LRU and a flat-memory save/scan.

**Acceptance (rows and columns).**
- Insert one row above a 1M-row, 20-column sheet: **3.7 ms command / 35.8 ms commit**, below 50 ms;
  no cell moved. Insert one column: **9.8 ms / 42.5 ms**, no cell moved.
- Every existing undo, sort, filter, merge and save test unchanged; the XLSX digest of a mapped
  sheet is the logical one (row and column save/reopen tests cover it).
- Reading a mapped sheet: `get` is one binary lookup over the segments; iteration walks only bands
  that exist; the full suites and the release fuzz stayed unchanged.

**Risk.** Every read path gains a mapping step. It is one branch on the identity fast path and one
binary search after a structural edit; the map is saved nowhere, so a reopened file pays neither.

### M21 — Parquet and Arrow interchange, linked tables · 20–30 h

**Landed (the export half, 10-24).** `file.exportParquet` writes the active sheet (or a `range`) as
Parquet: a column with only whole numbers becomes `INT64`, numbers `DOUBLE`, `TRUE`/`FALSE`
`BOOLEAN`, and anything else `STRING`; blanks are nulls, errors are their text, Snappy-compressed,
one row group per million cells. It is written with the Apache `parquet` crate (`crates/parquet`,
L2, wasm-clean) rather than by hand — the metadata is Thrift.

Measured on the 118 MB report: its 26M-cell sheet exports in ~2.6 s (on top of a 3.2 s open) to
**8.5 MB**, and `pyarrow` — a different implementation — reads it back with the same schema, the
same row count, and the same per-column sums as our own `SUM` (展现量 17,950,607; 花费 551,889.98;
总成交金额 3,070,965.57). A pandas `groupby` over 289 主体ID takes under a second: the query this
was for.

What remains of M21 is the read half — opening a Parquet file, and a linked table that reads row
groups lazily — which needs M19.

**Scope.** Export a sheet or table to Parquet (and to Arrow IPC); open a Parquet file as a sheet;
`data.linkParquet` for a read-only linked table whose row groups map one-to-one onto bands and are
read lazily.

**Acceptance.**
- Export of the 31.7M-cell report ≤ 15 s, file no larger than the xlsx we write today.
- DuckDB `SELECT count(*), sum(col)` on the export equals our own `SUM` over the same range.
- A 100M-row Parquet opened as a linked table: working set ≤ 300 MB, scroll ≥ 30 fps, and `SUM`
  over one column within 3× of the same sum on a native sheet.

**Depends on** M19 for the lazy half. This is the cheapest way to give users real big-data
capability: the data goes to the tools that already do it well, instead of a spreadsheet growing
one.

### M22 — A native working format · deferred, with triggers (10-26)

**Scope.** A block-based, append-friendly primary format that opens by mapping instead of parsing,
saves incrementally, and has no 1,048,576-row ceiling; `.xlsx` becomes the import/export format.

**Decision (10-26, owner: defer).** The condition for building it is that a native format be good
enough to trade interoperability for *and* `.xlsx` bad enough to need trading. Neither holds on the
measurements: the 31.7M-cell report opens in 1.37 s at 155 MiB, the 100M-cell export in 6.4 s, and
no real workbook has hit the row or column cap in a case a sheet split cannot solve. A second format
also costs a second write path forever, while `.xlsx` read and write stay a beta gate either way.
It is reconsidered when one of these is measured:

| Trigger | Threshold |
|---|---|
| Cold paging's spill file needs to be self-describing, versioned, crash-safe and randomly patchable | at M19's acceptance — the spill is then a format whether or not it is called one |
| AutoSave of a 100M-cell workbook takes more than 2 s a save | an incremental save has to buy most of that back |
| Real workbooks hit 1,048,576 rows or 16,384 columns and cannot be split | three independent cases |
| Open time is dominated by parts the document does not need (shared strings, other sheets) | more than half of it, and streaming cannot skip them |

If a trigger fires, the default shape is **one native container (blocks plus a manifest) with
`.xlsx` as the import/export format**, not a replacement for it: interoperability is the product's
floor, and "more than a million rows" is better served by M21's Parquet/Arrow export (20–30 h) than
by a private format (60–100 h).

**Why it was never scheduled.** It contradicts the standing decision against a custom binary format.
That decision was about a *cache* (rebuildable, worthless without the file); a primary format is a
different trade — speed and size against interoperability, and against every tool that reads xlsx.

### Order, and why

1. **M17** — cheapest, fixes a known correctness gap, and pays for the other work with skips.
2. **M21 (export half)** — a week of work that answers "my 30M rows are a nightmare" without a
   single line of query engine: the file goes to DuckDB or pandas, which are built for it.
3. **M18** — the compute side; needs M17's stripes.
4. **M19** — the memory side; the only change that decouples memory from file size.
5. **M20** — the structural side; largest surface, do it last of the five.
6. **M22** only if the owner decides interoperability is worth trading.

### Not doing

| Idea | Why not |
|---|---|
| Whole-table columnar layout (Parquet's shape) | a spreadsheet's locality is a 2D viewport and its hottest operation is a row insert; columnar helps only sort/filter and costs paint, edit and structural edits |
| Distributed or partitioned execution | one user, one machine; block-parallel over threads is the transferable part and is done |
| Vectorized joins and hash aggregation | no table objects to join yet; PivotTables scan their source region and are fast enough at 30M cells |
| MVCC / transaction logs | copy-on-write bands are the simpler equivalent for one writer |

## After beta (ranked)

| Work | Gap | Hours |
|---|---|---|
| Localization: catalog past the ribbon, then twelve languages | #11 | 100–165 |
| Get & Transform and the data model | #12 | 60–100 |
| Solver, Analysis ToolPak, Forecast Sheet | #13 | 22–35 |
| Remaining functions, oracle suite | #15 | 18–30 |
| Collaboration and review | #16 | 35–55 |
| Insert objects | #17 | 25–40 |
| Accessibility | #18 | 12–18 |
| Page Layout view, print polish | #14 | 12–20 |
| Mobile layout | #23 | 30–60 |
| AI features with local models | #22 | 30–50 |
| Plugin / custom-function API | #21 | 25–40 |

## Measuring

- `cargo xtask parity` regenerates [`parity-checklist.md`](parity-checklist.md) (ribbon
  catalog vs registered commands); `cargo xtask ci` checks it is current.
- `cargo test --workspace`: engine, formula, function, XLSX and UI behaviour.
- `cargo run --release -p gridcraft-ui-egui --example snapshot -- --sample sales out.png`: look at
  the UI offscreen.
- Function coverage: the script method in [function-parity.md](function-parity.md). A
  `cargo xtask functions` that diffs the registry against a checked-in Excel name list would make
  it a CI number (1–2 h).
- No `cargo xtask scorecard` or `bench` yet. Until there is: **every performance change ships with
  before and after numbers from the same machine and the same files, and a `cell_digest` of a real
  report before and after** — the 31,194,822-cell bug in M19's predecessor was found by that digest
  and by nothing else in the suite.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-02 | minor | Gate 3: `.xls` **CF12** rules landed — colour scales, data bars and icon sets read from `CondFmt12`/`CF12` (`rgbTemplateParms` is 16 bytes, `rgbCT` a `CFGradient`/`CFDatabar`/`CFMultistate`; thresholds as `CfValueKind`, colours with RGB/palette/theme tint, the 17 ECMA icon-set names). Settled with Excel 16-authored files, which showed Excel's percentile-as-type-7 literal and its inverted data-bar show-value bit. Three Excel `.xls` files read end-to-end and match Excel's own `.xlsx` copies. #7 falls to 4–11 h, beta ~65–115 h |
| 2026-11-02 | minor | PivotTable calculated items landed (`pivot.calculatedItem`, a formula over the field's own items in Excel's own encoding: the field's last shared item with `f="1"`, `<item f="1" x>`, `<calculatedItems>` on the cache with the `Field[item]` formula; sorted last, out of the grand totals, dropped with its field, its grouping or a lost item). **Verified in Excel 16**: the oracle pivot carries `Region[East]+Region[West]`, Excel keeps the definition across open/refresh, recomputes 116,320 and re-saves with every cell equal. #8 is met for beta (0–1 h, the corpus belongs to #1) and beta is ~68–120 h |
| 2026-11-02 | major | Gate #1's Excel half is met: the owner activated Excel 16 and `cargo xtask oracle` now opens/refreshes/recalculates/saves every fixture in Excel (all five agree; openpyxl and WPS remain). It found and fixed the PivotTable number-group encoding (no `refreshOnLoad`, base values in `sharedItems`, next-boundary tail, min-extent anchoring). Only the corpus (3–6 h), gate #7 and #8's calculated items remain; beta ~70–124 h |
| 2026-11-02 | minor | Gate 3b: number grouping landed (half-open buckets with `<start`/`>end` items, `pivot.group` start/end/interval defaulting to the field's extent, `rangePr startNum/endNum/groupInterval groupBy="range"` in the cache, model + report + save/reopen tests); #8 falls to 2–5 h, beta ~71–127 h |
| 2026-11-02 | minor | Gate 3b: label and value filters landed (`pivot.filterByLabel`, `pivot.filterByValue`; evaluated when set, stored as the field's hidden items so report/reopen/refresh agree, the rule kept in the model and reported by `pivot.fields`); #8 falls to 2–4 h, beta ~71–125 h |
| 2026-11-02 | minor | Gate 3b: `GETPIVOTDATA` landed (reads the report's own labels in every layout, with Σ Values in Rows, subtotals and page filters; depends on the report's written area so a refresh recalculates it; in the Insert Function list); #8 falls to 3–7 h, beta ~72–128 h |
| 2026-11-02 | minor | Gate #1 split and recorded as **owner-blocked at the time** (Excel was unlicensed here — volume SKU, no key, no account; the owner activated it later the same day, see the major row), so `cargo xtask oracle` — openpyxl as a foreign reader, WPS 12.0 as a foreign application that recalculates and re-saves — is the accepted substitute ([oracle.md](oracle.md)). It found a PivotTable part using an undeclared `r:` prefix, which `lxml` and WPS both rejected; fixed, and the package validator now refuses that class (11 saving tests red first). #1 → 3–6 h + the owner's run |
| 2026-11-02 | minor | High contrast (`view.highContrast`, View ▸ Show, keytip WVI): black/white/yellow chrome, yellow selection, egui widgets dark, the sheet's own white page, remembered in `ui.json`. Gap #18 → 1–2 h |
| 2026-11-02 | minor | A screen reader can walk the sheet: the visible cells are an AccessKit grid (value, address, formula, row/column index, the active cell selected and named as the grid's active descendant; frozen panes, scrolling, capped at 2,000 cells, built only while a reader is attached). Gap #18 → 3–5 h (high contrast and a status-bar entry remain); the header row and column are header nodes too |
| 2026-11-02 | minor | Alternative text for charts and shapes written to `xdr:cNvPr/@descr` (escaped) and set by `object.setAltText`; the accessibility check covers charts, pictures, shapes, pictures in cells and tables without a header row, and its dialog goes to each issue and sets alt text inline. Not a beta gate; #18 drops to 8–12 h |
| 2026-11-02 | minor | A PivotTable filter records its area (`PivotFilterArea`), so a page filter on a field that is also on an axis keeps its row and a field moved onto an axis becomes an axis filter. #8 → 4–8 h, beta ~73–129 h |
| 2026-11-02 | minor | A data label's own font (size, bold, colour) is modelled at the chart, series and point level and combines field by field from the point outward; read, written, patched, drawn, settable from `chart.setDataLabels` and the pane, and a chartex label font is refused with its reason. **Gate #4 is met** (chartex granularity and a shape/picture pane are outside it); beta ~73–130 h |
| 2026-11-02 | minor | Clicking a bar, area, slice, marker, data point, legend entry or axis label selects it in the Format pane: the renderer returns its click regions (`Drawing`) so the hit test cannot drift, the first click selects the chart and the next the element, and a clicked point is the Data Point section's point. #4 stays 0–2 h |
| 2026-11-02 | minor | Per-point marker size (2–72 pt) modelled, read, written, patched, drawn and settable from `chart.formatPoint` and the Format pane; custom error-bar references editable in the pane. #4 stays 0–2 h |
| 2026-11-02 | minor | `chart.setSeriesAxis` moves a series to the secondary axis and the Format pane has the checkbox; combo series are read back in `c:order` (a series index is stable across save/reopen) |
| 2026-11-02 | minor | A timeline's range handles drag on the sheet: either end resizes the range, the band itself slides without changing its length, both snap to whole days and stay on the field's axis, one drag is one undo step and Esc restores it; the axis is scanned once per revision and cached. #8 → 4–9 h, beta ~73–132 h |
| 2026-10-30 | minor | A table slicer follows a table rename and a column-header rename (its caption too while it still shows the old name); `insert.slicer` applies the documented `caption`; a filter changed on the PivotTable mirrors onto a connected slicer when every connected report agrees and the labels are tiles. Timeline drag, calculated items and value/label filters remain in #8 |
| 2026-10-30 | major | A whole-column/row fill narrows to its own lines' content (one stray far cell used to make a `Ctrl+D` over two columns write 2.1M cells; soak seed 415 s → 5 s), `pasteSpecial`'s moved-cell lookup is a map (O(cells²) → linear); three time regressions with 10× budgets, and the measured scaling/memory recorded in gaps §2 |
| 2026-10-30 | minor | Chart style/colour parts kept byte-identically (chartex and classic); per-point label prefix proven not expressible and retired; #4 → 0–2 h, beta ~75–134 h |
| 2026-10-30 | trivial | Soak harness: per-seed time budget reported as `slow`, wall clock always printed, full capture file, `GRIDCRAFT_FUZZ_REPLAY` |
| 2026-10-30 | minor | Table header names get one in-memory rule and the whole header row is reconciled after a structural edit, so the writer's repair can no longer rename a column behind the model's back |
| 2026-10-30 | minor | A spill that feeds its own source is `#CIRC!` with no values (the pass used to oscillate between two states and stop at its round cap, so neither answer was a fixed point), and a `#CIRC!` anchor is re-evaluated when an edit lands where it would spill. Bounded walk ≤4,096 formulas; two regressions |
| 2026-10-30 | major | Recalc correctness after the soak: a `u16` payload index in the cell store wrapped past 65,535 entries on a heavily rewritten row and aliased another cell's text (a **full** recalc caught it); plus circular self-reads marked in the same pass, discarded spills / defined-name readers / used-range readers re-evaluated, and `#SPILL!` after a used-range move. Five direct regressions |
| 2026-10-30 | minor | Remove Duplicates moves a legacy array with the compacted rows (or refuses when a row would cut it); `view.customViews` and `automate.recordActions` record the workbook-level state they write, so a full undo restores names and scripts. Five soak regressions landed as direct tests |
| 2026-10-30 | minor | Standard (ECMA-376) encryption written with the scheme selectable and remembered; verified by our reader, a from-the-spec Python reader and Excel opening the file; #7 → 7–16 h, beta ~77–136 h |
| 2026-10-30 | minor | Timeline date ranges modelled, written (`dateBetween`/`<selection>`) and filtering through the slicer path via `slicer.setRange`, with clamping; unmodellable states still kept raw; #8 → 6–11 h, beta ~79–139 h |
| 2026-10-30 | minor | Slicer/timeline widgets drawn and driven through commands (tiles, filter button, drag with `dx`/`dy`, arrow nudge, Delete, context menu, style submenu); #8 → 8–14 h, beta ~81–142 h |
| 2026-10-30 | minor | Chartex parts patched instead of regenerated, per-point data labels modelled/read/written/drawn/settable; #4 → 1–3 h, beta ~85–147 h |
| 2026-10-30 | minor | `.xls` validation, conditional formats, hyperlinks and frozen panes read from [MS-XLS], matched to the `.xlsx` reader; CF12 and BIFF5 refused with reasons; #7 → 9–19 h, beta ~86–150 h |
| 2026-10-30 | major | Text shaping wired into the grid (visual runs, Devanagari fallback, visual caret, shaped AutoFit) and into PDF export (cluster-exact `ToUnicode`); gate #5 met with a font-coverage caveat, #5 → 0–3 h, beta ~87–152 h |
| 2026-10-30 | minor | Slicers and timelines placed and wired per [MS-XLSX] and table slicers written (#8 → 12–19 h); a written widget is now listed, anchored and bound instead of merely stored |
| 2026-10-30 | major | Encryption hardened (OS CSPRNG, integrity HMAC verified on read, `Corrupted` distinguished, web refuses a password save) and the formula engine gained a 595-example documented oracle suite over 275 functions; gate #6 met with the oracle caveat, #7 down to 10–21 h, beta ~94–162 h |
| 2026-10-30 | major | Chart Format pane and object resize: the pane drives `chart.setAxis`/`setDataLabels`/`setErrorBars`/`formatPoint` with per-field Auto and shows the reason where a control cannot apply; charts, pictures, shapes and slicers resize from eight handles with Shift, a 20 pt minimum, Esc and one undo step per drag, and the size round-trips through XLSX. Gate #4 → 2–6 h, beta ~102–172 h |
| 2026-10-30 | minor | PivotTable slicers and timelines modelled (cache field, filtered pivots, item values and selected tiles, caption, style, column count, timeline unit, anchor), written once from the model and driven by six commands; a selection writes the field's `PivotFilter` and refreshes in one undo step, so it really filters and survives save/reopen. Unmodellable widget content stays byte-identical. Catalog 280/295, gate #8 → 14–22 h, beta ~104–176 h |
| 2026-10-30 | minor | Chart edits patch the kept chart part field by field (`chart_patch.rs`): only the changed element is rewritten, so unmodelled chart XML survives an edit; unmodelled kinds kept whole instead of dropped; secondary axis modelled, written, patched, drawn. Gate #4 → 4–9 h |
| 2026-10-29 | minor | M19 per-sheet save: worksheet XML deflates on a background worker as it is written, `write_zip` mixes raw/pre-deflated parts in order, wasm uses inline/sequential paths; 100M save **28.42 s / 1,814.3 MiB** (was 26.37 s / 6,049.8), 31.7M **3.48 s / 1,283.9 MiB**, byte-identical output; 273 xlsx / 423 engine tests |
| 2026-10-29 | minor | Measured save peak after S4a: 31.7M cells saves in **3.92 s / 1,655.8 MiB peak**, 100M cells in **26.37 s / 6,049.8 MiB peak** (`session_breakdown`, release, counting allocator). The remaining peak is every sheet's XML in `Out.parts` plus the archive; S4b (per-sheet release) is next |
| 2026-10-29 | minor | M19 writer ownership: `ZipOut::add` takes `Vec` parts, each moves into its deflate worker and is dropped there, zip64 is decided from a whole-archive bound before compression; output unchanged (37,153 bytes / `sha256 ae15eeb1…` pinned); 271 xlsx / 423 engine tests. Per-sheet release remains, after a measured save peak |
| 2026-10-29 | minor | M19 spill beside the document: document folder then temp, `GRIDCRAFT_SPILL=temp|off`, UNC skip, sanitized name, Unix 0600 + unlink-after-open, Windows delete-on-close, write-failure latch and one warning when paging is unavailable; 104 model / 268 xlsx / 423 engine tests |
| 2026-10-29 | minor | M19 uncached save scan: `iter_scan`/`visit_scan` decode one band at a time without touching the page cache, and the XLSX writer uses it (cache stats unchanged, decodes = paged band count, byte-identical output); 97 model / 266 xlsx / 422 engine tests |
| 2026-10-29 | minor | M19 byte-budget page cache: 64 MiB default (`GRIDCRAFT_PAGE_CACHE_MB` clamped 8–512), per-page-file sharing, decode-time heap charging with generation/`Weak` eviction, `PageCacheStats`, compaction-primed pages charged; 93 model / 422 engine tests; spill placement, save/scan and page-file rewrite remain |
| 2026-10-29 | minor | M20 mapped value writes complete: `set_values` streams through row-map runs with a run-merged parallel plan (1M values: 131→35 ms row insert, 116→33 ms column insert), `visit_packed` follows column runs (30.6→2.5–3.3 ns/cell), the owned/formula gate resolves a band once (9.5→0.84 ms/1M), and `diff_logical` is the map-independent diagnostic; 87 model / 422 engine tests |
| 2026-10-29 | minor | M20 page-aware compaction: O(1) identity early-out, page candidates from map gaps (never a full sweep), snapshot-safe page replacement, `document.compact` (noundo) and a `file.save` hook; 71 model / 422 engine tests; page-file rewrite/byte budget stay in M19 |
| 2026-10-29 | minor | M20 UI line geometry is cached per (document, revision, sheet): scrolling/selection/zoom reuse the `LineIndex` and outline axes, an edit or sheet switch rebuilds them; 26 targeted grid tests. 1M custom heights: clone 0.13 ms, insert/delete 0.013/0.007 ms (`linebench`) |
| 2026-10-29 | minor | M20 line metadata maps through physical ids: chunked `LineMap`, logical JSON/XLSX, inserts inherit and deletes drop entries, undo charges unshared chunks; 62 model / 419 engine / 265 xlsx tests; wave 0 and beta estimate narrow to 20–30 base / ~155–240 |
| 2026-10-29 | minor | M20 compaction at model level: `CellStore::compact` reclaims live orphan rows/columns and normalizes maps; 6 new model tests (52 total); page orphans and a save/idle caller remain |
| 2026-10-29 | major | Roadmap re-planned after M19/M20: current focus, execution queue, parallel plan, beta gate status and post-beta order set; large-workbook gate marked met and the old PR-queue wave folded into the gaps |
| 2026-10-28 | minor | Bulk edits scan blocked arrays once for the union of the changed blocks (Remove Duplicates 1M×10 91 → 12 s, recalc 80 s → 1.4 s); Sort 1M×10 3.2 s, Fill Down 1M rows 1.45 s; `data.sort`/`removeDuplicates`/`setValues` share a 12M-cell cap, 1M×20 refused in 1.3 s; `CellStore::style` fast path |
| 2026-10-28 | major | M20 row and column halves: `CellStore` keeps logical→physical `RowMap` and `ColMap`; inserts/deletes change maps instead of moving cells, gaps allocate physical keys on write, and reads/iteration/formulas/used range map. Row insert above 1M×20 3.7 ms / 35.8 ms commit, column insert 9.8 ms / 42.5 ms, 0 cells moved; formula rewrite and iteration stop walking every cell/empty row. Compaction, line metadata and mapped value writes remain |
| 2026-10-28 | major | M19 cold paging: completed bands are compressed into per-sheet `GPB1` page files, loaded on read through a 4,096-band LRU; the engine pages packages over 200 MB. 100M-cell export: 16.7 MiB live / 183.1 MiB peak / 729.5 MiB compressed pages (11.58 s open), digest unchanged; 31.7M report: 49.7 MiB live, 1.47 s open. Remaining: spill beside document, byte budget, save/scan decode |
| 2026-10-28 | minor | M19 step: a finished band drops its text pointer index and rebuilds it on the first edit; the 100M-cell export falls from 2,411 to 1,911 MiB live (texts 640 → 301 MiB, number-table slack returned), digest unchanged; a text past the u16 slots keeps its own slot |
| 2026-10-27 | minor | Independent formulas recalculate on up to eight threads (wave by wave, leaves only; arrays/volatile/iterative stay sequential): 100k end-to-end edit 65–75 → 20–25 ms, 1M calc 517–843 → 208–277 ms; parallel band value writes (4M cap); `Node::may_spill` and graph safety sets |
| 2026-10-27 | minor | The 100k-dependent edit gate is met single-threaded: 245 → 65–75 ms (full recalculation 281–289 → 134–147 ms) by reading parsed formulas and legacy ranges from the graph, one `Slot` per result (in-progress is an empty value), row-major `CellStore::set_values`, `Arc<RowExtras>` band copies, cached `Formula::expr_bytes` for undo accounting, a diff-first `check_arrays`, and single-cell/one-operator fast paths in the evaluator |
| 2026-10-27 | minor | The trust-track soak is closed: 500 seeds × 200 commands (100,000 commands, release, 49 min) are green. Reopening a file rebuilds spilled arrays from the anchor's `ref` and cached values (a contested area keeps its owner), F9 prefers real spill owners and lets a grow-back owner reclaim its cells, swallowed `#CIRC!` stays explicit, `_sc_*` view-state names are invisible to formulas, table filters and blocked legacy arrays round-trip, and Insert/Delete Cells refuses to cut a merge. The format's `#SPILL!`/`#CALC!`-as-`#VALUE!` cache is still to verify on a licensed Excel |
| 2026-10-26 | minor | M19's band stripes landed in `store.rs`: packing −10%, a range scan −27%, row headers 108 → 48 bytes; on the 118 MB report, open 1.55 → 1.28 s and the model 191 → 163 MiB, `cell_digest` unchanged |
| 2026-10-24 | major | Large-workbook plan: M17 zone maps, M18 vectorized kernels, M19 cold paging, M20 append-style edits, M21 Parquet/Arrow, M22 native format (undecided); baseline numbers from the 10-24 build |
| 2026-10-25 | minor | The band-stripe round is written down to the functions it changes, with its acceptance tests |
| 2026-10-25 | minor | `put_row` in one pass (packing 68 → 59 ns a cell, open 1.69 → 1.55 s) |
| 2026-10-26 | minor | M22 decided: deferred, with four measured triggers and one native container plus `.xlsx` import/export as the shape if one fires; the measurements behind it (1.37 s / 155 MiB for the report, 6.4 s for 100M cells, no row-cap case) are cited |
| 2026-10-26 | minor | M18 section gets the 100k-dependents measurement (223–251 ms an edit, 2.3 µs a formula) and the whole-column reference numbers (21.5 ms `SUM(A:A)` over a million cells, 0.0 ms on a ten-row used range) |
| 2026-10-26 | minor | 100 million cells measured: 2.4 GB of model on a unique-key export (24 B/cell) against ~490 MiB extrapolated at report density; the two per-band dictionaries are 1.3 GB of it and paging is still the answer. `packed_breakdown` stops counting the text and style dictionaries twice (the report measures 155 MiB of cells, not 163) |
| 2026-10-25 | minor | The zip container is written by us now, so the parts compress on several threads: deflate 1.76 → 1.18 s, a save 4.2 → 3.7 s, every part verified to unpack to the same bytes with Python's `zipfile` |
| 2026-10-25 | minor | Parallel sheet XML tried and reverted: the file was byte identical and the formatting 3× faster, but the serial walk that the shared-string table forces is 60% of the cost, so the second pass ate the gain |
| 2026-10-25 | minor | Save path measured: 2.25 s of XML and 1.76 s of deflate, with the two structural blockers (shared-string order, the zip writer) written down |
| 2026-10-25 | minor | `cell_probe` records where the decode's time is (band packing 68 ns a cell, mostly allocation), and two measured dead ends: dictionary memos and skipping compaction |
| 2026-10-25 | minor | Open path: a fast cell-reference parse, no row fitting when a workbook's styles cannot grow a row, and a hand-written row scanner (590 MiB/s against the general walker's 226) — a 118 MB report opens in 1.7 s instead of 4.1, the 1.28 GB one in 3.8 instead of 5.2 |
| 2026-10-24 | minor | `MAX`/`MIN` fold references (105 → 47 ms a million cells); a block no formula can change is built once a pass and shared, and text-key columns get an exact-match index: a full recalculation of the 1.28 GB report 8.5 → 3.7 s |
| 2026-10-24 | minor | Parquet export landed (M21's write half): 26M cells to 8.5 MB in ~2.6 s, verified against pyarrow |
| 2026-10-24 | minor | M17 re-scoped after measuring: whole-column references are already clipped, and `SUM`/`COUNT`/`AVERAGE` now fold the stored pages (154/286/414 → 52/50/53 ms a million cells); the layout cost behind the rest is written into M19 |
| 2026-10-11 | minor | PivotTable calculated fields landed |
| 2026-10-10 | minor | Current focus and M15 updated for the 10-10 merges |
| 2026-10-10 | minor | Added the alpha gate table (six core workflows; passes, stays alpha) |
| 2026-10-10 | major | Created: current focus, beta gates (~160–240 h), milestones moved from ROADMAP.md, post-beta ranking |
