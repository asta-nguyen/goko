# Vim mode in the native editor tasks

## Design

[Vim mode in the native editor](design.md)

## Global Constraints

- With `goko.editor.vim` off, `field_key` and every mounted element behave
  exactly as today; the engine is never called.
- No Rune, dependency, protocol or daemon change; the Normal-mode IME
  limitation is documented, not worked around.
- `apps/rune/src/editor/vim.rs` has no Rune types and no I/O; every
  `TextEdit`/`Tx` interaction stays in `code.rs`.
- Keys with `cmd` or `alt` held never reach the engine; every existing
  shortcut and chord keeps working.
- Outside Insert mode the caret is on a character: an empty line keeps byte 0
  and a non-empty line clamps to its last character.
- One Normal-mode text change is one undo step (`TextEdit::undo_group`);
  Insert keeps the native typing groups.
- `goko.editor.vim`: `"1"` is on; absent, unreadable, failed read or any other
  value is off. The setting is global, not per project.
- Read-only documents render `text(...)` and are never routed.
- New behavior reaches the wiki only through `delta.md` and `document-wiki`;
  no other `docs/llm/` edits.

## Impact map

Entry: `apps/rune/src/editor/code.rs::field_key` (key routing); `apps/rune/src/settings.rs::editor_section` (the switch)

Flow: `text_input.on_input_key` → `preempt_key` (`keyboard::on_key`) → `field_key` → `vim::step` → effects applied to `CodeState.edit` (`TextEdit`) → `notify_changed` → revision/syntax/gutter/find refresh; the setting flows `GetSetting`/`SetSetting` → `backend/apply.rs` → `EditorState::set_vim_enabled` → every `CodeState`

State changes: settings row `goko.editor.vim`; in-memory `CodeState.vim` per document; the unnamed register on `EditorState`

External effects: the existing settings write lane; the system clipboard (write on yank, asynchronous read on paste)

Change candidates: `apps/rune/src/editor/vim.rs` (new), `apps/rune/src/editor/code.rs`, `apps/rune/src/editor/state.rs`, `apps/rune/src/editor/find.rs`, `apps/rune/src/editor/mod.rs`, `apps/rune/src/settings.rs`, `apps/rune/src/main.rs`, `apps/rune/src/editor/backend/apply.rs`, `CHANGELOG.md`, `ARCHITECTURE.md`, `docs/agent-devkit/INDEX.md`

Verification: `cargo test --workspace`, `cargo fmt --all -- --check`, `cargo clippy --workspace -- -D warnings`, `cargo build --workspace`, plus the manual pass in Task 7

Verified at: 81660e98e0e5b79b055115dd82265fe3440b1f67

## Tasks

### Task 1 — Engine core: modes, keys, motions, clamping

Files: `apps/rune/src/editor/vim.rs` (new), `apps/rune/src/editor/mod.rs::vim` (module declaration)

Interfaces: produces `Mode`, `Key`, `Doc` (text, caret, indent), `VimState`, `Effect`, `step` and the pending-buffer helpers; consumes `indent::IndentUnit` and `indent::TAB_STOP`.

Change: the pure engine with Normal and Insert modes (Visual variants exist in the enum but only Normal/Insert transition in this task), an own `Key` enum mapped from `rune::KeyEvent` by the host, count parsing (digits, no leading zero), Escape cancelling a pending count, and these motions with counts and clamping: `h j k l`, `w b e W B E`, `0 ^ $`, `gg G`, `f t F T`, `; ,`, `{ }`, `%`. `j`/`k` keep the goal display column (tab = 4) across shorter lines; every other command clears it. Motions emit `Effect::Select` only. Unknown keys are consumed as a no-op. The module has no Rune imports and no I/O.

