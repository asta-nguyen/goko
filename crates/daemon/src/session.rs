//! In-memory sessions: a validated state machine around each `PtyProcess`.

use crate::pty::{validate_size, PtyProcess, SpawnSpec};
use protocol::session::{SessionId, SessionInfo, SessionOutput, SessionState};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;
use tokio::time::timeout;
use tracing::warn;

/// Events a slow receiver may fall behind by before it must re-list.
const EVENT_CAPACITY: usize = 64;
/// How long `shutdown` waits after SIGTERM before sending SIGKILL.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
/// How long a polite stop waits after SIGHUP and SIGTERM before sending SIGKILL.
pub const STOP_GRACE: Duration = Duration::from_secs(2);

/// SIGKILL `pty`'s group unless it exits within [`STOP_GRACE`]. Outside a Tokio
/// runtime there is nothing to wait on, so the polite signals stand alone.
fn escalate_after_grace(pty: Arc<PtyProcess>) {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    runtime.spawn(async move {
        if timeout(STOP_GRACE, pty.wait_exit()).await.is_err() {
            let _ = pty.kill();
        }
    });
}
/// How soon after a PTY process printed output or took input its working
/// directory is read again: a shell changes directory by running a command,
/// which prints at least the next prompt.
const CWD_REFRESH_INTERVAL: Duration = Duration::from_millis(200);
/// How often the working directory of a process that prints nothing is read,
/// for a directory change that came with no output.
const CWD_IDLE_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    Spawning,
    Spawned,
    StopRequested,
    ProcessExited,
    SpawnFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidTransition {
    pub from: SessionState,
    pub event: Transition,
}

impl fmt::Display for InvalidTransition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "cannot apply {:?} to a session that is {:?}",
            self.event, self.from
        )
    }
}

impl std::error::Error for InvalidTransition {}

/// The only way session state changes: created→starting→running→stopping→exited,
/// running→exited, and created|starting→failed. Everything else is rejected.
pub fn transition(
    state: SessionState,
    event: Transition,
) -> Result<SessionState, InvalidTransition> {
    use SessionState::{Created, Exited, Failed, Running, Starting, Stopping};

    match (state, event) {
        (Created, Transition::Spawning) => Ok(Starting),
        (Starting, Transition::Spawned) => Ok(Running),
        (Running, Transition::StopRequested) => Ok(Stopping),
        (Running | Stopping, Transition::ProcessExited) => Ok(Exited),
        (Created | Starting, Transition::SpawnFailed) => Ok(Failed),
        (from, event) => Err(InvalidTransition { from, event }),
    }
}

#[derive(Debug)]
pub enum SessionError {
    InvalidArgument(String),
    NotFound(SessionId),
}

impl fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArgument(message) => formatter.write_str(message),
            Self::NotFound(id) => write!(formatter, "session {} not found", id.as_uuid()),
        }
    }
}

impl std::error::Error for SessionError {}

#[derive(Debug, Clone)]
pub struct CreateSpec {
    pub argv: Vec<String>,
    pub cwd: String,
    pub cols: u16,
    pub rows: u16,
    /// Empty means the basename of `argv[0]`.
    pub title: String,
}

struct Session {
    info: Mutex<SessionInfo>,
    pty: OnceLock<Arc<PtyProcess>>,
    events: broadcast::Sender<SessionInfo>,
}

impl Session {
    /// Apply `event` and broadcast the result. The broadcast happens under the lock so
    /// events for one session arrive in the order the states changed.
    fn apply(
        &self,
        event: Transition,
        update: impl FnOnce(&mut SessionInfo),
    ) -> Result<SessionInfo, InvalidTransition> {
        let mut info = lock(&self.info);
        info.state = transition(info.state, event)?;
        update(&mut info);
        let _ = self.events.send(info.clone());
        Ok(info.clone())
    }
}

