# Delta

Merged into docs/llm/: 2026-11-09

## ADDED Requirements

### EDT-terminal-file-link

Page: `docs/llm/architecture/editor.md`
Search: checked every page with prefix EDT; no existing requirement covers opening an Editor file from a path in a session's terminal output.

Requirement:

````md
### EDT-terminal-file-link

The system SHALL, on Cmd+click (macOS) or Ctrl+click (other platforms) on a file path printed in a session's terminal output that resolves to a file in that session's checkout, switch to the Editor's Files view for that checkout and open the file, revealing the printed line when the path carries `:line`.

#### Scenario: Open a printed path at its line

- GIVEN a session in a Git checkout prints `src/App.tsx:10` and `src/App.tsx` is in the checkout's file list
- WHEN the user Cmd/Ctrl+clicks that text
- THEN the Editor's Files view opens `src/App.tsx` with line 10 revealed

#### Scenario: Unresolved or unmodified click

- GIVEN a printed path names no file of the checkout, or the user clicks without Cmd/Ctrl
- WHEN the pointer is over the text or the user clicks it
- THEN an unresolved path is not underlined and nothing opens, and a click without Cmd/Ctrl never opens a file

Evidence: `apps/desktop/src/features/terminal/terminal-links.ts`, `apps/desktop/src/features/terminal/TerminalController.ts`, `apps/desktop/src/features/terminal/TerminalView.tsx`, `apps/desktop/src/stores/ui.ts`, `apps/desktop/src/features/editor/EditorWorkspace.tsx`, `apps/desktop/src/features/editor/EditorFiles.tsx`, `apps/desktop/src/features/terminal/terminal-links.test.ts`, `apps/desktop/src/features/editor/EditorWorkspace.test.tsx`
````
