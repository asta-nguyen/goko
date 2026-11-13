//! Native History section: branch picker, filter, commit table with the lane
//! graph, paging, Jump to HEAD, match navigation, and the commit detail pane.
//! Ports the desktop `EditorHistory`/`HistoryTable` behaviour onto
//! `rune_kit::Table` (virtualised rows, keyboard nav, column resize) plus the
//! shared `patch` diff pane.
//!
//! Graph drawing is row-local: every primitive the desktop SVG emits for a row
//! lives inside that row's 28pt band, so each graph cell paints its own
//! `canvas`. The dashed uncommitted→HEAD connector is split per row band with
//! a shared global dash phase, exactly reproducing the single SVG line.

use std::borrow::Cow;
use std::collections::HashSet;

use chrono::{DateTime, Local, TimeZone};
use rune::{Color, Cursor, El, LineCap, Path, Shape, TextStyle, Tx, canvas, col, el, row};
use rune_kit::Kit;
use rune_kit::{MenuItem, context_menu};

use crate::State;
use crate::editor::backend::Req;
use crate::editor::commit_detail;
use crate::editor::history_graph::{GraphLayout, GraphRow, layout_graph};
use crate::editor::history_layout::{
    DEFAULT_LAYOUT, DetailPosition, HistoryColumn, HistoryField, HistoryLayout,
};
use crate::editor::patch::{self, Pane};
use crate::editor::state::{HistoryRowMenu, HistoryState, ListView, Section};
use crate::editor::table::{Cell, Table, TableColumn, TableState, TableStyle};
use crate::theme::{Ink, STATE_SPRING, T11M_LH, T11MN_BADGE, T12, T12M, btn_sm, txt};

pub const ROW_H: f32 = 28.0;
const LANE_W: f32 = 14.0;
const GRAPH_PAD: f32 = 8.0;
const TABLE_MIN_H: f32 = 120.0;
/// Desktop `DETAIL_HANDLE_HEIGHT` — the drag strip between table and detail.
const DETAIL_HANDLE_H: f32 = 8.0;
/// [`DETAIL_HANDLE_H`] turned sideways: the drag strip between the table and
/// a Right-position detail.
const DETAIL_HANDLE_W: f32 = 8.0;
/// Minimum width the commit table keeps in Right position; the detail width
/// clamp yields around it so the pane is never pushed off-screen.
const TABLE_MIN_W: f32 = 360.0;
/// Fixed toolbar height (`toolbar` below) — subtracted from the measured root
/// when clamping the detail pane so the patch never lands below the window.
const TOOLBAR_H: f32 = 34.0;
/// `stale_notice` row height when a refresh error shows above the table.
const STALE_H: f32 = 30.0;
/// Cell padding shared by `TableStyle` and the graph cell's negative margin
/// (desktop `px-2`). The graph canvas cancels it so lanes keep desktop
/// geometry instead of losing 2×pad of lane room to the kit's cell padding.
const CELL_PAD: f32 = 8.0;
/// Floor for the commit patch pane inside the detail split — enough for its
/// toolbar plus a few diff lines; the metadata header yields first.
pub(crate) const PATCH_MIN_H: f32 = 96.0;
/// Comfortable detail height on viewports that can fit it — the desktop
/// `detailHeight.min` (160). Shorter windows (the 760×480 minimum reaches
/// `dmax` ≈ 157) yield down to [`PATCH_MIN_H`] instead, and the persisted
/// model floor matches that yield so viewport-forced values round-trip.
const DETAIL_MIN: f64 = 160.0;
/// Row id for the synthetic "Uncommitted changes" row (never a commit oid).
pub const UNCOMMITTED_ID: &str = "__uncommitted__";
/// Row id for the synthetic "Load more commits" paging row.
const LOAD_MORE_ID: &str = "__load_more__";

// ---------------------------------------------------------------------------
// Shared helpers (commit_detail / stashes reuse these).
// ---------------------------------------------------------------------------

/// `matchesQuery`: case-insensitive, every whitespace term must appear.
pub(crate) fn matches_query(text: &str, query: &str) -> bool {
    let haystack = text.to_lowercase();
    query
        .to_lowercase()
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .all(|term| haystack.contains(term))
}

/// Epoch milliseconds → local-time `DateTime`. `None` for instants outside
/// chrono's representable range or unconvertible `u64` values.
fn local_dt(epoch_ms: u64) -> Option<DateTime<Local>> {
    Local
        .timestamp_millis_opt(i64::try_from(epoch_ms).ok()?)
        .single()
}

/// `formatTime`: local medium date + short time, matching the desktop
/// `toLocaleString({ dateStyle: 'medium', timeStyle: 'short' })` — e.g.
/// "Nov 14, 2023, 10:13 PM". Invalid instants render "-".
pub(crate) fn format_time(epoch_ms: u64) -> String {
    local_dt(epoch_ms)
        .map(|d| d.format("%b %-d, %Y, %-I:%M %p").to_string())
        .unwrap_or_else(|| "-".into())
}

/// `formatRelative`: relative under 30 days, medium local date beyond.
pub(crate) fn format_relative(epoch_ms: u64, now_ms: u64) -> String {
    const MINUTE: u64 = 60_000;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    const MONTH: u64 = 30 * DAY;
    let e = now_ms.saturating_sub(epoch_ms);
    if e < MINUTE {
        return "just now".into();
    }
    if e < MONTH {
        // Desktop rounds each unit like `Math.round`, not truncation.
        if e < HOUR {
            return ago((e as f64 / MINUTE as f64).round() as i64, "minute");
        }
        if e < DAY {
            return ago((e as f64 / HOUR as f64).round() as i64, "hour");
        }
        let days = (e as f64 / DAY as f64).round() as i64;
        return if days == 1 {
            "yesterday".into()
        } else {
            ago(days, "day")
        };
    }
    local_dt(epoch_ms)
        .map(|d| d.format("%b %-d, %Y").to_string())
        .unwrap_or_else(|| "-".into())
}

fn ago(n: i64, unit: &str) -> String {
    let n = n.max(1);
    if n == 1 {
        format!("1 {unit} ago")
    } else {
        format!("{n} {unit}s ago")
    }
}

/// `refBadge` → (label, is_tag, is_remote).
pub(crate) fn ref_badge(r: &str) -> (String, bool, bool) {
    if let Some(t) = r.strip_prefix("tag:") {
        (t.trim().to_string(), true, false)
    } else if let Some(rem) = r.strip_prefix("remotes/") {
        (rem.to_string(), false, true)
    } else {
        (r.to_string(), false, false)
    }
}

pub(crate) fn status_label(status: &str) -> &str {
    if status == "typechange" {
        "type changed"
    } else {
        status
    }
}

pub(crate) fn entry_name(entry: &app_core::git_view::StatusEntry) -> String {
    match &entry.orig_path {
        Some(orig) => format!("{orig} → {}", entry.path),
        None => entry.path.clone(),
    }
}

// ---------------------------------------------------------------------------
// Graph geometry — pure segments in row-local coords, unit-testable.
// ---------------------------------------------------------------------------

/// Paint source for a shape primitive.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Paint {
    Palette(usize),
    /// Pane background (HEAD ring fill, merge inner dot).
    Bg,
    /// Muted text colour (uncommitted dot when no HEAD lane is known).
    Muted,
}

/// One drawable primitive inside a graph cell: local x in `[0, graph_w]`,
/// local y in `[0, ROW_H]`.
#[derive(Clone, Debug, PartialEq)]
pub enum Seg {
    /// Polyline stroke (2 points = straight).
    Line {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        w: f32,
        color: Paint,
    },
    /// Filled/stroked circle approximated by a polygon.
    Disc {
        cx: f32,
        cy: f32,
        r: f32,
        fill: Option<Paint>,
        stroke: Option<(Paint, f32)>,
    },
    /// Cubic bezier stroke: move_to(p0) → cubic_to(c1, c2, p1).
    Cubic {
        p0: [f32; 2],
        c1: [f32; 2],
        c2: [f32; 2],
        p1: [f32; 2],
        color: Paint,
    },
}

pub fn lane_x(lane: usize) -> f32 {
    GRAPH_PAD + lane as f32 * LANE_W + LANE_W / 2.0
}

/// Expand a vertical dashed line at `x` covering LOCAL y `[y0, y1)` into solid
/// segments. `global_top` is this row band's top in global points; dash =
/// 4 on / 3 off (period 7) phase-locked to global y so adjacent row cells
/// continue the pattern seamlessly.
fn dashed_v(x: f32, y0: f32, y1: f32, global_top: f32, color: Paint) -> Vec<Seg> {
    let mut out = Vec::new();
    if y1 <= y0 {
        return out;
    }
    let period = 7.0f32;
    let on = 4.0f32;
    let mut ly = y0;
    // Advance to the first on-interval boundary at/after global y.
    let pos = (global_top + y0).rem_euclid(period);
    if pos >= on {
        ly += period - pos;
    } else {
        // Inside an on-interval: emit until it ends.
        let take = (on - pos).min(y1 - ly);
        out.push(Seg::Line {
            x0: x,
            y0: ly,
            x1: x,
            y1: ly + take,
            w: 2.0,
            color,
        });
        ly += take;
    }
    while ly < y1 - 0.001 {
        // gap of `period - on`
        ly += (period - on).min(y1 - ly);
        if ly >= y1 - 0.001 {
            break;
        }
        let take = on.min(y1 - ly);
        out.push(Seg::Line {
            x0: x,
            y0: ly,
            x1: x,
            y1: ly + take,
            w: 2.0,
            color,
        });
        ly += take;
    }
    out
}

/// Segments for one commit row (through/incoming/edges/node). The uncommitted
/// connector is added separately by [`connector_segments`].
pub fn row_segments(row: &GraphRow) -> Vec<Seg> {
    let mut segs = Vec::new();
    let cy = ROW_H / 2.0;
    for t in &row.through {
        segs.push(Seg::Line {
            x0: lane_x(t.lane),
            y0: 0.0,
            x1: lane_x(t.lane),
            y1: ROW_H,
            w: 2.0,
            color: Paint::Palette(t.color),
        });
    }
    if row.incoming {
        let cx = lane_x(row.lane);
        segs.push(Seg::Line {
            x0: cx,
            y0: 0.0,
            x1: cx,
            y1: cy,
            w: 2.0,
            color: Paint::Palette(row.color),
        });
    }
    for e in &row.edges {
        let (x1, x2) = (lane_x(e.from_lane), lane_x(e.to_lane));
        let p = Paint::Palette(e.color);
        if e.from_lane == e.to_lane {
            segs.push(Seg::Line {
                x0: x1,
                y0: cy,
                x1: x2,
                y1: ROW_H,
                w: 2.0,
                color: p,
            });
        } else {
            let mid = (cy + ROW_H) / 2.0;
            segs.push(Seg::Line {
                x0: x1,
                y0: cy,
                x1,
                y1: mid,
                w: 2.0,
                color: p,
            });
            segs.push(Seg::Cubic {
                p0: [x1, mid],
                c1: [x1, ROW_H],
                c2: [x2, mid],
                p1: [x2, ROW_H],
                color: p,
            });
        }
    }
    let cx = lane_x(row.lane);
    if row.head {
        segs.push(Seg::Disc {
            cx,
            cy,
            r: 5.0,
            fill: Some(Paint::Bg),
            stroke: Some((Paint::Palette(row.color), 2.0)),
        });
        if row.merge {
            segs.push(Seg::Disc {
                cx,
                cy,
                r: 1.5,
                fill: Some(Paint::Palette(row.color)),
                stroke: None,
            });
        }
    } else if row.merge {
        segs.push(Seg::Disc {
            cx,
            cy,
            r: 4.0,
            fill: Some(Paint::Palette(row.color)),
            stroke: None,
        });
        segs.push(Seg::Disc {
            cx,
            cy,
            r: 1.5,
            fill: Some(Paint::Bg),
            stroke: None,
        });
    } else {
        segs.push(Seg::Disc {
            cx,
            cy,
            r: 4.0,
            fill: Some(Paint::Palette(row.color)),
            stroke: None,
        });
    }
    segs
}

/// Segments for the synthetic uncommitted row (global band `[-ROW_H, 0)`):
/// node dot plus the local slice `[cy, ROW_H]` of the dashed connector.
pub fn uncommitted_segments(head_lane: Option<usize>, head_color: usize) -> Vec<Seg> {
    let x = lane_x(head_lane.unwrap_or(0));
    let cy = ROW_H / 2.0;
    let mut segs = vec![Seg::Disc {
        cx: x,
        cy,
        r: 4.0,
        fill: Some(if head_lane.is_some() {
            Paint::Palette(head_color)
        } else {
            Paint::Muted
        }),
        stroke: None,
    }];
    segs.extend(dashed_v(x, cy, ROW_H, -ROW_H, Paint::Palette(head_color)));
    segs
}

/// Connector slice for commit row `index` when the uncommitted row shows:
/// full band for rows above HEAD, top half (`[0, cy]`) for the HEAD row.
pub fn connector_segments(
    index: usize,
    head_index: usize,
    head_lane: usize,
    head_color: usize,
) -> Vec<Seg> {
    if index > head_index {
        return Vec::new();
    }
    let x = lane_x(head_lane);
    let base = index as f32 * ROW_H;
    let (y0, y1) = if index == head_index {
        (0.0, ROW_H / 2.0)
    } else {
        (0.0, ROW_H)
    };
    dashed_v(x, y0, y1, base, Paint::Palette(head_color))
}

fn circle_pts(cx: f32, cy: f32, r: f32) -> Vec<[f32; 2]> {
    const N: usize = 24;
    static UNIT_CIRCLE: std::sync::LazyLock<[[f32; 2]; 24]> = std::sync::LazyLock::new(|| {
        let mut pts = [[0.0, 0.0]; N];
        for (i, pt) in pts.iter_mut().enumerate() {
            let a = i as f32 / N as f32 * std::f32::consts::TAU;
            *pt = [a.cos(), a.sin()];
        }
        pts
    });
    UNIT_CIRCLE
        .iter()
        .map(|[cos, sin]| [cx + cos * r, cy + sin * r])
        .collect()
}

