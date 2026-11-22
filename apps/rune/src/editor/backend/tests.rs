use super::*;
use crate::editor::draft::OpenDoc;
use crate::editor::state::{EditorState, checkout_key};
use app_core::git_view::GitViewFile;
use app_core::projects::ProjectView;
use app_core::store::Project;

fn editor() -> EditorState {
    EditorState::new(rune_kit::Kit::neutral(true), crate::test_handle())
}

/// Registered project + canonical checkout key, as `select_checkout`
/// would leave them.
fn select(e: &mut EditorState) {
    e.project_id = Some("p1".into());
    e.checkout = Some(PathBuf::from("/tmp/co"));
    e.checkout_key = Some(checkout_key("p1", &PathBuf::from("/tmp/co")));
}

fn text_file(text: &str) -> GitViewFile {
    GitViewFile {
        path: "a.txt".into(),
        kind: "text",
        text: Some(text.into()),
        truncated: false,
        size: text.len() as u64,
    }
}

fn reply(
    req: Req,
    generation: u64,
    key: Option<String>,
    result: Result<ReplyData, String>,
) -> Reply {
    Reply {
        req,
        generation,
        checkout_key: key,
        result,
    }
}

#[test]
fn vim_switch_mirrors_into_the_active_and_scratch_surfaces() {
    let mut e = editor();
    e.doc = Some(OpenDoc::new("a.txt", &text_file("one\ntwo\n")));
    e.apply_vim_enabled(true);
    assert!(e.vim_enabled);
    assert!(e.doc.as_ref().unwrap().code.vim.enabled);
    assert!(e.scratch_code.vim.enabled);
    e.doc.as_mut().unwrap().code.vim.mode = crate::editor::vim::Mode::Insert;
    e.apply_vim_enabled(false);
    assert!(!e.vim_enabled);
    assert_eq!(
        e.doc.as_ref().unwrap().code.vim.mode,
        crate::editor::vim::Mode::Normal,
        "turning the setting off returns the document to Normal"
    );
    assert!(!e.scratch_code.vim.enabled);
}

#[test]
fn the_vim_setting_reply_accepts_only_one_as_on() {
    let mut e = editor();
    e.doc = Some(OpenDoc::new("a.txt", &text_file("one\n")));
    let key = crate::editor::vim::VIM_KEY.to_string();
    e.apply_reply(reply(
        Req::GetSetting { key: key.clone() },
        0,
        None,
        Ok(ReplyData::Setting(Some("1".into()))),
    ));
    assert!(e.vim_enabled);
    assert!(e.doc.as_ref().unwrap().code.vim.enabled);
    for value in [Some("0".to_string()), Some("yes".to_string()), None] {
        e.apply_vim_enabled(true);
        e.apply_reply(reply(
            Req::GetSetting { key: key.clone() },
            0,
            None,
            Ok(ReplyData::Setting(value.clone())),
        ));
        assert!(!e.vim_enabled, "{value:?} must read as off");
        assert!(!e.doc.as_ref().unwrap().code.vim.enabled);
    }
}

