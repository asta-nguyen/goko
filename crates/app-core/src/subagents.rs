//! The agent CLIs running under a session's process tree.
//!
//! A session runs a shell; an agent (claude, codex, opencode, devin) may
//! run under it. The session's own pid names the tree's root, so the
//! sub-agents are the agent CLIs below it. The desktop shell reads the
//! same process table; this module is the shared one.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use protocol::session::{SessionInfo, SessionState};
use serde::Serialize;

/// `command` is truncated to this many characters (not bytes) in the
/// published map.
pub const COMMAND_LIMIT: usize = 200;

/// One agent CLI running under a session's process tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Subagent {
    pub agent: String,
    pub pid: u32,
    pub command: String,
}

/// Goko session id -> the agent CLIs running under that session.
pub type SubagentMap = HashMap<String, Vec<Subagent>>;

/// One row of `ps -axww -o pid=,ppid=,command=` output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessRow {
    pub pid: u32,
    pub ppid: u32,
    /// The full command line; it may itself contain spaces.
    pub command: String,
}

/// Parses `ps` output: two integers, then the rest of the line as the
/// command. Empty lines and lines that cannot be parsed are skipped.
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

/// Every process below `root` in the `ppid` tree, at any depth; `root`
/// itself is never included. Each pid is visited once, so a `ppid` cycle
/// cannot loop.
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

/// The agent CLI `command` launches, when its program is a known agent: the
/// file stem of the first word — or of the word after `node`/`bun`, the
/// JS-runtime shape. A wrapper like `zsh -c '… devin …'` has program `zsh`
/// and does not match.
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

/// Session id -> agent CLIs under each running session's pid. A session
/// that is not running, has no pid, or spawns no agent CLI gets no entry.
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

/// The sub-agents of one session, as the row shows them: `devin`,
/// `codex, devin ×2` (names alphabetical, counts after the name).
pub fn subagent_label(subagents: &[Subagent]) -> String {
    let mut names: Vec<&str> = subagents
        .iter()
        .map(|subagent| subagent.agent.as_str())
        .collect();
    names.sort_unstable();
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for name in names {
        match counts.last_mut() {
            Some((last, count)) if *last == name => *count += 1,
            _ => counts.push((name, 1)),
        }
    }
    counts
        .into_iter()
        .map(|(name, count)| {
            if count > 1 {
                format!("{name} ×{count}")
            } else {
                name.to_string()
            }
        })
        .collect::<Vec<String>>()
        .join(", ")
}

// Codex 0.167 runs a session's shell tool inside a shared `codex app-server`
// daemon, not under the `codex` TUI Goko spawned, so walking the session
// pid's descendants finds no agent. The daemon's tool calls carry
// `CODEX_THREAD_ID` in their environment, and the same id ends the
// session's rollout file name; tying a rollout to its Goko session reuses
// the activity module's cwd + start-time heuristic. Known limit: named Codex
// accounts use their own `CODEX_HOME`, and only the default Codex home is
// scanned here.

/// A `codex app-server ...` process: the file stem of the first word is
/// `codex` and the second word is `app-server`.
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

/// The agent CLIs running under the shared `codex app-server` daemons: the
/// daemon's own children, minus the ones already reported directly.
pub fn codex_daemon_agents<'a>(
    rows: &'a [ProcessRow],
    direct: &SubagentMap,
) -> Vec<&'a ProcessRow> {
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

