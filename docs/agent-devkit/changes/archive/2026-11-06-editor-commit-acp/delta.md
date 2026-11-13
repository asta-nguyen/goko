# Delta

Merged into docs/llm/: 2026-11-07

## ADDED Requirements

### EDT-stage-commit

Page: `docs/llm/architecture/editor.md`
Search: checked every page with prefix EDT; no existing requirement covers staging or committing.

Requirement:

````md
### EDT-stage-commit

The system SHALL let the user stage and unstage files from the Editor's Changes view and commit the staged changes with a non-empty message, running Git hooks and never amending or skipping them.

#### Scenario: Stage a file and commit it

- GIVEN a checkout has an unstaged modified file
- WHEN the user stages the file, types a message, and presses Commit
- THEN Git creates a commit containing that file, the message box clears, and the Changes list refreshes

#### Scenario: Commit rejected

- GIVEN a pre-commit hook fails or nothing is staged
- WHEN the user presses Commit
- THEN Git's error is shown, the message stays, and the index is unchanged

Evidence: `apps/desktop/src-tauri/src/git_write.rs`, `apps/desktop/src/features/editor/CommitBox.tsx`, `apps/desktop/src/features/editor/EditorChanges.tsx`, `apps/desktop/src/features/editor/CommitBox.test.tsx`, `apps/desktop/src/features/editor/EditorWorkspace.test.tsx`
````

### EDT-discard-changes

Page: `docs/llm/architecture/editor.md`
Search: checked every page with prefix EDT; no existing requirement covers discarding changes.

Requirement:

````md
### EDT-discard-changes

The system SHALL discard one unstaged or untracked file only after the user confirms a dialog naming it, restoring a tracked file to its index content or deleting an untracked regular file or symlink, and never deleting a directory.

#### Scenario: Discard an unstaged edit

- GIVEN a tracked file has unstaged edits
- WHEN the user presses Discard and confirms
- THEN the file returns to its staged content and leaves the Unstaged list

#### Scenario: Cancel the confirmation

- GIVEN the Discard confirmation is open
- WHEN the user cancels
- THEN the file is unchanged

Evidence: `apps/desktop/src-tauri/src/git_write.rs`, `apps/desktop/src/features/editor/EditorChanges.tsx`, `apps/desktop/src/features/editor/EditorWorkspace.test.tsx`
````

### EDT-push

Page: `docs/llm/architecture/editor.md`
Search: checked every page with prefix EDT; no existing requirement covers pushing.

Requirement:

````md
### EDT-push

The system SHALL push the current branch from the Changes view with `git push` (or `git push -u origin HEAD` when it has no upstream), never forcing, never prompting for credentials, and stopping after 120 seconds.

#### Scenario: Push commits

- GIVEN the branch has commits ahead of its upstream
- WHEN the user presses Push
- THEN Git pushes without force and the ahead count refreshes

#### Scenario: Push fails

- GIVEN the remote rejects the push or credentials are missing
- WHEN the user presses Push
- THEN Git's error is shown and nothing is retried

Evidence: `apps/desktop/src-tauri/src/git_write.rs`, `apps/desktop/src-tauri/src/worktrees.rs`, `apps/desktop/src/features/editor/CommitBox.tsx`, `apps/desktop/src/features/editor/CommitBox.test.tsx`
````

### EDT-acp-commit-message

Page: `docs/llm/architecture/editor.md`
Search: checked every page with prefix EDT; no existing requirement covers agent-drafted commit messages.

Requirement:

````md
### EDT-acp-commit-message

The system SHALL, only when the user presses Generate, send the staged diff to the picked, connected local ACP agent in a new session whose cwd is an empty temporary directory with permission requests denied, and put the agent's reply in the commit message box without committing.

#### Scenario: Generate a message

- GIVEN a file is staged and the picked ACP agent is connected
- WHEN the user presses Generate and the agent finishes its turn
- THEN the agent's cleaned reply fills the message box and nothing is committed

#### Scenario: Cancel or fail

- GIVEN a generation is running
- WHEN the user presses Cancel, or the agent exits, times out, or does not end its turn normally
- THEN the session is cancelled, the connection stays, the message box keeps its text, and an error is shown for failures

Evidence: `apps/desktop/src-tauri/src/acp.rs`, `apps/desktop/src/features/editor/CommitBox.tsx`, `apps/desktop/src/features/editor/CommitBox.test.tsx`
````

### SET-acp-agents

Page: `docs/llm/domains/desktop-settings.md`
Search: checked every page with prefix SET; no existing requirement covers ACP agents.

Requirement:

````md
### SET-acp-agents

The system SHALL list five preset ACP agents (Devin, OpenCode, Claude Code, Codex, Gemini) in a Settings section where each can be enabled or disabled, given a command line and a commit prompt, and connected or disconnected with a visible status, saving overrides immediately and rejecting an invalid command line.

#### Scenario: Change an agent command

- GIVEN the ACP agents section is open
- WHEN the user edits the Devin command line to a valid value
- THEN the change is saved and the next Generate uses it

#### Scenario: Connect and disconnect

- GIVEN an enabled agent is disconnected
- WHEN the user presses Connect and later Disconnect
- THEN its status becomes connected and then disconnected, and Disconnect ends the agent process

#### Scenario: Reject an invalid command

- GIVEN the ACP agents section is open
- WHEN the user enters a command with an unterminated quote
- THEN an inline error is shown and the stored value is unchanged

Evidence: `apps/desktop/src/features/settings/SettingsPage.tsx`, `apps/desktop/src/features/settings/AcpAgentsSection.tsx`, `apps/desktop/src-tauri/src/acp.rs`, `apps/desktop/src/features/settings/SettingsPage.test.tsx`
````
