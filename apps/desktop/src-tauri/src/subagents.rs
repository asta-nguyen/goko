use crate::activity::{is_codex, match_codex_rollouts, recent_rollouts, CodexSession, RolloutMeta};
use crate::bridge::Bridge;
use crate::events::Emit;
use crate::usage::codex_home_from;
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

// Codex 0.167 runs a session's shell tool inside a shared `codex app-server`
// daemon, not under the `codex` TUI Goko spawned, so walking the session pid's
// descendants finds no agent. The daemon's tool calls carry `CODEX_THREAD_ID`
// in their environment, and the same id ends the session's rollout file name;
// tying a rollout to its Goko session reuses activity's cwd + start-time
// heuristic. Known limit: named Codex accounts use their own `CODEX_HOME`, and
// only the default Codex home is scanned here.

/// A `codex app-server …` process: the file stem of the first word is `codex`
/// and the second word is `app-server`.
fn is_codex_app_server(command: &str) -> bool {
    let mut words = command.split_whitespace();
    let Some(first) = words.next() else {
        return false;
    };
    Path::new(first)
        .file_stem()
        .is_some_and(|stem| stem == "codex")
        && words.next() == Some("app-server")
}

/// Agent-CLI descendants of every `codex app-server` root, sorted by pid and
/// deduplicated. Pids already reported as direct sub-agents are skipped, and so
/// are the daemon's own `codex app-server` children: shared infrastructure, not
/// sub-agents.
fn codex_daemon_agents<'a>(rows: &'a [ProcessRow], direct: &SubagentMap) -> Vec<&'a ProcessRow> {
    let claimed: HashSet<u32> = direct
        .values()
        .flat_map(|subagents| subagents.iter().map(|subagent| subagent.pid))
        .collect();
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    for root in rows {
        if !is_codex_app_server(&root.command) {
            continue;
        }
        for row in descendants(rows, root.pid) {
            if seen.insert(row.pid)
                && !is_codex_app_server(&row.command)
                && agent_of(&row.command).is_some()
                && !claimed.contains(&row.pid)
            {
                found.push(row);
            }
        }
    }
    found.sort_by_key(|row| row.pid);
    found
}

/// pid -> `CODEX_THREAD_ID` from `ps -E` output, one `<pid> <command…>
/// <ENV=VAL …>` line each. An env value containing spaces splits into several
/// tokens; a uuid never does.
fn parse_thread_ids(text: &str) -> HashMap<u32, String> {
    let mut threads = HashMap::new();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let Some(pid) = words.next().and_then(|word| word.parse::<u32>().ok()) else {
            continue;
        };
        if let Some(id) = words.find_map(|word| word.strip_prefix("CODEX_THREAD_ID=")) {
            threads.insert(pid, id.to_owned());
        }
    }
    threads
}

/// pid -> `CODEX_THREAD_ID` read from each process's environment. A `ps`
/// failure yields an empty map, so the tick attributes nothing.
async fn thread_ids(pids: &[u32]) -> HashMap<u32, String> {
    if pids.is_empty() {
        return HashMap::new();
    }
    let joined = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let Ok(output) = tokio::process::Command::new("ps")
        .args(["-Eww", "-o", "pid=,command=", "-p", joined.as_str()])
        .output()
        .await
    else {
        return HashMap::new();
    };
    if !output.status.success() {
        return HashMap::new();
    }
    parse_thread_ids(&String::from_utf8_lossy(&output.stdout))
}

/// The thread id a rollout belongs to: the uuid at the end of its file stem
/// (`rollout-<timestamp>-<uuid>.jsonl` keeps the last 36 chars). A shorter stem
/// is skipped.
fn rollout_thread_id(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let start = stem.char_indices().nth_back(35)?.0;
    Some(stem[start..].to_owned())
}

/// thread id -> Goko session id: each running Codex session with a pid is
/// matched to its rollout, and the rollout stem's trailing uuid is the thread.
fn thread_owners(sessions: &[SessionInfo], rollouts: &[RolloutMeta]) -> HashMap<String, String> {
    let running: Vec<&SessionInfo> = sessions
        .iter()
        .filter(|session| {
            session.state == SessionState::Running
                && session.pid.is_some()
                && is_codex(&session.argv)
        })
        .collect();
    let ids: Vec<String> = running
        .iter()
        .map(|session| session.id.as_uuid().to_string())
        .collect();
    let codex: Vec<CodexSession> = running
        .iter()
        .zip(ids.iter())
        .map(|(session, id)| CodexSession {
            id: id.as_str(),
            cwd: session.cwd.as_str(),
            started_at_ms: session.started_at,
        })
        .collect();
    match_codex_rollouts(&codex, rollouts)
        .into_iter()
        .filter_map(|(session_id, rollout)| {
            rollout_thread_id(&rollout).map(|thread| (thread, session_id))
        })
        .collect()
}

