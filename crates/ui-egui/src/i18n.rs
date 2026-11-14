//! Interface translations. Command ids and document text stay stable; only interface chrome is
//! translated. Untranslated labels fall back to English, so coverage can grow incrementally.
//!
//! Two interface languages ship: English, the source language every translation is keyed by, and
//! Simplified Chinese. The interface language is a preference ([`crate::UiState::language`]): on
//! first run it defaults to the system's language ([`Language::system`]), and the View tab
//! switches it by hand. A preference saved by a build that shipped more languages reads as English
//! ([`lenient`]), so losing a language never costs the rest of the preferences.
//!
//! CJK fonts stay system-provided: [`crate::theme`] appends available CJK faces to every family so
//! Chinese labels and workbook text can render without bundling font files.

use serde::{Deserialize, Serialize};

mod zh;
mod zh_functions;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    En,
    Zh,
}

impl Language {
    /// Every interface language: English first (the source language), then Simplified Chinese.
    pub const ALL: [Self; 2] = [Self::En, Self::Zh];

    /// The languages in the order the switcher lists them.
    pub fn for_picker() -> Vec<Self> {
        Self::ALL.to_vec()
    }

    /// The language's own name, shown in the switcher.
    pub fn name(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::Zh => "简体中文",
        }
    }

    /// The script name Excel's font scheme uses for this language's East Asian text
    /// (`<a:font script="Hans" typeface="宋体"/>` in a Chinese workbook's theme). Both shipped
    /// languages read Chinese glyph shapes: English only draws the CJK inside workbook text, and
    /// no shipped interface language asks for the Japanese (`Jpan`) or Korean (`Hang`) scheme.
    pub const fn east_asian_script(self) -> &'static str {
        match self {
            Self::En | Self::Zh => "Hans",
        }
    }

    /// The font Excel opens new workbooks with in this language: 等线 (DengXian) on a Chinese
    /// install, Calibri elsewhere. A name only: the renderer substitutes an installed face, and
    /// the value is what the file's Normal style carries.
    pub const fn default_cell_font(self) -> &'static str {
        match self {
            Self::Zh => "等线",
            Self::En => "Calibri",
        }
    }

    /// The language's code, as saved in `ui.json` and taken by `app.language.set`.
    pub const fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Zh => "zh",
        }
    }

    pub fn parse(code: &str) -> Option<Self> {
        Self::from_tag(code)
    }

    /// Every code `app.language.set` takes, quoted and joined (`"en"|"zh"`), for command docs
    /// and errors.
    pub fn code_list(separator: &str) -> String {
        Self::ALL.iter().map(|l| format!("\"{}\"", l.code())).collect::<Vec<_>>().join(separator)
    }

    /// A language code or a system tag (`zh-Hans-CN`, `en_US.UTF-8`); `None` for anything this
    /// build doesn't ship, including the codes of languages it used to.
    fn from_tag(tag: &str) -> Option<Self> {
        match tag.trim().split(['-', '_']).next()?.to_ascii_lowercase().as_str() {
            "en" => Some(Self::En),
            "zh" => Some(Self::Zh),
            _ => None,
        }
    }

    /// The system's preferred interface language, best effort; English when it is unknown or
    /// unsupported. Used as the first-run default while nothing is saved.
    pub fn system() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            sys_locale::get_locale().and_then(|tag| Self::from_tag(&tag)).unwrap_or(Self::En)
        }
        #[cfg(target_arch = "wasm32")]
        {
            web_sys::window().and_then(|window| window.navigator().language()).and_then(|tag| Self::from_tag(&tag)).unwrap_or(Self::En)
        }
    }

    /// The formula syntax this interface language reads and writes. Both shipped languages use
    /// the canonical English one — Chinese only translates the interface, and the engine, the
    /// files and the control channel always speak the canonical syntax.
    pub fn formula_locale(self) -> gridcraft_engine::formula::FormulaLocale {
        match self {
            Self::En | Self::Zh => gridcraft_engine::formula::FormulaLocale::En,
        }
    }

    /// Translates an interface label; untranslated labels fall back to English.
    ///
    /// A label that needs two translations (Home's text "Orientation" vs Page Layout's print
    /// "Orientation") is keyed `context|label`: English shows only the part after the `|`.
    pub fn tr(self, text: &str) -> &str {
        let translated = match self {
            Self::En => None,
            Self::Zh => zh::translate(text),
        };
        translated.unwrap_or_else(|| text.rsplit_once('|').map_or(text, |(_, label)| label))
    }

    pub(crate) fn tr_error(self, message: &str) -> String {
        let message = if message.starts_with("command `") { crate::clean_error(message) } else { message.to_owned() };

        let translated = self.tr(&message);
        let fallback = message.rsplit_once('|').map_or(message.as_str(), |(_, label)| label);

        if translated == fallback { message } else { translated.to_owned() }
    }

    /// A worksheet function's description in this language, looked up by the English text of the
    /// function catalog; `None` when untranslated, and the help shows the English.
    pub fn function_description(self, english: &str) -> Option<&'static str> {
        lookup(self.function_help()?.0, english)
    }

    /// A function argument name in this language, given as the catalog spells it without
    /// brackets or index (`number`, `logical_test`).
    pub fn function_argument(self, english: &str) -> Option<&'static str> {
        lookup(self.function_help()?.1, english)
    }

    /// The function help tables (descriptions, argument names) of the languages that have them.
    fn function_help(self) -> Option<(HelpTable, HelpTable)> {
        match self {
            Self::Zh => Some((zh_functions::DESCRIPTIONS, zh_functions::ARGUMENTS)),
            Self::En => None,
        }
    }
}

