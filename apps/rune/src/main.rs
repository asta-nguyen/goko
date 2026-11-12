//! goko-rune: a Rune (wgpu/winit) port of the Goko desktop shell — header,
//! project sidebar, workspace tab strip, controls, a terminal stub and the
//! native editor workspace backed by `app-core` (real projects, Git sections,
//! drafts with Save/Discard/Cancel guards).

mod chrome;
mod daemon;
mod data;
mod editor;
mod panes;
mod picker;
mod project_map;
mod sessions;
mod settings;
mod sidebar;
mod sound;
mod tab_debug;
mod term;
mod terminal_branch;
mod theme;
mod trace;
mod update;

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use rune::{AppSpec, Cx, El, Tx, col, row};

use crate::data::Data;
use crate::editor::backend;
use crate::theme::Ink;

/// How long a notice toast stays open after its text was last set, seconds.
const NOTICE_LIFETIME: f64 = 8.0;

/// The workspace column's content mode: the terminal stub, the editor
/// workspace, or the app-level settings page that replaces the whole column
/// — TerminalWorkspace's view switch plus App.tsx's `settingsMode`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum View {
    Terminal,
    Editor,
    Settings,
}

pub struct State {
    terminal_branch: terminal_branch::BranchState,
    ink: Ink,
    data: Data,
    /// The child process's activity, keyed by session id: what the
    /// session's agent is doing (working, waiting, done).
    activity: HashMap<String, app_core::activity::Activity>,
    /// The agent CLIs running under each session's process tree.
    subagents: app_core::subagents::SubagentMap,
    debug: app_core::session_debug::Snapshot,
    debug_at: Option<f64>,
    tab_menu: Option<tab_debug::MenuTarget>,
    /// When the sub-agents were last read; the reader polls every two
    /// seconds.
    subagents_at: Option<f64>,
    /// When the activity was last read; the reader polls once a second.
    activity_at: Option<f64>,
    /// When session output was last polled from the daemon.
    output_polled_at: Option<f64>,
    /// In-flight `session.output` request.
    pending_output: Option<
        std::sync::mpsc::Receiver<Result<daemon::SessionOutputList, app_core::error::BridgeError>>,
    >,
    /// Session key → (output_seq, last_output_at_ms, recorded_at_ms).
    session_output: HashMap<String, (u64, Option<i64>, i64)>,
    /// Project id → expanded; absent means expanded (ProjectTree default).
    expanded: HashMap<String, bool>,
    selected_session: Option<String>,
    active_workspace: Option<String>,
    /// Open workspace tabs: each owns a recursive pane tree, so a split
    /// tab's id is never "the tab's session".
    open_tabs: Vec<panes::Tab>,
    active_tab: usize,
    /// Project id → last focused tab id; hidden tabs keep their pane layouts.
    project_tabs: HashMap<String, String>,
    /// "Worktrees" group header open per project id; absent means open.
    worktree_groups: HashMap<String, bool>,
    /// Per-worktree session list open, keyed by worktree path; absent = open.
    worktrees_open: HashMap<String, bool>,
    /// Workspace column mode (terminal / editor / settings).
    view: View,
    /// The native editor workspace: projects/checkouts, sections, docs,
    /// backend worker and ACP runtime.
    editor: editor::state::EditorState,
    /// The daemon client: every session read and write goes through it.
    daemon: daemon::Daemon,
    /// The stored session metadata rows, keyed by session id.
    session_metas: HashMap<String, app_core::store::SessionMeta>,
    /// Daemon requests whose replies are drained in `poll`.
    pending: Vec<sessions::Pending>,
    /// The daemon version the last handshake reported.
    daemon_version: Option<String>,
    /// Why the daemon is unreachable, when it is.
    daemon_reason: Option<String>,
    /// The sessions whose pane terminals fell behind (the next poll
    /// resubscribes each).
    terminal_lagged: HashSet<protocol::session::SessionId>,
    /// Every visible pane's terminal, keyed by session id: grid, output
    /// stream and PTY size — the active tab's panes only.
    attachments: HashMap<String, panes::Attachment>,
    /// The last measured grid (cols, rows) of each session's pane. A pane's
    /// layout fires only when its rect changes, so an attachment made after
    /// that (tab switch, new session) starts from this, not the default.
    pane_grids: HashMap<String, (usize, usize)>,
    /// The pane (session key) a file drag is over: it draws the drop ring.
    drop_pane: Option<String>,
    /// What the input method is composing (Telex marked text): drawn at
    /// the terminal's cursor, not yet typed into the session.
    preedit: String,
    /// The measured size of every split's box along its axis, read by the
    /// separator's drag and keyboard bounds.
    split_sizes: HashMap<String, f32>,
    /// Sessions whose close has a daemon request in flight — a pane never
    /// issues its stop or delete twice.
    close_inflight: HashSet<String>,
    /// Repository roots a session's `cd` asked to register, until the
    /// `AddProject` reply lands — one request per root, and no "already
    /// registered" notice for a registration the user never asked for.
    auto_project_roots: HashSet<String>,
    /// The wall clock in epoch milliseconds — the elapsed counter's source.
    now_ms: i64,
    /// The app's clock in seconds — the pollers' pacing.
    clock: f64,
    /// A notice from the last failed daemon call.
    notice: Option<String>,
    /// When the notice's text last changed, on the app's clock.
    notice_set_at: Option<f64>,
    /// The notice text that `notice_set_at` timed.
    notice_seen: Option<String>,
    /// The sidebar's width in pixels.
    sidebar_width: f32,
    /// The open sidebar menu: the row it belongs to and what it offers.
    sidebar_menu: Option<(sidebar::RowTarget, sidebar::MenuKind)>,
    /// The ⌥↑/⌥↓ cursor: the element key of the sidebar row the keyboard is
    /// on — `sidebar::rows` rebuilds the list each press and re-anchors by it.
    pub sidebar_cursor: Option<String>,
    /// The last terminal link opened and when — a double-click's repeat is
    /// swallowed so one gesture opens once.
    last_link_open: Option<(String, std::time::Instant)>,
    /// The sidebar's workspace selector (rune-kit's searchable select).
    pub workspace_picker: rune_kit::ComboboxState,
    /// The one new-session picker (shell, agent or command to start) and
    /// where it is open — the PROJECTS `+`, a row's `+`, or New tab's caret.
    pub new_session_picker: rune_kit::ComboboxState,
    pub new_session_from: Option<sidebar::NewSessionFrom>,
    /// The text the sidebar asks the user for.
    sidebar_input: Option<sidebar::SidebarInput>,
    /// The text the user typed into the sidebar's prompt.
    sidebar_text: rune::edit::TextEdit,
    /// The named workspaces, Default first — the sidebar's selector.
    workspaces: Vec<app_core::store::Workspace>,
    workspaces_loading: bool,
    workspaces_error: Option<String>,
    /// The workspace whose projects the sidebar shows — distinct from the
    /// checkout `active_workspace` highlight.
    selected_workspace: Option<String>,
    project_map: project_map::ProjectMapState,
    /// The saved selection read from startup is still pending; a user choice
    /// or its reply clears it.
    workspace_restore_pending: bool,
    /// The workspace removal awaiting confirmation.
    workspace_remove_confirm: Option<String>,
    /// The worktree or project removal awaiting confirmation.
    sidebar_confirm: Option<sidebar::SidebarConfirm>,
    /// Monotonic token correlating a workspace-name prompt with its reply.
    workspace_token: u64,
    /// The workspace-name submission in flight, by token.
    workspace_pending: Option<u64>,
    /// The launches waiting for their agent's argv, each with its own
    /// token so concurrent requests never steal each other's answer.
    pending_argv: Vec<sessions::PendingArgv>,
    /// The next launch token — monotonic, so a reply always names exactly
    /// one request.
    argv_token: u64,
    /// When each session entered the stopping state, so the force kill can
    /// be offered once a stop has hung.
    stopping_since: HashMap<String, i64>,
    /// Stops the app already escalated to a force kill on its own.
    auto_forced: HashSet<String>,
    /// When the usage/accounts read was last issued; the worker re-reads
    /// the sources about every five seconds.
    usage_at: Option<f64>,
    /// The open usage-chip account menu: the agent it belongs to; the kit
    /// menu anchors to that chip.
    usage_menu: Option<String>,
    /// What the check for a newer release found (Settings → Updates).
    update: update::Status,
    /// The settings page's own state (section, forms, drafts).
    settings: settings::SettingsState,
    /// The appearance choice restored from `goko.theme`.
    theme_choice: theme::Theme,
    /// What the OS last reported: `true` when dark. Only consulted while
    /// `theme_choice` is `System`.
    os_dark: bool,
    /// Scroll speed multiplier for terminal and editor/diff scrolling.
    pub scroll_speed: f32,
    /// Whether the OS reports the window focused; done sounds stay silent
    /// while it is.
    window_focused: bool,
    /// The `goko.sound_done` setting: a sound when a session finishes.
    sound_done: bool,
    /// When the done sound last played, in epoch ms — the debounce.
    sound_last_ms: i64,
}

/// `AppSpec::init` is a plain `fn() -> S`, so the `AppHandle` the editor's
/// backend/ACP threads need is parked here between `App::new` and `run`
/// (init runs inside `run`, after the handle exists).
static APP_HANDLE: OnceLock<rune::AppHandle<State>> = OnceLock::new();

fn init_state() -> State {
    let handle = APP_HANDLE
        .get()
        .cloned()
        .expect("app handle registered before run");
    State::new(handle)
}

impl State {
    fn new(handle: rune::AppHandle<State>) -> State {
        let ink = Ink::new();
        let kit = rune_kit::Kit::neutral(true);
        // Workers own their storage connections; nothing here touches
        // GOKO_HOME on the UI thread.
        let editor = editor::state::EditorState::new(kit, handle.clone());
        // The daemon client runs on its own runtime thread; it wakes the
        // event loop after every event so `poll` drains it promptly.
        #[cfg(not(test))]
        let daemon = daemon::Daemon::spawn({
            let handle = handle.clone();
            move || handle.wake()
        });
        // Tests never touch a real daemon: the client stays disconnected
        // and the session state is driven by synthetic events.
        #[cfg(test)]
        let daemon = daemon::Daemon::spawn_disconnected();
        #[cfg(not(test))]
        update::spawn(handle.clone());
        State {
            terminal_branch: Default::default(),
            ink,
            data: Data::default(),
            expanded: HashMap::new(),
            selected_session: None,
            // No workspace is committed until the user picks a real
            // registered project/checkout in the sidebar (or the editor
            // pickers) — no session may seed editor identity.
            active_workspace: None,
            open_tabs: Vec::new(),
            active_tab: 0,
            project_tabs: HashMap::new(),
            worktree_groups: HashMap::new(),
            worktrees_open: HashMap::new(),
            view: View::Terminal,
            editor,
            daemon,
            session_metas: HashMap::new(),
            pending: Vec::new(),
            daemon_version: None,
            daemon_reason: None,
            terminal_lagged: HashSet::new(),
            attachments: HashMap::new(),
            pane_grids: HashMap::new(),
            drop_pane: None,
            preedit: String::new(),
            split_sizes: HashMap::new(),
            close_inflight: HashSet::new(),
            auto_project_roots: HashSet::new(),
            now_ms: data::now_ms(),
            clock: 0.0,
            notice: None,
            notice_set_at: None,
            notice_seen: None,
            sidebar_width: sidebar::DEFAULT_SIDEBAR_W,
            sidebar_menu: None,
            sidebar_cursor: None,
            last_link_open: None,
            workspace_picker: rune_kit::ComboboxState::default(),
            new_session_picker: rune_kit::ComboboxState::default(),
            new_session_from: None,
            sidebar_input: None,
            sidebar_text: rune::edit::TextEdit::new(""),
            workspaces: Vec::new(),
            workspaces_loading: false,
            workspaces_error: None,
            selected_workspace: None,
            project_map: Default::default(),
            workspace_restore_pending: true,
            workspace_remove_confirm: None,
            sidebar_confirm: None,
            workspace_token: 0,
            workspace_pending: None,
            pending_argv: Vec::new(),
            argv_token: 0,
            stopping_since: HashMap::new(),
            auto_forced: HashSet::new(),
            activity: HashMap::new(),
            activity_at: None,
            output_polled_at: None,
            pending_output: None,
            session_output: HashMap::new(),
            subagents: Default::default(),
            debug: Default::default(),
            debug_at: None,
            tab_menu: None,
            subagents_at: None,
            usage_at: None,
            usage_menu: None,
            update: update::Status::default(),
            settings: settings::SettingsState::default(),
            theme_choice: theme::Theme::Dark,
            os_dark: true,
            scroll_speed: 1.0,
            window_focused: true,
            sound_done: true,
            sound_last_ms: 0,
        }
    }

    /// Rebuild the palette (and the kit's) for the current theme choice:
    /// `System` resolves through the OS's last-reported appearance.
    fn apply_theme(&mut self) {
        let dark = self.theme_choice.resolve(self.os_dark).dark();
        self.ink = Ink::for_theme(self.theme_choice.resolve(self.os_dark));
        self.editor.kit = rune_kit::Kit::neutral(dark);
        let ink = self.ink;
        // Every pane's terminal holds rows rendered in the old palette:
        // repaint them in the new one — parser, stream and PTY untouched.
        for (key, att) in self.attachments.iter_mut() {
            att.term.refresh_theme();
            let lines = att.term.lines(&ink).to_vec();
            att.set_lines(&lines, key);
        }
    }

    /// Save the sidebar's width, so it survives the next launch. The write
    /// rides the worker lane like every other settings change.
    pub fn persist_sidebar_width(&mut self) {
        let width = self.sidebar_width.round();
        let _ = backend::enqueue(
            &mut self.editor.backend,
            backend::Job::project(backend::Req::SetSetting {
                key: sidebar::SIDEBAR_WIDTH_KEY.to_string(),
                value: format!("{width}"),
            }),
        );
    }

    /// Seconds until the notice toast closes; `None` with no toast. The poll
    /// asks for a wake at this time, so an idle app closes it on schedule.
    pub fn notice_remaining(&self, now: f64) -> Option<f64> {
        self.notice.as_ref()?;
        let at = self.notice_set_at?;
        Some(NOTICE_LIFETIME - (now - at))
    }

    /// Close the notice toast 8 s after its text last changed, so a newer
    /// notice restarts the timer. Returns `true` when it closed the toast.
    // ponytail: a repeat of the same text keeps the first timer; stamp the time in each setter if that shows.
    pub fn expire_notice(&mut self, now: f64) -> bool {
        if self.notice.is_none() {
            self.notice_set_at = None;
            self.notice_seen = None;
            return false;
        }
        if self.notice != self.notice_seen {
            self.notice_seen = self.notice.clone();
            self.notice_set_at = Some(now);
        }
        if self
            .notice_set_at
            .is_some_and(|at| now - at >= NOTICE_LIFETIME)
        {
            self.notice = None;
            self.notice_set_at = None;
            self.notice_seen = None;
            return true;
        }
        false
    }

    /// The selected workspace id, Default until a workspace list or a user
    /// choice names another.
    pub fn workspace_id(&self) -> &str {
        self.selected_workspace
            .as_deref()
            .unwrap_or(app_core::store::DEFAULT_WORKSPACE_ID)
    }

    /// The selected workspace's label, for the selector and prompts.
    pub fn workspace_name(&self) -> String {
        let id = self.workspace_id();
        self.workspaces
            .iter()
            .find(|workspace| workspace.id == id)
            .map_or_else(|| "Default".to_string(), |w| self.workspace_label(w))
    }

    /// What a workspace is called on screen. A named one shows its name. An
    /// unnamed one (created without a prompt, as herdr creates a space)
    /// follows its first project, the way herdr's unnamed space follows its
    /// terminal's folder; with no project yet it reads "New workspace",
    /// numbered when several are still empty.
    pub fn workspace_label(&self, workspace: &app_core::store::Workspace) -> String {
        if !workspace.name.trim().is_empty() {
            return workspace.name.clone();
        }
        let first_project = |id: &str| {
            self.editor
                .projects
                .iter()
                .find(|p| p.project.workspace_id == id)
        };
        if let Some(project) = first_project(&workspace.id) {
            return project.project.name.clone();
        }
        let earlier = self
            .workspaces
            .iter()
            .take_while(|w| w.id != workspace.id)
            .filter(|w| w.name.trim().is_empty() && first_project(&w.id).is_none())
            .count();
        match earlier {
            0 => "New workspace".to_string(),
            n => format!("New workspace {}", n + 1),
        }
    }

    /// The label of the workspace with `id`, or `fallback` when it is unknown.
    pub fn workspace_label_of(&self, id: &str, fallback: &str) -> String {
        self.workspaces
            .iter()
            .find(|w| w.id == id)
            .map_or_else(|| fallback.to_string(), |w| self.workspace_label(w))
    }

    /// Sessions waiting for the user, per workspace id. A session counts for
    /// its project's workspace; Unassigned sessions count for Default. Uses
    /// the dot's own activity so the flag and the row never disagree.
    pub fn workspace_waiting_counts(&self) -> HashMap<String, usize> {
        let known: HashSet<&str> = self
            .editor
            .projects
            .iter()
            .map(|p| p.project.id.as_str())
            .collect();
        let (by_project, unassigned) = data::partition(&self.data, &known);
        let waiting = |sv: &data::SessionView| {
            self.session_shown_activity(&sv.info.id) == Some(app_core::activity::Activity::Waiting)
        };
        let mut counts: HashMap<String, usize> = HashMap::new();
        for project in &self.editor.projects {
            let n = by_project
                .get(project.project.id.as_str())
                .map_or(0, |list| list.iter().filter(|sv| waiting(sv)).count());
            if n > 0 {
                *counts
                    .entry(project.project.workspace_id.clone())
                    .or_default() += n;
            }
        }
        // A session outside every project counts for the workspace it started in.
        for sv in unassigned.iter().filter(|sv| waiting(sv)) {
            *counts
                .entry(self.loose_workspace(sv).to_string())
                .or_default() += 1;
        }
        counts
    }

    /// Read every workspace on the worker lane.
    pub fn request_workspaces(&mut self) {
        if backend::enqueue(
            &mut self.editor.backend,
            backend::Job::project(backend::Req::Workspaces),
        ) {
            self.workspaces_loading = true;
            self.workspaces_error = None;
        } else {
            self.workspaces_loading = false;
            self.workspaces_error =
                Some("the editor queue is full; workspaces were not requested".into());
        }
    }

    /// Select a workspace and persist the choice. Switching never touches the
    /// editor checkout, open tabs, or running sessions.
    pub fn select_workspace(&mut self, id: &str) {
        if self.selected_workspace.as_deref() == Some(id) {
            self.workspace_restore_pending = false;
            self.workspace_remove_confirm = None;
            self.sidebar_confirm = None;
            return;
        }
        // The selection always applies (a reply handler relies on it after a
        // workspace is removed); a refused write only means the choice is not
        // remembered next launch, which the notice says.
        if !backend::enqueue(
            &mut self.editor.backend,
            backend::Job::project(backend::Req::SetSetting {
                key: sidebar::SELECTED_WORKSPACE_KEY.to_string(),
                value: id.to_string(),
            }),
        ) {
            self.notice =
                Some("the editor queue is full; the workspace choice was not saved".into());
        }
        self.workspace_restore_pending = false;
        self.workspace_remove_confirm = None;
        self.sidebar_confirm = None;
        self.selected_workspace = Some(id.to_string());
    }

    /// Ask the worker for a new workspace; the reply selects it.
    pub fn submit_workspace_create(&mut self, name: &str) {
        // One workspace write at a time: a double Enter sends one request.
        if self.workspace_pending.is_some() {
            return;
        }
        self.workspace_token = self.workspace_token.wrapping_add(1);
        let token = self.workspace_token;
        if backend::enqueue(
            &mut self.editor.backend,
            backend::Job::project(backend::Req::CreateWorkspace {
                name: name.trim().to_string(),
                token,
            }),
        ) {
            self.workspace_pending = Some(token);
        } else {
            self.notice = Some("the editor queue is full; the workspace was not created".into());
        }
    }

    /// Rename a named workspace; the reply refreshes the list.
    pub fn submit_workspace_rename(&mut self, id: &str, name: &str) {
        // One workspace write at a time: a double Enter sends one request.
        if self.workspace_pending.is_some() {
            return;
        }
        self.workspace_token = self.workspace_token.wrapping_add(1);
        let token = self.workspace_token;
        if backend::enqueue(
            &mut self.editor.backend,
            backend::Job::project(backend::Req::RenameWorkspace {
                id: id.to_string(),
                name: name.trim().to_string(),
                token,
            }),
        ) {
            self.workspace_pending = Some(token);
        } else {
            self.notice = Some("the editor queue is full; the workspace was not renamed".into());
        }
    }

    /// Remove a workspace after its confirmation: its projects move to
    /// Default in one storage transaction.
    pub fn remove_workspace(&mut self, id: &str) {
        if !backend::enqueue(
            &mut self.editor.backend,
            backend::Job::project(backend::Req::RemoveWorkspace { id: id.to_string() }),
        ) {
            self.notice = Some("the editor queue is full; try again".into());
            return;
        }
        self.workspace_remove_confirm = None;
    }

    /// Move one project to another workspace; the refreshed project list
    /// places its row after the write succeeds.
    pub fn move_project(&mut self, project_id: &str, workspace_id: &str) {
        if !backend::enqueue(
            &mut self.editor.backend,
            backend::Job::project(backend::Req::MoveProject {
                project_id: project_id.to_string(),
                workspace_id: workspace_id.to_string(),
            }),
        ) {
            self.notice = Some("the editor queue is full; try again".into());
        }
    }

    /// Open the rename prompt with the workspace's current name filled in.
    pub fn ask_for_workspace_rename(&mut self, id: &str, current: &str) {
        self.ask_for_input(sidebar::SidebarInput::WorkspaceRename { id: id.to_string() });
        self.sidebar_text = rune::edit::TextEdit::new(current);
        self.sidebar_text.focused = true;
    }

    /// Close the sidebar's text prompt and drop its draft.
    pub fn close_sidebar_input(&mut self) {
        self.sidebar_input = None;
        self.sidebar_text = rune::edit::TextEdit::new("");
    }

    /// Whether one of the sidebar's searchable selects is open (it owns the
    /// keyboard while its search field takes the typing).
    pub fn sidebar_picker_open(&self) -> bool {
        self.workspace_picker.open || self.new_session_picker.open
    }

    /// Whether the open prompt edits a workspace name.
    pub fn workspace_input_open(&self) -> bool {
        matches!(
            self.sidebar_input,
            Some(sidebar::SidebarInput::WorkspaceRename { .. })
        )
    }

