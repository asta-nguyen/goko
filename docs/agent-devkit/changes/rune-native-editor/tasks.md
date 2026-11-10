# Native Rune editor tasks

## Design

[Native Rune editor](design.md)

## Global Constraints

- Preserve its file/Git behavior and protections; use Rune/rune-kit components rather than an embedded WebView.
- Vim is explicitly deferred by the user on 2026-11-09.
- Keep the Tauri application working with the same command names, payloads, database and daemon protocol.
- Mock project IDs and session paths never authorize file or Git operations.
- Blocking Git/FS work runs outside the UI thread.

## Impact map

Entry: `apps/rune/src/main.rs::view` currently renders a placeholder for `View::Editor`; planned entry pending approval: `editor::workspace` in the sibling design's Interfaces section.

Flow: current Rune view switch → placeholder; reference main `EditorWorkspace` → Files/Changes/History/Stashes → frontend services → Tauri command validation → `git_view`/`git_write`/`AcpHub`. Planned pending approval: native editor state → background requests → shared validated app-core operations → revision/checkout-scoped results → native components.

State changes: planned pending approval: real project registration in existing Store; editor drafts and per-checkout/section state in memory; layout/preferences in existing settings. No schema or protocol change.

External effects: planned pending approval: validated file reads/atomic writes, Git inspection/index/commit/push, ACP child processes and existing SQLite storage. Terminal/daemon session migration excluded.

Change candidates: planned pending approval: root `Cargo.toml`/`Cargo.lock`; new `crates/app-core`; desktop Cargo manifest and `src/{error,store,git_view,git_write,worktrees,agents,acp,daemon_client,commands,lib,test_support,wiring_tests}.rs` as extraction/adapters require; backend fixture locations; Rune Cargo manifest, `src/{main,chrome,data,theme}.rs`, new `src/editor/`; wiki pages named in design.

Verification: planned pending approval: existing Rust/backend/frontend suites, native state/graph/navigation tests and manual scenarios in design and tasks below.

Verified at: `01aa1de037e9a90f51d6a64babec6a1d3729e189`; reference main `1fc1005a90f931f2cd2de9982e808b95542b6822`. Recheck status and source before implementation. Main's external open-file handling differs from this branch and must be ported from main explicitly.

## Tasks

Baseline recorded 2026-11-09 before source edits: `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`, `cargo build --workspace` all passed; desktop test (42 files, 450 tests), typecheck and build all passed. Logs: `/tmp/goko-rune-native-baseline/`. No pre-existing failures observed.

### Task 1 — Share the validated backend without Tauri

Files: new `crates/app-core::{error,store,git_view,git_write,worktrees,agents,acp,projects,home}`; desktop `src/{commands,daemon_client,worktrees,lib}.rs` and existing implementation modules; workspace/desktop/Rune Cargo manifests and moved backend tests/fixtures.
Interfaces: consumes existing Rust implementations; produces Tauri-free Store, Git/file/ACP operations, home resolution and registered-project/checkout validation with unchanged Tauri wire shapes.
Change: record baseline checks, extract implementation once, keep Bridge-dependent worktree mutations in desktop and preserve adapters/reexports. Retain current SQLite schema, file permissions, path/ref checks, Git caps/timeouts and ACP isolation. Retain existing desktop callers and public behavior.
Verify: moved file-save tests cover NUL/caps, traversal/symlink escape, stale expected text, permissions and atomic failure; moved Git tests cover stage/unstage/discard/commit/hooks/push; moved ACP fixture tests cover permissions, cancellation, timeouts and cleanup; existing desktop wiring/store/worktree suites still pass. Run workspace fmt/clippy/test/build and desktop tests/typecheck/build.
Progress: done
Result: 2026-11-09 — backend extraction verified with app-core 112 tests (107 moved + 5 new), desktop 116 tests (223 baseline = 116 retained + 107 moved), full workspace test/build/check/fmt/canonical clippy and desktop 450 tests/typecheck/build passed. Review repair restored the public desktop::commands::ProjectView path; cargo check -p desktop and owned-file rustfmt passed afterwards. Independent static extraction review found no actionable defects and verified all 36 command functions, moved guards/tests and the fixture rename against baseline. Final integrated review remains outstanding. Additional all-targets clippy observes the unchanged daemon pty.rs items-after-test-module lint; canonical required clippy passes.

### Task 2 — Mount native editor with real checkout selection and workers

