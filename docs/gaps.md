# Gaps

> **Last reviewed:** 2026-10-11 · **Last updated:** 2026-10-26 · **Change:** minor (§2: 100 million cells measured; §10: fuzz soak, two spill rules fixed, three seeds open) · **Target:** Microsoft Excel (Microsoft 365)

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
  class of bug.
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
- **Missing:** multi-threaded recalc; 100k-dependent edit under 100 ms (was ~0.4 s); whole-sheet
  operations bounded (#152, #151); memory proportional to the cells, never the file (the 1.28 GB,
  220k-cell workbook lives in 237 MiB), but no cold paging yet — the stripes are what makes a band
  one copy to page out; the per-band value dictionaries still cost full price when values do not
  repeat;
  structural edits still shift every cell below them; whole-column references are already clipped
  (`SUM(A:A)` folds a million numeric cells in 21.5 ms, a ten-row used range in 0.0 ms); a
  100k-dependent edit costs 223–251 ms against the 100 ms gate (a full recalculation is 267 ms,
  single-threaded). The 159 cells that still differ from the cached values of the 1.28 GB report are
  Excel's stale cache — 276 cells of the same `IFERROR(XLOOKUP(...))` shape in the same workbook
  cache `#N/A` — not a calculation difference.
- **Evidence:** `crates/xlsx/src/sheet_read.rs`, `crates/model/src/store.rs`, `docs/roadmap.md`
  (M17–M21), and the harness kept beside the fork (`_gridcraft-bench/`).
