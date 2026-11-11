//! The native Settings page — a compact port of `main`'s
//! `features/settings/SettingsPage.tsx`: a section rail on the left
//! (Editor, Appearance, Accounts, ACP agents) and the active section on
//! the right. Vim is deliberately absent (decision D1); the Editor
//! section carries the native indent preference instead. All mutations —
//! settings rows, accounts, ACP overrides — ride the existing worker
//! lanes, never the UI thread.

use std::collections::HashMap;

use app_core::accounts::AgentView;
use app_core::acp::AcpAgent;
use rune::{El, ScrollSpec, Tx, col, el, row};

use crate::editor::backend::{self, Job, Req};
use crate::editor::indent::{self, UnitChoice};
use crate::theme::{self, Ink, T11M, T12, T12M, T14M};
use crate::{State, theme::Theme};

/// The section rail's entries — SettingsPage.tsx's `SECTIONS`, minus the
/// Vim control (deferred) and in the same order.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Section {
    #[default]
    Editor,
    Appearance,
    Accounts,
    AcpAgents,
    Updates,
}

impl Section {
    const ALL: [Section; 5] = [
        Section::Editor,
        Section::Appearance,
        Section::Accounts,
        Section::AcpAgents,
        Section::Updates,
    ];

    fn label(self) -> &'static str {
        match self {
            Section::Editor => "Editor",
            Section::Appearance => "Appearance",
            Section::Accounts => "Accounts",
            Section::AcpAgents => "ACP agents",
            Section::Updates => "Updates",
        }
    }
}

/// One ACP agent's draft: the command/prompt text fields plus the enabled
/// flag, with the last values the store published so a refresh can resync
/// fields the user isn't editing (the desktop's `synced` record).
#[derive(Debug, Default)]
pub struct AcpDraft {
    pub command: rune::edit::TextEdit,
    pub prompt: rune::edit::TextEdit,
    pub enabled: bool,
    pub synced: (String, String, bool),
}

/// The settings page's own state.
#[derive(Default)]
pub struct SettingsState {
    pub section: Section,
    /// Appearance: the theme select (rune-kit's searchable Combobox).
    pub theme_picker: rune_kit::ComboboxState,
    /// Editor: the indent select.
    pub indent_picker: rune_kit::ComboboxState,
    /// Editor: the scroll speed select.
    pub scroll_speed_picker: rune_kit::ComboboxState,
    /// Accounts: the agent whose add form is open.
    add_open: Option<String>,
    add_name: rune::edit::TextEdit,
    add_dir: rune::edit::TextEdit,
    /// The add submission in flight: its token matches the reply that may
    /// clear the form.
    add_pending: Option<u64>,
    /// Why the last add failed — the drafts stay on screen with it.
    add_error: Option<String>,
    /// The account being renamed inline: `(agent, id)`.
    rename_open: Option<(String, String)>,
    rename_text: rune::edit::TextEdit,
    /// The rename submission in flight.
    rename_pending: Option<u64>,
    /// Why the last rename failed — the inline editor stays open with it.
    rename_error: Option<String>,
    /// The account awaiting remove confirmation: `(agent, id, name)`.
    remove_confirm: Option<(String, String, String)>,
    /// ACP drafts keyed by agent id.
    acp_drafts: HashMap<String, AcpDraft>,
    /// When the OS appearance was last checked while `System` is selected.
    pub appearance_at: Option<f64>,
    /// The next account submission token — monotonic, so every reply
    /// names exactly one submission.
    account_token: u64,
}

impl SettingsState {
    /// Start an account submission and return its token. The drafts stay
    /// on screen until the matching reply arrives.
    pub fn begin_account_add(&mut self) -> u64 {
        self.account_token = self.account_token.wrapping_add(1);
        self.add_pending = Some(self.account_token);
        self.add_error = None;
        self.account_token
    }

    /// The queue refused the job: nothing was sent, so the drafts stay and
    /// the reason is shown inline.
    pub fn cancel_account_add(&mut self, token: u64, message: String) {
        if self.add_pending == Some(token) {
            self.add_pending = None;
        }
        self.add_error = Some(message);
    }

    /// The matching add succeeded: close the form and clear the drafts.
    /// A stale reply (`false`) leaves the current form alone.
    pub fn finish_account_add(&mut self, token: u64) -> bool {
        if self.add_pending != Some(token) {
            return false;
        }
        self.add_pending = None;
        self.add_open = None;
        self.add_name = rune::edit::TextEdit::new("");
        self.add_dir = rune::edit::TextEdit::new("");
        self.add_error = None;
        true
    }

    /// The matching add failed: keep the drafts, show the reason.
    pub fn fail_account_add(&mut self, token: u64, message: String) -> bool {
        if self.add_pending != Some(token) {
            return false;
        }
        self.add_pending = None;
        self.add_error = Some(message);
        true
    }

    /// Start a rename submission and return its token.
    pub fn begin_account_rename(&mut self) -> u64 {
        self.account_token = self.account_token.wrapping_add(1);
        self.rename_pending = Some(self.account_token);
        self.rename_error = None;
        self.account_token
    }

    /// The queue refused the rename: the inline editor stays open.
    pub fn cancel_account_rename(&mut self, token: u64, message: String) {
        if self.rename_pending == Some(token) {
            self.rename_pending = None;
        }
        self.rename_error = Some(message);
    }

    /// The matching rename succeeded: close the inline editor.
    pub fn finish_account_rename(&mut self, token: u64) -> bool {
        if self.rename_pending != Some(token) {
            return false;
        }
        self.rename_pending = None;
        self.rename_open = None;
        self.rename_error = None;
        true
    }

