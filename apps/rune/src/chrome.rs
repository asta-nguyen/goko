//! The app chrome around the sidebar: the 48 px header, the workspace tab bar,
//! and the controls strip above the terminal. Ported from fidelity.rs with the
//! real components' measurements (App.tsx header, TerminalWorkspace tab strip,
//! SessionControls breadcrumb + state badge + elapsed + actions).

use std::f32::consts::FRAC_PI_3;

use rune::{el, row, spacer, El, Spring, TextStyle, Transition};

use crate::data;
use crate::theme::{self, Ink, STATE_SPRING, T11M_LH, T12, T12M, T12TN, T14, T14M, T14S};
use crate::{State, View};

/// "goko" on a 20 pt line box, so the wordmark sits on whole device pixels in the
/// 48 pt header (the sidebar's compact scale does the same with 16 pt boxes).
const WORDMARK: TextStyle = T14S.lh(20.0);

/// The gear turns one notch when settings opens and back when it closes. The glyph
/// has six lobes, so 60° lands on a shape identical to the one it left: it reads as a
/// ratchet click, not a spin, and the resting state is never a rotated (resampled)
/// glyph. Settings opens occasionally, so a visible turn is affordable; the spring has
/// no bounce and settles in about the time the view swap takes. Reduce Motion lands it
/// at once.
const GEAR_NOTCH: f32 = FRAC_PI_3;
const GEAR_SPRING: Spring = Spring::new(0.3, 0.0);

/// The settings toggle's icon-button states, on a 28 pt square: rest is muted (a quiet icon in a header full of text), hover lifts the glyph to
/// the text colour and washes the square, press shrinks it 5 % and deepens the wash,
/// and "settings is open" is an accent tint that hover leaves alone, so the two
/// states stay readable together. The fill is always mounted and only its alpha
/// changes, so opening settings is a colour transition on the quick state spring.
fn settings_button(open: bool, ink: &Ink) -> El {
    let glyph = if open {
        ink.accent.mix(ink.white, 0.4)
    } else {
        ink.muted
    };
    el().key("settings")
        .size(28.0, 28.0)
        .radius(8.0)
        .center()
        .fill(ink.accent.alpha(if open { 0.16 } else { 0.0 }))
        .hover_group()
        .hover(ink.text.alpha(0.08))
        .press(ink.text.alpha(0.12))
        .press_scale(0.95)
        .motion(STATE_SPRING)
        // A toggle button named "Settings", pressed while settings is open; the
        // tooltip repeats the name, so Rune leaves it out of the description.
        .role("toggle")
        .label("Settings")
        .checked(open)
        .help("Settings")
        .focusable(true)
        .focus_ring(2.0, 1.0, ink.accent)
        .child(
            theme::ic(&theme::SETTINGS, 16.0, glyph)
                .group_color(if open { glyph } else { ink.text })
                .icon_rotation(if open { GEAR_NOTCH } else { 0.0 })
                .motion(GEAR_SPRING),
        )
        // Settings toggles the app-level settings mode like App.tsx's gear;
        // pressing it again returns to the terminal.
        .on_press(|s: &mut State| {
            s.view = if s.view == View::Settings {
                View::Terminal
            } else {
                View::Settings
            };
        })
}

/// Connection status dot: a 6 pt core in a 12 pt halo, so it reads as a live
/// indicator rather than a stray pixel. Both change colour together on the state
/// spring when the daemon drops or returns.
fn connection_dot(connected: bool, ink: &Ink) -> El {
    let tone = if connected { ink.success } else { ink.danger };
    el().size(12.0, 12.0)
        .radius(6.0)
        .center()
        .fill(tone.alpha(0.16))
        .motion(STATE_SPRING)
        .child(el().size(6.0, 6.0).radius(3.0).fill(tone))
}

