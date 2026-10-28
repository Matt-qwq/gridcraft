//! Row and column metadata (`LineInfo`) kept under physical line ids.
//!
//! The sheet's logical line numbers are what formulas, the UI and files speak. A structural edit
//! (insert or delete rows/columns) only changes where a logical line points: the entries keep
//! their physical keys, so an insert or delete moves no metadata at all, and the sheet's undo
//! snapshot does not copy every entry.
//!
//! The entries live in chunks of [`LINE_CHUNK`] lines behind an `Arc`, so cloning a [`LineMap`]
//! (an undo snapshot) copies the chunk pointers, and editing one line copies at most one chunk —
//! the same copy-on-write arrangement as the cell store's bands, and for the same reason.
//!
//! The logical→physical mapping itself is a small sorted segment list ([`LineMap`]'s private
//! `LineAxisMap`), the row-map the cell store has always kept: identity until the first structural
//! edit, then runs that shift as a whole. Inserted lines start out unmapped (they have no entry
//! until something writes one), which is why an insert does not touch a single chunk.
//!
//! JSON keeps the shape a `BTreeMap<u32, LineInfo>` had — logical keys, no map state — because the
//! document format is part of the application's contract.

use std::collections::BTreeMap;
use std::ops::{Bound, RangeBounds};
use std::sync::Arc;

use gridcraft_core::{MAX_COLS, MAX_ROWS};
use serde::ser::SerializeMap as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::features::LineInfo;

/// Lines per chunk: an edit copies at most this many entries, and a snapshot shares the rest.
pub const LINE_CHUNK: u32 = 128;

/// Which axis a [`LineMap`] describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineAxis {
    Rows,
    Cols,
}

impl LineAxis {
    /// The number of lines the axis has: 1,048,576 rows or 16,384 columns.
    pub fn limit(self) -> u32 {
        match self {
            LineAxis::Rows => MAX_ROWS,
            LineAxis::Cols => MAX_COLS,
        }
    }
}

/// One run of lines whose logical and physical ids advance together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LineSegment {
    pub(crate) logical: u32,
    pub(crate) physical: u32,
    pub(crate) len: u32,
}

impl LineSegment {
    fn logical_end(self) -> u32 {
        self.logical.saturating_add(self.len)
    }

    fn physical_end(self) -> u32 {
        self.physical.saturating_add(self.len)
    }
}

/// Logical→physical lines. Identity (`logical == physical`) until the first structural edit, then
/// runs sorted by logical line; inserted lines are gaps with no run until something writes them.
///
/// Mirrors the cell store's row map: a shift moves only the runs, an allocated line gets a fresh
/// physical id above every id handed out so far, and adjacent runs merge afterwards without ever
/// renumbering an id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LineAxisMap {
    identity: bool,
    segments: Vec<LineSegment>,
    next_physical: u32,
    limit: u32,
}

impl LineAxisMap {
    fn new(limit: u32) -> LineAxisMap {
        LineAxisMap { identity: true, segments: Vec::new(), next_physical: limit, limit }
    }

