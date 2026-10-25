//! Every test here writes a file and reads it back: the schema says what a column became, and the
//! values say nothing was lost on the way.

use std::sync::Arc;

use gridcraft_core::{CellError, CellRef, RangeRef, Value};
use gridcraft_model::{Cell, Sheet, Workbook};
use parquet::file::reader::{FileReader, SerializedFileReader};

use super::*;

fn sheet_with(values: &[(u32, u32, Value)]) -> Sheet {
    let mut wb = Workbook::new();
    let sheet = wb.sheet_mut(0).expect("sheet");
    for (row, col, v) in values {
        sheet.cells.set(CellRef::new(*row, *col), Cell::value(v.clone()));
    }
    wb.sheet(0).expect("sheet").clone()
}

fn read(bytes: &[u8]) -> SerializedFileReader<bytes::Bytes> {
    SerializedFileReader::new(bytes::Bytes::from(bytes.to_vec())).expect("read back")
}

/// The row-major values of the file, all as text, with `None` for a null.
fn rows(reader: &SerializedFileReader<bytes::Bytes>) -> Vec<Vec<Option<String>>> {
    let iter = reader.get_row_iter(None).expect("rows");
    iter.map(|row| {
        let row = row.expect("row");
        (0..row.len())
            .map(|i| {
                let field = row.get_column_iter().nth(i).map(|(_, f)| f).expect("field");
                match field.to_string().as_str() {
                    "null" => None,
                    other => Some(other.trim_matches('"').to_string()),
                }
            })
            .collect()
    })
    .collect()
}

#[test]
fn a_sheet_becomes_typed_columns_with_real_nulls() {
    let sheet = sheet_with(&[
        // Header.
        (0, 0, Value::text("region")),
        (0, 1, Value::text("units")),
        (0, 2, Value::text("price")),
        (0, 3, Value::text("active")),
        // Body: text, whole number, fraction, boolean; the second row is blank in `price`.
        (1, 0, Value::text("North")),
        (1, 1, Value::Number(12.0)),
        (1, 2, Value::Number(3.5)),
        (1, 3, Value::Bool(true)),
        (2, 0, Value::text("South")),
        (2, 1, Value::Number(7.0)),
        (2, 3, Value::Bool(false)),
    ]);
    let (bytes, report) = export_sheet(&sheet, Options::default()).expect("export");

    let types: Vec<&str> = report.columns.iter().map(|(_, t)| t.as_str()).collect();
    assert_eq!(types, ["STRING", "INT64", "DOUBLE", "BOOLEAN"]);
    let names: Vec<&str> = report.columns.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["region", "units", "price", "active"]);
    assert_eq!(report.rows, 2);

    let reader = read(&bytes);
    assert_eq!(
        rows(&reader),
        [
            vec![Some("North".into()), Some("12".into()), Some("3.5".into()), Some("true".into())],
            vec![Some("South".into()), Some("7".into()), None, Some("false".into())],
        ]
    );
}

#[test]
fn a_column_of_numbers_stays_numbers_and_a_mixed_one_is_text() {
    let sheet = sheet_with(&[
        (0, 0, Value::text("n")),
        (0, 1, Value::text("mixed")),
        (0, 2, Value::text("flags")),
        (1, 0, Value::Number(1.0)),
        (1, 1, Value::Number(1.0)),
        (1, 2, Value::Bool(true)),
        (2, 0, Value::Number(2.5)),
        (2, 1, Value::text("two")),
        (2, 2, Value::Bool(false)),
    ]);
    let (bytes, report) = export_sheet(&sheet, Options::default()).expect("export");
    let types: Vec<&str> = report.columns.iter().map(|(_, t)| t.as_str()).collect();
    assert_eq!(types, ["DOUBLE", "STRING", "BOOLEAN"]);

    let reader = read(&bytes);
    assert_eq!(
        rows(&reader),
        [vec![Some("1.0".into()), Some("1".into()), Some("true".into())], vec![Some("2.5".into()), Some("two".into()), Some("false".into())],]
    );
}

#[test]
fn errors_are_written_as_their_text_and_blanks_as_nulls() {
    let sheet = sheet_with(&[(0, 0, Value::text("v")), (1, 0, Value::Error(CellError::NA)), (3, 0, Value::Error(CellError::Div0))]);
    let (bytes, report) = export_sheet(&sheet, Options::default()).expect("export");
    assert_eq!(report.rows, 3, "the blank row between the errors is still a row");
    let reader = read(&bytes);
    assert_eq!(rows(&reader), [vec![Some("#N/A".into())], vec![None], vec![Some("#DIV/0!".into())]]);
}

