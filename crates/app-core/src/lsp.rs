//! Language Server Protocol client for the native shells: one stdio child
//! per `(checkout, server)` speaking `Content-Length`-framed JSON-RPC 2.0,
//! plus the connection hub that owns them.
//!
//! `Connection` is the transport: spawn, `initialize` with typed client
//! capabilities, request/response routing, server→client request answers,
//! bounded stderr capture, and process-group cleanup. `LspHub` is the
//! connection table keyed by [`LspKey`]; it is constructed with the tokio
//! runtime `Handle` of its owner — the native editor supplies the single
//! dedicated thread, this crate spawns none of its own.
//!
//! Wire framing stays `serde_json::Value`; protocol message shapes use
//! `lsp-types` models so capability and sync fields cannot drift from the
//! official schema.

use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};

use lsp_types::{
    ClientCapabilities, ClientInfo, CompletionClientCapabilities, CompletionItemCapability,
    GeneralClientCapabilities, GotoCapability, HoverClientCapabilities, HoverProviderCapability,
    InitializeParams, MarkupKind, OneOf, PositionEncodingKind,
    PublishDiagnosticsClientCapabilities, ServerCapabilities, TextDocumentClientCapabilities,
    TextDocumentSyncCapability, TextDocumentSyncClientCapabilities, TextDocumentSyncKind,
    TextDocumentSyncSaveOptions, Uri, WorkspaceFolder,
};

use crate::error::{BridgeError, Code};
use protocol::message::ErrorCode;

/// Largest accepted `Content-Length` header block.
pub const MAX_HEADER_BYTES: usize = 16 * 1024;
/// Largest accepted message body. Must fit a 1 MiB document plus JSON
/// escaping on `didOpen`/`didChange`.
pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
/// Simultaneous live connections the hub keeps; the oldest idle one is
/// evicted for a new key beyond this.
pub const MAX_SERVERS: usize = 8;
/// Outstanding client→server requests per connection.
const MAX_PENDING: usize = 1024;
/// Server→client events buffered per connection before the reader applies
/// backpressure.
const CONN_EVENTS: usize = 1024;
/// Hub-level event queue for the editor actor.
const HUB_EVENTS: usize = 4096;
/// Bytes of server stderr retained for status/advice.
const MAX_STDERR_BYTES: usize = 8 * 1024;
const INIT_TIMEOUT: Duration = Duration::from_secs(60);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
/// How long `shutdown` waits for the server to exit on its own after `exit`.
const EXIT_GRACE: Duration = Duration::from_millis(500);
/// Backoff ladder indexed by consecutive-crash count (crash 1 → 1 s,
/// crash 2 → 2 s). The approved rule is "max 3 consecutive crashes →
/// Error until a 60 s cooldown", so the third crash never reaches the
/// 4 s rung — [`MAX_CONSECUTIVE_CRASHES`] preempts it; the rung stays so
/// the documented ladder is intact if the strike count ever changes.
const RETRY_BACKOFF: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];
/// Consecutive crashes tolerated before Error + cooldown (approved: 3).
const MAX_CONSECUTIVE_CRASHES: usize = 3;
const ERROR_COOLDOWN: Duration = Duration::from_secs(60);

// ---------- public types ----------

/// One configured language server: the spawn argv plus the file-extension →
/// languageId map the editor uses (independent of syntax highlighting).
#[derive(Clone, Debug)]
pub struct LspSpec {
    pub id: String,
    /// Full argv; `argv[0]` is the program.
    pub argv: Vec<String>,
    /// Extra environment for the child only.
    pub env: Vec<(String, String)>,
    pub disabled: bool,
    /// Extension without the dot (e.g. `rs`) → LSP `languageId`.
    pub languages: BTreeMap<String, String>,
}

impl LspSpec {
    /// Whether `argv[0]` resolves on `PATH` (or is an executable path).
    pub fn program_available(&self) -> bool {
        self.argv
            .first()
            .is_some_and(|program| program_available(program))
    }
}

/// The `(checkout, server)` pair a connection belongs to. `checkout` is the
/// editor's opaque checkout key; `server` is the [`LspSpec::id`].
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct LspKey {
    pub checkout: String,
    pub server: String,
}

impl LspKey {
    pub fn new(checkout: impl Into<String>, server: impl Into<String>) -> Self {
        Self {
            checkout: checkout.into(),
            server: server.into(),
        }
    }
}

/// Lifecycle state shown in the Language Servers section.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LspState {
    #[default]
    Off,
    Starting,
    Running,
    Error,
}

/// The server-advertised `change` kind after normalization.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncChange {
    #[default]
    None,
    Full,
    Incremental,
}

/// Whether the server wants `didSave`, and if so whether it carries `text`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SaveSupport {
    #[default]
    None,
    WithoutText,
    WithText,
}

/// The effective document-sync matrix for one connection, computed once
/// from the server's `textDocumentSync` capability (see the design's sync
/// table: absent/`None` → nothing, bare kind N>0 → change + openClose and no
/// save, options → literal fields with `openClose` absent meaning false).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct SyncSupport {
    pub open_close: bool,
    pub change: SyncChange,
    pub save: SaveSupport,
}

/// Per-feature availability parsed from `initialize`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct LspFeatures {
    pub completion: bool,
    pub hover: bool,
    pub definition: bool,
    pub sync: SyncSupport,
}

/// The serialized per-connection status.
#[derive(Clone, Debug, PartialEq)]
pub struct LspStatus {
    pub state: LspState,
    pub message: Option<String>,
    /// Connection generation; bumps on every (re)spawn so the editor can
    /// resync documents.
    pub epoch: u64,
    pub features: LspFeatures,
    /// Last bytes of server stderr, truncated for display.
    pub stderr_tail: Option<String>,
}

/// What a connection emits toward the editor actor.
#[derive(Clone, Debug)]
pub enum LspEvent {
    /// A server→client notification (`textDocument/publishDiagnostics`,
    /// `window/showMessage`, `exit`, …) tagged with its connection.
    Notification {
        key: LspKey,
        epoch: u64,
        method: String,
        params: Value,
    },
    /// The child exited or the stream broke; pending requests already
    /// failed. The hub schedules the bounded retry.
    Closed {
        key: LspKey,
        epoch: u64,
        detail: String,
    },
}

/// Errors the LSP layer reports. `kind` is coarse; `message` is displayable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LspErrorKind {
    /// The program could not be spawned or was not found.
    Spawn,
    /// A protocol/framing violation.
    Protocol,
    Timeout,
    /// The connection is gone (or never existed).
    Disconnected,
    /// The caller asked for something invalid.
    Invalid,
    /// A crash backoff/cooldown is still in effect.
    RetryLater,
    /// The operation was cancelled (e.g. a newer connect superseded it).
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct LspError {
    pub kind: LspErrorKind,
    pub message: String,
    pub hint: Option<String>,
}

impl LspError {
    fn new(kind: LspErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            hint: None,
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(LspErrorKind::Invalid, message)
    }

    fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl std::fmt::Display for LspError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for LspError {}

impl From<LspError> for BridgeError {
    fn from(error: LspError) -> Self {
        let code = match error.kind {
            LspErrorKind::Spawn => Code::DaemonStartFailed,
            LspErrorKind::Timeout => Code::Timeout,
            LspErrorKind::Disconnected | LspErrorKind::RetryLater => Code::Disconnected,
            LspErrorKind::Protocol | LspErrorKind::Invalid | LspErrorKind::Cancelled => {
                Code::Daemon(ErrorCode::Internal)
            }
        };
        let mut bridge = BridgeError::new(code, error.message);
        if let Some(hint) = error.hint {
            bridge = bridge.with_hint(hint);
        }
        bridge
    }
}

// ---------- URI and argv helpers ----------

/// Absolute `file://` URI for `path` (percent-encoded).
pub fn path_to_file_uri(path: &Path) -> Result<String, LspError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| {
                LspError::invalid(format!("cannot resolve the working directory: {error}"))
            })?
            .join(path)
    };
    url::Url::from_file_path(&absolute)
        .map(|url| url.to_string())
        .map_err(|_| {
            LspError::invalid(format!("cannot make a file URI for {}", absolute.display()))
        })
}

/// Decode a `file://` URI back to a path; non-file schemes are rejected.
pub fn file_uri_to_path(uri: &str) -> Result<PathBuf, LspError> {
    let url = url::Url::parse(uri)
        .map_err(|error| LspError::invalid(format!("invalid URI '{uri}': {error}")))?;
    if url.scheme() != "file" {
        return Err(LspError::invalid(format!("URI '{uri}' is not a file URI")));
    }
    url.to_file_path()
        .map_err(|_| LspError::invalid(format!("URI '{uri}' does not decode to a path")))
}

/// Split a UI-entered command line into argv with `shlex`; no shell
/// expansion ever happens.
pub fn parse_argv(command: &str) -> Result<Vec<String>, LspError> {
    let words = shlex::split(command)
        .ok_or_else(|| LspError::invalid("the command has an unterminated quote"))?;
    if words.is_empty() {
        return Err(LspError::invalid("the command is empty"));
    }
    Ok(words)
}

/// `argv[0]` resolves on `PATH` — or, containing `/`, is an executable file.
fn program_available(program: &str) -> bool {
    if program.contains('/') {
        let path = Path::new(program);
        return path.is_file()
            && path
                .metadata()
                .is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0);
    }
    crate::agents::resolve(program, std::env::var_os("PATH").as_deref()).is_some()
}

// ---------- transport ----------

/// What a connection pushes to its owner: notifications plus a terminal
/// `Closed` marker.
#[derive(Debug)]
pub enum ConnEvent {
    Notification { method: String, params: Value },
    Closed { detail: String },
}

