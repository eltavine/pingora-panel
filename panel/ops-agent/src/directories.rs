use panel_contracts::ops::v1::{
    self as wire, directories_server::Directories, DirectoryKind, DirectoryUsage,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};
use tonic::{Request, Response, Status};
use walkdir::WalkDir;

/// A walk stops after this many entries or this long, and says so.
const ENTRY_LIMIT: u64 = 1_000_000;
const TIME_LIMIT: Duration = Duration::from_secs(10);

/// The sizes of the configured directories, never of a caller's path.
pub(crate) struct DirectoryService {
    directories: Arc<[(DirectoryKind, PathBuf)]>,
}

impl DirectoryService {
    pub(crate) fn new(directories: Vec<(DirectoryKind, PathBuf)>) -> Self {
        Self {
            directories: directories.into(),
        }
    }
}

#[tonic::async_trait]
impl Directories for DirectoryService {
    async fn usage(
        &self,
        _: Request<wire::DirectoriesUsageRequest>,
    ) -> Result<Response<wire::DirectoriesUsageResponse>, Status> {
        let directories = Arc::clone(&self.directories);
        let usage = tokio::task::spawn_blocking(move || {
            directories
                .iter()
                .map(|(kind, path)| measure(*kind, path, Instant::now() + TIME_LIMIT))
                .collect()
        })
        .await
        .map_err(|_| Status::internal("the directory walk stopped"))?;
        Ok(Response::new(wire::DirectoriesUsageResponse {
            observed_at: Some(SystemTime::now().into()),
            directories: usage,
            error: None,
        }))
    }
}

/// Adds up the regular files below `path` on its own file system, without
/// following links.
fn measure(kind: DirectoryKind, path: &Path, deadline: Instant) -> DirectoryUsage {
    let mut usage = DirectoryUsage {
        kind: kind.into(),
        path: path.display().to_string(),
        present: path.is_dir(),
        ..DirectoryUsage::default()
    };
    if !usage.present {
        return usage;
    }
    let mut entries = 0u64;
    for entry in WalkDir::new(path).same_file_system(true) {
        entries += 1;
        if entries > ENTRY_LIMIT || (entries.is_multiple_of(1024) && Instant::now() > deadline) {
            usage.truncated = true;
            break;
        }
        match entry {
            Ok(entry) if entry.file_type().is_file() => match entry.metadata() {
                Ok(metadata) => {
                    usage.bytes += metadata.len();
                    usage.files += 1;
                }
                Err(_) => usage.unreadable += 1,
            },
            Ok(_) => {}
            Err(_) => usage.unreadable += 1,
        }
    }
    usage
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regular_files_add_up_without_following_links() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("a"), vec![0u8; 100]).unwrap();
        std::fs::create_dir(directory.path().join("nested")).unwrap();
        std::fs::write(directory.path().join("nested/b"), vec![0u8; 23]).unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::fs::write(elsewhere.path().join("big"), vec![0u8; 4096]).unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), directory.path().join("link")).unwrap();

        let usage = measure(
            DirectoryKind::Logs,
            directory.path(),
            Instant::now() + TIME_LIMIT,
        );
        assert!(usage.present);
        assert_eq!((usage.bytes, usage.files, usage.unreadable), (123, 2, 0));
        assert!(!usage.truncated);
        assert_eq!(usage.kind(), DirectoryKind::Logs);
    }

    #[test]
    fn missing_directories_are_absent_not_empty() {
        let directory = tempfile::tempdir().unwrap();
        let usage = measure(
            DirectoryKind::Certificates,
            &directory.path().join("missing"),
            Instant::now() + TIME_LIMIT,
        );
        assert!(!usage.present);
        assert_eq!((usage.bytes, usage.files), (0, 0));
    }

    #[test]
    fn a_walk_past_its_deadline_is_partial() {
        let directory = tempfile::tempdir().unwrap();
        for index in 0..2048 {
            std::fs::write(directory.path().join(index.to_string()), b"x").unwrap();
        }
        let usage = measure(DirectoryKind::Logs, directory.path(), Instant::now());
        assert!(usage.truncated);
        assert!(usage.files < 2048);
    }
}