/// Adds each daemon agent whose `CODEX_THREAD_ID` belongs to a Goko Codex
/// session to that session's entry; touched vecs stay sorted by pid.
fn attribute_codex(
    map: &mut SubagentMap,
    agents: &[&ProcessRow],
    threads: &HashMap<u32, String>,
    owners: &HashMap<String, String>,
) {
    let mut touched = HashSet::new();
    for row in agents {
        let Some(agent) = agent_of(&row.command) else {
            continue;
        };
        let Some(owner) = threads.get(&row.pid).and_then(|id| owners.get(id)) else {
            continue;
        };
        let subagents = map.entry(owner.clone()).or_default();
        if subagents.iter().any(|sub| sub.pid == row.pid) {
            continue;
        }
        subagents.push(Subagent {
            agent: agent.to_owned(),
            pid: row.pid,
            command: row.command.chars().take(COMMAND_LIMIT).collect(),
        });
        touched.insert(owner.clone());
    }
    for owner in touched {
        if let Some(subagents) = map.get_mut(&owner) {
            subagents.sort_by_key(|sub| sub.pid);
        }
    }
}

/// The Codex-daemon pass for this tick: a no-op unless a Codex home is known
/// and the process table holds `codex app-server` agents, so the common case
/// reads no environment and scans no rollouts.
async fn add_codex_daemon_agents(
    map: &mut SubagentMap,
    sessions: &[SessionInfo],
    rows: &[ProcessRow],
    codex_home: Option<&Path>,
) {
    let Some(codex_home) = codex_home else {
        return;
    };
    let agents = codex_daemon_agents(rows, map);
    if agents.is_empty() {
        return;
    }
    let pids: Vec<u32> = agents.iter().map(|row| row.pid).collect();
    let threads = thread_ids(&pids).await;
    let owners = thread_owners(sessions, &recent_rollouts(codex_home));
    attribute_codex(map, &agents, &threads, &owners);
}

