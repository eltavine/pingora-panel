//! Sessions `ppanel login` keeps, one per API, in a file only its owner
//! can read: `$PPANEL_CONFIG_DIR`, `$XDG_CONFIG_HOME/ppanel` or
//! `~/.config/ppanel`, holding `credentials.json`.

use crate::client::{CliError, Result};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

pub struct Credentials {
    path: PathBuf,
}

/// A stored session.
pub struct Stored {
    pub secret: String,
    pub username: String,
}

impl Credentials {
    /// The file in the user's configuration directory, if there is one.
    pub fn locate() -> Option<Self> {
        let directory = std::env::var_os("PPANEL_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("XDG_CONFIG_HOME").map(|base| PathBuf::from(base).join("ppanel"))
            })
            .or_else(|| {
                std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .map(|home| PathBuf::from(home).join(".config").join("ppanel"))
            })?;
        Some(Self::at(&directory))
    }

    pub fn at(directory: &Path) -> Self {
        Self {
            path: directory.join("credentials.json"),
        }
    }

    fn read(&self) -> Map<String, Value> {
        std::fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default()
    }

    fn write(&self, entries: &Map<String, Value>) -> Result<()> {
        let failed = |error: std::io::Error| {
            CliError::Usage(format!("cannot write {}: {error}", self.path.display()))
        };
        if let Some(directory) = self.path.parent() {
            std::fs::create_dir_all(directory).map_err(failed)?;
        }
        let text = serde_json::to_vec_pretty(entries).expect("JSON values serialize");
        let temporary = self.path.with_extension("json.tmp");
        write_private(&temporary, &text).map_err(failed)?;
        std::fs::rename(&temporary, &self.path).map_err(failed)
    }

    pub fn load(&self, api: &str) -> Option<Stored> {
        let entry = self.read().remove(api)?;
        Some(Stored {
            secret: entry["secret"].as_str()?.to_owned(),
            username: entry["username"].as_str().unwrap_or_default().to_owned(),
        })
    }

    pub fn save(&self, api: &str, stored: &Stored) -> Result<()> {
        let mut entries = self.read();
        entries.insert(
            api.to_owned(),
            json!({ "secret": stored.secret, "username": stored.username }),
        );
        self.write(&entries)
    }

    pub fn remove(&self, api: &str) -> Result<()> {
        let mut entries = self.read();
        if entries.remove(api).is_some() {
            self.write(&entries)?;
        }
        Ok(())
    }
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_are_kept_per_api_in_a_private_file() {
        let directory = tempfile::tempdir().unwrap();
        let credentials = Credentials::at(&directory.path().join("ppanel"));
        assert!(credentials.load("http://a").is_none());
        let stored = |secret: &str| Stored {
            secret: secret.into(),
            username: "root".into(),
        };
        credentials.save("http://a", &stored("one")).unwrap();
        credentials.save("http://b", &stored("two")).unwrap();
        assert_eq!(credentials.load("http://a").unwrap().secret, "one");
        credentials.remove("http://a").unwrap();
        assert!(credentials.load("http://a").is_none());
        assert_eq!(credentials.load("http://b").unwrap().username, "root");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(directory.path().join("ppanel/credentials.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
