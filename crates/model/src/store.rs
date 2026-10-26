//! Copy-on-write sparse cell storage, packed.
//!
//! Cells live in bands of 64 rows, each band behind an `Arc`, so cloning a store (an undo
//! snapshot) copies only the band pointers and editing a cell copies just its band.
//!
//! Inside a band the cells lie in four stripes — a presence bitmap over the columns a row touches,
//! then one entry per stored cell in three parallel arrays (kind, payload, style) — plus side
//! tables for the two things a cell only rarely carries (a formula and an array value). A `Cell`
//! struct costs 40 bytes; packed like this a cell costs about 11, which is the difference between
//! a 30-million-cell sheet taking 1.6 GB and taking 400 MB.
//!
//! The stripes are band-wide vectors with a slice per row, not per-row vectors. A row that owned
//! its four arrays put four allocations behind every pointer, so reading a million single-cell
//! rows chased four million of them at ~46 ns a cell against ~2 ns for the arithmetic, and the row
//! header cost 108 bytes. Walking a stripe is sequential, the header is 48 bytes, and a page of a
//! band is one copy instead of a thousand.
//!
//! Text is the one payload that does not fit in eight bytes, so a band keeps a table of the strings
//! its rows use and the payload holds an index into it. Strings that arrive as the same `Arc` (as a
//! file's shared strings do — one `Arc` shared by every cell that mentions it) are found by
//! pointer, so deduplicating them costs a hash of a pointer, not a comparison of strings.
//!
//! Numbers are handled the same way, and it pays more: a report repeats its figures — counted on
//! one of 31.7 million cells, only 6% of the numbers in a band of 64 rows are distinct. A band
//! therefore keeps the distinct numbers it saw and a cell's payload is a two-byte index into that
//! table, so the eight bytes a number costs are paid once per distinct figure instead of per cell.
//! A band that overflows the index (more than 65,535 distinct numbers in 64 rows) falls back to
//! `NUMBER_WIDE`, which keeps the bits in the row's side table.

use std::collections::BTreeMap;
use std::sync::Arc;

use gridcraft_core::{CellError, CellRef, RangeRef, Value};
use serde::{Deserialize, Serialize};

use crate::cell::{Cell, Formula};
use crate::style::StyleId;

/// Rows per band: one undo snapshot shares bands this size, so an edit copies at most 64 rows.
const BAND: u32 = 64;

/// What a payload holds, one byte per stored cell, in column order within the row.
const NUMBER: u8 = 0;
const TEXT: u8 = 1;
const BOOL: u8 = 2;
const ERROR: u8 = 3;
/// No value: a styled cell, or a formula without a cached result.
const EMPTY: u8 = 4;
/// An array value, held in the row's `arrays` side table.
const ARRAY: u8 = 5;
/// A number whose band holds more distinct values than a `u16` can index; the payload points into
/// the row's `wide` table, which holds the bits.
const NUMBER_WIDE: u8 = 6;
/// The same for a text.
const TEXT_WIDE: u8 = 7;

/// A payload that fits the row's `u16` array, or one that has to go to the row's side table.
#[derive(Clone, Copy, Debug)]
enum Payload {
    Small(u16),
    Wide(u64),
}

const ERRORS: [CellError; 11] = [
    CellError::Null,
    CellError::Div0,
    CellError::Value,
    CellError::Ref,
    CellError::Name,
    CellError::Num,
    CellError::NA,
    CellError::GettingData,
    CellError::Spill,
    CellError::Calc,
    CellError::Circ,
];

fn error_code(e: CellError) -> u64 {
    match e {
        CellError::Null => 0,
        CellError::Div0 => 1,
        CellError::Value => 2,
        CellError::Ref => 3,
        CellError::Name => 4,
        CellError::Num => 5,
        CellError::NA => 6,
        CellError::GettingData => 7,
        CellError::Spill => 8,
        CellError::Calc => 9,
        CellError::Circ => 10,
    }
}

// The kinds and the styles of a band are runs of equal values over consecutive stored cells:
// `(value, length)` pairs in cell order. A report formats a row in two or three runs — a date
// column, a block of numbers, a block of text — and its cells hold kinds in runs the same way, so a
// per-cell array is mostly repetition: two bytes a cell for the style, one for the kind. An entry
// costs four bytes and covers a run, which measured on a 31.7M-cell report is 0.04 style runs and
// 0.18 kind runs a cell. The worst case, every cell its own run, costs a third more than the
// per-cell array.
//
// The runs live in the band's stripe, one slice per row (`at..at + len`), so the helpers below take
// the slice they work on. A row's slice is its own: two neighbours that happen to hold the same
// value keep two runs, because a row is what gets moved, paged and rebuilt.

/// The values of a run slice, expanded: how many cells it covers.
#[cfg(test)]
fn runs_len(runs: &[(u16, u16)]) -> usize {
    runs.iter().map(|(_, n)| *n as usize).sum()
}

/// The value of the `i`th cell of a run slice.
fn runs_at(runs: &[(u16, u16)], i: usize) -> u16 {
    runs_at_or(runs, i, 0)
}

fn runs_at_or(runs: &[(u16, u16)], i: usize, default: u16) -> u16 {
    let mut left = i;
    for (v, n) in runs {
        let n = *n as usize;
        if left < n {
            return *v;
        }
        left -= n;
    }
    default
}

/// Appends a value at the end of the run slice that starts at `base`: it lengthens the last run
/// when that run already holds the value, and never reaches into the run of the row before.
fn run_push(runs: &mut Vec<(u16, u16)>, base: usize, value: u16) {
    let extends = runs.len() > base && matches!(runs.last(), Some((last, n)) if *last == value && *n < u16::MAX);
    if extends {
        if let Some(last) = runs.last_mut() {
            last.1 += 1;
        }
    } else {
        runs.push((value, 1));
    }
}

/// Merges run `idx` with its neighbours when they hold the same value, never across the slice
/// `at..at + slice`, and returns the slice length after the runs that went away.
fn runs_merge(runs: &mut Vec<(u16, u16)>, at: usize, slice: usize, idx: usize) -> usize {
    let mut slice = slice;
    for start in [idx, idx.saturating_sub(1)] {
        let j = start;
        while j >= at && j + 1 < at.saturating_add(slice) && runs.get(j).zip(runs.get(j + 1)).is_some_and(|(a, b)| a.0 == b.0) {
            let n = runs[j + 1].1;
            runs[j].1 = runs[j].1.saturating_add(n);
            runs.remove(j + 1);
            slice = slice.saturating_sub(1);
        }
    }
    slice
}

/// Inserts `value` at cell `i` of the run slice `at..at + len`, and returns the slice's new length.
fn runs_insert(runs: &mut Vec<(u16, u16)>, at: usize, len: usize, i: usize, value: u16) -> usize {
    let mut slice = len;
    let mut pos = 0usize;
    let mut idx = at;
    while idx < at + slice {
        let (rv, rn) = match runs.get(idx) {
            Some(&run) => run,
            None => break,
        };
        let end = pos + rn as usize;
        if i > end {
            pos = end;
            idx += 1;
            continue;
        }
        // Extending a neighbouring run that already holds the value is just a longer run.
        if rv == value {
            runs[idx].1 += 1;
            return slice;
        }
        if i == pos {
            runs.insert(idx, (value, 1));
            slice += 1;
            return runs_merge(runs, at, slice, idx);
        }
        let before = (i - pos) as u16;
        runs[idx].1 = before;
        runs.insert(idx + 1, (value, 1));
        slice += 1;
        if rn > before {
            runs.insert(idx + 2, (rv, rn - before));
            slice += 1;
        }
        return runs_merge(runs, at, slice, idx + 1);
    }
    // Past the slice's last run: the new cell is the slice's last one.
    runs.insert(at + slice, (value, 1));
    slice += 1;
    if slice > 1 {
        return runs_merge(runs, at, slice, at + slice - 2);
    }
    slice
}

/// Removes cell `i` from the run slice `at..at + len`, and returns the slice's new length.
fn runs_remove(runs: &mut Vec<(u16, u16)>, at: usize, len: usize, i: usize) -> usize {
    let mut slice = len;
    let mut pos = 0usize;
    let mut idx = at;
    while idx < at + slice {
        let rn = match runs.get(idx) {
            Some(&(_, n)) => n as usize,
            None => break,
        };
        if i < pos + rn {
            runs[idx].1 -= 1;
            if runs[idx].1 == 0 {
                runs.remove(idx);
                slice -= 1;
                if idx > at && slice > 0 {
                    return runs_merge(runs, at, slice, idx - 1);
                }
            } else {
                return runs_merge(runs, at, slice, idx);
            }
            return slice;
        }
        pos += rn;
        idx += 1;
    }
    slice
}

/// Sets cell `i` of the run slice to `value`, and returns the slice's new length.
fn runs_set(runs: &mut Vec<(u16, u16)>, at: usize, len: usize, i: usize, value: u16) -> usize {
    if runs_at(runs.get(at..at.saturating_add(len)).unwrap_or(&[]), i) == value {
        return len;
    }
    let len = runs_insert(runs, at, len, i, value);
    // `runs_insert` put the new cell in its own run: drop the old one at i + 1.
    runs_remove(runs, at, len, i + 1)
}

/// Walks one row's run slice while its cells are visited in order. A per-cell search from the
/// row's first run would be quadratic in the runs a row holds, which is what a walk over a whole
/// band pays.
struct RunCursor<'a> {
    runs: &'a [(u16, u16)],
    idx: usize,
    value: u16,
    /// First cell of the next run.
    end: usize,
    default: u16,
}

impl<'a> RunCursor<'a> {
    fn new(runs: &'a [(u16, u16)], default: u16) -> Self {
        let (value, end) = runs.first().map_or((default, 0), |(v, n)| (*v, *n as usize));
        RunCursor { runs, idx: 0, value, end, default }
    }

    /// The value at cell `i`. Cells must be asked for in order (`i` never goes back).
    fn at(&mut self, i: usize) -> u16 {
        while i >= self.end {
            self.idx += 1;
            match self.runs.get(self.idx) {
                Some(&(v, n)) => {
                    self.value = v;
                    self.end += n as usize;
                }
                None => return self.default,
            }
        }
        self.value
    }
}

/// `v[at..at + n]`, empty when the range runs off the end: a stripe's offsets and its rows are
/// built together, so this only guards against a bug becoming a panic.
fn slice<T>(v: &[T], at: u32, n: u32) -> &[T] {
    v.get(at as usize..(at as usize).saturating_add(n as usize)).unwrap_or(&[])
}

fn insert_at<T>(v: &mut Vec<T>, at: usize, x: T) {
    let at = at.min(v.len());
    v.insert(at, x);
}

fn drop_at<T>(v: &mut Vec<T>, at: usize) {
    if at < v.len() {
        v.remove(at);
    }
}

fn drop_slice<T>(v: &mut Vec<T>, at: usize, n: usize) {
    let lo = at.min(v.len());
    let hi = at.saturating_add(n).min(v.len());
    v.drain(lo..hi);
}

