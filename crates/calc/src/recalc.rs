//! Dependency graph and recalculation.
//!
//! The graph maps every formula cell to its static precedents (resolved areas). A reverse index
//! answers "which formulas read this cell?": single-cell precedents in a hash map, ranges as row
//! intervals per column (wide ranges per sheet), each an interval index so a lookup costs
//! O(log n + matches) however many ranges share the column. After an edit, the dirty set is the
//! transitive closure of dependents; it is evaluated in topological order, with on-demand
//! evaluation as a safety net for dynamic references. Cycles produce `#CIRC!` unless iterative
//! calculation is on: then a formula read while it is being evaluated gives its previous value,
//! and the formulas that read one are evaluated again until the loop settles
//! (`PassHost::iterate_loops`).

use std::collections::{BTreeSet, VecDeque};
use std::hash::{BuildHasherDefault, Hasher};

/// A fast, non-cryptographic hasher for cell keys (FxHash-style multiply-rotate).
#[derive(Default, Clone, Copy)]
pub struct FxHasher(u64);

impl Hasher for FxHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.write_u64(*b as u64);
        }
    }
    fn write_u64(&mut self, i: u64) {
        self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
    fn write_u32(&mut self, i: u32) {
        self.write_u64(i as u64);
    }
    fn write_usize(&mut self, i: usize) {
        self.write_u64(i as u64);
    }
}

type HashMap<K, V> = std::collections::HashMap<K, V, BuildHasherDefault<FxHasher>>;
type HashSet<K> = std::collections::HashSet<K, BuildHasherDefault<FxHasher>>;

use gridcraft_core::{Array, CellError, CellRef, RangeRef, Value};
use gridcraft_formula::{Expr, UnOp};
use gridcraft_model::{CalcMode, Workbook};

use crate::eval::{Area, Evaluator, Host, precedents};

pub type Key = (usize, CellRef);

#[derive(Clone, Debug, Default)]
struct Node {
    areas: Vec<Area>,
    dynamic: bool,
    /// The parsed formula, shared with the cell: a pass reads it from here instead of rebuilding
    /// the cell (which copies its value and both side tables) once per formula.
    expr: Option<std::sync::Arc<Expr>>,
    /// The range of a legacy (Ctrl+Shift+Enter) array formula, if this is its anchor.
    legacy: Option<RangeRef>,
    /// The formula may return an array and therefore spill: it cannot be evaluated on a thread of
    /// its own while other formulas are being laid out (the spill layout is a pass of its own).
    may_spill: bool,
    /// A reference is read through the sheet's used range (see [`crate::eval::trimmed`]), so the
    /// value changes when the used range moves even if no cell the formula names changed.
    trim: bool,
}

/// Ranges wider than this many columns go to the per-sheet wide index instead of per-column ones.
const WIDE: u32 = 64;

/// Insertions an [`Intervals`] keeps aside (scanned by every query) before merging them in.
const PENDING: usize = 32;

/// Formulas a formula may calculate on demand one inside the other (each needing one another's
/// value before it is done). Each level takes stack, so a chain deeper than this is suspended and
/// calculated from the top instead (see [`PassHost::calculate`]): long dependency chains calculate
/// whatever order they come in, with bounded stack.
const MAX_NESTED: usize = 64;

/// Formula evaluations iterative calculation may spend on one recalculation (passes × formulas
/// evaluated again), however high Maximum Iterations is.
const MAX_ITERATION_WORK: usize = 10_000_000;

/// Row intervals `[r0, r1]`, each with a payload, answering "which intervals overlap rows
/// `a..=b`?" in O(log n + k).
///
/// The intervals are sorted by start with an implicit segment tree of their largest end, so a query
/// only descends into subtrees that hold a match. Removals mark entries dead (re-inserting one
/// revives it); insertions wait in a small sorted set. [`Intervals::settle`] merges them and drops
/// dead entries once either grows, so mutations cost O(log n) amortised.
#[derive(Clone, Debug)]
struct Intervals<T> {
    /// Sorted, without duplicates.
    items: Vec<(u32, u32, T)>,
    dead: Vec<bool>,
    dead_count: usize,
    /// Segment tree over `items`: node `i` has children `2i` and `2i + 1`, leaves start at
    /// `max_end.len() / 2`. Each node holds the largest end below it (dead entries included).
    max_end: Vec<u32>,
    /// Inserted since the last merge.
    pending: BTreeSet<(u32, u32, T)>,
}

impl<T> Default for Intervals<T> {
    fn default() -> Self {
        Intervals { items: Vec::new(), dead: Vec::new(), dead_count: 0, max_end: Vec::new(), pending: BTreeSet::new() }
    }
}

impl<T: Copy + Ord> Intervals<T> {
    fn is_empty(&self) -> bool {
        self.items.len() == self.dead_count && self.pending.is_empty()
    }

    fn insert(&mut self, r0: u32, r1: u32, t: T) {
        let it = (r0, r1, t);
        if let Ok(i) = self.items.binary_search(&it) {
            if let Some(d) = self.dead.get_mut(i)
                && *d
            {
                *d = false;
                self.dead_count -= 1;
            }
            return;
        }
        self.pending.insert(it);
    }

    fn remove(&mut self, r0: u32, r1: u32, t: T) {
        let it = (r0, r1, t);
        if let Ok(i) = self.items.binary_search(&it) {
            if let Some(d) = self.dead.get_mut(i)
                && !*d
            {
                *d = true;
                self.dead_count += 1;
            }
            return;
        }
        self.pending.remove(&it);
    }

    /// Merges pending insertions and drops dead entries when there are enough of them.
    fn settle(&mut self) {
        if self.pending.len() <= PENDING && self.dead_count <= (self.items.len() / 2).max(PENDING) {
            return;
        }
        let mut all: Vec<(u32, u32, T)> = Vec::with_capacity(self.items.len() - self.dead_count + self.pending.len());
        all.extend(self.items.iter().zip(&self.dead).filter(|(_, d)| !**d).map(|(it, _)| *it));
        all.extend(std::mem::take(&mut self.pending));
        // Two sorted runs: the stable sort merges them in linear time.
        all.sort();
        all.dedup();
        let size = all.len().next_power_of_two();
        let mut max_end = vec![0; 2 * size];
        if let Some(leaves) = max_end.get_mut(size..) {
            for (slot, it) in leaves.iter_mut().zip(&all) {
                *slot = it.1;
            }
        }
        for i in (1..size).rev() {
            let m = max_end.get(2 * i).copied().unwrap_or(0).max(max_end.get(2 * i + 1).copied().unwrap_or(0));
            if let Some(slot) = max_end.get_mut(i) {
                *slot = m;
            }
        }
        self.dead = vec![false; all.len()];
        self.dead_count = 0;
        self.items = all;
        self.max_end = max_end;
    }

    /// Calls `f` for every interval overlapping rows `a..=b` (`r0 <= b && r1 >= a`). Returns the
    /// number of tree nodes visited.
    fn query(&self, a: u32, b: u32, f: &mut dyn FnMut(&T)) -> usize {
        for it in &self.pending {
            if it.0 <= b && it.1 >= a {
                f(&it.2);
            }
        }
        // Entries before `p` start at or before row `b`.
        let p = self.items.partition_point(|it| it.0 <= b);
        if p == 0 {
            return 0;
        }
        self.walk(1, 0, self.max_end.len() / 2, p, a, f)
    }