/// App.tsx header: `flex h-12 items-center gap-3 border-b border-border
/// bg-panel px-4` (48 px, gap 12, px 16).
pub fn header(s: &State, ink: &Ink) -> El {
    let mut right = row().gap(8.0).items_center();
    for u in &s.data.usage {
        right = right.child(theme::usage_chip(u.name, u.limits, ink));
    }
    // ConnectionBadge only renders when the daemon is not connected; the mock
    // is connected, so it is absent — kept for the real-data swap. It arrives
    // with a short fade and a small scale-up (never from nothing) and leaves
    // faster than it came, on Rune's default enter/exit springs.
    if !s.data.connected {
        right = right.child(
            row()
                .key("disconnected")
                .h(20.0)
                .px(8.0)
                .gap(6.0)
                .items_center()
                .radius(10.0)
                .border(1.0, ink.danger.alpha(0.3))
                .fill(ink.danger.alpha(0.1))
                .transition(Transition::opacity().combined(Transition::scale(0.92)))
                .child(el().size(6.0, 6.0).radius(3.0).fill(ink.danger))
                .child(theme::txt("Disconnected", T11M_LH, ink.danger)),
        );
    }
    right = right.child(settings_button(s.view == View::Settings, ink));

    // Persistent connection dot beside the wordmark; the pill on the right
    // only appears while disconnected, matching the React header. Hovering the
    // wordmark names what the dot says, since a bare coloured dot explains nothing.
    let status = if s.data.connected {
        "Connected to the Goko daemon"
    } else {
        "Disconnected from the Goko daemon"
    };

    row()
        .key("header")
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
                        .border(1.0, ink.white.alpha(0.14))
                        .center()
                        .child(theme::ic(&theme::GOKO_MARK, 16.0, ink.white)),
                )
                .child(
                    // 12 pt halo around the 6 pt dot: gap 5 keeps the dot's
                    // centre where the bare dot's was (8 pt gap + 3 pt radius).
                    row()
                        .key("status")
                        .gap(5.0)
                        .items_center()
                        .label(status)
                        .help(status)
                        .child(theme::txt("goko", WORDMARK, ink.text))
                        .child(connection_dot(s.data.connected, ink)),
                ),
        )
        .child(spacer())
        .child(right)
}

// ---------------------------------------------------------------------------
// Tab bar — TerminalWorkspace's strip: `min-h-10 border-b bg-panel px-2 gap-2`
// ---------------------------------------------------------------------------

/// Tab and segment labels on a 16 pt line box, so text sits on whole device pixels
/// inside the 28–32 pt rows (as the sidebar's compact scale does). Every tab title
/// and both segment labels are medium weight: the active one is told apart by
/// colour, so selecting never changes a label's width and nudges its neighbours.
const TAB: TextStyle = T12M.lh(16.0);

pub fn tab_bar(s: &State, ink: &Ink) -> El {
    let mut tabs = row().grow(1.0).min_w(0.0).items_center().gap(4.0);
    for (i, title) in s.open_tabs.iter().enumerate() {
        tabs = tabs.child(tab_el(title, i, i == s.active_tab, ink));
    }
    // A border is drawn inside the box without shrinking the space children are
    // centred in, so a 40 pt bar with a bottom line centred its 32 pt tabs in 40 and
    // left them 4 pt below the top edge but 3 pt above the line. 41 pt with the line's
    // pt taken back as bottom padding centres them in the 40 pt that's actually visible.
    row()
        .key("tabbar")
        .w_full()
        .h(41.0)
        .pb(1.0)
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
        .child(split_button(&theme::COLUMNS_2, "Split side by side", ink))
        .child(split_button(&theme::ROWS_2, "Split stacked", ink))
        .child(view_switch(s, ink))
}

