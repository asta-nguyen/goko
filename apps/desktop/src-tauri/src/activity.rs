use crate::bridge::Bridge;
use crate::events::Emit;
use crate::usage::{codex_home_from, collect_recent_days, rollouts_in, CODEX_TAIL_BYTES};
use protocol::session::{SessionInfo, SessionState};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};
use tauri::State;

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const CLAUDE_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Activity {
    Working,
    Waiting,
    Done,
}

/// The word a Claude hook wrote to its activity file. Empty, unknown, or mid-write
/// content means "no reading": the caller keeps the previous value.
pub fn parse_claude_activity(text: &str) -> Option<Activity> {
    match text.trim() {
        "working" => Some(Activity::Working),
        "waiting" => Some(Activity::Waiting),
        "done" => Some(Activity::Done),
        _ => None,
    }
}

/// The activity of a Codex rollout tail: the last `task_started` / `task_complete` event
/// wins. Lines that are not JSON (the read can start mid-line) or other events are ignored.
pub fn codex_activity_from_tail(bytes: &[u8]) -> Option<Activity> {
    bytes
        .split(|byte| *byte == b'\n')
        .rev()
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .find_map(|line| {
            if line.get("type").and_then(Value::as_str) != Some("event_msg") {
                return None;
            }
            match line.pointer("/payload/type").and_then(Value::as_str) {
                Some("task_started") => Some(Activity::Working),
                Some("task_complete") => Some(Activity::Done),
                _ => None,
            }
        })
}

/// The activity of the session writing `path`, read from its final 64 KiB the same way
/// `usage::read_codex_rollout` does.
pub fn codex_activity(path: &Path) -> Option<Activity> {
    let mut file = File::open(path).ok()?;
    let start = file.metadata().ok()?.len().saturating_sub(CODEX_TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    codex_activity_from_tail(&tail)
}

pub struct RolloutMeta {
    pub path: PathBuf,
    pub cwd: String,
    pub started_ms: i64,
}

/// The `session_meta` line that opens a rollout file. `read_line` grows its buffer, so a
/// `base_instructions` field of tens of KB still reads as one line without touching the rest.
pub fn rollout_meta(path: &Path) -> Option<RolloutMeta> {
    let mut line = String::new();
    BufReader::new(File::open(path).ok()?)
        .read_line(&mut line)
        .ok()?;
    let line = serde_json::from_str::<Value>(&line).ok()?;
    if line.get("type").and_then(Value::as_str) != Some("session_meta") {
        return None;
    }
    Some(RolloutMeta {
        path: path.to_owned(),
        cwd: line
            .pointer("/payload/cwd")
            .and_then(Value::as_str)?
            .to_owned(),
        started_ms: timestamp_ms(line.pointer("/payload/timestamp").and_then(Value::as_str)?)?,
    })
}

/// Rollout metas from the two newest day directories under `<codex_home>/sessions`, the
/// same scan window `usage` uses so a rollout that crossed midnight still shows up.
pub fn recent_rollouts(codex_home: &Path) -> Vec<RolloutMeta> {
    let mut days = Vec::new();
    collect_recent_days(&codex_home.join("sessions"), &mut days);
    days.iter()
        .flat_map(|day| rollouts_in(day))
        .filter_map(|(path, _)| rollout_meta(&path))
        .collect()
}

pub struct CodexSession<'a> {
    pub id: &'a str,
    pub cwd: &'a str,
    pub started_at_ms: i64,
}

/// Goko session id -> the rollout it owns. Sessions claim in `started_at_ms` order (ties by
/// id); each takes the earliest unclaimed rollout (ties by path) with the same `cwd` whose
/// `session_meta` timestamp is not earlier than the session start minus 5 s of clock slack.
pub fn match_codex_rollouts<'a>(
    sessions: &[CodexSession<'a>],
    rollouts: &[RolloutMeta],
) -> HashMap<String, PathBuf> {
    let mut ordered: Vec<&CodexSession> = sessions.iter().collect();
    ordered.sort_by(|left, right| {
        left.started_at_ms
            .cmp(&right.started_at_ms)
            .then_with(|| left.id.cmp(right.id))
    });
    let mut candidates: Vec<&RolloutMeta> = rollouts.iter().collect();
    candidates.sort_by(|left, right| {
        left.started_ms
            .cmp(&right.started_ms)
            .then_with(|| left.path.cmp(&right.path))
    });
    let mut claimed = HashSet::new();
    let mut matched = HashMap::new();
    for session in ordered {
        let Some(rollout) = candidates.iter().copied().find(|rollout| {
            !claimed.contains(&rollout.path)
                && rollout.cwd.as_str() == session.cwd
                && rollout.started_ms >= session.started_at_ms - 5_000
        }) else {
            continue;
        };
        claimed.insert(rollout.path.clone());
        matched.insert(session.id.to_owned(), rollout.path.clone());
    }
    matched
}

