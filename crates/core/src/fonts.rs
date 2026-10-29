//! Font *file* discovery for scripts the standard PDF fonts cannot set, and a Unicode coverage
//! probe that reads a font's `cmap`.
//!
//! This module is deliberately plain: no GUI toolkit, no fontconfig, no global cache. A caller
//! that needs a face for `字` asks [`resolve_for_char`]; a caller that has a string of text asks
//! [`resolve_for_text`] and gets a file whose glyphs cover the script the text is written in.
//! Every lookup reads the directories again, so a font installed while the app runs is found on
//! the next export (and nothing can go stale).
//!
//! Search order:
//! 1. `CRAFT_FONTS_DIR` — the repository's optional font input (AGENTS.md: fonts are never
//!    committed here). Matched by family/file stem, then by coverage.
//! 2. The platform's font directories: `%WINDIR%\Fonts` and the per-user Fonts folder on Windows;
//!    `/System/Library/Fonts`, `/System/Library/Fonts/Supplemental`, `/Library/Fonts` and
//!    `~/Library/Fonts` on macOS; `/usr/share/fonts`, `/usr/local/share/fonts`, `~/.fonts`,
//!    `~/.local/share/fonts` and Flatpak's read-only `/run/host/fonts*` on everything else (a
//!    plain recursive `read_dir` scan — installing fontconfig is not required).
//! 3. As a last resort any font in those directories that covers the character.
//!
//! Fonts are hostile input. Every table offset and length is range-checked against the file, and
//! a file that does not parse is simply not a candidate (the caller gets `Err`, never a panic).
//! Only TrueType outlines (`glyf`) are accepted: the PDF writer embeds them as `CIDFontType2`,
//! which cannot carry CFF outlines (`CIDFontType0` would need `FontFile3`).
//!
//! wasm has no filesystem: discovery returns [`FontError::Unsupported`] there, and the PDF writer
//! falls back to its standard-font path.

use std::path::{Path, PathBuf};

use thiserror::Error;

/// A font file read into memory, with the path it came from.
#[derive(Clone, Debug)]
pub struct FontFile {
    pub path: PathBuf,
    pub data: Vec<u8>,
}

/// Why no font could be resolved.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FontError {
    /// No installed face covers the character.
    #[error("no font with a glyph for U+{:04X} was found", u32::from(*_0))]
    NoCoverage(char),
    /// No face for the script could be found (or the named file is not a TrueType font).
    #[error("no TrueType font for {0} was found")]
    NoFont(String),
    /// Font discovery needs a filesystem.
    #[error("font discovery is not available on this platform")]
    Unsupported,
    /// A file could not be read.
    #[error("cannot read font {path}: {reason}")]
    Read { path: String, reason: String },
}

/// The script a character belongs to, used to pick a face without probing the whole Unicode
/// range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Script {
    Latin,
    Greek,
    Cyrillic,
    Hebrew,
    Arabic,
    Devanagari,
    Thai,
    Cjk,
    Hangul,
    /// A symbol, emoji or a script this list does not name: try the general-purpose faces.
    Other,
}

impl Script {
    /// The script of `c`, or `None` for ASCII (which the standard PDF fonts can set).
    pub fn of(c: char) -> Option<Script> {
        let u = c as u32;
        Some(match u {
            0x00..=0x7F => return None,
            0x80..=0x024F | 0x1E00..=0x1EFF | 0x2C60..=0x2C7F | 0xA720..=0xA7FF => Script::Latin,
            0x0370..=0x03FF | 0x1F00..=0x1FFF => Script::Greek,
            0x0400..=0x052F | 0x2DE0..=0x2DFF | 0xA640..=0xA69F => Script::Cyrillic,
            0x0590..=0x05FF | 0xFB1D..=0xFB4F => Script::Hebrew,
            0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF => Script::Arabic,
            0x0900..=0x097F | 0x1CD0..=0x1CFF | 0xA8E0..=0xA8FF => Script::Devanagari,
            0x0E00..=0x0E7F => Script::Thai,
            0x1100..=0x11FF | 0x3130..=0x318F | 0xA960..=0xA97F | 0xAC00..=0xD7AF => Script::Hangul,
            // Han, kana, compatibility ideographs and the half-width forms.
            0x2E80..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x3FFFF | 0xFF66..=0xFF9F => Script::Cjk,
            _ => Script::Other,
        })
    }

