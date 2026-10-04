#![forbid(unsafe_code)]

//! Protobuf representation of the configuration publication contract,
//! shared by its server and client so both map values identically.

use gateway_proto_codec::{decode_hash, encode_hash};
use panel_application::{
    AbortOutcome, ActivatedDeployment, ConfigDocument, DeploymentOutcome, GatewayStatus,
    IdempotencyLookup, IdempotencyRecord, PreparedDeployment,
};
use panel_config_api::{ApplyOutcome, ConfigurationCommand, ConfigurationQuery, DraftInfo};
use panel_contracts::{
    common::v1 as common,
    config::v1::{self as wire, activation_receipt::Outcome},
};
use panel_domain::RevisionId;
use panel_errors::{PanelError, Result, ValidationReport};
use std::time::SystemTime;

pub fn encode_document(document: &ConfigDocument) -> wire::ConfigDocument {
    wire::ConfigDocument {
        schema_version: document.schema_version().into(),
        media_type: document.media_type().into(),
        content: document.body().to_vec(),
    }
}

pub fn decode_document(value: Option<wire::ConfigDocument>) -> Result<ConfigDocument> {
    let value =
        value.ok_or_else(|| PanelError::invalid_argument("configuration document is required"))?;
    ConfigDocument::new(value.schema_version, value.media_type, value.content)
}

pub fn encode_report(report: &ValidationReport) -> wire::ValidationReport {
    wire::ValidationReport {
        valid: report.valid,
        diagnostics: report.diagnostics.iter().map(Into::into).collect(),
    }
}

pub fn decode_report(value: Option<wire::ValidationReport>) -> Result<ValidationReport> {
    let value =
        value.ok_or_else(|| PanelError::invalid_argument("validation report is required"))?;
    Ok(ValidationReport {
        valid: value.valid,
        diagnostics: value.diagnostics.into_iter().map(Into::into).collect(),
    })
}

pub fn encode_draft(draft: &DraftInfo) -> wire::Draft {
    wire::Draft {
        version: draft.version,
        updated_at: draft.updated_at.map(Into::into),
        applied_version: draft.applied_version,
        applied_at: draft.applied_at.map(Into::into),
    }
}

pub fn decode_draft(value: Option<wire::Draft>) -> Result<DraftInfo> {
    let value =
        value.ok_or_else(|| PanelError::internal("the configuration service sent no draft"))?;
    let time = |value: Option<prost_types::Timestamp>| {
        value.and_then(|value| SystemTime::try_from(value).ok())
    };
    Ok(DraftInfo {
        version: value.version,
        updated_at: time(value.updated_at),
        applied_version: value.applied_version,
        applied_at: time(value.applied_at),
    })
}

pub fn encode_query(query: &ConfigurationQuery) -> Vec<u8> {
    serde_json::to_vec(query).expect("configuration queries serialize")
}

pub fn decode_query(value: &[u8]) -> Result<ConfigurationQuery> {
    serde_json::from_slice(value).map_err(|error| {
        PanelError::invalid_argument(format!("the configuration query cannot be read: {error}"))
    })
}

pub fn encode_change(command: &ConfigurationCommand) -> Vec<u8> {
    serde_json::to_vec(command).expect("configuration commands serialize")
}

pub fn decode_change(value: &[u8]) -> Result<ConfigurationCommand> {
    serde_json::from_slice(value).map_err(|error| {
        PanelError::invalid_argument(format!("the configuration command cannot be read: {error}"))
    })
}

/// How an apply ended, as the response to it.
pub fn encode_apply_outcome(outcome: &ApplyOutcome) -> wire::ApplyResponse {
    match outcome {
        ApplyOutcome::Applied {
            draft,
            deployment,
            revision,
            ..
        } => wire::ApplyResponse {
            draft: Some(encode_draft(draft)),
            deployment: Some(encode_activated(deployment)),
            revision: *revision,
            ..wire::ApplyResponse::default()
        },
        ApplyOutcome::Rejected {
            draft,
            report,
            revision,
            ..
        } => wire::ApplyResponse {
            draft: Some(encode_draft(draft)),
            report: Some(encode_report(report)),
            revision: revision.unwrap_or_default(),
            ..wire::ApplyResponse::default()
        },
        ApplyOutcome::Checked { draft, report, .. } => wire::ApplyResponse {
            draft: Some(encode_draft(draft)),
            report: Some(encode_report(report)),
            ..wire::ApplyResponse::default()
        },
        ApplyOutcome::AwaitingApproval { draft, request, .. } => wire::ApplyResponse {
            draft: Some(encode_draft(draft)),
            approval: serde_json::to_vec(request).expect("approval requests serialize"),
            ..wire::ApplyResponse::default()
        },
        _ => wire::ApplyResponse {
            error: Some(
                (&PanelError::internal("the apply ended in a way this protocol cannot carry"))
                    .into(),
            ),
            ..wire::ApplyResponse::default()
        },
    }
}

