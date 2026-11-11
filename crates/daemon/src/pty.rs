use crate::buffer::RingBuffer;
use portable_pty::{native_pty_system, Child, CommandBuilder, PtySize};
use std::error::Error;
use std::ffi::CString;
use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};

const MAX_DIMENSION: u16 = 1000;
const DEFAULT_UTF8_LOCALE: &str = "en_US.UTF-8";
/// Chunks a subscriber may have queued before it is dropped as lagging.
const SUBSCRIBER_QUEUE: usize = 256;
/// Unacknowledged output bytes at which the reader pauses while a flow
/// subscriber is attached.
pub(crate) const FLOW_WINDOW: u64 = 256 * 1024;
/// Longest reader pause without the acknowledged offset moving before flow
/// subscribers are dropped through the lag path.
const FLOW_STALL: Duration = Duration::from_secs(5);
/// How often a paused reader re-checks the flow gate at the latest.
const FLOW_RECHECK: Duration = Duration::from_millis(250);

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

/// The replay window at the moment of subscribing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub replay_from: u64,
    pub bytes: Vec<u8>,
    pub next_seq: u64,
}

/// What a subscriber receives after its snapshot. `Lagged` and `Closed` are final.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamItem {
    /// Output starting at byte offset `seq`.
    Chunk { seq: u64, bytes: Arc<[u8]> },
    /// The subscriber's queue filled up and it was removed; subscribe again.
    Lagged,
    /// The process's output ended.
    Closed,
}

/// An attached subscriber. `flow` marks one whose unacknowledged output
/// gates the reader.
struct Subscriber {
    sender: mpsc::Sender<StreamItem>,
    flow: bool,
}

/// The replay buffer and its subscribers behind one lock, so a snapshot and a
/// registration are atomic with respect to the reader's append and fan-out.
struct Shared {
    buffer: RingBuffer,
    subscribers: Vec<Subscriber>,
    closed: bool,
    /// Highest output offset the session's consumer has acknowledged. It is
    /// session-scoped and never decreases.
    acknowledged: u64,
    /// Mirrors `acknowledged` so `stream_output` can wait for credit.
    acknowledged_watch: watch::Sender<u64>,
}

impl Shared {
    fn publish(&mut self, bytes: &[u8]) {
        let seq = self.buffer.append(bytes);
        if self.subscribers.is_empty() {
            return;
        }

        let bytes: Arc<[u8]> = Arc::from(bytes);
        self.subscribers.retain(|subscriber| {
            // Each queue has one slot beyond SUBSCRIBER_QUEUE, kept for the final item.
            if subscriber.sender.capacity() <= 1 {
                let _ = subscriber.sender.try_send(StreamItem::Lagged);
                return false;
            }
            subscriber
                .sender
                .try_send(StreamItem::Chunk {
                    seq,
                    bytes: Arc::clone(&bytes),
                })
                .is_ok()
        });
    }

    /// Whether the reader must pause: a live flow subscriber exists and the
    /// unacknowledged output has reached the window. A subscriber whose
    /// receiver is gone does not count.
    fn flow_blocked(&self) -> bool {
        self.buffer.next_seq().saturating_sub(self.acknowledged) >= FLOW_WINDOW
            && self
                .subscribers
                .iter()
                .any(|subscriber| subscriber.flow && !subscriber.sender.is_closed())
    }

    /// Raise the acknowledged offset toward `upto_seq`, capped at the
    /// published offset. It never decreases; returns whether it moved.
    fn raise_acknowledged(&mut self, upto_seq: u64) -> bool {
        let offset = upto_seq.min(self.buffer.next_seq());
        if offset <= self.acknowledged {
            return false;
        }
        self.acknowledged = offset;
        self.acknowledged_watch.send_replace(offset);
        true
    }

    /// Send `Lagged` to every flow subscriber and remove it, the same final
    /// notice a full queue produces.
    fn lag_flow_subscribers(&mut self) {
        self.subscribers.retain(|subscriber| {
            if subscriber.flow {
                let _ = subscriber.sender.try_send(StreamItem::Lagged);
            }
            !subscriber.flow
        });
    }

