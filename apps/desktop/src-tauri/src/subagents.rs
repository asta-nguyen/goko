use crate::bridge::Bridge;
use crate::events::Emit;
use protocol::session::{SessionInfo, SessionState};
use serde::Serialize;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tauri::State;

const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// `command` is truncated to this many characters (not bytes) in the published map.
const COMMAND_LIMIT: usize = 200;

/// One agent CLI running under a session's process tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Subagent {
    pub agent: String,
    pub pid: u32,
    pub command: String,
}

/// Goko session id -> the agent CLIs running under that session.
pub type SubagentMap = HashMap<String, Vec<Subagent>>;

#[derive(Clone, Default)]
pub struct SubagentState {
    map: Arc<RwLock<SubagentMap>>,
}

impl SubagentState {
    fn snapshot(&self) -> SubagentMap {
        self.map
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn replace(&self, map: SubagentMap) {
        *self
            .map
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = map;
    }
}

#[tauri::command]
pub fn subagents_get(state: State<'_, SubagentState>) -> SubagentMap {
    state.snapshot()
}

/// One row of `ps -axww -o pid=,ppid=,command=` output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessRow {
    pub pid: u32,
    pub ppid: u32,
    /// The full command line; it may itself contain spaces.
    pub command: String,
}

/// Parses `ps` output: two integers, then the rest of the line as the command.
/// Empty lines and lines that cannot be parsed are skipped.
pub fn parse_ps(text: &str) -> Vec<ProcessRow> {
    text.lines()
        .filter_map(|line| {
            let (pid, rest) = line.trim().split_once(char::is_whitespace)?;
            let (ppid, command) = rest.trim_start().split_once(char::is_whitespace)?;
            let command = command.trim();
            if command.is_empty() {
                return None;
            }
            Some(ProcessRow {
                pid: pid.parse().ok()?,
                ppid: ppid.parse().ok()?,
                command: command.to_owned(),
            })
        })
        .collect()
}

/// Every process below `root` in the `ppid` tree, at any depth; `root` itself is
/// never included. Each pid is visited once, so a `ppid` cycle cannot loop.
pub fn descendants(rows: &[ProcessRow], root: u32) -> Vec<&ProcessRow> {
    let mut children: HashMap<u32, Vec<&ProcessRow>> = HashMap::new();
    for row in rows {
        children.entry(row.ppid).or_default().push(row);
    }
    let mut found = Vec::new();
    let mut seen = HashSet::from([root]);
    let mut queue = vec![root];
    while let Some(pid) = queue.pop() {
        let Some(kids) = children.get(&pid) else {
            continue;
        };
        for row in kids {
            if seen.insert(row.pid) {
                found.push(*row);
                queue.push(row.pid);
            }
        }
    }
    found
}

/// The agent CLI `command` launches, when its program is a known agent: the file
/// stem of the first word — or of the word after `node`/`bun`, the JS-runtime
/// shape. A wrapper like `zsh -c '… devin …'` has program `zsh` and does not match.
pub fn agent_of(command: &str) -> Option<&'static str> {
    fn stem(word: &str) -> Option<&str> {
        Path::new(word).file_stem()?.to_str()
    }
    let mut words = command.split_whitespace();
    let first = stem(words.next()?)?;
    let program = if first == "node" || first == "bun" {
        stem(words.next()?)?
    } else {
        first
    };
    match program {
        "claude" => Some("claude"),
        "codex" => Some("codex"),
        "opencode" => Some("opencode"),
        "devin" => Some("devin"),
        _ => None,
    }
}

/// Session id -> agent CLIs under each running session's pid. A session that is
/// not running, has no pid, or spawns no agent CLI gets no entry.
pub fn compute_subagents(sessions: &[SessionInfo], rows: &[ProcessRow]) -> SubagentMap {
    let mut map = SubagentMap::new();
    for session in sessions {
        if session.state != SessionState::Running {
            continue;
        }
        let Some(pid) = session.pid else {
            continue;
        };
        let mut subagents: Vec<Subagent> = descendants(rows, pid)
            .into_iter()
            .filter_map(|row| {
                agent_of(&row.command).map(|agent| Subagent {
                    agent: agent.to_owned(),
                    pid: row.pid,
                    command: row.command.chars().take(COMMAND_LIMIT).collect(),
                })
            })
            .collect();
        subagents.sort_by_key(|subagent| subagent.pid);
        if !subagents.is_empty() {
            map.insert(session.id.as_uuid().to_string(), subagents);
        }
    }
    map
}

