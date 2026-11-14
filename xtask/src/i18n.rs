//! cargo xtask i18n [--check] [--write]: the user-visible string literals in the UI crate that do
//! not go through the translation lookup (tr(...)) — and, the other half of the same problem, every
//! label that *does* go through it but has no Chinese entry, so it still renders English — grouped
//! by file, with a committed baseline that
//! ratchets the count down.
//!
//! - a bare command lists them, per file, with line numbers and a total
//! - --write rewrites xtask/i18n-baseline.txt from the current tree
//! - --check fails if a literal outside the baseline appeared (a ci step)
//!
//! The scan is a small Rust lexer over the bytes, not a regex: comments, raw and byte strings, char
//! literals and lifetimes, #[cfg(test)] mod tests blocks and nested calls all come out right, and
//! nothing can panic on odd quoting or non-ASCII (no slicing, no unwrap). Each surface knows which
//! argument is the label, and the label argument is only followed through the wrappers text
//! actually arrives in (format!, RichText::new, ..), so an id, a JSON key or a version constant
//! next to the label is not mistaken for text.
//!
//! Known limit: a literal bound to a let first and passed along later (let tip = "Save"; then
//! ui.label(tip)) is not seen. The catalog grows in one direction, so those are translated by hand.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The committed baseline, relative to the workspace root.
pub const BASELINE: &str = "xtask/i18n-baseline.txt";

/// The crate the scan reads, relative to the workspace root.
const UI_SRC: &str = "crates/ui-egui/src";
/// The catalogue the Chinese build reads: every `"english" => "中文"` arm.
const ZH: &str = "crates/ui-egui/src/i18n/zh.rs";

/// How deep the tree walk may go. The UI source is a handful of directories deep; the cap keeps a
/// hostile layout from recursing forever.
const MAX_DEPTH: usize = 8;

/// The watched text surfaces: the call name the scan matches (on the last segment of the path
/// before the opening paren, so ui.label, egui::Button::new and egui::ComboBox::from_label all land
/// here) and which argument carries the label. egui's own signatures differ --
/// selectable_label(checked, text), selectable_value(current, value, text), radio(selected, text),
/// checkbox(checked, text), WidgetInfo::selected(typ, enabled, selected, label) -- which is what
/// keeps the value/id argument out of the report. Adding a user-facing widget means adding a row.
///
/// Every index here is checked against the egui the workspace pins (Cargo.toml: 0.36). A surface
/// whose argument is not text is deliberately absent, because a wrong index reports the argument
/// next to the label instead of the label: on_hover_ui / on_hover_ui_at_pointer take a closure (the
/// closure body is walked on its own, so the labels inside it are still found), text_edit_singleline
/// / text_edit_multiline take the buffer (the prompt is hint_text, below), and ComboBox::from_id_salt,
/// ComboBox::new's first argument and Popup::menu take an id salt or an anchor response.
///
/// The two WidgetInfo rows are the only surfaces that read an argument from position 2 and 3; both
/// names are unambiguous in this tree (no other call, method or setter is spelled labeled/selected).
const SURFACES: &[(&str, usize)] = &[
    ("label", 0),
    ("button", 0),
    ("link", 0),
    ("small_button", 0),
    ("selectable_label", 1),
    ("selectable_value", 2),
    ("menu_button", 0),
    ("checkbox", 1),
    ("radio", 1),
    ("radio_value", 2),
    ("toggle_value", 1),
    ("heading", 0),
    ("small", 0),
    ("weak", 0),
    ("on_hover_text", 0),
    ("on_hover_text_at_pointer", 0),
    ("on_disabled_hover_text", 0),
    ("tooltip_text", 0),
    ("hint_text", 0),
    ("labeled", 2),
    ("selected", 3),
    ("from_label", 0),
];

/// T::new(label) only counts for these widgets (a bare new is far too common elsewhere).
const CONSTRUCTORS: &[&str] = &["Label", "Button", "RichText", "Window", "MenuButton", "CollapsingHeader"];

/// Calls a label argument legitimately passes through: a label arrives as a string literal, a
/// format!(..) of one, or a RichText::new(..) wrapper. Anything else inside the argument -- a
/// field(n, "name") lookup, an element id, an env!(..) or json!(..) body -- is data, so the scan
/// does not descend into it. tr(..) is deliberately absent: what it wraps is already translated.
const TRANSPARENT: &[&str] = &[
    "format",
    "concat",
    "write",
    "writeln",
    "to_string",
    "into",
    "to_owned",
    "trim",
    "replace",
    "replace_n",
    "as_str",
    "to_lowercase",
    "to_uppercase",
    "fmt",
];

/// One user-visible literal that does not go through the lookup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    /// Workspace-relative path with forward slashes (the same on every platform, so the baseline is
    /// byte-identical on Windows and macOS).
    pub file: String,
    /// The line the literal starts on. A hint for the reviewer only: matching is by path and text.
    pub line: usize,
    pub text: String,
}

/// One row of the committed baseline: path:line, a tab, the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub file: String,
    pub line: usize,
    pub text: String,
}

/// What --check found: literals outside the baseline, and how many baseline rows no longer match
/// anything (the ratchet turning).
pub struct Diff {
    pub added: Vec<Hit>,
    pub gone: usize,
}

pub fn run(root: &Path, args: &[&str]) -> Result<(), String> {
    let mut write = false;
    let mut verify_only = false;
    for a in args {
        match *a {
            "--check" => verify_only = true,
            "--write" => write = true,
            other => {
                return Err(format!("i18n: unknown argument {other:?} (usage: cargo xtask i18n [--check | --write])"));
            }
        }
    }
    if write && verify_only {
        return Err("i18n: --check and --write are mutually exclusive".into());
    }
    let hits = scan_tree(root)?;
    if verify_only {
        return check(root, &hits);
    }
    print_report(&hits);
    let path = root.join(BASELINE);
    if !write {
        println!("\ni18n: {} untranslated literal(s) in {UI_SRC}; --write refreshes {}, --check verifies it", hits.len(), BASELINE);
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    let text = crate::eol::for_write(&path, &render(&hits));
    std::fs::write(&path, text).map_err(|e| format!("write {}: {e}", path.display()))?;
    println!("\ni18n: wrote {} ({} entries)", path.display(), hits.len());
    Ok(())
}

/// --check: every literal the scan finds is either translated or accepted in the baseline, and
/// every `tr("…")` key has a Chinese entry (a wrapped label without one still shows English).
fn check(root: &Path, hits: &[Hit]) -> Result<(), String> {
    let path = root.join(BASELINE);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e} (run cargo xtask i18n --write)", path.display()))?;
    let rows = parse_baseline(&text)?;
    println!("{}", verify(&rows, hits)?);
    let keys = scan_key_tree(root)?;
    let known = zh_keys(root)?;
    println!("{}", verify_keys(&keys, &known)?);
    Ok(())
}

