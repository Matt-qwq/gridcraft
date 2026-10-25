//! How fast the row scanner reads a sheet: `cargo run --release -p gridcraft-xlsx --example scan_probe -- <part.xml>`.
//!
//! Native-only benchmark: the clock read is behind an allow for that reason.
#![allow(clippy::disallowed_methods)]

fn main() {
    let path = std::env::args().nth(1).expect("usage: scan_probe <part.xml>");
    let xml = std::fs::read(&path).expect("read");
    let mib = xml.len() as f64 / (1024.0 * 1024.0);
    for round in 1..=3 {
        let t = std::time::Instant::now();
        let mut rows = 0u64;
        let mut cells = 0u64;
        gridcraft_xlsx::rows::parse(&xml, &mut |item| {
            match item {
                gridcraft_xlsx::rows::Item::RowStart(_) => rows += 1,
                gridcraft_xlsx::rows::Item::Cell(_) => cells += 1,
                _ => {}
            }
            Ok(())
        })
        .expect("parse");
        let took = t.elapsed().as_secs_f64();
        println!("round {round}: {mib:.1} MiB, {rows} rows, {cells} cells in {took:.3} s = {:.0} MiB/s", mib / took);
    }
}
