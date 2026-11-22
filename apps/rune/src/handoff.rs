//! Hand off the focused pane of a tab to another agent or account: review
//! the bounded context the worker read, choose an installed preset and one
//! of its accounts, then open a fresh tab in the pane's checkout. The
//! approved text is copied only once that session exists; nothing is ever
//! typed into a terminal, passed as argv or sent to a model.

use app_core::accounts::DEFAULT_ID;
use app_core::handoff::{DRAFT_LIMIT, Prepared};
use rune::edit::TextEdit;
use rune::{El, Key, KeyEvent, ScrollSpec, Tx, col, el, row};
use rune_kit::{ButtonVariant, RadioGroup, RadioItem};

use crate::editor::backend::{Job, Req};
use crate::sessions::SessionLaunch;
use crate::{State, theme};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    /// The worker is reading the source conversation.
    #[default]
    Preparing,
    /// The review is editable and the destination can be chosen.
    Review,
    /// The worker is resolving the destination's launch.
    Resolving,
    /// `session.create` is dispatched; only its answer closes the dialog.
    Creating,
}

#[derive(Default)]
pub struct HandoffState {
    pub open: bool,
    /// The request the dialog waits for; any other reply is stale.
    pub token: u64,
    pub phase: Phase,
    /// The source pane's session key.
    pub session: String,
    pub source_agent: String,
    pub source_account: String,
    pub project_id: String,
    pub checkout: String,
    pub prepared: Option<Prepared>,
    /// Claude accounts that share the source transcript's history.
    pub fork_accounts: Vec<String>,
    pub review: TextEdit,
    pub manual_id: TextEdit,
    pub agent: String,
    pub account: String,
    pub fork: bool,
    /// The approved text, held for the create it was confirmed with.
    pub copy: Option<String>,
    pub error: Option<String>,
    pub return_focus: Option<String>,
}

/// The pane a handoff starts from: the focused pane of `tab_id`, which must
/// run a built-in agent preset in a registered project.
struct SourcePane {
    key: String,
    agent: String,
    project_id: String,
    checkout: String,
    argv: Vec<String>,
}

fn source_pane(s: &State, tab_id: &str) -> Result<SourcePane, &'static str> {
    let tab = s
        .open_tabs
        .iter()
        .find(|tab| tab.id == tab_id)
        .ok_or("This tab is no longer open")?;
    let pane = crate::panes::pane_by_id(&tab.layout, &tab.active_pane)
        .ok_or("The focused pane is unavailable")?;
    let view = s
        .session_by_key(&pane.session)
        .ok_or("The focused pane is unavailable")?;
    let meta = view
        .meta
        .as_ref()
        .ok_or("This pane was not started from an agent preset")?;
    if meta.agent == "shell" || app_core::agents::label(&meta.agent).is_none() {
        return Err("Shells and custom commands are not agent presets");
    }
    let project_id = meta
        .project_id
        .clone()
        .ok_or("Only panes in a registered project can be handed off")?;
    Ok(SourcePane {
        key: view.info.id.clone(),
        agent: meta.agent.clone(),
        project_id,
        checkout: meta.cwd.clone(),
        argv: view.info.argv.clone(),
    })
}

/// Why the tab menu's Hand off item is disabled, if it is.
pub fn unavailable(s: &State, tab_id: &str) -> Option<&'static str> {
    source_pane(s, tab_id).err()
}

/// The source account's name: the stored account whose folder the launch
/// used, else Default.
fn account_name(s: &State, agent: &str, argv: &[String]) -> String {
    let Some(dir) =
        app_core::handoff::source_profile(agent, argv, &app_core::handoff::Folders::default())
    else {
        return "Default".into();
    };
    let dir = dir.display().to_string();
    s.data
        .accounts
        .as_ref()
        .and_then(|accounts| accounts.agent(agent))
        .and_then(|view| view.items.iter().find(|item| item.dir == dir))
        .map_or(dir, |item| item.name.clone())
}

fn next_token(s: &mut State) -> u64 {
    s.handoff_token = s.handoff_token.wrapping_add(1);
    s.handoff.token = s.handoff_token;
    s.handoff_token
}

