//! The app's single daemon connection, shared by commands, the event pump, and terminals.

use crate::daemon_client::DaemonClient;
use crate::error::{BridgeError, Code};
use crate::terminal;
use protocol::message::{ErrorCode, Request};
use serde_json::Value;
use std::fs::{DirBuilder, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::time::sleep;

/// A cheap handle: clones share one connection.
#[derive(Clone)]
pub struct Bridge(Arc<Inner>);

struct Inner {
    home: PathBuf,
    client: Mutex<Option<Arc<DaemonClient>>>,
    /// Held while connecting so racing callers make exactly one attempt.
    connecting: tokio::sync::Mutex<()>,
    shutting_down: AtomicBool,
    /// Serializes worktree operations within this bridge; the file lock covers other processes.
    worktree_operations: tokio::sync::RwLock<()>,
    /// Started by the first terminal subscription.
    terminals: OnceLock<mpsc::UnboundedSender<terminal::Command>>,
}

pub(crate) struct WorktreeOperationGuard(std::fs::File);

impl Drop for WorktreeOperationGuard {
    fn drop(&mut self) {
        // SAFETY: this descriptor owns the advisory lock acquired by `worktree_operation_guard`.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

impl Bridge {
    pub fn new(home: PathBuf) -> Self {
        Self(Arc::new(Inner {
            home,
            client: Mutex::new(None),
            connecting: tokio::sync::Mutex::new(()),
            shutting_down: AtomicBool::new(false),
            worktree_operations: tokio::sync::RwLock::new(()),
            terminals: OnceLock::new(),
        }))
    }

    pub fn home(&self) -> &Path {
        &self.0.home
    }

    pub(crate) fn terminals(&self) -> &OnceLock<mpsc::UnboundedSender<terminal::Command>> {
        &self.0.terminals
    }

    pub(crate) fn worktree_operations(&self) -> &tokio::sync::RwLock<()> {
        &self.0.worktree_operations
    }

    /// Coordinate launches and removals across desktop processes sharing this GOKO_HOME.
    pub(crate) async fn worktree_operation_guard(
        &self,
        exclusive: bool,
    ) -> Result<WorktreeOperationGuard, BridgeError> {
        let home = self.home().to_owned();
        let file = tokio::task::spawn_blocking(move || -> io::Result<std::fs::File> {
            let mut directory = DirBuilder::new();
            directory.recursive(true).mode(0o700);
            match directory.create(&home) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists && home.is_dir() => {}
                Err(error) => return Err(error),
            }
            let mut options = OpenOptions::new();
            options
                .create(true)
                .read(true)
                .write(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW);
            let file = options.open(home.join("worktree-operations.lock"))?;
            let operation = if exclusive {
                libc::LOCK_EX
            } else {
                libc::LOCK_SH
            };
            // SAFETY: `file` remains open for the lifetime of the returned guard.
            if unsafe { libc::flock(file.as_raw_fd(), operation) } == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(file)
        })
        .await
        .map_err(|error| {
            BridgeError::new(
                Code::Daemon(ErrorCode::Internal),
                format!("cannot acquire the worktree operation lock: {error}"),
            )
        })?
        .map_err(|error| {
            BridgeError::new(
                Code::Daemon(ErrorCode::Internal),
                format!("cannot acquire the worktree operation lock: {error}"),
            )
        })?;
        Ok(WorktreeOperationGuard(file))
    }

    /// The live connection, if there is one. Never connects or spawns.
    pub fn current(&self) -> Option<Arc<DaemonClient>> {
        let client = self.0.client.lock().unwrap();
        client.clone().filter(|client| !client.is_closed())
    }

    /// The live connection, connecting (and starting the daemon) first when needed.
    pub async fn client(&self) -> Result<Arc<DaemonClient>, BridgeError> {
        if self.0.shutting_down.load(Ordering::Acquire) {
            return Err(disconnected());
        }
        if let Some(client) = self.current() {
            return Ok(client);
        }
        let _one_attempt = self.0.connecting.lock().await;
        if self.0.shutting_down.load(Ordering::Acquire) {
            return Err(disconnected());
        }
        if let Some(client) = self.current() {
            return Ok(client);
        }
        let client = Arc::new(DaemonClient::connect_or_spawn(&self.0.home).await?);
        *self.0.client.lock().unwrap() = Some(Arc::clone(&client));
        Ok(client)
    }

    /// Stop the existing daemon without starting a replacement.
    pub async fn shutdown(&self) -> Result<(), BridgeError> {
        self.0.shutting_down.store(true, Ordering::Release);
        let _connecting = self.0.connecting.lock().await;
        let result = match self.current() {
            Some(client) => shutdown_client(&client, &self.0.home).await,
            None => shutdown_existing(&self.0.home).await,
        };
        if result.is_err() {
            self.0.shutting_down.store(false, Ordering::Release);
        }
        result
    }

    /// `DaemonClient::request` on the shared connection.
    pub async fn call(&self, build: impl FnOnce(u64) -> Request) -> Result<Value, BridgeError> {
        self.client().await?.request(build).await
    }
}

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

async fn shutdown_client(client: &DaemonClient, home: &Path) -> Result<(), BridgeError> {
    match client.request(|id| Request::SystemShutdown { id }).await {
        Ok(_) => wait_for_daemon_exit(home).await,
        Err(error) if matches!(error.code, Code::Disconnected | Code::DaemonUnavailable) => {
            wait_for_daemon_exit(home).await
        }
        Err(error) => Err(error),
    }
}

async fn shutdown_existing(home: &Path) -> Result<(), BridgeError> {
    let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
    loop {
        match DaemonClient::connect(home).await {
            Ok(client) => return shutdown_client(&client, home).await,
            Err(error) if matches!(error.code, Code::DaemonUnavailable | Code::Disconnected) => {
                if !daemon_lock_is_held(home).await? {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err(BridgeError::new(
                        Code::Timeout,
                        "gokod did not accept a shutdown connection",
                    ));
                }
                sleep(Duration::from_millis(25)).await;
            }
            Err(error) => return Err(error),
        }
    }
}

async fn wait_for_daemon_exit(home: &Path) -> Result<(), BridgeError> {
    let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
    while daemon_lock_is_held(home).await? {
        if Instant::now() >= deadline {
            return Err(BridgeError::new(
                Code::Timeout,
                "gokod did not exit after shutdown",
            ));
        }
        sleep(Duration::from_millis(25)).await;
    }
    Ok(())
}

async fn daemon_lock_is_held(home: &Path) -> Result<bool, BridgeError> {
    let lock = home.join("daemon.lock");
    tokio::task::spawn_blocking(move || {
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW);
        let file = match options.open(lock) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        match file.try_lock() {
            Ok(()) => Ok(false),
            Err(std::fs::TryLockError::WouldBlock) => Ok(true),
            Err(std::fs::TryLockError::Error(error)) => Err(error),
        }
    })
    .await
    .map_err(|error| {
        BridgeError::new(
            Code::DaemonUnavailable,
            format!("could not check daemon shutdown: {error}"),
        )
    })?
    .map_err(|error| {
        BridgeError::new(
            Code::DaemonUnavailable,
            format!("could not check daemon shutdown: {error}"),
        )
    })
}

fn disconnected() -> BridgeError {
    BridgeError::new(Code::Disconnected, "Goko is shutting down")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Home, TempDir};

    #[tokio::test]
    async fn shutdown_does_not_start_a_missing_daemon() {
        let temp = TempDir::new("shutdown-absent");
        let home = temp.path().join("home");

        Bridge::new(home.clone()).shutdown().await.unwrap();

        assert!(!home.exists());
    }

    #[tokio::test]
    async fn shutdown_stops_a_live_session_and_waits_for_the_daemon() {
        let home = Home::new("bridge-shutdown");
        home.start_daemon();
        let bridge = Bridge::new(home.path().to_owned());
        let pid_file = home.path().join("child.pid");
        let script = format!(
            "trap '' HUP; echo $$ > {}; exec sleep 30",
            pid_file.display()
        );

        bridge
            .call(|id| Request::SessionCreate {
                id,
                argv: vec!["sh".into(), "-c".into(), script],
                cwd: home.path().display().to_string(),
                cols: 80,
                rows: 24,
                title: "shutdown-test".into(),
            })
            .await
            .unwrap();

        let pid = tokio::time::timeout(SHUTDOWN_TIMEOUT, async {
            loop {
                if let Ok(pid) = std::fs::read_to_string(&pid_file) {
                    break pid.trim().parse::<libc::pid_t>().unwrap();
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("session child should start");
        // SAFETY: this pid was written by the session started above.
        assert_eq!(unsafe { libc::kill(pid, 0) }, 0);

        bridge.shutdown().await.unwrap();

        // SAFETY: this pid belongs to the session stopped by the daemon.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        assert!(!home.path().join("daemon.sock").exists());
    }
}
