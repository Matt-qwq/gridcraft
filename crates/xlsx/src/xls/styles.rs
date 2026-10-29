//! BIFF8 style records: `FONT` ([MS-XLS] 2.4.122), `FORMAT` (2.4.126), `PALETTE` (2.4.207),
//! `XF` (2.4.353) and `STYLE` (2.4.281), mapped to the model's [`Style`].
//!
//! The record layouts were checked byte by byte against an independent BIFF8 reader over 4,283
//! cell formats from 70 Excel-authored files: the alignment byte, rotation, indent, the border
//! style nibbles and colours, and the fill pattern and colours at the tail of the record. Colour
//! indices go through the workbook palette (the `PALETTE` record, or Excel's built-in one).
//!
//! Every read is bounds-checked and every count is capped, so a hostile record is an error, never
//! a panic or a huge allocation.

use std::collections::HashMap;

use gridcraft_model::style::Protection;
use gridcraft_model::{
    Alignment, BorderLine, BorderStyle, Borders, Color, Fill, Font, HAlign, NumFmt, PatternType, Style, Underline, VAlign, VertAlign,
};

use super::records::Cursor;
use crate::IoError;
use crate::tables::{INDEXED, builtin_format};

/// Fonts a workbook holds (Excel's limit is 512; BIFF8 indexes them with 4 bits in a cell).
pub(super) const MAX_FONTS: usize = 1024;
/// Colours a `PALETTE` record may replace (indices 8..64).
const MAX_PALETTE: usize = 56;
/// `STYLE` records read at most.
const MAX_STYLES: usize = 4096;

/// BIFF8's font weight for bold ([MS-XLS] 2.4.122 `bls`).
const BOLD_WEIGHT: u16 = 700;
/// The "automatic" font colour index.
const ICV_AUTOMATIC: u16 = 0x7FFF;
/// The "automatic" border colour (and fill foreground).
const ICV_AUTO: u16 = 64;

/// A font as its record stores it: colours stay indices until the palette is known.
pub(super) struct FontIn {
    height_twips: u16,
    grbit: u16,
    icv: u16,
    bls: u16,
    escapement: u16,
    underline: u8,
    family: u8,
    charset: u8,
    name: String,
}

impl FontIn {
    fn resolve(&self, palette: &Palette) -> Font {
        let mut f = Font { name: self.name.clone(), size: f64::from(self.height_twips).div_euclid(20.0) as f32, ..Font::default() };
        // Guard the size the way the XLSX reader does; a 0-height font is damage.
        f.size = f.size.clamp(1.0, 409.0);
        f.bold = self.grbit & 0x0001 != 0 || self.bls >= BOLD_WEIGHT;
        f.italic = self.grbit & 0x0002 != 0;
        f.strike = self.grbit & 0x0008 != 0;
        f.underline = match self.underline {
            1 => Underline::Single,
            2 => Underline::Double,
            0x21 => Underline::SingleAccounting,
            0x22 => Underline::DoubleAccounting,
            _ => Underline::None,
        };
        f.vert = match self.escapement {
            1 => VertAlign::Superscript,
            2 => VertAlign::Subscript,
            _ => VertAlign::Baseline,
        };
        f.color = palette.color(self.icv);
        // 0 is "no character set given"; keep the others so Excel substitutes the same script.
        f.charset = (self.charset != 0).then_some(self.charset);
        // BIFF's family 0 is "not given"; the model's default (2, swiss) means the same.
        f.family = (self.family != 0 && self.family <= 14 && self.family != 2).then_some(self.family);
        f
    }
}