    /// Apply one backend reply: the app-level kinds land here, the editor's
    /// own kinds in the editor state.
    pub fn apply_reply(&mut self, reply: backend::Reply) {
        match &reply.req {
            backend::Req::SessionMetaList => {
                if let Ok(backend::ReplyData::SessionMetas(rows)) = &reply.result {
                    let metas: HashMap<String, app_core::store::SessionMeta> = rows
                        .iter()
                        .map(|meta| (meta.session_id.clone(), meta.clone()))
                        .collect();
                    self.session_metas = metas.clone();
                    // The rows join the metadata now that it is known.
                    for view in self.data.sessions.iter_mut() {
                        view.meta = metas.get(&view.info.id).cloned();
                    }
                    self.sync_selection();
                }
                return;
            }
            backend::Req::Subagents { .. } => {
                if let Ok(backend::ReplyData::Subagents { agents, debug }) = &reply.result {
                    self.subagents = agents.clone();
                    self.debug = debug.clone();
                    self.debug_at = Some(self.clock);
                }
                return;
            }
            backend::Req::Activity { .. } => {
                if let Ok(backend::ReplyData::Activity(activity)) = &reply.result {
                    // A session whose activity newly reads done chimes once —
                    // the reply carries every session, so only a real flip
                    // counts (the debounce collapses simultaneous ones). A
                    // session seen for the first time is not a flip: the first
                    // reply after launch would chime for work done long ago.
                    if activity.iter().any(|(id, now)| {
                        *now == app_core::activity::Activity::Done
                            && self.activity.get(id).is_some_and(|was| was != now)
                    }) {
                        self.note_done_sound();
                    }
                    self.activity = activity.clone();
                }
                return;
            }
            backend::Req::Agents => {
                if let Ok(backend::ReplyData::Agents(agents)) = &reply.result {
                    self.data.agents = agents.clone();
                }
                return;
            }
            backend::Req::AgentArgv { token, .. } => {
                // The agent's launch argv: the session starts in the row's
                // checkout. The reply's own token names the request it
                // belongs to — two launches can be in flight at once, and
                // neither may take the other's answer.
                match &reply.result {
                    Ok(backend::ReplyData::AgentArgv(argv)) => {
                        if let Some(launch) = self.resolved_launch(*token, argv.clone()) {
                            self.launch_session(launch);
                        }
                    }
                    Err(error) => {
                        // A refused launch must say why: a removed account
                        // folder, a binary that disappeared, an invalid
                        // configuration. Only this request's context is
                        // dropped — another launch keeps its own.
                        self.take_pending_argv(*token);
                        self.notice = Some(error.clone());
                    }
                    _ => {}
                }
                return;
            }
            backend::Req::Usage { .. } => {
                if let Ok(backend::ReplyData::Usage { .. }) = &reply.result {
                    // Move the payload out — the view types aren't Clone.
                    if let Ok(backend::ReplyData::Usage { snapshot, accounts }) = reply.result {
                        self.data.usage = Some(snapshot);
                        self.data.accounts = Some(accounts);
                    }
                }
                return;
            }
            backend::Req::AccountAdd { agent, token, .. } => {
                match reply.result {
                    Ok(backend::ReplyData::Accounts(view)) => {
                        // Main's add flow opens sign-in for the new account —
                        // `add` leaves it active, so `active` names it.
                        let id = match agent.as_str() {
                            "claude" => view.claude.active.clone(),
                            _ => view.codex.active.clone(),
                        };
                        self.data.accounts = Some(view);
                        // The drafts are cleared by the reply that belongs
                        // to this submission only: a stale one must never
                        // wipe a newer form.
                        if self.settings.finish_account_add(*token) {
                            self.request_sign_in(agent, &id);
                        }
                    }
                    Err(error) => {
                        // The failure keeps the drafts: the user can fix
                        // the name or the folder and try again.
                        let matched = self.settings.fail_account_add(*token, error.clone());
                        if matched {
                            self.notice = Some(error.clone());
                        }
                    }
                    _ => {}
                }
                return;
            }
            backend::Req::AccountRename { token, .. } => {
                match reply.result {
                    Ok(backend::ReplyData::Accounts(view)) => {
                        self.data.accounts = Some(view);
                        // Usage chips show the new account name immediately.
                        self.request_usage();
                        self.settings.finish_account_rename(*token);
                    }
                    Err(error) => {
                        let matched = self.settings.fail_account_rename(*token, error.clone());
                        if matched {
                            self.notice = Some(error.clone());
                        }
                    }
                    _ => {}
                }
                return;
            }
            backend::Req::AccountRemove { .. } | backend::Req::AccountSetActive { .. } => {
                if let Ok(backend::ReplyData::Accounts(view)) = reply.result {
                    self.data.accounts = Some(view);
                    // Usage chips show the new active name immediately.
                    self.request_usage();
                } else if let Err(error) = &reply.result {
                    self.notice = Some(error.clone());
                }
                return;
            }
            backend::Req::AccountSignIn { .. } => {
                if let Ok(backend::ReplyData::SignIn { argv, cwd, title }) = reply.result {
                    self.launch_session(sessions::SessionLaunch {
                        argv,
                        cwd,
                        project_id: None,
                        agent: "custom".to_string(),
                        title,
                    });
                } else if let Err(error) = &reply.result {
                    self.notice = Some(error.clone());
                }
                return;
            }
            backend::Req::GetSetting { key } if key == theme::THEME_KEY => {
                if let Ok(backend::ReplyData::Setting(Some(raw))) = &reply.result {
                    self.theme_choice = theme::Theme::parse(raw);
                    self.apply_theme();
                }
                return;
            }
            backend::Req::GetSetting { key } if key == settings::SCROLL_SPEED_KEY => {
                if let Ok(backend::ReplyData::Setting(Some(raw))) = &reply.result
                    && let Ok(val) = raw.parse::<f32>()
                    && val > 0.0
                    && val <= 10.0
                {
                    self.scroll_speed = val;
                }
                return;
            }
            backend::Req::GetSetting { key } if key == settings::SOUND_DONE_KEY => {
                // Absent stays at the default; only an explicit "0" mutes.
                if let Ok(backend::ReplyData::Setting(Some(raw))) = &reply.result {
                    self.sound_done = raw != "0";
                }
                return;
            }
            backend::Req::Workspaces => {
                match &reply.result {
                    Ok(backend::ReplyData::Workspaces(list)) => {
                        self.workspaces = list.clone();
                        self.workspaces_loading = false;
                        self.workspaces_error = None;
                        // A selection that vanished (or never existed) falls
                        // back to the permanent Default workspace.
                        if self
                            .selected_workspace
                            .as_ref()
                            .is_none_or(|id| !list.iter().any(|workspace| &workspace.id == id))
                        {
                            self.selected_workspace =
                                Some(app_core::store::DEFAULT_WORKSPACE_ID.to_string());
                        }
                    }
                    Err(error) => {
                        self.workspaces_loading = false;
                        self.workspaces_error = Some(error.clone());
                    }
                    _ => {}
                }
                return;
            }
            backend::Req::GetSetting { key } if key == sidebar::SELECTED_WORKSPACE_KEY => {
                // Restore the saved workspace only while nothing newer has
                // been chosen; a deleted or superseded id falls back to Default.
                if self.workspace_restore_pending {
                    self.workspace_restore_pending = false;
                    if let Ok(backend::ReplyData::Setting(Some(saved))) = &reply.result
                        && self
                            .workspaces
                            .iter()
                            .any(|workspace| &workspace.id == saved)
                    {
                        self.selected_workspace = Some(saved.clone());
                    }
                }
                return;
            }
            backend::Req::CreateWorkspace { token, .. } => {
                match &reply.result {
                    Ok(backend::ReplyData::WorkspaceCreated(workspace)) => {
                        if !self.workspaces.iter().any(|w| w.id == workspace.id) {
                            self.workspaces.push(workspace.clone());
                        }
                        // New workspace has no prompt: the created one is
                        // selected as soon as its own request lands.
                        if self.workspace_pending == Some(*token) {
                            self.workspace_pending = None;
                            self.select_workspace(&workspace.id);
                        }
                    }
                    Err(error) => {
                        if self.workspace_pending == Some(*token) {
                            self.workspace_pending = None;
                        }
                        // The prompt and its text stay for correction.
                        self.notice = Some(error.clone());
                    }
                    _ => {}
                }
                return;
            }
            backend::Req::RenameWorkspace { token, .. } => {
                match &reply.result {
                    Ok(backend::ReplyData::Workspaces(list)) => {
                        self.workspaces = list.clone();
                        if self.workspace_pending == Some(*token) {
                            self.workspace_pending = None;
                            if self.workspace_input_open() {
                                self.close_sidebar_input();
                            }
                        }
                    }
                    Err(error) => {
                        if self.workspace_pending == Some(*token) {
                            self.workspace_pending = None;
                        }
                        self.notice = Some(error.clone());
                    }
                    _ => {}
                }
                return;
            }
            backend::Req::RemoveWorkspace { id } => {
                match &reply.result {
                    Ok(backend::ReplyData::Workspaces(list)) => {
                        // Removing a workspace ends what runs in it: its
                        // projects' sessions and its own terminals close
                        // (stop, then delete). Found before the list changes,
                        // while its terminals still name it.
                        let closing: Vec<String> = self
                            .data
                            .sessions
                            .iter()
                            .filter(|view| self.session_in_workspace(view, id))
                            .map(|view| view.info.id.clone())
                            .collect();
                        for key in &closing {
                            self.close_session(key);
                        }
                        self.workspaces = list.clone();
                        if self.selected_workspace.as_deref() == Some(id.as_str()) {
                            self.select_workspace(app_core::store::DEFAULT_WORKSPACE_ID);
                        }
                        // The transferred projects moved to Default.
                        self.editor.refresh_projects();
                    }
                    Err(error) => self.notice = Some(error.clone()),
                    _ => {}
                }
                return;
            }
            backend::Req::MoveProject { .. } => {
                match &reply.result {
                    Ok(_) => self.editor.refresh_projects(),
                    Err(error) => self.notice = Some(error.clone()),
                }
                return;
            }
            backend::Req::CreateWorktree { project_id, .. } => {
                // The new checkout joins the project's list.
                if reply.result.is_ok() {
                    self.editor.request_worktrees(project_id);
                }
                return;
            }
            backend::Req::RemoveWorktree { project_id, .. } => {
                // The list is stale without the removed checkout; a failed
                // removal leaves it as it was and says why.
                match &reply.result {
                    Ok(_) => {
                        self.editor.request_worktrees(project_id);
                    }
                    Err(error) => self.notice = Some(error.clone()),
                }
                return;
            }
            backend::Req::PruneWorktrees { project_id } => {
                // A failed prune leaves the list as it was and says why.
                match &reply.result {
                    Ok(_) => {
                        self.editor.request_worktrees(project_id);
                    }
                    Err(error) => self.notice = Some(error.clone()),
                }
                return;
            }
            backend::Req::AddProject { path, .. } => {
                // A cd-discovered repository is registered silently; only a
                // user's own Add Project hears that it was already there.
                let auto = self.auto_project_roots.remove(path);
                // Re-adding a registered folder keeps its membership: say
                // which workspace already holds it instead of moving it.
                if let Ok(backend::ReplyData::Added(project)) = &reply.result {
                    if !auto
                        && self
                            .editor
                            .projects
                            .iter()
                            .any(|p| p.project.id == project.project.id)
                    {
                        let name =
                            self.workspace_label_of(&project.project.workspace_id, "Default");
                        self.notice = Some(format!(
                            "{} is already registered in {name}",
                            project.project.name
                        ));
                    }
                    // Sessions with no project that sit in the new folder join
                    // it. One that already belongs to a project stays put, so
                    // its tab is not pulled out from under the project filter.
                    let root = std::path::Path::new(&project.project.root);
                    for s in &mut self.data.sessions {
                        let cwd = std::path::Path::new(&s.info.cwd);
                        if cwd.starts_with(root)
                            && let Some(meta) = self.session_metas.get_mut(&s.info.id)
                            && meta.project_id.is_none()
                        {
                            meta.project_id = Some(project.project.id.clone());
                            s.meta = Some(meta.clone());
                            backend::enqueue(
                                &mut self.editor.backend,
                                backend::Job::project(backend::Req::InsertSessionMeta(
                                    meta.clone(),
                                )),
                            );
                        }
                    }
                }
                self.editor.apply_reply(reply);
                return;
            }
            backend::Req::RemoveProject { id } => {
                if let Err(error) = &reply.result {
                    self.notice = Some(error.clone());
                }
                if reply.result.is_ok() {
                    self.editor.refresh_projects();
                    // The sessions of a removed project lose their project:
                    // they render under Unassigned from now on.
                    for view in self.data.sessions.iter_mut() {
                        if let Some(meta) = view.meta.as_mut()
                            && meta.project_id.as_deref() == Some(id.as_str())
                        {
                            meta.project_id = None;
                        }
                    }
                    for meta in self.session_metas.values_mut() {
                        if meta.project_id.as_deref() == Some(id.as_str()) {
                            meta.project_id = None;
                        }
                    }
                    self.sync_selection();
                }
                return;
            }
            backend::Req::GetSetting { key } if key == sidebar::SIDEBAR_WIDTH_KEY => {
                if let Ok(backend::ReplyData::Setting(Some(raw))) = &reply.result
                    && let Ok(width) = raw.parse::<f32>()
                {
                    self.sidebar_width = sidebar::clamp_width(width);
                }
                return;
            }
            _ => {}
        }
        self.editor.apply_reply(reply);
    }
}

/// A handle for state-level tests: `App::new` builds the shared request
/// queue without opening a window, so `update`/`quit` calls from worker
/// replies just queue harmlessly.
#[cfg(test)]
pub(crate) fn test_handle() -> rune::AppHandle<State> {
    fn init() -> State {
        unreachable!("init does not run in tests")
    }
    fn view(_: &State, _: &mut Cx<State>) -> El {
        unreachable!("view does not run in tests")
    }
    rune::App::new(AppSpec::new("test", (10.0, 10.0), init, view)).handle()
}

/// Root: header full-width, then [sidebar 320 | 8 px separator | workspace
/// column]. Settings replaces the whole column like App.tsx's settingsMode;
/// Editor/Terminal keep the tab strip + controls and swap only the content.
fn view(s: &State, cx: &mut Cx<State>) -> El {
    let ink = &s.ink;
    let workspace = if s.view == View::Settings {
        col()
            .grow(1.0)
            .min_w(0.0)
            .h_full()
            .min_h(0.0)
            .child(settings::view(s, ink))
    } else {
        let content = if s.view == View::Editor {
            editor::workspace::workspace(s, ink, &s.editor.kit)
        } else {
            panes::terminal_workspace(s, ink)
        };
        // SessionControls live inside TerminalWorkspace only — the editor
        // has no session pane, so the strip (and its mock Stop/Close) stays
        // terminal-only like App.tsx.
        let mut stack = col()
            .grow(1.0)
            .min_w(0.0)
            .h_full()
            .min_h(0.0)
            .child(chrome::tab_bar(s, ink));
        if s.view != View::Editor {
            stack = stack.child(chrome::controls_bar(s, ink));
        }
        // The content takes what the bars leave: its own `h_full` would
        // be the whole column's height and push the pane's bottom rows
        // (a TUI's input frame) below the window.
        stack.child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .min_h(0.0)
                .w_full()
                .child(content),
        )
    };
    // Overlays (menus, pickers, prompts) collect here and mount above the app.
    let mut layer = Vec::new();
    let mut root = col()
        .w_full()
        .h_full()
        .fill(ink.bg)
        .child(chrome::header(s, ink, cx.titlebar_inset()))
        .child(
            row()
                .grow(1.0)
                .min_h(0.0)
                .w_full()
                .child(sidebar::sidebar(s, ink, &mut layer))
                .child(sidebar::separator(s, ink))
                .child(workspace),
        );
    // Caret-anchored LSP hover card: mounts only once `Cx::caret` has
    // measured the field's caret in window points.
    if s.view == View::Editor
        && let Some(layer) = editor::lsp::hover_layer(s, cx, &s.editor.kit)
    {
        root = root.child(layer);
    }
    // Quick-open palette: scrim + top card mount above everything, the same
    // pair `rune_kit::Dialog` pushes for its modals.
    if s.view == View::Editor && s.editor.quick_open.open {
        for l in editor::quick_open::layers(s, ink, &s.editor.kit) {
            root = root.child(l);
        }
    }
    // The sidebar's menus and its text prompt mount above the app: they are
    // overlays, not part of the tree.
    if let Some(menu) = sidebar::menu_layer(s, &mut layer) {
        root = root.child(menu);
    }
    sidebar::new_session_layer(s, &mut layer);
    if let Some(menu) = editor::files::menu_layer(s, &mut layer) {
        root = root.child(menu);
    }
    if let Some(prompt) = sidebar::input_layer(s, ink, &mut layer) {
        root = root.child(prompt);
    }
    if let Some(menu) = chrome::usage_menu_layer(s, &mut layer) {
        root = root.child(menu);
    }
    if let Some(menu) = tab_debug::menu_layer(s, &mut layer) {
        root = root.child(menu);
    }
    // The New-worktree form is a modal layer too: it mounts above the app
    // while `sidebar_input` shields the keyboard for it.
    if let Some(form) = editor::worktree_form::layer(s, &mut layer) {
        root = root.child(form);
    }
    for map_layer in project_map::layers(s) {
        root = root.child(map_layer);
    }
    // A failed call's notice: a toast the user closes, or that expires after
    // 8 s. Without it the message was recorded and never shown.
    if let Some(text) = &s.notice {
        layer.push(
            rune_kit::ToastRegion::new()
                .toast(
                    rune_kit::Toast::new("notice", text.clone())
                        .on_close(|s: &mut State| s.notice = None)
                        .build(&s.editor.kit),
                )
                .build(&s.editor.kit),
        );
    }
    // Paste with no text field focused belongs to the focused terminal pane.
    rune_kit::root(root, layer).on_paste(|s: &mut State, text: &str, _tx: &mut Tx| {
        if s.view == View::Terminal
            && !s.project_map.open
            && s.sidebar_input.is_none()
            && s.tab_menu.is_none()
            && let Some(session) = s.focused_terminal_session()
        {
            s.paste_into_terminal(session, text);
        }
    })
}

/// `AppSpec::setup`: kick the initial project list, history-layout restore,
/// the session metadata and agent reads, the sidebar width restore, and ACP
/// agent discovery once the state exists.
fn setup(s: &mut State, waker: rune::Waker) {
    s.editor.media_host = Some((rune::ImageCache::new(waker.clone()), waker));
    s.editor.refresh_projects();
    // History pane sizes persist in the shared settings store; the read runs
    // on the worker like every storage access (no UI-thread SQLite).
    let _ = backend::enqueue(
        &mut s.editor.backend,
        backend::Job::project(backend::Req::GetSetting {
            key: editor::history_layout::HISTORY_LAYOUT_KEY.to_string(),
        }),
    );
    // The session metadata rows and the agents the New-session menu lists
    // come from the same worker lane.
    let _ = backend::enqueue(
        &mut s.editor.backend,
        backend::Job::project(backend::Req::SessionMetaList),
    );
    let _ = backend::enqueue(
        &mut s.editor.backend,
        backend::Job::project(backend::Req::Agents),
    );
    // The sidebar's width survives restarts.
    let _ = backend::enqueue(
        &mut s.editor.backend,
        backend::Job::project(backend::Req::GetSetting {
            key: sidebar::SIDEBAR_WIDTH_KEY.to_string(),
        }),
    );
    // The workspace list and the last selected workspace restore on the same
    // lane; the list is requested first so the saved id validates against it.
    s.request_workspaces();
    let _ = backend::enqueue(
        &mut s.editor.backend,
        backend::Job::project(backend::Req::GetSetting {
            key: sidebar::SELECTED_WORKSPACE_KEY.to_string(),
        }),
    );
    // Indent-unit override restores on the same settings lane.
    let _ = backend::enqueue(
        &mut s.editor.backend,
        backend::Job::project(backend::Req::GetSetting {
            key: editor::indent::INDENT_KEY.to_string(),
        }),
    );
    // The inline-blame toggle restores on the same settings lane.
    let _ = backend::enqueue(
        &mut s.editor.backend,
        backend::Job::project(backend::Req::GetSetting {
            key: editor::blame::BLAME_KEY.to_string(),
        }),
    );
    s.editor.acp.refresh();
    // The quota chips, the accounts they name, and the stored theme.
    let _ = backend::enqueue(
        &mut s.editor.backend,
        backend::Job::project(backend::Req::Usage {
            previous: app_core::usage::UsageSnapshot::default(),
        }),
    );
    let _ = backend::enqueue(
        &mut s.editor.backend,
        backend::Job::project(backend::Req::GetSetting {
            key: theme::THEME_KEY.to_string(),
        }),
    );
    let _ = backend::enqueue(
        &mut s.editor.backend,
        backend::Job::project(backend::Req::GetSetting {
            key: settings::SCROLL_SPEED_KEY.to_string(),
        }),
    );
    // The done-sound toggle restores on the same settings lane.
    let _ = backend::enqueue(
        &mut s.editor.backend,
        backend::Job::project(backend::Req::GetSetting {
            key: settings::SOUND_DONE_KEY.to_string(),
        }),
    );
}

/// `AppSpec::poll`: drain the daemon's events and the session replies it
/// is waiting for, then the editor housekeeping — quit-guard swizzle
/// install, cancelled-close reopen, search debounce, save-resume
/// navigation, queued external opens, and pending line reveals.
fn poll(s: &mut State, now: f64, tx: &mut Tx) -> bool {
    let mut changed = false;
    s.clock = now;
    s.now_ms = data::now_ms();
    // The daemon's events: the session rows, the connection state and the
    // terminal's lag notices all land here.
    for event in s.daemon.take_events() {
        s.apply_daemon_event(event);
        changed = true;
    }
    // The replies the session requests are waiting for.
    changed |= s.drain_pending();
    // Closing panes drive their own stop→delete lifecycle.
    changed |= s.poll_closes();
    changed |= s.poll_stop_escalation();
    // Every pane of the active tab holds its own terminal attachment;
    // panes that left the view detach.
    changed |= s.sync_terminals();
    let term_changed = s.poll_terminals(tx);
    changed |= term_changed;
    crate::trace::trace(&s.terminal_trace());
    // A subscriber that fell behind takes a fresh subscription, which
    // replays the daemon's retained output.
    for session in std::mem::take(&mut s.terminal_lagged) {
        s.resubscribe_session(session);
        changed = true;
    }
    // Split ids die with their tabs: drop the measured sizes of splits
    // that no longer exist.
    let live_splits = s.split_ids();
    s.split_sizes.retain(|id, _| live_splits.contains(id));
    // The notice toast closes itself 8 s after it was last set.
    changed |= s.expire_notice(now);
    // The toast still showing wakes the window at its deadline: an idle app
    // would otherwise close it at the next idle tick, late.
    if let Some(remaining) = s.notice_remaining(now) {
        tx.wake_after(std::time::Duration::from_secs_f64(remaining.max(0.05)));
    }
    editor::quit_guard::poll_install();
    editor::navigation::reopen_if_cancelled(s);
    changed |= editor::search::poll(s);
    if let Some(pending) = s.editor.pending_apply.take() {
        // Route through the guard: a save that landed clean applies
        // immediately; anything still dirty re-raises the dialog.
        editor::navigation::navigate(s, pending, tx);
        changed = true;
    }
    changed |= terminal_branch::poll(s, now, tx);
    changed |= editor::branch::poll(&mut s.editor, s.view == View::Editor, now, tx);
    editor::highlight::poll(&mut s.editor, tx);
    // The inline-blame column's relative ages roll over once a minute.
    changed |= editor::blame::poll(&mut s.editor);
    // LSP: drain runtime messages, reconcile document sync, drive hover.
    changed |= editor::lsp::poll(&mut s.editor, tx);
    // Quick-open: rebuild matches when the path cache or checkout moved.
    changed |= editor::quick_open::poll(s);
    changed |= project_map::poll(s);
    // A resolved LSP definition navigates through the same dirty-draft
    // guard as every other pending navigation.
    if let Some(nav) = editor::lsp::take_nav(&mut s.editor) {
        editor::navigation::navigate(s, nav, tx);
        changed = true;
    }
    // An empty/failed LSP definition reply falls back to the heuristic
    // go-to pipeline.
    if let Some(req) = editor::lsp::take_fallback(&mut s.editor) {
        editor::navigation::start_go_to_heuristic(s, req, tx);
        changed = true;
    }
    // History housekeeping that must run even while its rect is unchanged:
    // auto-select, jump_to consumption, table↔layout sync, detail fetches.
    changed |= editor::history::poll(s, tx);
    s.editor.poll_layout_save();
    // The child processes' activity: read once a second, on the worker.
    changed |= s.poll_activity(now);
    // The agent CLIs under the sessions: read every two seconds, on the
    // worker.
    changed |= s.poll_subagents(now);
    if s.tab_menu.is_some() {
        tx.wake_after(std::time::Duration::from_secs(2));
        if s.view != View::Terminal
            || !s
                .visible_tabs()
                .any(|(_, tab)| Some(&tab.id) == s.tab_menu.as_ref().map(|m| &m.tab))
        {
            s.tab_menu = None;
            changed = true;
        }
    }
    // The quota chips and account labels: read about every five seconds on
    // the worker. The wake keeps the cadence alive while the UI is idle;
    // the `System` theme checks the OS appearance on the same tick.
    changed |= s.poll_usage(now);
    tx.wake_after(std::time::Duration::from_secs(5));
    changed |= settings::poll_appearance(s);
    settings::sync_acp_drafts(s);
    if let Some(doc) = s.editor.doc.as_mut()
        && let Some(line) = doc.reveal_line.take()
    {
        // CodeState::reveal selects the line, focuses the field and
        // centres it in the scroll viewport (1-based, clamped).
        doc.code.reveal(line as u32, tx);
        changed = true;
    }
    changed
}

/// Window close (red button / Cmd-W): a dirty draft cancels the close —
/// Rune's close is a notification, so `keep_running` + reopen carries the
/// guard instead of a veto.
fn on_close(s: &mut State, tx: &mut Tx) {
    editor::navigation::on_window_close(s, tx);
}

/// Real quit (post-veto or clean): shut the backend, ACP and daemon
/// runtimes down with the app — `Backend::shutdown` refuses new jobs, lets
/// accepted mutating jobs finish, then joins; `Acp::shutdown` joins its
/// runtime thread after `hub.shutdown()`; `Daemon::shutdown` ends the
/// supervisor and drops every pending reply.
fn on_quit(s: &mut State, _tx: &mut Tx) {
    s.editor.backend.shutdown();
    s.editor.acp.shutdown();
    s.editor.lsp.shutdown();
    s.daemon.shutdown();
    s.editor.sync_quit_dirty();
}

/// Unhandled keys: the Terminal view hands them to the attached session's
/// PTY; the editor's global chords and section/list shortcuts come first
/// (⌘S/⌘F/⌘⇧F/⌘P, 1–4, j/k, Enter — `editor::keyboard`), the quick-open
/// palette owns every key while open, then F12 runs go-to-definition at the
/// caret (LSP first, heuristic fallback) and Esc dismisses the hover card.
fn on_key(s: &mut State, ev: &rune::KeyEvent, tx: &mut Tx) -> bool {
    if s.tab_menu.is_some() {
        return true;
    }
    if s.project_map.open {
        return project_map::key(s, ev, tx);
    }
    // The sidebar's prompt, or one of its searchable selects, owns the
    // keyboard while it is open: its field takes the typing, not the shell.
    if s.sidebar_input.is_some() || s.sidebar_picker_open() {
        return false;
    }
    // App shortcuts come before the Terminal forwards keys to the shell.
    if ev.mods.cmd && !ev.mods.ctrl {
        if matches!(ev.key, rune::Key::Command('c')) {
            s.notice = Some("Copied to clipboard".into());
        }
        if app_shortcut(s, ev, tx) {
            return true;
        }
    }
    if ev.mods.alt && alt_shortcut(s, ev, tx) {
        return true;
    }
    if s.view == View::Terminal {
        // Rune focus left on chrome eats Space — a focused pressable fires
        // on it — before the pane sees the key. Hand it back; the pane
        // separator (`split:split-…`) keeps its arrow-key resize and a
        // focused field keeps its keys.
        if !shortcut_owned(s)
            && !s.sidebar_text.focused
            && tx
                .focused()
                .is_some_and(|key| !key.starts_with("split:split-"))
        {
            tx.blur();
        }
        return terminal_key(s, ev, tx.key_base());
    }
    if s.view != View::Editor {
        return false;
    }
    if s.editor.quick_open.open {
        return editor::quick_open::key(s, ev, tx);
    }
    if editor::keyboard::on_key(s, ev, tx) {
        return true;
    }
    match ev.key {
        rune::Key::Function(12) => {
            // Go-to-definition belongs to the Files code surface: require
            // the section AND a focused editable field so a stray F12 in
            // History/Changes can't navigate on a stale caret.
            if s.editor.section != editor::state::Section::Files {
                return false;
            }
            let Some(doc) = s.editor.doc.as_ref() else {
                return false;
            };
            if !doc.editable() || !doc.code.edit.focused {
                return false;
            }
            let Some(req) = doc.code.go_to_token_at(doc.code.edit.focus) else {
                return false;
            };
            editor::navigation::start_go_to(s, req, tx);
            true
        }
        rune::Key::Escape => s.editor.lsp_dismiss_hover(),
        _ => false,
    }
}

/// A chrome press hands the keyboard back: rune focus left on a pressable
/// eats Space — it re-fires `on_press` — before `on_key` sees it. No-op
/// while a surface owns the keys, so pickers and menus keep their focus.
pub(crate) fn yield_focus(s: &State, tx: &mut Tx) {
    if s.tab_menu.is_none()
        && s.usage_menu.is_none()
        && !s.project_map.open
        && s.sidebar_input.is_none()
        && !shortcut_owned(s)
    {
        tx.blur();
    }
}

/// True while a dialog, picker, menu, draft guard or composition owns the
/// keyboard — app shortcuts wait for it.
fn shortcut_owned(s: &State) -> bool {
    // The worktree form also holds `sidebar_input`, which `on_key` checked.
    s.sidebar_confirm.is_some()
        || s.workspace_remove_confirm.is_some()
        || s.sidebar_menu.is_some()
        || s.sidebar_picker_open()
        || s.editor.guard.is_some()
        || s.editor.quick_open.open
        || s.editor.project_picker.open
        || s.editor.checkout_picker.open
        || s.editor.history.branch_picker.open
        || s.editor.commit.agent_picker.open
        || s.editor.history.row_menu.is_some()
        // A composition only owns the keys in the Terminal, where the IME types.
        || (s.view == View::Terminal && !s.preedit.is_empty())
}

/// The ⌘ shortcuts: `⌘T` a new shell tab, `⌘1`–`⌘9` the Nth tab, `⌘,` the
/// Settings toggle. `⌘W` stays window close. Only plain ⌘ chords count: ⇧ and
/// ⌥ chords belong to other commands. Returns `true` when one ran;
/// none runs while a surface owns the keyboard.
fn app_shortcut(s: &mut State, ev: &rune::KeyEvent, tx: &mut Tx) -> bool {
    let rune::Key::Command(c) = ev.key else {
        return false;
    };
    if shortcut_owned(s) || ev.mods.shift || ev.mods.alt {
        return false;
    }
    match c {
        'k' if project_map::available(s) => {
            project_map::open(s, tx);
            true
        }
        't' if s.view != View::Settings => {
            s.new_shell_tab();
            true
        }
        ',' => {
            let target = if s.view == View::Settings {
                View::Terminal
            } else {
                View::Settings
            };
            editor::navigation::navigate(s, editor::navigation::PendingNav::SetView(target), tx);
            true
        }
        '1'..='9' => {
            if s.view == View::Settings {
                return false;
            }
            activate_visible_tab(s, c as usize - '1' as usize, tx)
        }
        _ => false,
    }
}

/// Activate the Nth visible tab (0-based `n`), leaving the Editor for the
/// Terminal like every tab jump. `false` when there is no such tab.
fn activate_visible_tab(s: &mut State, n: usize, tx: &mut Tx) -> bool {
    let Some(index) = s.visible_tabs().nth(n).map(|(i, _)| i) else {
        return false;
    };
    // Activate before leaving the Editor, so a draft guard's resume
    // lands on the chosen tab.
    s.activate_tab(index);
    if s.view == View::Editor {
        editor::navigation::navigate(
            s,
            editor::navigation::PendingNav::SetView(View::Terminal),
            tx,
        );
    }
    true
}

/// The ⌥ chords, herdr-style: `⌥1`–`⌥9` the Nth visible tab, `⌥←`/`⌥→`
/// cycle tabs, `⌥↑`/`⌥↓` walk the sidebar rows — each press lands on a row
/// and runs its action. Terminal view only — the Editor keeps `⌥←`/`⌥→`
/// for word movement — and a chord falls through when its target does not
/// exist, so a lone tab's `⌥←`/`⌥→` still reaches the shell as word motion.
fn alt_shortcut(s: &mut State, ev: &rune::KeyEvent, tx: &mut Tx) -> bool {
    if s.view != View::Terminal
        || !ev.mods.alt
        || ev.mods.cmd
        || ev.mods.ctrl
        || ev.mods.logo
        || shortcut_owned(s)
    {
        return false;
    }
    match &ev.key {
        // `key_base` names the key's digit even where ⌥ types another
        // character (macOS ⌥2 is `™`).
        rune::Key::Text(text) => match tx.key_base().or_else(|| text.chars().next()) {
            Some(c @ '1'..='9') => activate_visible_tab(s, c as usize - '1' as usize, tx),
            _ => false,
        },
        rune::Key::Left | rune::Key::Right => cycle_visible_tab(s, ev.key == rune::Key::Right),
        rune::Key::Up | rune::Key::Down => sidebar::step(s, ev.key == rune::Key::Down, tx),
        _ => false,
    }
}

/// `⌥←`/`⌥→`: the previous/next visible tab, wrapping around.
fn cycle_visible_tab(s: &mut State, forward: bool) -> bool {
    let visible: Vec<usize> = s.visible_tabs().map(|(i, _)| i).collect();
    if visible.len() < 2 {
        return false;
    }
    let at = visible.iter().position(|i| *i == s.active_tab);
    let next = match (at, forward) {
        (Some(p), true) => visible[(p + 1) % visible.len()],
        (Some(p), false) => visible[(p + visible.len() - 1) % visible.len()],
        (None, true) => visible[0],
        (None, false) => visible[visible.len() - 1],
    };
    s.activate_tab(next);
    true
}

/// Modifier snapshot for the code owner's glyph-hit link overlays —
/// `EditorState::set_modifiers` mirrors the platform go-to modifier onto the
/// active document's `CodeState`.
fn on_modifiers(s: &mut State, mods: rune::Mods, _tx: &mut Tx) {
    s.editor.set_modifiers(mods);
}

/// The Terminal view's keyboard: every key belongs to the focused pane's
/// session — never "the tab's session", a split tab has several. An app
/// shortcut (⌘ on macOS) is left alone — `term::key_bytes` sends only the
/// ⌘ line-editing chords — and an exited session keeps its final grid
/// without taking input.
fn terminal_key(s: &mut State, ev: &rune::KeyEvent, base: Option<char>) -> bool {
    let Some(session) = s.focused_terminal_session() else {
        return false;
    };
    // macOS types `π` for ⌥P; a Meta chord wants the letter under the finger.
    let meta;
    let ev = match (&ev.key, base) {
        (rune::Key::Text(_), Some(c)) if ev.mods.alt && c.is_ascii_alphabetic() => {
            let c = if ev.mods.shift {
                c.to_ascii_uppercase()
            } else {
                c
            };
            meta = rune::KeyEvent {
                key: rune::Key::Text(c.to_string()),
                mods: ev.mods,
            };
            &meta
        }
        _ => ev,
    };
    let Some(bytes) = term::key_bytes(ev) else {
        return false;
    };
    s.type_into_terminal(session, &bytes)
}

