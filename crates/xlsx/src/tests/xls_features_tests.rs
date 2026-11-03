//! `.xls` records for data validation, conditional formats, hyperlinks and frozen panes, each
//! checked field by field against what the `.xlsx` reader builds for the same model: the two
//! readers share the model, so a `.xls` file must open with the objects an `.xlsx` file would.
//!
//! Every fixture is assembled here from the [MS-XLS] (and, for the hyperlink object, [MS-OSHARED])
//! structures; no Excel-produced file is used.

use gridcraft_core::{CellRef, RangeRef};
use gridcraft_model::{
    Cell, CfOperator, CfRule, CfValueKind, Color, CondFormat, ErrorStyle, Fill, Font, Hyperlink, Style, Validation, ValidationKind, Workbook,
};

use super::xls_tests::{bof, cat, cell, compound, eof, palette, rec, u16s, xlstr};
use crate::{read_xls, read_xlsx, write_xlsx};

/// A one-sheet workbook: `globals` before the BoundSheet8 record, `sheet` in the worksheet.
fn book(globals: &[u8], sheet: &[u8]) -> Vec<u8> {
    let g = |pos: u32| cat(&[&bof(0x0005), globals, &rec(0x0085, &cat(&[&pos.to_le_bytes(), &[0, 0, 6, 0], b"Sheet1"])), &eof()]);
    let sheet = cat(&[&bof(0x0010), sheet, &eof()]);
    compound("Workbook", &cat(&[&g(g(0).len() as u32), &sheet]))
}

/// The `.xlsx` the model writes and reads back: the reference the `.xls` reader has to match.
fn reference(wb: &Workbook) -> Workbook {
    read_xlsx(&write_xlsx(wb).unwrap()).unwrap().0
}

// ---------------------------------------------------------------- data validation

/// `Dv` flags ([MS-XLS] 2.4.95): `valType` 4 bits, `errStyle` 3, `fStrLookup` 1, `fAllowBlank` 1,
/// `fSuppressCombo` 1, `mdImeMode` 8, `fShowInputMsg` 1, `fShowErrorMsg` 1, `typOperator` 4.
fn dv_flags(val_type: u8, err_style: u8, allow_blank: bool, suppress: bool, show_input: bool, show_error: bool, op: u8) -> u32 {
    u32::from(val_type)
        | (u32::from(err_style) << 4)
        | (u32::from(allow_blank) << 8)
        | (u32::from(suppress) << 9)
        | (u32::from(show_input) << 18)
        | (u32::from(show_error) << 19)
        | (u32::from(op) << 20)
}

/// A `DVParsedFormula` (2.5.198.9): `cce`, two unused bytes, then the tokens.
fn dv_formula(rgce: &[u8]) -> Vec<u8> {
    cat(&[&(rgce.len() as u16).to_le_bytes(), &[0, 0], rgce])
}

/// A `Dv` record with its four strings, two formulas and `SqRefU`.
fn dv(flags: u32, texts: [&str; 4], f1: &[u8], f2: &[u8], ranges: &[(u16, u16, u16, u16)]) -> Vec<u8> {
    let mut p = flags.to_le_bytes().to_vec();
    for t in texts {
        p.extend(xlstr(t));
    }
    p.extend(dv_formula(f1));
    p.extend(dv_formula(f2));
    p.extend((ranges.len() as u16).to_le_bytes());
    for (r0, r1, c0, c1) in ranges {
        p.extend(u16s(&[*r0, *r1, *c0, *c1]));
    }
    rec(0x01BE, &p)
}

fn ptg_int(n: u16) -> Vec<u8> {
    cat(&[&[0x1E], &n.to_le_bytes()])
}

fn ptg_num(n: f64) -> Vec<u8> {
    cat(&[&[0x1F], &n.to_le_bytes()])
}

/// PtgStr (0x17): a string constant.
fn ptg_str(s: &str) -> Vec<u8> {
    cat(&[&[0x17, s.len() as u8, 0], s.as_bytes()])
}

/// PtgArea (0x25) with absolute corners (no relative flags: `0x4000` column, `0x8000` row).
fn ptg_area(r0: u16, r1: u16, c0: u16, c1: u16) -> Vec<u8> {
    cat(&[&[0x25], &u16s(&[r0, r1, c0, c1])])
}

/// PtgRef (0x44) with absolute row and column.
fn ptg_ref(r: u16, c: u16) -> Vec<u8> {
    cat(&[&[0x44], &u16s(&[r, c])])
}

