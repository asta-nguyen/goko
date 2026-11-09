//! Mock data layer. The structs mirror the real app's shapes — `SessionInfo`
//! and `SessionState` from `crates/protocol`, `Project`/`SessionMeta` from the
//! Tauri store, `WorktreeInfo`/`WorktreeList` from `worktrees.rs`, `AgentInfo`
//! from `agents.rs`, and the `activity`/`usage` service payloads — so a future
//! real source (daemon `session.list`, Tauri queries) can return the same
//! shapes without touching the view code.
//!
//! `SessionView` is the one app-side join the tree consumes: the daemon's
//! `SessionInfo` plus its optional `SessionMeta` row and the two enrichments
//! the real app sources from separate queries (`activity_get`, `usage_get`).

// These types mirror the daemon/Tauri wire shapes field-for-field; the mock
// UI reads a subset, so unused variants/fields/helpers are expected until the
// real data source lands. Lift this allow then.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};

use rune::Color;

use crate::theme::Ink;

/// `crates/protocol` `SessionState` (`#[serde(rename_all = "snake_case")]`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionState {
    Created,
    Starting,
    Running,
    WaitingForInput,
    Idle,
    Stopping,
    Exited,
    Failed,
    Disconnected,
    Interrupted,
    Unknown,
}

/// `services/activity.ts` `Activity`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Activity {
    Working,
    Waiting,
    Done,
}

impl SessionState {
    /// `exited`/`failed` rows render muted (SessionItems `ended`).
    pub fn ended(self) -> bool {
        matches!(self, SessionState::Exited | SessionState::Failed)
    }

    /// Everything that is not ended.
    pub fn live(self) -> bool {
        !self.ended()
    }

    /// SessionControls' `live`: states where Stop applies and elapsed ticks.
    pub fn controls_live(self) -> bool {
        matches!(
            self,
            SessionState::Created
                | SessionState::Starting
                | SessionState::Running
                | SessionState::Stopping
        )
    }

    /// `StateBadge`'s `text()`: snake_case to spaces ("waiting for input").
    pub fn label(self) -> &'static str {
        match self {
            SessionState::Created => "created",
            SessionState::Starting => "starting",
            SessionState::Running => "running",
            SessionState::WaitingForInput => "waiting for input",
            SessionState::Idle => "idle",
            SessionState::Stopping => "stopping",
            SessionState::Exited => "exited",
            SessionState::Failed => "failed",
            SessionState::Disconnected => "disconnected",
            SessionState::Interrupted => "interrupted",
            SessionState::Unknown => "unknown",
        }
    }

    /// `StateBadge`'s `TONE` map (also `StateDot`'s `DOT` fallback): the same
    /// colour serves badge text and the tree-row state dot.
    pub fn badge_tone(self, ink: &Ink) -> Color {
        match self {
            SessionState::Running => ink.success,
            SessionState::Created | SessionState::Starting | SessionState::Stopping => ink.warning,
            SessionState::Failed => ink.danger,
            _ => ink.muted,
        }
    }

    /// `StateDot`: only a running session shows its activity report; every
    /// other state falls back to the DOT colour map.
    pub fn dot_color(self, activity: Option<Activity>, ink: &Ink) -> Color {
        let shown = if self == SessionState::Running {
            activity
        } else {
            None
        };
        match shown {
            Some(Activity::Working) => ink.warning, // animate-pulse — static here
            Some(Activity::Waiting) => ink.danger,
            Some(Activity::Done) => ink.success,
            None => self.badge_tone(ink),
        }
    }
}

/// `crates/protocol` `SessionInfo` (times are Unix epoch milliseconds).
#[derive(Clone, Debug)]
pub struct SessionInfo {
    pub id: String,
    pub title: String,
    pub argv: Vec<String>,
    pub cwd: String,
    pub state: SessionState,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
}

