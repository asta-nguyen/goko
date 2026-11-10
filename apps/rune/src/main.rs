//! goko-rune: a Rune (wgpu/winit) port of the Goko desktop shell — header,
//! project sidebar, workspace tab strip, controls, a terminal stub and the
//! native editor workspace backed by `app-core` (real projects, Git sections,
//! drafts with Save/Discard/Cancel guards).

mod chrome;
mod daemon;
mod data;
mod editor;
mod panes;
mod sessions;
mod settings;
mod sidebar;
mod term;
mod theme;
mod trace;

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use rune::{AppSpec, Cx, El, Tx, col, row};

use crate::data::Data;
use crate::editor::backend;
use crate::theme::Ink;

/// The system's Reduce Motion setting, mirrored by the view — the only
/// place that can read it. Under it the spinner stands still and no frame
/// is asked for.
static REDUCED_MOTION: AtomicBool = AtomicBool::new(false);

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
    ink: Ink,
    data: Data,
    /// The child process's activity, keyed by session id: what the
    /// session's agent is doing (working, waiting, done).
    activity: HashMap<String, app_core::activity::Activity>,
    /// The agent CLIs running under each session's process tree.
    subagents: app_core::subagents::SubagentMap,
    /// When the sub-agents were last read; the reader polls every two
    /// seconds.
    subagents_at: Option<f64>,
    /// When the activity was last read; the reader polls once a second.
    activity_at: Option<f64>,
    /// The pulse of a waiting child's dot.
    pulse: rune::Live<f32>,
    /// Project id → expanded; absent means expanded (ProjectTree default).
    expanded: HashMap<String, bool>,
    selected_session: Option<String>,
    active_workspace: Option<String>,
    /// Open workspace tabs: each owns a recursive pane tree, so a split
    /// tab's id is never "the tab's session".
    open_tabs: Vec<panes::Tab>,
    active_tab: usize,
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
    /// The wall clock in epoch milliseconds — the elapsed counter's source.
    now_ms: i64,
    /// The app's clock in seconds — the spinner's phase.
    clock: f64,
    /// A notice from the last failed daemon call.
    notice: Option<String>,
    /// The sidebar's width in pixels.
    sidebar_width: f32,
    /// The open sidebar menu: the row it belongs to and what it offers.
    sidebar_menu: Option<(sidebar::RowTarget, sidebar::MenuKind)>,
    /// Where the menu opens — the pointer's last position over the row's
    /// action button.
    menu_anchor: Option<(f32, f32)>,
    /// The text the sidebar asks the user for.
    sidebar_input: Option<sidebar::SidebarInput>,
    /// The text the user typed into the sidebar's prompt.
    sidebar_text: rune::edit::TextEdit,
    /// The launches waiting for their agent's argv, each with its own
    /// token so concurrent requests never steal each other's answer.
    pending_argv: Vec<sessions::PendingArgv>,
    /// The next launch token — monotonic, so a reply always names exactly
    /// one request.
    argv_token: u64,
    /// When each session entered the stopping state, so the force kill can
    /// be offered once a stop has hung.
    stopping_since: HashMap<String, i64>,
    /// When the usage/accounts read was last issued; the worker re-reads
    /// the sources about every five seconds.
    usage_at: Option<f64>,
    /// The open usage-chip account menu: the agent it belongs to and where
    /// the chip was pressed, so the kit menu can anchor there.
    usage_menu: Option<(String, f32, f32)>,
    /// The settings page's own state (section, forms, drafts).
    settings: settings::SettingsState,
    /// The appearance choice restored from `goko.theme`.
    theme_choice: theme::Theme,
    /// What the OS last reported: `true` when dark. Only consulted while
    /// `theme_choice` is `System`.
    os_dark: bool,
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
        State {
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
            now_ms: data::now_ms(),
            clock: 0.0,
            notice: None,
            sidebar_width: sidebar::DEFAULT_SIDEBAR_W,
            sidebar_menu: None,
            menu_anchor: None,
            sidebar_input: None,
            sidebar_text: rune::edit::TextEdit::new(""),
            pending_argv: Vec::new(),
            argv_token: 0,
            stopping_since: HashMap::new(),
            activity: HashMap::new(),
            activity_at: None,
            subagents: Default::default(),
            subagents_at: None,
            pulse: rune::Live::new(1.0),
            usage_at: None,
            usage_menu: None,
            settings: settings::SettingsState::default(),
            theme_choice: theme::Theme::Dark,
            os_dark: true,
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
        for att in self.attachments.values_mut() {
            att.term.refresh_theme();
            att.lines = att.term.lines(&ink).to_vec();
        }
    }

    /// The spinner's angle: the app clock's phase, or a still ring under
    /// Reduce Motion.
    pub fn spinner_angle(&self) -> f32 {
        if REDUCED_MOTION.load(Ordering::Relaxed) {
            return 0.0;
        }
        (self.clock as f32 * 2.4) % std::f32::consts::TAU
    }

    /// `true` while a session works — its dot spins.
    pub fn spinning(&self) -> bool {
        self.data
            .sessions
            .iter()
            .any(|view| view.info.state.working())
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
                }
                return;
            }
            backend::Req::Subagents { .. } => {
                if let Ok(backend::ReplyData::Subagents(subagents)) = &reply.result {
                    self.subagents = subagents.clone();
                }
                return;
            }
            backend::Req::Activity { .. } => {
                if let Ok(backend::ReplyData::Activity(activity)) = &reply.result {
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
            backend::Req::CreateWorktree { project_id, .. } => {
                // The new checkout joins the project's list.
                if reply.result.is_ok() {
                    self.editor.request_worktrees(project_id);
                }
                return;
            }
            backend::Req::RemoveWorktree { project_id, .. } => {
                // The list is stale without the removed checkout.
                if reply.result.is_ok() {
                    self.editor.request_worktrees(project_id);
                }
                return;
            }
            backend::Req::RemoveProject { id } => {
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
    // The view is the only place that can read the system's Reduce Motion
    // setting; the poll needs it to know whether to keep frames coming.
    REDUCED_MOTION.store(cx.system_reduced_motion(), Ordering::Relaxed);
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
                .child(sidebar::sidebar(s, ink))
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
    let mut layer = Vec::new();
    if let Some(menu) = sidebar::menu_layer(s, &mut layer) {
        root = root.child(menu);
    }
    if let Some(prompt) = sidebar::input_layer(s, ink, &mut layer) {
        root = root.child(prompt);
    }
    if let Some(menu) = chrome::usage_menu_layer(s, &mut layer) {
        root = root.child(menu);
    }
    // The New-worktree form is a modal layer too: it mounts above the app
    // while `sidebar_input` shields the keyboard for it.
    if let Some(form) = editor::worktree_form::layer(s, &mut layer) {
        root = root.child(form);
    }
    // A failed call's notice: a toast the user closes. Without it the
    // message was recorded and never shown.
    // ponytail: closed by hand only; auto-expire when notices get noisy.
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
            && s.sidebar_input.is_none()
            && let Some(session) = s.focused_terminal_session()
        {
            s.paste_into_terminal(session, text);
        }
    })
}