fn validation_model() -> Workbook {
    let mut wb = Workbook::new();
    let sheet = wb.sheet_mut(0).unwrap();
    let ranges = |a1: &str| vec![RangeRef::parse(a1).unwrap()];
    sheet.validations = vec![
        Validation {
            ranges: ranges("A1:A10"),
            kind: ValidationKind::Whole,
            op: CfOperator::Between,
            f1: "1".into(),
            f2: Some("100".into()),
            allow_blank: true,
            in_cell_dropdown: true,
            input_title: "Whole".into(),
            input_message: "Enter 1-100".into(),
            show_input: true,
            error_title: "Bad".into(),
            error_message: "1 to 100".into(),
            error_style: ErrorStyle::Stop,
            show_error: true,
        },
        Validation {
            ranges: ranges("B1:B5"),
            kind: ValidationKind::Decimal,
            op: CfOperator::NotBetween,
            f1: "0".into(),
            f2: Some("1".into()),
            allow_blank: false,
            in_cell_dropdown: true,
            input_title: String::new(),
            input_message: String::new(),
            show_input: false,
            error_title: String::new(),
            error_message: String::new(),
            error_style: ErrorStyle::Warning,
            show_error: true,
        },
        Validation {
            ranges: ranges("C1:C3"),
            kind: ValidationKind::List,
            op: CfOperator::Between,
            f1: "\"a,b,c\"".into(),
            f2: None,
            allow_blank: true,
            in_cell_dropdown: false,
            input_title: String::new(),
            input_message: String::new(),
            show_input: true,
            error_title: "Pick".into(),
            error_message: "a, b or c".into(),
            error_style: ErrorStyle::Information,
            show_error: false,
        },
        Validation {
            ranges: ranges("D1:D9"),
            kind: ValidationKind::List,
            op: CfOperator::Between,
            f1: "=$A$1:$A$5".into(),
            f2: None,
            allow_blank: true,
            in_cell_dropdown: true,
            input_title: String::new(),
            input_message: String::new(),
            show_input: true,
            error_title: String::new(),
            error_message: String::new(),
            error_style: ErrorStyle::Stop,
            show_error: true,
        },
        Validation {
            ranges: ranges("E1"),
            kind: ValidationKind::Date,
            op: CfOperator::GreaterOrEqual,
            f1: "43831".into(),
            f2: None,
            allow_blank: false,
            in_cell_dropdown: true,
            input_title: String::new(),
            input_message: String::new(),
            show_input: true,
            error_title: String::new(),
            error_message: String::new(),
            error_style: ErrorStyle::Stop,
            show_error: true,
        },
        Validation {
            ranges: ranges("F1:F2"),
            kind: ValidationKind::Time,
            op: CfOperator::Less,
            f1: "0.5".into(),
            f2: None,
            allow_blank: true,
            in_cell_dropdown: true,
            input_title: String::new(),
            input_message: String::new(),
            show_input: true,
            error_title: String::new(),
            error_message: String::new(),
            error_style: ErrorStyle::Stop,
            show_error: true,
        },
        Validation {
            ranges: ranges("G1"),
            kind: ValidationKind::TextLength,
            op: CfOperator::NotEqual,
            f1: "10".into(),
            f2: None,
            allow_blank: true,
            in_cell_dropdown: true,
            input_title: String::new(),
            input_message: String::new(),
            show_input: true,
            error_title: String::new(),
            error_message: String::new(),
            error_style: ErrorStyle::Stop,
            show_error: true,
        },
        Validation {
            ranges: ranges("H1:H4"),
            kind: ValidationKind::Custom,
            op: CfOperator::Between,
            f1: "$A$1<5".into(),
            f2: None,
            allow_blank: true,
            in_cell_dropdown: true,
            input_title: String::new(),
            input_message: String::new(),
            show_input: true,
            error_title: String::new(),
            error_message: String::new(),
            error_style: ErrorStyle::Stop,
            show_error: true,
        },
    ];
    wb
}

fn validation_xls() -> Vec<u8> {
    let rules = cat(&[
        // whole, between, 1..100, with both messages.
        &dv(dv_flags(1, 0, true, false, true, true, 0), ["Whole", "Bad", "Enter 1-100", "1 to 100"], &ptg_int(1), &ptg_int(100), &[(0, 9, 0, 0)]),
        // decimal, not between, warning style, no prompt.
        &dv(dv_flags(2, 1, false, false, false, true, 1), ["", "", "", ""], &ptg_int(0), &ptg_int(1), &[(0, 4, 1, 1)]),
        // list of literal values, the in-cell arrow suppressed, information style.
        &dv(dv_flags(3, 2, true, true, true, false, 0), ["", "Pick", "", "a, b or c"], &ptg_str("a,b,c"), &[], &[(0, 2, 2, 2)]),
        // list from a range.
        &dv(dv_flags(3, 0, true, false, true, true, 0), ["", "", "", ""], &ptg_area(0, 4, 0, 0), &[], &[(0, 8, 3, 3)]),
        // date, greater than or equal.
        &dv(dv_flags(4, 0, false, false, true, true, 6), ["", "", "", ""], &ptg_int(43831), &[], &[(0, 0, 4, 4)]),
        // time, less than.
        &dv(dv_flags(5, 0, true, false, true, true, 5), ["", "", "", ""], &ptg_num(0.5), &[], &[(0, 1, 5, 5)]),
        // text length, not equal.
        &dv(dv_flags(6, 0, true, false, true, true, 3), ["", "", "", ""], &ptg_int(10), &[], &[(0, 0, 6, 6)]),
        // custom formula.
        &dv(dv_flags(7, 0, true, false, true, true, 0), ["", "", "", ""], &cat(&[&ptg_ref(0, 0), &ptg_int(5), &[0x09]]), &[], &[(0, 3, 7, 7)]),
    ]);
    let dval = rec(0x01B2, &cat(&[&[0u8; 16], &8u32.to_le_bytes()]));
    book(&[], &cat(&[&dval, &rules]))
}

