# Design: Terminal agent handoff

## Goal

Continue a task in the same checkout under any supported terminal agent
or account, even when the source has no quota. Do not depend on the source
agent generating a summary or on different agents sharing a session format.

## Scope

- All built-in interactive agent presets can be both source and destination:
  Claude Code, Codex, Devin, OpenCode, and Antigravity. The destination may
  be the same agent. Use the existing preset registry for eligibility:
  a new interactive preset gets the context-and-notes route even before
  it has a history reader. Use the account options it already supports:
  Default and registered profiles for Claude, Codex, and Devin on
  macOS/Linux; the ambient Default for OpenCode and Antigravity. On Windows
  use only Default and the same context flow; no named launches or native
  fork.
- Start from the focused pane of a Goko preset tab in a registered project
  or worktree. An exited pane still in its tab can hand off saved context.
  Shell and custom commands are not known agent providers. Gemini in the
  ACP settings is a commit-message generator, not an interactive tab;
  ACP generation, remote/cloud history and automatic quota failover are
  out of scope.
- Leave the source pane, source history and active account setting intact.
  Do not read or copy credentials, link history, import provider databases,
  send prompts, or change the daemon protocol. No new dependency, permanent
  handoff store, or cross-provider session index.

## Behavior

1. The source tab's menu offers **Hand off to...**. A native dialog shows
   the source agent, account and recorded checkout. Choose any installed
   agent preset and an explicit supported destination account. The choice
   applies to this launch only, not to future normal launches.
2. The default **New session with context** path fills an editable review
   with a short Git snapshot (branch, HEAD and up to 50 changed paths with
   a count of the rest, not diffs), available user/assistant text from the
   exact saved conversation, and room to add the next step. The preview
   labels the source and whether any history is missing or shortened. The
   user can remove any content before sharing it with a different account
   or organization.
   - The generated draft is at most 16 KiB of valid UTF-8. When the
     conversation does not fit, keep the first user message (cut to
     2 KiB), then the newest whole messages that fit, dropping the oldest
     first, and say how many messages were omitted. If the newest message
     alone does not fit, keep its end, cut at a character boundary and
     marked as shortened.
   - The user's edits may not take the review past 16 KiB: the dialog shows
     the byte count and disables opening while it is over.
3. **Open & Copy Context** opens a fresh tab in the recorded project and
   checkout, with the destination's regular preset and account launch
   environment. Copy only the reviewed text to the clipboard on successful
   session creation, as explicitly requested by the button. An empty
   review offers **Open session** and does not touch the clipboard. The
   user pastes and submits after the CLI is ready. Goko never inserts text
   into a PTY, uses it as argv, attaches files, or sends a model prompt.
4. Also offer **Fork Claude conversation** if both ends are Claude, the
   source transcript is confirmed, the host is macOS/Linux, and the
   destination profile's `projects` folder is the same directory as the
   source transcript's `projects` folder (as the existing shared `projects`
   link of named Claude profiles makes it). Explain that this gives the
   destination Claude the full saved history, not the reviewed excerpt.
   Launch the destination's normal argv, which already carries a fresh
   `--session-id` and the usage/activity hooks, plus `--resume <source ID>`
   and `--fork-session`. Never pass a transcript path. When the folders
   differ, do not offer the fork and say the destination does not share
   this Claude history. Do not copy the review in this mode. All other
   routes, including Codex A to Codex B and Devin A to Devin B, use the
   context flow and never attempt to make a different profile resume the
   old ID.

| Source | Local context reader | Destination accounts |
| --- | --- | --- |
| Claude Code | Exact transcript `projects/<dir>/<id>.jsonl` under the launch profile | Default and named Claude profiles |
| Codex | Exact uncompressed `sessions/**/rollout-*-<id>.jsonl` under the launch `CODEX_HOME` (or Default); compressed rollouts are unsupported | Default and named Codex profiles |
| Devin | Exact `sessions.db` row and its `main_chain_id` message chain under the launch `XDG_DATA_HOME` (or Default) | Default and named Devin profiles |
| OpenCode | Exact `session` row and its `message`/`part` text in the ambient `opencode/opencode.db`; sessions only in the newer `session_v2` tables are unsupported | Default only |
| Antigravity | Exact ID's CLI `brain/<id>/.system_generated/logs/transcript.jsonl`, where available; not the binary conversation DB | Default only |

