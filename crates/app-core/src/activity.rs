//! The child process's activity: what the session's agent is doing right
//! now.
//!
//! A session's own state says whether its process runs; the *activity*
//! says whether the agent inside it is working, waiting for the user, or
//! done. Two sources answer that, both read from files the agent itself
//! writes:
//!
//! - **Claude**: the hooks `agents::argv_for` installs write
//!   `GOKO_HOME/activity/claude/<session-id>` with `working`, `waiting` or
//!   `done`.
//! - **Codex**: the rollout file's tail carries `task_started` /
//!   `task_complete` events.
//!
//! The desktop shell reads the same files (its own copy lives in
//! `src-tauri/src/activity.rs`); this module is the shared one.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;
use serde_json::{json, Value};

/// How much of a rollout's tail is read: the last task event is near the
/// end.
pub const CODEX_TAIL_BYTES: u64 = 64 * 1024;

/// What the session's child process is doing.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Activity {
    /// A task is running.
    Working,
    /// The agent is waiting for the user.
    Waiting,
    /// The task finished.
    Done,
}

/// The word a Claude hook wrote to its activity file. Empty, unknown, or
/// mid-write content means "no reading": the caller keeps the previous
/// value.
pub fn parse_claude_activity(text: &str) -> Option<Activity> {
    match text.trim() {
        "working" => Some(Activity::Working),
        "waiting" => Some(Activity::Waiting),
        "done" => Some(Activity::Done),
        _ => None,
    }
}

/// The activity of a Codex rollout tail: the last `task_started` /
/// `task_complete` event wins. Lines that are not JSON (the read can start
/// mid-line) or other events are ignored.
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
                Some("task_complete" | "turn_aborted") => Some(Activity::Done),
                _ => None,
            }
        })
}

/// The activity of the session writing `path`, read from its final 64 KiB.
pub fn codex_activity(path: &Path) -> Option<Activity> {
    let mut file = File::open(path).ok()?;
    let start = file.metadata().ok()?.len().saturating_sub(CODEX_TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    codex_activity_from_tail(&tail)
}

/// The `session_meta` line that opens a rollout file. `read_line` grows
/// its buffer, so a `base_instructions` field of tens of KB still reads as
/// one line without touching the rest.
pub struct RolloutMeta {
    pub path: PathBuf,
    pub cwd: String,
    pub started_ms: i64,
}

/// The rollout's own metadata: where it ran and when it started.
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

/// Rollout metas from the two newest day directories under
/// `<codex_home>/sessions`, so a rollout that crossed midnight still shows
/// up.
pub fn recent_rollouts(codex_home: &Path) -> Vec<RolloutMeta> {
    rollouts_since(codex_home, None)
}

/// The most day directories one scan reads, however old the session is.
const MAX_ROLLOUT_DAYS: usize = 31;

/// Rollout metas a session started at `since_ms` (unix milliseconds) can own:
/// the two newest day directories, and every older one back to the day
/// before that start. A session lives in the daemon across app restarts, so
/// one started days ago still writes to the rollout in its own day's
/// directory. The extra day covers a directory named by local time.
pub fn rollouts_since(codex_home: &Path, since_ms: Option<i64>) -> Vec<RolloutMeta> {
    let since_day = since_ms.map(|ms| ms.div_euclid(86_400_000) - 1);
    let mut days = Vec::new();
    collect_days(&codex_home.join("sessions"), since_day, &mut days);
    days.iter()
        .flat_map(|day| rollouts_in(day))
        .filter_map(|(path, _)| rollout_meta(&path))
        .collect()
}

/// A session the activity reader is asked about.
#[derive(Clone, Debug)]
pub struct SessionActivity {
    /// The session's id (the daemon's uuid as a string).
    pub id: String,
    /// The session's argv — it names the agent (`claude`, `codex`, …).
    pub argv: Vec<String>,
    /// The session's working directory.
    pub cwd: String,
    /// When the session started, in unix milliseconds.
    pub started_at_ms: i64,
    /// Whether the session's process runs.
    pub running: bool,
    /// Whether the session was interrupted (Esc/Ctrl-C): a `working` file
    /// left over from before the interrupt reads as done.
    pub interrupted: bool,
}

/// Goko session id -> the rollout it owns. Sessions claim in
/// `started_at_ms` order (ties by id); each takes the earliest unclaimed
/// rollout (ties by path) with the same `cwd` whose `session_meta`
/// timestamp is not earlier than the session start minus 5 s of clock
/// slack.
pub fn match_codex_rollouts(
    sessions: &[SessionActivity],
    rollouts: &[RolloutMeta],
) -> HashMap<String, PathBuf> {
    let mut ordered: Vec<&SessionActivity> = sessions.iter().collect();
    ordered.sort_by(|left, right| {
        left.started_at_ms
            .cmp(&right.started_at_ms)
            .then_with(|| left.id.cmp(&right.id))
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
                && rollout.cwd == session.cwd
                && rollout.started_ms >= session.started_at_ms - 5_000
        }) else {
            continue;
        };
        claimed.insert(rollout.path.clone());
        matched.insert(session.id.to_owned(), rollout.path.clone());
    }
    matched
}

