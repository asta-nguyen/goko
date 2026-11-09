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
- Claude usage captures live at `usage/claude/<claude-session-id>.json` (Default account) or
  `usage/claude/<account-id>/<claude-session-id>.json` (a named account), and Claude
  activity words at `activity/claude/<claude-session-id>`, under that directory; files
  older than seven days are pruned when the app starts.
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
| Desktop bridge | `apps/desktop/src-tauri/` | Daemon connect-or-spawn client and handshake, `Bridge` request API, event pump to Tauri events, terminal `Channel`, SQLite `goko.db` store (projects, session metadata, settings), agent discovery and launch, usage polling and `usage_get` (per account), agent accounts and their launch environment (`accounts.rs`; see [Agent accounts](../domains/agent-accounts.md)), agent activity polling and `activity_get`, sub-agent process polling and `subagents_get`, worktree/env-file commands, `git_view_*` editor commands including checkout-scoped content search; see [Editor requirements](editor.md#edt-content-search) |
| Desktop UI | `apps/desktop/src/` | React 19 + TanStack Query + zustand: project-tree sidebar, terminal workspaces (tabs and split panes over xterm), session controls, Editor workspace (Files, Changes, History, Stashes), provider usage chips that switch between accounts, Claude context percentages, working/waiting/done session dots, a `↳ <agent>` indicator while a session runs sub-agent CLIs; Files uses CodeMirror, persisted optional Vim mode, and content search with line navigation; see [Editor requirements](editor.md#edt-codemirror-surface) and [terminal file links](editor.md#edt-terminal-file-link) |

## Terminal links

The terminal scans each row for HTTP(S) URLs and path-like tokens. Only Cmd+click on macOS or Ctrl+click elsewhere opens an HTTP(S) URL in the system browser; the Tauri opener capability allows only `http` and `https`, and trailing sentence punctuation is excluded. URL rows do not trigger file-list reads.

A path is linked only when it resolves to a file returned by the checkout's `git_view_file_list` (tracked or non-ignored untracked files). Absolute paths must be inside the checkout; relative paths are tried from the session's live working directory, then from the checkout root, with `.` and `..` normalized and paths that leave the checkout rejected. The shared query cache keeps the file list fresh for five seconds and reads it only for rows with path candidates. If that read fails, file links are omitted while URL links remain.

Activating a resolved path with the same modifier sends a one-shot request to the Editor for that checkout. The Editor opens Files through its existing navigation guard and reveals `:line` when present. Links are resolved within one terminal row; a path split across rows is not linked.

## Cross-domain dependencies

- `src-tauri` and `gokod` both depend on `crates/protocol` for frames and
  messages; the UI never sees the socket.
- The UI calls only Tauri commands (`src/services/call.ts` wraps `invoke` and
  normalizes errors to `BridgeError`) and listens for `session://updated`,
  `session://exited`, `daemon://status`, `usage://updated`, `activity://updated`, and
  `subagents://updated` events, which `services/sync.ts` writes into the query cache.
  `usage_get`, `activity_get`, and `subagents_get` supply the initial snapshots.
- Session metadata in SQLite is keyed by the daemon's session UUID; daemon
  session state itself is in-memory only.
- Git/worktree operations run `git` via `Command::new("git")` with argv and
  `-C`; no shell strings are built anywhere.

## Agent usage indicators

When the app launches Claude, the desktop backend supplies a UUIDv4 with
`--session-id` and inline `--settings` that wraps the Claude statusline command.
The wrapper saves Claude's statusline JSON to
`$GOKO_HOME/usage/claude/<claude-session-id>.json` and forwards it to the
`statusLine.command` found in the launched account's `settings.json` (the Default account's
folder is `$CLAUDE_CONFIG_DIR`, else `~/.claude`), when set. A named account launches through
`/usr/bin/env CLAUDE_CONFIG_DIR=<dir>` and captures to `usage/claude/<account-id>/` instead
(see [Agent accounts](../domains/agent-accounts.md#acc-launch-account)). Claude's context
percentage is associated with its session id; an account's limits come from the most recently
modified capture file in its folder that reports them, so a newer session without
`rate_limits` yet does not hide them.

At startup the backend reads a usage snapshot, then polls every five seconds.
Claude data comes from the capture files. Codex data comes from the last 64 KiB
of the newest modified `rollout-*.jsonl` in the two newest day directories
under the account's `sessions` folder (the Default account's is
`$CODEX_HOME/sessions`, or `~/.codex/sessions` when `CODEX_HOME` is
unset); the latest `token_count` event
provides its primary and secondary rate limits. Each tick reads every account of both agents:
the snapshot's `claude` and `codex` describe the active account and its `accounts` map holds
every account's limits (see [Agent accounts](../domains/agent-accounts.md#acc-usage-per-account)).
Changed snapshots are sent in `usage://updated`. The header shows known provider limits with reset time and
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
- Codex: a session whose argv is a lone path with stem `codex`, or `env` followed by
  `NAME=value` words and such a path (a named account), is matched to a rollout
  (`rollout-*.jsonl` in the two newest day directories under `$CODEX_HOME/sessions`, or
  `~/.codex/sessions`) with the same `cwd` as the rollout's `session_meta`, started no
  earlier than 5 s before the session. Sessions claim in start order and a rollout is
  claimed once. The last `task_started`, `task_complete`, or `turn_aborted` in the final
  64 KiB gives `working`, `done`, or `done`. Codex has no waiting event, so an approval prompt stays `working`,
  and two sessions in one directory can be mismatched when a rollout is missing.
- Shell, OpenCode, and Devin sessions have no activity and keep the plain `running` dot.

The sidebar marker for a `running` session with an activity is a spinning accent-colored ring
(`working`; a static ring when the system asks for reduced motion), a danger-toned dot
(`waiting`), or a success-toned dot (`done`); its title reads `running · <activity>`. Other
states render as before.

## Sub-agent indicator

A session row shows when its agent has started another agent CLI through its shell tool.
The daemon reports each session's child pid in `SessionInfo.pid`, set when the session becomes
`running` (`None` before the spawn, for a failed spawn, and from an older daemon). `subagents.rs`
ticks every 2 s: it lists sessions through the bridge and, when at least one running session has
a pid, reads the process table once with `ps -axww -o pid=,ppid=,command=`. For each running
session it walks the descendants of that pid and keeps those whose program is `claude`, `codex`,
`opencode`, or `devin` (the file stem of the first word of the command, or of the word after
`node` or `bun`); a `zsh -c … devin …` wrapper does not match. The full map of Goko session id to
`{agent, pid, command}` (the command truncated to 200 characters) is sent in `subagents://updated`
only when it changed, and `subagents_get` returns it; a failed listing or `ps` keeps the previous
map. A sidebar row with sub-agents shows muted `↳ <agent>` text (`×N` for repeats, agents in
alphabetical order, for example `↳ codex, devin ×2`) whose title lists `<agent> (pid <pid>):
<command>` per sub-agent. A running session with sub-agents shows the `working` spinner on its dot even
when its own activity reads `done` or is missing (its turn ended while a sub-agent still runs);
`waiting` keeps priority because it needs the user.

A Codex session's tools run in the shared `codex app-server` daemon, not under the session's pid.
For agent CLIs below a `codex app-server` process, `subagents.rs` reads each one's
`CODEX_THREAD_ID` (`ps -Eww -o pid=,command=`) and maps the thread to a session through the
rollout match `activity.rs` uses (`match_codex_rollouts`; the thread id is the rollout file
stem's last 36 characters), then adds the agent to the same map. It reads nothing extra when no
agent runs below a daemon, and covers only the default Codex home.

Limits: a sub-agent's output is not available (it goes to its parent agent's pipe, not to a Goko
terminal); an agent that runs inside its parent's own process is not a process and is not
detected; a sub-agent that runs for less than one tick can be missed; a program path whose first
word contains a space is split there and can be missed.

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
- `apps/desktop/src-tauri/Cargo.toml`
- `apps/desktop/src-tauri/capabilities/default.json`
- `apps/desktop/src-tauri/src/bridge.rs`
- `apps/desktop/src-tauri/src/commands.rs`
- `apps/desktop/src-tauri/src/daemon_client.rs`
- `apps/desktop/src-tauri/src/accounts.rs`
- `apps/desktop/src-tauri/src/agents.rs`
- `apps/desktop/src-tauri/src/usage.rs`
- `apps/desktop/src-tauri/src/activity.rs`
- `apps/desktop/src-tauri/src/subagents.rs`
- `apps/desktop/src-tauri/src/terminal.rs`
- `apps/desktop/src/features/terminal/TerminalController.ts`
- `apps/desktop/src/features/terminal/TerminalController.test.ts`
- `apps/desktop/src/features/terminal/TerminalView.tsx`
- `apps/desktop/src/features/terminal/TerminalView.test.tsx`
- `apps/desktop/src/features/terminal/TerminalWorkspace.tsx`
- `apps/desktop/src/features/terminal/terminal-links.ts`
- `apps/desktop/src/features/terminal/terminal-links.test.ts`
- `apps/desktop/src/features/editor/EditorWorkspace.tsx`
- `apps/desktop/src/features/editor/EditorWorkspace.test.tsx`
- `apps/desktop/src/features/editor/EditorFiles.tsx`
- `apps/desktop/src/services/git-view.ts`
- `apps/desktop/src/queryClient.ts`
- `apps/desktop/package.json`
- `apps/desktop/src-tauri/src/git_view.rs`
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
- `apps/desktop/src/services/subagents.ts`
- `apps/desktop/src/features/subagents/queries.ts`
- `apps/desktop/src/features/subagents/queries.test.tsx`
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
- `README.md`
