//! Hyperlinks: `HLink` ([MS-XLS] 2.4.140), `HLinkTooltip` (2.4.141) and the Hyperlink Object they
//! carry ([MS-OSHARED] 2.3.7.1, which [MS-XLS] names as the structure's definition), mapped to the
//! model's [`Hyperlink`]: the target a URL, file or in-workbook location, and the tooltip.
//!
//! The display text has no model field: in both formats it is the cell's own text.

use gridcraft_core::RangeRef;
use gridcraft_model::Hyperlink;

use super::records::Cursor;
use super::{invalid, ref8};
use crate::IoError;

/// Characters read from one hyperlink string at most.
const MAX_TEXT: usize = 32_768;
/// A composite moniker's monikers followed at most.
const MAX_MONIKERS: usize = 8;

/// A hyperlink and the cells it covers.
pub(super) struct Link {
    pub ranges: Vec<RangeRef>,
    pub link: Hyperlink,
}

/// `HLink` (2.4.140): `ref8`, the hyperlink's CLSID, then the Hyperlink Object.
pub(super) fn hlink(c: &mut Cursor<'_, '_>) -> Result<Option<Link>, IoError> {
    let Some(range) = ref8(c)? else { return Ok(None) };
    // `hlinkClsid` names the COM component that saved the object; the object itself follows.
    c.skip(16)?;
    let version = c.u32()?;
    if version != 2 {
        return Err(invalid("a hyperlink object has an unknown version"));
    }
    let flags = c.u32()?;
    let has = |bit: u32| flags & (1 << bit) != 0;
    // `displayName`: the text Excel shows in the cell; the cell's own value is what the model keeps.
    if has(4) {
        string(c)?;
    }
    if has(7) {
        string(c)?;
    }
    let target = if has(0) {
        if has(8) {
            // `hlstmfMonikerSavedAsStr`: the moniker is a plain string.
            Some(string(c)?)
        } else {
            moniker(c, 0)?
        }
    } else {
        None
    };
    let location = if has(3) { Some(string(c)?) } else { None };
    // A relationship target is outside the workbook; a bare location is a place inside it.
    let external = target.is_some();
    let mut target = target.unwrap_or_default();
    if let Some(loc) = location.filter(|l| !l.is_empty()) {
        if target.is_empty() {
            target = loc.trim_start_matches('#').to_string();
        } else {
            target = format!("{target}#{loc}");
        }
    }
    if target.is_empty() {
        return Ok(None);
    }
    Ok(Some(Link { ranges: vec![range], link: Hyperlink { target, tooltip: None, external } }))
}

/// `HLinkTooltip` (2.4.141): the tooltip of the hyperlink with the same range.
pub(super) fn tooltip(c: &mut Cursor<'_, '_>) -> Result<Option<(RangeRef, String)>, IoError> {
    c.skip(2)?;
    let Some(range) = ref8(c)? else { return Ok(None) };
    let mut units: Vec<u16> = Vec::new();
    while c.remaining() >= 2 && units.len() < MAX_TEXT {
        let u = c.u16()?;
        if u == 0 {
            break;
        }
        units.push(u);
    }
    Ok(Some((range, String::from_utf16_lossy(&units))))
}

/// A `HyperlinkString` (2.3.7.9) without its length: `length` counts characters including the
/// null terminator, which the caller has already read.
fn string(c: &mut Cursor<'_, '_>) -> Result<String, IoError> {
    let len = c.u32()? as usize;
    if len > MAX_TEXT {
        return Err(invalid("a hyperlink string is too long"));
    }
    let mut units: Vec<u16> = Vec::with_capacity(len.min(1024));
    for _ in 0..len {
        units.push(c.u16()?);
    }
    while units.last() == Some(&0) {
        units.pop();
    }
    Ok(String::from_utf16_lossy(&units))
}

/// A `HyperlinkMoniker` (2.3.7.2): a CLSID naming the moniker type, then its data.
fn moniker(c: &mut Cursor<'_, '_>, depth: usize) -> Result<Option<String>, IoError> {
    if depth > 2 {
        return Ok(None);
    }
    let clsid = c.bytes(16)?;
    match clsid.get(..4) {
        // URLMoniker {79EAC9E0-BAF9-11CE-8C82-00AA004BA90B}
        Some([0xE0, 0xC9, 0xEA, 0x79]) => url(c),
        // FileMoniker {00000303-0000-0000-C000-000000000046}
        Some([0x03, 0x03, 0x00, 0x00]) => file(c),
        // CompositeMoniker {00000309-...}: a file or URL followed by item monikers.
        Some([0x09, 0x03, 0x00, 0x00]) => {
            let count = c.u32()? as usize;
            let mut found = None;
            for _ in 0..count.min(MAX_MONIKERS) {
                if found.is_none() {
                    found = moniker(c, depth + 1)?;
                } else {
                    skip_moniker(c)?;
                }
            }
            Ok(found)
        }
        _ => Ok(None),
    }
}