/// One language-server child process. Cheap to clone (shared `Arc`).
#[derive(Clone)]
pub struct Connection {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("alive", &self.is_alive())
            .finish()
    }
}

impl Connection {
    /// Spawn `spec.argv` with `root` as cwd and complete `initialize` within
    /// [`INIT_TIMEOUT`]. `events` receives server→client notifications and
    /// the terminal `Closed`. Any failure kills the child.
    pub async fn connect(
        spec: &LspSpec,
        root: &Path,
        events: mpsc::Sender<ConnEvent>,
    ) -> Result<Connection, LspError> {
        Self::connect_with_timeout(spec, root, events, INIT_TIMEOUT).await
    }

    async fn connect_with_timeout(
        spec: &LspSpec,
        root: &Path,
        events: mpsc::Sender<ConnEvent>,
        init_timeout: Duration,
    ) -> Result<Connection, LspError> {
        let program = spec
            .argv
            .first()
            .ok_or_else(|| LspError::invalid(format!("{} has an empty command", spec.id)))?;
        let mut command = Command::new(program);
        command
            .args(&spec.argv[1..])
            .envs(
                spec.env
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_str())),
            )
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        let mut child = command.spawn().map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                LspError::new(LspErrorKind::Spawn, format!("{program} was not found"))
                    .with_hint("Install it or fix the command in Settings → Language Servers.")
            } else {
                LspError::new(
                    LspErrorKind::Spawn,
                    format!("cannot start {program}: {error}"),
                )
            }
        })?;
        let stdin = child.stdin.take().expect("piped server stdin");
        let stdout = child.stdout.take().expect("piped server stdout");
        let stderr = child.stderr.take().expect("piped server stderr");

        let (writes, mut incoming) = mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(framed) = incoming.recv().await {
                if stdin.write_all(framed.as_bytes()).await.is_err() || stdin.flush().await.is_err()
                {
                    break;
                }
            }
        });

        let inner = Arc::new(Inner::new(writes, child));
        tokio::spawn(read_stderr(Arc::downgrade(&inner), stderr));
        tokio::spawn(read_loop(Arc::downgrade(&inner), stdout, events.clone()));
        let connection = Connection {
            inner: Arc::clone(&inner),
        };

        let params = initialize_params(root)?;
        let initialized =
            tokio::time::timeout(init_timeout, connection.request("initialize", params)).await;
        match initialized {
            Ok(Ok(result)) => {
                let parsed: InitializeResult = serde_json::from_value(result).map_err(|error| {
                    LspError::new(
                        LspErrorKind::Protocol,
                        format!("the server returned a malformed initialize result: {error}"),
                    )
                })?;
                *inner.capabilities.lock().unwrap() = Some(parsed.capabilities);
                connection.notify("initialized", Value::Null);
                Ok(connection)
            }
            Ok(Err(error)) => {
                connection.terminate().await;
                Err(error)
            }
            Err(_) => {
                connection.terminate().await;
                Err(LspError::new(
                    LspErrorKind::Timeout,
                    "the server did not respond to initialize in time",
                ))
            }
        }
    }

    /// Send one request and resolve with its `result` (or the server's
    /// JSON-RPC error). Callers bound the wait with `tokio::time::timeout`;
    /// dropping this future **removes the pending transport entry and sends
    /// `$/cancelRequest`** — cancellation is real, not just a dropped UI
    /// slot (see [`RequestGuard`]).
    pub async fn request(&self, method: &str, params: impl Serialize) -> Result<Value, LspError> {
        let params = serde_json::to_value(params).map_err(|error| {
            LspError::invalid(format!("cannot encode {method} params: {error}"))
        })?;
        let (id, answer) = self.inner.request(method, params)?;
        let mut guard = RequestGuard {
            inner: self.inner.clone(),
            id,
            done: false,
        };
        let message = answer.await.map_err(|_| self.inner.exited())?;
        guard.done = true;
        response_result(message)
    }

    /// Queue a notification (no id, no response).
    pub fn notify(&self, method: &str, params: impl Serialize) {
        let Ok(params) = serde_json::to_value(params) else {
            return;
        };
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        let _ = self.inner.writes.send(frame(&message));
    }

    /// The server's `initialize` capabilities, once connected.
    pub fn capabilities(&self) -> Option<ServerCapabilities> {
        self.inner.capabilities.lock().unwrap().clone()
    }

    /// Feature availability parsed from those capabilities.
    pub fn features(&self) -> LspFeatures {
        self.capabilities()
            .as_ref()
            .map(features_from)
            .unwrap_or_default()
    }

    /// True while the child is expected to answer: the reader marks the
    /// connection dead on EOF; the child check covers the gap before it.
    pub fn is_alive(&self) -> bool {
        if self.inner.routes.lock().unwrap().is_none() {
            return false;
        }
        match self.inner.child.try_lock() {
            Ok(mut child) => child
                .try_wait()
                .map(|status| status.is_none())
                .unwrap_or(true),
            Err(_) => false,
        }
    }

    /// The bounded tail of the server's stderr.
    pub fn stderr_tail(&self) -> Option<String> {
        let tail = self.inner.stderr_tail.lock().unwrap().clone();
        (!tail.is_empty()).then_some(tail)
    }

    /// Graceful `shutdown` → `exit` → brief grace → kill the process group
    /// and reap. Safe to call more than once and while already dead.
    pub async fn shutdown(&self) {
        if self.is_alive() {
            let _ =
                tokio::time::timeout(SHUTDOWN_TIMEOUT, self.request("shutdown", Value::Null)).await;
            self.notify("exit", Value::Null);
            // Give the server a moment to act on `exit` and flush before
            // the group kill lands. A plain sleep: touching `child.wait()`
            // here would race the reap in `terminate`.
            tokio::time::sleep(EXIT_GRACE).await;
        }
        self.terminate().await;
    }

    /// Immediate group kill + reap, no graceful requests. Used for failed
    /// connects and as `shutdown`'s final step.
    async fn terminate(&self) {
        self.inner.mark_dead();
        let mut child = self.inner.child.lock().await;
        kill_tree(&mut child);
        if let Ok(status) = child.wait().await {
            self.inner.record_exit(status);
        }
    }
}

/// The typed `initialize` result (capabilities + optional serverInfo).
#[derive(Debug, serde::Deserialize)]
struct InitializeResult {
    capabilities: ServerCapabilities,
}

