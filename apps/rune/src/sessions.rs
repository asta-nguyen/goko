//! The session layer: the daemon's sessions as the app sees them.
//!
//! The daemon owns a session's state: the UI never fakes one. A press
//! issues a request, the daemon answers with an event, and the event is
//! what moves the row. This module holds that seam — the requests the UI
//! actions issue, the replies they wait for, and the events that turn into
//! rows, tabs and metadata.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

use app_core::error::BridgeError;
use app_core::store::SessionMeta;
use base64::Engine as _;
use protocol::message::{Event, Request};
use protocol::session::{SessionId, SessionInfo};

use crate::State;
use crate::daemon::UiEvent;
use crate::data::{self, SessionView};
use crate::editor::backend::{self, Job, Req};
use crate::panes::Placement;

/// The terminal size a new session starts with; the daemon resizes it when
/// the viewport is measured.
/// How long an unanswered activity or sub-agent read holds back the next
/// one, in seconds. A reply that never comes must not stop the polls for good.
const POLL_REPLY_WAIT: f64 = 30.0;

pub const DEFAULT_COLS: u16 = 80;
pub const DEFAULT_ROWS: u16 = 24;

/// How long a session may be stopping before the force kill is offered.
pub const FORCE_KILL_AFTER_MS: i64 = 5_000;
/// How long a polite stop gets before the app escalates it to a force kill
/// on its own: an interactive shell ignores SIGTERM, and closing it must not
/// need a manual Force kill. Matches the daemon's own grace period.
pub const AUTO_FORCE_AFTER_MS: i64 = 2_000;
// The automatic force kill fires before the manual one is offered.
const _: () = assert!(AUTO_FORCE_AFTER_MS < FORCE_KILL_AFTER_MS);

/// Adjectives for generated session names (`brave-meadow`).
pub const NAME_ADJECTIVES: &[&str] = &[
    "amber", "bold", "brave", "bright", "calm", "clever", "cosmic", "crisp", "eager", "frosty",
    "gentle", "golden", "keen", "lively", "lucid", "merry", "noble", "proud", "quiet", "radiant",
    "serene", "silent", "solar", "swift", "vivid", "wild", "zen", "astral",
];

/// Nouns for generated session names (`brave-meadow`).
pub const NAME_NOUNS: &[&str] = &[
    "badger", "beacon", "breeze", "canyon", "cedar", "echo", "falcon", "forest", "fox", "glade",
    "harbor", "haven", "island", "lagoon", "meadow", "orbit", "otter", "peak", "river", "shadow",
    "sparrow", "summit", "timber", "valley", "willow", "grove", "oasis", "ridge",
];

/// A random adjective-noun name for a shell outside any project: `brave-meadow`, `swift-fox`.
pub fn generate_session_name() -> String {
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as usize)
        .unwrap_or(0);
    let adj_idx = (nanos ^ count) % NAME_ADJECTIVES.len();
    let noun_idx = (nanos.rotate_left(5) ^ (count * 7)) % NAME_NOUNS.len();
    format!("{}-{}", NAME_ADJECTIVES[adj_idx], NAME_NOUNS[noun_idx])
}

/// Whether `name` is one `generate_session_name` could have produced.
pub fn is_generated_name(name: &str) -> bool {
    let parts: Vec<&str> = name.split('-').collect();
    if parts.len() == 2 {
        NAME_ADJECTIVES.contains(&parts[0]) && NAME_NOUNS.contains(&parts[1])
    } else {
        false
    }
}

/// A title nobody chose: generated, empty, or a bare shell name. Only these
/// are replaced by the repository name when a session joins a project.
fn is_placeholder_title(title: &str) -> bool {
    title.is_empty()
        || is_generated_name(title)
        || matches!(title, "shell" | "zsh" | "bash" | "fish")
}

/// What a new session runs: its argv, its working directory, and the
/// project/agent the app records for it.
#[derive(Clone, Debug)]
pub struct SessionLaunch {
    pub argv: Vec<String>,
    pub cwd: String,
    /// The registered project the session belongs to, if any.
    pub project_id: Option<String>,
    /// The agent id the app records (`shell`, `claude`, `custom`, …).
    pub agent: String,
    /// The daemon's title for the session — empty falls back to the argv.
    pub title: String,
}

/// The `session.create` request for a launch: the argv, the working
/// directory and the terminal size the session starts with.
pub fn create_request(id: u64, launch: &SessionLaunch) -> Request {
    Request::SessionCreate {
        id,
        argv: launch.argv.clone(),
        cwd: launch.cwd.clone(),
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
        title: launch.title.clone(),
    }
}

/// The `session.stop` request: the daemon decides when the session is
/// really gone. `force` kills the process group outright — an interactive
/// shell ignores SIGTERM, so the first stop can hang.
pub fn stop_request(id: u64, session: SessionId, force: bool) -> Request {
    Request::SessionStop {
        id,
        session_id: session,
        force,
    }
}

/// The `session.delete` request: the daemon drops the session for good.
pub fn delete_request(id: u64, session: SessionId) -> Request {
    Request::SessionDelete {
        id,
        session_id: session,
    }
}

/// A daemon request whose reply the UI is waiting for. The reply is drained
/// in `poll` — the UI thread never blocks on the daemon.
pub enum Pending {
    /// `session.create`: the new session lands where the launch pointed —
    /// its own tab or a split pane — and records metadata.
    Create {
        reply: Receiver<Result<SessionInfo, BridgeError>>,
        launch: SessionLaunch,
        placement: Placement,
        /// The workspace selected when a launch with no project went out:
        /// the session belongs to it even if the selection moves meanwhile.
        workspace_id: Option<String>,
    },
    /// `session.stop`: the daemon's answer updates the row. The key lets a
    /// closing pane see the stop's failure instead of a global notice.
    Stop {
        reply: Receiver<Result<SessionInfo, BridgeError>>,
        key: String,
    },
    /// `session.delete`: the row, its metadata and its panes go away.
    Delete {
        reply: Receiver<Result<serde_json::Value, BridgeError>>,
        key: String,
    },
}

impl State {
    /// Start a session in the daemon and remember why it exists. Its tab
    /// opens when the create resolves.
    pub fn launch_session(&mut self, launch: SessionLaunch) {
        self.launch_session_placed(launch, Placement::Tab);
    }

    /// A shell in the active checkout when its project is in the selected
    /// workspace, else a `$HOME` shell belonging to that workspace under a
    /// generated name: New tab, ⌘T and the sidebar's `+`. Either way it lands
    /// where the sidebar and tab bar are looking, so it is never hidden.
    pub fn new_shell_tab(&mut self) {
        // A real shell session: the daemon owns the session, the app records
        // why it exists.
        let project_id = self.active_project_in_view().map(str::to_string);
        let cwd = project_id
            .as_ref()
            .and(self.editor.checkout.as_ref())
            .map(|path| path.display().to_string())
            .or_else(|| std::env::var("HOME").ok())
            .unwrap_or_else(|| "/".to_string());
        let title = if project_id.is_none() {
            generate_session_name()
        } else {
            String::new()
        };
        self.launch_session(SessionLaunch {
            argv: vec![backend::default_shell()],
            cwd,
            project_id,
            agent: "shell".to_string(),
            title,
        });
    }

