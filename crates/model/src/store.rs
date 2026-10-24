//! Copy-on-write sparse cell storage, packed.
//!
//! Cells live in bands of 64 rows, each band behind an `Arc`, so cloning a store (an undo
//! snapshot) copies only the band pointers and editing a cell copies just its band.
//!
//! Inside a band a row is packed the way a worksheet is read and drawn: a presence bitmap over the
//! columns the row touches, then one entry per stored cell in three parallel arrays — kind,
//! payload, style — plus side tables for the two things a cell only rarely carries (a formula and
//! an array value). A `Cell` struct costs 40 bytes; packed like this a cell costs about 11, which
//! is the difference between a 30-million-cell sheet taking 1.6 GB and taking 400 MB.
//!
//! Text is the one payload that does not fit in eight bytes, so a band keeps a table of the strings
//! its rows use and the payload holds an index into it. Strings that arrive as the same `Arc` (as a
//! file's shared strings do — one `Arc` shared by every cell that mentions it) are found by
//! pointer, so deduplicating them costs a hash of a pointer, not a comparison of strings.

use std::collections::{BTreeMap, HashMap};
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

/// Runs of equal values over consecutive stored cells: `(value, length)` pairs in cell order.
///
/// A report formats a row in two or three runs — a date column, a block of numbers, a block of
/// text — and its cells hold kinds in runs the same way, so a per-cell array is mostly repetition:
/// two bytes a cell for the style, one for the kind. An entry here costs four bytes and covers a
/// run, which measured on a 31.7M-cell report is 0.04 style runs and 0.18 kind runs a cell. The
/// worst case, every cell its own run, costs a third more than the per-cell array.
#[derive(Clone, Debug, Default)]
struct Runs {
    runs: Vec<(u16, u16)>,
}

impl Runs {
    fn len(&self) -> usize {
        self.runs.iter().map(|(_, n)| *n as usize).sum()
    }

    fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    fn at(&self, i: usize) -> u16 {
        self.at_or(i, 0)
    }

    fn at_or(&self, i: usize, default: u16) -> u16 {
        let mut left = i;
        for (v, n) in &self.runs {
            let n = *n as usize;
            if left < n {
                return *v;
            }
            left -= n;
        }
        default
    }

    /// Gives back the growth slack: the runs of a row built from a file are final.
    fn compact(&mut self) {
        if self.runs.capacity() > self.runs.len() + self.runs.len() / 4 {
            self.runs.shrink_to_fit();
        }
    }

    fn push(&mut self, v: u16) {
        match self.runs.last_mut() {
            Some((last, n)) if *last == v && *n < u16::MAX => *n += 1,
            _ => self.runs.push((v, 1)),
        }
    }

    fn grow(&mut self, i: usize, v: u16) {
        let mut pos = 0usize;
        for idx in 0..self.runs.len() {
            let (rv, rn) = self.runs[idx];
            let end = pos + rn as usize;
            if i > end {
                pos = end;
                continue;
            }
            // Inserting a value a neighbouring run already holds is just a longer run.
            if rv == v {
                self.runs[idx].1 += 1;
                return;
            }
            if i == pos {
                self.runs.insert(idx, (v, 1));
                self.merge(idx);
                return;
            }
            let before = (i - pos) as u16;
            let after = rn - before;
            self.runs[idx].1 = before;
            self.runs.insert(idx + 1, (v, 1));
            if after > 0 {
                self.runs.insert(idx + 2, (rv, after));
            }
            // The new run may sit next to one that already holds the same value.
            self.merge(idx + 1);
            return;
        }
        self.push(v);
    }

    fn shrink(&mut self, i: usize) {
        let mut pos = 0usize;
        for idx in 0..self.runs.len() {
            let (_, rn) = self.runs[idx];
            let end = pos + rn as usize;
            if i < end {
                self.runs[idx].1 -= 1;
                if self.runs[idx].1 == 0 {
                    self.runs.remove(idx);
                    if idx > 0 {
                        self.merge(idx - 1);
                    }
                } else {
                    self.merge(idx);
                }
                return;
            }
            pos = end;
        }
    }

    fn set(&mut self, i: usize, v: u16) {
        if self.at(i) == v {
            return;
        }
        self.grow(i, v);
        // `grow` put the new cell in its own run: drop the old one at i + 1.
        self.shrink(i + 1);
    }

