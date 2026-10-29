use daemon::pty::{PtyError, PtyProcess, SpawnSpec};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[tokio::test]
async fn spawn_captures_output_and_exit_code() {
    let process = PtyProcess::spawn(spec(
        &["sh", "-c", "echo hi; exit 7"],
        std::env::current_dir().expect("current directory"),
    ))
    .expect("spawn command");

    let status = process.wait_exit().await.expect("wait for command");
    assert_eq!(status.code, 7);
    assert_eq!(status.signal, None);

    let (_, bytes, _) = wait_for_output(&process, b"hi");
    assert!(bytes.windows(2).any(|window| window == b"hi"));
}

#[test]
fn missing_working_directory_returns_spawn_error() {
    let temp = TempDir::new();
    let missing = temp.path().join("missing");

    let error = spawn_error(PtyProcess::spawn(spec(&["sh"], missing)));
    assert!(matches!(error, PtyError::SpawnFailed(_)));
    assert!(error.to_string().contains("working directory"));
}

#[test]
fn file_as_working_directory_returns_spawn_error() {
    let temp = TempDir::new();
    let file = temp.path().join("not-a-directory");
    fs::write(&file, b"test").expect("create file");

    let error = spawn_error(PtyProcess::spawn(spec(&["sh"], file)));
    assert!(matches!(error, PtyError::SpawnFailed(_)));
    assert!(error.to_string().contains("not a directory"));
}

#[test]
fn nonexistent_executable_returns_spawn_error() {
    let executable = format!("/definitely-not-a-goko-command-{}", std::process::id());
    let error = spawn_error(PtyProcess::spawn(spec(
        &[executable.as_str()],
        std::env::current_dir().expect("current directory"),
    )));

    assert!(matches!(error, PtyError::SpawnFailed(_)));
    assert!(error.to_string().contains(&executable));
}

#[test]
fn empty_argv_returns_spawn_error() {
    let error = spawn_error(PtyProcess::spawn(spec(
        &[],
        std::env::current_dir().expect("current directory"),
    )));

    assert!(matches!(error, PtyError::SpawnFailed(_)));
    assert!(error.to_string().contains("argv"));
}

#[test]
fn spawn_rejects_invalid_terminal_dimensions() {
    let mut spawn_spec = spec(&["sh"], std::env::current_dir().expect("current directory"));
    spawn_spec.cols = 0;

    let error = spawn_error(PtyProcess::spawn(spawn_spec));
    assert!(matches!(error, PtyError::InvalidArgument(_)));
}

fn spec(argv: &[&str], cwd: PathBuf) -> SpawnSpec {
    SpawnSpec {
        argv: argv.iter().map(|arg| (*arg).to_owned()).collect(),
        cwd,
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

fn spawn_error(result: Result<PtyProcess, PtyError>) -> PtyError {
    match result {
        Ok(_) => panic!("PTY spawn unexpectedly succeeded"),
        Err(error) => error,
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("gokod-pty-spawn-{}-{id}", std::process::id()));
        fs::create_dir_all(&path).expect("create temporary directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