Files: `apps/rune/src/main.rs::{State,view}`, `chrome.rs` view switch, `data.rs`, `theme.rs`; new `editor/{mod,state,backend,workspace}.rs`.
Interfaces: consumes app-core storage/project/worktree operations; produces real selected editor checkout, four-section workspace, bounded job/result coordinator and ACP runtime ownership.
Change: use existing GOKO_HOME data and native folder registration/checkout selection; never dispatch operations using mock IDs. Preserve terminal mock behavior outside editor. Keep per-section state, active-only reads, loading/empty/stale/error/retry states. Token results by checkout/request; serialize mutations and close owned workers cleanly.
Verify: native state tests for no projects, invalid/missing/non-Git checkout, storage failure, stale checkout results, bounded pending work, repeated actions and worker shutdown; manual real registered root/worktree selection and error/retry. Confirm no Git/FS work blocks UI rendering.
Progress: todo
Result: none

### Task 3 — Native file browser, safe drafts and navigation guards

Files: new `editor/{files,draft,navigation}.rs`; `main.rs::State` close/navigation integration.
Interfaces: consumes validated tree/file/save operations and worker coordinator; produces editable draft, expected original text, Save/Discard/Cancel guard and once-only file/line request handling.
Change: use FileTree/Tree for lazy browsing and filename filtering; support Markdown preview and show-changes handoff. Preserve UTF-8/newline handling, 1 MiB/NUL/read-only/binary/truncated restrictions, atomic save and disk-conflict errors. Guard file/checkout/settings/view changes and app/window close; failed Save retains draft and blocks navigation.
Verify: port main EditorFiles/workspace/store guard cases: Save/Discard/Cancel, save error, disk edit, binary/truncated file, newline round trip, switch checkout while saving, close with dirty draft, valid/invalid external request, same-file reveal, consumed request not replayed after remount. Manual native close dialog and reload conflict.
Progress: todo
Result: none

### Task 4 — Native code surface, syntax and in-file find

Files: new `editor/{code,syntax,find}.rs`; `theme.rs` editor styles.
Interfaces: consumes draft text/revision, language/path, save/reveal requests; produces revision-specific edits/decorations, selection and find/reveal state.
Change: compose TextEdit/text_input with native line-number gutter and synchronized scrolling. Preserve IME, clipboard, undo/redo, keyboard Save and contextual Find. Highlight with rune-highlights lexers across the complete document within a 131072-range safety budget (the rune-full-highlight delta superseded the 32 KiB/8192-range scanner budget), show a capped notice only when that budget is reached, and keep the complete file editable up to 1 MiB. Unknown/failed syntax visibly falls back to plain text; never split multiline language state into independent chunks. Never add Vim controls or alter stored editor.vim.
Verify: native tests for multibyte byte-boundaries, undo groups, reveal clamping, find navigation and stale syntax results; fixtures for multiline comments/strings, documents past the retired 32 KiB prefix highlighting through the tail, and documents reaching the range budget, with notice plus unchanged complete content up to 1 MiB; manual Vietnamese IME, clipboard, long-line scroll, gutter alignment, select-all/undo, read-only mode and foreground responsiveness. Confirm Tauri Vim setting still works unchanged.
Progress: todo
Result: none

### Task 5 — Content search and code navigation

Files: new `editor/{search,go_to_definition}.rs`; `files.rs`, `code.rs`, `navigation.rs`.
Interfaces: consumes shared file-list/content-search reads, native token hit-testing and guarded file/line requests; produces import/declaration targets or content-search fallback.
Change: port main's trimmed two-character/250 ms content search, result caps/line reveal, relative import/extension/index and nearest config-alias resolution. Preserve unique/current-file declaration rules and ambiguous-name fallback. Ignore comment/string names and blank-space clicks; use Cmd+click on macOS and Ctrl+click elsewhere ahead of native selection behavior.
Verify: port main go-to-definition fixtures for relative/index/JS-to-TS imports, aliases, missing import, unique/ambiguous/local declaration and ignored strings/comments; test stale searches/lookups and dirty navigation cancellation. Manual modifier underline/hit-testing and result-to-line focus.
Progress: todo
Result: none

### Task 6 — Changes and reusable native patch panes

Files: new `editor/{changes,patch,commit}.rs`; `workspace.rs`, worker mutation handling.
Interfaces: consumes app-core status/diff/stage/unstage/discard/commit/push; produces shared DiffView-based patch pane and guarded Git actions.
Change: use rune-kit DiffState/DiffView with unified/split presentation, preserving binary/truncated/rename/deletion notices. Match main status groups, filter, row/group action scope, named-file discard confirmation, commit draft/hooks and push labels/timeouts. Refresh the affected checkout after success without applying its data to a newly selected checkout.
Verify: port main workspace/CommitBox cases for row/group action scope, cancel discard, directory refusal, failed commit keeping message/index, disabled/repeated actions, push error and no force/prompt behavior. Test patch fixtures for root commit, rename, binary and truncated content. Run Git tests only against temp repositories; manually inspect native split/unified selection and refresh.
Progress: todo
Result: none

