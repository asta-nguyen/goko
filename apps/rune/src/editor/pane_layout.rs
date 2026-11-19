//! Horizontal pane resizing for the editor's list/detail splits (Files
//! aside ↔ editor, Changes list ↔ patch, Stashes list ↔ detail).
//!
//! Widths persist inside the existing `goko.history.layout` settings
//! document (`panes` object — see [`history_layout`]) so the already-wired
//! GetSetting/SetSetting lane carries them with no second key. The grip
//! mirrors rune-kit's `Split` separator: `role="separator"`,
//! `range_value`/`range_step`, `Cursor::ResizeColumn`, drag via
//! `Drag::data` start-width + `dx`, `on_step` keyboard increments and a
//! double-press reset — but stays visible (the pane's border line plus a
//! hover wash) instead of kit's 2%-opacity hit strip.
//!
//! Clamping is two-layered: [`PaneWidths::set`] bounds the stored value to
//! [`PANE_MIN`]..=[`PANE_MAX`] (the desktop `editorList` PANEL_LIMITS), and
//! [`clamp_width`] additionally bounds it against the measured split row so
//! the detail pane keeps [`SIBLING_MIN`] of usable space. Flex does the
//! display clamp (`aside.shrink().min_w(PANE_MIN)` +
//! `detail.grow(1).min_w(SIBLING_MIN)`), so a restored oversized width
//! shrinks on a narrow window and returns when the window grows again.

use rune::{Color, Cursor, Drag, El, Phase, Tx, col, el};

use crate::State;
use crate::editor::history_layout::PaneKind;
use crate::theme::Ink;

/// Narrowest a list aside may be dragged (desktop `editorList.minWidth`).
pub const PANE_MIN: f32 = 180.0;
/// Widest a list aside may be dragged (desktop `editorList.maxWidth`).
pub const PANE_MAX: f32 = 480.0;
/// Space the detail/editor pane keeps beside the aside — smaller than the
/// desktop's 320 because the native minimum window leaves ~440 px for the
/// whole editor column.
pub const SIBLING_MIN: f32 = 240.0;
/// Grip hit-strip width: wide enough to grab without aiming.
pub const GRIP_W: f32 = 8.0;
/// One keyboard step (arrows/Home/End via `on_step`, and AT increments).
pub const GRIP_STEP: f32 = 8.0;

/// Per-section defaults — each keeps the width its pane shipped with.
pub const FILES_ASIDE_DEFAULT: f64 = 280.0;
pub const CHANGES_LIST_DEFAULT: f64 = 300.0;
pub const STASHES_LIST_DEFAULT: f64 = 280.0;

/// The persisted width for `kind` in px (already statically bounded).
pub fn current(s: &State, kind: PaneKind) -> f32 {
    s.editor.history.layout.panes.get(kind) as f32
}

/// Largest width `kind` may take given the split row's measured width
/// (`pane_row_w`; 0 before the first layout pass means "unmeasured").
pub fn max_width(row_w: f32) -> f32 {
    if row_w > 0.0 {
        (row_w - SIBLING_MIN).clamp(PANE_MIN, PANE_MAX)
    } else {
        PANE_MAX
    }
}

/// Clamp a drag/step target into [PANE_MIN, viewport max].
pub fn clamp_width(row_w: f32, px: f32) -> f32 {
    px.clamp(PANE_MIN, max_width(row_w))
}

/// Store a new width for `kind` and mark the layout document dirty so the
/// existing `poll_layout_save` lane persists it.
fn set_width(s: &mut State, kind: PaneKind, px: f32) {
    s.editor.history.layout.panes.set(kind, px as f64);
    s.editor.history.layout_dirty = true;
}

fn grip_key(kind: PaneKind) -> &'static str {
    match kind {
        PaneKind::FilesAside => "files-pane-grip",
        PaneKind::ChangesList => "changes-pane-grip",
        PaneKind::StashesList => "stashes-pane-grip",
    }
}

/// Whether the editor's list asides (Files tree, Changes list, Stashes list)
/// dock on the right; the three sections share one side via `list_aside_side`.
pub fn docks_right(s: &State) -> bool {
    s.list_aside_side == crate::sidebar::Side::Right
}

/// The 1 px border edge that faces the detail pane: the list's trailing
/// (right) edge when docked left, its leading (left) edge when docked right.
pub fn aside_border(right: bool) -> [f32; 4] {
    if right {
        [0.0, 0.0, 0.0, 1.0]
    } else {
        [0.0, 1.0, 0.0, 0.0]
    }
}

/// The grip's pin edges `(left, top, right, bottom)` — the aside's trailing
/// edge when docked left, its leading edge when docked right.
fn grip_pin(right: bool) -> (Option<f32>, Option<f32>, Option<f32>, Option<f32>) {
    if right {
        (Some(0.0), Some(0.0), None, Some(0.0))
    } else {
        (None, Some(0.0), Some(0.0), Some(0.0))
    }
}

/// A right-docked list drags/steps the opposite way so it grows toward the
/// detail pane.
fn grip_sign(right: bool) -> f32 {
    if right { -1.0 } else { 1.0 }
}

