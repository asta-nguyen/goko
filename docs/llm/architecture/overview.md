# Architecture overview

Goko is a macOS-first desktop app for running interactive command-line agents
per project. A Tauri 2 / React app talks to a separate Rust daemon (`gokod`)
over a Unix socket; the daemon owns every PTY and child process. Project and
session metadata lives in an app-owned SQLite database. A second frontend, the
native Rune shell (`apps/rune`), shares the Tauri-free `crates/app-core`
backend for project, Git, file, ACP, and language-server operations; its
terminal/session chrome is still mock data pending the live-terminal
milestone.

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
- Claude usage captures live at `usage/claude/<claude-session-id>.json` and Claude
  activity words at `activity/claude/<claude-session-id>` under that directory; files
  older than seven days are pruned when the app starts.
- The Tauri backend (`src-tauri`) is the only socket client; the React UI
  reaches it through `invoke` commands, Tauri events, and a `Channel` carrying
  terminal output bytes.
- `apps/rune` (`goko-rune`, Rune/wgpu/winit) is a separate native process that
  never connects to `gokod`: its terminal and session chrome render mock data,
  and its editor workspace uses `crates/app-core` plus `goko.db` directly —
  see [Rune editor](../domains/rune-editor.md) and
  [Rune chrome](../domains/rune-chrome.md).

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
| Desktop bridge | `apps/desktop/src-tauri/` | Daemon connect-or-spawn client and handshake, `Bridge` request API, event pump to Tauri events, terminal `Channel`, SQLite `goko.db` store (projects, session metadata, settings), agent discovery and launch, usage polling and `usage_get`, agent activity polling and `activity_get`, worktree/env-file commands; editor `git_view_*`/`git_write_*`/`acp_*` commands delegate to the shared `app-core` implementations; see [Editor requirements](editor.md#edt-content-search) |
| Desktop UI | `apps/desktop/src/` | React 19 + TanStack Query + zustand: project-tree sidebar, terminal workspaces (tabs and split panes over xterm), session controls, Editor workspace (Files, Changes, History, Stashes), provider usage chips, Claude context percentages, working/waiting/done session dots; Files uses CodeMirror, persisted optional Vim mode, and content search with line navigation; see [Editor requirements](editor.md#edt-codemirror-surface) |
| Shared app core | `crates/app-core/` | Tauri-free backend shared by the desktop commands and the native Rune editor: typed errors, `goko.db` store and migrations, registered-project/checkout validation, bounded Git/file reads and writes, worktree discovery/create/env-copy, ACP hub and agent config, LSP hub, the derived Context search index (`code_index`), `GOKO_HOME` resolution |
| Native shell | `apps/rune/` | `goko-rune` (Rune/wgpu/winit, `rune-kit` pinned git rev): mock terminal/session chrome plus the native editor workspace — Files, Changes, History, Stashes, Servers — over `app-core` on a bounded worker thread (plus the Context search indexer thread) with owned ACP and LSP runtimes; see [Rune editor](../domains/rune-editor.md) and [Rune chrome](../domains/rune-chrome.md) |

## Cross-domain dependencies

- `src-tauri` and `gokod` both depend on `crates/protocol` for frames and
  messages; the UI never sees the socket.
- The UI calls only Tauri commands (`src/services/call.ts` wraps `invoke` and
  normalizes errors to `BridgeError`) and listens for `session://updated`,
  `session://exited`, `daemon://status`, `usage://updated`, and `activity://updated`
  events, which `services/sync.ts` writes into the query cache. `usage_get` and
  `activity_get` supply the initial snapshots.
- Session metadata in SQLite is keyed by the daemon's session UUID; daemon
  session state itself is in-memory only.
- Git/worktree operations run `git` via `Command::new("git")` with argv and
  `-C`; no shell strings are built anywhere.
- `apps/desktop/src-tauri` and `apps/rune` both depend on `crates/app-core`
  for editor/storage/Git/ACP/LSP behavior; the desktop keeps thin Tauri
  command adapters, while `goko-rune` calls `app-core` from its own worker
  thread.
- `goko-rune` shares `goko.db` (projects, settings, session metadata tables)
  but never opens `daemon.sock`; its terminal chrome is mock-only until the
  live-terminal milestone.

## Agent usage indicators

When the app launches Claude, the desktop backend supplies a UUIDv4 with
`--session-id` and inline `--settings` that wraps the Claude statusline command.
The wrapper saves Claude's statusline JSON to
`$GOKO_HOME/usage/claude/<claude-session-id>.json` and forwards it to the
`statusLine.command` found in `$CLAUDE_CONFIG_DIR/settings.json` (default
`~/.claude/settings.json`), when set. Claude's context percentage is associated
with its session id; account limits come from the most recently modified
capture file that reports them, so a newer session without `rate_limits` yet
does not hide them.

At startup the backend reads a usage snapshot, then polls every five seconds.
Claude data comes from the capture files. Codex data comes from the last 64 KiB
of the newest modified `rollout-*.jsonl` in the two newest day directories
under `$CODEX_HOME/sessions`, or `~/.codex/sessions` when `CODEX_HOME` is
unset; the latest `token_count` event
provides its primary and secondary rate limits. Changed snapshots are sent in
`usage://updated`. The header shows known provider limits with reset time and
data age in a tooltip, and a Claude session row shows its known context
percentage. Expired limits display `reset`.

## Agent activity

A running Claude or Codex session shows whether its agent is working, waiting for the
user, or done. `activity.rs` ticks every second, lists sessions through the bridge
(`session_list`), and keeps only sessions in state `running`. It sends the full map of
Goko session id to `working | waiting | done` in `activity://updated` only when it
changed; `activity_get` returns the current map. If the daemon list fails, or a source
cannot be read this tick, the previous value is kept.

- Claude: the launch `--settings` also carries `hooks`. `UserPromptSubmit` and
  `PostToolUse` write `working`, `Notification` writes `waiting`, `Stop` writes `done` to
  `$GOKO_HOME/activity/claude/<claude-session-id>`; each hook command is silent and
  always exits 0. A session is recognized by `--session-id` in its argv.
- Claude interrupts run no hook. `terminal_write` notes a lone Esc (`\x1b`) or input with
  Ctrl+C (`\x03`); if that session's delivered terminal output contains `Interrupted`
  within 2 s, the interrupt time is recorded and a `working` hook file written before it
  reads as `done`. Esc that only closes a menu prints no `Interrupted` and changes nothing.
- Codex: a session whose argv is a lone path with stem `codex` is matched to a rollout
  (`rollout-*.jsonl` in the two newest day directories under `$CODEX_HOME/sessions`, or
  `~/.codex/sessions`) with the same `cwd` as the rollout's `session_meta`, started no
  earlier than 5 s before the session. Sessions claim in start order and a rollout is
  claimed once. The last `task_started`, `task_complete`, or `turn_aborted` in the final
  64 KiB gives `working`, `done`, or `done`. Codex has no waiting event, so an approval prompt stays `working`,
  and two sessions in one directory can be mismatched when a rollout is missing.
- Shell, OpenCode, and Devin sessions have no activity and keep the plain `running` dot.

The sidebar dot for a `running` session with an activity pulses in the warning tone
(`working`), is danger-toned (`waiting`), or success-toned (`done`); its title reads
`running · <activity>`. Other states render as before.

## Tests

- Rust: `cargo test --workspace` — protocol unit tests, daemon integration
  tests under `crates/daemon/tests/` that spawn the real `gokod` in a temp
  `GOKO_HOME`, `app-core` unit tests (including the shared fake-ACP-agent
  fixture), `src-tauri` unit and wiring tests, and `goko-rune` unit and
  scene tests under `apps/rune/src`.
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
- `apps/desktop/src-tauri/src/agents.rs`
- `apps/desktop/src-tauri/src/usage.rs`
- `apps/desktop/src-tauri/src/activity.rs`
- `apps/desktop/src-tauri/src/terminal.rs`
- `apps/desktop/src/features/terminal/TerminalController.ts`
- `apps/desktop/src-tauri/src/wiring_tests.rs`
- `apps/desktop/src-tauri/src/store.rs`
- `apps/desktop/src/App.tsx`
- `apps/desktop/src/features/usage/UsageChips.tsx`
- `apps/desktop/src/features/usage/queries.ts`
- `apps/desktop/src/features/activity/queries.ts`
- `apps/desktop/src/features/sessions/StateBadge.tsx`
- `apps/desktop/src/features/sessions/SessionItems.tsx`
- `apps/desktop/src/main.tsx`
- `apps/desktop/src/services/call.ts`
- `apps/desktop/src/services/usage.ts`
- `apps/desktop/src/services/activity.ts`
- `apps/desktop/src/services/sync.ts`
- `apps/desktop/src/features/usage/UsageChips.test.tsx`
- `apps/desktop/src/features/usage/queries.test.tsx`
- `apps/desktop/src/features/activity/queries.test.tsx`
- `apps/desktop/src/features/sessions/StateBadge.test.tsx`
- `apps/desktop/src/features/usage/tone.test.ts`
- `apps/desktop/src/features/sessions/SessionItems.test.tsx`
- `apps/desktop/src/services/sync.test.tsx`
- `apps/desktop/src/stores/ui.ts`
- `apps/desktop/package.json`
- `crates/app-core/src/lib.rs`
- `crates/app-core/Cargo.toml`
- `apps/rune/Cargo.toml`
- `apps/rune/src/main.rs`
- `apps/rune/src/editor/mod.rs`
- `README.md`