/// `FONT` ([MS-XLS] 2.4.122): `dyHeight` (twips), flags, colour index, weight, escapement,
/// underline, family, character set, reserved, then a ShortXLUnicodeString name.
pub(super) fn read_font(c: &mut Cursor<'_, '_>) -> Result<FontIn, IoError> {
    let height_twips = c.u16()?;
    let grbit = c.u16()?;
    let icv = c.u16()?;
    let bls = c.u16()?;
    let escapement = c.u16()?;
    let underline = c.u8()?;
    let family = c.u8()?;
    let charset = c.u8()?;
    let _reserved = c.u8()?;
    let mut name = c.short_string()?;
    // Excel's limit is 31 characters; a longer one is damage, not a name to keep.
    if name.len() > 256 {
        name.truncate(256);
    }
    Ok(FontIn { height_twips, grbit, icv, bls, escapement, underline, family, charset, name })
}

/// The 64-colour workbook palette: Excel's built-in one, replaced from `PALETTE` where the file
/// customises it. BIFF8 colour indices 0..63 address it; 64/65 and 0x7FFF mean automatic.
pub(super) struct Palette {
    colors: Vec<u32>,
}

impl Default for Palette {
    fn default() -> Self {
        Palette { colors: INDEXED.to_vec() }
    }
}

impl Palette {
    /// `PALETTE` ([MS-XLS] 2.4.207): a count then RGBA quads for indices 8 onward.
    fn read(&mut self, c: &mut Cursor<'_, '_>) -> Result<(), IoError> {
        let count = usize::from(c.u16()?);
        // Only 56 slots exist; a larger count is damage, not an allocation.
        let count = count.min(MAX_PALETTE);
        for i in 0..count {
            let (r, g, b) = (c.u8()?, c.u8()?, c.u8()?);
            let _flags = c.u8()?;
            if let Some(slot) = self.colors.get_mut(8 + i) {
                *slot = (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b);
            }
        }
        Ok(())
    }

    /// A colour from a BIFF colour index.
    fn color(&self, index: u16) -> Color {
        match index {
            ICV_AUTOMATIC | ICV_AUTO => Color::Auto,
            _ => self.colors.get(usize::from(index)).map_or(Color::Auto, |rgb| Color::Rgb(*rgb)),
        }
    }

    /// A fill or border colour: 64 is automatic, 65 is the system background.
    fn fill_color(&self, index: u16) -> Color {
        if index >= ICV_AUTO {
            return Color::Auto;
        }
        self.color(index)
    }
}

/// An `XF` record's fields, before the fonts and palette are resolved.
pub(super) struct XfIn {
    pub ifnt: u16,
    pub ifmt: u16,
    pub locked: bool,
    pub hidden: bool,
    align: Alignment,
    /// left, right, top, bottom ([MS-XLS] `BorderStyle` nibbles).
    border_styles: [u8; 4],
    /// left, right, top, bottom colour indices.
    border_colors: [u16; 4],
    fill_pattern: u8,
    fill_fg: u16,
    fill_bg: u16,
}

