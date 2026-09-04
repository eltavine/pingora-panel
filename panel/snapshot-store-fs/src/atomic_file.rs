use crate::state_directory::StateDirectoryHandle;
use std::{
    error::Error,
    ffi::OsStr,
    fmt,
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

/// Exact durability stage at which an atomic publication failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AtomicPublishStage {
    CreateTemporary,
    WriteTemporary,
    SyncTemporary,
    Activate,
    SyncDirectory,
    Reclaim,
}

/// Context-rich filesystem failure returned by [`AtomicFilePublisher`].
#[derive(Debug)]
pub(crate) struct AtomicPublishError {
    stage: AtomicPublishStage,
    path: PathBuf,
    source: io::Error,
}

impl AtomicPublishError {
    pub(crate) fn into_parts(self) -> (AtomicPublishStage, PathBuf, io::Error) {
        (self.stage, self.path, self.source)
    }
}

impl fmt::Display for AtomicPublishError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "atomic publication failed during {:?} for {}: {}",
            self.stage,
            self.path.display(),
            self.source
        )
    }
}

impl Error for AtomicPublishError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

/// The namespace one publisher uses for its in-flight temporary files.
///
/// Publication and reclamation both derive their names from this value, so a
/// sweep can never target a different prefix than the writes it reclaims.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TemporaryPrefix(&'static str);

impl TemporaryPrefix {
    pub(crate) const fn new(prefix: &'static str) -> Self {
        Self(prefix)
    }

    fn name_for(self, identifier: Uuid) -> String {
        format!("{}{}{}", self.0, identifier, TEMPORARY_SUFFIX)
    }

    /// Reports whether one directory entry belongs to this prefix.
    fn claims(self, name: &OsStr) -> bool {
        let Some(name) = name.to_str() else {
            return false;
        };
        let Some(identifier) = name
            .strip_prefix(self.0)
            .and_then(|name| name.strip_suffix(TEMPORARY_SUFFIX))
        else {
            return false;
        };
        Uuid::parse_str(identifier).is_ok_and(|uuid| uuid.hyphenated().to_string() == identifier)
    }
}

const TEMPORARY_SUFFIX: &str = ".tmp";

/// Publishes one file relative to an already-open directory capability.
///
/// Every caller gets the same create-new, write, file-fsync, rename,
/// directory-fsync, and failure-cleanup sequence. The destination is never
/// opened for writing, so replacing a symlink or hard link cannot mutate its
/// target.
pub(crate) struct AtomicFilePublisher<'a> {
    directory: &'a StateDirectoryHandle,
    temporary_prefix: TemporaryPrefix,
}

impl<'a> AtomicFilePublisher<'a> {
    pub(crate) const fn new(
        directory: &'a StateDirectoryHandle,
        temporary_prefix: TemporaryPrefix,
    ) -> Self {
        Self {
            directory,
            temporary_prefix,
        }
    }

    /// Removes temporary files abandoned by a process that died mid-publish.
    ///
    /// A crash between creating a temporary file and renaming it leaves an
    /// entry that readers skip but that still consumes the directory-entry
    /// budget, so repeated crashes would eventually make the directory
    /// unscannable. Callers must hold exclusive ownership of the directory:
    /// with a concurrent writer, an entry claimed by this prefix may still be
    /// in flight.
    ///
    /// Individual removal failures are tolerated, because reclaiming is
    /// opportunistic and must never keep a healthy store from opening.
    pub(crate) fn reclaim_abandoned(
        &self,
        max_entries: usize,
    ) -> Result<usize, AtomicPublishError> {
        let names = self
            .directory
            .read_entry_names(max_entries)
            .map_err(|source| {
                publish_error(AtomicPublishStage::Reclaim, self.directory.path(), source)
            })?;
        let reclaimed = names
            .iter()
            .filter(|name| self.temporary_prefix.claims(name))
            .filter(|name| self.directory.remove_file(name).is_ok())
            .count();
        if reclaimed > 0 {
            self.directory.sync().map_err(|source| {
                publish_error(
                    AtomicPublishStage::SyncDirectory,
                    self.directory.path(),
                    source,
                )
            })?;
        }
        Ok(reclaimed)
    }

