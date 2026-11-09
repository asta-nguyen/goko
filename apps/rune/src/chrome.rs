//! The app chrome around the sidebar: the 48 px header, the workspace tab bar,
//! and the controls strip above the terminal. Ported from fidelity.rs with the
//! real components' measurements (App.tsx header, TerminalWorkspace tab strip,
//! SessionControls breadcrumb + state badge + elapsed + actions).

use rune::{el, row, spacer, El};

use crate::data;
use crate::theme::{self, Ink, T11M, T12, T12M, T12TN, T14, T14M, T14S};
use crate::{State, View};

/// App.tsx header: `flex h-12 items-center gap-3 border-b border-border
/// bg-panel px-4` (48 px, gap 12, px 16).
pub fn header(s: &State, ink: &Ink) -> El {
    let mut right = row().gap(8.0).items_center();
    for u in &s.data.usage {
        right = right.child(theme::usage_chip(u.name, u.limits, ink));
    }
    // ConnectionBadge only renders when the daemon is not connected; the mock
    // is connected, so it is absent — kept for the real-data swap.
    if !s.data.connected {
        right = right.child(
            row()
                .h(20.0)
                .px(8.0)
                .gap(6.0)
                .items_center()
                .radius(10.0)
                .border(1.0, ink.danger.alpha(0.3))
                .fill(ink.danger.alpha(0.1))
                .child(el().size(6.0, 6.0).radius(3.0).fill(ink.danger))
                .child(theme::txt("Disconnected", T11M, ink.danger)),
        );
    }
    // Settings toggles the app-level settings mode like App.tsx's gear;
    // pressing it again returns to the terminal.
    right = right.child(
        el().size(28.0, 28.0)
            .radius(8.0)
            .center()
            .hover(ink.bg)
            .child(theme::ic(&theme::SETTINGS, 16.0, ink.text))
            .on_press(|s: &mut State| {
                s.view = if s.view == View::Settings {
                    View::Terminal
                } else {
                    View::Settings
                };
            }),
    );

    // Persistent connection dot beside the wordmark; the pill on the right
    // only appears while disconnected, matching the React header.
    let conn = if s.data.connected {
        ink.success
    } else {
        ink.danger
    };

    row()
        .w_full()
        .h(48.0)
        .fill(ink.panel)
        .border_sides([0.0, 0.0, 1.0, 0.0], ink.border)
        .px(16.0)
        .gap(12.0)
        .items_center()
        .child(
            row()
                .gap(10.0)
                .items_center()
                .child(
                    el().size(28.0, 28.0)
                        .radius(8.0)
                        .fill(ink.primary)
                        .center()
                        .child(theme::ic(&theme::GOKO_MARK, 16.0, ink.white)),
                )
                .child(
                    row()
                        .gap(8.0)
                        .items_center()
                        .child(theme::txt("goko", T14S, ink.text))
                        .child(el().size(6.0, 6.0).radius(3.0).fill(conn)),
                ),
        )
        .child(spacer())
        .child(right)
}

// ---------------------------------------------------------------------------
// Tab bar — TerminalWorkspace's strip: `min-h-10 border-b bg-panel px-2 gap-2`
// ---------------------------------------------------------------------------

pub fn tab_bar(s: &State, ink: &Ink) -> El {
    let mut tabs = row().grow(1.0).min_w(0.0).items_center().gap(4.0);
    for (i, title) in s.open_tabs.iter().enumerate() {
        tabs = tabs.child(tab_el(title, i, i == s.active_tab, ink));
    }
    row()
        .w_full()
        .min_h(40.0)
        .items_center()
        .gap(8.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], ink.border)
        .fill(ink.panel)
        .px(8.0)
        .child(tabs)
        .child(
            theme::btn_sm("New tab", Some((&theme::PLUS, 14.0)), false, ink).on_press(
                |s: &mut State| {
                    // Mock session_create: a shell session titled like the
                    // tab, then the sidebar's activate-and-focus path.
                    let title = data::spawn_shell(&mut s.data);
                    s.open_tabs.push(title);
                    s.active_tab = s.open_tabs.len() - 1;
                    sync_selection(s);
                },
            ),
        )
        .child(theme::btn_icon(&theme::COLUMNS_2, ink))
        .child(theme::btn_icon(&theme::ROWS_2, ink))
        .child(view_switch(s, ink))
}

