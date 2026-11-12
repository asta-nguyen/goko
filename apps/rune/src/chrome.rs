//! The app chrome around the sidebar: the 48 px header, the workspace tab bar,
//! and the controls strip above the terminal. Ported from fidelity.rs with the
//! real components' measurements (App.tsx header, TerminalWorkspace tab strip,
//! SessionControls breadcrumb + state badge + elapsed + actions).

use std::f32::consts::FRAC_PI_3;

use rune::{El, Spring, TextStyle, Transition, Tx, el, row, spacer};

use crate::data;
use crate::editor::backend;
use crate::editor::navigation::{self, PendingNav};
use crate::panes::{self, Tab};
use crate::sidebar;
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
        .help("Settings (⌘,)")
        .focusable(true)
        .focus_ring(2.0, 1.0, ink.accent)
        .child(
            theme::ic(&theme::SETTINGS, 16.0, glyph)
                .group_color(if open { glyph } else { ink.text })
                .icon_rotation(if open { GEAR_NOTCH } else { 0.0 })
                .motion(GEAR_SPRING),
        )
        // Settings toggles the app-level settings mode like App.tsx's gear;
        // pressing it again returns to the terminal. Routed through the
        // editor's draft guard so an unsaved file can't be hidden silently.
        .on_press_tx(|s: &mut State, tx: &mut Tx| {
            let target = if s.view == View::Settings {
                View::Terminal
            } else {
                View::Settings
            };
            navigation::navigate(s, PendingNav::SetView(target), tx);
            crate::yield_focus(s, tx);
        })
}

/// App.tsx header: `flex h-12 items-center gap-3 border-b border-border
/// bg-panel px-4` (48 px, gap 12, px 16).
/// One provider's quota chip: the name, the active account's name when the
/// agent has more than one, and each window's `label value`. Mirrors
/// UsageChips.tsx — `usageValue` turns an elapsed window into a muted
/// "reset"; the chip exists only while it has limits or a second account.
fn usage_chip_for(
    s: &State,
    agent: &'static str,
    name: &'static str,
    updated_at: Option<i64>,
    limits: &[theme::ChipLimit],
    ink: &Ink,
) -> Option<El> {
    let view = s.data.accounts.as_ref().map(|v| match agent {
        "claude" => &v.claude,
        _ => &v.codex,
    });
    let multi = view.is_some_and(|v| v.items.len() >= 2);
    if limits.is_empty() && !multi {
        return None;
    }
    let active_name = if multi {
        view.and_then(|v| {
            v.items
                .iter()
                .find(|item| item.id == v.active)
                .map(|item| item.name.clone())
        })
    } else {
        None
    };
    // `<name> · <account>`: the account sits muted inside the name span.
    let mut chip = theme::usage_chip(name, limits, ink);
    if let Some(account) = &active_name {
        chip = chip.child(theme::txt(
            &format!("· {account}"),
            theme::T11M_LH,
            ink.muted,
        ));
    }
    let resets: Vec<String> = s
        .data
        .usage
        .as_ref()
        .map(|_| {
            limits
                .iter()
                .map(|limit| format!("{} resets {}", limit.label, limit.reset_at))
                .collect()
        })
        .unwrap_or_default();
    let help = {
        let mut lines =
            vec!["Each percentage is the share of that window already used".to_string()];
        lines.extend(resets);
        lines.push(format!("Updated {}", data::usage_age(updated_at, s.now_ms)));
        lines.join("\n")
    };
    let agent = agent.to_string();
    let mut chip = chip.help(help);
    if multi {
        // Its account menu drops from the chip itself.
        chip = chip
            .key(format!("usage-chip:{agent}"))
            .role("button")
            .focusable(true)
            .expanded(s.usage_menu.as_deref() == Some(agent.as_str()))
            .on_press_tx(move |s: &mut State, tx: &mut Tx| {
                s.usage_menu = Some(agent.clone());
                crate::yield_focus(s, tx);
            });
    }
    Some(chip)
}

