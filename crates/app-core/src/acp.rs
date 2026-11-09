//! An ACP (Agent Client Protocol) client: one long-lived agent process per
//! connection, speaking newline-delimited JSON-RPC 2.0 over the child's
//! stdio. A single reader task parses stdout and routes responses to the
//! request waiting on their id, `session/update` notifications to the
//! channel of their session, and agent-to-client requests inline: every
//! `session/request_permission` is answered `cancelled`, every other method
//! gets `-32601`. EOF or process exit marks the connection dead and fails
//! every pending request and session channel.
//!
//! `run_session` opens a fresh session in a fresh empty temp directory,
//! streams `agent_message_chunk` text until `session/prompt` answers, and
//! closes the session when the agent advertised `session/close`. Dropping
//! the `run_session` future (the hub aborts it to cancel) still sends
//! `session/cancel` and cleans up through `SessionGuard`.

use crate::agents;
use crate::error::{BridgeError, Code};
use crate::git_view;
use crate::store::Storage;
use crate::worktrees;
use protocol::message::ErrorCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdout};
use tokio::sync::{mpsc, oneshot};
use tokio::task::AbortHandle;

/// `initialize` must answer within a minute; a hung agent is killed.
const INIT_TIMEOUT: Duration = Duration::from_secs(60);
/// `session/close` is best-effort, so it gets a short deadline of its own.
const SESSION_CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// A failure of the ACP client itself. `code` picks the `BridgeError` code
/// the commands return; `hint` carries optional recovery advice (for example
/// the agent's advertised sign-in method).
#[derive(Clone, Debug)]
pub struct AcpError {
    pub message: String,
    pub hint: Option<String>,
    code: Code,
}

impl AcpError {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl std::fmt::Display for AcpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AcpError {}

impl From<AcpError> for BridgeError {
    fn from(error: AcpError) -> Self {
        let bridge = BridgeError::new(error.code, error.message);
        match error.hint {
            Some(hint) => bridge.with_hint(hint),
            None => bridge,
        }
    }
}

/// How to launch the agent: a program plus arguments, with `NAME=value`
/// pairs applied on top of the inherited environment.
#[derive(Clone, Debug, Default)]
pub struct AcpSpec {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// A live agent process. Clones share the same child.
#[derive(Clone)]
pub struct Connection {
    inner: Arc<Inner>,
}

impl Connection {
    /// Spawn the agent with a dedicated empty temp dir as its cwd and complete
    /// `initialize` within [`INIT_TIMEOUT`]. Any failure kills the child.
    pub async fn connect(spec: &AcpSpec) -> Result<Connection, AcpError> {
        Self::connect_with_timeout(spec, INIT_TIMEOUT).await
    }

    async fn connect_with_timeout(
        spec: &AcpSpec,
        init_timeout: Duration,
    ) -> Result<Connection, AcpError> {
        let dir = fresh_dir()?;
        let mut command = tokio::process::Command::new(&spec.program);
        command
            .args(&spec.args)
            .envs(
                spec.env
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_str())),
            )
            .current_dir(&dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .process_group(0);
        let mut child = command.spawn().map_err(|error| {
            let _ = std::fs::remove_dir_all(&dir);
            spawn_error(&spec.program, error)
        })?;
        let stdin = child.stdin.take().expect("piped agent stdin");
        let stdout = child.stdout.take().expect("piped agent stdout");

        let (writes, mut incoming) = mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(line) = incoming.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err()
                    || stdin.write_all(b"\n").await.is_err()
                    || stdin.flush().await.is_err()
                {
                    break;
                }
            }
        });

        let inner = Arc::new(Inner::new(dir.clone(), writes, child));
        tokio::spawn(read_loop(Arc::downgrade(&inner), stdout));
        let connection = Connection { inner };

        let initialized = tokio::time::timeout(
            init_timeout,
            connection.call(
                "initialize",
                json!({ "protocolVersion": 1, "clientCapabilities": {} }),
            ),
        )
        .await;
        match initialized {
            Ok(Ok(result)) => {
                *connection.inner.capabilities.lock().unwrap() = parse_capabilities(&result);
                Ok(connection)
            }
            Ok(Err(error)) => {
                let _ = connection.shutdown().await;
                Err(error)
            }
            Err(_) => {
                let _ = connection.shutdown().await;
                Err(AcpError::new(
                    Code::Timeout,
                    "the agent did not respond to initialize in time",
                ))
            }
        }
    }

    /// Run one prompt turn in a fresh session and return the cleaned reply.
    /// `timeout` bounds the whole exchange; on expiry `session/cancel` is sent
    /// for that session only and the connection stays alive.
    pub async fn run_session(&self, prompt: &str, timeout: Duration) -> Result<String, AcpError> {
        let cwd = fresh_dir()?;
        tokio::time::timeout(timeout, self.session_turn(prompt, cwd))
            .await
            .map_err(|_| {
                AcpError::new(
                    Code::Timeout,
                    "the agent did not finish the session in time",
                )
            })?
    }

    async fn session_turn(&self, prompt: &str, cwd: PathBuf) -> Result<String, AcpError> {
        let cwd_text = cwd
            .to_str()
            .ok_or_else(|| {
                AcpError::new(
                    Code::Daemon(ErrorCode::Internal),
                    "the session directory path is not valid UTF-8",
                )
            })?
            .to_owned();
        let mut guard = SessionGuard::new(Arc::clone(&self.inner), cwd);

        let result = match self
            .call("session/new", json!({ "cwd": cwd_text, "mcpServers": [] }))
            .await
        {
            Ok(result) => result,
            Err(error) => return Err(self.with_auth_hint(error)),
        };
        let session_id = result
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                AcpError::new(
                    Code::Daemon(ErrorCode::Internal),
                    "the agent did not return a session id",
                )
            })?;

        let (updates_tx, mut updates) = mpsc::unbounded_channel::<Value>();
        self.inner.register_session(&session_id, updates_tx)?;
        guard.set_session(session_id.clone());

        let mut answer = self.inner.request(
            "session/prompt",
            json!({
                "sessionId": session_id.as_str(),
                "prompt": [{ "type": "text", "text": prompt }],
            }),
        )?;
        guard.arm();

        let mut reply = String::new();
        let mut answered = false;
        let outcome = loop {
            tokio::select! {
                message = &mut answer => {
                    // The turn is over however the response reads. The reader
                    // routes lines in order, so every chunk sent before the
                    // response is already buffered here: drain it.
                    guard.disarm();
                    while let Ok(update) = updates.try_recv() {
                        append_chunk(&mut reply, &update);
                    }
                    answered = message.is_ok();
                    break message
                        .map_err(|_| self.inner.exited())
                        .and_then(|value| prompt_result(&value));
                }
                update = updates.recv() => match update {
                    Some(update) => append_chunk(&mut reply, &update),
                    None => {
                        guard.disarm();
                        break Err(self.inner.exited());
                    }
                },
            }
        };

        // An answered prompt — end_turn, a refusal, or the agent's own error —
        // still gets session/close when advertised. A dead process or a
        // dropped future skips it: nothing is left to answer, and the guard
        // already cancelled the turn.
        if answered && self.inner.session_close() {
            let _ = tokio::time::timeout(
                SESSION_CLOSE_TIMEOUT,
                self.call("session/close", json!({ "sessionId": session_id.as_str() })),
            )
            .await;
        }
        outcome?;
        clean_reply(&reply)
    }

    /// Send one request and return its `result`, or the agent's error.
    async fn call(&self, method: &str, params: Value) -> Result<Value, AcpError> {
        let answer = self.inner.request(method, params)?;
        let message = answer.await.map_err(|_| self.inner.exited())?;
        response_result(message)
    }

    /// `session/new` failures on an agent that advertised `authMethods` get a
    /// sign-in hint; infrastructure errors (dead process, timeout) do not.
    fn with_auth_hint(&self, error: AcpError) -> AcpError {
        if !matches!(error.code, Code::Daemon(_)) {
            return error;
        }
        match self.inner.first_auth() {
            Some(auth) => error.with_hint(format!(
                "Sign in with {} in a terminal, then try again.",
                auth.name
            )),
            None => error,
        }
    }

    /// True while the agent process is expected to answer. The reader marks
    /// the connection dead on EOF; the child check catches the gap before it.
    pub fn is_alive(&self) -> bool {
        if self.inner.routes.lock().unwrap().is_none() {
            return false;
        }
        match self.inner.child.try_lock() {
            Ok(mut child) => child
                .try_wait()
                .map(|status| status.is_none())
                .unwrap_or(true),
            // The child lock is held only while the reader or shutdown reaps
            // an exited process.
            Err(_) => false,
        }
    }

    /// Kill the agent, fail every waiter, and remove the connection's temp
    /// directory. Safe to call more than once.
    pub async fn shutdown(&self) {
        self.inner.mark_dead();
        let mut child = self.inner.child.lock().await;
        kill_tree(&mut child);
        if let Ok(status) = child.wait().await {
            self.inner.record_exit(status);
        }
        drop(child);
        let _ = std::fs::remove_dir_all(&self.inner.dir);
    }
}

struct Inner {
    /// Pending JSON-RPC responses by request id; `None` once the agent is gone.
    routes: Mutex<Option<Routes>>,
    /// Serialized JSON-RPC lines for the child's stdin; one writer task drains it.
    writes: mpsc::UnboundedSender<String>,
    child: tokio::sync::Mutex<Child>,
    /// The connection's dedicated empty working directory.
    dir: PathBuf,
    capabilities: Mutex<Capabilities>,
    /// The child's exit status once it has been reaped.
    exit: Mutex<Option<ExitStatus>>,
    next_id: AtomicU64,
}

#[derive(Default)]
struct Routes {
    pending: HashMap<u64, oneshot::Sender<Value>>,
    sessions: HashMap<String, mpsc::UnboundedSender<Value>>,
}

#[derive(Default)]
struct Capabilities {
    /// The agent advertised `sessionCapabilities.close`.
    session_close: bool,
    /// The first advertised auth method, kept for the sign-in hint.
    auth: Option<AuthMethod>,
}

#[derive(Clone)]
struct AuthMethod {
    name: String,
}