    fn map(&self, logical: u32) -> Option<u32> {
        if logical >= self.limit {
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

    /// Materializes the identity run so a structural edit has runs to split.
    fn ensure(&mut self) {
        if !self.identity {
            return;
        }
        self.identity = false;
        self.segments = vec![LineSegment { logical: 0, physical: 0, len: self.limit }];
    }

    /// Allocates one physical id for a logical line that has none (a line inside an inserted gap,
    /// or one exposed by a delete). The fresh id is above every id the map has handed out, so it
    /// cannot collide with an existing run.
    fn allocate(&mut self, logical: u32) -> Option<u32> {
        if let Some(physical) = self.map(logical) {
            return Some(physical);
        }
        self.allocate_run(logical, 1)
    }

    /// Allocates `len` consecutive physical ids for an unmapped logical run and maps it as one
    /// segment, so writing a whole inserted block costs one run instead of one per line.
    fn allocate_run(&mut self, logical: u32, len: u32) -> Option<u32> {
        if len == 0 || logical >= self.limit {
            return None;
        }
        let len = len.min(self.limit - logical);
        self.ensure();
        let physical = self.next_physical;
        self.next_physical = self.next_physical.checked_add(len)?;
        let at = self.segments.partition_point(|s| s.logical < logical);
        self.segments.insert(at, LineSegment { logical, physical, len });
        self.normalize();
        Some(physical)
    }

    /// The runs intersecting the logical range `r0..=r1`, clipped to it, in logical order.
    fn segments_in(&self, r0: u32, r1: u32) -> Vec<LineSegment> {
        let last = self.limit.saturating_sub(1);
        let (r0, r1) = (r0.min(last), r1.min(last));
        if r0 > r1 {
            return Vec::new();
        }
        if self.identity {
            return vec![LineSegment { logical: r0, physical: r0, len: r1 - r0 + 1 }];
        }
        let mut out = Vec::new();
        // Skip the runs that end before the range starts; there can be many after a long editing
        // session, and a range lookup is what every geometry call does.
        let first = self.segments.partition_point(|s| s.logical_end() <= r0);
        for s in self.segments.iter().skip(first) {
            if s.logical > r1 {
                break;
            }
            let lo = s.logical.max(r0);
            let hi = s.logical_end().min(r1.saturating_add(1));
            if lo >= hi {
                continue;
            }
            out.push(LineSegment { logical: lo, physical: s.physical.saturating_add(lo - s.logical), len: hi - lo });
        }
        out
    }

    /// Inserts (`delta > 0`) or deletes (`delta < 0`) logical lines at `from`, changing only the
    /// mapping. Lines below stay on their physical runs; the vacated/removed range becomes a gap
    /// or is dropped. `from` is the first source line for a delete, so the deleted destination
    /// range is `[from - count, from)`.
    fn shift(&mut self, from: u32, delta: i64) {
        if delta == 0 {
            return;
        }
        let limit = self.limit;
        let from = from.min(limit);
        let count = delta.unsigned_abs().min(u64::from(limit)) as u32;
        if count == 0 {
            return;
        }
        self.ensure();
        let mut out: Vec<LineSegment> = Vec::new();
        for s in &self.segments {
            let a = s.logical;
            let b = s.logical_end().min(limit);
            if a >= b {
                continue;
            }
            if delta > 0 {
                if a < from {
                    let hi = b.min(from);
                    if a < hi {
                        out.push(LineSegment { logical: a, physical: s.physical, len: hi - a });
                    }
                }
                if b > from {
                    let start = a.max(from);
                    let new_logical = start.saturating_add(count);
                    if new_logical < limit {
                        let end = b.saturating_add(count).min(limit);
                        if new_logical < end {
                            out.push(LineSegment { logical: new_logical, physical: s.physical.saturating_add(start - a), len: end - new_logical });
                        }
                    }
                }
            } else {
                let delete_start = from.saturating_sub(count);
                let delete_end = from;
                if a < delete_start {
                    let hi = b.min(delete_start);
                    if a < hi {
                        out.push(LineSegment { logical: a, physical: s.physical, len: hi - a });
                    }
                }
                if b > delete_end {
                    let start = a.max(delete_end);
                    let new_logical = start.saturating_sub(count);
                    if start < b {
                        out.push(LineSegment { logical: new_logical, physical: s.physical.saturating_add(start - a), len: b - start });
                    }
                }
            }
        }
        self.segments = out;
        self.normalize();
    }

    /// Merges adjacent runs after a shift. Ids are never renumbered: a run's physical id is what
    /// its entries are stored under.
    fn normalize(&mut self) {
        let mut merged: Vec<LineSegment> = Vec::with_capacity(self.segments.len());
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
    }
}

/// The entries of one chunk of physical lines, sorted by the physical line's offset in the chunk.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LineChunk {
    pub(crate) entries: Vec<(u8, LineInfo)>,
}

/// Row or column metadata, stored per physical line and mapped to logical lines by an
/// axis map. Cloning shares the chunk payloads (`Arc`), so an undo snapshot is O(chunks).
#[derive(Clone)]
pub struct LineMap {
    axis: LineAxis,
    map: LineAxisMap,
    chunks: BTreeMap<u32, Arc<LineChunk>>,
    count: usize,
    /// Where `entry(..).or_default()` writes when a line cannot be mapped (out of range, or the
    /// physical ids are exhausted). Never iterated and never counted.
    sink: LineInfo,
}

impl std::fmt::Debug for LineMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LineMap").field("axis", &self.axis).field("len", &self.count).field("chunks", &self.chunks.len()).finish()
    }
}

/// Logical equality: two maps holding the same lines are the same whatever their physical layout
/// (an undo that re-uses a gap heals to the same document).
impl PartialEq for LineMap {
    fn eq(&self, other: &Self) -> bool {
        self.count == other.count && self.iter().eq(other.iter())
    }
}

impl Eq for LineMap {}