#[test]
fn stage_resolved_checks_disk_and_markers_and_only_stages_one_path() {
    use app_core::test_support::TempDir;
    use std::fs;
    use std::process::Command;
    let temp = TempDir::new("resolve");
    let root = temp.path().canonicalize().unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{:?}: {:?}", args, out);
        String::from_utf8(out.stdout).unwrap()
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.name", "Tester"]);
    git(&["config", "user.email", "test@example.com"]);
    fs::write(root.join("a.txt"), "base\n").unwrap();
    git(&["add", "a.txt"]);
    git(&["commit", "-qm", "base"]);
    git(&["checkout", "-qb", "side"]);
    fs::write(root.join("a.txt"), "theirs\n").unwrap();
    git(&["commit", "-qam", "side"]);
    git(&["checkout", "-q", "main"]);
    fs::write(root.join("a.txt"), "ours\n").unwrap();
    git(&["commit", "-qam", "ours"]);
    assert!(
        !Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["merge", "side"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let marked = fs::read_to_string(root.join("a.txt")).unwrap();
    assert!(
        stage_resolved(&root, "a.txt", &marked, false)
            .unwrap_err()
            .contains("markers")
    );
    assert!(
        stage_resolved(&root, "a.txt", "resolved\n", false)
            .unwrap_err()
            .contains("changed on disk")
    );
    assert!(!git(&["ls-files", "-u"]).is_empty());
    fs::write(root.join("a.txt"), "ours\ntheirs\n").unwrap();
    fs::write(root.join("other.txt"), "leave unstaged\n").unwrap();
    let head = git(&["rev-parse", "HEAD"]);
    stage_resolved(&root, "a.txt", "ours\ntheirs\n", false).unwrap();
    assert!(git(&["ls-files", "-u"]).is_empty());
    assert_eq!(git(&["diff", "--cached", "--name-only"]), "a.txt\n");
    assert_eq!(git(&["rev-parse", "HEAD"]), head);
    assert!(stage_resolved(&root, "../outside", "", false).is_err());
}

#[test]
fn stage_resolved_with_markers_needs_allow_markers() {
    use app_core::test_support::TempDir;
    use std::fs;
    use std::process::Command;
    let temp = TempDir::new("resolve-markers");
    let root = temp.path().canonicalize().unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{:?}: {:?}", args, out);
        String::from_utf8(out.stdout).unwrap()
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.name", "Tester"]);
    git(&["config", "user.email", "test@example.com"]);
    fs::write(root.join("a.txt"), "base\n").unwrap();
    git(&["add", "a.txt"]);
    git(&["commit", "-qm", "base"]);
    git(&["checkout", "-qb", "side"]);
    fs::write(root.join("a.txt"), "theirs\n").unwrap();
    git(&["commit", "-qam", "side"]);
    git(&["checkout", "-q", "main"]);
    fs::write(root.join("a.txt"), "ours\n").unwrap();
    git(&["commit", "-qam", "ours"]);
    assert!(
        !Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["merge", "side"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let marked = fs::read_to_string(root.join("a.txt")).unwrap();
    assert!(crate::editor::conflict::has_markers(&marked));
    // Markers are refused until the request carries the dialog's
    // confirmation; a disk drift is refused either way.
    assert!(
        stage_resolved(&root, "a.txt", &marked, false)
            .unwrap_err()
            .contains("markers")
    );
    assert!(
        stage_resolved(&root, "a.txt", "resolved\n", true)
            .unwrap_err()
            .contains("changed on disk")
    );
    assert!(
        stage_resolved(&root, "a.txt", "resolved\n", false)
            .unwrap_err()
            .contains("changed on disk")
    );
    stage_resolved(&root, "a.txt", &marked, true).unwrap();
    assert!(git(&["ls-files", "-u"]).is_empty());
}

#[test]
fn resolve_after_save_only_runs_after_success_and_keeps_saved_baseline_on_stage_error() {
    let (backend, jobs) = channel_backend();
    let mut e = editor();
    e.backend = backend;
    select(&mut e);
    let mut doc = OpenDoc::new("a.txt", &text_file("original"));
    doc.code.edit.set_selection(0, doc.code.edit.text.len());
    doc.code.edit.insert("resolved");
    doc.saving = true;
    doc.resolve_after_save = true;
    e.doc = Some(doc);
    let req = Req::SaveFile {
        path: "a.txt".into(),
        expected: "original".into(),
        text: "resolved".into(),
    };
    e.apply_reply(reply(
        req.clone(),
        0,
        e.checkout_key.clone(),
        Err("disk changed".into()),
    ));
    assert!(!e.doc.as_ref().unwrap().resolve_after_save);
    assert!(jobs.try_recv().is_err());
    e.doc.as_mut().unwrap().resolve_after_save = true;
    e.apply_reply(reply(
        req,
        0,
        e.checkout_key.clone(),
        Ok(ReplyData::Saved(text_file("resolved"))),
    ));
    let queued: Vec<_> = jobs.try_iter().collect();
    assert!(queued.iter().any(|j| matches!(&j.req, Req::StageResolved { path, expected, allow_markers: false } if path == "a.txt" && expected == "resolved")));
    e.apply_reply(reply(
        Req::StageResolved {
            path: "a.txt".into(),
            expected: "resolved".into(),
            allow_markers: false,
        },
        0,
        e.checkout_key.clone(),
        Err("index locked".into()),
    ));
    assert_eq!(e.doc.as_ref().unwrap().original, "resolved");
    assert!(!e.doc.as_ref().unwrap().dirty());
    assert_eq!(e.changes.error.as_deref(), Some("index locked"));
    assert!(e.changes.busy.is_none());
}

#[test]
fn current_branch_replies_obey_checkout_and_generation_guards() {
    let mut e = editor();
    select(&mut e);
    let key = e.checkout_key.clone();
    let data = crate::editor::branch::Branch {
        name: Some("feature/current".into()),
        head: "12345678".into(),
    };
    e.req_gens.insert("currentbranch", 2);
    e.apply_reply(reply(
        Req::CurrentBranch,
        1,
        key.clone(),
        Ok(ReplyData::CurrentBranch(data.clone())),
    ));
    assert!(e.branch.data.is_none());
    e.apply_reply(reply(
        Req::CurrentBranch,
        2,
        Some("other-checkout".into()),
        Ok(ReplyData::CurrentBranch(data.clone())),
    ));
    assert!(e.branch.data.is_none());
    e.branch.pending = true;
    e.apply_reply(reply(
        Req::CurrentBranch,
        2,
        key.clone(),
        Ok(ReplyData::CurrentBranch(data.clone())),
    ));
    assert_eq!(e.branch.data, Some(data));
    assert_eq!(e.branch.checkout, key);
    assert!(!e.branch.pending);
    e.req_gens.insert("currentbranch", 3);
    e.apply_reply(reply(
        Req::CurrentBranch,
        3,
        key,
        Err("Git unavailable".into()),
    ));
    assert_eq!(e.branch.error.as_deref(), Some("Git unavailable"));
    assert!(!e.branch.pending);
}

#[test]
fn stale_generation_reply_is_dropped() {
    let mut e = editor();
    select(&mut e);
    let checkout = e.checkout.clone().unwrap();
    let key = e.checkout_key.clone().unwrap();
    // Two reads of the same kind: the second supersedes the first.
    assert!(e.enqueue_read(checkout.clone(), key.clone(), Req::Changes));
    assert!(e.enqueue_read(checkout, key.clone(), Req::Changes));
    // The superseded generation-1 reply must not touch state.
    e.apply_reply(reply(Req::Changes, 1, Some(key.clone()), Err("old".into())));
    assert!(e.changes.error.is_none());
    // The current generation applies.
    e.apply_reply(reply(Req::Changes, 2, Some(key), Err("new".into())));
    assert_eq!(e.changes.error.as_deref(), Some("new"));
}

#[test]
fn foreign_checkout_reply_is_dropped() {
    let mut e = editor();
    select(&mut e);
    let checkout = e.checkout.clone().unwrap();
    let key = e.checkout_key.clone().unwrap();
    assert!(e.enqueue_read(checkout, key.clone(), Req::Changes));
    e.apply_reply(reply(
        Req::Changes,
        1,
        Some("p9\u{1f}/other".into()),
        Err("foreign".into()),
    ));
    assert!(e.changes.error.is_none());
}

#[test]
fn stage_unstage_discard_success_clears_busy() {
    let mut e = editor();
    select(&mut e);
    let key = e.checkout_key.clone().unwrap();
    // Each write kind: busy arms, the Ok reply must clear it (and any
    // obsolete error) so the next action is permitted.
    for (req, kind) in [
        (Req::Stage { paths: None }, "stage"),
        (Req::Unstage { paths: None }, "unstage"),
        (
            Req::Discard {
                layer: "working".into(),
                path: "a.txt".into(),
            },
            "discard",
        ),
    ] {
        e.changes.busy = Some(kind);
        e.changes.error = Some("obsolete".into());
        e.apply_reply(reply(req, 0, Some(key.clone()), Ok(ReplyData::Ok)));
        assert_eq!(e.changes.busy, None, "{kind} success clears busy");
        assert_eq!(e.changes.error, None, "{kind} success clears stale error");
        // The gate is open: a subsequent write re-arms busy normally.
        crate::editor::changes::stage_paths(&mut e, None);
        assert_eq!(e.changes.busy, Some("stage"));
        e.changes.busy = None;
    }
}

#[test]
fn stage_error_clears_busy_and_keeps_error() {
    let mut e = editor();
    select(&mut e);
    let key = e.checkout_key.clone().unwrap();
    e.changes.busy = Some("stage");
    e.apply_reply(reply(
        Req::Stage { paths: None },
        0,
        Some(key),
        Err("denied".into()),
    ));
    assert_eq!(e.changes.busy, None, "error arm already clears busy");
    assert_eq!(e.changes.error.as_deref(), Some("denied"));
    assert!(e.notice.is_some());
}

#[test]
fn operation_reply_refreshes_changes_and_clears_busy() {
    let (backend, jobs) = channel_backend();
    let mut e = editor();
    e.backend = backend;
    select(&mut e);
    e.changes.busy = Some("resolve");
    e.apply_reply(reply(
        Req::Operation {
            step: "abort".into(),
        },
        0,
        e.checkout_key.clone(),
        Ok(ReplyData::Ok),
    ));
    assert_eq!(e.changes.busy, None);
    assert_eq!(e.changes.error, None);
    // Same write follow-up as Stage: Changes is re-read.
    let queued: Vec<_> = jobs.try_iter().collect();
    assert!(queued.iter().any(|j| matches!(j.req, Req::Changes)));
}

#[test]
fn foreign_checkout_write_reply_preserves_busy() {
    let mut e = editor();
    select(&mut e);
    e.changes.busy = Some("stage");
    // A Stage reply scoped to a different checkout drops before the
    // match — it must not clear the current checkout's busy flag.
    e.apply_reply(reply(
        Req::Stage { paths: None },
        0,
        Some("p9\u{1f}/other".into()),
        Ok(ReplyData::Ok),
    ));
    assert_eq!(e.changes.busy, Some("stage"));
}

#[test]
fn write_reply_always_applies() {
    let mut e = editor();
    select(&mut e);
    let key = e.checkout_key.clone().unwrap();
    e.doc = Some(OpenDoc::new("a.txt", &text_file("hi")));
    e.doc.as_mut().unwrap().saving = true;
    // Generation 0 marks a write: even though reads are gated, the save
    // reply must land so `saving` can never stick.
    e.apply_reply(reply(
        Req::SaveFile {
            path: "a.txt".into(),
            expected: "hi".into(),
            text: "hi".into(),
        },
        0,
        Some(key),
        Ok(ReplyData::Saved(text_file("hi"))),
    ));
    let doc = e.doc.as_ref().unwrap();
    assert!(!doc.saving);
    assert!(doc.save_error.is_none());
}

#[test]
fn enqueue_requires_registered_project() {
    let mut e = editor();
    // No project selected: reads and writes are refused before a job
    // can reach the worker with an arbitrary path.
    assert!(!e.enqueue_read(PathBuf::from("/tmp/co"), "k".into(), Req::Changes));
    assert!(!e.enqueue_write(
        PathBuf::from("/tmp/co"),
        "k".into(),
        Req::Commit {
            message: "m".into()
        }
    ));
}

#[test]
fn add_project_reply_never_switches_selection() {
    let mut e = editor();
    select(&mut e);
    let key = e.checkout_key.clone().unwrap();
    let added = ProjectView {
        project: Project {
            id: "p2".into(),
            name: "P2".into(),
            root: "/tmp/p2".into(),
            workspace_id: app_core::store::DEFAULT_WORKSPACE_ID.into(),
            created_at: 0,
        },
        missing: false,
        parent_project_id: None,
    };
    e.apply_reply(reply(
        Req::AddProject {
            path: "/tmp/p2".into(),
            workspace_id: app_core::store::DEFAULT_WORKSPACE_ID.into(),
        },
        0,
        None,
        Ok(ReplyData::Added(added)),
    ));
    // The project list grew but the active identity is untouched — an
    // added project must never auto-switch away from a live checkout.
    assert_eq!(e.project_id.as_deref(), Some("p1"));
    assert_eq!(e.checkout_key.as_deref(), Some(key.as_str()));
    assert!(e.projects.iter().any(|p| p.project.id == "p2"));
}

#[test]
fn same_file_reply_reveals_without_replacing_draft() {
    let mut e = editor();
    select(&mut e);
    let checkout = e.checkout.clone().unwrap();
    let key = e.checkout_key.clone().unwrap();
    e.doc = Some(OpenDoc::new("a.txt", &text_file("orig")));
    e.doc.as_mut().unwrap().code.edit.text = "dirty edit".into();
    e.doc_load = Some((9, "a.txt".into()));
    assert!(e.enqueue_read(
        checkout,
        key.clone(),
        Req::File {
            path: "a.txt".into(),
            token: 9,
            line: Some(7)
        }
    ));
    e.apply_reply(reply(
        Req::File {
            path: "a.txt".into(),
            token: 9,
            line: Some(7),
        },
        1,
        Some(key),
        Ok(ReplyData::File(text_file("orig"))),
    ));
    let doc = e.doc.as_ref().unwrap();
    // Same-path replies only queue the reveal — the dirty buffer and
    // its original are both preserved (external open-file contract).
    assert_eq!(doc.reveal_line, Some(7));
    assert_eq!(doc.code.edit.text, "dirty edit");
    assert_eq!(doc.original, "orig");
    assert_eq!(e.section, crate::editor::state::Section::Files);
}

#[test]
fn stale_file_token_reply_is_ignored() {
    let mut e = editor();
    select(&mut e);
    let checkout = e.checkout.clone().unwrap();
    let key = e.checkout_key.clone().unwrap();
    e.doc_load = Some((9, "a.txt".into()));
    assert!(e.enqueue_read(
        checkout,
        key.clone(),
        Req::File {
            path: "a.txt".into(),
            token: 9,
            line: None
        }
    ));
    // A reply for a superseded load token must not install a doc.
    e.apply_reply(reply(
        Req::File {
            path: "a.txt".into(),
            token: 4,
            line: None,
        },
        1,
        Some(key),
        Ok(ReplyData::File(text_file("orig"))),
    ));
    assert!(e.doc.is_none());
}

// -- go-to config lookup -------------------------------------------------

fn config_json(pattern: &str, target: &str) -> String {
    format!("{{\"compilerOptions\":{{\"paths\":{{\"{pattern}\":[\"{target}\"]}}}}}}")
}

fn config_aliases_of(data: ReplyData) -> Vec<crate::editor::go_to_definition::PathAlias> {
    let ReplyData::ConfigAliases(aliases) = data else {
        panic!("expected ConfigAliases, got {data:?}")
    };
    aliases
}

#[test]
fn config_aliases_skips_unreadable_and_aliasless_configs() {
    use app_core::test_support::TempDir;
    let dir = TempDir::new("goto-config");
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("src/deep")).unwrap();
    // Nearest candidates: missing file, malformed JSON, and a readable
    // config without `paths` — the desktop skips all three because only
    // a non-empty alias list ends its scan.
    std::fs::write(root.join("src/deep/tsconfig.json"), "{ not json").unwrap();
    std::fs::write(root.join("src/tsconfig.json"), "{\"compilerOptions\":{}}").unwrap();
    std::fs::write(
        root.join("tsconfig.json"),
        config_json("@app/*", "src/app/*"),
    )
    .unwrap();
    let paths: Vec<String> = [
        "src/deep/tsconfig.json",
        "src/deep/jsconfig.json",
        "src/tsconfig.json",
        "src/jsconfig.json",
        "tsconfig.json",
        "jsconfig.json",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let aliases = config_aliases_of(config_aliases(&root, &paths));
    assert_eq!(aliases.len(), 1);
    assert_eq!(aliases[0].pattern, "@app/*");
    assert_eq!(aliases[0].targets, vec!["src/app/*".to_string()]);
}

#[test]
fn config_aliases_jsconfig_fallback_and_nearest_wins() {
    use app_core::test_support::TempDir;
    // jsconfig.json answers when tsconfig is absent.
    let dir = TempDir::new("goto-jsconfig");
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(
        dir.path().join("jsconfig.json"),
        config_json("@lib/*", "lib/*"),
    )
    .unwrap();
    let aliases = config_aliases_of(config_aliases(
        &root,
        &["tsconfig.json".into(), "jsconfig.json".into()],
    ));
    assert_eq!(aliases[0].pattern, "@lib/*");
    // The nearest config WITH aliases shadows every ancestor.
    let dir = TempDir::new("goto-nearest");
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(
        root.join("sub/tsconfig.json"),
        config_json("@sub/*", "sub/*"),
    )
    .unwrap();
    std::fs::write(
        root.join("tsconfig.json"),
        config_json("@app/*", "src/app/*"),
    )
    .unwrap();
    let aliases = config_aliases_of(config_aliases(
        &root,
        &["sub/tsconfig.json".into(), "tsconfig.json".into()],
    ));
    assert_eq!(aliases[0].pattern, "@sub/*");
}

#[test]
fn config_aliases_empty_when_no_config_defines_paths() {
    use app_core::test_support::TempDir;
    let dir = TempDir::new("goto-noconfig");
    let aliases = config_aliases(
        &dir.path().canonicalize().unwrap(),
        &["tsconfig.json".into(), "jsconfig.json".into()],
    );
    assert!(config_aliases_of(aliases).is_empty());
}

#[test]
fn config_aliases_walks_past_the_old_hidden_cap() {
    use app_core::test_support::TempDir;
    // 25 candidates — far beyond the removed `.take(8)` UI cutoff; the
    // worker walks the whole ordered list like the desktop.
    let dir = TempDir::new("goto-deep");
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("tsconfig.json"), config_json("@deep/*", "deep/*")).unwrap();
    let mut paths: Vec<String> = (0..12)
        .flat_map(|i| [format!("d{i}/tsconfig.json"), format!("d{i}/jsconfig.json")])
        .collect();
    paths.push("tsconfig.json".into());
    paths.push("jsconfig.json".into());
    assert_eq!(paths.len(), 26);
    let aliases = config_aliases_of(config_aliases(&root, &paths));
    assert_eq!(aliases.len(), 1);
    assert_eq!(aliases[0].pattern, "@deep/*");
}

fn pending_import() -> crate::editor::state::GoToPending {
    crate::editor::state::GoToPending {
        from_path: "src/a.ts".into(),
        stage: crate::editor::state::GoToStage::AwaitConfigs {
            specifier: "@app/util".into(),
        },
    }
}

fn alias() -> crate::editor::go_to_definition::PathAlias {
    crate::editor::go_to_definition::PathAlias {
        pattern: "@app/*".into(),
        targets: vec!["src/app/*".into()],
    }
}

fn config_reply(
    generation: u64,
    key: Option<String>,
    aliases: Vec<crate::editor::go_to_definition::PathAlias>,
) -> Reply {
    reply(
        Req::ConfigFiles {
            paths: vec!["tsconfig.json".into()],
        },
        generation,
        key,
        Ok(ReplyData::ConfigAliases(aliases)),
    )
}

#[test]
fn config_files_reply_resolves_pending_import() {
    let mut e = editor();
    select(&mut e);
    e.section = Section::Files; // go-to lookups live on the Files surface
    let checkout = e.checkout.clone().unwrap();
    let key = e.checkout_key.clone().unwrap();
    e.files.paths = vec!["src/a.ts".into(), "src/app/util.ts".into()];
    e.doc = Some(OpenDoc::new("src/a.ts", &text_file("x")));
    e.go_to = Some(pending_import());
    assert!(e.enqueue_read(
        checkout,
        key.clone(),
        Req::ConfigFiles {
            paths: vec!["tsconfig.json".into()]
        }
    ));
    e.apply_reply(config_reply(1, Some(key), vec![alias()]));
    assert!(e.go_to.is_none(), "lookup consumed by its reply");
    assert!(
        matches!(
            e.pending_apply,
            Some(crate::editor::navigation::PendingNav::OpenDoc { ref path, .. })
                if path == "src/app/util.ts"
        ),
        "alias resolved to the target: {:?}",
        e.pending_apply
    );
}

#[test]
fn config_files_stale_generation_and_foreign_checkout_drop() {
    let mut e = editor();
    select(&mut e);
    e.section = Section::Files;
    let checkout = e.checkout.clone().unwrap();
    let key = e.checkout_key.clone().unwrap();
    e.files.paths = vec!["src/app/util.ts".into()];
    e.doc = Some(OpenDoc::new("src/a.ts", &text_file("x")));
    e.go_to = Some(pending_import());
    // Two lookups: the second supersedes the first generation.
    e.enqueue_read(
        checkout.clone(),
        key.clone(),
        Req::ConfigFiles {
            paths: vec!["tsconfig.json".into()],
        },
    );
    e.enqueue_read(
        checkout,
        key.clone(),
        Req::ConfigFiles {
            paths: vec!["tsconfig.json".into()],
        },
    );
    // The superseded gen-1 reply drops: pending survives, nothing opens.
    e.apply_reply(config_reply(1, Some(key.clone()), vec![alias()]));
    assert!(e.go_to.is_some());
    assert!(e.pending_apply.is_none());
    // A foreign-checkout reply drops the same way.
    e.apply_reply(config_reply(
        2,
        Some("p9\u{1f}/other".into()),
        vec![alias()],
    ));
    assert!(e.go_to.is_some());
    assert!(e.pending_apply.is_none());
    // The current generation resolves.
    e.apply_reply(config_reply(2, Some(key), vec![alias()]));
    assert!(e.go_to.is_none());
    assert!(e.pending_apply.is_some());
}

// -- external open + refresh refusal ------------------------------------

fn external_open_reply(project_id: &str, checkout: &str, path: &str, line: Option<u64>) -> Reply {
    reply(
        Req::ExternalOpen {
            project_id: project_id.into(),
            checkout_path: checkout.into(),
            path: path.into(),
            line,
        },
        0,
        None,
        Ok(ReplyData::ExternalOpen {
            checkout: PathBuf::from(checkout),
            path: path.into(),
            line,
        }),
    )
}

#[test]
fn external_open_queues_one_typed_pending_and_refuses_overwrite() {
    let mut e = editor();
    select(&mut e);
    e.apply_reply(external_open_reply("p1", "/tmp/co", "src/a.ts", Some(3)));
    assert!(
        matches!(
            e.pending_apply,
            Some(crate::editor::navigation::PendingNav::ExternalOpen {
                ref project_id,
                ref path,
                ..
            }) if project_id == "p1" && path == "src/a.ts"
        ),
        "external open queued with its identity: {:?}",
        e.pending_apply
    );
    // A second request before poll is refused loudly — the first
    // pending is never overwritten.
    e.apply_reply(external_open_reply("p1", "/tmp/co", "src/b.ts", None));
    assert!(
        matches!(
            e.pending_apply,
            Some(crate::editor::navigation::PendingNav::ExternalOpen {
                ref path, ..
            }) if path == "src/a.ts"
        ),
        "first request still pending: {:?}",
        e.pending_apply
    );
    assert!(
        e.notice
            .as_ref()
            .is_some_and(|n| n.text.contains("external open")),
        "refusal surfaced: {:?}",
        e.notice
    );
}

#[test]
fn external_open_refused_while_navigation_or_save_is_pending() {
    use crate::editor::navigation::{Guard, PendingNav};
    // Occupied pending_apply slot.
    let mut e = editor();
    select(&mut e);
    e.pending_apply = Some(PendingNav::OpenDoc {
        path: "x.ts".into(),
        line: None,
    });
    e.apply_reply(external_open_reply("p1", "/tmp/co", "a.ts", None));
    assert!(matches!(e.pending_apply, Some(PendingNav::OpenDoc { .. })));
    assert!(e.notice.is_some());
    // Occupied save-resume slot.
    let mut e = editor();
    select(&mut e);
    e.save_resume = Some(Guard {
        pending: PendingNav::OpenDoc {
            path: "y.ts".into(),
            line: None,
        },
        doc_path: "a.txt".into(),
        queue: Vec::new(),
        quitting: false,
    });
    e.apply_reply(external_open_reply("p1", "/tmp/co", "a.ts", None));
    assert!(e.pending_apply.is_none());
    assert!(e.save_resume.is_some(), "save resume untouched");
    assert!(e.notice.is_some());
    // Occupied draft guard.
    let mut e = editor();
    select(&mut e);
    e.guard = Some(Guard {
        pending: PendingNav::OpenDoc {
            path: "z.ts".into(),
            line: None,
        },
        doc_path: "a.txt".into(),
        queue: Vec::new(),
        quitting: false,
    });
    e.apply_reply(external_open_reply("p1", "/tmp/co", "a.ts", None));
    assert!(e.pending_apply.is_none());
    assert!(e.guard.is_some(), "guard untouched");
    assert!(e.notice.is_some());
}

#[test]
fn refresh_active_refusal_unwinds_every_section_flag() {
    let mut e = editor();
    select(&mut e);
    // `project_id = None` makes every `enqueue_read` refuse, like a
    // full bounded queue.
    e.project_id = None;
    for section in [
        Section::Changes,
        Section::Files,
        Section::History,
        Section::Stashes,
    ] {
        e.section = section;
        e.refresh_active();
        let (loading, error) = match section {
            Section::Changes => (e.changes.loading, e.changes.error.is_some()),
            Section::Files => (e.files.loading, e.files.error.is_some()),
            Section::History => (e.history.loading, e.history.error.is_some()),
            Section::Stashes => (e.stashes.loading, e.stashes.error.is_some()),
            // Not in the iterated set; arm exists for exhaustiveness.
            Section::Servers => (false, false),
        };
        assert!(!loading, "{section:?} stuck loading after refusal");
        assert!(error, "{section:?} missing queue-full error");
    }
}

#[test]
fn refresh_active_patch_refusal_unwinds_patch_loading() {
    let mut e = editor();
    select(&mut e);
    e.project_id = None;
    e.section = Section::Changes;
    e.changes.layer = Some("worktree".into());
    e.changes.patch_path = Some("a.txt".into());
    e.refresh_active();
    assert!(!e.changes.loading);
    assert!(!e.changes.patch_loading);
    assert!(e.changes.error.is_some());
}

#[test]
fn refresh_active_refusal_with_cached_data_uses_stale_notice() {
    let mut e = editor();
    select(&mut e);
    e.project_id = None;
    e.section = Section::Files;
    // Cached paths + a stale model key force the FileList read; the
    // refusal must not paint an error over data that is still shown.
    e.files.paths = vec!["a.txt".into()];
    e.files.model_checkout = Some("other".into());
    e.refresh_active();
    assert!(!e.files.loading);
    assert!(e.files.error.is_none());
    assert!(
        e.notice.as_ref().is_some_and(|n| n.stale),
        "cached-data refusal surfaces a stale notice: {:?}",
        e.notice
    );
}

// -- Req::ReloadFile ----------------------------------------------------

/// Issue a reload exactly like `navigation::request_doc_reload`, minus
/// the `State` wrapper: bump `doc_seq`, enqueue under the live checkout,
/// arm `doc_reload`. Returns `(token, revision)` for the reply.
fn issue_reload(e: &mut EditorState) -> (u64, u64) {
    let doc = e.doc.as_ref().expect("open doc");
    let path = doc.path.clone();
    let revision = doc.code.revision;
    e.doc_seq += 1;
    let token = e.doc_seq;
    let checkout = e.checkout.clone().unwrap();
    let key = e.checkout_key.clone().unwrap();
    assert!(e.enqueue_read(
        checkout,
        key,
        Req::ReloadFile {
            path,
            token,
            revision,
        }
    ));
    e.doc_reload = Some(token);
    (token, revision)
}

fn reload_reply(
    e: &EditorState,
    token: u64,
    revision: u64,
    result: Result<ReplyData, String>,
) -> Reply {
    Reply {
        req: Req::ReloadFile {
            path: e.doc.as_ref().unwrap().path.clone(),
            token,
            revision,
        },
        generation: e.req_gens["reloadfile"],
        checkout_key: e.checkout_key.clone(),
        result,
    }
}

#[test]
fn reload_file_replaces_doc_on_clean_match() {
    let mut e = editor();
    select(&mut e);
    e.doc = Some(OpenDoc::new("a.txt", &text_file("orig")));
    e.doc.as_mut().unwrap().code.edit.text = "edited".into();
    let (token, revision) = issue_reload(&mut e);
    e.apply_reply(reload_reply(
        &e,
        token,
        revision,
        Ok(ReplyData::File(text_file("disk"))),
    ));
    let doc = e.doc.as_ref().unwrap();
    // Baseline AND buffer now come from disk — the desktop's
    // reloadDraft clears the draft and reopens.
    assert_eq!(doc.original, "disk");
    assert_eq!(doc.code.edit.text, "disk");
    assert!(!doc.dirty());
    assert_eq!(e.doc_reload, None);
}

#[test]
fn reload_file_stale_token_is_dropped() {
    let mut e = editor();
    select(&mut e);
    e.doc = Some(OpenDoc::new("a.txt", &text_file("orig")));
    e.doc.as_mut().unwrap().code.edit.text = "edited".into();
    let (token, revision) = issue_reload(&mut e);
    // A second reload supersedes the first.
    let (token2, _) = issue_reload(&mut e);
    assert_ne!(token, token2);
    e.apply_reply(reload_reply(
        &e,
        token,
        revision,
        Ok(ReplyData::File(text_file("disk"))),
    ));
    // First reply dropped: buffer still holds the user's edits.
    assert_eq!(e.doc.as_ref().unwrap().code.edit.text, "edited");
    assert_eq!(e.doc_reload, Some(token2));
}

#[test]
fn reload_file_edit_during_flight_preserves_buffer() {
    let mut e = editor();
    select(&mut e);
    e.doc = Some(OpenDoc::new("a.txt", &text_file("orig")));
    let (token, revision) = issue_reload(&mut e);
    // The user typed while the read was in flight → revision moved on.
    let doc = e.doc.as_mut().unwrap();
    doc.code.edit.text = "edited".into();
    doc.code.text_edited();
    assert_ne!(doc.code.revision, revision);
    e.apply_reply(reload_reply(
        &e,
        token,
        revision,
        Ok(ReplyData::File(text_file("disk"))),
    ));
    assert_eq!(e.doc.as_ref().unwrap().code.edit.text, "edited");
    assert_eq!(e.doc_reload, None);
}

#[test]
fn reload_file_doc_switch_preserves_new_doc() {
    let mut e = editor();
    select(&mut e);
    e.doc = Some(OpenDoc::new("a.txt", &text_file("orig")));
    let (token, revision) = issue_reload(&mut e);
    // A different doc was installed mid-flight (install_doc clears
    // doc_reload, so the reply must not land on it).
    e.doc = Some(OpenDoc::new("b.txt", &text_file("other")));
    e.doc_reload = None;
    let mut reply = reload_reply(&e, token, revision, Ok(ReplyData::File(text_file("disk"))));
    reply.req = Req::ReloadFile {
        path: "a.txt".into(),
        token,
        revision,
    };
    e.apply_reply(reply);
    assert_eq!(e.doc.as_ref().unwrap().path, "b.txt");
    assert_eq!(e.doc.as_ref().unwrap().code.edit.text, "other");
}

#[test]
fn reload_file_error_preserves_buffer_and_surfaces_notice() {
    let mut e = editor();
    select(&mut e);
    e.doc = Some(OpenDoc::new("a.txt", &text_file("orig")));
    e.doc.as_mut().unwrap().code.edit.text = "edited".into();
    let (token, revision) = issue_reload(&mut e);
    e.apply_reply(reload_reply(
        &e,
        token,
        revision,
        Err("permission denied".into()),
    ));
    let doc = e.doc.as_ref().unwrap();
    assert_eq!(doc.code.edit.text, "edited");
    assert!(doc.dirty());
    assert_eq!(e.doc_reload, None);
    assert!(
        e.notice
            .as_ref()
            .is_some_and(|n| n.text.contains("permission denied"))
    );
}

// -- Req::CheckFile ---------------------------------------------------

/// Issue a disk check exactly like `autorefresh::poll`: the read lane
/// under the "checkfile" generation, `check_pending` armed. Returns
/// the revision the request carries.
fn issue_check(e: &mut EditorState) -> u64 {
    let doc = e.doc.as_ref().expect("open doc");
    let path = doc.path.clone();
    let revision = doc.code.revision;
    let checkout = e.checkout.clone().unwrap();
    let key = e.checkout_key.clone().unwrap();
    assert!(e.enqueue_read(checkout, key, Req::CheckFile { path, revision }));
    e.autorefresh.check_pending = true;
    revision
}

fn check_reply(e: &EditorState, revision: u64, result: Result<ReplyData, String>) -> Reply {
    reply(
        Req::CheckFile {
            path: e.doc.as_ref().unwrap().path.clone(),
            revision,
        },
        e.req_gens["checkfile"],
        e.checkout_key.clone(),
        result,
    )
}

#[test]
fn check_clean_doc_with_changed_disk_replaces_the_buffer() {
    let mut e = editor();
    select(&mut e);
    // A 12-char clean buffer with the caret near its end; the new disk
    // text is shorter, so the caret clamps to the new length.
    e.doc = Some(OpenDoc::new("a.txt", &text_file("orig text 12")));
    e.doc.as_mut().unwrap().code.edit.set_caret(10);
    let revision = issue_check(&mut e);
    e.apply_reply(check_reply(
        &e,
        revision,
        Ok(ReplyData::File(text_file("short"))),
    ));
    let doc = e.doc.as_ref().unwrap();
    // A clean buffer follows the disk silently: baseline AND buffer.
    assert_eq!(doc.code.edit.text, "short");
    assert_eq!(doc.original, "short");
    assert_eq!(doc.disk_notice, None);
    assert!(doc.code.revision > revision, "a stale probe drops");
    assert_eq!(doc.code.edit.focus, 5, "caret clamped to the new length");
    assert!(doc.highlight_dirty_at.is_some(), "highlight re-armed");
    assert!(!e.autorefresh.check_pending);
}

#[test]
fn check_dirty_doc_with_changed_disk_keeps_the_buffer_and_flags_it() {
    let mut e = editor();
    select(&mut e);
    e.doc = Some(OpenDoc::new("a.txt", &text_file("orig")));
    e.doc.as_mut().unwrap().code.edit.text = "dirty edit".into();
    let revision = issue_check(&mut e);
    e.apply_reply(check_reply(
        &e,
        revision,
        Ok(ReplyData::File(text_file("disk moved"))),
    ));
    let doc = e.doc.as_ref().unwrap();
    assert_eq!(doc.code.edit.text, "dirty edit", "the draft wins");
    assert_eq!(doc.original, "orig", "the baseline stays honest");
    assert_eq!(
        doc.disk_notice,
        Some(crate::editor::draft::DiskNotice::Changed)
    );
    // A save losing the same race flags the same notice — and the
    // earlier notice survives it.
    e.apply_reply(reply(
        Req::SaveFile {
            path: "a.txt".into(),
            expected: "orig".into(),
            text: "dirty edit".into(),
        },
        0,
        e.checkout_key.clone(),
        Err("the file changed on disk; reload it before saving".into()),
    ));
    let doc = e.doc.as_ref().unwrap();
    assert_eq!(
        doc.disk_notice,
        Some(crate::editor::draft::DiskNotice::Changed)
    );
    assert_eq!(
        doc.save_error.as_deref(),
        Some("the file changed on disk; reload it before saving")
    );
}

#[test]
fn check_with_same_text_clears_the_notice() {
    let mut e = editor();
    select(&mut e);
    e.doc = Some(OpenDoc::new("a.txt", &text_file("orig")));
    e.doc.as_mut().unwrap().disk_notice = Some(crate::editor::draft::DiskNotice::Changed);
    let revision = issue_check(&mut e);
    e.apply_reply(check_reply(
        &e,
        revision,
        Ok(ReplyData::File(text_file("orig"))),
    ));
    let doc = e.doc.as_ref().unwrap();
    assert_eq!(doc.disk_notice, None);
    assert_eq!(doc.code.edit.text, "orig");
}

#[test]
fn check_vanished_or_non_text_file_sets_deleted_or_not_text() {
    let mut e = editor();
    select(&mut e);
    e.doc = Some(OpenDoc::new("a.txt", &text_file("orig")));
    e.doc.as_mut().unwrap().code.edit.text = "dirty edit".into();
    for (kind, notice) in [
        ("missing", crate::editor::draft::DiskNotice::Deleted),
        ("binary", crate::editor::draft::DiskNotice::NotText),
    ] {
        let revision = issue_check(&mut e);
        let mut file = text_file("whatever");
        file.kind = kind;
        file.text = None;
        e.apply_reply(check_reply(&e, revision, Ok(ReplyData::File(file))));
        let doc = e.doc.as_ref().unwrap();
        assert_eq!(doc.disk_notice, Some(notice), "{kind}");
        assert_eq!(doc.code.edit.text, "dirty edit", "{kind}: buffer kept");
        assert!(!e.autorefresh.check_pending);
    }
}

#[test]
fn check_reply_with_a_stale_revision_or_while_saving_is_ignored() {
    let mut e = editor();
    select(&mut e);
    e.doc = Some(OpenDoc::new("a.txt", &text_file("orig")));
    let revision = issue_check(&mut e);
    // The user typed while the read was in flight → revision moved on.
    let doc = e.doc.as_mut().unwrap();
    doc.code.edit.text = "edited".into();
    doc.code.text_edited();
    e.apply_reply(check_reply(
        &e,
        revision,
        Ok(ReplyData::File(text_file("disk"))),
    ));
    assert_eq!(e.doc.as_ref().unwrap().code.edit.text, "edited");
    assert_eq!(e.doc.as_ref().unwrap().disk_notice, None);
    // A save in flight owns the doc's next state too.
    let revision = issue_check(&mut e);
    e.doc.as_mut().unwrap().saving = true;
    e.apply_reply(check_reply(
        &e,
        revision,
        Ok(ReplyData::File(text_file("disk"))),
    ));
    assert_eq!(e.doc.as_ref().unwrap().code.edit.text, "edited");
    assert_eq!(e.doc.as_ref().unwrap().disk_notice, None);
    assert!(!e.autorefresh.check_pending, "the probe still released");
}

#[test]
fn a_binary_image_gets_a_preview_and_a_binary_blob_does_not() {
    // A 1x1 PNG.
    const PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
        0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 218, 99, 100, 96, 248, 95, 15,
        0, 2, 135, 1, 128, 235, 71, 186, 146, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    let dir = std::env::temp_dir().join(format!("rune-media-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("dot.png"), PNG).unwrap();
    std::fs::write(dir.join("blob.bin"), [0u8, 1, 2]).unwrap();
    let mut e = editor();
    select(&mut e);
    e.checkout = Some(dir.clone());
    let waker: rune::Waker = std::sync::Arc::new(|| {});
    e.media_host = Some((rune::ImageCache::new(waker.clone()), waker));
    let binary = |path: &str| {
        let mut file = text_file("");
        file.path = path.into();
        file.kind = "binary";
        file.text = None;
        file
    };
    e.install_doc("dot.png".into(), binary("dot.png"), None);
    let doc = e.doc.as_ref().unwrap();
    let Some(crate::editor::media::Media::Image(image)) = &doc.media else {
        panic!("a PNG gets an image preview");
    };
    assert_eq!(image.wait().unwrap(), [1, 1], "the preview decodes");
    e.install_doc("blob.bin".into(), binary("blob.bin"), None);
    assert!(e.doc.as_ref().unwrap().media.is_none());
    let _ = std::fs::remove_dir_all(dir);
}

/// An ignored or empty folder sorts with the folders, before and after
/// it is opened — it must not jump when clicked.
#[test]
fn ignored_and_empty_folders_sort_with_the_folders_and_stay_put_when_opened() {
    let mut e = editor();
    select(&mut e);
    let key = e.checkout_key.clone().unwrap();
    e.files.paths = vec!["README.md".into(), "src/a.rs".into(), "top.txt".into()];
    e.files.ignored = vec!["public/".into(), "target/".into(), "zed.log".into()];
    e.files.empty_dirs.insert("new".into());
    e.rebuild_file_model();
    let roots = |e: &EditorState| -> Vec<String> {
        e.files.model.children(None).map(str::to_string).collect()
    };
    let want = [
        "new",
        "public",
        "src",
        "target",
        "README.md",
        "top.txt",
        "zed.log",
    ];
    assert_eq!(roots(&e), want, "folders first, files after");
    e.apply_reply(reply(
        Req::IgnoredChildren {
            dir: "public".into(),
        },
        0,
        Some(key),
        Ok(ReplyData::IgnoredChildren(vec!["public/a.png".into()])),
    ));
    assert_eq!(roots(&e), want, "opening a folder does not move it");
}

/// The whole way to a media file in an ignored folder, on a real Git
/// repository and the real worker jobs: open the folders one by one,
/// then the file.
#[test]
fn a_media_file_in_an_ignored_folder_opens_with_a_preview() {
    use crate::editor::media::Media;
    let (dir, storage) = temp_storage("rune-ignored-media");
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(repo.join("public/assets/clip")).unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "a@b.c"],
        vec!["config", "user.name", "t"],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(&args)
                .status()
                .unwrap()
                .success()
        );
    }
    std::fs::write(repo.join(".gitignore"), "public/assets\n").unwrap();
    std::fs::write(repo.join("README.md"), "hi\n").unwrap();
    std::fs::write(
        repo.join("public/assets/clip/lumen-with.mp4"),
        [0u8, 0, 0, 0x18, b'f', b't', b'y', b'p'],
    )
    .unwrap();
    std::fs::write(repo.join("public/assets/pic.png"), [0u8, 1, 2, 3]).unwrap();
    let reply = run_job(
        &storage,
        &Job::project(Req::AddProject {
            path: repo.display().to_string(),
            workspace_id: app_core::store::DEFAULT_WORKSPACE_ID.into(),
        }),
    );
    let Ok(ReplyData::Added(project)) = reply.result else {
        panic!("project added: {:?}", reply.result);
    };
    let checkout = std::fs::canonicalize(&repo).unwrap();
    let key = checkout_key(&project.project.id, &checkout);
    let mut e = editor();
    e.project_id = Some(project.project.id.clone());
    e.checkout = Some(checkout.clone());
    e.checkout_key = Some(key.clone());
    let waker: rune::Waker = std::sync::Arc::new(|| {});
    e.media_host = Some((rune::ImageCache::new(waker.clone()), waker));
    let run = |e: &mut EditorState, job: Job| {
        let reply = run_job(&storage, &job);
        assert!(reply.result.is_ok(), "{:?} -> {:?}", job.req, reply.result);
        e.apply_reply(reply);
    };
    let mut read = |e: &mut EditorState, req: Req| {
        assert!(e.enqueue_read(checkout.clone(), key.clone(), req.clone()));
    };
    let _ = &mut read;
    let (pid, co) = (project.project.id.clone(), checkout.clone());
    // The file list, with the ignored folder collapsed.
    let mut list = Job::read(&pid, co.clone(), key.clone(), Req::FileList);
    list.generation = {
        let g = e.req_gens.entry("filelist").or_insert(0);
        *g += 1;
        *g
    };
    run(&mut e, list);
    assert_eq!(
        e.files.model.get("public/assets").map(|n| n.data),
        Some(Some(crate::editor::state::FileMark::IgnoredDir))
    );
    for dir in ["public/assets", "public/assets/clip"] {
        run(
            &mut e,
            Job::write(
                &pid,
                co.clone(),
                key.clone(),
                Req::IgnoredChildren { dir: dir.into() },
            ),
        );
    }
    let file = "public/assets/clip/lumen-with.mp4";
    assert!(
        e.files.model.get(file).is_some(),
        "the media file is in the tree: {:?}",
        e.files
            .model
            .children(Some("public/assets/clip"))
            .collect::<Vec<_>>()
    );
    // Open it like the tree does.
    e.doc_seq += 1;
    let token = e.doc_seq;
    e.doc_load = Some((token, file.into()));
    let mut open = Job::read(
        &pid,
        co.clone(),
        key.clone(),
        Req::File {
            path: file.into(),
            token,
            line: None,
        },
    );
    open.generation = {
        let g = e.req_gens.entry("file").or_insert(0);
        *g += 1;
        *g
    };
    run(&mut e, open);
    let doc = e.doc.as_ref().expect("the file opened");
    assert_eq!(doc.path, file);
    assert!(
        matches!(doc.media, Some(Media::Player(_, true))),
        "a video preview"
    );
}

/// Opening the only ignored child of an open folder merges the two rows
/// (`public` + `assets` → `public/assets`): the merged row opens, and a
/// later rebuild keeps it open instead of dropping the stale id.
#[test]
fn a_folder_merged_by_opening_its_only_child_stays_open_across_rebuilds() {
    let mut e = editor();
    select(&mut e);
    let key = e.checkout_key.clone().unwrap();
    e.files.paths = vec!["README.md".into()];
    e.files.ignored = vec!["public/assets/".into()];
    e.rebuild_file_model();
    e.files.model.set_expanded("public", true);
    e.apply_reply(reply(
        Req::IgnoredChildren {
            dir: "public/assets".into(),
        },
        0,
        Some(key),
        Ok(ReplyData::IgnoredChildren(vec![
            "public/assets/clip/".into(),
        ])),
    ));
    let m = &e.files.model;
    assert!(
        m.contains("public/assets") && !m.contains("public"),
        "merged row"
    );
    assert!(m.is_expanded("public/assets"));
    assert_eq!(
        m.children(Some("public/assets")).collect::<Vec<_>>(),
        ["public/assets/clip"]
    );
    e.rebuild_file_model();
    assert!(
        e.files.model.is_expanded("public/assets"),
        "a rebuild keeps it"
    );
}

#[test]
fn an_opened_ignored_directory_shows_its_entries_and_keeps_the_open_folders() {
    use crate::editor::state::FileMark;
    let mut e = editor();
    select(&mut e);
    let key = e.checkout_key.clone().unwrap();
    e.files.paths = vec!["README.md".into(), "src/a.rs".into()];
    e.files.ignored = vec!["public/assets/".into(), "target/".into()];
    e.rebuild_file_model();
    e.files.model.set_expanded("src", true);
    let data = |e: &EditorState, id: &str| e.files.model.get(id).map(|n| n.data);
    assert_eq!(data(&e, "public/assets"), Some(Some(FileMark::IgnoredDir)));
    e.apply_reply(reply(
        Req::IgnoredChildren {
            dir: "public/assets".into(),
        },
        0,
        Some(key),
        Ok(ReplyData::IgnoredChildren(vec![
            "public/assets/a.png".into(),
            "public/assets/deep/".into(),
        ])),
    ));
    assert_eq!(
        data(&e, "public/assets/a.png"),
        Some(Some(FileMark::Ignored))
    );
    assert_eq!(
        data(&e, "public/assets/deep"),
        Some(Some(FileMark::IgnoredDir)),
        "a nested ignored directory is a leaf that opens in turn"
    );
    assert!(
        e.files.model.is_expanded("public/assets"),
        "the folder opened"
    );
    assert!(
        e.files.model.is_expanded("src"),
        "other open folders stay open"
    );
    assert_eq!(data(&e, "target"), Some(Some(FileMark::IgnoredDir)));
    // A refresh keeps what the user opened.
    e.rebuild_file_model();
    assert!(e.files.model.contains("public/assets/a.png"));
}

#[test]
fn a_created_folder_shows_as_an_empty_row_and_a_created_file_opens() {
    use crate::editor::state::FileMark;
    let (backend, jobs) = channel_backend();
    let mut e = editor();
    e.backend = backend;
    select(&mut e);
    let key = e.checkout_key.clone().unwrap();
    e.files.paths = vec!["src/a.rs".into()];
    e.rebuild_file_model();
    let created = |dir: &str, name: &str, folder, path: &str| {
        reply(
            Req::CreateEntry {
                dir: dir.into(),
                name: name.into(),
                folder,
            },
            0,
            Some(key.clone()),
            Ok(ReplyData::Created(path.into())),
        )
    };
    e.apply_reply(created("src", "docs", true, "src/docs"));
    // The list re-reads, then the tree opens on the new folder.
    e.apply_reply(reply(
        Req::FileList,
        e.req_gens.get("filelist").copied().unwrap_or(0),
        Some(key.clone()),
        Ok(ReplyData::FileList(
            vec!["src/a.rs".into()],
            Vec::new(),
            false,
        )),
    ));
    assert_eq!(
        e.files.model.get("src/docs").map(|n| n.data),
        Some(Some(FileMark::EmptyDir))
    );
    assert!(e.files.model.is_expanded("src"), "revealed");
    assert_eq!(e.files.model.cursor(), Some("src/docs"));
    // Once a file lands in it, the folder is a real one again.
    e.files.paths = vec!["src/a.rs".into(), "src/docs/x.md".into()];
    e.rebuild_file_model();
    assert_eq!(e.files.model.get("src/docs").map(|n| n.dir), Some(true));
    // A new file opens in the editor.
    e.apply_reply(created("src", "b.rs", false, "src/b.rs"));
    assert!(
        jobs.try_iter()
            .any(|j| matches!(&j.req, Req::File { path, .. } if path == "src/b.rs"))
    );
    assert_eq!(e.files.reveal.as_deref(), Some("src/b.rs"));
    e.apply_reply(reply(
        Req::CreateEntry {
            dir: "src".into(),
            name: "b.rs".into(),
            folder: false,
        },
        0,
        Some(key),
        Err("b.rs already exists".into()),
    ));
    assert!(
        e.notice
            .as_ref()
            .is_some_and(|n| n.text.contains("already exists"))
    );
}

#[test]
fn a_trash_reply_refreshes_and_closes_a_clean_open_doc_but_keeps_a_draft() {
    let (backend, jobs) = channel_backend();
    let mut e = editor();
    e.backend = backend;
    select(&mut e);
    let key = e.checkout_key.clone().unwrap();
    let req = || Req::TrashPath { path: "src".into() };
    e.doc = Some(OpenDoc::new("src/a.txt", &text_file("x")));
    e.apply_reply(reply(req(), 0, Some(key.clone()), Ok(ReplyData::Ok)));
    assert!(e.doc.is_none(), "the clean doc closed with its folder");
    assert!(
        jobs.try_iter().any(|j| matches!(j.req, Req::FileList)),
        "the tree re-reads"
    );
    e.doc = Some(OpenDoc::new("other.txt", &text_file("x")));
    e.apply_reply(reply(req(), 0, Some(key.clone()), Ok(ReplyData::Ok)));
    assert!(e.doc.is_some(), "an unrelated doc stays");
    e.doc = Some(OpenDoc::new("src/a.txt", &text_file("x")));
    e.doc.as_mut().unwrap().code.edit.text = "edited".into();
    e.apply_reply(reply(req(), 0, Some(key.clone()), Ok(ReplyData::Ok)));
    assert!(e.doc.is_some(), "a draft is kept");
    assert!(e.notice.as_ref().is_some_and(|n| n.text.contains("Trash")));
    e.notice = None;
    e.apply_reply(reply(req(), 0, Some(key), Err("denied".into())));
    assert!(e.notice.as_ref().is_some_and(|n| n.text == "denied"));
}

#[test]
fn a_rename_reply_refreshes_and_moves_a_clean_open_doc() {
    let (backend, jobs) = channel_backend();
    let mut e = editor();
    e.backend = backend;
    select(&mut e);
    let key = e.checkout_key.clone().unwrap();
    e.doc = Some(OpenDoc::new("src/a.txt", &text_file("x")));
    let req = || Req::RenamePath {
        path: "src".into(),
        new_name: "lib".into(),
    };
    e.apply_reply(reply(
        req(),
        0,
        Some(key.clone()),
        Ok(ReplyData::Renamed("lib".into())),
    ));
    assert!(e.doc.is_none(), "the clean doc reloads from its new path");
    assert!(e.doc_load.is_some());
    let sent: Vec<Job> = jobs.try_iter().collect();
    assert!(sent.iter().any(|j| matches!(j.req, Req::FileList)));
    assert!(
        sent.iter()
            .any(|j| matches!(&j.req, Req::File { path, .. } if path == "lib/a.txt"))
    );
    // A draft stays where it is, with a notice.
    e.doc = Some(OpenDoc::new("src/a.txt", &text_file("x")));
    e.doc.as_mut().unwrap().code.edit.text = "edited".into();
    e.apply_reply(reply(
        req(),
        0,
        Some(key),
        Ok(ReplyData::Renamed("lib".into())),
    ));
    assert!(e.doc.is_some());
    assert!(
        e.notice
            .as_ref()
            .is_some_and(|n| n.text.contains("lib/a.txt"))
    );
}

#[test]
fn reload_file_non_text_reply_preserves_buffer() {
    let mut e = editor();
    select(&mut e);
    e.doc = Some(OpenDoc::new("a.txt", &text_file("orig")));
    e.doc.as_mut().unwrap().code.edit.text = "edited".into();
    let (token, revision) = issue_reload(&mut e);
    let mut binary = text_file("");
    binary.kind = "binary";
    binary.text = None;
    e.apply_reply(reload_reply(
        &e,
        token,
        revision,
        Ok(ReplyData::File(binary)),
    ));
    let doc = e.doc.as_ref().unwrap();
    assert_eq!(doc.code.edit.text, "edited");
    assert!(doc.dirty());
    assert!(
        e.notice
            .as_ref()
            .is_some_and(|n| n.text.contains("not UTF-8"))
    );
}

#[test]
fn normal_file_reply_does_not_block_pending_reload() {
    let mut e = editor();
    select(&mut e);
    e.doc = Some(OpenDoc::new("a.txt", &text_file("orig")));
    // A same-path File reply takes the reveal fast path — it must not
    // consume or invalidate the in-flight reload.
    e.doc_load = Some((99, "a.txt".into()));
    let file_gen = e.req_gens.get("file").copied().unwrap_or(0) + 1;
    e.req_gens.insert("file", file_gen);
    e.apply_reply(reply(
        Req::File {
            path: "a.txt".into(),
            token: 99,
            line: Some(3),
        },
        file_gen,
        e.checkout_key.clone(),
        Ok(ReplyData::File(text_file("orig"))),
    ));
    assert_eq!(e.doc.as_ref().unwrap().reveal_line, Some(3));
    let (token, revision) = issue_reload(&mut e);
    e.apply_reply(reload_reply(
        &e,
        token,
        revision,
        Ok(ReplyData::File(text_file("disk"))),
    ));
    assert_eq!(e.doc.as_ref().unwrap().code.edit.text, "disk");
}

#[test]
fn install_doc_leaving_changes_cancels_acp() {
    let mut e = editor();
    select(&mut e);
    e.section = Section::Changes;
    e.acp.generating_for = Some("p1\u{1f}/tmp/co".into());
    e.install_doc("a.txt".into(), text_file("fresh"), None);
    assert_eq!(e.section, Section::Files);
    assert_eq!(e.acp.generating_for, None, "Changes→Files must cancel ACP");
}

#[test]
fn install_doc_same_path_leaving_changes_cancels_acp() {
    let mut e = editor();
    select(&mut e);
    e.doc = Some(OpenDoc::new("a.txt", &text_file("orig")));
    e.section = Section::Changes;
    e.acp.generating_for = Some("p1\u{1f}/tmp/co".into());
    e.install_doc("a.txt".into(), text_file("orig"), Some(2));
    assert_eq!(e.section, Section::Files);
    assert_eq!(e.doc.as_ref().unwrap().reveal_line, Some(2));
    assert_eq!(e.acp.generating_for, None);
}

/// A file opened from Changes (or History) lands in Files with no list
/// cached: the activation queues the `FileList` read itself instead of
/// leaving "No files" until a manual refresh. A list already cached
/// for this checkout queues nothing.
#[test]
fn opening_a_file_loads_the_file_list_when_it_is_missing() {
    let (backend, jobs) = channel_backend();
    let mut e = editor();
    e.backend = backend;
    select(&mut e);
    e.section = Section::Changes;
    e.install_doc("a.txt".into(), text_file("fresh"), None);
    assert_eq!(e.section, Section::Files);
    let sent: Vec<Job> = jobs.try_iter().collect();
    assert_eq!(
        sent.iter()
            .filter(|j| matches!(j.req, Req::FileList))
            .count(),
        1,
        "an empty file list loads once: {sent:?}"
    );

    let (backend, jobs) = channel_backend();
    let mut e = editor();
    e.backend = backend;
    select(&mut e);
    e.section = Section::Changes;
    e.files.paths = vec!["a.txt".into()];
    e.rebuild_file_model();
    e.install_doc("a.txt".into(), text_file("fresh"), None);
    assert_eq!(e.section, Section::Files);
    assert!(
        !jobs.try_iter().any(|j| matches!(j.req, Req::FileList)),
        "a cached list for this checkout queues no read"
    );
}

// -- Backend::shutdown ---------------------------------------------------

#[test]
fn shutdown_joins_worker_and_refuses_new_jobs() {
    let mut backend = Backend::new(crate::test_handle());
    // Queue reads + a mutating job, then shut down: the worker must
    // drain mutations, skip reads and exit — join() returning is the
    // proof (a leaked worker would hang the test).
    for _ in 0..8 {
        assert!(backend.send(Job::read(
            "p1",
            PathBuf::from("/tmp/co"),
            "k".into(),
            Req::Changes
        )));
    }
    assert!(backend.send(Job::write(
        "p1",
        PathBuf::from("/tmp/co"),
        "k".into(),
        Req::SetSetting {
            key: "k".into(),
            value: "v".into(),
        },
    )));
    backend.shutdown();
    backend.shutdown(); // idempotent
    assert!(!backend.send(Job::project(Req::Projects)));
}

#[test]
fn shutdown_skips_reads_but_runs_mutations() {
    // The drain predicate is the contract: reads/validation drop,
    // writes finish. Pinned directly so a future Req variant can't
    // silently join the wrong side.
    for req in [
        Req::Changes,
        Req::FileList,
        Req::File {
            path: "a".into(),
            token: 1,
            line: None,
        },
        Req::ReloadFile {
            path: "a".into(),
            token: 1,
            revision: 1,
        },
        Req::Highlight {
            path: "a".into(),
            revision: 1,
            text: String::new(),
        },
        Req::ExternalOpen {
            project_id: "p".into(),
            checkout_path: "c".into(),
            path: "a".into(),
            line: None,
        },
        Req::Projects,
        Req::Workspaces,
        Req::GetSetting { key: "k".into() },
    ] {
        assert!(!is_mutating(&req), "{req:?} must be skippable on shutdown");
    }
    for req in [
        Req::AddProject {
            path: "/tmp/x".into(),
            workspace_id: app_core::store::DEFAULT_WORKSPACE_ID.into(),
        },
        Req::CreateWorkspace {
            name: "Work".into(),
            token: 1,
        },
        Req::RenameWorkspace {
            id: "w".into(),
            name: "Work".into(),
            token: 1,
        },
        Req::RemoveWorkspace { id: "w".into() },
        Req::MoveProject {
            project_id: "p".into(),
            workspace_id: "w".into(),
        },
        Req::SetSetting {
            key: "k".into(),
            value: "v".into(),
        },
        Req::SaveFile {
            path: "a".into(),
            expected: String::new(),
            text: String::new(),
        },
        Req::OverwriteFile {
            path: "a".into(),
            text: String::new(),
        },
        Req::Stage { paths: None },
        Req::Unstage { paths: None },
        Req::Discard {
            layer: "l".into(),
            path: "a".into(),
        },
        Req::Resolve {
            path: "a".into(),
            action: "current".into(),
        },
        Req::Operation {
            step: "abort".into(),
        },
        Req::Commit {
            message: "m".into(),
        },
        Req::Push,
    ] {
        assert!(is_mutating(&req), "{req:?} must finish on shutdown");
    }
}

// -- accounts + usage ----------------------------------------------------

use app_core::test_support::TempDir;

fn temp_storage(name: &str) -> (TempDir, Option<Storage>) {
    let dir = TempDir::new(name);
    let storage = Some(Storage::open(dir.path()));
    (dir, storage)
}

fn concept_repo(name: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new(name);
    let root = dir.path().canonicalize().unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {:?}", out);
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.name", "Tester"]);
    git(&["config", "user.email", "tester@example.com"]);
    std::fs::write(root.join("a.txt"), "tracked needle\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "init"]);
    (dir, root)
}

fn concept_storage(name: &str) -> (TempDir, PathBuf, Option<Storage>, String) {
    let (dir, root) = concept_repo(name);
    let storage = Some(Storage::open(dir.path()));
    let project = {
        let store = storage.as_ref().unwrap().get().unwrap();
        app_core::projects::add_project(
            store,
            root.to_str().unwrap(),
            app_core::store::DEFAULT_WORKSPACE_ID,
        )
        .unwrap()
    };
    (dir, root, storage, project.project.id)
}

/// `concept_storage` with the merge stopped on a both-modified conflict
/// in `a.txt`.
fn conflict_storage(name: &str) -> (TempDir, PathBuf, Option<Storage>, String) {
    let (dir, root) = concept_repo(name);
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {:?}", out);
    };
    git(&["checkout", "-qb", "side"]);
    std::fs::write(root.join("a.txt"), "theirs\n").unwrap();
    git(&["commit", "-qam", "side"]);
    git(&["checkout", "-q", "main"]);
    std::fs::write(root.join("a.txt"), "ours\n").unwrap();
    git(&["commit", "-qam", "ours"]);
    assert!(
        !std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["merge", "side"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let storage = Some(Storage::open(dir.path()));
    let project = {
        let store = storage.as_ref().unwrap().get().unwrap();
        app_core::projects::add_project(
            store,
            root.to_str().unwrap(),
            app_core::store::DEFAULT_WORKSPACE_ID,
        )
        .unwrap()
    };
    (dir, root, storage, project.project.id)
}

/// The lookup that `ps -E` cannot do on Linux: a live process's
/// `CODEX_THREAD_ID` comes back, and one without it is left out.
#[cfg(target_os = "linux")]
#[test]
fn thread_ids_reads_a_live_process_environment() {
    let spawn = |thread: Option<&str>| {
        let mut command = std::process::Command::new("sleep");
        command.arg("30").env_remove("CODEX_THREAD_ID");
        if let Some(thread) = thread {
            command.env("CODEX_THREAD_ID", thread);
        }
        command.spawn().expect("spawn sleep")
    };
    let mut with = spawn(Some("0199-thread"));
    let mut without = spawn(None);
    let ids = thread_ids(&[with.id(), without.id(), u32::MAX]);
    let _ = with.kill();
    let _ = without.kill();
    let _ = with.wait();
    let _ = without.wait();
    assert_eq!(ids.len(), 1);
    assert_eq!(ids.get(&with.id()).map(String::as_str), Some("0199-thread"));
}

#[test]
fn concept_search_returns_index_matches() {
    let (_dir, root, storage, project_id) = concept_storage("backend-concept");
    app_core::code_index::refresh(storage.as_ref().unwrap().home(), &root, |_, _| true).unwrap();
    let key = checkout_key(&project_id, &root);
    let job = Job::read(
        &project_id,
        root.clone(),
        key,
        Req::ConceptSearch {
            query: "needle".into(),
        },
    );
    let reply = run_job(&storage, &job);
    match reply.result {
        Ok(ReplyData::Search(view)) => {
            assert!(view.matches.iter().any(|m| m.path == "a.txt"));
        }
        other => panic!("expected Search, got {other:?}"),
    }
}

#[test]
fn concept_search_without_an_index_writes_nothing() {
    let (_dir, root, storage, project_id) = concept_storage("backend-concept-empty");
    let key = checkout_key(&project_id, &root);
    let job = Job::read(
        &project_id,
        root.clone(),
        key,
        Req::ConceptSearch {
            query: "needle".into(),
        },
    );
    let reply = run_job(&storage, &job);
    match reply.result {
        Ok(ReplyData::Search(view)) => assert!(view.matches.is_empty()),
        other => panic!("expected Search, got {other:?}"),
    }
    assert!(!storage.as_ref().unwrap().home().join("search").exists());
}

fn nested_parent(name: &str) -> (TempDir, PathBuf, PathBuf, PathBuf) {
    let dir = TempDir::new(name);
    let parent = dir.path().canonicalize().unwrap();
    let first = parent.join("first");
    let second = parent.join("second");
    for root in [&first, &second] {
        std::fs::create_dir_all(root).unwrap();
        for args in [
            vec!["init", "-b", "main"],
            vec!["config", "user.name", "Tester"],
            vec!["config", "user.email", "tester@example.com"],
        ] {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(&args)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {out:?}");
        }
        std::fs::write(root.join("file.txt"), "hello\n").unwrap();
        for args in [vec!["add", "-A"], vec!["commit", "-qm", "init"]] {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(&args)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {out:?}");
        }
    }
    (dir, parent, first, second)
}

#[test]
fn scan_and_import_requests_cover_group_separate_and_tokens() {
    let (_dir, parent, first, second) = nested_parent("backend-nested-import");
    let (storage_dir, storage) = temp_storage("backend-nested-store");
    let _ = storage_dir;
    let scan = run_job(
        &storage,
        &Job::project(Req::ScanNestedRepos {
            path: parent.display().to_string(),
            token: 7,
        }),
    );
    let Ok(ReplyData::NestedScan { token, result }) = scan.result else {
        panic!("expected a scan, got {:?}", scan.result);
    };
    assert_eq!(token, 7);
    assert_eq!(result.repos.len(), 2);

    let paths = vec![first.display().to_string(), second.display().to_string()];
    let import = run_job(
        &storage,
        &Job::project(Req::ImportNestedRepos {
            parent_path: parent.display().to_string(),
            parent_id: None,
            group_name: "Group".into(),
            project_paths: paths.clone(),
            mode: app_core::projects::ImportMode::Group,
            workspace_id: app_core::store::DEFAULT_WORKSPACE_ID.into(),
            token: 8,
        }),
    );
    let Ok(ReplyData::NestedImport { token, result }) = import.result else {
        panic!("expected an import, got {:?}", import.result);
    };
    assert_eq!(token, 8);
    assert_eq!(result.imported, 2);
    assert_eq!(result.parent.unwrap().project.name, "Group");

    let again = run_job(
        &storage,
        &Job::project(Req::ImportNestedRepos {
            parent_path: parent.display().to_string(),
            parent_id: None,
            group_name: "Group".into(),
            project_paths: paths,
            mode: app_core::projects::ImportMode::Group,
            workspace_id: app_core::store::DEFAULT_WORKSPACE_ID.into(),
            token: 9,
        }),
    );
    let Ok(ReplyData::NestedImport { token, result }) = again.result else {
        panic!("expected an import, got {:?}", again.result);
    };
    assert_eq!(token, 9);
    assert_eq!(result.already_known, 2);

    let (_dir2, parent2, first2, second2) = nested_parent("backend-nested-separate");
    let (storage_dir2, storage2) = temp_storage("backend-nested-store2");
    let _ = storage_dir2;
    let separate = run_job(
        &storage2,
        &Job::project(Req::ImportNestedRepos {
            parent_path: parent2.display().to_string(),
            parent_id: None,
            group_name: String::new(),
            project_paths: vec![first2.display().to_string(), second2.display().to_string()],
            mode: app_core::projects::ImportMode::Separate,
            workspace_id: app_core::store::DEFAULT_WORKSPACE_ID.into(),
            token: 10,
        }),
    );
    let Ok(ReplyData::NestedImport { token, result }) = separate.result else {
        panic!("expected an import, got {:?}", separate.result);
    };
    assert_eq!(token, 10);
    assert!(result.parent.is_none());
    assert_eq!(result.imported, 2);
}

#[test]
fn file_jobs_accept_a_folder_root_and_git_jobs_still_require_a_worktree() {
    let (_dir, parent, _first, _second) = nested_parent("backend-folder-files");
    let (storage_dir, storage) = temp_storage("backend-folder-store");
    let _ = storage_dir;
    let added = run_job(
        &storage,
        &Job::project(Req::AddProject {
            path: parent.display().to_string(),
            workspace_id: app_core::store::DEFAULT_WORKSPACE_ID.into(),
        }),
    );
    let Ok(ReplyData::Added(project)) = added.result else {
        panic!("expected the folder project, got {:?}", added.result);
    };
    let key = checkout_key(&project.project.id, &parent);

    let list = run_job(
        &storage,
        &Job::read(
            &project.project.id,
            parent.clone(),
            key.clone(),
            Req::FileList,
        ),
    );
    let Ok(ReplyData::FileList(paths, ignored, truncated)) = list.result else {
        panic!("expected a file list, got {:?}", list.result);
    };
    assert!(paths.iter().any(|path| path == "first/file.txt"));
    assert!(ignored.is_empty());
    assert!(!truncated);

    let file = run_job(
        &storage,
        &Job::read(
            &project.project.id,
            parent.clone(),
            key.clone(),
            Req::File {
                path: "first/file.txt".into(),
                token: 1,
                line: None,
            },
        ),
    );
    let Ok(ReplyData::File(file)) = file.result else {
        panic!("expected a file, got {:?}", file.result);
    };
    assert_eq!(file.text.as_deref(), Some("hello\n"));

    let saved = run_job(
        &storage,
        &Job::write(
            &project.project.id,
            parent.clone(),
            key.clone(),
            Req::SaveFile {
                path: "first/file.txt".into(),
                expected: "hello\n".into(),
                text: "changed\n".into(),
            },
        ),
    );
    assert!(matches!(saved.result, Ok(ReplyData::Saved(_))));

    let changes = run_job(
        &storage,
        &Job::read(&project.project.id, parent, key, Req::Changes),
    );
    assert!(changes.result.is_err(), "{:?}", changes.result);
}

#[test]
fn resolve_job_clears_a_conflict_and_refreshes_changes() {
    let (_dir, root, storage, project_id) = conflict_storage("backend-resolve");
    let key = checkout_key(&project_id, &root);
    let job = Job::write(
        &project_id,
        root.clone(),
        key.clone(),
        Req::Resolve {
            path: "a.txt".into(),
            action: "current".into(),
        },
    );
    let reply = run_job(&storage, &job);
    assert!(
        matches!(reply.result, Ok(ReplyData::Ok)),
        "resolve failed: {:?}",
        reply.result
    );
    let reply = run_job(
        &storage,
        &Job::read(&project_id, root.clone(), key, Req::Changes),
    );
    match reply.result {
        Ok(ReplyData::Changes(view)) => {
            assert!(
                !view.conflicts.iter().any(|c| c.path == "a.txt"),
                "a.txt is still in conflict: {:?}",
                view.conflicts
            );
        }
        other => panic!("expected Changes, got {other:?}"),
    }
}

#[test]
fn resolve_with_an_invalid_action_sets_the_section_error() {
    let (_dir, root, storage, project_id) = conflict_storage("backend-resolve-invalid");
    let key = checkout_key(&project_id, &root);
    // `keep` applies to deletion/addition conflicts, not a both-modified
    // one — the worker re-reads status and refuses.
    let job = Job::write(
        &project_id,
        root.clone(),
        key,
        Req::Resolve {
            path: "a.txt".into(),
            action: "keep".into(),
        },
    );
    let mut reply = run_job(&storage, &job);
    assert!(
        reply.result.is_err(),
        "expected Err, got {:?}",
        reply.result
    );
    let mut e = editor();
    select(&mut e);
    e.changes.busy = Some("resolve");
    reply.checkout_key = e.checkout_key.clone();
    e.apply_reply(reply);
    assert!(e.changes.busy.is_none());
    assert!(e.changes.error.is_some());
    assert!(e.notice.is_some());
}

/// The Overwrite button's write on a real repository: the buffer lands
/// over the drifted disk text, and a disk file that stopped being text
/// refuses before the write is attempted.
#[test]
fn overwrite_saves_the_buffer_over_changed_disk_text() {
    let (_dir, root, storage, project_id) = concept_storage("backend-overwrite");
    let key = checkout_key(&project_id, &root);
    let mut e = editor();
    e.project_id = Some(project_id.clone());
    e.checkout = Some(root.clone());
    e.checkout_key = Some(key.clone());
    // The doc loaded when disk still held the committed text; its
    // buffer drifted — and then the disk moved too (the drift the
    // notice row is armed for).
    e.doc = Some(OpenDoc::new("a.txt", &text_file("tracked needle\n")));
    e.doc.as_mut().unwrap().code.edit.text = "buffer wins\n".into();
    e.doc.as_mut().unwrap().saving = true;
    e.doc.as_mut().unwrap().disk_notice = Some(crate::editor::draft::DiskNotice::Changed);
    std::fs::write(root.join("a.txt"), "disk moved\n").unwrap();
    let reply = run_job(
        &storage,
        &Job::write(
            &project_id,
            root.clone(),
            key,
            Req::OverwriteFile {
                path: "a.txt".into(),
                text: "buffer wins\n".into(),
            },
        ),
    );
    assert!(
        matches!(reply.result, Ok(ReplyData::Saved(_))),
        "overwrite failed: {:?}",
        reply.result
    );
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).unwrap(),
        "buffer wins\n",
        "disk holds the buffer"
    );
    e.apply_reply(reply);
    let doc = e.doc.as_ref().unwrap();
    assert_eq!(doc.original, "buffer wins\n", "baseline == buffer");
    assert_eq!(doc.code.edit.text, "buffer wins\n");
    assert_eq!(doc.disk_notice, None, "the notice clears");
    assert!(!doc.saving);
    // The expected-text guard the worker still enforces: a disk file
    // that stopped being complete text refuses before the replace.
    std::fs::write(root.join("a.txt"), [0xff, 0xfe]).unwrap();
    let reply = run_job(
        &storage,
        &Job::write(
            &project_id,
            root.clone(),
            checkout_key(&project_id, &root),
            Req::OverwriteFile {
                path: "a.txt".into(),
                text: "buffer wins\n".into(),
            },
        ),
    );
    assert_eq!(reply.result.unwrap_err(), "the file is no longer text");
}

#[test]
fn concept_and_content_replies_are_dropped_in_the_other_mode() {
    use crate::editor::state::search_state::SearchMode;
    use app_core::git_view::{GitViewSearch, GitViewSearchMatch};
    let mut e = editor();
    select(&mut e);
    let key = e.checkout_key.clone();
    let view = || GitViewSearch {
        matches: vec![GitViewSearchMatch {
            path: "a.txt".into(),
            line: 1,
            text: "needle".into(),
        }],
        truncated: false,
    };
    e.files.search.mode = SearchMode::Content;
    e.files.search.query = "needle".into();
    e.apply_reply(reply(
        Req::ConceptSearch {
            query: "needle".into(),
        },
        0,
        key.clone(),
        Ok(ReplyData::Search(view())),
    ));
    assert!(e.files.search.results.is_none());
    e.files.search.mode = SearchMode::Concept;
    e.apply_reply(reply(
        Req::Search {
            query: "needle".into(),
        },
        0,
        key.clone(),
        Ok(ReplyData::Search(view())),
    ));
    assert!(e.files.search.results.is_none());
    e.apply_reply(reply(
        Req::ConceptSearch {
            query: "needle".into(),
        },
        0,
        key,
        Ok(ReplyData::Search(view())),
    ));
    assert!(e.files.search.results.is_some());
}

#[test]
fn concept_search_errors_are_prefixed_except_no_words() {
    use crate::editor::state::search_state::SearchMode;
    let mut e = editor();
    select(&mut e);
    let key = e.checkout_key.clone();
    e.files.search.mode = SearchMode::Concept;
    e.files.search.query = "needle".into();
    e.apply_reply(reply(
        Req::ConceptSearch {
            query: "needle".into(),
        },
        0,
        key.clone(),
        Err("boom".into()),
    ));
    assert_eq!(
        e.files.search.error.as_deref(),
        Some("Context search unavailable: boom")
    );
    e.files.search.query = "needle".into();
    e.apply_reply(reply(
        Req::ConceptSearch {
            query: "needle".into(),
        },
        0,
        key,
        Err("No searchable words".into()),
    ));
    assert_eq!(e.files.search.error.as_deref(), Some("No searchable words"));
}

#[test]
fn concept_search_has_its_own_request_kind() {
    assert_ne!(
        kind_key(&Req::Search {
            query: "needle".into()
        }),
        kind_key(&Req::ConceptSearch {
            query: "needle".into()
        })
    );
}

fn accounts_of(reply: &Reply) -> &app_core::accounts::AccountsView {
    match &reply.result {
        Ok(ReplyData::Accounts(view)) => view,
        other => panic!("expected Accounts, got {other:?}"),
    }
}

#[test]
fn account_add_rename_set_active_and_metadata_only_remove() {
    let (dir, storage) = temp_storage("rune-acct-rt");
    let acct_dir = dir.path().join("acct with space");
    let reply = run_job(
        &storage,
        &Job::project(Req::AccountAdd {
            agent: "claude".into(),
            name: "work".into(),
            dir: acct_dir.display().to_string(),
            token: 1,
        }),
    );
    let view = accounts_of(&reply);
    assert_eq!(view.claude.items.len(), 2, "Default plus the new row");
    let added = view
        .claude
        .items
        .iter()
        .find(|item| !item.is_default)
        .expect("the named account");
    assert_eq!(added.name, "work");
    assert_eq!(view.claude.active, added.id, "add activates the account");
    assert!(acct_dir.is_dir(), "add creates the folder");

    let id = added.id.clone();
    let reply = run_job(
        &storage,
        &Job::project(Req::AccountRename {
            agent: "claude".into(),
            id: id.clone(),
            name: "work-renamed".into(),
            token: 2,
        }),
    );
    let view = accounts_of(&reply);
    assert_eq!(
        view.claude
            .items
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.name.as_str()),
        Some("work-renamed")
    );

    let reply = run_job(
        &storage,
        &Job::project(Req::AccountSetActive {
            agent: "claude".into(),
            id: "default".into(),
        }),
    );
    assert_eq!(accounts_of(&reply).claude.active, "default");

    let reply = run_job(
        &storage,
        &Job::project(Req::AccountRemove {
            agent: "claude".into(),
            id: id.clone(),
        }),
    );
    assert_eq!(accounts_of(&reply).claude.items.len(), 1);
    assert!(
        acct_dir.is_dir(),
        "remove is metadata-only: the folder and its credentials stay"
    );
}

#[test]
fn account_add_rejects_a_duplicate_name() {
    let (dir, storage) = temp_storage("rune-acct-dup");
    let mk = || {
        Job::project(Req::AccountAdd {
            agent: "claude".into(),
            name: "dup".into(),
            dir: dir.path().join("a").display().to_string(),
            token: 1,
        })
    };
    assert!(run_job(&storage, &mk()).result.is_ok());
    let reply = run_job(&storage, &mk());
    assert!(reply.result.is_err(), "the second add must fail");
}

/// Other presets and a Devin registry with no named accounts still launch
/// without an account override.
#[test]
fn agent_argv_serves_the_presets_without_accounts() {
    let (dir, storage) = temp_storage("rune-argv-presets");
    // The shell preset needs no binary and no account.
    let reply = run_job(
        &storage,
        &Job::project(Req::AgentArgv {
            agent: "shell".into(),
            token: 1,
        }),
    );
    let Ok(ReplyData::AgentArgv(argv)) = &reply.result else {
        panic!("expected AgentArgv, got {:?}", reply.result);
    };
    assert_eq!(*argv, vec![default_shell()]);

    // The other presets are never refused for their account: they
    // fail only when their binary is missing.
    for agent in ["opencode", "devin"] {
        let reply = run_job(
            &storage,
            &Job::project(Req::AgentArgv {
                agent: agent.into(),
                token: 2,
            }),
        );
        if let Err(error) = &reply.result {
            assert!(
                !error.contains("unknown agent"),
                "{agent} must not be refused as an unknown agent: {error}"
            );
        }
    }
    let _ = dir;
}

/// A worker-side `AgentArgv` resolves through the stored accounts: the
/// active named account's folder is required to exist.
#[test]
fn agent_argv_refuses_a_missing_account_folder() {
    let (dir, storage) = temp_storage("rune-argv-missing");
    let acct_dir = dir.path().join("gone");
    let reply = run_job(
        &storage,
        &Job::project(Req::AccountAdd {
            agent: "claude".into(),
            name: "gone".into(),
            dir: acct_dir.display().to_string(),
            token: 1,
        }),
    );
    assert!(reply.result.is_ok());
    std::fs::remove_dir_all(&acct_dir).unwrap();
    let reply = run_job(
        &storage,
        &Job::project(Req::AgentArgv {
            agent: "claude".into(),
            token: 1,
        }),
    );
    let Err(error) = &reply.result else {
        panic!("a missing account folder must fail, got {:?}", reply.result);
    };
    assert!(error.contains("no longer exists"), "{error}");
}

/// `PATH`/`GOKO_HOME`/`CODEX_HOME` point at temp dirs for the launch
/// and usage reads — the real account folders are never touched.
#[cfg(unix)]
struct EnvGuard(&'static str, Option<std::ffi::OsString>);
#[cfg(unix)]
impl EnvGuard {
    fn set(key: &'static str, value: &std::ffi::OsStr) -> Self {
        let prev = std::env::var_os(key);
        // SAFETY: only used by the single combined env-mutating unit test in
        // this module, and restored on Drop.
        unsafe { std::env::set_var(key, value) };
        EnvGuard(key, prev)
    }
}
#[cfg(unix)]
impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.1 {
            // SAFETY: restores the prior value saved in `EnvGuard::set`.
            Some(v) => unsafe { std::env::set_var(self.0, v) },
            // SAFETY: removes the variable that was absent before `EnvGuard::set`.
            None => unsafe { std::env::remove_var(self.0) },
        }
    }
}

