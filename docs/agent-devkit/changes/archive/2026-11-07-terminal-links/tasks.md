# Terminal links tasks

## Design

[Terminal links: open URLs and checkout files from session output](design.md)

## Global Constraints

- Only `http` and `https` URLs are ever opened: the handler re-checks `new URL(uri).protocol`, and the capability grants only `opener:allow-open-url` scoped to `http://*` and `https://*` (never `opener:default`).
- Activation is the Editor's `goToModifier` rule (`metaKey` on macOS, `ctrlKey` elsewhere) on a click; no other gesture opens a link and a plain click does nothing.
- A file path is linked only when it resolves to a path in the checkout's `git_view_file_list` result and stays inside the checkout; absolute paths must start with the checkout root plus `/`. Files outside the checkout, `~/` paths, and soft-wrapped paths are not linked.
- URL and file candidates are scanned per terminal row; a URL-only row does not fetch the file list, and a row's text is never executed.
- The file list is read through the shared query cache (`queryKeys.gitFileList`, `staleTime` 5000 ms) and only when the hovered row has a candidate token; a failed list or a missing project/Git checkout yields no file links while URL links still work.
- The open request is one-shot: consumed once per nonce, dropped when its workspace is not the Editor's checkout, and cleared when `EditorWorkspace` unmounts.
- No new Git command, and no change to `crates/protocol` or `crates/daemon`. The only new dependencies are `tauri-plugin-opener` and `@tauri-apps/plugin-opener`, each pinned exactly; URL and file detection use xterm's existing link-provider API.

## Impact map

Entry: `apps/desktop/src/features/terminal/TerminalController.ts::TerminalController.mount` (builds the xterm `Terminal` with `FitAddon` and `WebglAddon`, with no link provider) and `apps/desktop/src/features/terminal/TerminalView.tsx::TerminalView` (the only `new TerminalController` call site, current line 15).
Flow: `TerminalWorkspace.renderPane` (`TerminalWorkspace.tsx:351-360`) → `TerminalView` → `useSession(sessionId)` for the live `cwd` → `TerminalController.mount`. Existing Editor open path: `EditorWorkspace` in `App.tsx` when `editorMode` is true → `EditorFiles.selectPath(path, line)` (`EditorFiles.tsx:262`) → `requestEditorNavigation` guard → local `path`/`reveal` state → `useGitFile`. Existing Editor file-list flow: `useGitFileList` → `gitViewService.fileList` → `git_view_file_list` → `git_view::file_list` (`ls-files --cached --others`). Planned (pending approval): `TerminalView` reads `queryKeys.gitFileList` through `queryClient.fetchQuery`; one xterm link provider → opener plugin for HTTP(S) URLs, or `useUi.openEditorFile` → `EditorWorkspace` → `EditorFiles.openRequest` → `selectPath` for checkout files.
State changes: new in-memory UI store field `editorOpenRequest`; no persistence, no checkout or Git change.
External effects: the OS default browser opens an `http(s)` URL via the opener plugin; one cached `git_view_file_list` read per 5 s while hovering rows with path-like text.
Change candidates: `apps/desktop/src-tauri/Cargo.toml`, `src-tauri/src/lib.rs`, `src-tauri/capabilities/default.json`; `apps/desktop/package.json` and `pnpm-lock.yaml`; `src/lib/utils.ts`; `src/features/editor/{CodeEditor.tsx,EditorWorkspace.tsx,EditorFiles.tsx,EditorWorkspace.test.tsx}`; `src/stores/ui.ts` (+ its test); `src/features/terminal/{terminal-links.ts (new), terminal-links.test.ts (new), TerminalController.ts, TerminalController.test.ts, TerminalView.tsx, TerminalView.test.tsx, TerminalWorkspace.tsx}`; docs `ARCHITECTURE.md`, `docs/llm/architecture/editor.md` via the delta, and terminal URL behavior in `docs/llm/architecture/overview.md`.
Verification: `pnpm -C apps/desktop typecheck`, `pnpm -C apps/desktop test`, `cargo test --workspace`, `cargo build --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo fmt --check`; the user's app check from the design (cannot verify the real browser/Editor hand-off or TUI mouse-mode behavior automatically).
Verified at: c60dbd69d1fa280b904cfacfe0165e2b266893f0

