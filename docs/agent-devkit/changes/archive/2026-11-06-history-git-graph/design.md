# Editor History: Git Graph style table

## Goal

The Editor's History tab is a left list of two-line rows with a single-colour graph, cramped refs, an absolute
date, and a subject-only detail. Rebuild it after the VS Code extension *Git Graph* (mhutchie): a one-line-per-commit
table with a coloured lane graph, columns the user can drag to resize (remembered across app runs), a branch filter,
an "Uncommitted changes" row, keyboard and right-click actions, and a detail panel with the full message, parents,
and per-file added/removed counts.

## Scope

In:
- Layout: commit table on top (columns Graph, Description, Date, Author, Commit), detail panel below, a draggable
  horizontal divider between them. The old left/right `SplitPane` is no longer used by History.
- Graph: one colour per branch line from an 8-colour palette, curved merge/fork edges, a distinct HEAD node, a distinct
  merge-commit node. The graph stays visible while the filter is active.
- Rows: short hash column, relative date (full date in the tooltip), ref chips after the subject in their lane colour.
- Resize: drag Graph, Date, Author, and Commit column edges and the table/detail divider; arrow keys and double-click
  reset on every handle; widths and detail height persisted in `localStorage`.
- Navigation: ↑/↓/Home/End move focus between rows, the next page loads when the end of the table scrolls into view,
  a "Jump to HEAD" button, filter matches are highlighted and `Enter`/`Shift+Enter` jump to the next/previous match,
  right-click a row for Copy hash and Copy subject.
- Branch filter: a dropdown with "All branches", each local branch, and each remote-tracking branch.
- Uncommitted changes row on top of HEAD that lists the changed entries and opens the Changes tab.
- Detail panel: short hash with copy, subject, author with email, absolute date, committer when it differs from the
  author, parents, ref chips, full message body, changed files with `+added −removed`, patch.
- Rust: `git_view_history` accepts a branch; new `git_view_branches`; a commit diff also returns the message body,
  author email, committer, and per-file added/removed counts.

Out:
- Git writes from History (checkout, merge, reset, cherry-pick, tag); `git_view.rs` stays read-only.
- Selecting several branches, hiding/showing columns, author avatars, persisting the chosen branch.
- Virtualising the table (ceiling: noticeable scroll lag past roughly 2,000 loaded commits).
- Searching commits that are not loaded yet; the filter covers the loaded pages.
- Choosing a single file inside the commit patch; the whole commit patch stays one scroll area.
- Stash entries in the graph (Stashes keeps its own tab).
- Changes owned by the open change `2026-11-06-editor-commit-acp` (staging, commit box, ACP). Both changes edit
  `git_view.rs`, `commands.rs`, `types.ts`, and `EditorWorkspace.test.tsx`; this one builds on the working tree as it
  is and does not touch that change's functions or tests.

## Behavior

Table
- Rows are 28 px high and one line: Graph | Description | Date | Author | Commit. Description holds the subject first,
  then the ref chips (HEAD, branches, `remotes/…`, tags); the subject truncates before the chips do.
- Defaults: Date 120, Author 128, Commit 80 px; Graph is the natural lane width (14 px per lane plus padding) within
  48–320 px; Description takes the remaining width with a 200 px floor. A user-set column is 56–360 px. When the
  columns plus the floor exceed the pane, the table scrolls horizontally.
- Date shows a relative time ("just now", "5 minutes ago", "3 days ago") through `Intl.RelativeTimeFormat`, and the
  medium date once the commit is older than 30 days; the `title` is the existing absolute `formatTime`. It is computed
  at render and does not tick.
- Selecting a row (click, `Enter`, `Space`) shows it in the detail panel; the first load still selects HEAD, else the
  newest commit.

Graph
- A branch line keeps one palette colour from where it starts until it ends. A new lane takes the next unused colour in
  rotation. An edge is drawn as a vertical run plus a rounded bend into the parent's lane, never a straight diagonal.
