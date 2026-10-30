//! The app's single daemon connection, shared by commands, the event pump, and terminals.

use crate::daemon_client::DaemonClient;
use crate::error::BridgeError;
use crate::terminal;
use protocol::message::Request;
use serde_json::Value;
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
    /// Started by the first terminal subscription.
    terminals: OnceLock<mpsc::UnboundedSender<terminal::Command>>,
}

impl Bridge {
    pub fn new(home: PathBuf) -> Self {
        Self(Arc::new(Inner {
            home,
            client: Mutex::new(None),
            connecting: tokio::sync::Mutex::new(()),
            terminals: OnceLock::new(),
        }))
    }

    pub fn home(&self) -> &Path {
        &self.0.home
    }

    pub(crate) fn terminals(&self) -> &OnceLock<mpsc::UnboundedSender<terminal::Command>> {
        &self.0.terminals
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