/// One `tr("…")` call site: the key it looks up.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Key {
    pub file: String,
    pub line: usize,
    pub text: String,
}

/// Whether `zh.rs` answers `key`: the whole key, or the label half of a `context|label` key (which
/// is what `Language::tr` falls back to).
pub fn key_is_known(key: &str, known: &BTreeSet<String>) -> bool {
    known.contains(key) || key.rsplit_once('|').is_some_and(|(_, label)| known.contains(label))
}

/// The comparison behind --check for keys: every wrapped label has an entry, or the failure names it.
pub fn verify_keys(keys: &[Key], known: &BTreeSet<String>) -> Result<String, String> {
    let missing: Vec<&Key> = keys.iter().filter(|k| !key_is_known(&k.text, known)).collect();
    if !missing.is_empty() {
        let mut unique: BTreeMap<&str, &Key> = BTreeMap::new();
        for k in &missing {
            unique.entry(k.text.as_str()).or_insert(k);
        }
        let mut msg = format!(
            "{} translated label(s) have no Chinese entry in {}:
",
            unique.len(),
            ZH
        );
        for (text, k) in &unique {
            msg.push_str(&format!(
                "  {}:{}  {text}
",
                k.file, k.line
            ));
        }
        msg.push_str(
            "
Add the entry to zh.rs: a label that goes through tr() still shows English without one.",
        );
        return Err(msg);
    }
    Ok(format!("i18n: {} translated label(s), all with a Chinese entry", keys.len()))
}

/// The comparison behind --check, without the file: the summary line, or the list of new literals.
pub fn verify(baseline: &[Row], found: &[Hit]) -> Result<String, String> {
    let diff = reconcile(baseline, found);
    if !diff.added.is_empty() {
        let mut msg = format!("{} user-visible literal(s) are neither translated nor in {BASELINE}:\n", diff.added.len());
        for h in &diff.added {
            msg.push_str(&format!("  {}:{}  {}\n", h.file, h.line, h.text));
        }
        msg.push_str("\nPass them through tr(...), or accept the English text with cargo xtask i18n --write.");
        return Err(msg);
    }
    let stale = if diff.gone > 0 { format!(", {} row(s) gone (run --write to prune)", diff.gone) } else { String::new() };
    Ok(format!("i18n: {} untranslated literal(s), all in {BASELINE} ({} rows{stale})", found.len(), baseline.len()))
}

/// The grouped listing: one block per file, then a per-file count table.
fn print_report(hits: &[Hit]) {
    let mut by_file: BTreeMap<&str, Vec<&Hit>> = BTreeMap::new();
    for h in hits {
        by_file.entry(h.file.as_str()).or_default().push(h);
    }
    for (file, list) in &by_file {
        println!("\n{file} ({}):", list.len());
        for h in list {
            println!("  {:>5}  {}", h.line, h.text);
        }
    }
    println!("\n{} untranslated literal(s) in {} file(s):", hits.len(), by_file.len());
    for (file, list) in &by_file {
        let name = file.strip_prefix(UI_SRC).and_then(|s| s.strip_prefix('/')).unwrap_or(file);
        println!("  {:>5}  {name}", list.len());
    }
}

/// The baseline as committed: # comments and blank lines allowed, one path:line<TAB>text per entry,
/// sorted by path then line.
pub fn render(hits: &[Hit]) -> String {
    let mut s = String::from(HEADER);
    for h in hits {
        s.push_str(&format!("{}:{}\t{}\n", h.file, h.line, escape(&h.text)));
    }
    s
}

const HEADER: &str = "\
# Untranslated interface literals: the ratchet for cargo xtask i18n.
#
# One entry per line, path:line, a tab, then the text: a string in crates/ui-egui/src that reaches
# the user through a UI call (ui.label, on_hover_text, RichText::new, ...) without going through the
# translation lookup tr(...). The two shipped languages are English and Simplified Chinese, so every
# entry here is text that still reads English in the Chinese build.
#
# cargo xtask i18n lists them, --write refreshes this file, --check (a cargo xtask ci step) fails
# when a literal outside it appears. Deleting a row is the ratchet clicking down; adding one is a
# deliberate \"this stays English\" and belongs in the commit message with the reason. Rows are
# matched by path and text, so the line number is only a hint for the reviewer: an edit that shifts
# lines does not report the whole file as new.
";

/// Parse the baseline back. A malformed row is an error, not a silent skip: a baseline that quietly
/// stops parsing is a ratchet that quietly stops ratcheting.
pub fn parse_baseline(text: &str) -> Result<Vec<Row>, String> {
    let mut rows = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let at = format!("{BASELINE}:{}", n + 1);
        let line = raw.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((where_text, body)) = line.split_once('\t') else {
            return Err(format!("{at}: expected path:line, a tab, then the text"));
        };
        let Some((file, number)) = where_text.trim().rsplit_once(':') else {
            return Err(format!("{at}: expected path:line before the tab"));
        };
        let Ok(line_no) = number.trim().parse::<usize>() else {
            return Err(format!("{at}: {number:?} is not a line number"));
        };
        rows.push(Row { file: file.trim().to_string(), line: line_no, text: unescape(body) });
    }
    Ok(rows)
}