    fn walk(&self, node: usize, lo: usize, hi: usize, p: usize, a: u32, f: &mut dyn FnMut(&T)) -> usize {
        if lo >= p || self.max_end.get(node).is_none_or(|m| *m < a) {
            return 1;
        }
        if hi - lo <= 1 {
            if let (Some(it), Some(false)) = (self.items.get(lo), self.dead.get(lo))
                && it.1 >= a
            {
                f(&it.2);
            }
            return 1;
        }
        let mid = lo + (hi - lo) / 2;
        1 + self.walk(2 * node, lo, mid, p, a, f) + self.walk(2 * node + 1, mid, hi, p, a, f)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Graph {
    nodes: HashMap<Key, Node>,
    /// single cell → dependents
    cell_deps: HashMap<Key, HashSet<Key>>,
    /// (sheet, col) → row intervals of the ranges (up to `WIDE` columns) that cover the column
    col_deps: HashMap<(usize, u32), Intervals<Key>>,
    /// sheet → row intervals of wider ranges, with their columns: (first, last, dependent)
    wide_deps: HashMap<usize, Intervals<(u32, u32, Key)>>,
    dynamic: HashSet<Key>,
    /// Formulas whose value may be an array (and so spill), and the anchors of legacy arrays.
    /// A pass checks these against its dirty set: a dirty one means the pass cannot be split.
    may_spill: HashSet<Key>,
    legacy: HashSet<Key>,
    /// Formulas with a reference read through the used range. An edit that moves the used range
    /// (or a spill that does) has to evaluate them again: they read cells it didn't name.
    trim: HashSet<Key>,
    /// Range indexes changed since the last [`Graph::settle`].
    unsettled_cols: HashSet<(usize, u32)>,
    unsettled_wide: HashSet<usize>,
}

impl Graph {
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Index entries of a formula's precedents, without duplicates: single cells, column
    /// intervals and wide ranges.
    fn entries(areas: &[Area]) -> (Vec<Key>, Vec<(usize, u32, u32, u32)>, Vec<(usize, RangeRef)>) {
        let (mut cells, mut cols, mut wide) = (Vec::new(), Vec::new(), Vec::new());
        for a in areas {
            if a.range.is_single() {
                cells.push((a.sheet, a.range.start));
            } else if a.range.width() > WIDE {
                wide.push((a.sheet, a.range));
            } else {
                for c in a.range.start.col..=a.range.end.col {
                    cols.push((a.sheet, c, a.range.start.row, a.range.end.row));
                }
            }
        }
        cells.sort_unstable();
        cells.dedup();
        cols.sort_unstable();
        cols.dedup();
        wide.sort_unstable_by_key(|(s, r)| (*s, r.start, r.end));
        wide.dedup();
        (cells, cols, wide)
    }

    fn insert(&mut self, k: Key, node: Node) {
        self.remove(k);
        let (cells, cols, wide) = Self::entries(&node.areas);
        for c in cells {
            self.cell_deps.entry(c).or_default().insert(k);
        }
        for (s, c, r0, r1) in cols {
            self.col_deps.entry((s, c)).or_default().insert(r0, r1, k);
            self.unsettled_cols.insert((s, c));
        }
        for (s, r) in wide {
            self.wide_deps.entry(s).or_default().insert(r.start.row, r.end.row, (r.start.col, r.end.col, k));
            self.unsettled_wide.insert(s);
        }
        if node.dynamic {
            self.dynamic.insert(k);
        }
        if node.may_spill {
            self.may_spill.insert(k);
        }
        if node.legacy.is_some() {
            self.legacy.insert(k);
        }
        if node.trim {
            self.trim.insert(k);
        }
        self.nodes.insert(k, node);
    }

    fn remove(&mut self, k: Key) {
        let Some(old) = self.nodes.remove(&k) else { return };
        let (cells, cols, wide) = Self::entries(&old.areas);
        for c in cells {
            if let Some(v) = self.cell_deps.get_mut(&c) {
                v.remove(&k);
                if v.is_empty() {
                    self.cell_deps.remove(&c);
                }
            }
        }
        for (s, c, r0, r1) in cols {
            if let Some(v) = self.col_deps.get_mut(&(s, c)) {
                v.remove(r0, r1, k);
                self.unsettled_cols.insert((s, c));
            }
        }
        for (s, r) in wide {
            if let Some(v) = self.wide_deps.get_mut(&s) {
                v.remove(r.start.row, r.end.row, (r.start.col, r.end.col, k));
                self.unsettled_wide.insert(s);
            }
        }
        self.dynamic.remove(&k);
        self.may_spill.remove(&k);
        self.legacy.remove(&k);
        self.trim.remove(&k);
    }

    /// Tidies the range indexes changed by `insert`/`remove` (queries stay correct without it,
    /// just slower).
    fn settle(&mut self) {
        for key in std::mem::take(&mut self.unsettled_cols) {
            if let Some(v) = self.col_deps.get_mut(&key) {
                v.settle();
                if v.is_empty() {
                    self.col_deps.remove(&key);
                }
            }
        }
        for key in std::mem::take(&mut self.unsettled_wide) {
            if let Some(v) = self.wide_deps.get_mut(&key) {
                v.settle();
                if v.is_empty() {
                    self.wide_deps.remove(&key);
                }
            }
        }
    }

    /// Formulas that read cell `c` of `sheet`.
    pub fn dependents(&self, sheet: usize, c: CellRef, out: &mut Vec<Key>) {
        if let Some(v) = self.cell_deps.get(&(sheet, c)) {
            out.extend(v.iter().copied());
        }
        if let Some(v) = self.col_deps.get(&(sheet, c.col)) {
            v.query(c.row, c.row, &mut |k| out.push(*k));
        }
        if let Some(v) = self.wide_deps.get(&sheet) {
            v.query(c.row, c.row, &mut |(c0, c1, k)| {
                if *c0 <= c.col && c.col <= *c1 {
                    out.push(*k);
                }
            });
        }
    }

    /// Formulas that read any cell of `range` (a formula can be listed more than once).
    pub fn dependents_of_range(&self, sheet: usize, range: RangeRef, out: &mut Vec<Key>) {
        if range.count() <= 4096 {
            for c in range.iter() {
                if let Some(v) = self.cell_deps.get(&(sheet, c)) {
                    out.extend(v.iter().copied());
                }
            }
        } else {
            for ((s, c), v) in &self.cell_deps {
                if *s == sheet && range.contains(*c) {
                    out.extend(v.iter().copied());
                }
            }
        }
        let (r0, r1) = (range.start.row, range.end.row);
        if (range.width() as usize) <= self.col_deps.len() {
            for col in range.start.col..=range.end.col {
                if let Some(v) = self.col_deps.get(&(sheet, col)) {
                    v.query(r0, r1, &mut |k| out.push(*k));
                }
            }
        } else {
            for ((s, col), v) in &self.col_deps {
                if *s == sheet && range.start.col <= *col && *col <= range.end.col {
                    v.query(r0, r1, &mut |k| out.push(*k));
                }
            }
        }
        if let Some(v) = self.wide_deps.get(&sheet) {
            v.query(r0, r1, &mut |(c0, c1, k)| {
                if *c0 <= range.end.col && range.start.col <= *c1 {
                    out.push(*k);
                }
            });
        }
    }

    /// Precedent areas of a formula cell (for Trace Precedents).
    pub fn precedents_of(&self, k: Key) -> Vec<Area> {
        self.nodes.get(&k).map(|n| n.areas.clone()).unwrap_or_default()
    }
}

/// Calculation state kept beside a workbook.
#[derive(Clone, Debug)]
pub struct Calc {
    pub graph: Graph,
    rng: u64,
    /// Fixed "now" for deterministic tests; `None` = system clock.
    pub fixed_now: Option<f64>,
    /// Cells whose formulas have errors from cycles.
    pub circular: Vec<Key>,
    pub last_recalc_cells: usize,
    /// A parallel pass produced a spill or a cycle and was rolled back: keep passes sequential
    /// until a formula changes, rather than paying for the same failed attempt every recalculation.
    parallel_fallback: bool,
    #[cfg(test)]
    parallel_runs: usize,
}

impl Default for Calc {
    fn default() -> Self {
        Calc {
            graph: Graph::default(),
            rng: 0x9E37_79B9_7F4A_7C15,
            fixed_now: None,
            circular: vec![],
            last_recalc_cells: 0,
            parallel_fallback: false,
            #[cfg(test)]
            parallel_runs: 0,
        }
    }
}

/// Text keys a block may hold before the pass stops indexing it (and all blocks together): an
/// index costs ~60 bytes a key.
const MAX_INDEXED_TEXT_VALUES: usize = 200_000;
const MAX_INDEXED_TEXT_VALUES_TOTAL: usize = 2_000_000;

/// Case-folded text → the first and last position holding it.
///
/// Case folding follows `compare_text` exactly (per character, `char::to_lowercase`), because a
/// key that folded differently would answer where a scan would not. Numbers, blanks and errors are
/// not indexed: a non-empty text needle can only match a text entry, so leaving them out cannot
/// lose a match.
struct TextIndex {
    positions: HashMap<String, (u32, u32)>,
}

impl TextIndex {
    fn build(values: &[Value]) -> Self {
        let mut positions: HashMap<String, (u32, u32)> = HashMap::with_capacity_and_hasher(values.len().min(1024), Default::default());
        for (i, value) in values.iter().enumerate() {
            let Value::Text(t) = value else { continue };
            if t.is_empty() {
                continue;
            }
            match positions.entry(fold(t)) {
                std::collections::hash_map::Entry::Occupied(mut e) => e.get_mut().1 = i as u32,
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert((i as u32, i as u32));
                }
            }
        }
        Self { positions }
    }

    fn position(&self, needle: &str, reverse: bool) -> Option<usize> {
        self.positions.get(&fold(needle)).map(|(first, last)| if reverse { *last as usize } else { *first as usize })
    }
}

/// The case folding `compare_text` compares with, as a key.
fn fold(text: &str) -> String {
    text.chars().flat_map(char::to_lowercase).collect()
}

/// Arrays that own their cells when two spills want the same ones: everything already spilling.
/// The reader rebuilds that state from the file (an anchor's `ref` names the range it spilled
/// into, and the values inside it are cached), so F9 after a reload keeps the same owner. A
/// blocked array has no range and is deliberately absent: it must not be laid out before the
/// owner that sits further down the sheet, or it would take the cells by row-major order.
fn spill_owners(wb: &Workbook) -> HashSet<Key> {
    let mut owners: HashSet<Key> = HashSet::default();
    for (si, sh) in wb.sheets.iter().enumerate() {
        owners.extend(sh.spill_ranges.keys().map(|c| (si, *c)));
    }
    owners
}

/// Whether `c` holds content of its own. A cell kept only for its formatting (a fill, a border,
/// an empty value) does not: an array may spill over formatted cells, and a cell that was
/// formatted while a spill covered it must not turn that spill into `#SPILL!` on the next pass.
fn holds_content(sheet: &gridcraft_model::Sheet, c: CellRef) -> bool {
    sheet.cells.get(c).is_some_and(|x| x.formula.is_some() || !x.value.is_empty())
}

/// The largest single block the pass keeps built and shared. A lookup table is small; a whole
/// column read as an array is not, and rebuilding it is cheaper than holding it.
const MAX_CACHED_BLOCK_CELLS: usize = 4_000_000;
/// All shared blocks together, at 24 bytes a value.
const MAX_CACHED_BLOCK_CELLS_TOTAL: usize = 4_000_000;

/// One formula's state in a pass: its value once calculated, `None` while it is being evaluated
/// (which is what makes reading it a circular reference).
#[derive(Clone, Debug, Default)]
struct Slot {
    value: Option<Value>,
}

struct PassHost<'a> {
    wb: &'a Workbook,
    /// The dependency graph as it stands at the start of the pass: an array's own references,
    /// for the self-overlap check.
    graph: &'a Graph,
    exprs: &'a HashMap<Key, std::sync::Arc<Expr>>,
    /// The anchors of legacy array formulas in this pass (usually none).
    legacy: &'a HashMap<Key, RangeRef>,
    results: HashMap<Key, Slot>,
    /// Formulas whose slot has no value yet. Zero for all of a row-major pass, which is when the
    /// "is it already being evaluated?" hash lookup can be skipped.
    in_flight: usize,
    /// New spills found during the pass: anchor → array.
    spills: HashMap<Key, std::sync::Arc<Array>>,
    /// Used range per sheet: the workbook doesn't change during a pass, and working it out walks
    /// every row, so once per sheet rather than once per range read.
    used: HashMap<usize, Option<RangeRef>>,
    /// Blocks with no formula in them, read once a pass and shared. A reporting workbook's eighty
    /// thousand lookups read the same few columns; rebuilding each block costs ~100 µs a formula.
    blocks: HashMap<(usize, RangeRef), std::sync::Arc<Array>>,
    block_cells: usize,
    /// Text-key indexes per shared block, keyed by the block's address. The block itself is held
    /// here so its address stays unique while the index exists.
    text_indexes: HashMap<usize, Option<(std::sync::Arc<Array>, std::sync::Arc<TextIndex>)>>,
    indexed_values: usize,
    rng: &'a mut u64,
    now: f64,
    cycle: bool,
    cycles: Vec<Key>,
    depth: usize,
    /// A formula needed at [`MAX_NESTED`] levels deep: the evaluations in progress are abandoned
    /// and it is calculated first.
    suspend: Option<Key>,
    /// Formulas whose evaluation was abandoned for another one, to be calculated again after it.
    /// They all need it, so reading one of them while it is calculated is a circular reference,
    /// as reading a formula in progress is.
    waiting: HashSet<Key>,
    /// The formulas abandoned since the last suspension.
    abandoned: Vec<Key>,
    /// Other formulas read at [`MAX_NESTED`] levels deep by the evaluations being abandoned. They
    /// are calculated along with the suspended one, rather than one per restart (a wide range
    /// read at the limit would restart the abandoned evaluations once per cell in it).
    also: Vec<Key>,
    /// Suspensions so far (tests check that a wide read at the limit needs one, not one per cell).
    #[cfg(test)]
    suspensions: usize,
    /// The formulas in `cycles`.
    cycle_seen: HashSet<Key>,
    /// Iterative calculation: a circular reference reads the cell's previous value instead of
    /// giving `#CIRC!`.
    iterate: bool,
    /// Values from the previous iteration; cells not in it read their stored value.
    prev: HashMap<Key, Value>,
    /// Formulas being evaluated, innermost last (kept only when iterating).
    stack: Vec<Key>,
    /// Formulas found on a circular reference (when iterating).
    members: HashSet<Key>,
    /// Results that read a previous value, directly or through other formulas.
    tainted: HashSet<Key>,
    /// Whether the formula being evaluated has read a previous value.
    taint: bool,
}

impl<'a> PassHost<'a> {
    /// Whether a spill produced by this pass lies over `range`.
    fn spills_into(&self, sheet: usize, range: RangeRef) -> bool {
        self.spills.iter().any(|((s, anchor), arr)| {
            if *s != sheet {
                return false;
            }
            let end = CellRef::new(
                anchor.row.saturating_add((arr.rows as u32).saturating_sub(1)),
                anchor.col.saturating_add((arr.cols as u32).saturating_sub(1)),
            );
            let spilled = RangeRef::new(*anchor, end);
            spilled.start.row <= range.end.row
                && range.start.row <= spilled.end.row
                && spilled.start.col <= range.end.col
                && range.start.col <= spilled.end.col
        })
    }

    fn new(
        wb: &'a Workbook,
        graph: &'a Graph,
        exprs: &'a HashMap<Key, std::sync::Arc<Expr>>,
        legacy: &'a HashMap<Key, RangeRef>,
        rng: &'a mut u64,
        now: f64,
    ) -> Self {
        PassHost {
            wb,
            graph,
            exprs,
            legacy,
            results: HashMap::with_capacity_and_hasher(exprs.len(), Default::default()),
            in_flight: 0,
            spills: HashMap::default(),
            used: HashMap::default(),
            blocks: HashMap::default(),
            block_cells: 0,
            text_indexes: HashMap::default(),
            indexed_values: 0,
            rng,
            now,
            cycle: false,
            cycles: vec![],
            depth: 0,
            suspend: None,
            waiting: HashSet::default(),
            abandoned: Vec::new(),
            also: Vec::new(),
            #[cfg(test)]
            suspensions: 0,
            cycle_seen: HashSet::default(),
            iterate: false,
            prev: HashMap::default(),
            stack: Vec::new(),
            members: HashSet::default(),
            tainted: HashSet::default(),
            taint: false,
        }
    }
}

/// Whether spilling `a` at `k` would write cells that `k`'s own formula reads. Reading a cell the
/// same formula writes is a circular reference (Excel: `#CIRC!`), not a conflict with anything
/// else: `=TRANSPOSE(A1:B6)` in A2 spills over part of its own source. A range that only touches
/// the anchor is the anchor reading itself, which the graph's cycle detection already refuses.
fn reads_own_spill(host: &PassHost<'_>, (sheet, anchor): Key, a: &Array) -> bool {
    let Some(node) = host.graph.nodes.get(&(sheet, anchor)) else { return false };
    let end =
        CellRef::new(anchor.row.saturating_add((a.rows as u32).saturating_sub(1)), anchor.col.saturating_add((a.cols as u32).saturating_sub(1)));
    let range = RangeRef::new(anchor, end);
    node.areas.iter().any(|area| area.sheet == sheet && area.range.intersects(&range))
}

/// Whether an array result at `k` can't spill: it is in a table, or its area runs off the sheet,
/// overlaps merged cells, holds other cells, or another formula's spill (unless that formula is
/// recalculated in this pass, and so may spill elsewhere now).
fn spill_blocked(host: &PassHost<'_>, (sheet, anchor): Key, a: &Array) -> bool {
    let Some(sh) = host.wb.sheet(sheet) else { return false };
    let end =
        CellRef::new(anchor.row.saturating_add((a.rows as u32).saturating_sub(1)), anchor.col.saturating_add((a.cols as u32).saturating_sub(1)));
    let range = RangeRef::new(anchor, end);
    let own = sh.spill_ranges.get(&anchor);
    let spilled_over = |c: CellRef| {
        !own.is_some_and(|o| o.contains(c))
            && sh.spill_ranges.iter().any(|(o, r)| *o != anchor && r.contains(c) && !host.exprs.contains_key(&(sheet, *o)))
    };
    end.row >= gridcraft_core::MAX_ROWS
        || end.col >= gridcraft_core::MAX_COLS
        || sh.merges.iter().any(|m| m.intersects(&range))
        || sh.tables.iter().any(|t| t.range.contains(anchor))
        || sh.cells.iter_range(range).any(|(c, cell)| c != anchor && (!cell.value.is_empty() || cell.formula.is_some()))
        || sh.spill.range(range.start..=range.end).any(|(c, _)| range.contains(*c) && spilled_over(*c))
}

