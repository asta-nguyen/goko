# Rune chrome

The `goko-rune` app (`apps/rune`, Rune/wgpu/winit) renders the Goko shell chrome —
the 48 px header, the workspace tab strip, and the session controls strip — from
mock data shaped like the protocol/Tauri types. Daemon and live-terminal wiring
land in a later milestone, so sessions never really run: the chrome's buttons
mutate the in-memory mock `Data`/`State` instead (spawn shell sessions, stop or
close them, switch workspace views). The project sidebar and terminal stub are
separate regions outside this page's scope.

## Requirements

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

### RNC-header-status

The system SHALL render the header with the goko mark and wordmark, a persistent
6 px connection dot in a 12 px halo (success when `Data::connected`, danger
otherwise) whose wordmark group carries an accessible name and hover tooltip
("Connected to the Goko daemon" / "Disconnected from the Goko daemon"), one
usage chip per `Data::usage` entry, and a settings button; a "Disconnected" pill
appears on the right only while `connected` is false, entering with a short fade
and small scale-up.

#### Scenario: connected mock

- GIVEN `Data::connected` is `true`
- WHEN the view renders
- THEN the connection dot is success-coloured and no Disconnected pill is shown

#### Scenario: disconnected mock

- GIVEN `Data::connected` is `false`
- WHEN the view renders
- THEN the connection dot is danger-coloured and the Disconnected pill renders beside the settings button

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/data.rs`

### RNC-new-tab

The system SHALL create a workspace tab when New tab is pressed: it spawns a
mock running `shell` session via `data::spawn_shell` titled `Shell N`, pushes
the title onto `open_tabs`, makes it active, and selects the session.

#### Scenario: press New tab

- GIVEN `open_tabs` holds several titles
- WHEN New tab is pressed
- THEN a `Shell N` session is appended to `Data::sessions`, a matching tab opens and activates, and `selected_session`/`active_workspace` follow it

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/data.rs`

### RNC-session-actions

The system SHALL make the controls strip's buttons mutate mock session state:
Stop marks a live selected session `Exited` with `ended_at` frozen at
`MOCK_NOW_MS` and clears its activity, and Close pane removes the session from
`Data::sessions` plus its title from `open_tabs`, then re-syncs selection to
the active tab.

#### Scenario: stop a running session

- GIVEN the selected session's state is `Running`
- WHEN Stop is pressed
- THEN its state becomes `Exited`, `ended_at` is set, and the badge/elapsed freeze

#### Scenario: close the selected pane

- GIVEN the selected session's title is open as a tab
- WHEN Close pane is pressed
- THEN the session and tab are removed and selection moves to the active tab's session

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/data.rs`

### RNC-settings-toggle

The system SHALL toggle `State::view` between `Settings` and `Terminal` when
the header settings button is pressed; `Settings` replaces the whole workspace
column (tab strip, controls, content) with a placeholder, matching App.tsx's
settingsMode. The button is a keyboard-focusable toggle named "Settings" with a
tooltip, pressed (`checked`) while `view` is `Settings`; it shows that state as an
accent tint with the gear turned one 60° notch (a spring, instant under Reduce
Motion), and gives hover wash, press wash and a 0.95 press scale as feedback.

#### Scenario: open settings

- GIVEN `view` is `Terminal`
- WHEN the settings button is pressed
- THEN `view` becomes `Settings` and the workspace column shows the settings placeholder

#### Scenario: leave settings

- GIVEN `view` is `Settings`
- WHEN the settings button is pressed again
- THEN `view` returns to `Terminal`

Evidence: `apps/rune/src/chrome.rs`, `apps/rune/src/main.rs`

### RNC-tab-activate

The system SHALL activate a workspace tab when its pill is pressed, setting
`active_tab` to that index and selecting the session whose title matches, which
also sets `active_workspace` to that session's checkout. A hovered inactive tab
washes lighter and lifts its title, and stays visibly distinct from the active
tab; tabs are keyed by title so neighbours slide into place when one closes.

#### Scenario: press a tab

- GIVEN `open_tabs` holds several titles and `active_tab` points elsewhere
- WHEN the user presses a tab pill
- THEN `active_tab` becomes that index and `selected_session`/`active_workspace` follow the matching `SessionView`

Evidence: `apps/rune/src/chrome.rs`

### RNC-tab-close

The system SHALL remove a workspace tab when its close control is pressed,
re-indexing `active_tab` and reselecting the session of the tab that becomes
active, or clearing the selection when no tabs remain.

#### Scenario: close the active tab

- GIVEN three open tabs with the last one active
- WHEN its close control is pressed
- THEN the tab is removed, `active_tab` clamps to the new last index, and the matching session is selected

Evidence: `apps/rune/src/chrome.rs`

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

## Flow

`main.rs`'s `view` composes `chrome::header` full-width, then a row of
`sidebar::sidebar`, `sidebar::separator`, and a workspace column. When
`State::view` is `Settings` the column is only a placeholder; otherwise it
holds `chrome::tab_bar`, `chrome::controls_bar`, and the content area —
`term::terminal_area` for `Terminal`, a placeholder for `Editor`. Each chrome
function reads `State` (mock `Data`, tab list, selection, view) and `Ink`
(palette) and returns an `El` element tree; shared builders (`txt`, `ic`,
`btn_sm`, `btn_icon`, `usage_chip`) live in `theme.rs`.

## State changes

In-memory only: `open_tabs`, `active_tab`, `selected_session`,
`active_workspace`, `view`, and `Data::sessions` mutate on tab press/close,
New tab, Stop, Close pane, the view switch, and the settings button. Nothing
persists; there is no daemon, socket, or database in this app.

## Side effects

None found — the app renders mock data and performs no I/O beyond the window.

## Authorization & constraints

None — a local prototype with no access checks or validation.

## Error paths

`controls_bar` returns an empty element when no session is selected. Closing the
last tab clears `selected_session`/`active_workspace`; `active_tab` re-clamps
with `saturating_sub`. `sync_selection` clears the selection when the active
tab's title matches no session. Stop no-ops on non-live states, and the split
buttons remain inert — real pane splits need a layout the milestone doesn't
have — so they render dimmed and disabled, with no hover, a tooltip, and no Tab
stop. Disconnection only dims Stop and shows the header pill.

## Tests

Tests: none found — searched `apps/rune` for `#[test]`, `#[cfg(test)]`, and
`mod tests`; no matches.

## Related

- [Architecture overview](../architecture/overview.md)

## Sources

- `apps/rune/src/chrome.rs`
- `apps/rune/src/theme.rs`
- `apps/rune/src/data.rs`
- `apps/rune/src/main.rs`