    /// `launch_session` with a chosen landing spot for the new session —
    /// a fresh tab or a pane splitting an existing one. Returns whether the
    /// request went out.
    pub fn launch_session_placed(&mut self, launch: SessionLaunch, placement: Placement) -> bool {
        let asked = launch.clone();
        let reply = self
            .daemon
            .call_json::<SessionInfo>(move |id| create_request(id, &asked));
        match reply {
            Some(reply) => {
                let workspace_id = launch
                    .project_id
                    .is_none()
                    .then(|| self.workspace_id().to_string());
                self.pending.push(Pending::Create {
                    reply,
                    launch,
                    placement,
                    workspace_id,
                });
                true
            }
            None => {
                self.notice = Some("the Goko daemon is not reachable".into());
                false
            }
        }
    }

    /// Stop a session: the daemon decides when it is really gone, and the
    /// row follows its `session.updated` event. `force` kills the process
    /// group outright. Returns whether the request went out — a pane close
    /// needs to know, since nothing else will retry it.
    pub fn stop_session(&mut self, key: &str, force: bool) -> bool {
        let Some(session_id) = self.session_id(key) else {
            return false;
        };
        let reply = self
            .daemon
            .call_json::<SessionInfo>(move |id| stop_request(id, session_id, force));
        match reply {
            Some(reply) => {
                self.pending.push(Pending::Stop {
                    reply,
                    key: key.to_string(),
                });
                true
            }
            None => {
                self.notice = Some("the Goko daemon is not reachable".into());
                false
            }
        }
    }

    /// Escalate every polite stop that has hung past [`AUTO_FORCE_AFTER_MS`]
    /// to a force kill, once per stop. An older daemon only sends SIGTERM,
    /// which a shell ignores; a newer one hangs up and kills on its own, and
    /// then this never fires. Returns whether a request went out.
    pub fn poll_stop_escalation(&mut self) -> bool {
        if !self.data.connected {
            return false;
        }
        let due: Vec<String> = self
            .stopping_since
            .iter()
            .filter(|(key, since)| {
                self.now_ms - **since >= AUTO_FORCE_AFTER_MS && !self.auto_forced.contains(*key)
            })
            .map(|(key, _)| key.clone())
            .collect();
        let mut sent = false;
        for key in due {
            if self.stop_session(&key, true) {
                self.auto_forced.insert(key);
                sent = true;
            }
        }
        sent
    }

    /// The session has been stopping for a while: SIGTERM can be ignored
    /// (an interactive shell does), so the force kill is offered — the
    /// same escalation the desktop shows after five seconds.
    pub fn force_kill_ready(&self, key: &str) -> bool {
        self.stopping_since
            .get(key)
            .is_some_and(|since| self.now_ms - since >= FORCE_KILL_AFTER_MS)
    }

    /// Delete a session: the daemon drops it, the app drops its metadata
    /// and its panes. Returns whether the request went out.
    pub fn delete_session(&mut self, key: &str) -> bool {
        let Some(session_id) = self.session_id(key) else {
            return false;
        };
        let reply = self
            .daemon
            .call_json::<serde_json::Value>(move |id| delete_request(id, session_id));
        match reply {
            Some(reply) => {
                self.pending.push(Pending::Delete {
                    reply,
                    key: key.to_string(),
                });
                true
            }
            None => {
                self.notice = Some("the Goko daemon is not reachable".into());
                false
            }
        }
    }

