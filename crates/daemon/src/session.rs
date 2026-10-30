//! In-memory sessions: a validated state machine around each `PtyProcess`.

use crate::pty::{validate_size, PtyProcess, SpawnSpec};
use protocol::session::{SessionId, SessionInfo, SessionState};
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
                    .apply(Transition::Spawned, |_| {})
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
        lock(&self.sessions)
            .iter()
            .map(|session| lock(&session.info).clone())
            .collect()
    }

    pub fn get(&self, id: SessionId) -> Result<SessionInfo, SessionError> {
        Ok(lock(&self.find(id)?.info).clone())
    }

    /// The process behind a session, for terminal I/O. A failed session has none.
    pub fn pty(&self, id: SessionId) -> Result<Arc<PtyProcess>, SessionError> {
        self.find(id)?
            .pty
            .get()
            .cloned()
            .ok_or_else(|| SessionError::InvalidArgument("the session has no process".to_owned()))
    }

    /// SIGTERM the session's process group (SIGKILL when `force`). Idempotent: a repeat
    /// stop signals nothing, except that `force` escalates a pending stop to SIGKILL, and
    /// an exited or failed session is returned untouched.
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
        let signalled = if force { pty.kill() } else { pty.terminate() };
        if let Err(error) = signalled {
            // The group is already gone; the exit watcher is about to record the exit.
            warn!("could not signal session {}: {error}", id.as_uuid());
        }
        Ok(info.clone())
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
    let exit_code = match pty.wait_exit().await {
        Ok(status) if status.signal.is_none() => i32::try_from(status.code).ok(),
        _ => None,
    };
    let _ = session.apply(Transition::ProcessExited, |info| {
        info.exit_code = exit_code;
        info.ended_at = Some(now_ms());
    });
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
