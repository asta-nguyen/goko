# Changes and History view layout tasks

## Design
[Changes and History: View (List/Tree) and Group By menu](design.md)

## Global Constraints
- UI only: no change to `gokod`, `crates/protocol`, `goko.db`, `crates/app-core`, or any dependency.
- State: `ChangesState.view: ListView`, `ChangesState.group: ChangeGroup`, `HistoryState.view: ListView`; `ListView { List, Tree }`, `ChangeGroup { None, TrackedUntracked, StagedUnstaged }`; defaults `List`/`None`; in memory only, never persisted.
- Menu: the `context_menu`/`MenuItem` pattern in `files.rs`; the active option marked; Changes gets `View` and `Group By`; the History commit detail gets `View` only.
- Tree: files nest under directories by path; folder rows expand/collapse; file rows keep select, patch load, stage/unstage, and open.
- Grouping (Changes): `Tracked & Untracked` = (staged, unstaged, conflicts) vs untracked; `Staged & Unstaged` = staged + conflicts vs unstaged + untracked; a path in both appears in both; empty groups are not rendered.
- The existing filter and keyboard navigation apply to the rendered rows; selection is by row identity, not by path.
- The diff rendering, the Files tree, and the History commit list are unchanged.

## Impact map
Entry: `apps/rune/src/editor/changes.rs` (the Changes changed-file list renderer and its context menu) and `apps/rune/src/editor/history.rs` (the commit detail's changed-file list).
Flow: `Req::Changes` → `GitViewChanges` → the list renderer → selection → `Req::Diff` patch → `patch.rs`; the History detail's `GitViewCommitDetail.files` → the detail list. Planned (pending approval): the renderer consults `ChangesState.view/group` and `HistoryState.view`.
State changes: in-memory `ChangesState.view/group`, `HistoryState.view`.
External effects: none.
Change candidates: `apps/rune/src/editor/changes.rs`, `apps/rune/src/editor/history.rs`, `apps/rune/src/editor/state.rs`, `CHANGELOG.md`, `ARCHITECTURE.md`, `docs/llm/domains/rune-editor.md`.
Verification: `cargo test -p goko-rune changes history`, `cargo fmt --all -- --check`, `cargo clippy -p app-core -p goko-rune -- -D warnings`, `cargo test --workspace --no-fail-fast`; real-app check with `agent-desktop`.
Verified at: 87245bde2a773a0e13f40152c6a01c9186d86433

## Tasks
### Task 1 — State and context menu
Files: `apps/rune/src/editor/state.rs` (`ChangesState`, `HistoryState`, `ListView`, `ChangeGroup`), `apps/rune/src/editor/changes.rs` (the list context menu), `apps/rune/src/editor/history.rs` (the detail context menu).
Interfaces: produces the per-section state and the menu that writes it; Task 2 and Task 3 consume them.
Change: add the enums and fields with `List`/`None` defaults; right-click on the Changes list opens `View` (`List`, `Tree`) and `Group By` (`None`, `Tracked & Untracked`, `Staged & Unstaged`), the active option marked; the History detail opens `View` (`List`, `Tree`); choosing an option writes the state and redraws.
Verify: `cargo test -p goko-rune changes history` in the `Runtime` harness: the menu lists the options, marks the active one, and a choice updates the state; the defaults are `List`/`None`.
Progress: done
Result: `ListView`/`ChangeGroup`/`ListMenu` added; `ChangesState.view/group/menu` and `HistoryState.view/file_menu/collapsed_dirs` live per section and reset correctly (`ChangesState::reset()` keeps `view`/`group` across checkout switches; `navigation.rs` calls it on both reset paths). Both menus open on secondary press — Changes mounts its own `rune_kit::root` layer, the detail menu mounts inside `history_view_impl` beside `row_menu` — with `menuitemcheckbox` items checked on the active pick. `secondary_press_opens_the_list_menu`, `menu_items_switch_view_and_group`, `the_list_menu_opens_on_an_empty_list`, `changes_state_defaults_and_reset_keeps_layout`, `secondary_press_on_file_list_opens_view_menu`, `empty_file_list_still_takes_the_menu` all pass. The stash detail shares `commit_detail::changed_files` via `ChangedFileHost::Flat` so it keeps its flat list with no History state leak.

### Task 2 — Changes List/Tree and grouping
Files: `apps/rune/src/editor/changes.rs` (the renderer and tests).
Interfaces: consumes Task 1's state.
Change: `List` keeps the flat rows; `Tree` builds folder rows from the entries' paths and nests the file rows; `Group By` renders the group headers and rows per the Global Constraints; a path in both staged and unstaged appears in both; empty groups are not rendered; the filter, keyboard navigation, selection, patch load, stage/unstage, and open work on the rendered rows.
Verify: `cargo test -p goko-rune changes` in the `Runtime` harness: Tree renders folder headers and nested files and List flat rows; each group's headers and rows (a both-staged-and-unstaged path twice); selecting a file row still loads its patch; the filter applies to the rendered rows.
Progress: done
Result: `list_groups` renders the canonical four in `None`, `Tracked`(staged+unstaged+conflicts)/`Untracked` and `Staged`(staged+conflicts)/`Unstaged`(unstaged+untracked) composites with no empty groups, each entry keeping its own key/layer so row actions, selection and writes stay per-layer. `tree_rows` nests entries under dirs (subdirs first, `natural_cmp`), dir rows toggle `dir:{path}` in `changes.collapsed`, and `keyboard::nav_items` mirrors the same groups/filter/view so j/k lands on mounted rows. `list_groups_*`, `tree_rows_*`, `grouped_view_keeps_each_rows_layer` (a both-staged-and-unstaged path selects each layer), `tree_view_dir_rows_expand_and_collapse` all pass. Group-collapse and the `MAX_GROUP_RENDER` cap behave per rendered group as before.

### Task 3 — History detail List/Tree
Files: `apps/rune/src/editor/history.rs` (the detail renderer and tests).
Interfaces: consumes Task 1's `HistoryState.view`.
Change: the commit detail's changed-file list renders flat in `List` and nested under directories in `Tree`; the existing selection, open, and patch path is unchanged.
Verify: `cargo test -p goko-rune history`: the detail switches List/Tree; a nested file row opens its patch.
Progress: done
Result: `commit_detail::changed_files` takes `ChangedFileHost::{History,Flat}`: `History` reads `history.view`/`collapsed_dirs` and mounts the file list's `View` menu on secondary press (also on an empty list, which now still mounts for the menu); `Flat` (the stash detail) renders the plain list unchanged. `tree_view_nests_detail_files` (dir rows + collapse) and `empty_file_list_still_takes_the_menu` pass; row press still calls `diff.scroll_to_file`.

### Task 4 — Documentation and real-app check
Files: `CHANGELOG.md` (`## [Unreleased]`), `ARCHITECTURE.md` (the Changes/History bullet), `docs/llm/domains/rune-editor.md` (via `document-wiki`'s `## Task updates`, applying `delta.md`).
Interfaces: consumes Tasks 1–3.
Change: add an `Added` entry; describe the List/Tree and Group By menu; merge `RNE-changes-view-layout` into the Rune editor page, add the source files to `## Sources`, and run `document-wiki`'s `## Task updates`; build a uniquely named app copy with a temporary `GOKO_HOME` and a fixture repository, and with `agent-desktop` on that window only check the menu, Tree/List, each group, the History detail, and a row click.
Verify: `cargo fmt --all -- --check`, `cargo clippy -p app-core -p goko-rune -- -D warnings` (only the pre-existing `quit_guard` errors), `cargo test --workspace --no-fail-fast` (only the pre-existing `rune-kit` failure); the real-app steps produce the expected results, recorded in `Result`.
Progress: done
Result: `CHANGELOG.md`, `ARCHITECTURE.md`, `docs/llm/domains/rune-editor.md` (`RNE-changes-view-layout` + a header-button scenario + a shared-choice scenario) and this file are updated; the wiki validator reports no error for the new block (167 pre-existing repository-wide errors remain). Real app (`/tmp/GokoViewQA.app`, unique bundle `dev.goko.rune.viewqa`, temporary `GOKO_HOME`, fixture repo with a path both staged and unstaged): the section opens in `Tree` with `Staged & Unstaged` by default; the header view button opens the anchored menu with `Tree`/`Staged & Unstaged` checked; `List` flattens the rows and the History commit detail follows flat; picking `Tree` in History turns the Changes list back to directory rows; the History detail menu has `View` only; directory rows collapse; `dup.rs` stays in both groups; a row click still loads its patch. Full workspace tests: only the pre-existing `rune-kit` markdown failure; `cargo fmt --all -- --check` clean; clippy only the pre-existing `quit_guard` errors.

## Files inspected, no change
- `apps/rune/src/editor/files.rs` — the `context_menu`/`MenuItem` pattern and the Files tree style reused.
- `crates/app-core/src/git_view.rs` — `GitViewChanges`, `StatusEntry`, and `GitViewCommitDetail.files`.

## Approval Gate
Status: approved

## Decision Log
- 2026-11-13 — Created from GitHub issue #7 and the user's screenshot of the Changes diff context menu (`View`: `List`/`Tree`; `Group By`: `None`/`Tracked & Untracked`/`Staged & Unstaged`); the History commit detail gets the `View` menu. Gate pending.
- 2026-11-13 — User approved `design.md`, `tasks.md`, and `delta.md` together after the summary was presented; implementation authorized.
- 2026-11-13 — Follow-up after the first real-app check: the user asked for a visible affordance, so the Changes header gained a view button (the `list-tree` icon beside refresh) that opens the same `View`/`Group By` menu anchored under it; History keeps the secondary-press menu only.
- 2026-11-13 — Follow-up defaults: the user set `Tree` and `Group By: Staged & Unstaged` as the defaults, and made the `List`/`Tree` choice one shared `EditorState::list_view` so whichever section is switched, both follow (the `Group By` pick stays Changes-only). Still session-only, no persistence.
