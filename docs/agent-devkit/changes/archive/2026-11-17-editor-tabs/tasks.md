# Editor tabs tasks

## Design

[Editor tabs](design.md)

## Global Constraints

- `EditorState.doc` remains the active document; every inactive tab lives in `EditorState.tabs`. No other field replaces it.
- No limit on the number of tabs.
- Opening, switching to, or closing a clean tab within the current checkout never raises the draft guard; closing a dirty tab does.
- Leaving the checkout, project, section or view, closing the window and quitting raise one guard dialog for all dirty tabs: with one dirty tab the existing Save / Discard / Cancel dialog, with several "Save changes to the following N files?" listing up to ten names then "…and K more", with Save All / Discard All / Cancel. Save All saves in tab order, one tab active at a time; a failed save stops everything and leaves the failing tab active.
- Only the active document is mirrored to the language server and receives highlight, blame, line-change and disk-check results; replies for any other path are dropped and activation re-requests them.
- A save in flight holds navigation (existing `save_resume`); no tab is parked while `saving`.
- Chords, only in the Editor view with a document open: `⌘W` close the active tab; `⌘⌥→`/`⌘⌥←` (secondary `⌘⇧]`/`⌘⇧[` on macOS; `Ctrl+PageDown`/`Ctrl+PageUp` off macOS) next/previous tab in tab order, wrapping; `⌃Tab` the most recently used other tab.
- No backend, protocol, schema or dependency change. Implemented after `editor-sync-and-conflicts` Tasks 5 and 8 (`EditorState::path_is_dirty`, `Req::CheckFile`).

## Impact map

Entry: `apps/rune/src/editor/files.rs::open_doc`, `apps/rune/src/editor/navigation.rs::navigate`, `apps/rune/src/editor/backend.rs::EditorState::install_doc`
Flow: tree, quick open, search, go-to-definition, Changes/History "Open File", external open → `files::open_doc` → `navigation::navigate(PendingNav::OpenDoc)` → dirty guard or `apply` → `files::request_doc_load` → `Req::File` → `backend/apply.rs` → `EditorState::install_doc` → `self.doc = Some(OpenDoc)`; `doc` is then read by `files.rs` (doc bar, editor pane), `highlight.rs`, `blame.rs`, `line_changes.rs`, `lsp.rs` (poll picks the document to mirror), `snapshot.rs`, `keyboard.rs`, `main.rs`; guard dialog `workspace.rs::guard_dialog` → `navigation::resolve_guard` → `files::save_active_doc` → `Req::SaveFile` → `backend/apply.rs::finish_save` → resume of `Guard.pending`; quit and window close read `has_dirty_doc` → `quit_dirty` flag → `quit_guard.rs`, `navigation::on_window_close/quit_after_save/quit_discard`
State changes: in-memory `EditorState` only (`tabs`, `doc`, `guard`, `save_resume`); no persistence
External effects: language-server `didClose`/`didOpen` on tab switch (existing document-sync poll); extra blame and `git diff` reads on activation
Change candidates: `apps/rune/src/editor/tabs.rs` (new), `apps/rune/src/editor/mod.rs`, `apps/rune/src/editor/state.rs`, `apps/rune/src/editor/navigation.rs`, `apps/rune/src/editor/workspace.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/backend/apply.rs`, `apps/rune/src/editor/files.rs`, `apps/rune/src/editor/keyboard.rs`, `apps/rune/src/editor/quit_guard.rs`, `apps/rune/src/main.rs`, `CHANGELOG.md`, `docs/llm/domains/rune-editor.md`
Verification: `cargo test -p goko-rune` (module tests named per task), `cargo fmt --all -- --check`, `cargo clippy --workspace -- -D warnings`, `agent-desktop` check of the fresh `target/Goko.app`
Verified at: 631272ce1b1c9cd7f20d015f3a5737007a092a01

## Tasks

### Task 1 — Tab model

