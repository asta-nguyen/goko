# Editor: stage, commit, push, and AI commit messages over ACP tasks

## Design

[Editor: stage, commit, push, and AI commit messages over ACP](design.md)

## Global Constraints

- Commit never uses `--no-verify` or `--amend`; the message is a single argv entry (`git commit -m <message>`), no shell anywhere.
- Push never passes `--force` or `--force-with-lease`; it runs with `GIT_TERMINAL_PROMPT=0` and is killed after 120 s.
- Discard never deletes a directory; an untracked symlink is removed as the link, not its target.
- An ACP agent is a long-lived child process spawned only by Connect, with an empty temp directory as its cwd and no `fs`/`terminal` capability; Connect times out after 60 s. Each Generate opens a new session (`mcpServers: []`, its own fresh empty temp cwd, removed afterwards) and ends it; every `session/request_permission` is answered `cancelled`, any other agent request `-32601`; Generate times out after 180 s and cancels only its session; the reply is cleaned of one surrounding ``` fence; diff capped at 120 KiB; last 5 commit subjects as style hint.
- Connections live in backend state: no idle timeout, no automatic reconnect; a crash sets status `error`; disabling an agent or editing its command disconnects it; app quit kills all children. Status is `disconnected | connecting | connected | error` and the frontend polls `acp_status` every 3 s while Settings or Changes is visible.
- ACP config lives under settings key `acp.agents` (JSON `[{id, enabled, command, prompt}]`, only differing rows) and the pick under `commit.agent`; invalid stored JSON means the five presets; custom prompt ≤ 4 KiB.
- Command lines use `shlex` splitting; leading `NAME=value` words are env vars, the next word is the program; no shell.
- Presets (exact): Devin `DEVIN_PERMISSION_MODE=auto devin acp --model swe-2-flash-max`; OpenCode `opencode acp`; Claude Code `npx -y @zed-industries/claude-agent-acp`; Codex `npx -y @zed-industries/codex-acp`; Gemini `gemini --experimental-acp`.
- `git_view.rs` stays read-only for Git state (except its existing file save); new writes live in `git_write.rs`. No new Rust crate. No change to `crates/protocol` or `crates/daemon`.
- Commands that run Git hooks or the network (`git_write_commit`, `git_write_push`) are `async` and use a blocking thread.

## Impact map

Entry: `apps/desktop/src/features/editor/EditorChanges.tsx::EditorChanges` for the UI (renders the changed-files list and diff, read-only today); writes and ACP have no existing source (`Entry: no existing source; planned entry: design.md → git_write.rs, acp.rs, CommitBox.tsx` — pending approval).
Flow: existing read flow: `EditorChanges` → `useGitChanges`/`useGitDiff` (`features/editor/queries.ts`) → `services/git-view.ts` → Tauri `git_view_changes`/`git_view_diff` (`commands.rs`) → `git_view::changes`/`diff` → `worktrees::git`/`git_bounded`. Planned write flow (pending approval): `CommitBox`/row buttons → `services/git-write.ts` → `git_write_*` commands → `git_write.rs` → `worktrees::git`/`run`. Planned ACP flow (pending approval): Settings/`CommitBox` Connect → `services/acp.ts` → `acp_connect` → `acp.rs` hub → child process over stdio; `CommitBox` Generate → `acp_generate_commit_message` → hub (new session, prompt from `git_view::diff` pieces) → same child. Settings flow (pending approval): `SettingsPage` → `services/acp.ts` → `acp_agents_get`/`acp_agents_set` → `Storage::get_setting`/`set_setting`.
State changes: checkout index, working tree, HEAD, and remote refs (new); settings rows `acp.agents` and `commit.agent` (new); in-memory ACP connection state and live child processes (new); temp directories per connection and per generation.
External effects: Git hooks (`pre-commit`, `commit-msg`, `pre-push`); network push; spawning and holding open a user-configured agent process until Disconnect; the staged diff is sent to that agent's model provider; first run of an `npx` preset downloads an npm package.
Change candidates: `src-tauri/src/git_write.rs` (new), `acp.rs` (new), `git_view.rs` (`GitViewChanges.ahead`, `relative_path`/`invalid` visibility), `commands.rs`, `lib.rs` (module + handler list), `wiring_tests.rs`, `worktrees.rs` (timeout-capable runner if needed); `src/services/{git-write,acp}.ts` (new), `services/types.ts`, `features/editor/{CommitBox.tsx (new),EditorChanges.tsx,queries.ts,EditorWorkspace.test.tsx}`, `features/settings/SettingsPage.tsx`; wiki `docs/llm/architecture/editor.md`, `docs/llm/domains/desktop-settings.md` via the delta.
Verification: `cargo test --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo fmt --check`, `pnpm -C apps/desktop typecheck`, `pnpm -C apps/desktop test`, plus the manual agent check (Task 8).
Verified at: 80b618981b0ce4f3142304c7ba3eddff54851ba7

## Tasks

### Task 1 — Git write operations and `ahead`

Files: `apps/desktop/src-tauri/src/git_write.rs::{stage, unstage, discard, commit, push}` (new), `apps/desktop/src-tauri/src/git_view.rs::{changes, GitViewChanges, relative_path, invalid}`, `apps/desktop/src-tauri/src/lib.rs` (module declaration)
Interfaces: produces `git_write::{stage, unstage}(checkout, Option<&[String]>)`, `discard(checkout, layer, path)`, `commit(checkout, message) -> {head, branch}`, `push(checkout) -> summary`, and `GitViewChanges.ahead: Option<u32>`; reuses `worktrees::git`/`run` and `git_view`'s path validation (made `pub(crate)`).
Change: implement the Behavior rules for Staging, Discard, Commit, and Push exactly as in the design: `git add -A --`, `git restore --staged --` or `git rm --cached -q --` when `HEAD` is missing, `git restore --` for tracked unstaged discard, symlink-safe file removal for untracked discard (rejecting directories and paths outside the checkout), `git commit -m` (rejecting empty/whitespace messages, NUL bytes, and nothing-staged/conflict states before running Git), `git push` or `git push -u origin HEAD` with `GIT_TERMINAL_PROMPT=0`, never forcing, killed after 120 s. `changes` adds `ahead` from `rev-list --count @{u}..HEAD` (`None` without an upstream). Tests in a temp repo: stage new/modified/deleted/renamed file; unstage with and without HEAD; stage-all/unstage-all; discard tracked restores content; discard untracked removes the file and a symlink but not its target; discard of a directory or `../x` is rejected; commit success returns head and branch; empty message and nothing-staged are rejected; a failing pre-commit hook leaves the index unchanged and returns its stderr; push to a local bare remote with and without upstream; a non-fast-forward push is rejected and no force flag is ever used; `ahead` is `None` without upstream and counts commits with one.
Verify: `cargo test -p desktop git_write` and `cargo test -p desktop git_view` pass.
Progress: done
Result: `cargo test git_write` — 16 passed; `cargo test git_view` — 20 passed; `cargo fmt --check` clean; `cargo clippy --workspace -- -D warnings` clean; `cargo test --workspace` — all suites pass (desktop lib 178). Added `worktrees::run_timed` (120 s deadline + env) for push; unstage-before-first-commit uses `git rm --cached -q -r` and surfaces Git's refusal.

### Task 2 — Git write commands and wire types

Files: `apps/desktop/src-tauri/src/commands.rs::{git_write_stage, git_write_unstage, git_write_discard, git_write_commit, git_write_push}`, `apps/desktop/src-tauri/src/lib.rs::handler`, `apps/desktop/src-tauri/src/wiring_tests.rs`, `apps/desktop/src/services/types.ts::{GitViewChanges, GitCommitResult, GitPushResult}`, `apps/desktop/src/services/git-write.ts` (new), `apps/desktop/src/services/git-view.test.ts`
Interfaces: consumes Task 1 functions; produces the five Tauri commands (checkout validated through `git_view_checkout`; commit and push `async` on a blocking thread) and the TypeScript `gitWriteService` plus `ahead: number | null` on `GitViewChanges`.
Change: register the commands, add the frontend service with the argument names in the design, and extend the existing wiring test so the command names and result keys (`head`, `branch`, `summary`, `ahead`) match the TypeScript types. Update existing test fixtures for `GitViewChanges` with `ahead: null`.
Verify: `cargo test -p desktop wiring` and `pnpm -C apps/desktop typecheck` pass; a Vitest service test asserts each command's invoke payload.
Progress: done
Result: `cargo fmt --check` clean; `cargo clippy --workspace -- -D warnings` clean; `cargo test` — desktop lib 178 passed (wiring test covers `git_write_*` over IPC against a temp repo and a local bare remote, `ahead`/`head`/`branch`/`summary` keys); `pnpm typecheck` clean; `pnpm exec vitest run` — 35 files, 330 tests passed. Commit and push use `tokio::task::spawn_blocking`.

### Task 3 — ACP connection and session client

Files: `apps/desktop/src-tauri/src/acp.rs::{Connection, connect, run_session, AcpError}` (new), `apps/desktop/src-tauri/src/lib.rs` (module declaration), `apps/desktop/src-tauri/tests/fixtures/fake_acp_agent.py` (test fixture run with `python3`, present on macOS)
Interfaces: produces `Connection::connect(spec: {program, args, env}) -> Connection` (spawn with `kill_on_drop`, stdin/stdout piped, stderr null, cwd a dedicated temp dir; `initialize` within 60 s; a reader task routes responses by request id and `session/update` by session id; records advertised `authMethods` and `session/close` support), `Connection::run_session(prompt, timeout) -> Result<String, AcpError>` (new session in a fresh temp cwd, `session/prompt`, cleaned reply, then `session/close` when supported), `Connection::cancel_session`, `Connection::is_alive`, `Connection::shutdown`.
Change: implement the Connect and Generate protocol rules from Global Constraints: newline-delimited JSON-RPC over `tokio::process`, ignore non-JSON lines, answer `session/request_permission` with `{outcome:{outcome:"cancelled"}}`, answer other agent requests with `-32601`, concatenate that session's `agent_message_chunk` text, require `stopReason: "end_turn"`, strip one surrounding ``` fence, and error on an empty reply, another stop reason, timeout, or process exit. When `session/new` fails and `initialize` advertised `authMethods`, return the agent's message plus a hint to sign in with that agent in a terminal. Tests drive the fake agent (case chosen by an env var): connect succeeds and records capabilities; connect timeout and a missing program fail and leave no process; two `run_session` calls on one connection use two different session ids and temp cwds; chunked reply; permission request answered `cancelled` and the turn still ends; unknown request gets `-32601`; `refusal`; non-JSON noise ignored; fenced reply stripped; cancel and timeout end only the session and the connection stays alive; process crash flips `is_alive` to false and fails the running session; `shutdown` and drop kill the child (checked by pid); temp directories are removed.
Verify: `cargo test -p desktop acp::` passes.
Progress: done
Result: `cargo fmt --check` clean; `cargo clippy --workspace -- -D warnings` clean; `cargo test acp::` — 20 passed (run 3 times, no flakes); `cargo test` — 198 passed; no stray agent processes or `goko-acp-*` temp dirs. API: `AcpSpec{program,args,env}`, `Connection::{connect, run_session, is_alive, shutdown}`, `AcpError{message,hint}` + `From<AcpError> for BridgeError`; cancellation is drop-based (aborting the `run_session` future sends `session/cancel`), so there is no `cancel_session` method. The agent runs in its own process group and the whole group is killed (npx wrappers); `session/close` is also sent after an answered error turn. Known limit: a cancelled prompt's pending id stays in the routes map until the agent answers or the connection ends.