Verify: `cargo test -p goko-rune editor::vim` passes a table of input → expected caret, including: `("hello world", 0, w) → 6`, `("hello world", 6, b) → 0`, `("hello world", 0, e) → 4`, `3l`, `2w`, `$` on `"ab\ncd"` from 0 → 1, `gg`/`G`, `f o` → 4, `;` repeats, `%` on `(a [b] c)`, `}` over a blank line, `j` on a long line then a short line keeps the goal column, empty line `$`/`l` stay at the line start, Escape drops a pending `2`, and an unknown key returns no effects.

Progress: done
Result: `cargo test -p goko-rune editor::vim` — 13 passed; `cargo clippy -p goko-rune -- -D warnings` — clean.

### Task 2 — Setting, routing, mode chip and block caret

Files: `apps/rune/src/editor/vim.rs::VIM_KEY`, `apps/rune/src/editor/state.rs::EditorState::vim_enabled`, `apps/rune/src/editor/state.rs::EditorState::set_vim_enabled`, `apps/rune/src/editor/state.rs::CodeState::vim`, `apps/rune/src/editor/code.rs::field_key`, `apps/rune/src/editor/code.rs::Code::field`, `apps/rune/src/editor/code.rs::Code::footer`, `apps/rune/src/editor/code.rs::vim_caret_el`, `apps/rune/src/settings.rs::editor_section`, `apps/rune/src/main.rs` (startup `GetSetting`), `apps/rune/src/editor/backend/apply.rs` (`Setting` arm)

Interfaces: consumes `vim::step` (motions only) and the engine state; produces the routing contract, the footer mode chip, the block caret element (key `goko-vim-caret`), and the setting mirror used by Tasks 3–6.

Change: `goko.editor.vim` is requested at startup on the settings lane and applied to `EditorState::vim_enabled`, the active document and the scratch state; `set_vim_enabled(false)` returns the mode to Normal and clears pending state, `CodeState::load` resets the mode and pending buffer, and `CodeState::text_edited` clears the pending buffer so an externally changed document never keeps a half-typed operator. Settings → Editor gains the "Vim mode" checkbox whose description names the Normal-mode ABC input-source requirement. `field_key` routes to the engine only when Vim is on, the document is editable and the key has neither `cmd` nor `alt`; Insert passes ordinary keys through to the field, consumes Escape (Insert → Normal, no blur), and Tab in Normal/Visual is consumed as a no-op. Outside Insert the native caret is drawn transparent and a block one mono cell wide (a tab to its stop) is drawn inside the scroll content at `display columns × cell advance`, filled `ink.text` at 28% alpha. The footer shows `NORMAL`/`INSERT`/`VISUAL`/`V-LINE` plus pending keys on the left while Vim is on.

Verify: `cargo test -p goko-rune` passes: setting off leaves `h` untouched and no chip or block caret is mounted; setting on moves the caret on `h`/`j`/`k`/`l`, `i` switches the chip to `INSERT`, Escape returns to `NORMAL` and the field stays focused, Tab in Normal leaves the text unchanged, `⌘S` still fires save; the block caret element exists in Normal and is absent in Insert; `apply` handles `"1"`, `"0"`, absent and a failed read; `set_vim_enabled` mirrors into the active and scratch `CodeState`.

Progress: done
Result: `cargo test -p goko-rune` — 1023 passed; `cargo clippy -p goko-rune -- -D warnings` — clean. Runtime tests cover routing on/off, Insert pass-through, Escape focus, Tab no-op, chip and block caret.

### Task 3 — Operators, text objects, registers, paste, undo and repeat

Files: `apps/rune/src/editor/vim.rs`, `apps/rune/src/editor/code.rs::field_key` (effect application), `apps/rune/src/editor/state.rs::EditorState::vim_register`

Interfaces: consumes the engine and routing from Tasks 1–2; produces `Effect::Replace`/`Yank`/`Paste`/`Undo`/`Redo` application, the unnamed register, and `record_insert`.

