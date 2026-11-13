# Concept search: a ranked, indexed code search in the Files Search pane

## Goal

The Files Search pane finds code only by literal text (`git grep`), so a query
such as `reconnect backoff` finds nothing when the code says
`reconnect_backoff_ms` or `ReconnectBackoff`. Add a **Concept** mode that ranks
chunks of the checkout's text files by how well they match the query's words,
using a local SQLite FTS5 index (BM25), with no embeddings, graph, or external
service. Success: in a real checkout, a word query returns the chunks whose
identifiers, path, or text contain those words, best first, a click opens the
file at the matching line, and code added or edited a moment ago is already
found by the very next query.

## Scope

In:

- A `Content | Concept` mode switch inside the existing Search pane. `Content`
  stays the default and behaves exactly as `RNE-content-search` describes.
- A derived, per-checkout FTS5 index in `GOKO_HOME/search/`, built lazily on its
  own background thread, brought up to date with the working tree before every
  Concept query, and reconciled by a periodic full pass.
- Ranked results shown in the existing results pane, opened through the existing
  navigation and unsaved-draft guard.

Out (each is a separate change or not planned):

- Terminal find (decided 2026-11-13: a find bar over the open terminal's grid in
  `apps/rune/src/term.rs`, no index, no daemon, protocol, or store change).
  It is a Bounded change of its own.
- Persisting terminal output, cross-session search, agent transcript search.
- An MCP server that exposes the index to agents.
- Embeddings, a symbol graph, tree-sitter or LSP-based chunking.
- A file watcher (FSEvents or inotify). It is the only way to remove the
  query-time wait entirely, but it needs a dependency or FFI and adds
  ignored-directory, worktree, and event-flood cases; revisit it only if the
  bounded wait described below proves unacceptable.
- Replacing or changing `git grep` Content search.
- Any change to `gokod`, `crates/protocol`, or `goko.db` (no migration, no new
  dependency: `rusqlite` with `bundled` already builds FTS5).

## Behavior

**Mode.** The Search pane shows a two-option switch, `Content` and `Concept`.
The pane opens in `Content`; the chosen mode is not persisted across restarts.
Concept uses the same query input, the same 250 ms debounce, and the same
two-character minimum as Content (`MIN_QUERY` in `search.rs`). Switching mode, in
either direction, clears the pane's results, error, and searching state and
issues the existing input again for the new mode when it has at least
`MIN_QUERY` characters, so a reply still in flight for the other mode is dropped
and never leaves `Searching…` on screen, and a switch never shows an empty pane
for text that is still in the input. Every entry that is defined as content
search shows the pane in `Content`: `⇧⌘F` (`RNE-keyboard`) switches a pane that
is in `Concept` back to `Content` before it runs, and the go-to-definition
fallback (`RNE-go-to-definition`) does the same only at the point where it shows
the pane with its results, that is when no single declaration is found. A unique
declaration (which opens the file), a stale reply, and a reply outside the Files
section leave the pane untouched.

**Indexed files.** The same set `git grep` searches, taken from the existing
helpers instead of new rules: the listing is `git_view::file_list` (tracked
files plus non-ignored untracked files, sorted and deduplicated), and each file
is read with `git_view::file`, which refuses a path that resolves outside the
checkout (a symlink pointing out), never reads `.git`, and reports binary (a
NUL byte), non-UTF-8, and over-1 MiB files. Ignored files (for example `.env`)
are never listed, and every file that is not plain text within the limit, or
that `git_view::file` refuses, is skipped. Only the first
`MAX_INDEXED_FILES = 20000` files of the sorted listing are considered by a full
pass, and a sync (below) adds a file only while the index holds fewer than that;
when the limit is hit, the pane says so.

**Chunks.** A file is split into chunks of at most `CHUNK_LINES = 40` lines and
`CHUNK_BYTES = 4096` bytes, breaking at a line boundary. When a blank line
exists in the second half of a full window, the chunk ends at the last such
blank line. Chunks overlap: each chunk after the first starts at
`max(previous start + 1, previous end - CHUNK_OVERLAP + 1)` with
`CHUNK_OVERLAP = 4`, so it repeats up to the last four lines of the previous
chunk and words split across a boundary still meet in one chunk; a chunk is
created only while lines not yet covered remain, and the index is about 10%
larger than the text. A single line longer than `CHUNK_BYTES` is indexed
truncated to its first `CHUNK_BYTES` bytes. Each chunk stores its path, start
line, end line, and text.

