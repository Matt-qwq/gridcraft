//! A worksheet as a Parquet file: typed columns, real nulls, the format DuckDB, pandas and Spark
//! read directly.
//!
//! A spreadsheet cell has no type of its own, so the file's columns are typed by looking at what
//! the cells hold, once per column:
//!
//! | column holds | Parquet type |
//! |---|---|
//! | only whole numbers that survive a round trip through `i64` | `INT64` |
//! | only numbers | `DOUBLE` |
//! | only `TRUE`/`FALSE` | `BOOLEAN` |
//! | anything else (text, an error, text mixed with numbers) | `BYTE_ARRAY` with `STRING` |
//!
//! Blank cells are Parquet nulls rather than zeros or empty strings: "no value" is what they mean.
//! Errors are written as their text (`#N/A`), the way Excel's own export does, and numbers in a text
//! column are written with Excel's General format so `15` does not become `15.000000000000002`.
//! Dates stay numbers: a spreadsheet date *is* a number, and guessing a time zone for it would be
//! a guess.
//!
//! Written with the Apache `parquet` crate — the metadata is Thrift and the pages carry definition
//! levels, which is not something to hand-roll for a format three other tools have to read.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

use std::sync::Arc;

use gridcraft_core::{CellError, CellRef, RangeRef, Value, number_to_text};
use gridcraft_model::{Cell, Sheet};
use parquet::basic::{Compression, LogicalType, Repetition, Type as PhysicalType};
use parquet::data_type::{BoolType, ByteArrayType, DoubleType, Int64Type};
use parquet::file::properties::WriterProperties;
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::types::Type;

/// What a column became in the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnType {
    /// Whole numbers, `INT64`.
    Int64,
    /// Numbers, `DOUBLE`.
    Double,
    /// `TRUE`/`FALSE`, `BOOLEAN`.
    Bool,
    /// Text (and anything that had to be turned into it), `BYTE_ARRAY` with `STRING`.
    Text,
}

impl ColumnType {
    pub fn as_str(self) -> &'static str {
        match self {
            ColumnType::Int64 => "INT64",
            ColumnType::Double => "DOUBLE",
            ColumnType::Bool => "BOOLEAN",
            ColumnType::Text => "STRING",
        }
    }
}

/// What to export.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// The first row of the range holds the column names (it is not exported as data).
    pub header: bool,
    /// The block to export; `None` is the sheet's used range.
    pub range: Option<RangeRef>,
}

impl Default for Options {
    fn default() -> Self {
        Self { header: true, range: None }
    }
}

/// What was written.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Rows of data (the header row is not counted).
    pub rows: u64,
    /// One name and one type for each column of the file.
    pub columns: Vec<(String, ColumnType)>,
}

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("nothing to export: the sheet is empty")]
    Empty,
    #[error("{0}")]
    Parquet(#[from] parquet::errors::ParquetError),
}

/// Rows to buffer before starting a row group. A row group is what a reader skips whole, so it
/// wants to be big; the buffer holds every column of it, so it also has to stay small on a sheet
/// that is a thousand columns wide.
const ROW_GROUP_CELLS: usize = 1_000_000;
const ROW_GROUP_MAX_ROWS: usize = 65_536;

