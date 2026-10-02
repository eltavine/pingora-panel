#![forbid(unsafe_code)]

//! Protobuf representation of platform service descriptions.

use chrono::{DateTime, Utc};
use panel_contracts::{common::v1 as common, platform::v1 as wire, ProtocolRevisions};
use panel_errors::{PanelError, Result};
use panel_platform::{Capability, ProtocolRange, ServiceDescriptor, ServiceName};
use prost::Message;
use uuid::Uuid;

const PRODUCT: &str = "pingora-panel";

/// The range a build speaks for one generated protocol package.
pub fn protocol_range(revisions: ProtocolRevisions) -> ProtocolRange {
    ProtocolRange::new(revisions.package, revisions.min, revisions.max)
        .expect("generated protocol revisions are valid")
}

pub fn encode_descriptor(descriptor: &ServiceDescriptor) -> wire::ServiceDescriptor {
    let join = |values: Vec<String>| values.join(",");
    wire::ServiceDescriptor {
        service: descriptor.service().as_str().into(),
        instance_id: descriptor.instance_id().to_string(),
        version: Some(common::Version {
            product: PRODUCT.into(),
            component: descriptor.service().as_str().into(),
            build: descriptor.build_version().into(),
            schema: descriptor.schema_version().into(),
            protocol: join(
                descriptor
                    .protocols()
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
            ),
            capability_set: join(descriptor.capabilities().map(ToString::to_string).collect()),
        }),
        protocols: descriptor
            .protocols()
            .iter()
            .map(|range| wire::ProtocolSupport {
                name: range.name().into(),
                min_revision: range.min_revision(),
                max_revision: range.max_revision(),
            })
            .collect(),
        capabilities: descriptor
            .capabilities()
            .map(|capability| common::Capability {
                name: capability.name().into(),
                version: capability.version().into(),
            })
            .collect(),
        started_at: Some(timestamp(descriptor.started_at())),
    }
}

/// Decodes a descriptor. The summary strings in `version` are derived and
/// ignored; the repeated fields are authoritative.
pub fn decode_descriptor(value: wire::ServiceDescriptor) -> Result<ServiceDescriptor> {
    let invalid =
        |field: &str| PanelError::invalid_argument(format!("invalid service descriptor {field}"));
    let instance_id = Uuid::parse_str(&value.instance_id).map_err(|_| invalid("instance_id"))?;
    let started_at = value
        .started_at
        .and_then(|time| {
            DateTime::<Utc>::from_timestamp(time.seconds, u32::try_from(time.nanos).ok()?)
        })
        .ok_or_else(|| invalid("started_at"))?;
    let version = value.version.unwrap_or_default();
    let mut descriptor =
        ServiceDescriptor::new(ServiceName::new(value.service)?, version.build, started_at)
            .with_instance_id(instance_id)
            .with_schema_version(version.schema);
    for protocol in value.protocols {
        descriptor = descriptor.with_protocol(ProtocolRange::new(
            protocol.name,
            protocol.min_revision,
            protocol.max_revision,
        )?);
    }
    for capability in value.capabilities {
        descriptor =
            descriptor.with_capability(Capability::new(capability.name, capability.version)?);
    }
    Ok(descriptor)
}

pub fn encode_descriptor_bytes(descriptor: &ServiceDescriptor) -> Vec<u8> {
    encode_descriptor(descriptor).encode_to_vec()
}

pub fn decode_descriptor_bytes(bytes: &[u8]) -> Result<ServiceDescriptor> {
    let value = wire::ServiceDescriptor::decode(bytes).map_err(|error| {
        PanelError::invalid_argument(format!("undecodable service descriptor: {error}"))
    })?;
    decode_descriptor(value)
}

fn timestamp(time: DateTime<Utc>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: time.timestamp(),
        nanos: i32::try_from(time.timestamp_subsec_nanos()).unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_contracts::{GATEWAY_V1, PLATFORM_V1};

    fn descriptor() -> ServiceDescriptor {
        ServiceDescriptor::new(
            ServiceName::new("config-service").unwrap(),
            "0.1.0",
            DateTime::parse_from_rfc3339("2026-10-03T05:00:00.123456789Z")
                .unwrap()
                .with_timezone(&Utc),
        )
        .with_schema_version("2")
        .with_protocol(protocol_range(GATEWAY_V1))
        .with_protocol(protocol_range(PLATFORM_V1))
        .with_capability(Capability::new("revision.plan", "1").unwrap())
    }

    #[test]
    fn descriptors_round_trip_through_protobuf() {
        let original = descriptor();
        let decoded = decode_descriptor_bytes(&encode_descriptor_bytes(&original)).unwrap();
        assert_eq!(decoded, original);

        let wire = encode_descriptor(&original);
        let version = wire.version.unwrap();
        assert_eq!(version.component, "config-service");
        assert_eq!(
            version.protocol,
            "pingora.panel.gateway.v1@1..=1,pingora.panel.platform.v1@1..=1"
        );
        assert_eq!(version.capability_set, "revision.plan@1");
    }

    #[test]
    fn invalid_descriptors_are_rejected() {
        let mut wire = encode_descriptor(&descriptor());
        wire.instance_id = "not-a-uuid".into();
        assert!(decode_descriptor(wire).is_err());

        let mut wire = encode_descriptor(&descriptor());
        wire.started_at = None;
        assert!(decode_descriptor(wire).is_err());

        let mut wire = encode_descriptor(&descriptor());
        wire.protocols[0].min_revision = 0;
        assert!(decode_descriptor(wire).is_err());

        assert!(decode_descriptor_bytes(&[0xff, 0xff]).is_err());
    }
}
