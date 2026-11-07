# Desktop Editor

## Requirements

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

### EDT-codemirror-surface

The system SHALL edit complete UTF-8 text files in CodeMirror with line numbers and filename-selected syntax support, keeping unknown or failed language modules usable as plain text.

#### Scenario: Open an editable source file

- GIVEN a complete UTF-8 file is selected in Files
- WHEN its filename has a supported language
- THEN CodeMirror renders the text with line numbers and that language's syntax support

Evidence: `apps/desktop/src/features/editor/CodeEditor.tsx`, `apps/desktop/src/features/editor/code-theme.ts`, `apps/desktop/src/features/editor/EditorFiles.tsx`, `apps/desktop/src/features/editor/CodeEditor.test.tsx`

### EDT-content-search

The system SHALL search checkout file contents literally after a trimmed query has at least two characters and 250 ms without typing, then allow a result to open at its matching line through the unsaved-draft navigation guard.

#### Scenario: Find and open a content match

- GIVEN Content mode is active and the query has at least two trimmed characters
- WHEN typing pauses for 250 ms and a result is selected
- THEN the checkout search result opens in the editor with its matching line revealed

Evidence: `apps/desktop/src/features/editor/EditorSearch.tsx`, `apps/desktop/src/features/editor/EditorFiles.tsx`, `apps/desktop/src/features/editor/EditorWorkspace.test.tsx`, `apps/desktop/src-tauri/src/git_view.rs`

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

### EDT-go-to-definition

The system SHALL, on Cmd+click (macOS) or Ctrl+click (other platforms) in the Files editor, open the checkout file a clicked import specifier resolves to, or jump to the declaration of a clicked name found through the checkout's content search, and when no single declaration is found, open Content mode with that name as the query.

#### Scenario: Open a clicked relative import

- GIVEN a source file with `import { x } from './util'` is open in Files and `util.ts` exists next to it
- WHEN the user Cmd/Ctrl+clicks the `'./util'` specifier
- THEN `util.ts` opens through the unsaved-draft navigation guard

#### Scenario: Jump to a unique declaration

- GIVEN a name is declared exactly once in the checkout and not in the open file
- WHEN the user Cmd/Ctrl+clicks a use of that name
- THEN the declaring file opens with the declaration line revealed

#### Scenario: Ambiguous or missing declaration

- GIVEN a clicked name has no declaration or several declarations outside the open file
- WHEN the user Cmd/Ctrl+clicks it
- THEN Content mode opens with the name as its query; an import that resolves to no checkout file shows a short notice instead

Evidence: `apps/desktop/src/features/editor/go-to-definition.ts`, `apps/desktop/src/features/editor/CodeEditor.tsx`, `apps/desktop/src/features/editor/EditorFiles.tsx`, `apps/desktop/src/features/editor/go-to-definition.test.ts`, `apps/desktop/src/features/editor/CodeEditor.test.tsx`, `apps/desktop/src/features/editor/EditorWorkspace.test.tsx`

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

### EDT-vim-save

The system SHALL persist the optional Vim mode under `editor.vim` and route Vim `:w` to the active editor's existing save action.

#### Scenario: Save from Vim mode

- GIVEN Vim mode is enabled on the Settings page
- WHEN the user runs `:w`
- THEN the active editor invokes the same save action as the Save button

Evidence: `apps/desktop/src/features/editor/queries.ts`, `apps/desktop/src/features/settings/SettingsPage.tsx`, `apps/desktop/src/features/editor/CodeEditor.tsx`, `apps/desktop/src/features/editor/queries.test.tsx`, `apps/desktop/src/features/editor/CodeEditor.test.tsx`

## Flow