    /// Apply one daemon event to the app's session state.
    pub fn apply_daemon_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::Connected { version } => {
                self.data.connected = true;
                self.daemon_version = Some(version);
                // The connection was replaced: the output routes died with
                // the old one, so every pane's terminal takes a fresh
                // subscription — the daemon replays its retained output.
                self.resubscribe_all();
            }
            UiEvent::Disconnected { reason } => {
                // The rows stay: the sessions are still running, the app
                // just cannot talk to the daemon right now.
                self.data.connected = false;
                self.daemon_reason = Some(reason);
            }
            UiEvent::Sessions(list) => {
                // The daemon's list is authoritative: it replaces the rows.
                let views: Vec<SessionView> =
                    list.into_iter().map(|info| self.view_for(info)).collect();
                self.data.sessions = views;
                // A pane whose session the list no longer knows collapses.
                self.reconcile_panes();
            }
            UiEvent::Event(event) => self.apply_session_event(event),
        }
    }

    /// The daemon's session events: an update moves the row, an exit
    /// freezes it, a lag notice asks for a fresh subscription.
    fn apply_session_event(&mut self, event: Event) {
        match event {
            Event::SessionUpdated { session } => self.upsert_session(session),
            Event::SessionExited {
                session_id,
                exit_code,
            } => {
                let key = data::SessionInfo::key(session_id);
                let mut just_ended = false;
                if let Some(view) = self
                    .data
                    .sessions
                    .iter_mut()
                    .find(|view| view.info.id == key)
                {
                    just_ended = !view.ended();
                    view.info.state = data::SessionState::Exited;
                    view.info.exit_code = exit_code;
                    if view.info.ended_at.is_none() {
                        view.info.ended_at = Some(self.now_ms);
                    }
                }
                if just_ended {
                    self.note_done_sound();
                }
            }
            Event::TerminalLagged { session_id } => {
                // The pane's terminal resubscribes on the next poll.
                self.terminal_lagged.insert(session_id);
            }
        }
    }

    /// Insert or replace a session row, keeping the stored metadata join.
    pub fn upsert_session(&mut self, mut info: SessionInfo) {
        let key = data::SessionInfo::key(info.id);
        self.adopt_unassigned_session(&key, &mut info);
        let view = self.view_for(info);
        let stopping = view.info.state == data::SessionState::Stopping;
        let ended = view.ended();
        let (was_stopping, was_ended) = self
            .data
            .sessions
            .iter()
            .find(|slot| slot.info.id == key)
            .map_or((false, false), |slot| {
                (
                    slot.info.state == data::SessionState::Stopping,
                    slot.ended(),
                )
            });
        match self
            .data
            .sessions
            .iter_mut()
            .find(|slot| slot.info.id == key)
        {
            Some(slot) => *slot = view,
            None => self.data.sessions.push(view),
        }
        // A row that just reached an ended state chimes while unfocused.
        if ended && !was_ended {
            self.note_done_sound();
        }
        // The force kill is offered once the session has been stopping for
        // a while.
        if stopping && !was_stopping {
            self.stopping_since.insert(key, self.now_ms);
        } else if !stopping {
            self.stopping_since.remove(&key);
            self.auto_forced.remove(&key);
        }
    }

    /// Drain the replies the session requests are waiting for. Returns
    /// whether the app's session state changed.
    pub fn drain_pending(&mut self) -> bool {
        let queued = std::mem::take(&mut self.pending);
        let mut waiting = Vec::new();
        let mut changed = false;
        for entry in queued {
            match entry {
                Pending::Create {
                    reply,
                    launch,
                    placement,
                    workspace_id,
                } => match reply.try_recv() {
                    Ok(Ok(info)) => {
                        self.session_created(info, &launch, placement, workspace_id);
                        changed = true;
                    }
                    Ok(Err(error)) => {
                        self.notice = Some(format!("could not start the session: {error}"));
                        changed = true;
                    }
                    Err(TryRecvError::Empty) => waiting.push(Pending::Create {
                        reply,
                        launch,
                        placement,
                        workspace_id,
                    }),
                    Err(TryRecvError::Disconnected) => {
                        self.notice = Some("the Goko daemon is not reachable".into());
                        changed = true;
                    }
                },
                Pending::Stop { reply, key } => match reply.try_recv() {
                    Ok(Ok(info)) => {
                        self.close_inflight.remove(&key);
                        self.upsert_session(info);
                        changed = true;
                    }
                    Ok(Err(error)) => {
                        self.close_inflight.remove(&key);
                        let message = format!("could not stop the session: {error}");
                        if !self.set_pane_close_error(&key, &message) {
                            self.notice = Some(message);
                        }
                        changed = true;
                    }
                    Err(TryRecvError::Empty) => waiting.push(Pending::Stop { reply, key }),
                    Err(TryRecvError::Disconnected) => {
                        self.close_inflight.remove(&key);
                        self.notice = Some("the Goko daemon is not reachable".into());
                        changed = true;
                    }
                },
                Pending::Delete { reply, key } => match reply.try_recv() {
                    Ok(Ok(_)) => {
                        self.close_inflight.remove(&key);
                        self.session_deleted(&key);
                        changed = true;
                    }
                    Ok(Err(error)) => {
                        self.close_inflight.remove(&key);
                        let message = format!("could not delete the session: {error}");
                        if !self.set_pane_close_error(&key, &message) {
                            self.notice = Some(message);
                        }
                        changed = true;
                    }
                    Err(TryRecvError::Empty) => waiting.push(Pending::Delete { reply, key }),
                    Err(TryRecvError::Disconnected) => {
                        self.close_inflight.remove(&key);
                        self.notice = Some("the Goko daemon is not reachable".into());
                        changed = true;
                    }
                },
            }
        }
        self.pending = waiting;
        if let Some(rx) = &self.pending_output {
            match rx.try_recv() {
                Ok(Ok(list)) => {
                    for out in list.sessions {
                        let key = data::SessionInfo::key(out.session_id);
                        self.note_session_output(&key, out.output_seq, out.last_output_at);
                    }
                    self.pending_output = None;
                    changed = true;
                }
                Ok(Err(_)) => {
                    self.pending_output = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.pending_output = None;
                }
            }
        }
        changed
    }

    /// A session exists: record why it exists, then place it — its own
    /// tab or a split pane, per the launch's placement.
    fn session_created(
        &mut self,
        mut info: SessionInfo,
        launch: &SessionLaunch,
        placement: Placement,
        workspace_id: Option<String>,
    ) {
        if !launch.title.is_empty() && info.title.is_empty() {
            info.title = launch.title.clone();
        }
        let view = self.view_for(info.clone());
        let key = view.info.id.clone();
        let title = if !launch.title.is_empty() {
            launch.title.clone()
        } else {
            view.title()
        };
        let meta = SessionMeta {
            session_id: key.clone(),
            project_id: launch.project_id.clone(),
            agent: launch.agent.clone(),
            argv: info.argv.clone(),
            cwd: info.cwd.clone(),
            title: title.clone(),
            created_at: self.now_ms,
            workspace_id,
        };
        // The store is the app's memory of the session; the cache keeps the
        // UI coherent while the write rides the worker lane.
        self.session_metas.insert(key.clone(), meta.clone());
        backend::enqueue(
            &mut self.editor.backend,
            Job::project(Req::InsertSessionMeta(meta)),
        );
        self.upsert_session(info);
        self.place_session(&key, &title, placement);
    }

    /// A session is gone: drop its row; its pane collapses with it and the
    /// whole local trace goes (attachment, metadata, stored row).
    fn session_deleted(&mut self, key: &str) {
        self.data.sessions.retain(|view| view.info.id != key);
        self.forget_session(key);
    }

    /// The view row for a daemon session, joined with its stored metadata.
    pub fn view_for(&self, mut info: SessionInfo) -> SessionView {
        let key = data::SessionInfo::key(info.id);
        let meta = self.session_metas.get(&key).cloned();
        if info.title.is_empty()
            && let Some(m) = &meta
            && !m.title.is_empty()
        {
            info.title = m.title.clone();
        }
        SessionView::new(info, meta)
    }

    /// The session row for a session id.
    pub fn session_by_key(&self, key: &str) -> Option<&SessionView> {
        self.data.sessions.iter().find(|view| view.info.id == key)
    }

    /// The daemon's wire id for a session id.
    pub fn session_id(&self, key: &str) -> Option<SessionId> {
        self.session_by_key(key).map(|view| view.info.session_id)
    }

    /// The workspace a session row activates: its checkout's key for git
    /// projects, the project root's key otherwise, `__unassigned__` when it
    /// belongs to no registered project.
    pub fn session_workspace(&self, view: &SessionView) -> String {
        let project_id = view
            .meta
            .as_ref()
            .and_then(|meta| meta.project_id.as_deref())
            .map(str::to_string)
            .or_else(|| self.project_id_for_cwd(&view.info.cwd));
        let Some(project_id) = project_id.as_deref() else {
            return data::UNASSIGNED_WORKSPACE.to_string();
        };
        let Some(project) = self
            .editor
            .projects
            .iter()
            .find(|project| project.project.id == project_id)
        else {
            return data::UNASSIGNED_WORKSPACE.to_string();
        };
        let worktrees = self
            .editor
            .sidebar_worktrees
            .get(project_id)
            .and_then(|entry| entry.list.as_ref());
        data::workspace_for_session(view, Some(&data::ProjectCheckout { project, worktrees }))
    }

    /// A session started outside any project (New Terminal with no active
    /// project) joins one when its shell `cd`s into it: a registered project
    /// or worktree claims it, otherwise the enclosing Git repository is
    /// registered in the selected workspace and its `AddProject` reply links
    /// the session. A generated or shell-default title takes the repository's
    /// name. A session that already belongs to a project never moves — under
    /// the per-project filter, moving it would hide its tab mid-use. Runs
    /// only when the cwd changed, so the filesystem walk is not repeated on
    /// every state update.
    fn adopt_unassigned_session(&mut self, key: &str, info: &mut SessionInfo) {
        if info.cwd.is_empty() || self.editor.projects_loading {
            return;
        }
        if self
            .session_metas
            .get(key)
            .is_none_or(|meta| meta.project_id.is_some())
        {
            return;
        }
        let cwd_changed = self
            .data
            .sessions
            .iter()
            .find(|slot| slot.info.id == key)
            .is_none_or(|slot| slot.info.cwd != info.cwd);
        if !cwd_changed {
            return;
        }
        let project_id = self.project_id_for_cwd(&info.cwd);
        let git_root = Self::git_root_for_cwd(std::path::Path::new(&info.cwd));
        if project_id.is_none() {
            let Some(root) = &git_root else {
                return;
            };
            let path = root.display().to_string();
            if self.auto_project_roots.insert(path.clone()) {
                // The repository joins the workspace the terminal belongs to,
                // which need not be the one selected now.
                let workspace_id = self
                    .data
                    .sessions
                    .iter()
                    .find(|slot| slot.info.id == key)
                    .map_or_else(
                        || {
                            self.session_metas
                                .get(key)
                                .and_then(|meta| meta.workspace_id.clone())
                                .unwrap_or_else(|| self.workspace_id().to_string())
                        },
                        |view| self.loose_workspace(view).to_string(),
                    );
                backend::enqueue(
                    &mut self.editor.backend,
                    Job::project(Req::AddProject { path, workspace_id }),
                );
            }
        }
        let name = git_root
            .as_deref()
            .and_then(|root| root.file_name())
            .and_then(|name| name.to_str())
            .map(str::to_string)
            .or_else(|| {
                let pid = project_id.as_deref()?;
                self.editor
                    .projects
                    .iter()
                    .find(|p| p.project.id == pid)
                    .map(|p| p.project.name.clone())
            });
        let Some(meta) = self.session_metas.get_mut(key) else {
            return;
        };
        let mut changed = false;
        if let Some(pid) = project_id {
            meta.project_id = Some(pid);
            changed = true;
        }
        let renamed = name.filter(|_| is_placeholder_title(&meta.title));
        if let Some(name) = &renamed {
            meta.title = name.clone();
            info.title = name.clone();
            changed = true;
        }
        if changed {
            let meta = meta.clone();
            backend::enqueue(
                &mut self.editor.backend,
                Job::project(Req::InsertSessionMeta(meta)),
            );
        }
        if let Some(name) = renamed {
            self.rename_session_tab(key, &name);
        }
    }

    /// The workspace a session outside every project belongs to: the one it
    /// was started in, or Default for older sessions and for a workspace that
    /// has since been removed (Default also receives a removed workspace's
    /// projects).
    pub fn loose_workspace<'a>(&'a self, view: &'a SessionView) -> &'a str {
        view.meta
            .as_ref()
            .and_then(|meta| meta.workspace_id.as_deref())
            .filter(|id| self.workspaces.iter().any(|w| w.id == *id))
            .unwrap_or(app_core::store::DEFAULT_WORKSPACE_ID)
    }

    /// The registered project a session belongs to, if any.
    fn session_project(&self, view: &SessionView) -> Option<&app_core::projects::ProjectView> {
        let id = view.meta.as_ref()?.project_id.as_deref()?;
        self.editor.projects.iter().find(|p| p.project.id == id)
    }

    /// Whether a session belongs to `workspace_id`: through its project, or,
    /// with none, through the workspace it was started in.
    pub fn session_in_workspace(&self, view: &SessionView, workspace_id: &str) -> bool {
        match self.session_project(view) {
            Some(project) => project.project.workspace_id == workspace_id,
            None => self.loose_workspace(view) == workspace_id,
        }
    }

    /// A session outside every project, started in the selected workspace.
    pub fn loose_in_selected_workspace(&self, view: &SessionView) -> bool {
        self.session_project(view).is_none() && self.loose_workspace(view) == self.workspace_id()
    }

    /// The active project when it is in the selected workspace. Selecting
    /// another workspace leaves the editor's project alone, but the sidebar
    /// and tab bar then show that workspace instead of filtering it down to a
    /// project it does not contain.
    pub fn active_project_in_view(&self) -> Option<&str> {
        let id = self.editor.project_id.as_deref()?;
        self.editor
            .projects
            .iter()
            .any(|p| p.project.id == id && p.project.workspace_id == self.workspace_id())
            .then_some(id)
    }

    /// Match a session's `cwd` against registered projects or their worktrees.
    pub fn project_id_for_cwd(&self, cwd: &str) -> Option<String> {
        if cwd.is_empty() {
            return None;
        }
        let path = std::path::Path::new(cwd);
        for proj in &self.editor.projects {
            let root = std::path::Path::new(&proj.project.root);
            if path == root || path.starts_with(root) {
                return Some(proj.project.id.clone());
            }
            if let Some(entry) = self.editor.sidebar_worktrees.get(&proj.project.id)
                && let Some(list) = &entry.list
            {
                for wt in &list.worktrees {
                    let wt_path = std::path::Path::new(&wt.path);
                    if path == wt_path || path.starts_with(wt_path) {
                        return Some(proj.project.id.clone());
                    }
                }
            }
        }
        None
    }

    /// Walk up from `path` to find the enclosing Git repository root.
    /// Excludes `$HOME` and root `/`.
    pub fn git_root_for_cwd(path: &std::path::Path) -> Option<std::path::PathBuf> {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let mut cur = path;
        while let Some(parent) = cur.parent() {
            if home.as_deref() == Some(cur) || cur == std::path::Path::new("/") {
                break;
            }
            if cur.join(".git").exists() {
                return std::fs::canonicalize(cur).ok();
            }
            cur = parent;
        }
        None
    }

    /// Send bytes to the attached session's PTY.
    pub fn write_terminal(&mut self, session: SessionId, bytes: &[u8]) {
        // A frame over 1 MiB is rejected and closes the connection, so a big
        // paste goes in pieces (192 KiB raw is 256 KiB of base64), in order.
        for chunk in bytes.chunks(192 * 1024) {
            let data_b64 = base64::engine::general_purpose::STANDARD.encode(chunk);
            let _ = self.daemon.call(move |id| Request::TerminalWrite {
                id,
                session_id: session,
                data_b64: data_b64.clone(),
            });
        }
    }

    /// Tell the daemon how big the terminal we render is, so its PTY matches.
    pub fn resize_terminal(&mut self, session: SessionId, cols: u16, rows: u16) {
        let _ = self.daemon.call(move |id| Request::TerminalResize {
            id,
            session_id: session,
            cols,
            rows,
        });
    }

    /// Read the usage snapshot and the accounts view on the worker. The
    /// quota chips refresh about every five seconds; the job carries the
    /// last snapshot so a mid-write capture keeps its previous values.
    pub fn poll_usage(&mut self, now: f64) -> bool {
        if self.usage_at.is_some_and(|at| now - at < 5.0) {
            return false;
        }
        self.usage_at = Some(now);
        self.request_usage()
    }

    /// One usage/accounts read, regardless of the poll cadence — account
    /// mutations refresh the chips right away.
    pub fn request_usage(&mut self) -> bool {
        backend::enqueue(
            &mut self.editor.backend,
            Job::project(Req::Usage {
                previous: self.data.usage.clone().unwrap_or_default(),
            }),
        )
    }

    /// The agent's login command for one account, resolved on the worker
    /// and launched as a native session when it arrives.
    pub fn request_sign_in(&mut self, agent: &str, id: &str) {
        let _ = backend::enqueue(
            &mut self.editor.backend,
            Job::project(Req::AccountSignIn {
                agent: agent.to_string(),
                id: id.to_string(),
            }),
        );
    }

    /// Read the child processes' activity: what each session's agent is
    /// doing. The files are read on the worker; the UI only keeps the
    /// answer.
    pub fn poll_activity(&mut self, now: f64) -> bool {
        self.poll_output(now);
        if self.activity_at.is_some_and(|at| now - at < 1.0) {
            return false;
        }
        if self
            .activity_pending
            .is_some_and(|at| now - at < POLL_REPLY_WAIT)
        {
            return false;
        }
        self.activity_at = Some(now);
        let sessions: Vec<app_core::activity::SessionActivity> = self
            .data
            .sessions
            .iter()
            .map(|view| app_core::activity::SessionActivity {
                id: view.info.id.clone(),
                argv: view.info.argv.clone(),
                cwd: view.info.cwd.clone(),
                started_at_ms: view.info.started_at,
                running: view.info.state == data::SessionState::Running,
                interrupted: view.info.state == data::SessionState::Interrupted,
            })
            .collect();
        if sessions.is_empty() {
            return false;
        }
        let previous = self.activity.clone();
        if crate::trace::enabled() {
            crate::trace::trace(&format!("activity read for {} sessions", sessions.len()));
        }
        let queued = backend::enqueue(
            &mut self.editor.backend,
            Job::project(Req::Activity { sessions, previous }),
        );
        self.activity_pending = queued.then_some(now);
        queued
    }

    /// Poll the daemon for live output activity across all sessions.
    pub fn poll_output(&mut self, now: f64) -> bool {
        if !self.daemon.supports_session_output() {
            return false;
        }
        if self.pending_output.is_some() {
            return false;
        }
        if self.output_polled_at.is_some_and(|at| now - at < 0.5) {
            return false;
        }
        self.output_polled_at = Some(now);
        self.pending_output = self
            .daemon
            .call_json::<crate::daemon::SessionOutputList>(|id| Request::SessionOutput { id });
        self.pending_output.is_some()
    }

    /// Update session output stats from daemon readings.
    pub fn note_session_output(&mut self, key: &str, output_seq: u64, last_output_at: Option<i64>) {
        let (prev_seq, prev_last_at, prev_rec) = self.session_output.get(key).copied().unwrap_or((
            output_seq,
            last_output_at,
            self.now_ms,
        ));
        let rec = if prev_seq != output_seq || prev_last_at != last_output_at {
            self.now_ms
        } else {
            prev_rec
        };
        self.session_output
            .insert(key.to_string(), (output_seq, last_output_at, rec));
    }

    /// Whether output tracking sees this session as quiet (no output for >= 2 seconds).
    pub fn is_output_quiet(&self, key: &str) -> bool {
        let Some(&(_seq, last_at, rec)) = self.session_output.get(key) else {
            return false;
        };
        if let Some(last) = last_at {
            return self.now_ms.saturating_sub(last) >= 2000;
        }
        (self.now_ms - rec) >= 2000
    }

    /// What the session's child process is doing.
    pub fn session_activity(&self, key: &str) -> Option<app_core::activity::Activity> {
        self.activity.get(key).copied()
    }

    /// The activity the dot shows: a running sub-agent means the session
    /// is working — unless the agent is waiting for the user or sitting idle
    /// with no recent output.
    pub fn session_shown_activity(&self, key: &str) -> Option<app_core::activity::Activity> {
        let activity = self.session_activity(key);
        let has_subagents = !self.session_subagents(key).is_empty();
        if has_subagents && activity != Some(app_core::activity::Activity::Waiting) {
            if self.is_output_quiet(key) {
                return None;
            }
            return Some(app_core::activity::Activity::Working);
        }
        activity
    }

    /// Read the agent CLIs running under the sessions' process trees. The
    /// process table is read on the worker.
    pub fn poll_subagents(&mut self, now: f64) -> bool {
        if self.subagents_at.is_some_and(|at| now - at < 2.0) {
            return false;
        }
        if self
            .subagents_pending
            .is_some_and(|at| now - at < POLL_REPLY_WAIT)
        {
            return false;
        }
        self.subagents_at = Some(now);
        let sessions: Vec<protocol::session::SessionInfo> = self
            .data
            .sessions
            .iter()
            .map(|view| protocol::session::SessionInfo {
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
            })
            .collect();
        if sessions.is_empty() {
            return false;
        }
        let inspect = crate::tab_debug::inspection_sessions(self);
        let queued = backend::enqueue(
            &mut self.editor.backend,
            Job::project(Req::Subagents { sessions, inspect }),
        );
        self.subagents_pending = queued.then_some(now);
        queued
    }

    /// The agent CLIs running under a session, as the row shows them.
    pub fn session_subagents(&self, key: &str) -> Vec<app_core::subagents::Subagent> {
        self.subagents.get(key).cloned().unwrap_or_default()
    }

    /// Acknowledge the bytes the daemon sent, so a flow-controlled stream
    /// keeps moving.
    pub fn ack_terminal(&mut self, session: SessionId, upto_seq: u64) {
        let _ = self.daemon.call(move |id| Request::TerminalAck {
            id,
            session_id: session,
            upto_seq,
        });
    }

    /// Open a row's menu, anchored where the pointer is.
    pub fn open_menu(&mut self, target: crate::sidebar::RowTarget, kind: crate::sidebar::MenuKind) {
        self.sidebar_menu = Some((target, kind));
    }

    /// Ask the user for a line of text. The field takes the keyboard, so
    /// the terminal never sees the typing.
    pub fn ask_for_input(&mut self, input: crate::sidebar::SidebarInput) {
        self.sidebar_text = rune::edit::TextEdit::new("");
        self.sidebar_text.focused = true;
        self.sidebar_input = Some(input);
    }

    /// Ask the worker for an agent's launch argv, then start a session with
    /// it in the row's checkout.
    ///
    /// Every request carries its own token: two launches can be in flight
    /// at the same time (two rows, or the same agent twice), and each
    /// reply must start the session it belongs to.
    pub fn request_session_argv(&mut self, target: crate::sidebar::RowTarget, agent: String) {
        self.argv_token = self.argv_token.wrapping_add(1);
        let token = self.argv_token;
        let reply = crate::editor::backend::enqueue(
            &mut self.editor.backend,
            crate::editor::backend::Job::project(crate::editor::backend::Req::AgentArgv {
                agent: agent.clone(),
                token,
            }),
        );
        if !reply {
            self.notice = Some("the editor queue is full; the agent was not resolved".into());
            return;
        }
        self.pending_argv.push(PendingArgv {
            token,
            target,
            agent,
        });
    }

    /// The launch waiting for its agent's argv, taken out of the pending
    /// list by the token its reply carries. A reply for another request
    /// finds nothing here and leaves every pending launch alone.
    pub fn take_pending_argv(&mut self, token: u64) -> Option<PendingArgv> {
        let index = self
            .pending_argv
            .iter()
            .position(|pending| pending.token == token)?;
        Some(self.pending_argv.remove(index))
    }

    /// The launch a resolved agent-argv reply starts: the pending
    /// request's own row plus the argv the worker resolved for it.
    pub fn resolved_launch(&mut self, token: u64, argv: Vec<String>) -> Option<SessionLaunch> {
        let pending = self.take_pending_argv(token)?;
        let (cwd, project_id) = self.checkout_of(&pending.target);
        Some(SessionLaunch {
            argv,
            cwd,
            project_id,
            agent: pending.agent,
            title: String::new(),
        })
    }

    /// Remove a linked worktree through the worker lane.
    pub fn remove_worktree(&mut self, project_id: &str, path: &str) {
        if !crate::editor::backend::enqueue(
            &mut self.editor.backend,
            crate::editor::backend::Job::project(crate::editor::backend::Req::RemoveWorktree {
                project_id: project_id.to_string(),
                path: path.to_string(),
            }),
        ) {
            self.notice = Some("the editor queue is full; try again".into());
        }
    }

    /// Drop Git's records of missing worktrees through the worker lane.
    pub fn prune_worktrees(&mut self, project_id: &str) {
        if !crate::editor::backend::enqueue(
            &mut self.editor.backend,
            crate::editor::backend::Job::project(crate::editor::backend::Req::PruneWorktrees {
                project_id: project_id.to_string(),
            }),
        ) {
            self.notice = Some("the editor queue is full; try again".into());
        }
    }

    /// Remove a registered project through the worker lane.
    pub fn remove_project(&mut self, project_id: &str) {
        if !crate::editor::backend::enqueue(
            &mut self.editor.backend,
            crate::editor::backend::Job::project(crate::editor::backend::Req::RemoveProject {
                id: project_id.to_string(),
            }),
        ) {
            self.notice = Some("the editor queue is full; try again".into());
        }
    }

    /// Confirm the open worktree or project removal. The target must still be
    /// listed; otherwise nothing runs. Running sessions are never stopped.
    pub fn confirm_sidebar_removal(&mut self) {
        use crate::sidebar::SidebarConfirm;
        let Some(confirm) = self.sidebar_confirm.take() else {
            return;
        };
        match confirm {
            SidebarConfirm::RemoveWorktree {
                project_id, path, ..
            } => {
                let listed = self
                    .editor
                    .sidebar_worktrees
                    .get(&project_id)
                    .is_some_and(|e| {
                        e.list
                            .as_ref()
                            .is_some_and(|l| l.worktrees.iter().any(|w| w.path == path))
                    });
                if !listed {
                    self.notice = Some("That item no longer exists".into());
                    return;
                }
                self.remove_worktree(&project_id, &path);
            }
            SidebarConfirm::RemoveProject { project_id, .. } => {
                if !self
                    .editor
                    .projects
                    .iter()
                    .any(|p| p.project.id == project_id)
                {
                    self.notice = Some("That item no longer exists".into());
                    return;
                }
                self.remove_project(&project_id);
            }
        }
    }

    /// Start a session with a custom command in the row's checkout. The
    /// command is split like a shell would — quoted words stay one word —
    /// and an empty or unparseable one is refused.
    pub fn launch_custom_session(&mut self, target: crate::sidebar::RowTarget, command: &str) {
        let argv = match parse_custom_command(command) {
            Ok(argv) => argv,
            Err(reason) => {
                self.notice = Some(reason.to_string());
                return;
            }
        };
        let (cwd, project_id) = self.checkout_of(&target);
        self.launch_session(SessionLaunch {
            argv,
            cwd,
            project_id,
            agent: "custom".to_string(),
            title: String::new(),
        });
    }

    /// The editor's active checkout as a launch target, when one is selected.
    pub fn active_checkout_target(&self) -> Option<crate::sidebar::RowTarget> {
        let project_id = self.editor.project_id.clone()?;
        let path = self.editor.checkout.as_ref()?.display().to_string();
        Some(crate::sidebar::RowTarget::Worktree { project_id, path })
    }

    /// The checkout a row's session runs in, and the project it belongs to.
    pub fn checkout_of(&self, target: &crate::sidebar::RowTarget) -> (String, Option<String>) {
        let project_id = match target {
            crate::sidebar::RowTarget::Project { project_id }
            | crate::sidebar::RowTarget::Worktree { project_id, .. } => Some(project_id.clone()),
            crate::sidebar::RowTarget::Session { .. } | crate::sidebar::RowTarget::Workspace => {
                None
            }
        };
        let path = match target {
            crate::sidebar::RowTarget::Project { project_id } => self
                .editor
                .projects
                .iter()
                .find(|project| project.project.id == *project_id)
                .map(|project| project.project.root.clone()),
            crate::sidebar::RowTarget::Worktree { path, .. } => Some(path.clone()),
            crate::sidebar::RowTarget::Session { .. } | crate::sidebar::RowTarget::Workspace => {
                None
            }
        };
        let cwd = path.unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| "/".to_string()));
        (cwd, project_id)
    }

    /// Create a linked worktree named `name` beside the project's root.
    pub fn create_worktree(&mut self, project_id: &str, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            self.notice = Some("the worktree name is empty".into());
            return;
        }
        let Some(root) = self
            .editor
            .projects
            .iter()
            .find(|project| project.project.id == project_id)
            .map(|project| project.project.root.clone())
        else {
            self.notice = Some("the project is no longer registered".into());
            return;
        };
        let path = PathBuf::from(&root)
            .parent()
            .map(|parent| parent.join(name).display().to_string())
            .unwrap_or_else(|| name.to_string());
        let _ = crate::editor::backend::enqueue(
            &mut self.editor.backend,
            crate::editor::backend::Job::project(crate::editor::backend::Req::CreateWorktree {
                project_id: project_id.to_string(),
                path,
                mode: "new".to_string(),
                branch: format!("goko/{name}"),
                base: None,
            }),
        );
    }
}

