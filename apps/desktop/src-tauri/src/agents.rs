//! Local agent adapters resolved on `PATH` at launch time.
//!
//! An adapter is just an argv0 name: `claude`, `codex`, `opencode`, `devin`, or the user's
//! default shell, which is always available. Resolution happens when a session is
//! created, never earlier, so a binary that appears or disappears between `agent_list`
//! and Start is handled on the launch path.

use crate::accounts::LaunchAccount;
use crate::error::{BridgeError, Code};
use protocol::message::ErrorCode;
use serde::Serialize;
use serde_json::{json, Value};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// A Finder-launched app gets launchd's bare `PATH`, so `claude`/`codex` are not found.
/// Ask the user's login shell for its `PATH` and adopt it; the daemon and every
/// session inherit it. Markers skip any noise printed by rc files.
pub fn adopt_login_shell_path(shell: &str) {
    let Ok(output) = std::process::Command::new(shell)
        .args(["-ilc", "printf '__GOKO__%s__GOKO__' \"$PATH\""])
        .stdin(std::process::Stdio::null())
        .output()
    else {
        return;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    if let Some(path) = text
        .split("__GOKO__")
        .nth(1)
        .filter(|path| !path.is_empty())
    {
        std::env::set_var("PATH", path);
    }
}

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
    Adapter {
        id: "devin",
        label: "Devin",
        bin: Some("devin"),
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
/// agent, `[default_shell]` for `shell`. A named Claude or Codex account prepends
/// `/usr/bin/env NAME=<dir>`; Claude also captures usage under the account id.
/// An unknown id or a binary missing from `path_var` is `invalid_argument`.
pub fn argv_for(
    agent: &str,
    path_var: Option<&OsStr>,
    default_shell: &str,
    goko_home: &Path,
    original_statusline: Option<&str>,
    account: &LaunchAccount,
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
        Some(bin) => {
            let path = resolve(bin, path_var).ok_or_else(|| {
                BridgeError::new(
                    Code::Daemon(ErrorCode::InvalidArgument),
                    format!("{} '{bin}' was not found on PATH", adapter.label),
                )
                .with_hint("install it or pick Shell")
            })?;
            // A named Claude or Codex account sets its folder variable through
            // `env`; the dir stays a single argv element even with spaces.
            let env_word = match (agent, account.dir.as_ref()) {
                ("claude", Some(dir)) => Some(format!("CLAUDE_CONFIG_DIR={}", dir.display())),
                ("codex", Some(dir)) => Some(format!("CODEX_HOME={}", dir.display())),
                _ => None,
            };
            if agent != "claude" {
                return Ok(match env_word {
                    Some(word) => vec!["/usr/bin/env".to_owned(), word, path.display().to_string()],
                    None => vec![path.display().to_string()],
                });
            }

            let session_id = Uuid::new_v4().to_string();
            // A named account captures under its id; Default keeps the flat folder.
            let usage_dir = goko_home.join("usage/claude");
            let usage_dir = if account.dir.is_some() {
                usage_dir.join(&account.id)
            } else {
                usage_dir
            };
            fs::create_dir_all(&usage_dir).map_err(|error| {
                BridgeError::new(
                    Code::Storage,
                    format!("cannot create Claude usage directory: {error}"),
                )
                .with_hint("check permissions for GOKO_HOME")
            })?;
            let activity_dir = goko_home.join("activity/claude");
            fs::create_dir_all(&activity_dir).map_err(|error| {
                BridgeError::new(
                    Code::Storage,
                    format!("cannot create Claude activity directory: {error}"),
                )
                .with_hint("check permissions for GOKO_HOME")
            })?;
            let statusline = claude_statusline_command(
                &usage_dir.join(format!("{session_id}.json")),
                original_statusline,
            )?;
            let activity_file = activity_dir.join(&session_id);
            let hook = |word: &str| -> Result<Value, BridgeError> {
                Ok(json!([{
                    "hooks": [{
                        "type": "command",
                        "command": claude_hook_command(&activity_file, word)?,
                    }]
                }]))
            };
            let settings = json!({
                "statusLine": {
                    "type": "command",
                    "command": statusline,
                },
                "hooks": {
                    "UserPromptSubmit": hook("working")?,
                    "Notification": hook("waiting")?,
                    "Stop": hook("done")?,
                    "PostToolUse": hook("working")?,
                }
            });
            let mut argv = match env_word {
                Some(word) => vec!["/usr/bin/env".to_owned(), word],
                None => Vec::new(),
            };
            argv.extend([
                path.display().to_string(),
                "--session-id".to_owned(),
                session_id,
                "--settings".to_owned(),
                settings.to_string(),
            ]);
            Ok(argv)
        }
    }
}

pub(crate) fn original_statusline(settings_path: &Path) -> Option<String> {
    let settings: Value = serde_json::from_str(&fs::read_to_string(settings_path).ok()?).ok()?;
    settings
        .pointer("/statusLine/command")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn claude_statusline_command(path: &Path, original: Option<&str>) -> Result<String, BridgeError> {
    let quote = |value: &str| shell_quote(value, "statusline");
    let path = quote(&path.display().to_string())?;
    Ok(match original {
        Some(command) => format!("tee {path} | sh -c {}", quote(command)?),
        None => format!("tee {path} >/dev/null"),
    })
}

/// A Claude event hook: write `word` to `path`, print nothing, always exit 0.
fn claude_hook_command(path: &Path, word: &str) -> Result<String, BridgeError> {
    let path = shell_quote(&path.display().to_string(), "activity hook")?;
    Ok(format!("printf %s {word} >{path} 2>/dev/null; exit 0"))
}

fn shell_quote(value: &str, what: &str) -> Result<String, BridgeError> {
    shlex::try_quote(value)
        .map(|value| value.into_owned())
        .map_err(|_| {
            BridgeError::new(
                Code::Daemon(ErrorCode::InvalidArgument),
                format!("Claude {what} command cannot be shell-quoted"),
            )
        })
}

/// The first `name` under `path_var` that is an executable file.
pub(crate) fn resolve(name: &str, path_var: Option<&OsStr>) -> Option<PathBuf> {
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

    /// The implicit Default account every pre-accounts launch resolves to.
    fn default_account() -> LaunchAccount {
        LaunchAccount {
            id: "default".into(),
            dir: None,
        }
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

        let argv = argv_for(
            "opencode",
            Some(&path),
            "/bin/zsh",
            dir.path(),
            None,
            &default_account(),
        )
        .unwrap();
        assert_eq!(argv.len(), 1);
        assert!(argv[0].ends_with("/opencode"));

        let missing = argv_for(
            "codex",
            Some(&path),
            "/bin/zsh",
            dir.path(),
            None,
            &default_account(),
        )
        .unwrap_err();
        assert_eq!(missing.code, Code::Daemon(ErrorCode::InvalidArgument));
        assert!(missing.hint.unwrap().contains("Shell"));

        let unknown = argv_for(
            "emacs",
            Some(&path),
            "/bin/zsh",
            dir.path(),
            None,
            &default_account(),
        )
        .unwrap_err();
        assert_eq!(unknown.code, Code::Daemon(ErrorCode::InvalidArgument));

        assert_eq!(
            argv_for(
                "shell",
                Some(&path),
                "/bin/fish",
                dir.path(),
                None,
                &default_account()
            )
            .unwrap(),
            vec!["/bin/fish".to_owned()]
        );

        let codex_dir = TempDir::new("agents-codex");
        let codex_bin = fake_bin(&codex_dir, "codex", 0o755);
        let codex_path = path_var(&[codex_dir.path()]);
        assert_eq!(
            argv_for(
                "codex",
                Some(&codex_path),
                "/bin/zsh",
                dir.path(),
                None,
                &default_account()
            )
            .unwrap(),
            vec![fs::canonicalize(codex_bin).unwrap().display().to_string()]
        );
    }

    #[test]
    fn no_path_var_means_nothing_is_found() {
        let agents = discover(None, "/bin/zsh");
        assert!(agents
            .iter()
            .all(|agent| agent.available == (agent.id == "shell")));
        assert!(argv_for(
            "claude",
            None,
            "/bin/zsh",
            Path::new("/tmp"),
            None,
            &default_account()
        )
        .is_err());
        // Sanity check that OsStr bytes flow through: a non-UTF8 dir is still scanned.
        let weird = OsStr::from_bytes(b"\xffdir");
        assert!(resolve("claude", Some(weird)).is_none());
    }

    #[test]
    fn claude_argv_has_a_v4_session_id_and_capture_settings() {
        let dir = TempDir::new("agents-claude");
        let bin = fake_bin(&dir, "claude", 0o755);
        let path = path_var(&[dir.path()]);

        let argv = argv_for(
            "claude",
            Some(&path),
            "/bin/zsh",
            dir.path(),
            None,
            &default_account(),
        )
        .unwrap();

        assert_eq!(argv.len(), 5);
        assert_eq!(
            argv[0],
            fs::canonicalize(bin).unwrap().display().to_string()
        );
        assert_eq!(argv[1], "--session-id");
        let id = Uuid::parse_str(&argv[2]).unwrap();
        assert_eq!(id.get_version_num(), 4);
        assert_eq!(argv[3], "--settings");
        let settings: Value = serde_json::from_str(&argv[4]).unwrap();
        let capture = settings
            .pointer("/statusLine/command")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(dir.path().join("usage/claude").is_dir());
        assert!(capture.contains(&format!("usage/claude/{id}.json")));
        assert_eq!(settings.pointer("/statusLine/type").unwrap(), "command");
        assert_eq!(
            shlex::split(capture).unwrap(),
            vec![
                "tee".to_owned(),
                dir.path()
                    .join("usage/claude")
                    .join(format!("{id}.json"))
                    .display()
                    .to_string(),
                ">/dev/null".to_owned(),
            ]
        );
    }

    #[test]
    fn claude_argv_injects_activity_hooks_for_the_session() {
        let dir = TempDir::new("agents-claude-hooks");
        fake_bin(&dir, "claude", 0o755);
        let path = path_var(&[dir.path()]);

        let argv = argv_for(
            "claude",
            Some(&path),
            "/bin/zsh",
            dir.path(),
            None,
            &default_account(),
        )
        .unwrap();
        let id = &argv[2];
        let settings: Value = serde_json::from_str(&argv[4]).unwrap();
        let file = dir.path().join("activity/claude").join(id);

        assert!(dir.path().join("activity/claude").is_dir());
        for (event, word) in [
            ("UserPromptSubmit", "working"),
            ("Notification", "waiting"),
            ("Stop", "done"),
            ("PostToolUse", "working"),
        ] {
            let hook = settings
                .pointer(&format!("/hooks/{event}/0/hooks/0"))
                .unwrap_or_else(|| panic!("missing hook for {event}"));
            assert_eq!(hook.pointer("/type").unwrap(), "command");
            let command = hook.pointer("/command").unwrap().as_str().unwrap();
            assert!(
                command.starts_with(&format!("printf %s {word} >"))
                    && command.contains(&file.display().to_string())
                    && command.ends_with(" 2>/dev/null; exit 0"),
                "unexpected hook command for {event}: {command}"
            );
        }
    }

    #[test]
    fn claude_activity_hook_writes_its_word_and_always_exits_zero() {
        let dir = TempDir::new("agents-claude-hook-run");
        fake_bin(&dir, "claude", 0o755);
        let path = path_var(&[dir.path()]);
        let home = dir.path().join("goko home");
        fs::create_dir_all(&home).unwrap();

        let argv = argv_for(
            "claude",
            Some(&path),
            "/bin/zsh",
            &home,
            None,
            &default_account(),
        )
        .unwrap();
        let settings: Value = serde_json::from_str(&argv[4]).unwrap();
        let command = settings
            .pointer("/hooks/Stop/0/hooks/0/command")
            .unwrap()
            .as_str()
            .unwrap();

        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .output()
            .unwrap();

        assert_eq!(output.status.code(), Some(0));
        assert!(output.stdout.is_empty() && output.stderr.is_empty());
        assert_eq!(
            fs::read_to_string(home.join("activity/claude").join(&argv[2])).unwrap(),
            "done"
        );
    }

    #[test]
    fn claude_statusline_preserves_a_quoted_original_command() {
        let dir = TempDir::new("agents-claude-original");
        let bin = fake_bin(&dir, "claude", 0o755);
        let path = path_var(&[dir.path()]);
        let original = "my tool --message 'it's ready'";

        let argv = argv_for(
            "claude",
            Some(&path),
            "/bin/zsh",
            dir.path(),
            Some(original),
            &default_account(),
        )
        .unwrap();
        let id = &argv[2];
        let settings: Value = serde_json::from_str(&argv[4]).unwrap();
        let command = settings
            .pointer("/statusLine/command")
            .unwrap()
            .as_str()
            .unwrap();

        assert_eq!(
            argv[0],
            fs::canonicalize(bin).unwrap().display().to_string()
        );
        assert_eq!(
            shlex::split(command).unwrap(),
            vec![
                "tee".to_owned(),
                dir.path()
                    .join("usage/claude")
                    .join(format!("{id}.json"))
                    .display()
                    .to_string(),
                "|".to_owned(),
                "sh".to_owned(),
                "-c".to_owned(),
                original.to_owned(),
            ]
        );
    }

    #[test]
    fn original_statusline_is_read_only_from_the_user_settings_file() {
        let dir = TempDir::new("agents-settings");
        let settings = dir.path().join("settings.json");

        fs::write(&settings, r#"{"statusLine":{"command":"my statusline"}}"#).unwrap();
        assert_eq!(
            original_statusline(&settings).as_deref(),
            Some("my statusline")
        );

        fs::write(&settings, "not json").unwrap();
        assert_eq!(original_statusline(&settings), None);

        fs::remove_file(&settings).unwrap();
        assert_eq!(original_statusline(&settings), None);
    }

    #[test]
    fn a_named_claude_account_prefixes_env_and_captures_under_its_id() {
        let dir = TempDir::new("agents-claude-acct");
        let bin = fake_bin(&dir, "claude", 0o755);
        let path = path_var(&[dir.path()]);
        let goko_home = dir.path().join("goko-home");
        fs::create_dir_all(&goko_home).unwrap();
        // The space proves the dir stays one argv element.
        let account_dir = dir.path().join("account dir");
        fs::create_dir_all(&account_dir).unwrap();
        let account = LaunchAccount {
            id: "work".into(),
            dir: Some(account_dir.clone()),
        };

        let argv = argv_for(
            "claude",
            Some(&path),
            "/bin/zsh",
            &goko_home,
            None,
            &account,
        )
        .unwrap();

        assert_eq!(argv.len(), 7);
        assert_eq!(argv[0], "/usr/bin/env");
        assert_eq!(
            argv[1],
            format!("CLAUDE_CONFIG_DIR={}", account_dir.display())
        );
        assert_eq!(
            argv[2],
            fs::canonicalize(bin).unwrap().display().to_string()
        );
        assert_eq!(argv[3], "--session-id");
        let id = Uuid::parse_str(&argv[4]).unwrap();
        assert_eq!(id.get_version_num(), 4);
        assert_eq!(argv[5], "--settings");
        let settings: Value = serde_json::from_str(&argv[6]).unwrap();
        let capture = settings
            .pointer("/statusLine/command")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(capture.contains(&format!("usage/claude/work/{id}.json")));
        assert!(goko_home.join("usage/claude/work").is_dir());
        // The activity file path is unchanged: activity/claude/<session-id>.
        let hook = settings
            .pointer("/hooks/Stop/0/hooks/0/command")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(hook.contains(
            &goko_home
                .join("activity/claude")
                .join(&argv[4])
                .display()
                .to_string()
        ));
    }

    #[test]
    fn a_named_codex_account_prefixes_env_with_its_home() {
        let dir = TempDir::new("agents-codex-acct");
        let bin = fake_bin(&dir, "codex", 0o755);
        let path = path_var(&[dir.path()]);
        let account_dir = dir.path().join("codex account");
        fs::create_dir_all(&account_dir).unwrap();
        let account = LaunchAccount {
            id: "work".into(),
            dir: Some(account_dir.clone()),
        };

        let argv = argv_for("codex", Some(&path), "/bin/zsh", dir.path(), None, &account).unwrap();

        assert_eq!(
            argv,
            vec![
                "/usr/bin/env".to_owned(),
                format!("CODEX_HOME={}", account_dir.display()),
                fs::canonicalize(bin).unwrap().display().to_string(),
            ]
        );
    }

    #[test]
    fn opencode_devin_and_shell_ignore_a_named_account() {
        let dir = TempDir::new("agents-other-acct");
        fake_bin(&dir, "opencode", 0o755);
        fake_bin(&dir, "devin", 0o755);
        let path = path_var(&[dir.path()]);
        let account_dir = dir.path().join("account dir");
        fs::create_dir_all(&account_dir).unwrap();
        let named = LaunchAccount {
            id: "work".into(),
            dir: Some(account_dir),
        };

        for agent in ["opencode", "devin"] {
            let default = argv_for(
                agent,
                Some(&path),
                "/bin/zsh",
                dir.path(),
                None,
                &default_account(),
            )
            .unwrap();
            let launched =
                argv_for(agent, Some(&path), "/bin/zsh", dir.path(), None, &named).unwrap();
            assert_eq!(launched, default, "{agent}");
            assert_eq!(launched.len(), 1, "{agent}");
        }
        assert_eq!(
            argv_for("shell", Some(&path), "/bin/fish", dir.path(), None, &named).unwrap(),
            vec!["/bin/fish".to_owned()]
        );
    }
}
