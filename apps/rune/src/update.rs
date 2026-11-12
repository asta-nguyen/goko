//! Looks for a newer release: `latest.json` is fetched at startup and every few
//! hours, and Settings → Updates shows what it found. Updating is re-running
//! the installer (`distribution/install.sh`), so the page copies its command.
//! `GOKO_NO_UPDATE_CHECK` turns the background check off; `GOKO_RELEASES_URL`
//! points it elsewhere, as it does for the installer.

use std::process::Command;
#[cfg(not(test))]
use std::time::Duration;

use serde_json::Value;

use crate::State;

/// The version this build is.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

pub const INSTALL_COMMAND: &str =
    "curl -fsSL https://github.com/asta-nguyen/goko/releases/latest/download/install.sh | sh";

#[cfg(not(test))]
const RELEASES: &str = "https://github.com/asta-nguyen/goko/releases/latest/download";

/// How often a running app looks again.
#[cfg(not(test))]
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);

#[cfg(not(test))]
pub fn manifest_url() -> String {
    let base = std::env::var("GOKO_RELEASES_URL").unwrap_or_else(|_| RELEASES.to_string());
    format!("{}/latest.json", base.trim_end_matches('/'))
}

/// `major.minor.patch` and whether a `-suffix` marks a pre-release.
fn parse(version: &str) -> Option<((u64, u64, u64), bool)> {
    let (core, pre) = match version.split_once('-') {
        Some((core, _)) => (core, true),
        None => (version, false),
    };
    let mut parts = core.split('.').map(str::parse::<u64>);
    let numbers = (
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    );
    parts.next().is_none().then_some((numbers, pre))
}

/// Whether `latest` is a newer release than `current`. A pre-release is older
/// than the release it leads up to. Anything unparsable is not newer.
pub fn newer(latest: &str, current: &str) -> bool {
    let (Some((latest, latest_pre)), Some((current, current_pre))) =
        (parse(latest), parse(current))
    else {
        return false;
    };
    (latest, !latest_pre) > (current, !current_pre)
}

/// The release target this build runs on, as `latest.json` and the installer
/// name it; `None` where no release is built (Windows, Intel Macs).
pub fn target() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("macos-aarch64"),
        ("linux", "x86_64") => Some("linux-x86_64"),
        ("linux", "aarch64") => Some("linux-aarch64"),
        _ => None,
    }
}

/// The `version` a `latest.json` names, if it has a build for `target`: a
/// release without one is no update for this machine.
fn version_in(manifest: &str, target: &str) -> Option<String> {
    let manifest: Value = serde_json::from_str(manifest).ok()?;
    manifest["targets"].get(target)?;
    Some(manifest["version"].as_str()?.to_owned())
}

/// The newest version at `url`, if it is newer than this build and built for
/// this machine.
pub fn check(url: &str) -> Result<Option<String>, String> {
    let target = target()
        .ok_or("releases are built for macOS (Apple Silicon) and Linux; build from source")?;
    let output = Command::new("curl")
        .args(["-fsSL", "--max-time", "15", url])
        .output()
        .map_err(|error| format!("curl: {error}"))?;
    if !output.status.success() {
        return Err(format!("could not read {url}"));
    }
    let manifest = String::from_utf8_lossy(&output.stdout);
    let Some(version) = version_in(&manifest, target) else {
        return Err(format!("{url} has no build for {target}"));
    };
    Ok(newer(&version, CURRENT).then_some(version))
}

/// What the last check found.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    /// Nothing has been learned yet (offline, or no release exists).
    #[default]
    Unknown,
    Checking,
    UpToDate,
    Available(String),
    Failed(String),
}

impl State {
    /// What a check found. A failed background check stays silent; one the
    /// user asked for says why.
    pub(crate) fn update_checked(&mut self, result: Result<Option<String>, String>) {
        match result {
            Ok(Some(version)) => {
                if self.update != Status::Available(version.clone()) {
                    self.notice = Some(format!(
                        "Goko {version} is available — see Settings → Updates"
                    ));
                }
                self.update = Status::Available(version);
            }
            Ok(None) => self.update = Status::UpToDate,
            Err(error) if self.update == Status::Checking => self.update = Status::Failed(error),
            Err(_) => {}
        }
    }

    /// Look now, because the user asked.
    pub(crate) fn check_for_update(&mut self) {
        if self.update == Status::Checking {
            return;
        }
        self.update = Status::Checking;
        #[cfg(not(test))]
        if let Some(handle) = crate::APP_HANDLE.get().cloned() {
            check_once(handle);
        }
    }
}

