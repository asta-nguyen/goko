# Delta

Merged into docs/llm/: 2026-11-09

## MODIFIED Requirements

### RNC-view-switch

Previously: Editor renders a placeholder instead of the terminal stub.

Baseline:

````md
### RNC-view-switch

The system SHALL switch `State::view` between `Terminal` and `Editor` when the
view-switch items are pressed; `Editor` keeps the tab strip and controls and
renders a placeholder in place of the terminal stub. The switch is a radio group
(arrow keys move and select) with one thumb, shared by id (`view-thumb`), that
glides to the newly active segment; an inactive segment lifts to the text colour
on hover and every segment shrinks slightly while pressed.

#### Scenario: switch to editor

- GIVEN `view` is `Terminal`
- WHEN the Editor item is pressed
- THEN `view` becomes `Editor` and the content area shows the editor placeholder

#### Scenario: switch back

- GIVEN `view` is `Editor`
- WHEN the Terminal item is pressed
- THEN `view` becomes `Terminal` and the terminal stub renders again

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/main.rs`
````

Replacement:

````md
### RNC-view-switch

The system SHALL switch between Terminal and the native Editor workspace through the view-switch radio group, showing the real-checkout Files, Changes, History, Stashes, and Servers views in Editor while guarding navigation away from an unsaved draft with Save, Discard or Cancel and preserving the switch's keyboard and motion behavior.

#### Scenario: switch to editor

- GIVEN `view` is `Terminal`
- WHEN the Editor item is pressed
- THEN `view` becomes `Editor` and the native editor offers real registered-project and checkout selection

#### Scenario: cancel navigation away from a draft

- GIVEN `view` is `Editor` and the current file has unsaved changes
- WHEN the Terminal item is pressed and the user chooses Cancel
- THEN the editor and its draft remain active; a failed Save also keeps the editor and draft active

#### Scenario: switch back after resolving a draft

- GIVEN `view` is `Editor` and there is no dirty draft, or Save/Discard has resolved it
- WHEN navigation to Terminal proceeds
- THEN `view` becomes `Terminal` and the existing terminal stub renders again

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/main.rs`, `apps/rune/src/editor/workspace.rs`, `apps/rune/src/editor/navigation.rs`
````

### RNC-controls-bar

Previously: The strip's render condition names only session selection, so the
requirement reads as unconditional across views.

Baseline:

````md
### RNC-controls-bar

The system SHALL render the controls strip only while a session is selected,
showing the project/agent/title breadcrumb, a state badge with an
activity-aware tone dot, the elapsed time, and Stop/Close pane buttons; Stop is
dimmed when the state is not controllable or the mock is disconnected.

#### Scenario: session selected

- GIVEN `selected_session` names a `SessionView`
- WHEN the view renders
- THEN the strip shows the breadcrumb, badge, elapsed, and both buttons

#### Scenario: no session selected

- GIVEN `selected_session` is `None`
- WHEN the view renders
- THEN the controls strip renders nothing

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/data.rs`
````

Replacement:

````md
### RNC-controls-bar

The system SHALL render the controls strip only in the Terminal view while a
session is selected — the Editor view has no session pane, matching App.tsx —
showing the project/agent/title breadcrumb, a state badge with an
activity-aware tone dot, the elapsed time, and Stop/Close pane buttons; Stop is
dimmed when the state is not controllable or the mock is disconnected.

#### Scenario: session selected in Terminal view

- GIVEN `view` is `Terminal` and `selected_session` names a `SessionView`
- WHEN the view renders
- THEN the strip shows the breadcrumb, badge, elapsed, and both buttons

#### Scenario: no strip in Editor or without a session

- GIVEN `view` is `Editor`, or `selected_session` is `None`
- WHEN the view renders
- THEN the controls strip renders nothing

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/data.rs`, `apps/rune/src/main.rs`
````
