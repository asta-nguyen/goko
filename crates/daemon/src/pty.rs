use crate::buffer::RingBuffer;
use portable_pty::{native_pty_system, Child, CommandBuilder, PtySize};
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use tokio::sync::watch;

const MAX_DIMENSION: u16 = 1000;

#[derive(Debug, Clone)]
pub enum PtyError {
    SpawnFailed(String),
    InvalidArgument(String),
    Io(String),
}

impl fmt::Display for PtyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SpawnFailed(message) => write!(formatter, "PTY spawn failed: {message}"),
            Self::InvalidArgument(message) => write!(formatter, "invalid PTY argument: {message}"),
            Self::Io(message) => write!(formatter, "PTY I/O error: {message}"),
        }
    }
}

impl Error for PtyError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitStatus {
    pub code: u32,
    pub signal: Option<String>,
}

impl From<portable_pty::ExitStatus> for ExitStatus {
    fn from(status: portable_pty::ExitStatus) -> Self {
        Self {
            code: status.exit_code(),
            signal: status.signal().map(str::to_owned),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub cols: u16,
    pub rows: u16,
}

type ExitState = Option<Result<ExitStatus, PtyError>>;
type PtyReader = Box<dyn Read + Send>;
type PtyWriter = Box<dyn Write + Send>;

pub struct PtyProcess {
    pid: libc::pid_t,
    master: Mutex<Box<dyn portable_pty::MasterPty + Send>>,
    writer: Mutex<PtyWriter>,
    buffer: Arc<Mutex<RingBuffer>>,
    exit_state: watch::Receiver<ExitState>,
}

impl PtyProcess {
    pub fn spawn(spec: SpawnSpec) -> Result<Self, PtyError> {
        if spec.argv.is_empty() || spec.argv[0].is_empty() {
            return Err(PtyError::SpawnFailed(
                "argv must contain a non-empty executable path".to_owned(),
            ));
        }
        validate_size(spec.cols, spec.rows)?;
        validate_cwd(&spec.cwd)?;

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: spec.rows,
                cols: spec.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| PtyError::SpawnFailed(format!("could not open PTY: {error:#}")))?;
        let reader = pair.master.try_clone_reader().map_err(|error| {
            PtyError::SpawnFailed(format!("could not create PTY output reader: {error:#}"))
        })?;
        let writer = pair.master.take_writer().map_err(|error| {
            PtyError::SpawnFailed(format!("could not create PTY input writer: {error:#}"))
        })?;

        let argv = spec.argv.into_iter().map(Into::into).collect();
        let mut command = CommandBuilder::from_argv(argv);
        command.cwd(spec.cwd.as_os_str());
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");

        let portable_pty::PtyPair { slave, master } = pair;
        let mut child = slave.spawn_command(command).map_err(|error| {
            PtyError::SpawnFailed(format!("could not start command: {error:#}"))
        })?;
        drop(slave);

        // portable-pty starts the child with `setsid`, so its pid is also its process-group id.
        let Some(pid) = child
            .process_id()
            .and_then(|pid| libc::pid_t::try_from(pid).ok())
        else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(PtyError::SpawnFailed(
                "PTY backend did not provide a child process id".to_owned(),
            ));
        };

        let buffer = Arc::new(Mutex::new(RingBuffer::new()));
        if let Err(error) = spawn_reader(reader, Arc::clone(&buffer)) {
            let _ = signal_group(pid, libc::SIGKILL);
            let _ = child.wait();
            return Err(PtyError::Io(format!(
                "could not start PTY output reader: {error}"
            )));
        }

        let (exit_sender, exit_state) = watch::channel(None);
        if let Err(error) = spawn_waiter(child, exit_sender) {
            let _ = signal_group(pid, libc::SIGKILL);
            return Err(PtyError::Io(format!(
                "could not start PTY exit waiter: {error}"
            )));
        }

        Ok(Self {
            pid,
            master: Mutex::new(master),
            writer: Mutex::new(writer),
            buffer,
            exit_state,
        })
    }

    /// Write terminal input to the PTY and flush it to the child.
    pub fn write(&self, bytes: &[u8]) -> Result<(), PtyError> {
        if self.exit_state.borrow().is_some() {
            return Err(PtyError::Io("cannot write to an exited process".to_owned()));
        }

        let mut writer = lock_unpoisoned(&self.writer);
        writer
            .write_all(bytes)
            .and_then(|()| writer.flush())
            .map_err(|error| PtyError::Io(format!("could not write to PTY: {error}")))
    }