    /// The name used in error messages and documentation.
    pub fn name(self) -> &'static str {
        match self {
            Script::Latin => "Latin",
            Script::Greek => "Greek",
            Script::Cyrillic => "Cyrillic",
            Script::Hebrew => "Hebrew",
            Script::Arabic => "Arabic",
            Script::Devanagari => "Devanagari",
            Script::Thai => "Thai",
            Script::Cjk => "CJK",
            Script::Hangul => "Hangul",
            Script::Other => "this script",
        }
    }
}

/// The file-name stems of faces that usually cover `script`, in preference order: faces shipped
/// with the three desktop platforms first, open-licence fallbacks last. Matching is
/// case-insensitive and ignores weight/slant suffixes (see [`regular_first`]), so `segoeui`
/// finds `segoeui.ttf` and not `segoeuib.ttf`.
#[cfg(not(target_arch = "wasm32"))]
fn stems(script: Script) -> &'static [&'static str] {
    const LATIN: &[&str] = &["segoeui", "arial", "calibri", "helvetica", "tahoma", "verdana", "dejavusans", "liberationsans", "notosans", "freesans"];
    const HEBREW: &[&str] = &["segoeui", "arial", "tahoma", "david", "notosanshebrew", "dejavusans", "freesans"];
    const ARABIC: &[&str] = &["segoeui", "arial", "tahoma", "geeza", "notosansarabic", "dejavusans", "freesans"];
    const DEVANAGARI: &[&str] = &["nirmala", "mangal", "kohinoor", "devanagari", "notosansdevanagari", "lohitdevanagari", "freeserif"];
    const THAI: &[&str] = &["leelaw", "leelawadee", "tahoma", "thonburi", "ayuthaya", "notosansthai", "garuda", "freeserif"];
    const CJK: &[&str] = &[
        "msyh",
        "simhei",
        "simsun",
        "dengxian",
        "malgun",
        "yugoth",
        "meiryo",
        "msgothic",
        "pingfang",
        "hiragino",
        "stheiti",
        "songti",
        "arial unicode",
        "notosanscjk",
        "notosanssc",
        "notosansjp",
        "notosanstc",
        "sourcehansans",
        "wqy",
        "wenquanyi",
        "droidsansfallback",
    ];
    const HANGUL: &[&str] = &["malgun", "applesdgothicneo", "batang", "gulim", "nanumgothic", "notosanskr", "notosanscjk", "unbatang"];
    const OTHER: &[&str] = &["segoeui", "arial", "dejavusans", "notosans", "freeserif", "arial unicode", "seguisym"];
    match script {
        Script::Latin | Script::Greek | Script::Cyrillic => LATIN,
        Script::Hebrew => HEBREW,
        Script::Arabic => ARABIC,
        Script::Devanagari => DEVANAGARI,
        Script::Thai => THAI,
        Script::Cjk => CJK,
        Script::Hangul => HANGUL,
        Script::Other => OTHER,
    }
}

/// Most font files one scan collects (a directory with more is truncated; the named candidates
/// are matched first, so the cap only bites the last-resort coverage sweep).
#[cfg(not(target_arch = "wasm32"))]
const MAX_FONT_FILES: usize = 4000;
/// Largest font file read, in bytes.
#[cfg(not(target_arch = "wasm32"))]
const MAX_FONT_BYTES: u64 = 64 * 1024 * 1024;
/// Directory levels walked below each root.
#[cfg(not(target_arch = "wasm32"))]
const MAX_DEPTH: u32 = 3;
/// Most faces [`resolve_for_text`] reads and scores before settling for the best it has seen.
#[cfg(not(target_arch = "wasm32"))]
const MAX_SCORED_FACES: usize = 6;
/// Most faces per script [`resolve_for_text`] scores.
#[cfg(not(target_arch = "wasm32"))]
const FACES_PER_SCRIPT: usize = 2;
/// Most characters [`resolve_for_text`] scores a face against.
#[cfg(not(target_arch = "wasm32"))]
const MAX_SCORED_CHARS: usize = 2048;

