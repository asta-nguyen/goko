# Rune editor

The `goko-rune` app's Editor view (`apps/rune`, Rune/wgpu/winit) is a native
workspace over real registered projects and checkouts — not the terminal
chrome described in [Rune chrome](rune-chrome.md). It offers five sections —
Files, Changes, History, Stashes, and Servers (language servers) — backed by
`crates/app-core`, the same Tauri-free Git/file/ACP/LSP/storage implementation
the desktop commands call. All blocking Git, filesystem, storage, and
highlighting work runs on one worker thread; the UI thread only enqueues jobs
and applies replies. Editing uses rune-kit's native text surface, not
CodeMirror or a WebView.

Scope boundaries, verified against current source:

- The terminal and session chrome drives real daemon sessions
  (`apps/rune/src/data.rs`, `term.rs`, `panes.rs`); the editor never reads
  session state and no live native terminal/session producer exists.
- Vim is deferred: the shared `editor.vim` setting still drives the Tauri
  editor only; Rune exposes no Vim control.
- Language-server support covers hover and go-to-definition only;
  diagnostics and completion are deferred (see RNE-language-servers).
- External open-file/line requests have a tested consumer but no producer yet
  (see RNE-external-open).
- Inline blame is a Rune-only affordance: the desktop editor has no blame
  column, and the shared read lives in `crates/app-core/src/git_view.rs`
  (see RNE-inline-blame).

## Requirements

### RNE-acp-commit-message

The system SHALL generate a commit message only when the user presses Generate: the picked, connected local ACP agent receives the staged diff in a new session whose cwd is an empty temporary directory with permission requests denied, and its reply fills the commit draft only when the draft is unchanged since Generate was pressed; generation never commits.

#### Scenario: Generate a message

- GIVEN a checkout with staged changes and a connected agent picked in the commit panel
- WHEN the user presses Generate and the agent finishes its turn
- THEN the cleaned reply fills the commit message draft and no commit is created

#### Scenario: Draft edited or cancelled

- GIVEN a generation is running
- WHEN the user edits the draft, presses Cancel, or the agent fails or times out
- THEN a stale reply cannot overwrite the edited draft, Cancel cancels only its session while the connection stays, and failures surface an error without touching the draft

Evidence: `apps/rune/src/editor/acp.rs`, `apps/rune/src/editor/commit.rs`, `crates/app-core/src/acp.rs`

### RNE-backend-isolation

The system SHALL run every Git, filesystem, storage, and highlighting job on a single worker thread fed by a bounded queue of 128 pending jobs, applying a reply only when its checkout identity and read generation still match the live state and dropping stale results entirely.

#### Scenario: Checkout switch drops stale replies

- GIVEN a read was issued for checkout A
- WHEN the user switches to checkout B before the reply lands
- THEN the reply's `checkout_key` no longer matches and it is dropped without touching B's state

#### Scenario: Superseded read dropped, writes always reported

- GIVEN two reads of the same kind were issued for one checkout
- WHEN the older reply arrives after the newer one was issued
- THEN the older reply is dropped by generation while every write reply still applies so busy flags and drafts resolve

#### Scenario: Queue full

- GIVEN all 128 pending slots are occupied
- WHEN another job is enqueued
- THEN the enqueue is refused, the section shows a "Backend queue is full" error and notice, and no busy flag sticks

Evidence: `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/backend/apply.rs`, `apps/rune/src/editor/highlight.rs`, `crates/app-core/src/projects.rs`

### RNE-change-gutter

The system SHALL mark, beside the line numbers of the open file, the lines the working tree changed against `HEAD` — a green bar for added lines, a blue bar for modified lines and a red tick where lines were removed — read through `git diff -U0 HEAD` on the worker lane, hidden while the buffer holds unsaved edits and re-read after the file opens, reloads or saves.

#### Scenario: Changed lines are marked

- GIVEN a tracked file with uncommitted changes (staged or not) is open and clean
- WHEN the read lands
- THEN each added, modified or deleted span is drawn in the gutter at its lines, and an untracked file shows no marks

#### Scenario: A dirty draft hides the bars

- GIVEN the bars are shown
- WHEN the user edits the buffer
- THEN the bars disappear because they describe the file on disk, and return after the save lands

Known gap: a commit, stage or discard made from the Changes section does not re-read the open file's bars until it is reopened, reloaded or saved.

Evidence: `apps/rune/src/editor/line_changes.rs`, `apps/rune/src/editor/code.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/backend/apply.rs`, `crates/app-core/src/git_view.rs`

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

### RNE-checkout-selection

The system SHALL let the user pick a registered project and one of its checkouts — the project root or a listed Git worktree — through the workspace pickers or the chrome sidebar's project and worktree rows, routing every selection through the draft-guarded `SelectCheckout`/`SelectProject` navigation with each committed checkout becoming the sidebar's highlighted `active_workspace`, and validating every checkout through `app-core` project/checkout rules while never deriving editor selection from terminal sessions.

#### Scenario: Pick project and worktree

- GIVEN registered projects exist, one with linked worktrees
- WHEN the user picks a project then a checkout
- THEN the editor loads that checkout's sections through the guarded navigation path and keys all reads by `project_id` + canonical path

#### Scenario: Pick from the sidebar

- GIVEN the chrome sidebar lists a registered project's worktree row
- WHEN the row is pressed
- THEN the same guarded `SelectCheckout` commits that exact project+checkout — the stored project root for a project row, the listed worktree path for a worktree row — and `active_workspace` highlights it

#### Scenario: Invalid checkout rejected

- GIVEN a checkout path that is not the project root or a listed worktree
- WHEN the backend validates it for a Git or file job
- THEN `projects::git_checkout`/`project_checkout` rejects it and no Git command runs

Evidence: `apps/rune/src/editor/workspace.rs`, `apps/rune/src/sidebar.rs`, `apps/rune/src/editor/navigation.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/data.rs`, `crates/app-core/src/projects.rs`

### RNE-code-surface

The system SHALL edit the open document in a rune-kit native text surface with a line-number gutter, offering `⌘S` save, `⌘F` find with match count and `⌘G`/`⇧⌘G` match navigation, `Escape` to close the find bar, `Tab`/`Shift+Tab` line indentation, `Enter` auto-indent, and read-only selectable rendering for files that cannot be edited.

