//! Read-only, best-effort diagnostics. Shared stores alone never identify a process's session.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use protocol::session::{SessionInfo, SessionState};

use crate::subagents::{descendants, ProcessRow, SubagentMap};

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub rss_kib: HashMap<u32, u64>,
    pub trees: HashMap<String, Vec<u32>>,
    pub identities: HashMap<String, Result<Identity, String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub agent: String,
    pub id: String,
    pub source: String,
}

/// `ps` reports RSS in KiB on macOS and Linux. Keep commands intact after the three columns.
pub fn parse_processes(text: &str) -> (Vec<ProcessRow>, HashMap<u32, u64>) {
    let mut rows = Vec::new();
    let mut rss = HashMap::new();
    for line in text.lines() {
        let mut rest = line.trim();
        let mut number = || {
            let (word, tail) = rest.split_once(char::is_whitespace)?;
            rest = tail.trim_start();
            word.parse::<u64>().ok()
        };
        let parsed = (number(), number(), number());
        let (Some(pid), Some(ppid), Some(kib)) = parsed else {
            continue;
        };
        let (Ok(pid), Ok(ppid)) = (u32::try_from(pid), u32::try_from(ppid)) else {
            continue;
        };
        if rest.is_empty() {
            continue;
        }
        rows.push(ProcessRow {
            pid,
            ppid,
            command: rest.to_owned(),
        });
        rss.insert(pid, kib);
    }
    (rows, rss)
}

pub fn sample(
    sessions: &[SessionInfo],
    rows: &[ProcessRow],
    rss_kib: HashMap<u32, u64>,
    agents: &SubagentMap,
    inspect: &[String],
) -> Snapshot {
    let mut snapshot = Snapshot {
        rss_kib,
        ..Default::default()
    };
    for session in sessions.iter().filter(|s| live(s.state)) {
        let key = session.id.as_uuid().to_string();
        let Some(root) = session.pid.filter(|pid| snapshot.rss_kib.contains_key(pid)) else {
            continue;
        };
        let mut pids = HashSet::from([root]);
        pids.extend(descendants(rows, root).iter().map(|p| p.pid));
        // Include attributed tools below a shared Codex app-server, without charging the server.
        for agent in agents.get(&key).into_iter().flatten() {
            pids.insert(agent.pid);
            pids.extend(descendants(rows, agent.pid).iter().map(|p| p.pid));
        }
        snapshot.trees.insert(key, pids.into_iter().collect());
    }

    let inspected_pids: HashSet<u32> = inspect
        .iter()
        .filter_map(|key| snapshot.trees.get(key))
        .flatten()
        .copied()
        .collect();
    let files = open_files(&inspected_pids);
    for session in sessions
        .iter()
        .filter(|s| inspect.contains(&s.id.as_uuid().to_string()))
    {
        let key = session.id.as_uuid().to_string();
        snapshot
            .identities
            .insert(key.clone(), identity(session, rows, &files));
    }
    snapshot
}

pub fn live(state: SessionState) -> bool {
    !matches!(state, SessionState::Exited | SessionState::Failed)
}

/// Sum the union, so overlapping panes and attributed tools are counted once.
pub fn memory_kib(snapshot: &Snapshot, sessions: &[String]) -> Option<u64> {
    let mut seen = HashSet::new();
    let mut sum = 0_u64;
    for key in sessions {
        for pid in snapshot.trees.get(key)? {
            if seen.insert(*pid) {
                sum = sum.checked_add(*snapshot.rss_kib.get(pid)?)?;
            }
        }
    }
    Some(sum)
}

fn agent(argv: &[String]) -> Option<&'static str> {
    let first = argv.first()?;
    let program = if Path::new(first).file_name()? == "env" {
        argv.iter().skip(1).find(|word| !word.contains('='))?
    } else {
        first
    };
    match Path::new(program).file_stem()?.to_str()? {
        "claude" => Some("claude"),
        "codex" => Some("codex"),
        "opencode" => Some("opencode"),
        "devin" => Some("devin"),
        "agy" => Some("agy"),
        "node" | "bun" => agent(&argv[1..]),
        _ => None,
    }
}

