# Delta

Approved: 2026-11-12 — user approved the bounded branch-header design and requested implementation.

## ADDED Requirements

### RNE-current-branch

Page: `docs/llm/domains/rune-editor.md`
Search: checked all RNE requirements and visible open deltas; none covers a live current-branch header.

Requirement:

````md
### RNE-current-branch

The system SHALL show the current checkout's branch in the shared editor header across all sections, showing Detached with the short commit ID when detached, refreshing in the background while the editor is active, and showing loading or unavailable states instead of another checkout's branch or a failed cached value without changing Git state or document drafts.

#### Scenario: Branch stays visible

- GIVEN a checkout with a named branch is selected
- WHEN the user opens Files, Changes, History, Stashes, or Servers
- THEN the shared header displays that branch, truncating long names with the full name available on hover

#### Scenario: External switch or detached HEAD

- GIVEN the editor is active
- WHEN Git changes the checkout branch or detaches HEAD externally
- THEN a background worker refresh updates the header within the next three-second polling interval plus worker time, and detached HEAD includes its short commit ID

#### Scenario: Loading or failure

- GIVEN a checkout changes or its Git metadata cannot be read
- WHEN the header renders before a successful read
- THEN it shows loading or an unavailable state and does not present stale metadata as current

Evidence: `apps/rune/src/editor/branch.rs`, `apps/rune/src/editor/workspace.rs`, `apps/rune/src/editor/backend.rs`, `apps/rune/src/editor/backend/apply.rs`, `apps/rune/src/main.rs`
````

## Implementation evidence

- Baseline before source edits: `cargo test -p goko-rune` passed 735 tests. Existing staged inline-conflict changes were treated as user-owned baseline; this change never alters the index.
- Current impact: shared `workspace::pickers` → `branch::view`; `main::poll` → `branch::poll` → checkout-validated worker `Req::CurrentBranch` → generation/checkout-guarded reply. Checkout navigation clears metadata only after the draft guard commits the selection. No Git writes, schema/dependency/API changes or draft reloads.
- `branch.rs` tests real named/unborn/detached/invalid repositories plus interval, pending, hidden-editor, refusal and foreign/error-state behavior. Backend tests cover stale checkout/generation replies. The workspace test covers long names and minimum branch width across all five sections; existing History splitter drag regressions also pass after responsive sizing was corrected.
- Scoped independent read-only review found no remaining issues. Documentation updated through document-wiki Task updates; Rune editor page has zero validation errors. The full validator still reports 166 unchanged errors in legacy desktop, overview, and Rune chrome pages (`/tmp/goko-branch-wiki-validation.log`).
- Native QA: freshly built `target/GokoBranchReview.app`, bundle ID `dev.goko.branchreview`, isolated temporary GOKO_HOME and Git repository. Observed named branch, external Git branch switch updating header, retained badge in Files, and detached short ID. Window-only screenshots at normal and minimum window sizes were inspected; branch width gets priority over checkout path. `target/Goko.app` was rebuilt; installed /Applications copy was not changed.
- Changelog Unreleased now includes the current-branch header and the earlier inline conflict feature.
- Verified at: 901f0307dae6926096f454393b7932d5fed97004 plus current working tree.

## Final verification

- `cargo build --workspace`, `cargo fmt --check`, and `git diff --check`: passed.
- `cargo test --workspace`: 742 Rune tests passed, including all new branch tests and existing History drag tests; full command fails only on the unchanged `crates/rune-kit/src/markdown/stream_tests.rs:65` Markdown streaming assertion.
- `cargo clippy --workspace -- -D warnings`: fails on existing unfulfilled expectations at `apps/rune/src/editor/quit_guard.rs:30,32`. Allowing only that existing lint yields no issues.
- Wiki validator: changed Rune editor page has zero errors; existing 166 errors elsewhere keep full validation failing. Requirement content is merged, but final merge marker/archive remain pending repository verification.
- Native final-width QA observed full `Detached · 936a8dd9` at 760×600 and inspected `/tmp/goko-branch-narrow-final.png`; named branch and external switch were observed earlier on the same fixture. Only the Goko window was captured; all QA app/daemon processes were stopped afterward.
- No commit, push, install, or staging was performed. Existing staged conflict changes remain intact. This change remains open due to the repository-wide blockers above.