/// Client capabilities per LSP 3.17: `didSave` is a boolean (`includeText`
/// exists only in the server's `SaveOptions`), snippets and completion
/// resolve are off, hover is plaintext, definition has no link support,
/// diagnostics versions are understood, and UTF-16 is the position encoding.
fn client_capabilities() -> ClientCapabilities {
    ClientCapabilities {
        text_document: Some(TextDocumentClientCapabilities {
            synchronization: Some(TextDocumentSyncClientCapabilities {
                dynamic_registration: Some(false),
                will_save: None,
                will_save_wait_until: None,
                did_save: Some(true),
            }),
            completion: Some(CompletionClientCapabilities {
                dynamic_registration: Some(false),
                completion_item: Some(CompletionItemCapability {
                    snippet_support: Some(false),
                    insert_replace_support: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            hover: Some(HoverClientCapabilities {
                dynamic_registration: Some(false),
                content_format: Some(vec![MarkupKind::PlainText]),
            }),
            definition: Some(GotoCapability {
                dynamic_registration: Some(false),
                link_support: Some(false),
            }),
            publish_diagnostics: Some(PublishDiagnosticsClientCapabilities {
                version_support: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        }),
        general: Some(GeneralClientCapabilities {
            position_encodings: Some(vec![PositionEncodingKind::UTF16]),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn initialize_params(root: &Path) -> Result<Value, LspError> {
    let uri = path_to_file_uri(root)?;
    let name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string());
    let params = InitializeParams {
        process_id: Some(std::process::id()),
        capabilities: client_capabilities(),
        workspace_folders: Some(vec![WorkspaceFolder {
            uri: Uri::from_str(&uri).map_err(|error| {
                LspError::invalid(format!("cannot encode workspace URI '{uri}': {error}"))
            })?,
            name,
        }]),
        client_info: Some(ClientInfo {
            name: "goko".to_owned(),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        }),
        ..Default::default()
    };
    serde_json::to_value(params)
        .map_err(|error| LspError::invalid(format!("cannot encode initialize params: {error}")))
}

struct Inner {
    /// Pending responses by request id; `None` once the connection is dead.
    routes: Mutex<Option<Routes>>,
    /// Serialized framed messages for the child's stdin.
    writes: mpsc::UnboundedSender<String>,
    child: tokio::sync::Mutex<Child>,
    /// Bounded tail of stderr for status/advice.
    stderr_tail: Mutex<String>,
    capabilities: Mutex<Option<ServerCapabilities>>,
    exit: Mutex<Option<ExitStatus>>,
    next_id: AtomicU64,
}

#[derive(Default)]
struct Routes {
    pending: HashMap<u64, oneshot::Sender<Value>>,
}

impl Inner {
    fn new(writes: mpsc::UnboundedSender<String>, child: Child) -> Self {
        Self {
            routes: Mutex::new(Some(Routes::default())),
            writes,
            child: tokio::sync::Mutex::new(child),
            stderr_tail: Mutex::new(String::new()),
            capabilities: Mutex::new(None),
            exit: Mutex::new(None),
            next_id: AtomicU64::new(1),
        }
    }

    /// Register `id`'s waiter and queue the framed request. The receiver
    /// resolves with the whole response message, or fails once the server
    /// is gone. Returns the wire id so the caller's [`RequestGuard`] can
    /// cancel the pending entry if the request future is dropped.
    fn request(
        &self,
        method: &str,
        params: Value,
    ) -> Result<(u64, oneshot::Receiver<Value>), LspError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply, answer) = oneshot::channel();
        {
            let mut shared = self.routes.lock().unwrap();
            match shared.as_mut() {
                Some(routes) => {
                    if routes.pending.len() >= MAX_PENDING {
                        return Err(LspError::new(
                            LspErrorKind::Protocol,
                            "too many pending requests",
                        ));
                    }
                    routes.pending.insert(id, reply);
                }
                None => return Err(self.exited()),
            }
        }
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        if self.writes.send(frame(&message)).is_err() {
            if let Some(routes) = self.routes.lock().unwrap().as_mut() {
                routes.pending.remove(&id);
            }
            return Err(self.exited());
        }
        Ok((id, answer))
    }

    /// Connection over: dropping the routes fails pending requests and
    /// closes every waiter, which is what wakes `request` callers.
    fn mark_dead(&self) {
        self.routes.lock().unwrap().take();
    }

    fn record_exit(&self, status: ExitStatus) {
        *self.exit.lock().unwrap() = Some(status);
    }

    fn exited(&self) -> LspError {
        let detail = self
            .exit
            .lock()
            .unwrap()
            .as_ref()
            .map(exit_detail)
            .unwrap_or_default();
        LspError::new(
            LspErrorKind::Disconnected,
            format!("the language server exited{detail}"),
        )
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        // `&mut self` rules out a held child guard; `kill_on_drop` still
        // covers the child itself as the field drops.
        kill_tree(self.child.get_mut());
    }
}

/// Cancels a pending request when its `Connection::request` future is
/// dropped early (timeout, supersede, shutdown): removes the transport's
/// pending entry — freeing the bounded slot — and sends `$/cancelRequest`
/// so a well-behaved server stops work on it. Once the response has been
/// routed the pending entry is already gone, so the guard is a no-op.
struct RequestGuard {
    inner: Arc<Inner>,
    id: u64,
    done: bool,
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let still_pending = {
            let mut shared = self.inner.routes.lock().unwrap();
            shared
                .as_mut()
                .is_some_and(|routes| routes.pending.remove(&self.id).is_some())
        };
        if still_pending {
            let message = json!({
                "jsonrpc": "2.0",
                "method": "$/cancelRequest",
                "params": { "id": self.id },
            });
            let _ = self.inner.writes.send(frame(&message));
        }
    }
}

/// `Content-Length: <bytes>\r\n\r\n<json>` for one message.
fn frame(message: &Value) -> String {
    let body = message.to_string();
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body)
}

/// Kill the server's whole process group, not just the child: the child
/// leads its own group (`process_group(0)` at spawn), so wrappers like
/// `npx` cannot leave grandchildren behind.
fn kill_tree(child: &mut Child) {
    if let Some(pid) = child.id() {
        // SAFETY: killpg(2) on the group the child leads; ESRCH just means
        // the whole group is already gone.
        unsafe { libc::killpg(pid as libc::pid_t, libc::SIGKILL) };
    }
    let _ = child.start_kill();
}

fn exit_detail(status: &ExitStatus) -> String {
    match status.code() {
        Some(code) => format!(" with status {code}"),
        None => " by signal".to_owned(),
    }
}

// ---------- framing ----------

enum FrameError {
    /// Clean or unclean end of stream.
    Eof,
    /// Header/body bound exceeded or malformed framing.
    Protocol(String),
    Io,
}

/// Read one `Content-Length`-framed message. `Ok(None)` skips a body that is
/// not valid JSON (the stream stays framed); `Err` ends the connection.
async fn read_message(reader: &mut BufReader<ChildStdout>) -> Result<Option<Value>, FrameError> {
    let mut header_bytes = 0usize;
    let mut header_lines = 0usize;
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let read = reader
            .read_line(&mut line)
            .await
            .map_err(|_| FrameError::Io)?;
        if read == 0 {
            return Err(FrameError::Eof);
        }
        header_bytes += read;
        header_lines += 1;
        if header_bytes > MAX_HEADER_BYTES || header_lines > 64 {
            return Err(FrameError::Protocol(
                "the message headers exceed the limit".to_owned(),
            ));
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        let Some((name, value)) = trimmed.split_once(':') else {
            return Err(FrameError::Protocol(format!(
                "malformed header line '{trimmed}'"
            )));
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(FrameError::Protocol(
                    "duplicate Content-Length header".to_owned(),
                ));
            }
            let length: usize = value
                .trim()
                .parse()
                .map_err(|_| FrameError::Protocol("invalid Content-Length".to_owned()))?;
            if length == 0 || length > MAX_BODY_BYTES {
                return Err(FrameError::Protocol(format!(
                    "message body of {length} bytes exceeds the limit"
                )));
            }
            content_length = Some(length);
        }
    }
    let Some(length) = content_length else {
        return Err(FrameError::Protocol(
            "missing Content-Length header".to_owned(),
        ));
    };
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            FrameError::Eof
        } else {
            FrameError::Io
        }
    })?;
    match serde_json::from_slice::<Value>(&body) {
        Ok(message) => Ok(Some(message)),
        Err(_) => Ok(None),
    }
}

async fn read_loop(weak: Weak<Inner>, stdout: ChildStdout, events: mpsc::Sender<ConnEvent>) {
    let mut reader = BufReader::new(stdout);
    let mut detail = String::new();
    loop {
        match read_message(&mut reader).await {
            Ok(Some(message)) => {
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                route(&inner, &message, &events).await;
            }
            Ok(None) => continue,
            Err(FrameError::Eof) => break,
            Err(FrameError::Protocol(reason)) => {
                detail = reason;
                break;
            }
            Err(FrameError::Io) => break,
        }
    }
    let Some(inner) = weak.upgrade() else {
        return;
    };
    inner.mark_dead();
    // Reap the child for its exit status; killing its group also covers a
    // server that closed stdout but kept running.
    let mut child = inner.child.lock().await;
    kill_tree(&mut child);
    if let Ok(status) = child.wait().await {
        inner.record_exit(status);
    }
    drop(child);
    let detail = if detail.is_empty() {
        inner
            .exit
            .lock()
            .unwrap()
            .as_ref()
            .map(exit_detail)
            .unwrap_or_else(|| "the stream ended".to_owned())
    } else {
        detail
    };
    let _ = events
        .send(ConnEvent::Closed {
            detail: format!("the language server closed: {detail}"),
        })
        .await;
}

/// Bounded stderr capture: keeps only the last [`MAX_STDERR_BYTES`].
async fn read_stderr(weak: Weak<Inner>, mut stderr: ChildStderr) {
    let mut buffer = [0u8; 4096];
    loop {
        let Ok(read) = stderr.read(&mut buffer).await else {
            return;
        };
        if read == 0 {
            return;
        }
        let Some(inner) = weak.upgrade() else {
            return;
        };
        let mut tail = inner.stderr_tail.lock().unwrap();
        tail.push_str(&String::from_utf8_lossy(&buffer[..read]));
        if tail.len() > MAX_STDERR_BYTES {
            let excess = tail.len() - MAX_STDERR_BYTES;
            tail.replace_range(..excess, "");
        }
    }
}

async fn route(inner: &Inner, message: &Value, events: &mpsc::Sender<ConnEvent>) {
    let method = message.get("method").and_then(Value::as_str);
    let id = message.get("id").filter(|id| !id.is_null());
    match (id, method) {
        // A server→client request carries both `id` and `method`.
        (Some(id), Some(method)) => answer_server_request(inner, id, method, message),
        // A notification carries `method` and no `id`.
        (None, Some(method)) => {
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            let _ = events
                .send(ConnEvent::Notification {
                    method: method.to_owned(),
                    params,
                })
                .await;
            // `exit` means the server is going away; fail fast rather than
            // waiting for EOF.
            if method == "exit" {
                inner.mark_dead();
            }
        }
        // A response carries `id` and no `method`.
        (Some(id), None) => {
            if let Some(id) = id.as_u64() {
                let mut shared = inner.routes.lock().unwrap();
                if let Some(routes) = shared.as_mut() {
                    if let Some(waiting) = routes.pending.remove(&id) {
                        let _ = waiting.send(message.clone());
                    }
                }
            }
        }
        _ => {}
    }
}

/// The fixed server→client responder table: configuration and progress
/// requests get benign answers; dynamic-registration requests are accepted
/// as no-ops; everything else — including `workspace/applyEdit` and
/// `workspace/executeCommand` — is `-32601` and never executed.
fn answer_server_request(inner: &Inner, id: &Value, method: &str, message: &Value) {
    let reply = match method {
        "workspace/configuration" => {
            let count = message
                .pointer("/params/items")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": vec![Value::Null; count],
            })
        }
        "client/registerCapability"
        | "client/unregisterCapability"
        | "workspace/workDoneProgress/create" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": Value::Null,
        }),
        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": "method not found" },
        }),
    };
    let _ = inner.writes.send(frame(&reply));
}

/// The `result` of a response, or the server's JSON-RPC error.
fn response_result(message: Value) -> Result<Value, LspError> {
    match message.get("error") {
        Some(error) => Err(server_error(error)),
        None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
    }
}

fn server_error(error: &Value) -> LspError {
    let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("the server returned an error");
    LspError::new(
        LspErrorKind::Protocol,
        format!("the server returned error {code}: {message}"),
    )
}

// ---------- capability summary ----------

