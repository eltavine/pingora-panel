use crate::ProtocolRange;
use chrono::{DateTime, Utc};
use panel_context::ServiceName;
use panel_errors::{PanelError, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fmt};
use uuid::Uuid;

/// A named, versioned feature a service provides, such as `route.host@1`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "CapabilityFields", into = "CapabilityFields")]
pub struct Capability {
    name: String,
    version: String,
}

#[derive(Serialize, Deserialize)]
struct CapabilityFields {
    name: String,
    version: String,
}

impl TryFrom<CapabilityFields> for Capability {
    type Error = PanelError;

    fn try_from(value: CapabilityFields) -> Result<Self> {
        Self::new(value.name, value.version)
    }
}

impl From<Capability> for CapabilityFields {
    fn from(value: Capability) -> Self {
        Self {
            name: value.name,
            version: value.version,
        }
    }
}

impl Capability {
    /// `name` is dot-separated lowercase tokens; `version` is dot-separated
    /// decimal numbers.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Result<Self> {
        let name = name.into();
        let version = version.into();
        let token = |token: &str| {
            token.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        };
        if name.len() > 128 || !name.split('.').all(token) {
            return Err(PanelError::invalid_argument(format!(
                "capability `{name}` must be dot-separated lowercase tokens"
            )));
        }
        if version.len() > 32
            || !version
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return Err(PanelError::invalid_argument(format!(
                "capability `{name}` version must be dot-separated decimal numbers"
            )));
        }
        Ok(Self { name, version })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn version(&self) -> &str {
        &self.version
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.name, self.version)
    }
}

/// What one running service instance is and speaks.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ServiceDescriptor {
    service: ServiceName,
    instance_id: Uuid,
    build_version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    schema_version: String,
    protocols: Vec<ProtocolRange>,
    capabilities: BTreeSet<Capability>,
    started_at: DateTime<Utc>,
}

impl ServiceDescriptor {
    /// A descriptor for a process started at `started_at`, with a fresh
    /// time-ordered instance ID.
    pub fn new(
        service: ServiceName,
        build_version: impl Into<String>,
        started_at: DateTime<Utc>,
    ) -> Self {
        Self {
            service,
            instance_id: Uuid::now_v7(),
            build_version: build_version.into(),
            schema_version: String::new(),
            protocols: Vec::new(),
            capabilities: BTreeSet::new(),
            started_at,
        }
    }

    pub fn with_instance_id(mut self, instance_id: Uuid) -> Self {
        self.instance_id = instance_id;
        self
    }

    /// The version of the service's persistent schema.
    pub fn with_schema_version(mut self, schema_version: impl Into<String>) -> Self {
        self.schema_version = schema_version.into();
        self
    }

    /// Adds a spoken protocol, replacing a range for the same protocol.
    pub fn with_protocol(mut self, range: ProtocolRange) -> Self {
        self.protocols
            .retain(|existing| existing.name() != range.name());
        self.protocols.push(range);
        self.protocols.sort_by(|a, b| a.name().cmp(b.name()));
        self
    }

    pub fn with_capability(mut self, capability: Capability) -> Self {
        self.capabilities.insert(capability);
        self
    }

    pub fn service(&self) -> &ServiceName {
        &self.service
    }

    pub fn instance_id(&self) -> Uuid {
        self.instance_id
    }

    pub fn build_version(&self) -> &str {
        &self.build_version
    }

    pub fn schema_version(&self) -> &str {
        &self.schema_version
    }

    pub fn protocols(&self) -> &[ProtocolRange] {
        &self.protocols
    }

    pub fn protocol(&self, name: &str) -> Option<&ProtocolRange> {
        self.protocols.iter().find(|range| range.name() == name)
    }

    pub fn capabilities(&self) -> impl ExactSizeIterator<Item = &Capability> {
        self.capabilities.iter()
    }

    /// Whether the instance provides `name` in any version.
    pub fn provides(&self, name: &str) -> bool {
        self.capabilities
            .iter()
            .any(|capability| capability.name() == name)
    }

    pub fn started_at(&self) -> DateTime<Utc> {
        self.started_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_are_named_tokens_with_numeric_versions() {
        assert_eq!(
            Capability::new("route.path-prefix", "1.2")
                .unwrap()
                .to_string(),
            "route.path-prefix@1.2"
        );
        for (name, version) in [
            ("Route.host", "1"),
            ("route..host", "1"),
            ("route.host", "v1"),
            ("route.host", "1."),
            ("", "1"),
        ] {
            assert!(Capability::new(name, version).is_err(), "{name}@{version}");
        }
    }

    #[test]
    fn descriptors_keep_one_range_per_protocol_and_sorted_capabilities() {
        let descriptor = ServiceDescriptor::new(
            ServiceName::new("config-service").unwrap(),
            "0.1.0",
            Utc::now(),
        )
        .with_protocol(ProtocolRange::up_to("pingora.panel.platform.v1", 1).unwrap())
        .with_protocol(ProtocolRange::up_to("pingora.panel.platform.v1", 2).unwrap())
        .with_capability(Capability::new("revision.plan", "1").unwrap())
        .with_capability(Capability::new("revision.apply", "1").unwrap());
        assert_eq!(descriptor.protocols().len(), 1);
        assert_eq!(
            descriptor
                .protocol("pingora.panel.platform.v1")
                .unwrap()
                .max_revision(),
            2
        );
        assert_eq!(
            descriptor
                .capabilities()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["revision.apply@1", "revision.plan@1"]
        );
        assert!(descriptor.provides("revision.plan") && !descriptor.provides("revision"));
        assert_eq!(descriptor.instance_id().get_version_num(), 7);
    }
}