/// Open the dialog for the focused pane of `tab_id`.
pub fn open(s: &mut State, tab_id: &str, tx: &mut Tx) {
    if s.handoff.open {
        return;
    }
    let source = match source_pane(s, tab_id) {
        Ok(source) => source,
        Err(reason) => {
            s.notice = Some(reason.into());
            return;
        }
    };
    let account = account_name(s, &source.agent, &source.argv);
    let agent = if destinations(s).iter().any(|(id, _)| *id == source.agent) {
        source.agent.clone()
    } else {
        destinations(s)
            .first()
            .map(|(id, _)| id.clone())
            .unwrap_or_default()
    };
    s.handoff = HandoffState {
        open: true,
        session: source.key,
        source_agent: source.agent,
        source_account: account,
        project_id: source.project_id,
        checkout: source.checkout,
        agent,
        account: DEFAULT_ID.into(),
        return_focus: tx.focused().map(str::to_string),
        ..Default::default()
    };
    request_prepare(s, None);
}

/// Ask the worker for the review, for the pane's own conversation or a
/// typed ID. A later reply supersedes this one.
fn request_prepare(s: &mut State, manual_id: Option<String>) {
    let token = next_token(s);
    let h = &s.handoff;
    let Some(view) = s.session_by_key(&h.session) else {
        s.handoff.error = Some("The source pane is no longer available".into());
        s.handoff.phase = Phase::Review;
        return;
    };
    let session = protocol::session::SessionInfo {
        id: view.info.session_id,
        title: view.info.title.clone(),
        argv: view.info.argv.clone(),
        cwd: view.info.cwd.clone(),
        state: view.info.state.into(),
        pid: view.info.pid,
        exit_code: view.info.exit_code,
        error: view.info.error.clone(),
        started_at: view.info.started_at,
        ended_at: view.info.ended_at,
    };
    let req = Req::PrepareHandoff {
        token,
        session,
        agent: h.source_agent.clone(),
        project_id: h.project_id.clone(),
        checkout: h.checkout.clone(),
        manual_id,
    };
    s.handoff.phase = Phase::Preparing;
    s.handoff.error = None;
    if !s.editor.backend.send(Job::project(req)) {
        s.handoff.phase = Phase::Review;
        s.handoff.error = Some("The editor queue is full — try again".into());
    }
}

/// A review reply: only the dialog's current request counts.
pub fn apply_prepared(s: &mut State, token: u64, result: Result<(Prepared, Vec<String>), String>) {
    let h = &mut s.handoff;
    if !h.open || h.token != token || h.phase != Phase::Preparing {
        return;
    }
    h.phase = Phase::Review;
    match result {
        Ok((prepared, fork_accounts)) => {
            h.review = TextEdit::new(prepared.draft.clone());
            h.prepared = Some(prepared);
            h.fork_accounts = fork_accounts;
            h.fork = false;
            h.error = None;
        }
        Err(error) => h.error = Some(error),
    }
}

/// Installed agent presets, in registry order. Shell is not an agent.
fn destinations(s: &State) -> Vec<(String, String)> {
    s.data
        .agents
        .iter()
        .filter(|agent| agent.available && agent.id != "shell")
        .map(|agent| (agent.id.clone(), agent.label.clone()))
        .collect()
}

/// The accounts a destination launches with: Default, plus its registered
/// profiles where Goko supports named accounts (never on Windows).
fn accounts(s: &State, agent: &str) -> Vec<(String, String)> {
    let named = (!cfg!(windows))
        .then(|| s.data.accounts.as_ref()?.agent(agent))
        .flatten();
    match named {
        Some(view) => view
            .items
            .iter()
            .map(|item| (item.id.clone(), item.name.clone()))
            .collect(),
        None => vec![(DEFAULT_ID.into(), "Default".into())],
    }
}

/// A native fork is offered only between Claude ends whose destination
/// shares the verified source transcript's history.
fn fork_offered(h: &HandoffState) -> bool {
    h.source_agent == "claude"
        && h.agent == "claude"
        && h.prepared.as_ref().is_some_and(|p| p.fork.is_some())
}

