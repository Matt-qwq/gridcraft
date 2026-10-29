//! Slicers and timelines bound to a PivotTable's cache: on-sheet filter widgets whose tiles show
//! one pivot cache field, with the selected tiles stored per item.
//!
//! This is the *PivotTable* slicer. [`crate::features::Slicer`] is a different, older widget bound
//! to a table column; it predates file support and is kept apart so the UI that drives it does not
//! have to change. A slicer here owns everything an `.xlsx` part needs: its own name, the name and
//! field of the slicer cache it reads values from, the workbook pivot cache (and the PivotTables)
//! it filters, its style, layout, item list and on-sheet anchor. A timeline is the same shape for
//! a date field, with the time unit it groups by.
//!
//! Values, not labels, identify an item: a file stores each tile as an index into the pivot
//! cache's shared items, so the identity has to survive a save even when the source's order
//! changes ([`SlicerValue::key`]). [`SlicerValue::label`] is the display text, computed the same
//! way the file's shared items are rendered — never the cell's number format, which the pivot
//! cache does not carry.
//!
//! Clean room: the style *names* are file-format identifiers (Excel writes them and matches them
//! to its own built-in looks); the looks themselves are drawn by GridCraft's own code, never
//! copied from a theme part.

use gridcraft_core::{Value, number_to_text};
use serde::{Deserialize, Serialize};

use crate::features::{Anchor, AnchorMode};

/// The style a new slicer gets when the caller names none. A file-format identifier: the drawing
/// of the look lives in the UI, not here.
pub const DEFAULT_SLICER_STYLE: &str = "SlicerStyleLight1";

/// The slicer styles a command accepts. These are the names the format defines; how each looks is
/// GridCraft's own drawing code (the UI crate), not a copy of Excel's theme XML.
pub const SLICER_STYLES: &[&str] = &[
    "SlicerStyleLight1",
    "SlicerStyleLight2",
    "SlicerStyleLight3",
    "SlicerStyleLight4",
    "SlicerStyleLight5",
    "SlicerStyleLight6",
    "SlicerStyleOther1",
    "SlicerStyleOther2",
    "SlicerStyleDark1",
    "SlicerStyleDark2",
    "SlicerStyleDark3",
    "SlicerStyleDark4",
    "SlicerStyleDark5",
    "SlicerStyleDark6",
];

/// One value of a slicer's cache field. The variant is what the file's shared item is (`<s>`,
/// `<n>`, `<b>`, `<e>`, or none for a blank), so a save can write the same kind back and a source
/// value added later can be matched without its display text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SlicerValue {
    Blank,
    Number(f64),
    Text(String),
    Bool(bool),
    Error(String),
}

impl SlicerValue {
    /// The value of a cell, for the distinct item list of a cache field.
    pub fn from_value(v: &Value) -> SlicerValue {
        match v.scalar() {
            Value::Empty => SlicerValue::Blank,
            Value::Number(n) => SlicerValue::Number(n),
            Value::Text(t) => SlicerValue::Text(t.to_string()),
            Value::Bool(b) => SlicerValue::Bool(b),
            Value::Error(e) => SlicerValue::Error(e.as_str().to_string()),
            Value::Array(_) => SlicerValue::Blank,
        }
    }

    /// Display text of a tile. Matches how the pivot cache's shared items are rendered on read
    /// (`(blank)`, `TRUE`/`FALSE`, the error's text), so a slicer reads back the same tile.
    pub fn label(&self) -> String {
        match self {
            SlicerValue::Blank => "(blank)".into(),
            SlicerValue::Number(n) => number_to_text(*n),
            SlicerValue::Text(t) => t.clone(),
            SlicerValue::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
            SlicerValue::Error(e) => e.clone(),
        }
    }

    /// Identity of a value across a save. Text compares case-insensitively (the pivot cache does),
    /// a number by its bits (so a negative zero is not a zero and a NaN is itself), an error by
    /// its lower-cased text.
    pub fn key(&self) -> String {
        match self {
            SlicerValue::Blank => "b".into(),
            SlicerValue::Number(n) => format!("n{:016x}", n.to_bits()),
            SlicerValue::Text(t) => format!("t{}", t.to_lowercase()),
            SlicerValue::Bool(b) => format!("l{b}"),
            SlicerValue::Error(e) => format!("e{}", e.to_lowercase()),
        }
    }
}

