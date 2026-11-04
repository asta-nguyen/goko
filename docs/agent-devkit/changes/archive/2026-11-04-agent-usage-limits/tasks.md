# Agent usage and limit indicators tasks

## Design

[Agent usage and limit indicators](design.md)

## Global Constraints

- No change to `crates/protocol` or `crates/daemon`; the reader lives in `apps/desktop/src-tauri/`.
- Only the `claude` agent launches differently: `[claude, "--session-id", <uuid v4>, "--settings", <json>]`; Shell, Codex, OpenCode, and custom commands launch exactly as today.
- The injected `statusLine` command is `tee <GOKO_HOME>/usage/claude/<uuid>.json | <original>` when `~/.claude/settings.json` has `statusLine.command`, else `tee <path> >/dev/null`. Only the user-level settings file is read for `<original>`; an unreadable or invalid file means no original.
- Missing or unparseable data is "unknown", never zero; a source that fails to read keeps its previous value, shows no error, and logs no file contents.
- Account-wide Claude limits come from the most recently modified `usage/claude/*.json`; per-session context is keyed by the Claude session id recovered from the Goko session's `argv`.
- Codex limits come from the last `token_count` event in the last 64 KiB of the newest `rollout-*.jsonl` under `$CODEX_HOME/sessions` (default `~/.codex/sessions`).
- Polling interval is 5 s; `usage://updated` is emitted only when the snapshot changed.
- A limit whose `resets_at` has passed renders as `reset` (muted), not as its old percentage.
- Colour thresholds for every figure: below 80% muted, 80–89% warning, 90% and above danger.
- `usage/claude/*.json` older than 7 days are deleted at app start.
- No OS notifications, no Codex per-session context, no settings UI or kill switch in this change.

## Impact map

Entry: `apps/desktop/src-tauri/src/commands.rs::resolve_launch` (the one caller of `agents::argv_for`); for the reader, no existing source — planned entry: `apps/desktop/src-tauri/src/usage.rs`

Flow: `session_create` → `resolve_launch` → `agents::argv_for` → `Request::SessionCreate{argv}` → daemon `PtyProcess::spawn` → `claude` runs the injected statusline → file in `GOKO_HOME/usage/claude/`. `usage.rs` poller (5 s) → `UsageSnapshot` → `Emit("usage://updated")` and `usage_get` → `services/sync.ts` / `useUsage` → header chips and `SessionItems` row figure

State changes: new files under `GOKO_HOME/usage/claude/` written by Claude Code, pruned by Goko at start; no SQLite, protocol, or daemon change

External effects: reads `~/.claude/settings.json` and `$CODEX_HOME/sessions` rollout files read-only; changes how `claude` is launched (`--session-id`, `--settings`)

Change candidates: `apps/desktop/src-tauri/src/usage.rs` (new); `apps/desktop/src-tauri/src/agents.rs`; `apps/desktop/src-tauri/src/commands.rs`; `apps/desktop/src-tauri/src/lib.rs`; `apps/desktop/src/services/usage.ts` (new); `apps/desktop/src/services/sync.ts`; `apps/desktop/src/features/usage/` (new); `apps/desktop/src/features/sessions/SessionItems.tsx`; `apps/desktop/src/App.tsx`; their tests; `docs/llm/architecture/overview.md`; `ARCHITECTURE.md`; `CHANGELOG.md`

Verification: `cargo test --workspace`; `cargo fmt --check`; `cargo clippy --workspace -- -D warnings`; `pnpm -C apps/desktop typecheck`; `pnpm -C apps/desktop test`; manual acceptance on Claude Code 2.1.318

Verified at: `89b2f215dc4bd4e6038fcebce01bbdd48a72ea93`

## Tasks

### Task 1 — Parse Claude statusline JSON and Codex rollout tails

Files: `apps/desktop/src-tauri/src/usage.rs::parse_claude`, `usage.rs::parse_codex_tail`, `usage.rs::UsageSnapshot`, `usage.rs::Limit`; register `mod usage` in `apps/desktop/src-tauri/src/lib.rs`
Interfaces: produces the `UsageSnapshot`/`Limit` serde types from the design's Interfaces section (consumed by Tasks 3–5) and two pure parsers.
Change: `parse_claude(&str)` returns `Option` context percentage plus `Option<Limit>` for five-hour (window 300) and seven-day (window 10080), each absent when its field is absent. `parse_codex_tail(&[u8])` drops a possibly-truncated first line, scans lines for the last `token_count` event, and returns `primary`/`secondary` `Limit` (`used_percent`→`used_pct`, `window_minutes`, `resets_at`); no `token_count` or a null `rate_limits` yields none. Neither parser reads or retains any other field.
Verify: Rust unit tests with named cases — full Claude JSON → context 62.0, five-hour 42/resets_at, seven-day 13; Claude JSON without `rate_limits` → context only; Claude JSON with `context_window.used_percentage: null` → no context; non-JSON and half-written JSON → none; Codex tail with two `token_count` lines → the last one's limits (56/300, 13/10080); Codex tail starting mid-line → first line ignored, rest parsed; Codex tail with no `token_count` → none. `cargo test -p desktop usage` passes (package `desktop`).
Progress: done
Result: `cargo test -p desktop usage` — 7 passed, 109 filtered out.

