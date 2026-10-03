#![forbid(unsafe_code)]

//! Generated wire contracts for Pingora Panel internal services.
//!
//! This crate intentionally contains no domain logic. Convert generated values at
//! service boundaries instead of using them as the canonical domain model.

pub mod pingora {
    pub mod panel {
        pub mod audit {
            pub mod v1 {
                tonic::include_proto!("pingora.panel.audit.v1");
            }
        }

        pub mod common {
            pub mod v1 {
                tonic::include_proto!("pingora.panel.common.v1");
            }
        }

        pub mod config {
            pub mod v1 {
                tonic::include_proto!("pingora.panel.config.v1");
            }
        }

        pub mod gateway {
            pub mod v1 {
                tonic::include_proto!("pingora.panel.gateway.v1");
            }
        }

        pub mod platform {
            pub mod v1 {
                tonic::include_proto!("pingora.panel.platform.v1");
            }
        }
    }
}

pub use pingora::panel::{audit, common, config, gateway, platform};

/// The CloudEvents Protobuf format, generated from the vendored official schema.
pub mod cloudevents {
    pub mod v1 {
        tonic::include_proto!("io.cloudevents.v1");
    }
}

pub const PROTOCOL_NAME: &str = "pingora.panel";
pub const PROTOCOL_VERSION: &str = "v1";

/// The revisions of one protocol package this build speaks.
///
/// Raise `max` with every additive change to the package; raise `min` only
/// after the revisions below it are retired by every deployed peer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtocolRevisions {
    pub package: &'static str,
    pub min: u32,
    pub max: u32,
}

pub const AUDIT_V1: ProtocolRevisions = ProtocolRevisions {
    package: "pingora.panel.audit.v1",
    min: 1,
    max: 1,
};

pub const CONFIG_V1: ProtocolRevisions = ProtocolRevisions {
    package: "pingora.panel.config.v1",
    min: 1,
    max: 1,
};

pub const GATEWAY_V1: ProtocolRevisions = ProtocolRevisions {
    package: "pingora.panel.gateway.v1",
    min: 1,
    max: 1,
};

pub const PLATFORM_V1: ProtocolRevisions = ProtocolRevisions {
    package: "pingora.panel.platform.v1",
    min: 1,
    max: 1,
};

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

impl From<common::v1::Diagnostic> for panel_errors::Diagnostic {
    fn from(value: common::v1::Diagnostic) -> Self {
        let severity = match common::v1::DiagnosticSeverity::try_from(value.severity)
            .unwrap_or(common::v1::DiagnosticSeverity::Error)
        {
            common::v1::DiagnosticSeverity::Info => panel_errors::DiagnosticSeverity::Info,
            common::v1::DiagnosticSeverity::Warning => panel_errors::DiagnosticSeverity::Warning,
            _ => panel_errors::DiagnosticSeverity::Error,
        };
        let optional = |value: String| (!value.is_empty()).then_some(value);
        Self {
            code: panel_errors::ErrorCode::new(value.code),
            severity,
            message: value.message,
            source_span: optional(value.source_span),
            resource_id: optional(value.resource_id),
            help: optional(value.help),
        }
    }
}

impl From<common::v1::Error> for panel_errors::PanelError {
    fn from(value: common::v1::Error) -> Self {
        Self::new(value.code, value.message)
            .retryable(value.retryable)
            .with_diagnostics(value.diagnostics.into_iter().map(Into::into).collect())
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
                data_plane_version: "0.9.0".into(),
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
            recovery: Some(gateway::v1::RecoveryHealth {
                recovery_completed: 7,
                degraded_events: 11,
                unknown_commit_outcomes: 13,
            }),
        };
        let decoded =
            gateway::v1::StatusResponse::decode(original.encode_to_vec().as_slice()).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn protocol_revisions_name_generated_packages() {
        for (revisions, service) in [
            (CONFIG_V1, config::v1::publication_server::SERVICE_NAME),
            (GATEWAY_V1, gateway::v1::gateway_engine_server::SERVICE_NAME),
            (PLATFORM_V1, platform::v1::service_info_server::SERVICE_NAME),
        ] {
            assert_eq!(service.rsplit_once('.').unwrap().0, revisions.package);
            assert!(1 <= revisions.min && revisions.min <= revisions.max);
        }
    }

    #[test]
    fn wire_errors_convert_back_with_diagnostics() {
        let original = panel_errors::PanelError::new(panel_errors::ErrorCode::CONFLICT, "stale")
            .retryable(true)
            .with_diagnostics(vec![panel_errors::Diagnostic {
                help: Some("reload".into()),
                ..panel_errors::Diagnostic::error("CAS_MISMATCH", "changed")
            }]);
        let decoded = panel_errors::PanelError::from(common::v1::Error::from(&original));
        assert_eq!(decoded.code, original.code);
        assert_eq!(decoded.message, original.message);
        assert!(decoded.retryable);
        assert_eq!(decoded.diagnostics, original.diagnostics);
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