/// English text and its translation, sorted by the English.
type HelpTable = &'static [(&'static str, &'static str)];

/// The translation of `english` in a help table (binary search, so the table must stay sorted).
fn lookup(table: HelpTable, english: &str) -> Option<&'static str> {
    let i = table.binary_search_by(|(key, _)| (*key).cmp(english)).ok()?;
    table.get(i).map(|(_, translated)| *translated)
}

/// Deserializes a saved language leniently: an unknown or malformed value (a language removed in a
/// later version, a hand-edited `ui.json`) reads as English instead of failing the whole `UiState`,
/// which would reset every other preference with it.
pub fn lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Language, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Code(String),
        Other(serde::de::IgnoredAny),
    }
    Ok(match Raw::deserialize(d)? {
        Raw::Code(code) => Language::parse(&code).unwrap_or_default(),
        Raw::Other(_) => Language::default(),
    })
}

/// The language a saved `ui.json` asks for. The desktop app uses it to tell "saved English" from
/// "nothing usable saved" (first run, an older file): a missing `language` key is `None` and starts
/// from [`Language::system`], while a key that names a language this build no longer ships reads as
/// English the way [`lenient`] does, so a removed language never falls back to the system's.
pub fn saved_language(ui_json: &serde_json::Value) -> Option<Language> {
    let saved = ui_json.get("language")?.as_str()?;
    Some(Language::parse(saved).unwrap_or_default())
}

/// egui context-data key holding the language of the frame being drawn. Widgets that only get a
/// `Ui` (not the app) read it to translate their labels; [`set_current`] writes it once per frame.
fn lang_key() -> egui::Id {
    egui::Id::new("gridcraft.language")
}

/// The language the current frame is drawing in. `En` when unset.
pub fn current(ctx: &egui::Context) -> Language {
    ctx.data(|d| d.get_temp(lang_key())).unwrap_or_default()
}