/// Whether `arr`, anchored at `anchor`, would spill exactly what the sheet already holds over
/// `range`: the same area, nothing new in the way, and the same values.
fn spills_as_before(sheet: &gridcraft_model::Sheet, anchor: CellRef, range: RangeRef, arr: &Array) -> bool {
    let end =
        CellRef::new(anchor.row.saturating_add((arr.rows as u32).saturating_sub(1)), anchor.col.saturating_add((arr.cols as u32).saturating_sub(1)));
    RangeRef::new(anchor, end) == range
        && !sheet.merges.iter().any(|m| m.intersects(&range))
        && !sheet.tables.iter().any(|t| t.range.contains(anchor))
        && range
            .iter()
            .enumerate()
            .all(|(i, c)| c == anchor || (!holds_content(sheet, c) && sheet.spill.get(&c) == Some(arr.data.get(i).unwrap_or(&Value::Empty))))
}

impl PassHost<'_> {
    /// Calculates `k` (and the formulas it reads) with bounded stack: when [`Self::compute`] would
    /// go deeper than [`MAX_NESTED`], it suspends what is in progress, the formula it needed is
    /// calculated from here, then the suspended ones again (reading the results kept so far).
    fn calculate(&mut self, k: Key) {
        // Formulas to calculate, innermost last, each with the ones abandoned to calculate the
        // next.
        let mut work: Vec<(Key, Vec<Key>)> = vec![(k, Vec::new())];
        while let Some(&(top, _)) = work.last() {
            self.compute(top);
            match self.suspend.take() {
                Some(next) => {
                    #[cfg(test)]
                    {
                        self.suspensions += 1;
                    }
                    let held = std::mem::take(&mut self.abandoned);
                    self.waiting.extend(held.iter().copied());
                    if let Some(entry) = work.last_mut() {
                        entry.1 = held;
                    }
                    // The other formulas read at the limit are calculated after `next`, before the
                    // held evaluations restart. Each has nothing held of its own, so the held ones
                    // are released only when the last of them is done.
                    let mut seen: HashSet<Key> = HashSet::default();
                    seen.insert(next);
                    for s in std::mem::take(&mut self.also) {
                        if seen.insert(s) {
                            work.push((s, Vec::new()));
                        }
                    }
                    work.push((next, Vec::new()));
                }
                None => {
                    work.pop();
                    if let Some(entry) = work.last_mut() {
                        for a in entry.1.drain(..) {
                            self.waiting.remove(&a);
                        }
                    }
                }
            }
        }
    }

    fn compute(&mut self, k: Key) -> Value {
        if let Some(v) = self.results.get(&k).and_then(|s| s.value.as_ref()) {
            let v = v.clone();
            if self.iterate && self.tainted.contains(&k) {
                self.taint = true;
            }
            return v;
        }
        // A slot without a value is a formula being evaluated: reading it is a circular reference.
        let running = self.in_flight > 0 && self.results.contains_key(&k);
        let Some(expr) = self.exprs.get(&k) else {
            return self.wb.sheet(k.0).map(|s| s.value(k.1)).unwrap_or_default();
        };
        if self.suspend.is_some() {
            // Unwinding to `calculate`: the value is thrown away. A formula read at the limit is
            // calculated with the suspended one, so the evaluations are not restarted for each.
            if self.depth >= MAX_NESTED && !running && !self.waiting.contains(&k) {
                self.also.push(k);
            }
            return Value::Error(CellError::Calc);
        }
        if running || (!self.waiting.is_empty() && self.waiting.contains(&k)) {
            self.cycle = true;
            if self.iterate {
                return self.circular_read(k);
            }
            // Once each, though a suspended evaluation meets it again.
            if self.cycle_seen.insert(k) {
                self.cycles.push(k);
            }
            return Value::Error(CellError::Circ);
        }
        if self.depth >= MAX_NESTED {
            self.suspend = Some(k);
            return Value::Error(CellError::Calc);
        }
        self.results.insert(k, Slot::default());
        self.in_flight += 1;
        self.depth += 1;
        let outer_taint = std::mem::replace(&mut self.taint, false);
        if self.iterate {
            self.stack.push(k);
        }
        let expr = std::sync::Arc::clone(expr);
        let v = {
            let mut ev = Evaluator::new(self, k.0, k.1);
            ev.value(&expr)
        };
        if self.suspend.is_some() {
            // Abandoned: `calculate` evaluates this formula again once the one it needs is done.
            self.depth -= 1;
            self.results.remove(&k);
            self.in_flight = self.in_flight.saturating_sub(1);
            self.abandoned.push(k);
            if self.iterate {
                self.stack.pop();
            }
            self.taint = outer_taint;
            return Value::Error(CellError::Calc);
        }
        // A legacy array formula fills the range it was entered in.
        let legacy = if self.legacy.is_empty() { None } else { self.legacy.get(&k).copied() };
        let v = match legacy {
            Some(range) => fit_to_range(v, range),
            None => v,
        };
        self.depth -= 1;
        if self.iterate {
            self.stack.pop();
            if self.taint {
                self.tainted.insert(k);
            } else {
                self.tainted.remove(&k);
            }
        }
        self.taint |= outer_taint;
        // A blank cell shows as 0 in a formula's result, inside an array as well (`=A1:A3`,
        // FILTER or SORT of a range with blanks). GROUPBY and PIVOTBY lay out blank cells
        // of their own, which stay blank.
        let keep_blanks = matches!(&*expr, Expr::Call(n, _) if matches!(n.as_str(), "GROUPBY" | "PIVOTBY"));
        let v = match v {
            Value::Array(a) => {
                let a = if keep_blanks || !a.data.iter().any(|x| matches!(x, Value::Empty)) {
                    a
                } else {
                    let data = a.data.iter().map(|x| if matches!(x, Value::Empty) { Value::Number(0.0) } else { x.clone() }).collect();
                    Array::new(a.rows, a.cols, data).map(std::sync::Arc::new).unwrap_or(a)
                };
                // A dynamic-array formula anchors a spill range even while its result is one
                // value -- a FILTER matching one row, a SORT or UNIQUE over one item -- so `C1#`
                // is the 1 x 1 range and `ROWS(C1#)` is 1, as in Excel (#1491). A formula that
                // returns a scalar never reaches this arm, so it does not become an anchor.
                self.spills.insert(k, a.clone());
                a.data.first().cloned().unwrap_or_default()
            }
            Value::Empty => Value::Number(0.0),
            v => v,
        };
        // An array that can't spill is #SPILL! for the formulas evaluated after it in this pass too.
        // One that spills over its own source is circular: no layout, `#CIRC!`.
        let v = match self.spills.get(&k) {
            Some(a) if reads_own_spill(self, k, a) => {
                self.spills.remove(&k);
                if self.cycle_seen.insert(k) {
                    self.cycles.push(k);
                }
                Value::Error(CellError::Circ)
            }
            Some(a) if spill_blocked(self, k, a) => {
                self.spills.remove(&k);
                Value::Error(CellError::Spill)
            }
            _ => v,
        };
        self.results.insert(k, Slot { value: Some(v.clone()) });
        self.in_flight = self.in_flight.saturating_sub(1);
        v
    }

    /// A formula's previous value for iterative calculation: from the last iteration, else the
    /// value it shows. Blank, and the `#CIRC!` of a loop calculated with iteration off, are 0.
    fn previous(&self, k: Key) -> Value {
        let v = match self.prev.get(&k) {
            Some(v) => v.clone(),
            None => self.wb.sheet(k.0).map(|s| s.value(k.1)).unwrap_or_default(),
        };
        if matches!(v, Value::Empty | Value::Error(CellError::Circ)) { Value::Number(0.0) } else { v }
    }

    /// Reading `k` while it is being evaluated, with iterative calculation on: its previous value.
    /// The formulas around the loop are the ones whose changes decide when iteration stops.
    fn circular_read(&mut self, k: Key) -> Value {
        self.taint = true;
        match self.stack.iter().rposition(|s| *s == k) {
            Some(i) => self.members.extend(self.stack.get(i..).unwrap_or_default().iter().copied()),
            // Too deep to follow: the formula itself is watched.
            None => {
                self.members.insert(k);
            }
        }
        self.previous(k)
    }

    /// Iterative calculation, after a first pass over `order` that met circular references. The
    /// formulas that read a previous value (the loops and what depends on them) are evaluated
    /// again in the same order, each pass starting from the last one's values and reading what it
    /// has already produced (Gauss-Seidel), until no formula on a loop changes by `max_change` or
    /// more, or after `max_iterations` passes in all. Returns the number of passes.
    fn iterate_loops(&mut self, order: &[Key], max_iterations: u32, max_change: f64) -> u32 {
        let again: Vec<Key> = order.iter().copied().filter(|k| self.tainted.contains(k)).collect();
        let set: HashSet<Key> = again.iter().copied().collect();
        // Bounds the work whatever Maximum Iterations says.
        let max_passes = u32::try_from(MAX_ITERATION_WORK / again.len().max(1)).unwrap_or(u32::MAX).max(1);
        let mut passes = 1u32;
        while passes < max_iterations.min(max_passes) && !self.converged(max_change) {
            self.prev = again.iter().filter_map(|k| self.results.remove(k).and_then(|s| s.value).map(|v| (*k, v))).collect();
            self.spills.retain(|a, _| !set.contains(a));
            for k in &again {
                self.calculate(*k);
            }
            passes += 1;
        }
        passes
    }

    /// Whether every formula on a loop changed by less than `max_change` in the last pass (text,
    /// logical and error values: not at all).
    fn converged(&self, max_change: f64) -> bool {
        self.members.iter().all(|k| {
            let Some(new) = self.results.get(k).and_then(|s| s.value.as_ref()) else { return true };
            match (new, &self.previous(*k)) {
                (Value::Number(a), Value::Number(b)) => {
                    let d = (a - b).abs();
                    d < max_change || d == 0.0
                }
                (new, old) => new == old,
            }
        })
    }
}

/// A legacy (Ctrl+Shift+Enter) array formula's result fitted to the range it was entered in, as
/// Excel does: a single row or column repeats across the range, a single value fills it, cells
/// past the result show `#N/A`, and a result larger than the range is cut off.
fn fit_to_range(v: Value, range: RangeRef) -> Value {
    if range.count() > crate::eval::MAX_CELLS {
        return Value::Error(CellError::Num);
    }
    let (h, w) = (range.height() as usize, range.width() as usize);
    let Value::Array(a) = v else {
        return if h * w == 1 { v } else { Array::new(h, w, vec![v; h * w]).map(Value::from).unwrap_or(Value::Error(CellError::Value)) };
    };
    if h * w == 1 {
        return a.data.first().cloned().unwrap_or_default();
    }
    let mut data = Vec::with_capacity(h * w);
    for r in 0..h {
        for c in 0..w {
            let (r, c) = (if a.rows == 1 { 0 } else { r }, if a.cols == 1 { 0 } else { c });
            data.push(a.get(r, c).cloned().unwrap_or(Value::Error(CellError::NA)));
        }
    }
    Array::new(h, w, data).map(Value::from).unwrap_or(Value::Error(CellError::Value))
}