    pub(crate) fn publish_bytes(
        &self,
        destination: &OsStr,
        contents: &[u8],
    ) -> Result<(), AtomicPublishError> {
        self.publish_with(destination, |file| file.write_all(contents))
    }

    fn publish_with(
        &self,
        destination: &OsStr,
        write: impl FnOnce(&mut File) -> io::Result<()>,
    ) -> Result<(), AtomicPublishError> {
        let temporary_name = self.temporary_prefix.name_for(Uuid::new_v4());
        let temporary_name = OsStr::new(&temporary_name);
        let temporary_path = self.directory.path_for(temporary_name);
        let destination_path = self.directory.path_for(destination);
        let result = (|| {
            let mut file = self
                .directory
                .create_new_file(temporary_name)
                .map_err(|source| {
                    publish_error(AtomicPublishStage::CreateTemporary, &temporary_path, source)
                })?;
            write(&mut file).map_err(|source| {
                publish_error(AtomicPublishStage::WriteTemporary, &temporary_path, source)
            })?;
            file.sync_all().map_err(|source| {
                publish_error(AtomicPublishStage::SyncTemporary, &temporary_path, source)
            })?;
            self.directory
                .rename_file(temporary_name, destination)
                .map_err(|source| {
                    publish_error(AtomicPublishStage::Activate, &destination_path, source)
                })?;
            self.directory.sync().map_err(|source| {
                publish_error(
                    AtomicPublishStage::SyncDirectory,
                    self.directory.path(),
                    source,
                )
            })
        })();

        if result.is_err() {
            let _ = self.directory.remove_file(temporary_name);
        }
        result
    }
}

