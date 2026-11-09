//! Registered projects and validated checkouts, shared by every frontend.
//!
//! Nothing here trusts a caller-supplied id or path: project ids resolve
//! against `goko.db` first, checkout paths must canonicalize to the project
//! root or a worktree `git worktree list` reports, and Git views additionally
//! require a real non-bare worktree root.

use crate::error::{BridgeError, Code};
use crate::git_view;
use crate::store::{Project, Store};
use crate::worktrees;
use protocol::message::ErrorCode;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// A project as a frontend sees it: the stored row plus `missing`, computed at read time.
#[derive(Debug, Serialize)]
pub struct ProjectView {
    #[serde(flatten)]
    pub project: Project,
    pub missing: bool,
}

impl ProjectView {
    fn of(project: Project) -> Self {
        Self {
            missing: !Path::new(&project.root).is_dir(),
            project,
        }
    }
}

/// Every registered project, oldest first, with `missing` computed at read time.
pub fn list_projects(store: &Store) -> Result<Vec<ProjectView>, BridgeError> {
    Ok(store
        .list_projects()?
        .into_iter()
        .map(ProjectView::of)
        .collect())
}

/// Canonicalize `path` and register it; a file or a missing path is `invalid_argument`.
/// Registering the same root again returns the existing row.
pub fn add_project(store: &Store, path: &str) -> Result<ProjectView, BridgeError> {
    let root = std::fs::canonicalize(path)
        .map_err(|error| invalid(format!("cannot use '{path}': {error}")))?;
    if !root.is_dir() {
        return Err(invalid(format!("'{path}' is not a directory")));
    }
    let name = root.file_name().map_or_else(
        || root.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    store
        .add_project(&name, &root.display().to_string())
        .map(ProjectView::of)
}

/// `not_found` when no project has `id`.
pub fn remove_project(store: &Store, id: &str) -> Result<(), BridgeError> {
    match store.remove_project(id)? {
        true => Ok(()),
        false => Err(not_found(format!("no project '{id}' is registered"))),
    }
}

/// The registered project's root, which must still exist on disk.
pub fn project_root(store: &Store, project_id: &str) -> Result<String, BridgeError> {
    let project = store
        .list_projects()?
        .into_iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| not_found(format!("no project '{project_id}' is registered")))?;
    if !Path::new(&project.root).is_dir() {
        return Err(invalid(format!(
            "the project root {} no longer exists",
            project.root
        )));
    }
    Ok(project.root)
}

/// The canonical `checkout` for `root`: the root itself or a checkout
/// `git worktree list` reports for it. Anything else is `invalid_argument`.
pub fn project_checkout(root: &str, checkout: &str) -> Result<String, BridgeError> {
    let root = std::fs::canonicalize(root)
        .map_err(|error| invalid(format!("the project root is unavailable: {error}")))?;
    let checkout = std::fs::canonicalize(checkout)
        .map_err(|error| invalid(format!("the checkout path is unavailable: {error}")))?;
    if checkout == root {
        return Ok(checkout.display().to_string());
    }
    if worktrees::contains_checkout(&root, &checkout)? {
        Ok(checkout.display().to_string())
    } else {
        Err(invalid(
            "the checkout is not the project root or a listed Git worktree",
        ))
    }
}

/// Checkout for a Git operation: registered project, root-or-listed
/// worktree, and a real non-bare Git worktree root.
pub fn git_checkout(
    store: &Store,
    project_id: &str,
    checkout_path: &str,
) -> Result<PathBuf, BridgeError> {
    let root = project_root(store, project_id)?;
    let checkout = project_checkout(&root, checkout_path)?;
    git_view::repo_checkout(Path::new(&checkout))?;
    Ok(PathBuf::from(checkout))
}

fn invalid(message: impl Into<String>) -> BridgeError {
    BridgeError::new(Code::Daemon(ErrorCode::InvalidArgument), message)
}