/// The two things a cell only rarely carries: a formula, and an array value.
#[derive(Clone, Debug, Default)]
struct RowExtras {
    /// `(column, formula)`, sorted by column.
    formulas: Vec<(u32, Arc<Formula>)>,
    /// `(column, array value)`, sorted by column.
    arrays: Vec<(u32, Arc<gridcraft_core::Array>)>,
    /// Payloads too wide for the `u16` array, in the order their cells reached for them. Only a
    /// band with more than 65,535 distinct numbers ever has one.
    wide: Vec<u64>,
}

/// One row of a band: the row number, and where that row's cells sit in the band's four stripes.
///
/// This used to own the bitmap and the three cell arrays. It is an index now, so the row header is
/// 48 bytes instead of 108 and the cells of one band follow each other in memory.
#[derive(Clone, Debug, Default)]
struct BandRow {
    /// Row within the band.
    key: u32,
    /// Column of bit 0 of the bitmap, always a multiple of 64 so the bitmap grows by whole words.
    first: u32,
    /// First bitmap word in `Band::bits`, and how many words this row spans.
    bits_at: u32,
    words: u32,
    /// First cell in `Band::payloads`, and in the `Band::kinds` and `Band::styles` run stripes.
    cells_at: u32,
    kinds_at: u32,
    styles_at: u32,
    /// How many stored cells this row holds, and how many runs each stripe gives them.
    cells: u32,
    kind_runs: u32,
    style_runs: u32,
    /// Held out of line: a row with neither a formula nor an array pays nothing for them.
    extras: Option<Box<RowExtras>>,
}

impl BandRow {
    fn is_empty(&self) -> bool {
        self.cells == 0 && self.extras.is_none()
    }

    fn bits<'a>(&self, band: &'a Band) -> &'a [u64] {
        slice(&band.bits, self.bits_at, self.words)
    }

    fn kind_runs<'a>(&self, band: &'a Band) -> &'a [(u16, u16)] {
        slice(&band.kinds, self.kinds_at, self.kind_runs)
    }

    fn style_runs<'a>(&self, band: &'a Band) -> &'a [(u16, u16)] {
        slice(&band.styles, self.styles_at, self.style_runs)
    }

    /// The kind of the `i`th stored cell.
    fn kind_at(&self, band: &Band, i: usize) -> u8 {
        runs_at_or(self.kind_runs(band), i, u16::from(EMPTY)) as u8
    }

    /// The style of the `i`th stored cell.
    fn style_at(&self, band: &Band, i: usize) -> u16 {
        runs_at(self.style_runs(band), i)
    }

    /// The payload of the `i`th stored cell.
    fn payload_at(&self, band: &Band, i: usize) -> u16 {
        band.payloads.get(self.cells_at as usize + i).copied().unwrap_or(0)
    }

    fn wide_at(&self, band: &Band, i: usize) -> u64 {
        let slot = self.payload_at(band, i) as usize;
        self.extras.as_ref().and_then(|e| e.wide.get(slot).copied()).unwrap_or(0)
    }

    fn formulas(&self) -> &[(u32, Arc<Formula>)] {
        self.extras.as_ref().map_or(&[], |e| e.formulas.as_slice())
    }

    fn arrays(&self) -> &[(u32, Arc<gridcraft_core::Array>)] {
        self.extras.as_ref().map_or(&[], |e| e.arrays.as_slice())
    }

    fn drop_unused_extras(&mut self) {
        if self.extras.as_ref().is_some_and(|e| e.formulas.is_empty() && e.arrays.is_empty() && e.wide.is_empty()) {
            self.extras = None;
        }
    }

    /// Number of stored cells before `bit` of `word`.
    fn rank(&self, band: &Band, word: usize, bit: u32) -> usize {
        let bits = self.bits(band);
        let mut n = 0usize;
        for w in bits.get(..word).unwrap_or(&[]) {
            n += w.count_ones() as usize;
        }
        let w = bits.get(word).copied().unwrap_or(0);
        n + (w & ((1u64 << bit) - 1)).count_ones() as usize
    }

    /// Number of stored cells strictly before `col`.
    fn rank_before(&self, band: &Band, col: u32) -> usize {
        let Some(slot) = col.checked_sub(self.first) else { return 0 };
        let word = (slot / 64) as usize;
        if word >= self.words as usize {
            return self.cells as usize;
        }
        self.rank(band, word, slot % 64)
    }

    /// Index into the dense arrays for `col`, when the row stores a cell there.
    fn index_of(&self, band: &Band, col: u32) -> Option<usize> {
        let slot = col.checked_sub(self.first)?;
        let word = (slot / 64) as usize;
        let bit = slot % 64;
        let w = *self.bits(band).get(word)?;
        (w & (1u64 << bit) != 0).then(|| self.rank(band, word, bit))
    }

    /// The column of the `i`th stored cell.
    fn col_at(&self, band: &Band, i: usize) -> u32 {
        let mut seen = 0usize;
        for (word, bits) in self.bits(band).iter().enumerate() {
            let count = bits.count_ones() as usize;
            if seen + count <= i {
                seen += count;
                continue;
            }
            let mut left = i - seen;
            let mut w = *bits;
            while w != 0 {
                let bit = w.trailing_zeros();
                if left == 0 {
                    return self.first + word as u32 * 64 + bit;
                }
                left -= 1;
                w &= w - 1;
            }
        }
        self.first
    }

    fn columns(&self) -> u32 {
        self.words * 64
    }

    /// First and last stored column.
    fn column_span(&self, band: &Band) -> Option<(u32, u32)> {
        let mut first = None;
        let mut last = None;
        for (i, w) in self.bits(band).iter().enumerate() {
            if *w == 0 {
                continue;
            }
            if first.is_none() {
                first = Some(self.first + i as u32 * 64 + w.trailing_zeros());
            }
            last = Some(self.first + i as u32 * 64 + 63 - w.leading_zeros());
        }
        first.zip(last)
    }

    fn formula_at(&self, col: u32) -> Option<&Arc<Formula>> {
        let formulas = self.formulas();
        formulas.binary_search_by_key(&col, |(c, _)| *c).ok().map(|i| &formulas[i].1)
    }

    fn set_formula(&mut self, col: u32, formula: Option<&Arc<Formula>>) {
        match formula {
            Some(f) => {
                let e = self.extras.get_or_insert_with(Default::default);
                match e.formulas.binary_search_by_key(&col, |(c, _)| *c) {
                    Ok(i) => e.formulas[i].1 = f.clone(),
                    Err(i) => e.formulas.insert(i, (col, f.clone())),
                }
            }
            None => {
                if let Some(e) = &mut self.extras
                    && let Ok(i) = e.formulas.binary_search_by_key(&col, |(c, _)| *c)
                {
                    e.formulas.remove(i);
                }
                self.drop_unused_extras();
            }
        }
    }

    fn set_array(&mut self, col: u32, array: Option<&Arc<gridcraft_core::Array>>) {
        match array {
            Some(a) => {
                let e = self.extras.get_or_insert_with(Default::default);
                match e.arrays.binary_search_by_key(&col, |(c, _)| *c) {
                    Ok(i) => e.arrays[i].1 = a.clone(),
                    Err(i) => e.arrays.insert(i, (col, a.clone())),
                }
            }
            None => {
                if let Some(e) = &mut self.extras
                    && let Ok(i) = e.arrays.binary_search_by_key(&col, |(c, _)| *c)
                {
                    e.arrays.remove(i);
                }
                self.drop_unused_extras();
            }
        }
    }
}

/// The cells of one band, striped: the presence bitmaps, kinds, payloads and styles all live in one
/// vector each, a row being a slice of all four.
#[derive(Clone, Debug, Default)]
struct Band {
    /// Sorted by row: 64 rows of 48 bytes each, where a `BTreeMap`'s leaves sit three quarters full
    /// and cost some forty bytes a row for the privilege.
    rows: Vec<BandRow>,
    /// The presence bitmaps, 64 columns a word, one slice per row.
    bits: Vec<u64>,
    /// The kind of each stored cell, in runs; one slice per row.
    kinds: Vec<(u16, u16)>,
    /// The style of each stored cell, in runs; one slice per row.
    styles: Vec<(u16, u16)>,
    /// A `u16` per stored cell, in the same order as the kinds and styles stripes: an index into
    /// the band's number or text table, a boolean, an error code — or, for the rare wide kinds, an
    /// index into the row's `extras.wide`.
    payloads: Vec<u16>,
    /// The distinct numbers of this band, in the order they were seen; a `NUMBER` payload is an
    /// index into it.
    numbers: Vec<f64>,
    /// Open addressing over `numbers`: each slot holds an index plus one, zero meaning free.
    number_slots: Vec<u16>,
    texts: Vec<Arc<str>>,
    /// Pointer of each kept text → its slot, sorted by pointer, so one shared `Arc` is stored once
    /// per band. A band holds a handful of texts: a sorted vector beats a hash map's bookkeeping at
    /// that size, which is why the number table (hundreds of entries) is a hash and this is not.
    text_index: Vec<(usize, u32)>,
    /// The distinct styles of this band; the `styles` stripe holds indexes into it.
    style_ids: Vec<StyleId>,
}

/// Mixes the bits of a double into a slot index.
fn number_hash(bits: u64) -> usize {
    let mut h = bits.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 31;
    h as usize
}

impl Band {
    fn row_index(&self, key: u32) -> Option<usize> {
        self.rows.binary_search_by_key(&key, |r| r.key).ok()
    }

    fn row(&self, key: u32) -> Option<&BandRow> {
        self.row_index(key).map(|i| &self.rows[i])
    }

    /// The index of the row's entry, adding an empty row — its slices at the end of each stripe —
    /// when it isn't there yet.
    fn row_or_insert(&mut self, key: u32) -> usize {
        match self.rows.binary_search_by_key(&key, |r| r.key) {
            Ok(i) => i,
            Err(i) => {
                let row = BandRow {
                    key,
                    bits_at: self.bits.len() as u32,
                    cells_at: self.payloads.len() as u32,
                    kinds_at: self.kinds.len() as u32,
                    styles_at: self.styles.len() as u32,
                    ..Default::default()
                };
                self.rows.insert(i, row);
                i
            }
        }
    }

    /// Drops a row together with the cells it holds, keeping every stripe contiguous.
    fn clear_row_at(&mut self, at: usize) {
        if at >= self.rows.len() {
            return;
        }
        let row = self.rows.remove(at);
        let (bits_at, words) = (row.bits_at as usize, row.words as usize);
        let (cells_at, cells) = (row.cells_at as usize, row.cells as usize);
        let (kinds_at, kinds) = (row.kinds_at as usize, row.kind_runs as usize);
        let (styles_at, styles) = (row.styles_at as usize, row.style_runs as usize);
        drop_slice(&mut self.bits, bits_at, words);
        drop_slice(&mut self.payloads, cells_at, cells);
        drop_slice(&mut self.kinds, kinds_at, kinds);
        drop_slice(&mut self.styles, styles_at, styles);
        // Rows whose slices sat beyond what was dropped move up. Slices never overlap, and a row
        // built later may sit before one built earlier, so every remaining row is checked.
        for r in &mut self.rows {
            if r.bits_at as usize >= bits_at {
                r.bits_at = r.bits_at.saturating_sub(words as u32);
            }
            if r.cells_at as usize >= cells_at {
                r.cells_at = r.cells_at.saturating_sub(cells as u32);
            }
            if r.kinds_at as usize >= kinds_at {
                r.kinds_at = r.kinds_at.saturating_sub(kinds as u32);
            }
            if r.styles_at as usize >= styles_at {
                r.styles_at = r.styles_at.saturating_sub(styles as u32);
            }
        }
    }

