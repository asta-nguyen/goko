//! Git write operations for the workspace Editor: staging, unstaging,
//! discarding worktree changes, committing, and pushing. Like `git_view`,
//! public functions receive a checkout that `projects::git_checkout` already
//! validated; Git runs with separate argv and never through a shell.

use crate::error::{BridgeError, Code};
use crate::git_view;
use crate::worktrees;
use protocol::message::ErrorCode;
use serde::Serialize;
use std::path::Path;
use std::time::Duration;

/// A push that has not finished after two minutes is killed; terminal
/// prompts are disabled so a missing credential fails instead of hanging.
const PUSH_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitCommitResult {
    /// Abbreviated HEAD oid after the commit.
    pub head: String,
    /// Symbolic branch name; `None` on a detached HEAD.
    pub branch: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitPushResult {
    /// Git's own one-line summary of the push.
    pub summary: String,
}

/// Stage changes with `git add -A`: the whole worktree for `paths: None`, or
/// only the given checkout-relative paths. An empty path list is rejected —
/// `git add -A --` with no pathspec would stage everything.
pub fn stage(checkout: &Path, paths: Option<&[String]>) -> Result<(), BridgeError> {
    let mut args = vec!["add".to_owned(), "-A".to_owned()];
    match paths {
        None => {}
        Some(paths) => {
            if paths.is_empty() {
                return Err(git_view::invalid("no paths were given to stage"));
            }
            args.push("--".to_owned());
            for path in paths {
                args.push(path_arg(path)?);
            }
        }
    }
    worktrees::git(checkout, &argv(&args))?;
    Ok(())
}

/// Unstage changes. With a commit on HEAD this is `git restore --staged`;
/// before the first commit the index is emptied with `git rm --cached`,
/// which refuses — surfacing Git's error — when the staged content differs
/// from the worktree. `paths: None` unstages everything.
pub fn unstage(checkout: &Path, paths: Option<&[String]>) -> Result<(), BridgeError> {
    let head = worktrees::run(
        checkout,
        &["rev-parse", "--verify", "--quiet", "HEAD"],
        None,
    )?;
    let mut args: Vec<String> = if head.status.success() {
        vec!["restore".to_owned(), "--staged".to_owned()]
    } else {
        vec![
            "rm".to_owned(),
            "--cached".to_owned(),
            "-q".to_owned(),
            "-r".to_owned(),
        ]
    };
    args.push("--".to_owned());
    match paths {
        None => args.push(".".to_owned()),
        Some(paths) => {
            if paths.is_empty() {
                return Err(git_view::invalid("no paths were given to unstage"));
            }
            for path in paths {
                args.push(path_arg(path)?);
            }
        }
    }
    worktrees::git(checkout, &argv(&args))?;
    Ok(())
}

/// Discard one worktree change. `unstaged` restores the path from the index
/// (`git restore`); `untracked` deletes the file itself — a regular file or
/// a symlink, the link and never its target, and never a directory. The
/// layer must match what the index says about the path.
pub fn discard(checkout: &Path, layer: &str, path: &str) -> Result<(), BridgeError> {
    let rel = git_view::relative_path(path)?;
    let spec = literal(&rel)?;
    match (layer, is_tracked(checkout, &rel)?) {
        ("unstaged", true) => worktrees::git(checkout, &["restore", "--", spec.as_str()]).map(drop),
        ("untracked", false) => remove_untracked(checkout, &rel),
        ("unstaged", false) => Err(git_view::invalid("the path is not tracked by Git")),
        ("untracked", true) => Err(git_view::invalid("the path is tracked by Git")),
        _ => Err(git_view::invalid("unsupported discard layer")),
    }
}

/// Commit the staged changes as `git commit -m <message>` — one argv entry,
/// hooks enabled, never `--no-verify` or `--amend`.
pub fn commit(checkout: &Path, message: &str) -> Result<GitCommitResult, BridgeError> {
    if message.trim().is_empty() {
        return Err(git_view::invalid("the commit message is empty"));
    }
    if message.contains('\0') {
        return Err(git_view::invalid(
            "the commit message cannot contain a NUL character",
        ));
    }
    // Reject the states Git would refuse anyway before hooks ever run.
    let status = worktrees::git(
        checkout,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    let view = git_view::parse_status(&status);
    if !view.conflicts.is_empty() {
        return Err(git_view::invalid(
            "resolve the merge conflicts before committing",
        ));
    }
    if view.staged.is_empty() {
        return Err(git_view::invalid("nothing is staged to commit"));
    }
    worktrees::git(checkout, &["commit", "-m", message])?;
    let head = worktrees::git(checkout, &["rev-parse", "--short=8", "HEAD"])?;
    let branch = worktrees::run(
        checkout,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        None,
    )?;
    Ok(GitCommitResult {
        head: git_view::trim_str(&head).to_owned(),
        branch: branch
            .status
            .success()
            .then(|| git_view::trim_str(&branch.stdout).to_owned()),
    })
}

/// Push the current branch: `git push` when it has an upstream,
/// `git push -u origin HEAD` otherwise — never forced, with terminal
/// prompts disabled and a two-minute deadline.
pub fn push(checkout: &Path) -> Result<GitPushResult, BridgeError> {
    let upstream = worktrees::run(
        checkout,
        &["rev-parse", "--verify", "--quiet", "@{u}"],
        None,
    )?;
    let args: Vec<&str> = if upstream.status.success() {
        vec!["push"]
    } else {
        vec!["push", "-u", "origin", "HEAD"]
    };
    let out = worktrees::run_timed(
        checkout,
        &args,
        &[("GIT_TERMINAL_PROMPT", "0")],
        PUSH_TIMEOUT,
    )?;
    if out.timed_out {
        return Err(BridgeError::new(Code::Timeout, "Push timed out"));
    }
    if !out.status.success() {
        return Err(worktrees::git_error(&out.stderr));
    }
    // `git push` writes its ref-update summary to stderr.
    let mut summary = last_line(&out.stderr);
    if summary.is_empty() {
        summary = last_line(&out.stdout);
    }
    Ok(GitPushResult { summary })
}

/// A validated checkout-relative path as a literal Git pathspec, so glob
/// characters in a file name cannot widen the match.
fn path_arg(path: &str) -> Result<String, BridgeError> {
    literal(&git_view::relative_path(path)?)
}

fn literal(rel: &Path) -> Result<String, BridgeError> {
    let rel = git_view::rel_str(rel)?;
    Ok(format!(":(literal){rel}"))
}

fn argv(args: &[String]) -> Vec<&str> {
    args.iter().map(String::as_str).collect()
}

/// `git ls-files --error-unmatch` exits 0 for an index-tracked path and 1
/// for an untracked one; anything else is a real Git error.
fn is_tracked(checkout: &Path, rel: &Path) -> Result<bool, BridgeError> {
    let spec = literal(rel)?;
    let out = worktrees::run(
        checkout,
        &["ls-files", "--error-unmatch", "--", spec.as_str()],
        None,
    )?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(worktrees::git_error(&out.stderr)),
    }
}

/// Delete one untracked worktree entry. Only a regular file or a symlink
/// directly inside the checkout may go: directories, missing paths, and
/// paths that escape through a symlinked parent are rejected first. The
/// final component is removed as the directory entry itself, so a symlink
/// loses the link, never its target.
fn remove_untracked(checkout: &Path, rel: &Path) -> Result<(), BridgeError> {
    let target = checkout.join(rel);
    let meta = std::fs::symlink_metadata(&target)
        .map_err(|_| git_view::invalid(format!("the path {} does not exist", rel.display())))?;
    let kind = meta.file_type();
    if kind.is_dir() {
        return Err(git_view::invalid("cannot discard a directory"));
    }
    if !kind.is_file() && !kind.is_symlink() {
        return Err(git_view::invalid(
            "only files and symlinks can be discarded",
        ));
    }
    // Intermediate components may be symlinks; the real parent must stay
    // inside the checkout. The last component is never followed.
    let parent = target
        .parent()
        .ok_or_else(|| git_view::invalid("the path has no parent directory"))?;
    let real_parent = std::fs::canonicalize(parent)
        .map_err(|_| git_view::invalid(format!("the path {} is not accessible", rel.display())))?;
    if !real_parent.starts_with(checkout) {
        return Err(git_view::invalid("the path resolves outside the checkout"));
    }
    let name = target
        .file_name()
        .ok_or_else(|| git_view::invalid("the path has no file name"))?;
    std::fs::remove_file(real_parent.join(name)).map_err(|error| {
        BridgeError::new(
            Code::Daemon(ErrorCode::Internal),
            format!("cannot delete {}: {error}", rel.display()),
        )
    })
}

/// The last non-empty line of Git's output — where `git push` writes its
/// ref-update summary.
fn last_line(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_view::{GitViewChanges, StatusEntry};
    use crate::test_support::TempDir;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
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
        fs::write(root.join("third.txt"), "third\n").unwrap();
        git_ok(&root, &["add", "-A"]);
        git_ok(&root, &["commit", "-qm", "init"]);
        (dir, root)
    }

    /// A repository with no commits yet.
    fn unborn_repository(label: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new(label);
        let root = dir.path().canonicalize().unwrap();
        git_ok(&root, &["init", "-b", "main"]);
        git_ok(&root, &["config", "user.name", "Tester"]);
        git_ok(&root, &["config", "user.email", "tester@example.com"]);
        (dir, root)
    }

    /// An empty bare repository usable as `origin`.
    fn bare_remote(label: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new(label);
        let root = dir.path().canonicalize().unwrap();
        git_ok(&root, &["init", "--bare", "-b", "main"]);
        (dir, root)
    }

    fn commit_file(root: &Path, name: &str, content: &str) {
        fs::write(root.join(name), content).unwrap();
        git_ok(root, &["add", name]);
        git_ok(root, &["commit", "-qm", "c"]);
    }

    fn head(root: &Path) -> String {
        String::from_utf8_lossy(&git_ok(root, &["rev-parse", "HEAD"]))
            .trim_end()
            .to_owned()
    }

    fn remote_head(bare: &Path) -> String {
        String::from_utf8_lossy(&git_ok(bare, &["rev-parse", "main"]))
            .trim()
            .to_owned()
    }

    fn selected(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|path| path.to_string()).collect()
    }

    fn changes(root: &Path) -> GitViewChanges {
        crate::git_view::changes(root).unwrap()
    }

    fn names(entries: &[StatusEntry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.path.as_str()).collect()
    }

    fn find<'a>(entries: &'a [StatusEntry], path: &str) -> &'a StatusEntry {
        entries
            .iter()
            .find(|e| e.path == path)
            .unwrap_or_else(|| panic!("missing entry {path}: {entries:?}"))
    }

    #[test]
    fn stage_covers_new_modified_deleted_and_renamed_paths() {
        let (_t, root) = repository("stage-files");
        fs::write(root.join("new.txt"), "new\n").unwrap();
        fs::write(root.join("tracked.txt"), "tracked\nmore\n").unwrap();
        fs::remove_file(root.join("second.txt")).unwrap();
        fs::rename(root.join("third.txt"), root.join("third new.txt")).unwrap();

        stage(&root, Some(&selected(&["new.txt"]))).unwrap();
        stage(&root, Some(&selected(&["tracked.txt"]))).unwrap();
        stage(&root, Some(&selected(&["second.txt"]))).unwrap();
        stage(&root, Some(&selected(&["third.txt", "third new.txt"]))).unwrap();

        let view = changes(&root);
        assert!(view.untracked.is_empty());
        assert!(view.unstaged.is_empty());
        assert_eq!(find(&view.staged, "new.txt").status, "added");
        assert_eq!(find(&view.staged, "tracked.txt").status, "modified");
        assert_eq!(find(&view.staged, "second.txt").status, "deleted");
        let renamed = find(&view.staged, "third new.txt");
        assert_eq!(renamed.status, "renamed");
        assert_eq!(renamed.orig_path.as_deref(), Some("third.txt"));

        // An empty selection would stage everything, so it is rejected.
        assert!(stage(&root, Some(&[])).is_err());
        assert!(stage(&root, Some(&selected(&["../escape"]))).is_err());
        assert!(stage(&root, Some(&selected(&[".git/config"]))).is_err());
    }

    #[test]
    fn unstage_restores_index_entries_selectively_and_all() {
        let (_t, root) = repository("unstage-head");
        fs::write(root.join("tracked.txt"), "tracked\nstaged\n").unwrap();
        fs::write(root.join("second.txt"), "second\nstaged\n").unwrap();
        stage(&root, None).unwrap();
        assert_eq!(names(&changes(&root).staged), ["second.txt", "tracked.txt"]);

        unstage(&root, Some(&selected(&["tracked.txt"]))).unwrap();
        let view = changes(&root);
        assert_eq!(names(&view.staged), ["second.txt"]);
        assert_eq!(names(&view.unstaged), ["tracked.txt"]);

        unstage(&root, None).unwrap();
        let view = changes(&root);
        assert!(view.staged.is_empty());
        assert_eq!(names(&view.unstaged), ["second.txt", "tracked.txt"]);
        assert!(unstage(&root, Some(&[])).is_err());
    }

    #[test]
    fn unstage_before_the_first_commit_unstages_from_the_index() {
        let (_t, root) = unborn_repository("unstage-unborn");
        fs::write(root.join("a.txt"), "a\n").unwrap();
        fs::write(root.join("b.txt"), "b\n").unwrap();
        stage(&root, None).unwrap();
        assert_eq!(names(&changes(&root).staged), ["a.txt", "b.txt"]);

        unstage(&root, Some(&selected(&["a.txt"]))).unwrap();
        let view = changes(&root);
        assert_eq!(names(&view.staged), ["b.txt"]);
        assert!(names(&view.untracked).contains(&"a.txt"));

        unstage(&root, None).unwrap();
        let view = changes(&root);
        assert!(view.staged.is_empty());
        assert_eq!(names(&view.untracked), ["a.txt", "b.txt"]);
    }

    #[test]
    fn unstage_before_the_first_commit_surfaces_git_refusals() {
        let (_t, root) = unborn_repository("unstage-refused");
        fs::write(root.join("a.txt"), "a\n").unwrap();
        stage(&root, None).unwrap();
        // The staged content differs from the worktree: `git rm --cached`
        // refuses and its error is surfaced.
        fs::write(root.join("a.txt"), "a\nchanged\n").unwrap();
        assert!(unstage(&root, Some(&selected(&["a.txt"]))).is_err());
        assert_eq!(names(&changes(&root).staged), ["a.txt"]);
    }

    #[test]
    fn discard_restores_the_worktree_to_the_index() {
        let (_t, root) = repository("discard-unstaged");
        fs::write(root.join("tracked.txt"), "tracked\nstaged\n").unwrap();
        stage(&root, Some(&selected(&["tracked.txt"]))).unwrap();
        fs::write(root.join("tracked.txt"), "tracked\nstaged\nunstaged\n").unwrap();

        discard(&root, "unstaged", "tracked.txt").unwrap();
        assert_eq!(
            fs::read_to_string(root.join("tracked.txt")).unwrap(),
            "tracked\nstaged\n"
        );
        // The staged entry is untouched.
        assert_eq!(names(&changes(&root).staged), ["tracked.txt"]);
    }

    #[test]
    #[cfg(unix)]
    fn discard_deletes_untracked_files_and_symlinks_not_targets() {
        let (_t, root) = repository("discard-untracked");
        fs::write(root.join("loose.txt"), "loose\n").unwrap();
        fs::write(root.join("real.txt"), "real\n").unwrap();
        let outside = TempDir::new("discard-target");
        let secret = outside.path().join("secret.txt");
        fs::write(&secret, "secret\n").unwrap();
        std::os::unix::fs::symlink(&secret, root.join("outside-link.txt")).unwrap();
        std::os::unix::fs::symlink("real.txt", root.join("inner-link.txt")).unwrap();

        discard(&root, "untracked", "loose.txt").unwrap();
        discard(&root, "untracked", "outside-link.txt").unwrap();
        discard(&root, "untracked", "inner-link.txt").unwrap();

        assert!(!root.join("loose.txt").exists());
        assert!(fs::symlink_metadata(root.join("outside-link.txt")).is_err());
        assert!(fs::symlink_metadata(root.join("inner-link.txt")).is_err());
        // The symlink targets survive: only the links were removed.
        assert_eq!(fs::read_to_string(&secret).unwrap(), "secret\n");
        assert_eq!(fs::read_to_string(root.join("real.txt")).unwrap(), "real\n");
    }

    #[test]
    fn discard_rejects_directories_missing_paths_and_escapes() {
        let (_t, root) = repository("discard-reject");
        fs::create_dir_all(root.join("adir/nested")).unwrap();
        fs::write(root.join("adir/nested/f.txt"), "x\n").unwrap();
        fs::write(root.join("u.txt"), "u\n").unwrap();

        for path in [
            "adir",
            "adir/nested",
            "missing.txt",
            "../escape",
            ".git/config",
            "/etc/passwd",
            "",
        ] {
            assert!(discard(&root, "untracked", path).is_err(), "{path}");
        }
        assert!(root.join("adir/nested/f.txt").exists());
        // The layer must match what the index says about the path.
        assert!(discard(&root, "unstaged", "u.txt").is_err());
        assert!(discard(&root, "untracked", "tracked.txt").is_err());
        assert!(discard(&root, "wat", "u.txt").is_err());
    }

    #[test]
    #[cfg(unix)]
    fn discard_rejects_paths_escaping_through_symlinked_parents() {
        let (_t, root) = repository("discard-escape");
        let outside = TempDir::new("discard-escape-target");
        let target = outside.path().join("f.txt");
        fs::write(&target, "outside\n").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("linkdir")).unwrap();

        assert!(discard(&root, "untracked", "linkdir/f.txt").is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "outside\n");
    }

    #[test]
    fn commit_returns_the_new_head_and_branch() {
        let (_t, root) = repository("commit-ok");
        fs::write(root.join("new.txt"), "new\n").unwrap();
        stage(&root, Some(&selected(&["new.txt"]))).unwrap();

        let result = commit(&root, "add the new file").unwrap();
        assert_eq!(result.branch.as_deref(), Some("main"));
        assert_eq!(result.head.len(), 8);
        assert!(head(&root).starts_with(result.head.as_str()));
        let subject = git_ok(&root, &["log", "-1", "--format=%s"]);
        assert_eq!(String::from_utf8_lossy(&subject).trim(), "add the new file");
        assert!(changes(&root).staged.is_empty());
    }

    #[test]
    fn commit_rejects_bad_messages_and_uncommittable_states() {
        let (_t, root) = repository("commit-reject");
        let before = head(&root);
        for message in ["", "  \n\t ", "bad\0message"] {
            assert!(commit(&root, message).is_err(), "{message:?}");
        }
        // Untracked files alone are not committable.
        fs::write(root.join("loose.txt"), "loose\n").unwrap();
        let error = commit(&root, "nothing staged").unwrap_err();
        assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
        assert_eq!(head(&root), before);
    }

    #[test]
    fn commit_rejects_unresolved_conflicts() {
        let (_t, root) = repository("commit-conflict");
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
        // Even with another file staged, conflicts block the commit.
        fs::write(root.join("other.txt"), "o\n").unwrap();
        stage(&root, Some(&selected(&["other.txt"]))).unwrap();
        let error = commit(&root, "during a conflict").unwrap_err();
        assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
    }

    #[test]
    #[cfg(unix)]
    fn commit_runs_hooks_and_surfaces_their_failure() {
        let (_t, root) = repository("commit-hook");
        let hook = root.join(".git/hooks/pre-commit");
        fs::write(&hook, "#!/bin/sh\necho 'hook says no' >&2\nexit 1\n").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(root.join("new.txt"), "new\n").unwrap();
        stage(&root, Some(&selected(&["new.txt"]))).unwrap();
        let before = head(&root);

        let error = commit(&root, "blocked by the hook").unwrap_err();
        assert!(error.message.contains("hook says no"), "{}", error.message);
        // The index and HEAD are unchanged.
        assert_eq!(names(&changes(&root).staged), ["new.txt"]);
        assert_eq!(head(&root), before);
    }

    #[test]
    fn push_without_upstream_pushes_and_sets_it() {
        let (_t, root) = repository("push-new");
        let (_remote, bare) = bare_remote("push-new-remote");
        git_ok(&root, &["remote", "add", "origin", bare.to_str().unwrap()]);
        commit_file(&root, "work.txt", "work\n");
        assert_eq!(changes(&root).ahead, None);

        let result = push(&root).unwrap();
        assert!(!result.summary.is_empty());
        assert_eq!(changes(&root).ahead, Some(0));
        assert_eq!(remote_head(&bare), head(&root));
    }

    #[test]
    fn push_with_upstream_updates_the_remote() {
        let (_t, root) = repository("push-upstream");
        let (_remote, bare) = bare_remote("push-upstream-remote");
        git_ok(&root, &["remote", "add", "origin", bare.to_str().unwrap()]);
        push(&root).unwrap();
        commit_file(&root, "more.txt", "more\n");
        assert_eq!(changes(&root).ahead, Some(1));

        let result = push(&root).unwrap();
        assert!(result.summary.contains("main"), "{}", result.summary);
        assert_eq!(changes(&root).ahead, Some(0));
        assert_eq!(remote_head(&bare), head(&root));
    }

    #[test]
    fn push_rejects_non_fast_forward_without_forcing() {
        let (_t, root) = repository("push-nonff");
        let (_remote, bare) = bare_remote("push-nonff-remote");
        git_ok(&root, &["remote", "add", "origin", bare.to_str().unwrap()]);
        push(&root).unwrap();
        commit_file(&root, "theirs.txt", "theirs\n");
        push(&root).unwrap();
        let pushed = remote_head(&bare);

        // Rewrite local history so the push cannot fast-forward; no force
        // flag is ever passed, so Git must reject it.
        git_ok(&root, &["reset", "--hard", "HEAD~1"]);
        commit_file(&root, "ours.txt", "ours\n");
        let error = push(&root).unwrap_err();
        assert!(
            error.message.contains("non-fast-forward"),
            "{}",
            error.message
        );
        assert_eq!(remote_head(&bare), pushed);
    }

    #[test]
    fn push_without_an_origin_remote_fails() {
        let (_t, root) = repository("push-noremote");
        let error = push(&root).unwrap_err();
        assert!(error.message.contains("origin"), "{}", error.message);
    }
}
