use super::*;
use crate::test_support::TempDir;
use serde_json::json;

const ID: &str = "00000000-0000-0000-0000-0000000000aa";

fn write_jsonl(path: &Path, records: &[Value]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text: String = records.iter().map(|record| format!("{record}\n")).collect();
    fs::write(path, text).unwrap();
}

fn texts(transcript: &Transcript) -> Vec<(Role, &str)> {
    transcript
        .messages
        .iter()
        .map(|message| (message.role, message.text.as_str()))
        .collect()
}

fn big() -> String {
    "x".repeat(RECORD_LIMIT + 1)
}

struct Fixture {
    _root: TempDir,
    profile: PathBuf,
    checkout: PathBuf,
    other: PathBuf,
}

fn fixture(label: &str) -> Fixture {
    let root = TempDir::new(label);
    let profile = root.path().join("profile");
    let checkout = root.path().join("work");
    let other = root.path().join("elsewhere");
    for dir in [&profile, &checkout, &other] {
        fs::create_dir_all(dir).unwrap();
    }
    Fixture {
        _root: root,
        profile,
        checkout,
        other,
    }
}

// ---- Claude ----

fn claude_records(cwd: &Path) -> Vec<Value> {
    let cwd = cwd.display().to_string();
    vec![
        json!({"type":"user","uuid":"u1","parentUuid":null,"cwd":cwd,"message":{"content":"Fix the bug"}}),
        json!({"type":"assistant","uuid":"a1","parentUuid":"u1","message":{"content":[{"type":"thinking","thinking":"secret thought"}]}}),
        json!({"type":"assistant","uuid":"a2","parentUuid":"a1","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"rm"}}]}}),
        json!({"type":"user","uuid":"t1","parentUuid":"a2","message":{"content":[{"type":"tool_result","content":"tool output"}]}}),
        json!({"type":"assistant","uuid":"a3","parentUuid":"t1","message":{"content":[{"type":"text","text":"Found it"}]}}),
        json!({"type":"user","uuid":"r1","parentUuid":"a3","message":{"content":"abandoned branch"}}),
        json!({"type":"user","uuid":"s1","parentUuid":"a3","isSidechain":true,"message":{"content":"sidechain text"}}),
        json!({"type":"user","uuid":"m1","parentUuid":"a3","isMeta":true,"message":{"content":"meta caveat"}}),
        json!({"type":"user","uuid":"c1","parentUuid":"m1","message":{"content":"<command-name>/clear</command-name>"}}),
        json!({"type":"user","uuid":"u3","parentUuid":"c1","message":{"content":"Now add a test"}}),
        json!({"type":"attachment","uuid":"x1","parentUuid":"u3"}),
    ]
}

fn claude_file(profile: &Path) -> PathBuf {
    profile
        .join("projects/-tmp-work")
        .join(format!("{ID}.jsonl"))
}

#[test]
fn claude_keeps_the_live_chain_without_tools_meta_sidechains_or_rewinds() {
    let f = fixture("handoff-claude");
    write_jsonl(&claude_file(&f.profile), &claude_records(&f.checkout));
    let transcript = read("claude", &f.profile, ID, &f.checkout, false).unwrap();
    assert_eq!(
        texts(&transcript),
        [
            (Role::User, "Fix the bug"),
            (Role::Assistant, "Found it"),
            (Role::User, "Now add a test"),
        ]
    );
    assert_eq!(
        transcript.file.as_deref(),
        Some(claude_file(&f.profile).as_path())
    );
    assert!(!transcript.shortened);
    assert_eq!(
        read("claude", &f.profile, ID, &f.other, false).unwrap_err(),
        ReadError::WrongCheckout
    );
    assert_eq!(
        read("claude", &f.other, ID, &f.checkout, true).unwrap_err(),
        ReadError::Missing
    );
}

#[test]
fn claude_refuses_corrupt_oversized_and_linked_history() {
    let f = fixture("handoff-claude-bad");
    let file = claude_file(&f.profile);
    let mut records = claude_records(&f.checkout);
    records.insert(1, json!({"type":"assistant","uuid":"big","parentUuid":"u1","message":{"content":[{"type":"text","text":big()}]}}));
    write_jsonl(&file, &records);
    let transcript = read("claude", &f.profile, ID, &f.checkout, false).unwrap();
    assert!(transcript.shortened, "an oversized record is skipped");

    let mut text = fs::read_to_string(&file).unwrap();
    text.insert_str(0, "{not json\n");
    fs::write(&file, &text).unwrap();
    assert!(matches!(
        read("claude", &f.profile, ID, &f.checkout, false),
        Err(ReadError::Corrupt(_))
    ));
    // An unterminated last line is a write in progress, not corruption.
    fs::write(
        &file,
        format!("{}{{\"type\":\"us", &text["{not json\n".len()..]),
    )
    .unwrap();
    assert!(read("claude", &f.profile, ID, &f.checkout, false).is_ok());

    #[cfg(unix)]
    {
        let g = fixture("handoff-claude-link");
        write_jsonl(&claude_file(&g.other), &claude_records(&g.checkout));
        fs::create_dir_all(g.profile.join("projects")).unwrap();
        std::os::unix::fs::symlink(
            g.other.join("projects/-tmp-work"),
            g.profile.join("projects/-tmp-work"),
        )
        .unwrap();
        assert!(matches!(
            read("claude", &g.profile, ID, &g.checkout, false),
            Err(ReadError::Unsafe(_))
        ));
        // The top-level shared `projects` link is allowed.
        let h = fixture("handoff-claude-shared");
        write_jsonl(&claude_file(&h.other), &claude_records(&h.checkout));
        std::os::unix::fs::symlink(h.other.join("projects"), h.profile.join("projects")).unwrap();
        assert!(read("claude", &h.profile, ID, &h.checkout, false).is_ok());
    }
}

// ---- Codex ----

fn codex_records(cwd: &Path) -> Vec<Value> {
    let message = |role: &str, kind: &str, text: &str| json!({"type":"response_item","payload":{"type":"message","role":role,"content":[{"type":kind,"text":text}]}});
    vec![
        json!({"type":"session_meta","payload":{"id":ID,"cwd":cwd.display().to_string()}}),
        message("developer", "input_text", "developer secret"),
        message("user", "input_text", "# AGENTS.md instructions for /work"),
        message(
            "user",
            "input_text",
            "<environment_context>cwd</environment_context>",
        ),
        message("user", "input_text", "Ship it"),
        json!({"type":"response_item","payload":{"type":"reasoning","summary":[{"text":"reasoning secret"}]}}),
        json!({"type":"response_item","payload":{"type":"custom_tool_call","input":"tool input"}}),
        json!({"type":"event_msg","payload":{"type":"token_count"}}),
        message("assistant", "output_text", "Shipped"),
    ]
}

fn codex_file(profile: &Path) -> PathBuf {
    profile
        .join("sessions/2026/11/22")
        .join(format!("rollout-2026-11-22T10-00-00-{ID}.jsonl"))
}

#[test]
fn codex_reads_its_own_rollout_without_injected_or_tool_text() {
    let f = fixture("handoff-codex");
    write_jsonl(&codex_file(&f.profile), &codex_records(&f.checkout));
    let transcript = read("codex", &f.profile, ID, &f.checkout, true).unwrap();
    assert_eq!(
        texts(&transcript),
        [(Role::User, "Ship it"), (Role::Assistant, "Shipped")]
    );
    assert_eq!(
        read("codex", &f.profile, ID, &f.other, false).unwrap_err(),
        ReadError::WrongCheckout
    );
    assert_eq!(
        read("codex", &f.other, ID, &f.checkout, false).unwrap_err(),
        ReadError::Missing
    );
    let mut records = codex_records(&f.checkout);
    records.insert(2, json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":big()}]}}));
    write_jsonl(&codex_file(&f.profile), &records);
    assert!(
        read("codex", &f.profile, ID, &f.checkout, false)
            .unwrap()
            .shortened
    );
    let mut text = fs::read_to_string(codex_file(&f.profile)).unwrap();
    text.push_str("{broken\n{}\n");
    fs::write(codex_file(&f.profile), text).unwrap();
    assert!(matches!(
        read("codex", &f.profile, ID, &f.checkout, false),
        Err(ReadError::Corrupt(_))
    ));
}

#[test]
fn codex_compressed_and_linked_rollouts_are_not_read() {
    let f = fixture("handoff-codex-zst");
    let file = codex_file(&f.profile);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file.with_extension("jsonl.zst"), b"\x28\xb5").unwrap();
    assert!(matches!(
        read("codex", &f.profile, ID, &f.checkout, false),
        Err(ReadError::Unsupported(why)) if why.contains("compressed")
    ));
    #[cfg(unix)]
    {
        let g = fixture("handoff-codex-link");
        write_jsonl(&codex_file(&g.other), &codex_records(&g.checkout));
        fs::create_dir_all(g.profile.join("sessions/2026/11")).unwrap();
        std::os::unix::fs::symlink(
            g.other.join("sessions/2026/11/22"),
            g.profile.join("sessions/2026/11/22"),
        )
        .unwrap();
        assert!(matches!(
            read("codex", &g.profile, ID, &g.checkout, false),
            Err(ReadError::Unsafe(_))
        ));
    }
}

// ---- Devin ----

fn devin_db(profile: &Path, cwd: &Path) -> Connection {
    let dir = profile.join("devin/cli");
    fs::create_dir_all(&dir).unwrap();
    let db = Connection::open(dir.join("sessions.db")).unwrap();
    db.execute_batch(
        "CREATE TABLE sessions(id TEXT PRIMARY KEY, working_directory TEXT NOT NULL, main_chain_id INTEGER);
         CREATE TABLE message_nodes(row_id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL,
           node_id INTEGER NOT NULL, parent_node_id INTEGER, chat_message TEXT NOT NULL, UNIQUE(session_id, node_id));",
    )
    .unwrap();
    db.execute(
        "INSERT INTO sessions VALUES('private-peanut', ?1, 6)",
        [cwd.display().to_string()],
    )
    .unwrap();
    for (node, parent, message) in [
        (1, None, json!({"role":"system","content":"system prompt"})),
        (2, Some(1), json!({"role":"user","content":"Plan it"})),
        (
            3,
            Some(2),
            json!({"role":"assistant","content":"abandoned retry"}),
        ),
        (
            4,
            Some(2),
            json!({"role":"assistant","content":"Plan done","thinking":"hidden","tool_calls":[{"name":"exec"}]}),
        ),
        (5, Some(4), json!({"role":"tool","content":"tool output"})),
        (6, Some(5), json!({"role":"user","content":"Go"})),
        (
            7,
            Some(4),
            json!({"role":"assistant","content":"subagent text"}),
        ),
    ] {
        db.execute(
            "INSERT INTO message_nodes(session_id, node_id, parent_node_id, chat_message) VALUES('private-peanut', ?1, ?2, ?3)",
            params![node, parent, message.to_string()],
        )
        .unwrap();
    }
    db
}

#[test]
fn devin_reads_the_main_chain_only() {
    let f = fixture("handoff-devin");
    let db = devin_db(&f.profile, &f.checkout);
    let transcript = read("devin", &f.profile, "private-peanut", &f.checkout, true).unwrap();
    assert_eq!(
        texts(&transcript),
        [
            (Role::User, "Plan it"),
            (Role::Assistant, "Plan done"),
            (Role::User, "Go"),
        ]
    );
    assert_eq!(
        read("devin", &f.profile, "private-peanut", &f.other, false).unwrap_err(),
        ReadError::WrongCheckout
    );
    assert_eq!(
        read("devin", &f.other, "private-peanut", &f.checkout, false).unwrap_err(),
        ReadError::Missing
    );
    assert_eq!(
        read("devin", &f.profile, "other-session", &f.checkout, false).unwrap_err(),
        ReadError::Missing
    );

    db.execute(
        "UPDATE message_nodes SET chat_message = ?1 WHERE node_id = 4",
        [json!({"role":"assistant","content":big()}).to_string()],
    )
    .unwrap();
    let transcript = read("devin", &f.profile, "private-peanut", &f.checkout, false).unwrap();
    assert!(transcript.shortened);
    assert_eq!(texts(&transcript), [(Role::User, "Plan it\n\nGo")]);

    db.execute(
        "UPDATE message_nodes SET chat_message = '{bad' WHERE node_id = 2",
        [],
    )
    .unwrap();
    assert!(matches!(
        read("devin", &f.profile, "private-peanut", &f.checkout, false),
        Err(ReadError::Corrupt(_))
    ));

    db.execute_batch("BEGIN EXCLUSIVE").unwrap();
    assert_eq!(
        read("devin", &f.profile, "private-peanut", &f.checkout, false).unwrap_err(),
        ReadError::Locked
    );
    db.execute_batch("ROLLBACK").unwrap();
}

#[cfg(unix)]
#[test]
fn devin_refuses_a_linked_data_folder() {
    let f = fixture("handoff-devin-link");
    devin_db(&f.other, &f.checkout);
    std::os::unix::fs::symlink(f.other.join("devin"), f.profile.join("devin")).unwrap();
    assert!(matches!(
        read("devin", &f.profile, "private-peanut", &f.checkout, false),
        Err(ReadError::Unsafe(_))
    ));
}

// ---- OpenCode ----

fn opencode_db(profile: &Path, cwd: &Path) -> Connection {
    let dir = profile.join("opencode");
    fs::create_dir_all(&dir).unwrap();
    let db = Connection::open(dir.join("opencode.db")).unwrap();
    db.execute_batch(
        "CREATE TABLE session(id TEXT PRIMARY KEY, directory TEXT NOT NULL);
         CREATE TABLE session_v2(id TEXT PRIMARY KEY);
         CREATE TABLE message(id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, data TEXT NOT NULL);
         CREATE TABLE part(id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id TEXT NOT NULL, data TEXT NOT NULL);
         INSERT INTO session_v2 VALUES('ses_new');",
    )
    .unwrap();
    db.execute(
        "INSERT INTO session VALUES('ses_1', ?1)",
        [cwd.display().to_string()],
    )
    .unwrap();
    for (id, time, role) in [("msg_1", 1, "user"), ("msg_2", 2, "assistant")] {
        db.execute(
            "INSERT INTO message VALUES(?1, 'ses_1', ?2, ?3)",
            params![id, time, json!({"role":role}).to_string()],
        )
        .unwrap();
    }
    for (id, message, part) in [
        ("prt_1", "msg_1", json!({"type":"text","text":"Hello"})),
        (
            "prt_2",
            "msg_1",
            json!({"type":"file","url":"file:///secret"}),
        ),
        (
            "prt_3",
            "msg_2",
            json!({"type":"reasoning","text":"reasoning secret"}),
        ),
        (
            "prt_4",
            "msg_2",
            json!({"type":"tool","state":{"output":"tool output"}}),
        ),
        ("prt_5", "msg_2", json!({"type":"text","text":"World"})),
        (
            "prt_6",
            "msg_2",
            json!({"type":"text","text":"synthetic note","synthetic":true}),
        ),
    ] {
        db.execute(
            "INSERT INTO part VALUES(?1, ?2, 'ses_1', ?3)",
            params![id, message, part.to_string()],
        )
        .unwrap();
    }
    db
}

#[test]
fn opencode_reads_text_parts_and_names_the_newer_format() {
    let f = fixture("handoff-opencode");
    let db = opencode_db(&f.profile, &f.checkout);
    let transcript = read("opencode", &f.profile, "ses_1", &f.checkout, true).unwrap();
    assert_eq!(
        texts(&transcript),
        [(Role::User, "Hello"), (Role::Assistant, "World")]
    );
    assert!(matches!(
        read("opencode", &f.profile, "ses_new", &f.checkout, false),
        Err(ReadError::Unsupported(why)) if why.contains("newer")
    ));
    assert_eq!(
        read("opencode", &f.profile, "ses_1", &f.other, false).unwrap_err(),
        ReadError::WrongCheckout
    );
    assert_eq!(
        read("opencode", &f.other, "ses_1", &f.checkout, false).unwrap_err(),
        ReadError::Missing
    );
    db.execute(
        "UPDATE part SET data = ?1 WHERE id = 'prt_1'",
        [json!({"type":"text","text":big()}).to_string()],
    )
    .unwrap();
    let transcript = read("opencode", &f.profile, "ses_1", &f.checkout, false).unwrap();
    assert!(transcript.shortened);
    assert_eq!(texts(&transcript), [(Role::Assistant, "World")]);
    db.execute("UPDATE part SET data = '{bad' WHERE id = 'prt_5'", [])
        .unwrap();
    assert!(matches!(
        read("opencode", &f.profile, "ses_1", &f.checkout, false),
        Err(ReadError::Corrupt(_)) | Err(ReadError::Unsupported(_))
    ));
    db.execute_batch("BEGIN EXCLUSIVE").unwrap();
    assert_eq!(
        read("opencode", &f.profile, "ses_1", &f.checkout, false).unwrap_err(),
        ReadError::Locked
    );
    db.execute_batch("ROLLBACK").unwrap();
    #[cfg(unix)]
    {
        let g = fixture("handoff-opencode-link");
        opencode_db(&g.other, &g.checkout);
        fs::create_dir_all(g.profile.join("opencode")).unwrap();
        std::os::unix::fs::symlink(
            g.other.join("opencode/opencode.db"),
            g.profile.join("opencode/opencode.db"),
        )
        .unwrap();
        assert!(matches!(
            read("opencode", &g.profile, "ses_1", &g.checkout, false),
            Err(ReadError::Unsafe(_))
        ));
    }
}

// ---- Antigravity ----

fn agy_file(profile: &Path) -> PathBuf {
    profile
        .join("brain")
        .join(ID)
        .join(".system_generated/logs/transcript.jsonl")
}

#[test]
fn antigravity_reads_visible_turns_and_refuses_a_typed_id() {
    let f = fixture("handoff-agy");
    write_jsonl(
        &agy_file(&f.profile),
        &[
            json!({"type":"USER_INPUT","content":"<USER_REQUEST>\nBuild it\n</USER_REQUEST>\n<ADDITIONAL_METADATA>x</ADDITIONAL_METADATA>"}),
            json!({"type":"PLANNER_RESPONSE","thinking":"hidden","tool_calls":[{"name":"run"}]}),
            json!({"type":"GENERIC","content":"tool output"}),
            json!({"type":"SYSTEM_MESSAGE","content":"system text"}),
            json!({"type":"PLANNER_RESPONSE","content":"Built","truncated_fields":["content"]}),
        ],
    );
    let transcript = read("agy", &f.profile, ID, &f.checkout, false).unwrap();
    assert_eq!(
        texts(&transcript),
        [(Role::User, "Build it"), (Role::Assistant, "Built")]
    );
    assert!(transcript.shortened);
    assert_eq!(
        read("agy", &f.profile, ID, &f.checkout, true).unwrap_err(),
        ReadError::Unverifiable
    );
    assert_eq!(
        read("agy", &f.other, ID, &f.checkout, false).unwrap_err(),
        ReadError::Missing
    );
    let mut text = fs::read_to_string(agy_file(&f.profile)).unwrap();
    text.insert_str(0, "nope\n");
    fs::write(agy_file(&f.profile), text).unwrap();
    assert!(matches!(
        read("agy", &f.profile, ID, &f.checkout, false),
        Err(ReadError::Corrupt(_))
    ));
    #[cfg(unix)]
    {
        let g = fixture("handoff-agy-link");
        write_jsonl(
            &agy_file(&g.other),
            &[json!({"type":"USER_INPUT","content":"x"})],
        );
        fs::create_dir_all(g.profile.join("brain")).unwrap();
        std::os::unix::fs::symlink(
            g.other.join("brain").join(ID),
            g.profile.join("brain").join(ID),
        )
        .unwrap();
        assert!(matches!(
            read("agy", &g.profile, ID, &g.checkout, false),
            Err(ReadError::Unsafe(_))
        ));
    }
}

#[test]
fn an_unknown_agent_or_malformed_id_reads_nothing() {
    let f = fixture("handoff-unknown");
    assert!(matches!(
        read("shell", &f.profile, ID, &f.checkout, false),
        Err(ReadError::Unsupported(_))
    ));
    assert!(matches!(
        read("claude", &f.profile, "../etc", &f.checkout, true),
        Err(ReadError::Unsupported(_))
    ));
}

// ---- Source and draft ----

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| word.to_string()).collect()
}

