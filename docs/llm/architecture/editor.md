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

Evidence: `crates/app-core/src/acp.rs`, `apps/desktop/src/features/editor/CommitBox.tsx`, `apps/desktop/src/features/editor/CommitBox.test.tsx`

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

Evidence: `apps/desktop/src/features/editor/EditorSearch.tsx`, `apps/desktop/src/features/editor/EditorFiles.tsx`, `apps/desktop/src/features/editor/EditorWorkspace.test.tsx`, `crates/app-core/src/git_view.rs`

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

Evidence: `crates/app-core/src/git_write.rs`, `apps/desktop/src/features/editor/EditorChanges.tsx`, `apps/desktop/src/features/editor/EditorWorkspace.test.tsx`

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

### EDT-history-branch-filter

The system SHALL let the user limit History to one local or remote-tracking branch, or show all branches, accepting only a full branch ref that the checkout's branch list returned.

#### Scenario: Show one branch

- GIVEN the checkout has branches `main` and `feature`
- WHEN the user chooses `feature` in the branch dropdown
- THEN only commits reachable from `feature` are listed and the selection resets

#### Scenario: Unknown branch rejected

- GIVEN a request names a ref that is not under refs/heads or refs/remotes
- WHEN the backend receives it
- THEN it returns an invalid-input error and runs no `git log`

Evidence: `crates/app-core/src/git_view.rs`, `apps/desktop/src/features/editor/EditorHistory.tsx`, `apps/desktop/src/features/editor/queries.ts`, `apps/desktop/src/features/editor/EditorWorkspace.test.tsx`

### EDT-history-detail

The system SHALL show the selected commit's author with email, date, committer when it differs, parents, full message body, changed files with added and removed line counts, and its patch.

#### Scenario: Select a commit

- GIVEN a commit with a two-paragraph message that changed a text file and a binary file
- WHEN the user selects it
- THEN the panel shows both paragraphs, the text file with `+added −removed`, the binary file without counts, and the patch

#### Scenario: Jump to a parent

- GIVEN the selected commit's parent is loaded
- WHEN the user activates the parent's short hash
- THEN the parent becomes the selected commit

Evidence: `apps/desktop/src/features/editor/CommitDetail.tsx`, `apps/desktop/src/features/editor/EditorHistory.tsx`, `apps/desktop/src/features/editor/panes.tsx`, `crates/app-core/src/git_view.rs`, `apps/desktop/src/features/editor/CommitDetail.test.tsx`, `apps/desktop/src/features/editor/EditorWorkspace.test.tsx`

### EDT-history-table

The system SHALL show the Editor's History as a one-line-per-commit table with a branch-coloured commit graph, a distinct HEAD node, and Graph, Description, Date, Author, and Commit columns whose widths and the table/detail divider the user can drag and the app restores on the next run.

#### Scenario: Resize a column and restore it

- GIVEN the History table is open
- WHEN the user drags the Author column edge wider and the app is reopened
- THEN the Author column opens at the dragged width

#### Scenario: Filter keeps the graph

- GIVEN several commits are loaded
- WHEN the user types a filter that matches only some of them
- THEN every row and the whole graph stay visible, the non-matching rows are dimmed, and Enter selects the next match

#### Scenario: Uncommitted changes row

- GIVEN the checkout has uncommitted changes and HEAD is loaded
- WHEN History opens with All branches selected
- THEN an "Uncommitted changes" row sits above HEAD and offers to open the Changes tab

#### Scenario: Parents follow children

- GIVEN commits with skewed or equal commit timestamps
- WHEN History loads them
- THEN every parent is listed after its children so the graph lanes connect

