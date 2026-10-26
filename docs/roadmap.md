# Roadmap detail

> **Last reviewed:** 2026-10-24 · **Last updated:** 2026-10-26 · **Change:** minor (100M cells and 100k-dependents measured; M19 acceptance split by data density) · **Target:** Microsoft Excel (Microsoft 365)

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
| M21 | Parquet / Arrow interchange and linked tables | export half done (10-24); reading and linked tables remain |
| M22 | A native working format (decision needed) | undecided |

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
lookup, evaluation, storing the value), not the dependency walk: a pass split across the machine's
threads is the shape of the fix, and this table is its case. Whole-column references are not part of
it: `SUM(A:A)` folds a million cells in 21.5 ms, and a ten-row used range in 0.0 ms.

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

**Scope.** Bands outside the working set leave memory: compressed into a spill file beside the
document, kept as a handle, decompressed when the viewport, calculation or a command touches them.
An LRU budget (default a few hundred MB) decides what stays.

**Acceptance.**
- Working set after opening a 100M-cell workbook ≤ 600 MB, whatever the file size. Measured 10-26
  at both densities: report-shaped data extrapolates to ~490 MiB and passes; a unique-key export
  measures 2.4 GB and needs paging, or a dictionary break-even, to pass. Either way 100M cells is
  the number to hold the phase to.
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
| 2026-10-26 | minor | M19's band stripes landed in `store.rs`: packing −10%, a range scan −27%, row headers 108 → 48 bytes; on the 118 MB report, open 1.55 → 1.28 s and the model 191 → 163 MiB, `cell_digest` unchanged |
| 2026-10-24 | major | Large-workbook plan: M17 zone maps, M18 vectorized kernels, M19 cold paging, M20 append-style edits, M21 Parquet/Arrow, M22 native format (undecided); baseline numbers from the 10-24 build |
| 2026-10-25 | minor | The band-stripe round is written down to the functions it changes, with its acceptance tests |
| 2026-10-25 | minor | `put_row` in one pass (packing 68 → 59 ns a cell, open 1.69 → 1.55 s) |
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
