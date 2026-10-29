//! Data validation: `DVal` ([MS-XLS] 2.4.96) and `Dv` (2.4.95), mapped to the model's
//! [`Validation`] the way the `.xlsx` reader maps a `dataValidation` element: the same kinds, the
//! same operator names, a literal list kept quoted and a list range given its leading `=`.
//!
//! The `Dv` record's flags are packed into one 32-bit word (2.4.95's diagram: `valType` 4 bits,
//! `errStyle` 3, `fStrLookup` 1, `fAllowBlank` 1, `fSuppressCombo` 1, `mdImeMode` 8,
//! `fShowInputMsg` 1, `fShowErrorMsg` 1, `typOperator` 4, `reserved` 8), not into two bytes.

use gridcraft_core::{CellRef, RangeRef};
use gridcraft_model::{CfOperator, ErrorStyle, Validation, ValidationKind};

use super::records::Cursor;
use super::{Budget, Links, decode, invalid, ref8};
use crate::IoError;

/// The most `Dv` records one `DVal` may announce ([MS-XLS] 2.4.96: `idvMac` ≤ 65,534).
pub(super) const MAX_DV_COUNT: usize = 65_534;

/// `DVal` (2.4.96): flags, the input window's place, the drop-down object, then the count of `Dv`
/// records that follow. Returns that count.
pub(super) fn dval(c: &mut Cursor<'_, '_>) -> Result<usize, IoError> {
    c.skip(16)?;
    Ok(c.u32()? as usize)
}

/// `Dv` (2.4.95): one rule. `None` when its `sqref` names no range inside the grid.
pub(super) fn dv(c: &mut Cursor<'_, '_>, links: &Links, budget: &mut Budget) -> Result<Option<Validation>, IoError> {
    let flags = c.u32()?;
    let val_type = (flags & 0x0F) as u8;
    let err_style = ((flags >> 4) & 0x07) as u8;
    let allow_blank = flags & (1 << 8) != 0;
    let suppress_combo = flags & (1 << 9) != 0;
    let show_input = flags & (1 << 18) != 0;
    let show_error = flags & (1 << 19) != 0;
    let operator = ((flags >> 20) & 0x0F) as u8;
    let input_title = c.string()?;
    let error_title = c.string()?;
    let input_message = c.string()?;
    let error_message = c.string()?;
    budget.text(input_title.len().saturating_add(error_title.len()).saturating_add(input_message.len()).saturating_add(error_message.len()))?;
    let f1 = formula(c)?;
    let f2 = formula(c)?;
    let mut ranges: Vec<RangeRef> = vec![];
    let count = usize::from(c.u16()?);
    // A `SqRefU` of 8,192 ranges is the spec's limit; the record's own length caps it further.
    for _ in 0..count.min(0x2000).min(c.remaining() / 8) {
        if let Some(range) = ref8(c)? {
            ranges.push(range);
        }
    }
    if ranges.is_empty() {
        return Ok(None);
    }
    let kind = match val_type {
        0x1 => ValidationKind::Whole,
        0x2 => ValidationKind::Decimal,
        0x3 => ValidationKind::List,
        0x4 => ValidationKind::Date,
        0x5 => ValidationKind::Time,
        0x6 => ValidationKind::TextLength,
        0x7 => ValidationKind::Custom,
        // 0 (any value) and anything the spec does not name.
        _ => ValidationKind::Any,
    };
    // `valType` 0, 3 and 7 leave the operator undefined (2.4.95 `typOperator`); the model keeps
    // the default there, as an `.xlsx` rule without an `operator` attribute does.
    let op = match operator {
        0x1 => CfOperator::NotBetween,
        0x2 => CfOperator::Equal,
        0x3 => CfOperator::NotEqual,
        0x4 => CfOperator::Greater,
        0x5 => CfOperator::Less,
        0x6 => CfOperator::GreaterOrEqual,
        0x7 => CfOperator::LessOrEqual,
        _ => CfOperator::Between,
    };
    let base = ranges.first().map_or_else(CellRef::default, |r| r.start);
    let mut f1 = text(&f1, base, links);
    if kind == ValidationKind::List && !f1.is_empty() && !f1.starts_with('"') {
        f1 = format!("={f1}");
    }
    // `formula2` is only meaningful for "between" and "not between" on a typed rule (2.4.95).
    let typed = !matches!(kind, ValidationKind::Any | ValidationKind::List | ValidationKind::Custom);
    let f2 = (typed && matches!(op, CfOperator::Between | CfOperator::NotBetween) && !f2.is_empty())
        .then(|| text(&f2, base, links))
        .filter(|s| !s.is_empty());
    Ok(Some(Validation {
        ranges,
        kind,
        op,
        f1,
        f2,
        allow_blank,
        // `fSuppressCombo` hides the in-cell list arrow, as `showDropDown="1"` does.
        in_cell_dropdown: !suppress_combo,
        input_title,
        input_message,
        show_input,
        error_title,
        error_message,
        error_style: match err_style {
            0x1 => ErrorStyle::Warning,
            0x2 => ErrorStyle::Information,
            _ => ErrorStyle::Stop,
        },
        show_error,
    }))
}

/// A `DVParsedFormula` (2.5.198.9): `cce`, two unused bytes, then the tokens.
fn formula(c: &mut Cursor<'_, '_>) -> Result<Vec<u8>, IoError> {
    let cce = usize::from(c.u16()?);
    c.skip(2)?;
    if cce > c.remaining() {
        return Err(invalid("a data validation formula runs past the end of its record"));
    }
    let mut out = Vec::with_capacity(cce.min(1024));
    for _ in 0..cce {
        out.push(c.u8()?);
    }
    Ok(out)
}

/// A rule's formula as text; empty when the tokens can't be read (the rule keeps its bounds).
fn text(rgce: &[u8], base: CellRef, links: &Links) -> String {
    if rgce.is_empty() {
        return String::new();
    }
    match decode(rgce, &[], base, super::Mode::Cell, links) {
        Ok(e) => gridcraft_formula::print(&e),
        Err(_) => String::new(),
    }
}
