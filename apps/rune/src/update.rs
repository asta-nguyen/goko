//! Looks for a newer release: `latest.json` is fetched at startup and every few
//! hours, and Settings → Updates shows what it found and what changed. "Update
//! now" re-runs the installer (`distribution/install.sh`) over the copy that is
//! running; a copy it cannot replace (see [`install_dir`]) gets the installer's
//! command to copy instead. `GOKO_NO_UPDATE_CHECK` turns the background check
//! off; `GOKO_RELEASES_URL` points it elsewhere, as it does for the installer.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
fn base_url() -> String {
    std::env::var("GOKO_RELEASES_URL").unwrap_or_else(|_| RELEASES.to_string())
}

#[cfg(not(test))]
pub fn manifest_url() -> String {
    format!("{}/latest.json", base_url().trim_end_matches('/'))
}

/// A release newer than this build: its version and what changed in it (the
/// `CHANGELOG.md` entry `latest.json` carries; empty when it carries none).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Release {
    pub version: String,
    pub notes: String,
}

#[cfg(test)]
impl From<&str> for Release {
    fn from(version: &str) -> Self {
        Release {
            version: version.into(),
            notes: String::new(),
        }
    }
}

/// The notes as rows to show: `(true, heading)` for a `### Heading`, and
/// `(false, text)` for a bullet, its wrapped lines joined into one paragraph.
pub fn note_rows(notes: &str) -> Vec<(bool, String)> {
    let mut rows: Vec<(bool, String)> = Vec::new();
    for line in notes.lines().map(|line| line.replace('`', "")) {
        let line = line.trim_end();
        if let Some(heading) = line.strip_prefix("### ") {
            rows.push((true, heading.trim().to_owned()));
        } else if let Some(item) = line.strip_prefix("- ") {
            rows.push((false, item.trim().to_owned()));
        } else if !line.trim().is_empty() {
            match rows.last_mut() {
                Some((false, text)) => {
                    text.push(' ');
                    text.push_str(line.trim());
                }
                _ => rows.push((false, line.trim().to_owned())),
            }
        }
    }
    rows
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

/// The release a `latest.json` names, if it has a build for `target`: a
/// release without one is no update for this machine.
fn release_in(manifest: &str, target: &str) -> Option<Release> {
    let manifest: Value = serde_json::from_str(manifest).ok()?;
    manifest["targets"].get(target)?;
    Some(Release {
        version: manifest["version"].as_str()?.to_owned(),
        notes: manifest["notes"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_owned(),
    })
}

/// The newest release at `url`, if it is newer than this build and built for
/// this machine.
pub fn check(url: &str) -> Result<Option<Release>, String> {
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
    let Some(release) = release_in(&manifest, target) else {
        return Err(format!("{url} has no build for {target}"));
    };
    Ok(newer(&release.version, CURRENT).then_some(release))
}

/// The folder an update installs into so that it replaces this running copy:
/// the one holding `Goko.app` on macOS, the data folder `install.sh` keeps
/// `goko` in on Linux. `None` for a copy the installer would not replace (built
/// from source, renamed, run from a disk image) or cannot write to: that one
/// is updated by hand.
pub fn install_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let data = std::env::var_os("XDG_DATA_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".local/share")))?;
    let dir = dir_of(&exe, &data)?;
    writable(dir).then(|| dir.to_path_buf())
}

/// The install folder of the copy whose executable is `exe`, when it sits
/// where `install.sh` puts it: `<dir>/Goko.app/Contents/MacOS/goko-rune`, or
/// `<data>/goko/goko-rune`.
fn dir_of<'a>(exe: &'a Path, data: &Path) -> Option<&'a Path> {
    if cfg!(target_os = "macos") {
        let app = exe.ancestors().nth(3)?;
        return (app.file_name()? == "Goko.app").then_some(app)?.parent();
    }
    let home = exe.parent()?;
    let installed = data.join("goko").canonicalize().ok()?;
    (home.canonicalize().ok()? == installed)
        .then_some(home)?
        .parent()
}

/// Whether files can be made and removed in `dir`, which is what replacing
/// the app there takes. Found out before the installer removes anything.
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".goko-update-{}", std::process::id()));
    let ok = std::fs::File::create(&probe).is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Runs the installer published at `base` so that it replaces the copy in
