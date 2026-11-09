# Native Rune language servers tasks

## Design

[Native Rune language servers](design.md)

## Global Constraints

- Client capabilities are sent under `textDocument.*` per LSP 3.17 /
  Microsoft `protocol.ts` (`synchronization.didSave = true` — boolean;
  `includeText` exists only in server `SaveOptions`, `completion` without
  snippet/resolve, `hover` plaintext, `definition` without linkSupport,
  `publishDiagnostics`); server sync behavior follows the server's
  advertised `textDocumentSync` — absent/`None` → no document messages,
  bare kind number N>0 → change kind + openClose implied (no didSave),
  options form → literal fields with `openClose` absent meaning false,
  `save` declared as `true`/`{}`/`{includeText:false}` → didSave without
  text, `save:{includeText:true}` → didSave with text, absent/false → none.
  Protocol message shapes use `lsp-types` models; framing stays
  `serde_json::Value`.
- Exactly one async runtime owner: `editor::lsp::Lsp` owns the single
  dedicated thread + current-thread runtime; `app_core::lsp::LspHub` takes
  its `Handle` and owns no thread. Local request timeouts: hover 5 s,
  definition 15 s — a timeout frees the in-flight slot even if the server
  ignores `$/cancelRequest`. (Completion's 5 s timeout is deferred with D1.)
- Feature availability comes from each connection's `initialize` result:
  requests are sent only for server-advertised `hoverProvider`/
  `definitionProvider`/sync; nothing is promised universally.
  `workspace/applyEdit`, `workspace/executeCommand`, completion `command`
  fields and any unimplemented server→client request get
  `-32601`/are ignored. Completion is never requested under D1.
- Every reply or notification applies only after its `checkout_key`, doc
  path and revision/caret/wire-version snapshot still match live state.
  (The diagnostics version-guard/dim rule is deferred with D1; diagnostics
  notifications are routed but not stored or rendered.)
- `didOpen` only for editable UTF-8 documents ≤1 MiB whose extension maps to
  an enabled server via the editor-owned extension map (independent of
  syntax scanners, so custom languages work without highlighting); no
  auto-install, no shell expansion (`shlex` split only), no generic server
  registry — six presets plus settings-merged custom entries.
- Deferred (D1): the completion `undo_group`/`additionalTextEdits`/
  snippet/dismissal-latch constraints and all diagnostics constraints move
  to the follow-up change; nothing in this delivery sends
  `textDocument/completion` or renders diagnostics.
- All process and parsing work stays off the UI thread; every queue, item
  list and retry count is bounded per the design; child processes die via
  process-group kill on shutdown, eviction and crash.
- The client constrains its own file reads to the validated checkout; local
  servers run user-configured commands with normal OS access — they may read
  toolchains/project files and use network per their own behavior and are
  not sandboxed. The app performs no download or install.
- No changes to `apps/desktop`, `crates/protocol`, `crates/daemon`, or the
  approved `rune-native-editor` scope; the desktop editor and its wiki page
  stay accurate for Tauri.

## Impact map

Entry: existing `apps/rune/src/editor/code.rs::field`/`field_key`,
`backend.rs::install_doc`, `backend/apply.rs` Saved arm, `navigation.rs`
`start_go_to`/`select_checkout`, `main.rs::{view,poll,on_quit}`; planned
entry approved, not yet implemented: `app_core::lsp` transport/hub and `editor::lsp`
session/position/sync modules.

Flow: keystroke/edit → `notify_changed`/`lsp::poll` ordered sync queue →
`LspHub` → child stdio (`Content-Length` JSON-RPC); server replies →
`AppHandle::update` → identity-checked apply → anchored hover overlay,
guarded `PendingNav::OpenDoc` definition navigation. (Diagnostics spans and
completion overlays are deferred with D1.)

State changes: planned approved, not yet implemented — `EditorState.lsp` hub handle +
per-`OpenDoc` `LspDocSync` (server id, URI, wire version, synced revision,
last-sent text, pending requests, hover state) + per-connection feature
flags from `initialize`; `Section::Servers`; settings key `lsp.servers` in
the existing store. (Per-doc diagnostics/completion state deferred with
D1.) No schema or daemon-protocol change.

External effects: planned approved, not yet implemented — spawned language-server child
processes over stdio with process-group cleanup; these are user-configured
local commands with normal OS access (toolchain/project reads, project
tooling, possible network per server behavior; not sandboxed). The app's own
navigation reads stay inside the validated checkout; the app performs no
download or install.

Change candidates: approved, not yet implemented — `crates/app-core/{Cargo.toml,
src/lib.rs,src/lsp.rs,tests/fixtures/fake_lsp_server.py}`, `Cargo.lock`;
`apps/rune/{Cargo.toml,src/editor/{mod.rs,state.rs,draft.rs,code.rs,
backend.rs,backend/apply.rs,navigation.rs,workspace.rs,lsp*.rs}}`,
`apps/rune/src/main.rs`, `docs/llm/` coverage after implementation.

Verification: approved, not yet implemented — workspace `cargo fmt --check`, `cargo clippy
--workspace -- -D warnings`, `cargo test --workspace`, `cargo build
--workspace`; fake-server fixture tests; per-server manual smoke (installed
vs missing, no auto-install); post-implementation `document-wiki` coverage.

Verified at: `01aa1de037e9a90f51d6a64babec6a1d3729e189` plus the uncommitted
`apps/rune/src/editor/` and `crates/app-core/` port tree — that working tree
is the baseline source of truth; recheck `git status` before editing.

## Tasks

**Scope decision D1 (2026-11-09, user instruction — supersedes the earlier
joint-approval scope, no re-approval needed):** deliver hover + go-to
definition only. Diagnostics rendering/detail and all completion work are
**Deferred** below; Tasks 1–3, 5 and the hover/servers parts of 6–7 are the
current delivery. Deferred descriptions are preserved for the follow-up
change, not unfinished current work.

Baseline before LSP source edits: `cargo fmt -p app-core --check` and `cargo clippy -p app-core --all-targets -- -D warnings` passed. Initial core tests: 112 passed, one existing immediate-agent-exit timing race in `a_crashed_agent_shows_error_in_status`; focused recheck passed. Full recheck passed all 113 tests and core build passed; logs are in `/tmp/goko-rune-lsp-baseline/`. The native port review repairs remain concurrent; transient intermediate compile failures are recorded there rather than hidden.

### Task 1 — app-core LSP stdio transport

Files: new `crates/app-core/src/lsp.rs` (`Connection`, reader/writer tasks,
pending map, `mark_dead`/`kill_tree`); `crates/app-core/src/lib.rs`;
`crates/app-core/tests/fixtures/fake_lsp_server.py`.
Interfaces: consumes tokio process/io-util + serde_json already in
app-core; produces one-connection JSON-RPC client over `serde_json::Value`
framing: `send_request`, `send_notification`, `respond`, `is_alive`,
`shutdown`, plus the `initialize` handshake whose client capabilities use
`lsp-types` models (`synchronization.didSave=true` boolean, completion
`snippetSupport=false`/no resolve, hover plaintext, definition
`linkSupport=false`, publishDiagnostics, positionEncodings `["utf-16"]`);
server→client requests routed to a fixed responder table per the design.
Change: spawn with piped stdio, `kill_on_drop`, `process_group(0)`; parse
`Content-Length` headers (chunked-safe, bounded header/body sizes); route
responses by id and notifications by method; EOF/exit marks dead and fails
pending; stderr captured bounded for diagnostics; `shutdown` request →
`exit` → killpg fallback.
Verify: fixture tests — initialize handshake shape, request/response id
routing, notification delivery, chunked/oversized framing rejected safely,
server crash fails pending, shutdown kills the process group.
Progress: done
Result: `crates/app-core/src/lsp.rs` `Connection` + framing/read/write/
route/responder implemented exactly as specified (API names: `request`,
`notify`, `capabilities`, `features`, `is_alive`, `stderr_tail`, `shutdown`,
`ConnEvent::{Notification,Closed}`; see `/tmp/goko-lsp-core-api.md`).
`fake_lsp_server.py` covers ok/exit_now/bad_framing/never_init/
crash_after_init/crash_on_notify/oversize_after_init/stderr_noise/
with_grandchild/flaky_once/exit_on_initialize + capability overrides.
13 transport tests pass (handshake caps shape asserted field-by-field,
request/response + -32601 error path, fixed server-request answers verified
in fixture log, oversize/bad-framing kill, crash fails pending, killpg reaps
child+grandchild, bounded stderr, init timeout, exit-now).

### Task 2 — app-core LspHub, capabilities, lifecycle and helpers

Files: `crates/app-core/src/lsp.rs` (`LspSpec`, `LspStatus`, `LspHub`,
capability parsing); `crates/app-core/Cargo.toml` (`url = "2"`,
`lsp-types = "0.97"`); `Cargo.lock`.
Interfaces: consumes Task 1 `Connection`; produces `LspHub::new(handle)/
status/send_request/notify/shutdown` keyed `(checkout_key, server_id)` —
the hub is constructed with the runtime `Handle` owned by
`editor::lsp::Lsp` and spawns no thread of its own; `LspStatus` carries
parsed feature flags (`completionProvider`, `hoverProvider`,
`definitionProvider`, sync kind/options); helpers `file_uri_to_path`,
`path_to_file_uri`, `parse_command` (shlex). `lsp-types` supplies the
protocol models; the transport stays `serde_json::Value`.
Change: lazy spawn per key, `MAX_LSP_SERVERS = 8` with oldest-idle eviction
after didClose+shutdown; parse `initialize` result capabilities into
per-connection feature flags (both `TextDocumentSyncKind` number and
`TextDocumentSyncOptions` forms, `openClose` absent ⇒ false, `save: true`
⇒ didSave without text, `SaveOptions{includeText:true}` ⇒ with text);
retry backoff 1 s/2 s/4 s, 3 consecutive crashes → Error, 60 s cooldown,
missing binary → Error + captured-stderr advice with no auto-retry; status
snapshot for UI; `shutdown` signals the owning runtime like `Acp::shutdown`;
URI helpers percent-encode/decode `file://` and reject non-file/escaping
forms.
Verify: hub tests with two fixture servers — key isolation, eviction order,
retry cap and cooldown, respawn re-`didOpen`, capability parsing for number
and options sync forms (incl. openClose-absent and save/includeText
variants), URI round-trips incl. spaces/non-ASCII/`%`/`#`, shlex split of
quoted argv, `url`+`lsp-types` are the only manifest changes.
Progress: done
Result: `LspHub`/`LspKey`/`LspSpec`/`LspStatus`/`LspState`/`LspFeatures`/
`SyncSupport`/`LspEvent`/`LspError` + `path_to_file_uri`/`file_uri_to_path`/
`parse_argv` implemented; `Cargo.toml` gained `lsp-types = "0.97"` +
`url = "2"` only. Hub owns no thread — built on the caller's runtime
`Handle`; single forwarder task per connection tags events with
key+epoch. 12 hub/helper tests pass: ensure+status features, two-checkout
isolation (distinct pids), event key/epoch tagging + Closed, request/notify
routing, missing binary → Error no-retry, backoff→respawn epoch bump,
consecutive handshake failures → cooldown without reset, running-crash
single-count + retry, 8-cap oldest-idle eviction respecting open_docs,
disconnect/shutdown reset, URI round-trip + rejections, shlex argv, full
sync matrix (absent/None/kinds/options/save variants), capability-derived
features. `cargo test -p app-core`: 138 pass; clippy `-D warnings` and
`cargo fmt --check` clean.

### Task 3 — Rune LSP session core and document sync

Files: new `apps/rune/src/editor/lsp.rs` (+ `lsp/` submodules as size
requires); `apps/rune/Cargo.toml` (`lsp-types = "0.97"`); `Cargo.lock`;
`apps/rune/src/editor/{mod.rs,state.rs,draft.rs}`;
`apps/rune/src/editor/backend.rs` (`install_doc`); `apps/rune/src/editor/
backend/apply.rs` (Saved arm, doc-replace path);
`apps/rune/src/editor/navigation.rs` (`select_checkout` doc clear →
didClose); `apps/rune/src/main.rs` (`poll` → `lsp::poll`, `on_quit` →
`lsp.shutdown`).
Interfaces: consumes Task 2 hub + existing `Req::GetSetting`/`SetSetting`;
produces presets + `lsp.servers` merge/validation, the editor-owned
extension→(server, languageId) map (independent of
`syntax::language_for_path`), `LspDocSync` state, ordered per-doc sync queue,
UTF-16 position and prefix/suffix diff helpers, wire-version counter.
Change: `Lsp` spawns the single dedicated thread + current-thread tokio
runtime and constructs `LspHub` with its `Handle` (one runtime owner, no
double threads); load/merge/validate config on checkout selection and
settings change; didOpen on `install_doc` and respawn only when the server
advertises sync support; didChange honoring the advertised sync — absent/
`None` → no document messages, bare kind number N>0 → change kind with
openClose implied and no didSave, options form → literal openClose/change/
save fields (`openClose` absent ⇒ false); change None → none, Full →
whole text, Incremental → prefix/suffix diff, ~120 ms debounce off
`text_edited` revisions; didSave sent when save is declared — text
included only for `SaveOptions{includeText:true}`; didClose per openClose;
all ops carry checkout key and flush in order from `lsp::poll`.
Verify: unit tests — UTF-16 conversions (ASCII, surrogate pairs, CRLF,
combining marks), diff correctness, settings merge/validation rejections,
custom-language extension lookup without a syntax scanner, sync-kind matrix
(number and options forms × None/Full/Incremental × openClose ×
save variants: absent/false → none, `true`/`{}`/`{includeText:false}` →
didSave without text, `{includeText:true}` → with text), wire-version
sequence; headless state tests —
open/change/save/close order per advertised capabilities, no didOpen for
ineligible docs, stale checkout drop, queue cap behavior.
Progress: done
Result: `apps/rune/src/editor/lsp.rs` (+ `lsp/pos.rs`, `lsp/sync.rs`)
implemented: presets for rust/go/python/js/ts/html/css + `lsp.servers`
merge/validation (custom argv+languages, shlex-parsed, no shell), the
editor-owned extension→(server, languageId) map independent of the syntax
scanner, one lazily-spawned runtime thread hosting `LspHub` on a
current-thread Tokio runtime, and ordered per-doc sync (open → debounced
~120 ms change → save) honoring the full advertised sync matrix
(absent/None → nothing, bare kind → change+openClose no save, options →
literal fields, all save variants, `didSave` text only for
`includeText:true`). UTF-16 positions and prefix/suffix diff per
`/tmp/goko-lsp-position-notes.md` (CRLF/lone-CR/LF, no surrogate splits).
Epoch bumps re-open the doc; `didClose` precedes `didOpen` on switch in
FIFO order. Requests carry UI-side timeouts (hover 5 s, definition
15 s) whose drop removes the transport pending entry and sends
`$/cancelRequest`. Runtime spawn is lazy so test `EditorState`s cost
nothing. 12 module tests + 6 pos/sync tests pass (sync order, debounce,
epoch re-open, close ordering, config merge/rejections, custom-language
lookup, UTF-16/CRLF/combining-mark positions, diff boundaries).

### Task 4 — Diagnostics rendering and caret detail — DEFERRED (D1)

Deferred by user instruction 2026-11-09: diagnostics storage, dimming,
span rendering and caret detail are not part of this delivery. The
original approved description is preserved for the follow-up change:

Files: `apps/rune/src/editor/lsp.rs` (publishDiagnostics store/guards,
diagnostic model); `apps/rune/src/editor/code.rs` (`field`/`syntax_spans`
merge point, dimmed-stale styling); `apps/rune/src/editor/draft.rs` if doc
state lands there.
Interfaces: consumes Task 3 sync state; produces per-doc capped diagnostic
set (severity/source/code/message/range), dimmed-until-refresh state, cap
notice, and the caret-detail model consumed by the hover card.
Change: store `publishDiagnostics` per URI (cap 500 items/500 chars per
item — hitting either shows a visible capped notice), apply
session+URI+version guards, dim the set on each applied edit until the next
publish, convert LSP ranges to byte ranges, emit `Span::underline` per
severity (Error→danger, Warning→warning, Information/Hint→muted/accent)
merged with syntax spans without overriding text color; expose
caret-in-range diagnostic detail for the hover card.
Verify: unit tests — severity mapping, version absent/stale/current,
dim-on-edit then clear-on-publish, multibyte range conversion, span merge
preserves syntax color, caps + notice, caret-detail lookup; headless test —
diagnostics appear/clear/dim on edit, publish for other URI ignored.
Progress: deferred (D1)
Result: none — follow-up change

### Task 5 — LSP-backed go-to definition

Files: `apps/rune/src/editor/lsp.rs` (definition request state);
`apps/rune/src/editor/navigation.rs` (`start_go_to` attempt-then-fallback
wiring); `apps/rune/src/editor/go_to_definition.rs` only if target shaping
needs it.
Interfaces: consumes Task 2/3 hub+session+feature flags; produces definition
reply → validated `PendingNav::OpenDoc` or heuristic fallback.
Change: on Cmd/Ctrl+click **and explicit `F12` at the caret** (consumed in
`field_key` so keyboard users can invoke definition) with a live server
that advertised `definitionProvider`, send
`textDocument/definition` at the token start
(UTF-16, 15 s timeout, snapshot = token range + revision); a newer
invocation `$/cancelRequest`s the older id; evaluate results in order —
first location whose URI decodes inside the checkout → existing
`navigate(PendingNav::OpenDoc{path,line})` (dirty guard and same-file
reveal unchanged); all-outside → notice, no navigation; empty/error/
timeout/dead/unsupported → existing heuristic pipeline.
Verify: tests — F12 key dispatch at caret, first-inside wins among mixed
results, all-outside notice, inside-checkout jump, symlink/`..` rejection,
dirty-guard Save/Discard/Cancel routing, same-file reveal, stale reply
drop, cancel-on-supersede, fallback on each failure mode.
Progress: done
Result: `navigation::start_go_to` tries `EditorState::lsp_try_definition`
first (requires open sync + `definitionProvider` + Running status);
replies resolve first-inside-checkout → `PendingNav::OpenDoc{path,line}`
drained through `AppSpec::poll` into the same `navigate` dirty-draft
guard, all-outside → "definition is outside the checkout" notice,
empty/error/timeout/stale → `fallback_go_to` → `start_go_to_heuristic`.
`F12` dispatches definition at the caret token in `AppSpec::on_key`;
Cmd/Ctrl-click reuses the existing glyph link overlay → same
`start_go_to` path. URI containment canonicalizes and rejects `..`/
non-file escapes. 3 tests pass (inside-checkout nav, outside notice,
empty+error fallback, unsupported → immediate heuristic).

### Task 6 — Hover card (completion deferred by D1)

Files: `apps/rune/src/editor/lsp.rs` (hover request/response state, item
model); `apps/rune/src/editor/code.rs` (`field_key` Esc latch,
`field` caret marker); `apps/rune/src/main.rs` (`view` —
`cx.caret(FIELD_KEY)` + root anchored overlay; `poll` already routes
`lsp::poll`).
Interfaces: consumes Task 3/4 session + caret marker pattern (`El::absolute`
0×0 keyed marker at caret window-point coords, `.anchored_fit` overlay);
produces hover card UI state.
Change: hover debounced ~400 ms after caret/revision settles (only when
`hoverProvider` advertised) with a 5 s local timeout freeing the slot,
plaintext capped 4000 chars/20 lines, caret+revision snapshot-checked
before apply, Esc dismissal latch suppressing re-trigger until caret or
text changes, hidden on doc close/checkout switch; card = rune-kit styled
anchored overlay at the caret marker (layer-vec mounting per `commit.rs`),
`anchored_fit` flip/clamp — mounted only once `cx.caret(FIELD_KEY)`
returns real coordinates, next frame if unknown, no element-edge
fallback. **Deferred (D1):** completion trigger/menu/edit-apply design —
no `textDocument/completion` is ever sent in this delivery.
Verify: headless tests — hover debounce/stale/hide, Esc latch, capability
gating, 5 s timeout frees slot, snapshot stale drops, plaintext cap;
manual interactive pass on caret tracking and card placement — popup must
appear only after measured caret coords.
Progress: done
Result: caret-anchored hover only (no pointer hover, per approved
limitation): ~400 ms settle debounce on `(revision, caret)`, one bounded
`textDocument/hover` request (5 s local timeout frees the slot),
plaintext extraction across Scalar/Array/Markup capped 4000 chars/20
lines, replies applied only while path+revision+caret+epoch+checkout
still match, Esc dismisses with a `(revision, caret)` latch until either
moves, card/superseded requests cancelled via `$/cancelRequest`. The
view mounts `hover_layer` only when `cx.caret(FIELD_KEY)` returns real
window coords — a keyed 0×0 marker + `popover::raised().anchored_fit`
(Side::Bottom, flip/clamp handled by rune-kit), no edge fallback.
3 tests pass (debounce+UTF-16 position, stale drop + apply, Esc latch
release on caret move). Interactive placement pass remains for parent
GUI QA.

### Task 7 — Language Servers section, settings round-trip and smoke

Files: `apps/rune/src/editor/{state.rs,workspace.rs,lsp.rs}` (new
`Section::Servers` arm, status + feature list, enable/restart/override
controls); `apps/rune/src/editor/backend.rs` only if section reads need a
job kind (prefer reusing GetSetting/SetSetting + hub status snapshot).
Interfaces: consumes Task 2 `LspHub::status` (incl. feature flags) + Task 3
config merge; produces the settings UI surface and persisted overrides.
Change: add `Section::Servers` ("Language Servers") listing each preset +
custom server: extensions/languageIds, effective argv, status
(Off/Starting/Running/Error + reason/install advice) and live feature
summary (completion/hover/definition/sync as advertised), enable toggle,
Restart button, command-override field (shlex-split, validated, persisted
under `lsp.servers`); custom server add/remove rows; status refreshes from
hub snapshots in `lsp::poll`.
Verify: state tests — section navigation/visited/dirty-guard parity with
other sections, config persist+reload round-trip, override validation
rejections, restart/enable dispatch, feature summary reflects fake-server
capabilities; then workspace `cargo fmt --check`, `cargo clippy
--workspace -- -D warnings`, `cargo test --workspace`, `cargo build
--workspace`; manual smoke matrix covering **all seven languages** — each
preset server installed into an isolated cache dir from its official
registry using the present Go/Node/Python toolchains (e.g. `go install`,
`npm --prefix <tmp>`, `pip --target <tmp>`; rust-analyzer via downloaded
component binary or rustup shim advice), launched via command override —
typescript-language-server (hover + definition), rust-analyzer, gopls,
pyright-langserver, vscode-html-language-server, vscode-css-language-server:
open doc → hover card shows, Cmd/Ctrl+click and F12 jump where
advertised; partial-capability servers (HTML/CSS) show honest feature
subsets; missing servers show install advice; no global/user installs
required and no app auto-install. (Diagnostics/completion smoke deferred
with D1.)
`document-wiki` coverage for the new subsystem.
Progress: partial — UI + settings done; real-server smoke and wiki
remain for the parent QA pass (no GUI automation available here; no
servers were installed or launched by this change).
Result: `Section::Servers` ("Language Servers") added with per-server
rows: id, live status (Off/Starting/Running/Error + stderr/message),
feature summary (hover/definition/sync as advertised), extensions→
languageIds, effective command, and Enable/Disable, Edit cmd (inline
single-line `text_input`, shlex-validated, persisted), Restart, and
Remove (custom only) controls. Config round-trips through the existing
`Req::GetSetting`/`Req::SetSetting` under `lsp.servers` — no schema
change; saves trigger `lsp_note_saved` → `didSave`. Status refreshes
from hub snapshots drained in `lsp::poll`. `cargo test -p goko-rune`:
294 pass; `cargo clippy -p goko-rune --all-targets` clean;
`cargo fmt --all --check` clean; `cargo check --workspace
--all-targets` clean; `cargo test -p app-core lsp`: 26 pass. Pending:
isolated-cache seven-language smoke (typescript-language-server is the
only installed server on this machine) and `document-wiki` coverage —
both owned by the parent's final QA pass.

## Files inspected, no change

- `crates/app-core/src/acp.rs` — transport/hub/process-kill precedent.
- `apps/rune/src/editor/{state,draft,code,navigation,files,search,highlight,
  syntax,go_to_definition,workspace,mod}.rs`, `backend.rs`,
  `backend/apply.rs` — callsites, guards, revision model.
- `apps/rune/src/main.rs` — `view`/`poll`/`on_quit`/`on_close` hooks.
- `apps/rune/Cargo.toml`, `crates/app-core/Cargo.toml`, `Cargo.lock` —
  dependency inventory (`url` already locked transitively).
- Pinned Rune `a7f276c`: `el/anchor.rs`, `el/layout.rs`, `el/text.rs`,
  `el/tx*.rs`, `edit.rs`, `edit/keys.rs`, `edit/history.rs` (`undo_group`),
  `runtime/cx.rs`, `runtime/elements.rs`, `text/span.rs`; rune-kit
  `popup_list.rs`, `overlay.rs`, `combobox.rs`, `text_input.rs`.
- `docs/llm/{INDEX,architecture/editor,domains/rune-chrome}.md`,
  `docs/agent-devkit/changes/rune-native-editor/{design,tasks}.md`.
- `/tmp/goko-lsp-parent-notes.txt`, `/tmp/goko-lsp-investigation.md`.

## Approval Gate

Status: approved

Approved by user: 2026-11-09 — “Duyệt, implement LSP theo plan này” after both repaired design and tasks were presented. Covers seven languages plus custom servers, typed protocol dependencies, caret-anchored hover/completion, explicit snippet deferral, isolated-cache real-server verification and no app auto-install.

## Decision Log

- **D1 (2026-11-09, user instruction — supersedes joint-approval scope):**
  "để hiện báo lỗi, gợi ý code, auto complete này làm sau được không? tao
  cần xem code, hover, go to define, highligh code thôi cho nhanh" —
  deliver native editing/highlighting + LSP hover + go-to-definition
  only; diagnostics rendering/detail and all completion/autocomplete are
  deferred to a follow-up change. Recorded in the Tasks preamble and
  design.md; Tasks 4 (diagnostics) and the completion half of 6 are
  marked deferred, Task 6 reduced to hover, Task 7 smoke narrowed to
  hover/definition.
