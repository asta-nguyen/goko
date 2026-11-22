use super::*;
use crate::editor::backend::channel_backend;
use crate::panes;
use crate::sessions::Pending;
use app_core::accounts::{AccountView, AccountsView, AgentView};
use app_core::agents::AgentInfo;
use app_core::handoff::ForkSource;
use app_core::store::SessionMeta;
use protocol::session::{SessionId, SessionInfo, SessionState};
use std::sync::mpsc::{self, Receiver};

const PRESETS: &[(&str, &str)] = &[
    ("shell", "Shell"),
    ("claude", "Claude Code"),
    ("codex", "Codex"),
    ("opencode", "OpenCode"),
    ("devin", "Devin"),
    ("agy", "Antigravity"),
];

fn agent_view(named: &[(&str, &str, &str)]) -> AgentView {
    let mut items = vec![AccountView {
        id: DEFAULT_ID.into(),
        name: "Default".into(),
        dir: "/home/.claude".into(),
        is_default: true,
    }];
    items.extend(named.iter().map(|(id, name, dir)| AccountView {
        id: (*id).into(),
        name: (*name).into(),
        dir: (*dir).into(),
        is_default: false,
    }));
    AgentView {
        active: DEFAULT_ID.into(),
        items,
    }
}

/// One tab split into a shell pane and a Claude preset pane (focused), in
/// project `p1`; every preset is installed except Antigravity.
fn scene(state: SessionState) -> (State, Receiver<Job>, String, String) {
    let mut s = State::new(crate::test_handle());
    let (backend, rx) = channel_backend();
    s.editor.backend = backend;
    s.data.agents = PRESETS
        .iter()
        .map(|(id, label)| AgentInfo {
            id: (*id).into(),
            label: (*label).into(),
            available: *id != "agy",
            path: Some(format!("/bin/{id}")),
            binary: Some((*id).into()),
        })
        .collect();
    s.data.accounts = Some(AccountsView {
        claude: agent_view(&[("work", "Work", "/accounts/work")]),
        codex: agent_view(&[]),
        devin: agent_view(&[]),
    });
    let mut keys = Vec::new();
    for (agent, argv, project) in [
        ("shell", vec!["/bin/zsh".to_string()], Some("p1")),
        (
            "claude",
            vec![
                "/usr/bin/env".into(),
                "CLAUDE_CONFIG_DIR=/accounts/work".into(),
                "/bin/claude".into(),
            ],
            Some("p1"),
        ),
    ] {
        let id = SessionId::new();
        let key = id.as_uuid().to_string();
        s.session_metas.insert(
            key.clone(),
            SessionMeta {
                session_id: key.clone(),
                project_id: project.map(str::to_owned),
                agent: agent.into(),
                argv: argv.clone(),
                cwd: "/work".into(),
                title: agent.into(),
                created_at: 0,
                workspace_id: None,
            },
        );
        s.upsert_session(SessionInfo {
            id,
            title: agent.into(),
            argv,
            cwd: "/work".into(),
            state: if agent == "claude" {
                state
            } else {
                SessionState::Running
            },
            pid: Some(10),
            exit_code: None,
            error: None,
            started_at: 1,
            ended_at: None,
        });
        keys.push(key);
    }
    s.place_session(&keys[0], "shell", panes::Placement::Tab);
    let tab = s.open_tabs[0].id.clone();
    let shell_pane = panes::collect_panes(&s.open_tabs[0].layout)[0].clone();
    let claude_pane = panes::Pane {
        session: keys[1].clone(),
        ..shell_pane.clone()
    };
    let claude_pane = panes::Pane {
        id: format!("{}-b", shell_pane.id),
        ..claude_pane
    };
    s.open_tabs[0].layout = panes::Layout::Split(panes::Split {
        id: "split".into(),
        direction: panes::Direction::SideBySide,
        ratio: 0.5,
        first: Box::new(panes::Layout::Pane(shell_pane)),
        second: Box::new(panes::Layout::Pane(claude_pane.clone())),
    });
    s.open_tabs[0].active_pane = claude_pane.id;
    (s, rx, tab, keys[1].clone())
}

fn jobs(rx: &Receiver<Job>) -> Vec<Req> {
    rx.try_iter().map(|job| job.req).collect()
}

fn prepared(draft: &str, fork: bool) -> Prepared {
    Prepared {
        draft: draft.into(),
        status: "Conversation abc — PID-linked session file".into(),
        conversation: Some("abc".into()),
        shortened: false,
        omitted: 0,
        accepts_manual_id: true,
        fork: fork.then(|| ForkSource {
            id: "abc".into(),
            file: "/accounts/work/projects/-work/abc.jsonl".into(),
        }),
    }
}