/// The split-pane buttons are inert until the layout milestone, so they say so: dim,
/// no hover or press (a hover on a button that does nothing promises something it
/// won't deliver), skipped by Tab, and a tooltip that names what's missing.
fn split_button(icon: &'static rune::Icon, label: &str, ink: &Ink) -> El {
    el().key(format!("split:{label}"))
        .size(28.0, 28.0)
        .radius(8.0)
        .center()
        .opacity(0.4)
        .role("button")
        .label(label)
        .disabled(true)
        .help(format!("{label} — coming with pane layouts"))
        .child(theme::ic(icon, 16.0, ink.text))
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

/// Session tab pill: `bg-bg/50 rounded-lg`; the active tab gets `bg-bg`. Close X on
/// every tab. Clicking a tab selects the session with that title; the X closes the
/// tab.
///
/// Hover is a text-colour wash plus a lift of the title to full text colour, never
/// `ink.bg`: that is exactly the active fill, so a `bg` hover made a hovered inactive
/// tab indistinguishable from the active one. Tabs are keyed by title so closing one
/// slides its neighbours into place instead of re-labelling them in place.
fn tab_el(title: &str, index: usize, active: bool, ink: &Ink) -> El {
    // The close X sits in a 20 pt box with 6 pt right padding: the 12 pt glyph then
    // has 10 pt to the pill's edge, matching the title's 10 pt on the left. It owns
    // its hover (`hover_alone`), so the pill doesn't wash under the pointer too.
    let close = el()
        .key(format!("close:{title}"))
        .size(20.0, 20.0)
        .center()
        .radius(6.0)
        .hover_group()
        .hover_alone()
        .hover(ink.text.alpha(0.12))
        .press(ink.text.alpha(0.18))
        .press_scale(0.9)
        .role("button")
        .label(format!("Close {title}"))
        .help("Close tab")
        .focusable(true)
        .tab_index(-1)
        .child(theme::ic(&theme::X, 12.0, ink.muted).group_color(ink.text))
        .on_press(move |s: &mut State| {
            s.open_tabs.remove(index);
            if index < s.active_tab {
                s.active_tab -= 1;
            }
            if s.active_tab >= s.open_tabs.len() {
                s.active_tab = s.open_tabs.len().saturating_sub(1);
            }
            sync_selection(s);
        });

    row()
        .key(format!("tab:{title}"))
        .h(32.0)
        .gap(6.0)
        .items_center()
        .pl(10.0)
        .pr(6.0)
        .radius(8.0)
        .fill(if active { ink.bg } else { ink.bg.alpha(0.5) })
        .hover_group()
        .hover(ink.text.alpha(0.06))
        .press(ink.text.alpha(0.10))
        .motion(STATE_SPRING)
        .role("tab")
        .label(title)
        .focusable(true)
        .focus_ring(2.0, 1.0, ink.accent)
        .child(
            theme::txt(title, TAB, if active { ink.text } else { ink.muted })
                .group_color(ink.text)
                .max_w(160.0)
                .truncate(),
        )
        .child(close)
        .on_press(move |s: &mut State| {
            s.active_tab = index;
            sync_selection(s);
        })
}

/// View-switch geometry. The track's border is drawn inside its box and the content
/// inset is the padding alone, so the gap you see between the outline and the thumb
/// is `SEG_INSET - 1`. The track's height is derived from the same inset (it used to
/// be a fixed 34 over a 2 pt padding: 3 pt above and below, 2 pt either side, so the
/// thumb sat visibly tighter to the left and right edges than to the top and bottom),
/// and the corner radii are concentric: the thumb's is the track's minus the inset.
/// 32 pt tall lines the track up with the 32 pt tab pills beside it.
const SEG_H: f32 = 26.0;
const SEG_INSET: f32 = 3.0;
const TRACK_H: f32 = SEG_H + 2.0 * SEG_INSET;
const TRACK_RADIUS: f32 = 8.0;
const SEG_RADIUS: f32 = TRACK_RADIUS - SEG_INSET;

/// ViewSwitch: Terminal pressed / Editor unpressed — a bordered `bg-bg` track, a
/// raised thumb behind the active item, icons included (ViewSwitch.tsx). Exposed as
/// a radio group: arrow keys move between segments and select as they go.
///
/// The thumb is a translucent text-colour wash, not the TSX's `bg-panel`, which on a
/// `bg` track was barely visible (#252526 on #1e1e1e). It belongs to whichever item is
/// active and shares the id `view-thumb`, so a switch glides the one thumb across
/// (position, width and all) instead of cutting from one item to the other.
fn view_switch(s: &State, ink: &Ink) -> El {
    row()
        .key("view-switch")
        .role("segmented")
        .label("Workspace view")
        .focus_group()
        .focus_follows()
        .h(TRACK_H)
        .gap(2.0)
        .items_center()
        .padding(SEG_INSET)
        .radius(TRACK_RADIUS)
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

/// One segment. Rest is muted, hover lifts an inactive one to the text colour (the
/// thumb is the only background, so a hover can't be mistaken for a selection), and
/// press shrinks it 4 %. Selecting is a colour transition on the quick state spring.
fn switch_item(
    icon: &'static rune::Icon,
    label: &str,
    target: View,
    active: bool,
    ink: &Ink,
) -> El {
    let tone = if active { ink.text } else { ink.muted };
    // An active segment is already at full strength; hover only lifts the others.
    let hover_tone = ink.text;
    let mut b = row()
        .key(format!("view:{label}"))
        .h(SEG_H)
        .px(8.0)
        .gap(6.0)
        .items_center()
        .radius(SEG_RADIUS)
        .hover_group()
        .press_scale(0.96)
        .motion(STATE_SPRING)
        .role("segment")
        .label(label)
        .checked(active)
        .focusable(true)
        .focus_selected(active)
        .focus_ring(2.0, 1.0, ink.accent);
    if active {
        // First child, so the icon and label paint over it. The active segment has one
        // more child than the other, so every child is keyed: Rune matches unkeyed
        // children by position, and the icon would otherwise morph into the label
        // (and the label into nothing) on every switch.
        b = b.child(
            el().key("thumb")
                .absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .radius(SEG_RADIUS)
                .fill(ink.text.alpha(0.10))
                .matched("view-thumb"),
        );
    }
    b.child(
        theme::ic(icon, 14.0, tone)
            .key("icon")
            .group_color(hover_tone),
    )
    .child(
        theme::txt(label, TAB, tone)
            .key("label")
            .group_color(hover_tone),
    )
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
