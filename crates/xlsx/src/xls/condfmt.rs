//! Conditional formats: `CondFmt` ([MS-XLS] 2.4.56), `CF` (2.4.42) and `CFEx` (2.4.44).
//!
//! A `CondFmt` record names the ranges and the id; the `CF` records that follow it carry one rule
//! each (a cell-value comparison or a formula), and a `CFEx` record adds the priority, the
//! "stop if true" flag and — when it has one — a differential format. The rules of a `CF12` /
//! `CondFmt12` pair (colour scales, data bars, icon sets) are refused with a warning: their
//! record's tail packs `rgbTemplateParms` and `rgbCT` behind a byte count the specification's
//! diagram and its field description disagree about, so their offsets are not determined.

use std::collections::HashMap;

use gridcraft_core::{CellRef, RangeRef};
use gridcraft_model::{CfOperator, CfRule, Style};

use super::dxf::read_dxfn;
use super::records::Cursor;
use super::styles::Palette;
use super::{Links, decode, invalid, ref8};
use crate::IoError;

/// Ranges one `SqRefU` may hold ([MS-XLS] 2.5.247: `cref` ≤ 0x2000).
const MAX_SQREF: usize = 0x2000;

/// A `CondFmt` record: the cells its `CF` records apply to, the id a `CFEx` names it by, and how
/// many `CF` records follow.
pub(super) struct Group {
    pub ranges: Vec<RangeRef>,
    pub id: u16,
    pub count: usize,
}

/// `CondFmt` (2.4.56): `ccf`, `fToughRecalc` + `nID`, the bounding `Ref8U`, then the `SqRefU`.
pub(super) fn condfmt(c: &mut Cursor<'_, '_>) -> Result<Group, IoError> {
    let count = usize::from(c.u16()?);
    let flags = c.u16()?;
    let bound = ref8(c)?;
    let mut ranges: Vec<RangeRef> = vec![];
    let refs = usize::from(c.u16()?);
    for _ in 0..refs.min(MAX_SQREF).min(c.remaining() / 8) {
        if let Some(range) = ref8(c)? {
            ranges.push(range);
        }
    }
    if ranges.is_empty()
        && let Some(bound) = bound
    {
        ranges.push(bound);
    }
    Ok(Group { ranges, id: (flags >> 1) & 0x7FFF, count })
}

/// `CF` (2.4.42): the condition, its two formulas and a `DXFN`. The formulas sit at the end of the
/// record, so their offsets follow from `cce1` and `cce2` rather than from the format's length.
pub(super) fn cf(
    c: &mut Cursor<'_, '_>,
    base: CellRef,
    links: &Links,
    palette: &Palette,
    formats: &HashMap<u16, String>,
) -> Result<Option<CfRule>, IoError> {
    let ct = c.u8()?;
    let cp = c.u8()?;
    let cce1 = usize::from(c.u16()?);
    let cce2 = usize::from(c.u16()?);
    let rest = c.rest();
    let used = cce1.saturating_add(cce2);
    if used > rest.len() {
        return Err(invalid("a conditional format's formulas run past the end of their record"));
    }
    let split = rest.len() - used;
    let dxf = rest.get(..split).unwrap_or(&[]);
    let f1 = rest.get(split..split + cce1).unwrap_or(&[]);
    let f2 = rest.get(split + cce1..).unwrap_or(&[]);
    let style = Box::new(read_dxfn(dxf, palette, formats).unwrap_or_default());
    let Some(a) = text(f1, base, links) else { return Ok(None) };
    match ct {
        0x01 => {
            // `cp` 0 is "no comparison" in a `CFEx` extension; the CF record's own values are 1-8.
            let op = match cp {
                0x02 => CfOperator::NotBetween,
                0x03 => CfOperator::Equal,
                0x04 => CfOperator::NotEqual,
                0x05 => CfOperator::Greater,
                0x06 => CfOperator::Less,
                0x07 => CfOperator::GreaterOrEqual,
                0x08 => CfOperator::LessOrEqual,
                _ => CfOperator::Between,
            };
            let b = if cce2 == 0 { None } else { text(f2, base, links) };
            Ok(Some(CfRule::CellIs { op, a, b, style }))
        }
        0x02 => Ok(Some(CfRule::Expression { formula: a, style })),
        _ => Ok(None),
    }
}

/// What a `CFEx` (2.4.44) adds to the rule it extends.
pub(super) struct Ext {
    pub id: u16,
    /// Which `CF` record of that `CondFmt` collection this extends.
    pub icf: usize,
    pub priority: u32,
    pub stop_if_true: bool,
    pub style: Option<Style>,
    /// The extension introduces a `CF12` rule (a colour scale, data bar or icon set).
    pub cf12: bool,
}

/// `CFEx` (2.4.44): a future-record header, `fIsCF12`, `nID`, then a `CFExNonCF12` (2.5.27) when
/// the extension is not followed by a `CF12`.
pub(super) fn cfex(c: &mut Cursor<'_, '_>, palette: &Palette, formats: &HashMap<u16, String>) -> Result<Ext, IoError> {
    // FrtRefHeaderU (2.5.75): rt, grbitFrt, the range it repeats.
    c.skip(12)?;
    let cf12 = c.u32()? != 0;
    let id = c.u16()?;
    if cf12 {
        return Ok(Ext { id, icf: 0, priority: 0, stop_if_true: false, style: None, cf12 });
    }
    let icf = usize::from(c.u16()?);
    let _cp = c.u8()?;
    let _template = c.u8()?;
    let priority = u32::from(c.u16()?);
    let flags = c.u8()?;
    let has_dxf = c.u8()? != 0;
    let style = if has_dxf { read_dxfn12(c, palette, formats) } else { None };
    // `cbTemplateParm` (16) and `rgbTemplateParms`: the rule's template parameters, unmodelled.
    let parms = usize::from(c.u8()?);
    c.skip(parms.min(c.remaining()))?;
    Ok(Ext { id, icf, priority, stop_if_true: flags & 0x02 != 0, style, cf12 })
}

/// `DXFN12` (2.5.96): a byte count, then a `DXFN` and an optional `XFExtNoFRT` filling it.
fn read_dxfn12(c: &mut Cursor<'_, '_>, palette: &Palette, formats: &HashMap<u16, String>) -> Option<Style> {
    let size = c.u32().ok()? as usize;
    if size == 0 {
        return None;
    }
    let buf = c.bytes(size).ok()?;
    super::dxf::read_dxfn_prefix(&buf, palette, formats).map(|(style, _)| style)
}

/// A `CFParsedFormulaNoCCE` (2.5.198.6) as text; `None` when the tokens can't be read.
fn text(rgce: &[u8], base: CellRef, links: &Links) -> Option<String> {
    let e = decode(rgce, &[], base, super::Mode::Cell, links).ok()?;
    Some(gridcraft_formula::print(&e))
}