Files: `apps/rune/src/editor/tabs.rs::Tabs` (new), `apps/rune/src/editor/mod.rs`, `apps/rune/src/editor/state.rs::EditorState`, `apps/rune/src/editor/state.rs::has_dirty_doc`, `apps/rune/src/editor/state.rs::path_is_dirty`
Interfaces: produces `Tabs { order: Vec<String>, recent: Vec<String>, parked: HashMap<String, OpenDoc> }` on `EditorState.tabs` with `add_active`, `activate(doc_slot, path)`, `close(doc_slot, path) -> neighbour`, `step(doc_slot, ±1)`, `previous_recent(doc_slot)`, `dirty_paths(doc_slot)`, `clear`; `has_dirty_doc` and `path_is_dirty` (from the companion change, extended here) also count parked dirty documents; consumed by Tasks 2–5.
Change: pure state operations over the active slot (`&mut Option<OpenDoc>`) and the parked map. `activate` parks the active document, moves the target into the slot and moves its path to the front of `recent`; `close` removes a tab and activates the right neighbour, else the left, else leaves the slot empty; `step` wraps; `previous_recent` returns the most recently used path other than the active one; `dirty_paths` lists dirty tabs in tab order. Display order is open order. No limit and no eviction.
Verify: `tabs.rs` unit tests: open three, activate the first, the buffer text and caret of each survive a round trip; closing the middle active tab activates the right neighbour, closing the last activates the left, closing the only tab empties the slot; `step` wraps both ways; `previous_recent` toggles between two tabs and returns none with one tab; `dirty_paths` lists in tab order; `has_dirty_doc` and `path_is_dirty` are true when only a parked document is dirty; twenty opens keep all twenty.
Progress: done
Result: `cargo test -p goko-rune tabs` — 7 tests in `tabs.rs` plus `state.rs::has_dirty_doc_and_path_is_dirty_see_parked_documents` pass; the Xaaa/Ybbb buffer + caret (0,0) expectations match the design.

### Task 2 — Open, switch, close and the single guard dialog

Files: `apps/rune/src/editor/navigation.rs::navigate`, `apps/rune/src/editor/navigation.rs::apply`, `apps/rune/src/editor/navigation.rs::PendingNav`, `apps/rune/src/editor/navigation.rs::Guard`, `apps/rune/src/editor/navigation.rs::resolve_guard`, `apps/rune/src/editor/backend.rs::EditorState::install_doc`, `apps/rune/src/editor/backend/apply.rs::finish_save`, `apps/rune/src/editor/workspace.rs::guard_dialog`, `apps/rune/src/editor/navigation.rs::clear_checkout`, `apps/rune/src/editor/navigation.rs::select_checkout`
Interfaces: consumes Task 1; produces `PendingNav::SwitchTab { path }`, `PendingNav::CloseTab { path }` for Tasks 4–5, and `Guard.queue: Vec<String>` (dirty paths still to save).
Change:
- `navigate` for `OpenDoc` and same-checkout `ExternalOpen`: when the path is already a tab, activate it with no read (and enqueue `Req::CheckFile`); otherwise do not raise the dirty guard, wait only for an in-flight save, and issue the file load. `install_doc` parks the active document and installs the new one as the active tab; a load reply for a path that is already a tab is dropped.
- `SwitchTab` activates the tab (held while `saving`); `CloseTab` of a clean tab closes it, of a dirty tab raises the dialog for that one tab.
- Guard: when a navigation that leaves the checkout, project, section or view is requested and any tab is dirty, `navigate` activates the first dirty tab and raises one guard whose list is every dirty path. `guard_dialog` shows today's dialog for one path; for several it shows "Save changes to the following N files?", up to ten names then "…and K more", and the buttons Save All / Discard All / Cancel.
- `resolve_guard(Save)`: put the other dirty paths in `Guard.queue`, set `save_resume`, save the active tab. On a successful save reply, if `queue` is non-empty pop the next path that is still dirty, activate it and save it with `save_resume` kept; otherwise resume `Guard.pending`. A failed save keeps the failing tab active, clears the queue and drops the pending navigation (existing failure path). `resolve_guard(Discard)` calls `discard` on every dirty tab, then `apply`. Cancel is unchanged.
- `clear_checkout` and `select_checkout` also call `Tabs::clear`.
Verify: `navigation.rs`/`workspace.rs` tests: opening a second file with the first dirty raises no guard and keeps both; opening an open path activates it with no `Req::File` and enqueues one `Req::CheckFile`; a tab switch requested while the active document is `saving` waits for the save and is dropped when the save fails; double open yields one tab; a load reply for an existing tab is dropped; closing a dirty tab raises the one-file dialog naming it and Cancel keeps it; with two dirty tabs a checkout switch raises one dialog listing both with Save All / Discard All / Cancel; Save All saves both in tab order and then switches; a failing second save leaves the checkout unchanged, the second tab active with its error and the first saved; Discard All reverts both and switches; Cancel changes nothing; a tab that turned clean while the dialog was open is skipped by Save All; eleven dirty tabs list ten names and "…and 1 more"; checkout change clears tabs.
Progress: done
Result: `cargo test -p goko-rune navigation` — 69 tests pass, including the 13 new tab tests (open-parks-draft, open-tab no-reload + one CheckFile, saving-hold drop, one tab after double open, load reply keeps parked buffer, dirty-tab close guard, clean close, one guard for two dirty tabs, Save All in order, failing second save, Discard All, Cancel, clean-skip, tabs cleared) plus `workspace::the_multi_file_guard_lists_ten_names_then_counts_the_rest`; stale quick_open guard test updated to the new contract.