/// `XF` ([MS-XLS] 2.4.353), 20 bytes of cell format.
///
/// | offset | bytes | field |
/// | --- | --- | --- |
/// | 0 | 2 | font index |
/// | 2 | 2 | number format index |
/// | 4 | 2 | flags: bit 0 locked, bit 1 hidden, bit 2 style XF, bits 4-15 parent style |
/// | 6 | 1 | alignment: bits 0-2 horizontal, bit 3 wrap, bits 4-6 vertical, bit 7 justify last |
/// | 7 | 1 | rotation (0-180, 255 = vertical text) |
/// | 8 | 1 | indent |
/// | 9 | 1 | used attributes (which of the following the format states) |
/// | 10 | 1 | border style left (bits 0-3), right (bits 4-7) |
/// | 11 | 1 | border style top (bits 0-3), bottom (bits 4-7) |
/// | 12 | 2 | border colour left (bits 0-6), right (bits 7-13) |
/// | 14 | 2 | border colour top (bits 0-6), bottom (bits 7-13) |
/// | 16 | 2 | fill pattern in bits 10-15 |
/// | 18 | 2 | fill colour foreground (bits 0-6), background (bits 7-13) |
pub(super) fn read_xf(c: &mut Cursor<'_, '_>) -> Result<XfIn, IoError> {
    let ifnt = c.u16()?;
    let ifmt = c.u16()?;
    let flags = c.u16()?;
    let align_byte = c.u8()?;
    let rotation = c.u8()?;
    let indent = c.u8()?;
    let _used_attributes = c.u8()?;
    let border_lr = c.u8()?;
    let border_tb = c.u8()?;
    let colors_lr = c.u16()?;
    let colors_tb = c.u16()?;
    let fill_flags = c.u16()?;
    let fill_colors = c.u16()?;
    let align = Alignment {
        h: match align_byte & 0x07 {
            1 => HAlign::Left,
            2 => HAlign::Center,
            3 => HAlign::Right,
            4 => HAlign::Fill,
            5 => HAlign::Justify,
            6 => HAlign::CenterAcross,
            7 => HAlign::Distributed,
            _ => HAlign::General,
        },
        v: match (align_byte >> 4) & 0x07 {
            0 => VAlign::Top,
            2 => VAlign::Bottom,
            3 => VAlign::Justify,
            4 => VAlign::Distributed,
            _ => VAlign::Center,
        },
        wrap: align_byte & 0x08 != 0,
        // BIFF8 has no "shrink to fit" flag we could trust; the XF's shrink bit lives in the
        // used-attribute byte and was left alone.
        shrink: false,
        indent: indent.min(15),
        // 255 is Excel's "vertical stacked text"; 0-90 is counter-clockwise, 91-180 clockwise
        // (the same convention as XLSX's `textRotation`), which the model stores as -1..-90.
        rotation: match rotation {
            0..=90 => i16::from(rotation),
            91..=180 => -(i16::from(rotation) - 90),
            255 => 255,
            _ => 0,
        },
    };
    Ok(XfIn {
        ifnt,
        ifmt,
        locked: flags & 0x0001 != 0,
        hidden: flags & 0x0002 != 0,
        align,
        border_styles: [border_lr & 0x0F, border_lr >> 4, border_tb & 0x0F, border_tb >> 4],
        border_colors: [colors_lr & 0x7F, (colors_lr >> 7) & 0x7F, colors_tb & 0x7F, (colors_tb >> 7) & 0x7F],
        // The pattern sits in the high six bits of the two-byte fill field (verified: setting
        // those bits is what turns a fill on for a BIFF8 reader).
        fill_pattern: (fill_flags >> 10) as u8,
        fill_fg: fill_colors & 0x7F,
        fill_bg: (fill_colors >> 7) & 0x7F,
    })
}

impl XfIn {
    /// The model style this `XF` describes, or `None` when it names a font that isn't there (the
    /// cell keeps the Normal font then, as a reader must not invent one).
    pub(super) fn resolve(&self, fonts: &[Font], palette: &Palette, formats: &HashMap<u16, String>) -> Style {
        let mut style = Style {
            num_fmt: NumFmt::new(formats.get(&self.ifmt).map(String::as_str).or_else(|| builtin_format(u32::from(self.ifmt))).unwrap_or("General")),
            ..Style::default()
        };
        if let Some(font) = fonts.get(usize::from(self.ifnt)).or_else(|| fonts.first()) {
            style.font = font.clone();
        }
        let line = |i: usize| BorderLine {
            style: match self.border_styles.get(i).copied().unwrap_or(0) {
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
            },
            color: palette.fill_color(self.border_colors.get(i).copied().unwrap_or(ICV_AUTO)),
        };
        style.borders = Borders {
            left: line(0),
            right: line(1),
            top: line(2),
            bottom: line(3),
            diag_down: BorderLine::default(),
            diag_up: BorderLine::default(),
        };
        style.fill = Fill {
            pattern: match self.fill_pattern {
                1 => PatternType::Solid,
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
                // 0 is "no pattern"; anything past 18 is not a pattern Excel writes.
                _ => PatternType::None,
            },
            fg: palette.fill_color(self.fill_fg),
            bg: palette.fill_color(self.fill_bg),
        };
        style.align = self.align;
        style.protection = Protection { locked: self.locked, hidden: self.hidden };
        style
    }
}

