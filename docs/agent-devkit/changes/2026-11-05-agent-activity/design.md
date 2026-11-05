# Agent activity: working, needs you, done

## Goal

A running Claude Code or Codex session shows in the sidebar whether the agent is
working, waiting for the user, or finished its turn, so the user can leave a
session and know when to come back. Today `running` only means the process is alive.

## Scope

In:
- Claude Code: hooks injected through the existing `--settings` argument report the state.
- Codex: the state is read from its rollout JSONL (`task_started` / `task_complete`),
  matched to a Goko session by working directory and start time.
- A new app-layer module `activity.rs` in `apps/desktop/src-tauri/` (poller, `activity_get`
  command, `activity://updated` event), mirroring `usage.rs`.
- Sidebar `StateDot` shows the activity of a `running` session.

Out:
- Any change to `crates/protocol` or `crates/daemon`; `SessionState` keeps its meaning and the
  never-set `waiting_for_input` / `idle` states stay untouched.
- Devin, OpenCode, and Shell sessions (they keep today's `running` dot).
- macOS notifications, sounds, history of past turns.

## Behavior

Activity is one of `working`, `waiting` (needs the user), `done` (turn finished).

- Claude: `UserPromptSubmit` → `working`, `Notification` → `waiting`, `Stop` → `done`. Each hook
  command writes that word to `<GOKO_HOME>/activity/claude/<claude-session-id>`. The id is the
  `--session-id` Goko already passes. The hook command always exits 0 and prints nothing, so it
  can never block or break Claude.
- Codex: for a running session whose argv0 is `codex`, find the newest rollout in the two newest
  day directories whose `session_meta.cwd` equals the session `cwd` and whose
  `session_meta.timestamp` is not earlier than `started_at` minus 5 s. A rollout is claimed by at
  most one session; sessions claim in `started_at` order. The last `task_started` /
  `task_complete` event in the final 64 KiB gives `working` / `done`. Codex has no `waiting`
  event, so an approval prompt stays `working` (known limit).
- The poller ticks every second, lists the running sessions through the existing bridge, and
  emits `activity://updated` with the full `{goko_session_id: activity}` map only when it changed.
  `activity_get` returns the current map so the webview can hydrate on load.
- Only sessions in state `running` carry an activity; every other state ignores it.
- Sidebar dot for a `running` session: `working` pulses in the warning tone, `waiting` is solid
  danger tone, `done` is solid success tone, no activity keeps today's success dot. The dot's
  title reads `running · working` (etc.) for assistive tech. `created`, `starting`, `stopping`,
  `exited`, and `failed` render exactly as today.
- Activity files older than 7 days are pruned with the existing `prune_old` at startup.

## Edge cases

- Hook file missing, empty, or mid-write: keep the previous value; an unknown word is ignored.
- The user already has Claude hooks in their own settings: Goko's `--settings` hooks must run in
  addition to them. Task 1 verifies this on the installed Claude (2.1.311); if `--settings`
  replaces instead of merging, the design returns to approval.
- Claude resumed or restarted with the same session id: the file is overwritten by the next hook.
- Two Codex sessions in the same directory: the earlier-started session claims the earlier
  rollout; a session with no unclaimed match shows no activity (ceiling of the heuristic).
- Codex rollout crossing midnight: covered by scanning the two newest day directories, as
  `usage.rs` already does.
- App restart while Claude is mid-turn: the file persists, so the state is restored when the
  session is shown again; an exited process is ignored because its state is not `running`.
- Daemon unreachable: the poller keeps the previous map and retries next tick.

## Interfaces

- Claude launch argv (`agents::argv_for`): the `--settings` JSON gains
  `hooks.{UserPromptSubmit,Notification,Stop}`, one command each.
- Tauri command `activity_get() -> HashMap<String, Activity>`; event `activity://updated` with
  the same map. `Activity` serializes as `"working" | "waiting" | "done"`.
- Frontend: `services/activity.ts` (type + `activityService.get`), `features/activity/queries.ts`
  (`useActivity`, refreshed from the event like usage), `StateDot` takes an optional activity.
- On-disk: `<GOKO_HOME>/activity/claude/<claude-session-id>` containing one word.

## Verification

- Rust unit tests: hook command text per event, Claude file parsing, Codex rollout tail parsing,
  Codex session-to-rollout matching (claim order, cwd, start-time slack), poller emits only on change.
- Vitest: `StateDot` renders each activity and leaves non-running states unchanged; `useActivity`
  hydrates from `activity_get` and updates from the event.
- `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`,
  `pnpm -C apps/desktop typecheck` and the Vitest suite.
- Manual on the installed app: a Claude session goes working → done on a prompt and → waiting on a
  permission prompt; a Codex session goes working → done; a Shell session is unchanged.

## Related context

- [Architecture overview](../../../llm/architecture/overview.md)