#### Scenario: Edit and save a file

- GIVEN an editable UTF-8 file is open
- WHEN the user types and presses `⌘S`
- THEN the save enqueues through the write lane and the document's original text updates only on success

#### Scenario: Find in document

- GIVEN a document is open
- WHEN the user presses `⌘F`, types a query, and presses `⌘G`
- THEN the find bar shows the match count and steps through matches; `Escape` closes it

#### Scenario: Tab indents the caret line or selection

- GIVEN an editable document and the resolved indent unit
- WHEN `Tab` is pressed with no selection, or `Shift+Tab` with a multi-line selection
- THEN the caret line gains the indent unit, or every touched line's leading whitespace is indented/outdented by one unit as a single undoable edit

#### Scenario: Enter keeps the line's indent

- GIVEN an editable document whose current line has leading whitespace
- WHEN `Enter` is pressed
- THEN the new line starts with that whitespace, plus one indent unit when the line ends with an opening brace, bracket, or colon

#### Scenario: Read-only file

- GIVEN a binary, non-UTF-8, truncated, oversized, or read-only file is open
- WHEN the document renders
- THEN it shows the read-only surface and no edit or save path is offered

Evidence: `apps/rune/src/editor/code.rs`, `apps/rune/src/editor/find.rs`, `apps/rune/src/editor/files.rs`, `apps/rune/src/editor/draft.rs`, `apps/rune/src/editor/navigation.rs`, `apps/rune/src/editor/indent.rs`

### RNE-content-search

The system SHALL keep a filename filter on the file tree at all times and, in Content mode, search the checkout's file contents literally only after the trimmed query reaches two characters and 250 ms pass without typing, opening a selected result at its matching line through the unsaved-draft guard.

#### Scenario: Debounced content search

- GIVEN Content mode is active and the query has at least two trimmed characters
- WHEN typing pauses for 250 ms
- THEN exactly one checkout search is issued for the latest query; shorter queries never issue

#### Scenario: Open a match

- GIVEN content results are shown
- WHEN the user selects a result
- THEN the file opens at the matching line through the draft guard, or a same-file result reveals the line directly

#### Scenario: Grouped results

- GIVEN content results are shown
- WHEN the results pane renders
- THEN matches group under per-file header rows in first-seen order with indented `line: preview` rows, at most 500 displayed matches with a truncation note

Evidence: `apps/rune/src/editor/search.rs`, `apps/rune/src/editor/files.rs`, `apps/rune/src/editor/navigation.rs`, `crates/app-core/src/git_view.rs`

### RNE-current-branch

The system SHALL show the current checkout's branch in the shared editor header across all sections, showing Detached with the short commit ID when detached, refreshing in the background while the editor is active, and showing loading or unavailable states instead of another checkout's branch or a failed cached value without changing Git state or document drafts.

#### Scenario: Branch stays visible

- GIVEN a checkout with a named branch is selected
- WHEN the user opens Files, Changes, History, Stashes, or Servers
- THEN the shared header displays that branch, truncating long names with the full name available on hover

#### Scenario: External switch or detached HEAD

- GIVEN the editor is active
- WHEN Git changes the checkout branch or detaches HEAD externally
- THEN a background worker refresh updates the header within the next three-second polling interval plus worker time, and detached HEAD includes its short commit ID

#### Scenario: Loading or failure

- GIVEN a checkout changes or its Git metadata cannot be read
- WHEN the header renders before a successful read
- THEN it shows loading or an unavailable state and does not present stale metadata as current

Evidence: `apps/rune/src/editor/branch.rs`, `apps/rune/src/editor/workspace.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/backend/apply.rs`, `apps/rune/src/main.rs`

### RNE-draft-guard

The system SHALL route every navigation that would leave a dirty draft — opening another file, switching checkout or project, switching section or view, closing the window, and quitting — through a Save/Discard/Cancel guard, resuming the pending navigation only after a successful save, reverting on Discard, and preserving both editor and draft on Cancel or failed save.

#### Scenario: Guard on view switch

- GIVEN the open file has unsaved changes and `view` is `Editor`
- WHEN the user presses the Terminal view item and chooses Cancel
- THEN `view` stays `Editor` with the draft intact; choosing Save enqueues the write and switches only when it lands

#### Scenario: Guard on window close and quit

- GIVEN a dirty draft
- WHEN the user closes the window or quits the app
- THEN the close/quit is cancelled pending the guard on every platform for window close, and on macOS an OS-level quit is vetoed via the `applicationShouldTerminate:` swizzle; on other platforms OS-level quit remains an honest gap

#### Scenario: Discard reverts

- GIVEN a dirty draft and a pending navigation
- WHEN the user chooses Discard
- THEN the buffer reverts to the loaded original and the pending navigation proceeds

Evidence: `apps/rune/src/editor/navigation.rs`, `apps/rune/src/editor/draft.rs`, `apps/rune/src/editor/quit_guard.rs`, `apps/rune/src/main.rs`

### RNE-editor-indent

The system SHALL resolve the indent unit per document — manual override (`auto`, `2`, `4`, `8`, or `tab`) persisted through the shared settings store, else content sniffing of the dominant leading-whitespace unit, else a per-language default, else 4 spaces — show it on a compact status chip that cycles the override on press, and draw subtle indent guide ticks inside the code surface at each multiple of the resolved unit a line's leading-whitespace visual column reaches, computed for the whole document up to a bounded line count, merged into a few draw segments, and mounted inside the scroll content so it tracks scrolling natively; files already containing tab characters render them at the framework's fixed tab stops, and the resolved unit governs inserted whitespace and guides only.

#### Scenario: guides follow indentation

- GIVEN a document whose lines are indented with the resolved unit
- WHEN the editor renders and scrolls
- THEN each visible line shows guide ticks at its indent multiples, aligned to the monospace grid, without shifting text layout

#### Scenario: override persists

- GIVEN the status chip shows the resolved unit
- WHEN the user presses it to cycle to `4` and restarts the app
- THEN new documents default to the `4` override until changed back to `auto`

#### Scenario: auto detection

- GIVEN no manual override and a document whose indented lines dominantly use two spaces
- WHEN the editor resolves the indent unit
- THEN Tab inserts two spaces and guides draw every two columns