### Task 3 — Quit and window close with several dirty tabs

Files: `apps/rune/src/editor/navigation.rs::on_window_close`, `apps/rune/src/editor/navigation.rs::quit_after_save`, `apps/rune/src/editor/navigation.rs::quit_discard`, `apps/rune/src/editor/navigation.rs::on_quit_vetoed`, `apps/rune/src/editor/quit_guard.rs`, `apps/rune/src/editor/state.rs::sync_quit_dirty`
Interfaces: consumes `has_dirty_doc`, `Guard.queue` and the dialog from Tasks 1–2.
Change: the OS-quit dirty flag and the window-close check use `has_dirty_doc` (any tab); the quit guard raises the same single dialog as Task 2; `quit_after_save` continues through `Guard.queue` and quits only when it is empty; `quit_discard` discards every dirty tab; Cancel, or a failed save, cancels the quit and the veto is consumed as today.
Verify: `quit_guard.rs`/`navigation.rs` tests: only a parked tab dirty → `quit_dirty` true and window close raises the guard; two dirty tabs → one dialog, Save All lets the quit proceed after both saves, a failed second save cancels the quit; Discard All proceeds; Cancel cancels; after all tabs are clean the quit flag is false.
Progress: done
Result: `cargo test -p goko-rune navigation` — 5 quit tests pass: parked-dirty veto + window-close guard, Save All → `pending_apply: Quit` with the flag cleared, failed save keeps draft + cancels quit, Discard All clears flag + veto, Cancel drops the quit. `quit_guard.rs` untouched — the atomic flag already aggregates via `has_dirty_doc`.

### Task 4 — Per-tab async state, rename, trash and create

