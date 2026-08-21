use std::error::Error;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use crate::domain::identity::ApplicationId;

#[derive(Debug)]
pub enum ApplicationLockError {
    DatabasePathUnavailable,
    Open {
        path: PathBuf,
        source: std::io::Error,
    },
    Acquire {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for ApplicationLockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DatabasePathUnavailable => write!(
                formatter,
                "database path is unavailable for application locking"
            ),
            Self::Open { path, source } => write!(
                formatter,
                "failed to open application lock {}: {source}",
                path.display()
            ),
            Self::Acquire { path, source } => write!(
                formatter,
                "failed to acquire application lock {}: {source}",
                path.display()
            ),
        }
    }
}

impl Error for ApplicationLockError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Open { source, .. } | Self::Acquire { source, .. } => Some(source),
            Self::DatabasePathUnavailable => None,
        }
    }
}

// Holds the kernel advisory lock until the operation and all of its external effects complete.
pub struct ApplicationLock {
    file: File,
}

impl ApplicationLock {
    pub fn try_acquire(
        database_path: &Path,
        application_id: &ApplicationId,
    ) -> Result<Option<Self>, ApplicationLockError> {
        let path = lock_path(database_path, application_id);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|source| ApplicationLockError::Open {
                path: path.clone(),
                source,
            })?;
        // flock is process-scoped and releases automatically if the process exits.
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result == 0 {
            return Ok(Some(Self { file }));
        }
        let source = std::io::Error::last_os_error();
        if source.kind() == std::io::ErrorKind::WouldBlock {
            return Ok(None);
        }
        Err(ApplicationLockError::Acquire { path, source })
    }
}

impl Drop for ApplicationLock {
    fn drop(&mut self) {
        // Keep the sidecar inode stable: unlinking it could let a third process lock a new file.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

// Names the sidecar from both configured database identity and logical application identity.
pub fn lock_path(database_path: &Path, application_id: &ApplicationId) -> PathBuf {
    let application_id = application_id.as_str();
    if database_path.as_os_str().is_empty() || database_path == Path::new(":memory:") {
        return std::env::temp_dir().join(format!("pneuma-memory-{application_id}.lock"));
    }
    PathBuf::from(format!(
        "{}.{}.lock",
        database_path.display(),
        application_id
    ))
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{ApplicationLock, lock_path};
    use crate::domain::identity::ApplicationId;

    fn application_id(value: &str) -> ApplicationId {
        ApplicationId::from(value)
    }

    #[test]
    fn serializes_same_application_and_keeps_different_applications_independent() {
        let root = env::temp_dir().join(format!(
            "pneuma-application-lock-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let database = root.join("pneuma.sqlite3");

        let held = ApplicationLock::try_acquire(&database, &application_id("application-a"))
            .unwrap()
            .unwrap();
        assert!(
            ApplicationLock::try_acquire(&database, &application_id("application-a"))
                .unwrap()
                .is_none()
        );
        assert!(
            ApplicationLock::try_acquire(&database, &application_id("application-b"))
                .unwrap()
                .is_some()
        );
        assert_eq!(
            lock_path(&database, &application_id("application-a")),
            root.join("pneuma.sqlite3.application-a.lock")
        );

        drop(held);
        assert!(
            ApplicationLock::try_acquire(&database, &application_id("application-a"))
                .unwrap()
                .is_some()
        );
        fs::remove_file(lock_path(&database, &application_id("application-a"))).unwrap();
        fs::remove_file(lock_path(&database, &application_id("application-b"))).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