/// Session id -> activity, for running Claude and Codex sessions only. A
/// session whose source cannot be read this tick keeps its `previous`
/// value, so a hook file caught mid-write or a rollout between turns never
/// flickers the dot.
pub fn compute_activity(
    sessions: &[SessionActivity],
    goko_home: &Path,
    codex_home: Option<&Path>,
    previous: &HashMap<String, Activity>,
) -> HashMap<String, Activity> {
    let mut next = HashMap::new();
    let mut codex: Vec<SessionActivity> = Vec::new();
    for session in sessions.iter().filter(|session| session.running) {
        if let Some(claude_id) = claude_session_id(&session.argv) {
            let path = goko_home.join("activity/claude").join(claude_id);
            let read = fs::read_to_string(&path)
                .ok()
                .and_then(|text| parse_claude_activity(&text))
                .map(|activity| {
                    // A `working` file left over from before the
                    // interrupt reads as done.
                    if activity == Activity::Working && session.interrupted {
                        Activity::Done
                    } else {
                        activity
                    }
                });
            if let Some(activity) = read.or_else(|| previous.get(&session.id).copied()) {
                next.insert(session.id.clone(), activity);
            }
        } else if is_codex(&session.argv) {
            codex.push(session.clone());
        }
    }
    // The rollouts are read only for a Codex session, and back to the day the
    // oldest one started.
    let since = codex.iter().map(|session| session.started_at_ms).min();
    let matched = codex_home
        .filter(|_| !codex.is_empty())
        .map(|home| match_codex_rollouts(&codex, &rollouts_since(home, since)))
        .unwrap_or_default();
    for session in codex {
        let read = matched
            .get(&session.id)
            .and_then(|path| codex_activity(path));
        if let Some(activity) = read.or_else(|| previous.get(&session.id).copied()) {
            next.insert(session.id.clone(), activity);
        }
    }
    next
}

/// The value after `--session-id`, which `agents::argv_for` passes to
/// Claude. A value with a path separator is not an id Goko wrote, so it
/// never names a file.
fn claude_session_id(argv: &[String]) -> Option<&str> {
    let id = argv.get(argv.iter().position(|arg| arg == "--session-id")? + 1)?;
    (!id.is_empty() && !id.contains('/')).then_some(id.as_str())
}

/// Goko launches Codex as its bare resolved path, so a lone argv whose
/// stem is `codex` (`codex.js` for an npm install) is a Codex session.
pub fn is_codex(argv: &[String]) -> bool {
    fn stem_is_codex(arg: &str) -> bool {
        Path::new(arg)
            .file_stem()
            .is_some_and(|stem| stem == "codex")
    }
    match argv {
        [bin] => stem_is_codex(bin),
        [first, rest @ ..]
            if Path::new(first)
                .file_name()
                .is_some_and(|name| name == "env") =>
        {
            rest.iter()
                .position(|arg| !arg.contains('='))
                .is_some_and(|bin| bin == rest.len() - 1 && stem_is_codex(&rest[bin]))
        }
        _ => false,
    }
}

/// `<codex_home>`: `$CODEX_HOME`, else `~/.codex`.
pub fn codex_home_from(codex_home: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    codex_home
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            home.filter(|path| !path.is_empty())
                .map(PathBuf::from)
                .map(|home| home.join(".codex"))
        })
}

/// Rollouts live in `sessions/YYYY/MM/DD/`; only the two newest day
/// directories are scanned.
pub fn collect_recent_days(dir: &Path, days: &mut Vec<PathBuf>) {
    collect_days(dir, None, days);
}

