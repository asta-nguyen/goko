# Delta

Approved: user approved the design.
Merged into docs/llm/: 2026-11-05

## MODIFIED Requirements

### THM-dark-palette

Previously: two palettes (Dark+ default, Light) and mounted xterm instances kept their mount-time colors.

Baseline:

````md
### THM-dark-palette

The desktop SHALL apply the Dark+ inspired palette to the app shell, editor, and terminal surface by default, while allowing the user to choose the existing light palette.

#### Scenario: Start in the default dark theme

- GIVEN no `goko.theme` preference is stored
- WHEN the desktop initializes the theme
- THEN it uses a `#1e1e1e` background, `#252526` panels, `#cccccc` text, a `#007acc` primary accent, readable `#3794ff` accent text, and Dark+ syntax colors

#### Scenario: Choose the light theme

- GIVEN the desktop is running
- WHEN the user selects Light on the Settings page
- THEN the light tokens apply to the UI and `goko.theme` is stored as `light`; already-mounted xterm instances retain their mount-time colors

Evidence: `apps/desktop/src/features/settings/SettingsPage.tsx`, `apps/desktop/src/components/use-theme.ts`, `apps/desktop/src/styles/tokens.css`, `apps/desktop/src/styles/index.css`, `apps/desktop/src/App.tsx`, `apps/desktop/src/components/ui/button.tsx`, `apps/desktop/src/components/ui/badge.tsx`, `apps/desktop/src/components/ui/empty.tsx`, `apps/desktop/src/components/ui/field.tsx`, `apps/desktop/src/features/editor/code-theme.ts`, `apps/desktop/src/features/terminal/TerminalController.ts`
````

Replacement:

````md
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
````