/// One combined env-mutating test: the named account's argv carries its
/// `CLAUDE_CONFIG_DIR`, a quoted sign-in command parses into words, and
/// `Usage` reads a temp home.
#[cfg(unix)]
#[test]
fn named_account_launches_and_sign_in_and_usage_read() {
    let home = TempDir::new("rune-acct-home");
    let bin = TempDir::new("rune-acct-bin");
    let goko = TempDir::new("rune-acct-goko");
    let codex_home = TempDir::new("rune-acct-codex");
    for name in ["claude", "codex", "devin", "opencode", "agy"] {
        let path = bin.path().join(name);
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    // The fake agents come first, but the rest of the real PATH
    // stays: a parallel test that spawns `git` must still find it.
    let path = {
        let mut paths = vec![bin.path().to_path_buf()];
        if let Some(existing) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&existing));
        }
        std::env::join_paths(paths).expect("a PATH to join")
    };
    let _guards = [
        EnvGuard::set("HOME", home.path().as_os_str()),
        EnvGuard::set("PATH", path.as_os_str()),
        EnvGuard::set("GOKO_HOME", goko.path().as_os_str()),
        EnvGuard::set("CODEX_HOME", codex_home.path().as_os_str()),
        EnvGuard::set("CLAUDE_CONFIG_DIR", home.path().join(".claude").as_os_str()),
    ];
    let storage = Some(Storage::open(goko.path()));

    // A named account with a space in its path: sign_in_launch shell-
    // quotes it, `shell_words` must hand it back as ONE argv element.
    let acct_dir = home.path().join("acc with space");
    let reply = run_job(
        &storage,
        &Job::project(Req::AccountAdd {
            agent: "claude".into(),
            name: "work".into(),
            dir: acct_dir.display().to_string(),
            token: 1,
        }),
    );
    let view = accounts_of(&reply);
    let id = view
        .claude
        .items
        .iter()
        .find(|item| !item.is_default)
        .unwrap()
        .id
        .clone();

    let reply = run_job(
        &storage,
        &Job::project(Req::AgentArgv {
            agent: "claude".into(),
            token: 1,
        }),
    );
    let Ok(ReplyData::AgentArgv(argv)) = &reply.result else {
        panic!("expected AgentArgv, got {:?}", reply.result);
    };
    assert_eq!(argv[0], "/usr/bin/env");
    assert!(
        argv.iter()
            .any(|arg| arg == &format!("CLAUDE_CONFIG_DIR={}", acct_dir.display())),
        "named account's env folder: {argv:?}"
    );

    // The sign-in session argv: the quoted folder is one word.
    let reply = run_job(
        &storage,
        &Job::project(Req::AccountSignIn {
            agent: "claude".into(),
            id: id.clone(),
        }),
    );
    let Ok(ReplyData::SignIn { argv, cwd, title }) = &reply.result else {
        panic!("expected SignIn, got {:?}", reply.result);
    };
    assert!(
        argv.iter()
            .any(|arg| arg == &format!("CLAUDE_CONFIG_DIR={}", acct_dir.display())),
        "the quoted path must survive as one word: {argv:?}"
    );
    assert_eq!(cwd, &acct_dir.display().to_string());
    assert!(!title.is_empty());

    // A `Usage` read: the accounts view comes with the snapshot, and
    // the named account's usage folder may be created under the temp
    // GOKO_HOME — never the real one.
    let reply = run_job(
        &storage,
        &Job::project(Req::Usage {
            previous: app_core::usage::UsageSnapshot::default(),
        }),
    );
    let Ok(ReplyData::Usage { accounts, .. }) = &reply.result else {
        panic!("expected Usage, got {:?}", reply.result);
    };
    assert_eq!(accounts.claude.items.len(), 2);

    handoff_backend_launches(&storage, home.path(), bin.path(), &id, &acct_dir);

    if app_core::accounts::DEVIN_ACCOUNTS_SUPPORTED {
        let folder = home.path().join("Devin Work");
        let agent = "devin".to_owned();
        let added = run_job(
            &storage,
            &Job::project(Req::AccountAdd {
                agent: agent.clone(),
                name: "Work".into(),
                dir: folder.display().to_string(),
                token: 2,
            }),
        );
        let id = accounts_of(&added).devin.active.clone();
        let launch = Job::project(Req::AgentArgv {
            agent: agent.clone(),
            token: 3,
        });
        let Ok(ReplyData::AgentArgv(argv)) = run_job(&storage, &launch).result else {
            panic!("expected the Devin preset to launch");
        };
        assert_eq!(argv[3], format!("XDG_DATA_HOME={}", folder.display()));
        let signed_in = run_job(
            &storage,
            &Job::project(Req::AccountSignIn {
                agent,
                id: id.clone(),
            }),
        );
        let Ok(ReplyData::SignIn {
            argv: login, cwd, ..
        }) = signed_in.result
        else {
            panic!("expected a Devin sign-in launch");
        };
        assert_eq!(&login[..argv.len()], argv);
        assert_eq!(cwd, folder.display().to_string());
        let (project_id, root) = handoff_project(&storage, home.path());
        let handoff = Job::project(Req::HandoffArgv {
            token: 9,
            agent: "devin".into(),
            account: id.clone(),
            project_id,
            checkout: root.display().to_string(),
            fork: None,
        });
        let Ok(ReplyData::HandoffArgv { argv: named, .. }) = run_job(&storage, &handoff).result
        else {
            panic!("expected the named Devin handoff to launch");
        };
        assert_eq!(
            named, argv,
            "a named Devin handoff keeps the isolated profile"
        );
        std::fs::rename(&folder, home.path().join("Devin Moved")).unwrap();
        assert!(run_job(&storage, &handoff).result.is_err());
        assert!(
            run_job(&storage, &launch)
                .result
                .unwrap_err()
                .contains("missing")
        );
    }
}

