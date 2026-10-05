#![forbid(unsafe_code)]

//! The sites' files on the local file system (ADR 0034): every path is
//! resolved beneath the sites' directory, opened as a capability, so that
//! neither a path nor a symbolic link leads out of it.

use async_trait::async_trait;
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use panel_application::{
    CommandContext, RequestScope, SiteDirectory, SiteEntry, SiteEntryKind, SiteFile,
    SiteFileWritten, SiteFilesPort, SitePath, SiteRemoval, WriteCondition, MOST_FILE_BYTES,
};
use panel_errors::{PanelError, Result};
use sha2::{Digest, Sha256};
use std::{
    io::{self, ErrorKind, Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;

/// The sites' directory, opened once.
pub struct LocalSiteFiles {
    root: Arc<Dir>,
    /// One change at a time, so that a conditional write sees what it
    /// replaces.
    changes: Mutex<()>,
}

impl LocalSiteFiles {
    /// Opens `root`, which must be a directory.
    pub fn open(root: &Path) -> io::Result<Self> {
        Ok(Self {
            root: Arc::new(Dir::open_ambient_dir(root, ambient_authority())?),
            changes: Mutex::new(()),
        })
    }

    async fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Dir) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let root = Arc::clone(&self.root);
        tokio::task::spawn_blocking(move || work(&root))
            .await
            .map_err(|error| PanelError::internal(format!("a file operation stopped: {error}")))?
    }
}

/// `path` relative to the sites' directory, `.` for the directory itself.
fn relative(path: &SitePath) -> PathBuf {
    if path.is_root() {
        PathBuf::from(".")
    } else {
        PathBuf::from(path.as_str())
    }
}

/// The directory above `path` and its name; refused for the directory itself.
fn split(path: &SitePath) -> Result<(PathBuf, String)> {
    let mut parts: Vec<&str> = path.parts().collect();
    let name = parts
        .pop()
        .ok_or_else(|| PanelError::invalid_argument("name a file below the sites' directory"))?;
    let parent = if parts.is_empty() {
        PathBuf::from(".")
    } else {
        PathBuf::from(parts.join("/"))
    };
    Ok((parent, name.to_owned()))
}

fn failure(path: &SitePath, error: &io::Error) -> PanelError {
    match error.kind() {
        ErrorKind::NotFound => PanelError::not_found(format!("{path} does not exist")),
        ErrorKind::AlreadyExists => PanelError::conflict(format!("{path} already exists")),
        ErrorKind::DirectoryNotEmpty => PanelError::conflict(format!(
            "{path} is not empty; remove it with what it holds to remove it"
        )),
        ErrorKind::NotADirectory => {
            PanelError::invalid_argument(format!("a part of {path} is not a directory"))
        }
        ErrorKind::IsADirectory => PanelError::invalid_argument(format!("{path} is a directory")),
        // Such as a symbolic link that leads out of the sites' directory.
        ErrorKind::PermissionDenied => {
            PanelError::permission_denied(format!("{path} cannot be reached: {error}"))
        }
        _ => PanelError::unavailable(format!("{path}: {error}")),
    }
}

fn kind(file_type: cap_std::fs::FileType) -> SiteEntryKind {
    if file_type.is_symlink() {
        SiteEntryKind::Link
    } else if file_type.is_dir() {
        SiteEntryKind::Directory
    } else if file_type.is_file() {
        SiteEntryKind::File
    } else {
        SiteEntryKind::Other
    }
}

fn modified(metadata: &cap_std::fs::Metadata) -> Option<SystemTime> {
    metadata.modified().ok().map(|time| time.into_std())
}

fn digest(content: &[u8]) -> String {
    hex::encode(Sha256::digest(content))
}

/// A strong entity tag over `content`.
fn tag(content: &[u8]) -> String {
    format!("\"{}\"", digest(content))
}

fn listed(root: &Dir, path: &SitePath) -> Result<SiteDirectory> {
    let directory = if path.is_root() {
        root.try_clone().map_err(|error| failure(path, &error))?
    } else {
        root.open_dir(relative(path))
            .map_err(|error| failure(path, &error))?
    };
    let mut entries = Vec::new();
    for entry in directory.entries().map_err(|error| failure(path, &error))? {
        let entry = entry.map_err(|error| failure(path, &error))?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let metadata = entry.metadata().map_err(|error| failure(path, &error))?;
        let kind = kind(metadata.file_type());
        entries.push(SiteEntry {
            name,
            size_bytes: if kind == SiteEntryKind::File {
                metadata.len()
            } else {
                0
            },
            kind,
            modified: modified(&metadata),
        });
    }
    entries.sort_by(|left, right| {
        (left.kind != SiteEntryKind::Directory, &left.name)
            .cmp(&(right.kind != SiteEntryKind::Directory, &right.name))
    });
    Ok(SiteDirectory {
        path: path.clone(),
        entries,
    })
}

fn read(root: &Dir, path: &SitePath) -> Result<SiteFile> {
    if path.is_root() {
        return Err(PanelError::invalid_argument(
            "the sites' directory is not a file",
        ));
    }
    let mut file = root
        .open(relative(path))
        .map_err(|error| failure(path, &error))?;
    let metadata = file.metadata().map_err(|error| failure(path, &error))?;
    if !metadata.is_file() {
        return Err(PanelError::invalid_argument(format!(
            "{path} is not a file"
        )));
    }
    if metadata.len() > MOST_FILE_BYTES {
        return Err(PanelError::precondition_failed(format!(
            "{path} is larger than 64 MiB"
        )));
    }
    let mut content = Vec::new();
    (&mut file)
        .take(MOST_FILE_BYTES + 1)
        .read_to_end(&mut content)
        .map_err(|error| failure(path, &error))?;
    if content.len() as u64 > MOST_FILE_BYTES {
        return Err(PanelError::precondition_failed(format!(
            "{path} is larger than 64 MiB"
        )));
    }
    Ok(SiteFile {
        path: path.clone(),
        tag: tag(&content),
        content,
        modified: modified(&metadata),
    })
}

