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

use rune::Tx;

use crate::State;
use crate::daemon::UiEvent;
use crate::data::{self, SessionView};
use crate::editor::backend::{self, Job, Req};

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
        title: String::new(),
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
    /// `session.create`: the new session opens a tab and records metadata.
    Create {
        reply: Receiver<Result<SessionInfo, BridgeError>>,
        launch: SessionLaunch,
    },
    /// `session.stop`: the daemon's answer updates the row.
    Stop {
        reply: Receiver<Result<SessionInfo, BridgeError>>,
    },
    /// `session.delete`: the row, its metadata and its tab go away.
    Delete {
        reply: Receiver<Result<serde_json::Value, BridgeError>>,
        key: String,
    },
}

impl State {
    /// Start a session in the daemon and remember why it exists.
    pub fn launch_session(&mut self, launch: SessionLaunch) {
        let asked = launch.clone();
        let reply = self
            .daemon
            .call_json::<SessionInfo>(move |id| create_request(id, &asked));
        match reply {
            Some(reply) => self.pending.push(Pending::Create { reply, launch }),
            None => self.notice = Some("the Goko daemon is not reachable".into()),
        }
    }

    /// Stop a session: the daemon decides when it is really gone, and the
    /// row follows its `session.updated` event. `force` kills the process
    /// group outright.
    pub fn stop_session(&mut self, key: &str, force: bool) {
        let Some(session_id) = self.session_id(key) else {
            return;
        };
        let reply = self
            .daemon
            .call_json::<SessionInfo>(move |id| stop_request(id, session_id, force));
        match reply {
            Some(reply) => self.pending.push(Pending::Stop { reply }),
            None => self.notice = Some("the Goko daemon is not reachable".into()),
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
    /// and its tab.
    pub fn delete_session(&mut self, key: &str) {
        let Some(session_id) = self.session_id(key) else {
            return;
        };
        let reply = self
            .daemon
            .call_json::<serde_json::Value>(move |id| delete_request(id, session_id));
        match reply {
            Some(reply) => self.pending.push(Pending::Delete {
                reply,
                key: key.to_string(),
            }),
            None => self.notice = Some("the Goko daemon is not reachable".into()),
        }
    }

    /// Apply one daemon event to the app's session state.
    pub fn apply_daemon_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::Connected { version } => {
                self.data.connected = true;
                self.daemon_version = Some(version);
                // The connection was replaced: the output route died with
                // the old one, so the attached terminal takes a fresh
                // subscription — the daemon replays its retained output.
                self.resubscribe_attached();
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
            }
            UiEvent::Event(event) => self.apply_session_event(event),
        }
    }