/// The registered Git checkout the handoff launches run in.
#[cfg(unix)]
fn handoff_project(storage: &Option<Storage>, home: &std::path::Path) -> (String, PathBuf) {
    let root = home.join("handoff-repo");
    if !root.exists() {
        std::fs::create_dir_all(&root).unwrap();
        let out = std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .arg(&root)
            .output()
            .unwrap();
        assert!(out.status.success());
    }
    let root = root.canonicalize().unwrap();
    let store = storage.as_ref().unwrap().get().unwrap();
    let project = app_core::projects::list_projects(store)
        .unwrap()
        .into_iter()
        .find(|project| project.project.root == root.display().to_string())
        .unwrap_or_else(|| {
            app_core::projects::add_project(
                store,
                root.to_str().unwrap(),
                app_core::store::DEFAULT_WORKSPACE_ID,
            )
            .unwrap()
        });
    (project.project.id, root)
}

/// Every preset launches as a handoff destination on Default, a named
/// Claude account carries its folder, the stored selection never moves,
/// and a fork is built only into a profile that shares the source history.
#[cfg(unix)]
fn handoff_backend_launches(
    storage: &Option<Storage>,
    home: &std::path::Path,
    bin: &std::path::Path,
    claude_id: &str,
    claude_dir: &std::path::Path,
) {
    let (project_id, root) = handoff_project(storage, home);
    let before = app_core::accounts::read(storage.as_ref().unwrap());
    let launch = |agent: &str, account: &str, fork: Option<app_core::handoff::ForkSource>| {
        run_job(
            storage,
            &Job::project(Req::HandoffArgv {
                token: 5,
                agent: agent.into(),
                account: account.into(),
                project_id: project_id.clone(),
                checkout: root.display().to_string(),
                fork,
            }),
        )
        .result
    };
    for agent in ["claude", "codex", "devin", "opencode", "agy"] {
        let Ok(ReplyData::HandoffArgv { argv, cwd }) = launch(agent, "default", None) else {
            panic!("{agent} launches on Default");
        };
        let binary = bin.canonicalize().unwrap().join(agent);
        assert!(
            argv.iter().any(
                |word| std::path::Path::new(word).canonicalize().ok().as_ref() == Some(&binary)
            ),
            "{argv:?}"
        );
        assert!(
            !argv
                .iter()
                .any(|word| word.contains("CONFIG_DIR=") || word.contains("CODEX_HOME="))
        );
        assert_eq!(cwd, root.display().to_string());
    }
    let Ok(ReplyData::HandoffArgv { argv, .. }) = launch("claude", claude_id, None) else {
        panic!("the named Claude account launches");
    };
    assert!(argv.contains(&format!("CLAUDE_CONFIG_DIR={}", claude_dir.display())));
    assert!(
        !argv.iter().any(|word| word == "--resume"),
        "context mode never resumes"
    );

    let id = "00000000-0000-0000-0000-0000000000cc";
    let file = home
        .join(".claude/projects/-repo")
        .join(format!("{id}.jsonl"));
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "{}\n").unwrap();
    let source = app_core::handoff::ForkSource {
        id: id.into(),
        file: file.clone(),
    };
    let Ok(ReplyData::HandoffArgv { argv, .. }) = launch("claude", "default", Some(source.clone()))
    else {
        panic!("Default shares its own history");
    };
    let session = argv.iter().position(|word| word == "--session-id").unwrap();
    assert_ne!(
        argv[session + 1],
        id,
        "the fork gets a fresh conversation ID"
    );
    assert_eq!(&argv[argv.len() - 3..], ["--resume", id, "--fork-session"]);
    assert!(!argv.iter().any(|word| word.ends_with(".jsonl")));
    let error = launch("claude", claude_id, Some(source.clone())).unwrap_err();
    assert!(error.contains("does not share"), "{error}");
    std::os::unix::fs::symlink(home.join(".claude/projects"), claude_dir.join("projects")).unwrap();
    assert!(launch("claude", claude_id, Some(source)).is_ok());
    let after = app_core::accounts::read(storage.as_ref().unwrap());
    assert_eq!(after.claude.active, before.claude.active);
    assert_eq!(after.codex.active, before.codex.active);
    assert_eq!(after.devin.active, before.devin.active);
}