/// UsageChips.tsx: both providers' chips, hidden while there is nothing to
/// say (no limits and no second account).
fn usage_chips(s: &State, ink: &Ink) -> Vec<El> {
    let Some(usage) = &s.data.usage else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let claude = [
        usage.claude.five_hour.as_ref(),
        usage.claude.seven_day.as_ref(),
    ];
    let claude_limits = chip_limits(&claude, "claude", s.now_ms);
    if let Some(chip) = usage_chip_for(
        s,
        "claude",
        "Claude",
        usage.claude.updated_at,
        &claude_limits,
        ink,
    ) {
        out.push(chip);
    }
    let codex = [usage.codex.primary.as_ref(), usage.codex.secondary.as_ref()];
    let codex_limits = chip_limits(&codex, "codex", s.now_ms);
    if let Some(chip) = usage_chip_for(
        s,
        "codex",
        "Codex",
        usage.codex.updated_at,
        &codex_limits,
        ink,
    ) {
        out.push(chip);
    }
    out
}

/// One window's value as the chips show it: a window that has already
/// reset reads "reset", never its last percentage.
pub fn limit_value(limit: &app_core::usage::Limit, now_ms: i64) -> String {
    if limit.resets_at * 1000 <= now_ms {
        "reset".to_string()
    } else {
        format!("{}%", limit.used_pct.round() as i64)
    }
}

/// `limitsFor` + `usageValue`: one rendered window per present limit.
fn chip_limits(
    limits: &[Option<&app_core::usage::Limit>],
    provider: &str,
    now_ms: i64,
) -> Vec<theme::ChipLimit> {
    limits
        .iter()
        .flatten()
        .map(|limit| {
            let reset = limit.resets_at * 1000 <= now_ms;
            theme::ChipLimit {
                label: data::window_label(limit.window_minutes, provider),
                value: limit_value(limit, now_ms),
                tone: if reset {
                    theme::UsageTone::Muted
                } else {
                    theme::usage_tone(limit.used_pct)
                },
                reset_at: crate::data::relative_time(limit.resets_at * 1000 - now_ms),
            }
        })
        .collect()
}