/// `dir`. A failure is the installer's own last message.
pub fn install(base: &str, dir: &Path) -> Result<(), String> {
    let url = format!("{}/install.sh", base.trim_end_matches('/'));
    let script = Command::new("curl")
        .args(["-fsSL", "--max-time", "60", &url])
        .output()
        .map_err(|error| format!("curl: {error}"))?;
    if !script.status.success() {
        return Err(format!("could not read {url}"));
    }
    let output = Command::new("sh")
        .arg("-c")
        .arg(String::from_utf8_lossy(&script.stdout).as_ref())
        .env("GOKO_APP_DIR", dir)
        .env("GOKO_RELEASES_URL", base)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("sh: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&output.stderr);
    Err(said
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("the installer failed")
        .to_owned())
}

/// What the last check found.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    /// Nothing has been learned yet (offline, or no release exists).
    #[default]
    Unknown,
    Checking,
    UpToDate,
    Available(Release),
    /// "Update now" is running the installer.
    Installing(Release),
    /// The installer finished; what is running is still the old version.
    Installed(Release),
    Failed(String),
}

impl Status {
    /// The release a newer one was found, being installed, or installed.
    pub fn release(&self) -> Option<&Release> {
        match self {
            Status::Available(release)
            | Status::Installing(release)
            | Status::Installed(release) => Some(release),
            _ => None,
        }
    }
}

impl State {
    /// What a check found. A failed background check stays silent; one the
    /// user asked for says why. An install in progress or done is not
    /// disturbed: the running copy is still the old one, so a check would
    /// find the same release again.
    pub(crate) fn update_checked(&mut self, result: Result<Option<Release>, String>) {
        if matches!(self.update, Status::Installing(_) | Status::Installed(_)) {
            return;
        }
        match result {
            Ok(Some(release)) => {
                let known = matches!(&self.update, Status::Available(old) if old.version == release.version);
                if !known {
                    self.notice = Some(format!(
                        "Goko {} is available — see Settings → Updates",
                        release.version
                    ));
                }
                self.update = Status::Available(release);
            }
            Ok(None) => self.update = Status::UpToDate,
            Err(error) if self.update == Status::Checking => self.update = Status::Failed(error),
            Err(_) => {}
        }
    }

    /// Look now, because the user asked.
    pub(crate) fn check_for_update(&mut self) {
        if matches!(
            self.update,
            Status::Checking | Status::Installing(_) | Status::Installed(_)
        ) {
            return;
        }
        self.update = Status::Checking;
        #[cfg(not(test))]
        if let Some(handle) = crate::APP_HANDLE.get().cloned() {
            check_once(handle);
        }
    }

    /// Install the release that was found over this copy, because the user
    /// asked. Does nothing without one, or where this copy cannot be replaced.
    pub(crate) fn install_update(&mut self) {
        let Status::Available(release) = &self.update else {
            return;
        };
        let Some(dir) = self.update_dir.clone() else {
            return;
        };
        self.update = Status::Installing(release.clone());
        #[cfg(not(test))]
        if let Some(handle) = crate::APP_HANDLE.get().cloned() {
            install_once(handle, dir);
        }
        #[cfg(test)]
        drop(dir);
    }

    /// What the installer did: the new version waits for a restart, and a
    /// failure leaves the release available to try again.
    pub(crate) fn update_installed(&mut self, result: Result<(), String>) {
        let Status::Installing(release) = &self.update else {
            return;
        };
        let release = release.clone();
        match result {
            Ok(()) => {
                self.notice = Some(format!(
                    "Goko {} installed — restart Goko to use it",
                    release.version
                ));
                self.update = Status::Installed(release);
            }
            Err(error) => {
                self.notice = Some(format!("Could not update: {error}"));
                self.update = Status::Available(release);
            }
        }
    }
}

