# Excel checks: the owner's half day

> **Last reviewed:** 2026-11-03 · **Who:** the owner (or anyone with Excel 16 on their own machine).
> **Why:** `cargo xtask oracle` covers what can be automated — Excel opens, refreshes every
> PivotTable, recalculates, saves and GridCraft re-reads the copy, cell by cell, on five fixtures.
> This page covers what it cannot: the repair prompt on real workbooks, how the parts look in
> Excel's own UI, and the things Excel only shows interactively.
>
> **Status 11-03:** the automated half is green, and a local real-world corpus (3,084 readable
> workbooks) opened with no reader failure, saved/reopened with 0 mismatches, and Excel 16 found
> the copies identical. The checks below are the remaining fixture-level ones; the corpus section
> adds how to point the same run at your own files.

Nothing here needs a build of GridCraft except the fixtures: the oracle writes them.

## Get the fixtures

```text
cargo xtask oracle --keep
```

That writes GridCraft's own files to `target/oracle/`:

| Fixture | What it holds |
| --- | --- |
| `sales.gc.xlsx` | the sample dashboard: a table, two charts, sparklines, a dynamic array, currency and percent formats, two conditional formats |
| `budget.gc.xlsx` | accounting formats, totals, a second sheet |
| `grades.gc.xlsx` | pass marks, averages, conditional formatting |
| `features.gc.xlsx` | a merge, a note, a hyperlink, a list validation, frozen panes, a defined name, a custom number format |
| `pivot.gc.xlsx` | a PivotTable (grouped numbers, a value filter, a calculated item) with a slicer |

**Ground rules**

- Never save over a fixture. Save copies into a scratch directory (e.g. the desktop).
- **Nothing Excel produces goes into the repository.** Keep screenshots under
  `plan/excel/screenshots/` (gitignored) and send the text report, not files.
- Don't sign in to a Microsoft account or change the Office licence while checking; the local Excel
  install is what the gate is about.
- Record the Excel version and UI language in the report (`File ▸ Account`, or the About box);
  Excel localizes PivotTable chrome, which the automated comparison already tolerates.

## Gate #1 — Excel opens our output without repair

For each of the five `*.gc.xlsx` files:

1. Open it in Excel (double-click). **Pass = no "We found a problem with some content" prompt** and
   no repair dialog.
2. Look at the sheet: values, number formats, merges, frozen panes, the table, charts and
   conditional formats should look as they do in GridCraft's own window (open the same file in
   GridCraft side by side if convenient).
3. `File ▸ Save As` a new `.xlsx` in the scratch directory (keep the name).
4. With the five saved copies in one directory, run:

   ```text
   cargo xtask corpus <that-directory>
   ```

   **Pass = `0 failed` and `0 mismatch(es)`**, and every `openpyxl` cell says `ok`. (The command
   never writes into the directory, so the copies stay as Excel saved them.)

Report one line per file: `fixture | opens? | repair prompt? | notes`.

### Your own real workbooks (optional, and now one command)

If you have a folder of real `.xlsx` files, the automated half of this gate runs on them:

```text
cargo xtask corpus "<folder>" --recursive --keep "<scratch>"
```

It never writes into the folder. It prints per-file sheets/cells/warnings and any error, skips
`~$` files and cloud-only placeholders, saves and reopens one in five (add `--all` for every file),
compares the stronger digest and reads the copies back through openpyxl. `--keep` leaves the saved
copies and `saved-copies.tsv` in `<scratch>` so you can open a source and its copy in Excel and
compare.

- **Pass:** `0 failed` on valid files and `0 mismatch(es)`; open a handful of saved copies from
  `saved-copies.tsv` in Excel, **no repair prompt**, same values and look.
- Password-protected workbooks need their password (they are reported, not guessed); invalid or
  truncated files are reported and also fail openpyxl; a 200 MB single-file guard and a 2,000-file /
  4 GB per-directory cap are deliberate — split the folder or make the online-only files available
  offline first.

## Gate #8 — the PivotTable, refreshed and looked at

Open `pivot.gc.xlsx`:

1. Click inside the report and press **PivotTable Analyze ▸ Refresh** (or `Alt+F5`) twice.
   **Pass = the report still shows** the row "East+West" with **116,320**, the grouped buckets
   (`53740-78739`…), the "Online" row and a Grand Total of **188,730**.
2. **Pass = the slicer** (right of the report) lists the Region tiles with the model's selection,
   and clicking a tile changes the report; `Clear Filter` brings the rows back.
3. Open the field list: the calculated item appears as a field item; the field's dropdown shows
   the value-filter's hidden items (the rule is stored as hidden items, not as Excel's `<filters>`).
4. Save As a copy, reopen it, and confirm the same numbers.

Report: `pivot | refresh? | East+West 116,320? | slicer? | notes`.

## Gate #7 — `.xls` import and encryption

`.xls` (Excel-authored files live only under `plan/excel/`, never in the repository):

1. Open `plan/excel/excel_cf12b.xls` in **GridCraft** (it is an Excel-authored workbook with a
   colour scale, data bars and icon sets, including reverse/icon-only variants). Compare with
   Excel's view of the same file: the colours, bars and icons should match in kind, position and
   order. **Pass = every rule is present and none is dropped.**
2. Open `plan/excel/excel_pic2.xls` in GridCraft: the two pictures should sit in the same cells and
   at the same offsets as in Excel. **Pass = both pictures, right place, right size.**
3. Encryption: save a workbook with `file.save` and a password (Agile and Standard are both
   written), then open the `.xlsx` in Excel with the password. **Pass = Excel asks for the password
   and shows the cells; a wrong password never gets past the prompt.**
   RC4-encrypted `.xls` is not written yet — skip that line.

Report: `cf12 | pictures | agile | standard | notes`.

## Gate #10 — the machine that reported the freeze/crash

On the Windows GPU and macOS machines named in the issue (not the dev machine):

1. Open `sales.gc.xlsx` and the GridCraft sample (`cargo run --release -p gridcraft -- --sample
   sales`), scroll to the bottom and back, zoom around, resize the window, and switch sheets.
2. **Pass = no freeze, no "Issue executing", no error on close.**
3. Record the driver and OS versions (`winver`, `system_profiler SPDisplaysDataType`).

Report: `machine | driver/OS | what froze (if anything) | notes`.

## What to send back

A short text report with the Excel version and language, one line per check as above, and any
screenshot under `plan/excel/screenshots/`. No Excel-produced files, no `.xlsx` copies and no
Office assets — the same rule as everywhere in this repository (see `AGENTS.md`).
