//! Value-axis scaling ("nice numbers"), Format Axis options and label formatting.

use gridcraft_core::{DateSystem, Value, number_to_text};
use gridcraft_model::AxisOptions;
use gridcraft_numfmt::{NumberFormat, format_value};

/// Largest magnitude we plot; bigger values are clamped so ranges stay finite.
pub(crate) const MAX_ABS: f64 = 1e300;

/// An axis scale: `min..=max` with gridlines every `step`, optionally logarithmic and reversed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale {
    pub min: f64,
    pub max: f64,
    pub step: f64,
    /// Logarithmic base (2..=1000); `None` is linear.
    pub log: Option<f64>,
    /// Values run from high to low (`orientation maxMin`).
    pub reversed: bool,
}

impl Scale {
    /// Tick values from `min` to `max` (at most 51): powers of the base on a log axis.
    pub fn ticks(&self) -> Vec<f64> {
        if let Some(base) = self.log {
            return self.log_ticks(base);
        }
        let span = self.max - self.min;
        if !(span.is_finite() && self.step.is_finite() && self.step > 0.0 && span > 0.0) {
            return vec![self.min];
        }
        let n = ((span / self.step).round() as i64).clamp(1, 50);
        (0..=n)
            .map(|i| {
                let off = i as f64 * self.step;
                // Snap away accumulated float error, keeping a bound that isn't a multiple of the
                // step (Excel's fixed minimum) where the options put it.
                let snapped = self.min + (off / self.step).round() * self.step;
                if snapped.abs() < self.step * 1e-9 { 0.0 } else { snapped }
            })
            .collect()
    }

    /// Powers of `base` between `min` and `max`, at most 51 of them.
    fn log_ticks(&self, base: f64) -> Vec<f64> {
        if !(base > 1.0 && base.is_finite() && self.min.is_finite() && self.max.is_finite() && self.min > 0.0 && self.max > self.min) {
            return vec![self.min];
        }
        let (lo, hi) = (self.min.log(base).floor(), self.max.log(base).ceil());
        if !(lo.is_finite() && hi.is_finite()) {
            return vec![self.min];
        }
        let decades = (hi - lo).clamp(0.0, 300.0);
        // A major unit on a log axis is how many powers of the base sit between ticks.
        let every = if self.step.is_finite() && self.step > 1.0 { self.step.min(300.0) } else { (decades / 50.0).ceil().max(1.0) };
        let mut out = Vec::new();
        let mut e = lo;
        while e <= hi + 1e-9 && out.len() < 52 {
            let v = base.powf(e);
            if v.is_finite() && v > 0.0 && v >= self.min * (1.0 - 1e-9) && v <= self.max * (1.0 + 1e-9) {
                out.push(v);
            }
            e += every;
        }
        if out.is_empty() { vec![self.min] } else { out }
    }

    /// Position of `v` along the axis, 0 at `min`, 1 at `max` (clamped to a sane band).
    pub fn frac(&self, v: f64) -> f32 {
        let f = match self.log {
            Some(base) if base > 1.0 => {
                if !(v.is_finite() && v > 0.0 && self.min > 0.0 && self.max > self.min) {
                    // A value a log axis cannot show (zero, negative, a broken scale) sits at the
                    // low end.
                    0.0
                } else {
                    let (lo, hi) = (self.min.log(base), self.max.log(base));
                    ((v.log(base) - lo) / (hi - lo)).clamp(-0.5, 1.5)
                }
            }
            _ => {
                let span = self.max - self.min;
                if !(span.is_finite() && span > 0.0 && v.is_finite()) {
                    return 0.0;
                }
                ((v - self.min) / span).clamp(-0.5, 1.5)
            }
        };
        let f = if self.reversed { 1.0 - f } else { f };
        f as f32
    }
}

pub(crate) fn clean(v: f64) -> f64 {
    if v.is_finite() { v.clamp(-MAX_ABS, MAX_ABS) } else { 0.0 }
}

/// Picks a "nice" scale (steps of 1, 2 or 5 × 10ⁿ, 4–9 gridlines) covering `lo..=hi`.
///
/// `force_zero` always includes 0 (columns, bars, areas). Otherwise 0 is still included when the
/// data sits reasonably close to it (min below 5/6 of max), like common spreadsheet behaviour.
/// A little headroom is added past the data so the extreme value doesn't touch the frame.
pub fn nice_scale(lo: f64, hi: f64, force_zero: bool) -> Scale {
    let (mut lo, mut hi) = (clean(lo), clean(hi));
    if lo > hi {
        std::mem::swap(&mut lo, &mut hi);
    }
    if force_zero || (lo > 0.0 && lo < hi * 5.0 / 6.0) || lo == hi {
        lo = lo.min(0.0);
    }
    if force_zero || (hi < 0.0 && hi > lo * 5.0 / 6.0) || lo == hi {
        hi = hi.max(0.0);
    }
    if lo == hi {
        // Both zero.
        return Scale { min: 0.0, max: 1.0, step: 0.2, log: None, reversed: false };
    }
    let range = hi - lo;
    if hi > 0.0 {
        hi += range * 0.05;
    }
    if lo < 0.0 {
        lo -= range * 0.05;
    }
    let range = hi - lo;
    if !range.is_finite() || range <= 0.0 {
        return Scale { min: 0.0, max: 1.0, step: 0.2, log: None, reversed: false };
    }
    let e = (range / 8.0).log10().floor();
    let mut chosen = None;
    'outer: for k in [e - 1.0, e, e + 1.0, e + 2.0] {
        let p = 10f64.powf(k);
        for m in [1.0, 2.0, 5.0] {
            let s = m * p;
            if !(s.is_finite() && s > 0.0) {
                continue;
            }
            let intervals = (hi / s - 1e-9).ceil() - (lo / s + 1e-9).floor();
            if intervals <= 8.0 {
                chosen = Some(s);
                break 'outer;
            }
        }
    }
    let step = chosen.unwrap_or(range);
    let mut min = (lo / step + 1e-9).floor() * step;
    let mut max = (hi / step - 1e-9).ceil() * step;
    if min.abs() < step * 1e-9 {
        min = 0.0;
    }
    if max.abs() < step * 1e-9 {
        max = 0.0;
    }
    if !(min.is_finite() && max.is_finite()) || max <= min {
        return Scale { min: lo, max: hi, step: range, log: None, reversed: false };
    }
    Scale { min, max, step, log: None, reversed: false }
}

