//! Terminal pane splits: the recursive binary layout every workspace tab
//! owns, the close lifecycle the daemon drives, and the per-pane terminal
//! attachments.
//!
//! Ported from the desktop's `stores/ui.ts` pane tree and
//! `TerminalWorkspace.tsx`'s `ResizableSplit`/close flow, in Rune's compact
//! idiom: one `term::Terminal` per visible pane, fed and rendered by the
//! same poll the single terminal used. The daemon still owns every
//! process — a pane close stops (force) then deletes, with the error kept
//! on the pane until Retry, exactly like the desktop.

use std::collections::HashSet;
use std::time::Duration;

use protocol::session::SessionId;
use rune::{Color, El, Phase, TextStyle, Tx, col, el, row};

use crate::State;
use crate::data::SessionState;
use crate::editor::backend::{self, Job, Req};
use crate::sessions::{self, Pending, SessionLaunch};
use crate::term;
use crate::theme::{self, Ink};

/// The smallest a pane may be along either axis, in points — the desktop's
/// `MIN_PANE_SIZE`. A split too small for both minimums recentres at 50/50.
pub const MIN_PANE_PX: f32 = 180.0;
/// The draggable separator's thickness, in points.
pub const SPLITTER_PX: f32 = 8.0;
/// Arrow-key ratio step; PageUp/PageDown step ten of these (the slider's
/// `on_step` deltas).
const KEY_STEP: f32 = 0.025;
/// The stored ratio's clamp — the desktop's `setSplitRatio` bounds.
const RATIO_MIN: f32 = 0.05;
const RATIO_MAX: f32 = 0.95;
/// How long a closing pane's stop may hang before Force kill shows — the
/// desktop's five-second window.
const FORCE_KILL_AFTER_MS: i64 = sessions::FORCE_KILL_AFTER_MS;
/// The host inset around each pane's line grid — the same 8 px the single
/// terminal used.
const PANE_PAD: f32 = 8.0;
/// 13 pt monospace on a 16 pt line — must match `term::TERM_FONT` so the
/// measured grid still fits the pane.
const PANE_FONT: TextStyle = TextStyle::new(13.0, 400.0).mono().lh(16.0);

/// Which way a split arranges its panes: `SideBySide` is a row (left |
/// right), `Stacked` a column (top / bottom).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    SideBySide,
    Stacked,
}

/// One pane: the session it shows and where its close stands.
#[derive(Clone, Debug)]
pub struct Pane {
    pub id: String,
    /// The session key (`data::SessionInfo::id`) this pane renders.
    pub session: String,
    /// When the close began; `None` while the pane is open.
    pub closing_at: Option<i64>,
    /// When the close's stop was issued — the force-kill window counts
    /// from it.
    pub stop_requested_at: Option<i64>,
    /// The last close failure, shown on the pane until Retry.
    pub close_error: Option<String>,
}

/// A binary split: `first` takes `ratio` of the space, `second` the rest,
/// with the separator between them.
#[derive(Clone, Debug)]
pub struct Split {
    pub id: String,
    pub direction: Direction,
    /// The first child's share of the space along the split's axis.
    pub ratio: f32,
    pub first: Box<Layout>,
    pub second: Box<Layout>,
}

/// A tab's content: one pane, or a tree of splits down to panes.
#[derive(Clone, Debug)]
pub enum Layout {
    Pane(Pane),
    Split(Split),
}

/// One workspace tab: its title, its pane tree and the pane the selection,
/// controls strip and keyboard point at. The tab id is its own — a split
/// tab has several sessions, so "the tab's session" does not exist.
#[derive(Clone, Debug)]
pub struct Tab {
    pub id: String,
    pub title: String,
    pub layout: Layout,
    /// The focused pane's id.
    pub active_pane: String,
    /// Every pane is closing — set by the tab's close button.
    pub closing: bool,
}

/// Where a `session.create` reply lands once it arrives.
#[derive(Clone, Debug)]
pub enum Placement {
    /// Its own tab, selected.
    Tab,
    /// A new pane splitting `pane` inside tab `tab`, along `direction`.
    /// A tab or pane that moved while the create flew falls back to a
    /// fresh tab — the desktop's `splitPane` does the same.
    Split {
        tab: String,
        pane: String,
        direction: Direction,
    },
}

/// One pane's live terminal: the grid, its rendered rows and the size the
/// daemon's PTY was last told.
pub struct Attachment {
    pub term: term::Terminal,
    pub lines: Vec<term::TermLine>,
    /// The grid size measured from the pane's rect.
    pub cols: usize,
    pub rows: usize,
    /// The size the daemon's PTY was last told about.
    pub pty: (usize, usize),
}

// ---------------------------------------------------------------------------
// The layout tree — ports of stores/ui.ts's pane helpers.
// ---------------------------------------------------------------------------

pub fn pane_id(key: &str) -> String {
    format!("pane-{key}")
}

fn tab_id(key: &str) -> String {
    format!("tab-{key}")
}

fn split_id(added_pane: &str) -> String {
    format!("split-{added_pane}")
}

fn new_pane(session: &str) -> Pane {
    Pane {
        id: pane_id(session),
        session: session.to_string(),
        closing_at: None,
        stop_requested_at: None,
        close_error: None,
    }
}

/// Every pane in the layout, in tree order.
pub fn collect_panes(layout: &Layout) -> Vec<&Pane> {
    match layout {
        Layout::Pane(pane) => vec![pane],
        Layout::Split(split) => {
            let mut out = collect_panes(&split.first);
            out.extend(collect_panes(&split.second));
            out
        }
    }
}

/// The pane `id` names, wherever it sits in the tree.
pub fn pane_by_id<'a>(layout: &'a Layout, id: &str) -> Option<&'a Pane> {
    match layout {
        Layout::Pane(pane) => (pane.id == id).then_some(pane),
        Layout::Split(split) => {
            pane_by_id(&split.first, id).or_else(|| pane_by_id(&split.second, id))
        }
    }
}

/// The pane showing `session`, wherever it sits in the tree.
fn pane_by_session<'a>(layout: &'a Layout, session: &str) -> Option<&'a Pane> {
    match layout {
        Layout::Pane(pane) => (pane.session == session).then_some(pane),
        Layout::Split(split) => pane_by_session(&split.first, session)
            .or_else(|| pane_by_session(&split.second, session)),
    }
}

/// Whether `session` already has a pane in the layout — a session never
/// sits in two panes (the desktop's `sessionLocation` guard).
pub fn layout_has_session(layout: &Layout, session: &str) -> bool {
    pane_by_session(layout, session).is_some()
}

/// `splitPane`'s tree rewrite: the named pane becomes a 50/50 split with
/// the new session's pane second. `None` when the pane is missing or
/// already closing — a closing pane takes no new neighbour.
pub fn split_layout(
    layout: &Layout,
    pane: &str,
    direction: Direction,
    session: &str,
) -> Option<Layout> {
    match layout {
        Layout::Pane(p) => {
            if p.id != pane || p.closing_at.is_some() {
                return None;
            }
            let added = new_pane(session);
            Some(Layout::Split(Split {
                id: split_id(&added.id),
                direction,
                ratio: 0.5,
                first: Box::new(layout.clone()),
                second: Box::new(Layout::Pane(added)),
            }))
        }
        Layout::Split(s) => {
            if let Some(first) = split_layout(&s.first, pane, direction, session) {
                return Some(Layout::Split(Split {
                    id: s.id.clone(),
                    direction: s.direction,
                    ratio: s.ratio,
                    first: Box::new(first),
                    second: s.second.clone(),
                }));
            }
            split_layout(&s.second, pane, direction, session).map(|second| {
                Layout::Split(Split {
                    id: s.id.clone(),
                    direction: s.direction,
                    ratio: s.ratio,
                    first: s.first.clone(),
                    second: Box::new(second),
                })
            })
        }
    }
}

/// `removePane`: drop the session's pane; an emptied side collapses the
/// split into its sibling. `None` when nothing is left.
pub fn remove_pane(layout: &Layout, session: &str) -> Option<Layout> {
    match layout {
        Layout::Pane(p) => (p.session != session).then(|| layout.clone()),
        Layout::Split(s) => {
            let first = remove_pane(&s.first, session);
            let second = remove_pane(&s.second, session);
            match (first, second) {
                (None, second) => second,
                (first, None) => first,
                (Some(f), Some(snd)) => Some(Layout::Split(Split {
                    id: s.id.clone(),
                    direction: s.direction,
                    ratio: s.ratio,
                    first: Box::new(f),
                    second: Box::new(snd),
                })),
            }
        }
    }
}

/// The space a layout needs along one axis, in points — the desktop's
/// `minimumDimension`: a pane is `MIN_PANE_PX`, a split adds its
/// separator along its own axis and takes the larger child across it.
fn minimum_dimension(layout: &Layout, width_axis: bool) -> f32 {
    match layout {
        Layout::Pane(_) => MIN_PANE_PX,
        Layout::Split(s) => {
            let first = minimum_dimension(&s.first, width_axis);
            let second = minimum_dimension(&s.second, width_axis);
            let split_along = (s.direction == Direction::SideBySide) == width_axis;
            if split_along {
                first + SPLITTER_PX + second
            } else {
                first.max(second)
            }
        }
    }
}

