# s1-13 Vertical-slice acceptance tasks

## Design

[s1-13 Vertical-slice acceptance](design.md)

## Global Constraints

- A result is `pass` only if it was observed; anything not run is `not run` with a reason.
- Test directories and sessions are disposable; no real repository is modified.
- Fixes found here carry their own automated test where one is possible.

## Impact map

```text
Entry: apps/desktop/src/features/sessions/NewSessionForm.tsx:9-42 and SessionList.tsx:34-46; selected terminal at apps/desktop/src/App.tsx:11-17; Tauri command registration at apps/desktop/src-tauri/src/lib.rs:17-28
Flow: UI → services/session.ts:13-45 and features/sessions/queries.ts:7-43 → src-tauri/src/commands.rs:23-114,132-182 → Bridge/DaemonClient → crates/daemon/src/server.rs:113-158,199-340 → SessionManager::create → PtyProcess::spawn; terminal output returns via daemon replay/seq → Tauri Channel → TerminalController → xterm
State changes: daemon session state is in memory; Tauri forwards session events into TanStack Query; selected ID is transient Zustand state. Daemon startup creates GOKO_HOME files; session commands run child processes in PTYs.
External effects: runs interactive programs and shell commands in PTYs; Stop/Force kill signals their process groups; daemon launch writes lock, socket, token, and log files.
Change candidates: ARCHITECTURE.md; this task Result; code only if an observed acceptance failure points to it
Verification: cargo test --workspace; pnpm -C apps/desktop test; cargo clippy --workspace -- -D warnings; pnpm -C apps/desktop typecheck; 13-item manual checklist on Tauri dev plus items 1, 2, 9, 12, 13 on target/debug/desktop
Verified at: 014a6f40f2c5d4e9692c32e1652b4782c50b02cb
```

## Tasks

### Task 1 — Run the checklist and write `ARCHITECTURE.md`