/// A moniker whose type the reader doesn't use (an item or anti moniker): skip what it holds.
fn skip_moniker(c: &mut Cursor<'_, '_>) -> Result<(), IoError> {
    let clsid = c.bytes(16)?;
    match clsid.get(..4) {
        Some([0xE0, 0xC9, 0xEA, 0x79]) => {
            let len = c.u32()? as usize;
            c.skip(len.min(c.remaining()))?;
        }
        Some([0x03, 0x03, 0x00, 0x00]) => {
            // A file moniker's length is only known once its fields are read.
            skip_file(c)?;
        }
        // ItemMoniker (2.3.7.5): a delimiter and an item, each length-prefixed in bytes.
        Some([0x04, 0x03, 0x00, 0x00]) => {
            for _ in 0..2 {
                let len = c.u32()? as usize;
                c.skip(len.min(c.remaining()))?;
            }
        }
        // AntiMoniker (2.3.7.4): one count.
        Some([0x05, 0x03, 0x00, 0x00]) => c.skip(4)?,
        // Anything else: no length to follow, so nothing more can be read.
        _ => return Err(invalid("a hyperlink moniker cannot be read")),
    }
    Ok(())
}

/// `URLMoniker` (2.3.7.6): a byte count, a null-terminated UTF-16 URL, and an optional
/// serialization GUID and flags that the count covers.
fn url(c: &mut Cursor<'_, '_>) -> Result<Option<String>, IoError> {
    let len = c.u32()? as usize;
    if len < 2 || !len.is_multiple_of(2) {
        return Err(invalid("a hyperlink URL has a length that is not a whole string"));
    }
    let units_max = (len / 2).min(MAX_TEXT);
    let mut units: Vec<u16> = Vec::with_capacity(units_max.min(1024));
    let mut terminated = false;
    for _ in 0..units_max {
        let u = c.u16()?;
        if u == 0 {
            terminated = true;
            break;
        }
        units.push(u);
    }
    // Whatever follows the terminator inside the declared length (a serialization GUID, version
    // and flags) is skipped with it.
    let used = (units.len() + usize::from(terminated)) * 2;
    c.skip(len.saturating_sub(used).min(c.remaining()))?;
    Ok(Some(String::from_utf16_lossy(&units)))
}

/// `FileMoniker` (2.3.7.8): parent-directory count, an ANSI path, then an optional Unicode path.
fn file(c: &mut Cursor<'_, '_>) -> Result<Option<String>, IoError> {
    let (ansi, unicode) = read_file(c)?;
    Ok(unicode.or(ansi))
}

fn skip_file(c: &mut Cursor<'_, '_>) -> Result<(), IoError> {
    read_file(c).map(|_| ())
}

fn read_file(c: &mut Cursor<'_, '_>) -> Result<(Option<String>, Option<String>), IoError> {
    let _parents = c.u16()?;
    let len = c.u32()? as usize;
    if len > MAX_TEXT {
        return Err(invalid("a hyperlink file path is too long"));
    }
    let mut ansi: Vec<u8> = Vec::with_capacity(len.min(1024));
    for _ in 0..len {
        ansi.push(c.u8()?);
    }
    while ansi.last() == Some(&0) {
        ansi.pop();
    }
    let _server = c.u16()?;
    let _version = c.u16()?;
    c.skip(16)?;
    c.skip(4)?;
    let unicode_size = c.u32()? as usize;
    let mut unicode = None;
    if unicode_size > 0 {
        let bytes = c.u32()? as usize;
        let _key = c.u16()?;
        if bytes > MAX_TEXT * 2 {
            return Err(invalid("a hyperlink file path is too long"));
        }
        let units: Vec<u16> = (0..bytes / 2).map(|_| c.u16()).collect::<Result<_, _>>()?;
        unicode = Some(String::from_utf16_lossy(&units));
    }
    let ansi = (!ansi.is_empty()).then(|| ansi.iter().map(|b| char::from(*b)).collect::<String>());
    Ok((ansi, unicode))
}
