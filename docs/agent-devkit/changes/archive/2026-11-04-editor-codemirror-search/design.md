# Editor: CodeMirror surface and content search

## Goal

Opening a text file in the Editor's Files view gives a real code editor:
syntax highlighting for the file's language, line numbers, find-in-file, and an
optional Vim mode. The Files pane can also search file contents across the
checkout and jump to the matching line.

## Scope

In scope:

- Replace the plain `<textarea>` in `EditorFiles.tsx` with a CodeMirror 6
  editor (`CodeEditor`) that keeps the existing draft, save, reload, and
  unsaved-changes behavior.
- Highlighting and line numbers for every language CodeMirror's
  `@codemirror/language-data` can match by filename, loaded lazily per file.
- Find-in-file with `Cmd/Ctrl+F` inside the editor (CodeMirror's search panel,
  including replace).
- Vim mode, off by default, toggled from the file header, remembered across
  launches, with `:w` saving the file.
- Content search: a Name/Content switch next to the Files filter; Content mode
  runs `git grep` in the checkout and lists `path:line  preview` results;
  choosing one opens the file and moves the cursor to that line.

Out of scope:

- Replacing `highlight.js` in diff, history, and stash views, or the read-only
  `CodeView` used for truncated files (both stay as they are).
- Regex or case-toggle search, multi-file replace, search in ignored or binary
  files, searching git history.
- Editor settings beyond the Vim toggle (font size, tab width, wrap, themes).
- Any change to save semantics: the 1 MiB cap, the NUL rule, the
  `expected_text` conflict check, and the unsaved-changes dialog are untouched.
- LSP, completion, linting, minimap, multiple cursors beyond CodeMirror's
  defaults.

## Behavior

**Editor surface.** For an editable file (`kind === 'text'`, not truncated) the
Files view renders `CodeEditor` instead of the `<textarea>`. It shows line
numbers and highlights by filename; unknown extensions render as plain text.
Colours come from the existing `--bg`, `--text`, `--muted`, and `--syntax-*`
tokens, so light and dark themes follow `html[data-theme]` without a rebuild.
No line wrapping, as today. The editor is read-only while a save is in flight.
Each file gets a fresh editor (its undo history does not leak across files).
When the draft text is replaced from outside (Reload from disk), the document
is replaced to match without raising a change event. Typing raises the existing
`updateText` path, so Unsaved, size, and NUL badges behave as now. `Cmd/Ctrl+S`
still saves through the existing window handler.

**Find.** `Cmd/Ctrl+F` with focus inside the editor opens CodeMirror's search
panel. With focus anywhere else in the Files view it still focuses the Files
filter, as today.

**Vim mode.** A "Vim" toggle in the file header turns on Vim keybindings in the
editor (normal/insert/visual modes, motions, `/` search, `:w` to save). The
choice is stored in the existing settings store under `editor.vim` (`"1"` or
absent/other = off) and applies to every open editor. Global single-letter
shortcuts (`j`/`k`, `1`–`4`) already ignore keystrokes from editable elements,
so they do not fire while typing in the editor.

**Content search.** In the Files pane a two-state switch `Name | Content`
(default Name) selects what the existing input searches. In Content mode, a
trimmed query of at least 2 characters, after a 250 ms pause in typing, calls
`git_view_search`. Matching is literal (not regex) and smart-case:
case-insensitive unless the query contains an uppercase letter. It covers tracked
files and untracked non-ignored files; binary files are skipped. Results show
as rows `path:line` with the matching line text (trimmed, at most 300
characters); at most 500 matches are returned and a `truncated` notice shows
when more existed. Activating a row opens that file through the same path as
the file tree (so the unsaved-changes guard applies) and moves the editor cursor
to that line, scrolled into view and selected. If the file is already open the
cursor simply moves.

## Edge cases

- Language module fails to load or no language matches: the editor stays
  usable as plain text; no error is shown.
- Opening a large file near the 1 MiB cap: CodeMirror's viewport rendering
  keeps typing responsive; the existing over-1-MiB badge still blocks save.
- Save in flight: editing is blocked (read-only) and re-enabled after success or
  failure, as the textarea's `readOnly` is today.
- Reload from disk, draft reset, or switching files while dirty: handled by the
  existing store and guard; the editor only mirrors the value it is given.
