# Editor: CodeMirror surface and content search tasks

## Design

[Editor: CodeMirror surface and content search](design.md)

## Global Constraints

- Save semantics are unchanged: 1 MiB cap (`EDITOR_FILE_CAP`), NUL rejection, `expected_text` conflict check, and the unsaved-changes dialog stay as they are; `CodeEditor` only mirrors the `value` it is given and reports edits through the existing `updateText`.
- `highlight.js`, `useHighlightedLines`, the diff/history/stash views, and the read-only `CodeView` are not modified.
- Editor colours come only from the existing CSS tokens (`--bg`, `--text`, `--muted`, `--border`, `--accent`, `--syntax-*`); no hard-coded colours, no second theme object per mode.
- Languages load lazily by filename through `@codemirror/language-data`; a load failure or unknown extension means plain text and no error UI.
- Vim mode is off by default, stored in the settings store under `editor.vim` (`"1"` = on; absent, unreadable, or any other value = off); only `:w` is wired among ex commands.
- `Cmd/Ctrl+F` from inside `.cm-editor` opens CodeMirror's search panel and is not intercepted; elsewhere it still focuses the Files filter.
- Content search is literal (`git grep -F`), smart-case (`-i` unless the query has an uppercase letter), includes tracked and untracked non-ignored files (`--untracked`), skips binary files (`-I`), passes the query as `-e <query>` with no shell, and is read-only.
- Search limits: query trimmed length >= 2, 250 ms debounce, at most 500 matches, git output read at most 256 KiB, each preview line at most 300 characters; exceeding a cap sets `truncated`. `git grep` exit code 1 with empty stderr means "no matches", not an error.
- Dependencies are exactly: `codemirror`, `@codemirror/state`, `@codemirror/view`, `@codemirror/language`, `@codemirror/language-data`, `@lezer/highlight`, `@replit/codemirror-vim`, each pinned to the version resolved at install.
- No change to `crates/protocol`, `crates/daemon`, or the CSP.

## Impact map

Entry: `apps/desktop/src/features/editor/EditorFiles.tsx::EditorFiles` (the `<textarea aria-label="Edit …">` branch and `updateText`/`selectPath`) and `apps/desktop/src-tauri/src/commands.rs::git_view_file_list` (the sibling the new search command follows)

Flow: file tree or filter → `selectPath` → `useGitFile` → `editable` → `<textarea onChange=updateText>` → `setEditorDraft` → `saveDraft` → `gitViewService.saveFile` → `git_view::save_file`. Window keydown `Cmd+S`/`Cmd+F` in `EditorWorkspace.tsx`. File names for the filter come from `useGitFileList` → `git_view::file_list`. Approved new flows: `CodeEditor` replaces the textarea; `git_view_search` → `git_view::search` → `worktrees::run_bounded`.

State changes: none in SQLite or on disk beyond the existing `settings` row `editor.vim`; no protocol or daemon change

External effects: read-only `git grep` subprocess per debounced query; new npm dependencies and lazily loaded language chunks in `dist`

Change candidates: `apps/desktop/package.json` and `pnpm-lock.yaml`; `apps/desktop/src/features/editor/CodeEditor.tsx` (new), `code-theme.ts` (new), `EditorSearch.tsx` (new), `EditorFiles.tsx`, `EditorWorkspace.tsx`; `apps/desktop/src/services/git-view.ts`; `apps/desktop/src/services/types.ts`; `apps/desktop/src/queryClient.ts`; `apps/desktop/src/features/editor/queries.ts`; `apps/desktop/src/test-setup.ts`; `apps/desktop/src-tauri/src/git_view.rs`; `apps/desktop/src-tauri/src/commands.rs`; `apps/desktop/src-tauri/src/lib.rs`; their tests; `docs/llm/architecture/overview.md`; `ARCHITECTURE.md`; `CHANGELOG.md`

Verification: `cargo test --workspace`; `cargo fmt --check`; `cargo clippy --workspace -- -D warnings`; `pnpm -C apps/desktop typecheck`; `pnpm -C apps/desktop test`; `pnpm -C apps/desktop build`; manual acceptance in the running app

Verified at: `89b2f215dc4bd4e6038fcebce01bbdd48a72ea93`

## Baseline (before implementation)