impl Inner {
    fn new(dir: PathBuf, writes: mpsc::UnboundedSender<String>, child: Child) -> Self {
        Self {
            routes: Mutex::new(Some(Routes::default())),
            writes,
            child: tokio::sync::Mutex::new(child),
            dir,
            capabilities: Mutex::new(Capabilities::default()),
            exit: Mutex::new(None),
            next_id: AtomicU64::new(1),
        }
    }

    /// Register `id`'s waiter and queue the request line. The returned receiver
    /// resolves with the whole response message, or fails once the agent is gone.
    fn request(&self, method: &str, params: Value) -> Result<oneshot::Receiver<Value>, AcpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply, answer) = oneshot::channel();
        {
            let mut shared = self.routes.lock().unwrap();
            match shared.as_mut() {
                Some(routes) => {
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
        if self.writes.send(message.to_string()).is_err() {
            if let Some(routes) = self.routes.lock().unwrap().as_mut() {
                routes.pending.remove(&id);
            }
            return Err(self.exited());
        }
        Ok(answer)
    }

    /// Queue a notification (no id), used for `session/cancel`.
    fn notify(&self, method: &str, params: Value) {
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        let _ = self.writes.send(message.to_string());
    }

    fn register_session(
        &self,
        session_id: &str,
        updates: mpsc::UnboundedSender<Value>,
    ) -> Result<(), AcpError> {
        let mut shared = self.routes.lock().unwrap();
        match shared.as_mut() {
            Some(routes) => {
                routes.sessions.insert(session_id.to_owned(), updates);
                Ok(())
            }
            None => Err(self.exited()),
        }
    }

    fn unregister_session(&self, session_id: &str) {
        if let Some(routes) = self.routes.lock().unwrap().as_mut() {
            routes.sessions.remove(session_id);
        }
    }

    /// Connection over: dropping the routes fails pending requests and closes
    /// every session channel, which is what wakes the session waiters.
    fn mark_dead(&self) {
        self.routes.lock().unwrap().take();
    }

    fn record_exit(&self, status: ExitStatus) {
        *self.exit.lock().unwrap() = Some(status);
    }

    fn session_close(&self) -> bool {
        self.capabilities.lock().unwrap().session_close
    }

    fn first_auth(&self) -> Option<AuthMethod> {
        self.capabilities.lock().unwrap().auth.clone()
    }

    fn exited(&self) -> AcpError {
        let detail = self
            .exit
            .lock()
            .unwrap()
            .as_ref()
            .map(exit_detail)
            .unwrap_or_default();
        AcpError::new(Code::Disconnected, format!("the agent exited{detail}"))
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        // `&mut self` rules out a held child guard; `kill_on_drop` still
        // covers the child itself as the field drops.
        kill_tree(self.child.get_mut());
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Cleans up one `run_session` however its future ends: `session/cancel`
/// while a prompt is still in flight, session unregistration, and removal of
/// the session's temp directory. `Drop` is synchronous, so the cancel goes
/// through the writer queue.
struct SessionGuard {
    inner: Arc<Inner>,
    cwd: PathBuf,
    session_id: Option<String>,
    armed: bool,
}

impl SessionGuard {
    fn new(inner: Arc<Inner>, cwd: PathBuf) -> Self {
        Self {
            inner,
            cwd,
            session_id: None,
            armed: false,
        }
    }

    fn set_session(&mut self, session_id: String) {
        self.session_id = Some(session_id);
    }

    /// A prompt is in flight: a dropped future must cancel the turn.
    fn arm(&mut self) {
        self.armed = true;
    }

    /// The prompt response arrived: the turn is over, no cancel needed.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        if self.armed {
            if let Some(session_id) = &self.session_id {
                self.inner.notify(
                    "session/cancel",
                    json!({ "sessionId": session_id.as_str() }),
                );
            }
        }
        if let Some(session_id) = self.session_id.take() {
            self.inner.unregister_session(&session_id);
        }
        let _ = std::fs::remove_dir_all(&self.cwd);
    }
}

/// `std::env::temp_dir()/goko-acp-<uuid>`, created empty.
fn fresh_dir() -> Result<PathBuf, AcpError> {
    let dir = std::env::temp_dir().join(format!("goko-acp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).map_err(|error| {
        AcpError::new(
            Code::Daemon(ErrorCode::Internal),
            format!("cannot create {}: {error}", dir.display()),
        )
    })?;
    Ok(dir)
}

fn spawn_error(program: &str, error: std::io::Error) -> AcpError {
    AcpError::new(
        Code::DaemonStartFailed,
        format!("cannot start {program}: {error}"),
    )
}

/// Kill the agent's whole process group, not just the child: the child leads
/// its own group (`process_group(0)` at spawn), so wrappers like `npx` cannot
/// leave grandchildren behind. `start_kill` still covers the child itself —
/// and is the only option once its pid is gone.
fn kill_tree(child: &mut Child) {
    if let Some(pid) = child.id() {
        // SAFETY: killpg(2) on the group the child leads; ESRCH just means
        // the whole group is already gone.
        unsafe { libc::killpg(pid as libc::pid_t, libc::SIGKILL) };
    }
    let _ = child.start_kill();
}

async fn read_loop(weak: Weak<Inner>, stdout: ChildStdout) {
    let mut lines = BufReader::new(stdout).lines();
    loop {
        let Ok(Some(line)) = lines.next_line().await else {
            break;
        };
        // A strong reference is taken per line only: while blocked on read the
        // connection may be dropped, which is what kills the child.
        let Some(inner) = weak.upgrade() else {
            break;
        };
        route(&inner, &line);
    }
    let Some(inner) = weak.upgrade() else {
        return;
    };
    inner.mark_dead();
    // Reap the child for its exit status; killing its group also covers an
    // agent that closed stdout but kept running.
    let mut child = inner.child.lock().await;
    kill_tree(&mut child);
    if let Ok(status) = child.wait().await {
        inner.record_exit(status);
    }
}

fn route(inner: &Inner, line: &str) {
    let Ok(message) = serde_json::from_str::<Value>(line) else {
        return;
    };
    let method = message.get("method").and_then(Value::as_str);
    let id = message.get("id").filter(|id| !id.is_null());
    match (id, method) {
        // An agent-to-client request carries both `id` and `method`.
        (Some(id), Some(method)) => answer_request(inner, id, method),
        (None, Some("session/update")) => route_update(inner, &message),
        // A response carries `id` and no `method`.
        (Some(id), None) => {
            if let Some(id) = id.as_u64() {
                route_response(inner, id, message);
            }
        }
        _ => {}
    }
}

fn route_response(inner: &Inner, id: u64, message: Value) {
    let mut shared = inner.routes.lock().unwrap();
    let Some(routes) = shared.as_mut() else {
        return;
    };
    if let Some(waiting) = routes.pending.remove(&id) {
        let _ = waiting.send(message);
    }
}

fn route_update(inner: &Inner, message: &Value) {
    let Some(session_id) = message.pointer("/params/sessionId").and_then(Value::as_str) else {
        return;
    };
    let Some(update) = message.pointer("/params/update") else {
        return;
    };
    let mut shared = inner.routes.lock().unwrap();
    let Some(routes) = shared.as_mut() else {
        return;
    };
    let closed = match routes.sessions.get(session_id) {
        Some(sender) => sender.send(update.clone()).is_err(),
        None => false,
    };
    if closed {
        routes.sessions.remove(session_id);
    }
}

fn answer_request(inner: &Inner, id: &Value, method: &str) {
    let reply = if method == "session/request_permission" {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "outcome": { "outcome": "cancelled" } },
        })
    } else {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": "method not found" },
        })
    };
    let _ = inner.writes.send(reply.to_string());
}

/// The `result` of a response, or the agent's JSON-RPC error as an `AcpError`.
fn response_result(message: Value) -> Result<Value, AcpError> {
    match message.get("error") {
        Some(error) => Err(agent_error(error)),
        None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
    }
}

fn agent_error(error: &Value) -> AcpError {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("the agent returned an error");
    AcpError::new(Code::Daemon(ErrorCode::Internal), message)
}

/// The `session/prompt` response must end the turn with `end_turn`.
fn prompt_result(message: &Value) -> Result<(), AcpError> {
    if let Some(error) = message.get("error") {
        return Err(agent_error(error));
    }
    let reason = message
        .pointer("/result/stopReason")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    if reason == "end_turn" {
        Ok(())
    } else {
        Err(AcpError::new(
            Code::Daemon(ErrorCode::Internal),
            format!("the agent returned stop reason '{reason}'"),
        ))
    }
}

fn append_chunk(reply: &mut String, update: &Value) {
    if update.get("sessionUpdate").and_then(Value::as_str) != Some("agent_message_chunk") {
        return;
    }
    let Some(content) = update.get("content") else {
        return;
    };
    if content.get("type").and_then(Value::as_str) != Some("text") {
        return;
    }
    if let Some(text) = content.get("text").and_then(Value::as_str) {
        reply.push_str(text);
    }
}

/// Trim the reply and remove one surrounding ``` fence, if the whole reply
/// is fenced; an empty result is an error.
fn clean_reply(reply: &str) -> Result<String, AcpError> {
    let text = strip_fence(reply.trim());
    if text.is_empty() {
        return Err(AcpError::new(
            Code::Daemon(ErrorCode::Internal),
            "The agent returned no message.",
        ));
    }
    Ok(text.to_owned())
}

fn strip_fence(text: &str) -> &str {
    if text.len() <= 6 || !text.starts_with("```") || !text.ends_with("```") {
        return text;
    }
    let inner = &text[3..text.len() - 3];
    match inner.find('\n') {
        // The first line after the opening fence is the optional language tag.
        Some(newline) => inner[newline + 1..].trim(),
        None => inner.trim(),
    }
}

fn parse_capabilities(result: &Value) -> Capabilities {
    Capabilities {
        session_close: result
            .pointer("/agentCapabilities/sessionCapabilities/close")
            .is_some_and(Value::is_object),
        auth: result
            .get("authMethods")
            .and_then(Value::as_array)
            .and_then(|methods| methods.first())
            .and_then(|method| {
                let name = method.get("name").and_then(Value::as_str)?.to_owned();
                (!name.is_empty()).then_some(AuthMethod { name })
            }),
    }
}

fn exit_detail(status: &ExitStatus) -> String {
    if let Some(code) = status.code() {
        format!(" (exit code {code})")
    } else if let Some(signal) = status.signal() {
        format!(" (signal {signal})")
    } else {
        String::new()
    }
}

