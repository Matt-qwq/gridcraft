//! Conditional formats: `CondFmt` ([MS-XLS] 2.4.56), `CF` (2.4.42), `CFEx` (2.4.44),
//! `CondFmt12` (2.4.57) and `CF12` (2.4.43).
//!
//! A `CondFmt` record names the ranges and the id; the `CF` records that follow it carry one rule
//! each (a cell-value comparison or a formula), and a `CFEx` record adds the priority, the
//! "stop if true" flag and — when it has one — a differential format. `CondFmt12` does the same
//! for `CF12` rules — colour scales, data bars and icon sets — whose parameters are the `rgbCT`
//! structure after the rule's 16-byte `rgbTemplateParms`: a `CFGradient`, `CFDatabar` or
//! `CFMultistate` (2.5.32, 2.5.22, 2.5.36). The layout was checked against an Excel-authored
//! `.xls` (a colour scale with min/percentile/max, a data bar and an icon set) and the
//! specification.

use std::collections::HashMap;

use gridcraft_core::{CellRef, RangeRef};
use gridcraft_model::{CfOperator, CfRule, CfValueKind, Color, Style};

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
    structure(c)
}

/// `CondFmt12` (2.4.57): an `FrtRefHeaderU` (`rt` and `grbitFrt`) and then the same
/// `CondFmtStructure` (2.5.56) a `CondFmt` record holds.
pub(super) fn condfmt12(c: &mut Cursor<'_, '_>) -> Result<Group, IoError> {
    c.skip(12)?;
    structure(c)
}

