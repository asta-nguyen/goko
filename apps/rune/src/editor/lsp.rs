//! Native language-server support: one stdio LSP connection per
//! `(checkout, server)` behind [`app_core::lsp::LspHub`], driven entirely
//! from the poll loop — no work ever runs on the UI thread beyond queueing.
//!
//! Scope (decision D1, 2026-11-09): hover and go-to-definition only.
//! Diagnostics and completion are deferred; the transport and sync layers
//! already carry everything they will need.
//!
//! Layout:
//! - [`pos`] — UTF-16 position conversion and the incremental-sync diff.
//! - [`sync`] — [`DocSync`] document-mirror state.
//! - this file — configuration merge, the runtime thread + command/reply
//!   channels, document-sync orchestration, hover, definition, and the
//!   Language Servers section view.
//!
//! Ownership: exactly one runtime thread (created in [`Lsp::new`]) hosts a
//! current-thread Tokio runtime and the hub; the UI side only sends
//! [`LspCmd`]s and drains [`LspMsg`]s. Requests carry a UI-side timeout —
//! dropping the hub future removes the transport pending entry and emits
//! `$/cancelRequest`, so a timed-out or superseded request truly frees its
//! slot even when the server ignores cancellation.

mod pos;
mod sync;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc as std_mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use app_core::lsp::{
    self, LspEvent, LspHub, LspKey, LspSpec, LspState, LspStatus, SaveSupport, SyncChange,
};
use rune::{El, Tx, col, row};
use rune_kit::Kit;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::{AbortHandle, JoinSet};

use crate::editor::backend::{Backend, Job, Req, enqueue};
use crate::editor::code::GoToRequest;
use crate::editor::state::{EditorState, Notice, Section};
use sync::DocSync;

/// Settings-store key holding the `lsp.servers` JSON document.
pub const SERVERS_KEY: &str = "lsp.servers";

/// Documents larger than this never `didOpen` (wire-size guard).
const MAX_DOC_BYTES: usize = 1 << 20;
/// Quiet window after the last edit before `didChange` is sent.
const CHANGE_DEBOUNCE: Duration = Duration::from_millis(120);
/// Caret/revision settle time before a hover request is issued.
const HOVER_DEBOUNCE: Duration = Duration::from_millis(400);
/// Local hover timeout; frees the slot and cancels even if the server
/// ignores `$/cancelRequest`.
const HOVER_TIMEOUT: Duration = Duration::from_secs(5);
/// Local definition timeout (larger: project-wide lookups are slower).
const DEFINITION_TIMEOUT: Duration = Duration::from_secs(15);
/// Hover card caps: plaintext only, 4000 chars / 20 lines.
const HOVER_MAX_CHARS: usize = 4000;
const HOVER_MAX_LINES: usize = 20;
/// Bound on queued UI→runtime commands (~30 s of debounced traffic).
const CMD_QUEUE: usize = 256;
/// Bound on queued runtime→UI messages.
const MSG_QUEUE: usize = 256;
/// Status snapshot cadence on the runtime thread.
const STATUS_TICK: Duration = Duration::from_millis(250);
/// Bound on concurrent runtime request tasks.
const MAX_INFLIGHT: usize = 8;
/// `lsp.servers` reads retried after a settings-store failure.
const MAX_CONFIG_ATTEMPTS: u8 = 3;
/// Minimum spacing between `Ensure` commands for one key. The hub's own
/// `retry_at` backoff/cooldown still gates actual spawns — this only keeps
/// the UI from re-ensuring a dead connection every poll.
const ENSURE_RETRY: Duration = Duration::from_secs(1);
/// Marker element key for the caret-anchored hover card.
const HOVER_ANCHOR: &str = "goko-lsp-hover-anchor";

// ---------------------------------------------------------------------------
// Server configuration
// ---------------------------------------------------------------------------

/// One merged server entry: a builtin preset possibly overridden by
/// `lsp.servers`, or a custom server defined there.
#[derive(Clone)]
pub struct ServerEntry {
    /// `LspSpec::id` — also the `LspKey::server` half.
    id: String,
    spec: LspSpec,
    /// Builtin preset (not user-defined).
    preset: bool,
    /// The effective command line, for display and override editing.
    command: String,
}

/// A raw `lsp.servers[].*` entry as stored in settings (round-trips
/// untouched so UI edits preserve unknown fields).
#[derive(Clone, Debug)]
struct ServerOverride {
    raw: Value,
    id: String,
}

fn presets() -> Vec<ServerEntry> {
    let mk = |id: &str, command: &str, languages: &[(&str, &str)]| ServerEntry {
        id: id.to_owned(),
        spec: LspSpec {
            id: id.to_owned(),
            argv: app_core::lsp::parse_argv(command).unwrap_or_default(),
            env: Vec::new(),
            disabled: false,
            languages: languages
                .iter()
                .map(|(e, l)| (e.to_string(), l.to_string()))
                .collect(),
        },
        preset: true,
        command: command.to_owned(),
    };
    vec![
        mk("rust", "rust-analyzer", &[("rs", "rust")]),
        mk("go", "gopls", &[("go", "go")]),
        mk(
            "python",
            "pyright-langserver --stdio",
            &[("py", "python"), ("pyi", "python"), ("py3", "python")],
        ),
        mk(
            "typescript",
            "typescript-language-server --stdio",
            &[
                ("js", "javascript"),
                ("mjs", "javascript"),
                ("cjs", "javascript"),
                ("jsx", "javascriptreact"),
                ("ts", "typescript"),
                ("mts", "typescript"),
                ("cts", "typescript"),
                ("tsx", "typescriptreact"),
            ],
        ),
        mk(
            "html",
            "vscode-html-language-server --stdio",
            &[("html", "html"), ("htm", "html")],
        ),
        mk(
            "css",
            "vscode-css-language-server --stdio",
            &[("css", "css")],
        ),
    ]
}

/// Validate one settings entry; returns the override or a problem string.
fn parse_override(value: &Value) -> Result<ServerOverride, String> {
    let obj = value.as_object().ok_or("server entry is not an object")?;
    let id = obj
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if id.is_empty()
        || id.len() > 32
        || !id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!("bad server id '{id}'"));
    }
    if let Some(argv) = obj.get("argv").and_then(Value::as_str) {
        app_core::lsp::parse_argv(argv).map_err(|e| format!("{id}: {e}"))?;
    }
    if let Some(langs) = obj.get("languages") {
        let map = langs
            .as_object()
            .ok_or_else(|| format!("{id}: languages is not an object"))?;
        if map.len() > 64 {
            return Err(format!("{id}: too many language mappings"));
        }
        for (ext, lang) in map {
            if ext.is_empty()
                || ext.len() > 16
                || !ext.chars().all(|c| c.is_ascii_alphanumeric())
                || lang.as_str().is_none_or(|l| l.is_empty() || l.len() > 64)
            {
                return Err(format!("{id}: bad language mapping '{ext}'"));
            }
        }
    }
    Ok(ServerOverride {
        raw: value.clone(),
        id,
    })
}

/// Parse the `lsp.servers` settings JSON into overrides + merged entries.
/// Invalid entries are skipped with a collected warning (never fatal).
fn merge_config(value: Option<&str>) -> (Vec<ServerEntry>, Vec<ServerOverride>, Option<String>) {
    let mut entries = presets();
    let mut overrides = Vec::new();
    let mut error = None;
    let Some(text) = value else {
        return (entries, overrides, None);
    };
    if text.len() > 64 * 1024 {
        return (
            entries,
            overrides,
            Some("lsp.servers exceeds 64 KiB".into()),
        );
    }
    let doc: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => return (entries, overrides, Some(format!("lsp.servers: {e}"))),
    };
    let servers = doc
        .get("servers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if servers.len() > 32 {
        return (
            entries,
            overrides,
            Some("lsp.servers lists more than 32 servers".into()),
        );
    }
    for raw in &servers {
        match parse_override(raw) {
            Ok(over) => overrides.push(over),
            Err(why) => {
                if error.is_none() {
                    error = Some(why);
                }
            }
        }
    }
    for over in &overrides {
        let obj = over.raw.as_object().cloned().unwrap_or_default();
        let idx = entries.iter().position(|e| e.id == over.id);
        match idx {
            Some(i) => {
                // Same-id override: present fields replace, absent keep.
                let entry = &mut entries[i];
                if let Some(argv) = obj.get("argv").and_then(Value::as_str)
                    && let Ok(parsed) = app_core::lsp::parse_argv(argv)
                {
                    entry.spec.argv = parsed;
                    entry.command = argv.to_owned();
                }
                if let Some(disabled) = obj.get("disabled").and_then(Value::as_bool) {
                    entry.spec.disabled = disabled;
                }
                if let Some(langs) = obj.get("languages").and_then(Value::as_object) {
                    entry.spec.languages = langs
                        .iter()
                        .filter_map(|(e, l)| l.as_str().map(|l| (e.clone(), l.to_owned())))
                        .collect();
                }
            }
            None => {
                // Custom server: argv + languages are both required.
                let (Some(argv), Some(langs)) = (
                    obj.get("argv").and_then(Value::as_str),
                    obj.get("languages").and_then(Value::as_object),
                ) else {
                    if error.is_none() {
                        error = Some(format!(
                            "{}: custom servers need argv and languages",
                            over.id
                        ));
                    }
                    continue;
                };
                let Ok(parsed) = app_core::lsp::parse_argv(argv) else {
                    continue;
                };
                entries.push(ServerEntry {
                    id: over.id.clone(),
                    spec: LspSpec {
                        id: over.id.clone(),
                        argv: parsed,
                        env: Vec::new(),
                        disabled: obj.get("disabled").and_then(Value::as_bool) == Some(true),
                        languages: langs
                            .iter()
                            .filter_map(|(e, l)| l.as_str().map(|l| (e.clone(), l.to_owned())))
                            .collect(),
                    },
                    preset: false,
                    command: argv.to_owned(),
                });
            }
        }
    }
    (entries, overrides, error)
}

// ---------------------------------------------------------------------------
// Runtime-thread plumbing
// ---------------------------------------------------------------------------

/// UI → runtime commands (bounded tokio channel; `try_send` — a full
/// queue must never block the UI thread, and sync callers commit state
/// only on successful enqueue).
#[derive(Debug)]
enum LspCmd {
    Ensure {
        key: LspKey,
        spec: LspSpec,
        root: PathBuf,
    },
    DidOpen {
        key: LspKey,
        params: Value,
        /// Count this open for hub eviction protection (first open only).
        counted: bool,
    },
    DidChange {
        key: LspKey,
        params: Value,
    },
    DidSave {
        key: LspKey,
        params: Value,
    },
    DidClose {
        key: LspKey,
        params: Value,
        /// Decrement eviction protection only if the open was counted.
        counted: bool,
    },
    Request {
        key: LspKey,
        method: &'static str,
        params: Value,
        id: u64,
        timeout: Duration,
    },
    /// Abort one in-flight request task (drops the future → transport
    /// pending entry removed + `$/cancelRequest` sent).
    CancelReq {
        id: u64,
    },
    /// Abort every in-flight request for a connection, then disconnect.
    Disconnect {
        key: LspKey,
    },
    Shutdown,
}

/// Runtime → UI messages (bounded std channel; senders `try_send` and
/// drop on full — statuses re-publish every tick and replies are capped
/// by `MAX_INFLIGHT`, so a full queue only means the UI is far behind).
enum LspMsg {
    Event(LspEvent),
    Statuses(BTreeMap<LspKey, LspStatus>),
    Reply {
        id: u64,
        key: LspKey,
        result: Result<Value, String>,
    },
}

/// Runtime→UI delivery: the bounded channel stays the transport, and a
/// successful enqueue wakes the event loop via [`rune::AppHandle::wake`]
/// so `AppSpec::poll` drains the message without waiting for input — the
/// same wake contract `Backend` replies use. Status snapshots dedupe
/// against the last sent map: the 250 ms tick only wakes the app when a
/// status actually changed, so an idle runtime costs nothing.
#[derive(Clone)]
struct UiLink {
    tx: std_mpsc::SyncSender<LspMsg>,
    wake: Option<rune::AppHandle<crate::State>>,
    last_status: std::sync::Arc<std::sync::Mutex<Option<BTreeMap<LspKey, LspStatus>>>>,
}

impl UiLink {
    fn send(&self, msg: LspMsg) {
        if self.tx.try_send(msg).is_ok()
            && let Some(wake) = &self.wake
        {
            wake.wake();
        }
    }

    /// Publish the hub's status snapshot, skipping unchanged maps.
    fn status(&self, hub: &LspHub) {
        let map = hub.status();
        {
            let mut last = self.last_status.lock().unwrap();
            if last.as_ref() == Some(&map) {
                return;
            }
            *last = Some(map.clone());
        }
        self.send(LspMsg::Statuses(map));
    }
}