#[test]
fn data_validation_rules_match_the_xlsx_reader() {
    let model = validation_model();
    let want = reference(&model);
    // The `.xlsx` round trip keeps every field, so it is a fair reference.
    assert_eq!(want.sheets[0].validations, model.sheets[0].validations);
    let (wb, report) = read_xls(&validation_xls()).unwrap();
    let got = &wb.sheets[0].validations;
    assert_eq!(got.len(), 8);
    assert_eq!(*got, want.sheets[0].validations);
    // Field by field, so a failure says which kind broke.
    for (i, (a, b)) in got.iter().zip(want.sheets[0].validations.iter()).enumerate() {
        assert_eq!(a.kind, b.kind, "rule {i} kind");
        assert_eq!(a.op, b.op, "rule {i} operator");
        assert_eq!(a.ranges, b.ranges, "rule {i} ranges");
        assert_eq!((a.f1.as_str(), a.f2.as_deref()), (b.f1.as_str(), b.f2.as_deref()), "rule {i} formulas");
        assert_eq!((a.allow_blank, a.in_cell_dropdown), (b.allow_blank, b.in_cell_dropdown), "rule {i} flags");
        assert_eq!(
            (a.input_title.as_str(), a.input_message.as_str(), a.show_input),
            (b.input_title.as_str(), b.input_message.as_str(), b.show_input),
            "rule {i} input message"
        );
        assert_eq!(
            (a.error_title.as_str(), a.error_message.as_str(), a.error_style, a.show_error),
            (b.error_title.as_str(), b.error_message.as_str(), b.error_style, b.show_error),
            "rule {i} error message"
        );
    }
    assert!(!report.warnings.iter().any(|w| w.contains("data validation record")), "{:?}", report.warnings);
}

#[test]
fn a_validation_count_beyond_excels_limit_warns_and_still_opens() {
    // `idvMac` of a million: never an allocation, just a warning; the rules present are read.
    let dval = rec(0x01B2, &cat(&[&[0u8; 16], &1_000_000u32.to_le_bytes()]));
    let rule = dv(dv_flags(1, 0, true, false, true, true, 0), ["", "", "", ""], &ptg_int(1), &[], &[(0, 0, 0, 0)]);
    let (wb, report) = read_xls(&book(&[], &cat(&[&dval, &rule]))).unwrap();
    assert_eq!(wb.sheets[0].validations.len(), 1);
    assert!(report.warnings.iter().any(|w| w.contains("data validation count")), "{:?}", report.warnings);
}

// ---------------------------------------------------------------- conditional formats

/// A `DXFN` ([MS-XLS] 2.5.95): flags, then the font and fill parts it states.
fn dxfn(bold: bool, italic: bool, font_color: Option<u8>, fill: Option<u8>) -> Vec<u8> {
    let (mut low, mut body) = (0u32, Vec::new());
    if bold || italic || font_color.is_some() {
        low |= 1 << 26;
        body.extend(dxffntd(bold, italic, font_color));
    }
    if let Some(color) = fill {
        low |= 1 << 29;
        body.extend(dxfpat(color));
    }
    cat(&[&low.to_le_bytes(), &0u16.to_le_bytes(), &body])
}

/// `DXFFntD` (2.5.93): an absent name in its 63-byte field, a `Stxp`, and the `*Ninch` flags.
fn dxffntd(bold: bool, italic: bool, color: Option<u8>) -> Vec<u8> {
    let mut p = vec![0u8];
    p.extend([0u8; 63]);
    p.extend(u32::MAX.to_le_bytes()); // twpHeight -1: the size is not stated
    p.extend((if italic { 0x02u32 } else { 0 }).to_le_bytes());
    p.extend(if bold { 700u16 } else { 400 }.to_le_bytes());
    p.extend(0u16.to_le_bytes()); // sss
    p.push(0); // uls
    p.push(0); // family
    p.push(0); // charset
    p.push(0); // unused
    p.extend(color.map_or(u32::MAX, u32::from).to_le_bytes()); // icvFore
    p.extend([0u8; 4]); // reserved
    p.extend((0x80u32 | if italic { 0 } else { 0x02 }).to_le_bytes()); // Ts: italic, strikeout
    p.extend(0u32.to_le_bytes()); // fSssNinch
    p.extend(1u32.to_le_bytes()); // fUlsNinch: underline not stated
    p.extend(if bold { 0u32 } else { 1 }.to_le_bytes()); // fBlsNinch
    p.extend([0u8; 12]); // unused2, ich, cch
    p.extend(0u16.to_le_bytes()); // iFnt
    p
}

/// `DXFPat` (2.5.102): a solid fill in a palette colour.
fn dxfpat(color: u8) -> Vec<u8> {
    ((1u32 << 10) | (u32::from(color) << 16) | (64 << 23)).to_le_bytes().to_vec()
}

/// `CondFmt` (2.4.56): the count, `nID`, the bounding range and the `SqRefU`.
fn condfmt(nid: u16, count: u16, ranges: &[(u16, u16, u16, u16)]) -> Vec<u8> {
    let mut p = count.to_le_bytes().to_vec();
    p.extend((nid << 1).to_le_bytes());
    let (r0, r1, c0, c1) = ranges.first().copied().unwrap_or((0, 0, 0, 0));
    p.extend(u16s(&[r0, r1, c0, c1]));
    p.extend((ranges.len() as u16).to_le_bytes());
    for (r0, r1, c0, c1) in ranges {
        p.extend(u16s(&[*r0, *r1, *c0, *c1]));
    }
    rec(0x01B0, &p)
}

/// `CF` (2.4.42): the condition, its two formula sizes, the `DXFN` and the formulas.
fn cf(ct: u8, cp: u8, dxf: &[u8], f1: &[u8], f2: &[u8]) -> Vec<u8> {
    let mut p = vec![ct, cp];
    p.extend((f1.len() as u16).to_le_bytes());
    p.extend((f2.len() as u16).to_le_bytes());
    p.extend(dxf);
    p.extend(f1);
    p.extend(f2);
    rec(0x01B1, &p)
}