/// src-tauri `SessionMeta`: the app-owned join row from `session_id` to
/// `project_id`/`agent`. A daemon session with no row (or a null/unknown
/// project) renders under Unassigned.
#[derive(Clone, Debug)]
pub struct SessionMeta {
    pub session_id: String,
    pub project_id: Option<String>,
    pub agent: String,
    pub argv: Vec<String>,
    pub cwd: String,
    pub title: String,
    pub created_at: i64,
}

/// src-tauri `ProjectView`: the store row plus `missing` at read time.
#[derive(Clone, Debug)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub root: String,
    pub created_at: i64,
    pub missing: bool,
}

/// src-tauri `WorktreeInfo`.
#[derive(Clone, Debug)]
pub struct WorktreeInfo {
    pub name: String,
    pub path: String,
    pub branch: Option<String>,
    pub head: String,
    pub is_root: bool,
    pub missing: bool,
    /// `None` = git status unknown (renders the muted dot).
    pub dirty: Option<bool>,
    pub changed: Option<u32>,
}

/// src-tauri `WorktreeList`: `repo` is false for non-git projects.
#[derive(Clone, Debug)]
pub struct ProjectWorktrees {
    pub repo: bool,
    pub worktrees: Vec<WorktreeInfo>,
}

/// src-tauri `AgentInfo` (`agent_list`); the five presets from `agents.rs`.
#[derive(Clone, Debug)]
pub struct AgentInfo {
    pub id: String,
    pub label: String,
    pub available: bool,
    pub path: Option<String>,
}

/// One sidebar row's worth of session data.
#[derive(Clone, Debug)]
pub struct SessionView {
    pub info: SessionInfo,
    /// The app-side join row; `None` renders under Unassigned with no label.
    pub meta: Option<SessionMeta>,
    /// Mock-only enrichment: the real app reads `activity_get` and only shows
    /// it while the session is running.
    pub activity: Option<Activity>,
    /// Mock-only enrichment: the real app reads `usage_get` through the Claude
    /// `--session-id` argv flag; `None` hides the context-percentage chip.
    pub context_pct: Option<f64>,
}

impl SessionView {
    pub fn agent_id(&self) -> Option<&str> {
        self.meta.as_ref().map(|m| m.agent.as_str())
    }

    /// `labelOf`: the agent's display label, `None` when there is no meta row.
    pub fn agent_label<'a>(&'a self, agents: &'a [AgentInfo]) -> Option<&'a str> {
        self.meta.as_ref().map(|m| agent_label(agents, &m.agent))
    }

    /// `sessionTitle`: `title || argv.join(" ")`.
    pub fn title(&self) -> String {
        session_title(&self.info)
    }

    pub fn ended(&self) -> bool {
        self.info.state.ended()
    }
}

/// Header usage chip: provider name plus `(window label, used_pct)` pairs —
/// the real app builds these from `usage_get`'s five_hour/seven_day and
/// codex primary/secondary limits via `windowLabel`.
#[derive(Clone, Debug)]
pub struct UsageChip {
    pub name: &'static str,
    pub limits: &'static [(&'static str, u8)],
}

/// Everything the view needs, produced once by `mock_data()`.
#[derive(Clone, Debug)]
pub struct Data {
    pub projects: Vec<Project>,
    pub sessions: Vec<SessionView>,
    /// `project_id` → its worktree list (`useWorktreeList` per project).
    pub worktrees: HashMap<String, ProjectWorktrees>,
    pub agents: Vec<AgentInfo>,
    /// Workspace tab titles for the active checkout and the active tab index.
    pub tabs: Vec<String>,
    pub active_tab: usize,
    pub usage: Vec<UsageChip>,
    /// `daemon_status.connected`; the mock is always connected.
    pub connected: bool,
}

// ---------------------------------------------------------------------------
// Join helpers — ports of stores/ui.ts and the ProjectTree/WorktreeRows pure
// functions, operating on the shapes above.
// ---------------------------------------------------------------------------