Evidence: `apps/rune/src/editor/code.rs`, `apps/rune/src/editor/indent.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/state.rs`, `apps/rune/src/editor/files.rs`, `apps/rune/src/main.rs`, `crates/app-core/src/store.rs`

### RNE-empty-state-projects

The system SHALL list up to five registered projects (name and root) in the Editor when no checkout is selected, selecting a project's root checkout through the draft-guarded navigation when one is pressed, alongside the Add project button.

#### Scenario: pick a project

- GIVEN no checkout is selected and three registered projects
- WHEN the Editor renders and the second row is pressed
- THEN that project's root checkout is selected through the draft guard

Evidence: `apps/rune/src/editor/workspace.rs`, `apps/rune/src/editor/navigation.rs`

### RNE-external-open

The system SHALL accept an external open-file/line request only through the backend worker, which validates the project, checkout, and relative path shape before opening the document once at the requested line — and the current source has a consumer but no producer for this request.

#### Scenario: Valid external open

- GIVEN a registered project/checkout and a relative file path inside it
- WHEN an external open request arrives
- THEN the worker validates the checkout, the file opens at the requested line, or the request queues behind an in-flight checkout switch and applies once

#### Scenario: Malformed or wrong-checkout request

- GIVEN an absolute path, a `..` segment, or a checkout that fails validation
- WHEN the request is processed
- THEN it is rejected before any file opens

Note: `navigation::external_open` is `#[allow(dead_code)]` — the OS-level producer (launch arg / open-URL event) remains deferred; only the consumer, validation, and once-only application exist and are tested today.

Evidence: `apps/rune/src/editor/navigation.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/backend/apply.rs`, `apps/rune/src/editor/state.rs`

### RNE-file-editing

The system SHALL open complete UTF-8 text files up to the shared 1 MiB cap for editing, treat binary, non-UTF-8, missing, unreadable, truncated, or oversized files as read-only, track dirtiness against the loaded original, and save atomically only when the file still matches the expected original text.

#### Scenario: Edit within the cap

- GIVEN a UTF-8 file under 1 MiB
- WHEN the user edits and saves
- THEN `git_view::save_file` verifies the expected original, UTF-8/NUL/size limits, canonical containment, and a writable regular file, then replaces it atomically through a temporary sibling

#### Scenario: Save conflict

- GIVEN the file changed on disk since it was opened
- WHEN the user saves
- THEN the expected-original check fails, an error surfaces, and the buffer is preserved

#### Scenario: Non-editable file kinds

- GIVEN a binary, non-UTF-8, truncated, oversized, missing, or read-only file
- WHEN it is selected
- THEN the read-only surface renders and save is never offered

#### Scenario: Markdown preview

- GIVEN a `.md`, `.markdown`, or `.mdx` file is open — the match is case-insensitive and applies to read-only documents too
- WHEN the user toggles the preview in the document bar
- THEN the file renders as markdown while the raw source stays in the edit buffer; other extensions get no preview

Evidence: `apps/rune/src/editor/draft.rs`, `apps/rune/src/editor/files.rs`, `apps/rune/src/editor/code.rs`, `crates/app-core/src/git_view.rs`

### RNE-go-to-definition

The system SHALL, on `Cmd`/`Ctrl`+click in the Files editor, first offer the click to the language-server definition path when a server covers the file, and otherwise resolve an import specifier against the checkout's file list — relative paths with extension and `index.*` probing, `.js` to TypeScript source, and `tsconfig`/`jsconfig` `paths` aliases from the nearest config — or find a clicked name's unique declaration through content search, falling back to Content mode with the name as the query when no single declaration exists; an absent, failed, timed-out, or empty language-server answer falls back to the same heuristic path.

#### Scenario: Open a relative import

- GIVEN `import { x } from './util'` is open and `util.ts` exists beside it
- WHEN the user Cmd/Ctrl+clicks the specifier
- THEN `util.ts` opens through the draft guard

#### Scenario: Unique declaration vs fallback

- GIVEN a name declared exactly once outside the open file
- WHEN the user Cmd/Ctrl+clicks a use of it
- THEN the declaring file opens at the declaration line; with zero or several declarations, Content mode opens with the name as its query instead

#### Scenario: Language-server miss falls back

- GIVEN no language server covers the file, or its definition request errors, times out, or returns nothing
- WHEN the user Cmd/Ctrl+clicks a token
- THEN the heuristic import/declaration path runs unchanged

Evidence: `apps/rune/src/editor/go_to_definition.rs`, `apps/rune/src/editor/lsp.rs`, `apps/rune/src/editor/code.rs`, `apps/rune/src/editor/navigation.rs`, `apps/rune/src/editor/search.rs`

### RNE-history

The system SHALL show History as a branch-filtered, paged commit table with a lane graph, a synthetic uncommitted-changes row, a filter that dims non-matching rows while keeping the whole graph, match navigation, Jump to HEAD, a commit detail pane with parents and patch, and a resizable layout persisted through the shared settings store.

#### Scenario: Filter and jump

- GIVEN several commits are loaded
- WHEN the user types a filter and presses Enter
- THEN non-matching rows dim, the graph stays intact, and selection moves to the next match

#### Scenario: Branch scope and paging

- GIVEN the checkout has local and remote branches
- WHEN the user picks a branch or presses load-more
- THEN only that ref's commits list in 100-commit pages and the selection resets on branch change

#### Scenario: Uncommitted row and detail

- GIVEN the checkout has uncommitted changes and HEAD is loaded
- WHEN History opens on All branches
- THEN an "Uncommitted changes" row sits above HEAD offering the Changes section, and selecting any commit shows its detail with parents, files, and patch

#### Scenario: Parents follow children

- GIVEN commits with skewed or equal commit timestamps
- WHEN History loads them
- THEN every parent lists after its children (`git log --topo-order` in the shared backend) so the graph lanes connect

Evidence: `apps/rune/src/editor/history.rs`, `apps/rune/src/editor/history_graph.rs`, `apps/rune/src/editor/history_layout.rs`, `apps/rune/src/editor/commit_detail.rs`, `apps/rune/src/editor/patch.rs`, `crates/app-core/src/git_view.rs`

### RNE-inline-blame

