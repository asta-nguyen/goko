# Workspace editor and Git review

## Goal

Give each Git project/worktree workspace a dedicated Editor view for browsing its files and reviewing its changes, history, and repository stashes while preserving its terminal sessions.

## Scope

In:

- A clearly labeled `Editor` action on each Git project root checkout and linked worktree row. Activating it selects that checkout's workspace and opens Editor; non-Git projects do not show this action.
- A workspace-level `Terminal` / `Editor` mode in the main work area. Editor replaces terminal content while the project/worktree sidebar remains visible. Returning to Terminal restores the existing tabs and splits; running sessions continue unchanged.
- Four Editor sections: `Files`, `Changes` (default), `History`, and `Stashes`. The header always identifies the active project, checkout, and branch or detached HEAD.
- `Files` shows a lazy tree of tracked and non-ignored untracked files. `.git` and ignored files are excluded. Selecting a text file opens a bounded, read-only, line-numbered code view.
- `Changes` groups conflicts, staged, unstaged, and untracked files with counts. Partially staged files may appear in both staged and unstaged groups. Selecting a file opens its staged, unstaged, untracked, or conflict diff. Rename and deletion state is retained. Binary and oversized files get a bounded preview fallback.
- `History` shows a navigable graph of locally available branches, remote-tracking branches, and tags (100 commits per page), highlights the active checkout's branch or detached HEAD, and loads selected commit details, changed files, and patch. It excludes stash refs and does not fetch or change refs.
- `Stashes` lists the repository's stash entries and shows a selected stash's changed files and bounded diff, including untracked files when the stash contains them. It labels the list as shared across the project's worktrees and shows each entry's recorded branch hint when available. Git stores the stash stack in `refs/stash`, a ref shared across linked worktrees ([Git worktree documentation](https://git-scm.com/docs/git-worktree), [Git stash documentation](https://git-scm.com/docs/git-stash)).
- Changes status and the selected diff refresh every three seconds while `Changes` is visible; the stash list refreshes every three seconds while `Stashes` is visible. Both sections also have a manual refresh action. History, Files, and file/stash details load on selection or entry and can be manually refreshed. Diff and text-preview responses are capped at 1 MiB, with an explicit truncated/too-large state above the cap.
- Accessible keyboard navigation, visible focus, selected states, loading, clean, empty, unsupported-project, missing-checkout, Git-error, and refresh-error states. Existing semantic colour tokens and Lucide icons remain in use; no editor dependency is added for the read-only code view.
- Read-only behavior: code and Git state can be inspected. This view does not edit/save source files, stage, unstage, discard, commit, create/apply/pop/drop stashes, checkout, fetch, or otherwise mutate Git state.

Out:

- Editing or saving files, external editor integration, or unsaved-buffer/conflict resolution.
- Git write operations, stash management, branch/worktree management changes, remote fetch/pull/push, merges, rebases, and cross-worktree comparison.
- Daemon/protocol changes, database/schema changes, new dependencies, and new Tauri capabilities.

Dependency: implement after the open [Git worktree management UX](../2026-11-02-worktree-ux/design.md) and [terminal workspace change](../2026-11-02-terminal-workspaces-vietnamese/design.md) land. This view consumes their checkout identity and workspace lifecycle.

## Behavior

- The `Editor` action on a checkout row activates that checkout's workspace and switches the main area to Editor. While Editor is active, selecting another checkout from the sidebar keeps Editor active and loads that checkout's data. Selecting `Terminal` returns to that workspace's existing terminal tabs and splits. Selecting a non-Git project while Editor is active shows a clear unsupported-project state with a direct return to Terminal.
- The main-area mode control is separate from the terminal session tab bar. Editor uses a sub-navigation for `Files`, `Changes`, `History`, and `Stashes`; it fills the main work area instead of appearing as a narrow terminal side panel.
- Each Editor section uses a left navigation/list pane and a right read-only code/diff pane. `Files` uses the tree on the left. `Changes` opens by default with collapsible status groups and counts on the left, and the selected diff on the right. Diff headers identify the comparison (`HEAD → index`, `index → working tree`, a new untracked file, or conflict). A clean checkout shows a concise clean state.
- `History` uses graph lanes with commit subject, author/time, and local branch/remote/tag labels. Selecting a commit shows its metadata, changed files, and patch in the right pane. Graph data comes from refs already present locally; no network access is performed.
- `Stashes` explicitly says the stack is shared across worktrees. Selecting an entry shows its branch hint when available, changed files, and patch. The list is keyed to the registered repository; the stash diff is keyed by the selected stash commit ID so switching worktrees does not imply a different stash stack.
- When the active checkout changes, clear checkout-specific file, commit, and directory selection; default to the first available change, HEAD, or repository root as appropriate. Preserve the selected stash when switching worktrees in the same project because its stash list is shared; select the newest stash when changing projects.
- Data is keyed by project ID and canonical checkout path so two worktrees never share status, diff, history, tree, or code preview. Stash list identity is project/repository-wide because `refs/stash` is shared; stash details use the immutable selected stash commit ID. A failed refresh preserves the last successful result and shows a non-blocking stale-data message; an initial failure shows a retry action.
- All Git subprocesses use separate argv values and the existing Tauri-side Git boundary; no shell command string is constructed. Every request validates the project and confirms that the checkout is its root or a currently listed linked worktree. File/tree paths are relative to that checkout and reject absolute paths, traversal, and canonical targets outside it.

## Edge cases

- Non-Git projects have no Git data and show the unsupported-project state if Editor remains active after workspace selection changes.
- Missing worktree directories remain visible in the sidebar, but Editor queries are disabled and the main view explains that the checkout must be restored or refreshed.
- A missing Git executable, repository command failure, or Tauri command failure is shown as a retryable Git error; the app and terminal sessions remain usable.
- A repository with no commits shows an empty History state while Files and Changes remain available. An empty stash stack shows a concise empty state.
- A stash created with a custom message or from a detached HEAD may have no usable branch hint; show the stash message/date and omit the branch hint in that case.
- A file staged and modified again is represented in both staged and unstaged groups, with a separate diff for each layer.
- Binary, non-UTF-8, oversized, unreadable, symlink-escaping, and deleted files do not crash the view or expose content outside the selected checkout; the UI explains the available fallback.
- Filenames containing spaces, tabs, newlines, or valid non-ASCII Unicode are preserved in status, tree, and diff results.
- Large repositories avoid loading complete file contents or unbounded history/tree results at once; history pages contain at most 100 commits, trees load one directory at a time, and file/diff responses have a 1 MiB cap.
- A checkout switch during a pending request cannot display the old checkout's response under the new workspace; query identity includes the checkout path and stale responses are ignored.
- A stash created from any linked worktree appears in the repository-wide Stashes list. A stash removed externally while selected shows a retryable missing-entry state.

## Interfaces

- New Tauri read commands return Git status groups, a selected file/layer diff, paged commit graph/patch data, lazy tree entries, bounded file previews, a repository-wide stash list, and a selected stash patch. Inputs include `projectId` and `checkoutPath`; file/tree paths are relative, and commit/stash selection uses a validated object ID.
- The Tauri implementation reuses the existing separate-argv Git execution and project/worktree validation pattern. It does not change `crates/protocol` or `crates/daemon`.
- The frontend adds typed `gitViewService` calls and React Query hooks. Status/diff/history/tree/code queries include project ID and canonical checkout path. Stash-list keys use the registered project/repository identity; stash-diff keys use the stash commit ID. Changes and Stashes polling only runs while that section is visible.
- UI state adds the active workspace view (`terminal` or `editor`) without persisting it or changing the existing checkout-keyed terminal workspace model.
- Sidebar checkout rows expose the Editor entry action; `App`/workspace rendering switches the main area between terminal and Editor while preserving terminal component state.
- The implementation updates the S3 deliverable in `docs/agent-devkit/roadmap.md`, documents the commands and UI in `ARCHITECTURE.md`, and adds scoped Git inspection requirements coverage through `document-wiki`, because `docs/llm/` currently has no verified index or requirement page.

## Verification

- Rust tests use temporary real Git repositories to verify status parsing for staged, unstaged, partially staged, untracked, conflict, rename, delete, and unusual filenames; per-layer diffs; graph parent/ref data and empty history; tree filtering and lazy paths; bounded code/diff previews; stash listing and patches; shared stash visibility from two linked worktrees; and rejection of invalid checkout/file paths and symlink escapes.
- Tauri wiring tests verify command names, camelCase arguments, and JSON shapes. Read operations leave the index, worktree, and stash refs unchanged.
- Frontend tests verify the checkout-row Editor action opens the exact workspace, Editor follows workspace selection, Terminal returns with sessions/tabs intact, Changes grouping and diff selection work, History selection opens commit details, Files navigation opens code and links changed files to Changes, Stashes show repository scope and selected patches, polling stops when hidden, and loading/error/empty/unsupported states are accessible.
- Run the existing Rust and desktop test/typecheck/build commands after implementation, then manually inspect the Editor in light and dark themes with keyboard navigation and a repository containing staged, unstaged, untracked, stashed, binary, and large files. No checks are run during this design step.

## Related context

None. `docs/llm/` is absent; there is no verified wiki coverage.

## Previous work

- [Git worktree management UX](../2026-11-02-worktree-ux/design.md) and [tasks](../2026-11-02-worktree-ux/tasks.md) provide the checkout model, safe Git subprocess boundary, status summary, and checkout-scoped sessions that this view consumes.
- [Project terminal workspaces](../2026-11-02-terminal-workspaces-vietnamese/design.md) and [tasks](../2026-11-02-terminal-workspaces-vietnamese/tasks.md) define the current checkout-keyed workspaces, terminal tabs, and split-session lifecycle that Editor preserves.
