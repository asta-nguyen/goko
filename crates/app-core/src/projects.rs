//! Registered projects and validated checkouts, shared by every frontend.
//!
//! Nothing here trusts a caller-supplied id or path: project ids resolve
//! against `goko.db` first, checkout paths must canonicalize to the project
//! root or a worktree `git worktree list` reports, and Git views additionally
//! require a real non-bare worktree root.

use crate::error::{BridgeError, Code};
use crate::git_view;
use crate::store::{ChildLink, Project, Store};
use crate::worktrees;
use protocol::message::ErrorCode;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A project as a frontend sees it: the stored row plus `missing`, computed at read time,
/// and the folder project it is nested under, when it is a child repository.
#[derive(Clone, Debug, Serialize)]
pub struct ProjectView {
    #[serde(flatten)]
    pub project: Project,
    pub missing: bool,
    pub parent_project_id: Option<String>,
}

impl ProjectView {
    fn of(project: Project, parent_project_id: Option<String>) -> Self {
        Self {
            missing: !Path::new(&project.root).is_dir(),
            project,
            parent_project_id,
        }
    }
}

/// What one selected repository did during a nested import.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportStatus {
    Imported,
    AlreadyKnown,
    Failed,
}

/// One selected repository's import result.
#[derive(Debug, Serialize)]
pub struct ImportedProject {
    pub path: String,
    pub project: Option<ProjectView>,
    pub status: ImportStatus,
    pub error: Option<String>,
}

/// How Add Project registers a reviewed folder.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportMode {
    /// Register the parent folder and the selected repositories under it.
    Group,
    /// Register the selected repositories without the parent.
    Separate,
    /// Register only the parent folder.
    Folder,
}

/// The result of one reviewed import.
#[derive(Debug, Serialize)]
pub struct NestedImportResult {
    pub parent: Option<ProjectView>,
    pub projects: Vec<ImportedProject>,
    pub imported: usize,
    pub already_known: usize,
    pub failed: usize,
}

/// Every registered project, oldest first, with `missing` and its parent
/// computed at read time.
pub fn list_projects(store: &Store) -> Result<Vec<ProjectView>, BridgeError> {
    let parents: HashMap<String, String> = store.list_project_parents()?.into_iter().collect();
    Ok(store
        .list_projects()?
        .into_iter()
        .map(|project| {
            let parent = parents.get(&project.id).cloned();
            ProjectView::of(project, parent)
        })
        .collect())
}

/// Canonicalize `path` and register it in `workspace_id`; a file or a missing
/// path is `invalid_argument`. Registering the same root again returns the
/// existing row with its membership unchanged.
pub fn add_project(
    store: &Store,
    path: &str,
    workspace_id: &str,
) -> Result<ProjectView, BridgeError> {
    let root = canonical_dir(path)?;
    let name = folder_name(&root);
    add_project_root(store, &root, &name, workspace_id)
}

/// Register `path` under an explicit name (the review dialog's group name),
/// falling back to the folder name when `name` is blank.
pub fn add_project_named(
    store: &Store,
    path: &str,
    name: &str,
    workspace_id: &str,
) -> Result<ProjectView, BridgeError> {
    let root = canonical_dir(path)?;
    add_project_root(store, &root, name, workspace_id)
}

fn add_project_root(
    store: &Store,
    root: &Path,
    name: &str,
    workspace_id: &str,
) -> Result<ProjectView, BridgeError> {
    let name = name.trim();
    let name = if name.is_empty() {
        folder_name(root)
    } else {
        name.to_string()
    };
    let project = store.add_project(&name, &root.display().to_string(), workspace_id)?;
    let parent = store.parent_of(&project.id)?;
    Ok(ProjectView::of(project, parent))
}

/// Register a repository under a folder project. The child joins the parent's
/// workspace; a project already registered under another parent or workspace
/// keeps its place and is reported as already known.
pub fn add_child_project(
    store: &Store,
    parent_id: &str,
    path: &str,
) -> Result<(ProjectView, ImportStatus), BridgeError> {
    let root = canonical_dir(path)?;
    git_view::repo_checkout(&root)?;
    let name = folder_name(&root);
    let link = store.add_child_project(&name, &root.display().to_string(), parent_id)?;
    let (project, status) = match link {
        ChildLink::Added(project) => (project, ImportStatus::Imported),
        ChildLink::AlreadyKnown(project) => (project, ImportStatus::AlreadyKnown),
        ChildLink::OtherParent(project) => (project, ImportStatus::AlreadyKnown),
        ChildLink::OtherWorkspace(project) => (project, ImportStatus::AlreadyKnown),
    };
    let parent = store.parent_of(&project.id)?;
    let view = ProjectView::of(project, parent);
    Ok((view, status))
}

