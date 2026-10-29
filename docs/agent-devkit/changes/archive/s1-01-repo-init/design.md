# s1-01 Repo init and workspaces

## Goal

An empty but buildable skeleton: a git repo, a pnpm workspace root, and a Cargo
workspace with `protocol` and `daemon` crates, so every later plan only adds code.
Part of the [S1 contract](../../../s1-contract.md) and the [roadmap](../../../roadmap.md).

## Scope

In: `git init` (no commit), `.gitignore`, root `package.json`, `pnpm-workspace.yaml`,
root `Cargo.toml`, crates `protocol` and `daemon` (lib plus the `gokod` bin).
Out: any protocol or daemon logic, the frontend and Tauri (s1-02).
Depends on: nothing.

## Behavior

- `gokod --version` prints `gokod <crate version>` and exits 0. Any other argument,
  or none, prints a one-line usage to stderr and exits 2 (s1-04 replaces this with
  the real server).
- `cargo build --workspace` produces no warnings.

## Edge cases

- `git init` in an existing repo is a no-op; no existing file is overwritten.
- `Cargo.lock` is generated and left unignored for a future repository-owner commit; this task does not make a Git commit.
- `pnpm install` must succeed while `apps/*` matches nothing.

## Interfaces

`protocol::PROTOCOL_VERSION: u32 = 1`. Crates are named `protocol` and `daemon`; the
binary is `gokod`. Root scripts `dev`, `build`, `typecheck`, `test` delegate to
`apps/desktop` (created in s1-02).

## Verification

- `cargo build --workspace`, `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`.
- `cargo run -q -p daemon --bin gokod -- --version` prints the version.
- Individual `git check-ignore -q` checks succeed for `target/`, `node_modules/`, `dist/`, `apps/desktop/src-tauri/gen/`, and `.DS_Store`; the check for `Cargo.lock` returns 1.

## Related context

None.