    fn close(&mut self) {
        self.closed = true;
        for subscriber in self.subscribers.drain(..) {
            let _ = subscriber.sender.try_send(StreamItem::Closed);
        }
    }
}

type ExitState = Option<Result<ExitStatus, PtyError>>;
type PtyReader = Box<dyn Read + Send>;
type PtyWriter = Box<dyn Write + Send>;

pub struct PtyProcess {
    pid: libc::pid_t,
    master: Mutex<Box<dyn portable_pty::MasterPty + Send>>,
    writer: Mutex<PtyWriter>,
    shared: Arc<Mutex<Shared>>,
    gate: Arc<Condvar>,
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
        let lc_all = std::env::var_os("LC_ALL").map(|value| value.to_string_lossy().into_owned());
        let lc_ctype =
            std::env::var_os("LC_CTYPE").map(|value| value.to_string_lossy().into_owned());
        let lang = std::env::var_os("LANG").map(|value| value.to_string_lossy().into_owned());
        let (locale, clear_lc_all) =
            resolve_utf8_locale(lc_all.as_deref(), lc_ctype.as_deref(), lang.as_deref());
        if clear_lc_all {
            command.env_remove("LC_ALL");
        }
        command.env("LC_CTYPE", &locale);
        command.env("LANG", &locale);

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

