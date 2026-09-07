use panel_application::GatewayStatus;
use panel_contracts::{common::v1 as common, gateway::v1 as wire};
use panel_domain::{ContentHash, RevisionId};
use panel_errors::{PanelError, Result};

/// Decode the transport status projection without leaking protobuf types into
/// the application adapter. Each wire field has one explicit domain owner:
/// health owns readiness/message, Version owns schema metadata, and runtime
/// owns the adapter version.
pub(super) fn decode(
    response: wire::StatusResponse,
    hash: impl Fn(Option<common::ContentHash>) -> Result<ContentHash>,
) -> Result<GatewayStatus> {
    let active_hash = response
        .active_hash
        .map(|value| hash(Some(value)))
        .transpose()?;
    let (ready, message) = decode_health(response.health.as_ref());
    let active_revision_id = decode_revision(response.active_revision_id);
    let schema_version = decode_schema_version(response.version.as_ref());
    let adapter_version = decode_adapter_version(response.runtime.as_ref());
    let prepared_count = usize::try_from(response.prepared_count).map_err(|_| {
        PanelError::invalid_argument("gateway prepared_count exceeds local usize capacity")
    })?;

    Ok(GatewayStatus::new(
        ready,
        message,
        active_revision_id,
        active_hash,
        prepared_count,
        adapter_version,
        schema_version,
    ))
}

fn decode_health(health: Option<&common::HealthStatus>) -> (bool, Option<String>) {
    let ready = health.is_some_and(|health| {
        common::health_status::State::try_from(health.state)
            .is_ok_and(|state| state == common::health_status::State::Ready)
    });
    let message = health
        .map(|health| health.message.clone())
        .filter(|message| !message.is_empty());
    (ready, message)
}

fn decode_revision(value: u64) -> Option<RevisionId> {
    (value != 0).then(|| RevisionId::new(value))
}

fn decode_schema_version(version: Option<&common::Version>) -> String {
    version
        .map(|version| version.schema.clone())
        .unwrap_or_default()
}

fn decode_adapter_version(runtime: Option<&wire::GatewayRuntimeInfo>) -> String {
    runtime
        .map(|runtime| runtime.adapter_version.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_domain::ContentHash;

    fn hash(value: Option<common::ContentHash>) -> Result<ContentHash> {
        let value = value.expect("test status hash");
        ContentHash::from_hex(value.value)
            .map_err(|error| panel_errors::PanelError::invalid_argument(error.to_string()))
    }

    #[test]
    fn decodes_each_status_field_from_its_protocol_owner() {
        let response = wire::StatusResponse {
            version: Some(common::Version {
                schema: "schema-789".into(),
                build: "build-123".into(),
                ..Default::default()
            }),
            health: Some(common::HealthStatus {
                state: common::health_status::State::NotReady.into(),
                message: "recovering".into(),
                ..Default::default()
            }),
            runtime: Some(wire::GatewayRuntimeInfo {
                adapter_version: "adapter-456".into(),
                ..Default::default()
            }),
            ..Default::default()
        };

        let status = decode(response, hash).unwrap();
        assert!(!status.ready());
        assert_eq!(status.message(), Some("recovering"));
        assert_eq!(status.adapter_version(), "adapter-456");
        assert_eq!(status.schema_version(), "schema-789");
    }

    #[cfg(target_pointer_width = "32")]
    #[test]
    fn rejects_prepared_count_that_cannot_fit_the_local_counter_type() {
        let response = wire::StatusResponse {
            prepared_count: u64::MAX,
            ..Default::default()
        };

        let error = decode(response, hash).unwrap_err();
        assert_eq!(
            error.code.as_str(),
            panel_errors::ErrorCode::INVALID_ARGUMENT
        );
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn accepts_the_full_wire_counter_range_on_64_bit_targets() {
        let response = wire::StatusResponse {
            prepared_count: u64::MAX,
            ..Default::default()
        };

        assert_eq!(decode(response, hash).unwrap().prepared_count(), usize::MAX);
    }
}
