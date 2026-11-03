# UI parity

> **Last reviewed:** 2026-11-02 · **Last updated:** 2026-11-02 · **Change:** minor (a timeline's selected range handles drag on the sheet: resize either end or slide the band, one undo step, the field's date axis cached per revision) · **Target:** Microsoft Excel (Microsoft 365), Excel for Mac 16.113.4

Interaction, controls and feel, against Excel. Part of [target-app-parity.md](target-app-parity.md).
**UI/UX fidelity: ~55% ready, 50–80 h** (estimated). Ribbon command coverage is measured
separately in [parity-checklist.md](parity-checklist.md) (274 / 290).

Sources: `crates/ui-egui/src` (ribbon, grid, editor, formula bar, dialogs, panes),
`crates/engine/src/cmd` (shortcuts in command specs), the owner's black-box notes
(`plan/excel/observed-ui.md`, local only) and user issues.

## Window and chrome

| Element | Excel | GridCraft | Status |
|---|---|---|---|
| Title bar, Quick Access Toolbar, AutoSave toggle | yes | yes | done; macOS window controls misaligned (#170), Dock icon size (#169) |
| Ribbon: tabs, groups, split buttons, galleries, contextual tabs | yes | yes (Table Design, Chart Design, Format, PivotTable and Header & Footer tabs); Fill Color, Font Color and the Format Cells colour pickers end with Recent Colors and More Colors… (#928) | done; missing contextual tabs for pictures, shapes, sparklines, header/footer and slicers |
| Ribbon collapse, simplified ribbon, customize ribbon/QAT | yes | partial | customization missing (File ▸ Options carries only the settings GridCraft has) |
| Key tips (Alt / F10 then letters) | yes, every ribbon control | done (#43 → #33/#49): 417 sequences for every tab, group and ribbon button, contextual tabs included, badges drawn in code, Esc backs out one level, per-scope collision and reachability tests; QAT/Backstage and the View combo boxes excluded (see Keyboard) | done |
| Formula bar with Name Box, expand, function hints | yes | yes | done; dynamic-array spill children show the source formula muted and read-only, using current spill ownership |
| Sheet tabs: rename, reorder, colour, hide, scroll | yes | yes | Ctrl-drag (Cmd or Option on macOS) drops a copy (#971); active tab revealed on navigation and resize (#681); New sheet (+) right after the last tab, the strip capped at 60% of the bar (#943); manual wheel/trackpad browsing preserved; next/previous skips Hidden and VeryHidden sheets and keeps the current sheet and selection when no visible destination exists; rename focus bug on macOS (#145) |
| Status bar: Sum/Count/Average and more, view buttons, zoom | yes | yes | customizable statistics partial |
| Task panes (Format Chart, Comments, Watch, Selection, PivotTable fields) | yes | yes; PivotTable source-field list scrollbar sits at the right edge of its pane | the Format Chart pane edits the chart area, either axis, one series and a data point: title, type, legend, gridlines, size, Format Axis (bounds, major unit, log scale, reverse, crossing, display units, number format), Format Data Labels (five parts, position, separator, number format, a label's own size/weight/colour, per-series on/off), Error Bars (fixed, percentage, standard deviation, standard error; amount, direction, caps) and Format Data Point (fill, line, marker, label deletion, clear) through `chart.setAxis` / `chart.setDataLabels` / `chart.setErrorBars` / `chart.formatPoint`. An option the chart kind cannot store is not offered and the section says why (a pie's axes, a chartex part, a category axis's log/bounds/units/crossing, a marker on a column point). The Current Selection follows the sheet too: a click on a bar, area, slice, marker, data point, legend entry or axis label selects it (the first click selects the chart, as Excel does), and a clicked point is the Data Point section's point. Missing: element-level Format pane for shapes and pictures, per-point positions, callouts |
| Backstage (File) view: Info, New with templates, Open recent, Export, Options | yes | yes (#174, 10-11): the File tab opens a full-window view (Esc or the back arrow return) with Home, New (blank + the three samples), Open (recent + Browse; the recent list tells when each workbook was last changed, 10-12), Info (location, properties, Protect Workbook), Save, Save As, Print (page preview, active sheet or workbook, fit to page, Page Setup), Export (PDF, CSV, web page), Close and Options (`file.options`: theme, language, formula bar, AutoSave, calculation) | no Account, Share or template gallery beyond the samples; Options applies at once (no OK/Cancel dialog) and holds only the settings GridCraft has |
| Dark mode, system appearance | yes | yes | done (10-09); follows the system on first run, title-bar switch and File › Display theme (#194, #239) |
| Page Layout view (paper view with rulers and header editing) | yes | yes (#329, 10-11): per-sheet view saved in XLSX, pages with margins and gaps on the print pagination, "Click to add data" pages, rulers (inches or centimetres), headers and footers drawn and edited in place with a Header & Footer tab | margins drag on the rulers (#792, with header and footer margins); header/footer pictures, Different First Page / Odd & Even missing (1–3 h) |
| Multiple windows per workbook | yes | no (#162) | see [hardware-parity.md](hardware-parity.md) |

## Grid interaction

AutoFit column width includes dropdown space only for filter header cells (tables and ordinary
AutoFilters), then takes the maximum against body cells. Windows Excel 16.0 build 20326,
observed using synthetic workbooks via its public automation API on 2026-10-11, reserves
12.5 points at 100-200% zoom and 25 points at 50%, independently of row height. GridCraft
uses that allowance in its grid units and measures with the fonts the grid draws in
(`Session::text_width` → `GridText::cell_text_width`: the shaped width for a non-ASCII cell, the
unshaped galley width for ASCII, the average-glyph estimate when no font is available).
Header text aligns and clips before the button. Command and header-double-click paths share
this logic, with one undo step.

| Behaviour | Status |
|---|---|
| Click/drag/Shift/Ctrl selection, multi-area selection | rectangular drag works in one direction only in some cases (#28) |
| Cursor while dragging cells, headers, freeze handles and objects | stable for the active drag (#936); Draw tools keep their crosshair; header arrows and a custom cell cursor remain unimplemented |
| Selection outline glides to a new selection (Excel's "Provide feedback with animation") | **not done, deliberately**: the outline is drawn on its cells in the frame the selection moved. Two months of a 0.12 s glide showed what it cost — the picture lagged the hit testing, every arrow key kept repainting the grid, and Excel's own cell cursor does not animate either. The UI has no motion at all (see decisions.md) |
| Fill handle: drag to fill series, double-click to fill down, right-drag menu | fill done (Ctrl+drag swaps copy and series); right-drag menu partial: Copy Cells, Fill Series, Fill Formatting Only, Fill Without Formatting, Series…; Fill Days/Weekdays/Months/Years, Linear/Growth Trend and Flash Fill missing |
| Fill handle: drag to fill series, double-click to fill down, right-drag menu | fill done (Ctrl+drag swaps copy and series); month patterns use the workbook's 1900 or 1904 date system; right-drag menu missing |
| Drag-move and drag-copy cells (border drag, Ctrl/Option, Shift to insert) | done (M3) |
| In-cell editing, Enter/Tab movement, Alt/Option+Enter line breaks | done |
| Point mode (click/drag/arrow references), reference colouring, F4 | done; Command/Ctrl-click adds separate arguments with the edit's locale separator, and Shift-click extends the current reference from a fixed anchor on the same sheet. Pointer-event regressions cover repeated/reversed selections, manual separators, sheet switches and keyboard continuation. |
| Number-format colors (`[Green]`, `[Color1]`–`[Color56]`) | Rendered in grid and PDF from shared RGB mappings (#1452) |
| Point mode (click/drag/arrow references), reference colouring, F4 | done |
| AutoComplete in columns, function autocomplete and argument tips | done; cell helpers clip to the worksheet and hide when the editor scrolls out (#200) |
| Header resize and double-click AutoFit | done (#42, #88 for header dividers) |
| Data › Advanced Filter | dialog offers in-place or copy-to filtering, list and optional criteria ranges, and unique records; Alt+A,Q opens it (#1622). The engine's copy-to path clears only the result footprint and rejects worksheet overflow (#1031). |
| Outline symbols for grouped rows / columns (band beside the headers, brackets, − / +, level buttons 1…n+1) | done (#373): clicks run `data.hideDetail` / `data.showDetail` / `data.showLevels`, frozen panes and right-to-left sheets included; summary rows/columns are always below/right (the "Summary rows below detail" setting isn't modelled), no "Show outline symbols" option yet, and a block of cells still groups rows without the Rows / Columns dialog |
| Reorder selected whole columns by dragging their headers | done (10-11) |
| Freeze panes, split panes | freeze done; split to verify |
| A Table's header names and filter buttons in the column headings once its header row scrolls away | done (#1423): with the active cell in the Table, as in Excel; the button opens the column's filter menu, the rest of the heading still selects the column. Plain AutoFilter ranges keep their letters (Excel does the same) |
| Copy as HTML table for other apps | done (#36, 10-10) |
| Context menu on cells | 16 items (Cut, Copy, Paste, Paste Special, Insert, Delete, Clear Contents, Filter by value, Sort, New Comment, New Note, Format Cells, Pick From Drop-down List, Define Name, Link). Insert Copied Cells (Insert Cut Cells for whole rows/columns) while the clipboard holds a copy (#21). Missing: Insert Cut Cells for a cell block, paste-option icons, Quick Analysis, Get Data from Table, Show Changes, Smart Lookup |
| Move and resize charts, pictures and shapes | yes (#190): all eight handles on charts, pictures, shapes and slicers (corners and edge midpoints); dragging a left/top edge moves the anchor with it and writes the extent in the same command; Shift keeps the aspect ratio, a 20-point minimum size holds, Esc cancels a drag mid-way, and one drag is one undo step. Arrow keys nudge a selected object (10 points; Ctrl/Cmd 1 point) and Shift+arrow resizes it. Dragging a chart stops at the sheet's top-left; repeat to enlarge |
| PivotTable slicers and timelines on the sheet | yes: a slicer draws its caption band and value tiles in a grid of `columns` columns from theme tokens (selected tiles tinted with a check mark, unselected ones dimmed, a clear-filter button in the band), the caption and tile text is elided, tiles beyond the frame are elided behind an "N of M" line and the wheel over a filtered slicer scrolls its tiles, and the frame takes the same eight-handle selection border as other objects. Clicking a tile runs `slicer.toggle` (Ctrl/Cmd sends `multi`), dragging the frame or nudging it with the arrow keys runs `slicer.move`, Delete runs `slicer.delete`, Tab/Shift+Tab move a tile focus and Enter toggles it, and the right-click menu carries Delete, Select All, Clear Filter and a Slicer Styles submenu. A table slicer follows its table's and column's names when either is renamed, and a filter set from the PivotTable itself shows on a connected slicer. A timeline draws its caption, time unit and its field's full date axis as a track, with the selected range as a band whose two handles resize it (dragging the band slides it without changing its length; days snap, one drag is one undo step and Esc restores the range), and it is selected, moved and deleted like the slicer; both are listed in the Selection pane | done; the model's `sort` hint is not applied (tiles stay in cache order), and there is no contextual Slicer ribbon tab |
| Context menus on row/column headers, sheet tabs, charts, shapes | headers and tabs done; objects partial |
| Quick Analysis (Ctrl+Q) | missing |
| Smooth scrolling, scrollbars | done; vertical scrollbar added 10-09 (#73); horizontal scrollbar in the tab bar (60/40 split, drag, track click pages) 10-13 (#176, #840); the tabs/scrollbar split can't be dragged yet |
| Text overflow into empty neighbours, wrap, rotation, shrink to fit | done |
| Complex-script and bidirectional cell text | done for the grid and the in-cell editor: `crates/ui-egui/src/text.rs` owns one `gridcraft_core::text` engine per app and registers the very faces egui paints cells with (read from the live `FontDefinitions`), shapes each line into bidi runs in visual order and folds the runs back into one galley. Arabic joins, Devanagari reorders and merges clusters, Thai marks position, a Latin/digit island inside Hebrew or Arabic keeps left-to-right order, general right-to-left text aligns right, and neutral text on a Sheet Right-to-Left follows the sheet. Printable ASCII keeps the old unshaped path bit for bit (`text::needs_shaping`). The in-cell editor draws its shaped line, selection and caret itself and moves the caret through visual clusters with →/← and clicks, while the buffer, the formula bar and tooltips stay logical. Not shaped yet: wrapped cells, multi-line cells, rotated cells drawn from a wrap job, text boxes, chart labels and PDF export (`docs/localization-parity.md`) |

## Keyboard

Excel is a keyboard application: every ribbon control has an access-key path and the documented
shortcut list runs past 200 keys. GridCraft has the same two layers — **key tips** (sequential Alt
access keys) and **direct shortcuts** — plus the keys the grid, formula bar and cell editor own.

### Key tips (Alt or F10, then letters)

Alt, or F10 on keyboards without an Alt, arms the badges: one per tab, and once a tab is open, one per
control. A typed sequence switches tabs and runs its command; `Esc` backs out one level at any depth
(from a control's group to the tab, from the tab to the tab letters, then off); Alt again, a click, a
wheel or a focus loss cancels. The badges are painted from `theme::Tokens` in `ribbon::keytip` — no
bitmaps, no vendor artwork. Letters also work with left Alt held (`Alt`, `H`, `O`, `A` autofits the
column), as in Excel.

The table is `crates/ui-egui/src/keytips/keytips.json` (hand-edited, one line per sequence);
`cargo xtask keytips` generates `keytip_table/generated.rs` and one end-to-end test per sequence in
`crates/ui-egui/tests/keytips_generated.rs` (419 tests today). A sequence may:

| Kind | What it does |
|---|---|
| `tab` | switches to a ribbon tab and keeps the level so its badge letters can follow |
| `command` | runs an engine command by id |
| `dialog` | opens a dialog by name |
| `control` | routes to the ribbon control that owns the keytip (opens its dropdown, or runs its click handler with the right params), so a dropdown keytip does not fire the bare command |
| `continue` | a group level with no action of its own (the badge waits for the next key) |
| pending | a `command` whose command does not exist yet: badge and level reserved, invocation silent, live the moment the command lands |

**Tabs.** Every tab has one, contextual tabs included; a contextual tab's badge is painted only while
the tab is shown, and typing its sequence without the context does nothing (as in Excel).

| Tab | Sequence | Note |
|---|---|---|
| Home | `H` (and legacy `E` Edit, `O` Format paths) | |
| Insert | `N` | |
| Draw | `J I` | two-letter unit, one badge |
| Page Layout | `P` | |
| Formulas | `M` | |
| Data | `A` | |
| Review | `R` | |
| View | `W` | |
| Automate | `U` | our letter: Excel publishes none, and `A U` would be shadowed by Data ▸ Ungroup |
| Table Design | `J T` | contextual: active cell in a table |
| Chart Design | `J C` | contextual: a chart is selected |
| Header & Footer | `J H` | contextual: Page Layout view, editing a header or footer |
| File | `F` | opens the Backstage view; its letter path still runs (`F`, `S` saves) |

Groups follow Excel where it has a published sequence and our own mnemonic otherwise: `H`, `F` font
(`H F B` size, `H F C` colour), `H`, `B` borders, `H`, `L` conditional formatting, `H`, `O` cells,
`H`, `V` paste, `N`, `Z` (Symbol `N Z S`, Sparklines `N Z K`, then `L`/`O`/`W`), `W`, `V` show/hide
(`W V G` gridlines), `M`, `U` AutoSum, `A`, `S` sort, and so on. Every ribbon button paints the rest
of its sequence on the badge; a menu anchor shows one group letter and its items show the last key
(`H`, `V` opens Paste, `S` still chooses Paste Special).

**What the tests enforce** (`keytip_table.rs` unit tests and the generated suite):

- every sequence is 1–4 ASCII keys (Excel's sparkline paths run to five; ours stop at four so the
  whole scheme is typeable from memory), assigned once, and resolves to exactly one action;
- no terminal sequence is a strict prefix of another, so nothing is shadowed (`N Z T` used to hide
  `N Z T X`; the generator now validates both directions);
- no two tabs share a prefix at the armed root (`A` must not make a later `A U` Automate badge
  unreachable — this is why Automate is `U`);
- within one parent, two different controls never paint the same badge
  (`no_two_targets_paint_the_same_badge_under_the_same_parent`);
- every `command` target exists in the live catalog and every `dialog` target opens.

Deliberate exceptions: Quick Access Toolbar/title-bar buttons, Backstage pages, dialog fields and the
View tab's two combo boxes (Display theme, Interface size) carry no badges — they are not ribbon
buttons. Menus whose items share one command id (Paste Special variants, chart types, number-format
symbols) paint the anchor badge only: a trie cannot tell two items of the same command apart.

### Reachability of what the mouse can do

| What | Keyboard | Detail |
|---|---|---|
| Cell context menu | `Shift+F10` | opens at the active cell, without moving the selection |
| Row/column header menus | (mouse to open) + arrows | navigate like the cell menu |
| Menu navigation | arrows, Home/End, Enter, Space, Esc | the keyboard position is painted; Enter/Space run the highlighted row (Enter without a highlight takes the first), Esc closes. Nothing takes focus, so the grid keeps the keyboard and the selection while the menu is open, and Space never starts an edit behind it |
| Dedicated Menu key (VK_APPS) | **blocked upstream** | egui-winit 0.36 maps neither `KeyCode::ContextMenu` nor `NamedKey::ContextMenu` and egui has no `Key` variant for it, so the event never reaches the app; fixing it needs a dependency patch. `Shift+F10` is the reachable pair, as on keyboards without the key |
| Ribbon button | Alt/F10 + sequence | activates with no pointer; dropdowns open for typing/arrows/Enter |
| Menus keytips | — | while a context/menu is open the grid is quiet; the menu's own keys win, never both |

Tests: `crates/ui-egui/tests/context_menu_keyboard.rs` (open, arrows, wrap, Home/End, Enter, Space,
Escape, header menus, one-shot execution) and the generated keytip suite (activation with no pointer).

### Shortcut audit (#110)

Checked against Microsoft's published "Keyboard shortcuts in Excel" (Windows and Mac tables).
Bindings live in `crates/ui-egui/src/grid.rs` (navigation and editing keys), `ribbon::shortcut`
(Ctrl/Cmd letters), `keymap.rs` (F-keys, Alt/Option and the Mac variants, with a `lookup` table the
tests cover), `formula_bar.rs` (keys while editing) and `keytips.json` (Alt sequences).
"Ctrl" means Cmd on a Mac unless the Mac column says otherwise; number-format and border keys match
the physical key (Ctrl+Shift+4 is `$`'s position, wherever `$` is on the layout). `added` means it
landed with this audit; `missing` means Excel has the shortcut but we have no command behind it yet;
`n/a` means the feature does not apply to a single-window spreadsheet.

**Clipboard, selection and editing**

| Excel shortcut | Mac | Our command id (or handler) | Status |
|---|---|---|---|
| Ctrl+C / X / V | Cmd | `edit.copy` / `edit.cut` / `edit.paste` | have |
| Ctrl+Z / Y / Shift+Z | Cmd | `edit.undo` / `edit.redo` | have |
| Ctrl+A | Cmd | `edit.selectAll` | have |
| Ctrl+Space / Shift+Space | Control+Space / Shift+Space | `selection.column` / `selection.row` | have |
| Ctrl+Shift+Space | Cmd+Shift+Space | `edit.selectAll` | have |
| Shift+arrow, Ctrl+arrow, Ctrl+Shift+arrow | same with Cmd | `selection.move` (extend / jump / extend jump) | have |
| Home / Ctrl+Home / End / Ctrl+End, Shift+Home, Ctrl+Shift+Home/End | Cmd | `selection.set`, extend-to-start/end/row start in `keymap.rs` | have |
| Page Up/Down, Alt+Page Up/Down | same | `selection.move` one screen; `Shortcut::ScreenLeft/Right` | have |
| Ctrl+Page Up/Down | Cmd+Page Up/Down | `sheet.previous` / `sheet.next` | have |
| Tab / Shift+Tab, Enter / Shift+Enter | same | grid traversal (`move_after_enter`) | have |
| F2, Delete, Backspace | Control+U edits | `begin_edit`, `edit.clearContents` | have |
| Esc | same | cancels editing / copy / Format Painter | have |
| Ctrl+Enter | Control+Enter | `range.fill` (editor) | have |
| Ctrl+Shift+Enter | Control+Shift+Enter | array formula entry (editor) | have |
| Ctrl+D / Ctrl+R | Cmd | `edit.fillDown` / `edit.fillRight` | have |
| Ctrl+E | Control+E | `edit.flashFill` | have |
| Ctrl+; / Ctrl+Shift+; | Control+; / Cmd+; | date / time entry (`Shortcut::Date` / `Time`) | have |
| Ctrl+' / Ctrl+Shift+" | Control | formula / value from the cell above | have |
| Ctrl+- / Ctrl+Shift+= | Cmd | `home.deleteRows` / `home.deleteColumns`, `home.insertRows` / `home.insertColumns`, or the Insert/Delete Cells dialogs | have |
| Ctrl+Shift+`*` (Ctrl+Shift+8) | Cmd+Shift+Space | `selection.currentRegion` | have |
| Ctrl+[ / Ctrl+] / +Shift | same | `formulas.selectPrecedents` / `selectDependents` | have |
| Ctrl+Shift+O | same | `edit.goToSpecial` `{kind: "comments"}` (cells with comments/notes) | **added** |
| Alt+; | Option+; | `edit.goToSpecial` `{kind: "visible"}` (visible cells only) | **added** |
| Ctrl+Alt+5, Ctrl+/ , Ctrl+\ , Ctrl+Shift+\| | same | object cycling, current array, row / column differences | missing (no command) |
| Ctrl+Shift+Page Up/Down | same | group / ungroup sheets | missing (one sheet group) |

**File, workbook and print**

| Excel shortcut | Mac | Our command id (or handler) | Status |
|---|---|---|---|
| Ctrl+N / O / S / W | Cmd | `file.new` / `file.open` / `file.save` / `file.close` | have |
| Ctrl+Shift+S, F12 | Cmd / F12 | `file.saveAs` | have |
| Ctrl+P | Cmd+P | `file.print` (print window via `print_now`) | have |
| Ctrl+F2 | — | `file.print` | **added** |
| Ctrl+Shift+F12 | — | `file.print` | **added** |
| Ctrl+F1 | Cmd+F1 | `view.collapseRibbon` | have |
| Ctrl+F4 | Cmd+F4 | `file.close` | have |
| Alt+F8 | Fn+Alt+F8 | macros dialog | have |
| Ctrl+K | Cmd+K | Insert Link dialog | have |
| Ctrl+T, Ctrl+L | Cmd+T | `insert.table` | have |
| Ctrl+Shift+L | Cmd+Shift+F / Control+Shift+L | `data.filter` | have |
| Ctrl+O after typing a URL | — | open hyperlink | missing |
| Ctrl+F5 / F9 / F10, Ctrl+Shift+F1 | same | window size / minimize / maximize / full screen | n/a (the OS owns the window) |

**Navigation and tools**

| Excel shortcut | Mac | Our command id (or handler) | Status |
|---|---|---|---|
| Ctrl+F / Ctrl+H | Cmd (Control+H) | Find / Replace dialogs | have |
| Ctrl+G, F5 | same | Go To dialog | have |
| F3 | Cmd+F3 (Define Name) | paste-name list | missing (`formulas.pasteName` command missing) |
| Shift+F3, Ctrl+F3, Ctrl+Shift+F3 | Shift+F3, Cmd+F3, Cmd+Shift+F3 | Insert Function, Name Manager, `formulas.createFromSelection` | have |
| Shift+F4 | same | repeat last Find | missing (no command) |
| F4 | same | repeat last action | missing (no command; F4 cycles references while editing) |
| F6 / Shift+F6 | same | next / previous pane | missing (no split-pane cycling command) |
| F7 | same | `Spelling` dialog | have |
| F8 / Shift+F8 | same | extend / add to selection mode | missing (no extend mode) |
| F9 / Shift+F9 / Ctrl+Alt+F9 | same | `formulas.calculateNow` / `calculateSheet` | have |
| F11 / Shift+F11 | same | F11 is a chart sheet (missing, no chart sheets) / `home.insertSheet` | partial |
| Alt+F11 | Option+F11 | VBA editor | n/a |
| Shift+F10, Alt+Shift+F10 | same | context menu (see above) / Error Checking dialog | have |
| Alt+Down | Option+Down | validation drop-down or the AutoFilter popup | have |
| Alt+= | Option+= | `formulas.autoSum` | have |
| Ctrl+\` | Ctrl+\` | `formulas.showFormulas` | have |
| Ctrl+8 | same | outline symbols on/off | missing (no command) |
| Ctrl+Q | same | Quick Analysis | missing (feature) |
| Ctrl+Shift+U | Control+Shift+U | expand / collapse the formula bar | missing (the toggle `view.formulaBar` is a different action) |
| Ctrl+Shift+L | see file table | `data.filter` | have |
| Alt+Q | — | search / Tell Me | missing (`ui.search` reserved keytip `Q`) |
| Ctrl+Tab / Ctrl+Shift+Tab, Ctrl+F6 | same | next / previous workbook window | n/a (one window per workbook) |
| Ctrl+Shift+F2, Shift+F2 | Cmd+Shift+F2, Shift+F2 | `review.newComment` / `review.newNote` | have |

**Formatting**

| Excel shortcut | Mac | Our command id (or handler) | Status |
|---|---|---|---|
| Ctrl+1 | Cmd+1 | Format Cells dialog | have |
| Ctrl+B / I / U / 5 | Cmd | `home.bold` / `italic` / `underline` / `strikethrough` | have |
| Ctrl+2 / 3 / 4 | — | `home.bold` / `italic` / `underline` | have |
| Ctrl+Shift+~ ! @ # $ % ^ | Control+Shift | `home.numberFormat` variants (General, Number, Time, Date, Currency, Percent, Scientific) | have |
| Ctrl+Shift+& / _ | Cmd+Option+0 / Cmd+Option+- | `home.borders` `outline` / `none` (Option+arrows for one side) | have |
| Ctrl+9 / 0, Ctrl+Shift+9 / 0 | Cmd | `home.hideRows` / `home.hideColumns`, `home.unhideRows` / `home.unhideColumns` | have |
| Ctrl+6 | Control+6 | `view.showObjects` | have |
| Ctrl+Shift+> / < | Cmd+Shift+> / < | `home.increaseFontSize` / `decreaseFontSize` | have |
| Ctrl+Shift+F | Cmd+Shift+F | Format Cells (Font tab) | have |
| Ctrl+Shift+G | — | Workbook Statistics | have |

**Data, formulas and calculation**

| Excel shortcut | Mac | Our command id (or handler) | Status |
|---|---|---|---|
| Alt+= / Shift+F3 / F4 (editing) | Option+= | `formulas.autoSum`, Insert Function, reference cycling | have |
| Ctrl+Shift+A after a function name | Control+Shift+A | argument names in the editor | have |
| Alt+Shift+Right / Left | Option+Right / Left is next/previous sheet | `data.group` / `data.ungroup` | have |
| Alt+F5 | Option+F5 | `pivot.refresh` (selected PivotTable) | **added** |
| Ctrl+Alt+F5 | Cmd+Option+F5 | `data.refreshAll` | **added** |
| Alt+Shift+F1 / Alt+F1 | Shift+F11 / Option+F1 | `home.insertSheet` / `insert.chart` | have |
| Ctrl+F9 / F9 / Ctrl+Alt+F9 | same | `formulas.calculateNow` / `calculateSheet` | have |
| Ctrl+Shift+F9 | same | recheck dependents and recalculate | missing |

**Mac extras** (where the Windows key would clash with a system key)

| Excel for Mac | Our command id (or handler) | Status |
|---|---|---|
| Cmd+Option+R | `view.collapseRibbon` | have |
| Cmd+Shift+H / R / K / J, Control+L, Control+U | Replace, Sort, `data.group` / `ungroup`, Define Name, edit the active cell | have |
| Cmd+Option+arrows, Cmd+Option+0 / - | `home.borders` per side, outside / none | have |
| Cmd+Shift+F2, Cmd+F3, Cmd+Shift+F3 | New Comment, Name Manager, Create from Selection | have |
| Cmd+= , Shift+F7, Cmd+Shift+L | calculate, thesaurus, Modify Cell Style | missing (Cmd+= is egui's zoom key; no thesaurus or cell-style dialog) |

Count today: 68 command specs declare a `shortcut` string; the grid, editor and `keymap.rs` add the
rest of the keys above (about 40 more combinations), and six of them landed with this audit. The
remaining Excel keys are mostly features we do not have yet (Quick Analysis, extend mode, pane
cycling, sheet groups), listed as missing rather than silently absent.

On Windows, AltGr arrives as Ctrl+Alt: a Ctrl+Alt key that also types a character is treated as
text, so AltGr symbols (e.g. `~` on a German layout) and user-defined Ctrl+Alt+letter hotkeys that type
a character (e.g. Ctrl+Alt+A for `ä`) never trigger a shortcut, Ctrl+letter ones like Select All included.

## Dialogs

Format Cells, Insert Function/Formula Builder, Find and Replace, Go To and Go To Special, Sort,
Name Manager, Data Validation, Paste Special, Conditional Formatting manager, Page Setup, Goal
Seek, Scenario Manager, Data Analysis, Forecast Sheet and others exist. Missing or partial: Options,
Solver, Power Query editor, Insert Object, Equation editor, Protect/Allow Edit Ranges,
Workbook Links, Accessibility checker. Like Excel's, every dialog opens centred and moves by its
title bar, keeping its place while it is open (#1250).

## Accessibility

AccessKit is enabled; the ribbon, dialogs and panes carry labels, checked states and key tips, and
**Review ▸ Check Accessibility** lists objects without alternative text (charts, pictures, shapes
with no text of their own, pictures in cells), tables without a header row, merged cells and
default sheet names — each row goes to its sheet/object/range and takes alt text inline through
`object.setAltText` (written to `xdr:cNvPr/@descr`). With a screen reader attached the **sheet is
walkable**: the visible cells become an AccessKit grid — a `GridCell` per cell with its value, its
address and formula in the description, row/column indexes, the active cell selected and named as
the grid's active descendant, and the grid labelled with the sheet, the active cell and the
selection count. The tree covers frozen rows/columns, follows scrolling and is rebuilt only while
a reader is attached (capped at 2,000 cells a frame), and the header row and column are
`ColumnHeader`/`RowHeader` nodes with their letters, numbers and indexes. **High contrast**
(View ▸ Show, `view.highContrast`) paints the chrome black, white and yellow — with a yellow
selection and cyan accents, white sheet page and black cell text — and sends egui's own widgets dark
with it; the choice is remembered across restarts. The **status bar carries an Accessibility
entry**: it shows the issue count (`Accessibility: N`, or just `Accessibility` when there is
nothing to fix) from `review.checkAccessibility`, cached per document revision (the check walks
every sheet, so it does not run every frame), is a focusable status-bar button, and opens the same
Check Accessibility dialog the Review tab does. A selected chart, picture, shape or slicer opens
its **own context menu with Shift+F10** — the same one a right-click opens (delete, Select Data,
Format Chart, slicer actions), at the object's frame — while a cell selection keeps the cell menu.
What remains of the accessibility tail is nothing on this list; shape/picture menus carry alt text
through the pane.

## Feel

Pixel comparison against Excel screenshots hasn't been run since M1; fonts depend on the system
(no Calibri; Carlito would match metrics, #29, #4, #168 for weights); icons are GridCraft's own
by rule and will never match Office's, by design.

## Work (50–80 h)

Key tips done · remaining shortcuts with their features 2–4 · keyboard context menus done, Quick Analysis 5–8 · Page Layout view
8–12 · Options dialog and ribbon/QAT customization 6–10 · accessibility 12–18 · selection and
scrollbar bugs 2–4 · pixel/feel pass against observed Excel 7–12.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-02 | minor | Shift+F10 on a selected chart, picture, shape or slicer opens its own context menu at the object's frame (the right-click menu: delete, Select Data, Format Chart Area, slicer actions), and the cell menu otherwise; the test drives `keymap::dispatch` with a real `Geo`. `grid::selected_object` is the model side (kind, id, anchor) and has its own test |
| 2026-11-02 | minor | The status bar carries an **Accessibility** entry: the `review.checkAccessibility` issue count (cached per document revision, since the check walks every sheet), a focusable button that opens the same Check Accessibility dialog as the Review tab. Two tests (the count and its caching, and a status-bar frame that runs the command) |
| 2026-11-02 | minor | A timeline's selected range handles drag on the sheet: either end resizes, the band slides, days snap, one drag is one undo step |
| 2026-10-30 | minor | A table slicer follows a table rename and a column-header rename (its caption too while it still shows the old name), and a filter changed on the PivotTable mirrors onto a connected slicer when every connected report agrees; the timeline's range handles remain the one unfinished interaction |
| 2026-10-30 | minor | PivotTable slicers and timelines on the sheet: a slicer's caption band and value tiles (grid of `columns` columns, selected tiles tinted and checked, clear-filter button, elided text, wheel-scrolled tiles with an `N of M` line, eight-handle selection frame), a timeline's caption, unit and full-range track with two handles, the same object drag/nudge/Delete/menu/Selection-pane wiring as charts and pictures through `slicer.toggle`/`slicer.move`/`slicer.delete`/`slicer.selectAll`/`slicer.clear`/`slicer.setStyle`, Tab/Enter tile focus, and tests for tile clicks, one-step drag undo, Delete, hostile anchors, empty items, Tab/Enter, the timeline drawing, the Selection pane and the right-click menu |
| 2026-10-30 | minor | Grid and in-cell editor text is shaped and bidi-ordered through one `gridcraft_core::text` engine per app (`crates/ui-egui/src/text.rs`): Arabic joins, Devanagari reorders and merges clusters, Thai marks position, mixed left-to-right and right-to-left runs draw in visual order with digits keeping their reading order, general right-to-left text aligns right, and the editor's caret and selection move through visual clusters while the buffer stays logical. A `sys-devanagari` fallback face (Nirmala UI / Kohinoor / Noto Sans Devanagari) joins the other script fallbacks, so Hindi cells draw instead of boxes. Printable ASCII keeps the unshaped path bit for bit |
| 2026-11-02 | minor | High contrast (View ▸ Show, `view.highContrast`, keytip WVI): a black/white/yellow chrome with a yellow selection and cyan accents, egui's widgets darkened to match, the sheet keeping its own white page, and the choice remembered across restarts |
| 2026-11-02 | minor | The sheet is walkable by a screen reader: the visible cells become an AccessKit grid (`GridCell` nodes with value, address, formula, row/column index, the active cell selected and named as the grid's active descendant; frozen panes included, rebuilt only with a reader attached) |
| 2026-11-02 | minor | Alternative text for charts and shapes (`Chart::alt`, `Shape::alt`, `xdr:cNvPr/@descr`, escaped) with `object.setAltText`; Review ▸ Check Accessibility covers charts, pictures, shapes, pictures in cells, tables without a header row, merged cells and default sheet names, and its dialog goes to each issue and sets alt text inline |
| 2026-11-02 | minor | The Format Chart pane's Data Labels and Data Point sections set a label's own font (size, weight, colour) through `chart.setDataLabels`, at the chart, series or point level |
| 2026-11-02 | minor | Clicking a chart element selects it in the Format Chart pane: the renderer returns its click regions with its primitives, so a bar, area, slice, marker, data point, legend entry or axis label is hit exactly where it was drawn; the first click selects the chart and the next the element, and a clicked point becomes the Data Point section's point |
| 2026-10-30 | minor | The Format Chart pane drives axes, data labels, error bars and data points through the engine's four chart commands, with a Chart Elements selection list; options the chart kind cannot store are missing from the pane and the section says why |
| 2026-10-30 | minor | Chart, picture, shape and slicer resize (#190): all eight handles work (before, only the bottom-right corner did, and a drag of any other handle moved the object instead), a left/top edge drag carries the anchor, Shift keeps the aspect ratio, a 20-point minimum size, Esc cancels the drag and drops its undo step, one drag is one undo step, and arrow keys nudge or (with Shift) resize the selected object |
| 2026-10-30 | major | Key tips across the whole ribbon: 417 sequences, every standard tab and the contextual Table Design / Chart Design / Header & Footer tabs, per-control badges for what had none (strikethrough, Draw pens/eraser, Slicer, Checkbox, Show/Hide Detail, Data Analysis, Solver, Forecast Sheet, Show Comments, Insert Slicer, chart controls, header/footer fields, Automate), sparkline paths cut to four keys, and tests for length, uniqueness, per-scope badge collisions, tab reachability and catalog targets. Shift+F10's context menus are keyboard-navigable (arrows/Home/End/Enter/Space/Esc, highlight painted, the grid keeps focus and selection and Space does not start an edit); the Menu key is blocked in egui-winit. Shortcut audit: Alt+; visible cells, Ctrl+Shift+O commented cells, Ctrl+F2 / Ctrl+Shift+F12 print, Alt+F5 / Ctrl+Alt+F5 pivot and query refresh added, the full Excel → command table written down |
| 2026-10-21 | minor | Background error checking, first part (#1994): a constant whose text would be a number if typed (signs, thousands, currency, percent, exponent, parenthesised negative) gets Excel's green corner triangle; the active cell then shows a warning button whose menu names the problem and offers Convert to Number for the selection (`formulas.convertToNumber`). Dates, times and fractions kept as text and formula results are not flagged, as in Excel |
| 2026-10-21 | minor | Paste Special laid out as Excel's: Paste and Operation options each in two columns inside a frame, Skip blanks and Transpose side by side, Paste Link as a button that pastes references at once; labels translated (the generic form didn't translate them) and the window no longer runs past the screen |
| 2026-10-21 | minor | Form dialogs (conditional-format quick rules, New Name, Remove Duplicates, Text to Columns and every other `Dialog::form`) fit their content: OK and Cancel sit in a row under the last field instead of a right-to-left layout that took the screen's whole height and left the buttons floating halfway down |
| 2026-10-20 | minor | Esc and Enter close a message over the cell editor, such as a data validation alert, and the entry stays open to fix, with the keyboard back in it. Before, the editor took them (the first Esc cancelled the edit behind the alert and only the second closed it), and after the message typing went nowhere until the cell was clicked |
| 2026-10-19 | minor | Ctrl+Enter (`range.fill`) gives each cell what typing the input there gives: a General cell gets the automatic number format (`5%`, `$5`, `3/4/2026`, `=TODAY()`) even when it already held a value, and a cell with its own format, a formatted row or column, or Text keeps it. Cells that held a value used to keep General (`5%` showed 0.05), and empty cells took the active cell's format |
| 2026-10-19 | trivial | While a mouse button is held (a click or a drag selecting cells) the selection outline is drawn on its cells at once instead of gliding after the pointer, which read as lag; keyboard moves still glide |
| 2026-10-18 | minor | Keytip letters typed with left Alt held continue the sequence (Alt+H, O, A / Alt+H, O, I ran nothing before: Alt+letter cancelled keytips); other Alt combinations, AltGr and macOS Option are unchanged (#1846) |
| 2026-10-17 | minor | A Ctrl+Alt+letter key that types a character (a user-defined Windows hotkey such as Ctrl+Alt+A for `ä`) types it instead of running the Ctrl+letter shortcut, e.g. Select All (#1530) |
| 2026-10-16 | minor | Right-drag on the fill handle opens the fill menu on release: Copy Cells, Fill Series, Fill Formatting Only, Fill Without Formatting, Series…; Esc cancels the drag or closes the menu without filling |
| 2026-10-16 | minor | With the active cell in a Table whose header row is scrolled out of view, the column headings show the header names and their filter buttons in place of the letters (#1423) |
| 2026-10-16 | minor | Ctrl+6 hides every object (charts, pictures, shapes, slicers) and shows them again; hidden objects are not drawn, take no clicks and drop their selection, and the setting is saved with the workbook (`workbookPr showObjects`) |
| 2026-10-16 | minor | Ctrl+Shift+A (Control+Shift+A on a Mac) after a function name, or after its `(`, inserts the argument names and the closing parenthesis, as in Excel; one Undo in the editor takes them back |
| 2026-10-16 | minor | Advanced Filter ribbon dialog, optional criteria for unique-only extraction (#1622); safe copy-to output (#1031) |
| 2026-10-16 | minor | Remove Duplicates shows an editable Range field initialized from the selection and passes it to the engine on OK (#1388). Live range-picking outside the dialog remains unsupported |
| 2026-10-16 | minor | Web printing registers one-shot `afterprint` cleanup before opening the dialog; missing content windows and setup, listener or print failures also release the hidden iframe and PDF blob URL (#1054) |
| 2026-10-16 | minor | The shared UI clock delegates to the core Unix-millisecond clock, which reads Date.now() on wasm, instead of returning zero there (#1026). Native clock contract tested; browser timing remains unverified |
| 2026-10-16 | trivial | Web: preferences (theme, interface size, language, editing options…) are kept in the browser's `localStorage` between visits, as the desktop app keeps `ui.json` and `prefs.json`; before, a reload brought back light mode and the defaults (#1562) |
| 2026-10-15 | minor | Number-format named colors use their RGB values; indexed `[Color1]`–`[Color56]` resolve through the shared legacy palette in grid and PDF (#1452) |
| 2026-10-14 | minor | Spill-child formula bar uses the anchor formula with disabled theme text and blocks formula-bar edit entry; selection and recalculation restore ordinary behavior |
| 2026-10-14 | minor | PivotTable source-field list fills the pane width so its scrollbar stays at the list's right edge |
| 2026-10-13 | minor | Dragging a sheet tab with Ctrl held (Cmd or Option on macOS) drops a copy of the sheet where the tab is released, with the copy cursor (#971) |
| 2026-10-13 | trivial | More Colors… and Recent Colors (#928) |
| 2026-10-13 | minor | Grid drags keep their own cursor over selection borders, fill handles and outside the grid; hover resumes on release (#936) |
| 2026-10-13 | minor | AutoFill month patterns retain the workbook's date system, preventing shifted dates in 1904 workbooks when filling forward or backward |
| 2026-10-13 | minor | Sheet tab bar: the tab strip shrinks to its tabs, up to 60% of the bar, so New sheet (+) sits right after the last tab and the horizontal scrollbar takes the rest (part of #943) |
| 2026-10-13 | minor | Horizontal scrollbar in the sheet tab bar: drag, track click pages, frozen columns excluded from the page (#840) |
| 2026-10-13 | minor | Formula-entry Command/Ctrl-click adds arguments and Shift-click extends references; ordinary replacement and dragging retain their behavior. Readiness estimates unchanged. |
| 2026-10-12 | minor | Next/previous sheet navigation no longer activates hidden worksheets at either workbook boundary; command tests cover Hidden, VeryHidden and selection retention |
| 2026-10-12 | minor | The File view's recent list tells when each workbook was last changed, read off the UI thread |
| 2026-10-12 | minor | Ctrl+wheel zoom steps along a ladder of percentages about 20% apart that never skips 100%, so a notch out undoes a notch in (it drifted: 100 → 122 → 101); the step is taken at once (no easing); Excel steps 15 points, see decisions.md |
| 2026-10-12 | minor | Page Layout view: left/right/top/bottom margins are dragged on the rulers (#792) |
| 2026-10-12 | minor | Alt+Down opens the active filter header's existing popup, with validation priority and mirrored anchoring on right-to-left sheets (part of #441) |
| 2026-10-12 | minor | Paste from the ribbon (button and options menu), the cell and header context menus and the macOS menu bar's Edit menu pastes what is on the system clipboard, as Cmd/Ctrl+V does, instead of only GridCraft's own copy (the web build, which can't read the clipboard on request, still pastes GridCraft's copy). Copy and Cut by keytip (H,C / H,X) publish to the system clipboard like the ribbon buttons. After a control-channel or MCP copy, which leaves the system clipboard alone, Paste pastes that copy until other text is copied. Once a cut has been pasted, Paste (ribbon, menus or Cmd/Ctrl+V) doesn't paste the cut's clipboard text again over the moved cells (Excel pastes a cut once), and the Paste keytip (H,V,P) pastes the system clipboard like the button. Regression test for the Paste button click freeze in 0.5.0 (#273, fixed by #272); the Paste button gets an accessible "Paste" name for screen readers |
| 2026-10-11 | minor | AutoFit accounts for table and ordinary filter dropdowns, including low zoom; header text excludes the button area |
| 2026-10-11 | minor | Outline band for grouped rows and columns, Show / Hide Detail on the Data tab, numpad navigation keys in shortcuts (#373) |
| 2026-10-11 | minor | Shift+Space / Ctrl+Space no longer start an edit; Control+Space selects the column on Mac (#338) |
| 2026-10-11 | minor | Selected whole columns can be reordered by dragging their headers to an insertion point |
| 2026-10-11 | minor | Page Layout view with rulers and in-place headers and footers (#329) |
| 2026-10-11 | minor | Forecast Sheet dialog (#322) |
| 2026-10-11 | minor | Delete and Backspace now remove the selected chart, picture or shape (#213) |
| 2026-10-10 | minor | Keep formula helpers inside the worksheet when scrolling (#200) |
| 2026-10-11 | trivial | Theme: title-bar light/dark switch, File › Display theme, system appearance on first run (#194, #239) |
| 2026-10-11 | minor | Keyboard shortcut audit against Microsoft's Windows and Mac lists; missing shortcuts for existing features added (#110) |
| 2026-10-11 | minor | File tab Backstage view and File › Options (#174) |
| 2026-10-11 | minor | Selection outline glide and the Animations setting (both removed 10-24: the UI has no motion) |
| 2026-10-11 | major | Sequential Alt keytips: full-tab table (364 sequences), badges on every control, dropdowns open for typing/arrow editing, colour-palette arrows, real-Excel corrections (issues #33, #49) |
| 2026-10-10 | minor | Chart resize bug (#190) |
| 2026-10-10 | major | Created: chrome, grid, keyboard, dialogs, accessibility; key tips, Page Layout view and accessibility tree flagged |