/// `CFEx` (2.4.44) with a `CFExNonCF12` (2.5.27): the priority, "stop if true" and a format.
fn cfex(nid: u16, icf: u16, priority: u16, stop: bool, dxf12: Option<&[u8]>) -> Vec<u8> {
    let mut p = vec![0u8; 12]; // FrtRefHeaderU
    p.extend(0u32.to_le_bytes()); // fIsCF12
    p.extend(nid.to_le_bytes());
    p.extend(icf.to_le_bytes());
    p.push(0); // cp
    p.push(0); // icfTemplate
    p.extend(priority.to_le_bytes());
    p.push(u8::from(stop) << 1); // fActive, fStopIfTrue
    match dxf12 {
        Some(d) => {
            p.push(1);
            p.extend(d);
        }
        None => p.push(0),
    }
    p.push(16); // cbTemplateParm
    p.extend([0u8; 16]); // rgbTemplateParms
    rec(0x087B, &p)
}

fn dxfn12(inner: &[u8]) -> Vec<u8> {
    cat(&[&(inner.len() as u32).to_le_bytes(), inner])
}

fn cf_style(bold: bool, italic: bool) -> Style {
    Style {
        font: Font { bold, italic, color: if bold { Color::Rgb(0xFF0000) } else { Color::Auto }, ..Font::default() },
        fill: if bold { Fill::solid(Color::Rgb(0xFFFF00)) } else { Fill::default() },
        ..Style::default()
    }
}

fn cf_model() -> Workbook {
    let mut wb = Workbook::new();
    let sheet = wb.sheet_mut(0).unwrap();
    let a = RangeRef::parse("A1:A10").unwrap();
    let b = RangeRef::parse("B1:B5").unwrap();
    let c = RangeRef::parse("C1:C20").unwrap();
    sheet.cond_formats = vec![
        CondFormat {
            ranges: vec![a],
            rule: CfRule::CellIs { op: CfOperator::Between, a: "1".into(), b: Some("100".into()), style: Box::new(cf_style(true, false)) },
            priority: 1,
            stop_if_true: false,
        },
        CondFormat {
            ranges: vec![b],
            rule: CfRule::CellIs { op: CfOperator::GreaterOrEqual, a: "$B$2".into(), b: None, style: Box::new(cf_style(true, false)) },
            priority: 2,
            stop_if_true: true,
        },
        CondFormat {
            ranges: vec![c],
            rule: CfRule::Expression { formula: "$A$1>5".into(), style: Box::new(cf_style(false, true)) },
            priority: 3,
            stop_if_true: false,
        },
    ];
    wb
}

fn cf_xls() -> Vec<u8> {
    let bold = dxfn(true, false, Some(8), Some(9));
    let italic = dxfn(false, true, None, None);
    let records = cat(&[
        &condfmt(0, 1, &[(0, 9, 0, 0)]),
        &cf(1, 1, &bold, &ptg_int(1), &ptg_int(100)),
        &condfmt(1, 1, &[(0, 4, 1, 1)]),
        &cf(1, 7, &bold, &ptg_ref(1, 1), &[]),
        &cfex(1, 0, 2, true, None),
        &condfmt(2, 1, &[(0, 19, 2, 2)]),
        &cf(2, 0, &bold, &cat(&[&ptg_ref(0, 0), &ptg_int(5), &[0x0D]]), &[]),
        &cfex(2, 0, 3, false, Some(&dxfn12(&italic))),
    ]);
    // Two palette colours: index 8 red, index 9 yellow.
    book(&palette(&[(0xFF, 0, 0), (0xFF, 0xFF, 0)]), &records)
}

#[test]
fn conditional_formats_match_the_xlsx_reader() {
    let model = cf_model();
    let want = reference(&model);
    assert_eq!(want.sheets[0].cond_formats, model.sheets[0].cond_formats);
    let (wb, report) = read_xls(&cf_xls()).unwrap();
    let got = &wb.sheets[0].cond_formats;
    assert_eq!(got.len(), 3);
    assert_eq!(*got, want.sheets[0].cond_formats);
    // The differential format came through: a bold red font on a solid yellow fill.
    let CfRule::CellIs { style, .. } = &got[0].rule else { panic!("rule 0 is not a cell comparison") };
    assert!(style.font.bold);
    assert_eq!(style.font.color, Color::Rgb(0xFF0000));
    assert_eq!(style.fill, Fill::solid(Color::Rgb(0xFFFF00)));
    // The `CFEx` set the priority and the stop flag, and its `DXFN12` replaced the format.
    assert!(got[1].stop_if_true);
    let CfRule::Expression { style, .. } = &got[2].rule else { panic!("rule 2 is not an expression") };
    assert!(style.font.italic && !style.font.bold);
    assert!(!report.warnings.iter().any(|w| w.contains("CF12")), "{:?}", report.warnings);
}

/// `CondFmt12` ([MS-XLS] 2.4.57): the `FrtRefHeaderU`, then a `CondFmtStructure`.
fn condfmt12(nid: u16, ranges: &[(u16, u16, u16, u16)]) -> Vec<u8> {
    let (r0, r1, c0, c1) = ranges.first().copied().unwrap_or((0, 0, 0, 0));
    let mut p = 0x0879u16.to_le_bytes().to_vec();
    p.extend(1u16.to_le_bytes()); // grbitFrt.fFrtRef
    p.extend(u16s(&[r0, r1, c0, c1]));
    p.extend((ranges.len() as u16).to_le_bytes()); // ccf
    p.extend((nid << 1).to_le_bytes());
    p.extend(u16s(&[r0, r1, c0, c1])); // refBound
    p.extend((ranges.len() as u16).to_le_bytes()); // sqref.cref
    for (r0, r1, c0, c1) in ranges {
        p.extend(u16s(&[*r0, *r1, *c0, *c1]));
    }
    rec(0x0879, &p)
}