/// One tile: a value and whether it is selected. An item no one deselected is selected, as in
/// Excel ("Select All" is the default).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SlicerItem {
    pub value: SlicerValue,
    pub selected: bool,
}

impl SlicerItem {
    pub fn new(value: SlicerValue, selected: bool) -> SlicerItem {
        SlicerItem { value, selected }
    }
    pub fn label(&self) -> String {
        self.value.label()
    }
}

/// How a slicer's tiles are ordered for display. A UI hint: the item list keeps the cache's order,
/// and the file's slicer part does not carry this setting, so it is not written back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlicerSort {
    #[default]
    Ascending,
    Descending,
    /// The order of the source data, as Excel's "Data source order".
    Source,
}

impl SlicerSort {
    pub fn as_str(self) -> &'static str {
        match self {
            SlicerSort::Ascending => "ascending",
            SlicerSort::Descending => "descending",
            SlicerSort::Source => "source",
        }
    }
    pub fn parse(s: &str) -> Option<SlicerSort> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ascending" | "asc" | "a-z" => Some(SlicerSort::Ascending),
            "descending" | "desc" | "z-a" => Some(SlicerSort::Descending),
            "source" | "data" => Some(SlicerSort::Source),
            _ => None,
        }
    }
}

/// The time unit a timeline groups a date field by (its `level` in the file).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeUnit {
    Years,
    Quarters,
    #[default]
    Months,
    Days,
}

impl TimeUnit {
    pub fn as_str(self) -> &'static str {
        match self {
            TimeUnit::Years => "years",
            TimeUnit::Quarters => "quarters",
            TimeUnit::Months => "months",
            TimeUnit::Days => "days",
        }
    }
    pub fn parse(s: &str) -> Option<TimeUnit> {
        match s.trim().to_ascii_lowercase().as_str() {
            "years" | "year" => Some(TimeUnit::Years),
            "quarters" | "quarter" => Some(TimeUnit::Quarters),
            "months" | "month" => Some(TimeUnit::Months),
            "days" | "day" => Some(TimeUnit::Days),
            _ => None,
        }
    }
    /// The timeline part's `level` attribute (0 years … 3 days).
    pub fn level(self) -> u32 {
        match self {
            TimeUnit::Years => 0,
            TimeUnit::Quarters => 1,
            TimeUnit::Months => 2,
            TimeUnit::Days => 3,
        }
    }
    pub fn from_level(level: u32) -> Option<TimeUnit> {
        match level {
            0 => Some(TimeUnit::Years),
            1 => Some(TimeUnit::Quarters),
            2 => Some(TimeUnit::Months),
            3 => Some(TimeUnit::Days),
            _ => None,
        }
    }
}

/// A slicer bound to a PivotTable's cache: the value tiles of one cache field.
///
/// `cache` is the slicer cache's name (the `cache` attribute of the slicer part and the `name` of
/// the cache definition). Two slicers of the same field share it in Excel; GridCraft writes one
/// cache per distinct name. `pivot_cache_id` is the workbook pivot cache the cache reads shared
/// items from; on a save it is recomputed from the first PivotTable in `pivot_tables`, and a
/// slicer whose PivotTables are all gone is not written (it goes with them, like the kept path).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PivotSlicer {
    pub id: u32,
    /// Name of the slicer (the slicer part's `name`, unique in the workbook).
    pub name: String,
    /// Name of the slicer cache the tiles come from.
    pub cache: String,
    pub caption: String,
    /// Source field (a pivot cache field's name) whose values the tiles show.
    pub field: String,
    /// The workbook pivot cache (`<pivotCache cacheId>`) the slicer cache is built on.
    pub pivot_cache_id: u32,
    /// The PivotTables the slicer filters, by name.
    pub pivot_tables: Vec<String>,
    /// Tiles in cache order, each with its selected flag.
    pub items: Vec<SlicerItem>,
    pub style: String,
    /// Tiles per row in the layout (Excel default 1).
    pub columns: u32,
    pub sort: SlicerSort,
    /// Where it sits on its sheet.
    pub anchor: Anchor,
}