Files mode keeps the existing filename filter and tree. Content mode trims and debounces the shared input, then `useGitSearch` keys the result by project, checkout, and query; the service invokes `git_view_search`. The Tauri command revalidates the registered checkout before `git_view::search` runs Git through the bounded runner. Selecting a result uses the Files navigation path. An already open file receives a line reveal directly; changing files goes through the shared unsaved-draft guard. CodeMirror clamps the requested line to the document, selects it, scrolls it into view, and focuses the editor. `Cmd/Ctrl+F` inside CodeMirror remains available to its search panel.

Cmd+click (macOS) or Ctrl+click (other platforms) on a token in the Files editor starts a lookup; a modified click is handled ahead of Vim, and while the modifier is held the token under the pointer is underlined. An import specifier (`from`, `import`, `require` strings) is resolved against the checkout's file list: relative paths try the extensions `.ts .tsx .js .jsx .mjs .cjs .json` and `index.*`, a `.js` specifier also tries its TypeScript source, and other specifiers try the `compilerOptions.paths` aliases of the nearest `tsconfig.json` or `jsconfig.json` that defines any. A hit opens through the Files navigation path. Any other name of two or more characters outside a comment or string is searched with the content search; its declaration lines are picked by keyword and shape patterns (JS/TS, Python, Rust, Go). The first declaration in the open file wins, otherwise a single declaration in the checkout opens at its line, and anything else switches the sidebar to Content mode with the name as the query.

The Changes list writes through `git_write_*` commands. Row buttons Stage (`+`, on Unstaged, Untracked, and Conflicts rows), Unstage (`−`, on Staged rows), and Discard (trash, on Unstaged and Untracked rows only) act on one path without selecting the row; group headers add "Unstage all" (no paths) and "Stage all" (that group's file paths, so the other group is untouched). Discard opens a confirmation dialog naming the file and saying it cannot be undone; Cancel changes nothing. Each command revalidates the registered checkout before `git_write` runs Git, and a successful write invalidates the checkout's changes query and its diffs.

The commit box under the list holds the message draft, an agent picker with each agent's status, a Connect button, Generate/Cancel, Commit, and Push. Commit runs `git commit -m <message>` and, on success, clears the draft and shows `Committed <hash> on <branch>`. Push shows `Push ↑N` when `GitViewChanges.ahead` (from `git rev-list --count @{u}..HEAD`) is a positive number, `Push new branch` when it is `null` (no upstream), and is disabled at 0. The box is also shown on a clean tree so commits ahead of the upstream stay pushable.

Generate calls `acp_generate_commit_message`. The picked agent is the stored `commit.agent` while that agent stays enabled, else the first connected one, else the first enabled one found on `PATH`. The backend takes the agent's live connection from the ACP hub, builds the prompt (the agent's commit prompt or the default instructions, the last five commit subjects, `git diff --cached --stat`, and `git diff --cached` capped at 120 KiB with a `(diff truncated)` line), opens a new session in a fresh empty temporary directory, and returns the cleaned reply. The reply fills the draft only when the user has not typed since pressing Generate. Connect and Disconnect call `acp_connect` and `acp_disconnect`; the status map from `acp_status` is polled every 3 seconds while the Changes view or the ACP settings section is visible.

## State changes

The Vim mode switch on the Settings page reads and writes the existing settings key `editor.vim` as `"1"` or `"0"`; missing, unreadable, or other values leave Vim off. Editing continues to use the existing draft state and save path. Content search is read-only and does not change checkout files.