/// The single runtime thread: current-thread Tokio runtime + hub +
/// command loop. Exits on [`LspCmd::Shutdown`] or channel close.
fn runtime_main(
    ready: std_mpsc::Sender<mpsc::Sender<LspCmd>>,
    ui: std_mpsc::SyncSender<LspMsg>,
    wake: Option<rune::AppHandle<crate::State>>,
) {
    let (tx, rx) = mpsc::channel::<LspCmd>(CMD_QUEUE);
    if ready.send(tx).is_err() {
        return;
    }
    let link = UiLink {
        tx: ui,
        wake,
        last_status: std::sync::Arc::new(std::sync::Mutex::new(None)),
    };
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(_) => return,
    };
    rt.block_on(async move {
        let hub = LspHub::new(tokio::runtime::Handle::current());
        let (ev_tx, mut ev_rx) = mpsc::channel::<LspEvent>(256);
        if let Some(mut events) = hub.take_events() {
            tokio::spawn(async move {
                while let Some(ev) = events.recv().await {
                    if ev_tx.send(ev).await.is_err() {
                        break;
                    }
                }
            });
        }
        {
            let link = link.clone();
            tokio::spawn(async move {
                while let Some(ev) = ev_rx.recv().await {
                    link.send(LspMsg::Event(ev));
                }
            });
        }
        let mut rx = rx;
        // In-flight request tasks. The `JoinSet` reaps every task that
        // finishes on its own (reply sent, timeout, transport error) so a
        // normally-completed request frees its `MAX_INFLIGHT` slot —
        // `inflight` only mirrors ids still needing cancel/disconnect.
        let mut inflight: HashMap<u64, (LspKey, AbortHandle)> = HashMap::new();
        let mut tasks = JoinSet::new();
        let mut tick = tokio::time::interval(STATUS_TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                // `join_next` resolves `None` immediately on an empty set,
                // so the branch is gated on `!tasks.is_empty()`.
                joined = tasks.join_next(), if !tasks.is_empty() => {
                    if let Some(Ok(id)) = joined {
                        inflight.remove(&id);
                    }
                    // Err = a task we aborted (CancelReq/Disconnect already
                    // removed its entry) or a panic (bounded leak, no unwraps
                    // on server data make this effectively unreachable).
                }
                cmd = rx.recv() => {
                    let Some(cmd) = cmd else { break };
                    match cmd {
                        LspCmd::Shutdown => break,
                        LspCmd::Ensure { key, spec, root } => {
                            let hub = hub.clone();
                            let link = link.clone();
                            tokio::spawn(async move {
                                let _ = hub.ensure(&key, &spec, &root).await;
                                link.status(&hub);
                            });
                        }
                        LspCmd::DidOpen { key, params, counted } => {
                            if counted {
                                hub.doc_opened(&key);
                            }
                            hub.notify(&key, "textDocument/didOpen", params);
                        }
                        LspCmd::DidChange { key, params } => {
                            hub.notify(&key, "textDocument/didChange", params);
                        }
                        LspCmd::DidSave { key, params } => {
                            hub.notify(&key, "textDocument/didSave", params);
                        }
                        LspCmd::DidClose { key, params, counted } => {
                            hub.notify(&key, "textDocument/didClose", params);
                            if counted {
                                hub.doc_closed(&key);
                            }
                        }
                        LspCmd::Request { key, method, params, id, timeout } => {
                            if inflight.len() >= MAX_INFLIGHT {
                                link.send(LspMsg::Reply {
                                    id,
                                    key,
                                    result: Err("too many language-server requests".into()),
                                });
                                continue;
                            }
                            let hub = hub.clone();
                            let link = link.clone();
                            let map_key = key.clone();
                            let abort = tasks.spawn(async move {
                                let result = match tokio::time::timeout(
                                    timeout,
                                    hub.request(&key, method, params),
                                )
                                .await
                                {
                                    Ok(Ok(v)) => Ok(v),
                                    Ok(Err(e)) => Err(e.to_string()),
                                    Err(_) => Err("timeout".to_owned()),
                                };
                                link.send(LspMsg::Reply { id, key, result });
                                id
                            });
                            inflight.insert(id, (map_key, abort));
                        }
                        LspCmd::CancelReq { id } => {
                            if let Some((_, abort)) = inflight.remove(&id) {
                                abort.abort();
                            }
                        }
                        LspCmd::Disconnect { key } => {
                            let doomed: Vec<u64> = inflight
                                .iter()
                                .filter(|(_, (k, _))| *k == key)
                                .map(|(id, _)| *id)
                                .collect();
                            for id in doomed {
                                if let Some((_, abort)) = inflight.remove(&id) {
                                    abort.abort();
                                }
                            }
                            let hub = hub.clone();
                            let link = link.clone();
                            tokio::spawn(async move {
                                hub.disconnect(&key).await;
                                link.status(&hub);
                            });
                        }
                    }
                }
                _ = tick.tick() => {
                    link.status(&hub);
                }
            }
        }
        tasks.shutdown().await;
        hub.shutdown().await;
    });
}

// ---------------------------------------------------------------------------
// LSP state owned by EditorState
// ---------------------------------------------------------------------------

/// Hover request snapshot + lifecycle.
#[derive(Clone)]
struct HoverPending {
    id: u64,
    key: LspKey,
    epoch: u64,
    path: String,
    revision: u64,
    caret: usize,
}

/// Definition request snapshot; `req` is kept for the heuristic fallback.
struct DefPending {
    id: u64,
    key: LspKey,
    epoch: u64,
    path: String,
    revision: u64,
    req: GoToRequest,
}

/// What the open document should sync to, recomputed every poll.
struct DocTarget {
    key: LspKey,
    root: PathBuf,
    uri: String,
    path: String,
    language_id: String,
}

/// The LSP subsystem state: transport handles, merged config, the one
/// mirrored document, request snapshots and the hover card.
pub struct Lsp {
    tx: Option<mpsc::Sender<LspCmd>>,
    thread: Option<JoinHandle<()>>,
    rx: std_mpsc::Receiver<LspMsg>,
    /// Event-loop handle captured from `EditorState.app` on each poll so
    /// the runtime can wake `AppSpec::poll` when it queues a message —
    /// without it replies wait for the next input event.
    wake: Option<rune::AppHandle<crate::State>>,
    /// Latest hub status snapshot (polled every ~250 ms).
    pub statuses: BTreeMap<LspKey, LspStatus>,
    servers: Vec<ServerEntry>,
    /// Raw settings entries for UI round-trips.
    overrides: Vec<ServerOverride>,
    config_loaded: bool,
    config_pending: bool,
    /// Failed `lsp.servers` reads so far; the fetch retries at most
    /// [`MAX_CONFIG_ATTEMPTS`] times so a broken store cannot spin.
    config_attempts: u8,
    /// First config parse problem, shown in the Servers section.
    pub config_error: Option<String>,
    /// Keys already `ensure`d this config generation.
    ensured: HashSet<LspKey>,
    /// Earliest instant another `Ensure` may be sent per key (paces the
    /// dead-connection re-ensure; the hub's `retry_at` does the real
    /// backoff/cooldown gating).
    ensure_after: HashMap<LspKey, Instant>,
    active: Option<DocSync>,
    next_req: u64,
    /// Command-override buffer for the Servers section; `command_edit_id`
    /// marks which server is being edited (`None` = not editing).
    command_edit: rune::edit::TextEdit,
    command_edit_id: Option<String>,
    /// Set when the runtime thread failed to spawn — sends stay no-ops.
    runtime_failed: bool,
    hover_pending: Option<HoverPending>,
    /// Plaintext card body (already capped) + the snapshot it belongs to.
    pub hover_card: Option<HoverCard>,
    /// Caret/revision settle anchor for the hover debounce:
    /// `(revision, caret, since)`.
    hover_since: Option<(u64, usize, Instant)>,
    /// Esc latch: `(revision, caret)` the card was dismissed at; suppresses
    /// re-trigger until either changes.
    hover_dismissed: Option<(u64, usize)>,
    /// `(epoch, path, revision, caret)` that already consumed its one
    /// hover request — a reply (card, empty, or error) ends the snapshot,
    /// so the poll never re-requests an unchanged caret.
    hover_served: Option<(u64, String, u64, usize)>,
    definition: Option<DefPending>,
    /// Resolved definition navigation, drained by `AppSpec::poll` through
    /// the dirty-draft guard.
    pending_nav: Option<crate::editor::navigation::PendingNav>,
    /// Heuristic go-to fallback queued by an empty/failed LSP reply.
    fallback_go_to: Option<GoToRequest>,
}

impl Lsp {
    /// Lazy init: no thread exists until the first command is sent, so
    /// `EditorState`s that never touch LSP (most tests) cost nothing.
    /// A runtime that fails to start leaves `tx` unset — every send
    /// becomes a no-op and the UI shows servers as Off/Error.
    pub fn new() -> Lsp {
        let (_dead, rx) = std_mpsc::channel::<LspMsg>();
        Lsp {
            tx: None,
            thread: None,
            rx,
            wake: None,
            runtime_failed: false,
            statuses: BTreeMap::new(),
            servers: presets(),
            overrides: Vec::new(),
            config_loaded: false,
            config_pending: false,
            config_attempts: 0,
            config_error: None,
            ensured: HashSet::new(),
            ensure_after: HashMap::new(),
            active: None,
            next_req: 0,
            hover_pending: None,
            hover_card: None,
            hover_since: None,
            hover_dismissed: None,
            hover_served: None,
            definition: None,
            pending_nav: None,
            fallback_go_to: None,
            command_edit: rune::edit::TextEdit::new(""),
            command_edit_id: None,
        }
    }

    /// Spawn the runtime thread on first use and learn its command
    /// sender. Idempotent; a spawn failure latches `runtime_failed`.
    fn ensure_tx(&mut self) {
        if self.tx.is_some() || self.runtime_failed {
            return;
        }
        let (ui_tx, ui_rx) = std_mpsc::sync_channel::<LspMsg>(MSG_QUEUE);
        let (ready_tx, ready_rx) = std_mpsc::channel::<mpsc::Sender<LspCmd>>();
        let wake = self.wake.clone();
        match std::thread::Builder::new()
            .name("goko-lsp".into())
            .spawn(move || runtime_main(ready_tx, ui_tx, wake))
        {
            Ok(thread) => {
                self.thread = Some(thread);
                self.rx = ui_rx;
                self.tx = ready_rx.recv().ok();
                if self.tx.is_none() {
                    self.runtime_failed = true;
                }
            }
            Err(_) => self.runtime_failed = true,
        }
    }

