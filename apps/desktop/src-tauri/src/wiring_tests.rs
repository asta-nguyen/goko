//! The commands through Tauri's real IPC path, called the way the webview's services call
//! them, so argument names and the JSON shapes are the contract the frontend relies on.

use super::*;
use crate::test_support::Home;
use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody, InvokeResponseBody};
use tauri::test::{get_ipc_response, mock_builder, mock_context, noop_assets, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::{WebviewWindow, WebviewWindowBuilder};

fn webview(
    home: &Home,
) -> (
    tauri::App<tauri::test::MockRuntime>,
    WebviewWindow<tauri::test::MockRuntime>,
) {
    let app = mock_builder()
        .manage(Bridge::new(home.path().to_owned()))
        .invoke_handler(handler())
        .build(mock_context(noop_assets()))
        .unwrap();
    let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    (app, webview)
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
