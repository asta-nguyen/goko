# Agent activity tasks

## Design

[Agent activity: working, needs you, done](design.md)

## Global Constraints

- No change to `crates/protocol` or `crates/daemon`; `SessionState` is untouched.
- Activity values are exactly `working`, `waiting`, `done`; only a session whose state is `running` carries one.
- Claude hook commands write one word to `<GOKO_HOME>/activity/claude/<claude-session-id>`, print nothing, and always exit 0.
- Hooks: `UserPromptSubmit` and `PostToolUse` → `working`, `Notification` → `waiting`, `Stop` → `done`.
- Claude interrupt: lone `\x1b` or input containing `\x03`, confirmed by `Interrupted` in that session's output within 2 s; a `working` hook file older than the confirmed time reads as `done`.
- Codex `turn_aborted` reads as `done`.
- Codex match: same `cwd`, `session_meta.timestamp` not earlier than `started_at` minus 5 s, each rollout claimed once, sessions claim in `started_at` order, state from the last `task_started` / `task_complete` in the final 64 KiB; no `waiting` for Codex.
- Poller ticks every 1 s and emits `activity://updated` only when the map changes; `activity_get` returns the same map keyed by Goko session id.
- Dot tones for a `running` session: `working` pulsing warning, `waiting` solid danger, `done` solid success, none keeps today's dot; `created`, `starting`, `stopping`, `exited`, `failed` render as today.
- Activity files older than 7 days are pruned at startup with the existing `prune_old`.

## Impact map

Entry: `apps/desktop/src-tauri/src/agents.rs::argv_for` (Claude launch argv with `--settings`) and `apps/desktop/src-tauri/src/usage.rs::spawn_poller` (the polling pattern to mirror)
Flow: `commands.rs::session_create` → `agents::argv_for` → daemon spawns the PTY; separately `lib.rs::run` setup → `usage::spawn_poller` → `poll_once` → `emit("usage://updated")` → `services/sync.ts` listener → `features/usage/queries.ts::useUsage` → `SessionItems.tsx` / `StateBadge.tsx::StateDot`
State changes: new files under `<GOKO_HOME>/activity/claude/`; no database or daemon state
External effects: Claude runs the hook commands on its events; the poller reads Codex rollouts under `CODEX_HOME/sessions` and lists sessions through `Bridge::session_list`
Change candidates: `apps/desktop/src-tauri/src/agents.rs`, new `apps/desktop/src-tauri/src/activity.rs`, `apps/desktop/src-tauri/src/lib.rs`, `apps/desktop/src-tauri/src/commands.rs` (only if `usage.rs` helpers need `pub(crate)`), `apps/desktop/src-tauri/src/usage.rs` (reuse `prune_old`, rollout helpers), `apps/desktop/src/services/{activity.ts,sync.ts,types.ts}`, `apps/desktop/src/queryClient.ts`, new `apps/desktop/src/features/activity/queries.ts`, `apps/desktop/src/features/sessions/{StateBadge.tsx,SessionItems.tsx}`, `ARCHITECTURE.md`, `docs/llm/architecture/overview.md`
Verification: `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`, `pnpm -C apps/desktop typecheck`, Vitest, then manual on the built app
Verified at: 90790097b44c08c884da33fb0523cca44ac38b9a

## Tasks

Execution (AGENTS.md): the coding in each task is delegated to the Devin CLI with the `swe-2-max` model and full permissions; Claude reviews the result and sends fixes back until the task's `Verify` passes. Tasks 2, 3, and 5 do not depend on each other and may run as parallel `swe-2-max` runs; task 4 needs 3, and task 5's manual check needs 4.

### Task 1 — Confirm Claude `--settings` hooks add to the user's hooks

Files: none (spike; findings go in `Result:`)
Interfaces: produces the yes/no that task 2 relies on.
Change: With the installed Claude, run a throwaway `claude -p` using a `--settings` JSON that has a `Stop` hook writing a marker file, in an environment where the user's own settings also define hooks. Record whether both the user's hooks and the `--settings` hook fire, and whether the hook events `UserPromptSubmit`, `Notification`, and `Stop` exist in this version. If `--settings` replaces the user's hooks, stop and return to `brainstorm-feature`.
Verify: marker file exists after the run and the user's own hook output (for example the SessionStart hook) is still present.
Progress: done
Result: `claude -p` 2.1.311 with `--settings` hooks fired `UserPromptSubmit` and `Stop` (marker files written); with a project `.claude/settings.json` Stop hook and a `--settings` Stop hook both fired, so `--settings` adds to user hooks. `Notification` was not triggered (needs a permission prompt): cannot verify here, checked manually in task 6.

### Task 2 — Claude launch injects the activity hooks

