//! Git worktree discovery for registered projects.

use crate::error::{BridgeError, Code};
use protocol::message::ErrorCode;
use serde::Serialize;
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WorktreeInfo {
    pub name: String,
    pub path: String,
    pub branch: Option<String>,
    pub head: String,
    pub is_root: bool,
    pub missing: bool,
    pub dirty: Option<bool>,
    pub changed: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WorktreeList {
    pub repo: bool,
    pub worktrees: Vec<WorktreeInfo>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BranchInfo {
    pub name: String,
    pub checked_out_at: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BaseRef {
    pub name: String,
    pub kind: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WorktreeBranches {
    pub branches: Vec<BranchInfo>,
    pub bases: Vec<BaseRef>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EnvFileCandidate {
    pub name: String,
    pub eligible: bool,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EnvCopyFailure {
    pub name: String,
    pub error: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EnvCopyReport {
    pub copied: Vec<String>,
    pub skipped: Vec<String>,
    pub failed: Vec<EnvCopyFailure>,
}

/// Run Git with a fixed executable and separate arguments. This is shared by the
/// read-only commands and the later create/remove commands.
pub fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, BridgeError> {
    let output = run(root, args, None)?;
    if !output.status.success() {
        return Err(git_error(&output.stderr));
    }
    Ok(output.stdout)
}

pub(crate) fn run(root: &Path, args: &[&str], path: Option<&OsStr>) -> Result<Output, BridgeError> {
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args(args);
    if let Some(path) = path {
        command.env("PATH", path);
    }
    command.output().map_err(run_error)
}

pub(crate) struct TimedOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// `true` when the deadline killed the child before it exited on its own.
    pub timed_out: bool,
}

/// `run` for commands that can block on a network or a prompt: extra
/// environment variables, both pipes drained on reader threads so a full
/// pipe cannot deadlock the poll loop, and a wall-clock deadline after which
/// the child is killed.
pub(crate) fn run_timed(
    root: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
    timeout: Duration,
) -> Result<TimedOutput, BridgeError> {
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args(args);
    for &(key, value) in envs {
        command.env(key, value);
    }
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(run_error)?;
    let stdout = child.stdout.take().expect("piped child stdout");
    let stderr = child.stderr.take().expect("piped child stderr");
    let stdout_reader = thread::spawn(move || read_all(stdout));
    let stderr_reader = thread::spawn(move || read_all(stderr));

    let deadline = Instant::now() + timeout;
    let (status, timed_out) = loop {
        match child.try_wait().map_err(run_error)? {
            Some(status) => break (status, false),
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                break (child.wait().map_err(run_error)?, true);
            }
            None => thread::sleep(Duration::from_millis(20)),
        }
    };
    let stdout = join_pipe(stdout_reader)?;
    let stderr = join_pipe(stderr_reader)?;
    Ok(TimedOutput {
        status,
        stdout,
        stderr,
        timed_out,
    })
}

fn read_all<R: Read + Send + 'static>(mut pipe: R) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    pipe.read_to_end(&mut buf)?;
    Ok(buf)
}

fn join_pipe(handle: thread::JoinHandle<io::Result<Vec<u8>>>) -> Result<Vec<u8>, BridgeError> {
    handle
        .join()
        .map_err(|_| {
            BridgeError::new(Code::Daemon(ErrorCode::Internal), "cannot read Git output.")
        })?
        .map_err(run_error)
}

pub(crate) struct BoundedOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub truncated: bool,
}

pub(crate) fn git_bounded(
    root: &Path,
    args: &[&str],
    limit: usize,
) -> Result<(Vec<u8>, bool), BridgeError> {
    let output = run_bounded(root, args, limit)?;
    if !output.status.success() && !output.truncated {
        return Err(git_error(&output.stderr));
    }
    Ok((output.stdout, output.truncated))
}

pub(crate) fn run_bounded(
    root: &Path,
    args: &[&str],
    limit: usize,
) -> Result<BoundedOutput, BridgeError> {
    const STDERR_LIMIT: usize = 8 * 1024;
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(run_error)?;
    let stderr_reader = child.stderr.take().expect("piped child stderr");
    let stderr_reader = thread::spawn(move || -> io::Result<Vec<u8>> {
        let mut stderr_reader = stderr_reader;
        let mut stderr = Vec::new();
        (&mut stderr_reader)
            .take(STDERR_LIMIT as u64)
            .read_to_end(&mut stderr)?;
        io::copy(&mut stderr_reader, &mut io::sink())?;
        Ok(stderr)
    });
    let mut stdout_reader = child.stdout.take().expect("piped child stdout");
    let mut stdout = Vec::with_capacity(limit.min(64 * 1024));
    let stdout_read = (&mut stdout_reader)
        .take(limit as u64 + 1)
        .read_to_end(&mut stdout);
    let truncated = stdout.len() > limit;
    drop(stdout_reader);
    if truncated || stdout_read.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(run_error)?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| {
            BridgeError::new(
                Code::Daemon(ErrorCode::Internal),
                "cannot read Git error output.",
            )
        })?
        .map_err(run_error)?;
    stdout_read.map_err(run_error)?;
    if truncated {
        stdout.truncate(limit);
    }
    Ok(BoundedOutput {
        status,
        stdout,
        stderr,
        truncated,
    })
}

fn run_error(error: io::Error) -> BridgeError {
    if error.kind() == io::ErrorKind::NotFound {
        BridgeError::new(
            Code::Daemon(ErrorCode::InvalidArgument),
            "Git is required for repository operations.",
        )
        .with_hint("Install Git and make sure `git` is available on PATH.")
    } else {
        BridgeError::new(
            Code::Daemon(ErrorCode::Internal),
            format!("cannot run Git: {error}"),
        )
    }
}

pub(crate) fn git_error(stderr: &[u8]) -> BridgeError {
    let message = String::from_utf8_lossy(stderr).trim().to_owned();
    BridgeError::new(
        Code::Daemon(ErrorCode::Internal),
        if message.is_empty() {
            "Git command failed.".to_owned()
        } else {
            message
        },
    )
}

/// A project exposes worktree controls only when its registered path is exactly a
/// non-bare Git worktree root. A normal directory or a repository subdirectory is
/// a valid project with `repo: false`.
pub fn list(root: &Path) -> Result<WorktreeList, BridgeError> {
    let Some((canonical_root, mut worktrees)) = listed_worktrees(root)? else {
        return Ok(WorktreeList {
            repo: false,
            worktrees: Vec::new(),
        });
    };

    for item in &mut worktrees {
        let path = PathBuf::from(&item.path);
        item.is_root = path.canonicalize().is_ok_and(|path| path == canonical_root);
        item.missing = !path.is_dir();
        if item.missing {
            item.dirty = None;
            item.changed = None;
        } else {
            let status = git(
                &path,
                &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
            )?;
            let changed = status_count(&status);
            item.dirty = Some(changed > 0);
            item.changed = Some(changed);
        }
    }
    Ok(WorktreeList {
        repo: true,
        worktrees,
    })
}

pub fn contains_checkout(root: &Path, checkout: &Path) -> Result<bool, BridgeError> {
    let checkout = std::fs::canonicalize(checkout)
        .map_err(|error| invalid(format!("the checkout path is unavailable: {error}")))?;
    let Some((_, worktrees)) = listed_worktrees(root)? else {
        return Ok(false);
    };
    Ok(worktrees.iter().any(|worktree| {
        Path::new(&worktree.path)
            .canonicalize()
            .is_ok_and(|path| path == checkout)
    }))
}

fn listed_worktrees(root: &Path) -> Result<Option<(PathBuf, Vec<WorktreeInfo>)>, BridgeError> {
    let canonical_root = std::fs::canonicalize(root).map_err(|error| {
        invalid(format!(
            "the project root {} is unavailable: {error}",
            root.display()
        ))
    })?;
    if !canonical_root.is_dir() {
        return Err(invalid(format!("{} is not a directory", root.display())));
    }

    let top = run(root, &["rev-parse", "--show-toplevel"], None)?;
    if !top.status.success() {
        return Ok(None);
    }
    let top = trim_line_ending(&top.stdout);
    let Ok(top) = std::str::from_utf8(top) else {
        return Ok(None);
    };
    let Ok(top) = std::fs::canonicalize(top) else {
        return Ok(None);
    };
    if canonical_root != top {
        return Ok(None);
    }

    let bare = run(root, &["rev-parse", "--is-bare-repository"], None)?;
    if !bare.status.success() || trim_line_ending(&bare.stdout) != b"false" {
        return Ok(None);
    }

    let records = parse_worktree_list(
        &git(root, &["worktree", "list", "--porcelain", "-z"])?,
        &canonical_root,
    );
    Ok(Some((canonical_root, records)))
}

pub fn branches(root: &Path) -> Result<WorktreeBranches, BridgeError> {
    let listed = list(root)?;
    if !listed.repo {
        return Ok(WorktreeBranches {
            branches: Vec::new(),
            bases: Vec::new(),
        });
    }
    let mut checked_out = std::collections::HashMap::new();
    for worktree in &listed.worktrees {
        if let Some(branch) = &worktree.branch {
            checked_out.insert(branch.clone(), worktree.path.clone());
        }
    }

    let local_names = ref_names(root, "refs/heads")?;
    let remote_names = ref_names(root, "refs/remotes")?;
    let branches = local_names
        .iter()
        .map(|name| BranchInfo {
            name: name.clone(),
            checked_out_at: checked_out.get(name).cloned(),
        })
        .collect();
    let bases = local_names
        .into_iter()
        .map(|name| BaseRef {
            name,
            kind: "local".into(),
        })
        .chain(remote_names.into_iter().map(|name| BaseRef {
            name,
            kind: "remote".into(),
        }))
        .collect();
    Ok(WorktreeBranches { branches, bases })
}

fn ref_names(root: &Path, namespace: &str) -> Result<Vec<String>, BridgeError> {
    let namespace_arg = namespace;
    let output = git(
        root,
        &["for-each-ref", "--format=%(refname:short)", namespace_arg],
    )?;
    Ok(String::from_utf8_lossy(&output)
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

pub fn create(
    root: &Path,
    path: &str,
    mode: &str,
    branch: &str,
    base: Option<&str>,
) -> Result<WorktreeInfo, BridgeError> {
    let listed = list(root)?;
    if !listed.repo {
        return Err(invalid("the project root is not a Git worktree"));
    }
    let root = fs::canonicalize(root).map_err(|error| invalid(error.to_string()))?;
    let target = lexical_absolute(&root, Path::new(path));
    let name = target
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| invalid("the worktree path must end in a valid directory name"))?;
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err(invalid("the worktree name must be a single path component"));
    }
    if listed
        .worktrees
        .iter()
        .any(|worktree| worktree.name == name)
    {
        return Err(invalid(format!("a checkout named '{name}' already exists")));
    }

    validate_target(&target)?;
    let normalized_target = normalize_path(&target)?;
    check_no_overlap(&listed.worktrees, &normalized_target)?;

    let refs = branches(&root)?;
    let local_names: Vec<&str> = refs
        .branches
        .iter()
        .map(|item| item.name.as_str())
        .collect();
    let mut args = Vec::new();
    match mode {
        "new" => {
            let checked = run(&root, &["check-ref-format", "--branch", branch], None)?;
            let checked_name = trim_line_ending(&checked.stdout);
            if !checked.status.success() || checked_name != branch.as_bytes() {
                let reason = String::from_utf8_lossy(&checked.stderr).trim().to_owned();
                return Err(invalid(if reason.is_empty() {
                    "the branch name is invalid".to_owned()
                } else {
                    format!("the branch name is invalid: {reason}")
                }));
            }
            if local_names.contains(&branch) {
                return Err(invalid(format!("branch '{branch}' already exists")));
            }
            args.extend(["worktree", "add", "-b", branch]);
        }
        "existing" => {
            if !local_names.contains(&branch) {
                return Err(invalid(format!("local branch '{branch}' does not exist")));
            }
            if let Some(checked_out_at) = refs
                .branches
                .iter()
                .find(|item| item.name == branch)
                .and_then(|item| item.checked_out_at.as_deref())
            {
                return Err(invalid(format!(
                    "branch '{branch}' is already checked out at {checked_out_at}"
                )));
            }
            args.extend(["worktree", "add"]);
        }
        _ => return Err(invalid("branch mode must be 'new' or 'existing'")),
    }

    let parent = target
        .parent()
        .ok_or_else(|| invalid("the worktree path must have a parent directory"))?;
    normalize_path(parent)?;
    fs::create_dir_all(parent).map_err(|error| {
        invalid(format!(
            "cannot create the worktree parent directory: {error}"
        ))
    })?;
    let canonical_parent = fs::canonicalize(parent).map_err(|error| {
        invalid(format!(
            "cannot resolve the worktree parent directory: {error}"
        ))
    })?;
    let target = canonical_parent.join(name);
    validate_target(&target)?;
    check_no_overlap(&listed.worktrees, &target)?;

    let target_arg = target
        .to_str()
        .ok_or_else(|| invalid("the worktree path must be valid UTF-8"))?;
    args.push(target_arg);
    if mode == "existing" {
        args.push(branch);
    }
    let mut base_arg = None;
    if mode == "new" {
        if let Some(base) = base.filter(|base| !base.is_empty()) {
            let reference = refs.bases.iter().find(|item| item.name == base);
            let is_oid =
                (7..=64).contains(&base.len()) && base.bytes().all(|byte| byte.is_ascii_hexdigit());
            if reference.is_none() && !is_oid {
                return Err(invalid(format!(
                    "base '{base}' is not an available ref or commit"
                )));
            }
            let commitish = format!("{base}^{{commit}}");
            let check = run(
                &root,
                &["rev-parse", "--verify", "--end-of-options", &commitish],
                None,
            )?;
            if !check.status.success() {
                return Err(invalid(format!("base '{base}' is not a commit")));
            }
            base_arg = Some(match reference {
                Some(reference) if reference.kind == "local" => format!("refs/heads/{base}"),
                Some(_) => format!("refs/remotes/{base}"),
                None => base.to_owned(),
            });
        }
    }
    if let Some(base_arg) = &base_arg {
        args.push(base_arg);
    }
    git(&root, &args)?;
    let created = list(&root)?;
    created
        .worktrees
        .into_iter()
        .find(|worktree| {
            normalize_path(Path::new(&worktree.path)).ok().as_deref() == Some(target.as_path())
        })
        .ok_or_else(|| {
            BridgeError::new(
                Code::Daemon(ErrorCode::Internal),
                "Git created the worktree but it was not returned by git worktree list",
            )
        })
}

fn check_no_overlap(worktrees: &[WorktreeInfo], target: &Path) -> Result<(), BridgeError> {
    for worktree in worktrees {
        let checkout = normalize_path(Path::new(&worktree.path))?;
        if target == checkout || target.starts_with(&checkout) || checkout.starts_with(target) {
            return Err(invalid(format!(
                "the worktree path overlaps the existing checkout at {}",
                worktree.path
            )));
        }
    }
    Ok(())
}

fn validate_target(target: &Path) -> Result<(), BridgeError> {
    match fs::symlink_metadata(target) {
        Ok(metadata) if !metadata.file_type().is_dir() => Err(invalid(
            "the worktree path must be absent or an empty directory",
        )),
        Ok(_)
            if fs::read_dir(target)
                .map_err(|error| invalid(format!("cannot read the worktree path: {error}")))?
                .next()
                .is_some() =>
        {
            Err(invalid("the worktree path must be empty"))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(invalid(format!(
            "cannot inspect the worktree path: {error}"
        ))),
    }
}

/// Remove a linked worktree: `git worktree remove <path>`. The project's
/// own root checkout is never removed, and a checkout with uncommitted
/// work needs `force` — the caller's decision.
pub fn remove(root: &Path, path: &str, force: bool) -> Result<(), BridgeError> {
    let listed = list(root)?;
    if !listed.repo {
        return Err(invalid("the project root is not a Git worktree"));
    }
    let target = normalize_path(&lexical_absolute(root, Path::new(path)))?;
    let mut matched = None;
    for worktree in &listed.worktrees {
        let candidate = normalize_path(&lexical_absolute(root, Path::new(&worktree.path)))?;
        if candidate == target {
            matched = Some(worktree.is_root);
            break;
        }
    }
    match matched {
        None => Err(invalid(format!(
            "'{path}' is not a linked worktree of this project"
        ))),
        Some(true) => Err(invalid("the project's root checkout cannot be removed")),
        Some(false) => Ok(()),
    }?;
    let target = target.to_string_lossy().to_string();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(target.as_str());
    git(root, &args)?;
    Ok(())
}

/// Drop Git's records of linked worktrees whose folder is gone:
/// `git worktree prune`. It never touches a folder.
pub fn prune(root: &Path) -> Result<(), BridgeError> {
    git(root, &["worktree", "prune"])?;
    Ok(())
}

pub fn lexical_absolute(root: &Path, path: &Path) -> PathBuf {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

/// Resolve a path through its nearest existing parent, so containment checks work
/// before we create missing parent directories.
pub fn normalize_path(path: &Path) -> Result<PathBuf, BridgeError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| invalid(error.to_string()))?
            .join(path)
    };
    let absolute = lexical_absolute(Path::new("/"), &absolute);
    let mut current = absolute.clone();
    let mut suffix: Vec<OsString> = Vec::new();
    loop {
        match fs::metadata(&current) {
            Ok(metadata) => {
                if !metadata.is_dir() {
                    return Err(invalid(format!("{} is not a directory", current.display())));
                }
                let mut resolved = fs::canonicalize(&current).map_err(|error| {
                    invalid(format!("cannot resolve {}: {error}", current.display()))
                })?;
                for part in suffix.iter().rev() {
                    resolved.push(part);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let Some(name) = current.file_name() else {
                    return Err(invalid(format!("cannot resolve {}", path.display())));
                };
                suffix.push(name.to_owned());
                if !current.pop() {
                    return Err(invalid(format!("cannot resolve {}", path.display())));
                }
            }
            Err(error) => {
                return Err(invalid(format!(
                    "cannot resolve {}: {error}",
                    current.display()
                )))
            }
        }
    }
}

pub fn env_candidates(
    root: &Path,
    source_path: &str,
) -> Result<Vec<EnvFileCandidate>, BridgeError> {
    let source = listed_checkout(root, source_path)?;
    let directory = PathBuf::from(source.path);
    let mut candidates = Vec::new();
    for entry in fs::read_dir(&directory)
        .map_err(|error| invalid(format!("cannot read {}: {error}", directory.display())))?
    {
        let entry = entry
            .map_err(|error| invalid(format!("cannot read an environment filename: {error}")))?;
        let os_name = entry.file_name();
        let Some(name) = os_name.to_str() else {
            continue;
        };
        if name != ".env" && !name.starts_with(".env.") {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path());
        let (eligible, reason) = match metadata {
            Err(error) => (false, Some(error.to_string())),
            Ok(metadata) if metadata.file_type().is_symlink() => (false, Some("symlink".into())),
            Ok(metadata) if !metadata.file_type().is_file() => {
                (false, Some("not a regular file".into()))
            }
            Ok(_) if excluded_env_name(name) => (
                false,
                Some("example, sample, template, or distribution file".into()),
            ),
            Ok(_) => (true, None),
        };
        candidates.push(EnvFileCandidate {
            name: name.to_owned(),
            eligible,
            reason,
        });
    }
    candidates.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(candidates)
}

fn excluded_env_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [".example", ".sample", ".template", ".dist"]
        .iter()
        .any(|suffix| name.ends_with(suffix))
}

pub fn env_copy(
    root: &Path,
    source_path: &str,
    target_path: &str,
) -> Result<EnvCopyReport, BridgeError> {
    let source = PathBuf::from(listed_checkout(root, source_path)?.path);
    let target = PathBuf::from(listed_checkout(root, target_path)?.path);
    let candidates = env_candidates(root, source_path)?;
    let mut report = EnvCopyReport {
        copied: Vec::new(),
        skipped: Vec::new(),
        failed: Vec::new(),
    };
    for candidate in candidates
        .into_iter()
        .filter(|candidate| candidate.eligible)
    {
        match copy_env_file(&source.join(&candidate.name), &target.join(&candidate.name)) {
            Ok(true) => report.copied.push(candidate.name),
            Ok(false) => report.skipped.push(candidate.name),
            Err(error) => report.failed.push(EnvCopyFailure {
                name: candidate.name,
                error: error.to_string(),
            }),
        }
    }
    Ok(report)
}

fn copy_env_file(source: &Path, target: &Path) -> io::Result<bool> {
    let mut source_options = OpenOptions::new();
    source_options.read(true).custom_flags(libc::O_NOFOLLOW);
    let mut source_file = source_options.open(source)?;
    if !source_file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "source is not a regular file",
        ));
    }
    let mut target_options = OpenOptions::new();
    target_options.write(true).create_new(true).mode(0o600);
    let mut target_file = match target_options.open(target) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        Err(error) => return Err(error),
    };
    let identity = target_file.metadata().ok();
    if let Err(error) = io::copy(&mut source_file, &mut target_file) {
        drop(target_file);
        if let (Some(identity), Ok(current)) = (identity, fs::symlink_metadata(target)) {
            if current.dev() == identity.dev() && current.ino() == identity.ino() {
                let _ = fs::remove_file(target);
            }
        }
        return Err(error);
    }
    Ok(true)
}

