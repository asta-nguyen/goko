# Delta

Merged into docs/llm/: 2026-11-18

## ADDED Requirements

### RNE-changes-auto-refresh
Page: `docs/llm/domains/rune-editor.md`
Search: checked every RNE requirement; `RNE-current-branch` polls only the branch badge, `RNE-changes-git-writes` refreshes after writes, and the section requirement refreshes on activation or manual refresh. None covers a background refresh of Changes or the open file.
Requirement:
````md
### RNE-changes-auto-refresh

The system SHALL, while the Editor view is active and the window is focused, re-read the Changes list every three seconds (less often after a slow read) while the Changes section shows and compare the open document with disk every three seconds while the Files section shows — each, and the file list, also at once when the window regains focus or the Editor view is entered — without a loading indicator and without a Changes read while a Git write is in flight, silently replacing a clean document whose disk text differs, keeping a dirty document with a notice row offering Overwrite and Reload from disk, and keeping a vanished document with a notice row.

#### Scenario: Agent edits a file while Changes is open

- GIVEN the Changes section is showing and an agent writes a tracked file in the checkout
- WHEN the next three-second read lands
- THEN the file appears in the groups and the selected diff updates without pressing Refresh

#### Scenario: Open file changed on disk

- GIVEN a document with no unsaved edits is open and the file changes on disk
- WHEN the next check runs
- THEN the editor shows the disk text

#### Scenario: Unsaved edits and a changed file

- GIVEN a document with unsaved edits is open and the file changes on disk
- WHEN the next check runs or Save is refused
- THEN the buffer is kept and "File changed on disk" offers Overwrite, which saves the buffer over the new disk text, and Reload from disk, which drops the edits

#### Scenario: No read during a write

- GIVEN a stage, commit or resolve is in flight
- WHEN a poll tick arrives
- THEN no Changes read is queued for that tick

Evidence: `apps/rune/src/editor/autorefresh.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/files.rs`, `apps/rune/src/main.rs`
````

## MODIFIED Requirements

### RNE-inline-conflicts
Previously: only text conflicts with no markers could be marked resolved, a bare `=======` line counted as a marker, and binary, missing or non-editable conflicts offered no resolution.
Baseline:
````md
### RNE-inline-conflicts

The system SHALL offer inline Current, Incoming, and Both conflict resolution with undo, conflict navigation, and explicit Mark Resolved that saves and stages only the selected editable text file after all conflict markers are removed, preserving draft and disk-change guards and never committing automatically.

#### Scenario: Resolve a text conflict

- GIVEN an editable UTF-8 file in Conflicts
- WHEN the user opens it and accepts Current, Incoming, or Both for a block
- THEN the editor removes that block's markers, Both keeps Current before Incoming, and the edit can be undone or changed manually

#### Scenario: Finish resolving

- GIVEN the selected conflicted file has no remaining conflict markers
- WHEN the user presses Mark Resolved
- THEN the file is saved through the expected-text guard and only that file is staged; a save or stage failure is reported without committing

#### Scenario: Unsupported or incomplete conflict

- GIVEN a binary, missing, non-editable, or malformed conflict file
- WHEN the file is opened
- THEN the editor explains the limitation and does not offer unsafe automatic resolution or staging

Evidence: `apps/rune/src/editor/conflict.rs`, `apps/rune/src/editor/code.rs`, `apps/rune/src/editor/files.rs`, `apps/rune/src/editor/backend.rs`
````
Replacement:
````md
### RNE-inline-conflicts

The system SHALL offer inline Current, Incoming, and Both conflict resolution with undo, conflict navigation, and explicit Mark Resolved that saves and stages only the selected editable text file — asking for confirmation first while conflict markers (`<<<<<<<`, `|||||||` or `>>>>>>>` lines, never a bare `=======` line) remain — preserving draft and disk-change guards and never committing automatically, and offering explicit per-file Keep our version, Keep their version, Delete file, Use Current and Use Incoming actions, valid for the conflict's kind and refused while the file has unsaved edits, for a conflict the inline editor cannot resolve (binary, missing, non-editable) or that Git reports as deleted or added on one side.

#### Scenario: Resolve a text conflict

- GIVEN an editable UTF-8 file in Conflicts
- WHEN the user opens it and accepts Current, Incoming, or Both for a block
- THEN the editor removes that block's markers, Both keeps Current before Incoming, and the edit can be undone or changed manually

#### Scenario: Finish resolving

- GIVEN the selected conflicted file has no remaining conflict markers
- WHEN the user presses Mark Resolved
- THEN the file is saved through the expected-text guard and only that file is staged; a save or stage failure is reported without committing

#### Scenario: Markers left

- GIVEN the selected conflicted file still contains a `<<<<<<<` or `>>>>>>>` line
- WHEN the user presses Mark Resolved
- THEN a dialog names the file and asks whether to stage it anyway; only confirming saves and stages it, and cancelling changes nothing

#### Scenario: Heading underline is not a marker

- GIVEN a conflicted Markdown file whose resolved text still contains a `=======` heading underline and no `<<<<<<<` or `>>>>>>>` line
- WHEN the user opens it and presses Mark Resolved
- THEN no incomplete-marker notice shows and the file is staged without a dialog