/// `CF12` ([MS-XLS] 2.4.43): the header, an empty differential format and formula, the priority
/// and a 16-byte template-parameter block before the `rgbCT` payload.
fn cf12(ct: u8, priority: u16, icf: u16, rgb_ct: &[u8]) -> Vec<u8> {
    let mut p = 0x087Au16.to_le_bytes().to_vec();
    p.extend(0u16.to_le_bytes()); // grbitFrt
    p.extend([0u8; 8]); // ref8, unused for these rule types
    p.push(ct);
    p.push(0); // cp
    p.extend(0u16.to_le_bytes()); // cce1
    p.extend(0u16.to_le_bytes()); // cce2
    p.extend(0u32.to_le_bytes()); // cbDxf
    p.extend(0u16.to_le_bytes()); // DXFN12's reserved word (cbDxf == 0)
    p.extend(0u16.to_le_bytes()); // fmlaActive.cce
    p.push(0); // flags
    p.extend(priority.to_le_bytes());
    p.extend(icf.to_le_bytes());
    p.push(16); // cbTemplateParm
    p.extend([0u8; 16]);
    p.extend(rgb_ct);
    rec(0x087A, &p)
}

/// `CFColor` (2.5.21) as an RGB colour: type 2, then LongRGBA.
fn cf_rgb(r: u8, g: u8, b: u8) -> Vec<u8> {
    let mut v = 2u32.to_le_bytes().to_vec();
    v.extend((u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16)).to_le_bytes());
    v.extend(0f64.to_le_bytes());
    v
}

/// `CFVO` (2.5.39) for min/max (`[0x02]`/`[0x03]`); `number`, `percent` and `percentile` add a
/// value after the empty formula length.
fn cfvo(kind: u8, value: Option<f64>) -> Vec<u8> {
    let mut v = vec![kind, 0, 0];
    if let Some(n) = value {
        v.extend(n.to_le_bytes());
    }
    v
}

/// `CFGradient` (2.5.32): the two curves; each stop pairs an interpolation point with a colour.
fn gradient(stops: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
    let n = stops.len() as u8;
    let mut v = vec![0, 0, 0, n, n, 3]; // unused, reserved1, cInterp, cGradient, flags
    for (i, (kind, _)) in stops.iter().enumerate() {
        v.extend(kind);
        v.extend((i as f64 / (n as f64 - 1.0)).to_le_bytes()); // numDomain
    }
    for (i, (_, color)) in stops.iter().enumerate() {
        v.extend((i as f64 / (n as f64 - 1.0)).to_le_bytes()); // numGrange
        v.extend(color);
    }
    v
}

#[test]
fn cf12_rules_read_excels_encoding() {
    // The three shapes an Excel-authored .xls carries (checked against one): a colour scale with
    // min/percentile/max, a data bar and an icon set (4 traffic lights, reversed, icon only).
    let scale = gradient(&[
        (cfvo(0x02, None), cf_rgb(0xFF, 0x00, 0x00)),
        (cfvo(0x05, Some(50.0)), cf_rgb(0xFF, 0xEB, 0x84)),
        (cfvo(0x03, None), cf_rgb(0x63, 0xBE, 0x7B)),
    ]);
    let mut bar = vec![0, 0, 0]; // unused, reserved1
    bar.push(0x02); // fShowValue: Excel sets this bit when the value is hidden
    bar.extend([0, 100]); // iPercentMin, iPercentMax
    bar.extend(cf_rgb(0x63, 0x8E, 0xC6));
    bar.extend(cfvo(0x02, None));
    bar.extend(cfvo(0x03, None));
    let mut icons = vec![0, 0, 0, 4, 0x0C]; // unused, reserved1, cStates, iIconSet = 4TrafficLights
    icons.push(0x05); // fIconOnly | fReverse
    for (n, equal) in [(0.0, 1u8), (25.0, 1), (50.0, 1), (75.0, 1)] {
        icons.extend(cfvo(0x04, Some(n)));
        icons.push(equal);
        icons.extend([0u8; 4]); // unused
    }
    let records = cat(&[
        &condfmt12(1, &[(0, 9, 0, 0)]),
        &cf12(0x03, 1, 2, &scale),
        &condfmt12(2, &[(0, 9, 1, 1)]),
        &cf12(0x04, 2, 3, &bar),
        &condfmt12(3, &[(0, 9, 2, 2)]),
        &cf12(0x06, 3, 4, &icons),
        &rec(0x0203, &cat(&[&cell(0, 0, 0), &7.0f64.to_le_bytes()])),
    ]);
    let (wb, report) = read_xls(&book(&[], &records)).unwrap();
    assert_eq!(wb.sheets[0].value(CellRef::new(0, 0)), gridcraft_core::Value::Number(7.0));
    let rules = &wb.sheets[0].cond_formats;
    assert_eq!(rules.len(), 3, "{:?}", report.warnings);
    assert!(!report.warnings.iter().any(|w| w.contains("CF12") || w.contains("Colour scales")), "{:?}", report.warnings);
    // The colour scale keeps its thresholds, their order and their colours.
    let CfRule::ColorScale { stops } = &rules[0].rule else { panic!("rule 0 is not a colour scale") };
    assert_eq!(
        stops,
        &vec![
            (CfValueKind::Min, Color::Rgb(0xFF0000)),
            (CfValueKind::Percentile(50.0), Color::Rgb(0xFFEB84)),
            (CfValueKind::Max, Color::Rgb(0x63BE7B)),
        ]
    );
    assert_eq!(
        &rules[1].rule,
        &CfRule::DataBar { min: CfValueKind::Min, max: CfValueKind::Max, color: Color::Rgb(0x638EC6), gradient: true, show_value: false },
        "the hidden-value bit"
    );
    assert_eq!(
        &rules[2].rule,
        &CfRule::IconSet {
            set: "4TrafficLights".into(),
            thresholds: vec![CfValueKind::Percent(0.0), CfValueKind::Percent(25.0), CfValueKind::Percent(50.0), CfValueKind::Percent(75.0)],
            reverse: true,
            show_value: false,
        }
    );
    assert_eq!(rules[0].priority, 1);
    assert_eq!(rules[2].priority, 3);
    assert_eq!(rules[0].ranges, vec![RangeRef::parse("A1:A10").unwrap()]);
    assert_eq!(rules[2].ranges, vec![RangeRef::parse("C1:C10").unwrap()]);
}

