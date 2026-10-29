# Localization parity

> **Last reviewed:** 2026-10-30 · **Last updated:** 2026-10-30 · **Change:** major (PDF export embeds subset TrueType fonts, so non-Latin text exports) · **Target:** Microsoft Excel (Microsoft 365), Excel for Mac 16.113.4

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
  Leelawadee UI / DokChampa (Windows) or Noto Sans Lao / Phetsarath OT (Linux). There is no complex-text shaping: Arabic and
  Devanagari don't join or reorder, and there is no bidirectional layout. **Sheet Right-to-Left**
  (`view.rightToLeft`, Page Layout › Sheet Options, XLSX `rightToLeft`, `.xls` WINDOW2 fRightToLeft) mirrors the grid (#146):
  column A and the row headers on the right, the vertical scrollbar on the left; hit-testing,
  arrow keys, editor, popups and floating objects follow; text stays left-to-right inside its box
  (no bidi), and the sheet tabs, horizontal scrollbar and printing/PDF are not mirrored yet.
  Text boxes and shapes keep their paragraphs' right-to-left direction (`a:pPr rtl`) through XLSX
  save, though GridCraft still draws that text left to right. IME input goes
  through egui/winit and is unverified for Chinese, Japanese and Korean.
- **Names in any script.** Defined names (Define Name, Create from Selection) are limited to
  255 characters, counted as characters rather than UTF-8 bytes, so a 255-letter Arabic name is
  accepted as in Excel. Sheet names already counted characters (31).
- **PDF export** (M14) embeds a subset of a real TrueType font for text the 14 standard PDF fonts
  cannot encode: Cyrillic, Greek, Hebrew, CJK, Hangul, and the glyphs of Arabic, Devanagari and
  Thai (unshaped). The face is found through `gridcraft_core::fonts` (`CRAFT_FONTS_DIR`, then the
  platform's font directories) or an explicit `fontPath`, and the matrix below says which script
  works and what still goes wrong. Shaping and bidi are still missing, as on the grid.

## Languages

UI strings translated are measured counts against the ~2,200 estimate. Estimates to `full`
include the rest of the catalog work below (10–20 h, counted once), localized function names,
separators, number/date formats, machine translation with agent review, and script work.

| Language | Code | UI strings translated | Dialogs / tooltips / help | Script support | Native review | Status | Estimate to full |
|---|---|---|---|---|---|---|---|
| English | en | ~2,200 / ~2,200 (100%, source) | yes | Latin: yes | yes | full | — |
| Simplified Chinese (Mandarin) | zh-Hans | 266 (~12%) | no | CJK system-font fallback (#38); IME unverified; PDF embeds a subset (M14) | no | menus only | 8–13 h |
| Spanish | es | 0 (0%) | no | Latin: yes | no | none | 6–10 h |
| Hindi | hi | 0 (0%) | no | Devanagari: **no shaping** (PDF embeds the glyphs in stored order) | no | none | 12–18 h |
| Arabic | ar | 0 (0%) | no | **no shaping, no RTL UI**; Sheet Right-to-Left mirrors the grid (#146); PDF embeds the glyphs in stored order | no | none | 25–35 h |
| French | fr | 0 (0%) | no | Latin: yes | no | none | 6–10 h |
| Portuguese | pt-BR | 943 (~43%) | dialogs and tooltips (dialogs take effect with #669) | Latin: yes | no | menus, File view, dialogs | 2–4 h |
| Indonesian | id | 0 (0%) | no | Latin: yes | no | none | 5–8 h |
| Japanese | ja | 268 (~12%) | no | CJK system-font fallback; IME unverified; PDF embeds a subset (M14); no furigana display, but phonetic runs (ふりがな) survive an XLSX open + save | no | menus only | 8–12 h |
| Japanese | ja | 943 (~43%) | dialogs and messages | CJK system-font fallback; IME unverified; PDF embeds a subset (M14); no furigana display | no | menus, File view, dialogs, messages | 3–5 h |
| Japanese | ja | 268 (~12%) | no | CJK system-font fallback; the theme's East Asian typeface (`<a:ea>`) survives open + save (rendering does not use it yet); IME unverified; no furigana display | no | menus only | 8–12 h |
| German | de | 266 (~12%) | no | Latin: yes | no | menus only | 4–8 h |
| Italian | it | 971 (~44%) | dialogs, tooltips, pivot pane | Latin: yes | no | menus, File view, dialogs, pivot pane | 3–5 h |
| Korean | ko | 266 (~12%) | no | Hangul via the CJK fallback; IME unverified; PDF embeds a subset (M14) | no | menus only | 6–10 h |
| Vietnamese | vi | 0 (0%) | no | Latin with stacked diacritics: unverified | no | none | 6–10 h |
| Turkish | tr | 382 (~17%) | no | Latin: yes | yes | menus only | 4–8 h |

Other shipped languages: Russian (ru), German (de), Czech (cs), French (fr) and Turkish (tr), menus only.
| Vietnamese | vi | 398 (~18%) | no | Latin with stacked diacritics: unverified | no | menus, File view | 5–9 h |
Other shipped languages: Russian (ru) and German (de), 266 labels each (~12%), menus only; Dutch (nl), 518 labels (~24%): ribbon, context menus, File view, dialog titles and the Options page.

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
| Hebrew | `שלום` | yes, glyphs in the stored order | Segoe UI | no bidi: a Hebrew run inside Latin text prints in storage order, and RTL sheets still print left to right |
| Arabic | `مرحبا` | glyphs print, **not shaped** | Segoe UI | isolated forms only (no joining), no bidi |
| Devanagari | `हिन्दी` | glyphs print, **not shaped** | Nirmala UI | matras print in storage order (no reordering), no conjuncts |
| Thai | `สวัสดี` | glyphs print, **not shaped** | Leelawadee UI | no mark positioning above and below the base |
| CJK (Han) | `签字清单` | yes | Microsoft YaHei (`msyh.ttc`, face 0) | — |
| Kana | `こんにちは` | yes | Microsoft YaHei | — |
| Hangul | `안녕하세요` | yes | Malgun Gothic | — |
| Emoji / astral | `😀` | yes when a face with a monochrome glyph covers it; `ToUnicode` writes the surrogate pair | face dependent | a full-colour emoji font prints its outline form |

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
- **An ASCII/WinAnsi-only document is byte-for-byte what it was**, and setting `fontPath` on one
  changes nothing: the font program is written only when a page actually drew with it.
- **Hostile fonts are data, not code.** Every table offset is range-checked, `loca` must be
  non-decreasing and inside `glyf`, the glyph count is capped, composite glyphs are followed
  iteratively (a self-referencing composite terminates), and a malformed font makes the export
  fall back instead of failing. The parser is fuzzed by the truncation tests.
- **wasm** has no filesystem: font discovery returns an error, no face is embedded, and the
  document keeps the standard fonts and the CID fallback.
- Out of scope (the grid/editor shaping task owns it): shaping, bidi, per-run fallback, and
  mirrored printing for right-to-left sheets. **Needs a human:** native-speaker review of every
language (none so far) and of localized function names.

## Infrastructure (do first)

1. Extend the catalog from the ribbon to every user-visible string (dialogs, menus, task panes,
   status and error messages, command labels, function descriptions), with a missing-key check
   in `cargo xtask ci`. Language switching and the system-language default already exist (#31).
2. Locale-aware formulas: localized function-name tables and separators at the formula-bar
   boundary only (files keep en-US invariant names), locale number and date formats, list
   separators in CSV.
3. Text shaping and fallback: a shaping engine for grid, editor and PDF text (Arabic, Devanagari,
   Thai), system font fallback for CJK, bidirectional runs, right-to-left sheet tabs and printing.
   Embedded subset fonts in PDF landed in M14 (see below); shaping and bidi did not.

## Revision history

| Date | Change | Summary |
|---|---|---|
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
