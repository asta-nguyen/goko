# Terminal links: open URLs and checkout files from session output

## Goal

URLs and file paths that an agent prints in a session terminal become clickable. A URL opens in
the system browser; a path to a file in the session's checkout opens in the Editor's Files view,
at the printed line when there is one. Today the terminal is plain xterm with only the fit and
WebGL addons, so both are inert text.

## Scope

In:
- One built-in xterm `registerLinkProvider` in `TerminalController` for HTTP(S) URLs and checkout file paths.
- `tauri-plugin-opener` (Rust, npm, capability) so the webview can open a URL in the default
  browser; only `http` and `https`.
- A terminal link scanner (`apps/desktop/src/features/terminal/terminal-links.ts`) and one
  built-in xterm link provider that detects URLs and resolves printed file paths against the
  checkout's file list.
- A one-shot "open file" request in the UI store, consumed by the Editor, that switches to the
  Files section and opens the file through the existing `EditorFiles.selectPath`.
- Update the root `ARCHITECTURE.md` and terminal overview in `docs/llm/architecture/overview.md`;
  the Editor file-link requirement is added through the existing delta.

Out:
- Files outside the session's checkout (`/tmp/x.png`, `~/Desktop/x.pdf`): not linked in this
  change. Opening them with the default app or revealing them in Finder is a follow-up on the
  same opener plugin.
- A URL or path split across two terminal rows (a soft-wrapped line): not linked (known limit).
- Other URL schemes (`file:`, `mailto:`, `ssh:`), `~/` paths, Windows paths, `a/` and `b/` diff
  prefixes, and line ranges like `:10-20`.
- Any change to the daemon, the protocol, or what a session records.

## Behavior

Activation (decided: Cmd/Ctrl+click, the same modifier as Editor go-to-definition):
- A link is underlined while the pointer is over it. A click without Cmd (macOS) or Ctrl (other
  platforms) does nothing, so selecting text never opens anything. Activation uses the Editor's
  existing `goToModifier` rule (`metaKey` on macOS, `ctrlKey` elsewhere), moved to
  `apps/desktop/src/lib/utils.ts` so both features share it; Ctrl+click on macOS, which the
  webview treats as a right click, never activates.

URLs:
- The provider detects URLs that begin with `http://` or `https://` and end at whitespace, an
  angle bracket, a quote, or a backtick. Trailing sentence punctuation (`.,;:!?`) is excluded
  from the link. URL candidates are never treated as file paths.
- On activation the handler re-checks `new URL(uri).protocol` is `http:` or `https:` and calls
  the opener plugin's `openUrl`. Anything else is ignored. The capability grants
  `opener:allow-open-url` for `http://*` and `https://*` only, so the backend refuses other
  schemes even if the check were bypassed. A URL that fails `new URL` parsing is ignored.
- If opening the system browser rejects, the error is caught and the terminal remains usable.

File paths:
- Candidate token: a run of characters other than whitespace, brackets, angle brackets, quotes,
  backticks, commas, and semicolons, with trailing `.` and `:` trimmed, that contains a `/` or
  ends in `.<ext>` (1 to 10 letters or digits), optionally followed by `:line` or `:line:col`.
  A leading `./`, `../`, or `/` is kept.
- A candidate is underlined only if it resolves. Resolution maps it to a checkout-relative path
  that is in the checkout's file list (`git_view_file_list`: tracked and untracked, not ignored):
  1. absolute path: it must equal the checkout root plus `/`; strip that prefix;
  2. relative path: join it to the session's live `cwd` (made checkout-relative), normalise `.`
     and `..`;
  3. relative path: as written, from the checkout root. The first of 2 and 3 found in the list
     wins; a result that leaves the checkout is dropped.
- On activation, `openEditorFile(workspaceId, path, line)` stores the request and switches the
  active view to the Editor. The Editor opens Files, selects the path (through the unsaved-draft
  guard) and reveals `line` when given. The request is consumed once.
- The file list is read lazily: only when a hovered row has a candidate token, through the
  shared query cache with a 5 s stale time, so output the agent just wrote is found without a
  git call per hover.

No project, or the checkout is not a Git repository, or the file list fails: no file links; URLs
still work. Sessions under "Unassigned" have no checkout and get URL links only.

## Edge cases

- Unsaved draft in the Editor when a file link opens another file: the existing navigation guard
  asks to save, discard, or cancel; cancel leaves the Editor where it was.