The system SHALL show, next to the open file's code, an annotation on the caret's line naming the author who last touched it and how long ago the commit landed, read through `git blame` on the worker lane — the annotation blanked (the column stays mounted) while the buffer holds unsaved edits, re-read after the save lands, and toggled on and off from the editor footer with the choice persisted under `goko.editor.blame`.

#### Scenario: Annotations land beside the code

- GIVEN a tracked file is open in a checkout
- WHEN the blame read lands
- THEN the column shows the caret line's author name and a compact age, following the caret, and a line changed in the working tree but never committed reads "uncommitted"

#### Scenario: A dirty draft blanks the annotation

- GIVEN the annotation is shown
- WHEN the user edits the buffer
- THEN the annotation goes blank because it describes the file on disk, the column keeps its width so the code does not reflow, and fresh annotations return after the save lands

#### Scenario: The toggle persists

- GIVEN the user presses the blame chip in the editor footer
- WHEN the app restarts
- THEN the column comes back in the state the user left it, stored under `goko.editor.blame`

#### Scenario: A file Git cannot blame

- GIVEN the open file is untracked or otherwise rejected by Git
- WHEN the blame read fails
- THEN the editor shows the error and no annotation is invented

Evidence: `apps/rune/src/editor/blame.rs`, `apps/rune/src/editor/code.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/backend/apply.rs`, `apps/rune/src/editor/files.rs`, `crates/app-core/src/git_view.rs`

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

### RNE-keyboard

The system SHALL handle the editor's global chords — `⌘S` save, contextual `⌘F` find and `⇧⌘F` content search, `⌘P` quick-open — from anywhere in the Editor view, offering `1`–`4` section switching (Files/Changes/History/Stashes; Servers has no digit) plus `j`/`k` highlight-cursor movement with `Enter` activation only while a checkout is selected, no editable field holds focus or an IME composition, and no modal surface owns input.

#### Scenario: Chord from anywhere

- GIVEN the Editor view is active and a document or list has focus
- WHEN the user presses `⌘S`, `⌘F`, `⇧⌘F`, or `⌘P`
- THEN save, find, content-search, or the quick-open palette triggers even while a field is focused

#### Scenario: Guarded plain keys

- GIVEN no checkout is selected, an editable field is focused, an IME composition is active, or a modal (draft guard, picker, context menu, quick-open) owns input
- WHEN the user presses `1`–`4`, `j`, `k`, or `Enter`
- THEN the shortcut does not fire; with a clean state `j`/`k` move only the highlight cursor and `Enter` activates through the row's existing handler

Evidence: `apps/rune/src/editor/keyboard.rs`, `apps/rune/src/editor/quick_open.rs`, `apps/rune/src/editor/files.rs`, `apps/rune/src/editor/search.rs`, `apps/rune/src/editor/history.rs`, `apps/rune/src/editor/changes.rs`, `apps/rune/src/main.rs`

### RNE-language-servers

The system SHALL run one stdio language-server connection per `(checkout, server)` on a dedicated runtime thread — never on the UI thread — keep the Servers section listing each merged server's command, extensions, live status, and feature summary with enable/disable, restart, and command-override controls, mirror open documents with debounced incremental `didChange` notifications bounded to the 1 MiB file cap, show a plaintext hover card after the caret settles, answer go-to-definition with a local timeout and heuristic fallback, and defer diagnostics and completion entirely.

#### Scenario: Configure and monitor servers

- GIVEN `lsp.servers` settings JSON (64 KiB cap, at most 32 entries) overrides or adds to the builtin presets
- WHEN the Servers section renders for a checkout
- THEN each server row shows its effective command, covered extensions, live state (Off/Starting/Running/Error), feature summary, and its controls

#### Scenario: Hover and definition

- GIVEN a document is open and a server covers its extension
- WHEN the caret settles for 400 ms or the user Cmd/Ctrl+clicks a token
- THEN a hover card shows plaintext capped at 4000 characters/20 lines after a 5 s timeout budget, or a definition request navigates through the draft guard within 15 s — empty, failed, or timed-out answers fall back to the heuristic path

#### Scenario: Deferred features stay absent

- GIVEN any document and server state
- WHEN the editor renders or polls
- THEN no diagnostics surface and no completion UI exists; `didOpen` is skipped for documents over 1 MiB and `didChange` is debounced 120 ms

#### Scenario: Server capability gaps surface honestly

- GIVEN a server whose advertised capability returns nothing usable — the HTML server's definition request answers an empty list and CSS resolves only same-file symbols
- WHEN a hover or definition request completes
- THEN the empty or partial result surfaces honestly (empty card suppressed, heuristic fallback for definition) rather than fabricated content

Evidence: `apps/rune/src/editor/lsp.rs`, `apps/rune/src/editor/lsp/pos.rs`, `apps/rune/src/editor/lsp/sync.rs`, `apps/rune/src/editor/workspace.rs`, `apps/rune/src/editor/state.rs`, `apps/rune/src/main.rs`, `crates/app-core/src/lsp.rs`

### RNE-notice-dismiss

The system SHALL show a dismiss button on the Editor's notice bar that clears the notice at once.

#### Scenario: dismiss a stale notice

- GIVEN the notice bar shows a stale-data notice
- WHEN its dismiss button is pressed
- THEN the bar disappears

Evidence: `apps/rune/src/editor/workspace.rs`, `apps/rune/src/editor/state.rs`

### RNE-pane-resize

The system SHALL let the user drag an 8 px separator grip between each editor list pane and its detail pane — Files aside, Changes list/patch, Stashes list/detail, and the History table/detail divider — clamping the stored width to 180–480 px against at least 240 px of sibling space, persisting widths inside the shared `goko.history.layout` settings document, stepping by keyboard, and resetting on double-press.

#### Scenario: Drag and persist a pane width

- GIVEN a section's list/detail split is visible
- WHEN the user drags its separator or uses the keyboard step
- THEN the width clamps to the pane bounds and remaining viewport space, persists through the settings lane, and a double-press restores the section default

#### Scenario: Narrow window keeps detail usable

- GIVEN a stored width larger than the window allows
- WHEN the workspace renders at a narrow size
- THEN the detail pane keeps its minimum usable space and the aside shrinks; the stored value returns when the window grows

