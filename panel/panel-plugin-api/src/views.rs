use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Every plugin found, and what the host itself offers them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[non_exhaustive]
pub struct PluginList {
    /// The plugin protocol versions the host speaks.
    pub protocol_versions: Vec<u32>,
    /// The ports a plugin may provide.
    pub ports: Vec<String>,
    /// The capabilities a plugin may ask for.
    pub capabilities: Vec<String>,
    /// Whether this host enforces plugins' resource limits.
    pub limits_enforced: bool,
    /// When the plugins directory was last read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovered_at: Option<DateTime<Utc>>,
    pub plugins: Vec<PluginView>,
}

/// Whether a plugin runs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PluginState {
    #[default]
    Disabled,
    Enabled,
    /// Enabled, but failing its health checks or exited.
    Degraded,
}

/// A plugin: its versions, what it was granted and how it runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[non_exhaustive]
pub struct PluginView {
    pub name: String,
    pub state: PluginState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_version: Option<String>,
    /// The version a rollback returns to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_version: Option<String>,
    /// Every version found, oldest first.
    pub versions: Vec<VersionView>,
    /// The capabilities granted; none are by default.
    pub grants: Vec<String>,
    /// The settings as written: secret references stay references.
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub settings: Value,
    /// The limits set; zero leaves the manifest's or the host's default.
    pub limits: PluginLimits,
    /// What the active version, or else the newest valid one, runs under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_limits: Option<PluginLimits>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<PluginHealth>,
    /// Why the plugin, though enabled, does not run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    pub etag: String,
}

/// One version of a plugin as its signed manifest describes it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[non_exhaustive]
pub struct VersionView {
    pub version: String,
    pub publisher: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub homepage: String,
    pub ports: Vec<String>,
    pub capabilities: Vec<String>,
    pub protocol_versions: Vec<u32>,
    /// Whether the host speaks one of its protocol versions.
    pub compatible: bool,
    /// The trusted key that signed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_by: Option<String>,
    /// Why it cannot run; empty when it can.
    pub problems: Vec<String>,
    /// The JSON Schema its settings must match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<Object>))]
    pub config_schema: Option<Value>,
    /// The resources it asks for.
    pub resources: PluginLimits,
    pub executable_sha256: String,
}

/// Resource limits; zero leaves a limit to the manifest or the host.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct PluginLimits {
    pub memory_bytes: u64,
    pub cpu_seconds: u32,
    pub open_files: u32,
    /// Calls served at once.
    pub concurrency: u32,
    /// The longest a call may take, in milliseconds.
    pub call_timeout_ms: u32,
}

/// Whether a running plugin answers its health checks.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HealthStatus {
    Serving,
    #[default]
    Degraded,
}

/// What the health checks of a running plugin found.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[non_exhaustive]
pub struct PluginHealth {
    pub status: HealthStatus,
    pub version: String,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<DateTime<Utc>>,
    /// Failed checks in a row.
    pub failures: u32,
    pub restarts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A publisher key whose signatures the host accepts.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[non_exhaustive]
pub struct TrustedKeyView {
    pub id: String,
    /// The minisign key ID, in hexadecimal.
    pub key_id: String,
    /// The minisign public key, in base64.
    pub public_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
    pub created_at: DateTime<Utc>,
}

/// A publisher key to trust.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct NewTrustedKey {
    pub id: String,
    /// The minisign public key: its base64 line, or the whole key file.
    pub public_key: String,
    #[serde(default)]
    pub comment: String,
}

/// A secret kept for plugins' settings; its value is never returned.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[non_exhaustive]
pub struct SecretView {
    pub name: String,
    pub updated_at: DateTime<Utc>,
}