/// Palette index → linear colour.
pub fn palette(idx: usize) -> Color {
    crate::editor::history_graph::palette(idx)
}

fn paint(p: Paint, bg: Color, muted: Color, opacity: f32) -> Color {
    let c = match p {
        Paint::Palette(i) => palette(i),
        Paint::Bg => bg,
        Paint::Muted => muted,
    };
    if opacity >= 1.0 { c } else { c.alpha(opacity) }
}

fn segs_to_shape(segs: &[Seg], bg: Color, muted: Color, opacity: f32) -> Shape {
    let mut shape = Shape::new();
    for seg in segs {
        shape = match seg {
            Seg::Line {
                x0,
                y0,
                x1,
                y1,
                w,
                color,
            } => shape.line(
                [*x0, *y0],
                [*x1, *y1],
                *w,
                LineCap::Round,
                paint(*color, bg, muted, opacity),
            ),
            Seg::Disc {
                cx,
                cy,
                r,
                fill,
                stroke,
            } => {
                let pts = circle_pts(*cx, *cy, *r);
                let mut s = shape;
                if let Some(f) = fill {
                    s = s.fill(pts.clone(), paint(*f, bg, muted, opacity));
                }
                if let Some((c, w)) = stroke {
                    s = s.stroke_with(pts, true, *w, LineCap::Round, paint(*c, bg, muted, opacity));
                }
                s
            }
            Seg::Cubic {
                p0,
                c1,
                c2,
                p1,
                color,
            } => {
                let path = Path::new().move_to(*p0).cubic_to(*c1, *c2, *p1);
                shape.stroke_path(
                    &path,
                    2.0,
                    LineCap::Round,
                    paint(*color, bg, muted, opacity),
                )
            }
        };
    }
    shape
}

// ---------------------------------------------------------------------------
// View.
// ---------------------------------------------------------------------------

/// Required public entry: renders History from shared state.
pub fn view(s: &State) -> El {
    history_view_impl(s, &s.ink, &s.editor.kit)
}

fn layout_px(l: &HistoryLayout, f: HistoryField) -> f64 {
    match f {
        HistoryField::Graph => l.graph.unwrap_or(f64::NAN),
        HistoryField::Date => l.date,
        HistoryField::Author => l.author,
        HistoryField::Commit => l.commit,
        HistoryField::DetailHeight => l.detail_height,
        HistoryField::DetailWidth => l.detail_width,
    }
}

fn history_view_impl(s: &State, ink: &Ink, k: &Kit) -> El {
    let h = &s.editor.history;

    if h.commits.is_empty() && h.branch.is_none() && !h.loading {
        if let Some(err) = &h.error {
            return error_pane(err, ink);
        }
        return empty_pane(
            ink,
            "No commits yet",
            "This repository has no commits on its local, remote-tracking, or tag refs.",
        );
    }
    if h.loading && h.commits.is_empty() && h.error.is_none() {
        return loading_pane("Loading history…", ink);
    }
    if h.error.is_some() && h.commits.is_empty() {
        return error_pane(h.error.as_deref().unwrap_or("failed"), ink);
    }

    let mut root = col().grow(1.0).min_w(0.0).h_full().min_h(0.0);
    let mut layer = Vec::new();
    root = root.child(toolbar(s, ink, k, &mut layer));
    if let Some(err) = &h.error
        && !h.commits.is_empty()
    {
        root = root.child(stale_notice(err, ink));
    }

    if h.layout.detail_position == DetailPosition::Right {
        // Lazygit-style column layout:
        // Left column: Commits table on top + Commit details & changed files on bottom.
        // Right column: Full-height, prominent Diff / changes pane.
        let (wfloor, wmax) = detail_width_bounds(h);
        let detail_w = h.layout.detail_width.clamp(wfloor, wmax) as f32;

        let (detail_floor, dmax) = detail_height_bounds(h);
        let detail_h = h.layout.detail_height.clamp(detail_floor, dmax) as f32;

        root = root.child(
            row()
                .grow(1.0)
                .min_h(0.0)
                .w_full()
                .child(
                    col()
                        .grow(1.0)
                        .min_w(TABLE_MIN_W)
                        .h_full()
                        .child(table_view(s, ink, k).grow(1.0).min_h(TABLE_MIN_H))
                        .child(commit_detail_drawer_handle(ink))
                        .child(
                            col()
                                .h(detail_h)
                                .shrink()
                                .min_h(0.0)
                                .border_sides([1.0, 0.0, 0.0, 0.0], ink.border)
                                .child(commit_detail::view(s)),
                        ),
                )
                .child(detail_width_handle(ink))
                .child(
                    // Right column: full-height diff pane
                    col()
                        .w(detail_w)
                        .h_full()
                        .shrink()
                        .min_w(0.0)
                        .border_sides([0.0, 0.0, 0.0, 1.0], ink.border)
                        .child(right_diff_pane(s, ink, k)),
                ),
        );
    } else {
        root = root.child(table_view(s, ink, k).grow(1.0).min_h(TABLE_MIN_H));

        // Detail height clamped to the measured container when known.
        let (detail_floor, dmax) = detail_height_bounds(h);
        let detail_h = h.layout.detail_height.clamp(detail_floor, dmax) as f32;

        root = root.child(detail_handle(ink));
        root = root.child(
            col()
                .h(detail_h)
                .shrink()
                .min_h(0.0)
                .border_sides([1.0, 0.0, 0.0, 0.0], ink.border)
                .child(bottom_detail_pane(s, ink, k)),
        );
    }

    // Commit-row context menu (desktop `HistoryTable` ContextMenu): opened
    // by a cell's secondary press at the stored point, dismissed by the
    // kit scrim/modal, and cleared on close or item pick.
    if let Some(m) = &h.row_menu {
        let commit = m.commit.clone();
        if h.commits.iter().any(|c| c.id == commit) {
            let (x, y) = (m.x, m.y);
            let hash_id = commit.clone();
            let subj_id = commit;
            let menu = context_menu("history-row", el(), Some(rune::PointerPress { x, y }))
                .item(MenuItem::new("Copy hash").on_select_tx(
                    move |st: &mut State, tx: &mut Tx| {
                        st.editor.history.row_menu = None;
                        if let Some(c) = st.editor.history.commits.iter().find(|c| c.id == hash_id)
                        {
                            let t = c.id.clone();
                            tx.copy(t);
                            st.notice = Some("Copied to clipboard".into());
                        }
                    },
                ))
                .item(MenuItem::new("Copy subject").on_select_tx(
                    move |st: &mut State, tx: &mut Tx| {
                        st.editor.history.row_menu = None;
                        if let Some(c) = st.editor.history.commits.iter().find(|c| c.id == subj_id)
                        {
                            let t = c.subject.clone();
                            tx.copy(t);
                            st.notice = Some("Copied to clipboard".into());
                        }
                    },
                ))
                .on_close(|st: &mut State| st.editor.history.row_menu = None);
            root = root.child(menu.build(k, &mut layer));
        }
    }

    // Changed-file list context menu on the commit detail: a secondary
    // press on `commit_detail::changed_files` stores the point here and the
    // View choice is written straight into the shared `editor.list_view`.
    if let Some((x, y)) = h.file_menu.as_ref().and_then(|m| m.point()) {
        let view = s.editor.list_view;
        let view_item = |label: &'static str, v: ListView| {
            MenuItem::new(label)
                .checked(view == v)
                .on_select(move |st: &mut State| {
                    st.editor.list_view = v;
                    st.editor.history.file_menu = None;
                })
        };
        let menu = context_menu("history-files", el(), Some(rune::PointerPress { x, y }))
            .group_label("View")
            .item(view_item("List", ListView::List))
            .item(view_item("Tree", ListView::Tree))
            .on_close(|st: &mut State| st.editor.history.file_menu = None);
        root = root.child(menu.build(k, &mut layer));
    }

    // `on_layout_tx` fires only when this element's rect changes (>0.5pt), so
    // it is kept solely for the detail-pane clamp measurements. All
    // &mut housekeeping lives in [`poll`], driven by `AppSpec::poll`.
    root = root.on_layout_tx(|st: &mut State, r: [f32; 4], tx: &mut Tx| {
        let (hgt, wid) = (r[3], r[2]);
        let mut wake = false;
        if (hgt - st.editor.history.measured_root_height).abs() > 0.5 {
            st.editor.history.measured_root_height = hgt;
            wake = true;
        }
        if (wid - st.editor.history.measured_root_width).abs() > 0.5 {
            st.editor.history.measured_root_width = wid;
            wake = true;
        }
        if wake {
            // The detail clamps read these measurements — schedule one more
            // frame so the panes re-lay out against the real root size
            // instead of the pre-measure estimate.
            tx.wake_after(std::time::Duration::from_millis(16));
        }
    });
    rune_kit::root(root, layer)
}

/// `AppSpec::poll` hook for the History section (wire as
/// `changed |= editor::history::poll(s, tx)` in `main.rs`). Runs the
/// housekeeping that cannot live in `on_layout_tx` — that callback only
/// fires when the root rect changes, so commits landing, `jump_to`, and
/// column resizes would otherwise go unnoticed while the rect is stable.
/// Auto-select/fetch only while History is the active section; returns true
/// when visible state changed.
pub fn poll(s: &mut State, tx: &mut Tx) -> bool {
    if s.editor.section != Section::History {
        return false;
    }
    if s.editor.history.graph.is_none() && !s.editor.history.commits.is_empty() {
        s.editor.history.graph = Some(std::sync::Arc::new(
            crate::editor::history_graph::layout_graph(&s.editor.history.commits),
        ));
    }
    let mut changed = auto_select(s);
    changed |= consume_jump_to(s, tx);
    changed |= sync_layout_from_table(s);
    changed |= ensure_changes_data(s);
    changed |= clear_expired_copy_feedback(s);
    changed |= drop_stale_row_menu(s);
    changed
}

/// Desktop clears the detail "Copied" note on a 1500ms `setTimeout`; Rune
/// has no timer, so `poll` expires it (the copy handler also wakes a frame
/// so the note disappears without waiting for the next interaction).
fn clear_expired_copy_feedback(s: &mut State) -> bool {
    let expired = s
        .editor
        .history
        .copied_hash
        .as_ref()
        .is_some_and(|(_, at)| {
            at.elapsed() >= std::time::Duration::from_millis(commit_detail::COPY_FEEDBACK_MS)
        });
    if expired {
        s.editor.history.copied_hash = None;
    }
    expired
}

/// A row menu whose commit vanished (refresh/rebase while open) closes
/// instead of offering copies of a row that no longer exists.
fn drop_stale_row_menu(s: &mut State) -> bool {
    let stale = match &s.editor.history.row_menu {
        Some(m) => !s.editor.history.commits.iter().any(|c| c.id == m.commit),
        None => false,
    };
    if stale {
        s.editor.history.row_menu = None;
    }
    stale
}

fn auto_select(s: &mut State) -> bool {
    let pick = {
        let h = &s.editor.history;
        if h.selected.is_some() || h.commits.is_empty() {
            return false;
        }
        h.commits
            .iter()
            .find(|c| c.head)
            .or_else(|| h.commits.first())
            .map(|c| c.id.clone())
    };
    let Some(id) = pick else { return false };
    s.editor.history.selected = Some(id.clone());
    s.editor.history.detail = None;
    enqueue_commit_diff(s, &id);
    true
}

/// The synthetic uncommitted row needs `changes.data`, but `refresh_active`
/// only fetches it for the Changes section. Kick one guarded read so the row
/// appears without visiting Changes first; stops on data, in-flight, or error.
fn ensure_changes_data(s: &mut State) -> bool {
    {
        let c = &s.editor.changes;
        if changes_fresh(s) || c.loading || c.error.is_some() {
            return false;
        }
    }
    let (Some(checkout), Some(key)) = (s.editor.checkout.clone(), s.editor.checkout_key.clone())
    else {
        return false;
    };
    s.editor.changes.loading = true;
    if s.editor.enqueue_read(checkout, key, Req::Changes) {
        true
    } else {
        s.editor.changes.loading = false;
        false
    }
}

fn consume_jump_to(s: &mut State, tx: &mut Tx) -> bool {
    let Some(id) = s.editor.history.jump_to.take() else {
        return false;
    };
    if id == UNCOMMITTED_ID {
        select_uncommitted(s);
        return true;
    }
    if s.editor.history.commits.iter().any(|c| c.id == id) {
        s.editor.history.selected = Some(id.clone());
        s.editor.history.detail = None;
        s.editor.history.table.reveal(id.clone(), tx);
        enqueue_commit_diff(s, &id);
    }
    true
}

fn enqueue_commit_diff(s: &mut State, id: &str) {
    let (Some(checkout), Some(key)) = (s.editor.checkout.clone(), s.editor.checkout_key.clone())
    else {
        return;
    };
    s.editor.history.detail_loading = true;
    if !s.editor.enqueue_read(
        checkout,
        key,
        Req::Diff {
            layer: "commit".into(),
            path: None,
            commit: Some(id.to_string()),
        },
    ) {
        s.editor.history.detail_loading = false;
    }
}

/// Select a commit and enqueue its detail diff — shared by the table, Jump to
/// HEAD, match navigation, jump_to, and commit_detail parent buttons.
pub(crate) fn select_commit_pub(s: &mut State, id: &str, tx: &mut Tx) {
    select_commit(s, id, tx);
}

