//! Reading worksheet parts.

use std::collections::HashMap;
use std::sync::Arc;

use gridcraft_core::date::{serial_from_ymd, time_fraction};
use gridcraft_core::{CellError, CellRef, DateSystem, MAX_COLS, MAX_ROWS, RangeRef, Value};
use gridcraft_formula::Expr;
use gridcraft_model::{
    AutoFilter, Cell, CfOperator, CfRule, CfValueKind, Comment, CondFormat, EditRange, ErrorStyle, FilterCriterion, Formula, Hyperlink, LineInfo,
    NoteBox, NoteCorner, Orientation, PasswordHash, Sheet, SheetProtection, Sparkline, SparklineKind, Table, TableColumn, TotalsFn, Validation,
    ValidationKind, XmlColumnPr,
};

use crate::IoError;
use crate::fmla::from_file;
use crate::package::Rel;
use crate::read::Ctx;
use crate::styles::read_color;
use crate::tables::{col_width_to_px, paper_name, pt_to_px};
use crate::xml::{self, El};

struct Shared {
    anchor: CellRef,
    expr: Option<Expr>,
    text: String,
}

#[derive(Default)]
struct RowState {
    next_row: u32,
    shared: HashMap<u32, Shared>,
    /// Dynamic-array spill ranges and legacy array ranges: cached values inside them (other than
    /// the anchor) are not stored, so the formula can fill them again.
    dynamic: Vec<RangeRef>,
    dropped: u64,
    rows: u64,
    /// The row being read (0-based) and the next column when a `<c>` has no `r`.
    row: u32,
    next_col: u32,
    /// The current `<row>` was outside the grid, so its cells are dropped.
    skip_row: bool,
    /// The cells of the row being read, handed to the store in one go and reused for the next row.
    row_cells: Vec<(u32, Cell)>,
}

/// The read-only half of [`Ctx`] that row decoding needs: shared strings, cell formats, value
/// metadata and the date system. Nothing here mutates, so row chunks can share one — that is what
/// makes the parallel read below possible. Warnings are collected by the caller instead, in
/// document order.
pub(crate) struct DecodeCtx<'a> {
    sst: &'a [Arc<str>],
    xf_map: &'a [gridcraft_model::StyleId],
    pictures: &'a [Option<Arc<gridcraft_model::CellPicture>>],
    date_system: DateSystem,
}

impl DecodeCtx<'_> {
    fn style(&self, xf: Option<u32>) -> gridcraft_model::StyleId {
        xf.and_then(|i| self.xf_map.get(i as usize).copied()).unwrap_or_default()
    }
}

const MAX_HYPERLINK_CELLS: u64 = 10_000;
const MAX_COLUMN_ASSIGNMENTS: u64 = 1_000_000;

pub fn read_sheet(cx: &mut Ctx<'_>, part: &str, name: &str) -> Result<Sheet, IoError> {
    let bytes = cx.pkg.read(part)?.ok_or_else(|| IoError::Format(format!("missing part {part}")))?;
    let rels = cx.pkg.rels(part)?;
    let mut sheet = Sheet::new(name);
    let (root, warn, rows_read, dropped) = {
        let dc = DecodeCtx { sst: &cx.sst, xf_map: &cx.xf_map, pictures: &cx.cell_pictures, date_system: cx.date_system };
        let default_height = sheet.default_row_height;
        read_rows(&bytes, &dc, &mut sheet, default_height, name)
    };
    for w in warn {
        cx.warn(w);
    }
    if dropped > 0 {
        cx.warn(format!("sheet \"{name}\": {dropped} cells or rows outside the 1,048,576 × 16,384 grid were dropped"));
    }
    let _ = rows_read;
    drop(bytes);

    let mut fit_to_page = false;
    let mut column_assignments_left = MAX_COLUMN_ASSIGNMENTS;
    cx.cf_ext_ids.clear();
    for e in &root.children {
        match e.name.as_str() {
            "sheetPr" => {
                if let Some(t) = e.child("tabColor") {
                    sheet.tab_color = Some(read_color(t, &cx.palette));
                }
                fit_to_page = e.child("pageSetUpPr").is_some_and(|p| p.flag("fitToPage", false));
            }
            "sheetViews" => read_views(e, &mut sheet),
            "sheetFormatPr" => {
                if let Some(h) = e.attr_f64("defaultRowHeight") {
                    let px = pt_to_px(h);
                    if px > 0.0 {
                        sheet.default_row_height = px;
                    }
                }
                if let Some(w) = e.attr_f64("defaultColWidth") {
                    let px = col_width_to_px(w);
                    if px > 0.0 {
                        sheet.default_col_width = px;
                    }
                } else if let Some(b) = e.attr_f64("baseColWidth") {
                    let px = ((b.clamp(0.0, 255.0) * 7.0 + 5.0) / 8.0).ceil() * 8.0;
                    sheet.default_col_width = px as f32;
                }
            }
            "cols" => read_cols(cx, e, &mut sheet, &mut column_assignments_left)?,
            "mergeCells" => {
                for m in e.kids("mergeCell") {
                    if let Some(r) = m.attr("ref").and_then(RangeRef::parse)
                        && !r.is_single()
                    {
                        sheet.merges.push(r);
                    }
                }
            }
            "conditionalFormatting" => read_cf(cx, e, &mut sheet),
            "dataValidations" => {
                for v in e.kids("dataValidation") {
                    if let Some(v) = read_validation(v) {
                        sheet.validations.push(v);
                    }
                }
            }
            "hyperlinks" => read_hyperlinks(cx, name, e, &rels, &mut sheet),
            "autoFilter" => sheet.autofilter = read_autofilter(cx, e),
            "printOptions" => {
                sheet.print.gridlines = e.flag("gridLines", false);
                sheet.print.headings = e.flag("headings", false);
                sheet.print.center_h = e.flag("horizontalCentered", false);
                sheet.print.center_v = e.flag("verticalCentered", false);
            }
            "pageMargins" => {
                let m = &mut sheet.print.margins;
                for (i, k) in ["left", "right", "top", "bottom", "header", "footer"].iter().enumerate() {
                    if let (Some(v), Some(slot)) = (e.attr_f64(k), m.get_mut(i)) {
                        *slot = v.clamp(0.0, 50.0) as f32;
                    }
                }
            }
            "pageSetup" => {
                if e.attr("orientation") == Some("landscape") {
                    sheet.print.orientation = Orientation::Landscape;
                }
                if let Some(p) = e.attr_u32("paperSize").and_then(paper_name) {
                    sheet.print.paper = p.to_string();
                }
                if let Some(s) = e.attr_u32("scale") {
                    sheet.print.scale = s.clamp(10, 400) as u16;
                }
                if fit_to_page || root.child("sheetPr").and_then(|p| p.child("pageSetUpPr")).is_some_and(|p| p.flag("fitToPage", false)) {
                    let w = e.attr_u32("fitToWidth").unwrap_or(1).min(32767) as u16;
                    let h = e.attr_u32("fitToHeight").unwrap_or(1).min(32767) as u16;
                    sheet.print.fit_to = Some((w, h));
                }
            }
            "headerFooter" => {
                let pr = &mut sheet.print;
                for (tag, slot) in [
                    ("oddHeader", &mut pr.header),
                    ("oddFooter", &mut pr.footer),
                    ("evenHeader", &mut pr.even_header),
                    ("evenFooter", &mut pr.even_footer),
                    ("firstHeader", &mut pr.first_header),
                    ("firstFooter", &mut pr.first_footer),
                ] {
                    // Excel caps each at 255 characters; a hostile file can't make them huge.
                    *slot = e.child(tag).map(|h| h.text.chars().take(255).collect()).unwrap_or_default();
                }
                pr.different_first = e.flag("differentFirst", false);
                pr.different_odd_even = e.flag("differentOddEven", false);
                pr.hf_scale_with_doc = e.flag("scaleWithDoc", true);
                pr.hf_align_with_margins = e.flag("alignWithMargins", true);
            }
            "rowBreaks" => sheet.print.row_breaks = e.kids("brk").filter_map(|b| b.attr_u32("id")).filter(|&r| r > 0 && r < MAX_ROWS).collect(),
            "colBreaks" => sheet.print.col_breaks = e.kids("brk").filter_map(|b| b.attr_u32("id")).filter(|&c| c > 0 && c < MAX_COLS).collect(),
            "sheetProtection" if e.flag("sheet", false) => {
                sheet.protection = Some(SheetProtection {
                    password: read_password(e, ""),
                    legacy_password_hash: None,
                    select_locked: !e.flag("selectLockedCells", false),
                    select_unlocked: !e.flag("selectUnlockedCells", false),
                    format_cells: !e.flag("formatCells", true),
                    format_columns: !e.flag("formatColumns", true),
                    format_rows: !e.flag("formatRows", true),
                    insert_columns: !e.flag("insertColumns", true),
                    insert_rows: !e.flag("insertRows", true),
                    delete_columns: !e.flag("deleteColumns", true),
                    delete_rows: !e.flag("deleteRows", true),
                    sort: !e.flag("sort", true),
                    autofilter: !e.flag("autoFilter", true),
                });
            }
            "drawing" => {
                if let Some(rel) = e.attr("id").and_then(|id| rels.iter().find(|r| r.id == id)) {
                    let target = rel.target.clone();
                    crate::drawing::read_drawing(cx, &target, &mut sheet)?;
                }
            }
            "tableParts" => {
                for tp in e.kids("tablePart") {
                    if let Some(rel) = tp.attr("id").and_then(|id| rels.iter().find(|r| r.id == id)) {
                        let target = rel.target.clone();
                        if let Some(x) = cx.optional_xml(&target)?
                            && let Some(mut t) = read_table(cx, &x)
                        {
                            t.id = cx.next_object_id();
                            sheet.tables.push(t);
                        }
                    }
                }
            }
            "extLst" => read_ext(cx, e, &mut sheet),
            "protectedRanges" => read_protected_ranges(e, &mut sheet),
            _ => {}
        }
    }
    // Comments are related to the sheet without an element in it.
    for rel in rels.iter().filter(|r| r.kind == "comments") {
        if let Some(x) = cx.optional_xml(&rel.target)? {
            read_comments(&x, &mut sheet);
        }
    }
    for rel in rels.iter().filter(|r| r.kind == "threadedComment") {
        if let Some(x) = cx.optional_xml(&rel.target)? {
            read_threaded(cx, &x, &mut sheet);
        }
    }
    // Which notes are displayed (`<x:Visible/>`) and where their boxes sit live in the VML shapes.
    // Best effort: the VML is only read for those, so a part that doesn't parse is skipped without
    // a warning.
    if !sheet.comments.is_empty() {
        for rel in rels.iter().filter(|r| r.kind == "vmlDrawing") {
            if let Ok(Some(x)) = cx.pkg.read_xml(&rel.target) {
                read_note_shapes(&x, &mut sheet, 0);
            }
        }
    }
    Ok(sheet)
}