    fn remove_row(&mut self, key: u32) {
        if let Some(i) = self.row_index(key) {
            self.clear_row_at(i);
        }
    }

    /// Moves the stripe offsets of every row but the one at `at` when a slice changed size.
    /// Each argument is `(where the change happened, how many entries it added or removed)`.
    fn shift_offsets(&mut self, at: usize, bits: (u32, i32), cells: (u32, i32), kinds: (u32, i32), styles: (u32, i32)) {
        for (i, r) in self.rows.iter_mut().enumerate() {
            if i == at {
                continue;
            }
            if bits.1 != 0 && r.bits_at >= bits.0 {
                r.bits_at = r.bits_at.saturating_add_signed(bits.1);
            }
            if cells.1 != 0 && r.cells_at >= cells.0 {
                r.cells_at = r.cells_at.saturating_add_signed(cells.1);
            }
            if kinds.1 != 0 && r.kinds_at >= kinds.0 {
                r.kinds_at = r.kinds_at.saturating_add_signed(kinds.1);
            }
            if styles.1 != 0 && r.styles_at >= styles.0 {
                r.styles_at = r.styles_at.saturating_add_signed(styles.1);
            }
        }
    }

    /// Grows the bitmap of the row at `at` to cover `col`, keeping `first` word-aligned, and opens
    /// the stripes up for the words that adds.
    fn cover(&mut self, at: usize, col: u32) {
        let (first, words) = (self.rows[at].first, self.rows[at].words);
        if words == 0 {
            self.rows[at].first = col / 64 * 64;
            let bits_at = self.rows[at].bits_at;
            insert_at(&mut self.bits, bits_at as usize, 0);
            self.rows[at].words = 1;
            self.shift_offsets(at, (bits_at, 1), (0, 0), (0, 0), (0, 0));
            return;
        }
        let mut left = 0usize;
        if col < first {
            left = ((first - col) as usize).div_ceil(64);
        }
        let cover = first.saturating_add(words.saturating_mul(64));
        let right = if col >= cover { ((col - cover) / 64 + 1) as usize } else { 0 };
        if left == 0 && right == 0 {
            return;
        }
        let bits_at = self.rows[at].bits_at as usize;
        let added = (left + right) as u32;
        if left > 0 {
            for _ in 0..left {
                insert_at(&mut self.bits, bits_at, 0);
            }
            self.rows[at].first = first - left as u32 * 64;
        }
        for _ in 0..right {
            let end = bits_at + left + words as usize;
            insert_at(&mut self.bits, end, 0);
        }
        self.rows[at].words = words + added;
        self.shift_offsets(at, (bits_at as u32, added as i32), (0, 0), (0, 0), (0, 0));
    }

    /// Puts a stored cell at cell index `i` of the row at `at`, opening the stripes up for it.
    fn insert_cell(&mut self, at: usize, i: usize, col: u32, kind: u8, payload: u16, style: u16) {
        let (bits_at, first) = (self.rows[at].bits_at, self.rows[at].first);
        let (cells_at, old_cells, kinds_at, old_kinds) =
            (self.rows[at].cells_at, self.rows[at].cells, self.rows[at].kinds_at, self.rows[at].kind_runs);
        let (styles_at, old_styles) = (self.rows[at].styles_at, self.rows[at].style_runs);
        let kind_runs = runs_insert(&mut self.kinds, kinds_at as usize, old_kinds as usize, i, u16::from(kind));
        let style_runs = runs_insert(&mut self.styles, styles_at as usize, old_styles as usize, i, style);
        insert_at(&mut self.payloads, cells_at as usize + i, payload);
        // `cover` has already made room for the bit.
        let slot = col.wrapping_sub(first);
        if let Some(w) = self.bits.get_mut(bits_at as usize + (slot / 64) as usize) {
            *w |= 1u64 << (slot % 64);
        }
        self.shift_offsets(
            at,
            (bits_at, 0),
            (cells_at, 1),
            (kinds_at, kind_runs as i32 - old_kinds as i32),
            (styles_at, style_runs as i32 - old_styles as i32),
        );
        let row = &mut self.rows[at];
        row.cells = old_cells + 1;
        row.kind_runs = kind_runs as u32;
        row.style_runs = style_runs as u32;
    }

    /// Drops the stored cell at index `i` of the row at `at`.
    fn remove_cell(&mut self, at: usize, i: usize, col: u32) {
        let (bits_at, first) = (self.rows[at].bits_at, self.rows[at].first);
        let (cells_at, old_cells, kinds_at, old_kinds) =
            (self.rows[at].cells_at, self.rows[at].cells, self.rows[at].kinds_at, self.rows[at].kind_runs);
        let (styles_at, old_styles) = (self.rows[at].styles_at, self.rows[at].style_runs);
        let kind_runs = runs_remove(&mut self.kinds, kinds_at as usize, old_kinds as usize, i);
        let style_runs = runs_remove(&mut self.styles, styles_at as usize, old_styles as usize, i);
        drop_at(&mut self.payloads, cells_at as usize + i);
        let slot = col.wrapping_sub(first);
        if let Some(w) = self.bits.get_mut(bits_at as usize + (slot / 64) as usize) {
            *w &= !(1u64 << (slot % 64));
        }
        self.shift_offsets(
            at,
            (bits_at, 0),
            (cells_at, -1),
            (kinds_at, kind_runs as i32 - old_kinds as i32),
            (styles_at, style_runs as i32 - old_styles as i32),
        );
        let row = &mut self.rows[at];
        row.cells = old_cells.saturating_sub(1);
        row.kind_runs = kind_runs as u32;
        row.style_runs = style_runs as u32;
    }

    /// Gives back what building the band over-allocated: the stripes, and the number index that
    /// only adding cells needs.
    fn finish(&mut self) {
        self.rows.shrink_to_fit();
        self.bits.shrink_to_fit();
        self.kinds.shrink_to_fit();
        self.styles.shrink_to_fit();
        self.payloads.shrink_to_fit();
        self.drop_number_slots();
    }

    fn text_slot(&mut self, s: &Arc<str>) -> Option<u16> {
        let key = Arc::as_ptr(s) as *const u8 as usize;
        match self.text_index.binary_search_by_key(&key, |(at, _)| *at) {
            Ok(i) => u16::try_from(self.text_index[i].1).ok(),
            Err(at) => {
                let slot = self.texts.len() as u32;
                self.texts.push(s.clone());
                self.text_index.insert(at, (key, slot));
                u16::try_from(slot).ok()
            }
        }
    }

    /// The band's slot for `v`, adding it when new — `None` once the band holds more numbers than
    /// a `u16` can index, which sends the cell to the row's wide table instead.
    fn number_slot(&mut self, v: f64) -> Option<u16> {
        let bits = if v == 0.0 { 0f64.to_bits() } else { v.to_bits() };
        if self.number_slots.is_empty() {
            if self.numbers.is_empty() {
                self.number_slots = vec![0; 16];
            } else {
                self.reindex_numbers();
            }
        }
        let mask = self.number_slots.len() - 1;
        let mut i = number_hash(bits) & mask;
        loop {
            let slot = self.number_slots[i];
            if slot == 0 {
                break;
            }
            if self.numbers[(slot - 1) as usize].to_bits() == bits {
                return Some(slot - 1);
            }
            i = (i + 1) & mask;
        }
        if self.numbers.len() >= 0xFFFF {
            return None;
        }
        let index = self.numbers.len() as u16;
        self.numbers.push(f64::from_bits(bits));
        self.number_slots[i] = index + 1;
        // Three quarters full: the probe walk stays short and the table stays small, which matters
        // because a band only holds a couple of hundred distinct numbers.
        if self.numbers.len() * 4 > self.number_slots.len() * 3 {
            self.grow_numbers();
        }
        Some(index)
    }

    /// Offers the number index back to the allocator. It is only needed to find a value while
    /// cells are being added, so a band that has been built does not keep it; an edit rebuilds it
    /// from the numbers the band still holds.
    fn drop_number_slots(&mut self) {
        self.number_slots = Vec::new();
    }

    fn reindex_numbers(&mut self) {
        let len = (self.numbers.len() * 2).max(16).next_power_of_two();
        self.number_slots = vec![0; len];
        let mask = len - 1;
        for (index, v) in self.numbers.iter().enumerate() {
            let mut i = number_hash(v.to_bits()) & mask;
            while self.number_slots[i] != 0 {
                i = (i + 1) & mask;
            }
            self.number_slots[i] = index as u16 + 1;
        }
    }

    fn grow_numbers(&mut self) {
        let len = self.number_slots.len() * 2;
        self.number_slots = vec![0; len];
        let mask = len - 1;
        for (index, v) in self.numbers.iter().enumerate() {
            let mut i = number_hash(v.to_bits()) & mask;
            while self.number_slots[i] != 0 {
                i = (i + 1) & mask;
            }
            self.number_slots[i] = index as u16 + 1;
        }
    }

    fn number(&self, slot: u16) -> f64 {
        self.numbers.get(slot as usize).copied().unwrap_or(0.0)
    }

    /// Kind and payload for a value. The tables are per band, so this needs the band.
    fn payload(&mut self, value: &Value) -> (u8, Payload) {
        match value {
            Value::Text(t) => match self.text_slot(t) {
                Some(slot) => (TEXT, Payload::Small(slot)),
                None => (TEXT_WIDE, Payload::Wide(u64::from(self.texts.len() as u32 - 1))),
            },
            Value::Number(n) => match self.number_slot(*n) {
                Some(slot) => (NUMBER, Payload::Small(slot)),
                None => (NUMBER_WIDE, Payload::Wide(if *n == 0.0 { 0f64.to_bits() } else { n.to_bits() })),
            },
            Value::Bool(b) => (BOOL, Payload::Small(u16::from(*b))),
            Value::Error(e) => (ERROR, Payload::Small(error_code(*e) as u16)),
            Value::Array(_) => (ARRAY, Payload::Small(0)),
            Value::Empty => (EMPTY, Payload::Small(0)),
        }
    }

    fn style_slot(&mut self, s: StyleId) -> u16 {
        if let Some(i) = self.style_ids.iter().position(|x| *x == s) {
            return i as u16;
        }
        self.style_ids.push(s);
        (self.style_ids.len() - 1) as u16
    }

    fn text(&self, payload: u64) -> Arc<str> {
        self.texts.get(payload as usize).cloned().unwrap_or_else(|| Arc::from(""))
    }

    fn style(&self, slot: u16) -> StyleId {
        self.style_ids.get(slot as usize).copied().unwrap_or(StyleId::DEFAULT)
    }

    /// Puts a payload that does not fit `u16` in the row's side table and returns its index there.
    fn push_wide(&mut self, at: usize, value: u64) -> u16 {
        let extras = self.rows[at].extras.get_or_insert_with(Default::default);
        extras.wide.push(value);
        (extras.wide.len() - 1) as u16
    }

