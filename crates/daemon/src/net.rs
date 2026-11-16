//! The daemon's transport seam: a Unix socket on Unix, a named pipe on
//! Windows. Framing, auth, and the accept loop are shared; only the stream
//! implementation differs.
//!
//! The pipe name is derived from `GOKO_HOME` so one home maps to one daemon,
//! like `daemon.sock` does. The client (`apps/rune`) re-derives it with the
//! same FNV-1a helper and a shared test vector.

#[cfg(unix)]
pub use unix::*;
#[cfg(windows)]
pub use windows::*;

/// The pipe name a `GOKO_HOME` maps to: `\\.\pipe\goko-<16 hex>`.
///
/// FNV-1a over the home path's lossy bytes — a stable, dependency-free hash
/// both binaries can compute; collisions only merge two homes into one pipe,
/// which the token auth still rejects.
#[cfg_attr(unix, allow(dead_code))]
pub fn pipe_name(home: &std::path::Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in home.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!(r"\\.\pipe\goko-{hash:016x}")
}

#[cfg(unix)]
mod unix {
    use std::fs::{self, Permissions};
    use std::io;
    use std::os::unix::fs::PermissionsExt;

    use tokio::net::{UnixListener, UnixStream};

    use crate::paths::{remove_if_exists, Paths};

    /// One accepted connection.
    pub type Stream = UnixStream;

    /// A bound `daemon.sock`, mode 0600.
    pub struct Listener(UnixListener);

    impl Listener {
        /// Replace any stale `daemon.sock` and bind a fresh one. The caller
        /// holds the instance lock, which proves no live daemon owns the old
        /// socket.
        pub fn bind(paths: &Paths) -> io::Result<Self> {
            remove_if_exists(&paths.socket())?;
            let listener = UnixListener::bind(paths.socket())?;
            fs::set_permissions(paths.socket(), Permissions::from_mode(0o600))?;
            Ok(Self(listener))
        }

        pub async fn accept(&mut self) -> io::Result<Stream> {
            let (stream, _) = self.0.accept().await?;
            Ok(stream)
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::ffi::c_void;
    use std::io;
    use std::ptr;

    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
    use windows_sys::Win32::Foundation::{LocalFree, HLOCAL};
    use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};

    use crate::net::pipe_name;
    use crate::paths::Paths;

    /// One accepted connection (a connected pipe instance).
    pub type Stream = NamedPipeServer;

    /// Owner-only DACL: protected, generic-all to Owner Rights and LocalSystem.
    const SDDL: &str = "D:P(A;;GA;;;OW)(A;;GA;;;SY)";

    /// A pipe listener: one instance awaiting `ConnectNamedPipe` at a time,
    /// with the next instance created before the current one connects so a
    /// client never finds the pipe absent.
    pub struct Listener {
        name: String,
        current: Option<NamedPipeServer>,
    }

    impl Listener {
        /// Create the first instance. `ERROR_PIPE_BUSY` here means a live
        /// daemon already owns the name — the caller reports `already running`.
        pub fn bind(paths: &Paths) -> io::Result<Self> {
            let name = pipe_name(paths.dir());
            let current = create_instance(&name, true)?;
            Ok(Self {
                name,
                current: Some(current),
            })
        }

        pub async fn accept(&mut self) -> io::Result<Stream> {
            let current = self
                .current
                .take()
                .ok_or_else(|| io::Error::other("the pipe has no instance to accept"))?;
            self.current = Some(create_instance(&self.name, false)?);
            current.connect().await?;
            Ok(current)
        }
    }

    fn create_instance(name: &str, first: bool) -> io::Result<NamedPipeServer> {
        let mut options = ServerOptions::new();
        options
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .max_instances(254);
        let mut attributes = security_attributes()?;
        // SAFETY: `attributes` outlives the call and carries a valid
        // self-relative descriptor; the pipe copies it during creation.
        let created = unsafe {
            options.create_with_security_attributes_raw(
                name,
                &mut attributes as *mut SECURITY_ATTRIBUTES as *mut c_void,
            )
        };
        free_descriptor(attributes.lpSecurityDescriptor);
        created
    }

    /// A `SECURITY_ATTRIBUTES` whose descriptor is the owner-only SDDL above.
    fn security_attributes() -> io::Result<SECURITY_ATTRIBUTES> {
        let sddl: Vec<u16> = SDDL.encode_utf16().chain(std::iter::once(0)).collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: the SDDL is NUL-terminated and the out-pointer is valid.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1, // SDDL_REVISION_1
                &mut descriptor,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        })
    }

    /// Free a descriptor returned by `security_attributes` once the pipe
    /// instance exists.
    fn free_descriptor(descriptor: PSECURITY_DESCRIPTOR) {
        // SAFETY: `descriptor` came from the SDDL conversion above.
        unsafe {
            LocalFree(descriptor as HLOCAL);
        }
    }
}