## Tasks

### Task 1 — Opener plugin and dependencies

Files: `apps/desktop/src-tauri/Cargo.toml`, `apps/desktop/src-tauri/src/lib.rs::run`, `apps/desktop/src-tauri/capabilities/default.json`, `apps/desktop/package.json`, `pnpm-lock.yaml`, `Cargo.lock`
Interfaces: produces the `opener:allow-open-url` permission for the `main` window, the Rust plugin registration, and the importable `@tauri-apps/plugin-opener` (`openUrl`) package for Task 4.
Change: add `tauri-plugin-opener = "=2.7.2"` beside `tauri-plugin-dialog`, register it with `.plugin(tauri_plugin_opener::init())` next to the dialog plugin, and add one permission entry `{ "identifier": "opener:allow-open-url", "allow": [{ "url": "https://*" }, { "url": "http://*" }] }` to `default.json` beside the existing ones. Add `@tauri-apps/plugin-opener` at `2.7.2`.
Verify: `pnpm install` succeeds without a peer warning for the new package; `cargo build --workspace` and `cargo test --workspace` pass (tauri-build rejects an unknown permission identifier or malformed scope, so a clean build confirms the capability); `cargo clippy --workspace -- -D warnings`, `cargo fmt --check`, and `pnpm -C apps/desktop typecheck` pass.
Progress: done
Result: `pnpm install`, `cargo build --workspace`, `cargo test --workspace` (427 passed), `cargo clippy --workspace -- -D warnings`, `cargo fmt --check`, and `pnpm -C apps/desktop typecheck` all pass; `pnpm install` reported no peer warning.

### Task 2 — Shared modifier and file-path link logic

Files: `apps/desktop/src/lib/utils.ts::goToModifier` (moved), `apps/desktop/src/features/editor/CodeEditor.tsx` (imports the shared helper), `apps/desktop/src/features/terminal/terminal-links.ts::{findUrlRefs, findFileRefs, resolveFileRef}` (new), `apps/desktop/src/features/terminal/terminal-links.test.ts` (new)
Interfaces: produces `goToModifier(event: MouseEvent): boolean`; `findUrlRefs(text: string): { start: number; end: number; url: string }[]` (`start`/`end` are row string offsets); `findFileRefs(text: string): { start: number; end: number; path: string; line?: number; col?: number }[]` (`start`/`end` are string offsets of the whole token including `:line:col`); `resolveFileRef(ref, { checkoutPath, cwd, files }): string | null` returning a checkout-relative path present in `files` (`files: ReadonlySet<string>`, `cwd: string | undefined`).
Change: move `goToModifier` unchanged. Parse HTTP(S) URLs and file-reference candidates from one terminal row; URL candidates must not be returned as file refs. Implement the design's file candidate rule and three-step resolution (absolute under checkout root; relative to `cwd` made checkout-relative; relative to the checkout root; first hit wins; `.`/`..` normalised; result leaving the checkout dropped). Tests (named cases, input → expected): `https://example.com/a.ts` → one URL ref and no file ref; `see https://example.com/a?q=1,` → URL without trailing comma; `file:///tmp/a.ts` and `javascript:alert(1)` → no URL refs; `src/App.tsx:10` → path `src/App.tsx`, line 10; `a/b.ts:3:7` → line 3, col 7; `see (src/a.ts).` → `src/a.ts` without parens or trailing dot; `[x](docs/y.md)` → `docs/y.md`; `README.md` and `./x/y.rs` → refs; a bare word `hello` and `1.5` → none; absolute `/work/p1/src/a.ts` with checkout `/work/p1` → `src/a.ts`; absolute `/etc/passwd` → null; `../../outside.ts` from a cwd inside the checkout → null; relative `a.ts` with cwd `/work/p1/src` and `src/a.ts` in the list → `src/a.ts`; the same `a.ts` also at the root but the cwd match wins; a path absent from `files` → null; `cwd` undefined → falls back to the checkout root.
Verify: `pnpm -C apps/desktop exec vitest run src/features/terminal/terminal-links.test.ts src/features/editor/CodeEditor.test.tsx` passes, and `pnpm -C apps/desktop typecheck` is clean.
Progress: done
Result: `pnpm -C apps/desktop exec vitest run src/features/terminal/terminal-links.test.ts src/features/editor/CodeEditor.test.tsx` — 2 files, 32 tests passed; `pnpm -C apps/desktop typecheck` — clean. Added coverage for normalizing `.`/`..` in the session cwd before path lookup.