fn publish_error(stage: AtomicPublishStage, path: &Path, source: io::Error) -> AtomicPublishError {
    AtomicPublishError {
        stage,
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state_directory::StateDirectoryHandle;
    use std::{fs, path::PathBuf};

    struct TemporaryDirectory(PathBuf);

    impl TemporaryDirectory {
        fn new() -> Self {
            Self(
                std::env::temp_dir()
                    .join(format!("pingora-panel-atomic-file-test-{}", Uuid::new_v4())),
            )
        }

        fn open(&self) -> StateDirectoryHandle {
            StateDirectoryHandle::open_root(&self.0, true)
                .unwrap()
                .unwrap()
        }
    }

    impl Drop for TemporaryDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn write_failure_removes_the_unpublished_temporary_file() {
        let temporary = TemporaryDirectory::new();
        let directory = temporary.open();
        let publisher = AtomicFilePublisher::new(&directory, TemporaryPrefix::new(".failure-"));

        let error = publisher
            .publish_with(OsStr::new("record"), |_file| {
                Err(io::Error::other("injected write failure"))
            })
            .unwrap_err();

        assert_eq!(error.stage, AtomicPublishStage::WriteTemporary);
        assert_eq!(fs::read_dir(&temporary.0).unwrap().count(), 0);
    }

    #[test]
    fn activation_failure_removes_the_unpublished_temporary_file() {
        let temporary = TemporaryDirectory::new();
        let directory = temporary.open();
        fs::create_dir(temporary.0.join("record")).unwrap();
        let publisher = AtomicFilePublisher::new(&directory, TemporaryPrefix::new(".failure-"));

        let error = publisher
            .publish_bytes(OsStr::new("record"), b"new contents")
            .unwrap_err();

        assert_eq!(error.stage, AtomicPublishStage::Activate);
        assert_eq!(fs::read_dir(&temporary.0).unwrap().count(), 1);
        assert!(temporary.0.join("record").is_dir());
    }

    #[test]
    fn reclaiming_removes_only_temporaries_of_the_owning_prefix() {
        let temporary = TemporaryDirectory::new();
        let directory = temporary.open();
        let prefix = TemporaryPrefix::new(".record-");
        for name in [
            ".record-00000000-0000-4000-8000-000000000001.tmp",
            ".record-00000000-0000-4000-8000-000000000002.tmp",
            ".other-01234567.tmp",
            ".record-01234567.json",
            ".record-not-a-uuid.tmp",
            "active.json",
            ".record-.tmp",
        ] {
            fs::write(temporary.0.join(name), b"{}").unwrap();
        }

        let reclaimed = AtomicFilePublisher::new(&directory, prefix)
            .reclaim_abandoned(usize::MAX)
            .unwrap();

        assert_eq!(reclaimed, 2);
        let mut surviving: Vec<_> = fs::read_dir(&temporary.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        surviving.sort();
        assert_eq!(
            surviving,
            vec![
                ".other-01234567.tmp",
                ".record-.tmp",
                ".record-01234567.json",
                ".record-not-a-uuid.tmp",
                "active.json",
            ]
        );
    }

    #[test]
    fn reclaiming_a_directory_without_temporaries_changes_nothing() {
        let temporary = TemporaryDirectory::new();
        let directory = temporary.open();
        fs::write(temporary.0.join("active.json"), b"{}").unwrap();

        let reclaimed = AtomicFilePublisher::new(&directory, TemporaryPrefix::new(".record-"))
            .reclaim_abandoned(usize::MAX)
            .unwrap();

        assert_eq!(reclaimed, 0);
        assert_eq!(fs::read_dir(&temporary.0).unwrap().count(), 1);
    }

    #[test]
    fn reclaiming_recovers_a_temporary_abandoned_by_a_failed_publication() {
        let temporary = TemporaryDirectory::new();
        let directory = temporary.open();
        let prefix = TemporaryPrefix::new(".record-");
        // Model a crash: create the temporary the way publication does, then
        // leave it behind without renaming it.
        let abandoned = prefix.name_for(Uuid::new_v4());
        directory.create_new_file(OsStr::new(&abandoned)).unwrap();
        assert!(temporary.0.join(&abandoned).exists());

        let reclaimed = AtomicFilePublisher::new(&directory, prefix)
            .reclaim_abandoned(usize::MAX)
            .unwrap();

        assert_eq!(reclaimed, 1);
        assert!(!temporary.0.join(&abandoned).exists());
    }

    #[cfg(unix)]
    #[test]
    fn hard_link_destination_is_replaced_without_mutating_its_target() {
        let temporary = TemporaryDirectory::new();
        let directory = temporary.open();
        let external = temporary.0.join("external");
        fs::write(&external, b"external contents").unwrap();
        fs::hard_link(&external, temporary.0.join("record")).unwrap();

        AtomicFilePublisher::new(&directory, TemporaryPrefix::new(".record-"))
            .publish_bytes(OsStr::new("record"), b"new contents")
            .unwrap();

        assert_eq!(fs::read(external).unwrap(), b"external contents");
        assert_eq!(
            fs::read(temporary.0.join("record")).unwrap(),
            b"new contents"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_destination_is_replaced_without_mutating_its_target() {
        use std::os::unix::fs::symlink;

        let temporary = TemporaryDirectory::new();
        let directory = temporary.open();
        let external = temporary.0.join("external");
        fs::write(&external, b"external contents").unwrap();
        symlink(&external, temporary.0.join("record")).unwrap();

        AtomicFilePublisher::new(&directory, TemporaryPrefix::new(".record-"))
            .publish_bytes(OsStr::new("record"), b"new contents")
            .unwrap();

        assert_eq!(fs::read(external).unwrap(), b"external contents");
        assert_eq!(
            fs::read(temporary.0.join("record")).unwrap(),
            b"new contents"
        );
        assert!(!fs::symlink_metadata(temporary.0.join("record"))
            .unwrap()
            .file_type()
            .is_symlink());
    }
}