/// The ratio range keeping both sides at their minimum inside `size`
/// points along the split's axis — the desktop's `ResizableSplit` bounds.
/// A split too small for both minimums recentres at 50/50.
fn ratio_bounds(first: &Layout, second: &Layout, direction: Direction, size: f32) -> (f32, f32) {
    let width_axis = direction == Direction::SideBySide;
    let available = (size - SPLITTER_PX).max(1.0);
    let min = minimum_dimension(first, width_axis) / available;
    let max = 1.0 - minimum_dimension(second, width_axis) / available;
    if min <= max { (min, max) } else { (0.5, 0.5) }
}

/// `f` on every pane showing `session`, across all tabs. Returns whether a
/// pane was found.
fn each_pane_mut(tabs: &mut [Tab], session: &str, f: &mut impl FnMut(&mut Pane)) -> bool {
    fn walk(layout: &mut Layout, session: &str, f: &mut impl FnMut(&mut Pane)) -> bool {
        match layout {
            Layout::Pane(pane) => {
                if pane.session == session {
                    f(pane);
                    true
                } else {
                    false
                }
            }
            Layout::Split(split) => {
                walk(&mut split.first, session, f) || walk(&mut split.second, session, f)
            }
        }
    }
    tabs.iter_mut().any(|tab| walk(&mut tab.layout, session, f))
}

fn find_split<'a>(layout: &'a Layout, id: &str) -> Option<&'a Split> {
    match layout {
        Layout::Pane(_) => None,
        Layout::Split(split) => {
            if split.id == id {
                Some(split)
            } else {
                find_split(&split.first, id).or_else(|| find_split(&split.second, id))
            }
        }
    }
}

fn find_split_mut<'a>(layout: &'a mut Layout, id: &str) -> Option<&'a mut Split> {
    match layout {
        Layout::Pane(_) => None,
        Layout::Split(split) => {
            if split.id == id {
                Some(split)
            } else {
                find_split_mut(&mut split.first, id)
                    .or_else(|| find_split_mut(&mut split.second, id))
            }
        }
    }
}

fn mark_layout_closing(layout: &mut Layout, now: i64) {
    match layout {
        Layout::Pane(pane) => {
            if pane.closing_at.is_none() {
                pane.closing_at = Some(now);
            }
            pane.stop_requested_at = None;
            pane.close_error = None;
        }
        Layout::Split(split) => {
            mark_layout_closing(&mut split.first, now);
            mark_layout_closing(&mut split.second, now);
        }
    }
}

// ---------------------------------------------------------------------------
// State: tabs, panes, placement, focus and the close lifecycle.
// ---------------------------------------------------------------------------

/// What a closing pane asks the daemon for next.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CloseStep {
    /// A live session: stop it (force), then delete once it exits.
    Stop,
    /// An exited/failed session: delete it.
    Delete,
    /// The daemon's list lost it: drop every local trace, nothing to ask.
    Drop,
}

impl State {
    /// The tab `active_tab` points at, when there is one.
    fn current_tab(&self) -> Option<&Tab> {
        self.open_tabs.get(self.active_tab)
    }

    /// The pane the tab's selection, controls strip and keyboard point at.
    pub fn focused_pane(tab: &Tab) -> Option<&Pane> {
        pane_by_id(&tab.layout, &tab.active_pane)
    }

    /// The daemon session the keyboard writes to: the focused pane's
    /// session of the active tab — never "the tab's session", a split tab
    /// has several.
    pub fn focused_terminal_session(&self) -> Option<SessionId> {
        let tab = self.current_tab()?;
        let pane = Self::focused_pane(tab)?;
        self.session_id(&pane.session)
    }

    /// Type `bytes` into a live session, from the keyboard or a file drop.
    /// An exited session takes nothing; a live one first returns to the
    /// live screen, so typing never lands on the history being read.
    pub fn type_into_terminal(&mut self, session: SessionId, bytes: &[u8]) -> bool {
        let live = self
            .data
            .sessions
            .iter()
            .any(|view| view.info.session_id == session && view.info.state.live());
        if !live {
            return false;
        }
        for att in self.attachments.values_mut() {
            if att.term.session == session {
                att.term.scroll_to_bottom();
            }
        }
        self.write_terminal(session, bytes);
        true
    }

    /// Files dropped on a pane: focus it and type their escaped paths.
    pub fn drop_files(&mut self, tab: &str, pane: &str, key: &str, files: &[std::path::PathBuf]) {
        self.focus_pane(tab, pane);
        if let Some(session) = self.session_id(key) {
            self.type_into_terminal(session, term::dropped_paths_text(files).as_bytes());
        }
    }

    /// The focused pane's session key, for controls and close actions.
    pub fn focused_session_key(&self) -> Option<String> {
        let tab = self.current_tab()?;
        Self::focused_pane(tab).map(|pane| pane.session.clone())
    }

    /// Whether `key`'s session already sits in a pane — placement guards
    /// against a session landing twice.
    pub fn session_placed(&self, key: &str) -> bool {
        self.open_tabs
            .iter()
            .any(|tab| layout_has_session(&tab.layout, key))
    }

    /// Where a created session lands: its own tab, or a new pane splitting
    /// the named pane. A placement whose tab or pane moved while the
    /// create flew falls back to a fresh tab — the desktop's `splitPane`
    /// does the same.
    pub fn place_session(&mut self, key: &str, title: &str, placement: Placement) {
        match placement {
            Placement::Tab => {
                if self.session_placed(key) {
                    self.focus_session_tab(key);
                } else {
                    self.push_session_tab(key, title);
                }
            }
            Placement::Split {
                tab,
                pane,
                direction,
            } => {
                let placed = self
                    .open_tabs
                    .iter()
                    .position(|t| t.id == tab)
                    .and_then(|index| {
                        let layout =
                            split_layout(&self.open_tabs[index].layout, &pane, direction, key)?;
                        let new_pane = pane_id(key);
                        let tab = &mut self.open_tabs[index];
                        tab.layout = layout;
                        tab.active_pane = new_pane;
                        Some(())
                    })
                    .is_some();
                if !placed {
                    self.push_session_tab(key, title);
                }
            }
        }
        self.sync_selection();
    }

    /// Open `key`'s session in a fresh tab and make it active.
    fn push_session_tab(&mut self, key: &str, title: &str) {
        self.open_tabs.push(Tab {
            id: tab_id(key),
            title: title.to_string(),
            layout: Layout::Pane(new_pane(key)),
            active_pane: pane_id(key),
            closing: false,
        });
        self.active_tab = self.open_tabs.len() - 1;
    }

    /// Select `index`'s tab and point the selection at its active pane.
    pub fn activate_tab(&mut self, index: usize) {
        if index >= self.open_tabs.len() {
            return;
        }
        self.active_tab = index;
        self.sync_selection();
    }

    /// Focus a session: its pane's tab and pane become active, opening a
    /// fresh tab when the session isn't placed yet — the desktop's
    /// `openSession`/`focusSession` path.
    pub fn focus_session_tab(&mut self, key: &str) {
        for (index, tab) in self.open_tabs.iter().enumerate() {
            if let Some(pane) = pane_by_session(&tab.layout, key) {
                let pane_id = pane.id.clone();
                self.active_tab = index;
                self.open_tabs[index].active_pane = pane_id;
                self.sync_selection();
                return;
            }
        }
        let title = self
            .session_by_key(key)
            .map(|view| view.title())
            .unwrap_or_else(|| key.to_string());
        self.push_session_tab(key, &title);
        self.sync_selection();
    }

    /// Clicking a pane makes it the tab's active pane: the selection, the
    /// controls strip and the keyboard all follow it.
    pub fn focus_pane(&mut self, tab: &str, pane: &str) {
        let Some(index) = self.open_tabs.iter().position(|t| t.id == tab) else {
            return;
        };
        if pane_by_id(&self.open_tabs[index].layout, pane).is_none() {
            return;
        }
        self.open_tabs[index].active_pane = pane.to_string();
        self.sync_selection();
    }

    /// Point the selection at the active tab's active pane; clears it when
    /// that pane's session row is gone. Shared by every tab/pane change.
    pub fn sync_selection(&mut self) {
        let found = self.current_tab().and_then(|tab| {
            Self::focused_pane(tab)
                .and_then(|pane| self.session_by_key(&pane.session))
                .map(|view| (view.info.id.clone(), self.session_workspace(view)))
        });
        match found {
            Some((key, workspace)) => {
                self.selected_session = Some(key);
                self.active_workspace = Some(workspace);
            }
            None => {
                self.selected_session = None;
                self.active_workspace = None;
            }
        }
    }

    /// Whether the split buttons may fire: connected, a focused pane in a
    /// tab that isn't closing, and no create already in flight.
    pub fn can_split(&self) -> bool {
        self.data.connected
            && self
                .pending
                .iter()
                .all(|p| !matches!(p, Pending::Create { .. }))
            && self
                .current_tab()
                .is_some_and(|tab| !tab.closing && Self::focused_pane(tab).is_some())
    }

