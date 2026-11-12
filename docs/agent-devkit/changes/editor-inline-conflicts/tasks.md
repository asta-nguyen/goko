# Inline conflict implementation

## Approval Gate

Status: approved
Approved: 2026-11-12 — user invoked implement-task after the bounded chat design.

## Work

- Inline editor actions, guards, and scoped documentation.
  Progress: done
  Result: `cargo test --workspace` — 735 Rune tests passed; real native app Accept Both → Mark Resolved saved and staged only demo.rs in a temporary repository. Independent review found no remaining in-scope blocker.
- Full repository verification and archive.
  Progress: todo
  Result: blocked by unchanged baseline failures listed below; change remains open.

## Worker attempts

- Parser attempt 1: fail — diff3 rejected, nested block accepted, empty-side fixture incorrect.
- Parser attempt 2: fail — selected base instead of current and weakened assertion; orchestrator finished the parser.

## Impact map

Entry: `changes.rs::activate_entry` (row press and `keyboard.rs::nav_activate`).
Flow: guarded `files::open_doc` → `OpenDoc` / `CodeState` → inline parser/actions → `save_active_doc` → `finish_save` → `StageResolved` → `git_view::file` / `git_write::stage`.
State changes: document buffer, native undo, conflict ranges, resolve-after-save flag, Git index for one file.
External effects: expected-text file save and literal-path Git staging; no commit, API/schema/dependency/CI change.
Change candidates: implemented files shown in Git diff, new `conflict.rs`, Rune editor wiki.
Verification: cargo build --workspace; cargo fmt --check; cargo clippy --workspace -- -D warnings; cargo test --workspace; native accessibility interaction and temporary Git repository checks.
Verified at: 901f0307dae6926096f454393b7932d5fed97004

## Verification results

- Baseline: fmt passed; workspace tests failed only at existing Markdown streaming assertion (`crates/rune-kit/src/markdown/stream_tests.rs:65`); 725 Rune tests passed before edits.
- Final build, fmt, and diff whitespace check passed; 735 Rune tests passed in the final workspace run.
- Final workspace test run retains the same Markdown streaming failure, outside this change.
- Full Clippy is blocked by unfulfilled lint expectations in unchanged `apps/rune/src/editor/quit_guard.rs:30,32`. Clippy with only that existing lint allowed passed.
- Wiki validator: zero errors for changed `docs/llm/domains/rune-editor.md`; 166 existing errors in unchanged legacy desktop/overview/Rune chrome pages. Full output: `/tmp/goko-conflict-wiki-validation.log` (local verification artifact).
- Final independent read-only review passed the scoped parser, native actions, save/stage guards, and keyboard/pointer callers. Initial media-routing and mixed-malformed-message findings were fixed and covered by tests.
- Native QA used the fresh `target/GokoConflictQA.app` binary (mtime newer than HEAD), unique bundle ID `dev.goko.conflictqa`, isolated `GOKO_HOME` and repository. Accessibility confirmed disabled Mark Resolved before acceptance, edited Both content, successful save/stage and removal of Mark Resolved after changes refreshed. Screenshot was window-only but Stage Manager showed a thumbnail, limiting visual detail. QA processes were stopped after the saved test; `target/Goko.app` was rebuilt for user review.
- The new requirement block is copied into the wiki; archive and final merge marker remain pending full verification.
