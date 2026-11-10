//! The app's session data layer: the daemon's sessions joined with the app's
//! stored session metadata, plus the join helpers the sidebar tree consumes.
//!
//! `SessionView` is the one app-side join the tree consumes: the daemon's
//! `SessionInfo` plus its optional `SessionMeta` row. The shapes mirror the
//! wire and store types field-for-field, so a daemon answer drops straight in
//! and the view code never learns where the data came from.

use std::collections::{HashMap, HashSet};

use app_core::projects::ProjectView;
use app_core::store::SessionMeta;
use protocol::session::{SessionId, SessionInfo as DaemonSession, SessionState as DaemonState};
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

    /// The session is doing work: its dot spins instead of sitting still.
    /// The daemon has no activity feed, so the state is the only signal.
    pub fn working(self) -> bool {
        matches!(self, SessionState::Running | SessionState::Starting)
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

    /// `StateDot`: the tone dot the row carries. A working session draws a
    /// spinning accent ring over it instead (see `theme::spinner`).
    pub fn dot_color(self, ink: &Ink) -> Color {
        self.badge_tone(ink)
    }
}

impl From<DaemonState> for SessionState {
    fn from(state: DaemonState) -> SessionState {
        match state {
            DaemonState::Created => SessionState::Created,
            DaemonState::Starting => SessionState::Starting,
            DaemonState::Running => SessionState::Running,
            DaemonState::WaitingForInput => SessionState::WaitingForInput,
            DaemonState::Idle => SessionState::Idle,
            DaemonState::Stopping => SessionState::Stopping,
            DaemonState::Exited => SessionState::Exited,
            DaemonState::Failed => SessionState::Failed,
            DaemonState::Disconnected => SessionState::Disconnected,
            DaemonState::Interrupted => SessionState::Interrupted,
            DaemonState::Unknown => SessionState::Unknown,
        }
    }
}

/// `crates/protocol` `SessionInfo` (times are Unix epoch milliseconds). The
/// daemon's uuid becomes the string the UI keys sessions by; the wire id
/// stays alongside it for the requests that name it.
#[derive(Clone, Debug)]
pub struct SessionInfo {
    pub id: String,
    /// The id the daemon knows the session by.
    pub session_id: SessionId,
    pub title: String,
    pub argv: Vec<String>,
    pub cwd: String,
    pub state: SessionState,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
}

impl SessionInfo {
    /// The daemon's uuid as the string the UI keys on.
    pub fn key(id: SessionId) -> String {
        id.as_uuid().to_string()
    }
}

impl From<DaemonSession> for SessionInfo {
    fn from(info: DaemonSession) -> SessionInfo {
        SessionInfo {
            id: SessionInfo::key(info.id),
            session_id: info.id,
            title: info.title,
            argv: info.argv,
            cwd: info.cwd,
            state: info.state.into(),
            exit_code: info.exit_code,
            error: info.error,
            started_at: info.started_at,
            ended_at: info.ended_at,
        }
    }
}

/// src-tauri `WorktreeInfo` — the real `app_core::worktrees` shape.
pub use app_core::worktrees::WorktreeInfo;

/// src-tauri `WorktreeList`: `repo` is false for non-git projects.
pub use app_core::worktrees::WorktreeList;

/// src-tauri `AgentInfo` (`agent_list`); the five presets from `agents.rs`.
pub use app_core::agents::AgentInfo;

/// One sidebar row's worth of session data.
#[derive(Clone, Debug)]
pub struct SessionView {
    pub info: SessionInfo,
    /// The app-side join row; `None` renders under Unassigned with no label.
    pub meta: Option<SessionMeta>,
}

