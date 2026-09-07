#![forbid(unsafe_code)]

//! JSON configuration compiler adapter.
//!
//! JSON parsing and schema selection stay outside `panel-application`; a
//! future YAML/DSL compiler can implement the same port without changing the
//! use-case layer.

use panel_application::{ConfigCompiler, ConfigDocument};
use panel_errors::{PanelError, Result};
use panel_ir::{RuntimeSnapshot, IR_SCHEMA_VERSION};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JsonCompilerConfig {
    max_document_bytes: usize,
}

impl Default for JsonCompilerConfig {
    fn default() -> Self {
        Self {
            max_document_bytes: 2 * 1024 * 1024,
        }
    }
}

impl JsonCompilerConfig {
    pub fn with_max_document_bytes(mut self, value: usize) -> Self {
        self.max_document_bytes = value;
        self
    }

    pub fn max_document_bytes(self) -> usize {
        self.max_document_bytes
    }

    pub fn validate(self) -> Result<()> {
        if self.max_document_bytes == 0 {
            return Err(PanelError::invalid_argument(
                "JSON compiler document limit must be non-zero",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct JsonRuntimeSnapshotCompiler {
    config: JsonCompilerConfig,
}

impl JsonRuntimeSnapshotCompiler {
    pub fn new(config: JsonCompilerConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self { config })
    }

    pub fn config(&self) -> JsonCompilerConfig {
        self.config
    }
}

#[async_trait::async_trait]
impl ConfigCompiler for JsonRuntimeSnapshotCompiler {
    async fn compile(&self, document: ConfigDocument) -> Result<RuntimeSnapshot> {
        if document.media_type() != "application/json" {
            return Err(PanelError::invalid_argument(
                "JSON compiler requires media type application/json",
            ));
        }
        if document.body().len() > self.config.max_document_bytes {
            return Err(PanelError::resource_exhausted(
                "JSON configuration document exceeds compiler limit",
            ));
        }
        if document.schema_version() != IR_SCHEMA_VERSION {
            return Err(PanelError::unsupported_capability(format!(
                "unsupported JSON schema version: {}",
                document.schema_version()
            )));
        }
        let snapshot: RuntimeSnapshot =
            serde_json::from_slice(document.body()).map_err(|error| {
                PanelError::invalid_argument(format!("invalid JSON runtime snapshot: {error}"))
            })?;
        if snapshot.schema_version != document.schema_version() {
            return Err(PanelError::invalid_argument(
                "document schema version does not match runtime snapshot",
            ));
        }
        Ok(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_domain::RevisionId;

    fn document(snapshot: &RuntimeSnapshot) -> ConfigDocument {
        ConfigDocument::new(
            IR_SCHEMA_VERSION,
            "application/json",
            serde_json::to_vec(snapshot).unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn compiles_supported_snapshot_without_leaking_json_to_application() {
        let compiler = JsonRuntimeSnapshotCompiler::default();
        let snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        assert_eq!(
            compiler.compile(document(&snapshot)).await.unwrap(),
            snapshot
        );
    }

    #[tokio::test]
    async fn rejects_schema_media_type_and_size_before_decode() {
        let compiler = JsonRuntimeSnapshotCompiler::default();
        let wrong_media =
            ConfigDocument::new(IR_SCHEMA_VERSION, "text/plain", b"{}".to_vec()).unwrap();
        assert_eq!(
            compiler
                .compile(wrong_media)
                .await
                .unwrap_err()
                .code
                .as_str(),
            panel_errors::ErrorCode::INVALID_ARGUMENT
        );

        let compiler = JsonRuntimeSnapshotCompiler::new(
            JsonCompilerConfig::default().with_max_document_bytes(1),
        )
        .unwrap();
        assert_eq!(
            compiler
                .compile(document(&RuntimeSnapshot::empty(RevisionId::new(1))))
                .await
                .unwrap_err()
                .code
                .as_str(),
            panel_errors::ErrorCode::RESOURCE_EXHAUSTED
        );
    }
}
