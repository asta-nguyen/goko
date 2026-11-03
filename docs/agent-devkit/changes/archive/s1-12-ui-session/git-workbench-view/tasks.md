# Workspace editor and Git review tasks

## Design

[Workspace editor and Git review](design.md)

## Global Constraints

- Dependency: implement after the open [Git worktree management UX](../2026-11-02-worktree-ux/design.md) and [terminal workspace change](../2026-11-02-terminal-workspaces-vietnamese/design.md) land. This view consumes their checkout identity and workspace lifecycle.
- All Git subprocesses use separate argv values and the existing Tauri-side Git boundary; no shell command string is constructed. Every request validates the project and confirms that the checkout is its root or a currently listed linked worktree. File/tree paths are relative to that checkout and reject absolute paths, traversal, and canonical targets outside it.
- Data is keyed by project ID and canonical checkout path so two worktrees never share status, diff, history, tree, or code preview. Stash list identity is project/repository-wide because `refs/stash` is shared; stash details use the immutable selected stash commit ID. A failed refresh preserves the last successful result and shows a non-blocking stale-data message; an initial failure shows a retry action.
- Changes status and the selected diff refresh every three seconds while `Changes` is visible; the stash list refreshes every three seconds while `Stashes` is visible. Both sections also have a manual refresh action. History, Files, and file/stash details load on selection or entry and can be manually refreshed. Diff and text-preview responses are capped at 1 MiB, with an explicit truncated/too-large state above the cap.
- Read-only behavior: code and Git state can be inspected. This view does not edit/save source files, stage, unstage, discard, commit, create/apply/pop/drop stashes, checkout, fetch, or otherwise mutate Git state.
- Daemon/protocol changes, database/schema changes, new dependencies, and new Tauri capabilities.
- Existing semantic colour tokens and Lucide icons remain in use; no editor dependency is added for the read-only code view.

## Impact map

Entry: project/worktree-row Editor action and active workspace Editor mode, implemented at `ProjectTree` / `WorktreeRows`.

Flow: `ProjectTree` / `WorktreeRows` Editor action → active checkout workspace + Editor mode → `App` / `EditorWorkspace` → React Query hooks / `gitViewService` → Tauri commands → `git_view.rs` → local Git and bounded checkout file reads.

State changes: no persisted state or repository writes; Zustand stores the active main-area mode and React Query stores checkout-keyed read results, with repository-wide identity for shared stashes.

External effects: local Git subprocesses and bounded reads of files inside the selected checkout; no network requests or Git mutations.

Change candidates: `apps/desktop/src-tauri/src/git_view.rs` (new), `apps/desktop/src-tauri/src/commands.rs`, `apps/desktop/src-tauri/src/lib.rs`, `apps/desktop/src-tauri/src/commands/tests.rs`, `apps/desktop/src-tauri/src/wiring_tests.rs`; `apps/desktop/src/services/types.ts`, `apps/desktop/src/services/git-view.ts` (new), `apps/desktop/src/queryClient.ts`, `apps/desktop/src/features/editor/queries.ts` (new), `apps/desktop/src/stores/ui.ts`, `apps/desktop/src/App.tsx`, `apps/desktop/src/features/projects/ProjectTree.tsx`, `apps/desktop/src/features/worktrees/WorktreeRows.tsx`, `apps/desktop/src/features/editor/*` (new) and focused tests; `ARCHITECTURE.md`; `docs/agent-devkit/roadmap.md`.

Verification: temporary Git repositories cover status/diff/history/tree/preview/stash parsing and boundary cases; Tauri wiring checks cover command arguments/shapes; frontend tests cover workspace scoping, Editor navigation, read-only code review, shared stashes, refresh, and accessible states; existing Rust/desktop checks plus manual light/dark and keyboard review.

Verified at: `25b30bdff7aacb54807131d93582f5e9e3d9ff8e`

## Tasks

Baseline (recorded before any source edit): `cargo test -p desktop` — 75 passed; `pnpm -C apps/desktop test` — 135 passed; `pnpm -C apps/desktop typecheck` — passed.