    /// The matching rename failed: keep the inline editor and the draft.
    pub fn fail_account_rename(&mut self, token: u64, message: String) -> bool {
        if self.rename_pending != Some(token) {
            return false;
        }
        self.rename_pending = None;
        self.rename_error = Some(message);
        true
    }

    /// An add submission is in flight — the form must not send a second
    /// one while the first is unanswered.
    pub fn add_busy(&self) -> bool {
        self.add_pending.is_some()
    }

    /// A rename submission is in flight.
    pub fn rename_busy(&self) -> bool {
        self.rename_pending.is_some()
    }

    /// The last add failure, shown under the form.
    pub fn add_error(&self) -> Option<&str> {
        self.add_error.as_deref()
    }

    /// The last rename failure, shown under the inline editor.
    pub fn rename_error(&self) -> Option<&str> {
        self.rename_error.as_deref()
    }
}

/// Create/resync the ACP drafts from the published agent list — cheap
/// enough to run every poll tick: fields the user hasn't touched track the
/// store (the desktop's `synced` record), edited fields keep their text.
pub fn sync_acp_drafts(s: &mut State) {
    for agent in &s.editor.acp.agents {
        match s.settings.acp_drafts.get_mut(&agent.id) {
            Some(draft) => {
                let synced = (agent.command.clone(), agent.prompt.clone(), agent.enabled);
                if synced != draft.synced {
                    // The store moved: carry it into untouched fields.
                    if draft.command.text == draft.synced.0 {
                        draft.command = rune::edit::TextEdit::new(&agent.command);
                    }
                    if draft.prompt.text == draft.synced.1 {
                        draft.prompt = rune::edit::TextEdit::new(&agent.prompt);
                    }
                    if draft.enabled == draft.synced.2 {
                        draft.enabled = agent.enabled;
                    }
                    draft.synced = synced;
                }
            }
            None => {
                s.settings
                    .acp_drafts
                    .insert(agent.id.clone(), acp_draft_of_agent(agent));
            }
        }
    }
    // Agents that left the preset list lose their drafts too.
    let ids: Vec<String> = s.editor.acp.agents.iter().map(|a| a.id.clone()).collect();
    s.settings.acp_drafts.retain(|id, _| ids.contains(id));
}

/// What the OS reports right now: the appearance AppKit already resolved
/// for the app — `System` included. A dark appearance's name contains
/// "Dark" (DarkAqua, VibrantDark, the high-contrast variants) — the same
/// check the old `defaults read` ran on its output, but a pure read of the
/// running app's state: no subprocess on the UI thread.
#[cfg(target_os = "macos")]
fn os_dark_mode() -> bool {
    use objc2_app_kit::NSAppearance;
    use objc2_foundation::NSString;
    // SAFETY: property reads on the app's own appearance objects;
    // `currentDrawingAppearance` is callable off-main and, with nothing
    // drawing, answers the app's effective appearance.
    let appearance = unsafe { NSAppearance::currentDrawingAppearance() };
    let name = unsafe { appearance.name() };
    let dark = NSString::from_str("Dark");
    unsafe { name.containsString(&dark) }
}

#[cfg(not(target_os = "macos"))]
fn os_dark_mode() -> bool {
    true
}

/// The OS appearance answer landed while `System` is selected: remember it
/// and rebuild the palette when it actually flipped. Returns whether a
/// repaint is due — the poll has to know even when nothing else changed.
pub fn apply_os_appearance(s: &mut State, dark: bool) -> bool {
    if dark == s.os_dark {
        return false;
    }
    s.os_dark = dark;
    s.apply_theme();
    true
}

/// Called every poll tick: while `System` is selected, re-read the OS
/// appearance on the same five-second cadence as the usage read and
/// rebuild the palette when it flips. Returns whether a repaint is due.
pub fn poll_appearance(s: &mut State) -> bool {
    if s.theme_choice != Theme::System {
        return false;
    }
    if s.settings
        .appearance_at
        .is_some_and(|at| s.clock - at < 5.0)
    {
        return false;
    }
    s.settings.appearance_at = Some(s.clock);
    apply_os_appearance(s, os_dark_mode())
}

// ---------------------------------------------------------------------------
// Account actions — all through the editor backend's worker lane.
// ---------------------------------------------------------------------------

impl State {
    fn account_job(&mut self, req: Req) {
        let _ = backend::enqueue(&mut self.editor.backend, Job::project(req));
    }

    fn add_account(&mut self, agent: &str) {
        if self.settings.add_busy() {
            self.settings.add_error = Some("the account is still being added".into());
            return;
        }
        let name = self.settings.add_name.text.trim().to_string();
        let dir = self.settings.add_dir.text.trim().to_string();
        if name.is_empty() {
            self.settings.add_error = Some("the account name is empty".into());
            return;
        }
        if dir.is_empty() {
            self.settings.add_error = Some("the account folder is empty".into());
            return;
        }
        // The drafts stay until the matching reply arrives: a refused or
        // failed add must not lose what the user typed.
        let token = self.settings.begin_account_add();
        let queued = backend::enqueue(
            &mut self.editor.backend,
            Job::project(Req::AccountAdd {
                agent: agent.to_string(),
                name,
                dir,
                token,
            }),
        );
        if !queued {
            self.settings.cancel_account_add(
                token,
                "the editor queue is full; the account was not added".into(),
            );
        }
    }

