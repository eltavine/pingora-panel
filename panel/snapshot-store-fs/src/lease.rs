use crate::{
    atomic_file::{AtomicFilePublisher, AtomicPublishError, AtomicPublishStage, TemporaryPrefix},
    state_directory::StateDirectoryHandle,
};
use panel_errors::{PanelError, Result};
#[cfg(unix)]
use std::{ffi::OsStr, fs::File};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[cfg(unix)]
const LEASE_FILE_NAME: &str = ".gateway.lock";

/// Namespace for the lease's own in-flight diagnostic writes.
#[cfg(unix)]
const LEASE_TEMPORARY_PREFIX: TemporaryPrefix = TemporaryPrefix::new(".gateway-lock-");

/// Directory entries inspected when reclaiming abandoned lease temporaries.
///
/// Bounded for the same reason every other directory scan in this crate is: a
/// state directory that has been filled with entries must not be able to make
/// lease acquisition consume unbounded memory. Each crash abandons at most one
/// temporary, so this ceiling is far above any honest backlog, and a directory
/// holding more is drained across successive acquisitions.
#[cfg(unix)]
const LEASE_RECLAIM_ENTRY_CEILING: usize = 4096;

/// Process-lifetime exclusive ownership of a snapshot state directory.
///
/// The operating system locks the open directory inode, so crashes cannot leave
/// a stale lease and replacing the diagnostic lock file cannot bypass ownership.
/// Cloned stores share this guard through an `Arc`.
#[derive(Debug)]
pub struct StateDirectoryLease {
    #[cfg(unix)]
    file: File,
    directory: Arc<StateDirectoryHandle>,
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

    pub(crate) fn directory(&self) -> Arc<StateDirectoryHandle> {
        Arc::clone(&self.directory)
    }

    #[cfg(unix)]
    fn acquire_blocking(root: &Path) -> Result<Self> {
        use std::os::fd::AsRawFd;

        let directory = StateDirectoryHandle::open_root(root, true)
            .map_err(|error| {
                super::state_directory_open_error("open state directory", root, error)
            })?
            .expect("creating the state directory returns a handle");
        let directory = Arc::new(directory);
        let path = root.join(LEASE_FILE_NAME);
        let file = directory.clone_directory_file().map_err(|error| {
            super::storage_error("duplicate state directory lease handle", root, error)
        })?;

        // The directory inode is the lock authority. A diagnostic file can be
        // deleted or replaced without allowing another owner to lock the same
        // state directory while this descriptor remains alive.
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
                root,
                error,
            ));
        }

        // The exclusive lock is held from here on, so any diagnostic
        // temporary still present was abandoned by a dead owner.
        AtomicFilePublisher::new(&directory, LEASE_TEMPORARY_PREFIX)
            .reclaim_abandoned(LEASE_RECLAIM_ENTRY_CEILING)
            .map_err(|error| lease_publish_error(&path, error))?;
        replace_diagnostic_file(&directory, &path)?;

        Ok(Self {
            file,
            directory,
            path,
        })
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
fn replace_diagnostic_file(directory: &StateDirectoryHandle, path: &Path) -> Result<()> {
    let contents = format!("pid={}\n", std::process::id());
    AtomicFilePublisher::new(directory, LEASE_TEMPORARY_PREFIX)
        .publish_bytes(OsStr::new(LEASE_FILE_NAME), contents.as_bytes())
        .map_err(|error| lease_publish_error(path, error))
}

#[cfg(unix)]
fn lease_publish_error(destination: &Path, error: AtomicPublishError) -> PanelError {
    let (stage, path, source) = error.into_parts();
    let (operation, path) = match stage {
        AtomicPublishStage::CreateTemporary => ("create temporary state directory lease", path),
        AtomicPublishStage::WriteTemporary => ("write temporary state directory lease", path),
        AtomicPublishStage::SyncTemporary => ("sync temporary state directory lease", path),
        AtomicPublishStage::Activate => {
            ("activate state directory lease", destination.to_path_buf())
        }
        AtomicPublishStage::SyncDirectory => ("sync state directory after lease activation", path),
        AtomicPublishStage::Reclaim => ("reclaim abandoned state directory leases", path),
    };
    super::storage_error(operation, &path, source)
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use panel_errors::ErrorCode;
    use std::fs;
    use uuid::Uuid;

    struct TemporaryDirectory(PathBuf);

    impl TemporaryDirectory {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("pingora-panel-lease-test-{}", Uuid::new_v4())))
        }
    }

    impl Drop for TemporaryDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn hard_linked_diagnostic_is_replaced_without_truncating_its_target() {
        let temporary = TemporaryDirectory::new();
        let state = temporary.0.join("state");
        let external = temporary.0.join("external.txt");
        let contents = b"must remain intact";
        fs::create_dir_all(&state).unwrap();
        fs::write(&external, contents).unwrap();
        fs::hard_link(&external, state.join(LEASE_FILE_NAME)).unwrap();

        let _lease = StateDirectoryLease::acquire(&state).await.unwrap();

        assert_eq!(fs::read(external).unwrap(), contents);
        assert_ne!(fs::read(state.join(LEASE_FILE_NAME)).unwrap(), contents);
    }

    #[tokio::test]
    async fn symlinked_diagnostic_is_replaced_without_writing_its_target() {
        use std::os::unix::fs::symlink;

        let temporary = TemporaryDirectory::new();
        let state = temporary.0.join("state");
        let external = temporary.0.join("external.txt");
        let contents = b"must remain intact";
        fs::create_dir_all(&state).unwrap();
        fs::write(&external, contents).unwrap();
        symlink(&external, state.join(LEASE_FILE_NAME)).unwrap();

        let _lease = StateDirectoryLease::acquire(&state).await.unwrap();

        assert_eq!(fs::read(external).unwrap(), contents);
        assert!(!fs::symlink_metadata(state.join(LEASE_FILE_NAME))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[tokio::test]
    async fn deleting_the_diagnostic_file_cannot_bypass_directory_ownership() {
        let temporary = TemporaryDirectory::new();
        let state = temporary.0.join("state");
        let first = StateDirectoryLease::acquire(&state).await.unwrap();
        fs::remove_file(state.join(LEASE_FILE_NAME)).unwrap();

        let error = StateDirectoryLease::acquire(&state).await.unwrap_err();

        assert_eq!(error.code.as_str(), ErrorCode::PRECONDITION_FAILED);
        drop(first);
        StateDirectoryLease::acquire(&state).await.unwrap();
    }
}