#[test]
fn the_source_profile_comes_from_the_launch_environment_or_default() {
    let folders = Folders {
        claude: Some("/d/claude".into()),
        codex: Some("/d/codex".into()),
        data: Some("/d/share".into()),
        agy: Some("/d/agy".into()),
    };
    let cases = [
        (
            "claude",
            argv(&["/usr/bin/env", "CLAUDE_CONFIG_DIR=/a/c", "/bin/claude"]),
            "/a/c",
        ),
        (
            "codex",
            argv(&["/usr/bin/env", "CODEX_HOME=/a/x=y", "/bin/codex"]),
            "/a/x=y",
        ),
        (
            "devin",
            argv(&[
                "/usr/bin/env",
                "-u",
                "WINDSURF_API_KEY",
                "XDG_DATA_HOME=/a/d",
                "/bin/devin",
            ]),
            "/a/d",
        ),
        (
            "claude",
            argv(&["/bin/claude", "CLAUDE_CONFIG_DIR=/no"]),
            "/d/claude",
        ),
        ("codex", argv(&["/bin/codex"]), "/d/codex"),
        ("devin", argv(&["/bin/devin"]), "/d/share"),
        ("opencode", argv(&["/bin/opencode"]), "/d/share"),
        ("agy", argv(&["/bin/agy"]), "/d/agy"),
    ];
    for (agent, argv, expected) in cases {
        assert_eq!(
            source_profile(agent, &argv, &folders),
            Some(PathBuf::from(expected)),
            "{agent} {argv:?}"
        );
    }
    assert_eq!(
        source_profile("shell", &argv(&["/bin/zsh"]), &folders),
        None
    );
}