/// Owns every session of one daemon. Share it behind an `Arc`.
pub struct SessionManager {
    sessions: Mutex<Vec<Arc<Session>>>,
    events: broadcast::Sender<SessionInfo>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(Vec::new()),
            events: broadcast::channel(EVENT_CAPACITY).0,
        }
    }

    /// Every state change, with the full `SessionInfo`. A receiver that lags should
    /// `list` again instead of replaying.
    pub fn subscribe_events(&self) -> broadcast::Receiver<SessionInfo> {
        self.events.subscribe()
    }

    /// Validate, spawn, and drive created→starting→running. A process that cannot be
    /// started (bad cwd, missing executable) is returned as a `failed` session.
    /// Must be called inside a Tokio runtime: the exit watcher is a task.
    pub fn create(&self, spec: CreateSpec) -> Result<SessionInfo, SessionError> {
        let command = spec
            .argv
            .first()
            .filter(|command| !command.is_empty())
            .ok_or_else(|| SessionError::InvalidArgument("argv must name a command".to_owned()))?;
        validate_size(spec.cols, spec.rows)
            .map_err(|error| SessionError::InvalidArgument(error.to_string()))?;
        let title = match spec.title.trim() {
            "" => Path::new(command).file_name().map_or_else(
                || command.clone(),
                |name| name.to_string_lossy().into_owned(),
            ),
            title => title.to_owned(),
        };

        let session = Arc::new(Session {
            info: Mutex::new(SessionInfo {
                id: SessionId::new(),
                title,
                argv: spec.argv.clone(),
                cwd: spec.cwd.clone(),
                state: SessionState::Created,
                pid: None,
                exit_code: None,
                error: None,
                started_at: now_ms(),
                ended_at: None,
            }),
            pty: OnceLock::new(),
            events: self.events.clone(),
        });
        session
            .apply(Transition::Spawning, |_| {})
            .expect("a new session can always start spawning");

        let spawned = PtyProcess::spawn(SpawnSpec {
            argv: spec.argv,
            cwd: PathBuf::from(spec.cwd),
            cols: spec.cols,
            rows: spec.rows,
        });
        let info = match spawned {
            Ok(pty) => {
                let pty = Arc::new(pty);
                let _ = session.pty.set(Arc::clone(&pty));
                lock(&self.sessions).push(Arc::clone(&session));
                let info = session
                    .apply(Transition::Spawned, |info| info.pid = Some(pty.pid()))
                    .expect("a starting session can always run");
                tokio::spawn(watch_exit(Arc::clone(&session), pty));
                info
            }
            Err(error) => {
                lock(&self.sessions).push(Arc::clone(&session));
                session
                    .apply(Transition::SpawnFailed, |info| {
                        info.error = Some(error.to_string());
                        info.ended_at = Some(now_ms());
                    })
                    .expect("a starting session can always fail")
            }
        };
        Ok(info)
    }

    /// All sessions in creation order; exited and failed ones stay listed.
    pub fn list(&self) -> Vec<SessionInfo> {
        let sessions = lock(&self.sessions).clone();
        sessions
            .iter()
            .map(|session| {
                if let Some(pty) = session.pty.get() {
                    refresh_cwd(session, pty);
                }
                lock(&session.info).clone()
            })
            .collect()
    }

    /// Output statistics for all live sessions.
    pub fn output(&self) -> Vec<SessionOutput> {
        let sessions = lock(&self.sessions).clone();
        sessions
            .iter()
            .map(|session| {
                let session_id = lock(&session.info).id;
                let (output_seq, last_output_at) = session
                    .pty
                    .get()
                    .map(|pty| pty.output_info())
                    .unwrap_or((0, None));
                SessionOutput {
                    session_id,
                    output_seq,
                    last_output_at,
                }
            })
            .collect()
    }

    pub fn get(&self, id: SessionId) -> Result<SessionInfo, SessionError> {
        let session = self.find(id)?;
        if let Some(pty) = session.pty.get() {
            refresh_cwd(&session, pty);
        }
        let info = lock(&session.info).clone();
        Ok(info)
    }

    /// The process behind a session, for terminal I/O. A failed session has none.
    pub fn pty(&self, id: SessionId) -> Result<Arc<PtyProcess>, SessionError> {
        self.find(id)?
            .pty
            .get()
            .cloned()
            .ok_or_else(|| SessionError::InvalidArgument("the session has no process".to_owned()))
    }

    /// Stop the session's process group: SIGHUP then SIGTERM, as closing a terminal
    /// window does — an interactive shell ignores SIGTERM alone — and SIGKILL if the
    /// group is still alive after [`STOP_GRACE`], so closing a shell never needs a
    /// manual force kill. `force` sends SIGKILL at once. Idempotent: a repeat stop
    /// signals nothing, except that `force` escalates a pending stop to SIGKILL, and an
    /// exited or failed session is returned untouched.
    pub fn stop(&self, id: SessionId, force: bool) -> Result<SessionInfo, SessionError> {
        let session = self.find(id)?;
        let mut info = lock(&session.info);
        let pty = match info.state {
            SessionState::Exited | SessionState::Failed => return Ok(info.clone()),
            SessionState::Stopping if !force => return Ok(info.clone()),
            SessionState::Created | SessionState::Starting => {
                return Err(SessionError::InvalidArgument(
                    "the session has not started yet".to_owned(),
                ));
            }
            _ => session.pty.get().expect("a started session has a process"),
        };

        if info.state == SessionState::Running {
            info.state = transition(info.state, Transition::StopRequested)
                .expect("a running session can be stopped");
            let _ = session.events.send(info.clone());
        }
        let signalled = if force {
            pty.kill()
        } else {
            let hung_up = pty.hangup();
            let terminated = pty.terminate();
            escalate_after_grace(pty.clone());
            hung_up.and(terminated)
        };
        if let Err(error) = signalled {
            // The group is already gone; the exit watcher is about to record the exit.
            warn!("could not signal session {}: {error}", id.as_uuid());
        }
        Ok(info.clone())
    }

    /// Remove a finished session and its replay state. An absent id is already deleted,
    /// making a retry after partial app-side cleanup safe.
    pub fn delete(&self, id: SessionId) -> Result<(), SessionError> {
        let mut sessions = lock(&self.sessions);
        let Some(index) = sessions
            .iter()
            .position(|session| lock(&session.info).id == id)
        else {
            return Ok(());
        };

        let session = &sessions[index];
        let state = lock(&session.info).state;
        if !is_finished(state) {
            return Err(SessionError::InvalidArgument(
                "cannot delete a session before its process exits".to_owned(),
            ));
        }
        if let Some(pty) = session.pty.get() {
            pty.close_subscribers();
        }
        sessions.remove(index);
        Ok(())
    }

    /// SIGTERM every live process, wait up to two seconds, then SIGKILL the rest. Returns
    /// once every process has exited.
    pub async fn shutdown(&self) {
        let live: Vec<Arc<PtyProcess>> = lock(&self.sessions)
            .iter()
            .filter(|session| !is_finished(lock(&session.info).state))
            .filter_map(|session| session.pty.get().cloned())
            .collect();

        for pty in &live {
            let _ = pty.terminate();
        }
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        for pty in &live {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if timeout(remaining, pty.wait_exit()).await.is_err() {
                let _ = pty.kill();
                let _ = pty.wait_exit().await;
            }
        }
    }

    fn find(&self, id: SessionId) -> Result<Arc<Session>, SessionError> {
        lock(&self.sessions)
            .iter()
            .find(|session| lock(&session.info).id == id)
            .cloned()
            .ok_or(SessionError::NotFound(id))
    }
}