/// `UNASSIGNED_WORKSPACE` from stores/ui.ts.
pub const UNASSIGNED_WORKSPACE: &str = "__unassigned__";

/// `checkoutKey()`: project id and checkout path joined by NUL.
pub fn checkout_key(project_id: &str, checkout_path: &str) -> String {
    format!("{project_id}\u{0}{checkout_path}")
}

/// `checkoutContext()`: split a workspace id back into (project, checkout).
pub fn checkout_context(workspace_id: &str) -> Option<(&str, &str)> {
    let at = workspace_id.find('\0')?;
    Some((&workspace_id[..at], &workspace_id[at + 1..]))
}

/// `checkoutForCwd()`: the longest checkout path equal to `cwd` or containing
/// it (`cwd == path || cwd.startsWith(path + '/')`, trailing slashes ignored).
pub fn checkout_for_cwd<'a>(checkouts: &'a [WorktreeInfo], cwd: &str) -> Option<&'a str> {
    fn normalize(path: &str) -> &str {
        if path.len() > 1 {
            path.trim_end_matches('/')
        } else {
            path
        }
    }
    let cwd = normalize(cwd);
    let mut best: Option<&'a str> = None;
    for checkout in checkouts {
        let path = normalize(checkout.path.as_str());
        let matched = cwd == path
            || if path.ends_with('/') {
                cwd.starts_with(path)
            } else {
                cwd.strip_prefix(path)
                    .is_some_and(|rest| rest.starts_with('/'))
            };
        if matched && best.is_none_or(|b| path.len() > b.len()) {
            best = Some(path);
        }
    }
    best
}

/// `sessionsForCheckout()`: sessions whose cwd resolves to exactly this path.
pub fn sessions_for_checkout<'a>(
    checkouts: &[WorktreeInfo],
    sessions: &[&'a SessionView],
    checkout_path: &str,
) -> Vec<&'a SessionView> {
    sessions
        .iter()
        .copied()
        .filter(|s| checkout_for_cwd(checkouts, &s.info.cwd) == Some(checkout_path))
        .collect()
}

/// ProjectTree's `byProject`/`unassigned` partition: a session lands under its
/// meta's project when that project exists, else under Unassigned.
pub fn partition(data: &Data) -> (HashMap<&str, Vec<&SessionView>>, Vec<&SessionView>) {
    let known: HashSet<&str> = data.projects.iter().map(|p| p.id.as_str()).collect();
    let mut by_project: HashMap<&str, Vec<&SessionView>> = HashMap::new();
    let mut unassigned = Vec::new();
    for session in &data.sessions {
        match session.meta.as_ref().and_then(|m| m.project_id.as_deref()) {
            Some(id) if known.contains(id) => by_project.entry(id).or_default().push(session),
            _ => unassigned.push(session),
        }
    }
    (by_project, unassigned)
}

/// Workspace a session row activates: its checkout's key for git projects,
/// the project root's key otherwise, `UNASSIGNED_WORKSPACE` with no project —
/// mirrors the workspace_id each sidebar group passes to SessionItems.
pub fn workspace_for_session(data: &Data, sv: &SessionView) -> String {
    let Some(pid) = sv.meta.as_ref().and_then(|m| m.project_id.as_deref()) else {
        return UNASSIGNED_WORKSPACE.to_string();
    };
    let Some(project) = data.projects.iter().find(|p| p.id == pid) else {
        return UNASSIGNED_WORKSPACE.to_string();
    };
    let is_git = data.worktrees.get(pid).is_some_and(|w| w.repo);
    if is_git {
        if let Some(wts) = data.worktrees.get(pid) {
            if let Some(path) = checkout_for_cwd(&wts.worktrees, &sv.info.cwd) {
                return checkout_key(pid, path);
            }
        }
    }
    checkout_key(pid, &project.root)
}

