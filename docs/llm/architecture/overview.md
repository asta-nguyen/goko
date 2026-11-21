# Architecture overview

Goko is a native desktop app for running interactive command-line agents by
project. `goko-rune` owns the interface and app metadata; the separate Rust
daemon, `gokod`, owns the terminal sessions, PTYs and child processes. The
app's `crates/app-core` backend provides SQLite storage, project and worktree
operations, Git and file access, ACP and language-server support.

## Process topology

```text
goko-rune
  UI <-> daemon client <-> Unix socket / Windows named pipe <-> gokod
  UI <-> backend worker -> app-core -> goko.db / Git / files
```

- The daemon client connects or starts `gokod`, then authenticates with
  `daemon.token`. Requests and events use `crates/protocol` frames. The
  client runs on its own thread and wakes the UI to apply session events.
- macOS and Linux use `daemon.sock`; Windows uses a named pipe. The app and
  daemon resolve the same `$GOKO_HOME`, which also holds `goko.db`,
  `daemon.lock`, `daemon.token` and `daemon.log`. See
  [Gokod daemon](../domains/gokod-daemon.md) for platform defaults, instance
  locking, and terminal flow control.

## Graceful app shutdown

Normal app quit tries to queue the latest terminal layout, drains accepted
mutating backend jobs, and stops the app's indexer, ACP and LSP runtimes and
daemon client. It does not send `system.shutdown`: running sessions stay with
`gokod`. When the app opens again, it waits for both the stored layout and
the daemon's session list before restoring panes for known sessions.
If `gokod` itself restarts, app metadata remains in SQLite, but it cannot
restore the previous daemon's processes or terminal output.

## Domains

| Domain | Location | Scope |
|---|---|---|
| Native app | `apps/rune/` | Live tabs and split terminals, project sidebar, Settings and [Rune editor](../domains/rune-editor.md); see [Rune chrome](../domains/rune-chrome.md) |
| App core | `crates/app-core/` | SQLite store and migrations, registered project and checkout validation, Git and worktrees, ACP and LSP, usage and activity readers, Context search index |
| Daemon | `crates/daemon/` | Authenticated requests, session lifecycle, PTY ownership, buffered output and terminal snapshots |
| Wire protocol | `crates/protocol/` | Shared framing, requests, responses, events and session types |
| Terminal engine | `crates/vt/` | Ghostty VT bindings used to model terminal output in the daemon and render it in the app |
| UI components | `crates/rune-kit/` | Native Rune components, including the editor's code and diff surfaces |

## Cross-domain dependencies

- The daemon's session list and events supply runtime state. The app joins
  those sessions with SQLite metadata using the daemon session UUID.
- A terminal pane attaches through the daemon client. With a daemon that
  supports terminal snapshots, an attachment can start from a snapshot and
  then stream live output; otherwise it uses raw replay.
- Project, settings and editor operations use a bounded worker queue over
  `app-core`. Context search indexing and ACP and LSP communication run
  separately from the UI.

## Agent usage indicators

The app polls the usage worker about every five seconds. The worker reads
account configuration and usage data, and the header shows known quota
windows and their reset times. See [usage and accounts](../domains/rune-chrome.md#rnc-usage-accounts).

## Agent activity

The native app polls activity on the worker about once a second. `app-core`
derives Claude activity from hook files and Codex activity from rollouts,
then Rune uses the result for sidebar status and
[workspace attention](../domains/rune-chrome.md#rnc-workspace-attention).
The Codex rollout reader does not infer a waiting state from an approval prompt.

## Tests

`cargo test --workspace` includes protocol and app-core unit tests, real-daemon
integration tests, and Rune unit and scene tests. Daemon tests cover snapshot
and live output; pane tests cover layout save retries and restore.

## Sources

- `Cargo.toml`
- `apps/rune/src/daemon.rs`
- `apps/rune/src/editor/backend.rs`
- `apps/rune/src/main.rs`
- `apps/rune/src/panes.rs`
- `apps/rune/src/sessions.rs`
- `apps/rune/src/term.rs`
- `apps/rune/src/view_tests.rs`
- `crates/app-core/src/activity.rs`
- `crates/app-core/src/home.rs`
- `crates/app-core/src/lib.rs`
- `crates/app-core/src/store.rs`
- `crates/daemon/src/main.rs`
- `crates/daemon/src/terminal.rs`
- `crates/daemon/tests/paint.rs`
- `crates/protocol/src/frame.rs`
- `crates/protocol/src/message.rs`
- `crates/vt/src/lib.rs`
