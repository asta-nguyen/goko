# Terminal agent handoff tasks

## Design

[Terminal agent handoff](design.md)

## Global Constraints

- Leave the source pane, source history and active account setting intact.
  Do not read or copy credentials, link history, import provider databases,
  send prompts, or change the daemon protocol.
- No new dependency, permanent handoff store, or cross-provider session
  index.
- Never select the newest session by time or cwd, enumerate other accounts'
  histories, or use the current active account to locate the source.
- The generated draft and the approved review are each at most 16 KiB of
  valid UTF-8, with terminal control characters stripped.
- A Claude fork passes `--resume <source ID> --fork-session` on top of the
  destination's normal argv and never a transcript path.

## Impact map

Entry: no existing source; planned entry: `design.md` / Hand off to
action in `tab_debug::menu_layer` (pending approval).

Flow: no existing flow; planned flow: `design.md` / tab menu -> focused
preset pane -> worker samples the pane, reads exact history and checkout
metadata -> review and choose destination -> worker validates launch ->
`State::launch_session` -> optional copy on successful create (pending
approval). Reuse `tab_debug::focused_session`, `session_debug::sample`,
`agents::discover`, `agents::argv_for_account`, `accounts::resolve`'s
storage access, `accounts::reject_physical_alias`, the `Req::AgentArgv`
token pattern, `tx.copy` and `State::drain_pending`.

State changes: pending approval: transient picker, reviewed draft and
matched requests; normal new session metadata and layout, no account
setting changes or stored draft.

External effects: pending approval: worker samples the source pane's
processes, reads bounded local history and Git metadata, starts the chosen
CLI in a new PTY on confirmation, and copies the approved excerpt only on
successful create. A confirmed Claude fork instead resumes the source ID
from the shared `projects` folder with `--fork-session`. No source model
call or automatic PTY input.

Change candidates: pending approval: `crates/app-core/src/{lib,handoff,session_debug,accounts,agents}.rs`,
`apps/rune/src/{handoff,tab_debug,main,sessions,view_tests}.rs`,
`apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/backend/tests.rs`,
and `docs/llm/domains/rune-chrome.md`.

Verification: pending approval: focused core, backend and Rune tests for
all five preset providers; scratch macOS app smoke with temporary
fixtures; installed Claude CLI against a temporary loopback server;
workspace checks and wiki validation.

Verified at: 4b1d6de846f47a2a183b4d7ef3f3dcf3d0fd1134

## Tasks

### Task 1 - Identify the source conversation and build the bounded draft

Files: `crates/app-core/src/lib.rs::handoff`,
`crates/app-core/src/handoff.rs::{source,draft}`,
`crates/app-core/src/session_debug.rs::{identity,launch_id}`,
`apps/rune/src/editor/backend.rs::Req::PrepareHandoff`.

Interfaces: `Req::PrepareHandoff` carries a correlation token, the source
pane's session key, preset, recorded argv, checkout, live/exited state and
an optional explicit conversation ID. Its reply carries the token, a
bounded draft, history status (verified, launch ID, manual, missing with a
reason), the source profile folder, and a verified Claude source reference
when available. Provider readers (Task 2) return ordered user/assistant
messages to the draft builder.

Change: For a running pane, run `session_debug::sample` for that one
session key on the worker so PID-linked evidence is fresh; fall back to
the launch ID. For an exited pane use only `launch_id` on the recorded argv
(make it reachable without the live-only check). Resolve the source profile
from the recorded argv's `env` words (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`,
`XDG_DATA_HOME`), else the Default folder. A manual ID is accepted only
after the reader confirms it exists in that profile and its recorded cwd is
the checkout. Build the draft: source label and status line; Git branch,
HEAD and up to 50 changed paths plus a count of the rest from
`git status --porcelain` in the checkout (omitted for non-Git folders);
then messages under the 16 KiB rule: the first user message cut to 2 KiB,
then the newest whole messages that fit, oldest dropped first, with an
"N earlier messages omitted" line; a newest message too large on its own
keeps its end, cut at a character boundary and marked shortened. Strip
terminal control characters. Missing history returns a context-only draft.

Verify: `cargo test -p app-core handoff::tests::draft` and
`cargo test -p app-core session_debug` cover: running pane with one
PID-linked ID; ambiguous PID evidence → missing with reason; exited Claude
pane → launch ID labeled initial; exited Codex/Devin/OpenCode/agy pane
without a resume flag → missing; profile from each `env` word and Default;
manual ID with wrong cwd or wrong profile → refused; draft over 16 KiB
keeps the first message ≤2 KiB, newest messages and the omitted count;
oversized single message keeps its end with valid UTF-8; control
characters removed; non-Git checkout has no Git section; 51 changed paths
→ 50 listed and "1 more".

Progress: done

