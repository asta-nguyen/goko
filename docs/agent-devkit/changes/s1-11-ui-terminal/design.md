# s1-11 Terminal controller and view

## Goal

A real xterm.js terminal bound to one daemon session, with correct sizing, a clean
lifecycle, and no React state in the output path. Implements the terminal rules of the
[S1 contract](../../s1-contract.md).

## Scope

In: `features/terminal/TerminalController.ts` and `TerminalView.tsx`. Out: session
creation form, controls, app layout (s1-12), terminal search (S5). Depends on: s1-10.

## Behavior

- **TerminalController** (plain class, no React): `mount(element)`, `dispose()`.
  Options: monospace font stack, 13 px, scrollback 5000 lines, no cursor blink, colours
  read from the theme tokens at mount.
- **Mount order** (avoids a wrong-size flash): open xterm → fit → send `resize` with the
  fitted size → then `subscribe`. Bytes from the subscription go straight to
  `term.write`. `term.onData` goes to `sessionService.write`.
- **Resize:** a `ResizeObserver` on the element; events are debounced ~100 ms, then
  `fit()` and one `resize(cols, rows)` call. A zero-size element (hidden) is skipped.
- **dispose():** idempotent. Unsubscribes (even if the subscribe promise is still
  pending), disposes the xterm listeners, the fit addon and the terminal, disconnects
  the observer, clears the timer.
- **TerminalView({sessionId})**: creates the controller in an effect keyed by
  `sessionId` and disposes it on change or unmount. Reads session metadata through
  `useSession(sessionId)` from s1-10. No selected session → empty state with a hint. `exited` → banner
  with the exit code; `failed` → banner with the error. History is still shown.
- Output never passes through `useState`, props, Zustand, or the query cache.

## Edge cases

- Switching sessions quickly → old controller is disposed before the new one mounts; no
  output from the old session lands in the new terminal.
- `dispose()` before the subscribe promise resolves → unsubscribes as soon as it does.
- `mount` called twice → second call is ignored.
- Element resized to 0×0 (tab hidden) → no resize call, no fit error.
- Reconnect re-delivers the snapshot → duplicates are already trimmed by the bridge
  (s1-09), so the controller just writes.

## Interfaces

`new TerminalController(sessionId, service)`; `<TerminalView sessionId />`.
Dependencies added: `@xterm/xterm`, `@xterm/addon-fit` (pinned).

## Verification

Vitest with `@xterm/xterm` mocked by a fake class and a fake `sessionService`; typecheck.
Real rendering is checked manually in s1-13.

## Related context

None.