#[test]
fn sign_in_for_an_unknown_account_fails() {
    let (dir, storage) = temp_storage("rune-signin-miss");
    let reply = run_job(
        &storage,
        &Job::project(Req::AccountSignIn {
            agent: "claude".into(),
            id: "nonexistent".into(),
        }),
    );
    assert!(reply.result.is_err());
    let _ = dir;
}

/// The shutdown drain class: account mutations are writes, the rest
/// are reads.
#[test]
fn account_and_usage_reqs_have_their_drain_side() {
    for req in [
        Req::AccountAdd {
            agent: "a".into(),
            name: "n".into(),
            dir: "d".into(),
            token: 1,
        },
        Req::AccountRename {
            agent: "a".into(),
            id: "i".into(),
            name: "n".into(),
            token: 1,
        },
        Req::AccountRemove {
            agent: "a".into(),
            id: "i".into(),
        },
        Req::AccountSetActive {
            agent: "a".into(),
            id: "i".into(),
        },
    ] {
        assert!(is_mutating(&req), "{req:?} must finish on shutdown");
    }
    for req in [
        Req::Usage {
            previous: app_core::usage::UsageSnapshot::default(),
        },
        Req::AccountSignIn {
            agent: "a".into(),
            id: "i".into(),
        },
    ] {
        assert!(!is_mutating(&req), "{req:?} must be skippable on shutdown");
    }
}