/// Day directories with rollouts, newest first: the two newest, then older
/// ones while their day is `since_day` (days since the unix epoch) or later,
/// up to `MAX_ROLLOUT_DAYS`. Returns whether the walk may go on.
fn collect_days(dir: &Path, since_day: Option<i64>, days: &mut Vec<PathBuf>) -> bool {
    if days.len() >= 2 && since_day.is_none() {
        return false;
    }
    if !rollouts_in(dir).is_empty() {
        // Directories come newest first, so the first one too old ends the walk.
        let wanted = days.len() < 2
            || (days.len() < MAX_ROLLOUT_DAYS
                && since_day.is_some_and(|since| day_of(dir).is_some_and(|day| day >= since)));
        if !wanted {
            return false;
        }
        days.push(dir.to_owned());
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return true;
    };
    let mut children: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    children.sort_unstable_by(|left, right| right.cmp(left));
    children
        .iter()
        .all(|child| collect_days(child, since_day, days))
}

/// The day a `…/YYYY/MM/DD` directory names, in days since the unix epoch.
fn day_of(dir: &Path) -> Option<i64> {
    let mut parts = dir.components().rev().map(|part| {
        part.as_os_str()
            .to_str()
            .and_then(|text| text.parse::<i64>().ok())
    });
    let (day, month, year) = (parts.next()??, parts.next()??, parts.next()??);
    ((1..=12).contains(&month) && (1..=31).contains(&day))
        .then(|| days_from_civil(year, month, day))
}

/// The rollout files in one directory, with their modification times.
pub fn rollouts_in(dir: &Path) -> Vec<(PathBuf, SystemTime)> {
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

/// `YYYY-MM-DDTHH:MM:SS[.fff…]Z` -> unix milliseconds, hand-parsed so this
/// module needs no date crate. Anything outside that exact shape is
/// `None`.
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
            let mut fraction = 0i64;
            for (index, byte) in digits.iter().enumerate().take(3) {
                fraction += i64::from(byte - b'0') * 10i64.pow(2 - index as u32);
            }
            Some(millis + fraction)
        }
        _ => None,
    }
}

/// Days since the unix epoch for a civil date (Howard Hinnant's
/// algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_shift = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_shift + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The activity of one session, for the shell's own use.
pub fn activity_for(
    session: &SessionActivity,
    goko_home: &Path,
    codex_home: Option<&Path>,
    previous: Option<Activity>,
) -> Option<Activity> {
    if let Some(claude_id) = claude_session_id(&session.argv) {
        let path = goko_home.join("activity/claude").join(claude_id);
        return fs::read_to_string(&path)
            .ok()
            .and_then(|text| parse_claude_activity(&text))
            .or(previous);
    }
    if is_codex(&session.argv) {
        let rollouts = rollouts_since(codex_home?, Some(session.started_at_ms));
        let matched = match_codex_rollouts(std::slice::from_ref(session), &rollouts);
        return matched
            .get(&session.id)
            .and_then(|path| codex_activity(path))
            .or(previous);
    }
    previous
}