    /// Resize the PTY window after checking the supported dimension range.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), PtyError> {
        validate_size(cols, rows)?;
        if self.exit_state.borrow().is_some() {
            return Err(PtyError::Io("cannot resize an exited process".to_owned()));
        }

        lock_unpoisoned(&self.master)
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| PtyError::Io(format!("could not resize PTY: {error:#}")))
    }

    /// Send SIGTERM to the child process group.
    pub fn terminate(&self) -> Result<(), PtyError> {
        self.signal_process_group(libc::SIGTERM)
    }

    /// Send SIGKILL to the child process group.
    pub fn kill(&self) -> Result<(), PtyError> {
        self.signal_process_group(libc::SIGKILL)
    }

    /// Return the process identifier of the PTY child.
    pub fn pid(&self) -> u32 {
        self.pid as u32
    }

    /// Return the current replay window as (oldest offset, bytes, next offset).
    pub fn buffer(&self) -> (u64, Vec<u8>, u64) {
        lock_unpoisoned(&self.buffer).snapshot()
    }

    /// Wait for the child process to exit. Multiple callers can await the same status.
    pub async fn wait_exit(&self) -> Result<ExitStatus, PtyError> {
        let mut exit_state = self.exit_state.clone();
        loop {
            let recorded = { (*exit_state.borrow_and_update()).clone() };
            if let Some(status) = recorded {
                return status;
            }
            exit_state.changed().await.map_err(|_| {
                PtyError::Io("PTY exit waiter stopped before recording a status".to_owned())
            })?;
        }
    }

    fn signal_process_group(&self, signal: i32) -> Result<(), PtyError> {
        if self.exit_state.borrow().is_some() {
            return Ok(());
        }

        signal_group(self.pid, signal).map_err(|error| {
            PtyError::Io(format!(
                "could not signal process group {}: {error}",
                self.pid
            ))
        })
    }
}

fn validate_cwd(cwd: &std::path::Path) -> Result<(), PtyError> {
    match fs::metadata(cwd) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(PtyError::SpawnFailed(format!(
            "working directory is not a directory: {}",
            cwd.display()
        ))),
        Err(error) => Err(PtyError::SpawnFailed(format!(
            "could not access working directory {}: {error}",
            cwd.display()
        ))),
    }
}

fn validate_size(cols: u16, rows: u16) -> Result<(), PtyError> {
    if !(1..=MAX_DIMENSION).contains(&cols) || !(1..=MAX_DIMENSION).contains(&rows) {
        return Err(PtyError::InvalidArgument(format!(
            "PTY dimensions must be between 1 and {MAX_DIMENSION} columns and rows"
        )));
    }
    Ok(())
}

fn spawn_reader(
    mut reader: PtyReader,
    buffer: Arc<Mutex<RingBuffer>>,
) -> std::io::Result<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("gokod-pty-reader".to_owned())
        .spawn(move || {
            let mut chunk = [0_u8; 16 * 1024];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        lock_unpoisoned(&buffer).append(&chunk[..count]);
                    }
                }
            }
        })
}

fn spawn_waiter(
    child: Box<dyn Child + Send + Sync>,
    exit_sender: watch::Sender<ExitState>,
) -> std::io::Result<thread::JoinHandle<()>> {
    let child_slot = Arc::new(Mutex::new(Some(child)));
    let thread_child_slot = Arc::clone(&child_slot);
    let result = thread::Builder::new()
        .name("gokod-pty-waiter".to_owned())
        .spawn(move || {
            let mut child = lock_unpoisoned(&thread_child_slot)
                .take()
                .expect("PTY waiter owns the child");
            let result = child
                .wait()
                .map(ExitStatus::from)
                .map_err(|error| PtyError::Io(format!("could not wait for PTY child: {error}")));
            exit_sender.send_replace(Some(result));
        });

    if result.is_err() {
        let mut child = lock_unpoisoned(&child_slot)
            .take()
            .expect("PTY child remains available when waiter creation fails");
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn signal_group(process_group_id: libc::pid_t, signal: i32) -> std::io::Result<()> {
    // SAFETY: `killpg` takes plain integers and has no memory-safety preconditions.
    if unsafe { libc::killpg(process_group_id, signal) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}