/// `YYYY-MM-DDTHH:MM:SS[.fff…]Z` -> unix milliseconds, hand-parsed so this module needs no
/// date crate. Anything outside that exact shape is `None`.
fn timestamp_ms(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[bytes.len() - 1] != b'Z'
    {
        return None;
    }
    let field = |start: usize, end: usize| {
        bytes[start..end].iter().try_fold(0i64, |value, byte| {
            byte.is_ascii_digit()
                .then(|| value * 10 + i64::from(byte - b'0'))
        })
    };
    let (year, month, day) = (field(0, 4)?, field(5, 7)?, field(8, 10)?);
    let (hour, minute, second) = (field(11, 13)?, field(14, 16)?, field(17, 19)?);
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days: i64 = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if !(1..=month_days).contains(&day) || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let millis = (days * 86_400 + hour * 3_600 + minute * 60 + second) * 1_000;
    match bytes[19] {
        b'Z' if bytes.len() == 20 => Some(millis),
        b'.' => {
            let digits = &bytes[20..bytes.len() - 1];
            if digits.is_empty() || !digits.iter().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            let fraction = digits
                .iter()
                .take(3)
                .enumerate()
                .map(|(index, digit)| i64::from(digit - b'0') * 10i64.pow(2 - index as u32))
                .sum::<i64>();
            Some(millis + fraction)
        }
        _ => None,
    }
}

/// Days since 1970-01-01 in the proleptic Gregorian calendar (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * if month > 2 { month - 3 } else { month + 9 } + 2) / 5 + day - 1;
    era * 146_097 + year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year - 719_468
}

#[derive(Clone, Default)]
pub struct ActivityState(Arc<RwLock<HashMap<String, Activity>>>);

impl ActivityState {
    fn snapshot(&self) -> HashMap<String, Activity> {
        self.0
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn replace(&self, activity: HashMap<String, Activity>) {
        *self
            .0
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = activity;
    }
}

#[tauri::command]
pub fn activity_get(state: State<'_, ActivityState>) -> HashMap<String, Activity> {
    state.snapshot()
}

/// The value after `--session-id`, which `agents::argv_for` passes to Claude. A value with a
/// path separator is not an id Goko wrote, so it never names a file.
fn claude_session_id(argv: &[String]) -> Option<&str> {
    let id = argv.get(argv.iter().position(|arg| arg == "--session-id")? + 1)?;
    (!id.is_empty() && !id.contains('/')).then_some(id.as_str())
}

/// Goko launches Codex as its bare resolved path, so a lone argv whose stem is `codex`
/// (`codex.js` for an npm install) is a Codex session.
fn is_codex(argv: &[String]) -> bool {
    matches!(argv, [bin] if Path::new(bin).file_stem().is_some_and(|stem| stem == "codex"))
}

/// Goko session id -> activity, for running Claude and Codex sessions only. A session whose
/// source cannot be read this tick keeps its `previous` value, so a hook file caught
/// mid-write or a rollout between turns never flickers the dot.
pub fn compute_activity(
    sessions: &[SessionInfo],
    goko_home: &Path,
    codex_home: Option<&Path>,
    previous: &HashMap<String, Activity>,
) -> HashMap<String, Activity> {
    let running: Vec<(String, &SessionInfo)> = sessions
        .iter()
        .filter(|session| session.state == SessionState::Running)
        .map(|session| (session.id.as_uuid().to_string(), session))
        .collect();
    let mut next = HashMap::new();
    let mut codex = Vec::new();
    for (id, session) in &running {
        if let Some(claude_id) = claude_session_id(&session.argv) {
            let read = fs::read_to_string(goko_home.join("activity/claude").join(claude_id))
                .ok()
                .and_then(|text| parse_claude_activity(&text));
            if let Some(activity) = read.or_else(|| previous.get(id).copied()) {
                next.insert(id.clone(), activity);
            }
        } else if is_codex(&session.argv) {
            codex.push(CodexSession {
                id,
                cwd: &session.cwd,
                started_at_ms: session.started_at,
            });
        }
    }
    let matched = codex_home
        .map(|home| match_codex_rollouts(&codex, &recent_rollouts(home)))
        .unwrap_or_default();
    for session in &codex {
        let read = matched
            .get(session.id)
            .and_then(|path| codex_activity(path));
        if let Some(activity) = read.or_else(|| previous.get(session.id).copied()) {
            next.insert(session.id.to_owned(), activity);
        }
    }
    next
}

/// Store and announce `next` only when it differs from `current`.
fn apply(
    next: HashMap<String, Activity>,
    current: &mut HashMap<String, Activity>,
    state: &ActivityState,
    emit: &Emit,
) {
    if next == *current {
        return;
    }
    state.replace(next.clone());
    emit("activity://updated", json!(next));
    *current = next;
}

/// Hook files are named by session id with no extension, so `usage::prune_old` (`.json` only)
/// cannot clean them.
fn prune_activity(dir: &Path, max_age: Duration) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .ok()
            .filter(|metadata| metadata.is_file())
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age > max_age);
        if old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

