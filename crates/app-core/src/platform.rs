//! Platform seams shared by the app-core modules: executable checks, private
//! file modes, shared-folder links, and file identity. Unix behavior is
//! unchanged; the Windows branches keep the same contracts with the
//! equivalents that exist there (PATHEXT, `%LOCALAPPDATA%` ACLs, copies).

use std::fs::{self, DirBuilder, OpenOptions};
use std::io;
use std::path::Path;

/// Whether `path` is an executable program.
#[cfg(unix)]
pub fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.is_file()
        && path
            .metadata()
            .is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
}

/// Windows has no execute bit: a file whose extension is on `PATHEXT` (or any
/// existing file when `PATHEXT` is unset) counts as a program.
#[cfg(windows)]
pub fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let extensions = std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned())
        .to_ascii_lowercase();
    match path.extension().and_then(|extension| extension.to_str()) {
        Some(extension) => extensions
            .split(';')
            .any(|known| known.trim_start_matches('.') == extension.to_ascii_lowercase()),
        None => false,
    }
}

/// Create `dir` (and parents) private to the user: mode 0700 on Unix; on
/// Windows the `%LOCALAPPDATA%` ACL already scopes the tree to the user.
pub fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// Open `path` with `OpenOptions`, adding mode 0600 on Unix.
pub fn open_private(options: &mut OpenOptions, path: &Path) -> io::Result<fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Link `source` at `target`, as a directory link on both platforms. When the
/// link cannot be created (Windows without the symlink privilege), copy the
/// tree instead; returns whether a link was made.
pub fn link_dir(source: &Path, target: &Path) -> io::Result<bool> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, target)?;
        Ok(true)
    }
    #[cfg(windows)]
    {
        match std::os::windows::fs::symlink_dir(source, target) {
            Ok(()) => Ok(true),
            Err(error)
                if error.kind() == io::ErrorKind::PermissionDenied
                    || error.raw_os_error() == Some(1314) =>
            {
                copy_tree(source, target)?;
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }
}

#[cfg(windows)]
fn copy_tree(source: &Path, target: &Path) -> io::Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Prepare a child process so its whole tree can be killed: Unix gives it
/// its own process group; Windows hides its console window (the Job Object
/// covers the tree).
pub fn configure_child(command: &mut tokio::process::Command) {
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: a GUI-launched agent or server gets no console.
        command.creation_flags(0x0800_0000);
    }
}

/// The handle that owns a child's process tree. Unix: the child's own process
/// group (no handle needed); Windows: a kill-on-close Job Object.
#[cfg(unix)]
pub struct ChildJob;

#[cfg(unix)]
impl ChildJob {
    /// Unix children lead their own group, so there is nothing to create.
    pub fn assign(_pid: u32) -> io::Result<Self> {
        Ok(Self)
    }

    pub fn terminate(&self) {}
}

#[cfg(windows)]
pub struct ChildJob(windows_sys::Win32::Foundation::HANDLE);

// SAFETY: job handles are process-wide and the job APIs are thread-safe.
unsafe impl Send for ChildJob {}
// SAFETY: same as above.
unsafe impl Sync for ChildJob {}

#[cfg(windows)]
impl ChildJob {
    /// Create a `KILL_ON_JOB_CLOSE` job and assign `pid` to it.
    pub fn assign(pid: u32) -> io::Result<Self> {
        use std::ffi::c_void;
        use std::ptr;
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
        };

        // SAFETY: null attributes and name are allowed.
        let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self(handle);
        // SAFETY: the extended-limit struct is plain data; zero is valid.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: the struct matches the information class and its size.
        let configured = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the rights are the ones assignment needs; the handle is
        // closed right after.
        let process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
        if process.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: both handles are valid for this call.
        let assigned = unsafe { AssignProcessToJobObject(job.0, process) };
        unsafe { CloseHandle(process) };
        if assigned == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    /// Terminate every process in the job.
    pub fn terminate(&self) {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        // SAFETY: `self.0` is a live job handle.
        unsafe { TerminateJobObject(self.0, 1) };
    }
}

#[cfg(windows)]
impl Drop for ChildJob {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        // SAFETY: closing the last handle kills whatever is left in the job.
        unsafe { CloseHandle(self.0) };
    }
}

/// A file's identity, comparable across two reads of the same path.
#[cfg(unix)]
pub type FileId = (u64, u64);

/// A file's identity, comparable across two reads of the same path.
#[cfg(windows)]
pub type FileId = (u32, u64);

/// Read the identity of `meta`'s file. Unix: device + inode; Windows: volume
/// serial + file index.
#[cfg(unix)]
pub fn file_id(meta: &fs::Metadata) -> FileId {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino())
}

/// Read the identity of `meta`'s file. Unix: device + inode; Windows: volume
/// serial + file index.
#[cfg(windows)]
pub fn file_id(meta: &fs::Metadata) -> FileId {
    use std::os::windows::fs::MetadataExt;
    (
        meta.volume_serial_number().unwrap_or(0),
        meta.file_index().unwrap_or(0),
    )
}

/// Whether two paths name the same file.
pub fn same_file(left: &Path, right: &Path) -> bool {
    match (fs::metadata(left), fs::metadata(right)) {
        (Ok(a), Ok(b)) => file_id(&a) == file_id(&b),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn create_private_dir_is_idempotent() {
        let temp = TempDir::new("platform-dir");
        let dir = temp.path().join("nested/goko");
        create_private_dir(&dir).expect("create");
        create_private_dir(&dir).expect("create again");
        assert!(dir.is_dir());
    }

    #[test]
    fn same_file_matches_a_path_and_its_copy_target() {
        let temp = TempDir::new("platform-same");
        let file = temp.path().join("a.txt");
        fs::write(&file, b"x").unwrap();
        assert!(same_file(&file, &file));
        assert!(!same_file(&file, &temp.path().join("absent.txt")));
    }
}
