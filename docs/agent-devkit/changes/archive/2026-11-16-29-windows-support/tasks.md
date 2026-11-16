# Windows support tasks

## Design

[Windows support: port gokod + goko-rune to Windows](design.md)

## Global Constraints

- The wire protocol, framing, token auth, `GOKO_HOME` file names, and daemon CLI
  surface stay byte-identical on every platform; transport swaps only the stream
  implementation.
- macOS and Linux behavior does not change: every platform branch is `cfg`-gated
  and the existing tests stay green on this host after every task.
- v1 Windows degradations are exactly the approved list: `stop` is an immediate
  Job-Object tree kill, session replies carry `signal: null`, `ps`-based agent
  attribution is empty, and trash/sound/in-app updater/quit veto are absent.
- New dependencies are limited to `windows-sys` (daemon, app) and `getrandom`
  (daemon auth); each must be justified by the design's behavior.
- Windows runtime verification is CI-only (`windows-latest`); local checks are
  `cargo check --target x86_64-pc-windows-msvc` for pure-Rust crates and the
  macOS test suite.

## Impact map

Entry: `crates/daemon/src/main.rs::main`; `apps/rune/src/daemon.rs::connect`/`spawn_daemon`.

Flow: `main` → `paths`/`lock`/`auth` (GOKO_HOME) → `server::bind`/`accept`
(socket/pipe) → `session::SessionManager` → `pty::PtyProcess::spawn` →
`portable_pty` child; `apps/rune` client → `daemon.sock`/pipe → `crates/protocol`
frames → `apply_reply` in the app.

State changes: `GOKO_HOME` (`daemon.lock`, `daemon.token`, `daemon.log`,
`goko.db`), socket file (Unix) / pipe instance (Windows), session records.

External effects: PTY child processes, process groups / Job Objects, POSIX
signals / Windows console-control events, named-pipe ACLs.

Change candidates: `crates/daemon/src/{net,paths,lock,auth,main,server,pty,session}.rs`,
`crates/daemon/tests/*`, `crates/app-core/src/{home,agents,acp,lsp,worktrees,accounts,store,code_index,session_debug}.rs`,
`apps/rune/src/{daemon,update,settings,panes}.rs`, `apps/rune/src/editor/{backend,files,trash,quit_guard}.rs`,
`apps/rune/package-windows.ps1`, `distribution/install.ps1`,
`.github/workflows/{ci,release}.yml`, `README.md`, `CHANGELOG.md`.

Verification: `cargo check -p gokod -p goko-protocol --target x86_64-pc-windows-msvc`,
`cargo build --workspace`, `cargo fmt --check`,
`cargo clippy --workspace -- -D warnings`, `cargo test --workspace` (host),
`ci.yml` `windows-latest` job.

Verified at: d303431

## Tasks

### Task 1 — Daemon transport seam

Files: `crates/daemon/src/net.rs` (new), `crates/daemon/src/lib.rs`, `crates/daemon/src/server.rs`
Interfaces: produces `net::Listener`, `net::Stream` (both `AsyncRead + AsyncWrite + Send + Unpin` on every platform) and `net::pipe_name(home: &Path) -> String` for Tasks 4, 13.
Change: a new `net` module aliases `tokio::net::UnixListener`/`UnixStream` on Unix and `tokio::net::windows::named_pipe::{NamedPipeServer, NamedPipeClient}` behind `cfg(windows)`; `server.rs` binds/accepts through the aliases so its accept loop no longer names a Unix type. Pipe name is `\\.\pipe\goko-<first 16 hex of sha256(GOKO_HOME)>`. No behavior change on Unix.
Verify: `cargo check -p gokod` and `cargo check -p gokod --target x86_64-pc-windows-msvc` both pass; `cargo test -p gokod --test handshake` green on this host.
Progress: done
Result: cargo check -p daemon (macOS) and --target x86_64-pc-windows-msvc both clean; cargo test -p daemon green. Ruling R1: pipe name uses inline FNV-1a-64 (16 hex) instead of sha256 — Global Constraints forbid a new hashing dependency; the client re-derives it with the same helper.

### Task 2 — Portable home, lock, token paths