fn identity(agent: &str, id: &str, source: &str) -> Result<Identity, String> {
    Ok(Identity {
        agent: agent.into(),
        id: id.into(),
        source: source.into(),
    })
}

#[test]
fn prepare_reads_the_attributed_conversation_and_offers_a_fork_for_claude() {
    let f = fixture("handoff-prepare");
    write_jsonl(&claude_file(&f.profile), &claude_records(&f.checkout));
    let argv = argv(&[
        "/usr/bin/env",
        &format!("CLAUDE_CONFIG_DIR={}", f.profile.display()),
        "/bin/claude",
    ]);
    let git = GitSnapshot {
        branch: Some("main".into()),
        head: "abc12345".into(),
        paths: vec!["src/a.rs".into()],
    };
    let prepared = prepare(
        Source {
            agent: "claude",
            argv: &argv,
            checkout: &f.checkout,
            identity: identity("claude", ID, "PID-linked session file"),
            manual_id: None,
        },
        &Folders::default(),
        Some(&git),
    );
    assert!(
        prepared.status.starts_with(&format!("Conversation {ID}")),
        "{}",
        prepared.status
    );
    assert_eq!(prepared.conversation.as_deref(), Some(ID));
    for expected in [
        "# Handoff from Claude Code",
        "Branch: main at abc12345",
        "- src/a.rs",
        "User:\nFix the bug",
        "Assistant:\nFound it",
        "## Next step",
    ] {
        assert!(
            prepared.draft.contains(expected),
            "{expected}\n{}",
            prepared.draft
        );
    }
    for hidden in [
        "secret thought",
        "tool output",
        "abandoned",
        "sidechain",
        "meta caveat",
        "/clear",
    ] {
        assert!(!prepared.draft.contains(hidden), "{hidden}");
    }
    assert_eq!(
        prepared.fork.map(|fork| fork.id),
        (!cfg!(windows)).then(|| ID.to_owned())
    );

    // A typed ID for another folder is refused, and nothing else is read.
    let prepared = prepare(
        Source {
            agent: "claude",
            argv: &argv,
            checkout: &f.other,
            identity: Err("No PID-linked ID".into()),
            manual_id: Some(ID),
        },
        &Folders::default(),
        None,
    );
    assert!(
        prepared.status.contains("different folder"),
        "{}",
        prepared.status
    );
    assert!(prepared.conversation.is_none() && prepared.fork.is_none());
    assert!(!prepared.draft.contains("Fix the bug"));
}