pub fn decode_apply_outcome(response: wire::ApplyResponse) -> Result<ApplyOutcome> {
    decode_error(response.error)?;
    let draft = decode_draft(response.draft)?;
    if !response.approval.is_empty() {
        let request = serde_json::from_slice(&response.approval).map_err(|error| {
            PanelError::internal(format!("the approval request cannot be read: {error}"))
        })?;
        return Ok(ApplyOutcome::awaiting_approval(draft, request));
    }
    let revision = (response.revision != 0).then_some(response.revision);
    Ok(match (response.deployment, revision) {
        (Some(deployment), Some(revision)) => {
            ApplyOutcome::applied(draft, decode_activated(Some(deployment))?, revision)
        }
        (Some(_), None) => {
            return Err(PanelError::internal(
                "the configuration service applied a draft without recording a revision",
            ))
        }
        (None, _) => {
            let report = decode_report(response.report)?;
            if report.valid {
                ApplyOutcome::checked(draft, report)
            } else {
                ApplyOutcome::rejected(draft, report, revision)
            }
        }
    })
}

pub fn encode_prepared(value: &PreparedDeployment) -> wire::PreparedDeployment {
    wire::PreparedDeployment {
        revision_id: value.revision_id().get(),
        content_hash: Some(encode_hash(value.content_hash())),
        prepare_token: value.prepare_token().into(),
    }
}

pub fn decode_prepared(value: Option<wire::PreparedDeployment>) -> Result<PreparedDeployment> {
    let value =
        value.ok_or_else(|| PanelError::invalid_argument("prepared deployment is required"))?;
    PreparedDeployment::new(
        RevisionId::new(value.revision_id),
        decode_hash(value.content_hash)?,
        value.prepare_token,
    )
}

pub fn encode_activated(value: &ActivatedDeployment) -> wire::ActivatedDeployment {
    wire::ActivatedDeployment {
        revision_id: value.revision_id().get(),
        content_hash: Some(encode_hash(value.content_hash())),
        previous_active_hash: value.previous_active_hash().map(encode_hash),
    }
}

pub fn decode_activated(value: Option<wire::ActivatedDeployment>) -> Result<ActivatedDeployment> {
    let value =
        value.ok_or_else(|| PanelError::invalid_argument("activated deployment is required"))?;
    Ok(ActivatedDeployment::new(
        RevisionId::new(value.revision_id),
        decode_hash(value.content_hash)?,
        value
            .previous_active_hash
            .map(|hash| decode_hash(Some(hash)))
            .transpose()?,
    ))
}

pub fn encode_abort(value: AbortOutcome) -> bool {
    value.aborted()
}

pub fn decode_abort(aborted: bool) -> AbortOutcome {
    AbortOutcome::new(aborted)
}

pub fn encode_status(value: &GatewayStatus) -> wire::GatewayStatus {
    wire::GatewayStatus {
        ready: value.ready(),
        message: value.message().unwrap_or_default().into(),
        active_revision_id: value.active_revision_id().map_or(0, RevisionId::get),
        active_hash: value.active_hash().map(encode_hash),
        prepared_count: u64::try_from(value.prepared_count()).unwrap_or(u64::MAX),
        adapter_version: value.adapter_version().into(),
        schema_version: value.schema_version().into(),
    }
}

pub fn decode_status(value: Option<wire::GatewayStatus>) -> Result<GatewayStatus> {
    let value = value.ok_or_else(|| PanelError::invalid_argument("gateway status is required"))?;
    Ok(GatewayStatus::new(
        value.ready,
        (!value.message.is_empty()).then_some(value.message),
        (value.active_revision_id != 0).then(|| RevisionId::new(value.active_revision_id)),
        value
            .active_hash
            .map(|hash| decode_hash(Some(hash)))
            .transpose()?,
        usize::try_from(value.prepared_count).unwrap_or(usize::MAX),
        value.adapter_version,
        value.schema_version,
    ))
}

pub fn encode_lookup(
    value: &IdempotencyLookup,
) -> Result<(
    wire::get_activation_receipt_response::State,
    Option<wire::ActivationReceipt>,
)> {
    use wire::get_activation_receipt_response::State;
    match value {
        IdempotencyLookup::Completed(record) => {
            Ok((State::Completed, Some(encode_receipt(record)?)))
        }
        IdempotencyLookup::InProgress => Ok((State::InProgress, None)),
        IdempotencyLookup::Missing => Ok((State::Missing, None)),
        _ => Err(unsupported("receipt state")),
    }
}

pub fn decode_lookup(
    state: i32,
    receipt: Option<wire::ActivationReceipt>,
) -> Result<IdempotencyLookup> {
    use wire::get_activation_receipt_response::State;
    match State::try_from(state) {
        Ok(State::Missing) => Ok(IdempotencyLookup::Missing),
        Ok(State::InProgress) => Ok(IdempotencyLookup::InProgress),
        Ok(State::Completed) => Ok(IdempotencyLookup::Completed(decode_receipt(
            receipt.ok_or_else(|| PanelError::invalid_argument("completed receipt is missing"))?,
        )?)),
        _ => Err(PanelError::invalid_argument("unknown receipt state")),
    }
}