    /// Rebuilds the `Cell` at `col` from the packed row.
    fn cell_at(&self, row: &BandRow, i: usize, col: u32) -> Cell {
        let payload = row.payload_at(self, i);
        let value = match row.kind_at(self, i) {
            NUMBER => Value::Number(self.number(payload)),
            NUMBER_WIDE => Value::Number(f64::from_bits(row.wide_at(self, i))),
            TEXT => Value::Text(self.text(u64::from(payload))),
            TEXT_WIDE => Value::Text(self.text(row.wide_at(self, i))),
            BOOL => Value::Bool(payload != 0),
            ERROR => Value::Error(*ERRORS.get(payload as usize).unwrap_or(&CellError::Value)),
            ARRAY => match row.arrays().binary_search_by_key(&col, |(c, _)| *c) {
                Ok(j) => row.arrays().get(j).map_or(Value::Empty, |(_, a)| Value::Array(a.clone())),
                Err(_) => Value::Empty,
            },
            _ => Value::Empty,
        };
        Cell { value, formula: row.formula_at(col).cloned(), style: self.style(row.style_at(self, i)) }
    }

    /// Stores `cell` at `col`; a blank cell is removed instead. Text and style slots are resolved
    /// before the row is borrowed, because both live in the band.
    fn put(&mut self, row_key: u32, col: u32, cell: Cell) {
        let blank = cell.is_blank();
        let (kind, payload) = if blank { (EMPTY, Payload::Small(0)) } else { self.payload(&cell.value) };
        let style = if blank { 0 } else { self.style_slot(cell.style) };
        let array = match &cell.value {
            Value::Array(a) => Some(a.clone()),
            _ => None,
        };
        let formula = cell.formula.clone();
        let at = self.row_or_insert(row_key);
        self.cover(at, col);
        let existing = self.rows[at].index_of(self, col);
        self.rows[at].set_formula(col, if blank { None } else { formula.as_ref() });
        self.rows[at].set_array(col, if blank { None } else { array.as_ref() });
        let payload = match payload {
            Payload::Small(p) => p,
            Payload::Wide(v) => self.push_wide(at, v),
        };
        match existing {
            Some(i) if blank => self.remove_cell(at, i, col),
            Some(i) => {
                let (kinds_at, old_kinds) = (self.rows[at].kinds_at, self.rows[at].kind_runs);
                let kind_runs = runs_set(&mut self.kinds, kinds_at as usize, old_kinds as usize, i, u16::from(kind));
                let (styles_at, old_styles) = (self.rows[at].styles_at, self.rows[at].style_runs);
                let style_runs = runs_set(&mut self.styles, styles_at as usize, old_styles as usize, i, style);
                let slot = self.rows[at].cells_at as usize + i;
                if let Some(p) = self.payloads.get_mut(slot) {
                    *p = payload;
                }
                self.shift_offsets(
                    at,
                    (0, 0),
                    (0, 0),
                    (kinds_at, kind_runs as i32 - old_kinds as i32),
                    (styles_at, style_runs as i32 - old_styles as i32),
                );
                self.rows[at].kind_runs = kind_runs as u32;
                self.rows[at].style_runs = style_runs as u32;
            }
            None if !blank => {
                let i = self.rows[at].rank_before(self, col);
                self.insert_cell(at, i, col, kind, payload, style);
            }
            None => {}
        }
        if self.rows[at].is_empty() {
            self.clear_row_at(at);
        }
    }

    /// Builds a whole row at once, from cells already sorted by column. The row's slices are
    /// appended to the band's stripes, so there is no growth slack to give back and no
    /// insert-per-cell shifting.
    fn put_row(&mut self, row_key: u32, cells: &mut Vec<(u32, Cell)>) {
        // One pass: the cells arrive sorted by column, so the bitmap can be sized from the first
        // and last non-blank ones and the row built as it is walked. This used to collect a
        // `Vec<(u32, u8, Payload, u16)>` first and walk that, which was one allocation and one
        // copy per cell more than the work needs.
        let first = match cells.iter().find(|(_, cell)| !cell.is_blank()).map(|(col, _)| col / 64 * 64) {
            Some(first) => first,
            None => {
                self.remove_row(row_key);
                return;
            }
        };
        let last = cells.iter().rev().find(|(_, cell)| !cell.is_blank()).map(|(col, _)| *col).unwrap_or(first);
        let words = (last - first) / 64 + 1;
        // A row that is being replaced goes first: dropping it moves the stripes' ends.
        if let Some(at) = self.row_index(row_key) {
            self.clear_row_at(at);
        }
        let (bits_at, cells_at) = (self.bits.len() as u32, self.payloads.len() as u32);
        let (kinds_at, styles_at) = (self.kinds.len() as u32, self.styles.len() as u32);
        self.bits.resize(self.bits.len() + words as usize, 0);
        let mut formulas: Vec<(u32, Arc<Formula>)> = Vec::new();
        let mut arrays: Vec<(u32, Arc<gridcraft_core::Array>)> = Vec::new();
        let mut wide: Vec<u64> = Vec::new();
        let mut stored = 0u32;
        for (col, cell) in cells.drain(..) {
            if cell.is_blank() {
                continue;
            }
            let (kind, payload) = self.payload(&cell.value);
            let style = self.style_slot(cell.style);
            if let Some(f) = &cell.formula {
                formulas.push((col, f.clone()));
            }
            if let Value::Array(a) = &cell.value {
                arrays.push((col, a.clone()));
            }
            let slot = col.wrapping_sub(first);
            if let Some(w) = self.bits.get_mut(bits_at as usize + (slot / 64) as usize) {
                *w |= 1u64 << (slot % 64);
            }
            run_push(&mut self.kinds, kinds_at as usize, u16::from(kind));
            run_push(&mut self.styles, styles_at as usize, style);
            let payload = match payload {
                Payload::Small(p) => p,
                Payload::Wide(v) => {
                    wide.push(v);
                    (wide.len() - 1) as u16
                }
            };
            self.payloads.push(payload);
            stored += 1;
        }
        let extras =
            if formulas.is_empty() && arrays.is_empty() && wide.is_empty() { None } else { Some(Box::new(RowExtras { formulas, arrays, wide })) };
        let row = BandRow {
            key: row_key,
            first,
            bits_at,
            words,
            cells_at,
            kinds_at,
            styles_at,
            cells: stored,
            kind_runs: self.kinds.len() as u32 - kinds_at,
            style_runs: self.styles.len() as u32 - styles_at,
            extras,
        };
        match self.rows.binary_search_by_key(&row_key, |r| r.key) {
            Ok(i) => self.rows[i] = row,
            Err(i) => self.rows.insert(i, row),
        }
    }

    fn take(&mut self, row_key: u32, col: u32) -> Option<Cell> {
        let at = self.row_index(row_key)?;
        let i = self.rows[at].index_of(self, col)?;
        let cell = self.cell_at(&self.rows[at], i, col);
        self.rows[at].set_formula(col, None);
        self.rows[at].set_array(col, None);
        self.remove_cell(at, i, col);
        if self.rows[at].is_empty() {
            self.clear_row_at(at);
        }
        Some(cell)
    }

    /// True when both bands hold the same cells. Text is compared by content: two bands that came
    /// from different reads index different tables.
    fn same_as(&self, other: &Band) -> bool {
        self.rows.len() == other.rows.len()
            && self.rows.iter().map(|r| r.key).eq(other.rows.iter().map(|r| r.key))
            && self.rows.iter().all(|a| other.row(a.key).is_some_and(|b| row_same_as(self, a, other, b)))
    }
}

/// True when two stored cells hold the same value, whatever slot their band gave it.
fn same_value(a: &Band, ra: &BandRow, i: usize, b: &Band, rb: &BandRow, j: usize) -> bool {
    if a.style(ra.style_at(a, i)) != b.style(rb.style_at(b, j)) {
        return false;
    }
    let (ka, kb) = (ra.kind_at(a, i), rb.kind_at(b, j));
    let number = |band: &Band, row: &BandRow, kind: u8, i: usize| match kind {
        NUMBER => band.number(row.payload_at(band, i)).to_bits(),
        _ => row.wide_at(band, i),
    };
    let text = |band: &Band, row: &BandRow, kind: u8, i: usize| match kind {
        TEXT => u64::from(row.payload_at(band, i)),
        _ => row.wide_at(band, i),
    };
    match (ka, kb) {
        (NUMBER | NUMBER_WIDE, NUMBER | NUMBER_WIDE) => number(a, ra, ka, i) == number(b, rb, kb, j),
        (TEXT | TEXT_WIDE, TEXT | TEXT_WIDE) => a.text(text(a, ra, ka, i)) == b.text(text(b, rb, kb, j)),
        // Arrays are compared by value where their column is known; here the position is enough.
        (ARRAY, ARRAY) => true,
        _ => ka == kb && ra.payload_at(a, i) == rb.payload_at(b, j),
    }
}

fn row_same_as(a: &Band, ra: &BandRow, b: &Band, rb: &BandRow) -> bool {
    if ra.first != rb.first || ra.bits(a) != rb.bits(b) || ra.cells != rb.cells {
        return false;
    }
    for i in 0..ra.cells as usize {
        if !same_value(a, ra, i, b, rb, i) {
            return false;
        }
    }
    let texts = |row: &BandRow| -> Vec<(u32, String)> { row.formulas().iter().map(|(c, f)| (*c, f.text.clone())).collect() };
    texts(ra) == texts(rb)
}

/// Bands built away from the store, by a reader decoding rows on several threads. The step that
/// packs rows — interning their numbers and texts, then the four arrays of a row — is most of what
/// reading costs (1.7 s of a 2.3 s decode on a 21M-cell sheet), and it cannot be shared out unless
/// the bands are built beside the store and spliced in afterwards.
#[derive(Debug, Default)]
pub struct BandBatch {
    bands: BTreeMap<u32, Band>,
}

impl BandBatch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.bands.is_empty()
    }

    /// Stores a whole row, exactly as [`CellStore::set_row`] would; `cells` is drained.
    pub fn set_row(&mut self, row: u32, cells: &mut Vec<(u32, Cell)>) {
        if cells.is_empty() {
            return;
        }
        cells.sort_unstable_by_key(|(c, _)| *c);
        cells.dedup_by(|a, b| {
            a.0 == b.0 && {
                b.1 = a.1.clone();
                true
            }
        });
        let band_row = row % BAND;
        let band = self.bands.entry(row / BAND).or_default();
        if band.row(band_row).is_none_or(BandRow::is_empty) {
            band.put_row(band_row, cells);
        } else {
            for (col, cell) in cells.drain(..) {
                band.put(band_row, col, cell);
            }
        }
        if band.rows.is_empty() {
            self.bands.remove(&(row / BAND));
        }
    }
}

/// One stored cell as its page holds it: no `Cell`, no `Value`, no allocation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Packed<'a> {
    Number(f64),
    Text(&'a str),
    Bool(bool),
    Error(CellError),
}

/// A `CellStore`'s packed bands, plus the count of stored cells.
#[derive(Debug, Default)]
pub struct CellStore {
    bands: BTreeMap<u32, Arc<Band>>,
    len: usize,
    /// The band cells were last added to. Cells arrive band by band as a file is read, so the band
    /// before it is complete and can stop carrying its number index. Not part of what a store is:
    /// two stores holding the same cells are equal whatever this says.
    building: Option<u32>,
}