Change: operators `d c y` with motions and text objects (`iw aw iW aW`, quotes, backticks, brackets), doubled `dd cc yy`, counts on both sides, `x D C`, `p P` (linewise below/above; unnamed register first, otherwise an asynchronous clipboard read applied when it arrives), `J`, `>`/`<` through `indent::plan_indent`, `u`, `Ctrl-r`, and `.`. Each Normal-mode change applies its whole effect list inside `undo_group`; yanks also write `Tx::copy`; leaving Insert breaks the native typing group and records the net inserted text for `.`. `.` repeats the last operator/motion, single-key change or inserted text at the new caret; Visual changes are not repeatable in v1.

Verify: `cargo test -p goko-rune` passes engine tables for `dd`, `2dw`, `d$`, `ciw` (with inserted text), `yi"`, `d%`, `x`, `D`, `J`, `>>`, `3p` and `.` after each; a headless runtime test shows `dd` + `u` restores the text as one step, `y` then `p` in another tab pastes the register, and a memory clipboard (`Runtime::use_clipboard`) feeds `p` when the register is empty.

Progress: done
Result: `cargo test -p goko-rune` — 1032 passed; `cargo clippy -p goko-rune -- -D warnings` — clean. Engine tables cover operators, text objects, counts, `x`/`J`/`>>`/`<<`, `o`/`O`, paste and `.`; runtime tests cover one-step `dd`+`u`, register paste and the clipboard fallback.

### Task 4 — Visual and Visual Line

Files: `apps/rune/src/editor/vim.rs`, `apps/rune/src/editor/code.rs::field_key`

Interfaces: consumes the operator application from Task 3; produces `v`/`V`, extension, `o`, and Visual operators.

Change: `v` starts characterwise Visual at the caret, `V` starts linewise Visual over the caret line; motions extend, `o` swaps ends, `Escape` returns to Normal. The selection is drawn as an accent highlight over the inclusive range, with the native caret collapsed on the character under the cursor, so motions keep working; Visual Line covers whole lines. `d c y > < x` apply to the selection and are one undo step; `y` yanks, `c` deletes and enters Insert, and Visual Line acts on whole lines.

Verify: `cargo test -p goko-rune` passes engine tables for `v l l d`, `V j d`, `v e y`, `v o d`, and a runtime test asserting the `TextEdit` selection covers the character under the caret and `Escape` returns to `NORMAL` with the caret clamped.

Progress: done
Result: `cargo test -p goko-rune` — 1033 passed; `cargo clippy -p goko-rune -- -D warnings` — clean. The selection is drawn as a highlight over the inclusive range (the native caret stays on the character), and the runtime test covers the chip, range and delete.

### Task 5 — Search over the find bar

Files: `apps/rune/src/editor/find.rs`, `apps/rune/src/editor/vim.rs`, `apps/rune/src/editor/code.rs` (find-bar key handling and Vim find entry points)

Interfaces: consumes the find bar from Task 2; produces `FindState::open_at`/`next_after` and the `/ ? n N *` routing.

Change: `/` opens the find bar without moving the caret and records forward, `?` records backward; `Enter` commits (bar closes, caret to the first match at/after the origin, no selection), `Escape` cancels (bar closes, caret and mode restored). `n`/`N` step in the recorded direction and place the caret at the match start without selecting it; `*` seeds the query with the word under the caret and jumps to the next match. `⌘F`/`⌘G`/`⇧⌘G` are unchanged.

Verify: `cargo test -p goko-rune` passes `find` unit tests for `open_at` (no movement) and `next_after` (forward/backward, wrap, no match), and runtime tests: `/` leaves the caret in place, `Enter` lands on the next match, `Escape` restores the caret, `n`/`N` step both ways, `*` searches the word under the caret.

Progress: done
Result: `cargo test -p goko-rune` — 1039 passed; `cargo clippy -p goko-rune -- -D warnings` — clean. `find` unit tests and runtime tests cover open/commit/cancel, `n`/`N` wrapping and `*`.

### Task 6 — Command line

Files: `apps/rune/src/editor/code.rs::CodeState::vim_cmd`, `apps/rune/src/editor/code.rs` (command bar element and key handling), `apps/rune/src/editor/vim.rs` (`:` → `Command(Open)`)