/// The argv of a custom command: shell words, so a quoted argument stays
/// one word. An empty line and a line that cannot be parsed are refused
/// with the reason.
pub fn parse_custom_command(command: &str) -> Result<Vec<String>, &'static str> {
    if command.trim().is_empty() {
        return Err("the command is empty");
    }
    match app_core::agents::shell_words(command) {
        Some(argv) if !argv.is_empty() => Ok(argv),
        _ => Err("the command cannot be parsed; check the quotes"),
    }
}

/// A launch waiting for its agent's argv.
pub struct PendingArgv {
    /// The request's own token: the reply that carries it starts this
    /// launch, whatever else is in flight.
    pub token: u64,
    /// The row the session belongs to.
    pub target: crate::sidebar::RowTarget,
    /// The agent the user picked.
    pub agent: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::daemon::UiEvent;
    use crate::test_handle;
    use app_core::store::SessionMeta;
    use protocol::session::SessionState as DaemonState;

    fn daemon_session(state: DaemonState) -> SessionInfo {
        SessionInfo {
            id: SessionId::new(),
            title: "shell".into(),
            argv: vec!["/bin/zsh".into()],
            cwd: "/work/repo".into(),
            state,
            pid: None,
            exit_code: None,
            error: None,
            started_at: 1_700_000_000_000,
            ended_at: None,
        }
    }