Evidence: `apps/rune/src/editor/pane_layout.rs`, `apps/rune/src/editor/history_layout.rs`, `apps/rune/src/editor/history.rs`, `apps/rune/src/editor/files.rs`, `apps/rune/src/editor/changes.rs`, `apps/rune/src/editor/stashes.rs`, `apps/rune/src/editor/workspace.rs`

### RNE-quick-open

The system SHALL open a `⌘P`/`Ctrl+P` modal palette that fuzzy-ranks the cached checkout file list — a case-insensitive subsequence score with boundary, streak, and span bonuses — shows at most 12 rows with Page Up/Down stepping, keeps at most 1000 stored matches, tags matches to their checkout key so a switch or refreshed path list invalidates them, routes every selection through the dirty-draft navigation guard, and asks the backend for the file list when the cache is empty.

#### Scenario: Fuzzy pick a file

- GIVEN a checkout's file list is cached and the palette is open
- WHEN the user types a query and presses Enter on a match
- THEN the best-scoring path opens through `PendingNav::OpenDoc` and the dirty-draft guard applies

#### Scenario: Stale or empty palette state

- GIVEN the checkout switched or the path list refreshed since matches were built, or no cache exists yet
- WHEN the palette renders or a selection is attempted
- THEN stale matches are invalidated rather than opening a wrong-checkout path, and an empty cache triggers the normal Files refresh

Evidence: `apps/rune/src/editor/quick_open.rs`, `apps/rune/src/editor/keyboard.rs`, `apps/rune/src/editor/navigation.rs`, `apps/rune/src/editor/files.rs`, `apps/rune/src/editor/state.rs`

### RNE-section-views

The system SHALL present the editor as five sections — Files, Changes, History, Stashes, Servers — defaulting to Changes, render only the active section's view while retaining each section's state in `EditorState` across switches, and issue a section's reads on activation, checkout selection, or manual refresh rather than for hidden sections.

#### Scenario: Switch sections

- GIVEN a checkout is selected and Changes is active with loaded data
- WHEN the user presses the History tab
- THEN `section` becomes History, its reads issue through `refresh_active`, and only History's view renders; returning to Changes restores its retained selection and data from `EditorState`

#### Scenario: Hidden sections stay cold

- GIVEN only Changes has been activated
- WHEN the workspace renders and polls
- THEN no Files, History, Stashes, or Servers reads are issued until each section is first activated

Evidence: `apps/rune/src/editor/state.rs`, `apps/rune/src/editor/workspace.rs`, `apps/rune/src/editor/navigation.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/files.rs`, `apps/rune/src/editor/changes.rs`, `apps/rune/src/editor/history.rs`, `apps/rune/src/editor/stashes.rs`, `apps/rune/src/editor/lsp.rs`

### RNE-sidebar-sync

The system SHALL keep the chrome sidebar's committed workspace and the editor's checkout selection in sync — activating the committed checkout when the Editor view opens, decoding `active_workspace` into project and checkout, ignoring `__unassigned__`, unregistered or vanished project ids, and an already-matching selection, through the same draft-guarded `SelectCheckout` path, and clearing the committed workspace when a project switch drops the checkout so no stale row stays highlighted.

#### Scenario: Editor opens on the committed checkout

- GIVEN `active_workspace` names a registered project's checkout and the editor's selection is empty or different
- WHEN the user switches to the Editor view
- THEN `activate_sidebar_checkout` navigates `SelectCheckout` for that project+checkout and the sidebar highlight already matches it

#### Scenario: Unregistered workspace ignored

- GIVEN `active_workspace` is `__unassigned__`, names an unregistered or vanished project id, or no workspace is committed
- WHEN the Editor view opens
- THEN the view still switches but no project or checkout is selected

#### Scenario: Guarded view switch applies the sync after Discard

- GIVEN a dirty draft and `active_workspace` names another registered checkout
- WHEN the user switches to the Editor view and chooses Discard
- THEN the view becomes Editor with that project+checkout activated

Evidence: `apps/rune/src/editor/navigation.rs`, `apps/rune/src/chrome.rs`, `apps/rune/src/data.rs`, `apps/rune/src/main.rs`

### RNE-stashes

The system SHALL list the repository-wide stash stack — shared across linked worktrees — with a filter, auto-selection of the newest stash, a detail pane showing the selected stash's patch, and manual refresh, offering no stash mutation actions.

#### Scenario: Browse stashes

- GIVEN a repository with stashes
- WHEN the Stashes section activates
- THEN the stack lists newest-first, the newest is auto-selected, and its diff loads in the detail pane

#### Scenario: No stash writes

- GIVEN any stash is selected
- WHEN the section renders
- THEN only read actions (refresh, select, filter) exist — no apply, pop, drop, or create control is offered

Evidence: `apps/rune/src/editor/stashes.rs`, `apps/rune/src/editor/patch.rs`, `crates/app-core/src/git_view.rs`

### RNE-syntax-highlight

The system SHALL colour supported languages with rune-highlights lexers across the complete document up to the 1 MiB file cap — no byte-prefix truncation — while bounding coloured ranges at 131072 with a visible notice when that budget is reached, keeping the complete file editable, rendering unknown languages as plain text, and computing highlights off the UI thread scoped to document revision so stale results never render.

#### Scenario: Large file fully highlighted

- GIVEN a source file larger than 32 KiB in a supported language
- WHEN its highlight result applies
- THEN every line through the end of the file is coloured, no partial-highlighting notice shows, and the whole file remains editable

#### Scenario: Range budget reached

- GIVEN a document whose tokens exceed the 131072-range budget
- WHEN its highlight result applies
- THEN colouring stops at the budget, a visible capped notice shows, and the whole file remains editable

#### Scenario: Stale highlight dropped

- GIVEN a highlight job was issued for revision N
- WHEN the user edits before the reply lands
- THEN the reply's revision no longer matches and it is dropped; a newer revision re-arms the debounce

#### Scenario: Unknown language

- GIVEN a file whose extension maps to no lexer
- WHEN it opens
- THEN it renders as plain text and no highlight job is enqueued

Evidence: `apps/rune/src/editor/syntax.rs`, `apps/rune/src/editor/highlight.rs`, `apps/rune/src/editor/code.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/Cargo.toml`

## Flow

