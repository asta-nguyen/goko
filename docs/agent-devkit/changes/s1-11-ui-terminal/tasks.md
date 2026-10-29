# s1-11 Terminal controller and view tasks

## Design

[s1-11 Terminal controller and view](design.md)

## Global Constraints

- Terminal output never enters React state, props, Zustand, or the query cache.
- One xterm instance per mounted view; every listener, addon, and observer is disposed on unmount.
- Resize is debounced (~100 ms) and sent once per burst.
- Scrollback is bounded (5000 lines).

## Impact map

```text
Entry: no existing source; planned entry: apps/desktop/src/features/terminal/TerminalView.tsx::TerminalView — pending approval
Flow: no existing flow; planned flow: TerminalView effect → TerminalController.mount → sessionService.subscribe → term.write; term.onData → sessionService.write; ResizeObserver → debounce → fit → sessionService.resize — pending approval
State changes: none beyond the xterm instance owned by the controller
External effects: DOM listeners, ResizeObserver (all disposed) — pending approval
Change candidates: apps/desktop/package.json, apps/desktop/src/features/terminal/{TerminalController.ts,TerminalView.tsx}, tests beside them
Verification: pnpm -C apps/desktop test terminal; typecheck
Verified at: no commit exists
```

## Tasks

### Task 1 — `TerminalController`

Files: apps/desktop/src/features/terminal/TerminalController.ts::TerminalController
Interfaces: consumes `sessionService.subscribe/write/resize` (s1-10); produces `mount(element)` and `dispose()`.
Change: add `@xterm/xterm` and `@xterm/addon-fit`. Implement the mount order, input forwarding, debounced resize, and idempotent dispose from the design, including unsubscribing when `dispose` runs before the subscribe promise resolves.
Verify: `pnpm -C apps/desktop test TerminalController` with a fake terminal and service — mount then dispose leaves zero registered listeners and one unsubscribe; mount, dispose, mount again does not duplicate written bytes; ten rapid resizes produce one `resize` call; a 0×0 element produces none; dispose before the subscribe promise resolves still unsubscribes; `resize` is called before `subscribe`.
Progress: todo
Result: none

### Task 2 — `TerminalView`

Files: apps/desktop/src/features/terminal/TerminalView.tsx::TerminalView
Interfaces: consumes Task 1 and `useSession` (s1-10); produces a full-height component for one session.
Change: create the controller in an effect keyed by `sessionId`, dispose in the cleanup, and render the empty state, exit-code banner, or error banner from the session's state. No `useState` for output.
Verify: `pnpm -C apps/desktop test TerminalView` — switching `sessionId` disposes the old controller before the new one mounts; an `exited` session shows its exit code; a `failed` session shows its error; no session shows the empty state; unmount disposes; `pnpm -C apps/desktop typecheck` passes.
Progress: todo
Result: none

## Files inspected, no change

- None — depends on s1-10, which does not exist yet.

## Approval Gate

Status: pending

## Decision Log

None.