/// Import the reviewed repositories of one folder: group them under the
/// parent, register them separately, or register only the parent.
#[allow(clippy::too_many_arguments)]
pub fn import_nested(
    store: &Store,
    parent_path: &str,
    parent_id: Option<&str>,
    group_name: &str,
    project_paths: &[String],
    mode: ImportMode,
    workspace_id: &str,
) -> Result<NestedImportResult, BridgeError> {
    let parent_root = canonical_dir(parent_path)?;
    let parent = match mode {
        ImportMode::Folder | ImportMode::Group => Some(match parent_id {
            Some(id) => {
                let project = store
                    .list_projects()?
                    .into_iter()
                    .find(|project| project.id == id)
                    .ok_or_else(|| not_found(format!("no project '{id}' is registered")))?;
                if Path::new(&project.root) != parent_root {
                    return Err(invalid(
                        "the parent path does not match the registered project",
                    ));
                }
                let parent = store.parent_of(&project.id)?;
                ProjectView::of(project, parent)
            }
            None => add_project_root(store, &parent_root, group_name, workspace_id)?,
        }),
        ImportMode::Separate => None,
    };

    let mut projects = Vec::new();
    if mode != ImportMode::Folder {
        for path in project_paths {
            projects.push(import_one(
                store,
                &parent_root,
                path,
                parent.as_ref(),
                mode,
                workspace_id,
            ));
        }
    }
    let imported = projects
        .iter()
        .filter(|project| project.status == ImportStatus::Imported)
        .count();
    let already_known = projects
        .iter()
        .filter(|project| project.status == ImportStatus::AlreadyKnown)
        .count();
    let failed = projects
        .iter()
        .filter(|project| project.status == ImportStatus::Failed)
        .count();
    Ok(NestedImportResult {
        parent,
        projects,
        imported,
        already_known,
        failed,
    })
}

