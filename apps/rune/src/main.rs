//! goko-rune: a Rune (wgpu/winit) port of the Goko desktop shell — header,
//! project sidebar, workspace tab strip, controls, a terminal stub and the
//! native editor workspace backed by `app-core` (real projects, Git sections,
//! drafts with Save/Discard/Cancel guards).

mod askpass;
mod chrome;
mod daemon;
mod data;
mod editor;
mod handoff;
mod nested_import;
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
    /// When the activity read still unanswered was queued. No second one is
    /// queued behind it, so a busy worker is not handed a backlog of polls.
    activity_pending: Option<f64>,
    /// The same for the sub-agent read.
    subagents_pending: Option<f64>,
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
    /// The saved tab layout as read from the settings store, until it is restored
    /// (`Some(None)`: the read found nothing).
    tabs_pending: Option<Option<String>>,
    /// The daemon's first session list has arrived.
    sessions_loaded: bool,
    /// The restored or last acknowledged layout; `None` until restore runs
    /// so an empty start cannot overwrite it.
    tabs_saved: Option<String>,
    tabs_save_pending: Option<String>,
    tabs_save_retry_at: f64,
    /// The tab bar's drag to reorder, in positions among the visible tabs.
    tab_reorder: rune_kit::shell::Reorder,
    /// A pane has the pointer pressed in a drag: its program hears the drag's own
    /// moves, not hovers.
    pane_drag_held: bool,
    /// The mouse reports sent to programs, for the tests to read.
    #[cfg(test)]
    mouse_sent: Vec<Vec<u8>>,
    /// How many terminal output subscriptions were asked of the daemon.
    #[cfg(test)]
    output_subscribes: usize,
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
    /// The SSH passphrase prompt a running Push is waiting on, if any.
    askpass: Option<askpass::AskpassState>,
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
    /// What a find bar's query field edits when its pane's attachment is gone
    /// before the field's event arrives; never shown.
    find_idle: rune::edit::TextEdit,
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
    /// Which edge the sidebar docks against — left or right.
    sidebar_side: sidebar::Side,
    /// The user changed the sidebar side since launch, so the pending startup
    /// read must not overwrite it.
    sidebar_side_chosen: bool,
    /// Which side the Files tab's file tree sits on — left or right.
    list_aside_side: sidebar::Side,
    /// The user changed the list aside side since launch, so the pending
    /// startup read must not overwrite it.
    list_aside_side_chosen: bool,
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
    needs_you_picker: rune_kit::ComboboxState,
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
    /// Add Project's nested-repository review dialog.
    pub nested_import: nested_import::NestedImportState,
    /// The Hand off dialog for one tab's focused pane.
    pub handoff: handoff::HandoffState,
    /// Monotonic token for the next handoff request.
    pub handoff_token: u64,
    /// The approved handoff text, copied on the next poll once its session
    /// exists (the create's reply lands without a `Tx`).
    pub handoff_copy: Option<String>,
    /// Monotonic token for the next nested scan/import request.
    pub nested_import_token: u64,
    /// Project to select once an import reply's projects are refreshed.
    pub nested_import_select: Option<String>,
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
    /// The open Agents usage dropdown (`"agents"`); it anchors to the header's
    /// Agents button.
    usage_menu: Option<String>,
    /// The per-provider usage records the worker last read with the usage
    /// snapshot (`app_core::usage_records`): what Goko's own collectors and
    /// any other collector on this machine wrote.
    agent_usage: Vec<app_core::usage_records::UsageRecord>,
    /// Goko's own usage collectors (`app_core::usage_collect`), run on a
    /// thread of their own when the dropdown opens and on Refresh.
    usage_collector: std::sync::Arc<app_core::usage_collect::Collector>,
    /// Set by a collector run when it ends, so the next poll reads what it
    /// wrote at once instead of on the five-second cadence.
    usage_collected: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// What the check for a newer release found (Settings → Updates).
    update: update::Status,
    /// Where "Update now" installs over this copy; `None` when this copy can
    /// only be updated by hand (`update::install_dir`).
    update_dir: Option<std::path::PathBuf>,
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
            tabs_pending: None,
            sessions_loaded: false,
            tabs_saved: None,
            tabs_save_pending: None,
            tabs_save_retry_at: 0.0,
            tab_reorder: rune_kit::shell::Reorder::new().gap(4.0),
            pane_drag_held: false,
            #[cfg(test)]
            mouse_sent: Vec::new(),
            #[cfg(test)]
            output_subscribes: 0,
            project_tabs: HashMap::new(),
            worktree_groups: HashMap::new(),
            worktrees_open: HashMap::new(),
            view: View::Terminal,
            editor,
            askpass: None,
            daemon,
            session_metas: HashMap::new(),
            pending: Vec::new(),
            daemon_version: None,
            daemon_reason: None,
            terminal_lagged: HashSet::new(),
            attachments: HashMap::new(),
            find_idle: rune::edit::TextEdit::new(""),
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
            sidebar_side: sidebar::Side::Left,
            sidebar_side_chosen: false,
            list_aside_side: sidebar::Side::Left,
            list_aside_side_chosen: false,
            sidebar_menu: None,
            sidebar_cursor: None,
            last_link_open: None,
            workspace_picker: rune_kit::ComboboxState::default(),
            needs_you_picker: rune_kit::ComboboxState::default(),
            new_session_picker: rune_kit::ComboboxState::default(),
            new_session_from: None,
            sidebar_input: None,
            sidebar_text: rune::edit::TextEdit::new(""),
            workspaces: Vec::new(),
            workspaces_loading: false,
            workspaces_error: None,
            selected_workspace: None,
            project_map: Default::default(),
            nested_import: Default::default(),
            handoff: Default::default(),
            handoff_token: 0,
            handoff_copy: None,
            nested_import_token: 0,
            nested_import_select: None,
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
            activity_pending: None,
            subagents_pending: None,
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
            agent_usage: Vec::new(),
            usage_collector: Default::default(),
            usage_collected: Default::default(),
            update: update::Status::default(),
            update_dir: update::install_dir(),
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

    /// Save which edge the sidebar docks against, so it survives the next
    /// launch — the same worker-lane settings write as the width.
    pub fn persist_sidebar_side(&mut self) {
        let queued = backend::enqueue(
            &mut self.editor.backend,
            backend::Job::project(backend::Req::SetSetting {
                key: sidebar::SIDEBAR_SIDE_KEY.to_string(),
                value: self.sidebar_side.store_value().to_string(),
            }),
        );
        if !queued {
            self.notice =
                Some("the editor queue is full; the sidebar position was not saved".into());
        }
    }

    /// Move the project sidebar to the left or right edge and persist the
    /// choice; the chosen flag keeps the pending startup read from overwriting
    /// a side picked before that read lands.
    pub fn set_sidebar_side(&mut self, side: sidebar::Side) {
        if self.sidebar_side == side {
            return;
        }
        self.sidebar_side = side;
        self.sidebar_side_chosen = true;
        self.persist_sidebar_side();
    }

    /// Move the Files tab's file tree to the left or right edge and persist
    /// the choice through the same worker-lane settings write as the width.
    pub fn set_list_aside_side(&mut self, side: sidebar::Side) {
        if self.list_aside_side == side {
            return;
        }
        self.list_aside_side = side;
        self.list_aside_side_chosen = true;
        let queued = backend::enqueue(
            &mut self.editor.backend,
            backend::Job::project(backend::Req::SetSetting {
                key: editor::files::LIST_ASIDE_SIDE_KEY.to_string(),
                value: side.store_value().to_string(),
            }),
        );
        if !queued {
            self.notice = Some("the editor queue is full; the list position was not saved".into());
        }
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

    /// Sessions waiting for the user, grouped by workspace.
    pub fn workspace_waiting_counts(&self) -> HashMap<String, usize> {
        let mut counts = HashMap::new();
        for session in self.waiting_sessions() {
            *counts
                .entry(self.session_owner_workspace(session).to_string())
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
        self.workspace_picker.open || self.needs_you_picker.open || self.new_session_picker.open
    }

    /// Whether the open prompt edits a workspace name.
    pub fn workspace_input_open(&self) -> bool {
        matches!(
            self.sidebar_input,
            Some(sidebar::SidebarInput::WorkspaceRename { .. })
        )
    }

    /// Sessions with no project that sit inside `project`'s root join it.
    /// One that already belongs to a project stays put, so its tab is not
    /// pulled out from under the project filter.
    fn join_sessions_to_project(&mut self, project: &app_core::projects::ProjectView) {
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
                    backend::Job::project(backend::Req::InsertSessionMeta(meta.clone())),
                );
            }
        }
    }

    /// Apply one backend reply: the app-level kinds land here, the editor's
    /// own kinds in the editor state.
    pub fn apply_reply(&mut self, reply: backend::Reply) {
        // A Push reply ends the passphrase prompt that push was waiting on.
        if matches!(&reply.req, backend::Req::Push) {
            self.askpass = None;
        }
        let scan_token = match &reply.req {
            backend::Req::ScanNestedRepos { token, .. } => Some(*token),
            _ => None,
        };
        if let Some(token) = scan_token {
            match reply.result {
                Ok(backend::ReplyData::NestedScan {
                    token: reply_token,
                    result,
                }) if reply_token == token => nested_import::apply_scan(self, token, result),
                Err(error) => nested_import::apply_error(self, token, error),
                _ => {}
            }
            return;
        }
        let import_token = match &reply.req {
            backend::Req::ImportNestedRepos { token, .. } => Some(*token),
            _ => None,
        };
        if let Some(token) = import_token {
            match reply.result {
                Ok(backend::ReplyData::NestedImport {
                    token: reply_token,
                    result,
                }) if reply_token == token => nested_import::apply_import(self, token, result),
                Err(error) => nested_import::apply_error(self, token, error),
                _ => {}
            }
            return;
        }
        match &reply.req {
            backend::Req::PrepareHandoff { token, .. } => {
                let token = *token;
                let result = match reply.result {
                    Ok(backend::ReplyData::HandoffPrepared {
                        prepared,
                        fork_accounts,
                    }) => Ok((*prepared, fork_accounts)),
                    Ok(_) => return,
                    Err(error) => Err(error),
                };
                handoff::apply_prepared(self, token, result);
                return;
            }
            backend::Req::HandoffArgv { token, .. } => {
                let token = *token;
                let result = match reply.result {
                    Ok(backend::ReplyData::HandoffArgv { argv, cwd }) => Ok((argv, cwd)),
                    Ok(_) => return,
                    Err(error) => Err(error),
                };
                handoff::apply_argv(self, token, result);
                return;
            }
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
                self.subagents_pending = None;
                if let Ok(backend::ReplyData::Subagents { agents, debug }) = &reply.result {
                    self.subagents = agents.clone();
                    self.debug = debug.clone();
                    self.debug_at = Some(self.clock);
                }
                return;
            }
            backend::Req::Activity { .. } => {
                self.activity_pending = None;
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
                if let Ok(backend::ReplyData::Usage {
                    snapshot,
                    accounts,
                    records,
                }) = reply.result
                {
                    self.data.usage = Some(*snapshot);
                    self.data.accounts = Some(accounts);
                    self.agent_usage = records;
                }
                return;
            }
            backend::Req::AccountAdd { agent, token, .. } => {
                match reply.result {
                    Ok(backend::ReplyData::Accounts(view)) => {
                        // Main's add flow opens sign-in for the new account —
                        // `add` leaves it active, so `active` names it.
                        let id = view.agent(agent).map(|provider| provider.active.clone());
                        self.data.accounts = Some(view);
                        // The drafts are cleared by the reply that belongs
                        // to this submission only: a stale one must never
                        // wipe a newer form.
                        if self.settings.finish_account_add(*token)
                            && let Some(id) = id
                        {
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
                    // The sign-in tab opens behind Settings; show it so the
                    // click has a visible result.
                    self.view = View::Terminal;
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
                    self.join_sessions_to_project(project);
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
            backend::Req::GetSetting { key } if key == panes::TABS_KEY => {
                self.tabs_pending = Some(match &reply.result {
                    Ok(backend::ReplyData::Setting(raw)) => raw.clone(),
                    _ => None,
                });
                self.restore_tabs();
                return;
            }
            backend::Req::SetSetting { key, .. } if key == panes::TABS_KEY => {
                self.tabs_save_finished(matches!(reply.result, Ok(backend::ReplyData::Ok)));
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
            backend::Req::GetSetting { key } if key == sidebar::SIDEBAR_SIDE_KEY => {
                // Do not let the startup read overwrite a side picked before it lands.
                if !self.sidebar_side_chosen
                    && let Ok(backend::ReplyData::Setting(Some(raw))) = &reply.result
                {
                    self.sidebar_side = sidebar::Side::parse(raw);
                }
                return;
            }
            backend::Req::GetSetting { key } if key == editor::files::LIST_ASIDE_SIDE_KEY => {
                if !self.list_aside_side_chosen
                    && let Ok(backend::ReplyData::Setting(Some(raw))) = &reply.result
                {
                    self.list_aside_side = sidebar::Side::parse(raw);
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

/// Root: header full-width, then [sidebar | 8 px separator | workspace
/// column], with the sidebar docked left or right per `sidebar_side`.
/// Settings replaces the whole column like App.tsx's settingsMode;
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
        .child({
            let body = row().grow(1.0).min_h(0.0).w_full();
            let bar = sidebar::sidebar(s, ink, &mut layer);
            let rule = sidebar::separator(s, ink);
            if s.sidebar_side == sidebar::Side::Left {
                body.child(bar).child(rule).child(workspace)
            } else {
                body.child(workspace).child(rule).child(bar)
            }
        });
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
    if let Some(menu) = editor::files::code_menu_layer(s, &mut layer) {
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
    for import_layer in nested_import::layers(s) {
        root = root.child(import_layer);
    }
    for handoff_layer in handoff::layers(s) {
        root = root.child(handoff_layer);
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
    // A running Push's SSH passphrase prompt mounts above every view and
    // overlay; the push keeps running across view switches.
    askpass::modal(s, ink, &s.editor.kit, &mut layer);
    // Paste with no text field focused belongs to the focused terminal pane.
    // It hears every clipboard form: a lone image (a screenshot, Copy Image)
    // has no text, so a text-only handler never fired for it.
    rune_kit::root(root, layer).on_paste_content(
        |s: &mut State, content: &rune::ClipboardContent, _tx: &mut Tx| {
            if s.view == View::Terminal
                && !s.project_map.open
                && !s.nested_import.open
                && s.sidebar_input.is_none()
                && s.tab_menu.is_none()
                && s.askpass.is_none()
                && let Some(session) = s.focused_terminal_session()
            {
                s.paste_clipboard_into_terminal(session, content);
            }
        },
    )
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
    // The sidebar's docked side survives restarts too.
    let _ = backend::enqueue(
        &mut s.editor.backend,
        backend::Job::project(backend::Req::GetSetting {
            key: sidebar::SIDEBAR_SIDE_KEY.to_string(),
        }),
    );
    // The Files tab's tree side survives restarts too.
    let _ = backend::enqueue(
        &mut s.editor.backend,
        backend::Job::project(backend::Req::GetSetting {
            key: editor::files::LIST_ASIDE_SIDE_KEY.to_string(),
        }),
    );
    // The tabs' layout comes back once the daemon's session list is in too.
    let _ = backend::enqueue(
        &mut s.editor.backend,
        backend::Job::project(backend::Req::GetSetting {
            key: panes::TABS_KEY.to_string(),
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
    // The Vim switch restores on the same settings lane.
    let _ = backend::enqueue(
        &mut s.editor.backend,
        backend::Job::project(backend::Req::GetSetting {
            key: editor::vim::VIM_KEY.to_string(),
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
    s.save_tabs_if_changed();
    if s.tabs_save_retry_at > now {
        tx.wake_after(std::time::Duration::from_secs_f64(
            s.tabs_save_retry_at - now,
        ));
    }
    // The replies the session requests are waiting for.
    changed |= s.drain_pending();
    // A handoff's approved text goes to the clipboard only now that its
    // session exists.
    if let Some(text) = s.handoff_copy.take() {
        tx.copy(text);
        changed = true;
    }
    // Closing panes drive their own stop→delete lifecycle.
    changed |= s.poll_closes();
    changed |= s.poll_stop_escalation();
    // Every pane of the active tab holds its own terminal attachment;
    // panes that left the view detach.
    changed |= s.sync_terminals();
    let term_changed = s.poll_terminals(tx);
    changed |= term_changed;
    if crate::trace::enabled() {
        crate::trace::trace(&s.terminal_trace());
    }
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
    changed |= editor::search::poll(s, now, tx);
    if let Some(pending) = s.editor.pending_apply.take() {
        // Route through the guard: a save that landed clean applies
        // immediately; anything still dirty re-raises the dialog.
        editor::navigation::navigate(s, pending, tx);
        changed = true;
    }
    changed |= terminal_branch::poll(s, now, tx);
    changed |= editor::branch::poll(&mut s.editor, s.view == View::Editor, now, tx);
    changed |= editor::autorefresh::poll(
        &mut s.editor,
        s.view == View::Editor,
        s.window_focused,
        now,
        tx,
    );
    editor::highlight::poll(&mut s.editor, tx);
    // The inline-blame column's relative ages roll over once a minute.
    changed |= editor::blame::poll(&mut s.editor);
    // LSP: drain runtime messages, reconcile document sync, drive hover.
    changed |= editor::lsp::poll(&mut s.editor, tx);
    // Quick-open: rebuild matches when the path cache or checkout moved.
    changed |= editor::quick_open::poll(s);
    changed |= project_map::poll(s);
    if let Some(project_id) = s.nested_import_select.clone()
        && s.editor
            .projects
            .iter()
            .any(|project| project.project.id == project_id)
    {
        s.nested_import_select = None;
        editor::navigation::navigate(
            s,
            editor::navigation::PendingNav::SelectMappedProject { project_id },
            tx,
        );
        changed = true;
    }
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
    s.editor.indexer.stop();
    // Queue any newer layout behind the in-flight write before the worker drains.
    s.queue_tabs_save();
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
    if s.nested_import.open {
        return nested_import::key(s, ev, tx);
    }
    if s.handoff.open {
        return handoff::key(s, ev, tx);
    }
    // The sidebar's prompt, or one of its searchable selects, owns the
    // keyboard while it is open: its field takes the typing, not the shell.
    if s.sidebar_input.is_some() || s.sidebar_picker_open() {
        return false;
    }
    // App shortcuts come before the Terminal forwards keys to the shell.
    if ev.mods.cmd && !ev.mods.ctrl {
        if matches!(ev.key, rune::Key::Command('c')) {
            if s.view == View::Terminal && s.copy_pane_selection(tx) {
                return true;
            }
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
        // The find bar's query takes the typing while it is focused, not the shell.
        if s.terminal_find_focused() {
            return false;
        }
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
        || s.nested_import.open
        || s.handoff.open
        || s.editor.guard.is_some()
        || s.askpass.is_some()
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
        'f' if s.view == View::Terminal => s.open_terminal_find(),
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
    if s.project_map.open || s.nested_import.open || s.handoff.open || s.tab_menu.is_some() {
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
    // ssh runs this binary as its askpass helper during a Push: answer on the
    // bridge and exit before any GUI, login shell or daemon starts.
    if askpass::is_helper() {
        askpass::run_helper();
    }
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
mod view_tests;
