//! Differential formats: `DXFN` ([MS-XLS] 2.5.95) and its parts (`DXFNum` 2.5.99, `DXFFntD`
//! 2.5.93, `DXFALC` 2.5.91, `DXFBdr` 2.5.92, `DXFPat` 2.5.102, `DXFProt` 2.5.103), mapped to the
//! model's [`Style`] the way the `.xlsx` reader maps a `<dxf>`: an unstated part keeps the default,
//! and [`DxfStated`] records the few a style alone cannot tell from unset.
//!
//! `DXFN` is variable-length, so its parts are read in the order the structure lists and the
//! caller checks that the whole buffer was consumed; a format that doesn't fit is refused (the
//! rule then keeps a default style) rather than guessed at.

use std::collections::HashMap;

use gridcraft_model::style::{DxfStated, Protection};
use gridcraft_model::{
    Alignment, BorderLine, BorderStyle, Borders, Color, Fill, Font, HAlign, NumFmt, PatternType, Style, Underline, VAlign, VertAlign,
};

use super::records::Cursor;
use super::styles::Palette;
use crate::tables::builtin_format;

/// The font weight BIFF8 calls bold.
const BOLD_WEIGHT: i16 = 700;
/// `DXFFntD.icvFore`: the colour is not part of this format.
const ICV_NONE: u32 = u32::MAX;
/// `DXFFntD.icvFore`: the default foreground colour.
const ICV_DEFAULT_FG: u32 = 0x7FFF;

/// A `DXFN` as a model style, or `None` when its parts don't fill `bytes` exactly.
pub(super) fn read_dxfn(bytes: &[u8], palette: &Palette, formats: &HashMap<u16, String>) -> Option<Style> {
    let (style, used) = read_dxfn_prefix(bytes, palette, formats)?;
    (used == bytes.len()).then_some(style)
}

/// A `DXFN` at the start of `bytes`, with the number of bytes it took: `DXFN12` (2.5.96) puts an
/// `XFExtNoFRT` after it, which is not modelled.
pub(super) fn read_dxfn_prefix(bytes: &[u8], palette: &Palette, formats: &HashMap<u16, String>) -> Option<(Style, usize)> {
    let parts = [bytes];
    let mut c = Cursor::new(&parts);
    let style = read_dxfn_in(&mut c, palette, formats)?;
    Some((style, bytes.len() - c.remaining()))
}

/// `DXFN`'s fields, in order, each present only when its `ibitAtr*` bit is set.
fn read_dxfn_in(c: &mut Cursor<'_, '_>, palette: &Palette, formats: &HashMap<u16, String>) -> Option<Style> {
    let low = c.u32().ok()?;
    let high = c.u16().ok()?;
    let has = |bit: u32| low & (1 << bit) != 0;
    let mut style = Style::default();
    if has(25) {
        // Bit 32 (`fIfmtUser`) picks `DXFNumUsr` over `DXFNumIFmt`; `DXFNum` has no discriminant.
        style.num_fmt = read_dxfnum(c, high & 1 != 0, formats)?;
    }
    if has(26) {
        let (font, stated) = read_dxfn_font(c, palette)?;
        apply_stated(&mut style.dxf_stated, &stated);
        style.font = font;
    }
    if has(27) {
        style.align = read_dxfalc(c, low, &style.align)?;
    }
    if has(28) {
        style.borders = read_dxfbdr(c, low, palette)?;
    }
    if has(29) {
        style.fill = read_dxfpat(c, low, palette)?;
    }
    if has(30) {
        let prot = c.u16().ok()?;
        style.protection = Protection { locked: prot & 1 != 0, hidden: prot & 2 != 0 };
        style.dxf_stated.protection = style.protection == Protection::default();
    }
    Some(style)
}

/// `DXFNum` (2.5.99): `DXFNumIFmt` (2.5.100) when `fIfmtUser` is 0, else `DXFNumUsr` (2.5.101).
fn read_dxfnum(c: &mut Cursor<'_, '_>, user: bool, formats: &HashMap<u16, String>) -> Option<NumFmt> {
    if !user {
        c.skip(1).ok()?;
        let ifmt = u16::from(c.u8().ok()?);
        return Some(NumFmt::new(formats.get(&ifmt).map(String::as_str).or_else(|| builtin_format(u32::from(ifmt))).unwrap_or("General")));
    }
    let before = c.remaining();
    let cb = usize::from(c.u16().ok()?);
    let code = c.string().ok()?;
    let used = before - c.remaining();
    // `cb` counts the whole structure; accept either reading of it, refuse anything else.
    if cb != used && cb.checked_add(2) != Some(used) {
        return None;
    }
    Some(NumFmt::new(if code.is_empty() { "General" } else { &code }))
}

/// Which font attributes a `DXFFntD` states at all (the rest are not part of the format).
struct Stated {
    name: Option<String>,
    height: Option<u16>,
    italic: Option<bool>,
    strike: Option<bool>,
    bold: Option<bool>,
    underline: Option<Underline>,
}

