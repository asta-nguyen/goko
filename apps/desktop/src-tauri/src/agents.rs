//! Local agent adapters resolved on `PATH` at launch time.
//!
//! An adapter is just an argv0 name: `claude`, `codex`, `opencode`, or the user's
//! default shell, which is always available. Resolution happens when a session is
//! created, never earlier, so a binary that appears or disappears between `agent_list`
//! and Start is handled on the launch path.

use crate::error::{BridgeError, Code};
use protocol::message::ErrorCode;
use serde::Serialize;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AgentInfo {
    pub id: String,
    pub label: String,
    pub available: bool,
    pub path: Option<String>,
}

struct Adapter {
    id: &'static str,
    label: &'static str,
    /// The binary looked up on `PATH`; `None` for the always-available default shell.
    bin: Option<&'static str>,
}

const ADAPTERS: &[Adapter] = &[
    Adapter {
        id: "shell",
        label: "Shell",
        bin: None,
    },
    Adapter {
        id: "claude",
        label: "Claude Code",
        bin: Some("claude"),
    },
    Adapter {
        id: "codex",
        label: "Codex",
        bin: Some("codex"),
    },
    Adapter {
        id: "opencode",
        label: "OpenCode",
        bin: Some("opencode"),
    },
];

/// Every adapter with its availability on `path_var`. `shell` reports `default_shell`.
pub fn discover(path_var: Option<&OsStr>, default_shell: &str) -> Vec<AgentInfo> {
    ADAPTERS
        .iter()
        .map(|adapter| {
            let path = match adapter.bin {
                None => Some(default_shell.to_owned()),
                Some(bin) => resolve(bin, path_var).map(|path| path.display().to_string()),
            };
            AgentInfo {
                id: adapter.id.to_owned(),
                label: adapter.label.to_owned(),
                available: path.is_some(),
                path,
            }
        })
        .collect()
}

/// Launch argv for `agent`, resolved fresh: `[absolute resolved path]` for a real
/// agent, `[default_shell]` for `shell`. An unknown id or a binary missing from
/// `path_var` is `invalid_argument`.
pub fn argv_for(
    agent: &str,
    path_var: Option<&OsStr>,
    default_shell: &str,
) -> Result<Vec<String>, BridgeError> {
    let adapter = ADAPTERS
        .iter()
        .find(|adapter| adapter.id == agent)
        .ok_or_else(|| {
            BridgeError::new(
                Code::Daemon(ErrorCode::InvalidArgument),
                format!("unknown agent '{agent}'"),
            )
            .with_hint("pick Shell or a custom command")
        })?;
    match adapter.bin {
        None => Ok(vec![default_shell.to_owned()]),
        Some(bin) => resolve(bin, path_var)
            .map(|path| vec![path.display().to_string()])
            .ok_or_else(|| {
                BridgeError::new(
                    Code::Daemon(ErrorCode::InvalidArgument),
                    format!("{} '{bin}' was not found on PATH", adapter.label),
                )
                .with_hint("install it or pick Shell")
            }),
    }
}

/// The first `name` under `path_var` that is an executable file.
fn resolve(name: &str, path_var: Option<&OsStr>) -> Option<PathBuf> {
    std::env::split_paths(path_var?).find_map(|dir| {
        let hit = dir.join(name);
        executable(&hit).then(|| fs::canonicalize(&hit).unwrap_or(hit))
    })
}

fn executable(path: &Path) -> bool {
    path.is_file()
        && path
            .metadata()
            .is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStrExt;

    /// A `PATH` value naming `dirs` in order, each created under `dir`.
    fn path_var(dirs: &[&Path]) -> OsString {
        std::env::join_paths(dirs.iter().map(|dir| dir.as_os_str())).unwrap()
    }

    fn fake_bin(dir: &TempDir, name: &str, mode: u32) -> PathBuf {
        let file = dir.path().join(name);
        fs::write(&file, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(mode)).unwrap();
        file
    }

    #[test]
    fn a_found_agent_is_available_with_its_resolved_path() {
        let dir = TempDir::new("agents-hit");
        let bin = fake_bin(&dir, "claude", 0o755);
        let path = path_var(&[dir.path()]);

        let agents = discover(Some(&path), "/bin/zsh");

        let claude = agents.iter().find(|agent| agent.id == "claude").unwrap();
        assert!(claude.available);
        assert_eq!(
            claude.path.as_deref(),
            Some(fs::canonicalize(&bin).unwrap().to_str().unwrap())
        );
    }

    #[test]
    fn an_absent_agent_is_unavailable_and_shell_is_always_available() {
        let dir = TempDir::new("agents-miss");
        let path = path_var(&[dir.path()]);

        let agents = discover(Some(&path), "/bin/zsh");

        let codex = agents.iter().find(|agent| agent.id == "codex").unwrap();
        assert_eq!(codex.available, false);
        assert_eq!(codex.path, None);
        let shell = agents.iter().find(|agent| agent.id == "shell").unwrap();
        assert!(shell.available);
        assert_eq!(shell.path.as_deref(), Some("/bin/zsh"));
    }

    #[test]
    fn a_non_executable_file_does_not_count_and_the_first_hit_wins() {
        let dir = TempDir::new("agents-order");
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        let plain = first.join("codex");
        fs::write(&plain, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&plain, fs::Permissions::from_mode(0o644)).unwrap();
        let exec = second.join("codex");
        fs::write(&exec, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&exec, fs::Permissions::from_mode(0o755)).unwrap();
        let path = path_var(&[&first, &second]);

        let resolved = resolve("codex", Some(&path)).unwrap();

        assert_eq!(resolved, fs::canonicalize(&exec).unwrap());
    }

    #[test]
    fn argv_for_resolves_a_present_binary_and_rejects_a_missing_or_unknown_one() {
        let dir = TempDir::new("agents-argv");
        fake_bin(&dir, "opencode", 0o755);
        let path = path_var(&[dir.path()]);

        let argv = argv_for("opencode", Some(&path), "/bin/zsh").unwrap();
        assert_eq!(argv.len(), 1);
        assert!(argv[0].ends_with("/opencode"));

        let missing = argv_for("codex", Some(&path), "/bin/zsh").unwrap_err();
        assert_eq!(missing.code, Code::Daemon(ErrorCode::InvalidArgument));
        assert!(missing.hint.unwrap().contains("Shell"));

        let unknown = argv_for("emacs", Some(&path), "/bin/zsh").unwrap_err();
        assert_eq!(unknown.code, Code::Daemon(ErrorCode::InvalidArgument));

        assert_eq!(
            argv_for("shell", Some(&path), "/bin/fish").unwrap(),
            vec!["/bin/fish".to_owned()]
        );
    }

    #[test]
    fn no_path_var_means_nothing_is_found() {
        let agents = discover(None, "/bin/zsh");
        assert!(agents
            .iter()
            .all(|agent| agent.available == (agent.id == "shell")));
        assert!(argv_for("claude", None, "/bin/zsh").is_err());
        // Sanity check that OsStr bytes flow through: a non-UTF8 dir is still scanned.
        let weird = OsStr::from_bytes(b"\xffdir");
        assert!(resolve("claude", Some(weird)).is_none());
    }
}