impl Clone for CellStore {
    /// A copy shares the bands; where the next cell would go is not part of what a store is, so the
    /// copy starts afresh and the first row it takes settles it.
    fn clone(&self) -> Self {
        CellStore { bands: self.bands.clone(), len: self.len, building: None }
    }
}

impl PartialEq for CellStore {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len
            && self.bands.len() == other.bands.len()
            && self.bands.keys().eq(other.bands.keys())
            && self.bands.iter().all(|(k, a)| other.bands.get(k).is_some_and(|b| Arc::ptr_eq(a, b) || a.same_as(b)))
    }
}

/// Walks one packed row, in column order, without allocating.
struct PackedRow<'a> {
    band: &'a Band,
    row: u32,
    cells: &'a BandRow,
    col: u32,
    hi: u32,
    i: usize,
}

impl Iterator for PackedRow<'_> {
    type Item = (CellRef, Cell);
    fn next(&mut self) -> Option<(CellRef, Cell)> {
        while self.col <= self.hi && self.i < self.cells.cells as usize {
            let slot = self.col - self.cells.first;
            let word = (slot / 64) as usize;
            let w = *self.cells.bits(self.band).get(word)?;
            let masked = w & (!0u64 << (slot % 64));
            if masked == 0 {
                self.col = self.cells.first + (word as u32 + 1) * 64;
                continue;
            }
            let col = self.cells.first + word as u32 * 64 + masked.trailing_zeros();
            if col > self.hi {
                return None;
            }
            let cell = self.band.cell_at(self.cells, self.i, col);
            self.col = col + 1;
            self.i += 1;
            return Some((CellRef::new(self.row, col), cell));
        }
        None
    }
}

fn packed_row<'a>(band: &'a Band, row: u32, cells: &'a BandRow, c0: u32, c1: u32) -> PackedRow<'a> {
    let (lo, hi) = match cells.column_span(band) {
        Some(_) => (c0.max(cells.first), c1.min(cells.first.saturating_add(cells.columns()).saturating_sub(1))),
        None => (1, 0),
    };
    let col = lo.max(cells.first);
    let i = cells.rank_before(band, col);
    PackedRow { band, row, cells, col, hi, i }
}

impl CellStore {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Reads one cell. The value is rebuilt rather than borrowed: cells live in packed pages, so
    /// there is no `Cell` to point at. A text cell shares its string, so the copy is a pointer
    /// bump, not a string copy.
    pub fn get(&self, c: CellRef) -> Option<Cell> {
        let band = self.bands.get(&(c.row / BAND))?;
        let row = band.row(c.row % BAND)?;
        let i = row.index_of(band, c.col)?;
        Some(band.cell_at(row, i, c.col))
    }

    /// True when the position holds a cell at all (a stored style counts).
    pub fn contains(&self, c: CellRef) -> bool {
        self.bands.get(&(c.row / BAND)).and_then(|b| b.row(c.row % BAND).map(|r| (b, r))).is_some_and(|(b, r)| r.index_of(b, c.col).is_some())
    }

    /// Stores a cell; a default (empty, unstyled) cell is removed instead.
    pub fn set(&mut self, c: CellRef, cell: Cell) {
        let key = c.row / BAND;
        let row = c.row % BAND;
        let had = self.contains(c);
        let band = Arc::make_mut(self.bands.entry(key).or_default());
        band.put(row, c.col, cell);
        match (had, self.contains(c)) {
            (false, true) => self.len += 1,
            (true, false) => self.len -= 1,
            _ => {}
        }
        if self.bands.get(&key).is_some_and(|b| b.rows.is_empty()) {
            self.bands.remove(&key);
        }
    }

    /// Adds bands a [`BandBatch`] built off this thread. Whole bands are moved, not cells: packing
    /// a row needs its band's tables, so a reader that decodes rows on several threads builds them
    /// there and hands them over here. A band the store already holds is replaced.
    pub fn insert_batch(&mut self, batch: BandBatch) {
        let count = |band: &Band| band.rows.iter().map(|row| row.cells as usize).sum::<usize>();
        for (key, mut band) in batch.bands {
            // A batch is complete: give back what building it over-allocated.
            band.finish();
            let Some(existing) = self.bands.get_mut(&key) else {
                self.len += count(&band);
                self.bands.insert(key, Arc::new(band));
                continue;
            };
            // A band both sides hold: a chunk boundary fell inside it, since chunks are cut at row
            // boundaries and a band is 64 rows. The batch's rows point into its own number and text
            // tables, so they are rebuilt here rather than moved.
            let rows = std::mem::take(&mut band.rows);
            let target = Arc::make_mut(existing);
            for row in rows {
                let before = target.row(row.key).map_or(0, |r| r.cells as usize);
                let mut cells: Vec<(u32, Cell)> = (0..row.cells as usize)
                    .map(|i| {
                        let col = row.col_at(&band, i);
                        (col, band.cell_at(&row, i, col))
                    })
                    .collect();
                if before == 0 && target.row(row.key).is_none_or(BandRow::is_empty) {
                    target.put_row(row.key, &mut cells);
                } else {
                    for (col, cell) in cells {
                        target.put(row.key, col, cell);
                    }
                }
                self.len += target.row(row.key).map_or(0, |r| r.cells as usize) - before;
            }
        }
        if self.building.is_none() {
            self.building = self.bands.keys().next_back().copied();
        }
    }

    /// Stores a whole row at once: one band lookup and one row lookup instead of one per cell.
    /// Whatever the row already holds stays; the cells given win. `cells` is drained so a reader
    /// can hand in the same buffer for every row.
    pub fn set_row(&mut self, row: u32, cells: &mut Vec<(u32, Cell)>) {
        if cells.is_empty() {
            return;
        }
        cells.sort_unstable_by_key(|(c, _)| *c);
        cells.dedup_by(|a, b| {
            a.0 == b.0 && {
                b.1 = a.1.clone();
                true
            }
        });
        let key = row / BAND;
        let band_row = row % BAND;
        if self.building != Some(key) {
            // The band this replaces is finished; give back its index, if nobody shares it.
            if let Some(done) = self.building.replace(key)
                && let Some(band) = self.bands.get_mut(&done)
                && let Some(band) = Arc::get_mut(band)
            {
                band.finish();
            }
        }
        let band = Arc::make_mut(self.bands.entry(key).or_default());
        // A row that isn't there yet — every row while a file opens — is built in one go: the
        // stripes are extended once from the cells instead of grown cell by cell.
        if band.row(band_row).is_none_or(BandRow::is_empty) {
            let before = band.row(band_row).map_or(0, |r| r.cells as usize);
            band.put_row(band_row, cells);
            let after = band.row(band_row).map_or(0, |r| r.cells as usize);
            self.len = self.len + after - before;
        } else {
            for (col, cell) in cells.drain(..) {
                let had = band.row(band_row).is_some_and(|r| r.index_of(band, col).is_some());
                band.put(band_row, col, cell);
                let has = band.row(band_row).is_some_and(|r| r.index_of(band, col).is_some());
                match (had, has) {
                    (false, true) => self.len += 1,
                    (true, false) => self.len -= 1,
                    _ => {}
                }
            }
        }
        if band.rows.is_empty() {
            self.bands.remove(&key);
        }
    }

    /// Puts every cell in the band its row says and recounts the length — serialized band keys
    /// and the cached count are derived state, and the ones a file carries are not trusted. Cells
    /// outside the sheet grid are dropped; returns how many (#1008).
    pub fn rebuild(&mut self) -> usize {
        let mut out = CellStore::new();
        let mut dropped = 0;
        let cells: Vec<(CellRef, Cell)> = self.iter().collect();
        for (at, cell) in cells {
            if at.is_valid() {
                out.set(at, cell);
            } else {
                dropped += 1;
            }
        }
        *self = out;
        dropped
    }

    pub fn remove(&mut self, c: CellRef) -> Option<Cell> {
        let key = c.row / BAND;
        let band = self.bands.get_mut(&key)?;
        let cell = Arc::make_mut(band).take(c.row % BAND, c.col)?;
        self.len -= 1;
        if self.bands.get(&key).is_some_and(|b| b.rows.is_empty()) {
            self.bands.remove(&key);
        }
        Some(cell)
    }