fn listed_checkout(root: &Path, path: &str) -> Result<WorktreeInfo, BridgeError> {
    let listed = list(root)?;
    if !listed.repo {
        return Err(invalid("the project root is not a Git worktree"));
    }
    let wanted = normalize_path(&lexical_absolute(root, Path::new(path)))?;
    listed
        .worktrees
        .into_iter()
        .find(|worktree| {
            normalize_path(Path::new(&worktree.path)).ok().as_deref() == Some(wanted.as_path())
        })
        .filter(|worktree| !worktree.missing)
        .ok_or_else(|| {
            BridgeError::new(
                Code::Daemon(ErrorCode::NotFound),
                format!("{path} is not an existing checkout of this project"),
            )
        })
}

fn parse_worktree_list(bytes: &[u8], root: &Path) -> Vec<WorktreeInfo> {
    let mut rows = Vec::new();
    let mut path = None;
    let mut branch = None;
    let mut head = String::new();

    let finish = |rows: &mut Vec<WorktreeInfo>,
                  path: &mut Option<PathBuf>,
                  branch: &mut Option<String>,
                  head: &mut String| {
        let Some(path) = path.take() else {
            branch.take();
            head.clear();
            return;
        };
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |value| value.to_string_lossy().into_owned(),
        );
        let is_root = path.canonicalize().is_ok_and(|candidate| candidate == root);
        rows.push(WorktreeInfo {
            name,
            path: path.to_string_lossy().into_owned(),
            branch: branch.take(),
            head: head.chars().take(8).collect(),
            is_root,
            missing: false,
            dirty: None,
            changed: None,
        });
        head.clear();
    };

    for field in bytes.split(|byte| *byte == 0) {
        if field.is_empty() {
            finish(&mut rows, &mut path, &mut branch, &mut head);
        } else if let Some(value) = field.strip_prefix(b"worktree ") {
            finish(&mut rows, &mut path, &mut branch, &mut head);
            path = Some(PathBuf::from(String::from_utf8_lossy(value).into_owned()));
        } else if let Some(value) = field.strip_prefix(b"HEAD ") {
            head = String::from_utf8_lossy(value).into_owned();
        } else if let Some(value) = field.strip_prefix(b"branch refs/heads/") {
            branch = Some(String::from_utf8_lossy(value).into_owned());
        } else if field == b"detached"
            || field.starts_with(b"locked")
            || field.starts_with(b"prunable")
            || field == b"bare"
        {
            // These flags do not affect the row's branch or checkout identity.
        }
    }
    finish(&mut rows, &mut path, &mut branch, &mut head);
    rows
}