### Task 3 — Open-file request: store, Editor, Files

Files: `apps/desktop/src/stores/ui.ts::{editorOpenRequest, openEditorFile, clearEditorOpenRequest}`, `apps/desktop/src/stores/ui.test.tsx`, `apps/desktop/src/features/editor/EditorWorkspace.tsx::EditorWorkspace`, `apps/desktop/src/features/editor/EditorFiles.tsx::EditorFiles` (new props), `apps/desktop/src/features/editor/EditorWorkspace.test.tsx`
Interfaces: produces `editorOpenRequest: { workspaceId: string; path: string; line?: number; nonce: number } | null`, `openEditorFile(workspaceId, path, line?)` (sets the request with a fresh nonce, then `setActiveView('editor')`), `clearEditorOpenRequest()`; `EditorFiles` props `openRequest?: { path: string; line?: number; nonce: number } | null` and `onOpenHandled?: () => void`.
Change: `EditorWorkspace` reads the request; when `request.workspaceId === checkoutKey(projectId, checkoutPath)` and the Editor is `ready`, it sets the section to `files` and passes `openRequest` and `onOpenHandled` (= `clearEditorOpenRequest`) to `EditorFiles`; when it matches but the Editor cannot show the checkout (`unsupported` or `missingCheckout`), it clears the request; a non-matching request is cleared too; an unmount cleanup clears any request. `EditorFiles` runs one effect per nonce that calls the existing `selectPath(path, line)` and then `onOpenHandled()`.
Verify: `pnpm -C apps/desktop exec vitest run src/stores/ui.test.tsx src/features/editor` passes. Cases: `openEditorFile` sets the request and switches to the Editor view and a second call gets a new nonce; an Editor rendering `files` opens the requested path at the requested line (reveal asserted as in the existing line-navigation test) and the request is cleared; with the same file already open only the line reveal runs; with an unsaved draft the existing guard dialog appears and Cancel leaves the Editor on the draft; a request for another workspace opens nothing and is cleared; a request for an unsupported or missing checkout is cleared and the empty state shows; unmounting `EditorWorkspace` clears a pending request; a consumed request does not replay when the Editor remounts.
Progress: done
Result: `pnpm -C apps/desktop exec vitest run src/stores/ui.test.tsx src/features/editor` — 14 files, 229 tests passed; `pnpm -C apps/desktop typecheck` — clean. Covered one-shot consumption, workspace matching, unsupported/missing checkout, unmount, StrictMode remount behavior, line reveal, and both file-navigation and view-navigation draft cancellation.

### Task 4 — Terminal URL and file links