The shared editor header places a Git branch badge beside Project and Checkout
in all five sections. A named branch shows its name (including before the first
commit); a detached checkout shows `Detached · <short HEAD>`. Long names truncate
with the full value in the tooltip and accessibility label. The path yields
space and the header wraps at narrow widths. The badge is informational.

`branch::poll` issues `Req::CurrentBranch` immediately on entering the editor
and then at three-second intervals while it is active, allowing only one probe
in flight. The serialized worker validates project/checkout, runs `symbolic-ref`
(one metadata call for a named branch), and reads the short HEAD only for Git's
expected detached exit. Replies use the existing checkout/generation guards.
A committed checkout switch resets branch state; another checkout's cached data
is never shown. No probe runs while the editor is hidden; returning requests a
fresh probe. Pending reads retain the last result with a checking tooltip, while
initial loading and failed/unqueued reads show loading or unavailable states.
Git failures are retried by the same interval without changing documents.


Conflict rows activate the guarded Files editor from either pointer or keyboard.
Each complete marker block has Current (green) and Incoming (accent) highlights
and Accept Current / Accept Incoming / Accept Both controls. Both concatenates
Current then Incoming; each acceptance is one undoable native edit. Previous and
Next conflict wrap through blocks. Standard, diff3/zdiff3, custom marker widths,
CRLF, and a closing marker at EOF are supported; malformed/nested blocks require
manual editing. Binary and missing conflicts remain inside Rune with a limitation
message, including media types normally opened externally.

Mark Resolved is offered only for the open conflicted file, disabled until the
editable buffer is marker-free and no save/Git write is running. A dirty file
first uses the existing expected-original save. Only a successful save queues
`StageResolved`; the worker rereads the file, checks exact saved contents and
remaining markers, and stages that one literal path. Changes then refreshes;
no commit is created. A clean resolved file goes straight to that worker check.


`main.rs` mounts `editor::workspace::workspace` as the Editor view's content
while keeping the tab strip; the session controls strip stays terminal-only.
Editor selection happens in two places — the workspace's project and checkout
pickers and the chrome sidebar's project/worktree rows — both routing
`PendingNav::SelectCheckout`/`SelectProject` through `navigate` and never
reading terminal session state. `EditorState` (`state.rs`) owns the
checkout identity (`project_id` + canonical path as `checkout_key`), the
registered-project list with its loading/error flags, the per-project
`sidebar_worktrees` entries behind the sidebar rows, the picker's `worktrees`
list, per-section cached state, the open document and draft, the navigation
guard, pending external opens, request generations, the `Lsp` runtime handle,
and modifiers.

Every blocking operation — project/worktree lists, settings, file reads and
saves, Git reads and writes, content search, ACP agent discovery — is a `Req`
enqueued as a `Job` on the worker's `sync_channel` of capacity 128
(`backend.rs`). The worker revalidates project and checkout through
`app-core`'s `projects` module for every checkout job, runs the `app-core`
call, and returns a `Reply` carrying `checkout_key` and `generation`;
`apply_reply` (`backend/apply.rs`) drops replies whose checkout no longer
matches or whose generation was superseded, then routes results into section
state. Section modules (`files`, `changes`, `history`, `stashes`, `commit`)
issue their reads through `refresh_active`/`enqueue_read` and writes through
`enqueue_write` on the same serialized lane.

File opens flow `open_doc` → `navigate` (draft guard) → `Req::File` →
`install_doc`, which builds an `OpenDoc` (`draft.rs`) holding the loaded
original, the live edit buffer, and read-only/binary/truncated restrictions.
Edits mark the draft dirty and arm the highlight debounce (`highlight.rs`);
saves go through `enqueue_save` with the expected-original contract.
`navigation.rs` funnels section, checkout, project, view, document, window,
and quit changes through `navigate`, raising the Save/Discard/Cancel guard
for dirty drafts; `quit_guard.rs` adds the macOS `applicationShouldTerminate:`
swizzle so OS-level quits are vetoed while a draft is dirty. `select_checkout`
commits post-guard only: it records the checkout as the chrome's
`active_workspace` highlight key (`data::checkout_key`, NUL-joined), reloads
worktrees through the shared `request_worktrees` lane when the project
changed, resets every per-checkout cache, and refreshes the active section;
`clear_checkout` clears `active_workspace` so nothing stale stays
highlighted. `SetView(Editor)` apply then calls `activate_sidebar_checkout`,
which decodes `active_workspace` back into project+checkout and navigates
`SelectCheckout` when it names a still-registered project and differs from
the live `checkout_key` — the sidebar→editor sync.

ACP generation lives on its own Tokio runtime thread (`acp.rs`): UI commands
(connect, generate with a draft snapshot, cancel, refresh, shutdown) cross an
unbounded channel; the runtime owns the `AcpHub`, revalidates the checkout
before generating, and applies results back through `apply_acp_result`, which
fills the commit draft only when it is unchanged.

Language servers follow the same ownership shape (`lsp.rs` + `lsp/pos.rs` +
`lsp/sync.rs`): one runtime thread hosts a current-thread Tokio runtime and
the shared `app_core::lsp::LspHub`; the UI sends `LspCmd`s and drains
`LspMsg`s from the poll loop. Document sync mirrors open files per
`(checkout, server)` — `didOpen` only under the 1 MiB cap, `didChange`
debounced 120 ms as incremental UTF-16 edits — hover fires after a 400 ms
caret settle with a 5 s local timeout, and definition requests carry a 15 s
timeout whose empty/error/timeout outcome queues the existing heuristic
`GoToRequest` fallback. A hover card renders as a caret-anchored overlay and
`Escape` dismisses it; LSP-driven navigation goes through the same
`PendingNav`/draft-guard path as every other open. Diagnostics and
completion are deliberately absent — the transport already carries what they
will need.

## State changes

Conflict ranges are cached in `CodeState` and rebuilt on load/edit/undo;
`OpenDoc::resolve_after_save` records an explicit pending resolution and is
cleared on either save outcome. A stage failure keeps the successfully saved
baseline, reports the Git error, and permits retrying Mark Resolved.