/// The map for this tick. A failed session listing or process-table read keeps
/// `previous`; when no running session has a pid the answer is empty and the
/// table is not read at all.
async fn next_map<F, Fut>(
    sessions: Option<Vec<SessionInfo>>,
    table: F,
    previous: &SubagentMap,
) -> SubagentMap
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Option<Vec<ProcessRow>>>,
{
    let Some(sessions) = sessions else {
        return previous.clone();
    };
    let any_running = sessions
        .iter()
        .any(|session| session.state == SessionState::Running && session.pid.is_some());
    if !any_running {
        return SubagentMap::new();
    }
    match table().await {
        Some(rows) => compute_subagents(&sessions, &rows),
        None => previous.clone(),
    }
}

/// The process table as rows; `None` when `ps` cannot start or exits non-zero.
async fn process_table() -> Option<Vec<ProcessRow>> {
    let output = tokio::process::Command::new("ps")
        .args(["-axww", "-o", "pid=,ppid=,command="])
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(parse_ps(&String::from_utf8_lossy(&output.stdout)))
}

/// Store and announce `next` only when it differs from `current`.
fn apply(next: SubagentMap, current: &mut SubagentMap, state: &SubagentState, emit: &Emit) {
    if next == *current {
        return;
    }
    state.replace(next.clone());
    emit("subagents://updated", json!(next));
    *current = next;
}