- The same file is already open: only the line reveal runs.
- The request's workspace is not the Editor's current checkout, or the Editor cannot show the
  checkout (missing, not a repo): the request is dropped and never replays later; the Editor
  clears any unconsumed request when it unmounts.
- The path is in the printed output but the file was deleted since: it does not resolve after the
  5 s cache window, so it is not underlined; if it was removed within the window, the Files
  view's existing missing-file state shows.
- `:line` past the end of the file: the editor clamps to the last line (existing behavior).
- A hovered row is re-scanned on every hover; URL and file candidates are found in one scan and
  the file list comes from the cache, so rapid pointer movement costs no extra git calls. A
  URL-only row does not read the file list.
- Terminal apps that capture the mouse (a TUI with mouse reporting on): xterm sends the click to
  the app, so links in that app's screen need its mouse mode off or Shift held (xterm's own
  behavior; not changed here).
- The webview URL is attacker-influenced text (agent output): only `http(s)` is opened, only on
  an explicit Cmd/Ctrl+click, and no output is ever executed.

## Interfaces

- `package.json` (desktop): add `@tauri-apps/plugin-opener` at `2.7.2`; do not add an xterm links
  dependency.
- `src-tauri/Cargo.toml`: add `tauri-plugin-opener` pinned to `=2.7.2` like
  `tauri-plugin-dialog`;
  `src-tauri/src/lib.rs`: `.plugin(tauri_plugin_opener::init())`;
  `src-tauri/capabilities/default.json`: add the scoped `opener:allow-open-url` permission (never
  `opener:default`, which also allows revealing paths).
- `TerminalController(sessionId, service, links?)` where
  `links?: { openUrl(url: string): void; file?: { resolve(raw: string): Promise<string | null>; open(path: string, line?: number): void } }`.
  The controller registers one built-in link provider only when `links` is given; it handles
  HTTP(S) URL refs and, when available, resolved file refs.
- `TerminalView` gains optional props `workspaceId`, `projectId`, `checkoutPath`
  (`TerminalWorkspace` passes them) and builds `links`, reading the session's `cwd` from
  `useSession` through a ref at hover time.
- `terminal-links.ts` exports `findUrlRefs(text: string): { start: number; end: number; url: string }[]`,
  `findFileRefs(text)`, and `resolveFileRef(ref, { checkoutPath, cwd, files })`, pure and unit-tested.
- `src/lib/utils.ts` exports `goToModifier(event: MouseEvent)`, moved unchanged from
  `features/editor/CodeEditor.tsx`, which imports it from there.
- UI store (`stores/ui.ts`): `editorOpenRequest: { workspaceId, path, line?, nonce } | null`,
  `openEditorFile(workspaceId, path, line?)`, `clearEditorOpenRequest()`.
- `EditorWorkspace` reads the request; when its workspace id equals
  `checkoutKey(projectId, checkoutPath)` it sets the section to `files` and passes
  `openRequest` and `onOpenHandled` to `EditorFiles`, which calls `selectPath` once per nonce.

## Verification

- Unit: `terminal-links.test.ts` (HTTP(S) URL detection and delimiters; file token detection with
  `:line` and `:line:col`, trailing punctuation, markdown-link parentheses, URLs not matched as
  files, absolute/relative/cwd resolution, escape attempts, misses); store tests for
  `openEditorFile` and clearing;
  `TerminalController` tests that links are registered only when `links` is given and that a
  single provider handles URLs and file refs, only HTTP(S) URLs open after a modified click, and
  a plain click does nothing; `TerminalView` tests for props, `cwd`, and URL-only rows avoiding
  file-list reads; `EditorWorkspace` and `EditorFiles` tests for open-at-line, the draft guard,
  a dropped foreign request, and one-shot consumption.
- Commands, from `apps/desktop`: `pnpm typecheck`, `pnpm test`; from the repo root:
  `cargo test --workspace`, `cargo build --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo fmt --check`
  (the opener plugin and capability must build).
- App check the user runs (`pnpm -C apps/desktop tauri dev`): in a Shell session print an
  `https://` URL and `src/App.tsx:10` (a file that exists); both underline on hover; plain click
  does nothing; Cmd+click opens the browser and the Editor Files view at line 10; a path to a
  missing file and a `file:///` URL are not linked.

## Related context

- [Desktop Editor](../../../../llm/architecture/editor.md)
- [Architecture overview](../../../../llm/architecture/overview.md)