/// The chunk key and in-chunk offset of a physical line.
fn split(physical: u32) -> (u32, u8) {
    (physical / LINE_CHUNK, (physical % LINE_CHUNK) as u8)
}

/// The inclusive logical line range `bounds` covers, clipped to the axis. `lo > hi` when empty.
fn clip_bounds<R: RangeBounds<u32>>(bounds: &R, limit: u32) -> (u32, u32) {
    let last = limit.saturating_sub(1);
    let lo = match bounds.start_bound() {
        Bound::Included(v) => *v,
        Bound::Excluded(v) => v.saturating_add(1),
        Bound::Unbounded => 0,
    };
    let hi = match bounds.end_bound() {
        Bound::Included(v) => Some(*v),
        Bound::Excluded(v) => v.checked_sub(1),
        Bound::Unbounded => Some(last),
    };
    match hi {
        Some(hi) if lo <= last && hi >= lo => (lo, hi.min(last)),
        _ => (1, 0),
    }
}

impl LineMap {
    pub fn new(axis: LineAxis) -> LineMap {
        LineMap { axis, map: LineAxisMap::new(axis.limit()), chunks: BTreeMap::new(), count: 0, sink: LineInfo::default() }
    }

    /// An empty rows map.
    pub fn rows() -> LineMap {
        LineMap::new(LineAxis::Rows)
    }

    /// An empty columns map.
    pub fn cols() -> LineMap {
        LineMap::new(LineAxis::Cols)
    }

    /// The columns map the document format has always called `cols`.
    pub fn from_cols(entries: BTreeMap<u32, LineInfo>) -> LineMap {
        LineMap::from_entries(LineAxis::Cols, entries)
    }

    /// The rows map the document format has always called `rows`.
    pub fn from_rows(entries: BTreeMap<u32, LineInfo>) -> LineMap {
        LineMap::from_entries(LineAxis::Rows, entries)
    }

    /// Reads a logical-keyed map into an identity axis. Lines past the axis (a damaged file) are
    /// dropped, as the sheet cannot address them.
    fn from_entries(axis: LineAxis, entries: BTreeMap<u32, LineInfo>) -> LineMap {
        let mut map = LineMap::new(axis);
        let limit = map.map.limit;
        for (logical, info) in entries {
            if logical < limit {
                map.insert(logical, info);
            }
        }
        map
    }

    pub fn axis(&self) -> LineAxis {
        self.axis
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn get(&self, logical: &u32) -> Option<&LineInfo> {
        let physical = self.map.map(*logical)?;
        let (key, off) = split(physical);
        let chunk = self.chunks.get(&key)?;
        let index = chunk.entries.binary_search_by_key(&off, |&(at, _)| at).ok()?;
        chunk.entries.get(index).map(|(_, info)| info)
    }

    pub fn get_mut(&mut self, logical: &u32) -> Option<&mut LineInfo> {
        let LineMap { chunks, map: axis, .. } = self;
        let physical = axis.map(*logical)?;
        let (key, off) = split(physical);
        let chunk = Arc::make_mut(chunks.get_mut(&key)?);
        let index = chunk.entries.binary_search_by_key(&off, |&(at, _)| at).ok()?;
        chunk.entries.get_mut(index).map(|(_, info)| info)
    }

    /// Sets the metadata of `logical`, returning what was there. A line in an inserted gap gets a
    /// fresh physical id; a line outside the axis (or one the map cannot allocate) keeps nothing.
    pub fn insert(&mut self, logical: u32, info: LineInfo) -> Option<LineInfo> {
        let LineMap { chunks, map: axis, count, .. } = self;
        let physical = axis.map(logical).or_else(|| axis.allocate(logical))?;
        let (key, off) = split(physical);
        let chunk = Arc::make_mut(chunks.entry(key).or_insert_with(|| Arc::new(LineChunk::default())));
        match chunk.entries.binary_search_by_key(&off, |&(at, _)| at) {
            Ok(index) => chunk.entries.get_mut(index).map(|(_, old)| std::mem::replace(old, info)),
            Err(index) => {
                chunk.entries.insert(index, (off, info));
                *count = count.saturating_add(1);
                None
            }
        }
    }

    pub fn remove(&mut self, logical: &u32) -> Option<LineInfo> {
        let LineMap { chunks, map: axis, count, .. } = self;
        let physical = axis.map(*logical)?;
        let (key, off) = split(physical);
        let chunk = Arc::make_mut(chunks.get_mut(&key)?);
        let index = chunk.entries.binary_search_by_key(&off, |&(at, _)| at).ok()?;
        let (_, old) = chunk.entries.remove(index);
        let emptied = chunk.entries.is_empty();
        if emptied {
            chunks.remove(&key);
        }
        *count = count.saturating_sub(1);
        Some(old)
    }

    pub fn entry(&mut self, logical: u32) -> LineEntry<'_> {
        LineEntry { map: self, logical }
    }