impl Default for PivotSlicer {
    fn default() -> Self {
        PivotSlicer {
            id: 0,
            name: String::new(),
            cache: String::new(),
            caption: String::new(),
            field: String::new(),
            pivot_cache_id: 0,
            pivot_tables: vec![],
            items: vec![],
            style: DEFAULT_SLICER_STYLE.into(),
            columns: 1,
            sort: SlicerSort::Ascending,
            anchor: Anchor { cell: gridcraft_core::CellRef::default(), dx: 4.0, dy: 4.0, width: 168.0, height: 200.0, mode: AnchorMode::MoveOnly },
        }
    }
}

impl PivotSlicer {
    /// Whether every tile is selected (no filter from this slicer).
    pub fn all_selected(&self) -> bool {
        self.items.iter().all(|i| i.selected)
    }

    /// The selected tiles' labels, or `None` when every tile is selected. An empty list means
    /// every tile was deselected, which hides every row (as in Excel).
    pub fn selected_labels(&self) -> Option<Vec<String>> {
        if self.all_selected() { None } else { Some(self.items.iter().filter(|i| i.selected).map(SlicerItem::label).collect()) }
    }

    /// Selects the tiles named by `labels` (compared case-insensitively); `None` selects every
    /// tile. Returns the labels no tile matched, so a caller can reject a typo instead of
    /// silently hiding every row.
    pub fn select(&mut self, labels: Option<&[String]>) -> Vec<String> {
        let Some(labels) = labels else {
            for item in &mut self.items {
                item.selected = true;
            }
            return vec![];
        };
        let mut unmatched = vec![];
        for label in labels {
            if !self.items.iter().any(|i| i.label().eq_ignore_ascii_case(label.trim())) {
                unmatched.push(label.clone());
            }
        }
        let wanted: Vec<&String> = labels.iter().collect();
        for item in &mut self.items {
            item.selected = wanted.iter().any(|l| item.label().eq_ignore_ascii_case(l.trim()));
        }
        unmatched
    }
}

/// A timeline's selected date range: the first and last day it shows, as serial days in the
/// workbook's date system. The file keeps it as the timeline cache's `<state><selection>`
/// `startDate`/`endDate`; a range is inclusive at both ends.
///
/// Serial numbers, not dates, because that is how the sheet stores a date and how the source rows
/// are compared. A range is only written when it is finite and ordered ([`Self::is_valid`]); a
/// hostile one is treated as no selection rather than written as a number a reader would choke on.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimelineRange {
    pub start: f64,
    pub end: f64,
}

impl TimelineRange {
    pub fn new(start: f64, end: f64) -> TimelineRange {
        TimelineRange { start, end }
    }

    /// Whether this is a range a date axis can show: two finite serials with the start not after
    /// the end. A NaN compares false everywhere, so a hostile range is never valid.
    pub fn is_valid(&self) -> bool {
        self.start.is_finite() && self.end.is_finite() && self.start <= self.end
    }

    /// Whether `serial` is inside the range (both ends included). False for NaN and for a range
    /// that is not valid.
    pub fn contains(&self, serial: f64) -> bool {
        self.is_valid() && serial.is_finite() && serial >= self.start && serial <= self.end
    }
}

/// A timeline: the same widget for a date field, with the time unit it groups by.
///
/// A timeline's selection is a date *range* ([`TimelineRange`], the cache's
/// `<state><selection>`), `None` when every date is shown. `items` stays empty; the tiles are the
/// periods of `field` grouped by `unit`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PivotTimeline {
    pub id: u32,
    pub name: String,
    pub cache: String,
    pub caption: String,
    pub field: String,
    pub pivot_cache_id: u32,
    pub pivot_tables: Vec<String>,
    pub style: String,
    pub unit: TimeUnit,
    /// The dates the timeline shows; `None` = all dates.
    pub range: Option<TimelineRange>,
    pub anchor: Anchor,
}

