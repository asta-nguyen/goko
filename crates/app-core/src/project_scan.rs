//! Bounded discovery of Git worktree roots inside a plain folder.
//!
//! Add Project uses this when the selected path is not a Git worktree root:
//! the scan walks a small, bounded part of the tree and reports the nested
//! repositories for the review dialog. It never follows a symlink and never
//! descends into a repository it already found, so a submodule or a nested
//! checkout is not reported as a second repository.

use crate::error::{BridgeError, Code};
use crate::git_view;
use protocol::message::ErrorCode;
use serde::Serialize;
use std::collections::VecDeque;
use std::path::Path;
use std::time::{Duration, Instant};

/// VCS metadata directories that are never repositories themselves.
const VCS_DIRS: &[&str] = &[".git", ".svn", ".hg", ".jj", ".sl", ".repo", "CVS"];

/// Heavy build/dependency directories that would spend the scan budget.
const SKIPPED_DIRS: &[&str] = &[
    "node_modules",
    "dist",
    "build",
    ".next",
    ".cache",
    "vendor",
    "__pycache__",
    ".turbo",
    ".parcel-cache",
];

/// Bounds for one scan. The defaults are the review dialog's limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanOptions {
    pub max_depth: u32,
    pub max_repos: usize,
    pub max_dirs: usize,
    pub timeout: Duration,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            max_depth: 3,
            max_repos: 100,
            max_dirs: 10_000,
            timeout: Duration::from_secs(5),
        }
    }
}

/// One nested repository the scan found.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NestedRepoCandidate {
    pub path: String,
    pub display_name: String,
    pub depth: u32,
}

/// Whether the selected path was already a repository.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectedPathKind {
    GitRepo,
    NonGitFolder,
}

/// The bounded scan's result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NestedScanResult {
    pub selected_path: String,
    pub selected_path_kind: SelectedPathKind,
    pub repos: Vec<NestedRepoCandidate>,
    pub truncated: bool,
    pub timed_out: bool,
    pub stopped: bool,
    pub duration_ms: u64,
    pub max_depth: u32,
    pub max_repos: usize,
    pub max_dirs: usize,
    pub timeout_ms: u64,
}

/// True for a directory the scan never descends into. Hidden directories are
/// skipped below the selected root; VCS and heavy directories at every depth.
pub fn is_skipped_dir(name: &str, depth: u32) -> bool {
    VCS_DIRS.contains(&name) || SKIPPED_DIRS.contains(&name) || (depth > 0 && name.starts_with('.'))
}

/// A `.git` directory or regular `.git` file marks a worktree root. A bare
/// repository (HEAD/objects/refs without `.git`) is deliberately not a marker.
pub fn has_git_marker(path: &Path) -> bool {
    std::fs::symlink_metadata(path.join(".git"))
        .map(|metadata| metadata.file_type().is_dir() || metadata.file_type().is_file())
        .unwrap_or(false)
}

/// Scan `path` for nested Git worktree roots. `path` itself is checked first:
/// a real non-bare worktree root returns `GitRepo` with no candidates.
pub fn scan(path: &Path, options: ScanOptions) -> Result<NestedScanResult, BridgeError> {
    let root = std::fs::canonicalize(path)
        .map_err(|error| invalid(format!("cannot scan '{}': {error}", path.display())))?;
    if !root.is_dir() {
        return Err(invalid(format!("'{}' is not a directory", path.display())));
    }
    let started = Instant::now();
    let selected_path = root.display().to_string();
    let mut result = NestedScanResult {
        selected_path,
        selected_path_kind: SelectedPathKind::NonGitFolder,
        repos: Vec::new(),
        truncated: false,
        timed_out: false,
        stopped: false,
        duration_ms: 0,
        max_depth: options.max_depth,
        max_repos: options.max_repos,
        max_dirs: options.max_dirs,
        timeout_ms: options.timeout.as_millis() as u64,
    };
    if git_view::repo_checkout(&root).is_ok() {
        result.selected_path_kind = SelectedPathKind::GitRepo;
        result.duration_ms = started.elapsed().as_millis() as u64;
        return Ok(result);
    }

    let mut folders = VecDeque::from([(root, 0u32)]);
    let mut directories = 0usize;
    while let Some((folder, depth)) = folders.pop_front() {
        if result.repos.len() >= options.max_repos {
            result.truncated = true;
            break;
        }
        if started.elapsed() > options.timeout {
            result.timed_out = true;
            break;
        }
        if directories >= options.max_dirs {
            result.truncated = true;
            break;
        }
        directories += 1;
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            if result.repos.len() >= options.max_repos {
                result.truncated = true;
                break;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() || is_skipped_dir(&name, depth + 1) {
                continue;
            }
            let child = entry.path();
            if has_git_marker(&child) {
                result.repos.push(NestedRepoCandidate {
                    path: child.display().to_string(),
                    display_name: name,
                    depth: depth + 1,
                });
            } else if depth + 1 < options.max_depth {
                folders.push_back((child, depth + 1));
            }
        }
    }
    result.duration_ms = started.elapsed().as_millis() as u64;
    Ok(result)
}

