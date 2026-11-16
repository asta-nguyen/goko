# Design — Windows support: port gokod + goko-rune to Windows

Issue: 29

## Goal

Goko builds, runs, and ships on Windows x86_64 (Windows 10 1809+): `gokod`
serves named pipes instead of Unix sockets, owns PTYs through ConPTY, and
kills process trees through Job Objects; `goko-rune` connects over a named
pipe, spawns the daemon detached, and degrades the documented v1 features;
`release.yml` publishes a `windows-x86_64` artifact installed by
`install.ps1`. The wire protocol, framing, token auth, `GOKO_HOME` layout, and
CLI surface stay identical.

## Scope

Four phases, one change:

1. **Daemon** — `crates/daemon/src/{paths,lock,auth,main,server,pty}.rs`:
   `%LOCALAPPDATA%` home, mode-bit removal, named-pipe transport, ConPTY I/O,
   Job-Object lifecycle, `ctrl_c`/`ctrl_close` signals, locale cfg-out.
2. **app-core** — `home.rs` (`%LOCALAPPDATA%`), `agents.rs` (`.cmd` shims,
   Windows PATH adoption, no POSIX hooks), `acp.rs`/`lsp.rs` (Job Objects
   instead of process groups), `worktrees.rs`/`accounts.rs`/`store.rs`/
   `code_index.rs` (mode bits, symlinks), `session_debug.rs` (`lsof` cfg-out),
   `subagents.rs` callers degrade, `open_url`/`reveal`.
3. **App** — `apps/rune/src/daemon.rs` (named-pipe client, `DETACHED_PROCESS`
   spawn, `.exe` lookup), `backend.rs` (`default_shell`, `ps` cfg-out),
   `panes.rs`/`files.rs` openers, `trash.rs`/`sound.rs`/`settings.rs`
   degradations, `update.rs` (updater off on Windows), `quit_guard` no-op.
4. **Distribution** — `apps/rune/package-windows.ps1`, `distribution/install.ps1`,
   `release.yml` `windows-latest` job + `latest.json` `windows-x86_64` entry,
   README install command, `ci.yml` Windows job.

Out of scope: MSI/winget packaging, code signing (SmartScreen warning
accepted), ARM64 Windows, WSL interop.

## Behavior

**Transport.** On Unix the daemon keeps `daemon.sock` exactly as today. On
Windows it serves `\\.\pipe\goko-<sha256(GOKO_HOME)[..16]>` with a security
descriptor granting only the current user (and LocalSystem); a second `gokod`
detects the existing instance by failing to create the pipe's first instance
and prints `gokod: already running` like the lock path does. Client and server
both speak the existing length-framed protocol over `AsyncRead + AsyncWrite`;
no frame changes. Token auth is unchanged (`daemon.token` in `GOKO_HOME`).

**PTY I/O.** Unix keeps the fd/`select` nonblocking loop. Windows uses
`portable-pty`'s ConPTY backend: one blocking reader thread per PTY forwarding
`Read` bytes into the daemon's existing output channel and one writer handle
for input; `wait_ready` has no Windows analogue and the Windows path reports
always-ready, with the reader thread as the only blocking waiter. Resize goes
through `MasterPty::resize`.

**Process lifecycle.** Each Windows session is assigned to a Job Object with
`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`; `stop` calls `TerminateJobObject`
immediately (no graceful signal — accepted v1 degradation, documented), and
daemon shutdown closes every job handle, killing the trees.

**Signals.** Windows installs `tokio::signal::ctrl_c` and
`tokio::signal::windows::ctrl_close` handlers; the socket/pipe-driven
`system.shutdown` path is unchanged and remains the primary stop mechanism.

**Paths and permissions.** `home_dir` resolves `GOKO_HOME`, else
`%LOCALAPPDATA%\Goko` on Windows. All `.mode(0o600/0o700)` calls become
`cfg(unix)`; Windows privacy comes from the `%LOCALAPPDATA%` ACL and the pipe
ACL. `auth.rs` reads randomness via `getrandom` instead of `/dev/urandom`.

**app-core.** `default_shell()` returns `%COMSPEC%` (fallback
`powershell.exe`); agent executables resolve through `PATHEXT` (`claude.cmd`
etc.) and spawn without `/usr/bin/env`; `adopt_login_shell_path` is a no-op
(GUI inherits the merged PATH); POSIX-only Claude hook/statusline strings are
skipped on Windows. ACP and LSP children get Job Objects instead of
`process_group(0)`/`killpg`. Symlink creation uses `symlink_file`/
`symlink_dir` where the account holds the privilege, else copies; dev/ino
identity falls back to canonicalized paths. `ps`-based subagent attribution
returns empty on Windows (documented degradation); `lsof` debugging is
cfg'd out.