#[test]
fn git_writes_and_detachable_jobs_are_partitioned() {
    assert!(detachable(&Req::Push));
    assert!(detachable(&Req::Commit {
        message: "msg".into(),
    }));
    // Operations can run the repo's hooks, so they detach like Commit;
    // a whole-file resolve is an ordinary quick write.
    assert!(detachable(&Req::Operation {
        step: "continue".into(),
    }));
    assert!(!detachable(&Req::Changes));
    assert!(!detachable(&Req::Stage { paths: None }));
    assert!(!detachable(&Req::Resolve {
        path: "a".into(),
        action: "current".into(),
    }));

    for req in [
        Req::Stage { paths: None },
        Req::Unstage { paths: None },
        Req::Discard {
            layer: "unstaged".into(),
            path: "a".into(),
        },
        Req::StageResolved {
            path: "a".into(),
            expected: "base".into(),
            allow_markers: false,
        },
        Req::Resolve {
            path: "a".into(),
            action: "current".into(),
        },
        Req::Operation {
            step: "abort".into(),
        },
        Req::Commit {
            message: "m".into(),
        },
        Req::Push,
    ] {
        assert!(
            is_git_write(&req),
            "{req:?} must be recognized as a git write"
        );
        assert!(is_mutating(&req), "{req:?} must finish on shutdown");
    }
    for req in [Req::Changes, Req::FileList, Req::Projects, Req::Workspaces] {
        assert!(!is_git_write(&req), "{req:?} is not a git write");
    }
}