### Task 2 — Launch Claude with `--session-id` and the capture statusline

Files: `apps/desktop/src-tauri/src/agents.rs::argv_for`, `agents.rs` (new `claude_statusline_command` helper and `original_statusline` reader), `apps/desktop/src-tauri/src/commands.rs::resolve_launch`, `apps/desktop/src-tauri/src/usage.rs::prune_old` (new)
Interfaces: `argv_for` gains the Goko home path and the original statusline command (`Option<String>`) as parameters; `resolve_launch` obtains them from `daemon_client::home_dir()` and `~/.claude/settings.json`. Produces the file location `<GOKO_HOME>/usage/claude/<uuid>.json` that Task 3 reads.
Change: for agent id `claude` only, return `[path, "--session-id", uuid, "--settings", json]` where `json` sets `statusLine` per Global Constraints; the original command is embedded with shell quoting (`shlex`), and `usage/claude/` is created if missing. All other agents return exactly the argv they return today. `prune_old(dir, 7 days)` deletes older `*.json` and ignores errors.
Verify: Rust unit tests — `argv_for("claude")` yields the five elements with a v4 uuid that appears in both the flag and the path inside the JSON; with an original command containing a space and a single quote, the parsed `statusLine.command` runs `tee … | <original>` and `shlex::split` of it round-trips; with no original it ends in `>/dev/null`; unreadable and invalid-JSON settings files → no original; `argv_for("shell"|"codex"|"opencode")` unchanged (existing tests in `agents.rs` and `commands/tests.rs` updated for the new signature); `prune_old` removes an 8-day-old file and keeps a 1-day-old one. `cargo test -p desktop` passes.
Progress: done
Result: `cargo test -p desktop` — 120 passed.

### Task 3 — Poll, emit `usage://updated`, and serve `usage_get`

Files: `apps/desktop/src-tauri/src/usage.rs::spawn_poller`, `usage.rs::read_snapshot`, `usage.rs::usage_get` (Tauri command), `apps/desktop/src-tauri/src/lib.rs::handler`, `apps/desktop/src-tauri/src/lib.rs::run`
Interfaces: consumes Task 1 parsers and Task 2's file location and `prune_old`; produces command `usage_get` and event `usage://updated` (payload `UsageSnapshot`) for Task 4. Uses the same `Emit` closure shape as `events::spawn_pump`.
Change: `read_snapshot(goko_home, codex_home)` reads every `usage/claude/*.json` (per-session context keyed by file stem; account limits and `updated_at` from the newest by mtime) and the newest Codex rollout's last 64 KiB (newest day directories first), keeping the previous value for any source that fails. `spawn_poller` runs `prune_old` once, then every 5 s calls `read_snapshot`, stores it in managed state, and emits only when it differs from the last emitted. `usage_get` returns the stored snapshot (or one read on demand if none yet).
Verify: Rust tests using `test_support::TempDir` — two Claude files with different mtimes → account limits from the newer, both session contexts present; corrupt newest file → previous value kept; Codex rollouts in two day directories → the newest one's limits; `$CODEX_HOME` unset falls back to `~/.codex`; an unchanged second poll emits nothing, a changed one emits once. `cargo test -p desktop usage` passes.
Progress: done
Result: `cargo test -p desktop` — 125 passed.

### Task 4 — Frontend data layer