/// queries.ts `agentLabel()`: the AgentInfo label, "Custom" for `custom`,
/// else the raw id.
pub fn agent_label<'a>(agents: &'a [AgentInfo], id: &'a str) -> &'a str {
    agents
        .iter()
        .find(|a| a.id == id)
        .map(|a| a.label.as_str())
        .unwrap_or(if id == "custom" { "Custom" } else { id })
}

/// `sessionTitle` (`title || argv.join(" ")`).
pub fn session_title(info: &SessionInfo) -> String {
    if info.title.is_empty() {
        info.argv.join(" ")
    } else {
        info.title.clone()
    }
}

/// services/usage.ts `claudeSessionId()`: the `--session-id` argv value.
pub fn claude_session_id(argv: &[String]) -> Option<&str> {
    let flag = argv.iter().position(|arg| arg == "--session-id")?;
    argv.get(flag + 1).map(String::as_str)
}

/// SessionControls `format()`: `m:ss` under an hour, `h:mm:ss` above.
pub fn format_elapsed(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    let (h, m, sec) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m:02}:{sec:02}")
    }
}

// ---------------------------------------------------------------------------
// Mock dataset
// ---------------------------------------------------------------------------

pub const GOKO_ROOT: &str = "/home/user/Project/goko";
const AUTH_WT: &str = "/home/user/Project/goko-auth-flow";
const DOCS_WT: &str = "/home/user/Project/goko-docs-refresh";
const NOTES_ROOT: &str = "/home/user/Project/notes";
const PID_GOKO: &str = "proj-goko";
const PID_NOTES: &str = "proj-notes";

/// Fixed clock so elapsed times and snapshots are deterministic.
pub const MOCK_NOW_MS: i64 = 1_761_395_200_000;

#[allow(clippy::too_many_arguments)]
fn sess(
    id: &str,
    title: &str,
    argv: &[&str],
    cwd: &str,
    state: SessionState,
    project: Option<&str>,
    agent: Option<&str>,
    activity: Option<Activity>,
) -> SessionView {
    let info = SessionInfo {
        id: id.to_string(),
        title: title.to_string(),
        argv: argv.iter().map(|s| s.to_string()).collect(),
        cwd: cwd.to_string(),
        state,
        exit_code: None,
        error: None,
        started_at: MOCK_NOW_MS - 3_600_000,
        ended_at: state.ended().then_some(MOCK_NOW_MS - 3_000_000),
    };
    let meta = agent.map(|agent| SessionMeta {
        session_id: id.to_string(),
        project_id: project.map(str::to_string),
        agent: agent.to_string(),
        argv: info.argv.clone(),
        cwd: info.cwd.clone(),
        title: info.title.clone(),
        created_at: MOCK_NOW_MS - 3_600_000,
    });
    SessionView {
        info,
        meta,
        activity,
        context_pct: None,
    }
}

/// The tab strip's New tab button: mock `session_create` for a running shell
/// under the goko root checkout. Returns the tab title; `Shell N` numbers past
/// the highest live shell session so titles never collide after closes. The
/// caller pushes the title onto `open_tabs` and `sync_selection` resolves it
/// like any other.
pub fn spawn_shell(data: &mut Data) -> String {
    let n = data
        .sessions
        .iter()
        .filter_map(|v| {
            v.title()
                .strip_prefix("Shell ")
                .and_then(|n| n.parse::<usize>().ok())
        })
        .max()
        .unwrap_or(1)
        + 1;
    let title = format!("Shell {n}");
    let id = format!("sess-shell-new-{n}");
    data.sessions.push(sess(
        &id,
        &title,
        &["/bin/zsh"],
        GOKO_ROOT,
        SessionState::Running,
        Some(PID_GOKO),
        Some("shell"),
        None,
    ));
    title
}