/// Sets the language for the current frame; call once per frame before anything is drawn.
pub fn set_current(ctx: &egui::Context, language: Language) {
    ctx.data_mut(|d| d.insert_temp(lang_key(), language));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The placeholders (`{n}`, `{title}`) a label carries, in order.
    fn placeholders(text: &str) -> Vec<&str> {
        text.split('{').skip(1).filter_map(|rest| rest.split_once('}').map(|(name, _)| name)).collect()
    }

    /// The Simplified Chinese catalog as the compiler compiles it. `zh::translate` is a `match` on
    /// the English text, so reading the module's source is the only way to enumerate its entries
    /// without a second, hand-kept list that would drift from it. Every parsed pair is looked up
    /// through `translate`, so a parse that stops keeping up with the file fails loudly instead of
    /// quietly checking fewer strings.
    fn zh_catalog() -> Vec<(String, String)> {
        let source = include_str!("i18n/zh.rs");
        let bytes = source.as_bytes();
        let mut literals: Vec<String> = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                // A comment holds prose, not entries; a `//` inside a translation is part of the
                // literal the next arm copies whole.
                b'/' if bytes.get(i + 1) == Some(&b'/') => {
                    while i < bytes.len() && bytes[i] != b'\n' {
                        i += 1;
                    }
                }
                b'"' => {
                    let (text, end) = string_literal(source, i);
                    literals.push(text);
                    i = end;
                }
                _ => i += 1,
            }
        }
        assert_eq!(literals.len() % 2, 0, "zh.rs holds {} string literals, so one arm is not a `key => value` pair", literals.len());
        literals
            .chunks(2)
            .filter_map(|pair| match pair {
                [english, chinese] => Some((english.clone(), chinese.clone())),
                _ => None,
            })
            .collect()
    }

    /// The string literal starting at `start` (the opening quote), and the index just past it.
    fn string_literal(source: &str, start: usize) -> (String, usize) {
        let mut text = String::new();
        let mut chars = source[start + 1..].char_indices();
        while let Some((offset, c)) = chars.next() {
            match c {
                '"' => return (text, start + 2 + offset),
                '\\' => match chars.next().map(|(_, escaped)| escaped) {
                    Some('n') => text.push('\n'),
                    Some('t') => text.push('\t'),
                    Some('r') => text.push('\r'),
                    Some('0') => text.push('\0'),
                    Some('\\') => text.push('\\'),
                    Some('"') => text.push('"'),
                    other => panic!("zh.rs uses an escape this test doesn't read: {other:?}"),
                },
                c => text.push(c),
            }
        }
        panic!("zh.rs has an unterminated string literal");
    }

    /// What the filter menu and the save prompt say. An older installed build shipped these in
    /// English; they are the first two places a Chinese user meets, so a missing key here is a
    /// visible regression, not a polish item.
    #[test]
    fn the_filter_menu_and_the_save_prompt_speak_chinese() {
        for english in [
            "↑  Sort Ascending",
            "↓  Sort Descending",
            "Filter",
            "Search",
            "(Select All)",
            "(Select All Search Results)",
            "Only the first {n} values are listed.",
            "Clear Filter",
            "Apply",
            "Do you want to save the changes you made to {title}?",
            "Don't Save",
            "Save",
            "Cancel",
        ] {
            let text = zh::translate(english).unwrap_or_else(|| panic!("no Simplified Chinese for {english:?}"));
            assert!(text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)), "{english:?} is not Chinese: {text:?}");
        }
    }

    /// Help tables are sorted, unique, and keyed by text the function catalog still has.
    #[test]
    fn function_help_is_sorted_and_keyed_by_current_catalog_text() {
        // Lookups binary-search the tables. A key that no longer matches the catalog (an English
        // description reworded, an argument renamed) would quietly show English again.
        let catalog = gridcraft_engine::cmd::formulas::function_list();
        let descriptions: std::collections::HashSet<&str> = catalog.iter().filter_map(|f| f["description"].as_str()).collect();
        let arguments: std::collections::HashSet<&str> =
            catalog.iter().filter_map(|f| f["signature"].as_str()).flat_map(crate::formula_locale::argument_names).collect();
        for language in Language::ALL {
            let Some((description_table, argument_table)) = language.function_help() else { continue };
            for (table, catalog_text) in [(description_table, &descriptions), (argument_table, &arguments)] {
                assert!(table.windows(2).all(|pair| pair[0].0 < pair[1].0), "{language:?} function help is not sorted and unique");
                for (english, translated) in table {
                    assert!(catalog_text.contains(english), "{language:?} translates {english:?}, which the function catalog doesn't use");
                    assert!(!translated.is_empty(), "{language:?}: empty translation of {english:?}");
                }
            }
        }
    }

    /// Every entry of the Chinese catalog is there once, says something, and is what `translate`
    /// returns: a value that slipped onto its neighbour's key or was left empty shows on screen.
    #[test]
    fn every_chinese_entry_is_non_empty_unique_and_in_effect() {
        let catalog = zh_catalog();
        assert!(catalog.len() > 1000, "the source scan lost the catalog: {} entries", catalog.len());
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for (english, chinese) in &catalog {
            assert!(!english.is_empty(), "an entry with an empty English key");
            assert!(!chinese.is_empty(), "empty Simplified Chinese for {english:?}");
            assert_eq!(zh::translate(english), Some(chinese.as_str()), "the file and `translate` disagree on {english:?}");
            assert!(seen.insert(english), "{english:?} is translated twice");
            // `tr` returns a found translation as it is, so a `context|` kept in the value shows on
            // screen (File › Options once read "Options|Général").
            assert!(!chinese.contains('|'), "{english:?} => {chinese:?} keeps its context prefix");
            assert_eq!(placeholders(chinese), placeholders(english), "{english:?} => {chinese:?} lost a placeholder");
        }
    }

    /// Every command the engine registers draws its label through `tr` (the command list, the
    /// menus, the tooltips), so a command that lands without a Chinese label puts English on a
    /// Chinese screen. The labels come from the registry — a list here would drift from it — and
    /// the gap of untranslated ones may only shrink.
    #[test]
    fn the_engine_command_catalog_keeps_its_chinese() {
        // The gap when this guard landed: 148 of 362 registered labels, nearly all of them ribbon
        // items whose visible label the interface draws from its own table (Draw Border, Field
        // List, Subtotals). Translating one lowers this number; a command that lands without a
        // label raises it and fails here.
        const UNTRANSLATED_MAX: usize = 148;
        let specs = gridcraft_engine::cmd::command_specs();
        assert!(specs.len() > 300, "the command registry looks empty: {}", specs.len());
        let mut untranslated = Vec::new();
        for spec in specs {
            let english = spec.label;
            match zh::translate(english) {
                Some(chinese) => {
                    assert!(!chinese.is_empty(), "{} has an empty Simplified Chinese label", spec.id);
                    assert!(!chinese.contains('|'), "{}: {english:?} => {chinese:?} keeps a context prefix", spec.id);
                    assert_eq!(placeholders(chinese), placeholders(english), "{}: {english:?} => {chinese:?} lost a placeholder", spec.id);
                }
                None => untranslated.push(spec.id),
            }
        }
        assert!(
            untranslated.len() <= UNTRANSLATED_MAX,
            "{} of {} engine command labels have no Simplified Chinese (at most {UNTRANSLATED_MAX} may): {untranslated:?}",
            untranslated.len(),
            specs.len()
        );
        // A translated label is translated, not echoed: the count above would pass on entries that
        // give a label back unchanged.
        assert_ne!(Language::Zh.tr("Paste"), "Paste");
        assert_eq!(Language::Zh.tr("Paste"), "粘贴");
    }

    /// An English string no catalog knows renders as the English it is: the interface never shows
    /// a blank, a key or a translation of something else.
    #[test]
    fn unknown_text_falls_back_to_english() {
        for language in Language::ALL {
            assert_eq!(language.tr(""), "");
            assert_eq!(language.tr("Not a label any catalog has"), "Not a label any catalog has");
            assert_eq!(language.tr("Sheet1!A1"), "Sheet1!A1");
            assert!(!language.tr("Page Layout|Orientation").is_empty());
        }
        assert_eq!(Language::En.tr("Data"), "Data");
        assert_eq!(Language::En.tr("Page Layout|Orientation"), "Orientation");
        assert_eq!(Language::Zh.tr("Data"), "数据");
        assert_eq!(Language::Zh.tr("Page Layout|Orientation"), "纸张方向");
        assert_eq!(Language::Zh.tr_error("Printer Office: permission denied"), "Printer Office: permission denied");
        assert_eq!(Language::Zh.tr_error("Sheet|Unknown failure"), "Sheet|Unknown failure");
    }

    /// What the sheet tab menu, the AutoSave switch and the sheet-tab arrows say.
    #[test]
    fn sheet_tab_menu_and_autosave_labels_are_translated() {
        for label in [
            "Insert Sheet",
            "Delete",
            "Rename",
            "Move or Copy…",
            "Duplicate",
            "Tab Color",
            "No Color",
            "Hide",
            "Unhide…",
            "Protect Sheet…",
            "Unprotect Sheet…",
            "Select All Sheets",
            "AutoSave",
            "AutoSave: save after every change (workbooks saved to a file)",
            "Previous sheet",
            "Next sheet",
            "All sheets",
            "New sheet (⇧F11)",
        ] {
            assert_ne!(Language::Zh.tr(label), label, "no Simplified Chinese for {label:?}");
        }
        assert_eq!(Language::Zh.tr("Insert Sheet"), "插入工作表");
    }

    #[test]
    fn the_default_cell_font_follows_the_language() {
        // Excel's Chinese install creates workbooks in 等线 11; English uses Calibri.
        assert_eq!(Language::Zh.default_cell_font(), "等线");
        assert_eq!(Language::En.default_cell_font(), "Calibri");
    }

    /// A `ui.json` written by a build that shipped more languages still loads whole: a removed
    /// code costs its language, never the other preferences and never the file.
    #[test]
    fn a_language_this_build_no_longer_ships_loads_as_english() {
        for removed in ["ja", "de", "es", "el", "ko", "pt", "pt-BR", "cs", "fr", "sr", "tr", "pl", "it", "uk", "vi", "nl", "ro", "xx"] {
            let saved = format!(r#"{{"dark": true, "autocomplete": false, "language": "{removed}"}}"#);
            let ui: crate::UiState = serde_json::from_str(&saved).unwrap_or_else(|e| panic!("a saved {removed:?} must load: {e}"));
            assert_eq!(ui.language, Language::En, "{removed:?} reads as English");
            assert!(ui.dark, "{removed:?} kept the other preferences");
            assert!(!ui.autocomplete, "{removed:?} kept the other preferences");
            // The desktop app also reads the raw value to tell English from "nothing saved": a
            // code this build dropped is English, not the host's language.
            assert_eq!(saved_language(&serde_json::json!({"language": removed})), Some(Language::En), "{removed:?}");
        }
        // A missing key is still "nothing saved" (first run), which the desktop starts from the
        // system language; a value that isn't a code at all goes the same way as a removed one.
        assert_eq!(saved_language(&serde_json::json!({})), None);
        assert_eq!(saved_language(&serde_json::json!({"language": 7})), None);
        assert_eq!(saved_language(&serde_json::json!({"language": "zh"})), Some(Language::Zh));
        assert_eq!(saved_language(&serde_json::json!({"language": "en"})), Some(Language::En));
        assert_eq!(crate::UiState::default().language, Language::En, "the default never reads the host locale");
    }

    #[test]
    fn tags_reduce_to_a_supported_language() {
        assert_eq!(Language::from_tag("en_US"), Some(Language::En));
        assert_eq!(Language::from_tag("EN-gb"), Some(Language::En));
        assert_eq!(Language::parse("en"), Some(Language::En));
        assert_eq!(Language::from_tag("zh-Hans-CN"), Some(Language::Zh));
        assert_eq!(Language::from_tag("zh_cn.UTF-8"), Some(Language::Zh));
        assert_eq!(Language::parse("zh"), Some(Language::Zh));
        // Everything else is "unsupported", including the codes this build removed, so a system in
        // one of those languages starts in English instead of failing to start.
        for tag in [
            "ja-JP", "de-DE", "es_MX", "el-GR", "ko-KR", "pt-BR", "cs-CZ", "fr-FR", "sr-RS", "tr-TR", "pl-PL", "it-IT", "uk-UA", "vi-VN", "nl-NL",
            "ro-RO", "", "  ",
        ] {
            assert_eq!(Language::from_tag(tag), None, "{tag:?}");
        }
    }

    #[test]
    fn an_unset_language_preference_is_english_until_the_app_applies_the_system_one() {
        // Deserializing never reads the host locale (tests stay deterministic); the desktop app
        // applies `Language::system()` itself when `ui.json` names no usable language.
        let restored: crate::UiState = serde_json::from_str("{}").unwrap();
        assert_eq!(restored.language, Language::En);
    }

    #[test]
    fn language_commands_switch_and_persist() {
        let mut app = crate::SheetApp::new(gridcraft_engine::Session::default(), Default::default());
        for language in Language::ALL {
            app.run("app.language.set", serde_json::json!({"language": language.code()})).unwrap();
            assert_eq!(app.ui.language, language);
            let restored: crate::UiState = serde_json::from_str(&serde_json::to_string(&app.ui).unwrap()).unwrap();
            assert_eq!(restored.language, language);
        }
        app.run("app.language.set", serde_json::json!({"code": "zh"})).unwrap();
        assert_eq!(app.ui.language, Language::Zh, "`code` is an alias for `language`");
        assert!(app.run("app.language.set", serde_json::json!({})).is_err(), "no language is an error, not a dialog");
        let unknown = app.run("app.language.set", serde_json::json!({"language": "xx"})).unwrap_err().to_string();
        for l in Language::ALL {
            assert!(unknown.contains(&format!("\"{}\"", l.code())), "the error lists {:?}: {unknown}", l.code());
        }
        // A language this build no longer ships is simply unknown.
        assert!(app.run("app.language.set", serde_json::json!({"language": "ja"})).is_err());
        app.run("app.language.english", serde_json::json!({})).unwrap();
        assert_eq!(app.ui.language, Language::En);
        for l in Language::ALL {
            assert_eq!(Language::parse(l.code()), Some(l));
        }
    }

    #[test]
    fn picker_lists_english_first_then_chinese() {
        assert_eq!(Language::for_picker(), Language::ALL.to_vec());
        assert_eq!(Language::for_picker(), vec![Language::En, Language::Zh]);
        assert_eq!(Language::Zh.name(), "简体中文");
    }
}