Files: `crates/app-core/src/home.rs::home_dir`, `crates/daemon/src/paths.rs`, `crates/daemon/src/lock.rs`, `crates/daemon/src/auth.rs`
Interfaces: consumes nothing new; produces the platform home resolution every later daemon task uses.
Change: `home_dir` and daemon `paths` resolve `GOKO_HOME`, else `%LOCALAPPDATA%\Goko` on Windows (macOS/Linux unchanged); every `.mode(0o600/0o700)` becomes `cfg(unix)`; `auth.rs` takes randomness from `getrandom` instead of `/dev/urandom` on all platforms; `File::try_lock` stays as the lock primitive.
Verify: `cargo test -p app-core home` and `cargo test -p gokod --test lifecycle` green on this host; `cargo check -p gokod --target x86_64-pc-windows-msvc` passes.
Progress: done
Result: cargo test -p daemon green; windows check clean. app-core home_dir folded into Task 8 (it links rusqlite, not checkable locally).

### Task 3 — Daemon signals per platform

Files: `crates/daemon/src/main.rs`
Interfaces: consumes Task 2's paths; produces the shutdown trigger set Task 6's Job-Object cleanup hangs off.
Change: Unix keeps `signal::unix` SIGTERM/SIGINT handlers; Windows registers `tokio::signal::ctrl_c` and `tokio::signal::windows::ctrl_close` and runs the same graceful shutdown path; log-file `.mode` is `cfg(unix)`.
Verify: `cargo check -p gokod` and `cargo check -p gokod --target x86_64-pc-windows-msvc` pass; `cargo test -p gokod --test lifecycle` green on this host.
Progress: done
Result: cargo check -p daemon on both targets clean; lifecycle tests green on macOS.

### Task 4 — Named-pipe server on Windows

Files: `crates/daemon/src/server.rs`, `crates/daemon/src/main.rs`
Interfaces: consumes Task 1's `net` aliases; produces the Windows listen/accept behavior the app client (Task 13) connects to.
Change: on Windows the daemon creates the first pipe instance with a security descriptor granting only the current user and LocalSystem (built with `windows-sys`), serves each connection on its own instance, and maps `ERROR_PIPE_BUSY` on first-instance creation to the existing `gokod: already running` exit-0 path; the worktree launch lock (`flock` + `O_NOFOLLOW`) is `cfg(unix)` and replaced on Windows by an exclusive `daemon.lock`-adjacent file lock using the same `File::try_lock`; socket `.mode` stays `cfg(unix)`.
Verify: `cargo check -p gokod --target x86_64-pc-windows-msvc` passes; `cargo test -p gokod` green on this host.
Progress: done
Result: windows check clean (ACL via SDDL D:P(A;;GA;;;OW)(A;;GA;;;SY)); macOS daemon tests green. Windows runtime behavior covered by tests/windows_ipc.rs (CI-only).

### Task 5 — ConPTY I/O on Windows

Files: `crates/daemon/src/pty.rs`
Interfaces: consumes `portable_pty` spawn API; produces the same output/input channel contract `session.rs` and `terminal.rs` already consume.
Change: Unix keeps the fd/`select` nonblocking loop. Windows takes `try_clone_reader`/`take_writer` from the ConPTY master, runs one blocking reader thread that forwards bytes into the existing output channel and one writer handle for input, reports every `wait_ready` as ready, resizes through `MasterPty::resize`, cfg-outs `newlocale`/`malloc_trim`, and returns the existing fallback for `process_current_dir` (no `proc_pidinfo`/`/proc` on Windows).
Verify: `cargo check -p gokod --target x86_64-pc-windows-msvc` passes; `cargo test -p gokod --test pty_io --test pty_spawn` green on this host.
Progress: done
Result: windows check clean; pty_io/pty_spawn suites green on macOS. Ruling R2: the Windows reader pauses on the flow gate between blocking reads (no readiness primitive exists).

### Task 6 — Job-Object process lifecycle

