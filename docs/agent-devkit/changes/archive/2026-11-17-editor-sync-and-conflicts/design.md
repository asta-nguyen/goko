# Editor sync and conflicts

## Goal

The Editor shows what agents and Git do in the checkout without a manual refresh, and the merge conflicts the inline editor cannot fix (deleted-on-one-side, added-on-one-side, binary) are resolved in the app instead of the terminal. A merge or rebase is finished from the Changes section. A resolved README with a `=======` heading is no longer stuck as "unresolved".

Found by the 2026-11-17 review of the Editor and Changes sections (chat); behavior aligned with the reference editor the user pointed at. Tabs are a separate change: [editor-tabs](../2026-11-17-editor-tabs/design.md).

## Scope

In scope, in this order:

1. **Marker bug.** A bare `=======` line is not a conflict marker.
2. **Conflict kinds.** Changes names each conflict by what Git reports (both modified, deleted by us, …) instead of one "Conflict" badge.
3. **Resolve actions.** Per-file Keep our version, Keep their version, Delete file, Use Current, Use Incoming for the conflicts the inline editor cannot resolve.
4. **Mark Resolved asks instead of blocking.** Staging a file that still has conflict markers needs a confirmation rather than being refused.
5. **Operation banner and Continue.** When a merge, rebase, cherry-pick or revert is in progress, Changes says so and offers Abort; the Commit button becomes Continue; a merge's commit message is prefilled.
6. **Background refresh.** Changes data refreshes by itself while the Editor is in use, backing off when reads are slow.
7. **Open file vs disk.** The open file notices when it changed or vanished on disk, and a changed file offers Overwrite as well as Reload.

Out of scope: a file-system watcher (polling is enough, no new dependency), auto-refresh of History and Stashes, pull/fetch/checkout/stash writes, amend, a three-way merge view, a "Stage all" action for the Conflicts group, Overwrite for a file that vanished, Windows-specific work, tabs.

## Behavior

**Marker rule (1).** A line counts as a conflict marker only when it is `<<<<<<<`, `|||||||` or `>>>>>>>` (seven or more of the same character, optional label after). A bare `=======` line never counts on its own. A real conflict always has a start and an end line, so a malformed block is still flagged through those. The rule is the single `conflict::has_markers`, so the Mark Resolved confirmation, the "unsupported markers" notice and the backend re-check in `stage_resolved` all follow it.

**Kinds (2).** `StatusEntry.status` for a conflict becomes one of `both-modified` (UU), `both-added` (AA), `deleted-by-us` (DU), `deleted-by-them` (UD), `both-deleted` (DD), `added-by-us` (AU), `added-by-them` (UA). The badge reads "Both modified", "Both added", "Deleted by us", "Deleted by them", "Both deleted", "Added by us", "Added by them". "Us" is the `HEAD` side, the same side the inline editor calls Current, in merge and in rebase alike. Rows stay in the Conflicts group.

**Resolve actions (3).** Offered per conflict kind; the backend action ids are `current`, `incoming`, `keep`, `delete`:

| Kind | Buttons | Git |
|---|---|---|
| both-modified, both-added | Use Current, Use Incoming | `checkout --ours -- path` / `checkout --theirs -- path`, then `add -- path` |
| deleted-by-them, added-by-us | Keep our version, Delete file | `add -- path` / `rm -- path` |
| deleted-by-us, added-by-them | Keep their version, Delete file | `add -- path` / `rm -- path` |
| both-deleted | Delete file | `rm -- path` |

Text files with markers keep the inline editor and Mark Resolved; the whole-file actions are an alternative for them and the only route for binary, non-UTF-8, missing and oversized files. Actions live in the conflict row's context menu (every conflict) and as buttons in the editor pane's conflict notice when the file cannot be resolved inline or the kind is a deletion or addition one (`deleted-by-us`, `deleted-by-them`, `added-by-us`, `added-by-them`, `both-deleted`), where Git leaves only one side's file on disk and there is no merged text to edit. Editable `both-modified` and `both-added` files keep the inline editor and show no whole-file buttons. Each acts on one path, runs on the serialized git-write lane, and refreshes Changes afterwards. Nothing commits. An action is refused with the notice "save or discard the changes in {path} first" while an open document for that path has unsaved edits, because it rewrites the file on disk.

**Mark Resolved (4).** For an editable text conflict file the button is enabled whenever the file can be saved and no write is in flight. With no conflict markers left it behaves as today (save, then stage that one file). With markers left it opens a dialog "{file} still contains conflict markers. Stage it anyway?" with Stage and Cancel; only Stage saves and stages. The backend still re-reads the file and refuses when its text differs from what the editor sent, and refuses markers unless the request says the user confirmed.

**Operation banner and Continue (5).** Changes data gains `operation`: `merge`, `rebase`, `cherry-pick`, `revert` or none, found from Git's own state files (`MERGE_HEAD`, `rebase-merge`/`rebase-apply`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, resolved with `git rev-parse --git-path` so linked worktrees work). While set:

- A banner above the groups reads "Merge in progress" (etc.) with **Abort**, behind a confirmation dialog ("Abort the merge? Resolutions made so far are lost."), running `merge|rebase|cherry-pick|revert --abort`.
- The commit panel's primary button reads **Continue** instead of Commit and is enabled once no conflict remains; staged changes are not required.
  - Merge: Continue is Commit — it needs a non-blank message and runs `git commit -m`, which Git allows even with nothing staged because the merge is in progress.
  - Rebase, cherry-pick, revert: Continue runs `<op> --continue` with `GIT_EDITOR=true` and prompts off, 120 s limit like Push; the message box is neither used nor required.