#[test]
fn a_header_row_can_be_switched_off_and_blank_names_are_numbered() {
    let sheet = sheet_with(&[(0, 0, Value::Number(1.0)), (0, 1, Value::text("kept")), (1, 0, Value::Number(2.0)), (1, 1, Value::Number(3.0))]);

    let (_, named) = export_sheet(&sheet, Options { header: false, range: None }).expect("export");
    assert_eq!(named.columns.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["Column1", "Column2"]);
    assert_eq!(named.rows, 2);

    let (bytes, titled) = export_sheet(&sheet, Options::default()).expect("export");
    assert_eq!(titled.columns.first().map(|(n, _)| n.as_str()), Some("1"), "a numeric header is its text");
    assert_eq!(titled.rows, 1);
    assert_eq!(rows(&read(&bytes)), [vec![Some("2".into()), Some("3".into())]]);
}

#[test]
fn duplicate_names_are_numbered_so_select_works() {
    let sheet = sheet_with(&[(0, 0, Value::text("id")), (0, 1, Value::text("id")), (0, 2, Value::text("ID")), (1, 0, Value::Number(1.0))]);
    let (_, report) = export_sheet(&sheet, Options::default()).expect("export");
    assert_eq!(report.columns.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["id", "id_2", "ID"]);
}

#[test]
fn an_explicit_range_exports_only_that_block() {
    let sheet = sheet_with(&[
        (0, 0, Value::text("a")),
        (0, 1, Value::text("b")),
        (0, 5, Value::text("outside")),
        (1, 0, Value::Number(1.0)),
        (1, 1, Value::Number(2.0)),
        (1, 5, Value::Number(99.0)),
        (5, 0, Value::Number(100.0)),
    ]);
    let range = RangeRef::parse("A1:B2").expect("range");
    let (bytes, report) = export_sheet(&sheet, Options { header: true, range: Some(range) }).expect("export");
    assert_eq!(report.rows, 1);
    assert_eq!(report.columns.len(), 2);
    assert_eq!(rows(&read(&bytes)), [vec![Some("1".into()), Some("2".into())]]);
}

#[test]
fn an_empty_sheet_is_an_error_rather_than_an_empty_file() {
    let sheet = sheet_with(&[]);
    assert!(matches!(export_sheet(&sheet, Options::default()), Err(ExportError::Empty)));
}

#[test]
fn a_report_spanning_several_row_groups_reads_back_in_order() {
    // More rows than one row group holds, so the writer has to start a second one.
    let mut values = vec![(0u32, 0u32, Value::text("n"))];
    for row in 1..=ROW_GROUP_MAX_ROWS as u32 + 100 {
        values.push((row, 0, Value::Number(f64::from(row))));
    }
    let sheet = sheet_with(&values);
    let (bytes, report) = export_sheet(&sheet, Options::default()).expect("export");
    assert_eq!(report.rows, u64::from(ROW_GROUP_MAX_ROWS as u32 + 100));
    let reader = read(&bytes);
    assert!(reader.metadata().num_row_groups() > 1, "one row group, so the chunking never ran");
    let rows = rows(&reader);
    assert_eq!(rows.len(), ROW_GROUP_MAX_ROWS + 100);
    assert_eq!(rows.first().and_then(|r| r[0].clone()), Some("1".into()));
    assert_eq!(rows.last().and_then(|r| r[0].clone()), Some((ROW_GROUP_MAX_ROWS + 100).to_string()));
}

#[test]
fn a_block_that_cannot_fit_a_row_group_still_writes() {
    // A sheet wide enough that the cell budget wins over the row cap: 4096 columns × 300 rows.
    let mut values = Vec::new();
    for row in 0..300u32 {
        for col in 0..4096u32 {
            values.push((row, col, Value::Number(f64::from(row * 4096 + col))));
        }
    }
    let sheet = sheet_with(&values);
    let (bytes, report) = export_sheet(&sheet, Options { header: false, range: None }).expect("export");
    assert_eq!(report.rows, 300);
    assert_eq!(report.columns.len(), 4096);
    let reader = read(&bytes);
    assert_eq!(rows(&reader).len(), 300);
}

#[test]
fn the_arc_of_a_shared_text_is_written_once_per_value() {
    let shared: Arc<str> = Arc::from("shared");
    let sheet = sheet_with(&[(0, 0, Value::text("t")), (1, 0, Value::Text(shared.clone())), (2, 0, Value::Text(shared.clone()))]);
    let (bytes, _) = export_sheet(&sheet, Options::default()).expect("export");
    assert_eq!(rows(&read(&bytes)), [vec![Some("shared".into())], vec![Some("shared".into())]]);
}