fn not_found(message: impl Into<String>) -> BridgeError {
    BridgeError::new(Code::Daemon(ErrorCode::NotFound), message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::process::Command;

    fn store(dir: &TempDir) -> Store {
        Store::open(dir.path()).unwrap()
    }

    fn git_ok(root: &Path, args: &[&str]) {
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

    fn repo(label: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new(label);
        let root = dir.path().canonicalize().unwrap();
        git_ok(&root, &["init", "-b", "main"]);
        git_ok(&root, &["config", "user.name", "Tester"]);
        git_ok(&root, &["config", "user.email", "tester@example.com"]);
        std::fs::write(root.join("file.txt"), "x\n").unwrap();
        git_ok(&root, &["add", "-A"]);
        git_ok(&root, &["commit", "-qm", "init"]);
        (dir, root)
    }

    #[test]
    fn add_list_and_remove_projects_round_trip_with_missing_flag() {
        let dir = TempDir::new("projects-crud");
        let store = store(&dir);
        let (repo_dir, root) = repo("projects-repo");

        let added = add_project(&store, root.to_str().unwrap()).unwrap();
        assert!(!added.missing);
        assert_eq!(added.project.root, root.display().to_string());
        // Registering the same root again returns the existing row.
        let again = add_project(&store, root.to_str().unwrap()).unwrap();
        assert_eq!(again.project.id, added.project.id);

        let listed = list_projects(&store).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].project.id, added.project.id);

        remove_project(&store, &added.project.id).unwrap();
        assert!(list_projects(&store).unwrap().is_empty());
        assert_eq!(
            remove_project(&store, &added.project.id).unwrap_err().code,
            Code::Daemon(ErrorCode::NotFound)
        );
        drop(repo_dir);
    }

    #[test]
    fn add_project_rejects_missing_and_nonsense_paths() {
        let dir = TempDir::new("projects-add-bad");
        let store = store(&dir);

        assert!(add_project(&store, "/no/such/dir").is_err());
        let file = dir.path().join("file.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(add_project(&store, file.to_str().unwrap()).is_err());
    }

    #[test]
    fn project_root_requires_a_registered_existing_root() {
        let dir = TempDir::new("projects-root");
        let store = store(&dir);
        let (repo_dir, root) = repo("projects-root-repo");
        let project = add_project(&store, root.to_str().unwrap()).unwrap();

        assert_eq!(
            project_root(&store, &project.project.id).unwrap(),
            root.display().to_string()
        );
        assert_eq!(
            project_root(&store, "mock-id").unwrap_err().code,
            Code::Daemon(ErrorCode::NotFound)
        );

        std::fs::remove_dir_all(&root).unwrap();
        assert!(project_root(&store, &project.project.id).is_err());
        drop(repo_dir);
    }

    #[test]
    fn project_checkout_accepts_only_the_root_or_a_listed_worktree() {
        let (_repo_dir, root) = repo("projects-checkout-repo");
        let outside = TempDir::new("projects-outside");

        assert_eq!(
            project_checkout(root.to_str().unwrap(), root.to_str().unwrap()).unwrap(),
            root.display().to_string()
        );
        assert!(
            project_checkout(root.to_str().unwrap(), outside.path().to_str().unwrap()).is_err()
        );
        assert!(project_checkout(root.to_str().unwrap(), "/no/such/path").is_err());
    }

    #[test]
    fn git_checkout_validates_project_checkout_and_git_root() {
        let dir = TempDir::new("projects-git-checkout");
        let store = store(&dir);
        let (_repo_dir, root) = repo("projects-git-repo");
        let plain = TempDir::new("projects-plain");
        let project = add_project(&store, root.to_str().unwrap()).unwrap();
        let plain_project = add_project(&store, plain.path().to_str().unwrap()).unwrap();

        let checkout = git_checkout(&store, &project.project.id, root.to_str().unwrap()).unwrap();
        assert_eq!(checkout, root);

        // A registered folder that is not a Git worktree is rejected.
        assert!(git_checkout(
            &store,
            &plain_project.project.id,
            plain.path().to_str().unwrap()
        )
        .is_err());
        // An unregistered id never reaches the filesystem.
        assert_eq!(
            git_checkout(&store, "mock-id", root.to_str().unwrap())
                .unwrap_err()
                .code,
            Code::Daemon(ErrorCode::NotFound)
        );
    }
}