/// The visible, grabbable separator pinned inside a pane aside's trailing
/// edge. Place it as the aside's LAST child so it paints above the content.
pub fn grip(s: &State, ink: &Ink, kind: PaneKind, label: &'static str) -> El {
    // Report the effective (viewport-clamped) width so AT reads what is
    // actually rendered when a restored oversized value shrinks via flex.
    let width = clamp_width(s.editor.pane_row_w, current(s, kind));
    let max = max_width(s.editor.pane_row_w);
    // Any list aside (Files, Changes, Stashes) shares one side; a right-docked
    // grip sits on the aside's leading (left) edge and dragging left widens it.
    let right = docks_right(s);
    let dir = grip_sign(right);
    let mut b = col()
        .key(grip_key(kind))
        .role("separator")
        .label(label)
        .range_value(f64::from(width), f64::from(PANE_MIN), f64::from(max))
        .range_step(f64::from(GRIP_STEP))
        .w(GRIP_W)
        .items_center()
        .cursor(Cursor::ResizeColumn)
        .fill(Color::TRANSPARENT)
        .hover(ink.border);
    let (pl, pt, pr, pb) = grip_pin(right);
    b = b.pin(pl, pt, pr, pb);
    b.child(el().w(1.0).h_full().fill(ink.border))
        .on_drag(move |st: &mut State, d: &mut Drag, tx: &mut Tx| {
            match d.phase {
                Phase::Down => d.data = current(st, kind),
                Phase::Move | Phase::Up => {
                    if !d.cancelled() {
                        // Leading pane drags right to widen; docked right the
                        // grip drags left to widen instead.
                        let w = clamp_width(st.editor.pane_row_w, d.data + d.dx * dir);
                        set_width(st, kind, w);
                        tx.instant();
                    }
                }
            }
        })
        .on_double_press(move |st: &mut State| {
            st.editor.history.layout.panes.reset(kind);
            st.editor.history.layout_dirty = true;
        })
        .on_step(move |st: &mut State, steps: i32, _tx: &mut Tx| {
            let w = clamp_width(
                st.editor.pane_row_w,
                current(st, kind) + steps as f32 * GRIP_STEP * dir,
            );
            set_width(st, kind, w);
        })
        .focusable(false)
}

/// Record the split row's width for viewport clamping. Attach to the row
/// that directly contains the aside + detail panes; fires only when the
/// measured width changes by >0.5 pt.
pub fn watch_row_width() -> impl Fn(&mut State, [f32; 4], &mut Tx) + 'static {
    |st: &mut State, r: [f32; 4], _tx: &mut Tx| {
        st.editor.pane_row_w = r[2];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_width_respects_viewport_and_static_bounds() {
        // Unmeasured (0): static max only.
        assert_eq!(max_width(0.0), PANE_MAX);
        // Wide row: static max wins.
        assert_eq!(max_width(2000.0), PANE_MAX);
        // Row leaving exactly SIBLING_MIN for the detail.
        assert_eq!(max_width(720.0), 480.0);
        assert_eq!(max_width(520.0), 280.0);
        // Narrow row: floor keeps PANE_MIN rather than collapsing.
        assert_eq!(max_width(300.0), PANE_MIN);
        assert_eq!(max_width(100.0), PANE_MIN);
    }

    #[test]
    fn clamp_width_bounds_drag_targets() {
        assert_eq!(clamp_width(2000.0, 5000.0), PANE_MAX);
        assert_eq!(clamp_width(2000.0, 10.0), PANE_MIN);
        assert_eq!(clamp_width(520.0, 480.0), 280.0);
        assert_eq!(clamp_width(0.0, 300.0), 300.0);
    }

    #[test]
    fn right_docked_grip_inverts_direction_and_pins_the_leading_edge() {
        // Left-docked: pinned to the trailing (right) edge, drag right widens,
        // border on the trailing edge.
        assert_eq!(grip_pin(false), (None, Some(0.0), Some(0.0), Some(0.0)));
        assert_eq!(grip_sign(false), 1.0);
        assert_eq!(aside_border(false), [0.0, 1.0, 0.0, 0.0]);
        // Right-docked: pinned to the leading (left) edge, drag left widens,
        // border on the leading edge.
        assert_eq!(grip_pin(true), (Some(0.0), Some(0.0), None, Some(0.0)));
        assert_eq!(grip_sign(true), -1.0);
        assert_eq!(aside_border(true), [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn grip_builds_on_either_side_for_each_kind() {
        let mut s = State::new(crate::test_handle());
        let ink = Ink::new();
        for side in [crate::sidebar::Side::Left, crate::sidebar::Side::Right] {
            s.list_aside_side = side;
            for (kind, label) in [
                (PaneKind::FilesAside, "Resize file list and editor"),
                (PaneKind::ChangesList, "Resize changed files and diff"),
                (PaneKind::StashesList, "Resize stash list and detail"),
            ] {
                let _ = grip(&s, &ink, kind, label);
            }
        }
    }
}