    fn rename_account(&mut self, agent: &str, id: &str) {
        if self.settings.rename_busy() {
            self.settings.rename_error = Some("the account is still being renamed".into());
            return;
        }
        let name = self.settings.rename_text.text.trim().to_string();
        if name.is_empty() {
            self.settings.rename_error = Some("the account name is empty".into());
            return;
        }
        // The inline editor stays open until the matching reply arrives.
        let token = self.settings.begin_account_rename();
        let queued = backend::enqueue(
            &mut self.editor.backend,
            Job::project(Req::AccountRename {
                agent: agent.to_string(),
                id: id.to_string(),
                name,
                token,
            }),
        );
        if !queued {
            self.settings.cancel_account_rename(
                token,
                "the editor queue is full; the account was not renamed".into(),
            );
        }
    }

    /// Reset one ACP agent: drop the local draft and remove the stored
    /// override, so the next store read shows the preset again. Dropping
    /// only the draft would let the next sync recreate the saved row.
    fn reset_acp_agent(&mut self, id: &str) {
        self.settings.acp_drafts.remove(id);
        self.editor.acp.reset_agent(id);
    }
}

// ---------------------------------------------------------------------------
// The page
// ---------------------------------------------------------------------------

/// The settings view: section rail + the active section's card.
pub fn view(s: &State, ink: &Ink) -> El {
    let mut layer = Vec::new();
    let mut rail = col().w(176.0).h_full().min_h(0.0).gap(2.0).padding(12.0);
    for section in Section::ALL {
        let active = s.settings.section == section;
        let tone = if active { ink.text } else { ink.muted };
        let hover_tone = ink.text;
        let mut b = row()
            .key(format!("settings-section:{}", section.label()))
            .w_full()
            .h(28.0)
            .px(8.0)
            .items_center()
            .radius(6.0)
            .hover_group()
            .hover(if active {
                rune::Color::TRANSPARENT
            } else {
                ink.text.alpha(0.06)
            })
            .press_scale(0.96)
            .motion(theme::STATE_SPRING)
            .role("tab")
            .label(section.label())
            .focusable(true)
            .focus_selected(active)
            .focus_ring(2.0, 1.0, ink.accent);
        if active {
            b = b.child(
                el().key("thumb")
                    .absolute(0.0, 0.0)
                    .w_full()
                    .h_full()
                    .radius(6.0)
                    .fill(ink.text.alpha(0.10))
                    .matched("settings-section-thumb"),
            );
        }
        b = b
            .child(
                theme::txt(section.label(), T12M, tone)
                    .key("label")
                    .group_color(hover_tone),
            )
            .on_press(move |s: &mut State| {
                s.settings.section = section;
            });
        rail = rail.child(b);
    }

    let root = row()
        .grow(1.0)
        .w_full()
        .h_full()
        .fill(ink.bg)
        .child(
            col()
                .w(176.0)
                .h_full()
                .fill(ink.panel)
                .border_sides([0.0, 1.0, 0.0, 0.0], ink.border)
                .child(rail),
        )
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .h_full()
                .min_h(0.0)
                .gap(16.0)
                .padding(24.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(theme::txt(s.settings.section.label(), T14M, ink.text))
                .child(section_body(s, ink, &mut layer)),
        );
    rune_kit::root(root, layer).grow(1.0).min_w(0.0).min_h(0.0)
}

fn section_body(s: &State, ink: &Ink, layer: &mut Vec<El>) -> El {
    match s.settings.section {
        Section::Editor => editor_section(s, ink, layer),
        Section::Appearance => appearance_section(s, ink, layer),
        Section::Accounts => accounts_section(s, ink),
        Section::AcpAgents => acp_section(s, ink),
        Section::Updates => updates_section(s, ink),
    }
}

/// A labelled field row: the desktop's `<Field>` — label, control, muted
/// description underneath.
fn field(label: &str, description: &str, control: El, ink: &Ink) -> El {
    col()
        .w_full()
        .gap(4.0)
        .child(theme::txt(label, T12M, ink.text))
        .child(control)
        .child(theme::txt(description, T11M.lh(16.0), ink.muted))
}

/// A small ghost button for the settings actions (`btn_sm` outline).
fn ghost(label: &str, ink: &Ink) -> El {
    theme::btn_sm(label, None, true, ink)
}

fn text_input(
    edit: impl Fn(&mut State) -> &mut rune::edit::TextEdit + Clone + 'static,
    state: &rune::edit::TextEdit,
    placeholder: &'static str,
    s: &State,
) -> El {
    rune_kit::TextInput::new(state)
        .placeholder(placeholder)
        .width(Some(320.0))
        .on_edit(edit)
        .build(&s.editor.kit)
}

/// The draft a field edits — `poll` keeps every preset agent's draft alive,
/// so the accessor and the display point at the same `TextEdit`.
fn acp_draft_for<'a>(s: &'a State, id: &str) -> &'a AcpDraft {
    s.settings
        .acp_drafts
        .get(id)
        .unwrap_or_else(|| acp_empty_draft())
}

fn acp_empty_draft() -> &'static AcpDraft {
    static EMPTY: std::sync::OnceLock<AcpDraft> = std::sync::OnceLock::new();
    EMPTY.get_or_init(|| AcpDraft {
        command: rune::edit::TextEdit::new(""),
        prompt: rune::edit::TextEdit::new(""),
        enabled: false,
        synced: Default::default(),
    })
}

// ---------------------------------------------------------------------------
// Editor — the native editor preferences (indent); Vim is deferred, so no
// inert toggle is rendered.
// ---------------------------------------------------------------------------