    /// The cells of `r` that carry a formula, in row-major order.
    pub fn formula_cells(&self, r: RangeRef) -> impl Iterator<Item = CellRef> + '_ {
        self.bands.range(r.start.row / BAND..=r.end.row / BAND).flat_map(move |(bk, band)| {
            band.rows.iter().flat_map(move |row| {
                let at_row = bk * BAND + row.key;
                row.formulas()
                    .iter()
                    .filter(move |(c, _)| at_row >= r.start.row && at_row <= r.end.row && *c >= r.start.col && *c <= r.end.col)
                    .map(move |(c, _)| CellRef::new(at_row, *c))
            })
        })
    }

    /// Whether the block holds a cell whose value the store cannot give: a formula (it holds the
    /// value the file cached) or an array. Cheap next to reading the block: one check a row.
    pub fn has_owned_cells(&self, r: RangeRef) -> bool {
        self.bands.range(r.start.row / BAND..=r.end.row / BAND).any(|(bk, band)| {
            band.rows.iter().any(|row| {
                let at_row = bk * BAND + row.key;
                at_row >= r.start.row
                    && at_row <= r.end.row
                    && row.extras.as_ref().is_some_and(|e| {
                        e.formulas.iter().any(|(c, _)| (r.start.col..=r.end.col).contains(c))
                            || e.arrays.iter().any(|(c, _)| (r.start.col..=r.end.col).contains(c))
                    })
            })
        })
    }

    /// Calls `f` on each stored cell of `r`, row-major, handing the value as the page holds it, and
    /// `None` for a cell whose value the evaluator owns (a formula, an array, or a blank one).
    ///
    /// Reading a range through `iter_range` builds a `Cell` an iteration — a `Value`, a style, and
    /// two `Arc` bumps — which costs ~77 ns a cell against ~2 ns for the numbers themselves. Summing
    /// a million-cell column spent all of its 154 ms there.
    pub fn visit_packed(&self, r: RangeRef, f: &mut dyn FnMut(CellRef, Option<Packed<'_>>) -> bool) -> bool {
        let (r0, r1) = (r.start.row, r.end.row);
        for (bk, band) in self.bands.range(r0 / BAND..=r1 / BAND) {
            for row in &band.rows {
                let at_row = bk * BAND + row.key;
                if at_row < r0 || at_row > r1 {
                    continue;
                }
                // Start at the first stored column of the range and stop past its last, so a
                // narrow read of a wide row costs the cells it asks for, not the row's width.
                let lo = r.start.col.max(row.first);
                let hi = r.end.col.min(row.first.saturating_add(row.columns()).saturating_sub(1));
                if lo > hi {
                    continue;
                }
                let start_slot = (lo - row.first) as usize;
                // A cell that carries a formula stores the value the file cached, which is not
                // its value now: the evaluator owns it, so hand it over as `None`.
                let formulas = row.extras.as_ref().map(|e| e.formulas.as_slice()).filter(|f| !f.is_empty());
                let mut i = row.rank_before(band, lo);
                let mut kinds = RunCursor::new(row.kind_runs(band), u16::from(EMPTY));
                'words: for (word, bits) in row.bits(band).iter().enumerate().skip(start_slot / 64) {
                    let mut w = *bits;
                    if word == start_slot / 64 {
                        w &= !((1u64 << (start_slot % 64)) - 1);
                    }
                    while w != 0 {
                        let bit = w.trailing_zeros();
                        w &= w - 1;
                        let index = i;
                        i += 1;
                        let col = row.first + word as u32 * 64 + bit;
                        if col > hi {
                            // Past this row's last wanted column; later rows may still be inside.
                            break 'words;
                        }
                        let has_formula = formulas.is_some_and(|f| f.binary_search_by_key(&col, |(c, _)| *c).is_ok());
                        let payload = row.payload_at(band, index);
                        let packed = if has_formula {
                            None
                        } else {
                            match kinds.at(index) as u8 {
                                // An empty slot is not a stored value (no value, a style at most),
                                // and `iter_range` does not hand one out either.
                                EMPTY => continue,
                                NUMBER => Some(Packed::Number(band.number(payload))),
                                NUMBER_WIDE => Some(Packed::Number(f64::from_bits(row.wide_at(band, index)))),
                                TEXT => band.texts.get(payload as usize).map(|t| Packed::Text(t)),
                                TEXT_WIDE => band.texts.get(row.wide_at(band, index) as usize).map(|t| Packed::Text(t)),
                                BOOL => Some(Packed::Bool(payload != 0)),
                                ERROR => Some(Packed::Error(*ERRORS.get(payload as usize).unwrap_or(&CellError::Value))),
                                _ => None,
                            }
                        };
                        if !f(CellRef::new(at_row, col), packed) {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }

    pub fn iter_range(&self, r: RangeRef) -> impl Iterator<Item = (CellRef, Cell)> + '_ {
        let (r0, r1) = (r.start.row, r.end.row);
        self.bands.range(r0 / BAND..=r1 / BAND).flat_map(move |(bk, band)| {
            band.rows
                .iter()
                .filter_map(move |cells| {
                    let row = bk * BAND + cells.key;
                    (row >= r0 && row <= r1).then(|| packed_row(band, row, cells, r.start.col, r.end.col))
                })
                .flatten()
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = (CellRef, Cell)> + '_ {
        self.bands
            .iter()
            .flat_map(move |(bk, band)| band.rows.iter().flat_map(move |cells| packed_row(band, bk * BAND + cells.key, cells, 0, u32::MAX)))
    }

    pub fn row(&self, row: u32, c0: u32, c1: u32) -> impl Iterator<Item = (u32, Cell)> + '_ {
        let packed = self.bands.get(&(row / BAND)).and_then(|b| b.row(row % BAND).map(|cells| packed_row(b, row, cells, c0, c1)));
        packed.into_iter().flatten().map(|(at, cell)| (at.col, cell))
    }

    pub fn used_range(&self) -> Option<RangeRef> {
        let mut r0 = u32::MAX;
        let mut c0 = u32::MAX;
        let mut r1 = 0u32;
        let mut c1 = 0u32;
        for (bk, band) in &self.bands {
            for cells in &band.rows {
                let Some((first, last)) = cells.column_span(band) else { continue };
                let at = bk * BAND + cells.key;
                r0 = r0.min(at);
                r1 = r1.max(at);
                c0 = c0.min(first);
                c1 = c1.max(last);
            }
        }
        if c0 == u32::MAX {
            return None;
        }
        Some(RangeRef::new(CellRef::new(r0, c0), CellRef::new(r1, c1)))
    }

    /// Every formula in the store with the position it sits at and whether a cached value came
    /// with it. Iterates the per-row side tables rather than the cells: a sheet can hold a hundred
    /// million cells and a few thousand formulas, and scanning the cells to find them costs more
    /// than opening the file.
    pub fn formulas(&self) -> impl Iterator<Item = (CellRef, &Arc<Formula>, bool)> + '_ {
        self.bands.iter().flat_map(|(bk, band)| {
            band.rows.iter().flat_map(move |cells| {
                cells.formulas().iter().map(move |(col, f)| {
                    let has_value = cells.index_of(band, *col).is_some_and(|i| cells.kind_at(band, i) != EMPTY);
                    (CellRef::new(bk * BAND + cells.key, *col), f, has_value)
                })
            })
        })
    }

    /// A stored cell with a value or a formula (a style alone does not count).
    pub fn has(&self, c: CellRef) -> bool {
        self.bands.get(&(c.row / BAND)).and_then(|b| b.row(c.row % BAND).map(|row| (b, row))).is_some_and(|(b, row)| row_has_value(b, row, c.col))
    }

    /// Last row (inclusive, capped at `cap`) of the maximal run of contiguous rows at or after
    /// `from.row` that store a cell in `from.col` with a non-empty value. See `Sheet::value_ref`.
    pub fn contiguous_value_run(&self, from: CellRef, cap: u32) -> Option<u32> {
        let mut last = None;
        let mut expect = from.row;
        while expect <= cap {
            let band_key = expect / BAND;
            let Some(band) = self.bands.get(&band_key) else { break };
            let mut advanced = false;
            let start = expect % BAND;
            let first = band.rows.partition_point(|r| r.key < start);
            for row in &band.rows[first..] {
                let at = band_key * BAND + row.key;
                if at != expect || at > cap {
                    break;
                }
                if !row.value_nonempty(band, from.col) {
                    return last;
                }
                last = Some(expect);
                expect += 1;
                advanced = true;
            }
            if !advanced {
                break;
            }
        }
        last
    }

    /// Positions whose cells differ between `self` and `other`.
    pub fn diff(&self, other: &CellStore) -> Vec<CellRef> {
        let mut keys: Vec<u32> = self.bands.keys().chain(other.bands.keys()).copied().collect();
        keys.sort_unstable();
        keys.dedup();
        let mut out = Vec::new();
        for k in keys {
            let (a, b) = (self.bands.get(&k), other.bands.get(&k));
            if let (Some(a), Some(b)) = (a, b)
                && Arc::ptr_eq(a, b)
            {
                continue;
            }
            let band = k * BAND;
            let mut rows: Vec<u32> = Vec::new();
            if let Some(a) = a {
                rows.extend(a.rows.iter().map(|r| r.key));
            }
            if let Some(b) = b {
                rows.extend(b.rows.iter().map(|r| r.key));
            }
            rows.sort_unstable();
            rows.dedup();
            for row in rows {
                let (ra, rb) = (a.and_then(|x| x.row(row)), b.and_then(|x| x.row(row)));
                let mut cols: Vec<u32> = Vec::new();
                if let (Some(a), Some(ra)) = (a, ra) {
                    cols.extend(packed_row(a, band + row, ra, 0, u32::MAX).map(|(c, _)| c.col));
                }
                if let (Some(b), Some(rb)) = (b, rb) {
                    cols.extend(packed_row(b, band + row, rb, 0, u32::MAX).map(|(c, _)| c.col));
                }
                cols.sort_unstable();
                cols.dedup();
                for col in cols {
                    let x = a.and_then(|x| ra.and_then(|r| r.index_of(x, col).map(|i| (x, r, i))));
                    let y = b.and_then(|y| rb.and_then(|r| r.index_of(y, col).map(|i| (y, r, i))));
                    let same = match (x, y) {
                        (None, None) => true,
                        (Some((xb, xr, xi)), Some((yb, yr, yi))) => {
                            let arrays = |row: &BandRow| row.arrays().binary_search_by_key(&col, |(c, _)| *c).ok().map(|k| row.arrays()[k].1.clone());
                            same_value(xb, xr, xi, yb, yr, yi)
                                && (xr.kind_at(xb, xi) != ARRAY || arrays(xr) == arrays(yr))
                                && xr.formula_at(col).map(|f| f.text.clone()) == yr.formula_at(col).map(|f| f.text.clone())
                        }
                        _ => false,
                    };
                    if !same {
                        out.push(CellRef::new(band + row, col));
                    }
                }
            }
        }
        out
    }

    /// Rough heap storage kept alive by this snapshot's bands that `newer` no longer shares.
    pub fn retained_bytes_against(&self, newer: &CellStore) -> usize {
        let mut bytes = 0usize;
        for (key, band) in &self.bands {
            if newer.bands.get(key).is_some_and(|b| Arc::ptr_eq(b, band)) {
                continue;
            }
            bytes += std::mem::size_of::<Band>();
            bytes += band.bits.capacity() * 8;
            bytes += band.kinds.capacity() * 4;
            bytes += band.payloads.capacity() * 2;
            bytes += band.styles.capacity() * 4;
            for row in &band.rows {
                bytes += std::mem::size_of::<BandRow>() + row.formulas().len() * (4 + std::mem::size_of::<Formula>()) + row.arrays().len() * 8;
            }
            bytes += band.numbers.capacity() * 8 + band.number_slots.capacity() * 2;
            for t in &band.texts {
                bytes += t.len() + std::mem::size_of::<Arc<str>>();
            }
            for f in band.rows.iter().flat_map(|r| r.formulas().iter().map(|(_, f)| f)) {
                bytes += f.text.len();
                if let Some(expr) = &f.expr {
                    expr.walk(&mut |_| bytes += std::mem::size_of::<gridcraft_formula::Expr>());
                }
            }
        }
        bytes
    }

    /// Bytes the packing itself holds, split by what holds them. Diagnostics for the memory work,
    /// not an exact figure: allocator rounding is not visible from here.
    pub fn packed_breakdown(&self) -> [usize; 8] {
        use std::mem::size_of;
        // rows, bits, kinds, payloads, styles, texts, formulas+arrays, bands
        let mut out = [0usize; 8];
        for band in self.bands.values() {
            out[7] += size_of::<Band>() + band.texts.capacity() * size_of::<Arc<str>>() + band.style_ids.capacity() * size_of::<StyleId>();
            out[7] += band.text_index.capacity() * size_of::<(usize, u32)>();
            out[7] += band.numbers.capacity() * size_of::<f64>() + band.number_slots.capacity() * size_of::<u16>();
            out[6] += band.style_ids.capacity() * size_of::<StyleId>();
            out[5] += band.texts.capacity() * size_of::<Arc<str>>();
            out[0] += band.rows.capacity() * size_of::<BandRow>();
            out[1] += band.bits.capacity() * size_of::<u64>();
            out[2] += band.kinds.capacity() * size_of::<(u16, u16)>();
            out[3] += band.payloads.capacity() * size_of::<u16>();
            out[4] += band.styles.capacity() * size_of::<(u16, u16)>();
            for row in &band.rows {
                out[6] += row.extras.as_ref().map_or(0, |e| e.formulas.capacity() * (4 + 8) + e.arrays.capacity() * (4 + 8));
            }
        }
        out
    }

    /// Bytes the packing itself holds (see [`packed_breakdown`](Self::packed_breakdown)).
    pub fn packed_bytes(&self) -> usize {
        self.packed_breakdown().iter().sum()
    }

    /// Removes every cell in `r` and returns them.
    pub fn take_range(&mut self, r: RangeRef) -> Vec<(CellRef, Cell)> {
        let keys: Vec<CellRef> = self.iter_range(r).map(|(c, _)| c).collect();
        keys.into_iter().filter_map(|c| self.remove(c).map(|cell| (c, cell))).collect()
    }

    /// Rows at or after `from` moved by `delta` (positive = down).
    pub fn shift_rows(&mut self, from: u32, delta: i64) {
        let moved = self.take_pred(|c| c.row >= from);
        for (c, cell) in moved {
            let nr = c.row as i64 + delta;
            if (0..gridcraft_core::MAX_ROWS as i64).contains(&nr) {
                self.set(CellRef::new(nr as u32, c.col), cell);
            }
        }
    }

    pub fn shift_cols(&mut self, from: u32, delta: i64) {
        let moved = self.take_pred(|c| c.col >= from);
        for (c, cell) in moved {
            let nc = c.col as i64 + delta;
            if (0..gridcraft_core::MAX_COLS as i64).contains(&nc) {
                self.set(CellRef::new(c.row, nc as u32), cell);
            }
        }
    }

    /// Shifts cells within rows `r0..=r1` and columns `>= from` horizontally.
    pub fn shift_cols_in_rows(&mut self, r0: u32, r1: u32, from: u32, delta: i64) {
        let moved = self.take_pred(|c| (r0..=r1).contains(&c.row) && c.col >= from);
        for (c, cell) in moved {
            let nc = c.col as i64 + delta;
            if (0..gridcraft_core::MAX_COLS as i64).contains(&nc) {
                self.set(CellRef::new(c.row, nc as u32), cell);
            }
        }
    }

    /// Shifts cells within columns `c0..=c1` and rows `>= from` vertically.
    pub fn shift_rows_in_cols(&mut self, c0: u32, c1: u32, from: u32, delta: i64) {
        let moved = self.take_pred(|c| c.row >= from && (c0..=c1).contains(&c.col));
        for (c, cell) in moved {
            let nr = c.row as i64 + delta;
            if (0..gridcraft_core::MAX_ROWS as i64).contains(&nr) {
                self.set(CellRef::new(nr as u32, c.col), cell);
            }
        }
    }

    fn take_pred(&mut self, keep: impl Fn(CellRef) -> bool) -> Vec<(CellRef, Cell)> {
        let keys: Vec<CellRef> = self.iter().filter(|(c, _)| keep(*c)).map(|(c, _)| c).collect();
        keys.into_iter().filter_map(|c| self.remove(c).map(|cell| (c, cell))).collect()
    }
}

fn row_has_value(band: &Band, row: &BandRow, col: u32) -> bool {
    match row.index_of(band, col) {
        Some(i) => row.kind_at(band, i) != EMPTY || row.formula_at(col).is_some(),
        None => false,
    }
}

impl BandRow {
    fn value_nonempty(&self, band: &Band, col: u32) -> bool {
        match self.index_of(band, col) {
            Some(i) => match self.kind_at(band, i) {
                EMPTY => false,
                TEXT => !band.text(u64::from(self.payload_at(band, i))).is_empty(),
                TEXT_WIDE => !band.text(self.wide_at(band, i)).is_empty(),
                _ => true,
            },
            None => false,
        }
    }
}

// Serialization keeps the shape the model always wrote — bands of rows of columns of `Cell` — so a
// workbook saved before the packing still loads, and one saved now stays readable by anything that
// reads the old shape.

#[derive(Serialize, Deserialize)]
struct WireBand {
    #[serde(default)]
    rows: BTreeMap<u32, BTreeMap<u32, Cell>>,
}

impl Serialize for CellStore {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut wire: BTreeMap<u32, WireBand> = BTreeMap::new();
        for (at, cell) in self.iter() {
            wire.entry(at.row / BAND).or_insert_with(|| WireBand { rows: BTreeMap::new() });
            wire.get_mut(&(at.row / BAND)).and_then(|b| b.rows.entry(at.row).or_default().insert(at.col, cell));
        }
        let mut st = s.serialize_struct("CellStore", 2)?;
        st.serialize_field("bands", &wire)?;
        st.serialize_field("len", &self.len)?;
        st.end()
    }
}

impl<'de> Deserialize<'de> for CellStore {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            #[serde(default)]
            bands: BTreeMap<u32, WireBand>,
            #[serde(default)]
            len: usize,
        }
        let wire = Wire::deserialize(d)?;
        let mut store = CellStore::new();
        // Band keys are derived state: put each row where its number says, and recount.
        for band in wire.bands.values() {
            for (row, cols) in &band.rows {
                let mut cells: Vec<(u32, Cell)> = cols.iter().map(|(col, cell)| (*col, cell.clone())).collect();
                store.set_row(*row, &mut cells);
            }
        }
        store.len = store.iter().count().max(store.len);
        let _ = wire.len;
        Ok(store)
    }
}
#[cfg(test)]
mod packed_tests {
    use super::*;
    use crate::Cell;
    use gridcraft_core::{CellRef, RangeRef, Value};

