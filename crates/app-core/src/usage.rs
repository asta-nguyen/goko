//! Usage and quota snapshots for Claude and Codex, per account.
//!
//! Claude's statusline hook (installed by `agents::argv_for_account`) tees
//! each session's status JSON into `GOKO_HOME/usage/claude[/<account-id>]`;
//! Codex's rollout files under `<CODEX_HOME>/sessions` carry `token_count`
//! events with `rate_limits`. [`read_snapshot`] folds both sources into one
//! [`UsageSnapshot`]: `claude`/`codex` describe the *active* account and
//! `accounts` maps every `"<agent>:<account-id>"` key to that account's
//! limits, so a shell can show quota for Default and every added account.
//!
//! This is the Tauri-free core: [`read_snapshot`] is a plain function any
//! worker thread can call, and [`spawn_poller`] drives the same 5-second
//! poll main's desktop ran — on the ambient tokio runtime when one exists,
//! else on a dedicated thread — emitting `usage://updated` through an
//! [`Emit`] sink when the snapshot changes.
//!
//! The rollout-day scan (`collect_recent_days`, `rollouts_in`,
//! `CODEX_TAIL_BYTES`) is shared with `crate::activity` and re-exported here
//! so `usage::…` paths keep working for shells ported from the desktop.

use crate::accounts::{is_safe_account_id, Accounts, AgentAccounts, DEFAULT_ID};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Notify;

pub use crate::activity::{collect_recent_days, rollouts_in, CODEX_TAIL_BYTES};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const CLAUDE_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const CLAUDE_FIVE_HOUR_MINUTES: u64 = 300;
const CLAUDE_SEVEN_DAY_MINUTES: u64 = 10_080;

/// The event sink the poller reports `usage://updated` through — the same
/// `Arc<dyn Fn(&str, Value) + Send + Sync>` shape the desktop's
/// `events::Emit` uses, so a shell's emitter drops in unchanged.
pub type Emit = Arc<dyn Fn(&str, Value) + Send + Sync>;

pub fn prune_old(dir: &Path, max_age: Duration) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json")
            || !entry.file_type().is_ok_and(|kind| kind.is_file())
        {
            continue;
        }
        if entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > max_age)
        {
            let _ = fs::remove_file(path);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct UsageSnapshot {
    pub claude: ClaudeUsage,
    pub codex: CodexUsage,
    /// Every account's limits, keyed `"<agent>:<account-id>"` (`claude:default`,
    /// `codex:<id>`, …). `claude`/`codex` above describe the active account.
    pub accounts: HashMap<String, AccountUsage>,
}

/// One account's usage: its existing limits in window order (Claude: five-hour
/// then seven-day; Codex: primary then secondary) and when they were last read.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct AccountUsage {
    pub limits: Vec<Limit>,
    pub updated_at: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ClaudeUsage {
    pub five_hour: Option<Limit>,
    pub seven_day: Option<Limit>,
    pub sessions: HashMap<String, SessionContext>,
    pub updated_at: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CodexUsage {
    pub primary: Option<Limit>,
    pub secondary: Option<Limit>,
    pub updated_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Limit {
    pub used_pct: f64,
    pub resets_at: i64,
    pub window_minutes: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SessionContext {
    pub context_pct: f64,
}

#[derive(Clone, Debug, PartialEq)]
struct ClaudeStatus {
    context_pct: Option<f64>,
    five_hour: Option<Limit>,
    seven_day: Option<Limit>,
}

fn parse_claude(text: &str) -> Option<ClaudeStatus> {
    let value: Value = serde_json::from_str(text).ok()?;
    Some(ClaudeStatus {
        context_pct: value
            .pointer("/context_window/used_percentage")
            .and_then(Value::as_f64),
        five_hour: claude_limit(
            value.pointer("/rate_limits/five_hour"),
            CLAUDE_FIVE_HOUR_MINUTES,
        ),
        seven_day: claude_limit(
            value.pointer("/rate_limits/seven_day"),
            CLAUDE_SEVEN_DAY_MINUTES,
        ),
    })
}

fn claude_limit(value: Option<&Value>, window_minutes: u64) -> Option<Limit> {
    let value = value?;
    Some(Limit {
        used_pct: value.get("used_percentage")?.as_f64()?,
        resets_at: value.get("resets_at")?.as_i64()?,
        window_minutes,
    })
}

fn parse_codex_tail(bytes: &[u8]) -> Option<(Option<Limit>, Option<Limit>)> {
    let first_line_end = bytes.iter().position(|byte| *byte == b'\n')?;
    bytes[first_line_end + 1..]
        .split(|byte| *byte == b'\n')
        .rev()
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .find(|line| {
            line.get("type").and_then(Value::as_str) == Some("event_msg")
                && line.pointer("/payload/type").and_then(Value::as_str) == Some("token_count")
        })
        .and_then(|line| {
            let limits = line.pointer("/payload/rate_limits")?.as_object()?;
            Some((
                codex_limit(limits.get("primary")),
                codex_limit(limits.get("secondary")),
            ))
        })
}

fn codex_limit(value: Option<&Value>) -> Option<Limit> {
    let value = value?;
    Some(Limit {
        used_pct: value.get("used_percent")?.as_f64()?,
        resets_at: value.get("resets_at")?.as_i64()?,
        window_minutes: value.get("window_minutes")?.as_u64()?,
    })
}

#[derive(Clone)]
pub struct UsageState {
    snapshot: Arc<RwLock<UsageSnapshot>>,
    /// Wakes the poller between interval ticks so `refresh` reads at once.
    wake: Arc<Notify>,
}

impl Default for UsageState {
    fn default() -> Self {
        Self::new(UsageSnapshot::default())
    }
}

impl UsageState {
    fn new(snapshot: UsageSnapshot) -> Self {
        Self {
            snapshot: Arc::new(RwLock::new(snapshot)),
            wake: Arc::new(Notify::new()),
        }
    }

    /// The latest snapshot the poller published.
    pub fn snapshot(&self) -> UsageSnapshot {
        self.snapshot
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn replace(&self, snapshot: UsageSnapshot) {
        *self
            .snapshot
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = snapshot;
    }

    /// Wake the poller so it reads the sources and emits at once. Account
    /// commands call this after a change; the poller still emits only when
    /// the snapshot changed.
    pub fn refresh(&self) {
        self.wake.notify_one();
    }
}

/// Start the usage poller. `accounts_source` is read every tick (and once at
/// startup) so account changes take effect without a restart; it must be
/// infallible — a broken store reads as Default-only inside `accounts::read`.
///
/// The loop runs on the ambient tokio runtime when the caller has one (a
/// Rune task, a Tauri command); otherwise it gets a dedicated thread with
/// its own current-thread runtime, so calling this from a Tauri `setup`
/// hook — outside any runtime — is safe. Returns the [`UsageState`] whose
/// [`UsageState::snapshot`] mirrors what was last emitted.
pub fn spawn_poller(
    goko_home: PathBuf,
    emit: Emit,
    accounts_source: Box<dyn Fn() -> Accounts + Send + Sync>,
) -> UsageState {
    let codex_home = codex_home_from(std::env::var_os("CODEX_HOME"), std::env::var_os("HOME"));
    let accounts = accounts_source();
    // The seven-day retention lives in `read_snapshot`, so the startup read —
    // and every tick and direct call after it — prunes each owned folder.
    let mut current = read_snapshot(
        &goko_home,
        codex_home.as_deref(),
        &accounts,
        &UsageSnapshot::default(),
    );
    let state = UsageState::new(current.clone());
    let poll_state = state.clone();
    let wake = Arc::clone(&state.wake);
    let poll = async move {
        let mut interval = tokio::time::interval(POLL_INTERVAL);
        interval.tick().await;
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = wake.notified() => {}
            }
            let accounts = accounts_source();
            poll_once(
                &goko_home,
                codex_home.as_deref(),
                &accounts,
                &mut current,
                &poll_state,
                &emit,
            );
        }
    };
    spawn_poll_task(poll);
    state
}

/// Run `poll` on the ambient tokio runtime when one is entered, else on a
/// dedicated `goko-usage` thread with a current-thread runtime (the loop
/// needs only the timer driver — its reads are plain `std::fs`).
fn spawn_poll_task(poll: impl std::future::Future<Output = ()> + Send + 'static) {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        drop(handle.spawn(poll));
        return;
    }
    let _ = std::thread::Builder::new()
        .name("goko-usage".to_owned())
        .spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
            else {
                return;
            };
            runtime.block_on(poll);
        });
}

