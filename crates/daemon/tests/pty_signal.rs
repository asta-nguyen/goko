use daemon::pty::{PtyProcess, SpawnSpec};
use std::thread;
use std::time::Duration;

#[tokio::test]
async fn terminate_sends_sigterm_to_the_child_group() {
    let process = PtyProcess::spawn(spec(&["sleep", "30"])).expect("spawn sleep");

    process.terminate().expect("send SIGTERM");
    let status = process.wait_exit().await.expect("wait for SIGTERM");
    assert!(status.signal.is_some());
}

#[tokio::test]
async fn kill_stops_a_child_that_ignores_sigterm() {
    let process = PtyProcess::spawn(spec(&["sh", "-c", "trap '' TERM; echo ready; sleep 30"]))
        .expect("spawn shell");
    wait_for_output(&process, b"ready");

    process.terminate().expect("send SIGTERM");
    thread::sleep(Duration::from_millis(100));
    assert!(process_exists(process.pid()), "shell should ignore SIGTERM");

    process.kill().expect("send SIGKILL");
    let status = process.wait_exit().await.expect("wait for SIGKILL");
    assert!(status.signal.is_some());
}

#[tokio::test]
async fn kill_stops_grandchildren_in_the_process_group() {
    let process = PtyProcess::spawn(spec(&["sh", "-c", "sleep 30 & echo $!; wait"]))
        .expect("spawn shell and grandchild");
    let (_, bytes, _) = wait_for_output(&process, b"\n");
    let child_pid = String::from_utf8_lossy(&bytes)
        .split(|character: char| !character.is_ascii_digit())
        .find_map(|part| part.parse::<u32>().ok())
        .expect("read grandchild pid");

    process.kill().expect("kill process group");
    process.wait_exit().await.expect("wait for shell");

    for _ in 0..200 {
        if !process_exists(child_pid) {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !process_exists(child_pid),
        "grandchild must be gone after group kill"
    );
}

#[tokio::test]
async fn signals_on_an_exited_process_are_idempotent() {
    let process = PtyProcess::spawn(spec(&["true"])).expect("spawn true");
    process.wait_exit().await.expect("wait for true");

    assert!(process.terminate().is_ok());
    assert!(process.kill().is_ok());
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

fn process_exists(pid: u32) -> bool {
    if pid > i32::MAX as u32 {
        return false;
    }
    let result = unsafe { libc::kill(pid as i32, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}