fn invalid(message: impl Into<String>) -> BridgeError {
    BridgeError::new(Code::Daemon(ErrorCode::InvalidArgument), message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::path::PathBuf;
    use std::process::Command;

    fn git(root: &Path, args: &[&str]) {
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
    }

    fn repo(root: &Path) {
        std::fs::create_dir_all(root).unwrap();
        git(root, &["init", "-b", "main"]);
        git(root, &["config", "user.name", "Tester"]);
        git(root, &["config", "user.email", "tester@example.com"]);
        std::fs::write(root.join("file.txt"), "x\n").unwrap();
        git(root, &["add", "-A"]);
        git(root, &["commit", "-qm", "init"]);
    }

    fn marker(root: &Path) {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(root.join(".git"), "gitdir: /tmp/elsewhere\n").unwrap();
    }

    fn scan_default(root: &Path) -> NestedScanResult {
        scan(root, ScanOptions::default()).unwrap()
    }

    fn names(result: &NestedScanResult) -> Vec<&str> {
        result
            .repos
            .iter()
            .map(|repo| repo.display_name.as_str())
            .collect()
    }

    #[test]
    fn two_direct_children_are_found() {
        let dir = TempDir::new("scan-direct");
        let parent = dir.path().canonicalize().unwrap();
        marker(&parent.join("first"));
        marker(&parent.join("second"));
        let result = scan_default(&parent);
        assert_eq!(result.selected_path_kind, SelectedPathKind::NonGitFolder);
        assert_eq!(names(&result), vec!["first", "second"]);
        assert!(result.repos.iter().all(|repo| repo.depth == 1));
        assert!(!result.truncated && !result.timed_out && !result.stopped);
    }

    #[test]
    fn a_deeper_candidate_is_found_but_a_repository_is_not_descended() {
        let dir = TempDir::new("scan-depth");
        let parent = dir.path().canonicalize().unwrap();
        marker(&parent.join("group/one"));
        marker(&parent.join("group/one/nested"));
        let result = scan_default(&parent);
        assert_eq!(names(&result), vec!["one"]);
        assert_eq!(result.repos[0].depth, 2);
    }

    #[test]
    fn skip_rules_and_hidden_directories_are_not_scanned() {
        let dir = TempDir::new("scan-skip");
        let parent = dir.path().canonicalize().unwrap();
        marker(&parent.join("node_modules/dep"));
        marker(&parent.join("dist/output"));
        marker(&parent.join("build/output"));
        marker(&parent.join(".hidden/secret"));
        marker(&parent.join("src"));
        let result = scan_default(&parent);
        assert_eq!(names(&result), vec!["src"]);
    }

    #[test]
    fn a_bare_repository_is_not_a_candidate() {
        let dir = TempDir::new("scan-bare");
        let parent = dir.path().canonicalize().unwrap();
        let bare = parent.join("bare.git");
        std::fs::create_dir_all(&bare).unwrap();
        git(&bare, &["init", "--bare"]);
        let result = scan_default(&parent);
        assert!(result.repos.is_empty(), "{:?}", result.repos);
    }

    #[test]
    fn a_git_file_marker_is_a_candidate() {
        let dir = TempDir::new("scan-marker");
        let parent = dir.path().canonicalize().unwrap();
        marker(&parent.join("worktree"));
        let result = scan_default(&parent);
        assert_eq!(names(&result), vec!["worktree"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_directory_is_not_traversed() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new("scan-symlink");
        let parent = dir.path().canonicalize().unwrap();
        let outside = TempDir::new("scan-symlink-outside");
        repo(&outside.path().canonicalize().unwrap());
        symlink(outside.path(), parent.join("link")).unwrap();
        let result = scan_default(&parent);
        assert!(result.repos.is_empty(), "{:?}", result.repos);
    }

    #[test]
    fn repository_and_directory_bounds_truncate() {
        let dir = TempDir::new("scan-bounds");
        let parent = dir.path().canonicalize().unwrap();
        for index in 0..5 {
            marker(&parent.join(format!("repo-{index}")));
        }
        let result = scan(
            &parent,
            ScanOptions {
                max_repos: 2,
                ..ScanOptions::default()
            },
        )
        .unwrap();
        assert_eq!(result.repos.len(), 2);
        assert!(result.truncated);

        let result = scan(
            &parent,
            ScanOptions {
                max_dirs: 0,
                ..ScanOptions::default()
            },
        )
        .unwrap();
        assert!(result.truncated);
        assert!(result.repos.is_empty());
    }

    #[test]
    fn a_zero_timeout_reports_timed_out() {
        let dir = TempDir::new("scan-timeout");
        let parent = dir.path().canonicalize().unwrap();
        marker(&parent.join("repo"));
        let result = scan(
            &parent,
            ScanOptions {
                timeout: Duration::ZERO,
                ..ScanOptions::default()
            },
        )
        .unwrap();
        assert!(result.timed_out);
    }

    #[test]
    fn the_selected_path_repository_is_not_scanned() {
        let dir = TempDir::new("scan-selected");
        let root: PathBuf = dir.path().canonicalize().unwrap();
        repo(&root);
        marker(&root.join("child"));
        let result = scan_default(&root);
        assert_eq!(result.selected_path_kind, SelectedPathKind::GitRepo);
        assert!(result.repos.is_empty());
    }
}