    /// The tab-bar split buttons: a shell session in the focused pane's
    /// checkout, placed as a new pane when the create reply lands.
    pub fn split_active(&mut self, direction: Direction) {
        if !self.data.connected {
            return;
        }
        let Some(tab) = self.current_tab() else {
            return;
        };
        if tab.closing {
            return;
        }
        let Some(pane) = Self::focused_pane(tab) else {
            return;
        };
        let (tab_id, pane_id, session_key) =
            (tab.id.clone(), pane.id.clone(), pane.session.clone());
        if self
            .pending
            .iter()
            .any(|p| matches!(p, Pending::Create { .. }))
        {
            return;
        }
        // A split shares the focused session's checkout — "another shell
        // here", not a new workspace.
        let cwd = self
            .session_by_key(&session_key)
            .map(|view| view.info.cwd.clone())
            .filter(|cwd| !cwd.is_empty())
            .or_else(|| {
                self.editor
                    .checkout
                    .as_ref()
                    .map(|path| path.display().to_string())
            })
            .or_else(|| std::env::var("HOME").ok())
            .unwrap_or_else(|| "/".to_string());
        let project_id = self
            .session_by_key(&session_key)
            .and_then(|view| view.meta.as_ref().and_then(|m| m.project_id.clone()))
            .or_else(|| self.editor.project_id.clone());
        self.launch_session_placed(
            SessionLaunch {
                argv: vec![backend::default_shell()],
                cwd,
                project_id,
                agent: "shell".to_string(),
                title: String::new(),
            },
            Placement::Split {
                tab: tab_id,
                pane: pane_id,
                direction,
            },
        );
    }

    /// Close one pane the way the desktop does: mark it closing, then let
    /// `poll_closes` stop (force) and delete its session while the split
    /// collapses around it. A single-pane tab marks the whole tab closing.
    pub fn begin_close_pane(&mut self, tab: &str, pane: &str) {
        let Some(index) = self.open_tabs.iter().position(|t| t.id == tab) else {
            return;
        };
        let now = self.now_ms;
        let single = matches!(self.open_tabs[index].layout, Layout::Pane(_));
        let marked = {
            let layout = &mut self.open_tabs[index].layout;
            fn mark(layout: &mut Layout, pane: &str, now: i64) -> bool {
                match layout {
                    Layout::Pane(p) => {
                        if p.id == pane {
                            if p.closing_at.is_none() {
                                p.closing_at = Some(now);
                            }
                            p.stop_requested_at = None;
                            p.close_error = None;
                            true
                        } else {
                            false
                        }
                    }
                    Layout::Split(s) => {
                        mark(&mut s.first, pane, now) || mark(&mut s.second, pane, now)
                    }
                }
            }
            mark(layout, pane, now)
        };
        if marked && single {
            self.open_tabs[index].closing = true;
        }
    }

    /// Close a whole tab: every pane begins the stop→delete lifecycle and
    /// the tab drops as its last pane leaves.
    pub fn begin_close_tab(&mut self, index: usize) {
        let Some(tab) = self.open_tabs.get_mut(index) else {
            return;
        };
        mark_layout_closing(&mut tab.layout, self.now_ms);
        tab.closing = true;
    }

    /// The controls bar's Close pane: begin the focused pane's close.
    pub fn close_focused_pane(&mut self) {
        let Some(tab) = self.current_tab() else {
            return;
        };
        let (tab_id, pane_id) = (tab.id.clone(), tab.active_pane.clone());
        self.begin_close_pane(&tab_id, &pane_id);
    }

    /// The sidebar row's Close pane: a placed session begins the pane-close
    /// lifecycle; an unplaced one is stopped (force) or deleted when ended.
    pub fn close_session(&mut self, key: &str) {
        let located = self.open_tabs.iter().find_map(|tab| {
            pane_by_session(&tab.layout, key).map(|p| (tab.id.clone(), p.id.clone()))
        });
        if let Some((tab, pane)) = located {
            self.begin_close_pane(&tab, &pane);
            return;
        }
        match self.session_by_key(key).map(|view| view.info.state) {
            Some(state) if !state.ended() => {
                self.stop_session(key, true);
            }
            _ => {
                self.delete_session(key);
            }
        }
    }

    /// The pane showing `session` has a close failure: record it where the
    /// badge reads it. `false` when no closing pane owns the session — a
    /// manual stop's error stays a notice.
    pub fn set_pane_close_error(&mut self, session: &str, message: &str) -> bool {
        let mut found = false;
        let _ = each_pane_mut(&mut self.open_tabs, session, &mut |pane| {
            if pane.closing_at.is_some() {
                pane.close_error = Some(message.to_string());
                found = true;
            }
        });
        found
    }

    /// The closing pane's stop has hung: its session is still `stopping`
    /// past the force-kill window, so the pane offers Force kill.
    pub fn pane_close_ready(&self, session: &str) -> bool {
        let Some(since) = self.open_tabs.iter().find_map(|tab| {
            collect_panes(&tab.layout)
                .into_iter()
                .find(|pane| pane.session == session)
                .and_then(|pane| pane.stop_requested_at)
        }) else {
            return false;
        };
        self.session_by_key(session)
            .is_some_and(|view| view.info.state == SessionState::Stopping)
            && self.now_ms - since >= FORCE_KILL_AFTER_MS
    }

    /// The pane's close failed or stalled: clear the attempt so the next
    /// poll drives it again — stop a live session, delete a finished one.
    pub fn retry_pane_close(&mut self, session: &str) {
        self.close_inflight.remove(session);
        let _ = each_pane_mut(&mut self.open_tabs, session, &mut |pane| {
            pane.stop_requested_at = None;
            pane.close_error = None;
        });
    }

    /// The stop has hung past the window: ask the daemon to kill the
    /// process group outright and restart the wait — the desktop's Force
    /// kill button.
    pub fn force_kill_pane(&mut self, session: &str) {
        let now = self.now_ms;
        let _ = each_pane_mut(&mut self.open_tabs, session, &mut |pane| {
            pane.stop_requested_at = Some(now);
        });
        self.close_inflight.remove(session);
        self.stop_session(session, true);
    }

    /// Drive every closing pane one step: stop a live session (force),
    /// delete it once it has exited, drop it when the daemon's list lost
    /// it. Runs every poll; each pane's in-flight request is tracked so
    /// nothing is issued twice, and a failed issue simply retries next
    /// poll (a disconnected daemon pauses the whole pass).
    pub fn poll_closes(&mut self) -> bool {
        if !self.data.connected {
            return false;
        }
        let now = self.now_ms;
        // Collect first: the issues below mutate the same trees.
        let mut steps: Vec<(String, CloseStep)> = Vec::new();
        for tab in &self.open_tabs {
            for pane in collect_panes(&tab.layout) {
                if pane.closing_at.is_none() || pane.close_error.is_some() {
                    continue;
                }
                let key = pane.session.clone();
                if self.close_inflight.contains(&key) {
                    continue;
                }
                match self.session_by_key(&key).map(|view| view.info.state) {
                    // The daemon's list lost it: nothing to stop or delete.
                    None => steps.push((key, CloseStep::Drop)),
                    Some(state) if state.ended() => steps.push((key, CloseStep::Delete)),
                    // A session still starting is left alone until it is
                    // running — the desktop waits the same way.
                    Some(SessionState::Created) | Some(SessionState::Starting) => {}
                    Some(_) => {
                        if pane.stop_requested_at.is_none() {
                            steps.push((key, CloseStep::Stop));
                        }
                    }
                }
            }
        }
        let mut changed = false;
        for (key, step) in steps {
            match step {
                CloseStep::Stop => {
                    if self.stop_session(&key, true) {
                        self.close_inflight.insert(key.clone());
                        let _ = each_pane_mut(&mut self.open_tabs, &key, &mut |pane| {
                            pane.stop_requested_at = Some(now);
                        });
                        changed = true;
                    }
                }
                CloseStep::Delete => {
                    if self.delete_session(&key) {
                        self.close_inflight.insert(key);
                        changed = true;
                    }
                }
                CloseStep::Drop => {
                    self.forget_session(&key);
                    changed = true;
                }
            }
        }
        changed
    }

    /// Remove `key`'s pane wherever it lives, collapsing the splits around
    /// it; a tab emptied this way is dropped. Shared by session deletion
    /// and pane-close completion.
    pub fn remove_session_everywhere(&mut self, key: &str) {
        // The active tab is tracked by id: a removal before it shifts the
        // indices, and keeping the bare index would land on the next tab.
        let active_id = self
            .open_tabs
            .get(self.active_tab)
            .map(|tab| tab.id.clone());
        let mut index = 0;
        while index < self.open_tabs.len() {
            let (layout, active_pane, was_closing) = {
                let tab = &self.open_tabs[index];
                (
                    remove_pane(&tab.layout, key),
                    tab.active_pane.clone(),
                    tab.closing,
                )
            };
            match layout {
                Some(layout) => {
                    let panes = collect_panes(&layout);
                    let first_id = panes.first().map(|pane| pane.id.clone());
                    let still_active = panes.iter().any(|pane| pane.id == active_pane);
                    let all_closing = panes.iter().all(|pane| pane.closing_at.is_some());
                    let tab = &mut self.open_tabs[index];
                    tab.layout = layout;
                    if !still_active {
                        tab.active_pane = first_id.unwrap_or_default();
                    }
                    tab.closing = was_closing && all_closing;
                    index += 1;
                }
                None => {
                    self.open_tabs.remove(index);
                }
            }
        }
        self.active_tab = match active_id {
            Some(id) => self
                .open_tabs
                .iter()
                .position(|tab| tab.id == id)
                // The active tab itself closed: the tab that slid into its
                // slot takes focus (or the last one) — the desktop's
                // neighbouring focus.
                .unwrap_or_else(|| self.active_tab.min(self.open_tabs.len().saturating_sub(1))),
            None => self.open_tabs.len().saturating_sub(1),
        };
        self.sync_selection();
    }

