# Localization parity

> **Last reviewed:** 2026-10-30 · **Last updated:** 2026-10-30 · **Change:** major (text is shaped in **both** paths now: cell text on the grid and in the in-cell editor — Arabic joining, Devanagari reordering, Thai marks, mixed bidi runs and a visual caret, with a Devanagari fallback face — and PDF export, where `ToUnicode` follows each glyph's whole cluster so extraction returns the logical text and bidi runs are laid out in visual order) · **Target:** Microsoft Excel (Microsoft 365)

Per-language status against Excel, which ships its UI in dozens of languages (Windows language
packs cover 100+), with localized function names, argument separators, number and date formats,
right-to-left sheets and full complex-script support. Part of
[target-app-parity.md](target-app-parity.md). **Localization: ~10% ready, 100–165 h** (estimated).

## Where we are (measured from source on `main`, after the 2026-10-10 merges)

- **A first catalog exists, for the ribbon only.** `crates/ui-egui/src/i18n.rs` (merged 10-10 in
  #31, #104, #82; German added 10-11) adds an interface-language preference that defaults to the
  system language (closes #20) and a View-tab switcher, with English-keyed lookup tables:
  Japanese 268 labels, Simplified Chinese, Korean, Russian, Brazilian Portuguese, German and Czech 266
  each. The tables are used by
  the ribbon, sheet tabs and widgets (`ribbon.rs`, `tabs.rs`, `widgets.rs`), the Format Cells
  dialog, every dialog title and the shared OK/Cancel buttons; the other dialogs, menus,
  context menus, task panes, status messages and command labels are still English literals.
  Untranslated labels fall back to English.
- **Function help** (the argument hint, autocomplete, Insert Function and the Formulas-tab
  menus) follows the interface language through per-language tables keyed by the catalog's
  English text (`i18n/<lang>_functions.rs`): descriptions, and argument names from which the
  hint is assembled with the formula syntax's function name and separator. Spanish translates
  the 10 common functions it had help for (SUM, AVERAGE, IF…); other languages show the English
  help.
- **A first catalog exists.** `crates/ui-egui/src/i18n.rs` (merged 10-10 in #31, #104, #82; German
  added 10-11) adds an interface-language preference that defaults to the system language (closes
  #20) and a View-tab switcher, with English-keyed lookup tables: Simplified Chinese, Korean,
  Russian, German and Czech 266–518 labels each, Brazilian Portuguese and Japanese 943 each. The
  tables are used by the ribbon, sheet tabs and widgets (`ribbon.rs`, `tabs.rs`, `widgets.rs`), the
  Format Cells dialog, every dialog title and the shared OK/Cancel buttons; Japanese and Portuguese
  also cover the other dialog labels, message boxes, Backstage/File-view items, template names and
  native menus (Japanese, 10-17: `i18n/ja.rs` keeps the shared 398-label core and adds 545 of those
  strings). The remaining menus, context menus, task panes, status messages, command labels and
  function descriptions are still English literals. Untranslated labels fall back to English.
- **About 2,200 user-visible strings** in all (heuristic source scan: ~1,700 UI and command
  strings plus ~500 function descriptions), so the ribbon-only languages cover **~12%** and
  Japanese and Portuguese **~43%** (measured count / estimated total).
- **en-US only formulas and formats.** Function names, `,` argument separator, `.` decimal
  separator, en-US number and date formats (`crates/numfmt` says "en-US only"). Users ask for
  localized function names and `;` separators (#45). German with localized numbers and formulas
  is in PR #163 (open).
- **Script support.** System-font fallback for CJK, Thai and Hebrew landed in #38 (closes #32;
  #142 is still open pending confirmation). Native CJK fallback also probes Flatpak's read-only
  `/run/host/fonts`, `/run/host/local-fonts` and `/run/host/user-fonts` layouts, so installed
  Noto CJK and WenQuanYi faces can render Chinese without a broad filesystem grant. This does
  not change the installed-font catalogue or add bundled fonts. Lao falls back to Lao Sangam MN / Lao MN (macOS),
  Leelawadee UI / DokChampa (Windows) or Noto Sans Lao / Phetsarath OT (Linux). A Devanagari
  fallback face (`sys-devanagari`: Nirmala UI / Mangal on Windows, Kohinoor and Devanagari Sangam MN
  on macOS, Noto Sans Devanagari / Lohit / FreeSerif on Linux) joins them, so a Hindi cell draws
  instead of boxes. **Complex-script rendering on the grid and in the cell editor landed in M14**
  (`crates/ui-egui/src/text.rs` over `gridcraft_core::text`): Arabic joins with its contextual
  forms, Devanagari reorders and merges clusters, Thai marks are positioned, the Unicode
  bidirectional algorithm puts Latin and digit islands back in left-to-right order inside Hebrew
  or Arabic, general right-to-left cell text aligns right, neutral text on a right-to-left sheet
  follows the sheet, and the in-cell editor's caret and selection follow visual clusters while
  the buffer, the formula bar and tooltips stay logical. **Word-wrapped cells and chart labels are
  shaped since 11-02**: a wrapped paragraph is measured once, broken at spaces, hyphens and CJK
  boundaries, and each line is shaped on its own (bidi, joining and marks per line), with each line
  aligned on its own and general right-to-left wrapped text aligned right; the chart layout
  measures labels with the shaped width. Printable ASCII keeps egui's own wrap. **Shape and
  text-box text is shaped and wrapped since 11-02** (the same per-line engine, per-line alignment
  and the text box's outline halo), and the PDF draws shaped glyphs but still measures them
  unshaped.
  **Sheet Right-to-Left**
  (`view.rightToLeft`, Page Layout › Sheet Options, XLSX `rightToLeft`, `.xls` WINDOW2 fRightToLeft) mirrors the grid (#146):
  column A and the row headers on the right, the vertical scrollbar on the left; hit-testing,
  arrow keys, editor, popups and floating objects follow; cell text is shaped inside its box but
  the whole box is not re-anchored, and the sheet tabs, horizontal scrollbar and printing/PDF are
  not mirrored yet.
  Text boxes and shapes keep their paragraphs' right-to-left direction (`a:pPr rtl`) through XLSX
  save, though GridCraft still draws that text left to right. IME input goes
  through egui/winit and is unverified for Chinese, Japanese and Korean.
- **Names in any script.** Defined names (Define Name, Create from Selection) are limited to
  255 characters, counted as characters rather than UTF-8 bytes, so a 255-letter Arabic name is
  accepted as in Excel. Sheet names already counted characters (31).
- **PDF export** (M14) embeds a subset of a real TrueType font for text the 14 standard PDF fonts
  cannot encode, and shapes it with `gridcraft_core::text` (the shared L0 engine):
  Arabic joins, Devanagari is reordered, Thai marks are positioned and bidirectional text is laid
  out in visual order, with `TJ` adjustments and `Ts` rises on a single text matrix. `ToUnicode`
  maps each drawn glyph to its whole source cluster, so a ligature or conjunct extracts as all of
  its characters. The face is found through `gridcraft_core::fonts` (`CRAFT_FONTS_DIR`, then the
  platform's font directories) or an explicit `fontPath`, and the matrix below says which script
  works and what still goes wrong. WinAnsi-only text keeps its old bytes; the grid/editor text and
  mirrored printing for right-to-left sheets are still unshaped.

## Languages

UI strings translated are measured counts against the ~2,200 estimate. Estimates to `full`
include the rest of the catalog work below (10–20 h, counted once), localized function names,
separators, number/date formats, machine translation with agent review, and script work.

| Language | Code | UI strings translated | Dialogs / tooltips / help | Script support | Native review | Status | Estimate to full |
|---|---|---|---|---|---|---|---|
| English | en | ~2,200 / ~2,200 (100%, source) | yes | Latin: yes | yes | full | — |
| Simplified Chinese (Mandarin) | zh-Hans | 266 (~12%) | no | CJK system-font fallback (#38); IME unverified; PDF embeds a subset (M14) | no | menus only | 8–13 h |
| Spanish | es | 0 (0%) | no | Latin: yes | no | none | 6–10 h |
| Hindi | hi | 0 (0%) | no | Devanagari: shaped and reordered on screen (M14, `sys-devanagari` fallback face) and in PDF (M14, `cluster-exact ToUnicode`) | no | none | 12–18 h |
| Arabic | ar | 0 (0%) | no | Arabic joins and bidirectional runs draw in visual order on screen and in PDF (M14); **no RTL interface**; Sheet Right-to-Left mirrors the grid (#146) | no | none | 25–35 h |
| French | fr | 0 (0%) | no | Latin: yes | no | none | 6–10 h |
| Portuguese | pt-BR | 943 (~43%) | dialogs and tooltips (dialogs take effect with #669) | Latin: yes | no | menus, File view, dialogs | 2–4 h |
| Indonesian | id | 0 (0%) | no | Latin: yes | no | none | 5–8 h |
| Japanese | ja | 943 (~43%) | dialogs and messages | CJK system-font fallback; IME unverified; PDF embeds a subset (M14); no furigana display | no | menus, File view, dialogs, messages | 3–5 h |
| German | de | 266 (~12%) | no | Latin: yes | no | menus only | 4–8 h |
| Italian | it | 971 (~44%) | dialogs, tooltips, pivot pane | Latin: yes | no | menus, File view, dialogs, pivot pane | 3–5 h |
| Korean | ko | 266 (~12%) | no | Hangul via the CJK fallback; IME unverified; PDF embeds a subset (M14) | no | menus only | 6–10 h |
| Turkish | tr | 382 (~17%) | no | Latin: yes | yes | menus only | 4–8 h |
| Vietnamese | vi | 398 (~18%) | no | Latin with stacked diacritics: unverified | no | menus, File view | 5–9 h |

Other shipped languages, measured from the catalogs on 10-30 (entries, against a ~1,000-entry reference set): Romanian 991, Italian and Greek 965, Ukrainian and Polish 964, Japanese 963, Portuguese 959 — menus, File view, dialogs and messages; Czech 532, Dutch 513, Spanish 456, German 439, French 427, Serbian and Russian 426, Korean 425, Chinese 421, Vietnamese 400, Turkish 400 — menus and part of the dialogs.

**Total:** catalog work 10–20 h + languages 92–144 h ≈ **100–165 h**. Parallelizes almost fully
once the catalog covers dialogs and commands.

## PDF text (M14)

`file.exportPdf` and `file.print` take an optional `fontPath` (a `.ttf`/`.ttc` file, or a family
name) and, when any printed string falls outside WinAnsiEncoding, embed a **subset** of that face —
or, without `fontPath`, of the first face `gridcraft_core::fonts` finds that covers the script
(`CRAFT_FONTS_DIR`, then `%WINDIR%\Fonts` / `/System/Library/Fonts` / `/usr/share/fonts` and the
per-user font folders, scanned with plain `read_dir`; no fontconfig).

The PDF side reads the font itself (`head`, `hhea`, `maxp`, `hmtx`, `cmap` formats 0/4/6/12,
`loca`, `glyf` with composite glyphs, `name`, `post`, `OS/2`), keeps only the glyphs the document
uses, and writes a `CIDFontType2` with `FontFile2`, `/Identity-H`, a `/W` widths array taken from
`hmtx`, and a `ToUnicode` CMap so the text stays copyable and searchable. Layout measurement
(centring, right alignment, wrapping) uses the same `hmtx` advances for the characters the face
covers, and the standard-font table for the rest.

Measured on the Windows dev machine (2026-10-30). Each row was exported through `file.exportPdf`
and the resulting `ToUnicode` CMap and subset font were checked (the subset re-reads, and an
independent parser, `skrifa`, draws every glyph):

| Script | Sample | PDF result | Face used here | What still goes wrong |
| --- | --- | --- | --- | --- |
| Latin (WinAnsi) | `café €5 — “Q1”` | yes, in the standard 14 fonts; a WinAnsi-only document's bytes are unchanged | none embedded | — |
| Latin Extended-A/B | `Łódź ő` | yes | Segoe UI (subset) | — |
| Greek | `Ωμέγα` | yes | Segoe UI | — |
| Cyrillic | `Привет` | yes | Segoe UI | — |
| Hebrew | `שלום עולם 2024` | yes, shaped (bidi only); pdftotext returns the logical word | Segoe UI | a line that mixes Hebrew with Latin comes back in the extractor's run order (see below) |
| Arabic | `مرحبا بالعالم — لا 2024` | yes, joined and laid out right-to-left; pdftotext returns the logical line wrapped in U+202B/U+202A/U+202C | Segoe UI | a mixed Latin/Arabic line comes back in the extractor's run order |
| Devanagari | `नमस्ते दुनिया 2024` | yes, reordered with conjuncts and the i-matra in front; pdftotext returns the logical line | Nirmala UI | — |
| Thai | `สวัสดีชาวโลก 2024` | yes, marks positioned above/below the base; pdftotext returns the logical line | Leelawadee UI | — |
| CJK (Han) | `签字清单` | yes | Microsoft YaHei (`msyh.ttc`, face 0) | — |
| Kana | `こんにちは` | yes | Microsoft YaHei | — |
| Hangul | `안녕하세요` | yes | Malgun Gothic | — |
| Emoji / astral | `😀` | yes when a face with a monochrome glyph covers it; `ToUnicode` writes the surrogate pair | face dependent | a full-colour emoji font prints its outline form |

Measured with Xpdf's `pdftotext` 4.00 (UTF-8) on the 2026-10-30 dev machine. Before shaping,
Arabic came back reversed (`ابحرم`, the stored order read as visual) and Devanagari and Thai came
back with the marks detached; after shaping the samples above extract as the logical line. The
bidi controls around an RTL run are added by the extractor. A single-direction line extracts
exactly; a line that mixes an RTL script with Latin/digits is reordered by the extractor's own
bidi pass (Xpdf 4 re-applies the Unicode Bidi Algorithm to the visual string it read, which is not
invertible for mixed runs), so `Hello مرحبا 2024` extracts as `Hello 2024 ‫مرحبا‬` even though the
page is laid out as written. That is a reader limitation, not a loss of glyphs or characters.

Decisions and limits, also stated in the code:

- **One embedded font per document.** The face is chosen once, from the characters the printed
  sheets need (cells, spilled arrays, sheet names, drawing text, headers and footers): the
  candidate that covers the most of them among the faces for the scripts present, so a Cyrillic +
  Han document takes a face with both when one exists. A character the chosen face lacks keeps the
  previous non-embedded Adobe-GB1 CID fallback (`UniGB-UTF16-H`), which still needs a viewer with
  the Asian font pack: better than `?`, worse than an embedded face. A document mixing, say, Han
  and Arabic set in different fonts therefore embeds one and falls back for the other (no single
  face on the dev machine covers both).
- **Only TrueType outlines.** CFF/OTF faces are skipped (they would need a `CIDFontType0` and
  `FontFile3`); a `.ttc` collection is used from face 0.
- **WinAnsi-only text keeps its bytes.** A string with no character outside WinAnsiEncoding and
  no right-to-left base direction (`plain_ltr`) is written exactly as before, so a WinAnsi-only
  document stays byte-identical and setting `fontPath` on one changes nothing. Text that needs
  the embedded face is shaped; when the face cannot be shaped (the engine rejects it, the string
  is over its 4 MiB limit, or the writer's glyph cap is reached) the export falls back to the
  stored-order runs with a `log::warn`.
- **One glyph's extraction can need two CIDs.** `ToUnicode` maps the glyph that carries a cluster
  to the whole cluster's logical text (a ligature or conjunct is never reduced to one character),
  and the component glyphs of a multi-glyph cluster get no entry and width 0 so an extractor neither
  repeats the text nor sees a word gap. A glyph that extracts differently in two places gets two
  CIDs, and a `CIDToGIDMap` stream points them at one subset glyph; when every CID is its own
  glyph the map stays `/Identity`.
- **Placement does not move the text matrix.** One `Tm` starts a text item; the shaper's advances
  and mark offsets are `TJ` numbers (`Ts` for a vertical mark offset) on the `/W` widths and the
  `ToUnicode` CMap, so a viewer and an extractor both follow the shaped positions.
- **Measurement is still unshaped.** `text_width_with` sums per-character `hmtx` advances, so a
  centred, right-aligned or wrapped cell whose text shapes differently from its stored form is
  measured as if unshaped; the drawing itself is shaped. (Follow-up: measure through the engine.)
- **Hostile fonts are data, not code.** Every table offset is range-checked, `loca` must be
  non-decreasing and inside `glyf`, the glyph count is capped, composite glyphs are followed
  iteratively (a self-referencing composite terminates), and a malformed font makes the export
  fall back instead of failing. The parser is fuzzed by the truncation tests.
- **wasm** has no filesystem: font discovery returns an error, no face is embedded, and the
  document keeps the standard fonts and the CID fallback.
- Remaining: CJK system-font fallback for every script, mirrored printing and sheet tabs for
  right-to-left sheets, a shaped PDF `text_width`, and per-run font fallback. **Needs a human:** native-speaker review of every language (none so far) and of
  localized function names.

## Infrastructure (do first)

1. Extend the catalog from the ribbon to every user-visible string (dialogs, menus, task panes,
   status and error messages, command labels, function descriptions), with a missing-key check
   in `cargo xtask ci`. Language switching and the system-language default already exist (#31).
2. Locale-aware formulas: localized function-name tables and separators at the formula-bar
   boundary only (files keep en-US invariant names), locale number and date formats, list
   separators in CSV.
3. Text shaping and fallback: the shared `gridcraft_core::text` engine shapes cell text on the grid,
   in the in-cell editor and in PDF export (Arabic, Devanagari, Thai, bidi, cluster-exact extraction).
   Remaining: CJK system font fallback for every script, right-to-left sheet tabs and mirrored
   printing, a shaped PDF `text_width`, and per-run fallback.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-02 | minor | Word-wrapped cell text and chart labels are shaped: each paragraph is measured once from its shaped clusters, broken at spaces/hyphens/CJK boundaries, and each line is shaped and aligned on its own (RTL General text aligns right). Printable ASCII keeps the unshaped wrap bit for bit. Text boxes and the PDF's `text_width` remain unshaped |
| 2026-10-30 | major | PDF text is shaped with `gridcraft_core::text`: Arabic joins, Devanagari reorders, Thai marks are positioned and bidi runs are laid out in visual order (`TJ`/`Ts` on one text matrix); `ToUnicode` follows the glyph's whole cluster and a `CIDToGIDMap` is written only when one glyph must extract differently, so WinAnsi text stays byte-identical |
| 2026-10-30 | minor | Cell text on the grid and in the in-cell editor is shaped and bidi-ordered (`crates/ui-egui/src/text.rs`, one `gridcraft_core::text` engine per app, the faces egui paints with): Arabic joins, Devanagari reorders and merges clusters, Thai marks take zero advance, mixed runs draw in visual order and the caret walks visual clusters; a Devanagari fallback face makes Hindi cells draw |
| 2026-10-30 | major | PDF export embeds subset TrueType fonts (`crates/pdf/src/truetype.rs`, `gridcraft_core::fonts`): Cyrillic, Greek, Hebrew, CJK, Korean and the glyphs of Arabic, Devanagari and Thai now reach the PDF (unshaped, left to right) with `ToUnicode` so the text stays copyable; `file.exportPdf`/`file.print` take `fontPath`; a WinAnsi-only document's bytes are unchanged |
| 2026-10-21 | minor | Romanian interface (`ro`, Română): 970 labels covering every key of the other catalogs (`crates/ui-egui/src/i18n/ro.rs`), `ro-*` system locales, the View-tab switcher and `app.language.set`; context keys show no prefix (File › Options test). English formulas and formats; AI-assisted draft pending native-speaker review |
| 2026-10-20 | minor | Japanese: furigana (`rPh` phonetic runs and `<phoneticPr>` in `xl/sharedStrings.xml`) is no longer deleted by an open + save; it is kept on the workbook and written back (a `<phoneticPr>` without runs is kept too, while duplicate texts with different readings keep only the first, reported on open). Display of the readings is still missing (`crates/xlsx/src/tests/phonetic_tests.rs`) |
| 2026-10-20 | minor | Japanese catalog (`i18n/ja.rs`) covers every key the Brazilian Portuguese catalog translates: the 398-label shared core moved verbatim plus 545 dialog, message, Backstage and menu strings — 943 keys, ~12% → ~43%. A test parses `pt.rs`'s source, so the two catalogs can't drift apart again. Portuguese re-measured at 943 (was 923) |
| 2026-10-20 | minor | Dutch (nl) interface catalog (`i18n/nl.rs`, 518 labels, ~24%): the shared key set every language must cover (ribbon, context menus, File view, dialog titles, Options page), matching the German, French and Czech catalogs; `nl-NL`, `nl-BE` locale tags resolve to Dutch |
| 2026-10-20 | minor | XLSX: a theme's per-script typefaces (`<a:font script="Arab">`, Hebr, Jpan…) are kept through open + save, so Arabic Excel's theme fonts for shapes, charts and theme-font cells survive a GridCraft save |
| 2026-10-20 | trivial | XLSX: a workbook theme's major/minor East Asian (`<a:ea>`) and complex-script (`<a:cs>`) typefaces are kept through open + save, so a Japanese workbook keeps its theme's East Asian font; rendering does not resolve them yet (follow-up) |
| 2026-10-19 | minor | XLSX fonts retain charset and family metadata so systems can select a substitute font with the intended script when the named font is unavailable |
| 2026-10-19 | minor | Arabic CSV and text imports decode Windows-1256 text; Arabic shaping and bidirectional UI remain unsupported |
| 2026-10-19 | minor | Defined-name length counts characters, not bytes: Arabic names up to 255 letters are accepted, and Create from Selection keeps long Arabic headers |
| 2026-10-19 | minor | Table names are limited to 255 Unicode characters |
| 2026-10-19 | minor | Right-to-left sheets in legacy `.xls` files (common in older Arabic workbooks) open right-to-left |
| 2026-10-19 | trivial | Text boxes and shapes keep right-to-left paragraph direction (`a:pPr rtl`) on XLSX save, so Excel-authored Arabic and Hebrew text boxes reopen right to left |
| 2026-10-19 | minor | Save as Web Page and copied HTML mark right-to-left sheets `dir="rtl"`, let General-aligned Arabic and Hebrew text align by its own direction (`dir="auto"`), and keep numbers on right-to-left sheets left to right and on the left |
| 2026-10-19 | minor | French and Serbian kept the context in three `context\|label` translations ("Options\|Général", "Opcije\|Opšte" for `Options\|General`; also `Options\|Save`, `Sheet Options\|View`). `tr` shows a found translation as it is, so File › Options headings and the Sheet Options "View" checkboxes showed the prefix. Fixed; a test checks that no catalog shows a `\|` |
| 2026-10-17 | minor | Vietnamese catalog (`i18n/vi.rs`, every key in the master table); `vi` locale tags resolve to Vietnamese |
| 2026-10-17 | minor | Lao text falls back to an installed Lao face (`sys-lao`) instead of tofu boxes |
| 2026-10-15 | minor | Function help in the interface language: argument hints, autocomplete, Insert Function (search, categories) and the Formulas-tab menus read per-language tables of descriptions and argument names keyed by the catalog's English text; Spanish keeps its 10 functions in the new format |
| 2026-10-15 | minor | Italian catalog (`i18n/it.rs`, 971 entries, ~44%): every key the Brazilian Portuguese catalog translates plus the PivotTable Fields pane strings; `it` locale tags (it-IT, it-CH…) resolve to Italian. Labels follow Excel's Italian terminology; native-speaker review still pending |
| 2026-10-14 | minor | CJK fallback probes the read-only Flatpak host font mounts with the existing distribution layouts and TTC locale indices; no new sandbox permissions or bundled fonts |
| 2026-10-12 | minor | Turkish ribbon catalog (`i18n/tr.rs`, 382 labels); `tr` locale tags (tr-TR, tr_TR…) resolve to Turkish |
| 2026-10-12 | minor | Format Cells looks up its tabs, labels, options and note through `Language::tr` (Fill tab keyed `Format Cells\|Fill`); dialog titles and OK/Cancel too. Labels already in a table (Number, Currency, Fill…) show translated; the rest fall back to English until the tables add them |
| 2026-10-12 | minor | Brazilian Portuguese catalog (`i18n/pt.rs`) grows from 377 to 923 entries: every key the other catalogs translate, including the dialog strings #669 starts looking up (~42%) |
| 2026-10-11 | minor | German ribbon catalog (`i18n/de.rs`, 266 labels, ~12%); `de` locale tags (de-DE, de-AT, de-CH…) resolve to German. PR #163 (localized German formulas/numbers) still open |
| 2026-10-11 | minor | Sheet Right-to-Left mirrors the grid (#146) |
| 2026-10-11 | minor | Czech (cs) ribbon table: 266 labels (~12%), menus only |
| 2026-10-10 | minor | Re-measured after #31, #104, #82, #38 merged: ribbon translated into ja, zh, ko, pt-BR and ru (266–268 labels, ~12%), system-language default, CJK/Thai/Hebrew font fallback; ~10% ready, 100–165 h |
| 2026-10-10 | major | Created: English only, no catalog, ~2,200 strings, script-support gaps, open language PRs |