/// A logarithmic scale: bounds rounded out to powers of `base`, at most four decades below data
/// that sits at or below zero (a log axis cannot show it).
fn log_scale(lo: f64, hi: f64, base: f64) -> Scale {
    let fallback = Scale { min: 1.0, max: base.max(10.0), step: 1.0, log: Some(base), reversed: false };
    let (mut lo, mut hi) = (clean(lo), clean(hi));
    if lo > hi {
        std::mem::swap(&mut lo, &mut hi);
    }
    if hi <= 0.0 {
        return Scale { min: 1.0, max: base.powi(4), step: 1.0, log: Some(base), reversed: false };
    }
    if lo <= 0.0 || !lo.is_finite() {
        lo = hi / base.powi(3);
    }
    let e_lo = lo.log(base).floor().clamp(-300.0, 299.0);
    let e_hi = hi.log(base).ceil().clamp(e_lo + 1.0, 300.0);
    let (min, max) = (base.powf(e_lo), base.powf(e_hi));
    if !(min.is_finite() && max.is_finite() && min > 0.0 && max > min) {
        return fallback;
    }
    Scale { min, max, step: 1.0, log: Some(base), reversed: false }
}

/// Applies Format Axis options (bounds, major unit, log base, reverse) to an automatic scale.
/// Hostile values are ignored: a non-finite bound, a bound that leaves no range, a log base
/// outside 2..=1000, a non-positive log bound or a non-positive major unit.
pub(crate) fn axis_scale(lo: f64, hi: f64, force_zero: bool, opts: &AxisOptions) -> Scale {
    let base = opts.log_base.filter(|b| b.is_finite() && (2.0..=1000.0).contains(b));
    let mut s = match base {
        Some(b) => log_scale(lo, hi, b),
        None => nice_scale(lo, hi, force_zero),
    };
    let automatic = s;
    if let Some(m) = opts.min.filter(|v| v.is_finite()) {
        s.min = m;
    }
    if let Some(m) = opts.max.filter(|v| v.is_finite()) {
        s.max = m;
    }
    // A log axis only shows positive values; a bound on the wrong side is dropped.
    if s.log.is_some() {
        if s.min <= 0.0 {
            s.min = automatic.min;
        }
        if s.max <= 0.0 {
            s.max = automatic.max;
        }
    }
    if !(s.min.is_finite() && s.max.is_finite()) || s.max <= s.min {
        s = automatic;
    }
    if let Some(u) = opts.major_unit.filter(|u| u.is_finite() && *u > 0.0) {
        // On a log axis the unit is powers of the base; on a linear one it is a data step. The
        // renderer holds at most 51 ticks, so a crumb-sized unit can't explode the layout.
        s.step = if s.log.is_some() { u.clamp(1.0, 300.0) } else { u.max((s.max - s.min) / 50.0) };
    }
    s.reversed = opts.reversed;
    s
}

/// Exact fixed scale (100% stacked axes).
pub(crate) fn fixed_scale(min: f64, max: f64, step: f64) -> Scale {
    Scale { min, max, step, log: None, reversed: false }
}

/// A number format prepared once and reused for many labels.
pub(crate) struct Fmt {
    general: bool,
    fmt: NumberFormat,
}

impl Fmt {
    pub fn new(code: &str) -> Fmt {
        let t = code.trim();
        let general = t.is_empty() || t.eq_ignore_ascii_case("general");
        Fmt { general, fmt: NumberFormat::parse_cached(if general { "General" } else { t }) }
    }
    /// Formats `v`; `step` (axis labels) fixes the decimals for General.
    pub fn format(&self, v: f64, step: Option<f64>) -> String {
        let mut v = clean(v);
        if let Some(s) = step
            && s > 0.0
            && (v / s).abs() < 1e-9
        {
            v = 0.0;
        }
        if self.general {
            return general_text(v, step);
        }
        let mut t = format_value(&Value::Number(v), &self.fmt, DateSystem::D1900).text;
        if t.chars().count() > 40 {
            t = t.chars().take(40).collect();
        }
        t
    }
}

fn general_text(v: f64, step: Option<f64>) -> String {
    if v == 0.0 {
        return "0".into();
    }
    match step {
        Some(s) if s.is_finite() && s > 0.0 && v.abs() < 1e11 && v.abs() >= 1e-9 => {
            let d = (-s.log10().floor()).clamp(0.0, 10.0) as usize;
            let t = format!("{:.*}", d, v);
            if t.trim_start_matches('-').chars().all(|c| c == '0' || c == '.') { "0".into() } else { t }
        }
        _ => number_to_text(v),
    }
}

/// Formats a number with a format code (General shows up to the precision implied by `step`).
pub fn format_number(v: f64, code: &str, step: Option<f64>) -> String {
    Fmt::new(code).format(v, step)
}
