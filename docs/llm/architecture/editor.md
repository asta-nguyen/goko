# Desktop Editor

## Requirements

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

### EDT-vim-save

The system SHALL persist the optional Vim mode under `editor.vim` and route Vim `:w` to the active editor's existing save action.

#### Scenario: Save from Vim mode

- GIVEN Vim mode is enabled on the Settings page
- WHEN the user runs `:w`
- THEN the active editor invokes the same save action as the Save button

Evidence: `apps/desktop/src/features/editor/queries.ts`, `apps/desktop/src/features/settings/SettingsPage.tsx`, `apps/desktop/src/features/editor/CodeEditor.tsx`, `apps/desktop/src/features/editor/queries.test.tsx`, `apps/desktop/src/features/editor/CodeEditor.test.tsx`

## Flow

Files mode keeps the existing filename filter and tree. Content mode trims and debounces the shared input, then `useGitSearch` keys the result by project, checkout, and query; the service invokes `git_view_search`. The Tauri command revalidates the registered checkout before `git_view::search` runs Git through the bounded runner. Selecting a result uses the Files navigation path. An already open file receives a line reveal directly; changing files goes through the shared unsaved-draft guard. CodeMirror clamps the requested line to the document, selects it, scrolls it into view, and focuses the editor. `Cmd/Ctrl+F` inside CodeMirror remains available to its search panel.

## State changes

The Vim mode switch on the Settings page reads and writes the existing settings key `editor.vim` as `"1"` or `"0"`; missing, unreadable, or other values leave Vim off. Editing continues to use the existing draft state and save path. Content search is read-only and does not change checkout files.

## Side effects

Content search starts `git grep` with argv in the validated checkout and no shell. It searches tracked and non-ignored untracked text files, skips binary files, and does not mutate Git state. Saving Vim `:w` uses the existing file-save command and its conflict and file-size checks.

## Authorization & constraints

- Content queries are sent only when their trimmed length is at least two; the UI waits 250 ms after typing.
- Search is fixed-string and smart-case: Git gets `-i` only when the query contains no uppercase character. The query is passed as `-e <query>`.
- Search reads at most 256 KiB of Git output, returns at most 500 matches, truncates each preview to 300 characters, and reports whether output or match limits were reached.
- Vim is off by default. `:w` uses the existing save checks; no other ex command is connected to saving.

## Error paths

No matches produce an empty result. Git exit code 1 with empty stderr is treated as that normal outcome. When there is no cached result, another search error renders the existing error pane; if a refresh fails while cached results exist, those results remain visible with a stale notice. A language load failure falls back to plain text. A result path that no longer exists uses the existing missing-file state, and a result line outside the current document clamps to the final line.

## Tests

- Rust search tests cover tracked/untracked, ignored/binary, smart-case, literal queries, no matches, and output/preview/match caps.
- `CodeEditor` tests cover line numbers, edits, external value updates, read-only mode, filename language matching, reveal/clamping, and Vim `:w`.
- Editor workspace tests cover debouncing, the two-character threshold, line navigation, dirty-draft guarding, truncation, empty/error states, and switching back to Name mode.
- Query and service tests cover disabled requests, query-specific caching, and the Tauri command payload.

## Related

- [Architecture overview](overview.md)

## Sources

- `apps/desktop/src/App.tsx`
- `apps/desktop/src/features/editor/CodeEditor.tsx`
- `apps/desktop/src/features/editor/CodeEditor.test.tsx`
- `apps/desktop/src/features/editor/EditorFiles.tsx`
- `apps/desktop/src/features/editor/EditorSearch.tsx`
- `apps/desktop/src/features/editor/EditorWorkspace.tsx`
- `apps/desktop/src/features/editor/EditorWorkspace.test.tsx`
- `apps/desktop/src/features/editor/code-theme.ts`
- `apps/desktop/src/features/editor/queries.ts`
- `apps/desktop/src/features/editor/queries.test.tsx`
- `apps/desktop/src/features/settings/SettingsPage.tsx`
- `apps/desktop/src/queryClient.ts`
- `apps/desktop/src/services/git-view.ts`
- `apps/desktop/src/services/git-view.test.ts`
- `apps/desktop/src/services/settings.ts`
- `apps/desktop/src/services/types.ts`
- `apps/desktop/src/stores/ui.ts`
- `apps/desktop/src-tauri/src/commands.rs`
- `apps/desktop/src-tauri/src/git_view.rs`
- `apps/desktop/src-tauri/src/lib.rs`
- `apps/desktop/src-tauri/src/worktrees.rs`