/// A protection password hash: the iterated attributes (`algorithmName`, `hashValue`,
/// `saltValue`, `spinCount`) or the legacy 16-bit `password`, each name preceded by `prefix`
/// (`workbook` gives `workbookHashValue`…). One that can't be read keeps a hash that never
/// matches, so what it protects stays locked rather than open.
pub(crate) fn read_password(e: &El, prefix: &str) -> Option<PasswordHash> {
    let attr = |base: &str| e.attr(&password_attr(prefix, base));
    if attr("algorithmName").is_some() || attr("hashValue").is_some() {
        return Some(PasswordHash::Iterated {
            algorithm: attr("algorithmName").unwrap_or("").to_string(),
            hash: attr("hashValue").unwrap_or("").to_string(),
            salt: attr("saltValue").unwrap_or("").to_string(),
            spin_count: e.attr_u32(&password_attr(prefix, "spinCount")).unwrap_or(0),
        });
    }
    attr("password").map(|h| match u16::from_str_radix(h.trim(), 16) {
        Ok(h) => PasswordHash::Legacy(h),
        Err(_) => PasswordHash::Iterated { algorithm: String::new(), hash: String::new(), salt: String::new(), spin_count: 0 },
    })
}

/// `base` preceded by `prefix` in camel case: ("workbook", "hashValue") → `workbookHashValue`.
pub(crate) fn password_attr(prefix: &str, base: &str) -> String {
    let mut chars = base.chars();
    match chars.next() {
        Some(c) if !prefix.is_empty() => format!("{prefix}{}{}", c.to_ascii_uppercase(), chars.as_str()),
        _ => base.to_string(),
    }
}

/// Allow Edit Ranges. A range whose password can't be read keeps a hash that never matches
/// (or an unsupported algorithm), so it stays locked rather than open.
fn read_protected_ranges(e: &El, sheet: &mut Sheet) {
    for p in e.kids("protectedRange") {
        let ranges: Vec<RangeRef> = p.attr("sqref").unwrap_or("").split_whitespace().filter_map(RangeRef::parse).collect();
        if ranges.is_empty() {
            continue;
        }
        let password = read_password(p, "");
        let title = p.attr("name").unwrap_or("").to_string();
        sheet.edit_ranges.push(EditRange { title, ranges, password, security_descriptor: p.attr("securityDescriptor").map(str::to_string) });
    }
}

fn read_views(e: &El, sheet: &mut Sheet) {
    let Some(v) = e.child("sheetView") else { return };
    sheet.show_gridlines = v.flag("showGridLines", true);
    sheet.show_headings = v.flag("showRowColHeaders", true);
    sheet.show_formulas = v.flag("showFormulas", false);
    sheet.show_zeros = v.flag("showZeros", true);
    sheet.right_to_left = v.flag("rightToLeft", false);
    sheet.view = v.attr("view").and_then(gridcraft_model::SheetViewMode::parse).unwrap_or_default();
    if let Some(z) = v.attr_u32("zoomScale") {
        sheet.zoom = z.clamp(10, 400) as u16;
    }
    if let Some(c) = v.attr("topLeftCell").and_then(CellRef::parse) {
        sheet.view_top_left = c;
    }
    let mut active_pane = None;
    if let Some(p) = v.child("pane") {
        if matches!(p.attr("state"), Some("frozen") | Some("frozenSplit")) {
            let cols = p.attr_f64("xSplit").unwrap_or(0.0).clamp(0.0, (MAX_COLS - 1) as f64) as u32;
            let rows = p.attr_f64("ySplit").unwrap_or(0.0).clamp(0.0, (MAX_ROWS - 1) as f64) as u32;
            if rows > 0 || cols > 0 {
                sheet.freeze = Some((rows, cols));
                // With frozen panes the scroll position is the scrolling pane's first cell.
                if let Some(c) = p.attr("topLeftCell").and_then(CellRef::parse) {
                    sheet.view_top_left = c;
                }
            }
        }
        active_pane = p.attr("activePane");
    }
    let sel = v.kids("selection").find(|s| s.attr("pane") == active_pane).or_else(|| v.kids("selection").last());
    if let Some(c) = sel.and_then(|s| s.attr("activeCell")).and_then(CellRef::parse) {
        sheet.view_active = c;
    }
}