fn fork_allowed(h: &HandoffState) -> bool {
    fork_offered(h) && h.fork_accounts.contains(&h.account)
}

fn select_agent(s: &mut State, agent: &str) {
    if s.handoff.phase != Phase::Review || s.handoff.agent == agent {
        return;
    }
    s.handoff.agent = agent.to_string();
    if !accounts(s, agent)
        .iter()
        .any(|(id, _)| *id == s.handoff.account)
    {
        s.handoff.account = DEFAULT_ID.into();
    }
    s.handoff.fork &= fork_allowed(&s.handoff);
}

fn select_account(s: &mut State, id: &str) {
    if s.handoff.phase != Phase::Review {
        return;
    }
    s.handoff.account = id.to_string();
    s.handoff.fork &= fork_allowed(&s.handoff);
}

fn select_fork(s: &mut State, fork: bool) {
    if s.handoff.phase == Phase::Review && (!fork || fork_allowed(&s.handoff)) {
        s.handoff.fork = fork;
    }
}

fn load_manual_id(s: &mut State) {
    if s.handoff.phase != Phase::Review {
        return;
    }
    let id = s.handoff.manual_id.text.trim().to_string();
    if id.is_empty() {
        s.handoff.error = Some("Type a conversation ID to load".into());
        return;
    }
    request_prepare(s, Some(id));
}

fn over_limit(h: &HandoffState) -> bool {
    !h.fork && h.review.text.len() > DRAFT_LIMIT
}

/// Open the destination: resolve its launch on the worker first. The text
/// to copy is fixed now; later edits cannot change what is copied.
fn confirm(s: &mut State) {
    let h = &s.handoff;
    if h.phase != Phase::Review || h.prepared.is_none() || h.agent.is_empty() {
        return;
    }
    if over_limit(h) {
        s.handoff.error = Some(format!(
            "The review is over {DRAFT_LIMIT} bytes; shorten it first"
        ));
        return;
    }
    if s.session_by_key(&h.session).is_none() {
        s.handoff.error = Some("The source pane is no longer available".into());
        return;
    }
    let fork = h
        .fork
        .then(|| h.prepared.as_ref().and_then(|p| p.fork.clone()))
        .flatten();
    let copy = (!h.fork && !h.review.text.trim().is_empty()).then(|| h.review.text.clone());
    let token = next_token(s);
    let h = &mut s.handoff;
    let req = Req::HandoffArgv {
        token,
        agent: h.agent.clone(),
        account: h.account.clone(),
        project_id: h.project_id.clone(),
        checkout: h.checkout.clone(),
        fork,
    };
    h.copy = copy;
    h.error = None;
    h.phase = Phase::Resolving;
    if !s.editor.backend.send(Job::project(req)) {
        s.handoff.phase = Phase::Review;
        s.handoff.copy = None;
        s.handoff.error = Some("The editor queue is full — try again".into());
    }
}

/// The destination's resolved launch: start it unless the request is
/// stale or the source pane went away meanwhile.
pub fn apply_argv(s: &mut State, token: u64, result: Result<(Vec<String>, String), String>) {
    let h = &s.handoff;
    if !h.open || h.token != token || h.phase != Phase::Resolving {
        return;
    }
    let failed = |s: &mut State, error: String| {
        s.handoff.phase = Phase::Review;
        s.handoff.copy = None;
        s.handoff.error = Some(error);
    };
    let (argv, cwd) = match result {
        Ok(launch) => launch,
        Err(error) => return failed(s, error),
    };
    if s.session_by_key(&h.session).is_none() {
        return failed(s, "The source pane is no longer available".into());
    }
    let launch = SessionLaunch {
        argv,
        cwd,
        project_id: Some(h.project_id.clone()),
        agent: h.agent.clone(),
        title: String::new(),
    };
    if s.launch_handoff(launch, token) {
        s.handoff.phase = Phase::Creating;
    } else {
        failed(s, "The Goko daemon is not reachable".into());
    }
}