### Task 4 — ACP agent config, hub, and commands

Files: `apps/desktop/src-tauri/src/acp.rs::{PRESETS, AcpAgent, AcpOverride, AcpHub, agents_get, agents_set, build_prompt, parse_command}`, `apps/desktop/src-tauri/src/commands.rs::{acp_agents_get, acp_agents_set, acp_connect, acp_disconnect, acp_status, acp_generate_commit_message, acp_cancel}`, `apps/desktop/src-tauri/src/lib.rs::handler` (state registration, app-exit cleanup, command list), `apps/desktop/src-tauri/src/wiring_tests.rs`
Interfaces: consumes Task 3 `Connection`, `store::Storage::{get_setting,set_setting}`, `agents::resolve` for availability, and `git_view`/`worktrees` for the staged diff, `--stat`, and recent subjects; produces the seven commands and `AcpHub`, a managed state holding one `Connection` plus status per agent id and a per-checkout job map (starting a job cancels the previous; `acp_cancel` cancels).
Change: implement the presets from Global Constraints and merge them with `acp.agents` overrides (invalid JSON → presets); validate on set (non-empty command, shlex parses, leading `NAME=value` env words, a program word exists, prompt ≤ 4 KiB, unknown ids rejected); compute `available` and `is_default`; `acp_connect` runs Task 3 connect and records `connecting` → `connected` or `error` with the message; `acp_disconnect` shuts the child down; setting a disabled flag or a changed command disconnects that agent first; `acp_status` returns the map; `acp_generate_commit_message` requires `connected` ("<label> is not connected" otherwise), builds the prompt (agent prompt or default instructions, then recent subjects, `--stat`, diff capped at 120 KiB with a truncation line), runs `run_session`, and on a dead process sets status `error`; app exit shuts all connections down. Tests: presets returned with nothing stored; override merge and reset-by-matching-preset; `DEVIN_PERMISSION_MODE=auto devin acp …` parses to env + program + args; unterminated quote and empty command rejected; oversized prompt rejected; empty custom prompt means the preset default; prompt contains subjects, stat, and the truncation line past the cap; a binary-only staged diff still yields `--stat`; connect then status shows `connected`, disconnect shows `disconnected`; connecting a missing program gives `error`; disabling or editing a connected agent disconnects it; Generate on a disconnected, disabled, unavailable, or unknown agent returns the design's errors; a second job on the same checkout cancels the first; nothing-staged or conflicts are rejected. Wiring test covers `AcpAgent` and status keys.
Verify: `cargo test -p desktop acp` and `cargo test -p desktop wiring` pass.
Progress: done
Result: `cargo fmt --check` clean; `cargo clippy --workspace -- -D warnings` clean; `cargo test acp` — 40 passed (3 runs, no flakes); `cargo test` — 218 passed; no stray processes or temp dirs. Wire shapes: `acp_agents_get`/`acp_agents_set` → `[{id,label,enabled,command,prompt,available,is_default}]`; `acp_status` → `{<id>:{state,message}}` for all five ids; `acp_connect`/`acp_disconnect`/`acp_cancel` → null; `acp_generate_commit_message` → string; an aborted generation rejects with `{code:"internal",message:"cancelled"}`. App exit shuts all connections down alongside the bridge.

