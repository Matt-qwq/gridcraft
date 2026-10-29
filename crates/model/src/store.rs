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

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

use gridcraft_core::{CellError, CellRef, RangeRef, Value};
use serde::{Deserialize, Serialize};

use crate::cell::{Cell, Formula};
use crate::style::StyleId;

/// How many column runs a packed walk follows before it builds cells through `iter_range`
/// instead. Two or three runs is an ordinary insert-and-delete history; past this, repeating the
/// per-row walk once per run would cost more than one `Cell` per cell.
const MAX_PACKED_COLUMN_SEGMENTS: usize = 8;

/// Rows per band: one undo snapshot shares bands this size, so an edit copies at most 64 rows.
const BAND: u32 = 64;

/// A value-only write this large is split across threads; below it the thread start costs more than
/// the work. Above the ceiling the clone of every touched band would double too much of the model.
#[cfg(not(target_arch = "wasm32"))]
const PARALLEL_WRITE_MIN: usize = 16_384;
#[cfg(not(target_arch = "wasm32"))]
const PARALLEL_WRITE_MAX_CELLS: usize = 4_000_000;

/// How many decoded bands the page cache keeps. A band is at most 64 rows; this is roughly 2.6M
/// cells at the export's density (a few tens of MiB), enough for a viewport and a scrolling burst
/// without letting a full-sheet scan grow the working set back to the live model.
const PAGE_CACHE_PAGES: usize = 4_096;