fn read_cols(cx: &Ctx<'_>, e: &El, sheet: &mut Sheet, remaining: &mut u64) -> Result<(), IoError> {
    for c in e.kids("col") {
        let (Some(min), Some(max)) = (c.attr_u32("min"), c.attr_u32("max")) else { continue };
        if min == 0 || min > MAX_COLS || max < min {
            continue;
        }
        let max = max.min(MAX_COLS);
        let mut info = LineInfo::default();
        // A width without `customWidth` that equals the default is just the default.
        if let Some(w) = c.attr_f64("width") {
            let px = col_width_to_px(w);
            if c.flag("customWidth", false) || px != sheet.default_col_width {
                info.size = Some(px);
            }
        }
        info.hidden = c.flag("hidden", false);
        info.outline = c.attr_u32("outlineLevel").unwrap_or(0).min(7) as u8;
        info.collapsed = c.flag("collapsed", false);
        if let Some(s) = c.attr_u32("style") {
            let id = cx.style(Some(s));
            if id != gridcraft_model::StyleId::DEFAULT {
                info.style = Some(id);
            }
        }
        // A custom width equal to the default still pins the column (Standard Width leaves it alone).
        if info == LineInfo::default() {
            continue;
        }
        // Overlapping declarations repeat work even when the final map stays small.
        // Charge the clipped span before expansion, sharing the budget across <cols> blocks.
        *remaining = remaining
            .checked_sub(u64::from(max - min + 1))
            .ok_or_else(|| IoError::TooLarge(format!("sheet \"{}\": more than {MAX_COLUMN_ASSIGNMENTS} expanded column assignments", sheet.name)))?;
        for col in (min - 1)..max {
            sheet.cols.insert(col, info);
        }
    }
    Ok(())
}

/// Reads the rows of a worksheet part into `sheet`, returning the rest of the document (views,
/// columns, merges, tables…), the warnings in document order and the two counters.
///
/// A large part whose rows carry no cross-row state (no shared or array formulas) is cut at row
/// boundaries and decoded on several threads; the rows are then applied in document order, so the
/// model sees exactly the sequence a single pass gives it.
fn read_rows(bytes: &[u8], dc: &DecodeCtx<'_>, sheet: &mut Sheet, default_height: f32, name: &str) -> (El, Vec<String>, u64, u64) {
    let mut warn = Vec::new();
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(ranges) = plan_chunks(bytes) {
        let threads = threads();
        let last = ranges.last().map(|r| r.1).unwrap_or(0);
        let mut root = El::default();
        let mut first = true;
        let mut rows_read = 0u64;
        let mut dropped = 0u64;
        for wave in ranges.chunks(threads) {
            let decoded: Vec<Chunk> = std::thread::scope(|scope| {
                let handles: Vec<_> = wave
                    .iter()
                    .map(|&(a, b)| {
                        let wrapped = b == last && a != 0;
                        scope.spawn(move || decode_chunk(&bytes[a..b], dc, default_height, wrapped))
                    })
                    .collect();
                handles.into_iter().map(|h| h.join()).filter_map(Result::ok).collect()
            });
            for chunk in decoded {
                rows_read += chunk.rows_read;
                dropped += chunk.dropped;
                warn.extend(chunk.warn);
                if first {
                    root = chunk.root;
                    first = false;
                } else {
                    root.children.extend(chunk.root.children);
                }
                apply_chunk(sheet, chunk.rows);
            }
        }
        return (root, warn, rows_read, dropped);
    }

    let mut st = RowState::default();
    let mut damaged = None;
    let root = match crate::rows::parse(bytes, crate::rows::Mode::Document, &mut |item| {
        match item {
            crate::rows::Item::RowStart(a) => row_start(dc, sheet, &mut st, default_height, &a),
            crate::rows::Item::Cell(c) => cell(dc, sheet, &mut st, &c, &mut warn),
            crate::rows::Item::RowEnd => row_end(sheet, &mut st),
        }
        Ok(())
    }) {
        Ok(r) => r,
        Err(e) => {
            damaged = Some(e);
            El::default()
        }
    };
    if let Some(e) = damaged {
        warn.push(format!("sheet \"{name}\" is damaged ({e}); kept {} rows", st.rows));
    }
    (root, warn, st.rows, st.dropped)
}

/// One cell after decoding: either a value/formula cell, or a picture with its style (a picture
/// cell stores no value — the picture is held beside the cells).
struct DecodedCell {
    at: CellRef,
    style: gridcraft_model::StyleId,
    cell: Option<Cell>,
    picture: Option<Arc<gridcraft_model::CellPicture>>,
}

/// Rows decoded off one slice of the part, waiting to be applied in order.
struct Chunk {
    root: El,
    rows: Vec<(u32, Option<LineInfo>, Vec<DecodedCell>)>,
    rows_read: u64,
    dropped: u64,
    warn: Vec<String>,
}

/// How many threads to read rows on: the machine's parallelism, capped so a big sheet does not
/// spawn more workers than it can keep fed.
#[cfg(not(target_arch = "wasm32"))]
fn threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 8)
}