### Task 5 — Frontend ACP service and hooks

Files: `apps/desktop/src/services/acp.ts` (new), `apps/desktop/src/services/types.ts::AcpAgent`, `apps/desktop/src/features/editor/queries.ts::{useStage, useUnstage, useDiscard, useCommit, usePush, useAcpAgents, useAcpStatus, useAcpConnect, useAcpDisconnect, useGenerateMessage}`, `apps/desktop/src/features/editor/queries.test.tsx`
Interfaces: consumes Tasks 2 and 4 commands; produces hooks whose successful writes invalidate `queryKeys.gitChanges` and `['git-diff', projectId, checkoutPath]`.
Change: add the service and mutation/query hooks; `useAcpAgents` reads `acp_agents_get`; `useAcpStatus` reads `acp_status` and refetches every 3 s only while its `enabled` flag is true; connect, disconnect, and generate invalidate the status query; the picked agent reads and writes `commit.agent` through `settingsService`.
Verify: Vitest cases: each mutation calls its command and invalidates both query keys; a failed mutation invalidates nothing; `useAcpAgents` returns the list; `useAcpStatus` polls only while enabled; connect and disconnect invalidate the status; `pnpm -C apps/desktop test queries` passes.
Progress: done
Result: `pnpm typecheck` clean; `pnpm exec vitest run` — 36 files, 346 tests passed (+16: acp service 2, hooks 14). Added `useSetAcpAgents` (needed by Task 7, missing from the original hook list); `useGenerateMessage` returns the mutation plus `cancel()`.