/// The account menu's quota line: the same expiry-aware values the chips
/// show — a window that already reset reads "reset", never its last
/// percentage.
pub fn account_usage_summary(
    usage: &app_core::usage::AccountUsage,
    provider: &str,
    now_ms: i64,
) -> String {
    usage
        .limits
        .iter()
        .map(|limit| {
            format!(
                "{} {}",
                data::window_label(limit.window_minutes, provider),
                limit_value(limit, now_ms)
            )
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The multi-account chip's dropdown: pick the active account or jump to
/// the Accounts settings section, like the desktop's AccountMenu.
pub fn usage_menu_layer(s: &State, layer: &mut Vec<El>) -> Option<El> {
    let agent = s.usage_menu.clone()?;
    let view = s.data.accounts.as_ref()?;
    let agent_view = match agent.as_str() {
        "claude" => &view.claude,
        _ => &view.codex,
    };
    let mut menu = rune_kit::Menu::new("usage-menu")
        .anchor(format!("usage-chip:{agent}"))
        .placement(rune_kit::Placement::BottomEnd)
        .open(true);
    for item in &agent_view.items {
        let account_id = item.id.clone();
        let account_name = item.name.clone();
        let agent = agent.clone();
        // AccountUsageSummary: the account's own limits, joined.
        let summary = s
            .data
            .usage
            .as_ref()
            .and_then(|u| u.accounts.get(&format!("{}:{}", agent, account_id)))
            .map(|usage| {
                // The same expiry-aware value the chips show: a window
                // that already reset must not keep its last percentage
                // here.
                account_usage_summary(usage, &agent, s.now_ms)
            })
            .unwrap_or_else(|| "No usage data".to_string());
        let is_active = account_id == agent_view.active;
        menu = menu.item(
            rune_kit::MenuItem::new(account_name)
                .checked(is_active)
                .description(summary)
                .on_select_tx(move |st: &mut State, _tx: &mut Tx| {
                    st.usage_menu = None;
                    if !is_active {
                        let _ = backend::enqueue(
                            &mut st.editor.backend,
                            backend::Job::project(backend::Req::AccountSetActive {
                                agent: agent.clone(),
                                id: account_id.clone(),
                            }),
                        );
                    }
                }),
        );
    }
    menu = menu.item(rune_kit::MenuItem::new("Manage accounts…").on_select_tx(
        |st: &mut State, tx: &mut Tx| {
            st.usage_menu = None;
            st.settings.section = crate::settings::Section::Accounts;
            navigation::navigate(st, PendingNav::SetView(View::Settings), tx);
        },
    ));
    Some(
        menu.on_close(|st: &mut State| st.usage_menu = None)
            .build(&s.editor.kit, layer),
    )
}

/// `path` with a leading `$HOME` shown as `~`, as a shell prompt shows it.
/// Only whole components match, so `$HOME=/home/al` leaves `/home/alice` alone.
/// The cut for width is rune's `truncate_middle`, at paint time.
pub fn home_relative(path: &str) -> String {
    let Some(home) = std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty() && h != "/")
    else {
        return path.to_string();
    };
    match std::path::Path::new(path).strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.to_string(),
    }
}

/// The path the header should display:
/// - In Editor view: the active checkout path.
/// - In Terminal view: the active session's live working directory (`cwd`).
/// - Otherwise: falls back to the editor checkout, selected project root, or None.
pub fn active_display_path(s: &State) -> Option<String> {
    if s.view == View::Editor
        && let Some(co) = &s.editor.checkout
    {
        return Some(co.display().to_string());
    }
    if let Some(tab) = s.open_tabs.get(s.active_tab)
        && let Some(pane) = panes::pane_by_id(&tab.layout, &tab.active_pane)
        && let Some(session) = s.session_by_key(&pane.session)
        && !session.info.cwd.is_empty()
    {
        return Some(session.info.cwd.clone());
    }
    if let Some(co) = &s.editor.checkout {
        return Some(co.display().to_string());
    }
    if let Some(pid) = &s.editor.project_id
        && let Some(p) = s.editor.projects.iter().find(|p| &p.project.id == pid)
    {
        return Some(p.project.root.clone());
    }
    None
}

/// App.tsx header: `flex h-12 items-center gap-3 border-b border-border
/// bg-panel px-4` (48 px, gap 12, px 16).
pub fn header(s: &State, ink: &Ink, inset: f32) -> El {
    let mut right = row().gap(8.0).items_center();
    for chip in usage_chips(s, ink) {
        right = right.child(chip);
    }
    // ConnectionBadge only renders when the daemon is not connected; the mock
    // is connected, so it is absent — kept for the real-data swap. It arrives
    // with a short fade and a small scale-up (never from nothing) and leaves
    // faster than it came, on Rune's default enter/exit springs.
    if !s.data.connected {
        // Its help names the last disconnect reason, so the pill says why.
        let help = s
            .daemon_reason
            .clone()
            .filter(|reason| !reason.is_empty())
            .unwrap_or_else(|| "Waiting for the Goko daemon".into());
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
                .help(help)
                .child(el().size(6.0, 6.0).radius(3.0).fill(ink.danger))
                .child(theme::txt("Reconnecting…", T11M_LH, ink.danger)),
        );
    }
    right = right.child(settings_button(s.view == View::Settings, ink));

    // In the band's flow between two spacers rather than pinned over it: a
    // pinned pill ignored the right-hand controls and painted over them in a
    // narrow window. Here it shrinks and its path elides in the middle.
    let path_el = active_display_path(s).map(|path| {
        let shown = home_relative(&path);
        let path_copy = path.clone();
        row()
            .key("header-path")
            .min_w(0.0)
            .max_w(520.0)
            .shrink()
            .h(26.0)
            .px(10.0)
            .gap(6.0)
            .items_center()
            .radius(6.0)
            .fill(ink.panel)
            .border(1.0, ink.border)
            .hover(ink.text.alpha(0.06))
            .press(ink.text.alpha(0.12))
            .role("button")
            .label(format!("Path: {path}"))
            .help(format!("{path} (click to copy)"))
            .on_press_tx(move |st: &mut State, tx: &mut Tx| {
                tx.copy(path_copy.clone());
                st.notice = Some("Copied to clipboard".into());
            })
            .child(theme::ic(&theme::FOLDER, 14.0, ink.muted))
            .child(
                theme::txt(&shown, T12M, ink.muted)
                    .min_w(0.0)
                    .truncate_middle(),
            )
    });

    let path_el = path_el.map(|path| {
        row()
            .min_w(0.0)
            .shrink()
            .gap(8.0)
            .items_center()
            .child(path)
            .children(crate::terminal_branch::view(s, ink))
    });

    let band = row()
        .key("header")
        .w_full()
        .h(48.0)
        .fill(ink.panel)
        .border_sides([0.0, 0.0, 1.0, 0.0], ink.border)
        .pr(16.0)
        // macOS draws the traffic lights over the full-size content; start the
        // wordmark past them (`inset` is 0 in full screen and off macOS).
        .pl(if inset > 0.0 { inset + 12.0 } else { 16.0 })
        .gap(12.0)
        .items_center()
        .window_drag();

    band.child(
        row()
            .key("wordmark")
            .items_center()
            .child(theme::txt("goko", WORDMARK, ink.text)),
    )
    .child(spacer())
    // wordmark · spacer · path · spacer · controls: equal spacers center the
    // path in the free space and give it up first when the window narrows.
    .children(path_el.into_iter().flat_map(|path_el| [path_el, spacer()]))
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
    for (i, tab) in s.visible_tabs() {
        tabs = tabs.child(tab_el(tab, i, i == s.active_tab, ink));
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
            theme::btn_sm("New tab", Some((&theme::PLUS, 14.0)), false, ink)
                .help("New tab (⌘T)")
                .on_press(|s: &mut State| s.new_shell_tab()),
        )
        .child(new_tab_menu(ink))
        .child(split_button(
            &theme::COLUMNS_2,
            "Split side by side",
            panes::Direction::SideBySide,
            s.can_split(),
            ink,
        ))
        .child(split_button(
            &theme::ROWS_2,
            "Split stacked",
            panes::Direction::Stacked,
            s.can_split(),
            ink,
        ))
        .child(view_switch(s, ink))
}