/// Where a part may be cut so that every piece starts at a row that carries its own `r`. `None`
/// means "read it in one pass": too small, not UTF-8, or carrying the two kinds of formula that
/// make rows depend on each other (shared masters and array ranges).
#[cfg(not(target_arch = "wasm32"))]
fn plan_chunks(bytes: &[u8]) -> Option<Vec<(usize, usize)>> {
    const MIN_BYTES: usize = 8 * 1024 * 1024;
    if bytes.len() < MIN_BYTES || threads() < 2 {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    if text.contains("t=\"shared\"") || text.contains("t=\"array\"") {
        return None;
    }
    let close = if text.contains("</row>") { "</row>" } else { "</x:row>" };
    let open = if close == "</row>" { "<row " } else { "<x:row " };
    let parts = (threads() * 3).max(2);
    let mut cuts = Vec::with_capacity(parts + 1);
    let mut at = 0usize;
    for i in 1..parts {
        let want = text.len() / parts * i;
        if want <= at {
            continue;
        }
        let tail = text.get(want..)?;
        let rel = tail.find(close)?;
        let end = want + rel + close.len();
        let rest = text.get(end..)?.trim_start();
        if !rest.starts_with(open) {
            break;
        }
        let tag = rest.get(..rest.find('>')?)?;
        if !tag.contains(" r=\"") {
            return None;
        }
        let next = text.len() - rest.len();
        if next <= at {
            continue;
        }
        cuts.push(at);
        at = next;
    }
    if cuts.is_empty() {
        return None;
    }
    cuts.push(at);
    cuts.push(text.len());
    Some(cuts.windows(2).map(|w| (w[0], w[1])).collect())
}

fn decode_chunk(bytes: &[u8], dc: &DecodeCtx<'_>, default_height: f32, wrapped: bool) -> Chunk {
    let mut chunk = Chunk { root: El::default(), rows: Vec::new(), rows_read: 0, dropped: 0, warn: Vec::new() };
    let mut st = RowState::default();
    let mut row: Option<(u32, Option<LineInfo>, Vec<DecodedCell>)> = None;
    let mut next_row = 0u32;
    let mut take = |chunk: &mut Chunk, item: crate::rows::Item<'_>| match item {
        crate::rows::Item::RowStart(a) => {
            chunk.rows_read += 1;
            if let Some((r, info)) = row_fields(dc, default_height, &a, next_row) {
                next_row = r + 1;
                row = Some((r, (info != LineInfo::default()).then_some(info), Vec::new()));
            } else {
                chunk.dropped += 1;
            }
        }
        crate::rows::Item::Cell(c) => {
            if let Some((_, _, cells)) = row.as_mut()
                && let Some(pos) = cell_pos(&mut st, &c)
                && let Some(cell) = decode_cell(dc, &mut chunk.warn, &mut st, &c, pos)
            {
                cells.push(cell);
            }
        }
        crate::rows::Item::RowEnd => {
            if let Some(done) = row.take() {
                chunk.rows.push(done);
            }
        }
    };
    let parsed = if wrapped {
        let mut buf = Vec::with_capacity(bytes.len() + 32);
        buf.extend_from_slice(b"<worksheet><sheetData>");
        buf.extend_from_slice(bytes);
        crate::rows::parse(&buf, crate::rows::Mode::Document, &mut |it| {
            take(&mut chunk, it);
            Ok(())
        })
    } else {
        crate::rows::parse(bytes, crate::rows::Mode::Rows, &mut |it| {
            take(&mut chunk, it);
            Ok(())
        })
    };
    if let Some(done) = row.take() {
        chunk.rows.push(done);
    }
    if let Ok(root) = parsed {
        chunk.root = root;
    }
    chunk.dropped += st.dropped;
    chunk
}

fn apply_chunk(sheet: &mut Sheet, rows: Vec<(u32, Option<LineInfo>, Vec<DecodedCell>)>) {
    for (r, info, cells) in rows {
        if let Some(info) = info {
            sheet.rows.insert(r, info);
        }
        let mut batch: Vec<(u32, Cell)> = Vec::with_capacity(cells.len());
        for d in cells {
            if let Some(picture) = d.picture {
                sheet.set_picture(d.at, picture);
                sheet.set_style(d.at, d.style);
                continue;
            }
            if let Some(cell) = d.cell {
                // The cell's own reference wins over the row it sits in.
                if d.at.row == r {
                    batch.push((d.at.col, cell));
                } else {
                    sheet.cells.set(d.at, cell);
                }
            }
        }
        sheet.set_row_cells(r, &mut batch);
    }
}

/// The row number and line settings of a `<row>`, or `None` when it falls outside the grid.
fn row_fields(dc: &DecodeCtx<'_>, default_height: f32, row: &crate::rows::RowAttrs<'_>, fallback: u32) -> Option<(u32, LineInfo)> {
    let r = match row.r.and_then(|v| v.trim().parse::<u32>().ok()) {
        Some(r) if r >= 1 => r - 1,
        Some(_) => return None,
        // A row without `r` continues from the row before it.
        None => fallback,
    };
    if r >= MAX_ROWS {
        return None;
    }
    let mut info = LineInfo::default();
    if let Some(ht) = row.ht.and_then(|v| v.trim().parse::<f64>().ok()) {
        info.custom = row.custom_height;
        // Automatic heights equal to the default are noise.
        if info.custom || (pt_to_px(ht) - default_height).abs() > 0.5 {
            info.size = Some(pt_to_px(ht));
        }
    }
    info.hidden = row.hidden;
    info.outline = row.outline.and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(0).min(7) as u8;
    info.collapsed = row.collapsed;
    if row.custom_format
        && let Some(s) = row.s.and_then(|v| v.trim().parse::<u32>().ok())
    {
        info.style = Some(dc.style(Some(s)));
    }
    Some((r, info))
}

fn row_start(dc: &DecodeCtx<'_>, sheet: &mut Sheet, st: &mut RowState, default_height: f32, row: &crate::rows::RowAttrs<'_>) {
    st.rows += 1;
    st.skip_row = false;
    st.row_cells.clear();
    match row_fields(dc, default_height, row, st.next_row) {
        Some((r, info)) => {
            st.row = r;
            st.next_col = 0;
            st.next_row = r + 1;
            if info != LineInfo::default() {
                sheet.rows.insert(r, info);
            }
        }
        None => {
            st.dropped += 1;
            st.skip_row = true;
        }
    }
}

/// The position of a `<c>`, advancing the row's column cursor. `None` means it was dropped.
fn cell_pos(st: &mut RowState, c: &crate::rows::CellAttrs<'_>) -> Option<CellRef> {
    if st.skip_row {
        return None;
    }
    let pos = match c.r {
        Some(a) => match CellRef::parse(a.trim()) {
            Some(p) => p,
            None => {
                st.dropped += 1;
                return None;
            }
        },
        None => CellRef::new(st.row, st.next_col),
    };
    if pos.col >= MAX_COLS || pos.row >= MAX_ROWS {
        st.dropped += 1;
        return None;
    }
    st.next_col = pos.col + 1;
    Some(pos)
}

fn cell(dc: &DecodeCtx<'_>, sheet: &mut Sheet, st: &mut RowState, c: &crate::rows::CellAttrs<'_>, warn: &mut Vec<String>) {
    let Some(pos) = cell_pos(st, c) else { return };
    let Some(d) = decode_cell(dc, warn, st, c, pos) else { return };
    if let Some(picture) = d.picture {
        sheet.set_picture(d.at, picture);
        sheet.set_style(d.at, d.style);
        return;
    }
    match d.cell {
        // The cell's own reference wins over the row it sits in (a file may put a cell in a
        // different row); only the common case goes through the row batch.
        Some(cell) if d.at.row == st.row => st.row_cells.push((d.at.col, cell)),
        Some(cell) => sheet.cells.set(d.at, cell),
        None => {}
    }
}

fn row_end(sheet: &mut Sheet, st: &mut RowState) {
    if !st.skip_row {
        sheet.set_row_cells(st.row, &mut st.row_cells);
    }
    st.row_cells.clear();
}

