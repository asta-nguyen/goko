mod common;

use common::{Incoming, TestServer};
use daemon::pty::{PtyProcess, SpawnSpec};
use protocol::frame::PAINT_SEQ;
use protocol::session::SessionId;
use serde_json::json;
use std::time::{Duration, Instant};

fn spec(argv: &[&str]) -> SpawnSpec {
    SpawnSpec {
        argv: argv.iter().map(|arg| (*arg).to_owned()).collect(),
        cwd: std::env::current_dir().expect("current directory"),
        cols: 80,
        rows: 24,
    }
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    String::from_utf8_lossy(haystack).contains(needle)
}

/// A line older than the raw replay buffer is gone from the replay but still on
/// the screen the paint rebuilds, while the child keeps running.
#[tokio::test(flavor = "multi_thread")]
async fn a_paint_keeps_what_the_raw_replay_lost() {
    // Redraw one screen row 200,000 times: more than 2 MiB of output that never
    // scrolls, so the sentinel on row 1 stays on screen.
    let script = "printf 'SENTINEL\\n\\n'; \
                  yes \"$(printf '\\033[3;1Hredraw')\" | head -n 200000; sleep 30";
    let process = PtyProcess::spawn(spec(&["sh", "-c", script])).expect("spawn");

    let deadline = Instant::now() + Duration::from_secs(20);
    while process.buffer().2 < 2_000_000 {
        assert!(Instant::now() < deadline, "the output never reached 2 MB");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let (replay_from, retained, _) = process.buffer();
    assert!(replay_from > 0, "the raw buffer evicted early output");
    assert!(!contains(&retained, "SENTINEL"), "the raw replay lost it");

    let (snapshot, paint, _receiver) = process.subscribe_paint(false);
    assert!(snapshot.bytes.is_empty(), "a paint replaces the raw replay");
    assert_eq!(snapshot.replay_from, snapshot.next_seq);
    assert!(contains(&paint, "SENTINEL"), "the paint keeps the screen");
    assert!(contains(&paint, "redraw"), "and what was drawn after it");
    process.kill().expect("the child was still alive to kill");
    process.wait_exit().await.expect("reap");
}

/// Over the socket: the reply says `paint`, the paint frames carry `PAINT_SEQ`
/// and come first, and live output continues at the reply's `next_seq`.
#[tokio::test]
async fn a_paint_subscribe_streams_the_paint_then_live_output() {
    let server = TestServer::start();
    let mut client = server.authenticated().await;
    let created = client
        .request(json!({
            "type": "session.create",
            "argv": ["sh", "-c", "echo before; sleep 0.6; echo after; sleep 1"],
            "cwd": std::env::current_dir().unwrap().to_string_lossy(),
            "cols": 80,
            "rows": 24,
            "title": "",
        }))
        .await;
    let id = created["result"]["id"].clone();
    let session: SessionId = serde_json::from_value(id.clone()).expect("a session id");
    let pty = server.sessions.pty(session).unwrap();
    while !contains(&pty.buffer().1, "before") {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let reply = client
        .request(json!({"type": "terminal.subscribe", "session_id": id, "paint": true}))
        .await;
    assert_eq!(reply["result"]["paint"], true);
    let next_seq = reply["result"]["next_seq"].as_u64().unwrap();
    assert_eq!(reply["result"]["replay_from"].as_u64().unwrap(), next_seq);

    let mut paint = Vec::new();
    let mut live = Vec::new();
    while let Some(frame) = client.next_within(Duration::from_millis(1500)).await {
        if let Incoming::Output { seq, bytes, .. } = frame {
            if seq == PAINT_SEQ {
                assert!(live.is_empty(), "a paint frame after live output");
                paint.extend(bytes);
            } else {
                assert_eq!(seq, next_seq + live.len() as u64, "live output continues");
                live.extend(bytes);
            }
        }
    }
    assert!(contains(&paint, "before"), "the paint shows the screen");
    assert!(!contains(&paint, "after"));
    assert!(contains(&live, "after"), "live output follows the paint");
}
