# Git worktree management UX tasks

## Design

[Git worktree management UX](design.md)

## Global Constraints

- Git is invoked as separate argv values through `std::process::Command`; no shell string is ever built and no new dependency or Tauri capability is added. The protocol shape stays unchanged; the daemon refreshes live process `cwd` through the existing `SessionInfo` field.
- Worktree removal is fail-closed: only a clean, non-root linked worktree with zero daemon sessions attached to its path — including exited ones — is removed, re-checked in the backend at remove time; sessions are never killed or closed for removal, removal never uses force, the local branch is always preserved, and removal is refused when the daemon cannot confirm the session list or any session's current directory.
- Removal also requires every daemon session to be exited or failed, regardless of project, and a daemon version that supports live process CWD; daemons older than 0.1.1 are refused without an automatic restart.
- Session creation and worktree removal use the bridge's in-process read/write gate plus a shared/exclusive `flock` under `GOKO_HOME`: the daemon takes the shared lock for every `SessionCreate`, and Tauri removal takes the exclusive lock before checking sessions and running Git.
- `daemon_status` reports whether the connected daemon supports live CWD; blocked remove actions remain keyboard-focusable and expose their reason on focus and hover.
- The removal confirmation states that the checkout directory and its local/ignored contents, including a copied `.env`, will be deleted.
- Environment copying never follows symlinks, never overwrites an existing destination file, and never removes the created worktree when copying fails — the worktree is retained with a retry path.
- Removing a project registration never deletes the project root or any worktree directory.
- Checkout tab state stays in memory under the existing terminal-workspaces persistence limit; session association derives from the recorded `project_id` and `cwd`, not from new schema.
- New UI uses the existing semantic tokens and installed Radix Dialog primitive; checkout rows, disabled-branch reasons, and dialog fields are keyboard-operable and labeled, validation errors are announced, and focus returns to the trigger on close. TypeScript stays `strict` with no `any` without a comment, terminal bytes never enter React state, Zustand, or the query cache, and automated tests never launch a real agent CLI.

## Impact map

```text
Entry: `ProjectTree.tsx::ProjectTree` and `commands.rs::session_create`; live CWD flows through `SessionManager::{create,list,get}` and `PtyProcess`'s child PID, while worktree removal enters `worktrees::remove`.
Flow: ProjectTree → global session list/status → `session_create` → bridge in-process read gate → daemon `SessionCreate` shared flock → `SessionManager::create` → `PtyProcess`; daemon CWD refresh → `SessionInfo.cwd` and `session.updated` → Tauri event/query cache → checkout grouping; removal → in-process exclusive lock plus `GOKO_HOME` exclusive flock → handshake version check → fresh global `session.list` → no-live-session and CWD containment/unknown checks → `git worktree remove`.
State changes: in-memory `SessionInfo.cwd` changes as the running process changes directory; session metadata still stores its original launch context and no SQLite or protocol schema change is made. Git worktree directories and opt-in copied `.env` files change on disk.
External effects: Git subprocesses list/create/remove worktrees; environment files copy locally; daemon reads OS process metadata for live CWD and launches sessions in the validated checkout.
Change candidates: `crates/daemon/{Cargo.toml,src/pty.rs,src/session.rs}`, `crates/daemon/tests/session_manager.rs`, `apps/desktop/src-tauri/src/{bridge.rs,commands.rs,daemon_client.rs,test_support.rs,worktrees.rs,commands/tests.rs}`, `apps/desktop/src/{services/types.ts,features/projects/ProjectTree.tsx,features/projects/ProjectTree.test.tsx,features/worktrees/WorktreeRows.tsx}`, and `ARCHITECTURE.md`.
Verification: per-task `cargo test -p desktop`, `cargo test -p daemon`, and `pnpm -C apps/desktop test`; final `cargo test --workspace`, `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo build --workspace`, `pnpm -C apps/desktop typecheck`, `pnpm -C apps/desktop build`, plus the specified manual macOS acceptance checklist.
Verified at: 25b30bdff7aacb54807131d93582f5e9e3d9ff8e
```

The trace uses the current working tree at the baseline commit above. User-owned edits predate this plan: tracked modifications under `apps/desktop/` and `crates/` (the in-progress terminal workspaces implementation), `ARCHITECTURE.md`, `AGENTS.md`, and `docs/agent-devkit/INDEX.md`; untracked `apps/desktop/src/features/terminal/{TerminalWorkspace.tsx,TerminalWorkspace.test.tsx}` and the open plan folder `docs/agent-devkit/changes/2026-11-02-terminal-workspaces-vietnamese/`. Implementation of this change is sequenced after that open plan, which owns project-level terminal workspaces.