#[test]
fn a_broken_cf12_rule_warns_and_the_file_still_opens() {
    let records = cat(&[&condfmt12(1, &[(0, 9, 0, 0)]), &rec(0x087A, &[0u8; 40]), &rec(0x0203, &cat(&[&cell(0, 0, 0), &7.0f64.to_le_bytes()]))]);
    let (wb, report) = read_xls(&book(&[], &records)).unwrap();
    assert_eq!(wb.sheets[0].value(CellRef::new(0, 0)), gridcraft_core::Value::Number(7.0));
    assert!(wb.sheets[0].cond_formats.is_empty());
    assert!(report.warnings.iter().any(|w| w.contains("conditional format")), "{:?}", report.warnings);
}

#[test]
fn a_cf12_rule_with_excels_own_bytes_matches() {
    // The rgbCT of an Excel 16-authored colour scale (min / number 50 / max, red-yellow-green),
    // copied byte for byte: 119 bytes after the 16-byte template parameters.
    let rgb_ct = [
        0x85, 0x00, 0x00, 0x03, 0x03, 0x03, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x49, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xe0, 0x3f, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf0,
        0x3f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0xff, 0x00, 0x00, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xe0, 0x3f, 0x02, 0x00, 0x00, 0x00, 0xff, 0xeb, 0x84, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf0, 0x3f, 0x02, 0x00, 0x00, 0x00, 0x63, 0xbe, 0x7b, 0xff, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00,
    ];
    let records = cat(&[&condfmt12(1, &[(0, 9, 0, 0)]), &cf12(0x03, 1, 2, &rgb_ct)]);
    let (wb, report) = read_xls(&book(&[], &records)).unwrap();
    assert!(!report.warnings.iter().any(|w| w.contains("was skipped")), "{:?}", report.warnings);
    let CfRule::ColorScale { stops } = &wb.sheets[0].cond_formats[0].rule else { panic!("not a colour scale") };
    assert_eq!(stops.len(), 3);
    assert_eq!(stops[0].0, CfValueKind::Min);
    // Excel writes the percentile it was given as a type-7 threshold with no tokens and the
    // number itself, so it reads as that number.
    assert_eq!(stops[1].0, CfValueKind::Number(50.0));
    assert_eq!(stops[2].0, CfValueKind::Max);
    assert_eq!(stops[0].1, Color::Rgb(0xFF0000));
    assert_eq!(stops[1].1, Color::Rgb(0xFFEB84));
    assert_eq!(stops[2].1, Color::Rgb(0x63BE7B));
}

#[test]
fn a_conditional_format_whose_formula_overruns_its_record_warns() {
    // `cce1` far larger than the record: refused, and the next rule still reads.
    let mut bad = vec![1u8, 1];
    bad.extend(60_000u16.to_le_bytes());
    bad.extend(0u16.to_le_bytes());
    bad.extend([0u8; 6]); // an empty DXFN
    let records = cat(&[
        &condfmt(0, 1, &[(0, 9, 0, 0)]),
        &rec(0x01B1, &bad),
        &condfmt(1, 1, &[(0, 4, 1, 1)]),
        &cf(2, 0, &dxfn(false, false, None, None), &ptg_int(1), &[]),
    ]);
    let (wb, report) = read_xls(&book(&[], &records)).unwrap();
    assert_eq!(wb.sheets[0].cond_formats.len(), 1);
    assert!(matches!(wb.sheets[0].cond_formats[0].rule, CfRule::Expression { .. }));
    assert!(report.warnings.iter().any(|w| w.contains("conditional format record")), "{:?}", report.warnings);
}

// ---------------------------------------------------------------- hyperlinks

const URL_CLSID: [u8; 16] = [0xE0, 0xC9, 0xEA, 0x79, 0xF9, 0xBA, 0xCE, 0x11, 0x8C, 0x82, 0x00, 0xAA, 0x00, 0x4B, 0xA9, 0x0B];
const FILE_CLSID: [u8; 16] = [0x03, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46];

/// A `HyperlinkString` (2.3.7.9): the character count including the null, then UTF-16.
fn hss(s: &str) -> Vec<u8> {
    let mut units: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
    units.extend([0u8, 0]);
    cat(&[&((s.encode_utf16().count() + 1) as u32).to_le_bytes(), &units])
}

fn url_moniker(url: &str) -> Vec<u8> {
    let mut units: Vec<u8> = url.encode_utf16().flat_map(u16::to_le_bytes).collect();
    units.extend([0u8, 0]);
    cat(&[&URL_CLSID, &(units.len() as u32).to_le_bytes(), &units])
}