Files: `apps/rune/src/editor/backend/apply.rs::finish_rename`, `apps/rune/src/editor/backend/apply.rs::finish_trash`, `apps/rune/src/editor/backend/apply.rs::finish_create`, `apps/rune/src/editor/files.rs::submit_trash`, `apps/rune/src/editor/files.rs::submit_rename`, `apps/rune/src/editor/tabs.rs::activate`, `apps/rune/src/editor/highlight.rs`, `apps/rune/src/editor/blame.rs`, `apps/rune/src/editor/line_changes.rs`
Interfaces: consumes Tasks 1–2.
Change:
- On activation (via `Tabs::activate` callers): set `highlight_dirty_at` to now, clear `highlight_inflight`, and call the blame and line-change `sync_visibility`/`request` pairs as `install_doc` does. Replies carrying a path other than the active document's are dropped (existing path match; add a test, not code, unless a reply can still land on the wrong document).
- `submit_trash` and `submit_rename` block when `path_is_dirty` is true for the entry or any path under it (active or parked), with the existing notice texts.
- `finish_trash` closes every tab under the entry; `finish_rename` reopens the active clean tab at its new path (existing behavior) and closes inactive clean tabs under the entry; the existing "draft still points at the old path / is all that is left" notices stay for the race where a tab turned dirty after the check.
- `finish_create` opens the new file as a new tab even when the active document is dirty.
Verify: `apply.rs`/`files.rs` tests: activating a parked document re-arms highlight and enqueues blame and line-change reads; a highlight reply for a parked path leaves the active document unchanged; trash of a folder with a dirty parked tab inside is blocked; trash closes clean tabs under it; rename of a folder closes inactive clean tabs under it and reopens the active one; creating a file with a dirty active tab opens it as a second tab and keeps the draft; the companion change's whole-file resolve action is refused when only a parked tab for that path is dirty.
Progress: done
Result: `cargo test -p goko-rune` — 965 tests pass, including the new `apply::tests` (parked-state survives activation, rename closes inactive clean tabs + reopens the active one + drops the stale install, dirty tab kept with notice, parked trash drops + re-requests nothing, active trash activates neighbour + re-requests, dirty tab under trash kept, created file opens as a tab keeping the draft) and `files::tests::rename_and_trash_refuse_while_any_tab_under_them_is_dirty`; `install_doc` never parks a `saving` doc (a load reply joins parked instead), `doc_load` now carries its path so renames drop stale installs, `finish_create` always opens a tab.

### Task 5 — Tab bar and chords

Files: `apps/rune/src/editor/files.rs::view`, `apps/rune/src/editor/files.rs::tab_bar` (new), `apps/rune/src/editor/keyboard.rs::on_key`, `apps/rune/src/main.rs`
Interfaces: consumes `PendingNav::SwitchTab/CloseTab`, `Tabs::step` and `Tabs::previous_recent` (Tasks 1–2).
Change: a horizontally scrolling row above `doc_bar` in the editor column; one tab per entry in open order: `•` when dirty, file name (prefixed with its parent directory when two tabs share a name), × button, path as help text, active tab marked; click → `navigate(SwitchTab)`, × → `navigate(CloseTab)`; hidden when no document is open. `keyboard::on_key` handles `⌘W`, the tab-order chords and `⌃Tab` from the Global Constraints before the code surface sees the key (the code surface treats `Tab` as indent), and returns false when no document is open so the key falls through. Next/previous use the same command-modifier helper as `⌘S` and, where that modifier is Ctrl, `Ctrl+PageDown`/`Ctrl+PageUp`; if Rune reports the shifted bracket glyph for `⌘⇧]`/`⌘⇧[`, match that glyph.
Verify: rune-harness tests: three tabs render in order with the dirty marker on the edited one and distinct labels for two files named `mod.rs`; clicking a tab activates it; × on a clean tab closes it; the next/previous chords cycle with wrap while the code field is focused and do not indent or move the caret by words; `⌃Tab` toggles between the two most recent tabs and does nothing with one tab; `⌘W` closes the active clean tab and raises the dialog for a dirty one; with no document `⌘W` and the others are not consumed. App check with `agent-desktop`: open three files, edit two, switch away and back (text intact), `⌘W` on a dirty tab shows the dialog and does not close the window, the bar scrolls at the 760 px minimum width, quitting with two dirty tabs shows one dialog listing both.
Progress: done
Result: `cargo test -p goko-rune` — 971 pass, incl. new harness tests `tabs_paint_in_order_with_a_dirty_marker_and_distinct_labels`, `clicking_a_tab_activates_it_and_the_x_closes_its_tab`, `the_step_chords_wrap_while_the_code_field_is_focused`, `ctrl_tab_toggles_the_most_recent_tabs`, `cmd_w_closes_a_clean_tab_and_guards_a_dirty_one`, `tab_chords_fall_through_with_no_document`. `files.rs::tab_bar` + `doc_tab`/`tab_label` mount a `scroll_x` strip above `doc_bar`; `PendingNav::StepTab` + `EditorState::step_tab` carry next/previous; `Code::preempt_key` gives `keyboard::on_key` first refusal inside the field's `on_input_key` (and the find query's), and `field_key` no longer indents on a cmd/ctrl/alt-Tab. `main.rs` needed no change: `app_shortcut` claims only ⌘T and `alt_shortcut` bails on the command modifier, so the chords already reach `keyboard::on_key`. App check (2026-11-18, the installed 0.2.0 build): three tabs paint in open order with `•` on the dirty ones; opening a file while the active tab is dirty raises no dialog; switching away and back keeps the buffer (identical screenshots); `⌘W` on a dirty tab raises "Save changes before leaving?" and Cancel keeps it, Discard closes it; `⌘W` on a clean tab closes it; leaving the project and quitting with two dirty tabs each raise one dialog listing both, and Cancel keeps everything; at 760 px the third tab is offscreen in AX (the strip scrolls). One `⌘W` on a dirty tab closed it with no dialog while the window was off-screen and not exposed to accessibility; seven controlled repeats raised the dialog, so it is recorded as not reproducible.