## Tasks

### Task 1 — Git worktree queries in the Tauri backend

Files: `apps/desktop/src-tauri/src/worktrees.rs` (new), `apps/desktop/src-tauri/src/commands.rs`, `apps/desktop/src-tauri/src/lib.rs::handler`, `apps/desktop/src-tauri/src/commands/tests.rs`, `apps/desktop/src-tauri/src/wiring_tests.rs`

Interfaces: produces `worktrees::git(root, &[&str])` (the argv runner Tasks 2–3 share), `list_worktrees`, `status_counts`, `branches`, `base_refs`, and the serialized types `WorktreeInfo`, `WorktreeList`, `BranchInfo`, `BaseRef`; exposes Tauri commands `worktree_list` and `worktree_branches` consumed by Task 4's `worktreeService`.

Change: Implement the Git read model exactly as the design specifies: run `git -C <root>` with separate args and no shell; report `repo: false` unless the project's canonical root equals `git rev-parse --show-toplevel` and is a worktree root; parse `git worktree list --porcelain -z` into rows named by final path component with `branch` (null when detached), short `head` oid, `is_root` for the canonical match with the project root, and `missing` when Git lists a path absent from disk. A missing path has null `dirty`/`changed`; for present paths, parse `git status --porcelain=v1 -z --untracked-files=all` records into `dirty` and a changed-file count, treating a rename/copy record as one changed file; the NUL-delimited format preserves paths containing spaces or newlines. List local branches with the checkout path holding each and local plus remote-tracking refs for base selection. A missing `git` binary rejects `invalid_argument` with an install hint; a non-zero exit carries Git's stderr.

Verify: `cargo test -p desktop` — unit and command tests on real repositories created with `git init`/`git worktree add` in `TempDir`s: a root plus an externally added worktree list with names, branches, and detached oids; paths containing spaces/newlines parse correctly; a deleted worktree path stays listed with `missing: true`; a dirty tree counts modified, renamed, and untracked files; a non-Git directory and a repo subdirectory return `repo: false`; a PATH without `git` rejects `invalid_argument`; `wiring_tests` covers the `worktree_list`/`worktree_branches` JSON shapes the services send.

Progress: done
Result: `cargo test -p desktop` — 64 passed, including Git worktree parsing and Tauri command wiring.

### Task 2 — Guarded worktree create/remove and environment copy

Files: `apps/desktop/src-tauri/src/worktrees.rs`, `apps/desktop/src-tauri/src/commands.rs`, `apps/desktop/src-tauri/src/lib.rs::handler`, `apps/desktop/src-tauri/src/commands/tests.rs`, `apps/desktop/src-tauri/src/wiring_tests.rs`

Interfaces: consumes Task 1's `git` runner and list helpers plus `Bridge::session_list` for the removal guard; produces Tauri commands `worktree_create`, `worktree_remove`, `env_file_candidates`, `env_copy` and the types `EnvFileCandidate`, `EnvCopyReport` consumed by Task 4.

Change: Implement the mutations exactly as the design specifies: `worktree_create` validates the name from the final path component, accepts only an absent path or an empty directory, rejects a file/non-empty path or a path that equals/contains/nests inside an existing checkout, resolves and validates the nearest existing parent before creating missing directories, validates `mode: "new"` branches via `git check-ref-format` plus a local-existence check and `mode: "existing"` branches as local and not checked out, then runs `git worktree add`. `env_file_candidates` lists top-level regular `.env`/`.env.*` files, flagging exclusions (symlink, `.example`/`.sample`/`.template`/`.dist` suffix, non-regular type) with reasons. `env_copy` requires both paths to be listed checkouts, copies eligible files with exclusive-create writes (existing destinations report `skipped`, never overwritten), and returns copied/skipped/failed names. `worktree_remove` re-lists worktrees (target must be a non-root linked worktree), re-runs the dirty check, re-fetches `session.list` and counts sessions whose canonical `cwd` is the path or inside it — a daemon error refuses the removal — then runs `git worktree remove` without `--force`, preserving the local branch.

Verify: `cargo test -p desktop` — the create validation matrix rejects each bad name/path/branch case before mutating; new- and existing-branch creates produce listed worktrees; a checked-out branch is refused; env preview and copy apply the include/exclude/no-symlink/no-overwrite rules and a mid-copy failure reports failed names while the worktree remains listed; removal refuses a dirty worktree with its count, an attached live session, an attached exited session, a `fake_daemon` that fails `session.list` (fail closed), a root path, and a path that is not a linked worktree; a clean removal succeeds, `git branch --list` still shows its branch, and the removal confirmation says local/ignored files including copied `.env` are deleted.