/// `CondFmtStructure` (2.5.56): `ccf`, `fToughRecalc` + `nID`, the bounding `Ref8U`, `SqRefU`.
fn structure(c: &mut Cursor<'_, '_>) -> Result<Group, IoError> {
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

/// A `CF12` rule ([MS-XLS] 2.4.43): a colour scale, a data bar or an icon set.
pub(super) struct Cf12 {
    pub priority: u32,
    pub stop_if_true: bool,
    pub rule: CfRule,
}

/// `CF12` (2.4.43): the header, the empty differential format and formulas these rule types use,
/// the priority, then `rgbTemplateParms` (unmodelled) and the `rgbCT` the type selects.
pub(super) fn cf12(c: &mut Cursor<'_, '_>, base: CellRef, links: &Links, palette: &Palette) -> Result<Option<Cf12>, IoError> {
    c.skip(12)?; // FrtRefHeaderU: rt, grbitFrt, ref8 (unused for these rules)
    let ct = c.u8()?;
    let _cp = c.u8()?;
    let cce1 = usize::from(c.u16()?);
    let cce2 = usize::from(c.u16()?);
    let cb_dxf = c.u32()? as usize;
    if cb_dxf == 0 {
        // DXFN12's reserved word is present only when the differential format is empty.
        c.skip(2)?;
    } else {
        if cb_dxf > c.remaining() {
            return Err(invalid("a conditional format's differential format runs past its record"));
        }
        c.skip(cb_dxf)?;
    }
    if cce1.saturating_add(cce2) > c.remaining() {
        return Err(invalid("a conditional format's formulas run past the end of their record"));
    }
    c.skip(cce1.saturating_add(cce2))?;
    // fmlaActive: CFParsedFormula (2.5.198.5), a length and its tokens.
    let active = usize::from(c.u16()?);
    if active > c.remaining() {
        return Err(invalid("a conditional format's activity formula runs past its record"));
    }
    c.skip(active)?;
    let flags = c.u8()?;
    let priority = u32::from(c.u16()?);
    let _template = c.u16()?;
    let parms = usize::from(c.u8()?);
    if parms > c.remaining() {
        return Err(invalid("a conditional format's template parameters run past its record"));
    }
    c.skip(parms)?;
    let rule = match ct {
        0x03 => color_scale(c, base, links, palette)?,
        0x04 => data_bar(c, base, links, palette)?,
        0x06 => icon_set(c, base, links)?,
        // 0x01 and 0x02 are cell comparisons and formulas, which only a `CF` record carries.
        _ => return Ok(None),
    };
    Ok(Some(Cf12 { priority, stop_if_true: flags & 0x02 != 0, rule }))
}

/// `CFGradient` (2.5.32): the interpolation curve (`CFGradientInterpItem`s) and the gradient
/// curve (`CFGradientItem`s) of a colour scale.
fn color_scale(c: &mut Cursor<'_, '_>, base: CellRef, links: &Links, palette: &Palette) -> Result<CfRule, IoError> {
    c.skip(2)?; // unused
    let _reserved = c.u8()?;
    let interp = usize::from(c.u8()?);
    let curve = usize::from(c.u8()?);
    let _flags = c.u8()?;
    // The specification allows two or three control points; anything else is damage.
    if !(2..=3).contains(&interp) || interp != curve {
        return Err(invalid("a colour scale has an unusable number of control points"));
    }
    let mut kinds: Vec<CfValueKind> = Vec::with_capacity(interp);
    for _ in 0..interp {
        kinds.push(cfvo(c, base, links)?);
        let _domain = c.f64()?;
    }
    let mut stops = Vec::with_capacity(curve);
    for kind in kinds {
        let _value = c.f64()?;
        stops.push((kind, cf_color(c, palette)?));
    }
    Ok(CfRule::ColorScale { stops })
}

/// `CFDatabar` (2.5.22): the axis direction and value flags, the length percentages, the colour
/// and the two bounds.
fn data_bar(c: &mut Cursor<'_, '_>, base: CellRef, links: &Links, palette: &Palette) -> Result<CfRule, IoError> {
    c.skip(2)?; // unused
    let _reserved = c.u8()?;
    let flags = c.u8()?;
    let _percent_min = c.u8()?;
    let _percent_max = c.u8()?;
    let color = cf_color(c, palette)?;
    let min = cfvo(c, base, links)?;
    let max = cfvo(c, base, links)?;
    // Excel's BIFF writer sets bit 1 for a data bar whose value is *hidden* (checked against an
    // Excel-authored file: ShowValue=false wrote 0x02, the default wrote 0x00), the opposite of
    // what [MS-XLS] 2.5.22 says of `fShowValue`; Excel is the reference and reads it that way.
    // Excel 2007 stores only gradient bars; a solid fill is an Excel 2010 extension.
    Ok(CfRule::DataBar { min, max, color, gradient: true, show_value: flags & 0x02 == 0 })
}

/// `CFMultistate` (2.5.36): the icon set, its direction flags and one threshold per icon.
fn icon_set(c: &mut Cursor<'_, '_>, base: CellRef, links: &Links) -> Result<CfRule, IoError> {
    c.skip(2)?; // unused
    let _reserved = c.u8()?;
    let states = usize::from(c.u8()?);
    let set = c.u8()?;
    let flags = c.u8()?;
    let name = ICON_SETS.get(usize::from(set)).copied().ok_or_else(|| invalid("a conditional format names an icon set that does not exist"))?;
    if !(2..=5).contains(&states) {
        return Err(invalid("an icon set has an unusable number of icons"));
    }
    let mut thresholds = Vec::with_capacity(states);
    for _ in 0..states {
        thresholds.push(cfvo(c, base, links)?);
        let _equal = c.u8()?;
        c.skip(4)?; // unused
    }
    Ok(CfRule::IconSet { set: name.to_string(), thresholds, reverse: flags & 0x04 != 0, show_value: flags & 0x01 == 0 })
}

/// `CFVO` (2.5.39): how a threshold is computed, then its value or formula.
fn cfvo(c: &mut Cursor<'_, '_>, base: CellRef, links: &Links) -> Result<CfValueKind, IoError> {
    let ty = c.u8()?;
    let cce = usize::from(c.u16()?);
    if cce > c.remaining() {
        return Err(invalid("a conditional format's value formula runs past its record"));
    }
    let rgce = c.bytes(cce)?;
    let num = if cce == 0 && !matches!(ty, 0x02 | 0x03) { Some(c.f64()?) } else { None };
    let kind = match ty {
        0x02 => CfValueKind::Min,
        0x03 => CfValueKind::Max,
        0x04 => CfValueKind::Percent(num.ok_or_else(|| invalid("a percent threshold has no value"))?),
        0x05 => CfValueKind::Percentile(num.ok_or_else(|| invalid("a percentile threshold has no value"))?),
        // Excel writes a plain number as type 1, and a threshold it calls a formula as type 7
        // with no tokens and the number itself (seen in an Excel-authored colour scale).
        0x01 => CfValueKind::Number(num.ok_or_else(|| invalid("a number threshold has no value"))?),
        0x07 => match num {
            Some(n) => CfValueKind::Number(n),
            None => CfValueKind::Formula(text(&rgce, base, links).unwrap_or_default()),
        },
        _ => return Err(invalid("a conditional format has an unknown threshold type")),
    };
    Ok(kind)
}

/// `CFColor` (2.5.21): the reference type (RGB, palette index or theme colour), its value and a
/// tint in [-1, 1].
fn cf_color(c: &mut Cursor<'_, '_>, palette: &Palette) -> Result<Color, IoError> {
    let ty = c.u32()?;
    let value = c.u32()?;
    let tint = c.f64()?;
    let tint = if tint.is_finite() { (tint * 1000.0).round().clamp(-1000.0, 1000.0) as i16 } else { 0 };
    Ok(match ty {
        0x0000 => Color::Auto,
        0x0001 => u16::try_from(value).map(|i| palette.color(i)).unwrap_or(Color::Auto),
        // LongRGBA: red, green, blue and alpha, one byte each.
        0x0002 => Color::rgb(value as u8, (value >> 8) as u8, (value >> 16) as u8),
        0x0003 => Color::Theme(theme_index(value), tint),
        _ => Color::Auto,
    })
}

/// `ColorTheme`'s order (2.5.49: Dark 1, Light 1, Dark 2, Light 2, Accent 1-6, hyperlink,
/// followed) to the model's (`theme`: Light 1, Dark 1, Light 2, Dark 2, Accent 1-6, …).
fn theme_index(i: u32) -> u8 {
    match i {
        0 => 1,
        1 => 0,
        2 => 3,
        3 => 2,
        n @ 4..=11 => n as u8,
        _ => 0,
    }
}

/// `CFMultistate.iIconSet` (2.5.36) names, in ECMA-376 `ST_IconSetType` order (the order Excel's
/// own `IconSets` collection and the model's `.xlsx` names use).
const ICON_SETS: [&str; 17] = [
    "3Arrows",
    "3ArrowsGray",
    "3Flags",
    "3TrafficLights1",
    "3TrafficLights2",
    "3Signs",
    "3Symbols",
    "3Symbols2",
    "4Arrows",
    "4ArrowsGray",
    "4RedToBlack",
    "4Rating",
    "4TrafficLights",
    "5Arrows",
    "5ArrowsGray",
    "5Rating",
    "5Quarters",
];

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
