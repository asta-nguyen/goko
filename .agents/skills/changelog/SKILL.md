---
name: changelog
description: Use when writing or updating CHANGELOG.md entries, preparing a release, or when commits since the last release have no changelog entry or no contributor credit.
---

# Changelog

Build the `## [Unreleased]` entries of `CHANGELOG.md` from the commits since the
last release, each credited to who made it and the commits it came from. Users
read the changelog as "What's new" in Settings → Updates and on the GitHub
release, so write for them.

## 1. List the commits

From the repository root:

`node "<this-skill-directory>/scripts/commits.mjs"`

It prints, oldest first, `status hash login PR subject` for every commit since
the newest `release: [x.y.z]` commit.

- `ok`: the changelog already names the hash. `skip`: not user-visible on its
  own (a PR's merge commit, test, docs, ci, chore, build, style, release).
  `MISSING`: user-visible and absent from the changelog.
- The login is exact when it comes from a noreply address or the commits API.
  A trailing `?` is a guess from the author name: confirm it with the user
  before writing it, and never invent a login.
- `+co:` lists `Co-authored-by` trailers: credit those people too.

## 2. Write an entry per change

- One entry per user-visible change. A PR's commits, and the follow-up fixes to
  it, share one entry; a refactor, test or doc commit gets none. A `skip`ped
  commit that changes what users see still gets one: read the diff when unsure.
- File it under `### Added` (new), `### Changed` (existing behavior), `### Fixed`
  (a bug) or `### Removed`.
- Say what the user sees, not how it works; name where a setting lives. Match
  the surrounding entries and wrap at 80 columns.

## 3. Credit

Follow the convention at the top of `CHANGELOG.md`:

- Your own commits: ``By [@login](https://github.com/login) in `abc1234`.``
- A contributor's PR: ``Contributed by [@login](https://github.com/login) in #123
  (`abc1234`, `def5678`).`` Add a follow-up by someone else in the same entry:
  ``, with follow-up fixes by [@other](https://github.com/other) in `fff0000`.``
- Hashes are the 7-character short form, which is what the script matches.
- Keep each link on one line: What's new shows `[text](url)` as its text, line
  by line (`note_rows` in `apps/rune/src/update.rs`).

## 4. Verify

- `node "<this-skill-directory>/scripts/commits.mjs" --missing` prints nothing
  and exits 0.
- A rebase, amend or squash changes hashes. Run it again after the last history
  edit, and before the `release:` commit, so every hash in the changelog names a
  commit that exists.

When cutting a release, do this first, then rename `[Unreleased]` (AGENTS.md,
"Cutting a release").
