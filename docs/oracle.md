# The foreign-implementation oracle

> **Last reviewed:** 2026-11-03 · **Status:** Excel 16 is activated and is the gate probe (open,
> refresh every PivotTable, recalculate, save, re-read). The real-world corpus half of gate #1 ran
> on 11-03 on a local 3,084-workbook folder (0 save/reopen mismatches, Excel 16-identical copies);
> the pinned corpus remains the CI artefact — see [gaps.md §1](gaps.md).

GridCraft's target consumer is Microsoft Excel, and the one question a self round-trip cannot answer
is *"does somebody else's reader accept this package?"*. `cargo xtask oracle` answers it with three
probes that are **not ours**: openpyxl (a foreign parser), Excel 16 and WPS 12.0 (independent
applications that open, recalculate and save a copy).

```text
cargo xtask oracle                    # all fixtures, every available probe
cargo xtask oracle --fixture=pivot    # one fixture (sales | budget | grades | features | pivot)
cargo xtask oracle --no-app           # foreign reader only (no COM application needed)
cargo xtask oracle --keep             # keep the .xlsx artifacts in target/oracle/
cargo xtask corpus <dir>              # open every .xlsx in a directory, save/reopen a sample,
                                      # read it back with openpyxl (never writes into <dir>)
cargo xtask corpus <dir> --keep <out> # the same, keeping saved copies + saved-copies.tsv for Excel
```

It is a development tool, not part of `cargo xtask ci`: it needs `python` with `openpyxl`, and the
application probe needs Windows with Excel or WPS installed. Missing prerequisites are reported as
`SKIPPED`, never as a pass. The owner's manual checks (what the probes cannot compare) are in
[excel-checks.md](excel-checks.md).

## The probes

**readback — openpyxl, a foreign reader.** The package must open in a parser we did not write, and
the cells, number formats, merges, tables and their columns, conditional-format / validation / chart
/ image counts and defined names openpyxl sees must match the model that was saved. A part whose
relationships, content types or markup are wrong in a way only a strict consumer notices fails here.
The probe also fails if openpyxl *refuses* the package.

**app — an independent spreadsheet application.** Excel 16 (`Excel.Application`) and WPS 12.0
(`KET.Application`) each open the file, refresh every PivotTable, recalculate and save a copy;
GridCraft then reads that copy back and every value must survive. Excel is the gate probe; WPS is
the second opinion (and the only one on a machine without Excel). Only Excel's localized PivotTable
chrome (`Row Labels`, `Grand Total`) is tolerated as a text difference.

**The package validator** (`gridcraft_xlsx::testing::check_package`, run on every package the test
suite saves) gained the fifth repair-prompt cause: a part that uses a namespace prefix it never
declares. That one is not a heuristic — a strict XML parser rejects the part outright.

## Fixtures