/// The map for this tick. A failed session listing or process-table read keeps
/// `previous`; when no running session has a pid the answer is empty and the
/// table is not read at all.
async fn next_map<F, Fut>(
    sessions: Option<Vec<SessionInfo>>,
    table: F,
    previous: &SubagentMap,
    codex_home: Option<&Path>,
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
        Some(rows) => {
            let mut map = compute_subagents(&sessions, &rows);
            add_codex_daemon_agents(&mut map, &sessions, &rows, codex_home).await;
            map
        }
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
    let codex_home = codex_home_from(std::env::var_os("CODEX_HOME"), std::env::var_os("HOME"));
    let state = SubagentState::default();
    let poll_state = state.clone();
    tauri::async_runtime::spawn(async move {
        let mut current = SubagentMap::new();
        let mut interval = tokio::time::interval(POLL_INTERVAL);
        loop {
            interval.tick().await;
            // A daemon or `ps` error keeps the previous map; the next tick retries.
            let sessions = bridge.session_list().await.ok();
            let next = next_map(sessions, process_table, &current, codex_home.as_deref()).await;
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
    use std::path::PathBuf;
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

    #[test]
    fn parse_thread_ids_reads_the_thread_id_appended_after_the_command() {
        let text = "\
38155 /Users/nus/.local/bin/devin --model x -p --prompt-file a.md \
HOME=/Users/nus USER=nus CODEX_THREAD_ID=01a1be35-ef31-7d91-a7eb-ebfab364b664 PATH=/bin
38156 devin acp SHELL=/bin/zsh
63024 /opt/codex/bin/codex app-server --listen unix://x
not-a-row CODEX_THREAD_ID=ignored
";

        let threads = parse_thread_ids(text);

        assert_eq!(threads.len(), 1);
        assert_eq!(threads[&38155], "01a1be35-ef31-7d91-a7eb-ebfab364b664");
    }

    #[test]
    fn codex_daemon_agents_finds_agent_cli_descendants_of_the_shared_daemon() {
        // The Codex 0.167 shape: the TUI the session spawned (42066) has no
        // children; the shared `codex app-server` daemon (16453 -> 63024) owns
        // the tool calls, and `codex-code-mode-host`/`rtk proxy` are not agents.
        let rows = vec![
            row(1, 0, "launchd"),
            row(42066, 1, "codex"),
            row(16453, 1, "codex app-server daemon pid-update-loop"),
            row(
                63024,
                16453,
                "/opt/codex/bin/codex app-server --listen unix:///tmp/x.sock --managed-daemon",
            ),
            row(9483, 63024, "/opt/codex/bin/codex-code-mode-host"),
            row(
                38154,
                63024,
                "rtk proxy devin --model x -p --prompt-file x.md",
            ),
            row(
                38155,
                38154,
                "/Users/nus/.local/bin/devin --model x -p --prompt-file x.md",
            ),
            row(38156, 38155, "devin acp"),
            row(777, 1, "devin --model y"),
        ];

        let agents = codex_daemon_agents(&rows, &SubagentMap::new());
        let pids: Vec<u32> = agents.iter().map(|row| row.pid).collect();

        assert_eq!(pids, vec![38155, 38156]);
    }

    #[test]
    fn codex_daemon_agents_skips_pids_already_in_the_direct_map() {
        let rows = vec![
            row(16453, 1, "codex app-server daemon pid-update-loop"),
            row(
                63024,
                16453,
                "/opt/codex/bin/codex app-server --listen unix://x",
            ),
            row(38155, 63024, "devin --model x"),
            row(38156, 63024, "devin acp"),
        ];
        let direct = SubagentMap::from([(
            "s".to_owned(),
            vec![subagent("devin", 38155, "devin --model x")],
        )]);

        let agents = codex_daemon_agents(&rows, &direct);
        let pids: Vec<u32> = agents.iter().map(|row| row.pid).collect();

        assert_eq!(pids, vec![38156]);
    }

    #[test]
    fn thread_owners_maps_thread_ids_to_codex_sessions_via_rollouts() {
        let thread = "01a1be35-ef31-7d91-a7eb-ebfab364b664";
        let mut codex_session = info(SessionState::Running, Some(42066));
        codex_session.argv = vec!["/opt/homebrew/Caskroom/codex/0.167.0/bin/codex".into()];
        codex_session.cwd = "/p".into();
        codex_session.started_at = 2_000_000;
        // Same cwd and an earlier start: each would claim the "/p" rollout if
        // it were not filtered out before matching.
        let mut non_codex = info(SessionState::Running, Some(7));
        non_codex.cwd = "/p".into();
        non_codex.started_at = 1_000_000;
        let mut no_pid = info(SessionState::Running, None);
        no_pid.argv = vec!["codex".into()];
        no_pid.cwd = "/p".into();
        no_pid.started_at = 1_500_000;
        let rollout = |name: &str, cwd: &str, started_ms: i64| RolloutMeta {
            path: PathBuf::from(format!(
                "/codex/sessions/2026/11/09/rollout-2026-11-09T09-49-28-{name}.jsonl"
            )),
            cwd: cwd.to_owned(),
            started_ms,
        };
        let rollouts = vec![
            rollout(thread, "/p", 2_001_000),
            rollout("00000000-0000-0000-0000-000000000000", "/other", 2_002_000),
        ];
        let expected = sid(&codex_session);

        let owners = thread_owners(&[codex_session, non_codex, no_pid], &rollouts);

        assert_eq!(owners, HashMap::from([(thread.to_owned(), expected)]));
    }

    #[test]
    fn attribute_codex_adds_owned_agents_sorted_without_duplicates() {
        let rows = vec![
            row(38155, 1, "/x/devin --model x"),
            row(300, 1, "claude --x"),
            row(38156, 1, "devin acp"),
            row(500, 1, "opencode"),
        ];
        let agents: Vec<&ProcessRow> = rows.iter().collect();
        let threads = HashMap::from([
            (38155, "T1".to_owned()),
            (300, "T2".to_owned()),
            (38156, "T-unknown".to_owned()),
        ]);
        let owners = HashMap::from([
            ("T1".to_owned(), "S".to_owned()),
            ("T2".to_owned(), "S".to_owned()),
        ]);
        let mut map = SubagentMap::new();

        attribute_codex(&mut map, &agents, &threads, &owners);
        attribute_codex(&mut map, &agents, &threads, &owners);

        // 38156's thread has no owner and 500 has no thread: both are ignored.
        assert_eq!(map.len(), 1);
        assert_eq!(
            map["S"],
            vec![
                subagent("claude", 300, "claude --x"),
                subagent("devin", 38155, "/x/devin --model x"),
            ]
        );
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
            None,
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
            None,
        )
        .await;

        assert!(next.is_empty());
        assert!(!called.get());
    }

    #[tokio::test]
    async fn next_map_keeps_the_previous_map_when_the_process_table_fails() {
        let session = info(SessionState::Running, Some(100));
        let previous = previous_map();

        let next = next_map(Some(vec![session]), || async { None }, &previous, None).await;

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
            None,
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
    async fn thread_ids_reads_codex_thread_id_from_a_real_process_environment() {
        // `env` sets the variable and execs the command, so the spawned pid
        // carries `CODEX_THREAD_ID` in its environment like a Codex tool call.
        // The sleeper must be a non-platform binary: Apple platform binaries
        // (`/bin/sleep`, `/bin/sh`, …) never expose their environment to
        // `ps -E`, while user-installed binaries like node do.
        let mut child = tokio::process::Command::new("env")
            .args([
                "CODEX_THREAD_ID=test-thread-1",
                "node",
                "-e",
                "setTimeout(()=>{},30000)",
            ])
            .spawn()
            .unwrap();
        let pid = child.id().unwrap();

        // Poll briefly: `ps` may run in the window before `env` finishes exec.
        let deadline = Instant::now() + Duration::from_secs(5);
        let threads = loop {
            let threads = thread_ids(&[pid]).await;
            if threads.contains_key(&pid) || Instant::now() >= deadline {
                break threads;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        child.kill().await.unwrap();
        child.wait().await.unwrap();

        assert_eq!(threads.get(&pid).map(String::as_str), Some("test-thread-1"));
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
            let map = next_map(Some(sessions), process_table, &SubagentMap::new(), None).await;
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
        let map = next_map(Some(sessions), process_table, &SubagentMap::new(), None).await;
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