fn editor_section(s: &State, ink: &Ink, layer: &mut Vec<El>) -> El {
    let indent_options: Vec<String> = INDENT_OPTIONS.iter().map(|(_, l)| l.to_string()).collect();
    let indent_current = INDENT_OPTIONS
        .iter()
        .find(|(choice, _)| *choice == s.editor.indent_choice)
        .map(|(_, l)| *l)
        .unwrap_or("Auto-detect");

    let speed_options: Vec<String> = SCROLL_SPEED_OPTIONS
        .iter()
        .map(|(_, l)| l.to_string())
        .collect();
    let speed_current = SCROLL_SPEED_OPTIONS
        .iter()
        .find(|(val, _)| (*val - s.scroll_speed).abs() < 0.01)
        .map(|(_, l)| *l)
        .unwrap_or("1.0× (Normal)");

    col()
        .w_full()
        .gap(20.0)
        .child(field(
            "Indentation",
            "How the editor indents: Auto-detect sniffs the file, the rest pin a style.",
            rune_kit::Combobox::new(
                "settings-indent",
                &s.settings.indent_picker,
                &crate::picker::options(&indent_options),
                |s: &mut State| &mut s.settings.indent_picker,
            )
            .current(Some(indent_current))
            .placeholder("Auto-detect")
            .label("Indentation")
            .search_placeholder("Search indentation")
            .width(176.0)
            .popover_width(crate::picker::list_width(176.0))
            .on_choose(|s: &mut State, label: &str, _tx: &mut Tx| {
                if let Some((choice, _)) = INDENT_OPTIONS.iter().find(|(_, l)| *l == label) {
                    s.editor.set_indent_choice(*choice);
                }
            })
            .build(&s.editor.kit, layer),
            ink,
        ))
        .child(field(
            "Scroll Speed",
            "Multiplier for mouse wheel scrolling in terminal panes and views.",
            rune_kit::Combobox::new(
                "settings-scroll-speed",
                &s.settings.scroll_speed_picker,
                &crate::picker::options(&speed_options),
                |s: &mut State| &mut s.settings.scroll_speed_picker,
            )
            .current(Some(speed_current))
            .placeholder("1.0× (Normal)")
            .label("Scroll Speed")
            .search_placeholder("Search speeds")
            .width(176.0)
            .popover_width(crate::picker::list_width(176.0))
            .on_choose(|s: &mut State, label: &str, _tx: &mut Tx| {
                if let Some((speed, _)) = SCROLL_SPEED_OPTIONS.iter().find(|(_, l)| *l == label) {
                    s.scroll_speed = *speed;
                    s.account_job(Req::SetSetting {
                        key: SCROLL_SPEED_KEY.to_string(),
                        value: speed.to_string(),
                    });
                }
            })
            .build(&s.editor.kit, layer),
            ink,
        ))
}

/// The indent picker's options: the labels UnitChoice parses back from.
const INDENT_OPTIONS: [(UnitChoice, &str); 5] = [
    (UnitChoice::Auto, "Auto-detect"),
    (UnitChoice::Fixed(indent::IndentUnit::Spaces(2)), "2 spaces"),
    (UnitChoice::Fixed(indent::IndentUnit::Spaces(4)), "4 spaces"),
    (UnitChoice::Fixed(indent::IndentUnit::Spaces(8)), "8 spaces"),
    (UnitChoice::Fixed(indent::IndentUnit::Tab), "Tabs"),
];

/// The key under which scroll speed multiplier is stored.
pub const SCROLL_SPEED_KEY: &str = "goko.scroll_speed";

/// Scroll speed multiplier options: (multiplier, label).
pub const SCROLL_SPEED_OPTIONS: [(f32, &str); 6] = [
    (0.5, "0.5× (Slow)"),
    (0.75, "0.75× (Comfortable)"),
    (1.0, "1.0× (Normal)"),
    (1.25, "1.25× (Fast)"),
    (1.5, "1.5× (Faster)"),
    (2.0, "2.0× (Very Fast)"),
];

// ---------------------------------------------------------------------------
// Appearance — `goko.theme`, persisted through the settings lane.
// ---------------------------------------------------------------------------

fn appearance_section(s: &State, ink: &Ink, layer: &mut Vec<El>) -> El {
    let options: Vec<String> = Theme::OPTIONS
        .iter()
        .map(|t| t.label().to_string())
        .collect();
    field(
        "Theme",
        "Dark+ is the default. System follows the OS appearance.",
        rune_kit::Combobox::new(
            "settings-theme",
            &s.settings.theme_picker,
            &crate::picker::options(&options),
            |s: &mut State| &mut s.settings.theme_picker,
        )
        .current(Some(s.theme_choice.label()))
        .placeholder("System")
        .label("Theme")
        .search_placeholder("Search themes")
        .width(176.0)
        .popover_width(crate::picker::list_width(176.0))
        .on_choose(|s: &mut State, label: &str, _tx: &mut Tx| {
            if let Some(choice) = Theme::OPTIONS.iter().find(|t| t.label() == label) {
                s.theme_choice = *choice;
                // A fresh OS read keeps System honest at pick time.
                s.os_dark = os_dark_mode();
                s.apply_theme();
                s.account_job(Req::SetSetting {
                    key: theme::THEME_KEY.to_string(),
                    value: choice.id().to_string(),
                });
            }
        })
        .build(&s.editor.kit, layer),
        ink,
    )
}

// ---------------------------------------------------------------------------
// Updates — the running version and whether a newer release is out.
// ---------------------------------------------------------------------------