// ---------- agent configuration ----------

/// Settings key holding the `acp.agents` override rows.
const AGENTS_KEY: &str = "acp.agents";
/// A custom commit prompt is capped at 4 KiB.
const PROMPT_CAP: usize = 4 * 1024;
/// `git diff --cached` sent to the agent is capped at 120 KiB.
const DIFF_CAP: usize = 120 * 1024;
/// One Generate may run for three minutes before its session is cancelled.
const GENERATE_TIMEOUT: Duration = Duration::from_secs(180);

/// The commit-message instructions used when the agent has no custom prompt.
const DEFAULT_PROMPT: &str = "Write a git commit message for the staged changes below. Reply with ONLY the commit message: a subject line of at most 72 characters in the imperative mood, optionally followed by a blank line and a short body. Match the style of the recent commit subjects. Do not use code fences, quotes, or commentary, and do not use any tools.";

/// One built-in agent row; an empty `prompt` means [`DEFAULT_PROMPT`].
#[derive(Clone, Debug)]
pub(crate) struct AcpPreset {
    pub id: &'static str,
    pub label: &'static str,
    pub command: &'static str,
    pub enabled: bool,
    pub prompt: &'static str,
}

pub(crate) static PRESETS: &[AcpPreset] = &[
    AcpPreset {
        id: "devin",
        label: "Devin",
        command: "DEVIN_PERMISSION_MODE=auto devin acp --model swe-2-flash-max",
        enabled: true,
        prompt: "",
    },
    AcpPreset {
        id: "opencode",
        label: "OpenCode",
        command: "opencode acp",
        enabled: true,
        prompt: "",
    },
    AcpPreset {
        id: "claude",
        label: "Claude Code",
        command: "npx -y @zed-industries/claude-agent-acp",
        enabled: true,
        prompt: "",
    },
    AcpPreset {
        id: "codex",
        label: "Codex",
        command: "npx -y @zed-industries/codex-acp",
        enabled: true,
        prompt: "",
    },
    AcpPreset {
        id: "gemini",
        label: "Gemini",
        command: "gemini --experimental-acp",
        enabled: true,
        prompt: "",
    },
];

/// One agent row as the webview sees it: effective values plus PATH
/// availability and whether the row still matches its preset.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AcpAgent {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub command: String,
    pub prompt: String,
    pub available: bool,
    pub is_default: bool,
}

/// One `acp.agents` row, or one `acp_agents_set` argument: the fields that
/// make an agent differ from its preset.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AcpOverride {
    pub id: String,
    pub enabled: bool,
    pub command: String,
    pub prompt: String,
}

/// The effective configuration of one preset agent after stored overrides.
#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub command: String,
    pub prompt: String,
}

fn preset(id: &str) -> Option<&'static AcpPreset> {
    PRESETS.iter().find(|preset| preset.id == id)
}

/// The stored override rows keyed by agent id. Missing, unreadable, or
/// invalid JSON means no overrides — every agent shows its preset.
fn stored_overrides(storage: &Storage) -> Result<HashMap<String, AcpOverride>, BridgeError> {
    let Some(text) = storage.get()?.get_setting(AGENTS_KEY)? else {
        return Ok(HashMap::new());
    };
    let rows: Vec<AcpOverride> = serde_json::from_str(&text).unwrap_or_default();
    Ok(rows
        .into_iter()
        .filter(|row| preset(&row.id).is_some())
        .map(|row| (row.id.clone(), row))
        .collect())
}

/// The five agents with their effective values. `available` is the command's
/// program word resolving on `PATH`; `is_default` means the row equals its
/// preset on every field.
pub fn agents_get(storage: &Storage) -> Result<Vec<AcpAgent>, BridgeError> {
    let stored = stored_overrides(storage)?;
    Ok(PRESETS
        .iter()
        .map(|preset| {
            let row = stored.get(preset.id);
            let enabled = row.map_or(preset.enabled, |row| row.enabled);
            let command = row.map_or_else(|| preset.command.to_owned(), |row| row.command.clone());
            let prompt = row.map_or_else(|| preset.prompt.to_owned(), |row| row.prompt.clone());
            AcpAgent {
                id: preset.id.to_owned(),
                label: preset.label.to_owned(),
                enabled,
                available: command_available(&command),
                is_default: enabled == preset.enabled
                    && command == preset.command
                    && prompt == preset.prompt,
                command,
                prompt,
            }
        })
        .collect())
}

/// The effective configuration of one preset agent; an unknown id is
/// `not_found`.
pub fn agent_config(storage: &Storage, agent_id: &str) -> Result<AgentConfig, BridgeError> {
    let preset =
        preset(agent_id).ok_or_else(|| not_found(format!("unknown ACP agent '{agent_id}'")))?;
    let stored = stored_overrides(storage)?;
    let row = stored.get(agent_id);
    Ok(AgentConfig {
        id: preset.id.to_owned(),
        label: preset.label.to_owned(),
        enabled: row.map_or(preset.enabled, |row| row.enabled),
        command: row.map_or_else(|| preset.command.to_owned(), |row| row.command.clone()),
        prompt: row.map_or_else(|| preset.prompt.to_owned(), |row| row.prompt.clone()),
    })
}

/// Validate every override, store the rows that differ from their preset
/// under `acp.agents`, disconnect agents whose effective `enabled` or
/// `command` changed, and return the new effective list. A bad row rejects
/// the whole call and stores nothing.
pub async fn agents_set(
    storage: &Storage,
    hub: &AcpHub,
    overrides: Vec<AcpOverride>,
) -> Result<Vec<AcpAgent>, BridgeError> {
    let before = agents_get(storage)?;
    for row in &overrides {
        validate_override(row)?;
    }
    let mut merged = HashMap::new();
    for row in &overrides {
        merged.insert(row.id.as_str(), row);
    }
    let mut stored = Vec::new();
    for preset in PRESETS {
        let Some(row) = merged.get(preset.id) else {
            continue;
        };
        if row.enabled != preset.enabled
            || row.command != preset.command
            || row.prompt != preset.prompt
        {
            stored.push(AcpOverride {
                id: preset.id.to_owned(),
                enabled: row.enabled,
                command: row.command.clone(),
                prompt: row.prompt.clone(),
            });
        }
    }
    let text = serde_json::to_string(&stored).map_err(|error| {
        BridgeError::new(
            Code::Storage,
            format!("cannot encode the ACP agents: {error}"),
        )
    })?;
    storage.get()?.set_setting(AGENTS_KEY, &text)?;
    let after = agents_get(storage)?;
    for (was, now) in before.iter().zip(after.iter()) {
        if was.enabled && (!now.enabled || was.command != now.command) {
            hub.disconnect(&was.id).await?;
        }
    }
    Ok(after)
}

/// One override row must name a preset, carry a parseable command with a
/// program word, and keep the prompt under 4 KiB.
fn validate_override(row: &AcpOverride) -> Result<(), BridgeError> {
    let Some(preset) = preset(&row.id) else {
        return Err(invalid(format!("unknown ACP agent '{}'", row.id)));
    };
    if row.prompt.len() > PROMPT_CAP {
        return Err(invalid(format!(
            "{}'s commit prompt is longer than 4 KiB",
            preset.label
        )));
    }
    parse_command(&row.command).map_err(|error| {
        invalid(format!(
            "{}'s command is invalid: {}",
            preset.label, error.message
        ))
    })?;
    Ok(())
}

/// Split a command line into an [`AcpSpec`]: leading `NAME=value` words become
/// environment variables, the next word is the program, the rest are args.
/// No shell is involved; an unbalanced quote or a missing program word is
/// `invalid_argument`.
pub(crate) fn parse_command(command: &str) -> Result<AcpSpec, BridgeError> {
    let words =
        shlex::split(command).ok_or_else(|| invalid("the command has an unbalanced quote"))?;
    let mut env = Vec::new();
    let mut words = words.into_iter();
    while let Some(word) = words.next() {
        match env_assignment(&word) {
            Some((name, value)) => env.push((name.to_owned(), value.to_owned())),
            None => {
                return Ok(AcpSpec {
                    program: word,
                    args: words.collect(),
                    env,
                });
            }
        }
    }
    Err(invalid("the command has no program word"))
}

/// `NAME=value` where `NAME` is a shell identifier; the value may be empty or
/// hold further `=`.
fn env_assignment(word: &str) -> Option<(&str, &str)> {
    let (name, value) = word.split_once('=')?;
    let mut chars = name.chars();
    let valid = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    valid.then_some((name, value))
}

/// `available` is the command's program word resolving on `PATH` — or, for a
/// program containing `/`, the path itself being an executable file.
fn command_available(command: &str) -> bool {
    match parse_command(command) {
        Ok(spec) => program_available(&spec.program),
        Err(_) => false,
    }
}

fn program_available(program: &str) -> bool {
    if program.contains('/') {
        let path = Path::new(program);
        return path.is_file()
            && path
                .metadata()
                .is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0);
    }
    agents::resolve(program, std::env::var_os("PATH").as_deref()).is_some()
}

// ---------- the connection hub ----------

/// The serialized per-agent status inside `acp_status`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AcpStatus {
    pub state: AgentState,
    pub message: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Error,
}

#[derive(Default)]
struct Slot {
    state: AgentState,
    message: Option<String>,
    connection: Option<Connection>,
    /// Bumped on every connect and disconnect; a connect applies its result
    /// only while its epoch is current, so a disconnect during `initialize`
    /// cannot resurrect the agent.
    epoch: u64,
    /// Fires to cancel an in-flight `connect`; set only while connecting.
    cancel: Option<oneshot::Sender<()>>,
}

struct HubInner {
    slots: HashMap<String, Slot>,
    /// Running Generate jobs by (project, checkout); a new job aborts the old.
    jobs: HashMap<(String, String), (u64, AbortHandle)>,
    next_job: u64,
}

/// Managed state: one connection slot per preset agent plus the running
/// Generate jobs. Clones share the same state.
#[derive(Clone)]
pub struct AcpHub {
    inner: Arc<Mutex<HubInner>>,
}

impl AcpHub {
    pub fn new() -> Self {
        let slots = PRESETS
            .iter()
            .map(|preset| (preset.id.to_owned(), Slot::default()))
            .collect();
        Self {
            inner: Arc::new(Mutex::new(HubInner {
                slots,
                jobs: HashMap::new(),
                next_job: 0,
            })),
        }
    }