/// The dataset behind the app. Swap point for real data: everything below this
/// function's return value is what the views consume.
pub fn mock_data() -> Data {
    let projects = vec![
        Project {
            id: PID_GOKO.to_string(),
            name: "goko".to_string(),
            root: GOKO_ROOT.to_string(),
            created_at: MOCK_NOW_MS - 86_400_000,
            missing: false,
        },
        Project {
            id: PID_NOTES.to_string(),
            name: "notes".to_string(),
            root: NOTES_ROOT.to_string(),
            created_at: MOCK_NOW_MS - 43_200_000,
            missing: false,
        },
    ];

    let mut worktrees = HashMap::new();
    worktrees.insert(
        PID_GOKO.to_string(),
        ProjectWorktrees {
            repo: true,
            worktrees: vec![
                WorktreeInfo {
                    name: "goko".to_string(),
                    path: GOKO_ROOT.to_string(),
                    branch: Some("main".to_string()),
                    head: "3f9a1c2e8b7d4f6a0c5e9b2d8f1a4c7e0b3d6f9a".to_string(),
                    is_root: true,
                    missing: false,
                    dirty: Some(false),
                    changed: Some(0),
                },
                WorktreeInfo {
                    name: "auth-flow".to_string(),
                    path: AUTH_WT.to_string(),
                    branch: Some("fix/auth-flow".to_string()),
                    head: "8c4d2e1f9a0b3c5d7e8f2a4b6c8d0e2f4a6b8c0d".to_string(),
                    is_root: false,
                    missing: false,
                    dirty: Some(true),
                    changed: Some(3),
                },
                WorktreeInfo {
                    name: "docs-refresh".to_string(),
                    path: DOCS_WT.to_string(),
                    branch: Some("docs/refresh".to_string()),
                    head: "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b".to_string(),
                    is_root: false,
                    missing: false,
                    dirty: Some(false),
                    changed: Some(0),
                },
            ],
        },
    );
    worktrees.insert(
        PID_NOTES.to_string(),
        ProjectWorktrees {
            repo: false,
            worktrees: vec![],
        },
    );

    let mut sessions = vec![
        // goko · root checkout — every state, agent and row variant.
        sess(
            "sess-claude-auth",
            "fix-auth-flow",
            &["claude", "--session-id", "u-claude-auth"],
            GOKO_ROOT,
            SessionState::Running,
            Some(PID_GOKO),
            Some("claude"),
            Some(Activity::Working),
        ),
        sess(
            "sess-devin-spike",
            "rune-spike",
            &["devin"],
            GOKO_ROOT,
            SessionState::Running,
            Some(PID_GOKO),
            Some("devin"),
            Some(Activity::Done),
        ),
        sess(
            "sess-codex-migrate",
            "db-migration",
            &["codex", "migrate", "up"],
            &format!("{GOKO_ROOT}/crates/daemon"),
            SessionState::Exited,
            Some(PID_GOKO),
            Some("codex"),
            None,
        ),
        sess(
            "sess-claude-review",
            "review-pr-842",
            &["claude", "--session-id", "u-claude-review"],
            GOKO_ROOT,
            SessionState::Running,
            Some(PID_GOKO),
            Some("claude"),
            Some(Activity::Waiting),
        ),
        // "Shell"/"Shell" collapses to a single title, like the real row.
        sess(
            "sess-shell-zsh",
            "Shell",
            &["/bin/zsh"],
            GOKO_ROOT,
            SessionState::Created,
            Some(PID_GOKO),
            Some("shell"),
            None,
        ),
        sess(
            "sess-opencode-bench",
            "bench-parser",
            &["opencode", "run", "bench"],
            GOKO_ROOT,
            SessionState::Failed,
            Some(PID_GOKO),
            Some("opencode"),
            None,
        ),
        sess(
            "sess-custom-deploy",
            "one-off-script",
            &["sh", "-c", "./deploy.sh"],
            GOKO_ROOT,
            SessionState::Running,
            Some(PID_GOKO),
            Some("custom"),
            None,
        ),
        // goko · auth-flow worktree.
        sess(
            "sess-claude-tests",
            "fix-auth-tests",
            &["claude", "--session-id", "u-claude-tests"],
            AUTH_WT,
            SessionState::Running,
            Some(PID_GOKO),
            Some("claude"),
            Some(Activity::Done),
        ),
        sess(
            "sess-devin-mw",
            "port-auth-middleware",
            &["devin"],
            &format!("{AUTH_WT}/src"),
            SessionState::WaitingForInput,
            Some(PID_GOKO),
            Some("devin"),
            None,
        ),
        // goko · docs-refresh worktree.
        sess(
            "sess-shell-docs",
            "Shell",
            &["/bin/zsh"],
            DOCS_WT,
            SessionState::Exited,
            Some(PID_GOKO),
            Some("shell"),
            None,
        ),
        // goko · other sessions: cwd matches no checkout.
        sess(
            "sess-opencode-adhoc",
            "adhoc-bench",
            &["opencode"],
            "/home/user/scratch/bench",
            SessionState::Running,
            Some(PID_GOKO),
            Some("opencode"),
            Some(Activity::Waiting),
        ),
        sess(
            "sess-codex-rebase",
            "rebase-helper",
            &["codex"],
            "/tmp/gh-cli",
            SessionState::Interrupted,
            Some(PID_GOKO),
            Some("codex"),
            None,
        ),
        // notes · non-git project: every session is a "root" session.
        sess(
            "sess-claude-notes",
            "todo-triage",
            &["claude", "--session-id", "u-claude-notes"],
            NOTES_ROOT,
            SessionState::Running,
            Some(PID_NOTES),
            Some("claude"),
            Some(Activity::Working),
        ),
        sess(
            "sess-codex-index",
            "index-notes",
            &["codex"],
            NOTES_ROOT,
            SessionState::Starting,
            Some(PID_NOTES),
            Some("codex"),
            None,
        ),
        // Unassigned: meta with a null project, and no meta row at all.
        sess(
            "sess-devin-scratch",
            "scratch-pad",
            &["devin"],
            "/tmp",
            SessionState::Running,
            None,
            Some("devin"),
            Some(Activity::Done),
        ),
        sess(
            "sess-mystery",
            "mystery-session",
            &["python3", "train.py"],
            "/home/user",
            SessionState::Idle,
            None,
            None,
            None,
        ),
    ];

    // The selected session's elapsed reads "14:32", like the desktop controls.
    if let Some(s) = sessions
        .iter_mut()
        .find(|s| s.info.id == "sess-devin-spike")
    {
        s.info.started_at = MOCK_NOW_MS - 872_000;
    }

    // The real app resolves context% through `usage_get`: only Claude argv
    // carries `--session-id`, and only ids in the snapshot get a chip.
    let claude_context: HashMap<&str, f64> = [
        ("u-claude-auth", 82.0),
        ("u-claude-review", 91.0),
        ("u-claude-tests", 67.0),
        ("u-claude-notes", 45.0),
    ]
    .into_iter()
    .collect();
    for s in &mut sessions {
        s.context_pct =
            claude_session_id(&s.info.argv).and_then(|id| claude_context.get(id).copied());
    }

    Data {
        projects,
        sessions,
        worktrees,
        agents: [
            ("shell", "Shell"),
            ("claude", "Claude Code"),
            ("codex", "Codex"),
            ("opencode", "OpenCode"),
            ("devin", "Devin"),
        ]
        .into_iter()
        .map(|(id, label)| AgentInfo {
            id: id.to_string(),
            label: label.to_string(),
            available: true,
            path: Some(format!("/usr/local/bin/{id}")),
        })
        .collect(),
        tabs: vec![
            "Shell".to_string(),
            "fix-auth-flow".to_string(),
            "rune-spike".to_string(),
        ],
        active_tab: 2,
        usage: vec![
            UsageChip {
                name: "Claude",
                limits: &[("5h", 82), ("7d", 45)],
            },
            UsageChip {
                name: "Codex",
                limits: &[("5h", 34), ("wk", 62)],
            },
        ],
        connected: true,
    }
}
