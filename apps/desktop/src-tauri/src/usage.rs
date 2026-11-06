use crate::events::Emit;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::State;

pub(crate) const CODEX_TAIL_BYTES: u64 = 64 * 1024;
const POLL_INTERVAL: Duration = Duration::from_secs(5);
const CLAUDE_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

pub(crate) fn prune_old(dir: &Path, max_age: Duration) {
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
        five_hour: claude_limit(value.pointer("/rate_limits/five_hour"), 300),
        seven_day: claude_limit(value.pointer("/rate_limits/seven_day"), 10_080),
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

#[derive(Clone, Default)]
pub struct UsageState(Arc<RwLock<UsageSnapshot>>);

impl UsageState {
    fn new(snapshot: UsageSnapshot) -> Self {
        Self(Arc::new(RwLock::new(snapshot)))
    }

    fn snapshot(&self) -> UsageSnapshot {
        self.0
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn replace(&self, snapshot: UsageSnapshot) {
        *self
            .0
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = snapshot;
    }
}

#[tauri::command]
pub fn usage_get(state: State<'_, UsageState>) -> UsageSnapshot {
    state.snapshot()
}

pub fn spawn_poller(goko_home: PathBuf, emit: Emit) -> UsageState {
    prune_old(&goko_home.join("usage/claude"), CLAUDE_RETENTION);
    let codex_home = codex_home_from(std::env::var_os("CODEX_HOME"), std::env::var_os("HOME"));
    let mut current = read_snapshot(&goko_home, codex_home.as_deref(), &UsageSnapshot::default());
    let state = UsageState::new(current.clone());
    let poll_state = state.clone();
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(POLL_INTERVAL);
        interval.tick().await;
        loop {
            interval.tick().await;
            poll_once(
                &goko_home,
                codex_home.as_deref(),
                &mut current,
                &poll_state,
                &emit,
            );
        }
    });
    state
}

fn poll_once(
    goko_home: &Path,
    codex_home: Option<&Path>,
    current: &mut UsageSnapshot,
    state: &UsageState,
    emit: &Emit,
) {
    let next = read_snapshot(goko_home, codex_home, current);
    if next != *current {
        state.replace(next.clone());
        *current = next.clone();
        emit("usage://updated", json!(next));
    }
}

fn read_snapshot(
    goko_home: &Path,
    codex_home: Option<&Path>,
    previous: &UsageSnapshot,
) -> UsageSnapshot {
    UsageSnapshot {
        claude: read_claude(goko_home, &previous.claude),
        codex: read_codex(codex_home, &previous.codex),
    }
}

fn read_claude(goko_home: &Path, previous: &ClaudeUsage) -> ClaudeUsage {
    let mut usage = previous.clone();
    let Ok(entries) = fs::read_dir(goko_home.join("usage/claude")) else {
        return usage;
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
            if let Some(context_pct) = status.context_pct {
                usage
                    .sessions
                    .insert(session_id, SessionContext { context_pct });
            } else {
                usage.sessions.remove(&session_id);
            }
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
                update_claude_account(&mut usage, status, *modified);
                break;
            }
            None if usage.updated_at.is_some() => break,
            _ => {}
        }
    }
    usage
}