/// The comparison behind --check. An entry that disappeared is fine (the ratchet may only go down);
/// a new one is not.
pub fn reconcile(baseline: &[Row], found: &[Hit]) -> Diff {
    // Keyed by (path, text): a literal that moved down a file is the same entry, so an unrelated
    // edit at the top of a file does not report every entry in it as new.
    let mut keys: BTreeMap<(&str, &str), (Vec<usize>, Vec<&Hit>)> = BTreeMap::new();
    for r in baseline {
        keys.entry((r.file.as_str(), r.text.as_str())).or_default().0.push(r.line);
    }
    for h in found {
        keys.entry((h.file.as_str(), h.text.as_str())).or_default().1.push(h);
    }
    let mut added: Vec<Hit> = Vec::new();
    let mut gone = 0usize;
    for (_, (mut lines, hits)) in keys {
        lines.sort_unstable();
        let mut used = vec![false; hits.len()];
        for line in &lines {
            // Prefer the occurrence still on the baseline's line, so the one reported as new is the
            // one actually added; otherwise take any spare.
            let same_line = hits.iter().enumerate().position(|(k, h)| !used.get(k).copied().unwrap_or(true) && h.line == *line);
            match same_line.or_else(|| used.iter().position(|u| !*u)) {
                Some(k) => {
                    if let Some(slot) = used.get_mut(k) {
                        *slot = true;
                    }
                }
                None => gone += 1,
            }
        }
        for (k, h) in hits.iter().enumerate() {
            if !used.get(k).copied().unwrap_or(true) {
                added.push((*h).clone());
            }
        }
    }
    added.sort_by(|a, b| (&a.file, a.line, &a.text).cmp(&(&b.file, b.line, &b.text)));
    added.dedup();
    Diff { added, gone }
}

/// Every .rs file under crates/ui-egui/src, minus the catalog, the icon glyphs and the generated
/// keytip table, sorted so the report and the baseline are stable run to run.
fn scan_tree(root: &Path) -> Result<Vec<Hit>, String> {
    let mut hits = Vec::new();
    let mut keys = Vec::new();
    walk(&root.join(UI_SRC), "", 0, &mut hits, &mut keys)?;
    hits.sort_by(|a, b| (&a.file, a.line, &a.text).cmp(&(&b.file, b.line, &b.text)));
    hits.dedup();
    Ok(hits)
}

/// Every `tr("…")` call site in the UI crate, sorted and de-duplicated by (file, line, key).
fn scan_key_tree(root: &Path) -> Result<Vec<Key>, String> {
    let mut hits = Vec::new();
    let mut keys = Vec::new();
    walk(&root.join(UI_SRC), "", 0, &mut hits, &mut keys)?;
    keys.sort_by(|a, b| (&a.file, a.line, &a.text).cmp(&(&b.file, b.line, &b.text)));
    keys.dedup();
    Ok(keys)
}

/// The English keys `zh.rs` answers, from its `"…" =>` arms.
fn zh_keys(root: &Path) -> Result<BTreeSet<String>, String> {
    let path = root.join(ZH);
    let source = std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    Ok(zh_keys_of(&source))
}

/// The keys of a catalogue source: a string literal whose next token is `=>`.
pub fn zh_keys_of(source: &str) -> BTreeSet<String> {
    let src = source.as_bytes();
    let mut out = BTreeSet::new();
    let mut i = 0usize;
    while let Some(&b) = src.get(i) {
        if b == b'/' && src.get(i + 1) == Some(&b'/') {
            i = skip_line_comment(src, i);
            continue;
        }
        if b == b'/' && src.get(i + 1) == Some(&b'*') {
            i = skip_block_comment(src, i);
            continue;
        }
        if b == b'"'
            && let Some((from, to, after)) = string_end(src, i)
        {
            let mut next = skip_space(src, after);
            if src.get(next) == Some(&b'=') && src.get(next + 1) == Some(&b'>') {
                next += 2;
                if let Some(text) = src.get(from..to).and_then(|s| std::str::from_utf8(s).ok()) {
                    out.insert(text.to_string());
                }
            }
            i = after.max(i + 1);
            continue;
        }
        i += 1;
    }
    out
}

fn walk(dir: &Path, rel: &str, depth: usize, out: &mut Vec<Hit>, keys: &mut Vec<Key>) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Ok(());
    }
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read dir {}: {e}", dir.display()))?;
    let mut names: Vec<String> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("read dir {}: {e}", dir.display()))?;
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    for name in names {
        let rel_path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        if skip_path(&rel_path) {
            continue;
        }
        let full = dir.join(&name);
        let kind = match std::fs::symlink_metadata(&full) {
            Ok(meta) => meta.file_type(),
            Err(e) => return Err(format!("stat {}: {e}", full.display())),
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            walk(&full, &rel_path, depth + 1, out, keys)?;
            continue;
        }
        if !name.ends_with(".rs") {
            continue;
        }
        let source = std::fs::read_to_string(&full).map_err(|e| format!("read {}: {e}", full.display()))?;
        let file = format!("{UI_SRC}/{rel_path}");
        out.extend(scan_source(&file, &source));
        keys.extend(scan_keys_source(&file, &source));
    }
    Ok(())
}

/// Files the scan never reads: the catalog itself, the icon glyph names and the generated keytip
/// table are not interface text.
fn skip_path(rel: &str) -> bool {
    matches!(rel, "i18n.rs" | "icons.rs") || rel.starts_with("i18n/") || rel.starts_with("keytips/keytip_table/")
}