fn file_moniker(path: &str) -> Vec<u8> {
    let mut ansi = path.as_bytes().to_vec();
    ansi.push(0);
    cat(&[
        &FILE_CLSID,
        &0u16.to_le_bytes(),                // cAnti
        &(ansi.len() as u32).to_le_bytes(), // ansiLength
        &ansi,
        &0xFFFFu16.to_le_bytes(), // endServer: not a UNC path
        &0xDEADu16.to_le_bytes(), // versionNumber
        &[0u8; 16],               // reserved1
        &0u32.to_le_bytes(),      // reserved2
        &0u32.to_le_bytes(),      // cbUnicodePathSize: the ANSI path is enough
    ])
}

/// `HLink` (2.4.140): the range, the hyperlink's CLSID and the Hyperlink Object (2.3.7.1).
fn hlink(range: (u16, u16, u16, u16), flags: u32, parts: &[&[u8]]) -> Vec<u8> {
    let mut p = u16s(&[range.0, range.1, range.2, range.3]);
    p.extend([0u8; 16]); // hlinkClsid
    p.extend(2u32.to_le_bytes()); // streamVersion
    p.extend(flags.to_le_bytes());
    for part in parts {
        p.extend_from_slice(part);
    }
    rec(0x01B8, &p)
}

fn tooltip(range: (u16, u16, u16, u16), text: &str) -> Vec<u8> {
    let mut p = 0u16.to_le_bytes().to_vec(); // rt
    p.extend(u16s(&[range.0, range.1, range.2, range.3]));
    p.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    p.extend([0u8, 0]);
    rec(0x0800, &p)
}

fn hyperlink_model() -> Workbook {
    let mut wb = Workbook::new();
    let sheet = wb.sheet_mut(0).unwrap();
    // The display text of a hyperlink is the cell's own text in both formats.
    sheet.cells.set(CellRef::new(1, 0), Cell { value: "Example".into(), formula: None, style: Default::default() });
    sheet.cells.set(CellRef::new(2, 0), Cell { value: "Report".into(), formula: None, style: Default::default() });
    sheet.hyperlinks.insert(CellRef::new(1, 0), Hyperlink { target: "https://example.com/".into(), tooltip: Some("Web".into()), external: true });
    sheet.hyperlinks.insert(CellRef::new(2, 0), Hyperlink { target: "book.xlsx#Sheet2!A1".into(), tooltip: None, external: true });
    sheet.hyperlinks.insert(CellRef::new(3, 0), Hyperlink { target: "Sheet2!A1".into(), tooltip: None, external: false });
    wb
}

fn hyperlink_xls() -> Vec<u8> {
    let records = cat(&[
        // A URL moniker, a display name that the model has no field for, and a tooltip.
        &hlink((1, 1, 0, 0), 0x01 | 0x10, &[&hss("Example"), &url_moniker("https://example.com/")]),
        &tooltip((1, 1, 0, 0), "Web"),
        // A file moniker with an in-workbook location: the two join with `#`.
        &hlink((2, 2, 0, 0), 0x01 | 0x08, &[&file_moniker("book.xlsx"), &hss("Sheet2!A1")]),
        // A location only: a place in this workbook, not an external target.
        &hlink((3, 3, 0, 0), 0x08, &[&hss("Sheet2!A1")]),
    ]);
    let cells = cat(&[&rec(0x0204, &cat(&[&cell(1, 0, 0), &xlstr("Example")])), &rec(0x0204, &cat(&[&cell(2, 0, 0), &xlstr("Report")]))]);
    book(&[], &cat(&[&cells, &records]))
}

#[test]
fn hyperlinks_match_the_xlsx_reader() {
    let model = hyperlink_model();
    let want = reference(&model);
    assert_eq!(want.sheets[0].hyperlinks, model.sheets[0].hyperlinks);
    let (wb, report) = read_xls(&hyperlink_xls()).unwrap();
    assert_eq!(wb.sheets[0].hyperlinks, want.sheets[0].hyperlinks);
    // The display text is the cell's text, as in `.xlsx`.
    assert_eq!(wb.sheets[0].value(CellRef::new(1, 0)), gridcraft_core::Value::from("Example"));
    assert_eq!(wb.sheets[0].value(CellRef::new(3, 0)), gridcraft_core::Value::Empty);
    let a2 = wb.sheets[0].hyperlinks.get(&CellRef::new(1, 0)).unwrap();
    assert_eq!((a2.target.as_str(), a2.tooltip.as_deref(), a2.external), ("https://example.com/", Some("Web"), true));
    let a3 = wb.sheets[0].hyperlinks.get(&CellRef::new(2, 0)).unwrap();
    assert_eq!((a3.target.as_str(), a3.external), ("book.xlsx#Sheet2!A1", true));
    let a4 = wb.sheets[0].hyperlinks.get(&CellRef::new(3, 0)).unwrap();
    assert_eq!((a4.target.as_str(), a4.external), ("Sheet2!A1", false));
    assert!(!report.warnings.iter().any(|w| w.contains("hyperlink record")), "{:?}", report.warnings);
}