fn sync_change(kind: TextDocumentSyncKind) -> SyncChange {
    if kind == TextDocumentSyncKind::FULL {
        SyncChange::Full
    } else if kind == TextDocumentSyncKind::INCREMENTAL {
        SyncChange::Incremental
    } else {
        SyncChange::None
    }
}

/// Normalize `textDocumentSync` into the effective matrix the editor acts
/// on. Absent or kind `None` → no document messages; a bare kind N>0 is the
/// legacy form (change kind + openClose implied, no save); the options form
/// is literal with `openClose` absent meaning false, and `save` in any
/// declared form (`true`, `{}`, `{includeText:false}`) sends didSave
/// without text — only `{includeText:true}` includes it.
fn sync_support(capability: &Option<TextDocumentSyncCapability>) -> SyncSupport {
    let Some(capability) = capability else {
        return SyncSupport::default();
    };
    match capability {
        TextDocumentSyncCapability::Kind(kind) => {
            if *kind == TextDocumentSyncKind::NONE {
                SyncSupport::default()
            } else {
                SyncSupport {
                    open_close: true,
                    change: sync_change(*kind),
                    save: SaveSupport::None,
                }
            }
        }
        TextDocumentSyncCapability::Options(options) => SyncSupport {
            open_close: options.open_close.unwrap_or(false),
            change: options.change.map(sync_change).unwrap_or_default(),
            save: match &options.save {
                None | Some(TextDocumentSyncSaveOptions::Supported(false)) => SaveSupport::None,
                Some(TextDocumentSyncSaveOptions::Supported(true)) => SaveSupport::WithoutText,
                Some(TextDocumentSyncSaveOptions::SaveOptions(save)) => {
                    if save.include_text == Some(true) {
                        SaveSupport::WithText
                    } else {
                        SaveSupport::WithoutText
                    }
                }
            },
        },
    }
}

fn features_from(capabilities: &ServerCapabilities) -> LspFeatures {
    LspFeatures {
        completion: capabilities.completion_provider.is_some(),
        hover: capabilities
            .hover_provider
            .as_ref()
            .is_some_and(|provider| {
                matches!(provider, HoverProviderCapability::Simple(true))
                    || matches!(provider, HoverProviderCapability::Options(_))
            }),
        definition: capabilities.definition_provider.as_ref().is_some_and(
            |provider| match provider {
                OneOf::Left(enabled) => *enabled,
                OneOf::Right(_) => true,
            },
        ),
        sync: sync_support(&capabilities.text_document_sync),
    }
}

// ---------- the hub ----------

#[derive(Default)]
struct ConnSlot {
    connection: Option<Connection>,
    /// Bumped on every connect attempt; events and replies are tagged with
    /// it so stale generations cannot touch live state.
    epoch: u64,
    /// Spawn order for oldest-idle eviction.
    order: u64,
    state: LspState,
    message: Option<String>,
    /// Documents the editor reports open on this connection.
    open_docs: usize,
    /// Consecutive crashes/failures; reset by a successful connect.
    crashes: u32,
    /// Earliest next spawn attempt.
    retry_at: Option<Instant>,
    /// The epoch a crash was already counted for (idempotent accounting).
    crashed_epoch: u64,
    connecting: bool,
    cancel: Option<oneshot::Sender<()>>,
}

struct HubInner {
    conns: HashMap<LspKey, ConnSlot>,
    events_rx: Option<mpsc::Receiver<LspEvent>>,
    next_order: u64,
    /// Crash backoff ladder and post-limit cooldown; tests shorten them.
    backoff: Vec<Duration>,
    cooldown: Duration,
}

/// The connection table. Clone shares state; all work happens on the
/// runtime of the [`tokio::runtime::Handle`] it was built with.
#[derive(Clone)]
pub struct LspHub {
    inner: Arc<Mutex<HubInner>>,
    events: mpsc::Sender<LspEvent>,
    handle: tokio::runtime::Handle,
}

impl LspHub {
    /// Build the hub on `handle`'s runtime. Takes ownership of the event
    /// queue; call [`LspHub::take_events`] once for the receiver.
    pub fn new(handle: tokio::runtime::Handle) -> Self {
        let (events, events_rx) = mpsc::channel(HUB_EVENTS);
        Self {
            inner: Arc::new(Mutex::new(HubInner {
                conns: HashMap::new(),
                events_rx: Some(events_rx),
                next_order: 0,
                backoff: RETRY_BACKOFF.to_vec(),
                cooldown: ERROR_COOLDOWN,
            })),
            events,
            handle,
        }
    }

    /// The single consumer endpoint for [`LspEvent`]s.
    pub fn take_events(&self) -> Option<mpsc::Receiver<LspEvent>> {
        self.inner.lock().unwrap().events_rx.take()
    }

    /// Ensure a live connection for `key`: spawn + `initialize` lazily on
    /// first need, respawn after a crash once the backoff/cooldown allows.
    /// Idempotent for a live or connecting connection.
    pub async fn ensure(&self, key: &LspKey, spec: &LspSpec, root: &Path) -> Result<(), LspError> {
        if spec.disabled {
            return Err(LspError::invalid(format!("{} is disabled", spec.id)));
        }
        if spec.argv.is_empty() {
            return Err(LspError::invalid(format!(
                "{} has an empty command",
                spec.id
            )));
        }
        let program = spec.argv[0].clone();

        // Evict the oldest idle connection when at the cap and this key has
        // no live connection of its own.
        let evict = {
            let inner = self.inner.lock().unwrap();
            let active = inner
                .conns
                .values()
                .filter(|slot| {
                    slot.connecting || slot.connection.as_ref().is_some_and(|conn| conn.is_alive())
                })
                .count();
            let self_live = inner.conns.get(key).is_some_and(|slot| {
                slot.connecting || slot.connection.as_ref().is_some_and(|conn| conn.is_alive())
            });
            if active >= MAX_SERVERS && !self_live {
                inner
                    .conns
                    .iter()
                    .filter(|(other, slot)| {
                        *other != key
                            && slot.open_docs == 0
                            && !slot.connecting
                            && slot.connection.as_ref().is_some_and(|conn| conn.is_alive())
                    })
                    .min_by_key(|(_, slot)| slot.order)
                    .map(|(key, _)| key.clone())
            } else {
                None
            }
        };
        if let Some(victim) = evict {
            self.disconnect(&victim).await;
        }

        let (cancel_rx, epoch) = {
            let mut inner = self.inner.lock().unwrap();
            if !inner.conns.contains_key(key) {
                let order = inner.next_order;
                inner.next_order += 1;
                inner.conns.insert(
                    key.clone(),
                    ConnSlot {
                        order,
                        ..Default::default()
                    },
                );
            }
            let backoff = inner.backoff.clone();
            let cooldown = inner.cooldown;
            let slot = inner.conns.get_mut(key).expect("slot just inserted");
            if slot.connecting {
                return Ok(());
            }
            if let Some(connection) = &slot.connection {
                if connection.is_alive() {
                    // Live connection: running, or a stale Starting flag —
                    // either way it answers, so report it as running.
                    slot.state = LspState::Running;
                    return Ok(());
                }
                // Dead but the Closed event has not been processed yet:
                // count the crash here (idempotent with the forwarder).
                note_crash(
                    &backoff,
                    cooldown,
                    slot,
                    "the language server exited".to_owned(),
                );
            }
            if let Some(retry_at) = slot.retry_at {
                if Instant::now() < retry_at {
                    return Err(LspError::new(
                        LspErrorKind::RetryLater,
                        format!(
                            "the language server is restarting; retry in {}s",
                            retry_at.saturating_duration_since(Instant::now()).as_secs() + 1
                        ),
                    ));
                }
            }
            if !program_available(&program) {
                slot.state = LspState::Error;
                slot.message = Some(format!("{program} was not found on PATH"));
                slot.retry_at = None;
                return Err(LspError::new(
                    LspErrorKind::Spawn,
                    format!("{program} was not found on PATH"),
                )
                .with_hint("Install it or fix the command in Settings → Language Servers."));
            }
            slot.epoch += 1;
            let epoch = slot.epoch;
            slot.state = LspState::Starting;
            slot.message = None;
            slot.connecting = true;
            let (cancel, cancel_rx) = oneshot::channel();
            slot.cancel = Some(cancel);
            (cancel_rx, epoch)
        };

        let (events_tx, events_rx) = mpsc::channel(CONN_EVENTS);
        let spec_owned = spec.clone();
        let root_owned = root.to_path_buf();
        let mut cancel_rx = cancel_rx;
        let outcome = tokio::select! {
            outcome = Connection::connect(&spec_owned, &root_owned, events_tx) => Some(outcome),
            _ = &mut cancel_rx => None,
        };

        match outcome {
            None => {
                let mut inner = self.inner.lock().unwrap();
                if let Some(slot) = inner.conns.get_mut(key) {
                    if slot.epoch == epoch {
                        slot.connecting = false;
                        slot.cancel = None;
                    }
                }
                Err(LspError::new(LspErrorKind::Cancelled, "connect cancelled"))
            }
            Some(Err(error)) => {
                let mut inner = self.inner.lock().unwrap();
                let backoff = inner.backoff.clone();
                let cooldown = inner.cooldown;
                if let Some(slot) = inner.conns.get_mut(key) {
                    if slot.epoch == epoch {
                        slot.connecting = false;
                        slot.cancel = None;
                        if error.kind == LspErrorKind::Spawn {
                            // Missing/unspawnable binaries are status, not a
                            // crash: Error with advice, no auto-retry.
                            slot.state = LspState::Error;
                            slot.message = Some(error.message.clone());
                            slot.retry_at = None;
                        } else {
                            note_crash(&backoff, cooldown, slot, error.message.clone());
                        }
                    }
                }
                Err(error)
            }
            Some(Ok(connection)) => {
                // Store first, then forward events: a Closed that lands in
                // the gap is still accounted because the epoch already
                // matches. The slot guard is scoped so no await can hold it.
                enum Store {
                    Proceed,
                    Cancelled,
                    Dead,
                }
                let store = {
                    let mut inner = self.inner.lock().unwrap();
                    let proceed = inner.conns.get(key).is_some_and(|slot| slot.epoch == epoch);
                    if !proceed {
                        Store::Cancelled
                    } else if !connection.is_alive() {
                        // The server exited between initialize and the store.
                        let backoff = inner.backoff.clone();
                        let cooldown = inner.cooldown;
                        let slot = inner.conns.get_mut(key).expect("slot checked above");
                        slot.connecting = false;
                        slot.cancel = None;
                        note_crash(
                            &backoff,
                            cooldown,
                            slot,
                            "the language server exited during startup".to_owned(),
                        );
                        Store::Dead
                    } else {
                        let slot = inner.conns.get_mut(key).expect("slot checked above");
                        slot.connecting = false;
                        slot.cancel = None;
                        slot.connection = Some(connection.clone());
                        slot.state = LspState::Running;
                        slot.message = None;
                        slot.crashes = 0;
                        slot.retry_at = None;
                        Store::Proceed
                    }
                };
                match store {
                    Store::Proceed => {
                        self.spawn_forwarder(key.clone(), epoch, events_rx);
                        Ok(())
                    }
                    Store::Cancelled => {
                        connection.shutdown().await;
                        Err(LspError::new(LspErrorKind::Cancelled, "connect cancelled"))
                    }
                    Store::Dead => {
                        connection.shutdown().await;
                        Err(LspError::new(
                            LspErrorKind::Disconnected,
                            "the language server exited during startup",
                        ))
                    }
                }
            }
        }
    }

