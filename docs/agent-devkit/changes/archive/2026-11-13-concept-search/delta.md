# Delta

Merged into docs/llm/: 2026-11-13

## ADDED Requirements

### RNE-concept-search
Page: `docs/llm/domains/rune-editor.md`
Search: checked every page with prefix RNE; `RNE-content-search` covers only literal search and no requirement covers ranked, indexed search.
Requirement:
````md
### RNE-concept-search
The system SHALL, in the Files Search pane's Concept mode, rank text chunks of the checkout's tracked and non-ignored untracked files by how well they match the query's words, matching identifier words across `snake_case` and `camelCase`, and open a selected result at its matching line through the unsaved-draft guard, while Content mode remains a literal search.
#### Scenario: Words match identifiers
- GIVEN Concept mode is active and one chunk defines `reconnect_backoff_ms` while another only mentions "backoff" in a comment
- WHEN the user searches `reconnect backoff`
- THEN the chunk that defines the identifier ranks above the comment-only chunk
#### Scenario: The index builds in the background
- GIVEN a checkout that has no index
- WHEN Concept mode is first used for it
- THEN the pane shows `Indexing done/total files`, answers from what is indexed so far, the editor stays responsive, and the unchanged query is issued again when indexing finishes
#### Scenario: The index follows the working tree
- GIVEN an index exists and a file is edited, added, deleted, or rewritten as a binary file
- WHEN the next refresh pass runs
- THEN results reflect the change, only the changed files are re-chunked, and a file that is no longer indexable no longer appears
#### Scenario: A just-edited file is found by the next query
- GIVEN a full pass has finished and a file is then added, edited, or deleted in the working tree
- WHEN the user searches in Concept mode immediately, less than 10 seconds later
- THEN the first reply reflects the change and only that file was re-chunked; when the catch-up takes longer than one second the reply comes from the current index and is issued again when the catch-up finishes
#### Scenario: Ignored, binary, and outside files are not indexed
- GIVEN a git-ignored file, a binary file, a file over 1 MiB, and a symlink to a file outside the checkout each contain the query word
- WHEN the user searches in Concept mode
- THEN none of them appears in the results
#### Scenario: Content entry points show Content mode
- GIVEN the Search pane is in Concept mode
- WHEN the user presses `⇧⌘F`, or a go-to-definition finds no single declaration and shows the content search results
- THEN the pane is in Content mode and shows the literal results
#### Scenario: Failure leaves Content mode intact
- GIVEN the index cannot be opened or rebuilt
- WHEN Concept mode is used
- THEN the pane shows `Concept search unavailable` with the reason and Content mode still searches literally
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

The system SHALL run every Git, filesystem, storage, and highlighting job on a single worker thread fed by a bounded queue of 128 pending jobs, except Concept search index writes (full passes and working-tree syncs), which run on their own thread outside that queue, applying a reply only when its checkout identity and read generation still match the live state and dropping stale results entirely.

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

- GIVEN a Concept search full pass or working-tree sync is running for a checkout
- WHEN a save or a Git read is enqueued
- THEN the worker runs it without waiting for the index work, and the index work's progress never occupies one of the 128 pending slots

Evidence: `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/backend/apply.rs`, `apps/rune/src/editor/highlight.rs`, `crates/app-core/src/projects.rs`, `apps/rune/src/editor/search.rs`, `crates/app-core/src/code_index.rs`
````
