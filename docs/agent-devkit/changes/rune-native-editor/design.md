# Native Rune editor

## Goal

Port the usable desktop editor from `main` commit `1fc1005a90f931f2cd2de9982e808b95542b6822` to native Rune. Preserve its file/Git behavior and protections; use Rune/rune-kit components rather than an embedded WebView. Vim is explicitly deferred by the user on 2026-11-09.

## Scope

Files, Changes, History and Stashes; native text editing, content search and go-to-definition; Git writes; ACP commit-message generation; real registered project/checkout selection for the editor. Keep the Tauri application working with the same command names, payloads, database and daemon protocol.

Out of scope: Vim, LSP, autocomplete, new stash mutations, and migration of the mock terminal/session system. Support the editor's external open-file/line request interface now; a live native terminal producer is a separate change. Preserve `editor.vim` in shared settings for Tauri and expose no active Vim control in Rune.

### Component mapping

| Main feature | Native implementation |
|---|---|
| Files tree | rune-kit `FileTree`/`Tree`, existing file-list/filter semantics |
| CodeMirror surface | Rune `TextEdit`/`text_input`, native selection/IME/clipboard/undo; custom line-number gutter, find panel and language decorations |
| Patch panes | rune-kit `diff::parse`, `DiffState`, `DiffView` in unified/split mode; preserve binary/truncated notices |
| History table | rune-kit `Table`, `TableState`, `TableColumn`; retain Git topological order, selection, keyboard navigation and resize behavior |
| Commit graph | Port `history-graph.ts` lane/color algorithm; render custom `Cell::el` cells with Rune `canvas`/`Shape`/`Path` |
| Commit details / Stashes | Native list/detail composition with the shared patch pane; reuse existing Rust reads |
| Filters, confirmation, previews | rune-kit input/select/menu/dialog/Markdown components, themed with the existing Rune palette |

## Behavior

### Workspace and real data

The editor selects registered projects and validated root/listed worktrees from the existing `goko.db` under the same `GOKO_HOME` rules as Tauri. Offer a native folder picker to register a project when necessary. Mock project IDs and session paths never authorize file or Git operations. Keep current terminal/session chrome behavior outside the editor; editor selection is explicit and independent of mock session selection.

Open Changes by default, with Files, History and Stashes available. Retain section state while switching; query only active sections, refresh affected data after writes, retain usable cached data with a stale/error notice on refresh failure. Keep keyboard shortcuts from main, including Save, contextual Find, section selection and list navigation; typing into a field must not trigger list shortcuts. Settings navigation and app/window close go through the dirty-draft guard.

### Files and editing

Preserve lazy tree expansion, filename filtering, content-search debounce/limits, Markdown preview, show-changes navigation, line reveal and main's heuristic import/declaration lookup. Include the external file/line request behavior added on main but missing from beta-rune; consume requests once and validate checkout/path before loading.

Native editing includes line numbers, horizontal/vertical scrolling, selection, clipboard, undo/redo, IME and Vietnamese text input. Saving uses the existing UTF-8/NUL/1 MiB limits, expected-original-text check and atomic replacement. Binary, truncated, inaccessible or read-only files stay non-editable as appropriate. Navigation offers Save/Discard/Cancel; failed Save keeps the draft and blocks navigation. Preserve main's newline handling.

Highlight supported languages with rune-highlights lexers across the complete document — the rune-full-highlight delta superseded the original rune-kit 32 KiB/8192-range scanner budget. Colouring is bounded only by a 131072-range safety budget; show a visible notice when that budget is reached, and the complete file remains editable up to the existing 1 MiB file limit. Unknown or failed syntax support also stays usable as plain text with the fallback visible. This is an explicit proposed difference from CodeMirror, alongside the deferred Vim support; do not imply full CodeMirror language/highlighting parity. Do not split source into independent chunks that miscolor multiline comments/strings. Compute decorations outside the UI thread and apply results only to the document revision they describe.

### Changes, History and Stashes

