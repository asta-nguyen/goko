//! The whole daemon, as a client sees it: the real `gokod` in a temp `GOKO_HOME`.

mod common;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use common::{connect, process_exists, Client, Daemon, Incoming, TempDir};
use daemon::paths::Paths;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

fn create(script: &str) -> Value {
    json!({
        "type": "session.create",
        "argv": ["sh", "-c", script],
        "cwd": std::env::current_dir().unwrap().to_string_lossy(),
        "cols": 80,
        "rows": 24,
        "title": "",
    })
}

/// Read output frames until the text so far satisfies `done`; returns that text.
async fn read_until(client: &mut Client, done: impl Fn(&str) -> bool) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut text = String::new();
    while !done(&text) {
        assert!(
            Instant::now() < deadline,
            "timed out; output so far {text:?}"
        );
        if let Some(Incoming::Output { bytes, .. }) =
            client.next_within(Duration::from_millis(100)).await
        {
            text.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
    text
}

/// The `pid=N` line a session prints first, once it is complete.
fn pid_in(text: &str) -> Option<u32> {
    let rest = text.split("pid=").nth(1)?;
    let (digits, _) = rest.split_once('\r')?;
    digits.parse().ok()
}

/// Start a session that prints its pid, watched by its own connection.
async fn start(paths: &Paths, control: &mut Client, script: &str) -> (Value, Client, u32) {
    let created = control.request(create(script)).await;
    assert_eq!(created["result"]["state"], "running", "{created}");
    let id = created["result"]["id"].clone();
    let mut watcher = connect(paths).await;
    watcher
        .request(json!({"type": "terminal.subscribe", "session_id": id}))
        .await;
    let text = read_until(&mut watcher, |text| pid_in(text).is_some()).await;
    (id, watcher, pid_in(&text).unwrap())
}

async fn wait_for_exit(control: &mut Client, id: &Value) -> Value {
    for _ in 0..500 {
        let info = control
            .request(json!({"type": "session.get", "session_id": id}))
            .await;
        if info["result"]["state"] == "exited" {
            return info["result"].clone();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("session never exited");
}

#[tokio::test]
async fn sessions_run_stop_and_die_with_the_daemon() {
    let home = TempDir::new("e2e");
    let paths = Paths::at(home.path()).unwrap();
    let mut daemon = Daemon::start(&home);
    let mut control = connect(&paths).await;

    // Two concurrent sessions: `cat` and a long-running shell.
    let (cat_id, mut cat, cat_pid) = start(&paths, &mut control, "echo pid=$$; exec cat").await;
    let (sh_id, _sh, sh_pid) = start(&paths, &mut control, "echo pid=$$; sleep 30").await;
    assert_ne!(cat_pid, sh_pid);

    // Input reaches `cat` and its echo comes back.
    let write = json!({
        "type": "terminal.write",
        "session_id": cat_id,
        "data_b64": STANDARD.encode("ping\n"),
    });
    control.request(write).await;
    read_until(&mut cat, |text| text.contains("ping")).await;

    // Stop one gracefully, force-kill the other.
    control
        .request(json!({"type": "session.stop", "session_id": cat_id, "force": false}))
        .await;
    control
        .request(json!({"type": "session.stop", "session_id": sh_id, "force": true}))
        .await;
    wait_for_exit(&mut control, &cat_id).await;
    wait_for_exit(&mut control, &sh_id).await;
    assert!(!process_exists(cat_pid) && !process_exists(sh_pid));

    // A third session is still running when the daemon is told to stop. It ignores SIGHUP,
    // which closing the PTY would otherwise send, so only the daemon's own shutdown ends it.
    let script = "trap '' HUP; echo pid=$$; exec sleep 30";
    let (_, _third, third_pid) = start(&paths, &mut control, script).await;
    assert!(process_exists(third_pid));

    assert_eq!(unsafe { libc::kill(daemon.pid(), libc::SIGTERM) }, 0);
    let status = daemon
        .exited_within(Duration::from_secs(5))
        .await
        .expect("gokod should exit after SIGTERM");

    assert!(status.success());
    assert!(
        !paths.socket().exists(),
        "the socket file should be removed"
    );
    for _ in 0..100 {
        if !process_exists(third_pid) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        !process_exists(third_pid),
        "a session process outlived the daemon"
    );
}