Evidence: `apps/desktop/src/features/editor/EditorHistory.tsx`, `apps/desktop/src/features/editor/HistoryTable.tsx`, `apps/desktop/src/features/editor/CommitDetail.tsx`, `apps/desktop/src/features/editor/history-graph.ts`, `apps/desktop/src/features/editor/history-layout.ts`, `apps/desktop/src/components/ui/resizable-handle.tsx`, `apps/desktop/src/features/editor/HistoryTable.test.tsx`, `apps/desktop/src/features/editor/history-graph.test.ts`, `apps/desktop/src/features/editor/history-layout.test.ts`, `apps/desktop/src/features/editor/EditorWorkspace.test.tsx`, `crates/app-core/src/git_view.rs`

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

Evidence: `crates/app-core/src/git_write.rs`, `crates/app-core/src/worktrees.rs`, `apps/desktop/src/features/editor/CommitBox.tsx`, `apps/desktop/src/features/editor/CommitBox.test.tsx`

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

Evidence: `crates/app-core/src/git_write.rs`, `apps/desktop/src/features/editor/CommitBox.tsx`, `apps/desktop/src/features/editor/EditorChanges.tsx`, `apps/desktop/src/features/editor/CommitBox.test.tsx`, `apps/desktop/src/features/editor/EditorWorkspace.test.tsx`

### EDT-terminal-file-link

The system SHALL, on Cmd+click (macOS) or Ctrl+click (other platforms) on a file path printed in a session's terminal output that resolves to a file in that session's checkout, switch to the Editor's Files view for that checkout and open the file, revealing the printed line when the path carries `:line`.

#### Scenario: Open a printed path at its line

- GIVEN a session in a Git checkout prints `src/App.tsx:10` and `src/App.tsx` is in the checkout's file list
- WHEN the user Cmd/Ctrl+clicks that text
- THEN the Editor's Files view opens `src/App.tsx` with line 10 revealed

#### Scenario: Unresolved or unmodified click

- GIVEN a printed path names no file of the checkout, or the user clicks without Cmd/Ctrl
- WHEN the pointer is over the text or the user clicks it
- THEN an unresolved path is not underlined and nothing opens, and a click without Cmd/Ctrl never opens a file

Evidence: `apps/desktop/src/features/terminal/terminal-links.ts`, `apps/desktop/src/features/terminal/TerminalController.ts`, `apps/desktop/src/features/terminal/TerminalView.tsx`, `apps/desktop/src/stores/ui.ts`, `apps/desktop/src/features/editor/EditorWorkspace.tsx`, `apps/desktop/src/features/editor/EditorFiles.tsx`, `apps/desktop/src/features/terminal/terminal-links.test.ts`, `apps/desktop/src/features/editor/EditorWorkspace.test.tsx`

### EDT-vim-save

The system SHALL persist the optional Vim mode under `editor.vim` and route Vim `:w` to the active editor's existing save action.

#### Scenario: Save from Vim mode

- GIVEN Vim mode is enabled on the Settings page
- WHEN the user runs `:w`
- THEN the active editor invokes the same save action as the Save button

Evidence: `apps/desktop/src/features/editor/queries.ts`, `apps/desktop/src/features/settings/SettingsPage.tsx`, `apps/desktop/src/features/editor/CodeEditor.tsx`, `apps/desktop/src/features/editor/queries.test.tsx`, `apps/desktop/src/features/editor/CodeEditor.test.tsx`

## Flow

The Git, file, storage, and ACP operations described below are implemented in
`crates/app-core`, a Tauri-free crate shared with the native Rune editor (see
[Rune editor](../domains/rune-editor.md)); the `git_view_*`, `git_write_*`, and
`acp_*` Tauri commands in `apps/desktop/src-tauri/src/commands.rs` open the app
store, revalidate the registered checkout through `app-core`'s project rules,
and delegate to those shared implementations with unchanged names and payloads.

