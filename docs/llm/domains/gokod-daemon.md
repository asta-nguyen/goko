# Gokod daemon

`gokod` is the standalone process that owns every PTY and child process. It
holds `daemon.lock`, writes `daemon.token`, serves the transport, and logs to
`daemon.log`. The wire protocol, framing, token auth, and `GOKO_HOME` file
names are identical on every platform; the transport, PTY driver, and process
lifecycle are platform-specific behind `cfg` seams.

## Requirements

### GKD-home

The system SHALL resolve `GOKO_HOME`, else `~/Library/Application Support/Goko` on macOS, `$XDG_STATE_HOME/goko` or `~/.local/state/goko` on Linux, and `%LOCALAPPDATA%\Goko` (falling back to `%USERPROFILE%\AppData\Local\Goko`) on Windows, creating it private to the user: mode 0700 on Unix, the profile ACL on Windows.

#### Scenario: Default folder per platform

- GIVEN no `GOKO_HOME`
- WHEN `gokod` starts on macOS, Linux, or Windows
- THEN it uses that platform's default folder and creates it if missing

Evidence: `crates/daemon/src/paths.rs`, `crates/app-core/src/home.rs`

### GKD-process-stop

The system SHALL stop a session by signalling its process group with SIGTERM (and SIGHUP on hangup, SIGKILL on kill) on Unix, and by terminating the session's Job Object on Windows — where `stop` is an immediate tree kill and replies carry `signal: null` — killing the remaining trees when the daemon shuts down.

#### Scenario: Graceful on Unix

- GIVEN a live session on Unix
- WHEN `session.stop` arrives
- THEN SIGTERM reaches the process group and the exit reply reports the exit code or signal

#### Scenario: Immediate on Windows

- GIVEN a live session on Windows
- WHEN `session.stop` arrives
- THEN the Job Object is terminated, the whole tree exits, and the reply's `signal` field is null

#### Scenario: Daemon shutdown

- GIVEN live sessions on either platform
- WHEN the daemon shuts down (signal, console close, or `system.shutdown`)
- THEN every session tree is killed and the endpoint is released

Evidence: `crates/daemon/src/pty.rs`, `crates/daemon/src/session.rs`, `crates/daemon/src/main.rs`, `crates/daemon/tests/pty_signal.rs`, `crates/daemon/tests/lifecycle.rs`

### GKD-pty-driver

The system SHALL drive PTY I/O through a non-blocking descriptor loop (`select`) on Unix and through ConPTY with one blocking reader thread and a writer handle on Windows, keeping the same output channel, replay buffer, subscriber fan-out, flow window, and terminal-model ordering on both.

#### Scenario: Output and input

- GIVEN a session is created on either platform
- WHEN the child prints and receives input
- THEN bytes flow through the same replay/subscribe contract, and resizing reaches the child through the platform's PTY resize

#### Scenario: Flow control

- GIVEN a flow subscriber that stops acknowledging
- WHEN the window fills
- THEN the reader pauses (Unix between readiness waits, Windows between blocking reads) and the stall path drops the subscriber as on Unix

Evidence: `crates/daemon/src/pty.rs`, `crates/daemon/tests/pty_io.rs`, `crates/daemon/tests/pty_spawn.rs`, `crates/daemon/tests/windows_ipc.rs`

### GKD-transport

The system SHALL serve the framed protocol over `daemon.sock` on Unix and over the named pipe `\\.\pipe\goko-<16 hex>` on Windows — the pipe name derived from `GOKO_HOME` so one home maps to one daemon, created with an owner-only security descriptor (current user and LocalSystem) and rejecting remote clients — reporting `gokod: already running` with exit 0 when a live daemon owns the endpoint.

#### Scenario: Unix socket

- GIVEN a Unix host with no live daemon
- WHEN `gokod` starts
- THEN it replaces a stale `daemon.sock`, binds a fresh one with mode 0600, and serves the existing frames unchanged

#### Scenario: Windows pipe

- GIVEN a Windows host and a `GOKO_HOME`
- WHEN `gokod` starts
- THEN it serves the pipe named from that home with an owner-only DACL, and a second instance on the same home exits 0 printing `gokod: already running`

#### Scenario: Client connects

