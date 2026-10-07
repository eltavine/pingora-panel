#![forbid(unsafe_code)]

//! The plugin protocol (ADR 0044): what plugins serve and what their
//! manifests say, generated from `proto/plugin/v1`. Manifests read and
//! write in the Proto3 JSON mapping, with the fields' own names.

#[allow(clippy::all)]
pub mod v1 {
    tonic::include_proto!("pingora.panel.plugin.v1");
    include!(concat!(
        env!("OUT_DIR"),
        "/pingora.panel.plugin.v1.serde.rs"
    ));
}

/// The application protocol version of this package, in go-plugin's terms.
pub const PROTOCOL_VERSION: u32 = 1;

/// The environment variable a plugin checks before it serves, so that one
/// started by hand says what it is instead of waiting for a host.
pub const MAGIC_COOKIE_KEY: &str = "PINGORA_PANEL_PLUGIN";
pub const MAGIC_COOKIE_VALUE: &str = "e1b3c2a9-6f4d-4c1e-9a7b-2d5f8c0e4b61";

/// The service a plugin reports its health for, as go-plugin requires.
pub const HEALTH_SERVICE: &str = "plugin";

/// The ports a plugin can provide, each also the capability it asks for to
/// be called for it.
pub const PORTS: [&str; 6] = [
    "dns01",
    "secrets",
    "notifications",
    "backups",
    "containers",
    "gateway",
];

/// The capability to receive the secrets a plugin's settings reference.
pub const SECRETS_CAPABILITY: &str = "secret-references";

/// The request metadata naming the plugin a port's call is for, which the
/// host routes by and removes before the call reaches the plugin.
pub const PLUGIN_METADATA: &str = "x-pingora-panel-plugin";

/// The plugin a setting names as `plugin:<name>`, as settings that choose
/// between what the product provides and what a plugin does write it.
pub fn plugin_of(value: &str) -> Option<&str> {
    let name = value.strip_prefix("plugin:")?;
    let valid = !name.is_empty()
        && name.len() <= 64
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    valid.then_some(name)
}

/// Sends each call of a port's client to one plugin, as a tonic
/// interceptor: `Dns01ProviderClient::with_interceptor(channel, plugin)`.
#[derive(Clone, Debug)]
pub struct Plugin(tonic::metadata::AsciiMetadataValue);

impl Plugin {
    pub fn named(name: &str) -> Result<Self, tonic::Status> {
        name.parse()
            .map(Self)
            .map_err(|_| tonic::Status::invalid_argument(format!("{name:?} is not a plugin name")))
    }
}

impl tonic::service::Interceptor for Plugin {
    fn call(
        &mut self,
        mut request: tonic::Request<()>,
    ) -> Result<tonic::Request<()>, tonic::Status> {
        request
            .metadata_mut()
            .insert(PLUGIN_METADATA, self.0.clone());
        Ok(request)
    }
}

/// The JSON Schema format of a setting that names a secret.
pub const SECRET_REFERENCE_FORMAT: &str = "secret-reference";

#[cfg(test)]
mod tests {
    use super::v1::{Manifest, Resources};

    #[test]
    fn calls_name_their_plugin() {
        use super::{plugin_of, Plugin, PLUGIN_METADATA};
        use tonic::service::Interceptor;
        let mut plugin = Plugin::named("acme-dns").unwrap();
        let request = plugin.call(tonic::Request::new(())).unwrap();
        assert_eq!(request.metadata().get(PLUGIN_METADATA).unwrap(), "acme-dns");
        assert!(Plugin::named("bad\nname").is_err());
        assert_eq!(plugin_of("plugin:edge-engine"), Some("edge-engine"));
        for refused in ["gatewayd", "plugin:", "plugin:Edge", "plugin:a/b", "edge"] {
            assert_eq!(plugin_of(refused), None, "{refused}");
        }
    }

    #[test]
    fn manifests_read_and_write_with_the_fields_own_names() {
        let manifest: Manifest = serde_json::from_value(serde_json::json!({
            "name": "dns-example",
            "version": "1.2.0",
            "executable": "bin/dns-example",
            "executable_sha256": "00",
            "protocol_versions": [1],
            "ports": ["dns01"],
            "capabilities": ["dns01"],
            "resources": {"memory_bytes": "268435456", "open_files": 64},
            "callTimeoutMs": 5000
        }))
        .unwrap();
        assert_eq!(manifest.call_timeout_ms, 5000);
        assert_eq!(
            manifest.resources,
            Some(Resources {
                memory_bytes: 256 << 20,
                open_files: 64,
                ..Resources::default()
            })
        );
        let written = serde_json::to_value(&manifest).unwrap();
        assert_eq!(written["executable_sha256"], "00");
        assert_eq!(written["protocol_versions"], serde_json::json!([1]));
    }
}
