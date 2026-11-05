# Desktop Theme

## Requirements

### THM-dark-palette

The desktop SHALL apply the Dark+ inspired palette to the app shell, editor, and terminal surface by default, while allowing the user to choose among the bundled palettes.

#### Scenario: Start in the default dark theme

- GIVEN no `goko.theme` preference is stored
- WHEN the desktop initializes the theme
- THEN it uses a `#1e1e1e` background, `#252526` panels, `#cccccc` text, a `#007acc` primary accent, readable `#3794ff` accent text, and Dark+ syntax colors

#### Scenario: Choose a bundled theme

- GIVEN the desktop is running
- WHEN the user selects a theme on the Settings page
- THEN that palette applies to the UI, `goko.theme` stores the selection, and mounted terminals recolor to the new surface colors

#### Scenario: Follow the OS appearance

- GIVEN `goko.theme` is `system`
- WHEN the OS appearance changes
- THEN the desktop applies the matching Light or Dark+ palette without a restart

Evidence: `apps/desktop/src/features/settings/SettingsPage.tsx`, `apps/desktop/src/components/use-theme.ts`, `apps/desktop/src/styles/tokens.css`, `apps/desktop/src/styles/index.css`, `apps/desktop/src/App.tsx`, `apps/desktop/src/components/ui/button.tsx`, `apps/desktop/src/components/ui/badge.tsx`, `apps/desktop/src/components/ui/empty.tsx`, `apps/desktop/src/components/ui/field.tsx`, `apps/desktop/src/features/editor/code-theme.ts`, `apps/desktop/src/features/terminal/TerminalController.ts`

## Flow

The Theme picker on the Settings page lists the bundled palettes plus a System option that follows the OS appearance through `prefers-color-scheme`. The shared `useTheme` store applies the stored palette before first paint and writes the resolved theme id to `data-theme` on the document root (Dark+ removes the attribute), saving each selection as `goko.theme`. Tailwind utilities resolve through the shared CSS tokens. CodeMirror uses those tokens for its editor and syntax colors; xterm reads its background, foreground, and cursor colors when each terminal mounts and re-reads them whenever `data-theme` changes.

## State changes

The selected theme id is stored in local storage under `goko.theme`. A resolved non-Dark+ palette sets `data-theme` to that theme id; Dark+ removes the attribute. The `system` selection stores `system` while `data-theme` carries the resolved Light or Dark+ palette. Mounted terminals update their surface colors on `data-theme` changes.

## Side effects

The Theme picker updates the document root attribute and browser local storage; mounted terminals recolor through a MutationObserver on `data-theme`. It makes no Tauri or daemon calls.

## Authorization & constraints

Dark+ is the default for missing or unrecognized stored values. `system` resolves through `prefers-color-scheme`, and an OS appearance change re-applies the matching Light or Dark+ palette while `system` stays selected.

## Error paths

No theme-specific error state is defined in the current UI.

## Tests

`apps/desktop/src/features/settings/SettingsPage.test.tsx` covers reading the stored theme, applying `data-theme`, persisting a new selection, and resolving `system` through the OS preference. `apps/desktop/src/features/terminal/TerminalController.test.ts` covers live terminal recoloring on theme changes.

## Related

- [Architecture overview](../architecture/overview.md)

## Sources

- `apps/desktop/src/features/settings/SettingsPage.tsx`
- `apps/desktop/src/features/settings/SettingsPage.test.tsx`
- `apps/desktop/src/components/use-theme.ts`
- `apps/desktop/src/styles/tokens.css`
- `apps/desktop/src/styles/index.css`
- `apps/desktop/src/App.tsx`
- `apps/desktop/src/components/ui/button.tsx`
- `apps/desktop/src/components/ui/badge.tsx`
- `apps/desktop/src/components/ui/empty.tsx`
- `apps/desktop/src/components/ui/field.tsx`
- `apps/desktop/src/features/editor/code-theme.ts`
- `apps/desktop/src/features/terminal/TerminalController.ts`
- `apps/desktop/src/features/terminal/TerminalController.test.ts`
