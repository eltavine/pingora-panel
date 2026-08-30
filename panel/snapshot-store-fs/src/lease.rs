use panel_errors::{PanelError, Result};
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::{fs::File, fs::OpenOptions, io::Write};

const LEASE_FILE_NAME: &str = ".gateway.lock";

/// Process-lifetime exclusive ownership of a snapshot state directory.
///
/// The operating system owns the lock lifetime, so crashes cannot leave a stale
/// lease behind. Cloned stores share this guard through an `Arc`.
#[derive(Debug)]
pub struct StateDirectoryLease {
    #[cfg(unix)]
    file: File,
    path: PathBuf,
}

impl StateDirectoryLease {
    pub async fn acquire(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        tokio::task::spawn_blocking(move || Self::acquire_blocking(&root))
            .await
            .map_err(|error| PanelError::internal("state lease worker failed").with_source(error))?
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(unix)]
    fn acquire_blocking(root: &Path) -> Result<Self> {
        use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};

        super::ensure_directory(root)?;
        let path = root.join(LEASE_FILE_NAME);
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
        let mut file = options
            .open(&path)
            .map_err(|error| super::storage_error("open state directory lease", &path, error))?;
        let metadata = file.metadata().map_err(|error| {
            super::storage_error("read state directory lease metadata", &path, error)
        })?;
        if !metadata.is_file() {
            return Err(PanelError::corrupt_state(format!(
                "state directory lease is not a regular file: {}",
                path.display()
            )));
        }

        // SAFETY: `file` owns a live descriptor for the duration of this call and
        // `flock` neither retains nor dereferences userspace pointers.
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                return Err(PanelError::precondition_failed(format!(
                    "state directory is already owned by another gateway: {}",
                    root.display()
                )));
            }
            return Err(super::storage_error(
                "acquire state directory lease",
                &path,
                error,
            ));
        }

        file.set_len(0)
            .and_then(|()| writeln!(file, "pid={}", std::process::id()))
            .and_then(|()| file.sync_all())
            .map_err(|error| super::storage_error("write state directory lease", &path, error))?;

        Ok(Self { file, path })
    }

    #[cfg(not(unix))]
    fn acquire_blocking(root: &Path) -> Result<Self> {
        let _ = root;
        Err(PanelError::unsupported_capability(
            "exclusive state directory leases are not supported on this platform",
        ))
    }
}

#[cfg(unix)]
impl Drop for StateDirectoryLease {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;

        // SAFETY: the descriptor is still owned by `self.file`; unlock is best
        // effort because closing the descriptor immediately releases it anyway.
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}