#[test]
fn a_hyperlink_with_a_bogus_length_warns_and_the_file_still_opens() {
    let mut p = u16s(&[0, 0, 0, 0]);
    p.extend([0u8; 16]);
    p.extend(2u32.to_le_bytes());
    p.extend(1u32.to_le_bytes()); // hasMoniker
    p.extend(URL_CLSID);
    p.extend(0xFFFF_FFFFu32.to_le_bytes()); // a length no record holds
    let records = cat(&[&rec(0x01B8, &p), &rec(0x0203, &cat(&[&cell(0, 0, 0), &7.0f64.to_le_bytes()]))]);
    let (wb, report) = read_xls(&book(&[], &records)).unwrap();
    assert!(wb.sheets[0].hyperlinks.is_empty());
    assert_eq!(wb.sheets[0].value(CellRef::new(0, 0)), gridcraft_core::Value::Number(7.0));
    assert!(report.warnings.iter().any(|w| w.contains("hyperlink record")), "{:?}", report.warnings);
}

// ---------------------------------------------------------------- frozen panes

fn pane_xls(window2: &[u8], pane: &[u8]) -> Vec<u8> {
    let records = cat(&[window2, pane, &rec(0x0203, &cat(&[&cell(0, 0, 0), &7.0f64.to_le_bytes()]))]);
    book(&[], &records)
}

#[test]
fn frozen_panes_match_the_xlsx_reader() {
    let mut model = Workbook::new();
    let sheet = model.sheet_mut(0).unwrap();
    sheet.freeze = Some((2, 1));
    sheet.view_top_left = CellRef::new(3, 2);
    let want = reference(&model);
    assert_eq!(want.sheets[0].freeze, Some((2, 1)));
    // Window2 with fFrozenRt and fFrozenNoSplit, then Pane: 1 frozen column, 2 frozen rows.
    let window2 = rec(0x023E, &u16s(&[0x0002 | 0x0008 | 0x0100, 3, 2, 64, 0, 0, 0, 0, 0]));
    let pane = rec(0x0041, &cat(&[&u16s(&[1, 2, 3, 2]), &[0, 0]]));
    let (wb, _) = read_xls(&pane_xls(&window2, &pane)).unwrap();
    assert_eq!(wb.sheets[0].freeze, want.sheets[0].freeze);
    assert_eq!(wb.sheets[0].view_top_left, want.sheets[0].view_top_left);
}

#[test]
fn a_scrolled_sheet_keeps_its_top_left_cell() {
    // Without fFrozenRt the panes are not frozen; Window2's rwTop/colLeft are the scroll position,
    // which `.xlsx` stores as the view's `topLeftCell`.
    let mut model = Workbook::new();
    model.sheet_mut(0).unwrap().view_top_left = CellRef::new(4, 1);
    let want = reference(&model);
    let window2 = rec(0x023E, &u16s(&[0x0002, 4, 1, 64, 0, 0, 0, 0, 0]));
    let (wb, _) = read_xls(&pane_xls(&window2, &[])).unwrap();
    assert_eq!(wb.sheets[0].freeze, None);
    assert_eq!(wb.sheets[0].view_top_left, want.sheets[0].view_top_left);
}

#[test]
fn a_truncated_pane_record_warns_and_leaves_the_sheet_unfrozen() {
    let window2 = rec(0x023E, &u16s(&[0x0008 | 0x0100, 3, 2, 64, 0, 0, 0, 0, 0]));
    for payload in [&[][..], &[1u8][..], &[1u8, 0, 2, 0][..], &[1u8, 0, 2, 0, 3, 0, 2][..]] {
        let (wb, report) = read_xls(&pane_xls(&window2, &rec(0x0041, payload))).unwrap();
        assert_eq!(wb.sheets[0].freeze, None);
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 0)), gridcraft_core::Value::Number(7.0));
        assert!(report.warnings.iter().any(|w| w.contains("Pane") || w.contains("pane")), "{:?}", report.warnings);
    }
}

#[test]
fn a_short_window1_does_not_hide_the_active_sheet() {
    // Window1 names the selected sheet; without one the first visible sheet is active.
    let globals = rec(0x003D, &u16s(&[0, 0, 0, 0, 0x0001, 0, 0, 1, 600]));
    let (wb, _) = read_xls(&book(&globals, &[])).unwrap();
    assert_eq!(wb.active_sheet, 0);
    let (wb, _) = read_xls(&book(&rec(0x003D, &[0u8; 4]), &[])).unwrap();
    assert_eq!(wb.active_sheet, 0);
}

// ---------------------------------------------------------------- hostile input

#[test]
fn hostile_feature_records_never_panic_and_keep_the_file_open() {
    let files = [validation_xls(), cf_xls(), hyperlink_xls()];
    for file in &files {
        // Truncation anywhere.
        for cut in (0..file.len()).step_by(31) {
            let _ = read_xls(&file[..cut]);
        }
        // Every few bytes of the stream, flipped and zeroed.
        for at in (512 * 3..file.len()).step_by(5) {
            for x in [0u8, 0x7F, 0xFF] {
                let mut b = file.clone();
                b[at] = x;
                let _ = read_xls(&b);
            }
        }
    }
    // Every feature record a few bytes short is a warning, not an error.
    for id in [0x01B0u16, 0x01B1, 0x087B, 0x0879, 0x087A, 0x01B2, 0x01BE, 0x01B8, 0x0800, 0x0041] {
        for len in [0usize, 1, 2, 3, 5, 6, 7, 9, 11, 12, 15, 17, 19, 23, 40] {
            for fill in [0u8, 0xFF] {
                let records = rec(id, &vec![fill; len]);
                let file = book(&[], &records);
                let (_, report) = read_xls(&file).unwrap_or_else(|e| panic!("{id:#06x} len {len}: {e}"));
                assert!(!report.warnings.is_empty() || len == 0, "{id:#06x} len {len}");
            }
        }
    }
}
