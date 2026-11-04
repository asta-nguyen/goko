# Architecture overview

Goko is a macOS-first desktop app for running interactive command-line agents
per project. A Tauri 2 / React app talks to a separate Rust daemon (`gokod`)
over a Unix socket; the daemon owns every PTY and child process. Project and
session metadata lives in an app-owned SQLite database.

## Process topology

```text
apps/desktop (Tauri app)                        crates/daemon (gokod)
┌──────────────────────────────┐  daemon.sock   ┌────────────────────────────┐
│ React UI ──invoke/Channel──▶ │  (protocol     │ server ─▶ SessionManager   │
│  src-tauri bridge/client     │   frames)      │   └─▶ PtyProcess ─▶ child  │
└──────────────────────────────┘                └────────────────────────────┘
```

- `gokod` is a standalone process. It takes an exclusive `flock` on
  `daemon.lock`, writes a fresh token to `daemon.token`, serves `daemon.sock`,
  logs to `daemon.log`, and shuts down its sessions on SIGTERM/SIGINT or an
  authenticated `system.shutdown` request. A second `gokod` prints
  `gokod: already running` and exits 0. `--version` prints the version; any
  other argument prints usage and exits 2.
- The support directory is `$GOKO_HOME`, else
  `~/Library/Application Support/Goko`, created mode 0700. It also holds
  `worktree-operations.lock` (a shared/exclusive flock serializing session
  creation against worktree removal) and the app-owned `goko.db`.
- The Tauri backend (`src-tauri`) is the only socket client; the React UI
  reaches it through `invoke` commands, Tauri events, and a `Channel` carrying
  terminal output bytes.

## Graceful app shutdown

Closing the main window or choosing Cmd+Q prevents Tauri from exiting until its
shared shutdown coordinator finishes. The bridge sends the authenticated
`system.shutdown` request, then waits for `gokod` to release `daemon.lock`.
The daemon stops accepting work, and `SessionManager::shutdown()` sends SIGTERM
to each live PTY process group, waits up to two seconds, then sends SIGKILL to
remaining processes. Goko exits after the daemon has stopped; if no daemon is
running it exits normally, while a shutdown error or timeout shows a native
error dialog and leaves the app open for retry. Normal quit preserves
`GOKO_HOME`, including `goko.db`; Force Quit and crashes do not guarantee this
cleanup.

## Domains

| Domain | Location | Scope |
|---|---|---|
| Wire protocol | `crates/protocol/` | Frame codec (`u32` length, `u8` kind, ≤1 MiB payload), JSON message schema (requests, responses, events, error codes), `SessionId`/`SessionInfo`/`SessionState` types |
| Daemon | `crates/daemon/` | `gokod` binary: instance lock, token auth, socket server, request dispatch, session state machine, PTY spawn/signal/resize/live-CWD, 1 MiB replay buffer with subscriber streaming |
| Desktop bridge | `apps/desktop/src-tauri/` | Daemon connect-or-spawn client and handshake, `Bridge` request API, event pump to Tauri events, terminal `Channel`, SQLite `goko.db` store (projects, session metadata, settings), agent discovery, worktree/env-file commands, `git_view_*` editor commands |
| Desktop UI | `apps/desktop/src/` | React 19 + TanStack Query + zustand: project-tree sidebar, terminal workspaces (tabs and split panes over xterm), session controls, Editor workspace (Files, Changes, History, Stashes), daemon-event → query-cache sync |

## Cross-domain dependencies

- `src-tauri` and `gokod` both depend on `crates/protocol` for frames and
  messages; the UI never sees the socket.
- The UI calls only Tauri commands (`src/services/call.ts` wraps `invoke` and
  normalizes errors to `BridgeError`) and listens for `session://updated`,
  `session://exited`, and `daemon://status` events, which `services/sync.ts`
  writes into the query cache.
- Session metadata in SQLite is keyed by the daemon's session UUID; daemon
  session state itself is in-memory only.
- Git/worktree operations run `git` via `Command::new("git")` with argv and
  `-C`; no shell strings are built anywhere.

## Tests

- Rust: `cargo test --workspace` — protocol unit tests plus daemon integration
  tests under `crates/daemon/tests/` that spawn the real `gokod` in a temp
  `GOKO_HOME`; `src-tauri` unit and wiring tests.
- UI: `pnpm -C apps/desktop test` — Vitest + Testing Library.

## Sources

- `Cargo.toml`
- `crates/protocol/src/lib.rs`
- `crates/protocol/src/frame.rs`
- `crates/protocol/src/message.rs`
- `crates/protocol/src/session.rs`
- `crates/daemon/src/lib.rs`
- `crates/daemon/src/main.rs`
- `crates/daemon/src/session.rs`
- `crates/daemon/src/server.rs`
- `crates/daemon/tests/e2e.rs`
- `crates/daemon/src/paths.rs`
- `apps/desktop/src-tauri/src/lib.rs`
- `apps/desktop/src-tauri/src/bridge.rs`
- `apps/desktop/src-tauri/src/commands.rs`
- `apps/desktop/src-tauri/src/daemon_client.rs`
- `apps/desktop/src-tauri/src/store.rs`
- `apps/desktop/src/App.tsx`
- `apps/desktop/src/main.tsx`
- `apps/desktop/src/services/call.ts`
- `apps/desktop/src/services/sync.ts`
- `apps/desktop/src/stores/ui.ts`
- `apps/desktop/package.json`
- `README.md`
