//! What a cell's value costs to read: `cargo run --release -p gridcraft-xlsx --example number_probe`.
//!
//! A sheet stores numbers as text (`<v>17950607</v>`), so every numeric cell pays a parse. Rust's
//! `f64` parser handles the general form; a spreadsheet's numbers are mostly short integers, which a
//! two-line loop reads without going near the error paths.

// The clock only reads on a native build, and this is a native-only benchmark.
#![allow(clippy::disallowed_methods)]

use std::time::Instant;

/// Digits only, no sign, no fraction, no exponent: the shape of a count, a date serial or an id.
fn fast_unsigned(bytes: &[u8]) -> Option<f64> {
    let mut n: u64 = 0;
    for &b in bytes {
        if !b.is_ascii_digit() {
            return None;
        }
        n = n.checked_mul(10)?.checked_add(u64::from(b - b'0'))?;
    }
    Some(n as f64)
}

fn main() {
    // The report's numbers: counts, money with two decimals, ids as text.
    let mut samples: Vec<String> = Vec::with_capacity(10_000_000);
    let mut state = 0x2545_f491_4f6c_dd1du64;
    for _ in 0..10_000_000 {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let n = state.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40;
        samples.push(format!("{}", n % 10_000_000));
    }

    let t = Instant::now();
    let mut sum = 0.0;
    for s in &samples {
        sum += s.parse::<f64>().unwrap_or(0.0);
    }
    let std_parse = t.elapsed();
    println!("str::parse f64: {:?} for 10M ({:.1} ns each)  sum {sum}", std_parse, std_parse.as_secs_f64() * 100.0);

    let t = Instant::now();
    let mut sum2 = 0.0;
    for s in &samples {
        sum2 += fast_unsigned(s.as_bytes()).unwrap_or_else(|| s.parse::<f64>().unwrap_or(0.0));
    }
    let digits = t.elapsed();
    println!("digits only:    {:?} for 10M ({:.1} ns each)  sum {sum2}", digits, digits.as_secs_f64() * 100.0);
    assert_eq!(sum, sum2);

    // With two decimals, the shape money takes.
    let mut money: Vec<String> = Vec::with_capacity(10_000_000);
    for s in &samples {
        let v: f64 = s.parse().unwrap_or(0.0);
        money.push(format!("{:.2}", v / 100.0));
    }
    let t = Instant::now();
    let mut sum3 = 0.0;
    for s in &money {
        sum3 += s.parse::<f64>().unwrap_or(0.0);
    }
    let money_parse = t.elapsed();
    println!("money as text:  {:?} for 10M ({:.1} ns each)  sum {sum3}", money_parse, money_parse.as_secs_f64() * 100.0);
}