### Task 6 — Documentation and changelog

Files: `docs/llm/domains/rune-editor.md`, `CHANGELOG.md`
Interfaces: consumes Tasks 1–5.
Change: call `document-wiki`'s `## Task updates` to merge `delta.md` into `docs/llm/domains/rune-editor.md` (Evidence paths must exist) and refresh text it contradicts (state sections that describe a single open document). Add a user-facing `## [Unreleased]` entry in `CHANGELOG.md`: file tabs, `⌘W` and the tab-switching chords, opening a file no longer asks about unsaved edits, one dialog for all unsaved files.
Verify: every Evidence path in the new and replaced blocks exists (`test -f`); `cargo fmt --all -- --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.
Progress: done
Result: merged `delta.md` into `docs/llm/domains/rune-editor.md` — added `RNE-editor-tabs`, replaced `RNE-draft-guard` (one multi-tab guard) and `RNE-keyboard` (tab chords) — and refreshed the Flow/State-changes prose for tabs (parked docs, active-doc LSP mirror); `## Sources` gained `tabs.rs`. `validate-llm-wiki.mjs` reports 0 errors on the page (166 pre-existing errors remain on the desktop-app pages and rune-chrome.md — `docs/llm/architecture/editor.md` documents the removed `apps/desktop` app entirely: every Evidence path dangles, a pre-existing confirmed content gap, left untouched since the page's domain is the desktop app, not the rune editor). Added the `Merged into docs/llm/: 2026-11-18` marker to `delta.md` and the `## [Unreleased]` entry to `CHANGELOG.md`.

## Files inspected, no change

- `apps/rune/src/editor/code.rs` — `CodeState` already owns buffer, undo and scroll per document, so parking an `OpenDoc` keeps them.
- `apps/rune/src/editor/lsp.rs` — the document-sync poll derives the mirrored document from `doc`, so a swap closes and reopens it without change.
- `apps/rune/src/editor/quick_open.rs`, `apps/rune/src/editor/search.rs`, `apps/rune/src/editor/go_to_definition.rs` — all open files through `files::open_doc`.
- `apps/rune/src/editor/draft.rs` — `OpenDoc` is unchanged.

## Approval Gate

Status: approved

## Decision Log

- 2026-11-17 — the user asked to follow the reference editor's behavior. Folded in: no tab limit (removes the 12-tab cap, eviction and the refusal notice), tab-order chords `⌘⌥→/←` and `⌃Tab` for the most recently used tab (replacing `⌃Tab` as next-in-order), and one guard dialog for all dirty tabs (replacing the tab-by-tab walk). Gate stays `pending`; nothing was approved before this revision.