    fn plain(n: u32) -> Cell {
        Cell::value(Value::Number(n as f64))
    }

    /// One row per width, one band boundary, holes in both directions.
    fn store() -> CellStore {
        let mut s = CellStore::new();
        for (r, c, v) in
            [(0u32, 0u32, 1u32), (0, 1, 2), (0, 70, 3), (1, 0, 4), (1, 64, 5), (5, 3, 6), (63, 70, 7), (64, 0, 8), (64, 70, 9), (200, 1, 10)]
        {
            s.set(CellRef::new(r, c), plain(v));
        }
        s
    }

    /// A dense row read through a narrow window: the kind and payload indexes must not slip.
    #[test]
    fn packed_walk_over_a_dense_row() {
        let mut s = CellStore::new();
        for c in 0..200u32 {
            s.set(CellRef::new(3, c), plain(10_000 + c));
        }
        for (c0, c1) in [(0u32, 0u32), (26, 26), (26, 30), (63, 64), (64, 70), (127, 200), (199, 199)] {
            let range = RangeRef::new(CellRef::new(3, c0), CellRef::new(3, c1));
            let want: Vec<(CellRef, f64)> = s.iter_range(range).filter_map(|(at, c)| c.value.as_f64().map(|n| (at, n))).collect();
            let mut got = Vec::new();
            s.visit_packed(range, &mut |at, packed| {
                if let Some(Packed::Number(n)) = packed {
                    got.push((at, n));
                }
                true
            });
            assert_eq!(got, want, "range {}", range.a1());
        }
    }

    /// A row built through `BandBatch` may hold a slot with a style and no value: the packed walk
    /// must skip it exactly as `iter_range` does. The report that found this had `SUM(AA74:AA81)`
    /// answer 83,551.75 instead of 43,045.08 because the slot stopped the walk midway and the
    /// caller folded the block a second time.
    #[test]
    fn packed_walk_skips_a_slot_with_no_value() {
        let mut batch = BandBatch::new();
        let mut cells = vec![(3u32, plain(1)), (4, Cell { value: Value::Empty, formula: None, style: StyleId(0) }), (5, plain(3))];
        batch.set_row(9, &mut cells);
        let mut s = CellStore::new();
        s.insert_batch(batch);
        let range = RangeRef::new(CellRef::new(9, 0), CellRef::new(9, 10));
        let want: Vec<(CellRef, f64)> = s.iter_range(range).filter_map(|(at, c)| c.value.as_f64().map(|n| (at, n))).collect();
        let mut got = Vec::new();
        let mut needs_host = false;
        s.visit_packed(range, &mut |at, packed| {
            match packed {
                Some(Packed::Number(n)) => got.push((at, n)),
                None => needs_host = true,
                other => panic!("{} came out as {other:?}", at.a1()),
            }
            true
        });
        assert_eq!(got, want);
        assert!(!needs_host, "a value-less slot must not look like a formula");
    }

    /// The packed walk must hand out exactly the cells `iter_range` does, in the same order.
    #[test]
    fn packed_walk_matches_iter_range() {
        let s = store();
        for (r0, r1, c0, c1) in [
            (0u32, 300u32, 0u32, 100u32),
            (0, 0, 0, 0),
            (0, 0, 1, 1),
            (0, 0, 2, 69),
            (0, 0, 70, 70),
            (0, 1, 0, 200),
            (0, 63, 70, 70),
            (1, 1, 64, 64),
            (63, 64, 0, 0),
            (64, 200, 0, 70),
            (201, 400, 0, 200),
            (0, 400, 71, 200),
        ] {
            let range = RangeRef::new(CellRef::new(r0, c0), CellRef::new(r1, c1));
            let want: Vec<(CellRef, f64)> = s.iter_range(range).filter_map(|(at, cell)| cell.value.as_f64().map(|n| (at, n))).collect();
            let mut got: Vec<(CellRef, f64)> = Vec::new();
            let done = s.visit_packed(range, &mut |at, packed| {
                match packed {
                    Some(Packed::Number(n)) => got.push((at, n)),
                    other => panic!("{} came out as {other:?}", at.a1()),
                }
                true
            });
            assert!(done);
            assert_eq!(got, want, "range {}", range.a1());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gridcraft_core::Value;

    fn c(r: u32, col: u32) -> CellRef {
        CellRef::new(r, col)
    }

    #[test]
    fn runs_match_a_plain_vector() {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = |n: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % n.max(1)) as usize
        };
        let mut runs: Vec<(u16, u16)> = Vec::new();
        let mut plain: Vec<u16> = Vec::new();
        for step in 0..4_000 {
            let op = next(4);
            let value = next(3) as u16;
            // One row, so the slice is the whole stripe.
            let slice = runs.len();
            let mut did = String::new();
            match op {
                0 => {
                    run_push(&mut runs, 0, value);
                    plain.push(value);
                }
                1 => {
                    let i = next(plain.len() as u64 + 1);
                    let new = runs_insert(&mut runs, 0, slice, i, value);
                    assert_eq!(new, runs.len());
                    plain.insert(i, value);
                    did = format!("insert({i}, {value})");
                }
                2 if !plain.is_empty() => {
                    let i = next(plain.len() as u64);
                    let new = runs_remove(&mut runs, 0, slice, i);
                    assert_eq!(new, runs.len());
                    plain.remove(i);
                    did = format!("remove({i})");
                }
                3 if !plain.is_empty() => {
                    let i = next(plain.len() as u64);
                    let new = runs_set(&mut runs, 0, slice, i, value);
                    assert_eq!(new, runs.len());
                    plain[i] = value;
                    did = format!("set({i}, {value})");
                }
                _ => {}
            }
            assert_eq!(runs_len(&runs), plain.len(), "step {step}: length");
            if step % 25 == 0 {
                for (i, v) in plain.iter().enumerate() {
                    assert_eq!(runs_at(&runs, i), *v, "step {step}: at {i} of {plain:?}");
                }
            }
            assert!(runs.windows(2).all(|w| w[0].0 != w[1].0), "step {step} {did}: adjacent runs hold the same value: {runs:?} plain {plain:?}");
        }
    }

