//! Legacy Excel 97-2003 `.xls` (BIFF8) import, implemented from Microsoft's public
//! specifications [MS-CFB] (the compound file container) and [MS-XLS] (the Workbook stream):
//! <https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-xls/cd03cb5f-ca02-4934-a391-bb674cb8aa06>.
//!
//! Read: worksheets, shared strings, cell values, formulas (text and cached results, including
//! shared and array formulas), number formats, the 1904 date system, merged cells, column
//! widths, row heights, defined names, print areas, print titles and the calculation settings
//! (mode, iteration, precision as displayed). Style records (_FONT_, _FORMAT_, _PALETTE_, _XF_,
//! _STYLE_) are read into the workbook's style table, so cells keep their font, fill, borders,
//! alignment and number format; cell comments are read with their author and text. Data
//! validation (_DVAL_/_DV_), conditional formats (_CONDFMT_/_CF_/_CFEX_, the cell-value and
//! formula rules), hyperlinks (_HLINK_/_HLINKTOOLTIP_) and frozen panes (_PANE_/_WINDOW1_/_WINDOW2_)
//! are read into the same model objects the `.xlsx` reader builds. Pictures, embedded charts,
//! autoshapes and text boxes are read too (charts with their series ranges, kind, title and
//! legend; shapes from Excel 2007+'s DrawingML fragment, or from the OfficeArt type and anchor
//! for an older writer); chart sheets, OLE objects, controls and PivotTables are skipped with a
//! warning. Files encrypted with FILEPASS and pre-BIFF8 workbooks are refused with a message.

mod chart;
mod condfmt;
mod dxf;
mod formula;
mod ftab;
mod hyperlink;
mod officeart;
mod records;
mod styles;
mod validation;

use std::collections::HashMap;
use std::sync::Arc;

use gridcraft_core::{CellRef, DateSystem, MAX_COLS, MAX_ROWS, RangeRef, Value};
use gridcraft_model::{
    CalcMode, CalcSettings, Cell, CfRule, Comment, CondFormat, DefinedName, Formula, LineInfo, Sheet, Style, StyleId, StyleTable, Visibility,
    Workbook,
};

use crate::cfb::Compound;
use crate::tables::{col_width_to_px, pt_to_px};
use crate::{IoError, ReadReport};
use formula::{Links, Mode, SupBook, Unsupported, Xti, cell_error, decode};
use records::{Cursor, Record, Records};
use styles::StylesIn;

const MAX_CELLS: usize = 4_000_000;
const MAX_STRINGS: usize = 4_000_000;
const MAX_TEXT: usize = 256 * 1024 * 1024;
const MAX_SHEETS: usize = 4096;
const MAX_NAMES: usize = 65_536;
const MAX_XFS: usize = 65_536;
const MAX_MERGES: usize = 100_000;
/// Comments read per sheet (Excel allows far fewer).
const MAX_COMMENTS: usize = 65_536;
/// Hyperlink cells written per sheet (the `.xlsx` reader's limit, so both formats agree).
const MAX_HYPERLINK_CELLS: usize = 10_000;
/// Conditional format rules kept per sheet.
const MAX_CONDFMTS: usize = 65_536;
/// Embedded charts and shapes read from one sheet.
const MAX_CHARTS: usize = 1024;
const MAX_SHAPES: usize = 4096;
/// Records read from the whole Workbook stream at most (a 65,536 × 256 sheet has fewer). One
/// allowance shared by every substream: many BoundSheet records pointing into one huge
/// substream must not each get their own.
const MAX_RECORDS: usize = 40_000_000;

const BOF: u16 = 0x0809;
const EOF: u16 = 0x000A;
const BIFF8: u16 = 0x0600;
/// WsBool (MS-XLS 2.4.351): its first byte's `fDialog` bit marks a dialog sheet, whose BOF says worksheet.
const WSBOOL: u16 = 0x0081;
const WSBOOL_DIALOG: u8 = 0x10;

pub(crate) fn invalid(msg: impl Into<String>) -> IoError {
    IoError::Format(format!("XLS: {}", msg.into()))
}

/// The message for an encrypted workbook (a FilePass record).
const ENCRYPTED: &str =
    "this .xls workbook is password-protected (encrypted). GridCraft can't open encrypted .xls files; remove the password in Excel and try again.";

/// Whether the bytes are a compound file holding an Excel workbook stream (BIFF8 `Workbook`
/// or BIFF5 `Book`).
pub(crate) fn is_xls(bytes: &[u8]) -> bool {
    let Ok(cf) = Compound::open(bytes) else { return false };
    cf.has_root_stream("Workbook") || cf.has_root_stream("Book")
}

pub(crate) struct Budget {
    cells: usize,
    text: usize,
    records: usize,
    max_records: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Budget { cells: 0, text: 0, records: 0, max_records: MAX_RECORDS }
    }
}

impl Budget {
    /// A budget with a smaller record allowance (tests).
    #[cfg(test)]
    pub(crate) fn with_max_records(max_records: usize) -> Self {
        Budget { max_records, ..Budget::default() }
    }
    fn record(&mut self) -> Result<(), IoError> {
        self.records = self.records.saturating_add(1);
        if self.records > self.max_records {
            return Err(IoError::TooLarge("XLS: too many records".into()));
        }
        Ok(())
    }
    fn cell(&mut self) -> Result<(), IoError> {
        self.cells += 1;
        if self.cells > MAX_CELLS {
            return Err(IoError::TooLarge("XLS: more than 4 million cells".into()));
        }
        Ok(())
    }
    fn text(&mut self, len: usize) -> Result<(), IoError> {
        self.text = self.text.saturating_add(len);
        if self.text > MAX_TEXT {
            return Err(IoError::TooLarge("XLS: more than 256 MB of text".into()));
        }
        Ok(())
    }
}

struct BoundSheet {
    pos: usize,
    name: String,
    visibility: Visibility,
    kind: u8,
}

struct Lbl {
    name: String,
    builtin: Option<u8>,
    hidden: bool,
    macro_name: bool,
    itab: u16,
    rgce: Vec<u8>,
    rgcb: Vec<u8>,
}

#[derive(Default)]
struct Globals {
    sheets: Vec<BoundSheet>,
    date1904: bool,
    styles: StylesIn,
    sst: Vec<Arc<str>>,
    links: Links,
    names: Vec<Lbl>,
    precision_as_displayed: bool,
    /// `Window1.itabCur` ([MS-XLS] 2.4.345): the selected sheet, in BoundSheet8 order.
    itab_cur: Option<u16>,
    /// The workbook's `MSODRAWINGGROUP` payload and the blips it stores: one picture store for
    /// every sheet's drawings.
    dgg: Vec<u8>,
    blips: Vec<officeart::Blip>,
}