/// Writes `sheet` as a Parquet file and returns its bytes.
pub fn export_sheet(sheet: &Sheet, opts: Options) -> Result<(Vec<u8>, Report), ExportError> {
    let Some((range, header)) = plan_range(sheet, opts) else {
        return Err(ExportError::Empty);
    };
    let width = (range.end.col - range.start.col + 1) as usize;
    let body = if header { RangeRef::new(CellRef::new(range.start.row + 1, range.start.col), range.end) } else { range };
    let rows = if header && body.start.row > range.end.row { 0 } else { u64::from(body.end.row - body.start.row + 1) };

    let names = column_names(sheet, range, header, width);
    let kinds = infer(sheet, body, width);

    let fields = names
        .iter()
        .zip(&kinds)
        .map(|(name, kind)| {
            let physical = match kind {
                ColumnType::Int64 => PhysicalType::INT64,
                ColumnType::Double => PhysicalType::DOUBLE,
                ColumnType::Bool => PhysicalType::BOOLEAN,
                ColumnType::Text => PhysicalType::BYTE_ARRAY,
            };
            let mut field = Type::primitive_type_builder(name.as_str(), physical).with_repetition(Repetition::OPTIONAL);
            if *kind == ColumnType::Text {
                field = field.with_logical_type(Some(LogicalType::String));
            }
            field.build().map(Arc::new)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let schema = Type::group_type_builder("schema").with_fields(fields).build()?;
    let props = Arc::new(WriterProperties::builder().set_compression(Compression::SNAPPY).build());

    let mut out = Vec::new();
    let mut writer = SerializedFileWriter::new(&mut out, Arc::new(schema), props)?;
    let chunk = (ROW_GROUP_CELLS / width.max(1)).clamp(1, ROW_GROUP_MAX_ROWS);
    let mut at = body.start.row;
    while at <= body.end.row && rows > 0 {
        let last = at.saturating_add(chunk as u32 - 1).min(body.end.row);
        let mut buffers = Buffers::new(&kinds, (last - at + 1) as usize);
        // Read row by row: the store is row-major, so a column at a time would walk it once per
        // column.
        for row in at..=last {
            for (col, buf) in buffers.0.iter_mut().enumerate() {
                let c = CellRef::new(row, range.start.col + col as u32);
                buf.push(sheet.cells.get(c).as_ref());
            }
        }
        let mut group = writer.next_row_group()?;
        let mut col = 0usize;
        while let Some(mut writer_col) = group.next_column()? {
            if let Some(buf) = buffers.0.get(col) {
                buf.write(&mut writer_col)?;
            }
            writer_col.close()?;
            col += 1;
        }
        group.close()?;
        at = last.saturating_add(1);
    }
    writer.close()?;
    let report = Report { rows, columns: names.into_iter().zip(kinds).collect() };
    Ok((out, report))
}

/// The block to export and whether its first row is a header; `None` when there is nothing to write.
fn plan_range(sheet: &Sheet, opts: Options) -> Option<(RangeRef, bool)> {
    let range = opts.range.or_else(|| sheet.used_range())?;
    // Clamp to the grid so a hostile range cannot make the loops run past the sheet.
    let end = CellRef::new(range.end.row.min(gridcraft_core::MAX_ROWS - 1), range.end.col.min(gridcraft_core::MAX_COLS - 1));
    if range.start.row > end.row || range.start.col > end.col {
        return None;
    }
    Some((RangeRef::new(range.start, end), opts.header))
}

/// A header name per column: the cell's text, `Column{n}` when blank, made unique.
fn column_names(sheet: &Sheet, range: RangeRef, header: bool, width: usize) -> Vec<String> {
    let mut names: Vec<String> = (0..width)
        .map(|i| {
            let raw = if header {
                let cell = sheet.cells.get(CellRef::new(range.start.row, range.start.col + i as u32));
                cell.and_then(|c| match c.value {
                    Value::Text(t) => Some(t.to_string()),
                    Value::Number(n) => Some(number_to_text(n)),
                    Value::Bool(b) => Some(if b { "TRUE".into() } else { "FALSE".into() }),
                    _ => None,
                })
            } else {
                None
            };
            match raw.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) {
                Some(name) => name,
                None => format!("Column{}", i + 1),
            }
        })
        .collect();
    // Duplicate names are legal Parquet but break `SELECT` in every tool, so number the repeats.
    let mut seen = std::collections::HashMap::new();
    for name in &mut names {
        let count = seen.entry(name.clone()).or_insert(0usize);
        *count += 1;
        if *count > 1 {
            *name = format!("{name}_{count}");
        }
    }
    names
}

/// The type of each column, from what its cells hold.
fn infer(sheet: &Sheet, body: RangeRef, width: usize) -> Vec<ColumnType> {
    let mut ints = vec![true; width];
    let mut nums = vec![true; width];
    let mut bools = vec![true; width];
    for row in body.start.row..=body.end.row {
        for col in 0..width {
            let cell = sheet.cells.get(CellRef::new(row, body.start.col + col as u32));
            match cell.as_ref().map(|c| &c.value) {
                // A blank says nothing about the column.
                None | Some(Value::Empty) => {}
                Some(Value::Number(n)) => {
                    bools[col] = false;
                    if !whole(*n) {
                        ints[col] = false;
                    }
                }
                Some(Value::Bool(_)) => {
                    nums[col] = false;
                    ints[col] = false;
                }
                Some(Value::Array(a)) => {
                    // A spilled value in a cell: type the column by its top-left entry.
                    match a.data.first() {
                        Some(Value::Number(n)) => {
                            bools[col] = false;
                            if !whole(*n) {
                                ints[col] = false;
                            }
                        }
                        Some(Value::Bool(_)) => {
                            nums[col] = false;
                            ints[col] = false;
                        }
                        _ => {
                            nums[col] = false;
                            ints[col] = false;
                            bools[col] = false;
                        }
                    }
                }
                _ => {
                    nums[col] = false;
                    ints[col] = false;
                    bools[col] = false;
                }
            }
        }
    }
    (0..width)
        .map(|col| match (ints[col], nums[col], bools[col]) {
            (true, true, _) => ColumnType::Int64,
            (false, true, _) => ColumnType::Double,
            (false, false, true) => ColumnType::Bool,
            // Text, text mixed with numbers, and a column with no values at all: every value is a
            // null and a reader still needs a type.
            _ => ColumnType::Text,
        })
        .collect()
}

/// Whether a number can be written as an `i64` without changing it.
fn whole(n: f64) -> bool {
    n.is_finite() && n.trunc() == n && n.abs() <= 9_007_199_254_740_992.0
}

/// The values of one row group, one buffer per column.
struct Buffers(Vec<ColumnBuf>);

impl Buffers {
    fn new(kinds: &[ColumnType], rows: usize) -> Self {
        Self(
            kinds
                .iter()
                .map(|kind| match kind {
                    ColumnType::Int64 => ColumnBuf::Int(Vec::with_capacity(rows), Vec::with_capacity(rows)),
                    ColumnType::Double => ColumnBuf::Double(Vec::with_capacity(rows), Vec::with_capacity(rows)),
                    ColumnType::Bool => ColumnBuf::Bool(Vec::with_capacity(rows), Vec::with_capacity(rows)),
                    ColumnType::Text => ColumnBuf::Text(Vec::with_capacity(rows), Vec::with_capacity(rows)),
                })
                .collect(),
        )
    }
}

enum ColumnBuf {
    Int(Vec<i64>, Vec<i16>),
    Double(Vec<f64>, Vec<i16>),
    Bool(Vec<bool>, Vec<i16>),
    Text(Vec<String>, Vec<i16>),
}

impl ColumnBuf {
    /// Adds one cell: `None` for a blank (a null), and a value converted to the column's type.
    fn push(&mut self, cell: Option<&Cell>) {
        let value = cell.map(|c| &c.value);
        // A spilled array in a cell stands for its top-left value, as the sheet shows it.
        let value = match value {
            Some(Value::Array(a)) => a.data.first(),
            other => other,
        };
        match self {
            ColumnBuf::Int(values, defs) => match value {
                Some(Value::Number(n)) if whole(*n) => {
                    values.push(*n as i64);
                    defs.push(1);
                }
                // A column typed INT64 should hold only whole numbers, but a range that changed
                // between the two passes must not lose a value: write it as its integer part.
                Some(Value::Number(n)) if n.is_finite() => {
                    values.push(*n as i64);
                    defs.push(1);
                }
                _ => defs.push(0),
            },
            ColumnBuf::Double(values, defs) => match value {
                Some(Value::Number(n)) => {
                    values.push(*n);
                    defs.push(1);
                }
                _ => defs.push(0),
            },
            ColumnBuf::Bool(values, defs) => match value {
                Some(Value::Bool(b)) => {
                    values.push(*b);
                    defs.push(1);
                }
                _ => defs.push(0),
            },
            ColumnBuf::Text(values, defs) => match value {
                Some(Value::Text(t)) => {
                    values.push(t.to_string());
                    defs.push(1);
                }
                Some(Value::Number(n)) => {
                    values.push(number_to_text(*n));
                    defs.push(1);
                }
                Some(Value::Bool(b)) => {
                    values.push(if *b { "TRUE".into() } else { "FALSE".into() });
                    defs.push(1);
                }
                Some(Value::Error(e)) => {
                    values.push(error_text(*e).to_string());
                    defs.push(1);
                }
                _ => defs.push(0),
            },
        }
    }

    fn write(&self, writer: &mut parquet::file::writer::SerializedColumnWriter<'_>) -> Result<(), parquet::errors::ParquetError> {
        match self {
            ColumnBuf::Int(values, defs) => {
                writer.typed::<Int64Type>().write_batch(values, Some(defs), None)?;
            }
            ColumnBuf::Double(values, defs) => {
                writer.typed::<DoubleType>().write_batch(values, Some(defs), None)?;
            }
            ColumnBuf::Bool(values, defs) => {
                writer.typed::<BoolType>().write_batch(values, Some(defs), None)?;
            }
            ColumnBuf::Text(values, defs) => {
                let bytes: Vec<parquet::data_type::ByteArray> = values.iter().map(|v| parquet::data_type::ByteArray::from(v.as_str())).collect();
                writer.typed::<ByteArrayType>().write_batch(&bytes, Some(defs), None)?;
            }
        }
        Ok(())
    }
}

/// An error's text, as Excel writes it into a text column.
fn error_text(e: CellError) -> &'static str {
    match e {
        CellError::Null => "#NULL!",
        CellError::Div0 => "#DIV/0!",
        CellError::Value => "#VALUE!",
        CellError::Ref => "#REF!",
        CellError::Name => "#NAME?",
        CellError::Num => "#NUM!",
        CellError::NA => "#N/A",
        CellError::GettingData => "#GETTING_DATA",
        CellError::Spill => "#SPILL!",
        CellError::Calc => "#CALC!",
        CellError::Circ => "#CIRC!",
    }
}

#[cfg(test)]
mod tests;