fn select_commit(s: &mut State, id: &str, tx: &mut Tx) {
    if s.editor.history.selected.as_deref() == Some(id)
        && (s.editor.history.detail.is_some() || s.editor.history.detail_loading)
    {
        return;
    }
    s.editor.history.selected = Some(id.to_string());
    s.editor.history.detail = None;
    s.editor.history.table.select(id.to_string());
    let _ = tx;
    enqueue_commit_diff(s, id);
}

/// "Load more commits": enqueue the next page (`skip = next_skip`) for the
/// active branch filter; backend appends and refreshes `next_skip`.
fn load_more(s: &mut State) {
    let h = &s.editor.history;
    if h.loading_more {
        return;
    }
    let (Some(skip), Some(checkout), Some(key)) = (
        h.next_skip,
        s.editor.checkout.clone(),
        s.editor.checkout_key.clone(),
    ) else {
        return;
    };
    let branch = h.branch.clone();
    s.editor.history.loading_more = true;
    if !s
        .editor
        .enqueue_read(checkout, key, Req::History { skip, branch })
    {
        s.editor.history.loading_more = false;
    }
}

fn select_uncommitted(s: &mut State) {
    s.editor.history.selected = Some(UNCOMMITTED_ID.to_string());
    s.editor.history.detail = None;
    s.editor.history.detail_loading = false;
    s.editor.history.table.select(UNCOMMITTED_ID.to_string());
}

/// One-time seed of `TableState` column widths from a freshly loaded
/// `HistoryLayout` — called from the GetSetting reply arm so a restored
/// layout (including the graph override) lands in the table before the
/// first sync poll. Afterwards `column_width == None` only ever means
/// "auto or just reset", which is what lets double-click reset stick and
/// what stops a fresh table from wiping the stored graph width.
pub(crate) fn seed_columns_from_layout(h: &mut HistoryState) {
    let def = DEFAULT_LAYOUT;
    for (col_id, field) in [
        ("date", HistoryField::Date),
        ("author", HistoryField::Author),
        ("commit", HistoryField::Commit),
    ] {
        let px = layout_px(&h.layout, field);
        if (px - layout_px(&def, field)).abs() > f64::EPSILON {
            h.table.set_column_width(col_id, px as f32);
        }
    }
    match h.layout.graph {
        Some(px) => {
            h.table.set_column_width("graph", px as f32);
        }
        None => h.table.reset_column_width("graph"),
    }
}

/// Mirror rune-kit column widths ↔ persisted `HistoryLayout` (seeding
/// happens once in [`seed_columns_from_layout`] on the settings reply):
/// - date/author/commit: layout = effective width (user override or the
///   declared default); `column_width == None` means auto/reset.
/// - graph: override → `Some(px)`; no override → `None` (auto width).
fn sync_layout_from_table(s: &mut State) -> bool {
    let h = &mut s.editor.history;
    let def = DEFAULT_LAYOUT;
    let mut dirty = false;
    for (col_id, column) in [
        ("date", HistoryColumn::Date),
        ("author", HistoryColumn::Author),
        ("commit", HistoryColumn::Commit),
    ] {
        let field = column.field();
        let effective = h
            .table
            .column_width(col_id)
            .unwrap_or_else(|| layout_px(&def, field) as f32);
        let lim = field.limits();
        let px = (effective as f64).clamp(lim.min, lim.max.unwrap_or(f64::MAX));
        if (layout_px(&h.layout, field) - px).abs() > 0.5 {
            h.layout.set_column(column, px);
            dirty = true;
        }
    }
    match h.table.column_width("graph") {
        Some(w) => {
            let lim = HistoryColumn::Graph.limits();
            let px = (w as f64).clamp(lim.min, lim.max.unwrap_or(f64::MAX));
            if h.layout.graph != Some(px) {
                h.layout.graph = Some(px);
                dirty = true;
            }
        }
        None => {
            if h.layout.graph.take().is_some() {
                dirty = true;
            }
        }
    }
    if dirty {
        h.layout_dirty = true;
    }
    dirty
}

// ---------------------------------------------------------------------------
// Toolbar.
// ---------------------------------------------------------------------------

fn toolbar(s: &State, ink: &Ink, k: &Kit, layer: &mut Vec<El>) -> El {
    let mut bar = row()
        .w_full()
        .h(34.0)
        .px(8.0)
        .gap(6.0)
        .items_center()
        .border_sides([0.0, 0.0, 1.0, 0.0], ink.border);
    let h = &s.editor.history;

    // Branch combobox: "All branches" + locals, then remotes (desktop
    // optgroups flatten; remote display names already carry their remote
    // prefix). Option ids are ref names; `ALL_BRANCHES` stands for none.
    const ALL_BRANCHES: &str = "";
    let mut options = vec![rune_kit::ComboboxOption::new(ALL_BRANCHES, "All branches")];
    for remote in [false, true] {
        for b in h.branches.iter().filter(|b| b.remote == remote) {
            options.push(rune_kit::ComboboxOption::new(&b.ref_name, &b.name));
        }
    }
    let select = rune_kit::Combobox::new(
        "history-branch",
        &h.branch_picker,
        &options,
        |st: &mut State| &mut st.editor.history.branch_picker,
    )
    .current(Some(h.branch.as_deref().unwrap_or(ALL_BRANCHES)))
    .placeholder("All branches")
    .search_placeholder("Search branches")
    .width(160.0)
    .popover_width(crate::picker::list_width(160.0))
    .on_choose(move |st: &mut State, id: &str, _tx: &mut Tx| {
        let branch = (id != ALL_BRANCHES).then(|| id.to_string());
        if st.editor.history.branch == branch {
            return;
        }
        st.editor.history.branch = branch.clone();
        st.editor.history.commits.clear();
        st.editor.history.graph = None;
        st.editor.history.next_skip = None;
        st.editor.history.selected = None;
        st.editor.history.detail = None;
        st.editor.history.detail_loading = false;
        st.editor.history.filter.text.clear();
        st.editor.history.loading = true;
        st.editor.history.error = None;
        if let (Some(checkout), Some(key)) =
            (st.editor.checkout.clone(), st.editor.checkout_key.clone())
        {
            st.editor
                .enqueue_read(checkout, key, Req::History { skip: 0, branch });
        }
    });
    bar = bar.child(select.build(k, layer));

    // Detail position: Bottom stacks the pane under the table (desktop
    // layout), Right docks it beside the table. Same persisted lane as the
    // drag handles — `layout_dirty` arms `poll_layout_save`.
    bar = bar.child(detail_position_switch(h.layout.detail_position, ink));
    bar = bar.child(
        rune_kit::TextInput::new(&h.filter)
            .placeholder("Filter commits")
            // `None` fills the row like the desktop `flex-1` input; the
            // sibling controls keep their intrinsic widths.
            .width(None)
            .on_edit(|st: &mut State| -> &mut rune::edit::TextEdit {
                &mut st.editor.history.filter
            })
            .build(k)
            .shrink()
            .min_w(120.0),
    );

    if filter_active(s) {
        let n = match_ids(s).len();
        bar = bar.child(txt(
            &format!("{n} {}", if n == 1 { "match" } else { "matches" }),
            T12,
            ink.muted,
        ));
        bar = bar.child(
            btn_sm("↑", None, false, ink)
                .label("Previous match")
                .on_press_tx(|st: &mut State, tx: &mut Tx| jump_match(st, tx, -1)),
        );
        bar = bar.child(
            btn_sm("↓", None, false, ink)
                .label("Next match")
                .on_press_tx(|st: &mut State, tx: &mut Tx| jump_match(st, tx, 1)),
        );
    }

    // Jump to HEAD.
    if let Some(id) = h.commits.iter().find(|c| c.head).map(|c| c.id.clone()) {
        bar = bar.child(btn_sm("Jump to HEAD", None, false, ink).on_press_tx(
            move |st: &mut State, tx: &mut Tx| {
                st.editor.history.selected = Some(id.clone());
                st.editor.history.detail = None;
                st.editor.history.table.reveal(id.clone(), tx);
                enqueue_commit_diff(st, &id);
            },
        ));
    }

    bar = bar.child(
        btn_sm("Refresh", None, false, ink)
            .label("Refresh history")
            .on_press_tx(|st: &mut State, _tx: &mut Tx| {
                st.editor.refresh_active();
                if let Some(id) = st.editor.history.selected.clone()
                    && id != UNCOMMITTED_ID
                {
                    enqueue_commit_diff(st, &id);
                }
            }),
    );

    bar
}

/// Bottom / Right detail position segmented switch matching `chrome::view_switch`.
fn detail_position_switch(pos: DetailPosition, ink: &Ink) -> El {
    row()
        .key("history-detail-pos-switch")
        .role("radiogroup")
        .label("Detail position")
        .focus_group()
        .focus_follows()
        .h(24.0)
        .gap(2.0)
        .items_center()
        .padding(2.0)
        .radius(6.0)
        .border(1.0, ink.border)
        .fill(ink.bg)
        .child(detail_pos_item(
            "Bottom",
            DetailPosition::Bottom,
            pos == DetailPosition::Bottom,
            ink,
        ))
        .child(detail_pos_item(
            "Right",
            DetailPosition::Right,
            pos == DetailPosition::Right,
            ink,
        ))
}

fn detail_pos_item(label: &'static str, target: DetailPosition, active: bool, ink: &Ink) -> El {
    let tone = if active { ink.text } else { ink.muted };
    let hover_tone = ink.text;
    let mut b = row()
        .key(format!("history-detail-pos:{label}"))
        .h(20.0)
        .px(8.0)
        .items_center()
        .justify_center()
        .radius(4.0)
        .hover_group()
        .press_scale(0.96)
        .motion(STATE_SPRING)
        .role("radio")
        .label(label)
        .checked(active)
        .focusable(true)
        .focus_selected(active)
        .focus_ring(2.0, 1.0, ink.accent);
    if active {
        b = b.child(
            el().key("thumb")
                .absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .radius(4.0)
                .fill(ink.text.alpha(0.10))
                .matched("history-detail-pos-thumb"),
        );
    }
    b.child(
        txt(label, T11M_LH, tone)
            .key("label")
            .group_color(hover_tone),
    )
    .on_press_tx(move |st: &mut State, _tx: &mut Tx| {
        if st.editor.history.layout.detail_position != target {
            st.editor.history.layout.detail_position = target;
            st.editor.history.layout_dirty = true;
        }
    })
}

fn filter_active(s: &State) -> bool {
    !s.editor.history.filter.text.trim().is_empty()
}

fn match_ids(s: &State) -> HashSet<String> {
    let h = &s.editor.history;
    let q = h.filter.text.trim();
    let terms: Vec<String> = q
        .to_lowercase()
        .split_whitespace()
        .map(String::from)
        .collect();
    if terms.is_empty() {
        return HashSet::new();
    }
    h.commits
        .iter()
        .filter(|c| {
            let id_l = c.id.to_lowercase();
            let sub_l = c.subject.to_lowercase();
            let aut_l = c.author.to_lowercase();
            terms
                .iter()
                .all(|term| id_l.contains(term) || sub_l.contains(term) || aut_l.contains(term))
        })
        .map(|c| c.id.clone())
        .collect()
}