impl SessionView {
    /// Join a daemon session with its stored metadata row.
    pub fn new(info: DaemonSession, meta: Option<SessionMeta>) -> SessionView {
        SessionView {
            info: info.into(),
            meta,
        }
    }

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

/// Everything the view needs, produced by the daemon client and the store.
#[derive(Clone, Debug, Default)]
pub struct Data {
    /// The daemon's sessions, joined with their stored metadata.
    pub sessions: Vec<SessionView>,
    /// The agents the backend discovered; the sidebar's New-session menu
    /// lists them.
    pub agents: Vec<AgentInfo>,
    /// Usage chips — empty until the usage service lands.
    pub usage: Vec<UsageChip>,
    /// Whether the daemon client is connected.
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
/// it (`cwd == path || cwd.starts_with(path + '/')`, trailing slashes ignored).
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

/// ProjectTree's `byProject`/`unassigned` partition: a session lands under
/// its meta's project only when that id is a REAL registered project —
/// `known` comes from `editor.projects`, so a stale project id can never
/// masquerade as editor identity and such a session renders under
/// Unassigned.
pub fn partition<'a>(
    data: &'a Data,
    known: &HashSet<&str>,
) -> (HashMap<&'a str, Vec<&'a SessionView>>, Vec<&'a SessionView>) {
    let mut by_project: HashMap<&'a str, Vec<&'a SessionView>> = HashMap::new();
    let mut unassigned = Vec::new();
    for session in &data.sessions {
        match session.meta.as_ref().and_then(|m| m.project_id.as_deref()) {
            Some(id) if known.contains(id) => by_project.entry(id).or_default().push(session),
            _ => unassigned.push(session),
        }
    }
    (by_project, unassigned)
}

/// The registered project a session belongs to and its checkouts: what a
/// session's workspace key is resolved from.
pub struct ProjectCheckout<'a> {
    pub project: &'a ProjectView,
    /// The project's worktree list; `None` until it loads.
    pub worktrees: Option<&'a WorktreeList>,
}

/// Workspace a session row activates: its checkout's key for git projects,
/// the project root's key otherwise, `UNASSIGNED_WORKSPACE` with no project —
/// mirrors the workspace_id each sidebar group passes to SessionItems. Only
/// a registered project can produce a real workspace key; anything else
/// resolves to Unassigned so a stale project id never fabricates editor
/// identity.
pub fn workspace_for_session(sv: &SessionView, owner: Option<&ProjectCheckout<'_>>) -> String {
    let Some(owner) = owner else {
        return UNASSIGNED_WORKSPACE.to_string();
    };
    let Some(pid) = sv.meta.as_ref().and_then(|m| m.project_id.as_deref()) else {
        return UNASSIGNED_WORKSPACE.to_string();
    };
    if pid != owner.project.project.id {
        return UNASSIGNED_WORKSPACE.to_string();
    }
    let worktrees = owner.worktrees;
    let is_git = worktrees.is_some_and(|w| w.repo);
    if is_git
        && let Some(list) = worktrees
        && let Some(path) = checkout_for_cwd(&list.worktrees, &sv.info.cwd)
    {
        return checkout_key(pid, path);
    }
    checkout_key(pid, &owner.project.project.root)
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

/// The wall clock in Unix epoch milliseconds — the elapsed counter's source.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::session::SessionInfo as DaemonSession;

    fn daemon_session(state: DaemonState) -> DaemonSession {
        DaemonSession {
            id: SessionId::new(),
            title: "shell".into(),
            argv: vec!["/bin/zsh".into()],
            cwd: "/work/repo".into(),
            state,
            exit_code: None,
            error: None,
            started_at: 1_700_000_000_000,
            ended_at: None,
        }
    }

    #[test]
    fn a_daemon_session_maps_onto_the_view_shape() {
        let info = daemon_session(DaemonState::Running);
        let key = SessionInfo::key(info.id);
        let view = SessionView::new(info, None);

        assert_eq!(view.info.id, key);
        assert_eq!(view.info.state, SessionState::Running);
        assert_eq!(view.title(), "shell");
        assert!(view.info.state.working());
        assert!(!view.ended());
    }

    #[test]
    fn an_empty_title_falls_back_to_the_argv() {
        let mut info = daemon_session(DaemonState::Idle);
        info.title = String::new();
        info.argv = vec!["/bin/zsh".into(), "-l".into()];
        let view = SessionView::new(info, None);
        assert_eq!(view.title(), "/bin/zsh -l");
    }

    #[test]
    fn a_session_without_a_meta_row_is_unassigned() {
        let view = SessionView::new(daemon_session(DaemonState::Running), None);
        assert_eq!(
            workspace_for_session(&view, None),
            UNASSIGNED_WORKSPACE.to_string()
        );
    }

    #[test]
    fn the_elapsed_counter_formats_minutes_and_seconds() {
        assert_eq!(format_elapsed(14_000), "00:14");
        assert_eq!(format_elapsed(872_000), "14:32");
        assert_eq!(format_elapsed(3_725_000), "1:02:05");
    }
}