    /// Connect the agent: spawn its process and finish `initialize`. An
    /// already connected or connecting agent is left alone; a failure records
    /// `error` with the reason and is returned.
    pub async fn connect(&self, config: &AgentConfig) -> Result<(), BridgeError> {
        if !config.enabled {
            return Err(invalid(format!("{} is disabled", config.label)));
        }
        let spec = parse_command(&config.command).map_err(|error| {
            invalid(format!(
                "{}'s command is invalid: {}",
                config.label, error.message
            ))
        })?;
        if !program_available(&spec.program) {
            let message = format!("{} was not found on PATH", config.label);
            {
                let mut inner = self.inner.lock().unwrap();
                if let Some(slot) = inner.slots.get_mut(config.id.as_str()) {
                    slot.state = AgentState::Error;
                    slot.message = Some(message.clone());
                    slot.connection = None;
                }
            }
            return Err(invalid(message).with_hint("Fix the command in Settings → ACP agents."));
        }

        let (mut cancel_rx, epoch) = {
            let mut inner = self.inner.lock().unwrap();
            let slot = inner
                .slots
                .get_mut(config.id.as_str())
                .ok_or_else(|| not_found(format!("unknown ACP agent '{}'", config.id)))?;
            if slot.state == AgentState::Connecting
                || (slot.state == AgentState::Connected
                    && slot.connection.as_ref().is_some_and(Connection::is_alive))
            {
                return Ok(());
            }
            slot.epoch += 1;
            slot.state = AgentState::Connecting;
            slot.message = None;
            slot.connection = None;
            let (cancel, rx) = oneshot::channel();
            slot.cancel = Some(cancel);
            (rx, slot.epoch)
        };

        let outcome = tokio::select! {
            outcome = Connection::connect(&spec) => Some(outcome),
            _ = &mut cancel_rx => None,
        };

        match outcome {
            // Cancelled by disconnect: the slot was already reset there.
            None => Err(cancelled()),
            Some(Err(error)) => {
                let bridge = BridgeError::from(error.clone());
                let mut inner = self.inner.lock().unwrap();
                if let Some(slot) = inner.slots.get_mut(config.id.as_str()) {
                    if slot.epoch == epoch {
                        slot.state = AgentState::Error;
                        slot.message = Some(error.message);
                        slot.cancel = None;
                    }
                }
                Err(bridge)
            }
            Some(Ok(connection)) => {
                let mut pending = Some(connection);
                let stored = {
                    let mut inner = self.inner.lock().unwrap();
                    match inner.slots.get_mut(config.id.as_str()) {
                        Some(slot) if slot.epoch == epoch => {
                            slot.state = AgentState::Connected;
                            slot.message = None;
                            slot.connection = pending.take();
                            slot.cancel = None;
                            true
                        }
                        _ => false,
                    }
                };
                if stored {
                    Ok(())
                } else {
                    // A disconnect landed while initialize finished: do not
                    // resurrect the agent.
                    if let Some(connection) = pending.take() {
                        connection.shutdown().await;
                    }
                    Err(cancelled())
                }
            }
        }
    }

    /// Kill the agent process and drop back to `disconnected`; also cancels
    /// an in-flight connect.
    pub async fn disconnect(&self, agent_id: &str) -> Result<(), BridgeError> {
        let (connection, cancel) = {
            let mut inner = self.inner.lock().unwrap();
            let slot = inner
                .slots
                .get_mut(agent_id)
                .ok_or_else(|| not_found(format!("unknown ACP agent '{agent_id}'")))?;
            slot.epoch += 1;
            slot.state = AgentState::Disconnected;
            slot.message = None;
            (slot.connection.take(), slot.cancel.take())
        };
        if let Some(cancel) = cancel {
            let _ = cancel.send(());
        }
        if let Some(connection) = connection {
            connection.shutdown().await;
        }
        Ok(())
    }

    /// Every preset agent's status; a `connected` agent whose process died
    /// flips to `error` here so polling callers see it without a separate
    /// check.
    pub fn status(&self) -> BTreeMap<String, AcpStatus> {
        let mut inner = self.inner.lock().unwrap();
        for slot in inner.slots.values_mut() {
            if slot.state == AgentState::Connected
                && !slot.connection.as_ref().is_some_and(Connection::is_alive)
            {
                slot.state = AgentState::Error;
                slot.message = Some("the agent exited unexpectedly".to_owned());
                slot.connection = None;
            }
        }
        PRESETS
            .iter()
            .filter_map(|preset| {
                inner.slots.get(preset.id).map(|slot| {
                    (
                        preset.id.to_owned(),
                        AcpStatus {
                            state: slot.state,
                            message: slot.message.clone(),
                        },
                    )
                })
            })
            .collect()
    }

    /// Ask the connected agent to draft the commit message for this
    /// checkout's staged changes. One job runs per (project, checkout): a new
    /// call aborts the previous one, and [`Self::cancel`] aborts the current
    /// one; either way the aborted call reports `cancelled` and the
    /// connection stays up.
    pub async fn generate_commit_message(
        &self,
        config: AgentConfig,
        project_id: String,
        checkout_path: String,
        checkout: PathBuf,
    ) -> Result<String, BridgeError> {
        let AgentConfig {
            id,
            label,
            prompt: custom_prompt,
            ..
        } = config;
        let connection = {
            let mut inner = self.inner.lock().unwrap();
            let slot = inner
                .slots
                .get_mut(id.as_str())
                .ok_or_else(|| not_found(format!("unknown ACP agent '{id}'")))?;
            let connected = slot.state == AgentState::Connected
                && slot.connection.as_ref().is_some_and(Connection::is_alive);
            if !connected {
                if slot.state == AgentState::Connected {
                    slot.state = AgentState::Error;
                    slot.message = Some("the agent exited unexpectedly".to_owned());
                    slot.connection = None;
                }
                return Err(BridgeError::new(
                    Code::Disconnected,
                    format!("{label} is not connected"),
                ));
            }
            slot.connection
                .clone()
                .expect("a connected slot holds its connection")
        };

        // Git inspection and prompt assembly on a blocking thread.
        let prompt = tokio::task::spawn_blocking(move || build_prompt(&checkout, &custom_prompt))
            .await
            .map_err(|error| {
                BridgeError::new(
                    Code::Daemon(ErrorCode::Internal),
                    format!("cannot inspect the staged changes: {error}"),
                )
            })??;

        // Register the job and spawn its task under one lock, so two
        // concurrent Generates cannot both miss the other's entry. The guard
        // ends with the block so the await below stays `Send`.
        let key = (project_id, checkout_path);
        let task_key = key.clone();
        let task = {
            let mut inner = self.inner.lock().unwrap();
            if let Some((_, previous)) = inner.jobs.remove(&key) {
                previous.abort();
            }
            inner.next_job += 1;
            let job_id = inner.next_job;
            let hub = self.clone();
            let task = tokio::spawn(async move {
                let result = hub.run_generation(&id, connection, prompt).await;
                hub.complete_job(&task_key, job_id);
                result
            });
            let handle = task.abort_handle();
            inner.jobs.insert(key, (job_id, handle));
            task
        };

        match task.await {
            Ok(result) => result,
            Err(error) if error.is_cancelled() => Err(cancelled()),
            Err(error) => Err(BridgeError::new(
                Code::Daemon(ErrorCode::Internal),
                format!("commit message generation failed: {error}"),
            )),
        }
    }

    /// The Generate turn on a live connection: `run_session` at the 180 s
    /// deadline; a dead process flips the agent's slot to `error`.
    async fn run_generation(
        &self,
        agent_id: &str,
        connection: Connection,
        prompt: String,
    ) -> Result<String, BridgeError> {
        match connection.run_session(&prompt, GENERATE_TIMEOUT).await {
            Ok(reply) => Ok(reply),
            Err(error) => {
                if !connection.is_alive() {
                    self.mark_agent_dead(agent_id);
                }
                Err(error.into())
            }
        }
    }

    fn complete_job(&self, key: &(String, String), job_id: u64) {
        let mut inner = self.inner.lock().unwrap();
        if inner.jobs.get(key).is_some_and(|(id, _)| *id == job_id) {
            inner.jobs.remove(key);
        }
    }

    fn mark_agent_dead(&self, agent_id: &str) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(slot) = inner.slots.get_mut(agent_id) {
            if slot.state == AgentState::Connected {
                slot.state = AgentState::Error;
                slot.message = Some("the agent exited unexpectedly".to_owned());
                slot.connection = None;
            }
        }
    }

    /// Abort the running Generate for this checkout, if any.
    pub fn cancel(&self, project_id: &str, checkout_path: &str) {
        let key = (project_id.to_owned(), checkout_path.to_owned());
        if let Some((_, handle)) = self.inner.lock().unwrap().jobs.remove(&key) {
            handle.abort();
        }
    }

    /// App exit: cancel connects, abort running generations, and shut every
    /// connection down so no agent process outlives the app.
    pub async fn shutdown(&self) {
        let (connections, cancels, jobs) = {
            let mut inner = self.inner.lock().unwrap();
            for slot in inner.slots.values_mut() {
                slot.state = AgentState::Disconnected;
                slot.message = None;
            }
            let connections: Vec<Connection> = inner
                .slots
                .values_mut()
                .filter_map(|slot| slot.connection.take())
                .collect();
            let cancels: Vec<oneshot::Sender<()>> = inner
                .slots
                .values_mut()
                .filter_map(|slot| slot.cancel.take())
                .collect();
            let jobs: Vec<AbortHandle> =
                inner.jobs.drain().map(|(_, (_, handle))| handle).collect();
            (connections, cancels, jobs)
        };
        for cancel in cancels {
            let _ = cancel.send(());
        }
        for handle in jobs {
            handle.abort();
        }
        for connection in connections {
            connection.shutdown().await;
        }
    }
}

impl Default for AcpHub {
    fn default() -> Self {
        Self::new()
    }
}