Files: `apps/desktop/src-tauri/src/agents.rs::argv_for`, `apps/desktop/src-tauri/src/agents.rs::claude_statusline_command` (pattern for quoting)
Interfaces: consumes the Claude session id already generated in `argv_for`; produces files under `<goko_home>/activity/claude/<session-id>` that task 3 parses.
Change: The `--settings` JSON gains `hooks` for `UserPromptSubmit`, `Notification`, and `Stop`, each a single command that writes the matching word to that file, shell-quoted, silent, exit 0. The activity directory is created like the usage directory.
Verify: update `claude_argv_has_a_v4_session_id_and_capture_settings` and add a test that the settings JSON contains the three hook commands, that each command targets the session's file with the right word, and that a path with a space is quoted; `cargo test -p desktop agents` passes. Running a hook command in `sh` with that path creates the file with the word and exits 0.
Progress: done
Result: `cargo fmt --check`, `cargo clippy -p desktop -- -D warnings`, `cargo test -p desktop agents` — 10 passed, including the hook-injection test and a `sh -c` run with a `goko home` path (file `done`, exit 0, silent). Ran by Devin swe-2-max, reviewed.

### Task 3 — Activity model, parsing, and Codex matching

Files: new `apps/desktop/src-tauri/src/activity.rs::{Activity, parse_claude_activity, codex_activity_from_tail, match_codex_rollouts}`, `apps/desktop/src-tauri/src/usage.rs` (make `prune_old`, `newest_rollout`-style day scanning and `codex_home_from` reachable with `pub(crate)`, no behavior change)
Interfaces: produces the pure functions task 4 calls; consumes `SessionInfo` (`cwd`, `started_at`, `argv`) from `protocol`.
Change: `Activity` serializes as `working | waiting | done`. `parse_claude_activity` maps a file's text (trimmed) to an activity and returns `None` for empty or unknown text. `codex_activity_from_tail` returns the activity of the last `task_started` / `task_complete` line in the given bytes, ignoring a truncated first line. `match_codex_rollouts` takes the running Codex sessions and the candidate rollouts (`path`, `cwd`, `timestamp`) and returns session id → rollout path following the claim rules in Global Constraints.
Verify: Rust unit tests named by case: `"working\n"` → working; `"done"` → done; `""` and `"banana"` → none; a tail ending in `task_complete` → done, ending in `task_started` → working, no task events → none, first line cut mid-JSON → still reads the rest; two sessions with the same cwd claim two rollouts in start order; a rollout earlier than `started_at` minus 5 s is not matched; a different cwd is not matched; a rollout is never claimed twice; `cargo test -p desktop activity` passes.
Progress: done
Result: `cargo fmt --check`, `cargo clippy -p desktop -- -D warnings`, `cargo test -p desktop activity` — 15 activity tests passed (parse, tail, rollout_meta with a >16 KB first line, timestamp parser incl. leap day, claim order, 5 s slack, cwd, single claim, recent_rollouts); `cargo test -p desktop usage` 16 passed. Ran by Devin swe-2-max, reviewed.

### Task 4 — Poller, command, and event

Files: `apps/desktop/src-tauri/src/activity.rs::{spawn_poller, poll_once, activity_get}`, `apps/desktop/src-tauri/src/lib.rs::run` (manage the state, start the poller, register `activity_get`), `apps/desktop/src-tauri/src/wiring_tests.rs`
Interfaces: consumes task 3's functions, `Bridge::session_list`, `Emit`; produces `activity_get` and `activity://updated` carrying `{goko_session_id: "working"|"waiting"|"done"}`.
Change: Each 1 s tick lists sessions, keeps only `running` ones, resolves Claude sessions through `--session-id` → hook file and Codex sessions through the matcher plus the rollout tail, then replaces the stored map and emits only when it differs. A daemon error keeps the previous map. Startup prunes `activity/claude` with the 7-day retention.
Verify: unit tests with a fake session list and temp directories: a changed Claude file changes the map and emits once, an unchanged tick emits nothing, a non-running session is dropped, a list error keeps the previous map; the wiring test invokes `activity_get` and receives a JSON object; `cargo test --workspace` and `cargo clippy --workspace -- -D warnings` pass.
Progress: done
Result: `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` — all pass (desktop lib 156 tests, 11 new in activity/wiring: Claude file follow, keep-previous on missing or garbage file, only running Claude/Codex appear, two Codex sessions in one cwd, apply emits once, prune_activity, `activity_get` returns `{}` through IPC). Written by Claude directly: the `devin swe-2-max` launch for this task was denied by the auto-mode classifier and the user chose "implement đi" without allowing it.

### Task 5 — Sidebar dot shows the activity