    /// Join the runtime thread after telling it to shut the hub down —
    /// process groups are killed inside `hub.shutdown()`. Even if the
    /// bounded queue refuses `Shutdown`, dropping the sender closes the
    /// channel and the runtime exits through the same path.
    pub fn shutdown(&mut self) {
        self.ensure_tx();
        if let Some(tx) = self.tx.take() {
            let _ = tx.try_send(LspCmd::Shutdown);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    /// Enqueue a command without blocking; `false` means the bounded
    /// queue is full or the runtime is gone — callers must not commit
    /// the state that assumed delivery.
    fn send(&mut self, cmd: LspCmd) -> bool {
        self.ensure_tx();
        self.tx.as_ref().is_some_and(|tx| tx.try_send(cmd).is_ok())
    }

    /// Test hook: capture outgoing commands so no runtime thread or real
    /// server process is ever spawned.
    #[cfg(test)]
    fn test_cmd_channel(&mut self) -> mpsc::Receiver<LspCmd> {
        let (tx, rx) = mpsc::channel(CMD_QUEUE);
        self.tx = Some(tx);
        rx
    }

    /// Extension (no dot, lowercased) → `(server entry, languageId)`.
    /// Independent of the syntax scanner so custom languages work without
    /// highlighting support. First matching server wins.
    fn server_for_ext(&self, ext: &str) -> Option<(&ServerEntry, &str)> {
        for entry in &self.servers {
            if entry.spec.disabled {
                continue;
            }
            if let Some(lang) = entry.spec.languages.get(ext) {
                return Some((entry, lang.as_str()));
            }
        }
        None
    }

    /// All merged servers for the Servers section (presets + customs).
    pub fn servers(&self) -> &[ServerEntry] {
        &self.servers
    }

    /// Apply a `lsp.servers` settings payload (called on GetSetting reply
    /// and optimistically on local edits). Rebuilds the merged table and
    /// drops `ensured` so changed specs re-handshake; connections for
    /// servers that vanished or became disabled are disconnected.
    pub fn apply_config(&mut self, value: Option<&str>) {
        let (servers, overrides, error) = merge_config(value);
        let old_specs: HashMap<String, LspSpec> = self
            .servers
            .iter()
            .map(|s| (s.id.clone(), s.spec.clone()))
            .collect();
        let new_ids: HashSet<String> = servers.iter().map(|s| s.id.clone()).collect();
        self.servers = servers;
        self.overrides = overrides;
        self.config_error = error;
        self.config_loaded = true;
        self.config_pending = false;
        self.config_attempts = 0;
        self.ensured.clear();
        self.ensure_after.clear();
        // Disconnect keys whose server vanished, became disabled, or had
        // its spawn spec (argv/env) change — a new command must respawn,
        // not keep serving the old process.
        let dead: Vec<LspKey> = self
            .statuses
            .keys()
            .filter(|k| {
                if !new_ids.contains(&k.server) {
                    return true;
                }
                let Some(entry) = self.servers.iter().find(|s| s.id == k.server) else {
                    return true;
                };
                if entry.spec.disabled {
                    return true;
                }
                old_specs
                    .get(&k.server)
                    .is_some_and(|old| old.argv != entry.spec.argv || old.env != entry.spec.env)
            })
            .cloned()
            .collect();
        for key in dead {
            self.send(LspCmd::Disconnect { key });
        }
        // If the active document's server is gone/disabled, the next sync
        // pass closes it (desired target recomputes to None).
    }

    /// Next UI request id.
    fn next_id(&mut self) -> u64 {
        self.next_req += 1;
        self.next_req
    }

    /// Issue a bounded request on the runtime thread. `None` means the
    /// command queue refused it — the caller must not register a pending
    /// request (hover/definition simply retry on a later poll).
    fn request(
        &mut self,
        key: LspKey,
        method: &'static str,
        params: Value,
        timeout: Duration,
    ) -> Option<u64> {
        let id = self.next_id();
        self.send(LspCmd::Request {
            key,
            method,
            params,
            id,
            timeout,
        })
        .then_some(id)
    }

    /// Cancel one in-flight UI request (Esc dismissal, superseded hover).
    pub fn cancel_request(&mut self, id: u64) {
        self.send(LspCmd::CancelReq { id });
    }
}

// ---------------------------------------------------------------------------
// Settings mutations (Servers section)
// ---------------------------------------------------------------------------

impl Lsp {
    /// Serialize the stored overrides back into the `lsp.servers` document.
    fn settings_json(&self) -> String {
        let servers: Vec<Value> = self.overrides.iter().map(|o| o.raw.clone()).collect();
        json!({ "servers": servers }).to_string()
    }

    /// Find or create the override entry for `id`.
    fn override_mut(&mut self, id: &str) -> &mut Value {
        if let Some(pos) = self.overrides.iter().position(|o| o.id == id) {
            return &mut self.overrides[pos].raw;
        }
        self.overrides.push(ServerOverride {
            raw: json!({ "id": id }),
            id: id.to_owned(),
        });
        &mut self.overrides.last_mut().expect("just pushed").raw
    }

    /// Persist the current overrides and re-apply them locally. `false`
    /// means the bounded job queue refused the write — the caller must
    /// roll its in-memory override mutation back (done by the callers).
    fn persist(&mut self, backend: &mut Backend) -> bool {
        let doc = self.settings_json();
        if !enqueue(
            backend,
            Job::project(Req::SetSetting {
                key: SERVERS_KEY.to_owned(),
                value: doc.clone(),
            }),
        ) {
            self.config_error =
                Some("could not save lsp.servers; the change was not persisted".into());
            return false;
        }
        self.apply_config(Some(doc.as_str()));
        true
    }

    /// The settings worker rejected a `lsp.servers` write — surface it.
    pub fn persist_failed(&mut self, error: String) {
        self.config_error = Some(format!("could not save lsp.servers: {error}"));
    }

    /// A `lsp.servers` read failed: un-pend the fetch so the bounded retry
    /// in `lsp_poll` can fire again, and show the reason.
    pub fn config_get_failed(&mut self, error: String) {
        self.config_pending = false;
        self.config_attempts += 1;
        self.config_error = Some(format!("could not load lsp.servers: {error}"));
    }

    /// Enable/disable a server (preset or custom) via `lsp.servers`.
    pub fn set_disabled(&mut self, backend: &mut Backend, id: &str, disabled: bool) {
        let saved = self.overrides.clone();
        let raw = self.override_mut(id);
        if let Some(obj) = raw.as_object_mut() {
            obj.insert("disabled".into(), json!(disabled));
        }
        if !self.persist(backend) {
            self.overrides = saved;
        }
    }

    /// Override a server's command line; validated with shlex (no shell).
    pub fn set_command(
        &mut self,
        backend: &mut Backend,
        id: &str,
        command: &str,
    ) -> Result<(), String> {
        let argv = app_core::lsp::parse_argv(command).map_err(|e| e.to_string())?;
        if argv.is_empty() {
            return Err("empty command".into());
        }
        let saved = self.overrides.clone();
        let raw = self.override_mut(id);
        if let Some(obj) = raw.as_object_mut() {
            obj.insert("argv".into(), json!(command));
        }
        if !self.persist(backend) {
            self.overrides = saved;
        }
        Ok(())
    }

    /// Drop a custom server's settings entry entirely.
    pub fn remove_custom(&mut self, backend: &mut Backend, id: &str) {
        let saved = self.overrides.clone();
        self.overrides.retain(|o| o.id != id);
        if !self.persist(backend) {
            self.overrides = saved;
        }
    }

    /// Disconnect + forget `ensure` for every connection of `id`; the next
    /// eligible document (or section refresh) respawns a fresh epoch.
    pub fn restart(&mut self, id: &str) {
        let keys: Vec<LspKey> = self
            .statuses
            .keys()
            .filter(|k| k.server == id)
            .cloned()
            .collect();
        for key in keys {
            self.ensured.remove(&key);
            self.ensure_after.remove(&key);
            self.send(LspCmd::Disconnect { key });
        }
    }
}

// ---------------------------------------------------------------------------
// Document sync + hover + definition (EditorState methods)
// ---------------------------------------------------------------------------

/// Hover card payload kept for the view.
pub struct HoverCard {
    pub text: String,
    pub revision: u64,
    pub caret: usize,
}

impl EditorState {
    /// `AppSpec::poll` entry: drain runtime messages, load config once,
    /// reconcile document sync, drive the hover debounce.
    pub fn lsp_poll(&mut self, tx: &mut Tx) -> bool {
        // Capture the event-loop handle for the runtime thread's wake —
        // `EditorState.app` is the same handle `Backend` replies use.
        self.lsp.wake = self.app.clone();
        let mut changed = false;
        while let Ok(msg) = self.lsp.rx.try_recv() {
            match msg {
                LspMsg::Event(ev) => changed |= self.lsp_event(ev),
                LspMsg::Statuses(map) => {
                    if self.lsp.statuses != map {
                        self.lsp.statuses = map;
                        changed = true;
                    }
                }
                LspMsg::Reply { id, key, result } => {
                    changed |= self.lsp_reply(id, &key, result, tx);
                }
            }
        }
        if !self.lsp.config_loaded
            && !self.lsp.config_pending
            && self.lsp.config_attempts < MAX_CONFIG_ATTEMPTS
            && self.project_id.is_some()
        {
            self.lsp.config_pending = true;
            if !enqueue(
                &mut self.backend,
                Job::project(Req::GetSetting {
                    key: SERVERS_KEY.to_owned(),
                }),
            ) {
                self.lsp.config_pending = false;
            }
        }
        changed |= self.lsp_sync_docs(tx);
        changed |= self.lsp_poll_hover(tx);
        changed
    }

    fn lsp_event(&mut self, ev: LspEvent) -> bool {
        match ev {
            LspEvent::Notification { method, params, .. } => {
                if method == "window/showMessage"
                    && let Some(text) = params.get("message").and_then(Value::as_str)
                {
                    self.notice = Some(Notice {
                        text: text.to_owned(),
                        stale: false,
                    });
                    return true;
                }
                false
            }
            // Diagnostics are deferred (D1); Closed is reflected by the
            // next status snapshot.
            _ => false,
        }
    }

    fn lsp_reply(
        &mut self,
        id: u64,
        key: &LspKey,
        result: Result<Value, String>,
        tx: &mut Tx,
    ) -> bool {
        if self.lsp.hover_pending.as_ref().is_some_and(|p| p.id == id) {
            let p = self.lsp.hover_pending.take().expect("checked");
            return self.lsp_hover_apply(&p, result);
        }
        if self.lsp.definition.as_ref().is_some_and(|d| d.id == id) {
            // DefPending is not Clone (GoToRequest is, but keep it simple):
            // take it out — the reply always resolves one way or another.
            let d = self.lsp.definition.take().expect("checked");
            let _ = key;
            return self.lsp_definition_apply(&d, result, tx);
        }
        false
    }

    // ----- document sync -----

    /// What the open document should mirror to, recomputed each poll.
    fn lsp_desired_doc(&self) -> Option<DocTarget> {
        let doc = self.doc.as_ref()?;
        if !doc.editable() {
            return None;
        }
        let checkout = self.checkout.as_ref()?;
        let ckey = self.checkout_key.as_ref()?;
        let ext = Path::new(&doc.path).extension()?.to_str()?.to_lowercase();
        let (entry, lang) = self.lsp.server_for_ext(&ext)?;
        if doc.code.edit.text.len() > MAX_DOC_BYTES {
            return None;
        }
        let uri = lsp::path_to_file_uri(&checkout.join(&doc.path)).ok()?;
        Some(DocTarget {
            key: LspKey::new(ckey.clone(), entry.id.clone()),
            root: checkout.clone(),
            uri,
            path: doc.path.clone(),
            language_id: lang.to_owned(),
        })
    }

    /// Reconcile the mirrored document with the desired target, then flush
    /// ordered notifications (open → change → save) for the active one.
    fn lsp_sync_docs(&mut self, tx: &mut Tx) -> bool {
        let mut changed = false;
        let desired = self.lsp_desired_doc();
        let mismatch = match (&self.lsp.active, &desired) {
            (Some(s), Some(t)) => !(s.key == t.key && s.path == t.path),
            (Some(_), None) | (None, Some(_)) => true,
            (None, None) => false,
        };
        if mismatch {
            if let Some(old) = self.lsp.active.take() {
                self.lsp_close_sync(&old);
                changed = true;
            }
            if let Some(t) = desired {
                self.lsp.active = Some(DocSync::new(t.key, t.root, t.uri, t.path, t.language_id));
                changed = true;
            }
        }
        changed |= self.lsp_flush_active(tx, false);
        changed
    }

    /// Send `didClose` for a departing document. The notify drops silently
    /// on a dead connection; the counted `doc_closed` still runs so hub
    /// eviction bookkeeping stays correct across respawns.
    fn lsp_close_sync(&mut self, sync: &DocSync) {
        let params = json!({ "textDocument": { "uri": sync.uri } });
        self.lsp.send(LspCmd::DidClose {
            key: sync.key.clone(),
            params,
            counted: sync.reported_open,
        });
    }

    /// Flush pending sync ops for the active document. Field-level borrows
    /// keep `self.lsp.active` mutable while `statuses`/`servers`/`doc` are
    /// read; commands go through a cloned sender so no whole-`Lsp` borrow
    /// is needed. `force` treats the change debounce as already due —
    /// explicit navigation ([`Self::lsp_try_definition`]) must not race it.
    fn lsp_flush_active(&mut self, tx: &mut Tx, force: bool) -> bool {
        let mut changed = false;
        // Spawn the runtime on first sync traffic, then clone the sender
        // so `self.lsp.active` can stay mutably borrowed below.
        self.lsp.ensure_tx();
        let cmd_tx = self.lsp.tx.clone();
        // `try_send` on the bounded queue: `false` means the command was
        // NOT delivered — callers must leave sync state uncommitted so
        // the next poll retries instead of desyncing the document.
        let send = |cmd: LspCmd| cmd_tx.as_ref().is_some_and(|tx| tx.try_send(cmd).is_ok());
        let Some(sync) = self.lsp.active.as_mut() else {
            return false;
        };
        // Snapshot the status first: a dead connection (Error/Off) must
        // release the `ensured` latch so the hub's own backoff/cooldown
        // gets re-armed by a fresh `ensure()` — otherwise a crashed or
        // cooldown-refused server could never respawn. Re-ensures are
        // paced by `ensure_after` so a permanently missing binary does
        // not spin the command queue (the hub still gates real spawns
        // through `retry_at`, and a cheap `program_available` check).
        let st_opt = self.lsp.statuses.get(&sync.key).cloned();
        if st_opt
            .as_ref()
            .is_some_and(|st| matches!(st.state, LspState::Error | LspState::Off))
        {
            self.lsp.ensured.remove(&sync.key);
        }
        let paced = self
            .lsp
            .ensure_after
            .get(&sync.key)
            .is_some_and(|at| Instant::now() < *at);
        // Ensure BEFORE the status gate: the hub lazily spawns and reports
        // progress through status snapshots, so gating on `Running` first
        // deadlocks bootstrap — no status → no ensure → no spawn → the
        // status stays empty forever.
        if !self.lsp.ensured.contains(&sync.key)
            && !paced
            && let Some(spec) = self
                .lsp
                .servers
                .iter()
                .find(|s| s.id == sync.key.server)
                .map(|s| s.spec.clone())
            && send(LspCmd::Ensure {
                key: sync.key.clone(),
                spec,
                root: sync.checkout.clone(),
            })
        {
            self.lsp.ensured.insert(sync.key.clone());
            self.lsp
                .ensure_after
                .insert(sync.key.clone(), Instant::now() + ENSURE_RETRY);
        }
        let Some(st) = st_opt else {
            return changed;
        };
        if st.state != LspState::Running {
            return changed;
        }
        if sync.epoch != st.epoch {
            sync.reset_for_epoch(st.epoch);
            changed = true;
        }
        let caps = st.features.sync;
        if !caps.open_close {
            // The server cannot track documents; hover/definition stay
            // gated on `sync.open` which never becomes true.
            return changed;
        }
        let Some(doc) = self.doc.as_ref() else {
            return changed;
        };
        if doc.path != sync.path {
            return changed;
        }
        // didOpen — once per epoch, carrying the live text. State commits
        // only when the bounded queue accepted the command.
        if !sync.open {
            let text = doc.code.edit.text.clone();
            let counted = !sync.reported_open;
            let delivered = send(LspCmd::DidOpen {
                key: sync.key.clone(),
                params: json!({
                    "textDocument": {
                        "uri": sync.uri,
                        "languageId": sync.language_id,
                        "version": 1,
                        "text": text,
                    }
                }),
                counted,
            });
            if delivered {
                sync.wire_version = 1;
                sync.open = true;
                sync.reported_open = true;
                sync.last_sent = doc.code.edit.text.clone();
                sync.revision_seen = doc.code.revision;
                changed = true;
            }
        } else {
            // didChange — debounced ~120 ms after the last revision bump.
            let revision = doc.code.revision;
            if sync.revision_seen != revision {
                sync.revision_seen = revision;
                sync.change_at = Some(Instant::now() + CHANGE_DEBOUNCE);
            }
            if let Some(due) = sync.change_at
                && (force || Instant::now() >= due)
            {
                let text = doc.code.edit.text.clone();
                let next_v = sync.wire_version + 1;
                let cmd = match caps.change {
                    SyncChange::Full => Some(LspCmd::DidChange {
                        key: sync.key.clone(),
                        params: json!({
                            "textDocument": {
                                "uri": sync.uri,
                                "version": next_v,
                            },
                            "contentChanges": [{ "text": text.clone() }],
                        }),
                    }),
                    SyncChange::Incremental => {
                        match pos::diff_change(&sync.last_sent, &text) {
                            Some((start, end, inserted)) => {
                                let (sl, sc) = pos::position_at(&sync.last_sent, start);
                                let (el, ec) = pos::position_at(&sync.last_sent, end);
                                let range_length = pos::utf16_len(&sync.last_sent[start..end]);
                                Some(LspCmd::DidChange {
                                    key: sync.key.clone(),
                                    params: json!({
                                        "textDocument": {
                                            "uri": sync.uri,
                                            "version": next_v,
                                        },
                                        "contentChanges": [{
                                            "range": {
                                                "start": { "line": sl, "character": sc },
                                                "end": { "line": el, "character": ec },
                                            },
                                            "rangeLength": range_length,
                                            "text": inserted,
                                        }],
                                    }),
                                })
                            }
                            None => {
                                // Revision moved without a text delta (e.g.
                                // programmatic load): keep the diff base
                                // honest without sending an empty change.
                                sync.last_sent = text.clone();
                                None
                            }
                        }
                    }
                    SyncChange::None => None,
                };
                match cmd {
                    Some(cmd) => {
                        if send(cmd) {
                            sync.wire_version = next_v;
                            sync.last_sent = text;
                            sync.change_at = None;
                            changed = true;
                        } else {
                            // Queue full: keep the diff base and retry
                            // after another debounce window.
                            sync.change_at = Some(Instant::now() + CHANGE_DEBOUNCE);
                        }
                    }
                    None => {
                        sync.change_at = None;
                        changed = true;
                    }
                }
            } else if let Some(due) = sync.change_at {
                tx.wake_after(due.saturating_duration_since(Instant::now()));
            }
            // didSave — only after every pending change has flushed so the
            // server sees save strictly after the text it saved.
            if sync.save_pending && sync.change_at.is_none() {
                let cmd = match caps.save {
                    SaveSupport::WithoutText => Some(LspCmd::DidSave {
                        key: sync.key.clone(),
                        params: json!({ "textDocument": { "uri": sync.uri } }),
                    }),
                    SaveSupport::WithText => Some(LspCmd::DidSave {
                        key: sync.key.clone(),
                        params: json!({
                            "textDocument": { "uri": sync.uri },
                            "text": doc.code.edit.text,
                        }),
                    }),
                    SaveSupport::None => None,
                };
                match cmd {
                    Some(cmd) => {
                        if send(cmd) {
                            sync.save_pending = false;
                            changed = true;
                        } // else: queue full → save_pending stays, retried
                    }
                    None => {
                        sync.save_pending = false;
                        changed = true;
                    }
                }
            }
        }
        changed
    }

    /// A completed save marks `didSave` pending — but only when the saved
    /// path is still the mirrored document (a doc switch mid-save must
    /// not tag the new doc's sync with a save it never wrote).
    pub fn lsp_note_saved(&mut self, path: &str) {
        if let Some(sync) = &mut self.lsp.active
            && sync.path == path
        {
            sync.save_pending = true;
        }
    }

    /// Servers-section controls (delegate to [`Lsp`] with the backend
    /// lane for settings persistence).
    pub fn lsp_set_disabled(&mut self, id: &str, disabled: bool) {
        self.lsp.set_disabled(&mut self.backend, id, disabled);
    }
    pub fn lsp_set_command(&mut self, id: &str, command: &str) -> Result<(), String> {
        self.lsp.set_command(&mut self.backend, id, command)
    }
    pub fn lsp_remove_custom(&mut self, id: &str) {
        self.lsp.remove_custom(&mut self.backend, id);
    }

    // ----- hover -----

    /// Snapshot needed for a hover request, when one is currently legal.
    /// Hover belongs to the Files code surface with the field focused —
    /// the same surface [`hover_layer`] can actually draw the card on, so
    /// a stale caret in Changes/History/Servers never fires a request.
    fn hover_target(&self) -> Option<(LspKey, u64, String, String, u64, usize)> {
        if self.section != Section::Files {
            return None;
        }
        let sync = self.lsp.active.as_ref()?;
        if !sync.open {
            return None;
        }
        let st = self.lsp.statuses.get(&sync.key)?;
        if st.state != LspState::Running || !st.features.hover {
            return None;
        }
        let doc = self.doc.as_ref()?;
        if doc.path != sync.path || !doc.editable() || !doc.code.edit.focused {
            return None;
        }
        Some((
            sync.key.clone(),
            sync.epoch,
            sync.uri.clone(),
            doc.path.clone(),
            doc.code.revision,
            doc.code.edit.focus,
        ))
    }

    /// Caret/revision settle → debounce → one bounded hover request.
    /// Replies apply only while path+revision+caret+epoch still match.
    fn lsp_poll_hover(&mut self, tx: &mut Tx) -> bool {
        let mut changed = false;
        let Some((key, epoch, uri, path, revision, caret)) = self.hover_target() else {
            // Ineligible: hide the card, keep any pending request to
            // stale-drop on arrival.
            if self.lsp.hover_card.take().is_some() {
                self.lsp.hover_since = None;
                self.lsp.hover_dismissed = None;
                changed = true;
            }
            return changed;
        };
        // A pending request for a different snapshot is superseded: cancel
        // it (frees the transport slot) and restart the settle window.
        if let Some(p) = &self.lsp.hover_pending
            && (p.revision != revision || p.caret != caret)
        {
            let id = p.id;
            self.lsp.hover_pending = None;
            self.lsp.cancel_request(id);
            self.lsp.hover_since = None;
        }
        // The shown card belongs to an older snapshot → hide it.
        if let Some(card) = &self.lsp.hover_card
            && (card.revision != revision || card.caret != caret)
        {
            self.lsp.hover_card = None;
            changed = true;
        }
        if self.lsp.hover_dismissed == Some((revision, caret)) {
            return changed;
        }
        if self.lsp.hover_pending.is_some() {
            return changed;
        }
        // One request per settled snapshot: the reply — card, empty, or
        // error — already answered this (epoch, path, revision, caret);
        // only a caret move, edit, doc switch, or server respawn earns
        // another request. Without this latch every poll after a reply
        // fired a new request forever.
        if self
            .lsp
            .hover_served
            .as_ref()
            .is_some_and(|(e, p, r, c)| *e == epoch && *p == path && *r == revision && *c == caret)
        {
            return changed;
        }
        // Debounce: the settle anchor tracks (revision, caret); it resets
        // whenever either moves.
        let settled = match self.lsp.hover_since {
            Some((r, c, at)) if r == revision && c == caret => at,
            _ => {
                let at = Instant::now();
                self.lsp.hover_since = Some((revision, caret, at));
                at
            }
        };
        if settled.elapsed() < HOVER_DEBOUNCE {
            tx.wake_after(HOVER_DEBOUNCE - settled.elapsed());
            return changed;
        }
        // Issue the request at the caret's UTF-16 position.
        let (line, character) = self
            .doc
            .as_ref()
            .map(|d| pos::position_at(&d.code.edit.text, caret))
            .unwrap_or((0, 0));
        let Some(id) = self.lsp.request(
            key.clone(),
            "textDocument/hover",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character },
            }),
            HOVER_TIMEOUT,
        ) else {
            return changed; // queue refused: settle anchor keeps it retryable
        };
        self.lsp.hover_pending = Some(HoverPending {
            id,
            key,
            epoch,
            path: path.clone(),
            revision,
            caret,
        });
        self.lsp.hover_served = Some((epoch, path, revision, caret));
        changed
    }

    /// Apply a hover reply: only while the request snapshot is still the
    /// live document state; plaintext is capped before display.
    fn lsp_hover_apply(&mut self, p: &HoverPending, result: Result<Value, String>) -> bool {
        let current = self.doc.as_ref().is_some_and(|d| {
            d.path == p.path
                && d.code.revision == p.revision
                && d.code.edit.focus == p.caret
                && self
                    .lsp
                    .active
                    .as_ref()
                    .is_some_and(|s| s.key == p.key && s.epoch == p.epoch && s.path == p.path)
                && self.checkout_key.as_deref() == Some(p.key.checkout.as_str())
        });
        if !current {
            return false;
        }
        let Ok(value) = result else {
            return false;
        };
        match hover_plaintext(&value) {
            Some(text) => {
                self.lsp.hover_card = Some(HoverCard {
                    text,
                    revision: p.revision,
                    caret: p.caret,
                });
                true
            }
            None => false,
        }
    }

    /// Esc: dismiss the card or cancel the in-flight request (latched
    /// until caret/text moves) so the same snapshot never auto re-fires.
    /// Returns whether anything was dismissed.
    pub fn lsp_dismiss_hover(&mut self) -> bool {
        let mut changed = false;
        if let Some(p) = self.lsp.hover_pending.take() {
            self.lsp.cancel_request(p.id);
            changed = true;
        }
        let (revision, caret) = self
            .doc
            .as_ref()
            .map(|d| (d.code.revision, d.code.edit.focus))
            .unwrap_or((0, 0));
        if self.lsp.hover_card.take().is_some() {
            changed = true;
        }
        // Latch on either dismissal — a shown card OR a still-pending
        // request — otherwise Esc mid-flight let the reply's snapshot
        // re-request on the next poll.
        if changed {
            self.lsp.hover_dismissed = Some((revision, caret));
        }
        self.lsp.hover_since = None;
        changed
    }

    /// The card text for the view, only while it still matches the live
    /// document snapshot.
    pub fn lsp_hover_text(&self) -> Option<&str> {
        let card = self.lsp.hover_card.as_ref()?;
        let doc = self.doc.as_ref()?;
        if card.revision == doc.code.revision && card.caret == doc.code.edit.focus {
            Some(card.text.as_str())
        } else {
            None
        }
    }

    // ----- definition -----

    /// Try the LSP definition path for a [`GoToRequest`]. Returns `true`
    /// when the request was issued (reply resolves or falls back later);
    /// `false` lets the caller run the heuristic pipeline immediately.
    ///
    /// Explicit navigation force-flushes pending document sync first: a
    /// request sent while a `didChange` still sits in the 120 ms debounce
    /// would carry positions computed against text the server does not
    /// have yet. A change the bounded queue refused stays pending, so the
    /// caught-up check below refuses the request and the heuristic path
    /// answers instead.
    pub fn lsp_try_definition(&mut self, req: &GoToRequest, tx: &mut Tx) -> bool {
        // Cheap pre-gate: no mirror or no Running definition-capable
        // status means the heuristic path — without spawning the runtime
        // for a document that could never use it.
        {
            let Some(sync) = self.lsp.active.as_ref() else {
                return false;
            };
            let Some(st) = self.lsp.statuses.get(&sync.key) else {
                return false;
            };
            if st.state != LspState::Running || !st.features.definition {
                return false;
            }
        }
        self.lsp_flush_active(tx, true);
        // Post-flush gates on fresh state: the mirror must be open AND
        // fully caught up (`change_at` cleared means every pending edit
        // reached the wire; a refused send re-arms it).
        let (key, epoch, uri) = {
            let Some(sync) = self.lsp.active.as_ref() else {
                return false;
            };
            if !sync.open || sync.change_at.is_some() {
                return false;
            }
            (sync.key.clone(), sync.epoch, sync.uri.clone())
        };
        let Some(doc) = self.doc.as_ref() else {
            return false;
        };
        let sync_path = self
            .lsp
            .active
            .as_ref()
            .map(|s| s.path.as_str())
            .unwrap_or("");
        if doc.path != sync_path || !doc.editable() {
            return false;
        }
        let (path, revision) = (doc.path.clone(), doc.code.revision);
        let (line, character) = pos::position_at(&doc.code.edit.text, req.from);
        // Supersede: cancel the previous request before its slot is
        // replaced — a dropped pending entry would otherwise linger in
        // the runtime until its 15 s timeout.
        if let Some(old) = self.lsp.definition.take() {
            self.lsp.cancel_request(old.id);
        }
        let Some(id) = self.lsp.request(
            key.clone(),
            "textDocument/definition",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character },
            }),
            DEFINITION_TIMEOUT,
        ) else {
            return false; // queue refused → caller runs the heuristic path
        };
        self.lsp.definition = Some(DefPending {
            id,
            key,
            epoch,
            path,
            revision,
            req: req.clone(),
        });
        true
    }

    /// Resolve a definition reply: first in-checkout location wins and is
    /// queued as a guarded [`PendingNav`]; all-outside shows a notice;
    /// empty/error/timeout falls back to the heuristic pipeline via
    /// [`Self::lsp_fallback_go_to`] (drained by `AppSpec::poll`).
    fn lsp_definition_apply(
        &mut self,
        d: &DefPending,
        result: Result<Value, String>,
        _tx: &mut Tx,
    ) -> bool {
        let current = self.doc.as_ref().is_some_and(|doc| {
            doc.path == d.path
                && doc.code.revision == d.revision
                && self
                    .lsp
                    .active
                    .as_ref()
                    .is_some_and(|s| s.key == d.key && s.epoch == d.epoch && s.path == d.path)
                && self.checkout_key.as_deref() == Some(d.key.checkout.as_str())
        });
        if !current {
            return false;
        }
        let locations = result
            .as_ref()
            .ok()
            .map(definition_locations)
            .unwrap_or_default();
        for (uri, line) in &locations {
            if let Some(rel) = self.uri_inside_checkout(uri) {
                self.lsp.pending_nav = Some(crate::editor::navigation::PendingNav::OpenDoc {
                    path: rel,
                    line: Some(*line as u64 + 1),
                });
                return true;
            }
        }
        if !locations.is_empty() {
            self.notice = Some(Notice {
                text: "definition is outside the checkout".to_owned(),
                stale: false,
            });
            return true;
        }
        // Empty result, error, or timeout → heuristic fallback.
        self.lsp.fallback_go_to = Some(d.req.clone());
        true
    }

    /// Drain a resolved definition navigation for `AppSpec::poll`.
    pub fn lsp_take_nav(&mut self) -> Option<crate::editor::navigation::PendingNav> {
        self.lsp.pending_nav.take()
    }

    /// Drain a heuristic fallback request (LSP answered empty/failed).
    pub fn lsp_take_fallback(&mut self) -> Option<GoToRequest> {
        self.lsp.fallback_go_to.take()
    }

    /// Decode a `file://` URI and require the target inside the active
    /// checkout (canonicalized; `..` escapes rejected lexically first).
    fn uri_inside_checkout(&self, uri: &str) -> Option<String> {
        let checkout = self.checkout.as_ref()?;
        let target = lsp::file_uri_to_path(uri).ok()?;
        let normalized = normalize_lexical(&target);
        let resolved = normalized.canonicalize().unwrap_or(normalized);
        // Canonicalize the root the same way: a checkout reached through a
        // symlink (/tmp → /private/tmp) must still match, while a symlink
        // inside the checkout that escapes resolves outside and is rejected.
        let root = checkout.canonicalize().unwrap_or_else(|_| checkout.clone());
        let rel = resolved.strip_prefix(&root).ok()?;
        let mut parts = Vec::new();
        for c in rel.components() {
            match c {
                Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
                _ => return None,
            }
        }
        if parts.is_empty() {
            return None;
        }
        Some(parts.join("/"))
    }
}