/// The handoff's session exists: release the approved copy and close.
pub fn created(s: &mut State, token: u64) {
    let h = &mut s.handoff;
    if !h.open || h.token != token || h.phase != Phase::Creating {
        return;
    }
    let copy = h.copy.take();
    let label = app_core::agents::label(&h.agent).unwrap_or("the agent");
    s.notice = Some(if copy.is_some() {
        format!("Opened {label}. The context is on the clipboard — paste it when the CLI is ready.")
    } else {
        format!("Opened {label}.")
    });
    s.handoff_copy = copy;
    s.handoff = HandoffState::default();
}

/// The handoff's session did not start: keep the review, copy nothing.
pub fn create_failed(s: &mut State, token: u64, message: String) {
    let h = &mut s.handoff;
    if h.open && h.token == token && h.phase == Phase::Creating {
        h.phase = Phase::Review;
        h.copy = None;
        h.error = Some(message);
    } else {
        s.notice = Some(message);
    }
}

/// Dismiss, unless session creation is already dispatched.
pub fn close(s: &mut State) {
    if s.handoff.phase != Phase::Creating {
        s.handoff = HandoffState::default();
    }
}

pub fn key(s: &mut State, ev: &KeyEvent, _tx: &mut Tx) -> bool {
    if !s.handoff.open {
        return false;
    }
    if ev.mods.cmd && matches!(ev.key, Key::Command('w' | 'q')) {
        return false;
    }
    if ev.key == Key::Escape {
        close(s);
    }
    true
}

fn radio(label: &str, checked: bool, on: impl Fn(&mut State) + 'static) -> RadioItem {
    RadioItem::new(label, checked).on_select(on)
}