**Index text.** Each chunk is indexed in three FTS5 columns with the
`porter unicode61` tokenizer: `path` (the file path), `idents`, and `body` (the
chunk text). An identifier token is a maximal run of Unicode alphanumeric
characters and `_` in the chunk text. `idents` holds, in order of appearance and
without deduplication, the words the word splitter yields for each token that
yields at least two words (a compound name such as `reconnect_backoff_ms` or
`ReconnectBackoff`, which gives `reconnect backoff ms` and `reconnect backoff`);
a token that yields one word contributes nothing, because it is already in
`body`, so ordinary prose words never reach `idents`. `unicode61` already splits
`snake_case` in `body`; `idents` exists to split `camelCase` and to rank compound
names above plain prose. Ranking is `bm25` with initial column weights
`path 2.0`, `idents 1.5`, `body 1.0`; the weights are tunable and only the
ordering in the first scenario of `RNE-concept-search` is guaranteed.

**Query.** The query text goes through the same word splitter as `idents`. The
resulting words are each quoted as an FTS5 string, so FTS5 operators typed by
the user are inert, and combined with AND; the last word also matches by
prefix, so a half-typed word still matches. If AND returns nothing, the same
words are retried combined with OR. A query with no searchable word shows
`No searchable words` in the pane's existing error line instead of results.

**Results.** At most `CONCEPT_RESULT_CAP = 50` chunks, best first. Each is shown
with the existing `GitViewSearchMatch` shape: `path`, `line` (the first line of
the chunk that contains any query word as a case-insensitive substring, else the
chunk's first non-blank line, else the chunk's start line, which covers a chunk
of only blank lines matched through its path), and `text` (that line, cut to
`SEARCH_PREVIEW_CHARS`). When two chunks yield the same `(path, line)` because
the line is in their overlap, only the better-ranked row is kept, before the
cap applies. The pane groups rows under per-file headers in first-seen order,
which is rank order of each file's best chunk. Selecting a row opens the file at
`line` through the draft guard.

**Index lifecycle.**

- Location: `GOKO_HOME/search/` (mode `0700`) holds one file per checkout,
  `<fnv1a64(canonical checkout path) as 16 hex digits>.db` (mode `0600`). The
  checkout is canonicalized first, by `refresh`, `sync_changed`, `search`, and
  the file-name function alike, so every spelling of one checkout (a symlinked
  path from the sidebar, the validated path the worker uses) addresses the same
  file. The canonical path alone identifies a checkout, so a path registered
  under two projects shares one index. The file is derived data: deleting it
  only costs a rebuild. A `meta` table records the canonical checkout path and
  the schema version (`PRAGMA user_version`); a mismatch deletes and rebuilds
  the file. A `files` table records `(path, size, mtime_ns)` for every listed
  file, skipped ones included.
- Only a full pass creates, rebuilds, or deletes an index. A sync writes only to
  an existing, matching index and does nothing when there is none. A query opens
  an existing index read-only and never writes: a missing index, or one whose
  `meta` or `user_version` does not match, answers with no results until a pass
  has built it.
- Index writes, passes and syncs alike, happen on one dedicated indexer thread,
  never on the editor backend worker, so the 128-job queue, saves, and Git reads
  are not delayed. The thread serves one job at a time, first come first served.
- **First use.** Opening Concept mode for a checkout, or switching checkout with
  the pane open in Concept, starts a **full pass** when none has finished for
  that checkout in this app session. A full pass lists the files, compares each
  file's `(size, mtime_ns)` with the stored values, re-chunks only new or
  changed files, drops the chunks and the `files` row of a file that was removed,
  drops the chunks of a file that has become skipped (binary, non-UTF-8,
  over 1 MiB, or resolving outside the checkout) while recording its `files` row,
  and records every listed file so an unchanged skipped file is not read again.
  Content is committed in transactions of 100 files, so a stopped pass leaves a
  consistent index. While the pass runs, queries are answered immediately from
  what is indexed so far and the pane shows `Indexing done/total files`; when the
  pass finishes and the pane's query is unchanged, the query is issued again so
  results fill in.
