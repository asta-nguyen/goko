//! `GOKO_HOME` resolution shared by every Goko frontend.

use crate::error::{BridgeError, Code};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The folder Goko first used on every platform, and still uses on macOS.
const APPLICATION_SUPPORT: &str = "Library/Application Support/Goko";

/// `GOKO_HOME`, else the platform's default folder (the same rule as the
/// daemon's `paths::dir_from`):
///
/// - macOS: `~/Library/Application Support/Goko`.
/// - elsewhere: `$XDG_STATE_HOME/goko`, or `~/.local/state/goko` when that
///   variable is unset or not absolute. An install that already has the
///   macOS-style folder keeps using it, so its data stays where it is.
///
/// Not `$XDG_DATA_HOME/goko`: the installer keeps the binaries there and
/// replaces that folder on every update.
pub fn home_dir() -> Result<PathBuf, BridgeError> {
    home_dir_from(
        std::env::var_os("GOKO_HOME"),
        std::env::var_os("HOME"),
        std::env::var_os("XDG_STATE_HOME"),
    )
}

pub fn home_dir_from(
    goko_home: Option<OsString>,
    home: Option<OsString>,
    xdg_state_home: Option<OsString>,
) -> Result<PathBuf, BridgeError> {
    if let Some(dir) = goko_home.filter(|dir| !dir.is_empty()) {
        return Ok(dir.into());
    }
    let home = home.filter(|home| !home.is_empty()).ok_or_else(|| {
        BridgeError::new(Code::DaemonUnavailable, "neither GOKO_HOME nor HOME is set")
    })?;
    Ok(default_dir(
        Path::new(&home),
        xdg_state_home,
        cfg!(target_os = "macos"),
    ))
}

fn default_dir(home: &Path, xdg_state_home: Option<OsString>, macos: bool) -> PathBuf {
    let application_support = home.join(APPLICATION_SUPPORT);
    if macos || application_support.is_dir() {
        return application_support;
    }
    match xdg_state_home
        .map(PathBuf::from)
        .filter(|state| state.is_absolute())
    {
        Some(state) => state.join("goko"),
        None => home.join(".local/state/goko"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn goko_home_wins_and_a_missing_home_is_an_error() {
        let dir = home_dir_from(Some("/tmp/goko".into()), Some("/home/me".into()), None).unwrap();
        assert_eq!(dir, PathBuf::from("/tmp/goko"));
        assert!(home_dir_from(None, None, None).is_err());
        assert!(home_dir_from(Some("".into()), Some("".into()), None).is_err());
    }

    #[test]
    fn macos_uses_application_support() {
        let home = Path::new("/Users/me");
        assert_eq!(
            default_dir(home, Some("/elsewhere".into()), true),
            PathBuf::from("/Users/me/Library/Application Support/Goko")
        );
    }

    #[test]
    fn other_platforms_use_the_xdg_state_folder() {
        let home = Path::new("/home/goko-no-such-user");
        assert_eq!(
            default_dir(home, None, false),
            PathBuf::from("/home/goko-no-such-user/.local/state/goko")
        );
        assert_eq!(
            default_dir(home, Some("/var/state".into()), false),
            PathBuf::from("/var/state/goko")
        );
        // The XDG spec says to ignore a relative path.
        assert_eq!(
            default_dir(home, Some("state".into()), false),
            PathBuf::from("/home/goko-no-such-user/.local/state/goko")
        );
    }

    #[test]
    fn an_existing_application_support_folder_is_kept() {
        let home = TempDir::new("home-legacy");
        let legacy = home.path().join(APPLICATION_SUPPORT);
        std::fs::create_dir_all(&legacy).unwrap();
        assert_eq!(
            default_dir(home.path(), Some("/var/state".into()), false),
            legacy
        );
    }
}