#[test]
fn missing_history_still_gives_a_context_only_draft() {
    let f = fixture("handoff-missing");
    for (identity, expected) in [
        (
            Err("Session is not running".to_owned()),
            "Session is not running",
        ),
        (identity("codex", "abc", "Launch ID"), "runs codex"),
    ] {
        let prepared = prepare(
            Source {
                agent: "claude",
                argv: &[],
                checkout: &f.checkout,
                identity,
                manual_id: None,
            },
            &Folders::default(),
            None,
        );
        assert!(prepared.status.starts_with("No verified conversation text"));
        assert!(prepared.status.contains(expected), "{}", prepared.status);
        assert!(prepared.draft.contains("Conversation: not available"));
        assert!(!prepared.draft.contains("## Earlier conversation"));
        assert!(
            !prepared.draft.contains("Branch:"),
            "a non-Git folder has no Git section"
        );
        assert!(prepared.draft.ends_with("## Next step\n\n"));
        assert!(prepared.accepts_manual_id);
    }
    let prepared = prepare(
        Source {
            agent: "agy",
            argv: &[],
            checkout: &f.checkout,
            identity: Err("none".into()),
            manual_id: None,
        },
        &Folders::default(),
        None,
    );
    assert!(!prepared.accepts_manual_id);
}

