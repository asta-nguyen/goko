# Delta

Issue: 27

Merged into docs/llm/: 2026-11-19

## ADDED Requirements

### RNE-vim-mode

Page: `docs/llm/domains/rune-editor.md`
Search: checked every page with prefix RNE; no existing requirement covers modal editing in the code field.

Requirement:

````md
### RNE-vim-mode

The system SHALL offer an optional Vim mode for the code field, off by default and switched in Settings → Editor under `goko.editor.vim`, that routes the field's keys through a pure modal engine with Normal, Insert, Visual and Visual Line modes — motions `h j k l w b e W B E 0 ^ $ gg G f t F T ; , { } %` with counts, operators `d c y` with motions, text objects and doubled forms, `x D C p P J > < u`/`Ctrl-r`, `.` repeat, `i a I A o O` insert entries, an unnamed register mirrored to the system clipboard, `/ ? n N *` over the find bar, and `:w :q :wq :<line>` in a command bar — keeping the caret on a character outside Insert mode, drawing a block caret and a mode chip with pending keys, and leaving `⌘S`/`⌘F`/`⌘G`/`⌘/`/`⌘P` working in every mode; with the setting off the field behaves exactly as before, and in Normal mode the macOS input source must be ABC because the pinned Rune rev cannot disable the input method.

#### Scenario: Enable Vim mode

- GIVEN the Settings → Editor page with Vim mode off
- WHEN the user switches Vim mode on
- THEN `goko.editor.vim` stores `"1"`, the code field starts in Normal mode, and the footer shows `NORMAL`
- AND after a restart the field still starts in Normal mode; with the setting absent, unreadable or `"0"` it stays off and every key behaves as before

#### Scenario: Motions and counts

- GIVEN a document in Normal mode with the caret on the first character
- WHEN the user presses `w`, `3l`, `$`, `gg`, `G` or `%`
- THEN the caret lands on the expected character, never past a line's last character, and `j`/`k` keep the goal column across shorter lines

#### Scenario: Operator and text object

- GIVEN a document in Normal mode
- WHEN the user presses `dd`, `2dw`, `ciw` then types replacement text and presses `Escape`, or `y` with a motion
- THEN the affected text is deleted, changed or yanked as one undo step, `p` pastes the unnamed register after the caret, `u` reverts the command, and `.` repeats it at the new caret

#### Scenario: Insert mode keeps Vietnamese input

- GIVEN Vim mode on and the macOS Vietnamese input source active
- WHEN the user presses `i` and composes Vietnamese text
- THEN Insert mode commits the composed text normally, `Escape` returns to Normal without blurring the field, and the caret clamps onto the previous character when it sat past the line end

#### Scenario: Visual selection

- GIVEN a document in Normal mode
- WHEN the user presses `v` or `V`, extends with motions, and presses `d`, `c` or `y`
- THEN the selection is inclusive of the character under the caret, the operator applies to it as one undo step, and `Escape` returns to Normal

#### Scenario: Search

- GIVEN a document in Normal mode with the caret inside the document
- WHEN the user presses `/`, types a query and presses `Enter`, then `n` or `N`, or presses `*`
- THEN the find bar opens without moving the caret, `Enter` moves the caret to the first match after the search origin, `n`/`N` step in the recorded direction, `*` searches the word under the caret, and `Escape` restores the caret

#### Scenario: Command line

- GIVEN a document in Normal mode
- WHEN the user presses `:` and enters `w`, `q`, `wq` or a line number
- THEN `:w` saves, `:q` closes the tab through the draft guard, `:wq` saves and closes only after the save succeeds, `:<line>` reveals that line, and an unknown command leaves the bar open with an inline message

#### Scenario: Block caret and mode chip

- GIVEN Vim mode on and the code field focused
- WHEN the field is in Normal or Visual mode
- THEN the native caret is transparent, a block is drawn over the character under the caret, and the footer shows the mode and any pending keys; in Insert mode the native bar caret returns

#### Scenario: Normal mode needs the ABC input source

- GIVEN Vim mode on, a Vietnamese input source active, and Normal mode
- WHEN the user presses a letter that starts an IME composition
- THEN the key does not reach the engine, as documented in the Settings description, while Insert mode still composes Vietnamese normally

Evidence: `apps/rune/src/editor/vim.rs`, `apps/rune/src/editor/code.rs`, `apps/rune/src/editor/state.rs`, `apps/rune/src/editor/find.rs`, `apps/rune/src/settings.rs`, `apps/rune/src/main.rs`, `apps/rune/src/editor/backend/apply.rs`
````