impl Host for PassHost<'_> {
    fn workbook(&self) -> &Workbook {
        self.wb
    }
    fn spill_range(&mut self, sheet: usize, anchor: CellRef) -> Option<RangeRef> {
        let k = (sheet, anchor);
        if !self.exprs.contains_key(&k) {
            return self.wb.sheet(sheet)?.spill_ranges.get(&anchor).copied();
        }
        // Recalculated in this pass: its new array, unless something blocks it (#SPILL!).
        self.compute(k);
        let arr = self.spills.get(&k)?;
        let end = CellRef::new(
            anchor.row.saturating_add((arr.rows as u32).saturating_sub(1)),
            anchor.col.saturating_add((arr.cols as u32).saturating_sub(1)),
        );
        let range = RangeRef::new(anchor, end);
        let sh = self.wb.sheet(sheet)?;
        let blocked = end.row >= gridcraft_core::MAX_ROWS
            || end.col >= gridcraft_core::MAX_COLS
            || sh.cells.iter_range(range).any(|(c, cell)| c != anchor && (cell.formula.is_some() || !cell.value.is_empty()))
            || sh.merges.iter().any(|m| m.intersects(&range));
        (!blocked).then_some(range)
    }
    fn cell_value(&mut self, sheet: usize, c: CellRef) -> Value {
        let k = (sheet, c);
        if self.exprs.contains_key(&k) {
            return self.compute(k);
        }
        // A cell covered by a spill computed in this pass.
        for (anchor, arr) in &self.spills {
            if anchor.0 == sheet && c.row >= anchor.1.row && c.col >= anchor.1.col {
                let (dr, dc) = ((c.row - anchor.1.row) as usize, (c.col - anchor.1.col) as usize);
                if dr < arr.rows && dc < arr.cols {
                    if self.iterate && self.tainted.contains(anchor) {
                        self.taint = true;
                    }
                    return arr.get(dr, dc).cloned().unwrap_or_default();
                }
            }
        }
        self.wb.sheet(sheet).map(|s| s.value(c)).unwrap_or_default()
    }
    fn exact_text_position(&mut self, hay: &std::sync::Arc<Array>, needle: &str, reverse: bool) -> Option<Option<usize>> {
        let key = std::sync::Arc::as_ptr(hay) as *const () as usize;
        if let Some(slot) = self.text_indexes.get(&key) {
            return slot.as_ref().map(|(_, index)| index.position(needle, reverse));
        }
        if hay.data.len() > MAX_INDEXED_TEXT_VALUES || self.indexed_values.saturating_add(hay.data.len()) > MAX_INDEXED_TEXT_VALUES_TOTAL {
            self.text_indexes.insert(key, None);
            return None;
        }
        let index = std::sync::Arc::new(TextIndex::build(&hay.data));
        self.indexed_values += hay.data.len();
        self.text_indexes.insert(key, Some((std::sync::Arc::clone(hay), std::sync::Arc::clone(&index))));
        Some(index.position(needle, reverse))
    }

    fn range_array(&mut self, sheet: usize, range: RangeRef) -> Option<std::sync::Arc<Array>> {
        // A block can only be reused while nothing that writes can change it: no formula cell in
        // it still to be worked out, no spill inside it, and no spill this pass has already
        // produced over it.
        if self.spills_into(sheet, range) {
            return None;
        }
        if let Some(hit) = self.blocks.get(&(sheet, range)) {
            return Some(std::sync::Arc::clone(hit));
        }
        let (h, w) = (range.height() as usize, range.width() as usize);
        let cells = h.checked_mul(w)?;
        if cells > MAX_CACHED_BLOCK_CELLS || self.block_cells.saturating_add(cells) > MAX_CACHED_BLOCK_CELLS_TOTAL {
            return None;
        }
        let sh = self.wb.sheet(sheet)?;
        if sh.spill.range(range.start..=range.end).next().is_some() {
            return None;
        }
        // A formula cell in the block usually rules the block out — its value is being worked
        // out. Once it has been, the pass never evaluates it again (`results` is authoritative),
        // so the block is as fixed as a block of constants, and the next formula that reads it
        // gets the same array. That is the shape of a reporting sheet: eighty thousand lookups
        // into one small table whose own few formulas were worked out on the first read.
        // A formula cell is settled when this pass has already worked it out, or when the pass is
        // not working it out at all (it keeps the value the file or the last calculation gave it).
        let stable = !self.iterate
            && sh.cells.formula_cells(range).all(|c| {
                let k = (sheet, c);
                self.results.get(&k).is_some_and(|s| s.value.is_some()) || !self.exprs.contains_key(&k)
            });
        if !stable {
            return None;
        }
        let data = self.range_values(sheet, range);
        let array = std::sync::Arc::new(Array::new(h, w, data)?);
        self.block_cells += cells;
        self.blocks.insert((sheet, range), std::sync::Arc::clone(&array));
        Some(array)
    }
    fn range_values(&mut self, sheet: usize, range: RangeRef) -> Vec<Value> {
        let (h, w) = (range.height() as usize, range.width() as usize);
        let mut out = vec![Value::Empty; h * w];
        let Some(sh) = self.wb.sheet(sheet) else { return out };
        // Sparse walk: only stored cells; formula cells that are (or were) dirty go through
        // `compute`, constants are copied straight from the store.
        let mut formulas: Vec<(usize, CellRef)> = Vec::new();
        for (c, cell) in sh.cells.iter_range(range) {
            let i = (c.row - range.start.row) as usize * w + (c.col - range.start.col) as usize;
            if cell.formula.is_some() {
                formulas.push((i, c));
            } else if let Some(slot) = out.get_mut(i) {
                *slot = cell.value.clone();
            }
        }
        for (c, v) in sh.spill.range(range.start..=range.end) {
            if range.contains(*c) {
                let i = (c.row - range.start.row) as usize * w + (c.col - range.start.col) as usize;
                if let Some(slot) = out.get_mut(i)
                    && slot.is_empty()
                {
                    *slot = v.clone();
                }
            }
        }
        for (i, c) in formulas {
            let v = self.cell_value(sheet, c);
            if let Some(slot) = out.get_mut(i) {
                *slot = v;
            }
        }
        if !self.spills.is_empty() {
            let new: Vec<(usize, Value)> = self
                .spills
                .iter()
                .filter(|(a, _)| a.0 == sheet)
                .flat_map(|(a, arr)| {
                    let mut v = Vec::new();
                    for r in 0..arr.rows {
                        for cc in 0..arr.cols {
                            let c = CellRef::new(a.1.row.saturating_add(r as u32), a.1.col.saturating_add(cc as u32));
                            if c != a.1 && range.contains(c) {
                                v.push((
                                    (c.row - range.start.row) as usize * w + (c.col - range.start.col) as usize,
                                    arr.get(r, cc).cloned().unwrap_or_default(),
                                ));
                            }
                        }
                    }
                    v
                })
                .collect();
            // Iterating: an array spilled by a formula that read a previous value.
            if self.iterate
                && self.spills.iter().any(|(a, arr)| {
                    let end = CellRef::new(
                        a.1.row.saturating_add((arr.rows as u32).saturating_sub(1)),
                        a.1.col.saturating_add((arr.cols as u32).saturating_sub(1)),
                    );
                    a.0 == sheet && self.tainted.contains(a) && RangeRef::new(a.1, end).intersects(&range)
                })
            {
                self.taint = true;
            }
            for (i, v) in new {
                if let Some(slot) = out.get_mut(i) {
                    *slot = v;
                }
            }
        }
        out
    }
    fn visit_aggregate(
        &mut self,
        sheet: usize,
        range: RangeRef,
        kind: crate::eval::Agg,
        acc: &mut crate::eval::Acc,
    ) -> Option<Result<(), gridcraft_core::CellError>> {
        let wb = self.wb;
        let sh = wb.sheet(sheet)?;
        // Spilled values sit between the stored cells, and a spill may hold a number: leave such
        // blocks to `visit_range`, which knows about them.
        if self.spills.keys().any(|a| a.0 == sheet) || sh.spill.range(range.start..=range.end).any(|(c, _)| range.contains(*c)) {
            return None;
        }
        // Decide before folding anything: a block with a formula in it is priced by `visit_range`,
        // which evaluates those first, and folding half of it here would count the rest twice.
        if sh.cells.has_owned_cells(range) {
            return None;
        }
        let mut needs_host = false;
        let mut error = None;
        let done = sh.cells.visit_packed(range, &mut |_, packed| {
            let Some(packed) = packed else {
                needs_host = true;
                return false;
            };
            match acc.add_packed(kind, packed) {
                Ok(()) => true,
                Err(e) => {
                    error = Some(e);
                    false
                }
            }
        });
        if needs_host {
            return None;
        }
        // A stop with no error is the closure's only other exit, so `!done` here means `error`.
        Some(match error {
            Some(e) => Err(e),
            None => {
                let _ = done;
                Ok(())
            }
        })
    }
    fn visit_range(&mut self, sheet: usize, range: RangeRef, f: &mut dyn FnMut(&Value) -> bool) -> bool {
        let wb = self.wb;
        let Some(sh) = wb.sheet(sheet) else { return false };
        // Formula cells first, in row-major order like `range_values`: reading one calculates it
        // and the pass remembers the value, so the walk below gets the same value again without
        // this method holding any.
        for (c, cell) in sh.cells.iter_range(range) {
            if cell.formula.is_some() {
                let _ = self.cell_value(sheet, c);
            }
        }
        // Spilled values sit between the stored cells (and the formulas just calculated may have
        // produced some): leave such blocks to `range_values`.
        if self.spills.keys().any(|a| a.0 == sheet) || sh.spill.range(range.start..=range.end).any(|(c, _)| range.contains(*c)) {
            return false;
        }
        for (c, cell) in sh.cells.iter_range(range) {
            let go = if cell.formula.is_some() { f(&self.cell_value(sheet, c)) } else { f(&cell.value) };
            if !go {
                break;
            }
        }
        true
    }
    fn used_range(&mut self, sheet: usize) -> Option<RangeRef> {
        let wb = self.wb;
        *self.used.entry(sheet).or_insert_with(|| wb.sheet(sheet).and_then(|s| s.used_range()))
    }
    fn now_serial(&self) -> f64 {
        self.now
    }
    fn random(&mut self) -> f64 {
        // xorshift64*
        let mut x = *self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        *self.rng = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Whether the formula may return an array (and so spill). The check is deliberately
/// conservative and only used to keep such formulas out of the parallel pass; a miss falls back to
/// a sequential pass when the pass actually produces a spill.
///
/// Functions that aggregate or look up a block and return one value are listed as scalar: their
/// arguments are not walked, so `SUM(A1:A3)` stays parallel although its argument is a range.
fn may_return_array(e: &Expr) -> bool {
    let mut stack = vec![e];
    while let Some(e) = stack.pop() {
        match e {
            Expr::Array(_) => return true,
            Expr::Ref(r) => {
                if !r.range().is_single() {
                    return true;
                }
            }
            Expr::Unary(UnOp::Spill | UnOp::Trim(_), _) => return true,
            Expr::Call(name, args) => {
                if is_array_function(name) {
                    return true;
                }
                if is_scalar_function(name) {
                    continue;
                }
                stack.extend(args.iter());
            }
            Expr::Binary(_, a, b) => {
                stack.push(b);
                stack.push(a);
            }
            Expr::Unary(_, a) | Expr::Paren(a) => stack.push(a),
            Expr::Invoke(..) => return true,
            Expr::Name(_) | Expr::Struct(_) => return true,
            Expr::Missing | Expr::Number(_) | Expr::Text(_) | Expr::Bool(_) | Expr::Error(_) => {}
        }
    }
    false
}

/// Functions that can return an array of their own.
fn is_array_function(name: &str) -> bool {
    matches!(
        name,
        "FILTER"
            | "SORT"
            | "SORTBY"
            | "UNIQUE"
            | "SEQUENCE"
            | "RANDARRAY"
            | "TRANSPOSE"
            | "TAKE"
            | "DROP"
            | "VSTACK"
            | "HSTACK"
            | "TOROW"
            | "TOCOL"
            | "WRAPROWS"
            | "WRAPCOLS"
            | "EXPAND"
            | "CHOOSEROWS"
            | "CHOOSECOLS"
            | "TEXTSPLIT"
            | "MAP"
            | "SCAN"
            | "MAKEARRAY"
            | "BYROW"
            | "BYCOL"
            | "MMULT"
            | "MINVERSE"
            | "MUNIT"
            | "FREQUENCY"
            | "LINEST"
            | "TREND"
            | "GROWTH"
            | "LOGEST"
            | "GROUPBY"
            | "PIVOTBY"
    )
}

/// Functions that take a block but return one value (or a reference to one): a range argument to
/// one of these does not make the formula an array formula. A function that can also lift over an
/// array (`INDEX`, `XLOOKUP`) is listed here; if it does return an array, the pass falls back to
/// the sequential path when it sees the spill.
fn is_scalar_function(name: &str) -> bool {
    matches!(
        name,
        "SUM"
            | "SUMIF"
            | "SUMIFS"
            | "SUMX2MY2"
            | "SUMX2PY2"
            | "SUMXMY2"
            | "SUMPRODUCT"
            | "COUNT"
            | "COUNTA"
            | "COUNTBLANK"
            | "COUNTIF"
            | "COUNTIFS"
            | "AVERAGE"
            | "AVERAGEIF"
            | "AVERAGEIFS"
            | "MIN"
            | "MAX"
            | "MEDIAN"
            | "MODE"
            | "MODE.SNGL"
            | "PRODUCT"
            | "STDEV"
            | "STDEVP"
            | "STDEV.S"
            | "STDEV.P"
            | "VAR"
            | "VARP"
            | "VAR.S"
            | "VAR.P"
            | "MATCH"
            | "XMATCH"
            | "LOOKUP"
            | "VLOOKUP"
            | "HLOOKUP"
            | "XLOOKUP"
            | "INDEX"
            | "IFERROR"
            | "IFNA"
            | "ROW"
            | "COLUMN"
            | "ROWS"
            | "COLUMNS"
            | "AREAS"
            | "SHEET"
            | "SHEETS"
            | "CELL"
            | "INFO"
            | "TEXTJOIN"
            | "CONCAT"
            | "CONCATENATE"
            | "TEXT"
            | "VALUE"
            | "DATE"
            | "DATEVALUE"
            | "TIME"
            | "TIMEVALUE"
            | "SUBTOTAL"
            | "AGGREGATE"
            | "AND"
            | "OR"
            | "XOR"
    )
}

/// True when evaluating the formula can change on its own: `NOW`, `TODAY`, `RAND`, `OFFSET`…
/// Walking the parsed expression is cheap next to evaluating it, and a cell that reads one of these
/// must be recalculated on open even though the file carries a value for it.
fn is_volatile(e: &Expr) -> bool {
    let mut found = false;
    e.walk(&mut |node| {
        if let Expr::Call(name, _) = node
            && gridcraft_functions::lookup(name).is_some_and(|f| f.volatile)
        {
            found = true;
        }
    });
    found
}

/// Independent formulas a pass needs before splitting across threads: below this, starting
/// threads and building one expression map per thread costs more than the pass.
#[cfg(not(target_arch = "wasm32"))]
const PARALLEL_MIN: usize = 16_384;

/// Largest precedent block a leaf may read and still be a parallel candidate. A formula reading a
/// whole column is left to the sequential pass rather than checking a million cells for a dirty
/// precedent.
#[cfg(not(target_arch = "wasm32"))]
const PARALLEL_LEAF_CELLS: u64 = 256;

#[cfg(not(target_arch = "wasm32"))]
struct Shard {
    values: Vec<(Key, Value)>,
    clean: bool,
}

/// Writes values back without sorting: the caller guarantees row-major order.
fn write_values_ordered(wb: &mut Workbook, values: &[(Key, Value)]) {
    let mut start = 0;
    while start < values.len() {
        let si = values[start].0.0;
        let end = values[start..].partition_point(|(k, _)| k.0 == si) + start;
        if let Some(sheet) = wb.sheet_mut(si) {
            let mut batch: Vec<(CellRef, Value)> = values[start..end].iter().map(|(k, v)| (k.1, v.clone())).collect();
            sheet.cells.set_values(&mut batch);
        }
        start = end;
    }
}

/// Writes values back in row-major order. One sheet at a time, so the sheet's cells are borrowed
/// once rather than looked up per value; the store rewrites the packed value slot in place.
fn write_values(wb: &mut Workbook, mut values: Vec<(Key, Value)>) {
    values.sort_unstable_by_key(|(k, _)| (k.0, k.1.row, k.1.col));
    write_values_ordered(wb, &values);
}

#[cfg(not(target_arch = "wasm32"))]
impl Calc {
    /// Evaluates the independent leaf formulas of `dirty` on the machine's threads and writes their
    /// values, removing them from `dirty`. Returns false, changing nothing, when the pass is not
    /// safe to split or too small to pay for it.
    ///
    /// A formula is a leaf when none of its precedent blocks holds a formula this pass is also
    /// recalculating. Leaves cannot cycle with each other, and because the whole pass is refused
    /// when any dirty formula is dynamic, may spill or is a legacy array, the shards share only
    /// stored values. Each shard scans its slice of the dirty set, evaluates its leaves with its own
    /// expression and result maps, and returns them in row order; the values are merged and written
    /// in that order.
    fn try_parallel(&mut self, wb: &mut Workbook, dirty: &mut Vec<Key>) -> bool {
        if self.parallel_fallback || dirty.len() < PARALLEL_MIN || wb.calc.iterative {
            return false;
        }
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(8);
        if threads < 2 {
            return false;
        }
        let dirty_set: HashSet<Key> = dirty.iter().copied().collect();
        // A dynamic or array formula can read or write cells the static graph does not name; leave
        // the whole pass to the sequential path if one is being recalculated. The graph keeps these
        // sets small (they are formulas that can spill or are volatile), so checking them costs a
        // handful of lookups, not a scan of every formula.
        let intersects = |set: &HashSet<Key>| {
            if set.len() <= dirty_set.len() { set.iter().any(|k| dirty_set.contains(k)) } else { dirty_set.iter().any(|k| set.contains(k)) }
        };
        if intersects(&self.graph.dynamic) || intersects(&self.graph.may_spill) || intersects(&self.graph.legacy) {
            return false;
        }
        let chunk = dirty.len().div_ceil(threads);
        let now = self.now();
        let seed = self.rng;
        let graph = &self.graph;
        let wb_ref: &Workbook = wb;
        let dirty_set = &dirty_set;
        let started = prof_now();
        let shards: Vec<std::thread::Result<Shard>> = std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for (ti, part) in dirty.chunks(chunk).enumerate() {
                handles.push(scope.spawn(move || {
                    let mut local: Vec<Key> = Vec::new();
                    for &k in part {
                        let Some(node) = graph.nodes.get(&k) else { continue };
                        let mut leaf = true;
                        for area in &node.areas {
                            if area.range.count() > PARALLEL_LEAF_CELLS {
                                leaf = false;
                                break;
                            }
                            for c in area.range.iter() {
                                if dirty_set.contains(&(area.sheet, c)) {
                                    leaf = false;
                                    break;
                                }
                            }
                            if !leaf {
                                break;
                            }
                        }
                        if leaf && node.expr.is_some() {
                            local.push(k);
                        }
                    }
                    if local.is_empty() {
                        return Shard { values: Vec::new(), clean: true };
                    }
                    let mut exprs: HashMap<Key, std::sync::Arc<Expr>> = HashMap::with_capacity_and_hasher(local.len(), Default::default());
                    let mut legacy: HashMap<Key, RangeRef> = HashMap::default();
                    for &k in &local {
                        if let Some(node) = graph.nodes.get(&k) {
                            if let Some(e) = &node.expr {
                                exprs.insert(k, std::sync::Arc::clone(e));
                            }
                            if let Some(r) = node.legacy {
                                legacy.insert(k, r);
                            }
                        }
                    }
                    let mut rng = seed ^ (ti as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                    let mut host = PassHost::new(wb_ref, graph, &exprs, &legacy, &mut rng, now);
                    host.iterate = false;
                    for &k in &local {
                        host.calculate(k);
                    }
                    let clean = host.spills.is_empty()
                        && host.cycles.is_empty()
                        && !host.cycle
                        && host.suspend.is_none()
                        && host.abandoned.is_empty()
                        && host.also.is_empty()
                        && host.waiting.is_empty()
                        && host.tainted.is_empty()
                        && host.members.is_empty()
                        && host.depth == 0;
                    // Keep the local order: the values then concatenate into a row-major list.
                    let values: Vec<(Key, Value)> =
                        local.iter().filter_map(|k| host.results.get(k).and_then(|s| s.value.as_ref()).map(|v| (*k, v.clone()))).collect();
                    let clean = clean && values.len() == local.len();
                    Shard { values, clean }
                }));
            }
            handles.into_iter().map(|h| h.join()).collect()
        });
        let eval_ms = prof_now() - started;
        let total: usize = shards.iter().filter_map(|r| r.as_ref().ok()).map(|s| s.values.len()).sum();
        if total < PARALLEL_MIN {
            return false;
        }
        let mut values = Vec::with_capacity(total);
        for shard in shards {
            match shard {
                Ok(shard) if shard.clean => values.extend(shard.values),
                _ => {
                    self.parallel_fallback = true;
                    return false;
                }
            }
        }
        if values.len() != total {
            self.parallel_fallback = true;
            return false;
        }
        let write_started = prof_now();
        write_values_ordered(wb, &values);
        let write_ms = prof_now() - write_started;
        // Remove the candidates from `dirty` in one linear merge: each shard's candidates are a
        // subsequence of its slice of the (row-major) dirty list, and `values` is in that order.
        let mut vi = 0;
        let mut remaining = Vec::with_capacity(dirty.len() - total);
        for part in dirty.chunks(chunk) {
            for &k in part {
                if vi < values.len() && values[vi].0 == k {
                    vi += 1;
                } else {
                    remaining.push(k);
                }
            }
        }
        *dirty = remaining;
        self.last_recalc_cells += total;
        #[cfg(test)]
        {
            self.parallel_runs += 1;
        }
        if std::env::var_os("GRIDCRAFT_PROFILE").is_some() {
            eprintln!("  parallel {total} formulas on {threads} threads: eval {eval_ms:.1} ms, write {write_ms:.1} ms");
        }
        true
    }
}