Progress: done
Result: `cargo test -p desktop` — 73 passed; create validation, existing/new branch checkout, env preview/copy/no-overwrite/partial failure, Tauri wire shapes, and dirty/session/daemon-failure removal guards passed.

### Task 3 — Checkout-scoped session launch and grouping

Files: `apps/desktop/src-tauri/src/commands.rs::{CreateArgs,resolve_launch,session_create,record}`, `apps/desktop/src-tauri/src/commands/tests.rs`, `apps/desktop/src-tauri/src/wiring_tests.rs`, `apps/desktop/src/services/session.ts::CreateSessionInput`, `apps/desktop/src/stores/ui.ts`, `apps/desktop/src/stores/ui.test.tsx`, `apps/desktop/src/features/terminal/TerminalWorkspace.tsx`, `apps/desktop/src/App.tsx`, `apps/desktop/src/App.test.tsx`

Interfaces: extends `session_create` with optional `checkout` (only valid with `project_id`); produces `checkoutKey(projectId, checkoutPath)` and the cwd-to-checkout grouping helper consumed by Task 4's rows; consumes Task 1's `list_worktrees` for validation.

Change: Implement exactly what the design specifies: `resolve_launch` accepts `checkout` only with `project_id`, canonicalizes it, and requires it to be the project root or a Task 1 listed worktree, then uses it as the session `cwd` while `record` keeps storing `project_id` and the `SessionInfo.cwd`; `CreateSessionInput` gains `checkout?`; `stores/ui.ts` keys `workspaces` and the active workspace by `checkoutKey` (root checkout uses the project root, `__unassigned__` unchanged) and `removeProject` rehomes every workspace of the removed project under Unassigned; `TerminalWorkspace` and `App` pass the checkout path through to every `sessionService.create` call (default tab, new tab, splits, agent/custom launches).

Verify: `cargo test -p desktop` — `checkout` without `project_id`, a checkout outside the project, and a path that is not a listed worktree reject `invalid_argument` before the daemon (no spawn); a valid checkout launches with that `cwd` and metadata rows keep `project_id`; wiring tests cover the `checkout` wire name. `pnpm -C apps/desktop test` — checkout-keyed workspaces isolate tabs per checkout, `removeProject` rehomes all of a project's checkout workspaces, and sessions group under the checkout whose path contains their `cwd`.

Progress: done
Result: `cargo test -p desktop` — 74 passed, including checkout validation/wire shape and stored project_id/cwd; `pnpm -C apps/desktop test` — 116 passed, including checkout-key isolation, project removal rehoming, and cwd matching helper.

### Task 4 — Checkout rows, create dialog, and removal states

Files: `apps/desktop/src/services/types.ts`, `apps/desktop/src/services/worktrees.ts` (new), `apps/desktop/src/queryClient.ts`, `apps/desktop/src/features/worktrees/queries.ts` (new), `apps/desktop/src/features/worktrees/WorktreeRows.tsx` (new), `apps/desktop/src/features/worktrees/WorktreeCreateDialog.tsx` (new), `apps/desktop/src/features/projects/ProjectTree.tsx`, `apps/desktop/src/features/projects/ProjectTree.test.tsx`, `apps/desktop/src/features/worktrees/WorktreeCreateDialog.test.tsx` (new)

Interfaces: consumes Tasks 1–3 commands and the grouping helper; produces `worktreeService`, the `worktrees(projectId)`/`worktreeBranches(projectId)` query keys and hooks, and the sidebar checkout UX including the create dialog and remove confirmation.