/// The input method for the Terminal view: what it composes is drawn at
/// the cursor (`Preedit`) and typed into the focused session once it is
/// final (`Commit`) — Vietnamese Telex and the other marked-text methods.
fn on_ime(s: &mut State, ev: &rune::Ime, _tx: &mut Tx) -> bool {
    if s.project_map.open || s.tab_menu.is_some() {
        return true;
    }
    if s.view != View::Terminal {
        return false;
    }
    match ev {
        rune::Ime::Preedit(text, _) => s.preedit.clone_from(text),
        rune::Ime::Commit(text) => {
            s.preedit.clear();
            let Some(session) = s.focused_terminal_session() else {
                return false;
            };
            return s.type_into_terminal(session, term::committed_text(text).as_bytes());
        }
        rune::Ime::Enabled | rune::Ime::Disabled => return false,
    }
    true
}

/// The window's focus flips: the done sound only plays while it is away.
fn on_focus(s: &mut State, focused: bool, _tx: &mut Tx) {
    s.window_focused = focused;
}

fn main() {
    // A Dock/Finder launch gets launchd's bare PATH, so `claude`/`codex` would not
    // resolve; adopt the login shell's before any thread or the daemon starts.
    app_core::agents::adopt_login_shell_path(&editor::backend::default_shell());
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if !args.iter().any(|a| a == "--scroll-bars") {
        args.push("--scroll-bars".to_string());
        args.push("always".to_string());
    }
    let app = rune::App::new(AppSpec {
        families: ("Inter", "ui-monospace"),
        min_size: Some((760.0, 480.0)),
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        ..AppSpec::new("Goko", (1512.0, 814.0), init_state, view)
    })
    .args(args)
    // Centre the macOS traffic lights in the 48 pt header band.
    .titlebar(rune::Titlebar::new(48.0))
    .keep_running()
    .on_close(on_close)
    .on_quit(on_quit)
    .on_modifiers(on_modifiers)
    .on_ime(on_ime)
    .on_focus(on_focus);
    APP_HANDLE
        .set(app.handle())
        .unwrap_or_else(|_| unreachable!("app handle registered once"));
    app.run();
}

#[cfg(test)]
mod view_tests {
    use super::*;
    use crate::editor::draft::OpenDoc;
    use crate::editor::navigation::{Guard, GuardAction, PendingNav};
    use crate::editor::state::Section;
    use app_core::git_view::{GitViewFile, GitViewSearch, GitViewSearchMatch};
    use app_core::projects::ProjectView;
    use app_core::store::Project;
    use app_core::worktrees::{WorktreeInfo, WorktreeList};
    use rune::{Env, Key, KeyEvent, Mods, Runtime};
    use std::path::PathBuf;
    use std::sync::mpsc;

    fn project(id: &str, name: &str, root: &str) -> ProjectView {
        ProjectView {
            project: Project {
                id: id.into(),
                name: name.into(),
                root: root.into(),
                workspace_id: app_core::store::DEFAULT_WORKSPACE_ID.into(),
                created_at: 0,
            },
            missing: false,
        }
    }

    fn wt(name: &str, path: &str, is_root: bool) -> WorktreeInfo {
        WorktreeInfo {
            name: name.into(),
            path: path.into(),
            branch: None,
            head: String::new(),
            is_root,
            missing: false,
            dirty: None,
            changed: None,
        }
    }

    /// Full-app Editor scene at `view == View::Editor`: registered project,
    /// selected checkout, populated file tree — and the mock terminal
    /// session still selected, so a leaked controls bar would show.
    fn editor_scene() -> State {
        let mut s = State::new(test_handle());
        s.view = View::Editor;
        s.editor.project_id = Some("p1".into());
        s.editor.projects = vec![
            project("p1", "web", "/work/a/web"),
            project("p2", "api", "/work/b/api"),
        ];
        s.editor.worktrees = WorktreeList {
            repo: true,
            worktrees: vec![wt("web", "/work/a/web", true)],
        };
        let co = PathBuf::from("/tmp/goko-view-qa");
        s.editor.checkout = Some(co.clone());
        s.editor.checkout_key = Some(crate::editor::state::checkout_key("p1", &co));
        s.editor.section = Section::Files;
        s.editor.files.paths = [
            "README.md",
            "TREE_ONLY.txt",
            "docs/guide.md",
            "src/a.rs",
            "src/b.rs",
            "top.txt",
        ]
        .iter()
        .map(|p| p.to_string())
        .collect();
        s.editor.rebuild_file_model();
        s
    }

    /// An editable text doc whose buffer differs from the saved text.
    fn dirty_doc() -> OpenDoc {
        let file = GitViewFile {
            path: "src/a.rs".into(),
            kind: "text",
            text: Some("fn main() {}\n".into()),
            truncated: false,
            size: 12,
        };
        let mut doc = OpenDoc::new("src/a.rs", &file);
        doc.code.edit.text.push_str("// unsaved edit\n");
        assert!(doc.dirty());
        doc
    }

    fn dirty_scene() -> State {
        let mut s = editor_scene();
        s.editor.doc = Some(dirty_doc());
        s
    }

    fn els(rt: &Runtime<State>) -> Vec<rune::ElementInfo> {
        rt.elements()
    }

    fn press(rt: &mut Runtime<State>, key: Key) {
        rt.key(KeyEvent {
            key,
            mods: Mods::default(),
        });
        rt.frame(1.0 / 60.0);
    }

    /// Index of the element whose ancestor chain (or itself) has `role`.
    fn inside_role(els: &[rune::ElementInfo], idx: usize, role: &str) -> bool {
        let mut i = Some(idx);
        while let Some(n) = i {
            if els[n].role == role {
                return true;
            }
            i = els[n].parent;
        }
        false
    }

    /// Every mounted element stays inside the viewport horizontally —
    /// the regression the fixed-width columns caused.
    fn assert_no_horizontal_overflow(rt: &Runtime<State>, width: f32) {
        let mut bad = Vec::new();
        for e in els(rt) {
            let v = e.visible;
            if v[2] > 0.5 && !(v[0] >= -0.5 && v[0] + v[2] <= width + 0.5) {
                bad.push(format!(
                    "role={:?} label={:?} text={:?} key={:?} visible={:?}",
                    e.role,
                    e.label,
                    e.text,
                    e.key.as_deref(),
                    v
                ));
            }
        }
        assert!(
            bad.is_empty(),
            "overflow in {}px:\n{}",
            width,
            bad.join("\n")
        );
    }

    /// A daemon session in the scene, exactly as the client's `session.list`
    /// delivers it: no metadata row unless the test adds one.
    fn seed_session(s: &mut State, title: &str, cwd: &str) -> String {
        let info = protocol::session::SessionInfo {
            id: protocol::session::SessionId::new(),
            title: title.into(),
            argv: vec!["/bin/zsh".into()],
            cwd: cwd.into(),
            state: protocol::session::SessionState::Running,
            pid: None,
            exit_code: None,
            error: None,
            started_at: 0,
            ended_at: None,
        };
        let key = crate::data::SessionInfo::key(info.id);
        s.upsert_session(info);
        key
    }

    #[test]
    fn editor_view_hides_terminal_session_controls() {
        // A selected session in the Editor view: before the fix the
        // controls bar rendered Stop/Close pane inside the editor.
        let mut s = editor_scene();
        let key = seed_session(&mut s, "shell", "/tmp");
        s.selected_session = Some(key);
        assert!(s.selected_session.is_some());
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        // The controls strip itself must not be mounted in the Editor view.
        assert!(
            !els(&rt)
                .iter()
                .any(|el| el.key.as_deref() == Some("controls-bar")),
            "the session controls leaked into the editor"
        );
    }

    /// A pane holding `key`'s session with the terminal `terminal` feeds:
    /// the attachment every visible pane owns.
    fn attach(s: &mut State, key: &str, terminal: term::Terminal) {
        s.attachments.insert(
            key.to_string(),
            panes::Attachment {
                term: terminal,
                lines: Vec::new(),
                links: Vec::new(),
                cols: term::TERM_COLS,
                rows: term::TERM_ROWS,
                pty: (0, 0),
            },
        );
    }

    #[test]
    fn an_attached_terminal_is_drawn_by_the_poll_that_attaches_it() {
        // The terminal must not wait for a frame that nobody asked for:
        // the poll that attaches the session draws its grid.
        let mut s = State::new(test_handle());
        let key = seed_session(&mut s, "shell", "/tmp");
        s.focus_session_tab(&key);
        let session = s.session_id(&key).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let (_ready_tx, ready_rx) = std::sync::mpsc::channel();
        let terminal = term::Terminal::new(session, rx, ready_rx);
        tx.send(daemon::Output {
            seq: 0,
            data: b"hello\r\n".to_vec(),
        })
        .unwrap();
        attach(&mut s, &key, terminal);
        let pane_key = key.clone();

        let mut rt = Runtime::new(s, view, (1512.0, 814.0), Env::new(true, 2.0));
        rt.update(|s, tx| {
            s.poll_terminals(tx);
        });
        let mut lines = 0;
        rt.update(|s, _| {
            lines = s
                .attachments
                .get(&pane_key)
                .map(|a| a.lines.len())
                .unwrap_or(0);
        });
        assert!(lines > 0, "the terminal drew its grid in the same poll");
    }

    #[test]
    fn apply_theme_repaints_an_idle_terminal_without_a_resize() {
        // An attached terminal that has settled stays cached in the old
        // palette: only apply_theme's invalidation repaints it — no output,
        // no resize, no parser movement.
        let mut s = State::new(test_handle());
        let key = seed_session(&mut s, "shell", "/tmp");
        let session = s.session_id(&key).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let (_ready_tx, ready_rx) = std::sync::mpsc::channel();
        let mut terminal = term::Terminal::new(session, rx, ready_rx);
        tx.send(daemon::Output {
            seq: 0,
            data: "\u{1b}[41mRED\u{1b}[0m\r\n".as_bytes().to_vec(),
        })
        .unwrap();
        terminal.feed();
        attach(&mut s, &key, terminal);
        s.apply_theme();
        let before = s.attachments[&key].lines.clone();
        assert!(!before.is_empty(), "the idle grid was drawn once");

        s.theme_choice = theme::Theme::Light;
        s.apply_theme();
        let att = &s.attachments[&key];
        assert_ne!(att.lines, before, "the cached rows took the new ink");
        assert_eq!(
            att.lines.len(),
            before.len(),
            "the same grid: no wrapped lines, no resize"
        );
        assert_eq!(att.term.size(), (att.cols, att.rows));
        assert_eq!(att.term.next_seq, 14, "the stream offsets did not move");
    }

    #[test]
    fn the_terminal_grid_fits_the_pane() {
        // The grid follows the pane: a line that wrapped would make the
        // content taller than the viewport, and that is what made a
        // full-screen program shake.
        let mut s = State::new(test_handle());
        let key = seed_session(&mut s, "shell", "/tmp");
        s.focus_session_tab(&key);
        let session = s.session_id(&key).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let (_ready_tx, ready_rx) = std::sync::mpsc::channel();
        let mut terminal = term::Terminal::new(session, rx, ready_rx);
        tx.send(daemon::Output {
            seq: 0,
            data: "x".repeat(400).into_bytes(),
        })
        .unwrap();
        terminal.feed();
        attach(&mut s, &key, terminal);
        let pane_key = key.clone();

        let mut rt = Runtime::new(s, view, (1512.0, 814.0), Env::new(true, 2.0));
        rt.poll = Some(poll);
        rt.frame(0.0);
        // The first layout measures the pane and resizes the grid; the
        // poll then draws the grid at its new size.
        rt.frame(1.0 / 60.0);

        // Every drawn row is one line tall: nothing wrapped.
        let mut rows = 0;
        let mut height = 0.0;
        let mut widest = 0.0_f32;
        for e in els(&rt) {
            let Some(text) = e.text.as_deref() else {
                continue;
            };
            if !text.starts_with("xxx") {
                continue;
            }
            rows += 1;
            height += e.visible[3];
            widest = widest.max(e.visible[2]);
            assert!(
                e.visible[3] <= 17.0,
                "a terminal row wrapped: height {} for {text:?}",
                e.visible[3]
            );
        }
        assert!(rows > 0, "the terminal drew its grid");

        // The grid took the pane's size, and the drawn grid fits inside
        // it — the pane never has to scroll, so the content never jumps.
        let mut cols = 0;
        let mut grid_rows = 0;
        rt.update(|s, _| {
            if let Some(att) = s.attachments.get(&pane_key) {
                cols = att.cols;
                grid_rows = att.rows;
            }
        });
        assert!(cols >= 20, "the grid took the pane's width: {cols}");
        assert!(
            grid_rows >= 4,
            "the grid took the pane's height: {grid_rows}"
        );
        let pane = 1512.0 - sidebar::DEFAULT_SIDEBAR_W - 8.0 - 2.0 * 8.0;
        assert!(
            widest <= pane,
            "the grid is wider than the pane: {widest} > {pane}"
        );
        assert!(
            height <= 814.0 - 48.0 - 41.0 - 40.0,
            "the grid is taller than the pane: {height}"
        );
    }