- `cargo test --workspace` — failed in the existing `a_remount_with_a_far_ahead_offset_gets_one_burst_plus_a_live_window` terminal test (`subscribe_ipc.rs`: expected 1,048,576 bytes, received 1,050,480).
- `cargo fmt --check` — failed on existing formatting in `apps/desktop/src-tauri/src/usage.rs`.
- `cargo clippy --workspace -- -D warnings` — failed on five dead-code errors in `apps/desktop/src-tauri/src/usage.rs`.
- `pnpm -C apps/desktop typecheck` — passed.
- `pnpm -C apps/desktop test` — passed (23 files, 234 tests).
- `pnpm -C apps/desktop build` — passed; baseline `apps/desktop/dist` is 1.2 MiB and Vite warns that the main chunk exceeds 500 kB.

## Tasks

### Task 1 — `CodeEditor` component and theme

Files: `apps/desktop/package.json`, `pnpm-lock.yaml`, `apps/desktop/src/features/editor/CodeEditor.tsx::CodeEditor`, `apps/desktop/src/features/editor/code-theme.ts::codeTheme`, `apps/desktop/src/test-setup.ts`
Interfaces: produces `CodeEditor` with the props in the design's Interfaces section, used by Tasks 2, 3, and 6.
Change: install the dependencies; `CodeEditor` builds one `EditorView` per mount from `basicSetup` (line numbers, history, search keymap, bracket matching) with compartments for language, read-only, and vim; `code-theme.ts` maps editor chrome and `@lezer/highlight` tags to the existing CSS tokens; language is resolved with `LanguageDescription.matchFilename(languages, path)` and loaded asynchronously, falling back to plain on none or failure; an effect applies a changed `value` to the document only when it differs from the current text, tagged so it does not call `onChange`; `readOnly` and `ariaLabel` (content attribute `aria-label`) are applied; `reveal` moves the cursor to the clamped 1-based line, selects it, and scrolls it into view, re-running when `nonce` changes. `test-setup.ts` stubs the `Range` geometry methods CodeMirror measures in jsdom.
Verify: Vitest cases for `CodeEditor` — mounting `value="a\nb"` shows two `.cm-line` elements and a line-number gutter; dispatching an edit calls `onChange` with the new text; changing `value` from outside updates the document and does not call `onChange`; `readOnly` blocks `view.dispatch` user input (state `readOnly` facet true); `path="x.ts"` resolves a TypeScript language and `path="x.unknownext"` resolves none without throwing; `reveal={{line: 99, nonce: 1}}` on a 3-line document selects line 3, and a new nonce for the same line re-selects it. `pnpm -C apps/desktop test` and `pnpm -C apps/desktop typecheck` pass.
Progress: done
Result: `pnpm -C apps/desktop test` — 24 files/240 tests passed; `pnpm -C apps/desktop typecheck` — passed.

### Task 2 — Use `CodeEditor` in the Files view and fix `Cmd+F`

Files: `apps/desktop/src/features/editor/EditorFiles.tsx::EditorFiles`, `apps/desktop/src/features/editor/EditorWorkspace.tsx` (the window `keydown` handler), `apps/desktop/src/features/editor/EditorWorkspace.test.tsx`
Interfaces: consumes `CodeEditor` from Task 1; `updateText` and `saveDraft` are passed as `onChange`/`onSave`; `readOnly={saving}`; `ariaLabel={`Edit ${path}`}`; `vim` is `false` until Task 3; `reveal` is `null` until Task 6.
Change: replace the `<textarea>` branch with `<CodeEditor key={`${checkoutPath}:${path}`} …>`; in the `Cmd/Ctrl+F` branch of the `keydown` handler return without `preventDefault` when `event.target` is inside `.cm-editor`. Existing `EditorWorkspace` tests that look up the `textbox` named `Edit <path>` keep working by mocking the `CodeEditor` module with a textarea stand-in that has the same label, `value`, `readOnly`, and `onChange`.
Verify: the five existing textbox-based tests in `EditorWorkspace.test.tsx` pass unchanged in their assertions; a new test shows `Cmd+F` dispatched from an element inside `.cm-editor` is not `defaultPrevented` and does not move focus to the filter, while `Cmd+F` from the file tree still focuses the filter; a new test with the real `CodeEditor` shows an editable file renders `.cm-editor` and a read-only (truncated) file still renders `CodeView`. `pnpm -C apps/desktop test` passes.
Progress: done
Result: `pnpm -C apps/desktop test` — 24 files/241 tests passed; `pnpm -C apps/desktop typecheck` — passed.

### Task 3 — Vim mode toggle

