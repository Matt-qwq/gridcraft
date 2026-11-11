//! The chart substream of an embedded `.xls` chart ([MS-XLS] 2.1.7.20.1, `BOF` type 0x0020).
//!
//! Excel keeps an embedded chart inside its worksheet substream: the drawing record that anchors
//! the chart shape, its `OBJ` record, then the chart substream itself. The substream follows the
//! `CHARTFOMATS` rule: a `Chart` record, one or more chart groups (`ChartFormat` + `Bar`/`Line`/
//! `Pie`/`Area`/`Scatter`/`Radar`), a `Series` block per series (four `BRAI` formulas: name,
//! values, categories and bubble sizes), then `ShtProps`, axes and the chart title
//! (`SeriesText` + `ObjectLink` with `wLinkObj = 1`).
//!
//! Only what the model holds is read: the chart kind, its series and their range formulas, the
//! title and whether a legend exists. Axis titles, per-point formatting, trendlines, data labels
//! and the plot layout are left to the model's defaults; those options survive only for charts
//! GridCraft writes itself. A chart the reader cannot express is skipped rather than read wrong.

use gridcraft_core::CellRef;
use gridcraft_model::{Anchor, AnchorMode, Chart, ChartKind, LegendPos, Series};

use super::Budget;
use super::formula::{Links, Mode, decode};
use super::records::{Cursor, Record, Records};

const CHART: u16 = 0x1002;
const SERIES: u16 = 0x1003;
const BRAI: u16 = 0x1051;
const SERIES_TEXT: u16 = 0x100D;
const LEGEND: u16 = 0x1015;
const BAR: u16 = 0x1017;
const LINE: u16 = 0x1018;
const PIE: u16 = 0x1019;
const AREA: u16 = 0x101A;
const SCATTER: u16 = 0x101B;
const RADAR: u16 = 0x103E;
const SURFACE: u16 = 0x103F;
const RADAR_AREA: u16 = 0x1040;
const CHART3D: u16 = 0x103A;
const BOP_POP: u16 = 0x105D;
const OBJECT_LINK: u16 = 0x1027;
const SHT_PROPS: u16 = 0x1044;
const EOF: u16 = 0x000A;

/// Series read from one chart at most (Excel allows far fewer).
const MAX_SERIES: usize = 4096;
/// Chart text kept at most: series names and the title.
const MAX_TEXT: usize = 1 << 20;

/// Reads one embedded chart substream, up to and including its `EOF`. `anchor` is the shape's
/// anchor from the sheet drawing. `None` when the substream holds no chart the model can express.
pub(super) fn read(recs: &mut Records<'_>, budget: &mut Budget, links: &Links, anchor: Option<Anchor>) -> Result<Option<Chart>, crate::IoError> {
    let mut kind: Option<ChartKind> = None;
    let mut series: Vec<Series> = Vec::new();
    let mut current: Option<Series> = None;
    let mut title: Option<String> = None;
    let mut pending_text: Option<String> = None;
    let mut legend = false;
    let mut saw_chart = false;
    let mut unsupported = false;
    let mut text = 0usize;
    while let Some(rec) = recs.next_record()? {
        budget.record()?;
        match rec.id {
            EOF => break,
            CHART => saw_chart = true,
            SERIES => {
                if let Some(s) = current.take() {
                    keep(&mut series, s);
                }
                if series.len() >= MAX_SERIES {
                    return Ok(None);
                }
                current = Some(Series::default());
            }
            BRAI => {
                if let Some(s) = current.as_mut()
                    && let Ok((id, rt, formula)) = brai(&rec)
                    && rt == 2
                {
                    let value = print_formula(&formula, links);
                    // On a scatter or bubble chart the second formula is the horizontal (x)
                    // values and the third the vertical (y) ones, the opposite of a category chart
                    // ([MS-XLS] 2.2.3.9).
                    let xy = matches!(kind, Some(ChartKind::Scatter | ChartKind::Bubble));
                    match (id, value) {
                        // The name is cached in SeriesText; a reference is what a save can write
                        // back, so it is kept only when it really is one.
                        (0, Some(v)) if s.name.is_none() => s.name = Some(v),
                        (1, Some(v)) if xy => s.categories = Some(v),
                        (2, Some(v)) if xy => s.values = v,
                        (1, Some(v)) => s.values = v,
                        (2, Some(v)) => s.categories = Some(v),
                        (3, Some(v)) => s.bubble_sizes = Some(v),
                        _ => {}
                    }
                }
            }
            SERIES_TEXT => {
                let Ok(s) = series_text(&rec) else { continue };
                text = text.saturating_add(s.len());
                if text > MAX_TEXT {
                    return Ok(None);
                }
                if let Some(cur) = current.as_mut() {
                    if cur.name.is_none() {
                        cur.name = Some(s);
                    }
                } else {
                    pending_text = Some(s);
                }
            }
            SHT_PROPS => {
                if let Some(s) = current.take() {
                    keep(&mut series, s);
                }
            }
            BAR | LINE | PIE | AREA | SCATTER | RADAR | RADAR_AREA => {
                if kind.is_none() {
                    kind = group_kind(&rec);
                }
            }
            // Kinds the model has no chart kind for: a 3-D, surface or bar-of-pie chart is kept
            // out rather than imported as the wrong kind.
            CHART3D | SURFACE | BOP_POP => unsupported = true,
            LEGEND => legend = true,
            OBJECT_LINK => {
                // wLinkObj 1 is the chart title; 2 and 3 are axis titles.
                let mut c = Cursor::new(&rec.parts);
                if c.u16().ok() == Some(1) {
                    title = pending_text.take().filter(|s| !s.is_empty());
                }
            }
            _ => {}
        }
    }
    if let Some(s) = current.take() {
        keep(&mut series, s);
    }
    if unsupported {
        return Err(super::invalid("an .xls chart kind GridCraft can't import (3-D, surface or bar of pie)"));
    }
    if !saw_chart || series.is_empty() {
        return Ok(None);
    }
    let kind = kind.unwrap_or(ChartKind::ColumnClustered);
    let pie = matches!(kind, ChartKind::Pie | ChartKind::Doughnut);
    let anchor = anchor.unwrap_or(Anchor { cell: CellRef::default(), dx: 0.0, dy: 0.0, width: 480.0, height: 288.0, mode: AnchorMode::MoveAndSize });
    Ok(Some(Chart {
        kind,
        anchor,
        title,
        series,
        legend: if legend { LegendPos::Bottom } else { LegendPos::None },
        gridlines: !pie,
        ..Chart::default()
    }))
}

