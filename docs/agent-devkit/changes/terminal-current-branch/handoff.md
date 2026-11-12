# Terminal current branch

Approval: 2026-11-12 — user approved the bounded chat design, then said “ok”.

## Scope

Badge beside Terminal header cwd, following focused pane/session cwd. Background Git read, one outstanding request, three-second refresh while active. Hide stale/failed/outside-repo identities; detached HEAD shows short commit. No daemon/protocol change.

## Implementation

- `apps/rune/src/terminal_branch.rs`: focused target, bounded polling, stale-reply rejection, badge, two regression tests.
- `apps/rune/src/main.rs`: state and poll wiring.
- `apps/rune/src/chrome.rs`: header mounts badge next to path.
- Delta applied to Rune chrome wiki; global validator remains failing, so no merge marker or archive yet.

## Verification

- Baseline chrome tests: 8 passed.
- `cargo build --workspace`: pass, two pre-existing unfulfilled lint expectations in `editor/quit_guard.rs:30,32`.
- `cargo fmt --check`: pass.
- `cargo test -p goko-rune`: 750 passed (includes two new Terminal branch tests).
- `cargo test --workspace`: fails in untouched `crates/rune-kit/src/markdown/stream_tests.rs:65`, `an_open_span_never_shows_its_markers`; remaining suites passed.
- `cargo clippy --workspace -- -D warnings`: fails on `editor/quit_guard.rs:30,32`, unfulfilled lint expectations.
- `cargo clippy --workspace -- -D warnings -A unfulfilled-lint-expectations`: pass.
- `git diff --check`: pass.
- Wiki validator: 167 errors, mostly removed `apps/desktop` paths, existing missing Sources/sort errors in rune-chrome, and a user-owned rune-editor change. New requirement has no validator error.
- Independent source reviewer: no concrete bugs or simplifications.

## UI verification pending

Built `target/GokoBranchQA.app` from fresh `target/debug` with bundle ID `dev.goko.branch-qa`. Two QA instances (PIDs 81928 and 85238) were launched; main windows are hidden in Stage Manager. Headless snapshot cannot reach them. Only app-window screenshot was taken, showing the Stage Manager thumbnail, so badge appearance is not verified. User asked asynchronously for permission to foreground only QA; no headed action taken.

## User-owned changes

Existing edits in editor/backend.rs, editor/backend/apply.rs, editor/code.rs, editor/files.rs, editor/mod.rs, app-core/git_view.rs, rune-editor wiki, and untracked editor/line_changes.rs were left intact. Do not revert them. No commit or push requested.
