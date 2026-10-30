# s1-12 Session form, controls, and app composition

## Goal

The usable screen: pick a directory, enter a command, start a session, see it in a list,
switch between sessions, and stop or force-kill the selected one. Implements the
frontend behavior of the [S1 contract](../../../s1-contract.md).

## Scope

In: `NewSessionForm`, `SessionControls`, `SessionList`, `App` composition, the dialog
plugin. Out: sidebar with projects (S2), command palette (S5). Depends on: s1-09, s1-10,
s1-11.

## Behavior

- **NewSessionForm:** a directory button using the native open-directory dialog, a
  command field prefilled from `status().default_shell`, a Start button, and an inline
  error with the message and hint from a `BridgeError`. Start calls `create` with
  80×24 (the terminal resizes on mount) and selects the new session on success. The
  form is disabled while the call is pending.
- **SessionControls:** state badge, elapsed time, Stop (`force:false`) and Force kill.
  Force kill appears only when the state has been `stopping` for 5 s. Both are hidden or
  disabled for `exited` and `failed`. The elapsed-time tick lives inside the component so
  only it re-renders.
- **SessionList:** one row per session (title, state badge), click to select; the
  selected row is highlighted. Rows are memoized on session metadata.
- **App:** header (name and a daemon status dot), left column with `NewSessionForm` and
  `SessionList`, main area with `SessionControls` above `TerminalView`. When
  `connected` is false a banner explains the daemon is unreachable and that it is being
  restarted. `startQuerySync` runs once on mount and is stopped on unmount; `main.tsx` wraps the app in `QueryClientProvider`.
- Capabilities add only the dialog plugin's open permission.

## Edge cases

- Dialog cancelled → directory unchanged, no error.
- Empty command or no directory → Start disabled with a hint.
- `create` fails (bad cwd, command not found) → inline error; a `failed` session returned
  by the daemon is selected and shows its banner.
- Rapid double-click on Start → one session (disabled while pending).
- Daemon disconnected → controls disabled, banner visible, recovers on reconnect.
- Many sessions → list scrolls; no per-output re-render of the list.

## Interfaces

`<NewSessionForm />`, `<SessionControls sessionId />`, `<SessionList />`, `App`.
Dependencies added: `@tauri-apps/plugin-dialog` (JS) and `tauri-plugin-dialog` (Rust).

## Verification

Vitest component tests with mocked service, a fresh `QueryClient`, and fake timers; typecheck; capability
file contains only the dialog open permission addition. Manual flow in s1-13.

## Related context

None.
