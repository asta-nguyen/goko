# Delta

Merged into docs/llm/: 2026-11-16

Approved: 2026-11-16 — right-clicking the code offers Copy Snapshot and Send Snapshot to Terminal with their chords; the menu replaces the field's built-in one, so it carries Cut, Copy, Paste and Select All too

## MODIFIED Requirements

### RNE-agent-snapshot

Page: `docs/llm/domains/rune-editor.md`

Added scenario:

````md
#### Scenario: Context menu

- GIVEN an editable file is open in Files
- WHEN the user right-clicks the code
- THEN a menu lists Cut, Copy, Paste and Select All, then Copy Snapshot (⌘⇧C) and Send Snapshot to Terminal (⌘⇧Enter), and each snapshot item acts as its chord without the code holding focus
````

Evidence: `apps/rune/src/editor/files.rs`, `apps/rune/src/editor/code.rs`, `apps/rune/src/editor/snapshot.rs`
