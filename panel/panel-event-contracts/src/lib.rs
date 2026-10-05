#![forbid(unsafe_code)]

//! The data of domain events, generated from `proto/events` (ADR 0004). Each
//! message is the data of one event type and travels as JSON in the proto3
//! JSON mapping, keeping the field names of its definition, with the
//! message's type URL as its `dataschema`. Buf guards the definitions against
//! breaking changes.

use panel_events::{EventData, PROTOBUF_TYPE_URL_PREFIX};

macro_rules! packages {
    ($($module:ident => $package:literal,)*) => {
        $(
            pub mod $module {
                #[allow(clippy::all)]
                pub mod v1 {
                    include!(concat!(env!("OUT_DIR"), "/", $package, ".rs"));
                    include!(concat!(env!("OUT_DIR"), "/", $package, ".serde.rs"));
                }
            }
        )*
    };
}

packages! {
    automation => "pingora.panel.events.automation.v1",
    config => "pingora.panel.events.config.v1",
    containers => "pingora.panel.events.containers.v1",
    files => "pingora.panel.events.files.v1",
    gateway => "pingora.panel.events.gateway.v1",
    host => "pingora.panel.events.host.v1",
    identity => "pingora.panel.events.identity.v1",
    observability => "pingora.panel.events.observability.v1",
    tls => "pingora.panel.events.tls.v1",
}

macro_rules! event_types {
    ($($message:ty => $name:literal,)*) => {
        $(
            impl EventData for $message {
                const TYPE: &'static str = $name;

                fn schema() -> Option<String> {
                    Some(format!(
                        "{PROTOBUF_TYPE_URL_PREFIX}{}",
                        <$message as prost::Name>::full_name()
                    ))
                }
            }
        )*

        /// Every event type with a definition.
        pub const EVENT_TYPES: &[&str] = &[$($name),*];
    };
}