pub fn layers(s: &State) -> Vec<El> {
    if !s.handoff.open {
        return Vec::new();
    }
    let ink = &s.ink;
    let kit = &s.editor.kit;
    let h = &s.handoff;
    let editable = h.phase == Phase::Review;
    let source = format!(
        "From {} · {} · {}",
        app_core::agents::label(&h.source_agent).unwrap_or(&h.source_agent),
        h.source_account,
        h.checkout
    );
    let mut body = col()
        .key("handoff-body")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .gap(10.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    let status = match (&h.prepared, h.phase) {
        (_, Phase::Preparing) => "Reading the saved conversation…".to_string(),
        (Some(prepared), _) => prepared.status.clone(),
        (None, _) => "No review is available".to_string(),
    };
    body = body.child(theme::txt(&status, theme::T11, ink.muted).wrap_anywhere());
    if h.prepared
        .as_ref()
        .is_some_and(|p| p.conversation.is_none() && p.accepts_manual_id)
    {
        body = body.child(
            row()
                .w_full()
                .gap(8.0)
                .items_end()
                .child(
                    rune_kit::TextInput::new(&h.manual_id)
                        .label("Conversation ID")
                        .placeholder("Saved conversation ID in this account")
                        .width(None)
                        .disabled(!editable)
                        .on_edit(|s: &mut State| &mut s.handoff.manual_id)
                        .on_submit(|s: &mut State, _tx: &mut Tx| load_manual_id(s))
                        .build(kit)
                        .grow(1.0),
                )
                .child(
                    rune_kit::Button::new("Load")
                        .disabled(!editable)
                        .on_press(load_manual_id)
                        .build(kit),
                ),
        );
    }
    let mut agents = RadioGroup::new().label("Agent").disabled(!editable);
    let choices = destinations(s);
    if choices.is_empty() {
        body = body.child(theme::txt(
            "No agent preset is installed.",
            theme::T12,
            ink.danger,
        ));
    }
    for (id, label) in choices {
        let checked = id == h.agent;
        agents = agents.item(radio(&label, checked, move |s| select_agent(s, &id)));
    }
    let mut account_group = RadioGroup::new().label("Account").disabled(!editable);
    for (id, name) in accounts(s, &h.agent) {
        let checked = id == h.account;
        account_group = account_group.item(radio(&name, checked, move |s| select_account(s, &id)));
    }
    body = body.child(
        row()
            .w_full()
            .gap(24.0)
            .items_start()
            .child(agents.build(kit))
            .child(account_group.build(kit)),
    );
    if fork_offered(h) {
        let allowed = fork_allowed(h);
        let fork = RadioItem::new("Fork Claude conversation", h.fork)
            .description(if allowed {
                "Gives the destination Claude the full saved history, not the reviewed excerpt. Nothing is copied."
            } else {
                "This account does not share this Claude history, so it cannot fork it."
            })
            .disabled(!allowed)
            .on_select(|s: &mut State| select_fork(s, true));
        body = body.child(
            RadioGroup::new()
                .label("Mode")
                .disabled(!editable)
                .item(
                    RadioItem::new("New session with context", !h.fork)
                        .description("Opens a fresh session; you paste the reviewed text.")
                        .on_select(|s: &mut State| select_fork(s, false)),
                )
                .item(fork)
                .build(kit),
        );
    }
    if !h.fork {
        let bytes = h.review.text.len();
        let over = bytes > DRAFT_LIMIT;
        body = body
            .child(
                rune_kit::TextArea::new(&h.review)
                    .label("Context to copy")
                    .placeholder("Add the next step, or remove anything you do not want to share")
                    .width(None)
                    .rows(12)
                    .max_height(320.0)
                    .wrap_anywhere()
                    .invalid(over)
                    .disabled(!editable)
                    .on_edit(|s: &mut State| &mut s.handoff.review)
                    .on_submit(|s: &mut State, _tx: &mut Tx| confirm(s))
                    .build(kit),
            )
            .child(theme::txt(
                &format!(
                    "{bytes} / {DRAFT_LIMIT} bytes · copied only after the new session starts; you paste and send it yourself"
                ),
                theme::T11,
                if over { ink.danger } else { ink.muted },
            ));
    }
    if let Some(error) = &h.error {
        body = body.child(theme::txt(error, theme::T11, ink.danger).wrap_anywhere());
    }
    let busy = match h.phase {
        Phase::Preparing => Some("Reading…"),
        Phase::Resolving => Some("Checking the destination…"),
        Phase::Creating => Some("Starting the session…"),
        Phase::Review => None,
    };
    let primary = if h.fork {
        "Fork Conversation"
    } else if h.review.text.trim().is_empty() {
        "Open session"
    } else {
        "Open & Copy Context"
    };
    let can_open = editable && h.prepared.is_some() && !h.agent.is_empty() && !over_limit(h);
    let mut actions = row().w_full().items_center().justify_end().gap(8.0);
    if let Some(busy) = busy {
        actions = actions.child(theme::txt(busy, theme::T11, ink.muted).grow(1.0));
    }
    actions = actions
        .child(
            rune_kit::Button::new("Cancel")
                .disabled(h.phase == Phase::Creating)
                .on_press(close)
                .build(kit),
        )
        .child(
            rune_kit::Button::new(primary)
                .variant(ButtonVariant::Submit)
                .disabled(!can_open)
                .on_press(confirm)
                .build(kit),
        );
    let card = col()
        .key("handoff-dialog")
        .role("dialog")
        .label("Hand off to")
        .w_full()
        .max_w(680.0)
        .h_full()
        .max_h(720.0)
        .min_h(0.0)
        .fill(ink.panel)
        .border_sides([1.0; 4], ink.border)
        .radius(8.0)
        .padding(12.0)
        .gap(10.0)
        .child(theme::txt("Hand off to…", theme::T12M, ink.text))
        .child(
            theme::txt(&source, theme::T11, ink.muted)
                .min_w(0.0)
                .truncate_middle()
                .help_when_truncated(source.clone()),
        )
        .child(body)
        .child(actions);
    let mut card = card.modal(true, close);
    if let Some(key) = &h.return_focus {
        card = card.focus_return(key);
    }
    vec![
        el().absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .fill(ink.bg.alpha(0.55))
            .a11y_hidden(),
        col()
            .key("handoff-layer")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .padding(20.0)
            .items_center()
            .justify_center()
            .child(card),
    ]
}

#[cfg(test)]
mod tests;