    fn state() -> State {
        State::new(test_handle())
    }

    #[test]
    fn the_session_list_replaces_the_rows() {
        let mut s = state();
        let first = daemon_session(DaemonState::Running);
        let second = daemon_session(DaemonState::Exited);
        s.apply_daemon_event(UiEvent::Sessions(vec![first.clone(), second]));
        assert_eq!(s.data.sessions.len(), 2);
        assert_eq!(
            s.data.sessions[0].info.id,
            crate::data::SessionInfo::key(first.id)
        );
    }

    #[test]
    fn a_session_update_upserts_the_row() {
        let mut s = state();
        let info = daemon_session(DaemonState::Running);
        s.apply_daemon_event(UiEvent::Event(Event::SessionUpdated {
            session: info.clone(),
        }));
        assert_eq!(s.data.sessions.len(), 1);
        // A second update replaces the row instead of adding one.
        let mut changed = info.clone();
        changed.state = DaemonState::Exited;
        s.apply_daemon_event(UiEvent::Event(Event::SessionUpdated { session: changed }));
        assert_eq!(s.data.sessions.len(), 1);
        assert_eq!(
            s.data.sessions[0].info.state,
            crate::data::SessionState::Exited
        );
    }

    #[test]
    fn an_exit_event_freezes_the_row() {
        let mut s = state();
        let info = daemon_session(DaemonState::Running);
        s.apply_daemon_event(UiEvent::Event(Event::SessionUpdated {
            session: info.clone(),
        }));
        s.apply_daemon_event(UiEvent::Event(Event::SessionExited {
            session_id: info.id,
            exit_code: Some(3),
        }));
        let row = &s.data.sessions[0];
        assert_eq!(row.info.state, crate::data::SessionState::Exited);
        assert_eq!(row.info.exit_code, Some(3));
        assert!(row.info.ended_at.is_some());
    }