### Task 1 — Checkout-bound Git and stash read commands

Files: `apps/desktop/src-tauri/src/git_view.rs` (new); `apps/desktop/src-tauri/src/commands.rs::project_root`, `apps/desktop/src-tauri/src/commands.rs::project_checkout`; `apps/desktop/src-tauri/src/lib.rs::handler`; `apps/desktop/src-tauri/src/commands/tests.rs`; `apps/desktop/src-tauri/src/wiring_tests.rs`

Interfaces: consumes existing registered-project lookup, canonical checkout validation, worktree enumeration, and separate-argv Git execution; produces read-only Tauri commands `git_view_changes`, `git_view_diff`, `git_view_history`, `git_view_tree`, `git_view_file`, `git_view_stashes`, and `git_view_stash_diff`, plus serializable status, diff, commit, tree, file, and stash responses for Task 2.

Change: Validate `projectId` and `checkoutPath` through the existing command helpers on every request. Parse NUL-delimited porcelain status into conflict, staged, unstaged, and untracked groups, preserving partially staged paths, rename/deletion state, and unusual UTF-8 filenames. Return the selected staged/unstaged/untracked/conflict diff, selected commit patch, or selected stash patch (including stashed untracked files) without invoking a shell or mutating Git. Return locally available branch/remote/tag commit nodes and parent/ref links, excluding stash refs, in pages of 100 without fetching. Return one requested tree directory at a time, excluding `.git` and ignored paths, and bounded text-file previews. Return the stash stack from repository-shared `refs/stash` with commit ID and optional recorded branch hint; verify a requested stash ID remains in the current stack before returning its patch. Cap diff and text-preview output at 1 MiB. Reject absolute/traversal paths, invalid commit/stash IDs, missing/outside checkouts, and symlink-resolved file reads outside the checkout. Report non-Git projects, missing Git executable, command errors, binary, non-UTF-8, oversized, or unreadable content as structured error/fallback states. Do not expose file-write or Git-write commands.

Verify: `cargo test -p desktop` with temporary repositories proves staged/unstaged/partially staged/untracked/conflict/rename/delete status, per-layer diffs, odd filenames, commit parent/ref pages excluding stash refs, empty history, tree filtering/lazy directory results, bounded code and binary fallbacks, symlink escape and traversal rejection, missing/outside worktree rejection, non-Git project response, missing-Git and Git-command errors, and stash list/patch behavior from two linked worktrees with optional branch hints. Snapshot the index, worktree, and `refs/stash` before each read path and confirm they are unchanged. Tauri wiring tests assert command names and camelCase request/response JSON.

Progress: done
Result: `cargo test -p desktop` — 91 passed, including 13 `git_view` tests (status groups, per-layer diffs, >1 MiB truncation, history pages/refs excluding stash refs, lazy tree, bounded/binary/non-UTF-8/missing previews, symlink escapes, shared stash list/patch across two linked worktrees, and index/worktree/`refs/stash` snapshots unchanged by reads) and the `git_view_*` IPC wire-shape test (camelCase args, JSON keys, `not_found`/`invalid_argument` rejections). `cargo fmt --check` and `cargo clippy --workspace -- -D warnings` — clean.

### Task 2 — Checkout-keyed frontend service and Editor mode state

Files: `apps/desktop/src/services/types.ts`; `apps/desktop/src/services/git-view.ts` (new); `apps/desktop/src/queryClient.ts`; `apps/desktop/src/features/editor/queries.ts` (new); `apps/desktop/src/stores/ui.ts`

Interfaces: consumes Task 1's command names and wire types; produces typed `gitViewService`, checkout-keyed query keys/hooks for Files, Changes, History, and file content, repository-keyed stash queries, and an in-memory `terminal | editor` main-area state consumed by Task 3.