/// Point selection at whatever `active_tab` resolves to after the tab list
/// changed; clears it when the active title has no session. Shared by tab
/// close/select, New tab and Close pane.
fn sync_selection(s: &mut State) {
    let found = s.open_tabs.get(s.active_tab).and_then(|t| {
        s.data
            .sessions
            .iter()
            .find(|v| v.title() == *t)
            .map(|sv| (sv.info.id.clone(), data::workspace_for_session(&s.data, sv)))
    });
    match found {
        Some((id, ws)) => {
            s.selected_session = Some(id);
            s.active_workspace = Some(ws);
        }
        None => {
            s.selected_session = None;
            s.active_workspace = None;
        }
    }
}

/// Session tab pill: `bg-bg/50 rounded-lg`; the active title gets `bg-bg
/// font-medium`. Close X on every tab. Clicking a tab selects the session
/// with that title; the X closes the tab.
fn tab_el(title: &str, index: usize, active: bool, ink: &Ink) -> El {
    let mut b = row()
        .h(32.0)
        .gap(6.0)
        .items_center()
        .px(10.0)
        .radius(8.0)
        .fill(if active { ink.bg } else { ink.bg.alpha(0.5) })
        .hover(ink.bg);
    b = if active {
        b.child(theme::txt(title, T12M, ink.text).max_w(160.0).truncate())
    } else {
        b.child(theme::txt(title, T12, ink.muted).max_w(160.0).truncate())
    };
    b.child(
        el().size(28.0, 28.0)
            .center()
            .radius(4.0)
            .hover(ink.bg)
            .child(theme::ic(&theme::X, 14.0, ink.muted))
            .on_press(move |s: &mut State| {
                s.open_tabs.remove(index);
                if index < s.active_tab {
                    s.active_tab -= 1;
                }
                if s.active_tab >= s.open_tabs.len() {
                    s.active_tab = s.open_tabs.len().saturating_sub(1);
                }
                sync_selection(s);
            }),
    )
    .on_press(move |s: &mut State| {
        s.active_tab = index;
        sync_selection(s);
    })
}

/// ViewSwitch: Terminal pressed / Editor unpressed — a bordered `bg-bg`
/// track with a raised `bg-panel` active item, icons included (ViewSwitch.tsx).
fn view_switch(s: &State, ink: &Ink) -> El {
    row()
        .h(34.0)
        .gap(2.0)
        .items_center()
        .padding(2.0)
        .radius(8.0)
        .border(1.0, ink.border)
        .fill(ink.bg)
        .child(switch_item(
            &theme::SQUARE_TERMINAL,
            "Terminal",
            View::Terminal,
            s.view == View::Terminal,
            ink,
        ))
        .child(switch_item(
            &theme::BOOK_OPEN,
            "Editor",
            View::Editor,
            s.view == View::Editor,
            ink,
        ))
}

fn switch_item(
    icon: &'static rune::Icon,
    label: &str,
    target: View,
    active: bool,
    ink: &Ink,
) -> El {
    let tone = if active { ink.text } else { ink.muted };
    let mut b = row().h(28.0).px(8.0).gap(6.0).items_center().radius(6.0);
    if active {
        b = b.fill(ink.panel);
    }
    b.child(theme::ic(icon, 14.0, tone))
        .child(theme::txt(label, if active { T12M } else { T12 }, tone))
        .on_press(move |s: &mut State| {
            s.view = target;
        })
}

// ---------------------------------------------------------------------------
// Controls strip — SessionControls: breadcrumb + StateBadge + elapsed + Stop /
// Close pane. Renders only while a session is selected (the real component
// returns null without a focused pane).
// ---------------------------------------------------------------------------

