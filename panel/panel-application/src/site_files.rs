//! The files of the static sites (ADR 0034), below the directory the
//! gateway serves them from.

use crate::{CommandContext, Operation, OperationLog, RequestScope};
use async_trait::async_trait;
use panel_errors::{PanelError, Result};
use std::{fmt, sync::Arc, time::SystemTime};

/// The largest file read or written whole.
pub const MOST_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MOST_PATH_BYTES: usize = 4096;
const MOST_NAME_BYTES: usize = 255;

/// A path below the sites' directory, written with `/`, each part a name;
/// the empty path is the directory itself.
#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SitePath(String);

impl SitePath {
    /// Refuses empty parts other than leading and trailing slashes, `.`,
    /// `..`, backslashes, NULs and control characters.
    pub fn parse(value: &str) -> Result<Self> {
        let trimmed = value.trim_matches('/');
        if trimmed.len() > MOST_PATH_BYTES {
            return Err(PanelError::invalid_argument(
                "the path is longer than 4096 bytes",
            ));
        }
        if trimmed.is_empty() {
            return Ok(Self::default());
        }
        for part in trimmed.split('/') {
            let refused = part.is_empty()
                || part == "."
                || part == ".."
                || part.len() > MOST_NAME_BYTES
                || part.chars().any(|c| c == '\\' || c.is_control());
            if refused {
                return Err(PanelError::invalid_argument(format!(
                    "`{value}` is not a path below the sites' directory"
                )));
            }
        }
        Ok(Self(trimmed.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// Its parts, outermost first.
    pub fn parts(&self) -> impl Iterator<Item = &str> {
        self.0.split('/').filter(|part| !part.is_empty())
    }

    /// Its last part; none for the directory itself.
    pub fn name(&self) -> Option<&str> {
        self.parts().last()
    }
}

impl fmt::Display for SitePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "/{}", self.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SiteEntryKind {
    File,
    Directory,
    /// A symbolic link, which is never followed out of the directory.
    Link,
    Other,
}

impl SiteEntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory",
            Self::Link => "link",
            Self::Other => "other",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SiteEntry {
    pub name: String,
    pub kind: SiteEntryKind,
    pub size_bytes: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SiteDirectory {
    pub path: SitePath,
    /// Directories first, then by name.
    pub entries: Vec<SiteEntry>,
}

#[derive(Clone, Eq, PartialEq)]
pub struct SiteFile {
    pub path: SitePath,
    pub content: Vec<u8>,
    /// A strong entity tag over its content.
    pub tag: String,
    pub modified: Option<SystemTime>,
}

impl fmt::Debug for SiteFile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SiteFile")
            .field("path", &self.path)
            .field("size_bytes", &self.content.len())
            .field("tag", &self.tag)
            .finish_non_exhaustive()
    }
}

/// What a write must find in place.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum WriteCondition {
    /// Anything, or nothing.
    Any,
    /// Nothing: the write creates the file.
    Absent,
    /// The file with this entity tag.
    Tagged(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SiteFileWritten {
    pub path: SitePath,
    pub size_bytes: u64,
    /// The content's SHA-256, in hexadecimal.
    pub sha256: String,
    pub tag: String,
    /// Whether the file was new.
    pub created: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SiteRemoval {
    pub path: SitePath,
    pub kind: SiteEntryKind,
    /// The files and directories removed with a directory, itself included.
    pub removed: u64,
}

#[async_trait]
pub trait SiteFilesPort: Send + Sync {
    async fn directory(&self, scope: RequestScope, path: SitePath) -> Result<SiteDirectory>;

    /// A file whole, up to [`MOST_FILE_BYTES`].
    async fn file(&self, scope: RequestScope, path: SitePath) -> Result<SiteFile>;

    /// Replaces or creates a file atomically, when `condition` holds.
    async fn write_file(
        &self,
        context: CommandContext,
        path: SitePath,
        content: Vec<u8>,
        condition: WriteCondition,
    ) -> Result<SiteFileWritten>;

    /// Creates a directory, and the directories above it.
    async fn create_directory(&self, context: CommandContext, path: SitePath) -> Result<()>;

    /// Removes a file, a link, or a directory: an empty one, or with what it
    /// holds when `recursive`.
    async fn remove(
        &self,
        context: CommandContext,
        path: SitePath,
        recursive: bool,
    ) -> Result<SiteRemoval>;
}

/// The port of an installation that does not manage the sites' files.
pub struct NoSiteFiles;

impl NoSiteFiles {
    fn refusal() -> PanelError {
        PanelError::unsupported_capability("the sites' directory is not managed here")
    }
}

#[async_trait]
impl SiteFilesPort for NoSiteFiles {
    async fn directory(&self, _: RequestScope, _: SitePath) -> Result<SiteDirectory> {
        Err(Self::refusal())
    }

    async fn file(&self, _: RequestScope, _: SitePath) -> Result<SiteFile> {
        Err(Self::refusal())
    }

    async fn write_file(
        &self,
        _: CommandContext,
        _: SitePath,
        _: Vec<u8>,
        _: WriteCondition,
    ) -> Result<SiteFileWritten> {
        Err(Self::refusal())
    }

    async fn create_directory(&self, _: CommandContext, _: SitePath) -> Result<()> {
        Err(Self::refusal())
    }

    async fn remove(&self, _: CommandContext, _: SitePath, _: bool) -> Result<SiteRemoval> {
        Err(Self::refusal())
    }
}

/// A change of the sites' files, as the audit trail records it.
#[derive(Clone, Copy, Debug)]
pub enum SiteFileChange<'a> {
    Written(std::result::Result<&'a SiteFileWritten, &'a PanelError>),
    DirectoryCreated(std::result::Result<(), &'a PanelError>),
    Removed {
        recursive: bool,
        result: std::result::Result<&'a SiteRemoval, &'a PanelError>,
    },
}

/// A sites' files port that records each change, refused or not.
pub struct RecordedSiteFiles {
    inner: Arc<dyn SiteFilesPort>,
    log: Arc<dyn OperationLog>,
}

impl RecordedSiteFiles {
    pub fn new(inner: Arc<dyn SiteFilesPort>, log: Arc<dyn OperationLog>) -> Self {
        Self { inner, log }
    }
}

#[async_trait]
impl SiteFilesPort for RecordedSiteFiles {
    async fn directory(&self, scope: RequestScope, path: SitePath) -> Result<SiteDirectory> {
        self.inner.directory(scope, path).await
    }

    async fn file(&self, scope: RequestScope, path: SitePath) -> Result<SiteFile> {
        self.inner.file(scope, path).await
    }

    async fn write_file(
        &self,
        context: CommandContext,
        path: SitePath,
        content: Vec<u8>,
        condition: WriteCondition,
    ) -> Result<SiteFileWritten> {
        let result = self
            .inner
            .write_file(context.clone(), path.clone(), content, condition)
            .await;
        let operation = Operation::SiteFile {
            path: &path,
            change: SiteFileChange::Written(result.as_ref()),
        };
        self.log.record(&context, operation).await;
        result
    }

    async fn create_directory(&self, context: CommandContext, path: SitePath) -> Result<()> {
        let result = self
            .inner
            .create_directory(context.clone(), path.clone())
            .await;
        let operation = Operation::SiteFile {
            path: &path,
            change: SiteFileChange::DirectoryCreated(result.as_ref().map(|_| ())),
        };
        self.log.record(&context, operation).await;
        result
    }

    async fn remove(
        &self,
        context: CommandContext,
        path: SitePath,
        recursive: bool,
    ) -> Result<SiteRemoval> {
        let result = self
            .inner
            .remove(context.clone(), path.clone(), recursive)
            .await;
        let operation = Operation::SiteFile {
            path: &path,
            change: SiteFileChange::Removed {
                recursive,
                result: result.as_ref(),
            },
        };
        self.log.record(&context, operation).await;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_names_below_the_directory() {
        for (value, expected) in [
            ("", ""),
            ("/", ""),
            ("shop", "shop"),
            ("/shop/assets/app.js", "shop/assets/app.js"),
            ("shop/", "shop"),
            (
                "shop/.well-known/security.txt",
                "shop/.well-known/security.txt",
            ),
            ("shop/caf\u{e9}.html", "shop/caf\u{e9}.html"),
        ] {
            assert_eq!(
                SitePath::parse(value).unwrap().as_str(),
                expected,
                "{value}"
            );
        }
        for value in [
            "..",
            "shop/../..",
            "shop/./index.html",
            "shop//index.html",
            "shop\\..\\etc",
            "shop/\0",
            "shop/line\nbreak",
        ] {
            assert!(SitePath::parse(value).is_err(), "{value:?}");
        }
        let long = "a".repeat(256);
        assert!(SitePath::parse(&long).is_err());
        let path = SitePath::parse("shop/assets/app.js").unwrap();
        assert_eq!(path.name(), Some("app.js"));
        assert_eq!(path.to_string(), "/shop/assets/app.js");
        assert_eq!(SitePath::default().name(), None);
    }

    #[test]
    fn files_never_print_their_content() {
        let file = SiteFile {
            path: SitePath::parse("shop/.env").unwrap(),
            content: b"TOKEN=hunter2".to_vec(),
            tag: "\"t\"".into(),
            modified: None,
        };
        let printed = format!("{file:?}");
        assert!(
            !printed.contains("hunter2") && printed.contains("size_bytes: 13"),
            "{printed}"
        );
    }
}