/// One check on its own thread, reported to the app when it ends.
#[cfg(not(test))]
fn check_once(handle: rune::AppHandle<State>) {
    let _ = std::thread::Builder::new()
        .name("goko-update-check".into())
        .spawn(move || {
            let result = check(&manifest_url());
            handle.update(move |s: &mut State, _tx| s.update_checked(result));
        });
}

/// Check at startup and every few hours. A failed check (offline, no release
/// yet) is silent and tried again.
#[cfg(not(test))]
pub fn spawn(handle: rune::AppHandle<State>) {
    if std::env::var_os("GOKO_NO_UPDATE_CHECK").is_some() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("goko-update-loop".into())
        .spawn(move || {
            loop {
                let result = check(&manifest_url());
                handle.update({
                    let result = result.clone();
                    move |s: &mut State, _tx| s.update_checked(result)
                });
                std::thread::sleep(EVERY);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_higher_release_is_newer() {
        assert!(newer("0.2.0", "0.1.0"));
        assert!(newer("0.1.10", "0.1.9"), "numbers, not text");
        assert!(newer("1.0.0", "0.9.9"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.1.0", "0.2.0"));
    }

    #[test]
    fn a_pre_release_is_older_than_its_release() {
        assert!(
            newer("0.2.0", "0.2.0-rc1"),
            "the release follows its candidate"
        );
        assert!(!newer("0.2.0-rc1", "0.2.0"));
        assert!(newer("0.3.0-rc1", "0.2.0"));
    }

    #[test]
    fn nonsense_is_never_newer() {
        assert!(!newer("latest", "0.1.0"));
        assert!(!newer("0.2", "0.1.0"));
        assert!(!newer("0.2.0", ""));
    }

    #[test]
    fn the_manifest_names_its_version_for_a_target_it_builds() {
        let json = r#"{"version":"0.3.1","targets":{"macos-aarch64":{"url":"u","sha256":"s"}}}"#;
        assert_eq!(version_in(json, "macos-aarch64").as_deref(), Some("0.3.1"));
        assert_eq!(version_in(json, "linux-x86_64"), None, "no build for it");
        assert_eq!(version_in("{}", "macos-aarch64"), None);
        assert_eq!(version_in("not json", "macos-aarch64"), None);
    }

    #[test]
    fn this_machine_has_a_release_target_on_the_supported_systems() {
        let expected = matches!(
            (std::env::consts::OS, std::env::consts::ARCH),
            ("macos", "aarch64") | ("linux", "x86_64" | "aarch64")
        );
        assert_eq!(target().is_some(), expected);
    }

    #[test]
    fn check_reads_a_manifest_and_reports_only_a_newer_one_built_for_here() {
        let Some(target) = target() else { return };
        let dir = std::env::temp_dir().join(format!("goko-update-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let url = |name: &str, version: &str, target: &str| {
            let path = dir.join(name);
            let json = format!(r#"{{"version":"{version}","targets":{{"{target}":{{}}}}}}"#);
            std::fs::write(&path, json).unwrap();
            format!("file://{}", path.display())
        };
        assert_eq!(
            check(&url("new.json", "99.0.0", target)),
            Ok(Some("99.0.0".into()))
        );
        assert_eq!(check(&url("same.json", CURRENT, target)), Ok(None));
        assert!(
            check(&url("other.json", "99.0.0", "plan9-mips")).is_err(),
            "a release without a build for this machine is no update"
        );
        assert!(check("file:///nonexistent/latest.json").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_found_release_is_announced_once_and_kept() {
        let mut s = State::new(crate::test_handle());
        s.update_checked(Ok(Some("9.9.9".into())));
        assert_eq!(s.update, Status::Available("9.9.9".into()));
        assert!(s.notice.as_deref().is_some_and(|n| n.contains("9.9.9")));
        s.notice = None;
        s.update_checked(Ok(Some("9.9.9".into())));
        assert!(
            s.notice.is_none(),
            "the same release is not announced again"
        );
        s.update_checked(Err("offline".into()));
        assert_eq!(
            s.update,
            Status::Available("9.9.9".into()),
            "a failed recheck keeps it"
        );
    }

    #[test]
    fn a_failure_is_shown_only_when_the_user_asked() {
        let mut s = State::new(crate::test_handle());
        s.update_checked(Err("offline".into()));
        assert_eq!(s.update, Status::Unknown, "a background failure is silent");
        s.check_for_update();
        assert_eq!(s.update, Status::Checking);
        s.update_checked(Err("offline".into()));
        assert_eq!(s.update, Status::Failed("offline".into()));
        s.check_for_update();
        s.update_checked(Ok(None));
        assert_eq!(s.update, Status::UpToDate);
    }
}