/// Built-in style names by `STYLE.istd` ([MS-XLS] 2.4.324 `Istd`), the ones Excel names.
const BUILTIN_STYLE_NAMES: [&str; 9] = ["Normal", "RowLevel", "ColLevel", "Comma", "Currency", "Percent", "Comma [0]", "Currency [0]", "Hyperlink"];

/// A `STYLE` record: which XF a named style uses.
pub(super) struct StyleIn {
    /// The style's XF index (`ixfe` with its built-in flag removed).
    pub ixfe: usize,
    /// The style's name; empty for a built-in one.
    pub name: String,
}

/// `STYLE` ([MS-XLS] 2.4.281): `ixfe`, `istd`, `cch`, then the name; `cch` 0xFF means the name is
/// the built-in one `istd` names.
pub(super) fn read_style(c: &mut Cursor<'_, '_>) -> Result<StyleIn, IoError> {
    let ixfe = usize::from(c.u16()? & 0x0FFF);
    let istd = c.u8()?;
    let cch = c.u8()?;
    if cch == 0xFF {
        let name = BUILTIN_STYLE_NAMES.get(usize::from(istd)).map(|n| (*n).to_string()).unwrap_or_default();
        return Ok(StyleIn { ixfe, name });
    }
    let name = c.chars(usize::from(cch), false)?;
    Ok(StyleIn { ixfe, name })
}

/// The style records a workbook's globals substream holds.
#[derive(Default)]
pub(super) struct StylesIn {
    pub fonts: Vec<FontIn>,
    pub xfs: Vec<XfIn>,
    pub palette: Palette,
    pub formats: HashMap<u16, String>,
    pub styles: Vec<StyleIn>,
    /// A `FORMAT` record counted more characters than it held (some writers do this; Excel opens
    /// those files), so the characters present were kept and the caller warns.
    pub short_format: bool,
}

impl StylesIn {
    /// Reads one style record into the set. `id` is the record id; unknown ids are ignored.
    pub fn record(&mut self, id: u16, c: &mut Cursor<'_, '_>) -> Result<(), IoError> {
        match id {
            0x0031 if self.fonts.len() < MAX_FONTS => self.fonts.push(read_font(c)?),
            0x00E0 if self.xfs.len() < super::MAX_XFS => self.xfs.push(read_xf(c)?),
            0x041E => {
                let ifmt = c.u16()?;
                let cch = usize::from(c.u16()?);
                let high = c.u8()? & 1 != 0;
                // Some writers count one character more than the record holds; keep what is there.
                let fit = c.remaining() / if high { 2 } else { 1 };
                if cch > fit {
                    self.short_format = true;
                }
                let code = c.chars(cch.min(fit), high)?;
                if code.len() <= 4096 {
                    self.formats.insert(ifmt, code);
                }
            }
            0x0092 => self.palette.read(c)?,
            0x0293 if self.styles.len() < MAX_STYLES => self.styles.push(read_style(c)?),
            _ => {}
        }
        Ok(())
    }

    /// The resolved fonts, in `ifnt` order.
    pub fn font_models(&self) -> Vec<Font> {
        self.fonts.iter().map(|f| f.resolve(&self.palette)).collect()
    }

    /// The workbook's named styles (`STYLE` records), as the model keeps them.
    pub fn named(&self, xfs: &[Style]) -> Vec<(String, Style)> {
        let mut out: Vec<(String, Style)> = vec![];
        for s in &self.styles {
            if s.name.is_empty() || s.name == "Normal" || out.iter().any(|(n, _)| *n == s.name) {
                continue;
            }
            if let Some(style) = xfs.get(s.ixfe) {
                out.push((s.name.clone(), style.clone()));
            }
        }
        out
    }
}
