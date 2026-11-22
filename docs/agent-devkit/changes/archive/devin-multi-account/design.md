# Design: named Devin CLI accounts

## Goal

Let a Goko user switch between two or more Devin logins without logging out.
Reuse the Claude/Codex account UI and launch-time selection. Preserve the
current Devin login as Default.

## Scope

- macOS and Linux local terminal sessions started through the Devin preset.
- Add, sign in, select, rename, and remove Devin accounts in Settings >
  Accounts. Offer quick selection in the existing Agents dropdown.
- Exclude Windows named accounts, live-session handoff, per-project defaults,
  automatic switching, new quota collection, CLI account migration, and ACP
  agent routing. Do not change `gokod` or its wire protocol.

## Behavior

- Default continues to launch the plain Devin command with the ambient
  environment. It is implicit, cannot be removed or renamed, and never
  triggers sign-in from Settings.
- A named account has a name and a dedicated absolute folder, using the
  existing add form. The folder is the account's `XDG_DATA_HOME`; it can
  be an existing dedicated Devin data root. Add activates the entry and
  opens the same sign-in terminal as the row's Sign in action. Devin owns
  that login flow; Goko never imports, copies, reads the contents of, or
  links its credential file. Canceling the login leaves the registered
  account in place so the user can retry or return to Default.
- Named sign-in and regular launches set `XDG_DATA_HOME` to the account
  folder and `XDG_CONFIG_HOME` to `config` within it. This also isolates
  organization selection and user-level CLI configuration; Goko does not
  seed or share that config. Project instructions are still discovered in
  the checkout on regular launches. Unset `WINDSURF_API_KEY` for named
  launches so an ambient key cannot replace the selected profile's login.
  Keep the CLI's cache under the named profile too, so the existing ambient
  usage collector cannot mistake a named launch's data for Default quota.
- The active account is per Goko installation and survives restart. Select
  applies to future preset launches only, in whichever checkout started
  them. The sidebar derives each row's label from its own launch argv,
  never the current selection. If a named account is removed, its already
  launched row shows the folder basename. Removing a registration leaves
  its files untouched and resets an active removed account to Default.
- The Agents dropdown shows the Devin selector with Default plus at least
  one named account, even with no collected usage. It checks the selected
  row, shows no per-account quota, and does not display the existing
  Default-only meter while a named account is selected. Claude/Codex
  selection and quota are unchanged.

## Edge cases

- Reject missing names, invalid paths, or paths equal to or aliasing the
  effective Default data root or another Devin account root. Recheck named
  profile paths for aliasing before login/launch, including symlinks at the
  profile's `devin` directory, `devin/credentials.toml`, `config` directory,
  or `config/devin` directory. Do not read or rewrite credentials to do this.
- For a registered account whose folder is gone or unsafe, or a missing
  Devin binary, show the existing error notice and do not silently launch
  Default. Missing or unknown active ids in persisted settings normalize to
  Default as they do for the existing providers.
- Existing settings without a `devin` field read as Devin Default-only,
  retaining the Claude/Codex entries. A failed mutation retains the existing
  view and draft just like the Claude/Codex form.
- On Windows, preserve the ordinary Devin preset; do not expose or launch
  named Devin accounts using an environment convention that does not isolate
  credentials there.

## Interfaces

- Add a backward-compatible `devin` member to the existing `accounts`
  settings value and `AccountsView`. Use the new provider id with the existing
  worker requests; retain their payloads and current persistence path.
- For a named profile, generate argv equivalent to
  `/usr/bin/env -u WINDSURF_API_KEY XDG_DATA_HOME=<dir> XDG_CONFIG_HOME=<dir>/config XDG_CACHE_HOME=<dir>/cache devin --process-model in-process`.
  Use the resolved executable and separate argv words, including for paths
  with spaces. Sign-in appends `auth login` and runs in the account folder.
  `in-process` avoids reusing a daemon during a named login or launch. The
  account label and session diagnostics must recognize the prefixed argv.
- Register current behavior with
  `RNC-devin-accounts`, `RNC-sidebar-row-actions`, `RNC-session-account-label`,
  and `RNC-usage-accounts` in the delta. The separate Extensions change owns
  `RNC-settings-page`; this change adds the Devin-specific requirement without
  rewriting that existing Settings requirement.

## Verification

- Verify the store reads an older accounts value without losing data, and
  verify independent selection, alias/missing-folder handling, and
  metadata-only removal. Verify named sign-in and worker launch select the
  same isolated environment; Default emits unchanged argv.
- Exercise the dropdown without quota data, add-to-sign-in routing, and
  sidebar labels using session argv rather than the current account.
- Run the required workspace build, format, clippy, and tests. Smoke-test the
  real macOS Goko window with a uniquely named test bundle, scratch Goko home,
  and a fake Devin binary that reports its environment. Check Default,
  named add/sign-in, switch, launch, and the old row's label. Do not open or
  mutate any real account. Use the CLI's read-only `auth status` with an empty
  data root to check isolation without a login.

## Related context

- [Rune chrome requirements](../../../../llm/domains/rune-chrome.md)
