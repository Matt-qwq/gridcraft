//! What a cell costs to read: `cargo run --release -p gridcraft-xlsx --example cell_probe -- <part.xml>`.
//!
//! Three callbacks over the same sheet: nothing, then the work `decode_cell` does (an attribute
//! parse, a number, a `Cell`), then that plus the band packing `pack_row` does. The difference
//! says where the decode's time goes, which is what to attack next.
#![allow(clippy::disallowed_methods)]

use gridcraft_core::CellRef;
use gridcraft_core::Value;
use gridcraft_model::{BandBatch, Cell};

fn main() {
    let path = std::env::args().nth(1).expect("usage: cell_probe <part.xml>");
    let xml = std::fs::read(&path).expect("read");
    let mib = xml.len() as f64 / (1024.0 * 1024.0);

    fn run(what: &str, xml: &[u8], mib: f64, mut cell: impl FnMut(&gridcraft_xlsx::rows::CellAttrs<'_>) -> Option<(u32, Cell)>, pack: bool) {
        let t = std::time::Instant::now();
        let mut cells = 0u64;
        let mut pending: Vec<(u32, Cell)> = Vec::new();
        let mut batch = BandBatch::new();
        let mut row_no = 0u32;
        let mut col = 0u32;
        gridcraft_xlsx::rows::parse(xml, &mut |item| {
            match item {
                gridcraft_xlsx::rows::Item::RowStart(a) => {
                    row_no = a.r.and_then(|r| r.parse().ok()).unwrap_or(row_no + 1);
                    col = 0;
                    pending.clear();
                }
                gridcraft_xlsx::rows::Item::Cell(c) => {
                    col += 1;
                    if let Some(done) = cell(&c) {
                        cells += 1;
                        pending.push(done);
                    }
                }
                gridcraft_xlsx::rows::Item::RowEnd => {
                    if pack && !pending.is_empty() {
                        batch.set_row(row_no.saturating_sub(1), &mut pending);
                    }
                }
            }
            Ok(())
        })
        .expect("parse");
        let took = t.elapsed().as_secs_f64();
        println!("{what:24} {mib:.1} MiB, {cells} cells in {took:.3} s = {:.0} MiB/s", mib / took);
    }

    run("scan only", &xml, mib, |_| None, false);
    run(
        "decode_cell",
        &xml,
        mib,
        |c| {
            // The style attribute and the number, as `decode_cell` reads them.
            let _style = c.s.and_then(|v| v.trim().parse::<u32>().ok());
            let v = c.v.map(str::trim).filter(|v| !v.is_empty())?;
            let n = v.parse::<f64>().ok()?;
            let col = c.r.and_then(CellRef::parse).map(|p| p.col).unwrap_or(0);
            Some((col, Cell { value: Value::Number(n), formula: None, style: Default::default() }))
        },
        false,
    );
    run(
        "decode_cell + bands",
        &xml,
        mib,
        |c| {
            let _style = c.s.and_then(|v| v.trim().parse::<u32>().ok());
            let v = c.v.map(str::trim).filter(|v| !v.is_empty())?;
            let n = v.parse::<f64>().ok()?;
            let col = c.r.and_then(CellRef::parse).map(|p| p.col).unwrap_or(0);
            Some((col, Cell { value: Value::Number(n), formula: None, style: Default::default() }))
        },
        true,
    );
}