#[cfg(target_arch = "wasm32")]
impl Calc {
    fn try_parallel(&mut self, _wb: &mut Workbook, _dirty: &mut Vec<Key>) -> bool {
        false
    }
}

impl Calc {
    pub fn new() -> Self {
        Self::default()
    }

    fn now(&self) -> f64 {
        if let Some(n) = self.fixed_now {
            return n;
        }
        now_serial()
    }

    /// Rebuilds the graph from scratch.
    pub fn rebuild(&mut self, wb: &Workbook) {
        self.parallel_fallback = false;
        self.graph = Graph::default();
        for (si, sheet) in wb.sheets.iter().enumerate() {
            for (c, f, _) in sheet.cells.formulas() {
                if let Some(e) = f.expr_arc() {
                    let (areas, dynamic, trim) = precedents(wb, si, &e);
                    self.graph
                        .insert((si, c), Node { areas, dynamic, expr: Some(e.clone()), legacy: f.array, may_spill: may_return_array(&e), trim });
                }
            }
        }
        self.graph.settle();
    }

    /// Recalculates every formula (F9 / Ctrl+Alt+F9, and after loading).
    pub fn recalc_all(&mut self, wb: &mut Workbook) {
        self.rebuild(wb);
        let prefer = spill_owners(wb);
        // Spills are laid out again from the formulas where they are now (a structural edit moves
        // or deletes the formulas, not the values they spilled).
        for i in 0..wb.sheets.len() {
            if wb.sheet(i).is_some_and(|s| !s.spill_ranges.is_empty() || !s.spill.is_empty())
                && let Some(sh) = wb.sheet_mut(i)
            {
                sh.spill.clear();
                sh.spill_ranges.clear();
            }
        }
        let all: Vec<Key> = self.graph.nodes.keys().copied().collect();
        self.run(wb, all, true, &prefer);
    }

    /// Recalculates what a freshly opened workbook needs, and nothing else.
    ///
    /// A file carries the values of its formulas, so recalculating all of them only repeats what it
    /// already says — on a large sheet that is seconds of the open. Values are still computed when
    /// the file asks for it (`fullCalcOnLoad`), when a cell has no cached value (saved by a producer
    /// that did not calculate), when the formula is volatile (a cached `NOW()` is out of date by
    /// definition), and when the value is not in the file at all: a dynamic array's spilled cells
    /// and a legacy array formula's non-anchor cells are derived on open. Their dependents come
    /// along, so nothing is left reading a value the file does not hold.
    pub fn recalc_on_open(&mut self, wb: &mut Workbook) {
        let tp = prof_now();
        self.rebuild(wb);
        let rebuild_ms = prof_now() - tp;
        let ts = prof_now();
        if wb.calc.full_calc_on_load {
            self.recalc_all(wb);
            return;
        }
        // Spilled cells are derived state, never file content.
        let mut seeds: Vec<Key> = self.graph.dynamic.iter().copied().collect();
        for (si, sheet) in wb.sheets.iter().enumerate() {
            for (c, f, has_value) in sheet.cells.formulas() {
                let derived = f.array.is_some()      // a legacy array fills its range
                    || f.dynamic                     // a dynamic array's spilled cells are derived
                    || !has_value; // the producer saved no value
                if derived || f.expr().is_some_and(|e| is_volatile(&e)) {
                    seeds.push((si, c));
                }
            }
        }
        if seeds.is_empty() {
            if std::env::var_os("GRIDCRAFT_PROFILE").is_some() {
                eprintln!("  open: rebuild {rebuild_ms:.1} ms, no stale cells ({} ms)", prof_now() - ts);
            }
            return;
        }
        let stale = self.dirty_closure(wb, &seeds);
        if std::env::var_os("GRIDCRAFT_PROFILE").is_some() {
            eprintln!("  open: rebuild {rebuild_ms:.1} ms, {} seeds, {} stale ({} ms)", seeds.len(), stale.len(), prof_now() - ts);
        }
        self.run(wb, stale, true, &HashSet::default());
    }

    /// Whether any formula on `sheet` reads a reference through the used range, so an edit that
    /// moves the used range has to evaluate it again.
    pub fn trims_used_range(&self, sheet: usize) -> bool {
        self.graph.trim.iter().any(|k| k.0 == sheet)
    }

    /// Updates the graph for cells whose content changed and recalculates what depends on them.
    pub fn cells_changed(&mut self, wb: &mut Workbook, changed: &[Key]) {
        self.cells_changed_with_used(wb, changed, &[]);
    }

