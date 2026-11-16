use crate::paths::Paths;
use std::fmt;
use std::fs::{File, OpenOptions, TryLockError};
use std::io;

#[derive(Debug)]
pub enum LockError {
    AlreadyRunning,
    Io(io::Error),
}

impl fmt::Display for LockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning => formatter.write_str("another gokod is already running"),
            Self::Io(error) => write!(formatter, "could not lock the daemon: {error}"),
        }
    }
}

impl std::error::Error for LockError {}

/// Exclusive `flock` on `daemon.lock`, released when dropped.
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    pub fn acquire(paths: &Paths) -> Result<Self, LockError> {
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(paths.lock()).map_err(LockError::Io)?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(LockError::AlreadyRunning),
            Err(TryLockError::Error(error)) => Err(LockError::Io(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{InstanceLock, LockError};
    use crate::paths::Paths;
    use crate::test_support::TempDir;

    #[test]
    fn second_acquire_fails_until_the_first_lock_is_dropped() {
        let temp = TempDir::new("lock");
        let paths = Paths::at(temp.path()).unwrap();

        let first = InstanceLock::acquire(&paths).expect("first lock");
        assert!(matches!(
            InstanceLock::acquire(&paths),
            Err(LockError::AlreadyRunning)
        ));

        drop(first);
        InstanceLock::acquire(&paths).expect("lock after release");
    }
}