fn transcript(messages: &[(Role, String)]) -> Transcript {
    Transcript {
        messages: messages
            .iter()
            .map(|(role, text)| Message {
                role: *role,
                text: text.clone(),
            })
            .collect(),
        ..Default::default()
    }
}

#[test]
fn a_long_conversation_keeps_the_first_request_and_the_newest_messages() {
    let mut messages = vec![(Role::User, format!("GOAL {}", "g".repeat(5000)))];
    for turn in 0..40 {
        messages.push((
            Role::Assistant,
            format!("answer {turn} {}", "a".repeat(600)),
        ));
        messages.push((Role::User, format!("ask {turn} {}", "u".repeat(600))));
    }
    let t = transcript(&messages);
    let (text, omitted) = draft("Codex", Path::new("/work"), None, Ok(("id", "how", &t)));
    assert!(text.len() <= DRAFT_LIMIT);
    let first = text.find("User:\nGOAL").unwrap();
    let block_end = text[first..].find("\n\n").unwrap();
    assert!(
        block_end <= FIRST_LIMIT,
        "the first message is cut to 2 KiB"
    );
    assert!(text[first..].contains(CUT));
    assert!(text.contains("ask 39 "), "the newest message is kept");
    assert!(!text.contains("answer 0 "), "the oldest are dropped first");
    assert!(omitted > 0);
    assert!(text.contains(&format!("[{omitted} earlier messages omitted]")));
    let kept = text.matches("Assistant:\n").count() + text.matches("User:\n").count();
    assert_eq!(kept + omitted, messages.len());
}

