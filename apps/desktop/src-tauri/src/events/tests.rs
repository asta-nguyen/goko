use super::*;
use crate::store::Storage;
use crate::test_support::{create_args, Home};
use protocol::session::SessionId;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use tokio::time::timeout;

type Seen = UnboundedReceiver<(String, Value)>;

/// Runs the pump against `home` and records what it emits. Dropping it stops the pump,
/// which must happen before the test's `Home` kills the daemon, or the pump would start
/// a new one.
struct Pump {
    bridge: Bridge,
    seen: Seen,
    task: tokio::task::JoinHandle<()>,
}

impl Pump {
    fn start(home: &Home) -> Self {
        let bridge = Bridge::new(home.path().to_owned());
        let (tx, seen) = unbounded_channel();
        let emit: Emit = Arc::new(move |name, payload| {
            let _ = tx.send((name.to_owned(), payload));
        });
        let task = tokio::spawn(run_pump(bridge.clone(), emit));
        Self { bridge, seen, task }
    }

    /// The next emitted event named `name` that satisfies `wanted`; others are skipped.
    async fn next(&mut self, name: &str, wanted: impl Fn(&Value) -> bool) -> Value {
        timeout(Duration::from_secs(15), async {
            loop {
                let (seen, payload) = self.seen.recv().await.expect("the pump stopped");
                if seen == name && wanted(&payload) {
                    return payload;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("no {name} event arrived"))
    }

    async fn connected(&mut self, connected: bool) {
        self.next("daemon://status", |status| status["connected"] == connected)
            .await;
    }
}

impl Drop for Pump {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn is(id: SessionId, state: &'static str) -> impl Fn(&Value) -> bool {
    move |session| session["id"] == json!(id) && session["state"] == state
}

#[tokio::test]
async fn a_state_change_arrives_as_updated_and_an_exit_also_as_exited() {
    let home = Home::new("ev-flow");
    let mut pump = Pump::start(&home);
    pump.connected(true).await;

    let session = pump
        .bridge
        .session_create(&Storage::open(home.path()), create_args("sleep 30", "/tmp"))
        .await
        .unwrap();
    let updated = pump
        .next("session://updated", is(session.id, "running"))
        .await;
    // The payload is the whole session, as the webview's `SessionInfo`.
    assert_eq!(updated["argv"], json!(["sleep", "30"]));
    assert_eq!(updated["exit_code"], Value::Null);

    pump.bridge.session_stop(session.id, true).await.unwrap();
    pump.next("session://updated", is(session.id, "exited"))
        .await;
    let exited = pump
        .next("session://exited", is(session.id, "exited"))
        .await;
    assert_eq!(exited["id"], json!(session.id));
    assert!(exited["ended_at"].is_i64());
}

#[tokio::test]
async fn losing_the_daemon_reports_it_then_restarts_it_and_events_resume() {
    let home = Home::new("ev-loss");
    let mut pump = Pump::start(&home);
    pump.connected(true).await;
    assert_eq!(home.spawns().len(), 1);

    home.signal_daemons(libc::SIGKILL);
    pump.connected(false).await;
    pump.connected(true).await;
    assert_eq!(home.spawns().len(), 2, "a new daemon was started");

    let session = pump
        .bridge
        .session_create(&Storage::open(home.path()), create_args("sleep 30", "/tmp"))
        .await
        .unwrap();
    pump.next("session://updated", is(session.id, "running"))
        .await;
}

#[tokio::test]
async fn a_daemons_own_exited_event_is_not_forwarded_twice() {
    let home = Home::new("ev-once");
    let mut pump = Pump::start(&home);
    pump.connected(true).await;

    let session = pump
        .bridge
        .session_create(&Storage::open(home.path()), create_args("true", "/tmp"))
        .await
        .unwrap();
    pump.next("session://exited", is(session.id, "exited"))
        .await;

    // One `session://exited` per exit: nothing more arrives for this session.
    tokio::time::sleep(Duration::from_millis(300)).await;
    while let Ok((name, payload)) = pump.seen.try_recv() {
        assert!(
            !(name == "session://exited" && payload["id"] == json!(session.id)),
            "exited was emitted twice"
        );
    }
}
