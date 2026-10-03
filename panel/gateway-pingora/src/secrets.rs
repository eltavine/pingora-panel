//! Secret material referenced by IR identifiers.
//!
//! Snapshots name secrets; the material stays with the gateway so it never
//! appears in configuration documents, events or the snapshot store.

use panel_errors::{PanelError, Result};
use std::path::PathBuf;

pub trait SecretSource: Send + Sync {
    fn read(&self, id: &str) -> Result<Vec<u8>>;

    /// Who may use the secret, when the source can tell.
    fn permissions(&self, _id: &str) -> Option<Result<SecretPermissions>> {
        None
    }
}

/// Who may use a stored secret.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct SecretPermissions {
    /// Unix permission bits, where the platform has them.
    pub mode: Option<u32>,
    /// Whether only the owner may read or write it.
    pub owner_only: bool,
}

/// Reads each secret from `<directory>/<id>`.
#[derive(Clone, Debug)]
pub struct DirectorySecrets {
    directory: PathBuf,
}

impl DirectorySecrets {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }
}

impl SecretSource for DirectorySecrets {
    fn read(&self, id: &str) -> Result<Vec<u8>> {
        if !is_file_name(id) {
            return Err(PanelError::validation_failed(format!(
                "secret id {id:?} must be a plain file name"
            )));
        }
        std::fs::read(self.directory.join(id)).map_err(|error| {
            PanelError::validation_failed(format!("secret {id} is unavailable: {error}"))
        })
    }

    fn permissions(&self, id: &str) -> Option<Result<SecretPermissions>> {
        if !is_file_name(id) {
            return Some(Err(PanelError::validation_failed(format!(
                "secret id {id:?} must be a plain file name"
            ))));
        }
        Some(
            std::fs::metadata(self.directory.join(id))
                .map(|metadata| permissions(&metadata))
                .map_err(|error| {
                    PanelError::validation_failed(format!("secret {id} is unavailable: {error}"))
                }),
        )
    }
}

#[cfg(unix)]
fn permissions(metadata: &std::fs::Metadata) -> SecretPermissions {
    use std::os::unix::fs::PermissionsExt;
    let mode = metadata.permissions().mode() & 0o7777;
    SecretPermissions {
        mode: Some(mode),
        owner_only: mode & 0o077 == 0,
    }
}

#[cfg(not(unix))]
fn permissions(_metadata: &std::fs::Metadata) -> SecretPermissions {
    SecretPermissions {
        mode: None,
        owner_only: true,
    }
}

/// Used when the gateway has no secret directory.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoSecrets;

impl SecretSource for NoSecrets {
    fn read(&self, id: &str) -> Result<Vec<u8>> {
        Err(PanelError::validation_failed(format!(
            "secret {id} is unavailable: the gateway has no secret directory"
        )))
    }
}

fn is_file_name(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_cannot_leave_the_directory() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("site.pem"), b"material").unwrap();
        let secrets = DirectorySecrets::new(directory.path());
        assert_eq!(secrets.read("site.pem").unwrap(), b"material");
        for id in ["../site.pem", ".hidden", "a/b", "", "missing"] {
            assert!(secrets.read(id).is_err(), "{id}");
        }
        assert!(NoSecrets.read("site.pem").is_err());
    }
}
