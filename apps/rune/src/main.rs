//! goko-rune: a Rune (wgpu/winit) port of the Goko desktop shell — header,
//! project sidebar, workspace tab strip, controls and a terminal stub, all
//! driven by mock data. Real daemon/Tauri wiring lands in a later milestone.

mod chrome;
mod data;
mod sidebar;
mod term;
mod theme;

use std::collections::HashMap;

use rune::{col, row, AppSpec, Cx, El};

use crate::data::Data;
use crate::theme::Ink;

pub struct State {
    ink: Ink,
    data: Data,
    lines: Vec<term::TermLine>,
    /// Project id → expanded; absent means expanded (ProjectTree default).
    expanded: HashMap<String, bool>,
    selected_session: Option<String>,
    active_workspace: Option<String>,
    /// Open workspace tabs (titles); seeded from `Data::tabs`.
    open_tabs: Vec<String>,
    active_tab: usize,
    /// "Worktrees" group header open per project id; absent means open.
    worktree_groups: HashMap<String, bool>,
    /// Per-worktree session list open, keyed by worktree path; absent = open.
    worktrees_open: HashMap<String, bool>,
}

impl State {
    fn new() -> State {
        let ink = Ink::new();
        let data = data::mock_data();
        let lines = term::real_term_lines(&ink);
        // The second project starts collapsed so its Count badge + right
        // chevron render; "goko" is expanded and owns the active workspace.
        let mut expanded = HashMap::new();
        expanded.insert("proj-notes".to_string(), false);
        let open_tabs = data.tabs.clone();
        let active_tab = data.active_tab;
        State {
            ink,
            data,
            lines,
            expanded,
            selected_session: Some("sess-devin-spike".to_string()),
            active_workspace: Some(data::checkout_key("proj-goko", data::GOKO_ROOT)),
            open_tabs,
            active_tab,
            worktree_groups: HashMap::new(),
            worktrees_open: HashMap::new(),
        }
    }
}

/// Root: header full-width, then [sidebar 320 | 8 px separator | workspace
/// column (tab bar / controls / terminal)] — App.tsx + ResizableSplit.
fn view(s: &State, _cx: &mut Cx<State>) -> El {
    let ink = &s.ink;
    col()
        .w_full()
        .h_full()
        .fill(ink.bg)
        .child(chrome::header(s, ink))
        .child(
            row()
                .grow(1.0)
                .min_h(0.0)
                .w_full()
                .child(sidebar::sidebar(s, ink))
                .child(sidebar::separator(ink))
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .h_full()
                        .min_h(0.0)
                        .child(chrome::tab_bar(s, ink))
                        .child(chrome::controls_bar(s, ink))
                        .child(term::terminal_area(s, ink)),
                ),
        )
}

fn main() {
    rune::run(AppSpec {
        families: ("Inter", "ui-monospace"),
        min_size: Some((760.0, 480.0)),
        ..AppSpec::new("Goko", (1512.0, 814.0), State::new, view)
    });
}