/// Reads an Excel 97-2003 workbook. Inspect the report's warnings for what was skipped.
pub fn read_xls(bytes: &[u8]) -> Result<(Workbook, ReadReport), IoError> {
    read_xls_with(bytes, Budget::default())
}

/// [`read_xls`] with an explicit resource budget.
pub(crate) fn read_xls_with(bytes: &[u8], mut budget: Budget) -> Result<(Workbook, ReadReport), IoError> {
    let cf = Compound::open(bytes).map_err(invalid)?;
    let stream = match cf.root_stream("Workbook").map_err(invalid)? {
        Some(s) => s,
        None => {
            if cf.has_root_stream("Book") {
                return Err(invalid("Excel 5.0/95 workbooks (BIFF5) are not supported; save the file as .xlsx in Excel and try again."));
            }
            return Err(invalid("the file has no Workbook stream, so it is not an Excel workbook"));
        }
    };
    let mut report = ReadReport::default();
    let g = globals(&stream, &mut budget, &mut report)?;

    let mut wb = Workbook::new();
    wb.sheets.clear();
    wb.date_system = if g.date1904 { DateSystem::D1904 } else { DateSystem::D1900 };
    // One model style per XF, carrying the whole cell format: font, fill, borders, alignment,
    // number format and protection. XF 0 is the Normal style, which every other format builds on.
    let fonts = g.styles.font_models();
    let resolved: Vec<Style> = g.styles.xfs.iter().map(|xf| xf.resolve(&fonts, &g.styles.palette, &g.styles.formats)).collect();
    if let Some(normal) = resolved.first() {
        wb.styles = StyleTable::with_default(normal.clone());
    }
    let styles: Vec<StyleId> =
        resolved.iter().enumerate().map(|(i, s)| if i == 0 { StyleId::DEFAULT } else { wb.styles.intern(s.clone()) }).collect();
    wb.cell_styles = g.styles.named(&resolved);

    // BoundSheet8 index → model sheet index.
    let mut model_index: Vec<Option<usize>> = Vec::with_capacity(g.sheets.len());
    for bs in &g.sheets {
        if bs.kind != 0 {
            report.warn(match bs.kind {
                2 => "Chart sheets in .xls files are not imported yet and were skipped.",
                1 => "Excel 4.0 macro sheets were skipped.",
                _ => "VBA modules and other non-worksheet sheets were skipped.",
            });
            model_index.push(None);
            continue;
        }
        wb.check_sheet_name(&bs.name, None).map_err(invalid)?;
        let Some((mut sheet, calc)) = worksheet(&stream, bs, &g, &styles, &mut budget, &mut report)? else {
            report.warn("Dialog sheets and unreadable sheets were skipped.");
            model_index.push(None);
            continue;
        };
        sheet.visibility = bs.visibility;
        // Every worksheet repeats the workbook's calculation settings; Excel applies the first.
        if wb.sheets.is_empty() {
            wb.calc = calc;
        }
        model_index.push(Some(wb.sheets.len()));
        wb.sheets.push(Arc::new(sheet));
    }
    if wb.sheets.is_empty() {
        return Err(invalid("the workbook has no worksheets GridCraft can read"));
    }
    wb.calc.precision_as_displayed = g.precision_as_displayed;
    defined_names(&g, &model_index, &mut wb, &mut report);
    // Window1 names the selected sheet in BoundSheet8 order, as `activeTab` does in `.xlsx`.
    let active = g
        .itab_cur
        .and_then(|i| model_index.get(usize::from(i)).copied().flatten())
        .filter(|i| wb.sheets.get(*i).is_some_and(|s| s.visibility == Visibility::Visible));
    wb.active_sheet = active.or_else(|| wb.sheets.iter().position(|s| s.visibility == Visibility::Visible)).unwrap_or(0);
    // The overview comes first.
    report.warnings.insert(
        0,
        "Imported from an Excel 97-2003 (.xls) workbook: values, formulas, fonts, fills, borders, alignment, number formats, merged cells, column widths, row heights, comments, data validation, conditional formats (including colour scales, data bars and icon sets), hyperlinks, frozen panes and defined names, pictures, embedded charts, shapes and text boxes were read. Chart sheets, OLE objects and controls are not imported yet. Save as .xlsx; saving .xls is not supported.".into(),
    );
    Ok((wb, report))
}

fn bof(rec: &Record<'_>) -> Result<(u16, u16), IoError> {
    if rec.id != BOF {
        return Err(invalid("a substream does not start with a BOF record"));
    }
    let mut c = Cursor::new(&rec.parts);
    Ok((c.u16()?, c.u16()?))
}

