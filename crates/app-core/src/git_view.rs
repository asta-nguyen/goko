//! Git inspection and bounded file editing for the workspace Editor. Public
//! functions receive a checkout that `projects::git_checkout` already
//! validated: registered project, root-or-listed-worktree path, real non-bare
//! Git worktree root. Git calls use separate argv; file saves only replace a
//! validated working-tree file and never mutate the index, refs, or stash.

use crate::error::{BridgeError, Code};
use crate::worktrees;
use protocol::message::ErrorCode;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Diffs, text reads, and file saves are capped at 1 MiB.
const CAP: usize = 1024 * 1024;
const SEARCH_OUTPUT_CAP: usize = 256 * 1024;
const SEARCH_MATCH_CAP: usize = 500;
const SEARCH_PREVIEW_CHARS: usize = 300;
static TEMP_FILE_ID: AtomicU64 = AtomicU64::new(0);
/// Commits per History page.
const HISTORY_PAGE: u64 = 100;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StatusEntry {
    /// Checkout-relative path. For a rename this is the destination.
    pub path: String,
    /// Rename or copy source path, when Git reported one.
    pub orig_path: Option<String>,
    /// `modified`, `added`, `deleted`, `renamed`, `copied`, `typechange`,
    /// `unmerged`, or `untracked`.
    pub status: &'static str,
    /// Lines added, from `git diff --numstat`; `None` outside commit diffs
    /// and for binary files.
    pub added: Option<u64>,
    /// Lines removed, from `git diff --numstat`; `None` outside commit diffs
    /// and for binary files.
    pub removed: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitViewChanges {
    /// Symbolic branch name; `None` on a detached HEAD.
    pub branch: Option<String>,
    /// Abbreviated HEAD oid; empty on an unborn branch.
    pub head: String,
    pub conflicts: Vec<StatusEntry>,
    pub staged: Vec<StatusEntry>,
    pub unstaged: Vec<StatusEntry>,
    pub untracked: Vec<StatusEntry>,
    /// Commits on HEAD not yet on its upstream; `None` without an upstream
    /// or before the first commit.
    pub ahead: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitViewDiff {
    /// Human-readable diff layer label (e.g. `HEAD → index`, `stash@{2}`).
    pub label: String,
    pub patch: String,
    /// `true` when the unified diff exceeded 1 MiB and was cut.
    pub truncated: bool,
    /// Changed files for commit and stash layers; empty for per-file layers.
    pub files: Vec<StatusEntry>,
    /// Commit identity and message body; `Some` only for `layer = commit`.
    pub detail: Option<GitViewCommitDetail>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitViewCommitDetail {
    /// Author email address.
    pub author_email: String,
    /// Committer display name; shown only when it differs from the author.
    pub committer: String,
    /// Committer email address.
    pub committer_email: String,
    /// Message body (`%b`), without the subject line.
    pub body: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitViewCommit {
    /// Full commit oid (immutable identity).
    pub id: String,
    pub parents: Vec<String>,
    /// Local branches, `remotes/`-style remote-tracking refs, and `tag:` names.
    pub refs: Vec<String>,
    /// `true` when this commit is the checkout's HEAD.
    pub head: bool,
    pub author: String,
    /// Commit time, epoch milliseconds.
    pub time: u64,
    pub subject: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitViewHistory {
    pub commits: Vec<GitViewCommit>,
    /// Skip offset for the next page; `None` at the end.
    pub next_skip: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitViewBranch {
    /// Full ref (`refs/heads/…` or `refs/remotes/…`) — the value `history`
    /// accepts as its branch filter.
    pub ref_name: String,
    /// Short display name (`main`, `origin/main`).
    pub name: String,
    /// `true` for remote-tracking refs under `refs/remotes/`.
    pub remote: bool,
    /// `true` for the branch this checkout has checked out.
    pub current: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitViewTreeEntry {
    /// Checkout-relative path.
    pub path: String,
    /// Last path component, used as the display name.
    pub name: String,
    /// `dir` or `file`.
    pub kind: &'static str,
    /// Matched by a Git ignore rule; shown dimmed rather than hidden.
    pub ignored: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitViewTree {
    /// One directory level: subdirectories first, then files.
    pub entries: Vec<GitViewTreeEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitViewFile {
    pub path: String,
    /// `text`, `binary`, `nonutf8`, `missing`, or `unreadable`.
    pub kind: &'static str,
    /// Present only for text files; read responses may be truncated.
    pub text: Option<String>,
    /// `true` when the file exceeded 1 MiB and the preview was cut.
    pub truncated: bool,
    /// File size in bytes.
    pub size: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitViewSearchMatch {
    pub path: String,
    pub line: u64,
    pub text: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitViewSearch {
    pub matches: Vec<GitViewSearchMatch>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitViewStash {
    /// `stash@{N}` position.
    pub index: usize,
    /// Full stash commit oid — immutable identity for detail requests.
    pub id: String,
    /// Stash message (`WIP on …`/`On …:` suffix when Git recorded one).
    pub message: String,
    /// Branch hint parsed from the message, when present.
    pub branch: Option<String>,
    /// Stash commit time, epoch milliseconds.
    pub time: u64,
}

/// The checkout must itself be a non-bare Git worktree root. A plain
/// directory, a repository subdirectory, or a bare repo rejects before any
/// inspection runs.
pub fn repo_checkout(checkout: &Path) -> Result<(), BridgeError> {
    let top = worktrees::run(checkout, &["rev-parse", "--show-toplevel"], None)?;
    if !top.status.success() {
        return Err(invalid("the checkout is not a Git worktree"));
    }
    let canonical_top = std::fs::canonicalize(trim_str(&top.stdout))
        .map_err(|_| invalid("the checkout is not a Git worktree"))?;
    if canonical_top != checkout {
        return Err(invalid("the checkout is not the root of its Git worktree"));
    }
    let bare = worktrees::run(checkout, &["rev-parse", "--is-bare-repository"], None)?;
    if !bare.status.success() || trim_str(&bare.stdout) != "false" {
        return Err(invalid("the checkout is not a Git worktree"));
    }
    Ok(())
}

/// `git status` grouped into conflicts / staged / unstaged / untracked.
pub fn changes(checkout: &Path) -> Result<GitViewChanges, BridgeError> {
    let out = worktrees::git(
        checkout,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    let mut view = parse_status(&out);
    let branch = worktrees::run(
        checkout,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        None,
    )?;
    if branch.status.success() {
        view.branch = Some(trim_str(&branch.stdout).to_owned());
    }
    let head = worktrees::run(checkout, &["rev-parse", "--short=8", "HEAD"], None)?;
    if head.status.success() {
        view.head = trim_str(&head.stdout).to_owned();
    }
    let ahead = worktrees::run(checkout, &["rev-list", "--count", "@{u}..HEAD"], None)?;
    if ahead.status.success() {
        view.ahead = trim_str(&ahead.stdout).parse::<u32>().ok();
    }
    Ok(view)
}

/// Unified diff for one layer: `staged`, `unstaged`, `untracked`, `conflict`
/// (file layers), or `commit` (whole-commit patch with a changed-file list).
pub fn diff(
    checkout: &Path,
    layer: &str,
    path: Option<&str>,
    commit: Option<&str>,
) -> Result<GitViewDiff, BridgeError> {
    match layer {
        "staged" | "unstaged" | "conflict" => {
            let rel = required_path(path)?;
            let rel = rel_str(&rel)?;
            let label = match layer {
                "staged" => "HEAD → index",
                "conflict" => "conflict",
                _ => "index → working tree",
            };
            let base: &[&str] = if layer == "staged" {
                &["diff", "--cached", "--"]
            } else {
                &["diff", "--"]
            };
            let (patch, truncated) =
                worktrees::git_bounded(checkout, [base, &[rel]].concat().as_slice(), CAP)?;
            Ok(GitViewDiff {
                label: label.to_owned(),
                patch: String::from_utf8_lossy(&patch).into_owned(),
                truncated,
                files: Vec::new(),
                detail: None,
            })
        }
        "untracked" => {
            let rel = required_path(path)?;
            // `--no-index` reads filesystem paths directly, so resolve the
            // target first and reject symlink escapes before Git touches it.
            let target = std::fs::canonicalize(checkout.join(&rel))
                .map_err(|_| invalid("the untracked file is not accessible"))?;
            if !target.starts_with(checkout) {
                return Err(invalid("the path resolves outside the checkout"));
            }
            let out = worktrees::run_bounded(
                checkout,
                &["diff", "--no-index", "--", "/dev/null", rel_str(&rel)?],
                CAP,
            )?;
            // Exit 1 means "differences found"; stdout-empty with stderr is an
            // error such as an unreadable file.
            if !out.truncated {
                match out.status.code() {
                    Some(0) | Some(1) if !out.stdout.is_empty() || out.stderr.is_empty() => {}
                    _ => return Err(worktrees::git_error(&out.stderr)),
                }
            }
            let patch = String::from_utf8_lossy(&out.stdout).into_owned();
            Ok(GitViewDiff {
                label: "new untracked file".to_owned(),
                patch,
                truncated: out.truncated,
                files: Vec::new(),
                detail: None,
            })
        }
        "commit" => {
            let oid = object_id(commit.ok_or_else(|| invalid("a commit id is required"))?)?;
            commit_diff(checkout, oid)
        }
        _ => Err(invalid("unsupported diff layer")),
    }
}

fn commit_diff(checkout: &Path, oid: &str) -> Result<GitViewDiff, BridgeError> {
    let exists = worktrees::run(
        checkout,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{oid}^{{commit}}"),
        ],
        None,
    )?;
    if !exists.status.success() {
        return Err(not_found(format!("no commit '{oid}' exists")));
    }
    let parent = format!("{oid}^");
    let has_parent = worktrees::run(
        checkout,
        &["rev-parse", "--verify", "--quiet", &parent],
        None,
    )?
    .status
    .success();
    let (names, numstat, patch, mut truncated) = if has_parent {
        let names = worktrees::git(checkout, &["diff", "--name-status", "-z", &parent, oid])?;
        let numstat = worktrees::git(checkout, &["diff", "--numstat", "-z", &parent, oid])?;
        let (patch, truncated) = worktrees::git_bounded(checkout, &["diff", &parent, oid], CAP)?;
        (names, numstat, patch, truncated)
    } else {
        let names = worktrees::git(
            checkout,
            &[
                "diff-tree",
                "--root",
                "-r",
                "-z",
                "--name-status",
                "--no-commit-id",
                oid,
            ],
        )?;
        let numstat = worktrees::git(
            checkout,
            &[
                "diff-tree",
                "--root",
                "-r",
                "--numstat",
                "-z",
                "--no-commit-id",
                oid,
            ],
        )?;
        let (patch, truncated) = worktrees::git_bounded(
            checkout,
            &["diff-tree", "--root", "-r", "-p", "--no-commit-id", oid],
            CAP,
        )?;
        (names, numstat, patch, truncated)
    };
    let (patch, combined_truncated) = bounded(patch);
    truncated |= combined_truncated;
    let mut files = parse_name_status(&names);
    let counts = parse_numstat(&numstat);
    for file in &mut files {
        if let Some(&(added, removed)) = counts.get(file.path.as_str()) {
            file.added = added;
            file.removed = removed;
        }
    }
    Ok(GitViewDiff {
        label: format!("commit {}", &oid[..8]),
        patch,
        truncated,
        files,
        detail: Some(commit_detail(checkout, oid)?),
    })
}

/// Author email, committer identity, and message body for one commit.
fn commit_detail(checkout: &Path, oid: &str) -> Result<GitViewCommitDetail, BridgeError> {
    let out = worktrees::git(
        checkout,
        &["log", "-1", "--format=%ae%x00%cn%x00%ce%x00%b", oid],
    )?;
    let mut fields = out.splitn(4, |b| *b == 0);
    let field = |bytes: Option<&[u8]>| bytes.map(lossy).unwrap_or_default();
    let author_email = field(fields.next());
    let committer = field(fields.next());
    let committer_email = field(fields.next());
    let body = fields
        .next()
        .map(|body| lossy(body).trim_end_matches(['\r', '\n']).to_owned())
        .unwrap_or_default();
    Ok(GitViewCommitDetail {
        author_email,
        committer,
        committer_email,
        body,
    })
}

/// One page of commits. `branch: None` walks local branches, remote-tracking
/// refs, and tags — never stash or other internal refs. `Some` must be a full
/// `refs/heads/` or `refs/remotes/` ref that resolves to a commit, and walks
/// only the commits reachable from it.
pub fn history(
    checkout: &Path,
    skip: u64,
    branch: Option<&str>,
) -> Result<GitViewHistory, BridgeError> {
    let skip_arg = format!("--skip={skip}");
    let max_arg = format!("--max-count={}", HISTORY_PAGE + 1);
    // `--topo-order` keeps every parent after its children before `--skip` and
    // `--max-count` page the ordered walk; date order can emit a parent first.
    let mut args = vec![
        "log",
        "--format=%H%x00%P%x00%an%x00%at%x00%D%x00%s",
        "--topo-order",
        skip_arg.as_str(),
        max_arg.as_str(),
    ];
    match branch {
        Some(branch) => {
            verify_branch_ref(checkout, branch)?;
            args.push("--end-of-options");
            args.push(branch);
        }
        None => args.extend(["--branches", "--tags", "--remotes"]),
    }
    let out = worktrees::git(checkout, &args)?;
    let head = worktrees::run(checkout, &["rev-parse", "HEAD"], None)?;
    let head_oid = if head.status.success() {
        trim_str(&head.stdout).to_owned()
    } else {
        String::new()
    };
    let mut commits = Vec::new();
    for line in out.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let f: Vec<&[u8]> = line.split(|b| *b == 0).collect();
        if f.len() < 6 {
            continue;
        }
        let id = lossy(f[0]);
        commits.push(GitViewCommit {
            head: id == head_oid,
            id,
            parents: f[1]
                .split(|b| *b == b' ')
                .filter(|p| !p.is_empty())
                .map(lossy)
                .collect(),
            author: lossy(f[2]),
            time: lossy(f[3]).parse::<u64>().unwrap_or(0) * 1000,
            refs: parse_refs(f[4]),
            subject: lossy(f[5]),
        });
    }
    let next_skip = if commits.len() as u64 > HISTORY_PAGE {
        commits.truncate(HISTORY_PAGE as usize);
        Some(skip + HISTORY_PAGE)
    } else {
        None
    };
    Ok(GitViewHistory { commits, next_skip })
}

/// Local and remote-tracking branches for the History filter, in
/// `for-each-ref` order. Remote `*/HEAD` aliases are skipped; `current` marks
/// the branch this checkout has checked out (none when HEAD is detached).
pub fn branches(checkout: &Path) -> Result<Vec<GitViewBranch>, BridgeError> {
    let out = worktrees::git(
        checkout,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    let head = worktrees::run(checkout, &["symbolic-ref", "--quiet", "HEAD"], None)?;
    let current = if head.status.success() {
        Some(trim_str(&head.stdout).to_owned())
    } else {
        None
    };
    let mut list = Vec::new();
    for line in out.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let ref_name = lossy(line);
        let remote = ref_name.starts_with("refs/remotes/");
        if remote && is_remote_head(&ref_name) {
            continue;
        }
        let name = ref_name
            .strip_prefix("refs/heads/")
            .or_else(|| ref_name.strip_prefix("refs/remotes/"))
            .unwrap_or(&ref_name)
            .to_owned();
        list.push(GitViewBranch {
            current: current.as_deref() == Some(ref_name.as_str()),
            remote,
            ref_name,
            name,
        });
    }
    Ok(list)
}

/// `refs/remotes/<remote>/HEAD` aliases point at a remote's default branch;
/// they are not branches the History filter can select.
fn is_remote_head(ref_name: &str) -> bool {
    ref_name
        .strip_prefix("refs/remotes/")
        .is_some_and(|rest| rest.rsplit('/').next() == Some("HEAD"))
}

/// A History branch filter is a trust boundary: only a full ref under
/// `refs/heads/` or `refs/remotes/` that `rev-parse` resolves to a commit is
/// accepted. Anything else is `invalid` before `git log` runs.
fn verify_branch_ref(checkout: &Path, branch: &str) -> Result<(), BridgeError> {
    if !(branch.starts_with("refs/heads/") || branch.starts_with("refs/remotes/"))
        || branch.contains('\0')
        || branch.starts_with('-')
    {
        return Err(invalid(
            "the branch must be a full refs/heads/ or refs/remotes/ ref",
        ));
    }
    // `rev-parse` alone also accepts revision syntax such as `main~1` or
    // `main@{0}`; a valid full ref name is the only thing the filter takes.
    let name = worktrees::run(checkout, &["check-ref-format", branch], None)?;
    if !name.status.success() {
        return Err(invalid(format!("'{branch}' is not a valid ref name")));
    }
    let resolved = worktrees::run(
        checkout,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &format!("{branch}^{{commit}}"),
        ],
        None,
    )?;
    if !resolved.status.success() {
        return Err(invalid(format!(
            "the branch '{branch}' does not resolve to a commit"
        )));
    }
    Ok(())
}

/// Every tracked and untracked (non-ignored) path, sorted, for the Files filter.
pub fn file_list(checkout: &Path) -> Result<Vec<String>, BridgeError> {
    let out = worktrees::git(
        checkout,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )?;
    let mut paths: Vec<String> = out
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(lossy)
        .collect();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// Search tracked and non-ignored untracked text files without changing the checkout.
pub fn search(checkout: &Path, query: &str) -> Result<GitViewSearch, BridgeError> {
    let query = query.trim();
    if query.is_empty() {
        return Err(invalid(
            "the search query must contain at least one character",
        ));
    }
    if query.contains('\0') {
        return Err(invalid("the search query cannot contain a NUL character"));
    }

    let mut args = vec!["grep", "-n", "-I", "-F", "--untracked", "--no-color", "-z"];
    if !query.chars().any(char::is_uppercase) {
        args.push("-i");
    }
    args.extend(["-e", query]);
    let output = worktrees::run_bounded(checkout, &args, SEARCH_OUTPUT_CAP)?;
    if !output.status.success() && !output.truncated {
        if output.status.code() == Some(1) && output.stderr.is_empty() {
            return Ok(GitViewSearch {
                matches: Vec::new(),
                truncated: false,
            });
        }
        return Err(worktrees::git_error(&output.stderr));
    }

    let mut matches = Vec::new();
    let mut truncated = output.truncated;
    let mut remaining = output.stdout.as_slice();
    while !remaining.is_empty() {
        let Some(path_end) = remaining.iter().position(|byte| *byte == 0) else {
            break;
        };
        let path = &remaining[..path_end];
        remaining = &remaining[path_end + 1..];
        let Some(line_end) = remaining.iter().position(|byte| *byte == 0) else {
            break;
        };
        let line = &remaining[..line_end];
        remaining = &remaining[line_end + 1..];
        let (text, next) = if let Some(text_end) = remaining.iter().position(|byte| *byte == b'\n')
        {
            (&remaining[..text_end], &remaining[text_end + 1..])
        } else if output.truncated {
            (remaining, &[][..])
        } else {
            truncated = true;
            break;
        };
        remaining = next;

        let Ok(line) = std::str::from_utf8(line).unwrap_or("").parse::<u64>() else {
            continue;
        };
        if matches.len() == SEARCH_MATCH_CAP {
            truncated = true;
            break;
        }
        let text = lossy(text).chars().take(SEARCH_PREVIEW_CHARS).collect();
        matches.push(GitViewSearchMatch {
            path: lossy(path),
            line,
            text,
        });
    }
    Ok(GitViewSearch { matches, truncated })
}

/// One directory level of tracked files plus non-ignored untracked files.
/// `.git` and ignored paths are never listed. `directory` is checkout-relative
/// (`""` for the root) and cannot escape the checkout.
pub fn tree(checkout: &Path, directory: &str) -> Result<GitViewTree, BridgeError> {
    let prefix = if directory.is_empty() || directory == "." {
        String::new()
    } else {
        let rel = relative_path(directory)?;
        let target = std::fs::canonicalize(checkout.join(&rel))
            .map_err(|_| invalid(format!("the directory {directory} is not accessible")))?;
        if !target.starts_with(checkout) {
            return Err(invalid("the path resolves outside the checkout"));
        }
        if !target.is_dir() {
            return Ok(GitViewTree {
                entries: Vec::new(),
            });
        }
        format!("{}/", rel.to_string_lossy())
    };
    let pathspec = format!(":(glob){}*", escape_glob_path(&prefix));
    let args = ["ls-files", "-z", "--cached", "--others", "--", &pathspec];
    let out = worktrees::git(checkout, &args)?;
    // Ignored names at this level; `--directory` collapses ignored directories to `name/`.
    let ignored_out = worktrees::git(
        checkout,
        &[
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "--",
            &pathspec,
        ],
    )?;
    let ignored: std::collections::BTreeSet<String> = ignored_out
        .split(|b| *b == 0)
        .map(lossy)
        .filter_map(|full| {
            let rel = full
                .strip_prefix(prefix.as_str())?
                .trim_end_matches('/')
                .to_owned();
            (!rel.is_empty() && !rel.contains('/')).then_some(rel)
        })
        .collect();
    let mut entries: BTreeMap<String, GitViewTreeEntry> = BTreeMap::new();
    for record in out.split(|b| *b == 0) {
        if record.is_empty() {
            continue;
        }
        let full = lossy(record);
        let rel = full.strip_prefix(prefix.as_str()).unwrap_or(&full);
        if rel.contains('/') || rel == ".git" {
            continue;
        }
        let (name, kind, path) = (rel, "file", format!("{prefix}{rel}"));
        entries.entry(path.clone()).or_insert(GitViewTreeEntry {
            path,
            name: name.to_owned(),
            kind,
            ignored: ignored.contains(rel),
        });
    }

    let directory_path = checkout.join(&prefix);
    let mut directories = Vec::new();
    for item in std::fs::read_dir(&directory_path).map_err(|error| {
        BridgeError::new(
            Code::Daemon(ErrorCode::Internal),
            format!("cannot read {}: {error}", directory_path.display()),
        )
    })? {
        let item = item.map_err(|error| {
            BridgeError::new(
                Code::Daemon(ErrorCode::Internal),
                format!("cannot read {}: {error}", directory_path.display()),
            )
        })?;
        let name = item.file_name().to_string_lossy().into_owned();
        if name == ".git"
            || !item
                .file_type()
                .map_err(|error| {
                    BridgeError::new(
                        Code::Daemon(ErrorCode::Internal),
                        format!("cannot inspect {}: {error}", item.path().display()),
                    )
                })?
                .is_dir()
        {
            continue;
        }
        let path = format!("{prefix}{name}");
        directories.push((path, name));
    }
    for (path, name) in directories {
        entries.entry(path.clone()).or_insert(GitViewTreeEntry {
            path,
            ignored: ignored.contains(&name),
            name,
            kind: "dir",
        });
    }

    let mut entries: Vec<GitViewTreeEntry> = entries.into_values().collect();
    entries.sort_by(|a, b| a.kind.cmp(b.kind).then_with(|| a.name.cmp(&b.name)));
    Ok(GitViewTree { entries })
}

fn escape_glob_path(path: &str) -> String {
    let mut escaped = String::with_capacity(path.len());
    for character in path.chars() {
        if matches!(character, '\\' | '*' | '?' | '[' | ']') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

/// Bounded read-only preview of one checkout-relative file. The path must stay
/// inside the checkout after symlink resolution; `.git` is never readable.
pub fn file(checkout: &Path, path: &str) -> Result<GitViewFile, BridgeError> {
    let rel = relative_path(path)?;
    let fallback = |kind: &'static str| GitViewFile {
        path: rel.to_string_lossy().into_owned(),
        kind,
        text: None,
        truncated: false,
        size: 0,
    };
    let target = match std::fs::canonicalize(checkout.join(&rel)) {
        Ok(target) => target,
        Err(_) => return Ok(fallback("missing")),
    };
    if !target.starts_with(checkout) {
        return Err(invalid("the path resolves outside the checkout"));
    }
    let size = match std::fs::metadata(&target) {
        Ok(meta) if meta.is_file() => meta.len(),
        _ => return Ok(fallback("unreadable")),
    };
    let mut buf = Vec::new();
    match std::fs::File::open(&target).and_then(|f| f.take(CAP as u64 + 1).read_to_end(&mut buf)) {
        Ok(_) => {}
        Err(_) => return Ok(fallback("unreadable")),
    }
    let truncated = buf.len() > CAP;
    if truncated {
        buf.truncate(CAP);
    }
    let nul = buf.contains(&0);
    let utf8 = std::str::from_utf8(&buf);
    let kind = if nul {
        "binary"
    } else if utf8.is_ok() || (truncated && matches!(utf8, Err(e) if e.error_len().is_none())) {
        // `error_len() == None` is an incomplete char cut by the cap — still text.
        "text"
    } else {
        "nonutf8"
    };
    Ok(GitViewFile {
        path: rel.to_string_lossy().into_owned(),
        kind,
        text: (kind == "text").then(|| String::from_utf8_lossy(&buf).into_owned()),
        truncated,
        size,
    })
}

/// Save a complete UTF-8 text file only when its on-disk contents still match
/// the version that was opened in the editor.
pub fn save_file(
    checkout: &Path,
    path: &str,
    expected_text: &str,
    text: &str,
) -> Result<GitViewFile, BridgeError> {
    let rel = relative_path(path)?;
    if expected_text.len() > CAP || text.len() > CAP || text.contains('\0') {
        return Err(invalid("edited text exceeds the 1 MiB text-file limit"));
    }
    let target = std::fs::canonicalize(checkout.join(&rel))
        .map_err(|_| not_found(format!("the file {} is not available", rel.display())))?;
    if !target.starts_with(checkout) {
        return Err(invalid("the path resolves outside the checkout"));
    }
    let metadata = std::fs::metadata(&target)
        .map_err(|_| not_found(format!("the file {} is not available", rel.display())))?;
    if !metadata.is_file() {
        return Err(invalid("only existing text files can be saved"));
    }
    if metadata.permissions().readonly() {
        return Err(invalid("the file is read-only"));
    }
    let current = current_text(&target)?;
    if current != expected_text.as_bytes() {
        return Err(invalid("the file changed on disk; reload it before saving"));
    }
    atomic_replace(
        &target,
        expected_text.as_bytes(),
        text.as_bytes(),
        metadata.permissions(),
    )?;
    Ok(GitViewFile {
        path: rel.to_string_lossy().into_owned(),
        kind: "text",
        text: Some(text.to_owned()),
        truncated: false,
        size: text.len() as u64,
    })
}

fn current_text(path: &Path) -> Result<Vec<u8>, BridgeError> {
    let mut current = Vec::new();
    match std::fs::File::open(path)
        .and_then(|file| file.take(CAP as u64 + 1).read_to_end(&mut current))
    {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(not_found("the file is no longer available"));
        }
        Err(_) => {
            return Err(BridgeError::new(
                Code::Daemon(ErrorCode::Internal),
                "the file could not be read before saving",
            ));
        }
    }
    if current.len() > CAP || current.contains(&0) || std::str::from_utf8(&current).is_err() {
        return Err(invalid("the file is no longer an editable text file"));
    }
    Ok(current)
}

fn atomic_replace(
    target: &Path,
    expected_text: &[u8],
    text: &[u8],
    permissions: std::fs::Permissions,
) -> Result<(), BridgeError> {
    let parent = target
        .parent()
        .ok_or_else(|| invalid("the file has no checkout parent directory"))?;
    // Write beside the target so replacement stays on the same filesystem.
    let (temporary_path, mut temporary_file) = (0..8)
        .find_map(|_| {
            let sequence = TEMP_FILE_ID.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(".goko-save-{}-{sequence}.tmp", std::process::id()));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => Some(Ok((path, file))),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(error)),
            }
        })
        .transpose()
        .map_err(|error| {
            BridgeError::new(
                Code::Daemon(ErrorCode::Internal),
                format!("could not create a temporary file for saving: {error}"),
            )
        })?
        .ok_or_else(|| {
            BridgeError::new(
                Code::Daemon(ErrorCode::Internal),
                "could not create a temporary file for saving",
            )
        })?;
    let result: std::io::Result<()> = (|| {
        temporary_file.write_all(text)?;
        temporary_file.set_permissions(permissions)?;
        temporary_file.sync_all()?;
        drop(temporary_file);
        Ok(())
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temporary_path);
        return Err(BridgeError::new(
            Code::Daemon(ErrorCode::Internal),
            format!("could not prepare the file for saving: {error}"),
        ));
    }
    let current = match current_text(target) {
        Ok(current) => current,
        Err(error) => {
            let _ = std::fs::remove_file(&temporary_path);
            return Err(error);
        }
    };
    if current != expected_text {
        let _ = std::fs::remove_file(&temporary_path);
        return Err(invalid("the file changed on disk; reload it before saving"));
    }
    std::fs::rename(&temporary_path, target).map_err(|error| {
        let _ = std::fs::remove_file(&temporary_path);
        BridgeError::new(
            Code::Daemon(ErrorCode::Internal),
            format!("could not save the file: {error}"),
        )
    })
}

/// The repository-wide stash stack. `refs/stash` is shared by every linked
/// worktree, so the list is identical from any checkout of the repository.
pub fn stashes(checkout: &Path) -> Result<Vec<GitViewStash>, BridgeError> {
    let exists = worktrees::run(
        checkout,
        &["rev-parse", "--verify", "--quiet", "refs/stash"],
        None,
    )?;
    if !exists.status.success() {
        return Ok(Vec::new());
    }
    let out = worktrees::git(
        checkout,
        &[
            "log",
            "-g",
            "--format=%H%x00%gd%x00%gs%x00%ct",
            "refs/stash",
        ],
    )?;
    let mut list = Vec::new();
    for (position, line) in out.split(|b| *b == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        let f: Vec<&[u8]> = line.split(|b| *b == 0).collect();
        if f.len() < 4 {
            continue;
        }
        let (branch, message) = stash_subject(&lossy(f[2]));
        list.push(GitViewStash {
            index: stash_index(&lossy(f[1])).unwrap_or(position),
            id: lossy(f[0]),
            message,
            branch,
            time: lossy(f[3]).parse::<u64>().unwrap_or(0) * 1000,
        });
    }
    Ok(list)
}

/// Unified diff of one stash entry — index + worktree patch plus the untracked
/// commit (`stash^3`) when the stash was created with `-u`.
pub fn stash_diff(checkout: &Path, stash_id: &str) -> Result<GitViewDiff, BridgeError> {
    let oid = object_id(stash_id)?;
    let entry = stashes(checkout)?
        .into_iter()
        .find(|s| s.id == oid)
        .ok_or_else(|| not_found("the stash is not in the current stack"))?;
    let base = format!("{oid}^");
    let mut files = parse_name_status(&worktrees::git(
        checkout,
        &["diff", "--name-status", "-z", &base, oid],
    )?);
    let (mut patch, mut truncated) = worktrees::git_bounded(checkout, &["diff", &base, oid], CAP)?;
    let third = format!("{oid}^3");
    if worktrees::run(
        checkout,
        &["rev-parse", "--verify", "--quiet", &third],
        None,
    )?
    .status
    .success()
    {
        files.extend(parse_name_status(&worktrees::git(
            checkout,
            &["show", "--name-status", "-z", "--format=", &third],
        )?));
        let (untracked, untracked_truncated) =
            worktrees::git_bounded(checkout, &["show", "--format=", &third], CAP)?;
        truncated |= untracked_truncated;
        if !patch.is_empty() && !untracked.is_empty() {
            patch.extend(b"\n");
        }
        patch.extend(untracked);
    }
    let (patch, combined_truncated) = bounded(patch);
    truncated |= combined_truncated;
    Ok(GitViewDiff {
        label: format!("stash@{{{}}}", entry.index),
        patch,
        truncated,
        files,
        detail: None,
    })
}

// ---------- parsing and validation helpers ----------

pub(crate) fn parse_status(bytes: &[u8]) -> GitViewChanges {
    let records: Vec<&[u8]> = bytes.split(|b| *b == 0).filter(|r| !r.is_empty()).collect();
    let mut view = GitViewChanges {
        branch: None,
        head: String::new(),
        conflicts: Vec::new(),
        staged: Vec::new(),
        unstaged: Vec::new(),
        untracked: Vec::new(),
        ahead: None,
    };
    let mut i = 0;
    while i < records.len() {
        let record = records[i];
        i += 1;
        if record.len() < 3 {
            continue;
        }
        let (x, y) = (record[0], record[1]);
        if x == b'!' && y == b'!' {
            continue;
        }
        let path = lossy(&record[3..]);
        // Rename/copy records carry a second NUL-separated source path.
        let mut orig_path = None;
        if x == b'R' || x == b'C' {
            orig_path = records.get(i).map(|p| lossy(p));
            i += usize::from(orig_path.is_some());
        }
        if x == b'?' && y == b'?' {
            view.untracked.push(entry(path, None, "untracked"));
        } else if is_conflict(x, y) {
            view.conflicts.push(entry(path, orig_path, "unmerged"));
        } else {
            if x != b' ' {
                view.staged
                    .push(entry(path.clone(), orig_path, status_label(x)));
            }
            if y != b' ' {
                view.unstaged.push(entry(path, None, status_label(y)));
            }
        }
    }
    view
}

fn is_conflict(x: u8, y: u8) -> bool {
    // Every unmerged status pair: DD, AU, UA, DU, UD, AA, UU.
    matches!(
        (x, y),
        (b'D', b'D')
            | (b'A', b'U')
            | (b'U', b'A')
            | (b'U', b'D')
            | (b'D', b'U')
            | (b'A', b'A')
            | (b'U', b'U')
    )
}

fn status_label(code: u8) -> &'static str {
    match code {
        b'M' => "modified",
        b'A' => "added",
        b'D' => "deleted",
        b'R' => "renamed",
        b'C' => "copied",
        b'T' => "typechange",
        b'U' => "unmerged",
        _ => "modified",
    }
}

fn entry(path: String, orig_path: Option<String>, status: &'static str) -> StatusEntry {
    StatusEntry {
        path,
        orig_path,
        status,
        added: None,
        removed: None,
    }
}

/// `git diff --name-status -z`: `M\0p\0`, `R100\0old\0new\0`, …
fn parse_name_status(bytes: &[u8]) -> Vec<StatusEntry> {
    let records: Vec<&[u8]> = bytes.split(|b| *b == 0).filter(|r| !r.is_empty()).collect();
    let mut files = Vec::new();
    let mut i = 0;
    while i < records.len() {
        let code = records[i][0];
        i += 1;
        if code == b'R' || code == b'C' {
            let orig_path = records.get(i).map(|p| lossy(p));
            let path = records.get(i + 1).map(|p| lossy(p));
            i += 2;
            if let (Some(orig_path), Some(path)) = (orig_path, path) {
                files.push(entry(path, Some(orig_path), status_label(code)));
            }
        } else if let Some(path) = records.get(i) {
            files.push(entry(lossy(path), None, status_label(code)));
            i += 1;
        }
    }
    files
}

/// `git diff --numstat -z` → per-path `(added, removed)` counts keyed by the
/// path shown in the name-status list. Records are `added\tremoved\tpath\0`;
/// a rename/copy is `added\tremoved\t\0old\0new\0` and its counts attach to
/// the new path. `-\t-` marks a binary file, which has no counts.
fn parse_numstat(bytes: &[u8]) -> BTreeMap<String, (Option<u64>, Option<u64>)> {
    let records: Vec<&[u8]> = bytes.split(|b| *b == 0).collect();
    let mut counts = BTreeMap::new();
    let mut i = 0;
    while i < records.len() {
        let record = records[i];
        i += 1;
        if record.is_empty() {
            continue;
        }
        let mut fields = record.splitn(3, |b| *b == b'\t');
        let added = numstat_count(fields.next().unwrap_or(b""));
        let removed = numstat_count(fields.next().unwrap_or(b""));
        let path = fields.next().unwrap_or(b"");
        if path.is_empty() {
            // Rename/copy: the next two records are the old then the new path.
            if let Some(new) = records.get(i + 1) {
                counts.insert(lossy(new), (added, removed));
            }
            i += 2;
        } else {
            counts.insert(lossy(path), (added, removed));
        }
    }
    counts
}

fn numstat_count(field: &[u8]) -> Option<u64> {
    if field == b"-" {
        return None;
    }
    std::str::from_utf8(field).ok()?.parse().ok()
}

/// `%D` decoration → label list, minus the synthetic `HEAD`/`HEAD -> ` wrapper.
fn parse_refs(decoration: &[u8]) -> Vec<String> {
    lossy(decoration)
        .split(", ")
        .filter_map(|r| match r {
            "" | "HEAD" => None,
            _ => Some(r.strip_prefix("HEAD -> ").unwrap_or(r).to_owned()),
        })
        .collect()
}

/// `WIP on main: abc msg` / `On main: custom` → (`main`, rest). Anything else
/// keeps the whole subject as the message.
fn stash_subject(gs: &str) -> (Option<String>, String) {
    let rest = gs
        .strip_prefix("WIP on ")
        .or_else(|| gs.strip_prefix("On "));
    match rest.and_then(|r| r.split_once(':')) {
        Some((branch, message)) => (Some(branch.trim().to_owned()), message.trim().to_owned()),
        None => (None, gs.to_owned()),
    }
}

fn stash_index(gd: &str) -> Option<usize> {
    gd.strip_prefix("stash@{")?.strip_suffix('}')?.parse().ok()
}

/// Checkout-relative path validation: rejects absolute paths, `..` traversal,
/// and `.git` access. Symlink escapes are caught separately by the canonical
/// containment check in the callers that touch the filesystem.
pub(crate) fn relative_path(path: &str) -> Result<PathBuf, BridgeError> {
    let rel = Path::new(path);
    if path.is_empty() || rel.is_absolute() {
        return Err(invalid("a checkout-relative path is required"));
    }
    let mut clean = PathBuf::new();
    for component in rel.components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            _ => return Err(invalid("the path must stay inside the checkout")),
        }
    }
    if clean.as_os_str().is_empty()
        || clean.components().any(
            |component| matches!(component, Component::Normal(part) if part.to_string_lossy().eq_ignore_ascii_case(".git")),
        )
    {
        return Err(invalid("a non-`.git` checkout-relative path is required"));
    }
    Ok(clean)
}

fn required_path(path: Option<&str>) -> Result<PathBuf, BridgeError> {
    relative_path(path.ok_or_else(|| invalid("a checkout-relative path is required"))?)
}

/// Commit and stash selection must be a full SHA-1/SHA-256 oid, never a ref expression —
/// callers cannot smuggle `HEAD^{tree}` or option-like argv through.
fn object_id(id: &str) -> Result<&str, BridgeError> {
    if matches!(id.len(), 40 | 64) && id.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(id)
    } else {
        Err(invalid("expected a full 40- or 64-hex object id"))
    }
}

fn bounded(out: Vec<u8>) -> (String, bool) {
    let truncated = out.len() > CAP;
    let out = if truncated { &out[..CAP] } else { &out[..] };
    (String::from_utf8_lossy(out).into_owned(), truncated)
}

pub(crate) fn rel_str(rel: &Path) -> Result<&str, BridgeError> {
    rel.to_str()
        .ok_or_else(|| invalid("the path is not valid UTF-8"))
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

pub(crate) fn trim_str(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes)
        .unwrap_or("")
        .trim_end_matches(['\r', '\n'])
}

pub(crate) fn invalid(message: impl Into<String>) -> BridgeError {
    BridgeError::new(Code::Daemon(ErrorCode::InvalidArgument), message.into())
}

fn not_found(message: impl Into<String>) -> BridgeError {
    BridgeError::new(Code::Daemon(ErrorCode::NotFound), message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::fs;
    use std::process::Command;

    fn git_ok(root: &Path, args: &[&str]) -> Vec<u8> {
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .expect("git run");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }

    fn repository(label: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new(label);
        let root = dir.path().canonicalize().unwrap();
        git_ok(&root, &["init", "-b", "main"]);
        git_ok(&root, &["config", "user.name", "Tester"]);
        git_ok(&root, &["config", "user.email", "tester@example.com"]);
        fs::write(root.join("tracked.txt"), "tracked\n").unwrap();
        fs::write(root.join("second.txt"), "second\n").unwrap();
        git_ok(&root, &["add", "-A"]);
        git_ok(&root, &["commit", "-qm", "init"]);
        (dir, root)
    }

    fn commit(root: &Path, name: &str, content: &str) {
        fs::write(root.join(name), content).unwrap();
        git_ok(root, &["add", name]);
        git_ok(root, &["commit", "-qm", "c"]);
    }

    /// `git` with fixed author and committer dates, so log ordering is
    /// deterministic under skewed or equal timestamps.
    fn dated_git(root: &Path, args: &[&str], timestamp: u64) {
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .env("GIT_AUTHOR_DATE", format!("@{timestamp} +0000"))
            .env("GIT_COMMITTER_DATE", format!("@{timestamp} +0000"))
            .output()
            .expect("git run");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn head(root: &Path) -> String {
        lossy(&git_ok(root, &["rev-parse", "HEAD"]))
            .trim_end_matches('\n')
            .to_owned()
    }

    /// Semantic snapshots of the index (`ls-files -s`), worktree state
    /// (porcelain status), and `refs/stash` — none may change under reads.
    fn snapshot(root: &Path) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        (
            git_ok(
                root,
                &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
            ),
            git_ok(root, &["ls-files", "-s"]),
            worktrees::run(
                root,
                &["rev-parse", "--verify", "--quiet", "refs/stash"],
                None,
            )
            .unwrap()
            .stdout,
        )
    }

    fn find<'a>(entries: &'a [StatusEntry], path: &str) -> &'a StatusEntry {
        entries
            .iter()
            .find(|e| e.path == path)
            .unwrap_or_else(|| panic!("missing entry {path}: {entries:?}"))
    }

    #[test]
    fn status_groups_every_layer_and_preserves_unusual_names() {
        let (_t, root) = repository("status");
        // D: a file deleted through the index shows in the staged group.
        commit(&root, "deleted.txt", "gone\n");
        git_ok(&root, &["rm", "-q", "deleted.txt"]);
        // R then M: staged rename plus a worktree change on the new path.
        git_ok(&root, &["mv", "tracked.txt", "new name.txt"]);
        fs::write(root.join("new name.txt"), "tracked\nmore\n").unwrap();
        // A: staged add. M+M: second.txt staged then modified again.
        fs::write(root.join("added.txt"), "added\n").unwrap();
        fs::write(root.join("second.txt"), "second\nstaged\n").unwrap();
        git_ok(&root, &["add", "added.txt", "second.txt"]);
        fs::write(root.join("second.txt"), "second\nstaged\nunstaged\n").unwrap();
        // ?? entries with difficult names.
        for name in ["sp ace.txt", "new\nline.txt", "ñüñ.txt"] {
            fs::write(root.join(name), "u\n").unwrap();
        }

        let view = changes(&root).unwrap();
        assert_eq!(view.branch.as_deref(), Some("main"));
        assert_eq!(view.head.len(), 8);
        assert!(view.conflicts.is_empty());
        let rename = find(&view.staged, "new name.txt");
        assert_eq!(rename.status, "renamed");
        assert_eq!(rename.orig_path.as_deref(), Some("tracked.txt"));
        assert_eq!(find(&view.staged, "added.txt").status, "added");
        assert_eq!(find(&view.staged, "second.txt").status, "modified");
        assert_eq!(find(&view.staged, "deleted.txt").status, "deleted");
        assert_eq!(find(&view.unstaged, "second.txt").status, "modified");
        assert!(view.unstaged.iter().any(|e| e.path == "new name.txt"));
        let names: Vec<&str> = view.untracked.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(names, vec!["new\nline.txt", "sp ace.txt", "ñüñ.txt"]);
    }

    #[test]
    fn conflicts_form_their_own_group() {
        let (_t, root) = repository("conflict");
        fs::write(root.join("f.txt"), "base\n").unwrap();
        git_ok(&root, &["add", "f.txt"]);
        git_ok(&root, &["commit", "-qm", "base"]);
        git_ok(&root, &["checkout", "-qb", "side"]);
        fs::write(root.join("f.txt"), "side\n").unwrap();
        git_ok(&root, &["commit", "-qam", "side"]);
        git_ok(&root, &["checkout", "-q", "main"]);
        fs::write(root.join("f.txt"), "main\n").unwrap();
        git_ok(&root, &["commit", "-qam", "main"]);
        assert!(!Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["merge", "--no-edit", "side"])
            .output()
            .unwrap()
            .status
            .success());

        let view = changes(&root).unwrap();
        assert_eq!(find(&view.conflicts, "f.txt").status, "unmerged");
        assert!(view.staged.is_empty());
        assert!(view.unstaged.is_empty());
        let patch = diff(&root, "conflict", Some("f.txt"), None).unwrap();
        assert_eq!(patch.label, "conflict");
        assert!(patch.patch.contains("<<<<<<<"));
    }

    #[test]
    fn diffs_stay_inside_their_layer() {
        let (_t, root) = repository("layers");
        fs::write(root.join("tracked.txt"), "tracked\nstaged\n").unwrap();
        git_ok(&root, &["add", "tracked.txt"]);
        fs::write(root.join("tracked.txt"), "tracked\nstaged\nunstaged\n").unwrap();
        fs::write(root.join("fresh.txt"), "new\n").unwrap();

        let staged = diff(&root, "staged", Some("tracked.txt"), None).unwrap();
        assert_eq!(staged.label, "HEAD → index");
        assert!(staged.patch.contains("+staged") && !staged.patch.contains("+unstaged"));
        let unstaged = diff(&root, "unstaged", Some("tracked.txt"), None).unwrap();
        assert_eq!(unstaged.label, "index → working tree");
        assert!(unstaged.patch.contains("+unstaged") && !unstaged.patch.contains("+staged"));
        let untracked = diff(&root, "untracked", Some("fresh.txt"), None).unwrap();
        assert_eq!(untracked.label, "new untracked file");
        assert!(untracked.patch.contains("new file mode"));

        // A diff above 1 MiB reports the explicit truncated flag.
        fs::write(root.join("huge.txt"), "x".repeat(CAP + 4096)).unwrap();
        git_ok(&root, &["add", "huge.txt"]);
        let huge = diff(&root, "staged", Some("huge.txt"), None).unwrap();
        assert!(huge.truncated);
        assert!(huge.patch.len() <= CAP);

        fs::write(root.join("huge-untracked.txt"), "y".repeat(CAP + 4096)).unwrap();
        let huge = diff(&root, "untracked", Some("huge-untracked.txt"), None).unwrap();
        assert!(huge.truncated);
        assert!(huge.patch.len() <= CAP);
    }

    #[test]
    fn commit_layer_shows_the_whole_commit_patch() {
        let (_t, root) = repository("commitlayer");
        commit(&root, "topic.txt", "topic\n");
        let oid = head(&root);
        let view = diff(&root, "commit", None, Some(&oid)).unwrap();
        assert_eq!(view.label, format!("commit {}", &oid[..8]));
        assert_eq!(find(&view.files, "topic.txt").status, "added");
        assert!(view.patch.contains("+topic"));
        // Root commit uses the empty tree.
        let root_oid = lossy(&git_ok(&root, &["rev-list", "--max-parents=0", "HEAD"]))
            .trim_end_matches('\n')
            .to_owned();
        let view = diff(&root, "commit", None, Some(&root_oid)).unwrap();
        assert!(!view.files.is_empty() && !view.patch.is_empty());
        // Identity is a full oid — refs and short names reject.
        assert!(diff(&root, "commit", None, Some("HEAD")).is_err());
        let absent = diff(&root, "commit", None, Some(&"f".repeat(40))).unwrap_err();
        assert_eq!(absent.code, Code::Daemon(ErrorCode::NotFound));
    }

    #[test]
    fn diff_inputs_are_validated() {
        let (_t, root) = repository("diffvalidate");
        assert!(object_id(&"a".repeat(64)).is_ok());
        for (layer, path) in [
            ("staged", None),
            ("unstaged", Some("../escape")),
            ("unstaged", Some("/absolute")),
            ("staged", Some(".git/config")),
        ] {
            assert!(diff(&root, layer, path, None).is_err(), "{layer} {path:?}");
        }
        assert!(diff(&root, "wat", Some("x"), None).is_err());
        assert!(diff(&root, "untracked", Some("nope.txt"), None).is_err());
    }

    #[test]
    fn history_lists_refs_parents_and_marks_head() {
        let (_t, root) = repository("history");
        commit(&root, "c.txt", "c\n");
        git_ok(&root, &["tag", "v1"]);
        git_ok(&root, &["branch", "side"]);
        git_ok(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        let oid = head(&root);

        let page = history(&root, 0, None).unwrap();
        assert_eq!(page.commits.len(), 2);
        assert_eq!(page.next_skip, None);
        let top = &page.commits[0];
        assert_eq!(top.id, oid);
        assert!(top.head);
        assert_eq!(top.parents.len(), 1);
        assert!(top.refs.contains(&"main".to_owned()));
        assert!(top.refs.contains(&"tag: v1".to_owned()));
        assert!(top.refs.contains(&"origin/main".to_owned()));
        assert_eq!(top.subject, "c");
        assert!(top.time > 0);
        let first = &page.commits[1];
        assert!(first.parents.is_empty() && !first.head);
    }

    #[test]
    fn history_pages_one_hundred_commits_and_empty_repos() {
        let (_t, root) = repository("pages");
        for i in 0..105 {
            git_ok(
                &root,
                &["commit", "-q", "--allow-empty", "-m", &format!("c{i}")],
            );
        }
        let page0 = history(&root, 0, None).unwrap();
        assert_eq!(page0.commits.len(), 100);
        assert_eq!(page0.next_skip, Some(100));
        let page1 = history(&root, 100, None).unwrap();
        // 105 new commits plus the init commit on the second page.
        assert_eq!(page1.commits.len(), 6);
        assert_eq!(page1.next_skip, None);

        let dir = TempDir::new("empty-history");
        let empty = dir.path().canonicalize().unwrap();
        git_ok(&empty, &["init", "-b", "main"]);
        let page = history(&empty, 0, None).unwrap();
        assert!(page.commits.is_empty() && page.next_skip.is_none());
    }

    #[test]
    fn history_scoped_to_branch() {
        let (_t, root) = repository("branch-scope");
        git_ok(&root, &["checkout", "-qb", "side"]);
        commit(&root, "side.txt", "side\n");
        git_ok(&root, &["checkout", "-q", "main"]);
        commit(&root, "main.txt", "main\n");
        git_ok(
            &root,
            &["update-ref", "refs/remotes/origin/side", "refs/heads/side"],
        );
        let side_tip = lossy(&git_ok(&root, &["rev-parse", "refs/heads/side"]))
            .trim_end_matches('\n')
            .to_owned();
        let main_tip = head(&root);

        // Only commits reachable from the branch are listed; refs still
        // decorate every row and HEAD keeps marking the checkout tip.
        let page = history(&root, 0, Some("refs/heads/side")).unwrap();
        assert_eq!(page.commits.len(), 2);
        assert_eq!(page.commits[0].id, side_tip);
        assert!(!page.commits[0].head);
        assert!(page.commits[1].parents.is_empty());
        assert!(!page.commits.iter().any(|c| c.id == main_tip));

        // Remote-tracking refs scope the walk the same way.
        let remote = history(&root, 0, Some("refs/remotes/origin/side")).unwrap();
        assert_eq!(remote.commits.len(), 2);
        assert_eq!(remote.commits[0].id, side_tip);
    }

    #[test]
    fn history_rejects_unsafe_branch() {
        let (_t, root) = repository("branch-guard");
        git_ok(&root, &["tag", "v1"]);
        for branch in [
            "-x",
            "--all",
            "HEAD",
            "main",
            "refs/tags/v1",
            "refs/heads/missing",
            "refs/heads/",
            "refs/remotes/",
            "refs/heads/bad\0name",
            // Revision syntax is not a ref name even when it resolves to a commit.
            "refs/heads/main~1",
            "refs/heads/main^",
            "refs/heads/main^{tree}",
            "refs/heads/main@{0}",
            "refs/heads/main..refs/heads/main",
            "refs/heads/../tags/v1",
            "refs/heads/with space",
        ] {
            let error = history(&root, 0, Some(branch)).unwrap_err();
            assert_eq!(
                error.code,
                Code::Daemon(ErrorCode::InvalidArgument),
                "branch {branch:?} must be rejected as invalid input"
            );
        }
    }

    /// Plain `git log` orders by commit date, so skewed or equal timestamps
    /// can emit a parent before the merge child that references it; the graph
    /// then leaves a dangling lane. History must keep every loaded parent
    /// after its child, on every page of both scopes.
    #[test]
    fn history_lists_children_before_their_parents() {
        let dir = TempDir::new("history-topo");
        let root = dir.path().canonicalize().unwrap();
        git_ok(&root, &["init", "-b", "main"]);
        git_ok(&root, &["config", "user.name", "Tester"]);
        git_ok(&root, &["config", "user.email", "tester@example.com"]);
        // The root and feature tip carry newer committer dates than the merge,
        // so date order emits parents before their children.
        dated_git(
            &root,
            &["commit", "-q", "--allow-empty", "-m", "init"],
            9_000_000_000,
        );
        git_ok(&root, &["checkout", "-qb", "feature"]);
        dated_git(
            &root,
            &["commit", "-q", "--allow-empty", "-m", "feature"],
            8_000_000_000,
        );
        git_ok(&root, &["checkout", "-q", "main"]);
        for i in 0..105 {
            dated_git(
                &root,
                &["commit", "-q", "--allow-empty", "-m", &format!("main {i}")],
                2_000 + i as u64,
            );
        }
        dated_git(&root, &["merge", "--no-edit", "feature"], 3_000);

        for branch in [None, Some("refs/heads/main")] {
            let mut commits = Vec::new();
            let mut skip = 0;
            loop {
                let page = history(&root, skip, branch).unwrap();
                let next = page.next_skip;
                commits.extend(page.commits);
                match next {
                    Some(next) => skip = next,
                    None => break,
                }
            }
            assert!(commits.len() as u64 > HISTORY_PAGE);
            let position: std::collections::HashMap<&str, usize> = commits
                .iter()
                .enumerate()
                .map(|(index, commit)| (commit.id.as_str(), index))
                .collect();
            for (index, commit) in commits.iter().enumerate() {
                for parent in &commit.parents {
                    if let Some(&parent_index) = position.get(parent.as_str()) {
                        assert!(
                            parent_index > index,
                            "{branch:?}: parent {parent} of {} is listed before its child",
                            commit.id
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn branches_lists_local_remote_and_current() {
        let (_t, root) = repository("branch-list");
        git_ok(&root, &["branch", "feature"]);
        git_ok(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git_ok(&root, &["update-ref", "refs/remotes/origin/dev", "HEAD"]);
        git_ok(
            &root,
            &[
                "update-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );

        let list = branches(&root).unwrap();
        let by_ref: std::collections::BTreeMap<_, _> = list
            .iter()
            .map(|branch| (branch.ref_name.as_str(), branch))
            .collect();
        assert_eq!(list.len(), 4);
        let main = by_ref["refs/heads/main"];
        assert_eq!(main.name, "main");
        assert!(!main.remote && main.current);
        let feature = by_ref["refs/heads/feature"];
        assert_eq!(feature.name, "feature");
        assert!(!feature.remote && !feature.current);
        let remote = by_ref["refs/remotes/origin/main"];
        assert_eq!(remote.name, "origin/main");
        assert!(remote.remote && !remote.current);
        assert!(by_ref.contains_key("refs/remotes/origin/dev"));
        // Remote HEAD aliases are not selectable branches.
        assert!(!list.iter().any(|b| b.ref_name.ends_with("/HEAD")));

        // A detached HEAD marks no branch as current.
        git_ok(&root, &["checkout", "-q", "--detach", "HEAD"]);
        assert!(branches(&root).unwrap().iter().all(|b| !b.current));
    }

    #[test]
    fn commit_detail_has_body_email_and_committer() {
        let (_t, root) = repository("commit-detail");
        fs::write(root.join("detail.txt"), "d\n").unwrap();
        git_ok(&root, &["add", "detail.txt"]);
        git_ok(
            &root,
            &[
                "-c",
                "user.name=Committer",
                "-c",
                "user.email=committer@example.com",
                "commit",
                "--author=Author <author@example.com>",
                "-qm",
                "subject line",
                "-m",
                "body line one",
                "-m",
                "body line two",
            ],
        );
        let oid = head(&root);
        let view = diff(&root, "commit", None, Some(&oid)).unwrap();
        let detail = view.detail.as_ref().expect("commit diffs carry detail");
        assert_eq!(detail.author_email, "author@example.com");
        assert_eq!(detail.committer, "Committer");
        assert_eq!(detail.committer_email, "committer@example.com");
        assert!(detail.body.contains("body line one"));
        assert!(detail.body.contains("body line two"));
        assert!(!detail.body.contains("subject line"));

        // File layers never carry commit detail.
        fs::write(root.join("detail.txt"), "d\nmore\n").unwrap();
        git_ok(&root, &["add", "detail.txt"]);
        let staged = diff(&root, "staged", Some("detail.txt"), None).unwrap();
        assert!(staged.detail.is_none());
    }

    #[test]
    fn numstat_counts_text_binary_rename_and_root() {
        let (_t, root) = repository("numstat");
        fs::write(root.join("tracked.txt"), "tracked\nmore\n").unwrap();
        fs::write(root.join("bin.dat"), [0x00u8, 0x9f, 0x41]).unwrap();
        git_ok(&root, &["mv", "second.txt", "renamed.txt"]);
        git_ok(&root, &["add", "-A"]);
        git_ok(&root, &["commit", "-qm", "change"]);
        let oid = head(&root);

        let view = diff(&root, "commit", None, Some(&oid)).unwrap();
        let tracked = find(&view.files, "tracked.txt");
        assert_eq!(tracked.added, Some(1));
        assert_eq!(tracked.removed, Some(0));
        let binary = find(&view.files, "bin.dat");
        assert_eq!(binary.status, "added");
        assert_eq!(binary.added, None);
        assert_eq!(binary.removed, None);
        // A rename reports its counts against the new path.
        let renamed = find(&view.files, "renamed.txt");
        assert_eq!(renamed.status, "renamed");
        assert_eq!(renamed.orig_path.as_deref(), Some("second.txt"));
        assert_eq!(renamed.added, Some(0));
        assert_eq!(renamed.removed, Some(0));

        // The root commit gets its counts from `diff-tree --root`.
        let root_oid = lossy(&git_ok(&root, &["rev-list", "--max-parents=0", "HEAD"]))
            .trim_end_matches('\n')
            .to_owned();
        let root_view = diff(&root, "commit", None, Some(&root_oid)).unwrap();
        let added = find(&root_view.files, "tracked.txt");
        assert_eq!(added.status, "added");
        assert_eq!(added.added, Some(1));
        assert_eq!(added.removed, Some(0));
    }

    #[test]
    fn file_list_returns_sorted_tracked_and_untracked_paths() {
        let (_t, root) = repository("file-list");
        fs::create_dir_all(root.join("a")).unwrap();
        commit(&root, "b.txt", "b\n");
        commit(&root, "a/z.txt", "z\n");
        fs::write(root.join("loose.txt"), "l\n").unwrap();
        fs::create_dir_all(root.join("ignored")).unwrap();
        fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
        fs::write(root.join("ignored/f.txt"), "i\n").unwrap();

        let paths = file_list(&root).unwrap();
        assert_eq!(
            paths,
            vec![
                ".gitignore",
                "a/z.txt",
                "b.txt",
                "loose.txt",
                "second.txt",
                "tracked.txt"
            ]
        );
    }

    #[test]
    fn tree_lists_one_level_and_excludes_git_but_keeps_ignored() {
        let (_t, root) = repository("tree");
        fs::create_dir_all(root.join("sub/mix")).unwrap();
        commit(&root, "sub/mix/tracked.txt", "t\n");
        fs::create_dir_all(root.join("sub/untir")).unwrap();
        fs::write(root.join("sub/mix/unt.txt"), "u\n").unwrap();
        fs::write(root.join("sub/untir/f.txt"), "u\n").unwrap();
        fs::create_dir_all(root.join("ignored")).unwrap();
        fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
        git_ok(&root, &["add", ".gitignore"]);
        fs::write(root.join("ignored/f.txt"), "i\n").unwrap();
        fs::write(root.join("loose.txt"), "l\n").unwrap();

        let top = tree(&root, "").unwrap();
        let dirs: Vec<&str> = top
            .entries
            .iter()
            .filter(|e| e.kind == "dir")
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(dirs, vec!["ignored", "sub"]);
        let flags: Vec<(&str, bool)> = top
            .entries
            .iter()
            .filter(|e| e.kind == "dir")
            .map(|e| (e.name.as_str(), e.ignored))
            .collect();
        assert_eq!(flags, vec![("ignored", true), ("sub", false)]);
        let files: Vec<&str> = top
            .entries
            .iter()
            .filter(|e| e.kind == "file")
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(
            files,
            vec![".gitignore", "loose.txt", "second.txt", "tracked.txt"]
        );
        assert!(!top.entries.iter().any(|e| e.name == ".git"));

        let sub = tree(&root, "sub").unwrap();
        let names: Vec<(&str, &str)> = sub
            .entries
            .iter()
            .map(|e| (e.name.as_str(), e.kind))
            .collect();
        assert_eq!(names, vec![("mix", "dir"), ("untir", "dir")]);
        let mix = tree(&root, "sub/mix").unwrap();
        let names: Vec<&str> = mix.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["tracked.txt", "unt.txt"]);

        assert!(tree(&root, "../escape").is_err());
        assert!(tree(&root, "/absolute").is_err());
        assert!(tree(&root, ".git").is_err());
        assert!(tree(&root, "missing").is_err());
        assert!(tree(&root, "tracked.txt").unwrap().entries.is_empty());
    }

    #[test]
    fn tree_treats_glob_characters_in_paths_literally() {
        let (_t, root) = repository("tree-glob-path");
        fs::create_dir_all(root.join("folder[1]")).unwrap();
        fs::write(root.join("folder[1]/file*.txt"), "visible\n").unwrap();

        let root_entries = tree(&root, "").unwrap();
        assert!(root_entries
            .entries
            .iter()
            .any(|entry| entry.name == "folder[1]" && entry.kind == "dir"));
        let nested = tree(&root, "folder[1]").unwrap();
        assert_eq!(nested.entries.len(), 1);
        assert_eq!(nested.entries[0].name, "file*.txt");
    }

    #[test]
    fn file_previews_are_bounded_and_flag_fallbacks() {
        let (_t, root) = repository("files");
        commit(&root, "text.txt", "hello\nworld\n");
        let view = file(&root, "text.txt").unwrap();
        assert_eq!(view.kind, "text");
        assert_eq!(view.text.as_deref(), Some("hello\nworld\n"));
        assert!(!view.truncated && view.size == 12);

        fs::write(root.join("big.txt"), "a".repeat(CAP + 64)).unwrap();
        let view = file(&root, "big.txt").unwrap();
        assert!(view.kind == "text" && view.truncated);
        assert_eq!(view.text.unwrap().len(), CAP);

        fs::write(root.join("bin.dat"), [0x00u8, 0x9f, 0x41]).unwrap();
        let view = file(&root, "bin.dat").unwrap();
        assert!(view.kind == "binary" && view.text.is_none());
        fs::write(root.join("enc.dat"), [0xFFu8, 0xFE, 0x41]).unwrap();
        assert_eq!(file(&root, "enc.dat").unwrap().kind, "nonutf8");
        assert_eq!(file(&root, "gone.txt").unwrap().kind, "missing");
        assert_eq!(
            file(&root, ".").unwrap_err().code,
            Code::Daemon(ErrorCode::InvalidArgument)
        );
        fs::create_dir_all(root.join("adir")).unwrap();
        assert_eq!(file(&root, "adir").unwrap().kind, "unreadable");

        for bad in [
            "../x",
            "/etc/passwd",
            ".git/config",
            "nested/.git/config",
            "",
        ] {
            assert!(file(&root, bad).is_err(), "{bad}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn file_and_tree_reject_symlink_escapes() {
        let (_t, root) = repository("escapes");
        let outside = TempDir::new("escape-target");
        fs::write(outside.path().join("secret.txt"), "secret\n").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.txt"), root.join("link.txt"))
            .unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("linkdir")).unwrap();
        assert!(file(&root, "link.txt").is_err());
        assert!(tree(&root, "linkdir").is_err());
        assert!(diff(&root, "untracked", Some("link.txt"), None).is_err());
        // A symlink that stays inside the checkout is readable.
        fs::write(root.join("real.txt"), "inside\n").unwrap();
        std::os::unix::fs::symlink("real.txt", root.join("inner.txt")).unwrap();
        assert_eq!(file(&root, "inner.txt").unwrap().kind, "text");
    }

    #[test]
    fn stash_stack_is_repository_wide_and_patches_include_untracked() {
        let (_t, root) = repository("stash");
        let linked_dir = TempDir::new("stash-linked");
        git_ok(
            &root,
            &[
                "worktree",
                "add",
                "-b",
                "linked",
                linked_dir.path().to_str().unwrap(),
            ],
        );
        let linked = linked_dir.path().canonicalize().unwrap();
        fs::write(root.join("tracked.txt"), "tracked\nstash change\n").unwrap();
        fs::write(root.join("loose.txt"), "untracked stash file\n").unwrap();
        git_ok(&root, &["stash", "push", "-u", "-m", "save it"]);

        let list = stashes(&root).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].index, 0);
        assert_eq!(list[0].branch.as_deref(), Some("main"));
        assert_eq!(list[0].message, "save it");
        // The same stack is visible from the linked worktree.
        let shared = stashes(&linked).unwrap();
        assert_eq!(shared[0].id, list[0].id);

        let view = stash_diff(&linked, &list[0].id).unwrap();
        assert_eq!(view.label, "stash@{0}");
        assert!(!view.truncated);
        assert_eq!(find(&view.files, "tracked.txt").status, "modified");
        assert_eq!(find(&view.files, "loose.txt").status, "added");
        assert!(view.patch.contains("+stash change"));
        assert!(view.patch.contains("+untracked stash file"));

        assert!(stash_diff(&root, "stash@{0}").is_err());
        let missing = stash_diff(&root, &"e".repeat(40)).unwrap_err();
        assert_eq!(missing.code, Code::Daemon(ErrorCode::NotFound));
        assert!(stashes(&root).unwrap().len() == 1);
        // Stash commits are never surfaced in History: `refs/stash` is not a
        // branch, remote-tracking ref, or tag.
        let page = history(&root, 0, None).unwrap();
        assert!(!page.commits.iter().any(|c| c.id == list[0].id));
    }

    #[test]
    fn reads_leave_the_index_worktree_and_stash_untouched() {
        let (_t, root) = repository("readonly");
        fs::write(root.join("tracked.txt"), "tracked\nstash me\n").unwrap();
        fs::write(root.join("stashed-loose.txt"), "loose\n").unwrap();
        git_ok(&root, &["stash", "push", "-u", "-m", "s"]);
        // Dirty every layer again after the stash so each read has content.
        fs::write(root.join("second.txt"), "second\nstaged\n").unwrap();
        git_ok(&root, &["add", "second.txt"]);
        fs::write(root.join("second.txt"), "second\nstaged\nunstaged\n").unwrap();
        fs::write(root.join("loose.txt"), "loose\n").unwrap();

        let before = snapshot(&root);
        changes(&root).unwrap();
        diff(&root, "staged", Some("second.txt"), None).unwrap();
        diff(&root, "unstaged", Some("second.txt"), None).unwrap();
        diff(&root, "untracked", Some("loose.txt"), None).unwrap();
        diff(&root, "commit", None, Some(&head(&root))).unwrap();
        history(&root, 0, None).unwrap();
        tree(&root, "").unwrap();
        search(&root, "second").unwrap();
        file(&root, "second.txt").unwrap();
        let stack = stashes(&root).unwrap();
        stash_diff(&root, &stack[0].id).unwrap();
        assert_eq!(before, snapshot(&root));
    }

    #[test]
    fn search_includes_untracked_text_and_skips_ignored_and_binary_files() {
        let (_t, root) = repository("search-scope");
        fs::write(root.join("tracked.txt"), "needle\n").unwrap();
        fs::write(root.join("untracked.txt"), "needle\n").unwrap();
        fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(root.join("ignored.txt"), "needle\n").unwrap();
        fs::write(root.join("binary.bin"), b"needle\0binary").unwrap();

        let mut paths: Vec<_> = search(&root, "needle")
            .unwrap()
            .matches
            .into_iter()
            .map(|matched| matched.path)
            .collect();
        paths.sort();
        assert_eq!(paths, ["tracked.txt", "untracked.txt"]);
    }

    #[test]
    fn search_is_fixed_string_and_smart_case_and_no_matches_are_empty() {
        let (_t, root) = repository("search-literals");
        fs::write(root.join("cases.txt"), "Foo\nfoo\na.b*\n-n\n").unwrap();
        git_ok(&root, &["add", "cases.txt"]);

        let uppercase = search(&root, "Foo").unwrap();
        assert_eq!(
            uppercase.matches.iter().map(|m| m.line).collect::<Vec<_>>(),
            [1]
        );
        let lowercase = search(&root, "foo").unwrap();
        assert_eq!(
            lowercase.matches.iter().map(|m| m.line).collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(search(&root, "a.b*").unwrap().matches[0].text, "a.b*");
        assert_eq!(search(&root, "-n").unwrap().matches[0].text, "-n");
        assert!(search(&root, "no such text").unwrap().matches.is_empty());
        assert!(search(&root, "  \t ").is_err());
        assert!(search(&root, "bad\0query").is_err());
    }

    #[test]
    fn search_caps_matches_previews_and_output() {
        let (_t, root) = repository("search-caps");
        let lines = (0..600).map(|_| "needle").collect::<Vec<_>>().join("\n");
        fs::write(root.join("many.txt"), lines).unwrap();
        let many = search(&root, "needle").unwrap();
        assert_eq!(many.matches.len(), SEARCH_MATCH_CAP);
        assert!(many.truncated);

        fs::write(
            root.join("many.txt"),
            format!("needle{}\n", "x".repeat(1_000)),
        )
        .unwrap();
        let preview = search(&root, "needle").unwrap();
        assert_eq!(
            preview.matches[0].text.chars().count(),
            SEARCH_PREVIEW_CHARS
        );
        assert!(!preview.truncated);

        fs::write(
            root.join("many.txt"),
            format!("needle{}\n", "x".repeat(300 * 1024)),
        )
        .unwrap();
        let bounded = search(&root, "needle").unwrap();
        assert!(bounded.truncated);
        assert_eq!(bounded.matches.len(), 1);
        assert_eq!(
            bounded.matches[0].text.chars().count(),
            SEARCH_PREVIEW_CHARS
        );
    }

    #[test]
    fn non_git_checkouts_are_rejected() {
        let plain = TempDir::new("not-git");
        assert!(repo_checkout(&plain.path().canonicalize().unwrap()).is_err());
        // A directory inside a repo is a checkout path but not its root.
        let (_t, root) = repository("subdir");
        fs::create_dir_all(root.join("sub")).unwrap();
        assert!(repo_checkout(&root.join("sub").canonicalize().unwrap()).is_err());
        // A bare repository has no worktree.
        let bare = TempDir::new("bare");
        let bare_root = bare.path().canonicalize().unwrap();
        git_ok(&bare_root, &["init", "--bare", "-b", "main"]);
        assert!(repo_checkout(&bare_root).is_err());
    }

    #[test]
    fn ahead_is_none_without_upstream_and_counts_unpushed_commits() {
        let (_t, root) = repository("ahead");
        // No remote and no upstream yet.
        assert_eq!(changes(&root).unwrap().ahead, None);
        let remote_dir = TempDir::new("ahead-remote");
        let bare = remote_dir.path().canonicalize().unwrap();
        git_ok(&bare, &["init", "--bare", "-b", "main"]);
        git_ok(&root, &["remote", "add", "origin", bare.to_str().unwrap()]);
        git_ok(&root, &["push", "-qu", "origin", "HEAD"]);
        assert_eq!(changes(&root).unwrap().ahead, Some(0));
        commit(&root, "one.txt", "1\n");
        commit(&root, "two.txt", "2\n");
        assert_eq!(changes(&root).unwrap().ahead, Some(2));
        // An unborn branch has no HEAD to count from.
        let dir = TempDir::new("ahead-unborn");
        let unborn = dir.path().canonicalize().unwrap();
        git_ok(&unborn, &["init", "-b", "main"]);
        assert_eq!(changes(&unborn).unwrap().ahead, None);
    }
}