#[test]
fn an_oversized_newest_message_keeps_its_end_on_a_character_boundary() {
    let t = transcript(&[
        (Role::User, "Start".into()),
        (Role::Assistant, format!("{}END", "é".repeat(20_000))),
    ]);
    let (text, omitted) = draft("Devin", Path::new("/work"), None, Ok(("id", "how", &t)));
    assert!(text.len() <= DRAFT_LIMIT);
    assert!(text.contains("User:\nStart"));
    assert!(text.contains(&format!("Assistant:\n{CUT}\né")));
    assert!(text.contains("END\n\n## Next step"));
    assert_eq!(omitted, 0);
}

#[test]
fn the_draft_strips_terminal_control_and_bounds_the_path_list() {
    let t = transcript(&[(
        Role::User,
        "red \u{1b}[31mtext\u{1b}[0m bell\u{7} title\u{1b}]0;x\u{7} ok\r\n".into(),
    )]);
    let git = GitSnapshot {
        branch: None,
        head: "1234abcd".into(),
        paths: (0..51).map(|i| format!("file{i}.rs")).collect(),
    };
    let (text, _) = draft(
        "OpenCode",
        Path::new("/work"),
        Some(&git),
        Ok(("id", "how", &t)),
    );
    assert!(text.contains("User:\nred text bell title ok"), "{text}");
    assert!(!text.chars().any(|c| c.is_control() && c != '\n'));
    assert!(text.contains("Branch: detached at 1234abcd"));
    assert!(text.contains("Changed paths (51):"));
    assert!(text.contains("- file49.rs") && !text.contains("- file50.rs"));
    assert!(text.contains("- … 1 more"));
}