Files: new `apps/desktop/src/services/activity.ts`, `apps/desktop/src/services/sync.ts`, `apps/desktop/src/queryClient.ts`, new `apps/desktop/src/features/activity/queries.ts::useActivity`, `apps/desktop/src/features/sessions/StateBadge.tsx::StateDot`, `apps/desktop/src/features/sessions/SessionItems.tsx`
Interfaces: consumes `activity_get` and `activity://updated` from task 4 (type `Record<string, 'working' | 'waiting' | 'done'>`).
Change: `useActivity` hydrates from `activity_get` and updates the cache from the event, the way `usage://updated` is handled. The session row passes its activity to `StateDot`; a `running` session with an activity gets the tone and title from Global Constraints; every other state is unchanged.
Verify: Vitest: `StateDot` for `running` + each activity renders the expected tone class and a title like `running · working`; `running` with no activity and `exited`/`failed`/`starting` render as before; the hook updates when the mocked event fires; `pnpm -C apps/desktop typecheck` and the Vitest suite pass.
Progress: done
Result: `pnpm typecheck` clean, Vitest 291 passed (32 files), including `StateDot` per-activity tests, `useActivity` hydration, event cache update, `SessionItems` title. Ran by Devin swe-2-max, reviewed.

### Task 6 — Docs and manual check on the built app

Files: `ARCHITECTURE.md`, `docs/llm/architecture/overview.md`
Interfaces: consumes the finished behavior of tasks 2 to 5.
Change: Record the activity module, the hook files, and the Codex limits (no `waiting`, same-directory ambiguity) where the agent adapters and usage are described; call `document-wiki`'s `## Task updates` for the overview page. Build with `pnpm tauri build --bundles app` and install to `/Applications` as before.
Verify: on the installed app, a Claude session shows working after a prompt, done after the reply, and waiting on a permission prompt; a Codex session shows working then done; a Shell session keeps its plain dot; `cannot verify` is recorded for any step the user did not observe.
Progress: todo
Result: none

### Task 7 — Interrupts end a turn (Claude keys + output, Codex `turn_aborted`)

Files: `apps/desktop/src-tauri/src/agents.rs::argv_for` (add `PostToolUse` hook), `apps/desktop/src-tauri/src/activity.rs::{Interrupts, compute_activity, codex_activity_from_tail}`, `apps/desktop/src-tauri/src/commands.rs::terminal_write`, `apps/desktop/src-tauri/src/terminal.rs::terminal_subscribe`
Interfaces: consumes `ActivityState` (task 4); `terminal_write` and `terminal_subscribe` gain an `ActivityState` state parameter (no IPC argument change).
Change: per Global Constraints and the design's interrupt rule.
Verify: unit tests: lone Esc then output with `Interrupted` within 2 s turns a `working` file written earlier into `done`; Esc with no `Interrupted` keeps `working`; `Interrupted` without a key, or after 2 s, is ignored; a hook write after the interrupt wins; arrow-key `\x1b[A` is not an interrupt; Ctrl+C counts; Codex tail ending in `turn_aborted` → done; settings JSON has a `PostToolUse` → `working` hook. `cargo test --workspace`, clippy `-D warnings`, fmt pass. Manual: Esc mid-turn turns the dot green within about 1 s; `/` then Esc keeps it yellow.
Progress: done
Result: `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` — 283 passed, 0 failed; new: `codex_tail_ending_in_turn_aborted_is_done`, `esc_confirmed_by_interrupted_output_ends_a_working_turn` (Esc and Ctrl+C), `esc_without_interrupted_output_or_a_non_interrupt_key_keeps_working` (menu Esc, `\x1b[A`, plain key), `interrupted_output_after_the_window_is_ignored`, `a_hook_written_after_the_interrupt_wins`; `PostToolUse` in the hook-injection test. PTY run of Claude 2.1.311: Esc printed `Interrupted · What should Claude do instead?` and ran no hook; `/` then Esc ran `PostToolUse` and `Stop`, no `Interrupted`. Manual check on the app: cannot verify yet.

## Files inspected, no change

- `apps/desktop/src-tauri/src/events.rs` — session events already reach the webview; activity uses its own event.
- `crates/protocol/src/session.rs`, `crates/daemon/src/session.rs` — `waiting_for_input` and `idle` are defined but no transition produces them; left as is.
- `apps/desktop/src/features/usage/queries.ts` — the hydrate-and-listen pattern task 5 copies.

## Approval Gate

Status: approved

## Decision Log

### D1 — Detect interrupts

Question: Claude runs no hook on Esc and Codex records `turn_aborted`, so the dot stayed `working`.
Decision: Claude: lone Esc or Ctrl+C confirmed by `Interrupted` in the output within 2 s; add `PostToolUse` → `working`. Codex: `turn_aborted` → `done`. Reading the Claude transcript was rejected: interactive sessions wrote no `.jsonl` on this machine.
Impact: design Behavior and Edge cases, Global Constraints, new task 7.
Confirmed by user: 2026-11-06

### D2 — Devin stays out of scope

Question: Devin CLI has its own hook system (`.devin/hooks.v1.json`).
Decision: not in this change; injecting hooks per session without editing user config needs its own investigation.
Impact: none for this change.
Confirmed by user: not yet — proposed as a follow-up