    /// Take a fresh output subscription for the attached terminal.
    fn resubscribe_attached(&mut self) {
        let Some(session) = self.terminal.as_ref().map(|terminal| terminal.session) else {
            return;
        };
        let flow = self.daemon.supports_flow_control();
        let Some((chunks, ready)) = self.daemon.attach_output(session, flow) else {
            return;
        };
        if let Some(terminal) = self.terminal.as_mut() {
            terminal.resubscribe(chunks, ready);
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
                // The terminal resubscribes on the next poll (see `term`).
                self.terminal_lagged = Some(session_id);
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
                Pending::Create { reply, launch } => match reply.try_recv() {
                    Ok(Ok(info)) => {
                        self.session_created(info, &launch);
                        changed = true;
                    }
                    Ok(Err(error)) => {
                        self.notice = Some(format!("could not start the session: {error}"));
                        changed = true;
                    }
                    Err(TryRecvError::Empty) => waiting.push(Pending::Create { reply, launch }),
                    Err(TryRecvError::Disconnected) => {
                        self.notice = Some("the Goko daemon is not reachable".into());
                        changed = true;
                    }
                },
                Pending::Stop { reply } => match reply.try_recv() {
                    Ok(Ok(info)) => {
                        self.upsert_session(info);
                        changed = true;
                    }
                    Ok(Err(error)) => {
                        self.notice = Some(format!("could not stop the session: {error}"));
                        changed = true;
                    }
                    Err(TryRecvError::Empty) => waiting.push(Pending::Stop { reply }),
                    Err(TryRecvError::Disconnected) => {
                        self.notice = Some("the Goko daemon is not reachable".into());
                        changed = true;
                    }
                },
                Pending::Delete { reply, key } => match reply.try_recv() {
                    Ok(Ok(_)) => {
                        self.session_deleted(&key);
                        changed = true;
                    }
                    Ok(Err(error)) => {
                        self.notice = Some(format!("could not delete the session: {error}"));
                        changed = true;
                    }
                    Err(TryRecvError::Empty) => waiting.push(Pending::Delete { reply, key }),
                    Err(TryRecvError::Disconnected) => {
                        self.notice = Some("the Goko daemon is not reachable".into());
                        changed = true;
                    }
                },
            }
        }
        self.pending = waiting;
        changed
    }

    /// A session exists: record why it exists, open its tab and select it.
    fn session_created(&mut self, info: SessionInfo, launch: &SessionLaunch) {
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
        if !self.open_tabs.contains(&key) {
            self.open_tabs.push(key.clone());
        }
        self.active_tab = self
            .open_tabs
            .iter()
            .position(|tab| *tab == key)
            .unwrap_or(self.open_tabs.len().saturating_sub(1));
        self.selected_session = Some(key);
        self.active_workspace = self
            .selected_session
            .as_deref()
            .and_then(|selected| self.session_by_key(selected))
            .map(|view| self.session_workspace(view));
    }

    /// A session is gone: drop its row, its metadata and its tab.
    fn session_deleted(&mut self, key: &str) {
        self.session_metas.remove(key);
        backend::enqueue(
            &mut self.editor.backend,
            Job::project(Req::DeleteSessionMeta(key.to_string())),
        );
        self.data.sessions.retain(|view| view.info.id != key);
        if let Some(index) = self.open_tabs.iter().position(|tab| tab == key) {
            self.open_tabs.remove(index);
            if index < self.active_tab {
                self.active_tab -= 1;
            }
        }
        if self.active_tab >= self.open_tabs.len() {
            self.active_tab = self.open_tabs.len().saturating_sub(1);
        }
        if self.selected_session.as_deref() == Some(key) {
            self.selected_session = None;
            self.active_workspace = None;
        }
        self.sync_selection();
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

    /// The tab the session belongs to, opening one when it has none.
    pub fn focus_session_tab(&mut self, key: &str) {
        match self.open_tabs.iter().position(|tab| tab == key) {
            Some(index) => self.active_tab = index,
            None => {
                self.open_tabs.push(key.to_string());
                self.active_tab = self.open_tabs.len() - 1;
            }
        }
    }

    /// Point the selection at whatever `active_tab` resolves to after the
    /// tab list changed; clears it when the active tab has no session.
    /// Shared by tab close/select, New tab and Close pane.
    pub fn sync_selection(&mut self) {
        let found = self.open_tabs.get(self.active_tab).and_then(|key| {
            let view = self.session_by_key(key)?;
            Some((view.info.id.clone(), self.session_workspace(view)))
        });
        match found {
            Some((key, workspace)) => {
                self.selected_session = Some(key);
                self.active_workspace = Some(workspace);
            }
            None => {
                self.selected_session = None;
                self.active_workspace = None;
            }
        }
    }

    /// Attach the terminal to the selected session — and detach it from the
    /// one it was showing. The Terminal view owns the attachment: the
    /// Editor view never holds one.
    pub fn sync_terminal(&mut self) -> bool {
        let wanted = if self.view == crate::View::Terminal {
            self.selected_session
                .as_deref()
                .and_then(|key| self.session_by_key(key))
                .map(|view| view.info.session_id)
        } else {
            None
        };
        let attached = self.terminal.as_ref().map(|terminal| terminal.session);
        if attached == wanted {
            return false;
        }
        if let Some(terminal) = self.terminal.take() {
            // The old session stops streaming: the daemon is told to stop.
            self.daemon.detach_output(terminal.session);
        }
        self.lines.clear();
        let Some(session) = wanted else {
            return true;
        };
        let flow = self.daemon.supports_flow_control();
        let Some((chunks, ready)) = self.daemon.attach_output(session, flow) else {
            return true;
        };
        self.terminal = Some(crate::term::Terminal::new(session, chunks, ready));
        // The PTY must match the grid we render, so the daemon is told the
        // terminal's size right after the attach.
        self.terminal_pty = (0, 0);
        self.resize_terminal_grid();
        true
    }

    /// Feed the daemon's output into the attached terminal and refresh the
    /// rendered rows. Returns whether the grid changed.
    pub fn poll_terminal(&mut self, tx: &mut Tx) -> bool {
        let mut changed = false;
        let ink = self.ink;
        let flow = self.daemon.supports_flow_control();
        let mut ack = None;
        if let Some(terminal) = self.terminal.as_mut() {
            // A subscribe the daemon refused means there is nothing to
            // show: drop the attachment and try again later.
            if let Some(Err(error)) = terminal.subscribe_reply() {
                self.notice = Some(format!("the terminal could not attach: {error}"));
                let session = terminal.session;
                self.terminal = None;
                self.daemon.detach_output(session);
                self.lines.clear();
                return true;
            }
            terminal.feed();
            // A repaint that waits for the output to settle asks for the
            // frame that will draw it — without that, the grid would
            // stay empty until something else woke the app.
            if let Some(wait) = terminal.hold() {
                tx.wake_after(wait);
            }
            if flow && terminal.unacked() > 0 {
                ack = Some((terminal.session, terminal.next_seq));
            }
            let lines = terminal.lines(&ink);
            if self.lines != lines {
                self.lines = lines.to_vec();
                changed = true;
            }
        }
        // A flow-controlled stream waits for the acknowledgement, so the
        // bytes we processed are reported back.
        if let Some((session, upto_seq)) = ack {
            self.ack_terminal(session, upto_seq);
            if let Some(terminal) = self.terminal.as_mut() {
                terminal.acknowledge(upto_seq);
            }
        }
        changed
    }

    /// The subscriber fell behind: drop the subscription and take a fresh
    /// one, which replays the daemon's retained output.
    pub fn resubscribe_terminal(&mut self, session: SessionId) -> bool {
        if self
            .terminal
            .as_ref()
            .is_none_or(|terminal| terminal.session != session)
        {
            return false;
        }
        self.terminal = None;
        self.lines.clear();
        self.daemon.detach_output(session);
        self.sync_terminal()
    }

    /// Send bytes to the attached session's PTY.
    pub fn write_terminal(&mut self, session: SessionId, bytes: &[u8]) {
        let data_b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        let _ = self.daemon.call(move |id| Request::TerminalWrite {
            id,
            session_id: session,
            data_b64: data_b64.clone(),
        });
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

    /// Fit the grid to the pane and tell the daemon the same size: the
    /// session draws in the space the pane has, so a full-screen program
    /// never makes the view slide.
    pub fn resize_terminal_grid(&mut self) {
        let (cols, rows) = (self.terminal_cols, self.terminal_rows);
        let session = self.terminal.as_ref().map(|terminal| terminal.session);
        if let Some(terminal) = self.terminal.as_mut()
            && terminal.size() != (cols, rows)
        {
            terminal.resize(cols, rows);
        }
        // The daemon only hears about a size it does not know yet.
        if let Some(session) = session
            && self.terminal_pty != (cols, rows)
        {
            self.terminal_pty = (cols, rows);
            self.resize_terminal(session, cols as u16, rows as u16);
        }
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

    /// A line describing the terminal's state, for the trace.
    pub fn terminal_trace(&self) -> String {
        let grid = self
            .terminal
            .as_ref()
            .map(|terminal| {
                let (cols, rows) = terminal.size();
                format!("{cols}x{rows}")
            })
            .unwrap_or_else(|| "none".to_string());
        let head: String = self
            .lines
            .first()
            .map(|line| line.text().chars().take(60).collect())
            .unwrap_or_default();
        format!("grid={grid} lines={} head={head:?}", self.lines.len())
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
    pub fn request_session_argv(&mut self, target: crate::sidebar::RowTarget, agent: String) {
        let reply = crate::editor::backend::enqueue(
            &mut self.editor.backend,
            crate::editor::backend::Job::project(crate::editor::backend::Req::AgentArgv {
                agent: agent.clone(),
            }),
        );
        if !reply {
            self.notice = Some("the editor queue is full; the agent was not resolved".into());
            return;
        }
        self.pending_argv = Some(PendingArgv { target, agent });
    }

    /// Remove a linked worktree through the worker lane.
    pub fn remove_worktree(&mut self, project_id: &str, path: &str) {
        let _ = crate::editor::backend::enqueue(
            &mut self.editor.backend,
            crate::editor::backend::Job::project(crate::editor::backend::Req::RemoveWorktree {
                project_id: project_id.to_string(),
                path: path.to_string(),
            }),
        );
    }

    /// Remove a registered project through the worker lane.
    pub fn remove_project(&mut self, project_id: &str) {
        let _ = crate::editor::backend::enqueue(
            &mut self.editor.backend,
            crate::editor::backend::Job::project(crate::editor::backend::Req::RemoveProject {
                id: project_id.to_string(),
            }),
        );
    }

    /// Start a session with a custom command in the row's checkout. The
    /// command is split on whitespace; an empty one is refused.
    pub fn launch_custom_session(&mut self, target: crate::sidebar::RowTarget, command: &str) {
        let argv: Vec<String> = command.split_whitespace().map(str::to_string).collect();
        if argv.is_empty() {
            self.notice = Some("the command is empty".into());
            return;
        }
        let (cwd, project_id) = self.checkout_of(&target);
        self.launch_session(SessionLaunch {
            argv,
            cwd,
            project_id,
            agent: "custom".to_string(),
        });
    }

    /// The checkout a row's session runs in, and the project it belongs to.
    pub fn checkout_of(&self, target: &crate::sidebar::RowTarget) -> (String, Option<String>) {
        let project_id = match target {
            crate::sidebar::RowTarget::Project { project_id }
            | crate::sidebar::RowTarget::Worktree { project_id, .. } => Some(project_id.clone()),
            crate::sidebar::RowTarget::Session { .. } => None,
        };
        let path = match target {
            crate::sidebar::RowTarget::Project { project_id } => self
                .editor
                .projects
                .iter()
                .find(|project| project.project.id == *project_id)
                .map(|project| project.project.root.clone()),
            crate::sidebar::RowTarget::Worktree { path, .. } => Some(path.clone()),
            crate::sidebar::RowTarget::Session { .. } => None,
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

/// A launch waiting for its agent's argv.
pub struct PendingArgv {
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
}