// ---- Fork ----

#[cfg(unix)]
#[test]
fn a_fork_needs_the_same_projects_folder_and_resumes_by_id() {
    let f = fixture("handoff-fork");
    let file = claude_file(&f.profile);
    write_jsonl(&file, &claude_records(&f.checkout));
    let source = ForkSource {
        id: ID.into(),
        file: file.clone(),
    };
    let shared = f.other.join("shared");
    fs::create_dir_all(&shared).unwrap();
    std::os::unix::fs::symlink(f.profile.join("projects"), shared.join("projects")).unwrap();
    let words = validate_fork(&source, Some(&shared), &Folders::default()).unwrap();
    assert_eq!(words, ["--resume", ID, "--fork-session"]);
    assert!(!words.iter().any(|word| word.ends_with(".jsonl")));
    let default = Folders {
        claude: Some(f.profile.clone()),
        ..Default::default()
    };
    assert!(validate_fork(&source, None, &default).is_ok());

    let separate = f.other.join("separate");
    fs::create_dir_all(separate.join("projects")).unwrap();
    assert!(validate_fork(&source, Some(&separate), &Folders::default())
        .unwrap_err()
        .contains("does not share"));
    assert!(validate_fork(&source, None, &Folders::default()).is_err());
    fs::remove_file(&file).unwrap();
    assert!(validate_fork(&source, Some(&shared), &Folders::default()).is_err());
}