/// Every `tr("…")` call site in one file's source: the key it looks up, or nothing when the
/// argument is not a lone literal (a variable, a `format!`, a `match` arm).
pub fn scan_keys_source(file: &str, source: &str) -> Vec<Key> {
    let src = source.as_bytes();
    let lines = newline_offsets(src);
    let mut out = Vec::new();
    let mut i = 0usize;
    while let Some(&b) = src.get(i) {
        if b == b'/' && src.get(i + 1) == Some(&b'/') {
            i = skip_line_comment(src, i);
            continue;
        }
        if b == b'/' && src.get(i + 1) == Some(&b'*') {
            i = skip_block_comment(src, i);
            continue;
        }
        // A literal is not a call, and a literal may spell "tr(" itself.
        if b == b'"' {
            match string_end(src, i) {
                Some((_, _, after)) => i = after.max(i + 1),
                None => break,
            }
            continue;
        }
        if b == b'\'' {
            i = char_end(src, i).unwrap_or(i + 1);
            continue;
        }
        if let Some((_, _, after)) = raw_string(src, i) {
            i = after.max(i + 1);
            continue;
        }
        if is_ident_start(b) {
            let (segments, after_path) = read_path(src, i);
            if segments.last() == Some(&"tr") {
                let mut open = skip_space(src, after_path);
                if src.get(open) == Some(&b'!') {
                    open = skip_space(src, open + 1);
                }
                if src.get(open) == Some(&b'(') {
                    // The label is the call's last argument: tr("x") and tr(ui, "x") both hold it
                    // there, and a template built by a match is not a literal at all.
                    let args = split_args(src, open);
                    if let Some(&(start, end)) = args.last() {
                        let start = skip_space(src, start);
                        if src.get(start) == Some(&b'"')
                            && let Some((from, to, after)) = string_end(src, start)
                            && after <= end
                            && let Some(text) = src.get(from..to).and_then(|s| std::str::from_utf8(s).ok())
                        {
                            out.push(Key { file: file.to_string(), line: line_at(&lines, from), text: text.to_string() });
                        }
                    }
                    i = skip_balanced(src, open, b'(', b')').max(i + 1);
                    continue;
                }
            }
            i = after_path.max(i + 1);
            continue;
        }
        i += 1;
    }
    out
}

/// Scan one file's source. The file name only labels the hits.
pub fn scan_source(file: &str, source: &str) -> Vec<Hit> {
    let src = source.as_bytes();
    let lines = newline_offsets(src);
    let mut hits: Vec<Hit> = Vec::new();
    let mut i = 0usize;
    // #[cfg(test)] and mod tests arm this; the next block is then skipped whole.
    let mut skip_block = false;
    while let Some(&b) = src.get(i) {
        if b == b'/' && src.get(i + 1) == Some(&b'/') {
            i = skip_line_comment(src, i);
            continue;
        }
        if b == b'/' && src.get(i + 1) == Some(&b'*') {
            i = skip_block_comment(src, i);
            continue;
        }
        if b == b'"' {
            i = string_end(src, i).map_or(src.len(), |t| t.2);
            continue;
        }
        if b == b'\'' {
            i = char_end(src, i).unwrap_or(i + 1);
            continue;
        }
        if let Some(t) = raw_string(src, i) {
            i = t.2;
            continue;
        }
        if b == b'#' && src.get(i + 1) == Some(&b'[') {
            let end = attribute_end(src, i);
            if src.get(i..end).and_then(|s| std::str::from_utf8(s).ok()).is_some_and(is_test_marker) {
                skip_block = true;
            }
            i = end.max(i + 1);
            continue;
        }
        if b == b'{' {
            if skip_block {
                i = skip_balanced(src, i, b'{', b'}');
                skip_block = false;
            } else {
                i += 1;
            }
            continue;
        }
        if b == b';' {
            skip_block = false;
            i += 1;
            continue;
        }
        if is_ident_start(b) {
            let (segments, after_path) = read_path(src, i);
            let next = skip_space(src, after_path);
            if segments.as_slice() == ["mod"] {
                let (name, after_name) = read_path(src, next);
                if name.as_slice() == ["tests"] {
                    skip_block = true;
                }
                i = after_name.max(i + 1);
                continue;
            }
            if let Some(arg) = surface_arg(&segments) {
                if src.get(next) != Some(&b'(') || is_definition(src, i) {
                    i = after_path.max(i + 1);
                    continue;
                }
                let args = split_args(src, next);
                if let Some((from, to)) = args.get(arg).copied() {
                    literals_in(file, src, &lines, from, to, &mut hits);
                }
                // Step just past the opening paren: a closure taken as an argument is still walked,
                // and its own calls are found on their own (the overlap is deduplicated below).
                i = next + 1;
                continue;
            }
            i = after_path.max(i + 1);
            continue;
        }
        i += 1;
    }
    hits.sort_by(|a, b| (a.line, &a.text).cmp(&(b.line, &b.text)));
    hits.dedup();
    hits
}

/// The literals inside one label argument, descending only through the wrappers text arrives in
/// (see TRANSPARENT): a call the scan does not recognise holds data, so its arguments are skipped.
fn literals_in(file: &str, src: &[u8], lines: &[usize], start: usize, end: usize, out: &mut Vec<Hit>) {
    let end = end.min(src.len());
    let mut i = start;
    while i < end {
        let Some(&b) = src.get(i) else { break };
        if b == b'/' && src.get(i + 1) == Some(&b'/') {
            i = skip_line_comment(src, i).min(end).max(i + 1);
            continue;
        }
        if b == b'/' && src.get(i + 1) == Some(&b'*') {
            i = skip_block_comment(src, i).min(end).max(i + 1);
            continue;
        }
        // An index is a key, not text: w["value"] names a field.
        if b == b'[' {
            i = skip_balanced(src, i, b'[', b']').min(end).max(i + 1);
            continue;
        }
        if b == b'"' {
            match string_end(src, i) {
                Some((from, to, after)) => {
                    push_literal(file, src, lines, from, to, out);
                    i = after.min(end).max(i + 1);
                }
                // Unterminated: nothing after it can be trusted.
                None => return,
            }
            continue;
        }
        if b == b'\'' {
            i = char_end(src, i).unwrap_or(i + 1);
            continue;
        }
        if let Some((from, to, after)) = raw_string(src, i) {
            push_literal(file, src, lines, from, to, out);
            i = after.min(end).max(i + 1);
            continue;
        }
        if is_ident_start(b) {
            let (segments, after_path) = read_path(src, i);
            let mut next = skip_space(src, after_path);
            // A macro invocation: format!(..) and concat!(..) build text, json!(..) and env!(..) do
            // not.
            if src.get(next) == Some(&b'!') {
                next = skip_space(src, next + 1);
            }
            if src.get(next) == Some(&b'(') && !is_transparent(&segments) {
                i = skip_balanced(src, next, b'(', b')').min(end).max(i + 1);
                continue;
            }
            i = after_path.max(i + 1);
            continue;
        }
        i += 1;
    }
}

