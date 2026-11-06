# Desktop Settings

## Requirements

### SET-settings-page

The system SHALL provide a Settings page, opened from the header Settings button or the Cmd/Ctrl+, shortcut, that groups preferences into named sections and applies each change immediately.

#### Scenario: Open the Settings page

- GIVEN the desktop is running
- WHEN the user clicks the header Settings button or presses Cmd/Ctrl+,
- THEN the main area shows the Settings page with its section navigation and preference controls

Evidence: `apps/desktop/src/App.tsx`, `apps/desktop/src/features/settings/SettingsPage.tsx`, `apps/desktop/src/stores/ui.ts`, `apps/desktop/src/App.test.tsx`, `apps/desktop/src/features/settings/SettingsPage.test.tsx`

## Flow

The header gear button and Cmd/Ctrl+, call `setActiveView('settings')`; the UI store switches the main area through the same unsaved-draft guard as other view changes, and terminal workspaces stay mounted behind it. A left rail lists the sections (Editor, Appearance); selecting one shows that section's card. The Editor section's Vim mode switch reuses `useEditorVim`, so toggling reconfigures any open CodeMirror editor at once (see [Desktop Editor](../architecture/editor.md#edt-vim-save)). The Appearance section's Theme picker lists the bundled palettes and a System option that follows the OS appearance, all through `useTheme` (see [Desktop Theme](desktop-theme.md#thm-dark-palette)).

## State changes

The Vim mode switch reads and writes `editor.vim` through the settings bridge. The Theme picker reads and writes `goko.theme` in local storage and applies `data-theme` on the document root. The active Settings section is component state only; `activeView: 'settings'` lives in the in-memory UI store and returns to `terminal` when a session opens, a workspace is activated (even the active one), or the header Settings button is pressed again.

## Side effects

Writing Vim mode persists `editor.vim` via `settings_set`; writing the theme updates local storage and the root `data-theme` attribute. No other side effects.

## Authorization & constraints

Settings are global to the app, not scoped to a project or checkout, and the page renders regardless of workspace selection.

## Error paths

A failed `editor.vim` read leaves the switch off; a failed write leaves the previous value. The page defines no additional error UI.

## Tests

`apps/desktop/src/features/settings/SettingsPage.test.tsx` covers the Vim switch read/write, theme read/apply/persist, and section switching. `apps/desktop/src/App.test.tsx` covers opening the page from the gear button and Cmd/Ctrl+,.

## Related

- [Architecture overview](../architecture/overview.md)
- [Desktop Editor](../architecture/editor.md)
- [Desktop Theme](desktop-theme.md)

## Sources

- `apps/desktop/src/App.tsx`
- `apps/desktop/src/App.test.tsx`
- `apps/desktop/src/components/use-theme.ts`
- `apps/desktop/src/components/ui/switch.tsx`
- `apps/desktop/src/features/settings/SettingsPage.tsx`
- `apps/desktop/src/features/settings/SettingsPage.test.tsx`
- `apps/desktop/src/features/editor/queries.ts`
- `apps/desktop/src/stores/ui.ts`