fn globals(stream: &[u8], budget: &mut Budget, report: &mut ReadReport) -> Result<Globals, IoError> {
    let mut recs = Records::new(stream, 0);
    let first = recs.next_record()?.ok_or_else(|| invalid("empty Workbook stream"))?;
    let (vers, dt) = bof(&first)?;
    if vers != BIFF8 {
        return Err(invalid(if vers == 0x0500 {
            "Excel 5.0/95 workbooks (BIFF5) are not supported; save the file as .xlsx in Excel and try again."
        } else {
            "unsupported BIFF version (only Excel 97-2003 BIFF8 files are read)"
        }));
    }
    if dt != 0x0005 {
        return Err(invalid("the Workbook stream does not start with the workbook globals"));
    }
    let mut g = Globals::default();
    let mut offsets = std::collections::HashSet::new();
    while let Some(rec) = recs.next_record()? {
        budget.record()?;
        let mut c = Cursor::new(&rec.parts);
        match rec.id {
            EOF => break,
            // The drawing group (OfficeArt's blip store) belongs to the workbook, not a sheet.
            0x00EB => g.dgg.extend(c.rest()),
            0x002F => return Err(invalid(ENCRYPTED)),
            0x0085 => {
                if g.sheets.len() >= MAX_SHEETS {
                    return Err(IoError::TooLarge("XLS: too many sheets".into()));
                }
                let pos = c.u32()? as usize;
                if !offsets.insert(pos) {
                    return Err(invalid("two sheets point at the same substream"));
                }
                let state = c.u8()?;
                let kind = c.u8()?;
                let name = c.short_string()?;
                let visibility = match state & 3 {
                    0 => Visibility::Visible,
                    1 => Visibility::Hidden,
                    _ => Visibility::VeryHidden,
                };
                g.links.sheets.push(name.clone());
                g.sheets.push(BoundSheet { pos, name, visibility, kind });
            }
            0x0022 => g.date1904 = c.u16()? == 1,
            0x000E => g.precision_as_displayed = c.u16()? == 0,
            // Window1 ([MS-XLS] 2.4.345): the window's place, flags, then itabCur.
            0x003D => {
                if c.remaining() >= 12 {
                    c.skip(10)?;
                    g.itab_cur = Some(c.u16()?);
                }
            }
            // Font, number format, palette, cell format and named style records.
            0x0031 | 0x041E | 0x0092 | 0x00E0 | 0x0293 => g.styles.record(rec.id, &mut c)?,
            0x00FC => g.sst = shared_strings(&mut c, budget)?,
            0x01AE => {
                let _ctab = c.u16()?;
                let cch = c.u16()?;
                g.links.books.push(SupBook { internal: cch == 0x0401, addin: cch == 0x3A01, names: vec![] });
            }
            0x0023 => {
                // ExternName: flags, then an AddinUdf or ExternDocName body; both start with
                // four bytes before the name.
                c.u16()?;
                c.skip(4)?;
                let name = c.short_string()?;
                if let Some(book) = g.links.books.last_mut()
                    && book.names.len() < MAX_NAMES
                {
                    book.names.push(name);
                }
            }
            0x0017 => {
                let n = usize::from(c.u16()?);
                for _ in 0..n.min(c.remaining() / 6) {
                    let book = c.u16()?;
                    let first = c.u16()? as i16;
                    let last = c.u16()? as i16;
                    g.links.xti.push(Xti { book, first, last });
                }
            }
            0x0018 => {
                if g.names.len() >= MAX_NAMES {
                    continue;
                }
                let lbl = label(&mut c)?;
                g.links.names.push(lbl.name.clone());
                g.names.push(lbl);
            }
            _ => {}
        }
    }
    g.blips = officeart::read_blips(&g.dgg);
    // The same test `defined_names` applies, before any cell formula is decoded.
    let skipped = g.names.iter().map(|l| !l.rgce.is_empty() && decode(&l.rgce, &l.rgcb, CellRef::new(0, 0), Mode::Name, &g.links).is_err()).collect();
    g.links.skipped = skipped;
    if g.styles.short_format {
        report.warn("Some number formats were shorter than their declared length; the characters present were kept.");
    }
    Ok(g)
}

/// Built-in defined names ([MS-XLS] 2.4.150, Lbl.Name).
const BUILTIN_NAMES: [&str; 14] = [
    "Consolidate_Area",
    "Auto_Open",
    "Auto_Close",
    "Extract",
    "Database",
    "Criteria",
    "Print_Area",
    "Print_Titles",
    "Recorder",
    "Data_Form",
    "Auto_Activate",
    "Auto_Deactivate",
    "Sheet_Title",
    "_FilterDatabase",
];

fn label(c: &mut Cursor<'_, '_>) -> Result<Lbl, IoError> {
    let flags = c.u16()?;
    let _key = c.u8()?;
    let cch = usize::from(c.u8()?);
    let cce = usize::from(c.u16()?);
    c.skip(2)?;
    let itab = c.u16()?;
    c.skip(4)?;
    let high = c.u8()? & 1 != 0;
    let raw = c.chars(cch, high)?;
    let builtin = flags & 0x0020 != 0;
    let (name, builtin) = if builtin {
        let code = raw.chars().next().map_or(0, |ch| ch as u32);
        let code = u8::try_from(code).unwrap_or(u8::MAX);
        (BUILTIN_NAMES.get(usize::from(code)).map_or_else(|| raw.clone(), |n| (*n).to_string()), Some(code))
    } else {
        (raw, None)
    };
    let rest = c.rest();
    let rgce = rest.get(..cce).unwrap_or(&rest).to_vec();
    let rgcb = rest.get(cce..).unwrap_or(&[]).to_vec();
    Ok(Lbl { name, builtin, hidden: flags & 1 != 0, macro_name: flags & 0x000A != 0, itab, rgce, rgcb })
}

fn shared_strings(c: &mut Cursor<'_, '_>, budget: &mut Budget) -> Result<Vec<Arc<str>>, IoError> {
    let _total = c.u32()?;
    let unique = c.u32()? as usize;
    if unique > MAX_STRINGS {
        return Err(IoError::TooLarge("XLS: too many shared strings".into()));
    }
    // Each string takes at least three bytes: the declared count can't exceed what's there.
    let mut out = Vec::with_capacity(unique.min(c.remaining() / 3));
    for _ in 0..unique {
        if c.remaining() == 0 {
            break;
        }
        let s = c.rich_string()?;
        budget.text(s.len())?;
        out.push(Arc::from(s));
    }
    Ok(out)
}

/// A formula cell waiting for its shared or array formula (which follows it in the file).
struct Pending {
    pos: CellRef,
    anchor: CellRef,
}

/// A parsed formula: tokens and extra data.
struct Parsed {
    rgce: Vec<u8>,
    rgcb: Vec<u8>,
}

fn parsed(c: &mut Cursor<'_, '_>) -> Result<Parsed, IoError> {
    let cce = usize::from(c.u16()?);
    let rest = c.rest();
    if cce > rest.len() {
        return Err(invalid("formula runs past the end of its record"));
    }
    Ok(Parsed { rgce: rest.get(..cce).unwrap_or(&[]).to_vec(), rgcb: rest.get(cce..).unwrap_or(&[]).to_vec() })
}

