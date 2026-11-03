# goko roadmap

Source: the master prompt (Tauri 2 + Rust daemon + React/xterm multi-agent
coding workspace, macOS arm64 first). The full scope is too large for one design,
so it is split into sub-projects. Each gets its own design → tasks → implementation
cycle; only S1 is written out. Start S(n+1) only after S(n) is archived.

Project name: `goko` (same as the folder).

| # | Sub-project | Delivers | Exit check |
|---|---|---|---|
| S1 | [Vertical slice](s1-contract.md), built as 13 small plans below | Cargo + pnpm workspace, `protocol` crate, `gokod` daemon (single instance, authed socket, PTY manager, bounded replay buffer, in-memory sessions), Tauri shell that spawns/connects to the daemon, one xterm view that runs a real command | Run `vim`/`less`/`sh` in the app, type, resize, Ctrl-C, stop; daemon tests green |
| S2 | [Projects, agents, sessions](changes/archive/2026-10-30-s2-projects-agents-sessions/design.md) | SQLite (minimal schema + migrations: projects, sessions, settings), agent adapters (Claude Code, Codex, OpenCode, generic) with discovery, project import, sidebar, multiple concurrent sessions | Two different real agents run side by side, switching tabs does not stop either |
| S3 | Git | Repo validation, worktree create/list/remove with dirty/running guards, git status, read-only workspace Editor (checkout-scoped Files and Changes diff viewer with binary/large fallbacks, History graph, repository-shared Stashes inspection), session ↔ worktree link | Two sessions in two worktrees; diffs show real, isolated changes; dirty removal refused |
| S4 | Background sessions | Detached-daemon lifecycle, GUI detach/reattach, headless terminal emulator (`vt100`) for correct TUI snapshots, state reconciliation after daemon crash/reboot (`interrupted`), reconnect backoff | Quit GUI, reopen, reattach to a live Claude Code TUI with the screen intact |
| S5 | UX and performance | Command palette, shortcuts, terminal search, settings screens, empty/error states, notifications, benchmarks and `PERFORMANCE.md` | Documented RSS/CPU numbers; no UI jank under heavy output |
| S6 | Release | Signed-or-honestly-unsigned arm64 bundle, README/ARCHITECTURE/DEVELOPMENT/SECURITY docs, troubleshooting | Fresh-machine install and the "definition of success" walkthrough |

## S1 plans

Shared wire and behavior contract: [s1-contract.md](s1-contract.md). Each plan is a
change folder with its own `design.md` and `tasks.md`, built in this order; a plan
starts only after its dependencies are archived.

| Plan | Delivers | Depends on | Tasks |
|---|---|---|---|
| [s1-01-repo-init](changes/archive/s1-01-repo-init/design.md) | git, pnpm and Cargo workspaces, crate skeletons | none | 2 |
| [s1-02-ui-shell](changes/archive/s1-02-ui-shell/design.md) | Vite/React/Tailwind scaffold, theme tokens, Tauri window | 01 | 3 |
| [s1-03-protocol](changes/archive/s1-03-protocol/design.md) | frame codec, session types, messages | 01 | 3 |
| [s1-04-daemon-server](changes/archive/s1-04-daemon-server/design.md) | paths, single-instance lock, token, authed socket server, `gokod` main | 01, 03 | 5 |
| [s1-05-pty-core](changes/archive/s1-05-pty-core/design.md) | ring buffer, PTY spawn/write/resize, process-group signals | 01 | 4 |
| [s1-06-pty-streaming](changes/archive/s1-06-pty-streaming/design.md) | gap-free subscribe, slow-consumer drop | 05 | 2 |
| [s1-07-sessions](changes/archive/s1-07-sessions/design.md) | state machine, `SessionManager`, shutdown | 03, 05, 06 | 3 |
| [s1-08-daemon-ipc](changes/archive/s1-08-daemon-ipc/design.md) | `session.*` / `terminal.*` over the socket, daemon e2e test | 04, 06, 07 | 4 |
| [s1-09-tauri-bridge](changes/archive/s1-09-tauri-bridge/design.md) | daemon client, spawn + reconnect, commands, events, terminal Channel | 02, 03, 08 | 5 |
| [s1-10-ui-state](changes/archive/s1-10-ui-state/design.md) | Vitest, typed service, TanStack Query cache with event sync, UI store | 02, 09 | 4 |
| [s1-11-ui-terminal](changes/archive/s1-11-ui-terminal/design.md) | `TerminalController`, `TerminalView` | 10 | 2 |
| [s1-12-ui-session](changes/archive/s1-12-ui-session/design.md) | new-session form, controls, session list, app composition | 09, 10, 11 | 3 |
| [s1-13-acceptance](changes/archive/s1-13-acceptance/design.md) | manual checklist, `ARCHITECTURE.md` | all | 1 |

Parallel tracks once s1-01 is done: protocol → daemon (03 → 04, and 05 → 06 → 07 → 08)
and UI shell (02). The bridge (09) is the first plan that needs both sides.

## Ordering decisions

- SQLite moves from the prompt's Phase 4 to S2: session ids and schema are needed
  as soon as more than one session exists, and retrofitting them is rework.
- Terminal emulator snapshots land in S4, not S1: raw byte replay corrupts
  alt-screen TUIs, so S1 documents that limit instead of hiding it.
- GUI E2E on macOS is Rust-level (test client against the daemon) plus a manual
  checklist: `tauri-driver` has no WKWebView driver on macOS.
- Real agents are manual smoke tests only; automated tests use `cat`, `sh`, `python`.
- TanStack Query (user decision) holds daemon data from S1 and gets pushed events written into its cache; Zustand holds UI-only state. S3 reuses Query for git status and diffs.