fn decode_cell(dc: &DecodeCtx<'_>, warn: &mut Vec<String>, st: &mut RowState, c: &crate::rows::CellAttrs<'_>, pos: CellRef) -> Option<DecodedCell> {
    let style = dc.style(c.s.and_then(|v| v.trim().parse::<u32>().ok()));
    let v = c.v;
    let value = match c.t.unwrap_or("n") {
        "s" => match v.and_then(|v| v.trim().parse::<usize>().ok()) {
            Some(i) => match dc.sst.get(i) {
                Some(s) => Value::Text(s.clone()),
                None => {
                    warn.push("some cells refer to missing shared strings".into());
                    Value::Empty
                }
            },
            None => Value::Empty,
        },
        "str" => v.map(|v| Value::text(xml::decode_xstring(v))).unwrap_or(Value::Empty),
        "inlineStr" => {
            c.inline.map(|is| Value::text(xml::rich_text(is))).or_else(|| v.map(|v| Value::text(xml::decode_xstring(v)))).unwrap_or(Value::Empty)
        }
        "b" => v.map(|v| Value::Bool(matches!(v.trim(), "1" | "true" | "TRUE"))).unwrap_or(Value::Empty),
        "e" => v.map(|v| Value::Error(CellError::parse(v.trim()).unwrap_or(CellError::Value))).unwrap_or(Value::Empty),
        "d" => match v.and_then(|v| parse_iso_datetime(v.trim(), dc.date_system)) {
            Some(n) => Value::number(n),
            None => {
                if let Some(v) = v
                    && !v.trim().is_empty()
                {
                    warn.push("some date cells could not be read".into());
                }
                Value::Empty
            }
        },
        _ => match v.map(str::trim).filter(|v| !v.is_empty()) {
            Some(t) => match t.parse::<f64>() {
                Ok(n) if n.is_finite() => Value::Number(if n == 0.0 { 0.0 } else { n }),
                _ => {
                    warn.push("some numeric cells could not be read".into());
                    Value::Empty
                }
            },
            None => Value::Empty,
        },
    };
    let picture = c.vm.and_then(|vm| {
        if vm == "0" {
            return None;
        }
        let picture = vm.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).and_then(|n| dc.pictures.get(n)).cloned().flatten().clone();
        if picture.is_none() {
            warn.push("some cells have unsupported or missing value metadata; kept their scalar fallback (cell pictures may be missing)".into());
        }
        picture
    });
    if let Some(picture) = picture {
        if c.f.is_some() {
            warn.push("formula-generated cell pictures were imported as static pictures; their formulas were not retained".into());
        }
        return Some(DecodedCell { at: pos, style, cell: None, picture: Some(picture) });
    }
    let dynamic = c.cm;
    let formula = c.f.as_ref().and_then(|f| read_formula(warn, st, f, pos, dynamic));
    let value = if formula.is_none() && st.dynamic.iter().any(|r| r.contains(pos) && r.start != pos) { Value::Empty } else { value };
    Some(DecodedCell { at: pos, style, cell: Some(Cell { value, formula: formula.map(Arc::new), style }), picture: None })
}

fn read_formula(warn: &mut Vec<String>, st: &mut RowState, f: &crate::rows::FormulaAttrs<'_>, pos: CellRef, dynamic: bool) -> Option<Formula> {
    // Keep the file's dynamic-array mark, so saving never turns the formula into a legacy one.
    read_formula_text(warn, st, f, pos, dynamic).map(|fm| Formula { dynamic, ..fm })
}

fn read_formula_text(warn: &mut Vec<String>, st: &mut RowState, f: &crate::rows::FormulaAttrs<'_>, pos: CellRef, dynamic: bool) -> Option<Formula> {
    let model = from_file(f.text.trim());
    let text = model.as_str();
    match f.kind.unwrap_or("normal") {
        "shared" => {
            let si = f.si.and_then(|v| v.trim().parse::<u32>().ok())?;
            if !text.is_empty() {
                let expr = gridcraft_formula::parse(text).ok();
                st.shared.insert(si, Shared { anchor: pos, expr, text: text.to_string() });
                Some(Formula::new(text))
            } else {
                let Some(sh) = st.shared.get(&si) else {
                    warn.push("a shared formula refers to a missing master cell; kept the cached value".into());
                    return None;
                };
                Some(match &sh.expr {
                    Some(e) => {
                        let dr = pos.row as i64 - sh.anchor.row as i64;
                        let dc = pos.col as i64 - sh.anchor.col as i64;
                        Formula::from_expr(gridcraft_formula::adjust::shift_relative(e.clone(), dr, dc))
                    }
                    None => Formula::new(&sh.text),
                })
            }
        }
        "array" => {
            if text.is_empty() {
                return None;
            }
            let mut fm = Formula::new(text);
            let range = f.range.and_then(RangeRef::parse).unwrap_or(RangeRef::cell(pos));
            if !range.is_single() {
                st.dynamic.push(range);
            }
            if !dynamic {
                fm.array = Some(range);
            }
            Some(fm)
        }
        "dataTable" => {
            warn.push("what-if data tables are not supported; kept their values".into());
            None
        }
        _ => {
            if text.is_empty() {
                None
            } else {
                Some(Formula::new(text))
            }
        }
    }
}

/// `2024-03-15`, `2024-03-15T10:30:00Z`, `10:30:00` → serial.
pub fn parse_iso_datetime(s: &str, sys: DateSystem) -> Option<f64> {
    let s = s.trim_end_matches('Z');
    let (date, time) = match s.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None if s.contains(':') => ("", Some(s)),
        None => (s, None),
    };
    let mut serial = 0.0;
    if !date.is_empty() {
        let mut it = date.split('-');
        let y: i64 = it.next()?.parse().ok()?;
        let m: i64 = it.next()?.parse().ok()?;
        let d: i64 = it.next()?.parse().ok()?;
        if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
            return None;
        }
        serial = serial_from_ymd(sys, y, m, d)?;
    }
    if let Some(t) = time {
        // Drop a time-zone offset.
        let t = t.split(['+']).next().unwrap_or(t);
        let mut it = t.split(':');
        let h: f64 = it.next()?.parse().ok()?;
        let mi: f64 = it.next().unwrap_or("0").parse().ok()?;
        let se: f64 = it.next().unwrap_or("0").parse().ok()?;
        if !(0.0..24.0).contains(&h) || !(0.0..60.0).contains(&mi) || !(0.0..61.0).contains(&se) {
            return None;
        }
        serial += time_fraction(h, mi, se);
    }
    Some(serial)
}

pub fn parse_sqref(s: Option<&str>) -> Vec<RangeRef> {
    s.unwrap_or("").split_whitespace().filter_map(RangeRef::parse).collect()
}

fn cf_op(s: Option<&str>) -> CfOperator {
    match s.unwrap_or("between") {
        "notBetween" => CfOperator::NotBetween,
        "equal" => CfOperator::Equal,
        "notEqual" => CfOperator::NotEqual,
        "greaterThan" => CfOperator::Greater,
        "lessThan" => CfOperator::Less,
        "greaterThanOrEqual" => CfOperator::GreaterOrEqual,
        "lessThanOrEqual" => CfOperator::LessOrEqual,
        _ => CfOperator::Between,
    }
}

fn cfvo(e: &El) -> CfValueKind {
    let val = e.attr("val").unwrap_or("");
    let n = val.trim().parse::<f64>().ok().filter(|v| v.is_finite());
    match e.attr("type").unwrap_or("num") {
        "min" | "autoMin" => CfValueKind::Min,
        "max" | "autoMax" => CfValueKind::Max,
        "percent" => n.map(CfValueKind::Percent).unwrap_or_else(|| CfValueKind::Formula(from_file(val))),
        "percentile" => n.map(CfValueKind::Percentile).unwrap_or_else(|| CfValueKind::Formula(from_file(val))),
        "formula" => CfValueKind::Formula(from_file(val)),
        _ => n.map(CfValueKind::Number).unwrap_or_else(|| CfValueKind::Formula(from_file(val))),
    }
}

/// `cfvo` elements: plain children, or x14 ones with the value in an `xm:f` child.
fn cfvos(e: &El) -> Vec<CfValueKind> {
    e.kids("cfvo")
        .map(|c| {
            if c.attr("val").is_none()
                && let Some(f) = c.child("f")
            {
                let mut c2 = c.clone();
                c2.attrs.push(("val".into(), f.text.clone()));
                return cfvo(&c2);
            }
            cfvo(c)
        })
        .collect()
}