    /// Merges run `idx` with its neighbours when they hold the same value.
    fn merge(&mut self, idx: usize) {
        for j in [idx, idx.saturating_sub(1)] {
            while j + 1 < self.runs.len() && self.runs[j].0 == self.runs[j + 1].0 {
                let n = self.runs[j + 1].1;
                self.runs[j].1 += n;
                self.runs.remove(j + 1);
            }
        }
    }
}

/// The two things a cell only rarely carries: a formula, and an array value.
#[derive(Clone, Debug, Default)]
struct RowExtras {
    /// `(column, formula)`, sorted by column.
    formulas: Vec<(u32, Arc<Formula>)>,
    /// `(column, array value)`, sorted by column.
    arrays: Vec<(u32, Arc<gridcraft_core::Array>)>,
}

/// One row of a band: a bitmap of the columns it touches plus one entry per stored cell.
#[derive(Clone, Debug, Default)]
struct Row {
    /// Column of bit 0, always a multiple of 64 so the bitmap grows by whole words.
    first: u32,
    bits: Vec<u64>,
    /// The kind of each stored cell, in runs: a row is mostly a block of numbers and a block of
    /// text.
    kinds: Runs,
    payloads: Vec<u64>,
    /// Index into the band's style dictionary, in runs: a report formats a row in two or three.
    styles: Runs,
    /// Held out of line: a row with neither a formula nor an array pays nothing for them.
    extras: Option<Box<RowExtras>>,
}

impl Row {
    fn is_empty(&self) -> bool {
        self.kinds.is_empty() && self.extras.is_none()
    }

    /// The style of the `i`th stored cell.
    fn style_at(&self, i: usize) -> u16 {
        self.styles.at(i)
    }

    fn kind_at(&self, i: usize) -> u8 {
        self.kinds.at_or(i, u16::from(EMPTY)) as u8
    }

    fn formulas(&self) -> &[(u32, Arc<Formula>)] {
        self.extras.as_ref().map_or(&[], |e| e.formulas.as_slice())
    }

    fn arrays(&self) -> &[(u32, Arc<gridcraft_core::Array>)] {
        self.extras.as_ref().map_or(&[], |e| e.arrays.as_slice())
    }

    fn drop_unused_extras(&mut self) {
        if self.extras.as_ref().is_some_and(|e| e.formulas.is_empty() && e.arrays.is_empty()) {
            self.extras = None;
        }
    }

    /// Number of stored cells before `bit` of `word`.
    fn rank(&self, word: usize, bit: u32) -> usize {
        let mut n = 0usize;
        for w in &self.bits[..word] {
            n += w.count_ones() as usize;
        }
        n + (self.bits[word] & ((1u64 << bit) - 1)).count_ones() as usize
    }

    /// Number of stored cells strictly before `col`.
    fn rank_before(&self, col: u32) -> usize {
        let Some(slot) = col.checked_sub(self.first) else { return 0 };
        let word = (slot / 64) as usize;
        if word >= self.bits.len() {
            return self.kinds.len();
        }
        self.rank(word, slot % 64)
    }

    /// Index into the dense arrays for `col`, when the row stores a cell there.
    fn index_of(&self, col: u32) -> Option<usize> {
        let slot = col.checked_sub(self.first)?;
        let word = (slot / 64) as usize;
        let bit = slot % 64;
        let w = *self.bits.get(word)?;
        (w & (1u64 << bit) != 0).then(|| self.rank(word, bit))
    }

    fn columns(&self) -> u32 {
        self.bits.len() as u32 * 64
    }

    /// Grows the bitmap to cover `col`, keeping `first` word-aligned.
    fn cover(&mut self, col: u32) {
        if self.bits.is_empty() {
            self.first = col / 64 * 64;
            self.bits.push(0);
            return;
        }
        if col < self.first {
            let words = ((self.first - col) as usize).div_ceil(64);
            for _ in 0..words {
                self.bits.insert(0, 0);
            }
            self.first -= words as u32 * 64;
        }
        let need = ((col - self.first) / 64 + 1) as usize;
        if need > self.bits.len() {
            self.bits.resize(need, 0);
        }
    }