- **Sync before every query.** Once a full pass has finished successfully for the
  checkout, each Concept query is preceded by a **sync**, a cheap working-tree
  catch-up on the indexer thread. It runs `git --no-optional-locks status --porcelain=v1 -z --untracked-files=all` through the same helper `git_view::changes` uses (the option keeps it from taking the `index.lock` that a `git add` or `git commit` by the user or an agent in that checkout needs),
  collects the distinct paths of every conflict, staged, unstaged, and untracked
  entry together with the source path of a rename or copy, and re-evaluates only
  those paths: a path whose `(size, mtime_ns)` equals the stored values is left
  alone without being read; a path that no longer exists has its chunks and its
  `files` row deleted (a deletion, or the old name of a rename); any other path
  is read with `git_view::file` and re-chunked, or, when it is not plain text
  within the limit or is refused, has its chunks dropped and its `files` row
  recorded. A copy's source is re-evaluated like any other path and, being
  unchanged, costs only a stat. Nothing is listed or stat-ed beyond the paths
  `git status` named. Ignored files never appear in `git status`.
- **Query deferral.** The query waits for the sync, for at most
  `SYNC_WAIT = 1 s`. When the sync finishes in time the query runs once, against
  an index that already reflects the working tree. When the bound expires first,
  the query is answered from the current index and issued again as soon as the
  sync finishes, if the input is unchanged. If the sync fails (for example `git`
  is missing or the checkout is not a repository), the query is answered from the
  current index and no error is shown for the sync; a full pass reports a
  persistent failure. The wait happens on the pane's state, never on the worker.
- **Order.** When a sync is needed and a full pass is also due, the sync is
  queued first, so the query waits only for the cheap job and not for the pass.
  While a pass is running, a query that needs a sync waits behind it only for
  the bound. A pass that finishes while a sync is pending does not issue the
  query again; the sync result does.
- **Coalescing.** A sync that is queued and has not started serves every query
  issued before it starts; a sync that is already running does not, because it
  may have read the working tree before the latest edit, so a new one is queued
  behind it.
  A pass is coalesced the same way per checkout: a pass already queued or running
  for a checkout serves a new request for it, and each checkout keeps its own
  record of when its last pass finished, so switching to another checkout and
  back does not start a new pass.
- **No throttle on syncs.** `REFRESH_INTERVAL = 10 s` applies only to the full
  pass, which stays as reconciliation: before a query, a full pass is started
  when none is running and either none has finished for the checkout in this
  session or the last one, whether it succeeded or failed, finished more than
  `REFRESH_INTERVAL` ago. A failed pass therefore retries at most once per
  interval, not on every query, and while it is failing queries are answered from
  the current index without a sync.
- Stale-reply rules are `RNE-backend-isolation`'s: results are scoped to the
  checkout and generation they were issued for.

## Edge cases

- Query shorter than two trimmed characters: nothing is issued (as Content).
- FTS5 syntax in the query (`"`, `*`, `-`, `:`, `AND`, `OR`, `NEAR`): treated as
  plain text by quoting each word.
- A file that changes or disappears between listing (or `git status`) and
  reading: skipped, picked up by the next sync or pass.
- A symlink that resolves outside the checkout: never indexed, and its target's
  content is never read (`git_view::file` refuses it).
- A file that becomes binary, non-UTF-8, over 1 MiB, or a symlink pointing out:
  its earlier chunks are removed by the next sync that names it or the next full
  pass.
- A rename: the destination is read and indexed and the old name, which no longer
  exists, has its chunks and `files` row deleted. A delete: its chunks and
  `files` row are deleted. A copy: the destination is indexed and the source is
  left as it is. A text file that turns binary: its chunks are dropped.
- A path with unchanged size and mtime is not read, so a sync of an unchanged
  repository reads no file; an edit that keeps both size and mtime is missed
  (ceiling).
- `git status` fails (git missing, not a repository, or a status too slow): the
  query is answered from the current index and, when the wait bound passes,
  issued again when the sync ends; the pane never blocks, and a failed sync shows
  no error of its own.
- A very large status (for example a big untracked tree): the query waits only
  the bound, then is answered from the current index; the sync continues on the
  indexer thread, honors the stop flag, adds a path only while the index holds
  fewer than `MAX_INDEXED_FILES` files, and re-issues the query when done.
- A chunk of only blank lines matched through its path: opens at the chunk's
  start line.
- Mode switch while a search is in flight: the reply is dropped and `Searching…`
  is cleared; nothing is left spinning. A sync result that arrives after the mode,
  query, or checkout changed is ignored.
