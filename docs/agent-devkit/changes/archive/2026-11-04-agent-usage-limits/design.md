# Agent usage and limit indicators

## Goal

The user sees, inside Goko, how close their Claude Code and Codex accounts are
to the 5-hour and weekly usage limits, and how full each Claude session's
context window is, so they can stop or redistribute work before a limit stops
them.

## Scope

In scope:

- Claude Code: account limits (5-hour, 7-day) and per-session context fill,
  read from the JSON Claude Code feeds its `statusLine` command.
- Codex: account limits (5-hour, weekly) read from the newest Codex rollout
  file.
- A usage module in the desktop Rust backend (`apps/desktop/src-tauri/`), one
  new Tauri command and one new Tauri event, and UI: limit chips in the app
  header and a context percentage on Claude session rows.
- Launching Claude sessions with `--session-id` and `--settings` so Goko can
  capture the statusline JSON without the user configuring anything.

Out of scope (each is a separate change):

- macOS notifications or Dock badges when a threshold is crossed. This change
  colours the indicators only.
- Per-session context for Codex (Codex has no session-id flag; mapping needs
  cwd and start-time matching).
- OpenCode, Shell, and custom-command sessions; Claude sessions not started by
  Goko.
- Windows and Linux, cost in dollars, history or charts, a settings UI or
  kill switch for the injection.
- Any change to the wire protocol or to `gokod`. The reader lives in the
  desktop backend because nothing but the UI consumes the data; the daemon
  keeps owning only PTYs.

## Behavior

**Claude launch.** For the `claude` agent only, `agents::argv_for` returns
`[claude, "--session-id", <uuid v4>, "--settings", <json>]`. The JSON sets
`statusLine` to `{"type":"command","command":<wrapper>}`. The wrapper is
`tee <GOKO_HOME>/usage/claude/<uuid>.json | <original>` when the user's
`~/.claude/settings.json` has a `statusLine.command` (so their statusline keeps
rendering), and `tee <path> >/dev/null` when it has none. Only the user-level
`~/.claude/settings.json` is consulted; a project-level `statusLine` is
overridden for Goko-launched sessions (known ceiling). Shell, Codex, OpenCode,
and custom commands launch exactly as today.

**Claude data.** Claude Code runs the statusline command after each response
and writes the JSON to its stdin. The fields used are
`context_window.used_percentage`, `rate_limits.five_hour.{used_percentage,
resets_at}`, and `rate_limits.seven_day.{used_percentage, resets_at}`. The
rate-limit fields exist only for subscribers and only after the first API
response; absent fields mean "unknown", never zero. Rate limits are
account-wide, so the account figure is taken from the most recently modified
`usage/claude/*.json` (even from an exited session). Per-session context is
keyed by the Claude session id, which the UI recovers from the Goko session's
`argv` (`--session-id <uuid>`).

**Codex data.** The reader finds the newest `rollout-*.jsonl` under
`$CODEX_HOME/sessions` (default `~/.codex/sessions`, searched in the newest day
directories), reads its last 64 KiB, and takes the last `token_count` event's
`rate_limits.primary` and `rate_limits.secondary` (`used_percent`,
`window_minutes`, `resets_at`). No Goko-side launch change.

**Polling and delivery.** A backend task re-reads both sources every 5 seconds.
When the resulting `UsageSnapshot` differs from the previous one it emits
`usage://updated` with the snapshot; `usage_get` returns the current snapshot
for the first render. `services/sync.ts` writes the event into the query cache.

**Display.**

- Header: one chip per provider that has data, e.g. `Claude 5h 42% · 7d 13%`
  and `Codex 5h 56% · wk 13%`. The tooltip shows the reset time and how old the
  figure is. A limit whose `resets_at` has passed is shown as `reset` (muted)
  instead of its old percentage.
- Session row: a right-aligned `NN%` (tabular numerals) for a Claude session
  whose context figure is known.
- Colour: below 80% muted, 80–89% warning, 90% and above danger, for chips and
  row figures alike.

## Edge cases

- Missing, unreadable, truncated, or non-JSON files (Claude writes with `tee`,
  so a read can see a partial file): that source keeps its previous value; no
  error is shown and nothing is logged with file contents.
- Codex rollout whose tail has no `token_count` yet: Codex chip hidden.
- Gateway or API-key Claude users: `rate_limits` absent, so only context is
  shown (`rate_limits.spend_limit` is not displayed in this change).
- Two Claude sessions at once: each writes its own file; account limits come
  from the newest, per-session context from its own file.
- The original statusline command contains quotes or spaces: it is embedded
  with shell quoting, covered by a test; the settings path being unreadable or
  invalid JSON means "no original".
- `claude` too old to know `--settings` or `--session-id`: the session fails to
  start and shows as `failed`, as any bad launch does. Both flags exist in the
  installed 2.1.318. No fallback is built.
- `usage/claude/*.json` older than 7 days are deleted at app start.
- Daemon-owned sessions survive an app restart, and their files remain on disk,
  so the figures reappear after restart without a new response.
- Figures are only as fresh as the last Claude response or Codex event; the
  tooltip states the age rather than implying live data.

## Interfaces

Tauri command `usage_get() -> UsageSnapshot`; event `usage://updated` with the
same payload.

```
UsageSnapshot {
  claude: { five_hour: Limit | null, seven_day: Limit | null,
            sessions: { [claudeSessionId]: { context_pct: number } },
            updated_at: number | null /* ms */ },
  codex:  { primary: Limit | null, secondary: Limit | null,
            updated_at: number | null }
}
Limit { used_pct: number, resets_at: number /* epoch seconds */, window_minutes: number }
```

Changed signature: `agents::argv_for` gains the Goko home and the original
statusline command (caller `commands.rs` `resolve_launch`, plus tests). New
files: `src-tauri/src/usage.rs`; `src/services/usage.ts`, `src/features/usage/`
(chips, hook). Touched UI: `App.tsx` header, `SessionItems.tsx`,
`services/sync.ts`. No change to `crates/protocol` or `crates/daemon`. The
`usage/` directory under `GOKO_HOME` is a new on-disk location written by
Claude Code, not by Goko.

## Verification

- Rust unit tests: Claude JSON parsing (full, partial, absent `rate_limits`),
  Codex rollout tail parsing (last `token_count`, truncated first line, no
  event), wrapper command quoting, `argv_for` for `claude` and unchanged for
  other agents, stale-file cleanup.
- Frontend tests: threshold colours, `reset` display, Claude session id
  extraction from `argv`, row figure shown only when known.
- Existing suites stay green: `cargo test --workspace`, `pnpm -C apps/desktop
  test`, typecheck, `cargo clippy --workspace -- -D warnings`.
- Manual acceptance on the installed Claude Code 2.1.318: launch Claude from
  Goko, confirm the user's own statusline still renders, confirm the header
  chip and row percentage appear after the first response, confirm a Claude
  started outside Goko is unaffected.

## Related context

- [Architecture overview](../../../../llm/architecture/overview.md)
