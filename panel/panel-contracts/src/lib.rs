//! Generated wire contracts for Pingora Panel internal services.
//!
//! This crate intentionally contains no domain logic. Convert generated values at
//! service boundaries instead of using them as the canonical domain model.

pub mod pingora {
    pub mod panel {
        pub mod common {
            pub mod v1 {
                tonic::include_proto!("pingora.panel.common.v1");
            }
        }

        pub mod gateway {
            pub mod v1 {
                tonic::include_proto!("pingora.panel.gateway.v1");
            }
        }
    }
}

pub use pingora::panel::{common, gateway};

pub const PROTOCOL_NAME: &str = "pingora.panel";
pub const PROTOCOL_VERSION: &str = "v1";

impl From<&panel_errors::Diagnostic> for common::v1::Diagnostic {
    fn from(value: &panel_errors::Diagnostic) -> Self {
        let severity = match value.severity {
            panel_errors::DiagnosticSeverity::Info => common::v1::DiagnosticSeverity::Info,
            panel_errors::DiagnosticSeverity::Warning => common::v1::DiagnosticSeverity::Warning,
            panel_errors::DiagnosticSeverity::Error => common::v1::DiagnosticSeverity::Error,
        };
        Self {
            code: value.code.to_string(),
            severity: severity.into(),
            message: value.message.clone(),
            source_span: value.source_span.clone().unwrap_or_default(),
            resource_id: value.resource_id.clone().unwrap_or_default(),
            help: value.help.clone().unwrap_or_default(),
        }
    }
}

impl From<&panel_errors::PanelError> for common::v1::Error {
    fn from(value: &panel_errors::PanelError) -> Self {
        Self {
            code: value.code.to_string(),
            message: value.message.clone(),
            retryable: value.retryable,
            diagnostics: value.diagnostics.iter().map(Into::into).collect(),
        }
    }
}

impl From<panel_errors::PanelError> for common::v1::Error {
    fn from(value: panel_errors::PanelError) -> Self {
        Self::from(&value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    #[test]
    fn request_context_round_trips() {
        let original = common::v1::RequestContext {
            request_id: "request-1".into(),
            correlation_id: "correlation-1".into(),
            actor: "operator@example.com".into(),
            deadline: "2026-08-27T12:00:00Z".into(),
            idempotency_key: "idempotency-1".into(),
            schema_version: "v1".into(),
        };
        let decoded =
            common::v1::RequestContext::decode(original.encode_to_vec().as_slice()).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn gateway_response_round_trips() {
        let original = gateway::v1::StatusResponse {
            version: Some(common::v1::Version {
                product: "pingora-panel".into(),
                component: "gateway".into(),
                build: "0.1.0".into(),
                schema: "v1".into(),
                protocol: "pingora.panel".into(),
                capability_set: "activation.cas".into(),
            }),
            health: Some(common::v1::HealthStatus {
                state: common::v1::health_status::State::Ready as i32,
                version: None,
                message: "ready".into(),
            }),
            active_revision_id: 7,
            active_hash: Some(common::v1::ContentHash {
                algorithm: "sha256".into(),
                value: "00".repeat(32),
            }),
            error: None,
            prepared_count: 0,
            runtime: Some(gateway::v1::GatewayRuntimeInfo {
                gateway_version: "0.1.0".into(),
                data_plane_version: "0.8.0".into(),
                adapter_version: "pingora-v1".into(),
                started_at_unix_seconds: 1_787_800_000,
                uptime_seconds: 42,
                worker_count: 4,
            }),
            event_delivery: Some(gateway::v1::EventDeliveryHealth {
                queue_full_events: 2,
                disconnected_events: 3,
                consumer_panics: 5,
            }),
        };
        let decoded =
            gateway::v1::StatusResponse::decode(original.encode_to_vec().as_slice()).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn stable_error_converts_without_internal_source() {
        let error = panel_errors::PanelError::new(panel_errors::ErrorCode::CONFLICT, "stale")
            .with_diagnostics(vec![panel_errors::Diagnostic::error(
                "CAS_MISMATCH",
                "changed",
            )]);
        let wire = common::v1::Error::from(&error);
        assert_eq!(wire.code, panel_errors::ErrorCode::CONFLICT);
        assert_eq!(wire.diagnostics.len(), 1);
    }
}