/// `DXFFntD` (2.5.93): the font name in a fixed 63-byte field, a `Stxp` (2.5.248) and the
/// `*Ninch` flags that say which attributes to ignore.
fn read_dxfn_font(c: &mut Cursor<'_, '_>, palette: &Palette) -> Option<(Font, Stated)> {
    let cch = usize::from(c.u8().ok()?);
    let mut name = None;
    if cch == 0 {
        // No name: `stFontName` is absent and `unused1` is the whole 63 bytes.
        c.skip(63).ok()?;
    } else {
        // 62 compressed or 31 UTF-16 characters at most, so the name fits its 63-byte field.
        if cch > 62 {
            return None;
        }
        let high = c.u8().ok()? & 1 != 0;
        let s = c.chars(cch, high).ok()?;
        let used = 1 + if high { cch * 2 } else { cch };
        if used > 63 {
            return None;
        }
        c.skip(63 - used).ok()?;
        name = Some(s);
    }
    let height = c.u32().ok()? as i32;
    let ts = c.u32().ok()?;
    let weight = c.u16().ok()? as i16;
    let sss = c.u16().ok()? as i16;
    let uls = c.u8().ok()?;
    let family = c.u8().ok()?;
    let charset = c.u8().ok()?;
    c.skip(1).ok()?;
    let icv = c.u32().ok()?;
    c.skip(4).ok()?;
    let ts_ninch = c.u32().ok()?;
    let sss_ninch = c.u32().ok()? != 0;
    let uls_ninch = c.u32().ok()? != 0;
    let bls_ninch = c.u32().ok()? != 0;
    // unused2, ich, cch, iFnt: the last 14 bytes of the 118-byte structure.
    c.skip(14).ok()?;
    let height = u16::try_from(height).ok().filter(|h| (20..=8191).contains(h));
    let italic = (ts_ninch & 0x02 == 0).then_some(ts & 0x02 != 0);
    let strike = (ts_ninch & 0x80 == 0).then_some(ts & 0x80 != 0);
    let bold = (!bls_ninch).then_some(weight >= BOLD_WEIGHT);
    let underline = (!uls_ninch).then_some(match uls {
        0x01 => Underline::Single,
        0x02 => Underline::Double,
        0x21 => Underline::SingleAccounting,
        0x22 => Underline::DoubleAccounting,
        _ => Underline::None,
    });
    let mut font = Font::default();
    if let Some(n) = &name {
        font.name = n.clone();
    }
    if let Some(h) = height {
        font.size = (f64::from(h) / 20.0) as f32;
    }
    font.italic = italic.unwrap_or(font.italic);
    font.strike = strike.unwrap_or(font.strike);
    font.bold = bold.unwrap_or(font.bold);
    font.underline = underline.unwrap_or(font.underline);
    if !sss_ninch {
        font.vert = match sss {
            1 => VertAlign::Superscript,
            2 => VertAlign::Subscript,
            _ => VertAlign::Baseline,
        };
    }
    if icv != ICV_NONE {
        font.color = if icv == ICV_DEFAULT_FG { Color::Auto } else { palette.color(icv as u16) };
    }
    // 0 is "no character set given"; BIFF's family 0 is "not given" and 2 is the model's default.
    font.charset = (charset != 0).then_some(charset);
    font.family = (family != 0 && family <= 14 && family != 2).then_some(family);
    Some((font, Stated { name, height, italic, strike, bold, underline }))
}

/// The attributes a `DXFFntD` states, as [`DxfStated`] keeps them: only where they equal the
/// default, so a format read from a file and one built here stay equal.
fn apply_stated(dxf: &mut DxfStated, f: &Stated) {
    let def = Font::default();
    dxf.bold = f.bold == Some(false);
    dxf.italic = f.italic == Some(false);
    dxf.strike = f.strike == Some(false);
    dxf.underline = f.underline == Some(Underline::None);
    dxf.size = f.height.is_some_and(|h| (f64::from(h) / 20.0) as f32 == def.size);
    dxf.name = f.name.as_deref() == Some(def.name.as_str());
}

/// `DXFALC` (2.5.91): the alignment a format states; `*Ninch` flags in `DXFN` say which to ignore.
fn read_dxfalc(c: &mut Cursor<'_, '_>, flags: u32, base: &Alignment) -> Option<Alignment> {
    let b0 = c.u8().ok()?;
    let trot = c.u8().ok()?;
    let b2 = c.u8().ok()?;
    c.skip(1).ok()?;
    let i_indent = c.u32().ok()? as i32;
    let ninch = |bit: u32| flags & (1 << bit) != 0;
    let mut al = *base;
    if !ninch(0) {
        al.h = match b0 & 0x07 {
            1 => HAlign::Left,
            2 => HAlign::Center,
            3 => HAlign::Right,
            4 => HAlign::Fill,
            5 => HAlign::Justify,
            6 => HAlign::CenterAcross,
            7 => HAlign::Distributed,
            _ => HAlign::General,
        };
    }
    if !ninch(2) {
        al.wrap = b0 & 0x08 != 0;
    }
    if !ninch(1) {
        al.v = match (b0 >> 4) & 0x07 {
            0 => VAlign::Top,
            2 => VAlign::Bottom,
            3 => VAlign::Justify,
            4 => VAlign::Distributed,
            _ => VAlign::Center,
        };
    }
    if !ninch(3) {
        al.rotation = match trot {
            0..=90 => i16::from(trot),
            91..=180 => -(i16::from(trot) - 90),
            255 => 255,
            _ => 0,
        };
    }
    if !ninch(5) {
        let indent = i32::from(b2 & 0x0F);
        // `iIndent` 255 means "the absolute level is the one in `cIndent`"; otherwise it is a
        // level relative to the previous format.
        al.indent = if i_indent == 255 { indent as u8 } else { (indent + i_indent).clamp(0, 255) as u8 };
    }
    if !ninch(6) {
        al.shrink = b2 & 0x10 != 0;
    }
    Some(al)
}