fn push_literal(file: &str, src: &[u8], lines: &[usize], from: usize, to: usize, out: &mut Vec<Hit>) {
    let Some(text) = src.get(from..to).and_then(|s| std::str::from_utf8(s).ok()) else { return };
    if is_user_text(text) {
        out.push(Hit { file: file.to_string(), line: line_at(lines, from), text: text.to_string() });
    }
}

/// "This literal is interface text": it has letters, and it is not a cell address ("A1",
/// "$B$2:$C$3"), a layout/size/colour constant ("+", "0.00") or an element id ("notes_pane").
fn is_user_text(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() || !has_words(t) {
        return false;
    }
    !is_cell_reference(t) && !looks_like_id(t)
}

/// Whether the literal has a word of its own, ignoring format placeholders and lone glyphs:
/// "{n}:", "{sheet}!{cell}" and the "ƒ" of a formula chip are decoration, "Version {version}" is a
/// label. Two letters in a row is the bar; a single-letter or single-glyph label is never translated.
fn has_words(text: &str) -> bool {
    let mut depth = 0usize;
    let mut run = 0usize;
    for c in text.chars() {
        match c {
            '{' => {
                depth = depth.saturating_add(1);
                run = 0;
            }
            '}' => {
                depth = depth.saturating_sub(1);
                run = 0;
            }
            other if depth == 0 && other.is_alphabetic() => {
                run += 1;
                if run >= 2 {
                    return true;
                }
            }
            _ => run = 0,
        }
    }
    false
}

fn is_cell_reference(text: &str) -> bool {
    let parts: Vec<&str> = text.split(':').collect();
    !parts.is_empty() && parts.len() <= 2 && parts.iter().all(|p| is_cell_part(p))
}

fn is_cell_part(part: &str) -> bool {
    let (mut letters, mut digits) = (0usize, 0usize);
    for c in part.chars() {
        if c == '$' {
            continue;
        }
        if c.is_ascii_alphabetic() {
            letters += 1;
        } else if c.is_ascii_digit() {
            digits += 1;
        } else {
            return false;
        }
    }
    (1..=3).contains(&letters) && (1..=7).contains(&digits)
}

/// snake_case element ids are not text: no spaces, all lowercase, at least one underscore.
fn looks_like_id(text: &str) -> bool {
    text.contains('_') && text.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Which argument carries the label, when the call is one of the watched surfaces.
fn surface_arg(segments: &[&str]) -> Option<usize> {
    let last = segments.last().copied()?;
    if let Some((_, arg)) = SURFACES.iter().find(|(name, _)| *name == last) {
        return Some(*arg);
    }
    if last == "new" && segments.len() >= 2 && segments.get(segments.len() - 2).is_some_and(|s| CONSTRUCTORS.contains(s)) {
        return Some(0);
    }
    None
}

/// Whether a call (or macro) is a wrapper a label passes through unchanged (see TRANSPARENT).
fn is_transparent(segments: &[&str]) -> bool {
    let Some(last) = segments.last().copied() else { return false };
    if TRANSPARENT.contains(&last) {
        return true;
    }
    let receiver = segments.len().checked_sub(2).and_then(|i| segments.get(i)).copied();
    matches!(
        (receiver, last),
        (Some("RichText" | "Label" | "Button" | "Window" | "MenuButton" | "CollapsingHeader"), "new") | (Some("String" | "str"), "from")
    )
}

/// The top-level arguments of the call whose opening paren is at open.
fn split_args(src: &[u8], open: usize) -> Vec<(usize, usize)> {
    let mut args: Vec<(usize, usize)> = Vec::new();
    let mut start = open + 1;
    let mut depth = 0usize;
    let mut j = start;
    while let Some(&b) = src.get(j) {
        if b == b'/' && src.get(j + 1) == Some(&b'/') {
            j = skip_line_comment(src, j).max(j + 1);
            continue;
        }
        if b == b'/' && src.get(j + 1) == Some(&b'*') {
            j = skip_block_comment(src, j).max(j + 1);
            continue;
        }
        if b == b'"' {
            j = match string_end(src, j) {
                Some(t) => t.2,
                None => break,
            };
            continue;
        }
        if b == b'\'' {
            j = char_end(src, j).unwrap_or(j + 1);
            continue;
        }
        if let Some(t) = raw_string(src, j) {
            j = t.2;
            continue;
        }
        match b {
            b'(' | b'[' | b'{' => {
                depth = depth.saturating_add(1);
                j += 1;
            }
            b')' | b']' | b'}' => {
                if depth == 0 {
                    args.push((start, j));
                    return args;
                }
                depth = depth.saturating_sub(1);
                j += 1;
            }
            b',' if depth == 0 => {
                args.push((start, j));
                start = j + 1;
                j += 1;
            }
            _ => j += 1,
        }
    }
    args.push((start, src.len()));
    args
}

/// A fn definition, not a call: fn label(&mut self, text: &str) is not a surface.
fn is_definition(src: &[u8], start: usize) -> bool {
    let mut j = start;
    while j > 0 && src.get(j - 1).is_some_and(|b| b.is_ascii_whitespace()) {
        j -= 1;
    }
    let end = j;
    while j > 0 && src.get(j - 1).is_some_and(|b| is_ident_continue(*b)) {
        j -= 1;
    }
    src.get(j..end).and_then(|s| std::str::from_utf8(s).ok()) == Some("fn")
}

/// #[cfg(test)], #[cfg(all(test, ..))], #[cfg_attr(test, ..)] and #[test] all mean the item beside
/// them is test-only.
fn is_test_marker(attr: &str) -> bool {
    let squeezed: String = attr.chars().filter(|c| !c.is_whitespace()).collect();
    if !squeezed.starts_with("#[") {
        return false;
    }
    squeezed.starts_with("#[test]")
        || squeezed.starts_with("#[cfg(test)")
        || squeezed.starts_with("#[cfg(all(test")
        || squeezed.starts_with("#[cfg(any(test")
        || squeezed.starts_with("#[cfg_attr(test")
}

fn newline_offsets(src: &[u8]) -> Vec<usize> {
    let mut v = Vec::new();
    for (i, b) in src.iter().enumerate() {
        if *b == b'\n' {
            v.push(i);
        }
    }
    v
}

fn line_at(lines: &[usize], offset: usize) -> usize {
    lines.partition_point(|p| *p < offset) + 1
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn skip_space(src: &[u8], mut i: usize) -> usize {
    while src.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
        i += 1;
    }
    i
}

/// Read an a::b::c path: the identifier segments and the index just past the last one.
fn read_path(src: &[u8], start: usize) -> (Vec<&str>, usize) {
    let mut segments: Vec<&str> = Vec::new();
    let mut i = start;
    loop {
        let begin = i;
        if !src.get(i).is_some_and(|b| is_ident_start(*b)) {
            return (segments, i);
        }
        i += 1;
        while src.get(i).is_some_and(|b| is_ident_continue(*b)) {
            i += 1;
        }
        match src.get(begin..i).and_then(|s| std::str::from_utf8(s).ok()) {
            Some(text) => segments.push(text),
            None => return (segments, i),
        }
        let after = skip_space(src, i);
        if src.get(after) == Some(&b':') && src.get(after + 1) == Some(&b':') {
            let next = skip_space(src, after + 2);
            if src.get(next).is_some_and(|b| is_ident_start(*b)) {
                i = next;
                continue;
            }
        }
        return (segments, i);
    }
}

fn skip_line_comment(src: &[u8], mut i: usize) -> usize {
    while let Some(b) = src.get(i) {
        if *b == b'\n' {
            return i;
        }
        i += 1;
    }
    i
}

fn skip_block_comment(src: &[u8], i: usize) -> usize {
    let mut depth = 0usize;
    let mut j = i;
    while let Some(b) = src.get(j) {
        if *b == b'/' && src.get(j + 1) == Some(&b'*') {
            depth = depth.saturating_add(1);
            j += 2;
            continue;
        }
        if *b == b'*' && src.get(j + 1) == Some(&b'/') {
            depth = depth.saturating_sub(1);
            j += 2;
            if depth == 0 {
                return j;
            }
            continue;
        }
        j += 1;
    }
    j
}

/// A plain string literal: the content range and the index just past the closing quote.
fn string_end(src: &[u8], i: usize) -> Option<(usize, usize, usize)> {
    if src.get(i) != Some(&b'"') {
        return None;
    }
    let mut j = i + 1;
    while let Some(b) = src.get(j) {
        match *b {
            b'\\' => j = j.saturating_add(2),
            b'"' => return Some((i + 1, j, j + 1)),
            _ => j += 1,
        }
    }
    None
}

/// r"..", r#".."# (any number of hashes) and their byte-string forms.
fn raw_string(src: &[u8], i: usize) -> Option<(usize, usize, usize)> {
    // An r inside a longer identifier (rect, raw_string) is not a raw-string start.
    if i > 0 && src.get(i - 1).is_some_and(|b| is_ident_continue(*b)) {
        return None;
    }
    let mut j = i;
    if src.get(j) == Some(&b'b') {
        j += 1;
    }
    if src.get(j) != Some(&b'r') {
        return None;
    }
    j += 1;
    let mut hashes = 0usize;
    while src.get(j) == Some(&b'#') {
        hashes += 1;
        j += 1;
    }
    if src.get(j) != Some(&b'"') {
        return None;
    }
    let from = j + 1;
    let mut k = from;
    while let Some(b) = src.get(k) {
        if *b == b'"' {
            let mut seen = 0usize;
            while seen < hashes && src.get(k + 1 + seen) == Some(&b'#') {
                seen += 1;
            }
            if seen == hashes {
                return Some((from, k, k + 1 + hashes));
            }
        }
        k += 1;
    }
    None
}

/// How many bytes the UTF-8 character starting with b takes, or 0 when b cannot start one.
fn utf8_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else if b >> 3 == 0b11110 {
        4
    } else {
        0
    }
}