/// The chevron beside New tab: the sidebar's New session menu for the active
/// checkout, so every agent and Custom command… start from here too. New tab
/// itself still starts a shell.
fn new_tab_menu(ink: &Ink) -> El {
    el().key("new-tab-menu")
        .size(28.0, 28.0)
        .radius(8.0)
        .center()
        .hover(ink.text.alpha(0.08))
        .press(ink.text.alpha(0.12))
        .press_scale(0.95)
        .motion(STATE_SPRING)
        .role("button")
        .label("New tab menu")
        .help("Start an agent or a custom command")
        .focusable(true)
        .focus_ring(2.0, 1.0, ink.accent)
        .child(theme::ic(&theme::CHEVRON_DOWN, 14.0, ink.text))
        // The same new-session picker as the sidebar's `+`, dropping from
        // this caret: it starts where the app is looking, as New tab does.
        .on_press_tx(|s: &mut State, tx: &mut Tx| {
            sidebar::open_new_session(s, "new-tab-menu".to_string(), None, 300.0);
            crate::yield_focus(s, tx);
        })
}

/// A split-pane button: another shell session lands beside the focused
/// pane in the same checkout, as a second pane. Disabled until a pane is
/// focused and the daemon is connected — a hover on a button that does
/// nothing promises something it won't deliver.
fn split_button(
    icon: &'static rune::Icon,
    label: &str,
    direction: panes::Direction,
    enabled: bool,
    ink: &Ink,
) -> El {
    let mut b = el()
        .key(format!("split:{label}"))
        .size(28.0, 28.0)
        .radius(8.0)
        .center()
        .role("button")
        .label(label)
        .child(theme::ic(
            icon,
            16.0,
            if enabled { ink.text } else { ink.muted },
        ));
    if enabled {
        b = b
            .hover(ink.text.alpha(0.08))
            .press(ink.text.alpha(0.12))
            .press_scale(0.95)
            .motion(STATE_SPRING)
            .help(label.to_string())
            .focusable(true)
            .focus_ring(2.0, 1.0, ink.accent)
            .on_press_tx(move |s: &mut State, tx: &mut Tx| {
                s.split_active(direction);
                crate::yield_focus(s, tx);
            });
    } else {
        b = b.opacity(0.4).disabled(true).help(format!(
            "{label} — connect the daemon and focus a terminal pane"
        ));
    }
    b
}

