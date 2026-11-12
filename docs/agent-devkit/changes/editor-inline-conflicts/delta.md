# Delta

Approved: 2026-11-12 — inline conflict actions, undo, navigation, guarded save and explicit per-file stage

## ADDED Requirements

### RNE-inline-conflicts

Page: `docs/llm/domains/rune-editor.md`
Search: checked the RNE page and open deltas; no existing requirement covers inline conflict resolution.

Requirement:

````md
### RNE-inline-conflicts

The system SHALL offer inline Current, Incoming, and Both conflict resolution with undo, conflict navigation, and explicit Mark Resolved that saves and stages only the selected editable text file after all conflict markers are removed, preserving draft and disk-change guards and never committing automatically.

#### Scenario: Resolve a text conflict

- GIVEN an editable UTF-8 file in Conflicts
- WHEN the user opens it and accepts Current, Incoming, or Both for a block
- THEN the editor removes that block's markers, Both keeps Current before Incoming, and the edit can be undone or changed manually

#### Scenario: Finish resolving

- GIVEN the selected conflicted file has no remaining conflict markers
- WHEN the user presses Mark Resolved
- THEN the file is saved through the expected-text guard and only that file is staged; a save or stage failure is reported without committing

#### Scenario: Unsupported or incomplete conflict

- GIVEN a binary, missing, non-editable, or malformed conflict file
- WHEN the file is opened
- THEN the editor explains the limitation and does not offer unsafe automatic resolution or staging

Evidence: `apps/rune/src/editor/conflict.rs`, `apps/rune/src/editor/code.rs`, `apps/rune/src/editor/files.rs`, `apps/rune/src/editor/backend.rs`
````