/// 'a', with a non-ASCII character too, and escapes like the newline one: the index just past the
/// closing quote. None for a lifetime ('a, 'static), which the caller steps past one byte at a time.
fn char_end(src: &[u8], i: usize) -> Option<usize> {
    if src.get(i) != Some(&b'\'') {
        return None;
    }
    if src.get(i + 1) == Some(&b'\\') {
        // An escape: the closing quote follows the escape body (a newline, a unicode brace, ..).
        let mut j = i + 2;
        while j < i + 16 {
            match src.get(j) {
                Some(b'\'') => return Some(j + 1),
                Some(_) => j += 1,
                None => return None,
            }
        }
        return None;
    }
    let len = src.get(i + 1).copied().map(utf8_len).unwrap_or(0);
    if len > 0 && src.get(i + 1 + len) == Some(&b'\'') {
        return Some(i + 2 + len);
    }
    None
}

/// The index just after the bracket closing the attribute opening at i (a hash).
fn attribute_end(src: &[u8], i: usize) -> usize {
    let mut depth = 0usize;
    let mut j = i;
    while let Some(b) = src.get(j) {
        match *b {
            b'[' => {
                depth = depth.saturating_add(1);
                j += 1;
            }
            b']' => {
                depth = depth.saturating_sub(1);
                j += 1;
                if depth == 0 {
                    return j;
                }
            }
            b'"' => j = string_end(src, j).map_or(src.len(), |t| t.2),
            _ => j += 1,
        }
    }
    src.len()
}

/// The index just past the delimiter matching the one at i, skipping strings, comments and char
/// literals. src.len() when the source ends first, so callers can clamp and carry on.
fn skip_balanced(src: &[u8], i: usize, open: u8, close: u8) -> usize {
    let mut depth = 0usize;
    let mut j = i;
    while let Some(&b) = src.get(j) {
        if b == b'/' && src.get(j + 1) == Some(&b'/') {
            j = skip_line_comment(src, j);
            continue;
        }
        if b == b'/' && src.get(j + 1) == Some(&b'*') {
            j = skip_block_comment(src, j);
            continue;
        }
        if b == b'"' {
            j = match string_end(src, j) {
                Some(t) => t.2,
                None => return src.len(),
            };
            continue;
        }
        if b == b'\''
            && let Some(after) = char_end(src, j)
        {
            j = after;
            continue;
        }
        if let Some(t) = raw_string(src, j) {
            j = t.2;
            continue;
        }
        if b == open {
            depth = depth.saturating_add(1);
            j += 1;
            continue;
        }
        if b == close {
            depth = depth.saturating_sub(1);
            j += 1;
            if depth == 0 {
                return j;
            }
            continue;
        }
        j += 1;
    }
    src.len()
}