/// Record the exit. A process killed by a signal has no exit code.
async fn watch_exit(session: Arc<Session>, pty: Arc<PtyProcess>) {
    let mut idle = tokio::time::interval(CWD_IDLE_INTERVAL);
    idle.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // The directory is read `CWD_REFRESH_INTERVAL` after activity, so a flood
    // of output reads it as often as the fixed timer used to and an idle
    // session only on the slow tick. A new process counts as active: its
    // command may change directory at once.
    let settle = tokio::time::sleep(CWD_REFRESH_INTERVAL);
    tokio::pin!(settle);
    let mut settling = true;
    let exit_code = loop {
        tokio::select! {
            status = pty.wait_exit() => {
                break match status {
                    Ok(status) if status.signal.is_none() => i32::try_from(status.code).ok(),
                    _ => None,
                };
            }
            _ = pty.activity(), if !settling => {
                settling = true;
                settle
                    .as_mut()
                    .reset(tokio::time::Instant::now() + CWD_REFRESH_INTERVAL);
            }
            _ = &mut settle, if settling => {
                settling = false;
                refresh_cwd(&session, &pty);
            }
            _ = idle.tick() => refresh_cwd(&session, &pty),
        }
    };
    let _ = session.apply(Transition::ProcessExited, |info| {
        info.exit_code = exit_code;
        info.ended_at = Some(now_ms());
    });
}

fn refresh_cwd(session: &Session, pty: &PtyProcess) {
    // An exiting (zombie) process has no readable cwd: keep the last one seen
    // rather than recording an empty directory for the finished session.
    let Some(cwd) = pty
        .current_dir()
        .and_then(|path| path.into_os_string().into_string().ok())
    else {
        return;
    };
    let mut info = lock(&session.info);
    if is_finished(info.state) || info.cwd == cwd {
        return;
    }
    info.cwd = cwd;
    let _ = session.events.send(info.clone());
}

fn is_finished(state: SessionState) -> bool {
    matches!(state, SessionState::Exited | SessionState::Failed)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::{transition, InvalidTransition, Transition};
    use protocol::session::SessionState::{
        self, Created, Disconnected, Exited, Failed, Idle, Interrupted, Running, Starting,
        Stopping, Unknown, WaitingForInput,
    };

    const STATES: [SessionState; 11] = [
        Created,
        Starting,
        Running,
        WaitingForInput,
        Idle,
        Stopping,
        Exited,
        Failed,
        Disconnected,
        Interrupted,
        Unknown,
    ];
    const EVENTS: [Transition; 5] = [
        Transition::Spawning,
        Transition::Spawned,
        Transition::StopRequested,
        Transition::ProcessExited,
        Transition::SpawnFailed,
    ];

    #[test]
    fn exactly_the_allowed_edges_succeed() {
        let allowed = [
            (Created, Transition::Spawning, Starting),
            (Starting, Transition::Spawned, Running),
            (Running, Transition::StopRequested, Stopping),
            (Running, Transition::ProcessExited, Exited),
            (Stopping, Transition::ProcessExited, Exited),
            (Created, Transition::SpawnFailed, Failed),
            (Starting, Transition::SpawnFailed, Failed),
        ];

        for state in STATES {
            for event in EVENTS {
                let expected = allowed
                    .iter()
                    .find(|(from, on, _)| *from == state && *on == event)
                    .map(|(_, _, to)| *to)
                    .ok_or(InvalidTransition { from: state, event });
                assert_eq!(transition(state, event), expected, "{state:?} + {event:?}");
            }
        }
    }

    #[test]
    fn exited_and_failed_accept_no_event() {
        for state in [Exited, Failed] {
            for event in EVENTS {
                assert!(transition(state, event).is_err(), "{state:?} + {event:?}");
            }
        }
    }
}