fn handoff_session(
    argv: &[&str],
    state: protocol::session::SessionState,
) -> protocol::session::SessionInfo {
    protocol::session::SessionInfo {
        id: protocol::session::SessionId::new(),
        title: "agent".into(),
        argv: argv.iter().map(|word| word.to_string()).collect(),
        cwd: "/".into(),
        state,
        pid: None,
        exit_code: None,
        error: None,
        started_at: 0,
        ended_at: None,
    }
}

fn prepared(reply: Reply) -> app_core::handoff::Prepared {
    match reply.result {
        Ok(ReplyData::HandoffPrepared { prepared, .. }) => *prepared,
        other => panic!("expected a handoff review, got {other:?}"),
    }
}

/// The review reads the exited pane's recorded conversation from its own
/// profile, adds the checkout's Git picture, and never sends a draft back.
#[test]
fn handoff_backend_prepares_the_exited_panes_own_conversation() {
    use protocol::session::SessionState;
    let (dir, root, storage, project_id) = concept_storage("rune-handoff-prepare");
    let profile = dir.path().join("claude work");
    let id = "00000000-0000-0000-0000-0000000000bb";
    let transcript = profile.join("projects/-work").join(format!("{id}.jsonl"));
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    let cwd = root.display().to_string();
    std::fs::write(
        &transcript,
        format!(
            "{}\n{}\n",
            serde_json::json!({"type":"user","uuid":"u1","cwd":cwd,"message":{"content":"Finish the parser"}}),
            serde_json::json!({"type":"assistant","uuid":"a1","parentUuid":"u1","message":{"content":[{"type":"text","text":"Half done"}]}}),
        ),
    )
    .unwrap();
    let env_word = format!("CLAUDE_CONFIG_DIR={}", profile.display());
    let job = |session, manual_id: Option<&str>| {
        Job::project(Req::PrepareHandoff {
            token: 7,
            session,
            agent: "claude".into(),
            project_id: project_id.clone(),
            checkout: cwd.clone(),
            manual_id: manual_id.map(str::to_owned),
        })
    };
    let claude = handoff_session(
        &["/usr/bin/env", &env_word, "/bin/claude", "--session-id", id],
        SessionState::Exited,
    );
    let reply = run_job(&storage, &job(claude.clone(), None));
    assert!(matches!(reply.req, Req::PrepareHandoff { token: 7, .. }));
    let review = prepared(reply);
    assert_eq!(review.conversation.as_deref(), Some(id));
    assert!(review.status.contains("Launch ID"), "{}", review.status);
    for expected in [
        "User:\nFinish the parser",
        "Assistant:\nHalf done",
        "Branch: main at",
    ] {
        assert!(
            review.draft.contains(expected),
            "{expected}: {}",
            review.draft
        );
    }
    assert!(review.fork.is_some() || cfg!(windows));

    // An exited pane started without an ID has no conversation, but a
    // context-only review; a typed ID is verified in the pane's profile.
    let bare = handoff_session(
        &["/usr/bin/env", &env_word, "/bin/claude"],
        SessionState::Exited,
    );
    let review = prepared(run_job(&storage, &job(bare.clone(), None)));
    assert!(
        review.status.contains("without a conversation ID"),
        "{}",
        review.status
    );
    assert!(review.conversation.is_none() && review.accepts_manual_id);
    assert!(review.draft.contains("Branch: main at"));
    let review = prepared(run_job(&storage, &job(bare, Some(id))));
    assert_eq!(review.conversation.as_deref(), Some(id));

    // An unregistered checkout is refused before any history is read.
    let reply = run_job(
        &storage,
        &Job::project(Req::PrepareHandoff {
            token: 8,
            session: claude,
            agent: "claude".into(),
            project_id: project_id.clone(),
            checkout: dir.path().join("elsewhere").display().to_string(),
            manual_id: None,
        }),
    );
    assert!(reply.result.is_err());
}