Result: `cargo test -p app-core handoff` (16 passed: draft limits, first-message cut, newest kept, omitted count, UTF-8 tail, control stripping, 50-path list, non-Git, profile from env words, prepare/missing/manual) and `cargo test -p app-core session_debug` (8 passed, incl. exited launch ID) — pass. Backend `PrepareHandoff` covered by `cargo test -p goko-rune handoff_backend` — pass. Baseline before edits: app-core 380 and goko-rune 1064 tests passed.

### Task 2 - Read exact saved context from all five providers

Files: `crates/app-core/src/handoff.rs::{claude,codex,devin,opencode,agy}`.

Interfaces: Each reader takes the source profile folder (or ambient Default
folder), the conversation ID and the checkout, and returns ordered
user/assistant text plus a shortened flag, or an error naming the reason
(missing, locked, corrupt, unsupported, wrong checkout, unsafe path).
Databases open with `SQLITE_OPEN_READ_ONLY` and a zero busy timeout.

Change: Per provider, only the listed records become text:
- Claude: `<profile>/projects/<dir>/<id>.jsonl`; the top-level `projects`
  link is allowed, a symlinked `<dir>` or file is refused. Walk
  `parentUuid` back from the last `user`/`assistant` record so rewound
  branches are excluded. Keep `user` string content or `text` blocks and
  `assistant` `text` blocks; skip `isMeta`/`isSidechain` records,
  `tool_result`, `thinking`, `tool_use` and every other record type. The
  records' `cwd` must equal the checkout.
- Codex: `<CODEX_HOME>/sessions/YYYY/MM/DD/rollout-*-<id>.jsonl`, searched
  only within the bounded date directories; `.zst`/`.gz` rollouts are
  unsupported. `session_meta.payload.id` and `cwd` must match. Keep
  `response_item` `message` items: role `user` `input_text` and role
  `assistant` `output_text`; skip `developer`, `reasoning`, tool calls and
  outputs, and injected user text starting with `# AGENTS.md instructions`,
  `<environment_context>` or `<image name`.
- Devin: `<XDG_DATA_HOME>/devin/cli/sessions.db`. The `sessions` row must
  exist with `working_directory` equal to the checkout. Walk
  `message_nodes` from `node_id = main_chain_id` through `parent_node_id`
  to the root (bounded node count), then reverse. Keep `chat_message` role
  `user`/`assistant` text `content`; skip `system`, `tool`, `thinking`,
  `tool_calls`, nodes off the main chain and `subagent_heads` chains.
- OpenCode: ambient `opencode/opencode.db`. The `session` row must exist
  with `directory` equal to the checkout; an ID only in `session_v2` is
  unsupported. Order `message` rows by `time_created, id`; keep
  `data.role` `user`/`assistant` and their `part` rows with
  `data.type = "text"` that are not `synthetic` or `ignored`; skip
  `reasoning`, `tool`, `patch`, `file`, `step-*` and `compaction` parts.
- Antigravity: `<AGY_DIR or ~/.gemini/antigravity-cli>/brain/<id>/.system_generated/logs/transcript.jsonl`
  (same home rule as `usage_collect::agy::app_root`). Keep `USER_INPUT`
  `content` as user and `PLANNER_RESPONSE` `content` as assistant; skip
  `thinking`, `tool_calls`, `GENERIC` and `SYSTEM_MESSAGE`. A record whose
  `truncated_fields` names `content` sets the shortened flag. The
  transcript has no checkout field, so only PID-linked or launch-ID
  evidence is accepted; a manual ID is refused with that reason.

All readers cap per-record bytes, record count and total bytes read, and
stop with "shortened" rather than reading past the budget.

Verify: `cargo test -p app-core handoff::tests::readers` with synthetic
histories per provider in temp profile folders: the intended turns appear
in order; Claude rewound branch, sidechain and tool records absent; Codex
developer and injected AGENTS/environment text absent, compressed rollout
→ unsupported; Devin abandoned retry node and subagent chain absent;
OpenCode tool, reasoning and synthetic text parts absent, `session_v2`-only
ID → unsupported; Antigravity thinking/tool calls absent, truncated
content → shortened, manual ID → refused. Each provider: wrong checkout →
refused, other profile's ID → missing, corrupt record → corrupt, locked
database → locked, symlinked child path → unsafe, over-budget input →
shortened without reading past the cap.

Progress: done

Result: `cargo test -p app-core handoff` — pass: per-provider fixtures for Claude, Codex, Devin, OpenCode and Antigravity cover kept turns, excluded tool/reasoning/injected/rewound/retry/subagent/synthetic text, wrong checkout, other profile, corrupt record, locked database (Devin, OpenCode), linked child path, oversized record, compressed rollout, `session_v2`-only ID and refused Antigravity typed ID.