fn updates_section(s: &State, ink: &Ink) -> El {
    use crate::update::{self, Status};

    let (line, color) = match &s.update {
        Status::Unknown => (
            "Goko looks for a newer release when it starts.".to_string(),
            ink.muted,
        ),
        Status::Checking => ("Checking…".to_string(), ink.muted),
        Status::UpToDate => ("You are on the latest version.".to_string(), ink.muted),
        Status::Available(version) => (format!("Goko {version} is available."), ink.accent),
        Status::Failed(error) => (format!("Could not check: {error}"), ink.danger),
    };
    let mut actions = row().gap(8.0).items_center().child(
        ghost("Check for updates", ink)
            .key("update-check")
            .on_press(|s: &mut State| s.check_for_update()),
    );
    if matches!(s.update, Status::Available(_)) {
        actions = actions.child(
            ghost("Copy install command", ink)
                .key("update-copy")
                .on_press_tx(|s: &mut State, tx: &mut Tx| {
                    tx.copy(update::INSTALL_COMMAND.to_string());
                    s.notice = Some("Install command copied — paste it in a terminal".into());
                }),
        );
    }
    col()
        .w_full()
        .gap(20.0)
        .child(field(
            "Version",
            "The version of Goko that is running.",
            theme::txt(&format!("Goko {}", update::CURRENT), T14M, ink.text),
            ink,
        ))
        .child(field(
            "Updates",
            "To update, run the install command in a terminal, then reopen Goko. \
             Set GOKO_NO_UPDATE_CHECK=1 to stop the automatic check.",
            col()
                .gap(8.0)
                .child(theme::txt(&line, T12, color))
                .child(actions),
            ink,
        ))
}

// ---------------------------------------------------------------------------
// Accounts — per-agent lists plus add/rename/remove/set-active/sign-in.
// ---------------------------------------------------------------------------

fn agent_accounts<'a>(s: &'a State, agent: &str) -> Option<&'a AgentView> {
    let view = s.data.accounts.as_ref()?;
    Some(match agent {
        "claude" => &view.claude,
        _ => &view.codex,
    })
}

fn accounts_section(s: &State, ink: &Ink) -> El {
    let mut body = col().w_full().gap(20.0);
    for (agent, title) in [("claude", "Claude Code"), ("codex", "Codex")] {
        body = body.child(agent_card(s, ink, agent, title));
    }
    body
}

fn agent_card(s: &State, ink: &Ink, agent: &str, title: &str) -> El {
    let mut card = col()
        .w_full()
        .max_w(560.0)
        .gap(8.0)
        .padding(12.0)
        .radius(8.0)
        .border(1.0, ink.border)
        .fill(ink.panel)
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .child(theme::txt(title, T12M, ink.text))
                .child(if s.settings.add_open.as_deref() == Some(agent) {
                    el().size(0.0, 0.0)
                } else {
                    ghost("Add account", ink).on_press_tx({
                        let agent = agent.to_string();
                        move |s: &mut State, _tx: &mut Tx| {
                            s.settings.add_open = Some(agent.clone());
                            s.settings.add_name = rune::edit::TextEdit::new("");
                            s.settings.add_dir = rune::edit::TextEdit::new("");
                            s.settings.add_error = None;
                        }
                    })
                }),
        );
    let view = agent_accounts(s, agent);
    match view {
        None => {
            card = card.child(theme::txt("Accounts are loading…", T12, ink.muted));
        }
        Some(view) => {
            for item in &view.items {
                card = card.child(account_row(s, ink, agent, view, item));
            }
        }
    }
    if s.settings.add_open.as_deref() == Some(agent) {
        card = card.child(add_form(s, ink, agent));
    }
    card
}

fn account_row(
    s: &State,
    ink: &Ink,
    agent: &str,
    view: &AgentView,
    item: &app_core::accounts::AccountView,
) -> El {
    let active = item.id == view.active;
    let row_el = row()
        .w_full()
        .items_center()
        .gap(8.0)
        .child(el().size(6.0, 6.0).radius(3.0).fill(if active {
            ink.success
        } else {
            ink.muted.alpha(0.3)
        }))
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .child({
                    theme::txt(&item.name, T12M, ink.text)
                        .min_w(0.0)
                        .truncate_middle()
                        .help_when_truncated(item.name.clone())
                })
                .child({
                    theme::txt(&item.dir, T11M.lh(16.0), ink.muted)
                        .min_w(0.0)
                        .truncate_middle()
                        .help_when_truncated(item.dir.clone())
                }),
        );
    if let Some((rename_agent, rename_id)) = &s.settings.rename_open
        && rename_agent == agent
        && rename_id == &item.id
    {
        let agent = agent.to_string();
        let id = item.id.clone();
        let mut editor = col().w_full().gap(4.0).child(row_el).child(
            row()
                .gap(8.0)
                .items_center()
                .child(text_input(
                    |s: &mut State| &mut s.settings.rename_text,
                    &s.settings.rename_text,
                    "Account name",
                    s,
                ))
                .child(ghost("Save", ink).on_press(move |s: &mut State| {
                    s.rename_account(&agent, &id);
                }))
                .child(ghost("Cancel", ink).on_press(|s: &mut State| {
                    s.settings.rename_open = None;
                    s.settings.rename_error = None;
                })),
        );
        // A failed rename keeps the inline editor and the typed name.
        if let Some(error) = s.settings.rename_error() {
            editor = editor.child(theme::txt(error, T11M.lh(16.0), ink.danger));
        }
        return editor;
    }
    if let Some((agent_r, id_r, name_r)) = &s.settings.remove_confirm
        && agent_r == agent
        && id_r == &item.id
    {
        let agent = agent.to_string();
        let id = item.id.clone();
        return col().w_full().gap(4.0).child(row_el).child(
            row()
                .gap(8.0)
                .items_center()
                .child(theme::txt(
                    &format!("Remove {name_r}? Its folder and sign-in stay on disk."),
                    T12,
                    ink.muted,
                ))
                .child(ghost("Remove", ink).on_press(move |s: &mut State| {
                    s.settings.remove_confirm = None;
                    s.account_job(Req::AccountRemove {
                        agent: agent.clone(),
                        id: id.clone(),
                    });
                }))
                .child(ghost("Cancel", ink).on_press(|s: &mut State| {
                    s.settings.remove_confirm = None;
                })),
        );
    }
    let mut actions = row().gap(4.0).items_center();
    if !active {
        let agent = agent.to_string();
        let id = item.id.clone();
        actions = actions.child(ghost("Set active", ink).on_press(move |s: &mut State| {
            s.account_job(Req::AccountSetActive {
                agent: agent.clone(),
                id: id.clone(),
            });
        }));
    }
    // Default has no sign-in of its own — `accounts::sign_in_launch`
    // refuses it; the user signs in from a terminal.
    if !item.is_default {
        let agent = agent.to_string();
        let id = item.id.clone();
        actions = actions.child(ghost("Sign in", ink).on_press(move |s: &mut State| {
            s.request_sign_in(&agent, &id);
        }));
    }
    if !item.is_default {
        actions = actions.child(ghost("Rename", ink).on_press({
            let agent = agent.to_string();
            let id = item.id.clone();
            let current = item.name.clone();
            move |s: &mut State| {
                s.settings.rename_open = Some((agent.clone(), id.clone()));
                s.settings.rename_text = rune::edit::TextEdit::new(&current);
                s.settings.rename_error = None;
            }
        }));
        actions = actions.child(ghost("Remove", ink).on_press({
            let agent = agent.to_string();
            let id = item.id.clone();
            let name = item.name.clone();
            move |s: &mut State| {
                s.settings.remove_confirm = Some((agent.clone(), id.clone(), name.clone()));
            }
        }));
    }
    row_el.child(actions)
}

