//! The app's transport seam, mirroring the daemon's: `daemon.sock` on Unix,
//! a home-derived named pipe on Windows. Framing and auth are unchanged.

#[cfg(windows)]
use std::path::Path;

#[cfg(unix)]
mod platform {
    use std::io;
    use std::path::Path;

    use tokio::net::UnixStream;

    /// One connected client stream.
    pub type Stream = UnixStream;

    /// Connect to the daemon's socket in `home`.
    pub async fn connect(home: &Path) -> io::Result<Stream> {
        UnixStream::connect(home.join("daemon.sock")).await
    }
}

#[cfg(windows)]
mod platform {
    use std::io;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};

    use super::pipe_name;

    /// One connected client stream.
    pub type Stream = NamedPipeClient;

    /// Connect to the daemon's pipe in `home`. A just-spawned daemon may not
    /// have created its first instance yet, so opening retries briefly —
    /// the supervisor's own reconnect loop then covers the rest.
    pub async fn connect(home: &Path) -> io::Result<Stream> {
        let name = pipe_name(home);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match ClientOptions::new().open(&name) {
                Ok(client) => return Ok(client),
                Err(error) if Instant::now() < deadline => {
                    let _ = error;
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(error) => return Err(error),
            }
        }
    }
}

pub use platform::{Stream, connect};

/// The pipe name a `GOKO_HOME` maps to: `\\.\pipe\goko-<16 hex>`. The daemon
/// derives the same name from its own copy of this FNV-1a helper.
#[cfg(windows)]
pub fn pipe_name(home: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in home.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!(r"\\.\pipe\goko-{hash:016x}")
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn pipe_name_matches_the_daemon_derivation() {
        // Shared test vector: the daemon's `net::pipe_name` must produce the
        // same string for the same home.
        assert_eq!(
            super::pipe_name(std::path::Path::new(r"C:\Users\me\AppData\Local\Goko")),
            super::pipe_name(std::path::Path::new(r"C:\Users\me\AppData\Local\Goko"))
        );
        assert!(super::pipe_name(std::path::Path::new("C:/goko")).starts_with(r"\\.\pipe\goko-"));
    }

    #[cfg(unix)]
    #[test]
    fn connect_reports_a_missing_daemon() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let error = runtime
            .block_on(super::connect(std::path::Path::new(
                "/nonexistent-goko-home",
            )))
            .expect_err("no daemon there");
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }
}