fn import_one(
    store: &Store,
    parent_root: &Path,
    path: &str,
    parent: Option<&ProjectView>,
    mode: ImportMode,
    workspace_id: &str,
) -> ImportedProject {
    let failed = |error: String| ImportedProject {
        path: path.to_string(),
        project: None,
        status: ImportStatus::Failed,
        error: Some(error),
    };
    let root = match canonical_dir(path) {
        Ok(root) => root,
        Err(error) => return failed(error.message),
    };
    if !root.starts_with(parent_root) {
        return failed("the repository is outside the selected folder".into());
    }
    if let Err(error) = git_view::repo_checkout(&root) {
        return failed(error.message);
    }
    match mode {
        ImportMode::Group => {
            let Some(parent) = parent else {
                return failed("the parent folder is unavailable".into());
            };
            match add_child_project(store, &parent.project.id, &root.display().to_string()) {
                Ok((project, status)) => ImportedProject {
                    path: root.display().to_string(),
                    project: Some(project),
                    status,
                    error: None,
                },
                Err(error) => failed(error.message),
            }
        }
        ImportMode::Separate => {
            if let Ok(Some(project)) = store.project_by_root(&root.display().to_string()) {
                let parent = store.parent_of(&project.id).ok().flatten();
                return ImportedProject {
                    path: root.display().to_string(),
                    project: Some(ProjectView::of(project, parent)),
                    status: ImportStatus::AlreadyKnown,
                    error: None,
                };
            }
            let name = folder_name(&root);
            match add_project_root(store, &root, &name, workspace_id) {
                Ok(project) => ImportedProject {
                    path: root.display().to_string(),
                    project: Some(project),
                    status: ImportStatus::Imported,
                    error: None,
                },
                Err(error) => failed(error.message),
            }
        }
        ImportMode::Folder => failed("the folder mode imports no repositories".into()),
    }
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

/// Checkout for a file job: the registered project root or a listed Git
/// worktree, without requiring a Git worktree. The flag says whether the
/// canonical checkout is a non-bare Git worktree root.
pub fn file_checkout(
    store: &Store,
    project_id: &str,
    checkout_path: &str,
) -> Result<(PathBuf, bool), BridgeError> {
    let root = project_root(store, project_id)?;
    let checkout = project_checkout(&root, checkout_path)?;
    let path = PathBuf::from(&checkout);
    let is_git = git_view::repo_checkout(&path).is_ok();
    Ok((path, is_git))
}

fn canonical_dir(path: &str) -> Result<PathBuf, BridgeError> {
    let root = std::fs::canonicalize(path)
        .map_err(|error| invalid(format!("cannot use '{path}': {error}")))?;
    if !root.is_dir() {
        return Err(invalid(format!("'{path}' is not a directory")));
    }
    Ok(root)
}

fn folder_name(root: &Path) -> String {
    root.file_name().map_or_else(
        || root.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
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
    use crate::store::DEFAULT_WORKSPACE_ID;
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

        let added = add_project(&store, root.to_str().unwrap(), DEFAULT_WORKSPACE_ID).unwrap();
        assert!(!added.missing);
        assert_eq!(added.project.root, root.display().to_string());
        // Registering the same root again returns the existing row.
        let again = add_project(&store, root.to_str().unwrap(), DEFAULT_WORKSPACE_ID).unwrap();
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

        assert!(add_project(&store, "/no/such/dir", DEFAULT_WORKSPACE_ID).is_err());
        let file = dir.path().join("file.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(add_project(&store, file.to_str().unwrap(), DEFAULT_WORKSPACE_ID).is_err());
    }

    #[test]
    fn add_project_registers_into_the_named_workspace_and_listings_carry_it() {
        let dir = TempDir::new("projects-workspace");
        let store = store(&dir);
        let (repo_dir, root) = repo("projects-workspace-repo");
        let work = store.create_workspace("Work").unwrap();

        let added = add_project(&store, root.to_str().unwrap(), &work.id).unwrap();
        assert_eq!(added.project.workspace_id, work.id);
        assert_eq!(
            list_projects(&store).unwrap()[0].project.workspace_id,
            work.id
        );

        // Re-adding under Default keeps the original membership.
        let again = add_project(&store, root.to_str().unwrap(), DEFAULT_WORKSPACE_ID).unwrap();
        assert_eq!(again.project.id, added.project.id);
        assert_eq!(again.project.workspace_id, work.id);
        drop(repo_dir);
    }

    fn parent_with_repos(label: &str) -> (TempDir, PathBuf, PathBuf, PathBuf) {
        let dir = TempDir::new(label);
        let parent = dir.path().canonicalize().unwrap();
        let first = parent.join("first");
        let second = parent.join("second");
        for root in [&first, &second] {
            std::fs::create_dir_all(root).unwrap();
            git_ok(root, &["init", "-b", "main"]);
            git_ok(root, &["config", "user.name", "Tester"]);
            git_ok(root, &["config", "user.email", "tester@example.com"]);
            std::fs::write(root.join("file.txt"), "x\n").unwrap();
            git_ok(root, &["add", "-A"]);
            git_ok(root, &["commit", "-qm", "init"]);
        }
        (dir, parent, first, second)
    }

    #[test]
    fn import_nested_group_registers_the_parent_and_children() {
        let (dir, parent, first, second) = parent_with_repos("projects-import-group");
        let store = store(&dir);
        let result = import_nested(
            &store,
            parent.to_str().unwrap(),
            None,
            "Group",
            &[first.display().to_string(), second.display().to_string()],
            ImportMode::Group,
            DEFAULT_WORKSPACE_ID,
        )
        .unwrap();

        assert_eq!(result.imported, 2);
        assert_eq!(result.already_known, 0);
        assert_eq!(result.failed, 0);
        let parent_view = result.parent.expect("the group parent");
        assert_eq!(parent_view.project.name, "Group");
        let listed = list_projects(&store).unwrap();
        assert_eq!(listed.len(), 3);
        for child in [&first, &second] {
            let view = listed
                .iter()
                .find(|project| project.project.root == child.display().to_string())
                .unwrap();
            assert_eq!(
                view.parent_project_id.as_deref(),
                Some(parent_view.project.id.as_str())
            );
        }
    }

    #[test]
    fn import_nested_separate_and_folder_modes() {
        let (dir, parent, first, second) = parent_with_repos("projects-import-modes");
        let store = store(&dir);
        let paths = [first.display().to_string(), second.display().to_string()];

        let separate = import_nested(
            &store,
            parent.to_str().unwrap(),
            None,
            "",
            &paths,
            ImportMode::Separate,
            DEFAULT_WORKSPACE_ID,
        )
        .unwrap();
        assert!(separate.parent.is_none());
        assert_eq!(separate.imported, 2);
        assert!(list_projects(&store)
            .unwrap()
            .iter()
            .all(|project| project.parent_project_id.is_none()));

        let again = import_nested(
            &store,
            parent.to_str().unwrap(),
            None,
            "",
            &paths,
            ImportMode::Separate,
            DEFAULT_WORKSPACE_ID,
        )
        .unwrap();
        assert_eq!(again.already_known, 2);

        let folder_dir = TempDir::new("projects-import-folder");
        let folder_store = Store::open(folder_dir.path()).unwrap();
        let folder = import_nested(
            &folder_store,
            parent.to_str().unwrap(),
            None,
            "",
            &[],
            ImportMode::Folder,
            DEFAULT_WORKSPACE_ID,
        )
        .unwrap();
        assert_eq!(folder.imported, 0);
        assert_eq!(
            folder.parent.unwrap().project.name,
            parent.file_name().unwrap().to_string_lossy()
        );
        assert!(list_projects(&folder_store).unwrap()[0]
            .parent_project_id
            .is_none());
    }

    #[test]
    fn import_nested_reports_already_known_and_failed_paths() {
        let (dir, parent, first, second) = parent_with_repos("projects-import-fail");
        let store = store(&dir);
        let paths = [first.display().to_string(), second.display().to_string()];
        let group = import_nested(
            &store,
            parent.to_str().unwrap(),
            None,
            "Group",
            &paths,
            ImportMode::Group,
            DEFAULT_WORKSPACE_ID,
        )
        .unwrap();
        assert_eq!(group.imported, 2);

        let again = import_nested(
            &store,
            parent.to_str().unwrap(),
            None,
            "Group",
            &paths,
            ImportMode::Group,
            DEFAULT_WORKSPACE_ID,
        )
        .unwrap();
        assert_eq!(again.already_known, 2);

        let plain = parent.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        let outside = TempDir::new("projects-import-outside");
        let mixed = import_nested(
            &store,
            parent.to_str().unwrap(),
            None,
            "Group",
            &[
                plain.display().to_string(),
                outside.path().display().to_string(),
                first.display().to_string(),
            ],
            ImportMode::Group,
            DEFAULT_WORKSPACE_ID,
        )
        .unwrap();
        assert_eq!(mixed.imported, 0);
        assert_eq!(mixed.already_known, 1);
        assert_eq!(mixed.failed, 2);
    }

    #[test]
    fn file_checkout_accepts_a_folder_root_and_rejects_a_foreign_path() {
        let (dir, parent, first, _second) = parent_with_repos("projects-file-checkout");
        let store = store(&dir);
        let folder = add_project(&store, parent.to_str().unwrap(), DEFAULT_WORKSPACE_ID).unwrap();

        let (path, is_git) =
            file_checkout(&store, &folder.project.id, parent.to_str().unwrap()).unwrap();
        assert_eq!(path, parent);
        assert!(!is_git);
        assert!(file_checkout(&store, &folder.project.id, first.to_str().unwrap()).is_err());

        let (repo_dir, root) = repo("projects-file-checkout-git");
        let git = add_project(&store, root.to_str().unwrap(), DEFAULT_WORKSPACE_ID).unwrap();
        let (path, is_git) =
            file_checkout(&store, &git.project.id, root.to_str().unwrap()).unwrap();
        assert_eq!(path, root);
        assert!(is_git);
        drop(repo_dir);
    }

    #[test]
    fn project_root_requires_a_registered_existing_root() {
        let dir = TempDir::new("projects-root");
        let store = store(&dir);
        let (repo_dir, root) = repo("projects-root-repo");
        let project = add_project(&store, root.to_str().unwrap(), DEFAULT_WORKSPACE_ID).unwrap();

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
        let project = add_project(&store, root.to_str().unwrap(), DEFAULT_WORKSPACE_ID).unwrap();
        let plain_project =
            add_project(&store, plain.path().to_str().unwrap(), DEFAULT_WORKSPACE_ID).unwrap();

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
