# Named Devin CLI accounts tasks

## Design

[Named Devin CLI accounts](design.md)

## Global Constraints

- macOS and Linux local terminal sessions started through the Devin preset.
- Do not change `gokod` or its wire protocol.
- Default continues to launch the plain Devin command with the ambient
  environment.
- Goko never imports, copies, reads the contents of, or links its credential
  file.
- The active account is per Goko installation and survives restart. Select
  applies to future preset launches only, in whichever checkout started
  them.

## Impact map

Entry: `apps/rune/src/settings.rs::accounts_section`, `apps/rune/src/chrome.rs::usage_menu_layer`, `apps/rune/src/sessions.rs::request_sign_in` and the preset launcher.

Flow: Settings and Agents dropdown enqueue existing `Req::Account*` requests; the editor worker calls `app_core::accounts` to load, mutate, and return the account view; `Req::AgentArgv` resolves the active folder via `accounts::resolve` and `agents::argv_for_account`; the app passes argv and working directory to `session.create`; the sidebar labels a session via `data::account_name_of` and `session_debug` detects its program from argv.

State changes: `accounts` JSON in Goko's existing settings store; the proposed `devin` field is defaulted when absent. UI accounts are held in `Data::accounts`.

External effects: add creates a dedicated profile folder if missing; child login and TUI own their profile files. Remove updates the registry only. The daemon receives the same `session.create` payload shape.

Change candidates: existing account store and launch helpers, Rune worker, Settings, dropdown, add reply and row-label paths, unit test fixtures, and the Rune chrome wiki.

Verification: focused `app-core` and `goko-rune` account/diagnostics tests, required workspace checks, and the scratch macOS bundle exercise in the design.

Verified at: 4d474c04d152211bc2c63fadc5787da7be5646b9

## Tasks

### Task 1: Extend account storage and named invocation

Files: `crates/app-core/src/accounts.rs::Accounts`, `AccountsView`, `default_dir`, `add`, `resolve`, `sign_in_launch`; `crates/app-core/src/agents.rs::argv_for_account`; `crates/app-core/src/session_debug.rs::agent`; affected `Accounts` literals in `crates/app-core/src/usage.rs::tests`.

Interfaces: preserves the current settings key, row shape, and sign-in reply; adds the Devin provider view and launch environment used by Task 2.

Change: Default to an empty Devin registry in old settings; reuse normal account mutation and normalization. On supported OSes, resolve Default from the effective `XDG_DATA_HOME` or `~/.local/share`. Treat a named folder as an XDG data root and validate it and its auth/config path components on add and before sign-in or launch. Share neither data files nor config. Build the same named environment for login and regular launch: separate argv words, no ambient `WINDSURF_API_KEY`, in-process execution, and `auth login` for sign-in only. Preserve ordinary Devin argv for Default and keep named launches unsupported on Windows. Teach session diagnostics to identify Devin behind the environment prefix without changing other agent detection.

Verify: Before the first source edit, run and record existing `cargo test -p app-core account` and `cargo test -p goko-rune account`. Add a regression check for loading older JSON with non-default Claude/Codex selections, then adding/selecting/removing Devin without changing those selections or deleting its folder. Cover duplicate or aliased roots, missing profiles, identical env in login/launch with a space in the path, unchanged Default argv, and diagnostics that still recognize a named launch. Run `cargo test -p app-core`.

Progress: done

Result: `cargo test -p app-core` passed 380 tests, including legacy account settings, profile validation, isolated argv, and session diagnostics.

### Task 2: Wire Devin through the existing worker and Rune controls

Files: `apps/rune/src/editor/backend.rs::agent_argv` and account request handling; `apps/rune/src/main.rs` account reply handling; `apps/rune/src/settings.rs::accounts_section`; `apps/rune/src/chrome.rs::usage_records`, `usage_menu_layer`; `apps/rune/src/data.rs::account_name_of`; `apps/rune/src/sidebar.rs` session row and account label; affected view fixtures in `apps/rune/src/view_tests.rs` and targeted worker tests in `apps/rune/src/editor/backend/tests.rs`.

Interfaces: consumes the Devin member and environment from Task 1; reuses `AccountAdd`, `AccountSignIn`, `AccountSetActive`, and existing session creation.

Change: Show the Devin card and account dropdown on supported OSes; keep Default available, request sign-in for the actual new Devin id, and send select changes through the worker. Use the selected profile for Devin presets only. Leave current processes alone and label them with their original launch account. Make the selector visible without a usage record; when a named Devin account is active, suppress the Default-only display for Devin rather than showing another login's quota. Leave Claude/Codex and other preset behavior intact. Missing or unsafe selected profile or missing executable reports the existing notice without a replacement launch.

Verify: `cargo test -p goko-rune account` plus the Devin dropdown test by name if not covered by the filter. Check that a Devin add reply queues its own sign-in, the worker emits the named argv for a project launch and rejects an unavailable folder, and the dropdown can select Devin without usage while an old session keeps its original label.

Progress: done

Result: `cargo test -p goko-rune account` passed all 15 tests, including Devin add-to-sign-in, worker routing, and account selection without usage.

### Task 3: Update the wiki and check the real app

Files: `docs/llm/domains/rune-chrome.md` requirements owned by [the delta](delta.md).

Interfaces: documents the implemented Settings, dropdown and terminal behavior. Uses the normal bundled app entry point.

Change: Merge [the delta](delta.md) using `document-wiki` Task updates. Do not change the Extensions-owned `RNC-settings-page` or link to process files from the wiki.

Verify: Wiki validator passes and all four delta blocks reflect source and tests. Run `cargo build --workspace`, `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace`. Use `agent-desktop` on a freshly built, uniquely named macOS bundle and scratch Goko home only. A fake Devin binary in the scratch PATH reports cwd, argv and XDG path values (no credential contents) for Default, add/sign-in, and named preset launches. Confirm quick selection works with no usage record, the earlier session keeps its old label, and Default still has the ambient settings after a named launch. Check the installed real CLI with read-only `auth status` in a fresh data root. Live second-account login and Windows/Linux GUI interaction are not part of the local smoke test.

Progress: done

Result: Workspace build, format, Clippy with warnings denied, and all workspace test suites passed. The wiki validator reported zero errors/warnings and the four delta blocks match the wiki. A fresh macOS scratch bundle verified the Default launch, named add and isolated sign-in, named checkout launch with a private cache, selection back to Default without quota, and an unchanged Work label on the earlier session. The real CLI reported `Not logged in` in an empty profile; no real login or Windows/Linux GUI was exercised.

## Files inspected, no change

- `apps/rune/src/sessions.rs::request_sign_in`, `launch_custom_session`: existing session paths accept resolved argv and working directory.
- `crates/protocol/src/message.rs`: session creation contract does not need an environment field.
- `crates/daemon/src/pty.rs`: existing process spawn runs the environment wrapper as argv, not as shell text.
- `crates/app-core/src/usage_collect/devin.rs`: collector reads the ambient Devin paths; no named-account quota collection.

## Approval Gate

Status: approved

## Decision Log

None.

## Baseline

- `cargo test -p app-core account`: 61 passed.
- `cargo test -p goko-rune account`: 13 passed.
- Zig 0.16 resolved through the installed Homebrew executable.
