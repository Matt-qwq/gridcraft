# The foreign-implementation oracle

> **Last reviewed:** 2026-11-02 · **Status:** the accepted substitute for the Excel half of gate #1.
> The gate itself is **not met** — see [gaps.md §1](gaps.md).

GridCraft's target consumer is Microsoft Excel, and the one question a self round-trip cannot answer
is *"does somebody else's reader accept this package?"*. Excel is the only consumer whose repair
prompt actually matters, and it is not licensed on the dev machine (`ProPlus2024Volume`, no key, no
KMS host, no signed-in account). `cargo xtask oracle` closes as much of that gap as can be closed
without it: it reads a GridCraft-written package back through implementations that are **not ours**.

```text
cargo xtask oracle                    # all fixtures, both probes
cargo xtask oracle --fixture=pivot    # one fixture (sales | budget | grades | features | pivot)
cargo xtask oracle --no-app           # foreign reader only (no COM application needed)
cargo xtask oracle --keep             # keep the .xlsx artifacts in target/oracle/
```

It is a development tool, not part of `cargo xtask ci`: it needs `python` with `openpyxl`, and the
application probe needs Windows with WPS installed. Missing prerequisites are reported as `SKIPPED`,
never as a pass.

## The probes

**readback — openpyxl, a foreign reader.** The package must open in a parser we did not write, and
the cells, number formats, merges, tables and their columns, conditional-format / validation / chart
/ image counts and defined names openpyxl sees must match the model that was saved. A part whose
relationships, content types or markup are wrong in a way only a strict consumer notices fails here.
The probe also fails if openpyxl *refuses* the package.

**app — an independent spreadsheet application.** WPS 12.0 (`KET.Application`) opens the file,
recalculates it and saves a copy; GridCraft then reads that copy back and every value must survive.
This is the closest available proxy for "Excel opens our output": the part a foreign writer mangles
is exactly the part Excel would complain about.

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
| `pivot` | a table-sourced PivotTable plus a slicer cache linked to it; a numeric field in Rows grouped by `rangePr` (the cache's `startNum`/`endNum`/`groupInterval` + one caption per group item) and a value filter whose kept items are the field's hidden items. Both probes refresh every PivotTable before saving, so the report is re-rendered from the cache as an application would |

## What it has found

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