### Task 3 - Resolve one destination and build its launch

Files: `crates/app-core/src/accounts.rs::resolve_id` (new),
`crates/app-core/src/agents.rs::argv_for_account`,
`crates/app-core/src/handoff.rs::validate_fork`,
`apps/rune/src/editor/backend.rs::Req::HandoffArgv`,
`apps/rune/src/editor/backend/tests.rs`.

Interfaces: `Req::HandoffArgv` carries a correlation token, the prepared
source reference, destination preset, destination account ID and mode
(context or Claude fork). Its reply carries the token and verified argv
with cwd, or an error. The reviewed text is not part of this request.

Change: `accounts::resolve_id(agent, id, storage)` returns the
`LaunchAccount` for exactly that stored ID (or Default) without touching
the active selection; an unknown ID is an error, and a stored folder that
now physically aliases Default or another registration is refused via
`reject_physical_alias`. On Windows, or for OpenCode/Antigravity, only
Default resolves. Revalidate the source checkout and destination binary.
Context mode uses `argv_for_account` unchanged, with no initial prompt. For
fork mode, `validate_fork` requires both ends Claude on macOS/Linux, a
verified source ID, and the destination `<profile>/projects` resolving to
the same directory as the source transcript's `projects`; then append
`--resume <source ID> --fork-session` to the normal argv, which keeps its
fresh `--session-id` and usage/activity settings. Errors never fall back to
another account or chat.

Verify: `cargo test -p app-core accounts::tests::resolve_id` and
`cargo test -p app-core handoff::tests::fork` cover: named and Default
lookups for Claude, Codex and Devin; unknown and aliased IDs refused;
OpenCode/agy named ID refused; account store bytes and active selection
unchanged after resolution; fork accepted with shared `projects`, refused
with separate `projects`, non-Claude ends, unverified source or Windows;
fork argv contains the source ID once after the binary, `--fork-session`,
a different `--session-id`, and no path ending in `.jsonl`.
`cargo test -p goko-rune handoff_backend` covers token echo, all five
destination presets, named Devin isolation, same-account context allowed,
and missing binary/checkout errors.

Progress: done

Result: `cargo test -p app-core resolve_id` and `cargo test -p app-core handoff` (fork) — pass; `cargo test -p goko-rune handoff_backend` (2 passed) and `cargo test -p goko-rune named_account_launches_and_sign_in_and_usage_read` (all five presets on Default, named Claude and Devin, shared-`projects` fork with fresh `--session-id`, refused separate-`projects` fork, selection unchanged) — pass.

### Task 4 - Wire the reviewed handoff to the tab menu

Files: `apps/rune/src/handoff.rs::layers`, `apps/rune/src/tab_debug.rs::menu_layer`,
`apps/rune/src/main.rs::State`, `apps/rune/src/sessions.rs::State::drain_pending`,
`apps/rune/src/view_tests.rs`.

Interfaces: Consume the discoverable preset list, account view,
`PrepareHandoff` and `HandoffArgv` replies, and normal `SessionLaunch`.
Keep the reviewed copy only with the pending create for its confirmed
destination.

Change: Add **Hand off to...** to the tab menu for a preset pane, live or
exited. Select from any installed preset and supported account (Windows and
OpenCode/agy: Default only); show attribution, missing history and an
explicit ID field where needed. Keep the preview editable, show its byte
count, and disable opening while it is over 16 KiB. Offer **Fork Claude
conversation** only when preparation reports a forkable source and the
selected destination shares `projects`, with the full-history-versus-excerpt
explanation; otherwise show why it is unavailable. Open in the source
pane's checkout on confirmation, call `tx.copy` only for that successful
create if the review is nonempty and the mode is context, and never type
the context. Discard replies whose token is stale (cancelled, superseded
selection, removed source pane) before create is dispatched. Prevent
duplicate requests, retain review edits when switching destination, and
keep the draft and clipboard unchanged on errors or failed creation.

Verify: `cargo test -p goko-rune handoff` and
`cargo test -p goko-rune tab_debug::tests` exercise the targeted split
pane, exited pane, picker coverage against the preset registry, Windows and
OpenCode/agy Default-only filtering, missing history, manual ID
preparation, edited context kept across destination changes, the 16 KiB
disable, fork offered only with shared `projects`, stale/failed replies,
failed create leaving draft and clipboard unchanged, duplicate open
ignored, and copy only on the matching successful new tab. Assert that
launching changes neither the source pane nor the active account setting.

Progress: done

Result: `cargo test -p goko-rune handoff` (13 passed: focused split pane, exited pane, shell/projectless refusal, registry picker, Default-only OpenCode/agy and Windows, manual ID, edits kept across destinations, 16 KiB disable, fork only with a sharing account, stale/cancelled/orphaned replies, failed argv and failed create keep the review, copy only after the matching create, poll copies once) and `cargo test -p goko-rune tab_debug::tests` (3 passed, menu shows Hand off with its reason) — pass.