### Task 7 — ACP commit-message generation

Files: new `editor/acp.rs`; `commit.rs`, `backend.rs` runtime lifecycle.
Interfaces: consumes app-core AcpHub and existing stored agent configuration; produces connect/status/Generate/Cancel controls and conditional draft replacement.
Change: preserve main picker/fallback, explicit connect/generate, isolated temporary session and denied permissions. Generation only fills the unchanged target draft; edit, checkout switch, cancel, timeout or exit must not overwrite another draft. Cancel jobs and shut owned processes/runtime down on exit.
Verify: existing fake-agent tests plus native state tests for connect failure, generation success, edited draft, cancelled/replaced job, wrong checkout, empty/refused reply and shutdown. Manual Generate/Cancel with a configured agent; verify no automatic commit or access-permission approval.
Progress: todo
Result: none

### Task 8 — History table, graph and details

Files: new `editor/{history,history_graph,history_layout,commit_detail}.rs`; shared `patch.rs`.
Interfaces: consumes app-core branches/history/commit detail, shared patch pane and settings; produces Table-based history, custom graph cells and restored layout.
Change: use Table/TableState/TableColumn and custom element cells; preserve Git row order instead of enabling arbitrary sorting. Port lane/color/edge geometry from main; draw with Rune canvas/Shape/Path, maintaining continuity across virtual rows and paging. Preserve branch filtering, match dimming/jump, HEAD/uncommitted, metadata/body/parents/files, column and detail resizing/reset. Persist bounded layout values in existing settings; do not migrate browser storage.
Verify: port history-graph/layout fixtures for linear/merge/octopus, lane reuse/color continuity, duplicate-parent lanes, pagination and malformed/out-of-range persistence. Port history UI cases for branch/reset, dimmed matches, Jump to HEAD, uncommitted fallback, parent selection and stale refresh. Manual graph continuity at viewport boundaries, table keyboard behavior and resize restoration on restart.
Progress: todo
Result: none

### Task 9 — Stashes and integrated parity documentation

Files: new `editor/stashes.rs`; `workspace.rs`, shared `patch.rs`; `ARCHITECTURE.md`, `docs/llm/architecture/overview.md`, `docs/llm/domains/rune-chrome.md`, new `docs/llm/domains/rune-editor.md`, `docs/llm/INDEX.md`.
Interfaces: consumes shared repository-wide stashes/stash-diff reads and native patch pane; produces Stashes list/detail and verified native editor documentation.
Change: preserve main selection/filter/metadata/patch behavior, state retention, stale/error/refresh states and repository-wide worktree sharing. Implement no new stash mutations. Run document-wiki Task updates, merge sibling delta and register a distinct Rune editor requirement prefix with evidence from delivered native source/tests. Document the explicit Vim deferral and terminal-link producer boundary.
Verify: native tests for empty/filter/selection/error/stale stash data and same-repository worktrees. Perform all native manual scenarios in design plus main parity comparison; run full Rust and desktop checks, and the document-wiki validator. Record native interaction limits honestly; do not claim delivery from backend tests alone.
Progress: todo
Result: none

## Files inspected, no change

- `apps/desktop/src/features/editor/{EditorWorkspace,EditorFiles,CodeEditor,EditorChanges,EditorHistory,EditorStashes,HistoryTable,CommitBox}.tsx` — reference behavior, with main's external-open additions read separately.
- `apps/desktop/src/features/editor/{history-graph,history-layout,go-to-definition}.ts` and associated tests — parity oracles; TS implementation remains for Tauri.
- `apps/desktop/src/stores/ui.ts` — draft/navigation semantics to preserve.
- Rune/rune-kit pinned `a7f276c` sources: `table.rs`, `diff/*`, `text_input.rs`, `syntax.rs`, Rune `edit.rs`, `edit/history.rs`, `shape.rs`, `runtime/cx.rs` — existing native primitives; no vendored edits planned.
- `crates/daemon/src/paths.rs` — same GOKO_HOME contract; daemon implementation remains unchanged.

## Approval Gate

Status: approved

Approved by user: 2026-11-09 — "implement đi" after the design, tasks and delta were presented; includes the disclosed native syntax budget and Vim deferral.

## Decision Log

- 2026-11-09: User selected pure native Rune; no embedded editor WebView.
- 2026-11-09: User explicitly deferred Vim. This settles scope, not joint approval of these documents.
- 2026-11-09: Proposed pending joint approval: use existing rune-kit Table/DiffView/Tree and native controls; custom graph geometry/rendering and editor-specific composition. Reuse native syntax within its 32 KiB/8192-range budget with a visible partial-highlighting notice, keeping complete files editable; this is an explicit CodeMirror parity difference, not yet user-approved.