    #[test]
    fn the_done_sound_waits_for_an_unfocused_window() {
        let mut s = state();
        s.window_focused = true;
        s.note_done_sound();
        assert_eq!(crate::sound::test_plays(), 0);
        s.window_focused = false;
        s.note_done_sound();
        assert_eq!(crate::sound::test_plays(), 1);
    }

    #[test]
    fn the_done_sound_respects_the_setting_and_debounces() {
        let mut s = state();
        s.window_focused = false;
        s.sound_done = false;
        s.now_ms = 2_000;
        s.note_done_sound();
        assert_eq!(crate::sound::test_plays(), 0);
        s.sound_done = true;
        s.note_done_sound();
        assert_eq!(crate::sound::test_plays(), 1);
        // Inside the debounce window a second finish stays silent.
        s.now_ms = 2_500;
        s.note_done_sound();
        assert_eq!(crate::sound::test_plays(), 1);
        s.now_ms = 3_001;
        s.note_done_sound();
        assert_eq!(crate::sound::test_plays(), 2);
    }

    #[test]
    fn an_exit_chimes_the_done_sound_once() {
        let mut s = state();
        s.window_focused = false;
        let info = daemon_session(DaemonState::Running);
        s.apply_daemon_event(UiEvent::Event(Event::SessionUpdated {
            session: info.clone(),
        }));
        assert_eq!(crate::sound::test_plays(), 0);
        s.apply_daemon_event(UiEvent::Event(Event::SessionExited {
            session_id: info.id,
            exit_code: Some(0),
        }));
        assert_eq!(crate::sound::test_plays(), 1);
        // A repeated exit event for the same row stays silent.
        s.now_ms += 2_000;
        s.apply_daemon_event(UiEvent::Event(Event::SessionExited {
            session_id: info.id,
            exit_code: Some(0),
        }));
        assert_eq!(crate::sound::test_plays(), 1);
    }

