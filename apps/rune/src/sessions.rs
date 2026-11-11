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
pub const DEFAULT_COLS: u16 = 80;
pub const DEFAULT_ROWS: u16 = 24;

/// How long a session may be stopping before the force kill is offered.
pub const FORCE_KILL_AFTER_MS: i64 = 5_000;

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

    /// A shell in the active checkout, else `$HOME`: New tab and ⌘T.
    pub fn new_shell_tab(&mut self) {
        // A real shell session in the active checkout: the daemon owns the
        // session, the app records why it exists.
        let cwd = self
            .editor
            .checkout
            .as_ref()
            .map(|path| path.display().to_string())
            .or_else(|| std::env::var("HOME").ok())
            .unwrap_or_else(|| "/".to_string());
        let project_id = self.editor.project_id.clone();
        self.launch_session(SessionLaunch {
            argv: vec![backend::default_shell()],
            cwd,
            project_id,
            agent: "shell".to_string(),
            title: String::new(),
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
                self.pending.push(Pending::Create {
                    reply,
                    launch,
                    placement,
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
                if let Some(view) = self
                    .data
                    .sessions
                    .iter_mut()
                    .find(|view| view.info.id == key)
                {
                    view.info.state = data::SessionState::Exited;
                    view.info.exit_code = exit_code;
                    if view.info.ended_at.is_none() {
                        view.info.ended_at = Some(self.now_ms);
                    }
                }
            }
            Event::TerminalLagged { session_id } => {
                // The pane's terminal resubscribes on the next poll.
                self.terminal_lagged.insert(session_id);
            }
        }
    }

    /// Insert or replace a session row, keeping the stored metadata join.
    pub fn upsert_session(&mut self, info: SessionInfo) {
        let view = self.view_for(info);
        let key = view.info.id.clone();
        let stopping = view.info.state == data::SessionState::Stopping;
        let was_stopping = self
            .data
            .sessions
            .iter()
            .find(|slot| slot.info.id == key)
            .is_some_and(|slot| slot.info.state == data::SessionState::Stopping);
        match self
            .data
            .sessions
            .iter_mut()
            .find(|slot| slot.info.id == key)
        {
            Some(slot) => *slot = view,
            None => self.data.sessions.push(view),
        }
        // The force kill is offered once the session has been stopping for
        // a while.
        if stopping && !was_stopping {
            self.stopping_since.insert(key, self.now_ms);
        } else if !stopping {
            self.stopping_since.remove(&key);
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
                } => match reply.try_recv() {
                    Ok(Ok(info)) => {
                        self.session_created(info, &launch, placement);
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
        changed
    }

    /// A session exists: record why it exists, then place it — its own
    /// tab or a split pane, per the launch's placement.
    fn session_created(&mut self, info: SessionInfo, launch: &SessionLaunch, placement: Placement) {
        let view = self.view_for(info.clone());
        let key = view.info.id.clone();
        let title = view.title();
        let meta = SessionMeta {
            session_id: key.clone(),
            project_id: launch.project_id.clone(),
            agent: launch.agent.clone(),
            argv: info.argv.clone(),
            cwd: info.cwd.clone(),
            title: title.clone(),
            created_at: self.now_ms,
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
    pub fn view_for(&self, info: SessionInfo) -> SessionView {
        let key = data::SessionInfo::key(info.id);
        let meta = self.session_metas.get(&key).cloned();
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
        let Some(project_id) = view
            .meta
            .as_ref()
            .and_then(|meta| meta.project_id.as_deref())
        else {
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
        if self.activity_at.is_some_and(|at| now - at < 1.0) {
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
        crate::trace::trace(&format!("activity read for {} sessions", sessions.len()));
        backend::enqueue(
            &mut self.editor.backend,
            Job::project(Req::Activity { sessions, previous }),
        )
    }

    /// What the session's child process is doing.
    pub fn session_activity(&self, key: &str) -> Option<app_core::activity::Activity> {
        self.activity.get(key).copied()
    }

    /// The activity the dot shows: a running sub-agent means the session
    /// is working — unless the agent is waiting for the user.
    pub fn session_shown_activity(&self, key: &str) -> Option<app_core::activity::Activity> {
        let activity = self.session_activity(key);
        let has_subagents = !self.session_subagents(key).is_empty();
        if has_subagents && activity != Some(app_core::activity::Activity::Waiting) {
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
        backend::enqueue(
            &mut self.editor.backend,
            Job::project(Req::Subagents { sessions }),
        )
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
        // The first poll asks; the next one waits for the tick.
        assert!(s.poll_activity(0.0));
        assert!(!s.poll_activity(0.5));
        assert!(s.poll_activity(1.5));
        let _ = key;
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
}
