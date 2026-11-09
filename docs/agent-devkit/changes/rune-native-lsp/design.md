# Native Rune language servers

## Goal

Give the native Rune editor real language-server **hover and go-to
definition** for Rust, Go, Python, JavaScript/TypeScript and HTML/CSS, plus
user-configured servers for other languages, wherever each server's
advertised capabilities allow. One stdio language-server process per
(checkout, server), owned off the UI thread, preserving the editor's
checkout-identity, dirty-draft and project-boundary protections. Feature
availability is per-server, not universal: e.g. the HTML server may offer
fewer capabilities than rust-analyzer. This is a separate subsystem: the
in-flight `rune-native-editor` change explicitly lists LSP as out of scope,
and the desktop (CodeMirror) editor stays unchanged.

**Scope decision D1 (2026-11-09, user instruction, supersedes the earlier
joint approval scope):** deliver hover + go-to definition first for speed;
diagnostics rendering/detail and all completion/autocomplete (triggers,
menu, `additionalTextEdits`, snippet handling) are **deferred** — the
transport, document sync, guards, settings and Servers UI built now must
not preclude them later.

## Scope

In scope:

- `crates/app-core/src/lsp.rs`: a minimal LSP stdio client — `Content-Length`
  framed JSON-RPC 2.0 over child stdio, request/response routing, server→client
  request handling, and an `LspHub` that owns one connection per
  `(checkout_key, server_id)` with lazy spawn, caps, bounded crash retry and
  clean shutdown. Mirrors `acp.rs` process-group/kill semantics; no generic
  plugin registry.
- `apps/rune/src/editor/lsp*`: per-document session state and sync
  (didOpen / didChange / didSave / didClose as the server advertises them),
  UTF-16 position conversion, stale-reply guards, hover card, LSP-backed
  definition with heuristic fallback, and a "Language Servers" editor
  section for status, per-server feature summary, restart, enable/disable
  and command override.
- Six built-in presets covering seven languages; custom argv/extension entries
  merged through the existing settings store under one JSON key
  (`lsp.servers`). No schema change, no shell expansion, no auto-install.
- Servers are user-configured local commands running with normal OS access:
  they may read toolchains and project files and use network per their own
  behavior (e.g. rust-analyzer invokes cargo) and are not sandboxed. The app
  itself performs no download or install, and only its own navigation reads
  are constrained to the validated checkout.

**Deferred (D1):** diagnostics storage/rendering/caret detail and the entire
completion feature (triggers, menu, `additionalTextEdits`, snippet
rejection). The `publishDiagnostics` capability stays advertised-aware in
the client caps and server→client notifications are still consumed by the
transport, but nothing is stored or rendered; completion is never
requested. Both return in a follow-up change.

Out of scope: formatting, rename, code actions, references, signature help,
`workspace/applyEdit`, `workspace/executeCommand`, completion item
`resolve`, inlay hints, multi-root workspaces, auto-downloading servers,
Tauri/desktop integration, Vim, diagnostics UI, completion UI.

Known limitation (prominent): **hover is caret-anchored, not
mouse-anchored.** Rune has no pointer→byte hit test, so there is no
hover-on-mouse-rest; the hover card follows the text caret.

## Behavior

### Servers and configuration

- The editor owns an extension→(server, languageId) map seeded by presets and
  extended by `lsp.servers` settings JSON
  (`{servers:[{id, argv?, disabled?, languages?}]}`; same-id entries replace
  fields, unknown ids append). It is independent of `syntax::language_for_path`
  — custom languages work without a syntax scanner.
- Presets:
  - `rust` → `rust-analyzer` (`rs` → `rust`)
  - `go` → `gopls` (`go` → `go`)
  - `python` → `pyright-langserver --stdio` (`py|pyi|py3` → `python`)
  - `typescript` → `typescript-language-server --stdio`
    (`js|mjs|cjs`→`javascript`, `jsx`→`javascriptreact`,
    `ts|mts|cts`→`typescript`, `tsx`→`typescriptreact`)
  - `html` → `vscode-html-language-server --stdio` (`html|htm` → `html`)
  - `css` → `vscode-css-language-server --stdio` (`css` → `css`)
- Documents over the 1 MiB editable cap, binary/non-UTF-8 and read-only docs
  never open LSP documents; extensions matching no enabled server get no
  session.
- Config validation: `id` `[a-z0-9-]{1,32}`, non-empty `argv`, `languages` map
  of extension→languageId strings, ≤16 custom entries, JSON ≤64 KiB. Command
  strings entered in the UI are split with `shlex` — never through a shell.