    #[test]
    fn a_session_update_into_an_ended_state_chimes_once() {
        let mut s = state();
        s.window_focused = false;
        let mut info = daemon_session(DaemonState::Running);
        s.upsert_session(info.clone());
        info.state = DaemonState::Failed;
        s.upsert_session(info.clone());
        assert_eq!(crate::sound::test_plays(), 1);
        // Past the debounce, an already-ended row still does not chime.
        s.now_ms += 2_000;
        s.upsert_session(info);
        assert_eq!(crate::sound::test_plays(), 1);
    }

    #[test]
    fn the_bulk_session_list_never_chimes() {
        let mut s = state();
        s.window_focused = false;
        // A session already ended when the list syncs is not a finish the
        // user just watched happen.
        s.apply_daemon_event(UiEvent::Sessions(vec![daemon_session(DaemonState::Exited)]));
        assert_eq!(crate::sound::test_plays(), 0);
    }

    #[test]
    fn the_connection_flag_follows_the_daemon() {
        let mut s = state();
        assert!(!s.data.connected);
        s.apply_daemon_event(UiEvent::Connected {
            version: "0.1.2".into(),
        });
        assert!(s.data.connected);
        assert_eq!(s.daemon_version.as_deref(), Some("0.1.2"));
        // A dropped socket keeps the rows: the sessions are still running.
        let info = daemon_session(DaemonState::Running);
        s.apply_daemon_event(UiEvent::Event(Event::SessionUpdated { session: info }));
        s.apply_daemon_event(UiEvent::Disconnected {
            reason: "gone".into(),
        });
        assert!(!s.data.connected);
        assert_eq!(s.data.sessions.len(), 1);
    }

    #[test]
    fn the_stored_metadata_joins_the_row() {
        let mut s = state();
        let info = daemon_session(DaemonState::Running);
        let key = crate::data::SessionInfo::key(info.id);
        s.session_metas.insert(
            key.clone(),
            SessionMeta {
                session_id: key.clone(),
                project_id: None,
                agent: "claude".into(),
                argv: info.argv.clone(),
                cwd: info.cwd.clone(),
                title: info.title.clone(),
                created_at: 0,
                workspace_id: None,
            },
        );
        s.apply_daemon_event(UiEvent::Event(Event::SessionUpdated { session: info }));
        let row = &s.data.sessions[0];
        assert_eq!(
            row.meta.as_ref().map(|meta| meta.agent.as_str()),
            Some("claude")
        );
        assert_eq!(row.agent_id(), Some("claude"));
    }

    #[test]
    fn a_refused_spawn_opens_no_tab() {
        let mut s = state();
        s.launch_session(SessionLaunch {
            argv: vec!["/bin/zsh".into()],
            cwd: "/tmp".into(),
            project_id: None,
            agent: "shell".into(),
            title: String::new(),
        });
        // The daemon refuses the request: the tab must not appear.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while s.notice.is_none() && std::time::Instant::now() < deadline {
            s.drain_pending();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(s.open_tabs.is_empty());
        assert!(s.notice.is_some());
    }

    #[test]
    fn a_hung_stop_offers_the_force_kill() {
        let mut s = state();
        let info = daemon_session(DaemonState::Running);
        let key = crate::data::SessionInfo::key(info.id);
        s.upsert_session(info.clone());
        assert!(!s.force_kill_ready(&key));

        // The session starts stopping: the force kill waits for the daemon
        // to have had its five seconds.
        let mut stopping = info.clone();
        stopping.state = DaemonState::Stopping;
        s.upsert_session(stopping);
        assert!(!s.force_kill_ready(&key));
        s.now_ms += FORCE_KILL_AFTER_MS;
        assert!(s.force_kill_ready(&key));

        // A session that leaves the stopping state forgets the wait.
        let mut exited = daemon_session(DaemonState::Running);
        exited.id = info.id;
        exited.state = DaemonState::Exited;
        s.upsert_session(exited);
        assert!(!s.force_kill_ready(&key));
    }

    /// A polite stop that a shell ignores is escalated to a force kill by the
    /// app after two seconds, once — the user never has to press Force kill.
    #[test]
    fn a_hung_polite_stop_is_forced_automatically_once() {
        let mut s = state();
        s.data.connected = true;
        let info = daemon_session(DaemonState::Running);
        let key = crate::data::SessionInfo::key(info.id);
        s.upsert_session(info.clone());
        let mut stopping = info.clone();
        stopping.state = DaemonState::Stopping;
        s.upsert_session(stopping);

        let forced = |s: &State| {
            s.pending
                .iter()
                .filter(|p| matches!(p, Pending::Stop { key: k, .. } if *k == key))
                .count()
        };
        assert!(!s.poll_stop_escalation(), "inside the grace period");
        assert_eq!(forced(&s), 0);
        s.now_ms += AUTO_FORCE_AFTER_MS;
        assert!(s.poll_stop_escalation(), "past it: a force kill goes out");
        assert_eq!(forced(&s), 1);
        assert!(!s.poll_stop_escalation(), "only once per stop");
        assert_eq!(forced(&s), 1);
    }

    #[test]
    fn a_running_sub_agent_makes_the_dot_spin() {
        let mut s = state();
        let key = seed_session_id(&mut s);
        // Nothing is known about the child process yet.
        assert_eq!(s.session_shown_activity(&key), None);

        // The child is waiting for the user: the dot blinks amber.
        s.activity
            .insert(key.clone(), app_core::activity::Activity::Waiting);
        assert_eq!(
            s.session_shown_activity(&key),
            Some(app_core::activity::Activity::Waiting)
        );

        // A sub-agent runs under the session: the session is working —
        // unless the agent is waiting for the user, which wins.
        s.activity.remove(&key);
        s.subagents.insert(
            key.clone(),
            vec![app_core::subagents::Subagent {
                agent: "devin".into(),
                pid: 4242,
                command: "devin".into(),
            }],
        );
        assert_eq!(
            s.session_shown_activity(&key),
            Some(app_core::activity::Activity::Working)
        );

        s.activity
            .insert(key.clone(), app_core::activity::Activity::Waiting);
        assert_eq!(
            s.session_shown_activity(&key),
            Some(app_core::activity::Activity::Waiting)
        );

        // When the agent goes quiet (no terminal output recently), the spinner stops.
        s.activity.remove(&key);
        s.note_session_output(&key, 100, Some(s.now_ms - 5000));
        assert_eq!(s.session_shown_activity(&key), None);

        // When fresh output arrives, it spins again.
        s.note_session_output(&key, 200, Some(s.now_ms));
        assert_eq!(
            s.session_shown_activity(&key),
            Some(app_core::activity::Activity::Working)
        );
    }