fn poll_once(
    goko_home: &Path,
    codex_home: Option<&Path>,
    accounts: &Accounts,
    current: &mut UsageSnapshot,
    state: &UsageState,
    emit: &Emit,
) {
    let next = read_snapshot(goko_home, codex_home, accounts, current);
    if next != *current {
        state.replace(next.clone());
        *current = next.clone();
        emit("usage://updated", json!(next));
    }
}

/// One full read of every usage source. Callable from any thread — the
/// desktop ran it inside the poller, and a native shell can call it on a
/// worker whenever it wants fresh numbers. `previous` carries last tick's
/// values so a corrupt or mid-write capture keeps the prior reading instead
/// of blinking the gauges empty; pass `&UsageSnapshot::default()` for a
/// cold read.
pub fn read_snapshot(
    goko_home: &Path,
    codex_home: Option<&Path>,
    accounts: &Accounts,
    previous: &UsageSnapshot,
) -> UsageSnapshot {
    let mut account_usage = HashMap::new();
    let mut sessions = previous.claude.sessions.clone();

    // Claude: Default reads the flat `usage/claude` folder, a named account its
    // `usage/claude/<id>` subfolder; sessions merge across every owned folder.
    let claude_root = goko_home.join("usage/claude");
    let active_claude = active_id(&accounts.claude);
    let mut claude = ClaudeUsage::default();
    for (id, dir) in claude_folders(&claude_root, &accounts.claude) {
        let key = format!("claude:{id}");
        // Retention rides every read, not just poller startup: accounts added
        // later and long uptimes keep the seven-day window, and the prune
        // stays on whatever worker thread the caller chose.
        prune_old(&dir, CLAUDE_RETENTION);
        let read = read_claude_folder(&dir, &claude_previous(previous.accounts.get(&key)));
        for (session_id, context) in read.sessions {
            match context {
                Some(context) => {
                    sessions.insert(session_id, context);
                }
                None => {
                    sessions.remove(&session_id);
                }
            }
        }
        if id == active_claude {
            claude = ClaudeUsage {
                five_hour: read.five_hour.clone(),
                seven_day: read.seven_day.clone(),
                sessions: HashMap::new(),
                updated_at: read.updated_at,
            };
        }
        account_usage.insert(
            key,
            AccountUsage {
                limits: [read.five_hour, read.seven_day]
                    .into_iter()
                    .flatten()
                    .collect(),
                updated_at: read.updated_at,
            },
        );
    }
    claude.sessions = sessions;

    // Codex: Default reads the resolved default `CODEX_HOME`, a named account
    // its own folder's `sessions`.
    let active_codex = active_id(&accounts.codex);
    let mut codex = CodexUsage::default();
    for (id, home) in codex_folders(codex_home, &accounts.codex) {
        let key = format!("codex:{id}");
        let usage = read_codex(
            home.as_deref(),
            &codex_previous(previous.accounts.get(&key)),
        );
        if id == active_codex {
            codex = usage.clone();
        }
        account_usage.insert(
            key,
            AccountUsage {
                limits: [usage.primary, usage.secondary]
                    .into_iter()
                    .flatten()
                    .collect(),
                updated_at: usage.updated_at,
            },
        );
    }

    UsageSnapshot {
        claude,
        codex,
        accounts: account_usage,
    }
}