- HEAD is a hollow ring. A commit with two or more parents is drawn with a centre mark so it differs from a normal
  filled dot. Ref chips and the Commit column hash use their row's lane colour.
- Filtering never removes rows: non-matching rows are dimmed, matching rows stay at full strength, a count `N matches`
  shows beside the input, and `Enter` / `Shift+Enter` select the next / previous match and scroll it into view. (The
  previous behaviour hid non-matching rows and the graph; that existing test is replaced.)

Resize
- A handle is a `role="separator"` with `aria-valuenow`, pointer drag, `←`/`→` (or `↑`/`↓` for the divider) in 16 px
  steps (64 px with Shift), `Home`/`End` for min/max, and double-click to restore the default. Widths are clamped to
  their limits, and the divider keeps the table at least 120 px and the detail at least 160 px (default 320 px).
- Every change writes `goko.history.layout` to `localStorage` as JSON `{graph?, date, author, commit, detailHeight}`.
  A missing, unreadable, or invalid value falls back to defaults; a failing read or write never breaks the pane.

Navigation and actions
- `↑`/`↓` move focus to the previous/next row, `Home`/`End` to the first/last; the existing global `j`/`k` keep
  working. Focus does not select; selection stays on click, `Enter`, and `Space` so arrowing does not fetch a patch per row.
- An `IntersectionObserver` sentinel under the last row calls `fetchNextPage` when it enters the view. The "Load more
  commits" button stays in the sentinel as the keyboard and fallback path and shows "Loading…" while fetching.
- "Jump to HEAD" selects the HEAD row and scrolls it into view; it is disabled when HEAD is not in the loaded pages.
- Right-click on a row opens a menu with "Copy hash" (full oid) and "Copy subject" through `navigator.clipboard`.
  The detail panel's copy button copies the full oid and shows "Copied" for 1.5 s.

Branch filter
- The dropdown sits in the table toolbar. "All branches" (default) keeps the current `--branches --tags --remotes` scope;
  choosing a branch lists only commits reachable from that ref. Changing the branch clears the selection and the filter
  text, restarts paging at the first page, and refetches. The branch list loads with the History query and refreshes
  with the Refresh button.

Uncommitted changes row
- A row labelled "Uncommitted changes (N)" with a dashed edge down to HEAD appears at the top when HEAD is among the loaded
  commits, N > 0 (from the existing `useGitChanges`), and the branch filter allows HEAD (All, or a branch that contains
  HEAD). Selecting it shows the changed entries with their status badges and an "Open in Changes" button that switches to
  the Changes tab; no working-tree patch is fetched.

Detail panel
- Header: short hash with copy, subject, ref chips. Then author `Name <email>` with the absolute date, `Committer: …`
  only when the committer name or email differs, and parents as short-hash buttons that select that parent when it is
  loaded (disabled otherwise). The message body renders as preformatted text, collapsed to 6 lines with "Show more"
  when longer. Then the changed-file list with `+added −removed` (omitted for binary files), then the patch with the
  existing unified/split toggle. For a merge commit the files and patch are against the first parent, and the header says
  "Changes compared with the first parent".

## Edge cases

- No commits: the existing "No commits yet" empty state. A filter with no match: the "No commits match." row is gone
  because rows are never hidden; the count reads `0 matches`.
- Detached HEAD: HEAD ring on the detached commit; the Uncommitted row attaches to it.
- Root commit: parents are empty, the graph line ends, files and stats come from `diff-tree --root`.
- A chosen branch deleted elsewhere: `git log` fails, the pane shows the existing error pane with Retry, and the toolbar
  dropdown stays usable so "All branches" recovers; Refresh reloads the branch list.
- Branch names starting with `-` or not under `refs/heads/` / `refs/remotes/`: the Rust command rejects them as invalid
  input; only a full ref name the branch list returned is accepted.
