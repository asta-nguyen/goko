# Editor: stage, commit, push, and AI commit messages over ACP

## Goal

The Editor's Changes view can stage, unstage, discard, commit, and push. The user connects
a local agent from a configurable list (ACP Connect / Disconnect), and a "Generate" button asks the connected
agent to draft the commit message from the staged diff using the agent's own model over the Agent Client Protocol (ACP). The agent only drafts text; the user reads
and edits it, then presses Commit. Today Changes is read-only.

## Scope

In:
- Changes list actions: Stage / Unstage a file, Stage all / Unstage all, Discard one file (with confirmation).
- A commit box under the changed-files list: message textarea, agent picker, Generate / Cancel, Commit, Push.
- Backend module `git_write.rs` (stage, unstage, discard, commit, push) and Tauri commands; `git_view_changes` also reports `ahead`.
- Backend module `acp.rs`: an ACP client over a child process's stdio, hand-rolled newline-delimited JSON-RPC with
  `serde_json` + `tokio` (no new crate); a connection hub that keeps one live agent process per connected agent; and the ACP agent
  configuration (presets, validation, storage).
- Manual Connect / Disconnect per agent with a status (`disconnected`, `connecting`, `connected`, `error`) shown in Settings and the commit box.
- Five preset ACP agents: Devin, OpenCode, Claude Code, Codex, Gemini.
- A Settings section "ACP agents": per agent enable/disable, command line (command + args), an optional commit prompt, status, and Connect / Disconnect.
- Settings keys: `acp.agents` (JSON overrides of the presets) and `commit.agent` (the picked agent id).

Out:
- Agents running tools in the repo, agent-made commits, chat with an agent, streaming the reply into the textarea.
- Force push, pull, amend, `--no-verify`, branch operations, Discard all, hunk/line staging.
- Idle timeout, automatic reconnect, reusing one session across commits, adding agents beyond the five presets, a separate env-var field, per-project ACP config, a model picker.
- Any change to `crates/protocol`, `crates/daemon`, or terminal sessions.
- `git_view.rs` stays read-only for Git state; writes live in `git_write.rs`.

## Behavior

Staging
- Rows in Unstaged, Untracked, and Conflicts show a `+` button; Staged rows show a `−` button. The Staged header has
  "Unstage all"; the Unstaged and Untracked headers have "Stage all".
- Stage runs `git add -A -- <path>` (deletions and renames are staged). Unstage runs `git restore --staged -- <path>`, or
  `git rm --cached -q -- <path>` when the repository has no commit yet. "All" variants take no path.
- After any write the changes list and open diffs refetch.

Discard
- Rows in Unstaged and Untracked show a Discard button that opens a confirmation dialog naming the file and saying the
  change cannot be recovered. Staged and Conflicts rows have no Discard.
- Confirmed Discard of a tracked unstaged file runs `git restore -- <path>` (the working tree returns to the index).
  Discard of an untracked file deletes that regular file or symlink (the link, not its target); it never deletes a directory.

Commit
- The commit box sits at the bottom of the changed-files aside. Commit is enabled only with at least one staged entry,
  no conflicts, and a non-empty trimmed message. `Cmd/Ctrl+Enter` in the textarea commits.
- Commit runs `git commit -m <message>` in the checkout with hooks enabled; never `--no-verify`, never `--amend`. The message is
  one argv entry, no shell. On success the textarea clears, queries refetch, and a status line shows
  `Committed <short-hash> on <branch>`. On failure (hook rejected, missing `user.name`/`user.email`, nothing staged) Git's
  stderr is shown in an alert and the message is kept.

Push
- A Push button beside Commit shows `↑N` when the branch has an upstream and N commits to push (`ahead`), and "Push new branch"
  when there is no upstream. It is disabled when `ahead` is 0.
- Push runs `git push` when an upstream exists, else `git push -u origin HEAD`; with no `origin` remote it fails with a clear error.
  It never passes `--force` or `--force-with-lease`. Git runs with `GIT_TERMINAL_PROMPT=0` so a missing credential fails instead of
  hanging, and the whole command is killed after 120 s. Hooks (`pre-push`) run. Success shows Git's summary line in the status line;
  failure shows stderr in the alert. `ahead` refetches afterwards.

Connect
- Each enabled agent has a status: `disconnected` (initial), `connecting`, `connected`, or `error` with a message. Settings → ACP agents shows a
  status dot and a Connect / Disconnect button per row; the commit box shows the picked agent's status and a Connect button when it is
  `disconnected` or `error`. Both call the same commands.
- Connect spawns the agent process (program, args, and `NAME=value` env from its command line) with a dedicated empty temp directory as its cwd and
  sends `initialize` (protocol version 1, no `fs`/`terminal` capability). It becomes `connected` when `initialize` succeeds, within 60 s; otherwise
  the process is killed and the status is `error` with the reason. Connecting an already connected agent does nothing.