- GIVEN a running daemon on either platform
- WHEN a client opens the socket or pipe and sends the hello with the token
- THEN the handshake, responses, events, and output frames are byte-identical to the Unix contract

Evidence: `crates/daemon/src/net.rs`, `crates/daemon/src/server.rs`, `crates/daemon/src/main.rs`, `crates/daemon/tests/handshake.rs`, `crates/daemon/tests/windows_ipc.rs`


## Flow

`gokod` resolves `GOKO_HOME`, takes the exclusive `daemon.lock`, opens
`daemon.log`, writes a fresh 64-hex token through `getrandom`, binds the
transport (`daemon.sock` mode 0600 on Unix; the owner-only pipe on Windows),
and accepts connections until SIGTERM/SIGINT (Unix), console close/ctrl-c
(Windows), or an authenticated `system.shutdown`. Each connection is one
writer task plus one reader loop over `AsyncRead + AsyncWrite`, so framing and
ordering do not depend on the transport.

A `session.create` spawns through `portable-pty`: on Unix the master is
duplicated into two non-blocking descriptors with `fcntl`, the child starts
with `setsid` (its pid is its group id), and the reader waits on `select`
between reads. On Windows the ConPTY master yields a blocking reader (one
dedicated thread) and a writer handle, and the child is assigned to a
`KILL_ON_JOB_CLOSE` Job Object. Output flows through the same ring buffer,
model feed, and subscriber fan-out in both cases; the flow window pauses the
reader between reads on Windows, since no readiness primitive exists there.

The session-launch lock is a shared `flock` on `worktree-operations.lock` on
Unix and a shared `File` lock on Windows, so worktree removal stays mutually
exclusive with PTY creation on both.

## State changes

`GOKO_HOME` holds `daemon.lock` (exclusive instance lock), `daemon.token`
(rewritten per start), `daemon.log`, and the shared `goko.db`. Sessions live in
memory in `SessionManager`; their output lives in each `PtyProcess`'s ring
buffer. Windows job handles die with their session (and with the daemon
process), which is what makes `KILL_ON_JOB_CLOSE` the shutdown contract.

## Side effects

- PTY children (and their trees) on the host.
- POSIX signals on Unix; Job Object termination on Windows.
- `daemon.sock` file creation/removal on Unix; pipe instance creation on
  Windows.

## Authorization & constraints

- The token file is mode 0600 on Unix; the pipe's DACL is owner-only on
  Windows, and `reject_remote_clients` keeps the pipe local.
- `daemon.lock` is `try_lock`, never blocking: a second instance exits 0.
- The daemon never derives session identity from the app's state; every read
  validates project and checkout through `app-core`.

## Error paths

- Wrong token or version: an error response, then the connection closes.
- No hello within 3 s: an error response, then close.
- A spawn failure reports `session.create` as an error and leaves no session.
- A PTY read error ends the output stream (`Closed` for subscribers); a write
  to an exited process fails instead of blocking.

## Tests

`crates/daemon/tests/` covers dispatch, handshake, lifecycle, PTY spawn/IO,
signals, subscribe, flow, and paint over the Unix socket (all `cfg(unix)`);
`windows_ipc.rs` covers the pipe round-trip, the already-running path, ConPTY
spawn, and `stop` killing the tree on Windows CI.

## Related

- [Architecture overview](../architecture/overview.md)
- [Rune editor](rune-editor.md)
- [Rune chrome](rune-chrome.md)

## Sources

- `crates/daemon/src/net.rs`
- `crates/daemon/src/paths.rs`
- `crates/daemon/src/lock.rs`
- `crates/daemon/src/auth.rs`
- `crates/daemon/src/main.rs`
- `crates/daemon/src/server.rs`
- `crates/daemon/src/pty.rs`
- `crates/daemon/src/session.rs`
- `crates/daemon/src/terminal.rs`
- `crates/daemon/src/buffer.rs`
- `crates/daemon/tests/handshake.rs`
- `crates/daemon/tests/lifecycle.rs`
- `crates/daemon/tests/pty_io.rs`
- `crates/daemon/tests/pty_spawn.rs`
- `crates/daemon/tests/pty_signal.rs`
- `crates/daemon/tests/windows_ipc.rs`
- `crates/app-core/src/home.rs`
