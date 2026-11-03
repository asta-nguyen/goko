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
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::mpsc;

/// A cheap handle: clones share one connection.
#[derive(Clone)]
pub struct Bridge(Arc<Inner>);

struct Inner {
    home: PathBuf,
    client: Mutex<Option<Arc<DaemonClient>>>,
    /// Held while connecting so racing callers make exactly one attempt.
    connecting: tokio::sync::Mutex<()>,
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
        if let Some(client) = self.current() {
            return Ok(client);
        }
        let _one_attempt = self.0.connecting.lock().await;
        if let Some(client) = self.current() {
            return Ok(client);
        }
        let client = Arc::new(DaemonClient::connect_or_spawn(&self.0.home).await?);
        *self.0.client.lock().unwrap() = Some(Arc::clone(&client));
        Ok(client)
    }

    /// `DaemonClient::request` on the shared connection.
    pub async fn call(&self, build: impl FnOnce(u64) -> Request) -> Result<Value, BridgeError> {
        self.client().await?.request(build).await
    }
}