Files: `apps/desktop/src/features/editor/CodeEditor.tsx` (vim compartment, `:w` hook), `apps/desktop/src/features/editor/EditorFiles.tsx` (header toggle), `apps/desktop/src/features/editor/queries.ts::useEditorVim`
Interfaces: consumes `settingsService.get/set` (`editor.vim`); feeds `vim` into `CodeEditor`.
Change: `useEditorVim` reads `editor.vim` through TanStack Query and exposes `[enabled, setEnabled]`, writing `"1"` or `"0"`; any read failure yields off. The file header gets a `Vim` toggle (accessible name `Vim mode`, pressed state reflects the setting). `CodeEditor` enables `@replit/codemirror-vim` through its compartment when `vim` is true and defines the ex command `write`/`w` once at module level to call the mounted editor's latest `onSave`.
Verify: Vitest — with `settings.get` mocked to `"1"` the toggle renders pressed and `CodeEditor` receives `vim`; toggling calls `settings.set('editor.vim','1')` then `'0'`; a rejected `settings.get` renders unpressed; in `CodeEditor`, with `vim` true, the vim extension is active (`getCM(view)` is non-null) and invoking the `:w` ex command calls `onSave` once; with `vim` false `getCM(view)` is null. `pnpm -C apps/desktop test` passes.
Progress: done
Result: `pnpm -C apps/desktop test` — 24 files/245 tests passed; `pnpm -C apps/desktop typecheck` — passed.

### Task 4 — Backend `git grep` search

Files: `apps/desktop/src-tauri/src/git_view.rs::search`, `apps/desktop/src-tauri/src/git_view.rs::GitViewSearch`, `apps/desktop/src-tauri/src/commands.rs::git_view_search`, `apps/desktop/src-tauri/src/lib.rs::handler`
Interfaces: produces Tauri command `git_view_search(project_id, checkout_path, query) -> GitViewSearch` for Task 5; reuses `git_view_checkout` for validation and `worktrees::run_bounded` with a 256 KiB limit.
Change: build `git grep` args `["grep","-n","-I","-F","--untracked","--no-color","-z", ("-i" unless the query contains an uppercase letter), "-e", query]`; run bounded; exit code 1 with empty stderr returns an empty result; other failures return the usual git error; parse records into `{path,line,text}` (format confirmed by the test), cut `text` to 300 characters on a character boundary, stop at 500 matches, and set `truncated` when the output limit or the match cap was hit. A query shorter than 1 character after trimming is rejected as invalid input at the command boundary (the UI enforces 2).
Verify: Rust tests in `git_view.rs` using a temporary repo (existing helpers) — tracked and untracked files match, a `.gitignore`d file and a binary file do not; `Foo` is case-sensitive while `foo` also matches `Foo`; query `-n` and query `a.b*` match literally; no match returns an empty list without error; 600 matching lines return 500 with `truncated`; a 1000-character line returns 300 characters; a non-repo checkout returns the existing repo error. `cargo test -p desktop git_view` passes, then `cargo fmt --check` and `cargo clippy --workspace -- -D warnings`.
Progress: done
Result: `cargo test -p desktop git_view` — 19 passed; changed-file `rustfmt --edition 2021 --check` and `cargo fmt --check` — passed. Workspace Clippy still reports two `unnecessary_map_or` errors in `apps/desktop/src-tauri/src/usage.rs`.

### Task 5 — Frontend search service and query

Files: `apps/desktop/src/services/git-view.ts::gitViewService.search`, `apps/desktop/src/services/types.ts::GitViewSearch`, `apps/desktop/src/queryClient.ts::queryKeys.gitSearch`, `apps/desktop/src/features/editor/queries.ts::useGitSearch`
Interfaces: consumes `git_view_search` from Task 4; produces `useGitSearch(projectId, checkoutPath, query, enabled)` for Task 6.
Change: add the typed `search` call, the `GitViewSearch` type, a `gitSearch(projectId, checkoutPath, query)` key, and a hook that keeps previous results while a new query loads and does not retain results forever (`gcTime` short).
Verify: Vitest — `gitViewService.search` calls `invoke('git_view_search', {projectId, checkoutPath, query})`; `useGitSearch` with `enabled=false` makes no call; a changed query issues a new call keyed separately. `pnpm -C apps/desktop test` and `pnpm -C apps/desktop typecheck` pass.
Progress: done
Result: `gitViewService.search` payload, disabled query, and query re-key/cache tests pass as part of `pnpm -C apps/desktop test`; `pnpm -C apps/desktop typecheck` — passed.

### Task 6 — Content search UI and jump to line