    /// Snapshot every known connection's status; a `running` slot whose
    /// process died flips to `error` here so pollers see it without waiting
    /// for the Closed event.
    pub fn status(&self) -> BTreeMap<LspKey, LspStatus> {
        let mut inner = self.inner.lock().unwrap();
        for slot in inner.conns.values_mut() {
            if slot.state == LspState::Running
                && slot
                    .connection
                    .as_ref()
                    .is_some_and(|conn| !conn.is_alive())
            {
                slot.state = LspState::Error;
                slot.message = Some("the language server exited".to_owned());
                slot.connection = None;
            }
        }
        inner
            .conns
            .iter()
            .map(|(key, slot)| {
                let (features, stderr_tail) = slot
                    .connection
                    .as_ref()
                    .filter(|conn| conn.is_alive())
                    .map(|conn| {
                        (
                            conn.features(),
                            conn.stderr_tail()
                                .map(|tail| tail.chars().take(512).collect::<String>()),
                        )
                    })
                    .unwrap_or_default();
                (
                    key.clone(),
                    LspStatus {
                        state: slot.state,
                        message: slot.message.clone(),
                        epoch: slot.epoch,
                        features,
                        stderr_tail,
                    },
                )
            })
            .collect()
    }

    /// One client→server request on a live connection.
    pub async fn request(
        &self,
        key: &LspKey,
        method: &str,
        params: impl Serialize,
    ) -> Result<Value, LspError> {
        let connection = {
            let mut inner = self.inner.lock().unwrap();
            let backoff = inner.backoff.clone();
            let cooldown = inner.cooldown;
            let Some(slot) = inner.conns.get_mut(key) else {
                return Err(LspError::new(
                    LspErrorKind::Disconnected,
                    format!("no language server for {}", key.server),
                ));
            };
            match &slot.connection {
                Some(conn) if conn.is_alive() => conn.clone(),
                _ => {
                    if slot.connection.is_some() {
                        note_crash(
                            &backoff,
                            cooldown,
                            slot,
                            "the language server exited".to_owned(),
                        );
                    }
                    return Err(LspError::new(
                        LspErrorKind::Disconnected,
                        format!("the {} language server is not running", key.server),
                    ));
                }
            }
        };
        connection.request(method, params).await
    }

    /// Queue a client→server notification; silently dropped when the
    /// connection is not live.
    pub fn notify(&self, key: &LspKey, method: &str, params: impl Serialize) {
        let connection = {
            let inner = self.inner.lock().unwrap();
            inner
                .conns
                .get(key)
                .and_then(|slot| slot.connection.clone())
                .filter(|conn| conn.is_alive())
        };
        if let Some(connection) = connection {
            connection.notify(method, params);
        }
    }

    /// The editor reports a document opened on this connection (drives
    /// oldest-idle eviction).
    pub fn doc_opened(&self, key: &LspKey) {
        if let Some(slot) = self.inner.lock().unwrap().conns.get_mut(key) {
            slot.open_docs += 1;
        }
    }

    /// The editor reports a document closed on this connection.
    pub fn doc_closed(&self, key: &LspKey) {
        if let Some(slot) = self.inner.lock().unwrap().conns.get_mut(key) {
            slot.open_docs = slot.open_docs.saturating_sub(1);
        }
    }

    /// Shut one connection down and mark the slot `off`; the next `ensure`
    /// spawns a fresh epoch.
    pub async fn disconnect(&self, key: &LspKey) {
        let (connection, cancel) = {
            let mut inner = self.inner.lock().unwrap();
            let Some(slot) = inner.conns.get_mut(key) else {
                return;
            };
            slot.epoch += 1;
            slot.state = LspState::Off;
            slot.message = None;
            slot.connecting = false;
            (slot.connection.take(), slot.cancel.take())
        };
        if let Some(cancel) = cancel {
            let _ = cancel.send(());
        }
        if let Some(connection) = connection {
            connection.shutdown().await;
        }
    }

    /// Shut every connection down (app quit).
    pub async fn shutdown(&self) {
        let keys: Vec<LspKey> = self.inner.lock().unwrap().conns.keys().cloned().collect();
        for key in keys {
            self.disconnect(&key).await;
        }
    }

    /// Forward one connection's events to the hub queue, tagging them with
    /// key and epoch and applying crash accounting on `Closed`.
    fn spawn_forwarder(&self, key: LspKey, epoch: u64, mut events_rx: mpsc::Receiver<ConnEvent>) {
        let hub = Arc::clone(&self.inner);
        let events = self.events.clone();
        self.handle.spawn(async move {
            while let Some(event) = events_rx.recv().await {
                match event {
                    ConnEvent::Notification { method, params } => {
                        let _ = events
                            .send(LspEvent::Notification {
                                key: key.clone(),
                                epoch,
                                method,
                                params,
                            })
                            .await;
                    }
                    ConnEvent::Closed { detail } => {
                        {
                            let mut inner = hub.lock().unwrap();
                            let backoff = inner.backoff.clone();
                            let cooldown = inner.cooldown;
                            if let Some(slot) = inner.conns.get_mut(&key) {
                                if slot.epoch == epoch {
                                    note_crash(&backoff, cooldown, slot, detail.clone());
                                }
                            }
                        }
                        let _ = events
                            .send(LspEvent::Closed {
                                key: key.clone(),
                                epoch,
                                detail,
                            })
                            .await;
                        break;
                    }
                }
            }
        });
    }
}