/// `AppSpec::setup`: kick the initial project list, history-layout restore,
/// the session metadata and agent reads, the sidebar width restore, and ACP
/// agent discovery once the state exists.
fn setup(s: &mut State, _waker: rune::Waker) {
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
}

/// `AppSpec::poll`: drain the daemon's events and the session replies it
/// is waiting for, then the editor housekeeping — quit-guard swizzle
/// install, cancelled-close reopen, search debounce, save-resume
/// navigation, queued external opens, and pending line reveals.
fn poll(s: &mut State, now: f64, tx: &mut Tx) -> bool {
    let mut changed = false;
    s.clock = now;
    s.now_ms = data::now_ms();
    // The waiting child's dot blinks on the app's clock.
    s.pulse
        .set(0.35 + 0.65 * (0.5 + 0.5 * (now as f32 * 3.0).sin()));
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
    // Every pane of the active tab holds its own terminal attachment;
    // panes that left the view detach.
    changed |= s.sync_terminals();
    changed |= s.poll_terminals(tx);
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
    editor::quit_guard::poll_install();
    editor::navigation::reopen_if_cancelled(s);
    changed |= editor::search::poll(s);
    if let Some(pending) = s.editor.pending_apply.take() {
        // Route through the guard: a save that landed clean applies
        // immediately; anything still dirty re-raises the dialog.
        editor::navigation::navigate(s, pending, tx);
        changed = true;
    }
    editor::highlight::poll(&mut s.editor, tx);
    // The inline-blame column's relative ages roll over once a minute.
    changed |= editor::blame::poll(&mut s.editor);
    // LSP: drain runtime messages, reconcile document sync, drive hover.
    changed |= editor::lsp::poll(&mut s.editor, tx);
    // Quick-open: rebuild matches when the path cache or checkout moved.
    changed |= editor::quick_open::poll(s);
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
    // The quota chips and account labels: read about every five seconds on
    // the worker. The wake keeps the cadence alive while the UI is idle;
    // the `System` theme checks the OS appearance on the same tick.
    changed |= s.poll_usage(now);
    tx.wake_after(std::time::Duration::from_secs(5));
    changed |= settings::poll_appearance(s);
    settings::sync_acp_drafts(s);
    // A working session's dot spins: ask for the next frame so the ring
    // keeps turning. Reduce Motion stops the animation entirely.
    if s.spinning() && !REDUCED_MOTION.load(Ordering::Relaxed) {
        tx.wake_after(std::time::Duration::from_millis(33));
        changed = true;
    }
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
    // The sidebar's prompt owns the keyboard while it is open.
    if s.sidebar_input.is_some() {
        return false;
    }
    if s.view == View::Terminal {
        return terminal_key(s, ev);
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

/// Modifier snapshot for the code owner's glyph-hit link overlays —
/// `EditorState::set_modifiers` mirrors the platform go-to modifier onto the
/// active document's `CodeState`.
fn on_modifiers(s: &mut State, mods: rune::Mods, _tx: &mut Tx) {
    s.editor.set_modifiers(mods);
}

/// The Terminal view's keyboard: every key belongs to the focused pane's
/// session — never "the tab's session", a split tab has several. An app
/// shortcut (⌘ on macOS) is left alone, and an exited session keeps its
/// final grid without taking input.
fn terminal_key(s: &mut State, ev: &rune::KeyEvent) -> bool {
    let Some(session) = s.focused_terminal_session() else {
        return false;
    };
    // A shortcut belongs to the app, not to the shell.
    if ev.mods.cmd && !ev.mods.ctrl {
        return false;
    }
    let Some(bytes) = term::key_bytes(ev) else {
        return false;
    };
    s.type_into_terminal(session, &bytes)
}

/// The input method for the Terminal view: what it composes is drawn at
/// the cursor (`Preedit`) and typed into the focused session once it is
/// final (`Commit`) — Vietnamese Telex and the other marked-text methods.
fn on_ime(s: &mut State, ev: &rune::Ime, _tx: &mut Tx) -> bool {
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

fn main() {
    // A Dock/Finder launch gets launchd's bare PATH, so `claude`/`codex` would not
    // resolve; adopt the login shell's before any thread or the daemon starts.
    app_core::agents::adopt_login_shell_path(&editor::backend::default_shell());
    let app = rune::App::new(AppSpec {
        families: ("Inter", "ui-monospace"),
        min_size: Some((760.0, 480.0)),
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        ..AppSpec::new("Goko", (1512.0, 814.0), init_state, view)
    })
    // Centre the macOS traffic lights in the 48 pt header band.
    .titlebar(rune::Titlebar::new(48.0))
    .keep_running()
    .on_close(on_close)
    .on_quit(on_quit)
    .on_modifiers(on_modifiers)
    .on_ime(on_ime);
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
            .find(|e| e.label.as_deref() == Some("Add project"))
            .expect("Add project button mounted");
        assert!(
            add.rect[0] + add.rect[2] <= 760.0 && add.rect[2] > 0.0,
            "Add project escapes viewport: {:?}",
            add.rect
        );
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
        let pv_a = app_core::projects::add_project(&store, root_a.to_str().unwrap()).unwrap();
        let pv_b = app_core::projects::add_project(&store, root_b.to_str().unwrap()).unwrap();
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
        let pa = store.add_project("repo", root_a.to_str().unwrap()).unwrap();
        let pb = store.add_project("repo", root_b.to_str().unwrap()).unwrap();
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
    fn poll_usage_paces_the_worker_reads() {
        let mut s = State::new(test_handle());
        assert!(s.poll_usage(0.0), "the first read goes out");
        assert!(!s.poll_usage(1.0), "inside the 5 s window");
        assert!(!s.poll_usage(4.9));
        assert!(s.poll_usage(5.1), "the cadence allows the next read");
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
        s.usage_menu = Some(("claude".to_string(), 10.0, 10.0));
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
            .map(|att| term::term_lines_el(&att.lines, &s.ink, ""))
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
        s.attachments.get_mut(&key).unwrap().lines = lines;

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
            assert!(terminal_key(s, &ev), "the key reached the live session");
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
    fn a_terminal_link_opens_only_on_cmd_click_and_never_takes_tab() {
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
        assert_eq!(rt.state.notice, None, "a plain click does nothing");

        rt.update(|s, _| {
            s.editor.set_modifiers(Mods {
                cmd: true,
                ..Mods::default()
            })
        });
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert_eq!(
            rt.state.notice.as_deref(),
            Some("nope/zzz.rs: no such file"),
            "Cmd-click resolved the file"
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
}