- Binary files in `--numstat` (`-`) have no counts; renames take their counts from the new path.
- Truncated patch (1 MiB cap) keeps its notice. A failed background refresh keeps the cached table with the existing
  stale notice.
- `localStorage` blocked (private window or quota): defaults are used and the layout does not persist.
- `navigator.clipboard` rejects: the copy button shows "Copy failed" for 1.5 s and the app continues.
- Window narrower than the minimums: the table scrolls horizontally; the detail panel keeps its minimum height.

## Interfaces

Rust (`apps/desktop/src-tauri/src/`)
- `git_view::history(checkout, skip, branch: Option<&str>)` and command `git_view_history(…, skip, branch: Option<String>)`.
  `branch` is a full ref (`refs/heads/x`, `refs/remotes/o/x`); `None` keeps today's scope.
- `git_view::branches(checkout) -> Vec<GitViewBranch { ref_name, name, remote, current }>` and command `git_view_branches`,
  registered in `lib.rs`.
- `GitViewDiff` gains `detail: Option<GitViewCommitDetail { author_email, committer, committer_email, body }>` (set only for
  `layer = commit`); `StatusEntry` gains `added: Option<u64>` and `removed: Option<u64>` (null outside commit diffs and for
  binary files).

TypeScript (`apps/desktop/src/`)
- `services/types.ts`: `GitViewBranch`, `GitViewCommitDetail`, and the new `GitViewDiff.detail` / entry counts.
- `services/git-view.ts`: `history(projectId, checkoutPath, skip, branch)` and `branches(projectId, checkoutPath)`.
- `queryClient.ts`: `gitHistory(projectId, checkoutPath, branch)` keeps `['git-history', projectId, checkoutPath, …]` so the
  existing prefix invalidation still matches; new `gitBranches`.
- `features/editor/queries.ts`: `useGitHistory(…, branch)` and `useGitBranches`.
- New files: `features/editor/history-graph.ts` (lane + colour layout, pure), `HistoryTable.tsx`, `CommitDetail.tsx`,
  `history-layout.ts` (column widths and persistence), `components/ui/resizable-handle.tsx`,
  `components/ui/context-menu.tsx` (thin wrapper over the already installed `radix-ui`). `EditorHistory.tsx` becomes the
  container; `panes.tsx` `ChangedFiles` shows the counts when present. `ResizableSplit` is unchanged. No new dependency.

## Verification

- `history-graph.ts` unit test: colour continuity along a first-parent line, a new colour for a merge's second parent,
  lane reuse after a branch ends, root commit, and the HEAD/merge flags.
- `history-layout.ts` and `resizable-handle` tests: clamping, defaults, keyboard steps, double-click reset, invalid or
  throwing `localStorage`.
- Component tests in `EditorWorkspace.test.tsx`: the HEAD row, refs, and patch still render; the filter now dims instead of
  hiding and `Enter` jumps; branch dropdown passes the ref to `history`; Uncommitted row opens Changes; context-menu copy;
  detail shows body, parents, and counts; the sentinel calls `fetchNextPage` (a fake `IntersectionObserver`).
- Rust tests in `git_view.rs`: branch-scoped history, invalid branch names rejected, `branches` lists local and remote with
  the current flag, commit detail (body, email, committer), numstat for text, binary, rename, and root commits.
  `wiring_tests.rs` covers the new command and the extra argument.
- Commands: `pnpm -C apps/desktop typecheck`, `pnpm -C apps/desktop test`, `cargo test --workspace`, `cargo fmt --check`,
  `cargo clippy --workspace -- -D warnings`, each after a baseline run before the first edit.
- Visual check: run the app on this repository (it has merge commits) and confirm the lane colours, curved edges,
  HEAD/merge nodes, column dragging, reload persistence, and dark/light themes. If the app cannot be driven here, say so
  instead of claiming the visual check passed.

## Related context

- [Desktop Editor](../../../../llm/architecture/editor.md)
