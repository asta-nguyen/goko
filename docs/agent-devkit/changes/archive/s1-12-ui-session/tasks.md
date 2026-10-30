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
Entry: apps/desktop/src/App.tsx::App
Flow: NewSessionForm → sessionService.create → UI store select → SessionList/SessionControls/TerminalView render the selected session
State changes: Zustand selection; none persisted
External effects: native directory dialog
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
Progress: done
Result: Done on the frontend: `NewSessionForm.tsx` (+ shared `StateBadge.tsx`) and `@tauri-apps/plugin-dialog` 2.8.4 (exact pin; its `@tauri-apps/api ^2.12.0` matches our 2.12.4). `pnpm -C apps/desktop test NewSessionForm` — 6 tests pass: Start stays disabled with a hint until a directory is chosen and the command is non-blank, and the command is prefilled from `default_shell`; Start calls `create` with `{command, cwd, cols:80, rows:24}` and selects the returned session; a `BridgeError` renders message and hint and selects nothing; two clicks in one tick call `create` once (the test fails without the `inFlight` ref guard, because `isPending` lags a render) and the form is disabled through its `<fieldset>`; a cancelled dialog leaves the directory unchanged with no error; a rejected `open()` shows an inline error. `pnpm -C apps/desktop typecheck` passes. Rust half: `tauri-plugin-dialog` `=2.8.4` (same version as the JS plugin) is a dependency of `desktop`, registered with `.plugin(tauri_plugin_dialog::init())` in `lib.rs::run`, and `capabilities/default.json` gained only `dialog:allow-open` (its permissions are now `core:event:default` and `dialog:allow-open`); the build validates the identifier and the generated schema lists it as "Enables the open command without any pre-configured scope"; `grep -E 'shell|fs:|http'` on the file finds nothing. `cargo check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (135 tests), and `pnpm -C apps/desktop test` (72 tests) pass. The native dialog itself has not been opened by a person: that is part of the s1-13 manual checks.

### Task 2 — Session controls and status badge

Files: apps/desktop/src/features/sessions/SessionControls.tsx::SessionControls
Interfaces: consumes `useSession`, `useStopSession`, and `useDaemonStatus` (s1-10).
Change: show the state badge and elapsed time (interval local to the component, only while running or stopping). Stop calls `stop(id,false)`. Force kill appears after the session has been `stopping` for 5 s and calls `stop(id,true)`. Controls are disabled for `exited`, `failed`, and when disconnected.
Verify: `pnpm -C apps/desktop test SessionControls` with fake timers — Force kill is absent at 4 s and present at 5 s of `stopping`; Stop calls the service with `force:false`; both are disabled when exited; the interval is cleared on unmount.
Progress: done
Result: `pnpm -C apps/desktop test SessionControls` — 10 tests pass with fake timers: badge and elapsed time tick while running (00:00 → 01:05 after 65 s); Stop calls the service with `force:false`; Force kill is absent at 4 s of `stopping` and present at 5 s (fails if the threshold is changed) and calls `stop(id, true)`, and disappears when the session exits; Stop is disabled for `exited` and `failed` (Force kill is hidden there, never shown) and their elapsed time is frozen at `ended_at - started_at` with no interval created; Stop and Force kill are disabled while `connected:false` and re-enabled on reconnect; a failed stop shows its message; every `setInterval` is cleared on unmount (fails without the `clearInterval`). `pnpm -C apps/desktop typecheck` passes. Rulings: Force kill is timed from when the component first sees `stopping` (the daemon sends no stop timestamp), so opening an already-stopping session waits a further 5 s; the elapsed time for a live session counts from `started_at`. Test note: TanStack notifies observers from a 0 ms timer, so tests with fake timers flush it with `advanceTimersByTime(0)`.

### Task 3 — Session list and app composition

Files: apps/desktop/src/features/sessions/SessionList.tsx::SessionList, apps/desktop/src/App.tsx::App, apps/desktop/src/main.tsx
Interfaces: consumes Tasks 1–2, `TerminalView` (s1-11), and `startQuerySync` and `queryClient` (s1-10); produces the complete S1 screen.
Change: `SessionList` renders one memoized row per session (title, badge) and selects on click. `App` composes header with daemon dot, left column (form and list), main area (controls above terminal), and the disconnected banner; it starts `startQuerySync` on mount and stops it on unmount, and `main.tsx` wraps the app in `QueryClientProvider`.
Verify: `pnpm -C apps/desktop test App` — with a fake service and a fresh `QueryClient`, two sessions render two rows and selecting the second mounts a `TerminalView` for it; `connected:false` shows the banner and disables controls; unmount stops the sync; `pnpm -C apps/desktop typecheck && pnpm -C apps/desktop build` pass; `grep -rE '#[0-9a-fA-F]{3,6}' apps/desktop/src --include=*.tsx` finds nothing.
Progress: done
Result: `pnpm -C apps/desktop test App` — 4 tests pass with a fake service, a fresh `QueryClient`, a fake `listen` registry and a faked `TerminalController` (jsdom has no layout): two sessions render two rows, nothing mounts until a row is clicked, selecting `b` mounts a `TerminalView` for `b` (`new:b`) with `aria-current` on that row and Stop visible, switching to `a` logs `dispose:b` before `new:a`; `daemon://status {connected:false}` shows the "unreachable" banner, sets the header dot label to disconnected and disables Stop, and `connected:true` removes the banner and re-enables Stop after the refetch; a pushed `session://updated` for an unknown id adds a row; unmount removes all three event listeners. `pnpm -C apps/desktop test SessionList` — 2 tests pass (empty hint; only the changed row re-renders, and the test fails without `memo`). `pnpm -C apps/desktop test sessions` — 4 files, 28 tests pass. `pnpm -C apps/desktop typecheck` passes and `pnpm -C apps/desktop build` succeeds: JS `601.95 kB` (gzip `167.35 kB`), CSS `12.83 kB` (gzip `3.58 kB`) — up from `219.97 kB` JS because `App` now bundles xterm, and Vite warns about a chunk over 500 kB (not acted on: a local desktop app, no code-splitting added). The plan's grep `grep -rE '#[0-9a-fA-F]{3,6}' apps/desktop/src --include='*.tsx'` finds nothing (the glob must be quoted in zsh); no hex in any `.ts`/`.tsx` outside `styles/`. `App` header dot is `bg-success`/`bg-danger`/`bg-muted` for connected/disconnected/unknown. `main.tsx` wraps `App` in `QueryClientProvider` with the shared `queryClient`; `startQuerySync` starts in an effect and the effect returns its stop function. The disconnected banner says only that the daemon is unreachable and being restarted.

## Files inspected, no change

- `services/`, `queryClient.ts`, `features/sessions/queries.ts`, `stores/ui.ts` (s1-10) and `features/terminal/TerminalView.tsx` (s1-11), all archived — consumed as-is.

## Approval Gate

Status: approved

Approved by the user on 2026-10-29: "các tasks được approve hết".

## Decision Log

None.