/// Lexically resolve `.`/`..` without touching the filesystem.
fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Response parsing + hover card text
// ---------------------------------------------------------------------------

/// Extract plaintext from an LSP `Hover` result (`null` → `None`),
/// capped at 4000 chars / 20 lines.
fn hover_plaintext(value: &Value) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let hover: lsp_types::Hover = serde_json::from_value(value.clone()).ok()?;
    let marked = |m: &lsp_types::MarkedString| match m {
        lsp_types::MarkedString::String(s) => s.clone(),
        lsp_types::MarkedString::LanguageString(ls) => {
            format!("```{}\n{}\n```", ls.language, ls.value)
        }
    };
    let raw = match &hover.contents {
        lsp_types::HoverContents::Scalar(m) => marked(m),
        lsp_types::HoverContents::Array(items) => {
            items.iter().map(marked).collect::<Vec<_>>().join("\n")
        }
        lsp_types::HoverContents::Markup(content) => content.value.clone(),
    };
    let capped = cap_hover(&raw);
    if capped.is_empty() {
        None
    } else {
        Some(capped)
    }
}

fn cap_hover(text: &str) -> String {
    let mut out = String::new();
    let mut lines = text.lines();
    let mut truncated = false;
    for (i, line) in lines.by_ref().take(HOVER_MAX_LINES).enumerate() {
        if out.len() + line.len() + 1 > HOVER_MAX_CHARS {
            truncated = true;
            break;
        }
        if i > 0 {
            out.push('\n');
        }
        out.push_str(line);
    }
    if lines.next().is_some() {
        truncated = true;
    }
    if truncated {
        out.push('\n');
        out.push('…');
    }
    out
}

