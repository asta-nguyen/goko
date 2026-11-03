use daemon::lock::{InstanceLock, LockError};
use daemon::paths::Paths;
use daemon::session::SessionManager;
use daemon::{auth, server};
use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use tokio::signal::unix::{signal, SignalKind};

#[tokio::main]
async fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    if first.is_none() {
        return match run().await {
            Ok(code) => code,
            Err(error) => {
                eprintln!("gokod: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if first.as_deref() == Some(OsStr::new("--version")) && args.next().is_none() {
        println!("gokod {}", env!("CARGO_PKG_VERSION"));
        ExitCode::SUCCESS
    } else {
        eprintln!("usage: gokod [--version]");
        ExitCode::from(2)
    }
}

async fn run() -> io::Result<ExitCode> {
    let paths = Paths::resolve()?;
    let _lock = match InstanceLock::acquire(&paths) {
        Ok(lock) => lock,
        Err(LockError::AlreadyRunning) => {
            println!("gokod: already running");
            return Ok(ExitCode::SUCCESS);
        }
        Err(LockError::Io(error)) => return Err(error),
    };

    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(paths.log())?;
    tracing_subscriber::fmt()
        .with_writer(Mutex::new(log))
        .with_ansi(false)
        .init();

    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let token = auth::create_token(&paths)?;
    let listener = server::bind(&paths)?;
    tracing::info!(
        "gokod {} listening on {}",
        env!("CARGO_PKG_VERSION"),
        paths.socket().display()
    );

    let sessions = Arc::new(SessionManager::new());
    server::serve(
        listener,
        token,
        Arc::clone(&sessions),
        paths.worktree_operations_lock(),
        async {
            tokio::select! {
                _ = terminate.recv() => {}
                _ = interrupt.recv() => {}
            }
        },
    )
    .await?;

    tracing::info!("shutting down");
    sessions.shutdown().await;
    // Best effort: the daemon is exiting either way.
    let _ = std::fs::remove_file(paths.socket());
    tracing::info!("gokod stopped");
    Ok(ExitCode::SUCCESS)
}
