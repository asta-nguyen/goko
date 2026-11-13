# Changes and History: View (List/Tree) and Group By menu

## Goal

The Changes changed-file list and the History commit detail's changed-file list
can be shown as a flat `List` or a directory `Tree`, and the Changes list can
group its rows by tracked/untracked or staged/unstaged, from the context menu in
the user's screenshot, without changing selection, patch loading, staging,
commit, or open-at-line.

## Scope

In:

- Changes changed-file list: a context menu with `View` (`List`, `Tree`) and
  `Group By` (`None`, `Tracked & Untracked`, `Staged & Unstaged`), the active
  option marked, opened by right-click on the list or the header's view button.
- History commit detail changed-file list: the same `View` (`List`, `Tree`) menu.
- One shared in-memory `List`/`Tree` choice for both sections and a
  Changes-only `Group By`, for the app session (not persisted); defaults
  `Tree` and `Staged & Unstaged`.

Out:

- Backend, Git, or store changes; persisted preferences; the diff rendering
  itself; the Files tree; grouping the History commit list (only its detail's
  changed-file list gets the View menu).

## Behavior

**Menu.** Right-clicking the Changes changed-file list opens a context menu with
`View` and `Group By`; the active option is marked. The Changes header's view
button (beside refresh) opens the same menu anchored under it, so the choice is
also reachable with a left click. The History commit detail's changed-file list
opens the same menu with `View` only, on right-click. A choice applies to that
section and survives section and checkout switches for the app session; a
restart returns to `List` and `None`.

**List.** The existing flat rows, unchanged.

**Tree.** Files nest under their directories, with a folder row per directory;
folders expand and collapse like the Files tree. The row actions (select, load
patch, stage/unstage, open) apply to the file rows. The tree is derived from
each entry's path.

**Group By (Changes).** `None` keeps today's order. `Tracked & Untracked`
renders a `Tracked` group (staged, unstaged, conflicts) and an `Untracked` group.
`Staged & Unstaged` renders a `Staged` group (staged, conflicts) and an
`Unstaged` group (unstaged plus untracked); a path that is both staged and
unstaged appears in both. Empty groups are not rendered. The filter and keyboard
navigation apply to the rendered rows.

**State.** `EditorState` gains one shared `list_view: ListView` that both
sections read and both menus write, so picking `List` or `Tree` in either
section applies to the other; `ChangesState` gains `group: ChangeGroup` for the
`Group By` choice. Defaults are `Tree` and `Staged & Unstaged`, in memory only.

## Edge cases

- A path both staged and unstaged: both groups show it; selection is by row
  identity, not by path.
- A conflict: the `Tracked`/`Staged` group (the working tree has markers); it is
  selectable and its patch loads.
- A rename or copy: the entry's `path` is the destination; `orig_path` is not a
  row.
- A deleted file: still a row; its patch loads.
- An empty list: the menu still opens; `Tree` and grouping render nothing.
- A filter that hides every row: the menu and the chosen view stay; the pane
  shows its existing empty state.
- A History detail with no changed files: the menu opens but the list is empty.
- A checkout switch: the choice stays; the new checkout's rows render in it.

## Interfaces

- `apps/rune/src/editor/changes.rs`: the list renderer, its context menu, and
  the header's view button (`changes-view-menu`, the menu's anchor).
- `apps/rune/src/editor/history.rs`: the commit detail's changed-file list and
  its `View` menu.
- `apps/rune/src/editor/state.rs`: `EditorState.list_view` (shared by both
  sections), `ChangesState.group`; `ListView { List, Tree }`,
  `ChangeGroup { None, TrackedUntracked, StagedUnstaged }`.
- Reuses the `context_menu`/`MenuItem` pattern in `files.rs` and the existing
  `GitViewChanges` entries.

## Verification

- `cargo test -p goko-rune changes history` in the Runtime harness: menu and
  active option; Tree versus List; each group; the History switch; state across
  section and checkout switches; a row selection still loads its patch.
- `cargo fmt --all -- --check`, `cargo clippy -p app-core -p goko-rune -- -D
  warnings` (only the pre-existing `quit_guard` errors), `cargo test --workspace
  --no-fail-fast` (only the pre-existing `rune-kit` failure).
- The real app with `agent-desktop` on a uniquely named copy and a temporary
  `GOKO_HOME`: the menu options, Tree/List, each group, the History detail, and
  a row click.

## Related context

- [Rune editor](../../../llm/domains/rune-editor.md): `RNE-changes-git-writes`
  and the Changes/History sections this list sits in.