- **Impact:** finance and data users routinely have big workbooks.
- **Estimate:** 20–30 h for the rest of the beta gate (recalc threading, edit latency, bounded
  whole-sheet work); the big-data-shaped work beyond it is planned in
  [`roadmap.md`](roadmap.md#large-workbooks-what-big-data-engineering-offers-a-spreadsheet).

### 3. Some unmodelled XLSX content is dropped on save (FF)
- **Missing:** slicers and their caches, timelines, the data model, query tables attached to
  tables, unmodelled `extLst` entries, chart details. Pass-through (#330) keeps custom XML,
  external links, connections, sheet query tables, printer settings, form and ActiveX controls,
  OLE objects and sheet backgrounds; never checked in Excel (repair prompts unknown).
- **Evidence:** `crates/xlsx/src/passthrough.rs` (what is kept, and why the rest isn't).
- **Impact:** open-edit-save still simplifies workbooks with slicers, timelines or Power Pivot.
- **Estimate:** 6–10 h. [file-format-parity.md](file-format-parity.md)

### 4. Charts lose options on round trip; shallow chart model (F, FF)
- **Missing:** axis scaling, label options, error bars, per-point formatting.
- **Evidence:** `Chart` holds only title, legend position, two flags and axis titles
  (`crates/model/src/features.rs`); PRs #158, #161 open; charts can't be resized (#190). Smooth lines (#62), Switch Row/Column (#116) and trendlines (read, written, drawn) landed. Multi-level category axes (year beside month) are read, drawn and written back.
  (`crates/model/src/features.rs`); all 2-D kinds survive XLSX round trip (#158, #161); charts
  can't be resized (#190). Smooth lines (#62), Switch Row/Column (#116) and trendlines (read,
  written, drawn) landed.
- **Impact:** charts are in most business workbooks.
- **Estimate:** 25–35 h for beta (40–60 h full). [chart-parity.md](chart-parity.md)

### 5. Text in non-Latin scripts doesn't render or export (L)
- **Missing:** CJK font fallback, complex shaping (Arabic, Devanagari, Thai), bidi, non-Latin PDF.
- **Evidence:** system-font fallback for CJK, Thai and Hebrew landed in #38 (closed #32); #142
  (Chinese text) awaits confirmation; #54 (symbols as boxes on Fedora); no shaping for Arabic or
  Devanagari; PDF writer uses 14 standard fonts only.
- **Impact:** blocks most of the world's users even with an English UI.
- **Estimate:** 12–20 h. [localization-parity.md](localization-parity.md)

### 6. Calculation correctness bugs (F)
- **Missing:** nothing known. The four this entry tracked have landed: 15-significant-digit
  equality (#57, so `=0.1+0.2=0.3` is TRUE and MATCH and COUNTIF agree), spill updates after
  structural edits and cleared blockers (#133, #139; PRs #148, #157), 3-D span deletion (#140),
  and qualified names after a sheet rename (#134). Iterative calculation for circular references
  landed too (no Options dialog yet, which is §13's).
- **Impact:** wrong numbers are worse than missing features, so this entry is kept until the
  oracle suite of §15 can say the category is clear rather than only these five reports being so.
- **Estimate:** — [function-parity.md](function-parity.md#formula-language)

### 7. Legacy `.xls` only partly read; encrypted workbooks can't be saved with a password (FF)
- **Evidence:** #121: Excel 97-2003 (BIFF8) `.xls` files now open (values, formulas, number
  formats, merges, sizes, names); fonts, fills, borders, charts, pictures, comments, conditional
  formats, validation and BIFF5 files are not read, and `.xls` is never written. Password-encrypted
  `.xlsx` workbooks (Agile and Standard encryption) open with their password since #11, but are
  saved without it: GridCraft can't write an encrypted file yet. Encrypted `.xls` (FILEPASS) and
  RC4-encrypted files are still refused with a message.
- **Impact:** common in finance, government and anything older than 2007; encrypted files are
  routine in business.
- **Estimate:** 14–21 h (`.xls` styles and objects 10–15, encryption 4–6). [file-format-parity.md](file-format-parity.md)

### 8. PivotTables: no slicers, calculated items, PivotCharts; unverified in Excel (F)
- **Evidence:** `parity-checklist.md` missing `insert.slicer`, `insert.timeline`,
  `insert.pivotChart`, `table.insertSlicer`; calculated items and GETPIVOTDATA missing (calculated fields
  and values on rows done).
- **Impact:** pivots are the analyst's main tool.
- **Estimate:** 20–30 h for beta (25–40 h full).

### 9. Keyboard-first users: key tips, shortcuts, context menus (U)
- Next/previous sheet navigation now skips Hidden and VeryHidden worksheets without activating a
  hidden sheet at the workbook boundary. Other keyboard gaps below remain.
- **Evidence:** #33, #49 (key tips: initial Home-tab support landed in #43 on 10-10, the rest of the ribbon not yet), #110, #28 (selection),
  59 command shortcuts vs Excel's 200+.
- **Estimate:** 12–20 h. [ui-parity.md](ui-parity.md)

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
- **Open (3 of 100 seeds), all the same file-format limit:** spilled arrays are derived state, and
  opening a file clears them and recalculates every dynamic anchor, so who won a contested area and
  which spilled element was `#CIRC!` are decided again — a blocked anchor is cached as `#VALUE!`
  (the writer maps `#SPILL!`/`#CALC!`, which Excel refuses in cached values), so the file cannot say
  it either. One seed is unrelated: `=SUM(_sc_watch)` over the Watch Window's sheet-scoped name is
  `#VALUE!` in memory and `#NAME?` after F9. Replays in `plan/HANDOVER.md`.
- **Next for this entry:** reconstruct a dynamic array's spill on read from the anchor's `ref` and
  the interior values instead of clearing and recalculating it on open (4–6 h), then the name seed.
  Details and replays in `plan/HANDOVER.md`.
- **Estimate:** 8–12 h plus an autosave/recovery soak; 4–6 h of it is the file side of spilled
  arrays.

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
- 18 missing, 9 stubs; no Excel-computed oracle corpus; #137, #138. **Estimate:** 16–26 h.
- 20 missing, 9 stubs; no Excel-computed oracle corpus; #137, #138. **Estimate:** 17–29 h.
- 21 missing, 9 stubs; no Excel-computed oracle corpus; #137, #138. **Estimate:** 18–30 h.
- 18 missing, 9 stubs; no Excel-computed oracle corpus; #137, #138. **Estimate:** 18–30 h.
  [function-parity.md](function-parity.md)

### 16. Collaboration and review (F)
- No co-authoring, Show Changes, comment navigation, notes ↔ comments
  conversion; cloud storage requested (#86). **Estimate:** 35–55 h (co-authoring scope needs the owner).

### 17. Insert objects (F)
- Ink strokes now survive XLSX save/reopen as editable polylines. Inserted icons still lose their
  identity/artwork on save (#551); native Office InkML import is not implemented.
- SmartArt, WordArt, equations, signature line, objects, 3-D models, screenshot; 8 shape kinds vs
  ~170; pictures in cells (#61, PR #105). **Estimate:** 25–40 h.

### 18. Accessibility (U)
- AccessKit on, but no accessible grid/ribbon tree, no high contrast, no checker. **Estimate:** 12–18 h.
  [ui-parity.md](ui-parity.md#accessibility)

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
