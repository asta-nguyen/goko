//! The app chrome around the sidebar: the 48 px header, the workspace tab bar,
//! and the controls strip above the terminal. Ported from fidelity.rs with the
//! real components' measurements (App.tsx header, TerminalWorkspace tab strip,
//! SessionControls breadcrumb + state badge + elapsed + actions).

use rune::{el, rich, row, spacer, Color, El, Span, TextStyle};

use crate::data;
use crate::theme::{self, Ink, T11M, T12, T12M, T12TN, T14, T14M, T14S};
use crate::State;

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
    right = right.child(el().size(28.0, 28.0).radius(8.0).center().child(theme::ic(
        &theme::SETTINGS,
        16.0,
        ink.text,
    )));

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
                        .gap(6.0)
                        .items_center()
                        .child(theme::txt("goko", T14S, ink.text))
                        // Prototype marker so this window is unmistakable next
                        // to the real app in side-by-side comparisons.
                        .child(
                            el().h(16.0)
                                .px(5.0)
                                .radius(4.0)
                                .fill(ink.danger)
                                .center()
                                .child(theme::txt("RUNE", TextStyle::new(9.0, 700.0), ink.white)),
                        ),
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
        .child(theme::btn_sm(
            "New tab",
            Some((&theme::PLUS, 14.0)),
            false,
            ink,
        ))
        .child(theme::btn_sm("Split left/right", None, true, ink))
        .child(theme::btn_sm("Split top/bottom", None, true, ink))
        .child(view_switch(ink))
}

/// Session tab pill: `bg-bg/50 rounded-lg`; the active title gets `bg-bg
/// font-medium`. Close X on every tab. Clicking a tab selects the session
/// with that title; the X closes the tab.
fn tab_el(title: &str, index: usize, active: bool, ink: &Ink) -> El {
    let mut b = row()
        .h(28.0)
        .gap(6.0)
        .items_center()
        .px(10.0)
        .radius(8.0)
        .fill(ink.bg.alpha(0.5));
    b = if active {
        b.child(theme::txt(title, T14M, ink.text))
    } else {
        b.child(theme::txt(title, T14, ink.muted))
    };
    let tab_title = title.to_string();
    b.child(
        el().size(16.0, 16.0)
            .center()
            .radius(4.0)
            .hover(ink.bg)
            .child(theme::ic(&theme::X, 12.0, ink.muted))
            .on_press(move |s: &mut State| {
                s.open_tabs.remove(index);
                if index < s.active_tab {
                    s.active_tab -= 1;
                }
                if s.active_tab >= s.open_tabs.len() {
                    s.active_tab = s.open_tabs.len().saturating_sub(1);
                }
                if let Some(t) = s.open_tabs.get(s.active_tab) {
                    let t = t.clone();
                    if let Some(sv) = s.data.sessions.iter().find(|v| v.title() == t) {
                        s.selected_session = Some(sv.info.id.clone());
                        s.active_workspace = Some(data::workspace_for_session(&s.data, sv));
                    }
                } else {
                    s.selected_session = None;
                    s.active_workspace = None;
                }
            }),
    )
    .on_press(move |s: &mut State| {
        s.active_tab = index;
        if let Some(sv) = s.data.sessions.iter().find(|v| v.title() == tab_title) {
            s.selected_session = Some(sv.info.id.clone());
            s.active_workspace = Some(data::workspace_for_session(&s.data, sv));
        }
    })
}

/// ViewSwitch: Terminal pressed / Editor unpressed — a two-item segmented
/// control, `bg-bg/60 rounded-lg p-0.5` with `bg-bg` on the active item.
fn view_switch(ink: &Ink) -> El {
    row()
        .h(28.0)
        .gap(2.0)
        .items_center()
        .padding(2.0)
        .radius(8.0)
        .fill(ink.bg.alpha(0.6))
        .child(switch_item("Terminal", true, ink))
        .child(switch_item("Editor", false, ink))
}

fn switch_item(label: &str, active: bool, ink: &Ink) -> El {
    let mut b = row().h(24.0).px(8.0).items_center().radius(6.0);
    if active {
        b = b.fill(ink.bg);
    }
    b.child(theme::txt(
        label,
        T12M,
        if active { ink.text } else { ink.muted },
    ))
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

    // Breadcrumb rich text: "<project> › " muted + "<agent> · " muted +
    // "<title>" 14 medium — `min-w-0 truncate text-sm`.
    let project_name = sv
        .meta
        .as_ref()
        .and_then(|m| m.project_id.as_deref())
        .and_then(|pid| s.data.projects.iter().find(|p| p.id == pid))
        .map(|p| p.name.as_str());
    let agent = sv
        .agent_label(&s.data.agents)
        .filter(|a| !a.eq_ignore_ascii_case(title.as_str()));

    let mut content = String::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut push = |text: &str, color: Color, weight: Option<f32>| {
        let start = content.len();
        content.push_str(text);
        let mut sp = Span::new(start, content.len()).color(color);
        if let Some(w) = weight {
            sp = sp.weight(w);
        }
        spans.push(sp);
    };
    if let Some(pn) = project_name {
        push(&format!("{pn} › "), ink.muted, None);
    }
    if let Some(a) = agent {
        push(&format!("{a} · "), ink.muted, None);
    }
    push(&title, ink.text, Some(500.0));

    // StateBadge: `rounded border border-border px-1.5 py-0.5 text-xs` + TONE.
    let badge = row()
        .radius(4.0)
        .border(1.0, ink.border)
        .px(6.0)
        .py(2.0)
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
    let mut stop = theme::btn_sm("Stop", None, true, ink);
    if !live || !s.data.connected {
        stop = stop.opacity(0.5);
    }

    row()
        .w_full()
        .items_center()
        .gap(12.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], ink.border)
        .fill(ink.panel)
        .px(16.0)
        .py(8.0)
        .child(
            rich(content, spans)
                .font(T14)
                .color(ink.text)
                .min_w(0.0)
                .truncate(),
        )
        .child(badge)
        .child(elapsed)
        .child(spacer())
        .child(
            row()
                .gap(8.0)
                .items_center()
                .child(stop)
                .child(theme::btn_sm("Close pane", None, false, ink)),
        )
}
