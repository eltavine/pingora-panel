//! Operator changes to the running gateway that persist across restarts.

use panel_errors::{PanelError, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    io::Write,
    path::{Path, PathBuf},
};
use tokio::sync::Mutex;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeSettings {
    /// Overrides the configured worker count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_count: Option<u32>,
    /// Endpoints an operator took out of rotation, as `[upstream, endpoint]`.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub drained: BTreeSet<(String, String)>,
}

/// `<state>/runtime/settings.json`, written atomically.
pub(crate) struct RuntimeSettingsStore {
    path: PathBuf,
    writes: Mutex<()>,
}

impl RuntimeSettingsStore {
    pub(crate) fn new(state_directory: &Path) -> Self {
        Self {
            path: state_directory.join("runtime").join("settings.json"),
            writes: Mutex::new(()),
        }
    }

    pub(crate) async fn load(&self) -> Result<RuntimeSettings> {
        match tokio::fs::read(&self.path).await {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
                PanelError::corrupt_state(format!("{} is invalid: {error}", self.path.display()))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(RuntimeSettings::default())
            }
            Err(error) => Err(PanelError::storage_unavailable(format!(
                "cannot read {}: {error}",
                self.path.display()
            ))),
        }
    }

    /// Applies `change` and persists the result before returning it.
    pub(crate) async fn update(
        &self,
        change: impl FnOnce(&mut RuntimeSettings),
    ) -> Result<RuntimeSettings> {
        let _writing = self.writes.lock().await;
        let mut settings = self.load().await?;
        change(&mut settings);
        let path = self.path.clone();
        let bytes = serde_json::to_vec_pretty(&settings).expect("settings serialize");
        tokio::task::spawn_blocking(move || write_atomically(&path, &bytes))
            .await
            .map_err(|error| PanelError::internal(format!("settings write stopped: {error}")))??;
        Ok(settings)
    }
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let failed = |error: std::io::Error| {
        PanelError::storage_unavailable(format!("cannot write {}: {error}", path.display()))
    };
    let directory = path.parent().expect("settings live in a directory");
    std::fs::create_dir_all(directory).map_err(failed)?;
    let mut file = tempfile::NamedTempFile::new_in(directory).map_err(failed)?;
    file.write_all(bytes).map_err(failed)?;
    file.as_file().sync_all().map_err(failed)?;
    file.persist(path).map_err(|error| failed(error.error))?;
    std::fs::File::open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn settings_survive_a_new_store() {
        let state = tempfile::tempdir().unwrap();
        let store = RuntimeSettingsStore::new(state.path());
        assert_eq!(store.load().await.unwrap(), RuntimeSettings::default());
        store
            .update(|settings| {
                settings.worker_count = Some(4);
                settings.drained.insert(("app".into(), "node".into()));
            })
            .await
            .unwrap();
        let reloaded = RuntimeSettingsStore::new(state.path())
            .load()
            .await
            .unwrap();
        assert_eq!(reloaded.worker_count, Some(4));
        assert!(reloaded.drained.contains(&("app".into(), "node".into())));
        std::fs::write(state.path().join("runtime/settings.json"), b"{").unwrap();
        assert!(store.load().await.is_err());
    }
}