/// Port of `jumpToMatch`: circular walk from the selection to the next match.
fn jump_match(s: &mut State, tx: &mut Tx, direction: i64) {
    let matches = match_ids(s);
    if matches.is_empty() || s.editor.history.commits.is_empty() {
        return;
    }
    let commits = &s.editor.history.commits;
    let n = commits.len() as i64;
    let current = commits
        .iter()
        .position(|c| Some(&c.id) == s.editor.history.selected.as_ref());
    let start = match current {
        Some(i) => i as i64,
        None => {
            if direction == 1 {
                -1
            } else {
                n
            }
        }
    };
    for step in 1..=n {
        let idx = (start + direction * step).rem_euclid(n) as usize;
        let id = commits[idx].id.clone();
        if matches.contains(&id) {
            s.editor.history.selected = Some(id.clone());
            s.editor.history.detail = None;
            s.editor.history.table.reveal(id.clone(), tx);
            enqueue_commit_diff(s, &id);
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// Table.
// ---------------------------------------------------------------------------

fn natural_graph_width(lanes: usize) -> f32 {
    (lanes as f32 * LANE_W + GRAPH_PAD).clamp(
        HistoryColumn::Graph.limits().min as f32,
        HistoryColumn::Graph
            .limits()
            .max
            .map_or(f32::INFINITY, |v| v as f32),
    )
}

fn table_get(s: &State) -> &TableState {
    &s.editor.history.table
}
fn table_get_mut(s: &mut State) -> &mut TableState {
    &mut s.editor.history.table
}
fn show_load_more(s: &State) -> bool {
    s.editor.history.next_skip.is_some() || s.editor.history.loading_more
}

fn table_len(s: &State) -> usize {
    s.editor.history.commits.len()
        + usize::from(uncommitted_visible(s))
        + usize::from(show_load_more(s))
}

fn row_id_at(s: &State, i: usize) -> Cow<'_, str> {
    if show_load_more(s) && i + 1 == table_len(s) {
        return Cow::Borrowed(LOAD_MORE_ID);
    }
    if uncommitted_visible(s) {
        if i == 0 {
            return Cow::Borrowed(UNCOMMITTED_ID);
        }
        return Cow::Borrowed(s.editor.history.commits[i - 1].id.as_str());
    }
    Cow::Borrowed(s.editor.history.commits[i].id.as_str())
}

/// `changes.data` belongs to the current checkout (backend tags replies with
/// the checkout key; a stale payload must not paint the uncommitted row).
pub(crate) fn changes_fresh(s: &State) -> bool {
    s.editor.changes.data.is_some() && s.editor.changes.checkout == s.editor.checkout_key
}

/// Desktop rule: the uncommitted row shows when entries exist AND a HEAD commit
/// is loaded (the dashed connector needs a lane to land on).
fn uncommitted_visible(s: &State) -> bool {
    let Some(data) = changes_fresh(s)
        .then_some(s.editor.changes.data.as_ref())
        .flatten()
    else {
        return false;
    };
    let n = data.conflicts.len() + data.staged.len() + data.unstaged.len() + data.untracked.len();
    n > 0 && s.editor.history.commits.iter().any(|c| c.head)
}

fn uncommitted_count(s: &State) -> usize {
    let Some(data) = changes_fresh(s)
        .then_some(s.editor.changes.data.as_ref())
        .flatten()
    else {
        return 0;
    };
    data.conflicts.len() + data.staged.len() + data.unstaged.len() + data.untracked.len()
}

fn table_view(s: &State, ink: &Ink, k: &Kit) -> El {
    let h = &s.editor.history;
    let graph = h
        .graph
        .clone()
        .unwrap_or_else(|| std::sync::Arc::new(layout_graph(&h.commits)));
    let graph_w = h
        .layout
        .graph
        .map(|g| g as f32)
        .unwrap_or_else(|| natural_graph_width(graph.lanes));
    let matches = if filter_active(s) {
        Some(match_ids(s))
    } else {
        None
    };
    let now = now_ms();

    let lim = |c: HistoryColumn| c.limits();
    let (g, d, a, c) = (
        lim(HistoryColumn::Graph),
        lim(HistoryColumn::Date),
        lim(HistoryColumn::Author),
        lim(HistoryColumn::Commit),
    );

    // Desktop `HistoryTable` chrome: h-7 header/rows, px-2 cells, 11px
    // semibold muted uppercase titles, `bg-secondary/70` selection
    // (`--secondary` is `ink.panel`). "Graph" stays title-case: uppercase
    // "GRAPH" cannot fit the 48pt layout-contract minimum at 11px, while
    // "Graph" renders fully — the compact treatment the narrow column needs.
    let mut style = TableStyle::from_kit(k);
    style.header_height = ROW_H;
    style.cell_pad = CELL_PAD;
    style.header_font = k.ty.small_medium;
    style.selected = ink.panel.alpha(0.7);
    style.rule = ink.border;

    let cell_font = style.font;
    let cell_mono = style.mono;
    let mut t = Table::new(table_get, table_get_mut, table_len, row_id_at)
        .column(
            TableColumn::new("graph", "Graph")
                .width(graph_w)
                .min(g.min as f32)
                .max(g.max.map_or(f32::INFINITY, |v| v as f32)),
        )
        .column(
            TableColumn::new("desc", "DESCRIPTION")
                .flex(1.0)
                .min(200.0)
                .resizable(false),
        )
        .column(
            TableColumn::new("date", "DATE")
                .width(h.layout.date as f32)
                .min(d.min as f32)
                .max(d.max.map_or(f32::INFINITY, |v| v as f32)),
        )
        .column(
            TableColumn::new("author", "AUTHOR")
                .width(h.layout.author as f32)
                .min(a.min as f32)
                .max(a.max.map_or(f32::INFINITY, |v| v as f32)),
        )
        .column(
            TableColumn::new("commit", "COMMIT")
                .width(h.layout.commit as f32)
                .min(c.min as f32)
                .max(c.max.map_or(f32::INFINITY, |v| v as f32)),
        )
        .style(style)
        .name("Commit history");

    let ink_c = *ink;
    let ctx = CellCtx {
        matches,
        now,
        ink: ink_c,
        graph,
        graph_w,
        font: cell_font,
        mono: cell_mono,
    };
    t = t.cell(move |st: &State, i: usize, col_id: &str| cell_content(st, i, col_id, &ctx));
    t = t.on_select(|st: &mut State, id: &str, tx: &mut Tx| {
        if id == UNCOMMITTED_ID {
            select_uncommitted(st);
        } else if id != LOAD_MORE_ID {
            let id = id.to_string();
            select_commit(st, &id, tx);
        }
    });
    t = t.on_open(|st: &mut State, id: &str, tx: &mut Tx| {
        if id != UNCOMMITTED_ID && id != LOAD_MORE_ID {
            let id = id.to_string();
            select_commit(st, &id, tx);
        }
    });
    t.build(s, k)
}

/// Per-table render context shared by every cell: filter matches, clock,
/// theme ink, the computed lane graph and the graph column width.
struct CellCtx {
    matches: Option<HashSet<String>>,
    now: u64,
    ink: Ink,
    graph: std::sync::Arc<GraphLayout>,
    graph_w: f32,
    /// The table style's text faces, so `Cell::text` arms converted to
    /// `Cell::el` (needed to carry the row's context-menu handler) keep the
    /// exact kit styling: font, truncate, tooltip, shrink.
    font: TextStyle,
    mono: TextStyle,
}

fn cell_content(s: &State, i: usize, col_id: &str, ctx: &CellCtx) -> Cell {
    let CellCtx {
        matches,
        now,
        ink,
        graph,
        graph_w,
        ..
    } = ctx;
    let (now, graph_w) = (*now, *graph_w);
    let h = &s.editor.history;
    if show_load_more(s) && i + 1 == table_len(s) {
        return match col_id {
            "desc" => Cell::el(
                row().w_full().h_full().items_center().child(
                    btn_sm(
                        if h.loading_more {
                            "Loading…"
                        } else {
                            "Load more commits"
                        },
                        None,
                        true,
                        ink,
                    )
                    .w_full()
                    .disabled(h.loading_more)
                    .on_press_tx(|st: &mut State, _tx: &mut Tx| load_more(st)),
                ),
            ),
            _ => Cell::text(""),
        };
    }
    let uncommitted = uncommitted_visible(s);
    let ci = if uncommitted { i.wrapping_sub(1) } else { i };

    if uncommitted && i == 0 {
        return match col_id {
            "graph" => Cell::el(graph_cell_uncommitted(s, graph, graph_w)),
            "desc" => Cell::el(
                txt(
                    &format!("Uncommitted changes ({})", uncommitted_count(s)),
                    T12,
                    ink.muted,
                )
                .w_full()
                .truncate_middle(),
            ),
            _ => Cell::text(""),
        };
    }
    if ci >= h.commits.len() {
        return Cell::text("");
    }
    let c = &h.commits[ci];
    let dim = matches.as_ref().is_some_and(|m| !m.contains(&c.id));
    let color = |base: Color| if dim { base.alpha(0.4) } else { base };

    // Desktop wraps each commit `<li>` in a ContextMenu ("Copy hash" /
    // "Copy subject"); the kit `Table` has no per-row element hook, so every
    // cell of a commit row carries the same secondary-press handler that
    // stores the menu point + commit id for `history_view_impl`'s kit menu.
    // Uncommitted/load-more rows return above and stay menu-less, matching
    // main where only commit rows get the wrapper.
    let cid = c.id.clone();
    let inner: El = match col_id {
        "graph" => graph_cell(s, ci, dim, graph, graph_w),
        "desc" => {
            let paint = palette(graph_color_at(graph, ci));
            let mut content = row().w_full().h_full().items_center().gap(4.0).min_w(0.0);
            content = content.child(
                txt(
                    &c.subject,
                    T12,
                    color(if dim { ink.muted } else { ink.text }),
                )
                .min_w(96.0)
                .grow(1.0)
                .truncate_middle()
                .help_when_truncated(c.subject.clone()),
            );
            if c.head || !c.refs.is_empty() {
                let mut badges = row().items_center().gap(4.0).shrink();
                if c.head {
                    badges = badges.child(
                        el().px(6.0)
                            .h(20.0)
                            .radius(10.0)
                            .items_center()
                            .justify_center()
                            .fill(color(ink.accent))
                            .label("HEAD")
                            .child(txt("HEAD", T11MN_BADGE, ink.white)),
                    );
                }
                for r in &c.refs {
                    badges = badges.child(ref_badge_el(r, paint, ink, color));
                }
                content = content.child(badges);
            }
            content
        }
        "date" => cell_text(format_relative(c.time, now), false, color(ink.muted), ctx),
        "author" => cell_text(c.author.clone(), false, color(ink.text), ctx),
        "commit" => cell_text(
            c.id.chars().take(8).collect(),
            true,
            color(palette(graph_color_at(graph, ci))),
            ctx,
        ),
        _ => return Cell::text(""),
    };
    Cell::el(
        el().w_full()
            .h_full()
            .on_secondary_press(move |st: &mut State, p: rune::PointerPress, _tx: &mut Tx| {
                st.editor.history.row_menu = Some(HistoryRowMenu {
                    commit: cid.clone(),
                    x: p.x,
                    y: p.y,
                });
            })
            .child(inner),
    )
}

/// `Cell::text`'s exact styling as an `El`, for cells that must also carry
/// the row's context-menu handler (kit `Cell` exposes no handler surface):
/// same font face, truncate, shrink and truncated-text tooltip.
fn cell_text(t: String, mono: bool, color: Color, ctx: &CellCtx) -> El {
    txt(&t, if mono { ctx.mono } else { ctx.font }, color)
        .truncate_middle()
        .shrink()
        .min_w(0.0)
        .help_when_truncated(t)
}

/// Table ref badge: desktop colours every badge's text and border with the
/// row's lane `paint` (its Badge variant only changes the fill — `warning`
/// for tags, `outline` for remotes, `secondary` for locals).
fn ref_badge_el(r: &str, paint: Color, ink: &Ink, color: impl Fn(Color) -> Color) -> El {
    let (label, is_tag, is_remote) = ref_badge(r);
    // Desktop `ui/badge.tsx`: 20pt rounded-full pill (HistoryTable shrinks
    // only the font). The full ref name stays on `.label` for a11y/tooltips
    // while the visible text truncates.
    let badge = el()
        .px(6.0)
        .h(20.0)
        .radius(10.0)
        .items_center()
        .justify_center()
        .border(1.0, color(paint))
        .label(r)
        .child(
            txt(&label, T11MN_BADGE, color(paint))
                .max_w(112.0)
                .truncate_middle()
                .help_when_truncated(label),
        );
    if is_tag {
        badge.fill(color(ink.warning.alpha(0.15)))
    } else if is_remote {
        badge
    } else {
        badge.fill(color(paint.alpha(0.16)))
    }
}

fn graph_color_at(graph: &GraphLayout, ci: usize) -> usize {
    graph.rows.get(ci).map(|r| r.color).unwrap_or(0)
}

fn graph_cell(s: &State, ci: usize, dim: bool, graph: &GraphLayout, graph_w: f32) -> El {
    let Some(row) = graph.rows.get(ci) else {
        return el();
    };
    let head_index = graph.head_index;
    let uncommitted = uncommitted_visible(s);
    // Desktop paints the uncommitted connector under the row's own strokes.
    let mut segs = Vec::new();
    if uncommitted && let Some(hi) = head_index {
        segs.extend(connector_segments(
            ci,
            hi,
            graph.rows[hi].lane,
            graph.rows[hi].color,
        ));
    }
    segs.extend(row_segments(row));
    let opacity = if dim { 0.4 } else { 1.0 };
    // The kit pads every cell by `CELL_PAD`; the negative left margin pulls
    // the canvas back over that padding so the drawing spans the full column
    // width with desktop `GRAPH_PAD` geometry (the cell clips at its own
    // bounds, exactly the column edge).
    canvas(segs_to_shape(&segs, s.ink.bg, s.ink.muted, opacity))
        .size(graph_w, ROW_H)
        .ml(-CELL_PAD)
}

fn graph_cell_uncommitted(s: &State, graph: &GraphLayout, graph_w: f32) -> El {
    let head = graph.head_index.and_then(|hi| graph.rows.get(hi));
    let segs = uncommitted_segments(head.map(|r| r.lane), head.map(|r| r.color).unwrap_or(0));
    canvas(segs_to_shape(&segs, s.ink.bg, s.ink.muted, 1.0))
        .size(graph_w, ROW_H)
        .ml(-CELL_PAD)
}

// ---------------------------------------------------------------------------
// Detail pane.
// ---------------------------------------------------------------------------

fn right_diff_pane(s: &State, ink: &Ink, _k: &Kit) -> El {
    let h = &s.editor.history;
    let selected = h.selected.as_deref();

    if selected == Some(UNCOMMITTED_ID) {
        return patch::view(s, Pane::History).grow(1.0).h_full();
    }
    if selected
        .and_then(|id| h.commits.iter().find(|c| c.id == id))
        .is_none()
    {
        return empty_pane(ink, "Select a commit to see its patch", "");
    }
    if h.detail.is_some() {
        return patch::view(s, Pane::History).grow(1.0).h_full();
    }
    if h.detail_loading {
        return loading_pane("Loading commit…", ink);
    }
    empty_pane(ink, "Select a commit to see its patch", "")
}

fn bottom_detail_pane(s: &State, ink: &Ink, _k: &Kit) -> El {
    let h = &s.editor.history;
    let selected = h.selected.as_deref();

    if selected == Some(UNCOMMITTED_ID) {
        return row()
            .grow(1.0)
            .h_full()
            .min_h(0.0)
            .w_full()
            .child(
                col()
                    .w(380.0)
                    .h_full()
                    .shrink()
                    .min_w(0.0)
                    .border_sides([0.0, 1.0, 0.0, 0.0], ink.border)
                    .child(commit_detail::uncommitted_view(s, ink)),
            )
            .child(patch::view(s, Pane::History).grow(1.0).h_full());
    }
    if selected
        .and_then(|id| h.commits.iter().find(|c| c.id == id))
        .is_none()
    {
        return empty_pane(ink, "Select a commit to see its patch", "");
    }
    if h.detail.is_some() {
        return row()
            .grow(1.0)
            .h_full()
            .min_h(0.0)
            .w_full()
            .child(
                col()
                    .w(380.0)
                    .h_full()
                    .shrink()
                    .min_w(0.0)
                    .border_sides([0.0, 1.0, 0.0, 0.0], ink.border)
                    .child(commit_detail::view(s)),
            )
            .child(patch::view(s, Pane::History).grow(1.0).h_full());
    }
    if h.detail_loading {
        return loading_pane("Loading commit…", ink);
    }
    empty_pane(ink, "Select a commit to see its patch", "")
}

fn commit_detail_drawer_handle(ink: &Ink) -> El {
    el().key("detail-drawer-handle")
        .role("separator")
        .label("Resize commit details")
        .h(DETAIL_HANDLE_H)
        .w_full()
        .cursor(Cursor::ResizeRow)
        .hover(ink.border)
        .on_drag(
            |st: &mut State, d: &mut rune::Drag, _tx: &mut Tx| match d.phase {
                rune::Phase::Down => {
                    let (floor, dmax) = detail_height_bounds(&st.editor.history);
                    d.data = st.editor.history.layout.detail_height.clamp(floor, dmax) as f32;
                }
                rune::Phase::Move | rune::Phase::Up => {
                    if !d.cancelled() {
                        let (floor, dmax) = detail_height_bounds(&st.editor.history);
                        let v = (d.data - d.dy).clamp(floor as f32, dmax as f32);
                        st.editor.history.layout.set_detail_height(f64::from(v));
                        st.editor.history.layout_dirty = true;
                    }
                }
            },
        )
        .on_double_press(|st: &mut State| {
            st.editor.history.layout.reset(HistoryField::DetailHeight);
            st.editor.history.layout_dirty = true;
        })
}

/// Measured detail-pane height (0 before the first layout pass) — feeds
/// `commit_detail::meta_cap`'s metadata/patch split.
pub(crate) fn detail_pane_height(s: &State) -> f32 {
    s.editor.history.measured_detail_height
}

/// `(render_floor, dmax)` for the detail pane from the measured container —
/// the single source for the view's render clamp AND the handle's drag clamp.
/// `dmax` is `f64::MAX`-ish while the root height is unmeasured so the first
/// frames still lay out.
fn detail_height_bounds(h: &HistoryState) -> (f64, f64) {
    let container_h = h.measured_root_height;
    let stale_h = if h.error.is_some() && !h.commits.is_empty() {
        STALE_H as f64
    } else {
        0.0
    };
    let chrome_h = TOOLBAR_H as f64 + stale_h;
    let dmax = if container_h <= 0.0 {
        f64::from(u32::MAX)
    } else {
        (container_h as f64 - chrome_h - TABLE_MIN_H as f64 - DETAIL_HANDLE_H as f64).max(0.0)
    };
    // Floor: the comfortable 160 whenever the viewport can fit it — which
    // also reclaims sub-160 persisted values back to the desktop minimum on
    // large windows. Below that (760×480 minimum) yield to the patch floor
    // so the pane stays usable and viewport-forced sub-160 values render.
    let floor = if dmax >= DETAIL_MIN {
        DETAIL_MIN
    } else {
        (PATCH_MIN_H as f64).min(dmax)
    };
    (floor, dmax)
}

fn detail_handle(ink: &Ink) -> El {
    el().key("detail-handle")
        .role("separator")
        .label("Resize history table and detail")
        .w_full()
        .h(DETAIL_HANDLE_H)
        .cursor(Cursor::ResizeRow)
        .hover(ink.border)
        .on_drag(|st: &mut State, d: &mut rune::Drag, _tx: &mut Tx| {
            match d.phase {
                rune::Phase::Down => {
                    // Start from the EFFECTIVE rendered height, not the raw
                    // persisted value: an oversized stored height renders at
                    // `dmax`, and dragging must move the visible boundary —
                    // otherwise every move writes values that stay clamped
                    // and the handle feels dead.
                    let (floor, dmax) = detail_height_bounds(&st.editor.history);
                    d.data = st.editor.history.layout.detail_height.clamp(floor, dmax) as f32;
                }
                rune::Phase::Move | rune::Phase::Up => {
                    if !d.cancelled() {
                        // Detail sits below the handle: dragging down shrinks it.
                        // Persist the value bounded by the measured viewport so
                        // the stored model matches what the user can see. The
                        // drag floor is the comfortable 160 — but when the
                        // window is too short to fit it (`dmax < 160`,
                        // reachable at the 760×480 minimum), the render yields
                        // to `floor` and the drag must persist those effective
                        // values too or the handle is dead exactly where
                        // it's needed. The model floor (PATCH_MIN_H) accepts
                        // them, so `set_detail_height` stays the write path.
                        let (floor, dmax) = detail_height_bounds(&st.editor.history);
                        let (lo, hi) = if dmax < DETAIL_MIN {
                            (floor as f32, dmax as f32)
                        } else {
                            (DETAIL_MIN as f32, dmax.max(DETAIL_MIN) as f32)
                        };
                        let v = (d.data - d.dy).clamp(lo, hi);
                        st.editor.history.layout.set_detail_height(f64::from(v));
                        st.editor.history.layout_dirty = true;
                    }
                }
            }
        })
        .on_double_press(|st: &mut State| {
            st.editor.history.layout.reset(HistoryField::DetailHeight);
            st.editor.history.layout_dirty = true;
        })
}

/// `(render_floor, wmax)` for a Right-position detail pane from the measured
/// root width — the horizontal mirror of [`detail_height_bounds`], and the
/// single source for the view's render clamp AND the handle's drag clamp.
/// `wmax` is `u32::MAX`-ish while the root width is unmeasured so the first
/// frames still lay out; the floor yields below the 280 model minimum on
/// windows too narrow to fit it beside [`TABLE_MIN_W`] — the detail
/// compresses first, the table never does.
fn detail_width_bounds(h: &HistoryState) -> (f64, f64) {
    let container_w = h.measured_root_width;
    let wmax = if container_w <= 0.0 {
        f64::from(u32::MAX)
    } else {
        (container_w as f64 - TABLE_MIN_W as f64 - DETAIL_HANDLE_W as f64).max(0.0)
    };
    (HistoryField::DetailWidth.limits().min.min(wmax), wmax)
}

/// The vertical counterpart of [`detail_handle`]: an 8pt strip between the
/// table and a Right-position detail. Dragging left grows the detail
/// (`data - dx`), double-press restores the default width.
fn detail_width_handle(ink: &Ink) -> El {
    el().key("detail-width-handle")
        .role("separator")
        .label("Resize history detail width")
        .w(DETAIL_HANDLE_W)
        .h_full()
        .cursor(Cursor::ResizeColumn)
        .hover(ink.border)
        .on_drag(|st: &mut State, d: &mut rune::Drag, _tx: &mut Tx| {
            match d.phase {
                rune::Phase::Down => {
                    // Start from the EFFECTIVE rendered width, like the
                    // height handle: an oversized stored width renders at
                    // `wmax`, and dragging must move the visible boundary.
                    let (floor, wmax) = detail_width_bounds(&st.editor.history);
                    d.data = st.editor.history.layout.detail_width.clamp(floor, wmax) as f32;
                }
                rune::Phase::Move | rune::Phase::Up => {
                    if !d.cancelled() {
                        let (floor, wmax) = detail_width_bounds(&st.editor.history);
                        let v = (d.data - d.dx).clamp(floor as f32, wmax as f32);
                        st.editor.history.layout.set_detail_width(f64::from(v));
                        st.editor.history.layout_dirty = true;
                    }
                }
            }
        })
        .on_double_press(|st: &mut State| {
            st.editor.history.layout.reset(HistoryField::DetailWidth);
            st.editor.history.layout_dirty = true;
        })
}

// ---------------------------------------------------------------------------
// Shared pane bits.
// ---------------------------------------------------------------------------

pub(crate) fn loading_pane(label: &str, ink: &Ink) -> El {
    col()
        .grow(1.0)
        .items_center()
        .justify_center()
        .child(txt(label, T12, ink.muted))
}

pub(crate) fn stale_notice(err: &str, ink: &Ink) -> El {
    row()
        .w_full()
        .px(12.0)
        .py(6.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], ink.border)
        .fill(ink.warning.alpha(0.08))
        .child(txt(
            &format!("Refresh failed — showing the last result. {err}"),
            T12,
            ink.warning,
        ))
}

pub(crate) fn error_pane(err: &str, ink: &Ink) -> El {
    col()
        .grow(1.0)
        .items_center()
        .justify_center()
        .gap(8.0)
        .child(txt(err, T12, ink.danger))
        .child(
            btn_sm("Retry", None, true, ink).on_press_tx(|st: &mut State, _tx: &mut Tx| {
                st.editor.refresh_active();
            }),
        )
}

pub(crate) fn empty_pane(ink: &Ink, title: &str, body: &str) -> El {
    let mut c = col()
        .grow(1.0)
        .items_center()
        .justify_center()
        .gap(6.0)
        .px(24.0);
    c = c.child(txt(title, T12M, ink.text));
    if !body.is_empty() {
        c = c.child(txt(body, T12, ink.muted));
    }
    c
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Tests — pure geometry / helpers only (no Rune runtime needed).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use app_core::git_view::GitViewCommit;

    fn commit(id: &str, parents: &[&str], head: bool) -> GitViewCommit {
        GitViewCommit {
            id: id.into(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            refs: vec![],
            head,
            author: "A".into(),
            time: 0,
            subject: "s".into(),
        }
    }

    #[test]
    fn lane_x_matches_desktop_formula() {
        assert_eq!(lane_x(0), 15.0);
        assert_eq!(lane_x(1), 29.0);
        assert_eq!(lane_x(3), 57.0);
    }

    #[test]
    fn linear_chain_edge_and_nodes() {
        // b (HEAD, index 0) → a (index 1): straight same-lane edge on row 0.
        let commits = vec![commit("b", &["a"], true), commit("a", &[], false)];
        let g = layout_graph(&commits);
        let segs = row_segments(&g.rows[0]);
        assert!(
            segs.iter().any(|s| matches!(s, Seg::Line { .. })),
            "edge line"
        );
        assert!(
            segs.iter().any(|s| matches!(s, Seg::Disc { r: 5.0, .. })),
            "HEAD ring"
        );
        assert!(!segs.iter().any(|s| matches!(s, Seg::Cubic { .. })));
        // Root row: node only, no edges.
        let root = row_segments(&g.rows[1]);
        assert_eq!(
            root.iter()
                .filter(|s| matches!(s, Seg::Disc { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn merge_row_emits_cubic_and_both_nodes() {
        // m (HEAD) merges p1 (lane continues) and p2 (second lane).
        let commits = vec![
            commit("m", &["p1", "p2"], true),
            commit("p1", &["base"], false),
            commit("base", &[], false),
            commit("p2", &["base"], false),
        ];
        let g = layout_graph(&commits);
        let head = &g.rows[0];
        assert!(head.merge);
        let segs = row_segments(head);
        assert!(
            segs.iter().any(|s| matches!(s, Seg::Cubic { .. })),
            "second parent curves"
        );
        assert!(
            segs.iter().any(|s| matches!(s, Seg::Disc { r: 5.0, .. })),
            "HEAD ring"
        );
        // Merge centre dot.
        assert!(segs.iter().any(|s| matches!(s, Seg::Disc { r: 1.5, .. })));
    }

    #[test]
    fn connector_covers_rows_above_and_at_head() {
        // HEAD at index 2: rows 0,1 span the full band — dashes are
        // phase-locked, so coverage ends at the last on-interval before the
        // trailing gap (≤3pt short of ROW_H); row 2 top half only; row 3 none.
        let full = connector_segments(0, 2, 0, 0);
        assert!(!full.is_empty());
        let min_y = full
            .iter()
            .map(|s| match s {
                Seg::Line { y0, .. } => *y0,
                _ => f32::MAX,
            })
            .fold(f32::MAX, f32::min);
        let max_y = full
            .iter()
            .map(|s| match s {
                Seg::Line { y1, .. } => *y1,
                _ => 0.0,
            })
            .fold(0.0f32, f32::max);
        assert_eq!(min_y, 0.0, "first dash starts at band top");
        assert!(
            max_y >= ROW_H - 3.0,
            "dashes reach the band's trailing gap, got {max_y}"
        );
        let head = connector_segments(2, 2, 0, 0);
        let hmin = head
            .iter()
            .map(|s| match s {
                Seg::Line { y0, .. } => *y0,
                _ => f32::MAX,
            })
            .fold(f32::MAX, f32::min);
        let hmax = head
            .iter()
            .map(|s| match s {
                Seg::Line { y1, .. } => *y1,
                _ => 0.0,
            })
            .fold(0.0f32, f32::max);
        // Covers the top half modulo the phase-locked trailing gap (≤3pt).
        assert_eq!(hmin, 0.0, "HEAD connector starts at band top");
        assert!(
            hmax >= ROW_H / 2.0 - 3.0,
            "HEAD row reaches centre minus trailing gap, got {hmax}"
        );
        assert!(connector_segments(3, 2, 0, 0).is_empty());
    }

    #[test]
    fn uncommitted_row_has_dot_and_dashed_tail() {
        let segs = uncommitted_segments(Some(0), 2);
        assert!(segs.iter().any(|s| matches!(s, Seg::Disc { .. })));
        let lines: Vec<_> = segs
            .iter()
            .filter_map(|s| match s {
                Seg::Line { y0, y1, .. } => Some(y1 - y0),
                _ => None,
            })
            .collect();
        assert!(lines.len() >= 2, "dash splits the connector: {lines:?}");
        assert!(
            lines.iter().all(|&d| d <= 4.01),
            "each dash <= 4pt: {lines:?}"
        );
    }

    #[test]
    fn dashed_v_is_phase_continuous_across_rows() {
        // Row band [28,56) continuing a dash that started in [0,28): the first
        // emitted segment must not restart at y=0 of the period blindly —
        // global phase decides. Band top global=28 → pos=0 → on-interval.
        let segs = dashed_v(15.0, 0.0, ROW_H, 28.0, Paint::Palette(0));
        assert!(!segs.is_empty());
        // At global y=28 (pos 0) an on-interval begins → first dash starts at local 0.
        if let Seg::Line { y0, .. } = segs[0] {
            assert_eq!(y0, 0.0);
        }
        // Band [56,84): global pos = 0 again → same pattern.
        let segs2 = dashed_v(15.0, 0.0, ROW_H, 56.0, Paint::Palette(0));
        assert_eq!(
            segs.len(),
            segs2.len(),
            "period 7 vs band 28 → same count per band"
        );
    }

    #[test]
    fn matches_query_requires_all_terms() {
        assert!(matches_query("abc123 fix bug", "ABC fix"));
        assert!(!matches_query("abc123 fix bug", "abc missing"));
        assert!(matches_query("anything", "   "));
        // Every whitespace term must appear as a substring.
        assert!(!matches_query("abc", "a x"));
        assert!(matches_query("abc", "a b"));
    }

    #[test]
    fn format_relative_boundaries() {
        let now = 1_700_000_000_000u64;
        assert_eq!(format_relative(now, now), "just now");
        assert_eq!(format_relative(now - 30_000, now), "just now");
        assert_eq!(format_relative(now - 61_000, now), "1 minute ago");
        // Desktop Math.round semantics: 91s rounds up to 2 minutes.
        assert_eq!(format_relative(now - 91_000, now), "2 minutes ago");
        assert_eq!(format_relative(now - 3 * 3_600_000, now), "3 hours ago");
        assert_eq!(format_relative(now - 86_400_000, now), "yesterday");
        assert_eq!(format_relative(now - 5 * 86_400_000, now), "5 days ago");
        assert!(
            format_relative(now - 40 * 86_400_000, now).contains(','),
            "old → absolute date"
        );
        // Old-date fallback agrees with the local-zone formatter.
        let expected = local_dt(now - 40 * 86_400_000)
            .unwrap()
            .format("%b %-d, %Y")
            .to_string();
        assert_eq!(format_relative(now - 40 * 86_400_000, now), expected);
    }

    #[test]
    fn format_time_uses_local_zone() {
        // 1_700_000_000_000 = 2023-11-14T22:13:20Z; every real timezone lands
        // on Nov 14–15, 2023, so these hold without touching OS preferences.
        let s = format_time(1_700_000_000_000);
        assert!(s.contains("2023") && s.contains("Nov"), "{s}");
        // And it agrees exactly with chrono's local conversion.
        let expected = Local
            .timestamp_millis_opt(1_700_000_000_000)
            .single()
            .unwrap()
            .format("%b %-d, %Y, %-I:%M %p")
            .to_string();
        assert_eq!(s, expected);
    }

    #[test]
    fn format_time_invalid_renders_dash() {
        assert_eq!(format_time(u64::MAX), "-");
        // i64-valid but beyond chrono's ~±262k-year range.
        assert_eq!(format_time(9_000_000_000_000_000), "-");
    }

    #[test]
    fn ref_badge_variants() {
        assert_eq!(ref_badge("tag: v1.0"), ("v1.0".into(), true, false));
        assert_eq!(
            ref_badge("remotes/origin/main"),
            ("origin/main".into(), false, true)
        );
        assert_eq!(ref_badge("main"), ("main".into(), false, false));
    }

    #[test]
    fn uncommitted_id_cannot_collide_with_oid() {
        assert!(!UNCOMMITTED_ID.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn layout_px_maps_fields() {
        let l = HistoryLayout::default();
        assert!(layout_px(&l, HistoryField::Graph).is_nan());
        assert_eq!(layout_px(&l, HistoryField::Date), 120.0);
        assert_eq!(layout_px(&l, HistoryField::DetailHeight), 320.0);
    }

    #[test]
    fn natural_graph_width_clamps() {
        assert_eq!(natural_graph_width(0), 48.0);
        assert_eq!(natural_graph_width(1), 48.0);
        assert_eq!(natural_graph_width(30), 320.0);
    }

    #[test]
    fn palette_converts_srgb_to_linear() {
        let c = palette(0); // #e23636
        assert!(c.r > 0.7 && c.r < 0.85, "{:?}", c.r);
    }

    // -- poll(): housekeeping that on_layout_tx cannot drive ----------------

    use crate::editor::state::checkout_key;
    use std::path::PathBuf;

    fn state_on_history() -> State {
        let mut s = State::new(crate::test_handle());
        s.editor.section = Section::History;
        s
    }

    #[test]
    fn poll_auto_selects_head_with_stable_rect() {
        // Commits "arrive" while the root rect never changes — poll must pick
        // the selection anyway (previously dead in on_layout_tx).
        let mut s = state_on_history();
        s.editor.history.commits = vec![commit("b", &["a"], true), commit("a", &[], false)];
        let mut tx = Tx::default();
        assert!(poll(&mut s, &mut tx), "first poll selects the HEAD commit");
        assert_eq!(s.editor.history.selected.as_deref(), Some("b"));
        assert!(!poll(&mut s, &mut tx), "converged — no further changes");
    }

    #[test]
    fn poll_consumes_jump_to() {
        let mut s = state_on_history();
        s.editor.history.commits = vec![commit("b", &["a"], true), commit("a", &[], false)];
        s.editor.history.selected = Some("b".into());
        s.editor.history.jump_to = Some("a".into());
        let mut tx = Tx::default();
        assert!(poll(&mut s, &mut tx));
        assert_eq!(s.editor.history.selected.as_deref(), Some("a"));
        assert!(s.editor.history.jump_to.is_none());
        assert!(!poll(&mut s, &mut tx));
    }

    #[test]
    fn poll_persists_column_resize_without_window_resize() {
        // A TableState width drag lands in `table.widths`; poll folds it into
        // HistoryLayout and flags layout_dirty — no relayout needed.
        let mut s = state_on_history();
        s.editor.history.table.set_column_width("date", 300.0);
        let mut tx = Tx::default();
        assert!(poll(&mut s, &mut tx));
        assert_eq!(s.editor.history.layout.date, 300.0);
        assert!(s.editor.history.layout_dirty);
        assert!(
            !poll(&mut s, &mut tx),
            "widths already mirrored — converged"
        );
    }

    #[test]
    fn poll_is_inert_off_history() {
        let mut s = State::new(crate::test_handle());
        s.editor.section = Section::Changes;
        s.editor.history.commits = vec![commit("b", &["a"], true)];
        let mut tx = Tx::default();
        assert!(!poll(&mut s, &mut tx));
        assert!(s.editor.history.selected.is_none());
    }

    #[test]
    fn poll_kicks_changes_read_once_for_uncommitted_row() {
        let mut s = state_on_history();
        let checkout = PathBuf::from("/tmp/co");
        s.editor.project_id = Some("p1".into());
        s.editor.checkout = Some(checkout.clone());
        s.editor.checkout_key = Some(checkout_key("p1", &checkout));
        let mut tx = Tx::default();
        assert!(poll(&mut s, &mut tx), "kicks Req::Changes once");
        assert!(s.editor.changes.loading);
        assert!(!poll(&mut s, &mut tx), "in-flight read is not re-kicked");
    }

    // -- Runtime: the filter field must actually edit -------------------------

    /// Restores `GOKO_HOME` on drop so the throwaway home never leaks into
    /// parallel tests (same guard the changes-scene tests use).
    struct HomeGuard(Option<std::ffi::OsString>);

    impl HomeGuard {
        fn set(path: &std::path::Path) -> Self {
            let prev = std::env::var_os("GOKO_HOME");
            // SAFETY: test-only; the pointed-at directory outlives the test.
            unsafe { std::env::set_var("GOKO_HOME", path) };
            Self(prev)
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            // SAFETY: same test-only env window as `set`.
            unsafe {
                match &self.0 {
                    Some(v) => std::env::set_var("GOKO_HOME", v),
                    None => std::env::remove_var("GOKO_HOME"),
                }
            }
        }
    }

    /// CUA repro for the inert filter: `TextInput::new` without `.on_edit`
    /// left `history.filter` read-only — typing changed nothing and rows
    /// never filtered. Click the field, type, and the parent `TextEdit` plus
    /// the toolbar match count must follow; backspacing clears it.
    #[test]
    fn filter_input_types_and_filters_commit_rows() {
        use crate::View;
        use rune::{Env, Runtime};

        let home = app_core::test_support::TempDir::new("history-filter-qa");
        let _guard = HomeGuard::set(home.path());
        let mut s = State::new(crate::test_handle());
        s.view = View::Editor;
        s.editor.project_id = Some("p1".into());
        let co = PathBuf::from("/tmp/goko-qa-checkout");
        s.editor.checkout_key = Some(checkout_key("p1", &co));
        s.editor.checkout = Some(co);
        s.editor.section = Section::History;
        let mk = |id: &str, subject: &str, head: bool| {
            let mut c = commit(id, &[], head);
            c.subject = subject.into();
            c
        };
        s.editor.history.commits = vec![
            mk(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "Merge feature into main",
                true,
            ),
            mk(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "Fix login redirect bug",
                false,
            ),
            mk(
                "cccccccccccccccccccccccccccccccccccccccc",
                "Add filter tests",
                false,
            ),
        ];
        let mut rt = Runtime::new(s, crate::view, (900.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);

        // The only textbox in the History view is the commit filter.
        let tb = rt
            .elements()
            .into_iter()
            .filter(|e| e.role == "textbox")
            .min_by(|a, b| a.rect[1].total_cmp(&b.rect[1]))
            .expect("filter textbox mounted");
        let r = tb.rect;
        let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);

        for ch in "merge".chars() {
            rt.key(rune::KeyEvent {
                key: rune::Key::Text(ch.to_string()),
                mods: rune::Mods::default(),
            });
        }
        rt.frame(1.0 / 60.0);
        assert_eq!(
            rt.state.editor.history.filter.text, "merge",
            "typing must reach history.filter"
        );
        assert!(
            rt.elements()
                .iter()
                .any(|e| e.text.as_deref() == Some("1 match")),
            "match count must show while the filter is active"
        );

        for _ in 0..5 {
            rt.key(rune::KeyEvent {
                key: rune::Key::Backspace,
                mods: rune::Mods::default(),
            });
        }
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.editor.history.filter.text, "");
        assert!(
            !rt.elements()
                .iter()
                .any(|e| e.text.as_deref() == Some("1 match")),
            "match count hides once the filter clears"
        );
    }

    /// Desktop `HistoryTable` wraps each commit row in a ContextMenu with
    /// "Copy hash" / "Copy subject". Secondary-pressing a row's cell must
    /// open the kit menu at the press point; picking an item clears the
    /// menu state (the `tx.copy` write itself is platform-side).
    #[test]
    fn row_context_menu_offers_copy_actions() {
        use crate::View;
        use rune::{Env, Runtime};

        let home = app_core::test_support::TempDir::new("history-menu-qa");
        let _guard = HomeGuard::set(home.path());
        let mut s = State::new(crate::test_handle());
        s.view = View::Editor;
        s.editor.project_id = Some("p1".into());
        let co = PathBuf::from("/tmp/goko-qa-checkout");
        s.editor.checkout_key = Some(checkout_key("p1", &co));
        s.editor.checkout = Some(co);
        s.editor.section = Section::History;
        let mut head = commit("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", &[], true);
        head.subject = "Merge feature into main".into();
        let mut other = commit(
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            &["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],
            false,
        );
        other.subject = "Fix login redirect bug".into();
        s.editor.history.commits = vec![head, other];

        let mut rt = Runtime::new(s, crate::view, (900.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        rt.frame(1.0 / 60.0);

        // Secondary-press the second row's subject cell.
        let subj = rt
            .elements()
            .into_iter()
            .find(|e| !e.exiting && e.text.as_deref() == Some("Fix login redirect bug"))
            .expect("second row subject mounted");
        let r = subj.rect;
        let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.secondary_down(x, y);
        rt.secondary_up(x, y);
        rt.frame(1.0 / 60.0);

        let menu = rt
            .state
            .editor
            .history
            .row_menu
            .as_ref()
            .expect("secondary press opens the row menu");
        assert_eq!(menu.commit, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");

        // The kit menu mounts both desktop items.
        rt.frame(1.0 / 60.0);
        for label in ["Copy hash", "Copy subject"] {
            assert!(
                rt.elements()
                    .iter()
                    .any(|e| e.text.as_deref() == Some(label)),
                "menu item {label:?} missing"
            );
        }

        // Picking "Copy hash" closes the menu.
        let item = rt
            .elements()
            .into_iter()
            .find(|e| !e.exiting && e.text.as_deref() == Some("Copy hash"))
            .expect("Copy hash item");
        let r = item.rect;
        let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert!(rt.state.editor.history.row_menu.is_none());
    }

    // -- detail-handle drag regression ------------------------------------

    /// History scene with commits and NO detail selected: the handle strip
    /// between table and detail is mounted either way.
    fn handle_scene(detail_height: f64) -> State {
        use crate::View;

        let mut s = State::new(crate::test_handle());
        s.view = View::Editor;
        s.editor.project_id = Some("p1".into());
        let co = PathBuf::from("/tmp/goko-qa-checkout");
        s.editor.checkout_key = Some(checkout_key("p1", &co));
        s.editor.checkout = Some(co);
        s.editor.section = Section::History;
        s.editor.history.commits = vec![
            commit("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", &[], true),
            commit("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", &[], false),
        ];
        s.editor.history.layout.set_detail_height(detail_height);
        s.editor.history.layout.detail_position = DetailPosition::Bottom;
        s
    }

    fn detail_grip(rt: &rune::Runtime<State>) -> rune::ElementInfo {
        rt.elements()
            .into_iter()
            .find(|e| {
                e.role == "separator"
                    && e.label
                        .as_deref()
                        .is_some_and(|l| l.contains("history table and detail"))
            })
            .expect("detail resize handle mounted")
    }

    fn drag_y(rt: &mut rune::Runtime<State>, x: f32, y0: f32, y1: f32) {
        rt.pointer_move(x, y0);
        rt.pointer_down(x, y0);
        rt.frame(1.0 / 60.0);
        for i in 1..=4 {
            rt.pointer_move(x, y0 + (y1 - y0) * i as f32 / 4.0);
            rt.frame(1.0 / 60.0);
        }
        rt.pointer_up(x, y1);
        rt.frame(1.0 / 60.0);
    }

    /// Frame until the detail boundary stops moving. The first frames can
    /// render a different `detail_h` (unmeasured root → 160 floor) and rune's
    /// layout spring then animates the change, so a fixed 4-frame wait can
    /// catch a mid-flight height (e.g. 140 on the way to 97).
    fn settle_detail(rt: &mut rune::Runtime<State>) {
        let mut last = f32::NAN;
        let mut stable = 0;
        for _ in 0..180 {
            rt.frame(1.0 / 60.0);
            let y = detail_grip(rt).rect[1];
            if (y - last).abs() < 0.25 {
                stable += 1;
                if stable >= 3 {
                    return;
                }
            } else {
                stable = 0;
            }
            last = y;
        }
    }

    /// CUA repro: with a persisted detail height far above the viewport
    /// clamp, BOTH drag directions used to write raw values that never
    /// moved the rendered boundary — the handle felt dead. The drag must
    /// start from the effective (clamped) height and stay inside the
    /// viewport bound so the boundary moves on the first gesture.
    #[test]
    fn detail_handle_drag_snaps_oversized_height_into_view() {
        use rune::{Env, Runtime};

        let home = app_core::test_support::TempDir::new("history-pane-oversize");
        let _guard = HomeGuard::set(home.path());
        let s = handle_scene(5000.0);
        let mut rt = Runtime::new(s, crate::view, (900.0, 700.0), Env::new(true, 2.0));
        for _ in 0..4 {
            rt.frame(1.0 / 60.0);
        }

        let container = rt.state.editor.history.measured_root_height as f64;
        assert!(container > 0.0, "root height measured");
        let dmax = container - TOOLBAR_H as f64 - TABLE_MIN_H as f64 - DETAIL_HANDLE_H as f64;
        assert!(dmax > 0.0 && dmax < 5000.0, "dmax={dmax} bounded");

        // Rendered boundary: the handle's y sits below the grown table.
        let g = detail_grip(&rt);
        let y_before = g.rect[1];
        let (x, y) = (g.rect[0] + g.rect[2] / 2.0, y_before + g.rect[3] / 2.0);

        // Drag DOWN 120px: detail must shrink, table grows, handle moves down.
        drag_y(&mut rt, x, y, y + 120.0);
        let persisted = rt.state.editor.history.layout.detail_height;
        assert!(
            persisted <= dmax + 1.0,
            "persisted {persisted} snapped into viewport bound {dmax}"
        );
        let y_after = detail_grip(&rt).rect[1];
        assert!(
            (y_after - y_before - 120.0).abs() < 2.0,
            "handle boundary moved with the drag: {y_before} -> {y_after}"
        );

        // Drag back UP 60px: detail grows again — responsive both ways.
        let g = detail_grip(&rt);
        let (x, y) = (g.rect[0] + g.rect[2] / 2.0, g.rect[1] + g.rect[3] / 2.0);
        let y_mid = g.rect[1];
        drag_y(&mut rt, x, y, y - 60.0);
        let persisted = rt.state.editor.history.layout.detail_height;
        assert!(
            persisted > 160.0 && persisted <= dmax + 1.0,
            "persisted {persisted} inside (160, {dmax}]"
        );
        let y_up = detail_grip(&rt).rect[1];
        assert!(
            (y_mid - y_up - 60.0).abs() < 2.0,
            "handle boundary moved up with the drag: {y_mid} -> {y_up}"
        );
    }

    /// Normal-range drags keep working: default 320 shrinks/grows with the
    /// pointer and the persisted model tracks it.
    #[test]
    fn detail_handle_drag_in_normal_range() {
        use rune::{Env, Runtime};

        let home = app_core::test_support::TempDir::new("history-pane-normal");
        let _guard = HomeGuard::set(home.path());
        let s = handle_scene(320.0);
        let mut rt = Runtime::new(s, crate::view, (900.0, 700.0), Env::new(true, 2.0));
        for _ in 0..4 {
            rt.frame(1.0 / 60.0);
        }
        let g = detail_grip(&rt);
        let y_before = g.rect[1];
        let (x, y) = (g.rect[0] + g.rect[2] / 2.0, y_before + g.rect[3] / 2.0);
        drag_y(&mut rt, x, y, y + 60.0);
        let persisted = rt.state.editor.history.layout.detail_height;
        assert!(
            (persisted - 260.0).abs() < 2.0,
            "320 - 60 drag -> {persisted}"
        );
        assert!(rt.state.editor.history.layout_dirty);
        let y_after = detail_grip(&rt).rect[1];
        assert!(
            (y_after - y_before - 60.0).abs() < 2.0,
            "boundary followed drag: {y_before} -> {y_after}"
        );
    }

    /// A short window still yields: oversized persisted height renders at
    /// the clamped maximum and the first drag snaps it into range instead
    /// of writing invisible values. (Detail already sits at dmax, so the
    /// responsive direction here is DOWN — shrinking it.)
    #[test]
    fn detail_handle_short_window_still_drags() {
        use rune::{Env, Runtime};

        let home = app_core::test_support::TempDir::new("history-pane-short");
        let _guard = HomeGuard::set(home.path());
        let s = handle_scene(5000.0);
        let mut rt = Runtime::new(s, crate::view, (760.0, 480.0), Env::new(true, 2.0));
        for _ in 0..4 {
            rt.frame(1.0 / 60.0);
        }
        let g = detail_grip(&rt);
        let y_before = g.rect[1];
        let (x, y) = (g.rect[0] + g.rect[2] / 2.0, y_before + g.rect[3] / 2.0);
        drag_y(&mut rt, x, y, y + 40.0);
        let persisted = rt.state.editor.history.layout.detail_height;
        let container = rt.state.editor.history.measured_root_height as f64;
        let dmax = container - TOOLBAR_H as f64 - TABLE_MIN_H as f64 - DETAIL_HANDLE_H as f64;
        assert!(
            persisted <= dmax.max(160.0) + 1.0,
            "persisted {persisted} within short-window bound"
        );
        let y_after = detail_grip(&rt).rect[1];
        assert!(
            (y_after - y_before - 40.0).abs() < 2.0,
            "short-window boundary moved: {y_before} -> {y_after}"
        );
    }

    /// A selected commit mounts the real detail branch (metadata + patch)
    /// inside the fixed-height wrapper — the pane must still reach the
    /// window bottom and the handle must drag in both directions.
    #[test]
    fn detail_handle_drag_with_selected_commit() {
        use app_core::git_view::{GitViewCommitDetail, StatusEntry};
        use rune::{Env, Runtime};

        use crate::editor::state::Patch;

        let home = app_core::test_support::TempDir::new("history-pane-selected");
        let _guard = HomeGuard::set(home.path());
        let mut s = handle_scene(320.0);
        let id = s.editor.history.commits[0].id.clone();
        s.editor.history.selected = Some(id);
        s.editor.history.detail = Some(Patch {
            label: "commit:test".into(),
            raw: "diff --git a/x b/x\n".into(),
            truncated: false,
            binary: false,
            files: vec![StatusEntry {
                path: "src/x.rs".into(),
                orig_path: None,
                status: "modified",
                added: Some(1),
                removed: Some(0),
            }],
            detail: Some(GitViewCommitDetail {
                author_email: "a@b.c".into(),
                committer: "A U Thor".into(),
                committer_email: "a@b.c".into(),
                body: String::new(),
            }),
            checkout: s.editor.checkout_key.clone().unwrap_or_default(),
        });
        let mut rt = Runtime::new(s, crate::view, (900.0, 700.0), Env::new(true, 2.0));
        for _ in 0..4 {
            rt.frame(1.0 / 60.0);
        }
        let g = detail_grip(&rt);
        let y_before = g.rect[1];
        // Detail pane bottom edge must sit at the viewport bottom.
        let detail_bottom = rt
            .elements()
            .into_iter()
            .filter(|e| e.rect[1] >= y_before + DETAIL_HANDLE_H - 1.0)
            .map(|e| e.rect[1] + e.rect[3])
            .fold(0.0f32, f32::max);
        assert!(
            (detail_bottom - 700.0).abs() < 2.0,
            "detail pane reaches the window bottom: {detail_bottom}"
        );
        let (x, y) = (g.rect[0] + g.rect[2] / 2.0, y_before + g.rect[3] / 2.0);
        drag_y(&mut rt, x, y, y - 50.0);
        let persisted = rt.state.editor.history.layout.detail_height;
        assert!(
            (persisted - 370.0).abs() < 2.0,
            "320 + 50 up-drag -> {persisted}"
        );
        let y_after = detail_grip(&rt).rect[1];
        assert!(
            (y_before - y_after - 50.0).abs() < 2.0,
            "boundary moved up with the drag: {y_before} -> {y_after}"
        );
    }

    /// The comfortable 160 floor still binds on viewports that can fit it:
    /// a long down-drag stops at 160 and the boundary tracks it exactly.
    #[test]
    fn detail_handle_respects_floor_on_large_window() {
        use rune::{Env, Runtime};

        let home = app_core::test_support::TempDir::new("history-pane-floor");
        let _guard = HomeGuard::set(home.path());
        let s = handle_scene(320.0);
        let mut rt = Runtime::new(s, crate::view, (900.0, 700.0), Env::new(true, 2.0));
        for _ in 0..4 {
            rt.frame(1.0 / 60.0);
        }
        let g = detail_grip(&rt);
        let y_before = g.rect[1];
        let (x, y) = (g.rect[0] + g.rect[2] / 2.0, y_before + g.rect[3] / 2.0);
        drag_y(&mut rt, x, y, y + 300.0);
        assert_eq!(
            rt.state.editor.history.layout.detail_height, 160.0,
            "drag floor is the comfortable 160"
        );
        let y_after = detail_grip(&rt).rect[1];
        assert!(
            (y_after - y_before - 160.0).abs() < 2.0,
            "boundary moved exactly to the floor: {y_before} -> {y_after}"
        );
    }

    /// Restart regression for the graph column: a fresh `TableState` has no
    /// widths, so without the GetSetting-time seed the first sync poll read
    /// `column_width("graph") == None` as "auto" and wiped the stored
    /// override. Seed → sync must preserve it, and reset must clear it.
    #[test]
    fn graph_column_width_survives_restart_and_reset() {
        use rune::{Env, Runtime};

        let home = app_core::test_support::TempDir::new("history-graph-roundtrip");
        let _guard = HomeGuard::set(home.path());

        // Without the seed a fresh table reads graph as auto and sync wipes
        // the stored override — the exact restart bug.
        let mut s0 = handle_scene(320.0);
        s0.editor.history.layout = HistoryLayout::parse(r#"{"graph":198}"#);
        sync_layout_from_table(&mut s0);
        assert_eq!(s0.editor.history.layout.graph, None);

        // Session A: user dragged Graph to 198pt; poll persists it.
        let mut s = handle_scene(320.0);
        s.editor.history.table.set_column_width("graph", 198.0);
        let mut rt = Runtime::new(s, crate::view, (900.0, 700.0), Env::new(true, 2.0));
        for _ in 0..4 {
            rt.frame(1.0 / 60.0);
        }
        // Test Runtime has no app poll wired — drive the sync seam directly,
        // exactly what `history::poll` does every frame in the real app.
        sync_layout_from_table(&mut rt.state);
        let json = rt.state.editor.history.layout.to_json();
        assert!(json.contains("\"graph\":198"), "graph persisted: {json}");

        // Session B: fresh state, settings-reply path — load + seed.
        let mut s2 = handle_scene(320.0);
        s2.editor.history.layout = HistoryLayout::load(Some(json.as_str()));
        seed_columns_from_layout(&mut s2.editor.history);
        assert_eq!(s2.editor.history.table.column_width("graph"), Some(198.0));

        let mut rt2 = Runtime::new(s2, crate::view, (900.0, 700.0), Env::new(true, 2.0));
        for _ in 0..4 {
            rt2.frame(1.0 / 60.0);
        }
        sync_layout_from_table(&mut rt2.state);
        assert_eq!(
            rt2.state.editor.history.layout.graph,
            Some(198.0),
            "graph override survived restart"
        );
        let hdr = rt2
            .elements()
            .into_iter()
            .find(|e| e.role == "columnheader" && e.label.as_deref() == Some("Graph"))
            .expect("graph column header mounted");
        assert!(
            (hdr.rect[2] - 198.0).abs() < 4.0,
            "graph column renders at restored width: {}",
            hdr.rect[2]
        );

        // Double-click reset semantics: clearing the table width clears the
        // model and the next store write omits the key.
        rt2.state.editor.history.table.reset_column_width("graph");
        for _ in 0..4 {
            rt2.frame(1.0 / 60.0);
        }
        sync_layout_from_table(&mut rt2.state);
        assert_eq!(rt2.state.editor.history.layout.graph, None);
        let json2 = rt2.state.editor.history.layout.to_json();
        assert!(
            !json2.contains("graph"),
            "graph omitted after reset: {json2}"
        );
    }

    /// A sub-160 `detailHeight` persisted at the 760×480 minimum must
    /// round-trip the store AND render at the restored boundary; a large
    /// window reclaims it to the 160 floor.
    #[test]
    fn sub_floor_detail_height_round_trips_and_renders() {
        use rune::{Env, Runtime};

        let home = app_core::test_support::TempDir::new("history-detail-roundtrip");
        let _guard = HomeGuard::set(home.path());

        // Drag at the min window: persisted lands below 160 but ≥ 96.
        let s = handle_scene(320.0);
        let mut rt = Runtime::new(s, crate::view, (760.0, 480.0), Env::new(true, 2.0));
        for _ in 0..4 {
            rt.frame(1.0 / 60.0);
        }
        let g = detail_grip(&rt);
        let (x, y) = (g.rect[0] + g.rect[2] / 2.0, g.rect[1] + g.rect[3] / 2.0);
        drag_y(&mut rt, x, y, y + 60.0);
        let persisted = rt.state.editor.history.layout.detail_height;
        assert!(
            (96.0..160.0).contains(&persisted),
            "persisted {persisted} is viewport-valid sub-floor"
        );

        // Store round-trip keeps the sub-160 value (was clamped to 160).
        let json = rt.state.editor.history.layout.to_json();
        let loaded = HistoryLayout::load(Some(json.as_str()));
        assert!(
            (loaded.detail_height - persisted).abs() < 0.5,
            "loaded {} == persisted {persisted}",
            loaded.detail_height
        );

        // Same min window: the restored value renders at its own boundary —
        // handle y = window bottom − rendered detail − handle height.
        let mut s2 = handle_scene(320.0);
        s2.editor.history.layout = HistoryLayout::load(Some(json.as_str()));
        let mut rt2 = Runtime::new(s2, crate::view, (760.0, 480.0), Env::new(true, 2.0));
        settle_detail(&mut rt2);
        let container = rt2.state.editor.history.measured_root_height as f64;
        let dmax = container - TOOLBAR_H as f64 - TABLE_MIN_H as f64 - DETAIL_HANDLE_H as f64;
        let floor = if dmax >= DETAIL_MIN {
            DETAIL_MIN
        } else {
            (PATCH_MIN_H as f64).min(dmax)
        };
        let rendered = loaded.detail_height.clamp(floor, dmax);
        let g2 = detail_grip(&rt2);
        let expected_y = 480.0 - rendered as f32 - DETAIL_HANDLE_H;
        assert!(
            (g2.rect[1] - expected_y).abs() < 2.0,
            "restored boundary: expected y={expected_y}, got {}",
            g2.rect[1]
        );

        // Large window: the same stored value is reclaimed to the 160 floor.
        let mut s3 = handle_scene(320.0);
        s3.editor.history.layout = HistoryLayout::load(Some(json.as_str()));
        let mut rt3 = Runtime::new(s3, crate::view, (900.0, 700.0), Env::new(true, 2.0));
        settle_detail(&mut rt3);
        let expected_y3 = 700.0 - DETAIL_MIN as f32 - DETAIL_HANDLE_H;
        let g3 = detail_grip(&rt3);
        assert!(
            (g3.rect[1] - expected_y3).abs() < 2.0,
            "large window reclaims to 160: expected y={expected_y3}, got {}",
            g3.rect[1]
        );
    }

    // -- detail position: Right split --------------------------------------

    /// History scene in Right position: same commits as `handle_scene`, with
    /// `detail_position` flipped and `detail_width` persisted.
    fn right_scene(detail_width: f64) -> State {
        let mut s = handle_scene(320.0);
        s.editor.history.layout.detail_position = DetailPosition::Right;
        s.editor.history.layout.set_detail_width(detail_width);
        s
    }

    fn width_grip(rt: &rune::Runtime<State>) -> rune::ElementInfo {
        rt.elements()
            .into_iter()
            .find(|e| {
                e.role == "separator" && e.label.as_deref() == Some("Resize history detail width")
            })
            .expect("detail width handle mounted")
    }

    fn drag_x(rt: &mut rune::Runtime<State>, x0: f32, y: f32, x1: f32) {
        rt.pointer_move(x0, y);
        rt.pointer_down(x0, y);
        rt.frame(1.0 / 60.0);
        for i in 1..=4 {
            rt.pointer_move(x0 + (x1 - x0) * i as f32 / 4.0, y);
            rt.frame(1.0 / 60.0);
        }
        rt.pointer_up(x1, y);
        rt.frame(1.0 / 60.0);
    }

    /// The commit table's rendered width — the "Commit history" grid spans
    /// exactly the space the flex row gives it.
    fn table_width(rt: &rune::Runtime<State>) -> f32 {
        rt.elements()
            .into_iter()
            .find(|e| e.role == "grid" && e.label.as_deref() == Some("Commit history"))
            .expect("commit table mounted")
            .rect[2]
    }

    /// Right position mounts the vertical handle (not the horizontal one)
    /// and renders table | handle | detail in one row. An oversized stored
    /// width clamps to `wmax` — the detail never exceeds the row minus the
    /// table's minimum — while a fitting width renders as stored.
    #[test]
    fn right_position_clamps_detail_width_and_keeps_table_min() {
        use rune::{Env, Runtime};

        let home = app_core::test_support::TempDir::new("history-right-clamp");
        let _guard = HomeGuard::set(home.path());
        let s = right_scene(5000.0);
        let mut rt = Runtime::new(s, crate::view, (1200.0, 700.0), Env::new(true, 2.0));
        for _ in 0..4 {
            rt.frame(1.0 / 60.0);
        }

        // Only the vertical handle exists in Right position.
        assert!(width_grip(&rt).rect[3] > 100.0, "handle spans row height");
        assert!(
            !rt.elements().iter().any(|e| e
                .label
                .as_deref()
                .is_some_and(|l| l.contains("history table and detail"))),
            "horizontal handle unmounted in Right position"
        );

        let root_w = rt.state.editor.history.measured_root_width as f64;
        assert!(root_w > 0.0, "root width measured");
        let (floor, wmax) = detail_width_bounds(&rt.state.editor.history);
        let expected = rt
            .state
            .editor
            .history
            .layout
            .detail_width
            .clamp(floor, wmax);
        // 5000 stored → renders at wmax; the table keeps exactly TABLE_MIN_W
        // because the clamp reserves it (plus the 8pt handle).
        assert!(expected < 5000.0 && expected > 0.0, "clamped: {expected}");

        let grid = rt
            .elements()
            .into_iter()
            .find(|e| e.role == "grid" && e.label.as_deref() == Some("Commit history"))
            .expect("commit table mounted");
        let table_w = grid.rect[2];
        assert!(
            table_w >= TABLE_MIN_W - 1.0,
            "table keeps its minimum: {table_w}"
        );
        // Detail right edge is the row's right edge (window width); its left
        // edge is the handle's right edge.
        let g = width_grip(&rt);
        let detail_left = g.rect[0] + g.rect[2];
        let detail_w = 1200.0 - detail_left;
        assert!(
            (detail_w - expected as f32).abs() < 2.0,
            "rendered detail width {detail_w} == clamped {expected}"
        );
        // And the row accounts exactly: table + handle + detail == root width.
        let accounted = table_w + g.rect[2] + detail_w;
        assert!(
            (accounted - root_w as f32).abs() < 2.0,
            "row fills the root: {accounted} vs {root_w}"
        );
    }

    /// A stored width inside the bounds renders un-clamped, and narrowing
    /// the window re-clamps it (the stored value is the user's choice).
    #[test]
    fn right_position_renders_stored_width_then_reclamps() {
        use rune::{Env, Runtime};

        let home = app_core::test_support::TempDir::new("history-right-width");
        let _guard = HomeGuard::set(home.path());
        let s = right_scene(420.0);
        let mut rt = Runtime::new(s, crate::view, (1200.0, 700.0), Env::new(true, 2.0));
        for _ in 0..4 {
            rt.frame(1.0 / 60.0);
        }
        let root_w = rt.state.editor.history.measured_root_width as f64;
        assert!(root_w > 0.0, "root width measured");
        let (floor, wmax) = detail_width_bounds(&rt.state.editor.history);
        assert!(
            420.0 >= floor && 420.0 <= wmax,
            "420 inside bounds at 1200px"
        );
        let g = width_grip(&rt);
        let detail_w = 1200.0 - (g.rect[0] + g.rect[2]);
        assert!(
            (detail_w - 420.0).abs() < 2.0,
            "stored 420 renders: {detail_w}"
        );
        assert!(table_width(&rt) > TABLE_MIN_W, "table keeps free space");

        // Shrink the window below what 420 + table-min needs: the detail
        // yields, the stored 420 stays.
        rt.set_viewport((760.0, 700.0), 2.0);
        for _ in 0..4 {
            rt.frame(1.0 / 60.0);
        }
        let root_w = rt.state.editor.history.measured_root_width as f64;
        assert!(
            root_w < 872.0,
            "narrow window re-measured the root: {root_w}"
        );
        let (_, wmax) = detail_width_bounds(&rt.state.editor.history);
        assert!(wmax < 420.0, "narrow row caps below stored: {wmax}");
        assert_eq!(
            rt.state.editor.history.layout.detail_width, 420.0,
            "stored intent preserved"
        );
        let g = width_grip(&rt);
        let detail_w = 760.0 - (g.rect[0] + g.rect[2]);
        assert!(
            (detail_w - wmax as f32).abs() < 2.0,
            "detail re-clamped to {wmax}: {detail_w}"
        );
        assert!(
            width_grip(&rt).rect[0] + width_grip(&rt).rect[2] + detail_w <= 760.5,
            "detail stays on-screen at 760px"
        );
    }

    /// Dragging the vertical handle left grows the persisted width and arms
    /// the settings lane; a double-press restores the 420 default.
    #[test]
    fn right_detail_handle_drags_and_double_press_resets() {
        use rune::{Env, Runtime};

        let home = app_core::test_support::TempDir::new("history-right-drag");
        let _guard = HomeGuard::set(home.path());
        let s = right_scene(300.0);
        let mut rt = Runtime::new(s, crate::view, (1200.0, 700.0), Env::new(true, 2.0));
        for _ in 0..4 {
            rt.frame(1.0 / 60.0);
        }
        let g = width_grip(&rt);
        assert!(
            (g.rect[2] - DETAIL_HANDLE_W).abs() < 0.5,
            "handle is the 8pt strip"
        );
        let (x, y) = (g.rect[0] + g.rect[2] / 2.0, g.rect[1] + g.rect[3] / 2.0);

        // Drag LEFT 60px: the detail (right of the handle) grows.
        drag_x(&mut rt, x, y, x - 60.0);
        let persisted = rt.state.editor.history.layout.detail_width;
        assert!(
            (persisted - 360.0).abs() < 2.0,
            "300 + 60 left-drag -> {persisted}"
        );
        assert!(rt.state.editor.history.layout_dirty);

        // Double-press restores the default width.
        let g = width_grip(&rt);
        let (x, y) = (g.rect[0] + g.rect[2] / 2.0, g.rect[1] + g.rect[3] / 2.0);
        for _ in 0..2 {
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(
            rt.state.editor.history.layout.detail_width, 550.0,
            "double-press resets to the default width"
        );
        assert!(rt.state.editor.history.layout_dirty);
    }

    /// The toolbar segmented control flips `detail_position` and arms the
    /// same persistence lane the drag handles use.
    #[test]
    fn detail_position_toggle_flips_and_arms_persist() {
        use rune::{Env, Runtime};

        let home = app_core::test_support::TempDir::new("history-pos-toggle");
        let _guard = HomeGuard::set(home.path());
        let s = handle_scene(320.0);
        let mut rt = Runtime::new(s, crate::view, (1200.0, 700.0), Env::new(true, 2.0));
        for _ in 0..4 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(
            rt.state.editor.history.layout.detail_position,
            DetailPosition::Bottom
        );

        let pick = |rt: &Runtime<State>, label: &str| {
            rt.elements()
                .into_iter()
                .find(|e| e.role == "radio" && e.label.as_deref() == Some(label))
                .unwrap_or_else(|| panic!("{label:?} segment mounted"))
        };
        let r = pick(&rt, "Right").rect;
        let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);

        assert_eq!(
            rt.state.editor.history.layout.detail_position,
            DetailPosition::Right
        );
        assert!(rt.state.editor.history.layout_dirty, "toggle arms persist");
        assert!(
            rt.state
                .editor
                .history
                .layout
                .to_json()
                .contains("\"detailPosition\":\"right\""),
            "position serialized"
        );
        // The vertical handle mounts and the horizontal one unmounts.
        assert!(width_grip(&rt).rect[3] > 100.0);

        let r = pick(&rt, "Bottom").rect;
        let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert_eq!(
            rt.state.editor.history.layout.detail_position,
            DetailPosition::Bottom
        );
    }

    #[test]
    fn branch_combobox_opens_with_every_ref() {
        use rune::{Cx, Env, Runtime};

        fn branch(name: &str, remote: bool) -> app_core::git_view::GitViewBranch {
            app_core::git_view::GitViewBranch {
                ref_name: format!("refs/{name}"),
                name: name.into(),
                remote,
                current: false,
            }
        }
        fn toolbar_view(s: &State, _cx: &mut Cx<State>) -> El {
            let mut layer = Vec::new();
            let bar = toolbar(s, &s.ink, &s.editor.kit, &mut layer);
            rune_kit::root(bar, layer)
        }
        let mut s = State::new(crate::test_handle());
        s.editor.history.branches = vec![branch("main", false), branch("origin/dev", true)];
        let mut rt = Runtime::new(s, toolbar_view, (800.0, 300.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let options = |rt: &Runtime<State>| {
            rt.elements()
                .into_iter()
                .filter(|e| e.role == "option")
                .count()
        };
        assert_eq!(options(&rt), 0, "starts closed");

        let trigger = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("history-branch-value-trigger"))
            .expect("the combobox trigger is mounted");
        let (x, y) = (
            trigger.rect[0] + 4.0,
            trigger.rect[1] + trigger.rect[3] / 2.0,
        );
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert!(rt.state.editor.history.branch_picker.open);
        assert_eq!(options(&rt), 3, "All branches + local + remote");
    }
}