In-memory: `EditorState` holds the registered-project list
(`projects`/`projects_loading`/`projects_error`), per-project
`sidebar_worktrees` entries, the picker's `worktrees` list, project/checkout
selection (`project_id`, `checkout`, `checkout_key`, `pending_checkout`),
per-section state (changes data and selection, files tree/model/search,
history commits/branches/layout/selection, stashes list/detail, LSP server
entries/statuses/document mirrors/hover state), the open document and its
dirty flag, the navigation guard and pending navigation, pending external
opens, request generations, and ACP runtime handles. A committed
`SelectCheckout` also mirrors into the chrome's `State::active_workspace` —
the sidebar highlight key — inside `select_checkout` post-guard, and
`clear_checkout` clears it; session and tab presses can overwrite it with
`__unassigned__` without touching the editor's own selection. Each section's
state survives switches because it lives in `EditorState`, even though
`section_body` renders only the active section; `visited` records which
sections have been activated. Nothing in the editor's section state persists
across launches.

Persisted through the shared `goko.db` settings table (same store the desktop
uses): the history pane layout under `goko.history.layout`, the commit agent
pick under `commit.agent`, ACP agent configuration under `acp.agents`, and
the language-server override document under `lsp.servers`. Project
registration itself lives in `goko.db`'s `projects` table, written through
the worker's `Req::AddProject`.

Git state changes only through the write lane: stage/unstage/discard mutate
the index and working tree, commit creates a commit (hooks run), push updates
the remote. File saves replace working-tree files atomically. ACP sessions
spawn short-lived agent processes in temporary directories; LSP connections
spawn configured server processes per checkout. None of these touch daemon
session state — the daemon's session lifecycle is not involved in the editor.

## Side effects

- Current-branch probes read only Git metadata. They never switch branches,
  stage/commit, reload a file, or replace an unsaved editor draft.


- Project metadata runs on the same worker lane as project-scoped jobs:
  `Req::Projects` lists the `goko.db` projects table, `Req::AddProject`
  canonicalizes and registers a picked folder (a mutating job that finishes
  during the shutdown drain), and `Req::Worktrees` resolves the stored
  project root worker-side, then `worktrees::list` reports the repository's
  worktrees with per-checkout dirty/changed status; replies feed both the
  checkout picker and the per-project sidebar entries.
- File reads cap at 1 MiB + 1 byte and classify binary (NUL), non-UTF-8,
  missing, unreadable, and truncated files; saves enforce the same cap plus
  expected-original equality, canonical containment, regular-file and
  non-read-only checks, then replace via a temporary sibling (`app-core`).
- Content search runs `git grep` fixed-string smart-case with argv and no
  shell, capped at 256 KiB output, 500 matches, and 300-character previews.
- Git writes run `git` via argv with `-C <checkout>`, hooks enabled, no
  force, push killed after 120 seconds.
- History reads page 100 commits per request through `git log`/`for-each-ref`
  in the validated checkout; none mutates refs.