- Vim `Escape` and `:w`: `:w` calls the same save as the Save button; `:q` and
  other ex commands are not wired and do nothing.
- Vim toggle state unreadable from the settings store: Vim stays off.
- Search query that begins with `-` or contains regex characters: matched
  literally (query is passed as a `-e` argument to `git grep` with fixed-string
  matching, never through a shell).
- Search with no matches: an empty-state row; `git grep` exits with code 1 and
  empty stderr in that case, which is not an error.
- Search output exceeds 256 KiB or 500 matches: stop reading, return what was
  parsed, set `truncated`.
- Non-UTF-8 text in a result line: shown lossy-decoded, as other git views do.
- Result file deleted or renamed after the search: the file opens into the
  existing "This file is missing" pane; no crash.
- Result line beyond the document (file changed since): the cursor clamps to
  the last line.
- Fast typing: only the latest debounced query is shown; earlier in-flight
  results are discarded by the query key.
- Checkout is not a git repository or the project root is missing: the same
  errors the other `git_view_*` commands return.
- Light/dark switch while a file is open: colours update through CSS variables
  without remounting the editor.

## Interfaces

New dependencies in `apps/desktop/package.json`, pinned to the resolved version
at install time: `codemirror`, `@codemirror/state`, `@codemirror/view`,
`@codemirror/language`, `@codemirror/language-data`, `@lezer/highlight`,
`@replit/codemirror-vim`. The existing CSP (`style-src 'self' 'unsafe-inline'`,
`default-src 'self'`) already admits CodeMirror's injected styles and lazily
loaded chunks.

Frontend:

```
CodeEditor props {
  value: string; path: string; ariaLabel: string
  readOnly: boolean; vim: boolean
  reveal: { line: number; nonce: number } | null   // 1-based line; nonce re-triggers the same line
  onChange(text: string): void; onSave(): void
}
```

Tauri command `git_view_search(project_id, checkout_path, query) -> GitViewSearch`:

```
GitViewSearch { matches: { path: string; line: number; text: string }[]; truncated: boolean }
```

Touched files: `EditorFiles.tsx`, `EditorWorkspace.tsx` (the `Cmd/Ctrl+F`
handler ignores events from inside `.cm-editor`), `services/git-view.ts`,
`services/settings.ts` consumer for `editor.vim`, `queryClient.ts` (a search
key), `src-tauri/src/git_view.rs`, `commands.rs`, `lib.rs` (handler list), and
`test-setup.ts` (jsdom `Range` geometry stubs CodeMirror needs). New:
`features/editor/CodeEditor.tsx`, `features/editor/code-theme.ts`,
`features/editor/EditorSearch.tsx`. No change to the wire protocol or `gokod`.

## Verification

- Rust unit tests for `git_view::search` against a real temporary Git repo:
  tracked, untracked, ignored, and binary files; smart-case; a query starting
  with `-`; regex metacharacters; no match; output cap and match cap setting
  `truncated`; a line longer than 300 characters.
- Frontend tests: `CodeEditor` mounts with text and shows line numbers, reports
  a change when its document is edited, is read-only when told, selects a
  language by filename and falls back for an unknown one, and honors `reveal`
  including re-reveal with a new nonce and clamping; the Vim toggle reads and
  writes `editor.vim`; Content search debounces, ignores queries under 2
  characters, renders rows and the truncated notice, and opens the file at the
  line; `Cmd/Ctrl+F` from inside the editor is not intercepted while from the
  tree it still focuses the filter. Existing `EditorWorkspace` tests keep their
  textbox assertions by stubbing `CodeEditor` with a textarea.
- `cargo test --workspace`, `cargo fmt --check`, `cargo clippy --workspace -- -D
  warnings`, `pnpm -C apps/desktop typecheck`, `pnpm -C apps/desktop test`, and
  `pnpm -C apps/desktop build` (record the `dist` size change).
- Manual acceptance in the running app: open `.tsx`, `.rs`, `.md`, and an
  unknown-extension file; confirm highlighting, line numbers, `Cmd+F` find, Vim
  on/off persisting across a restart with `:w` saving, a Content search that
  opens a result at its line, and the unsaved-changes dialog when searching
  away from a dirty file.

## Related context

- [Architecture overview](../../../../llm/architecture/overview.md)