fn read_cf(cx: &mut Ctx<'_>, e: &El, sheet: &mut Sheet) {
    let ranges = parse_sqref(e.attr("sqref"));
    if ranges.is_empty() {
        return;
    }
    for r in e.kids("cfRule") {
        let style = Box::new(r.attr_u32("dxfId").and_then(|i| cx.dxfs.get(i as usize)).cloned().unwrap_or_default());
        let formulas: Vec<String> = r.kids("formula").map(|f| from_file(f.text.trim())).collect();
        let f0 = formulas.first().cloned().unwrap_or_default();
        let text = r.attr("text").unwrap_or("").to_string();
        // The x14 id that links a data bar to its Excel 2010 extension (solid fill lives there).
        let ext_id =
            r.child("extLst").and_then(|l| l.kids("ext").find_map(|x| x.child("id"))).map(|i| i.text.trim().to_string()).filter(|i| !i.is_empty());
        let rule = match r.attr("type").unwrap_or("") {
            "cellIs" => CfRule::CellIs { op: cf_op(r.attr("operator")), a: f0, b: formulas.get(1).cloned(), style },
            "expression" => CfRule::Expression { formula: f0, style },
            "containsText" => CfRule::ContainsText { text, style },
            "notContainsText" => CfRule::NotContainsText { text, style },
            "beginsWith" => CfRule::BeginsWith { text, style },
            "endsWith" => CfRule::EndsWith { text, style },
            "containsBlanks" => CfRule::Blanks { style },
            "notContainsBlanks" => CfRule::NoBlanks { style },
            "containsErrors" => CfRule::Errors { style },
            "notContainsErrors" => CfRule::NoErrors { style },
            "duplicateValues" => CfRule::Duplicate { style },
            "uniqueValues" => CfRule::Unique { style },
            "top10" => {
                CfRule::Top10 { bottom: r.flag("bottom", false), percent: r.flag("percent", false), rank: r.attr_u32("rank").unwrap_or(10), style }
            }
            "aboveAverage" => CfRule::AboveAverage {
                below: !r.flag("aboveAverage", true),
                equal: r.flag("equalAverage", false),
                std_dev: r.attr_u32("stdDev").unwrap_or(0).min(3) as u8,
                style,
            },
            "timePeriod" => CfRule::TimePeriod { period: r.attr("timePeriod").unwrap_or("today").to_string(), style },
            "colorScale" => {
                let Some(cs) = r.child("colorScale") else { continue };
                let colors = cs.kids("color").map(|c| read_color(c, &cx.palette));
                CfRule::ColorScale { stops: cfvos(cs).into_iter().zip(colors).collect() }
            }
            "dataBar" => {
                let Some(db) = r.child("dataBar") else { continue };
                let v = cfvos(db);
                CfRule::DataBar {
                    min: v.first().cloned().unwrap_or(CfValueKind::Min),
                    max: v.get(1).cloned().unwrap_or(CfValueKind::Max),
                    color: db.child("color").map(|c| read_color(c, &cx.palette)).unwrap_or_default(),
                    gradient: true,
                    show_value: db.flag("showValue", true),
                }
            }
            "iconSet" => {
                let Some(is) = r.child("iconSet") else { continue };
                CfRule::IconSet {
                    set: is.attr("iconSet").unwrap_or("3TrafficLights1").to_string(),
                    thresholds: cfvos(is),
                    reverse: is.flag("reverse", false),
                    show_value: is.flag("showValue", true),
                }
            }
            other => {
                cx.warn(format!("conditional format type \"{other}\" is not supported"));
                continue;
            }
        };
        if let Some(id) = ext_id {
            cx.cf_ext_ids.push((id, sheet.cond_formats.len()));
        }
        sheet.cond_formats.push(CondFormat {
            ranges: ranges.clone(),
            rule,
            priority: r.attr_u32("priority").unwrap_or(0),
            stop_if_true: r.flag("stopIfTrue", false),
        });
    }
}

/// A `dataValidation` (main namespace, or x14 with `xm:f` formulas and an `xm:sqref` child).
fn read_validation(v: &El) -> Option<Validation> {
    let sq = v.attr("sqref").map(str::to_string).or_else(|| v.child("sqref").map(|s| s.text.clone()));
    let ranges = parse_sqref(sq.as_deref());
    if ranges.is_empty() {
        return None;
    }
    let formula = |n: &str| -> Option<String> {
        let f = v.child(n)?;
        let t = f.child("f").map(|x| x.text.as_str()).unwrap_or(f.text.as_str()).trim();
        if t.is_empty() { None } else { Some(t.to_string()) }
    };
    let kind = match v.attr("type").unwrap_or("none") {
        "whole" => ValidationKind::Whole,
        "decimal" => ValidationKind::Decimal,
        "list" => ValidationKind::List,
        "date" => ValidationKind::Date,
        "time" => ValidationKind::Time,
        "textLength" => ValidationKind::TextLength,
        "custom" => ValidationKind::Custom,
        _ => ValidationKind::Any,
    };
    let mut f1 = formula("formula1").unwrap_or_default();
    if kind == ValidationKind::List && !f1.is_empty() && !f1.starts_with('"') {
        f1 = format!("={}", from_file(&f1));
    } else if !f1.starts_with('"') && !f1.is_empty() {
        f1 = from_file(&f1);
    }
    Some(Validation {
        ranges,
        kind,
        op: cf_op(v.attr("operator")),
        f1,
        f2: formula("formula2").map(|f| from_file(&f)),
        allow_blank: v.flag("allowBlank", false),
        // `showDropDown="1"` *hides* the in-cell list arrow.
        in_cell_dropdown: !v.flag("showDropDown", false),
        input_title: v.attr("promptTitle").unwrap_or("").to_string(),
        input_message: v.attr("prompt").unwrap_or("").to_string(),
        show_input: v.flag("showInputMessage", false),
        error_title: v.attr("errorTitle").unwrap_or("").to_string(),
        error_message: v.attr("error").unwrap_or("").to_string(),
        error_style: match v.attr("errorStyle") {
            Some("warning") => ErrorStyle::Warning,
            Some("information") => ErrorStyle::Information,
            _ => ErrorStyle::Stop,
        },
        show_error: v.flag("showErrorMessage", false),
    })
}

fn read_hyperlinks(cx: &mut Ctx<'_>, name: &str, e: &El, rels: &[Rel], sheet: &mut Sheet) {
    let mut cells = 0u64;
    for h in e.kids("hyperlink") {
        let Some(range) = h.attr("ref").and_then(RangeRef::parse) else { continue };
        let mut target = h.attr("id").and_then(|id| rels.iter().find(|r| r.id == id)).map(|r| r.target.clone()).unwrap_or_default();
        // A relationship is a file or web page; a bare `location` is a place in this workbook.
        let external = !target.is_empty();
        if let Some(loc) = h.attr("location").filter(|l| !l.is_empty()) {
            if target.is_empty() {
                target = loc.trim_start_matches('#').to_string();
            } else {
                target = format!("{target}#{loc}");
            }
        }
        if target.is_empty() {
            continue;
        }
        let link = Hyperlink { target, tooltip: h.attr("tooltip").map(str::to_string), external };
        for c in range.iter() {
            if cells >= MAX_HYPERLINK_CELLS {
                cx.warn(format!("sheet \"{name}\": hyperlinks past the first {MAX_HYPERLINK_CELLS} cells were dropped"));
                return;
            }
            cells += 1;
            sheet.hyperlinks.insert(c, link.clone());
        }
    }
}

