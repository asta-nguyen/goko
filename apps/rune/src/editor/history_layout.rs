//! Persisted History pane sizes — a port of the desktop `history-layout.ts`
//! persistence semantics as pure logic. The desktop hook reads/writes
//! `localStorage`; natively the same JSON document lives in the shared
//! settings store under [`HISTORY_LAYOUT_KEY`]. Parsing never fails: corrupt
//! or out-of-range values fall back per field, exactly like `readLayout`.

use serde_json::{Map, Value};

/// Settings-store key holding the serialized [`HistoryLayout`] JSON.
pub const HISTORY_LAYOUT_KEY: &str = "goko.history.layout";

/// Pixel bounds for one field; `max: None` is unbounded above
/// (`detailHeight` has no upper limit, matching `LAYOUT_LIMITS`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutLimits {
    pub min: f64,
    pub max: Option<f64>,
}

/// A resizable History column. `detailHeight` is not a column — it has its
/// own setter, like `setDetailHeight` on the desktop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryColumn {
    Graph,
    Date,
    Author,
    Commit,
}

/// Any persisted layout field; the set `reset` accepts, mirroring
/// `keyof HistoryLayout` on the desktop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryField {
    Graph,
    Date,
    Author,
    Commit,
    DetailHeight,
}

/// One of the editor's horizontal list/detail splits that persists a width.
/// These are NOT history columns — they live in the same settings document
/// (`panes` object) so the existing GetSetting/SetSetting lane carries them
/// without a second key or a parallel persistence path.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum PaneKind {
    /// Files section: file tree / content-search results aside.
    FilesAside,
    /// Changes section: changed-file list aside.
    ChangesList,
    /// Stashes section: stash list aside.
    StashesList,
}

impl PaneKind {
    /// JSON key inside the `panes` object (camelCase, like `detailHeight`).
    pub fn key(self) -> &'static str {
        match self {
            PaneKind::FilesAside => "filesAside",
            PaneKind::ChangesList => "changesList",
            PaneKind::StashesList => "stashesList",
        }
    }

    /// Pixel bounds shared by every list aside — the desktop `editorList`
    /// `PANEL_LIMITS` (default 288, min 180, max 480); per-pane defaults
    /// keep each section's established width.
    pub fn limits(self) -> LayoutLimits {
        LayoutLimits {
            min: crate::editor::pane_layout::PANE_MIN as f64,
            max: Some(crate::editor::pane_layout::PANE_MAX as f64),
        }
    }

    /// This pane's width in [`DEFAULT_LAYOUT`].
    pub fn default_width(self) -> f64 {
        match self {
            PaneKind::FilesAside => crate::editor::pane_layout::FILES_ASIDE_DEFAULT,
            PaneKind::ChangesList => crate::editor::pane_layout::CHANGES_LIST_DEFAULT,
            PaneKind::StashesList => crate::editor::pane_layout::STASHES_LIST_DEFAULT,
        }
    }
}

/// Persisted widths (px) of the editor's horizontal list/detail splits.
/// Stored under `panes` in the layout document; each field parses
/// independently like the history fields.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneWidths {
    pub files_aside: f64,
    pub changes_list: f64,
    pub stashes_list: f64,
}

impl Default for PaneWidths {
    fn default() -> PaneWidths {
        PaneWidths {
            files_aside: PaneKind::FilesAside.default_width(),
            changes_list: PaneKind::ChangesList.default_width(),
            stashes_list: PaneKind::StashesList.default_width(),
        }
    }
}

impl PaneWidths {
    /// Width for `kind`, already inside its static bounds.
    pub fn get(&self, kind: PaneKind) -> f64 {
        match kind {
            PaneKind::FilesAside => self.files_aside,
            PaneKind::ChangesList => self.changes_list,
            PaneKind::StashesList => self.stashes_list,
        }
    }

    /// Bounds `px` and assigns it to `kind`; non-finite values are ignored.
    pub fn set(&mut self, kind: PaneKind, px: f64) {
        let Some(value) = bounded_number(px, kind.limits()) else {
            return;
        };
        match kind {
            PaneKind::FilesAside => self.files_aside = value,
            PaneKind::ChangesList => self.changes_list = value,
            PaneKind::StashesList => self.stashes_list = value,
        }
    }