/// Keeps a series if it has values: a series without a range has nothing to draw or save.
fn keep(out: &mut Vec<Series>, s: Series) {
    if !s.values.is_empty() && out.len() < MAX_SERIES {
        out.push(s);
    }
}

/// A `BRAI` record's id, reference type and formula (`ChartParsedFormula`: a `cce` count and the
/// token stream). The cached values that follow in the substream are not needed: the model stores
/// the range and the `.xlsx` writer caches the cells itself.
fn brai(rec: &Record<'_>) -> Result<(u8, u8, Vec<u8>), crate::IoError> {
    let mut c = Cursor::new(&rec.parts);
    let id = c.u8()?;
    let rt = c.u8()?;
    c.skip(4)?; // the flags/ifmt word
    let n = usize::from(c.u16()?);
    Ok((id, rt, c.bytes(n)?))
}

/// `SeriesText` ([MS-XLS] 2.4.254): two reserved bytes then a ShortXLUnicodeString.
fn series_text(rec: &Record<'_>) -> Result<String, crate::IoError> {
    let mut c = Cursor::new(&rec.parts);
    c.skip(2)?;
    c.short_string()
}

/// The chart group a record opens, from its flags ([MS-XLS] 2.4.2 Area, 2.4.15 Bar, 2.4.155 Line,
/// 2.4.194 Pie, 2.4.213 RadarArea, 2.4.243 Scatter).
fn group_kind(rec: &Record<'_>) -> Option<ChartKind> {
    let p = rec.parts.first().copied().unwrap_or_default();
    let u = |at: usize| p.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).unwrap_or(0);
    Some(match rec.id {
        BAR => {
            let (transpose, stacked, hundred) = (u(4) & 1 != 0, u(4) & 2 != 0, u(4) & 4 != 0);
            match (transpose, stacked, hundred) {
                (false, true, true) => ChartKind::ColumnStacked100,
                (false, true, false) => ChartKind::ColumnStacked,
                (false, false, _) => ChartKind::ColumnClustered,
                (true, true, true) => ChartKind::BarStacked100,
                (true, true, false) => ChartKind::BarStacked,
                (true, false, _) => ChartKind::BarClustered,
            }
        }
        LINE => {
            if u(0) & 1 != 0 {
                ChartKind::LineStacked
            } else {
                ChartKind::Line
            }
        }
        PIE => {
            // Pie ([MS-XLS] 2.4.194) is anStart (the first slice's angle), then pcDonut: zero is
            // a pie, anything else a doughnut.
            if u(2) == 0 { ChartKind::Pie } else { ChartKind::Doughnut }
        }
        AREA => {
            if u(0) & 1 != 0 {
                ChartKind::AreaStacked
            } else {
                ChartKind::Area
            }
        }
        SCATTER => {
            if u(4) & 1 != 0 {
                ChartKind::Bubble
            } else {
                ChartKind::Scatter
            }
        }
        RADAR | RADAR_AREA => ChartKind::Radar,
        _ => return None,
    })
}

/// A chart range formula as the text the model stores (`Data!$B$2:$B$5`). A chart formula is a
/// reference, so only reference expressions are printed; a literal or a constant is not a range.
fn print_formula(rgce: &[u8], links: &Links) -> Option<String> {
    let expr = decode(rgce, &[], CellRef::default(), Mode::Cell, links).ok()?;
    Some(gridcraft_formula::print(&expr))
}
