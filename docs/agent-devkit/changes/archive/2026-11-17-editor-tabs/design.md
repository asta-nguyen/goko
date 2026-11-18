# Editor tabs

## Goal

The Editor keeps several files open as tabs. Opening another file no longer interrupts a file with unsaved edits, and switching back finds the edits where they were. Today `EditorState.doc` holds one document, so opening a file while the current one is dirty raises a Save/Discard/Cancel dialog.

Found by the 2026-11-17 review of the Editor section (chat); behavior aligned with the reference editor the user pointed at. Companion change: [editor-sync-and-conflicts](../2026-11-17-editor-sync-and-conflicts/design.md). This change is implemented after that change's Task 5 (`EditorState::path_is_dirty`) and Task 8 (`Req::CheckFile`).

## Scope

In scope: a tab bar over the editor pane, open/switch/close, per-tab unsaved edits, one guard dialog for every dirty tab (draft guard and quit guard), `⌘W` and tab-switching chords.

Out of scope: split panes, drag-reordering, pinned or preview tabs, restoring tabs or unsaved edits after a restart (hot exit), tabs across checkouts, a tabs overflow menu (the bar scrolls), a tab context menu, a most-recently-used picker for `⌃Tab`, a limit on the number of tabs.

## Behavior

**Model.** `EditorState.doc` stays the active document, so the ~270 existing uses keep working. A new `tabs` value holds the open order (every open path, active included), the most-recently-used order, and the inactive documents (`OpenDoc`, with their unsaved buffer, undo history, caret and scroll). Switching parks the active document and takes the target out of the parked set. There is no limit on the number of tabs: closing is the user's call, and a clean tab costs one document of at most 1 MiB.

**Open.** Every route that opens a file (tree, quick open, search, go-to-definition, Changes and History "Open File", external open) already goes through `files::open_doc` → `PendingNav::OpenDoc`, and keeps doing so. Within the current checkout it never raises the dirty guard: the current document is parked and the file opens as a new active tab, or the existing tab is activated without re-reading the file (the disk check from the companion change runs on activation). Opening in another checkout still goes through the guard (below).

**Close.** A tab's × button and `⌘W` close the active tab. A clean tab closes at once. A dirty tab raises the Save/Discard/Cancel dialog naming that file. After closing the active tab the tab to its right activates, else the one to its left; closing the last tab leaves the "Open a file" empty state.

**Switch.** Clicking a tab activates it. `⌘⌥→` and `⌘⌥←` activate the next and previous tab in tab order, wrapping (`Ctrl+PageDown` and `Ctrl+PageUp` off macOS, where `⌘⌥` clashes with desktop shortcuts); `⌘⇧]` and `⌘⇧[` do the same on macOS when the key arrives as that chord. `⌃Tab` activates the most recently used other tab, so pressing it again toggles back. Switching is held for the few milliseconds of a save in flight, like navigation is today.

**Guard.** Leaving the checkout or project, switching section or view, closing the window and quitting still go through the guard, now as one dialog for all dirty tabs. With one dirty tab it is today's dialog (Save / Discard / Cancel). With several it reads "Save changes to the following N files?", lists up to ten file names ("…and K more"), and offers Save All / Discard All / Cancel. The first dirty tab in tab order is activated while the dialog shows.

- Save All saves the dirty tabs one after another in tab order (each is activated in turn, which is how a save runs on the active document). The navigation resumes after the last save lands. A failed save stops everything: the failing tab stays active with its error row, tabs not yet saved keep their drafts, the navigation is dropped.
- Discard All reverts every dirty tab, then the navigation proceeds.
- Cancel changes nothing.

A checkout change, once past the guard, closes all tabs.

**Tab bar.** Above the path bar. Each tab shows the file name (with its parent directory when two open tabs share a name), a `•` while dirty, and a × button; the full path is the help text. The active tab is marked. The bar scrolls horizontally when tabs overflow, including at the 760 px minimum window.

**Rename, delete, create.** Renaming or trashing a file or folder is blocked when any tab under it has unsaved edits (today: when the open draft is under it). After a rename the active clean tab reopens at its new path, as today, and inactive clean tabs under it are closed. After a trash every tab under the entry is closed. A newly created file always opens as a new tab (today it opens only when no draft is open).

**Language servers, highlighting, blame, change gutter.** They follow the active document only. Parking a document closes it in the language server; activating one reopens it and re-requests highlight, blame and change bars, as opening a file does.

## Edge cases

- Same file opened twice quickly (double-click): the second request activates the existing tab; no duplicate tab.
- A file load reply lands for a path already open as a tab (race): the existing tab wins and the reply is dropped.
- A file deleted on disk while parked: activation runs the disk check from the companion change; the tab shows the "File deleted on disk" notice and keeps the buffer.
- Replies for a parked document (highlight, blame, line changes, check) match no active document and are dropped; activation re-requests them.
- Save in flight on the active tab while another file is requested: navigation waits for the save (existing `save_resume` path), then proceeds; a failed save keeps the tab active and the request is dropped.
- Save All where the second save fails (disk changed, read-only): the first stays saved, the second stays active with its error and Overwrite/Reload row, the navigation does not happen.
- A tab becomes dirty or clean while the dialog is open: the list is taken when the dialog is raised; Save All re-checks each tab's dirty state when it reaches it and skips a tab that is clean by then.
- Quit with several dirty tabs: the OS quit is vetoed and the same single dialog shows; Cancel cancels the quit.
- A checkout whose worktree disappears: existing `clear_checkout` clears the tabs with the document.
- `⌃Tab` with one tab or no earlier tab: nothing happens.
- Memory: no cap; each open document holds its text (at most 1 MiB) plus syntax data. If memory use becomes a problem, add a soft limit that closes the least recently used clean tab and never a dirty one.

## Interfaces

- New `editor::tabs` module: `Tabs { order, recent, parked }` with open, activate, close, next/previous, previous-recent, dirty iteration and clear, owned by `EditorState.tabs`.
- `PendingNav::CloseTab { path }`, `PendingNav::SwitchTab { path }`; `navigation::Guard` gains the list of dirty paths still to save.
- `EditorState::has_dirty_doc` and `EditorState::path_is_dirty` also report inactive dirty documents (feeds the OS quit flag, rename, trash and the companion change's resolve actions).
- `keyboard::on_key` handles `⌘W`, the tab-order chords and `⌃Tab` while the Editor view is active and a document is open.
- No backend, protocol or schema change.

## Verification

- Unit tests per task: `tabs.rs` state tests, `navigation.rs` guard tests, `quit_guard.rs` tests, rune-harness view and key tests in `files.rs`/`keyboard.rs`/`workspace.rs`.
- `cargo fmt --all -- --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`.
- App check in the fresh `target/Goko.app` window with `agent-desktop`: open three files, edit two, switch away and back, close a dirty tab, quit with two dirty tabs and see one dialog; confirm `⌘W` does not close the window.

## Related context

- [rune-editor.md](../../../../llm/domains/rune-editor.md) (`RNE-draft-guard`, `RNE-keyboard`, `RNE-file-editing`).
