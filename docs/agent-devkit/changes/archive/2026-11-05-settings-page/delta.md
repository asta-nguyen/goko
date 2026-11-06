# Delta

Merged into docs/llm/: 2026-11-05

Approved: 2026-11-05 — a dedicated Settings page (gear in header, Cmd+,) holds the Vim mode switch and a Theme control; the editor header Vim toggle is removed

## MODIFIED Requirements

### EDT-vim-save

Previously: the Vim mode toggle lived in the Files editor header; the Settings page is now its only control.

Baseline:

````md
### EDT-vim-save

The system SHALL persist the optional Vim mode under `editor.vim` and route Vim `:w` to the active editor's existing save action.

#### Scenario: Save from Vim mode

- GIVEN Vim mode is enabled in the Files editor
- WHEN the user runs `:w`
- THEN the active editor invokes the same save action as the Save button

Evidence: `apps/desktop/src/features/editor/queries.ts`, `apps/desktop/src/features/editor/EditorFiles.tsx`, `apps/desktop/src/features/editor/CodeEditor.tsx`, `apps/desktop/src/features/editor/queries.test.tsx`, `apps/desktop/src/features/editor/CodeEditor.test.tsx`
````

Replacement:

````md
### EDT-vim-save

The system SHALL persist the optional Vim mode under `editor.vim` and route Vim `:w` to the active editor's existing save action.

#### Scenario: Save from Vim mode

- GIVEN Vim mode is enabled on the Settings page
- WHEN the user runs `:w`
- THEN the active editor invokes the same save action as the Save button

Evidence: `apps/desktop/src/features/editor/queries.ts`, `apps/desktop/src/features/settings/SettingsPage.tsx`, `apps/desktop/src/features/editor/CodeEditor.tsx`, `apps/desktop/src/features/editor/queries.test.tsx`, `apps/desktop/src/features/editor/CodeEditor.test.tsx`
````