pub fn spawn_poller(bridge: Bridge, emit: Emit) -> SubagentState {
    let state = SubagentState::default();
    let poll_state = state.clone();
    tauri::async_runtime::spawn(async move {
        let mut current = SubagentMap::new();
        let mut interval = tokio::time::interval(POLL_INTERVAL);
        loop {
            interval.tick().await;
            // A daemon or `ps` error keeps the previous map; the next tick retries.
            let sessions = bridge.session_list().await.ok();
            let next = next_map(sessions, process_table, &current).await;
            apply(next, &mut current, &poll_state, &emit);
        }
    });
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Storage;
    use crate::test_support::{self, Home, TempDir};
    use protocol::session::SessionId;
    use serde_json::{json, Value};
    use std::cell::Cell;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    fn row(pid: u32, ppid: u32, command: &str) -> ProcessRow {
        ProcessRow {
            pid,
            ppid,
            command: command.to_owned(),
        }
    }

    fn info(state: SessionState, pid: Option<u32>) -> SessionInfo {
        SessionInfo {
            id: SessionId::new(),
            title: "t".into(),
            argv: vec!["/bin/sh".into()],
            cwd: "/p".into(),
            state,
            pid,
            exit_code: None,
            error: None,
            started_at: 0,
            ended_at: None,
        }
    }

    fn sid(session: &SessionInfo) -> String {
        session.id.as_uuid().to_string()
    }

    fn subagent(agent: &str, pid: u32, command: &str) -> Subagent {
        Subagent {
            agent: agent.to_owned(),
            pid,
            command: command.to_owned(),
        }
    }

    fn previous_map() -> SubagentMap {
        SubagentMap::from([("old".to_owned(), vec![subagent("codex", 7, "codex")])])
    }

    #[test]
    fn parse_ps_reads_a_command_with_spaces_and_skips_bad_lines() {
        let text = "\
  101    1 /sbin/launchd
  202  101 claude --resume yesterday
not a row
  303  202

";
        let rows = parse_ps(text);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].pid, 101);
        assert_eq!(rows[0].ppid, 1);
        assert_eq!(rows[0].command, "/sbin/launchd");
        assert_eq!(rows[1].pid, 202);
        assert_eq!(rows[1].ppid, 101);
        assert_eq!(rows[1].command, "claude --resume yesterday");
    }

    #[test]
    fn descendants_finds_children_at_depth_one_and_three_but_never_the_root() {
        let rows = vec![
            row(100, 1, "zsh -l"),
            row(200, 100, "sh -c work"),
            row(300, 200, "node server.js"),
            row(400, 300, "devin --model x"),
            row(500, 999, "claude"),
        ];

        let found = descendants(&rows, 100);
        let mut pids: Vec<u32> = found.iter().map(|row| row.pid).collect();
        pids.sort_unstable();

        assert_eq!(pids, vec![200, 300, 400]);
    }

    #[test]
    fn descendants_terminates_on_a_ppid_cycle() {
        // 10's ppid is 20 and 20's ppid is 10: the walk must not loop, and the
        // root row itself is never returned even though the cycle reaches it.
        let rows = vec![
            row(10, 20, "claude --main"),
            row(20, 10, "sh"),
            row(30, 20, "devin"),
        ];

        let found = descendants(&rows, 10);
        let mut pids: Vec<u32> = found.iter().map(|row| row.pid).collect();
        pids.sort_unstable();

        assert_eq!(pids, vec![20, 30]);
    }

    #[test]
    fn agent_of_matches_known_agent_commands() {
        assert_eq!(
            agent_of("devin --model x -p --prompt-file /tmp/a.md"),
            Some("devin")
        );
        assert_eq!(
            agent_of("/opt/homebrew/Caskroom/claude-code/2.1.311/claude --resume y"),
            Some("claude")
        );
        assert_eq!(agent_of("node /usr/lib/codex.js"), Some("codex"));
        assert_eq!(agent_of("bun /x/opencode"), Some("opencode"));
    }

    #[test]
    fn agent_of_ignores_wrappers_other_programs_and_empty_commands() {
        assert_eq!(agent_of("/bin/zsh -c eval 'devin x'"), None);
        assert_eq!(agent_of("node server.js"), None);
        assert_eq!(agent_of("caffeinate -i"), None);
        assert_eq!(agent_of(""), None);
    }

    #[test]
    fn compute_subagents_groups_running_sessions_and_skips_the_rest() {
        let first = info(SessionState::Running, Some(100));
        let second = info(SessionState::Running, Some(200));
        let exited = info(SessionState::Exited, Some(300));
        let no_pid = info(SessionState::Running, None);
        let idle = info(SessionState::Idle, Some(400));
        let rows = vec![
            // The session's own process is an agent CLI but is never its own sub-agent.
            row(100, 1, "claude --session-id a"),
            row(101, 100, "devin --model x"),
            row(200, 1, "zsh"),
            row(201, 200, "codex"),
            row(202, 201, "opencode serve"),
            row(301, 300, "devin"),
            row(999, 1, "devin"),
        ];

        let map = compute_subagents(
            &[first.clone(), second.clone(), exited, no_pid, idle],
            &rows,
        );

        assert_eq!(map.len(), 2);
        assert_eq!(
            map[&sid(&first)],
            vec![subagent("devin", 101, "devin --model x")]
        );
        assert_eq!(
            map[&sid(&second)],
            vec![
                subagent("codex", 201, "codex"),
                subagent("opencode", 202, "opencode serve"),
            ]
        );
    }

    #[test]
    fn compute_subagents_truncates_commands_to_200_characters() {
        let session = info(SessionState::Running, Some(100));
        let long = format!("devin {}", "é".repeat(300));
        let rows = vec![row(101, 100, &long)];

        let map = compute_subagents(&[session.clone()], &rows);
        let subs = &map[&sid(&session)];

        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].command.chars().count(), 200);
        assert_eq!(subs[0].command, long.chars().take(200).collect::<String>());
    }

    #[tokio::test]
    async fn next_map_keeps_the_previous_map_when_the_session_list_fails() {
        let previous = previous_map();
        let called = Cell::new(false);

        let next = next_map(
            None,
            || {
                called.set(true);
                async { None }
            },
            &previous,
        )
        .await;

        assert_eq!(next, previous);
        assert!(!called.get());
    }

    #[tokio::test]
    async fn next_map_returns_empty_without_reading_the_table_when_no_session_has_a_pid() {
        let previous = previous_map();
        let called = Cell::new(false);
        let sessions = vec![
            info(SessionState::Exited, Some(10)),
            info(SessionState::Running, None),
        ];

        let next = next_map(
            Some(sessions),
            || {
                called.set(true);
                async { None }
            },
            &previous,
        )
        .await;

        assert!(next.is_empty());
        assert!(!called.get());
    }

    #[tokio::test]
    async fn next_map_keeps_the_previous_map_when_the_process_table_fails() {
        let session = info(SessionState::Running, Some(100));
        let previous = previous_map();

        let next = next_map(Some(vec![session]), || async { None }, &previous).await;

        assert_eq!(next, previous);
    }

    #[tokio::test]
    async fn next_map_computes_the_map_when_sessions_and_processes_are_read() {
        let session = info(SessionState::Running, Some(100));
        let rows = vec![row(100, 1, "claude"), row(101, 100, "devin -p x")];

        let next = next_map(
            Some(vec![session.clone()]),
            || async move { Some(rows) },
            &SubagentMap::new(),
        )
        .await;

        assert_eq!(
            next,
            SubagentMap::from([(sid(&session), vec![subagent("devin", 101, "devin -p x")])])
        );
    }

    #[test]
    fn apply_emits_once_per_change_and_never_for_an_unchanged_map() {
        let emitted: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
        let sink = Arc::clone(&emitted);
        let emit: Emit = Arc::new(move |name, payload| {
            sink.lock().unwrap().push((name.to_owned(), payload));
        });
        let state = SubagentState::default();
        let mut current = SubagentMap::new();
        let next = SubagentMap::from([(
            "s".to_owned(),
            vec![subagent("devin", 42, "devin --model x")],
        )]);

        apply(next.clone(), &mut current, &state, &emit);
        apply(next.clone(), &mut current, &state, &emit);

        assert_eq!(state.snapshot(), next);
        assert_eq!(
            *emitted.lock().unwrap(),
            vec![(
                "subagents://updated".to_owned(),
                json!({"s": [{"agent": "devin", "pid": 42, "command": "devin --model x"}]})
            )]
        );
    }

    #[tokio::test]
    async fn a_real_devin_process_under_a_session_is_reported_and_then_dropped() {
        let dir = TempDir::new("subagents-e2e");
        let bin = dir.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        // `exec -a` runs the real `/bin/sleep` under the argv[0] of `bin/devin`:
        // macOS kills copies of platform binaries, so the copy itself cannot exec.
        // `agent_of` matches on the file stem of the first ps word.
        let devin = bin.join("devin");
        fs::copy("/bin/sleep", &devin).unwrap();
        fs::set_permissions(&devin, fs::Permissions::from_mode(0o755)).unwrap();
        let work = dir.path().join("work");
        fs::create_dir_all(&work).unwrap();

        let home = Home::new("subagents-e2e");
        let bridge = Bridge::new(home.path().to_owned());
        let storage = Storage::open(home.path());
        let inner = format!("exec -a {} /bin/sleep 20 & wait", devin.display());
        let command = format!("sh -c {}", shlex::try_quote(&inner).unwrap());
        let session = bridge
            .session_create(
                &storage,
                test_support::create_args(&command, &work.display().to_string()),
            )
            .await
            .unwrap();
        let sid = session.id.as_uuid().to_string();

        // Poll the real session list and process table until the devin child shows.
        let deadline = Instant::now() + Duration::from_secs(5);
        let subs = loop {
            let sessions = bridge.session_list().await.unwrap();
            let map = next_map(Some(sessions), process_table, &SubagentMap::new()).await;
            match map.get(&sid) {
                Some(subs) => break subs.clone(),
                None => {
                    assert!(
                        Instant::now() < deadline,
                        "no sub-agent reported for session {sid}"
                    );
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        };
        assert_eq!(subs.len(), 1, "{subs:?}");
        assert_eq!(subs[0].agent, "devin");
        assert!(subs[0].command.contains("/devin 20"), "{}", subs[0].command);
        let sub_pid = subs[0].pid;

        bridge.session_stop(session.id, true).await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let sessions = bridge.session_list().await.unwrap();
            if sessions
                .iter()
                .any(|item| item.id == session.id && item.state == SessionState::Exited)
            {
                break;
            }
            assert!(Instant::now() < deadline, "session did not exit");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let sessions = bridge.session_list().await.unwrap();
        let map = next_map(Some(sessions), process_table, &SubagentMap::new()).await;
        assert!(map.get(&sid).is_none(), "{map:?}");

        // The forced stop killed the process group, so the sleep is gone too.
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let rows = process_table().await.unwrap();
            if !rows.iter().any(|row| row.pid == sub_pid) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "devin pid {sub_pid} survived the forced stop"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}