- Missing/broken servers are status, not failure: spawn ENOENT or a dead
  initialize surfaces `Off`/`Error` plus install advice (e.g. rust-analyzer:
  "run `rustup component add rust-analyzer`"; the rustup shim's stderr is
  captured for this). No retries for missing binaries; manual Restart and
  per-doc reopen re-attempt.

### Lifecycle and capabilities

- Exactly one async owner: `apps/rune::editor::lsp::Lsp` owns the single
  dedicated thread running a current-thread tokio runtime; app-core
  `LspHub` is constructed with that runtime's `Handle`, holds connection
  state, and owns **no thread of its own** (no double runtime). Connections
  are keyed `(checkout_key, server_id)`.
  Spawn is lazy on first eligible `didOpen`; at most `MAX_LSP_SERVERS = 8`
  connections — evict the oldest connection with no open documents (sends
  didClose + shutdown first).
- `initialize` (60 s timeout) sends client capabilities under
  `textDocument.*` per LSP 3.17 / Microsoft `protocol.ts`: `synchronization.
  didSave = true` (boolean — `includeText` exists only on the server's
  `SaveOptions`), `completion` (`snippetSupport: false`, no resolve),
  `hover` (`contentFormat: ["plaintext"]`), `definition` (`linkSupport:
  false`), `publishDiagnostics`. Position encodings: `["utf-16"]`. All
  message models use the `lsp-types` crate types, serialized through the
  Value-based transport.
- The `initialize` result's `capabilities` are parsed into per-connection
  feature flags: `completionProvider` → completion, `hoverProvider` → hover,
  `definitionProvider` → definition, `textDocumentSync` → document sync.
  UI→server requests are only sent for advertised features; the Servers
  section shows each server's live feature summary.
- `textDocumentSync` may be a `TextDocumentSyncKind` number or
  `TextDocumentSyncOptions`; both forms are honored (see Document
  synchronization). A server advertising no sync support gets no document
  messages; requests that still need a document may fail server-side and are
  surfaced as errors, not retried blindly.
- Unexpected exit/EOF marks the connection dead, fails its pending requests
  and surfaces a notice once. Respawn on next need with backoff 1 s/2 s/4 s,
  max 3 consecutive crashes → `Error` until manual Restart or a 60 s cooldown.
  On respawn the hub re-sends `didOpen` for documents the editor still has
  open on that `(checkout, server)`.
- `shutdown` request (5 s) → `exit` → join; `kill_on_drop` +
  `process_group(0)` + `killpg` cleanup identical to ACP, so `npx`-style
  wrappers cannot orphan grandchildren. App quit calls `Lsp::shutdown()` next
  to `Acp::shutdown` in `on_quit`.

### Document synchronization

All sync messages respect the server's advertised `textDocumentSync`
(LSP 3.17 `TextDocumentSyncKind | TextDocumentSyncOptions`, per Microsoft
`protocol.ts`):

- `textDocumentSync` absent or kind `None` (0) → no document messages at
  all; document-dependent requests then fail server-side and surface as
  errors.
- Bare `TextDocumentSyncKind` number N > 0 (legacy form: `Full` = 1,
  `Incremental` = 2) → `change = N` **and** openClose implied true
  (didOpen/didClose sent); the number form expresses no save → no didSave.
- `TextDocumentSyncOptions` → each field literal, omissions mean
  unsupported: `openClose: true` → didOpen/didClose (`openClose` absent or
  false → neither is sent); `change` → below; `save` declared in any form —
  `true`, `{}`, or `{includeText: false}` → didSave **without** `text`;
  `save: {includeText: true}` → didSave **with** `text`; only `save`
  absent/false sends nothing.
- `change`: `None`/absent → no change events; `Full` (1) → whole text;
  `Incremental` (2) → one `TextDocumentContentChangeEvent` computed as the
  common prefix/suffix diff between last-sent text and current text
  (char-boundary safe, coalesced by a ~120 ms debounce like highlighting).
- Subject to the matrix above: `didOpen` fires from `install_doc` (and on
  respawn); `didClose` fires when the open doc is replaced, the checkout
  changes, the doc becomes ineligible, or the connection is
  evicted/shut down. All sync ops for a doc ride one ordered per-doc queue
  flushed in `lsp::poll`, preserving order.
- Wire `version` is a per-doc monotonic counter (1 at didOpen, +1 per applied
  change), independent of `CodeState.revision` which stays the UI staleness
  token.

### Positions and URIs

- Pure helpers convert `TextEdit` byte offsets ↔ LSP `(line, utf-16 column)`
  and back, and `file://` URIs ↔ checkout-relative paths (`url` crate). All
  are unit-tested on ASCII, emoji/CJK (surrogate pairs), combining marks, CRLF
  and multibyte text.