- During a merge, when the commit message draft is empty, it is filled once with Git's own merge message (`MERGE_MSG`, comment lines removed). A message the user cleared is not refilled during the same merge; the memory resets when the merge ends.

**Background refresh (6).** While `view == Editor`, the window is focused and the section is Changes, the app re-reads Changes every 3 seconds, the same one-outstanding-probe rule as the branch badge. After a read that took longer than 600 ms, the next read waits five times as long (capped at 30 s), so a huge repository is polled gently. It also reads immediately when the window regains focus and when the Editor view is entered (the entry waits until a checkout is selected), and then also refreshes the file list so agent-created files appear in the tree. Opening a file from Changes or History loads the file list when it is missing, so the Files tree is never empty after that jump. The poll is quiet: no loading spinner, no notice on success. It is skipped while a write is in flight (`changes.busy`) or the queue is full. The selected diff is re-read only when the new Changes data differs from the old.

**Open file vs disk (7).** On the same triggers (every 3 s while the Files section shows a document, and on focus), the active document is compared with disk:

- Clean document, disk text differs: replaced with the disk text silently, caret and scroll kept as near as the text allows.
- Dirty document, disk text differs: kept; a notice row "File changed on disk" with **Overwrite** (save the buffer over the new disk text) and **Reload from disk** (the existing reload path, which drops the edits). A save refused because the file changed on disk shows the same row.
- Disk file missing, unreadable or no longer text: kept; the row reads "File deleted on disk" / "File is no longer text" with Reload from disk disabled and no Overwrite.

## Edge cases

- Stale UI: the backend re-reads status inside `resolve` and refuses an action not valid for the path's current kind, or a path no longer in conflict.
- `checkout --theirs` on a kind without that side cannot happen: the table above never offers it, and the backend rejects it.
- Path safety: every action takes a checkout-relative path validated by `git_view::relative_path` and passed as a literal pathspec, like Stage.
- Whole-file action while the file is open with unsaved edits: refused as described; with the file open and clean, the disk change is picked up by the open-file check.
- Mark Resolved confirmation raced by a disk change: the backend's expected-text check fails and the notice reports it; nothing is staged.
- Abort with nothing in progress, or Continue with unresolved conflicts: Git's error is shown in the section notice; no state changes in the app.
- Merge Continue with nothing staged: allowed because Git allows it; if Git refuses, its error is shown.
- `rebase --continue` needs an editor for reworded commits: `GIT_EDITOR=true` keeps it non-interactive; a hook or Git prompt that blocks hits the 120 s limit and reports a timeout.
- Rename detection: renamed-and-conflicted files keep their `orig_path` display; actions use `path`.
- A poll reply that lands after a checkout switch or a write is dropped by the existing checkout-key guard; a poll never overwrites a newer write's data because writes already refresh through the same lane in order.
- Queue refused or poll error: the previous data stays, no spinner, no notice for a single poll failure; a manual Refresh still reports errors.
- The document replaced by auto-reload bumps `code.revision`; an in-flight highlight/blame reply for the old revision is dropped by the existing revision check and the new revision is re-requested.
- Overwrite races a second disk change: the backend reads the disk text right before replacing and the existing expected-text check rejects a change in between; the notice stays.
- Huge repositories: the Changes poll uses the existing `--no-optional-locks` status, runs at most one at a time, backs off as described, and adds no git process beyond the one `rev-parse --git-path` for operation state.

## Interfaces

- `app_core::git_view::StatusEntry.status`: seven new conflict values replace `"unmerged"`.
- `app_core::git_view::GitViewChanges.operation: Option<&'static str>` and `merge_message: Option<String>` (set only while `operation` is `merge`).
- `app_core::git_write::resolve(checkout, path, action)` with `action` one of `current`, `incoming`, `keep`, `delete`; `app_core::git_write::operation(checkout, step)` with `step` `abort` or `continue`, the operation detected inside the call; `git_write::commit` also accepts an empty index while a merge is in progress.
- `editor::backend::Req::{Resolve, Operation, ChangesPoll, CheckFile, OverwriteFile}` (`ChangesPoll` is the quiet background Changes read, applied without a loading indicator or notice); `Req::StageResolved` gains `allow_markers: bool`; `Resolve` and `Operation` join the git-write lane lists (`is_mutating`, `is_git_write`, `detachable`).
- `editor::commit::can_commit` takes the operation into account; `EditorState::path_is_dirty(path)` is the one test for "this path has unsaved edits" (the tabs change extends it to inactive tabs).
- `OpenDoc` gains a `disk_notice: Option<DiskNotice>`.
- No protocol, daemon or schema change.

## Verification

- Unit tests per task in the module that owns the behavior (`conflict.rs`, `git_view.rs`, `git_write.rs` against real temporary repositories, `backend.rs`, `changes.rs`, `commit.rs`, `files.rs`).
- `cargo fmt --all -- --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`.
- UI changes are checked in the fresh `target/Goko.app` window with `agent-desktop`, per AGENTS.md (Goko window only, no real data written): a conflicted temporary repository for the actions, banner, Continue and message prefill; an external file edit for refresh, the disk notice and Overwrite.

## Related context

- [rune-editor.md](../../../../llm/domains/rune-editor.md) (`RNE-inline-conflicts`, `RNE-changes-git-writes`, the branch-badge polling requirement).