/// Open the dialog and answer its review request.
fn opened(state: SessionState, fork: bool) -> (State, Receiver<Job>, String) {
    let (mut s, rx, tab, key) = scene(state);
    open(&mut s, &tab, &mut Tx::default());
    let token = s.handoff.token;
    jobs(&rx);
    apply_prepared(
        &mut s,
        token,
        Ok((
            prepared("# Handoff\n\nUser:\nFix it\n\n## Next step\n\n", fork),
            vec![DEFAULT_ID.into()],
        )),
    );
    (s, rx, key)
}

#[test]
fn opening_reads_the_focused_split_pane_and_drops_stale_reviews() {
    let (mut s, rx, tab, key) = scene(SessionState::Running);
    assert_eq!(unavailable(&s, &tab), None);
    open(&mut s, &tab, &mut Tx::default());
    let sent = jobs(&rx);
    let [
        Req::PrepareHandoff {
            token,
            session,
            agent,
            project_id,
            checkout,
            manual_id,
        },
    ] = sent.as_slice()
    else {
        panic!("one review request: {sent:?}");
    };
    assert_eq!(
        session.id.as_uuid().to_string(),
        key,
        "the focused pane, not the first"
    );
    assert_eq!(
        (agent.as_str(), project_id.as_str(), checkout.as_str()),
        ("claude", "p1", "/work")
    );
    assert!(manual_id.is_none());
    assert_eq!(
        s.handoff.source_account, "Work",
        "named from the launch's folder"
    );
    assert_eq!(s.handoff.agent, "claude");
    assert_eq!(s.handoff.account, DEFAULT_ID);

    apply_prepared(
        &mut s,
        token - 1,
        Ok((prepared("stale", false), Vec::new())),
    );
    assert_eq!(
        s.handoff.phase,
        Phase::Preparing,
        "a stale reply is ignored"
    );
    apply_prepared(&mut s, *token, Ok((prepared("draft", false), Vec::new())));
    assert_eq!(s.handoff.phase, Phase::Review);
    assert_eq!(s.handoff.review.text, "draft");
}

#[test]
fn an_exited_pane_can_hand_off_but_shells_and_projectless_panes_cannot() {
    let (mut s, rx, tab, key) = scene(SessionState::Exited);
    open(&mut s, &tab, &mut Tx::default());
    assert!(matches!(
        jobs(&rx).as_slice(),
        [Req::PrepareHandoff { session, .. }] if session.state == SessionState::Exited
    ));
    close(&mut s);
    let shell = panes::collect_panes(&s.open_tabs[0].layout)[0].id.clone();
    s.open_tabs[0].active_pane = shell;
    assert!(unavailable(&s, &tab).unwrap().contains("not agent presets"));
    s.open_tabs[0].active_pane = format!("{}-b", s.open_tabs[0].active_pane);
    if let Some(view) = s.data.sessions.iter_mut().find(|view| view.info.id == key) {
        view.meta.as_mut().unwrap().project_id = None;
    }
    assert!(
        unavailable(&s, &tab)
            .unwrap()
            .contains("registered project")
    );
    open(&mut s, &tab, &mut Tx::default());
    assert!(!s.handoff.open && jobs(&rx).is_empty());
}

#[test]
fn the_picker_follows_the_registry_and_each_presets_accounts() {
    let (s, _rx, _) = opened(SessionState::Running, false);
    let agents: Vec<String> = destinations(&s).into_iter().map(|(id, _)| id).collect();
    assert_eq!(
        agents,
        ["claude", "codex", "opencode", "devin"],
        "installed presets, no shell"
    );
    let claude: Vec<String> = accounts(&s, "claude")
        .into_iter()
        .map(|(_, name)| name)
        .collect();
    if cfg!(windows) {
        assert_eq!(claude, ["Default"]);
    } else {
        assert_eq!(claude, ["Default", "Work"]);
    }
    for agent in ["opencode", "agy"] {
        assert_eq!(
            accounts(&s, agent),
            [(DEFAULT_ID.to_string(), "Default".to_string())]
        );
    }
}