/// A real tab, newline or carriage return in the text is escaped so one entry stays one line;
/// unescape is its inverse.
fn escape(text: &str) -> String {
    let mut s = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\t' => s.push_str("\\t"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            other => s.push(other),
        }
    }
    s
}

fn unescape(text: &str) -> String {
    let mut s = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            s.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => s.push('\t'),
            Some('n') => s.push('\n'),
            Some('r') => s.push('\r'),
            Some('\\') => s.push('\\'),
            Some(other) => {
                s.push('\\');
                s.push(other);
            }
            None => s.push('\\'),
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "crates/ui-egui/src/demo.rs";

    fn found(source: &str) -> Vec<(usize, String)> {
        scan_source(FILE, source).into_iter().map(|h| (h.line, h.text)).collect()
    }

    fn keys(source: &str) -> Vec<(usize, String)> {
        scan_keys_source(FILE, source).into_iter().map(|k| (k.line, k.text)).collect()
    }

    /// The other half of the check: a label that goes through `tr` still reads English when the
    /// catalogue has no entry for it, which is how three waves shipped English by accident.
    #[test]
    fn finds_the_key_of_every_tr_call_and_only_a_lone_literal() {
        let source = concat!(
            "fn f(ui: &Ui, language: Language, name: &str) {\n",
            "    ui.label(tr(ui, \"Paste\"));\n",
            "    ui.label(tr(\"Bold\"));\n",
            "    ui.label(language.tr(\"Italic\"));\n",
            "    ui.label(widgets::tr(ui, \"Height: {:.2} ({} pixels)\"));\n",
            "    ui.label(tr(ui, name));\n",
            "    ui.label(tr(ui, format!(\"Total: {n}\")));\n",
            "    let s = \"tr(\\\"Not a call\\\")\";\n",
            "    // tr(\"A comment is not a call\")\n",
            "}\n",
        );
        assert_eq!(
            keys(source),
            vec![(2, "Paste".to_string()), (3, "Bold".to_string()), (4, "Italic".to_string()), (5, "Height: {:.2} ({} pixels)".to_string()),]
        );
    }

    #[test]
    fn a_key_without_an_entry_fails_and_a_context_key_may_use_its_label() {
        let known: std::collections::BTreeSet<String> = zh_keys_of(concat!(
            "    match text {\n",
            "        \"Paste\" => \"粘贴\",\n",
            "        \"General\" => \"常规\",\n",
            "        _ => return None,\n",
            "    }\n"
        ));
        assert!(known.contains("Paste") && known.contains("General"), "{known:?}");
        let ok = vec![
            Key { file: "a.rs".into(), line: 1, text: "Paste".into() },
            // tr() strips a context prefix when the full key is missing.
            Key { file: "a.rs".into(), line: 2, text: "Options|General".into() },
        ];
        assert!(verify_keys(&ok, &known).is_ok());
        let missing = vec![Key { file: "grid.rs".into(), line: 42, text: "Minor gridlines".into() }];
        let err = verify_keys(&missing, &known).unwrap_err();
        assert!(err.contains("grid.rs:42") && err.contains("Minor gridlines"), "{err}");
    }

    #[test]
    fn reports_the_unwrapped_literals_of_text_arguments_only() {
        let src = "\
fn demo(ui: &mut Ui) {
    ui.label(lang.tr(\"Paste\"));
    ui.label(\"Paste Special\");
    ui.button(format!(\"Insert {} row\", n));
    ui.label(RichText::new(\"Bold\").strong());
    ui.add(Button::new(\"OK\"));
    ui.selectable_label(sel, \"Home\").on_hover_text(\"+\");
    ui.label(\"A1\");
    ui.checkbox(&mut self.on, \"Gridlines\");
    ui.label(RichText::new(\"Cell\").id_salt(\"notes_pane\"));
}
";
        assert_eq!(
            found(src),
            vec![
                (3, "Paste Special".to_string()),
                (4, "Insert {} row".to_string()),
                (5, "Bold".to_string()),
                (6, "OK".to_string()),
                (7, "Home".to_string()),
                (9, "Gridlines".to_string()),
                (10, "Cell".to_string()),
            ],
            "the tr-wrapped literal, the symbol, the cell address and the id are all skipped"
        );
    }

    #[test]
    fn skips_non_text_arguments_and_the_data_inside_a_label() {
        let src = "\
fn demo(ui: &mut Ui) {
    ui.selectable_value(&mut self.by, \"values\".into(), tr(ui, \"Cell Values\"));
    ui.label(field(row, \"name\"));
    ui.label(w[\"value\"].as_str().unwrap_or(\"\"));
    ui.label(format!(\"{n}:\"));
    ui.label(format!(\"Total: {total}\"));
    ui.button(RichText::new(format!(\"ƒ  {n}\")).font(f));
    ui.label(lang.tr(\"Version {version}\").replace(\"{version}\", env!(\"CARGO_PKG_VERSION\")));
}
";
        assert_eq!(
            found(src),
            vec![(6, "Total: {total}".to_string())],
            "an id, a field lookup, an index key, a placeholder-only template, a lone glyph and an
             already-translated label are not untranslated text"
        );
    }

    #[test]
    fn reports_the_tooltip_link_hint_and_accessibility_surfaces() {
        let src = "\
fn demo(ui: &mut Ui, resp: Response) {
    resp.on_hover_text_at_pointer(\"Drag to fill cells\");
    resp.clone().on_hover_text_at_pointer(tr(ui, \"Fill handle\"));
    resp.on_disabled_hover_text(\"Nothing to undo\");
    ui.link(\"More workbooks\");
    ui.link(lang.tr(\"Page Setup\"));
    ui.small_button(\"Keep\");
    ui.small_button(crate::widgets::tr(ui, \"Delete\"));
    ui.toggle_value(&mut self.bold, \"Bold\");
    ui.radio(mode == Mode::Fit, \"Fit selection\");
    ui.add(TextEdit::singleline(&mut self.name).hint_text(\"Type to filter\"));
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, \"Paste\"));
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, on, \"Format Painter\"));
}
";
        assert_eq!(
            found(src),
            vec![
                (2, "Drag to fill cells".to_string()),
                (4, "Nothing to undo".to_string()),
                (5, "More workbooks".to_string()),
                (7, "Keep".to_string()),
                (9, "Bold".to_string()),
                (10, "Fit selection".to_string()),
                (11, "Type to filter".to_string()),
                (12, "Paste".to_string()),
                (13, "Format Painter".to_string()),
            ],
            "the wrapped twin on the line under each one is skipped, the unwrapped label is not"
        );
    }

    #[test]
    fn ignores_calls_whose_argument_is_a_closure_an_id_or_a_buffer() {
        // A wrong index here would report the argument beside the label -- a layout enum, an id salt
        // or a &mut String -- so these calls carry no surface at all. The closure bodies are still
        // walked on their own, which is where their labels are found.
        let src = "\
fn demo(ui: &mut Ui, resp: Response, buf: &mut String) {
    resp.on_hover_ui(|ui| {
        ui.label(\"Inside the hover ui\");
    });
    resp.on_hover_ui_at_pointer(|ui| ui.label(\"At the pointer\"));
    let by_id = ComboBox::from_id_salt(\"sort_menu\");
    let menu = Popup::menu(&resp);
    ui.add(TextEdit::singleline(buf));
    ui.text_edit_singleline(buf);
    let labelled = ComboBox::from_label(\"Alignment\");
}
";
        assert_eq!(
            found(src),
            vec![(3, "Inside the hover ui".to_string()), (5, "At the pointer".to_string()), (10, "Alignment".to_string()),],
            "only the labels are text: the closure, the id salt and the buffer are not, and
             from_label is the one ComboBox constructor that takes a label"
        );
    }

    #[test]
    fn skips_test_modules_and_test_only_items() {
        let src = "\
fn demo(ui: &mut Ui) {
    ui.label(\"Visible\");
    ui.heading(\"Also visible\");
}

#[cfg(test)]
mod tests {
    fn t(ui: &mut Ui) {
        ui.label(\"Test only\");
    }
}

mod tests {
    fn u(ui: &mut Ui) {
        ui.button(\"Test only too\");
    }
}

#[test]
fn one_test(ui: &mut Ui) {
    ui.button(\"Test still\");
}

#[cfg(test)]
use harness::Harness;

fn after(ui: &mut Ui) {
    ui.label(\"After the tests\");
}
";
        assert_eq!(found(src), vec![(2, "Visible".to_string()), (3, "Also visible".to_string()), (28, "After the tests".to_string())]);
    }

    #[test]
    fn reports_literals_across_a_multiline_call_without_double_counting() {
        let src = "\
fn demo(ui: &mut Ui) {
    ui.menu_button(
        \"File\",
        |ui| {
            ui.label(\"Save\");
        },
    );
    let sql = r#\"select \"quoted\"\"#;
    ui.label(r\"Raw label\");
    ui.label(\"日本語のラベル\");
}
";
        assert_eq!(
            found(src),
            vec![(3, "File".to_string()), (5, "Save".to_string()), (9, "Raw label".to_string()), (10, "日本語のラベル".to_string()),],
            "the closure body is reported once, by its own call, not by the menu_button arguments"
        );
    }

    #[test]
    fn survives_odd_quoting_and_unterminated_input() {
        // None of these may panic: the scanner is a byte walk with no slicing and no unwrap.
        for src in [
            "",
            "ui.label(\"unterminated",
            "ui.label(r#\"unterminated",
            "/* unterminated",
            "ui.label('",
            "ui.label(\"abc\\\"def\")",
            "ui.label(\"with a real\ttab\")",
            "ui.label(",
            "r",
            "br##\"x\"##",
            "ui.label(\"\u{7f}\")",
            "ui.label(\"\\u{1F600} smile\")",
        ] {
            let _ = scan_source(FILE, src);
        }
        assert_eq!(found("ui.label(\"abc\\\"def\")"), vec![(1, "abc\\\"def".to_string())]);
        assert_eq!(found("ui.label(\"\\u{1F600} smile\")"), vec![(1, "\\u{1F600} smile".to_string())]);
    }

    #[test]
    fn baseline_round_trips_through_the_tab_format() {
        let hits = scan_source(FILE, "fn f(ui: &mut Ui) {\n    ui.label(\"a real\ttab\");\n}\n");
        let text = render(&hits);
        assert!(text.contains(&format!("{FILE}:2\ta real\\ttab")), "{text}");
        assert_eq!(
            parse_baseline(&text).expect("the rendered baseline parses"),
            vec![Row { file: FILE.to_string(), line: 2, text: "a real\ttab".to_string() }]
        );
        assert!(parse_baseline("not a row").is_err(), "a malformed row is an error, not a silent skip");
        assert!(parse_baseline("a.rs:x\ttext").is_err());
    }

    #[test]
    fn check_fails_on_a_new_literal_and_passes_once_it_is_in_the_baseline() {
        let before = scan_source(FILE, "fn f(ui: &mut Ui) {\n    ui.label(\"Paste\");\n}\n");
        let baseline = parse_baseline(&render(&before)).expect("the baseline parses");
        verify(&baseline, &before).expect("a literal already in the baseline passes --check");

        let after = scan_source(FILE, "fn f(ui: &mut Ui) {\n    ui.label(\"Paste\");\n    ui.button(\"Unlisted\");\n}\n");
        let err = verify(&baseline, &after).expect_err("a literal outside the baseline fails --check");
        assert!(err.contains("1 user-visible literal"), "{err}");
        assert!(err.contains(&format!("{FILE}:3")), "the failure names the file and line: {err}");
        assert!(err.contains("Unlisted"), "the failure quotes the literal: {err}");

        let listed = parse_baseline(&render(&after)).expect("the baseline parses");
        verify(&listed, &after).expect("--write accepts a deliberate literal");
        verify(&listed, &before).expect("an entry that disappeared is fine: the ratchet only goes down");
    }
}