/// What a path holds now: nothing, a file with its entity tag, or something
/// else.
enum Found {
    Nothing,
    File(String),
    Other(SiteEntryKind),
}

fn found(directory: &Dir, name: &str, path: &SitePath) -> Result<Found> {
    match directory.symlink_metadata(name) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(Found::Nothing),
        Err(error) => Err(failure(path, &error)),
        Ok(metadata) if metadata.is_file() => {
            let mut content = Vec::new();
            directory
                .open(name)
                .and_then(|mut file| file.read_to_end(&mut content))
                .map_err(|error| failure(path, &error))?;
            Ok(Found::File(tag(&content)))
        }
        Ok(metadata) => Ok(Found::Other(kind(metadata.file_type()))),
    }
}

fn written(
    root: &Dir,
    path: &SitePath,
    content: &[u8],
    condition: &WriteCondition,
) -> Result<SiteFileWritten> {
    if content.len() as u64 > MOST_FILE_BYTES {
        return Err(PanelError::invalid_argument("a file is at most 64 MiB"));
    }
    let (parent, name) = split(path)?;
    root.create_dir_all(&parent)
        .map_err(|error| failure(path, &error))?;
    let directory = root
        .open_dir(&parent)
        .map_err(|error| failure(path, &error))?;
    let before = found(&directory, &name, path)?;
    match (&before, condition) {
        (Found::Other(kind), _) => {
            return Err(PanelError::conflict(format!(
                "{path} is a {}, not a file",
                kind.as_str()
            )))
        }
        (Found::File(_), WriteCondition::Absent) => {
            return Err(PanelError::precondition_failed(format!(
                "{path} already exists"
            )))
        }
        (Found::File(current), WriteCondition::Tagged(expected)) if current != expected => {
            return Err(PanelError::precondition_failed(format!(
                "{path} changed since it was read"
            )))
        }
        (Found::Nothing, WriteCondition::Tagged(_)) => {
            return Err(PanelError::precondition_failed(format!(
                "{path} no longer exists"
            )))
        }
        _ => {}
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let temporary = format!(".{name}.{nanos}.writing");
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    let result = directory
        .open_with(&temporary, &options)
        .and_then(|mut file| {
            file.write_all(content)?;
            file.sync_all()
        })
        .and_then(|()| directory.rename(&temporary, &directory, &name));
    if let Err(error) = result {
        let _ = directory.remove_file(&temporary);
        return Err(failure(path, &error));
    }
    Ok(SiteFileWritten {
        path: path.clone(),
        size_bytes: content.len() as u64,
        sha256: digest(content),
        tag: tag(content),
        created: matches!(before, Found::Nothing),
    })
}

/// How many entries `relative` holds, itself included, without following
/// symbolic links.
fn count(root: &Dir, relative: &Path) -> io::Result<u64> {
    let metadata = root.symlink_metadata(relative)?;
    if !metadata.is_dir() {
        return Ok(1);
    }
    let mut total = 1;
    for entry in root.read_dir(relative)? {
        total += count(root, &relative.join(entry?.file_name()))?;
    }
    Ok(total)
}

fn removed(root: &Dir, path: &SitePath, recursive: bool) -> Result<SiteRemoval> {
    if path.is_root() {
        return Err(PanelError::invalid_argument(
            "the sites' directory itself is not removed",
        ));
    }
    let relative = relative(path);
    let metadata = root
        .symlink_metadata(&relative)
        .map_err(|error| failure(path, &error))?;
    let kind = kind(metadata.file_type());
    let removed = if kind == SiteEntryKind::Directory {
        if recursive {
            let total = count(root, &relative).map_err(|error| failure(path, &error))?;
            root.remove_dir_all(&relative)
                .map_err(|error| failure(path, &error))?;
            total
        } else {
            root.remove_dir(&relative)
                .map_err(|error| failure(path, &error))?;
            1
        }
    } else {
        root.remove_file(&relative)
            .map_err(|error| failure(path, &error))?;
        1
    };
    Ok(SiteRemoval {
        path: path.clone(),
        kind,
        removed,
    })
}

#[async_trait]
impl SiteFilesPort for LocalSiteFiles {
    async fn directory(&self, _: RequestScope, path: SitePath) -> Result<SiteDirectory> {
        self.blocking(move |root| listed(root, &path)).await
    }

    async fn file(&self, _: RequestScope, path: SitePath) -> Result<SiteFile> {
        self.blocking(move |root| read(root, &path)).await
    }

    async fn write_file(
        &self,
        _: CommandContext,
        path: SitePath,
        content: Vec<u8>,
        condition: WriteCondition,
    ) -> Result<SiteFileWritten> {
        let _change = self.changes.lock().await;
        self.blocking(move |root| written(root, &path, &content, &condition))
            .await
    }

    async fn create_directory(&self, _: CommandContext, path: SitePath) -> Result<()> {
        if path.is_root() {
            return Err(PanelError::invalid_argument("name a directory to create"));
        }
        let _change = self.changes.lock().await;
        self.blocking(move |root| {
            root.create_dir_all(relative(&path))
                .map_err(|error| failure(&path, &error))
        })
        .await
    }

    async fn remove(
        &self,
        _: CommandContext,
        path: SitePath,
        recursive: bool,
    ) -> Result<SiteRemoval> {
        let _change = self.changes.lock().await;
        self.blocking(move |root| removed(root, &path, recursive))
            .await
    }
}

#[cfg(test)]
mod tests;