#[test]
fn edits_survive_destination_changes_and_an_oversized_review_cannot_open() {
    let (mut s, rx, _) = opened(SessionState::Running, false);
    s.handoff.review = TextEdit::new("my own notes");
    select_account(&mut s, "work");
    select_agent(&mut s, "codex");
    assert_eq!(s.handoff.review.text, "my own notes");
    assert_eq!(s.handoff.account, DEFAULT_ID, "codex has no Work account");
    select_agent(&mut s, "claude");
    assert_eq!(s.handoff.review.text, "my own notes");

    s.handoff.review = TextEdit::new("x".repeat(DRAFT_LIMIT + 1));
    confirm(&mut s);
    assert!(jobs(&rx).is_empty(), "over the limit nothing is resolved");
    assert!(s.handoff.error.as_deref().unwrap().contains("over"));
}

#[test]
fn a_manual_id_asks_for_that_conversation_again() {
    let (mut s, rx, _) = opened(SessionState::Exited, false);
    s.handoff.manual_id = TextEdit::new(" ses_123 ");
    load_manual_id(&mut s);
    assert!(matches!(
        jobs(&rx).as_slice(),
        [Req::PrepareHandoff { manual_id: Some(id), .. }] if id == "ses_123"
    ));
    assert_eq!(s.handoff.phase, Phase::Preparing);
}

/// Swap the create's daemon reply for one the test answers.
fn answer_create(s: &mut State) -> mpsc::Sender<Result<SessionInfo, app_core::error::BridgeError>> {
    let index = s
        .pending
        .iter()
        .position(|p| {
            matches!(
                p,
                Pending::Create {
                    handoff: Some(_),
                    ..
                }
            )
        })
        .expect("the handoff create went out");
    let Pending::Create {
        launch,
        placement,
        workspace_id,
        handoff,
        ..
    } = s.pending.remove(index)
    else {
        unreachable!()
    };
    let (tx, reply) = mpsc::channel();
    s.pending.push(Pending::Create {
        reply,
        launch,
        placement,
        workspace_id,
        handoff,
    });
    tx
}

fn created_session() -> SessionInfo {
    SessionInfo {
        id: SessionId::new(),
        title: "codex".into(),
        argv: vec!["/bin/codex".into()],
        cwd: "/work".into(),
        state: SessionState::Running,
        pid: Some(30),
        exit_code: None,
        error: None,
        started_at: 2,
        ended_at: None,
    }
}

#[test]
fn the_reviewed_text_is_copied_only_after_the_matching_create_succeeds() {
    let (mut s, rx, key) = opened(SessionState::Running, false);
    let source = s.session_by_key(&key).unwrap().info.clone();
    select_agent(&mut s, "codex");
    s.handoff.review = TextEdit::new("Continue the parser");
    confirm(&mut s);
    confirm(&mut s);
    let sent = jobs(&rx);
    let [
        Req::HandoffArgv {
            token,
            agent,
            account,
            project_id,
            checkout,
            fork,
        },
    ] = sent.as_slice()
    else {
        panic!("exactly one launch request: {sent:?}");
    };
    assert_eq!(
        (
            agent.as_str(),
            account.as_str(),
            project_id.as_str(),
            checkout.as_str()
        ),
        ("codex", DEFAULT_ID, "p1", "/work")
    );
    assert!(fork.is_none());
    assert!(
        !format!("{sent:?}").contains("Continue the parser"),
        "the review never goes to the worker"
    );

    // Edits after confirming cannot change what is copied.
    s.handoff.review = TextEdit::new("changed later");
    apply_argv(
        &mut s,
        *token,
        Ok((vec!["/bin/codex".into()], "/work".into())),
    );
    assert_eq!(s.handoff.phase, Phase::Creating);
    close(&mut s);
    assert!(s.handoff.open, "cancel waits once creation is dispatched");
    let answer = answer_create(&mut s);
    assert!(
        s.handoff_copy.is_none(),
        "nothing is copied before the session exists"
    );

    answer.send(Ok(created_session())).unwrap();
    assert!(s.drain_pending());
    assert_eq!(s.handoff_copy.as_deref(), Some("Continue the parser"));
    assert!(!s.handoff.open);
    let after = s.session_by_key(&key).unwrap();
    assert_eq!(
        (after.info.argv.clone(), after.info.state),
        (source.argv, source.state),
        "the source pane is untouched"
    );
    assert!(
        jobs(&rx)
            .iter()
            .all(|req| !matches!(req, Req::AccountSetActive { .. }))
    );
}