/// A destination is exactly the chosen account: an unknown one, a named
/// account for an agent without accounts, or a fork into another agent is
/// an error, never a fallback.
#[test]
fn handoff_backend_refuses_unknown_accounts_and_foreign_forks() {
    let (_dir, root, storage, project_id) = concept_storage("rune-handoff-argv");
    let job = |agent: &str, account: &str, fork: bool| {
        Job::project(Req::HandoffArgv {
            token: 3,
            agent: agent.into(),
            account: account.into(),
            project_id: project_id.clone(),
            checkout: root.display().to_string(),
            fork: fork.then(|| app_core::handoff::ForkSource {
                id: "abc".into(),
                file: root.join("abc.jsonl"),
            }),
        })
    };
    for (agent, account) in [("claude", "ghost"), ("opencode", "work"), ("agy", "work")] {
        let reply = run_job(&storage, &job(agent, account, false));
        assert!(reply.result.is_err(), "{agent}/{account}");
    }
    let error = run_job(&storage, &job("codex", "default", true))
        .result
        .unwrap_err();
    assert!(error.contains("only a Claude destination"), "{error}");
    let reply = run_job(
        &storage,
        &Job::project(Req::HandoffArgv {
            token: 4,
            agent: "shell".into(),
            account: "default".into(),
            project_id: "missing".into(),
            checkout: root.display().to_string(),
            fork: None,
        }),
    );
    assert!(reply.result.is_err(), "an unregistered project is refused");
}