        let shared = Arc::new(Mutex::new(Shared {
            buffer: RingBuffer::new(),
            subscribers: Vec::new(),
            closed: false,
            acknowledged: 0,
            acknowledged_watch: watch::channel(0).0,
        }));
        let gate = Arc::new(Condvar::new());
        if let Err(error) = spawn_reader(reader, Arc::clone(&shared), Arc::clone(&gate)) {
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
            shared,
            gate,
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

    /// Send SIGHUP to the child process group: what a terminal sends when its
    /// window closes. Interactive shells ignore SIGTERM but exit on SIGHUP
    /// (and pass it on to their jobs), as do TUI agents.
    pub fn hangup(&self) -> Result<(), PtyError> {
        self.signal_process_group(libc::SIGHUP)
    }

    /// Send SIGKILL to the child process group.
    pub fn kill(&self) -> Result<(), PtyError> {
        self.signal_process_group(libc::SIGKILL)
    }

    /// Return the process identifier of the PTY child.
    pub fn pid(&self) -> u32 {
        self.pid as u32
    }

    /// Read the child process's current working directory when the platform exposes it.
    pub fn current_dir(&self) -> Option<PathBuf> {
        process_current_dir(self.pid).filter(|path| path.is_absolute())
    }

    /// Return the current replay window as (oldest offset, bytes, next offset).
    pub fn buffer(&self) -> (u64, Vec<u8>, u64) {
        lock_unpoisoned(&self.shared).buffer.snapshot()
    }

    /// Attach a subscriber. The snapshot and the registration happen under the lock the
    /// reader appends under, so every byte is either in the snapshot or delivered live
    /// as a `Chunk` whose `seq` continues from `snapshot.next_seq`. After the output has
    /// ended the receiver yields `Closed` straight away.
    pub fn subscribe(&self) -> (Snapshot, mpsc::Receiver<StreamItem>) {
        let mut shared = lock_unpoisoned(&self.shared);
        let (replay_from, bytes, next_seq) = shared.buffer.snapshot();
        let (sender, receiver) = mpsc::channel(SUBSCRIBER_QUEUE + 1);
        if shared.closed {
            let _ = sender.try_send(StreamItem::Closed);
        } else {
            shared.subscribers.push(Subscriber {
                sender,
                flow: false,
            });
        }
        let snapshot = Snapshot {
            replay_from,
            bytes,
            next_seq,
        };
        (snapshot, receiver)
    }

    /// Attach a flow-controlled subscriber: same snapshot contract as
    /// `subscribe`, but the reader pauses while unacknowledged output stays
    /// at or above `FLOW_WINDOW` and the subscriber is dropped through the
    /// `Lagged` path when no ack arrives for `FLOW_STALL`. The acknowledged
    /// offset is raised to the snapshot's `replay_from` when lower, so bytes
    /// already evicted are never treated as acknowledged; it never decreases
    /// and is never set to `next_seq`.
    pub fn subscribe_flow(&self) -> (Snapshot, mpsc::Receiver<StreamItem>) {
        let mut shared = lock_unpoisoned(&self.shared);
        let (replay_from, bytes, next_seq) = shared.buffer.snapshot();
        let raised = shared.raise_acknowledged(replay_from);
        let (sender, receiver) = mpsc::channel(SUBSCRIBER_QUEUE + 1);
        if shared.closed {
            let _ = sender.try_send(StreamItem::Closed);
        } else {
            shared.subscribers.push(Subscriber { sender, flow: true });
        }
        drop(shared);
        if raised {
            self.gate.notify_all();
        }
        (
            Snapshot {
                replay_from,
                bytes,
                next_seq,
            },
            receiver,
        )
    }

    /// Record that the consumer finished parsing output through `upto_seq`, an
    /// absolute end offset. The acknowledged offset never decreases and is
    /// capped at the published offset, and a raise wakes a reader paused on
    /// the window.
    pub fn ack(&self, upto_seq: u64) {
        let raised = lock_unpoisoned(&self.shared).raise_acknowledged(upto_seq);
        if raised {
            self.gate.notify_all();
        }
    }

    /// Observe the session's acknowledged offset, which never decreases.
    pub fn acknowledged_offset(&self) -> watch::Receiver<u64> {
        lock_unpoisoned(&self.shared).acknowledged_watch.subscribe()
    }

    /// Number of attached subscribers; one whose receiver was dropped leaves on the next
    /// output.
    pub fn subscriber_count(&self) -> usize {
        lock_unpoisoned(&self.shared).subscribers.len()
    }

    /// Stop every output subscription after the owning session has been deleted.
    pub fn close_subscribers(&self) {
        lock_unpoisoned(&self.shared).close();
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

fn resolve_utf8_locale(
    lc_all: Option<&str>,
    lc_ctype: Option<&str>,
    lang: Option<&str>,
) -> (String, bool) {
    let locale = [lc_all, lc_ctype, lang]
        .into_iter()
        .flatten()
        .find(|value| is_available_utf8_locale(value))
        .unwrap_or(DEFAULT_UTF8_LOCALE)
        .to_owned();
    let clear_lc_all = lc_all.is_some_and(|value| !is_available_utf8_locale(value));
    (locale, clear_lc_all)
}

fn is_utf8_locale(locale: &str) -> bool {
    let locale = locale.split('@').next().unwrap_or(locale);
    let encoding = locale
        .rsplit_once('.')
        .map_or(locale, |(_, encoding)| encoding);
    matches!(encoding.to_ascii_lowercase().as_str(), "utf8" | "utf-8")
}

fn is_available_utf8_locale(locale: &str) -> bool {
    if !is_utf8_locale(locale) {
        return false;
    }
    let Ok(name) = CString::new(locale) else {
        return false;
    };
    let resolved =
        unsafe { libc::newlocale(libc::LC_CTYPE_MASK, name.as_ptr(), std::ptr::null_mut()) };
    if resolved.is_null() {
        return false;
    }
    unsafe { libc::freelocale(resolved) };
    true
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

#[cfg(target_os = "macos")]
fn process_current_dir(pid: libc::pid_t) -> Option<PathBuf> {
    // SAFETY: proc_vnodepathinfo contains only integer fields, for which zero is valid.
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of_val(&info);
    // SAFETY: `info` is writable storage of the size passed to proc_pidinfo.
    let result = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            (&mut info as *mut libc::proc_vnodepathinfo).cast(),
            i32::try_from(size).ok()?,
        )
    };
    if usize::try_from(result).ok()? < size {
        return None;
    }
    // SAFETY: proc_pidinfo returns a NUL-terminated path in vip_path on success.
    let path = unsafe { std::ffi::CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr().cast()) }
        .to_str()
        .ok()?;
    (!path.is_empty()).then(|| PathBuf::from(path))
}

#[cfg(target_os = "linux")]
fn process_current_dir(pid: libc::pid_t) -> Option<PathBuf> {
    fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn process_current_dir(_pid: libc::pid_t) -> Option<PathBuf> {
    None
}

pub(crate) fn validate_size(cols: u16, rows: u16) -> Result<(), PtyError> {
    if !(1..=MAX_DIMENSION).contains(&cols) || !(1..=MAX_DIMENSION).contains(&rows) {
        return Err(PtyError::InvalidArgument(format!(
            "PTY dimensions must be between 1 and {MAX_DIMENSION} columns and rows"
        )));
    }
    Ok(())
}

fn spawn_reader(
    mut reader: PtyReader,
    shared: Arc<Mutex<Shared>>,
    gate: Arc<Condvar>,
) -> std::io::Result<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("gokod-pty-reader".to_owned())
        .spawn(move || {
            let mut chunk = [0_u8; 16 * 1024];
            loop {
                wait_for_flow_credit(&shared, &gate);
                match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => lock_unpoisoned(&shared).publish(&chunk[..count]),
                }
            }
            lock_unpoisoned(&shared).close();
        })
}