/// The add-account form: name, folder (typed or picked), Add/Cancel. The
/// add leaves the account active and the UI asks the worker for its
/// sign-in command automatically — main's add→sign-in flow.
fn add_form(s: &State, ink: &Ink, agent: &str) -> El {
    let agent = agent.to_string();
    let mut form = col()
        .w_full()
        .gap(8.0)
        .padding(8.0)
        .radius(6.0)
        .border(1.0, ink.border)
        .child(theme::txt("New account", T12M, ink.text))
        .child(
            row()
                .gap(8.0)
                .items_center()
                .child(text_input(
                    |s: &mut State| &mut s.settings.add_name,
                    &s.settings.add_name,
                    "Account name",
                    s,
                ))
                .child(
                    row()
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(8.0)
                        .items_center()
                        .child(
                            text_input(
                                |s: &mut State| &mut s.settings.add_dir,
                                &s.settings.add_dir,
                                "Account folder",
                                s,
                            )
                            .grow(1.0)
                            .min_w(0.0),
                        )
                        .child(ghost("Choose…", ink).on_press_tx({
                            let agent = agent.clone();
                            move |_s: &mut State, tx: &mut Tx| {
                                tx.open_folder(
                                    rune::file_dialog::FileDialogOptions {
                                        title: "Choose the account folder".into(),
                                        ..Default::default()
                                    },
                                    {
                                        let agent = agent.clone();
                                        move |s: &mut State, result, _tx: &mut Tx| {
                                            match result {
                                                Ok(Some(path)) => {
                                                    s.settings.add_dir.text =
                                                        path.display().to_string();
                                                }
                                                Ok(None) => {}
                                                Err(e) => s.notice = Some(e.to_string()),
                                            }
                                            let _ = agent;
                                        }
                                    },
                                );
                            }
                        })),
                ),
        )
        .child(
            row()
                .gap(8.0)
                .items_center()
                .child(ghost("Add", ink).on_press({
                    let agent = agent.clone();
                    move |s: &mut State| s.add_account(&agent)
                }))
                .child(ghost("Cancel", ink).on_press(|s: &mut State| {
                    s.settings.add_open = None;
                    s.settings.add_error = None;
                })),
        );
    // The failure stays with the drafts: the user can fix the name or the
    // folder and press Add again.
    if let Some(error) = s.settings.add_error() {
        form = form.child(theme::txt(error, T11M.lh(16.0), ink.danger));
    }
    form
}

// ---------------------------------------------------------------------------
// ACP agents — enabled toggle + command + prompt drafts, saved together
// through `acp::agents_set` on the ACP runtime (it owns the hub).
// ---------------------------------------------------------------------------

fn acp_section(s: &State, ink: &Ink) -> El {
    let mut body = col().w_full().gap(12.0);
    for agent in &s.editor.acp.agents {
        body = body.child(acp_card(s, ink, agent));
    }
    if s.editor.acp.agents.is_empty() {
        body = body.child(theme::txt("ACP agents are loading…", T12, ink.muted));
    }
    body
}

