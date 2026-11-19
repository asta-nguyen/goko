# Delta

Merged into docs/llm/: 2026-11-19

Approved: 2026-11-19 — Push may prompt for an SSH key passphrase in a masked app modal; passphrase-only, never stored, cancel stops the push.

## MODIFIED Requirements

### RNE-changes-git-writes

Previously: Push never prompted for credentials; a passphrase-protected SSH key failed with Git's publickey error.

Baseline:

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

Replacement:

````md
### RNE-changes-git-writes

The system SHALL show the checkout's conflict, staged, unstaged, and untracked groups with counts, per-layer diffs and each conflict's kind (both modified, both added, deleted by us, deleted by them, both deleted, added by us, added by them), staging, unstaging, discarding, resolving conflicts, committing, and pushing only through the serialized write lane — Discard only after a confirmation naming the file, Commit only with a non-empty message and no conflicts, and Push never forcing, prompting only for an SSH key passphrase in a masked modal whose answer goes to that one push and is never stored, and failing every other credential prompt — and, while Git reports a merge, rebase, cherry-pick or revert in progress, showing a banner offering Abort behind a confirmation and turning Commit into Continue, which is enabled once no conflict remains (and, for a merge, the message is not blank) and either commits a merge, with Git's merge message prefilled, or runs the operation's `--continue` non-interactively.

#### Scenario: Stage, commit, push

- GIVEN a checkout has unstaged files
- WHEN the user stages a file, types a message, and presses Commit then Push
- THEN Git creates the commit, the draft clears on success, and the push runs without force under the backend's 120-second limit

#### Scenario: SSH passphrase prompt

- GIVEN the push needs a passphrase for its SSH key and no ssh-agent holds the key
- WHEN the user presses Push and ssh asks for the passphrase
- THEN a masked "SSH key passphrase" modal shows ssh's own prompt, submitting continues the same push, and a rejected passphrase opens the prompt again

#### Scenario: Cancel the passphrase prompt

- GIVEN the passphrase modal is open
- WHEN the user cancels or presses Escape
- THEN no passphrase is sent, the push stops, and the panel shows "Push cancelled" instead of Git's publickey error

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

Evidence: `apps/rune/src/editor/changes.rs`, `apps/rune/src/editor/commit.rs`, `apps/rune/src/editor/patch.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/askpass.rs`, `apps/rune/src/main.rs`, `crates/app-core/src/git_write.rs`, `crates/app-core/src/git_view.rs`
````