- Disconnect kills the process and removes its temp directory; status returns to `disconnected`. The process also ends when the app quits.
- If the process exits on its own, the status becomes `error` ("exited unexpectedly" plus the exit code); there is no automatic reconnect.
- Disabling an agent, or editing its command line, disconnects it first. An agent that is `connecting` is cancelled the same way.
- Connected agents stay alive until Disconnect, crash, or app quit; there is no idle timeout. Settings shows which agents are connected, since each
  holds an agent process (and whatever MCP servers it started).

Generate
- Generate is enabled only when something is staged, no conflicts exist, and the picked agent is `connected`. The picker lists enabled agents with their status;
  unavailable ones (first command word not on PATH) are disabled. The pick is stored in `commit.agent`; with no valid stored pick the first
  connected agent, else the first enabled available agent, is used. Nothing is sent to any agent until the user presses Generate.
- Each Generate opens a **new session** (`session/new`) on the live connection, so earlier commits' context never leaks in, and the session ends afterwards
  (`session/close` when the agent advertises it). Its cwd is a fresh empty temp directory (removed afterwards), not the checkout; `mcpServers: []`.
- The prompt is the agent's commit prompt (the preset default unless overridden) followed by fixed context: the last 5 commit subjects
  (style hint), `git diff --cached --stat`, and `git diff --cached` capped at 120 KiB (a "diff truncated" line is added when capped).
  The default prompt asks for a single conventional message (subject ≤ 72 chars, optional body), no code fences, no commentary, no tool use.
