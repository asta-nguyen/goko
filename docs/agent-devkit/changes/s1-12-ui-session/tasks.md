# s1-12 Session form, controls, and app composition tasks

## Design

[s1-12 Session form, controls, and app composition](design.md)

## Global Constraints

- Commands are passed as text to the bridge, which splits them into `argv`; the UI never builds a shell string.
- Only the dialog open permission is added to capabilities.
- Components use semantic tokens only; no hex colours.
- Nothing in the list or controls re-renders on terminal output.

## Impact map

```text
Entry: no existing source; planned entry: apps/desktop/src/App.tsx::App — pending approval
Flow: no existing flow; planned flow: NewSessionForm → sessionService.create → UI store select → SessionList/SessionControls/TerminalView render the selected session — pending approval
State changes: Zustand selection; none persisted
External effects: native directory dialog — pending approval
Change candidates: apps/desktop/src/App.tsx, apps/desktop/src/main.tsx, apps/desktop/src/features/sessions/{NewSessionForm,SessionControls,SessionList}.tsx, apps/desktop/src-tauri/Cargo.toml, apps/desktop/src-tauri/src/lib.rs, apps/desktop/src-tauri/capabilities/default.json, apps/desktop/package.json
Verification: pnpm -C apps/desktop test sessions; typecheck; capabilities grep
Verified at: no commit exists
```

## Tasks

### Task 1 — New-session form with native directory picker

Files: apps/desktop/src/features/sessions/NewSessionForm.tsx::NewSessionForm, apps/desktop/src-tauri/Cargo.toml, apps/desktop/src-tauri/src/lib.rs::run, apps/desktop/src-tauri/capabilities/default.json
Interfaces: consumes `useCreateSession`, `useDaemonStatus`, and `useUi` (s1-10); produces directory and command selection.
Change: register `tauri-plugin-dialog` and grant only its open permission. The form shows the chosen directory, a command field prefilled with `default_shell`, a Start button disabled while `useCreateSession().isPending` or when input is missing, and an inline typed error (message plus hint). On success select the returned session.
Verify: `pnpm -C apps/desktop test NewSessionForm` — Start calls `create` with `{command, cwd, cols:80, rows:24}`; an error renders its message and hint; double-click calls `create` once; a cancelled dialog leaves the directory unchanged; the capability diff contains only the dialog open permission.
Progress: todo
Result: none

### Task 2 — Session controls and status badge

Files: apps/desktop/src/features/sessions/SessionControls.tsx::SessionControls
Interfaces: consumes `useSession`, `useStopSession`, and `useDaemonStatus` (s1-10).
Change: show the state badge and elapsed time (interval local to the component, only while running or stopping). Stop calls `stop(id,false)`. Force kill appears after the session has been `stopping` for 5 s and calls `stop(id,true)`. Controls are disabled for `exited`, `failed`, and when disconnected.
Verify: `pnpm -C apps/desktop test SessionControls` with fake timers — Force kill is absent at 4 s and present at 5 s of `stopping`; Stop calls the service with `force:false`; both are disabled when exited; the interval is cleared on unmount.
Progress: todo
Result: none

### Task 3 — Session list and app composition

Files: apps/desktop/src/features/sessions/SessionList.tsx::SessionList, apps/desktop/src/App.tsx::App, apps/desktop/src/main.tsx
Interfaces: consumes Tasks 1–2, `TerminalView` (s1-11), and `startQuerySync` and `queryClient` (s1-10); produces the complete S1 screen.
Change: `SessionList` renders one memoized row per session (title, badge) and selects on click. `App` composes header with daemon dot, left column (form and list), main area (controls above terminal), and the disconnected banner; it starts `startQuerySync` on mount and stops it on unmount, and `main.tsx` wraps the app in `QueryClientProvider`.
Verify: `pnpm -C apps/desktop test App` — with a fake service and a fresh `QueryClient`, two sessions render two rows and selecting the second mounts a `TerminalView` for it; `connected:false` shows the banner and disables controls; unmount stops the sync; `pnpm -C apps/desktop typecheck && pnpm -C apps/desktop build` pass; `grep -rE '#[0-9a-fA-F]{3,6}' apps/desktop/src --include=*.tsx` finds nothing.
Progress: todo
Result: none

## Files inspected, no change

- None — depends on s1-09, s1-10, s1-11, which do not exist yet.

## Approval Gate

Status: pending

## Decision Log

None.
