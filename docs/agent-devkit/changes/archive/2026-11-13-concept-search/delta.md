# Delta

Merged into docs/llm/: 2026-11-13

## ADDED Requirements

### RNE-concept-search
Page: `docs/llm/domains/rune-editor.md`
Search: checked every page with prefix RNE; `RNE-content-search` covers only literal search and no requirement covers ranked, indexed search.
Requirement:
````md
### RNE-concept-search
The system SHALL, when the Files toolbar's search box is in Context mode, rank text chunks of the checkout's tracked and non-ignored untracked files by how well they match the query's words, matching identifier words across `snake_case` and `camelCase`, and open a selected result at its matching line through the unsaved-draft guard, while Text mode remains a literal search.
#### Scenario: The search box is always available
- GIVEN the Files section is open
- WHEN the user has pressed no shortcut
- THEN the search box and its `Text | Context` switch are visible under the file filter with Text selected, and typing two characters searches
#### Scenario: Words match identifiers
- GIVEN Context mode is active and one chunk defines `reconnect_backoff_ms` while another only mentions "backoff" in a comment
- WHEN the user searches `reconnect backoff`
- THEN the chunk that defines the identifier ranks above the comment-only chunk
#### Scenario: The chosen mode is kept
- GIVEN the user switched the search box to Context
- WHEN the user changes checkout or searches again in the same session
- THEN the box stays in Context
#### Scenario: Filter and search are independent
- GIVEN search results are showing
- WHEN the user edits the file filter
- THEN the file tree shows, filtered by name, the search text and mode are kept, and editing the search box shows the results again
#### Scenario: The index builds in the background
- GIVEN a checkout that has no index
- WHEN Context mode is first used for it
- THEN the search row shows `Indexing done/total files`, answers from what is indexed so far, the editor stays responsive, and the unchanged query is issued again when indexing finishes
#### Scenario: The index follows the working tree
- GIVEN an index exists and a file is edited, added, deleted, or rewritten as a binary file
- WHEN the next refresh pass runs
- THEN results reflect the change, only the changed files are re-chunked, and a file that is no longer indexable no longer appears
#### Scenario: A just-edited file is found by the next query
- GIVEN a full pass has finished and a file is then added, edited, or deleted in the working tree
- WHEN the user searches in Context mode immediately, less than 10 seconds later
- THEN the first reply reflects the change and only that file was re-chunked; when the catch-up takes longer than one second the reply comes from the current index and is issued again when the catch-up finishes
#### Scenario: Ignored, binary, and outside files are not indexed
- GIVEN a git-ignored file, a binary file, a file over 1 MiB, and a symlink to a file outside the checkout each contain the query word
- WHEN the user searches in Context mode
- THEN none of them appears in the results
#### Scenario: Content entry points show Text mode
- GIVEN the search box is in Context mode
- WHEN the user presses `⇧⌘F`, or a go-to-definition finds no single declaration and shows the content search results
- THEN the box is in Text mode and shows the literal results
#### Scenario: Failure leaves Text mode intact
- GIVEN the index cannot be opened or rebuilt
- WHEN Context mode is used
- THEN the search row shows `Context search unavailable` with the reason and Text mode still searches literally
Evidence: `crates/app-core/src/code_index.rs`, `apps/rune/src/editor/search.rs`, `apps/rune/src/editor/files.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/backend/apply.rs`, `apps/rune/src/editor/navigation.rs`, `apps/rune/src/editor/keyboard.rs`
````

## MODIFIED Requirements

### RNE-backend-isolation
Previously: every Git, filesystem, storage, and highlighting job ran on the single worker thread, with no exception.
Baseline:
````md
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
````
Replacement:
````md
### RNE-backend-isolation

The system SHALL run every Git, filesystem, storage, and highlighting job on a single worker thread fed by a bounded queue of 128 pending jobs, except Context search index writes (full passes and working-tree syncs), which run on their own thread outside that queue, applying a reply only when its checkout identity and read generation still match the live state and dropping stale results entirely.

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

#### Scenario: Index writes do not use the queue

- GIVEN a Context search full pass or working-tree sync is running for a checkout
- WHEN a save or a Git read is enqueued
- THEN the worker runs it without waiting for the index work, and the index work's progress never occupies one of the 128 pending slots

Evidence: `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/backend/apply.rs`, `apps/rune/src/editor/highlight.rs`, `crates/app-core/src/projects.rs`, `apps/rune/src/editor/search.rs`, `crates/app-core/src/code_index.rs`
````

### RNE-content-search
Previously: the file filter was always visible and content search ran only in Content mode of a Search pane opened with a toggle button.
Baseline:
````md
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
````
Replacement:
````md
### RNE-content-search

The system SHALL keep a filename filter on the file tree and a search box, both always visible in the Files toolbar, and, in the search box's Text mode, search the checkout's file contents literally only after the trimmed query reaches two characters and 250 ms pass without typing, opening a selected result at its matching line through the unsaved-draft guard.

#### Scenario: Debounced content search

- GIVEN Text mode is active and the query has at least two trimmed characters
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
````

### RNE-go-to-definition
Previously: the declaration fallback opened the Search pane in Content mode.
Baseline:
````md
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
````
Replacement:
````md
### RNE-go-to-definition

The system SHALL, on `Cmd`/`Ctrl`+click in the Files editor, first offer the click to the language-server definition path when a server covers the file, and otherwise resolve an import specifier against the checkout's file list — relative paths with extension and `index.*` probing, `.js` to TypeScript source, and `tsconfig`/`jsconfig` `paths` aliases from the nearest config — or find a clicked name's unique declaration through content search, falling back to Text mode with the name as the query when no single declaration exists; an absent, failed, timed-out, or empty language-server answer falls back to the same heuristic path.

#### Scenario: Open a relative import

- GIVEN `import { x } from './util'` is open and `util.ts` exists beside it
- WHEN the user Cmd/Ctrl+clicks the specifier
- THEN `util.ts` opens through the draft guard

#### Scenario: Unique declaration vs fallback

- GIVEN a name declared exactly once outside the open file
- WHEN the user Cmd/Ctrl+clicks a use of it
- THEN the declaring file opens at the declaration line; with zero or several declarations, Text mode opens with the name as its query instead

#### Scenario: Language-server miss falls back

- GIVEN no language server covers the file, or its definition request errors, times out, or returns nothing
- WHEN the user Cmd/Ctrl+clicks a token
- THEN the heuristic import/declaration path runs unchanged

Evidence: `apps/rune/src/editor/go_to_definition.rs`, `apps/rune/src/editor/lsp.rs`, `apps/rune/src/editor/code.rs`, `apps/rune/src/editor/navigation.rs`, `apps/rune/src/editor/search.rs`
````