fn acp_card(s: &State, ink: &Ink, agent: &AcpAgent) -> El {
    let draft = acp_draft_for(s, &agent.id);
    let status = s
        .editor
        .acp
        .statuses
        .get(&agent.id)
        .map(|status| format!("{:?}", status.state))
        .unwrap_or_else(|| {
            if agent.available {
                "available".to_string()
            } else {
                "unavailable".to_string()
            }
        });
    let id = agent.id.clone();
    col()
        .w_full()
        .max_w(560.0)
        .gap(8.0)
        .padding(12.0)
        .radius(8.0)
        .border(1.0, ink.border)
        .fill(ink.panel)
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    row()
                        .gap(8.0)
                        .items_center()
                        .child(theme::txt(&agent.label, T12M, ink.text))
                        .child(theme::txt(&status, T11M.lh(16.0), ink.muted)),
                )
                .child(
                    rune_kit::Checkbox::new(draft.enabled)
                        .label("Enabled")
                        .on_toggle({
                            let id = id.clone();
                            move |s: &mut State| {
                                s.settings.acp_drafts.entry(id.clone()).or_default().enabled =
                                    !s.settings.acp_drafts[&id].enabled;
                            }
                        })
                        .build(&s.editor.kit),
                ),
        )
        .child(theme::txt("Command", T11M.lh(16.0), ink.muted))
        .child(text_input(
            {
                let id = id.clone();
                move |s: &mut State| -> &mut rune::edit::TextEdit {
                    &mut s.settings.acp_drafts.entry(id.clone()).or_default().command
                }
            },
            &draft.command,
            "Agent command",
            s,
        ))
        .child(theme::txt("Prompt", T11M.lh(16.0), ink.muted))
        .child(text_input(
            {
                let id = id.clone();
                move |s: &mut State| -> &mut rune::edit::TextEdit {
                    &mut s.settings.acp_drafts.entry(id.clone()).or_default().prompt
                }
            },
            &draft.prompt,
            "Instructions",
            s,
        ))
        .child(
            row()
                .gap(8.0)
                .items_center()
                .child(ghost("Save", ink).on_press({
                    move |s: &mut State| {
                        let mut overrides = Vec::new();
                        for agent in &s.editor.acp.agents {
                            let draft = s.settings.acp_drafts.get(&agent.id);
                            overrides.push(app_core::acp::AcpOverride {
                                id: agent.id.clone(),
                                enabled: draft.map(|d| d.enabled).unwrap_or(agent.enabled),
                                command: draft
                                    .map(|d| d.command.text.clone())
                                    .unwrap_or_else(|| agent.command.clone()),
                                prompt: draft
                                    .map(|d| d.prompt.text.clone())
                                    .unwrap_or_else(|| agent.prompt.clone()),
                            });
                        }
                        s.editor.acp.set_agents(overrides);
                    }
                }))
                .child(ghost("Reset", ink).on_press({
                    let id = id.clone();
                    move |s: &mut State| {
                        // Discard the edits and remove the stored override:
                        // dropping only the local draft lets the next store
                        // sync recreate the customized row.
                        s.reset_acp_agent(&id);
                    }
                })),
        )
}