- Index cannot be opened or is corrupt: a pass deletes the file and rebuilds once;
  if that also fails, the pane shows `Concept search unavailable: <reason>` and
  Content mode is unaffected. A query or sync that meets a corrupt file reports
  the same text for a query, does nothing for a sync, and never repairs it.
- `git` missing, or the checkout is not a repository: a full pass fails with the
  same error text Content search shows.
- Checkout switch during a build: the build for the earlier checkout finishes in
  the background; its progress and results are dropped for the live checkout.
- App exit during a pass or sync: the indexer stops at the next file boundary;
  the last committed batch remains valid.
- Two Goko processes on the same checkout: SQLite WAL and a 5 s busy timeout
  serialize writers; a pass and a sync are idempotent.
- A checkout directory that no longer exists (for example a removed worktree): its
  index file is deleted when the indexer thread starts, so at the next app start
  and not at the moment of removal.
- Git run by the user or an agent in the same checkout, for example a `git add` or
  `git commit` while a query's sync runs: the sync's `git status` takes no
  `index.lock`, so it cannot make that command fail; `git ls-files` and file
  reads take no lock.
- Several projects or worktrees: a worktree is a checkout like any other, with its own
  `git ls-files`, `git status`, and index file; Goko refuses a worktree path that
  overlaps another checkout, so a worktree it creates never appears in the main
  checkout's listing. Only a
  checkout whose Concept mode is used is indexed. Disk use grows with each
  indexed checkout (several times its text size, not yet measured) and nothing
  evicts an index while its checkout directory exists, including one whose
  project was removed from Goko. One indexer thread serves every checkout in
  order, so a long first pass for one checkout delays the sync of another by up
  to its duration; that query then waits only `SYNC_WAIT`, is answered from the
  current index, and is issued again when its sync finishes (ceiling).
- Index contents mirror indexed source files, so a secret present in a tracked
  or untracked non-ignored file is also stored under `GOKO_HOME/search/` (mode
  `0600`, same protection as `goko.db`).
- Ceilings: `git status` names only paths that differ from `HEAD` or are
  untracked, so a file reverted to `HEAD`, a branch switch, a pull, or a stash
  changes the working tree without appearing in it; those are corrected by the
  next full pass, within `REFRESH_INTERVAL` plus the pass time, and the query
  is issued again when it finishes. Lockfiles and generated files are indexed
  like any text file; no stemming beyond Porter; ranking is word-based, not
  semantic.

## Interfaces

- `crates/app-core/src/code_index.rs` (new):
  `refresh(home, checkout, progress)` runs one full pass and calls
  `progress(done, total)` after every file; the callback returns `false` to stop
  the pass at that file boundary. It delegates to a private
  `refresh_with(home, checkout, limit, progress)` so tests can inject a small
  file limit. `sync_changed(home, checkout, stop)` runs one sync and returns
  counts of paths re-chunked, removed, and left unchanged. `search(home,
  checkout, query)` returns `GitViewSearch` (reusing `GitViewSearchMatch`) from
  the index. All three canonicalize the checkout themselves, because the indexer
  thread receives the sidebar's path and the worker receives the validated one.
  A private index opener takes a mode: read-only for `search`, write-to-an-
  existing-matching-index for `sync_changed`, create-or-rebuild for `refresh`
  only. Both `refresh` and `sync_changed` re-evaluate a path through one shared
  private function, so the skip rules, the `files` row, and the chunk replacement
  are written once. Listing and reading reuse `git_view::file_list` and
  `git_view::file`.
- `crates/app-core/src/git_view.rs`: a new `pub(crate) fn status(checkout)` that
  runs the existing `git status` call, now with `--no-optional-locks`, and parse and returns `GitViewChanges`
  without the branch, head, and ahead lookups; `changes` calls it and then adds
  those three, so its output is unchanged (it no longer refreshes the index opportunistically). `SEARCH_PREVIEW_CHARS` becomes
  `pub(crate)`.
- `crates/app-core/src/store.rs`: `Storage::home()`, a new accessor for the
  `GOKO_HOME` the store was opened with, so the worker can locate
  `GOKO_HOME/search/` without reading the environment (tests pass a temporary
  home). No schema or migration change.
- `apps/rune/src/editor/backend.rs`: `Req::ConceptSearch { query }`, run on the
  existing worker through `code_index::search` and answered with
  `ReplyData::Search`; `backend/apply.rs` applies it to `files.search.results`
  exactly as `Req::Search`, each arm also requiring the pane's mode to match its
  request kind.