| Fixture | What it puts in front of a foreign reader |
|---|---|
| `sales` | a table with 8 columns, 2 conditional formats, 2 charts, a sparkline, a dynamic array, currency and percent formats |
| `budget` | accounting formats (built-in id 44), totals, a second sheet |
| `grades` | pass marks, averages, conditional formatting over a small grid |
| `features` | a merge, a note, a hyperlink, a list validation, frozen panes, a defined name, a custom number code |
| `pivot` | a table-sourced PivotTable plus a slicer cache linked to it; a numeric field in Rows grouped by `rangePr` (the cache's `startNum`/`endNum`/`groupInterval` + one caption per group item), a value filter whose kept items are the field's hidden items, and a calculated item (`Region[East]+Region[West]`). Every probe refreshes each PivotTable before saving, so the report is re-rendered from the cache as an application would |

## The corpus run

`cargo xtask corpus <dir>` is the one-command version of the run that gate #1 needs on real-world
files: it opens every readable `.xlsx` in the directory (skipping `~$` temporary files and
cloud-sync placeholders that are not stored locally), prints each workbook's sheet and cell count,
its warnings and its error, saves and reopens one in five (or `--all`) and compares a digest of the
sheet (names, state, freeze, merges, comments, links, validation and conditional-format ranges,
tables, charts, shapes, pictures, defined names, every cell's value, formula text and style), and —
when python and openpyxl are installed — reads the saved copy back through openpyxl. It **never
writes into the corpus directory**: saved copies go to a temporary directory that is removed
afterwards, and a directory over the caps (2,000 files, 4 GB; 200 MB per file) is refused rather
than sampled. `--recursive` descends into subdirectories (eight levels); `--no-openpyxl` skips the
foreign readback.

`--keep <out>` keeps the saved copies in `<out>` and writes `saved-copies.tsv` there (source →
saved), which is what an Excel-side check needs: open a source and its saved copy in Excel and
compare. Saved names carry the row index, so two paths that sanitize alike cannot overwrite each
other.

`cargo xtask corpus target/oracle` on a fresh `cargo xtask oracle --keep` run is green (15 fixtures
and their Excel/WPS copies, 0 failures, 0 save/reopen mismatches). A local real-world run on 11-03
(9.6 GB, 3,199 workbooks) opened 3,084 readable workbooks with no reader failure, saved/reopened
635 with 0 mismatches, and Excel 16 found the copies identical; the 115 failures were 4 invalid
files and 111 password-protected ones, and 449 cloud placeholders were skipped. The run's details
are in `plan/corpus-run-2026-11-03.md` (local).

## What it has found

- **2026-11-03 — a real folder is a harsher input than a fixture.** The corpus run over a real work
  folder found a panic on a non-ASCII path in the report, cloud placeholders counted as failures, a
  weak digest that missed formula text and every non-cell feature, and two `.xls` files whose saved
  copy grew its used range to column IV because a trailing styled column range (default width,
  `fUserSet = 0`) was written as a custom-width partial `<col>`. All four are fixed (commits
  `09397e79`, `011d7ec1`).
- **2026-11-02 — an undeclared `r:` prefix in a PivotTable part.** `slicer::pivot_ext` wrote
  `<x14:slicerCache r:id="rId2"/>` into `xl/pivotTables/pivotTableN.xml` without `xmlns:r` on that
  element, so `lxml` refused the part ("Namespace prefix r for id is not defined") and WPS refused
  the file. Excel would have answered with a repair prompt. Fixed by declaring the prefix where it
  is used; the new validator rule caught 11 saving tests red before the fix, and they are green
  after it. This is the class of defect a self round-trip cannot see: our own reader does not care.
- **2026-11-02 — openpyxl's own built-in table for number format id 44** is missing the `;`
  separators. The first run reported 33 "number format" differences that were openpyxl's defect,
  not ours. The comparison therefore identifies a built-in format by its **id**, not by its text.

## What it does not prove

- **Excel does not repair-prompt on our files.** Only Excel does; the gate stays open.
- **Charts are counted, not validated.** openpyxl does not model chart parts; it warns and drops
  unsupported extensions (sparklines, for one), so their presence is all that is checked here.
- **WPS is not Excel.** It may accept a part Excel rejects, and it may reject one Excel accepts. It
  is one more independent implementation, not the target.
- **A PivotTable's rendered body is not compared** in the app probe: the application rebuilds the
  report in its own locale (WPS wrote `行标签` where the model has `Row Labels`), so the sheet that
  holds only a report body is skipped. The source data on the other sheet is still compared.
- **Not covered yet:** `.xls` import, encrypted packages, ODS, and a real-world corpus of
  Excel-authored files (that needs the pinned corpus repo, see [gaps.md §1](gaps.md)).

- **2026-11-02 — the new PivotTable encodings go through both probes.** The `pivot` fixture now
  groups a numeric field (`rangePr ... groupBy="range"`, 13 group captions) and applies a value
  filter (hidden `item/@h`); openpyxl reads every cell back and WPS 12.0 opens, recalculates and
  re-saves the package. This does not prove Excel accepts the parts — gate #1 — but it does prove
  the XML is well formed and that a foreign application tolerates it.

- **2026-11-02 — Excel is the gate probe.** The owner activated Excel 16 on the dev machine, so
  `cargo xtask oracle` now opens every fixture in Excel (`Excel.Application`) as well as WPS: it
  refreshes every PivotTable, recalculates, saves a copy and reads it back, comparing every cell,
  format, merge, table, CF/DV/chart count and defined name with the model. All five fixtures pass
  in both applications. What this does **not** prove is a real-world corpus (still 3–6 h, needs the
  pinned files); it is a set of synthetic workbooks we wrote. The probe tolerates only the
  application's own localized PivotTable chrome (`Row Labels`, `Grand Total`) on the pivot fixture,
  because Excel re-renders those in its UI language; every value and item label is compared.
- **2026-11-02 — the probe found a PivotTable grouping bug.** Excel opened our grouped-number
  pivot but silently rebuilt its cache on load (`refreshOnLoad="1"` in the cache definition) and
  re-anchored the buckets at the field's minimum, localizing the captions; a grouped number
  field's base values were missing from `sharedItems`, and the upper tail was named after `end`
  instead of the next boundary. With the encoding corrected (below) and a refresh added to the
  probe, Excel and our model agree on the whole report. See `file-format-parity.md`.
## Adding a fixture

A fixture is a `Workbook` in `xtask/src/oracle/mod.rs`. Samples come straight from
`gridcraft_engine::sample::build`; anything else is built by running real commands through `Session`,
so the fixture also exercises the commands' own contracts. If the application regenerates part of
the workbook (a PivotTable body, for instance), name that sheet in `app_skips` and say why.