/// The prompt for one Generate: the agent's commit prompt (or the default
/// instructions), the last few commit subjects for style, `--stat`, and the
/// staged diff capped at [`DIFF_CAP`]. Rejects when nothing is staged or
/// conflicts remain.
fn build_prompt(checkout: &Path, custom: &str) -> Result<String, BridgeError> {
    let view = git_view::changes(checkout)?;
    if !view.conflicts.is_empty() {
        return Err(git_view::invalid(
            "resolve the merge conflicts before generating a commit message",
        ));
    }
    if view.staged.is_empty() {
        return Err(git_view::invalid(
            "nothing is staged to generate a commit message from",
        ));
    }
    let mut prompt = String::new();
    if custom.trim().is_empty() {
        prompt.push_str(DEFAULT_PROMPT);
    } else {
        prompt.push_str(custom);
    }
    prompt.push_str("\n\nRecent commit subjects (git log -5 --format=%s):\n");
    let log = worktrees::run(checkout, &["log", "-5", "--format=%s"], None)?;
    if log.status.success() {
        prompt.push_str(&String::from_utf8_lossy(&log.stdout));
    }
    prompt.push_str("\nStaged changes (git diff --cached --stat):\n");
    let stat = worktrees::git(checkout, &["diff", "--cached", "--stat"])?;
    prompt.push_str(&String::from_utf8_lossy(&stat));
    prompt.push_str("\nStaged diff (git diff --cached):\n");
    let (diff, truncated) = worktrees::git_bounded(checkout, &["diff", "--cached"], DIFF_CAP)?;
    prompt.push_str(&String::from_utf8_lossy(&diff));
    if truncated {
        prompt.push_str("\n(diff truncated)");
    }
    prompt.push('\n');
    Ok(prompt)
}

fn cancelled() -> BridgeError {
    BridgeError::new(Code::Daemon(ErrorCode::Internal), "cancelled")
}

fn invalid(message: impl Into<String>) -> BridgeError {
    BridgeError::new(Code::Daemon(ErrorCode::InvalidArgument), message.into())
}