Files mode keeps the existing filename filter and tree. Content mode trims and debounces the shared input, then `useGitSearch` keys the result by project, checkout, and query; the service invokes `git_view_search`. The Tauri command revalidates the registered checkout before `git_view::search` runs Git through the bounded runner. Selecting a result uses the Files navigation path. An already open file receives a line reveal directly; changing files goes through the shared unsaved-draft guard. CodeMirror clamps the requested line to the document, selects it, scrolls it into view, and focuses the editor. `Cmd/Ctrl+F` inside CodeMirror remains available to its search panel.

Terminal file links arrive as one-shot open requests scoped to the session's checkout. `EditorWorkspace` switches to Files only for its matching checkout; `EditorFiles` selects the requested path through the same unsaved-draft guard as other navigation, then reveals the requested line. The request is cleared after handling, when its checkout cannot be shown, or when the Editor unmounts. See [Terminal links](overview.md#terminal-links) for URL opening and path resolution.

Cmd+click (macOS) or Ctrl+click (other platforms) on a token in the Files editor starts a lookup; a modified click is handled ahead of Vim, and while the modifier is held the token under the pointer is underlined. An import specifier (`from`, `import`, `require` strings) is resolved against the checkout's file list: relative paths try the extensions `.ts .tsx .js .jsx .mjs .cjs .json` and `index.*`, a `.js` specifier also tries its TypeScript source, and other specifiers try the `compilerOptions.paths` aliases of the nearest `tsconfig.json` or `jsconfig.json` that defines any. A hit opens through the Files navigation path. Any other name of two or more characters outside a comment or string is searched with the content search; its declaration lines are picked by keyword and shape patterns (JS/TS, Python, Rust, Go). The first declaration in the open file wins, otherwise a single declaration in the checkout opens at its line, and anything else switches the sidebar to Content mode with the name as the query.

The Changes list writes through `git_write_*` commands. Row buttons Stage (`+`, on Unstaged, Untracked, and Conflicts rows), Unstage (`−`, on Staged rows), and Discard (trash, on Unstaged and Untracked rows only) act on one path without selecting the row; group headers add "Unstage all" (no paths) and "Stage all" (that group's file paths, so the other group is untouched). Discard opens a confirmation dialog naming the file and saying it cannot be undone; Cancel changes nothing. Each command revalidates the registered checkout before `git_write` runs Git, and a successful write invalidates the checkout's changes query and its diffs.

The commit box under the list holds the message draft, an agent picker with each agent's status, a Connect button, Generate/Cancel, Commit, and Push. Commit runs `git commit -m <message>` and, on success, clears the draft and shows `Committed <hash> on <branch>`. Push shows `Push ↑N` when `GitViewChanges.ahead` (from `git rev-list --count @{u}..HEAD`) is a positive number, `Push new branch` when it is `null` (no upstream), and is disabled at 0. The box is also shown on a clean tree so commits ahead of the upstream stay pushable.

Generate calls `acp_generate_commit_message`. The picked agent is the stored `commit.agent` while that agent stays enabled, else the first connected one, else the first enabled one found on `PATH`. The backend takes the agent's live connection from the ACP hub, builds the prompt (the agent's commit prompt or the default instructions, the last five commit subjects, `git diff --cached --stat`, and `git diff --cached` capped at 120 KiB with a `(diff truncated)` line), opens a new session in a fresh empty temporary directory, and returns the cleaned reply. The reply fills the draft only when the user has not typed since pressing Generate. Connect and Disconnect call `acp_connect` and `acp_disconnect`; the status map from `acp_status` is polled every 3 seconds while the Changes view or the ACP settings section is visible.

History loads commit pages through `useGitHistory`, keyed by project, checkout, and the chosen branch (`All branches` is the default), plus `useGitBranches` for the dropdown and `useGitChanges` for the Uncommitted row. `EditorHistory` owns the selection, filter, branch, and the single `useHistoryLayout` instance; it passes commits to `HistoryTable`, which lays the lanes out with `layoutGraph` and draws one SVG next to 28 px rows. A commit selection requests the existing `commit` diff, whose reply now carries the message body, author email, committer, and per-file added/removed counts for `CommitDetail`. A vertical `ResizableHandle` splits the table from the detail. The branch dropdown, the `N matches` count, and Enter/Shift+Enter match jumping, Jump to HEAD, and Refresh live in the container; arrow keys move focus between rows without selecting; right-click on a row copies its hash or subject.

