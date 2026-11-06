# Delta

Merged into docs/llm/: 2026-11-06

Approved: 2026-11-06 — Cmd/Ctrl+click in the Files editor opens a clicked import path, or jumps to a clicked name's declaration found with the existing content search, falling back to Content mode when it is ambiguous

## ADDED Requirements

### EDT-go-to-definition

Page: `docs/llm/architecture/editor.md`
Search: checked every page with prefix EDT; no existing requirement covers navigating from a clicked import or name.

Requirement:

````md
### EDT-go-to-definition

The system SHALL, on Cmd+click (macOS) or Ctrl+click (other platforms) in the Files editor, open the checkout file a clicked import specifier resolves to, or jump to the declaration of a clicked name found through the checkout's content search, and when no single declaration is found, open Content mode with that name as the query.

#### Scenario: Open a clicked relative import

- GIVEN a source file with `import { x } from './util'` is open in Files and `util.ts` exists next to it
- WHEN the user Cmd/Ctrl+clicks the `'./util'` specifier
- THEN `util.ts` opens through the unsaved-draft navigation guard

#### Scenario: Jump to a unique declaration

- GIVEN a name is declared exactly once in the checkout and not in the open file
- WHEN the user Cmd/Ctrl+clicks a use of that name
- THEN the declaring file opens with the declaration line revealed

#### Scenario: Ambiguous or missing declaration

- GIVEN a clicked name has no declaration or several declarations outside the open file
- WHEN the user Cmd/Ctrl+clicks it
- THEN Content mode opens with the name as its query; an import that resolves to no checkout file shows a short notice instead

Evidence: `apps/desktop/src/features/editor/go-to-definition.ts`, `apps/desktop/src/features/editor/CodeEditor.tsx`, `apps/desktop/src/features/editor/EditorFiles.tsx`, `apps/desktop/src/features/editor/go-to-definition.test.ts`, `apps/desktop/src/features/editor/CodeEditor.test.tsx`, `apps/desktop/src/features/editor/EditorWorkspace.test.tsx`
````