- `apps/rune/src/editor/search.rs`: a `SearchMode { Content, Concept }`, an
  `IndexStatus { Idle, Building { done, total }, Truncated, Failed(String) }`, and
  a sync state (`Idle`, `Waiting { query, deadline }`, `Expired { query }`) in
  `search_state`; the indexer thread with its two job kinds, pass and sync, and a
  set of checkouts that already have a sync queued; an `IndexEvent` (progress,
  done, failed, synced) the thread posts through a sink, with the pass body, the
  sync body, and the function that applies an event to the state written as plain
  functions so tests can drive them without an app loop; `set_mode`,
  `force_content`, `ensure_index(state, now)`, and `poll(state, now, tx)`, which
  holds the deferral logic and takes the app clock `now` (seconds) and the `tx`
  that `main.rs`'s `poll` already has (its call passes them). Times kept in the
  state, the last finished pass and the sync deadline, are seconds on that clock.
- `apps/rune/src/editor/navigation.rs` and `apps/rune/src/editor/keyboard.rs`:
  `⇧⌘F` calls `force_content` first; the go-to-definition fallback calls it only
  in the branch that shows the pane, after the unique-declaration return and
  before it writes the query and results.
- `apps/rune/src/editor/files.rs`: the mode switch and the status line in the
  Search pane.
- No change to `gokod`, `crates/protocol`, `goko.db`, or any dependency.

## Verification

- `crates/app-core` tests on a temporary Git repository (the pattern in
  `git_view` tests): the indexed set (tracked, untracked, ignored, binary, over
  1 MiB, a symlink pointing outside the checkout); ranking (`reconnect backoff`
  ranks the identifier chunk above a comment-only chunk, and `ReconnectBackoff`
  and `reconnect_backoff_ms` both match; a plain prose word never reaches
  `idents`); incremental refresh (edit, add, delete one file each re-chunks only
  those; a file that turns binary loses its chunks; a removed file loses its
  `files` row); sync (an added, an edited, a deleted, a renamed, a copied, and a
  text-to-binary file are each reflected by `sync_changed` alone with only those
  files re-chunked; a repository with no changes re-chunks nothing, shown by an
  unreadable file with unchanged stat not failing the sync; a non-repository
  returns `Err`; a path outside the file cap is not added; no index means no
  writes); a checkout given through a symlinked path and through its canonical
  path use one index file; a corrupt or version-mismatched index is rebuilt by a
  pass while a query never writes or creates anything; hostile query text never
  errors; partial results while a pass runs; the file-limit behavior through
  `refresh_with`; FTS5 is available in the linked SQLite (create a virtual table
  in a test); the existing `changes` tests still pass after the `status`
  extraction.
- `apps/rune` state tests (the pattern near `files.rs` tests): mode switch in
  both directions (including clearing `searching` while a reply is in flight and
  re-arming the existing input), debounce and minimum length reuse, stale reply
  dropped on checkout switch, status line, re-issue after a pass completes,
  `⇧⌘F` and the go-to-definition fallback forcing Content only where they show
  the pane (a unique declaration, a stale reply, and a non-Files section leave a
  Concept pane alone), the sync deferral (the query is issued when the sync
  finishes, after the bound with the current index and again on completion, and
  immediately while a pass runs or after a failed pass), the full-pass throttle
  (a failed pass is retried once per interval, not on every query), and sync
  coalescing. The indexer is tested through its pass and sync functions with a
  collecting sink plus the function that applies an event to the state, because
  `AppHandle::update` only queues when no app loop runs; the thread wiring is
  checked in the real app.
- `cargo fmt --all -- --check`, `cargo clippy --workspace -- -D warnings`, and
  `cargo test --workspace`.
- The real app, per `AGENTS.md`: build `target/Goko.app`, check with
  `agent-desktop` on the Goko window only that Concept results appear, rank, and
  open at the right line, that a file added or edited a moment before the query is
  found by the first reply, and that Content search is unchanged.

## Related context

- [Rune editor](../../../../llm/domains/rune-editor.md): `RNE-content-search` (the
  Content mode this sits beside), `RNE-backend-isolation` (worker and
  stale-reply rules; the indexer thread is a stated exception),
  `RNE-go-to-definition` and `RNE-keyboard` (the two entries that must show
  Content mode).
- [Architecture overview](../../../../llm/architecture/overview.md): its one-line
  description of the native shell's single bounded worker thread.