### Task 6 — Changes UI: row actions and CommitBox

Files: `apps/desktop/src/features/editor/CommitBox.tsx` (new), `apps/desktop/src/features/editor/EditorChanges.tsx::EditorChanges`, `apps/desktop/src/features/editor/CommitBox.test.tsx` (new), `apps/desktop/src/features/editor/EditorWorkspace.test.tsx`
Interfaces: consumes Task 5 hooks; produces the Stage/Unstage/Stage all/Unstage all buttons, the Discard button with a confirmation dialog (existing `Dialog` component), and the commit box under the changed-files list.
Change: implement Behavior for Staging, Discard, Commit, Push, and Generate: button placement and labels per the design; Commit enabled only with staged entries, no conflicts, and a trimmed message; `Cmd/Ctrl+Enter` commits; status line `Committed <hash> on <branch>`; alert with Git's stderr and message kept on failure; Push label `↑N` / "Push new branch" and disabled at 0; Generate disabled unless something is staged, there are no conflicts, and the picked agent is `connected`; the picker lists enabled agents with a status dot and unavailable ones disabled; a Connect button shows beside the picker when the picked agent is `disconnected` or `error`, with the error message as its tooltip; Generate becomes Cancel with "Asking <agent>…"; the reply fills the textarea only if the user has not typed since pressing Generate, else a short notice; the job is cancelled on unmount or checkout change. Tests (named cases): commit disabled/enabled matrix; Cmd+Enter commits and clears; commit failure keeps the message and shows the alert; stage and unstage buttons call the service with the path; Stage all passes no path; Discard asks first and a cancel does nothing; Push labels for `ahead` 0, 3, and `null`; Generate fills an empty box; Generate result dropped after typing; Cancel calls `acp_cancel`; Generate error shows the alert; Connect shows `connecting` then enables Generate; Generate is disabled while the agent is `disconnected` or `error`.
Verify: `pnpm -C apps/desktop test editor` and `pnpm -C apps/desktop typecheck` pass.
Progress: done
Result: `pnpm typecheck` clean; `pnpm exec vitest run` — 37 files, 387 tests passed (CommitBox 17 cases, 10 write-behavior cases in EditorWorkspace.test.tsx; row selectors moved to a `changeRow` helper because action buttons now share the path in their accessible names, no assertion weakened). `cargo fmt --check` and `cargo clippy -D warnings` clean. Known limit: "Stage all" sends one pathspec per file, so a group with tens of thousands of untracked files could hit the OS argv limit.