## State changes

The Vim mode switch on the Settings page reads and writes the existing settings key `editor.vim` as `"1"` or `"0"`; missing, unreadable, or other values leave Vim off. Editing continues to use the existing draft state and save path. Content search is read-only and does not change checkout files.

Staging, unstaging, discarding, committing, and pushing change the checkout's index, working tree, HEAD, and remote refs. The commit box's agent pick is stored under the settings key `commit.agent`; the agent list and overrides live under `acp.agents` (see [Desktop Settings](../domains/desktop-settings.md#set-acp-agents)). ACP connections and the running Generate jobs (one per project and checkout) are in-memory state in the backend hub and are not persisted.

History keeps the selected commit, the Uncommitted selection, the filter, and the branch in memory only; changing the branch clears the selection and the filter and restarts paging. The column widths and the detail height are the only persisted History state, stored in the browser's `localStorage` under `goko.history.layout`; a chosen Uncommitted row that disappears stops counting as chosen, and the HEAD or first commit is selected instead.

## Side effects

Content search starts `git grep` with argv in the validated checkout and no shell. It searches tracked and non-ignored untracked text files, skips binary files, and does not mutate Git state. Saving Vim `:w` uses the existing file-save command and its conflict and file-size checks. A go-to lookup is read-only: it reuses `git_view_file_list`, `git_view_search`, and `git_view_file` (for config files), caches each result for 30 seconds, and changes no checkout file.

Git writes run Git with separate argv and no shell, in the validated checkout, with hooks enabled. Commit and push run on a blocking thread because hooks and the network can be slow. Connect spawns the configured agent command with no shell, in its own process group and a dedicated empty temporary directory; Disconnect, a crash, or app exit kills the whole group and removes the directory. The staged diff leaves the machine only when the user presses Generate, to the model provider of the picked agent.

History reads run `git log`, `git for-each-ref`, `git symbolic-ref`, `git check-ref-format`, `git rev-parse`, `git diff --numstat -z` (or `git diff-tree --root` for a root commit), and `git log -1` for the message body, all with argv in the validated checkout and no shell; none changes Git state. Copy hash, Copy subject, and the detail copy button write to the clipboard on a user action; a missing or rejecting clipboard only shows "Copy failed" in the detail or is ignored in the menu.

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
- A History branch is accepted only as a full ref under `refs/heads/` or `refs/remotes/` with no NUL or leading `-`, a valid ref name according to `git check-ref-format`, and a commit target; revision syntax such as `main~1` or `main@{0}` is rejected as invalid input before `git log` runs, and the ref is passed after `--end-of-options`.
- The default History scope is local branches, remote-tracking refs, and tags, in pages of 100 commits; `refs/remotes/*/HEAD` aliases are not offered in the dropdown.
- History walks `git log --topo-order`, so a parent never lists before its children on any page; the graph lanes rely on that order.
- Stored layout values are validated and clamped: Graph 48–320 px, Date/Author/Commit 56–360 px, detail height at least 160 px (the table keeps at least 120 px); an invalid or unreadable value falls back to the defaults (Date 120, Author 128, Commit 80, detail 320 px).
- Numstat counts are shown only for text files; a binary file shows none, and a merge commit's files and patch are against its first parent.

## Error paths

No matches produce an empty result. Git exit code 1 with empty stderr is treated as that normal outcome. When there is no cached result, another search error renders the existing error pane; if a refresh fails while cached results exist, those results remain visible with a stale notice. A language load failure falls back to plain text. An import that resolves to no checkout file, or a lookup that fails, shows a status line under the file header that clears after four seconds; an unreadable or malformed config simply contributes no aliases. A result path that no longer exists uses the existing missing-file state, and a result line outside the current document clamps to the final line. A failed stage, unstage, or discard shows its message in an alert above the list (in the dialog for Discard) and leaves the checkout as Git left it. A failed commit or push shows Git's stderr under the message box and keeps the draft; a push over 120 seconds reports `Push timed out`. A Generate failure shows its message and leaves the draft untouched: the agent is not connected, was not found on `PATH`, exited, returned an empty reply or a stop reason other than `end_turn`, or timed out; when the agent advertised sign-in methods and `session/new` fails, the message adds a hint to sign in with that agent in a terminal. A cancelled Generate is silent, and a reply that arrives after the user edited the draft is dropped with a notice. An agent whose process dies shows status `error` ("the agent exited unexpectedly") and is not reconnected automatically.

A History page that fails with no cached data shows the existing error pane with Retry while the toolbar stays usable, so choosing All branches recovers from a vanished branch; a failed refresh keeps the table behind the stale notice. A commit whose diff fails shows the error pane in the detail only. A root commit has no parents and no incoming graph line, and a parent outside the loaded pages leaves an open line and a disabled parent button. A blocked `localStorage` keeps the default layout for that run.

## Tests

- Rust search tests cover tracked/untracked, ignored/binary, smart-case, literal queries, no matches, and output/preview/match caps.
- `CodeEditor` tests cover line numbers, edits, external value updates, read-only mode, filename language matching, reveal/clamping, and Vim `:w`.
- Editor workspace tests cover debouncing, the two-character threshold, line navigation, dirty-draft guarding, truncation, empty/error states, and switching back to Name mode.
- `go-to-definition` tests cover token detection, relative/extension/index/`.js` resolution, `tsconfig` paths parsing, and declaration matching and selection; `CodeEditor` tests cover the modified click, hover underline, and Vim ordering; Editor workspace tests cover opening imports and declarations, the Content fallback, notices, stale lookups, and the dirty-draft guard.
- Query and service tests cover disabled requests, query-specific caching, and the Tauri command payload.
- `git_write` tests, in temporary repositories, cover stage (new, modified, deleted, renamed), unstage with and without a first commit, discard (tracked, untracked, symlink, directory, escapes), commit (success, bad messages, conflicts, hook failure keeping the index), and push (with and without upstream, non-fast-forward, no origin) plus the `ahead` count.
- `acp` tests drive a fake ACP agent (`crates/app-core/tests/fixtures/fake_acp_agent.py`) for connect and timeout, per-session ids and directories, chunked and fenced replies, denied permission and unknown requests, refusal and empty replies, cancel and timeout, crashes, process-group kill, and temporary-directory cleanup; hub tests cover preset and override merging, command parsing, connect/disconnect status, prompt contents and the diff cap, and job replacement and cancel. Wiring tests check the command names and JSON shapes against the frontend types.
- `CommitBox` and Editor workspace tests cover the commit enable rules, `Cmd/Ctrl+Enter`, commit failure, push labels, Generate fill, discard-after-edit, cancel, unmount cancel, connect, agent fallback, row stage/unstage/discard buttons, and the discard dialog.
- Rust `git_view` tests cover branch-scoped history, rejected unsafe branches (including revision syntax), the branch list with the current flag, the commit body, email, and committer, numstat for text, binary, renamed, and root commits, and children-before-parents ordering across History pages; the wiring test checks the command names and wire keys.
- `history-graph`, `history-layout`, and `resizable-handle` tests cover colour continuity and lane reuse, clamped and failing storage, keyboard steps, drag clamping, and double-click reset; `HistoryTable` and `CommitDetail` tests cover rows, dimming, keyboard focus, paging, the context menu, copy feedback, parents, the collapsed body, and counts; Editor workspace tests cover the branch dropdown, match jumping, Jump to HEAD, the Uncommitted row and its fallback, and stale refreshes.
- Terminal controller and view tests cover modified-click activation, resolved checkout paths, lazy cached file-list reads, file-list failures that leave URL links available, and the Editor open request; Editor workspace tests cover one-shot consumption, line reveal, checkout matching, and the existing draft-navigation guard.

## Related

- [Architecture overview](overview.md)

## Sources

- `apps/desktop/src-tauri/src/commands.rs`
- `apps/desktop/src-tauri/src/lib.rs`
- `apps/desktop/src-tauri/src/wiring_tests.rs`
- `apps/desktop/src/App.tsx`
- `apps/desktop/src/components/ui/context-menu.tsx`
- `apps/desktop/src/components/ui/resizable-handle.test.tsx`
- `apps/desktop/src/components/ui/resizable-handle.tsx`
- `apps/desktop/src/features/editor/CodeEditor.test.tsx`
- `apps/desktop/src/features/editor/CodeEditor.tsx`
- `apps/desktop/src/features/editor/CommitBox.test.tsx`
- `apps/desktop/src/features/editor/CommitBox.tsx`
- `apps/desktop/src/features/editor/CommitDetail.test.tsx`
- `apps/desktop/src/features/editor/CommitDetail.tsx`
- `apps/desktop/src/features/editor/EditorChanges.tsx`
- `apps/desktop/src/features/editor/EditorFiles.tsx`
- `apps/desktop/src/features/editor/EditorHistory.tsx`
- `apps/desktop/src/features/editor/EditorSearch.tsx`
- `apps/desktop/src/features/editor/EditorWorkspace.test.tsx`
- `apps/desktop/src/features/editor/EditorWorkspace.tsx`
- `apps/desktop/src/features/editor/HistoryTable.test.tsx`
- `apps/desktop/src/features/editor/HistoryTable.tsx`
- `apps/desktop/src/features/editor/code-theme.ts`
- `apps/desktop/src/features/editor/go-to-definition.test.ts`
- `apps/desktop/src/features/editor/go-to-definition.ts`
- `apps/desktop/src/features/editor/history-graph.test.ts`
- `apps/desktop/src/features/editor/history-graph.ts`
- `apps/desktop/src/features/editor/history-layout.test.ts`
- `apps/desktop/src/features/editor/history-layout.ts`
- `apps/desktop/src/features/editor/panes.tsx`
- `apps/desktop/src/features/editor/queries.test.tsx`
- `apps/desktop/src/features/editor/queries.ts`
- `apps/desktop/src/features/settings/SettingsPage.tsx`
- `apps/desktop/src/features/terminal/TerminalController.test.ts`
- `apps/desktop/src/features/terminal/TerminalController.ts`
- `apps/desktop/src/features/terminal/TerminalView.test.tsx`
- `apps/desktop/src/features/terminal/TerminalView.tsx`
- `apps/desktop/src/features/terminal/TerminalWorkspace.tsx`
- `apps/desktop/src/features/terminal/terminal-links.test.ts`
- `apps/desktop/src/features/terminal/terminal-links.ts`
- `apps/desktop/src/queryClient.ts`
- `apps/desktop/src/services/acp.ts`
- `apps/desktop/src/services/git-view.test.ts`
- `apps/desktop/src/services/git-view.ts`
- `apps/desktop/src/services/git-write.ts`
- `apps/desktop/src/services/settings.ts`
- `apps/desktop/src/services/types.ts`
- `apps/desktop/src/stores/ui.ts`
- `crates/app-core/src/acp.rs`
- `crates/app-core/src/git_view.rs`
- `crates/app-core/src/git_write.rs`
- `crates/app-core/src/projects.rs`
- `crates/app-core/src/store.rs`
- `crates/app-core/src/agents.rs`
- `crates/app-core/src/worktrees.rs`
- `crates/app-core/tests/fixtures/fake_acp_agent.py`