fn status_count(bytes: &[u8]) -> usize {
    let records: Vec<&[u8]> = bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .collect();
    let mut count = 0;
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        if record.len() >= 3 && (record[0] != b' ' || record[1] != b' ') {
            count += 1;
            if record[0] == b'R' || record[0] == b'C' || record[1] == b'R' || record[1] == b'C' {
                index += 1; // -z emits the original path as the next record.
            }
        }
        index += 1;
    }
    count
}

fn trim_line_ending(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}

pub fn invalid(message: impl Into<String>) -> BridgeError {
    BridgeError::new(Code::Daemon(ErrorCode::InvalidArgument), message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::fs;
    use std::process::Command;

    fn git_ok(root: &Path, args: &[&str]) -> Vec<u8> {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    fn repository(label: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new(label);
        let root = dir.path().join("repo");
        fs::create_dir_all(&root).unwrap();
        git_ok(&root, &["init", "-b", "main"]);
        git_ok(&root, &["config", "user.name", "Goko Test"]);
        git_ok(&root, &["config", "user.email", "goko@example.invalid"]);
        fs::write(root.join("tracked.txt"), "initial\n").unwrap();
        git_ok(&root, &["add", "."]);
        git_ok(&root, &["commit", "-m", "initial"]);
        (dir, root)
    }

    #[test]
    fn lists_root_external_branches_and_detached_head_with_newline_paths() {
        let (_dir, root) = repository("wt-list");
        let linked = root.parent().unwrap().join("linked checkout\nsecond line");
        git_ok(
            &root,
            &[
                "worktree",
                "add",
                "-b",
                "feature/new",
                linked.to_str().unwrap(),
            ],
        );
        let detached = root.parent().unwrap().join("detached");
        git_ok(
            &root,
            &[
                "worktree",
                "add",
                "--detach",
                detached.to_str().unwrap(),
                "HEAD",
            ],
        );

        let listed = list(&root).unwrap();

        assert!(listed.repo);
        assert_eq!(listed.worktrees.len(), 3);
        let main = listed.worktrees.iter().find(|item| item.is_root).unwrap();
        assert_eq!(main.branch.as_deref(), Some("main"));
        let linked_row = listed
            .worktrees
            .iter()
            .find(|item| item.name == "linked checkout\nsecond line")
            .unwrap();
        assert_eq!(linked_row.name, "linked checkout\nsecond line");
        assert_eq!(linked_row.branch.as_deref(), Some("feature/new"));
        let detached_row = listed
            .worktrees
            .iter()
            .find(|item| !item.is_root && item.branch.is_none())
            .unwrap();
        assert_eq!(detached_row.branch, None);
        assert_eq!(
            detached_row.head,
            String::from_utf8_lossy(&git_ok(&root, &["rev-parse", "--short=8", "HEAD"])).trim()
        );

        let refs = branches(&root).unwrap();
        assert_eq!(
            refs.branches
                .iter()
                .find(|item| item.name == "feature/new")
                .unwrap()
                .checked_out_at
                .as_deref(),
            Some(linked.canonicalize().unwrap().to_string_lossy().as_ref())
        );
        assert!(refs
            .bases
            .iter()
            .any(|item| item.name == "main" && item.kind == "local"));
    }

    #[test]
    fn contains_checkout_checks_root_and_linked_membership() {
        let (_dir, root) = repository("wt-membership");
        let linked = root.parent().unwrap().join("linked");
        git_ok(
            &root,
            &[
                "worktree",
                "add",
                "-b",
                "feature/membership",
                linked.to_str().unwrap(),
            ],
        );
        let unrelated = root.parent().unwrap().join("unrelated");
        fs::create_dir(&unrelated).unwrap();

        assert!(contains_checkout(&root, &root).unwrap());
        assert!(contains_checkout(&root, &linked.canonicalize().unwrap()).unwrap());
        assert!(!contains_checkout(&root, &unrelated).unwrap());
    }

    #[test]
    fn missing_worktree_remains_listed_and_status_counts_changes_and_renames_once() {
        let (_dir, root) = repository("wt-status");
        let linked = root.parent().unwrap().join("changed");
        git_ok(
            &root,
            &[
                "worktree",
                "add",
                "-b",
                "feature/changed",
                linked.to_str().unwrap(),
            ],
        );
        git_ok(&linked, &["mv", "tracked.txt", "renamed.txt"]);
        fs::write(linked.join("new\nfile.txt"), "untracked\n").unwrap();

        let listed = list(&root).unwrap();
        let row = listed
            .worktrees
            .iter()
            .find(|item| item.name == "changed")
            .unwrap();
        assert_eq!(row.changed, Some(2), "rename plus untracked file");
        assert_eq!(row.dirty, Some(true));

        fs::remove_dir_all(&linked).unwrap();
        let listed = list(&root).unwrap();
        let missing = listed
            .worktrees
            .iter()
            .find(|item| item.name == "changed")
            .unwrap();
        assert!(missing.missing);
        assert_eq!(missing.dirty, None);
        assert_eq!(missing.changed, None);
    }

    #[test]
    fn prune_drops_the_record_of_a_deleted_worktree_and_keeps_a_live_one() {
        let (_dir, root) = repository("wt-prune");
        let gone = root.parent().unwrap().join("gone");
        let live = root.parent().unwrap().join("live");
        git_ok(
            &root,
            &[
                "worktree",
                "add",
                "-b",
                "feature/gone",
                gone.to_str().unwrap(),
            ],
        );
        git_ok(
            &root,
            &[
                "worktree",
                "add",
                "-b",
                "feature/live",
                live.to_str().unwrap(),
            ],
        );
        fs::remove_dir_all(&gone).unwrap();

        prune(&root).unwrap();

        let listed = list(&root).unwrap();
        assert!(listed.worktrees.iter().all(|item| item.name != "gone"));
        let kept = listed
            .worktrees
            .iter()
            .find(|item| item.name == "live")
            .unwrap();
        assert!(!kept.missing);
        assert!(live.is_dir());
    }

    #[test]
    fn prune_outside_a_repository_is_an_error() {
        let dir = TempDir::new("wt-prune-nonrepo");
        assert!(prune(dir.path()).is_err());
    }

    #[test]
    fn non_git_directory_and_repository_subdirectory_are_not_worktree_roots() {
        let dir = TempDir::new("wt-nonrepo");
        assert!(!list(dir.path()).unwrap().repo);

        let (_repo_dir, root) = repository("wt-subdir");
        let child = root.join("child");
        fs::create_dir(&child).unwrap();
        let listed = list(&child).unwrap();
        assert!(!listed.repo);
        assert!(listed.worktrees.is_empty());
    }

    #[test]
    fn absent_git_binary_has_an_install_hint() {
        let dir = TempDir::new("wt-no-git");
        let empty_path = dir.path().as_os_str();
        let error = run(dir.path(), &["status"], Some(empty_path)).unwrap_err();
        assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
        assert!(error
            .hint
            .as_deref()
            .unwrap_or_default()
            .contains("Install Git"));
    }

    #[test]
    fn creates_new_and_existing_branches_and_rejects_checked_out_or_invalid_inputs() {
        let (_dir, root) = repository("wt-create");
        let sibling = root.parent().unwrap();
        let new_path = sibling.join("new-worktree");
        let created = create(
            &root,
            new_path.to_str().unwrap(),
            "new",
            "feature/new",
            Some("main"),
        )
        .unwrap();
        assert_eq!(created.branch.as_deref(), Some("feature/new"));
        assert!(created.path.ends_with("new-worktree"));

        git_ok(&root, &["branch", "free-branch"]);
        let existing_path = sibling.join("existing-worktree");
        let existing = create(
            &root,
            existing_path.to_str().unwrap(),
            "existing",
            "free-branch",
            None,
        )
        .unwrap();
        assert_eq!(existing.branch.as_deref(), Some("free-branch"));

        let cases = [
            (sibling.join("bad-branch"), "new", "bad..name", Some("main")),
            (sibling.join("collision"), "new", "main", Some("main")),
            (sibling.join("checked-out"), "existing", "main", None),
            (root.join("nested"), "new", "nested", Some("main")),
        ];
        for (path, mode, branch, base) in cases {
            let error = create(&root, path.to_str().unwrap(), mode, branch, base).unwrap_err();
            assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
            assert!(!path.exists(), "invalid request created {}", path.display());
        }

        let file = sibling.join("not-a-directory");
        fs::write(&file, "x").unwrap();
        let error = create(&root, file.to_str().unwrap(), "new", "file-target", None).unwrap_err();
        assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));

        let nonempty = sibling.join("nonempty");
        fs::create_dir(&nonempty).unwrap();
        fs::write(nonempty.join("keep.txt"), "keep").unwrap();
        let error = create(
            &root,
            nonempty.to_str().unwrap(),
            "new",
            "nonempty-target",
            None,
        )
        .unwrap_err();
        assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
        assert_eq!(
            fs::read_to_string(nonempty.join("keep.txt")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn creates_missing_parent_directories_only_after_validation() {
        let (_dir, root) = repository("wt-create-parent");
        let target = root
            .parent()
            .unwrap()
            .join("generated-parent")
            .join("nested")
            .join("checkout");
        let created = create(
            &root,
            target.to_str().unwrap(),
            "new",
            "generated",
            Some("main"),
        )
        .unwrap();
        assert!(Path::new(&created.path).is_dir());
        assert_eq!(created.branch.as_deref(), Some("generated"));
    }

    #[test]
    fn accepts_empty_paths_and_rejects_symlink_targets() {
        use std::os::unix::fs::symlink;

        let (_dir, root) = repository("wt-create-empty");
        let empty = root.parent().unwrap().join("empty");
        fs::create_dir(&empty).unwrap();
        create(
            &root,
            empty.to_str().unwrap(),
            "new",
            "empty-target",
            Some("main"),
        )
        .unwrap();

        let referent = root.parent().unwrap().join("referent");
        fs::create_dir(&referent).unwrap();
        let alias = root.parent().unwrap().join("alias");
        symlink(&referent, &alias).unwrap();
        let error = create(
            &root,
            alias.to_str().unwrap(),
            "new",
            "symlink-target",
            Some("main"),
        )
        .unwrap_err();
        assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
        assert!(alias.is_symlink());
        assert!(referent.is_dir());
    }

    #[test]
    fn previews_and_copies_only_regular_non_example_environment_files_without_overwriting() {
        use std::os::unix::fs::symlink;

        let (_dir, root) = repository("wt-env");
        fs::write(root.join(".env"), "SECRET=one\n").unwrap();
        fs::write(root.join(".env.local"), "LOCAL=one\n").unwrap();
        fs::write(root.join(".env.example"), "EXAMPLE=one\n").unwrap();
        fs::write(root.join(".env.Sample"), "SAMPLE=one\n").unwrap();
        fs::write(root.join(".env.template"), "TEMPLATE=one\n").unwrap();
        fs::write(root.join(".env.dist"), "DIST=one\n").unwrap();
        fs::create_dir(root.join(".env.directory")).unwrap();
        fs::write(root.join("real-env"), "LINK=one\n").unwrap();
        symlink(root.join("real-env"), root.join(".env.symlink")).unwrap();
        let target = root.parent().unwrap().join("env-target");
        create(
            &root,
            target.to_str().unwrap(),
            "new",
            "env-target",
            Some("main"),
        )
        .unwrap();
        fs::write(target.join(".env.local"), "LOCAL=keep\n").unwrap();

        let preview = env_candidates(&root, root.to_str().unwrap()).unwrap();
        assert!(
            preview
                .iter()
                .find(|item| item.name == ".env")
                .unwrap()
                .eligible
        );
        assert!(
            preview
                .iter()
                .find(|item| item.name == ".env.local")
                .unwrap()
                .eligible
        );
        for excluded in [
            ".env.example",
            ".env.Sample",
            ".env.template",
            ".env.dist",
            ".env.directory",
            ".env.symlink",
        ] {
            assert!(
                !preview
                    .iter()
                    .find(|item| item.name == excluded)
                    .unwrap()
                    .eligible,
                "{excluded}"
            );
        }
        assert_eq!(
            preview
                .iter()
                .find(|item| item.name == ".env.symlink")
                .unwrap()
                .reason
                .as_deref(),
            Some("symlink")
        );

        let report = env_copy(&root, root.to_str().unwrap(), target.to_str().unwrap()).unwrap();
        assert_eq!(report.copied, [".env"]);
        assert_eq!(report.skipped, [".env.local"]);
        assert!(report.failed.is_empty());
        assert_eq!(
            fs::read_to_string(target.join(".env")).unwrap(),
            "SECRET=one\n"
        );
        assert_eq!(
            fs::read_to_string(target.join(".env.local")).unwrap(),
            "LOCAL=keep\n"
        );
    }

    #[test]
    fn reports_partial_copy_failures_and_keeps_the_created_checkout() {
        use std::os::unix::fs::PermissionsExt;

        let (_dir, root) = repository("wt-env-failure");
        fs::write(root.join(".env"), "SECRET=one\n").unwrap();
        let target = root.parent().unwrap().join("env-failure-target");
        create(
            &root,
            target.to_str().unwrap(),
            "new",
            "env-failure-target",
            Some("main"),
        )
        .unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o500)).unwrap();
        let report = env_copy(&root, root.to_str().unwrap(), target.to_str().unwrap()).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();

        assert!(
            target.is_dir(),
            "the worktree must remain after a copy failure"
        );
        assert!(report.copied.is_empty());
        assert!(report.skipped.is_empty());
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].name, ".env");
        assert!(!target.join(".env").exists());
    }
}
