//! The commands through Tauri's real IPC path, called the way the webview's services call
//! them, so argument names and the JSON shapes are the contract the frontend relies on.

use super::*;
use crate::store::Storage;
use crate::test_support::Home;
use serde_json::{json, Value};
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

    // An agent launch under a project: `agent` + `project_id` (camelCase on the wire),
    // no `command` or `cwd`.
    let session = invoke(
        &webview,
        "session_create",
        json!({"agent": "shell", "projectId": project_id, "cols": 80, "rows": 24}),
    )
    .unwrap();
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
