# Sub-agent indicator tasks

## Design

[Sub-agent indicator on session rows](design.md)

## Global Constraints

- The only protocol or daemon change is the additive `SessionInfo.pid: Option<u32>`, serialized as `pid` with `#[serde(default)]`; no request, response, or event schema changes.
- The daemon sets `pid` when a session becomes `running` (the PTY child's pid) and leaves it `None` before spawn and for a failed spawn.
- Detection runs every 2 s and only when at least one running session has a pid; it reads the process table once per tick with `ps -axww -o pid=,ppid=,command=` started with an argv and no shell.
- A sub-agent is a descendant of a running session's pid at any depth (never the session process itself) whose program is `claude`, `codex`, `opencode`, or `devin`: the file stem of the first word of its command, or of the first word after it when the first word is `node` or `bun`. Wrappers such as `zsh -c … devin …` do not match.
- The map is `{goko_session_id: [{agent, pid, command}]}` with `command` truncated to 200 characters; only `running` sessions with a pid and at least one sub-agent have an entry; `subagents://updated` carries the full map and is sent only when it changed; `subagents_get` returns the current map; a failed `ps` or session listing keeps the previous map.
- Indicator text: `↳ <agent>` for one, `↳ <agent> ×N` for N of the same agent, agents joined by `, ` in alphabetical order; muted, small, truncating before the context percentage; title and accessible name list `<agent> (pid <pid>): <command>` per sub-agent, the accessible name prefixed `Running sub-agents:`. A row without sub-agents is unchanged.
- No nested rows, no output, no kill action, no history; the indicator is shown only while the process runs.

## Impact map

Entry: `crates/daemon/src/session.rs::SessionManager::create` (where a session becomes `running`) and `apps/desktop/src-tauri/src/activity.rs::spawn_poller` (the poller pattern to mirror); `apps/desktop/src/features/sessions/SessionItems.tsx::SessionRow` (where the indicator renders)
Flow: `session_create` → bridge → daemon `PtyProcess::spawn` (`PtyProcess::pid()` already exists) → `SessionInfo` broadcast (`session.updated`) and `session.list` → app `Bridge::session_list`. New: `subagents::spawn_poller` → `session_list` → one `ps` per tick → descendants by `ppid` → `subagents://updated` → `services/sync.ts` → `queryKeys.subagents` → `useSessionSubagents` → `SessionRow`
State changes: none persisted; the map lives in memory in the app and the query cache
External effects: a `ps` child process every 2 s while a running session has a pid; no writes
Change candidates: `crates/protocol/src/session.rs`, `crates/protocol/src/message.rs` (test fixture), `crates/daemon/src/session.rs`, daemon tests under `crates/daemon/tests/`, new `apps/desktop/src-tauri/src/subagents.rs`, `apps/desktop/src-tauri/src/lib.rs`, `apps/desktop/src-tauri/src/wiring_tests.rs`, `SessionInfo` literals in `apps/desktop/src-tauri/src/activity.rs` and `apps/desktop/src-tauri/src/commands/tests.rs`, `apps/desktop/src/services/types.ts`, new `apps/desktop/src/services/subagents.ts`, `apps/desktop/src/services/sync.ts`, `apps/desktop/src/queryClient.ts`, new `apps/desktop/src/features/subagents/queries.ts`, `apps/desktop/src/features/sessions/SessionItems.tsx` and its test, `ARCHITECTURE.md`, `docs/llm/architecture/overview.md`
Verification: `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`, `pnpm -C apps/desktop typecheck`, `pnpm -C apps/desktop test`, then manual on `pnpm -C apps/desktop tauri dev`
Verified at: e2b66dadcbc16917838b4a93c2da85582f415875

## Tasks

Tasks run in order.

### Task 1 — `SessionInfo.pid` in protocol and daemon

Files: `crates/protocol/src/session.rs::SessionInfo`, `crates/protocol/src/message.rs` (test fixture `session()`), `crates/daemon/src/session.rs::SessionManager::create`, `apps/desktop/src-tauri/src/activity.rs` (test helper `running`), `apps/desktop/src-tauri/src/commands/tests.rs` (the `SessionInfo` literal), `apps/desktop/src/services/types.ts::SessionInfo`
Interfaces: produces `SessionInfo.pid` (`Option<u32>` in Rust, `pid?: number | null` in TS) for tasks 2 and 3.
Change: Add the field with `#[serde(default)]`. In the daemon, set it from `pty.pid()` inside the `Transition::Spawned` update so the `running` broadcast and every later `list` and `get` carry it; the initial value and the failed-spawn path stay `None`. Update every struct literal and fixture that builds a `SessionInfo`.
Verify: `cargo test --workspace` and `pnpm -C apps/desktop typecheck`; new cases: a session running `sleep` reports `pid` equal to `Some(n)` with `n > 0` in `create`, `list`, and `get`; a session whose command does not exist reports `pid: None`; a JSON `SessionInfo` without a `pid` key deserializes with `pid: None`; a `SessionInfo` with a pid round-trips through JSON.
Progress: done
Result: `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` — 412 passed, no failures (the supervisor's run), `pnpm -C apps/desktop typecheck` clean. Ran by Devin swe-2-flash-max, reviewed and rerun by the supervisor; no review changes needed. New tests: `a_running_session_reports_its_pid` (create, list, get, kept after exit), `a_failed_spawn_reports_no_pid`, `session_info_without_a_pid_key_deserializes_as_none`, `session_info_with_a_pid_round_trips`. Note: before editing, `a_shell_directory_change_updates_session_list_get_and_events` failed once (a cwd-refresh race: `proc_pidinfo` returns nothing for a just-exited process); it is unrelated to this change and passed on every later run.

### Task 2 — Sub-agent detection poller

Files: new `apps/desktop/src-tauri/src/subagents.rs::{Subagent, SubagentState, parse_ps, descendants, agent_of, compute_subagents, spawn_poller, subagents_get}`, `apps/desktop/src-tauri/src/lib.rs` (declare the module, register `subagents_get`, manage the state next to the activity poller), `apps/desktop/src-tauri/src/wiring_tests.rs` (manage the state so the command resolves)
Interfaces: consumes `SessionInfo.pid` and `state` from task 1 and `Bridge::session_list`; produces the `subagents_get` command and the `subagents://updated` event with `{goko_session_id: [{agent, pid, command}]}` for task 3. Mirrors `activity.rs` (`ActivityState`, `apply`-on-change, `Emit`).
Change: `spawn_poller(bridge, emit)` ticks every 2 s, lists sessions, returns early when no running session has a pid, runs `ps` once, and computes the map under the Global Constraints; it emits only when the map changed and keeps the previous map when `ps` or the listing fails. `parse_ps` turns the `ps` text into `(pid, ppid, command)` rows, skipping lines it cannot parse.
Verify: `cargo test -p desktop -- subagents` plus `cargo fmt --check` and `cargo clippy --workspace -- -D warnings`; named cases: `parse_ps` reads a row whose command has spaces and skips an empty line and a malformed line; `descendants` finds children at depth 1 and depth 3, never the root, and survives a ppid cycle; `agent_of` matches `devin --model x`, `/opt/homebrew/Caskroom/claude-code/2.1.311/claude --resume y`, `node /usr/lib/codex.js`, and `bun /x/opencode`, and does not match `zsh -c eval 'devin …'`, `node server.js`, or `caffeinate -i`; a grouped map for two sessions with one and two sub-agents; a session that is not running or has no pid has no entry; the same input twice emits once; a failing source keeps the previous map; `subagents_get` is resolvable in the wiring test.
Progress: done
Result: `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test -p desktop -- subagents wiring` — 20 passed, `cargo test --workspace` — no failures (desktop 298). Ran by Devin swe-2-flash-max, reviewed and rerun by the supervisor; no review changes needed. One justified deviation: `next_map` is `async` (it awaits the `tokio::process` read of `ps` through a closure returning a future) instead of taking a synchronous closure; every specified behavior is kept and tested, including that the table is not read when no running session has a pid. `descendants` seeds its visited set with the root so a cycle can never return the session's own process; commands are truncated by characters.

### Task 3 — Indicator on session rows

Files: new `apps/desktop/src/services/subagents.ts::{Subagent, SubagentMap, subagentsService}`, `apps/desktop/src/services/sync.ts::startQuerySync`, `apps/desktop/src/queryClient.ts::queryKeys`, new `apps/desktop/src/features/subagents/queries.ts::{useSubagents, useSessionSubagents}`, `apps/desktop/src/features/sessions/SessionItems.tsx::SessionRow`, tests `apps/desktop/src/features/subagents/queries.test.tsx`, `apps/desktop/src/features/sessions/SessionItems.test.tsx`, `apps/desktop/src/services/sync.test.tsx`
Interfaces: consumes `subagents_get` and the `subagents://updated` event from task 2; mirrors `services/activity.ts`, `features/activity/queries.ts`, and the activity listener in `sync.ts`.
Change: `useSessionSubagents(session)` returns the session's sub-agents while it is `running`, else an empty list. `SessionRow` renders the indicator described in the Global Constraints after the title and before the context percentage, with the title attribute and accessible name.
Verify: `pnpm -C apps/desktop typecheck` and `pnpm -C apps/desktop test`; new cases: one `devin` renders `↳ devin`; two `devin` render `↳ devin ×2`; `codex` and two `devin` render `↳ codex, devin ×2`; the title and accessible name list `devin (pid 123): <command>` with the `Running sub-agents:` prefix; a session with no entry or not running shows no indicator and its row is otherwise unchanged; `useSessionSubagents` hydrates from `subagents_get` and updates from `subagents://updated`; `startQuerySync` writes the event into the cache.
Progress: done
Result: `pnpm -C apps/desktop typecheck` clean, `pnpm -C apps/desktop test` — 46 files, 499 tests passed (489 before). Ran by Devin swe-2-flash-max (the first attempt died on a transient Devin connection error before editing anything and was rerun), reviewed and rerun by the supervisor; no review changes needed. Deviation: `App.test.tsx` was also edited, outside the task's file list, because it asserts the total number of event listeners and `startQuerySync` now registers one more (`6` became `7` in four waits); nothing else in it changed. Built as `subagentsService`, `useSubagents`, `useSessionSubagents` (a shared empty array for non-running sessions), and a module-private `SubagentIndicator` (aria-hidden `↳ …` text, `title`, and an `sr-only` `Running sub-agents:` list).

### Task 4 — Documentation

Files: `ARCHITECTURE.md` (the Sessions section and a short sub-agent paragraph), `docs/llm/architecture/overview.md` (Domains table, a sub-agent paragraph beside Agent activity, Sources)
Interfaces: consumes the merged behavior of tasks 1 to 3; `document-wiki`'s `## Task updates` owns the procedure. There is no `delta.md`: no existing wiki requirement covers sessions or activity, so this is scoped coverage, not an `ADDED` requirement.
Change: Document `SessionInfo.pid`, the 2 s `ps` poller, the agent-name matcher and its limits, the `subagents://updated` event and `subagents_get`, and the row indicator, in the smallest relevant places.
Verify: `node …/validate-llm-wiki.mjs` reports 0 errors and 0 warnings, `git diff --check` is clean, and every claim added to the wiki is checked against the source and tests of tasks 1 to 3.
Progress: done
Result: `node …/validate-llm-wiki.mjs` — 7 files, 19 requirements, 0 errors, 0 warnings; `git diff --check` clean. Added a "Sub-agent indicator" section (the `pid` field, the 2 s `ps` poller, the agent-name matcher, the map and event, the row indicator, and the limits) to `docs/llm/architecture/overview.md` and to `ARCHITECTURE.md`, plus the `SessionInfo.pid` bullet in its Sessions section, the new event and command in the overview's cross-domain list, the domain rows, and Sources. Each claim was written from the code and tests read in tasks 1 to 3. No delta: no existing requirement covers sessions or activity.

### Task 5 — Manual check on the running app

Files: none
Interfaces: consumes everything above.
Change: On `pnpm -C apps/desktop tauri dev`, open a Claude session, ask it to run a long `devin` command through its shell tool, and watch the sidebar row.
Verify: the row shows `↳ devin` while the command runs, hovering it lists the pid and command, and the indicator disappears within about 4 s of the command finishing; a plain Shell session running `devin` by hand shows the same indicator; a session with no sub-agents shows nothing. Any miss is recorded as a failure with the observation.
Progress: done
Result: The user ran the manual check on the running app and reported it fine on 2026-11-09 ("ok ổn đó"). The supervisor did not observe the row indicator itself; the real-daemon test of Task 6 covers detection and removal with the real `ps`, and the UI cases are covered by Vitest.

### Task 6 — End-to-end sub-agent detection test

Files: `apps/desktop/src-tauri/src/subagents.rs` (tests only)
Interfaces: consumes tasks 1 and 2 and the real `gokod` started by `test_support::Home`.
Change: A test only, no production change. A copy of `sleep` named `devin`, started in the background under a real session, is reported by `next_map` fed from the real `Bridge::session_list` and the real `process_table`, and is gone once the session is stopped. A production bug it exposes is reported, not fixed here.
Verify: `cargo test -p desktop -- subagents` and `cargo test --workspace`; named case: a running session that started `<tmp>/bin/devin 20` has one sub-agent `devin` whose command contains `/devin 20`, and after a forced stop and exit the map has no entry for that session.
Progress: done
Result: `cargo test -p desktop -- a_real_devin` passed and `cargo test --workspace` — 427 passed (Devin's run and the supervisor's rerun). `a_real_devin_process_under_a_session_is_reported_and_then_dropped` runs against the real `gokod` and the real `ps`: `next_map` reports exactly one `devin` sub-agent under the live session, and after `session_stop(id, true)` the entry is gone and the child pid no longer exists. Deviation: copied Apple platform binaries are SIGKILLed at exec on this macOS (AMFI launch constraint; the supervisor reproduced exit 137 for a copied `sleep`), so the session runs `sh -c 'exec -a <tmp>/bin/devin /bin/sleep 20 & wait'`: the real `sleep` runs with `argv[0]` `<tmp>/bin/devin`, which is exactly what `ps` and `agent_of` see, so no assertion changed. Test only; no production file changed.

## Files inspected, no change

- `crates/daemon/src/pty.rs` — `PtyProcess::pid()` already exposes the child's pid (and the child is its own session and process-group leader), so the daemon needs only to copy it into `SessionInfo`.
- `apps/desktop/src-tauri/src/activity.rs` — the poller shape (`spawn_poller`, `ActivityState`, emit on change) that `subagents.rs` mirrors.
- `apps/desktop/src/features/activity/queries.ts`, `apps/desktop/src/services/activity.ts` — the TS counterpart pattern.
- `apps/desktop/src/services/sync.ts` — where the event listener is added.

## Approval Gate

Status: approved

## Decision Log

None.