/// Count one crash for `slot`'s current epoch (idempotent) and schedule the
/// bounded retry: 1 s/2 s/4 s backoff, then a 60 s cooldown after three
/// consecutive crashes. Spawn failures never reach here. Takes the timing
/// knobs separately so callers can hold a `conns` borrow.
fn note_crash(backoff: &[Duration], cooldown: Duration, slot: &mut ConnSlot, detail: String) {
    slot.connection = None;
    if slot.crashed_epoch == slot.epoch {
        slot.state = LspState::Error;
        slot.message = Some(detail);
        return;
    }
    slot.crashed_epoch = slot.epoch;
    slot.crashes += 1;
    slot.state = LspState::Error;
    slot.message = Some(detail);
    // Strikes 1–2 retry on the backoff ladder; strike 3 (and beyond)
    // waits the 60 s cooldown — the approved 3-consecutive-crash rule.
    let delay = if slot.crashes as usize >= MAX_CONSECUTIVE_CRASHES || backoff.is_empty() {
        cooldown
    } else {
        backoff[((slot.crashes - 1) as usize).min(backoff.len() - 1)]
    };
    slot.retry_at = Some(Instant::now() + delay);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use lsp_types::{SaveOptions, TextDocumentSyncOptions};
    use std::fs;

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_lsp_server.py")
    }

    fn fake_spec(case: &str, dir: &Path, caps: &str) -> LspSpec {
        LspSpec {
            id: "fake".to_owned(),
            argv: vec![
                "python3".to_owned(),
                fixture().to_string_lossy().into_owned(),
            ],
            env: vec![
                ("FAKE_LSP_CASE".to_owned(), case.to_owned()),
                (
                    "FAKE_LSP_PIDFILE".to_owned(),
                    dir.join("server.pid").to_string_lossy().into_owned(),
                ),
                (
                    "FAKE_LSP_CHILD_PIDFILE".to_owned(),
                    dir.join("server-child.pid").to_string_lossy().into_owned(),
                ),
                (
                    "FAKE_LSP_LOG".to_owned(),
                    dir.join("server.log").to_string_lossy().into_owned(),
                ),
                (
                    "FAKE_LSP_COUNT".to_owned(),
                    dir.join("count").to_string_lossy().into_owned(),
                ),
                ("FAKE_LSP_CAPS".to_owned(), caps.to_owned()),
                // Empty default: no method hangs unless a test opts in.
                ("FAKE_LSP_HANG".to_owned(), String::new()),
            ],
            disabled: false,
            languages: BTreeMap::new(),
        }
    }

    const CAPS_FULL: &str = "{}";

    fn conn_events() -> (mpsc::Sender<ConnEvent>, mpsc::Receiver<ConnEvent>) {
        mpsc::channel(CONN_EVENTS)
    }

    async fn connect_fake(case: &str, dir: &Path) -> (Connection, mpsc::Receiver<ConnEvent>) {
        let (tx, rx) = conn_events();
        let spec = fake_spec(case, dir, CAPS_FULL);
        let conn = Connection::connect(&spec, dir, tx).await.unwrap();
        (conn, rx)
    }

    async fn pid_in_file(dir: &Path, name: &str) -> libc::pid_t {
        let path = dir.join(name);
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    if let Ok(pid) = text.trim().parse::<libc::pid_t>() {
                        return pid;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the server should write its pid")
    }

    fn process_alive(pid: libc::pid_t) -> bool {
        // SAFETY: signal 0 only probes for the process; it sends nothing.
        (unsafe { libc::kill(pid, 0) }) == 0
    }

    fn process_gone(pid: libc::pid_t) -> bool {
        // SAFETY: signal 0 only probes for the process; it sends nothing.
        if unsafe { libc::kill(pid, 0) } == 0 {
            return false;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }

    async fn wait_gone(pid: libc::pid_t) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !process_gone(pid) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the process should exit");
    }

    async fn wait_dead(conn: &Connection) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while conn.is_alive() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the connection should die");
    }

    fn log_events(dir: &Path) -> Vec<Value> {
        std::fs::read_to_string(dir.join("server.log"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    async fn wait_log(dir: &Path, method: &str) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if log_events(dir)
                    .iter()
                    .any(|entry| entry["method"].as_str() == Some(method))
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("the server never logged {method}"));
    }

    fn hub() -> (LspHub, mpsc::Receiver<LspEvent>) {
        let hub = LspHub::new(tokio::runtime::Handle::current());
        let events = hub.take_events().expect("events taken once");
        (hub, events)
    }

    async fn next_event(events: &mut mpsc::Receiver<LspEvent>) -> LspEvent {
        tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .expect("an event should arrive")
            .expect("the event channel stays open")
    }

    // ----- transport -----

    #[tokio::test]
    async fn connect_completes_initialize_and_reports_capabilities() {
        let dir = TempDir::new("lsp-connect");
        let (conn, _rx) = connect_fake("ok", dir.path()).await;
        assert!(conn.is_alive());
        let caps = conn.capabilities().expect("capabilities stored");
        assert!(caps.completion_provider.is_some());
        assert!(caps.hover_provider.is_some());
        assert!(caps.definition_provider.is_some());
        let features = conn.features();
        assert!(features.completion && features.hover && features.definition);
        // Bare kind 2 (Incremental): change + openClose implied, no save.
        assert!(features.sync.open_close);
        assert_eq!(features.sync.change, SyncChange::Incremental);
        assert_eq!(features.sync.save, SaveSupport::None);
        conn.shutdown().await;
    }

    #[tokio::test]
    async fn initialize_sends_typed_client_capabilities() {
        let dir = TempDir::new("lsp-caps");
        let (conn, _rx) = connect_fake("ok", dir.path()).await;
        conn.shutdown().await;
        let init = log_events(dir.path())
            .into_iter()
            .find(|entry| entry["method"].as_str() == Some("initialize"))
            .expect("initialize was logged");
        let caps = &init["params"]["capabilities"];
        let text = &caps["textDocument"];
        assert_eq!(text["synchronization"]["didSave"], json!(true));
        assert_eq!(
            text["completion"]["completionItem"]["snippetSupport"],
            json!(false)
        );
        assert_eq!(
            text["completion"]["completionItem"]["insertReplaceSupport"],
            json!(true)
        );
        assert_eq!(text["hover"]["contentFormat"], json!(["plaintext"]));
        assert_eq!(text["definition"]["linkSupport"], json!(false));
        assert_eq!(text["publishDiagnostics"]["versionSupport"], json!(true));
        assert_eq!(caps["general"]["positionEncodings"], json!(["utf-16"]));
        let folder = &init["params"]["workspaceFolders"][0];
        assert!(folder["uri"].as_str().unwrap().starts_with("file://"));
        assert!(init["params"]["processId"].is_u64());
    }

    #[tokio::test]
    async fn request_response_and_server_error() {
        let dir = TempDir::new("lsp-request");
        let (conn, _rx) = connect_fake("ok", dir.path()).await;
        let result = conn
            .request(
                "textDocument/hover",
                json!({"textDocument": {"uri": "file:///x.rs"}, "position": {"line": 0, "character": 0}}),
            )
            .await
            .unwrap();
        assert_eq!(result["contents"]["value"], json!("fake hover"));
        let error = conn.request("bogus/method", json!({})).await.unwrap_err();
        assert_eq!(error.kind, LspErrorKind::Protocol);
        assert!(error.message.contains("-32601"));
        conn.shutdown().await;
    }

    /// Dropping a request future must free the transport's pending slot AND
    /// send `$/cancelRequest` — not merely drop a UI-side waiter.
    #[tokio::test]
    async fn dropping_a_request_cancels_the_pending_transport_entry() {
        let dir = TempDir::new("lsp-cancel");
        let (tx, _rx) = conn_events();
        let mut spec = fake_spec("ok", dir.path(), CAPS_FULL);
        for (key, value) in &mut spec.env {
            if key == "FAKE_LSP_HANG" {
                *value = "textDocument/hover".to_owned();
            }
        }
        let conn = Connection::connect(&spec, dir.path(), tx).await.unwrap();
        let mut fut = Box::pin(conn.request(
            "textDocument/hover",
            json!({"textDocument": {"uri": "file:///x.rs"}, "position": {"line": 0, "character": 0}}),
        ));
        // Poll the future so the request is registered and sent; the hung
        // server keeps it pending, then drop the future itself.
        assert!(
            tokio::time::timeout(Duration::from_millis(100), fut.as_mut())
                .await
                .is_err(),
            "hover should stay pending against the hung server"
        );
        drop(fut);
        // Pending slot freed at the transport layer.
        assert!(
            conn.inner
                .routes
                .lock()
                .unwrap()
                .as_ref()
                .is_none_or(|routes| routes.pending.is_empty()),
            "the dropped request must leave no pending entry"
        );
        // And the server saw a real $/cancelRequest notification.
        wait_log(dir.path(), "$/cancelRequest").await;
        // The connection still serves other requests afterwards.
        let result = conn
            .request("textDocument/completion", json!({}))
            .await
            .unwrap();
        assert!(result.is_object());
        conn.shutdown().await;
    }

    #[tokio::test]
    async fn notifications_and_fixed_server_request_answers() {
        let dir = TempDir::new("lsp-events");
        let (conn, mut rx) = connect_fake("ok", dir.path()).await;
        // The fixture fires workspace/configuration, registerCapability,
        // executeCommand, showMessage and publishDiagnostics after init.
        let mut saw_diagnostics = false;
        let mut saw_message = false;
        for _ in 0..8 {
            match tokio::time::timeout(Duration::from_secs(10), rx.recv()).await {
                Ok(Some(ConnEvent::Notification { method, .. })) => {
                    saw_diagnostics |= method == "textDocument/publishDiagnostics";
                    saw_message |= method == "window/showMessage";
                    if saw_diagnostics && saw_message {
                        break;
                    }
                }
                _ => break,
            }
        }
        assert!(saw_diagnostics, "publishDiagnostics should arrive");
        assert!(saw_message, "showMessage should arrive");
        conn.shutdown().await;
        // The fixture logged the answers it received for its requests.
        let logged = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let events = log_events(dir.path());
                let cfg = events.iter().any(|entry| {
                    entry["event"].as_str() == Some("response")
                        && entry["id"].as_str() == Some("srv-cfg-1")
                        && entry["result"] == json!([null, null])
                });
                let exec = events.iter().any(|entry| {
                    entry["event"].as_str() == Some("response")
                        && entry["id"].as_str() == Some("srv-exec-3")
                        && entry["error"]["code"] == json!(-32601)
                });
                if cfg && exec {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if logged.is_err() {
            panic!(
                "the fixture should log the server-request answers; log was: {:?}",
                log_events(dir.path())
            );
        }
    }

    #[tokio::test]
    async fn oversize_message_kills_the_connection() {
        let dir = TempDir::new("lsp-oversize");
        let (conn, mut rx) = connect_fake("oversize_after_init", dir.path()).await;
        wait_dead(&conn).await;
        let event = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("closed event")
            .expect("channel open");
        assert!(matches!(event, ConnEvent::Closed { .. }));
    }

    #[tokio::test]
    async fn bad_framing_fails_connect() {
        let dir = TempDir::new("lsp-badframe");
        let (tx, _rx) = conn_events();
        let spec = fake_spec("bad_framing", dir.path(), CAPS_FULL);
        let error = Connection::connect(&spec, dir.path(), tx)
            .await
            .unwrap_err();
        assert!(
            matches!(
                error.kind,
                LspErrorKind::Protocol | LspErrorKind::Disconnected
            ),
            "unexpected kind {:?}",
            error.kind
        );
    }

    #[tokio::test]
    async fn crash_fails_pending_requests() {
        let dir = TempDir::new("lsp-crash");
        let (conn, _rx) = connect_fake("crash_on_notify", dir.path()).await;
        conn.notify("textDocument/didOpen", json!({"textDocument": {"uri": "file:///x.rs", "languageId": "rust", "version": 1, "text": "fn main() {}"}}));
        wait_dead(&conn).await;
        let error = conn
            .request("textDocument/hover", json!({}))
            .await
            .unwrap_err();
        assert_eq!(error.kind, LspErrorKind::Disconnected);
    }

    #[tokio::test]
    async fn shutdown_terminates_the_process_group() {
        let dir = TempDir::new("lsp-killpg");
        let (conn, _rx) = connect_fake("with_grandchild", dir.path()).await;
        let child_pid = pid_in_file(dir.path(), "server.pid").await;
        let grandchild = pid_in_file(dir.path(), "server-child.pid").await;
        assert!(process_alive(child_pid));
        assert!(process_alive(grandchild));
        conn.shutdown().await;
        wait_gone(child_pid).await;
        wait_gone(grandchild).await;
    }

    #[tokio::test]
    async fn stderr_is_captured_and_bounded() {
        let dir = TempDir::new("lsp-stderr");
        let (conn, _rx) = connect_fake("stderr_noise", dir.path()).await;
        // Give the stderr task a moment to drain the flood.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let tail = conn.stderr_tail().expect("stderr captured");
        assert!(tail.len() <= MAX_STDERR_BYTES);
        assert!(tail.contains("fake stderr"));
        conn.shutdown().await;
    }

    #[tokio::test]
    async fn initialize_timeout_kills_the_child() {
        let dir = TempDir::new("lsp-timeout");
        let (tx, _rx) = conn_events();
        let spec = fake_spec("never_init", dir.path(), CAPS_FULL);
        let started = std::time::Instant::now();
        let error =
            Connection::connect_with_timeout(&spec, dir.path(), tx, Duration::from_millis(300))
                .await
                .unwrap_err();
        assert_eq!(error.kind, LspErrorKind::Timeout);
        assert!(started.elapsed() < Duration::from_secs(10));
        let pid = pid_in_file(dir.path(), "server.pid").await;
        wait_gone(pid).await;
    }

    #[tokio::test]
    async fn exit_now_fails_connect() {
        let dir = TempDir::new("lsp-exitnow");
        let (tx, _rx) = conn_events();
        let spec = fake_spec("exit_now", dir.path(), CAPS_FULL);
        let error = Connection::connect(&spec, dir.path(), tx)
            .await
            .unwrap_err();
        assert_eq!(error.kind, LspErrorKind::Disconnected);
    }

    // ----- hub -----

    #[tokio::test]
    async fn ensure_runs_and_status_reports_features() {
        let dir = TempDir::new("lsp-hub-ensure");
        let (hub, _events) = hub();
        let key = LspKey::new("co", "fake");
        let spec = fake_spec("ok", dir.path(), CAPS_FULL);
        hub.ensure(&key, &spec, dir.path()).await.unwrap();
        let status = hub.status();
        let entry = status.get(&key).expect("status entry");
        assert_eq!(entry.state, LspState::Running);
        assert_eq!(entry.epoch, 1);
        assert!(entry.features.completion);
        assert_eq!(entry.features.sync.change, SyncChange::Incremental);
        hub.shutdown().await;
    }

    #[tokio::test]
    async fn two_checkouts_run_isolated_servers() {
        let dir_a = TempDir::new("lsp-iso-a");
        let dir_b = TempDir::new("lsp-iso-b");
        let (hub, _events) = hub();
        let spec_a = fake_spec("ok", dir_a.path(), CAPS_FULL);
        let spec_b = fake_spec("ok", dir_b.path(), CAPS_FULL);
        let key_a = LspKey::new("checkout-a", "fake");
        let key_b = LspKey::new("checkout-b", "fake");
        hub.ensure(&key_a, &spec_a, dir_a.path()).await.unwrap();
        hub.ensure(&key_b, &spec_b, dir_b.path()).await.unwrap();
        let pid_a = pid_in_file(dir_a.path(), "server.pid").await;
        let pid_b = pid_in_file(dir_b.path(), "server.pid").await;
        assert_ne!(pid_a, pid_b);
        hub.shutdown().await;
        wait_gone(pid_a).await;
        wait_gone(pid_b).await;
    }

    #[tokio::test]
    async fn events_carry_key_epoch_and_closed() {
        let dir = TempDir::new("lsp-hub-events");
        let (hub, mut events) = hub();
        let key = LspKey::new("co", "fake");
        let spec = fake_spec("ok", dir.path(), CAPS_FULL);
        hub.ensure(&key, &spec, dir.path()).await.unwrap();
        // publishDiagnostics arrives tagged.
        let mut saw_diag = false;
        for _ in 0..10 {
            match next_event(&mut events).await {
                LspEvent::Notification {
                    key: k,
                    epoch,
                    method,
                    ..
                } if k == key && method == "textDocument/publishDiagnostics" => {
                    assert_eq!(epoch, 1);
                    saw_diag = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(saw_diag);
        hub.disconnect(&key).await;
        // disconnect kills the child → Closed for epoch 1.
        let mut saw_closed = false;
        for _ in 0..10 {
            match next_event(&mut events).await {
                LspEvent::Closed { key: k, epoch, .. } if k == key => {
                    assert_eq!(epoch, 1);
                    saw_closed = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(saw_closed);
        hub.shutdown().await;
    }

    #[tokio::test]
    async fn request_and_notify_route_through_the_hub() {
        let dir = TempDir::new("lsp-hub-req");
        let (hub, _events) = hub();
        let key = LspKey::new("co", "fake");
        let spec = fake_spec("ok", dir.path(), CAPS_FULL);
        hub.ensure(&key, &spec, dir.path()).await.unwrap();
        let result = hub
            .request(
                &key,
                "textDocument/definition",
                json!({"textDocument": {"uri": "file:///x.rs"}, "position": {"line": 0, "character": 0}}),
            )
            .await
            .unwrap();
        assert_eq!(result[0]["uri"], json!("file:///fake/checkout/def.rs"));
        hub.notify(
            &key,
            "textDocument/didOpen",
            json!({"textDocument": {"uri": "file:///x.rs", "languageId": "rust", "version": 1, "text": "fn main() {}"}}),
        );
        wait_log(dir.path(), "textDocument/didOpen").await;
        hub.shutdown().await;
    }

    #[tokio::test]
    async fn missing_binary_is_error_without_retry() {
        let dir = TempDir::new("lsp-hub-missing");
        let (hub, _events) = hub();
        let key = LspKey::new("co", "fake");
        let mut spec = fake_spec("ok", dir.path(), CAPS_FULL);
        spec.argv = vec!["definitely-not-a-real-lsp-xyz".to_owned()];
        let error = hub.ensure(&key, &spec, dir.path()).await.unwrap_err();
        assert_eq!(error.kind, LspErrorKind::Spawn);
        let status = hub.status();
        let status = status.get(&key).unwrap();
        assert_eq!(status.state, LspState::Error);
        assert!(status.message.as_deref().unwrap().contains("not found"));
        // No crash accounting and no backoff: an immediate retry fails fast.
        {
            let inner = hub.inner.lock().unwrap();
            let slot = inner.conns.get(&key).unwrap();
            assert_eq!(slot.crashes, 0);
            assert!(slot.retry_at.is_none());
        }
        let started = Instant::now();
        let error = hub.ensure(&key, &spec, dir.path()).await.unwrap_err();
        assert_eq!(error.kind, LspErrorKind::Spawn);
        assert!(started.elapsed() < Duration::from_secs(2));
        hub.shutdown().await;
    }

    #[tokio::test]
    async fn crash_backoff_then_respawn_with_new_epoch() {
        let dir = TempDir::new("lsp-hub-backoff");
        let (hub, _events) = hub();
        {
            let mut inner = hub.inner.lock().unwrap();
            inner.backoff = vec![Duration::from_millis(80)];
            inner.cooldown = Duration::from_millis(200);
        }
        let key = LspKey::new("co", "fake");
        let spec = fake_spec("flaky_once", dir.path(), CAPS_FULL);
        // First connect dies during initialize → counted crash + backoff.
        let error = hub.ensure(&key, &spec, dir.path()).await.unwrap_err();
        assert_eq!(error.kind, LspErrorKind::Disconnected);
        {
            let inner = hub.inner.lock().unwrap();
            let slot = inner.conns.get(&key).unwrap();
            assert_eq!(slot.crashes, 1);
            assert!(slot.retry_at.is_some());
        }
        // Inside the backoff window ensure refuses to spawn.
        let early = hub.ensure(&key, &spec, dir.path()).await.unwrap_err();
        assert_eq!(early.kind, LspErrorKind::RetryLater);
        // After the backoff the second (working) connection comes up.
        tokio::time::sleep(Duration::from_millis(120)).await;
        hub.ensure(&key, &spec, dir.path()).await.unwrap();
        let status = hub.status();
        let status = status.get(&key).unwrap();
        assert_eq!(status.state, LspState::Running);
        assert_eq!(status.epoch, 2);
        assert_eq!(status.features.sync.change, SyncChange::Incremental);
        hub.shutdown().await;
    }

    #[tokio::test]
    async fn consecutive_failures_reach_cooldown_without_reset() {
        let dir = TempDir::new("lsp-hub-cooldown");
        let (hub, _events) = hub();
        {
            let mut inner = hub.inner.lock().unwrap();
            inner.backoff = vec![Duration::from_millis(20)];
            inner.cooldown = Duration::from_secs(60);
        }
        let key = LspKey::new("co", "fake");
        // exit_on_initialize dies during every handshake → each ensure is
        // a counted failure; the counter must not reset between attempts.
        // Strikes 1–2 retry on the (test-shortened) backoff; strike 3 —
        // the approved maximum of 3 consecutive crashes — lands in the
        // 60 s cooldown.
        let spec = fake_spec("exit_on_initialize", dir.path(), CAPS_FULL);
        for _ in 0..3 {
            let _ = hub.ensure(&key, &spec, dir.path()).await;
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        {
            let inner = hub.inner.lock().unwrap();
            let slot = inner.conns.get(&key).unwrap();
            assert_eq!(slot.crashes, 3);
            assert_eq!(slot.state, LspState::Error);
            // Cooldown (~60s) is far longer than the backoff steps were.
            assert!(slot.retry_at.unwrap() > Instant::now() + Duration::from_secs(30));
        }
        // A further ensure is refused by the cooldown without counting.
        let held = hub.ensure(&key, &spec, dir.path()).await.unwrap_err();
        assert_eq!(held.kind, LspErrorKind::RetryLater);
        {
            let inner = hub.inner.lock().unwrap();
            assert_eq!(inner.conns.get(&key).unwrap().crashes, 3);
        }
        let status = hub.status();
        assert_eq!(status.get(&key).unwrap().state, LspState::Error);
        hub.shutdown().await;
    }

    #[tokio::test]
    async fn running_crash_counts_once_and_schedules_retry() {
        let dir = TempDir::new("lsp-hub-crash");
        let (hub, mut events) = hub();
        {
            let mut inner = hub.inner.lock().unwrap();
            inner.backoff = vec![Duration::from_millis(50)];
        }
        let key = LspKey::new("co", "fake");
        let spec = fake_spec("crash_on_notify", dir.path(), CAPS_FULL);
        hub.ensure(&key, &spec, dir.path()).await.unwrap();
        hub.notify(
            &key,
            "textDocument/didOpen",
            json!({"textDocument": {"uri": "file:///x.rs", "languageId": "rust", "version": 1, "text": "x"}}),
        );
        // Wait for the Closed event, then check accounting.
        for _ in 0..10 {
            if matches!(next_event(&mut events).await, LspEvent::Closed { .. }) {
                break;
            }
        }
        {
            let inner = hub.inner.lock().unwrap();
            let slot = inner.conns.get(&key).unwrap();
            assert_eq!(slot.crashes, 1);
            assert_eq!(slot.state, LspState::Error);
        }
        // A second ensure inside backoff refuses; after it, a fresh epoch.
        let early = hub.ensure(&key, &spec, dir.path()).await.unwrap_err();
        assert_eq!(early.kind, LspErrorKind::RetryLater);
        tokio::time::sleep(Duration::from_millis(80)).await;
        hub.ensure(&key, &spec, dir.path()).await.unwrap();
        assert_eq!(hub.status().get(&key).unwrap().epoch, 2);
        hub.shutdown().await;
    }

    #[tokio::test]
    async fn eviction_drops_the_oldest_idle_connection() {
        let (hub, _events) = hub();
        // Keep the TempDirs alive for the whole test; shutdown reaps the
        // processes at the end.
        let mut live: Vec<(TempDir, LspKey)> = Vec::new();
        for index in 0..MAX_SERVERS {
            let dir = TempDir::new(&format!("lsp-evict-{index}"));
            let key = LspKey::new(format!("co-{index}"), "fake");
            let spec = fake_spec("ok", dir.path(), CAPS_FULL);
            hub.ensure(&key, &spec, dir.path()).await.unwrap();
            live.push((dir, key));
        }
        // Two connections have open docs; they are not eviction candidates.
        hub.doc_opened(&live[0].1);
        hub.doc_opened(&live[1].1);
        let extra_dir = TempDir::new("lsp-evict-extra");
        let extra_key = LspKey::new("co-extra", "fake");
        let extra_spec = fake_spec("ok", extra_dir.path(), CAPS_FULL);
        hub.ensure(&extra_key, &extra_spec, extra_dir.path())
            .await
            .unwrap();
        let status = hub.status();
        // live[2] was the oldest idle connection → evicted to Off.
        assert_eq!(status.get(&live[2].1).unwrap().state, LspState::Off);
        assert_eq!(status.get(&live[0].1).unwrap().state, LspState::Running);
        assert_eq!(status.get(&extra_key).unwrap().state, LspState::Running);
        hub.shutdown().await;
    }

    #[tokio::test]
    async fn disconnect_and_shutdown_reset_slots() {
        let dir = TempDir::new("lsp-hub-disc");
        let (hub, _events) = hub();
        let key = LspKey::new("co", "fake");
        let spec = fake_spec("ok", dir.path(), CAPS_FULL);
        hub.ensure(&key, &spec, dir.path()).await.unwrap();
        hub.disconnect(&key).await;
        assert_eq!(hub.status().get(&key).unwrap().state, LspState::Off);
        hub.ensure(&key, &spec, dir.path()).await.unwrap();
        // disconnect consumed one epoch to invalidate the old forwarder;
        // the respawn is a newer generation still.
        assert!(hub.status().get(&key).unwrap().epoch > 1);
        hub.shutdown().await;
        assert!(hub.status().values().all(|s| s.state == LspState::Off));
    }

    // ----- helpers and capability matrix -----

    #[test]
    fn uri_helpers_round_trip_and_reject() {
        let dir = TempDir::new("lsp-uri");
        let nested = dir.path().join("some dir/ünïcode.rs");
        fs::create_dir_all(nested.parent().unwrap()).unwrap();
        fs::write(&nested, "x").unwrap();
        let uri = path_to_file_uri(&nested).unwrap();
        assert!(uri.starts_with("file://"));
        let back = file_uri_to_path(&uri).unwrap();
        assert_eq!(back, nested);
        assert!(file_uri_to_path("https://example.com/x.rs").is_err());
        assert!(file_uri_to_path("not a uri").is_err());
        // Relative input resolves against the cwd without panicking.
        assert!(path_to_file_uri(Path::new("Cargo.toml")).is_ok());
    }

    #[test]
    fn parse_argv_splits_and_rejects_empty() {
        assert_eq!(
            parse_argv("rust-analyzer --verbose").unwrap(),
            vec!["rust-analyzer", "--verbose"]
        );
        assert_eq!(
            parse_argv("pyright-langserver --stdio 'a b'").unwrap(),
            vec!["pyright-langserver", "--stdio", "a b"]
        );
        assert!(parse_argv("   ").is_err());
        assert!(parse_argv("unbalanced '\"quote").is_err());
    }

    #[test]
    fn sync_support_covers_the_full_matrix() {
        // Absent → nothing.
        assert_eq!(sync_support(&None), SyncSupport::default());
        // Bare kinds.
        for (kind, change, open_close) in [
            (TextDocumentSyncKind::NONE, SyncChange::None, false),
            (TextDocumentSyncKind::FULL, SyncChange::Full, true),
            (
                TextDocumentSyncKind::INCREMENTAL,
                SyncChange::Incremental,
                true,
            ),
        ] {
            let support = sync_support(&Some(TextDocumentSyncCapability::Kind(kind)));
            assert_eq!(support.change, change, "kind {kind:?}");
            assert_eq!(support.open_close, open_close, "kind {kind:?}");
            assert_eq!(support.save, SaveSupport::None, "kind {kind:?}");
        }
        // Options: openClose absent means false.
        let options = |open_close, change, save| {
            TextDocumentSyncCapability::Options(TextDocumentSyncOptions {
                open_close,
                change,
                will_save: None,
                will_save_wait_until: None,
                save,
            })
        };
        let support = sync_support(&Some(options(None, Some(TextDocumentSyncKind::FULL), None)));
        assert!(!support.open_close);
        assert_eq!(support.change, SyncChange::Full);
        assert_eq!(support.save, SaveSupport::None);
        // save variants.
        let save = |value: TextDocumentSyncSaveOptions| {
            sync_support(&Some(options(
                Some(true),
                Some(TextDocumentSyncKind::INCREMENTAL),
                Some(value),
            )))
            .save
        };
        assert_eq!(
            save(TextDocumentSyncSaveOptions::Supported(true)),
            SaveSupport::WithoutText
        );
        assert_eq!(
            save(TextDocumentSyncSaveOptions::Supported(false)),
            SaveSupport::None
        );
        assert_eq!(
            save(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                include_text: Some(true)
            })),
            SaveSupport::WithText
        );
        assert_eq!(
            save(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                include_text: Some(false)
            })),
            SaveSupport::WithoutText
        );
        assert_eq!(
            save(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                include_text: None
            })),
            SaveSupport::WithoutText
        );
        // change None inside options → no change events but openClose may hold.
        let support = sync_support(&Some(options(
            Some(true),
            Some(TextDocumentSyncKind::NONE),
            None,
        )));
        assert!(support.open_close);
        assert_eq!(support.change, SyncChange::None);
    }

    #[test]
    fn features_follow_advertised_capabilities() {
        let caps = |json: Value| -> ServerCapabilities { serde_json::from_value(json).unwrap() };
        let none = features_from(&caps(json!({})));
        assert!(!none.completion && !none.hover && !none.definition);
        assert_eq!(none.sync, SyncSupport::default());
        let partial = features_from(&caps(json!({
            "hoverProvider": false,
            "definitionProvider": {"dynamicRegistration": false},
            "textDocumentSync": {"openClose": true, "change": 1, "save": {"includeText": true}}
        })));
        assert!(!partial.hover);
        assert!(partial.definition);
        assert!(!partial.completion);
        assert!(partial.sync.open_close);
        assert_eq!(partial.sync.change, SyncChange::Full);
        assert_eq!(partial.sync.save, SaveSupport::WithText);
    }
}