Files: ARCHITECTURE.md
Interfaces: consumes everything from s1-01 through s1-12.
Change: run the 13 checklist items from the design on `tauri dev`, repeat items 1, 2, 9, 12, 13 on the debug binary, and record each as pass, fail, or not run with the observation. Fix or document every failure. Write `ARCHITECTURE.md` (topology, framing, PTY ownership, buffer and `seq`, the raw-replay limit, crash behavior from items 12–13) from the code as built, not from the design.
Verify: every checklist item has a recorded observed result in `Result`; `cargo test --workspace && cargo clippy --workspace -- -D warnings && pnpm -C apps/desktop test && pnpm -C apps/desktop typecheck` all pass.
Progress: done
Result:

  **Run setup and baseline**

  - Started `pnpm -C apps/desktop tauri dev`; Vite and `target/debug/desktop` launched, but CUA could not attach to the raw executable. Manual interaction used a temporary `.app` wrapper and separate Vite server. For the debug-binary quit check, a second temporary `.app` pointed directly at `target/debug/desktop`, with `gokod` beside it and its own `GOKO_HOME`. All runs used disposable `/tmp` work directories. This does not verify normally packaged app behavior.
  - Before the first source edit, `cargo test --workspace` passed (135 tests / 21 suites), `pnpm -C apps/desktop test` passed (73 tests / 11 files), `cargo clippy --workspace -- -D warnings` passed, and `pnpm -C apps/desktop typecheck` passed.

  **Dev run: all 13 items**

  1. **FAIL (system-wide process count); scoped start PASS.** The status dot was connected and exactly one daemon held the private test lock. A system-wide `ps` count was greater than one because pre-existing daemons 46016 and 60799 were left untouched.
  2. **PASS.** Chose the temp work directory, started `$SHELL` (`/opt/homebrew/bin/zsh`), saw a prompt, and `ls --color` rendered `color-dir` cyan.
  3. **PASS.** Edited `vim-test.txt`, ran `:wq`, and returned to a clean shell prompt.
  4. **PASS.** `less long.txt` displayed 2,000 lines; Page Down and `/1500` moved to and highlighted line 1500; `q` returned to the shell.
  5. **PASS.** `top` showed live process/time updates; `q` returned to the shell.
  6. **PASS.** `sleep 100`, Ctrl-C, and the prompt returned.
  7. **PASS.** `tput cols; tput lines` changed from 199 × 58 to 97 × 38 after resizing, matching the smaller terminal grid.
  8. **PASS.** Vietnamese, Chinese, and the rocket emoji rendered. Pasting three `echo` lines and pressing Return ran all three commands.
  9. **FAIL (numeric exit code); Force kill subcase PASS.** Stop on `sh -c 'sleep 100'` reached `exited`, but the terminal said `Process exited` with no number. The process ended by SIGTERM, for which S1 records `exit_code: null`. `sh -c 'trap "" TERM; sleep 100'` stayed `stopping`; Force kill appeared after about 5 s and ended it.
  10. **PASS.** A ticking session A kept producing timestamps while hidden behind session B; returning to A displayed the continuous sequence through the hidden interval.
  11. **FAIL.** `yes` made the WebView unresponsive before Ctrl-C could be delivered. CUA reported `AXError.cannotComplete`; after direct SIGINT to the isolated `yes` PID 47061, the UI still showed stale state and WebKit was about 4.2 GB RSS. The temporary app was stopped to release that memory.
  12. **FAIL in the CUA wrapper run.** With a live zsh session, Cmd-Q was followed by daemon shutdown in `daemon.log`; the previous daemon/session were gone on refresh and the relaunch showed an empty list. Because CUA needed a temporary app wrapper, normal packaged-app quit behavior remains unverified.
  13. **PASS for reconnect and empty list; banner NOT RUN.** Killing only the isolated daemon PID 74655 with SIGKILL led to a connected UI and an empty session list from the replacement daemon. The first accessibility read was already connected, so the transient disconnected banner was not observed.

  **Debug-binary repeats: items 1, 2, 9, 12, 13**

  - **1 — FAIL (system-wide process count); scoped start PASS.** Connected status and one daemon under the private debug home; old system daemons remained present and untouched.
  - **2 — PASS.** The debug binary started zsh in the isolated work directory; prompt appeared and `ls --color` rendered `color-dir` cyan.
  - **9 — FAIL (numeric exit code); Force kill subcase PASS.** SIGTERM stop reached `exited` without a number; the TERM-ignoring command remained `stopping` until Force kill appeared after about 5 s and ended it.
  - **12 — PASS for the direct-binary app bundle; wrapper run differed.** With a live zsh session (PID 91479), Cmd-Q closed the app. The app process ended while daemon PID 88966 retained the private `daemon.lock`, the shell stayed alive, and `daemon.log` had no shutdown entry. In the earlier launcher-wrapper run, Cmd-Q was followed by daemon shutdown and a lost session. Normal packaged-app behavior remains unverified.
  - **13 — PASS for reconnect and empty list; banner NOT RUN.** Killed the isolated debug daemon while zsh was live. The app showed connected with no sessions after restart; the transient disconnected banner was not captured.

  The signal exit-code behavior and the high-output UI freeze are documented in `ARCHITECTURE.md`. No production code was changed; adding end-to-end backpressure would change the cross-layer output flow and needs a separate design decision.
  - Fresh verification after the documentation edits: `cargo test --workspace` — 135 passed; `cargo clippy --workspace -- -D warnings` — passed; `pnpm -C apps/desktop test` — 73 passed; `pnpm -C apps/desktop typecheck` — passed.

## Files inspected

- apps/desktop/src/App.tsx
- apps/desktop/src/features/sessions/NewSessionForm.tsx
- apps/desktop/src/features/sessions/SessionControls.tsx
- apps/desktop/src/features/terminal/TerminalView.tsx
- apps/desktop/src/features/terminal/TerminalController.ts
- apps/desktop/src/services/session.ts
- apps/desktop/src/services/sync.ts
- apps/desktop/src-tauri/src/commands.rs
- apps/desktop/src-tauri/src/daemon_client.rs
- apps/desktop/src-tauri/src/terminal.rs
- crates/daemon/src/server.rs
- crates/daemon/src/session.rs
- crates/daemon/src/pty.rs

## Files changed

- ARCHITECTURE.md — recorded the observed stop-code, high-output, app-quit, and daemon-crash behavior.
- docs/agent-devkit/changes/s1-13-acceptance/tasks.md — refreshed the impact map and recorded checklist results.

## Approval Gate

Status: approved

Approved by the user on 2026-10-29: "các tasks được approve hết".

## Decision Log

None.
