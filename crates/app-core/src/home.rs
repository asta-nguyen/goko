//! `GOKO_HOME` resolution shared by every Goko frontend.

use crate::error::{BridgeError, Code};
use std::ffi::OsString;
use std::path::PathBuf;

/// `GOKO_HOME`, else `~/Library/Application Support/Goko` (the same rule as the daemon).
pub fn home_dir() -> Result<PathBuf, BridgeError> {
    home_dir_from(std::env::var_os("GOKO_HOME"), std::env::var_os("HOME"))
}

pub fn home_dir_from(
    goko_home: Option<OsString>,
    home: Option<OsString>,
) -> Result<PathBuf, BridgeError> {
    if let Some(dir) = goko_home.filter(|dir| !dir.is_empty()) {
        return Ok(dir.into());
    }
    let home = home.filter(|home| !home.is_empty()).ok_or_else(|| {
        BridgeError::new(Code::DaemonUnavailable, "neither GOKO_HOME nor HOME is set")
    })?;
    Ok(PathBuf::from(home).join("Library/Application Support/Goko"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_prefers_goko_home_then_application_support() {
        let dir = home_dir_from(Some("/tmp/goko".into()), Some("/Users/me".into())).unwrap();
        assert_eq!(dir, PathBuf::from("/tmp/goko"));

        for goko_home in [None, Some("".into())] {
            let dir = home_dir_from(goko_home, Some("/Users/me".into())).unwrap();
            assert_eq!(
                dir,
                PathBuf::from("/Users/me/Library/Application Support/Goko")
            );
        }
        assert!(home_dir_from(None, None).is_err());
    }
}