Interfaces: consumes the routing from Task 2 and `navigation`/`save_active_doc`; produces the `:` bar and its commands.

Change: `:` in Normal opens a one-line command bar above the code with a `:` prefix and a single-line `TextEdit`, styled like the find bar; it takes focus and the field blurs. Enter parses and closes: `w`/`write` saves, `q`/`quit` closes the active tab through the draft guard, `wq`/`x` saves and closes on success through `save_resume` with `PendingNav::CloseTab`, a decimal number reveals that line, and anything else leaves the bar open with `Unknown command: :<text>`. Escape closes and refocuses the field.

Verify: `cargo test -p goko-rune` passes runtime tests: `:` mounts the bar keyed `goko-vim-cmd`; `:w` enqueues a save; `:q` on a dirty document raises the guard and on a clean one closes the tab; `:wq` sets the close resume and closes only after the save reply; `:12` reveals line 12; `:bogus` keeps the bar with the error text; Escape closes it and the field regains focus.

Progress: done
Result: `cargo test -p goko-rune` — 1045 passed; `cargo clippy -p goko-rune -- -D warnings` — clean. Runtime tests cover the bar mount, Escape, `:w`, `:q` (guard and clean close), `:wq` resume, `:<line>` and an unknown command.

### Task 7 — Documentation

Files: `CHANGELOG.md` (`## [Unreleased]` → `### Added`), `ARCHITECTURE.md` (the "There is no Vim mode" line), `apps/rune/src/settings.rs` (module and Editor-section comments), `docs/llm/domains/rune-editor.md` (merge the delta, update the "Vim is deferred" note through `document-wiki`), `docs/agent-devkit/INDEX.md`

Interfaces: consumes the delivered behavior and tests; produces user-facing and wiki documentation that matches it.

Change: the changelog entry describes Vim mode, its setting and the Normal-mode input-source requirement; `ARCHITECTURE.md` replaces "There is no Vim mode" with the delivered engine and its limitation; the Settings comments no longer call Vim absent; `document-wiki`'s `## Task updates` merges the delta and refreshes the Rune editor page.

Verify: `cargo fmt --all -- --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` and `cargo build --workspace` pass; the wiki validator reports no error on `docs/llm/domains/rune-editor.md` or `docs/llm/INDEX.md`.

Progress: done
Result: `cargo fmt --all -- --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` (1045 goko-rune tests) and `cargo build --workspace` pass. The delta is merged (`Merged into docs/llm/: 2026-11-19`), `RNE-vim-mode` is in the page and its `## Sources`, and the validator reports no error on the changed page or index. The repo-wide validator still exits 1 on 161 pre-existing errors in the stale Desktop pages (`apps/desktop` is removed) and `rune-chrome.md`; unrelated to this change.

### Task 8 — Manual pass in the running app

Files: `apps/rune/bundle-macos.sh` (the bundle used for the check); no source changes

Interfaces: consumes the delivered behavior; produces the observed UI results.

Change: launch a uniquely named copy of `target/Goko.app` with an isolated `GOKO_HOME` and a seeded temporary project, then check: Vim off behaves as before; Vim on shows `NORMAL` and the block caret; motions, `dd`/`ciw`/`p`/`u`/`.`; `/ ? n N *`; `:w :q :wq :<line>`; per-tab mode; a read-only file has no Vim; Settings → Editor shows the Vim switch with its input-source note; a Vietnamese IME session composes in Insert and Normal mode requires ABC.

Verify: the accessibility tree or window screenshots show each item pass, with the exact failing step recorded otherwise.

Progress: done
Result: the user ran the manual pass locally on the built `/Users/nus/Applications/Goko.app` (2026-11-19) and confirmed it: Vim mode, `zz`/`zt`/`zb`, and the IME guard in Normal mode all behave. `agent-desktop` could not read this session's window, so the observation is the user's report.

### Task 9 — Scroll commands `zz`, `zt`, `zb`

Files: `apps/rune/src/editor/vim.rs` (the `z` prefix and `Effect::Scroll`), `apps/rune/src/editor/code.rs::CodeState::scroll_line`