/// How many rewritten pages a compaction pass keeps decoded. Genuine orphans are a handful per
/// edit, and the read that follows a save is likely to want them; a bigger pass (or one that a
/// conservative column-gap test over-selected) leaves the page undecoded for the cache to load
/// once, so a rewrite cannot pin a band per page.
const PAGE_PRIME_LIMIT: usize = 8;

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
    /// Held out of line and shared: a band copied for a value edit (an undo snapshot keeps the
    /// old one) shares the formulas and arrays of every row that did not change, instead of
    /// copying one small vector per row.
    extras: Option<Arc<RowExtras>>,
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
                let e = Arc::make_mut(self.extras.get_or_insert_with(Default::default));
                match e.formulas.binary_search_by_key(&col, |(c, _)| *c) {
                    Ok(i) => e.formulas[i].1 = f.clone(),
                    Err(i) => e.formulas.insert(i, (col, f.clone())),
                }
            }
            None => {
                if let Some(e) = self.extras.as_mut()
                    && let Ok(i) = e.formulas.binary_search_by_key(&col, |(c, _)| *c)
                {
                    Arc::make_mut(e).formulas.remove(i);
                }
                self.drop_unused_extras();
            }
        }
    }

    fn set_array(&mut self, col: u32, array: Option<&Arc<gridcraft_core::Array>>) {
        match array {
            Some(a) => {
                let e = Arc::make_mut(self.extras.get_or_insert_with(Default::default));
                match e.arrays.binary_search_by_key(&col, |(c, _)| *c) {
                    Ok(i) => e.arrays[i].1 = a.clone(),
                    Err(i) => e.arrays.insert(i, (col, a.clone())),
                }
            }
            None => {
                if let Some(e) = self.extras.as_mut()
                    && let Ok(i) = e.arrays.binary_search_by_key(&col, |(c, _)| *c)
                {
                    Arc::make_mut(e).arrays.remove(i);
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
        self.numbers.shrink_to_fit();
        self.texts.shrink_to_fit();
        self.style_ids.shrink_to_fit();
        self.drop_number_slots();
        self.drop_text_index();
    }

    /// The band's slot for `s`, adding it when new. A band that has been finished has no index;
    /// the first edit that needs one rebuilds it from the texts the band still holds.
    fn text_slot(&mut self, s: &Arc<str>) -> u32 {
        if self.text_index.is_empty() && !self.texts.is_empty() {
            self.reindex_texts();
        }
        let key = Arc::as_ptr(s) as *const u8 as usize;
        match self.text_index.binary_search_by_key(&key, |(at, _)| *at) {
            Ok(i) => self.text_index[i].1,
            Err(at) => {
                let slot = self.texts.len() as u32;
                self.texts.push(s.clone());
                self.text_index.insert(at, (key, slot));
                slot
            }
        }
    }

    /// Rebuilds the pointer index from the band's text table. The table is kept after the band is
    /// finished; the index is only needed while cells are being added.
    fn reindex_texts(&mut self) {
        self.text_index = self.texts.iter().enumerate().map(|(i, t)| (Arc::as_ptr(t) as *const u8 as usize, i as u32)).collect();
        self.text_index.sort_unstable_by_key(|(at, _)| *at);
    }

    /// Offers the text index back to the allocator, like the number index: only adding cells needs
    /// it. Dropping it matters most on a unique-key export, where a band holds hundreds of texts
    /// and every band would otherwise keep a 16-byte sorted entry per text forever.
    fn drop_text_index(&mut self) {
        self.text_index = Vec::new();
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
            Value::Text(t) => {
                let slot = self.text_slot(t);
                match u16::try_from(slot) {
                    Ok(slot) => (TEXT, Payload::Small(slot)),
                    // The table holds more texts than a `u16` can index: the payload points into
                    // the row's side table, which holds this text's slot.
                    Err(_) => (TEXT_WIDE, Payload::Wide(u64::from(slot))),
                }
            }
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
        let extras = Arc::make_mut(self.rows[at].extras.get_or_insert_with(Default::default));
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

    /// Replaces the value of a stored cell, keeping its formula and style. Returns false when no
    /// cell is stored at `col` (a result has nowhere to go there). Saves what [`put`](Self::put)
    /// costs for a cell that only changes value: nothing is constructed, no formula or style slot
    /// is looked up, and the row already holds every stripe entry.
    fn set_value(&mut self, row_key: u32, col: u32, value: &Value) -> bool {
        let Some(at) = self.row_index(row_key) else { return false };
        let Some(i) = self.rows[at].index_of(self, col) else { return false };
        let (kind, payload) = self.payload(value);
        let payload = match payload {
            Payload::Small(p) => p,
            Payload::Wide(v) => self.push_wide(at, v),
        };
        let slot = self.rows[at].cells_at as usize + i;
        if let Some(p) = self.payloads.get_mut(slot) {
            *p = payload;
        }
        // Most results keep their kind (`=A1*2` stays a number): the run stripe need not move.
        if self.rows[at].kind_at(self, i) != kind {
            let (kinds_at, old_kinds) = (self.rows[at].kinds_at, self.rows[at].kind_runs);
            let kind_runs = runs_set(&mut self.kinds, kinds_at as usize, old_kinds as usize, i, u16::from(kind));
            self.shift_offsets(at, (0, 0), (0, 0), (kinds_at, kind_runs as i32 - old_kinds as i32), (0, 0));
            self.rows[at].kind_runs = kind_runs as u32;
        }
        match value {
            Value::Array(a) => self.rows[at].set_array(col, Some(a)),
            _ if self.rows[at].arrays().iter().any(|(c, _)| *c == col) => self.rows[at].set_array(col, None),
            _ => {}
        }
        true
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
            if formulas.is_empty() && arrays.is_empty() && wide.is_empty() { None } else { Some(Arc::new(RowExtras { formulas, arrays, wide })) };
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

/// One logical→physical row run. Rows are stored under physical keys; a structural row edit changes
/// this map instead of moving every cell below it. `len` is in rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RowSegment {
    logical: u32,
    physical: u32,
    len: u32,
}

impl RowSegment {
    fn logical_end(self) -> u32 {
        self.logical.saturating_add(self.len)
    }

    fn physical_end(self) -> u32 {
        self.physical.saturating_add(self.len)
    }
}

/// The logical→physical row map. `identity` is the fast path used by every workbook that has never
/// had a row inserted or deleted: logical row = physical row, no lookup. The first structural edit
/// materializes one segment covering the sheet and thereafter keeps two sorted views, by logical
/// row for reads/writes and by physical row for iteration and `used_range`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RowMap {
    identity: bool,
    segments: Vec<RowSegment>,
    by_physical: Vec<RowSegment>,
    /// Rows written into an inserted gap get fresh physical rows from here; existing rows keep
    /// their physical keys, so an insert never touches a band.
    next_physical: u32,
}

impl Default for RowMap {
    fn default() -> Self {
        RowMap { identity: true, segments: Vec::new(), by_physical: Vec::new(), next_physical: gridcraft_core::MAX_ROWS }
    }
}

impl RowMap {
    fn is_identity(&self) -> bool {
        self.identity
    }

    fn map(&self, logical: u32) -> Option<u32> {
        if logical >= gridcraft_core::MAX_ROWS {
            return None;
        }
        if self.identity {
            return Some(logical);
        }
        let index = self.segments.partition_point(|s| s.logical <= logical).checked_sub(1)?;
        let segment = self.segments.get(index)?;
        let offset = logical.checked_sub(segment.logical)?;
        (offset < segment.len).then(|| segment.physical.saturating_add(offset))
    }

    fn inverse(&self, physical: u32) -> Option<u32> {
        if self.identity {
            return (physical < gridcraft_core::MAX_ROWS).then_some(physical);
        }
        let index = self.by_physical.partition_point(|s| s.physical <= physical).checked_sub(1)?;
        let segment = self.by_physical.get(index)?;
        let offset = physical.checked_sub(segment.physical)?;
        (offset < segment.len).then(|| segment.logical.saturating_add(offset))
    }

    fn ensure(&mut self) {
        if self.identity {
            self.identity = false;
            self.segments = vec![RowSegment { logical: 0, physical: 0, len: gridcraft_core::MAX_ROWS }];
            self.rebuild();
        }
    }

    /// Allocates a physical row for a logical row that has no mapping (a row inside an inserted
    /// gap, or one exposed by a delete). The fresh key is above every physical row the sheet had,
    /// so it cannot collide with an existing band.
    fn allocate(&mut self, logical: u32) -> Option<u32> {
        if let Some(physical) = self.map(logical) {
            return Some(physical);
        }
        if logical >= gridcraft_core::MAX_ROWS {
            return None;
        }
        self.ensure();
        let physical = self.next_physical;
        self.next_physical = self.next_physical.checked_add(1)?;
        let at = self.segments.partition_point(|s| s.logical < logical);
        self.segments.insert(at, RowSegment { logical, physical, len: 1 });
        self.rebuild();
        Some(physical)
    }

    /// The segments intersecting a logical row range, clipped to it, in logical order.
    fn segments_in(&self, r0: u32, r1: u32) -> Vec<RowSegment> {
        let (r0, r1) = (r0.min(gridcraft_core::MAX_ROWS.saturating_sub(1)), r1.min(gridcraft_core::MAX_ROWS.saturating_sub(1)));
        if r0 > r1 {
            return Vec::new();
        }
        if self.identity {
            return vec![RowSegment { logical: r0, physical: r0, len: r1 - r0 + 1 }];
        }
        let mut out = Vec::new();
        for s in &self.segments {
            let lo = s.logical.max(r0);
            let hi = s.logical_end().min(r1.saturating_add(1));
            if lo >= hi {
                continue;
            }
            out.push(RowSegment { logical: lo, physical: s.physical.saturating_add(lo - s.logical), len: hi - lo });
        }
        out
    }

    /// Inserts (`delta > 0`) or deletes (`delta < 0`) logical rows at `from`, changing only the
    /// mapping. Rows below stay on their physical bands; the vacated/removed range is a gap or a
    /// dropped set of segments. `from` is the first source row for a delete, as in the old
    /// `shift_rows`, so the deleted destination range is `[from - count, from)`.
    fn shift(&mut self, from: u32, delta: i64) {
        if delta == 0 {
            return;
        }
        let from = from.min(gridcraft_core::MAX_ROWS);
        let count = delta.unsigned_abs().min(u64::from(gridcraft_core::MAX_ROWS)) as u32;
        if count == 0 {
            return;
        }
        self.ensure();
        let mut out: Vec<RowSegment> = Vec::new();
        for s in &self.segments {
            let a = s.logical;
            let b = s.logical_end().min(gridcraft_core::MAX_ROWS);
            if a >= b {
                continue;
            }
            if delta > 0 {
                if a < from {
                    let hi = b.min(from);
                    if a < hi {
                        out.push(RowSegment { logical: a, physical: s.physical, len: hi - a });
                    }
                }
                if b > from {
                    let start = a.max(from);
                    let new_logical = start.saturating_add(count);
                    if new_logical < gridcraft_core::MAX_ROWS {
                        let end = b.saturating_add(count).min(gridcraft_core::MAX_ROWS);
                        if new_logical < end {
                            out.push(RowSegment { logical: new_logical, physical: s.physical.saturating_add(start - a), len: end - new_logical });
                        }
                    }
                }
            } else {
                let delete_start = from.saturating_sub(count);
                let delete_end = from;
                if a < delete_start {
                    let hi = b.min(delete_start);
                    if a < hi {
                        out.push(RowSegment { logical: a, physical: s.physical, len: hi - a });
                    }
                }
                if b > delete_end {
                    let start = a.max(delete_end);
                    let new_logical = start.saturating_sub(count);
                    if start < b {
                        out.push(RowSegment { logical: new_logical, physical: s.physical.saturating_add(start - a), len: b - start });
                    }
                }
            }
        }
        self.segments = out;
        self.rebuild();
    }

    /// Reclaims the map after cells have been removed: merges the runs the way [`rebuild`] does
    /// and returns a single run from the origin to the identity fast path when this store has no
    /// page that the shorter run would otherwise leave behind. A page can hold cells at physical
    /// rows past the run; turning the map into an identity would make them readable again, so a
    /// store with pages only resets a map that covers every row.
    ///
    /// [`rebuild`]: Self::rebuild
    fn normalize(&mut self, reset_prefix: bool) {
        if self.identity {
            return;
        }
        self.rebuild();
        let Some(segment) = self.segments.first().copied() else { return };
        if self.segments.len() != 1 || segment.logical != 0 || segment.physical != 0 || segment.len == 0 {
            return;
        }
        if !reset_prefix && segment.len < gridcraft_core::MAX_ROWS {
            return;
        }
        self.identity = true;
        self.segments.clear();
        self.by_physical.clear();
        self.next_physical = gridcraft_core::MAX_ROWS;
    }

    /// Merges adjacent logical/physical runs and refreshes the inverse view.
    fn rebuild(&mut self) {
        let mut merged: Vec<RowSegment> = Vec::with_capacity(self.segments.len());
        for s in self.segments.drain(..) {
            if s.len == 0 {
                continue;
            }
            if let Some(last) = merged.last_mut()
                && last.logical_end() == s.logical
                && last.physical_end() == s.physical
                && last.len.checked_add(s.len).is_some()
            {
                last.len = last.len.saturating_add(s.len);
                continue;
            }
            merged.push(s);
        }
        self.segments = merged;
        let mut by = self.segments.clone();
        by.sort_unstable_by_key(|s| (s.physical, s.logical));
        let mut inverse: Vec<RowSegment> = Vec::with_capacity(by.len());
        for s in by {
            if let Some(last) = inverse.last_mut()
                && last.physical_end() == s.physical
                && last.logical_end() == s.logical
                && last.len.checked_add(s.len).is_some()
            {
                last.len = last.len.saturating_add(s.len);
                continue;
            }
            inverse.push(s);
        }
        self.by_physical = inverse;
    }
}

/// The column sibling of [`RowMap`]. Columns need one extra rule: their physical space is the
/// worksheet's 16,384 columns plus whatever a structural edit allocated, so a deleted run's
/// physical columns go on a free list and a write into an inserted gap reuses one before extending
/// the range. `len` is in columns.
type ColSegment = RowSegment;

#[derive(Clone, Debug, PartialEq, Eq)]
struct ColMap {
    identity: bool,
    segments: Vec<ColSegment>,
    by_physical: Vec<ColSegment>,
    next_physical: u32,
    free: Vec<u32>,
}

impl Default for ColMap {
    fn default() -> Self {
        ColMap { identity: true, segments: Vec::new(), by_physical: Vec::new(), next_physical: 0, free: Vec::new() }
    }
}

impl ColMap {
    fn is_identity(&self) -> bool {
        self.identity
    }

    /// Materializes the initial identity run for the columns the sheet actually holds. Unlike
    /// rows, a full 16,384-column identity range would make a write into the first inserted column
    /// pick column 16,384 and widen every row's bitmap; only the used width is materialized.
    fn ensure_with(&mut self, physical_max: u32) {
        if !self.identity {
            return;
        }
        self.identity = false;
        let width = physical_max.saturating_add(1).min(gridcraft_core::MAX_COLS);
        self.next_physical = width;
        if width > 0 {
            self.segments = vec![ColSegment { logical: 0, physical: 0, len: width }];
        }
        self.rebuild();
    }

    fn map(&self, logical: u32) -> Option<u32> {
        if logical >= gridcraft_core::MAX_COLS {
            return None;
        }
        if self.identity {
            return Some(logical);
        }
        let index = self.segments.partition_point(|s| s.logical <= logical).checked_sub(1)?;
        let segment = self.segments.get(index)?;
        let offset = logical.checked_sub(segment.logical)?;
        (offset < segment.len).then(|| segment.physical.saturating_add(offset))
    }

    fn inverse(&self, physical: u32) -> Option<u32> {
        if self.identity {
            return (physical < gridcraft_core::MAX_COLS).then_some(physical);
        }
        let index = self.by_physical.partition_point(|s| s.physical <= physical).checked_sub(1)?;
        let segment = self.by_physical.get(index)?;
        let offset = physical.checked_sub(segment.physical)?;
        (offset < segment.len).then(|| segment.logical.saturating_add(offset))
    }

    fn allocate(&mut self, logical: u32) -> Option<u32> {
        if let Some(physical) = self.map(logical) {
            return Some(physical);
        }
        if logical >= gridcraft_core::MAX_COLS {
            return None;
        }
        if self.identity {
            self.ensure_with(self.next_physical.min(gridcraft_core::MAX_COLS.saturating_sub(1)));
        }
        let physical = if let Some(physical) = self.free.pop() {
            physical
        } else {
            let physical = self.next_physical;
            self.next_physical = self.next_physical.checked_add(1)?;
            physical
        };
        let at = self.segments.partition_point(|s| s.logical < logical);
        self.segments.insert(at, ColSegment { logical, physical, len: 1 });
        self.rebuild();
        Some(physical)
    }

    fn segments_in(&self, c0: u32, c1: u32) -> Vec<ColSegment> {
        let (c0, c1) = (c0.min(gridcraft_core::MAX_COLS.saturating_sub(1)), c1.min(gridcraft_core::MAX_COLS.saturating_sub(1)));
        if c0 > c1 {
            return Vec::new();
        }
        if self.identity {
            return vec![ColSegment { logical: c0, physical: c0, len: c1 - c0 + 1 }];
        }
        let mut out = Vec::new();
        for s in &self.segments {
            let lo = s.logical.max(c0);
            let hi = s.logical_end().min(c1.saturating_add(1));
            if lo >= hi {
                continue;
            }
            out.push(ColSegment { logical: lo, physical: s.physical.saturating_add(lo - s.logical), len: hi - lo });
        }
        out
    }

    fn shift(&mut self, from: u32, delta: i64) {
        if delta == 0 {
            return;
        }
        let from = from.min(gridcraft_core::MAX_COLS);
        let count = delta.unsigned_abs().min(u64::from(gridcraft_core::MAX_COLS)) as u32;
        if count == 0 {
            return;
        }
        self.ensure_with(0);
        let mut out: Vec<ColSegment> = Vec::new();
        for s in &self.segments {
            let a = s.logical;
            let b = s.logical_end().min(gridcraft_core::MAX_COLS);
            if a >= b {
                continue;
            }
            if delta > 0 {
                if a < from {
                    let hi = b.min(from);
                    if a < hi {
                        out.push(ColSegment { logical: a, physical: s.physical, len: hi - a });
                    }
                }
                if b > from {
                    let start = a.max(from);
                    let new_logical = start.saturating_add(count);
                    if new_logical < gridcraft_core::MAX_COLS {
                        let end = b.saturating_add(count).min(gridcraft_core::MAX_COLS);
                        if new_logical < end {
                            out.push(ColSegment { logical: new_logical, physical: s.physical.saturating_add(start - a), len: end - new_logical });
                        }
                    }
                }
            } else {
                let delete_start = from.saturating_sub(count);
                let delete_end = from;
                if a < delete_start {
                    let hi = b.min(delete_start);
                    if a < hi {
                        out.push(ColSegment { logical: a, physical: s.physical, len: hi - a });
                    }
                }
                if b > delete_end {
                    let start = a.max(delete_end);
                    let new_logical = start.saturating_sub(count);
                    if start < b {
                        out.push(ColSegment { logical: new_logical, physical: s.physical.saturating_add(start - a), len: b - start });
                    }
                }
                // The destination columns are no longer mapped; their physical keys are reusable.
                let drop_start = a.max(delete_start);
                let drop_end = b.min(delete_end);
                for physical in drop_start.saturating_sub(a)..drop_end.saturating_sub(a) {
                    self.free.push(s.physical.saturating_add(physical));
                }
            }
        }
        self.segments = out;
        self.rebuild();
    }

    /// Reclaims the map after cells have been removed: merges the runs, drops the free keys that
    /// are mapped or past every physical column this map has handed out, and returns a single run
    /// from the origin to the identity fast path when this store has no page that the shorter run
    /// would otherwise leave behind. See [`RowMap::normalize`] for why pages bound the reset.
    fn normalize(&mut self, reset_prefix: bool) {
        if self.identity {
            self.free.clear();
            return;
        }
        self.rebuild();
        let mut free = std::mem::take(&mut self.free);
        free.sort_unstable();
        free.dedup();
        free.retain(|physical| *physical < self.next_physical && self.inverse(*physical).is_none());
        self.free = free;
        let Some(segment) = self.segments.first().copied() else { return };
        if self.segments.len() != 1 || segment.logical != 0 || segment.physical != 0 || segment.len == 0 {
            return;
        }
        if !reset_prefix && segment.len < gridcraft_core::MAX_COLS {
            return;
        }
        self.identity = true;
        self.segments.clear();
        self.by_physical.clear();
        self.free.clear();
        self.next_physical = 0;
    }

    fn rebuild(&mut self) {
        let mut merged: Vec<ColSegment> = Vec::with_capacity(self.segments.len());
        for s in self.segments.drain(..) {
            if s.len == 0 {
                continue;
            }
            if let Some(last) = merged.last_mut()
                && last.logical_end() == s.logical
                && last.physical_end() == s.physical
            {
                last.len = last.len.saturating_add(s.len);
                continue;
            }
            merged.push(s);
        }
        self.segments = merged;
        let mut by = self.segments.clone();
        by.sort_unstable_by_key(|s| (s.physical, s.logical));
        let mut inverse: Vec<ColSegment> = Vec::with_capacity(by.len());
        for s in by {
            if let Some(last) = inverse.last_mut()
                && last.physical_end() == s.physical
                && last.logical_end() == s.logical
            {
                last.len = last.len.saturating_add(s.len);
                continue;
            }
            inverse.push(s);
        }
        self.by_physical = inverse;
    }
}

/// What one [`CellStore::compact`] pass reclaimed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Compacted {
    /// Cells dropped because no logical row or column could reach them.
    pub cells: usize,
    /// Live bands dropped because compaction emptied them.
    pub bands: usize,
    /// Paged bands rewritten (or dropped) to lose their orphans.
    pub pages: usize,
    /// Whether the store differs from before: a cell, band or page was reclaimed, or a map merged
    /// or returned to the identity fast path.
    pub changed: bool,
}

/// A `CellStore`'s packed bands, plus the count of stored cells.
#[derive(Debug, Default)]
pub struct CellStore {
    bands: BTreeMap<u32, Arc<Band>>,
    /// Bands that were spilled to a page file. A paged band is still in the store's logical range
    /// and `len`; it is decoded on the first read and then cached in its `Page`.
    pages: BTreeMap<u32, Arc<page::Page>>,
    /// The temporary file the pages live in. One per store (and shared by its clones); dropping
    /// the last store removes it.
    page_file: Option<Arc<page::PageFile>>,
    /// Logical→physical rows. Identity until a row insert or delete.
    row_map: RowMap,
    /// Logical→physical columns. Identity until a column insert or delete.
    col_map: ColMap,
    /// The recency queue for decoded pages; shared by clones, like the file. It holds keys whose
    /// page is (or was) decoded; a key whose page was evicted is a no-op when it reaches the front.
    page_order: Option<Arc<Mutex<VecDeque<u32>>>>,
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
        CellStore {
            bands: self.bands.clone(),
            pages: self.pages.clone(),
            page_file: self.page_file.clone(),
            page_order: self.page_order.clone(),
            row_map: self.row_map.clone(),
            col_map: self.col_map.clone(),
            len: self.len,
            building: None,
        }
    }
}

impl PartialEq for CellStore {
    fn eq(&self, other: &Self) -> bool {
        if self.len != other.len {
            return false;
        }
        if self.row_map != other.row_map || self.col_map != other.col_map {
            // A structural edit changes where a cell lives, not what it is: compare the logical
            // walk, which applies each map. A saved-and-reopened sheet has an identity map, so this
            // is the path that holds it equal to the mapped sheet it came from.
            let mut mine = self.iter();
            let mut theirs = other.iter();
            loop {
                match (mine.next(), theirs.next()) {
                    (None, None) => break,
                    (Some((a_at, a)), Some((b_at, b))) if a_at == b_at && same_cell(&a, &b) => {}
                    _ => return false,
                }
            }
            return true;
        }
        self.band_count() == other.band_count()
            && self.bands.keys().chain(self.pages.keys()).eq(other.bands.keys().chain(other.pages.keys()))
            && self.bands.keys().chain(self.pages.keys()).all(|k| match (self.band_arc(*k), other.band_arc(*k)) {
                (Some(a), Some(b)) => Arc::ptr_eq(&a, &b) || a.same_as(&b),
                _ => false,
            })
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

/// Walks a row range over live and paged bands one band at a time, so a whole-column read does not
/// collect the sheet (or decode every page at once). A band's cells are gathered into a small
/// iterator before the next band is loaded.
struct CellWalk<'a> {
    store: &'a CellStore,
    segments: std::vec::IntoIter<RowSegment>,
    segment: Option<RowSegment>,
    keys: std::vec::IntoIter<u32>,
    col_segments: Vec<ColSegment>,
    cells: std::vec::IntoIter<(CellRef, Cell)>,
    r: RangeRef,
}

impl<'a> CellWalk<'a> {
    fn new(store: &'a CellStore, r: RangeRef) -> CellWalk<'a> {
        CellWalk {
            store,
            segments: store.row_map.segments_in(r.start.row, r.end.row).into_iter(),
            segment: None,
            keys: Vec::new().into_iter(),
            col_segments: store.col_map.segments_in(r.start.col, r.end.col),
            cells: Vec::new().into_iter(),
            r,
        }
    }
}

impl Iterator for CellWalk<'_> {
    type Item = (CellRef, Cell);

    fn next(&mut self) -> Option<(CellRef, Cell)> {
        loop {
            if let Some(cell) = self.cells.next() {
                return Some(cell);
            }
            if let Some(key) = self.keys.next() {
                let Some(segment) = self.segment else { continue };
                let Some(band) = self.store.band_arc(key) else { continue };
                let physical_start = segment.physical;
                let physical_end = segment.physical.saturating_add(segment.len);
                let mut cells = Vec::new();
                for row in &band.rows {
                    let physical = key * BAND + row.key;
                    if physical < physical_start || physical >= physical_end {
                        continue;
                    }
                    let logical = segment.logical.saturating_add(physical - physical_start);
                    if logical < self.r.start.row || logical > self.r.end.row {
                        continue;
                    }
                    for columns in &self.col_segments {
                        let physical_first = columns.physical;
                        let physical_last = columns.physical.saturating_add(columns.len).saturating_sub(1);
                        if physical_first > physical_last {
                            continue;
                        }
                        for (at, cell) in packed_row(&band, logical, row, physical_first, physical_last) {
                            let col = columns.logical.saturating_add(at.col - physical_first);
                            if col >= self.r.start.col && col <= self.r.end.col {
                                cells.push((CellRef::new(logical, col), cell));
                            }
                        }
                    }
                }
                self.cells = cells.into_iter();
                continue;
            }
            let segment = self.segments.next()?;
            if segment.len == 0 {
                continue;
            }
            self.keys = self.store.bands_in(segment.physical, segment.physical.saturating_add(segment.len)).into_iter();
            self.segment = Some(segment);
        }
    }
}

/// One run of a value batch that lands in one band: `values[start..end]` all map to band `key`,
/// and `base` is the physical row of `values[start]`. A band can be fed by several runs when the
/// row map has a gap inside it, so a plan merges them into one task.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug)]
struct BandRun {
    key: u32,
    base: u32,
    start: usize,
    end: usize,
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

    /// The live band, or the decoded copy of its page. The `Arc` is returned so a caller can keep
    /// it alive while it walks the band after the store borrow ends.
    fn band_arc(&self, key: u32) -> Option<Arc<Band>> {
        if let Some(band) = self.bands.get(&key) {
            return Some(band.clone());
        }
        let page = self.pages.get(&key)?;
        let (band, first) = page.load(self.page_file.as_deref()?);
        if first && band.is_some() {
            self.touch_page(key);
        }
        band
    }

    /// The band keys that hold data in a physical row range. Unlike a range of band numbers, this
    /// only walks bands the store actually has, so iterating a small sheet does not probe 16,384
    /// empty bands (or a million empty logical rows).
    fn bands_in(&self, physical_start: u32, physical_end: u32) -> Vec<u32> {
        let first = physical_start / BAND;
        let last = physical_end.saturating_sub(1) / BAND;
        if physical_start >= physical_end {
            return Vec::new();
        }
        let mut keys: Vec<u32> = self.bands.range(first..=last).map(|(k, _)| *k).collect();
        keys.extend(self.pages.range(first..=last).map(|(k, _)| *k));
        keys.sort_unstable();
        keys.dedup();
        keys
    }

    /// The physical row a logical row lives on, or `None` when the row is a gap made by an insert
    /// or a delete.
    fn physical_row(&self, logical: u32) -> Option<u32> {
        self.row_map.map(logical)
    }

    /// The physical row for a write: an existing mapping, or a fresh row for a logical row in an
    /// inserted gap. A failed allocation (the physical key space is exhausted) refuses the write.
    fn physical_row_for_write(&mut self, logical: u32) -> Option<u32> {
        if let Some(physical) = self.row_map.map(logical) {
            return Some(physical);
        }
        self.row_map.allocate(logical)
    }

    fn physical_col(&self, logical: u32) -> Option<u32> {
        self.col_map.map(logical)
    }

    fn physical_col_for_write(&mut self, logical: u32) -> Option<u32> {
        if let Some(physical) = self.col_map.map(logical) {
            return Some(physical);
        }
        self.col_map.allocate(logical)
    }

    /// The rightmost physical column that holds a cell, used to build just the existing width of
    /// the column map instead of a full 16,384-column identity run.
    fn physical_max_col(&self) -> Option<u32> {
        let mut max = None;
        for band in self.bands.values() {
            for row in &band.rows {
                if let Some((_, last)) = row.column_span(band) {
                    max = Some(max.map_or(last, |m: u32| m.max(last)));
                }
            }
        }
        for page in self.pages.values() {
            if let Some((_, _, _, last)) = page.bounds() {
                max = Some(max.map_or(last, |m: u32| m.max(last)));
            }
        }
        max
    }

    fn ensure_col_map(&mut self) {
        if self.col_map.is_identity() {
            self.col_map.ensure_with(self.physical_max_col().unwrap_or(0));
        }
    }

    /// The logical columns covered by a physical column span, if any. Used to keep `used_range`
    /// logical after a column edit without loading pages or walking every column.
    fn logical_col_span(&self, physical_first: u32, physical_last: u32) -> Option<(u32, u32)> {
        if self.col_map.is_identity() {
            return (physical_first < gridcraft_core::MAX_COLS).then(|| (physical_first, physical_last.min(gridcraft_core::MAX_COLS - 1)));
        }
        let mut lo = u32::MAX;
        let mut hi = 0u32;
        for segment in &self.col_map.segments {
            let a = segment.physical.max(physical_first);
            let b = segment.physical_end().min(physical_last.saturating_add(1));
            if a >= b {
                continue;
            }
            let logical_a = segment.logical.saturating_add(a - segment.physical);
            let logical_b = segment.logical.saturating_add(b - 1 - segment.physical);
            lo = lo.min(logical_a);
            hi = hi.max(logical_b);
        }
        (lo != u32::MAX).then_some((lo, hi))
    }

    /// Records a decoded page and evicts the oldest when the cache is full. The queue is shared by
    /// undo snapshots, so an eviction here helps every store holding the same file.
    fn touch_page(&self, key: u32) {
        let Some(order) = &self.page_order else { return };
        let Ok(mut order) = order.lock() else { return };
        order.push_back(key);
        while order.len() > PAGE_CACHE_PAGES {
            let Some(old) = order.pop_front() else { break };
            if old != key
                && let Some(page) = self.pages.get(&old)
            {
                page.clear();
            }
        }
    }

    /// Bands held in memory, and bands on the page file.
    pub fn paged_band_count(&self) -> usize {
        self.pages.len()
    }

    /// Turns paging on for this store: every completed band already in it is written to a fresh
    /// temporary page file, and later `insert_batch` calls spill the bands they add. Called by the
    /// reader before it fills a sheet, so a large open keeps pages instead of every band.
    ///
    /// Returns false when a temporary file cannot be created; the store stays ordinary.
    pub fn enable_paging(&mut self) -> bool {
        if self.page_file.is_some() {
            return true;
        }
        let Some(file) = page::PageFile::new() else { return false };
        self.page_file = Some(Arc::new(file));
        self.page_order = Some(Arc::new(Mutex::new(VecDeque::new())));
        self.spill_pages();
        true
    }

    /// Spills every live band that can be encoded. A band that fails to encode (or a page that
    /// cannot be written) stays live; paging never loses cells.
    pub fn spill_pages(&mut self) {
        let keys: Vec<u32> = self.bands.keys().copied().collect();
        for key in keys {
            self.spill_band(key);
        }
    }

    fn spill_band(&mut self, key: u32) {
        let Some(file) = self.page_file.clone() else { return };
        let Some(band) = self.bands.get(&key).cloned() else { return };
        let Some(bytes) = page::encode_band(&band) else { return };
        let Some(at) = file.write(&bytes) else { return };
        let bounds = page::band_bounds(&band);
        let formulas = band.rows.iter().any(|r| !r.formulas().is_empty());
        let owned = band.rows.iter().any(|r| !r.formulas().is_empty() || !r.arrays().is_empty());
        self.bands.remove(&key);
        self.pages.insert(key, Arc::new(page::Page::new(at, bounds, formulas, owned)));
    }

    /// Loads a paged band back into the live map, so a write can modify it. A corrupt page stays
    /// paged and the write is refused rather than replacing data with an empty band.
    fn materialize(&mut self, key: u32) -> Option<Arc<Band>> {
        if let Some(band) = self.bands.get(&key) {
            return Some(band.clone());
        }
        let page = self.pages.get(&key)?.clone();
        let band = page.load(self.page_file.as_deref()?).0?;
        self.pages.remove(&key);
        self.bands.insert(key, band.clone());
        Some(band)
    }

    /// Bytes the page file holds, for the memory diagnostics. Pages are encoded, not compressed
    /// yet; this is the live cost of the decoded bands that have been touched, plus the file.
    pub fn paged_bytes(&self) -> usize {
        self.pages.values().map(|p| p.page_len()).sum()
    }

    /// Reads one cell. The value is rebuilt rather than borrowed: cells live in packed pages, so
    /// there is no `Cell` to point at. A text cell shares its string, so the copy is a pointer
    /// bump, not a string copy.
    pub fn get(&self, c: CellRef) -> Option<Cell> {
        let physical = self.physical_row(c.row)?;
        let col = self.physical_col(c.col)?;
        let band = self.band_arc(physical / BAND)?;
        let row = band.row(physical % BAND)?;
        let i = row.index_of(&band, col)?;
        Some(band.cell_at(row, i, col))
    }

    /// The style of a stored cell, without reconstructing the `Cell` (and its value and formula).
    /// A style is all a formatting or duplicate check needs; `get` rebuilds the rest for nothing.
    pub fn style(&self, c: CellRef) -> Option<StyleId> {
        let physical = self.physical_row(c.row)?;
        let col = self.physical_col(c.col)?;
        let band = self.band_arc(physical / BAND)?;
        let row = band.row(physical % BAND)?;
        let i = row.index_of(&band, col)?;
        Some(band.style(row.style_at(&band, i)))
    }

    /// True when the position holds a cell at all (a stored style counts).
    pub fn contains(&self, c: CellRef) -> bool {
        let (Some(physical), Some(col)) = (self.physical_row(c.row), self.physical_col(c.col)) else { return false };
        self.band_arc(physical / BAND).is_some_and(|b| b.row(physical % BAND).is_some_and(|r| r.index_of(&b, col).is_some()))
    }

    /// Stores a cell; a default (empty, unstyled) cell is removed instead.
    pub fn set(&mut self, c: CellRef, cell: Cell) {
        let row_physical = self.physical_row(c.row);
        let col_physical = self.physical_col(c.col);
        // A blank write into an inserted row/column must not allocate storage for nothing.
        if (row_physical.is_none() || col_physical.is_none()) && cell.is_blank() {
            return;
        }
        let Some(physical) = (match row_physical {
            Some(physical) => Some(physical),
            None => self.physical_row_for_write(c.row),
        }) else {
            return;
        };
        let Some(col) = (match col_physical {
            Some(physical) => Some(physical),
            None => self.physical_col_for_write(c.col),
        }) else {
            return;
        };
        let key = physical / BAND;
        let row = physical % BAND;
        if self.pages.contains_key(&key) && self.materialize(key).is_none() {
            return;
        }
        let had = self.contains(c);
        let band = Arc::make_mut(self.bands.entry(key).or_default());
        band.put(row, col, cell);
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
            // A chunk boundary can land in a band already spilled by the previous batch: bring it
            // back so the merge can see its rows, then spill the result again.
            if self.pages.contains_key(&key) && self.materialize(key).is_none() {
                continue;
            }
            let Some(existing) = self.bands.get_mut(&key) else {
                self.len += count(&band);
                self.bands.insert(key, Arc::new(band));
                self.spill_band(key);
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
            self.spill_band(key);
        }
        if self.building.is_none() {
            self.building = self.bands.keys().next_back().copied();
        }
    }

    /// Replaces the value of a cell that is already stored, keeping its formula and style. Returns
    /// false when nothing is stored there.
    ///
    /// A recalculation only changes values, but [`set`](Self::set) rebuilds the whole `Cell` (and
    /// takes the store's bands apart) for each one; at 100k results that is most of the write-back.
    pub fn set_value(&mut self, c: CellRef, value: &Value) -> bool {
        let Some(physical) = self.physical_row(c.row) else { return false };
        let Some(col) = self.physical_col(c.col) else { return false };
        let key = physical / BAND;
        if self.pages.contains_key(&key) && self.materialize(key).is_none() {
            return false;
        }
        let Some(band) = self.bands.get_mut(&key) else { return false };
        Arc::make_mut(band).set_value(physical % BAND, col, value)
    }

    /// Replaces the values of cells that already exist. `values` must be sorted in row-major
    /// order (the order a recalculation writes them in), so each band is looked up once.
    ///
    /// A batch that is not in that order is written one value at a time instead: a mapped store's
    /// logical rows are not its physical rows, so a band-grouped walk would put a value on the
    /// wrong row. The check is one pass over the batch.
    pub fn set_values(&mut self, values: &mut [(CellRef, Value)]) {
        if values.is_empty() {
            return;
        }
        if !values.windows(2).all(|w| (w[0].0.row, w[0].0.col) <= (w[1].0.row, w[1].0.col)) {
            for (c, v) in values.iter() {
                self.set_value(*c, v);
            }
            return;
        }
        if !self.row_map.is_identity() || !self.col_map.is_identity() {
            // The logical batch still arrives in row order, and inside one row segment physical
            // rows are affine in the logical row, so the write streams band by band without an
            // index, a sort, or a per-value map lookup. A mapped sheet is one that has had a
            // structural edit, not the common editing case.
            #[cfg(not(target_arch = "wasm32"))]
            if values.len() >= PARALLEL_WRITE_MIN && values.len() <= PARALLEL_WRITE_MAX_CELLS {
                let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(8);
                if threads > 1 && self.set_values_mapped_parallel(values, threads) {
                    return;
                }
            }
            self.set_values_mapped(values);
            return;
        }
        // A paged band a result lands in has to come back before either write path looks at the
        // live map. Values are sorted by row, so the touched bands are a short run of keys.
        if !self.pages.is_empty() {
            let mut keys: Vec<u32> = values.iter().map(|(c, _)| c.row / BAND).collect();
            keys.dedup();
            for key in keys {
                if self.pages.contains_key(&key) {
                    self.materialize(key);
                }
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        if values.len() >= PARALLEL_WRITE_MIN && values.len() <= PARALLEL_WRITE_MAX_CELLS {
            let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(8);
            if threads > 1 {
                self.set_values_parallel(values, threads);
                return;
            }
        }
        self.set_values_sequential(values);
    }

    /// The value write on a mapped store: walk the row map's segments in logical order and each
    /// segment's bands in physical order, taking one band borrow and one page materialize per run.
    /// Rows the map does not cover are gaps: a value there has nowhere to go, exactly as
    /// [`set_value`](Self::set_value) finds no cell, so no band is allocated for them.
    fn set_values_mapped(&mut self, values: &[(CellRef, Value)]) {
        let (Some(first), Some(last)) = (values.first(), values.last()) else { return };
        let mut i = 0;
        for segment in self.row_map.segments_in(first.0.row, last.0.row) {
            // The values before this segment start on logical rows the map does not cover.
            while i < values.len() && values[i].0.row < segment.logical {
                i += 1;
            }
            let physical = |row: u32| segment.physical.saturating_add(row.saturating_sub(segment.logical));
            let stop = i + values[i..].partition_point(|(c, _)| c.row < segment.logical_end());
            while i < stop {
                let key = physical(values[i].0.row) / BAND;
                let run_end = i + values[i..stop].partition_point(|(c, _)| physical(c.row) / BAND == key);
                // A corrupt page skips the run it would have held, not the batch.
                if self.pages.contains_key(&key) && self.materialize(key).is_none() {
                    i = run_end;
                    continue;
                }
                let col_map = &self.col_map;
                if let Some(band) = self.bands.get_mut(&key) {
                    let band = Arc::make_mut(band);
                    let first_row = values[i].0.row;
                    let row0 = physical(first_row) % BAND;
                    for (c, v) in &values[i..run_end] {
                        if let Some(col) = col_map.map(c.col) {
                            band.set_value(row0 + (c.row - first_row), col, v);
                        }
                    }
                }
                i = run_end;
            }
            if i >= values.len() {
                break;
            }
        }
    }

    fn set_values_sequential(&mut self, values: &[(CellRef, Value)]) {
        let mut i = 0;
        while i < values.len() {
            let key = values[i].0.row / BAND;
            let end = values[i..].partition_point(|(c, _)| c.row / BAND == key) + i;
            if self.pages.contains_key(&key) && self.materialize(key).is_none() {
                i = end;
                continue;
            }
            if let Some(band) = self.bands.get_mut(&key) {
                let band = Arc::make_mut(band);
                for (c, v) in &values[i..end] {
                    band.set_value(c.row % BAND, c.col, v);
                }
            }
            i = end;
        }
    }

    /// The value-only write on a mapped store, split over threads. The plan is the list of band
    /// runs the logical batch covers: sorted by band key and merged, so a band that two row
    /// segments feed becomes one task and its writes cannot overwrite each other. Paged bands are
    /// materialized once here, before the workers look at the live map; a corrupt page drops its
    /// task, not the batch. Returns false when the plan would be larger than the batch itself (or
    /// a worker panics): the caller then streams the batch sequentially.
    #[cfg(not(target_arch = "wasm32"))]
    fn set_values_mapped_parallel(&mut self, values: &[(CellRef, Value)], threads: usize) -> bool {
        let (Some(first), Some(last)) = (values.first(), values.last()) else { return true };
        let mut runs: Vec<BandRun> = Vec::new();
        let mut i = 0;
        for segment in self.row_map.segments_in(first.0.row, last.0.row) {
            while i < values.len() && values[i].0.row < segment.logical {
                i += 1;
            }
            let physical = |row: u32| segment.physical.saturating_add(row.saturating_sub(segment.logical));
            let stop = i + values[i..].partition_point(|(c, _)| c.row < segment.logical_end());
            while i < stop {
                let key = physical(values[i].0.row) / BAND;
                let run_end = i + values[i..stop].partition_point(|(c, _)| physical(c.row) / BAND == key);
                runs.push(BandRun { key, base: physical(values[i].0.row), start: i, end: run_end });
                // A batch that touches a band per value (a sparse sheet after a shift) would make
                // the plan as large as the batch; streaming does the same writes with no plan.
                if runs.len() > values.len() / 4 {
                    return false;
                }
                i = run_end;
            }
            if i >= values.len() {
                break;
            }
        }
        runs.sort_unstable_by_key(|run| run.key);
        let mut tasks: Vec<(u32, Vec<BandRun>)> = Vec::new();
        for run in runs {
            match tasks.last_mut() {
                Some((key, list)) if *key == run.key => list.push(run),
                _ => tasks.push((run.key, vec![run])),
            }
        }
        // A paged band must be live before a worker copies it; one that fails to decode is
        // skipped entirely, as in the sequential mapped path.
        let mut live: Vec<(u32, Vec<BandRun>)> = Vec::with_capacity(tasks.len());
        for (key, list) in tasks {
            if self.pages.contains_key(&key) && self.materialize(key).is_none() {
                continue;
            }
            live.push((key, list));
        }
        if live.is_empty() {
            return true;
        }
        let bands = &self.bands;
        let col_map = &self.col_map;
        let chunk = live.len().div_ceil(threads);
        let results: Vec<std::thread::Result<Vec<(u32, Arc<Band>)>>> = std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for part in live.chunks(chunk) {
                handles.push(scope.spawn(move || {
                    let mut out = Vec::with_capacity(part.len());
                    for (key, list) in part {
                        let Some(old) = bands.get(key) else { continue };
                        let mut band = (**old).clone();
                        for run in list {
                            let Some((first_cell, _)) = values.get(run.start) else { continue };
                            let Some(slice) = values.get(run.start..run.end) else { continue };
                            let row0 = run.base % BAND;
                            for (c, v) in slice {
                                if let Some(col) = col_map.map(c.col) {
                                    band.set_value(row0 + (c.row - first_cell.row), col, v);
                                }
                            }
                        }
                        out.push((*key, Arc::new(band)));
                    }
                    out
                }));
            }
            handles.into_iter().map(|h| h.join()).collect()
        });
        let mut new_bands = Vec::with_capacity(live.len());
        for result in results {
            match result {
                Ok(part) => new_bands.extend(part),
                // A worker panicked: nothing has reached the store yet, so the sequential path can
                // still do the whole batch.
                Err(_) => return false,
            }
        }
        // One task a band, so no insert can overwrite another run's writes.
        for (key, band) in new_bands {
            self.bands.insert(key, band);
        }
        true
    }

    /// The value-only write, split over threads when the batch is large. A band is independent of
    /// every other (its numbers, texts and stripes are its own), so each worker clones the bands it
    /// was given, applies their values and hands the new bands back; the store replaces them only
    /// when every worker finished. The old bands stay with the undo snapshot.
    #[cfg(not(target_arch = "wasm32"))]
    fn set_values_parallel(&mut self, values: &[(CellRef, Value)], threads: usize) {
        let mut groups: Vec<(u32, usize, usize)> = Vec::new();
        let mut i = 0;
        while i < values.len() {
            let key = values[i].0.row / BAND;
            let end = values[i..].partition_point(|(c, _)| c.row / BAND == key) + i;
            groups.push((key, i, end));
            i = end;
        }
        let bands = &self.bands;
        let chunk = groups.len().div_ceil(threads);
        let results: Vec<std::thread::Result<Vec<(u32, Arc<Band>)>>> = std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for part in groups.chunks(chunk) {
                handles.push(scope.spawn(move || {
                    let mut out = Vec::with_capacity(part.len());
                    for &(key, start, end) in part {
                        let Some(old) = bands.get(&key) else { continue };
                        let mut band = (**old).clone();
                        for (c, v) in &values[start..end] {
                            band.set_value(c.row % BAND, c.col, v);
                        }
                        out.push((key, Arc::new(band)));
                    }
                    out
                }));
            }
            handles.into_iter().map(|h| h.join()).collect()
        });
        let mut new_bands = Vec::with_capacity(groups.len());
        for result in results {
            match result {
                Ok(part) => new_bands.extend(part),
                // A worker panicked: nothing has reached the store yet, so the sequential path can
                // still do the whole batch.
                Err(_) => {
                    self.set_values_sequential(values);
                    return;
                }
            }
        }
        for (key, band) in new_bands {
            self.bands.insert(key, band);
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
        if !self.col_map.is_identity() {
            let mut mapped = Vec::with_capacity(cells.len());
            for (col, cell) in cells.drain(..) {
                if let Some(physical) = self.physical_col(col).or_else(|| self.physical_col_for_write(col)) {
                    mapped.push((physical, cell));
                }
            }
            mapped.sort_unstable_by_key(|(c, _)| *c);
            mapped.dedup_by(|a, b| {
                a.0 == b.0 && {
                    b.1 = a.1.clone();
                    true
                }
            });
            *cells = mapped;
        }
        // A serialized band may carry a row outside the sheet grid; keep it physical so `rebuild`
        // can find and report it rather than silently losing it (#1008).
        let physical = if row >= gridcraft_core::MAX_ROWS {
            row
        } else {
            let Some(physical) = self.physical_row_for_write(row) else { return };
            physical
        };
        let key = physical / BAND;
        let band_row = physical % BAND;
        if self.pages.contains_key(&key) && self.materialize(key).is_none() {
            return;
        }
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
        // Physical order, not logical: a deserialized off-grid row has no logical mapping, and
        // that is exactly what `rebuild` is here to find and drop.
        let cells: Vec<(CellRef, Cell)> = self.raw_cells();
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

    /// Every stored cell at its physical key, ignoring the row map. Used by `rebuild` to see
    /// rows a serialized file put outside the grid.
    fn raw_cells(&self) -> Vec<(CellRef, Cell)> {
        let mut keys: Vec<u32> = self.bands.keys().chain(self.pages.keys()).copied().collect();
        keys.sort_unstable();
        keys.dedup();
        let mut out = Vec::new();
        for key in keys {
            let Some(band) = self.band_arc(key) else { continue };
            for row in &band.rows {
                let at = key * BAND + row.key;
                out.extend(packed_row(&band, at, row, 0, u32::MAX));
            }
        }
        out
    }

    pub fn remove(&mut self, c: CellRef) -> Option<Cell> {
        let physical = self.physical_row(c.row)?;
        let col = self.physical_col(c.col)?;
        let key = physical / BAND;
        if self.pages.contains_key(&key) && self.materialize(key).is_none() {
            return None;
        }
        let band = self.bands.get_mut(&key)?;
        let cell = Arc::make_mut(band).take(physical % BAND, col)?;
        self.len -= 1;
        if self.bands.get(&key).is_some_and(|b| b.rows.is_empty()) {
            self.bands.remove(&key);
        }
        Some(cell)
    }

    /// The cells of `r` that carry a formula, in row-major order.
    pub fn formula_cells(&self, r: RangeRef) -> impl Iterator<Item = CellRef> + '_ {
        let mut out = Vec::new();
        // A whole-column range is millions of rows over a few thousand bands: resolve a band's
        // page and `Arc` once, not once per row. Rows arrive in physical order inside a segment,
        // so the last key covers every row of its band.
        let mut current: Option<(u32, Option<Arc<Band>>)> = None;
        for segment in self.row_map.segments_in(r.start.row, r.end.row) {
            for offset in 0..segment.len {
                let logical = segment.logical.saturating_add(offset);
                let physical = segment.physical.saturating_add(offset);
                let key = physical / BAND;
                if current.as_ref().is_none_or(|(k, _)| *k != key) {
                    // A page that says it holds no formula is skipped without decoding it.
                    let band = if self.pages.get(&key).is_some_and(|page| !page.has_formulas()) { None } else { self.band_arc(key) };
                    current = Some((key, band));
                }
                let Some(band) = current.as_ref().and_then(|(_, band)| band.as_ref()) else { continue };
                let Some(row) = band.row(physical % BAND) else { continue };
                for (col, _) in row.formulas() {
                    let Some(logical_col) = self.col_map.inverse(*col) else { continue };
                    if logical_col >= r.start.col && logical_col <= r.end.col {
                        out.push(CellRef::new(logical, logical_col));
                    }
                }
            }
        }
        out.into_iter()
    }

    /// Whether the block holds a cell whose value the store cannot give: a formula (it holds the
    /// value the file cached) or an array. Cheap next to reading the block: one check a row.
    pub fn has_owned_cells(&self, r: RangeRef) -> bool {
        // Same band cache as `formula_cells`: every whole-column `SUM`/`COUNT` pays this gate
        // before folding, and resolving the band per row cost 25 ms a million rows.
        let mut current: Option<(u32, Option<Arc<Band>>)> = None;
        for segment in self.row_map.segments_in(r.start.row, r.end.row) {
            for offset in 0..segment.len {
                let physical = segment.physical.saturating_add(offset);
                let key = physical / BAND;
                if current.as_ref().is_none_or(|(k, _)| *k != key) {
                    // A page that says it holds no formula or array is not decoded at all.
                    let band = if self.pages.get(&key).is_some_and(|page| !page.has_owned()) { None } else { self.band_arc(key) };
                    current = Some((key, band));
                }
                let Some(band) = current.as_ref().and_then(|(_, band)| band.as_ref()) else { continue };
                let Some(row) = band.row(physical % BAND) else { continue };
                let owned = row.extras.as_ref().is_some_and(|e| {
                    e.formulas.iter().any(|(c, _)| self.col_map.inverse(*c).is_some_and(|col| (r.start.col..=r.end.col).contains(&col)))
                        || e.arrays.iter().any(|(c, _)| self.col_map.inverse(*c).is_some_and(|col| (r.start.col..=r.end.col).contains(&col)))
                });
                if owned {
                    return true;
                }
            }
        }
        false
    }

    /// Calls `f` on each stored cell of `r`, row-major, handing the value as the page holds it, and
    /// `None` for a cell whose value the evaluator owns (a formula, an array, or a blank one).
    ///
    /// Reading a range through `iter_range` builds a `Cell` an iteration — a `Value`, a style, and
    /// two `Arc` bumps — which costs ~77 ns a cell against ~2 ns for the numbers themselves. Summing
    /// a million-cell column spent all of its 154 ms there.
    ///
    /// Columns are walked from the column map's runs, so a column-mapped sheet stays on this path:
    /// `segments_in` hands out clipped, physical-contiguous, logically ordered runs — one when the
    /// map is identity — and a run's logical column is `logical + (physical - first)`.
    pub fn visit_packed(&self, r: RangeRef, f: &mut dyn FnMut(CellRef, Option<Packed<'_>>) -> bool) -> bool {
        let col_runs = self.col_map.segments_in(r.start.col, r.end.col);
        // A sheet with many column edits would repeat the row walk once per column run; past a
        // handful of runs build each cell once instead, so the work stays bounded by the range.
        if col_runs.len() > MAX_PACKED_COLUMN_SEGMENTS {
            return self.visit_packed_by_iter(r, f);
        }
        let (r0, r1) = (r.start.row, r.end.row);
        let mut current_key = None;
        let mut current_band: Option<Arc<Band>> = None;
        for segment in self.row_map.segments_in(r0, r1) {
            for offset in 0..segment.len {
                let at_row = segment.logical.saturating_add(offset);
                let physical = segment.physical.saturating_add(offset);
                if at_row < r0 || at_row > r1 {
                    continue;
                }
                let key = physical / BAND;
                if current_key != Some(key) {
                    current_key = Some(key);
                    current_band = self.band_arc(key);
                }
                let Some(band) = current_band.as_ref() else { continue };
                let Some(row) = band.row(physical % BAND) else { continue };
                for columns in &col_runs {
                    let last = columns.physical.saturating_add(columns.len).saturating_sub(1);
                    if columns.physical > last {
                        continue;
                    }
                    if !self.visit_packed_row(band, row, at_row, columns.logical, columns.physical, last, f) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Walks the cells of one stored row that fall in the physical column run `[first, last]`,
    /// handing each out at its logical column. Returns false when `f` stopped the walk.
    #[allow(clippy::too_many_arguments)]
    fn visit_packed_row(
        &self,
        band: &Band,
        row: &BandRow,
        at_row: u32,
        logical: u32,
        first: u32,
        last: u32,
        f: &mut dyn FnMut(CellRef, Option<Packed<'_>>) -> bool,
    ) -> bool {
        // Start at the first stored column of the range and stop past its last, so a narrow read
        // of a wide row costs the cells it asks for, not the row's width.
        let lo = first.max(row.first);
        let hi = last.min(row.first.saturating_add(row.columns()).saturating_sub(1));
        if lo > hi {
            return true;
        }
        let start_slot = (lo - row.first) as usize;
        // A cell that carries a formula stores the value the file cached, which is not its value
        // now: the evaluator owns it, so hand it over as `None`.
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
                    // Past this run's last wanted column; the next column run may still be inside.
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
                if !f(CellRef::new(at_row, logical.saturating_add(col - first)), packed) {
                    return false;
                }
            }
        }
        true
    }

    /// The packed walk built from `iter_range` cells, for a column map with more runs than
    /// [`MAX_PACKED_COLUMN_SEGMENTS`]: one `Cell` per cell, but no repeated row walk.
    fn visit_packed_by_iter(&self, r: RangeRef, f: &mut dyn FnMut(CellRef, Option<Packed<'_>>) -> bool) -> bool {
        for (at, cell) in self.iter_range(r) {
            let packed = if cell.formula.is_some() {
                None
            } else {
                match &cell.value {
                    gridcraft_core::Value::Number(n) => Some(Packed::Number(*n)),
                    gridcraft_core::Value::Text(t) => Some(Packed::Text(t.as_ref())),
                    gridcraft_core::Value::Bool(b) => Some(Packed::Bool(*b)),
                    gridcraft_core::Value::Error(e) => Some(Packed::Error(*e)),
                    gridcraft_core::Value::Array(_) => None,
                    gridcraft_core::Value::Empty => continue,
                }
            };
            if !f(at, packed) {
                return false;
            }
        }
        true
    }

    pub fn iter_range(&self, r: RangeRef) -> impl Iterator<Item = (CellRef, Cell)> + '_ {
        CellWalk::new(self, r)
    }

    pub fn iter(&self) -> impl Iterator<Item = (CellRef, Cell)> + '_ {
        CellWalk::new(self, RangeRef::new(CellRef::new(0, 0), CellRef::new(u32::MAX, u32::MAX)))
    }

    pub fn row(&self, row: u32, c0: u32, c1: u32) -> impl Iterator<Item = (u32, Cell)> + '_ {
        let mut out = Vec::new();
        let Some(physical) = self.physical_row(row) else { return out.into_iter() };
        let Some(band) = self.band_arc(physical / BAND) else { return out.into_iter() };
        let Some(cells) = band.row(physical % BAND) else { return out.into_iter() };
        for columns in self.col_map.segments_in(c0, c1) {
            let first = columns.physical;
            let last = columns.physical.saturating_add(columns.len).saturating_sub(1);
            if first > last {
                continue;
            }
            out.extend(packed_row(&band, row, cells, first, last).filter_map(|(at, cell)| {
                let col = columns.logical.saturating_add(at.col - first);
                (col >= c0 && col <= c1).then_some((col, cell))
            }));
        }
        out.into_iter()
    }

    pub fn used_range(&self) -> Option<RangeRef> {
        let mut r0 = u32::MAX;
        let mut c0 = u32::MAX;
        let mut r1 = 0u32;
        let mut c1 = 0u32;
        for (bk, band) in &self.bands {
            for cells in &band.rows {
                let Some((first, last)) = cells.column_span(band) else { continue };
                let physical = bk * BAND + cells.key;
                let Some(logical) = self.row_map.inverse(physical) else { continue };
                let Some((logical_first, logical_last)) = self.logical_col_span(first, last) else { continue };
                r0 = r0.min(logical);
                r1 = r1.max(logical);
                c0 = c0.min(logical_first);
                c1 = c1.max(logical_last);
            }
        }
        for (bk, page) in &self.pages {
            let Some((pr0, pr1, pc0, pc1)) = page.bounds() else { continue };
            for row in pr0..=pr1 {
                let Some(logical) = self.row_map.inverse(bk * BAND + row) else { continue };
                r0 = r0.min(logical);
                r1 = r1.max(logical);
            }
            if let Some((logical_first, logical_last)) = self.logical_col_span(pc0, pc1) {
                c0 = c0.min(logical_first);
                c1 = c1.max(logical_last);
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
    pub fn formulas(&self) -> impl Iterator<Item = (CellRef, Arc<Formula>, bool)> + '_ {
        let mut keys: Vec<u32> = self.bands.keys().chain(self.pages.keys()).copied().collect();
        keys.sort_unstable();
        keys.dedup();
        let mut out = Vec::new();
        for key in keys {
            if self.pages.get(&key).is_some_and(|page| !page.has_formulas()) {
                continue;
            }
            let Some(band) = self.band_arc(key) else { continue };
            for cells in &band.rows {
                if cells.formulas().is_empty() {
                    continue;
                }
                let Some(logical) = self.row_map.inverse(key * BAND + cells.key) else { continue };
                for (col, f) in cells.formulas() {
                    let Some(logical_col) = self.col_map.inverse(*col) else { continue };
                    let has_value = cells.index_of(&band, *col).is_some_and(|i| cells.kind_at(&band, i) != EMPTY);
                    out.push((CellRef::new(logical, logical_col), f.clone(), has_value));
                }
            }
        }
        out.sort_unstable_by_key(|(c, _, _)| (c.row, c.col));
        out.into_iter()
    }

    /// A stored cell with a value or a formula (a style alone does not count).
    pub fn has(&self, c: CellRef) -> bool {
        let (Some(physical), Some(col)) = (self.physical_row(c.row), self.physical_col(c.col)) else { return false };
        self.band_arc(physical / BAND).is_some_and(|b| b.row(physical % BAND).is_some_and(|row| row_has_value(&b, row, col)))
    }

    /// Last row (inclusive, capped at `cap`) of the maximal run of contiguous rows at or after
    /// `from.row` that store a cell in `from.col` with a non-empty value. See `Sheet::value_ref`.
    pub fn contiguous_value_run(&self, from: CellRef, cap: u32) -> Option<u32> {
        let mut last = None;
        let mut expect = from.row;
        let col = self.physical_col(from.col)?;
        while expect <= cap {
            let Some(physical) = self.physical_row(expect) else { break };
            let Some(band) = self.band_arc(physical / BAND) else { break };
            let Some(row) = band.row(physical % BAND) else { break };
            if !row.value_nonempty(&band, col) {
                return last;
            }
            last = Some(expect);
            expect += 1;
        }
        last
    }

    /// Positions whose cells differ between `self` and `other`.
    pub fn diff(&self, other: &CellStore) -> Vec<CellRef> {
        let mut keys: Vec<u32> = self.bands.keys().chain(self.pages.keys()).chain(other.bands.keys()).chain(other.pages.keys()).copied().collect();
        keys.sort_unstable();
        keys.dedup();
        let mut out = Vec::new();
        for k in keys {
            let (a, b) = (self.band_arc(k), other.band_arc(k));
            if let (Some(a), Some(b)) = (&a, &b)
                && Arc::ptr_eq(a, b)
            {
                continue;
            }
            let band = k * BAND;
            let mut rows: Vec<u32> = Vec::new();
            if let Some(a) = a.as_deref() {
                rows.extend(a.rows.iter().map(|r| r.key));
            }
            if let Some(b) = b.as_deref() {
                rows.extend(b.rows.iter().map(|r| r.key));
            }
            rows.sort_unstable();
            rows.dedup();
            for row in rows {
                let (ra, rb) = (a.as_deref().and_then(|x| x.row(row)), b.as_deref().and_then(|x| x.row(row)));
                let mut cols: Vec<u32> = Vec::new();
                if let (Some(a), Some(ra)) = (a.as_deref(), ra) {
                    cols.extend(packed_row(a, band + row, ra, 0, u32::MAX).map(|(c, _)| c.col));
                }
                if let (Some(b), Some(rb)) = (b.as_deref(), rb) {
                    cols.extend(packed_row(b, band + row, rb, 0, u32::MAX).map(|(c, _)| c.col));
                }
                cols.sort_unstable();
                cols.dedup();
                for col in cols {
                    let x = a.as_deref().and_then(|x| ra.and_then(|r| r.index_of(x, col).map(|i| (x, r, i))));
                    let y = b.as_deref().and_then(|y| rb.and_then(|r| r.index_of(y, col).map(|i| (y, r, i))));
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
                        let physical = band + row;
                        if let Some(logical) = self.row_map.inverse(physical).or_else(|| other.row_map.inverse(physical))
                            && let Some(logical_col) = self.col_map.inverse(col).or_else(|| other.col_map.inverse(col))
                        {
                            out.push(CellRef::new(logical, logical_col));
                        }
                    }
                }
            }
        }
        out
    }

    /// Rough heap storage kept alive by this snapshot's bands that `newer` no longer shares.
    pub fn retained_bytes_against(&self, newer: &CellStore) -> usize {
        let mut bytes = 0usize;
        for (key, page) in &self.pages {
            if newer.pages.get(key).is_some_and(|next| Arc::ptr_eq(next, page)) {
                continue;
            }
            bytes += page.page_len();
        }
        for (key, band) in &self.bands {
            let next = newer.bands.get(key);
            if next.is_some_and(|b| Arc::ptr_eq(b, band)) {
                continue;
            }
            bytes += std::mem::size_of::<Band>();
            bytes += band.bits.capacity() * 8;
            bytes += band.kinds.capacity() * 4;
            bytes += band.payloads.capacity() * 2;
            bytes += band.styles.capacity() * 4;
            for row in &band.rows {
                bytes += std::mem::size_of::<BandRow>();
                // A value edit copies the band but shares each row's extras with the newer store;
                // those formulas and arrays are live anyway, not storage the snapshot keeps.
                let shared = next.and_then(|b| b.row(row.key)).is_some_and(|n| match (&row.extras, &n.extras) {
                    (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                    (None, None) => true,
                    _ => false,
                });
                if shared {
                    continue;
                }
                bytes += row.formulas().len() * (4 + std::mem::size_of::<Formula>()) + row.arrays().len() * 8;
                for f in row.formulas().iter().map(|(_, f)| f) {
                    // The parsed tree's size is counted when the formula is parsed: walking every
                    // formula of every band an edit touched was most of what an undo entry cost.
                    bytes += f.text.len() + f.expr_bytes as usize;
                }
            }
            bytes += band.numbers.capacity() * 8 + band.number_slots.capacity() * 2;
            for t in &band.texts {
                bytes += t.len() + std::mem::size_of::<Arc<str>>();
            }
        }
        bytes
    }

    /// Bytes the packing itself holds, split by what holds them. Diagnostics for the memory work,
    /// not an exact figure: allocator rounding is not visible from here.
    pub fn packed_breakdown(&self) -> [usize; 8] {
        use std::mem::size_of;
        // rows, bits, kinds, payloads, styles, texts, formulas+arrays+style ids, band overhead
        // (the struct, the number table and its slots)
        let mut out = [0usize; 8];
        for band in self.bands.values() {
            out[7] += size_of::<Band>();
            out[7] += band.numbers.capacity() * size_of::<f64>() + band.number_slots.capacity() * size_of::<u16>();
            out[6] += band.style_ids.capacity() * size_of::<StyleId>();
            out[5] += band.texts.capacity() * size_of::<Arc<str>>() + band.text_index.capacity() * size_of::<(usize, u32)>();
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

    /// How many bands the store holds: the unit paging will work in, and what tells a probe whether
    /// a file's cells are sparse across rows or packed into a few.
    pub fn band_count(&self) -> usize {
        self.bands.len().saturating_add(self.pages.len())
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

    /// Reclaims cells that no logical row or column can reach, and normalizes the maps.
    ///
    /// A structural edit changes the row/column map instead of moving cells, so a shift that is
    /// not paired with a delete can leave cells at physical rows or columns whose inverse is gone.
    /// Those cells are invisible to `get`, `iter` and `used_range` but still hold storage and
    /// count in `len`. This walks the live bands and, when a gap in either map could hold one, the
    /// pages that straddle it, drops each unreachable cell and every band that empties, then
    /// merges the map runs, bounds the column free list and returns a contiguous map to the
    /// identity fast path.
    ///
    /// With both maps identity every physical row and column has an inverse, so nothing can be an
    /// orphan and the call returns at once: a clean store (and every freshly opened one) pays
    /// O(1). Pages are found through the *gaps* of the maps — the physical runs no logical row or
    /// column covers — so a row delete probes the band keys of the deleted rows instead of
    /// decoding the sheet. A page is decoded without priming its cache and is only cloned,
    /// rewritten (appended to the page file) and replaced when it actually held an orphan, so a
    /// snapshot keeps reading its own `Arc<Page>` and its old offsets. A page that cannot be
    /// decoded leaves the pass incomplete, which keeps the maps from resetting to identity.
    pub fn compact(&mut self) -> Compacted {
        if self.row_map.is_identity() && self.col_map.is_identity() {
            return Compacted::default();
        }
        let shape = self.map_shape();
        let mut compacted = Compacted::default();
        let keys: Vec<u32> = self.bands.keys().copied().collect();
        for key in keys {
            if self.bands.get(&key).is_some_and(|band| band.rows.is_empty()) {
                self.bands.remove(&key);
                compacted.bands = compacted.bands.saturating_add(1);
                continue;
            }
            // Look before copying: a band with nothing to reclaim keeps sharing its `Arc` with the
            // undo snapshot, so a second pass (or a pass over a clean store) copies no band.
            let (gone, clear_rows, drop_cells) = match self.bands.get(&key) {
                Some(band) => Self::orphan_plan(band, key, &self.row_map, &self.col_map),
                None => continue,
            };
            if gone == 0 {
                continue;
            }
            let Some(band) = self.bands.get_mut(&key) else { continue };
            let band = Arc::make_mut(band);
            for row_key in clear_rows {
                band.remove_row(row_key);
            }
            for (row_key, col) in drop_cells {
                let _ = band.take(row_key, col);
            }
            compacted.cells = compacted.cells.saturating_add(gone);
            if band.rows.is_empty() {
                self.bands.remove(&key);
                compacted.bands = compacted.bands.saturating_add(1);
            }
        }
        // A page can hold a cell at a position no logical row or column covers; once every page
        // that could is checked, a single run from the origin is identity for every cell left.
        let complete = self.compact_pages(&mut compacted);
        self.len = self.len.saturating_sub(compacted.cells);
        self.row_map.normalize(complete);
        self.col_map.normalize(complete);
        compacted.changed = compacted.cells > 0 || compacted.bands > 0 || compacted.pages > 0 || self.map_shape() != shape;
        compacted
    }

    /// Whether [`compact`](Self::compact) could reclaim anything. `compact` returns at once when
    /// both maps are the identity, and this is that same test, so a caller can use it to skip a
    /// copy of the workbook.
    pub fn needs_compaction(&self) -> bool {
        !self.row_map.is_identity() || !self.col_map.is_identity()
    }

    /// The page half of [`compact`](Self::compact): visits the pages a gap in either map could
    /// leave an orphan in, rewrites the ones that hold one and drops the ones that emptied.
    /// Returns true when every candidate was read and handled (or there were none): only then is
    /// the store free of orphans and a contiguous map may return to the identity fast path.
    fn compact_pages(&mut self, compacted: &mut Compacted) -> bool {
        let mut candidates: BTreeSet<u32> = BTreeSet::new();
        for (lo, hi) in Self::unmapped_runs(self.row_map.is_identity(), &self.row_map.by_physical, self.row_map.next_physical) {
            let (first, last) = (lo / BAND, hi.saturating_sub(1) / BAND);
            candidates.extend(self.pages.range(first..=last).map(|(key, _)| *key));
        }
        if !self.col_map.is_identity() {
            // Bounds are a span over physical columns, so this can over-select a page whose cells
            // sit on both sides of a dropped column; the plan below then finds nothing and the
            // page is left as it was. Only the bounds are read here, never the page.
            let gaps = Self::unmapped_runs(false, &self.col_map.by_physical, self.col_map.next_physical);
            if !gaps.is_empty() {
                candidates.extend(
                    self.pages
                        .iter()
                        .filter(|(_, page)| {
                            page.bounds().is_some_and(|(_, _, first, last)| gaps.iter().any(|(lo, hi)| first <= hi.saturating_sub(1) && *lo <= last))
                        })
                        .map(|(key, _)| *key),
                );
            }
        }
        let mut complete = true;
        let mut rewritten = 0usize;
        for key in candidates {
            let Some(page) = self.pages.get(&key).cloned() else { continue };
            let Some(file) = self.page_file.clone() else {
                // A page without its file cannot be read; keep it and report the pass incomplete.
                complete = false;
                continue;
            };
            // Decode without priming the page cache: a pass over many candidates must not pin a
            // band per page in a queue that would never evict it.
            let Some(band) = page.decode(&file) else {
                complete = false;
                continue;
            };
            let (gone, clear_rows, drop_cells) = Self::orphan_plan(&band, key, &self.row_map, &self.col_map);
            if gone == 0 {
                continue;
            }
            let mut band = Arc::unwrap_or_clone(band);
            for row_key in clear_rows {
                band.remove_row(row_key);
            }
            for (row_key, col) in drop_cells {
                let _ = band.take(row_key, col);
            }
            if band.rows.is_empty() {
                self.pages.remove(&key);
                compacted.cells = compacted.cells.saturating_add(gone);
                compacted.pages = compacted.pages.saturating_add(1);
                continue;
            }
            let Some(bytes) = page::encode_band(&band) else {
                complete = false;
                continue;
            };
            let Some(at) = file.write(&bytes) else {
                complete = false;
                continue;
            };
            rewritten = rewritten.saturating_add(1);
            let page = page::Page::from_band(at, Arc::new(band));
            // A few rewritten pages are worth keeping decoded for the read that follows a save. A
            // column-gap pass can over-select and a big pass would pin a band per page, so those
            // are left for the page cache to decode once.
            if !(self.col_map.is_identity() && rewritten <= PAGE_PRIME_LIMIT) {
                page.clear();
            }
            self.pages.insert(key, Arc::new(page));
            compacted.cells = compacted.cells.saturating_add(gone);
            compacted.pages = compacted.pages.saturating_add(1);
        }
        complete
    }

    /// The physical runs `[lo, hi)` that no logical row or column covers: the holes between the
    /// mapped runs, from the origin to the watermark of physical keys the map has handed out.
    /// Nothing is stored past that watermark, so these runs bound every cell that could be an
    /// orphan.
    fn unmapped_runs(identity: bool, by_physical: &[RowSegment], next_physical: u32) -> Vec<(u32, u32)> {
        if identity {
            return Vec::new();
        }
        let mut runs = Vec::new();
        let mut at = 0u32;
        for segment in by_physical {
            if segment.physical > at {
                runs.push((at, segment.physical));
            }
            at = at.max(segment.physical.saturating_add(segment.len));
        }
        if next_physical > at {
            runs.push((at, next_physical));
        }
        runs
    }

    /// A cheap fingerprint of the two maps: `Compacted::changed` needs to know whether
    /// normalization merged runs, returned a map to identity or trimmed the column free list.
    fn map_shape(&self) -> (bool, usize, bool, usize, usize) {
        (self.row_map.is_identity(), self.row_map.segments.len(), self.col_map.is_identity(), self.col_map.segments.len(), self.col_map.free.len())
    }

    /// The rows to clear and the cells to drop of one live band whose physical row or column has
    /// no logical inverse: `(cells, rows, (row, column))`. Rows are visited by key, the cells of a
    /// mapped row by position; the grid is never walked.
    fn orphan_plan(band: &Band, key: u32, rows: &RowMap, cols: &ColMap) -> (usize, Vec<u32>, Vec<(u32, u32)>) {
        let mut removed = 0usize;
        let mut clear_rows: Vec<u32> = Vec::new();
        let mut drop_cells: Vec<(u32, u32)> = Vec::new();
        for row in &band.rows {
            let physical = key.saturating_mul(BAND).saturating_add(row.key);
            if rows.inverse(physical).is_none() {
                removed = removed.saturating_add(row.cells as usize);
                clear_rows.push(row.key);
                continue;
            }
            // An identity column map maps every column in the grid; only a cell past the last grid
            // column can be orphaned, so the per-cell walk is skipped for a row that stops short.
            let scan = !cols.is_identity() || row.column_span(band).is_some_and(|(_, last)| last >= gridcraft_core::MAX_COLS);
            if !scan {
                continue;
            }
            for i in 0..row.cells as usize {
                let col = row.col_at(band, i);
                if cols.inverse(col).is_none() {
                    removed = removed.saturating_add(1);
                    drop_cells.push((row.key, col));
                }
            }
        }
        (removed, clear_rows, drop_cells)
    }

    /// Rows at or after `from` moved by `delta` (positive = down). This changes the row map, not
    /// the bands: an insert above a million rows is a handful of segments, not a million cell
    /// moves.
    pub fn shift_rows(&mut self, from: u32, delta: i64) {
        self.row_map.shift(from, delta);
    }

    pub fn shift_cols(&mut self, from: u32, delta: i64) {
        self.ensure_col_map();
        self.col_map.shift(from, delta);
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

/// A cell's content for logical equality: the parsed formula tree and its cached byte count are
/// derived (one side may have reparsed), so compare the text and the array/dynamic flags.
fn same_cell(a: &Cell, b: &Cell) -> bool {
    if a.value != b.value || a.style != b.style {
        return false;
    }
    match (&a.formula, &b.formula) {
        (None, None) => true,
        (Some(a), Some(b)) => a.text == b.text && a.array == b.array && a.dynamic == b.dynamic,
        _ => false,
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

    /// The same equality on a mapped sheet: a column-mapped store walks the column map's runs, so
    /// the cells come out at their logical columns and the whole range stays row-major.
    #[test]
    fn packed_walk_matches_iter_range_on_a_column_mapped_sheet() {
        for (rows, cols) in [(0u32, 2u32), (3, 0), (3, 2)] {
            let mut s = store();
            if rows > 0 {
                s.shift_rows(3, i64::from(rows));
            }
            if cols > 0 {
                s.shift_cols(1, i64::from(cols));
            }
            let mut ranges = vec![
                // A single cell, then a range that crosses the column map's segment boundary.
                (0u32, 0u32, 0u32, 0u32),
                (0, 0, 0, 2 + cols),
                // A whole row, and a range that starts and ends inside one segment.
                (0, 0, 0, u32::MAX),
                (0, 3 + cols, 64 + cols, 64 + cols),
                // A range that spans both axes' segment boundaries, and one past the last row.
                (0, 400, 0, 200 + cols),
                (rows, rows + 200, 0, 100 + cols),
                (0, 400, 71 + cols, 200 + cols),
            ];
            // The inserted columns themselves hold nothing.
            if cols > 0 {
                ranges.push((0, 0, 1, cols));
            }
            for (r0, r1, c0, c1) in ranges {
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
                assert_eq!(got, want, "range {} with {rows} rows and {cols} cols inserted", range.a1());
                assert!(got.windows(2).all(|w| (w[0].0.row, w[0].0.col) < (w[1].0.row, w[1].0.col)), "range {} is not row-major", range.a1());
            }
        }
    }

    /// The value-less slot a `BandBatch` row can hold must stay invisible on a column-mapped
    /// sheet too: it is neither a number nor a formula the evaluator owns.
    #[test]
    fn packed_walk_on_a_column_map_skips_an_empty_slot() {
        let mut batch = BandBatch::new();
        let mut cells = vec![(3u32, plain(1)), (4, Cell { value: Value::Empty, formula: None, style: StyleId(0) }), (5, plain(3))];
        batch.set_row(9, &mut cells);
        let mut s = CellStore::new();
        s.insert_batch(batch);
        s.shift_cols(1, 2);
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

    /// Past the column-run cap the walk builds cells through `iter_range` instead of repeating
    /// the row walk once per run; the answer must not change.
    #[test]
    fn packed_walk_falls_back_past_the_segment_cap() {
        let mut s = CellStore::new();
        for col in 0..40u32 {
            s.set(CellRef::new(2, col), plain(col + 1));
        }
        for at in [1u32, 5, 9, 13, 17, 21, 25, 29, 33] {
            s.shift_cols(at, 1);
        }
        assert!(s.col_map.segments_in(0, u32::MAX).len() > MAX_PACKED_COLUMN_SEGMENTS, "the fallback is the path under test");
        let range = RangeRef::new(CellRef::new(2, 0), CellRef::new(2, u32::MAX));
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
        assert_eq!(got, want);
    }
}

/// The page file a store can spill completed bands into, and the band codec it uses.
///
/// A page holds one finished band: the four stripes, the number table, the text table and the
/// style table. The format is deliberately small and self-checking on read; a page that does not
/// parse is reported as missing rather than trusted. The file is a temporary beside the system's
/// temp directory, removed when the last store that owns it drops, and never referenced by a
/// document on disk.
mod page {
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Seek, SeekFrom, Write};

    use flate2::Compression;
    use flate2::read::ZlibDecoder;
    use flate2::write::ZlibEncoder;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use gridcraft_core::{CellError, CellRef, RangeRef, Value};

    use super::{Band, BandRow, ERRORS, Formula, RowExtras, StyleId, error_code};

    const MAGIC: u32 = 0x4750_4231; // "GPB1"
    const VERSION: u8 = 1;
    const MAX_PAGE: usize = 64 * 1024 * 1024;
    const MAX_ROWS: u32 = 64;
    const MAX_COLS: u32 = 16_384;
    const MAX_CELLS: usize = 1_048_576;
    const MAX_TEXT: usize = 16 * 1024 * 1024;
    const MAX_DEPTH: u32 = 8;

    #[derive(Clone, Copy, Debug)]
    pub(super) struct PageRef {
        offset: u64,
        len: u32,
    }

    /// A completed band on disk, with the range its rows cover so `used_range` does not have to
    /// load every page. `band` caches the decoded copy after the first read; a corrupt page is
    /// remembered as `None` and behaves as missing.
    #[derive(Debug)]
    enum Cached {
        Miss,
        Corrupt,
        Band(Arc<Band>),
    }

    #[derive(Debug)]
    pub(super) struct Page {
        at: PageRef,
        bounds: Option<(u32, u32, u32, u32)>,
        formulas: bool,
        owned: bool,
        band: Mutex<Cached>,
    }

    impl Page {
        pub(super) fn new(at: PageRef, bounds: Option<(u32, u32, u32, u32)>, formulas: bool, owned: bool) -> Page {
            Page { at, bounds, formulas, owned, band: Mutex::new(Cached::Miss) }
        }

        /// The page a rewritten band becomes: the bounds and flags are recomputed from the new
        /// band exactly as [`spill_band`](super::CellStore::spill_band) computes them, and the
        /// decoded copy is installed so the read that follows does not decode it again.
        pub(super) fn from_band(at: PageRef, band: Arc<Band>) -> Page {
            let bounds = band_bounds(&band);
            let formulas = band.rows.iter().any(|r| !r.formulas().is_empty());
            let owned = band.rows.iter().any(|r| !r.formulas().is_empty() || !r.arrays().is_empty());
            Page { at, bounds, formulas, owned, band: Mutex::new(Cached::Band(band)) }
        }

        /// Reads and decodes the page without touching the cached copy or the store's page-order
        /// queue: a compaction pass looks at pages it may leave alone, and must not pin a decoded
        /// band per page (the queue only evicts pages it was told about).
        pub(super) fn decode(&self, file: &PageFile) -> Option<Arc<Band>> {
            file.decodes.fetch_add(1, Ordering::Relaxed);
            let bytes = file.read(self.at)?;
            decode_band(&bytes).map(Arc::new)
        }

        pub(super) fn bounds(&self) -> Option<(u32, u32, u32, u32)> {
            self.bounds
        }

        pub(super) fn has_formulas(&self) -> bool {
            self.formulas
        }

        pub(super) fn has_owned(&self) -> bool {
            self.owned
        }

        pub(super) fn page_len(&self) -> usize {
            usize::try_from(self.at.len).unwrap_or(0)
        }

        /// The decoded band, reading and validating the page on first use. The bool says this call
        /// decoded it, which is what the store's page-cache queue needs to know.
        pub(super) fn load(&self, file: &PageFile) -> (Option<Arc<Band>>, bool) {
            if let Ok(guard) = self.band.lock() {
                match &*guard {
                    Cached::Band(band) => return (Some(band.clone()), false),
                    Cached::Corrupt => return (None, false),
                    Cached::Miss => {}
                }
            }
            let decoded = file.read(self.at).and_then(|bytes| decode_band(&bytes).map(Arc::new));
            if let Ok(mut guard) = self.band.lock() {
                match &*guard {
                    Cached::Band(band) => return (Some(band.clone()), false),
                    Cached::Corrupt => return (None, false),
                    Cached::Miss => {
                        *guard = match &decoded {
                            Some(band) => Cached::Band(band.clone()),
                            None => Cached::Corrupt,
                        };
                    }
                }
            }
            (decoded, true)
        }

        /// Drops the decoded copy; a later read decodes the page again. A corrupt page stays
        /// remembered as corrupt rather than being retried on every access.
        pub(super) fn clear(&self) {
            if let Ok(mut guard) = self.band.lock()
                && matches!(&*guard, Cached::Band(_))
            {
                *guard = Cached::Miss;
            }
        }

        #[cfg(test)]
        pub(super) fn is_cached(&self) -> bool {
            self.band.lock().is_ok_and(|guard| matches!(&*guard, Cached::Band(_)))
        }
    }

    /// A temporary file pages are appended to. One handle is shared by every clone of a store and
    /// by its undo snapshots; writes and reads serialise on the mutex, which is fine for the read
    /// path a viewport exercises (one page at a time).
    #[derive(Debug)]
    pub(super) struct PageFile {
        path: PathBuf,
        file: Mutex<Option<File>>,
        /// How many pages [`Page::decode`] has read through this file. Compaction of a store with
        /// nothing to reclaim must decode none, and a test can prove it because the count belongs
        /// to this file (not to the process, where parallel tests would see each other's reads).
        decodes: AtomicU64,
    }

    impl PageFile {
        pub(super) fn new() -> Option<PageFile> {
            // A counter, not the clock: `SystemTime::now` traps on wasm. The pid separates
            // processes; `create_new` still refuses a stale collision, and the loop tries the next
            // name rather than trusting an existing file.
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let nonce = NEXT.fetch_add(1, Ordering::Relaxed);
            for attempt in 0..16u32 {
                let path = std::env::temp_dir().join(format!("gridcraft-page-{}-{nonce}-{attempt}.bin", std::process::id()));
                match OpenOptions::new().read(true).write(true).create_new(true).open(&path) {
                    Ok(file) => return Some(PageFile { path, file: Mutex::new(Some(file)), decodes: AtomicU64::new(0) }),
                    Err(_) => continue,
                }
            }
            None
        }

        /// Appends `bytes` and returns where they landed. A page larger than the cap is refused
        /// before it is written.
        #[cfg(test)]
        pub(super) fn path(&self) -> &std::path::Path {
            &self.path
        }

        /// Pages decoded through [`Page::decode`] so far (see the field's comment).
        #[cfg(test)]
        pub(super) fn decodes(&self) -> u64 {
            self.decodes.load(Ordering::Relaxed)
        }

        pub(super) fn write(&self, bytes: &[u8]) -> Option<PageRef> {
            if bytes.is_empty() || bytes.len() > MAX_PAGE {
                return None;
            }
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
            encoder.write_all(bytes).ok()?;
            let packed = encoder.finish().ok()?;
            let len = u32::try_from(packed.len()).ok()?;
            if packed.is_empty() || packed.len() > MAX_PAGE {
                return None;
            }
            let mut guard = self.file.lock().ok()?;
            let file = guard.as_mut()?;
            let offset = file.seek(SeekFrom::End(0)).ok()?;
            file.write_all(&packed).ok()?;
            Some(PageRef { offset, len })
        }

        fn read(&self, at: PageRef) -> Option<Vec<u8>> {
            let len = usize::try_from(at.len).ok()?;
            if len == 0 || len > MAX_PAGE {
                return None;
            }
            let mut guard = self.file.lock().ok()?;
            let file = guard.as_mut()?;
            file.seek(SeekFrom::Start(at.offset)).ok()?;
            let mut packed = vec![0u8; len];
            file.read_exact(&mut packed).ok()?;
            drop(guard);
            let mut decoder = ZlibDecoder::new(packed.as_slice());
            let mut bytes = Vec::new();
            decoder.by_ref().take(MAX_PAGE as u64 + 1).read_to_end(&mut bytes).ok()?;
            (bytes.len() <= MAX_PAGE).then_some(bytes)
        }
    }

    impl Drop for PageFile {
        fn drop(&mut self) {
            if let Ok(mut guard) = self.file.lock()
                && let Some(file) = guard.take()
            {
                drop(file);
            }
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn put_u8(out: &mut Vec<u8>, v: u8) {
        out.push(v);
    }

    fn put_u16(out: &mut Vec<u8>, v: u16) {
        out.extend_from_slice(&v.to_le_bytes());
    }

    fn put_u32(out: &mut Vec<u8>, v: u32) {
        out.extend_from_slice(&v.to_le_bytes());
    }

    fn put_u64(out: &mut Vec<u8>, v: u64) {
        out.extend_from_slice(&v.to_le_bytes());
    }

    fn put_len(out: &mut Vec<u8>, len: usize) {
        // Every caller has checked the length against a page cap; a truncated length would make
        // the decoder reject the page anyway, so saturate rather than truncate silently.
        put_u32(out, u32::try_from(len).unwrap_or(u32::MAX));
    }

    fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
        put_len(out, bytes.len());
        out.extend_from_slice(bytes);
    }

    fn put_runs(out: &mut Vec<u8>, runs: &[(u16, u16)]) {
        put_len(out, runs.len());
        for (a, b) in runs {
            put_u16(out, *a);
            put_u16(out, *b);
        }
    }

    fn take_u8(input: &mut &[u8]) -> Option<u8> {
        let (first, rest) = input.split_first()?;
        *input = rest;
        Some(*first)
    }

    fn take_u16(input: &mut &[u8]) -> Option<u16> {
        let bytes: [u8; 2] = input.get(..2)?.try_into().ok()?;
        *input = input.get(2..)?;
        Some(u16::from_le_bytes(bytes))
    }

    fn take_u32(input: &mut &[u8]) -> Option<u32> {
        let bytes: [u8; 4] = input.get(..4)?.try_into().ok()?;
        *input = input.get(4..)?;
        Some(u32::from_le_bytes(bytes))
    }

    fn take_u64(input: &mut &[u8]) -> Option<u64> {
        let bytes: [u8; 8] = input.get(..8)?.try_into().ok()?;
        *input = input.get(8..)?;
        Some(u64::from_le_bytes(bytes))
    }

    fn take_len(input: &mut &[u8], cap: usize) -> Option<usize> {
        let len = usize::try_from(take_u32(input)?).ok()?;
        (len <= cap).then_some(len)
    }

    fn take_bytes(input: &mut &[u8], cap: usize) -> Option<Vec<u8>> {
        let len = take_len(input, cap)?;
        let bytes = input.get(..len)?.to_vec();
        *input = input.get(len..)?;
        Some(bytes)
    }

    fn encode_value(out: &mut Vec<u8>, value: &Value, depth: u32) -> Option<()> {
        if depth > MAX_DEPTH {
            return None;
        }
        match value {
            Value::Empty => put_u8(out, 0),
            Value::Number(n) => {
                put_u8(out, 1);
                put_u64(out, if *n == 0.0 { 0.0f64.to_bits() } else { n.to_bits() });
            }
            Value::Text(t) => {
                put_u8(out, 2);
                if t.len() > MAX_TEXT {
                    return None;
                }
                put_bytes(out, t.as_bytes());
            }
            Value::Bool(b) => {
                put_u8(out, 3);
                put_u8(out, u8::from(*b));
            }
            Value::Error(e) => {
                put_u8(out, 4);
                put_u8(out, error_code(*e) as u8);
            }
            Value::Array(a) => {
                put_u8(out, 5);
                put_u32(out, u32::try_from(a.rows).ok()?);
                put_u32(out, u32::try_from(a.cols).ok()?);
                put_len(out, a.data.len());
                for v in &a.data {
                    encode_value(out, v, depth.saturating_add(1))?;
                }
            }
        }
        Some(())
    }

    fn decode_value(input: &mut &[u8], depth: u32) -> Option<Value> {
        if depth > MAX_DEPTH {
            return None;
        }
        match take_u8(input)? {
            0 => Some(Value::Empty),
            1 => Some(Value::Number(f64::from_bits(take_u64(input)?))),
            2 => {
                let bytes = take_bytes(input, MAX_TEXT)?;
                let text = std::str::from_utf8(&bytes).ok()?;
                Some(Value::Text(Arc::from(text)))
            }
            3 => Some(Value::Bool(take_u8(input)? != 0)),
            4 => {
                let code = take_u8(input)?;
                Some(Value::Error(*ERRORS.get(usize::from(code)).unwrap_or(&CellError::Value)))
            }
            5 => {
                let rows = usize::try_from(take_u32(input)?).ok()?;
                let cols = usize::try_from(take_u32(input)?).ok()?;
                let len = take_len(input, MAX_CELLS)?;
                if rows == 0 || cols == 0 || rows.checked_mul(cols)? != len {
                    return None;
                }
                let mut data = Vec::with_capacity(len);
                for _ in 0..len {
                    data.push(decode_value(input, depth.saturating_add(1))?);
                }
                Some(Value::Array(Arc::new(gridcraft_core::Array { rows, cols, data })))
            }
            _ => None,
        }
    }

    fn encode_formula(out: &mut Vec<u8>, f: &Formula) -> Option<()> {
        if f.text.len() > MAX_TEXT {
            return None;
        }
        put_bytes(out, f.text.as_bytes());
        match f.array {
            Some(a) => {
                put_u8(out, 1);
                put_u32(out, a.start.row);
                put_u32(out, a.start.col);
                put_u32(out, a.end.row);
                put_u32(out, a.end.col);
            }
            None => put_u8(out, 0),
        }
        put_u8(out, u8::from(f.dynamic));
        Some(())
    }

    fn decode_formula(input: &mut &[u8]) -> Option<Arc<Formula>> {
        let bytes = take_bytes(input, MAX_TEXT)?;
        let text = String::from_utf8(bytes).ok()?;
        let array = match take_u8(input)? {
            0 => None,
            1 => Some(RangeRef::new(CellRef::new(take_u32(input)?, take_u32(input)?), CellRef::new(take_u32(input)?, take_u32(input)?))),
            _ => return None,
        };
        let dynamic = take_u8(input)? != 0;
        // Reparse so a loaded formula behaves exactly like the one that was spilled (including the
        // parsed tree and the undo byte count). Most pages hold no formulas, so the cost is rare.
        let mut formula = Formula::new(&text);
        formula.array = array;
        formula.dynamic = dynamic;
        Some(Arc::new(formula))
    }

    fn encode_row(out: &mut Vec<u8>, row: &BandRow) -> Option<()> {
        put_u32(out, row.key);
        put_u32(out, row.first);
        put_u32(out, row.bits_at);
        put_u32(out, row.words);
        put_u32(out, row.cells_at);
        put_u32(out, row.kinds_at);
        put_u32(out, row.styles_at);
        put_u32(out, row.cells);
        put_u32(out, row.kind_runs);
        put_u32(out, row.style_runs);
        match &row.extras {
            None => put_u8(out, 0),
            Some(e) => {
                put_u8(out, 1);
                put_len(out, e.formulas.len());
                for (col, f) in &e.formulas {
                    put_u32(out, *col);
                    encode_formula(out, f)?;
                }
                put_len(out, e.arrays.len());
                for (col, a) in &e.arrays {
                    put_u32(out, *col);
                    put_u32(out, u32::try_from(a.rows).ok()?);
                    put_u32(out, u32::try_from(a.cols).ok()?);
                    put_len(out, a.data.len());
                    for v in &a.data {
                        encode_value(out, v, 0)?;
                    }
                }
                put_len(out, e.wide.len());
                for v in &e.wide {
                    put_u64(out, *v);
                }
            }
        }
        Some(())
    }

    fn decode_row(input: &mut &[u8]) -> Option<BandRow> {
        let key = take_u32(input)?;
        if key >= MAX_ROWS {
            return None;
        }
        let first = take_u32(input)?;
        if first > MAX_COLS {
            return None;
        }
        let bits_at = take_u32(input)?;
        let words = take_u32(input)?;
        if words > MAX_COLS / 64 {
            return None;
        }
        let cells_at = take_u32(input)?;
        let kinds_at = take_u32(input)?;
        let styles_at = take_u32(input)?;
        let cells = take_u32(input)?;
        if cells > MAX_COLS {
            return None;
        }
        let kind_runs = take_u32(input)?;
        let style_runs = take_u32(input)?;
        if kind_runs > cells || style_runs > cells {
            return None;
        }
        let extras = match take_u8(input)? {
            0 => None,
            1 => {
                let formula_len = take_len(input, cells as usize)?;
                let mut formulas = Vec::with_capacity(formula_len);
                for _ in 0..formula_len {
                    let col = take_u32(input)?;
                    formulas.push((col, decode_formula(input)?));
                }
                formulas.sort_unstable_by_key(|(c, _)| *c);
                formulas.dedup_by_key(|(c, _)| *c);
                let array_len = take_len(input, cells as usize)?;
                let mut arrays = Vec::with_capacity(array_len);
                for _ in 0..array_len {
                    let col = take_u32(input)?;
                    let rows = usize::try_from(take_u32(input)?).ok()?;
                    let cols = usize::try_from(take_u32(input)?).ok()?;
                    let len = take_len(input, MAX_CELLS)?;
                    if rows == 0 || cols == 0 || rows.checked_mul(cols)? != len {
                        return None;
                    }
                    let mut data = Vec::with_capacity(len);
                    for _ in 0..len {
                        data.push(decode_value(input, 0)?);
                    }
                    arrays.push((col, Arc::new(gridcraft_core::Array { rows, cols, data })));
                }
                arrays.sort_unstable_by_key(|(c, _)| *c);
                arrays.dedup_by_key(|(c, _)| *c);
                let wide_len = take_len(input, MAX_CELLS)?;
                let mut wide = Vec::with_capacity(wide_len);
                for _ in 0..wide_len {
                    wide.push(take_u64(input)?);
                }
                Some(Arc::new(RowExtras { formulas, arrays, wide }))
            }
            _ => return None,
        };
        Some(BandRow { key, first, bits_at, words, cells_at, kinds_at, styles_at, cells, kind_runs, style_runs, extras })
    }

    /// Encodes one finished band. The callers spill only bands with their indexes dropped, so the
    /// hash/sorted indexes are not part of the page; the decoder leaves them empty and an edit
    /// rebuilds them.
    pub(super) fn encode_band(band: &Band) -> Option<Vec<u8>> {
        if band.rows.len() > MAX_ROWS as usize {
            return None;
        }
        let mut out = Vec::with_capacity(16 * 1024);
        put_u32(&mut out, MAGIC);
        put_u8(&mut out, VERSION);
        put_len(&mut out, band.rows.len());
        for row in &band.rows {
            encode_row(&mut out, row)?;
        }
        put_len(&mut out, band.bits.len());
        for v in &band.bits {
            put_u64(&mut out, *v);
        }
        put_runs(&mut out, &band.kinds);
        put_runs(&mut out, &band.styles);
        put_len(&mut out, band.payloads.len());
        for v in &band.payloads {
            put_u16(&mut out, *v);
        }
        put_len(&mut out, band.numbers.len());
        for v in band.numbers.iter() {
            put_u64(&mut out, if *v == 0.0 { 0.0f64.to_bits() } else { v.to_bits() });
        }
        put_len(&mut out, band.texts.len());
        for t in &band.texts {
            if t.len() > MAX_TEXT {
                return None;
            }
            put_bytes(&mut out, t.as_bytes());
        }
        put_len(&mut out, band.style_ids.len());
        for id in &band.style_ids {
            put_u32(&mut out, id.0);
        }
        if out.len() > MAX_PAGE {
            return None;
        }
        Some(out)
    }

    pub(super) fn decode_band(bytes: &[u8]) -> Option<Band> {
        let mut input = bytes;
        if take_u32(&mut input)? != MAGIC || take_u8(&mut input)? != VERSION {
            return None;
        }
        let rows_len = take_len(&mut input, MAX_ROWS as usize)?;
        let mut rows = Vec::with_capacity(rows_len);
        for _ in 0..rows_len {
            rows.push(decode_row(&mut input)?);
        }
        rows.sort_unstable_by_key(|r| r.key);
        if rows.windows(2).any(|w| w[0].key == w[1].key) {
            return None;
        }
        let bits_len = take_len(&mut input, MAX_ROWS as usize * (MAX_COLS as usize / 64))?;
        let mut bits = Vec::with_capacity(bits_len);
        for _ in 0..bits_len {
            bits.push(take_u64(&mut input)?);
        }
        let kinds = decode_runs(&mut input, MAX_CELLS)?;
        let styles = decode_runs(&mut input, MAX_CELLS)?;
        let payload_len = take_len(&mut input, MAX_CELLS)?;
        let mut payloads = Vec::with_capacity(payload_len);
        for _ in 0..payload_len {
            payloads.push(take_u16(&mut input)?);
        }
        let numbers_len = take_len(&mut input, MAX_CELLS)?;
        let mut numbers = Vec::with_capacity(numbers_len);
        for _ in 0..numbers_len {
            numbers.push(f64::from_bits(take_u64(&mut input)?));
        }
        let texts_len = take_len(&mut input, MAX_CELLS)?;
        let mut texts = Vec::with_capacity(texts_len);
        for _ in 0..texts_len {
            let raw = take_bytes(&mut input, MAX_TEXT)?;
            texts.push(Arc::<str>::from(std::str::from_utf8(&raw).ok()?));
        }
        let styles_len = take_len(&mut input, MAX_CELLS)?;
        let mut style_ids = Vec::with_capacity(styles_len);
        for _ in 0..styles_len {
            style_ids.push(StyleId(take_u32(&mut input)?));
        }
        if !input.is_empty() {
            return None;
        }
        let band = Band { rows, bits, kinds, styles, payloads, numbers, number_slots: Vec::new(), texts, text_index: Vec::new(), style_ids };
        let valid = band.rows.iter().all(|r| {
            let bits_end = (r.bits_at as usize).checked_add(r.words as usize).is_some_and(|end| end <= band.bits.len());
            let cells_end = (r.cells_at as usize).checked_add(r.cells as usize).is_some_and(|end| end <= band.payloads.len());
            let kinds_end = (r.kinds_at as usize).checked_add(r.kind_runs as usize).is_some_and(|end| end <= band.kinds.len());
            let styles_end = (r.styles_at as usize).checked_add(r.style_runs as usize).is_some_and(|end| end <= band.styles.len());
            let bits = r.bits(&band).iter().map(|w| w.count_ones()).sum::<u32>();
            let kinds = r.kind_runs(&band).iter().map(|(_, n)| u32::from(*n)).sum::<u32>();
            let styles = r.style_runs(&band).iter().map(|(_, n)| u32::from(*n)).sum::<u32>();
            bits_end && cells_end && kinds_end && styles_end && bits == r.cells && kinds == r.cells && styles == r.cells
        });
        if !valid {
            return None;
        }
        Some(band)
    }

    fn decode_runs(input: &mut &[u8], cap: usize) -> Option<Vec<(u16, u16)>> {
        let len = take_len(input, cap)?;
        let mut runs = Vec::with_capacity(len);
        for _ in 0..len {
            runs.push((take_u16(input)?, take_u16(input)?));
        }
        Some(runs)
    }

    /// The range a band's rows cover, for `used_range` without a load.
    pub(super) fn band_bounds(band: &Band) -> Option<(u32, u32, u32, u32)> {
        let (mut r0, mut r1, mut c0, mut c1) = (u32::MAX, 0u32, u32::MAX, 0u32);
        for row in &band.rows {
            let Some((first, last)) = row.column_span(band) else { continue };
            r0 = r0.min(row.key);
            r1 = r1.max(row.key);
            c0 = c0.min(first);
            c1 = c1.max(last);
        }
        (r0 != u32::MAX).then_some((r0, r1, c0, c1))
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
    fn set_value_keeps_what_the_cell_is() {
        let mut s = CellStore::new();
        let styled = Cell { value: Value::Number(1.0), formula: Some(std::sync::Arc::new(Formula::new("=1+1"))), style: StyleId(3) };
        s.set(c(0, 0), styled);
        s.set(c(0, 1), Cell::value(Value::Number(2.0)));
        assert!(s.set_value(c(0, 0), &Value::Text("changed".into())));
        let cell = s.get(c(0, 0)).unwrap();
        assert_eq!(cell.value, Value::Text("changed".into()));
        assert_eq!(cell.formula.as_ref().map(|f| f.text.as_str()), Some("1+1"));
        assert_eq!(cell.style, StyleId(3), "a value write keeps the cell's formatting");
        assert_eq!(s.len(), 2);
        // A cell that isn't stored gets no value (and is not created).
        assert!(!s.set_value(c(9, 9), &Value::Number(5.0)));
        assert!(s.get(c(9, 9)).is_none());
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn a_page_file_is_removed_with_its_store() {
        let path = {
            let mut s = CellStore::new();
            s.set(c(0, 0), Cell::value(Value::Number(1.0)));
            assert!(s.enable_paging());
            let path = s.page_file.as_ref().map(|file| file.path().to_path_buf()).expect("page file");
            assert!(path.exists());
            path
        };
        assert!(!path.exists(), "dropping the store removes the spill file");
    }

    #[test]
    fn the_page_cache_evicts_the_oldest_decoded_band() {
        let mut s = CellStore::new();
        for r in 0..(PAGE_CACHE_PAGES as u32 + 1) {
            s.set(c(r * 64, 0), Cell::value(Value::Number(f64::from(r))));
        }
        assert!(s.enable_paging());
        assert_eq!(s.pages.len(), PAGE_CACHE_PAGES + 1);
        assert_eq!(s.iter().count(), PAGE_CACHE_PAGES + 1);
        // The first band was decoded first and is past the cache window now.
        assert!(s.pages.get(&0).is_some_and(|page| !page.is_cached()));
        // The last one, and the rest of the window, are still decoded.
        let last = PAGE_CACHE_PAGES as u32;
        assert!(s.pages.get(&last).is_some_and(|page| page.is_cached()));
        assert_eq!(s.get(c(0, 0)).map(|x| x.value), Some(Value::Number(0.0)));
        assert!(s.pages.get(&0).is_some_and(|page| page.is_cached()));
    }

    #[test]
    fn a_band_round_trips_through_a_page() {
        use gridcraft_core::Array;
        let mut batch = BandBatch::new();
        let mut cells = vec![
            (0, Cell { value: Value::Number(1.25), formula: None, style: StyleId(3) }),
            (1, Cell::formula(Formula::new("=A1+1"))),
            (2, Cell::value(Value::Text(Arc::from("hello")))),
            (3, Cell::value(Value::Bool(true))),
            (4, Cell::value(Value::Error(CellError::Div0))),
            (5, Cell::value(Value::Array(Arc::new(Array::row(vec![Value::Number(2.0), Value::Text(Arc::from("x"))]))))),
        ];
        batch.set_row(0, &mut cells);
        let mut cells = vec![(0, Cell::value(Value::Number(-0.0))), (1, Cell::value(Value::Text(Arc::from("hello"))))];
        batch.set_row(1, &mut cells);
        let mut s = CellStore::new();
        s.insert_batch(batch);
        let band = s.bands.get(&0).expect("band");
        let bytes = page::encode_band(band).expect("encode");
        let file = page::PageFile::new().expect("page file");
        let at = file.write(&bytes).expect("write");
        let page = page::Page::new(at, page::band_bounds(band), true, true);
        let loaded = page.load(&file).0.expect("decode");
        assert!(band.same_as(&loaded), "the decoded band holds the same cells");
        for row in 0..2u32 {
            for col in 0..6u32 {
                assert_eq!(
                    s.get(c(row, col)),
                    {
                        let mut other = CellStore::new();
                        other.bands.insert(0, loaded.clone());
                        other.len = s.len();
                        other.get(c(row, col))
                    },
                    "row {row} col {col}"
                );
            }
        }
        // A corrupt or truncated page is refused rather than trusted.
        let mut corrupt = bytes.clone();
        if let Some(byte) = corrupt.first_mut() {
            *byte ^= 0xff;
        }
        assert!(page::decode_band(&corrupt).is_none());
        assert!(page::decode_band(&bytes[..bytes.len().saturating_sub(1)]).is_none());
        let mut huge = bytes.clone();
        huge.truncate(12);
        if huge.len() == 12 {
            huge[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        }
        assert!(page::decode_band(&huge).is_none());
    }

    /// A finished band keeps its texts but not the pointer index that only adding cells needs;
    /// the first edit rebuilds it. On a unique-key export this index was 16 bytes for every text
    /// of every band (640 MiB on the 100M-cell export).
    /// Paging takes completed bands out of the live map and answers the same reads from the page
    /// file; an edit brings only the touched band back, and a snapshot taken before keeps its
    /// values.
    #[test]
    fn paged_bands_read_and_edit_like_live_bands() {
        let mut s = CellStore::new();
        for r in 0..200u32 {
            s.set(c(r, 0), Cell::value(Value::Number(f64::from(r))));
            s.set(c(r, 1), Cell::value(Value::Text(Arc::from(if r % 2 == 0 { "even" } else { "odd" }))));
        }
        s.set(c(3, 2), Cell::formula(Formula::new("=A4+1")));
        s.set(c(3, 2), Cell { value: Value::Number(4.0), formula: Some(Arc::new(Formula::new("=A4+1"))), style: StyleId(2) });
        let snapshot = s.clone();
        assert!(s.enable_paging(), "a temporary page file is created");
        assert!(s.paged_band_count() > 0, "the completed bands were spilled");
        assert_eq!(s.len(), snapshot.len());
        assert_eq!(s.used_range(), snapshot.used_range());
        for r in 0..200u32 {
            assert_eq!(s.get(c(r, 0)), snapshot.get(c(r, 0)), "row {r} number");
            assert_eq!(s.get(c(r, 1)), snapshot.get(c(r, 1)), "row {r} text");
            assert_eq!(s.style(c(r, 0)), snapshot.style(c(r, 0)));
            assert!(s.contains(c(r, 1)));
        }
        assert_eq!(s.iter_range(RangeRef::parse("A1:C50").expect("range")).count(), 101);
        assert_eq!(s.iter().count(), 401);
        assert_eq!(s.formulas().count(), 1);
        assert_eq!(s.diff(&snapshot), []);
        // Editing a paged band materializes just that band and keeps the snapshot untouched.
        assert!(s.set_value(c(70, 0), &Value::Number(999.0)));
        assert_eq!(s.get(c(70, 0)).map(|x| x.value), Some(Value::Number(999.0)));
        assert_eq!(snapshot.get(c(70, 0)).map(|x| x.value), Some(Value::Number(70.0)));
        s.set(c(71, 0), Cell::value(Value::Text(Arc::from("new"))));
        assert_eq!(s.get(c(71, 0)).map(|x| x.value), Some(Value::Text(Arc::from("new"))));
        assert_eq!(s.remove(c(72, 0)).map(|x| x.value), Some(Value::Number(72.0)));
        assert_eq!(s.diff(&snapshot), [c(70, 0), c(71, 0), c(72, 0)]);
        // A band split across two batches still merges after paging.
        let mut batch = BandBatch::new();
        let mut cells = vec![(4, Cell::value(Value::Number(1234.0)))];
        batch.set_row(74, &mut cells);
        s.insert_batch(batch);
        assert_eq!(s.get(c(74, 4)).map(|x| x.value), Some(Value::Number(1234.0)));
    }

    #[test]
    fn a_finished_band_drops_its_text_index_and_rebuilds_it_on_edit() {
        let alpha: Arc<str> = Arc::from("alpha");
        let beta: Arc<str> = Arc::from("beta");
        let mut batch = BandBatch::new();
        let mut cells = vec![
            (0, Cell::value(Value::Text(alpha.clone()))),
            (1, Cell::value(Value::Text(alpha.clone()))),
            (2, Cell::value(Value::Text(beta.clone()))),
        ];
        batch.set_row(0, &mut cells);
        let mut s = CellStore::new();
        s.insert_batch(batch);
        let band = s.bands.get(&0).expect("band");
        assert!(band.text_index.is_empty(), "the completed band dropped its text index");
        assert_eq!(band.texts.len(), 2, "the same Arc is stored once");
        assert_eq!(s.get(c(0, 0)).map(|x| x.value), Some(Value::Text(alpha.clone())));
        assert_eq!(s.get(c(0, 2)).map(|x| x.value), Some(Value::Text(beta.clone())));
        // The edit finds the existing text through the rebuilt index and interns a new one.
        s.set(c(1, 0), Cell::value(Value::Text(alpha.clone())));
        s.set(c(1, 1), Cell::value(Value::Text(Arc::from("gamma"))));
        let band = s.bands.get(&0).expect("band");
        assert_eq!(band.texts.len(), 3);
        assert!(!band.text_index.is_empty(), "the index is back while the band is being edited");
        assert_eq!(s.get(c(1, 0)).map(|x| x.value), Some(Value::Text(alpha)));
        assert_eq!(s.get(c(1, 1)).map(|x| x.value), Some(Value::Text(Arc::from("gamma"))));
    }

    /// A band with more texts than a `u16` can index stores the actual slot of an existing text in
    /// the row's wide table. The old code stored the last text instead, which was only right when
    /// the text was the one that had just been added.
    #[test]
    fn a_text_past_the_u16_slots_keeps_its_own_slot() {
        let mut band = Band::default();
        let mut texts: Vec<Arc<str>> = Vec::new();
        for i in 0..=65_536usize {
            let text: Arc<str> = Arc::from(format!("t{i}"));
            assert_eq!(band.text_slot(&text), i as u32, "slot {i}");
            texts.push(text);
        }
        // One more text so the band's last slot is not the one the lookup asks for.
        assert_eq!(band.text_slot(&Arc::from("extra")), 65_537);
        let found = texts.get(65_536).cloned().expect("the text at the last u16 slot");
        let (kind, payload) = band.payload(&Value::Text(found));
        assert_eq!(kind, TEXT_WIDE);
        let Payload::Wide(slot) = payload else { panic!("expected a wide payload") };
        assert_eq!(slot, 65_536);
        assert_eq!(&*band.text(slot), "t65536");
    }

    #[test]
    fn set_values_writes_in_row_order() {
        let mut s = CellStore::new();
        let mut values: Vec<(CellRef, Value)> = Vec::new();
        for r in (0..200).rev() {
            s.set(c(r, 4), Cell::formula(Formula::new("=1+1")));
            values.push((c(r, 4), Value::Number(r as f64)));
        }
        values.sort_unstable_by_key(|(c, _)| (c.row, c.col));
        s.set_values(&mut values);
        for r in 0..200 {
            assert_eq!(s.get(c(r, 4)).unwrap().value, Value::Number(r as f64));
        }
        assert_eq!(s.len(), 200);
    }

    /// The large-batch path (above `PARALLEL_WRITE_MIN`) copies bands in workers and replaces them
    /// only after every worker finished; the old snapshot must keep its values.
    #[test]
    fn parallel_value_writes_keep_the_snapshot() {
        let mut s = CellStore::new();
        for r in 0..20_000u32 {
            s.set(c(r, 9), Cell::formula(Formula::new("=$A$1*2")));
        }
        let snapshot = s.clone();
        let mut values: Vec<(CellRef, Value)> = (0..20_000u32).map(|r| (c(r, 9), Value::Number(f64::from(r)))).collect();
        s.set_values(&mut values);
        for r in 0..20_000u32 {
            assert_eq!(s.get(c(r, 9)).unwrap().value, Value::Number(f64::from(r)));
            assert_eq!(snapshot.get(c(r, 9)).unwrap().value, Value::Empty);
        }
        assert_eq!(s.len(), 20_000);
        assert_eq!(snapshot.len(), 20_000);
    }

    /// A result batch writes through the row map: the logical rows are not the physical ones, and
    /// neither `len` nor the bands move.
    #[test]
    fn set_values_writes_through_a_row_map() {
        let mut s = CellStore::new();
        for r in 0..200u32 {
            s.set(c(r, 4), Cell::formula(Formula::new("=1+1")));
        }
        let (bands, len) = (s.band_count(), s.len());
        s.shift_rows(1, 1);
        assert_eq!(s.band_count(), bands, "a row insert moves the map, not the bands");
        let mut values: Vec<(CellRef, Value)> = (0..200u32).map(|r| (c(r, 4), Value::Number(f64::from(r)))).collect();
        s.set_values(&mut values);
        for r in 0..200u32 {
            // An insert at logical row 1 leaves that row a gap; the batch's value there is dropped.
            let want = if r == 1 { None } else { Some(Value::Number(f64::from(r))) };
            assert_eq!(s.get(c(r, 4)).map(|x| x.value), want, "row {r}");
        }
        assert_eq!(s.get(c(1, 4)), None, "the inserted row stays empty");
        assert_eq!(s.len(), len);
        assert_eq!(s.band_count(), bands);
    }

    /// A value on a logical row the map does not cover has nowhere to go: a gap row must neither
    /// allocate a physical row nor create a band, exactly as `set_value` refuses it.
    #[test]
    fn set_values_skips_a_gap_row_without_allocating() {
        let mut s = CellStore::new();
        s.set(c(0, 0), Cell::formula(Formula::new("=1")));
        s.shift_rows(0, 1);
        let (next_physical, bands) = (s.row_map.next_physical, s.band_count());
        let mut values = vec![(c(0, 0), Value::Number(1.0)), (c(1, 0), Value::Number(2.0))];
        s.set_values(&mut values);
        assert_eq!(s.get(c(1, 0)).map(|x| x.value), Some(Value::Number(2.0)));
        assert_eq!(s.row_map.next_physical, next_physical, "the gap row did not allocate a physical row");
        assert_eq!(s.band_count(), bands, "and did not create a band");
        assert_eq!(s.len(), 1);
    }

    /// The differential proof: a streamed batch must leave exactly the store `set_value` per cell
    /// leaves, on a store mapped along both axes (two row segments, two column segments). The
    /// batch covers the gap row and gap column and rows/columns no cell holds.
    #[test]
    fn set_values_equals_value_at_a_time_on_a_mapped_store() {
        let mut s = CellStore::new();
        for r in 0..140u32 {
            for col in 0..8u32 {
                s.set(c(r, col), Cell::formula(Formula::new("=1+1")));
                s.set_value(c(r, col), &Value::Number(f64::from(r * 10 + col)));
            }
        }
        s.shift_rows(1, 1);
        s.shift_cols(1, 1);
        let mut slow = s.clone();
        let mut values: Vec<(CellRef, Value)> = Vec::new();
        for r in 0..=141u32 {
            for col in 0..=9u32 {
                values.push((c(r, col), Value::Number(f64::from(r * 100 + col))));
            }
        }
        let mut fast = s;
        fast.set_values(&mut values);
        for (at, v) in values.iter() {
            slow.set_value(*at, v);
        }
        assert_eq!(fast.iter().collect::<Vec<_>>(), slow.iter().collect::<Vec<_>>());
        assert_eq!(fast, slow);
    }

    /// The batch must be in row-major order; a caller that hands in another order gets the
    /// value-at-a-time path (the same result), never a value on a band-grouped wrong row.
    #[test]
    fn set_values_writes_an_unsorted_batch_like_set_value() {
        let mut s = CellStore::new();
        for r in 0..20u32 {
            s.set(c(r, 0), Cell::formula(Formula::new("=1+1")));
        }
        s.shift_rows(1, 1);
        let mut slow = s.clone();
        let mut values: Vec<(CellRef, Value)> = (0..20u32).map(|r| (c(r, 0), Value::Number(f64::from(r)))).collect();
        values.reverse();
        let mut fast = s;
        fast.set_values(&mut values);
        for (at, v) in values.iter() {
            slow.set_value(*at, v);
        }
        assert_eq!(fast.iter().collect::<Vec<_>>(), slow.iter().collect::<Vec<_>>());
    }

    /// A band fed by two row segments must stay one task in the parallel plan: two runs of the
    /// same band inserted as two bands would let one run's writes win. This batch is one band
    /// wide (64 rows by 300 columns, 19,200 values) and spans the inserted row inside band 0.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn set_values_writes_a_band_split_by_two_row_segments() {
        let mut s = CellStore::new();
        for r in 0..64u32 {
            let mut cells: Vec<(u32, Cell)> = (0..300u32).map(|col| (col, Cell::value(Value::Number(f64::from(r * 1_000 + col))))).collect();
            s.set_row(r, &mut cells);
        }
        s.shift_rows(30, 1);
        let segments = s.row_map.segments_in(0, 63);
        assert_eq!(segments.len(), 2, "the batch spans the insert point");
        assert!(segments.iter().all(|seg| seg.physical / BAND == 0), "both runs land in band 0");
        let mut values: Vec<(CellRef, Value)> = Vec::new();
        for r in 0..64u32 {
            for col in 0..300u32 {
                values.push((c(r, col), Value::Number(f64::from(r * 1_000 + col + 7))));
            }
        }
        let mut slow = s.clone();
        assert!(s.set_values_mapped_parallel(&values, 4), "the plan fits the run budget");
        for (at, v) in values.iter() {
            slow.set_value(*at, v);
        }
        assert_eq!(s.iter().collect::<Vec<_>>(), slow.iter().collect::<Vec<_>>());
        assert_eq!(s.band_count(), 1, "the two runs write one band");
    }

    /// A paged band a result lands in comes back once, through the map; the batch does not leave a
    /// second copy or an undecoded page behind.
    #[test]
    fn set_values_materializes_a_paged_band_once() {
        let mut s = CellStore::new();
        for r in 0..200u32 {
            s.set(c(r, 0), Cell::formula(Formula::new("=1+1")));
        }
        assert!(s.enable_paging());
        let paged = s.paged_band_count();
        assert_eq!(paged, 4, "200 rows over 64-row bands");
        s.shift_rows(1, 1);
        let mut values: Vec<(CellRef, Value)> = (0..200u32).map(|r| (c(r, 0), Value::Number(f64::from(r)))).collect();
        s.set_values(&mut values);
        assert_eq!(s.paged_band_count(), 0, "every touched band is live after the write");
        for r in 0..200u32 {
            let want = if r == 1 { None } else { Some(Value::Number(f64::from(r))) };
            assert_eq!(s.get(c(r, 0)).map(|x| x.value), want, "row {r}");
        }
        assert_eq!(s.get(c(1, 0)), None, "the gap row holds nothing");
    }

    /// The mapped parallel write copies bands in workers exactly as the identity one does: the
    /// snapshot keeps its values and every value lands.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn parallel_mapped_value_writes_keep_the_snapshot() {
        let mut s = CellStore::new();
        for r in 0..20_000u32 {
            s.set(c(r, 9), Cell::formula(Formula::new("=$A$1*2")));
        }
        s.shift_rows(0, 1);
        s.shift_cols(1, 1);
        let snapshot = s.clone();
        // Logical row 0 is the inserted gap; the formula cells live on rows 1..=20,000.
        let mut values: Vec<(CellRef, Value)> = vec![(c(0, 10), Value::Number(-1.0))];
        values.extend((0..20_000u32).map(|r| (c(r + 1, 10), Value::Number(f64::from(r)))));
        let mut par = s.clone();
        assert!(par.set_values_mapped_parallel(&values, 4), "the plan fits the run budget");
        s.set_values(&mut values);
        for r in 0..20_000u32 {
            let at = c(r + 1, 10);
            assert_eq!(s.get(at).map(|x| x.value), Some(Value::Number(f64::from(r))), "row {}", r + 1);
            assert_eq!(par.get(at).map(|x| x.value), Some(Value::Number(f64::from(r))), "parallel row {}", r + 1);
            assert_eq!(snapshot.get(at).map(|x| x.value), Some(Value::Empty));
        }
        assert_eq!(s.get(c(0, 10)), None, "the gap row was not written");
        assert_eq!(s.len(), 20_000);
        assert_eq!(par, s);
    }

    /// A formula that recalculates to nothing keeps its cell: the value becomes `EMPTY`, the slot,
    /// the style and the formula stay, so `len`, `used_range` and `iter` do not change.
    #[test]
    fn set_values_keeps_a_cell_whose_result_is_empty() {
        let mut s = CellStore::new();
        s.set(c(3, 2), Cell { value: Value::Number(1.0), formula: Some(Arc::new(Formula::new("=A1"))), style: StyleId(4) });
        s.shift_rows(1, 1);
        let mut values = vec![(c(4, 2), Value::Empty)];
        s.set_values(&mut values);
        let got = s.get(c(4, 2)).expect("the cell keeps its slot");
        assert_eq!(got.value, Value::Empty);
        assert!(got.formula.is_some(), "the formula stays");
        assert_eq!(got.style, StyleId(4), "so does the style");
        assert_eq!(s.len(), 1);
        assert!(s.contains(c(4, 2)));
    }

    /// A band copied for a value edit shares the formulas of its rows; changing one formula in
    /// the copy must not change the snapshot's.
    #[test]
    fn formula_extras_are_shared_until_changed() {
        let mut s = CellStore::new();
        s.set(c(0, 0), Cell::formula(Formula::new("=1+1")));
        s.set(c(0, 1), Cell::formula(Formula::new("=2+2")));
        let mut copy = s.clone();
        copy.set_value(c(0, 0), &Value::Number(7.0));
        copy.set(c(0, 1), Cell::formula(Formula::new("=5+5")));
        let text = |store: &CellStore, at| store.get(at).and_then(|x| x.formula).map(|f| f.text.clone());
        assert_eq!(text(&s, c(0, 1)).as_deref(), Some("2+2"));
        assert_eq!(text(&copy, c(0, 1)).as_deref(), Some("5+5"));
        assert_eq!(text(&s, c(0, 0)).as_deref(), Some("1+1"));
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
    fn a_row_insert_changes_the_map_not_the_bands() {
        let mut s = CellStore::new();
        for r in 0..8u32 {
            s.set(c(r, 0), Cell::value(Value::Number(f64::from(r))));
            s.set(c(r, 1), Cell::value(Value::Text(Arc::from(format!("r{r}")))));
        }
        s.set(c(2, 2), Cell { value: Value::Number(2.0), formula: Some(Arc::new(Formula::new("=A3"))), style: StyleId(1) });
        let bands = s.band_count();
        let len = s.len();
        s.shift_rows(1, 1);
        assert_eq!(s.band_count(), bands, "an insert does not move or split a band");
        assert_eq!(s.len(), len);
        assert_eq!(s.get(c(0, 0)).map(|x| x.value), Some(Value::Number(0.0)));
        assert_eq!(s.get(c(1, 0)), None, "the inserted row is empty");
        assert_eq!(s.get(c(2, 0)).map(|x| x.value), Some(Value::Number(1.0)));
        assert_eq!(s.get(c(3, 2)).and_then(|x| x.formula).map(|f| f.text.clone()), Some("A3".to_string()));
        let mut rows: Vec<u32> = s.iter().map(|(at, _)| at.row).collect();
        rows.dedup();
        assert_eq!(rows, [0, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(s.used_range().map(|r| r.a1()), Some("A1:C9".to_string()));
        // Writing the inserted row allocates a fresh physical row without disturbing the rest.
        s.set(c(1, 0), Cell::value(Value::Number(-1.0)));
        s.set_row(1, &mut vec![(1, Cell::value(Value::Text(Arc::from("new"))))]);
        assert_eq!(s.get(c(1, 0)).map(|x| x.value), Some(Value::Number(-1.0)));
        assert_eq!(s.get(c(1, 1)).map(|x| x.value), Some(Value::Text(Arc::from("new"))));
        assert_eq!(s.get(c(2, 0)).map(|x| x.value), Some(Value::Number(1.0)));
        assert_eq!(s.iter_range(RangeRef::parse("A1:C3").expect("range")).count(), 6);
        // A delete drops the inserted row and pulls the later rows up, still without moving bands.
        s.take_range(RangeRef::rows(1, 1));
        s.shift_rows(2, -1);
        assert_eq!(s.get(c(1, 0)).map(|x| x.value), Some(Value::Number(1.0)));
        assert_eq!(s.get(c(2, 0)).map(|x| x.value), Some(Value::Number(2.0)));
        assert_eq!(s.get(c(1, 1)).map(|x| x.value), Some(Value::Text(Arc::from("r1"))));
        assert!(s.get(c(2, 2)).is_some());
        // A formula at a logical position is yielded there after the shifts.
        let formulas: Vec<CellRef> = s.formulas().map(|(at, _, _)| at).collect();
        assert_eq!(formulas, [c(2, 2)]);
        assert_eq!(s.formula_cells(RangeRef::parse("A1:C9").expect("range")).collect::<Vec<_>>(), [c(2, 2)]);
        // The serialized shape is logical: a reopened store has an identity map but the same cells.
        let bytes = serde_json::to_vec(&s).expect("serialize");
        let back: CellStore = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(s, back, "the row map is not part of the saved cells");
    }

    #[test]
    fn a_column_delete_then_value_write_keeps_the_formula_cell() {
        let mut s = CellStore::new();
        s.set(c(15, 8), Cell { value: Value::Empty, formula: Some(Arc::new(Formula::new("=1/0"))), style: StyleId(0) });
        for col in 10..=13 {
            s.set(c(15, col), Cell::value(Value::Number(f64::from(col))));
        }
        s.take_range(RangeRef::cols(10, 13));
        s.shift_cols(14, -4);
        assert_eq!(s.get(c(15, 8)).and_then(|x| x.formula).map(|f| f.text.clone()), Some("1/0".to_string()));
        let mut values = vec![(c(15, 8), Value::Error(gridcraft_core::CellError::Div0))];
        s.set_values(&mut values);
        assert_eq!(s.get(c(15, 8)).map(|x| x.value), Some(Value::Error(gridcraft_core::CellError::Div0)));
    }

    #[test]
    fn a_column_map_value_write_reaches_the_formula_cell() {
        let mut s = CellStore::new();
        s.set(c(15, 8), Cell { value: Value::Empty, formula: Some(Arc::new(Formula::new("=1/0"))), style: StyleId(0) });
        s.set(c(15, 10), Cell::value(Value::Number(1.0)));
        s.shift_cols(10, 4);
        assert_eq!(s.get(c(15, 8)).and_then(|x| x.formula).map(|f| f.text.clone()), Some("1/0".to_string()));
        assert!(s.set_value(c(15, 8), &Value::Error(gridcraft_core::CellError::Div0)), "the mapped formula cell takes a value");
        assert_eq!(s.get(c(15, 8)).map(|x| x.value), Some(Value::Error(gridcraft_core::CellError::Div0)));
    }

    /// The owned-cell gate answers the same on a mapped and a paged store, and a band whose page
    /// says it holds no formula or array is skipped without a decode: a whole-column `SUM` pays
    /// one band lookup per band instead of one per row.
    #[test]
    fn the_owned_cell_gate_reads_a_band_once() {
        let mut s = CellStore::new();
        for r in 0..300u32 {
            s.set(c(r, 0), Cell::value(Value::Number(f64::from(r))));
        }
        s.set(c(130, 1), Cell { value: Value::Number(3.0), formula: Some(Arc::new(Formula::new("=A1+1"))), style: StyleId(0) });
        let narrow = RangeRef::parse("A1:A300").expect("range");
        let wide = RangeRef::parse("A1:B300").expect("range");
        assert!(!s.has_owned_cells(narrow));
        assert!(s.has_owned_cells(wide));
        // Through a row map the band cache still has to find the formula, at its new logical row.
        s.shift_rows(1, 1);
        assert!(!s.has_owned_cells(narrow), "the formula moved out of column A");
        assert!(s.has_owned_cells(wide), "and is still inside the block");
        assert_eq!(s.formula_cells(wide).collect::<Vec<_>>(), [c(131, 1)]);
        // Paging keeps the flag on the pages that hold a formula; the ones without are skipped
        // without being decoded, so the gate stays a comparison per band.
        assert!(s.enable_paging());
        assert!(s.paged_band_count() > 0);
        let decodes = || s.page_file.as_ref().map_or(0, |file| file.decodes());
        let before = decodes();
        assert!(!s.has_owned_cells(RangeRef::parse("A1:A100").expect("range")), "the first bands hold no formula");
        assert_eq!(decodes(), before, "a page without a formula or array is not decoded to answer the gate");
        assert!(!s.has_owned_cells(narrow));
        assert!(s.has_owned_cells(wide));
        assert_eq!(s.formula_cells(wide).collect::<Vec<_>>(), [c(131, 1)]);
    }

    #[test]
    fn a_column_insert_changes_the_map_not_the_bands() {
        let mut s = CellStore::new();
        for col in 0..8u32 {
            s.set(c(0, col), Cell::value(Value::Number(f64::from(col))));
            s.set(c(1, col), Cell::value(Value::Text(Arc::from(format!("c{col}")))));
        }
        s.set(c(2, 2), Cell { value: Value::Number(2.0), formula: Some(Arc::new(Formula::new("=A1"))), style: StyleId(1) });
        let bands = s.band_count();
        let len = s.len();
        s.shift_cols(1, 1);
        assert_eq!(s.band_count(), bands, "a column insert does not move or split a band");
        assert_eq!(s.len(), len);
        assert_eq!(s.get(c(0, 0)).map(|x| x.value), Some(Value::Number(0.0)));
        assert_eq!(s.get(c(0, 1)), None, "the inserted column is empty");
        assert_eq!(s.get(c(0, 2)).map(|x| x.value), Some(Value::Number(1.0)));
        assert_eq!(s.get(c(1, 3)).map(|x| x.value), Some(Value::Text(Arc::from("c2"))));
        assert_eq!(s.used_range().map(|r| r.a1()), Some("A1:I3".to_string()));
        // Writing into the gap allocates a physical column; the other cells stay put.
        s.set(c(0, 1), Cell::value(Value::Number(-1.0)));
        assert_eq!(s.get(c(0, 1)).map(|x| x.value), Some(Value::Number(-1.0)));
        assert_eq!(s.get(c(0, 2)).map(|x| x.value), Some(Value::Number(1.0)));
        let mut cols: Vec<u32> = s.iter().map(|(at, _)| at.col).collect();
        cols.sort_unstable();
        cols.dedup();
        assert_eq!(cols, [0, 1, 2, 3, 4, 5, 6, 7, 8]);
        // Delete pulls the later columns left and drops the inserted one.
        s.take_range(RangeRef::cols(1, 1));
        s.shift_cols(2, -1);
        assert_eq!(s.get(c(0, 1)).map(|x| x.value), Some(Value::Number(1.0)));
        assert_eq!(s.get(c(0, 2)).map(|x| x.value), Some(Value::Number(2.0)));
        assert_eq!(s.get(c(1, 1)).map(|x| x.value), Some(Value::Text(Arc::from("c1"))));
        let formulas: Vec<CellRef> = s.formulas().map(|(at, _, _)| at).collect();
        assert_eq!(formulas, [c(2, 2)]);
        assert_eq!(s.formula_cells(RangeRef::parse("A1:I3").expect("range")).collect::<Vec<_>>(), [c(2, 2)]);
        let bytes = serde_json::to_vec(&s).expect("serialize");
        let back: CellStore = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(s, back, "the column map is not part of the saved cells");
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

    #[test]
    fn compact_reclaims_rows_with_no_logical_inverse() {
        let mut s = CellStore::new();
        for row in 0..10u32 {
            s.set(c(row, 0), Cell::value(Value::Number(f64::from(row))));
        }
        s.set(c(2, 1), Cell { value: Value::Text(Arc::from("kept")), formula: Some(Arc::new(Formula::new("=A3"))), style: StyleId(4) });
        let mut dynamic = Formula::new("=A1:A2*2");
        dynamic.array = Some(RangeRef::parse("C4:D5").expect("range"));
        dynamic.dynamic = true;
        let array = Value::Array(Arc::new(gridcraft_core::Array::row(vec![Value::Number(1.0), Value::Number(2.0)])));
        s.set(c(3, 2), Cell { value: array, formula: Some(Arc::new(dynamic)), style: StyleId(9) });
        let before = s.len();
        assert_eq!(before, 12);
        let snapshot = s.clone();
        // A direct model shift: unlike the engine's delete command, it does not clear the cells
        // first, so the row that falls off the map stays in its band.
        s.shift_rows(1, -1);
        let visible: Vec<(CellRef, Cell)> = s.iter().collect();
        let used = s.used_range();
        assert_eq!(visible.len(), 11);
        assert_eq!(s.len(), before, "the orphan is still counted before compaction");
        s.compact();
        assert_eq!(s.iter().collect::<Vec<_>>(), visible, "mapped cells keep their content");
        assert_eq!(s.used_range(), used, "the used range is unchanged");
        assert_eq!(s.len(), visible.len(), "len now counts exactly the reachable cells");
        assert_eq!(snapshot.len(), before, "the undo snapshot keeps its cells");
        assert_eq!(snapshot.iter().count(), before);
        assert!(s.len() < before);
        assert_eq!(s.get(c(9, 0)), None, "the clipped row cannot be read");
        let (len, bands, segments) = (s.len(), s.band_count(), s.row_map.segments.len());
        s.compact();
        assert_eq!((s.len(), s.band_count(), s.row_map.segments.len()), (len, bands, segments), "a second pass is a no-op");
        assert_eq!(s.iter().collect::<Vec<_>>(), visible);
    }

    #[test]
    fn compact_reclaims_columns_with_no_logical_inverse() {
        let mut s = CellStore::new();
        for col in 0..10u32 {
            s.set(c(0, col), Cell::value(Value::Number(f64::from(col))));
        }
        let before = s.len();
        s.shift_cols(1, -1);
        let visible: Vec<(CellRef, Cell)> = s.iter().collect();
        let used = s.used_range();
        assert_eq!(visible.len(), 9);
        assert_eq!(s.len(), before, "the orphan column is still counted before compaction");
        s.compact();
        assert_eq!(s.iter().collect::<Vec<_>>(), visible, "mapped cells keep their content");
        assert_eq!(s.used_range(), used, "the used range is unchanged");
        assert_eq!(s.len(), visible.len(), "len now counts exactly the reachable cells");
        assert!(s.len() < before);
        assert_eq!(s.get(c(0, 9)), None, "the clipped column cannot be read");
        assert_eq!(s.col_map.segments.len(), 1);
        assert!(
            s.col_map.free.iter().all(|physical| *physical < s.col_map.next_physical && s.col_map.inverse(*physical).is_none()),
            "every free column is reusable and none is still mapped"
        );
        let (len, bands, free, segments) = (s.len(), s.band_count(), s.col_map.free.clone(), s.col_map.segments.len());
        s.compact();
        assert_eq!(
            (s.len(), s.band_count(), s.col_map.free.clone(), s.col_map.segments.len()),
            (len, bands, free, segments),
            "a second pass is a no-op"
        );
        assert_eq!(s.iter().collect::<Vec<_>>(), visible);
    }

    #[test]
    fn compact_bounds_the_column_free_list() {
        let mut s = CellStore::new();
        for col in 0..8u32 {
            s.set(c(0, col), Cell::value(Value::Number(f64::from(col))));
        }
        s.shift_cols(1, -1);
        assert_eq!(s.col_map.free, [0]);
        // A stale key that is mapped, duplicates, and one past every physical column handed out.
        s.col_map.free.extend([0, 0, 4, 4, 9999]);
        s.compact();
        assert_eq!(s.col_map.free, [0], "only an in-range, unmapped key survives");
        assert_eq!(s.len(), 7);
        s.compact();
        assert_eq!(s.col_map.free, [0]);
    }

    #[test]
    fn compact_reclaims_a_fragmented_store_and_restores_a_contiguous_map() {
        let mut s = CellStore::new();
        for col in 0..40u32 {
            s.set(c(0, col), Cell::value(Value::Number(f64::from(col))));
        }
        let mut max_segments = s.col_map.segments.len();
        for k in [8u32, 16, 24, 32] {
            s.shift_cols(k, 1);
            max_segments = max_segments.max(s.col_map.segments.len());
        }
        assert!(max_segments > 1, "repeated inserts fragment the map into several runs");
        // A shift that is not paired with a delete leaves the cells of the dropped columns behind.
        s.shift_cols(2, -1);
        let visible: Vec<(CellRef, Cell)> = s.iter().collect();
        let before_len = s.len();
        let before_segments = s.col_map.segments.len();
        assert!(before_len > visible.len(), "the dropped column is still stored");
        s.compact();
        assert_eq!(s.iter().collect::<Vec<_>>(), visible);
        assert!(s.col_map.segments.len() <= before_segments, "compaction never adds a run");
        assert_eq!(s.len(), visible.len(), "every unreachable cell is gone");
        assert!(s.col_map.free.windows(2).all(|w| w[0] < w[1]), "the free list is deduplicated");
        assert!(
            s.col_map.free.iter().all(|physical| *physical < s.col_map.next_physical && s.col_map.inverse(*physical).is_none()),
            "the free list is bounded to reusable, unmapped columns"
        );
        let (len, bands, free, segments) = (s.len(), s.band_count(), s.col_map.free.clone(), s.col_map.segments.len());
        s.compact();
        assert_eq!(
            (s.len(), s.band_count(), s.col_map.free.clone(), s.col_map.segments.len()),
            (len, bands, free, segments),
            "a second pass is a no-op"
        );

        // A heal in the order the inserts were made leaves one contiguous run. The runs are
        // already merged by the shift; compaction is what returns that run to the fast path.
        let mut t = CellStore::new();
        for col in 0..40u32 {
            t.set(c(0, col), Cell::value(Value::Number(f64::from(col))));
        }
        for k in [8u32, 16, 24, 32] {
            t.shift_cols(k, 1);
        }
        let fragmented = t.col_map.segments.len();
        for k in [32u32, 24, 16, 8] {
            t.shift_cols(k + 1, -1);
        }
        assert_eq!(t.col_map.segments.len(), 1, "a matched delete heals the insert (had {fragmented} runs)");
        assert!(!t.col_map.is_identity());
        let visible: Vec<(CellRef, Cell)> = t.iter().collect();
        let before_segments = t.col_map.segments.len();
        t.compact();
        assert!(t.col_map.is_identity(), "a fully contiguous map returns to the fast path");
        assert_eq!(t.col_map.segments.len(), 0);
        assert!(t.col_map.segments.len() < before_segments, "the single run collapsed");
        let after: Vec<(CellRef, Cell)> = t.iter().collect();
        assert_eq!(after, visible);
        t.compact();
        assert!(t.col_map.is_identity());
        assert_eq!(t.iter().collect::<Vec<_>>(), visible);
    }

    #[test]
    fn compact_keeps_the_logical_cells_across_serialization() {
        let mut s = CellStore::new();
        for row in 0..5u32 {
            s.set(c(row, 0), Cell { value: Value::Number(f64::from(row)), formula: None, style: StyleId(row % 3) });
            s.set(c(row, 1), Cell::value(Value::Text(Arc::from(format!("r{row}").as_str()))));
        }
        s.set(c(2, 2), Cell { value: Value::Number(9.0), formula: Some(Arc::new(Formula::new("=A1+1"))), style: StyleId(7) });
        // Direct shifts leave the first physical row and column behind.
        s.shift_rows(1, -1);
        s.shift_cols(1, -1);
        s.compact();
        let visible: Vec<(CellRef, Cell)> = s.iter().collect();
        let positions: Vec<CellRef> = visible.iter().map(|(at, _)| *at).collect();
        let len = s.len();
        assert_eq!(len, visible.len());
        let bytes = serde_json::to_vec(&s).expect("serialize");
        let back: CellStore = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(back.len(), len);
        assert_eq!(back.iter().map(|(at, _)| at).collect::<Vec<_>>(), positions);
        assert_eq!(back, s, "the saved cells are the logical cells");
    }

    /// A paged store whose map lost a row: the page holding it is decoded, its orphan dropped and
    /// the band written back at the same key, while every mapped cell reads as before.
    #[test]
    fn compact_reclaims_paged_rows() {
        let mut s = CellStore::new();
        for row in 0..200u32 {
            s.set(c(row, 0), Cell::value(Value::Number(f64::from(row))));
        }
        s.set(c(3, 1), Cell { value: Value::Number(99.0), formula: None, style: StyleId(2) });
        assert!(s.enable_paging());
        assert_eq!(s.paged_band_count(), 4);
        let pages: Vec<u32> = s.pages.keys().copied().collect();
        // A direct model shift: unlike the engine's delete command, it does not clear the cells
        // first, so physical row 0 stays in its page with no logical row to reach it.
        s.shift_rows(1, -1);
        let visible: Vec<(CellRef, Cell)> = s.iter().collect();
        let used = s.used_range();
        assert_eq!(visible.len(), 200);
        assert_eq!(s.len(), 201, "the orphan is still counted before compaction");
        let out = s.compact();
        assert_eq!(out.cells, 1);
        assert_eq!(out.pages, 1, "the page holding the dropped row is rewritten");
        assert_eq!(out.bands, 0);
        assert!(out.changed);
        assert_eq!(s.iter().collect::<Vec<_>>(), visible, "mapped cells keep their content");
        assert_eq!(s.used_range(), used, "the used range is unchanged");
        assert_eq!(s.len(), visible.len(), "len now counts exactly the reachable cells");
        assert_eq!(s.pages.keys().copied().collect::<Vec<_>>(), pages, "the page keeps its band key");
        for (at, cell) in &visible {
            assert_eq!(s.get(*at).as_ref(), Some(cell), "{}", at.a1());
        }
        assert_eq!(visible.last().map(|(at, _)| *at), Some(c(198, 0)));
    }

    /// The rewrite installs a new `Arc<Page>`; the snapshot keeps the old one, its offsets and the
    /// cell compaction dropped.
    #[test]
    fn compact_keeps_the_old_page_for_a_snapshot() {
        let mut s = CellStore::new();
        for row in 0..200u32 {
            s.set(c(row, 0), Cell::value(Value::Number(f64::from(row))));
        }
        assert!(s.enable_paging());
        let snapshot = s.clone();
        let old_len = s.pages.get(&0).map(|page| page.page_len()).unwrap_or(0);
        assert!(old_len > 0, "page 0 holds the first band");
        s.shift_rows(1, -1);
        assert_eq!(s.compact().pages, 1);
        let new_page = s.pages.get(&0).expect("the rewritten page stays at its key");
        let old_page = snapshot.pages.get(&0).expect("the snapshot keeps its page");
        assert!(!Arc::ptr_eq(new_page, old_page), "the rewrite installs a new page");
        assert_eq!(snapshot.get(c(0, 0)).map(|x| x.value), Some(Value::Number(0.0)), "the snapshot still reads the dropped row");
        assert_eq!(s.get(c(0, 0)).map(|x| x.value), Some(Value::Number(1.0)), "the live store moved row 0 down");
        assert_eq!(snapshot.len(), 200);
        assert_eq!(snapshot.iter().count(), 200);
        assert!(snapshot.retained_bytes_against(&s) >= old_len, "the snapshot is charged for the page it no longer shares");
    }

    /// A page whose only rows lost their inverse is dropped rather than rewritten empty.
    #[test]
    fn compact_drops_a_page_that_only_held_orphans() {
        let mut s = CellStore::new();
        for row in [0u32, 1, 2, 100, 101, 102] {
            s.set(c(row, 0), Cell::value(Value::Number(f64::from(row))));
        }
        assert!(s.enable_paging());
        assert_eq!(s.paged_band_count(), 2);
        // Delete logical rows 0..63: page 0 held only those rows.
        s.shift_rows(64, -64);
        let visible: Vec<(CellRef, Cell)> = s.iter().collect();
        let used = s.used_range();
        assert_eq!(visible.len(), 3);
        assert_eq!(s.len(), 6);
        let out = s.compact();
        assert_eq!(out.cells, 3);
        assert_eq!(out.pages, 1, "the page that only held orphans is dropped");
        assert_eq!(s.paged_band_count(), 1);
        assert_eq!(s.len(), 3);
        assert_eq!(s.iter().collect::<Vec<_>>(), visible);
        assert_eq!(s.used_range(), used);
        assert_eq!(s.get(c(36, 0)).map(|x| x.value), Some(Value::Number(100.0)), "the remaining page still reads");
    }

    /// The early-out: a clean paged store (identity maps) decodes nothing, touches no page and
    /// changes nothing. This is the regression test for the O(1) return.
    #[test]
    fn compact_is_free_when_nothing_is_orphaned() {
        let mut s = CellStore::new();
        for row in 0..200u32 {
            s.set(c(row, 0), Cell::value(Value::Number(f64::from(row))));
        }
        assert!(s.enable_paging());
        assert!(!s.needs_compaction());
        let pages: Vec<(u32, Arc<page::Page>)> = s.pages.iter().map(|(key, page)| (*key, page.clone())).collect();
        let bytes = s.paged_bytes();
        let len = s.len();
        let decodes = s.page_file.as_ref().map(|file| file.decodes()).unwrap_or(0);
        let out = s.compact();
        assert_eq!(out, Compacted::default());
        assert_eq!(s.page_file.as_ref().map(|file| file.decodes()).unwrap_or(0), decodes, "a clean paged store decodes no page");
        assert_eq!(s.paged_bytes(), bytes);
        assert_eq!(s.len(), len);
        for (key, page) in pages {
            assert!(s.pages.get(&key).is_some_and(|now| Arc::ptr_eq(now, &page)), "page {key} is untouched");
        }
    }

    /// A page can hold a cell at a physical column whose inverse is gone; the column gaps select
    /// it, the rewrite drops the cell and recomputes the bounds.
    #[test]
    fn compact_reclaims_paged_columns() {
        let mut s = CellStore::new();
        for row in 0..100u32 {
            for col in 0..4u32 {
                s.set(c(row, col), Cell::value(Value::Number(f64::from(row * 10 + col))));
            }
        }
        assert!(s.enable_paging());
        let pages = s.paged_band_count();
        assert_eq!(pages, 2);
        s.shift_cols(1, -1);
        let visible: Vec<(CellRef, Cell)> = s.iter().collect();
        let used = s.used_range();
        assert_eq!(visible.len(), 300);
        assert_eq!(s.len(), 400);
        let out = s.compact();
        assert_eq!(out.cells, 100);
        assert_eq!(out.pages, pages, "every page held a cell in the dropped column");
        assert_eq!(s.len(), visible.len());
        assert_eq!(s.iter().collect::<Vec<_>>(), visible);
        assert_eq!(s.used_range(), used);
        assert!(
            s.pages.values().all(|page| page.bounds().is_some_and(|(_, _, first, _)| first >= 1)),
            "the rewritten bounds no longer cross the dropped column"
        );
        assert_eq!(s.get(c(0, 0)).map(|x| x.value), Some(Value::Number(1.0)), "the rewritten page reads");
        assert_eq!(s.get(c(99, 2)).map(|x| x.value), Some(Value::Number(993.0)));
    }

    /// With every page checked, a contiguous map returns to the identity fast path even though the
    /// store is still paged — the `reset_prefix` change — and the next pass is O(1).
    #[test]
    fn compact_restores_identity_after_a_complete_page_sweep() {
        let mut s = CellStore::new();
        for row in 0..200u32 {
            s.set(c(row, 0), Cell::value(Value::Number(f64::from(row))));
        }
        // A cell in the last row of the grid: deleting that row leaves one contiguous run from the
        // origin and puts the cell at physical `MAX_ROWS - 1` in a page of its own with no inverse.
        s.set(c(gridcraft_core::MAX_ROWS - 1, 0), Cell::value(Value::Number(7.0)));
        assert!(s.enable_paging());
        let pages = s.paged_band_count();
        assert_eq!(pages, 5);
        s.shift_rows(gridcraft_core::MAX_ROWS, -1);
        let visible: Vec<(CellRef, Cell)> = s.iter().collect();
        assert_eq!(visible.len(), 200);
        assert_eq!(s.len(), 201);
        assert!(!s.row_map.is_identity());
        let out = s.compact();
        assert_eq!(out.cells, 1);
        assert_eq!(out.pages, 1, "the page that only held the dropped row is gone");
        assert!(out.changed);
        assert!(s.row_map.is_identity(), "a complete sweep returns a contiguous map to the fast path");
        assert!(s.col_map.is_identity());
        assert_eq!(s.paged_band_count(), pages - 1);
        assert_eq!(s.len(), visible.len());
        assert_eq!(s.iter().collect::<Vec<_>>(), visible);
        let lens: Vec<usize> = s.pages.values().map(|page| page.page_len()).collect();
        let again = s.compact();
        assert_eq!(again, Compacted::default(), "the second pass is a no-op");
        assert!(s.row_map.is_identity());
        assert_eq!(s.pages.values().map(|page| page.page_len()).collect::<Vec<_>>(), lens);
        assert_eq!(s.iter().collect::<Vec<_>>(), visible);
    }

    /// A second pass over a store whose map cannot return to identity decodes the pages a gap still
    /// selects, finds nothing and changes nothing.
    #[test]
    fn compact_is_idempotent_across_pages() {
        let mut s = CellStore::new();
        for row in 0..200u32 {
            s.set(c(row, 0), Cell::value(Value::Number(f64::from(row))));
        }
        assert!(s.enable_paging());
        s.shift_rows(1, -1);
        assert_eq!(s.compact().pages, 1);
        let pages = s.paged_band_count();
        let lens: Vec<usize> = s.pages.values().map(|page| page.page_len()).collect();
        let bytes = s.paged_bytes();
        let visible: Vec<(CellRef, Cell)> = s.iter().collect();
        let used = s.used_range();
        let len = s.len();
        let again = s.compact();
        assert_eq!(again, Compacted::default(), "nothing left to reclaim");
        assert!(!again.changed);
        assert_eq!((s.paged_band_count(), s.paged_bytes(), s.len()), (pages, bytes, len));
        assert_eq!(s.pages.values().map(|page| page.page_len()).collect::<Vec<_>>(), lens);
        assert_eq!(s.used_range(), used);
        assert_eq!(s.iter().collect::<Vec<_>>(), visible);
    }

    /// A rewritten page recomputes its formula/array flags from the new band instead of dropping
    /// them, so `formulas` and `has_owned_cells` still answer from the page.
    #[test]
    fn compact_keeps_formulas_on_a_rewritten_page() {
        let mut s = CellStore::new();
        for row in 0..200u32 {
            s.set(c(row, 0), Cell::value(Value::Number(f64::from(row))));
        }
        s.set(c(1, 1), Cell { value: Value::Number(2.0), formula: Some(Arc::new(Formula::new("=A1+1"))), style: StyleId(3) });
        assert!(s.enable_paging());
        s.shift_rows(1, -1);
        let formulas: Vec<(CellRef, String, bool)> = s.formulas().map(|(at, f, has_value)| (at, f.text.clone(), has_value)).collect();
        assert_eq!(formulas.len(), 1);
        let block = RangeRef::new(c(0, 0), c(63, 10));
        assert!(s.has_owned_cells(block));
        assert_eq!(s.compact().pages, 1);
        assert_eq!(s.formulas().map(|(at, f, has_value)| (at, f.text.clone(), has_value)).collect::<Vec<_>>(), formulas);
        assert!(s.has_owned_cells(block));
        let page = s.pages.get(&0).expect("the rewritten page");
        assert!(page.has_formulas(), "the formula flag is recomputed");
        assert!(page.has_owned(), "the owned flag is recomputed");
        assert!(s.get(c(0, 1)).and_then(|cell| cell.formula).is_some(), "the formula reads back from the rewritten page");
    }

    /// A page that cannot be read leaves the pass incomplete: the store keeps its orphan and its
    /// map, and reports no change.
    #[test]
    fn compact_keeps_the_maps_when_a_page_cannot_be_read() {
        let mut s = CellStore::new();
        for row in 0..200u32 {
            s.set(c(row, 0), Cell::value(Value::Number(f64::from(row))));
        }
        assert!(s.enable_paging());
        s.shift_rows(1, -1);
        // The page file is gone (as a corrupt or missing page would be): the candidate cannot be
        // decoded, so nothing is dropped and the shortened run must not become identity.
        s.page_file = None;
        let out = s.compact();
        assert_eq!(out, Compacted::default(), "nothing was reclaimed");
        assert_eq!(s.len(), 200, "the orphan stays counted");
        assert_eq!(s.paged_band_count(), 4);
        assert!(!s.row_map.is_identity(), "an incomplete pass does not reset the map");
        assert!(s.needs_compaction(), "the map still has a gap to check");
    }

    /// A rewritten page serializes to the same logical cells: the wire format walks `iter`, and a
    /// save after compaction must not lose or move anything.
    #[test]
    fn compact_keeps_the_logical_cells_of_a_rewritten_page_across_serialization() {
        let mut s = CellStore::new();
        for row in 0..200u32 {
            s.set(c(row, 0), Cell { value: Value::Number(f64::from(row)), formula: None, style: StyleId(row % 3) });
            s.set(c(row, 1), Cell::value(Value::Text(Arc::from(format!("r{row}").as_str()))));
        }
        s.set(c(2, 2), Cell { value: Value::Number(9.0), formula: Some(Arc::new(Formula::new("=A1+1"))), style: StyleId(7) });
        assert!(s.enable_paging());
        s.shift_rows(1, -1);
        s.shift_cols(1, -1);
        let before = s.len();
        let visible: Vec<(CellRef, Cell)> = s.iter().collect();
        let out = s.compact();
        assert_eq!(out.cells, before - visible.len());
        assert_eq!(s.len(), visible.len());
        assert_eq!(s.iter().collect::<Vec<_>>(), visible);
        let positions: Vec<CellRef> = visible.iter().map(|(at, _)| *at).collect();
        let values: Vec<Value> = visible.iter().map(|(_, cell)| cell.value.clone()).collect();
        let bytes = serde_json::to_vec(&s).expect("serialize");
        let back: CellStore = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(back.len(), visible.len());
        assert_eq!(back.iter().map(|(at, _)| at).collect::<Vec<_>>(), positions);
        assert_eq!(back.iter().map(|(_, cell)| cell.value).collect::<Vec<_>>(), values);
        assert_eq!(back, s, "the saved cells are the logical cells");
    }
}
