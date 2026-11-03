# Gaps

> **Last reviewed:** 2026-11-02 · **Last updated:** 2026-11-02 · **Change:** minor (the corpus half of gate #1 is recorded as impossible on this machine — no corpus URL, no checkout, no network; a fresh 100 × 200 soak after the PivotTable changes is green; PivotTable label and value filters keep the items whose captions or aggregates match; PivotTable number grouping buckets a number field into ranges, the way the Group Field dialog does; GETPIVOTDATA reads a report cell by its field items; the owner recorded that Excel is unlicensed on the dev machine, so gate #1 stays open and owner-blocked; `cargo xtask oracle` reads our output back through openpyxl and WPS as the accepted substitute and found a PivotTable part with an undeclared `r:` prefix, now fixed and guarded by the package validator; a timeline's range handles drag on the sheet, a chart series moves to the secondary axis, combo series keep their order across save/reopen, and a 100 × 200 command-sequence soak is green) · **Target:** Microsoft Excel (Microsoft 365)

Every known shortfall against Excel, one entry each, ranked by user impact (what blocks someone
using GridCraft for real work first). This is the work list: pick from the top, check the open
PRs first (50 open on 2026-10-10; several already address entries here), and update the entry
when you land work. Estimates are Opus 5.5 agent-hours. Area detail lives in the linked parity
docs; the summary is in [target-app-parity.md](target-app-parity.md).

Kinds: **F** feature · **U** UI/UX · **FF** file format · **H** hardware · **L** localization ·
**P** performance · **S** stability.

## Beta blockers

No alpha blockers: every core workflow passes the [alpha gate](roadmap.md#alpha-gate).


### 1. XLSX output not proven in Excel or on real files (FF)
- **Missing:** a run over a real-world XLSX corpus, and confirmation that Excel opens what we save
  with no repair prompt (PivotTables, charts, CF, validation, dynamic arrays, tables).
- **Evidence:** round-trip tests cover our own files only (`crates/xlsx/src/tests/`); PivotTable
  round-trip noted as "unverified in real Excel" since 10-07; #64 (dynamic-array flag) shows the
  class of bug. The non-Excel half now runs through [`cargo xtask oracle`](oracle.md) (openpyxl and
  WPS) and `gridcraft-cli check`. **The corpus half cannot start on this machine (11-02):** the repo
  records no corpus URL, `craftrules` is not checked out next to the repo, and there is no network
  (github.com timed out), so the pinned corpus cannot be fetched here — 3–6 h still, on a machine
  with the corpus and network.
- **Impact:** the main format; a repair prompt or silent loss destroys trust.
- **Estimate:** 15–25 h, plus owner time in Excel. [file-format-parity.md](file-format-parity.md)

### 2. Large workbooks: what remains of the open/save problem (P, H)
- **Landed (10-24):** the read path streams a worksheet as it decompresses, decodes windows on
  eight threads, packs row bands inside those threads (`BandBatch`), and inflates the next window
  while the last decodes; the write path walks the store once and compresses at level 3. The 118 MB
  report of 31,709,565 cells now opens in 4.1 s (6.1 s on the desktop) at 549 MB of working set and
  saves in 10.8 s; the 1.28 GB report opens in 5.2 s at 569 MB.
- **Landed (10-26):** the store's rows are band-level stripes now (bitmaps, kinds, payloads and
  styles one vector a band with a slice a row) instead of four vectors a row: a range scan costs
  2.9 ns a cell instead of 3.9, packing is 10% cheaper, and the 118 MB report opens in 1.37 s
  holding 155 MiB of cells (191 MiB), digest unchanged; details and the A/B in
  [`roadmap.md`](roadmap.md#m19--cold-paging--6090-h). The 163 MiB first reported was the
  breakdown counting the text and style dictionaries twice, now fixed.
- **Measured (10-26) at 100 million cells:** a 551 MiB, 10-sheet export opens in 6.4 s and holds
  2.4 GB (24 B/cell); its per-band dictionaries are 1.3 GB of that, because every id and amount is
  distinct. Report-shaped data at the same count extrapolates to ~490 MiB. So the M19 acceptance
  (≤ 600 MB) holds at report density and paging is what the unique-key export needs.
- **Landed (10-27): the 100k-dependent edit gate.** An edit of A1 with 100,000 `=$A$1*2` cells costs
  **65–75 ms** against the 100 ms gate (was 245 ms; a full recalculation of the same sheet fell from
  281–289 to 134–147 ms), single-threaded, 0.65–0.75 µs a formula. The pass reads parsed formulas and
  legacy array ranges from the dependency graph instead of rebuilding 100k cells, keeps one `Slot`
  per result (in-progress is an empty value), writes values row-major through
  `CellStore::set_values`, shares each row's `Arc<RowExtras>` in a band copied for undo, charges undo
  from a cached `Formula::expr_bytes`, diffs before looking for legacy array anchors, and reads a
  single-cell reference without an area vector. Details and the phase split in
  [`roadmap.md`](roadmap.md#m18--vectorized-aggregation-kernels--3045-h).
- **Landed (10-27, second round): multi-threaded recalculation of independent formulas.** A pass now
  evaluates its leaves in up to eight shards (one expression/result map each), writes them in row
  order, and repeats for the next layer (up to 16 waves); a whole-column precedent, a dynamic or
  array formula in the dirty set, or iterative calculation keeps the pass sequential, and a spill or
  cycle rolls the parallel results back. `CellStore::set_values` clones touched bands in workers for
  large batches. An edit of A1 with 100,000 `=$A$1*2` cells is **20–25 ms** end to end (was 65–75),
  200k is ~35–42 ms, 500k ~100–132 ms and 1M ~208–277 ms in `_gridcraft-bench/calcbench`; the 100k
  full recalculation is 89–114 ms. Details in [`roadmap.md`](roadmap.md#m18--vectorized-aggregation-kernels--3045-h).
- **Landed (10-28): bulk edits and whole-sheet data commands.** `Calc::cells_changed` used to scan
  a 64×64 block for blocked arrays **per changed cell**; a 10M-cell Remove Duplicates therefore did
  ~40 billion block visits. It now scans the union of the changed blocks once (or keeps the per-cell
  scan when the changed cells are scattered), and the same 10M-cell Remove Duplicates fell from
  **91 s to 12 s** (1M×3: 15.4 → 5.3 s); its recalculation phase went from ~80 s to 1.4 s. Sort on
  the same 1M×10 sheet is **3.2 s** (recalculation 0.02 s) and Fill Down on 1M rows **1.45 s**.
  `data.sort`, `data.removeDuplicates` and `range.setValues` share a **12M-cell cap** (1M×10 fits;
  1M×20 is refused in 1.3 s with a clear error), and `CellStore::style`/Remove Duplicates parse a
  style's number format once rather than once a cell.
- **Landed (10-28): the per-band text index goes away when a band is finished.** The sorted
  pointer→slot index is only needed while cells are being added, so `Band::finish` drops it and the
  first edit rebuilds it; a completed band keeps one `Arc<str>` per distinct text. The 100M-cell
  unique-key export drops from 2,411 to **1,911 MiB** live (texts 640 → 301 MiB; number tables 679 → 519 MiB), digest unchanged.
- **Landed (10-28): cold paging.** Completed bands are compressed into per-sheet `GPB1` page files
  (zlib, fast) and read back through a 4,096-band LRU; the engine pages packages over 200 MB, so the
  55 MB 1M×10 sheet stays live. The 100M-cell export opens with **16.7 MiB live and 183.1 MiB peak**
  (was 1.91 GB live) and 729.5 MiB of compressed pages on disk; its digest is cell-for-cell the same
  with and without paging. The 31.7M report stays live at 49.7 MiB and opens in 1.47 s. Page files
  are removed with the store; a corrupt or truncated page is refused.
- **Landed (10-28): row inserts/deletes are a logical→physical map.** `CellStore::shift_rows`
  changes a segment list instead of moving cells; a gap row gets a fresh physical key when written,
  and reads/iteration/used range/formulas map through. Insert one row above a 1M×20 sheet is
  **3.7 ms command / 35.8 ms commit**, with no cell moved. The formula rewrite no longer walks every
  cell to find none (it reads the store's formula side tables), and iteration skips empty bands.
- **Landed (10-28): the column sibling.** `ColMap` materializes only the used width, allocates a
  fresh physical column for a written insert (or reuses a deleted one), and maps reads/iteration/
  formulas/used range. A column insert at A in the same 1M×20 sheet is **9.8 ms / 42.5 ms commit**,
  no cell moved. Save/reopen tests cover both axes; column-mapped value writes take the
  value-at-a-time path.
- **Landed (10-29): row/column line metadata is a physical-id map too.** `Sheet.rows`/`cols` are a
  `LineMap`: the store's logical→physical runs with the entries in 128-line `Arc` chunks. A row or
  column insert/delete moves runs instead of reindexing every custom height, hidden line or
  outline entry, an undo snapshot shares untouched chunks, and JSON/XLSX still store logical lines.
  Insert inheritance, delete, undo and a mapped-sheet XLSX round trip are covered (model 62,
  engine 419, xlsx 265 tests). The UI also caches the derived `LineIndex`/outline axes per
  `(document, revision, sheet)`, so a 1M-custom-height sheet no longer rebuilds them every frame.
- **Landed 10-29: per-sheet save.** Worksheet XML deflates on a background worker as each sheet
  is written; the archive mixes raw/pre-deflated parts in order, and wasm uses inline/sequential
  paths. The 100M-cell save now peaks at **1,814.3 MiB in 28.42 s** (was 6,049.8 MiB / 26.37 s),
  and `real_117m` at 1,283.9 MiB in 3.48 s; output hashes are unchanged. Only the deferred
  page-file rewrite and post-beta streaming save remain.
- **Landed (10-29): writer ownership.** `ZipOut::add` takes each part `Vec` instead of copying it,
  `write_zip` moves each part into its deflate worker and drops it there, and the zip64 decision is
  taken before compression from a whole-archive bound. Output is byte-identical (37,153 bytes pinned
  by test); a worker panic fails the save instead of writing an empty part. The writer still holds
  every sheet's XML until layout, so per-sheet release (S4b) remains.
- **Landed (10-29): spill beside the document.** A paging read creates the page file in the
  document's folder (temp fallback), with `GRIDCRAFT_SPILL=temp|off`, a UNC skip, a sanitized name,
  Unix 0600 + unlink-after-open and Windows delete-on-close. A write failure latches spilling off
  while existing pages stay readable; no file means the workbook opens live with one warning.
- **Landed (10-29): byte-budget page cache and uncached save scan.** The decoded-band cache is
  64 MiB by default (`GRIDCRAFT_PAGE_CACHE_MB` clamped 8–512), shared per page file, charged at
  decode with generation/`Weak` eviction and `PageCacheStats`. `iter_scan`/`visit_scan` decode one
  band at a time without touching it, and the XLSX writer uses that path: a save leaves the
  viewport's cache exactly unchanged and no longer collects a 16k-wide band.
- **Landed (10-29): mapped value writes and packed folds.** `set_values` streams through the row
  map's logical runs and keeps a run-merged parallel plan; 1M values after a row insert write in
  **35 ms** (was 131–135), after a column insert 33 ms (was 116–142). `visit_packed` follows the
  column map's runs, so a dense block is **2.5–3.3 ns/cell** (was 30.6), and the owned/formula gate
  resolves a band once (1M paged rows 0.84 ms, was 9.5). `diff` keeps its documented physical
  contract; `diff_logical` is the O(cells) map-independent diagnostic for tests/fuzz.
- **Missing:** dependent chains that never become a wave of leaves remain sequential; whole-sheet
  clear/copy/paste still clone the stored cells (they scale with the data, not the file); the
  per-band value dictionaries still cost full price when values do not repeat;
  page-aware compaction and the 64 MiB byte-budget page cache landed 10-29 (identity early-out,
  gap-scoped candidates, snapshot-safe page replacement, `document.compact` and a save hook,
  `PageCacheStats`); a page-file rewrite remains;
  whole-column references are already clipped (`SUM(A:A)` folds a million numeric cells in 21.5 ms,
  a ten-row used range in 0.0 ms). The 159 cells that still differ from the cached values of the
  1.28 GB report are Excel's stale cache — 276 cells of the same `IFERROR(XLOOKUP(...))` shape in the
  same workbook cache `#N/A` — not a calculation difference.
- **Evidence:** `crates/xlsx/src/sheet_read.rs`, `crates/model/src/store.rs`, `crates/model/src/line.rs`,
  `docs/roadmap.md` (M17–M21), and the harness kept beside the fork (`_gridcraft-bench/`).
- **Impact:** finance and data users routinely have big workbooks.
- **Status (10-29): the large-workbook beta gate is met.** Recalc, edit latency, 1M-row operations,
  bounded data commands and memory all pass their targets; M19 cold paging and the M20 row/column
  maps landed. Remaining work is base hardening, not capacity: M19 spill placement, flat-memory
  save/scan (and the deferred page-file rewrite) are wave 0 in
  [`roadmap.md`](roadmap.md#execution-queue). The M21 Parquet read half is post-beta. The old
  estimate is superseded by those waves.

### 3. Some unmodelled XLSX content is dropped on save (FF)
- **Landed (10-30, fills and pastes):** a whole-row/whole-column fill narrowed its target to the
  *sheet's* used-range box, so one stray cell far down the sheet made `Ctrl+D` over two columns write
  1,048,576 rows (2,097,102 cells where the sheet held 84, 830 ms); it now narrows to the content in
  **its own lines** (spilled arrays included), 50 cells in under a millisecond. `pasteSpecial`
  looked a moved cell up by scanning a list (`O(cells²)`): 25/90/332/1,322 ms at 10k/20k/40k/80k rows
  → 10/20/67/97 ms. Both pinned by time tests with 10× budgets. Measured with them: structural edits
  scale linearly (delete an empty column 0.42/2.4/18.7/32.9 ms at 20k/200k/1M/2.1M cells; a filled one
  2.6/25.1/157/293 ms), 2.1M unique cells hold 171 MB (≈75 B/cell), and a **save transiently doubles
  cell memory** because the writer serialises through a second, unshared copy.
- **Landed (10-29):** pass-through keeps slicers and their caches, timelines and their caches,
  table-attached query tables, and unknown `extLst` children on the workbook, worksheet, table,
  PivotTable and pivot cache definition. Part bytes come back byte-identical, rels are re-pointed
  with their ids preserved, and every kept fragment is keyed to its owner (`table:`, `pivot:`,
  `pivotcache:`): a deleted, renamed or replaced owner drops its extras, the parts only they reach
  and the rels in the same save, so no dangling relationship is written. A strict package validator
  (every rel target exists, every part has a content type, no orphans, no case-only duplicate
  names, no stale `Override`, no duplicate rel ids) runs inside `write_xlsx` for every save in the
  crate's tests. The earlier set (#330) still holds: custom XML, external links, connections, sheet
  query tables, printer settings, controls/ActiveX/OLE/backgrounds, header-footer pictures,
  chart sheets, workbook and worksheet `extLst`.
- **Missing:** the data model (Power Pivot `xl/model/…`, `powerPivotData`), chart `extLst` and chart
  details (a chart part has no stable identity to key a fragment on; chart depth is #4), Strict Open
  XML content. A renamed table or PivotTable loses its extras by policy — misattaching a query table
  is worse than dropping it — which is a product decision worth revisiting.
- **Not verified:** whether Excel opens a re-saved slicer/timeline workbook without a repair prompt
  (needs the owner's Excel half-day, #1). No Excel run has happened.
- **Evidence:** `crates/xlsx/src/passthrough.rs` (what is kept, and why the rest isn't);
  `crates/xlsx/src/testing.rs` (`check_package`); the matrix in
  [file-format-parity.md](file-format-parity.md).
- **Impact:** open-edit-save no longer simplifies workbooks with slicers, timelines or query tables;
  Power Pivot workbooks still lose the model.
- **Estimate:** 0–2 h left of the 6–10, plus the Excel check folded into #1.
  [file-format-parity.md](file-format-parity.md)

### 4. Charts lose options on round trip; shallow chart model (F, FF)
- **Landed (10-29):** the model carries, and reads/writes/draws, axis scaling (min, max, major unit,
  log base, reversed, crossing, display units, number format), data labels (value, percentage,
  series name, category name, legend key, position, separator, number format, per-series visibility,
  per-point deletion), error bars per series (fixed, percentage, sample standard deviation, standard
  error, custom `numRef`/`numLit`, both directions, `noEndCap`) and per-point fill/line/marker
  colour. Four commands: `chart.setAxis`, `chart.setDataLabels`, `chart.setErrorBars`,
  `chart.formatPoint` — a chart kind that cannot store an option refuses it rather than holding a
  dead value. Catalog 274 → 279 / 290 → 295.
- **Landed (10-29, second round):** an edit **patches** the chart part instead of regenerating it
  (`crates/xlsx/src/chart_patch.rs`): only the changed element is rewritten, so everything we do not
  model survives an edit — manual plot-area and label layout, label callouts and leader lines,
  `dTable`, `serLines`, `overlap`, date-axis units, minor units and gridlines with their formatting,
  axis `spPr`/`txPr`, legend entry and title-run formatting, series `extLst`, `printSettings` — with
  the test asserting `part.replace(<patched element>, "")` equals the original bytes. Charts of
  unmodelled kinds (3-D, surface, pie of pie, bar of pie, Pareto, filled map) are kept whole
  (`Sheet::kept_charts`, `Obj::RawChart`) instead of being dropped or flattened. The secondary axis
  is modelled (`Chart::y2_axis`), written, patched, drawn and addressable as `chart.setAxis axis:
  "y2"`; category-axis bounds crop the drawn window.
- **Landed (10-30, UI half):** charts, pictures, shapes and slicers resize from all eight handles
  (before, only the bottom-right corner resized and any other drawn handle *moved* the object): one
  `object.move` per frame carrying the anchor and the extent, Shift keeps the aspect ratio, 20 pt
  minimum, Esc cancels without an undo entry, one drag is one undo step, arrow keys nudge 10 pt
  (1 pt with Ctrl) and the size survives save/reopen. The **Format pane** drives
  `chart.setAxis`/`setDataLabels`/`setErrorBars`/`formatPoint` for the chart area, either axis and
  each series, with per-field Auto; an option the chart kind cannot store is not offered and the
  section shows the command's reason (chartex charts, a pie's axes, bounds on a label axis, error
  bars on a radar).
- **Landed (10-30, chartex + point labels):** a modelled **chartex** chart (histogram, waterfall,
  box & whisker, funnel, treemap, sunburst) is now **patched** instead of regenerated — an edit
  rewrites only the element it touched (titles, legend position, gridlines, `cx:valScaling`,
  per-series fill/outline and label flags, `quartileMethod`) and every other element, including the
  hidden `_xlchart` name table, stays byte-identical; a change the part cannot express (data
  reference, kind, series set) regenerates it. **Per-point data labels** are modelled (`PointLabel`:
  rich text or a `strRef`, the four show flags, position) on `Series::point_labels`, read from
  `c:dLbl` inside a series and inside a `dPt`, written back with `idx`/`tx`/`dLblPos`/`show*`, drawn
  at the right point and exposed as `chart.setDataLabels` with `point` + `text`.
- **Corrected (10-30):** the claim that a chart's custom error-bar references are lost on an edit was
  **wrong** — they already survived read → edit → save byte-for-byte; a regression test now pins
  read → patch → save → read.
- **Landed (10-30, style parts):** a chart's own `styleN.xml`/`colorsN.xml` are **read and written back
  byte-identically** for both chartex and classic parts (classic parts with style/colour relationships
  are now kept too, which real Excel files usually have); they survive a field patch and are dropped
  only when the part must be regenerated (a kind, series-set or reference change), which is stated in
  `chart-parity.md`.
- **Settled (10-30):** a **per-point label prefix/suffix does not exist in the schema** — `c:dLbl`
  carries `idx`/`delete`/`layout`/`tx` (`strRef` or rich text)/`numFmt`/`spPr`/`txPr`/`dLblPos`/show flags/
  `separator`, and chartex's `cx:dataLabel` has **no text element at all**. A fixed rich text replaces the
  composed label, which is exactly what `PointLabel::text` already does, so the claim is retired rather
  than faked.
- **Landed 11-02 (secondary axis and series order):** `chart.setSeriesAxis {series, secondary?,
  type?, subtype?}` moves one series to the secondary value axis and optionally changes its kind
  (the model side of Change Chart Type → Secondary Axis), turning the chart into a combo; the
  Format pane's Series Options carries the checkbox. Reading a combo chart back used to return its
  series in the writer's **group order** (primary bars, primary lines, secondary), so a series
  index from before a save pointed at another series after reopen — the reader now sorts by
  `c:order`/`c:idx`, and a secondary line group keeps its markers when written.
- **Landed 11-02 (per-point marker size):** `PointFormat::marker_size` (2–72 points) is read from
  `c:dPt/c:marker/c:size/@val`, written, patched, drawn at that radius and settable from
  `chart.formatPoint {markerSize}` and the Format pane's Data Point section (default 5). A point's
  own marker now writes even when the series marker is switched off.
- **Landed 11-02 (click-select):** a chart element is selected by clicking it, with the pane's
  dropdown reaching the same elements: the chart is laid out into its primitives **and the boxes a
  click can land on** (`gridcraft_chart::draw` → `Drawing { prims, hits }`), so the hit test is the
  drawing's own geometry and cannot drift from it. A bar, area band, marker, a line's data point,
  a pie/doughnut/sunburst slice (an exact wedge region), a legend entry or an axis label band names
  the series and, when one point was hit, that point; the first click on an unselected chart
  selects the chart, the next picks the element (Excel's two-step gesture), and a click on the
  frame's padding goes back to the chart area. The Data Point section follows the clicked point, so
  a bar can be formatted without touching the point number.
- **Landed 11-02 (label font):** a data label's own **font** — size, bold and colour — is modelled
  at the chart, series and point level (`c:txPr` on `c:dLbls` and on `c:dLbl`), read, written,
  patched, drawn and settable from `chart.setDataLabels {fontSize, bold, color}` and the Format
  pane's Data Labels and Data Point sections. Levels combine **field by field**: a point that sets
  only a colour keeps its series' size and the chart's weight, and the renderer draws the size,
  weight and colour it ends up with. A chartex chart's label font is refused with the reason rather
  than stored and dropped. The typeface, italic and the label's own fill/callout stay unmodelled
  (kept while their element is untouched, lost when that label's font is edited).
- **Missing:** a label's typeface, italic, fill and callout shape are not modelled; date-axis units
  are kept but not editable; from-cells labels, callouts added through the UI, chart templates and
  an element-level pane for shapes/pictures are not there.
- **Evidence:** `Chart` (`crates/model/src/features.rs`) holds the option families above;
  `chart_options_roundtrip` and `chart_unmodelled_kinds` pin build → save → reopen → second save
  byte equality; the patch path is pinned per chart kind; `ui-egui` `chart_format_pane` and
  `chart_resize` (11 tests, including save → reopen of a dragged chart) pin the UI half. Smooth
  lines (#62), Switch Row/Column (#116) and trendlines (read, written, drawn) landed; multi-level
  category axes (year beside month) are read, drawn and written back.
- **Impact:** charts are in most business workbooks.
- **Estimate:** 0–2 h for beta (30–45 h full). [chart-parity.md](chart-parity.md)

### 5. Text in non-Latin scripts doesn't render or export (L)
- **Landed 10-29 (font discovery + PDF embedding):** `gridcraft-core::fonts` finds a font file
  (`CRAFT_FONTS_DIR`, then the platform's font directories, plain `read_dir`, no fontconfig) and
  answers per-character coverage from `cmap`; `gridcraft-pdf` reads TrueType, subsets it (composite
  closure, rebuilt tables) and embeds a `CIDFontType2` with `FontFile2`, `Identity-H`, `/W` and a
  `ToUnicode` CMap, so Latin Extended, Greek, Cyrillic, Hebrew, Arabic, Devanagari, Thai, CJK and
  Hangul text reaches the page instead of vanishing. ASCII/WinAnsi documents keep their previous
  bytes because the engine only resolves a font when a character falls outside WinAnsi.
- **Landed 10-30 (shaping engine):** `gridcraft-core::text` resolves paragraph levels and reordering
  (`unicode-bidi`) and shapes each run (`rustybuzz`), returning positioned glyphs in **visual order**
  with logical UTF-8 byte clusters, mark offsets and a bounded cache. Arabic joins and ligates,
  Devanagari reorders and merges conjuncts, Thai marks take zero advance, `fi` ligates and RTL
  mirrors brackets; 26 tests pin levels, visual chunk order, cluster tiling and hostile input.
- **Landed 10-30 (both paths wired):** the **grid** draws each cell's bidi runs in visual order
  (one engine per app, registered on the faces egui paints with, chosen by a bounded cmap probe so
  the per-char fallback cannot substitute a glyph the engine did not position), AutoFit measures the
  shaped width, the **in-cell editor** walks the caret through visual clusters and maps a click to the
  cluster under it, and a Devanagari fallback face makes Hindi cells draw at all. **PDF export**
  shapes too: glyphs are placed with `TJ`/`Ts` on one text matrix, `ToUnicode` maps the glyph that
  carries a cluster to the whole logical cluster text (a ligature never loses a character, a
  multi-glyph cluster never repeats one) and a `CIDToGIDMap` is written only when one glyph must
  extract differently — so WinAnsi text stays **byte-identical**. `pdftotext` now returns Cyrillic,
  Arabic, Hebrew, Devanagari and Thai in logical order.
- **Still missing:** CJK and Arabic in one document (no single face covers both), fonts shipped per
  platform, per-run font fallback, shaped layout for **wrapped** cells, text boxes and chart labels,
  mirrored printing and right-to-left sheet tabs, and shaped *measurement* in the PDF's
  `text_width`; CFF/OTF (`CIDFontType0`) and TTC faces past index 0 are not embedded; a
  mixed-direction line cannot extract back to source order through an extractor that re-applies the
  UBA (documented, not fixable in the file for Xpdf 4.00).
- **Evidence:** the matrix in [localization-parity.md](localization-parity.md); #142, #54; UI 997
  tests (18 new), PDF 43 (10 new), `pdftotext` samples per script.
- **Impact:** text in Arabic, Hebrew, Devanagari and Thai now draws and prints in the right order;
  what remains is font coverage and the shapes of layout (wrapping, labels, mirrored print).
- **Estimate:** 2–5 h left of the beta slice (font coverage and the RTL print flag); the rest is
  localization work. [localization-parity.md](localization-parity.md)

### 6. Calculation correctness bugs (F)
- **Closed 10-30.** Missing: nothing known. The four this entry tracked have landed: 15-significant-digit
  equality (#57, so `=0.1+0.2=0.3` is TRUE and MATCH and COUNTIF agree), spill updates after
  structural edits and cleared blockers (#133, #139; PRs #148, #157), 3-D span deletion (#140),
  and qualified names after a sheet rename (#134). Iterative calculation for circular references
  landed too (no Options dialog yet, which is §13's).
- **Soak (10-30):** a 400-seed × 200-command release soak found class-level defects the reported bugs
  never showed. Fixed here: a `u16` payload index in the cell store's per-row wide table **wrapped past
  65,535 entries** on a row rewritten thousands of times, so rewriting one cell's text aliased another
  cell's value (a *full* recalc disagreeing with memory is what exposed it; the table now compacts to
  one entry per distinct value its row still uses); a formula that read its own cell was marked
  `#CIRC!` only after the pass, so later formulas in the round read its pre-override number and an
  eight-round cap could leave a spill from one state with cells from another; a `COUNTIF` reading a
  spill the same pass discarded as circular; a defined name reading a discarded spill (a name hides its
  range from the dependency graph, so dynamic formulas and used-range readers are re-evaluated on any
  spill change); and `#SPILL!` after an edit moved the used range. Each has a direct regression test
  that asserts the two paths agree **and** the right value.
- **Soak, save/reload (10-30):** an array entry over more cells than the evaluator lays out left the
  array `#NUM!` while cells inside its declared range kept values that the reader's own array rule
  drops — the entry is refused now (as Excel refuses it), so that state cannot be created; a written
  `defaultColWidth="0"` was silently dropped by the reader, and `home.columnWidth` accepted widths
  past the format's 255-character maximum, which were saved as 1785 px behind the user's back; the
  legacy-array entry now marks every cell it clears as a change, so a formula reading part of the
  range recalculates instead of keeping a value a full recalc disagrees with.
- **Soak, names (10-30):** a table resized over a blank column kept **two columns named `Column3`**
  (the reconciler deduped through a name→column map that cannot hold a duplicate), and the writer's
  documented repair then renamed the later one to `Column32` **at the file boundary** — a rename by a
  rule nothing in memory had applied, which no undo can reach. One function owns the naming rule now and
  the whole header row is reconciled after a structural edit, so model, cells and file cannot disagree.
- **Oracle suite (10-30):** `crates/calc/tests/formula_oracle.rs` runs **595 documented examples over
  275 functions** through the real calc path (`gridcraft_calc::evaluate`), plus dedicated tests for
  real spills, `#SPILL!` unblocking, the 1904 date system, 3-D references and qualified names. Each
  row cites the published example it comes from. It is **documentation-derived, not
  Excel-computed**, and it is labelled that way: it can say the documented behaviour holds, not that
  installed Excel agrees. It found no implementation defect — every disagreement was a
  transcription error resolved by the documented rule. An Excel-computed corpus run stays part of
  gate #1.
- **Impact:** wrong numbers are worse than missing features.
- **Estimate:** 2–5 h, all of it re-running the suite against an Excel-computed corpus once Excel is
  available. [function-parity.md](function-parity.md#formula-language)

### 7. Legacy `.xls` reads everything but CF12; RC4 encryption cannot be written (FF)
- **Landed 10-29 (encryption write):** `file.save`/`file.saveAs`/`file.saveBytes` write **Agile**
  encryption (AES-256-CBC, SHA-512, spin count 100,000, random salts, HMAC-SHA512 integrity, a
  version-3 compound file), and a workbook opened with a password re-encrypts with it on a plain
  `file.save` — the silent loss of a file's protection is fixed. `password: ""` removes protection;
  a password on a format that cannot carry one is refused, never dropped. Standard (RC4/AES-128) and
  certificate encryption are not written.
- **Landed 10-30 (Standard encryption write):** `file.save`/`saveAs`/`saveBytes` take `encryption`
  (`agile` by default, `standard` on request; an unknown name is an error listing both), and a
  workbook opened with a Standard file **re-saves as Standard**, so a save never silently changes the
  form. Standard is [MS-OFFCRYPTO] 2.3.2 — a 224-byte binary header (AES-128, SHA-1, `PROV_RSA_AES`)
  and AES-128-**ECB** over the zero-padded package, with **no integrity field** — not Agile's
  segmented CBC, which is what the earlier sketch assumed and would have produced files our own
  reader refuses.
- **Landed 10-30 (encryption hardening):** keys and salts come from the **operating system's CSPRNG**
  (`getrandom`, native targets only) instead of stretched SipHash seeds; the Agile `dataIntegrity`
  HMAC-SHA-512 is **verified on read** after the password check (both fields decrypted with the
  package key under their own block keys' IVs, HMAC computed with the `keyData` hash over the
  plaintext package, constant-time compare, and the segments must cover the stated package size), so
  a flipped byte, a tampered key or a truncated package is an error; `DecryptError::Corrupted` is a
  distinct failure, so a damaged file no longer reads as a wrong password. The landed writer had
  encrypted those two fields with a password-derived key and one shared block key, which
  [MS-OFFCRYPTO] 2.3.4.14 does not allow — Excel would have rejected our files, and our own verifier
  Excel's; both directions are fixed. The **web build refuses** a password-protected save
  ("needs a secure random source this build does not have") rather than writing predictable salts,
  because `getrandom`'s `wasm_js` backend would add JavaScript glue and the repo is Rust only. An
  independent CFB reader (a dev-only dependency, no shared code) validates the written container at
  every size class and through the DIFAT chain, and found a real defect: the mini FAT's last sector
  was zero-padded, and a zero entry names mini sector 0 (now `FREESECT`).
- **Landed 10-29 (`.xls` styles):** `FONT`, `FORMAT`, `PALETTE`, `XF`, `STYLE` build a real style
  table (family, size, bold/italic/strike/underline, palette colour, number format, fill pattern and
  colours, four borders, alignment/wrap/rotation/indent, protection, named styles), cells pick up
  their XF, and `NOTE` + `TXO`/`CONTINUE` give comments with author and text. Layouts were calibrated
  against an independent BSD-licensed BIFF8 reader over 70 Excel-authored files on the dev machine
  (metadata only; no file or data copied into the repo, per the clean-room rule).
- **Landed 10-30 (`.xls` records):** data validation (`Dv`/`DVal` — seven kinds, eight
  operators, literal and range lists, prompt and error text, the three error styles, dropdown and
  allow-blank flags), conditional formats (`CondFmt`/`CF`/`CFEx` with a full `DXFN`/`DXFN12`
  differential style: number format, font, alignment, borders, pattern, protection), hyperlinks
  (`HLINK`/`HLINKTOOLTIP` through [MS-OSHARED] URL, file and composite monikers with the tooltip) and
  freeze panes (`Pane` + `Window2`, plus the scrolled top-left cell and the active sheet from
  `Window1`). Each is asserted equal to what the `.xlsx` reader produces for the same model, and every
  fixture is a byte array built in code. **Spec-derived and fixture-tested only — no `.xls` corpus and
  no Excel on this machine.** Still refused, with the reason recorded: **CF12** colour
  scales/data bars/icon sets (the specification's diagram and its field text disagree on
  `rgbTemplateParms`' length, so the tail's offset is undetermined — a guessed offset would be
  invention), and BIFF5, whose record layouts are simply **not in [MS-XLS]** (§1.6 specifies BIFF8
  only), so a second reader would be ~1,000–1,500 lines from a source we do not have. An 11-02
  attempt to settle the layout from a **WPS-authored** `.xls` did not help: WPS 12.0 writes its own
  `0x087C`/`0x087D` extension records for a colour scale, data bar and icon set instead of Excel's
  `CF12`, so an **Excel-authored** `.xls` (gate #1) is still the only source that can resolve it.
- **Still missing:** `.xls` conditional formats of the **CF12** kinds (colour scales, data bars, icon
  sets) — everything older is read now; BIFF5 is refused with a clear message; hyperlinks and
  freeze panes; charts, pictures, OLE objects and PivotTables are dropped with a warning; `.xls` is
  never written.
- **Evidence:** #121: Excel 97-2003 (BIFF8) `.xls` files now open (values, formulas, number
  formats, merges, sizes, names); fonts, fills, borders, charts, pictures, comments, conditional
  formats, validation and BIFF5 files are not read, and `.xls` is never written. Password-encrypted
  `.xlsx` workbooks (Agile and Standard encryption) open with their password since #11, but are
  saved without it: GridCraft can't write an encrypted file yet. Encrypted `.xls` (FILEPASS) and
  RC4-encrypted files are still refused with a message.
- **Impact:** common in finance, government and anything older than 2007; encrypted files are
  routine in business.
- **Estimate:** 7–16 h (CF12 3–5 once its tail length is settled, charts/pictures 2–4, RC4 write 1,
  an Excel run over a `.xls` 1–6). [file-format-parity.md](file-format-parity.md)

### 8. PivotTables: calculated items, value/label filters; unverified in Excel (F)
- **Landed 10-30 (slicers and timelines):** a slicer and a timeline are **model objects** now — cache
  field, the PivotTables they filter, item values with their selected flags, name, caption, style,
  column count, timeline unit and anchor — read into the model and written once from it, so
  `insert.slicer` (pivot or table), `insert.timeline`, `slicer.setSelection`, `slicer.setStyle`,
  `slicer.move` and `slicer.delete` drive a real filter: `setSelection` writes the field's
  `PivotFilter` on every PivotTable in the cache list and refreshes, in one undo step, and the
  selection survives save/reopen. Catalog 279 → 280 (94.9 %). Everything GridCraft cannot model (an
  OLAP cache, item properties, an unknown attribute or relationship, a timeline's date range) stays
  byte-identical on the pass-through path, all-or-nothing per kind.
- **Landed 10-30 (placement):** a written slicer or timeline is placed and wired the way [MS-XLSX]
  defines — an anchored `xdr:graphicFrame` naming the view in its sheet's drawing part (read back as
  the anchor), one slicers/timelines part per sheet related from the worksheet and listed in its
  `x14:slicerList`/`x15:timelineRefs`, the caches in the workbook's `slicerCaches`/`timelineCacheRefs`
  — and **table slicers are written** with `x15:tableSlicerCache`, taking the selection from the
  column's AutoFilter where Excel keeps it. Verifying against the schema also corrected a forbidden
  cache→slicer relationship, a non-existent cache child, the workbook extension URIs, the timeline
  content/relationship types and `pivotTable/@tabId` (a `sheetId`, not a sheet index). The writer
  omits the optional `mc:AlternateContent` fallback shape because the specification does not define
  its content, and it refuses a timeline whose field holds no dates (no bounds to write).
- **Landed 10-30 (UI):** a slicer draws on the sheet as Excel draws it — a caption band with a filter
  button and value tiles in the model's column count, selected tiles accent-filled with a check, an
  `N of M` footer and wheel scrolling when the body overflows, and the same eight resize handles as
  charts and pictures. A timeline draws its caption, unit and a range track. Clicking a tile runs
  `slicer.toggle`, the filter button `slicer.clear`, a drag or an arrow nudge `slicer.move` (now with
  `dx`/`dy`, so it no longer has to snap to a cell, and it reaches timelines too), Delete removes it,
  a right-click menu carries delete/select-all/clear and a Slicer Styles submenu, and Tab/Enter move
  between tiles without the grid stealing the keys.
- **Landed 10-30 (timeline ranges):** `PivotTimeline` carries a `TimelineRange`, written as
  `<state><selection startDate endDate>` with `filterType="dateBetween"` and read back exactly, and
  `slicer.setRange` filters the report through the slicer path in one undo step (clamping a range past
  the field's own bounds and saying so). The UI's timeline handles are still not draggable — the model
  can hold the range now, wiring the drag is UI work in the next round. Fidelity note: the pivot
  filters by item labels, so a range covering part of a month shows the whole month; a real
  Excel-authored timeline with non-zero refresh versions stays on the kept path.
- **Landed 10-30 (names and a report-side filter):** a table slicer follows its table's rename and
  its column's header rename (`Slicer.column`, and `caption` while it still shows the column's
  name), so a rename can neither empty its tile list nor leave its filter pointing at a column that
  no longer exists;
  `insert.slicer` now applies a `caption` the caller passes (it was documented but dropped). And a
  field selection changed from the PivotTable itself (`pivot.filter`, and any update that rewrites
  the report's filters) is mirrored onto a slicer connected to that field **when every connected
  report agrees on the selection and every label names a tile**. A grouped date label or a
  number-format label has no tile, so the widget keeps its state rather than showing a filter the
  model does not hold; a slicer whose reports disagree is left alone; timelines keep their own range.
- **Landed 11-02 (timeline drag):** a timeline's selected range is now edited on the sheet. The
  track is the field's own date axis (`cmd::slicer::timeline_axis` scans the report's source once
  per document revision, and the grid caches it), the range draws as a band with its own handles,
  and dragging a handle resizes it while dragging the band slides it without changing its length.
  Both ends snap to whole days, stay inside the axis and never cross; each drag runs
  `slicer.setRange` and coalesces into one undo step, and Esc puts the range back. A timeline whose
  report is gone has no axis, so its handles stay decoration rather than inventing a date.
- **Landed 11-02 (a filter's area):** a field can sit in the Filters area and on an axis at once (a
  file says so with a `pageFields` entry), but a filter carried no area, so the writer and the
  refresh guessed it from whether the field was in Rows/Columns — a page filter on an axis field
  collapsed into hidden items and lost its row. `PivotFilter` now records its area (`Auto` for
  documents written before it existed, `Page` for the Filters area, `Axis` for hidden items), the
  reader sets it (`pageFields` vs `item/@h`), the writer and the refresh honour it, and moving a
  filtered field onto an axis turns a page filter into an axis filter.
- **Landed 11-02 (GETPIVOTDATA):** `GETPIVOTDATA(data_field, pivot_table, field, item…)` reads the
  cell the pairs name out of the report's own labels — so it works on a file opened without a
  refresh, where no PivotTable cache exists to recompute from. Text matches case-insensitively, a
  number by value or by its formatted text, `"Grand Total"` names either total axis, and no pairs
  returns the grand total. The formula depends on the report's whole written area, so a refresh
  recalculates it; the function is in the Insert Function list. Each layout is covered (compact
  indentation, outline/tabular columns, Σ Values in Rows, subtotals on top or off) and the lookup
  survives save/reopen; the file-format side is unchanged (it reads cells the report already
  wrote). Differences kept: a report whose grand totals are turned off has no total cell to return
  (`#REF!`; Excel reads its cache), and a subtotal drawn on its own line below the group is named
  by the parent item only in the layouts that put the subtotal on the item's row.
- **Landed 11-02 (number grouping):** a number field can be grouped the way Excel's Group Field
  dialog does it — `pivot.group {start, end, interval}` on a field in Rows or Columns (start/end
  default to the field's own extent) — and the report buckets its values in half-open steps:
  whole steps read `0-9`, `10-19`, a fractional one shows the half-open end, and values outside
  the bounds become `<start`/`>end` items, sorted outermost. The model holds the range
  (`PivotField.number_group`), the cache writes [MS-XLSX] `rangePr startNum`/`endNum`/
  `groupInterval` with `groupBy="range"` and one caption per group item, and a saved report reads
  the range back and rebuilds the same labels on refresh. `pivot.fields` reports it; a range past
  100,000 groups, a non-positive interval, a reversed range or a text field is refused by name.
  **Never opened in Excel** — the caption question above is gate #1.
- **Landed 11-02 (label and value filters):** `pivot.filterByLabel` keeps the items of a row or
  column field whose caption matches — equals, not equals, begins/ends with, contains, does not
  contain, the four comparisons and between/not between, case-insensitively — and
  `pivot.filterByValue` keeps the items whose aggregate of a chosen value field (sum, count,
  average… by the value field's own function) passes the same comparisons. A value filter
  aggregates over the records the report's other filters keep, across the column axis. Both are
  evaluated when set and stored as the field's hidden items (`item/@h`), the representation the
  file already uses for an axis filter and the slicer path already drives, so the report, a
  save/reopen, a refresh and Excel all agree on the visible items; the rule itself is kept in the
  model (`PivotTable.label_filters`/`value_filters`, reported by `pivot.fields`) and not written
  as Excel's `<filters>` element — a refresh after new data keeps the item set instead of
  re-evaluating the rule, and Excel's field dropdown will show hidden items rather than "filtered
  by value". A rule and an item selection on one field replace each other; a calculated field as
  the measure is refused by name. Guarded: between needs both bounds, and the field must be on an
  axis.
- **Missing:** calculated items and data-model pivots; the PivotTable gate's Excel verification. (`insert.pivotChart` landed 10-11 and is in the catalog, so PivotCharts are
  no longer part of this gap.) The "Generate GetPivotData" click option (Excel writes a
  `GETPIVOTDATA` formula when a pivot cell is clicked) is not wired into the grid yet.
- **Evidence:** `parity-checklist.md` missing `insert.pivotChart`, `data.relationships`; the slicer
  evidence is `crates/model/src/slicer.rs`, `crates/xlsx/src/slicer.rs` (the modelled-vs-kept contract
  is documented at the top), `slicer_tests.rs`, engine `cmd/slicer.rs`. The written slicer part has
  never been opened in Excel — gate #1. The number-grouping and hidden-item encodings do pass
  [`cargo xtask oracle`](oracle.md) on the `pivot` fixture (openpyxl reads every cell back; WPS 12.0
  opens, recalculates and re-saves the package), which proves the XML is well formed and tolerated,
  not that Excel accepts it.
- **Impact:** pivots are the analyst's main tool.
- **Estimate:** 2–4 h for beta (18–27 h full).

### 9. Keyboard-first users: key tips, shortcuts, context menus (U)
- **Landed (10-29):** key tips now cover the whole ribbon — 417 sequences across every tab, group
  and button (contextual tabs switch with their context), served by one JSON source of truth
  (`crates/ui-egui/src/keytips/keytips.json`) and a generated table kept in step by
  `cargo xtask keytips --check`. `Alt` shows the badges, a typed sequence activates, `Esc` cancels
  at any depth. Tests enforce sequence length ≤ 4, per-scope uniqueness, no shadowing (two-way
  prefix), tab reachability, that every badge resolves to a catalog command, and one end-to-end
  test per sequence (419). `Shift+F10` opens the context menu at the selection for cells and for
  row/column headers, navigable with arrows/Home/End/Enter/Space/Esc; the grid keeps focus and the
  selection, and Space cannot start an edit behind the menu. Six Excel shortcuts were added on
  commands that already existed (`Alt+;`, `Ctrl+Shift+O`, `Ctrl+F2`, `Ctrl+Shift+F12`, `Alt+F5`,
  `Ctrl+Alt+F5`). The full Excel → command-id → status audit is in [ui-parity.md](ui-parity.md).
- **Still missing:** the Menu key itself (egui-winit 0.36 drops `KeyCode::ContextMenu` before the app
  and egui has no `Key` variant for it), context menus for charts/pictures/shapes from the keyboard,
  QAT/title bar/Backstage badges, the View tab's two combo boxes, and per-item badges in menus whose
  items share one command id. Remaining Excel shortcuts map to features we do not have yet (Quick
  Analysis, extend mode, pane cycling, sheet groups, Paste Name, Repeat, outline symbols, formula-bar
  expand, window keys).
- **Estimate:** 0–6 h of the 12–20 left (the missing extras above). [ui-parity.md](ui-parity.md)

### 10. Crashes and freezes reported from the field (S)
- **Evidence:** #120 (Intel UHD crash on Windows), #117 (error on close, macOS), #166 (freeze opening
  from Finder; PRs #164, #167), #76 (flashing on Windows 11), #130 (caught panic in table styles).
- **Landed (10-26): a 100-seed command-sequence soak.** 200 commands a seed in release build, every
  invariant checked per step (`crates/engine/tests/command_sequence_fuzz.rs`). Found and fixed:
  Merge Across of whole columns ran 1,048,576 merges and then re-scanned the sheet per row (a
  freeze); a table's total row could grow into the next table; `data.subtotal` took a dozen undo
  steps per group instead of one; a table column named with `!` was saved as a sheet-qualified
  formula and came back changed. The harness now ends an episode before the 128 MB undo budget
  trims it, traces steps with `GRIDCRAFT_FUZZ_TRACE`, and names the differing sheet field.
- **Fixed (10-26, same soak):** an array that spills over its own source (`=TRANSPOSE(J23:L28)`
  in J24) was laid out anyway, with some elements `#CIRC!`; it is now `#CIRC!` at the anchor, as a
  formula that reads a cell it writes. And two arrays whose areas overlap kept their cells across
  F9 instead of being re-laid out in row-major order (`=SEQUENCE(7,4)` at K7 beside `=SEQUENCE(6,5)`
  at L2).
- **Fixed (10-27): a 500-seed soak is green.** 200 commands a seed (100,000 commands, release,
  49 minutes) over the three samples, every invariant checked per step. The three open seeds from
  the 100-seed run are gone: on read, a dynamic anchor's `ref` is rebuilt into `spill_ranges` and
  the cached interior values become the spill map, not cells that block the array, so a reload
  keeps the array that won a contested area; F9 prefers the anchors that actually spill, and an
  owner that grows back after a spill it reads is laid out later reclaims its cells. `#SPILL!`/
  `#CALC!` cached as `#VALUE!` is compared as what the file can store, not as a changed value.
  The `_sc_*` names (Watch Window list, custom views, scripts, scenarios) are view state and
  formulas can no longer resolve them. Further seeds found and fixed: a formula that reads its own
  cell through a range and swallows the `#CIRC!` (`COUNTIF` over its own column) is `#CIRC!`
  instead of a number that depended on evaluation order; a filter installed over a selection that
  covers a table is narrowed to the table's filter range, so its criteria reach the table part and
  SUBTOTAL still skips the hidden rows after opening; a multi-cell legacy array that cannot spill
  still refuses edits of its cells; Insert/Delete Cells that would cut a merged area in two is
  refused; the harness tells a byte-capped (trimmed) undo history from a real undo bug, and
  ignores view-only freeze in undo comparisons.
- **Remaining caveat:** the file format cannot cache `#SPILL!`/`#CALC!` (GridCraft writes
  `#VALUE!`, following the claim that Excel refuses those error codes; **needs verification on a
  licensed Excel**) or `#CIRC!` (written as 0). A *literal* `#SPILL!` pasted as a value therefore
  comes back `#VALUE!`; formulas and spilled values are recalculated and agree.
- **Re-run 11-02 after the timeline/secondary-axis work:** 100 seeds × 200 steps (20,000 commands)
  in the debug build, 68.3 s wall, **0 invariant failures, 0 slow seeds** (`soak: 100 seeds × 200
  steps, 68.3 s wall, 0 invariant failures, 0 slow seeds, slowest seed 7 (6620 ms)`). The three
  samples were exercised against undo/redo, XLSX round-trip and full recalc.
- **Re-run 11-02 after the three PivotTable changes (GETPIVOTDATA, number grouping, label/value
  filters):** 100 seeds × 200 steps at commit `3574f5d2`, debug test profile, **131 s wall, 3 tests
  passed, 0 invariant failures**. Seeds are only valid at the commit that generated them, so this
  line records that commit.
  
- **Next:** the trust track's soak is closed; the 100k-dependent gate is met, independent formulas
  recalculate on up to eight threads (20–25 ms at 100k, 208–277 ms at 1M) and the bulk data
  commands are bounded (12M cells; Remove Duplicates 1M×10 in 12 s, Sort in 3.2 s, gaps §2). Cold
  paging (M19) is the next large-workbook task. Replays and details in `plan/HANDOVER.md`. The
  Windows GPU/driver reports (#120, #76) and the macOS close/open reports (#117, #166) still need
  the hardware they were reported on; they cannot be closed here.
- **Estimate:** 12–16 h for this soak and its fixes; done.
## After beta

### 11. No localization at all (L)
- Catalog: ribbon-only in five languages (#31, #104, #82; ~12% of strings each), and 943 strings
  (~43%) in Japanese (10-17) and Brazilian Portuguese, including the dialog labels, messages, File
  view and menus. The remaining dialogs, menus and messages are English; en-US function names and
  separators (#45); German in PR #163. **Estimate:**
  100–165 h including #5. [localization-parity.md](localization-parity.md)

### 12. Get & Transform, external data, data model (F)
- No Power Query editor, connections, refresh, From Web/database/folder sources, relationships or
  data model (`data.fromWeb`, `data.relationships` missing). **Estimate:** 60–100 h.

### 13. Analysis tools: Solver, Analysis ToolPak, Forecast Sheet (F)
- `data.solver`, `data.analysisToolPak` missing. Forecast Sheet (`data.forecastSheet`) and the
  FORECAST.ETS family are in, including Forecast Start (hindcast, #785). **Estimate:** 15–25 h.
- Solver (`data.solver`, #327) has Simplex LP with integer/binary branch and bound, a local
  nonlinear method (augmented Lagrangian, not GRG), the Evolutionary method, and Answer and
  Limits reports. Report refusal leaves the solution, saved model and history unchanged; a
  successful solve and its reports share one Undo step (#1050). AllDifferent constraints,
  the Solver Options dialog, the Sensitivity and Population reports and Load/Save Model remain.
  The Analysis ToolPak
  (`data.analysisToolPak`) has 13 of 19 tools. Forecast Sheet (`data.forecastSheet`) and the
  FORECAST.ETS family are in; the sheet has no Forecast Start (hindcast) option yet. **Estimate:** 12–20 h.

### 14. Page Layout view and print polish (U, F)
- Page Layout view landed (#329): paper view with rulers and in-place header/footer editing.
  Margins are dragged on the rulers (#792). Remaining: pictures in headers/footers, Different First Page / Odd & Even pages. Desktop now submits directly through craft-print with printer choice, copies, duplex and grayscale; browser printing stays available. Print ranges and driver tray/options UI remain. **Estimate:** 5–10 h.

### 15. Remaining functions and an oracle suite (F)
- **9 missing by name** (514 of Microsoft's 523 present, 505 working): DETECTLANGUAGE, TRANSLATE,
  IMAGE, RTD, STOCKHISTORY, CALL, REGISTER.ID, COPILOT and PY; nine stubs (7 CUBE, WEBSERVICE,
  FILTERXML). Measured 11-02 with `gridcraft-cli functions --json` (515 specs, one of them
  USDOLLAR, which is not on Microsoft's list). No Excel-computed oracle corpus; #137, #138.
  **Estimate:** 13–22 h. [function-parity.md](function-parity.md)

### 16. Collaboration and review (F)
- No co-authoring, Show Changes, comment navigation, notes ↔ comments
  conversion; cloud storage requested (#86). **Estimate:** 35–55 h (co-authoring scope needs the owner).

### 17. Insert objects (F)
- Ink strokes now survive XLSX save/reopen as editable polylines. Inserted icons still lose their
  identity/artwork on save (#551); native Office InkML import is not implemented.
- SmartArt, WordArt, equations, signature line, objects, 3-D models, screenshot; 8 shape kinds vs
  ~170; pictures in cells (#61, PR #105). **Estimate:** 25–40 h.

### 18. Accessibility (U)
- **Landed 11-02 (alt text + checker):** charts and shapes carry alternative text now — the model
  (`Chart::alt`, `Shape::alt`), the file (`xdr:cNvPr/@descr`, escaped, read back on charts, kept
  charts, pictures and shapes), the command `object.setAltText` and a check that notices when it
  is missing. Review ▸ Check Accessibility reports charts, pictures, shapes with no text of their
  own and pictures in cells without alt text, tables without a header row, merged cells and sheets
  still on their default name, each with its sheet and object/range, and the dialog **goes to**
  each one and takes the alt text inline.
- **Landed 11-02 (the grid is walkable):** while a screen reader is attached the visible cells are
  an **AccessKit grid** — one `GridCell` node per visible cell, with the displayed value as its
  value, its address (and formula) as its description, its row and column indexes, and the active
  cell marked selected and named as the grid's active descendant; the grid itself is labelled with
  the sheet, says which cell is active and how many are selected, and carries bounds of its own,
  with a `ColumnHeader`/`RowHeader` node per visible header (its letters or number and its index). Frozen rows and columns are
  included and the tree follows scrolling; nothing is built when no reader is attached (one branch
  a frame) and the walk is capped at 2,000 cells a frame, so a reader scrolls for the rest as a
  sighted user does.
- **Landed 11-02 (high contrast):** View ▸ Show ▸ **High Contrast** (`view.highContrast`, keytip
  WVI — GridCraft's own, Excel takes it from the OS) turns the chrome into a black, white and
  yellow palette: black window, ribbon, tabs and status bar with white text and yellow separators,
  a yellow selection border and header highlight, cyan accents, and a white sheet page that keeps
  its own black text (as Excel's high-contrast themes do — the document is not the chrome). egui's
  own widgets go dark with it so menus and dialogs stay readable. The preference is kept across
  restarts (`ui.json`). Remaining: an accessibility entry in the status bar. **Estimate:** 1–2 h. [ui-parity.md](ui-parity.md#accessibility)

### 19. XLSB/ODS beyond values; minor formats (FF)
- XLSB and ODS import values only, no write; no `.prn`, `.dif`, `.slk`, XML Spreadsheet 2003; CSV
  writes raw instead of displayed values; templates saved with the workbook content type.
  **Estimate:** 15–23 h.

### 20. Multiple windows; pen pressure (H)
- One window (#162, PR #171); no pressure or tilt in ink. **Estimate:** 10–14 h.
  [hardware-parity.md](hardware-parity.md)

### 21. Automation and add-ins (F)
- VBA out of scope (dropped on open, #6); no Office Scripts compatibility; no plugin or custom
  function API. **Estimate:** 25–40 h plus an owner decision.

### 22. AI features (F)
- No in-app assistant, natural-language formulas or Analyze Data; agent control via MCP is
  already strong. **Estimate:** 30–50 h; model choice needs the owner.

### 23. Mobile (platform)
- No iOS, iPadOS or Android layout. **Estimate:** 30–60 h.

### 24. Fonts and look (U)
- Font weights beyond regular/bold (#168), system font list (#4), Calibri metrics (#29), macOS
  chrome (#169, #170), Linux title bar (#54), icon consistency across apps (#173). **Estimate:** 7–12 h.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-02 | minor | §8: the `pivot` oracle fixture now carries a grouped numeric field (`rangePr` + 13 group captions) and a value filter's hidden items, and both probes pass (openpyxl readback; WPS opens, recalculates, re-saves) — well-formedness evidence, not the Excel check |
| 2026-11-02 | minor | §1: the corpus half cannot start on this machine — no corpus URL in the repo, no `craftrules` checkout, no network (github.com timed out) — so it is recorded instead of attempted; §10: a fresh 100 seeds × 200 steps soak at `3574f5d2` (after the three PivotTable changes) is green, 131 s, 0 failures |
| 2026-11-02 | minor | §8: label and value filters landed (`pivot.filterByLabel`/`pivot.filterByValue`, evaluated when set and stored as the field's hidden items, so the report/reopen/refresh agree; the rule stays in the model and is not written as Excel's `<filters>`); calculated items and the Excel check are what remain |
| 2026-11-02 | minor | §8: number grouping landed (`PivotNumberGroup`, `pivot.group` start/end/interval, `<start`/`>end` items, `rangePr` + `groupBy="range"` in the cache, round-trips through save/reopen); calculated items, value/label filters and the Excel check are what remain |
| 2026-11-02 | minor | §8: GETPIVOTDATA reads a report cell by its field items (every layout, Σ Values in Rows, subtotals, page filters, numbers and grand totals), depends on the report's written area so a refresh recalculates it, and is in the Insert Function list; calculated items, value/label filters and number grouping are what remain |
| 2026-11-02 | minor | 100 seeds × 200 steps (20,000 commands) command-sequence soak green in 68.3 s, 0 invariant failures; the hardware reports still need their machines. A WPS-authored `.xls` writes proprietary `0x087C`/`0x087D` records, not Excel `CF12`, so CF12 still needs gate #1 |
| 2026-11-02 | minor | `chart.setSeriesAxis` moves a series to the secondary axis (and optionally changes its kind); the Format pane has the checkbox; combo series are read back in `c:order`, not group order |
| 2026-10-30 | minor | §8: a table slicer follows a table and column rename, `insert.slicer` applies `caption`, and a report-side `pivot.filter` mirrors onto a connected slicer; corrected the stale "widget UI / timeline range / PivotCharts missing" list (`insert.pivotChart` landed 10-11) and dropped the stale "table slicer is not written" line |
| 2026-10-28 | minor | §2: a finished band drops its text pointer index (rebuilt on the first edit); the 100M-cell export falls from 2,411 to 1,911 MiB live (texts 640 → 301, number tables 679 → 519) and a text past the u16 slots keeps its own slot |
| 2026-10-28 | minor | §2: bulk changes scan blocked arrays once for the union of the changed cells and data commands cap at 12M cells: Remove Duplicates 1M×10 91 → 12 s, Sort 3.2 s, Fill Down 1.45 s; 1M×20 refused in 1.3 s |
| 2026-10-27 | minor | §2: independent formulas recalculate on up to eight threads (leaves wave by wave; arrays/volatile/iterative sequential): 100k end-to-end 65–75 → 20–25 ms, 1M calc 517–843 → 208–277 ms; §10 next step is bounded whole-sheet work and paging |
| 2026-10-27 | minor | §2: the 100k-dependent edit gate is met single-threaded (245 → 65–75 ms; full recalculation 281–289 → 134–147 ms) by reading formulas and legacy ranges from the graph, one result slot, row-major value writes, shared row extras, cached AST byte counts and a diff-first array check; §10 next step is multi-threaded recalc |
| 2026-10-26 | minor | §10: self-spilling arrays are `#CIRC!` and a contested spill keeps its owner across F9; three seeds remain, two of them the file side of spilled arrays |
| 2026-10-26 | minor | §10: 100-seed soak; Merge Across freeze, total-row overlap, Subtotal undo depth and a `!` column name fixed; four dynamic-array conflict seeds open with replays |
| 2026-10-26 | minor | §2: whole-column references are already clipped (21.5 ms for `SUM(A:A)` over a million cells); a 100k-dependent edit measures 223–251 ms; the 159 remaining cells are Excel's stale cache, not a difference |
| 2026-10-26 | minor | §2: the stripes' report numbers and a 100M-cell measurement (2.4 GB on a unique-key export against ~490 MiB extrapolated at report density); `packed_breakdown` no longer counts two dictionaries twice |
| 2026-10-20 | minor | §11: the Japanese catalog covers the dialogs, messages and menus — 943 keys (~43%), matching the pt-BR coverage; a test keeps the two catalogs in step |
| 2026-10-17 | minor | §6: the five reports it tracked are closed and their PRs merged (#57, #133, #139, #140, #134; PRs #148, #155, #156, #157); the entry stays open for §15's oracle suite |
| 2026-10-15 | minor | §13: Solver prepares reports before committing the solution, model and report sheets together; report refusal preserves the workbook and Undo/Redo (#1050) |
| 2026-10-13 | trivial | §4: multi-level category axes landed |
| 2026-10-12 | minor | §9: next/previous sheet commands retain the current visible sheet and selection when there is no visible destination |
| 2026-10-12 | minor | rows auto-fit oversized fonts on open and whole-row/column formatting (#833) |
| 2026-10-12 | minor | Page Layout view: dragging margins on the rulers (#792) removed from the §14 remainder |
| 2026-10-12 | minor | §13: Forecast Sheet Forecast Start (hindcast) landed (#785) |
| 2026-10-12 | minor | §17: ink geometry survives XLSX saves; the icon portion of #551 remains |
| 2026-10-12 | minor | §8: values on rows (Σ Values in Rows, `dataOnRows`) laid out, moved and round-tripped |
| 2026-10-12 | minor | Desktop File > Print now discovers printers and submits PDF jobs through pinned craft-print with printer choice, copies, duplex and grayscale; browser printing and PDF export remain available. Remaining print parity includes page ranges and driver-specific tray/options UI; physical printer fidelity needs hardware validation. |
| 2026-10-11 | minor | §6: iterative calculation for circular references landed (Maximum Iterations, Maximum Change) |
| 2026-10-11 | minor | §15: trim-reference operators (`A1:.E10`, `A1.:E10`, `A1.:.E10`) parse and evaluate |
| 2026-10-11 | minor | §4: trendlines (six types, forecast, equation and R²) read, written and drawn |
| 2026-10-11 | minor | §7: password-encrypted workbooks open with their password; saving with one remains (#11) |
| 2026-10-11 | minor | §16: comment navigation, Show Comments, Show All Notes and Convert to Comments landed (#281) |
| 2026-10-11 | minor | §7: `.xls` (BIFF8) import of values, formulas and number formats (#121) |
| 2026-10-11 | minor | §3: XLSX pass-through keeps custom XML, external links, connections, query tables, printer settings, controls and OLE objects (#330, #324) |
| 2026-10-11 | minor | §13: Forecast Sheet landed (#322) |
| 2026-10-11 | minor | §15: ODDFPRICE, ODDFYIELD, ODDLPRICE, ODDLYIELD added (18 functions missing) |
| 2026-10-11 | minor | §15: PERCENTOF and EUROCONVERT added (20 functions missing) |
| 2026-10-11 | minor | §15: BAHTTEXT added (21 functions missing) |
| 2026-10-11 | minor | §15: TRIMRANGE added (21 functions missing; trim-reference operators still missing) |
| 2026-10-11 | minor | §16: Allow Edit Ranges landed (#307) |
| 2026-10-11 | minor | §8: calculated fields added |
| 2026-10-11 | minor | §8: PivotCharts added |
| 2026-10-11 | minor | §13, §15: FORECAST.ETS family landed (#314) |
| 2026-10-10 | minor | Updated for the 10-10 merges (#17, #31, #38, #62, #82, #104, #116) and the new chart resize bug (#190) |
| 2026-10-10 | minor | Noted that no gap blocks the alpha gate |
| 2026-10-10 | major | Created: 24 ranked gaps with evidence, from the full re-measure and the 60 open issues |
