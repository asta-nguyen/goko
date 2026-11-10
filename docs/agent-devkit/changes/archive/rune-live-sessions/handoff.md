# Handoff: Rune main parity port — COMPLETE (implementation + verification)

## Objective

Finish review fixes and port Usage/quota, multiple accounts/account names,
split terminals, full create-worktree and finally Settings from `main` into Rune.
The user explicitly authorized implementation. No commits/pushes or real-account
credential/login actions for verification.

## State

- Phase: **complete** — every task in [tasks.md](tasks.md) is `done`; the change
  was archived after the review pass.
- Approval: gate approved; scope in [design.md](design.md).
- Branch `beta-rune`, HEAD `24e6115` at the start of the final session. The user
  owns the working tree: staged/unstaged/untracked files and the index were
  preserved (no reset/clean/stash/unstage/discards). **No commits were made.**
- The eight Task 20 findings from the previous checkpoint are fixed and covered
  by scenario regressions; the protocol fixture (Task 21) is verified.

## Decisions

- [D1](decisions.md#d1--settings-port-excludes-vim-implementation): user chose
  Appearance, Accounts, ACP agents and existing native editor preferences;
  **Vim implementation is explicitly deferred**, no inert Vim switch.
- Accounts preserve existing settings schema, implicit Default, safe named-folder
  launch env and metadata-only removal. Session account derives from own argv.
- AGENTS requires Devin CLI as the default executor; the user overrode it for
  this session ("làm luôn không cần gọi Devin CLI"), so the fixes were written
  and verified directly.

## Evidence

- `cargo test --workspace`: green — app-core 235, desktop 116, Rune 525,
  protocol 20, daemon 7, plus the smaller crates.
- `cargo fmt --check`: clean for every file of this change. The one file
  rustfmt still reports is `apps/rune/src/editor/history.rs`, which the user
  edited concurrently (branch picker → `Combobox`) — outside this change, so
  it was left as it is.
- `cargo clippy --workspace --all-targets -D warnings`: clean (four
  pre-existing lints fixed: the daemon's test module after items, an unused
  `mut`, a needless borrow, a collapsible `if`).
- New regressions: two launches keep their own answer (cross-agent and
  same-agent rows), a failed argv reply surfaces the error and leaves the other
  launch pending, custom-command shell-word parsing, account draft
  retention/refusal/stale-reply, ACP reset persistence (app-core + rune
  dispatch), expired-vs-live quota values, and a Settings render test proving
  the Default account has no Sign in button.
- Native QA: a separate QA bundle (`GokoQA.app`, distinct bundle id, isolated
  `GOKO_HOME`) was launched with `open -n --env GOKO_HOME=<temp>`. The app
  booted, spawned its own `gokod`, completed the handshake (green connection
  dot), rendered the header chips, the sidebar ("No projects registered") and
  the empty terminal state, and stayed alive; the isolated home holds
  `daemon.sock`, `daemon.token`, `daemon.log` and `goko.db`. The QA instance
  and its daemon were stopped afterwards; the user's own Goko instances were
  never touched.
- Wiki: `docs/llm/domains/rune-chrome.md` refreshed for the corrected
  behavior (preset launch, launch correlation and failure visibility, quoted
  custom commands, retained account drafts, persisted ACP Reset, expiry-aware
  account-menu quotas, Default without Sign in). Validator: 8 pages, 52
  requirements, 0 errors, 0 warnings; `git diff --check` clean.

## Changed files

- `crates/app-core/src/acp.rs` — `agents_reset` (persisted ACP override
  removal) with the shared store/reconcile helpers.
- Rune `sessions.rs` — token-correlated launches (`PendingArgv::token`,
  `take_pending_argv`, `resolved_launch`), shared shell-word parsing for custom
  commands.
- Rune `main.rs` — reply correlation for `AgentArgv`/`AccountAdd`/
  `AccountRename`, launch failures surfaced as the notice.
- Rune `settings.rs` — account drafts kept until the matching reply, inline
  errors, busy/stale handling, Default without Sign in, ACP Reset action.
- Rune `chrome.rs` — expiry-aware `limit_value` / `account_usage_summary`.
- Rune `editor/backend.rs` — accounts resolved only for Claude and Codex;
  `Req::{AgentArgv,AccountAdd,AccountRename}` carry a token.
- Rune `editor/acp.rs` — `AcpCmd::ResetAgent`.
- `crates/daemon/src/pty.rs`, `apps/rune/src/daemon{.rs,/tests.rs}`,
  `apps/rune/src/editor/{panes.rs,worktree_form.rs}` — lint-only fixes.
- Wiki pages and this change's artifacts.

## Verification limits

- Provider authentication was never exercised: no real account login, no
  credential writes, no global OS appearance change. The Sign in action is
  verified against the core's refusal of Default and the stored-account path,
  not against a live login.
- Click-level native UI QA was not possible: the terminal has no Accessibility
  permission (System Events reports zero windows for every app, Finder
  included), so only launch and rendered state could be observed.
- The usage chips read the real `~/.codex` (read-only) in the QA run — the
  isolated home covers storage and the daemon, not the provider's own files.

## Notes for a follow-up

- No requirement was added or retired by the final fixes; the wiki pages stay
  as the current-behavior record.
- The folder was moved to `changes/archive/rune-live-sessions/` and relinked
  under `## Archived changes` in [INDEX.md](../../../INDEX.md); the design page's
  wiki links were re-pointed one level deeper. Archived folders are history:
  further work on this scope starts a new change.
- The user was editing the same working tree concurrently during the final
  session (the history branch picker became a `Combobox`, the patch pane grew
  a spinner, `term.rs` lost its scratch module). Those edits are theirs; the
  final workspace test run covers them too and stays green.
