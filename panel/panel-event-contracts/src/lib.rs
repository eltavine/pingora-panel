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
    config => "pingora.panel.events.config.v1",
    gateway => "pingora.panel.events.gateway.v1",
    identity => "pingora.panel.events.identity.v1",
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
    identity::v1::AccountCreated => "identity.account.created",
    identity::v1::AccountUpdated => "identity.account.updated",
    identity::v1::PasswordChanged => "identity.password.changed",
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