/// Session tab pill: `bg-bg/50 rounded-lg`; the active tab gets `bg-bg`. Close X on
/// every tab. Clicking a tab selects its active pane's session; the X begins the
/// tab's close — every pane's session is stopped and deleted, and the tab drops
/// as its last pane leaves.
///
/// Hover is a text-colour wash plus a lift of the title to full text colour, never
/// `ink.bg`: that is exactly the active fill, so a `bg` hover made a hovered inactive
/// tab indistinguishable from the active one. Tabs carry their own id — a split tab
/// has several sessions, so "the tab's session" does not exist.
fn tab_el(tab: &Tab, index: usize, active: bool, ink: &Ink) -> El {
    let title = tab.title.clone();
    let tab_id = tab.id.clone();
    // The close X sits in a 20 pt box with 6 pt right padding: the 12 pt glyph then
    // has 10 pt to the pill's edge, matching the title's 10 pt on the left. It owns
    // its hover (`hover_alone`), so the pill doesn't wash under the pointer too.
    let mut close = el()
        .key(format!("close:{}", tab.id))
        .size(20.0, 20.0)
        .center()
        .radius(6.0)
        .role("button")
        .label(format!("Close {title}"))
        .child(theme::ic(&theme::X, 12.0, ink.muted).group_color(ink.text));
    if tab.closing {
        close = close
            .opacity(0.4)
            .disabled(true)
            .help("This tab is closing");
    } else {
        close = close
            .hover_group()
            .hover_alone()
            .hover(ink.text.alpha(0.12))
            .press(ink.text.alpha(0.18))
            .press_scale(0.9)
            .help("Close tab".to_string())
            .focusable(true)
            .tab_index(-1)
            .on_press_tx(move |s: &mut State, tx: &mut Tx| {
                // Every pane in the tab enters the close lifecycle; the
                // tab itself drops when its last pane leaves.
                s.begin_close_tab(index);
                crate::yield_focus(s, tx);
            });
    }

    let mut pill = row()
        .key(format!("tab:{}", tab.id))
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
        .label(title.clone())
        .focusable(true)
        .focus_ring(2.0, 1.0, ink.accent)
        .child(
            theme::txt(&title, TAB, if active { ink.text } else { ink.muted })
                .group_color(ink.text)
                .max_w(160.0)
                .truncate_middle()
                .help_when_truncated(title.clone()),
        );
    if tab.closing {
        pill = pill
            .opacity(0.6)
            .child(theme::txt("closing", theme::T11, ink.muted));
    }
    pill.child(close)
        .on_secondary_press(move |s: &mut State, p: rune::PointerPress, _tx: &mut Tx| {
            s.tab_menu = Some(crate::tab_debug::MenuTarget {
                tab: tab_id.clone(),
                x: p.x,
                y: p.y,
            });
            s.subagents_at = None;
        })
        .on_press_tx(move |s: &mut State, tx: &mut Tx| {
            s.activate_tab(index);
            // And shows it: from the Editor the click leaves for the Terminal.
            // The tab is chosen first so a draft guard's resume lands on it.
            if s.view != View::Terminal {
                navigation::navigate(s, PendingNav::SetView(View::Terminal), tx);
            }
            crate::yield_focus(s, tx);
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
    // View switches pass through the editor's draft guard: leaving the
    // Editor view with unsaved work raises Save/Discard/Cancel first.
    .on_press_tx(move |s: &mut State, tx: &mut Tx| {
        navigation::navigate(s, PendingNav::SetView(target), tx);
        crate::yield_focus(s, tx);
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
        .and_then(|pid| s.editor.projects.iter().find(|p| p.project.id == pid))
        .map(|p| p.project.name.as_str());
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
    crumb = crumb.child(
        theme::txt(&title, T14M, ink.text)
            .truncate_middle()
            .help_when_truncated(title.clone()),
    );

    // StateBadge + StateDot's tone dot: `rounded border border-border px-1.5
    // py-0.5 text-xs` + TONE, with the tone dot ahead of the label. A
    // working session draws a spinning accent ring instead of the dot.
    let badge = row()
        .radius(4.0)
        .border(1.0, ink.border)
        .px(6.0)
        .py(2.0)
        .gap(4.0)
        .items_center()
        .child(theme::state_dot(
            info.state,
            s.session_shown_activity(&sv.info.id),
            ink,
            &s.editor.kit,
        ))
        .child(theme::txt(
            info.state.label(),
            T12,
            info.state.badge_tone(ink),
        ));

    // Elapsed: ticks while live, frozen at ended_at otherwise.
    let live = info.state.controls_live();
    let end = if live {
        s.now_ms
    } else {
        info.ended_at.unwrap_or(s.now_ms)
    };
    let elapsed = theme::txt(
        &data::format_elapsed(end - info.started_at),
        T12TN,
        ink.muted,
    );

    // Stop is disabled when the state is not controllable or disconnected.
    // The daemon decides when the session is really gone: the row follows
    // its `session.updated` event. An interactive shell ignores SIGTERM,
    // so a stop that hangs offers the force kill after five seconds — the
    // same escalation the desktop shows.
    let force = s.force_kill_ready(&sv.info.id);
    let mut stop = if force {
        theme::btn_sm("Force kill", None, true, ink)
    } else {
        theme::btn_sm("Stop", None, true, ink)
    };
    if !live || !s.data.connected {
        stop = stop.opacity(0.5);
    }
    let stop = stop.on_press_tx(move |s: &mut State, tx: &mut Tx| {
        let Some(id) = s.selected_session.clone() else {
            return;
        };
        if s.session_by_key(&id)
            .is_some_and(|view| view.info.state.controls_live())
        {
            s.stop_session(&id, force);
        }
        crate::yield_focus(s, tx);
    });

    // Close pane closes the focused pane: its session is stopped (force),
    // deleted once it exits, and the split collapses around it — the
    // desktop's pane close. The daemon drives the lifecycle; a failure
    // stays on the pane until Retry.
    let close =
        theme::btn_sm("Close pane", None, false, ink).on_press_tx(|s: &mut State, tx: &mut Tx| {
            s.close_focused_pane();
            crate::yield_focus(s, tx);
        });

    row()
        .key("controls-bar")
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

#[cfg(test)]
mod tests {
    use super::*;
    use app_core::usage::{AccountUsage, Limit};
    use rune::{Cx, Env, Runtime};

    fn limit(used_pct: f64, resets_at: i64) -> Limit {
        Limit {
            used_pct,
            resets_at,
            window_minutes: 300,
        }
    }

    /// A window that already reset reads "reset" — never its last
    /// percentage, even at 100%.
    #[test]
    fn an_expired_window_reads_reset_not_its_percentage() {
        let now_ms = 1_700_000_000_000;
        let expired = limit(100.0, (now_ms / 1000) - 10);
        assert_eq!(limit_value(&expired, now_ms), "reset");
        let usage = AccountUsage {
            limits: vec![expired],
            updated_at: Some(now_ms),
        };
        assert_eq!(
            account_usage_summary(&usage, "claude", now_ms),
            "5h reset",
            "the account menu must not show a spent window's percentage"
        );
    }

    /// A chip with a limit says what its percentage measures.
    #[test]
    fn a_usage_chip_says_its_percentage_is_the_used_share() {
        let now_ms = 1_700_000_000_000;
        let mut s = State::new(crate::test_handle());
        s.now_ms = now_ms;
        s.data.usage = Some(app_core::usage::UsageSnapshot {
            claude: app_core::usage::ClaudeUsage {
                five_hour: Some(limit(42.0, now_ms / 1000 + 3600)),
                updated_at: Some(now_ms),
                ..Default::default()
            },
            ..Default::default()
        });
        let els = header_elements(s);
        let chip = els
            .iter()
            .find(|e| e.help.as_deref().is_some_and(|h| h.contains("Updated")))
            .expect("the Claude chip carries its help");
        let help = chip.help.as_deref().unwrap();
        assert!(
            help.contains("share of that window already used"),
            "chip help {help:?} does not say what the percentage measures",
        );
    }

    fn header_only(s: &State, _cx: &mut Cx<State>) -> El {
        header(s, &Ink::new(), 0.0)
    }

    /// The header's elements for `s`, laid out once.
    fn header_elements(s: State) -> Vec<rune::ElementInfo> {
        let mut rt = Runtime::new(s, header_only, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.elements()
    }

    fn pill(els: &[rune::ElementInfo]) -> Option<&rune::ElementInfo> {
        els.iter()
            .find(|e| e.key.as_deref() == Some("disconnected"))
    }

    #[test]
    fn connected_header_shows_no_pill() {
        let mut s = State::new(crate::test_handle());
        s.data.connected = true;
        let els = header_elements(s);
        assert!(pill(&els).is_none(), "no pill while connected");
    }

    #[test]
    fn disconnected_pill_gives_the_last_reason_as_help() {
        let mut s = State::new(crate::test_handle());
        s.data.connected = false;
        s.daemon_reason = Some("connection refused".into());
        let els = header_elements(s);
        let pill = pill(&els).expect("pill shown while disconnected");
        assert_eq!(pill.help.as_deref(), Some("connection refused"));
        assert!(
            els.iter()
                .any(|e| e.text.as_deref() == Some("Reconnecting…")),
            "the pill reads Reconnecting…"
        );
    }

    #[test]
    fn disconnected_pill_without_a_reason_uses_the_fallback_help() {
        let mut s = State::new(crate::test_handle());
        s.data.connected = false;
        s.daemon_reason = None;
        let els = header_elements(s);
        let pill = pill(&els).expect("pill shown while disconnected");
        assert_eq!(pill.help.as_deref(), Some("Waiting for the Goko daemon"));
    }

    /// A window that is still running shows its percentage.
    #[test]
    fn a_live_window_shows_its_percentage() {
        let now_ms = 1_700_000_000_000;
        let live = limit(42.4, (now_ms / 1000) + 600);
        assert_eq!(limit_value(&live, now_ms), "42%");
        let usage = AccountUsage {
            limits: vec![live],
            updated_at: Some(now_ms),
        };
        assert_eq!(account_usage_summary(&usage, "claude", now_ms), "5h 42%");
    }

    #[test]
    fn home_relative_shortens_only_a_whole_home_prefix() {
        let Ok(home) = std::env::var("HOME") else {
            return;
        };
        if home.is_empty() || home == "/" {
            return;
        }
        assert_eq!(home_relative(&home), "~");
        assert_eq!(home_relative(&format!("{home}/src/goko")), "~/src/goko");
        // A sibling that merely shares the prefix's characters is untouched.
        assert_eq!(home_relative(&format!("{home}x/a")), format!("{home}x/a"));
        assert_eq!(home_relative("/tmp/repo"), "/tmp/repo");
    }

    #[test]
    fn header_mounts_active_path_with_middle_ellipsis() {
        let mut s = State::new(crate::test_handle());
        s.editor.checkout = Some(std::path::PathBuf::from("/work/my-project/sub-repo"));
        s.view = View::Editor;
        let els = header_elements(s);
        let path_el = els.iter().find(|e| e.key.as_deref() == Some("header-path"));
        assert!(path_el.is_some(), "header mounts path element");
    }
}