/// Pause before the next read while a live flow subscriber has a full window
/// of unacknowledged output. The `Shared` lock is not held across the wait and
/// the state is re-checked at least every `FLOW_RECHECK`. When `FLOW_STALL`
/// passes without the acknowledged offset moving, the flow subscribers are
/// dropped through the usual `Lagged` path and reading continues.
fn wait_for_flow_credit(shared_mutex: &Mutex<Shared>, gate: &Condvar) {
    let mut shared = lock_unpoisoned(shared_mutex);
    // (when the current wait began, acknowledged offset it began at); an ack
    // move restarts the stall clock.
    let mut waiting: Option<(Instant, u64)> = None;
    while shared.flow_blocked() {
        match waiting {
            Some((since, baseline)) if shared.acknowledged == baseline => {
                if since.elapsed() >= FLOW_STALL {
                    shared.lag_flow_subscribers();
                    break;
                }
            }
            _ => waiting = Some((Instant::now(), shared.acknowledged)),
        }
        let (guard, _) = gate
            .wait_timeout(shared, FLOW_RECHECK)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        shared = guard;
    }
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

#[cfg(test)]
mod locale_tests {
    use super::{resolve_utf8_locale, DEFAULT_UTF8_LOCALE};

    #[test]
    fn locale_precedence_prefers_lc_all_then_lc_ctype_then_lang() {
        assert_eq!(
            resolve_utf8_locale(Some("en_US.UTF-8"), Some("C.UTF-8"), Some("en_US.UTF-8")),
            ("en_US.UTF-8".into(), false)
        );
        assert_eq!(
            resolve_utf8_locale(Some("C"), Some("C.UTF-8"), Some("en_US.UTF-8")),
            ("C.UTF-8".into(), true)
        );
        assert_eq!(
            resolve_utf8_locale(Some("C"), Some("POSIX"), Some("en_US.UTF-8")),
            ("en_US.UTF-8".into(), true)
        );
    }

    #[test]
    fn locale_falls_back_when_inherited_locales_are_not_utf8() {
        assert_eq!(
            resolve_utf8_locale(Some("C"), Some("POSIX"), None),
            ("en_US.UTF-8".into(), true)
        );
        assert_eq!(
            resolve_utf8_locale(None, None, None),
            ("en_US.UTF-8".into(), false)
        );
    }

    #[test]
    fn unavailable_utf8_named_locale_is_rejected() {
        assert_eq!(
            resolve_utf8_locale(Some("goko_missing_locale.UTF-8"), None, None),
            (DEFAULT_UTF8_LOCALE.into(), true)
        );
    }
}