    /// Every entry, in logical ascending order.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &LineInfo)> + '_ {
        self.range(..)
    }

    pub fn keys(&self) -> impl Iterator<Item = u32> + '_ {
        self.iter().map(|(line, _)| line)
    }

    pub fn values(&self) -> impl Iterator<Item = &LineInfo> + '_ {
        self.iter().map(|(_, info)| info)
    }

    /// Every entry in physical (chunk) order. Only for callers that touch every line alike.
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut LineInfo> + '_ {
        self.chunks.values_mut().flat_map(|chunk| Arc::make_mut(chunk).entries.iter_mut().map(|(_, info)| info))
    }

    /// The entries whose logical line is in `bounds`, in logical ascending order.
    pub fn range<R: RangeBounds<u32>>(&self, bounds: R) -> impl Iterator<Item = (u32, &LineInfo)> + '_ {
        let (lo, hi) = clip_bounds(&bounds, self.map.limit);
        let segments = if lo <= hi { self.map.segments_in(lo, hi) } else { Vec::new() };
        let chunks = &self.chunks;
        segments.into_iter().flat_map(move |segment| {
            let l0 = segment.logical.max(lo);
            let l1 = segment.logical_end().saturating_sub(1).min(hi);
            let p0 = segment.physical.saturating_add(l0.saturating_sub(segment.logical));
            let p1 = segment.physical.saturating_add(l1.saturating_sub(segment.logical));
            chunks.range(p0 / LINE_CHUNK..=p1 / LINE_CHUNK).flat_map(move |(key, chunk)| {
                let base = key.saturating_mul(LINE_CHUNK);
                chunk.entries.iter().filter_map(move |&(off, ref info)| {
                    let physical = base.saturating_add(u32::from(off));
                    (physical >= p0 && physical <= p1).then(|| (segment.logical.saturating_add(physical.saturating_sub(segment.physical)), info))
                })
            })
        })
    }

    /// `insert_lines(at, count)` for the axis: lines at and below `at` move down, and the inserted
    /// range is a gap with no entries.
    pub fn insert_lines(&mut self, at: u32, count: u32) {
        if count == 0 {
            return;
        }
        self.map.shift(at, i64::from(count));
    }

    /// `delete_lines(at, count)` for the axis: drops the entries of the deleted lines, then moves
    /// the lines below up by `count` (the axis shift happens at `at + count`).
    pub fn delete_lines(&mut self, at: u32, count: u32) {
        if count == 0 || at >= self.map.limit {
            return;
        }
        let count = count.min(self.map.limit - at);
        let end = at.saturating_add(count).saturating_sub(1);
        for segment in self.map.segments_in(at, end) {
            self.remove_physical_run(segment.physical, segment.len);
        }
        self.map.shift(at.saturating_add(count), -i64::from(count));
    }

    /// Drops the entries of `from..from + count` without moving anything.
    pub fn remove_range(&mut self, from: u32, count: u32) {
        if count == 0 || from >= self.map.limit {
            return;
        }
        let count = count.min(self.map.limit - from);
        let end = from.saturating_add(count).saturating_sub(1);
        for segment in self.map.segments_in(from, end) {
            self.remove_physical_run(segment.physical, segment.len);
        }
    }

    /// Writes `info` to every line of `at..at + count`, allocating an unmapped stretch as one run.
    /// Excel's "format same as above": the caller passes the line above's metadata.
    pub fn insert_range(&mut self, at: u32, count: u32, info: &LineInfo) {
        if count == 0 || at >= self.map.limit {
            return;
        }
        let end = at.saturating_add(count.min(self.map.limit - at));
        // Unmapped stretches become one run each instead of a segment per line.
        let mut gaps: Vec<(u32, u32)> = Vec::new();
        let mut cursor = at;
        for segment in self.map.segments_in(at, end.saturating_sub(1)) {
            if segment.logical > cursor {
                gaps.push((cursor, segment.logical - cursor));
            }
            cursor = cursor.max(segment.logical_end());
        }
        if cursor < end {
            gaps.push((cursor, end - cursor));
        }
        for (logical, len) in gaps {
            self.map.allocate_run(logical, len);
        }
        for segment in self.map.segments_in(at, end.saturating_sub(1)) {
            self.fill_physical(segment.physical, segment.len, info);
        }
    }

    /// Bytes this map keeps alive that `newer` does not share: its chunks' payload, charged to the
    /// undo entry the snapshot belongs to.
    pub fn retained_bytes_against(&self, newer: &LineMap) -> usize {
        let entry = std::mem::size_of::<(u8, LineInfo)>();
        let mut bytes = 0usize;
        for (key, chunk) in &self.chunks {
            if newer.chunks.get(key).is_some_and(|next| Arc::ptr_eq(chunk, next)) {
                continue;
            }
            bytes = bytes.saturating_add(std::mem::size_of::<LineChunk>()).saturating_add(chunk.entries.capacity().saturating_mul(entry));
        }
        bytes
    }

    /// Drops every entry whose physical line is in `physical..physical + len`.
    fn remove_physical_run(&mut self, physical: u32, len: u32) {
        if len == 0 {
            return;
        }
        let last = physical.saturating_add(len - 1);
        let LineMap { chunks, count, .. } = self;
        let mut removed = 0usize;
        let mut emptied: Vec<u32> = Vec::new();
        for (key, chunk) in chunks.range_mut(physical / LINE_CHUNK..=last / LINE_CHUNK) {
            let base = key.saturating_mul(LINE_CHUNK);
            let chunk = Arc::make_mut(chunk);
            let before = chunk.entries.len();
            chunk.entries.retain(|&(off, _)| {
                let line = base.saturating_add(u32::from(off));
                line < physical || line > last
            });
            removed = removed.saturating_add(before - chunk.entries.len());
            if chunk.entries.is_empty() {
                emptied.push(*key);
            }
        }
        for key in emptied {
            chunks.remove(&key);
        }
        *count = count.saturating_sub(removed);
    }

    /// Writes `info` to `physical..physical + len`, merging it into the chunk's sorted entries.
    fn fill_physical(&mut self, physical: u32, len: u32, info: &LineInfo) {
        if len == 0 {
            return;
        }
        let last = physical.saturating_add(len - 1);
        let LineMap { chunks, count, .. } = self;
        for key in physical / LINE_CHUNK..=last / LINE_CHUNK {
            let base = key.saturating_mul(LINE_CHUNK);
            let lo = physical.max(base);
            let hi = last.min(base.saturating_add(LINE_CHUNK - 1));
            let (off0, off1) = ((lo - base) as u8, (hi - base) as u8);
            let old_len = chunks.get(&key).map_or(0, |chunk| chunk.entries.len());
            let chunk = Arc::make_mut(chunks.entry(key).or_insert_with(|| Arc::new(LineChunk::default())));
            let mut rest = std::mem::take(&mut chunk.entries).into_iter().peekable();
            let mut merged: Vec<(u8, LineInfo)> = Vec::with_capacity(old_len.saturating_add(usize::from(off1 - off0)).saturating_add(1));
            while let Some(entry) = rest.next_if(|&(off, _)| off < off0) {
                merged.push(entry);
            }
            for off in off0..=off1 {
                merged.push((off, *info));
            }
            for entry in rest {
                if entry.0 > off1 {
                    merged.push(entry);
                }
            }
            let new_len = merged.len();
            chunk.entries = merged;
            if new_len >= old_len {
                *count = count.saturating_add(new_len - old_len);
            } else {
                *count = count.saturating_sub(old_len - new_len);
            }
        }
    }

    #[cfg(test)]
    fn segment_count(&self) -> usize {
        if self.map.identity { 1 } else { self.map.segments.len() }
    }
}