/// Reads `path` and returns it if it is a TrueType font with `glyf` outlines.
pub fn load(path: &Path) -> Result<FontFile, FontError> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = path;
        Err(FontError::Unsupported)
    }
    #[cfg(not(target_arch = "wasm32"))]
    read_checked(path).ok_or_else(|| FontError::NoFont(path.display().to_string()))
}

/// Reads a `fontPath` parameter: a file path, or a family name (`Noto Sans`) matched against the
/// font directories.
pub fn load_named(name: &str) -> Result<FontFile, FontError> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = name;
        Err(FontError::Unsupported)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let p = Path::new(name);
        if p.is_file() {
            return load(p);
        }
        let stem: String = name.chars().filter(|c| !c.is_whitespace() && *c != '-').flat_map(char::to_lowercase).collect();
        if stem.is_empty() {
            return Err(FontError::NoFont(name.to_string()));
        }
        resolve_candidates(&[stem.as_str()], None, name)
    }
}

/// True when `data` is an SFNT font (single face or collection) with TrueType outlines.
pub fn is_embeddable(data: &[u8]) -> bool {
    let Some(tables) = sfnt::tables(data) else { return false };
    ["glyf", "loca", "head", "cmap", "hhea", "maxp", "hmtx"].iter().all(|t| sfnt::find(&tables, t).is_some())
}

/// A font face with a glyph for `c`.
pub fn resolve_for_char(c: char) -> Result<FontFile, FontError> {
    let script = Script::of(c).unwrap_or(Script::Latin);
    resolve(script, Some(c))
}

/// A font face for `script` (a face that covers nothing in particular is still accepted when no
/// better one exists).
pub fn resolve_for_script(script: Script) -> Result<FontFile, FontError> {
    resolve(script, None)
}

/// A font face that covers `text` best: among the candidate faces for the scripts it uses, the one
/// with the most of its characters, so a single face can serve as much of a mixed-script document
/// as possible. Characters the standard PDF fonts set (ASCII, WinAnsi) should be filtered out by
/// the caller: they do not influence the choice.
pub fn resolve_for_text(text: &str) -> Result<FontFile, FontError> {
    // Count characters per script; remember one representative character of each.
    let mut counts: Vec<(Script, usize, char)> = Vec::new();
    for c in text.chars() {
        let Some(script) = Script::of(c) else { continue };
        match counts.iter_mut().find(|(s, _, _)| *s == script) {
            Some((_, n, _)) => *n += 1,
            None => counts.push((script, 1, c)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let Some(&(_, _, probe)) = counts.first() else {
        return Err(FontError::NoFont("a script the standard fonts cover".to_string()));
    };
    #[cfg(target_arch = "wasm32")]
    {
        let _ = probe;
        Err(FontError::Unsupported)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // One face per document has to serve every script in it, so the face that covers the
        // most distinct characters wins rather than the one for the most common script: a
        // Cyrillic + Han document takes a face with both when one is installed. Two faces per
        // script get read (a regular face and its first alternative); the total is capped, so a
        // resolution stays a handful of file reads.
        let files = scan(search_dirs());
        // Scoring reads a `cmap` once per character, so bound the sample.
        let sample: Vec<char> = text.chars().take(MAX_SCORED_CHARS).collect();
        let total = sample.len();
        let mut best: Option<(usize, FontFile)> = None;
        let mut tried: Vec<PathBuf> = Vec::new();
        'scripts: for (script, _, _) in &counts {
            for path in named_faces(&files, stems(*script)).into_iter().take(FACES_PER_SCRIPT) {
                if tried.len() >= MAX_SCORED_FACES {
                    break 'scripts;
                }
                if tried.contains(&path) {
                    continue;
                }
                tried.push(path.clone());
                let Some(font) = read_checked(&path) else { continue };
                let covered = sample.iter().filter(|c| covers(&font.data, **c)).count();
                if best.as_ref().is_none_or(|(b, _)| covered > *b) {
                    let full = covered == total;
                    best = Some((covered, font));
                    if full {
                        break 'scripts;
                    }
                }
            }
        }
        match best {
            Some((0, _)) | None => Err(FontError::NoCoverage(probe)),
            Some((_, font)) => Ok(font),
        }
    }
}

fn resolve(script: Script, probe: Option<char>) -> Result<FontFile, FontError> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (script, probe);
        Err(FontError::Unsupported)
    }
    #[cfg(not(target_arch = "wasm32"))]
    resolve_candidates(stems(script), probe, script.name())
}