struct SheetState<'g> {
    g: &'g Globals,
    styles: &'g [StyleId],
    sheet: Sheet,
    pending: Vec<Pending>,
    shared: HashMap<CellRef, Parsed>,
    arrays: HashMap<CellRef, (RangeRef, Parsed)>,
    last_formula: Option<CellRef>,
    /// The cell waiting for its String record, with its style: a blank cell isn't stored, so a
    /// formula kept as its value may have no cell to receive the text.
    pending_string: Option<(CellRef, StyleId)>,
    /// `NOTE` records: the cell and the comment's author, in file order.
    notes: Vec<(CellRef, String)>,
    /// The text of each comment, from the `TXO` records that follow the notes.
    note_texts: Vec<String>,
    dropped: usize,
    unsupported: Vec<&'static str>,
    calc: CalcSettings,
    /// The `CondFmt` whose `CF` records follow ([MS-XLS] 2.4.56).
    cf_group: Option<condfmt::Group>,
    /// How many `CF` records the group still expects.
    cf_left: usize,
    /// `CondFmt` id → the rules it holds, for the `CFEx` records that extend them.
    cf_ids: HashMap<u16, Vec<usize>>,
    /// The priority the next rule gets: rules apply in the order the file lists them.
    cf_seq: u32,
    /// The `DVal` count of the sheet's validation rules, and how many `Dv` records were seen.
    dv_expected: Option<usize>,
    dv_seen: usize,
    dv_overflow: bool,
    /// The `CondFmt12` whose `CF12` records follow ([MS-XLS] 2.4.57).
    cf12_group: Option<condfmt::Group>,
    /// How many `CF12` records the group still expects.
    cf12_left: usize,
    /// A `CF12` rule reached through a `CFEx` record, which this reader does not follow.
    cf12_via_cfex: bool,
    /// The sheet's `MSODRAWING` payloads: the drawings, one OfficeArt tree when concatenated.
    drawing: Vec<u8>,
    /// How many chart substreams the sheet held so far (skipped ones count: a chart's anchor is
    /// the shape at its substream's position).
    chart_index: usize,
    /// The substream position of each chart kept, parallel to `sheet.charts`.
    chart_anchor_index: Vec<usize>,
    /// Hyperlinks and their tooltips, paired once the whole sheet has been read.
    links: Vec<hyperlink::Link>,
    tooltips: Vec<(RangeRef, String)>,
    link_cells: usize,
    /// `Pane` ([MS-XLS] 2.4.189): x, y, rwTop, colLeft.
    pane: Option<(u16, u16, u16, u16)>,
    /// `Window2.fFrozenRt`: the panes are frozen.
    frozen: bool,
    /// `Window2.rwTop` and `colLeft`: the first visible row and column.
    scrolled_to: Option<(u16, u16)>,
}

impl SheetState<'_> {
    /// Records why a formula was kept as its value (each reason once).
    fn unsupported(&mut self, why: &'static str) {
        if !self.unsupported.contains(&why) {
            self.unsupported.push(why);
        }
    }

    fn style(&self, ixfe: u16) -> StyleId {
        self.styles.get(usize::from(ixfe)).copied().unwrap_or(StyleId::DEFAULT)
    }

    fn pos(&mut self, row: u16, col: u16) -> Option<CellRef> {
        let (r, c) = (u32::from(row), u32::from(col));
        if r >= MAX_ROWS || c >= MAX_COLS.min(256) {
            self.dropped += 1;
            return None;
        }
        Some(CellRef::new(r, c))
    }

    fn put(&mut self, budget: &mut Budget, row: u16, col: u16, ixfe: u16, value: Value) -> Result<Option<CellRef>, IoError> {
        let Some(pos) = self.pos(row, col) else { return Ok(None) };
        let style = self.style(ixfe);
        if value.is_empty() && style == StyleId::DEFAULT {
            return Ok(Some(pos));
        }
        budget.cell()?;
        self.sheet.cells.set(pos, Cell { value, formula: None, style });
        Ok(Some(pos))
    }

    /// Decodes a formula and attaches it to the cell at `pos`, or records why it can't be.
    fn attach(&mut self, pos: CellRef, p: &Parsed, mode: Mode, array: Option<RangeRef>) {
        match decode(&p.rgce, &p.rgcb, pos, mode, &self.g.links) {
            Ok(e) => {
                let text = gridcraft_formula::print(&e);
                let mut f = Formula::new(&text);
                if f.expr.is_none() {
                    self.unsupported("syntax GridCraft can't parse yet");
                    return;
                }
                f.array = array.filter(|r| !r.is_single());
                if let Some(mut cell) = self.sheet.cells.get(pos) {
                    cell.formula = Some(Arc::new(f));
                    self.sheet.cells.set(pos, cell);
                } else {
                    self.sheet.cells.set(pos, Cell { value: Value::Empty, formula: Some(Arc::new(f)), style: StyleId::DEFAULT });
                }
            }
            Err(Unsupported(why)) => self.unsupported(why),
        }
    }
}

/// RK number ([MS-XLS] 2.5.217): a 30-bit integer or the top of an IEEE double, maybe ×100.
fn rk(v: u32) -> Value {
    let n = if v & 2 != 0 { f64::from((v as i32) >> 2) } else { f64::from_bits(u64::from(v & !3) << 32) };
    Value::number(if v & 1 != 0 { n / 100.0 } else { n })
}

/// A `Ref8U` ([MS-XLS] 2.5.209): first and last row, then first and last column. `None` when the
/// range is inverted or starts outside the grid (the columns are 8-bit in BIFF8, so only 256
/// exist).
fn ref8(c: &mut Cursor<'_, '_>) -> Result<Option<RangeRef>, IoError> {
    let (r0, r1, c0, c1) = (c.u16()?, c.u16()?, c.u16()?, c.u16()?);
    let (r0, r1, c0, c1) = (u32::from(r0), u32::from(r1), u32::from(c0), u32::from(c1));
    if r1 < r0 || c1 < c0 || c0 >= MAX_COLS.min(256) || r0 >= MAX_ROWS {
        return Ok(None);
    }
    Ok(Some(RangeRef::new(CellRef::new(r0, c0), CellRef::new(r1.min(MAX_ROWS - 1), c1.min(MAX_COLS.min(256) - 1)))))
}