/// `DXFBdr` (2.5.92): the four borders, their colours, and the diagonals.
fn read_dxfbdr(c: &mut Cursor<'_, '_>, flags: u32, palette: &Palette) -> Option<Borders> {
    let low = c.u32().ok()?;
    let high = c.u32().ok()?;
    let ninch = |bit: u32| flags & (1 << bit) != 0;
    let line = |dg: u32, icv: u32, ignored: bool| {
        if ignored || dg == 0 {
            return BorderLine::default();
        }
        BorderLine { style: border_style(dg), color: palette.fill_color(icv as u16) }
    };
    let dg = [low & 0x0F, (low >> 4) & 0x0F, (low >> 8) & 0x0F, (low >> 12) & 0x0F];
    let icv = [high & 0x7F, (high >> 7) & 0x7F, (high >> 16) & 0x7F, (high >> 23) & 0x7F];
    let diag_style = border_style((high >> 21) & 0x0F);
    let diag_color = palette.fill_color(((high >> 14) & 0x7F) as u16);
    let diag = |on: bool, ignored: bool| {
        if on && !ignored { BorderLine { style: diag_style, color: diag_color } } else { BorderLine::default() }
    };
    Some(Borders {
        left: line(dg[0], icv[0], ninch(10)),
        right: line(dg[1], icv[1], ninch(11)),
        top: line(dg[2], icv[2], ninch(12)),
        bottom: line(dg[3], icv[3], ninch(13)),
        diag_down: diag(high & (1 << 30) != 0, ninch(14)),
        diag_up: diag(high & (1 << 31) != 0, ninch(15)),
    })
}

/// `BorderStyle` (2.5.15) as the model names it.
fn border_style(dg: u32) -> BorderStyle {
    match dg {
        1 => BorderStyle::Thin,
        2 => BorderStyle::Medium,
        3 => BorderStyle::Dashed,
        4 => BorderStyle::Dotted,
        5 => BorderStyle::Thick,
        6 => BorderStyle::Double,
        7 => BorderStyle::Hair,
        8 => BorderStyle::MediumDashed,
        9 => BorderStyle::DashDot,
        10 => BorderStyle::MediumDashDot,
        11 => BorderStyle::DashDotDot,
        12 => BorderStyle::MediumDashDotDot,
        13 => BorderStyle::SlantDashDot,
        _ => BorderStyle::None,
    }
}

/// `DXFPat` (2.5.102): the fill pattern and its two colours.
fn read_dxfpat(c: &mut Cursor<'_, '_>, flags: u32, palette: &Palette) -> Option<Fill> {
    let v = c.u32().ok()?;
    let ninch = |bit: u32| flags & (1 << bit) != 0;
    let fls = (v >> 10) & 0x3F;
    let fg = (v >> 16) & 0x7F;
    let bg = (v >> 23) & 0x7F;
    // A differential fill states a colour, not a pattern: "solid" is how Excel writes one, and it
    // is the single colour the `.xlsx` reader keeps from a `<patternFill>` in a dxf.
    if fls == 0 {
        return Some(Fill::default());
    }
    let fg = if ninch(17) { Color::Auto } else { palette.fill_color(fg as u16) };
    if fls == 1 {
        return Some(Fill::solid(fg));
    }
    Some(Fill {
        pattern: match fls {
            2 => PatternType::Gray50,
            3 => PatternType::Gray75,
            4 => PatternType::Gray25,
            5 => PatternType::DarkHorizontal,
            6 => PatternType::DarkVertical,
            7 => PatternType::DarkDown,
            8 => PatternType::DarkUp,
            9 => PatternType::DarkGrid,
            10 => PatternType::DarkTrellis,
            11 => PatternType::LightHorizontal,
            12 => PatternType::LightVertical,
            13 => PatternType::LightDown,
            14 => PatternType::LightUp,
            15 => PatternType::LightGrid,
            16 => PatternType::LightTrellis,
            17 => PatternType::Gray125,
            18 => PatternType::Gray0625,
            _ => PatternType::None,
        },
        fg,
        bg: if ninch(18) { Color::Auto } else { palette.fill_color(bg as u16) },
    })
}