    #[test]
    fn the_activity_says_what_the_child_process_is_doing() {
        let mut s = state();
        let key = seed_session_id(&mut s);
        // The worker read the child's activity files.
        s.apply_reply(crate::editor::backend::Reply {
            req: Req::Activity {
                sessions: Vec::new(),
                previous: HashMap::new(),
            },
            generation: 0,
            checkout_key: None,
            result: Ok(crate::editor::backend::ReplyData::Activity(HashMap::from(
                [(key.clone(), app_core::activity::Activity::Working)],
            ))),
        });
        assert_eq!(
            s.session_activity(&key),
            Some(app_core::activity::Activity::Working)
        );
    }

    #[test]
    fn the_activity_read_asks_the_worker_about_the_running_sessions() {
        let mut s = state();
        let key = seed_session_id(&mut s);
        // The first poll asks; the next one waits for the tick, and then for
        // the first one's answer: a busy worker is not sent a second read.
        assert!(s.poll_activity(0.0));
        assert!(!s.poll_activity(0.5));
        assert!(!s.poll_activity(1.5));
        s.apply_reply(crate::editor::backend::Reply {
            req: Req::Activity {
                sessions: Vec::new(),
                previous: HashMap::new(),
            },
            generation: 0,
            checkout_key: None,
            result: Err("the worker was busy".into()),
        });
        assert!(s.poll_activity(1.6));
        // An answer that never comes holds the polls back only so long.
        assert!(!s.poll_activity(10.0));
        assert!(s.poll_activity(1.6 + POLL_REPLY_WAIT));
        let _ = key;
    }

    #[test]
    fn the_subagent_read_waits_for_the_previous_answer() {
        let mut s = state();
        seed_session_id(&mut s);
        assert!(s.poll_subagents(0.0));
        assert!(!s.poll_subagents(2.5));
        s.apply_reply(crate::editor::backend::Reply {
            req: Req::Subagents {
                sessions: Vec::new(),
                inspect: Default::default(),
            },
            generation: 0,
            checkout_key: None,
            result: Err("the worker was busy".into()),
        });
        assert!(s.poll_subagents(2.6));
    }

    /// A session in the scene, with its key.
    fn seed_session_id(s: &mut State) -> String {
        let info = daemon_session(DaemonState::Running);
        let key = crate::data::SessionInfo::key(info.id);
        s.upsert_session(info);
        key
    }

    #[test]
    fn the_requests_carry_the_session_the_ui_named() {
        let session = SessionId::new();
        let launch = SessionLaunch {
            argv: vec!["/bin/zsh".into()],
            cwd: "/work/repo".into(),
            project_id: Some("p1".into()),
            agent: "shell".into(),
            title: String::new(),
        };
        assert_eq!(
            create_request(7, &launch),
            Request::SessionCreate {
                id: 7,
                argv: vec!["/bin/zsh".into()],
                cwd: "/work/repo".into(),
                cols: DEFAULT_COLS,
                rows: DEFAULT_ROWS,
                title: String::new(),
            }
        );
        assert_eq!(
            stop_request(8, session, false),
            Request::SessionStop {
                id: 8,
                session_id: session,
                force: false,
            }
        );
        assert_eq!(
            stop_request(9, session, true),
            Request::SessionStop {
                id: 9,
                session_id: session,
                force: true,
            }
        );
        assert_eq!(
            delete_request(9, session),
            Request::SessionDelete {
                id: 9,
                session_id: session,
            }
        );
    }

    /// A custom command is split like a shell would: a quoted argument
    /// stays one word.
    #[test]
    fn a_custom_command_keeps_quoted_words_together() {
        assert_eq!(
            parse_custom_command("claude --model \"opus 4\"").unwrap(),
            vec![
                "claude".to_string(),
                "--model".to_string(),
                "opus 4".to_string()
            ]
        );
        assert_eq!(
            parse_custom_command("echo 'one two' three").unwrap(),
            vec![
                "echo".to_string(),
                "one two".to_string(),
                "three".to_string()
            ]
        );
    }

    /// An empty command and a malformed quote are refused with a reason.
    #[test]
    fn a_custom_command_that_cannot_run_is_refused() {
        assert_eq!(
            parse_custom_command("   ").unwrap_err(),
            "the command is empty"
        );
        assert_eq!(
            parse_custom_command("echo \"unclosed").unwrap_err(),
            "the command cannot be parsed; check the quotes"
        );
    }

    /// The refusal reaches the user: the notice explains why and nothing
    /// was started.
    #[test]
    fn an_unparseable_custom_command_shows_why() {
        let mut s = state();
        s.launch_custom_session(
            crate::sidebar::RowTarget::Project {
                project_id: "p1".into(),
            },
            "echo \"unclosed",
        );
        assert_eq!(
            s.notice.as_deref(),
            Some("the command cannot be parsed; check the quotes")
        );
        assert!(s.pending.is_empty(), "nothing was started");
    }

    #[test]
    fn a_generated_name_is_recognized_and_a_repo_name_is_not() {
        let name = generate_session_name();
        assert!(name.contains('-'), "name should be hyphenated: {name}");
        assert!(is_generated_name(&name), "a generated name is recognized");
        assert!(
            !is_generated_name("goko"),
            "a repo name is not a generated name"
        );
        assert!(
            !is_generated_name("random-unrelated-noun"),
            "words outside the lists are not a generated name"
        );
    }

    #[test]
    fn a_generated_name_becomes_the_repo_name_on_entering_it() {
        let mut s = state();
        let repo_dir = std::env::current_dir().unwrap();
        let git_root = State::git_root_for_cwd(&repo_dir).expect("should find git root");
        let expected_name = git_root.file_name().unwrap().to_str().unwrap().to_string();

        let mut info = daemon_session(DaemonState::Running);
        let key = crate::data::SessionInfo::key(info.id);
        info.title = "brave-meadow".to_string();
        info.cwd = repo_dir.display().to_string();

        s.session_metas.insert(
            key.clone(),
            SessionMeta {
                session_id: key.clone(),
                project_id: None,
                agent: "shell".into(),
                argv: vec!["/bin/bash".into()],
                cwd: "/tmp".into(),
                title: "brave-meadow".into(),
                created_at: 0,
                workspace_id: None,
            },
        );
        s.place_session(&key, "brave-meadow", Placement::Tab);

        // When upsert_session processes cwd in git repo, it should auto-rename
        s.upsert_session(info);

        let meta = s
            .session_metas
            .get(&key)
            .expect("session meta should exist");
        assert_eq!(
            meta.title, expected_name,
            "meta title should auto-rename to git repo name"
        );
        assert_eq!(
            s.open_tabs[0].title, expected_name,
            "tab title should auto-rename to git repo name"
        );
    }
}
