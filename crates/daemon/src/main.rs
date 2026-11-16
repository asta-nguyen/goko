use daemon::lock::{InstanceLock, LockError};
use daemon::paths::Paths;
use daemon::session::SessionManager;
use daemon::{auth, server};
use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long exiting waits for blocking work that is still running before the
/// process ends anyway, so no stuck thread can keep a stopped daemon alive.
const RUNTIME_SHUTDOWN: Duration = Duration::from_secs(2);

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    if first.is_none() {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                eprintln!("gokod: cannot start the runtime: {error}");
                return ExitCode::FAILURE;
            }
        };
        let code = match runtime.block_on(run()) {
            Ok(code) => code,
            Err(error) => {
                eprintln!("gokod: {error}");
                ExitCode::FAILURE
            }
        };
        runtime.shutdown_timeout(RUNTIME_SHUTDOWN);
        return code;
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

    let mut log_options = OpenOptions::new();
    log_options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        log_options.mode(0o600);
    }
    let log = log_options.open(paths.log())?;
    tracing_subscriber::fmt()
        .with_writer(Mutex::new(log))
        .with_ansi(false)
        .init();

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::unbounded_channel();
    let token = auth::create_token(&paths)?;
    let listener = match server::bind(&paths) {
        Ok(listener) => listener,
        Err(error)
            if error.kind() == io::ErrorKind::AddrInUse || error.raw_os_error() == Some(231) =>
        {
            // Windows: a live daemon owns the pipe; Unix reaches this through
            // the instance lock above.
            println!("gokod: already running");
            return Ok(ExitCode::SUCCESS);
        }
        Err(error) => return Err(error),
    };
    tracing::info!(
        "gokod {} listening on {}",
        env!("CARGO_PKG_VERSION"),
        paths.endpoint()
    );

    let sessions = Arc::new(SessionManager::new());
    server::serve(
        listener,
        token,
        Arc::clone(&sessions),
        paths.worktree_operations_lock(),
        shutdown_tx,
        async {
            tokio::select! {
                () = shutdown_signal() => {}
                _ = shutdown_rx.recv() => {}
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

/// SIGTERM/SIGINT on Unix; console close/ctrl-c on Windows. Either ends the
/// same graceful shutdown the socket request runs.
#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut terminate = match signal(SignalKind::terminate()) {
        Ok(signal) => signal,
        Err(_) => return std::future::pending().await,
    };
    let mut interrupt = match signal(SignalKind::interrupt()) {
        Ok(signal) => signal,
        Err(_) => return std::future::pending().await,
    };
    tokio::select! {
        _ = terminate.recv() => {}
        _ = interrupt.recv() => {}
    }
}

#[cfg(windows)]
async fn shutdown_signal() {
    let mut close = match tokio::signal::windows::ctrl_close() {
        Ok(signal) => signal,
        Err(_) => return std::future::pending().await,
    };
    let mut ctrl_c = match tokio::signal::windows::ctrl_c() {
        Ok(signal) => signal,
        Err(_) => return std::future::pending().await,
    };
    tokio::select! {
        _ = close.recv() => {}
        _ = ctrl_c.recv() => {}
    }
}