fn valid_id(id: &str) -> bool {
    id.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn launch_id(kind: &str, argv: &[String]) -> Option<String> {
    let flags: &[&str] = match kind {
        "claude" => &["--session-id", "--resume", "-r"],
        "codex" => &["resume"],
        "opencode" => &["--session", "-s"],
        "devin" => &["--resume", "-r"],
        "agy" => &["--conversation"],
        _ => return None,
    };
    let args: Vec<_> = argv.iter().take_while(|arg| arg.as_str() != "--").collect();
    for (i, arg) in args.iter().enumerate() {
        if flags.contains(&arg.as_str()) {
            let id = args.get(i + 1)?;
            return valid_id(id).then(|| id.to_string());
        }
        if let Some((flag, id)) = arg.split_once('=') {
            if flags.contains(&flag) && valid_id(id) {
                return Some(id.to_owned());
            }
        }
    }
    None
}

fn identity(
    session: &SessionInfo,
    rows: &[ProcessRow],
    files: &HashMap<u32, Vec<PathBuf>>,
) -> Result<Identity, String> {
    if !live(session.state) {
        return Err("Session is not running; current agent ID is unavailable".into());
    }
    let root = session.pid.ok_or("Session has no process ID")?;
    let kind = agent(&session.argv);
    let mut processes: Vec<(u32, Vec<String>)> = vec![(root, session.argv.clone())];
    processes.extend(
        descendants(rows, root)
            .iter()
            .filter_map(|row| shlex::split(&row.command).map(|argv| (row.pid, argv))),
    );
    let mut candidates = Vec::new();
    let mut has_agent = kind.is_some();
    for (pid, argv) in &processes {
        let Some(process_kind) = agent(argv) else {
            continue;
        };
        // A nested tool agent has its own conversation, not the parent CLI's.
        if kind.is_some_and(|kind| kind != process_kind) {
            continue;
        }
        has_agent = true;
        for file in files.get(pid).into_iter().flatten() {
            if let Some(id) = file_id(process_kind, file) {
                candidates.push(Identity {
                    agent: process_kind.into(),
                    id,
                    source: "PID-linked session file".into(),
                });
            }
        }
    }
    candidates.sort_by(|a, b| (&a.agent, &a.id).cmp(&(&b.agent, &b.id)));
    candidates.dedup_by(|a, b| a.agent == b.agent && a.id == b.id);
    match candidates.len() {
        1 => return Ok(candidates.remove(0)),
        0 => {}
        _ => return Err("Multiple agent sessions; active conversation ID is unknown".into()),
    }
    if let Some(kind) = kind {
        if let Some(id) = launch_id(kind, &session.argv) {
            return Ok(Identity {
                agent: kind.into(),
                id,
                source: "Launch ID; may change inside the CLI".into(),
            });
        }
    }
    Err(if has_agent {
        "No PID-linked ID; shared session lists are ambiguous".into()
    } else {
        "Shell has no agent conversation ID".into()
    })
}

fn file_id(kind: &str, path: &Path) -> Option<String> {
    let parent = path.parent()?;
    let stem = path.file_stem()?.to_str()?;
    let uuid = |id: &str| uuid::Uuid::parse_str(id).ok().map(|id| id.to_string());
    match kind {
        "claude" if path.extension()? == "jsonl" && parent.parent()?.file_name()? == "projects" => {
            uuid(stem)
        }
        "codex" if path.extension()? == "jsonl" && stem.starts_with("rollout-") => {
            if !path
                .ancestors()
                .any(|p| p.file_name().is_some_and(|n| n == "sessions"))
            {
                return None;
            }
            uuid(stem.get(stem.len().checked_sub(36)?..)?)
        }
        "devin"
            if parent.file_name()? == "session_locks"
                && path.extension()? == "lock"
                && valid_id(stem) =>
        {
            let db = parent.parent()?.join("sessions.db");
            let connection = rusqlite::Connection::open_with_flags(
                db,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .ok()?;
            connection.busy_timeout(std::time::Duration::ZERO).ok()?;
            let exists: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1)",
                    [stem],
                    |r| r.get(0),
                )
                .ok()?;
            exists.then(|| stem.to_owned())
        }
        "agy" if parent.file_name()? == "presence" && path.extension()? == "lock" => {
            let id = uuid(stem)?;
            parent
                .parent()?
                .join("conversations")
                .join(format!("{id}.db"))
                .is_file()
                .then_some(id)
        }
        _ => None,
    }
}