#[test]
fn a_failed_create_or_launch_keeps_the_review_and_copies_nothing() {
    let (mut s, rx, _) = opened(SessionState::Running, false);
    confirm(&mut s);
    let token = s.handoff.token;
    jobs(&rx);
    apply_argv(
        &mut s,
        token,
        Err("the account folder no longer exists".into()),
    );
    assert_eq!(s.handoff.phase, Phase::Review);
    assert!(s.handoff.copy.is_none());
    assert!(
        s.handoff
            .error
            .as_deref()
            .unwrap()
            .contains("no longer exists")
    );

    confirm(&mut s);
    let token = s.handoff.token;
    apply_argv(
        &mut s,
        token,
        Ok((vec!["/bin/claude".into()], "/work".into())),
    );
    let answer = answer_create(&mut s);
    answer
        .send(Err(app_core::error::BridgeError::new(
            app_core::error::Code::Storage,
            "spawn failed",
        )))
        .unwrap();
    s.drain_pending();
    assert!(s.handoff.open && s.handoff.phase == Phase::Review);
    assert!(s.handoff.error.as_deref().unwrap().contains("spawn failed"));
    assert!(s.handoff_copy.is_none());
    assert!(s.handoff.review.text.contains("Fix it"));
}

#[test]
fn cancelled_or_orphaned_launches_never_start() {
    let (mut s, rx, key) = opened(SessionState::Running, false);
    confirm(&mut s);
    let token = s.handoff.token;
    close(&mut s);
    apply_argv(
        &mut s,
        token,
        Ok((vec!["/bin/claude".into()], "/work".into())),
    );
    assert!(
        !s.pending
            .iter()
            .any(|p| matches!(p, Pending::Create { .. }))
    );

    let _ = key;
    let (mut s, _rx2, key) = opened(SessionState::Running, false);
    confirm(&mut s);
    let token = s.handoff.token;
    s.data.sessions.retain(|view| view.info.id != key);
    apply_argv(
        &mut s,
        token,
        Ok((vec!["/bin/claude".into()], "/work".into())),
    );
    assert!(
        s.handoff
            .error
            .as_deref()
            .unwrap()
            .contains("no longer available")
    );
    assert!(
        !s.pending
            .iter()
            .any(|p| matches!(p, Pending::Create { .. }))
    );
    let _ = rx;
}

#[test]
fn a_fork_needs_a_sharing_claude_account_and_copies_nothing() {
    let (mut s, rx, _) = opened(SessionState::Running, true);
    assert!(fork_offered(&s.handoff));
    select_account(&mut s, "work");
    select_fork(&mut s, true);
    assert!(!s.handoff.fork, "Work does not share the history");
    select_account(&mut s, DEFAULT_ID);
    select_fork(&mut s, true);
    assert!(s.handoff.fork);
    select_agent(&mut s, "codex");
    assert!(
        !s.handoff.fork && !fork_offered(&s.handoff),
        "only Claude to Claude"
    );
    select_agent(&mut s, "claude");
    select_fork(&mut s, true);
    confirm(&mut s);
    assert!(matches!(
        jobs(&rx).as_slice(),
        [Req::HandoffArgv { fork: Some(fork), .. }] if fork.id == "abc"
    ));
    assert!(s.handoff.copy.is_none(), "a fork copies nothing");
}

#[test]
fn the_dialog_renders_its_controls_and_escape_closes_it() {
    let (s, _rx, _) = opened(SessionState::Running, true);
    let mut rt = rune::Runtime::new(s, crate::view, (1100.0, 800.0), rune::Env::new(true, 2.0));
    rt.on_key = Some(crate::on_key);
    rt.frame(0.0);
    for label in [
        "Hand off to…",
        "Codex",
        "Work",
        "New session with context",
        "Fork Claude conversation",
        "Open & Copy Context",
        "Cancel",
    ] {
        assert!(
            rt.elements()
                .iter()
                .any(|e| !e.exiting && e.text.as_deref() == Some(label)),
            "{label} mounted"
        );
    }
    rt.key(rune::KeyEvent {
        key: Key::Escape,
        mods: Default::default(),
    });
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.handoff.open);
}

#[test]
fn the_poll_puts_the_released_copy_on_the_clipboard_once() {
    let (s, _rx, _) = opened(SessionState::Running, false);
    let mut rt = rune::Runtime::new(s, crate::view, (1100.0, 800.0), rune::Env::new(true, 2.0));
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    rt.poll = Some(crate::poll);
    rt.frame(0.0);
    assert!(
        clipboard.text().is_none(),
        "opening the review copies nothing"
    );
    rt.update(|s, _| s.handoff_copy = Some("approved text".into()));
    rt.frame(1.0 / 60.0);
    assert_eq!(
        clipboard.text().map(|text| text.to_string()).as_deref(),
        Some("approved text")
    );
    assert!(rt.state.handoff_copy.is_none());
}