    /// Restores `kind` to its per-section default (grip double-press).
    pub fn reset(&mut self, kind: PaneKind) {
        self.set(kind, kind.default_width());
    }
}

impl From<HistoryColumn> for HistoryField {
    fn from(column: HistoryColumn) -> HistoryField {
        match column {
            HistoryColumn::Graph => HistoryField::Graph,
            HistoryColumn::Date => HistoryField::Date,
            HistoryColumn::Author => HistoryField::Author,
            HistoryColumn::Commit => HistoryField::Commit,
        }
    }
}

/// Persisted History pane sizes in px; `graph` unset means the natural lane
/// width. Field order in [`HistoryLayout::to_json`] matches the desktop
/// document (`detailHeight` stays camelCase for store compatibility).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HistoryLayout {
    pub graph: Option<f64>,
    pub date: f64,
    pub author: f64,
    pub commit: f64,
    pub detail_height: f64,
    /// Horizontal list/detail split widths for the Files/Changes/Stashes
    /// sections — serialized under `panes`, parsed per field like the rest.
    pub panes: PaneWidths,
}

/// `DEFAULT_LAYOUT` from the desktop module, plus the native pane defaults.
pub const DEFAULT_LAYOUT: HistoryLayout = HistoryLayout {
    graph: None,
    date: 120.0,
    author: 128.0,
    commit: 80.0,
    detail_height: 320.0,
    panes: PaneWidths {
        files_aside: crate::editor::pane_layout::FILES_ASIDE_DEFAULT,
        changes_list: crate::editor::pane_layout::CHANGES_LIST_DEFAULT,
        stashes_list: crate::editor::pane_layout::STASHES_LIST_DEFAULT,
    },
};

impl Default for HistoryLayout {
    fn default() -> HistoryLayout {
        DEFAULT_LAYOUT
    }
}

impl HistoryColumn {
    /// The persisted field this column resizes.
    pub fn field(self) -> HistoryField {
        HistoryField::from(self)
    }

    /// Pixel bounds applied by [`HistoryLayout::set_column`].
    pub fn limits(self) -> LayoutLimits {
        self.field().limits()
    }
}

impl HistoryField {
    /// Pixel bounds for this field (`LAYOUT_LIMITS` on the desktop).
    pub fn limits(self) -> LayoutLimits {
        match self {
            HistoryField::Graph => LayoutLimits {
                min: 48.0,
                max: Some(320.0),
            },
            HistoryField::Date | HistoryField::Author | HistoryField::Commit => LayoutLimits {
                min: 56.0,
                max: Some(360.0),
            },
            // Native floor is the patch minimum, not the desktop's 160: the
            // renderer yields below 160 on windows too short to fit it (the
            // 760×480 minimum reaches ~157), and those viewport-valid values
            // must round-trip through the store. The comfortable 160 minimum
            // is still enforced by the drag on viewports that can fit it.
            HistoryField::DetailHeight => LayoutLimits {
                min: f64::from(crate::editor::history::PATCH_MIN_H),
                max: None,
            },
        }
    }
}

/// `bounded` from the desktop module: a finite number is rounded like
/// `Math.round` (halves toward +∞) and clamped; anything else is rejected.
fn bounded_number(value: f64, limits: LayoutLimits) -> Option<f64> {
    if !value.is_finite() {
        return None;
    }
    let rounded = (value + 0.5).floor();
    Some(
        rounded
            .max(limits.min)
            .min(limits.max.unwrap_or(f64::INFINITY)),
    )
}

/// One `panes.<key>` entry: bounded like the history fields, falling back
/// to that pane's default when the object or value is absent/bad.
fn pane_field(record: &Map<String, Value>, kind: PaneKind) -> f64 {
    record
        .get("panes")
        .and_then(Value::as_object)
        .and_then(|panes| panes.get(kind.key()))
        .and_then(|value| bounded_json(value, kind.limits()))
        .unwrap_or_else(|| kind.default_width())
}

fn bounded_json(value: &Value, limits: LayoutLimits) -> Option<f64> {
    bounded_number(value.as_f64()?, limits)
}