Files: `apps/desktop/src/features/editor/EditorSearch.tsx::EditorSearch`, `apps/desktop/src/features/editor/EditorFiles.tsx::EditorFiles` (mode switch, `reveal` state, `selectPath` reuse)
Interfaces: consumes `useGitSearch` (Task 5) and `CodeEditor.reveal` (Task 1); keeps the existing Name filter and file tree behavior untouched in Name mode.
Change: add a `Name | Content` switch (`ToggleGroup`, default Name) beside `FilterInput`; in Content mode the input value is debounced 250 ms and queries of fewer than 2 trimmed characters make no request; `EditorSearch` renders rows `path:line` with the preview text, an empty state, an error state using the existing `ErrorPane`, and a `Showing first 500 matches` notice when `truncated`; activating a row calls the same `selectPath` flow as the file tree (so the unsaved guard applies) and then sets `reveal = {line, nonce: nonce + 1}`; when the path is already open only `reveal` changes.
Verify: Vitest with a mocked `gitViewService` — typing `a` makes no call; `ab` makes exactly one call after the debounce; results render as rows; clicking a row for the open file updates `reveal` without reloading the file; clicking a row for another file while the draft is dirty shows the unsaved-changes dialog and leaves the path unchanged until Save or Discard; `truncated` shows the notice; zero matches shows the empty state; a rejected call shows the error pane; switching back to Name restores the filtered file list. `pnpm -C apps/desktop test` passes.
Progress: done
Result: `EditorWorkspace.test.tsx` — 33 passed, including debounce threshold/timing, line reveal, dirty navigation guard, truncation, empty/error states, and returning to Name filtering.

### Task 7 — Documentation and acceptance

Files: `docs/llm/architecture/overview.md` (via `document-wiki` `## Task updates`), `ARCHITECTURE.md`, `CHANGELOG.md`
Interfaces: documents Tasks 1–6.
Change: update the Desktop UI and Desktop bridge domain rows (CodeMirror editor surface, Vim setting, `git_view_search`), the Editor section of `ARCHITECTURE.md`, and add `CHANGELOG.md` `Added`/`Changed` entries. Run the full checks and the manual acceptance from the design.
Verify: `cargo test --workspace`, `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `pnpm -C apps/desktop typecheck`, `pnpm -C apps/desktop test`, and `pnpm -C apps/desktop build` pass, with the `dist` size before and after recorded in the Result. Manual: `.tsx`, `.rs`, `.md`, and unknown-extension files show the expected highlighting (plain for unknown) and line numbers; `Cmd+F` inside the editor opens the search panel; Vim on/off persists across an app restart and `:w` saves; Content search opens a result at its line; searching away from a dirty file shows the unsaved dialog; switching light/dark updates editor colours live. Record any item that could not be observed as `cannot verify`.
Progress: done
Result: `cargo test --workspace` — 250 passed on rerun (first run hit the existing remount snapshot assertion in `crates/daemon/tests/subscribe_ipc.rs`); `cargo build --workspace`, `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `pnpm -C apps/desktop typecheck`, `pnpm -C apps/desktop test` (29 files/267 tests), and `pnpm -C apps/desktop build` — passed. `apps/desktop/dist`: 1.2 MiB baseline → 3.1 MiB. Manual acceptance passed for `.tsx`, `.rs`, `.md`, and unknown-extension editing, line numbers and syntax colors, in-editor `Cmd+F`, Vim persistence and `:w` save, content search with line reveal, dirty navigation guard, and live theme switching.

## Files inspected, no change

- `apps/desktop/src/features/editor/highlight.ts` — highlight.js language table and the 4000-line cap; stays for diffs.
- `apps/desktop/src/features/editor/panes.tsx` — `CodeView`, `Highlighted`, `FilterInput`, `matchesQuery`; reused or left alone.
- `apps/desktop/src-tauri/src/worktrees.rs::run_bounded` — the bounded git runner the search reuses.
- `apps/desktop/src-tauri/tauri.conf.json` — CSP already allows CodeMirror's inline styles and same-origin chunks.
- `apps/desktop/src/services/settings.ts` — existing `get`/`set` used for `editor.vim`.
- `docs/agent-devkit/changes/archive/2026-11-04-agent-usage-limits/` — a separate change, now archived; no shared files except `ARCHITECTURE.md`, `CHANGELOG.md`, `docs/llm/architecture/overview.md`, and `lib.rs`, so whichever lands second rebases those.

## Approval Gate

Status: approved

## Decision Log

None.