/// One font file per candidate stem: the regular face of a stem is preferred over its bold,
/// italic and light siblings, and a stem that is not installed is skipped.
#[cfg(not(target_arch = "wasm32"))]
fn named_faces(files: &[(PathBuf, String)], candidates: &[&str]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for stem in candidates {
        let mut named: Vec<&Path> = files.iter().filter(|(_, name)| name_matches(name, stem)).map(|(p, _)| p.as_path()).collect();
        regular_first(&mut named, stem);
        if let Some(path) = named.first().map(|p| p.to_path_buf())
            && !out.contains(&path)
        {
            out.push(path);
        }
    }
    out
}

/// Tries `candidates` (file-name stems) in order against the font directories, then every other
/// font when `probe` is set.
#[cfg(not(target_arch = "wasm32"))]
fn resolve_candidates(candidates: &[&str], probe: Option<char>, what: &str) -> Result<FontFile, FontError> {
    let files = scan(search_dirs());
    for path in named_faces(&files, candidates) {
        let Some(font) = read_checked(&path) else { continue };
        if probe.is_none_or(|c| covers(&font.data, c)) {
            return Ok(font);
        }
    }
    if let Some(c) = probe {
        for (path, _) in &files {
            let Some(font) = read_checked(path) else { continue };
            if covers(&font.data, c) {
                return Ok(font);
            }
        }
        return Err(FontError::NoCoverage(c));
    }
    Err(FontError::NoFont(what.to_string()))
}

/// Reads a font file if it is small enough and has embeddable TrueType outlines.
#[cfg(not(target_arch = "wasm32"))]
fn read_checked(path: &Path) -> Option<FontFile> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FONT_BYTES {
        return None;
    }
    let data = std::fs::read(path).ok()?;
    is_embeddable(&data).then_some(FontFile { path: path.to_path_buf(), data })
}

/// Directory roots for this platform, `CRAFT_FONTS_DIR` first.
#[cfg(not(target_arch = "wasm32"))]
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = std::env::var_os("CRAFT_FONTS_DIR") {
        let p = PathBuf::from(dir);
        if p.is_dir() {
            dirs.push(p);
        }
    }
    #[cfg(target_os = "windows")]
    {
        dirs.push(PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into())).join("Fonts"));
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("Microsoft").join("Windows").join("Fonts"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        for p in ["/System/Library/Fonts", "/System/Library/Fonts/Supplemental", "/Library/Fonts"] {
            dirs.push(PathBuf::from(p));
        }
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join("Library").join("Fonts"));
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        for p in ["/usr/share/fonts", "/usr/local/share/fonts", "/run/host/fonts", "/run/host/local-fonts", "/run/host/user-fonts"] {
            dirs.push(PathBuf::from(p));
        }
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join(".fonts"));
            dirs.push(PathBuf::from(home).join(".local").join("share").join("fonts"));
        }
    }
    dirs
}

/// Every `.ttf`/`.ttc`/`.otf` file below `dirs` (depth-limited), with its lower-cased file name.
#[cfg(not(target_arch = "wasm32"))]
fn scan(dirs: Vec<PathBuf>) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for dir in dirs {
        walk(&dir, 0, &mut out);
        if out.len() >= MAX_FONT_FILES {
            break;
        }
    }
    out.truncate(MAX_FONT_FILES);
    out
}

#[cfg(not(target_arch = "wasm32"))]
fn walk(dir: &Path, depth: u32, out: &mut Vec<(PathBuf, String)>) {
    if out.len() >= MAX_FONT_FILES || depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            subdirs.push(path);
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".ttf") || lower.ends_with(".ttc") || lower.ends_with(".otf") {
            out.push((path, lower));
            if out.len() >= MAX_FONT_FILES {
                return;
            }
        }
    }
    subdirs.sort();
    for sub in subdirs {
        walk(&sub, depth + 1, out);
    }
}

