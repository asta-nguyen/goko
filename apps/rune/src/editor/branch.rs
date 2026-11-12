//! Current checkout identity in the editor header; Git reads stay on the worker.

use std::path::Path;

use rune::{El, Tx, row};

use crate::editor::backend::Req;
use crate::editor::state::EditorState;
use crate::theme::{self, Ink};

const INTERVAL: f64 = 3.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub name: Option<String>,
    pub head: String,
}

impl Branch {
    fn label(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("Detached · {}", self.head))
    }
}

#[derive(Default)]
pub struct BranchState {
    pub checkout: Option<String>,
    pub data: Option<Branch>,
    pub error: Option<String>,
    pub pending: bool,
    pub checked_at: Option<f64>,
}

/// Named branches need one metadata read; detached HEAD needs its short commit ID.
pub fn read(checkout: &Path) -> Result<Branch, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        return Ok(Branch {
            name: Some(String::from_utf8_lossy(&output.stdout).trim().to_string()),
            head: String::new(),
        });
    }
    if output.status.code() != Some(1) {
        return Err(format!(
            "cannot read current branch ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let bytes = app_core::worktrees::git(checkout, &["rev-parse", "--short=8", "HEAD"])
        .map_err(|error| error.to_string())?;
    Ok(Branch {
        name: None,
        head: String::from_utf8_lossy(&bytes).trim().to_string(),
    })
}

/// Active-editor polling allows one outstanding probe, including while the queue is busy.
pub fn poll(ed: &mut EditorState, active: bool, now: f64, tx: &mut Tx) -> bool {
    if !active {
        ed.branch.checked_at = None;
        return false;
    }
    let (Some(checkout), Some(key)) = (ed.checkout.clone(), ed.checkout_key.clone()) else {
        return false;
    };
    if ed.branch.checkout.as_ref() != Some(&key) {
        ed.branch = BranchState {
            checkout: Some(key.clone()),
            ..Default::default()
        };
    }
    tx.wake_after(std::time::Duration::from_secs_f64(INTERVAL));
    if ed.branch.pending || ed.branch.checked_at.is_some_and(|at| now - at < INTERVAL) {
        return false;
    }
    ed.branch.checked_at = Some(now);
    ed.branch.pending = ed.enqueue_read(checkout, key, Req::CurrentBranch);
    if !ed.branch.pending {
        ed.branch.error = Some("Branch refresh could not be queued".into());
    }
    true
}

pub fn view(ed: &EditorState, ink: &Ink) -> El {
    let fresh = ed.branch.checkout == ed.checkout_key;
    let error = fresh.then_some(ed.branch.error.as_deref()).flatten();
    let data = fresh.then_some(ed.branch.data.as_ref()).flatten();
    let label = if error.is_some() {
        "Branch unavailable".into()
    } else {
        data.map(Branch::label)
            .unwrap_or_else(|| "Loading branch…".into())
    };
    let color = if error.is_some() {
        ink.warning
    } else {
        ink.accent
    };
    let help = if let Some(error) = error {
        format!("Current branch unavailable: {error}")
    } else if ed.branch.pending {
        format!("Current branch: {label} · checking Git…")
    } else {
        format!("Current branch: {label}")
    };
    row()
        .key("editor-current-branch")
        .role("group")
        .label(format!("Current branch: {label}"))
        .h(26.0)
        .px(8.0)
        .gap(6.0)
        .items_center()
        .radius(6.0)
        .w(176.0)
        .grow(1.0)
        .max_w(240.0)
        .shrink()
        .min_w(176.0)
        .fill(color.alpha(0.08))
        .border(1.0, color.alpha(0.25))
        .help(help)
        .child(theme::ic(&theme::GIT_BRANCH, 13.0, color))
        .child(
            theme::txt(&label, theme::T12M, color)
                .min_w(0.0)
                .truncate_middle(),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn reads_unborn_named_branch() {
        let dir = app_core::test_support::TempDir::new("branch-unborn");
        git(dir.path(), &["init", "-b", "topic"]);
        let branch = read(dir.path()).unwrap();
        assert_eq!(branch.name.as_deref(), Some("topic"));
        assert!(branch.head.is_empty());
    }

    #[test]
    fn reflects_checkout_and_detached_head() {
        let dir = app_core::test_support::TempDir::new("branch-head");
        git(dir.path(), &["init", "-b", "main"]);
        git(dir.path(), &["config", "user.email", "test@example.com"]);
        git(dir.path(), &["config", "user.name", "Test"]);
        std::fs::write(dir.path().join("file"), "one").unwrap();
        git(dir.path(), &["add", "file"]);
        git(dir.path(), &["commit", "-m", "initial"]);
        git(dir.path(), &["branch", "other"]);
        git(dir.path(), &["checkout", "other"]);
        assert_eq!(read(dir.path()).unwrap().name.as_deref(), Some("other"));
        let head = String::from_utf8(
            app_core::worktrees::git(dir.path(), &["rev-parse", "--short=8", "HEAD"]).unwrap(),
        )
        .unwrap()
        .trim()
        .to_string();
        git(dir.path(), &["checkout", "--detach"]);
        let detached = read(dir.path()).unwrap();
        assert_eq!(detached.name, None);
        assert_eq!(detached.head, head);
        assert_eq!(detached.head.len(), 8);
    }

    #[test]
    fn broken_head_is_unavailable_instead_of_detached() {
        let dir = app_core::test_support::TempDir::new("branch-broken");
        git(dir.path(), &["init", "-b", "main"]);
        std::fs::write(dir.path().join(".git/HEAD"), "not a ref or object\n").unwrap();
        assert!(
            read(dir.path())
                .unwrap_err()
                .contains("cannot read current branch")
        );
    }

    #[test]
    fn rejects_non_repository() {
        let dir = app_core::test_support::TempDir::new("branch-not-git");
        assert!(read(dir.path()).is_err());
    }
    #[test]
    fn polling_is_paced_bounded_and_only_active_and_never_displays_foreign_or_failed_data() {
        use rune::{Env, Runtime};
        use rune_kit::Kit;
        use std::path::PathBuf;
        let mut ed = EditorState::new(Kit::neutral(true), crate::test_handle());
        let (backend, jobs) = crate::editor::backend::channel_backend();
        ed.backend = backend;
        ed.project_id = Some("p".into());
        ed.checkout = Some(PathBuf::from("/tmp/branch-test"));
        ed.checkout_key = Some("current".into());
        let mut rt = Runtime::new(
            ed,
            |e, _| view(e, &Ink::new()),
            (400.0, 100.0),
            Env::new(true, 2.0),
        );
        rt.update(|ed, tx| {
            assert!(poll(ed, true, 0.0, tx));
        });
        assert!(matches!(jobs.try_recv().unwrap().req, Req::CurrentBranch));
        rt.update(|ed, tx| {
            assert!(!poll(ed, true, 10.0, tx));
        });
        assert!(
            jobs.try_recv().is_err(),
            "one outstanding read even past deadline"
        );
        rt.update(|ed, tx| {
            ed.branch.pending = false;
            assert!(!poll(ed, true, 2.9, tx));
            assert!(poll(ed, true, 3.0, tx));
        });
        assert!(jobs.try_recv().is_ok());
        rt.update(|ed, tx| {
            ed.branch.pending = false;
            assert!(!poll(ed, false, 4.0, tx));
        });
        assert!(jobs.try_recv().is_err());
        rt.update(|ed, tx| {
            assert!(poll(ed, true, 4.1, tx));
        });
        assert!(
            jobs.try_recv().is_ok(),
            "returning to editor refreshes immediately"
        );
        rt.update(|ed, _| {
            ed.branch.checkout = Some("foreign".into());
            ed.branch.data = Some(Branch {
                name: Some("do-not-show".into()),
                head: "12345678".into(),
            });
        });
        rt.frame(0.0);
        let header = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("editor-current-branch"))
            .unwrap();
        assert_eq!(
            header.label.as_deref(),
            Some("Current branch: Loading branch…")
        );
        rt.update(|ed, tx| {
            ed.project_id = None;
            assert!(poll(ed, true, 8.0, tx));
            assert!(!ed.branch.pending, "queue refusal unwinds pending flag");
            ed.branch.data = Some(Branch {
                name: Some("cached".into()),
                head: String::new(),
            });
        });
        rt.frame(0.0);
        let header = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("editor-current-branch"))
            .unwrap();
        assert_eq!(
            header.label.as_deref(),
            Some("Current branch: Branch unavailable")
        );
    }
}