    /// The daemon's session list is authoritative: a pane whose session is
    /// no longer listed is dropped like a deleted one.
    pub fn reconcile_panes(&mut self) {
        let missing: Vec<String> = self
            .open_tabs
            .iter()
            .flat_map(|tab| collect_panes(&tab.layout))
            .filter(|pane| self.session_by_key(&pane.session).is_none())
            .map(|pane| pane.session.clone())
            .collect();
        for key in missing {
            self.forget_session(&key);
        }
    }

    /// Drop every local trace of a session the daemon no longer has: its
    /// pane(s), terminal attachment and stored metadata row.
    pub(crate) fn forget_session(&mut self, key: &str) {
        if let Some(att) = self.attachments.remove(key) {
            self.daemon.detach_output(att.term.session);
        }
        self.session_metas.remove(key);
        backend::enqueue(
            &mut self.editor.backend,
            Job::project(Req::DeleteSessionMeta(key.to_string())),
        );
        self.remove_session_everywhere(key);
    }

    // -- Terminal attachments ------------------------------------------------

    /// Attach a terminal to every pane of the active tab — and detach the
    /// ones it replaced. Only the visible tab holds attachments: switching
    /// tabs or views detaches, and coming back resubscribes (the daemon
    /// replays what was missed).
    pub fn sync_terminals(&mut self) -> bool {
        let wanted: Vec<String> = if self.view == crate::View::Terminal {
            self.current_tab()
                .map(|tab| {
                    collect_panes(&tab.layout)
                        .iter()
                        .map(|pane| pane.session.clone())
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let mut changed = false;
        let stale: Vec<String> = self
            .attachments
            .keys()
            .filter(|key| !wanted.contains(*key))
            .cloned()
            .collect();
        for key in stale {
            if let Some(att) = self.attachments.remove(&key) {
                self.daemon.detach_output(att.term.session);
                changed = true;
            }
        }
        let flow = self.daemon.supports_flow_control();
        for key in wanted {
            if self.attachments.contains_key(&key) {
                continue;
            }
            let Some(session) = self.session_id(&key) else {
                continue;
            };
            let Some((chunks, ready)) = self.daemon.attach_output(session, flow) else {
                continue;
            };
            let (cols, rows) = self
                .pane_grids
                .get(&key)
                .copied()
                .unwrap_or((term::TERM_COLS, term::TERM_ROWS));
            self.attachments.insert(
                key.clone(),
                Attachment {
                    term: term::Terminal::new(session, chunks, ready),
                    lines: Vec::new(),
                    cols,
                    rows,
                    pty: (0, 0),
                },
            );
            changed = true;
        }
        changed
    }

    /// Feed every attached pane's terminal and refresh its rendered rows.
    /// Returns whether any grid changed.
    pub fn poll_terminals(&mut self, tx: &mut Tx) -> bool {
        let mut changed = false;
        let ink = self.ink;
        let flow = self.daemon.supports_flow_control();
        let mut acks: Vec<(SessionId, u64)> = Vec::new();
        let mut resizes: Vec<(SessionId, u16, u16)> = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        for (key, att) in self.attachments.iter_mut() {
            // A subscribe the daemon refused means there is nothing to
            // show: drop the attachment; sync_terminals retries later.
            if let Some(Err(error)) = att.term.subscribe_reply() {
                self.notice = Some(format!("the terminal could not attach: {error}"));
                failed.push(key.clone());
                continue;
            }
            att.term.feed();
            // Fit the grid to the measured pane before drawing.
            if att.term.size() != (att.cols, att.rows) {
                att.term.resize(att.cols, att.rows);
            }
            // A repaint that waits for the output to settle asks for the
            // frame that will draw it.
            if let Some(wait) = att.term.hold() {
                tx.wake_after(wait);
            }
            if flow && att.term.unacked() > 0 {
                acks.push((att.term.session, att.term.next_seq));
            }
            if att.cols > 0 && att.rows > 0 && att.pty != (att.cols, att.rows) {
                att.pty = (att.cols, att.rows);
                resizes.push((att.term.session, att.cols as u16, att.rows as u16));
            }
            let lines = att.term.lines(&ink).to_vec();
            if att.lines != lines {
                att.lines = lines;
                changed = true;
            }
        }
        for key in failed {
            if let Some(att) = self.attachments.remove(&key) {
                self.daemon.detach_output(att.term.session);
            }
            changed = true;
        }
        // A flow-controlled stream waits for the acknowledgement, so the
        // bytes each pane processed are reported back.
        for (session, upto_seq) in acks {
            self.ack_terminal(session, upto_seq);
            if let Some(att) = self
                .attachments
                .values_mut()
                .find(|att| att.term.session == session)
            {
                att.term.acknowledge(upto_seq);
            }
        }
        for (session, cols, rows) in resizes {
            self.resize_terminal(session, cols, rows);
        }
        changed
    }

    /// Fit one attachment's grid to its measured pane and tell the daemon
    /// the same size — per pane, so a split never shares a PTY size.
    pub fn resize_attachment(&mut self, key: &str) {
        let send = self.attachments.get_mut(key).and_then(|att| {
            let (cols, rows) = (att.cols, att.rows);
            if att.term.size() != (cols, rows) {
                att.term.resize(cols, rows);
            }
            if cols > 0 && rows > 0 && att.pty != (cols, rows) {
                att.pty = (cols, rows);
                return Some((att.term.session, cols as u16, rows as u16));
            }
            None
        });
        if let Some((session, cols, rows)) = send {
            self.resize_terminal(session, cols, rows);
        }
    }

    /// A subscriber fell behind (or the connection was replaced): drop the
    /// session's output route and take a fresh subscription, which replays
    /// the daemon's retained output. The grid stays.
    pub fn resubscribe_session(&mut self, session: SessionId) -> bool {
        let Some(key) = self
            .attachments
            .iter()
            .find(|(_, att)| att.term.session == session)
            .map(|(key, _)| key.clone())
        else {
            return false;
        };
        self.daemon.detach_output(session);
        let flow = self.daemon.supports_flow_control();
        let Some((chunks, ready)) = self.daemon.attach_output(session, flow) else {
            return false;
        };
        if let Some(att) = self.attachments.get_mut(&key) {
            att.term.resubscribe(chunks, ready);
        }
        true
    }

    /// Every attachment's stream died with the connection: resubscribe all.
    pub fn resubscribe_all(&mut self) {
        let sessions: Vec<SessionId> = self
            .attachments
            .values()
            .map(|att| att.term.session)
            .collect();
        for session in sessions {
            self.resubscribe_session(session);
        }
    }

    /// Every split id the tabs currently hold — measured sizes for splits
    /// that no longer exist are pruned against this.
    pub fn split_ids(&self) -> HashSet<String> {
        fn walk(layout: &Layout, out: &mut HashSet<String>) {
            if let Layout::Split(split) = layout {
                out.insert(split.id.clone());
                walk(&split.first, out);
                walk(&split.second, out);
            }
        }
        let mut out = HashSet::new();
        for tab in &self.open_tabs {
            walk(&tab.layout, &mut out);
        }
        out
    }

    /// A line describing the attachments, for the trace.
    pub fn terminal_trace(&self) -> String {
        let grids: Vec<String> = self
            .attachments
            .values()
            .map(|att| {
                let (cols, rows) = att.term.size();
                format!("{cols}x{rows}")
            })
            .collect();
        format!("panes={} grids={grids:?}", self.attachments.len())
    }

    // -- Split ratios ---------------------------------------------------------

    /// The stored ratio of the split `id` names, wherever it lives.
    fn split_ratio(&self, id: &str) -> Option<f32> {
        self.open_tabs
            .iter()
            .find_map(|tab| find_split(&tab.layout, id).map(|split| split.ratio))
    }

    /// The split's stored ratio and its measured minimum bounds, when the
    /// separator has been laid out at least once.
    fn split_context(&self, id: &str) -> Option<(f32, Option<(f32, f32)>)> {
        self.open_tabs.iter().find_map(|tab| {
            let split = find_split(&tab.layout, id)?;
            let bounds = self
                .split_sizes
                .get(id)
                .copied()
                .map(|size| ratio_bounds(&split.first, &split.second, split.direction, size));
            Some((split.ratio, bounds))
        })
    }

    /// Store a new ratio for the split, clamped to the stored range — the
    /// render clamps again against the measured minimums.
    pub fn set_split_ratio(&mut self, id: &str, ratio: f32) {
        if let Some(split) = self
            .open_tabs
            .iter_mut()
            .find_map(|tab| find_split_mut(&mut tab.layout, id))
        {
            split.ratio = ratio.clamp(RATIO_MIN, RATIO_MAX);
        }
    }

    /// Arrow-key resize: ±1 per press, ±10 for PageUp/PageDown — the same
    /// deltas a slider's `on_step` reports, clamped to the measured
    /// minimums when the separator has a size.
    pub fn adjust_split(&mut self, id: &str, steps: i32) {
        let Some((ratio, bounds)) = self.split_context(id) else {
            return;
        };
        let delta = steps as f32 * KEY_STEP;
        let next = match bounds {
            Some((min, max)) => (ratio + delta).clamp(min, max),
            None => (ratio + delta).clamp(RATIO_MIN, RATIO_MAX),
        };
        self.set_split_ratio(id, next);
    }

    /// Home/End on the separator: the measured minimum bounds, or the
    /// stored range before the first layout.
    pub fn jump_split(&mut self, id: &str, to_max: bool) {
        let Some((_, bounds)) = self.split_context(id) else {
            return;
        };
        match bounds {
            Some((lo, hi)) => self.set_split_ratio(id, if to_max { hi } else { lo }),
            None => self.set_split_ratio(id, if to_max { RATIO_MAX } else { RATIO_MIN }),
        }
    }
}

// ---------------------------------------------------------------------------
// The workspace view — TerminalWorkspace's PaneTree.
// ---------------------------------------------------------------------------

/// The terminal workspace: the active tab's pane tree. Every visible pane
/// draws its own attached session's grid; splits get a draggable,
/// keyboard-resizable separator with the desktop's minimum pane size.
pub fn terminal_workspace(s: &State, ink: &Ink) -> El {
    let Some(tab) = s.open_tabs.get(s.active_tab) else {
        return col()
            .grow(1.0)
            .min_w(0.0)
            .h_full()
            .min_h(0.0)
            .fill(ink.bg)
            .items_center()
            .justify_center()
            .child(theme::txt(
                "No terminal sessions — start one from the sidebar or New tab",
                theme::T12,
                ink.muted,
            ));
    };
    layout_el(s, ink, &tab.layout, tab)
}

/// One node of the pane tree, filling its parent's box.
fn layout_el(s: &State, ink: &Ink, layout: &Layout, tab: &Tab) -> El {
    match layout {
        Layout::Pane(pane) => pane_cell(s, ink, pane, tab),
        Layout::Split(split) => {
            let side = split.direction == Direction::SideBySide;
            let (first_share, second_share) = effective_shares(s, split);
            let sid = split.id.clone();
            let is_side = side;
            (if side { row() } else { col() })
                .w_full()
                .h_full()
                .min_w(0.0)
                .min_h(0.0)
                .child(split_child(
                    s,
                    ink,
                    &split.first,
                    tab,
                    split,
                    first_share,
                    "a",
                ))
                .child(split_separator(ink, split, side))
                .child(split_child(
                    s,
                    ink,
                    &split.second,
                    tab,
                    split,
                    second_share,
                    "b",
                ))
                .on_layout_tx(move |s: &mut State, rect: [f32; 4], tx: &mut Tx| {
                    // The separator's drag and keyboard math need the
                    // split's own size along its axis.
                    let size = if is_side { rect[2] } else { rect[3] };
                    if s.split_sizes.get(&sid).copied() != Some(size) {
                        s.split_sizes.insert(sid.clone(), size);
                        tx.wake_after(Duration::from_millis(16));
                    }
                })
        }
    }
}

/// The ratio actually laid out: clamped to the measured pane minimums —
/// the desktop computes the same bounds inside `ResizableSplit`.
fn effective_shares(s: &State, split: &Split) -> (f32, f32) {
    let raw = split.ratio.clamp(RATIO_MIN, RATIO_MAX);
    let Some(size) = s.split_sizes.get(&split.id).copied() else {
        return (raw, 1.0 - raw);
    };
    let (min, max) = ratio_bounds(&split.first, &split.second, split.direction, size);
    let ratio = raw.clamp(min, max);
    (ratio, 1.0 - ratio)
}

/// One side of a split: a grown box the subtree fills. The cross axis is
/// stated explicitly (`h_full` in a row, `w_full` in a column) — `grow`
/// only owns the main axis.
#[allow(clippy::too_many_arguments)]
fn split_child(
    s: &State,
    ink: &Ink,
    layout: &Layout,
    tab: &Tab,
    split: &Split,
    share: f32,
    tag: &str,
) -> El {
    let side = split.direction == Direction::SideBySide;
    let mut cell = el()
        .key(format!("split:{}:{tag}", split.id))
        .grow(share.max(0.01))
        .min_w(0.0)
        .min_h(0.0);
    cell = if side { cell.h_full() } else { cell.w_full() };
    cell.child(layout_el(s, ink, layout, tab))
}

/// The pointer travel that moves a separator, along the split's axis: a
/// `SideBySide` split's panes sit in a row so the separator drags
/// horizontally, a `Stacked` split's vertically. The off-axis component
/// of a diagonal drag does nothing.
fn axis_delta(dx: f32, dy: f32, side_by_side: bool) -> f32 {
    if side_by_side { dx } else { dy }
}

/// The 8 px drag bar between two panes: pointer drag and arrow keys (with
/// Home/End at the bounds) both move the ratio, clamped so neither pane
/// drops under the minimum size.
fn split_separator(ink: &Ink, split: &Split, side: bool) -> El {
    let sid = split.id.clone();
    let mut sep = el()
        .key(format!("split:{sid}"))
        .role("separator")
        .label("Resize terminal panes")
        .focusable(true)
        .focus_ring(2.0, 1.0, ink.accent);
    sep = if side {
        sep.w(SPLITTER_PX).h_full()
    } else {
        sep.h(SPLITTER_PX).w_full()
    };
    // The visible hairline centred in the hit area.
    sep = sep.child(if side {
        el().absolute((SPLITTER_PX - 1.0) / 2.0, 0.0)
            .w(1.0)
            .h_full()
            .fill(ink.border)
    } else {
        el().absolute(0.0, (SPLITTER_PX - 1.0) / 2.0)
            .w_full()
            .h(1.0)
            .fill(ink.border)
    });
    let drag_id = sid.clone();
    let step_id = sid.clone();
    let limit_id = sid;
    sep.on_drag(
        move |s: &mut State, d: &mut rune::Drag, tx: &mut Tx| match d.phase {
            Phase::Down => {
                d.data = s.split_ratio(&drag_id).unwrap_or(0.5);
            }
            Phase::Move => {
                let Some(size) = s.split_sizes.get(&drag_id).copied() else {
                    return;
                };
                let Some((min, max)) = s.split_context(&drag_id).and_then(|(_, b)| b) else {
                    return;
                };
                let available = (size - SPLITTER_PX).max(1.0);
                let ratio = (d.data + axis_delta(d.dx, d.dy, side) / available).clamp(min, max);
                s.set_split_ratio(&drag_id, ratio);
                tx.instant();
            }
            Phase::Up => {}
        },
    )
    .on_step(move |s: &mut State, steps: i32, _tx: &mut Tx| {
        s.adjust_split(&step_id, steps);
    })
    .on_step_limit(move |s: &mut State, max: bool, _tx: &mut Tx| {
        s.jump_split(&limit_id, max);
    })
}

/// One pane: its session's live grid, the focus ring when active, and the
/// Closing badge while the close lifecycle runs.
fn pane_cell(s: &State, ink: &Ink, pane: &Pane, tab: &Tab) -> El {
    let key = pane.session.clone();
    let pane_key = key.clone();
    let wheel_key = key.clone();
    let pid = pane.id.clone();
    let tid = tab.id.clone();
    let (drop_tid, drop_pid, drop_key, hover_key) =
        (tid.clone(), pid.clone(), key.clone(), key.clone());
    let focused = pane.id == tab.active_pane;
    let title = s
        .session_by_key(&key)
        .map(|view| view.title())
        .unwrap_or_else(|| key.clone());
    // The same cell metrics the single terminal measured.
    let cell_w = rune::text::advance("M", PANE_FONT).max(1.0);
    let (_, line_h) = rune::text::measure("M", &[], PANE_FONT, None);
    let line_h = line_h.max(1.0);
    let lines: Vec<El> = s
        .attachments
        .get(&key)
        .map(|att| term::term_lines_el(&att.lines, ink))
        .unwrap_or_default();
    let border = if s.drop_pane.as_deref() == Some(key.as_str()) {
        ink.accent
    } else if focused {
        ink.accent.alpha(0.4)
    } else {
        ink.accent.alpha(0.0)
    };
    let mut cell = el()
        .key(format!("pane:{}", pane.id))
        .label(title)
        .w_full()
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .fill(ink.bg)
        .border(1.0, border)
        .on_press(move |s: &mut State| s.focus_pane(&tid, &pid))
        .on_drop_files(move |s: &mut State, files, _tx: &mut Tx| {
            s.drop_files(&drop_tid, &drop_pid, &drop_key, files);
        })
        .on_drop_hover(move |s: &mut State, over, _tx: &mut Tx| {
            if over {
                s.drop_pane = Some(hover_key.clone());
            } else if s.drop_pane.as_deref() == Some(hover_key.as_str()) {
                s.drop_pane = None;
            }
        })
        .on_wheel(move |s: &mut State, e: rune::WheelEvent, _tx: &mut Tx| {
            let ink = s.ink;
            let Some(att) = s.attachments.get_mut(&wheel_key) else {
                return false;
            };
            att.term.scroll(e.dy, line_h);
            att.lines = att.term.lines(&ink).to_vec();
            true
        })
        .on_layout_tx(move |s: &mut State, rect: [f32; 4], tx: &mut Tx| {
            // A point to spare on the width: a line a hair too wide would
            // wrap, and a wrapped line is what made content jump.
            let cols = ((rect[2] - 2.0 * PANE_PAD - 1.0) / cell_w)
                .floor()
                .max(20.0) as usize;
            let rows = ((rect[3] - 2.0 * PANE_PAD) / line_h).floor().max(4.0) as usize;
            s.pane_grids.insert(pane_key.clone(), (cols, rows));
            let resized = s.attachments.get_mut(&pane_key).is_some_and(|att| {
                if (att.cols, att.rows) == (cols, rows) {
                    return false;
                }
                att.cols = cols;
                att.rows = rows;
                true
            });
            if resized {
                s.resize_attachment(&pane_key);
                tx.wake_after(Duration::from_millis(16));
            }
        })
        .child(
            col()
                .w_full()
                .h_full()
                .min_h(0.0)
                .min_w(0.0)
                .padding(PANE_PAD)
                .shrink()
                .children(lines),
        );
    // The input method follows the focused, live pane's cursor: the
    // marked text is drawn there and the candidate window opens beside it.
    if focused
        && pane.closing_at.is_none()
        && let Some(att) = s.attachments.get(&key)
        && let Some((row, col)) = att.term.cursor_cell()
    {
        let (x, y) = (
            PANE_PAD + col as f32 * cell_w,
            PANE_PAD + row as f32 * line_h,
        );
        cell = cell.child(el().absolute(x, y).w(cell_w).h(line_h).ime_cursor());
        if !s.preedit.is_empty() {
            cell = cell.child(preedit_el(&s.preedit, ink).absolute(x, y));
        }
    }
    if pane.closing_at.is_some() {
        cell = cell.child(closing_badge(s, ink, pane, &key));
    }
    cell
}

/// The marked text an input method is composing, over the cursor's cell.
fn preedit_el(text: &str, ink: &Ink) -> El {
    el().fill(ink.accent.alpha(0.25))
        .border(1.0, ink.accent.alpha(0.6))
        .child(theme::txt(text, PANE_FONT, ink.text))
}

/// The pane's close progress: "Closing", the last error with Retry, and
/// Force kill once the stop has hung — the desktop's pane badge.
fn closing_badge(s: &State, ink: &Ink, pane: &Pane, key: &str) -> El {
    let mut badge = row()
        .absolute(12.0, 12.0)
        .gap(8.0)
        .items_center()
        .px(10.0)
        .py(6.0)
        .radius(6.0)
        .border(1.0, ink.border)
        .fill(ink.panel.alpha(0.95));
    badge = badge.child(theme::txt("Closing", theme::T11M, ink.text));
    if let Some(error) = &pane.close_error {
        badge = badge.child(
            theme::txt(error, theme::T11, ink.danger)
                .max_w(280.0)
                .truncate(),
        );
        let retry_key = key.to_string();
        badge = badge.child(badge_button(
            "Retry",
            ink.text,
            ink,
            move |s: &mut State| {
                s.retry_pane_close(&retry_key);
            },
        ));
    }
    if s.pane_close_ready(key) {
        let kill_key = key.to_string();
        badge = badge.child(badge_button(
            "Force kill",
            ink.danger,
            ink,
            move |s: &mut State| {
                s.force_kill_pane(&kill_key);
            },
        ));
    }
    badge
}

/// A small pressable label inside the closing badge.
fn badge_button(label: &str, tone: Color, ink: &Ink, f: impl Fn(&mut State) + 'static) -> El {
    el().px(6.0)
        .py(2.0)
        .radius(4.0)
        .role("button")
        .label(label)
        .focusable(true)
        .focus_ring(2.0, 1.0, ink.accent)
        .hover(tone.alpha(0.12))
        .child(theme::txt(label, theme::T11M, tone))
        .on_press(move |s: &mut State| f(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_handle;
    use app_core::error::BridgeError;
    use protocol::session::{SessionInfo as DaemonInfo, SessionState as DaemonState};
    use std::sync::mpsc;

    fn state() -> State {
        State::new(test_handle())
    }

    fn daemon_session(state: DaemonState) -> DaemonInfo {
        DaemonInfo {
            id: SessionId::new(),
            title: "shell".into(),
            argv: vec!["/bin/zsh".into()],
            cwd: "/work/repo".into(),
            state,
            pid: None,
            exit_code: None,
            error: None,
            started_at: 1_700_000_000_000,
            ended_at: None,
        }
    }

    /// A running session in the scene, opened in its own tab.
    fn tabbed_session(s: &mut State, title: &str) -> (String, DaemonInfo) {
        let info = daemon_session(DaemonState::Running);
        let mut info = info;
        info.title = title.into();
        let key = crate::data::SessionInfo::key(info.id);
        s.upsert_session(info.clone());
        s.focus_session_tab(&key);
        (key, info)
    }

    fn pane(layout: &Layout) -> &Pane {
        match layout {
            Layout::Pane(p) => p,
            Layout::Split(_) => panic!("expected a single pane"),
        }
    }

    // -- The tree ------------------------------------------------------------

    #[test]
    fn split_layout_turns_the_pane_into_a_5050_split() {
        let first = new_pane("a");
        let layout = Layout::Pane(first.clone());
        let split = split_layout(&layout, &first.id, Direction::SideBySide, "b").unwrap();
        let Layout::Split(s) = &split else {
            panic!("expected a split");
        };
        assert_eq!(s.direction, Direction::SideBySide);
        assert_eq!(s.ratio, 0.5);
        assert_eq!(collect_panes(&split).len(), 2);
        assert_eq!(pane_by_id(&split, &pane_id("b")).unwrap().session, "b");
    }

    #[test]
    fn split_layout_nests_inside_a_split_and_refuses_bad_targets() {
        let a = new_pane("a");
        let layout = Layout::Pane(a.clone());
        let split = split_layout(&layout, &a.id, Direction::SideBySide, "b").unwrap();
        // Splitting the right pane nests a second split on that side.
        let nested = split_layout(&split, &pane_id("b"), Direction::Stacked, "c").unwrap();
        let Layout::Split(root) = &nested else {
            panic!("expected a split");
        };
        assert!(matches!(&*root.second, Layout::Split(_)));
        assert_eq!(collect_panes(&nested).len(), 3);
        // A missing pane or a closing one refuses the split.
        assert!(split_layout(&nested, "pane-nope", Direction::SideBySide, "d").is_none());
        let mut closing = nested.clone();
        if let Layout::Split(s) = &mut closing
            && let Layout::Pane(p) = &mut *s.first
        {
            p.closing_at = Some(1);
        }
        assert!(split_layout(&closing, &pane_id("a"), Direction::SideBySide, "d").is_none());
    }

    #[test]
    fn remove_pane_collapses_the_split_around_it() {
        let a = new_pane("a");
        let layout =
            split_layout(&Layout::Pane(a.clone()), &a.id, Direction::SideBySide, "b").unwrap();
        // Removing the added pane leaves the original pane alone.
        let left = remove_pane(&layout, "b").unwrap();
        assert!(matches!(left, Layout::Pane(_)));
        assert_eq!(pane(&left).session, "a");
        // Removing the only pane empties the tree.
        assert!(remove_pane(&left, "a").is_none());
        // In a three-pane tree, removing the middle keeps the outer split.
        let three = split_layout(&layout, &pane_id("b"), Direction::Stacked, "c").unwrap();
        let two = remove_pane(&three, "b").unwrap();
        assert_eq!(collect_panes(&two).len(), 2);
        assert!(pane_by_session(&two, "b").is_none());
    }

    #[test]
    fn minimum_dimension_and_ratio_bounds_follow_the_desktop() {
        let a = new_pane("a");
        let layout =
            split_layout(&Layout::Pane(a.clone()), &a.id, Direction::SideBySide, "b").unwrap();
        let Layout::Split(s) = &layout else {
            panic!("expected a split");
        };
        // Along the split's axis the minimums add with the separator;
        // across it they take the larger child.
        assert_eq!(
            minimum_dimension(&layout, true),
            2.0 * MIN_PANE_PX + SPLITTER_PX
        );
        assert_eq!(minimum_dimension(&layout, false), MIN_PANE_PX);
        // At 800 pt the bounds keep both panes >= MIN_PANE_PX.
        let (min, max) = ratio_bounds(&s.first, &s.second, s.direction, 800.0);
        assert!((min - MIN_PANE_PX / (800.0 - SPLITTER_PX)).abs() < 0.001);
        assert!((max - (1.0 - MIN_PANE_PX / (800.0 - SPLITTER_PX))).abs() < 0.001);
        assert!(max > min);
        // Too small for both minimums: recentre.
        let (min, max) = ratio_bounds(&s.first, &s.second, s.direction, 100.0);
        assert_eq!((min, max), (0.5, 0.5));
    }

    // -- Focus and selection ---------------------------------------------------

    #[test]
    fn activating_a_tab_selects_its_active_pane() {
        let mut s = state();
        let (a, _) = tabbed_session(&mut s, "first");
        let (b, _) = tabbed_session(&mut s, "second");
        assert_eq!(s.selected_session.as_deref(), Some(b.as_str()));
        s.activate_tab(0);
        assert_eq!(s.selected_session.as_deref(), Some(a.as_str()));
        assert_eq!(s.active_workspace.is_some(), s.session_by_key(&a).is_some());
    }

    // -- Placement -------------------------------------------------------------

    #[test]
    fn a_split_placement_lands_the_new_session_as_a_pane() {
        let mut s = state();
        let (a, _) = tabbed_session(&mut s, "first");
        let info = daemon_session(DaemonState::Running);
        let key = crate::data::SessionInfo::key(info.id);
        // Drive the real reply path: a Pending::Create whose placement is
        // the active tab's only pane.
        let (reply_tx, reply_rx) = mpsc::channel();
        let tab = s.open_tabs[0].id.clone();
        let pane = pane_id(&a);
        s.pending.push(Pending::Create {
            reply: reply_rx,
            launch: SessionLaunch {
                argv: vec!["/bin/zsh".into()],
                cwd: "/work/repo".into(),
                project_id: None,
                agent: "shell".into(),
                title: String::new(),
            },
            placement: Placement::Split {
                tab,
                pane,
                direction: Direction::SideBySide,
            },
        });
        reply_tx.send(Ok(info.clone())).unwrap();
        assert!(s.drain_pending());
        // One tab, a split layout, the new session focused.
        assert_eq!(s.open_tabs.len(), 1);
        let tab = &s.open_tabs[0];
        assert!(matches!(tab.layout, Layout::Split(_)));
        assert_eq!(collect_panes(&tab.layout).len(), 2);
        assert_eq!(tab.active_pane, pane_id(&key));
        assert_eq!(s.selected_session.as_deref(), Some(key.as_str()));
    }

    #[test]
    fn a_split_placement_with_a_missing_tab_falls_back_to_a_new_tab() {
        let mut s = state();
        let (_a, _) = tabbed_session(&mut s, "first");
        let info = daemon_session(DaemonState::Running);
        let key = crate::data::SessionInfo::key(info.id);
        let (reply_tx, reply_rx) = mpsc::channel();
        s.pending.push(Pending::Create {
            reply: reply_rx,
            launch: SessionLaunch {
                argv: vec!["/bin/zsh".into()],
                cwd: "/work/repo".into(),
                project_id: None,
                agent: "shell".into(),
                title: String::new(),
            },
            placement: Placement::Split {
                tab: "tab-gone".into(),
                pane: "pane-gone".into(),
                direction: Direction::SideBySide,
            },
        });
        reply_tx.send(Ok(info)).unwrap();
        s.drain_pending();
        assert_eq!(s.open_tabs.len(), 2);
        assert_eq!(s.selected_session.as_deref(), Some(key.as_str()));
        assert!(matches!(s.open_tabs[1].layout, Layout::Pane(_)));
    }

    #[test]
    fn a_tab_placement_opens_and_selects_a_fresh_tab() {
        let mut s = state();
        let (a, _) = tabbed_session(&mut s, "first");
        let info = daemon_session(DaemonState::Running);
        let key = crate::data::SessionInfo::key(info.id);
        let (reply_tx, reply_rx) = mpsc::channel();
        s.pending.push(Pending::Create {
            reply: reply_rx,
            launch: SessionLaunch {
                argv: vec!["/bin/zsh".into()],
                cwd: "/work/repo".into(),
                project_id: None,
                agent: "shell".into(),
                title: String::new(),
            },
            placement: Placement::Tab,
        });
        reply_tx.send(Ok(info)).unwrap();
        s.drain_pending();
        assert_eq!(s.open_tabs.len(), 2);
        assert_eq!(s.active_tab, 1);
        assert_eq!(s.selected_session.as_deref(), Some(key.as_str()));
        assert_ne!(s.open_tabs[1].id, s.open_tabs[0].id);
        let _ = a;
    }

    // -- The close lifecycle -----------------------------------------------------

    #[test]
    fn closing_a_pane_stops_its_session_then_waits() {
        let mut s = state();
        s.data.connected = true;
        let (key, _) = tabbed_session(&mut s, "first");
        let tab = s.open_tabs[0].id.clone();
        let pane = pane_id(&key);
        s.begin_close_pane(&tab, &pane);
        // The pane is marked closing and the tab (single pane) too.
        let marked = match &s.open_tabs[0].layout {
            Layout::Pane(p) => p.closing_at.is_some(),
            _ => false,
        };
        assert!(marked);
        assert!(s.open_tabs[0].closing);
        // The poll issues the force stop once.
        assert!(s.poll_closes());
        let pane = match &s.open_tabs[0].layout {
            Layout::Pane(p) => p,
            _ => panic!("expected a pane"),
        };
        assert!(pane.stop_requested_at.is_some());
        assert!(s.close_inflight.contains(&key));
        assert!(s.pending.iter().any(|p| matches!(p, Pending::Stop { .. })));
        // A second poll does not re-issue while the stop is in flight.
        let pending_before = s.pending.len();
        assert!(!s.poll_closes());
        assert_eq!(s.pending.len(), pending_before);
    }

    #[test]
    fn an_exited_closing_session_is_deleted_and_the_pane_collapses() {
        let mut s = state();
        s.data.connected = true;
        let (a, _) = tabbed_session(&mut s, "first");
        let (b, mut info_b) = tabbed_session(&mut s, "second");
        // B's session already exited: its close goes straight to delete.
        info_b.state = DaemonState::Exited;
        s.upsert_session(info_b);
        s.begin_close_tab(1);
        assert!(s.poll_closes());
        assert!(
            s.pending
                .iter()
                .any(|p| matches!(p, Pending::Delete { key: k, .. } if *k == b))
        );
        // The delete reply lands: the tab and its pane are gone.
        let (reply_tx, reply_rx) = mpsc::channel();
        // The auto-issued pending resolves Err on a disconnected daemon;
        // swap in a resolvable one to drive the success path.
        s.pending.retain(|p| !matches!(p, Pending::Delete { .. }));
        s.pending.push(Pending::Delete {
            reply: reply_rx,
            key: b.clone(),
        });
        reply_tx.send(Ok(serde_json::json!({}))).unwrap();
        assert!(s.drain_pending());
        assert_eq!(s.open_tabs.len(), 1);
        assert!(!s.session_placed(&b));
        assert!(s.session_placed(&a));
        assert_eq!(s.selected_session.as_deref(), Some(a.as_str()));
    }

    #[test]
    fn a_delete_error_stays_on_the_pane_until_retry() {
        let mut s = state();
        s.data.connected = true;
        let (key, mut info) = tabbed_session(&mut s, "first");
        info.state = DaemonState::Exited;
        s.upsert_session(info);
        let tab = s.open_tabs[0].id.clone();
        let pane = pane_id(&key);
        s.begin_close_pane(&tab, &pane);
        assert!(s.poll_closes());
        // The daemon refuses the delete: the pane keeps the error.
        let (reply_tx, reply_rx) = mpsc::channel();
        s.pending.retain(|p| !matches!(p, Pending::Delete { .. }));
        s.pending.push(Pending::Delete {
            reply: reply_rx,
            key: key.clone(),
        });
        reply_tx
            .send(Err(BridgeError::new(
                app_core::error::Code::Daemon(protocol::message::ErrorCode::Malformed),
                "cannot delete",
            )))
            .unwrap();
        assert!(s.drain_pending());
        let pane = match &s.open_tabs[0].layout {
            Layout::Pane(p) => p,
            _ => panic!("expected a pane"),
        };
        assert!(pane.close_error.is_some());
        // While the error stands the poll leaves the pane alone.
        let pending_before = s.pending.len();
        assert!(!s.poll_closes());
        assert_eq!(s.pending.len(), pending_before);
        // Retry clears it and the poll issues the delete again.
        s.retry_pane_close(&key);
        assert!(s.poll_closes());
        assert!(
            s.pending
                .iter()
                .any(|p| matches!(p, Pending::Delete { .. }))
        );
    }

    #[test]
    fn removing_a_session_everywhere_collapses_its_pane() {
        let mut s = state();
        let (a, _) = tabbed_session(&mut s, "first");
        // Split A's tab with a third session placed like a create reply.
        let info = daemon_session(DaemonState::Running);
        let c = crate::data::SessionInfo::key(info.id);
        s.upsert_session(info.clone());
        let tab = s.open_tabs[0].id.clone();
        let pane_a = pane_id(&a);
        let (reply_tx, reply_rx) = mpsc::channel();
        s.pending.push(Pending::Create {
            reply: reply_rx,
            launch: SessionLaunch {
                argv: vec!["/bin/zsh".into()],
                cwd: "/work/repo".into(),
                project_id: None,
                agent: "shell".into(),
                title: String::new(),
            },
            placement: Placement::Split {
                tab,
                pane: pane_a.clone(),
                direction: Direction::SideBySide,
            },
        });
        reply_tx.send(Ok(info)).unwrap();
        s.drain_pending();
        assert!(matches!(s.open_tabs[0].layout, Layout::Split(_)));
        // Deleting C collapses the split back to A's single pane and the
        // selection lands on A — the pane that remains.
        s.remove_session_everywhere(&c);
        assert_eq!(s.open_tabs.len(), 1);
        assert!(matches!(s.open_tabs[0].layout, Layout::Pane(_)));
        assert_eq!(s.selected_session.as_deref(), Some(a.as_str()));
        assert!(!s.session_placed(&c));
    }

    #[test]
    fn the_daemon_list_reconcile_drops_orphaned_panes() {
        let mut s = state();
        let (a, _) = tabbed_session(&mut s, "first");
        // The daemon's list no longer knows the session: the pane goes.
        s.data.sessions.clear();
        s.reconcile_panes();
        assert!(s.open_tabs.is_empty());
        assert_eq!(s.selected_session, None);
        let _ = a;
    }

    #[test]
    fn a_new_attachment_starts_at_the_measured_pane_grid() {
        let mut s = state();
        let (a, _) = tabbed_session(&mut s, "first");
        s.data.connected = true;
        // The pane was measured before the attachment existed.
        s.pane_grids.insert(a.clone(), (90, 20));
        assert!(s.sync_terminals());
        let att = &s.attachments[&a];
        assert_eq!((att.cols, att.rows), (90, 20));
    }

    #[test]
    fn terminal_attachments_follow_the_active_tab_only() {
        let mut s = state();
        let (a, info_a) = tabbed_session(&mut s, "first");
        let (b, _) = tabbed_session(&mut s, "second");
        s.data.connected = true;
        // B's tab was focused last: only its pane attaches, not A's.
        assert!(s.sync_terminals());
        assert!(!s.attachments.contains_key(&a));
        assert!(s.attachments.contains_key(&b));
        // Switching back to A's tab detaches B and attaches A.
        s.activate_tab(0);
        assert!(s.sync_terminals());
        assert!(s.attachments.contains_key(&a));
        assert!(!s.attachments.contains_key(&b));
        // Leaving the Terminal view detaches everything.
        s.view = crate::View::Editor;
        assert!(s.sync_terminals());
        assert!(s.attachments.is_empty());
        let _ = info_a;
    }

    #[test]
    fn attachments_render_their_own_output() {
        let mut s = state();
        let (key, info) = tabbed_session(&mut s, "first");
        let (tx_out, rx) = mpsc::channel();
        let (_ready_tx, ready_rx) = mpsc::channel();
        let mut terminal = term::Terminal::new(info.id, rx, ready_rx);
        tx_out
            .send(crate::daemon::Output {
                seq: 0,
                data: b"hello\r\n".to_vec(),
            })
            .unwrap();
        terminal.feed();
        s.attachments.insert(
            key.clone(),
            Attachment {
                term: terminal,
                lines: Vec::new(),
                cols: term::TERM_COLS,
                rows: term::TERM_ROWS,
                pty: (0, 0),
            },
        );
        // poll_terminals needs a Tx; drive it through a runtime update.
        let mut rt = rune::Runtime::new(
            s,
            |_: &State, _: &mut rune::Cx<State>| el(),
            (800.0, 600.0),
            rune::Env::new(true, 2.0),
        );
        rt.update(|s, tx| {
            s.poll_terminals(tx);
        });
        rt.update(|s, _| {
            let att = s.attachments.get(&key).unwrap();
            assert!(!att.lines.is_empty());
            assert_eq!(att.lines[0].text().trim_end(), "hello");
        });
    }

    #[test]
    fn split_ratio_clamps_and_measured_bounds_apply() {
        let mut s = state();
        let (a, _) = tabbed_session(&mut s, "first");
        let info = daemon_session(DaemonState::Running);
        let key = crate::data::SessionInfo::key(info.id);
        let tab = s.open_tabs[0].id.clone();
        let pane = pane_id(&a);
        let (reply_tx, reply_rx) = mpsc::channel();
        s.pending.push(Pending::Create {
            reply: reply_rx,
            launch: SessionLaunch {
                argv: vec!["/bin/zsh".into()],
                cwd: "/work/repo".into(),
                project_id: None,
                agent: "shell".into(),
                title: String::new(),
            },
            placement: Placement::Split {
                tab,
                pane,
                direction: Direction::SideBySide,
            },
        });
        reply_tx.send(Ok(info)).unwrap();
        s.drain_pending();
        let split_id = match &s.open_tabs[0].layout {
            Layout::Split(split) => split.id.clone(),
            _ => panic!("expected a split"),
        };
        // Out-of-range stores clamp to 0.05..0.95.
        s.set_split_ratio(&split_id, 2.0);
        assert_eq!(s.split_ratio(&split_id), Some(0.95));
        s.set_split_ratio(&split_id, -1.0);
        assert_eq!(s.split_ratio(&split_id), Some(0.05));
        // Arrow keys step the ratio.
        s.set_split_ratio(&split_id, 0.5);
        s.adjust_split(&split_id, 1);
        assert!((s.split_ratio(&split_id).unwrap() - 0.525).abs() < 1e-6);
        s.adjust_split(&split_id, -2);
        assert!((s.split_ratio(&split_id).unwrap() - 0.475).abs() < 1e-6);
        // Home/End jump to the stored bounds before any layout.
        s.jump_split(&split_id, true);
        assert_eq!(s.split_ratio(&split_id), Some(0.95));
        s.jump_split(&split_id, false);
        assert_eq!(s.split_ratio(&split_id), Some(0.05));
        let _ = key;
    }

    #[test]
    fn removing_an_earlier_tab_keeps_the_active_one_selected() {
        let mut s = state();
        let (a, _) = tabbed_session(&mut s, "first");
        let (b, _) = tabbed_session(&mut s, "second");
        let (_c, _) = tabbed_session(&mut s, "third");
        s.activate_tab(1); // B
        assert_eq!(s.selected_session.as_deref(), Some(b.as_str()));
        // Deleting A drops its tab before B's: the index shifts, and the
        // selection must stay on B — the bug landed it on C.
        s.remove_session_everywhere(&a);
        assert_eq!(s.open_tabs.len(), 2);
        assert!(!s.session_placed(&a));
        assert_eq!(s.selected_session.as_deref(), Some(b.as_str()));
        // Keyboard input still targets B's session.
        assert_eq!(s.focused_terminal_session(), s.session_id(&b));
    }

    #[test]
    fn a_reconcile_that_drops_neighbours_keeps_the_active_tab() {
        let mut s = state();
        let (_a, _) = tabbed_session(&mut s, "first");
        let (b, _) = tabbed_session(&mut s, "second");
        let (_c, _) = tabbed_session(&mut s, "third");
        let (d, _) = tabbed_session(&mut s, "fourth");
        s.activate_tab(1); // B
        // The daemon's list lost A and C: their tabs drop from both sides
        // of B's, and B stays the selected tab — the bug left it on D.
        s.data
            .sessions
            .retain(|view| view.info.id == b || view.info.id == d);
        s.reconcile_panes();
        assert_eq!(s.open_tabs.len(), 2);
        assert!(s.session_placed(&b));
        assert!(s.session_placed(&d));
        assert_eq!(s.selected_session.as_deref(), Some(b.as_str()));
        assert_eq!(s.focused_terminal_session(), s.session_id(&b));
    }

    #[test]
    fn removing_the_active_tab_focuses_the_next_survivor() {
        let mut s = state();
        let (_a, _) = tabbed_session(&mut s, "first");
        let (b, _) = tabbed_session(&mut s, "second");
        let (c, _) = tabbed_session(&mut s, "third");
        s.activate_tab(1); // B
        // The active tab itself goes: the tab that slid into its slot
        // takes the focus — C here, like the desktop's neighbouring focus.
        s.remove_session_everywhere(&b);
        assert_eq!(s.open_tabs.len(), 2);
        assert_eq!(s.selected_session.as_deref(), Some(c.as_str()));
    }

    #[test]
    fn the_separator_drag_moves_only_along_the_splits_axis() {
        // A SideBySide split's separator drags horizontally (dx); a
        // Stacked one's vertically (dy). A diagonal drag's off-axis part
        // moves nothing.
        assert_eq!(axis_delta(120.0, -40.0, true), 120.0);
        assert_eq!(axis_delta(120.0, -40.0, false), -40.0);
        assert_eq!(axis_delta(0.0, 60.0, true), 0.0);
        assert_eq!(axis_delta(75.0, 0.0, false), 0.0);
    }

    #[test]
    fn keyboard_input_goes_to_the_focused_panes_session() {
        let mut s = state();
        let (a, _) = tabbed_session(&mut s, "first");
        // The focused pane's session is the keyboard target.
        assert_eq!(s.focused_terminal_session(), s.session_id(&a));
        // Focus the other pane after a split: the target follows.
        let info = daemon_session(DaemonState::Running);
        let key = crate::data::SessionInfo::key(info.id);
        let tab = s.open_tabs[0].id.clone();
        let pane = pane_id(&a);
        let (reply_tx, reply_rx) = mpsc::channel();
        s.pending.push(Pending::Create {
            reply: reply_rx,
            launch: SessionLaunch {
                argv: vec!["/bin/zsh".into()],
                cwd: "/work/repo".into(),
                project_id: None,
                agent: "shell".into(),
                title: String::new(),
            },
            placement: Placement::Split {
                tab,
                pane,
                direction: Direction::SideBySide,
            },
        });
        reply_tx.send(Ok(info)).unwrap();
        s.drain_pending();
        assert_eq!(s.focused_terminal_session(), s.session_id(&key));
        // Focus the first pane again: the target moves back.
        s.focus_pane(&s.open_tabs[0].id.clone(), &pane_id(&a));
        assert_eq!(s.focused_terminal_session(), s.session_id(&a));
        assert_eq!(s.selected_session.as_deref(), Some(a.as_str()));
    }
}