Preserve conflict/staged/unstaged/untracked groups, row and group stage/unstage actions, named-file discard confirmation, commit message draft, hooks, push without force/prompts and existing timeout/error handling. Preserve ACP agent selection/connect, Generate/Cancel, generation failure and stale-job behavior; generation never commits automatically.

History preserves branch filtering, paging, lane colors and merge edges, distinct HEAD, uncommitted row, match dimming/jumping, parent navigation, commit metadata/body/files and patches. Use the existing graph algorithm and fixtures as the parity oracle. Restore column widths and detail-pane size using app settings with the same defaults/clamps as main; Rune need not import browser `localStorage`. Stashes remain repository-wide inspection with selection, filtering, metadata and patch detail, with no new write actions.

## Edge cases

- Missing/non-Git/unregistered projects or checkouts: explain the state, allow retry/selection, perform no file/Git write.
- Escaping paths, symlinks, `.git`, invalid refs, file caps, disk conflicts and atomic-save failures: preserve current Rust validation/errors and draft text.
- Switch checkout/file/section or refresh during a request: discard stale read/highlight results; serialize writes for the affected checkout and refresh it even if it is no longer selected. A stale result must never replace another checkout's state.
- Empty/unborn repositories, merge/conflict status, binary/renamed/deleted files, truncated patches and missing remote/credentials/hooks: retain main's behavior and notices.
- Storage unavailable or malformed layout: show the storage error, use bounded in-memory layout defaults, never replace/delete the database. Keep existing setting values and schema.
- ACP cancellation, agent exit, denied permission, timeout and shutdown: retain existing process cleanup, isolation and permission denial. Failed/cancelled generation keeps the commit draft; app exit shuts down owned workers/connections after the draft guard resolves.

## Interfaces

Extract a Tauri-free `crates/app-core` containing the existing error/store, file/Git read/write, agent/ACP logic and registered-checkout validation. Split shared Git runner/read discovery from `worktrees.rs` operations that require `Bridge`; those daemon/session operations stay desktop-owned. Keep Tauri modules as reexports/adapters where needed so command signatures and callers remain stable. Move tests with their implementation and preserve fake-agent fixtures. Do not duplicate the implementations or make Rune depend on the Tauri crate.

Rune owns `apps/rune/src/editor/` for state, views, draft/navigation guards, graph layout/rendering and request coordination. Extend `main.rs`, `chrome.rs`, `data.rs` and `theme.rs` only to mount/theme the editor and select real editor checkouts. Reuse the desktop home-directory resolution via app-core while leaving daemon paths/protocol unchanged.

Blocking Git/FS work runs outside the UI thread. A bounded worker/result channel carries checkout identity plus request generation/document revision; Rune wake/update hooks apply current results. ACP uses its existing Tokio-based hub on an owned runtime. Serialize mutating actions, bound pending read work, and shut workers down cleanly. Reuse workspace dependencies; no new editor engine or framework is planned.

## Verification

Before source edits, record relevant existing Rust/frontend baselines. Move existing backend tests without weakening assertions; port graph/layout, go-to-definition and navigation cases from main. Add focused native state tests for stale responses, revision-specific syntax, dirty navigation and external open requests. Use temporary repositories for Git operations, never the user's checkout.

Run `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`, `cargo build --workspace`, and desktop tests/typecheck/build after extraction and integration. Native manual QA covers all four sections, real root/worktree selection, Vietnamese IME, clipboard/undo/scroll, save/conflict/close guards, full-document highlighting on files beyond 32 KiB with complete content still editable and the range-budget notice on pathological input, graph continuity during paging/scrolling, layout restoration, ACP cancel/failure and safe Git write behavior. Compare behavior against the pinned main editor, accounting for the two explicit Vim/syntax differences; passing unit tests alone does not establish native interaction quality.

Update Rune chrome's view-switch requirement and add verified native editor wiki coverage after implementation. Keep the Tauri editor wiki accurate for Tauri; do not rewrite its CodeMirror requirement as a claim about Rune.

## Related context

- [Desktop Editor](../../../llm/architecture/editor.md)
- [Rune chrome](../../../llm/domains/rune-chrome.md)
- [Architecture overview](../../../llm/architecture/overview.md)