- Every `session/request_permission` from the agent is answered `cancelled`; any other agent request gets JSON-RPC error `-32601`.
- Text from `agent_message_chunk` updates of that session is concatenated until `session/prompt` returns `stopReason: "end_turn"`. The reply is trimmed and
  one surrounding ``` fence is removed. It replaces the textarea content only if the user has not typed since pressing Generate;
  otherwise it is dropped with a short notice.
- While running, Generate becomes Cancel with "Asking <agent>…". Cancel sends `session/cancel` for that session and stops waiting; the connection stays
  `connected`. One generation runs per checkout; starting another cancels the previous. Overall timeout 180 s, after which the session is cancelled and an error is shown.
  (Measured one-shot on this machine: OpenCode took 52 s and about 62k input tokens for a one-line prompt, mostly model processing of its own skills and commands;
  Devin loads its configured MCP servers first. Keeping the process connected removes process and MCP start-up from each Generate but not model time; the saving
  is measured in the manual check and not promised.)

ACP agent configuration (Settings → "ACP agents")
- The section lists the five presets. Each row has an Enabled switch, a command-line field, an optional commit-prompt textarea, and
  "Reset to default". Changes apply immediately and persist under `acp.agents` as JSON `[{id, enabled, command, prompt}]`, only for rows that differ
  from the preset. Missing, unreadable, or invalid stored JSON means all presets.
- The command line is split with shell-style quoting (`shlex`). Leading `NAME=value` words become environment variables for the child
  (so Devin's preset can carry `DEVIN_PERMISSION_MODE=auto`); the next word is the program, the rest are arguments. No shell is involved.
  A command that is empty, has an unterminated quote, or has no program word is rejected with an inline error and not saved.
- Presets (the Claude and Codex adapters are unverified until the manual check in `Verification`):
  - Devin: `DEVIN_PERMISSION_MODE=auto devin acp --model swe-2-flash-max`
  - OpenCode: `opencode acp`
  - Claude Code: `npx -y @zed-industries/claude-agent-acp`
  - Codex: `npx -y @zed-industries/codex-acp`
  - Gemini: `gemini --experimental-acp`
- All presets are enabled by default. Availability is the program word resolved on PATH; for the `npx` presets the adapter downloads on first run, so
  the first Generate can be slow or fail offline.

## Edge cases

- Agent program missing at Connect: status `error`, "<label> was not found on PATH" with a hint to fix it in Settings → ACP agents.
- `session/new` fails after a successful `initialize` that advertised `authMethods` (for example OpenCode before login): the Generate error shows the agent's message and a
  hint to sign in with that agent in a terminal; the connection stays `connected`.
- Generate while the agent is not connected (for example it crashed between render and click): rejected with "<label> is not connected".
- Agent process dies during a Generate: that Generate fails with the exit reason and the status becomes `error`.
- App quits or reloads the webview while connected: the process is killed on app quit; a webview reload keeps connections (state lives in the backend).
- `session/prompt` returns a `stopReason` other than `end_turn` (`cancelled`, `refusal`, `max_tokens`): error naming the reason; the textarea is untouched.
  Non-JSON stdout lines are ignored; stderr is discarded.
- Empty reply after cleanup: error "The agent returned no message."
- Nothing staged or conflicts present: Generate and Commit are disabled and the commands reject the request.
- Binary-only staged changes: the diff holds only "Binary files differ" lines; the agent still gets `--stat`.
- Checkout switches or the component unmounts while a job runs: the session is cancelled; the connection stays up.
- `index.lock` held by another Git process, or any Git write fails: Git's error is shown; nothing is retried.
- Discard of a path that no longer exists, resolves outside the checkout, or is a directory: rejected with an error; nothing is deleted.
- Push rejected as non-fast-forward, auth failure, no network, or 120 s timeout: Git's message or "Push timed out" is shown; no retry, no force.
- Prompt injection through diff content: the reply only fills the textarea and the user reviews it before Commit. Permission requests are denied
  and the cwd is empty. Known limit: read-only tools an agent auto-approves can still read files outside the temp dir, and `claude`/`codex`/`gemini`
  adapters apply their own permission policy.
- The staged diff is sent to the chosen agent's model provider; the user sees which agent is picked before pressing Generate.
- Invalid custom prompt: longer than 4 KiB is rejected inline; an empty prompt means the preset default.

## Interfaces

Tauri commands (git commands validate the checkout with the existing `git_view_checkout` path; `paths: None` means all):
- `git_write_stage(project_id, checkout_path, paths: Option<Vec<String>>)`, `git_write_unstage(...)` with the same shape.
- `git_write_discard(project_id, checkout_path, layer: "unstaged" | "untracked", path)`.
- `git_write_commit(project_id, checkout_path, message) -> { head, branch }`.
- `git_write_push(project_id, checkout_path) -> { summary }`.
- `git_view_changes` result gains `ahead: number | null` (`null` = no upstream).
- `acp_agents_get() -> AcpAgent[]` where `AcpAgent = { id, label, enabled, command, prompt, available, is_default }`.
- `acp_agents_set(overrides: { id, enabled, command, prompt }[])`: validates and stores `acp.agents`.
- `acp_connect(agent_id)`, `acp_disconnect(agent_id)`, `acp_status() -> { [agent_id]: { state: "disconnected" | "connecting" | "connected" | "error", message: string | null } }`.
  The frontend refetches `acp_status` every 3 s while Settings or the Changes view is visible, and after every connect, disconnect, or generate.
- `acp_generate_commit_message(project_id, checkout_path, agent_id) -> string`; `acp_cancel(project_id, checkout_path)`.
- Settings key `commit.agent` through the existing `settings_get` / `settings_set`.

Frontend: `services/git-write.ts`, `services/acp.ts`, hooks in `features/editor/queries.ts` (invalidate `gitChanges` and `git-diff`),
`features/editor/CommitBox.tsx`, row actions and a discard confirmation in `EditorChanges.tsx`, and an "ACP agents" section in `SettingsPage.tsx`.

ACP wire (protocol version 1, newline-delimited JSON-RPC 2.0): `initialize`, `session/new`, `session/prompt`, `session/close` (when advertised), notifications
`session/update` (`agent_message_chunk`) and `session/cancel`, response to `session/request_permission`. Verified against installed `devin acp` and `opencode acp`.

## Verification

- Rust `git_write` tests in a temp repository: stage new/modified/deleted/renamed, unstage with and without HEAD, all variants, discard tracked and
  untracked, discard of a directory/outside path rejected, commit success, empty message, nothing staged, failing pre-commit hook keeps the index,
  path escape rejected, push to a local bare remote (with and without upstream, never force, non-fast-forward rejected), `ahead` values.
- Rust `acp` tests against a fake agent script speaking ACP: connect reaches `connected`; connect timeout and bad program give `error`; disconnect kills the process;
  a crash flips the status to `error`; two Generates on one connection use two different sessions; chunked reply; permission request answered `cancelled`;
  unknown request gets `-32601`; `refusal`; non-JSON noise; Cancel and timeout cancel the session but keep the connection; Generate on a disconnected agent is rejected;
  disabling or editing a connected agent disconnects it; fence stripping; config tests: preset defaults, override merge, `NAME=value`
  env words, shlex errors, invalid JSON falls back to presets, availability.
- Wiring test: command names and shapes (`ahead`, commit/push results, `AcpAgent`) match the frontend types.
- Vitest: status dot and Connect / Disconnect in Settings and the commit box, CommitBox rules, Generate fill / dropped after typing / cancel / error, stage / unstage / discard-confirm calls and invalidation, Push labels,
  Settings "ACP agents" edit / reset / validation error.
- Manual (cannot be automated): in a scratch repo with a local bare remote, Connect each of Devin, OpenCode, Gemini, Claude Code, and Codex, stage a file, Generate with each (twice, to time the second run against the one-shot numbers above), then Disconnect. Original steps: Generate with each of Devin, OpenCode, Gemini, Claude Code, and Codex,
  edit, Commit, Push, and confirm `git log`. Record which presets work; fix presets that do not.
- `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`, `pnpm -C apps/desktop typecheck`, `pnpm -C apps/desktop test`.

## Related context

- [Desktop Editor](../../../../llm/architecture/editor.md)
- [Desktop Settings](../../../../llm/domains/desktop-settings.md)
- [Codebase wiki overview](../../../../llm/architecture/overview.md)