**App.** The Rune client connects to the named pipe, resolves `gokod.exe`
beside the executable, and spawns it with `DETACHED_PROCESS |
CREATE_NEW_PROCESS_GROUP`. `open_url`/`reveal` use `explorer` /
`ShellExecuteW`. `trash`, `sound`, in-app updater, and quit veto are absent
on Windows in v1 (each shows its existing non-macOS fallback or is skipped);
`os_dark_mode` reads the registry app-theme value.

**Distribution.** `package-windows.ps1` produces `goko-windows-x86_64.zip`
(`goko-rune.exe`, `gokod.exe`, icon, README) plus a
`windows-x86_64.target.json` entry; `install.ps1` downloads, extracts to
`%LOCALAPPDATA%\Goko\app`, and puts `goko.exe` on the user PATH via a
Start-menu shortcut; `release.yml` gains a `windows-latest` job; `ci.yml`
gains `windows-latest` running build + the portable test subset.

## Edge cases

- **Pipe name collision / stale instance**: a dead `gokod` leaves no pipe
  (kernel-owned), so a fresh instance just binds; a live one fails with
  `ERROR_PIPE_BUSY`, which the daemon maps to `already running`.
- **Pipe ACL failure**: daemon exits with a clear error rather than serving a
  world-writable pipe.
- **ConPTY spawn failure** (pre-1809): the daemon reports the session spawn
  error through the existing error path; the app shows it like any spawn
  failure.
- **Kill contract**: `stop` on Windows is immediate tree-kill; the session
  `signal` field in replies is always `null`, and tests assert `exit_code`
  only.
- **Detached daemon**: `DETACHED_PROCESS` means no console; daemon logging
  goes to `daemon.log` as today.
- **`.cmd` shims**: agent argv resolution tries the literal path, then
  `PATHEXT` variants; a missing shim surfaces the existing "agent not found"
  error.
- **Symlink privilege**: without Developer Mode/admin, `link_shared` copies
  instead of linking; worktree identity compares canonical paths.
- **`rune` GUI dependency**: the pinned rev's Windows backend is claimed but
  unverified from this repo. If `apps/rune` fails to build on Windows CI, the
  daemon + app-core phases still ship (CI split), and the app phase reports
  the upstream blocker instead of masking it.
- **CRLF**: ConPTY emits `\r\n`; the terminal emulator already handles CR, so
  no translation layer is added.

## Interfaces

- Protocol (`crates/protocol`): unchanged.
- Daemon CLI: unchanged (`gokod`, `--version`, usage exit 2).
- Env: `GOKO_HOME` honored on all platforms; Windows default
  `%LOCALAPPDATA%\Goko`.
- Named pipe: `\\.\pipe\goko-<16 hex of sha256(GOKO_HOME)>`, byte mode,
  first-instance ACL owner-only.
- Artifacts: `windows-x86_64` target key in `latest.json`; zip layout mirrors
  the Linux tar (`goko-rune.exe`, `gokod.exe`, icon).
- New test-only seam: a `daemon::net` module with `Listener`/`Stream` type
  aliases per platform so tests can drive both.

## Verification

- **Local (macOS)**: `cargo check -p gokod -p goko-protocol --target
  x86_64-pc-windows-msvc` (pure-Rust crates; msvc std is installed). `app-core`
  and `apps/rune` link `rusqlite`/`libsqlite3-sys` and cannot be checked
  locally without a Windows C toolchain — a recorded verification limit, not a
  silent pass.
- **CI**: new `windows-latest` job runs `cargo build --workspace` and
  `cargo test` for crates whose tests are portable on Windows (daemon
  protocol/dispatch tests plus app-core unit tests that do not need a POSIX
  shell), with Unix-only tests `cfg(unix)`-gated. The job is the runtime
  evidence for this change.
- **Release**: `release.yml` `windows-latest` job produces the zip + target
  entry; `latest.json` merge already handles arbitrary targets.
- **Manual**: no Windows machine is available to this session — runtime
  verification is CI-only and this limit is reported in the final review.
- **Degradation checks**: daemon tests assert `stop` kills the tree and
  shutdown closes jobs; app tests assert the Windows client path and
  `DETACHED_PROCESS` flags compile and arm the right argv (platform-gated).

## Related context

- [Architecture overview](../../../llm/architecture/overview.md) — process
  topology, daemon responsibilities, and shutdown contract this port keeps.
- [Rune editor](../../../llm/domains/rune-editor.md) — the editor surface
  whose daemon-facing client and session chrome this port adapts.
- [Rune chrome](../../../llm/domains/rune-chrome.md) — the shell whose
  daemon/session controls ride the same transport.