/// One install on its own thread, reported to the app when it ends.
#[cfg(not(test))]
fn install_once(handle: rune::AppHandle<State>, dir: PathBuf) {
    let _ = std::thread::Builder::new()
        .name("goko-update-install".into())
        .spawn(move || {
            let result = install(&base_url(), &dir);
            handle.update(move |s: &mut State, _tx| s.update_installed(result));
        });
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
    fn the_manifest_names_its_release_for_a_target_it_builds() {
        let json = r#"{"version":"0.3.1","targets":{"macos-aarch64":{"url":"u","sha256":"s"}}}"#;
        assert_eq!(release_in(json, "macos-aarch64"), Some("0.3.1".into()));
        assert_eq!(release_in(json, "linux-x86_64"), None, "no build for it");
        assert_eq!(release_in("{}", "macos-aarch64"), None);
        assert_eq!(release_in("not json", "macos-aarch64"), None);
    }

    #[test]
    fn the_manifest_carries_the_notes_when_it_has_them() {
        let json = r#"{"version":"0.3.1","targets":{"t":{}},"notes":"\n### Added\n\n- a\n"}"#;
        assert_eq!(release_in(json, "t").unwrap().notes, "### Added\n\n- a");
    }

    #[test]
    fn notes_become_headings_and_one_paragraph_per_bullet() {
        let notes = "### Added\n\n- Find in the `terminal`: opens a bar\n  that searches the pane.\n- Second.\n\n### Fixed\n\n- Third\n  - nested";
        let rows = note_rows(notes);
        assert_eq!(
            rows,
            [
                (true, "Added".to_string()),
                (
                    false,
                    "Find in the terminal: opens a bar that searches the pane.".into()
                ),
                (false, "Second.".into()),
                (true, "Fixed".into()),
                (false, "Third - nested".into()),
            ]
        );
        assert!(note_rows("").is_empty());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn the_install_folder_is_the_one_holding_goko_app() {
        let dir = std::env::temp_dir().join(format!("goko-dir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = |app: &str| dir.join(app).join("Contents/MacOS/goko-rune");
        assert_eq!(
            dir_of(&exe("Goko.app"), Path::new("/data")),
            Some(dir.as_path())
        );
        assert_eq!(dir_of(&exe("Renamed.app"), Path::new("/data")), None);
        assert_eq!(
            dir_of(
                Path::new("/repo/target/debug/goko-rune"),
                Path::new("/data")
            ),
            None,
            "a build from source is not an installed copy"
        );
        assert_eq!(
            install_dir(),
            None,
            "the test binary is not an installed app"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn the_install_folder_is_the_data_folder_holding_goko() {
        let dir = std::env::temp_dir().join(format!("goko-dir-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("goko")).unwrap();
        std::fs::create_dir_all(dir.join("other")).unwrap();
        assert_eq!(
            dir_of(&dir.join("goko/goko-rune"), &dir).map(|d| d.canonicalize().unwrap()),
            Some(dir.canonicalize().unwrap())
        );
        assert_eq!(dir_of(&dir.join("other/goko-rune"), &dir), None);
        assert_eq!(
            install_dir(),
            None,
            "the test binary is not an installed copy"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_that_cannot_be_written_is_not_writable() {
        let dir = std::env::temp_dir();
        assert!(writable(&dir));
        assert!(!writable(&dir.join("goko-no-such-folder")));
    }

    #[test]
    fn install_runs_the_published_installer_over_the_given_folder() {
        let dir = std::env::temp_dir().join(format!("goko-install-{}", std::process::id()));
        let apps = dir.join("apps");
        std::fs::create_dir_all(&apps).unwrap();
        let base = format!("file://{}", dir.display());
        let installer = |script: &str| std::fs::write(dir.join("install.sh"), script).unwrap();

        installer(
            "set -eu\nprintf '%s' \"$GOKO_APP_DIR $GOKO_RELEASES_URL\" > \"$GOKO_APP_DIR/ran\"\n",
        );
        assert_eq!(install(&base, &apps), Ok(()));
        assert_eq!(
            std::fs::read_to_string(apps.join("ran")).unwrap(),
            format!("{} {base}", apps.display()),
            "it installs where it was told, reading the same releases"
        );

        installer("echo 'goko: starting' >&2\necho 'goko: download failed' >&2\nexit 1\n");
        assert_eq!(install(&base, &apps), Err("goko: download failed".into()));
        std::fs::remove_file(dir.join("install.sh")).unwrap();
        assert!(install(&base, &apps).is_err(), "no installer, no update");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn update_now_installs_a_found_release_where_it_can_and_keeps_it_on_failure() {
        let mut s = State::new(crate::test_handle());
        s.install_update();
        assert_eq!(
            s.update,
            Status::Unknown,
            "nothing found, nothing to install"
        );

        s.update_checked(Ok(Some("9.9.9".into())));
        s.install_update();
        assert_eq!(
            s.update,
            Status::Available("9.9.9".into()),
            "a copy that cannot be replaced is updated by hand"
        );

        s.update_dir = Some(std::env::temp_dir());
        s.install_update();
        assert_eq!(s.update, Status::Installing("9.9.9".into()));
        s.update_checked(Ok(None));
        s.check_for_update();
        assert_eq!(
            s.update,
            Status::Installing("9.9.9".into()),
            "a check does not disturb it"
        );

        s.update_installed(Err("goko: download failed".into()));
        assert_eq!(s.update, Status::Available("9.9.9".into()), "try again");
        assert!(
            s.notice
                .as_deref()
                .is_some_and(|n| n.contains("download failed"))
        );

        s.install_update();
        s.update_installed(Ok(()));
        assert_eq!(s.update, Status::Installed("9.9.9".into()));
        s.update_checked(Ok(Some("9.9.9".into())));
        s.check_for_update();
        assert_eq!(
            s.update,
            Status::Installed("9.9.9".into()),
            "the old copy would find the same release again"
        );
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