impl Default for PivotTimeline {
    fn default() -> Self {
        PivotTimeline {
            id: 0,
            name: String::new(),
            cache: String::new(),
            caption: String::new(),
            field: String::new(),
            pivot_cache_id: 0,
            pivot_tables: vec![],
            // Empty = Excel's default look: the file carries no style and the UI picks its own.
            style: String::new(),
            unit: TimeUnit::Months,
            range: None,
            anchor: Anchor { cell: gridcraft_core::CellRef::default(), dx: 4.0, dy: 4.0, width: 240.0, height: 96.0, mode: AnchorMode::MoveOnly },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(values: &[&str], selected: &[bool]) -> Vec<SlicerItem> {
        values.iter().zip(selected).map(|(v, s)| SlicerItem::new(SlicerValue::Text((*v).into()), *s)).collect()
    }

    #[test]
    fn selecting_labels_returns_the_unmatched_and_derives_the_filter() {
        let mut sl =
            PivotSlicer { items: items(&["East", "West", "10", "TRUE", "(blank)"], &[true, false, false, false, false]), ..PivotSlicer::default() };
        // One tile of five is selected: the filter is that tile.
        assert_eq!(sl.selected_labels(), Some(vec!["East".into()]));
        // A label is matched case-insensitively; the rest are reported back.
        assert_eq!(sl.select(Some(&["east".into(), "Nowhere".into()])), vec!["Nowhere".to_string()]);
        assert_eq!(sl.selected_labels(), Some(vec!["East".into()]));
        assert!(!sl.all_selected());
        // Selecting nothing hides every row (an empty filter, not "no filter").
        assert!(sl.select(Some(&[])).is_empty());
        assert_eq!(sl.selected_labels(), Some(vec![]));
        // `None` selects every tile, which is the same as no filter at all.
        assert!(sl.select(None).is_empty());
        assert!(sl.all_selected());
        assert_eq!(sl.selected_labels(), None);
    }

    #[test]
    fn values_compare_by_kind_and_text_ignores_case() {
        assert_eq!(SlicerValue::Number(1.5).key(), SlicerValue::Number(1.5).key());
        assert_ne!(SlicerValue::Number(0.0).key(), SlicerValue::Number(-0.0).key());
        assert_ne!(SlicerValue::Number(1.0).key(), SlicerValue::Text("1".into()).key());
        assert_eq!(SlicerValue::Text("East".into()).key(), SlicerValue::Text("EAST".into()).key());
        assert_eq!(SlicerValue::Blank.label(), "(blank)");
        assert_eq!(SlicerValue::Bool(true).label(), "TRUE");
        assert_eq!(SlicerValue::Number(12.5).label(), "12.5");
    }

    #[test]
    fn the_model_survives_json() {
        let sl = PivotSlicer {
            id: 3,
            name: "Region".into(),
            cache: "Slicer_Region".into(),
            field: "Region".into(),
            pivot_cache_id: 1,
            pivot_tables: vec!["PivotTable1".into()],
            items: items(&["East", "(blank)"], &[true, false]),
            columns: 2,
            ..PivotSlicer::default()
        };
        let json = serde_json::to_string(&sl).unwrap();
        assert_eq!(serde_json::from_str::<PivotSlicer>(&json).unwrap(), sl);
        let tl = PivotTimeline { unit: TimeUnit::Quarters, range: Some(TimelineRange::new(45_658.0, 45_720.0)), ..PivotTimeline::default() };
        assert_eq!(serde_json::from_str::<PivotTimeline>(&serde_json::to_string(&tl).unwrap()).unwrap(), tl);
        assert_eq!(TimeUnit::from_level(TimeUnit::Days.level()), Some(TimeUnit::Days));
        assert_eq!(TimeUnit::from_level(9), None);
        assert_eq!(SlicerSort::parse("Z-A"), Some(SlicerSort::Descending));
        assert_eq!(SlicerSort::parse("sideways"), None);
    }

    #[test]
    fn a_hostile_date_range_is_never_valid_and_contains_nothing() {
        let r = TimelineRange::new(45_658.0, 45_720.0);
        assert!(r.is_valid() && r.contains(45_658.0) && r.contains(45_720.0) && !r.contains(45_657.0));
        // A single day is a range (the two handles meet).
        assert!(TimelineRange::new(45_658.0, 45_658.0).contains(45_658.0));
        // Reversed, NaN, infinite: nothing is shown, and the writer will drop the selection.
        for bad in [TimelineRange::new(45_720.0, 45_658.0), TimelineRange::new(f64::NAN, 1.0), TimelineRange::new(0.0, f64::INFINITY)] {
            assert!(!bad.is_valid(), "{bad:?}");
            assert!(!bad.contains(45_658.0), "{bad:?}");
        }
    }
}