event_types! {
    automation::v1::JobQueued => "automation.job.queued",
    automation::v1::JobStarted => "automation.job.started",
    automation::v1::JobProgressed => "automation.job.progressed",
    automation::v1::JobSucceeded => "automation.job.succeeded",
    automation::v1::JobRetrying => "automation.job.retrying",
    automation::v1::JobFailed => "automation.job.failed",
    automation::v1::JobCancelled => "automation.job.cancelled",
    config::v1::DraftChanged => "config.draft.changed",
    config::v1::DraftApplied => "config.draft.applied",
    config::v1::ApplyChecked => "config.apply.checked",
    config::v1::ApplyRejected => "config.apply.rejected",
    config::v1::ApplyFailed => "config.apply.failed",
    config::v1::RevisionNoted => "config.revision.noted",
    config::v1::ChangeRefused => "config.change.refused",
    config::v1::ApprovalPolicyCreated => "config.approval_policy.created",
    config::v1::ApprovalPolicyUpdated => "config.approval_policy.updated",
    config::v1::ApprovalPolicyDeleted => "config.approval_policy.deleted",
    config::v1::ApprovalRequested => "config.approval.requested",
    config::v1::ApprovalApproved => "config.approval.approved",
    config::v1::ApprovalRejected => "config.approval.rejected",
    config::v1::ApprovalWithdrawn => "config.approval.withdrawn",
    config::v1::ApprovalOutdated => "config.approval.outdated",
    config::v1::ApprovalRevoked => "config.approval.revoked",
    config::v1::ApprovalApplied => "config.approval.applied",
    config::v1::ApprovalBypassed => "config.approval.bypassed",
    gateway::v1::SnapshotPrepared => "gateway.snapshot.prepared",
    gateway::v1::SnapshotActivated => "gateway.snapshot.activated",
    gateway::v1::SnapshotAborted => "gateway.snapshot.aborted",
    gateway::v1::SnapshotRefused => "gateway.snapshot.refused",
    gateway::v1::Reloaded => "gateway.reloaded",
    gateway::v1::WorkersChanged => "gateway.workers.changed",
    gateway::v1::ShutdownRequested => "gateway.shutdown.requested",
    gateway::v1::EndpointDrained => "gateway.endpoint.drained",
    gateway::v1::EndpointRestored => "gateway.endpoint.restored",
    gateway::v1::LogsDeleted => "gateway.logs.deleted",
    gateway::v1::OperationRefused => "gateway.operation.refused",
    identity::v1::AccountCreated => "identity.account.created",
    identity::v1::AccountUpdated => "identity.account.updated",
    identity::v1::PasswordChanged => "identity.password.changed",
    identity::v1::AccessDenied => "identity.access.denied",
    identity::v1::LoginFailed => "identity.login.failed",
    identity::v1::LoginSucceeded => "identity.login.succeeded",
    identity::v1::BreakGlassUsed => "identity.break_glass.used",
    identity::v1::SessionEnded => "identity.session.ended",
    identity::v1::TokenCreated => "identity.token.created",
    identity::v1::TokenRevoked => "identity.token.revoked",
    identity::v1::TokenRotated => "identity.token.rotated",
    identity::v1::RoleCreated => "identity.role.created",
    identity::v1::RoleUpdated => "identity.role.updated",
    identity::v1::RoleDeleted => "identity.role.deleted",
    identity::v1::GrantCreated => "identity.grant.created",
    identity::v1::GrantDeleted => "identity.grant.deleted",
    identity::v1::SignInPolicyUpdated => "identity.sign_in_policy.updated",
    identity::v1::ProviderCreated => "identity.provider.created",
    identity::v1::ProviderUpdated => "identity.provider.updated",
    identity::v1::ProviderDeleted => "identity.provider.deleted",
    identity::v1::WorkloadTrustCreated => "identity.workload_trust.created",
    identity::v1::WorkloadTrustUpdated => "identity.workload_trust.updated",
    identity::v1::WorkloadTrustDeleted => "identity.workload_trust.deleted",
    tls::v1::CertificateCreated => "tls.certificate.created",
    tls::v1::CertificateReplaced => "tls.certificate.replaced",
    tls::v1::CertificateDeleted => "tls.certificate.deleted",
    tls::v1::CertificateExpiring => "tls.certificate.expiring",
    tls::v1::CertificateRefused => "tls.certificate.refused",
    tls::v1::AcmeAccountCreated => "tls.acme.account.created",
    tls::v1::AcmeAccountDeleted => "tls.acme.account.deleted",
    tls::v1::AcmeAccountRefused => "tls.acme.account.refused",
    tls::v1::AcmeCertificateCreated => "tls.acme.certificate.created",
    tls::v1::AcmeCertificateRenewalRequested => "tls.acme.certificate.renewal_requested",
    tls::v1::AcmeCertificateDeleted => "tls.acme.certificate.deleted",
    tls::v1::AcmeCertificateFailed => "tls.acme.certificate.failed",
    tls::v1::AcmeCertificateRefused => "tls.acme.certificate.refused",
    tls::v1::DnsProviderCreated => "tls.acme.dns_provider.created",
    tls::v1::DnsProviderUpdated => "tls.acme.dns_provider.updated",
    tls::v1::DnsProviderDeleted => "tls.acme.dns_provider.deleted",
    tls::v1::DnsProviderRefused => "tls.acme.dns_provider.refused",
    observability::v1::AlertRuleCreated => "observability.alert_rule.created",
    observability::v1::AlertRuleUpdated => "observability.alert_rule.updated",
    observability::v1::AlertRuleDeleted => "observability.alert_rule.deleted",
    observability::v1::AlertRuleRefused => "observability.alert_rule.refused",
    observability::v1::AlertChannelCreated => "observability.alert_channel.created",
    observability::v1::AlertChannelRotated => "observability.alert_channel.rotated",
    observability::v1::AlertChannelDeleted => "observability.alert_channel.deleted",
    observability::v1::AlertChannelRefused => "observability.alert_channel.refused",
    observability::v1::AlertFired => "observability.alert.fired",
    observability::v1::AlertResolved => "observability.alert.resolved",
    host::v1::GatewayServiceStarted => "host.gateway_service.started",
    host::v1::GatewayServiceStopped => "host.gateway_service.stopped",
    host::v1::GatewayServiceRestarted => "host.gateway_service.restarted",
    host::v1::OperationRefused => "host.operation.refused",
    files::v1::FileWritten => "files.file.written",
    files::v1::DirectoryCreated => "files.directory.created",
    files::v1::EntryRemoved => "files.entry.removed",
    files::v1::OperationRefused => "files.operation.refused",
    containers::v1::EngineEnabled => "container.engine.enabled",
    containers::v1::EngineDisabled => "container.engine.disabled",
    containers::v1::ContainerStarted => "container.started",
    containers::v1::ContainerStopped => "container.stopped",
    containers::v1::ContainerRestarted => "container.restarted",
    containers::v1::ContainerKilled => "container.killed",
    containers::v1::ContainerRemoved => "container.removed",
    containers::v1::ImageRemoved => "container.image.removed",
    containers::v1::ImagePulled => "container.image.pulled",
    containers::v1::EnginePruned => "container.engine.pruned",
    containers::v1::ComposeUp => "container.compose.up",
    containers::v1::ComposeDown => "container.compose.down",
    containers::v1::ComposeRestarted => "container.compose.restarted",
    containers::v1::OperationRefused => "container.operation.refused",
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_events::{EventPayload, EventType};
    use std::collections::BTreeSet;

    #[test]
    fn every_event_type_is_named_once_and_validly() {
        let unique: BTreeSet<&str> = EVENT_TYPES.iter().copied().collect();
        assert_eq!(unique.len(), EVENT_TYPES.len());
        for name in EVENT_TYPES {
            assert!(EventType::new(*name).is_ok(), "{name}");
        }
    }

    #[test]
    fn data_is_json_with_proto_field_names_defaults_and_a_type_url() {
        let ended = identity::v1::SessionEnded {
            account: "a".into(),
            reason: "revoked".into(),
            sessions: Some(2),
            ..Default::default()
        };
        let payload = EventPayload::of(&ended).unwrap();
        assert_eq!(
            payload.schema(),
            Some("https://type.googleapis.com/pingora.panel.events.identity.v1.SessionEnded")
        );
        assert_eq!(
            payload.decode_json::<serde_json::Value>().unwrap(),
            serde_json::json!({"account": "a", "reason": "revoked", "sessions": 2})
        );
        let updated = identity::v1::AccountUpdated {
            account: "a".into(),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(&updated).unwrap(),
            serde_json::json!({"account": "a", "unlocked": false}),
            "unset optional fields are left out, others are written"
        );
    }
}