#### Scenario: Deleted-on-one-side conflict

- GIVEN a file Git reports as deleted by them and modified by us
- WHEN the user chooses Keep our version or Delete file
- THEN only that path is staged (`git add`) or removed (`git rm`), Changes refreshes, and nothing is committed

#### Scenario: Whole-file choice for a binary conflict

- GIVEN a binary file in conflict that both sides modified
- WHEN the user chooses Use Incoming
- THEN that path takes the incoming version and is staged, and a stale choice not valid for the path's current kind is refused without changing it

#### Scenario: Unsaved edits block a whole-file choice

- GIVEN the conflicted file is open with unsaved edits
- WHEN the user chooses Use Incoming
- THEN a notice asks to save or discard first and the file is not changed

Evidence: `apps/rune/src/editor/conflict.rs`, `apps/rune/src/editor/code.rs`, `apps/rune/src/editor/files.rs`, `apps/rune/src/editor/changes.rs`, `apps/rune/src/editor/backend.rs`, `crates/app-core/src/git_write.rs`, `crates/app-core/src/git_view.rs`
````

### RNE-changes-git-writes
Previously: no conflict kinds, no merge, rebase, cherry-pick or revert state, no abort or continue, and an empty merge message.
Baseline:
````md
### RNE-changes-git-writes

The system SHALL show the checkout's conflict, staged, unstaged, and untracked groups with counts and per-layer diffs, staging, unstaging, discarding, committing, and pushing only through the serialized write lane — Discard only after a confirmation naming the file, Commit only with a non-empty message and no conflicts, and Push never forcing or prompting for credentials.

#### Scenario: Stage, commit, push

- GIVEN a checkout has unstaged files
- WHEN the user stages a file, types a message, and presses Commit then Push
- THEN Git creates the commit, the draft clears on success, and the push runs without force under the backend's 120-second limit

#### Scenario: Discard requires confirmation

- GIVEN an unstaged or untracked file is selected
- WHEN the user presses Discard
- THEN a dialog names the file; only confirming enqueues the write, and cancelling changes nothing

#### Scenario: Write failure keeps state honest

- GIVEN a stage, commit, or push fails in Git
- WHEN the error reply lands
- THEN the message shows in the section or notice, the commit draft stays, and the checkout is left as Git left it

Evidence: `apps/rune/src/editor/changes.rs`, `apps/rune/src/editor/commit.rs`, `apps/rune/src/editor/patch.rs`, `apps/rune/src/editor/backend.rs`, `crates/app-core/src/git_write.rs`, `crates/app-core/src/git_view.rs`
````
Replacement:
````md
### RNE-changes-git-writes

The system SHALL show the checkout's conflict, staged, unstaged, and untracked groups with counts, per-layer diffs and each conflict's kind (both modified, both added, deleted by us, deleted by them, both deleted, added by us, added by them), staging, unstaging, discarding, resolving conflicts, committing, and pushing only through the serialized write lane — Discard only after a confirmation naming the file, Commit only with a non-empty message and no conflicts, and Push never forcing or prompting for credentials — and, while Git reports a merge, rebase, cherry-pick or revert in progress, showing a banner offering Abort behind a confirmation and turning Commit into Continue, which is enabled once no conflict remains (and, for a merge, the message is not blank) and either commits a merge, with Git's merge message prefilled, or runs the operation's `--continue` non-interactively.

#### Scenario: Stage, commit, push

- GIVEN a checkout has unstaged files
- WHEN the user stages a file, types a message, and presses Commit then Push
- THEN Git creates the commit, the draft clears on success, and the push runs without force under the backend's 120-second limit

#### Scenario: Discard requires confirmation

- GIVEN an unstaged or untracked file is selected
- WHEN the user presses Discard
- THEN a dialog names the file; only confirming enqueues the write, and cancelling changes nothing

#### Scenario: Write failure keeps state honest

- GIVEN a stage, commit, push, resolve, abort or continue fails in Git
- WHEN the error reply lands
- THEN the message shows in the section or notice, the commit draft stays, and the checkout is left as Git left it

#### Scenario: Abort an operation

- GIVEN a merge stopped on conflicts
- WHEN Changes loads
- THEN a "Merge in progress" banner shows with Abort; confirming runs `git merge --abort`, and cancelling changes nothing

#### Scenario: Finish a merge

- GIVEN a merge stopped on conflicts and every conflict is resolved
- WHEN Changes shows the commit panel
- THEN the message is prefilled with Git's merge message once, the button reads Continue, and pressing it creates the merge commit even when nothing else is staged

#### Scenario: Continue a rebase

- GIVEN a rebase stopped and every conflict is resolved and staged
- WHEN the user presses Continue
- THEN `git rebase --continue` runs non-interactively with a 120-second limit, and Continue is disabled while any conflict remains

Evidence: `apps/rune/src/editor/changes.rs`, `apps/rune/src/editor/commit.rs`, `apps/rune/src/editor/patch.rs`, `apps/rune/src/editor/backend.rs`, `crates/app-core/src/git_write.rs`, `crates/app-core/src/git_view.rs`
````