/// pid -> `CODEX_THREAD_ID` from `ps -E` output, one `<pid> <command...>
/// <ENV=VAL ...>` line each. An env value containing spaces splits into
/// several tokens; a uuid never does.
pub fn parse_thread_ids(text: &str) -> HashMap<u32, String> {
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

/// The thread id a rollout belongs to: the uuid at the end of its file stem
/// (`rollout-<timestamp>-<uuid>.jsonl` keeps the last 36 chars). A shorter
/// stem is skipped.
pub fn rollout_thread_id(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let start = stem.char_indices().nth_back(35)?.0;
    Some(stem[start..].to_owned())
}

/// thread id -> Goko session id: each running Codex session with a pid is
/// matched to its rollout, and the rollout stem's trailing uuid is the
/// thread.
pub fn thread_owners(
    sessions: &[SessionInfo],
    rollouts: &[crate::activity::RolloutMeta],
) -> HashMap<String, String> {
    let running: Vec<&SessionInfo> = sessions
        .iter()
        .filter(|session| {
            session.state == SessionState::Running
                && session.pid.is_some()
                && crate::activity::is_codex(&session.argv)
        })
        .collect();
    let ids: Vec<String> = running
        .iter()
        .map(|session| session.id.as_uuid().to_string())
        .collect();
    let codex: Vec<crate::activity::SessionActivity> = running
        .iter()
        .zip(ids.iter())
        .map(|(session, id)| crate::activity::SessionActivity {
            id: id.as_str().to_owned(),
            argv: session.argv.clone(),
            cwd: session.cwd.clone(),
            started_at_ms: session.started_at,
            running: true,
            interrupted: false,
        })
        .collect();
    crate::activity::match_codex_rollouts(&codex, rollouts)
        .into_iter()
        .filter_map(|(session_id, rollout)| {
            rollout_thread_id(&rollout).map(|thread| (thread, session_id))
        })
        .collect()
}

/// Adds each daemon agent whose `CODEX_THREAD_ID` belongs to a Goko Codex
/// session to that session's entry; touched vecs stay sorted by pid.
pub fn attribute_codex(
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

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::session::SessionId;

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
            title: "shell".into(),
            argv: vec!["sh".into()],
            cwd: "/tmp".into(),
            state,
            pid,
            exit_code: None,
            error: None,
            started_at: 1_700_000_000_000,
            ended_at: None,
        }
    }

    #[test]
    fn ps_output_is_parsed_into_rows() {
        let rows = parse_ps("  12   1 /bin/zsh -l\n  13  12 claude --resume\nnot a row\n");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].pid, 12);
        assert_eq!(rows[0].ppid, 1);
        assert_eq!(rows[0].command, "/bin/zsh -l");
        assert_eq!(rows[1].command, "claude --resume");
    }

    #[test]
    fn descendants_are_found_at_any_depth() {
        let rows = vec![
            row(1, 0, "launchd"),
            row(10, 1, "zsh"),
            row(11, 10, "claude"),
            row(12, 11, "node devin"),
        ];
        let found = descendants(&rows, 10);
        assert_eq!(
            found.iter().map(|row| row.pid).collect::<Vec<_>>(),
            vec![11, 12]
        );
    }

    #[test]
    fn a_known_agent_command_is_recognized() {
        assert_eq!(agent_of("/usr/local/bin/claude --resume"), Some("claude"));
        assert_eq!(
            agent_of("/opt/homebrew/Caskroom/claude-code/2.1.311/claude --resume y"),
            Some("claude")
        );
        assert_eq!(agent_of("codex"), Some("codex"));
        // The JS-runtime shape: the script's stem names the agent.
        assert_eq!(agent_of("node /usr/lib/codex.js"), Some("codex"));
        assert_eq!(agent_of("bun /x/opencode"), Some("opencode"));
        // A wrapper is not an agent.
        assert_eq!(agent_of("/bin/zsh -c devin"), None);
        assert_eq!(agent_of("node server.js"), None);
        assert_eq!(agent_of("/usr/bin/ssh host"), None);
    }

    #[test]
    fn only_running_sessions_with_a_pid_get_subagents() {
        let rows = vec![row(10, 1, "zsh"), row(11, 10, "claude")];
        let session = info(SessionState::Running, Some(10));
        let map = compute_subagents(&[session], &rows);
        assert_eq!(map.len(), 1);
        let subagents = map.values().next().unwrap();
        assert_eq!(subagents.len(), 1);
        assert_eq!(subagents[0].agent, "claude");

        // A session with no pid, or one that is not running, gets none.
        let idle = info(SessionState::Running, None);
        assert!(compute_subagents(&[idle], &rows).is_empty());
        let stopped = info(SessionState::Exited, Some(10));
        assert!(compute_subagents(&[stopped], &rows).is_empty());
    }

    #[test]
    fn the_row_label_counts_the_agents() {
        let rows = vec![
            row(11, 10, "claude"),
            row(12, 10, "devin"),
            row(13, 10, "devin"),
        ];
        let session = info(SessionState::Running, Some(10));
        let map = compute_subagents(&[session], &rows);
        let label = subagent_label(map.values().next().unwrap());
        assert_eq!(label, "claude, devin ×2");
    }
}