    /// [`cells_changed`](Self::cells_changed) for an edit that moved the used range of the sheets
    /// in `used_changed`: their formulas that read a reference through the used range (a whole
    /// column or row, `SUM(A:A)`, `=23:26`) are evaluated again, because the cells they read
    /// changed without any of them being named as a precedent.
    pub fn cells_changed_with_used(&mut self, wb: &mut Workbook, changed: &[Key], used_changed: &[usize]) {
        self.parallel_fallback = false;
        let mut cleared: Vec<(usize, RangeRef)> = Vec::new();
        for &k in changed {
            let formula = wb.sheet(k.0).and_then(|s| s.cell(k.1)).and_then(|c| c.formula.clone());
            let parsed = formula.as_ref().and_then(|f| f.expr_arc().map(|e| (e, f.array)));
            match parsed {
                Some((e, legacy)) => {
                    let (areas, dynamic, trim) = precedents(wb, k.0, &e);
                    let may_spill = may_return_array(&e);
                    self.graph.insert(k, Node { areas, dynamic, expr: Some(e), legacy, may_spill, trim });
                }
                None => {
                    self.graph.remove(k);
                    // A formula replaced by a constant (or cleared) takes its spilled values with it.
                    if let Some(sh) = wb.sheet_mut(k.0)
                        && let Some(r) = sh.spill_ranges.remove(&k.1)
                    {
                        for c in r.iter() {
                            sh.spill.remove(&c);
                        }
                        cleared.push((k.0, r));
                    }
                    // A formula that doesn't parse evaluates to #NAME?.
                    if let Some(sheet) = wb.sheet_mut(k.0)
                        && let Some(mut cell) = sheet.cells.get(k.1)
                        && cell.formula.is_some()
                    {
                        cell.value = Value::Error(CellError::Name);
                        sheet.cells.set(k.1, cell);
                    }
                }
            }
        }
        self.graph.settle();
        if wb.calc.mode == CalcMode::Manual {
            // Only the edited formulas themselves are evaluated.
            let own: Vec<Key> = changed.iter().copied().filter(|k| self.graph.nodes.contains_key(k)).collect();
            self.run(wb, own, false, &HashSet::default());
            return;
        }
        let mut seeds: Vec<Key> = changed.to_vec();
        for &si in used_changed {
            seeds.extend(self.graph.trim.iter().filter(|k| k.0 == si).copied());
        }
        let mut emptied: Vec<Key> = Vec::new();
        // Per-sheet bounds of the edit, so a bulk change scans for blocked arrays once instead of
        // once per cell: redoing a whole row of Remove Duplicates used to walk a 64×64 block for
        // each of a million changed cells.
        let mut bounds: Vec<(usize, u32, u32, u32, u32)> = Vec::new();
        for &k in changed {
            let Some(sh) = wb.sheet(k.0) else { continue };
            match bounds.iter_mut().find(|x| x.0 == k.0) {
                Some(x) => {
                    x.1 = x.1.min(k.1.row);
                    x.2 = x.2.min(k.1.col);
                    x.3 = x.3.max(k.1.row);
                    x.4 = x.4.max(k.1.col);
                }
                None => bounds.push((k.0, k.1.row, k.1.col, k.1.row, k.1.col)),
            }
            if !sh.cells.has(k.1) {
                match emptied.iter_mut().find(|x| x.0 == k.0) {
                    Some(x) => x.1 = CellRef::new(x.1.row.max(k.1.row), x.1.col.max(k.1.col)),
                    None => emptied.push(k),
                }
            }
        }
        // The per-cell scan looks at the 64×64 block above and to the left of each changed cell.
        // For a bulk edit, scan the union of those blocks once (or keep the per-cell scan when the
        // changed cells are scattered and the union would be much larger).
        let per_cell_cost = (changed.len() as u64).saturating_mul(4096);
        let union_cost: u64 = bounds.iter().map(|&(_, r0, c0, r1, c1)| (u64::from(r1 - r0) + 64).saturating_mul(u64::from(c1 - c0) + 64)).sum();
        let bulk_spill_scan = changed.len() > 64 && union_cost <= per_cell_cost;
        if bulk_spill_scan {
            for &(si, r0, c0, r1, c1) in &bounds {
                let Some(sh) = wb.sheet(si) else { continue };
                let area = RangeRef::new(CellRef::new(r0.saturating_sub(64), c0.saturating_sub(64)), CellRef::new(r1, c1));
                for (c, cell) in sh.cells.iter_range(area) {
                    if cell.value == Value::Error(CellError::Spill) {
                        seeds.push((si, c));
                    }
                }
                let body = RangeRef::new(CellRef::new(r0, c0), CellRef::new(r1, c1));
                for (anchor, r) in &sh.spill_ranges {
                    if r.intersects(&body) {
                        seeds.push((si, *anchor));
                    }
                }
            }
        } else {
            // Typing into (or clearing) a cell a dynamic array spills over (or would spill over)
            // re-evaluates the anchor, which may now be blocked or unblocked.
            for &k in changed {
                if let Some(sh) = wb.sheet(k.0) {
                    for (anchor, r) in &sh.spill_ranges {
                        if *anchor != k.1 && r.contains(k.1) {
                            seeds.push((k.0, *anchor));
                        }
                    }
                    for (c, cell) in sh.cells.iter_range(RangeRef::new(CellRef::new(k.1.row.saturating_sub(64), k.1.col.saturating_sub(64)), k.1)) {
                        if c != k.1 && cell.value == Value::Error(CellError::Spill) {
                            seeds.push((k.0, c));
                        }
                    }
                }
            }
        }
        // An emptied cell can unblock an anchor any distance above or to the left of it.
        for (si, end) in emptied {
            for &(s, c) in self.graph.nodes.keys() {
                if s == si && c.row <= end.row && c.col <= end.col && wb.sheet(s).is_some_and(|sh| sh.value(c) == Value::Error(CellError::Spill)) {
                    seeds.push((s, c));
                }
            }
        }
        // Spill areas of changed anchors also change.
        for &k in changed {
            if let Some(r) = wb.sheet(k.0).and_then(|s| s.spill_ranges.get(&k.1)) {
                seeds.extend(r.iter().map(|c| (k.0, c)));
            }
        }
        for (si, r) in &cleared {
            seeds.extend(r.iter().map(|c| (*si, c)));
        }
        let t0 = prof_now();
        let dirty = self.dirty_closure(wb, &seeds);
        let t1 = prof_now();
        let n = dirty.len();
        self.run(wb, dirty, false, &HashSet::default());
        if std::env::var_os("GRIDCRAFT_PROFILE").is_some() {
            eprintln!("recalc: {} seeds, closure {} cells {:.1} ms, run {:.1} ms", seeds.len(), n, t1 - t0, prof_now() - t1);
        }
    }

    fn dirty_closure(&self, _wb: &Workbook, seeds: &[Key]) -> Vec<Key> {
        let mut seen: HashSet<Key> = HashSet::default();
        let mut queue: VecDeque<Key> = VecDeque::new();
        for k in seeds {
            if self.graph.nodes.contains_key(k) && seen.insert(*k) {
                queue.push_back(*k);
            }
            let mut deps = Vec::new();
            self.graph.dependents(k.0, k.1, &mut deps);
            // One seed (the cell every formula reads) can name a million dependents: reserve once
            // instead of growing the set and queue through a dozen rehashes.
            seen.reserve(deps.len());
            queue.reserve(deps.len());
            for d in deps {
                if seen.insert(d) {
                    queue.push_back(d);
                }
            }
        }
        for k in &self.graph.dynamic {
            if seen.insert(*k) {
                queue.push_back(*k);
            }
        }
        let mut deps = Vec::new();
        while let Some(k) = queue.pop_front() {
            deps.clear();
            self.graph.dependents(k.0, k.1, &mut deps);
            for d in deps.drain(..) {
                if seen.insert(d) {
                    queue.push_back(d);
                }
            }
        }
        seen.into_iter().collect()
    }

    /// Evaluates the dirty set and writes results (and spills) back.
    fn run(&mut self, wb: &mut Workbook, mut dirty: Vec<Key>, full: bool, prefer: &HashSet<Key>) {
        let mut dynamic_again = !self.graph.dynamic.is_empty();
        self.last_recalc_cells = 0;
        // Every loop the rounds met: a later round evaluates only its dependents, so the cells
        // that were re-entered are remembered for the UI and `Calc::circular`.
        let mut found_cycles: Vec<Key> = Vec::new();
        for _round in 0..8 {
            if dirty.is_empty() {
                break;
            }
            // Row-major order is a good topological guess; on-demand evaluation fixes the rest.
            dirty.sort_by_key(|(s, c)| (*s, c.row, c.col));
            // Independent leaves are the common shape of a large sheet (a column of `=A1*2`);
            // evaluate them on the machine's threads before the sequential pass takes the rest.
            // Writing them makes their dependents leaves too, so repeat while each wave is large
            // enough to pay for the threads (a layered sheet parallelises level by level).
            if _round == 0 {
                // A layered sheet can have many waves; after a handful the remaining formulas are
                // cheaper to take in one sequential pass than to rescan again and again.
                let mut waves = 0;
                while waves < 16 && self.try_parallel(wb, &mut dirty) {
                    waves += 1;
                    if dirty.is_empty() {
                        break;
                    }
                }
                if dirty.is_empty() {
                    break;
                }
            }
            // The graph already holds each formula's parsed expression (and the range of a
            // legacy array): the pass reads them from there instead of rebuilding 100k cells.
            let mut exprs: HashMap<Key, std::sync::Arc<Expr>> = HashMap::with_capacity_and_hasher(dirty.len(), Default::default());
            let mut legacy: HashMap<Key, RangeRef> = HashMap::default();
            for k in &dirty {
                if let Some(node) = self.graph.nodes.get(k) {
                    if let Some(e) = &node.expr {
                        exprs.insert(*k, std::sync::Arc::clone(e));
                    }
                    if let Some(r) = node.legacy {
                        legacy.insert(*k, r);
                    }
                }
            }
            let now = self.now();
            let mut rng = self.rng;
            let tp = prof_now();
            let (mut results, mut spills, cycles) = {
                let mut host = PassHost::new(wb, &self.graph, &exprs, &legacy, &mut rng, now);
                host.iterate = wb.calc.iterative;
                for k in &dirty {
                    host.calculate(*k);
                }
                if host.iterate && !host.members.is_empty() {
                    let passes = host.iterate_loops(&dirty, wb.calc.max_iterations, wb.calc.max_change);
                    if std::env::var_os("GRIDCRAFT_PROFILE").is_some() {
                        eprintln!("  iterative calculation: {} passes over {} formulas", passes, host.tainted.len());
                    }
                }
                (host.results, host.spills, host.cycles)
            };
            self.rng = rng;
            // A formula that read its own cell is circular even when the function swallowed the
            // #CIRC! the read returned (`COUNTIF` over a column that holds its own formula counts
            // the other cells and ignores the error). Excel shows 0 with a warning; we keep it
            // explicit. Without this the loop's cells hold values that depend on the order the
            // pass happened to compute them in (found by the command-sequence fuzzer).
            //
            // An array this pass laid out but then dropped (the anchor turned out to be circular)
            // leaves the cells it would have spilled over empty. Formulas that read that area —
            // a whole-column `COUNTIF` is one — read its values during the pass, so they are
            // evaluated again after the write-back.
            let mut discarded: Vec<(usize, RangeRef)> = Vec::new();
            if !wb.calc.iterative && !cycles.is_empty() {
                for k in &cycles {
                    results.insert(*k, Slot { value: Some(Value::Error(CellError::Circ)) });
                    if let Some(arr) = spills.remove(k) {
                        let end = CellRef::new(
                            k.1.row.saturating_add((arr.rows as u32).saturating_sub(1)),
                            k.1.col.saturating_add((arr.cols as u32).saturating_sub(1)),
                        );
                        discarded.push((k.0, RangeRef::new(k.1, end)));
                    }
                }
            }
            if std::env::var_os("GRIDCRAFT_PROFILE").is_some() {
                eprintln!("  eval {} formulas {:.1} ms", results.len(), prof_now() - tp);
            }
            let tw = prof_now();
            self.last_recalc_cells += results.len();
            found_cycles.extend(cycles.iter().copied());
            // Write back.
            let mut spill_changes: Vec<Key> = Vec::new();
            for (si, range) in discarded {
                self.graph.dependents_of_range(si, range, &mut spill_changes);
            }
            let mut freed: Vec<(usize, RangeRef)> = Vec::new();
            let twr = prof_now();
            // Row-major order: the results come out of a hash map, and walking bands in row order
            // is what makes the write-back touch each band once instead of jumping between 1,563
            // of them at random. Values only change, so the store rewrites the packed slot and
            // nothing else (no `Cell` built, no formula or style looked up).
            let values: Vec<(Key, Value)> = results.into_iter().filter_map(|(k, s)| s.value.map(|v| (k, v))).collect();
            write_values(wb, values);
            let two = prof_now();
            if std::env::var_os("GRIDCRAFT_PROFILE").is_some() {
                eprintln!("  write values {:.1} ms", two - twr);
            }
            // Arrays that held a spill before this pass lay out first, so a contested area keeps
            // its owner; everything else follows in row-major order.
            let any_spills = !spills.is_empty() || wb.sheets.iter().any(|s| !s.spill_ranges.is_empty() || !s.spill.is_empty());
            let ordered: Vec<Key> = if any_spills {
                dirty.iter().copied().filter(|k| prefer.contains(k)).chain(dirty.iter().copied().filter(|k| !prefer.contains(k))).collect()
            } else {
                Vec::new()
            };
            for k in &ordered {
                let new = spills.get(k);
                let Some(sheet) = wb.sheet_mut(k.0) else { continue };
                let old = sheet.spill_ranges.get(&k.1).copied();
                if old.is_none() && new.is_none() {
                    continue;
                }
                // An array that spills exactly as before changes nothing its dependents read, so
                // they aren't queued again. Otherwise two arrays that read each other's spill areas
                // queue each other in every round, up to the round limit.
                if let (Some(r), Some(arr)) = (old, new)
                    && spills_as_before(sheet, k.1, r, arr)
                {
                    continue;
                }
                if let Some(r) = old {
                    for c in r.iter() {
                        sheet.spill.remove(&c);
                    }
                    sheet.spill_ranges.remove(&k.1);
                }
                // An array that owned its cells before the pass may grow back into cells another
                // array took in an earlier round, after that array was laid out (this pass only
                // learns the new shape now). The owner gets them back: the other array follows in
                // the layout order and is blocked if it still wants them.
                if prefer.contains(k) {
                    let evicted: Vec<(CellRef, RangeRef)> = sheet
                        .spill_ranges
                        .iter()
                        .filter(|(anchor, _)| **anchor != k.1 && !prefer.contains(&(k.0, **anchor)))
                        .map(|(anchor, range)| (*anchor, *range))
                        .collect();
                    for (anchor, range) in evicted {
                        for c in range.iter() {
                            sheet.spill.remove(&c);
                        }
                        sheet.spill_ranges.remove(&anchor);
                        // It has to re-evaluate: the cells it took are gone, and it is either
                        // blocked now or spills somewhere else.
                        spill_changes.push((k.0, anchor));
                    }
                }
                let mut changed_range = old;
                if let Some(arr) = new {
                    let end = CellRef::new(k.1.row.saturating_add(arr.rows as u32 - 1), k.1.col.saturating_add(arr.cols as u32 - 1));
                    let range = RangeRef::new(k.1, end);
                    let blocked = end.row >= gridcraft_core::MAX_ROWS
                        || end.col >= gridcraft_core::MAX_COLS
                        || range.iter().any(|c| c != k.1 && (holds_content(sheet, c) || sheet.spill.contains_key(&c)))
                        || sheet.merges.iter().any(|m| m.intersects(&range))
                        || sheet.tables.iter().any(|t| t.range.contains(k.1));
                    if blocked {
                        if let Some(mut cell) = sheet.cells.get(k.1) {
                            cell.value = Value::Error(CellError::Spill);
                            sheet.cells.set(k.1, cell);
                        }
                        // Not blocked while the pass ran (it would have been #SPILL! there), so
                        // blocked by another array of this pass: the formulas that read this one,
                        // at its anchor or over the area it would have spilled into, read #SPILL!
                        // or empty cells now.
                        let mut deps = Vec::new();
                        self.graph.dependents_of_range(k.0, range, &mut deps);
                        spill_changes.extend(deps.into_iter().filter(|d| d != k));
                    } else {
                        for (i, c) in range.iter().enumerate() {
                            if c != k.1 {
                                sheet.spill.insert(c, arr.data.get(i).cloned().unwrap_or_default());
                            }
                        }
                        sheet.spill_ranges.insert(k.1, range);
                        changed_range = Some(changed_range.map_or(range, |o| o.union(&range)));
                    }
                }
                if let Some(r) = changed_range {
                    let mut deps = Vec::new();
                    self.graph.dependents_of_range(k.0, r, &mut deps);
                    // Not the anchor itself (that would read its own array), unless its old array
                    // is gone or partly freed: its new formula may have read the old values, still
                    // laid out while it was evaluated.
                    let freed = match (old, sheet.spill_ranges.get(&k.1)) {
                        (Some(o), Some(n)) => !(n.contains(o.start) && n.contains(o.end)),
                        (_, None) => true,
                        (None, Some(_)) => false,
                    };
                    spill_changes.extend(deps.into_iter().filter(|d| d != k || freed));
                    // The spill moved the sheet's used range, and a formula that reads a whole
                    // column or row (trimmed to it) reads different cells now even when the
                    // spill lies nowhere near it.
                    spill_changes.extend(self.graph.trim.iter().filter(|t| t.0 == k.0).copied());
                    // A dynamic reference (INDIRECT, OFFSET, a defined name) can hide a read of
                    // the cells the spill just gave back; the graph does not name them.
                    spill_changes.extend(self.graph.dynamic.iter().filter(|d| d.0 == k.0).copied());
                }
                if let Some(o) = old
                    && !sheet.spill_ranges.get(&k.1).is_some_and(|n| n.contains(o.start) && n.contains(o.end))
                {
                    freed.push((k.0, o));
                }
            }
            // A formula that read one of the loop's cells before they were marked #CIRC! holds a
            // value computed from what the cell happened to show then; it is evaluated again so
            // it sees the #CIRC! a full recalculation would have given it.
            let mut cycle_changes: Vec<Key> = Vec::new();
            for k in &cycles {
                self.graph.dependents(k.0, k.1, &mut cycle_changes);
            }
            // The loop's own cells are not evaluated again: they would compute a number from the
            // other cells and their dependents would read it before this pass could mark them
            // #CIRC! again. Their value is already written.
            let cyclic: HashSet<Key> = cycles.iter().copied().collect();
            spill_changes.extend(cycle_changes.into_iter().filter(|k| !cyclic.contains(k)));
            // Formulas with references only known while evaluating (INDIRECT, OFFSET) may have read
            // a spill area before it was laid out: they are evaluated once more.
            if dynamic_again && !spills.is_empty() {
                dynamic_again = false;
                spill_changes.extend(self.graph.dynamic.iter().copied());
            }
            // An area a formula no longer spills over may unblock another formula's array.
            for (si, o) in freed {
                for &(s, c) in self.graph.nodes.keys() {
                    if s == si
                        && c.row <= o.end.row
                        && c.col <= o.end.col
                        && wb.sheet(s).is_some_and(|sh| sh.value(c) == Value::Error(CellError::Spill))
                    {
                        spill_changes.push((s, c));
                    }
                }
            }
            if full {
                // Everything was already evaluated once; spill dependents need another pass.
            }
            if std::env::var_os("GRIDCRAFT_PROFILE").is_some() {
                eprintln!("  spills {:.1} ms, write-back {:.1} ms", prof_now() - two, prof_now() - tw);
            }
            spill_changes.sort_by_key(|(s, c)| (*s, c.row, c.col));
            spill_changes.dedup();
            if spill_changes.is_empty() {
                break;
            }
            let mut next = self.dirty_closure(wb, &spill_changes);
            next.retain(|k| !cyclic.contains(k));
            dirty = next;
        }
        found_cycles.sort_unstable_by_key(|(s, c)| (*s, c.row, c.col));
        found_cycles.dedup();
        self.circular = found_cycles;
    }
}