pub fn spawn_poller(bridge: Bridge, goko_home: PathBuf, emit: Emit) -> ActivityState {
    prune_activity(&goko_home.join("activity/claude"), CLAUDE_RETENTION);
    let codex_home = codex_home_from(std::env::var_os("CODEX_HOME"), std::env::var_os("HOME"));
    let state = ActivityState::default();
    let poll_state = state.clone();
    tauri::async_runtime::spawn(async move {
        let mut current = HashMap::new();
        let mut interval = tokio::time::interval(POLL_INTERVAL);
        loop {
            interval.tick().await;
            // A daemon error keeps the previous map; the next tick retries.
            let Ok(sessions) = bridge.session_list().await else {
                continue;
            };
            let next = compute_activity(&sessions, &goko_home, codex_home.as_deref(), &current);
            apply(next, &mut current, &poll_state, &emit);
        }
    });
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    fn event(kind: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "timestamp": "2026-11-05T15:42:35.501Z",
            "type": "event_msg",
            "payload": {"type": kind}
        }))
        .unwrap()
    }

    fn tail(lines: &[Vec<u8>]) -> Vec<u8> {
        let mut tail = Vec::new();
        for line in lines {
            tail.extend_from_slice(line);
            tail.push(b'\n');
        }
        tail
    }

    fn write_rollout(dir: &Path, name: &str, cwd: &str, timestamp: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(format!("rollout-{name}.jsonl"));
        let mut contents = serde_json::to_vec(&json!({
            "timestamp": timestamp,
            "type": "session_meta",
            "payload": {"id": name, "timestamp": timestamp, "cwd": cwd}
        }))
        .unwrap();
        contents.push(b'\n');
        contents.extend_from_slice(&event("task_started"));
        fs::write(&path, contents).unwrap();
        path
    }

    fn session<'a>(id: &'a str, cwd: &'a str, started_at_ms: i64) -> CodexSession<'a> {
        CodexSession {
            id,
            cwd,
            started_at_ms,
        }
    }

    fn meta(path: &str, cwd: &str, started_ms: i64) -> RolloutMeta {
        RolloutMeta {
            path: PathBuf::from(path),
            cwd: cwd.to_owned(),
            started_ms,
        }
    }

    #[test]
    fn parse_claude_activity_reads_the_hook_words() {
        assert_eq!(parse_claude_activity("working\n"), Some(Activity::Working));
        assert_eq!(parse_claude_activity("waiting"), Some(Activity::Waiting));
        assert_eq!(parse_claude_activity("done"), Some(Activity::Done));
    }

    #[test]
    fn parse_claude_activity_ignores_empty_and_unknown_text() {
        assert_eq!(parse_claude_activity(""), None);
        assert_eq!(parse_claude_activity(" \n"), None);
        assert_eq!(parse_claude_activity("banana"), None);
    }

    #[test]
    fn codex_tail_ending_in_task_complete_is_done() {
        let tail = tail(&[event("task_started"), event("task_complete")]);
        assert_eq!(codex_activity_from_tail(&tail), Some(Activity::Done));
    }

    #[test]
    fn codex_tail_ending_in_task_started_is_working() {
        let tail = tail(&[event("task_complete"), event("task_started")]);
        assert_eq!(codex_activity_from_tail(&tail), Some(Activity::Working));
    }

    #[test]
    fn codex_tail_without_task_events_has_no_activity() {
        let token_count = serde_json::to_vec(&json!({
            "type": "event_msg",
            "payload": {"type": "token_count", "info": null}
        }))
        .unwrap();
        assert_eq!(codex_activity_from_tail(&tail(&[token_count])), None);
        assert_eq!(codex_activity_from_tail(b""), None);
        assert_eq!(codex_activity_from_tail(b"not json\n"), None);
    }

    #[test]
    fn codex_tail_reads_past_a_truncated_first_line() {
        let mut bytes = b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_sta".to_vec();
        bytes.push(b'\n');
        bytes.extend_from_slice(&event("task_complete"));
        bytes.push(b'\n');
        assert_eq!(codex_activity_from_tail(&bytes), Some(Activity::Done));
    }

    #[test]
    fn codex_activity_reads_the_final_64_kib_of_a_rollout() {
        let dir = crate::test_support::TempDir::new("activity-tail");
        let path = dir.path().join("rollout-big.jsonl");
        let mut contents = vec![b'x'; CODEX_TAIL_BYTES as usize + 16];
        contents.push(b'\n');
        contents.extend_from_slice(&event("task_complete"));
        contents.push(b'\n');
        fs::write(&path, contents).unwrap();

        assert_eq!(codex_activity(&path), Some(Activity::Done));
    }

    #[test]
    fn rollout_meta_parses_the_session_meta_first_line() {
        let dir = crate::test_support::TempDir::new("activity-meta");
        let path = write_rollout(dir.path(), "abc", "/work/dir", "2026-11-05T15:42:35.501Z");

        let meta = rollout_meta(&path).unwrap();

        assert_eq!(meta.path, path);
        assert_eq!(meta.cwd, "/work/dir");
        assert_eq!(meta.started_ms, 1_793_893_355_501);
    }

    #[test]
    fn rollout_meta_reads_a_first_line_longer_than_16_kib() {
        let dir = crate::test_support::TempDir::new("activity-meta-long");
        let path = dir.path().join("rollout-long.jsonl");
        let mut contents = serde_json::to_vec(&json!({
            "type": "session_meta",
            "payload": {
                "id": "long",
                "timestamp": "2026-11-05T15:42:35.501Z",
                "cwd": "/big",
                "base_instructions": "x".repeat(20 * 1024)
            }
        }))
        .unwrap();
        contents.push(b'\n');
        contents.extend_from_slice(&event("task_started"));
        fs::write(&path, contents).unwrap();

        let meta = rollout_meta(&path).unwrap();

        assert_eq!(meta.cwd, "/big");
        assert_eq!(meta.started_ms, 1_793_893_355_501);
    }

    #[test]
    fn rollout_meta_rejects_a_file_that_does_not_start_with_session_meta() {
        let dir = crate::test_support::TempDir::new("activity-meta-none");
        let path = dir.path().join("rollout-x.jsonl");
        fs::write(&path, b"{\"type\":\"event_msg\"}\n").unwrap();
        assert!(rollout_meta(&path).is_none());
        fs::write(&path, b"").unwrap();
        assert!(rollout_meta(&path).is_none());
    }

    #[test]
    fn timestamp_ms_parses_rfc3339_utc() {
        assert_eq!(timestamp_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            timestamp_ms("2026-11-05T15:42:35.501Z"),
            Some(1_793_893_355_501)
        );
        assert_eq!(
            timestamp_ms("2024-02-29T12:34:56Z"),
            Some(1_709_210_096_000)
        );
    }

    #[test]
    fn timestamp_ms_rejects_other_shapes() {
        assert_eq!(timestamp_ms(""), None);
        assert_eq!(timestamp_ms("not a timestamp"), None);
        assert_eq!(timestamp_ms("2026-11-05"), None);
        assert_eq!(timestamp_ms("2026-11-05T15:42:35"), None);
        assert_eq!(timestamp_ms("2026-11-05T15:42:35+07:00"), None);
        assert_eq!(timestamp_ms("2026-13-05T15:42:35Z"), None);
        assert_eq!(timestamp_ms("2026-11-05T15:42:35.Z"), None);
    }

    #[test]
    fn match_claims_rollouts_in_session_start_order() {
        let sessions = [
            session("newer", "/project", 20_000),
            session("older", "/project", 10_000),
        ];
        let rollouts = [
            meta("/rollouts/second.jsonl", "/project", 15_000),
            meta("/rollouts/first.jsonl", "/project", 10_000),
        ];

        let matched = match_codex_rollouts(&sessions, &rollouts);

        assert_eq!(matched.len(), 2);
        assert_eq!(matched["older"], PathBuf::from("/rollouts/first.jsonl"));
        assert_eq!(matched["newer"], PathBuf::from("/rollouts/second.jsonl"));
    }

    #[test]
    fn match_skips_a_rollout_started_more_than_five_seconds_early() {
        let sessions = [session("s", "/project", 10_000)];
        let rollouts = [
            meta("/rollouts/too-early.jsonl", "/project", 4_000),
            meta("/rollouts/inside.jsonl", "/project", 6_000),
        ];

        let matched = match_codex_rollouts(&sessions, &rollouts);

        assert_eq!(matched["s"], PathBuf::from("/rollouts/inside.jsonl"));
    }

    #[test]
    fn match_requires_the_same_working_directory() {
        let sessions = [session("s", "/project/a", 10_000)];
        let rollouts = [meta("/rollouts/other.jsonl", "/project/b", 12_000)];
        assert!(match_codex_rollouts(&sessions, &rollouts).is_empty());
    }

    #[test]
    fn match_claims_a_rollout_at_most_once() {
        let sessions = [
            session("earlier", "/project", 10_000),
            session("later", "/project", 20_000),
        ];
        let rollouts = [meta("/rollouts/only.jsonl", "/project", 15_000)];

        let matched = match_codex_rollouts(&sessions, &rollouts);

        assert_eq!(matched.len(), 1);
        assert_eq!(matched["earlier"], PathBuf::from("/rollouts/only.jsonl"));
    }

    #[test]
    fn recent_rollouts_finds_metas_in_the_two_newest_day_directories() {
        let home = crate::test_support::TempDir::new("activity-recent");
        let newest = write_rollout(
            &home.path().join("sessions/2026/11/05"),
            "new",
            "/p",
            "2026-11-05T15:42:35.501Z",
        );
        let older = write_rollout(
            &home.path().join("sessions/2026/11/04"),
            "old",
            "/q",
            "2026-11-04T01:02:03Z",
        );
        write_rollout(
            &home.path().join("sessions/2026/11/03"),
            "outside",
            "/x",
            "2026-11-03T01:02:03Z",
        );

        let mut paths: Vec<PathBuf> = recent_rollouts(home.path())
            .into_iter()
            .map(|meta| meta.path)
            .collect();
        paths.sort();

        assert_eq!(paths, vec![older, newest]);
    }

    use protocol::session::SessionId;
    use std::sync::Mutex;

    fn running(argv: &[&str], cwd: &str, started_at: i64) -> SessionInfo {
        SessionInfo {
            id: SessionId::new(),
            title: "t".into(),
            argv: argv.iter().map(|arg| (*arg).to_owned()).collect(),
            cwd: cwd.into(),
            state: SessionState::Running,
            exit_code: None,
            error: None,
            started_at,
            ended_at: None,
        }
    }

    fn sid(session: &SessionInfo) -> String {
        session.id.as_uuid().to_string()
    }

    fn claude(id: &str) -> SessionInfo {
        running(
            &["/bin/claude", "--session-id", id, "--settings", "{}"],
            "/p",
            0,
        )
    }

    fn write_hook(home: &Path, id: &str, text: &str) {
        let dir = home.join("activity/claude");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(id), text).unwrap();
    }

    #[test]
    fn a_claude_session_follows_its_hook_file() {
        let home = crate::test_support::TempDir::new("act-claude");
        let session = claude("c1");
        let sessions = [session.clone()];

        write_hook(home.path(), "c1", "working");
        let first = compute_activity(&sessions, home.path(), None, &HashMap::new());
        assert_eq!(first, HashMap::from([(sid(&session), Activity::Working)]));

        write_hook(home.path(), "c1", "done");
        let second = compute_activity(&sessions, home.path(), None, &first);
        assert_eq!(second, HashMap::from([(sid(&session), Activity::Done)]));
    }

    #[test]
    fn a_missing_or_garbage_hook_file_keeps_the_previous_value_or_stays_absent() {
        let home = crate::test_support::TempDir::new("act-keep");
        let session = claude("c2");
        let sessions = [session.clone()];
        let previous = HashMap::from([(sid(&session), Activity::Waiting)]);

        assert!(compute_activity(&sessions, home.path(), None, &HashMap::new()).is_empty());
        assert_eq!(
            compute_activity(&sessions, home.path(), None, &previous),
            previous
        );
        write_hook(home.path(), "c2", "banana");
        assert_eq!(
            compute_activity(&sessions, home.path(), None, &previous),
            previous
        );
    }

    #[test]
    fn only_running_claude_and_codex_sessions_appear() {
        let home = crate::test_support::TempDir::new("act-only");
        write_hook(home.path(), "c3", "working");
        let mut exited = claude("c3");
        exited.state = SessionState::Exited;
        let sessions = [
            exited,
            running(&["/bin/zsh"], "/p", 0),
            running(&["/bin/opencode"], "/p", 0),
            running(&["/bin/devin"], "/p", 0),
            running(&["/bin/sh", "--session-id", "../../etc/passwd"], "/p", 0),
        ];

        assert!(compute_activity(&sessions, home.path(), None, &HashMap::new()).is_empty());
    }

    #[test]
    fn codex_sessions_in_one_directory_each_read_their_own_rollout() {
        let home = crate::test_support::TempDir::new("act-codex");
        let day = home.path().join("sessions/2026/11/05");
        let first = write_rollout(&day, "a", "/p", "2026-11-05T10:00:00Z");
        write_rollout(&day, "b", "/p", "2026-11-05T10:05:00Z");
        let mut done = fs::read(&first).unwrap();
        done.push(b'\n');
        done.extend_from_slice(&event("task_complete"));
        done.push(b'\n');
        fs::write(&first, done).unwrap();
        let start = |text: &str| timestamp_ms(text).unwrap();
        let early = running(&["/bin/codex"], "/p", start("2026-11-05T10:00:00Z"));
        let late = running(&["/bin/codex.js"], "/p", start("2026-11-05T10:05:00Z"));

        let activity = compute_activity(
            &[late.clone(), early.clone()],
            home.path(),
            Some(home.path()),
            &HashMap::new(),
        );

        assert_eq!(
            activity,
            HashMap::from([
                (sid(&early), Activity::Done),
                (sid(&late), Activity::Working)
            ])
        );
    }

    #[test]
    fn apply_emits_once_per_change_and_never_for_an_unchanged_map() {
        let emitted: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
        let sink = Arc::clone(&emitted);
        let emit: Emit = Arc::new(move |name, payload| {
            sink.lock().unwrap().push((name.to_owned(), payload));
        });
        let state = ActivityState::default();
        let mut current = HashMap::new();
        let next = HashMap::from([("s".to_owned(), Activity::Working)]);

        apply(next.clone(), &mut current, &state, &emit);
        apply(next.clone(), &mut current, &state, &emit);

        assert_eq!(state.snapshot(), next);
        assert_eq!(
            *emitted.lock().unwrap(),
            vec![("activity://updated".to_owned(), json!({"s": "working"}))]
        );
    }

    #[test]
    fn prune_activity_removes_only_old_files() {
        let dir = crate::test_support::TempDir::new("act-prune");
        let old = dir.path().join("old");
        let fresh = dir.path().join("fresh");
        fs::write(&old, "done").unwrap();
        fs::write(&fresh, "done").unwrap();
        File::open(&old)
            .unwrap()
            .set_times(
                fs::FileTimes::new()
                    .set_modified(SystemTime::now() - Duration::from_secs(8 * 24 * 60 * 60)),
            )
            .unwrap();

        prune_activity(dir.path(), CLAUDE_RETENTION);

        assert!(!old.exists() && fresh.exists());
    }
}