/// The activity map the shells read, as JSON.
pub fn activity_json(activity: &HashMap<String, Activity>) -> Value {
    json!(activity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, SystemTime};

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "goko-activity-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_codex_account_is_recognised_behind_its_env_prefix() {
        // A named account runs the agent through `env NAME=value bin`.
        assert!(is_codex(&["codex".into()]));
        assert!(is_codex(&[
            "env".into(),
            "CODEX_HOME=/x".into(),
            "/bin/codex".into()
        ]));
        assert!(is_codex(&[
            "/usr/bin/env".into(),
            "CODEX_HOME=/x".into(),
            "/bin/codex".into()
        ]));
        // Not codex.
        assert!(!is_codex(&[
            "env".into(),
            "CLAUDE_CONFIG_DIR=/x".into(),
            "/bin/claude".into(),
            "--session-id".into(),
            "a".into()
        ]));
        assert!(!is_codex(&["codex".into(), "--flag".into()]));
        assert!(!is_codex(&["env".into(), "CODEX_HOME=/x".into()]));
    }

    #[test]
    fn an_interrupted_session_reads_as_done() {
        let home = temp_dir("interrupted");
        fs::create_dir_all(home.join("activity/claude")).unwrap();
        fs::write(home.join("activity/claude/s1"), "working").unwrap();
        let session = SessionActivity {
            id: "s1".into(),
            argv: vec!["claude".into(), "--session-id".into(), "s1".into()],
            cwd: "/tmp".into(),
            started_at_ms: 0,
            running: true,
            interrupted: true,
        };
        let activity = compute_activity(&[session], &home, None, &HashMap::new());
        assert_eq!(activity.get("s1"), Some(&Activity::Done));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn a_claude_hook_file_names_the_activity() {
        assert_eq!(parse_claude_activity("working"), Some(Activity::Working));
        assert_eq!(parse_claude_activity("waiting\n"), Some(Activity::Waiting));
        assert_eq!(parse_claude_activity("done"), Some(Activity::Done));
        assert_eq!(parse_claude_activity(""), None);
        assert_eq!(parse_claude_activity("half-written"), None);
    }

    #[test]
    fn a_codex_tail_answers_with_the_last_task_event() {
        let tail = b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n";
        assert_eq!(codex_activity_from_tail(tail), Some(Activity::Done));
        let tail = b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n";
        assert_eq!(codex_activity_from_tail(tail), Some(Activity::Working));
        // A line that is not JSON (the read can start mid-line) is ignored.
        assert_eq!(codex_activity_from_tail(b"not json\n"), None);
    }

    #[test]
    fn a_claude_session_reads_its_hook_file() {
        let home = temp_dir("claude");
        let id = "session-1";
        fs::create_dir_all(home.join("activity/claude")).unwrap();
        fs::write(home.join("activity/claude").join(id), "working").unwrap();
        let session = SessionActivity {
            id: "s1".into(),
            argv: vec!["claude".into(), "--session-id".into(), id.into()],
            cwd: "/tmp".into(),
            started_at_ms: 0,
            running: true,
            interrupted: false,
        };
        let activity = compute_activity(&[session], &home, None, &HashMap::new());
        assert_eq!(activity.get("s1"), Some(&Activity::Working));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn a_session_whose_source_cannot_be_read_keeps_its_previous_activity() {
        let home = temp_dir("previous");
        let session = SessionActivity {
            id: "s1".into(),
            argv: vec!["claude".into(), "--session-id".into(), "gone".into()],
            cwd: "/tmp".into(),
            started_at_ms: 0,
            running: true,
            interrupted: false,
        };
        let previous = HashMap::from([("s1".to_string(), Activity::Working)]);
        let activity = compute_activity(&[session], &home, None, &previous);
        assert_eq!(activity.get("s1"), Some(&Activity::Working));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn a_stopped_session_has_no_activity() {
        let home = temp_dir("stopped");
        let session = SessionActivity {
            id: "s1".into(),
            argv: vec!["claude".into(), "--session-id".into(), "x".into()],
            cwd: "/tmp".into(),
            started_at_ms: 0,
            running: false,
            interrupted: false,
        };
        let activity = compute_activity(&[session], &home, None, &HashMap::new());
        assert!(activity.is_empty());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn the_codex_home_falls_back_to_the_dot_codex_directory() {
        assert_eq!(
            codex_home_from(Some("/opt/codex"), Some("/home/me")),
            Some(PathBuf::from("/opt/codex"))
        );
        assert_eq!(
            codex_home_from(None, Some("/home/me")),
            Some(PathBuf::from("/home/me/.codex"))
        );
        assert_eq!(codex_home_from(None, None), None);
    }

    #[test]
    fn a_rollout_is_matched_to_the_session_that_started_it() {
        let dir = temp_dir("rollout");
        let day = dir.join("sessions/2026/11/10");
        fs::create_dir_all(&day).unwrap();
        let path = day.join("rollout-1.jsonl");
        let meta = json!({
            "type": "session_meta",
            "payload": { "cwd": "/work/app", "timestamp": "2026-11-10T10:00:00.000Z" }
        });
        fs::write(&path, format!("{meta}\n")).unwrap();
        let rollouts = recent_rollouts(&dir);
        assert_eq!(rollouts.len(), 1);
        assert_eq!(rollouts[0].cwd, "/work/app");

        let session = SessionActivity {
            id: "s1".into(),
            argv: vec!["/opt/codex".into()],
            cwd: "/work/app".into(),
            started_at_ms: rollouts[0].started_ms - 1_000,
            running: true,
            interrupted: false,
        };
        let matched = match_codex_rollouts(&[session], &rollouts);
        assert_eq!(matched.get("s1"), Some(&path));
        let _ = fs::remove_dir_all(&dir);
    }

    /// A session that started three days ago still owns the rollout in its
    /// own day's directory, behind two newer days of other rollouts.
    #[test]
    fn an_old_session_finds_its_rollout_behind_newer_days() {
        let dir = temp_dir("old-session");
        let write = |day: &str, name: &str, cwd: &str| {
            let folder = dir.join("sessions").join(day);
            fs::create_dir_all(&folder).unwrap();
            let meta = json!({
                "type": "session_meta",
                "payload": { "cwd": cwd, "timestamp": format!("{}T10:00:00.000Z", day.replace('/', "-")) }
            });
            let path = folder.join(name);
            fs::write(&path, format!("{meta}\n")).unwrap();
            path
        };
        let stale = write("2026/11/08", "rollout-0.jsonl", "/work/app");
        let own = write("2026/11/10", "rollout-1.jsonl", "/work/app");
        write("2026/11/12", "rollout-2.jsonl", "/work/other");
        write("2026/11/13", "rollout-3.jsonl", "/work/other");

        // The two newest days alone miss it.
        assert!(recent_rollouts(&dir).iter().all(|meta| meta.path != own));

        let started = timestamp_ms("2026-11-10T09:59:58.000Z").unwrap();
        let found = rollouts_since(&dir, Some(started));
        assert!(found.iter().any(|meta| meta.path == own));
        assert!(
            found.iter().all(|meta| meta.path != stale),
            "days before the session started are not read"
        );

        let session = SessionActivity {
            id: "s1".into(),
            argv: vec!["/opt/codex".into()],
            cwd: "/work/app".into(),
            started_at_ms: started,
            running: true,
            interrupted: false,
        };
        let goko_home = dir.join("goko");
        let activity = compute_activity(
            std::slice::from_ref(&session),
            &goko_home,
            Some(&dir),
            &HashMap::new(),
        );
        assert_eq!(
            match_codex_rollouts(&[session], &found).get("s1"),
            Some(&own)
        );
        // The rollout holds no turn yet, so there is no activity to report.
        assert!(activity.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rollout_of_another_directory_is_not_matched() {
        let dir = temp_dir("other");
        let day = dir.join("sessions/2026/11/10");
        fs::create_dir_all(&day).unwrap();
        let path = day.join("rollout-1.jsonl");
        let meta = json!({
            "type": "session_meta",
            "payload": { "cwd": "/somewhere/else", "timestamp": "2026-11-10T10:00:00.000Z" }
        });
        fs::write(&path, format!("{meta}\n")).unwrap();
        let rollouts = recent_rollouts(&dir);
        let session = SessionActivity {
            id: "s1".into(),
            argv: vec!["/opt/codex".into()],
            cwd: "/work/app".into(),
            started_at_ms: 0,
            running: true,
            interrupted: false,
        };
        assert!(match_codex_rollouts(&[session], &rollouts).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_timestamp_outside_the_shape_is_no_timestamp() {
        assert!(timestamp_ms("2026-11-10T10:00:00Z").is_some());
        assert!(timestamp_ms("2026-11-10 10:00:00Z").is_none());
        assert!(timestamp_ms("nonsense").is_none());
    }

    #[test]
    fn a_rollout_file_is_found_under_the_two_newest_days() {
        let dir = temp_dir("days");
        let old = dir.join("sessions/2026/01/01");
        let older = dir.join("sessions/2025/01/01");
        let new = dir.join("sessions/2026/11/10");
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(&older).unwrap();
        fs::create_dir_all(&new).unwrap();
        let meta = json!({
            "type": "session_meta",
            "payload": { "cwd": "/work/app", "timestamp": "2026-11-10T10:00:00.000Z" }
        });
        fs::write(new.join("rollout-3.jsonl"), format!("{meta}\n")).unwrap();
        fs::write(old.join("rollout-2.jsonl"), format!("{meta}\n")).unwrap();
        fs::write(older.join("rollout-1.jsonl"), format!("{meta}\n")).unwrap();
        let rollouts = recent_rollouts(&dir);
        assert_eq!(rollouts.len(), 2, "only the two newest days are scanned");
        // Touch it so the file system is happy with the clock.
        let _ = fs::OpenOptions::new()
            .write(true)
            .open(new.join("rollout-2.jsonl"))
            .map(|_| ())
            .map(|_| SystemTime::now() - Duration::from_secs(1));
        let _ = fs::remove_dir_all(&dir);
    }
}