/// Weight and width words that make a face a poor default: a regular upright face is preferred.
#[cfg(not(target_arch = "wasm32"))]
const IRREGULAR: &[&str] =
    &["bold", "bd", "italic", "oblique", "light", "semibold", "black", "thin", "cond", "narrow", "extrabold", "heavy", "medium", "it"];

/// True when the file name `name` is the stem `stem`, or starts with it. `dejavusans` matches
/// `dejavusans.ttf` and `dejavusansmono-oblique.ttf`; `arial unicode` matches
/// `arialunicode.ttf`.
#[cfg(not(target_arch = "wasm32"))]
fn name_matches(name: &str, stem: &str) -> bool {
    let squeeze = |s: &str| -> String { s.chars().filter(|c| !c.is_whitespace() && *c != '-' && *c != '_').flat_map(char::to_lowercase).collect() };
    squeeze(name).starts_with(&squeeze(stem))
}

/// Orders candidate faces: the file named exactly after `stem` first, then names that carry no
/// weight/slant word, then weighted faces, then anything else (`segoeui.ttf`, `segoeuisb.ttf`,
/// `segoeuib.ttf`, `arial.ttf`).
#[cfg(not(target_arch = "wasm32"))]
fn regular_first(paths: &mut Vec<&Path>, stem: &str) {
    let squeeze = |s: &str| -> String { s.chars().filter(|c| !c.is_whitespace() && *c != '-' && *c != '_').flat_map(char::to_lowercase).collect() };
    let want = squeeze(stem);
    let rank = |p: &Path| -> u8 {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_ascii_lowercase();
        let base = squeeze(name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s));
        if base == want {
            return 0;
        }
        let Some(suffix) = base.strip_prefix(&want) else { return 3 };
        // `segoeuib` (bold) and `segoeuiz` (bold italic) carry a one-letter weight suffix.
        let irregular = IRREGULAR.iter().any(|w| suffix.contains(w)) || matches!(suffix, "b" | "i" | "z" | "l");
        if irregular { 2 } else { 1 }
    };
    paths.sort_by_key(|p| rank(p));
}

/// Does the font in `data` have a glyph for `c`? A malformed font covers nothing.
pub fn covers(data: &[u8], c: char) -> bool {
    sfnt::glyph_for(data, c).is_some_and(|g| g != 0)
}

/// The throw-away SFNT/cmap reader used for discovery and coverage. The PDF writer has its own,
/// fuller reader; this one only needs the table directory and `cmap`, so it never parses
/// outlines and reads a whole Windows `Fonts` directory in milliseconds.
mod sfnt {
    /// Bounds-checked big-endian reads: every offset comes from a hostile file.
    fn u16_at(d: &[u8], at: usize) -> Option<u16> {
        let b = d.get(at..at.checked_add(2)?)?;
        Some(u16::from_be_bytes([b[0], b[1]]))
    }
    fn u32_at(d: &[u8], at: usize) -> Option<u32> {
        let b = d.get(at..at.checked_add(4)?)?;
        Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Face 0's table directory: `(tag, offset, length)` for each table.
    pub(super) fn tables(data: &[u8]) -> Option<Vec<([u8; 4], usize, usize)>> {
        let base = if u32_at(data, 0)? == u32::from_be_bytes(*b"ttcf") {
            // A TrueType collection: face 0's directory (the outlines are shared).
            u32_at(data, 12)? as usize
        } else {
            0
        };
        if !matches!(u32_at(data, base)?, 0x0001_0000 | 0x7472_7565) {
            return None;
        }
        let n = u16_at(data, base + 4)? as usize;
        if n == 0 || n > 512 {
            return None;
        }
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let rec = base + 12 + i * 16;
            let tag = data.get(rec..rec + 4)?;
            let offset = u32_at(data, rec + 8)? as usize;
            let length = u32_at(data, rec + 12)? as usize;
            if offset.checked_add(length)? > data.len() {
                return None;
            }
            out.push(([tag[0], tag[1], tag[2], tag[3]], offset, length));
        }
        Some(out)
    }