### Stale-reply protection

- Every UI→server request records `(checkout_key, doc path, snapshot)`:
  definition snapshots the clicked token's byte range + doc revision;
  completion/hover snapshot caret byte + revision. A reply applies only while
  all still match the live `EditorState`; otherwise it is dropped silently.
- `publishDiagnostics` notifications are still routed to the editor by the
  transport (they carry `uri` + optional `version`), but per D1 nothing is
  stored, dimmed or rendered yet — the deferred diagnostics change will add
  the session+URI+version guards described in the archived plan.
- Definition targets are validated before navigation: the URI must decode to
  a path inside the active checkout (canonicalized, `..`/symlink escape
  rejected). Result policy is unambiguous: evaluate returned locations in
  order and take the **first whose URI is inside the checkout**; if every
  result is outside → notice "definition is outside the checkout" and no
  navigation; empty result, error, timeout or dead server → fall back to the
  heuristic pipeline.

### Diagnostics — deferred (D1)

The full diagnostics design (500-item/500-char caps with visible notice,
severity→span mapping preserving syntax color, dim-until-refresh,
caret-in-range detail in the hover card) is deferred to the follow-up
change; this delivery stores and renders nothing.

### Completion — deferred (D1)

The full completion design (Ctrl+Space + ~250 ms auto-trigger, one in-flight
request per doc with 5 s local timeout and `$/cancelRequest`, dismissal
latch, 100-item cap disclosure, `isIncomplete` re-request,
`CompletionTextEdit`/`InsertReplaceEdit`/prefix-replacement edit shapes,
same-doc `additionalTextEdits` applied atomically in one `undo_group` with
caret adjustment, snippet-syntax rejection, caret-anchored `PopupList`
mounted only after measured `cx.caret(FIELD_KEY)` coordinates, Up/Down/
Ctrl+N/P/Enter/Tab/Esc keyboard handling) is deferred to the follow-up
change; this delivery never sends `textDocument/completion`.

### Hover

- Debounced ~400 ms after caret/revision settles → `textDocument/hover` at
  the caret position (only when `hoverProvider` was advertised), with a
  **5 s local timeout** freeing the in-flight slot. Response
  (`MarkupContent`/`MarkedString`, plaintext) capped at 4000 chars / 20
  lines. Applies only while caret+revision still match; the same dismissal
  latch as completion applies after Esc.
- The anchored card (same caret marker, `anchored_fit`, prefers below)
  renders hover text. Hidden on doc close/checkout switch.

### Definition

- On the existing Cmd/Ctrl+click go-to path **and on explicit `F12`** at the
  caret (`navigation::start_go_to`, keyboard-accessible alongside click): if
  the doc has a live server that advertised `definitionProvider`, send
  `textDocument/definition` at the token's start (UTF-16) with a 15 s request
  timeout; a newer invocation supersedes and `$/cancelRequest`s the older
  id.
  Result policy per Stale-reply protection: first inside-checkout location →
  `navigate(PendingNav::OpenDoc { path, line })` — the existing dirty-draft
  guard, same-file reveal and path validation apply unchanged; all-outside →
  notice; empty/error/timeout/dead/unsupported → heuristic
  (import/tsconfig/content-search) pipeline untouched. Multiple inside
  results: first wins in v1.

## Edge cases

- rust-analyzer shim / missing component: initialize never completes or the
  process exits; captured stderr yields the "component not installed" advice.
- Crash loops: backoff + 3-strike cap; pending requests fail with a notice;
  documents re-sync on respawn.
- Stale everything: checkout switch, doc replace, rapid typing, caret moves —
  every reply is identity-checked before it can touch state; diagnostics dim
  on edit until refreshed, with the unversioned-publish lag disclosed above.
- Server→client requests: `workspace/configuration` → nulls,
  `client/registerCapability`/`unregisterCapability` → null,
  `workspace/workDoneProgress/create` → null, `window/showMessage` → notice,
  `window/logMessage`/`telemetry/event`/`$/cancelRequest` → ignored,
  `workspace/applyEdit` and anything else → `-32601`. We never advertise
  `workspace.applyEdit` or `executeCommand`.
- Oversized/binary/truncated docs: no didOpen; LSP state cleared.
- Checkout switch while requests fly: replies carry `checkout_key` and are
  dropped; per-checkout servers stay alive (bounded) so switching back is
  warm.
- macOS Ctrl+Space may be captured by the OS input switcher — automatic
  trigger still covers completion.
- pyright without a configured environment still works for syntax-level
  features; import resolution quality is environment-dependent (limitation).
- Servers with partial capabilities (e.g. HTML/CSS servers may lack
  definition or completion) simply show fewer features; nothing is promised
  universally.