/// Integral pixel values serialize without a fraction, matching the desktop
/// `JSON.stringify` output (`140`, not `140.0`).
fn json_number(value: f64) -> Value {
    if value.fract() == 0.0 && (0.0..=u64::MAX as f64).contains(&value) {
        Value::from(value as u64)
    } else {
        Value::from(value)
    }
}

impl HistoryLayout {
    /// `readLayout` on the desktop: parses the stored JSON document, falling
    /// back to [`DEFAULT_LAYOUT`] when it is missing, unparseable, or not an
    /// object. Each field is bounded independently — a bad value resets only
    /// that field. Numbers beyond the f64 range (e.g. `1e400`) are
    /// unparseable for `serde_json`, so they fall back for the whole
    /// document rather than per field.
    pub fn parse(raw: &str) -> HistoryLayout {
        let Ok(value) = serde_json::from_str::<Value>(raw) else {
            return DEFAULT_LAYOUT;
        };
        let Some(record) = value.as_object() else {
            return DEFAULT_LAYOUT;
        };
        let field = |name: &str, limits: LayoutLimits, default: f64| {
            record
                .get(name)
                .and_then(|value| bounded_json(value, limits))
                .unwrap_or(default)
        };
        HistoryLayout {
            graph: record
                .get("graph")
                .and_then(|value| bounded_json(value, HistoryField::Graph.limits())),
            date: field("date", HistoryField::Date.limits(), DEFAULT_LAYOUT.date),
            author: field(
                "author",
                HistoryField::Author.limits(),
                DEFAULT_LAYOUT.author,
            ),
            commit: field(
                "commit",
                HistoryField::Commit.limits(),
                DEFAULT_LAYOUT.commit,
            ),
            detail_height: field(
                "detailHeight",
                HistoryField::DetailHeight.limits(),
                DEFAULT_LAYOUT.detail_height,
            ),
            panes: PaneWidths {
                files_aside: pane_field(record, PaneKind::FilesAside),
                changes_list: pane_field(record, PaneKind::ChangesList),
                stashes_list: pane_field(record, PaneKind::StashesList),
            },
        }
    }

    /// Parses an optional stored value: `None` (key absent) yields
    /// [`DEFAULT_LAYOUT`], `Some` goes through [`HistoryLayout::parse`].
    pub fn load(stored: Option<&str>) -> HistoryLayout {
        stored.map_or(DEFAULT_LAYOUT, HistoryLayout::parse)
    }

    /// `JSON.stringify(layout)` on the desktop: `graph` is omitted while
    /// unset, `detailHeight` keeps its camelCase key.
    pub fn to_json(self) -> String {
        let mut record = Map::new();
        if let Some(graph) = self.graph {
            record.insert("graph".to_string(), json_number(graph));
        }
        record.insert("date".to_string(), json_number(self.date));
        record.insert("author".to_string(), json_number(self.author));
        record.insert("commit".to_string(), json_number(self.commit));
        record.insert("detailHeight".to_string(), json_number(self.detail_height));
        let mut panes = Map::new();
        for kind in [
            PaneKind::FilesAside,
            PaneKind::ChangesList,
            PaneKind::StashesList,
        ] {
            panes.insert(kind.key().to_string(), json_number(self.panes.get(kind)));
        }
        record.insert("panes".to_string(), Value::Object(panes));
        serde_json::to_string(&Value::Object(record))
            .expect("serializing a JSON object to a String cannot fail")
    }

    /// `setColumn` on the desktop: bounds `px` and assigns it to `column`;
    /// a non-finite value leaves the layout unchanged.
    pub fn set_column(&mut self, column: HistoryColumn, px: f64) {
        let Some(value) = bounded_number(px, column.limits()) else {
            return;
        };
        match column {
            HistoryColumn::Graph => self.graph = Some(value),
            HistoryColumn::Date => self.date = value,
            HistoryColumn::Author => self.author = value,
            HistoryColumn::Commit => self.commit = value,
        }
    }

    /// `setDetailHeight` on the desktop.
    pub fn set_detail_height(&mut self, px: f64) {
        if let Some(value) = bounded_number(px, HistoryField::DetailHeight.limits()) {
            self.detail_height = value;
        }
    }