fn worksheet(
    stream: &[u8],
    bs: &BoundSheet,
    g: &Globals,
    styles: &[StyleId],
    budget: &mut Budget,
    report: &mut ReadReport,
) -> Result<Option<(Sheet, CalcSettings)>, IoError> {
    if bs.pos >= stream.len() {
        return Err(invalid(format!("sheet '{}' points outside the Workbook stream", bs.name)));
    }
    let mut recs = Records::new(stream, bs.pos);
    let first = recs.next_record()?.ok_or_else(|| invalid("missing sheet substream"))?;
    let (vers, dt) = bof(&first)?;
    if vers != BIFF8 || dt != 0x0010 {
        return Ok(None);
    }
    let mut st = SheetState {
        g,
        styles,
        sheet: Sheet::new(&bs.name),
        pending: vec![],
        shared: HashMap::new(),
        arrays: HashMap::new(),
        last_formula: None,
        pending_string: None,
        notes: vec![],
        note_texts: vec![],
        dropped: 0,
        unsupported: vec![],
        calc: CalcSettings::default(),
        cf_group: None,
        cf_left: 0,
        cf_ids: HashMap::new(),
        cf_seq: 0,
        cf12_group: None,
        cf12_left: 0,
        cf12_via_cfex: false,
        drawing: vec![],
        chart_index: 0,
        chart_anchor_index: vec![],
        dv_expected: None,
        dv_seen: 0,
        dv_overflow: false,
        links: vec![],
        tooltips: vec![],
        link_cells: 0,
        pane: None,
        frozen: false,
        scrolled_to: None,
    };
    let mut depth = 1usize;
    while let Some(rec) = recs.next_record()? {
        budget.record()?;
        match rec.id {
            BOF => {
                // An embedded chart's substream: read it as a chart, then continue the sheet.
                let (_, dt) = bof(&rec)?;
                depth += 1;
                if depth == 2 && dt == 0x0020 {
                    let at = st.chart_index;
                    st.chart_index = st.chart_index.saturating_add(1);
                    match chart::read(&mut recs, budget, &st.g.links, None) {
                        Ok(Some(mut chart)) => {
                            if st.sheet.charts.len() < MAX_CHARTS {
                                chart.id = next_object_id(&st.sheet);
                                st.sheet.charts.push(chart);
                                st.chart_anchor_index.push(at);
                            }
                        }
                        Ok(None) => {}
                        Err(IoError::Format(_)) => {
                            report.warn("An embedded chart in an .xls file could not be imported (an unsupported kind or damaged records).")
                        }
                        Err(e) => return Err(e),
                    }
                    depth -= 1;
                }
                continue;
            }
            EOF => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
                continue;
            }
            _ if depth > 1 => continue,
            WSBOOL if rec.parts.first().and_then(|p| p.first()).is_some_and(|b| b & WSBOOL_DIALOG != 0) => return Ok(None),
            _ => {}
        }
        sheet_record(&rec, &mut st, budget, report)?;
    }
    // Shared and array formulas: their definitions follow the first cell that uses them.
    let (arrays, shared) = (std::mem::take(&mut st.arrays), std::mem::take(&mut st.shared));
    let pending = std::mem::take(&mut st.pending);
    for p in &pending {
        if let Some((range, f)) = arrays.get(&p.anchor) {
            if p.pos == p.anchor {
                st.attach(p.pos, f, Mode::Cell, Some(*range));
            }
        } else if let Some(f) = shared.get(&p.anchor) {
            st.attach(p.pos, f, Mode::Shared, None);
        } else {
            st.unsupported("a shared formula that is missing from the file");
        }
    }
    // The other cells of a legacy array formula hold only cached results: dropped, as when an
    // .xlsx is read, so that the formula fills its range on recalculation. If the formula itself
    // couldn't be imported, they keep their values.
    for p in &pending {
        let filled = st.sheet.cell(p.anchor).and_then(|c| c.formula).and_then(|f| f.array).is_some_and(|r| r.contains(p.pos));
        if p.pos != p.anchor && filled {
            let style = st.sheet.style_id(p.pos);
            st.sheet.cells.set(p.pos, Cell { value: Value::Empty, formula: None, style });
        }
    }
    if st.dropped > 0 {
        report.warn(format!("{} cells outside the grid were skipped.", st.dropped));
    }
    // Comments: a `NOTE` record names the cell and its author; the text that follows the matching
    // `TXO` record is paired with it in file order (Excel writes one of each per comment).
    let (notes, texts) = (std::mem::take(&mut st.notes), std::mem::take(&mut st.note_texts));
    if !notes.is_empty() && texts.len() != notes.len() {
        report.warn("Some cell comments were missing their text (the comment object was not in the file).");
    }
    for (i, (at, author)) in notes.into_iter().enumerate() {
        let text = texts.get(i).cloned().unwrap_or_default();
        budget.text(text.len())?;
        st.sheet
            .comments
            .insert(at, Comment { author, text, replies: vec![], threaded: false, resolved: false, visible: false, dates: vec![], note_box: None });
    }
    for why in &st.unsupported {
        report.warn(format!("Some formulas use {why}; their saved results were kept as values."));
    }
    // Frozen panes: `Pane` holds the split and `Window2.fFrozenRt` says it is frozen. `rwTop` and
    // `colLeft` are the bottom-right pane's first cell, which is the scroll position, as a frozen
    // `.xlsx` pane's `topLeftCell` is.
    if let Some((x, y, rw_top, col_left)) = st.pane
        && st.frozen
    {
        let rows = u32::from(y).min(MAX_ROWS - 1);
        let cols = u32::from(x).min(MAX_COLS - 1);
        if rows > 0 || cols > 0 {
            st.sheet.freeze = Some((rows, cols));
            st.sheet.view_top_left = CellRef::new(u32::from(rw_top).min(MAX_ROWS - 1), u32::from(col_left).min(MAX_COLS - 1));
        }
    } else if let Some((row, col)) = st.scrolled_to {
        st.sheet.view_top_left = CellRef::new(u32::from(row).min(MAX_ROWS - 1), u32::from(col).min(MAX_COLS - 1));
    }
    // Hyperlinks: each `HLink` covers a range; the tooltip record that follows names the same range.
    let (links, tooltips) = (std::mem::take(&mut st.links), std::mem::take(&mut st.tooltips));
    for mut link in links {
        if let Some((_, tip)) = tooltips.iter().find(|(r, _)| link.ranges.first() == Some(r)) {
            link.link.tooltip = Some(tip.clone());
        }
        for range in &link.ranges {
            for cell in range.iter() {
                if st.link_cells >= MAX_HYPERLINK_CELLS {
                    report.warn(format!("Hyperlinks past the first {MAX_HYPERLINK_CELLS} cells were dropped."));
                    break;
                }
                st.link_cells += 1;
                st.sheet.hyperlinks.insert(cell, link.link.clone());
            }
        }
    }
    if st.cf12_via_cfex {
        report.warn("A colour scale, data bar or icon set introduced by a CFEx record was skipped; the other conditional formats were read.");
    }
    // A chart's anchor lives in the sheet drawing, not in the chart substream. The drawing is one
    // tree split across `MSODRAWING` records, and the substreams are in the shapes' order.
    if !st.sheet.charts.is_empty() {
        let anchors = officeart::chart_anchors(&st.drawing, &st.sheet);
        for (chart, at) in st.sheet.charts.iter_mut().zip(&st.chart_anchor_index) {
            if let Some(anchor) = anchors.get(*at) {
                chart.anchor = *anchor;
            }
        }
    }
    // Pictures and autoshapes/text boxes are the drawing objects the model holds; vector pictures,
    // OLE objects and controls are skipped, counted once.
    if !st.drawing.is_empty() {
        let (images, vector) = officeart::read_images(&st.drawing, &st.g.blips, &st.sheet);
        st.sheet.images = images;
        let (shapes, unsupported) = officeart::read_shapes(&st.drawing, &st.sheet);
        for mut shape in shapes {
            if st.sheet.shapes.len() >= MAX_SHAPES {
                break;
            }
            shape.id = next_object_id(&st.sheet);
            st.sheet.shapes.push(shape);
        }
        if vector + unsupported > 0 {
            report.warn("Some drawings in an .xls file (vector pictures, OLE objects or controls) were not imported.");
        }
    }
    Ok(Some((st.sheet, st.calc)))
}