Staging, unstaging, discarding, committing, and pushing change the checkout's index, working tree, HEAD, and remote refs. The commit box's agent pick is stored under the settings key `commit.agent`; the agent list and overrides live under `acp.agents` (see [Desktop Settings](../domains/desktop-settings.md#set-acp-agents)). ACP connections and the running Generate jobs (one per project and checkout) are in-memory state in the backend hub and are not persisted.

## Side effects

Content search starts `git grep` with argv in the validated checkout and no shell. It searches tracked and non-ignored untracked text files, skips binary files, and does not mutate Git state. Saving Vim `:w` uses the existing file-save command and its conflict and file-size checks. A go-to lookup is read-only: it reuses `git_view_file_list`, `git_view_search`, and `git_view_file` (for config files), caches each result for 30 seconds, and changes no checkout file.

Git writes run Git with separate argv and no shell, in the validated checkout, with hooks enabled. Commit and push run on a blocking thread because hooks and the network can be slow. Connect spawns the configured agent command with no shell, in its own process group and a dedicated empty temporary directory; Disconnect, a crash, or app exit kills the whole group and removes the directory. The staged diff leaves the machine only when the user presses Generate, to the model provider of the picked agent.

## Authorization & constraints

- Content queries are sent only when their trimmed length is at least two; the UI waits 250 ms after typing.
- Search is fixed-string and smart-case: Git gets `-i` only when the query contains no uppercase character. The query is passed as `-e <query>`.
- Search reads at most 256 KiB of Git output, returns at most 500 matches, truncates each preview to 300 characters, and reports whether output or match limits were reached.
- Go-to lookups ignore one-character names, numbers, common keywords, and names inside comments or strings, and a click in the blank space after a token does not count. Only the newest lookup may act: one superseded by a later click, or finishing after the checkout changed, is dropped. Aliases come from a single config (`extends` is not followed); packages and files outside the checkout never resolve, and declaration matching is textual, not type-aware.
- Stage and unstage take checkout-relative paths as literal pathspecs; an empty path list is rejected because `git add -A --` with no pathspec would stage everything. Before the first commit, unstage uses `git rm --cached -r`, which Git refuses when the staged content differs from the worktree.
- Commit rejects an empty or whitespace-only message, a message containing NUL, unresolved conflicts, and an index with nothing staged, before Git runs. It never passes `--no-verify` or `--amend`.
- Discard restores a tracked unstaged file from the index, or deletes an untracked regular file or symlink (the link, never its target); it rejects directories, missing paths, paths escaping the checkout through a symlinked parent, and a layer that disagrees with what the index says about the path.
- Push runs `git push` with an upstream and `git push -u origin HEAD` without one, never with `--force`, with `GIT_TERMINAL_PROMPT=0`, and is killed after 120 seconds.
- ACP: the client offers no file-system or terminal capability and passes no MCP servers; every `session/request_permission` is answered `cancelled` and any other agent request `-32601`. Connect waits at most 60 seconds for `initialize`, Generate at most 180 seconds for a reply; only `stopReason: "end_turn"` counts as an answer. A new Generate for the same checkout cancels the running one, and a cancelled or timed-out Generate cancels only its session and leaves the connection up. Generate is refused unless the agent is connected, something is staged, and no conflicts exist.
- Vim is off by default. `:w` uses the existing save checks; no other ex command is connected to saving.

## Error paths

No matches produce an empty result. Git exit code 1 with empty stderr is treated as that normal outcome. When there is no cached result, another search error renders the existing error pane; if a refresh fails while cached results exist, those results remain visible with a stale notice. A language load failure falls back to plain text. An import that resolves to no checkout file, or a lookup that fails, shows a status line under the file header that clears after four seconds; an unreadable or malformed config simply contributes no aliases. A result path that no longer exists uses the existing missing-file state, and a result line outside the current document clamps to the final line. A failed stage, unstage, or discard shows its message in an alert above the list (in the dialog for Discard) and leaves the checkout as Git left it. A failed commit or push shows Git's stderr under the message box and keeps the draft; a push over 120 seconds reports `Push timed out`. A Generate failure shows its message and leaves the draft untouched: the agent is not connected, was not found on `PATH`, exited, returned an empty reply or a stop reason other than `end_turn`, or timed out; when the agent advertised sign-in methods and `session/new` fails, the message adds a hint to sign in with that agent in a terminal. A cancelled Generate is silent, and a reply that arrives after the user edited the draft is dropped with a notice. An agent whose process dies shows status `error` ("the agent exited unexpectedly") and is not reconnected automatically.

## Tests

- Rust search tests cover tracked/untracked, ignored/binary, smart-case, literal queries, no matches, and output/preview/match caps.
- `CodeEditor` tests cover line numbers, edits, external value updates, read-only mode, filename language matching, reveal/clamping, and Vim `:w`.
- Editor workspace tests cover debouncing, the two-character threshold, line navigation, dirty-draft guarding, truncation, empty/error states, and switching back to Name mode.
- `go-to-definition` tests cover token detection, relative/extension/index/`.js` resolution, `tsconfig` paths parsing, and declaration matching and selection; `CodeEditor` tests cover the modified click, hover underline, and Vim ordering; Editor workspace tests cover opening imports and declarations, the Content fallback, notices, stale lookups, and the dirty-draft guard.
- Query and service tests cover disabled requests, query-specific caching, and the Tauri command payload.
- `git_write` tests, in temporary repositories, cover stage (new, modified, deleted, renamed), unstage with and without a first commit, discard (tracked, untracked, symlink, directory, escapes), commit (success, bad messages, conflicts, hook failure keeping the index), and push (with and without upstream, non-fast-forward, no origin) plus the `ahead` count.
- `acp` tests drive a fake ACP agent (`apps/desktop/src-tauri/tests/fixtures/fake_acp_agent.py`) for connect and timeout, per-session ids and directories, chunked and fenced replies, denied permission and unknown requests, refusal and empty replies, cancel and timeout, crashes, process-group kill, and temporary-directory cleanup; hub tests cover preset and override merging, command parsing, connect/disconnect status, prompt contents and the diff cap, and job replacement and cancel. Wiring tests check the command names and JSON shapes against the frontend types.
- `CommitBox` and Editor workspace tests cover the commit enable rules, `Cmd/Ctrl+Enter`, commit failure, push labels, Generate fill, discard-after-edit, cancel, unmount cancel, connect, agent fallback, row stage/unstage/discard buttons, and the discard dialog.

## Related

- [Architecture overview](overview.md)

## Sources

- `apps/desktop/src/App.tsx`
- `apps/desktop/src/features/editor/CodeEditor.tsx`
- `apps/desktop/src/features/editor/CommitBox.test.tsx`
- `apps/desktop/src/features/editor/CommitBox.tsx`
- `apps/desktop/src/features/editor/EditorChanges.tsx`
- `apps/desktop/src/features/editor/CodeEditor.test.tsx`
- `apps/desktop/src/features/editor/EditorFiles.tsx`
- `apps/desktop/src/features/editor/EditorSearch.tsx`
- `apps/desktop/src/features/editor/EditorWorkspace.tsx`
- `apps/desktop/src/features/editor/EditorWorkspace.test.tsx`
- `apps/desktop/src/features/editor/code-theme.ts`
- `apps/desktop/src/features/editor/go-to-definition.test.ts`
- `apps/desktop/src/features/editor/go-to-definition.ts`
- `apps/desktop/src/features/editor/queries.ts`
- `apps/desktop/src/features/editor/queries.test.tsx`
- `apps/desktop/src/features/settings/SettingsPage.tsx`
- `apps/desktop/src/queryClient.ts`
- `apps/desktop/src/services/acp.ts`
- `apps/desktop/src/services/git-view.ts`
- `apps/desktop/src/services/git-view.test.ts`
- `apps/desktop/src/services/git-write.ts`
- `apps/desktop/src/services/settings.ts`
- `apps/desktop/src/services/types.ts`
- `apps/desktop/src/stores/ui.ts`
- `apps/desktop/src-tauri/src/acp.rs`
- `apps/desktop/src-tauri/src/commands.rs`
- `apps/desktop/src-tauri/src/git_view.rs`
- `apps/desktop/src-tauri/src/git_write.rs`
- `apps/desktop/src-tauri/src/lib.rs`
- `apps/desktop/src-tauri/src/worktrees.rs`
- `apps/desktop/src-tauri/tests/fixtures/fake_acp_agent.py`