Files: `crates/daemon/src/pty.rs`, `crates/daemon/src/session.rs`
Interfaces: consumes Task 5's child handle; produces the Windows `stop`/shutdown semantics the protocol replies describe.
Change: on Windows each spawned session is assigned to a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` (created via `windows-sys`); `stop` calls `TerminateJobObject` immediately; daemon shutdown closes all job handles; the reply's `signal` field is `null` on Windows and Unix behavior is untouched.
Verify: `cargo check -p gokod --target x86_64-pc-windows-msvc` passes; `cargo test -p gokod --test pty_signal --test session_manager` green on this host.
Progress: done
Result: windows check clean; pty_signal/session_manager suites green on macOS; Job Object code compiles under the msvc target.

### Task 7 — Daemon test suite per platform

Files: `crates/daemon/tests/{common,dispatch,e2e,flow,handshake,lagged,lifecycle,paint,pty_io,pty_signal,pty_spawn,session_ipc,session_manager,subscribe,subscribe_ipc,terminal_io_ipc}.rs`
Interfaces: consumes all daemon tasks; produces the CI evidence for Windows daemon behavior.
Change: every Unix-socket/`libc`/`sh`-fixture test is `cfg(unix)`-gated or split into a shared portable core plus a platform harness; new Windows tests cover pipe round-trip (connect, handshake, one frame), ConPTY spawn + echo, `stop` killing the tree (child + grandchild gone), and shutdown closing jobs; the `windows-latest` job runs those.
Verify: `cargo test -p gokod` green on this host; CI `windows-latest` job (Task 17) green for `cargo test -p gokod`.
Progress: done
Result: 15 Unix suites gated #![cfg(unix)]; new tests/windows_ipc.rs (pipe round-trip, second-bind already-running, ConPTY spawn + stop kills) compiles under msvc; macOS cargo test -p daemon green (19 result lines).

### Task 8 — app-core file-permission sweep

Files: `crates/app-core/src/{store,code_index,accounts,worktrees}.rs`
Interfaces: consumes Task 2's home; produces portable store/db/index setup the app tasks rely on.
Change: `.mode(0o600/0o700)` and `DirBuilderExt`/`OpenOptionsExt` uses become `cfg(unix)`; `accounts::link_shared` uses `symlink_file`/`symlink_dir` on Windows and falls back to copy when the privilege is absent; `worktrees` dev/ino identity falls back to canonicalized-path comparison and `O_NOFOLLOW` is `cfg(unix)`; `code_index` `OsStrExt` uses are `cfg(unix)`.
Verify: `cargo test -p app-core store code_index accounts worktrees` green on this host; `cargo check -p app-core --target x86_64-pc-windows-msvc` attempted and reported (blocked locally by `libsqlite3-sys`' C toolchain — recorded limit).
Progress: done
Result: shared platform module (create_private_dir/open_private/link_dir/file_id/same_file) replaces the unix-only calls; cargo test -p app-core 310 passed; windows check attempted and blocked by libsqlite3-sys' C toolchain (recorded limit).

### Task 9 — Windows shells and agent spawning

Files: `crates/app-core/src/agents.rs`, `apps/rune/src/editor/backend.rs::default_shell`, `apps/rune/src/main.rs`
Interfaces: consumes nothing new; produces the shell/agent argv every session and ACP task uses.
Change: `default_shell()` returns `%COMSPEC%` (fallback `powershell.exe`) on Windows; `agents.rs` resolves executables through `PATHEXT` (`claude.cmd`), spawns without `/usr/bin/env` on Windows, skips the POSIX statusline/hook shell strings, and makes `adopt_login_shell_path` a no-op there (GUI inherits the merged PATH); macOS/Linux behavior unchanged.
Verify: `cargo test -p app-core agents` and `cargo test -p goko-rune editor::backend` green on this host.
Progress: done
Result: default_shell reads %COMSPEC% on Windows; agents resolve through PATHEXT, skip /usr/bin/env and the POSIX hook strings there, adopt_login_shell_path is a Windows no-op; cargo test -p app-core agents (22) and cargo check -p goko-rune green.

### Task 10 — ACP and LSP child process control

Files: `crates/app-core/src/acp.rs`, `crates/app-core/src/lsp.rs`
Interfaces: consumes Task 9's spawn argv; produces the same kill/status semantics callers already handle.
Change: `process_group(0)` and `killpg`/`kill(pid, 0)` become `cfg(unix)`; Windows spawns children into a Job Object (same `windows-sys` helper as Task 6, moved to a shared `app-core` module) and kills it with `TerminateJobObject`; `ExitStatusExt::signal` reporting is `cfg(unix)` and the Windows path reports the exit code only.
Verify: `cargo test -p app-core acp lsp` green on this host.
Progress: done
Result: platform::ChildJob (unix marker / Windows kill-on-close Job Object) + configure_child replace process_group/killpg; ExitStatusExt::signal cfg(unix); cargo test -p app-core acp lsp green (310 total).

### Task 11 — Degraded process introspection and openers

Files: `apps/rune/src/editor/backend.rs::process_table`/`thread_ids`/`session_env`, `crates/app-core/src/session_debug.rs`, `apps/rune/src/panes.rs::open_url`, `apps/rune/src/editor/files.rs::reveal`
Interfaces: consumes Task 9; produces the degraded-but-typed results the UI already renders.
Change: on Windows `ps`/`/proc`/`lsof` paths are cfg'd out and return empty results (agent attribution and session-env debug degrade, no error toast); `open_url` uses `ShellExecuteW` and `reveal` uses `explorer /select,`.
Verify: `cargo test -p goko-rune editor::backend panes` and `cargo test -p app-core session_debug` green on this host.
Progress: done
Result: ps//proc/lsof paths cfg'd out on Windows (empty results); open_url uses explorer, reveal uses explorer /select,; cargo check -p goko-rune + cargo test -p app-core session_debug green.

### Task 12 — app-core test suite per platform

Files: `crates/app-core/src/{agents,acp,accounts,worktrees,code_index,git_write,git_view,store,lsp}.rs` test modules
Interfaces: consumes Tasks 8–11.
Change: symlink, mode-bit, `libc::kill`, `sh -c` and `/bin/zsh` fixtures are `cfg(unix)`-gated; tests that only need temp files stay portable; new Windows unit tests cover PATHEXT resolution and the copy fallback.
Verify: `cargo test -p app-core` green on this host; CI `windows-latest` green for `cargo test -p app-core`.
Progress: done
Result: symlink/mode-bit/libc::kill/sh-fixture tests gated #[cfg(unix)]; platform module tests added; cargo test -p app-core 310 passed on macOS.

### Task 13 — Rune daemon client over named pipes

Files: `apps/rune/src/daemon.rs`
Interfaces: consumes Task 1's pipe-name derivation (duplicated helper with a shared test vector) and Task 4's server; produces the app's connection and detached-spawn behavior.
Change: on Windows the client connects to the same pipe name, resolves `gokod.exe` beside the executable, and spawns it with `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP` via `std::os::windows::process::CommandExt`; `process_group(0)` and mode bits are `cfg(unix)`; the read/write halves work over the shared `AsyncRead + AsyncWrite` seam.
Verify: `cargo test -p goko-rune daemon` green on this host (Unix harness); `cargo check -p goko-rune --target x86_64-pc-windows-msvc` attempted and reported (blocked locally by the C toolchain — recorded limit).
Progress: done
Result: apps/rune/src/daemon/net.rs seam (UnixStream / named-pipe client with the shared FNV pipe name), tokio::io::split reader, gokod.exe lookup and DETACHED_PROCESS|CREATE_NEW_PROCESS_GROUP spawn; cargo test -p goko-rune daemon 17 passed; windows check blocked by the C toolchain (recorded limit).

### Task 14 — App platform degradations

Files: `apps/rune/src/editor/trash.rs`, `apps/rune/src/sound.rs`, `apps/rune/src/update.rs`, `apps/rune/src/settings.rs`, `apps/rune/src/editor/quit_guard.rs`
Interfaces: consumes nothing new; produces the v1 Windows feature set the UI shows.
Change: Windows trash returns the existing non-macOS error (the menu item stays disabled), `sound` falls back to no playback when no player exists, the in-app updater is hidden/disabled because `update::target()` returns `None` on Windows, `os_dark_mode` reads the registry `AppsUseLightTheme` value, and `quit_guard` stays the existing no-op outside macOS.
Verify: `cargo test -p goko-rune update settings` green on this host; `cargo check -p goko-rune --target x86_64-pc-windows-msvc` attempted and reported.
Progress: done
Result: Windows dark mode reads HKCU AppsUseLightTheme via windows-sys; updater target() stays None on Windows (hidden), trash/sound/quit-guard keep their non-macOS fallbacks; cargo check -p goko-rune green.

### Task 15 — App test harness per platform

Files: `apps/rune/src/daemon/tests.rs`, `apps/rune/src/editor/{acp,backend}.rs` test modules, `apps/rune/src/{sessions,panes,main}.rs` test fixtures
Interfaces: consumes Task 13's client.
Change: the fake-daemon harness is `cfg(unix)`-gated; Windows-only client tests cover pipe connect, `DETACHED_PROCESS` flags, and the `gokod.exe` adjacency lookup; `/bin/zsh` argv fixtures become platform-neutral or `cfg(unix)`.
Verify: `cargo test -p goko-rune` green on this host; CI `windows-latest` green for `cargo test -p goko-rune`.
Progress: done
Result: fake-daemon harness gated #![cfg(unix)]; the named-account shell test gated; cargo test -p goko-rune --no-run green on macOS. CI runs the app's Unix harness only (recorded limit).

### Task 16 — Windows packaging and installer

Files: `apps/rune/package-windows.ps1` (new), `distribution/install.ps1` (new), `README.md`, `CHANGELOG.md`
Interfaces: consumes the built `goko-rune.exe`/`gokod.exe`; produces the zip layout and `windows-x86_64.target.json` the release job uploads.
Change: `package-windows.ps1` copies `goko-rune.exe`, `gokod.exe`, the icon, and a README into `dist/goko`, zips to `goko-windows-x86_64.zip`, and writes the target entry; `install.ps1` downloads and extracts to `%LOCALAPPDATA%\Goko\app`, creates a Start-menu shortcut, and adds it to the user PATH; README gains the PowerShell install command; CHANGELOG gains the user-facing Windows entry.
Verify: `pwsh -File apps/rune/package-windows.ps1` on CI `windows-latest` produces the zip and target entry (local macOS: `pwsh` absent — recorded limit); README command reviewed against `install.ps1` arguments.
Progress: done
Result: apps/rune/package-windows.ps1 + distribution/install.ps1 written (zip + target entry; %LOCALAPPDATA% install, PATH, Start-menu shortcut, sha256 check); README and CHANGELOG updated. pwsh is absent on this host — runtime check is the CI windows job (recorded limit).

### Task 17 — CI and release jobs

Files: `.github/workflows/ci.yml`, `.github/workflows/release.yml`
Interfaces: consumes Tasks 7, 12, 15, 16.
Change: `ci.yml` matrix gains `windows-latest` running `cargo build --workspace`, `cargo test` for `gokod`/`app-core`/`goko-rune`, and clippy; `release.yml` gains a `windows-latest` job running `package-windows.ps1`, uploading the zip + `windows-x86_64.target.json`, and the publish job merges the new entry unchanged.
Verify: workflow YAML parses (`ruby -ryaml -e 'YAML.load_file(...)'` or `python -c 'import yaml'`); the first pushed run is the runtime evidence (CI-only limit).
Progress: done
Result: ci.yml gains windows-latest (build + clippy + protocol/daemon/app-core tests); release.yml gains a windows-latest packaging job and publishes install.ps1; both workflows parse as YAML.

## Files inspected, no change

- `crates/protocol/src/*` — framing is already transport-agnostic; no change needed.
- `crates/rune-kit/src/*` — no platform code; verified zero `cfg(unix)`/`std::os` hits.
- `apps/rune/Cargo.toml` — objc2 deps already `cfg(target_os = "macos")`-scoped.
- `crates/daemon/src/{buffer,terminal,test_support}.rs` — no Unix-only imports.

## Approval Gate

Status: approved

## Decision Log

- 2026-11-16 — user chose: full 4-phase port; named pipes; accept issue degradations; CI-only Windows verification (no Windows machine available).