Interfaces: consumes the engine's pending-prefix grammar; produces the scroll effect and the host's instant scroll.

Change: `z` becomes a pending prefix (shown on the chip); `zz`, `zt` and `zb` ask the host to centre the caret line, put it at the top or put it at the bottom of the viewport without moving the caret. The commands work in Normal and Visual; a count is ignored; any other key after `z` drops the prefix.

Verify: `cargo test -p goko-rune editor::vim` passes the `zz_zt_zb_ask_for_scroll_without_moving` table (each command returns its `Effect::Scroll`, the caret does not move, the chip shows `z`, and an unknown `z` command is dropped).

Progress: done
Result: `cargo test -p goko-rune editor::vim` — 21 passed; `cargo clippy --workspace -- -D warnings` — clean. Added after the user's local test found `zz` missing (2026-11-19).

### Task 10 — Reject native edits outside Insert

Files: `apps/rune/src/editor/code.rs::Code::field` (`on_input_change`), `apps/rune/src/editor/files.rs` (runtime test)

Interfaces: consumes the routing from Task 2 and the mode state; produces the guard that keeps Normal/Visual read-only for native input.

Change: a native field change (an IME commit, a paste) while Vim is on and the mode is not Insert is undone before the change notification, so the buffer never changes outside Insert even though the pinned Rune rev cannot disable the input method. Insert keeps native typing.

Verify: `cargo test -p goko-rune` passes `vim_normal_rejects_an_ime_commit` (`Ime::Commit("x")` in Normal leaves the text and caret unchanged) and the existing Insert typing test.

Progress: done
Result: `cargo test -p goko-rune vim` — 41 passed; `cargo clippy --workspace -- -D warnings` — clean. Added after the user's local test showed IME commits editing in Normal mode (2026-11-19).

## Files inspected, no change

- `apps/rune/src/editor/draft.rs` — `OpenDoc` already owns the `CodeState`; no field needed.
- `apps/rune/src/editor/tabs.rs` — parked documents keep their `CodeState`, so per-tab mode survives.
- `apps/rune/src/editor/navigation.rs` — `Guard`/`save_resume` and `PendingNav::CloseTab` are reused for `:q`/`:wq`.
- `apps/rune/src/editor/keyboard.rs` — `preempt_key` runs before `field_key`; ordering unchanged.
- `apps/rune/src/editor/indent.rs` — `plan_indent`, `auto_indent` and `TAB_STOP` are reused.
- `apps/rune/src/editor/comment.rs` — the plan-function pattern the engine follows.
- `apps/rune/src/editor/blame.rs` — the settings read/apply pattern.
- `apps/rune/src/editor/backend.rs` — job lanes unchanged; only the `apply.rs` reply arm changes.
- `crates/rune-kit` — no component change.

## Approval Gate

Status: approved

## Decision Log

- 2026-11-19: Scope is the editor code field only — no terminal, find bar or commit message box.
- 2026-11-19: Normal-mode IME is a documented limitation (ABC input source required) rather than a Rune fork change and rev bump.
- 2026-11-19: The block caret is an in-app overlay; no Rune change.
- 2026-11-19: The unnamed register lives on `EditorState` (shared across tabs); mode, pending keys, goal column and last change live per `CodeState`.
- 2026-11-19: v1 keeps the issue's editor scope, including search, the `:` command line and `.` repeat.
- 2026-11-19: Joint approval of `design.md`, `tasks.md` and `delta.md` — "implement đi".
- 2026-11-19: The manual UI pass is delegated to the user, who tests the built `target/Goko.app` locally; the change stays open until that result is reported.
- 2026-11-19: The user's local test found `zz` missing; `zz`/`zt`/`zb` were added in Task 9.
- 2026-11-19: The user's local test showed IME commits editing in Normal mode; native changes outside Insert are now reverted (Task 10).
- 2026-11-19: The user confirmed the manual pass on the installed build and asked to commit.