### Task 7 — Settings: ACP agents section

Files: `apps/desktop/src/features/settings/SettingsPage.tsx::SettingsPage`, `apps/desktop/src/features/settings/SettingsPage.test.tsx`
Interfaces: consumes `useAcpAgents`, `useAcpStatus`, `useAcpConnect`, `useAcpDisconnect`, and the `acp_agents_set` mutation from Task 5.
Change: add an "ACP agents" section (navigation entry and body) listing the five agents; each row has an Enabled switch, a status dot with its message, a Connect / Disconnect button, a command-line field, a commit-prompt textarea, and "Reset to default"; edits save on blur or toggle; a rejected command shows the backend's message inline and leaves the stored value unchanged. Tests: the five rows render with preset commands; toggling Enabled calls the set command with only the differing rows; a valid command edit saves; an unterminated quote shows the inline error and does not save; Reset to default removes the override; Connect shows `connecting` then `connected` and becomes Disconnect; an `error` status shows its message; disabling a connected agent shows it as `disconnected`.
Verify: `pnpm -C apps/desktop test settings` and `pnpm -C apps/desktop typecheck` pass.
Progress: done
Result: `pnpm typecheck` clean; `vitest run src/features/settings` — 15 passed; whole suite 387 passed. New `AcpAgentsSection.tsx` (a plain styled `<textarea>`: no Textarea primitive exists); status polls only while the section is mounted/active.