Change: Add typed service calls for status, file/layer diff, paged history/commit patch, lazy tree, code preview, stash list, and stash patch. Include `projectId`, canonical `checkoutPath`, and selected path/layer/commit/directory in checkout-scoped query keys. Key stash list by the registered project/repository because the stack is shared across its worktrees; key stash details by the selected commit ID. Override the global infinite stale time so entering a section fetches fresh data while cached data remains visible during refresh. Enable queries only for the active Editor workspace. Poll Changes status/diff every three seconds only while Changes is visible and poll the repository stash list every three seconds only while Stashes is visible. Load History/tree on entry and file/stash details on selection; manual refresh invalidates only the relevant query. Switching checkouts changes query identity immediately so previous-checkout results cannot render as current.

Verify: `pnpm -C apps/desktop test` confirms different checkout paths produce distinct Git queries, repository stash queries remain shared across worktrees, disabled Editor views do not poll, Changes and Stashes poll only while visible, and returning to a checkout reuses only its checkout-scoped cache. `pnpm -C apps/desktop typecheck` passes with strict TypeScript.

Progress: done
Result: `pnpm -C apps/desktop test` — 149 passed across 17 files, including `services/git-view.test.ts` (command names, payloads, `BridgeError` mapping) and `features/editor/queries.test.tsx` (checkout-key identity/cache reuse, checkout-scoped diff/history/tree/file keys, no reads while the view is disabled, 3 s polls only while visible, repository-wide stash list shared across checkouts, stash-diff keyed by immutable commit ID, failed refresh keeping prior data with `isError`, 100-commit `fetchNextPage` paging). `pnpm -C apps/desktop typecheck` — clean.

### Task 3 — Dedicated Editor workspace UI

Files: `apps/desktop/src/App.tsx`; `apps/desktop/src/features/projects/ProjectTree.tsx`; `apps/desktop/src/features/worktrees/WorktreeRows.tsx`; `apps/desktop/src/features/editor/EditorWorkspace.tsx` (new); `apps/desktop/src/features/editor/EditorFiles.tsx` (new); `apps/desktop/src/features/editor/EditorChanges.tsx` (new); `apps/desktop/src/features/editor/EditorHistory.tsx` (new); `apps/desktop/src/features/editor/EditorStashes.tsx` (new); focused tests beside the new components and in `apps/desktop/src/features/projects/ProjectTree.test.tsx`

Interfaces: consumes Task 2's active view state, service hooks, and Git types; produces accessible checkout-row Editor actions and the full-width Editor workbench rendered in the existing main area.

Change: Add a discoverable, labeled Editor action to each Git project root and linked worktree row; activating it selects that exact workspace and enters Editor mode. Add a workspace-level `Terminal` / `Editor` switch separate from session tabs. Keep the project/worktree sidebar visible; replace only the main content with a header showing project, checkout, and branch/HEAD. Use internal `Files`, `Changes`, `History`, and `Stashes` navigation, defaulting to Changes. Each section has a left tree/list and a right read-only, line-numbered code/diff pane. Changes groups staged, unstaged, untracked, and conflict files with counts and the correct comparison layer. History shows branch/remote/tag graph lanes, commit metadata, paging, and selected commit details. Stashes clearly labels its repository-wide shared scope and shows the selected stash's branch hint when available and patch. Selecting another checkout while Editor is active keeps Editor active, clears checkout-scoped file/commit/directory selection, preserves stash selection within the same project, and updates the view; selecting another project selects its newest stash. Terminal returns with existing tabs/splits and running sessions unchanged. Include clean, loading, empty-history/stash, unsupported-project, missing-checkout, binary/large-file, stale-data, and retry states; preserve keyboard focus, accessible names, selection semantics, and semantic tokens in both themes. No code editing or Git write actions appear in this view.

Verify: `pnpm -C apps/desktop test` confirms each Editor row action opens the corresponding project/worktree, selecting another checkout while Editor is open switches data without showing stale results, Terminal restores the same tabs/splits and does not stop sessions, Files/Changes/History/Stashes selection works, stash UI identifies repository-wide scope, and empty/error/fallback states are accessible. Manually inspect the layout at normal desktop width in light/dark modes and operate navigation, file groups, graph, stash list, and refresh with keyboard only.