/// Where an OfficeArt container starts in a `Continue` tail left after a text box's text: the
/// record header's first two bytes are `0x000F` and the type is a container (`0xF00x`) or one of
/// `Sp`/`Fsp`/`Opt`/`ClientTextbox` (`0xF00A`–`0xF00D`).
fn officeart_start(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| {
        w.first() == Some(&0x0F) && w.get(1) == Some(&0) && w.get(3) == Some(&0xF0) && matches!(w.get(2), Some(0x00..=0x05 | 0x0A..=0x0D))
    })
}

/// An object id not already used on the sheet: pictures, charts and shapes share the space, and
/// the reader starts at 1.
fn next_object_id(sheet: &Sheet) -> u32 {
    sheet
        .charts
        .iter()
        .map(|c| c.id)
        .chain(sheet.images.iter().map(|i| i.id))
        .chain(sheet.shapes.iter().map(|s| s.id))
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

/// The shortest WINDOW2 a file holds (the chart-sheet form); a shorter one is truncated.
const WINDOW2_MIN_LEN: usize = 10;

/// A record reader's outcome, with a truncated or malformed record turned into a warning so the
/// rest of the file still opens. A budget error still stops the import.
fn soft<T>(report: &mut ReadReport, what: &str, r: Result<T, IoError>) -> Result<Option<T>, IoError> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(IoError::Format(_)) => {
            report.warn(format!("A {what} record was truncated or malformed and was skipped."));
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

fn sheet_record(rec: &Record<'_>, st: &mut SheetState<'_>, budget: &mut Budget, report: &mut ReadReport) -> Result<(), IoError> {
    let mut c = Cursor::new(&rec.parts);
    // A formula's string result is the String record right after it (shared/array records may sit between).
    if !matches!(rec.id, 0x0207 | 0x04BC | 0x0221 | 0x0236) {
        st.pending_string = None;
    }
    match rec.id {
        0x0203 => {
            let (r, col, x) = (c.u16()?, c.u16()?, c.u16()?);
            let v = Value::number(c.f64()?);
            st.put(budget, r, col, x, v)?;
        }
        0x027E => {
            let (r, col, x) = (c.u16()?, c.u16()?, c.u16()?);
            let v = rk(c.u32()?);
            st.put(budget, r, col, x, v)?;
        }
        0x00BD => {
            let r = c.u16()?;
            let first = c.u16()?;
            let n = c.remaining().saturating_sub(2) / 6;
            for i in 0..n {
                let x = c.u16()?;
                let v = rk(c.u32()?);
                let col = u16::try_from(usize::from(first) + i).unwrap_or(u16::MAX);
                st.put(budget, r, col, x, v)?;
            }
        }
        0x00FD => {
            let (r, col, x) = (c.u16()?, c.u16()?, c.u16()?);
            let i = c.u32()? as usize;
            match st.g.sst.get(i) {
                Some(s) => {
                    st.put(budget, r, col, x, Value::Text(s.clone()))?;
                }
                None => report.warn("Some cells refer to missing shared strings and were left empty."),
            }
        }
        0x0204 => {
            let (r, col, x) = (c.u16()?, c.u16()?, c.u16()?);
            let s = c.string()?;
            budget.text(s.len())?;
            st.put(budget, r, col, x, Value::from(s))?;
        }
        0x0205 => {
            let (r, col, x) = (c.u16()?, c.u16()?, c.u16()?);
            let (v, is_err) = (c.u8()?, c.u8()?);
            let v = if is_err != 0 { Value::Error(cell_error(v)) } else { Value::Bool(v != 0) };
            st.put(budget, r, col, x, v)?;
        }
        0x0201 => {
            let (r, col, x) = (c.u16()?, c.u16()?, c.u16()?);
            st.put(budget, r, col, x, Value::Empty)?;
        }
        0x00BE => {
            let r = c.u16()?;
            let first = c.u16()?;
            let n = c.remaining().saturating_sub(2) / 2;
            for i in 0..n {
                let x = c.u16()?;
                let col = u16::try_from(usize::from(first) + i).unwrap_or(u16::MAX);
                st.put(budget, r, col, x, Value::Empty)?;
            }
        }
        0x0006 => formula_cell(&mut c, st, budget)?,
        0x0207 => {
            if let Some((pos, style)) = st.pending_string.take() {
                let s = c.string()?;
                budget.text(s.len())?;
                if let Some(mut cell) = st.sheet.cells.get(pos) {
                    cell.value = Value::from(s);
                    st.sheet.cells.set(pos, cell);
                } else {
                    st.sheet.cells.set(pos, Cell { value: Value::from(s), formula: None, style });
                }
            }
        }
        0x04BC => {
            // RefU (the cells that may use it), reserved, cUse.
            c.skip(8)?;
            let f = parsed(&mut c)?;
            if let Some(at) = st.last_formula {
                st.shared.insert(at, f);
            }
        }
        0x0221 => {
            let (r0, r1) = (c.u16()?, c.u16()?);
            let (c0, c1) = (c.u8()?, c.u8()?);
            c.skip(6)?;
            let f = parsed(&mut c)?;
            let range = RangeRef::new(CellRef::new(u32::from(r0), u32::from(c0)), CellRef::new(u32::from(r1.max(r0)), u32::from(c1.max(c0))));
            if let Some(at) = st.last_formula {
                st.arrays.insert(at, (range, f));
            }
        }
        0x000D => {
            st.calc.mode = match c.u16()? as i16 {
                0 => CalcMode::Manual,
                -1 => CalcMode::AutomaticExceptTables,
                _ => CalcMode::Automatic,
            }
        }
        0x000C => st.calc.set_max_iterations(u32::from(c.u16()?)),
        0x0011 => st.calc.iterative = c.u16()? == 1,
        0x0010 => st.calc.set_max_change(c.f64()?),
        0x0236 => report.warn("What-if data tables are not supported; their values were kept."),
        0x00E5 => {
            let n = usize::from(c.u16()?);
            for _ in 0..n.min(c.remaining() / 8) {
                let (r0, r1, c0, c1) = (c.u16()?, c.u16()?, c.u16()?, c.u16()?);
                if r1 < r0 || c1 < c0 || u32::from(c1) >= 256 {
                    continue;
                }
                if st.sheet.merges.len() >= MAX_MERGES {
                    return Err(IoError::TooLarge("XLS: too many merged ranges".into()));
                }
                let range = RangeRef::new(CellRef::new(u32::from(r0), u32::from(c0)), CellRef::new(u32::from(r1), u32::from(c1)));
                if !range.is_single() {
                    st.sheet.merges.push(range);
                }
            }
        }
        0x007D => {
            let (first, last) = (c.u16()?, c.u16()?);
            let width = c.u16()?;
            let x = c.u16()?;
            let flags = c.u16()?;
            let last = last.min(255);
            if first > last {
                return Ok(());
            }
            let mut info = LineInfo::default();
            let px = col_width_to_px(f64::from(width) / 256.0);
            if flags & 0x0002 != 0 || px != st.sheet.default_col_width {
                info.size = Some(px);
            }
            info.hidden = flags & 0x0001 != 0;
            info.outline = ((flags >> 8) & 7) as u8;
            info.collapsed = flags & 0x1000 != 0;
            let style = st.style(x);
            if style != StyleId::DEFAULT {
                info.style = Some(style);
            }
            if info != LineInfo::default() {
                for col in first..=last {
                    st.sheet.cols.insert(u32::from(col), info);
                }
            }
        }
        0x0208 => {
            let r = c.u16()?;
            c.skip(4)?;
            let height = c.u16()?;
            c.skip(4)?;
            let flags = c.u32()?;
            let mut info = LineInfo { custom: flags & 0x40 != 0, hidden: flags & 0x20 != 0, ..LineInfo::default() };
            let px = pt_to_px(f64::from(height & 0x7FFF) / 20.0);
            if info.custom || (px - st.sheet.default_row_height).abs() > 0.5 {
                info.size = Some(px);
            }
            info.outline = (flags & 7) as u8;
            info.collapsed = flags & 0x10 != 0;
            if flags & 0x80 != 0 {
                let style = st.style(((flags >> 16) & 0x0FFF) as u16);
                if style != StyleId::DEFAULT {
                    info.style = Some(style);
                }
            }
            if info != LineInfo::default() && u32::from(r) < MAX_ROWS {
                st.sheet.rows.insert(u32::from(r), info);
            }
        }
        0x0225 => {
            let flags = c.u16()?;
            let height = c.u16()?;
            if flags & 0x0002 == 0 && height > 0 {
                st.sheet.default_row_height = pt_to_px(f64::from(height) / 20.0);
            }
        }
        0x023E => {
            let flags = if c.remaining() >= WINDOW2_MIN_LEN { c.u16().ok() } else { None };
            let Some(flags) = flags else { return Ok(()) };
            let on = |bit: u16| flags & bit != 0;
            st.sheet.show_formulas = on(0x0001);
            st.sheet.show_gridlines = on(0x0002);
            st.sheet.show_headings = on(0x0004);
            st.sheet.show_zeros = on(0x0010);
            st.sheet.right_to_left = on(0x0040);
            // `fFrozenRt` (bit 3) says the `Pane` record's split is frozen; `rwTop` and `colLeft`
            // follow the flags and hold the first visible row and column.
            st.frozen = on(0x0008);
            if c.remaining() >= 4
                && let (Ok(row), Ok(col)) = (c.u16(), c.u16())
            {
                st.scrolled_to = Some((row, col));
            }
        }
        0x001C => {
            // NOTE ([MS-XLS] 2.4.176): row, column, XF, flags, then the author as an
            // XLUnicodeString. The comment's text follows in the drawing object's TXO record.
            if st.notes.len() >= MAX_COMMENTS {
                return Err(IoError::TooLarge("XLS: too many comments".into()));
            }
            let r = c.u16()?;
            let col = c.u16()?;
            let _ixfe = c.u16()?;
            let _flags = c.u16()?;
            let author = c.string()?;
            if let Some(at) = st.pos(r, col) {
                st.notes.push((at, author));
            }
        }
        0x01B6 => {
            // TXO ([MS-XLS] 2.4.344): the drawing object's text. The fixed part ends with
            // cchText and cbRuns; the characters follow it, starting with their own compression
            // flag, and continue into Continue records (which Records already gathered).
            c.skip(10)?;
            let cch = usize::from(c.u16()?);
            let _runs = c.u16()?;
            c.skip(4)?;
            let high = c.u8()? & 1 != 0;
            let text = c.chars(cch, high)?;
            if st.note_texts.len() < MAX_COMMENTS {
                st.note_texts.push(text);
            }
            // Excel can carry the last shape's drawing in the Continue records that follow a
            // text box's text (after the text's formatting data): an OfficeArt container header
            // marks where it starts.
            let rest = c.rest();
            if let Some(at) = officeart_start(&rest) {
                st.drawing.extend_from_slice(rest.get(at..).unwrap_or_default());
            }
        }
        // The drawings: an OfficeArt tree whose records may be split across several records;
        // concatenated, they make the one tree. The blip store is the workbook's.
        0x00EC => st.drawing.extend(c.rest()),
        0x01B0 => {
            // CondFmt ([MS-XLS] 2.4.56): the ranges and the id its CF records are stored under.
            if let Some(group) = soft(report, "conditional format", condfmt::condfmt(&mut c))? {
                st.cf_left = group.count;
                st.cf_ids.insert(group.id, vec![]);
                st.cf_group = Some(group);
            }
        }
        0x01B1 => {
            // CF (2.4.42): one cell-value comparison or one formula.
            let Some(group) = st.cf_group.as_ref() else { return Ok(()) };
            if group.ranges.is_empty() || st.cf_left == 0 || st.sheet.cond_formats.len() >= MAX_CONDFMTS {
                return Ok(());
            }
            let base = group.ranges.first().map_or_else(CellRef::default, |r| r.start);
            let (ranges, id) = (group.ranges.clone(), group.id);
            st.cf_left -= 1;
            let rule = soft(report, "conditional format", condfmt::cf(&mut c, base, &st.g.links, &st.g.styles.palette, &st.g.styles.formats))?;
            match rule {
                Some(Some(rule)) => {
                    st.cf_seq += 1;
                    let at = st.sheet.cond_formats.len();
                    st.sheet.cond_formats.push(CondFormat { ranges, rule, priority: st.cf_seq, stop_if_true: false });
                    if let Some(ids) = st.cf_ids.get_mut(&id) {
                        ids.push(at);
                    }
                }
                Some(None) => report.warn("A conditional format whose condition could not be read was skipped."),
                None => {}
            }
        }
        0x087B => {
            let ext = soft(report, "conditional format extension", condfmt::cfex(&mut c, &st.g.styles.palette, &st.g.styles.formats))?;
            if let Some(ext) = ext {
                if ext.cf12 {
                    st.cf12_via_cfex = true;
                } else {
                    let at = st.cf_ids.get(&ext.id).and_then(|v| v.get(ext.icf)).copied();
                    if let Some(cf) = at.and_then(|i| st.sheet.cond_formats.get_mut(i)) {
                        cf.priority = ext.priority;
                        cf.stop_if_true = ext.stop_if_true;
                        if let Some(style) = ext.style {
                            match &mut cf.rule {
                                CfRule::CellIs { style: s, .. } | CfRule::Expression { style: s, .. } => **s = style,
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
        // CondFmt12 / CF12: colour scales, data bars and icon sets.
        0x0879 => {
            if let Some(group) = soft(report, "conditional format", condfmt::condfmt12(&mut c))? {
                st.cf12_left = group.count;
                st.cf12_group = Some(group);
            }
        }
        0x087A => {
            let Some(group) = st.cf12_group.as_ref() else { return Ok(()) };
            if group.ranges.is_empty() || st.cf12_left == 0 || st.sheet.cond_formats.len() >= MAX_CONDFMTS {
                return Ok(());
            }
            let base = group.ranges.first().map_or_else(CellRef::default, |r| r.start);
            let (ranges, count) = (group.ranges.clone(), st.cf12_left);
            st.cf12_left = count - 1;
            let rule = soft(report, "conditional format", condfmt::cf12(&mut c, base, &st.g.links, &st.g.styles.palette))?;
            match rule {
                Some(Some(rule)) => {
                    st.sheet.cond_formats.push(CondFormat { ranges, rule: rule.rule, priority: rule.priority, stop_if_true: rule.stop_if_true })
                }
                Some(None) => report.warn("A conditional format whose rule type could not be read was skipped."),
                None => {}
            }
        }
        0x01B2 => {
            if let Some(count) = soft(report, "data validation count", validation::dval(&mut c))? {
                if count > validation::MAX_DV_COUNT {
                    report.warn("A sheet's data validation count was larger than Excel allows; the rules present were read.");
                }
                st.dv_expected = Some(count.min(validation::MAX_DV_COUNT));
            }
        }
        0x01BE => {
            st.dv_seen += 1;
            if st.dv_expected.is_some_and(|n| st.dv_seen > n) {
                if !st.dv_overflow {
                    st.dv_overflow = true;
                    report.warn("A sheet had more data validation rules than its DVal record announced; the extra ones were skipped.");
                }
                return Ok(());
            }
            let rule = soft(report, "data validation", validation::dv(&mut c, &st.g.links, budget))?;
            if let Some(Some(v)) = rule
                && st.sheet.validations.len() < validation::MAX_DV_COUNT
            {
                st.sheet.validations.push(v);
            }
        }
        0x01B8 => {
            if let Some(Some(link)) = soft(report, "hyperlink", hyperlink::hlink(&mut c))? {
                st.links.push(link);
            }
        }
        0x0800 => {
            if let Some(Some(t)) = soft(report, "hyperlink tooltip", hyperlink::tooltip(&mut c))? {
                st.tooltips.push(t);
            }
        }
        0x0041 => {
            // Pane (2.4.189): the split, the bottom-right pane's first cell, the active pane.
            if c.remaining() < 10 {
                report.warn("A pane record was truncated or malformed and was skipped.");
            } else {
                st.pane = Some((c.u16()?, c.u16()?, c.u16()?, c.u16()?));
            }
        }
        0x00B0 => report.warn("PivotTables in .xls files are not imported yet; their values were kept."),
        _ => {}
    }
    Ok(())
}

fn formula_cell(c: &mut Cursor<'_, '_>, st: &mut SheetState<'_>, budget: &mut Budget) -> Result<(), IoError> {
    let (r, col, x) = (c.u16()?, c.u16()?, c.u16()?);
    let mut val = [0u8; 8];
    for b in val.iter_mut() {
        *b = c.u8()?;
    }
    let _flags = c.u16()?;
    c.skip(4)?;
    let f = parsed(c)?;
    let [b0, _, b2, _, _, _, e0, e1] = val;
    let mut string_result = false;
    let value = if [e0, e1] == [0xFF, 0xFF] {
        match b0 {
            0x00 => {
                string_result = true;
                Value::Empty
            }
            0x01 => Value::Bool(b2 != 0),
            0x02 => Value::Error(cell_error(b2)),
            0x03 => Value::from(""),
            _ => Value::Empty,
        }
    } else {
        Value::number(f64::from_le_bytes(val))
    };
    let Some(pos) = st.pos(r, col) else { return Ok(()) };
    budget.cell()?;
    let style = st.style(x);
    st.sheet.cells.set(pos, Cell { value, formula: None, style });
    st.last_formula = Some(pos);
    if string_result {
        st.pending_string = Some((pos, style));
    }
    // PtgExp: part of a shared or array formula anchored at another cell.
    if f.rgce.len() == 5 && f.rgce.first() == Some(&0x01) {
        let get = |i: usize| f.rgce.get(i).copied().unwrap_or(0);
        let anchor = CellRef::new(u32::from(u16::from_le_bytes([get(1), get(2)])), u32::from(u16::from_le_bytes([get(3), get(4)])));
        st.pending.push(Pending { pos, anchor });
        return Ok(());
    }
    st.attach(pos, &f, Mode::Cell, None);
    Ok(())
}

fn defined_names(g: &Globals, model_index: &[Option<usize>], wb: &mut Workbook, report: &mut ReadReport) {
    let scope_of = |itab: u16| -> Option<Option<usize>> {
        if itab == 0 {
            return Some(None);
        }
        model_index.get(usize::from(itab) - 1).copied().flatten().map(Some)
    };
    for lbl in &g.names {
        let Some(scope) = scope_of(lbl.itab) else { continue };
        let text = match decode(&lbl.rgce, &lbl.rgcb, CellRef::new(0, 0), Mode::Name, &g.links) {
            Ok(e) => gridcraft_formula::print(&e),
            Err(_) if lbl.rgce.is_empty() => continue,
            Err(Unsupported(why)) => {
                report.warn(format!("Some defined names use {why} and were skipped."));
                continue;
            }
        };
        match lbl.builtin {
            Some(6 | 7) => {
                let Some(si) = scope else { continue };
                let Some(sheet) = wb.sheet_mut(si) else { continue };
                if lbl.builtin == Some(6) {
                    sheet.print.print_area = crate::read::parse_print_area(&text);
                } else {
                    let (rows, cols) = crate::read::parse_print_titles(&text);
                    sheet.print.title_rows = rows;
                    sheet.print.title_cols = cols;
                }
            }
            Some(_) => {}
            None if lbl.macro_name => report.warn("Macro names were skipped."),
            None => wb.names.push(DefinedName { name: lbl.name.clone(), scope, formula: text, comment: String::new(), hidden: lbl.hidden }),
        }
    }
}
