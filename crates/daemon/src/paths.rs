use std::env;
use std::ffi::OsString;
use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

/// Locations of the daemon's files inside its support directory.
#[derive(Debug, Clone)]
pub struct Paths {
    dir: PathBuf,
}

impl Paths {
    /// `GOKO_HOME`, else `~/Library/Application Support/Goko`, created with mode 0700.
    pub fn resolve() -> io::Result<Self> {
        Self::at(dir_from(env::var_os("GOKO_HOME"), env::var_os("HOME"))?)
    }

    /// Use `dir` as the support directory, creating it (and parents) with mode 0700.
    pub fn at(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn lock(&self) -> PathBuf {
        self.dir.join("daemon.lock")
    }

    pub fn socket(&self) -> PathBuf {
        self.dir.join("daemon.sock")
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

fn dir_from(goko_home: Option<OsString>, home: Option<OsString>) -> io::Result<PathBuf> {
    if let Some(dir) = goko_home.filter(|dir| !dir.is_empty()) {
        return Ok(dir.into());
    }
    let home = home.filter(|home| !home.is_empty()).ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "neither GOKO_HOME nor HOME is set")
    })?;
    Ok(PathBuf::from(home).join("Library/Application Support/Goko"))
}

#[cfg(test)]
mod tests {
    use super::{dir_from, Paths};
    use crate::test_support::TempDir;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    #[test]
    fn at_creates_a_0700_directory_and_names_the_four_files() {
        let temp = TempDir::new("paths");
        let paths = Paths::at(temp.path().join("nested/goko")).expect("create paths");

        let mode = std::fs::metadata(paths.dir()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        assert_eq!(paths.lock(), paths.dir().join("daemon.lock"));
        assert_eq!(paths.socket(), paths.dir().join("daemon.sock"));
        assert_eq!(paths.token(), paths.dir().join("daemon.token"));
        assert_eq!(paths.log(), paths.dir().join("daemon.log"));
    }

    #[test]
    fn goko_home_wins_over_the_default_location() {
        let dir = dir_from(Some("/tmp/goko-home".into()), Some("/Users/me".into())).unwrap();
        assert_eq!(dir, PathBuf::from("/tmp/goko-home"));
    }

    #[test]
    fn default_location_is_under_application_support() {
        for goko_home in [None, Some("".into())] {
            let dir = dir_from(goko_home, Some("/Users/me".into())).unwrap();
            assert_eq!(
                dir,
                PathBuf::from("/Users/me/Library/Application Support/Goko")
            );
        }
    }

    #[test]
    fn missing_goko_home_and_home_is_an_error() {
        assert!(dir_from(None, None).is_err());
    }
}