/// Fails for outcomes this contract revision cannot represent rather than
/// reporting them as another outcome.
pub fn encode_receipt(record: &IdempotencyRecord) -> Result<wire::ActivationReceipt> {
    let outcome = match record.outcome() {
        DeploymentOutcome::Succeeded(deployment) => {
            Outcome::Succeeded(encode_activated(deployment))
        }
        DeploymentOutcome::Rejected(report) => Outcome::Rejected(encode_report(report)),
        DeploymentOutcome::FailedBeforeCommit => {
            Outcome::FailedBeforeCommit(wire::FailedBeforeCommit {})
        }
        DeploymentOutcome::PendingReconciliation => {
            Outcome::PendingReconciliation(wire::PendingReconciliation {})
        }
        _ => return Err(unsupported("deployment outcome")),
    };
    Ok(wire::ActivationReceipt {
        request_hash: Some(encode_hash(record.request_hash())),
        outcome: Some(outcome),
    })
}

fn unsupported(what: &str) -> PanelError {
    PanelError::unsupported_capability(format!(
        "this {what} has no representation in pingora.panel.config.v1"
    ))
}

pub fn decode_receipt(value: wire::ActivationReceipt) -> Result<IdempotencyRecord> {
    let outcome = match value
        .outcome
        .ok_or_else(|| PanelError::invalid_argument("receipt outcome is required"))?
    {
        Outcome::Succeeded(deployment) => {
            DeploymentOutcome::Succeeded(decode_activated(Some(deployment))?)
        }
        Outcome::Rejected(report) => DeploymentOutcome::Rejected(decode_report(Some(report))?),
        Outcome::FailedBeforeCommit(_) => DeploymentOutcome::FailedBeforeCommit,
        Outcome::PendingReconciliation(_) => DeploymentOutcome::PendingReconciliation,
    };
    Ok(IdempotencyRecord::new(
        decode_hash(value.request_hash)?,
        outcome,
    ))
}

/// The error a response carries, if any.
pub fn decode_error(value: Option<common::Error>) -> Result<()> {
    value.map_or(Ok(()), |error| Err(error.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_application::ContentHash;
    use panel_errors::Diagnostic;

    fn hash(seed: &[u8]) -> ContentHash {
        ContentHash::from_bytes(seed)
    }

    #[test]
    fn values_round_trip() {
        let document = ConfigDocument::new("v1", "application/json", b"{}".to_vec()).unwrap();
        assert_eq!(
            decode_document(Some(encode_document(&document))).unwrap(),
            document
        );

        let report = ValidationReport::from_diagnostics(vec![Diagnostic::error("BAD", "bad")]);
        assert_eq!(decode_report(Some(encode_report(&report))).unwrap(), report);

        let prepared = PreparedDeployment::new(RevisionId::new(3), hash(b"a"), "token").unwrap();
        assert_eq!(
            decode_prepared(Some(encode_prepared(&prepared))).unwrap(),
            prepared
        );

        let activated = ActivatedDeployment::new(RevisionId::new(3), hash(b"a"), Some(hash(b"b")));
        assert_eq!(
            decode_activated(Some(encode_activated(&activated))).unwrap(),
            activated
        );

        for status in [
            GatewayStatus::new(
                true,
                None,
                Some(RevisionId::new(3)),
                Some(hash(b"a")),
                2,
                "pingora-v1",
                "v1",
            ),
            GatewayStatus::new(
                false,
                Some("starting".into()),
                None,
                None,
                0,
                "pingora-v1",
                "v1",
            ),
        ] {
            assert_eq!(decode_status(Some(encode_status(&status))).unwrap(), status);
        }

        for outcome in [
            DeploymentOutcome::Succeeded(activated),
            DeploymentOutcome::Rejected(report),
            DeploymentOutcome::FailedBeforeCommit,
            DeploymentOutcome::PendingReconciliation,
        ] {
            let lookup = IdempotencyLookup::Completed(IdempotencyRecord::new(hash(b"r"), outcome));
            let (state, receipt) = encode_lookup(&lookup).unwrap();
            assert_eq!(decode_lookup(state as i32, receipt).unwrap(), lookup);
        }
        for lookup in [IdempotencyLookup::Missing, IdempotencyLookup::InProgress] {
            let (state, receipt) = encode_lookup(&lookup).unwrap();
            assert_eq!(decode_lookup(state as i32, receipt).unwrap(), lookup);
        }
    }

    #[test]
    fn incomplete_messages_are_rejected() {
        assert!(decode_document(None).is_err());
        assert!(decode_prepared(None).is_err());
        assert!(decode_lookup(0, None).is_err());
        assert!(decode_lookup(3, None).is_err());
        assert!(decode_error(Some(common::Error {
            code: "CONFLICT".into(),
            ..Default::default()
        }))
        .is_err());
    }
}