fn filter_op(s: Option<&str>) -> &'static str {
    match s.unwrap_or("equal") {
        "lessThan" => "<",
        "lessThanOrEqual" => "<=",
        "notEqual" => "<>",
        "greaterThanOrEqual" => ">=",
        "greaterThan" => ">",
        _ => "=",
    }
}

pub fn read_autofilter(cx: &mut Ctx<'_>, e: &El) -> Option<AutoFilter> {
    let range = e.attr("ref").and_then(RangeRef::parse)?;
    let mut criteria = vec![];
    for fc in e.kids("filterColumn") {
        let Some(col) = fc.attr_u32("colId") else { continue };
        let crit = if let Some(f) = fc.child("filters") {
            FilterCriterion::Values {
                values: f.kids("filter").filter_map(|x| x.attr("val")).map(str::to_string).collect(),
                blanks: f.flag("blank", false),
            }
        } else if let Some(cf) = fc.child("customFilters") {
            let mut it = cf.kids("customFilter").map(|c| (filter_op(c.attr("operator")).to_string(), c.attr("val").unwrap_or("").to_string()));
            let Some(a) = it.next() else { continue };
            FilterCriterion::Custom { a, b: it.next(), and: cf.flag("and", false) }
        } else if let Some(t) = fc.child("top10") {
            FilterCriterion::Top10 {
                bottom: !t.flag("top", true),
                percent: t.flag("percent", false),
                count: t.attr_f64("val").unwrap_or(10.0).clamp(0.0, 1e9) as u32,
            }
        } else if let Some(d) = fc.child("dynamicFilter") {
            match d.attr("type") {
                Some("aboveAverage") => FilterCriterion::AboveAverage(true),
                Some("belowAverage") => FilterCriterion::AboveAverage(false),
                _ => {
                    cx.warn("date filters in AutoFilters are not supported");
                    continue;
                }
            }
        } else if let Some(c) = fc.child("colorFilter") {
            let Some(dxf) = c.attr_u32("dxfId").and_then(|i| cx.dxfs.get(i as usize)) else { continue };
            if c.flag("cellColor", true) { FilterCriterion::FillColor(dxf.fill.fg) } else { FilterCriterion::FontColor(dxf.font.color) }
        } else if let Some(i) = fc.child("iconFilter") {
            let Some(set) = i.attr("iconSet") else { continue };
            FilterCriterion::Icon { set: set.to_string(), id: i.attr_u32("iconId") }
        } else {
            continue;
        };
        criteria.push((col, crit));
    }
    Some(AutoFilter { range, criteria })
}