fn acp_draft_of_agent(agent: &AcpAgent) -> AcpDraft {
    AcpDraft {
        command: rune::edit::TextEdit::new(&agent.command),
        prompt: rune::edit::TextEdit::new(&agent.prompt),
        enabled: agent.enabled,
        synced: (agent.command.clone(), agent.prompt.clone(), agent.enabled),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::backend::ReplyData;

    fn agent(id: &str, enabled: bool, command: &str, prompt: &str) -> AcpAgent {
        AcpAgent {
            id: id.into(),
            label: id.into(),
            enabled,
            command: command.into(),
            prompt: prompt.into(),
            available: true,
            is_default: false,
        }
    }

    #[test]
    fn acp_drafts_track_the_published_agents_and_keep_edits() {
        let mut s = State::new(crate::test_handle());
        s.editor.acp.agents = vec![agent("opencode", true, "opencode acp", "p")];
        sync_acp_drafts(&mut s);
        assert!(s.settings.acp_drafts.contains_key("opencode"));

        // A user edit survives a resync of the same store values.
        let draft = s.settings.acp_drafts.get_mut("opencode").unwrap();
        draft.command = rune::edit::TextEdit::new("custom cmd");
        sync_acp_drafts(&mut s);
        assert_eq!(s.settings.acp_drafts["opencode"].command.text, "custom cmd");

        // A store-side change lands in untouched fields only.
        s.editor.acp.agents = vec![agent("opencode", true, "opencode acp --new", "p2")];
        sync_acp_drafts(&mut s);
        let draft = &s.settings.acp_drafts["opencode"];
        assert_eq!(draft.command.text, "custom cmd", "the edit is kept");
        assert_eq!(
            draft.prompt.text, "p2",
            "untouched prompt followed the store"
        );

        // A removed preset drops its draft.
        s.editor.acp.agents = vec![];
        sync_acp_drafts(&mut s);
        assert!(s.settings.acp_drafts.is_empty());
    }

    #[test]
    fn theme_labels_round_trip_the_option_list() {
        for theme in Theme::OPTIONS {
            assert_eq!(Theme::parse(theme.id()), theme);
        }
    }

    #[test]
    fn scroll_speed_options_include_normal_and_parseable_floats() {
        assert!(
            SCROLL_SPEED_OPTIONS
                .iter()
                .any(|(speed, label)| *speed == 1.0 && label.contains("Normal"))
        );
        for (speed, label) in SCROLL_SPEED_OPTIONS {
            assert!(speed > 0.0 && speed <= 10.0);
            assert!(!label.is_empty());
        }
    }

    #[test]
    fn a_system_appearance_flip_repaints_only_on_change() {
        let mut s = State::new(crate::test_handle());
        s.theme_choice = Theme::System;
        s.os_dark = true;
        assert!(
            !apply_os_appearance(&mut s, true),
            "the same answer needs no repaint"
        );
        assert!(apply_os_appearance(&mut s, false), "a flip repaints");
        assert!(!s.os_dark);
        assert_eq!(s.ink.bg, Ink::for_theme(Theme::Light).bg);
        assert!(
            !apply_os_appearance(&mut s, false),
            "settled again: no repaint"
        );
    }

    #[test]
    fn poll_appearance_stays_off_the_os_unless_system_is_due() {
        let mut s = State::new(crate::test_handle());
        // A fixed theme never asks the OS at all.
        s.theme_choice = Theme::Dark;
        assert!(!poll_appearance(&mut s));
        // And inside the cadence window a System choice doesn't either.
        s.theme_choice = Theme::System;
        s.settings.appearance_at = Some(s.clock);
        assert!(!poll_appearance(&mut s), "inside the five-second window");
        assert_eq!(s.settings.appearance_at, Some(s.clock));
    }

    // -- accounts: the drafts survive a refused or failed submission -------

    fn accounts_view() -> app_core::accounts::AccountsView {
        app_core::accounts::AccountsView {
            claude: app_core::accounts::AgentView {
                active: "default".into(),
                items: vec![app_core::accounts::AccountView {
                    id: "default".into(),
                    name: "Default".into(),
                    dir: "/me/.claude".into(),
                    is_default: true,
                }],
            },
            codex: app_core::accounts::AgentView {
                active: "default".into(),
                items: vec![app_core::accounts::AccountView {
                    id: "default".into(),
                    name: "Default".into(),
                    dir: "/me/.codex".into(),
                    is_default: true,
                }],
            },
        }
    }

    fn add_reply(token: u64, result: Result<ReplyData, String>) -> crate::editor::backend::Reply {
        crate::editor::backend::Reply {
            req: Req::AccountAdd {
                agent: "claude".into(),
                name: "work".into(),
                dir: "/accts/work".into(),
                token,
            },
            generation: 0,
            checkout_key: None,
            result,
        }
    }

    /// A failed add keeps what the user typed and shows why.
    #[test]
    fn a_failed_add_keeps_the_drafts_and_shows_the_reason() {
        let mut s = State::new(crate::test_handle());
        s.settings.add_open = Some("claude".into());
        s.settings.add_name = rune::edit::TextEdit::new("work");
        s.settings.add_dir = rune::edit::TextEdit::new("/accts/work");
        let token = s.settings.begin_account_add();
        assert!(s.settings.add_busy(), "the submission is in flight");

        s.apply_reply(add_reply(token, Err("the account name is taken".into())));

        assert_eq!(s.settings.add_name.text, "work", "the name survives");
        assert_eq!(
            s.settings.add_dir.text, "/accts/work",
            "the folder survives"
        );
        assert_eq!(
            s.settings.add_error(),
            Some("the account name is taken"),
            "the reason is shown"
        );
        assert_eq!(s.notice.as_deref(), Some("the account name is taken"));
        assert!(s.settings.add_open.is_some(), "the form stays open");
        assert!(!s.settings.add_busy());
    }

    /// A queue refusal never reaches the worker: the drafts stay and the
    /// reason is shown.
    #[test]
    fn a_refused_add_keeps_the_drafts() {
        let mut s = State::new(crate::test_handle());
        s.settings.add_open = Some("claude".into());
        s.settings.add_name = rune::edit::TextEdit::new("work");
        s.settings.add_dir = rune::edit::TextEdit::new("/accts/work");
        let token = s.settings.begin_account_add();
        s.settings
            .cancel_account_add(token, "the editor queue is full".into());
        assert_eq!(s.settings.add_name.text, "work");
        assert_eq!(s.settings.add_dir.text, "/accts/work");
        assert_eq!(s.settings.add_error(), Some("the editor queue is full"));
        assert!(!s.settings.add_busy());
    }

    /// A stale reply must not clear the form the user is filling now.
    #[test]
    fn a_stale_add_reply_leaves_the_current_form_alone() {
        let mut s = State::new(crate::test_handle());
        s.settings.add_open = Some("claude".into());
        s.settings.add_name = rune::edit::TextEdit::new("work");
        s.settings.add_dir = rune::edit::TextEdit::new("/accts/work");
        let stale = s.settings.begin_account_add();
        let current = s.settings.begin_account_add();

        s.apply_reply(add_reply(stale, Ok(ReplyData::Accounts(accounts_view()))));

        assert_eq!(s.settings.add_name.text, "work", "the drafts stay");
        assert!(s.settings.add_open.is_some(), "the form stays open");
        assert!(s.settings.add_busy(), "the current submission still waits");

        // The matching reply closes the form and clears the drafts.
        s.apply_reply(add_reply(current, Ok(ReplyData::Accounts(accounts_view()))));
        assert!(s.settings.add_open.is_none());
        assert!(s.settings.add_name.text.is_empty());
        assert!(s.settings.add_dir.text.is_empty());
    }

    /// A failed rename keeps the inline editor and the typed name.
    #[test]
    fn a_failed_rename_keeps_the_inline_editor() {
        let mut s = State::new(crate::test_handle());
        s.settings.rename_open = Some(("claude".into(), "aa11bb22".into()));
        s.settings.rename_text = rune::edit::TextEdit::new("work-renamed");
        let token = s.settings.begin_account_rename();

        s.apply_reply(crate::editor::backend::Reply {
            req: Req::AccountRename {
                agent: "claude".into(),
                id: "aa11bb22".into(),
                name: "work-renamed".into(),
                token,
            },
            generation: 0,
            checkout_key: None,
            result: Err("no such account".into()),
        });

        assert_eq!(s.settings.rename_text.text, "work-renamed");
        assert_eq!(s.settings.rename_error(), Some("no such account"));
        assert_eq!(
            s.settings.rename_open,
            Some(("claude".to_string(), "aa11bb22".to_string())),
            "the inline editor stays open"
        );
    }
}