fn open_files(pids: &HashSet<u32>) -> HashMap<u32, Vec<PathBuf>> {
    if pids.is_empty() {
        return HashMap::new();
    }
    let pids = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    // -b avoids potentially blocking filesystem calls; -F is the machine-readable interface.
    let Ok(output) = std::process::Command::new("lsof")
        .args(["-b", "-w", "-n", "-P", "-p", &pids, "-Fpn"])
        .output()
    else {
        return HashMap::new();
    };
    parse_open_files(&String::from_utf8_lossy(&output.stdout))
}

fn parse_open_files(text: &str) -> HashMap<u32, Vec<PathBuf>> {
    let mut files = HashMap::<u32, Vec<PathBuf>>::new();
    let mut pid = None;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = value.parse().ok();
        } else if let (Some(path), Some(pid)) = (line.strip_prefix('n'), pid) {
            files.entry(pid).or_default().push(path.into());
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_rss_and_unique_tree_totals_preserve_commands_and_missing_data() {
        let (rows, rss_kib) = parse_processes(
            "10 1 1024 /bin/zsh -l\n11 10 2048 node /a path/agent.js\ninvalid\n12 1 -1 bad\n",
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].command, "node /a path/agent.js");
        let snapshot = Snapshot {
            rss_kib,
            trees: HashMap::from([("a".into(), vec![10, 11]), ("b".into(), vec![11])]),
            ..Default::default()
        };
        assert_eq!(memory_kib(&snapshot, &["a".into(), "b".into()]), Some(3072));
        assert_eq!(memory_kib(&snapshot, &["missing".into()]), None);
    }

    #[test]
    fn launch_ids_cover_every_adapter_and_do_not_accept_options_or_prompts() {
        for (kind, flag, id) in [
            ("claude", "--session-id", "abc-123"),
            ("codex", "resume", "abc-123"),
            ("opencode", "--session", "ses_123"),
            ("devin", "--resume", "private-peanut"),
            ("agy", "--conversation", "abc-123"),
        ] {
            let argv = vec![kind.into(), flag.into(), id.into()];
            assert_eq!(launch_id(kind, &argv).as_deref(), Some(id));
            assert!(launch_id(kind, &[kind.into(), flag.into(), "--other".into()]).is_none());
            assert!(launch_id(kind, &[kind.into(), "--".into(), flag.into(), id.into()]).is_none());
        }
        assert_eq!(
            agent(&[
                "/usr/bin/env".into(),
                "CODEX_HOME=/tmp/a=b".into(),
                "/bin/codex".into()
            ]),
            Some("codex")
        );
        assert_eq!(agent(&["/bin/agy".into()]), Some("agy"));
        assert!(launch_id("shell", &["sh".into()]).is_none());
    }

    #[test]
    fn open_files_stay_bound_to_their_pid_and_shared_stores_are_not_ids() {
        let files = parse_open_files(
            "p10\nn/tmp/sessions.db\np11\nn/tmp/session_locks/private-peanut.lock\n",
        );
        assert_eq!(files[&10], vec![PathBuf::from("/tmp/sessions.db")]);
        assert_eq!(
            files[&11],
            vec![PathBuf::from("/tmp/session_locks/private-peanut.lock")]
        );
        for kind in ["claude", "codex", "opencode", "devin", "agy"] {
            assert!(file_id(kind, Path::new("/tmp/opencode.db")).is_none());
        }
        assert!(file_id(
            "codex",
            Path::new("/tmp/rollout-00000000-0000-0000-0000-000000000001.jsonl")
        )
        .is_none());
        assert_eq!(
            file_id(
                "codex",
                Path::new(
                    "/tmp/sessions/2026/rollout-time-00000000-0000-0000-0000-000000000001.jsonl"
                )
            )
            .as_deref(),
            Some("00000000-0000-0000-0000-000000000001")
        );
    }

    #[test]
    fn devin_lock_id_must_exist_in_its_own_database() {
        let home = crate::test_support::TempDir::new("debug-lock");
        let locks = home.path().join("session_locks");
        std::fs::create_dir(&locks).unwrap();
        let db = rusqlite::Connection::open(home.path().join("sessions.db")).unwrap();
        db.execute_batch("CREATE TABLE sessions(id TEXT PRIMARY KEY); INSERT INTO sessions VALUES('private-peanut');").unwrap();
        assert_eq!(
            file_id("devin", &locks.join("private-peanut.lock")).as_deref(),
            Some("private-peanut")
        );
        assert!(file_id("devin", &locks.join("unknown.lock")).is_none());
    }

    fn session(kind: &str, pid: u32) -> SessionInfo {
        SessionInfo {
            id: protocol::session::SessionId::new(),
            title: kind.into(),
            argv: vec![kind.into()],
            cwd: "/tmp".into(),
            state: SessionState::Running,
            pid: Some(pid),
            exit_code: None,
            error: None,
            started_at: 0,
            ended_at: None,
        }
    }

    #[test]
    fn identities_prefer_owned_files_and_refuse_ambiguity_or_exited_sessions() {
        let id = "00000000-0000-0000-0000-000000000001";
        let other = "00000000-0000-0000-0000-000000000002";
        let mut s = session("claude", 10);
        s.argv.extend(["--session-id".into(), other.into()]);
        let mut files = HashMap::from([(
            10,
            vec![PathBuf::from(format!("/tmp/projects/work/{id}.jsonl"))],
        )]);
        assert_eq!(identity(&s, &[], &files).unwrap().id, id);
        files
            .get_mut(&10)
            .unwrap()
            .push(format!("/tmp/projects/work/{other}.jsonl").into());
        assert!(identity(&s, &[], &files).unwrap_err().contains("Multiple"));
        s.state = SessionState::Exited;
        assert!(identity(&s, &[], &HashMap::new()).is_err());
        for kind in ["codex", "opencode", "devin", "agy"] {
            assert!(identity(&session(kind, 10), &[], &HashMap::new())
                .unwrap_err()
                .contains("PID-linked"));
        }
        assert!(identity(&session("/bin/zsh", 10), &[], &HashMap::new())
            .unwrap_err()
            .contains("Shell"));
    }

    #[test]
    fn antigravity_presence_requires_a_matching_conversation_database() {
        let home = crate::test_support::TempDir::new("agy-presence");
        let id = "00000000-0000-0000-0000-000000000001";
        let path = home.path().join("presence").join(format!("{id}.lock"));
        assert!(file_id("agy", &path).is_none());
        std::fs::create_dir(home.path().join("conversations")).unwrap();
        std::fs::write(
            home.path().join("conversations").join(format!("{id}.db")),
            [],
        )
        .unwrap();
        assert_eq!(file_id("agy", &path).as_deref(), Some(id));
    }

    #[test]
    fn sample_includes_attributed_tools_but_not_the_shared_server_or_exited_trees() {
        let s = session("codex", 10);
        let key = s.id.as_uuid().to_string();
        let (rows, rss) = parse_processes("10 1 100 codex\n11 10 200 sh\n20 1 400 codex app-server\n21 20 800 devin\n22 21 1600 sh\n");
        let agents = HashMap::from([(
            key.clone(),
            vec![crate::subagents::Subagent {
                agent: "devin".into(),
                pid: 21,
                command: "devin".into(),
            }],
        )]);
        let snapshot = sample(std::slice::from_ref(&s), &rows, rss.clone(), &agents, &[]);
        assert_eq!(
            memory_kib(&snapshot, std::slice::from_ref(&key)),
            Some(2700)
        );
        let mut exited = s;
        exited.state = SessionState::Exited;
        assert!(sample(&[exited], &rows, rss, &agents, &[]).trees.is_empty());
    }
}