pub fn read_table(cx: &mut Ctx<'_>, x: &El) -> Option<Table> {
    let range = x.attr("ref").and_then(RangeRef::parse)?;
    let name = x.attr("displayName").or_else(|| x.attr("name")).unwrap_or("Table").to_string();
    let header_row = x.attr_u32("headerRowCount").unwrap_or(1) > 0;
    let totals_row = x.attr_u32("totalsRowCount").unwrap_or(0) > 0;
    let columns = x
        .child("tableColumns")
        .map(|tc| {
            tc.kids("tableColumn")
                .map(|c| {
                    let totals = match c.attr("totalsRowFunction").unwrap_or("none") {
                        "average" => TotalsFn::Average,
                        "count" => TotalsFn::Count,
                        "countNums" => TotalsFn::CountNums,
                        "max" => TotalsFn::Max,
                        "min" => TotalsFn::Min,
                        "sum" => TotalsFn::Sum,
                        "stdDev" => TotalsFn::StdDev,
                        "var" => TotalsFn::Var,
                        "custom" => TotalsFn::Custom,
                        _ => TotalsFn::None,
                    };
                    let totals_label = if totals == TotalsFn::Custom {
                        c.child("totalsRowFormula").map(|f| from_file(f.text.trim()))
                    } else {
                        c.attr("totalsRowLabel").map(str::to_string)
                    };
                    TableColumn {
                        name: xml::decode_xstring(c.attr("name").unwrap_or("")),
                        totals,
                        totals_label,
                        formula: c.child("calculatedColumnFormula").map(|f| from_file(f.text.trim())),
                        unique_name: c.attr("uniqueName").map(str::to_string),
                        xml: c.child("xmlColumnPr").and_then(|x| {
                            Some(XmlColumnPr {
                                map_id: x.attr_u32("mapId")?,
                                xpath: x.attr("xpath")?.to_string(),
                                data_type: x.attr("xmlDataType").unwrap_or("string").to_string(),
                                denormalized: x.flag("denormalized", false),
                            })
                        }),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let si = x.child("tableStyleInfo");
    // Only XML tables are kept as such: a query table's definition hangs off the table part, which
    // is rewritten, so it would lose it.
    let xml_table = x.attr("tableType") == Some("xml");
    Some(Table {
        id: x.attr_u32("id").unwrap_or(0),
        name,
        range,
        header_row,
        totals_row,
        columns,
        style: si.and_then(|s| s.attr("name")).unwrap_or("").to_string(),
        banded_rows: si.is_some_and(|s| s.flag("showRowStripes", false)),
        banded_cols: si.is_some_and(|s| s.flag("showColumnStripes", false)),
        first_col: si.is_some_and(|s| s.flag("showFirstColumn", false)),
        last_col: si.is_some_and(|s| s.flag("showLastColumn", false)),
        filter_button: x.child("autoFilter").is_some(),
        filter: x.child("autoFilter").and_then(|e| read_autofilter(cx, e)).map(|af| af.criteria).unwrap_or_default(),
        table_type: xml_table.then(|| "xml".to_string()),
        connection_id: x.attr_u32("connectionId").filter(|_| xml_table),
    })
}

fn read_comments(x: &El, sheet: &mut Sheet) {
    let authors: Vec<String> = x.child("authors").map(|a| a.kids("author").map(|a| a.text.clone()).collect()).unwrap_or_default();
    let Some(list) = x.child("commentList") else { return };
    for c in list.kids("comment") {
        let Some(cell) = c.attr("ref").and_then(CellRef::parse) else { continue };
        let author = c.attr_u32("authorId").and_then(|i| authors.get(i as usize)).cloned().unwrap_or_default();
        let mut text = c.child("text").map(xml::rich_text).unwrap_or_default();
        // Excel starts a note with a bold "Author:" line.
        if !author.is_empty()
            && let Some(rest) = text.strip_prefix(&format!("{author}:"))
        {
            text = rest.strip_prefix('\n').unwrap_or(rest).to_string();
        }
        sheet
            .comments
            .insert(cell, Comment { author, text, replies: vec![], threaded: false, resolved: false, visible: false, dates: vec![], note_box: None });
    }
}

/// Reads the notes' VML shapes: a shape carrying `<x:Visible/>` marks its note as shown, and its
/// `<x:Anchor>` and style size become the note's box. Shapes may sit in groups.
fn read_note_shapes(x: &El, sheet: &mut Sheet, depth: usize) {
    if depth > xml::MAX_DEPTH {
        return;
    }
    for e in &x.children {
        if e.name != "shape" {
            read_note_shapes(e, sheet, depth + 1);
            continue;
        }
        let Some(cd) = e.child("ClientData").filter(|cd| cd.attr("ObjectType") == Some("Note")) else { continue };
        let num = |k: &str| cd.child(k).and_then(|v| v.text.trim().parse::<u32>().ok());
        let (Some(row), Some(col)) = (num("Row"), num("Column")) else { continue };
        let cell = CellRef::new(row, col);
        let Some(c) = sheet.comments.get_mut(&cell) else { continue };
        if cd.child("Visible").is_some() && !c.threaded {
            c.visible = true;
        }
        if let Some(anchor) = cd.child("Anchor") {
            c.note_box = note_box(&anchor.text, e.attr("style").unwrap_or(""), cell).filter(|b| *b != crate::sheet_write::DEFAULT_NOTE_BOX);
        }
    }
}

/// A note's box from its VML `<x:Anchor>` (`LeftColumn, LeftOffset, TopRow, TopOffset,
/// RightColumn, RightOffset, BottomRow, BottomOffset`; offsets in pixels) and its shape's style,
/// the corners made relative to the note's `cell`. `None` when the anchor isn't eight in-sheet
/// numbers.
fn note_box(anchor: &str, style: &str, cell: CellRef) -> Option<NoteBox> {
    let v: Vec<u32> = anchor.split(',').map(|p| p.trim().parse::<u32>().ok()).collect::<Option<_>>()?;
    let [c1, dx1, r1, dy1, c2, dx2, r2, dy2] = <[u32; 8]>::try_from(v).ok()?;
    if c1.max(c2) >= MAX_COLS || r1.max(r2) >= MAX_ROWS {
        return None;
    }
    let corner = |c: u32, dx: u32, r: u32, dy: u32| NoteCorner {
        col: i64::from(c) - i64::from(cell.col),
        col_px: dx,
        row: i64::from(r) - i64::from(cell.row),
        row_px: dy,
    };
    let pt = |key: &str| {
        style
            .split(';')
            .filter_map(|d| d.split_once(':'))
            .find(|(k, _)| k.trim().eq_ignore_ascii_case(key))
            .and_then(|(_, v)| v.trim().strip_suffix("pt")?.trim().parse::<f64>().ok().filter(|n| n.is_finite() && *n > 0.0 && *n < 1e6))
    };
    Some(NoteBox { from: corner(c1, dx1, r1, dy1), to: corner(c2, dx2, r2, dy2), size_pt: pt("width").zip(pt("height")) })
}

fn read_threaded(cx: &Ctx<'_>, x: &El, sheet: &mut Sheet) {
    let mut by_id: HashMap<String, CellRef> = HashMap::new();
    let mut replies: Vec<(String, String, String, String)> = vec![];
    for t in x.kids("threadedComment") {
        let Some(cell) = t.attr("ref").and_then(CellRef::parse) else { continue };
        let author = t.attr("personId").and_then(|p| cx.persons.get(p)).cloned().unwrap_or_default();
        let text = t.child("text").map(|e| e.text.clone()).unwrap_or_default();
        let date = t.attr("dT").unwrap_or("").to_string();
        match t.attr("parentId") {
            Some(p) => replies.push((p.to_string(), author, text, date)),
            None => {
                if let Some(id) = t.attr("id") {
                    by_id.insert(id.to_string(), cell);
                }
                let dates = if date.is_empty() { vec![] } else { vec![date] };
                sheet.comments.insert(
                    cell,
                    Comment { author, text, replies: vec![], threaded: true, resolved: t.flag("done", false), visible: false, dates, note_box: None },
                );
            }
        }
    }
    // Replies in file order; ones whose thread is missing are dropped.
    for (parent, author, text, date) in replies {
        if let Some(c) = by_id.get(&parent)
            && let Some(cm) = sheet.comments.get_mut(c)
        {
            if !date.is_empty() {
                // `dates[0]` is the thread, `dates[i + 1]` reply i; unknown dates stay empty.
                let at = cm.replies.len() + 1;
                if cm.dates.len() < at {
                    cm.dates.resize(at, String::new());
                }
                cm.dates.push(date);
            }
            cm.replies.push((author, text));
        }
    }
}

/// Worksheet extensions: x14 data validations and sparklines.
fn read_ext(cx: &mut Ctx<'_>, e: &El, sheet: &mut Sheet) {
    for ext in e.kids("ext") {
        for c in &ext.children {
            match c.name.as_str() {
                "dataValidations" => {
                    for v in c.kids("dataValidation") {
                        if let Some(v) = read_validation(v) {
                            sheet.validations.push(v);
                        }
                    }
                }
                "sparklineGroups" => {
                    for g in c.kids("sparklineGroup") {
                        let kind = match g.attr("type") {
                            Some("column") => SparklineKind::Column,
                            Some("stacked") => SparklineKind::WinLoss,
                            _ => SparklineKind::Line,
                        };
                        let color = g.child("colorSeries").map(|c| read_color(c, &cx.palette)).unwrap_or_default();
                        let markers = g.flag("markers", false);
                        for s in g.child("sparklines").map(|s| s.kids("sparkline").collect::<Vec<_>>()).unwrap_or_default() {
                            let source = s.child("f").map(|f| f.text.trim().to_string()).unwrap_or_default();
                            if let Some(cell) = s.child("sqref").and_then(|q| CellRef::parse(q.text.trim())) {
                                sheet.sparklines.push(Sparkline { cell, source, kind, color, markers });
                            }
                        }
                    }
                }
                "conditionalFormattings" => {
                    let mut skipped = false;
                    for r in c.kids("conditionalFormatting").flat_map(|f| f.kids("cfRule")) {
                        let idx = r.attr("id").and_then(|id| cx.cf_ext_ids.iter().find(|(i, _)| i == id)).map(|(_, n)| *n);
                        match (idx.and_then(|n| sheet.cond_formats.get_mut(n)), r.child("dataBar")) {
                            (Some(CondFormat { rule: CfRule::DataBar { gradient, .. }, .. }), Some(db)) => {
                                *gradient = db.flag("gradient", true);
                                // Only the fill style is modelled; borders, lengths, axis or negative colours are lost.
                                skipped |= db.attrs.iter().any(|(k, _)| k != "gradient") || db.children.iter().any(|k| k.name != "cfvo");
                            }
                            _ => skipped = true,
                        }
                    }
                    if skipped {
                        cx.warn("some Excel 2010 conditional format extensions were skipped");
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_dates() {
        assert_eq!(parse_iso_datetime("1900-01-01", DateSystem::D1900), Some(1.0));
        assert_eq!(parse_iso_datetime("2024-01-01T12:00:00Z", DateSystem::D1900), Some(45292.5));
        assert_eq!(parse_iso_datetime("1904-01-02", DateSystem::D1904), Some(1.0));
        assert_eq!(parse_iso_datetime("06:00:00", DateSystem::D1900), Some(0.25));
        assert_eq!(parse_iso_datetime("garbage", DateSystem::D1900), None);
        assert_eq!(parse_iso_datetime("2024-13-01", DateSystem::D1900), None);
    }

    #[test]
    fn sqref_keeps_more_than_ten_thousand_ranges() {
        let sq = (1..=12_000).map(|r| format!("A{r}")).collect::<Vec<_>>().join(" ");
        let ranges = parse_sqref(Some(&sq));
        assert_eq!(ranges.len(), 12_000);
        assert_eq!(ranges.last().copied(), RangeRef::parse("A12000"));
    }
}
