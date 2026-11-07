//! The commands through Tauri's real IPC path, called the way the webview's services call
//! them, so argument names and the JSON shapes are the contract the frontend relies on.

use super::*;
use crate::store::Storage;
use crate::test_support::Home;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::process::Command;
use tauri::ipc::{CallbackFn, InvokeBody, InvokeResponseBody};
use tauri::test::{get_ipc_response, mock_builder, mock_context, noop_assets, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::{WebviewWindow, WebviewWindowBuilder};

fn webview_with(
    storage: Storage,
    home: &Home,
) -> (
    tauri::App<tauri::test::MockRuntime>,
    WebviewWindow<tauri::test::MockRuntime>,
) {
    let app = mock_builder()
        .manage(Bridge::new(home.path().to_owned()))
        .manage(storage)
        .manage(crate::acp::AcpHub::new())
        .manage(crate::usage::UsageState::default())
        .manage(crate::activity::ActivityState::default())
        .invoke_handler(handler())
        .build(mock_context(noop_assets()))
        .unwrap();
    let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    (app, webview)
}

fn webview(
    home: &Home,
) -> (
    tauri::App<tauri::test::MockRuntime>,
    WebviewWindow<tauri::test::MockRuntime>,
) {
    webview_with(Storage::open(home.path()), home)
}

/// `invoke(cmd, args)`: the resolved value, or the rejection the webview would see.
fn invoke(
    webview: &WebviewWindow<tauri::test::MockRuntime>,
    cmd: &str,
    args: Value,
) -> Result<Value, Value> {
    let request = InvokeRequest {
        cmd: cmd.into(),
        callback: CallbackFn(0),
        error: CallbackFn(1),
        url: "tauri://localhost".parse().unwrap(),
        body: InvokeBody::Json(args),
        headers: Default::default(),
        invoke_key: INVOKE_KEY.to_string(),
    };
    get_ipc_response(webview, request).map(|body| match body {
        InvokeResponseBody::Json(json) => serde_json::from_str(&json).unwrap(),
        InvokeResponseBody::Raw(_) => panic!("expected JSON"),
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn commands_take_and_return_what_the_frontend_services_send_and_expect() {
    let home = Home::new("wire");
    let (_app, webview) = webview(&home);

    let status = invoke(&webview, "daemon_status", json!({})).unwrap();
    assert_eq!(status["connected"], false);
    assert_eq!(status["home"], json!(home.path().display().to_string()));
    assert!(status["default_shell"]
        .as_str()
        .is_some_and(|shell| !shell.is_empty()));

    let usage = invoke(&webview, "usage_get", json!({})).unwrap();
    assert_eq!(usage["claude"]["five_hour"], Value::Null);
    assert_eq!(usage["claude"]["sessions"], json!({}));
    assert_eq!(usage["codex"]["primary"], Value::Null);

    assert_eq!(
        invoke(&webview, "activity_get", json!({})).unwrap(),
        json!({})
    );

    // `title` omitted, as `sessionService.create` does when the caller gives none.
    let session = invoke(
        &webview,
        "session_create",
        json!({"command": "cat", "cwd": "/tmp", "cols": 80, "rows": 24}),
    )
    .unwrap();
    for key in [
        "id",
        "title",
        "argv",
        "cwd",
        "state",
        "exit_code",
        "error",
        "started_at",
        "ended_at",
    ] {
        assert!(
            session.get(key).is_some(),
            "SessionInfo lacks {key}: {session}"
        );
    }
    let id = session["id"].clone();

    let listed = invoke(&webview, "session_list", json!({})).unwrap();
    assert_eq!(
        listed.as_array().unwrap()[0]["id"],
        id,
        "a bare array of sessions"
    );

    assert_eq!(
        invoke(&webview, "terminal_write", json!({"id": id, "data": "x\n"})),
        Ok(Value::Null)
    );
    assert_eq!(
        invoke(
            &webview,
            "terminal_resize",
            json!({"id": id, "cols": 100, "rows": 30})
        ),
        Ok(Value::Null)
    );
    // `sessionService.subscribe` passes its `Channel` as `onOutput`.
    let subscribe = json!({"id": id, "onOutput": "__CHANNEL__:1"});
    assert_eq!(
        invoke(&webview, "terminal_subscribe", subscribe),
        Ok(Value::Null)
    );
    assert_eq!(
        invoke(&webview, "terminal_unsubscribe", json!({"id": id})),
        Ok(Value::Null)
    );
    assert_eq!(
        invoke(&webview, "session_stop", json!({"id": id, "force": true})),
        Ok(Value::Null)
    );
    assert_eq!(
        invoke(&webview, "daemon_status", json!({})).unwrap()["connected"],
        true
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn session_delete_uses_the_frontend_shape_and_removes_metadata_after_exit() {
    let home = Home::new("wire-delete");
    let (_app, webview) = webview(&home);
    let session = invoke(
        &webview,
        "session_create",
        json!({"command": "sh -c 'exit 0'", "cwd": "/tmp", "cols": 80, "rows": 24}),
    )
    .unwrap();
    let id = session["id"].clone();

    for _ in 0..100 {
        let listed = invoke(&webview, "session_list", json!({})).unwrap();
        if listed.as_array().unwrap()[0]["state"] == "exited" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        invoke(&webview, "session_list", json!({}))
            .unwrap()
            .as_array()
            .unwrap()[0]["state"],
        "exited"
    );
    assert_eq!(
        invoke(&webview, "session_delete", json!({"id": id})),
        Ok(Value::Null)
    );
    assert!(invoke(&webview, "session_list", json!({}))
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
    assert!(invoke(&webview, "session_meta_list", json!({}))
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_s2_commands_take_and_return_the_frontend_shapes() {
    let home = Home::new("wire-s2");
    let (_app, webview) = webview(&home);

    // `projectService.add` sends the picked directory as `path`.
    let project = invoke(
        &webview,
        "project_add",
        json!({"path": home.path().display().to_string()}),
    )
    .unwrap();
    for key in ["id", "name", "root", "created_at", "missing"] {
        assert!(project.get(key).is_some(), "Project lacks {key}: {project}");
    }
    assert_eq!(project["missing"], false);
    let project_id = project["id"].clone();

    let worktrees = invoke(
        &webview,
        "worktree_list",
        json!({"projectId": project_id.clone()}),
    )
    .unwrap();
    assert_eq!(worktrees["repo"], false);
    assert_eq!(worktrees["worktrees"], json!([]));
    let branches = invoke(
        &webview,
        "worktree_branches",
        json!({"projectId": project_id.clone()}),
    )
    .unwrap();
    assert_eq!(branches["branches"], json!([]));
    assert_eq!(branches["bases"], json!([]));

    // Re-adding the same canonical root returns the existing project.
    let again = invoke(
        &webview,
        "project_add",
        json!({"path": home.path().join(".").display().to_string()}),
    )
    .unwrap();
    assert_eq!(again["id"], project_id);

    let listed = invoke(&webview, "project_list", json!({})).unwrap();
    assert_eq!(listed.as_array().unwrap()[0]["id"], project_id);

    // `agentService.list` is always a bare array; `shell` is always available.
    let agents = invoke(&webview, "agent_list", json!({})).unwrap();
    let agents = agents.as_array().unwrap();
    let shell = agents.iter().find(|a| a["id"] == "shell").unwrap();
    assert_eq!(shell["available"], true);
    assert!(shell["path"].is_string());
    for agent in agents {
        for key in ["id", "label", "available", "path"] {
            assert!(agent.get(key).is_some(), "AgentInfo lacks {key}: {agent}");
        }
    }

    // An agent launch under a project checkout: `agent`, `project_id`, and `checkout`
    // (camelCase on the wire), no `command` or `cwd`.
    let session = invoke(
        &webview,
        "session_create",
        json!({
            "agent": "shell",
            "projectId": project_id.clone(),
            "checkout": home.path().display().to_string(),
            "cols": 80,
            "rows": 24
        }),
    )
    .unwrap();
    assert_eq!(
        session["cwd"],
        std::fs::canonicalize(home.path())
            .unwrap()
            .display()
            .to_string()
    );
    let session_id = session["id"].clone();
    invoke(
        &webview,
        "session_stop",
        json!({"id": session_id, "force": false}),
    )
    .unwrap();

    // `sessionMeta` rows carry the join fields the sidebar needs.
    let metas = invoke(&webview, "session_meta_list", json!({})).unwrap();
    let meta = metas
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["session_id"] == session_id)
        .expect("the session's metadata was recorded");
    assert_eq!(meta["project_id"], project_id);
    assert_eq!(meta["agent"], "shell");
    for key in ["argv", "cwd", "title", "created_at"] {
        assert!(meta.get(key).is_some(), "SessionMeta lacks {key}: {meta}");
    }

    // `settingsService` rounds out the persisted keys (`last_agent` came from the launch).
    assert_eq!(
        invoke(&webview, "settings_get", json!({"key": "last_agent"})).unwrap(),
        json!("shell")
    );
    assert_eq!(
        invoke(
            &webview,
            "settings_set",
            json!({"key": "active_project_id", "value": project_id})
        ),
        Ok(Value::Null)
    );
    assert_eq!(
        invoke(
            &webview,
            "settings_get",
            json!({"key": "active_project_id"})
        )
        .unwrap(),
        project_id
    );
    assert_eq!(
        invoke(&webview, "settings_get", json!({"key": "unset"})).unwrap(),
        Value::Null
    );

    // Rejections carry their codes.
    assert_eq!(
        invoke(&webview, "project_remove", json!({"id": "not-a-project"})).unwrap_err()["code"],
        "not_found"
    );
    assert_eq!(
        invoke(&webview, "project_remove", json!({"id": project_id})),
        Ok(Value::Null)
    );
    assert_eq!(
        invoke(&webview, "project_add", json!({"path": "/no/such/dir"})).unwrap_err()["code"],
        "invalid_argument"
    );
    assert_eq!(
        invoke(
            &webview,
            "session_create",
            json!({"command": "cat", "cwd": "/tmp", "projectId": "p", "cols": 80, "rows": 24})
        )
        .unwrap_err()["code"],
        "invalid_argument"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_broken_store_rejects_storage_commands_while_the_daemon_answers() {
    let home = Home::new("wire-nostore");
    // `goko.db` is a directory, so the store cannot open; the error stays managed.
    std::fs::create_dir_all(home.path().join("goko.db")).unwrap();
    let (_app, webview) = webview_with(Storage::open(home.path()), &home);

    for cmd in ["project_list", "session_meta_list"] {
        assert_eq!(
            invoke(&webview, cmd, json!({})).unwrap_err()["code"],
            "storage",
            "{cmd} should reject with the storage code"
        );
    }
    for (cmd, args) in [
        ("project_add", json!({"path": "/tmp"})),
        ("project_remove", json!({"id": "x"})),
        ("settings_get", json!({"key": "k"})),
        ("settings_set", json!({"key": "k", "value": "v"})),
    ] {
        assert_eq!(
            invoke(&webview, cmd, args).unwrap_err()["code"],
            "storage",
            "{cmd} should reject with the storage code"
        );
    }
    // A project_id cannot be resolved without the store; a cwd create still works.
    assert_eq!(
        invoke(
            &webview,
            "session_create",
            json!({"command": "cat", "projectId": "p", "cols": 80, "rows": 24})
        )
        .unwrap_err()["code"],
        "storage"
    );
    let session = invoke(
        &webview,
        "session_create",
        json!({"command": "cat", "cwd": "/tmp", "cols": 80, "rows": 24}),
    )
    .unwrap();
    invoke(
        &webview,
        "session_stop",
        json!({"id": session["id"], "force": false}),
    )
    .unwrap();
    assert_eq!(
        invoke(&webview, "daemon_status", json!({})).unwrap()["connected"],
        true
    );
    assert_eq!(
        invoke(&webview, "agent_list", json!({}))
            .unwrap()
            .as_array()
            .unwrap()[0]["id"],
        "shell"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn failures_reject_with_code_and_message() {
    let home = Home::new("wire-err");
    let (_app, webview) = webview(&home);

    let rejection = invoke(
        &webview,
        "session_create",
        json!({"command": "echo 'x", "cwd": "/tmp", "cols": 80, "rows": 24}),
    )
    .unwrap_err();

    assert_eq!(rejection["code"], "invalid_argument");
    assert!(rejection["message"].is_string());
}

#[tokio::test(flavor = "multi_thread")]
async fn worktree_and_environment_commands_match_the_frontend_wire_shapes() {
    let home = Home::new("wire-worktree");
    let (_app, webview) = webview(&home);
    let root = home.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    git_ok(&root, &["init", "-b", "main"]);
    git_ok(&root, &["config", "user.name", "Goko Test"]);
    git_ok(&root, &["config", "user.email", "goko@example.invalid"]);
    fs::write(root.join("tracked.txt"), "tracked\n").unwrap();
    fs::write(root.join(".gitignore"), ".env\n").unwrap();
    git_ok(&root, &["add", "."]);
    git_ok(&root, &["commit", "-m", "initial"]);
    fs::write(root.join(".env"), "SECRET=wire\n").unwrap();
    let root = fs::canonicalize(&root).unwrap();
    let project = invoke(
        &webview,
        "project_add",
        json!({"path": root.display().to_string()}),
    )
    .unwrap();
    let project_id = project["id"].as_str().unwrap();

    let listed = invoke(&webview, "worktree_list", json!({"projectId": project_id})).unwrap();
    assert_eq!(listed["repo"], true);
    let root_row = &listed["worktrees"][0];
    for key in [
        "name", "path", "branch", "head", "is_root", "missing", "dirty", "changed",
    ] {
        assert!(root_row.get(key).is_some(), "missing {key}: {root_row}");
    }
    let branch_data = invoke(
        &webview,
        "worktree_branches",
        json!({"projectId": project_id}),
    )
    .unwrap();
    assert!(branch_data["branches"].is_array());
    assert!(branch_data["bases"].is_array());

    let target = home.path().join("wire-checkout");
    let created = invoke(
        &webview,
        "worktree_create",
        json!({
            "projectId": project_id,
            "path": target.display().to_string(),
            "mode": "new",
            "branch": "feature/wire",
            "base": "main"
        }),
    )
    .unwrap();
    assert_eq!(created["branch"], "feature/wire");

    let candidates = invoke(
        &webview,
        "env_file_candidates",
        json!({"projectId": project_id, "sourcePath": root.display().to_string()}),
    )
    .unwrap();
    assert_eq!(candidates[0]["name"], ".env");
    assert_eq!(candidates[0]["eligible"], true);
    assert!(candidates[0].get("reason").is_some());
    let copied = invoke(
        &webview,
        "env_copy",
        json!({
            "projectId": project_id,
            "sourcePath": root.display().to_string(),
            "targetPath": target.display().to_string()
        }),
    )
    .unwrap();
    assert_eq!(copied["copied"], json!([".env"]));
    assert_eq!(
        fs::read_to_string(target.join(".env")).unwrap(),
        "SECRET=wire\n"
    );

    assert_eq!(
        invoke(
            &webview,
            "worktree_remove",
            json!({"projectId": project_id, "path": target.display().to_string()}),
        ),
        Ok(Value::Null)
    );
    assert!(!Path::new(&target).exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn git_view_commands_match_the_frontend_wire_shapes() {
    let home = Home::new("wire-gitview");
    let (_app, webview) = webview(&home);
    let root = home.path().join("gv");
    fs::create_dir_all(&root).unwrap();
    git_ok(&root, &["init", "-b", "main"]);
    git_ok(&root, &["config", "user.name", "Goko Test"]);
    git_ok(&root, &["config", "user.email", "goko@example.invalid"]);
    fs::write(root.join("tracked.txt"), "one\n").unwrap();
    fs::create_dir_all(root.join("dir")).unwrap();
    fs::write(root.join("dir/nested.txt"), "nested\n").unwrap();
    git_ok(&root, &["add", "."]);
    git_ok(&root, &["commit", "-qm", "initial"]);
    fs::write(root.join("tracked.txt"), "one\ntwo\n").unwrap();
    fs::write(root.join("loose.txt"), "loose\n").unwrap();
    git_ok(&root, &["stash", "push", "-u", "-m", "wire stash"]);
    fs::write(root.join("tracked.txt"), "one\nunstaged\n").unwrap();
    let root = fs::canonicalize(&root).unwrap();
    let project = invoke(
        &webview,
        "project_add",
        json!({"path": root.display().to_string()}),
    )
    .unwrap();
    let project_id = project["id"].as_str().unwrap();
    let checkout = json!({
        "projectId": project_id,
        "checkoutPath": root.display().to_string()
    });

    let changes = invoke(&webview, "git_view_changes", checkout.clone()).unwrap();
    for key in [
        "branch",
        "head",
        "conflicts",
        "staged",
        "unstaged",
        "untracked",
        "ahead",
    ] {
        assert!(
            changes.get(key).is_some(),
            "GitViewChanges lacks {key}: {changes}"
        );
    }
    assert_eq!(changes["branch"], "main");
    // No upstream yet: `ahead` is present but null, as `number | null` allows.
    assert_eq!(changes["ahead"], Value::Null);
    let entry = &changes["unstaged"][0];
    for key in ["path", "orig_path", "status"] {
        assert!(entry.get(key).is_some(), "StatusEntry lacks {key}: {entry}");
    }

    let diff = invoke(
        &webview,
        "git_view_diff",
        json!({
            "projectId": project_id,
            "checkoutPath": root.display().to_string(),
            "layer": "unstaged",
            "path": "tracked.txt"
        }),
    )
    .unwrap();
    for key in ["label", "patch", "truncated", "files"] {
        assert!(diff.get(key).is_some(), "GitViewDiff lacks {key}: {diff}");
    }
    assert!(diff["patch"].as_str().unwrap().contains("+unstaged"));

    let history = invoke(
        &webview,
        "git_view_history",
        json!({
            "projectId": project_id,
            "checkoutPath": root.display().to_string(),
            "skip": 0
        }),
    )
    .unwrap();
    let commit = &history["commits"][0];
    for key in ["id", "parents", "refs", "head", "author", "time", "subject"] {
        assert!(
            commit.get(key).is_some(),
            "GitViewCommit lacks {key}: {commit}"
        );
    }
    assert!(history.get("next_skip").is_some());
    let oid = commit["id"].as_str().unwrap().to_owned();
    let commit_diff = invoke(
        &webview,
        "git_view_diff",
        json!({
            "projectId": project_id,
            "checkoutPath": root.display().to_string(),
            "layer": "commit",
            "commit": oid
        }),
    )
    .unwrap();
    assert_eq!(commit_diff["files"][0]["status"], "added");

    let tree = invoke(
        &webview,
        "git_view_tree",
        json!({
            "projectId": project_id,
            "checkoutPath": root.display().to_string(),
            "directory": ""
        }),
    )
    .unwrap();
    let dir = &tree["entries"][0];
    for key in ["path", "name", "kind", "ignored"] {
        assert!(
            dir.get(key).is_some(),
            "GitViewTreeEntry lacks {key}: {dir}"
        );
    }

    let file = invoke(
        &webview,
        "git_view_file",
        json!({
            "projectId": project_id,
            "checkoutPath": root.display().to_string(),
            "path": "dir/nested.txt"
        }),
    )
    .unwrap();
    for key in ["path", "kind", "text", "truncated", "size"] {
        assert!(file.get(key).is_some(), "GitViewFile lacks {key}: {file}");
    }
    assert_eq!(file["kind"], "text");

    let stashes = invoke(&webview, "git_view_stashes", checkout.clone()).unwrap();
    let stash = &stashes.as_array().unwrap()[0];
    for key in ["index", "id", "message", "branch", "time"] {
        assert!(
            stash.get(key).is_some(),
            "GitViewStash lacks {key}: {stash}"
        );
    }
    let stash_diff = invoke(
        &webview,
        "git_view_stash_diff",
        json!({
            "projectId": project_id,
            "checkoutPath": root.display().to_string(),
            "stashId": stash["id"]
        }),
    )
    .unwrap();
    assert_eq!(stash_diff["label"], "stash@{0}");
    assert!(stash_diff["patch"].as_str().unwrap().contains("+loose"));

    // `gitWriteService`: stage the unstaged change, then commit it.
    assert_eq!(
        invoke(
            &webview,
            "git_write_stage",
            json!({
                "projectId": project_id,
                "checkoutPath": root.display().to_string(),
                "paths": ["tracked.txt"]
            }),
        ),
        Ok(Value::Null)
    );
    let staged = invoke(&webview, "git_view_changes", checkout.clone()).unwrap();
    assert_eq!(staged["staged"][0]["path"], "tracked.txt");

    let commit = invoke(
        &webview,
        "git_write_commit",
        json!({
            "projectId": project_id,
            "checkoutPath": root.display().to_string(),
            "message": "wire commit"
        }),
    )
    .unwrap();
    for key in ["head", "branch"] {
        assert!(
            commit.get(key).is_some(),
            "GitCommitResult lacks {key}: {commit}"
        );
    }
    assert_eq!(commit["branch"], "main");

    // Stage all / unstage all take `paths: null`; discard restores the file.
    fs::write(root.join("tracked.txt"), "one\nunstaged\nagain\n").unwrap();
    for (cmd, args) in [
        (
            "git_write_stage",
            json!({
                "projectId": project_id,
                "checkoutPath": root.display().to_string(),
                "paths": null
            }),
        ),
        (
            "git_write_unstage",
            json!({
                "projectId": project_id,
                "checkoutPath": root.display().to_string(),
                "paths": null
            }),
        ),
        (
            "git_write_discard",
            json!({
                "projectId": project_id,
                "checkoutPath": root.display().to_string(),
                "layer": "unstaged",
                "path": "tracked.txt"
            }),
        ),
    ] {
        assert_eq!(invoke(&webview, cmd, args), Ok(Value::Null), "{cmd}");
    }
    fs::write(root.join("loose2.txt"), "loose\n").unwrap();
    assert_eq!(
        invoke(
            &webview,
            "git_write_discard",
            json!({
                "projectId": project_id,
                "checkoutPath": root.display().to_string(),
                "layer": "untracked",
                "path": "loose2.txt"
            }),
        ),
        Ok(Value::Null)
    );
    assert!(!root.join("loose2.txt").exists());
    let cleaned = invoke(&webview, "git_view_changes", checkout.clone()).unwrap();
    assert!(cleaned["staged"].as_array().unwrap().is_empty());
    assert!(cleaned["unstaged"].as_array().unwrap().is_empty());

    // Push against a local bare remote: no upstream yet, so `-u origin HEAD`.
    let remote = home.path().join("gv-remote");
    fs::create_dir_all(&remote).unwrap();
    git_ok(&remote, &["init", "--bare", "-b", "main"]);
    git_ok(
        &root,
        &[
            "remote",
            "add",
            "origin",
            remote.display().to_string().as_str(),
        ],
    );
    let push = invoke(
        &webview,
        "git_write_push",
        json!({
            "projectId": project_id,
            "checkoutPath": root.display().to_string()
        }),
    )
    .unwrap();
    assert!(
        push.get("summary").is_some(),
        "GitPushResult lacks summary: {push}"
    );
    // The new upstream makes `ahead` a number: 0 unpushed commits.
    let pushed = invoke(&webview, "git_view_changes", checkout.clone()).unwrap();
    assert_eq!(pushed["ahead"], json!(0));

    // Rejections: unregistered project, non-Git project, non-checkout path,
    // escaping paths.
    assert_eq!(
        invoke(
            &webview,
            "git_view_changes",
            json!({"projectId": "nope", "checkoutPath": root.display().to_string()})
        )
        .unwrap_err()["code"],
        "not_found"
    );
    let plain_dir = home.path().join("plain");
    fs::create_dir_all(&plain_dir).unwrap();
    let plain = invoke(
        &webview,
        "project_add",
        json!({"path": plain_dir.display().to_string()}),
    )
    .unwrap();
    assert_eq!(
        invoke(
            &webview,
            "git_view_changes",
            json!({
                "projectId": plain["id"],
                "checkoutPath": fs::canonicalize(&plain_dir).unwrap().display().to_string()
            })
        )
        .unwrap_err()["code"],
        "invalid_argument"
    );
    assert_eq!(
        invoke(
            &webview,
            "git_view_changes",
            json!({"projectId": project_id, "checkoutPath": home.path().display().to_string()})
        )
        .unwrap_err()["code"],
        "invalid_argument"
    );
    assert_eq!(
        invoke(
            &webview,
            "git_view_file",
            json!({
                "projectId": project_id,
                "checkoutPath": root.display().to_string(),
                "path": "../outside"
            })
        )
        .unwrap_err()["code"],
        "invalid_argument"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn acp_commands_match_the_frontend_wire_shapes() {
    let home = Home::new("wire-acp");
    let (_app, webview) = webview(&home);

    let agents = invoke(&webview, "acp_agents_get", json!({})).unwrap();
    let agents = agents.as_array().unwrap();
    assert_eq!(agents.len(), 5);
    let ids: Vec<&str> = agents.iter().map(|a| a["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["devin", "opencode", "claude", "codex", "gemini"]);
    for agent in agents {
        for key in [
            "id",
            "label",
            "enabled",
            "command",
            "prompt",
            "available",
            "is_default",
        ] {
            assert!(agent.get(key).is_some(), "AcpAgent lacks {key}: {agent}");
        }
    }

    let status = invoke(&webview, "acp_status", json!({})).unwrap();
    let status = status.as_object().unwrap();
    assert_eq!(status.len(), 5);
    for id in ["devin", "opencode", "claude", "codex", "gemini"] {
        let slot = &status[id];
        assert!(slot.get("state").is_some(), "{id} lacks state: {slot}");
        assert!(slot.get("message").is_some(), "{id} lacks message: {slot}");
        assert_eq!(slot["state"], "disconnected");
        assert_eq!(slot["message"], Value::Null);
    }
}

fn git_ok(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