## Interfaces

- `crates/app-core/src/lsp.rs` (new): `LspSpec { id, argv, disabled,
  languages }`, `LspStatus` (incl. parsed feature flags), `LspHub::
  new(handle)/status/send_request/notify/shutdown`, `file_uri_to_path`/
  `path_to_file_uri`, `parse_command` (shlex). Internals: `Connection` (spawn,
  Content-Length reader/writer tasks, pending map, `mark_dead`,
  `kill_tree`), `Routes`, initialize-capability parsing. Wire framing and
  raw JSON-RPC stay `serde_json::Value`; protocol message shapes use
  `lsp-types` models (`InitializeParams`, `CompletionParams`,
  `PublishDiagnosticsParams`, `Diagnostic`, `CompletionItem`, `TextEdit`,
  `Location`/`LocationLink`, …) so capability and completion/diagnostic
  fields cannot drift from the official schema.
- `apps/rune/src/editor/lsp.rs` (+ `lsp/` submodules as size requires):
  `Lsp` — the single runtime-thread owner wrapping `LspHub` — presets +
  settings merge + extension map, `LspDocSync` state on `OpenDoc`,
  poll-driven sync queue honoring advertised sync kinds, position/diff
  helpers, request/response state for hover/definition.
- Hooks (existing files): `backend.rs::install_doc` → didOpen;
  `backend/apply.rs` Saved arm → didSave; doc replace/`navigation.rs
  select_checkout`/quit → didClose/shutdown; `code.rs::field` → caret
  marker for the hover card; `navigation.rs::start_go_to` → LSP definition
  attempt before heuristic (plus `F12` in `field_key`); `state.rs` →
  `Section::Servers`, `EditorState.lsp`; `main.rs::view` →
  `cx.caret(FIELD_KEY)` + root overlays, `poll` → `lsp::poll`, `on_quit` →
  `lsp.shutdown`.
- Settings: key `lsp.servers` via existing `Req::GetSetting`/`SetSetting`.
- Dependencies: `lsp-types = "0.97"` (latest crates.io release 0.97.0) in
  `crates/app-core` **and** `apps/rune` — justified by the many-message
  protocol schema (initialize/capabilities, sync notifications,
  completion/hover/definition params and responses, diagnostics) versus
  error-prone hand-shaped `Value` structs; the JSON-RPC framing itself
  stays `Value`-based. `url = "2"` in `crates/app-core` only (already in
  Cargo.lock transitively). `Cargo.lock` updates accordingly.
- Tests: `crates/app-core/tests/fixtures/fake_lsp_server.py` (Content-Length
  JSON-RPC fixture, sibling of `fake_acp_agent.py`); unit tests beside each
  module; headless `Runtime` tests for UI state.

## Verification

- `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`,
  `cargo test --workspace`, `cargo build --workspace` — all must pass; desktop
  untouched.
- New unit tests: UTF-16/byte conversion (ASCII, surrogate pairs, CRLF),
  prefix/suffix diff, URI↔path round-trip and boundary rejection, settings
  merge/validation, stale-reply drops (revision/checkout/caret/version),
  sync-kind matrix (absent/None/number Full|Incremental × options
  openClose/change/save × `SaveOptions.includeText` — didSave text only
  when `includeText: true`), 5 s hover and 15 s definition local timeouts
  freeing the in-flight slot, definition first-inside/all-outside/empty
  policies, hover debounce/stale/hide, framing edge cases (chunked
  headers, large payloads), fake-server lifecycle (init, routing,
  crash→respawn, shutdown/killpg), capability-flag gating per feature.
  Deferred-scope tests (diagnostics caps/dimming, completion edit shapes,
  undo-group apply) move to the follow-up change.
- Manual smoke **all seven languages** — each preset server is installed
  into an isolated cache from its official registry using the already-
  present Go/Node/Python toolchains (e.g. `go install`/`npm --prefix`/
  `pip --target` into a temp dir passed via command override; rust-analyzer
  via a downloaded component binary or rustup shim advice): open doc →
  hover card shows, Cmd/Ctrl+click and F12 jump; partial-capability
  servers (HTML/CSS) show honest feature subsets. Missing servers show
  install advice; the app never auto-installs and QA never requires a
  global/user install. Diagnostics/completion smoke is deferred with D1.
- Post-implementation wiki coverage for the new subsystem via `document-wiki`
  (new domain — no delta requirement).

## Related context

- [Desktop Editor](../../../llm/architecture/editor.md) — the Tauri/CodeMirror
  editor this port mirrors; its go-to-definition is heuristic/textual, which
  the LSP path augments rather than replaces.