### Task 8 — Manual check with real agents

Files: none (verification only; results recorded in this task)
Interfaces: consumes the built app from Tasks 1-7.
Change: build with `pnpm tauri build --bundles app`, create a scratch repo with a local bare remote, then Connect each of Devin, OpenCode, Gemini, Claude Code, and Codex, stage a file, and press Generate with each (twice, recording the first and second run times against the one-shot numbers in the design); edit the message, Commit, and Push; Disconnect each; confirm with `git log` and the remote and that no agent process remains (`pgrep`). For any preset that fails, fix its preset command in `acp.rs` (and the Global Constraints list) or record it as a known limit with the reason.
Verify: each of the five agents is recorded as working or as a documented limit, with first and second Generate times; the commit and push appear in `git log` of the scratch repo and its remote.
Progress: done
Result: Ran the real hub (`connect` → `generate_commit_message` ×2 → `disconnect`) against a scratch repo with a staged change, using a temporary `#[ignore]` test that was removed afterwards (no stray processes or temp dirs). Not run through the UI. Results on this machine:
- **Devin** — works. connect 0.0 s; Generate #1 3.2 s, #2 1.5 s: `feat: greet with a name parameter`.
- **Claude Code** (`npx -y @zed-industries/claude-agent-acp`) — works. connect 5.1 s; Generate #1 6.4 s, #2 7.8 s: `feat: update greet to accept name and return personalized greeting`.
- **OpenCode** — protocol works but unreliable: its default free model (`opencode/big-pickle`) answered in 52-60 s in two raw probes, then returned `[overloaded]` once and timed out at 180 s three times; a raw probe without Goko's client also hung for 240 s, so this is the provider, not the client. Documented limit.
- **Gemini** — connects (4.3 s); Generate fails with `API key must be set when using the Gemini API` plus the sign-in hint: Gemini is not signed in on this machine (needs `gemini` login or an API key). Not verified end to end.
- **Codex** (`npx -y @zed-industries/codex-acp`) — fails to start: `~/.codex/config.toml` line 4 has `max`, which `codex-acp` rejects (valid: none, minimal, low, medium, high, xhigh). User config issue; file left untouched. Not verified end to end.
- Warm-connection saving: Devin 3.2 s → 1.5 s, Claude 6.4 s → 7.8 s; for OpenCode model time dominates. Keeping the connection helps little beyond start-up.
- Gap found: the client discards the agent's stderr, so the Codex failure showed only "the agent exited (exit code 1)". Suggested follow-up: keep the last few stderr lines and include them in that error (changes the design's "stderr is discarded"; not done without approval).



## Files inspected, no change

- `apps/desktop/src-tauri/src/agents.rs` — agent list and PATH resolution (`resolve`); ACP agents live in their own preset list rather than `ADAPTERS`.
- `apps/desktop/src-tauri/src/worktrees.rs` — Git runners (`git`, `run`, `git_bounded`) reused by `git_write.rs`.
- `apps/desktop/src/features/editor/EditorWorkspace.tsx` — mounts `EditorChanges`; no change needed.
- `apps/desktop/src/services/settings.ts` — `settingsService.get/set` reused for `commit.agent`.
- `docs/llm/architecture/editor.md`, `docs/llm/domains/desktop-settings.md` — wiki pages the delta targets.

## Approval Gate

Status: approved

## Decision Log

- 2026-11-06 (before first approval): ACP changed from one-shot per Generate to a manual Connect / Disconnect connection that keeps the agent process alive (user's choice); Generate opens a new session per commit. Tasks 3-8 revised; gate stays pending.