- ACP `generate` sends the staged diff (capped like the desktop's) to the
  picked agent's model provider; the agent runs in an empty temp directory
  with permissions denied and is killed on cancel/crash/app exit.
- Language servers spawn their configured commands as stdio child processes
  scoped to `(checkout, server)`; cancelled or timed-out requests emit
  `$/cancelRequest` and free their slot even when the server ignores it.
- Highlight jobs ship document text to the worker thread for rune-kit
  scanning; no text leaves the process.

## Authorization & constraints

- Every checkout job revalidates `project_id` + canonical checkout through
  `app-core`: the checkout must be the project root or a listed worktree, and
  Git operations additionally require a real non-bare worktree — identical
  whether the selection came from the pickers or the sidebar. Project-scoped
  jobs (`Projects`, `AddProject`, `Worktrees`) carry no checkout identity;
  `Worktrees` resolves the registered root from the store on the worker and
  fails for vanished projects or missing roots.
- File and patch paths must be relative, resolve inside the checkout after
  symlink resolution, and never touch `.git`.
- Reads are generation-gated per request kind; writes are never
  generation-gated but always serialize on the one lane; the queue holds at
  most 128 pending jobs.
- Highlighting lexes the complete document per revision through
  `rune-highlights`, bounded only by a 131072-range safety budget; editing
  itself is capped only by the 1 MiB file limit.
- The draft guard is the only path off a dirty document; Discard reverts to
  the loaded original rather than deleting anything.
- Stashes are read-only in this view; the repository-wide `refs/stash` is
  shared across worktrees and keyed by project.
- `lsp.servers` settings entries are validated (64 KiB document cap, at most
  32 servers, per-entry command/extension/disabled parsing) and merged over
  the builtin presets; malformed JSON surfaces a config error in the Servers
  section rather than failing the editor.
- Terminal/session state is never consulted for editor authorization;
  conversely the editor performs no daemon or protocol calls.

## Error paths

Stale replies (wrong `checkout_key` or superseded generation) drop silently;
queue-full enqueues surface "Backend queue is full — try again" plus a notice
and never leave busy flags set. Project reads fail honestly too: a refused
`Projects`/`Worktrees` enqueue sets `projects_error`/`worktrees_error` or the
per-project sidebar entry error, the Projects reply's ensure pass re-requests
worktrees for every project missing a list or holding an error and drops
entries for vanished projects, a fresh worktree list that no longer contains
the selected checkout triggers `refresh_active` so section reads fail
honestly, and `AddProject` failures surface as a notice while success
re-lists projects. Save conflicts (expected-original mismatch,
read-only, oversized, non-UTF-8) show the backend error and preserve the
buffer. A failed save keeps the guard's pending navigation cancelled — the
draft and editor stay. Discard's confirmation survives a refused enqueue so
the dialog stays open for retry. Section read failures show the section error
pane or a stale notice over cached data; history keeps the table behind a
stale notice when a refresh fails. ACP failures (not connected, agent missing,
empty reply, non-`end_turn` stop, timeout) show an error and leave the draft;
cancel is silent. External-open requests with malformed paths or invalid
checkouts are rejected before any file opens. Language-server connections
report Off/Starting/Running/Error per `(checkout, server)` with the server's
message; hover and definition timeouts fall back to the heuristic path
silently, and `lsp.servers` parse problems appear as a config-error line in
the Servers section. On non-macOS platforms, OS-level quit (⌘Q equivalent,
logout) cannot be vetoed — window close still can — and that gap is
documented in `quit_guard.rs` rather than silently claimed.

## Tests

- Empty-file regression in `files.rs` applies create and file-load replies before
  clicking the first line or blank editor body and typing on the first open.
  `code.rs` checks short files accept blank-space clicks and long documents keep
  their content height and vertical scrolling; the input fills unused viewport
  space while its wrapper retains at least one code line per document line.


- `branch.rs` covers named/unborn branches, external checkout, detached HEAD,
  invalid Git metadata, paced active-only polling, one outstanding request,
  queue refusal, and hiding foreign/failed cached values. `backend.rs` covers
  branch-reply checkout/generation guards; `workspace.rs` verifies the long-name
  branch header remains inside a 480 px editor pane across all five sections.


- `conflict.rs` tests marker widths, diff3/base exclusion, CRLF, UTF-8, empty
  sides, EOF, nested/incomplete blocks, and malformed fragments beside valid
  blocks. `code.rs` runtime tests exercise Accept choices, undo, read-only guards,
  and previous/next wrapping. `files.rs` tests save-before-stage, repeat presses,
  marker guards, and media-conflict routing. `backend.rs` tests real Git unmerged
  entries, disk mismatch, per-file staging, unchanged HEAD, and save/stage failure
  handling. A native app check on a temporary repository observed Accept Both
  followed by Mark Resolved saving the result and clearing the unmerged entry.


- `apps/rune` carries extensive inline `#[test]`/`#[tokio::test]` cases across
  `src/` — the count grows with each slice, so `cargo test -p goko-rune` is
  the current total. `src/editor/` covers worker queue/generation/stale-reply
  handling, draft dirty/save/discard semantics, navigation guard and
  quit-guard install, file tree filter and search debounce, grouped search
  display, find-bar navigation, quick-open fuzzy scoring and stale-match
  invalidation, keyboard chord routing and plain-key guards, pane-layout
  clamping/persistence, go-to-definition token/import/alias/declaration
  resolution, syntax budget and revision matching, history graph/layout/
  selection, stashes, ACP command routing and draft-snapshot application, and
  the LSP layer's UTF-16 position conversion, incremental document sync,
  hover/definition debounce and timeout handling, settings merge/validation,
  and server-row state, project/sidebar worktree reply routing (per-project
  keys, the Projects ensure pass, vanished-project cleanup), committed-
  workspace activation and bogus-workspace rejection, and sidebar-driven
  selection guards; `src/main.rs` `view_tests` scenes cover editor
  workspace composition, the draft-guard modal, picker layout,
  session-controls absence in the Editor view, and the sidebar→editor
  scenes — real-project listing with sessions under Unassigned, root
  and worktree row selection committing identity, dirty-guard
  Discard/Cancel, status rows, and real-backend fixtures driving
  cross-project selection including in-flight replies.
- `crates/app-core` carries its own inline tests plus the shared
  `tests/fixtures/fake_acp_agent.py` driving ACP connect/generate/cancel/
  permission-denial coverage — the same fixture the desktop page cites — and
  an `lsp` module whose transport was smoke-verified against seven real
  language servers (initialize, sync, hover, definition, disconnect all
  passing; server-side capability gaps such as empty HTML definitions and
  same-file-only CSS resolution are recorded honestly, not transport bugs).
- Manual native GUI QA is partially done: file open/edit, Changes
  stage/unstage, History ordering and copy, commit/push on a disposable repo,
  the draft guard, quick-open, F12 definition navigation, Files pane drag,
  and the History detail-divider drag in both directions are verified;
  idle-hover recheck and the history persistence patch remain in flight, so
  interactive claims stay source-verified rather than fully accepted.

## Related

- [Rune chrome](rune-chrome.md) — the daemon session chrome and view switch
- [Desktop Editor](../architecture/editor.md) — the Tauri CodeMirror/Vim editor over the same `app-core` backend
- [Architecture overview](../architecture/overview.md)

## Sources

- `apps/rune/Cargo.toml`
- `apps/rune/src/main.rs`
- `apps/rune/src/chrome.rs`
- `apps/rune/src/data.rs`
- `apps/rune/src/term.rs`
- `apps/rune/src/sidebar.rs`
- `apps/rune/src/editor/mod.rs`
- `apps/rune/src/editor/state.rs`
- `apps/rune/src/editor/backend.rs`
- `apps/rune/src/editor/backend/apply.rs`
- `apps/rune/src/editor/workspace.rs`
- `apps/rune/src/editor/navigation.rs`
- `apps/rune/src/editor/draft.rs`
- `apps/rune/src/editor/quit_guard.rs`
- `apps/rune/src/editor/files.rs`
- `apps/rune/src/editor/code.rs`
- `apps/rune/src/editor/conflict.rs`
- `apps/rune/src/editor/blame.rs`
- `apps/rune/src/editor/branch.rs`
- `apps/rune/src/editor/indent.rs`
- `apps/rune/src/editor/syntax.rs`
- `apps/rune/src/editor/highlight.rs`
- `apps/rune/src/editor/search.rs`
- `apps/rune/src/editor/find.rs`
- `apps/rune/src/editor/go_to_definition.rs`
- `apps/rune/src/editor/changes.rs`
- `apps/rune/src/editor/commit.rs`
- `apps/rune/src/editor/commit_detail.rs`
- `apps/rune/src/editor/keyboard.rs`
- `apps/rune/src/editor/pane_layout.rs`
- `apps/rune/src/editor/patch.rs`
- `apps/rune/src/editor/quick_open.rs`
- `apps/rune/src/editor/history.rs`
- `apps/rune/src/editor/history_graph.rs`
- `apps/rune/src/editor/history_layout.rs`
- `apps/rune/src/editor/stashes.rs`
- `apps/rune/src/editor/acp.rs`
- `apps/rune/src/editor/lsp.rs`
- `apps/rune/src/editor/lsp/pos.rs`
- `apps/rune/src/editor/lsp/sync.rs`
- `crates/app-core/src/projects.rs`
- `crates/app-core/src/worktrees.rs`
- `crates/app-core/src/git_view.rs`
- `crates/app-core/src/git_write.rs`
- `crates/app-core/src/acp.rs`
- `crates/app-core/src/lsp.rs`
- `crates/app-core/src/store.rs`
- `crates/app-core/tests/fixtures/fake_acp_agent.py`
