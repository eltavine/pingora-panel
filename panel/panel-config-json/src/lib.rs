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
    use panel_domain::{NormalizedHost, RevisionId, SiteId};
    use panel_ir::{DomainSpec, SiteSpec};

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

    #[tokio::test]
    async fn unknown_and_invalid_nested_values_fail_closed() {
        let compiler = JsonRuntimeSnapshotCompiler::default();
        let baseline = serde_json::to_value(RuntimeSnapshot::empty(RevisionId::new(1))).unwrap();
        let compile = |value: serde_json::Value| {
            ConfigDocument::new(
                IR_SCHEMA_VERSION,
                "application/json",
                serde_json::to_vec(&value).unwrap(),
            )
            .unwrap()
        };

        let mut unknown_top = baseline.clone();
        unknown_top["listners"] = serde_json::json!([]);
        assert!(compiler.compile(compile(unknown_top)).await.is_err());

        let mut unknown_site = baseline.clone();
        unknown_site["sites"] = serde_json::json!([{
            "id": "site-1", "name": "site", "enabled": true, "domains": [],
            "enabeld": true
        }]);
        assert!(compiler.compile(compile(unknown_site)).await.is_err());

        let mut unknown_domain = baseline.clone();
        unknown_domain["sites"] = serde_json::json!([{
            "id": "site-1", "name": "site", "enabled": true,
            "domains": [{"host": "example.com", "tls_profile_id": null, "hostname": "ignored"}]
        }]);
        assert!(compiler.compile(compile(unknown_domain)).await.is_err());

        let mut invalid_site = baseline.clone();
        invalid_site["sites"] = serde_json::json!([{
            "id": "bad id", "name": "site", "enabled": true, "domains": []
        }]);
        assert!(compiler.compile(compile(invalid_site)).await.is_err());

        let mut invalid_hash = baseline.clone();
        invalid_hash["content_hash"] = serde_json::json!("not-a-hash");
        assert!(compiler.compile(compile(invalid_hash)).await.is_err());

        let mut unknown_route = baseline.clone();
        unknown_route["routes"] = serde_json::json!([{
            "id": "route-1", "site_id": "site-1", "priority": 0, "enabled": true,
            "matcher": {"kind": "path_prefix", "path": "/"},
            "action": {"kind": "static", "policy_id": "static-1"}
        }]);
        // Prove that the seed decodes before testing its extra field. A seed
        // with missing required fields would pass this negative test falsely.
        assert!(compiler
            .compile(compile(unknown_route.clone()))
            .await
            .is_ok());
        unknown_route["routes"][0]["extra"] = serde_json::json!("ignored");
        let error = compiler.compile(compile(unknown_route)).await.unwrap_err();
        assert!(error.message.contains("unknown field `extra`"), "{error}");

        assert!(compiler.compile(compile(baseline)).await.is_ok());

        let mut canonical = RuntimeSnapshot::empty(RevisionId::new(2));
        canonical.sites.push(SiteSpec {
            id: SiteId::new("site-1").unwrap(),
            name: "site".into(),
            enabled: true,
            domains: vec![DomainSpec {
                host: NormalizedHost::new("example.com").unwrap(),
                tls_profile_id: None,
            }],
        });
        canonical.refresh_content_hash();
        let mut wire = serde_json::to_value(&canonical).unwrap();
        wire["sites"][0]["domains"][0]["host"] = serde_json::json!("Example.COM.");
        let decoded = compiler.compile(compile(wire)).await.unwrap();
        assert_eq!(decoded.sites[0].domains[0].host.as_str(), "example.com");
        assert!(decoded.has_valid_content_hash());
    }
}