    pub(super) fn find<'a>(tables: &'a [([u8; 4], usize, usize)], tag: &str) -> Option<&'a ([u8; 4], usize, usize)> {
        let want = tag.as_bytes();
        tables.iter().find(|(t, _, _)| t == want)
    }

    /// The glyph id `c` maps to, from the best `cmap` subtable.
    pub(super) fn glyph_for(data: &[u8], c: char) -> Option<u16> {
        let tables = tables(data)?;
        let (_, off, len) = *find(&tables, "cmap")?;
        let cmap = data.get(off..off.checked_add(len)?)?;
        let u = c as u32;
        let n = u16_at(cmap, 2)? as usize;
        let mut best: Option<(u8, u16)> = None;
        for i in 0..n.min(64) {
            // A `cmap` subtable record is 8 bytes: platform, encoding, offset.
            let rec = 4 + i * 8;
            let platform = u16_at(cmap, rec)?;
            let encoding = u16_at(cmap, rec + 2)?;
            let at = u32_at(cmap, rec + 4)? as usize;
            let Some(format) = u16_at(cmap, at) else { continue };
            // Formats 0, 4, 6 and 12 (an early TrueType cmap uses 0, 4 is BMP, 12 covers all of
            // Unicode). Symbol fonts (3,0) claim the private-use range and are a poor fallback.
            if !matches!(format, 0 | 4 | 6 | 12) || (platform == 3 && encoding == 0) {
                continue;
            }
            let rank = match (platform, encoding, format) {
                (3, 10, 12) | (0, _, 12) => 0u8,
                (3, 1, 4) | (0, _, 4) => 1,
                (3, _, 6) | (0, _, 6) => 2,
                _ => 3,
            };
            if best.is_some_and(|(r, _)| r <= rank) {
                continue;
            }
            if let Some(g) = lookup(cmap, at, format, u) {
                best = Some((rank, g));
            }
        }
        best.map(|(_, g)| g)
    }

    fn lookup(cmap: &[u8], at: usize, format: u16, u: u32) -> Option<u16> {
        match format {
            0 => {
                if u > 0xFF {
                    return None;
                }
                u16_at(cmap, at + 6 + u as usize)
            }
            4 => {
                if u > 0xFFFF {
                    return None;
                }
                let u = u as u16;
                let seg_x2 = u16_at(cmap, at + 6)? as usize;
                let segs = seg_x2 / 2;
                if segs == 0 {
                    return None;
                }
                let end = at + 14;
                let start = end + seg_x2 + 2;
                let delta = start + seg_x2;
                let range = delta + seg_x2;
                for i in 0..segs {
                    let e = u16_at(cmap, end + i * 2)?;
                    let s = u16_at(cmap, start + i * 2)?;
                    if u < s || u > e {
                        continue;
                    }
                    let d = u16_at(cmap, delta + i * 2)?;
                    let ro = u16_at(cmap, range + i * 2)? as usize;
                    if ro == 0 {
                        return Some(u.wrapping_add(d));
                    }
                    let addr = range + i * 2 + ro + (u - s) as usize * 2;
                    let g = u16_at(cmap, addr)?;
                    return Some(if g == 0 { 0 } else { g.wrapping_add(d) });
                }
                None
            }
            6 => {
                let first = u16_at(cmap, at + 6)? as u32;
                let count = u16_at(cmap, at + 8)? as usize;
                let i = u.checked_sub(first)? as usize;
                if i >= count {
                    return None;
                }
                u16_at(cmap, at + 10 + i * 2)
            }
            12 => {
                let groups = u32_at(cmap, at + 12)? as usize;
                for i in 0..groups.min(4_000_000) {
                    let g = at + 16 + i * 12;
                    let start = u32_at(cmap, g)?;
                    let end = u32_at(cmap, g + 4)?;
                    if u < start || u > end {
                        continue;
                    }
                    let base = u32_at(cmap, g + 8)?;
                    return Some((base.checked_add(u - start)? & 0xFFFF) as u16);
                }
                None
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