/// Current date-time as a serial (1900 system), from the system clock (browser clock on wasm).
/// UTC; the local offset isn't available without platform calls. Falls back to a fixed date only
/// when no clock is available.
pub fn now_serial() -> f64 {
    gridcraft_core::date::unix_now_ms().map_or(46302.5, gridcraft_core::date::serial_from_unix_ms)
}

/// Evaluates a formula in the context of a cell without storing it (conditional formats, data
/// validation, Evaluate Formula, Name Manager previews, the CLI `eval`).
pub fn evaluate(wb: &Workbook, sheet: usize, at: CellRef, formula: &str) -> Value {
    let body = formula.strip_prefix('=').unwrap_or(formula);
    let Ok(expr) = gridcraft_formula::parse(body) else { return Value::Error(CellError::Name) };
    evaluate_expr(wb, sheet, at, &expr)
}

pub fn evaluate_expr(wb: &Workbook, sheet: usize, at: CellRef, expr: &Expr) -> Value {
    let exprs = HashMap::default();
    let legacy = HashMap::default();
    let graph = Graph::default();
    let mut rng = 0x1234_5678_9ABC_DEF0u64;
    let mut host = PassHost::new(wb, &graph, &exprs, &legacy, &mut rng, now_serial());
    let mut ev = Evaluator::new(&mut host, sheet, at);
    ev.value(expr)
}