fn update_claude_account(usage: &mut ClaudeUsage, status: &ClaudeStatus, modified: SystemTime) {
    usage.five_hour = status.five_hour.clone();
    usage.seven_day = status.seven_day.clone();
    usage.updated_at = unix_millis(modified);
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

pub(crate) fn codex_home_from(
    codex_home: Option<OsString>,
    home: Option<OsString>,
) -> Option<PathBuf> {
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

pub(crate) fn collect_recent_days(dir: &Path, days: &mut Vec<PathBuf>) {
    if days.len() >= 2 {
        return;
    }
    if !rollouts_in(dir).is_empty() {
        days.push(dir.to_owned());
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    children.sort_unstable_by(|left, right| right.cmp(left));
    for child in children {
        collect_recent_days(&child, days);
        if days.len() >= 2 {
            break;
        }
    }
}

pub(crate) fn rollouts_in(dir: &Path) -> Vec<(PathBuf, SystemTime)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| {
            let path = entry.path();
            entry.file_type().is_ok_and(|kind| kind.is_file())
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("rollout-"))
                && path.extension().and_then(|extension| extension.to_str()) == Some("jsonl")
        })
        .filter_map(|entry| {
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()?;
            Some((entry.path(), modified))
        })
        .collect()
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
        let dir = home.join("usage/claude");
        fs::create_dir_all(&dir).unwrap();
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
        write_claude(
            home.path(),
            "session-old",
            21.0,
            10.0,
            UNIX_EPOCH + Duration::from_secs(10),
        );
        write_claude(
            home.path(),
            "session-new",
            62.0,
            42.0,
            UNIX_EPOCH + Duration::from_secs(20),
        );

        let snapshot = read_snapshot(home.path(), None, &UsageSnapshot::default());

        assert_eq!(snapshot.claude.five_hour.as_ref().unwrap().used_pct, 42.0);
        assert_eq!(snapshot.claude.updated_at, Some(20_000));
        assert_eq!(snapshot.claude.sessions["session-old"].context_pct, 21.0);
        assert_eq!(snapshot.claude.sessions["session-new"].context_pct, 62.0);
    }

    #[test]
    fn read_snapshot_skips_a_newer_claude_file_that_has_no_rate_limits_yet() {
        // A session that has not had its first API response (or an API-key session) writes
        // context only; it must not wipe the account limits another session reported.
        let home = crate::test_support::TempDir::new("usage-claude-no-limits");
        write_claude(
            home.path(),
            "session-old",
            21.0,
            42.0,
            UNIX_EPOCH + Duration::from_secs(10),
        );
        write_claude_context_only(
            home.path(),
            "session-new",
            3.0,
            UNIX_EPOCH + Duration::from_secs(20),
        );

        let snapshot = read_snapshot(home.path(), None, &UsageSnapshot::default());

        assert_eq!(snapshot.claude.five_hour.as_ref().unwrap().used_pct, 42.0);
        assert_eq!(snapshot.claude.updated_at, Some(10_000));
        assert_eq!(snapshot.claude.sessions["session-old"].context_pct, 21.0);
        assert_eq!(snapshot.claude.sessions["session-new"].context_pct, 3.0);

        // Still true on a later poll that starts from that snapshot.
        let again = read_snapshot(home.path(), None, &snapshot);
        assert_eq!(again.claude.five_hour, snapshot.claude.five_hour);
    }

    #[test]
    fn read_snapshot_without_any_claude_limits_reports_context_only() {
        let home = crate::test_support::TempDir::new("usage-claude-context-only");
        write_claude_context_only(
            home.path(),
            "only",
            7.0,
            UNIX_EPOCH + Duration::from_secs(5),
        );

        let snapshot = read_snapshot(home.path(), None, &UsageSnapshot::default());

        assert_eq!(snapshot.claude.five_hour, None);
        assert_eq!(snapshot.claude.seven_day, None);
        assert_eq!(snapshot.claude.sessions["only"].context_pct, 7.0);
    }

    #[test]
    fn read_snapshot_keeps_the_previous_claude_value_when_the_newest_file_is_corrupt() {
        let home = crate::test_support::TempDir::new("usage-claude-corrupt");
        write_claude(
            home.path(),
            "session-old",
            21.0,
            10.0,
            UNIX_EPOCH + Duration::from_secs(10),
        );
        let newest = write_claude(
            home.path(),
            "session-new",
            62.0,
            42.0,
            UNIX_EPOCH + Duration::from_secs(20),
        );
        let previous = read_snapshot(home.path(), None, &UsageSnapshot::default());
        fs::write(newest, "{half-written").unwrap();

        let snapshot = read_snapshot(home.path(), None, &previous);

        assert_eq!(snapshot.claude.five_hour, previous.claude.five_hour);
        assert_eq!(snapshot.claude.updated_at, previous.claude.updated_at);
        assert_eq!(snapshot.claude.sessions, previous.claude.sessions);
    }

    #[test]
    fn read_snapshot_reads_codex_from_the_newest_rollout_day_and_last_64_kib() {
        let home = crate::test_support::TempDir::new("usage-codex");
        write_rollout(
            home.path(),
            "2026/11/03",
            10.0,
            UNIX_EPOCH + Duration::from_secs(10),
        );
        write_rollout(
            home.path(),
            "2026/11/04",
            56.0,
            UNIX_EPOCH + Duration::from_secs(20),
        );

        let snapshot = read_snapshot(
            crate::test_support::TempDir::new("usage-codex-home").path(),
            Some(home.path()),
            &UsageSnapshot::default(),
        );

        assert_eq!(snapshot.codex.primary.as_ref().unwrap().used_pct, 56.0);
        assert_eq!(snapshot.codex.updated_at, Some(20_000));
    }

    #[test]
    fn read_snapshot_scans_only_the_two_newest_codex_day_directories() {
        let home = crate::test_support::TempDir::new("usage-codex-days");
        // The oldest day has the newest mtime, so a full walk would wrongly pick it.
        write_rollout(
            home.path(),
            "2026/11/01",
            99.0,
            UNIX_EPOCH + Duration::from_secs(30),
        );
        write_rollout(
            home.path(),
            "2026/11/03",
            10.0,
            UNIX_EPOCH + Duration::from_secs(10),
        );
        write_rollout(
            home.path(),
            "2026/11/04",
            56.0,
            UNIX_EPOCH + Duration::from_secs(20),
        );

        let snapshot = read_snapshot(
            crate::test_support::TempDir::new("usage-codex-days-home").path(),
            Some(home.path()),
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
        let mut current = read_snapshot(home.path(), None, &UsageSnapshot::default());
        let state = UsageState::new(current.clone());
        let emitted = Arc::new(std::sync::Mutex::new(Vec::new()));
        let received = Arc::clone(&emitted);
        let emit: Emit = Arc::new(move |name, payload| {
            received.lock().unwrap().push((name.to_owned(), payload));
        });

        poll_once(home.path(), None, &mut current, &state, &emit);
        assert!(emitted.lock().unwrap().is_empty());

        write_claude(
            home.path(),
            "session-one",
            62.0,
            42.0,
            UNIX_EPOCH + Duration::from_secs(20),
        );
        poll_once(home.path(), None, &mut current, &state, &emit);
        poll_once(home.path(), None, &mut current, &state, &emit);

        let emitted = emitted.lock().unwrap();
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].0, "usage://updated");
        assert_eq!(state.snapshot(), current);
    }
}