    /// `reset` on the desktop: restores one field to its default — `graph`
    /// becomes unset, the rest return to [`DEFAULT_LAYOUT`] values.
    pub fn reset(&mut self, field: HistoryField) {
        match field {
            HistoryField::Graph => self.graph = None,
            HistoryField::Date => self.date = DEFAULT_LAYOUT.date,
            HistoryField::Author => self.author = DEFAULT_LAYOUT.author,
            HistoryField::Commit => self.commit = DEFAULT_LAYOUT.commit,
            HistoryField::DetailHeight => self.detail_height = DEFAULT_LAYOUT.detail_height,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> HistoryLayout {
        HistoryLayout::parse(json)
    }

    #[test]
    fn defaults_without_storage() {
        let layout = HistoryLayout::load(None);
        assert_eq!(layout, DEFAULT_LAYOUT);
        assert_eq!(layout.graph, None);
        assert_eq!(layout.date, 120.0);
    }

    #[test]
    fn invalid_json_falls_back_to_defaults() {
        assert_eq!(parse("not json {"), DEFAULT_LAYOUT);
        assert_eq!(parse("42"), DEFAULT_LAYOUT);
        assert_eq!(parse("null"), DEFAULT_LAYOUT);
        assert_eq!(parse("[1,2]"), DEFAULT_LAYOUT);
        assert_eq!(parse("\"wide\""), DEFAULT_LAYOUT);
        assert_eq!(HistoryLayout::load(Some("")), DEFAULT_LAYOUT);
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let layout =
            parse(r#"{"graph":500,"date":9999,"author":10,"commit":200,"detailHeight":5}"#);
        assert_eq!(layout.graph, Some(320.0));
        assert_eq!(layout.date, 360.0);
        assert_eq!(layout.author, 56.0);
        assert_eq!(layout.commit, 200.0);
        // Native floor is the patch minimum (96), not the desktop's 160 —
        // viewport-forced sub-160 values must survive a store round-trip.
        assert_eq!(layout.detail_height, 96.0);
    }

    #[test]
    fn sub_floor_detail_height_round_trips() {
        // A drag at the 760×480 minimum can persist ~117; the loader must
        // keep it (previously clamped back to 160 and lost on restart).
        let layout = parse(r#"{"detailHeight":117}"#);
        assert_eq!(layout.detail_height, 117.0);
        // Below the render floor still clamps.
        let layout = parse(r#"{"detailHeight":50}"#);
        assert_eq!(layout.detail_height, 96.0);
        // And it serializes back out unchanged.
        let json = layout.to_json();
        assert_eq!(HistoryLayout::load(Some(json.as_str())).detail_height, 96.0);
    }

    #[test]
    fn bad_fields_fall_back_without_touching_valid_ones() {
        let layout = parse(r#"{"date":"wide","detailHeight":"tall","author":200}"#);
        assert_eq!(layout.date, 120.0);
        assert_eq!(layout.detail_height, 320.0);
        assert_eq!(layout.author, 200.0);
    }

    #[test]
    fn values_are_rounded_and_floats_accepted() {
        let layout = parse(r#"{"date":200.6,"author":199.5,"commit":-40,"graph":null}"#);
        assert_eq!(layout.date, 201.0);
        assert_eq!(layout.author, 200.0);
        assert_eq!(layout.commit, 56.0);
        assert_eq!(layout.graph, None);
    }

    #[test]
    fn empty_object_gives_defaults() {
        assert_eq!(parse("{}"), DEFAULT_LAYOUT);
    }

    #[test]
    fn changes_serialize_to_the_store_document() {
        let mut layout = HistoryLayout::default();
        layout.set_column(HistoryColumn::Date, 200.0);
        let stored = layout.to_json();
        let value: Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(value["date"], json_number(200.0));
        assert!(value.get("graph").is_none());

        layout.set_column(HistoryColumn::Author, 9999.0);
        assert_eq!(HistoryLayout::parse(&layout.to_json()).author, 360.0);

        layout.set_column(HistoryColumn::Graph, 140.0);
        let stored = layout.to_json();
        let value: Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(value["graph"], json_number(140.0));

        layout.set_detail_height(500.0);
        let stored = layout.to_json();
        let value: Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(value["detailHeight"], json_number(500.0));
    }

    #[test]
    fn serialization_omits_graph_until_set() {
        let stored = DEFAULT_LAYOUT.to_json();
        assert!(!stored.contains("graph"));
        let value: Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "date": 120,
                "author": 128,
                "commit": 80,
                "detailHeight": 320,
                "panes": {
                    "filesAside": 280,
                    "changesList": 300,
                    "stashesList": 280,
                },
            })
        );

        let mut layout = DEFAULT_LAYOUT;
        layout.set_column(HistoryColumn::Graph, 140.0);
        let stored = layout.to_json();
        assert!(stored.contains("\"graph\":140"));
        assert_eq!(HistoryLayout::parse(&stored), layout);
    }

    #[test]
    fn reset_restores_one_field() {
        let mut layout = HistoryLayout::default();
        layout.set_column(HistoryColumn::Date, 200.0);
        layout.set_column(HistoryColumn::Author, 300.0);
        layout.set_column(HistoryColumn::Graph, 140.0);
        layout.reset(HistoryField::Date);
        assert_eq!(layout.date, 120.0);
        assert_eq!(layout.author, 300.0);
        assert_eq!(layout.graph, Some(140.0));

        layout.reset(HistoryField::Graph);
        assert_eq!(layout.graph, None);

        layout.set_detail_height(500.0);
        layout.reset(HistoryField::DetailHeight);
        assert_eq!(layout.detail_height, 320.0);
    }

    #[test]
    fn non_finite_setters_leave_layout_unchanged() {
        let mut layout = DEFAULT_LAYOUT;
        layout.set_column(HistoryColumn::Date, f64::NAN);
        layout.set_column(HistoryColumn::Date, f64::INFINITY);
        layout.set_detail_height(f64::NEG_INFINITY);
        assert_eq!(layout, DEFAULT_LAYOUT);
    }

    #[test]
    fn round_trip_preserves_values() {
        let mut layout = DEFAULT_LAYOUT;
        layout.set_column(HistoryColumn::Graph, 140.0);
        layout.set_column(HistoryColumn::Commit, 200.0);
        layout.set_detail_height(400.0);
        assert_eq!(HistoryLayout::parse(&layout.to_json()), layout);
    }

    #[test]
    fn panes_default_when_absent_or_malformed() {
        // Documents written before pane widths existed have no `panes` key.
        let layout = parse(r#"{"date":200}"#);
        assert_eq!(layout.panes, PaneWidths::default());
        assert_eq!(layout.panes.files_aside, 280.0);
        assert_eq!(layout.panes.changes_list, 300.0);
        assert_eq!(layout.panes.stashes_list, 280.0);

        // A non-object `panes` or bad fields fall back per field.
        let layout = parse(r#"{"panes":"wide","date":200}"#);
        assert_eq!(layout.panes, PaneWidths::default());
        let layout =
            parse(r#"{"panes":{"filesAside":9999,"changesList":"x","stashesList":240.4}}"#);
        assert_eq!(layout.panes.files_aside, 480.0);
        assert_eq!(layout.panes.changes_list, 300.0);
        assert_eq!(layout.panes.stashes_list, 240.0);
    }

    #[test]
    fn pane_widths_set_get_reset_and_round_trip() {
        let mut layout = DEFAULT_LAYOUT;
        layout.panes.set(PaneKind::FilesAside, 350.0);
        layout.panes.set(PaneKind::StashesList, 5000.0);
        assert_eq!(layout.panes.get(PaneKind::FilesAside), 350.0);
        // Static bounds clamp like the history fields.
        assert_eq!(layout.panes.get(PaneKind::StashesList), 480.0);
        layout.panes.reset(PaneKind::ChangesList);
        assert_eq!(layout.panes.get(PaneKind::ChangesList), 300.0);

        let stored = layout.to_json();
        let value: Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(value["panes"]["filesAside"], json_number(350.0));
        assert_eq!(HistoryLayout::parse(&stored).panes, layout.panes);
    }

    #[test]
    fn pane_setters_ignore_non_finite() {
        let mut panes = PaneWidths::default();
        panes.set(PaneKind::FilesAside, f64::NAN);
        panes.set(PaneKind::FilesAside, f64::INFINITY);
        assert_eq!(panes, PaneWidths::default());
    }
}