Files: `apps/desktop/src/services/usage.ts` (new: types, `usageService.get`, `claudeSessionId(argv)`), `apps/desktop/src/queryClient.ts::queryKeys`, `apps/desktop/src/services/sync.ts::startQuerySync`, `apps/desktop/src/features/usage/queries.ts` (new: `useUsage`, `useSessionContext`)
Interfaces: consumes `usage_get` and `usage://updated` from Task 3; produces `useUsage()` and `useSessionContext(session)` for Task 5.
Change: `usage` query key; `startQuerySync` listens to `usage://updated` and `setQueryData`; `useUsage` fetches via `usage_get` once and then follows the event. `claudeSessionId(argv)` returns the value after `--session-id` or `null`. `useSessionContext(session)` returns the context percentage for that session's Claude id, or `null`.
Verify: Vitest — `claudeSessionId(['claude','--session-id','u1','--settings','{}'])` → `u1`; argv without the flag → `null`; a `usage://updated` event updates `useUsage` data; `useSessionContext` returns `null` for a session whose id is absent from the snapshot. `pnpm -C apps/desktop test` passes.
Progress: done
Result: focused Vitest run — 27 passed; `pnpm -C apps/desktop typecheck` — passed.

### Task 5 — Header chips and session-row figure

Files: `apps/desktop/src/features/usage/UsageChips.tsx` (new), `apps/desktop/src/features/usage/tone.ts` (new: threshold and reset logic), `apps/desktop/src/App.tsx::App` (header), `apps/desktop/src/features/sessions/SessionItems.tsx::SessionRow`
Interfaces: consumes `useUsage` and `useSessionContext` from Task 4.
Change: `UsageChips` renders one chip per provider that has data (`Claude 5h 42% · 7d 13%`, `Codex 5h 56% · wk 13%`; window labels derived from `window_minutes`), tooltip with reset time and figure age; a limit past `resets_at` renders `reset` muted. `SessionRow` shows a right-aligned `NN%` (tabular numerals) only when `useSessionContext` is non-null. Colour per Global Constraints via `tone.ts`. Chips render nothing when the snapshot has no data.
Verify: Vitest — 79 → muted, 80 → warning, 89 → warning, 90 → danger; `resets_at` in the past → text `reset`; snapshot with only `claude.sessions` → no header chip but the row figure shows; empty snapshot → no chip and no row figure; existing `ProjectTree` and `App` tests still pass. `pnpm -C apps/desktop test` and `pnpm -C apps/desktop typecheck` pass.
Progress: done
Result: `UsageChips` Vitest — 5 passed, including the fake-clock reset and age refresh case; desktop typecheck — passed.

### Task 6 — Documentation and acceptance

Files: `docs/llm/architecture/overview.md` (via `document-wiki` `## Task updates`), `ARCHITECTURE.md`, `CHANGELOG.md`
Interfaces: documents Tasks 1–5.
Change: record the usage module in the Desktop bridge and Desktop UI domain rows and the new event/command, the new `usage/` directory under `GOKO_HOME`, and the Claude launch flags; add a `CHANGELOG.md` `Added` entry. Run the manual acceptance from the design on Claude Code 2.1.318.
Verify: `cargo test --workspace`, `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `pnpm -C apps/desktop typecheck`, `pnpm -C apps/desktop test` all pass. Manual: a Claude session launched from Goko still shows the user's own statusline, the header chip and row percentage appear after its first response, a Claude started in another terminal is unaffected, and an app restart restores the figures from disk; record the observed result, or `cannot verify` for the subscriber-only limits if the account has no `rate_limits`.
Progress: done
Result: fresh final checks passed: `cargo fmt --check`, `cargo build --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` (250 passed across 22 suites), desktop typecheck, desktop tests (268 passed across 29 files), desktop build, `git diff --check`, and the wiki validator (4 Markdown files, 3 requirements, 0 errors/warnings). Manual Claude Code 2.1.318 acceptance in an isolated `GOKO_HOME`: the configured PONYTAIL statusline rendered; after a response Goko showed Claude 5h 39%, 7d 78%, and row context 4%; a separate `claude --print` from `/tmp` returned `OK`; after restarting the desktop process with the daemon still running, the usage chip and row context returned from disk.

## Files inspected, no change

- `crates/protocol/src/message.rs` — `session.create` carries argv, cwd, cols, rows, title and no env; confirms the launch change fits in argv.
- `crates/daemon/src/session.rs`, `crates/daemon/src/pty.rs` — `SessionInfo` keeps `argv`, which is how the UI recovers the Claude session id; the child inherits the daemon environment.
- `apps/desktop/src-tauri/src/events.rs` — the `Emit` closure shape reused by the poller.
- `apps/desktop/src/services/sync.ts` — where daemon events are written into the query cache.
- `docs/agent-devkit/changes/archive/2026-11-04-stop-processes-on-exit/` — shutdown change, now archived; no behavioral overlap, but both edit `apps/desktop/src-tauri/src/lib.rs` (this change only adds `mod usage`, one handler entry, and one `spawn_poller` call in `setup`), so whichever lands second rebases that file.

## Approval Gate

Status: approved

## Decision Log

None.
