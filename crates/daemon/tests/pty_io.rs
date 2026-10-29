use daemon::pty::{PtyError, PtyProcess, SpawnSpec};
use std::thread;
use std::time::Duration;

#[tokio::test]
async fn write_to_cat_is_recorded_in_the_output_buffer() {
    let process = PtyProcess::spawn(spec(&["cat"])).expect("spawn cat");

    process.write(b"hello\n").expect("write to cat");
    let (_, bytes, _) = wait_for_output(&process, b"hello");
    assert!(bytes.windows(5).any(|window| window == b"hello"));

    process.terminate().expect("terminate cat");
    process.wait_exit().await.expect("wait for cat");
}

#[tokio::test]
async fn resize_updates_the_child_terminal_dimensions() {
    let process =
        PtyProcess::spawn(spec(&["sh", "-c", "read ready; stty size"])).expect("spawn shell");

    process.resize(100, 30).expect("resize PTY");
    process.write(b"go\n").expect("release shell read");
    let (_, bytes, _) = wait_for_output(&process, b"30 100");
    assert!(bytes.windows(6).any(|window| window == b"30 100"));
    process.wait_exit().await.expect("wait for shell");
}

#[tokio::test]
async fn resize_rejects_zero_and_oversized_dimensions() {
    let process = PtyProcess::spawn(spec(&["sleep", "30"])).expect("spawn sleep");

    assert!(matches!(
        process.resize(0, 0),
        Err(PtyError::InvalidArgument(_))
    ));
    assert!(matches!(
        process.resize(1001, 10),
        Err(PtyError::InvalidArgument(_))
    ));

    process.kill().expect("kill sleep");
    process.wait_exit().await.expect("wait for sleep");
}

#[tokio::test]
async fn write_after_exit_returns_io_error() {
    let process = PtyProcess::spawn(spec(&["true"])).expect("spawn true");
    process.wait_exit().await.expect("wait for true");

    assert!(matches!(process.write(b"late input"), Err(PtyError::Io(_))));
}

#[tokio::test]
async fn output_buffer_keeps_non_utf8_bytes() {
    let process = PtyProcess::spawn(spec(&["sh", "-c", r"printf '\377'"]))
        .expect("spawn byte-producing shell");
    process.wait_exit().await.expect("wait for shell");

    let (_, bytes, _) = wait_for_output(&process, &[0xff]);
    assert!(bytes.contains(&0xff));
}

fn spec(argv: &[&str]) -> SpawnSpec {
    SpawnSpec {
        argv: argv.iter().map(|arg| (*arg).to_owned()).collect(),
        cwd: std::env::current_dir().expect("current directory"),
        cols: 80,
        rows: 24,
    }
}

fn wait_for_output(process: &PtyProcess, needle: &[u8]) -> (u64, Vec<u8>, u64) {
    for _ in 0..200 {
        let snapshot = process.buffer();
        if snapshot
            .1
            .windows(needle.len())
            .any(|window| window == needle)
        {
            return snapshot;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "PTY output did not contain {:?}",
        String::from_utf8_lossy(needle)
    );
}
