# Stop Goko-owned processes when the app exits

## Goal

Closing Goko’s main window or choosing Cmd+Q stops the Goko daemon and every
terminal process it owns, so no Goko process keeps running after a normal app
exit.

## Scope

- Route the main-window close request and Cmd+Q through one graceful shutdown
  path.
- Add an authenticated daemon shutdown request and use the daemon’s existing
  session shutdown behavior.
- Keep `GOKO_HOME`, `goko.db`, preferences, and other app data on normal quit.
- Do not add a Finder uninstall hook. macOS does not run app code when a user
  moves the `.app` bundle to Trash; users must quit Goko first to stop its
  processes. Removing app data remains a separate uninstall action.
- Force Quit, crashes, and OS termination do not promise graceful cleanup.

## Behavior

- The main-window close request and Cmd+Q use the same idempotent shutdown
  coordinator. Closing the only window exits the desktop app after shutdown.
- The desktop backend sends `system.shutdown` over the existing authenticated
  Unix socket. The daemon acknowledges the request, stops accepting work, shuts
  down all sessions, removes `daemon.sock`, and exits. The desktop app waits for
  the daemon lock to be released before completing its exit, so the response by
  itself is not treated as proof that shutdown finished.
- The daemon uses `SessionManager::shutdown()`: it sends SIGTERM to every live
  PTY process group, waits up to two seconds, then sends SIGKILL to any process
  still running. This stops shells, agents, and their child processes.
- If no daemon is running, Goko exits normally. If a reachable daemon rejects
  or fails the shutdown request, Goko stays open and reports the failure rather
  than silently leaving a daemon behind. A daemon from an older build that does
  not understand `system.shutdown` follows this error path.
- Quitting immediately stops active sessions; there is no confirmation prompt.
  App data and session metadata remain on disk.

## Edge cases

- Concurrent window-close and Cmd+Q requests share one shutdown operation; they
  do not race or send duplicate shutdowns.
- A daemon that has already exited is treated as stopped, and the app may exit.
- A shutdown timeout or unsupported request keeps the app open so the user can
  retry. Force Quit remains an OS-level escape and may leave processes running.
- A user who moves Goko to Trash while it is still open does not trigger cleanup;
  the user must quit first. Normal quit preserves app data.
- Active terminal work ends on close or Cmd+Q. Unsaved work inside a shell,
  editor, or agent may be lost.

## Interfaces

- `protocol::message::Request` gains the authenticated `system.shutdown`
  operation. It uses the existing protocol framing and token handshake.
- The daemon server connects that request to the shutdown path currently entered
  by SIGTERM/SIGINT; existing session signaling and timeout behavior remain the
  authority for PTY cleanup.
- The Tauri lifecycle in `apps/desktop/src-tauri/src/lib.rs::run` routes window
  close and app exit through one shutdown coordinator exposed by the existing
  daemon bridge. No frontend IPC command is added.

## Verification

- Protocol serialization check for `system.shutdown`.
- Daemon integration check: start a session with a child process, request
  `system.shutdown`, then confirm the daemon exits, its socket is removed, and
  the child process is gone.
- macOS app acceptance check: with a live shell and an agent process, close the
  window and separately use Cmd+Q; each action leaves no `desktop`, `gokod`, or
  session process, while `goko.db` remains.
- Check the unavailable-daemon and unsupported-request paths do not report a
  successful shutdown while a daemon is still serving.

## Related context

- [Architecture overview](../../../../llm/architecture/overview.md)