    /// The runs of two rows share one stripe: a row must never merge into its neighbour's run, and
    /// editing one row must leave the other's slice alone.
    #[test]
    fn runs_stay_inside_their_slice() {
        let mut runs: Vec<(u16, u16)> = Vec::new();
        run_push(&mut runs, 0, 7);
        run_push(&mut runs, 0, 7);
        let second = runs.len();
        run_push(&mut runs, second, 7);
        assert_eq!(runs, [(7, 2), (7, 1)], "a new row starts its own run");
        // Two rows share one stripe: the first holds [3, 3, 4], the second [3, 3, 3].
        let mut stripe: Vec<(u16, u16)> = vec![(3, 2), (4, 1), (3, 3)];
        let first = runs_set(&mut stripe, 0, 2, 1, 4);
        assert_eq!(first, 2);
        assert_eq!(stripe, [(3, 1), (4, 2), (3, 3)], "the second row's slice is untouched");
        assert_eq!(runs_len(&stripe[..2]), 3);
        assert_eq!(runs_len(&stripe[2..]), 3);
        // The second row now starts where the first one ends, and its runs merge with each other
        // but never with the run the first row left beside them.
        let second = runs_set(&mut stripe, 2, 1, 1, 4);
        assert_eq!(second, 3);
        assert_eq!(stripe, [(3, 1), (4, 2), (3, 1), (4, 1), (3, 1)]);
    }

    #[test]
    fn set_get_remove() {
        let mut s = CellStore::new();
        s.set(c(0, 0), Cell::value(1.0.into()));
        s.set(c(100, 5), Cell::value("x".into()));
        assert_eq!(s.len(), 2);
        assert_eq!(s.get(c(100, 5)).map(|x| x.value.clone()), Some(Value::from("x")));
        assert_eq!(s.used_range().unwrap().a1(), "A1:F101");
        s.set(c(0, 0), Cell::default());
        assert_eq!(s.len(), 1);
        assert!(s.remove(c(100, 5)).is_some());
        assert!(s.is_empty());
        assert!(s.used_range().is_none());
    }

    #[test]
    fn cow_snapshots() {
        let mut s = CellStore::new();
        for r in 0..1000 {
            s.set(c(r, 0), Cell::value((r as f64).into()));
        }
        let snap = s.clone();
        s.set(c(5, 0), Cell::value(99.0.into()));
        assert_eq!(snap.get(c(5, 0)).unwrap().value, Value::Number(5.0));
        assert_eq!(s.get(c(5, 0)).unwrap().value, Value::Number(99.0));
    }

    #[test]
    fn diff_visits_changed_bands() {
        let mut s = CellStore::new();
        for r in 0..1000 {
            s.set(c(r, 0), Cell::value((r as f64).into()));
        }
        let snap = s.clone();
        assert!(s.diff(&snap).is_empty());
        s.set(c(5, 0), Cell::value(99.0.into()));
        s.set(c(700, 3), Cell::value("new".into()));
        s.remove(c(900, 0));
        assert_eq!(snap.diff(&s), [c(5, 0), c(700, 3), c(900, 0)]);
    }

    #[test]
    fn range_iteration_order() {
        let mut s = CellStore::new();
        s.set(c(70, 2), Cell::value(3.0.into()));
        s.set(c(1, 1), Cell::value(1.0.into()));
        s.set(c(1, 3), Cell::value(2.0.into()));
        s.set(c(500, 0), Cell::value(4.0.into()));
        let v: Vec<f64> = s.iter_range(RangeRef::parse("A1:D100").unwrap()).map(|(_, x)| x.value.as_f64().unwrap()).collect();
        assert_eq!(v, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn a_band_that_overflows_its_number_table_still_reads_back() {
        // 64 rows of 2,000 distinct numbers is 128,000 values in one band — twice what the
        // two-byte payload can index, so the tail of it has to go through the wide table.
        let value = |row: u32, col: u32| row as f64 * 2_000.0 + col as f64 + 0.5;
        let mut s = CellStore::new();
        for row in 0..64u32 {
            let mut cells: Vec<(u32, Cell)> = (0..2_000u32).map(|col| (col, Cell::value(value(row, col).into()))).collect();
            s.set_row(row, &mut cells);
        }
        assert_eq!(s.len(), 128_000);
        // The band really did overflow: some cells index the number table, the rest are wide.
        let band = s.bands.get(&0).expect("band");
        let kinds: Vec<u8> = band.rows.iter().flat_map(|row| (0..row.cells as usize).map(|i| row.kind_at(band, i))).collect();
        assert_eq!(kinds.iter().filter(|k| **k == NUMBER).count(), 65_535, "cells the table could hold");
        assert_eq!(kinds.iter().filter(|k| **k == NUMBER_WIDE).count(), 128_000 - 65_535, "cells that fell back");
        for row in 0..64u32 {
            for col in 0..2_000u32 {
                let got = s.get(c(row, col)).map(|cell| cell.value);
                assert_eq!(got, Some(Value::Number(value(row, col))), "row {row} col {col}");
            }
        }
        // An edit into the overflowed rows keeps working: overwrite, insert a new column, remove.
        s.set(c(63, 1_999), Cell::value((-1.5).into()));
        assert_eq!(s.get(c(63, 1_999)).map(|cell| cell.value), Some(Value::Number(-1.5)));
        s.set(c(63, 2_500), Cell::value(7.25.into()));
        assert_eq!(s.get(c(63, 2_500)).map(|cell| cell.value), Some(Value::Number(7.25)));
        assert_eq!(s.remove(c(0, 1_500)).map(|cell| cell.value), Some(Value::Number(value(0, 1_500))));
        assert_eq!(s.len(), 128_000);
        // Text in the same band, past the numbers, is still found.
        s.set(c(0, 3_000), Cell::value("tail".into()));
        assert_eq!(s.get(c(0, 3_000)).map(|cell| cell.value), Some(Value::Text("tail".into())));
    }

    #[test]
    fn shifting() {
        let mut s = CellStore::new();
        s.set(c(2, 0), Cell::value(1.0.into()));
        s.set(c(5, 0), Cell::value(2.0.into()));
        s.shift_rows(3, 2);
        assert!(s.get(c(7, 0)).is_some());
        assert!(s.get(c(2, 0)).is_some());
        s.shift_cols(0, 1);
        assert!(s.get(c(7, 1)).is_some());
    }

    /// A reader cuts its work at row boundaries, so two chunks can land in the same band: the
    /// second batch is rebuilt against the band the store already holds.
    #[test]
    fn a_band_split_across_two_batches_keeps_both_halves() {
        let mut s = CellStore::new();
        for (from, to) in [(0u32, 40u32), (40, 80)] {
            let mut batch = BandBatch::new();
            for row in from..to {
                let mut cells: Vec<(u32, Cell)> = (0..10u32).map(|c| (c, Cell::value(Value::Number((row * 100 + c) as f64)))).collect();
                cells.push((20, Cell::value(Value::Text(Arc::from("shared")))));
                batch.set_row(row, &mut cells);
            }
            s.insert_batch(batch);
        }
        assert_eq!(s.len(), 80 * 11);
        assert_eq!(s.used_range().map(|r| r.a1()), Some("A1:U80".to_string()));
        for row in 0..80u32 {
            for col in 0..10u32 {
                assert_eq!(s.get(c(row, col)).map(|x| x.value), Some(Value::Number((row * 100 + col) as f64)), "{row}:{col}");
            }
            assert_eq!(s.get(c(row, 20)).map(|x| x.value), Some(Value::Text(Arc::from("shared"))), "{row}:U");
        }
        check_stripes(&s);
    }

    /// The stripes are one vector each, so a bug shows up as a row whose slice points into the
    /// middle of another's. Every band must account for every entry, exactly once.
    fn check_stripes(s: &CellStore) {
        for (bk, band) in &s.bands {
            let ranges = |name: &str, len: usize, f: &dyn Fn(&BandRow) -> (u32, u32)| {
                let mut v: Vec<(usize, usize)> = band
                    .rows
                    .iter()
                    .map(|r| {
                        let (at, n) = f(r);
                        (at as usize, n as usize)
                    })
                    .collect();
                v.sort_unstable();
                let mut end = 0;
                for (at, n) in &v {
                    assert_eq!(*at, end, "band {bk} {name}: a stripe has a hole or an overlap before {at}");
                    end += n;
                }
                assert_eq!(end, len, "band {bk} {name}: the stripe holds {len} entries, its rows account for {end}");
            };
            ranges("bits", band.bits.len(), &|r| (r.bits_at, r.words));
            ranges("payloads", band.payloads.len(), &|r| (r.cells_at, r.cells));
            ranges("kinds", band.kinds.len(), &|r| (r.kinds_at, r.kind_runs));
            ranges("styles", band.styles.len(), &|r| (r.styles_at, r.style_runs));
            assert!(band.rows.windows(2).all(|w| w[0].key < w[1].key), "band {bk}: rows out of order");
            for row in &band.rows {
                assert_eq!(runs_len(row.kind_runs(band)), row.cells as usize, "band {bk} row {}: kinds", row.key);
                assert_eq!(runs_len(row.style_runs(band)), row.cells as usize, "band {bk} row {}: styles", row.key);
                let bits: usize = row.bits(band).iter().map(|w| w.count_ones() as usize).sum();
                assert_eq!(bits, row.cells as usize, "band {bk} row {}: bitmap", row.key);
                assert!(row.kind_runs(band).windows(2).all(|w| w[0].0 != w[1].0), "band {bk} row {}: same-valued runs", row.key);
            }
        }
    }

    /// A store built and edited cell by cell must agree with a plain map, and its stripes must stay
    /// one contiguous run of slices — the layout is the thing this file is about, so the layout is
    /// what the test checks, not only what it reads back.
    #[test]
    fn stripes_hold_exactly_the_cells_after_an_edit_soup() {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = |n: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % n.max(1)) as u32
        };
        let mut s = CellStore::new();
        let mut model: BTreeMap<(u32, u32), Value> = BTreeMap::new();
        for step in 0..3_000 {
            // Rows either side of two band boundaries; columns either side of a bitmap word and of
            // the 64-column alignment, so the bitmap has to grow left and right.
            let row = [0, 1, 62, 63, 64, 65, 127, 128][next(8) as usize % 8];
            let col = [0, 1, 63, 64, 65, 127, 128, 200][next(8) as usize % 8];
            let value = Value::Number((next(5) as i64 - 1) as f64);
            match next(4) {
                0 => {
                    s.set(c(row, col), Cell::value(value.clone()));
                    model.insert((row, col), value);
                }
                1 => {
                    let gone = s.remove(c(row, col));
                    assert_eq!(gone.map(|x| x.value), model.remove(&(row, col)), "{row}:{col}");
                }
                2 => {
                    // A whole row at once, the way a file arrives.
                    let mut cells: Vec<(u32, Cell)> = (0..4).map(|i| (col + i * 64, Cell::value(Value::Number((step * 7 + i) as f64)))).collect();
                    s.set_row(row, &mut cells);
                    for i in 0..4 {
                        model.insert((row, col + i * 64), Value::Number((step * 7 + i) as f64));
                    }
                }
                _ => {
                    let text = Value::Text(Arc::from(format!("t{step}").as_str()));
                    s.set(c(row, col), Cell::value(text.clone()));
                    model.insert((row, col), text);
                }
            }
            check_stripes(&s);
        }
        let got: Vec<((u32, u32), Value)> = s.iter().map(|(at, cell)| ((at.row, at.col), cell.value)).collect();
        let want: Vec<((u32, u32), Value)> = model.iter().map(|(at, v)| (*at, v.clone())).collect();
        assert_eq!(got, want);
        assert_eq!(s.len(), model.len());
        assert_eq!(s.iter().count(), model.len());
        for ((row, col), v) in &model {
            assert_eq!(s.get(c(*row, *col)).map(|cell| cell.value), Some(v.clone()), "{row}:{col}");
        }
    }
}