Files: `apps/desktop/src/features/terminal/TerminalController.ts::{TerminalController, TerminalLinks}`, `apps/desktop/src/features/terminal/TerminalView.tsx::TerminalView`, `apps/desktop/src/features/terminal/TerminalWorkspace.tsx::renderPane` (line 360), `apps/desktop/src/features/terminal/TerminalController.test.ts`, `apps/desktop/src/features/terminal/TerminalView.test.tsx`, `ARCHITECTURE.md`, `docs/llm/architecture/overview.md` (via `document-wiki`)
Interfaces: consumes Task 1 opener plugin, Task 2 `findUrlRefs`/`findFileRefs`/`resolveFileRef`/`goToModifier`, and Task 3 `openEditorFile`; produces the exported `TerminalLinks` type `{ openUrl(url: string): void; file?: { resolve(raw: string): Promise<string | null>; open(path: string, line?: number): void } }`, a third optional `TerminalController` constructor argument `links?: TerminalLinks`, and optional `TerminalView` props `workspaceId`, `projectId`, `checkoutPath`.
Change: in `mount`, when `links` is given, register one `term.registerLinkProvider` that reads the row text, combines `findUrlRefs` with file refs when `links.file` exists, resolves file refs asynchronously only when the row has file candidates, and returns links at the matching buffer ranges. URL activation returns unless `goToModifier(event)` and `new URL(uri).protocol` is `http:` or `https:`, then calls `links.openUrl(uri)`. File activation calls `links.file.open(path, line)` only when `goToModifier(event)`. Dispose the provider with the controller. `TerminalView` builds `links`: `openUrl` calls `@tauri-apps/plugin-opener` and handles its rejected promise; `file` only when `projectId` and `checkoutPath` are present, with `resolve` doing `queryClient.fetchQuery({ queryKey: queryKeys.gitFileList(projectId, checkoutPath), queryFn: () => gitViewService.fileList(projectId, checkoutPath), staleTime: 5000 })` (a rejection resolves to `null`) then `resolveFileRef` with the session's live `cwd` read from a ref updated from `useSession`, and `open` calling `useUi.getState().openEditorFile(workspaceId, path, line)`. `TerminalWorkspace` passes `workspaceId`, `project?.id`, `checkoutPath` to `TerminalView`. Update `ARCHITECTURE.md`'s Terminal workspaces section with URL/file activation and clarify that Markdown-rendered anchors remain inert in the Editor.
Verify: `pnpm -C apps/desktop exec vitest run src/features/terminal` passes, `pnpm -C apps/desktop typecheck` is clean, then the full `pnpm -C apps/desktop test` and `cargo test --workspace`. After code checks, merge the file-link delta into `docs/llm/architecture/editor.md`, update terminal URL behavior in `docs/llm/architecture/overview.md` with `document-wiki`, and verify the wiki. Cases: with no `links` no provider is registered; with `links` one provider handles both URL and file refs; URL activation ignores a click without the modifier, ignores `javascript:` and `file:` URIs, ignores malformed `https://` values without throwing, and calls `openUrl` for an `https://` URI with the modifier; a URL-only row does not call `file.resolve`; the file provider returns a link only for a resolved ref; file activation ignores a plain click and calls `open(path, line)` for a modified click; the provider is disposed on `dispose()`; a rejecting `openUrl` promise is handled; in `TerminalView`, no `projectId` means no `file`, a rejecting `fileList` yields no file link while URL links remain, `cwd` from the session reaches `resolveFileRef`, and a second hover within 5 s does not call `fileList` again.
Progress: done
Result: `pnpm -C apps/desktop exec vitest run src/features/terminal` — 4 files, 95 tests passed; `pnpm -C apps/desktop typecheck` — clean; `pnpm -C apps/desktop test` — 47 files, 548 tests passed; `cargo test --workspace` — 433 passed across 22 suites; `cargo build --workspace`, `cargo clippy --workspace -- -D warnings`, and `cargo fmt --check` pass. `pnpm install --frozen-lockfile`, wiki validation (7 Markdown files, 20 requirements), and `git diff --check` pass. No interactive Tauri/system-browser/TUI mouse smoke test was run.

## Files inspected, no change

- `apps/desktop/src/features/editor/go-to-definition.ts` — modifier and lookup pattern the Editor already uses.
- `apps/desktop/src/services/git-view.ts` and `apps/desktop/src-tauri/src/git_view.rs::file_list` — the file list includes tracked and untracked files and no ignored ones, so no Rust change is needed.
- `apps/desktop/src/queryClient.ts` — checkout-scoped `gitFileList` query key and shared query cache.
- `apps/desktop/src/App.tsx` — `EditorWorkspace` renders only in editor mode, which the unmount cleanup relies on.

## Approval Gate

Status: approved
Approved by user on 2026-11-09: implement the approved terminal URL and checkout-file link behavior in `design.md`, `tasks.md`, and `delta.md`.

## Decision Log

### D1 — Use xterm's built-in link provider

Question: How should the terminal detect HTTP(S) URLs when no `@xterm/addon-web-links` release declares a peer range compatible with `@xterm/xterm` 6.0.0?
Decision: Keep xterm 6.0.0 and use its built-in `registerLinkProvider` for both URL and checkout-file links; do not add `@xterm/addon-web-links`.
Impact: The design and Tasks 1, 2, and 4 now use one terminal link provider and a shared URL/file scanner; the opener plugin remains the only new frontend/Rust dependency pair.
Confirmed by user: 2026-11-09