Change: Implement the sidebar exactly as the design specifies: each Git project renders a checkouts block (root row flagged, then every listed worktree including external ones) with name, branch or `detached <short oid>`, clean/dirty badge with changed count, truncated path, session count, and a manual refresh; a missing linked-worktree path stays visible with a Missing badge and existing sessions, while new launches and normal removal are disabled; `worktrees(projectId)` invalidates on activation and after every worktree/env mutation while session changes recount via the existing sessions query; non-Git projects show no controls and keep flat session grouping. The New worktree dialog uses the installed Radix Dialog primitive and offers a name linked to the final path component, the generated-and-editable `../<project>-worktrees/<name>` path with a Choose… picker, new-vs-existing branch modes (checked-out branches disabled with their reason, base ref defaulting to the selected checkout's branch or commit), the default-off env checkbox with source select, secret-copy helper text and preview, and inline errors. Confirm create runs `worktree_create` then `env_copy` when checked, shows the copied/skipped/failed report with retry, selects the new checkout, and opens its first Shell tab — a failed launch keeps the worktree and shows the workspace error with Retry, never a phantom tab. The form and removal confirmation remain keyboard-operable, have visible labels/descriptions and announced validation errors, and return focus to their triggers. Remove explains that the directory including ignored local files such as copied `.env` will be deleted while the branch remains, and renders each refusal reason; success drops the empty workspace, activates the project-root checkout when needed, and refreshes.

Verify: `pnpm -C apps/desktop test` — rows render all fields and counts; missing paths stay visible and disable new launches/removal; refresh triggers and manual refresh; dialog defaults, linked name/path edits, disabled checked-out branches, base-ref default on a detached selection, env preview and report with retry; post-create activation and Shell launch including its failure path; removal refusal messages for dirty and attached sessions, ignored-file warning, active-checkout fallback, and success cleanup; keyboard operation, focus return, accessible labels and announced errors; non-Git projects hide controls.

Progress: done
Result: Fresh `pnpm -C apps/desktop test` — 173 passed and `pnpm -C apps/desktop typecheck` — clean; coverage includes create from Editor switching to Terminal, environment copy/retry, checkout launch, cross-project attached-session count and removal guard, pending-removal launch disablement, focus return, and cleanup.

### Task 5 — Live shell CWD tracking and removal safety

Files: `crates/daemon/Cargo.toml`, `crates/daemon/src/pty.rs::PtyProcess`, `crates/daemon/src/session.rs::{SessionManager,watch_exit}`, `crates/daemon/tests/{session_manager.rs,session_ipc.rs}`, `apps/desktop/src-tauri/src/{bridge.rs,commands.rs,daemon_client.rs,test_support.rs,worktrees.rs,commands/tests.rs}`, `apps/desktop/src/{services/types.ts,features/projects/ProjectTree.tsx,features/projects/ProjectTree.test.tsx,features/worktrees/WorktreeRows.tsx}`

Interfaces: consumes the existing PTY child PID, `SessionInfo.cwd`, daemon list/get/events, the bridge launch/remove operation gate, and checkout grouping/removal UX; changes the meaning of `SessionInfo.cwd` for live sessions without changing the protocol shape.

Change: Track the PTY child process's actual current directory on supported macOS and Linux systems. Refresh live sessions periodically and synchronously before `SessionManager::list`/`get` returns; emit `session.updated` only when the directory changes. Preserve the last known directory after exit. If a live directory cannot be read or encoded as UTF-8, publish an empty `cwd`; the sidebar keeps that session under the project's other sessions and disables linked-worktree removal globally. Retain `HelloOk.daemon_version` in the desktop client and mark the first live-CWD daemon as 0.1.1; before removal, refuse daemons older than 0.1.1 without restarting them, and expose the capability through `daemon_status`. Under the backend removal gate, require every daemon session across every project to be exited or failed, then refuse if any session has an empty, relative, or unresolvable `cwd`; otherwise re-check known directories and refuse when one is the target checkout or a descendant. Keep exited attached-session checks and fail closed on daemon errors. The daemon takes the shared `GOKO_HOME` `flock` for each `SessionCreate`; Tauri removal holds the exclusive lock across its fresh `session.list` check and Git removal, in addition to the in-process bridge gate. Keep blocked reasons visible on focus/hover. Do not change the handshake shape, protocol version, `SessionMeta`, or `SessionInfo` fields.

Verify: `cargo test -p daemon` — a shell launched in one directory, then changed into a temp directory, updates `session.list`, `session.get`, and a `session.updated` event; its exited session retains the last known CWD; and a direct authenticated `SessionCreate` waits while an exclusive worktree-operation lock is held. `cargo test -p desktop` — a shell that `cd`s into a linked worktree blocks removal, a live session in another project blocks removal globally, exited attached sessions remain blocked, empty/relative/unresolvable CWDs fail closed, a daemon reporting 0.1.0 is refused without deleting the worktree, and the lock serializes independent Bridge instances. `pnpm -C apps/desktop test` — unknown CWD and live-session guards apply across projects; the old-daemon reason is shown before confirmation, and blocked buttons expose reasons by focus and hover.

Progress: done
Result: The regression `worktree_removal_fails_closed_when_a_session_cwd_is_relative_or_unresolvable` failed before the fix for a missing absolute directory under an existing parent, then passed after removal switched to exact canonicalization and a directory check. A direct authenticated `SessionCreate` against a real child daemon waits while an exclusive worktree-operation lock is held, then succeeds after release. Fresh `cargo test --workspace` — 204 passed; `pnpm -C apps/desktop test` — 173 passed; `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo build --workspace`, `pnpm -C apps/desktop typecheck`, and `git diff --check HEAD` — clean.

### Task 6 — Architecture update and acceptance

Files: `ARCHITECTURE.md`

Interfaces: records the shipped worktree model for the next contributor; no code.

Change: Update the architecture document with the Git argv invocation boundary, the `worktrees.rs` commands and their wire shapes, the checkout model under a project, the env copy policy, live daemon CWD refresh and event flow, the process-shared launch/removal gate, the fail-closed removal guard for global live sessions, old-daemon compatibility through `daemon_status`, attached and unknown CWDs, accessible blocked reasons, the ignored-file warning, missing-worktree behavior, checkout-keyed workspaces, session↔checkout association by live `cwd`, and what remains out (diff viewer). Record the manual macOS checklist results per item like s1-13.

Verify: `cargo test --workspace`, `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo build --workspace`, `pnpm -C apps/desktop test`, `pnpm -C apps/desktop typecheck`, `pnpm -C apps/desktop build`; then the manual checklist — register a Git project, create worktrees on new and existing branches, see an external worktree appear, copy env files, run agents in two checkouts side by side, confirm a shell changing directory moves its session group, confirm dirty, global-live-session, exited-attached-session, unknown-CWD, old-daemon, and daemon-down removal refusals, and a clean removal preserving the branch while warning about ignored local files, verify a missing worktree stays visible until externally repaired/pruned, and remove the project without deleting directories.

Progress: in progress
Result: Fresh full verification passed: `cargo test --workspace` — 204 passed; `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo build --workspace`, `pnpm -C apps/desktop test` — 173 passed, `pnpm -C apps/desktop typecheck`, `pnpm -C apps/desktop build`, and `git diff --check HEAD` — clean. A Markdown link pass checked 40 files and found all local targets. The architecture update is present. Manual macOS acceptance was not run: an isolated app started with `GOKO_HOME=/tmp/goko-acceptance.gt7de8`, but `cua_repl` failed with `Sky Computer Use native pipe startup failed` before exposing a window. Worktree creation/removal and environment-copy acceptance therefore remain unverified.

## Files inspected, no change

- `apps/desktop/src-tauri/src/{bridge.rs,error.rs}` — the daemon call path and the existing error codes the new commands reuse.
- `apps/desktop/src-tauri/src/store.rs` — `sessions` rows already carry `project_id` and `cwd`; no schema work.
- `apps/desktop/src-tauri/src/agents.rs` — the PATH-resolution pattern the `git` lookup follows.
- `apps/desktop/src-tauri/src/test_support.rs` — `TempDir`, `Home`, and `fake_daemon` fixtures the new tests reuse.
- `apps/desktop/src-tauri/capabilities/default.json` — `dialog:allow-open` already covers the path chooser; no new permission.
- `apps/desktop/src/services/{call.ts,errors.ts,sync.ts,projects.ts}` and `apps/desktop/src/features/sessions/queries.ts` — the invoke helper, error union (codes reused unchanged), event→cache sync, and service/query patterns.
- `apps/desktop/src/features/terminal/{TerminalView.tsx,TerminalController.ts}` — the unchanged per-pane rendering path.
- `crates/protocol/src/message.rs` — the `SessionInfo` JSON shape stays unchanged; daemon CWD tracking is implemented in Task 5.
- `docs/agent-devkit/changes/2026-11-02-terminal-workspaces-vietnamese/{design.md,tasks.md}` — the open dependency; read for sequencing, left unmodified.
- `docs/llm/` — absent; no verified wiki coverage or delta applies.

## Approval Gate

Status: approved

## Decision Log

### D1 — Track live shell CWD

Question: How should removal safety and checkout grouping handle a shell started in the project root that later changes into a worktree?
Decision: Track the live process CWD in the daemon; session grouping may move as the shell changes directory.
Impact: Adds daemon CWD lookup, periodic updates through existing `SessionInfo.cwd` and session events, fail-closed removal for unknown directories, and coverage for updated grouping/removal behavior. The protocol shape remains unchanged.
Confirmed by user: 2026-11-02

### D2 — Block removal while any Goko session is live

Question: How should removal handle a shell whose live CWD changes between the `session.list` snapshot and Git removal?
Decision: Refuse worktree removal while any daemon session is running, across all projects and checkouts; require all sessions to be exited or failed first.
Impact: Adds a global live-session check to backend removal and a sidebar disable reason; a shared/exclusive `flock` under `GOKO_HOME` also serializes launches and removals across app processes. The daemon is not asked to stop or delete sessions.
Confirmed by user: 2026-11-02