Read history off the UI thread only for the handoff. For a running pane,
take a fresh process sample of that pane for PID-linked evidence, then
fall back to the exact conversation ID recorded in launch args (label a
launch ID as the initial conversation, not necessarily the current one after
`/clear` or `/resume`). An exited pane has only its launch ID: Goko's own
Claude launches always record one, while Codex, Devin, OpenCode and
Antigravity panes started without a resume flag have none. If none is
reliable, allow the user to enter an explicit saved conversation ID. Verify
it within the source profile and checkout; a manual ID with unverifiable
checkout metadata is not enough. Never select the newest session by time or
cwd, enumerate other accounts' histories, or use the current active account
to locate the source. Offer a plain context review even on hosts or CLI
releases where only a manual ID or no local reader is available.

Read only the expected transcript paths or the exact row in a read-only local
database, with bounded lookups, per-record sizes and total read budgets.
Allow the existing Claude `projects` link, but reject symlinked child files
or directories that redirect the lookup. Transfer plain, visible user and
assistant text only: no system, reasoning, tool input/output, attachments,
terminal scrollback, or raw provider records. Strip terminal control
characters. This is not a secret detector; the user is responsible for the
reviewed text.

## Edge cases

- Missing, locked, corrupt, truncated or unsupported history, ambiguous
  attribution, an exited or OpenCode pane without a recorded conversation
  ID, or Antigravity without a readable verified transcript: show the
  reason and optionally accept an explicit ID (not for Antigravity, whose
  transcript records no checkout to verify it against). Still allow a handoff with
  Git state and the user's notes, without claiming the old conversation was
  loaded. For non-Git projects omit Git details. Never try a different chat.
- Disappearing binary or checkout, removed source pane/project, unknown
  destination account, or two different account registrations now aliasing
  the same folder: refuse with an explanation, without falling back to
  Default. The same intentionally selected account is allowed. The CLI
  keeps its usual credential precedence and workspace trust requirements;
  a destination without a usable login or quota cannot answer.
- Correlate preparation and launch with the source pane and dialog request.
  Discard stale replies before dispatching a create and prevent duplicate
  opens. Changing destinations does not reload or overwrite review edits.
  Cancel is available until session creation is dispatched; after that,
  wait for its result. Failed preparation or creation keeps the review
  usable and does not change the clipboard. A rejected native fork in an
  older CLI remains an error in the new terminal and does not retry a
  different chat. A Claude destination whose `projects` folder no longer
  matches the source at launch is refused, not forked from elsewhere.

## Interfaces

- `crates/app-core/src/handoff.rs`: source identity and profile-scoped
  context readers over the existing `serde_json` and `rusqlite` dependencies,
  draft construction, and validated fork data. Reuse `session_debug`
  identity evidence, `accounts` for an explicit destination lookup, and
  `agents::argv_for_account` for launch flags and environment.
- `apps/rune/src/editor/backend.rs`: correlate a preparation job and a
  final validation/argv job. Read source context and filesystem state on
  the existing worker; do not send the user-edited draft back to the
  worker or into daemon requests.
- `apps/rune/src/handoff.rs`, `tab_debug.rs`, and `main.rs`: native picker,
  preview, confirmation and stale-reply handling. Use normal `SessionLaunch`
  and new-tab placement. `sessions.rs` associates a transient, approved copy
  with the successful create result; neither tab layout nor metadata stores it.

## Verification

- Core fixtures cover the five source formats and all source/destination
  combinations, mismatched IDs/checkouts/profiles, unreadable or unsupported
  stores, size limits and the truncation rule. Checks include Devin's main
  chain versus abandoned retries, OpenCode's text parts versus tool output,
  Claude's rewound branches, and account resolution that does not alter the
  global selection.
- Rune harness covers the target tab's focused split pane, exited panes,
  explicit account selection, editable review and its byte limit,
  no-history fallback, cancel/stale/error paths, and an approved copy only
  after the matching session is created.
- In a freshly built scratch macOS bundle, use temporary fixture binaries
  and synthetic histories outside the repository: exercise all five presets
  as sources and targets, including Claude to Codex, Codex to Devin, Devin
  A to Devin B, and unavailable history. Assert exact cwd/account
  environment, bounded review, unchanged original pane/session and no
  automatic PTY input. Verify a real Claude CLI fork between two synthetic
  profiles sharing `projects`, against a temporary loopback model server:
  the target request carries the old turn, the new ID differs, and the
  source file is unchanged. No real subscription use, Linux/Windows GUI
  interaction, or quota exhaustion in live accounts is required locally.
- Run workspace build, format, Clippy and tests; update and validate the
  Rune chrome wiki. Windows routing is covered by the Default-only
  eligibility rule, with real Windows GUI verification a separate limit.

## Related context

- [Rune chrome requirements](../../../../llm/domains/rune-chrome.md)
- [Rune editor requirements](../../../../llm/domains/rune-editor.md)

## Previous work

- [Named Devin CLI accounts](../devin-multi-account/design.md)
- [Tab IDs and agent diagnostics](../tab-debug-info/delta.md)