    /// First and last stored column.
    fn column_span(&self) -> Option<(u32, u32)> {
        let mut first = None;
        let mut last = None;
        for (i, w) in self.bits.iter().enumerate() {
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

    fn remove_at(&mut self, i: usize, col: u32) {
        self.kinds.shrink(i);
        self.payloads.remove(i);
        self.styles.shrink(i);
        let slot = col - self.first;
        let word = (slot / 64) as usize;
        self.bits[word] &= !(1u64 << (slot % 64));
    }
}

/// The rows of one band, plus the two tables their packed cells index into.
#[derive(Clone, Debug, Default)]
struct Band {
    rows: BTreeMap<u32, Row>,
    texts: Vec<Arc<str>>,
    /// Pointer of each kept text → its slot, so one shared `Arc` is stored once per band.
    text_index: HashMap<usize, u32>,
    styles: Vec<StyleId>,
}

impl Band {
    fn text_slot(&mut self, s: &Arc<str>) -> u64 {
        let key = Arc::as_ptr(s) as *const u8 as usize;
        if let Some(i) = self.text_index.get(&key) {
            return *i as u64;
        }
        let i = self.texts.len() as u32;
        self.texts.push(s.clone());
        self.text_index.insert(key, i);
        i as u64
    }

    fn style_slot(&mut self, s: StyleId) -> u16 {
        if let Some(i) = self.styles.iter().position(|x| *x == s) {
            return i as u16;
        }
        self.styles.push(s);
        (self.styles.len() - 1) as u16
    }

    fn text(&self, payload: u64) -> Arc<str> {
        self.texts.get(payload as usize).cloned().unwrap_or_else(|| Arc::from(""))
    }

    fn style(&self, slot: u16) -> StyleId {
        self.styles.get(slot as usize).copied().unwrap_or(StyleId::DEFAULT)
    }

    /// Rebuilds the `Cell` at `col` from the packed row.
    fn cell_at(&self, row: &Row, i: usize, col: u32) -> Cell {
        let payload = row.payloads[i];
        let value = match row.kind_at(i) {
            TEXT => Value::Text(self.text(payload)),
            BOOL => Value::Bool(payload != 0),
            ERROR => Value::Error(*ERRORS.get(payload as usize).unwrap_or(&CellError::Value)),
            ARRAY => match row.arrays().binary_search_by_key(&col, |(c, _)| *c) {
                Ok(j) => Value::Array(row.arrays()[j].1.clone()),
                Err(_) => Value::Empty,
            },
            EMPTY => Value::Empty,
            _ => Value::Number(f64::from_bits(payload)),
        };
        Cell { value, formula: row.formula_at(col).cloned(), style: self.style(row.style_at(i)) }
    }

    /// Stores `cell` at `col`; a blank cell is removed instead. Text and style slots are resolved
    /// before the row is borrowed, because both live in the band.
    fn put(&mut self, row_key: u32, col: u32, cell: Cell) {
        let blank = cell.is_blank();
        let (kind, payload) = if blank {
            (EMPTY, 0)
        } else {
            match &cell.value {
                Value::Text(t) => {
                    let slot = self.text_slot(t);
                    (TEXT, slot)
                }
                other => encode(other),
            }
        };
        let style = if blank { 0 } else { self.style_slot(cell.style) };
        let array = match &cell.value {
            Value::Array(a) => Some(a.clone()),
            _ => None,
        };
        let formula = cell.formula.clone();
        let row = self.rows.entry(row_key).or_default();
        row.cover(col);
        let existing = row.index_of(col);
        row.set_formula(col, if blank { None } else { formula.as_ref() });
        row.set_array(col, if blank { None } else { array.as_ref() });
        match existing {
            Some(i) if blank => row.remove_at(i, col),
            Some(i) => {
                row.kinds.set(i, u16::from(kind));
                row.payloads[i] = payload;
                row.styles.set(i, style);
            }
            None if !blank => {
                let i = row.rank_before(col);
                row.kinds.grow(i, u16::from(kind));
                row.payloads.insert(i, payload);
                row.styles.grow(i, style);
                let slot = col - row.first;
                row.bits[(slot / 64) as usize] |= 1u64 << (slot % 64);
            }
            None => {}
        }
        if row.is_empty() {
            self.rows.remove(&row_key);
        }
    }

    /// Builds a whole row at once, from cells already sorted by column. Every array is sized
    /// once, so there is no growth slack to give back and no insert-per-cell shifting.
    fn put_row(&mut self, row_key: u32, cells: &mut Vec<(u32, Cell)>) {
        let mut kept: Vec<(u32, u8, u64, u16)> = Vec::with_capacity(cells.len());
        let mut formulas: Vec<(u32, Arc<Formula>)> = Vec::new();
        let mut arrays: Vec<(u32, Arc<gridcraft_core::Array>)> = Vec::new();
        for (col, cell) in cells.drain(..) {
            if cell.is_blank() {
                continue;
            }
            let (kind, payload) = match &cell.value {
                Value::Text(t) => {
                    let slot = self.text_slot(t);
                    (TEXT, slot)
                }
                other => encode(other),
            };
            let style = self.style_slot(cell.style);
            if let Some(f) = &cell.formula {
                formulas.push((col, f.clone()));
            }
            if let Value::Array(a) = &cell.value {
                arrays.push((col, a.clone()));
            }
            kept.push((col, kind, payload, style));
        }
        if kept.is_empty() {
            self.rows.remove(&row_key);
            return;
        }
        let first = kept[0].0 / 64 * 64;
        let last = kept[kept.len() - 1].0;
        let words = ((last - first) / 64 + 1) as usize;
        let mut bits = vec![0u64; words];
        let mut kinds = Runs::default();
        let mut payloads = Vec::with_capacity(kept.len());
        let mut styles = Runs::default();
        for (col, kind, payload, style) in kept {
            let slot = col - first;
            bits[(slot / 64) as usize] |= 1u64 << (slot % 64);
            kinds.push(u16::from(kind));
            payloads.push(payload);
            styles.push(style);
        }
        formulas.sort_by_key(|(c, _)| *c);
        arrays.sort_by_key(|(c, _)| *c);
        let extras = if formulas.is_empty() && arrays.is_empty() { None } else { Some(Box::new(RowExtras { formulas, arrays })) };
        kinds.compact();
        styles.compact();
        let row = Row { first, bits, kinds, payloads, styles, extras };
        self.rows.insert(row_key, row);
    }

    fn take(&mut self, row_key: u32, col: u32) -> Option<Cell> {
        let i = self.rows.get(&row_key)?.index_of(col)?;
        let cell = self.cell_at(self.rows.get(&row_key)?, i, col);
        let row = self.rows.get_mut(&row_key)?;
        row.set_formula(col, None);
        row.set_array(col, None);
        row.remove_at(i, col);
        if row.is_empty() {
            self.rows.remove(&row_key);
        }
        Some(cell)
    }

    /// True when both bands hold the same cells. Text is compared by content: two bands that came
    /// from different reads index different tables.
    fn same_as(&self, other: &Band) -> bool {
        self.rows.len() == other.rows.len()
            && self.rows.keys().eq(other.rows.keys())
            && self.rows.iter().all(|(k, a)| other.rows.get(k).is_some_and(|b| row_same_as(self, a, other, b)))
    }
}

/// The kind and payload for a value. Text slots are per band, so this needs the band.
fn encode(value: &Value) -> (u8, u64) {
    match value {
        Value::Number(n) => (NUMBER, if *n == 0.0 { 0f64.to_bits() } else { n.to_bits() }),
        Value::Text(_) => (TEXT, 0),
        Value::Bool(b) => (BOOL, u64::from(*b)),
        Value::Error(e) => (ERROR, error_code(*e)),
        Value::Array(_) => (ARRAY, 0),
        Value::Empty => (EMPTY, 0),
    }
}

fn row_same_as(a: &Band, ra: &Row, b: &Band, rb: &Row) -> bool {
    if ra.first != rb.first || ra.bits != rb.bits || ra.kinds.runs != rb.kinds.runs {
        return false;
    }
    for i in 0..ra.kinds.len() {
        if a.style(ra.style_at(i)) != b.style(rb.style_at(i)) {
            return false;
        }
        match ra.kind_at(i) {
            TEXT => {
                if a.text(ra.payloads[i]).as_ref() != b.text(rb.payloads[i]).as_ref() {
                    return false;
                }
            }
            ARRAY => {}
            _ => {
                if ra.payloads[i] != rb.payloads[i] {
                    return false;
                }
            }
        }
    }
    let texts = |row: &Row| -> Vec<(u32, String)> { row.formulas().iter().map(|(c, f)| (*c, f.text.clone())).collect() };
    texts(ra) == texts(rb)
}

/// A `CellStore`'s packed bands, plus the count of stored cells.
#[derive(Clone, Debug, Default)]
pub struct CellStore {
    bands: BTreeMap<u32, Arc<Band>>,
    len: usize,
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
    cells: &'a Row,
    col: u32,
    hi: u32,
    i: usize,
}

impl Iterator for PackedRow<'_> {
    type Item = (CellRef, Cell);
    fn next(&mut self) -> Option<(CellRef, Cell)> {
        while self.col <= self.hi && self.i < self.cells.kinds.len() {
            let slot = self.col - self.cells.first;
            let word = (slot / 64) as usize;
            let w = self.cells.bits.get(word).copied()?;
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

fn packed_row<'a>(band: &'a Band, row: u32, cells: &'a Row, c0: u32, c1: u32) -> PackedRow<'a> {
    let (lo, hi) = match cells.column_span() {
        Some(_) => (c0.max(cells.first), c1.min(cells.first.saturating_add(cells.columns()).saturating_sub(1))),
        None => (1, 0),
    };
    let col = lo.max(cells.first);
    let i = cells.rank_before(col);
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
        let row = band.rows.get(&(c.row % BAND))?;
        let i = row.index_of(c.col)?;
        Some(band.cell_at(row, i, c.col))
    }

    /// True when the position holds a cell at all (a stored style counts).
    pub fn contains(&self, c: CellRef) -> bool {
        self.bands.get(&(c.row / BAND)).and_then(|b| b.rows.get(&(c.row % BAND))).and_then(|r| r.index_of(c.col)).is_some()
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
        let band = Arc::make_mut(self.bands.entry(key).or_default());
        // A row that isn't there yet — every row while a file opens — is built in one go: the
        // arrays are sized once from the cells instead of grown cell by cell.
        if band.rows.get(&band_row).is_none_or(Row::is_empty) {
            let before = band.rows.get(&band_row).map_or(0, |r| r.kinds.len());
            band.put_row(band_row, cells);
            let after = band.rows.get(&band_row).map_or(0, |r| r.kinds.len());
            self.len = self.len + after - before;
        } else {
            for (col, cell) in cells.drain(..) {
                let had = band.rows.get(&band_row).is_some_and(|r| r.index_of(col).is_some());
                band.put(band_row, col, cell);
                let has = band.rows.get(&band_row).is_some_and(|r| r.index_of(col).is_some());
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

    pub fn iter_range(&self, r: RangeRef) -> impl Iterator<Item = (CellRef, Cell)> + '_ {
        let (r0, r1) = (r.start.row, r.end.row);
        self.bands.range(r0 / BAND..=r1 / BAND).flat_map(move |(bk, band)| {
            band.rows
                .iter()
                .filter_map(move |(in_band, cells)| {
                    let row = bk * BAND + *in_band;
                    (row >= r0 && row <= r1).then(|| packed_row(band, row, cells, r.start.col, r.end.col))
                })
                .flatten()
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = (CellRef, Cell)> + '_ {
        self.bands
            .iter()
            .flat_map(move |(bk, band)| band.rows.iter().flat_map(move |(in_band, cells)| packed_row(band, bk * BAND + *in_band, cells, 0, u32::MAX)))
    }

    pub fn row(&self, row: u32, c0: u32, c1: u32) -> impl Iterator<Item = (u32, Cell)> + '_ {
        let packed = self.bands.get(&(row / BAND)).and_then(|b| b.rows.get(&(row % BAND)).map(|cells| packed_row(b, row, cells, c0, c1)));
        packed.into_iter().flatten().map(|(at, cell)| (at.col, cell))
    }

    pub fn used_range(&self) -> Option<RangeRef> {
        let mut r0 = u32::MAX;
        let mut c0 = u32::MAX;
        let mut r1 = 0u32;
        let mut c1 = 0u32;
        for (bk, band) in &self.bands {
            for (row, cells) in &band.rows {
                let Some((first, last)) = cells.column_span() else { continue };
                let at = bk * BAND + row;
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
            band.rows.iter().flat_map(move |(row, cells)| {
                cells.formulas().iter().map(move |(col, f)| {
                    let has_value = cells.index_of(*col).is_some_and(|i| cells.kind_at(i) != EMPTY);
                    (CellRef::new(bk * BAND + *row, *col), f, has_value)
                })
            })
        })
    }

    /// A stored cell with a value or a formula (a style alone does not count).
    pub fn has(&self, c: CellRef) -> bool {
        self.bands.get(&(c.row / BAND)).and_then(|b| b.rows.get(&(c.row % BAND))).is_some_and(|row| row_has_value(row, c.col))
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
            for (in_band, row) in band.rows.range(start..) {
                let at = band_key * BAND + *in_band;
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
                rows.extend(a.rows.keys().copied());
            }
            if let Some(b) = b {
                rows.extend(b.rows.keys().copied());
            }
            rows.sort_unstable();
            rows.dedup();
            for row in rows {
                let (ra, rb) = (a.and_then(|x| x.rows.get(&row)), b.and_then(|x| x.rows.get(&row)));
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
                    let x = a.and_then(|x| ra.and_then(|r| r.index_of(col).map(|i| (x, r, i))));
                    let y = b.and_then(|y| rb.and_then(|r| r.index_of(col).map(|i| (y, r, i))));
                    let same = match (x, y) {
                        (None, None) => true,
                        (Some((xb, xr, xi)), Some((yb, yr, yi))) => {
                            xb.style(xr.style_at(xi)) == yb.style(yr.style_at(yi))
                                && xr.kind_at(xi) == yr.kind_at(yi)
                                && match xr.kind_at(xi) {
                                    TEXT => xb.text(xr.payloads[xi]) == yb.text(yr.payloads[yi]),
                                    ARRAY => {
                                        let at =
                                            |row: &Row| row.arrays().binary_search_by_key(&col, |(c, _)| *c).ok().map(|k| row.arrays()[k].1.clone());
                                        at(xr) == at(yr)
                                    }
                                    _ => xr.payloads[xi] == yr.payloads[yi],
                                }
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
            for row in band.rows.values() {
                bytes += std::mem::size_of::<Row>()
                    + row.bits.len() * 8
                    + row.kinds.runs.len() * 4
                    + row.payloads.len() * 8
                    + row.styles.runs.len() * 4
                    + row.formulas().len() * (4 + std::mem::size_of::<Formula>())
                    + row.arrays().len() * 8;
            }
            for t in &band.texts {
                bytes += t.len() + std::mem::size_of::<Arc<str>>();
            }
            for f in band.rows.values().flat_map(|r| r.formulas().iter().map(|(_, f)| f)) {
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
            out[7] += size_of::<Band>() + band.texts.capacity() * size_of::<Arc<str>>() + band.styles.capacity() * size_of::<StyleId>();
            out[7] += band.text_index.capacity() * (size_of::<usize>() + size_of::<u32>() + 1);
            out[6] += band.styles.capacity() * size_of::<StyleId>();
            out[5] += band.texts.capacity() * size_of::<Arc<str>>();
            out[0] += band.rows.len() * size_of::<(u32, Row)>();
            for row in band.rows.values() {
                out[1] += row.bits.capacity() * 8;
                out[2] += row.kinds.runs.capacity() * 4;
                out[3] += row.payloads.capacity() * 8;
                out[4] += row.styles.runs.capacity() * 4;
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

fn row_has_value(row: &Row, col: u32) -> bool {
    match row.index_of(col) {
        Some(i) => row.kind_at(i) != EMPTY || row.formula_at(col).is_some(),
        None => false,
    }
}

impl Row {
    fn value_nonempty(&self, band: &Band, col: u32) -> bool {
        match self.index_of(col) {
            Some(i) => match self.kind_at(i) {
                EMPTY => false,
                TEXT => !band.text(self.payloads[i]).is_empty(),
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
        let mut runs = Runs::default();
        let mut plain: Vec<u16> = Vec::new();
        for step in 0..4_000 {
            let op = next(4);
            let value = next(3) as u16;
            let mut did = String::new();
            match op {
                0 => {
                    runs.push(value);
                    plain.push(value);
                }
                1 => {
                    let i = next(plain.len() as u64 + 1);
                    runs.grow(i, value);
                    plain.insert(i, value);
                    did = format!("insert({i}, {value})");
                }
                2 if !plain.is_empty() => {
                    let i = next(plain.len() as u64);
                    runs.shrink(i);
                    plain.remove(i);
                    did = format!("remove({i})");
                }
                3 if !plain.is_empty() => {
                    let i = next(plain.len() as u64);
                    runs.set(i, value);
                    plain[i] = value;
                    did = format!("set({i}, {value})");
                }
                _ => {}
            }
            assert_eq!(runs.len(), plain.len(), "step {step}: length");
            if step % 25 == 0 {
                for (i, v) in plain.iter().enumerate() {
                    assert_eq!(runs.at(i), *v, "step {step}: at {i} of {plain:?}");
                }
            }
            assert!(
                runs.runs.windows(2).all(|w| w[0].0 != w[1].0),
                "step {step} {did}: adjacent runs hold the same value: {:?} plain {plain:?}",
                runs.runs
            );
        }
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
}