    #[test]
    fn a_notice_is_shown_as_a_toast() {
        let mut s = State::new(test_handle());
        s.notice = Some("boom happened".into());
        let mut rt = Runtime::new(s, view, (1512.0, 814.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        let shown = |rt: &Runtime<State>| {
            els(rt)
                .iter()
                .any(|e| e.text.as_deref() == Some("boom happened"))
        };
        assert!(shown(&rt), "the notice is on screen");
    }

    #[test]
    fn the_notice_reports_the_time_left_before_it_closes() {
        let mut s = State::new(test_handle());
        assert_eq!(
            s.notice_remaining(0.0),
            None,
            "no toast, nothing to wait for"
        );
        s.notice = Some("boom".into());
        s.expire_notice(10.0);
        assert!((s.notice_remaining(10.0).unwrap() - 8.0).abs() < 1e-9);
        assert!((s.notice_remaining(15.0).unwrap() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn a_notice_closes_eight_seconds_after_it_was_set() {
        let mut s = State::new(test_handle());
        s.notice = Some("first".into());
        s.expire_notice(0.0);
        s.expire_notice(7.0);
        assert!(s.notice.is_some(), "still shown at 7 s");
        s.expire_notice(8.1);
        assert!(s.notice.is_none(), "closed past 8 s");
    }

    #[test]
    fn a_newer_notice_restarts_the_eight_second_timer() {
        let mut s = State::new(test_handle());
        s.notice = Some("first".into());
        s.expire_notice(0.0);
        s.notice = Some("second".into());
        s.expire_notice(7.0);
        s.expire_notice(14.9);
        assert_eq!(s.notice.as_deref(), Some("second"), "alive until 15 s");
        s.expire_notice(15.0);
        assert!(s.notice.is_none(), "closed 8 s after the second notice");
    }

    #[test]
    fn a_full_screen_redraw_does_not_move_the_grid() {
        // What an agent TUI does: rewrite every row of the screen. The
        // grid may change, but the rows must stay where they are — a
        // moving row is what the eye reads as shaking.
        let mut s = State::new(test_handle());
        let key = seed_session(&mut s, "shell", "/tmp");
        s.focus_session_tab(&key);
        let session = s.session_id(&key).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let (_ready_tx, ready_rx) = std::sync::mpsc::channel();
        let mut terminal = term::Terminal::new(session, rx, ready_rx);
        tx.send(daemon::Output {
            seq: 0,
            data: "x".repeat(400).into_bytes(),
        })
        .unwrap();
        terminal.feed();
        attach(&mut s, &key, terminal);

        let mut rt = Runtime::new(s, view, (1512.0, 814.0), Env::new(true, 2.0));
        rt.poll = Some(poll);
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);

        let rows_of = |rt: &Runtime<State>| -> Vec<[f32; 4]> {
            els(rt)
                .iter()
                .filter(|e| e.text.as_deref().is_some_and(|t| t.starts_with("xxx")))
                .map(|e| e.visible)
                .collect()
        };
        let before = rows_of(&rt);
        assert!(!before.is_empty(), "the terminal drew its grid");

        // The redraw: every row is rewritten, with the cursor jumping home.
        let mut redraw = String::from("\u{1b}[H");
        for line in 0..20 {
            redraw.push_str(&format!("\u{1b}[{};1Hxxx{line} {}", line, "y".repeat(120)));
        }
        tx.send(daemon::Output {
            seq: 1,
            data: redraw.into_bytes(),
        })
        .unwrap();
        rt.frame(1.0 / 60.0);
        // The repaint waits for the burst to settle.
        std::thread::sleep(std::time::Duration::from_millis(30));
        rt.frame(1.0 / 60.0);

        let after = rows_of(&rt);
        assert!(after.len() >= 2, "the redraw drew its rows");
        // The grid starts where it did: the redraw changed the content,
        // not the layout.
        assert_eq!(
            before.first().unwrap()[1],
            after.first().unwrap()[1],
            "the grid moved: {:?} -> {:?}",
            before.first().unwrap(),
            after.first().unwrap()
        );
        // And its rows sit on the line grid.
        let gap = after[1][1] - after[0][1];
        assert!(
            (gap - 16.0).abs() < 1.0,
            "the rows are not on the line grid: {gap}"
        );
    }

    #[test]
    fn pickers_fit_duplicate_project_names_at_760() {
        let mut s = editor_scene();
        // Two registered projects sharing a name with deep canonical roots:
        // labels disambiguate by path and must not stretch the bar.
        s.editor.projects = vec![
            project(
                "p1",
                "web",
                "/Users/dev/work/clients/acme-corp/frontend/apps/web",
            ),
            project(
                "p2",
                "web",
                "/Users/dev/work/clients/globex-inc/backend/services/web",
            ),
        ];
        s.editor.project_id = Some("p1".into());
        let mut rt = Runtime::new(s, view, (760.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        let mut combos: Vec<_> = els(&rt)
            .into_iter()
            .filter(|e| e.role == "combobox")
            .collect();
        combos.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
        assert_eq!(combos.len(), 2, "project + checkout pickers mounted");
        for c in &combos {
            assert!(c.rect[2] > 40.0, "picker collapsed: {:?}", c.rect);
            assert!(
                c.rect[0] >= 0.0 && c.rect[0] + c.rect[2] <= 760.0,
                "picker escapes viewport: {:?}",
                c.rect
            );
        }
        // The project picker's help keeps the full disambiguated identity.
        let help = combos[0].help.as_deref().unwrap_or_default();
        assert!(
            help.contains("acme-corp"),
            "project picker lost its path identity: {help:?}"
        );
        let add = els(&rt)
            .into_iter()
            .find(|e| e.label.as_deref() == Some("Add project"));
        assert!(add.is_none(), "Add project button is removed from header");
        assert_no_horizontal_overflow(&rt, 760.0);
    }

    #[test]
    fn search_open_shows_results_in_aside_without_overflow() {
        let mut s = dirty_scene();
        s.editor.doc = Some({
            let file = GitViewFile {
                path: "src/a.rs".into(),
                kind: "text",
                text: Some("fn main() {}\n".into()),
                truncated: false,
                size: 12,
            };
            OpenDoc::new("src/a.rs", &file)
        });
        s.editor.files.search.open = true;
        s.editor.files.search.results = Some(GitViewSearch {
            matches: (1..=30)
                .map(|i| GitViewSearchMatch {
                    path: format!("src/f{i}.rs"),
                    line: i,
                    text: "fn f() -> i32 { i }".into(),
                })
                .collect(),
            truncated: false,
        });
        let mut rt = Runtime::new(s, view, (760.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        let e = els(&rt);
        // Results live in the aside (desktop SplitPane parity): a match row
        // is mounted and the file tree is gone.
        assert!(
            e.iter()
                .any(|el| el.label.as_deref() == Some("src/f1.rs:1")),
            "search result row missing"
        );
        assert!(
            !e.iter()
                .any(|el| el.label.as_deref() == Some("TREE_ONLY.txt")),
            "file tree still mounted beside results"
        );
        assert_no_horizontal_overflow(&rt, 760.0);
        // The editor surface keeps usable width in the remaining space:
        // its `text_input` mounts below the toolbars (y > 150 at this
        // viewport) and must be wider than a sliver.
        let editor_fields: Vec<_> = els(&rt)
            .into_iter()
            .filter(|el| el.role == "textbox" && el.rect[1] > 150.0)
            .collect();
        assert!(
            editor_fields.iter().any(|el| el.rect[2] > 100.0),
            "editor surface collapsed: {editor_fields:?}"
        );
    }

    #[test]
    fn guard_dialog_is_modal_with_inert_background() {
        let mut s = dirty_scene();
        s.editor.guard = Some(Guard {
            pending: PendingNav::SetSection(Section::History),
            doc_path: "src/a.rs".into(),
            quitting: false,
        });
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        let e = els(&rt);
        let dialog_idx = e
            .iter()
            .position(|el| el.role == "dialog")
            .expect("guard dialog mounted with dialog role");
        // Background controls are inert; the dialog's own controls are not.
        let tab_idx = e
            .iter()
            .position(|el| el.role == "tab")
            .expect("section tabs mounted");
        assert!(e[tab_idx].inert, "background tab not inert under modal");
        for label in ["Cancel", "Discard", "Save"] {
            // The doc bar also renders Save/Discard for the dirty draft —
            // pick the control INSIDE the dialog.
            let idx = e
                .iter()
                .enumerate()
                .find(|(i, el)| {
                    (el.label.as_deref() == Some(label) || el.text.as_deref() == Some(label))
                        && inside_role(&e, *i, "dialog")
                })
                .map(|(i, _)| i)
                .unwrap_or_else(|| panic!("{label} button mounted in dialog"));
            assert!(!e[idx].inert, "{label} inert inside dialog");
        }
        let _ = dialog_idx;
    }

    #[test]
    fn escape_cancels_guard_keeping_doc_identity_and_text() {
        let mut s = dirty_scene();
        s.editor.guard = Some(Guard {
            pending: PendingNav::SetSection(Section::History),
            doc_path: "src/a.rs".into(),
            quitting: false,
        });
        let before_text = s.editor.doc.as_ref().unwrap().code.edit.text.clone();
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        assert!(rt.state.editor.guard.is_some());

        press(&mut rt, Key::Escape);
        assert!(rt.state.editor.guard.is_none(), "Escape dismissed guard");
        let doc = rt.state.editor.doc.as_ref().expect("doc retained");
        assert!(doc.dirty(), "draft must stay dirty after Cancel");
        assert_eq!(doc.code.edit.text, before_text, "buffer untouched");
        assert_eq!(doc.path, "src/a.rs");
        // Identity + pending navigation all preserved: Cancel drops the
        // pending SetSection, keeps project/checkout/section as they were.
        assert_eq!(rt.state.editor.project_id.as_deref(), Some("p1"));
        assert_eq!(
            rt.state.editor.checkout.as_deref(),
            Some(std::path::Path::new("/tmp/goko-view-qa"))
        );
        assert_eq!(rt.state.editor.section, Section::Files);
    }

    #[test]
    fn tab_focus_stays_trapped_inside_guard_dialog() {
        let mut s = dirty_scene();
        s.editor.guard = Some(Guard {
            pending: PendingNav::SetSection(Section::History),
            doc_path: "src/a.rs".into(),
            quitting: false,
        });
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.set_tab_visits_controls(true);
        rt.frame(0.0);

        // Walk Tab around the dialog; focus must never land on inert
        // background chrome.
        for _ in 0..8 {
            press(&mut rt, Key::Tab);
            let e = els(&rt);
            if let Some(fidx) = e.iter().position(|el| el.focused) {
                assert!(
                    inside_role(&e, fidx, "dialog"),
                    "Tab escaped the guard dialog to {:?}",
                    e[fidx].label
                );
            }
        }
    }

    #[test]
    fn section_tabs_report_selected_state() {
        let s = editor_scene();
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        // The workspace tab strip also uses role="tab" — section tabs are
        // keyed `section:<label>`.
        let tabs: Vec<_> = els(&rt)
            .into_iter()
            .filter(|e| {
                e.role == "tab" && e.key.as_deref().is_some_and(|k| k.starts_with("section:"))
            })
            .collect();
        assert_eq!(tabs.len(), 5, "five section tabs mounted");
        let selected: Vec<_> = tabs.iter().filter(|t| t.item_selected).collect();
        assert_eq!(selected.len(), 1, "exactly one tab selected");
        assert_eq!(selected[0].label.as_deref(), Some("Files"));
        for t in &tabs {
            assert_eq!(
                t.item_selected,
                t.label.as_deref() == Some("Files"),
                "tab {:?} selected state wrong",
                t.label
            );
        }
    }

    /// Doc bar at the narrow 760px viewport with a long path, a dirty
    /// draft (Save/Discard live), staged change data (Show changes) and a
    /// save error (Reload from disk): every control stays mounted inside
    /// the window — the path label truncates instead of pushing buttons
    /// off-screen.
    #[test]
    fn doc_bar_controls_fit_long_path_at_760() {
        let mut s = editor_scene();
        let long_path = "src/deeply/nested/directory/structure/with/a/very/long/file/name.rs";
        let file = GitViewFile {
            path: long_path.into(),
            kind: "text",
            text: Some("fn main() {}\n".into()),
            truncated: false,
            size: 12,
        };
        let mut doc = OpenDoc::new(long_path, &file);
        doc.code.edit.text.push_str("// unsaved edit\n");
        doc.save_error = Some("disk conflict".into());
        s.editor.doc = Some(doc);
        s.editor.changes.data = Some(app_core::git_view::GitViewChanges {
            branch: Some("main".into()),
            head: "abc1234".into(),
            conflicts: vec![],
            staged: vec![app_core::git_view::StatusEntry {
                path: long_path.into(),
                orig_path: None,
                status: "modified",
                added: None,
                removed: None,
            }],
            unstaged: vec![],
            untracked: vec![],
            ahead: None,
        });
        s.editor.changes.checkout = s.editor.checkout_key.clone();

        let mut rt = Runtime::new(s, view, (760.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let labels: Vec<String> = els(&rt).into_iter().filter_map(|e| e.label).collect();
        for want in ["Save", "Discard", "Show changes", "Reload from disk"] {
            assert!(
                labels.iter().any(|l| l == want),
                "missing {want}: {labels:?}"
            );
        }
        assert_no_horizontal_overflow(&rt, 760.0);
    }

    // -- sidebar → editor identity -------------------------------------------

    /// Two registered projects as the backend's Projects reply leaves them,
    /// each with its own loaded worktree list — p1 has two linked
    /// worktrees, p2 has one. Terminal view, nothing committed yet.
    fn sidebar_scene() -> State {
        let mut s = State::new(test_handle());
        s.editor.projects = vec![
            project("p1", "web", "/work/a/web"),
            project("p2", "api", "/work/b/api"),
        ];
        let lists = [
            (
                "p1",
                vec![
                    wt("web", "/work/a/web", true),
                    wt("feat-a", "/work/a/feat-a", false),
                    wt("feat-b", "/work/a/feat-b", false),
                ],
            ),
            (
                "p2",
                vec![
                    wt("api", "/work/b/api", true),
                    wt("fix", "/work/b/fix", false),
                ],
            ),
        ];
        for (pid, worktrees) in lists {
            s.editor.sidebar_worktrees.insert(
                pid.to_string(),
                crate::editor::state::SidebarWorktrees {
                    list: Some(WorktreeList {
                        repo: true,
                        worktrees,
                    }),
                    loading: false,
                    error: None,
                },
            );
        }
        s
    }

    /// Editor already committed to p1's root checkout — the "already in
    /// Editor" starting point for in-editor sidebar switches.
    fn sidebar_editor_scene() -> State {
        let mut s = sidebar_scene();
        s.view = View::Editor;
        s.editor.project_id = Some("p1".into());
        s.editor.checkout = Some(PathBuf::from("/work/a/web"));
        s.editor.checkout_key = Some(crate::editor::state::checkout_key(
            "p1",
            &PathBuf::from("/work/a/web"),
        ));
        s.active_workspace = Some(crate::data::checkout_key("p1", "/work/a/web"));
        s
    }

    fn click_key(rt: &mut Runtime<State>, key: &str) {
        let e = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(key))
            .unwrap_or_else(|| panic!("element {key} mounted"));
        let (x, y) = (e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn sidebar_lists_real_projects_and_sessions_without_metadata_stay_unassigned() {
        let mut s = sidebar_scene();
        // A daemon session with no stored metadata row: it belongs to no
        // registered project, so it renders under Unassigned.
        seed_session(&mut s, "fix-auth-flow", "/work/a/web");
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let e = els(&rt);
        for key in [
            "sidebar-project:p1",
            "sidebar-project:p2",
            "sidebar-worktree:p1:0",
            "sidebar-worktree:p1:1",
            "sidebar-worktree:p2:0",
            "sidebar-unassigned",
        ] {
            assert!(
                e.iter().any(|el| el.key.as_deref() == Some(key)),
                "missing {key}"
            );
        }
        // Unregistered project ids never become sidebar rows — they are not
        // registered projects, so nothing may select them as identity.
        for mock in ["proj-goko", "proj-notes"] {
            assert!(
                !e.iter()
                    .any(|el| el.key.as_deref() == Some(&format!("sidebar-project:{mock}"))),
                "mock project {mock} leaked into the sidebar"
            );
        }
        // The session renders under Unassigned, never nested under a real
        // project — the honest terminal/session boundary.
        let texts: Vec<&str> = e
            .iter()
            .filter_map(|el| el.text.as_deref().or(el.label.as_deref()))
            .collect();
        assert!(texts.iter().any(|t| t.contains("fix-auth-flow")));
        assert!(texts.contains(&"Unassigned"));
    }

    /// Click the sidebar row of the session titled `title`.
    fn click_session_row(rt: &mut Runtime<State>, title: &str) {
        let row = els(rt)
            .into_iter()
            .find(|e| e.text.as_deref().is_some_and(|t| t.contains(title)) && e.rect[0] < 330.0)
            .unwrap_or_else(|| panic!("session row {title} mounted"));
        let (x, y) = (row.rect[0] + 10.0, row.rect[1] + row.rect[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn a_session_row_opens_its_terminal_from_the_editor_and_from_settings() {
        for start in [View::Terminal, View::Editor, View::Settings] {
            let mut s = sidebar_scene();
            let first = seed_session(&mut s, "first-agent", "/work/a/web");
            let second = seed_session(&mut s, "second-agent", "/work/a/web");
            s.focus_session_tab(&first);
            s.focus_session_tab(&second);
            s.view = start;
            let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
            rt.frame(0.0);
            click_session_row(&mut rt, "first-agent");
            assert_eq!(rt.state.view, View::Terminal, "from {start:?}");
            assert_eq!(rt.state.selected_session.as_deref(), Some(first.as_str()));
            assert_eq!(
                rt.state.focused_session_key().as_deref(),
                Some(first.as_str()),
                "the clicked session's pane is the active one, from {start:?}"
            );
        }
    }

    #[test]
    fn a_tab_pill_opens_its_terminal_from_the_editor() {
        let mut s = sidebar_scene();
        let first = seed_session(&mut s, "first-agent", "/work/a/web");
        let second = seed_session(&mut s, "second-agent", "/work/a/web");
        s.focus_session_tab(&first);
        s.focus_session_tab(&second);
        s.view = View::Editor;
        let tab = s.open_tabs[0].id.clone();
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        click_key(&mut rt, &format!("tab:{tab}"));
        assert_eq!(rt.state.view, View::Terminal);
        assert_eq!(rt.state.active_tab, 0);
    }

    #[test]
    fn a_session_row_click_in_the_editor_respects_the_draft_guard() {
        let mut s = sidebar_scene();
        let key = seed_session(&mut s, "agent-one", "/work/a/web");
        s.focus_session_tab(&key);
        s.view = View::Editor;
        s.editor.doc = Some(editor::draft::OpenDoc::new(
            "a.txt",
            &app_core::git_view::GitViewFile {
                path: "a.txt".into(),
                kind: "text",
                text: Some("x".into()),
                truncated: false,
                size: 1,
            },
        ));
        s.editor.doc.as_mut().unwrap().code.edit.text = "edited".into();
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        click_session_row(&mut rt, "agent-one");
        assert_eq!(rt.state.view, View::Editor, "an unsaved draft asks first");
        assert!(rt.state.editor.guard.is_some(), "the guard dialog is up");
        assert_eq!(rt.state.selected_session.as_deref(), Some(key.as_str()));
    }

    #[test]
    fn sidebar_project_row_activates_real_checkout_then_editor_opens_it() {
        let s = sidebar_scene();
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        // The original bug: pick a real project in the sidebar, then open
        // the Editor — the SAME project+checkout must activate.
        click_key(&mut rt, "sidebar-project:p1");
        assert_eq!(rt.state.view, View::Terminal, "selection alone stays put");
        assert_eq!(rt.state.editor.project_id.as_deref(), Some("p1"));
        assert_eq!(
            rt.state.editor.checkout,
            Some(PathBuf::from("/work/a/web")),
            "root row selects the registered project root"
        );
        assert_eq!(
            rt.state.active_workspace.as_deref(),
            Some(crate::data::checkout_key("p1", "/work/a/web").as_str())
        );
        click_key(&mut rt, "view:Editor");
        assert_eq!(rt.state.view, View::Editor);
        assert_eq!(rt.state.editor.project_id.as_deref(), Some("p1"));
        assert_eq!(
            rt.state.editor.checkout,
            Some(PathBuf::from("/work/a/web")),
            "Editor activated the sidebar's committed checkout"
        );
    }

    #[test]
    fn sidebar_worktree_row_selects_exact_checkout_while_in_editor() {
        let mut rt = Runtime::new(
            sidebar_editor_scene(),
            view,
            (1100.0, 700.0),
            Env::new(true, 2.0),
        );
        rt.frame(0.0);
        // Already in Editor on p1's root: a worktree row press must move
        // project+checkout identity to that exact registered path.
        click_key(&mut rt, "sidebar-worktree:p1:1"); // feat-b
        assert_eq!(rt.state.view, View::Editor);
        assert_eq!(rt.state.editor.project_id.as_deref(), Some("p1"));
        assert_eq!(
            rt.state.editor.checkout,
            Some(PathBuf::from("/work/a/feat-b"))
        );
        assert_eq!(
            rt.state.active_workspace.as_deref(),
            Some(crate::data::checkout_key("p1", "/work/a/feat-b").as_str())
        );
        // A different project's worktree row switches project AND checkout.
        click_key(&mut rt, "sidebar-worktree:p2:0"); // fix
        assert_eq!(rt.state.editor.project_id.as_deref(), Some("p2"));
        assert_eq!(rt.state.editor.checkout, Some(PathBuf::from("/work/b/fix")));
        assert_eq!(
            rt.state.active_workspace.as_deref(),
            Some(crate::data::checkout_key("p2", "/work/b/fix").as_str())
        );
    }

    #[test]
    fn sidebar_select_with_dirty_doc_guard_discard_commits() {
        let mut s = sidebar_editor_scene();
        s.editor.doc = Some(dirty_doc());
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        click_key(&mut rt, "sidebar-project:p2");
        // The guard held: NOTHING moved — project, checkout, workspace
        // highlight and the dirty draft all still belong to p1.
        assert!(rt.state.editor.guard.is_some());
        assert_eq!(rt.state.editor.project_id.as_deref(), Some("p1"));
        assert_eq!(
            rt.state.active_workspace.as_deref(),
            Some(crate::data::checkout_key("p1", "/work/a/web").as_str())
        );
        assert!(rt.state.editor.doc.as_ref().unwrap().dirty());
        // Resolve the guard the way the dialog's Discard button does —
        // Runtime's headless hit-test does not deliver presses inside
        // modal layers, so the state-level path is the honest check.
        rt.update(|s, tx| {
            crate::editor::navigation::resolve_guard(s, GuardAction::Discard, tx);
        });
        assert_eq!(rt.state.editor.project_id.as_deref(), Some("p2"));
        assert_eq!(
            rt.state.editor.checkout,
            Some(PathBuf::from("/work/b/api")),
            "Discard applied the pending sidebar selection"
        );
        assert!(rt.state.editor.doc.is_none());
        assert_eq!(
            rt.state.active_workspace.as_deref(),
            Some(crate::data::checkout_key("p2", "/work/b/api").as_str())
        );
    }

    #[test]
    fn sidebar_select_with_dirty_doc_guard_cancel_restores_everything() {
        let mut s = sidebar_editor_scene();
        s.editor.doc = Some(dirty_doc());
        let before_text = s.editor.doc.as_ref().unwrap().code.edit.text.clone();
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        click_key(&mut rt, "sidebar-worktree:p2:0"); // fix
        assert!(rt.state.editor.guard.is_some());
        rt.update(|s, tx| {
            crate::editor::navigation::resolve_guard(s, GuardAction::Cancel, tx);
        });
        assert!(rt.state.editor.guard.is_none(), "Cancel resolved the guard");
        assert_eq!(rt.state.editor.project_id.as_deref(), Some("p1"));
        assert_eq!(rt.state.editor.checkout, Some(PathBuf::from("/work/a/web")));
        assert_eq!(
            rt.state.active_workspace.as_deref(),
            Some(crate::data::checkout_key("p1", "/work/a/web").as_str()),
            "Cancel restored the previous visible workspace identity"
        );
        let doc = rt.state.editor.doc.as_ref().expect("draft retained");
        assert!(doc.dirty());
        assert_eq!(doc.code.edit.text, before_text, "buffer untouched");
    }

    #[test]
    fn sidebar_status_rows_cover_loading_error_and_empty() {
        // Fresh State::new: no projects yet, no load in flight — the honest
        // empty state, never a fabricated row.
        let s = State::new(test_handle());
        assert!(s.active_workspace.is_none());
        assert!(s.editor.project_id.is_none());
        assert!(s.editor.checkout.is_none());
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let texts: Vec<String> = els(&rt)
            .iter()
            .filter_map(|e| e.text.clone().or_else(|| e.label.clone()))
            .collect();
        assert!(
            texts.iter().any(|t| t.contains("No projects registered")),
            "empty state missing: {texts:?}"
        );

        // Initial async load in flight → loading row. Mutations go through
        // `update` so the next frame knows the view reads changed state.
        rt.update(|s, _| s.editor.projects_loading = true);
        rt.frame(0.0);
        let texts: Vec<String> = els(&rt)
            .iter()
            .filter_map(|e| e.text.clone().or_else(|| e.label.clone()))
            .collect();
        assert!(texts.iter().any(|t| t.contains("Loading projects")));

        // Registry error with no projects → the error itself, in danger tone.
        rt.update(|s, _| {
            s.editor.projects_loading = false;
            s.editor.projects_error = Some("goko.db is locked".into());
        });
        rt.frame(0.0);
        let texts: Vec<String> = els(&rt).iter().filter_map(|e| e.text.clone()).collect();
        assert!(texts.iter().any(|t| t.contains("goko.db is locked")));

        // A per-project worktree failure renders under ITS row only.
        rt.update(|s, _| *s = sidebar_scene());
        rt.update(|s, _| {
            let entry = s
                .editor
                .sidebar_worktrees
                .get_mut("p2")
                .expect("p2 worktrees seeded");
            entry.error = Some("not a git repository".into());
            entry.list = None;
        });
        rt.frame(0.0);
        let texts: Vec<String> = els(&rt).iter().filter_map(|e| e.text.clone()).collect();
        assert!(texts.iter().any(|t| t.contains("not a git repository")));
        // p1's rows are untouched.
        assert!(
            els(&rt)
                .iter()
                .any(|el| el.key.as_deref() == Some("sidebar-worktree:p1:0"))
        );
    }

    fn workspace(id: &str, name: &str) -> app_core::store::Workspace {
        app_core::store::Workspace {
            id: id.into(),
            name: name.into(),
            created_at: 0,
        }
    }

    /// Two workspaces with p1 in "Work" and p2 in Default, "Work" selected.
    fn two_workspace_scene() -> State {
        let mut s = sidebar_scene();
        s.workspaces = vec![
            workspace(app_core::store::DEFAULT_WORKSPACE_ID, "Default"),
            workspace("w1", "Work"),
        ];
        s.editor.projects[0].project.workspace_id = "w1".into();
        s.selected_workspace = Some("w1".into());
        s
    }

    /// A shell session with `title` and `project_id`, then the daemon's update
    /// reporting its shell in `cd_to`. Returns the session key.
    fn session_that_cds(
        s: &mut State,
        title: &str,
        project_id: Option<&str>,
        from: &str,
        cd_to: &str,
    ) -> String {
        let mut info = protocol::session::SessionInfo {
            id: protocol::session::SessionId::new(),
            title: String::new(),
            argv: vec!["/bin/zsh".into()],
            cwd: from.into(),
            state: protocol::session::SessionState::Running,
            pid: None,
            exit_code: None,
            error: None,
            started_at: 0,
            ended_at: None,
        };
        let key = crate::data::SessionInfo::key(info.id);
        s.session_metas.insert(
            key.clone(),
            app_core::store::SessionMeta {
                session_id: key.clone(),
                project_id: project_id.map(Into::into),
                agent: "shell".into(),
                argv: info.argv.clone(),
                cwd: from.into(),
                title: title.into(),
                created_at: 0,
                workspace_id: None,
            },
        );
        s.upsert_session(info.clone());
        info.cwd = cd_to.into();
        s.upsert_session(info);
        key
    }

    /// The flow from a new workspace: its `+` → Shell runs a terminal that
    /// belongs to it, so its row and tab show there — even with the editor's
    /// project still in Default — while Default's own loose terminal does not.
    /// Removing a workspace closes what runs in it — its projects' sessions
    /// and its own terminals — and nothing else.
    #[test]
    fn removing_a_workspace_closes_its_sessions() {
        let mut s = two_workspace_scene();
        let in_project = seed_session_in(&mut s, "/work/a/web", Some("p1"));
        let loose_here = seed_session_in(&mut s, "/tmp", None);
        let loose_default = seed_session_in(&mut s, "/tmp", None);
        let other_project = seed_session_in(&mut s, "/work/b/api", Some("p2"));
        let meta = s.session_metas.get_mut(loose_here.as_str()).unwrap();
        meta.workspace_id = Some("w1".into());
        let meta = meta.clone();
        s.data
            .sessions
            .iter_mut()
            .find(|v| v.info.id == loose_here)
            .unwrap()
            .meta = Some(meta);
        for key in [&in_project, &loose_here, &loose_default, &other_project] {
            s.place_session(key, key, panes::Placement::Tab);
        }

        s.apply_reply(reply(
            Req::RemoveWorkspace { id: "w1".into() },
            Ok(ReplyData::Workspaces(vec![workspace(
                app_core::store::DEFAULT_WORKSPACE_ID,
                "Default",
            )])),
        ));

        let closing = |key: &str| {
            s.open_tabs.iter().any(|tab| {
                panes::collect_panes(&tab.layout)
                    .iter()
                    .any(|pane| pane.session == key && pane.closing_at.is_some())
            })
        };
        assert!(closing(&in_project), "Work's project session closes");
        assert!(closing(&loose_here), "Work's own terminal closes");
        assert!(!closing(&loose_default), "Default's terminal keeps running");
        assert!(
            !closing(&other_project),
            "Default's project session keeps running"
        );
    }

    #[test]
    fn a_new_workspaces_shell_shows_its_row_and_tab_there() {
        let mut s = sidebar_scene();
        s.workspaces = vec![
            workspace(app_core::store::DEFAULT_WORKSPACE_ID, "Default"),
            workspace("w2", ""),
        ];
        editor::navigation::select_checkout(&mut s, "p1".into(), PathBuf::from("/work/a/web"));
        let home = seed_session_in(&mut s, "/tmp", None);
        let mine = seed_session_in(&mut s, "/tmp", None);
        for (key, title, workspace_id) in [
            (&home, "default-shell", None),
            (&mine, "brave-meadow", Some("w2")),
        ] {
            let meta = s.session_metas.get_mut(key.as_str()).unwrap();
            meta.title = title.into();
            meta.workspace_id = workspace_id.map(Into::into);
            let meta = meta.clone();
            let view = s
                .data
                .sessions
                .iter_mut()
                .find(|v| &v.info.id == key)
                .unwrap();
            view.info.title = title.into();
            view.meta = Some(meta);
            s.place_session(key, title, panes::Placement::Tab);
        }
        s.selected_workspace = Some("w2".into());
        let rt = key_runtime(s);
        let live = els(&rt)
            .into_iter()
            .filter(|el| !el.exiting)
            .collect::<Vec<_>>();
        let shown = |text: &str| live.iter().any(|el| el.text.as_deref() == Some(text));
        assert!(shown("New workspace"), "the unnamed workspace's label");
        assert!(shown("brave-meadow"), "its shell has a row");
        assert!(
            !shown("default-shell"),
            "Default's loose terminal stays in Default"
        );
        let tabs: Vec<_> = live
            .iter()
            .filter(|el| el.key.as_deref().is_some_and(|key| key.starts_with("tab:")))
            .filter_map(|el| el.label.clone())
            .collect();
        assert_eq!(tabs, vec!["brave-meadow".to_string()], "only its tab shows");
    }

    #[test]
    fn a_project_session_stays_in_its_project_when_its_shell_cds_elsewhere() {
        let mut s = sidebar_scene();
        s.editor.project_id = Some("p1".into());
        let key = session_that_cds(&mut s, "web", Some("p1"), "/work/a/web", "/work/b/api");
        let meta = s.session_metas.get(&key).expect("meta");
        assert_eq!(meta.project_id.as_deref(), Some("p1"), "not moved to p2");
        assert_eq!(meta.title, "web", "a chosen title is kept");
    }

    #[test]
    fn an_unassigned_shell_joins_the_registered_project_it_cds_into() {
        let mut s = sidebar_scene();
        // `/work/b/fix` is p2's linked worktree, not its root.
        let key = session_that_cds(&mut s, "brave-meadow", None, "/tmp", "/work/b/fix");
        let meta = s.session_metas.get(&key).expect("meta");
        assert_eq!(meta.project_id.as_deref(), Some("p2"));
        assert_eq!(meta.title, "api", "the generated name takes the project's");
        let view = s.data.sessions.iter().find(|v| v.info.id == key).unwrap();
        assert_eq!(
            view.meta.as_ref().and_then(|m| m.project_id.as_deref()),
            Some("p2"),
            "the sidebar's row joins p2 on this update, not the next one"
        );
        assert!(
            s.auto_project_roots.is_empty(),
            "a registered project is never re-added"
        );
    }

    #[test]
    fn the_sidebar_shows_only_the_selected_workspaces_projects() {
        let mut s = two_workspace_scene();
        seed_session(&mut s, "fix-auth-flow", "/work/a/web");
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let e = els(&rt);
        assert!(
            e.iter()
                .any(|el| el.key.as_deref() == Some("sidebar-project:p1"))
        );
        assert!(
            !e.iter()
                .any(|el| el.key.as_deref() == Some("sidebar-project:p2")),
            "the other workspace's project is hidden"
        );
        assert!(
            !e.iter()
                .any(|el| el.key.as_deref() == Some("sidebar-unassigned")),
            "Unassigned is Default-only"
        );
        let texts: Vec<String> = e.iter().filter_map(|el| el.text.clone()).collect();
        assert!(
            !texts.iter().any(|t| t.contains("fix-auth-flow")),
            "an unassigned session stays out of a named workspace"
        );

        rt.update(|s, _| s.select_workspace(app_core::store::DEFAULT_WORKSPACE_ID));
        rt.frame(1.0 / 60.0);
        let e = els(&rt);
        assert!(
            e.iter()
                .any(|el| el.key.as_deref() == Some("sidebar-project:p2"))
        );
        assert!(
            !e.iter()
                .any(|el| el.key.as_deref() == Some("sidebar-project:p1") && !el.exiting),
            "the Work group's project is gone from the live tree"
        );
        assert!(
            e.iter()
                .any(|el| el.key.as_deref() == Some("sidebar-unassigned"))
        );
        let texts: Vec<String> = e.iter().filter_map(|el| el.text.clone()).collect();
        assert!(texts.iter().any(|t| t.contains("fix-auth-flow")));
    }

    /// A daemon session attached to `project_id` (None: Unassigned), with
    /// its stored metadata row, so the sidebar partitions it.
    fn seed_session_in(s: &mut State, cwd: &str, project_id: Option<&str>) -> String {
        let info = protocol::session::SessionInfo {
            id: protocol::session::SessionId::new(),
            title: "waiting-agent".into(),
            argv: vec!["claude".into()],
            cwd: cwd.into(),
            state: protocol::session::SessionState::Running,
            pid: None,
            exit_code: None,
            error: None,
            started_at: 0,
            ended_at: None,
        };
        let key = crate::data::SessionInfo::key(info.id);
        s.session_metas.insert(
            key.clone(),
            app_core::store::SessionMeta {
                session_id: key.clone(),
                project_id: project_id.map(Into::into),
                agent: "claude".into(),
                argv: info.argv.clone(),
                cwd: info.cwd.clone(),
                title: info.title.clone(),
                created_at: 0,
                workspace_id: None,
            },
        );
        s.upsert_session(info);
        key
    }

    fn attention_dot_shown(rt: &Runtime<State>) -> bool {
        els(rt)
            .iter()
            .any(|e| e.help.as_deref() == Some("Another workspace needs you"))
    }

    fn entry_shown(rt: &Runtime<State>, text: &str) -> bool {
        els(rt)
            .iter()
            .any(|e| e.text.as_deref() == Some(text) || e.label.as_deref() == Some(text))
    }

    #[test]
    fn the_selector_flags_a_waiting_session_in_another_workspace() {
        let mut s = two_workspace_scene();
        // p2 is in Default, so the session is hidden while Work is selected.
        let key = seed_session_in(&mut s, "/work/b/api", Some("p2"));
        s.activity
            .insert(key.clone(), app_core::activity::Activity::Waiting);
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        assert!(attention_dot_shown(&rt), "the hidden waiting session flags");
        let selector = els(&rt)
            .into_iter()
            .find(|e| e.key.as_deref() == Some("sidebar-workspace-selector"))
            .expect("the workspace selector mounts");
        assert_eq!(
            selector.label.as_deref(),
            Some("Workspace: Work, another workspace needs you")
        );

        click_key(&mut rt, "sidebar-workspace-selector");
        assert!(entry_shown(&rt, "Default · 1 need you"), "menu suffix");
        assert!(
            !entry_shown(&rt, "Work · 1 need you"),
            "selected: no suffix"
        );
    }

    #[test]
    fn the_flag_clears_when_the_hidden_session_is_done() {
        let mut s = two_workspace_scene();
        let key = seed_session_in(&mut s, "/work/b/api", Some("p2"));
        s.activity
            .insert(key.clone(), app_core::activity::Activity::Waiting);
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        assert!(attention_dot_shown(&rt));

        rt.update(|s, _| {
            s.activity
                .insert(key.clone(), app_core::activity::Activity::Done);
        });
        rt.frame(1.0 / 60.0);
        assert!(!attention_dot_shown(&rt), "no dot once Done");
        click_key(&mut rt, "sidebar-workspace-selector");
        assert!(
            !entry_shown(&rt, "Default · 1 need you"),
            "no suffix once Done"
        );
    }

    #[test]
    fn a_waiting_session_in_the_selected_workspace_adds_no_flag() {
        let mut s = two_workspace_scene();
        let key = seed_session_in(&mut s, "/work/a/web", Some("p1"));
        s.activity
            .insert(key.clone(), app_core::activity::Activity::Waiting);
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        assert!(
            !attention_dot_shown(&rt),
            "the selected workspace shows its own"
        );
        click_key(&mut rt, "sidebar-workspace-selector");
        assert!(!entry_shown(&rt, "Default · 1 need you"));
        assert!(!entry_shown(&rt, "Work · 1 need you"));
    }

    #[test]
    fn a_moved_project_moves_its_waiting_count() {
        let mut s = two_workspace_scene();
        s.workspaces.push(workspace("w2", "Home"));
        let key = seed_session_in(&mut s, "/work/a/web", Some("p1"));
        s.activity
            .insert(key.clone(), app_core::activity::Activity::Waiting);
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        assert!(!attention_dot_shown(&rt), "p1 is in the selected Work");

        rt.update(|s, _| s.editor.projects[0].project.workspace_id = "w2".into());
        rt.frame(1.0 / 60.0);
        assert!(attention_dot_shown(&rt), "the count follows the project");
        click_key(&mut rt, "sidebar-workspace-selector");
        assert!(entry_shown(&rt, "Home · 1 need you"));
    }

    #[test]
    fn an_unassigned_waiting_session_counts_for_default_only() {
        let mut s = two_workspace_scene();
        let key = seed_session_in(&mut s, "/tmp", None);
        s.activity
            .insert(key.clone(), app_core::activity::Activity::Waiting);
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        assert!(
            attention_dot_shown(&rt),
            "Default is hidden while Work is selected"
        );
        click_key(&mut rt, "sidebar-workspace-selector");
        assert!(entry_shown(&rt, "Default · 1 need you"));

        rt.update(|s, _| s.select_workspace(app_core::store::DEFAULT_WORKSPACE_ID));
        rt.frame(1.0 / 60.0);
        assert!(!attention_dot_shown(&rt), "Default is selected: no flag");
    }

    #[test]
    fn switching_workspaces_keeps_the_editor_checkout_and_running_sessions() {
        let mut s = two_workspace_scene();
        // The editor is committed to p1's checkout inside the Work group.
        s.view = View::Editor;
        s.editor.project_id = Some("p1".into());
        s.editor.checkout = Some(PathBuf::from("/work/a/web"));
        s.editor.checkout_key = Some(crate::editor::state::checkout_key(
            "p1",
            &PathBuf::from("/work/a/web"),
        ));
        s.active_workspace = Some(crate::data::checkout_key("p1", "/work/a/web"));
        let key = seed_session(&mut s, "web", "/work/a/web");
        s.selected_session = Some(key.clone());
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        rt.update(|s, _| s.select_workspace(app_core::store::DEFAULT_WORKSPACE_ID));
        rt.frame(1.0 / 60.0);

        assert_eq!(
            rt.state.view,
            View::Editor,
            "switching never changes the view"
        );
        assert_eq!(rt.state.editor.project_id.as_deref(), Some("p1"));
        assert_eq!(
            rt.state.editor.checkout,
            Some(PathBuf::from("/work/a/web")),
            "the editor checkout stays committed"
        );
        assert_eq!(
            rt.state.active_workspace.as_deref(),
            Some(crate::data::checkout_key("p1", "/work/a/web").as_str())
        );
        assert_eq!(
            rt.state.selected_session.as_deref(),
            Some(key.as_str()),
            "the running session stays selected"
        );
        assert!(
            rt.state.data.sessions.iter().any(|v| v.info.title == "web"),
            "the session keeps running"
        );
    }

    #[test]
    fn the_workspace_selector_and_remove_confirmation_are_accessible() {
        let mut s = two_workspace_scene();
        s.workspace_remove_confirm = Some("w1".into());
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let e = els(&rt);
        let selector = e
            .iter()
            .find(|el| el.key.as_deref() == Some("sidebar-workspace-selector"))
            .expect("the workspace selector mounts");
        assert_eq!(selector.role, "button");
        assert_eq!(selector.label.as_deref(), Some("Workspace: Work"));
        assert!(selector.focusable, "the selector takes keyboard focus");
        assert!(
            selector.rect[3] <= 32.0,
            "the selector keeps one row's height: {:?}",
            selector.rect
        );
        for key in [
            "sidebar-workspace-remove-confirm",
            "sidebar-workspace-remove-cancel",
        ] {
            let button = e
                .iter()
                .find(|el| el.key.as_deref() == Some(key))
                .unwrap_or_else(|| panic!("{key} mounts"));
            assert_eq!(button.role, "button");
            assert!(button.focusable);
        }
        let texts: Vec<String> = e.iter().filter_map(|el| el.text.clone()).collect();
        assert!(
            texts.iter().any(|t| t.contains(
                "Remove Work? Its terminals and agents are closed; its projects move to Default."
            )),
            "{texts:?}"
        );

        click_key(&mut rt, "sidebar-workspace-remove-cancel");
        assert!(rt.state.workspace_remove_confirm.is_none());
        assert!(rt.state.workspaces.iter().any(|w| w.id == "w1"));
    }

    #[test]
    fn an_empty_named_workspace_shows_its_own_hint() {
        let mut s = sidebar_scene();
        s.workspaces = vec![
            workspace(app_core::store::DEFAULT_WORKSPACE_ID, "Default"),
            workspace("w2", "Empty"),
        ];
        s.selected_workspace = Some("w2".into());
        seed_session(&mut s, "fix-auth-flow", "/work/a/web");
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let e = els(&rt);
        let texts: Vec<String> = e.iter().filter_map(|el| el.text.clone()).collect();
        assert!(
            texts.iter().any(|t| t.contains("No projects in Empty")),
            "{texts:?}"
        );
        assert!(
            !e.iter()
                .any(|el| el.key.as_deref() == Some("sidebar-unassigned")),
            "Unassigned never renders outside Default"
        );
    }

    #[test]
    fn re_adding_a_registered_project_reports_its_workspace() {
        let mut s = two_workspace_scene();
        s.selected_workspace = Some(app_core::store::DEFAULT_WORKSPACE_ID.into());
        let duplicate = app_core::projects::ProjectView {
            project: app_core::store::Project {
                id: "p1".into(),
                name: "web".into(),
                root: "/work/a/web".into(),
                workspace_id: "w1".into(),
                created_at: 0,
            },
            missing: false,
        };
        s.apply_reply(backend::Reply {
            req: backend::Req::AddProject {
                path: "/work/a/web".into(),
                workspace_id: app_core::store::DEFAULT_WORKSPACE_ID.into(),
            },
            generation: 0,
            checkout_key: None,
            result: Ok(backend::ReplyData::Added(duplicate)),
        });

        let notice = s.notice.as_deref().unwrap_or_default();
        assert!(notice.contains("already registered in Work"), "{notice}");
        assert_eq!(
            s.editor.projects[0].project.workspace_id, "w1",
            "re-adding never moves the project"
        );
    }

    /// The real end-to-end fixture: two registered projects in a temp
    /// `Store`, real `git worktree` checkouts, real `worktrees::list`
    /// payloads — driven through the same sidebar → navigation → editor
    /// identity path production uses.
    #[test]
    fn sidebar_drives_editor_with_real_registered_projects_and_worktrees() {
        use std::process::Command;

        fn git_ok(root: &std::path::Path, args: &[&str]) {
            let out = Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .expect("git run");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        fn repo(base: &std::path::Path, name: &str) -> std::path::PathBuf {
            let root = base.join(name);
            std::fs::create_dir_all(&root).unwrap();
            let root = root.canonicalize().unwrap();
            git_ok(&root, &["init", "-b", "main"]);
            git_ok(&root, &["config", "user.name", "Tester"]);
            git_ok(&root, &["config", "user.email", "tester@example.com"]);
            std::fs::write(root.join("file.txt"), "x\n").unwrap();
            git_ok(&root, &["add", "-A"]);
            git_ok(&root, &["commit", "-qm", "init"]);
            root
        }

        let home = app_core::test_support::TempDir::new("rune-sidebar");
        let base = home.path().canonicalize().unwrap();
        let root_a = repo(&base, "proj-web");
        let root_b = repo(&base, "proj-api");
        // Two linked worktrees on proj-web, one on proj-api.
        git_ok(&root_a, &["worktree", "add", "../wt-a", "-b", "feat-a"]);
        git_ok(&root_a, &["worktree", "add", "../wt-b", "-b", "feat-b"]);
        git_ok(&root_b, &["worktree", "add", "../wt-c", "-b", "fix-c"]);

        // Register through the real store — the same rows Req::Projects
        // and Req::Worktrees return in production.
        let store = app_core::store::Store::open(home.path()).unwrap();
        let pv_a = app_core::projects::add_project(
            &store,
            root_a.to_str().unwrap(),
            app_core::store::DEFAULT_WORKSPACE_ID,
        )
        .unwrap();
        let pv_b = app_core::projects::add_project(
            &store,
            root_b.to_str().unwrap(),
            app_core::store::DEFAULT_WORKSPACE_ID,
        )
        .unwrap();
        let list_a = app_core::worktrees::list(&root_a).unwrap();
        let list_b = app_core::worktrees::list(&root_b).unwrap();
        assert_eq!(list_a.worktrees.len(), 3, "root + two linked on web");
        assert_eq!(list_b.worktrees.len(), 2, "root + one linked on api");

        let id_a = pv_a.project.id.clone();
        let id_b = pv_b.project.id.clone();
        let mut s = State::new(test_handle());
        s.editor.projects = vec![pv_a, pv_b];
        for (pid, list) in [(&id_a, list_a), (&id_b, list_b)] {
            s.editor.sidebar_worktrees.insert(
                pid.clone(),
                crate::editor::state::SidebarWorktrees {
                    list: Some(list),
                    loading: false,
                    error: None,
                },
            );
        }
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        // Real project names render as rows keyed by their real ids.
        let e = els(&rt);
        assert!(
            e.iter()
                .any(|el| el.key.as_deref() == Some(&format!("sidebar-project:{id_a}")))
        );
        assert!(
            e.iter()
                .any(|el| el.key.as_deref() == Some(&format!("sidebar-project:{id_b}")))
        );

        // Root row → the registered canonical root, exactly.
        click_key(&mut rt, &format!("sidebar-project:{id_a}"));
        assert_eq!(rt.state.editor.project_id.as_deref(), Some(id_a.as_str()));
        assert_eq!(
            rt.state.editor.checkout,
            Some(PathBuf::from(&root_a)),
            "root row committed the canonical registered root"
        );

        // Worktree row → the exact path `git worktree list` reported.
        let wt_path = rt.state.editor.sidebar_worktrees[&id_a]
            .list
            .as_ref()
            .unwrap()
            .worktrees
            .iter()
            .find(|w| !w.is_root)
            .unwrap()
            .path
            .clone();
        click_key(&mut rt, &format!("sidebar-worktree:{id_a}:0"));
        assert_eq!(
            rt.state.editor.checkout,
            Some(PathBuf::from(&wt_path)),
            "worktree row committed the exact listed checkout"
        );

        // Editor toggle keeps the committed identity; switching projects
        // through the sidebar while in Editor moves it wholesale.
        click_key(&mut rt, "view:Editor");
        assert_eq!(rt.state.view, View::Editor);
        assert_eq!(rt.state.editor.checkout, Some(PathBuf::from(&wt_path)));
        click_key(&mut rt, &format!("sidebar-project:{id_b}"));
        assert_eq!(rt.state.editor.project_id.as_deref(), Some(id_b.as_str()));
        assert_eq!(rt.state.editor.checkout, Some(PathBuf::from(&root_b)));
        click_key(&mut rt, "view:Terminal");
        assert_eq!(rt.state.view, View::Terminal);
        click_key(&mut rt, "view:Editor");
        assert_eq!(rt.state.editor.project_id.as_deref(), Some(id_b.as_str()));
        assert_eq!(rt.state.editor.checkout, Some(PathBuf::from(&root_b)));
    }

    /// Drain the channel-backed backend exactly like the worker thread:
    /// every accepted job runs through the real `run_job` against the real
    /// temp `Storage`, and its reply applies in FIFO order — including any
    /// jobs a reply itself enqueues (the Projects ensure pass, worktree
    /// re-reads, section refreshes).
    fn drain_backend(
        s: &mut State,
        rx: &std::sync::mpsc::Receiver<crate::editor::backend::Job>,
        storage: &Option<app_core::store::Storage>,
    ) {
        while let Ok(job) = rx.try_recv() {
            let reply = crate::editor::backend::run_job(storage, &job);
            s.editor.apply_reply(reply);
            if s.editor.section == Section::Changes {
                crate::editor::changes::ensure_selection(s);
            }
        }
    }

    /// Regression for the native QA hang: two registered projects sharing
    /// the display name "repo" whose stored roots are NON-canonical
    /// (`/var/folders/…`) while `git worktree list` reports canonical
    /// (`/private/var/folders/…`) paths — the exact DB-vs-Git divergence the
    /// QA fixture had. Selecting project B's root and then project A's
    /// worktree must commit the exact checkout and let Changes settle,
    /// driven through the real `run_job`/`apply_reply` stream rather than
    /// synthetic replies.
    #[test]
    fn cross_project_worktree_selection_settles_with_real_backend_replies() {
        use std::process::Command;

        fn git_ok(root: &std::path::Path, args: &[&str]) {
            let out = Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .expect("git run");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        fn repo(base: &std::path::Path, rel: &str) -> PathBuf {
            let root = base.join(rel);
            std::fs::create_dir_all(&root).unwrap();
            git_ok(&root, &["init", "-b", "main"]);
            git_ok(&root, &["config", "user.name", "Tester"]);
            git_ok(&root, &["config", "user.email", "tester@example.com"]);
            std::fs::write(root.join("file.txt"), "x\n").unwrap();
            git_ok(&root, &["add", "-A"]);
            git_ok(&root, &["commit", "-qm", "init"]);
            root // deliberately NOT canonicalized: the QA DB kept /var/…
        }

        let home = app_core::test_support::TempDir::new("rune-qa-repro");
        let base = home.path().join("repos");
        std::fs::create_dir_all(&base).unwrap();
        let root_a = repo(&base, "repo");
        let root_b = repo(&base, "other/repo");
        git_ok(
            &root_a,
            &[
                "worktree",
                "add",
                &base.join("wt-a").display().to_string(),
                "-b",
                "wt-a",
            ],
        );
        git_ok(
            &root_a,
            &[
                "worktree",
                "add",
                &base.join("nested/wt-a2").display().to_string(),
                "-b",
                "wt-a2",
            ],
        );
        git_ok(
            &root_b,
            &[
                "worktree",
                "add",
                &base.join("other/wt-b").display().to_string(),
                "-b",
                "wt-b",
            ],
        );

        // Register VERBATIM non-canonical roots under the same display
        // name — `Store::add_project` stores the string it is given, which
        // is how the QA database ended up with /var/… roots.
        let storage = app_core::store::Storage::open(home.path());
        let store = storage.get().unwrap();
        let pa = store
            .add_project(
                "repo",
                root_a.to_str().unwrap(),
                app_core::store::DEFAULT_WORKSPACE_ID,
            )
            .unwrap();
        let pb = store
            .add_project(
                "repo",
                root_b.to_str().unwrap(),
                app_core::store::DEFAULT_WORKSPACE_ID,
            )
            .unwrap();
        assert_ne!(pa.id, pb.id);
        let storage = Some(storage);

        let (backend, rx) = crate::editor::backend::channel_backend();
        let mut s = State::new(test_handle());
        s.editor.backend = backend;
        s.view = View::Editor;

        // Startup project load + the per-project worktree ensure pass.
        s.editor.refresh_projects();
        drain_backend(&mut s, &rx, &storage);
        assert_eq!(s.editor.projects.len(), 2);
        assert!(
            s.editor
                .sidebar_worktrees
                .get(&pa.id)
                .and_then(|e| e.list.as_ref())
                .is_some_and(|l| l.worktrees.iter().any(|w| !w.is_root))
        );

        // The worktree row's checkout is the path `git worktree list`
        // reported, verbatim — exactly what sidebar.rs sends.
        let wt_path = s.editor.sidebar_worktrees[&pa.id]
            .list
            .as_ref()
            .unwrap()
            .worktrees
            .iter()
            .find(|w| !w.is_root && w.path.ends_with("wt-a"))
            .expect("linked worktree listed")
            .path
            .clone();

        // QA step A: sidebar project-B root row while in Terminal→Editor.
        crate::editor::navigation::navigate(
            &mut s,
            PendingNav::SelectCheckout {
                project_id: pb.id.clone(),
                checkout: PathBuf::from(&pb.root),
            },
            &mut rune::Tx::default(),
        );
        drain_backend(&mut s, &rx, &storage);
        assert_eq!(s.editor.project_id.as_deref(), Some(pb.id.as_str()));
        assert_eq!(
            s.editor.checkout.as_deref(),
            Some(std::path::Path::new(&pb.root)),
        );
        assert!(
            s.editor.changes.data.is_some(),
            "root checkout changes loaded"
        );
        assert!(!s.editor.changes.loading);

        // QA step B: while in Editor, the sidebar worktree row of project A.
        crate::editor::navigation::navigate(
            &mut s,
            PendingNav::SelectCheckout {
                project_id: pa.id.clone(),
                checkout: PathBuf::from(&wt_path),
            },
            &mut rune::Tx::default(),
        );
        drain_backend(&mut s, &rx, &storage);

        // The committed identity is the exact listed checkout…
        assert_eq!(s.editor.project_id.as_deref(), Some(pa.id.as_str()));
        assert_eq!(
            s.editor.checkout.as_deref(),
            Some(std::path::Path::new(&wt_path)),
            "worktree selection committed the exact listed path"
        );
        // …the picker has real options for it (not stuck on the placeholder)…
        assert!(
            !s.editor.worktrees.worktrees.is_empty(),
            "checkout picker populated for the selected project"
        );
        assert!(!s.editor.worktrees_loading);
        assert!(s.editor.worktrees_error.is_none());
        // …and Changes settled on the new checkout instead of spinning.
        assert!(
            s.editor.changes.data.is_some(),
            "changes loaded for the worktree checkout"
        );
        assert!(!s.editor.changes.loading, "changes loading settled");
        assert_eq!(
            s.editor.changes.checkout.as_deref(),
            s.editor.checkout_key.as_deref(),
            "changes data belongs to the committed checkout key"
        );
        // The sidebar highlight tracks the same committed identity.
        assert_eq!(
            s.active_workspace.as_deref(),
            Some(crate::data::checkout_key(&pa.id, &wt_path).as_str())
        );
    }

    // -- usage / accounts / theme replies ------------------------------------

    use crate::editor::backend::{Reply, ReplyData, Req};

    fn usage_view() -> app_core::accounts::AccountsView {
        let view =
            |active: &str, items: &[(&str, &str, &str, bool)]| app_core::accounts::AgentView {
                active: active.to_string(),
                items: items
                    .iter()
                    .map(
                        |(id, name, dir, is_default)| app_core::accounts::AccountView {
                            id: id.to_string(),
                            name: name.to_string(),
                            dir: dir.to_string(),
                            is_default: *is_default,
                        },
                    )
                    .collect(),
            };
        app_core::accounts::AccountsView {
            claude: view(
                "default",
                &[
                    ("default", "Default", "/me/.claude", true),
                    ("aa11bb22", "work", "/accts/work", false),
                ],
            ),
            codex: view("default", &[("default", "Default", "/me/.codex", true)]),
        }
    }

    fn reply(req: Req, result: Result<ReplyData, String>) -> Reply {
        Reply {
            req,
            generation: 0,
            checkout_key: None,
            result,
        }
    }

    // -- Task 4: removal confirmations ---------------------------------------

    /// Press the menu item (or any element) carrying this label.
    fn click_label(rt: &mut Runtime<State>, label: &str) {
        let e = rt
            .elements()
            .into_iter()
            .find(|e| e.label.as_deref() == Some(label))
            .unwrap_or_else(|| panic!("{label} is on screen"));
        let (x, y) = (e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    fn texts_on(rt: &Runtime<State>) -> Vec<String> {
        els(rt).into_iter().filter_map(|e| e.text).collect()
    }

    #[test]
    fn opening_a_worktree_removal_requests_its_projects_worktrees() {
        let mut s = sidebar_scene();
        let (backend, rx) = crate::editor::backend::channel_backend();
        s.editor.backend = backend;
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        click_key(&mut rt, "sidebar-action:more:/work/a/feat-a");
        click_label(&mut rt, "Remove worktree");
        let reqs = workspace_jobs(&rx);
        assert!(
            reqs.iter()
                .any(|req| matches!(req, Req::Worktrees(pid) if pid == "p1")),
            "{reqs:?}"
        );
    }

    #[test]
    fn the_confirmation_shows_the_live_dirty_state_not_the_captured_one() {
        let mut s = sidebar_scene();
        s.editor
            .sidebar_worktrees
            .get_mut("p1")
            .unwrap()
            .list
            .as_mut()
            .unwrap()
            .worktrees
            .iter_mut()
            .find(|w| w.path == "/work/a/feat-a")
            .unwrap()
            .dirty = Some(true);
        s.sidebar_confirm = Some(sidebar::SidebarConfirm::RemoveWorktree {
            project_id: "p1".into(),
            path: "/work/a/feat-a".into(),
            name: "feat-a".into(),
            dirty: Some(false),
            sessions: 0,
        });
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        assert!(
            texts_on(&rt)
                .iter()
                .any(|t| t.starts_with("It has uncommitted changes.")),
            "{:?}",
            texts_on(&rt)
        );
    }

    #[test]
    fn an_ended_session_is_not_counted_in_the_removal_confirmation() {
        use protocol::session::{SessionId, SessionInfo, SessionState};

        let mut s = sidebar_scene();
        for state in [SessionState::Exited, SessionState::Running] {
            s.upsert_session(SessionInfo {
                id: SessionId::new(),
                title: "zsh".into(),
                argv: vec!["sh".into()],
                cwd: "/work/a/feat-a".into(),
                state,
                pid: None,
                exit_code: None,
                error: None,
                started_at: 0,
                ended_at: None,
            });
        }
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        click_key(&mut rt, "sidebar-action:more:/work/a/feat-a");
        click_label(&mut rt, "Remove worktree");

        assert!(
            texts_on(&rt)
                .iter()
                .any(|t| t.contains("1 session runs in it.")),
            "{:?}",
            texts_on(&rt)
        );
    }

    #[test]
    fn remove_worktree_waits_for_confirm_then_enqueues_once() {
        let mut s = sidebar_scene();
        let (backend, rx) = crate::editor::backend::channel_backend();
        s.editor.backend = backend;
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        click_key(&mut rt, "sidebar-action:more:/work/a/feat-a");
        click_label(&mut rt, "Remove worktree");
        assert!(
            !workspace_jobs(&rx)
                .iter()
                .any(|req| matches!(req, Req::RemoveWorktree { .. })),
            "nothing is removed before Confirm"
        );
        assert!(matches!(
            &rt.state.sidebar_confirm,
            Some(sidebar::SidebarConfirm::RemoveWorktree { path, .. }) if path == "/work/a/feat-a"
        ));

        click_key(&mut rt, "sidebar-confirm-remove");
        let reqs: Vec<Req> = workspace_jobs(&rx)
            .into_iter()
            .filter(|req| matches!(req, Req::RemoveWorktree { .. }))
            .collect();
        assert_eq!(reqs.len(), 1, "Confirm enqueues the removal once");
        assert!(matches!(
            &reqs[0],
            Req::RemoveWorktree { project_id, path }
                if project_id == "p1" && path == "/work/a/feat-a"
        ));
        assert!(rt.state.sidebar_confirm.is_none());
    }

    #[test]
    fn cancel_on_a_worktree_removal_enqueues_nothing() {
        let mut s = sidebar_scene();
        let (backend, rx) = crate::editor::backend::channel_backend();
        s.editor.backend = backend;
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        click_key(&mut rt, "sidebar-action:more:/work/a/feat-a");
        click_label(&mut rt, "Remove worktree");
        click_key(&mut rt, "sidebar-confirm-cancel");
        assert!(rt.state.sidebar_confirm.is_none());
        assert!(
            !workspace_jobs(&rx)
                .iter()
                .any(|req| matches!(req, Req::RemoveWorktree { .. })),
            "cancel removes nothing"
        );
    }

    #[test]
    fn a_dirty_worktree_says_it_has_uncommitted_changes() {
        let mut s = sidebar_scene();
        let list = s.editor.sidebar_worktrees.get_mut("p1").unwrap();
        list.list.as_mut().unwrap().worktrees[1].dirty = Some(true);
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        click_key(&mut rt, "sidebar-action:more:/work/a/feat-a");
        click_label(&mut rt, "Remove worktree");
        let texts = texts_on(&rt);
        assert!(
            texts
                .iter()
                .any(|t| t.contains("It has uncommitted changes. No sessions run in it.")),
            "{texts:?}"
        );
        assert!(texts.iter().any(|t| t == "Remove feat-a?"), "{texts:?}");
    }

    #[test]
    fn a_clean_or_unknown_worktree_says_so() {
        let mut s = sidebar_scene();
        let list = s.editor.sidebar_worktrees.get_mut("p1").unwrap();
        list.list.as_mut().unwrap().worktrees[1].dirty = Some(false);
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        click_key(&mut rt, "sidebar-action:more:/work/a/feat-a");
        click_label(&mut rt, "Remove worktree");
        assert!(
            texts_on(&rt)
                .iter()
                .any(|t| t.contains("It has no uncommitted changes.")),
        );

        rt.state.sidebar_confirm = None;
        rt.state
            .editor
            .sidebar_worktrees
            .get_mut("p1")
            .unwrap()
            .list
            .as_mut()
            .unwrap()
            .worktrees[1]
            .dirty = None;
        click_key(&mut rt, "sidebar-action:more:/work/a/feat-a");
        click_label(&mut rt, "Remove worktree");
        assert!(
            texts_on(&rt)
                .iter()
                .any(|t| t.contains("Git status unknown.")),
        );
    }

    #[test]
    fn two_attached_sessions_are_counted_in_the_worktree_row() {
        let mut s = sidebar_scene();
        seed_session(&mut s, "one", "/work/a/feat-a");
        seed_session(&mut s, "two", "/work/a/feat-a");
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        click_key(&mut rt, "sidebar-action:more:/work/a/feat-a");
        click_label(&mut rt, "Remove worktree");
        assert!(
            texts_on(&rt)
                .iter()
                .any(|t| t.contains("Git status unknown. 2 sessions run in it.")),
            "{:?}",
            texts_on(&rt)
        );
    }

    #[test]
    fn a_vanished_worktree_shows_the_notice_and_enqueues_nothing() {
        let mut s = sidebar_scene();
        let (backend, rx) = crate::editor::backend::channel_backend();
        s.editor.backend = backend;
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        click_key(&mut rt, "sidebar-action:more:/work/a/feat-a");
        click_label(&mut rt, "Remove worktree");

        rt.state
            .editor
            .sidebar_worktrees
            .get_mut("p1")
            .unwrap()
            .list
            .as_mut()
            .unwrap()
            .worktrees
            .retain(|w| w.path != "/work/a/feat-a");
        rt.frame(1.0 / 60.0);
        click_key(&mut rt, "sidebar-confirm-remove");

        assert_eq!(
            rt.state.notice.as_deref(),
            Some("That item no longer exists")
        );
        assert!(rt.state.sidebar_confirm.is_none());
        assert!(
            !workspace_jobs(&rx)
                .iter()
                .any(|req| matches!(req, Req::RemoveWorktree { .. })),
            "nothing is removed"
        );
    }

    #[test]
    fn opening_a_removal_closes_the_workspace_confirmation_and_back() {
        let mut s = two_workspace_scene();
        s.workspace_remove_confirm = Some("w1".into());
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        click_key(&mut rt, "sidebar-action:more:/work/a/feat-a");
        click_label(&mut rt, "Remove worktree");
        assert!(rt.state.workspace_remove_confirm.is_none());
        assert!(rt.state.sidebar_confirm.is_some());

        click_key(&mut rt, "sidebar-workspace-selector");
        assert!(rt.state.workspace_picker.open, "the workspace picker opens");
        // Let the surface finish opening before pressing one of its options.
        rt.frame(0.5);
        click_label(&mut rt, "Remove workspace…");
        assert!(
            !rt.state.workspace_picker.open,
            "choosing closes the picker"
        );
        assert_eq!(rt.state.workspace_remove_confirm.as_deref(), Some("w1"));
        assert!(rt.state.sidebar_confirm.is_none());
    }

    #[test]
    fn remove_project_waits_for_confirm_then_enqueues_once() {
        let mut s = sidebar_scene();
        let (backend, rx) = crate::editor::backend::channel_backend();
        s.editor.backend = backend;
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        click_key(&mut rt, "sidebar-action:more:p1");
        click_label(&mut rt, "Remove project");
        assert!(
            workspace_jobs(&rx).is_empty(),
            "nothing runs before Confirm"
        );
        assert!(
            texts_on(&rt).iter().any(|t| t
                == "Remove web? Only its registration is removed; its sessions become Unassigned and its files stay."),
            "{:?}",
            texts_on(&rt)
        );

        click_key(&mut rt, "sidebar-confirm-remove");
        let reqs = workspace_jobs(&rx);
        assert_eq!(reqs.len(), 1, "Confirm enqueues once");
        assert!(matches!(&reqs[0], Req::RemoveProject { id } if id == "p1"));
        assert!(rt.state.sidebar_confirm.is_none());
    }

    #[test]
    fn cancel_on_a_project_removal_enqueues_nothing() {
        let mut s = sidebar_scene();
        let (backend, rx) = crate::editor::backend::channel_backend();
        s.editor.backend = backend;
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        click_key(&mut rt, "sidebar-action:more:p1");
        click_label(&mut rt, "Remove project");
        click_key(&mut rt, "sidebar-confirm-cancel");
        assert!(rt.state.sidebar_confirm.is_none());
        assert!(workspace_jobs(&rx).is_empty());
    }

    #[test]
    fn a_vanished_project_shows_the_notice_and_enqueues_nothing() {
        let mut s = sidebar_scene();
        let (backend, rx) = crate::editor::backend::channel_backend();
        s.editor.backend = backend;
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        click_key(&mut rt, "sidebar-action:more:p1");
        click_label(&mut rt, "Remove project");

        rt.state.editor.projects.retain(|p| p.project.id != "p1");
        rt.frame(1.0 / 60.0);
        click_key(&mut rt, "sidebar-confirm-remove");

        assert_eq!(
            rt.state.notice.as_deref(),
            Some("That item no longer exists")
        );
        assert!(rt.state.sidebar_confirm.is_none());
        assert!(workspace_jobs(&rx).is_empty());
    }

    #[test]
    fn a_failed_worktree_removal_sets_the_notice_and_keeps_the_list() {
        let mut s = sidebar_scene();
        let before = s.editor.sidebar_worktrees.get("p1").unwrap().list.clone();
        s.apply_reply(reply(
            Req::RemoveWorktree {
                project_id: "p1".into(),
                path: "/work/a/feat-a".into(),
            },
            Err("worktree has local changes".into()),
        ));

        assert_eq!(s.notice.as_deref(), Some("worktree has local changes"));
        assert_eq!(s.editor.sidebar_worktrees.get("p1").unwrap().list, before);
    }

    #[test]
    fn a_failed_project_removal_sets_the_notice_and_keeps_the_projects() {
        let mut s = sidebar_scene();
        let before = s.editor.projects.len();
        s.apply_reply(reply(
            Req::RemoveProject { id: "p1".into() },
            Err("project is locked".into()),
        ));

        assert_eq!(s.notice.as_deref(), Some("project is locked"));
        assert_eq!(s.editor.projects.len(), before);
        assert!(s.editor.projects.iter().any(|p| p.project.id == "p1"));
    }

    #[test]
    fn a_refused_worktree_removal_sets_the_notice() {
        let mut s = sidebar_scene();
        let (backend, rx) = crate::editor::backend::channel_backend();
        drop(rx);
        s.editor.backend = backend;
        s.remove_worktree("p1", "/work/a/feat-a");

        assert_eq!(
            s.notice.as_deref(),
            Some("the editor queue is full; try again")
        );
    }

    #[test]
    fn a_refused_project_removal_sets_the_notice() {
        let mut s = sidebar_scene();
        let (backend, rx) = crate::editor::backend::channel_backend();
        drop(rx);
        s.editor.backend = backend;
        s.remove_project("p1");

        assert_eq!(
            s.notice.as_deref(),
            Some("the editor queue is full; try again")
        );
    }

    // -- Task 8: prune missing worktrees -------------------------------------

    fn with_missing_feat_a(mut s: State) -> State {
        let list = s.editor.sidebar_worktrees.get_mut("p1").unwrap();
        let worktrees = &mut list.list.as_mut().unwrap().worktrees;
        worktrees
            .iter_mut()
            .find(|w| w.path == "/work/a/feat-a")
            .unwrap()
            .missing = true;
        s
    }

    #[test]
    fn prune_is_absent_from_the_more_menu_without_a_missing_worktree() {
        let mut rt = Runtime::new(sidebar_scene(), view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        click_key(&mut rt, "sidebar-action:more:p1");
        assert!(!texts_on(&rt).iter().any(|t| t == "Prune missing worktrees"));
    }

    #[test]
    fn prune_is_offered_on_a_project_and_a_worktree_row_with_a_missing_worktree() {
        let s = with_missing_feat_a(sidebar_scene());
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        click_key(&mut rt, "sidebar-action:more:p1");
        assert!(texts_on(&rt).iter().any(|t| t == "Prune missing worktrees"));
        rt.update(|s, _| s.sidebar_menu = None);
        // The menu drops under its button, over the rows below: let it finish
        // closing before pressing the next row's button.
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }

        click_key(&mut rt, "sidebar-action:more:/work/a/feat-b");
        assert!(texts_on(&rt).iter().any(|t| t == "Prune missing worktrees"));
    }

    #[test]
    fn choosing_prune_enqueues_once_and_closes_the_menu() {
        let mut s = with_missing_feat_a(sidebar_scene());
        let (backend, rx) = crate::editor::backend::channel_backend();
        s.editor.backend = backend;
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        click_key(&mut rt, "sidebar-action:more:p1");
        click_label(&mut rt, "Prune missing worktrees");

        let reqs = workspace_jobs(&rx);
        assert_eq!(reqs.len(), 1, "one prune request");
        assert!(matches!(&reqs[0], Req::PruneWorktrees { project_id } if project_id == "p1"));
        assert!(rt.state.sidebar_menu.is_none());
    }

    #[test]
    fn a_failed_prune_sets_the_notice_and_keeps_the_list() {
        let mut s = with_missing_feat_a(sidebar_scene());
        let before = s.editor.sidebar_worktrees.get("p1").unwrap().list.clone();
        s.apply_reply(reply(
            Req::PruneWorktrees {
                project_id: "p1".into(),
            },
            Err("not a git repository".into()),
        ));

        assert_eq!(s.notice.as_deref(), Some("not a git repository"));
        assert_eq!(s.editor.sidebar_worktrees.get("p1").unwrap().list, before);
    }

    #[test]
    fn a_successful_prune_requests_the_worktree_list_again() {
        let mut s = with_missing_feat_a(sidebar_scene());
        let (backend, rx) = crate::editor::backend::channel_backend();
        s.editor.backend = backend;
        s.apply_reply(reply(
            Req::PruneWorktrees {
                project_id: "p1".into(),
            },
            Ok(backend::ReplyData::Ok),
        ));

        assert!(s.notice.is_none());
        let reqs = workspace_jobs(&rx);
        assert!(
            reqs.iter()
                .any(|req| matches!(req, Req::Worktrees(pid) if pid == "p1")),
            "{reqs:?}"
        );
    }

    // -- Task 12: workspace mutations ----------------------------------------

    fn workspace_jobs(rx: &std::sync::mpsc::Receiver<backend::Job>) -> Vec<Req> {
        rx.try_iter().map(|job| job.req).collect()
    }

    // -- Task 5: start buttons, New tab menu, unavailable agents ------------

    fn agent(
        id: &str,
        label: &str,
        binary: Option<&str>,
        available: bool,
    ) -> crate::data::AgentInfo {
        crate::data::AgentInfo {
            id: id.into(),
            label: label.into(),
            available,
            path: available.then(|| format!("/usr/local/bin/{id}")),
            binary: binary.map(str::to_string),
        }
    }

    /// No tab open, two registered projects in Default, Claude and OpenCode
    /// available, Codex missing from PATH.
    fn start_scene() -> State {
        let mut s = State::new(test_handle());
        s.editor.projects = vec![
            project("p1", "web", "/work/a/web"),
            project("p2", "api", "/work/b/api"),
        ];
        s.data.connected = true;
        s.data.agents = vec![
            agent("shell", "Shell", None, true),
            agent("claude", "Claude Code", Some("claude"), true),
            agent("codex", "Codex", Some("codex"), false),
            agent("opencode", "OpenCode", Some("opencode"), true),
        ];
        s
    }

    fn element_with(rt: &Runtime<State>, role: &str, label: &str) -> Option<rune::ElementInfo> {
        rt.elements()
            .into_iter()
            .find(|e| e.role == role && e.label.as_deref() == Some(label))
    }

    /// Press the element with this role and label (a menu item or a button
    /// whose label another element shares).
    fn click_role(rt: &mut Runtime<State>, role: &str, label: &str) {
        let e = element_with(rt, role, label).unwrap_or_else(|| panic!("{label} is on screen"));
        let (x, y) = (e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn an_empty_terminal_offers_each_available_agent_and_shell() {
        let mut rt = Runtime::new(start_scene(), view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        for label in ["Claude Code", "OpenCode", "Shell"] {
            assert!(
                element_with(&rt, "button", label).is_some(),
                "{label} button"
            );
        }
        assert!(
            element_with(&rt, "button", "Codex").is_none(),
            "an unavailable agent gets no start button"
        );
    }

    #[test]
    fn a_workspace_without_projects_offers_only_add_project() {
        let mut s = start_scene();
        for project in &mut s.editor.projects {
            project.project.workspace_id = "elsewhere".into();
        }
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        assert!(
            els(&rt)
                .iter()
                .any(|e| e.key.as_deref() == Some("terminal-add-project")),
            "Add Project is offered"
        );
        assert!(element_with(&rt, "button", "Claude Code").is_none());
        assert!(element_with(&rt, "button", "Shell").is_none());
    }

    #[test]
    fn disconnected_start_buttons_are_disabled_with_the_reason() {
        let mut s = start_scene();
        s.data.connected = false;
        let (backend, rx) = crate::editor::backend::channel_backend();
        s.editor.backend = backend;
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        assert!(
            texts_on(&rt)
                .iter()
                .any(|t| t == "Waiting for the Goko daemon"),
            "the start row says why it is disabled"
        );
        click_role(&mut rt, "button", "Claude Code");
        click_role(&mut rt, "button", "Shell");
        assert!(workspace_jobs(&rx).is_empty());
        assert!(rt.state.pending_argv.is_empty());
        assert!(rt.state.pending.is_empty());
    }

    /// Steps a picker or menu through its open (or close) animation.
    fn settle(rt: &mut Runtime<State>) {
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
    }

    /// Every "start something" button opens the same picker, and an agent
    /// missing from PATH is listed disabled with the reason: picking it starts
    /// nothing.
    #[test]
    fn an_unavailable_agent_says_why_in_every_new_session_picker() {
        let reason = "codex not found on PATH";
        for trigger in ["new-tab-menu", "sidebar-action:new-session:p1"] {
            let (backend, rx) = crate::editor::backend::channel_backend();
            let mut s = start_scene();
            s.editor.backend = backend;
            let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
            rt.frame(0.0);
            click_key(&mut rt, trigger);
            settle(&mut rt);
            assert!(
                rt.state.new_session_picker.open,
                "{trigger} opens the picker"
            );
            assert!(
                texts_on(&rt).iter().any(|t| t == reason),
                "{trigger}: the reason is shown"
            );
            click_role(&mut rt, "option", "Codex");
            assert!(
                workspace_jobs(&rx).is_empty(),
                "{trigger}: a disabled entry starts nothing"
            );
        }
    }

    #[test]
    fn new_tab_still_starts_one_shell() {
        let mut rt = Runtime::new(start_scene(), view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        click_label(&mut rt, "New tab");
        assert_eq!(rt.state.pending.len(), 1, "one session.create");
        assert!(matches!(
            &rt.state.pending[0],
            crate::sessions::Pending::Create { launch, .. } if launch.agent == "shell"
        ));
        assert!(rt.state.pending_argv.is_empty());
    }

    // -- Task 6: app shortcuts ----------------------------------------------

    fn cmd_key(c: char) -> KeyEvent {
        KeyEvent {
            key: Key::Command(c),
            mods: Mods {
                cmd: true,
                ..Mods::default()
            },
        }
    }

    /// The window's runtime, with the app's key handler attached.
    fn key_runtime(s: State) -> Runtime<State> {
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.on_key = Some(on_key);
        rt.frame(0.0);
        rt
    }

    fn press_cmd(rt: &mut Runtime<State>, c: char) {
        rt.key(cmd_key(c));
        rt.frame(1.0 / 60.0);
    }

    fn alt_key(key: Key) -> KeyEvent {
        KeyEvent {
            key,
            mods: Mods {
                alt: true,
                ..Mods::default()
            },
        }
    }

    fn press_alt(rt: &mut Runtime<State>, key: Key) {
        rt.key(alt_key(key));
        rt.frame(1.0 / 60.0);
    }

    /// Two open tabs (one session each) in the Terminal view; the second is active.
    fn two_tab_scene() -> State {
        let mut s = State::new(test_handle());
        s.view = View::Terminal;
        let first = seed_session(&mut s, "one", "/tmp");
        let second = seed_session(&mut s, "two", "/tmp");
        s.focus_session_tab(&first);
        s.focus_session_tab(&second);
        s
    }

    #[test]
    fn cmd_t_starts_exactly_one_shell() {
        let mut rt = key_runtime(start_scene());
        press_cmd(&mut rt, 't');
        assert_eq!(rt.state.pending.len(), 1, "one session.create");
        assert!(matches!(
            &rt.state.pending[0],
            crate::sessions::Pending::Create { launch, .. } if launch.agent == "shell"
        ));
    }

    #[test]
    fn cmd_digits_activate_the_nth_tab_and_a_missing_tab_does_nothing() {
        let mut rt = key_runtime(two_tab_scene());
        press_cmd(&mut rt, '1');
        assert_eq!(rt.state.active_tab, 0);
        press_cmd(&mut rt, '2');
        assert_eq!(rt.state.active_tab, 1);
        press_cmd(&mut rt, '9');
        assert_eq!(rt.state.active_tab, 1, "no ninth tab");
        assert_eq!(rt.state.view, View::Terminal);
    }

    #[test]
    fn cmd_digit_in_the_editor_view_switches_only_when_the_tab_exists() {
        let mut rt = key_runtime(two_tab_scene());
        rt.update(|s, _| s.view = View::Editor);
        press_cmd(&mut rt, '9');
        assert_eq!(rt.state.view, View::Editor, "no ninth tab");
        press_cmd(&mut rt, '2');
        assert_eq!(rt.state.view, View::Terminal);
        assert_eq!(rt.state.active_tab, 1);
    }

    #[test]
    fn cmd_comma_opens_then_closes_settings() {
        let mut rt = key_runtime(editor_scene());
        press_cmd(&mut rt, ',');
        assert_eq!(rt.state.view, View::Settings);
        press_cmd(&mut rt, ',');
        assert_eq!(rt.state.view, View::Terminal);
    }

    #[test]
    fn cmd_comma_with_a_dirty_draft_raises_the_guard_instead() {
        let mut rt = key_runtime(dirty_scene());
        press_cmd(&mut rt, ',');
        assert!(rt.state.editor.guard.is_some(), "the draft guard is up");
        assert_eq!(rt.state.view, View::Editor, "the view did not switch");
    }

    #[test]
    fn shortcuts_do_not_fire_under_a_confirmation_or_a_composition() {
        let mut s = two_tab_scene();
        s.sidebar_confirm = Some(crate::sidebar::SidebarConfirm::RemoveWorktree {
            project_id: "p1".into(),
            path: "/work/a/web".into(),
            name: "web".into(),
            dirty: Some(false),
            sessions: 0,
        });
        let mut rt = key_runtime(s);
        press_cmd(&mut rt, 't');
        press_cmd(&mut rt, '1');
        press_cmd(&mut rt, ',');
        assert!(rt.state.pending.is_empty(), "no session starts");
        assert_eq!(rt.state.active_tab, 1, "no tab switch");
        assert_eq!(rt.state.view, View::Terminal, "no settings toggle");

        rt.update(|s, _| {
            s.sidebar_confirm = None;
            s.preedit = "vie".into();
        });
        press_cmd(&mut rt, 't');
        press_cmd(&mut rt, '1');
        press_cmd(&mut rt, ',');
        assert!(
            rt.state.pending.is_empty(),
            "no session starts while composing"
        );
        assert_eq!(rt.state.active_tab, 1);
        assert_eq!(rt.state.view, View::Terminal);
    }

    #[test]
    fn cmd_t_is_consumed_by_the_app_before_the_terminal_sees_it() {
        let mut rt = key_runtime(two_tab_scene());
        rt.update(|s, tx| {
            assert!(on_key(s, &cmd_key('t'), tx), "the app takes ⌘T");
            assert!(
                !terminal_key(s, &cmd_key('t'), None),
                "the terminal never types a ⌘ chord"
            );
        });
    }

    #[test]
    fn shortcuts_wait_behind_the_pickers_and_the_sidebar_menu() {
        type Block = fn(&mut State);
        let blocks: [(&str, Block); 3] = [
            ("the checkout picker", |s| {
                s.editor.checkout_picker.open = true
            }),
            ("the project picker", |s| {
                s.editor.project_picker.open = true
            }),
            ("the sidebar menu", |s| {
                s.open_menu(
                    crate::sidebar::RowTarget::Project {
                        project_id: "p1".into(),
                    },
                    crate::sidebar::MenuKind::More,
                );
            }),
        ];
        for (what, block) in blocks {
            let mut s = two_tab_scene();
            block(&mut s);
            let mut rt = key_runtime(s);
            press_cmd(&mut rt, 't');
            press_cmd(&mut rt, '1');
            press_cmd(&mut rt, ',');
            assert!(
                rt.state.pending.is_empty(),
                "no session starts under {what}"
            );
            assert_eq!(rt.state.active_tab, 1, "no tab switch under {what}");
            assert_eq!(rt.state.view, View::Terminal, "no Settings under {what}");
        }
    }

    #[test]
    fn shifted_and_option_chords_are_not_app_shortcuts() {
        let mut rt = key_runtime(start_scene());
        let view_before = rt.state.view;
        for mods in [
            Mods {
                cmd: true,
                shift: true,
                ..Mods::default()
            },
            Mods {
                cmd: true,
                alt: true,
                ..Mods::default()
            },
        ] {
            rt.key(KeyEvent {
                key: Key::Command('t'),
                mods,
            });
            rt.frame(1.0 / 60.0);
        }
        rt.key(KeyEvent {
            key: Key::Command(','),
            mods: Mods {
                cmd: true,
                shift: true,
                ..Mods::default()
            },
        });
        rt.frame(1.0 / 60.0);

        assert!(rt.state.pending.is_empty(), "⇧⌘T and ⌥⌘T start no shell");
        assert_eq!(rt.state.view, view_before, "⇧⌘, does not toggle Settings");
    }

    #[test]
    fn alt_digits_activate_the_nth_tab_like_cmd_digits() {
        let mut rt = key_runtime(two_tab_scene());
        press_alt(&mut rt, Key::Text("1".into()));
        assert_eq!(rt.state.active_tab, 0);
        press_alt(&mut rt, Key::Text("2".into()));
        assert_eq!(rt.state.active_tab, 1);
        press_alt(&mut rt, Key::Text("9".into()));
        assert_eq!(rt.state.active_tab, 1, "no ninth tab");
        assert_eq!(rt.state.view, View::Terminal);
    }

    #[test]
    fn alt_arrows_cycle_tabs_and_wrap_around() {
        let mut rt = key_runtime(two_tab_scene());
        // two_tab_scene leaves the second tab active.
        press_alt(&mut rt, Key::Left);
        assert_eq!(rt.state.active_tab, 0);
        press_alt(&mut rt, Key::Left);
        assert_eq!(rt.state.active_tab, 1, "wraps to the last tab");
        press_alt(&mut rt, Key::Right);
        assert_eq!(rt.state.active_tab, 0, "wraps to the first tab");
        press_alt(&mut rt, Key::Right);
        assert_eq!(rt.state.active_tab, 1);
    }

    #[test]
    fn alt_arrows_with_a_single_tab_fall_through_to_the_shell() {
        let mut s = start_scene();
        let session = seed_session(&mut s, "one", "/tmp");
        s.focus_session_tab(&session);
        let mut rt = key_runtime(s);
        rt.update(|s, tx| {
            assert!(
                !alt_shortcut(s, &alt_key(Key::Left), tx),
                "one tab: nothing to cycle, word motion reaches the shell"
            );
            assert!(!alt_shortcut(s, &alt_key(Key::Right), tx));
        });
        assert_eq!(rt.state.active_tab, 0);
    }

    #[test]
    fn alt_up_down_walks_the_sidebar_rows_in_render_order() {
        let mut s = sidebar_scene();
        s.view = View::Terminal;
        let root_sess = seed_session_in(&mut s, "/work/a/web", Some("p1"));
        let wt_sess = seed_session_in(&mut s, "/work/a/feat-a", Some("p1"));
        let p2_sess = seed_session_in(&mut s, "/work/b/api", Some("p2"));
        let loose = seed_session(&mut s, "loose", "/tmp");
        let mut rt = key_runtime(s);

        // The walk's list is the sidebar's own order: project, its root
        // sessions, the worktree group and each worktree's sessions, then
        // the next project — Unassigned last while no project is active.
        let keys: Vec<String> = crate::sidebar::rows(&rt.state)
            .iter()
            .map(|row| row.key.clone())
            .collect();
        assert_eq!(
            keys,
            vec![
                "sidebar-project:p1".to_string(),
                format!("sidebar-session:{root_sess}"),
                "sidebar-wtgroup:p1".to_string(),
                "sidebar-worktree:p1:0".to_string(),
                format!("sidebar-session:{wt_sess}"),
                "sidebar-worktree:p1:1".to_string(),
                "sidebar-project:p2".to_string(),
                format!("sidebar-session:{p2_sess}"),
                "sidebar-wtgroup:p2".to_string(),
                "sidebar-worktree:p2:0".to_string(),
                "sidebar-unassigned".to_string(),
                format!("sidebar-session:{loose}"),
            ]
        );

        // First press lands on the first row: p1's project row selects its
        // root checkout through the same guarded path a click takes — and
        // activating a project hides the Unassigned group, so the list
        // shrinks under the cursor.
        press_alt(&mut rt, Key::Down);
        assert_eq!(
            rt.state.sidebar_cursor.as_deref(),
            Some("sidebar-project:p1")
        );
        assert_eq!(rt.state.editor.project_id.as_deref(), Some("p1"));
        assert_eq!(rt.state.editor.checkout, Some(PathBuf::from("/work/a/web")));
        assert_eq!(
            crate::sidebar::rows(&rt.state).len(),
            10,
            "Unassigned is gone"
        );

        // Next is p1's root session: the row's own focus+show action.
        press_alt(&mut rt, Key::Down);
        assert_eq!(
            rt.state.sidebar_cursor.as_deref(),
            Some(format!("sidebar-session:{root_sess}").as_str())
        );
        assert_eq!(
            rt.state.selected_session.as_deref(),
            Some(root_sess.as_str())
        );

        // The worktree group header is a row too: landing on it toggles —
        // closed, p1's worktree rows leave the list and the walk lands on
        // p2's project row.
        press_alt(&mut rt, Key::Down);
        assert_eq!(
            rt.state.sidebar_cursor.as_deref(),
            Some("sidebar-wtgroup:p1")
        );
        assert_eq!(rt.state.worktree_groups.get("p1"), Some(&false));
        press_alt(&mut rt, Key::Down);
        assert_eq!(
            rt.state.sidebar_cursor.as_deref(),
            Some("sidebar-project:p2")
        );
        assert_eq!(rt.state.editor.project_id.as_deref(), Some("p2"));

        // Back onto the header reopens the group; the next press enters
        // feat-a's checkout row, then its session.
        press_alt(&mut rt, Key::Up);
        assert_eq!(
            rt.state.sidebar_cursor.as_deref(),
            Some("sidebar-wtgroup:p1")
        );
        assert_eq!(rt.state.worktree_groups.get("p1"), Some(&true));
        press_alt(&mut rt, Key::Down);
        assert_eq!(
            rt.state.sidebar_cursor.as_deref(),
            Some("sidebar-worktree:p1:0")
        );
        assert_eq!(
            rt.state.editor.checkout,
            Some(PathBuf::from("/work/a/feat-a"))
        );
        press_alt(&mut rt, Key::Down);
        assert_eq!(
            rt.state.sidebar_cursor.as_deref(),
            Some(format!("sidebar-session:{wt_sess}").as_str())
        );
        assert_eq!(rt.state.selected_session.as_deref(), Some(wt_sess.as_str()));
    }

    #[test]
    fn alt_up_from_nowhere_lands_on_the_last_row() {
        let mut s = sidebar_scene();
        s.view = View::Terminal;
        let loose = seed_session(&mut s, "loose", "/tmp");
        let mut rt = key_runtime(s);
        // No project is active, so Unassigned and its session close the list.
        press_alt(&mut rt, Key::Up);
        assert_eq!(
            rt.state.sidebar_cursor.as_deref(),
            Some(format!("sidebar-session:{loose}").as_str())
        );
        assert_eq!(rt.state.selected_session.as_deref(), Some(loose.as_str()));
        assert_eq!(
            rt.state.active_workspace.as_deref(),
            Some(crate::data::UNASSIGNED_WORKSPACE)
        );
    }

    #[test]
    fn alt_up_down_with_an_empty_sidebar_fall_through() {
        let mut s = State::new(test_handle());
        s.view = View::Terminal;
        let mut rt = key_runtime(s);
        rt.update(|s, tx| {
            assert!(!alt_shortcut(s, &alt_key(Key::Down), tx));
            assert!(!alt_shortcut(s, &alt_key(Key::Up), tx));
        });
        assert!(rt.state.sidebar_cursor.is_none());
    }

    #[test]
    fn alt_chords_wait_behind_the_sidebar_menu() {
        let mut s = two_tab_scene();
        s.open_menu(
            crate::sidebar::RowTarget::Project {
                project_id: "p1".into(),
            },
            crate::sidebar::MenuKind::More,
        );
        let mut rt = key_runtime(s);
        press_alt(&mut rt, Key::Right);
        press_alt(&mut rt, Key::Text("1".into()));
        assert_eq!(rt.state.active_tab, 1, "no tab switch under the menu");
    }

    #[test]
    fn alt_arrows_in_the_editor_view_are_left_to_word_motion() {
        let mut rt = key_runtime(two_tab_scene());
        rt.update(|s, _| s.view = View::Editor);
        rt.update(|s, tx| {
            assert!(!alt_shortcut(s, &alt_key(Key::Left), tx));
            assert!(!alt_shortcut(s, &alt_key(Key::Down), tx));
        });
        assert_eq!(rt.state.active_tab, 1);
    }

    #[test]
    fn a_stale_preedit_does_not_block_shortcuts_outside_the_terminal() {
        let mut s = start_scene();
        s.view = View::Editor;
        s.preedit = "vie".into();
        let mut rt = key_runtime(s);
        press_cmd(&mut rt, 't');
        assert_eq!(
            rt.state.pending.len(),
            1,
            "⌘T still starts a shell in the Editor"
        );
    }

    #[test]
    fn choosing_an_agent_in_the_new_tab_menu_requests_it_for_the_active_checkout() {
        let mut s = start_scene();
        s.editor.project_id = Some("p1".into());
        s.editor.checkout = Some(PathBuf::from("/work/a/web"));
        let (backend, rx) = crate::editor::backend::channel_backend();
        s.editor.backend = backend;
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        click_key(&mut rt, "new-tab-menu");
        settle(&mut rt);
        click_role(&mut rt, "option", "Claude Code");
        assert!(matches!(
            workspace_jobs(&rx).as_slice(),
            [Req::AgentArgv { agent, .. }] if agent == "claude"
        ));
        assert_eq!(rt.state.pending_argv.len(), 1);
        let pending = &rt.state.pending_argv[0];
        assert_eq!(pending.agent, "claude");
        assert_eq!(
            pending.target,
            crate::sidebar::RowTarget::Worktree {
                project_id: "p1".into(),
                path: "/work/a/web".into(),
            }
        );
    }

    #[test]
    fn a_second_create_while_one_is_pending_enqueues_once() {
        let mut s = State::new(test_handle());
        let (backend, rx) = crate::editor::backend::channel_backend();
        s.editor.backend = backend;
        s.submit_workspace_create("Work");
        s.submit_workspace_create("Work");
        let creates = workspace_jobs(&rx)
            .into_iter()
            .filter(|req| matches!(req, Req::CreateWorkspace { .. }))
            .count();
        assert_eq!(creates, 1, "a double Enter sends one request");
    }

    #[test]
    fn a_second_rename_while_one_is_pending_enqueues_once() {
        let mut s = State::new(test_handle());
        let (backend, rx) = crate::editor::backend::channel_backend();
        s.editor.backend = backend;
        s.submit_workspace_rename("w1", "Work");
        s.submit_workspace_rename("w1", "Work");
        let renames = workspace_jobs(&rx)
            .into_iter()
            .filter(|req| matches!(req, Req::RenameWorkspace { .. }))
            .count();
        assert_eq!(renames, 1, "a double Enter sends one request");
    }

    #[test]
    fn the_next_submission_is_accepted_after_the_first_reply() {
        let mut s = State::new(test_handle());
        let (backend, rx) = crate::editor::backend::channel_backend();
        s.editor.backend = backend;
        s.submit_workspace_create("Work");
        let token = s.workspace_pending.expect("first submission in flight");
        s.apply_reply(reply(
            Req::CreateWorkspace {
                name: "Work".into(),
                token,
            },
            Err("a workspace named 'Work' already exists".into()),
        ));
        s.submit_workspace_create("Other");
        let creates = workspace_jobs(&rx)
            .into_iter()
            .filter(|req| matches!(req, Req::CreateWorkspace { .. }))
            .count();
        assert_eq!(creates, 2, "the second name is sent once the first settled");
    }

    #[test]
    fn a_refused_selection_still_applies_and_says_it_was_not_saved() {
        let mut s = State::new(test_handle());
        let (backend, rx) = crate::editor::backend::channel_backend();
        drop(rx);
        s.editor.backend = backend;
        s.select_workspace("w1");
        assert_eq!(s.workspace_id(), "w1", "the choice applies for this run");
        assert_eq!(
            s.notice.as_deref(),
            Some("the editor queue is full; the workspace choice was not saved")
        );
    }

    #[test]
    fn a_refused_remove_keeps_the_selection_and_the_confirmation() {
        let mut s = State::new(test_handle());
        s.select_workspace("w1");
        s.workspace_remove_confirm = Some("w1".into());
        let (backend, rx) = crate::editor::backend::channel_backend();
        drop(rx);
        s.editor.backend = backend;
        s.remove_workspace("w1");
        assert_eq!(s.workspace_id(), "w1");
        assert_eq!(s.workspace_remove_confirm.as_deref(), Some("w1"));
        assert_eq!(
            s.notice.as_deref(),
            Some("the editor queue is full; try again")
        );
    }

    #[test]
    fn a_refused_move_sets_the_notice() {
        let mut s = State::new(test_handle());
        let (backend, rx) = crate::editor::backend::channel_backend();
        drop(rx);
        s.editor.backend = backend;
        s.move_project("p1", "w1");
        assert_eq!(
            s.notice.as_deref(),
            Some("the editor queue is full; try again")
        );
    }

    #[test]
    fn a_usage_reply_stores_the_snapshot_and_accounts() {
        let mut s = State::new(test_handle());
        s.apply_reply(reply(
            Req::Usage {
                previous: app_core::usage::UsageSnapshot::default(),
            },
            Ok(ReplyData::Usage {
                snapshot: app_core::usage::UsageSnapshot {
                    claude: app_core::usage::ClaudeUsage {
                        updated_at: Some(42),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                accounts: usage_view(),
            }),
        ));
        assert!(s.data.usage.is_some());
        assert_eq!(s.data.usage.as_ref().unwrap().claude.updated_at, Some(42));
        assert_eq!(s.data.accounts.as_ref().unwrap().claude.items.len(), 2);
    }

    #[test]
    fn a_failed_account_mutation_leaves_the_cache_and_sets_the_notice() {
        let mut s = State::new(test_handle());
        s.apply_reply(reply(
            Req::AccountRemove {
                agent: "claude".into(),
                id: "x".into(),
            },
            Err("boom".into()),
        ));
        assert!(s.data.accounts.is_none(), "cache mutates only on success");
        assert_eq!(s.notice.as_deref(), Some("boom"));
    }

    #[test]
    fn a_set_active_reply_swaps_the_view_and_refreshes_usage() {
        let mut s = State::new(test_handle());
        s.apply_reply(reply(
            Req::AccountSetActive {
                agent: "claude".into(),
                id: "aa11bb22".into(),
            },
            Ok(ReplyData::Accounts(usage_view())),
        ));
        assert!(s.data.accounts.is_some());
    }

    #[test]
    fn an_add_reply_asks_for_the_new_accounts_sign_in() {
        let mut s = State::new(test_handle());
        // The reply carries the view `add` produced — the new account is
        // active, so it is the one sign-in is requested for.
        let mut view = usage_view();
        view.claude.active = "aa11bb22".into();
        s.apply_reply(reply(
            Req::AccountAdd {
                agent: "claude".into(),
                name: "work".into(),
                dir: "/accts/work".into(),
                token: 1,
            },
            Ok(ReplyData::Accounts(view)),
        ));
        assert_eq!(
            s.data.accounts.as_ref().unwrap().claude.active,
            "aa11bb22",
            "the added account is active"
        );
    }

    // -- launch correlation ---------------------------------------------------

    /// Two launches in flight: each answer starts its own request. The
    /// second answer landing first must not take the first one's context.
    #[test]
    fn two_launches_keep_their_own_answer() {
        let mut s = State::new(test_handle());
        let first_row = crate::sidebar::RowTarget::Worktree {
            project_id: "p1".into(),
            path: "/w/one".into(),
        };
        let second_row = crate::sidebar::RowTarget::Worktree {
            project_id: "p1".into(),
            path: "/w/two".into(),
        };
        s.request_session_argv(first_row, "claude".into());
        s.request_session_argv(second_row, "codex".into());
        assert_eq!(s.pending_argv.len(), 2, "both launches wait");

        let claude_token = s.pending_argv[0].token;
        let codex_token = s.pending_argv[1].token;

        // The codex answer lands first.
        let launch = s
            .resolved_launch(codex_token, vec!["codex".into()])
            .expect("the codex launch");
        assert_eq!(launch.agent, "codex");
        assert_eq!(launch.argv, vec!["codex".to_string()]);
        assert_eq!(launch.cwd, "/w/two", "the second row's checkout");
        assert_eq!(s.pending_argv.len(), 1, "the claude launch still waits");
        assert_eq!(s.pending_argv[0].token, claude_token);

        // The claude answer still starts its own launch — in its own row.
        let launch = s
            .resolved_launch(claude_token, vec!["claude".into()])
            .expect("the claude launch");
        assert_eq!(launch.agent, "claude");
        assert_eq!(launch.cwd, "/w/one", "the first row's checkout");
        assert!(s.pending_argv.is_empty());
    }

    /// The same agent asked for twice: the answers must not cross over.
    #[test]
    fn two_launches_of_the_same_agent_keep_their_own_row() {
        let mut s = State::new(test_handle());
        s.request_session_argv(
            crate::sidebar::RowTarget::Worktree {
                project_id: "p1".into(),
                path: "/w/one".into(),
            },
            "claude".into(),
        );
        s.request_session_argv(
            crate::sidebar::RowTarget::Worktree {
                project_id: "p1".into(),
                path: "/w/two".into(),
            },
            "claude".into(),
        );
        let first = s.pending_argv[0].token;
        let second = s.pending_argv[1].token;

        let launch = s
            .resolved_launch(first, vec!["claude-a".into()])
            .expect("the first launch");
        assert_eq!(launch.cwd, "/w/one");
        assert_eq!(launch.argv, vec!["claude-a".to_string()]);
        assert_eq!(
            s.pending_argv.len(),
            1,
            "the second launch keeps waiting for its own answer"
        );
        assert_eq!(s.pending_argv[0].token, second);
    }

    /// A refused launch must say why, and it must not consume another
    /// request's launch context.
    #[test]
    fn a_failed_agent_argv_surfaces_the_error_and_keeps_the_other_launch() {
        let mut s = State::new(test_handle());
        s.request_session_argv(
            crate::sidebar::RowTarget::Project {
                project_id: "p1".into(),
            },
            "claude".into(),
        );
        s.request_session_argv(
            crate::sidebar::RowTarget::Project {
                project_id: "p2".into(),
            },
            "codex".into(),
        );
        let claude_token = s.pending_argv[0].token;
        let codex_token = s.pending_argv[1].token;

        s.apply_reply(reply(
            Req::AgentArgv {
                agent: "claude".into(),
                token: claude_token,
            },
            Err("the account folder /accts/work no longer exists".into()),
        ));
        assert_eq!(
            s.notice.as_deref(),
            Some("the account folder /accts/work no longer exists"),
            "the failure is visible"
        );
        assert_eq!(s.pending_argv.len(), 1, "the other launch still waits");
        assert_eq!(s.pending_argv[0].token, codex_token);
    }

    #[test]
    fn a_sign_in_reply_launches_a_session_with_the_title() {
        let mut s = State::new(test_handle());
        s.apply_reply(reply(
            Req::AccountSignIn {
                agent: "claude".into(),
                id: "aa11bb22".into(),
            },
            Ok(ReplyData::SignIn {
                argv: vec![
                    "/usr/bin/env".into(),
                    "CLAUDE_CONFIG_DIR=/a".into(),
                    "claude".into(),
                ],
                cwd: "/a".into(),
                title: "Claude sign-in".into(),
            }),
        ));
        // The daemon isn't connected in tests: the launch is queued (or the
        // notice explains the refusal) — never a panic on the reply path.
        assert!(
            !s.pending.is_empty() || s.notice.is_some(),
            "the sign-in launch must at least be attempted"
        );
    }

    #[test]
    fn a_sign_in_error_surfaces_as_the_notice() {
        let mut s = State::new(test_handle());
        s.apply_reply(reply(
            Req::AccountSignIn {
                agent: "claude".into(),
                id: "x".into(),
            },
            Err("no account".into()),
        ));
        assert_eq!(s.notice.as_deref(), Some("no account"));
        assert!(s.pending.is_empty());
    }

    #[test]
    fn the_stored_theme_restores_on_its_getsetting_reply() {
        let mut s = State::new(test_handle());
        assert_eq!(s.theme_choice, crate::theme::Theme::Dark);
        s.apply_reply(reply(
            Req::GetSetting {
                key: crate::theme::THEME_KEY.to_string(),
            },
            Ok(ReplyData::Setting(Some("dracula".into()))),
        ));
        assert_eq!(s.theme_choice, crate::theme::Theme::Dracula);
        // A missing row leaves the default alone.
        let mut s = State::new(test_handle());
        s.apply_reply(reply(
            Req::GetSetting {
                key: crate::theme::THEME_KEY.to_string(),
            },
            Ok(ReplyData::Setting(None)),
        ));
        assert_eq!(s.theme_choice, crate::theme::Theme::Dark);
    }

    #[test]
    fn the_stored_scroll_speed_restores_on_its_getsetting_reply() {
        let mut s = State::new(test_handle());
        assert_eq!(s.scroll_speed, 1.0);
        s.apply_reply(reply(
            Req::GetSetting {
                key: settings::SCROLL_SPEED_KEY.to_string(),
            },
            Ok(ReplyData::Setting(Some("1.5".into()))),
        ));
        assert_eq!(s.scroll_speed, 1.5);
        // A missing row leaves the default alone.
        let mut s = State::new(test_handle());
        s.apply_reply(reply(
            Req::GetSetting {
                key: settings::SCROLL_SPEED_KEY.to_string(),
            },
            Ok(ReplyData::Setting(None)),
        ));
        assert_eq!(s.scroll_speed, 1.0);
    }

    #[test]
    fn the_done_sound_setting_restores_on_its_getsetting_reply() {
        let mut s = State::new(test_handle());
        assert!(s.sound_done, "absent means on");
        s.apply_reply(reply(
            Req::GetSetting {
                key: settings::SOUND_DONE_KEY.to_string(),
            },
            Ok(ReplyData::Setting(Some("0".into()))),
        ));
        assert!(!s.sound_done);
        // A missing row leaves the default alone.
        let mut s = State::new(test_handle());
        s.apply_reply(reply(
            Req::GetSetting {
                key: settings::SOUND_DONE_KEY.to_string(),
            },
            Ok(ReplyData::Setting(None)),
        ));
        assert!(s.sound_done);
    }

    #[test]
    fn an_activity_flip_to_done_chimes_once() {
        let mut s = State::new(test_handle());
        s.window_focused = false;
        let reply_with = |activity: HashMap<String, app_core::activity::Activity>| {
            reply(
                Req::Activity {
                    sessions: Vec::new(),
                    previous: HashMap::new(),
                },
                Ok(ReplyData::Activity(activity)),
            )
        };
        let mut activity = HashMap::new();
        activity.insert("sess-1".to_string(), app_core::activity::Activity::Working);
        s.apply_reply(reply_with(activity.clone()));
        assert_eq!(crate::sound::test_plays(), 0, "working is not done");
        activity.insert("sess-1".to_string(), app_core::activity::Activity::Done);
        s.apply_reply(reply_with(activity.clone()));
        assert_eq!(crate::sound::test_plays(), 1, "the flip chimes");
        // Past the debounce, the same done state does not chime again.
        s.now_ms += 2_000;
        s.apply_reply(reply_with(activity));
        assert_eq!(crate::sound::test_plays(), 1);
        // Focused windows stay silent even on a fresh flip.
        s.window_focused = true;
        let mut next = HashMap::new();
        next.insert("sess-2".to_string(), app_core::activity::Activity::Done);
        s.apply_reply(reply_with(next));
        assert_eq!(crate::sound::test_plays(), 1);
    }

    #[test]
    fn poll_usage_paces_the_worker_reads() {
        let mut s = State::new(test_handle());
        assert!(s.poll_usage(0.0), "the first read goes out");
        assert!(!s.poll_usage(1.0), "inside the 5 s window");
        assert!(!s.poll_usage(4.9));
        assert!(s.poll_usage(5.1), "the cadence allows the next read");
    }

    /// A running session's dot spins on the renderer's own clock: the poll
    /// must not ask for a frame (or report a change) every tick for it.
    #[test]
    fn a_running_session_does_not_keep_the_poll_redrawing() {
        let mut s = State::new(test_handle());
        s.data.sessions.push(data::SessionView::new(
            protocol::session::SessionInfo {
                id: protocol::session::SessionId::new(),
                title: "shell".into(),
                argv: vec!["/bin/zsh".into()],
                cwd: "/work/repo".into(),
                state: protocol::session::SessionState::Running,
                pid: None,
                exit_code: None,
                error: None,
                started_at: 1_700_000_000_000,
                ended_at: None,
            },
            None,
        ));
        assert!(s.data.sessions[0].info.state.working());
        poll(&mut s, 0.0, &mut Tx::default());
        for tick in 1..=5 {
            let mut tx = Tx::default();
            assert!(!poll(&mut s, tick as f64 * 0.033, &mut tx), "tick {tick}");
        }
    }

    #[test]
    fn theme_parse_and_resolve_follow_the_desktop() {
        use crate::theme::Theme;
        assert_eq!(Theme::parse("system"), Theme::System);
        assert_eq!(Theme::parse("one-dark"), Theme::OneDark);
        assert_eq!(Theme::parse("catppuccin-mocha"), Theme::CatppuccinMocha);
        assert_eq!(Theme::parse("dracula"), Theme::Dracula);
        assert_eq!(Theme::parse("nonsense"), Theme::Dark, "unknown → Dark+");
        assert_eq!(Theme::System.resolve(true), Theme::Dark);
        assert_eq!(Theme::System.resolve(false), Theme::Light);
        assert_eq!(Theme::Dracula.resolve(false), Theme::Dracula);
    }

    #[test]
    fn the_usage_menu_needs_an_account_view() {
        let mut s = State::new(test_handle());
        s.usage_menu = Some("claude".to_string());
        let mut layer = Vec::new();
        assert!(chrome::usage_menu_layer(&s, &mut layer).is_none());
        s.data.accounts = Some(usage_view());
        assert!(chrome::usage_menu_layer(&s, &mut layer).is_some());
    }

    /// The Settings page alone, so its controls can be counted.
    fn settings_view(s: &State, _cx: &mut Cx<State>) -> El {
        settings::view(s, &s.ink)
    }

    /// Default has no sign-in of its own — the core refuses it — so the
    /// Accounts section must not offer the button for it.
    #[test]
    fn the_default_account_has_no_sign_in_button() {
        let mut s = State::new(test_handle());
        s.settings.section = crate::settings::Section::Accounts;
        // The view: Default plus one named Claude account, and Codex with
        // Default only.
        s.data.accounts = Some(usage_view());
        let mut rt = Runtime::new(s, settings_view, (1512.0, 814.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let sign_ins = els(&rt)
            .iter()
            .filter(|element| element.label.as_deref() == Some("Sign in"))
            .count();
        assert_eq!(sign_ins, 1, "only the named account can sign in");
    }

    /// An open combobox reports `expanded` on its trigger — the chevron
    /// and the a11y state come from the same flag.
    #[test]
    fn an_open_combobox_reports_expanded_on_its_trigger() {
        let mut s = State::new(test_handle());
        s.settings.section = crate::settings::Section::Editor;
        let mut rt = Runtime::new(s, settings_view, (1512.0, 814.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let expanded = |rt: &Runtime<State>| {
            els(rt)
                .iter()
                .find(|e| e.key.as_deref() == Some("settings-indent-value-trigger"))
                .expect("the indent picker mounts")
                .expanded
        };
        assert_eq!(expanded(&rt), Some(false), "closed reports collapsed");
        click_key(&mut rt, "settings-indent-value-trigger");
        assert!(rt.state.settings.indent_picker.open, "press opens the list");
        assert_eq!(expanded(&rt), Some(true), "open reports expanded");
    }

    /// Settings → Updates names the running version, and offers the install
    /// command only once a newer release is known.
    #[test]
    fn settings_updates_shows_the_version_and_the_install_command_when_one_is_out() {
        let page = |status: update::Status| {
            let mut s = State::new(test_handle());
            s.settings.section = crate::settings::Section::Updates;
            s.update = status;
            let mut rt = Runtime::new(s, settings_view, (1512.0, 814.0), Env::new(true, 2.0));
            rt.frame(0.0);
            let infos = els(&rt);
            let has = |wanted: &str| {
                infos.iter().any(|e| {
                    e.label.as_deref() == Some(wanted) || e.text.as_deref() == Some(wanted)
                })
            };
            (
                has(&format!("Goko {}", update::CURRENT)),
                has("Check for updates"),
                has("Copy install command"),
                has("Goko 9.9.9 is available."),
            )
        };
        let (version, check, copy, said) = page(update::Status::UpToDate);
        assert!(
            version && check,
            "the version and the check button are always there"
        );
        assert!(!copy && !said, "nothing to install yet");
        let (version, _, copy, said) = page(update::Status::Available("9.9.9".into()));
        assert!(
            version && copy && said,
            "the new release comes with the command"
        );
    }

    // -- Terminal row geometry ------------------------------------------------

    /// Whether the element `idx` sits under the element `ancestor` —
    /// `ElementInfo::parent` chains up the mounted tree.
    fn inside_ancestor(els: &[rune::ElementInfo], idx: usize, ancestor: usize) -> bool {
        let mut i = Some(idx);
        while let Some(n) = i {
            if n == ancestor {
                return true;
            }
            i = els[n].parent;
        }
        false
    }

    /// The text rows mounted inside the pane `pane_key`'s cell, plus the
    /// cell's own rect — a pane holds exactly one terminal's grid.
    fn pane_rows(rt: &Runtime<State>, pane_key: &str) -> (Vec<rune::ElementInfo>, [f32; 4]) {
        let want = format!("pane:{}", panes::pane_id(pane_key));
        let els = els(rt);
        let cell = els
            .iter()
            .position(|e| e.key.as_deref() == Some(want.as_str()))
            .expect("the pane cell is mounted");
        let rows = els
            .iter()
            .enumerate()
            .filter(|(i, e)| e.text.is_some() && inside_ancestor(&els, *i, cell))
            .map(|(_, e)| e.clone())
            .collect();
        (rows, els[cell].rect)
    }

    /// A frame like the agent TUI draws: styled banner lines (colour runs,
    /// spaces to wrap at, and 'W's wider than the measured cell), a blank
    /// stretch, then home. 'W' shapes wider than the cell in the test
    /// fonts — what a real frame's wide glyphs do — so a wrapping row is
    /// reproduced, not hoped for.
    fn feed_tui_frame(tx: &mpsc::Sender<daemon::Output>, seq: &mut u64) {
        let mut data = Vec::new();
        for _ in 0..6 {
            data.extend_from_slice("\u{1b}[31m".as_bytes());
            data.extend_from_slice("W ".repeat(60).as_bytes());
            data.extend_from_slice("\u{1b}[32m".as_bytes());
            data.extend_from_slice("W ".repeat(60).as_bytes());
            data.extend_from_slice("\u{1b}[0m\r\n".as_bytes());
        }
        data.extend_from_slice("\r\n".repeat(4).as_bytes());
        data.extend_from_slice("\u{1b}[H".as_bytes());
        let len = data.len() as u64;
        tx.send(daemon::Output { seq: *seq, data }).unwrap();
        *seq += len;
    }

    /// Address the input prompt onto the grid's last line — where a
    /// full-screen program keeps its input frame.
    fn feed_last_row_marker(tx: &mpsc::Sender<daemon::Output>, seq: &mut u64, row: usize) {
        let data = format!("\u{1b}[{row};1H\u{276f} PROMPT-MARKER").into_bytes();
        let len = data.len() as u64;
        tx.send(daemon::Output { seq: *seq, data }).unwrap();
        *seq += len;
    }

    /// The terminal's rows inside a column deliberately narrower than the
    /// text: the same mount the pane gives, squeezed.
    fn narrow_term_view(s: &State, _cx: &mut Cx<State>) -> El {
        let rows = s
            .attachments
            .values()
            .next()
            .map(|att| term::term_lines_el(&att.lines, &att.links, &s.ink))
            .unwrap_or_default();
        col().w(220.0).min_h(0.0).children(rows)
    }

    #[test]
    fn a_terminal_row_stays_one_cell_line_wide_or_not() {
        // Every drawn grid row is a single line: `rich` wrapped a row
        // wider than the pane onto a second line, and the extra height
        // pushed the grid's bottom rows — a TUI's input frame — out of
        // view. Narrow the column under the rows to force the check.
        let mut s = State::new(test_handle());
        let key = seed_session(&mut s, "claude", "/work/repo");
        let session = s.session_id(&key).unwrap();
        let (tx, rx) = mpsc::channel();
        let (_ready_tx, ready_rx) = mpsc::channel();
        let mut terminal = term::Terminal::new(session, rx, ready_rx);
        let mut seq = 0u64;
        feed_tui_frame(&tx, &mut seq);
        terminal.feed();
        attach(&mut s, &key, terminal);
        let ink = s.ink;
        let lines = s
            .attachments
            .get_mut(&key)
            .unwrap()
            .term
            .lines(&ink)
            .to_vec();
        assert!(
            lines.iter().any(|l| l.text().contains("W W")),
            "the styled rows fed the grid"
        );
        s.attachments.get_mut(&key).unwrap().set_lines(&lines, &key);

        let mut rt = Runtime::new(s, narrow_term_view, (400.0, 300.0), Env::new(true, 2.0));
        rt.frame(0.0);
        for e in els(&rt) {
            let Some(text) = e.text.as_deref() else {
                continue;
            };
            assert!(
                e.rect[3] <= 17.0,
                "a terminal row took {:.1}pt inside a narrow pane: {text:?}",
                e.rect[3]
            );
        }
    }

    #[test]
    fn the_pane_ends_inside_the_window() {
        for (w, h) in [(963.0, 780.0), (1512.0, 814.0), (900.0, 560.0)] {
            let mut s = State::new(test_handle());
            let key = seed_session(&mut s, "claude", "/work/repo");
            s.focus_session_tab(&key);
            let session = s.session_id(&key).unwrap();
            let (_tx, rx) = mpsc::channel();
            let (_ready_tx, ready_rx) = mpsc::channel();
            let terminal = term::Terminal::new(session, rx, ready_rx);
            attach(&mut s, &key, terminal);
            let pk = key.clone();
            let mut rt = Runtime::new(s, view, (w, h), Env::new(true, 2.0));
            rt.poll = Some(poll);
            rt.frame(0.0);
            rt.frame(1.0 / 60.0);
            rt.frame(1.0 / 60.0);
            // The tab bar and controls sit above the pane: it must take
            // what they leave, not the whole column's height.
            let (_, rect) = pane_rows(&rt, &pk);
            assert!(
                rect[1] + rect[3] <= h + 0.5,
                "the pane runs below the window at {w}x{h}: {rect:?}"
            );
        }
    }

    #[test]
    fn the_wheel_over_a_pane_scrolls_its_history() {
        let mut s = State::new(test_handle());
        let key = seed_session(&mut s, "claude", "/work/repo");
        s.focus_session_tab(&key);
        let session = s.session_id(&key).unwrap();
        let (tx, rx) = mpsc::channel();
        let (_ready_tx, ready_rx) = mpsc::channel();
        let mut terminal = term::Terminal::new(session, rx, ready_rx);
        let data: String = (0..200).map(|n| format!("row{n}\r\n")).collect();
        tx.send(daemon::Output {
            seq: 0,
            data: data.into_bytes(),
        })
        .unwrap();
        terminal.feed();
        attach(&mut s, &key, terminal);
        let pane_key = key.clone();
        let mut rt = Runtime::new(s, view, (1200.0, 800.0), Env::new(true, 2.0));
        rt.poll = Some(poll);
        for _ in 0..6 {
            rt.frame(1.0 / 60.0);
        }
        let top = |rt: &mut Runtime<State>| {
            let mut text = String::new();
            rt.update(|s, _| text = s.attachments[&pane_key].lines[0].text());
            text.trim_end().to_string()
        };
        let live = top(&mut rt);
        rt.wheel_event(rune::WheelEvent::new(0.0, 160.0).at(700.0, 400.0));
        rt.frame(1.0 / 60.0);
        let up = top(&mut rt);
        assert_ne!(up, live, "the wheel moved the view into the history");
        rt.update(|s, _| {
            let ev = rune::KeyEvent {
                key: rune::Key::Text("a".into()),
                mods: rune::Mods::default(),
            };
            assert!(
                terminal_key(s, &ev, None),
                "the key reached the live session"
            );
        });
        rt.frame(1.0 / 60.0);
        assert_eq!(top(&mut rt), live, "typing returned to the live screen");
    }

    #[test]
    fn files_dropped_on_a_pane_are_typed_into_its_session() {
        let mut s = State::new(test_handle());
        let key = seed_session(&mut s, "claude", "/work/repo");
        s.focus_session_tab(&key);
        let session = s.session_id(&key).unwrap();
        let (tx, rx) = mpsc::channel();
        let (_ready_tx, ready_rx) = mpsc::channel();
        let mut terminal = term::Terminal::new(session, rx, ready_rx);
        let data: String = (0..200).map(|n| format!("row{n}\r\n")).collect();
        tx.send(daemon::Output {
            seq: 0,
            data: data.into_bytes(),
        })
        .unwrap();
        terminal.feed();
        attach(&mut s, &key, terminal);
        let pane_key = key.clone();
        let mut rt = Runtime::new(s, view, (1200.0, 800.0), Env::new(true, 2.0));
        rt.poll = Some(poll);
        for _ in 0..6 {
            rt.frame(1.0 / 60.0);
        }
        let top = |rt: &mut Runtime<State>| {
            let mut text = String::new();
            rt.update(|s, _| text = s.attachments[&pane_key].lines[0].text());
            text.trim_end().to_string()
        };
        let live = top(&mut rt);
        rt.wheel_event(rune::WheelEvent::new(0.0, 160.0).at(700.0, 400.0));
        rt.frame(1.0 / 60.0);
        assert_ne!(top(&mut rt), live, "the view is in the history");

        let file = std::path::PathBuf::from("/tmp/a b.txt");
        rt.drag_files(vec![file.clone()], Some((700.0, 400.0)));
        for _ in 0..3 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(
            rt.state.drop_pane.as_deref(),
            Some(key.as_str()),
            "the pane shows the drop ring"
        );
        rt.drop_files(vec![file], Some((700.0, 400.0)));
        for _ in 0..3 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(rt.state.drop_pane, None, "the ring went with the drop");
        assert_eq!(top(&mut rt), live, "the drop typed into the live session");
    }

    #[test]
    fn a_working_session_row_shows_rune_kits_spinner_in_place_of_its_icon() {
        let mut s = sidebar_scene();
        let key = seed_session(&mut s, "agent", "/work/a/web");
        s.focus_session_tab(&key);
        s.activity
            .insert(key.clone(), app_core::activity::Activity::Working);
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let row = format!("sidebar-session:{key}");
        // The radar's 25 dots (`d0`–`d24`) mount inside the session's row.
        let dots = |rt: &Runtime<State>| {
            els(rt)
                .iter()
                .filter(|e| {
                    e.path.contains(&row) && e.key.as_deref().is_some_and(|k| k.starts_with('d'))
                })
                .count()
        };
        assert_eq!(dots(&rt), 25, "the working row draws the spinner");

        // Done: the agent icon and its green dot come back.
        rt.update(|s, _| {
            s.activity
                .insert(key.clone(), app_core::activity::Activity::Done);
        });
        // The spinner fades out through its exit presence first (~0.3 s).
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(dots(&rt), 0, "a finished row has no spinner");
    }

    #[test]
    fn the_terminal_hands_keyboard_focus_back_from_chrome_rows() {
        // Rune focus left on a pressable eats Space — it re-fires
        // `on_press` — before `on_key` sees it. A chrome press yields the
        // focus in the same frame, so the keystroke reaches the terminal.
        let mut s = sidebar_scene();
        let key = seed_session(&mut s, "agent", "/work/a/web");
        s.focus_session_tab(&key);
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.poll = Some(poll);
        rt.frame(0.0);
        // Click the tab pill left of centre — on the title, away from the
        // close X — the way a stray click lands focus on chrome.
        let tab_key = format!("tab:tab-{key}");
        let tab = els(&rt)
            .into_iter()
            .find(|e| e.key.as_deref() == Some(tab_key.as_str()))
            .expect("the session's tab pill mounted");
        let (x, y) = (tab.rect[0] + 24.0, tab.rect[1] + tab.rect[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..3 {
            rt.frame(1.0 / 60.0);
        }
        let stuck: Vec<_> = els(&rt)
            .iter()
            .filter(|e| e.focused)
            .map(|e| (e.key.clone(), e.role, e.path.clone()))
            .collect();
        assert!(stuck.is_empty(), "chrome kept rune focus: {stuck:?}");
    }

    #[test]
    fn dragging_across_terminal_rows_selects_them_and_cmd_c_copies() {
        let mut s = State::new(test_handle());
        let key = seed_session(&mut s, "claude", "/work/repo");
        s.focus_session_tab(&key);
        let session = s.session_id(&key).unwrap();
        let (tx, rx) = mpsc::channel();
        let (_ready_tx, ready_rx) = mpsc::channel();
        let mut terminal = term::Terminal::new(session, rx, ready_rx);
        let data: String = (0..60).map(|n| format!("row{n}\r\n")).collect();
        tx.send(daemon::Output {
            seq: 0,
            data: data.into_bytes(),
        })
        .unwrap();
        terminal.feed();
        attach(&mut s, &key, terminal);
        let mut rt = Runtime::new(s, view, (1200.0, 800.0), Env::new(true, 2.0));
        rt.poll = Some(poll);
        for _ in 0..6 {
            rt.frame(1.0 / 60.0);
        }
        let rows: Vec<[f32; 4]> = rt
            .elements()
            .iter()
            .filter(|e| e.text.as_deref().is_some_and(|t| t.starts_with("row")))
            .map(|e| e.rect)
            .collect();
        assert!(rows.len() > 4, "terminal rows are laid out: {}", rows.len());
        let (first, last) = (rows[1], rows[rows.len() - 2]);
        let (x0, y0) = (first[0] + 2.0, first[1] + first[3] / 2.0);
        let (x1, y1) = (last[0] + 20.0, last[1] + last[3] / 2.0);
        rt.pointer_down(x0, y0);
        rt.pointer_move((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        rt.pointer_move(x1, y1);
        rt.pointer_up(x1, y1);
        rt.frame(1.0 / 60.0);
        assert!(rt.selected_text().is_some(), "the drag selected rows");
        rt.key(KeyEvent {
            key: Key::Command('c'),
            mods: Mods {
                cmd: true,
                ..Mods::default()
            },
        });
        rt.frame(1.0 / 60.0);
        let copied = rt
            .copy_request
            .as_ref()
            .map(|w| w.content().text().to_owned())
            .unwrap_or_default();
        assert!(copied.contains("row"), "copied {copied:?}");
        assert!(copied.contains('\n'), "several rows: {copied:?}");
        assert!(
            copied.lines().all(|l| l == l.trim_end()),
            "no grid padding: {copied:?}"
        );
        assert_eq!(rt.state.notice.as_deref(), Some("Copied to clipboard"));
    }

    #[test]
    fn a_finished_selection_copies_without_cmd_c() {
        let mut s = State::new(test_handle());
        let key = seed_session(&mut s, "claude", "/work/repo");
        s.focus_session_tab(&key);
        let session = s.session_id(&key).unwrap();
        let (tx, rx) = mpsc::channel();
        let (_ready_tx, ready_rx) = mpsc::channel();
        let mut terminal = term::Terminal::new(session, rx, ready_rx);
        let data: String = (0..60).map(|n| format!("row{n}\r\n")).collect();
        tx.send(daemon::Output {
            seq: 0,
            data: data.into_bytes(),
        })
        .unwrap();
        terminal.feed();
        attach(&mut s, &key, terminal);
        let mut rt = Runtime::new(s, view, (1200.0, 800.0), Env::new(true, 2.0));
        rt.poll = Some(poll);
        for _ in 0..6 {
            rt.frame(1.0 / 60.0);
        }
        let rows: Vec<[f32; 4]> = rt
            .elements()
            .iter()
            .filter(|e| e.text.as_deref().is_some_and(|t| t.starts_with("row")))
            .map(|e| e.rect)
            .collect();
        let (first, last) = (rows[1], rows[rows.len() - 2]);
        let (x0, y0) = (first[0] + 2.0, first[1] + first[3] / 2.0);
        let (x1, y1) = (last[0] + 20.0, last[1] + last[3] / 2.0);
        rt.pointer_down(x0, y0);
        rt.pointer_move((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        rt.pointer_move(x1, y1);
        // Mid-drag frames hear the change as `dragging`: nothing copies yet.
        rt.frame(1.0 / 60.0);
        assert!(
            rt.copy_request.is_none(),
            "a drag in progress must not copy"
        );
        rt.pointer_up(x1, y1);
        rt.frame(1.0 / 60.0);
        let copied = rt
            .copy_request
            .as_ref()
            .map(|w| w.content().text().to_owned())
            .unwrap_or_default();
        assert!(copied.contains("row"), "release copied {copied:?}");
        assert!(copied.contains('\n'), "several rows: {copied:?}");
        assert!(
            copied.lines().all(|l| l == l.trim_end()),
            "no grid padding: {copied:?}"
        );
        assert_eq!(rt.state.notice.as_deref(), Some("Copied to clipboard"));
    }

    /// A pane whose program captures the mouse (`prefix` turns the mode on)
    /// over 60 numbered rows, mounted with the poll running.
    fn captured_pane_runtime(prefix: &str) -> Runtime<State> {
        let mut s = State::new(test_handle());
        let key = seed_session(&mut s, "claude", "/work/repo");
        s.focus_session_tab(&key);
        let session = s.session_id(&key).unwrap();
        let (tx, rx) = mpsc::channel();
        let (_ready_tx, ready_rx) = mpsc::channel();
        let mut terminal = term::Terminal::new(session, rx, ready_rx);
        let rows: String = (0..60).map(|n| format!("row{n}\r\n")).collect();
        tx.send(daemon::Output {
            seq: 0,
            data: format!("{prefix}{rows}").into_bytes(),
        })
        .unwrap();
        terminal.feed();
        attach(&mut s, &key, terminal);
        let mut rt = Runtime::new(s, view, (1200.0, 800.0), Env::new(true, 2.0));
        rt.poll = Some(poll);
        for _ in 0..6 {
            rt.frame(1.0 / 60.0);
        }
        rt
    }

    /// Drag from the second shown row to the second-to-last and release.
    fn drag_across_rows(rt: &mut Runtime<State>) {
        let rows: Vec<[f32; 4]> = rt
            .elements()
            .iter()
            .filter(|e| e.text.as_deref().is_some_and(|t| t.starts_with("row")))
            .map(|e| e.rect)
            .collect();
        let (first, last) = (rows[1], rows[rows.len() - 2]);
        let (x0, y0) = (first[0] + 2.0, first[1] + first[3] / 2.0);
        let (x1, y1) = (last[0] + 20.0, last[1] + last[3] / 2.0);
        rt.pointer_down(x0, y0);
        rt.pointer_move((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        rt.pointer_move(x1, y1);
        rt.frame(1.0 / 60.0);
        rt.pointer_up(x1, y1);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn shift_drag_selects_and_copies_while_the_program_captures_the_mouse() {
        let mode = "\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h";
        // A plain drag belongs to the program: nothing is selected locally.
        let mut rt = captured_pane_runtime(mode);
        drag_across_rows(&mut rt);
        assert!(rt.copy_request.is_none(), "a captured drag copied");

        // Shift takes the pointer back: the drag selects and its release copies.
        let mut rt = captured_pane_runtime(mode);
        rt.update(|s, _| {
            s.editor.set_modifiers(Mods {
                shift: true,
                ..Mods::default()
            })
        });
        rt.frame(1.0 / 60.0);
        drag_across_rows(&mut rt);
        let copied = rt
            .copy_request
            .as_ref()
            .map(|w| w.content().text().to_owned())
            .unwrap_or_default();
        assert!(
            copied.contains("row") && copied.contains('\n'),
            "shift-drag copied {copied:?}"
        );
    }

    #[test]
    fn a_program_copy_on_the_shown_pane_reaches_the_clipboard() {
        let mut s = State::new(test_handle());
        let key = seed_session(&mut s, "claude", "/work/repo");
        s.focus_session_tab(&key);
        let session = s.session_id(&key).unwrap();
        let (tx, rx) = mpsc::channel();
        let (_ready_tx, ready_rx) = mpsc::channel();
        let terminal = term::Terminal::new(session, rx, ready_rx);
        attach(&mut s, &key, terminal);
        let mut rt = Runtime::new(s, view, (1200.0, 800.0), Env::new(true, 2.0));
        rt.poll = Some(poll);
        rt.frame(0.0);
        // The live stream starts with the daemon's paint; then the program
        // copies its own selection with OSC 52 ("copied" in base64).
        tx.send(daemon::Output {
            seq: protocol::frame::PAINT_SEQ,
            data: b"\x1bc".to_vec(),
        })
        .unwrap();
        tx.send(daemon::Output {
            seq: 0,
            data: b"\x1b]52;c;Y29waWVk\x07".to_vec(),
        })
        .unwrap();
        for _ in 0..3 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(
            rt.copy_request
                .as_ref()
                .map(|w| w.content().text().to_owned())
                .as_deref(),
            Some("copied")
        );
    }

    #[test]
    fn a_trackpad_scrolls_the_history_even_with_rows_selected() {
        let mut s = State::new(test_handle());
        let key = seed_session(&mut s, "claude", "/work/repo");
        s.focus_session_tab(&key);
        let session = s.session_id(&key).unwrap();
        let (tx, rx) = mpsc::channel();
        let (_ready_tx, ready_rx) = mpsc::channel();
        let mut terminal = term::Terminal::new(session, rx, ready_rx);
        let data: String = (0..200).map(|n| format!("row{n}\r\n")).collect();
        tx.send(daemon::Output {
            seq: 0,
            data: data.into_bytes(),
        })
        .unwrap();
        terminal.feed();
        attach(&mut s, &key, terminal);
        let pane_key = key.clone();
        let mut rt = Runtime::new(s, view, (1200.0, 800.0), Env::new(true, 2.0));
        rt.poll = Some(poll);
        for _ in 0..6 {
            rt.frame(1.0 / 60.0);
        }
        let top = |rt: &mut Runtime<State>| {
            let mut text = String::new();
            rt.update(|s, _| text = s.attachments[&pane_key].lines[0].text());
            text.trim_end().to_string()
        };
        let live = top(&mut rt);
        rt.pointer_down(560.0, 300.0);
        rt.pointer_move(600.0, 380.0);
        rt.pointer_move(640.0, 460.0);
        rt.pointer_up(640.0, 460.0);
        rt.frame(1.0 / 60.0);
        assert!(rt.selected_text().is_some(), "rows are selected");
        for ph in [
            rune::GesturePhase::Began,
            rune::GesturePhase::Moved,
            rune::GesturePhase::Moved,
            rune::GesturePhase::Ended,
        ]
        .into_iter()
        {
            rt.wheel_event(
                rune::WheelEvent::new(0.0, 12.0)
                    .at(700.0, 400.0)
                    .phase(ph)
                    .precise(true),
            );
            rt.frame(1.0 / 60.0);
        }
        assert_ne!(
            top(&mut rt),
            live,
            "trackpad scrolled, selected {:?}",
            rt.selected_text()
        );
    }

    #[test]
    fn a_terminal_link_opens_on_click_and_never_takes_tab() {
        let mut s = State::new(test_handle());
        let key = seed_session(&mut s, "claude", "/work/repo");
        s.focus_session_tab(&key);
        let session = s.session_id(&key).unwrap();
        let (tx, rx) = mpsc::channel();
        let (_ready_tx, ready_rx) = mpsc::channel();
        let mut terminal = term::Terminal::new(session, rx, ready_rx);
        tx.send(daemon::Output {
            seq: 0,
            data: b"see nope/zzz.rs:3 now\r\n".to_vec(),
        })
        .unwrap();
        terminal.feed();
        attach(&mut s, &key, terminal);
        let mut rt = Runtime::new(s, view, (1200.0, 800.0), Env::new(true, 2.0));
        rt.poll = Some(poll);
        for _ in 0..6 {
            rt.frame(1.0 / 60.0);
        }
        let row = rt
            .elements()
            .iter()
            .find(|e| e.text.as_deref().is_some_and(|t| t.starts_with("see nope")))
            .map(|e| e.rect)
            .expect("the row is laid out");
        let (cell_w, _) = term::cell_metrics();
        let (x, y) = (row[0] + cell_w * 8.0, row[1] + row[3] / 2.0);

        // Tab stays the terminal's: no link takes keyboard focus from it.
        rt.key(KeyEvent {
            key: Key::Tab,
            mods: Mods::default(),
        });
        rt.frame(1.0 / 60.0);
        assert!(
            !rt.elements().iter().any(|e| e.focused && e.role == "link"),
            "Tab focused a link"
        );

        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert_eq!(
            rt.state.notice.as_deref(),
            Some("nope/zzz.rs: no such file"),
            "a click resolved the file"
        );
    }

    #[test]
    fn an_input_method_composes_at_the_cursor_and_commits_into_the_session() {
        let mut s = State::new(test_handle());
        let key = seed_session(&mut s, "claude", "/work/repo");
        s.focus_session_tab(&key);
        let session = s.session_id(&key).unwrap();
        let (tx, rx) = mpsc::channel();
        let (_ready_tx, ready_rx) = mpsc::channel();
        let mut terminal = term::Terminal::new(session, rx, ready_rx);
        let data: String = (0..200).map(|n| format!("row{n}\r\n")).collect();
        tx.send(daemon::Output {
            seq: 0,
            data: data.into_bytes(),
        })
        .unwrap();
        terminal.feed();
        attach(&mut s, &key, terminal);
        let pane_key = key.clone();
        let mut rt = Runtime::new(s, view, (1200.0, 800.0), Env::new(true, 2.0));
        rt.poll = Some(poll);
        for _ in 0..6 {
            rt.frame(1.0 / 60.0);
        }
        // The window turns the input method on at the focused pane's cursor.
        assert!(
            rt.ime_area.is_some(),
            "the cursor asks for the input method"
        );

        rt.update(|s, tx| {
            assert!(on_ime(s, &rune::Ime::Preedit("vie".into(), None), tx));
        });
        rt.frame(1.0 / 60.0);
        assert!(
            els(&rt).iter().any(|e| e.text.as_deref() == Some("vie")),
            "the marked text is drawn at the cursor"
        );

        let top = |rt: &mut Runtime<State>| {
            let mut text = String::new();
            rt.update(|s, _| text = s.attachments[&pane_key].lines[0].text());
            text.trim_end().to_string()
        };
        let live = top(&mut rt);
        rt.wheel_event(rune::WheelEvent::new(0.0, 160.0).at(700.0, 400.0));
        rt.frame(1.0 / 60.0);
        assert_ne!(top(&mut rt), live, "the view is in the history");
        rt.update(|s, tx| {
            assert!(on_ime(s, &rune::Ime::Commit("việt".into()), tx));
        });
        rt.frame(1.0 / 60.0);
        assert!(
            rt.state.preedit.is_empty(),
            "the commit ended the composition"
        );
        assert_eq!(top(&mut rt), live, "the commit typed into the live session");
    }

    #[test]
    fn the_last_terminal_row_stays_inside_its_pane() {
        // The regression the missing Claude input frame reported: the
        // grid's bottom row must land inside the pane, at full width and
        // squeezed.
        for (w, h) in [(1512.0, 814.0), (900.0, 560.0)] {
            let mut s = State::new(test_handle());
            let key = seed_session(&mut s, "claude", "/work/repo");
            s.focus_session_tab(&key);
            let session = s.session_id(&key).unwrap();
            let (tx, rx) = mpsc::channel();
            let (_ready_tx, ready_rx) = mpsc::channel();
            let terminal = term::Terminal::new(session, rx, ready_rx);
            attach(&mut s, &key, terminal);
            let pane_key = key.clone();
            let mut seq = 0u64;
            feed_tui_frame(&tx, &mut seq);

            let mut rt = Runtime::new(s, view, (w, h), Env::new(true, 2.0));
            rt.poll = Some(poll);
            rt.frame(0.0);
            // The first layout measures the pane and resizes the grid;
            // the next frames let the pane and its rows settle.
            for _ in 0..4 {
                rt.frame(1.0 / 60.0);
            }

            let mut rows = 0;
            rt.update(|s, _| {
                rows = s.attachments.get(&pane_key).unwrap().rows;
            });
            feed_last_row_marker(&tx, &mut seq, rows);
            // The poll consumes the chunk (a burst opens), then the
            // repaint waits for the output to settle.
            rt.frame(1.0 / 60.0);
            std::thread::sleep(std::time::Duration::from_millis(30));
            rt.frame(1.0 / 60.0);

            // Rows of the old, taller grid fade out over a few frames:
            // let them go before measuring what is mounted.
            for _ in 0..30 {
                std::thread::sleep(std::time::Duration::from_millis(20));
                rt.frame(1.0 / 60.0);
            }
            let (rows, pane_rect) = pane_rows(&rt, &pane_key);
            assert!(!rows.is_empty(), "the pane drew its grid at {w}x{h}");
            let pane_bottom = pane_rect[1] + pane_rect[3];
            for row in &rows {
                assert!(
                    row.rect[3] <= 17.0,
                    "a terminal row is {:.1}pt tall at {w}x{h}",
                    row.rect[3]
                );
                assert!(
                    row.rect[1] + row.rect[3] <= pane_bottom + 0.5,
                    "a terminal row sits below its pane: {:?} vs {pane_bottom} at {w}x{h}",
                    row.rect
                );
            }
            assert!(
                rows.iter().any(|e| e
                    .text
                    .as_deref()
                    .is_some_and(|t| t.contains("PROMPT-MARKER"))),
                "the input row on the grid's last line is mounted at {w}x{h}"
            );
        }
    }

    #[test]
    fn split_panes_keep_their_last_rows_inside() {
        // Two panes side by side: each is narrower, so a wrapped row
        // pushes its bottom rows out sooner — both panes are checked.
        let mut s = State::new(test_handle());
        let a = seed_session(&mut s, "claude-a", "/work/repo");
        s.focus_session_tab(&a);
        let b = seed_session(&mut s, "claude-b", "/work/repo");
        s.place_session(
            &b,
            "claude-b",
            panes::Placement::Split {
                tab: s.open_tabs[0].id.clone(),
                pane: panes::pane_id(&a),
                direction: panes::Direction::SideBySide,
            },
        );
        for key in [&a, &b] {
            let session = s.session_id(key).unwrap();
            let (tx, rx) = mpsc::channel();
            let (_ready_tx, ready_rx) = mpsc::channel();
            let terminal = term::Terminal::new(session, rx, ready_rx);
            let mut seq = 0u64;
            feed_tui_frame(&tx, &mut seq);
            attach(&mut s, key, terminal);
        }
        let (a2, b2) = (a.clone(), b.clone());

        let mut rt = Runtime::new(s, view, (1512.0, 814.0), Env::new(true, 2.0));
        rt.poll = Some(poll);
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        std::thread::sleep(std::time::Duration::from_millis(30));
        rt.frame(1.0 / 60.0);

        for key in [&a2, &b2] {
            let (rows, pane_rect) = pane_rows(&rt, key);
            assert!(!rows.is_empty(), "pane {key} drew its grid");
            let pane_bottom = pane_rect[1] + pane_rect[3];
            for row in &rows {
                assert!(
                    row.rect[3] <= 17.0,
                    "a terminal row is {:.1}pt tall in pane {key}",
                    row.rect[3]
                );
                assert!(
                    row.rect[1] + row.rect[3] <= pane_bottom + 0.5,
                    "a terminal row sits below pane {key}: {:?} vs {pane_bottom}",
                    row.rect
                );
            }
        }
    }
    #[test]
    fn project_terminals_filter_rows_tabs_and_shortcuts_and_restore_split_focus() {
        let mut s = sidebar_scene();
        let a = seed_session_in(&mut s, "/work/a/web", Some("p1"));
        let b = seed_session_in(&mut s, "/work/b/api", Some("p2"));
        let a2 = seed_session_in(&mut s, "/work/a/feat-a", Some("p1"));
        let a3 = seed_session_in(&mut s, "/work/a/feat-a", Some("p1"));
        let unassigned = seed_session_in(&mut s, "/tmp", None);
        for (key, title) in [
            (&a, "A shell"),
            (&b, "B shell"),
            (&a2, "A worktree"),
            (&a3, "A split"),
            (&unassigned, "Unassigned shell"),
        ] {
            s.data
                .sessions
                .iter_mut()
                .find(|view| &view.info.id == key)
                .unwrap()
                .info
                .title = title.into();
        }
        s.focus_session_tab(&a);
        s.focus_session_tab(&b);
        s.focus_session_tab(&a2);
        let split_tab = s.open_tabs[2].id.clone();
        s.place_session(
            &a3,
            "A split",
            panes::Placement::Split {
                tab: split_tab.clone(),
                pane: panes::pane_id(&a2),
                direction: panes::Direction::SideBySide,
            },
        );
        editor::navigation::select_checkout(&mut s, "p1".into(), PathBuf::from("/work/a/web"));
        let mut rt = key_runtime(s);
        let live = els(&rt)
            .into_iter()
            .filter(|el| !el.exiting)
            .collect::<Vec<_>>();
        for key in ["sidebar-project:p1", "sidebar-project:p2"] {
            assert!(live.iter().any(|el| el.key.as_deref() == Some(key)));
        }
        assert!(
            live.iter()
                .any(|el| el.text.as_deref() == Some("A worktree"))
        );
        for hidden in ["B shell", "Unassigned shell"] {
            assert!(!live.iter().any(|el| el.text.as_deref() == Some(hidden)));
        }
        assert_eq!(
            live.iter()
                .filter(|el| el.key.as_deref().is_some_and(|key| key.starts_with("tab:")))
                .count(),
            2
        );
        press_cmd(&mut rt, '1');
        assert_eq!(rt.state.selected_session.as_deref(), Some(a.as_str()));
        press_cmd(&mut rt, '2');
        assert_eq!(rt.state.selected_session.as_deref(), Some(a3.as_str()));
        press_cmd(&mut rt, '3');
        assert_eq!(rt.state.selected_session.as_deref(), Some(a3.as_str()));
        click_key(&mut rt, "sidebar-project:p2");
        assert_eq!(rt.state.visible_tabs().count(), 1);
        assert_eq!(rt.state.selected_session.as_deref(), Some(b.as_str()));
        click_key(&mut rt, "sidebar-project:p1");
        assert_eq!(rt.state.selected_session.as_deref(), Some(a3.as_str()));
        assert_eq!(rt.state.open_tabs[rt.state.active_tab].id, split_tab);
        assert!(matches!(
            &rt.state.open_tabs[rt.state.active_tab].layout,
            panes::Layout::Split(_)
        ));
        assert_eq!(rt.state.open_tabs.len(), 3);
        assert_eq!(rt.state.data.sessions.len(), 5);
        assert!(
            rt.state.pending.is_empty(),
            "switching does not stop or delete sessions"
        );
    }

    #[test]
    fn project_terminals_empty_project_has_no_selection_input_or_controls() {
        let mut s = sidebar_scene();
        let a = seed_session_in(&mut s, "/work/a/web", Some("p1"));
        s.focus_session_tab(&a);
        editor::navigation::select_checkout(&mut s, "p1".into(), PathBuf::from("/work/a/web"));
        let mut rt = key_runtime(s);
        assert!(
            els(&rt)
                .iter()
                .any(|el| el.key.as_deref() == Some("controls-bar"))
        );
        click_key(&mut rt, "sidebar-project:p2");
        assert!(rt.state.selected_session.is_none());
        assert!(rt.state.focused_terminal_session().is_none());
        assert!(rt.state.focused_session_key().is_none());
        assert!(!rt.state.can_split());
        assert_eq!(
            rt.state.active_workspace,
            Some(data::checkout_key("p2", "/work/b/api"))
        );
        let live = els(&rt)
            .into_iter()
            .filter(|el| !el.exiting)
            .collect::<Vec<_>>();
        assert!(live.iter().any(|el| {
            el.text
                .as_deref()
                .is_some_and(|text| text.starts_with("No terminal sessions"))
        }));
        assert!(
            !live
                .iter()
                .any(|el| el.key.as_deref() == Some("controls-bar"))
        );
        press_cmd(&mut rt, '1');
        assert!(rt.state.selected_session.is_none());
        click_key(&mut rt, "sidebar-project:p1");
        assert_eq!(rt.state.selected_session.as_deref(), Some(a.as_str()));
    }

    #[test]
    fn project_terminals_editor_picker_and_background_placement_keep_project_focus() {
        let mut s = sidebar_scene();
        let a = seed_session_in(&mut s, "/work/a/web", Some("p1"));
        let b = seed_session_in(&mut s, "/work/b/api", Some("p2"));
        s.focus_session_tab(&a);
        s.focus_session_tab(&b);
        editor::navigation::select_project(&mut s, "p2".into());
        let info = protocol::session::SessionInfo {
            id: protocol::session::SessionId::new(),
            title: "late A".into(),
            argv: vec!["/bin/zsh".into()],
            cwd: "/work/a/feat-a".into(),
            state: protocol::session::SessionState::Running,
            pid: None,
            exit_code: None,
            error: None,
            started_at: 0,
            ended_at: None,
        };
        let late = data::SessionInfo::key(info.id);
        let (reply_tx, reply) = std::sync::mpsc::channel();
        s.pending.push(sessions::Pending::Create {
            reply,
            launch: sessions::SessionLaunch {
                argv: info.argv.clone(),
                cwd: info.cwd.clone(),
                project_id: Some("p1".into()),
                agent: "shell".into(),
                title: info.title.clone(),
            },
            placement: panes::Placement::Tab,
            workspace_id: None,
        });
        reply_tx.send(Ok(info)).unwrap();
        assert!(s.drain_pending());
        assert_eq!(s.selected_session.as_deref(), Some(b.as_str()));
        assert_eq!(s.visible_tabs().count(), 1);
        editor::navigation::select_project(&mut s, "p1".into());
        assert_eq!(s.visible_tabs().count(), 2);
        assert_eq!(s.selected_session.as_deref(), Some(a.as_str()));
        s.remove_session_everywhere(&a);
        assert_eq!(s.selected_session.as_deref(), Some(late.as_str()));
        assert_eq!(s.focused_terminal_session(), s.session_id(&late));
        s.remove_session_everywhere(&late);
        assert!(s.selected_session.is_none());
        assert!(s.focused_terminal_session().is_none());
        assert!(s.session_placed(&b));
    }

    #[test]
    fn project_map_opens_from_shortcut_or_button_and_switch_hides_foreign_terminal() {
        for button in [false, true] {
            let mut s = two_workspace_scene();
            let sid = seed_session_in(&mut s, "/work/a/web", Some("p1"));
            s.focus_session_tab(&sid);
            let mut rt = key_runtime(s);
            if button {
                click_key(&mut rt, "project-map-button");
            } else {
                press_cmd(&mut rt, 'k');
            }
            assert!(rt.state.project_map.open);
            click_key(&mut rt, "project-map-project:p2");
            assert!(!rt.state.project_map.open);
            assert_eq!(rt.state.workspace_id(), "default");
            assert_eq!(rt.state.editor.project_id.as_deref(), Some("p2"));
            assert_eq!(rt.state.view, View::Terminal);
            assert!(rt.state.selected_session.is_none());
            assert!(rt.state.focused_terminal_session().is_none());
            assert_eq!(rt.state.visible_tabs().count(), 0);
            assert!(rt.state.session_by_key(&sid).is_some());
            assert_eq!(rt.state.open_tabs.len(), 1);
            assert!(rt.state.pending.is_empty());
        }
    }

    #[test]
    fn project_map_types_filters_and_consumes_underlying_shortcuts() {
        let mut rt = key_runtime(two_workspace_scene());
        press_cmd(&mut rt, 'k');
        rt.key(KeyEvent {
            key: Key::Text("API".into()),
            mods: Mods::default(),
        });
        rt.frame(0.1);
        assert_eq!(rt.state.project_map.query.text, "API");
        assert!(
            els(&rt)
                .iter()
                .any(|el| el.key.as_deref() == Some("project-map-project:p2"))
        );
        assert!(
            !els(&rt)
                .iter()
                .any(|el| el.key.as_deref() == Some("project-map-project:p1"))
        );
        press_cmd(&mut rt, 't');
        press_cmd(&mut rt, ',');
        assert_eq!(rt.state.view, View::Terminal);
        assert!(rt.state.pending.is_empty());
        rt.key(KeyEvent {
            key: Key::Down,
            mods: Mods::default(),
        });
        rt.key(KeyEvent {
            key: Key::Enter,
            mods: Mods::default(),
        });
        rt.frame(0.2);
        assert_eq!(rt.state.editor.project_id.as_deref(), Some("p2"));
    }

    #[test]
    fn project_map_shortcut_respects_settings_ime_modifiers_and_modal_owners() {
        type Block = fn(&mut State);
        let blocks: [Block; 9] = [
            |s| s.view = View::Settings,
            |s| s.preedit = "đ".into(),
            |s| s.editor.project_picker.open = true,
            |s| s.editor.checkout_picker.open = true,
            |s| s.editor.quick_open.open = true,
            |s| s.editor.history.branch_picker.open = true,
            |s| s.workspace_remove_confirm = Some("w1".into()),
            |s| s.editor.commit.agent_picker.open = true,
            |s| s.workspace_picker.open = true,
        ];
        for block in blocks {
            let mut s = two_workspace_scene();
            block(&mut s);
            let mut tx = Tx::default();
            on_key(&mut s, &cmd_key('k'), &mut tx);
            assert!(!s.project_map.open);
        }
        for mods in [
            Mods {
                cmd: true,
                shift: true,
                ..Mods::default()
            },
            Mods {
                cmd: true,
                alt: true,
                ..Mods::default()
            },
            Mods {
                cmd: true,
                ctrl: true,
                ..Mods::default()
            },
        ] {
            let mut s = two_workspace_scene();
            on_key(
                &mut s,
                &KeyEvent {
                    key: Key::Command('k'),
                    mods,
                },
                &mut Tx::default(),
            );
            assert!(!s.project_map.open);
        }
    }

    #[test]
    fn project_map_dismissal_and_editor_focus_restore() {
        let mut s = two_workspace_scene();
        s.view = View::Editor;
        let mut rt = key_runtime(s);
        for close_button in [false, true] {
            press_cmd(&mut rt, 'k');
            assert!(rt.state.project_map.open);
            if close_button {
                click_label(&mut rt, "Close");
            } else {
                rt.key(KeyEvent {
                    key: Key::Escape,
                    mods: Mods::default(),
                });
            }
            rt.frame(0.2);
            assert!(!rt.state.project_map.open);
            assert_eq!(rt.state.workspace_id(), "w1");
            assert!(rt.state.editor.project_id.is_none());
            assert_eq!(rt.state.view, View::Editor);
        }
    }

    #[test]
    fn project_map_fits_narrow_window_in_both_themes() {
        for theme in [theme::Theme::Light, theme::Theme::Dark] {
            let mut s = two_workspace_scene();
            s.theme_choice = theme;
            s.apply_theme();
            project_map::open(&mut s, &mut Tx::default());
            let mut rt = Runtime::new(
                s,
                view,
                (760.0, 480.0),
                Env::new(theme == theme::Theme::Dark, 2.0),
            );
            rt.frame(0.0);
            let card = els(&rt)
                .into_iter()
                .find(|e| e.key.as_deref() == Some("project-map-dialog"))
                .unwrap();
            assert!(card.rect[0] >= 0.0 && card.rect[1] >= 0.0);
            assert!(card.rect[0] + card.rect[2] <= 760.0);
            assert!(card.rect[1] + card.rect[3] <= 480.0);
        }
    }

    #[test]
    fn project_map_paste_stays_in_search_and_sidebar_reveals_distant_target() {
        let mut s = two_workspace_scene();
        for i in 0..35 {
            s.editor.projects.push(project(
                &format!("extra-{i}"),
                &format!("Extra {i}"),
                &format!("/tmp/extra-{i}"),
            ));
        }
        let mut rt = key_runtime(s);
        press_cmd(&mut rt, 'k');
        rt.paste("Extra 34");
        rt.frame(0.1);
        assert_eq!(rt.state.project_map.query.text, "Extra 34");
        click_key(&mut rt, "project-map-project:extra-34");
        for i in 1..20 {
            rt.frame(0.1 + i as f64 * 0.1);
        }
        assert_eq!(rt.state.workspace_id(), "default");
        let elements = els(&rt);
        let viewport = elements
            .iter()
            .find(|e| e.key.as_deref() == Some(sidebar::PROJECTS_SCROLL_KEY))
            .unwrap();
        let target = elements
            .iter()
            .find(|e| e.key.as_deref() == Some("sidebar-project:extra-34"))
            .unwrap();
        assert!(
            target.rect[1] >= viewport.rect[1],
            "target above viewport: {:?}",
            target.rect
        );
        assert!(
            target.rect[1] + target.rect[3] <= viewport.rect[1] + viewport.rect[3] + 1.0,
            "target below viewport: {:?}, {:?}",
            target.rect,
            viewport.rect
        );
    }
    #[test]
    fn project_map_ime_composes_in_search_and_fallback_cannot_type_into_terminal() {
        let mut rt = key_runtime(two_workspace_scene());
        press_cmd(&mut rt, 'k');
        rt.ime(rune::Ime::Preedit("vie".into(), None));
        rt.frame(0.1);
        assert!(rt.state.preedit.is_empty());
        rt.ime(rune::Ime::Commit("việt".into()));
        rt.frame(0.2);
        assert_eq!(rt.state.project_map.query.text, "việt");
        assert!(rt.state.preedit.is_empty());
        rt.update(|s, tx| assert!(on_ime(s, &rune::Ime::Commit("blocked".into()), tx)));
        assert_eq!(rt.state.project_map.query.text, "việt");
    }

    #[test]
    fn project_map_scrim_dismissal_restores_focused_trigger_and_tab_is_trapped() {
        let mut rt = key_runtime(two_workspace_scene());
        click_key(&mut rt, "project-map-button");
        rt.key(KeyEvent {
            key: Key::Tab,
            mods: Mods::default(),
        });
        rt.frame(0.1);
        let focused = els(&rt)
            .into_iter()
            .filter(|e| e.focused)
            .collect::<Vec<_>>();
        assert!(
            focused.iter().any(|e| e.label.as_deref() == Some("Close")),
            "{focused:?}"
        );
        rt.pointer_down(20.0, 400.0);
        rt.pointer_up(20.0, 400.0);
        rt.frame(0.2);
        assert!(!rt.state.project_map.open);
        assert!(
            els(&rt)
                .iter()
                .any(|e| e.focused && e.key.as_deref() == Some("project-map-button"))
        );
    }

    #[test]
    fn project_map_status_distinguishes_loading_errors_empty_and_no_matches() {
        for (kind, want) in [
            (0, "Loading projects and workspaces…"),
            (1, "read failed"),
            (2, "No registered projects"),
            (3, "No matching projects or workspaces"),
        ] {
            let mut s = two_workspace_scene();
            project_map::open(&mut s, &mut Tx::default());
            match kind {
                0 => s.editor.projects_loading = true,
                1 => s.workspaces_error = Some("read failed".into()),
                2 => s.editor.projects.clear(),
                _ => s.project_map.query.text = "no-such-project".into(),
            }
            let mut rt = Runtime::new(s, view, (760.0, 480.0), Env::new(true, 2.0));
            rt.frame(0.0);
            assert!(
                texts_on(&rt).iter().any(|t| t == want),
                "{kind}: {:?}",
                texts_on(&rt)
            );
        }
    }
}
