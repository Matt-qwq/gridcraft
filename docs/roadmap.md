# Roadmap detail

> **Last reviewed:** 2026-10-24 · **Last updated:** 2026-10-24 · **Change:** major (large-workbook plan, M17–M22) · **Target:** Microsoft Excel (Microsoft 365)

Forward-looking plan. The one-page summary is [`ROADMAP.md`](../ROADMAP.md); the ranked work
list is [`gaps.md`](gaps.md); numbers and method are in
[`target-app-parity.md`](target-app-parity.md). Estimates are Opus 5.5 agent-hours.

## Current focus

1. **Review and land the open PR queue** (50 PRs on 2026-10-10): recalc and spill fixes (#148,
   #155, #156, #157), whole-sheet and large-range fixes (#151, #152, #153, #154), chart kinds
   (#158, #161), macOS document open (#164, #167), German UI, numbers and formulas (#163),
   15-digit comparison (#187). 8–15 h. Language work should extend the new `i18n` catalog past the
   ribbon rather than add scattered literals.
2. **XLSX on real files** (gaps #1, #3): corpus run plus Excel opening our output.
3. **Large workbooks and multi-threaded recalc** (gap #2).

## Alpha gate

The core workflows a typical Excel user runs every day, checked end to end on the main platform
(macOS desktop), including saving and reopening the work. Any "no", or a "partial" that blocks the
workflow, would make GridCraft pre-alpha.

| Core workflow | Works end to end? | Evidence | Hours to pass |
|---|---|---|---|
| Build a model: enter data and formulas, recalc, save XLSX, reopen | yes | 492 working functions, dynamic arrays, LET/LAMBDA, names, tables; XLSX round-trip tests (`crates/xlsx/src/tests/roundtrip.rs`). Edge-case bugs (#57 15-digit equality, stale spills #133/#139) give wrong results in specific cases, not in the workflow | 0 (fixes are beta work, gap #6) |
| Open a colleague's XLSX, edit, save, hand it back | partial, not blocking | Typical workbooks open and save with cells, styles, CF, validation, tables, names and pivots; unmodelled parts are dropped with a warning (gap #3); Excel-side repair prompts unverified (gap #1); ~100 MB files fail (#175) | 0 for alpha (gaps #1–#3 are beta gates) |
| Format and present: number formats, styles, conditional formatting, print or PDF | yes | Full number-format language, 47 cell styles, all CF kinds, Page Break Preview, PDF export and printing. PDF can't carry non-Latin text (gap #5) | 0 |
| Analyze: sort, filter, tables, PivotTable | yes | Multi-level sort, AutoFilter, tables with totals, PivotTables with field list, layouts, grouping, refresh and XLSX round-trip. Calculated fields done; slicers missing (gap #8) | 0 |
| Chart the data | partial, not blocking | Column, bar, line, pie, area, scatter, combo, histogram, box, waterfall, funnel, treemap, sunburst and stock charts are created, edited and survive save and reopen; trendlines are drawn; no axis options (gap #4) | 0 for alpha |
| Import and export CSV | yes | Encoding and delimiter detection on read, CSV/TSV write; exports raw rather than displayed values (gap #19) | 0 |

**Passes:** every core workflow works end to end on macOS with save and reopen; the two partial
rows lose fidelity only on less common content (very large files, unmodelled parts, advanced
chart kinds), which are beta gates, not alpha blockers. Ready for real work ~50% is inside the
40–75% alpha band.

## Beta gates

Beta means: ~75% ready for real work, opens and saves XLSX reliably, remaining gaps are known bugs
and edge features. Each gate links its gap.

| Gate | Gap | Hours |
|---|---|---|
| XLSX proven on a real-world corpus; Excel opens our output without repair | #1 | 15–25 |
| Unmodelled XLSX parts preserved on save | #3 | 10–15 |
| 100 MB workbooks open; 1M rows usable; multi-threaded recalc; 100k-edit recalc < 100 ms | #2 | 25–40 |
| Charts: kinds and options survive round trip; axes, trendlines, error bars | #4 | 25–35 |
| Text in every script renders and exports | #5 | 12–20 |
| Calculation correctness bugs closed; iterative calculation | #6 | 10–15 |
| `.xls` import, password-protected workbooks | #7 | 28–42 |
| PivotTables: slicers, calculated fields, PivotCharts, verified in Excel | #8 | 20–30 |
| Keyboard: key tips, shortcut audit, context menus | #9 | 12–20 |
| Field crashes and freezes closed; autosave/recovery soak | #10 | 8–12 |
| **Total** | | **~165–255 (≈160–240 after overlap with the PR queue)** |

With 4–5 agents and an integrator, about 40–60 wall-clock hours. Needs the owner: running Excel
to check our files, and Windows machines with the reported GPU drivers.

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
| M12 | Performance, accessibility, i18n | partial; i18n not started |
| M13 | Release & polish | releases shipping (v0.2.0–v0.4.0, signed) |
| M14 | **Beta gates** (table above) | not started as a milestone; parts in open PRs |
| M15 | Localization infrastructure and the twelve languages | started: ribbon catalog in ja, zh, ko, pt-BR, ru (#31, #104, #82); German in PR #163 |
| M16 | Get & Transform, data model | not started |
| M17 | Block statistics (zone maps) for skips and whole-column bounds | planned |
| M18 | Vectorized aggregation kernels | planned |
| M19 | Cold paging: bands leave memory when unused | planned |
| M20 | Append-style structural edits (segments, no whole-sheet shifts) | planned |
| M21 | Parquet / Arrow interchange and linked tables | planned |
| M22 | A native working format (decision needed) | undecided |

## Large workbooks: what big-data engineering offers a spreadsheet

Baseline, measured 2026-10-24 on the deployable build `A162544A98`, on the machine the work is
done on (8 threads, loaded): a **118 MB report of 31,709,565 cells** opens in **4.1 s** without
the interface and **6.1 s** on the desktop, holds **191 MiB** of model (**6.0 bytes a cell**) and
**549 MB** of working set, and **saves in 10.8 s**. A **1.28 GB** workbook of 220,715 cells opens
in 5.2 s and 569 MB. An empty window is 293 MB of that working set — GPU driver and interface, not
data.

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

### M17 — Block statistics (zone maps) · 8–12 h

**Scope.** Every band records, per column it touches, `(min, max, last row with a value, count)`.
Three uses: skip bands whose range cannot match a predicate (`COUNTIF`, `SUMIFS`, filters,
`MATCH` over a whole column); bound a whole-column reference (`$O:$O`) at the used range instead of
1,048,576 rows; `used_range` in O(bands).

**Acceptance.**
- `=SUMIF(A:A,">0",B:B)` over 3.1M cells: ≥5× faster than the same formula on the build before it.
- Whole-column semantics match the values Excel cached: the 2,463 differing cells documented for
  the 1.28 GB file go to 0 (`_gridcraft-bench/recalc_check`).
- Memory: ≤ 2 MiB added to the 31.7M-cell model; open time within 5%.
- Digest of that report unchanged.

**Risk.** Computing statistics on read costs time; measure it and, if it does, compute them lazily
for numeric columns only. Rollback is a one-line switch in the band builder.

### M18 — Vectorized aggregation kernels · 30–45 h

**Scope.** A range read as column stripes (`&[f64]` plus a validity bitmap, taken straight from a
band's payload stripe) and typed loops for `SUM`, `SUMPRODUCT`, `COUNTIF`, `SUMIFS`, `AVERAGE`,
`MIN`, `MAX`. The scalar evaluator stays the fallback for everything else; nothing changes about
what a formula means.

**Acceptance.**
- `=SUM(A:A)` over 1M numeric cells: ≥10× the scalar path.
- Differential test: 1,000 random ranges, identical results (bit for bit) against the scalar path.
- A 1M-row sheet with 100k formulas: full recalc ≤ 2 s (today: minutes at the per-cell rate).

**Depends on** M17 for the stripes being worth walking.

### M19 — Cold paging · 60–90 h

**Scope.** Bands outside the working set leave memory: compressed into a spill file beside the
document, kept as a handle, decompressed when the viewport, calculation or a command touches them.
An LRU budget (default a few hundred MB) decides what stays.

**Acceptance.**
- Working set after opening a 100M-cell workbook ≤ 600 MB, whatever the file size.
- The 31.7M-cell report: open time within 15% of the 4.1 s baseline; paint and edit within 5%.
- Digest unchanged after a cold-then-warm round trip of every sheet.
- A spill file never outlives the document, and a crash leaves no half-written state that a reopen
  would trust.

**Risk.** The highest of the group: I/O errors, eviction bugs, and a slow first paint after
switching sheets. Ship behind `settings.paging` (default on once measured), with the old behaviour
reachable.

### M20 — Append-style structural edits · 80–120 h

**Scope.** Inserting or deleting rows stops rewriting the sheet. Each sheet keeps a row-mapping
segment list (immutable runs plus a delta), reads resolve a logical row to a physical one, and a
compaction pass folds segments on save or while idle. Today `shift_rows` walks every cell below the
insertion point.

**Acceptance.**
- Insert one row above a 1M-row, 20-column sheet: ≤ 50 ms (today: a full shift, seconds).
- Every existing undo, sort, filter, merge and save test unchanged; digest of the saved file
  unchanged.
- Reading a mapped sheet within 5% of an unmapped one (the mapping is one lookup per row).

**Risk.** Every read path gains a mapping step. Do it after M17–M19, when the benches to prove "no
regression" exist.

### M21 — Parquet and Arrow interchange, linked tables · 20–30 h

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

### M22 — A native working format (undecided) · 60–100 h

**Scope.** A block-based, append-friendly primary format that opens by mapping instead of parsing,
saves incrementally, and has no 1,048,576-row ceiling; `.xlsx` becomes the import/export format.

**Why it is not scheduled.** It contradicts the standing decision against a custom binary format.
That decision was about a *cache* (rebuildable, worthless without the file); a primary format is a
different trade — speed and size against interoperability, and against every tool that reads xlsx.
Needs the owner's call, not an agent's.

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
| 2026-10-24 | major | Large-workbook plan: M17 zone maps, M18 vectorized kernels, M19 cold paging, M20 append-style edits, M21 Parquet/Arrow, M22 native format (undecided); baseline numbers from the 10-24 build |
| 2026-10-11 | minor | PivotTable calculated fields landed |
| 2026-10-10 | minor | Current focus and M15 updated for the 10-10 merges |
| 2026-10-10 | minor | Added the alpha gate table (six core workflows; passes, stays alpha) |
| 2026-10-10 | major | Created: current focus, beta gates (~160–240 h), milestones moved from ROADMAP.md, post-beta ranking |