#[allow(clippy::disallowed_methods)] // the clock is read only off wasm
fn prof_now() -> f64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
    }
    #[cfg(target_arch = "wasm32")]
    {
        0.0
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::time::{Duration, Instant};

    use gridcraft_core::{CellError, CellRef, RangeRef, Value};
    use gridcraft_model::{Cell, Formula, Workbook};

    use super::{Calc, Graph, HashMap, Host, Intervals, Key, PassHost};

    /// xorshift64 for repeatable pseudo-random cases.
    fn rng(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    fn found(iv: &Intervals<u32>, a: u32, b: u32) -> Vec<u32> {
        let mut v = Vec::new();
        iv.query(a, b, &mut |t| v.push(*t));
        v.sort_unstable();
        v
    }

    #[test]
    fn intervals_match_a_linear_scan() {
        let mut iv: Intervals<u32> = Intervals::default();
        let mut model: Vec<(u32, u32, u32)> = Vec::new();
        let mut s = 0x9E37_79B9_7F4A_7C15;
        for step in 0..20_000u32 {
            let roll = rng(&mut s) % 10;
            if roll < 6 || model.is_empty() {
                let r0 = (rng(&mut s) % 500) as u32;
                let r1 = r0 + (rng(&mut s) % 40) as u32;
                if !model.contains(&(r0, r1, step)) {
                    iv.insert(r0, r1, step);
                    model.push((r0, r1, step));
                }
            } else {
                let i = (rng(&mut s) % model.len() as u64) as usize;
                let (r0, r1, t) = model.swap_remove(i);
                iv.remove(r0, r1, t);
                if roll == 9 {
                    // Removed and put back before the next merge.
                    iv.insert(r0, r1, t);
                    model.push((r0, r1, t));
                }
            }
            if rng(&mut s).is_multiple_of(50) {
                iv.settle();
            }
            if step.is_multiple_of(97) {
                let a = (rng(&mut s) % 560) as u32;
                let b = a + (rng(&mut s) % 30) as u32;
                let mut want: Vec<u32> = model.iter().filter(|(r0, r1, _)| *r0 <= b && *r1 >= a).map(|(_, _, t)| *t).collect();
                want.sort_unstable();
                assert_eq!(found(&iv, a, b), want, "rows {a}..={b} at step {step}");
            }
        }
        iv.settle();
        for row in 0..560 {
            let mut want: Vec<u32> = model.iter().filter(|(r0, r1, _)| *r0 <= row && row <= *r1).map(|(_, _, t)| *t).collect();
            want.sort_unstable();
            assert_eq!(found(&iv, row, row), want, "row {row}");
        }
        for (r0, r1, t) in std::mem::take(&mut model) {
            iv.remove(r0, r1, t);
        }
        iv.settle();
        assert!(iv.is_empty());
    }

    #[test]
    fn interval_queries_cost_log_n_plus_matches() {
        // 40k one-row ranges (like =SUM(E1:J1) down a column) and 1k ranges over everything:
        // a point query visits O(log n) tree nodes per match, not every range in the column.
        let n = 40_000u32;
        let mut iv: Intervals<u32> = Intervals::default();
        for r in 0..n {
            iv.insert(r, r, r);
        }
        for t in 0..1_000 {
            iv.insert(0, n, n + t);
        }
        iv.settle();
        let mut worst = 0;
        for row in (0..n).step_by(7) {
            let mut k = 0;
            let visited = iv.query(row, row, &mut |_| k += 1);
            assert_eq!(k, 1_001);
            worst = worst.max(visited);
        }
        // 1,001 matches in a tree of depth 16: well under 4 nodes per match plus the path.
        assert!(worst < 4 * 1_001 + 64, "visited {worst} nodes");
        let mut k = 0;
        let visited = iv.query(n + 1, n + 5, &mut |_| k += 1);
        assert_eq!(k, 0);
        assert!(visited < 64, "visited {visited} nodes for no match");
    }

    /// `rows` rows of six numbers (E:J) and `=SUM(E{r}:J{r})` in K.
    fn sum_rows(rows: u32) -> Workbook {
        let mut wb = Workbook::new();
        if let Some(sh) = wb.sheet_mut(0) {
            for r in 0..rows {
                for c in 4..10 {
                    sh.cells.set(CellRef::new(r, c), Cell::value(Value::Number(f64::from(r % 97 + c))));
                }
                sh.cells.set(CellRef::new(r, 10), Cell::formula(Formula::new(&format!("=SUM(E{0}:J{0})", r + 1))));
            }
        }
        wb
    }

    /// Opening (full recalc), changing every value of a column, and re-entering every formula.
    #[allow(clippy::disallowed_methods)] // a native-only timing test: the clock is read off wasm
    fn open_and_bulk_edit(rows: u32) -> Duration {
        let mut wb = sum_rows(rows);
        let mut calc = Calc::new();
        let t = Instant::now();
        calc.recalc_all(&mut wb);
        let values: Vec<Key> = (0..rows).map(|r| (0, CellRef::new(r, 4))).collect();
        if let Some(sh) = wb.sheet_mut(0) {
            for (_, c) in &values {
                sh.cells.set(*c, Cell::value(Value::Number(1.0)));
            }
        }
        calc.cells_changed(&mut wb, &values);
        let formulas: Vec<Key> = (0..rows).map(|r| (0, CellRef::new(r, 10))).collect();
        calc.cells_changed(&mut wb, &formulas);
        let elapsed = t.elapsed();
        let last = wb.sheet(0).map(|s| s.value(CellRef::new(rows - 1, 10)));
        let r = rows - 1;
        assert_eq!(last, Some(Value::Number(f64::from(1 + (5..10).map(|c| r % 97 + c).sum::<u32>()))));
        elapsed
    }

    #[test]
    fn range_formulas_recalc_and_bulk_edit_correctly() {
        // The values behind the timing test below, checked on every run.
        open_and_bulk_edit(500);
    }

    #[test]
    #[ignore = "timing-sensitive; run with --ignored in release"]
    #[allow(clippy::disallowed_methods)]
    fn recalc_with_range_formulas_scales_linearly() {
        // Each row's SUM reads a range in the same columns. Recalculating on open used to walk
        // every row per range read (the used range), and finding a cell's dependents scanned
        // every range in its column: 4x the rows took 16x the time. Linear is 4x; allow 8x.
        let best = |rows| (0..3).map(|_| open_and_bulk_edit(rows)).min().unwrap_or_default();
        let small = best(5_000);
        let large = best(20_000);
        assert!(large < small * 8 + Duration::from_millis(100), "5k rows {small:?}, 20k rows {large:?}");
    }

    #[test]
    fn range_dependents_after_edits() {
        // Formulas that read ranges, wide ranges and whole columns, found again after edits.
        let mut wb = Workbook::new();
        let mut calc = Calc::new();
        let set = |wb: &mut Workbook, calc: &mut Calc, at: &str, input: &str| {
            let c = CellRef::parse(at).unwrap();
            let cell = if input.starts_with('=') { Cell::formula(Formula::new(input)) } else { Cell::value(Value::Number(input.parse().unwrap())) };
            wb.sheet_mut(0).unwrap().cells.set(c, cell);
            calc.cells_changed(wb, &[(0, c)]);
        };
        let num = |wb: &Workbook, at: &str| {
            let v = wb.sheet(0).unwrap().value(CellRef::parse(at).unwrap());
            v.as_f64().unwrap_or_else(|| panic!("{at} = {v:?}"))
        };
        set(&mut wb, &mut calc, "A1", "1");
        set(&mut wb, &mut calc, "B2", "2");
        set(&mut wb, &mut calc, "CZ3", "3");
        set(&mut wb, &mut calc, "DA5", "=SUM(A1:B2)");
        set(&mut wb, &mut calc, "DA6", "=SUM(A1:CZ3)");
        set(&mut wb, &mut calc, "DA7", "=SUM(B:B)");
        set(&mut wb, &mut calc, "DA8", "=SUM(2:2)");
        assert_eq!((num(&wb, "DA5"), num(&wb, "DA6"), num(&wb, "DA7"), num(&wb, "DA8")), (3.0, 6.0, 2.0, 2.0));
        set(&mut wb, &mut calc, "B2", "20");
        assert_eq!((num(&wb, "DA5"), num(&wb, "DA6"), num(&wb, "DA7"), num(&wb, "DA8")), (21.0, 24.0, 20.0, 20.0));
        // Re-entering a formula with another range drops the old one.
        set(&mut wb, &mut calc, "DA5", "=SUM(A1:A2)");
        set(&mut wb, &mut calc, "B2", "5");
        assert_eq!(num(&wb, "DA5"), 1.0);
        let mut deps = Vec::new();
        calc.graph.dependents(0, CellRef::parse("B2").unwrap(), &mut deps);
        deps.sort_unstable();
        let at = |s: &str| (0, CellRef::parse(s).unwrap());
        assert_eq!(deps, vec![at("DA6"), at("DA7"), at("DA8")]);
        deps.clear();
        calc.graph.dependents_of_range(0, RangeRef::new(CellRef::parse("A1").unwrap(), CellRef::parse("A9").unwrap()), &mut deps);
        deps.sort_unstable();
        deps.dedup();
        assert_eq!(deps, vec![at("DA5"), at("DA6"), at("DA8")]);
        set(&mut wb, &mut calc, "CZ3", "30");
        assert_eq!(num(&wb, "DA6"), 36.0);
        // Ranges on another sheet.
        wb.sheets.push(std::sync::Arc::new(gridcraft_model::Sheet::new("Data")));
        set(&mut wb, &mut calc, "DA9", "=SUM(Data!A1:A3)+SUM(Data!A:CZ)");
        assert_eq!(num(&wb, "DA9"), 0.0);
        wb.sheet_mut(1).unwrap().cells.set(CellRef::parse("A2").unwrap(), Cell::value(Value::Number(4.0)));
        calc.cells_changed(&mut wb, &[(1, CellRef::parse("A2").unwrap())]);
        assert_eq!(num(&wb, "DA9"), 8.0);
        deps.clear();
        calc.graph.dependents(1, CellRef::parse("A2").unwrap(), &mut deps);
        assert_eq!(deps, vec![at("DA9"), at("DA9")]);
    }

    /// `n` rows of `text` in column J, with A1 = 1 and B1 = 2 as inputs. Above the parallel
    /// threshold, every row is an independent leaf.
    fn leaf_book(n: u32, text: &str) -> Workbook {
        let mut wb = Workbook::new();
        if let Some(sh) = wb.sheet_mut(0) {
            sh.cells.set(CellRef::new(0, 0), Cell::value(Value::Number(1.0)));
            sh.cells.set(CellRef::new(0, 1), Cell::value(Value::Number(2.0)));
            for r in 0..n {
                sh.cells.set(CellRef::new(r, 9), Cell::formula(Formula::new(text)));
            }
        }
        wb
    }

    fn can_parallel() -> bool {
        std::thread::available_parallelism().is_ok_and(|n| n.get() > 1)
    }

    fn column_j(wb: &Workbook, n: u32) -> Vec<Value> {
        (0..n).map(|r| wb.sheet(0).unwrap().value(CellRef::new(r, 9))).collect()
    }

    #[test]
    fn independent_leaves_recalc_on_threads() {
        let n = 17_000u32;
        let mut wb = leaf_book(n, "=$A$1*2");
        let mut calc = Calc::new();
        calc.recalc_all(&mut wb);
        if can_parallel() {
            assert!(calc.parallel_runs > 0, "the parallel path did not run");
        }
        assert!(column_j(&wb, n).iter().all(|v| *v == Value::Number(2.0)));
        wb.sheet_mut(0).unwrap().cells.set(CellRef::new(0, 0), Cell::value(Value::Number(10.0)));
        calc.cells_changed(&mut wb, &[(0, CellRef::new(0, 0))]);
        if can_parallel() {
            assert!(calc.parallel_runs > 1, "the edit did not run the parallel path");
        }
        assert!(column_j(&wb, n).iter().all(|v| *v == Value::Number(20.0)));
    }

    #[test]
    fn independent_range_leaves_recalc_on_threads() {
        let n = 17_000u32;
        let mut wb = leaf_book(n, "=SUM($A$1:$B$1)");
        let mut calc = Calc::new();
        calc.recalc_all(&mut wb);
        if can_parallel() {
            assert!(calc.parallel_runs > 0, "the parallel path did not run");
        }
        assert!(column_j(&wb, n).iter().all(|v| *v == Value::Number(3.0)));
        wb.sheet_mut(0).unwrap().cells.set(CellRef::new(0, 0), Cell::value(Value::Number(4.0)));
        calc.cells_changed(&mut wb, &[(0, CellRef::new(0, 0))]);
        assert!(column_j(&wb, n).iter().all(|v| *v == Value::Number(6.0)));
    }

    /// Writing the first wave makes the next layer independent: the pass repeats, so a layered
    /// sheet parallelises level by level rather than falling back to one sequential pass.
    #[test]
    fn a_second_wave_parallelises_after_the_first() {
        let n = 17_000u32;
        let mut wb = leaf_book(n, "=$A$1*2");
        if let Some(sh) = wb.sheet_mut(0) {
            for r in 0..n {
                sh.cells.set(CellRef::new(r, 10), Cell::formula(Formula::new(&format!("=J{}+1", r + 1))));
            }
        }
        let mut calc = Calc::new();
        calc.recalc_all(&mut wb);
        if can_parallel() {
            assert!(calc.parallel_runs >= 2, "the second wave did not run on threads");
        }
        for r in 0..n {
            assert_eq!(wb.sheet(0).unwrap().value(CellRef::new(r, 9)), Value::Number(2.0));
            assert_eq!(wb.sheet(0).unwrap().value(CellRef::new(r, 10)), Value::Number(3.0));
        }
    }

    /// Clearing a blocked array's cell along with a bulk change (Remove Duplicates, a paste, a
    /// whole-row clear) still finds the anchor: the blocked-anchor scan covers the union of the
    /// changed cells' 64×64 blocks once.
    #[test]
    fn a_bulk_change_unblocks_a_blocked_array() {
        let mut wb = Workbook::new();
        {
            let sh = wb.sheet_mut(0).expect("sheet");
            sh.cells.set(CellRef::new(0, 0), Cell::value(Value::Number(1.0)));
            sh.cells.set(CellRef::new(1, 0), Cell::value(Value::Number(2.0)));
            sh.cells.set(CellRef::new(0, 10), Cell::formula(Formula::new("=A1:A2")));
            sh.cells.set(CellRef::new(1, 10), Cell::value(Value::Number(3.0)));
            for r in 0..1_000 {
                sh.cells.set(CellRef::new(r, 1), Cell::value(Value::Number(4.0)));
            }
        }
        let mut calc = Calc::new();
        calc.recalc_all(&mut wb);
        assert_eq!(wb.sheet(0).unwrap().value(CellRef::new(0, 10)), Value::Error(CellError::Spill));
        let mut changed = vec![(0, CellRef::new(1, 10))];
        wb.sheet_mut(0).unwrap().cells.remove(CellRef::new(1, 10));
        for r in 0..1_000 {
            wb.sheet_mut(0).unwrap().cells.remove(CellRef::new(r, 1));
            changed.push((0, CellRef::new(r, 1)));
        }
        calc.cells_changed(&mut wb, &changed);
        assert_eq!(wb.sheet(0).unwrap().value(CellRef::new(0, 10)), Value::Number(1.0));
        assert_eq!(wb.sheet(0).unwrap().value(CellRef::new(1, 10)), Value::Number(2.0));
    }

    #[test]
    fn a_dynamic_formula_keeps_the_pass_sequential() {
        let n = 17_000u32;
        let mut wb = leaf_book(n, "=$A$1*2");
        wb.sheet_mut(0).unwrap().cells.set(CellRef::new(0, 10), Cell::formula(Formula::new("=RAND()")));
        let mut calc = Calc::new();
        calc.recalc_all(&mut wb);
        assert_eq!(calc.parallel_runs, 0, "a volatile formula must keep the pass sequential");
        assert!(column_j(&wb, n).iter().all(|v| *v == Value::Number(2.0)));
        assert!(matches!(wb.sheet(0).unwrap().value(CellRef::new(0, 10)), Value::Number(_)));
    }

    #[test]
    fn an_array_formula_keeps_the_pass_sequential() {
        let n = 17_000u32;
        let mut wb = leaf_book(n, "=$A$1*2");
        wb.sheet_mut(0).unwrap().cells.set(CellRef::new(1, 0), Cell::value(Value::Number(2.0)));
        wb.sheet_mut(0).unwrap().cells.set(CellRef::new(0, 10), Cell::formula(Formula::new("=A1:A2")));
        let mut calc = Calc::new();
        calc.recalc_all(&mut wb);
        assert_eq!(calc.parallel_runs, 0, "an array formula must keep the pass sequential");
        assert!(column_j(&wb, n).iter().all(|v| *v == Value::Number(2.0)));
        assert_eq!(wb.sheet(0).unwrap().value(CellRef::new(0, 10)), Value::Number(1.0));
        assert_eq!(wb.sheet(0).unwrap().value(CellRef::new(1, 10)), Value::Number(2.0));
    }

    /// A formula at the nesting limit reading thousands of formulas not calculated yet suspends the
    /// evaluations in progress once, not once per formula read.
    #[test]
    fn wide_read_at_the_nesting_limit_suspends_once() {
        const WIDTH: u32 = 3000;
        let mut wb = Workbook::new();
        let mut exprs = HashMap::default();
        let mut put = |wb: &mut Workbook, at: String, text: String| {
            let at = CellRef::parse(&at).unwrap();
            wb.sheet_mut(0).unwrap().cells.set(at, Cell::formula(Formula::new(&text)));
            exprs.insert((0, at), std::sync::Arc::new(gridcraft_formula::parse(&text[1..]).unwrap()));
        };
        // A1 reads A2, ..., A64 (the 64th formula deep) reads a wide range of formulas.
        for r in 1..64 {
            put(&mut wb, format!("A{r}"), format!("=A{}+1", r + 1));
        }
        put(&mut wb, "A64".to_string(), format!("=SUM(B1:B{WIDTH})"));
        for r in 1..=WIDTH {
            put(&mut wb, format!("B{r}"), format!("={r}*2"));
        }
        let mut rng = 1u64;
        let graph = Graph::default();
        let legacy = HashMap::default();
        let mut host = PassHost::new(&wb, &graph, &exprs, &legacy, &mut rng, 0.0);
        host.calculate((0, CellRef::parse("A1").unwrap()));
        let total = (WIDTH * (WIDTH + 1)) as f64;
        assert_eq!(host.results.get(&(0, CellRef::parse("A1").unwrap())).and_then(|s| s.value.as_ref()), Some(&Value::Number(total + 63.0)));
        assert!(host.cycles.is_empty());
        assert!(host.suspensions <= 3, "{} suspensions", host.suspensions);
    }

    /// For tests of `PassHost::visit_range`, which can't be reached from outside this module: the
    /// values it hands over for `range` while the formulas in `pending` (cell, formula) have not been
    /// calculated yet, or `None` when it declines the block.
    pub(crate) fn probe_visit_range(wb: &Workbook, pending: &[(&str, &str)], range: RangeRef) -> Option<Vec<Value>> {
        let mut exprs = HashMap::default();
        for (at, formula) in pending {
            let expr = gridcraft_formula::parse(formula.strip_prefix('=').unwrap_or(formula)).ok()?;
            exprs.insert((0, CellRef::parse(at)?), std::sync::Arc::new(expr));
        }

        // A cell in `pending` is calculated the first time something reads it, which is the state
        // of a real pass partway through; `visit_range` has to cope with the formulas it reads
        // being calculated under it.
        let mut rng = 1u64;
        let graph = Graph::default();
        let legacy = HashMap::default();
        let mut host = PassHost::new(wb, &graph, &exprs, &legacy, &mut rng, 0.0);

        let mut seen = Vec::new();
        host.visit_range(0, range, &mut |v| {
            seen.push(v.clone());
            true
        })
        .then_some(seen)
    }
}