/// The active account's id, falling back to Default when `active` names no
/// stored item (the stored value is normalized on read; this is defensive).
fn active_id(agent: &AgentAccounts) -> &str {
    if agent.items.iter().any(|item| item.id == agent.active) {
        agent.active.as_str()
    } else {
        DEFAULT_ID
    }
}

/// `(account id, capture folder)` for Default and each stored Claude account.
/// Default reads the flat `usage/claude` folder — its `*.json` files only, so
/// account subfolders are never its captures — and a named account reads its
/// `usage/claude/<id>` subfolder. A folder no account owns is never listed.
fn claude_folders(root: &Path, agent: &AgentAccounts) -> Vec<(String, PathBuf)> {
    let mut folders = vec![(DEFAULT_ID.to_owned(), root.to_path_buf())];
    folders.extend(
        agent
            .items
            .iter()
            .filter(|item| valid_stored_id(&item.id))
            .map(|item| (item.id.clone(), root.join(&item.id))),
    );
    folders
}

/// `(account id, codex home)` for Default (the resolved `CODEX_HOME`, possibly
/// `None`) and each stored Codex account (its folder is its `CODEX_HOME`).
fn codex_folders(
    default_home: Option<&Path>,
    agent: &AgentAccounts,
) -> Vec<(String, Option<PathBuf>)> {
    let mut folders = vec![(DEFAULT_ID.to_owned(), default_home.map(Path::to_path_buf))];
    folders.extend(
        agent
            .items
            .iter()
            .filter(|item| valid_stored_id(&item.id))
            .map(|item| (item.id.clone(), Some(item.dir.clone()))),
    );
    folders
}

/// A stored id only names a `usage/claude` subfolder when it is a single
/// safe path component — the same predicate `accounts` normalizes with.
fn valid_stored_id(id: &str) -> bool {
    is_safe_account_id(id)
}

/// The per-account previous `ClaudeUsage` for the limits scan: the stored
/// `limits` are flat in window order, so they map back by `window_minutes`
/// (the Claude windows are fixed constants).
fn claude_previous(account: Option<&AccountUsage>) -> ClaudeUsage {
    let mut usage = ClaudeUsage::default();
    if let Some(account) = account {
        usage.updated_at = account.updated_at;
        for limit in &account.limits {
            match limit.window_minutes {
                CLAUDE_FIVE_HOUR_MINUTES => usage.five_hour = Some(limit.clone()),
                CLAUDE_SEVEN_DAY_MINUTES => usage.seven_day = Some(limit.clone()),
                _ => {}
            }
        }
    }
    usage
}

/// The per-account previous `CodexUsage`. Window order is primary then
/// secondary, but a lone stored limit cannot say which slot it held; it is
/// kept as primary — the `accounts` map round-trips it either way.
fn codex_previous(account: Option<&AccountUsage>) -> CodexUsage {
    let mut usage = CodexUsage::default();
    if let Some(account) = account {
        usage.updated_at = account.updated_at;
        let mut limits = account.limits.iter();
        usage.primary = limits.next().cloned();
        usage.secondary = limits.next().cloned();
    }
    usage
}

/// What one Claude capture folder contributed this tick.
struct ClaudeFolderRead {
    /// Session-context updates for the merged map: `Some` sets the fill,
    /// `None` clears it. Unparseable files contribute nothing, so their
    /// previous session value survives.
    sessions: HashMap<String, Option<SessionContext>>,
    five_hour: Option<Limit>,
    seven_day: Option<Limit>,
    updated_at: Option<i64>,
}

fn read_claude_folder(dir: &Path, previous: &ClaudeUsage) -> ClaudeFolderRead {
    let mut read = ClaudeFolderRead {
        sessions: HashMap::new(),
        five_hour: previous.five_hour.clone(),
        seven_day: previous.seven_day.clone(),
        updated_at: previous.updated_at,
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return read;
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json")
            || !entry.file_type().is_ok_and(|kind| kind.is_file())
        {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|metadata| metadata.modified()) else {
            continue;
        };
        let Some(session_id) = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_owned)
        else {
            continue;
        };
        let parsed = fs::read_to_string(&path)
            .ok()
            .and_then(|text| parse_claude(&text));
        if let Some(status) = &parsed {
            read.sessions.insert(
                session_id,
                status
                    .context_pct
                    .map(|context_pct| SessionContext { context_pct }),
            );
        }
        files.push((modified, path, parsed));
    }
    files.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));

    // Account limits come from the newest file that reports them. A file without limits is a
    // session that has not had its first response (or an API-key session): look past it. A file
    // that cannot be parsed is usually mid-write: keep the previous value unless nothing is known.
    for (modified, _, status) in files.iter().rev() {
        match status {
            Some(status) if status.five_hour.is_some() || status.seven_day.is_some() => {
                read.five_hour = status.five_hour.clone();
                read.seven_day = status.seven_day.clone();
                read.updated_at = unix_millis(*modified);
                break;
            }
            None if read.updated_at.is_some() => break,
            _ => {}
        }
    }
    read
}

fn read_codex(codex_home: Option<&Path>, previous: &CodexUsage) -> CodexUsage {
    let Some(codex_home) = codex_home else {
        return previous.clone();
    };
    let Some((path, modified)) = newest_rollout(&codex_home.join("sessions")) else {
        return previous.clone();
    };
    let Some((primary, secondary)) = read_codex_rollout(&path) else {
        return previous.clone();
    };
    CodexUsage {
        primary,
        secondary,
        updated_at: unix_millis(modified),
    }
}