pub fn controls_bar(s: &State, ink: &Ink) -> El {
    let Some(sv) = s
        .selected_session
        .as_deref()
        .and_then(|id| s.data.sessions.iter().find(|v| v.info.id == id))
    else {
        return el();
    };
    let info = &sv.info;
    let title = sv.title();

    // Breadcrumb: "<project> › " muted + agent glyph "<agent> · " muted +
    // "<title>" 14 medium — `min-w-0 truncate text-sm`. Segments are separate
    // elements because rich text cannot embed the agent icon; only the title
    // truncates, keeping the project/agent context visible.
    let project_name = sv
        .meta
        .as_ref()
        .and_then(|m| m.project_id.as_deref())
        .and_then(|pid| s.data.projects.iter().find(|p| p.id == pid))
        .map(|p| p.name.as_str());
    let agent = sv
        .agent_label(&s.data.agents)
        .filter(|a| !a.eq_ignore_ascii_case(title.as_str()));

    let mut crumb = row().min_w(0.0).shrink().items_center();
    if let Some(pn) = project_name {
        crumb = crumb.child(theme::txt(&format!("{pn} › "), T14, ink.muted));
    }
    if let Some(a) = agent {
        let (glyph, tint) = theme::agent_glyph(sv.agent_id(), ink);
        crumb = crumb.child(theme::ic(glyph, 14.0, tint)).child(theme::txt(
            &format!(" {a} · "),
            T14,
            ink.muted,
        ));
    }
    crumb = crumb.child(theme::txt(&title, T14M, ink.text).truncate());

    // StateBadge + StateDot's tone dot: `rounded border border-border px-1.5
    // py-0.5 text-xs` + TONE, with the activity-aware dot ahead of the label.
    let badge = row()
        .radius(4.0)
        .border(1.0, ink.border)
        .px(6.0)
        .py(2.0)
        .gap(4.0)
        .items_center()
        .child(
            el().size(6.0, 6.0)
                .radius(3.0)
                .fill(info.state.dot_color(sv.activity, ink)),
        )
        .child(theme::txt(
            info.state.label(),
            T12,
            info.state.badge_tone(ink),
        ));

    // Elapsed: ticks while live, frozen at ended_at otherwise.
    let live = info.state.controls_live();
    let end = if live {
        data::MOCK_NOW_MS
    } else {
        info.ended_at.unwrap_or(data::MOCK_NOW_MS)
    };
    let elapsed = theme::txt(
        &data::format_elapsed(end - info.started_at),
        T12TN,
        ink.muted,
    );

    // Stop is disabled when the state is not controllable or disconnected.
    // Mock stop_session: a live session exits with a frozen ended_at.
    let mut stop = theme::btn_sm("Stop", None, true, ink);
    if !live || !s.data.connected {
        stop = stop.opacity(0.5);
    }
    let stop = stop.on_press(|s: &mut State| {
        let Some(id) = s.selected_session.clone() else {
            return;
        };
        if let Some(sv) = s.data.sessions.iter_mut().find(|v| v.info.id == id) {
            if sv.info.state.controls_live() {
                sv.info.state = data::SessionState::Exited;
                sv.info.ended_at = Some(data::MOCK_NOW_MS);
                sv.activity = None;
            }
        }
    });

    // Close pane drops the session and its tab, then focuses whatever the
    // active tab resolves to — the same path tab close takes.
    let close = theme::btn_sm("Close pane", None, false, ink).on_press(|s: &mut State| {
        let Some(id) = s.selected_session.clone() else {
            return;
        };
        let title = s
            .data
            .sessions
            .iter()
            .find(|v| v.info.id == id)
            .map(|v| v.title());
        s.data.sessions.retain(|v| v.info.id != id);
        s.selected_session = None;
        s.active_workspace = None;
        if let Some(title) = title {
            if let Some(pos) = s.open_tabs.iter().position(|t| *t == title) {
                s.open_tabs.remove(pos);
                if pos < s.active_tab {
                    s.active_tab -= 1;
                }
                if s.active_tab >= s.open_tabs.len() {
                    s.active_tab = s.open_tabs.len().saturating_sub(1);
                }
            }
        }
        sync_selection(s);
    });

    row()
        .w_full()
        .items_center()
        .gap(12.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], ink.border)
        .fill(ink.panel)
        .px(16.0)
        .py(8.0)
        .child(crumb)
        .child(badge)
        .child(elapsed)
        .child(spacer())
        .child(row().gap(8.0).items_center().child(stop).child(close))
}