fn not_found(message: impl Into<String>) -> BridgeError {
    BridgeError::new(Code::Daemon(ErrorCode::NotFound), message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::fs;
    use std::path::Path;

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_acp_agent.py")
    }

    fn fake_spec(case: &str, dir: &Path) -> AcpSpec {
        AcpSpec {
            program: "python3".to_owned(),
            args: vec![fixture().to_string_lossy().into_owned()],
            env: vec![
                ("FAKE_ACP_CASE".to_owned(), case.to_owned()),
                (
                    "FAKE_ACP_PIDFILE".to_owned(),
                    dir.join("agent.pid").to_string_lossy().into_owned(),
                ),
                (
                    "FAKE_ACP_CHILD_PIDFILE".to_owned(),
                    dir.join("agent-child.pid").to_string_lossy().into_owned(),
                ),
                (
                    "FAKE_ACP_LOG".to_owned(),
                    dir.join("agent.log").to_string_lossy().into_owned(),
                ),
            ],
        }
    }

    async fn connect_fake(case: &str, dir: &Path) -> Connection {
        Connection::connect(&fake_spec(case, dir)).await.unwrap()
    }

    async fn agent_pid(dir: &Path) -> libc::pid_t {
        pid_in_file(dir, "agent.pid").await
    }

    async fn grandchild_pid(dir: &Path) -> libc::pid_t {
        pid_in_file(dir, "agent-child.pid").await
    }

    async fn pid_in_file(dir: &Path, name: &str) -> libc::pid_t {
        let path = dir.join(name);
        tokio::time::timeout(Duration::from_secs(5), async {
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
        .expect("the agent should write its pid")
    }

    fn process_alive(pid: libc::pid_t) -> bool {
        // SAFETY: signal 0 only probes for the process; it sends nothing.
        (unsafe { libc::kill(pid, 0) }) == 0
    }

    async fn wait_dead(pid: libc::pid_t) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while process_alive(pid) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the agent process should exit");
    }

    fn process_gone(pid: libc::pid_t) -> bool {
        // SAFETY: signal 0 only probes for the process; it sends nothing.
        if unsafe { libc::kill(pid, 0) } == 0 {
            return false;
        }
        // ESRCH means no such process; EPERM would mean it is still alive.
        std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }

    async fn wait_gone(pid: libc::pid_t) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !process_gone(pid) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the grandchild process should exit");
    }

    async fn wait_dead_connection(conn: &Connection) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while conn.is_alive() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the connection should die");
    }

    fn log_events(dir: &Path) -> Vec<Value> {
        std::fs::read_to_string(dir.join("agent.log"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    async fn wait_log(dir: &Path, event: &str) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if log_events(dir)
                    .iter()
                    .any(|entry| entry["event"].as_str() == Some(event))
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("the agent never logged {event}"));
    }

    fn session_new_events(dir: &Path) -> Vec<Value> {
        log_events(dir)
            .into_iter()
            .filter(|entry| entry["event"].as_str() == Some("session_new"))
            .collect()
    }

    #[tokio::test]
    async fn connect_records_agent_capabilities() {
        let dir = TempDir::new("acp-connect");
        let conn = connect_fake("ok", dir.path()).await;

        assert!(conn.is_alive());
        assert!(conn.inner.session_close());
        assert!(conn.inner.first_auth().is_none());

        conn.shutdown().await;
        assert!(!conn.is_alive());
    }

    #[tokio::test]
    async fn connect_timeout_kills_the_agent() {
        let dir = TempDir::new("acp-noinit");
        let error = Connection::connect_with_timeout(
            &fake_spec("no_init", dir.path()),
            Duration::from_millis(300),
        )
        .await
        .err()
        .expect("connect should time out");

        assert_eq!(error.code, Code::Timeout);
        wait_dead(agent_pid(dir.path()).await).await;
    }

    #[tokio::test]
    async fn connect_missing_program_fails() {
        let spec = AcpSpec {
            program: "goko-no-such-acp-agent".to_owned(),
            ..AcpSpec::default()
        };
        let error = Connection::connect(&spec)
            .await
            .err()
            .expect("connect should fail");

        assert!(error.message.contains("goko-no-such-acp-agent"));
    }

    #[tokio::test]
    async fn two_sessions_use_distinct_ids_and_cwds() {
        let dir = TempDir::new("acp-sessions");
        let conn = connect_fake("ok", dir.path()).await;

        let first = conn
            .run_session("one", Duration::from_secs(10))
            .await
            .unwrap();
        let second = conn
            .run_session("two", Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(first, "feat: demo");
        assert_eq!(second, "feat: demo");

        let sessions = session_new_events(dir.path());
        assert_eq!(sessions.len(), 2);
        assert_ne!(sessions[0]["sessionId"], sessions[1]["sessionId"]);
        let cwds: Vec<&str> = sessions
            .iter()
            .map(|session| session["cwd"].as_str().unwrap())
            .collect();
        assert_ne!(cwds[0], cwds[1]);
        for cwd in cwds {
            assert!(
                !Path::new(cwd).exists(),
                "session cwd {cwd} should be removed"
            );
        }
        // Each turn was closed afterwards.
        assert_eq!(
            log_events(dir.path())
                .iter()
                .filter(|entry| entry["event"].as_str() == Some("session_close"))
                .count(),
            2
        );

        conn.shutdown().await;
    }

    #[tokio::test]
    async fn reply_chunks_are_concatenated() {
        let dir = TempDir::new("acp-chunks");
        let conn = connect_fake("ok", dir.path()).await;

        let reply = conn
            .run_session("hi", Duration::from_secs(10))
            .await
            .unwrap();

        assert_eq!(reply, "feat: demo");
        conn.shutdown().await;
    }

    #[tokio::test]
    async fn fenced_reply_is_unwrapped() {
        let dir = TempDir::new("acp-fenced");
        let conn = connect_fake("fenced", dir.path()).await;

        let reply = conn
            .run_session("hi", Duration::from_secs(10))
            .await
            .unwrap();

        assert_eq!(reply, "feat: demo");
        conn.shutdown().await;
    }

    #[tokio::test]
    async fn agent_requests_get_cancelled_and_method_not_found() {
        let dir = TempDir::new("acp-perm");
        let conn = connect_fake("perm", dir.path()).await;

        let reply = conn
            .run_session("hi", Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(reply, "feat: demo");

        let events = log_events(dir.path());
        let responses: Vec<&Value> = events
            .iter()
            .filter_map(|entry| entry.get("response"))
            .collect();
        assert_eq!(responses.len(), 2, "expected both agent request answers");
        assert!(responses.iter().any(|response| {
            response
                .pointer("/result/outcome/outcome")
                .and_then(Value::as_str)
                == Some("cancelled")
        }));
        assert!(responses.iter().any(|response| {
            response.pointer("/error/code").and_then(Value::as_i64) == Some(-32601)
        }));

        conn.shutdown().await;
    }

    #[tokio::test]
    async fn non_json_lines_are_ignored() {
        let dir = TempDir::new("acp-noise");
        let conn = connect_fake("noise", dir.path()).await;

        let reply = conn
            .run_session("hi", Duration::from_secs(10))
            .await
            .unwrap();

        assert_eq!(reply, "feat: demo");
        conn.shutdown().await;
    }

    #[tokio::test]
    async fn refusal_stop_reason_is_an_error() {
        let dir = TempDir::new("acp-refusal");
        let conn = connect_fake("refusal", dir.path()).await;

        let error = conn
            .run_session("hi", Duration::from_secs(10))
            .await
            .unwrap_err();

        assert!(error.message.contains("refusal"), "{}", error.message);
        conn.shutdown().await;
    }

    #[tokio::test]
    async fn refusal_still_closes_the_session() {
        let dir = TempDir::new("acp-refusal-close");
        let conn = connect_fake("refusal", dir.path()).await;

        let error = conn
            .run_session("hi", Duration::from_secs(10))
            .await
            .unwrap_err();
        assert!(error.message.contains("refusal"), "{}", error.message);

        wait_log(dir.path(), "session_close").await;
        conn.shutdown().await;
    }

    #[tokio::test]
    async fn empty_reply_is_an_error() {
        let dir = TempDir::new("acp-empty");
        let conn = connect_fake("empty", dir.path()).await;

        let error = conn
            .run_session("hi", Duration::from_secs(10))
            .await
            .unwrap_err();

        assert_eq!(error.message, "The agent returned no message.");
        conn.shutdown().await;
    }

    #[tokio::test]
    async fn timeout_cancels_the_session_and_keeps_the_connection() {
        let dir = TempDir::new("acp-hang");
        let conn = connect_fake("hang_prompt", dir.path()).await;

        let error = conn
            .run_session("hi", Duration::from_millis(300))
            .await
            .unwrap_err();
        assert_eq!(error.code, Code::Timeout);

        wait_log(dir.path(), "session_cancel").await;
        assert!(conn.is_alive());
        conn.shutdown().await;
    }

    #[tokio::test]
    async fn aborting_run_session_cancels_the_turn() {
        let dir = TempDir::new("acp-abort");
        let conn = connect_fake("hang_prompt", dir.path()).await;
        let worker = {
            let conn = conn.clone();
            tokio::spawn(async move { conn.run_session("hi", Duration::from_secs(30)).await })
        };

        wait_log(dir.path(), "prompt").await;
        worker.abort();
        assert!(worker.await.unwrap_err().is_cancelled());

        wait_log(dir.path(), "session_cancel").await;
        assert!(conn.is_alive());
        conn.shutdown().await;
    }

    #[tokio::test]
    async fn agent_exit_marks_the_connection_dead() {
        let dir = TempDir::new("acp-crash");
        let conn = connect_fake("crash_after_init", dir.path()).await;

        wait_dead_connection(&conn).await;
        let error = conn
            .run_session("hi", Duration::from_secs(10))
            .await
            .unwrap_err();
        assert!(error.message.contains("exited"), "{}", error.message);
    }

    #[tokio::test]
    async fn killing_the_agent_fails_the_running_session() {
        let dir = TempDir::new("acp-kill");
        let conn = connect_fake("hang_prompt", dir.path()).await;
        let worker = {
            let conn = conn.clone();
            tokio::spawn(async move { conn.run_session("hi", Duration::from_secs(30)).await })
        };

        wait_log(dir.path(), "prompt").await;
        // SAFETY: plain kill(2) on the pid the agent wrote for this test.
        unsafe { libc::kill(agent_pid(dir.path()).await, libc::SIGKILL) };

        let error = worker.await.unwrap().unwrap_err();
        assert!(error.message.contains("exited"), "{}", error.message);
        assert!(!conn.is_alive());
        conn.shutdown().await;
    }

    #[tokio::test]
    async fn session_new_auth_failure_adds_a_sign_in_hint() {
        let dir = TempDir::new("acp-auth");
        let conn = connect_fake("auth_fail", dir.path()).await;

        let error = conn
            .run_session("hi", Duration::from_secs(10))
            .await
            .unwrap_err();
        assert!(
            error.message.contains("authentication required"),
            "{}",
            error.message
        );
        let hint = error.hint.expect("a sign-in hint");
        assert!(hint.contains("Login with fake"), "{hint}");
        assert!(conn.is_alive());
        conn.shutdown().await;
    }

    #[tokio::test]
    async fn shutdown_kills_the_agent_and_removes_its_dir() {
        let dir = TempDir::new("acp-shutdown");
        let conn = connect_fake("ok", dir.path()).await;
        let pid = agent_pid(dir.path()).await;
        let conn_dir = conn.inner.dir.clone();

        conn.shutdown().await;

        wait_dead(pid).await;
        assert!(!conn.is_alive());
        assert!(!conn_dir.exists());
    }

    #[tokio::test]
    async fn dropping_the_connection_kills_the_agent() {
        let dir = TempDir::new("acp-drop");
        let conn = connect_fake("ok", dir.path()).await;
        let pid = agent_pid(dir.path()).await;
        let conn_dir = conn.inner.dir.clone();

        drop(conn);

        wait_dead(pid).await;
        assert!(!conn_dir.exists());
    }

    #[tokio::test]
    async fn shutdown_kills_the_agent_process_group() {
        let dir = TempDir::new("acp-group-shutdown");
        let conn = connect_fake("with_grandchild", dir.path()).await;
        let grandchild = grandchild_pid(dir.path()).await;
        assert!(process_alive(grandchild));

        conn.shutdown().await;

        wait_gone(grandchild).await;
    }

    #[tokio::test]
    async fn dropping_the_connection_kills_the_agent_process_group() {
        let dir = TempDir::new("acp-group-drop");
        let conn = connect_fake("with_grandchild", dir.path()).await;
        let grandchild = grandchild_pid(dir.path()).await;
        assert!(process_alive(grandchild));

        drop(conn);

        wait_gone(grandchild).await;
    }

    // ---------- agent configuration and the hub ----------

    fn storage(dir: &TempDir) -> Storage {
        Storage::open(dir.path())
    }

    /// The command line a test override gives one preset agent: the fake
    /// fixture selected by `case`, logging to `<dir>/agent.log` and writing
    /// its pid to `<dir>/agent.pid` so tests can signal their own agent.
    fn fake_command(case: &str, dir: &TempDir) -> String {
        format!(
            "FAKE_ACP_CASE={case} FAKE_ACP_LOG={} FAKE_ACP_PIDFILE={} python3 {}",
            dir.path().join("agent.log").display(),
            dir.path().join("agent.pid").display(),
            fixture().display()
        )
    }

    fn override_row(id: &str, command: &str) -> AcpOverride {
        AcpOverride {
            id: id.to_owned(),
            enabled: true,
            command: command.to_owned(),
            prompt: String::new(),
        }
    }

    async fn set_fake_agent(storage: &Storage, hub: &AcpHub, id: &str, case: &str, dir: &TempDir) {
        agents_set(
            storage,
            hub,
            vec![override_row(id, &fake_command(case, dir))],
        )
        .await
        .unwrap();
    }

    fn git_ok(root: &Path, args: &[&str]) -> Vec<u8> {
        let out = std::process::Command::new("git")
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
        out.stdout
    }

    /// A two-commit repo on `main` with clean history.
    fn repo(label: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new(label);
        let root = dir.path().canonicalize().unwrap();
        git_ok(&root, &["init", "-b", "main"]);
        git_ok(&root, &["config", "user.name", "Tester"]);
        git_ok(&root, &["config", "user.email", "tester@example.com"]);
        fs::write(root.join("tracked.txt"), "tracked\n").unwrap();
        git_ok(&root, &["add", "-A"]);
        git_ok(&root, &["commit", "-qm", "init"]);
        fs::write(root.join("widget.txt"), "widget\n").unwrap();
        git_ok(&root, &["add", "widget.txt"]);
        git_ok(&root, &["commit", "-qm", "add widget"]);
        (dir, root)
    }

    /// A repo mid-merge: `file.txt` is conflicted, `other.txt` is staged.
    fn conflicted_repo(label: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new(label);
        let root = dir.path().canonicalize().unwrap();
        git_ok(&root, &["init", "-b", "main"]);
        git_ok(&root, &["config", "user.name", "Tester"]);
        git_ok(&root, &["config", "user.email", "tester@example.com"]);
        fs::write(root.join("file.txt"), "base\n").unwrap();
        git_ok(&root, &["add", "-A"]);
        git_ok(&root, &["commit", "-qm", "base"]);
        git_ok(&root, &["checkout", "-qb", "side"]);
        fs::write(root.join("file.txt"), "side\n").unwrap();
        git_ok(&root, &["commit", "-qam", "side"]);
        git_ok(&root, &["checkout", "-q", "main"]);
        fs::write(root.join("file.txt"), "main\n").unwrap();
        git_ok(&root, &["commit", "-qam", "main"]);
        let merge = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["merge", "side"])
            .output()
            .expect("git merge");
        assert!(!merge.status.success(), "the merge should conflict");
        stage(&root, "other.txt", b"other\n");
        (dir, root)
    }

    fn stage(root: &Path, name: &str, contents: &[u8]) {
        fs::write(root.join(name), contents).unwrap();
        git_ok(root, &["add", name]);
    }

    fn project(storage: &Storage, root: &Path) -> String {
        storage
            .get()
            .unwrap()
            .add_project("p", &root.display().to_string())
            .unwrap()
            .id
    }

    /// The `text` of every `prompt` event the fake agent logged.
    fn prompt_texts(dir: &TempDir) -> Vec<String> {
        log_events(dir.path())
            .into_iter()
            .filter(|entry| entry["event"].as_str() == Some("prompt"))
            .filter_map(|entry| entry["text"].as_str().map(str::to_owned))
            .collect()
    }

    #[test]
    fn presets_are_returned_when_nothing_is_stored() {
        let dir = TempDir::new("acp-presets");
        let agents = agents_get(&storage(&dir)).unwrap();

        let ids: Vec<&str> = agents.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["devin", "opencode", "claude", "codex", "gemini"]);
        let commands: Vec<(&str, &str)> = agents
            .iter()
            .map(|a| (a.id.as_str(), a.command.as_str()))
            .collect();
        assert_eq!(
            commands,
            [
                (
                    "devin",
                    "DEVIN_PERMISSION_MODE=auto devin acp --model swe-2-flash-max"
                ),
                ("opencode", "opencode acp"),
                ("claude", "npx -y @zed-industries/claude-agent-acp"),
                ("codex", "npx -y @zed-industries/codex-acp"),
                ("gemini", "gemini --experimental-acp"),
            ]
        );
        for agent in &agents {
            assert!(agent.enabled, "{}", agent.id);
            assert!(agent.is_default, "{}", agent.id);
            assert_eq!(agent.prompt, "");
        }
        assert_eq!(agents[0].label, "Devin");
        assert_eq!(agents[1].label, "OpenCode");
        assert_eq!(agents[2].label, "Claude Code");
        assert_eq!(agents[3].label, "Codex");
        assert_eq!(agents[4].label, "Gemini");
    }

    #[tokio::test]
    async fn overrides_merge_with_presets_and_reset_rows_store_nothing() {
        let dir = TempDir::new("acp-merge");
        let storage = storage(&dir);
        let hub = AcpHub::new();

        agents_set(
            &storage,
            &hub,
            vec![
                AcpOverride {
                    id: "opencode".into(),
                    enabled: false,
                    command: "custom acp".into(),
                    prompt: "custom prompt".into(),
                },
                // Equal to the devin preset on every field: dropped, not stored.
                override_row(
                    "devin",
                    "DEVIN_PERMISSION_MODE=auto devin acp --model swe-2-flash-max",
                ),
            ],
        )
        .await
        .unwrap();

        let stored = storage
            .get()
            .unwrap()
            .get_setting("acp.agents")
            .unwrap()
            .unwrap();
        let rows: Vec<AcpOverride> = serde_json::from_str(&stored).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "opencode");

        let agents = agents_get(&storage).unwrap();
        let opencode = agents.iter().find(|a| a.id == "opencode").unwrap();
        assert!(!opencode.enabled);
        assert_eq!(opencode.command, "custom acp");
        assert_eq!(opencode.prompt, "custom prompt");
        assert!(!opencode.is_default);
        let devin = agents.iter().find(|a| a.id == "devin").unwrap();
        assert!(devin.is_default);
        assert!(devin.enabled);
        assert_eq!(
            devin.command,
            "DEVIN_PERMISSION_MODE=auto devin acp --model swe-2-flash-max"
        );

        // Setting the preset values back drops the row entirely.
        agents_set(
            &storage,
            &hub,
            vec![AcpOverride {
                id: "opencode".into(),
                enabled: true,
                command: "opencode acp".into(),
                prompt: String::new(),
            }],
        )
        .await
        .unwrap();
        let stored = storage
            .get()
            .unwrap()
            .get_setting("acp.agents")
            .unwrap()
            .unwrap();
        let rows: Vec<AcpOverride> = serde_json::from_str(&stored).unwrap();
        assert!(rows.is_empty());
        let opencode = agents_get(&storage)
            .unwrap()
            .into_iter()
            .find(|a| a.id == "opencode")
            .unwrap();
        assert!(opencode.is_default);
        assert!(opencode.enabled);
    }

    #[test]
    fn devin_command_parses_env_program_and_args() {
        let spec =
            parse_command("DEVIN_PERMISSION_MODE=auto devin acp --model swe-2-flash-max").unwrap();
        assert_eq!(
            spec.env,
            vec![("DEVIN_PERMISSION_MODE".to_owned(), "auto".to_owned())]
        );
        assert_eq!(spec.program, "devin");
        assert_eq!(spec.args, ["acp", "--model", "swe-2-flash-max"]);
    }

    #[test]
    fn command_parsing_handles_quotes_env_and_program_words() {
        let spec = parse_command("prog \"two words\" 'x=y'").unwrap();
        assert!(spec.env.is_empty());
        assert_eq!(spec.program, "prog");
        assert_eq!(spec.args, ["two words", "x=y"]);

        // Env words only count before the program; `FOO=bar` after it is an arg.
        let spec = parse_command("A=1 B=two=2 prog run FOO=bar").unwrap();
        assert_eq!(
            spec.env,
            vec![
                ("A".to_owned(), "1".to_owned()),
                ("B".to_owned(), "two=2".to_owned())
            ]
        );
        assert_eq!(spec.program, "prog");
        assert_eq!(spec.args, ["run", "FOO=bar"]);
    }

    #[tokio::test]
    async fn agents_set_rejects_bad_rows_and_stores_nothing() {
        let dir = TempDir::new("acp-bad-rows");
        let storage = storage(&dir);
        let hub = AcpHub::new();
        let cases: Vec<(&str, AcpOverride)> = vec![
            (
                "unterminated quote",
                override_row("opencode", "opencode 'acp"),
            ),
            ("empty command", override_row("opencode", "   ")),
            ("env words only", override_row("opencode", "FOO=bar BAZ=1")),
            ("unknown id", override_row("nope", "whatever acp")),
            (
                "oversized prompt",
                AcpOverride {
                    id: "opencode".into(),
                    enabled: true,
                    command: "opencode acp".into(),
                    prompt: "x".repeat(PROMPT_CAP + 1),
                },
            ),
        ];
        for (name, row) in cases {
            let error = agents_set(&storage, &hub, vec![row]).await.unwrap_err();
            assert_eq!(
                error.code,
                Code::Daemon(ErrorCode::InvalidArgument),
                "{name}: {error:?}"
            );
            assert!(
                storage
                    .get()
                    .unwrap()
                    .get_setting("acp.agents")
                    .unwrap()
                    .is_none(),
                "{name} stored something"
            );
        }
        // Nothing was stored, so a good row still applies cleanly.
        agents_set(
            &storage,
            &hub,
            vec![override_row("opencode", "opencode acp")],
        )
        .await
        .unwrap();
    }

    #[test]
    fn invalid_stored_json_falls_back_to_presets() {
        let dir = TempDir::new("acp-bad-json");
        let storage = storage(&dir);
        let store = storage.get().unwrap();
        for bad in ["not json", "[1,2]", "{\"id\":\"opencode\"}", "null"] {
            store.set_setting("acp.agents", bad).unwrap();
            let agents = agents_get(&storage).unwrap();
            assert!(
                agents.iter().all(|a| a.is_default && a.enabled),
                "{bad} did not fall back to presets"
            );
        }
    }

    #[tokio::test]
    async fn connect_then_status_and_disconnect_round_trip() {
        let dir = TempDir::new("acp-hub-life");
        let storage = storage(&dir);
        let hub = AcpHub::new();
        set_fake_agent(&storage, &hub, "opencode", "ok", &dir).await;
        let config = agent_config(&storage, "opencode").unwrap();

        hub.connect(&config).await.unwrap();
        let status = hub.status();
        assert_eq!(status.len(), 5);
        assert_eq!(status["opencode"].state, AgentState::Connected);
        assert_eq!(status["opencode"].message, None);
        assert_eq!(status["devin"].state, AgentState::Disconnected);

        // Connecting again is a no-op.
        hub.connect(&config).await.unwrap();
        assert_eq!(hub.status()["opencode"].state, AgentState::Connected);

        hub.disconnect("opencode").await.unwrap();
        assert_eq!(hub.status()["opencode"].state, AgentState::Disconnected);
        assert_eq!(hub.status()["opencode"].message, None);
    }

    #[tokio::test]
    async fn connecting_a_missing_program_sets_the_error_status() {
        let dir = TempDir::new("acp-hub-missing");
        let storage = storage(&dir);
        let hub = AcpHub::new();
        agents_set(
            &storage,
            &hub,
            vec![override_row("opencode", "goko-no-such-acp-agent acp")],
        )
        .await
        .unwrap();
        let config = agent_config(&storage, "opencode").unwrap();

        let error = hub.connect(&config).await.unwrap_err();
        assert!(
            error.message.contains("OpenCode was not found on PATH"),
            "{}",
            error.message
        );
        assert_eq!(
            error.hint.as_deref(),
            Some("Fix the command in Settings → ACP agents.")
        );
        let status = hub.status();
        assert_eq!(status["opencode"].state, AgentState::Error);
        assert!(status["opencode"]
            .message
            .as_deref()
            .unwrap()
            .contains("was not found on PATH"));
    }

    #[tokio::test]
    async fn connecting_a_disabled_agent_is_rejected() {
        let dir = TempDir::new("acp-hub-disabled");
        let storage = storage(&dir);
        let hub = AcpHub::new();
        agents_set(
            &storage,
            &hub,
            vec![AcpOverride {
                id: "opencode".into(),
                enabled: false,
                command: "opencode acp".into(),
                prompt: String::new(),
            }],
        )
        .await
        .unwrap();
        let config = agent_config(&storage, "opencode").unwrap();
        assert!(!config.enabled);

        let error = hub.connect(&config).await.unwrap_err();
        assert!(error.message.contains("disabled"), "{}", error.message);
        assert_eq!(hub.status()["opencode"].state, AgentState::Disconnected);
    }

    #[tokio::test]
    async fn a_crashed_agent_shows_error_in_status() {
        let dir = TempDir::new("acp-hub-crash");
        let storage = storage(&dir);
        let hub = AcpHub::new();
        // "ok" stays alive after initialize; the test kills the agent itself
        // once Connected is observed, so the Error transition cannot race
        // the assert the way `crash_after_init`'s immediate exit could.
        set_fake_agent(&storage, &hub, "opencode", "ok", &dir).await;
        let config = agent_config(&storage, "opencode").unwrap();

        hub.connect(&config).await.unwrap();
        assert_eq!(hub.status()["opencode"].state, AgentState::Connected);

        // SAFETY: plain kill(2) on the pid this test's agent wrote.
        unsafe { libc::kill(agent_pid(dir.path()).await, libc::SIGKILL) };

        // The next status poll flips the dead connection to Error.
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if hub.status()["opencode"].state == AgentState::Error {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the crashed agent should show error");
        hub.disconnect("opencode").await.unwrap();
    }

    #[tokio::test]
    async fn disabling_or_reediting_a_connected_agent_disconnects_it() {
        let dir = TempDir::new("acp-hub-reconfig");
        let storage = storage(&dir);
        let hub = AcpHub::new();
        set_fake_agent(&storage, &hub, "opencode", "ok", &dir).await;
        let config = agent_config(&storage, "opencode").unwrap();
        hub.connect(&config).await.unwrap();
        assert_eq!(hub.status()["opencode"].state, AgentState::Connected);

        // Disabling disconnects it.
        agents_set(
            &storage,
            &hub,
            vec![AcpOverride {
                id: "opencode".into(),
                enabled: false,
                command: fake_command("ok", &dir),
                prompt: String::new(),
            }],
        )
        .await
        .unwrap();
        assert_eq!(hub.status()["opencode"].state, AgentState::Disconnected);

        // Re-enable, reconnect, then editing the command disconnects it again.
        set_fake_agent(&storage, &hub, "opencode", "ok", &dir).await;
        let config = agent_config(&storage, "opencode").unwrap();
        hub.connect(&config).await.unwrap();
        assert_eq!(hub.status()["opencode"].state, AgentState::Connected);

        agents_set(
            &storage,
            &hub,
            vec![override_row("opencode", &fake_command("ok2", &dir))],
        )
        .await
        .unwrap();
        assert_eq!(hub.status()["opencode"].state, AgentState::Disconnected);
    }

    #[tokio::test]
    async fn generate_requires_a_connected_agent() {
        let home = TempDir::new("acp-gen-noconn");
        let storage = storage(&home);
        let hub = AcpHub::new();
        let (_repo_dir, root) = repo("acp-gen-noconn-repo");
        let project_id = project(&storage, &root);
        stage(&root, "demo.txt", b"demo\n");
        let config = agent_config(&storage, "opencode").unwrap();

        let error = hub
            .generate_commit_message(config, project_id, root.display().to_string(), root.clone())
            .await
            .unwrap_err();
        assert_eq!(error.message, "OpenCode is not connected");
        assert_eq!(error.code, Code::Disconnected);

        // An unknown agent id never reaches the hub.
        assert!(agent_config(&storage, "nope").is_err());
    }

    #[tokio::test]
    async fn generate_rejects_nothing_staged_and_merge_conflicts() {
        let home = TempDir::new("acp-gen-states");
        let storage = storage(&home);
        let hub = AcpHub::new();
        set_fake_agent(&storage, &hub, "opencode", "ok", &home).await;
        hub.connect(&agent_config(&storage, "opencode").unwrap())
            .await
            .unwrap();

        // Clean repo: nothing staged.
        let (_clean_dir, clean) = repo("acp-gen-clean");
        let error = hub
            .generate_commit_message(
                agent_config(&storage, "opencode").unwrap(),
                project(&storage, &clean),
                clean.display().to_string(),
                clean.clone(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
        assert!(
            error.message.contains("nothing is staged"),
            "{}",
            error.message
        );

        // Mid-merge repo: unresolved conflicts reject before staging matters.
        let (_conf_dir, conflicted) = conflicted_repo("acp-gen-conflict");
        let error = hub
            .generate_commit_message(
                agent_config(&storage, "opencode").unwrap(),
                project(&storage, &conflicted),
                conflicted.display().to_string(),
                conflicted.clone(),
            )
            .await
            .unwrap_err();
        assert!(error.message.contains("conflicts"), "{}", error.message);

        hub.disconnect("opencode").await.unwrap();
    }

    #[tokio::test]
    async fn generate_returns_the_reply_and_sends_the_full_prompt() {
        let home = TempDir::new("acp-gen-ok");
        let storage = storage(&home);
        let hub = AcpHub::new();
        set_fake_agent(&storage, &hub, "opencode", "ok", &home).await;
        hub.connect(&agent_config(&storage, "opencode").unwrap())
            .await
            .unwrap();

        let (_repo_dir, root) = repo("acp-gen-ok-repo");
        let project_id = project(&storage, &root);
        stage(&root, "demo.txt", b"demo\n");

        let reply = hub
            .generate_commit_message(
                agent_config(&storage, "opencode").unwrap(),
                project_id,
                root.display().to_string(),
                root.clone(),
            )
            .await
            .unwrap();
        assert_eq!(reply, "feat: demo");

        let texts = prompt_texts(&home);
        assert_eq!(texts.len(), 1);
        let prompt = &texts[0];
        assert!(prompt.contains(DEFAULT_PROMPT), "missing instructions");
        assert!(prompt.contains("add widget"), "missing recent subjects");
        assert!(prompt.contains("init"));
        assert!(prompt.contains("--stat"));
        assert!(prompt.contains("demo.txt"));
        assert!(prompt.contains("git diff --cached"));
        assert!(prompt.contains("+demo"));

        hub.disconnect("opencode").await.unwrap();
    }

    #[tokio::test]
    async fn generate_marks_a_capped_diff_as_truncated() {
        let home = TempDir::new("acp-gen-trunc");
        let storage = storage(&home);
        let hub = AcpHub::new();
        set_fake_agent(&storage, &hub, "opencode", "ok", &home).await;
        hub.connect(&agent_config(&storage, "opencode").unwrap())
            .await
            .unwrap();

        let (_repo_dir, root) = repo("acp-gen-trunc-repo");
        let project_id = project(&storage, &root);
        // One line past the 120 KiB cap.
        stage(&root, "big.txt", &vec![b'x'; DIFF_CAP + 4096]);

        let reply = hub
            .generate_commit_message(
                agent_config(&storage, "opencode").unwrap(),
                project_id,
                root.display().to_string(),
                root.clone(),
            )
            .await
            .unwrap();
        assert_eq!(reply, "feat: demo");

        let texts = prompt_texts(&home);
        assert_eq!(texts.len(), 1);
        assert!(texts[0].contains("(diff truncated)"));

        hub.disconnect("opencode").await.unwrap();
    }

    #[tokio::test]
    async fn generate_with_only_binary_changes_still_sends_the_stat() {
        let home = TempDir::new("acp-gen-binary");
        let storage = storage(&home);
        let hub = AcpHub::new();
        set_fake_agent(&storage, &hub, "opencode", "ok", &home).await;
        hub.connect(&agent_config(&storage, "opencode").unwrap())
            .await
            .unwrap();

        let (_repo_dir, root) = repo("acp-gen-binary-repo");
        let project_id = project(&storage, &root);
        // Contains NUL bytes, so git treats it as binary.
        let bytes: Vec<u8> = (0..256u32).map(|i| i as u8).collect();
        stage(&root, "blob.bin", &bytes);

        let reply = hub
            .generate_commit_message(
                agent_config(&storage, "opencode").unwrap(),
                project_id,
                root.display().to_string(),
                root.clone(),
            )
            .await
            .unwrap();
        assert_eq!(reply, "feat: demo");

        let texts = prompt_texts(&home);
        assert_eq!(texts.len(), 1);
        assert!(texts[0].contains("--stat"));
        assert!(texts[0].contains("blob.bin"));

        hub.disconnect("opencode").await.unwrap();
    }

    #[tokio::test]
    async fn a_custom_prompt_replaces_the_default_and_an_empty_one_uses_it() {
        let home = TempDir::new("acp-gen-custom");
        let storage = storage(&home);
        let hub = AcpHub::new();
        agents_set(
            &storage,
            &hub,
            vec![AcpOverride {
                id: "opencode".into(),
                enabled: true,
                command: fake_command("ok", &home),
                prompt: "CUSTOM PROMPT MARKER".into(),
            }],
        )
        .await
        .unwrap();
        hub.connect(&agent_config(&storage, "opencode").unwrap())
            .await
            .unwrap();

        let (_repo_dir, root) = repo("acp-gen-custom-repo");
        let project_id = project(&storage, &root);
        stage(&root, "demo.txt", b"demo\n");
        let checkout_path = root.display().to_string();

        hub.generate_commit_message(
            agent_config(&storage, "opencode").unwrap(),
            project_id.clone(),
            checkout_path.clone(),
            root.clone(),
        )
        .await
        .unwrap();
        let texts = prompt_texts(&home);
        assert_eq!(texts.len(), 1);
        assert!(texts[0].contains("CUSTOM PROMPT MARKER"));
        assert!(!texts[0].contains(DEFAULT_PROMPT));

        // An empty prompt means the default; the command is unchanged, so the
        // connection survives the settings write.
        agents_set(
            &storage,
            &hub,
            vec![override_row("opencode", &fake_command("ok", &home))],
        )
        .await
        .unwrap();
        assert_eq!(hub.status()["opencode"].state, AgentState::Connected);
        hub.generate_commit_message(
            agent_config(&storage, "opencode").unwrap(),
            project_id,
            checkout_path,
            root.clone(),
        )
        .await
        .unwrap();
        let texts = prompt_texts(&home);
        assert_eq!(texts.len(), 2);
        assert!(texts[1].contains(DEFAULT_PROMPT));
        assert!(!texts[1].contains("CUSTOM PROMPT MARKER"));

        hub.disconnect("opencode").await.unwrap();
    }

    #[tokio::test]
    async fn a_second_generate_aborts_the_first() {
        let home = TempDir::new("acp-gen-abort");
        let storage = storage(&home);
        let hub = AcpHub::new();
        // opencode hangs inside its prompt turn; gemini answers immediately.
        agents_set(
            &storage,
            &hub,
            vec![
                override_row("opencode", &fake_command("hang_prompt", &home)),
                override_row("gemini", &fake_command("ok", &home)),
            ],
        )
        .await
        .unwrap();
        hub.connect(&agent_config(&storage, "opencode").unwrap())
            .await
            .unwrap();
        hub.connect(&agent_config(&storage, "gemini").unwrap())
            .await
            .unwrap();

        let (_repo_dir, root) = repo("acp-gen-abort-repo");
        let project_id = project(&storage, &root);
        stage(&root, "demo.txt", b"demo\n");
        let checkout_path = root.display().to_string();

        // The first Generate hangs inside its session.
        let first = {
            let hub = hub.clone();
            let config = agent_config(&storage, "opencode").unwrap();
            let (project_id, checkout_path, checkout) =
                (project_id.clone(), checkout_path.clone(), root.clone());
            tokio::spawn(async move {
                hub.generate_commit_message(config, project_id, checkout_path, checkout)
                    .await
            })
        };
        wait_log(home.path(), "prompt").await;

        // A second Generate for the same checkout aborts it and answers.
        let reply = hub
            .generate_commit_message(
                agent_config(&storage, "gemini").unwrap(),
                project_id,
                checkout_path,
                root.clone(),
            )
            .await
            .unwrap();
        assert_eq!(reply, "feat: demo");

        let error = first.await.unwrap().unwrap_err();
        assert_eq!(error.message, "cancelled");

        // Both connections are still alive.
        let status = hub.status();
        assert_eq!(status["opencode"].state, AgentState::Connected);
        assert_eq!(status["gemini"].state, AgentState::Connected);
        hub.disconnect("opencode").await.unwrap();
        hub.disconnect("gemini").await.unwrap();
    }

    #[tokio::test]
    async fn cancel_ends_a_running_generation_and_keeps_the_connection() {
        let home = TempDir::new("acp-gen-cancel");
        let storage = storage(&home);
        let hub = AcpHub::new();
        set_fake_agent(&storage, &hub, "opencode", "hang_prompt", &home).await;
        hub.connect(&agent_config(&storage, "opencode").unwrap())
            .await
            .unwrap();

        let (_repo_dir, root) = repo("acp-gen-cancel-repo");
        let project_id = project(&storage, &root);
        stage(&root, "demo.txt", b"demo\n");
        let checkout_path = root.display().to_string();

        let running = {
            let hub = hub.clone();
            let config = agent_config(&storage, "opencode").unwrap();
            let (project_id, checkout_path, checkout) =
                (project_id.clone(), checkout_path.clone(), root.clone());
            tokio::spawn(async move {
                hub.generate_commit_message(config, project_id, checkout_path, checkout)
                    .await
            })
        };
        wait_log(home.path(), "prompt").await;

        hub.cancel(&project_id, &checkout_path);
        let error = running.await.unwrap().unwrap_err();
        assert_eq!(error.message, "cancelled");
        // The session was cancelled on the wire and the connection stayed up.
        wait_log(home.path(), "session_cancel").await;
        assert_eq!(hub.status()["opencode"].state, AgentState::Connected);
        hub.disconnect("opencode").await.unwrap();
    }
}
