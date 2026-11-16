use std::env;
use std::ffi::OsString;
use std::fs::{self, DirBuilder};
use std::io;
use std::path::{Path, PathBuf};

/// Locations of the daemon's files inside its support directory.
#[derive(Debug, Clone)]
pub struct Paths {
    dir: PathBuf,
}

impl Paths {
    /// `GOKO_HOME`, else the platform's default folder (see [`dir_from`]),
    /// created with mode 0700.
    pub fn resolve() -> io::Result<Self> {
        #[cfg(windows)]
        {
            return Self::at(dir_from_windows(
                env::var_os("GOKO_HOME"),
                env::var_os("LOCALAPPDATA"),
                env::var_os("USERPROFILE"),
            )?);
        }
        #[cfg(not(windows))]
        Self::at(dir_from(
            env::var_os("GOKO_HOME"),
            env::var_os("HOME"),
            env::var_os("XDG_STATE_HOME"),
        )?)
    }

    /// Use `dir` as the support directory, creating it (and parents) with mode 0700.
    pub fn at(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
        }
        #[cfg(not(unix))]
        {
            let _ = DirBuilder::new();
            fs::create_dir_all(&dir)?;
        }
        Ok(Self { dir })
    }

    /// The support directory itself (pipe-name derivation on Windows).
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn lock(&self) -> PathBuf {
        self.dir.join("daemon.lock")
    }

    pub fn worktree_operations_lock(&self) -> PathBuf {
        self.dir.join("worktree-operations.lock")
    }

    pub fn socket(&self) -> PathBuf {
        self.dir.join("daemon.sock")
    }

    /// The transport endpoint to log: the socket path on Unix, the named-pipe
    /// name on Windows.
    pub fn endpoint(&self) -> String {
        #[cfg(unix)]
        {
            self.socket().display().to_string()
        }
        #[cfg(windows)]
        {
            crate::net::pipe_name(&self.dir)
        }
    }

    pub fn token(&self) -> PathBuf {
        self.dir.join("daemon.token")
    }

    pub fn log(&self) -> PathBuf {
        self.dir.join("daemon.log")
    }
}

/// Remove `path`, treating a missing file as success.
pub(crate) fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// The folder Goko first used on every platform, and still uses on macOS.
#[cfg(unix)]
const APPLICATION_SUPPORT: &str = "Library/Application Support/Goko";

/// `GOKO_HOME`, else the platform's default folder (the same rule as
/// `app_core::home`): `~/Library/Application Support/Goko` on macOS;
/// elsewhere `$XDG_STATE_HOME/goko`, or `~/.local/state/goko` when that
/// variable is unset or not absolute, unless the macOS-style folder already
/// exists, which an older install keeps using.
#[cfg(unix)]
fn dir_from(
    goko_home: Option<OsString>,
    home: Option<OsString>,
    xdg_state_home: Option<OsString>,
) -> io::Result<PathBuf> {
    if let Some(dir) = goko_home.filter(|dir| !dir.is_empty()) {
        return Ok(dir.into());
    }
    let home = home.filter(|home| !home.is_empty()).ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "neither GOKO_HOME nor HOME is set")
    })?;
    Ok(default_dir(
        Path::new(&home),
        xdg_state_home,
        cfg!(target_os = "macos"),
    ))
}

/// `GOKO_HOME`, else `%LOCALAPPDATA%\Goko`, else
/// `%USERPROFILE%\AppData\Local\Goko` (the same rule as `app_core::home`).
#[cfg(windows)]
fn dir_from_windows(
    goko_home: Option<OsString>,
    local_app_data: Option<OsString>,
    user_profile: Option<OsString>,
) -> io::Result<PathBuf> {
    if let Some(dir) = goko_home.filter(|dir| !dir.is_empty()) {
        return Ok(dir.into());
    }
    if let Some(local) = local_app_data.filter(|dir| !dir.is_empty()) {
        return Ok(PathBuf::from(local).join("Goko"));
    }
    let profile = user_profile.filter(|dir| !dir.is_empty()).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "neither GOKO_HOME, LOCALAPPDATA nor USERPROFILE is set",
        )
    })?;
    Ok(PathBuf::from(profile).join("AppData/Local/Goko"))
}

#[cfg(unix)]
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
    use super::{default_dir, dir_from, Paths, APPLICATION_SUPPORT};
    use crate::test_support::TempDir;
    use std::path::{Path, PathBuf};

    #[test]
    fn at_creates_a_0700_directory_and_names_the_four_files() {
        let temp = TempDir::new("paths");
        let dir = temp.path().join("nested/goko");
        let paths = Paths::at(&dir).expect("create paths");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        assert_eq!(paths.lock(), dir.join("daemon.lock"));
        assert_eq!(
            paths.worktree_operations_lock(),
            dir.join("worktree-operations.lock")
        );
        assert_eq!(paths.socket(), dir.join("daemon.sock"));
        assert_eq!(paths.token(), dir.join("daemon.token"));
        assert_eq!(paths.log(), dir.join("daemon.log"));
    }

    #[test]
    fn goko_home_wins_over_the_default_location() {
        let dir = dir_from(
            Some("/tmp/goko-home".into()),
            Some("/Users/me".into()),
            None,
        )
        .unwrap();
        assert_eq!(dir, PathBuf::from("/tmp/goko-home"));
    }

    #[test]
    fn macos_default_is_under_application_support() {
        assert_eq!(
            default_dir(Path::new("/Users/me"), Some("/elsewhere".into()), true),
            PathBuf::from("/Users/me/Library/Application Support/Goko")
        );
    }

    #[test]
    fn other_platforms_default_to_the_xdg_state_folder() {
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
        let home = TempDir::new("paths-legacy");
        let legacy = home.path().join(APPLICATION_SUPPORT);
        std::fs::create_dir_all(&legacy).unwrap();
        assert_eq!(
            default_dir(home.path(), Some("/var/state".into()), false),
            legacy
        );
    }

    #[test]
    fn an_empty_goko_home_falls_back_to_the_default() {
        let dir = dir_from(
            Some("".into()),
            Some("/home/goko-no-such-user".into()),
            None,
        )
        .unwrap();
        assert!(dir.starts_with("/home/goko-no-such-user"));
    }

    #[test]
    fn missing_goko_home_and_home_is_an_error() {
        assert!(dir_from(None, None, None).is_err());
    }
}