Progress: in progress
Result: Fresh `pnpm -C apps/desktop test` — 173 passed across 18 files; `pnpm -C apps/desktop typecheck` — clean. Tests cover stale worktree/tree/file refresh feedback. Manual normal-width layout inspection, light/dark review, and keyboard-only operation remain unrun because the UI automation host returned `Sky Computer Use native pipe startup failed`. `TerminalWorkspace.tsx` hosts the `Terminal | Editor` switch; no other plan-file deviations were found.

### Task 4 — Project docs and end-to-end verification

Files: `ARCHITECTURE.md`; `docs/agent-devkit/roadmap.md`; scoped `docs/llm/` requirements created/updated through `document-wiki`

Interfaces: consumes the approved Git command, query, and Editor behavior from Tasks 1–3; documents the S3 workspace Editor and Git safety boundary for future contributors.

Change: Update the S3 roadmap row to include checkout-scoped Editor Files, Changes, History graph, and repository-shared Stashes inspection. Document the Tauri read boundary, response limits, checkout-context navigation, refresh behavior, read-only scope, and stash sharing in `ARCHITECTURE.md`. Because `docs/llm/` has no current index or Git requirements page, run `document-wiki` after implementation checks to add scoped Editor/Git inspection requirements coverage and its index links. Record the manual checklist outcomes.

Verify: Confirm all internal Markdown links resolve; run `cargo test --workspace`, `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo build --workspace`, `pnpm -C apps/desktop test`, `pnpm -C apps/desktop typecheck`, and `pnpm -C apps/desktop build`; complete the manual checkout isolation, staged/unstaged/untracked diff, history graph, file code view, shared stash list/patch, refresh/error, keyboard, and theme checklist. No checks are run during planning.

Progress: in progress
Result: `ARCHITECTURE.md` documents the Editor sections, the `git_view_*` read boundary (per-call project/checkout validation, separate argv, relative-path + symlink containment, bounded 1 MiB diff capture and file previews, explicit binary/non-UTF-8/oversized/missing kinds, read-only scope), checkout-scoped vs repository-shared query keys, and the 3 s polling/refresh/stale-data behavior; `docs/agent-devkit/roadmap.md` S3 names the checkout-scoped Editor Files/Changes, History graph, and repository-shared Stashes. Fresh checks: `cargo test --workspace` — 204 passed; `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo build --workspace`, `pnpm -C apps/desktop typecheck`, `pnpm -C apps/desktop build`, and `git diff --check HEAD` — clean; `pnpm -C apps/desktop test` — 173 passed. A Markdown link pass checked 40 files and found all local targets. `docs/llm/` is absent, so the repository workflow skips wiki setup. Manual checkout isolation, staged/unstaged/untracked diffs, history graph, file code view, shared stash patch, refresh/error, keyboard navigation, and light/dark review remain unrun because `cua_repl` failed with `Sky Computer Use native pipe startup failed`.

## Files inspected, no change

- `apps/desktop/src/features/terminal/TerminalWorkspace.tsx` — changed only to host the `Terminal | Editor` switch; its session-tab and split behavior is preserved (note: planned as "no change", the switch landed here).
- `apps/desktop/src/services/worktrees.ts` and `apps/desktop/src/features/worktrees/queries.ts` — existing typed service and React Query patterns.
- `apps/desktop/src-tauri/src/worktrees.rs` — changed only in visibility (`run` and the git-missing error surfaced to `git_view.rs`); its safe Git subprocess and worktree-read model is unchanged.
- `crates/protocol/` and `crates/daemon/` — no changes planned; the daemon remains responsible only for terminal sessions.
- `apps/desktop/package.json` — no code editor component/dependency is installed; the read-only code view uses existing React and semantic UI.
- `docs/llm/` — absent; no existing Git requirement page is available to update.

## Approval Gate

Status: approved

## Decision Log

- 2026-11-02: User revised the pending Git-only view into a workspace-level Editor view. Accepted recommendation: read-only code and diff review; Stashes list and patch inspection only; no file writes or Git mutations. Stashes are labeled repository-wide because `refs/stash` is shared across linked worktrees. Approval: approved by user; implementation authorized.