impl<'a> IntoIterator for &'a LineMap {
    type Item = (u32, &'a LineInfo);
    type IntoIter = LineIter<'a>;
    fn into_iter(self) -> LineIter<'a> {
        LineIter(Box::new(self.iter()))
    }
}

/// The iterator `&LineMap` gives: logical ascending entries.
pub struct LineIter<'a>(Box<dyn Iterator<Item = (u32, &'a LineInfo)> + 'a>);

impl<'a> Iterator for LineIter<'a> {
    type Item = (u32, &'a LineInfo);
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}

/// The pending entry for one line, as `BTreeMap::entry` gives.
pub struct LineEntry<'a> {
    map: &'a mut LineMap,
    logical: u32,
}

impl<'a> LineEntry<'a> {
    /// The line's metadata, inserting a default one if it has none.
    pub fn or_default(self) -> &'a mut LineInfo {
        let LineEntry { map, logical } = self;
        let LineMap { chunks, map: axis, count, sink, .. } = map;
        let Some(physical) = axis.map(logical).or_else(|| axis.allocate(logical)) else {
            return sink;
        };
        let (key, off) = split(physical);
        let chunk = Arc::make_mut(chunks.entry(key).or_insert_with(|| Arc::new(LineChunk::default())));
        let index = match chunk.entries.binary_search_by_key(&off, |&(at, _)| at) {
            Ok(index) => index,
            Err(index) => {
                chunk.entries.insert(index, (off, LineInfo::default()));
                *count = count.saturating_add(1);
                index
            }
        };
        match chunk.entries.get_mut(index) {
            Some((_, info)) => info,
            // Unreachable: the entry was just found or inserted.
            None => sink,
        }
    }
}