### Task 5 - Verify native and CLI behavior and update the wiki

Files: `docs/llm/domains/rune-chrome.md::RNC-agent-handoff`.

Interfaces: Consume the tested app and the approved `delta.md` requirement.

Change: Merge the approved requirement with source and test evidence.
Document which history is read per provider, the 16 KiB rule, the explicit
clipboard interaction, and the separate native fork and no-history
behavior.

Verify: `cargo build --workspace`, `cargo fmt --check`,
`cargo clippy --workspace -- -D warnings`, `cargo test --workspace`, and
`node .agents/skills/document-wiki/scripts/validate-llm-wiki.mjs`. In a
fresh scratch macOS bundle, use temporary fixture CLI scripts and synthetic
histories under a temp `GOKO_HOME` and profile folders, all outside the
repository and deleted afterwards. Cover each provider as source and
destination, including Claude to Codex, Codex to Devin, Devin A to Devin B
and unavailable history, with exact cwd/account argv, no automatic PTY
input, and original tab/setting intact. With the installed Claude CLI, two
synthetic profiles sharing `projects`, a dummy API key and
`ANTHROPIC_BASE_URL` pointing at a temporary loopback server script, check
the forked request includes the old turn, the new session ID differs, and
the source file is unchanged; if the CLI asks to approve the dummy key or
refuses the loopback URL, record the limit instead of using a real
account. No real subscription use or Linux/Windows GUI interaction.

Progress: done

Result: `cargo build --workspace`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` (app-core 398, goko-rune 1077, all other suites) — pass; `validate-llm-wiki.mjs` — 0 errors, 0 warnings; `git diff --check` — clean. Installed Claude 2.1.323 with synthetic profiles, a dummy key and a loopback server: a real source session, then `--session-id <new> --resume <id> --fork-session` from a profile whose `projects` links to the source sent the old turn and reply, wrote a new transcript and left the source file byte-identical; a profile with separate `projects` got "No conversation found" and `validate_fork` refuses it; the Claude reader read the real CLI transcript. Scratch `GokoHandoff.app` (fresh debug build, own bundle id, temp GOKO_HOME, fixture CLIs, `--headed` right-click allowed by the user): Claude → Codex opened a new tab in the repo with plain argv, copied the reviewed draft (conversation turns plus branch/changed path) after creation; Claude → Claude Default fork launched with a fresh `--session-id` plus `--resume <source> --fork-session` and left the clipboard unchanged; Codex (no ID) → Devin showed "No verified conversation text", the Conversation ID field, a context-only draft, and launched Devin Default. Verification limit: OpenCode and Antigravity as GUI sources, named accounts and Devin A → B in the GUI, and Linux/Windows GUI were not exercised; they are covered by the unit and backend tests only.

## Files inspected, no change

- `apps/rune/src/chrome.rs` - tab menu is anchored to a stable tab ID.
- `apps/rune/src/data.rs` - session launch argv and account display names.
- `apps/rune/src/panes.rs` - normal tab placement and `tx.copy`; no
  handoff calls the PTY paste path.
- `crates/app-core/src/activity.rs` - heuristic Codex activity ownership is
  not sufficient to share transcript contents.
- `crates/app-core/src/acp.rs` - Gemini is a commit-message preset, not a
  terminal session provider.
- `crates/app-core/src/store.rs` - project and session metadata already exist.
- `crates/app-core/src/usage_collect/agy.rs` - `app_root` gives the
  Antigravity CLI home.

## Approval Gate

Status: approved

## Decision Log

- Pre-approval scope revision: replace Claude-only handoff with one context
  flow for all five terminal presets. Retain an explicit Claude fork as a
  capability, not a requirement for other providers. Approval remains pending.
- Pre-approval review revision: Claude 2.1.323 documents `--resume` only
  for a session ID or name, so the fork resumes the source ID and is offered
  only when the destination shares the source's `projects` folder (the
  existing named-profile link), never by transcript path.
- Pre-approval review revision: the 16 KiB draft keeps the first user
  message (≤2 KiB) and the newest messages, dropping the oldest with a
  count; user edits past 16 KiB disable opening.
- Pre-approval review revision: exited panes use only their launch ID, so
  non-Claude exited panes need a manual ID; Antigravity manual IDs are
  refused because its transcript has no checkout field.
- Pre-approval review revision: provider record rules taken from local
  store structure (keys and schema only) and the former Task 1 split into
  identity/draft and readers. Approval remains pending.
- Approved by the user on 2026-11-22 ("implement di") after the review
  revision was presented: design.md, tasks.md and delta.md together.