/// `<CODEX_HOME>`: `$CODEX_HOME`, else `$HOME/.codex`, from explicit
/// environment values so tests never mutate the process environment. The
/// `&str` variant for already-UTF-8 paths lives in `crate::activity`.
pub fn codex_home_from(codex_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    codex_home
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            home.filter(|path| !path.is_empty())
                .map(PathBuf::from)
                .map(|home| home.join(".codex"))
        })
}

/// Rollouts live in `sessions/YYYY/MM/DD/`; only the two newest day directories are scanned
/// (two, so a session that crossed midnight still counts) instead of walking every old session.
fn newest_rollout(dir: &Path) -> Option<(PathBuf, SystemTime)> {
    let mut days = Vec::new();
    collect_recent_days(dir, &mut days);
    days.iter()
        .flat_map(|day| rollouts_in(day))
        .max_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)))
}

fn read_codex_rollout(path: &Path) -> Option<(Option<Limit>, Option<Limit>)> {
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(CODEX_TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut tail = Vec::new();
    if start == 0 {
        tail.push(b'\n');
    }
    file.read_to_end(&mut tail).ok()?;
    parse_codex_tail(&tail)
}

fn unix_millis(time: SystemTime) -> Option<i64> {
    time.duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis()
        .try_into()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::StoredAccount;
    use serde_json::json;
    use std::time::SystemTime;

    fn codex_line(primary: Value, secondary: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "rate_limits": { "primary": primary, "secondary": secondary }
            }
        }))
        .unwrap()
    }

    fn append_line(tail: &mut Vec<u8>, line: &[u8]) {
        tail.extend_from_slice(line);
        tail.push(b'\n');
    }

    fn set_modified(path: &Path, modified: SystemTime) {
        File::open(path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
    }

    fn write_claude(
        home: &Path,
        id: &str,
        context_pct: f64,
        used_pct: f64,
        modified: SystemTime,
    ) -> PathBuf {
        write_claude_capture(
            &home.join("usage/claude"),
            id,
            context_pct,
            used_pct,
            modified,
        )
    }

    /// A statusline capture in an explicit folder: Default's flat
    /// `usage/claude` or a named account's `usage/claude/<id>`.
    fn write_claude_capture(
        dir: &Path,
        id: &str,
        context_pct: f64,
        used_pct: f64,
        modified: SystemTime,
    ) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(format!("{id}.json"));
        fs::write(
            &path,
            json!({
                "context_window": {"used_percentage": context_pct},
                "rate_limits": {
                    "five_hour": {"used_percentage": used_pct, "resets_at": 1_700_000_000}
                }
            })
            .to_string(),
        )
        .unwrap();
        set_modified(&path, modified);
        path
    }

    fn write_claude_context_only(home: &Path, id: &str, context_pct: f64, modified: SystemTime) {
        let dir = home.join("usage/claude");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{id}.json"));
        fs::write(
            &path,
            json!({"context_window": {"used_percentage": context_pct}}).to_string(),
        )
        .unwrap();
        set_modified(&path, modified);
    }

    fn write_rollout(home: &Path, day: &str, used_pct: f64, modified: SystemTime) -> PathBuf {
        let dir = home.join("sessions").join(day);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rollout-test.jsonl");
        let event = codex_line(
            json!({"used_percent": used_pct, "window_minutes": 300, "resets_at": 1_700_000_100}),
            Value::Null,
        );
        let mut contents = vec![b'x'; CODEX_TAIL_BYTES as usize + 16];
        contents.push(b'\n');
        append_line(&mut contents, &event);
        fs::write(&path, contents).unwrap();
        set_modified(&path, modified);
        path
    }

    /// A fresh mtime `offset_secs` after a fixed ~1-hour-old base: captures
    /// are never stale to the read-time prune, and offsets still order by
    /// mtime exactly as the old `UNIX_EPOCH + N` fixtures did. The base is
    /// captured once per process, so `recent(n)` is a stable value that
    /// `unix_millis` comparisons can use directly.
    fn recent(offset_secs: u64) -> SystemTime {
        static BASE: std::sync::OnceLock<SystemTime> = std::sync::OnceLock::new();
        *BASE.get_or_init(|| SystemTime::now() - Duration::from_secs(3600))
            + Duration::from_secs(offset_secs)
    }

    fn stored(id: &str, dir: &Path) -> StoredAccount {
        StoredAccount {
            id: id.to_owned(),
            name: id.to_owned(),
            dir: dir.to_path_buf(),
        }
    }

    fn agent_accounts(active: &str, items: Vec<StoredAccount>) -> AgentAccounts {
        AgentAccounts {
            active: active.to_owned(),
            items,
        }
    }

    #[test]
    fn parse_claude_reads_context_and_subscriber_limits() {
        let parsed = parse_claude(
            r#"{"context_window":{"used_percentage":62},"rate_limits":{"five_hour":{"used_percentage":42,"resets_at":1700000000},"seven_day":{"used_percentage":13,"resets_at":1700600000}}}"#,
        )
        .unwrap();

        assert_eq!(parsed.context_pct, Some(62.0));
        assert_eq!(
            parsed.five_hour,
            Some(Limit {
                used_pct: 42.0,
                resets_at: 1_700_000_000,
                window_minutes: 300,
            })
        );
        assert_eq!(
            parsed.seven_day,
            Some(Limit {
                used_pct: 13.0,
                resets_at: 1_700_600_000,
                window_minutes: 10_080,
            })
        );
    }

    #[test]
    fn parse_claude_without_rate_limits_keeps_only_context() {
        let parsed = parse_claude(r#"{"context_window":{"used_percentage":62}}"#).unwrap();

        assert_eq!(parsed.context_pct, Some(62.0));
        assert_eq!(parsed.five_hour, None);
        assert_eq!(parsed.seven_day, None);
    }

    #[test]
    fn parse_claude_null_context_is_unknown() {
        let parsed = parse_claude(r#"{"context_window":{"used_percentage":null}}"#).unwrap();

        assert_eq!(parsed.context_pct, None);
    }

    #[test]
    fn parse_claude_rejects_non_json_and_partial_json() {
        assert_eq!(parse_claude("not json"), None);
        assert_eq!(
            parse_claude(r#"{"context_window":{"used_percentage":62}"#),
            None
        );
    }

    #[test]
    fn parse_codex_tail_uses_the_last_token_count_event() {
        let mut tail = b"\n".to_vec();
        append_line(
            &mut tail,
            &codex_line(
                json!({"used_percent": 1, "window_minutes": 300, "resets_at": 100}),
                json!({"used_percent": 2, "window_minutes": 10_080, "resets_at": 200}),
            ),
        );
        append_line(
            &mut tail,
            &codex_line(
                json!({"used_percent": 56, "window_minutes": 300, "resets_at": 300}),
                json!({"used_percent": 13, "window_minutes": 10_080, "resets_at": 400}),
            ),
        );

        assert_eq!(
            parse_codex_tail(&tail),
            Some((
                Some(Limit {
                    used_pct: 56.0,
                    window_minutes: 300,
                    resets_at: 300,
                }),
                Some(Limit {
                    used_pct: 13.0,
                    window_minutes: 10_080,
                    resets_at: 400,
                }),
            ))
        );
    }

    #[test]
    fn parse_codex_tail_ignores_a_truncated_first_line() {
        let mut tail = b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\"\n".to_vec();
        append_line(
            &mut tail,
            &codex_line(
                json!({"used_percent": 56, "window_minutes": 300, "resets_at": 300}),
                Value::Null,
            ),
        );

        assert_eq!(
            parse_codex_tail(&tail),
            Some((
                Some(Limit {
                    used_pct: 56.0,
                    window_minutes: 300,
                    resets_at: 300,
                }),
                None,
            ))
        );
    }

    #[test]
    fn parse_codex_tail_returns_none_without_a_token_count_or_rate_limits() {
        let mut no_event = b"\n".to_vec();
        append_line(&mut no_event, br#"{"type":"turn_context"}"#);
        assert_eq!(parse_codex_tail(&no_event), None);

        let mut no_limits = b"\n".to_vec();
        append_line(
            &mut no_limits,
            &serde_json::to_vec(&json!({
                "type": "event_msg",
                "payload": {"type": "token_count", "rate_limits": null}
            }))
            .unwrap(),
        );
        assert_eq!(parse_codex_tail(&no_limits), None);
    }

    #[test]
    fn prune_old_removes_only_json_older_than_the_limit() {
        let dir = crate::test_support::TempDir::new("usage-prune");
        let old = dir.path().join("old.json");
        let recent = dir.path().join("recent.json");
        let other = dir.path().join("old.txt");
        fs::write(&old, "{}").unwrap();
        fs::write(&recent, "{}").unwrap();
        fs::write(&other, "{}").unwrap();
        let old_time = SystemTime::now()
            .checked_sub(Duration::from_secs(8 * 24 * 60 * 60))
            .unwrap();
        fs::File::open(&old)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(old_time))
            .unwrap();

        prune_old(dir.path(), Duration::from_secs(7 * 24 * 60 * 60));

        assert!(!old.exists());
        assert!(recent.exists());
        assert!(other.exists());
    }

    #[test]
    fn read_snapshot_uses_the_newest_claude_file_and_keeps_each_session_context() {
        let home = crate::test_support::TempDir::new("usage-claude-latest");
        write_claude(home.path(), "session-old", 21.0, 10.0, recent(10));
        write_claude(home.path(), "session-new", 62.0, 42.0, recent(20));

        let snapshot = read_snapshot(
            home.path(),
            None,
            &Accounts::default(),
            &UsageSnapshot::default(),
        );

        assert_eq!(snapshot.claude.five_hour.as_ref().unwrap().used_pct, 42.0);
        assert_eq!(snapshot.claude.updated_at, unix_millis(recent(20)));
        assert_eq!(snapshot.claude.sessions["session-old"].context_pct, 21.0);
        assert_eq!(snapshot.claude.sessions["session-new"].context_pct, 62.0);
    }

    #[test]
    fn read_snapshot_skips_a_newer_claude_file_that_has_no_rate_limits_yet() {
        // A session that has not had its first API response (or an API-key session) writes
        // context only; it must not wipe the account limits another session reported.
        let home = crate::test_support::TempDir::new("usage-claude-no-limits");
        write_claude(home.path(), "session-old", 21.0, 42.0, recent(10));
        write_claude_context_only(home.path(), "session-new", 3.0, recent(20));

        let snapshot = read_snapshot(
            home.path(),
            None,
            &Accounts::default(),
            &UsageSnapshot::default(),
        );

        assert_eq!(snapshot.claude.five_hour.as_ref().unwrap().used_pct, 42.0);
        assert_eq!(snapshot.claude.updated_at, unix_millis(recent(10)));
        assert_eq!(snapshot.claude.sessions["session-old"].context_pct, 21.0);
        assert_eq!(snapshot.claude.sessions["session-new"].context_pct, 3.0);

        // Still true on a later poll that starts from that snapshot.
        let again = read_snapshot(home.path(), None, &Accounts::default(), &snapshot);
        assert_eq!(again.claude.five_hour, snapshot.claude.five_hour);
    }

    #[test]
    fn read_snapshot_without_any_claude_limits_reports_context_only() {
        let home = crate::test_support::TempDir::new("usage-claude-context-only");
        write_claude_context_only(home.path(), "only", 7.0, recent(5));

        let snapshot = read_snapshot(
            home.path(),
            None,
            &Accounts::default(),
            &UsageSnapshot::default(),
        );

        assert_eq!(snapshot.claude.five_hour, None);
        assert_eq!(snapshot.claude.seven_day, None);
        assert_eq!(snapshot.claude.sessions["only"].context_pct, 7.0);
    }

    #[test]
    fn read_snapshot_keeps_the_previous_claude_value_when_the_newest_file_is_corrupt() {
        let home = crate::test_support::TempDir::new("usage-claude-corrupt");
        write_claude(home.path(), "session-old", 21.0, 10.0, recent(10));
        let newest = write_claude(home.path(), "session-new", 62.0, 42.0, recent(20));
        let previous = read_snapshot(
            home.path(),
            None,
            &Accounts::default(),
            &UsageSnapshot::default(),
        );
        fs::write(newest, "{half-written").unwrap();

        let snapshot = read_snapshot(home.path(), None, &Accounts::default(), &previous);

        assert_eq!(snapshot.claude.five_hour, previous.claude.five_hour);
        assert_eq!(snapshot.claude.updated_at, previous.claude.updated_at);
        assert_eq!(snapshot.claude.sessions, previous.claude.sessions);
    }

    #[test]
    fn read_snapshot_reads_codex_from_the_newest_rollout_day_and_last_64_kib() {
        let home = crate::test_support::TempDir::new("usage-codex");
        write_rollout(home.path(), "2026/11/03", 10.0, recent(10));
        write_rollout(home.path(), "2026/11/04", 56.0, recent(20));

        let snapshot = read_snapshot(
            crate::test_support::TempDir::new("usage-codex-home").path(),
            Some(home.path()),
            &Accounts::default(),
            &UsageSnapshot::default(),
        );

        assert_eq!(snapshot.codex.primary.as_ref().unwrap().used_pct, 56.0);
        assert_eq!(snapshot.codex.updated_at, unix_millis(recent(20)));
    }

    #[test]
    fn read_snapshot_scans_only_the_two_newest_codex_day_directories() {
        let home = crate::test_support::TempDir::new("usage-codex-days");
        // The oldest day has the newest mtime, so a full walk would wrongly pick it.
        write_rollout(home.path(), "2026/11/01", 99.0, recent(30));
        write_rollout(home.path(), "2026/11/03", 10.0, recent(10));
        write_rollout(home.path(), "2026/11/04", 56.0, recent(20));

        let snapshot = read_snapshot(
            crate::test_support::TempDir::new("usage-codex-days-home").path(),
            Some(home.path()),
            &Accounts::default(),
            &UsageSnapshot::default(),
        );

        assert_eq!(snapshot.codex.primary.as_ref().unwrap().used_pct, 56.0);
    }

    #[test]
    fn codex_home_defaults_to_the_users_dot_codex_directory() {
        assert_eq!(
            codex_home_from(None, Some(OsString::from("/Users/tester"))),
            Some(PathBuf::from("/Users/tester/.codex"))
        );
        assert_eq!(
            codex_home_from(
                Some(OsString::from("/custom/codex")),
                Some(OsString::from("/Users/tester"))
            ),
            Some(PathBuf::from("/custom/codex"))
        );
    }

    #[test]
    fn poll_once_emits_only_when_the_snapshot_changes() {
        let home = crate::test_support::TempDir::new("usage-poll");
        let mut current = read_snapshot(
            home.path(),
            None,
            &Accounts::default(),
            &UsageSnapshot::default(),
        );
        let state = UsageState::new(current.clone());
        let emitted = Arc::new(std::sync::Mutex::new(Vec::new()));
        let received = Arc::clone(&emitted);
        let emit: Emit = Arc::new(move |name, payload| {
            received.lock().unwrap().push((name.to_owned(), payload));
        });

        poll_once(
            home.path(),
            None,
            &Accounts::default(),
            &mut current,
            &state,
            &emit,
        );
        assert!(emitted.lock().unwrap().is_empty());

        write_claude(home.path(), "session-one", 62.0, 42.0, recent(20));
        poll_once(
            home.path(),
            None,
            &Accounts::default(),
            &mut current,
            &state,
            &emit,
        );
        poll_once(
            home.path(),
            None,
            &Accounts::default(),
            &mut current,
            &state,
            &emit,
        );

        let emitted = emitted.lock().unwrap();
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].0, "usage://updated");
        assert_eq!(state.snapshot(), current);
    }

    #[test]
    fn read_snapshot_reports_the_default_account_exactly_as_before() {
        let home = crate::test_support::TempDir::new("usage-default-account");
        write_claude(home.path(), "session-one", 62.0, 42.0, recent(20));

        let snapshot = read_snapshot(
            home.path(),
            None,
            &Accounts::default(),
            &UsageSnapshot::default(),
        );

        // `claude` keeps today's shape and values.
        assert_eq!(snapshot.claude.five_hour.as_ref().unwrap().used_pct, 42.0);
        assert_eq!(snapshot.claude.seven_day, None);
        assert_eq!(snapshot.claude.updated_at, unix_millis(recent(20)));
        assert_eq!(snapshot.claude.sessions["session-one"].context_pct, 62.0);

        // `claude:default` carries the same limits and update time.
        let entry = &snapshot.accounts["claude:default"];
        assert_eq!(
            entry.limits,
            vec![snapshot.claude.five_hour.clone().unwrap()]
        );
        assert_eq!(entry.updated_at, snapshot.claude.updated_at);

        // Codex has its Default entry too, empty without a home or rollouts.
        assert!(snapshot.accounts["codex:default"].limits.is_empty());
        assert_eq!(snapshot.accounts["codex:default"].updated_at, None);
    }

    #[test]
    fn read_snapshot_reads_every_claude_account_and_reports_the_active_one() {
        let home = crate::test_support::TempDir::new("usage-two-claude");
        write_claude(home.path(), "default-session", 10.0, 11.0, recent(10));
        write_claude_capture(
            &home.path().join("usage/claude/work"),
            "work-session",
            70.0,
            77.0,
            recent(20),
        );
        let accounts = Accounts {
            claude: agent_accounts(
                "work",
                vec![stored("work", &home.path().join("claude-work"))],
            ),
            codex: AgentAccounts::default(),
        };

        let snapshot = read_snapshot(home.path(), None, &accounts, &UsageSnapshot::default());

        // `claude` describes the active account ("work"); `accounts` has both.
        assert_eq!(snapshot.claude.five_hour.as_ref().unwrap().used_pct, 77.0);
        assert_eq!(snapshot.claude.updated_at, unix_millis(recent(20)));
        assert_eq!(snapshot.accounts["claude:default"].limits[0].used_pct, 11.0);
        assert_eq!(
            snapshot.accounts["claude:default"].updated_at,
            unix_millis(recent(10))
        );
        assert_eq!(snapshot.accounts["claude:work"].limits[0].used_pct, 77.0);
        assert_eq!(
            snapshot.accounts["claude:work"].updated_at,
            unix_millis(recent(20))
        );

        // Switching the active id changes `claude` but not `accounts`.
        let mut flipped = accounts.clone();
        flipped.claude.active = "default".to_owned();
        let snapshot = read_snapshot(home.path(), None, &flipped, &snapshot);
        assert_eq!(snapshot.claude.five_hour.as_ref().unwrap().used_pct, 11.0);
        assert_eq!(snapshot.accounts["claude:work"].limits[0].used_pct, 77.0);
        assert_eq!(
            snapshot.accounts,
            read_snapshot(home.path(), None, &accounts, &UsageSnapshot::default()).accounts
        );
    }

    #[test]
    fn read_snapshot_merges_claude_sessions_across_account_folders() {
        let home = crate::test_support::TempDir::new("usage-claude-sessions");
        write_claude(home.path(), "default-session", 10.0, 11.0, recent(10));
        write_claude_capture(
            &home.path().join("usage/claude/work"),
            "work-session",
            70.0,
            77.0,
            recent(20),
        );
        let accounts = Accounts {
            claude: agent_accounts(
                "default",
                vec![stored("work", &home.path().join("claude-work"))],
            ),
            codex: AgentAccounts::default(),
        };

        let snapshot = read_snapshot(home.path(), None, &accounts, &UsageSnapshot::default());

        assert_eq!(
            snapshot.claude.sessions["default-session"].context_pct,
            10.0
        );
        assert_eq!(snapshot.claude.sessions["work-session"].context_pct, 70.0);
    }

    #[test]
    fn read_snapshot_does_not_read_a_subfolder_as_default_claude_captures() {
        let home = crate::test_support::TempDir::new("usage-claude-subdir");
        // A stray folder inside `usage/claude` that no account owns.
        write_claude_capture(
            &home.path().join("usage/claude/stray"),
            "stray-session",
            50.0,
            55.0,
            recent(30),
        );
        write_claude(home.path(), "real-session", 10.0, 11.0, recent(10));

        let snapshot = read_snapshot(
            home.path(),
            None,
            &Accounts::default(),
            &UsageSnapshot::default(),
        );

        // Default read only the flat folder: the stray file's newer limits and
        // session never leak in.
        assert_eq!(snapshot.claude.five_hour.as_ref().unwrap().used_pct, 11.0);
        assert_eq!(snapshot.claude.updated_at, unix_millis(recent(10)));
        assert!(snapshot.claude.sessions.contains_key("real-session"));
        assert!(!snapshot.claude.sessions.contains_key("stray-session"));
        assert!(!snapshot.accounts.contains_key("claude:stray"));
    }

    #[test]
    fn read_snapshot_reads_codex_per_account_and_reports_the_active_one() {
        let home = crate::test_support::TempDir::new("usage-codex-accounts");
        let default_codex = crate::test_support::TempDir::new("usage-codex-default");
        let work_codex = crate::test_support::TempDir::new("usage-codex-work");
        write_rollout(default_codex.path(), "2026/11/04", 10.0, recent(10));
        write_rollout(work_codex.path(), "2026/11/04", 56.0, recent(20));
        let accounts = Accounts {
            claude: AgentAccounts::default(),
            codex: agent_accounts("work", vec![stored("work", work_codex.path())]),
        };

        let snapshot = read_snapshot(
            home.path(),
            Some(default_codex.path()),
            &accounts,
            &UsageSnapshot::default(),
        );

        // `codex` describes the active account ("work"); `accounts` has both.
        assert_eq!(snapshot.codex.primary.as_ref().unwrap().used_pct, 56.0);
        assert_eq!(snapshot.codex.updated_at, unix_millis(recent(20)));
        assert_eq!(snapshot.accounts["codex:default"].limits[0].used_pct, 10.0);
        assert_eq!(snapshot.accounts["codex:work"].limits[0].used_pct, 56.0);

        // Switching the active id changes `codex` but not `accounts`.
        let mut flipped = accounts.clone();
        flipped.codex.active = "default".to_owned();
        let snapshot = read_snapshot(home.path(), Some(default_codex.path()), &flipped, &snapshot);
        assert_eq!(snapshot.codex.primary.as_ref().unwrap().used_pct, 10.0);
        assert_eq!(
            snapshot.accounts,
            read_snapshot(
                home.path(),
                Some(default_codex.path()),
                &accounts,
                &UsageSnapshot::default(),
            )
            .accounts
        );
    }

    #[test]
    fn read_snapshot_keeps_an_accounts_previous_value_when_its_capture_is_corrupt() {
        let home = crate::test_support::TempDir::new("usage-corrupt-account");
        write_claude(home.path(), "default-session", 10.0, 11.0, recent(30));
        let work_file = write_claude_capture(
            &home.path().join("usage/claude/work"),
            "work-session",
            70.0,
            77.0,
            recent(20),
        );
        let accounts = Accounts {
            claude: agent_accounts(
                "default",
                vec![stored("work", &home.path().join("claude-work"))],
            ),
            codex: AgentAccounts::default(),
        };
        let previous = read_snapshot(home.path(), None, &accounts, &UsageSnapshot::default());
        assert_eq!(previous.accounts["claude:work"].limits[0].used_pct, 77.0);

        // The account's newest capture is now mid-write garbage; Default gets a
        // newer good file, so Default still updates while "work" keeps its value.
        fs::write(&work_file, "{half-written").unwrap();
        write_claude(home.path(), "default-session-new", 12.0, 13.0, recent(40));

        let snapshot = read_snapshot(home.path(), None, &accounts, &previous);

        assert_eq!(
            snapshot.accounts["claude:work"],
            previous.accounts["claude:work"]
        );
        assert_eq!(snapshot.accounts["claude:default"].limits[0].used_pct, 13.0);
    }

    #[test]
    fn read_snapshot_ignores_a_capture_folder_no_account_owns() {
        let home = crate::test_support::TempDir::new("usage-orphan");
        // "gone" was removed; its capture folder stays on disk with the newest file.
        write_claude_capture(
            &home.path().join("usage/claude/gone"),
            "gone-session",
            90.0,
            95.0,
            recent(50),
        );
        write_claude_capture(
            &home.path().join("usage/claude/work"),
            "work-session",
            70.0,
            77.0,
            recent(20),
        );
        write_claude(home.path(), "default-session", 10.0, 11.0, recent(10));
        let accounts = Accounts {
            claude: agent_accounts(
                "default",
                vec![stored("work", &home.path().join("claude-work"))],
            ),
            codex: AgentAccounts::default(),
        };

        let snapshot = read_snapshot(home.path(), None, &accounts, &UsageSnapshot::default());

        assert_eq!(snapshot.accounts.len(), 3);
        assert!(!snapshot.accounts.contains_key("claude:gone"));
        assert_eq!(snapshot.claude.five_hour.as_ref().unwrap().used_pct, 11.0);
        assert!(!snapshot.claude.sessions.contains_key("gone-session"));
        assert!(snapshot.claude.sessions.contains_key("work-session"));
    }

    #[tokio::test]
    async fn refresh_completes_a_pending_wait_on_the_poller_wake_signal() {
        let state = UsageState::default();
        let wake = Arc::clone(&state.wake);
        let waiter = tokio::spawn(async move { wake.notified().await });
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished(), "the waiter finished before refresh");

        state.refresh();

        tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("refresh did not wake the poller")
            .unwrap();
    }

    #[test]
    fn read_snapshot_prunes_stale_captures_in_owned_folders_only() {
        let home = crate::test_support::TempDir::new("usage-prune-read");
        let stale = SystemTime::now() - Duration::from_secs(8 * 24 * 60 * 60);
        // Owned: Default's flat folder and the "work" account subfolder.
        let default_stale = write_claude(home.path(), "stale-default", 1.0, 2.0, stale);
        let work_stale = write_claude_capture(
            &home.path().join("usage/claude/work"),
            "stale-work",
            1.0,
            2.0,
            stale,
        );
        // Fresh capture that must survive.
        let fresh = write_claude(home.path(), "fresh", 5.0, 6.0, recent(10));
        // Not a .json file: survives even at a stale age.
        let txt = home.path().join("usage/claude/old.txt");
        fs::write(&txt, "{}").unwrap();
        set_modified(&txt, stale);
        // A folder no account owns is never read, so never pruned.
        let orphan = write_claude_capture(
            &home.path().join("usage/claude/gone"),
            "stale-orphan",
            1.0,
            2.0,
            stale,
        );
        let accounts = Accounts {
            claude: agent_accounts(
                "work",
                vec![stored("work", &home.path().join("claude-work"))],
            ),
            codex: AgentAccounts::default(),
        };

        let snapshot = read_snapshot(home.path(), None, &accounts, &UsageSnapshot::default());

        assert!(
            !default_stale.exists(),
            "Default's stale capture was pruned"
        );
        assert!(!work_stale.exists(), "work's stale capture was pruned");
        assert!(orphan.exists(), "an unowned folder is never pruned");
        assert!(txt.exists(), "only .json captures are pruned");
        assert!(fresh.exists());
        assert!(snapshot.claude.sessions.contains_key("fresh"));
    }

    #[test]
    fn read_snapshot_gives_every_account_an_entry_even_without_data() {
        let home = crate::test_support::TempDir::new("usage-all-accounts");
        let empty_codex = crate::test_support::TempDir::new("usage-empty-codex");
        write_claude_capture(
            &home.path().join("usage/claude/used"),
            "used-session",
            1.0,
            2.0,
            recent(10),
        );
        let accounts = Accounts {
            claude: agent_accounts(
                "default",
                vec![
                    stored("empty", &home.path().join("claude-empty")),
                    stored("used", &home.path().join("claude-used")),
                ],
            ),
            codex: agent_accounts("default", vec![stored("cx", empty_codex.path())]),
        };

        let snapshot = read_snapshot(home.path(), None, &accounts, &UsageSnapshot::default());

        for key in [
            "claude:default",
            "claude:empty",
            "claude:used",
            "codex:default",
            "codex:cx",
        ] {
            assert!(snapshot.accounts.contains_key(key), "missing {key}");
        }
        assert!(snapshot.accounts["claude:empty"].limits.is_empty());
        assert_eq!(snapshot.accounts["claude:empty"].updated_at, None);
        assert!(snapshot.accounts["codex:cx"].limits.is_empty());
        assert_eq!(snapshot.accounts["codex:cx"].updated_at, None);
        assert_eq!(snapshot.accounts["claude:used"].limits[0].used_pct, 2.0);

        // The wire shape is exactly {"limits": [...], "updated_at": ms|null}.
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(
            json["accounts"]["claude:empty"],
            json!({"limits": [], "updated_at": null})
        );
    }
}