/// Flatten a `textDocument/definition` result into `(uri, line)` pairs in
/// server order. Handles `Location`, `Location[]`, `LocationLink[]` and
/// `null`.
fn definition_locations(value: &Value) -> Vec<(String, u32)> {
    let Ok(resp) =
        serde_json::from_value::<Option<lsp_types::GotoDefinitionResponse>>(value.clone())
    else {
        return Vec::new();
    };
    let Some(resp) = resp else {
        return Vec::new();
    };
    match resp {
        lsp_types::GotoDefinitionResponse::Scalar(loc) => {
            vec![(loc.uri.to_string(), loc.range.start.line)]
        }
        lsp_types::GotoDefinitionResponse::Array(locs) => locs
            .into_iter()
            .map(|l| (l.uri.to_string(), l.range.start.line))
            .collect(),
        lsp_types::GotoDefinitionResponse::Link(links) => links
            .into_iter()
            .map(|l| (l.target_uri.to_string(), l.target_range.start.line))
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// Poll entry + Servers section view
// ---------------------------------------------------------------------------

/// `AppSpec::poll` hook for the LSP subsystem.
pub fn poll(ed: &mut EditorState, tx: &mut Tx) -> bool {
    ed.lsp_poll(tx)
}

/// Drain a resolved LSP definition navigation — `AppSpec::poll` routes it
/// through the same dirty-draft guard as every other navigation.
pub fn take_nav(ed: &mut EditorState) -> Option<crate::editor::navigation::PendingNav> {
    ed.lsp_take_nav()
}

/// Drain a heuristic go-to fallback queued by an empty/failed LSP reply.
pub fn take_fallback(ed: &mut EditorState) -> Option<GoToRequest> {
    ed.lsp_take_fallback()
}

/// Caret-anchored hover card for the root view. Returns `None` until
/// [`rune::Cx::caret`] has measured the code field's caret in window
/// points — the card never mounts on a guessed position. The marker +
/// card ride an out-of-flow wrapper so `anchored_fit` resolves the marker
/// key and handles below/flip/clamp itself.
pub fn hover_layer(
    s: &crate::State,
    cx: &rune::Cx<crate::State>,
    k: &Kit,
) -> Option<El> {
    let text = s.editor.lsp_hover_text()?;
    let caret = cx.caret(crate::editor::code::FIELD_KEY)?;
    let marker = rune::el()
        .absolute(caret[0], caret[1] + caret[3] + 2.0)
        .w(1.0)
        .h(1.0)
        .key(HOVER_ANCHOR);
    // The markdown root is `w_full`, which has no parent width to resolve
    // against inside a shrink-wrapped card — give it an explicit width sized
    // to the longest line (~8pt/char), clamped to the old 480pt cap.
    let longest = text.lines().map(|l| l.chars().count()).max().unwrap_or(0);
    let width = (longest as f32 * 8.0).clamp(160.0, 480.0);
    let body = col()
        .w(width + 16.0)
        .padding(8.0)
        .child(rune_kit::markdown::Markdown::new(text).build(k));
    let card = rune_kit::popover::raised(k, body, k.radius.md).anchored_fit(
        HOVER_ANCHOR,
        rune::Side::Bottom,
        rune::Align::Start,
        4.0,
    );
    Some(
        rune::el()
            .absolute(0.0, 0.0)
            .w(0.0)
            .h(0.0)
            .children([marker, card]),
    )
}

/// Language Servers section body: one row per merged server with its
/// effective command, extensions, live status, feature summary, and
/// enable/restart/command-override controls. Status comes from hub
/// snapshots; no worker reads are needed.
pub fn servers_view(s: &crate::State, ink: &crate::theme::Ink, k: &Kit) -> El {
    use crate::theme;
    let ed = &s.editor;
    let lsp = &ed.lsp;
    let mut body = col().gap(6.0).padding(12.0);
    body = body.child(theme::txt("Language servers", theme::T14M, ink.text));
    if let Some(err) = &lsp.config_error {
        body = body.child(theme::txt(err, theme::T11M_LH, ink.muted));
    }
    if lsp.runtime_failed {
        body = body.child(theme::txt(
            "the language-server runtime failed to start",
            theme::T11M_LH,
            ink.muted,
        ));
    }
    if lsp.servers().is_empty() {
        body = body.child(theme::txt("No servers configured.", theme::T12, ink.muted));
    }
    for entry in lsp.servers() {
        body = body.child(server_row(s, ink, k, entry));
    }
    body
}

fn server_row(s: &crate::State, ink: &crate::theme::Ink, k: &Kit, entry: &ServerEntry) -> El {
    use crate::theme;
    let ed = &s.editor;
    // Aggregate status across this checkout's connections for the server.
    let (state, message, features) = ed
        .checkout_key
        .as_ref()
        .and_then(|ck| {
            ed.lsp
                .statuses
                .get(&LspKey::new(ck.clone(), entry.id.clone()))
        })
        .map(|st| (st.state, st.message.clone(), Some(st.features)))
        .unwrap_or((LspState::Off, None, None));
    let state_label = match state {
        LspState::Off => "Off".to_owned(),
        LspState::Starting => "Starting".to_owned(),
        LspState::Running => "Running".to_owned(),
        LspState::Error => format!(
            "Error{}",
            message
                .as_deref()
                .map(|m| format!(": {m}"))
                .unwrap_or_default()
        ),
    };
    let langs: Vec<&str> = entry.spec.languages.values().map(String::as_str).collect();
    let mut langs = langs;
    langs.sort_unstable();
    langs.dedup();
    let feature_summary = features
        .map(|f| {
            let mut parts = Vec::new();
            if f.hover {
                parts.push("hover");
            }
            if f.definition {
                parts.push("definition");
            }
            if f.sync.open_close {
                parts.push("sync");
            }
            if parts.is_empty() {
                "no features".to_owned()
            } else {
                parts.join(" · ")
            }
        })
        .unwrap_or_else(|| "—".to_owned());
    let enabled = !entry.spec.disabled;
    // Row: name + state, command, extensions, controls.
    let mut lines = col().gap(2.0).padding(6.0);
    lines = lines.child(
        row()
            .gap(8.0)
            .items_center()
            .child(theme::txt(&entry.id, theme::T12M, ink.text))
            .child(theme::txt(&state_label, theme::T11M_LH, ink.muted)),
    );
    lines = lines.child(theme::txt(&entry.command, theme::T11M_LH, ink.muted));
    let exts: Vec<String> = entry.spec.languages.keys().cloned().collect();
    lines = lines.child(theme::txt(
        &format!(".{} → {}", exts.join(", ."), langs.join(", ")),
        theme::T11M_LH,
        ink.muted,
    ));
    lines = lines.child(theme::txt(&feature_summary, theme::T11M_LH, ink.muted));
    // Command override: an inline single-line field + Save/Cancel.
    if ed.lsp.command_edit_id.as_deref() == Some(entry.id.as_str()) {
        let input = rune::text_input(&ed.lsp.command_edit, theme::T12)
            .single_line()
            .on_edit(|st: &mut crate::State| -> &mut rune::edit::TextEdit {
                &mut st.editor.lsp.command_edit
            });
        lines = lines.child(input);
        let mut edit_ctl = row().gap(8.0);
        let id_save = entry.id.clone();
        edit_ctl = edit_ctl.child(
            rune::el()
                .key(format!("lsp-cmd-save-{}", entry.id))
                .child(theme::txt("Save", theme::T11M_LH, ink.accent))
                .on_press(move |st: &mut crate::State| {
                    let text = st.editor.lsp.command_edit.text.clone();
                    match st.editor.lsp_set_command(&id_save, &text) {
                        Ok(()) => st.editor.lsp.command_edit_id = None,
                        Err(e) => {
                            st.editor.notice = Some(Notice {
                                text: e,
                                stale: false,
                            });
                        }
                    }
                }),
        );
        edit_ctl = edit_ctl.child(
            rune::el()
                .key(format!("lsp-cmd-cancel-{}", entry.id))
                .child(theme::txt("Cancel", theme::T11M_LH, ink.muted))
                .on_press(|st: &mut crate::State| {
                    st.editor.lsp.command_edit_id = None;
                }),
        );
        lines = lines.child(edit_ctl);
        return lines;
    }
    // Controls: enable toggle + command edit + restart (custom removable).
    let mut controls = row().gap(8.0).items_center();
    let toggle_label = if enabled { "Disable" } else { "Enable" };
    let id_for_toggle = entry.id.clone();
    let id_for_edit = entry.id.clone();
    let command_for_edit = entry.command.clone();
    let id_for_restart = entry.id.clone();
    let id_for_remove = entry.id.clone();
    let preset = entry.preset;
    controls = controls.child(
        rune::el()
            .key(format!("lsp-toggle-{}", entry.id))
            .child(theme::txt(toggle_label, theme::T11M_LH, ink.accent))
            .on_press(move |st: &mut crate::State| {
                st.editor.lsp_set_disabled(&id_for_toggle, enabled);
            }),
    );
    controls = controls.child(
        rune::el()
            .key(format!("lsp-edit-{}", entry.id))
            .child(theme::txt("Edit cmd", theme::T11M_LH, ink.accent))
            .on_press(move |st: &mut crate::State| {
                st.editor.lsp.command_edit = rune::edit::TextEdit::new(command_for_edit.clone());
                st.editor.lsp.command_edit_id = Some(id_for_edit.clone());
            }),
    );
    if state != LspState::Off {
        controls = controls.child(
            rune::el()
                .key(format!("lsp-restart-{}", entry.id))
                .child(theme::txt("Restart", theme::T11M_LH, ink.accent))
                .on_press(move |st: &mut crate::State| {
                    st.editor.lsp.restart(&id_for_restart);
                }),
        );
    }
    if !preset {
        controls = controls.child(
            rune::el()
                .key(format!("lsp-remove-{}", entry.id))
                .child(theme::txt("Remove", theme::T11M_LH, ink.muted))
                .on_press(move |st: &mut crate::State| {
                    st.editor.lsp_remove_custom(&id_for_remove);
                }),
        );
    }
    let _ = k;
    lines.child(controls)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::draft::OpenDoc;
    use crate::editor::go_to_definition::GoToTarget;
    use crate::editor::state::checkout_key;
    use app_core::git_view::GitViewFile;
    use app_core::lsp::{LspFeatures, SyncSupport};

    fn file(text: &str) -> GitViewFile {
        GitViewFile {
            path: "a.rs".into(),
            kind: "text",
            text: Some(text.into()),
            truncated: false,
            size: text.len() as u64,
        }
    }

    fn with_doc(path: &str, text: &str) -> EditorState {
        let mut e = EditorState::new(rune_kit::Kit::neutral(true), crate::test_handle());
        e.project_id = Some("p1".into());
        e.checkout = Some(PathBuf::from("/tmp/co"));
        e.checkout_key = Some(checkout_key("p1", &PathBuf::from("/tmp/co")));
        e.doc = Some(OpenDoc::new(path, &file(text)));
        // Hover gates mirror the real view: Files section + focused field.
        e.section = Section::Files;
        e.doc.as_mut().unwrap().code.edit.focused = true;
        e.lsp.config_loaded = true; // skip the settings fetch
        e
    }

    /// Fabricate a Running status for the `rust` server so sync/hover/
    /// definition paths proceed without a real connection.
    fn running(e: &mut EditorState, hover: bool, definition: bool) {
        let key = LspKey::new(e.checkout_key.clone().unwrap(), "rust");
        e.lsp.statuses.insert(
            key.clone(),
            LspStatus {
                state: LspState::Running,
                message: None,
                epoch: 1,
                features: LspFeatures {
                    completion: false,
                    hover,
                    definition,
                    sync: SyncSupport {
                        open_close: true,
                        change: SyncChange::Incremental,
                        save: SaveSupport::WithoutText,
                    },
                },
                stderr_tail: None,
            },
        );
        e.lsp.ensured.insert(key);
    }

    fn cmds(rx: &mut mpsc::Receiver<LspCmd>) -> Vec<LspCmd> {
        let mut out = Vec::new();
        while let Ok(c) = rx.try_recv() {
            out.push(c);
        }
        out
    }

    fn method_of(cmd: &LspCmd) -> Option<&'static str> {
        match cmd {
            LspCmd::DidOpen { .. } => Some("didOpen"),
            LspCmd::DidChange { .. } => Some("didChange"),
            LspCmd::DidSave { .. } => Some("didSave"),
            LspCmd::DidClose { .. } => Some("didClose"),
            LspCmd::Request { method, .. } => Some(method),
            _ => None,
        }
    }

    fn rust_key(e: &EditorState) -> LspKey {
        LspKey::new(e.checkout_key.clone().unwrap(), "rust")
    }

    // ----- configuration -----

    #[test]
    fn config_merges_presets_overrides_and_custom() {
        let mut e = with_doc("a.rs", "fn main() {}");
        e.lsp.apply_config(Some(
            r#"{"servers":[
                {"id":"rust","disabled":true},
                {"id":"custom-ls","argv":"my-ls --flag","languages":{"xyz":"mylang"}}
            ]}"#,
        ));
        assert!(e.lsp.config_error.is_none());
        let rust = e.lsp.servers.iter().find(|s| s.id == "rust").unwrap();
        assert!(rust.spec.disabled);
        let custom = e.lsp.servers.iter().find(|s| s.id == "custom-ls").unwrap();
        assert!(!custom.preset);
        assert_eq!(
            custom.spec.languages.get("xyz").map(String::as_str),
            Some("mylang")
        );
        // Custom extension resolves without any syntax-scanner support.
        let (entry, lang) = e.lsp.server_for_ext("xyz").unwrap();
        assert_eq!(entry.id, "custom-ls");
        assert_eq!(lang, "mylang");
    }

    #[test]
    fn config_rejects_bad_entries_but_keeps_presets() {
        let mut e = with_doc("a.rs", "fn main() {}");
        e.lsp.apply_config(Some("not json"));
        assert!(e.lsp.config_error.is_some());
        assert!(!e.lsp.servers.is_empty(), "presets survive a bad doc");
        e.lsp.apply_config(Some(
            r#"{"servers":[{"id":"broken"},{"id":"rust","argv":"   "}]}"#,
        ));
        // `broken` lacks argv+languages → skipped with an error note;
        // `rust` with an empty argv keeps its preset command.
        assert!(e.lsp.servers.iter().all(|s| s.id != "broken"));
        let rust = e.lsp.servers.iter().find(|s| s.id == "rust").unwrap();
        assert_eq!(
            rust.spec.argv.first().map(String::as_str),
            Some("rust-analyzer")
        );
    }

    #[test]
    fn ineligible_doc_never_opens() {
        let mut e = with_doc("README", "hello");
        running(&mut e, true, true);
        let mut rx = e.lsp.test_cmd_channel();
        let mut tx = Tx::default();
        e.lsp_poll(&mut tx);
        assert!(e.lsp.active.is_none());
        assert!(cmds(&mut rx).is_empty());
    }

    // ----- document sync -----

    #[test]
    fn open_then_debounced_incremental_change_then_save() {
        let mut e = with_doc("src/a.rs", "fn main() {}\n");
        running(&mut e, false, false);
        let mut rx = e.lsp.test_cmd_channel();
        let mut tx = Tx::default();
        e.lsp_poll(&mut tx);
        let got = cmds(&mut rx);
        assert_eq!(got.len(), 1);
        assert_eq!(method_of(&got[0]), Some("didOpen"));
        let sync = e.lsp.active.as_ref().unwrap();
        assert!(sync.open && sync.reported_open);
        assert_eq!(sync.wire_version, 1);
        assert_eq!(sync.language_id, "rust");

        // Edit: bump revision + text; the next poll arms the debounce.
        {
            let doc = e.doc.as_mut().unwrap();
            doc.code.edit.text = "fn main() { println!(\"hi\"); }\n".into();
            doc.code.revision += 1;
        }
        e.lsp_poll(&mut tx);
        assert!(cmds(&mut rx).is_empty(), "change still inside debounce");
        {
            let sync = e.lsp.active.as_mut().unwrap();
            sync.change_at = Some(Instant::now() - Duration::from_millis(1));
        }
        e.lsp_poll(&mut tx);
        let got = cmds(&mut rx);
        assert_eq!(got.len(), 1);
        assert_eq!(method_of(&got[0]), Some("didChange"));
        if let LspCmd::DidChange { params, .. } = &got[0] {
            let changes = params["contentChanges"].as_array().unwrap();
            assert_eq!(changes.len(), 1);
            assert!(
                changes[0].get("range").is_some(),
                "incremental change carries a range"
            );
            assert_eq!(params["textDocument"]["version"], 2);
        } else {
            panic!("expected didChange");
        }
        assert_eq!(e.lsp.active.as_ref().unwrap().wire_version, 2);

        // Save lands strictly after the flushed change.
        e.lsp_note_saved("src/a.rs");
        e.lsp_poll(&mut tx);
        let got = cmds(&mut rx);
        assert_eq!(got.len(), 1);
        assert_eq!(method_of(&got[0]), Some("didSave"));
        if let LspCmd::DidSave { params, .. } = &got[0] {
            assert!(params.get("text").is_none(), "WithoutText sends no text");
        } else {
            panic!("expected didSave");
        }
    }

    #[test]
    fn save_for_a_different_path_never_marks_did_save() {
        let mut e = with_doc("a.rs", "fn a() {}\n");
        running(&mut e, false, false);
        let mut rx = e.lsp.test_cmd_channel();
        let mut tx = Tx::default();
        e.lsp_poll(&mut tx);
        cmds(&mut rx);
        // A save that completed for another document must not arm didSave
        // on the mirrored one.
        e.lsp_note_saved("b.rs");
        e.lsp_poll(&mut tx);
        assert!(!e.lsp.active.as_ref().unwrap().save_pending);
        assert!(cmds(&mut rx).is_empty());
        // The matching path does arm it.
        e.lsp_note_saved("a.rs");
        e.lsp_poll(&mut tx);
        assert_eq!(method_of(&cmds(&mut rx)[0]), Some("didSave"));
    }

    #[test]
    fn doc_switch_closes_then_reopens_in_order() {
        let mut e = with_doc("a.rs", "fn a() {}");
        running(&mut e, false, false);
        let mut rx = e.lsp.test_cmd_channel();
        let mut tx = Tx::default();
        e.lsp_poll(&mut tx);
        cmds(&mut rx);
        // Switch documents.
        e.doc = Some(OpenDoc::new("b.rs", &file("fn b() {}")));
        e.lsp_poll(&mut tx);
        let got = cmds(&mut rx);
        let methods: Vec<_> = got.iter().map(method_of).collect();
        assert_eq!(methods, vec![Some("didClose"), Some("didOpen")]);
        assert_eq!(e.lsp.active.as_ref().unwrap().path, "b.rs");
    }

    #[test]
    fn closing_doc_sends_did_close() {
        let mut e = with_doc("a.rs", "fn a() {}");
        running(&mut e, false, false);
        let mut rx = e.lsp.test_cmd_channel();
        let mut tx = Tx::default();
        e.lsp_poll(&mut tx);
        cmds(&mut rx);
        e.doc = None;
        e.lsp_poll(&mut tx);
        let got = cmds(&mut rx);
        assert_eq!(got.len(), 1);
        assert_eq!(method_of(&got[0]), Some("didClose"));
        assert!(e.lsp.active.is_none());
    }

    #[test]
    fn epoch_change_reopens_document() {
        let mut e = with_doc("a.rs", "fn a() {}");
        running(&mut e, false, false);
        let mut rx = e.lsp.test_cmd_channel();
        let mut tx = Tx::default();
        e.lsp_poll(&mut tx);
        cmds(&mut rx);
        // Server crashed and respawned → epoch bump → re-open.
        let key = rust_key(&e);
        let st = e.lsp.statuses.get_mut(&key).unwrap();
        st.epoch = 2;
        e.lsp_poll(&mut tx);
        let got = cmds(&mut rx);
        assert_eq!(got.len(), 1);
        assert_eq!(method_of(&got[0]), Some("didOpen"));
        assert_eq!(e.lsp.active.as_ref().unwrap().epoch, 2);
    }

    // ----- hover -----

    fn settle_hover(e: &mut EditorState, focus: usize) {
        e.doc.as_mut().unwrap().code.edit.focus = focus;
        let (rev, caret) = {
            let d = e.doc.as_ref().unwrap();
            (d.code.revision, d.code.edit.focus)
        };
        e.lsp.hover_since = Some((
            rev,
            caret,
            Instant::now() - HOVER_DEBOUNCE - Duration::from_millis(1),
        ));
    }

    #[test]
    fn hover_debounces_then_requests_at_caret() {
        let mut e = with_doc("a.rs", "fn main() {}\nlet x = 1;\n");
        running(&mut e, true, false);
        let mut rx = e.lsp.test_cmd_channel();
        let mut tx = Tx::default();
        e.lsp_poll(&mut tx); // didOpen
        cmds(&mut rx);
        // Caret on `x` in `let x` — byte offset 17 (line 1 starts at 13).
        e.doc.as_mut().unwrap().code.edit.focus = 17;
        e.lsp_poll(&mut tx); // arms the settle window
        assert!(cmds(&mut rx).is_empty());
        settle_hover(&mut e, 17);
        e.lsp_poll(&mut tx);
        let got = cmds(&mut rx);
        assert_eq!(got.len(), 1);
        match &got[0] {
            LspCmd::Request { method, params, .. } => {
                assert_eq!(*method, "textDocument/hover");
                assert_eq!(params["textDocument"]["uri"], "file:///tmp/co/a.rs");
                // byte 20 → line 1, utf-16 col 4
                assert_eq!(params["position"]["line"], 1);
                assert_eq!(params["position"]["character"], 4);
            }
            _ => panic!("expected hover request"),
        }
    }

    #[test]
    fn hover_reply_applies_and_stale_is_dropped() {
        let mut e = with_doc("a.rs", "fn main() {}\n");
        running(&mut e, true, false);
        let mut rx = e.lsp.test_cmd_channel();
        let mut tx = Tx::default();
        e.lsp_poll(&mut tx);
        cmds(&mut rx);
        settle_hover(&mut e, 4);
        e.lsp_poll(&mut tx);
        let got = cmds(&mut rx);
        let req_id = match &got[0] {
            LspCmd::Request { id, .. } => *id,
            _ => panic!("expected request"),
        };
        let key = rust_key(&e);
        let rev = e.doc.as_ref().unwrap().code.revision;
        let caret = e.doc.as_ref().unwrap().code.edit.focus;
        // Stale reply (wrong revision) is ignored.
        e.lsp.hover_pending = Some(HoverPending {
            id: req_id,
            key: key.clone(),
            epoch: 1,
            path: "a.rs".into(),
            revision: rev + 99,
            caret,
        });
        let _ = e.lsp_reply(
            req_id,
            &key,
            Ok(json!({"contents":{"kind":"plaintext","value":"stale"}})),
            &mut tx,
        );
        assert!(e.lsp.hover_card.is_none());
        // Fresh pending + matching reply applies the card.
        e.lsp.hover_pending = Some(HoverPending {
            id: req_id,
            key: key.clone(),
            epoch: 1,
            path: "a.rs".into(),
            revision: rev,
            caret,
        });
        let _ = e.lsp_reply(
            req_id,
            &key,
            Ok(json!({"contents":{"kind":"plaintext","value":"fn main()"}})),
            &mut tx,
        );
        assert_eq!(e.lsp_hover_text(), Some("fn main()"));
    }

    #[test]
    fn hover_escape_latches_until_caret_moves() {
        let mut e = with_doc("a.rs", "fn main() {}\n");
        running(&mut e, true, false);
        let mut rx = e.lsp.test_cmd_channel();
        let mut tx = Tx::default();
        e.lsp_poll(&mut tx);
        cmds(&mut rx);
        settle_hover(&mut e, 4);
        e.lsp_poll(&mut tx);
        let got = cmds(&mut rx);
        let req_id = match &got[0] {
            LspCmd::Request { id, .. } => *id,
            _ => panic!("expected request"),
        };
        let key = rust_key(&e);
        let _ = e.lsp_reply(
            req_id,
            &key,
            Ok(json!({"contents":{"kind":"plaintext","value":"fn main()"}})),
            &mut tx,
        );
        assert!(e.lsp_hover_text().is_some());
        // Esc dismisses and latches: no new request at the same snapshot.
        assert!(e.lsp_dismiss_hover());
        assert!(e.lsp_hover_text().is_none());
        e.lsp_poll(&mut tx);
        assert!(cmds(&mut rx).is_empty());
        // Moving the caret releases the latch.
        settle_hover(&mut e, 5);
        e.lsp_poll(&mut tx);
        assert!(cmds(&mut rx).iter().any(
            |c| matches!(c, LspCmd::Request { method, .. } if *method == "textDocument/hover")
        ));
    }

    #[test]
    fn hover_requests_once_per_settled_snapshot() {
        let mut e = with_doc("a.rs", "fn main() {}\n");
        running(&mut e, true, false);
        let mut rx = e.lsp.test_cmd_channel();
        let mut tx = Tx::default();
        e.lsp_poll(&mut tx);
        cmds(&mut rx);
        settle_hover(&mut e, 4);
        e.lsp_poll(&mut tx);
        let got = cmds(&mut rx);
        let req_id = match &got[0] {
            LspCmd::Request { id, .. } => *id,
            _ => panic!("expected request"),
        };
        let key = rust_key(&e);
        // A successful reply ends the snapshot: repeated polls at the same
        // (epoch, path, revision, caret) must not fire another request —
        // the pre-fix code stormed one request per poll forever.
        let _ = e.lsp_reply(
            req_id,
            &key,
            Ok(json!({"contents":{"kind":"plaintext","value":"fn main()"}})),
            &mut tx,
        );
        assert!(e.lsp_hover_text().is_some());
        for _ in 0..3 {
            e.lsp_poll(&mut tx);
        }
        assert!(
            cmds(&mut rx).iter().all(
                |c| !matches!(c, LspCmd::Request { method, .. } if *method == "textDocument/hover")
            ),
            "unchanged snapshot re-requested hover"
        );
        // An error reply ends the snapshot the same way: a fresh caret
        // earns one request, its Err consumes it, and silence follows.
        settle_hover(&mut e, 5);
        e.lsp_poll(&mut tx);
        let got = cmds(&mut rx);
        let req_id = match &got[0] {
            LspCmd::Request { id, .. } => *id,
            _ => panic!("expected request"),
        };
        let _ = e.lsp_reply(req_id, &key, Err("boom".into()), &mut tx);
        for _ in 0..3 {
            e.lsp_poll(&mut tx);
        }
        assert!(
            cmds(&mut rx).iter().all(
                |c| !matches!(c, LspCmd::Request { method, .. } if *method == "textDocument/hover")
            ),
            "failed reply re-requested hover"
        );
    }

    #[test]
    fn hover_escape_during_pending_latches() {
        let mut e = with_doc("a.rs", "fn main() {}\n");
        running(&mut e, true, false);
        let mut rx = e.lsp.test_cmd_channel();
        let mut tx = Tx::default();
        e.lsp_poll(&mut tx);
        cmds(&mut rx);
        settle_hover(&mut e, 4);
        e.lsp_poll(&mut tx);
        let got = cmds(&mut rx);
        let req_id = match &got[0] {
            LspCmd::Request { id, .. } => *id,
            _ => panic!("expected request"),
        };
        // Esc while the request is still in flight — no card exists yet,
        // but the dismissal must still latch for this snapshot.
        assert!(e.lsp_dismiss_hover());
        assert!(e.lsp.hover_card.is_none());
        let (rev, caret) = {
            let d = e.doc.as_ref().unwrap();
            (d.code.revision, d.code.edit.focus)
        };
        assert_eq!(e.lsp.hover_dismissed, Some((rev, caret)));
        // The cancelled request's late reply is stale-dropped...
        let key = rust_key(&e);
        let _ = e.lsp_reply(
            req_id,
            &key,
            Ok(json!({"contents":{"kind":"plaintext","value":"late"}})),
            &mut tx,
        );
        assert!(e.lsp.hover_card.is_none());
        // ...and the same snapshot never re-fires on later polls.
        for _ in 0..3 {
            e.lsp_poll(&mut tx);
        }
        assert!(
            cmds(&mut rx).iter().all(
                |c| !matches!(c, LspCmd::Request { method, .. } if *method == "textDocument/hover")
            ),
            "pending-escape snapshot re-requested hover"
        );
    }

    #[test]
    fn hover_requires_files_section_and_focused_field() {
        let mut e = with_doc("a.rs", "fn main() {}\n");
        running(&mut e, true, false);
        let mut rx = e.lsp.test_cmd_channel();
        let mut tx = Tx::default();
        e.lsp_poll(&mut tx);
        cmds(&mut rx);
        settle_hover(&mut e, 4);
        // A settled caret outside the Files section fires nothing.
        e.section = Section::Changes;
        e.lsp_poll(&mut tx);
        assert!(cmds(&mut rx).is_empty(), "hover fired outside Files");
        e.section = Section::Files;
        // An unfocused code field fires nothing either.
        e.doc.as_mut().unwrap().code.edit.focused = false;
        e.lsp_poll(&mut tx);
        assert!(cmds(&mut rx).is_empty(), "hover fired without focus");
        // Focus returns: the still-settled snapshot requests normally.
        e.doc.as_mut().unwrap().code.edit.focused = true;
        e.lsp_poll(&mut tx);
        assert!(
            cmds(&mut rx).iter().any(
                |c| matches!(c, LspCmd::Request { method, .. } if *method == "textDocument/hover")
            ),
            "hover did not fire once Files+focus returned"
        );
    }

    #[test]
    fn ui_link_send_and_status_dedupe_with_wake_handle() {
        // The production path: `wake` is `Some` (test_handle queues the
        // wake harmlessly) and messages still reach the channel `poll`
        // drains — this is what lets a runtime reply apply without
        // another input event.
        let (tx, rx) = std_mpsc::sync_channel::<LspMsg>(MSG_QUEUE);
        let link = UiLink {
            tx,
            wake: Some(crate::test_handle()),
            last_status: Default::default(),
        };
        link.send(LspMsg::Statuses(BTreeMap::new()));
        assert!(matches!(rx.try_recv(), Ok(LspMsg::Statuses(_))));
        // Status snapshots dedupe: an unchanged map never re-sends, so
        // the 250 ms tick cannot wake the app while idle.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let hub = LspHub::new(rt.handle().clone());
        link.status(&hub);
        link.status(&hub);
        assert_eq!(rx.try_iter().count(), 1, "unchanged status re-sent");
    }

    // ----- definition -----

    fn go_req(from: usize, to: usize) -> GoToRequest {
        GoToRequest {
            target: GoToTarget::Name("main".into()),
            from,
            to,
        }
    }

    #[test]
    fn definition_inside_checkout_queues_guarded_nav() {
        let mut e = with_doc("src/a.rs", "fn main() {}\nmain();\n");
        running(&mut e, false, true);
        let mut rx = e.lsp.test_cmd_channel();
        let mut tx = Tx::default();
        e.lsp_poll(&mut tx);
        cmds(&mut rx);
        assert!(e.lsp_try_definition(&go_req(20, 24), &mut tx));
        let got = cmds(&mut rx);
        let req_id = match &got[0] {
            LspCmd::Request { method, id, .. } if *method == "textDocument/definition" => *id,
            _ => panic!("expected definition request"),
        };
        let key = rust_key(&e);
        let _ = e.lsp_reply(
            req_id,
            &key,
            Ok(json!({
                "uri": "file:///tmp/co/src/b.rs",
                "range": {"start":{"line":2,"character":0},"end":{"line":2,"character":4}}
            })),
            &mut tx,
        );
        match e.lsp_take_nav() {
            Some(crate::editor::navigation::PendingNav::OpenDoc { path, line }) => {
                assert_eq!(path, "src/b.rs");
                assert_eq!(line, Some(3));
            }
            _ => panic!("expected OpenDoc nav"),
        }
    }

    #[test]
    fn definition_outside_checkout_shows_notice() {
        let mut e = with_doc("a.rs", "fn main() {}\n");
        running(&mut e, false, true);
        let mut rx = e.lsp.test_cmd_channel();
        let mut tx = Tx::default();
        e.lsp_poll(&mut tx);
        cmds(&mut rx);
        assert!(e.lsp_try_definition(&go_req(3, 7), &mut tx));
        let got = cmds(&mut rx);
        let req_id = match &got[0] {
            LspCmd::Request { id, .. } => *id,
            _ => panic!("expected request"),
        };
        let key = rust_key(&e);
        let _ = e.lsp_reply(
            req_id,
            &key,
            Ok(json!({
                "uri": "file:///etc/hostname",
                "range": {"start":{"line":0,"character":0},"end":{"line":0,"character":1}}
            })),
            &mut tx,
        );
        assert!(e.lsp_take_nav().is_none());
        assert_eq!(
            e.notice.as_ref().map(|n| n.text.as_str()),
            Some("definition is outside the checkout")
        );
    }

    #[test]
    fn definition_empty_or_error_falls_back_to_heuristic() {
        let mut e = with_doc("a.rs", "fn main() {}\n");
        running(&mut e, false, true);
        let mut rx = e.lsp.test_cmd_channel();
        let mut tx = Tx::default();
        e.lsp_poll(&mut tx);
        cmds(&mut rx);
        assert!(e.lsp_try_definition(&go_req(3, 7), &mut tx));
        let got = cmds(&mut rx);
        let req_id = match &got[0] {
            LspCmd::Request { id, .. } => *id,
            _ => panic!("expected request"),
        };
        let key = rust_key(&e);
        // Empty result → fallback.
        let _ = e.lsp_reply(req_id, &key, Ok(Value::Null), &mut tx);
        let fb = e.lsp_take_fallback().expect("fallback queued");
        assert_eq!(fb.from, 3);
        assert_eq!(fb.to, 7);
        // Error result → fallback too.
        assert!(e.lsp_try_definition(&go_req(3, 7), &mut tx));
        let got = cmds(&mut rx);
        let req_id = match &got[0] {
            LspCmd::Request { id, .. } => *id,
            _ => panic!("expected request"),
        };
        let _ = e.lsp_reply(req_id, &key, Err("server exploded".into()), &mut tx);
        assert!(e.lsp_take_fallback().is_some());
        // Unsupported server → heuristic path immediately (no request).
        let mut e2 = with_doc("a.rs", "fn main() {}\n");
        running(&mut e2, false, false); // definition: false
        let mut rx2 = e2.lsp.test_cmd_channel();
        let mut tx2 = Tx::default();
        e2.lsp_poll(&mut tx2);
        cmds(&mut rx2);
        assert!(!e2.lsp_try_definition(&go_req(3, 7), &mut tx2));
        assert!(
            cmds(&mut rx2)
                .iter()
                .all(|c| !matches!(c, LspCmd::Request { .. }))
        );
    }

    #[test]
    fn definition_flushes_pending_change_before_request() {
        let mut e = with_doc("a.rs", "fn main() {}\n");
        running(&mut e, false, true);
        let mut rx = e.lsp.test_cmd_channel();
        let mut tx = Tx::default();
        e.lsp_poll(&mut tx);
        assert_eq!(method_of(&cmds(&mut rx)[0]), Some("didOpen"));
        // Edit then F12 while the 120 ms change debounce has NOT elapsed:
        // the definition request must be preceded by didChange on the
        // same FIFO command channel, or the server answers against text
        // it does not have yet.
        {
            let doc = e.doc.as_mut().unwrap();
            doc.code.edit.text = "fn main() {\n    changed();\n}\n".into();
            doc.code.revision += 1;
        }
        assert!(e.lsp.active.as_ref().unwrap().change_at.is_none());
        assert!(e.lsp_try_definition(&go_req(0, 3), &mut tx));
        let got = cmds(&mut rx);
        assert_eq!(got.len(), 2, "expected didChange then definition: {got:?}");
        assert_eq!(method_of(&got[0]), Some("didChange"));
        assert!(
            matches!(&got[1], LspCmd::Request { method, .. } if *method == "textDocument/definition")
        );
        assert!(e.lsp.active.as_ref().unwrap().change_at.is_none());
    }

    #[test]
    fn definition_supersedes_cancels_previous_request() {
        let mut e = with_doc("a.rs", "fn main() {}\nmain();\n");
        running(&mut e, false, true);
        let mut rx = e.lsp.test_cmd_channel();
        let mut tx = Tx::default();
        e.lsp_poll(&mut tx);
        cmds(&mut rx);
        assert!(e.lsp_try_definition(&go_req(3, 7), &mut tx));
        let got = cmds(&mut rx);
        let first_id = match &got[0] {
            LspCmd::Request { id, .. } => *id,
            _ => panic!("expected request"),
        };
        // A second invocation supersedes: CancelReq for the old id must
        // precede the new Request — the old transport entry would
        // otherwise linger until its 15 s timeout.
        assert!(e.lsp_try_definition(&go_req(20, 24), &mut tx));
        let got = cmds(&mut rx);
        assert_eq!(got.len(), 2, "expected cancel then request: {got:?}");
        assert!(matches!(&got[0], LspCmd::CancelReq { id } if *id == first_id));
        match &got[1] {
            LspCmd::Request { id, method, .. } => {
                assert_eq!(*method, "textDocument/definition");
                assert_ne!(*id, first_id);
            }
            _ => panic!("expected definition request"),
        }
        assert!(e.lsp.definition.as_ref().is_some_and(|d| d.id != first_id));
    }

    #[test]
    fn definition_with_no_change_support_still_requests() {
        let mut e = with_doc("a.rs", "fn main() {}\n");
        // Definition-capable but the sync matrix says no didChange: the
        // cap is honored (no change command) and the request still goes.
        let key = LspKey::new(e.checkout_key.clone().unwrap(), "rust");
        e.lsp.statuses.insert(
            key.clone(),
            LspStatus {
                state: LspState::Running,
                message: None,
                epoch: 1,
                features: LspFeatures {
                    completion: false,
                    hover: false,
                    definition: true,
                    sync: SyncSupport {
                        open_close: true,
                        change: SyncChange::None,
                        save: SaveSupport::None,
                    },
                },
                stderr_tail: None,
            },
        );
        e.lsp.ensured.insert(key);
        let mut rx = e.lsp.test_cmd_channel();
        let mut tx = Tx::default();
        e.lsp_poll(&mut tx);
        cmds(&mut rx);
        {
            let doc = e.doc.as_mut().unwrap();
            doc.code.edit.text = "fn main() {\n    changed();\n}\n".into();
            doc.code.revision += 1;
        }
        assert!(e.lsp_try_definition(&go_req(0, 3), &mut tx));
        let got = cmds(&mut rx);
        assert!(
            got.iter().all(|c| method_of(c) != Some("didChange")),
            "SyncChange::None must not emit didChange: {got:?}"
        );
        assert!(
            got.iter()
                .any(|c| matches!(c, LspCmd::Request { method, .. } if *method == "textDocument/definition")),
            "definition request missing: {got:?}"
        );
    }

    // ----- real runtime + fake server (no seeded status, no test channel) -----

    /// The app-core fixture, reached from this crate's manifest dir.
    fn fixture_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/app-core/tests/fixtures/fake_lsp_server.py")
    }

    /// A real checkout dir for `Connection::connect`'s cwd.
    fn temp_checkout(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "goko-lsp-it-{}-{}-{}",
            std::process::id(),
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A custom-server entry pointing at the python fixture; `env` carries
    /// the fixture's case/log/pid controls (settings has no env field, so
    /// tests inject the merged entry directly — config merge is covered
    /// by the unit tests above).
    fn fake_server_entry(dir: &Path) -> ServerEntry {
        ServerEntry {
            id: "fake".into(),
            spec: LspSpec {
                id: "fake".into(),
                argv: vec![
                    "python3".into(),
                    fixture_path().to_string_lossy().into_owned(),
                ],
                env: vec![
                    ("FAKE_LSP_CASE".into(), "ok".into()),
                    (
                        "FAKE_LSP_PIDFILE".into(),
                        dir.join("server.pid").to_string_lossy().into_owned(),
                    ),
                    (
                        "FAKE_LSP_LOG".into(),
                        dir.join("server.log").to_string_lossy().into_owned(),
                    ),
                    ("FAKE_LSP_CAPS".into(), "{}".into()),
                ],
                disabled: false,
                languages: BTreeMap::from([("ts".to_owned(), "typescript".to_owned())]),
            },
            preset: false,
            command: "python3 fake_lsp_server.py".into(),
        }
    }

    /// Drive `lsp_poll` until `want` holds or `within` elapses.
    fn pump(
        e: &mut EditorState,
        tx: &mut Tx,
        mut want: impl FnMut(&EditorState) -> bool,
        within: Duration,
    ) -> bool {
        let deadline = Instant::now() + within;
        loop {
            e.lsp_poll(tx);
            if want(e) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    }

    /// End-to-end lifecycle regression: EMPTY statuses + eligible doc →
    /// real runtime thread → real `LspHub`/`Connection` → python fixture.
    /// Covers the bootstrap deadlock (Ensure was gated behind a status
    /// that could never exist), didOpen ordering, hover + definition
    /// replies, >MAX_INFLIGHT sequential requests (slot release), and
    /// crash → Error → paced re-ensure → respawn → re-open → hover.
    #[test]
    fn real_runtime_bootstraps_syncs_and_recovers() {
        let dir = temp_checkout("boot");
        let mut e = EditorState::new(rune_kit::Kit::neutral(true), crate::test_handle());
        e.project_id = Some("p1".into());
        e.checkout = Some(dir.clone());
        e.checkout_key = Some(checkout_key("p1", &dir));
        e.doc = Some(OpenDoc::new("main.ts", &file("let x = 1;\n")));
        e.section = Section::Files;
        e.doc.as_mut().unwrap().code.edit.focused = true;
        e.lsp.config_loaded = true; // settings fetch is not under test
        e.lsp.servers = vec![fake_server_entry(&dir)];
        let mut tx = Tx::default();

        // 1. Bootstrap: no status, no seed — the flush must Ensure first.
        assert!(
            pump(
                &mut e,
                &mut tx,
                |e| e
                    .lsp
                    .statuses
                    .values()
                    .any(|s| s.state == LspState::Running),
                Duration::from_secs(20)
            ),
            "server never reached Running from empty statuses"
        );

        // 2. The fixture logged initialize + didOpen on the real wire.
        assert!(
            pump(
                &mut e,
                &mut tx,
                |_| {
                    std::fs::read_to_string(dir.join("server.log"))
                        .map(|log| {
                            log.contains("\"method\": \"initialize\"")
                                && log.contains("\"method\": \"textDocument/didOpen\"")
                        })
                        .unwrap_or(false)
                },
                Duration::from_secs(10)
            ),
            "fixture never logged initialize/didOpen"
        );
        assert!(e.lsp.active.as_ref().is_some_and(|s| s.open));

        // 3. Hover end-to-end → plaintext card.
        settle_hover(&mut e, 4);
        assert!(
            pump(
                &mut e,
                &mut tx,
                |e| e.lsp_hover_text().is_some(),
                Duration::from_secs(10)
            ),
            "hover reply never applied"
        );
        assert_eq!(e.lsp_hover_text(), Some("fake hover"));

        // 4. Definition end-to-end: the fixture answers an out-of-checkout
        //    URI, so the applied outcome is the outside-checkout notice —
        //    proving request → reply → apply ran (inside-nav is unit-tested).
        assert!(e.lsp_try_definition(&go_req(0, 3), &mut tx));
        assert!(
            pump(
                &mut e,
                &mut tx,
                |e| e
                    .notice
                    .as_ref()
                    .is_some_and(|n| n.text.contains("outside the checkout")),
                Duration::from_secs(15)
            ),
            "definition reply never applied"
        );

        // 4b. Edit then immediate definition: the forced sync flush must
        //     put didChange on the wire BEFORE the definition request —
        //     proven on the fixture's own method log, not just the
        //     command channel.
        {
            let doc = e.doc.as_mut().unwrap();
            doc.code.edit.text = "let x = 1;\nlet y = 2;\n".into();
            doc.code.revision += 1;
        }
        assert!(e.lsp_try_definition(&go_req(0, 3), &mut tx));
        assert!(
            pump(
                &mut e,
                &mut tx,
                |_| {
                    std::fs::read_to_string(dir.join("server.log"))
                        .map(|log| {
                            log.matches("\"method\": \"textDocument/definition\"")
                                .count()
                                >= 2
                        })
                        .unwrap_or(false)
                },
                Duration::from_secs(15)
            ),
            "second definition request never reached the fixture"
        );
        {
            let log = std::fs::read_to_string(dir.join("server.log")).unwrap();
            let change = log
                .rfind("\"method\": \"textDocument/didChange\"")
                .expect("didChange logged");
            let def = log
                .rfind("\"method\": \"textDocument/definition\"")
                .expect("definition logged");
            assert!(
                change < def,
                "didChange must reach the server before definition"
            );
        }

        // 5. MAX_INFLIGHT+2 sequential hovers: each completed request must
        //    free its runtime slot or later requests die with
        //    "too many language-server requests". Carets must be distinct —
        //    `hover_served` allows exactly one request per settled
        //    (epoch, path, revision, caret) snapshot. Positions clamp to
        //    EOF; the card is cleared first so a repeated reply cannot
        //    satisfy the wait trivially.
        for i in 0..(MAX_INFLIGHT + 2) {
            e.lsp.hover_card = None;
            settle_hover(&mut e, 10 + i);
            assert!(
                pump(
                    &mut e,
                    &mut tx,
                    |e| e.lsp_hover_text().is_some(),
                    Duration::from_secs(10)
                ),
                "hover {i} never applied — inflight slot leaked"
            );
        }

        // 6. Crash → Error → paced re-ensure → respawn (epoch bump) →
        //    epoch re-open → hover works again.
        let pid = std::fs::read_to_string(dir.join("server.pid"))
            .unwrap()
            .trim()
            .to_owned();
        let epoch_before = e.lsp.statuses.values().map(|s| s.epoch).max().unwrap_or(0);
        assert!(
            std::process::Command::new("kill")
                .args(["-9", pid.as_str()])
                .status()
                .is_ok_and(|s| s.success()),
            "could not kill the fixture process"
        );
        assert!(
            pump(
                &mut e,
                &mut tx,
                |e| e.lsp.statuses.values().any(|s| s.state == LspState::Error),
                Duration::from_secs(15)
            ),
            "crash never surfaced as Error"
        );
        assert!(
            pump(
                &mut e,
                &mut tx,
                |e| e
                    .lsp
                    .statuses
                    .values()
                    .any(|s| s.state == LspState::Running && s.epoch > epoch_before),
                Duration::from_secs(25)
            ),
            "server never respawned after crash"
        );
        assert!(
            pump(
                &mut e,
                &mut tx,
                |_| {
                    std::fs::read_to_string(dir.join("server.log"))
                        .map(|log| log.matches("\"method\": \"textDocument/didOpen\"").count() >= 2)
                        .unwrap_or(false)
                },
                Duration::from_secs(10)
            ),
            "document never re-opened on the respawned connection"
        );
        e.lsp.hover_card = None;
        settle_hover(&mut e, 5);
        assert!(
            pump(
                &mut e,
                &mut tx,
                |e| e.lsp_hover_text().is_some(),
                Duration::from_secs(10)
            ),
            "hover dead after respawn"
        );

        e.lsp.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