impl Serialize for LineMap {
    /// The document format's shape: a map keyed by *logical* line, exactly as the
    /// `BTreeMap<u32, LineInfo>` it replaces wrote it.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.count))?;
        for (line, info) in self.iter() {
            map.serialize_entry(&line, info)?;
        }
        map.end()
    }
}

/// Serde entry point for `Sheet::cols`: logical keys, as a `BTreeMap` wrote them.
pub(crate) fn deserialize_cols<'de, D: Deserializer<'de>>(deserializer: D) -> Result<LineMap, D::Error> {
    Ok(LineMap::from_cols(BTreeMap::<u32, LineInfo>::deserialize(deserializer)?))
}

/// Serde entry point for `Sheet::rows`: logical keys, as a `BTreeMap` wrote them.
pub(crate) fn deserialize_rows<'de, D: Deserializer<'de>>(deserializer: D) -> Result<LineMap, D::Error> {
    Ok(LineMap::from_rows(BTreeMap::<u32, LineInfo>::deserialize(deserializer)?))
}

pub(crate) fn default_cols() -> LineMap {
    LineMap::cols()
}

pub(crate) fn default_rows() -> LineMap {
    LineMap::rows()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic xorshift32 so a failure reproduces without a random-number dependency.
    struct Rng(u32);

    impl Rng {
        fn next(&mut self) -> u32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            self.0
        }

        fn below(&mut self, n: u32) -> u32 {
            self.next() % n.max(1)
        }
    }

    fn info(seed: u32) -> LineInfo {
        LineInfo {
            size: Some(10.0 + (seed % 91) as f32),
            hidden: seed.is_multiple_of(5),
            outline: (seed % 8) as u8,
            collapsed: seed.is_multiple_of(7),
            style: None,
            custom: seed.is_multiple_of(3),
        }
    }

    fn entries(map: &LineMap) -> Vec<(u32, LineInfo)> {
        map.iter().map(|(line, info)| (line, *info)).collect()
    }

    /// The `BTreeMap` the old model used, answering the same structural operations.
    fn shift(map: &mut BTreeMap<u32, LineInfo>, at: u32, count: u32) {
        let old = std::mem::take(map);
        for (line, info) in old {
            if line >= at {
                if let Some(moved) = line.checked_add(count).filter(|line| *line < MAX_ROWS) {
                    map.insert(moved, info);
                }
            } else {
                map.insert(line, info);
            }
        }
    }

    fn drop_and_shift(map: &mut BTreeMap<u32, LineInfo>, at: u32, count: u32) {
        let old = std::mem::take(map);
        for (line, info) in old {
            if line >= at && line < at.saturating_add(count) {
                continue;
            }
            let to = if line >= at.saturating_add(count) { line - count } else { line };
            map.insert(to, info);
        }
    }

    #[test]
    fn matches_a_btreemap_through_structural_edits() {
        let mut rng = Rng(0x5eed_1a7e);
        let mut map = LineMap::new(LineAxis::Rows);
        let mut reference: BTreeMap<u32, LineInfo> = BTreeMap::new();
        let mut seed = 0u32;
        for step in 0..600 {
            let line = rng.below(300);
            match rng.below(10) {
                0..=2 => {
                    seed += 1;
                    let value = info(seed);
                    map.insert(line, value);
                    reference.insert(line, value);
                }
                3 => {
                    let got = map.remove(&line);
                    let want = reference.remove(&line);
                    assert_eq!(got, want, "step {step}: remove {line}");
                }
                4 => {
                    seed += 1;
                    let value = info(seed);
                    map.entry(line).or_default().size = value.size;
                    reference.entry(line).or_default().size = value.size;
                }
                5 => {
                    let count = 1 + rng.below(4);
                    map.insert_lines(line, count);
                    shift(&mut reference, line, count);
                }
                6 => {
                    let count = 1 + rng.below(4);
                    map.delete_lines(line, count);
                    drop_and_shift(&mut reference, line, count);
                }
                7 => {
                    let count = 1 + rng.below(5);
                    map.remove_range(line, count);
                    for at in line..line.saturating_add(count) {
                        reference.remove(&at);
                    }
                }
                8 => {
                    let count = 1 + rng.below(4);
                    seed += 1;
                    let value = info(seed);
                    map.insert_range(line, count, &value);
                    for at in line..line.saturating_add(count).min(MAX_ROWS) {
                        reference.insert(at, value);
                    }
                }
                _ => {
                    let line = rng.below(400);
                    assert_eq!(map.get(&line), reference.get(&line), "step {step}: get {line}");
                }
            }
            assert_eq!(entries(&map), reference.iter().map(|(line, info)| (*line, *info)).collect::<Vec<_>>(), "step {step}");
            assert_eq!(map.len(), reference.len(), "step {step}");
            assert_eq!(map.is_empty(), reference.is_empty(), "step {step}");
            for sample in [line, line.saturating_add(1), rng.below(320)] {
                assert_eq!(map.get(&sample), reference.get(&sample), "step {step}: sample {sample}");
            }
        }
    }

    #[test]
    fn insert_lines_moves_metadata_below_and_leaves_the_gap_empty() {
        let mut map = LineMap::new(LineAxis::Rows);
        map.insert(1, info(1));
        map.insert(2, info(2));
        map.insert(3, info(3));
        map.insert_lines(2, 3);
        // Line 1 stays, the inserted 2..5 are empty, and 2..3 moved to 5..6.
        assert_eq!(map.keys().collect::<Vec<_>>(), [1, 5, 6]);
        assert_eq!(map.get(&2), None);
        assert_eq!(map.get(&4), None);
        assert_eq!(map.get(&5), Some(&info(2)));
        assert_eq!(map.get(&6), Some(&info(3)));
        assert_eq!(map.len(), 3);
        // Logical order is what iteration promises, whatever the physical layout.
        assert_eq!(map.keys().collect::<Vec<_>>(), [1, 5, 6]);
        // The inserted gap takes writes.
        map.insert(3, info(9));
        assert_eq!(map.get(&3), Some(&info(9)));
        assert_eq!(map.keys().collect::<Vec<_>>(), [1, 3, 5, 6]);
    }

    #[test]
    fn delete_lines_drops_its_entries_and_shifts_the_rest_up() {
        let mut map = LineMap::new(LineAxis::Rows);
        for line in 0..6 {
            map.insert(line, info(line));
        }
        // `delete_lines` takes the deleted range's first line; the axis shifts at `at + count`.
        map.delete_lines(2, 2);
        assert_eq!(map.keys().collect::<Vec<_>>(), [0, 1, 2, 3]);
        assert_eq!(map.get(&2), Some(&info(4)));
        assert_eq!(map.get(&3), Some(&info(5)));
        assert_eq!(map.get(&4), None);
        assert_eq!(map.len(), 4);
        // A delete of the first lines leaves nothing above them.
        map.delete_lines(0, 1);
        assert_eq!(map.keys().collect::<Vec<_>>(), [0, 1, 2]);
        assert_eq!(map.get(&0), Some(&info(1)));
        assert_eq!(map.get(&1), Some(&info(4)));
        assert_eq!(map.get(&3), None);
    }

    #[test]
    fn a_bulk_insert_range_allocates_one_run() {
        let mut map = LineMap::new(LineAxis::Rows);
        map.insert(5, info(5));
        map.insert_lines(10, 5000);
        let before = map.segment_count();
        map.insert_range(10, 5000, &info(7));
        assert_eq!(map.len(), 5001);
        assert!(map.segment_count() <= before + 1, "one run for the whole gap, got {}", map.segment_count());
        assert!(map.get(&10) == Some(&info(7)) && map.get(&5009) == Some(&info(7)));
        assert_eq!(map.get(&5), Some(&info(5)));
        assert_eq!(map.get(&(10 + 5000)), None);
        assert_eq!(map.keys().last(), Some(5009));
        // Ascending writes into many chunks stay one run, one entry per line.
        let mut wide = LineMap::new(LineAxis::Cols);
        wide.insert_range(0, LINE_CHUNK * 4, &info(3));
        assert_eq!(wide.len() as u32, LINE_CHUNK * 4);
        assert_eq!(wide.segment_count(), 1);
    }

    #[test]
    fn clone_shares_chunks_until_the_copy_is_written() {
        let mut map = LineMap::new(LineAxis::Rows);
        for line in (0..400).step_by(3) {
            map.insert(line, info(line));
        }
        let snapshot = map.clone();
        assert_eq!(snapshot, map);
        assert_eq!(snapshot.retained_bytes_against(&map), 0, "an unedited clone shares every chunk");
        // Mutating the copy must not touch the snapshot, and must copy only the touched chunk.
        map.insert(301, info(301));
        assert_ne!(snapshot.get(&301), Some(&info(301)));
        assert_eq!(snapshot.len() + 1, map.len());
        let charged = snapshot.retained_bytes_against(&map);
        let chunk = LINE_CHUNK as usize * std::mem::size_of::<(u8, LineInfo)>();
        assert!(charged > 0 && charged <= 2 * chunk, "one chunk, not the map: {charged}");
        // A write to a line the snapshot does not hold leaves it alone entirely.
        map.insert(302, info(302));
        assert_eq!(snapshot.get(&302), None);
        assert_eq!(entries(&snapshot), {
            let mut old: Vec<(u32, LineInfo)> = (0..400).step_by(3).map(|line| (line, info(line))).collect();
            old.retain(|(line, _)| *line != 301);
            old
        });
    }

    #[test]
    fn serde_keeps_logical_keys_only_and_round_trips() {
        let mut sheet = crate::sheet::Sheet::new("S");
        for line in 0..5 {
            sheet.rows.insert(line, info(line));
        }
        sheet.cols.insert(1, info(11));
        sheet.rows.insert_lines(2, 3);
        sheet.rows.remove(&6);
        let json = serde_json::to_value(&sheet).unwrap();
        let rows = json.get("rows").and_then(|v| v.as_object()).unwrap();
        assert_eq!(rows.keys().cloned().collect::<Vec<_>>(), ["0", "1", "5", "7"]);
        assert_eq!(json.get("cols").and_then(|v| v.as_object()).unwrap().keys().cloned().collect::<Vec<_>>(), ["1"]);
        // No map state (`chunks`, `segments`, physical ids) leaks into the document format.
        assert!(rows.values().all(|v| v.get("size").is_some()));
        let back: crate::sheet::Sheet = serde_json::from_value(json).unwrap();
        assert_eq!(back.rows, sheet.rows);
        assert_eq!(back.cols, sheet.cols);
        assert!(back.rows.iter().eq(sheet.rows.iter()));
        // Writing into the gap after a round trip allocates a line of its own.
        let mut back = back;
        back.rows.insert(3, info(9));
        assert_eq!(back.rows.get(&3), Some(&info(9)));
        assert_eq!(back.rows.len(), 5);
        // A document from before the map existed (no rows/cols) still opens.
        let mut json = serde_json::to_value(&sheet).unwrap();
        let object = json.as_object_mut().unwrap();
        object.remove("rows");
        object.remove("cols");
        let sheet: crate::sheet::Sheet = serde_json::from_value(json).unwrap();
        assert!(sheet.rows.is_empty() && sheet.cols.is_empty());
    }

    #[test]
    fn from_rows_drops_lines_past_the_axis() {
        let mut raw = BTreeMap::new();
        raw.insert(0, info(0));
        raw.insert(MAX_ROWS - 1, info(1));
        raw.insert(MAX_ROWS, info(2));
        raw.insert(u32::MAX, info(3));
        let map = LineMap::from_rows(raw);
        assert_eq!(map.keys().collect::<Vec<_>>(), [0, MAX_ROWS - 1]);
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn entry_outside_the_axis_writes_to_the_sink() {
        let mut map = LineMap::new(LineAxis::Cols);
        map.entry(MAX_COLS).or_default().hidden = true;
        assert!(map.is_empty());
        map.entry(7).or_default().hidden = true;
        assert_eq!(map.len(), 1);
        assert!(map.get(&7).is_some_and(|info| info.hidden));
        assert!(!map.get(&MAX_COLS).is_some_and(|info| info.hidden));
    }

    #[test]
    fn remove_range_does_not_move_lines() {
        let mut map = LineMap::new(LineAxis::Cols);
        for line in 0..10 {
            map.insert(line, info(line));
        }
        map.remove_range(3, 4);
        assert_eq!(map.keys().collect::<Vec<_>>(), [0, 1, 2, 7, 8, 9]);
        assert_eq!(map.get(&7), Some(&info(7)));
    }
}
