#![forbid(unsafe_code)]

use panel_config_domain::{RevisionState, RevisionStatus, RevisionTransition};
use panel_config_state_codec::{
    decode_revision_state, decode_revision_state_with_limits, decode_revision_transition_outcome,
    decode_revision_transition_outcome_with_limits, encode_revision_state,
    encode_revision_transition_outcome, RevisionCodecLimits, RevisionStateDecodeErrorKind,
    RevisionTransitionOutcomeDecodeErrorKind,
};
use std::num::NonZeroUsize;

const DRAFT_V1: &[u8] = include_bytes!("fixtures/v1/draft.json");
const TRANSITION_OUTCOMES_V1: &[u8] = include_bytes!("fixtures/v1/transition_outcomes.jsonl");

const STATUS_FIXTURES_V1: &[(RevisionStatus, &[u8])] = &[
    (
        RevisionStatus::Draft,
        include_bytes!("fixtures/v1/draft.json"),
    ),
    (
        RevisionStatus::Validated,
        include_bytes!("fixtures/v1/validated.json"),
    ),
    (
        RevisionStatus::AwaitingApproval,
        include_bytes!("fixtures/v1/awaiting_approval.json"),
    ),
    (
        RevisionStatus::Approved,
        include_bytes!("fixtures/v1/approved.json"),
    ),
    (
        RevisionStatus::Preparing,
        include_bytes!("fixtures/v1/preparing.json"),
    ),
    (
        RevisionStatus::Prepared,
        include_bytes!("fixtures/v1/prepared.json"),
    ),
    (
        RevisionStatus::Activating,
        include_bytes!("fixtures/v1/activating.json"),
    ),
    (
        RevisionStatus::Reconciling,
        include_bytes!("fixtures/v1/reconciling.json"),
    ),
    (
        RevisionStatus::Active,
        include_bytes!("fixtures/v1/active.json"),
    ),
    (
        RevisionStatus::Superseded,
        include_bytes!("fixtures/v1/superseded.json"),
    ),
    (
        RevisionStatus::Rejected,
        include_bytes!("fixtures/v1/rejected.json"),
    ),
    (
        RevisionStatus::Failed,
        include_bytes!("fixtures/v1/failed.json"),
    ),
];

const TRANSITION_CASES_V1: &[(RevisionStatus, RevisionTransition, RevisionStatus)] = &[
    (
        RevisionStatus::Draft,
        RevisionTransition::ValidationSucceeded,
        RevisionStatus::Validated,
    ),
    (
        RevisionStatus::Draft,
        RevisionTransition::ValidationFailed,
        RevisionStatus::Failed,
    ),
    (
        RevisionStatus::Validated,
        RevisionTransition::ApprovalRequired,
        RevisionStatus::AwaitingApproval,
    ),
    (
        RevisionStatus::Validated,
        RevisionTransition::ApprovalNotRequired,
        RevisionStatus::Approved,
    ),
    (
        RevisionStatus::AwaitingApproval,
        RevisionTransition::ApprovalGranted,
        RevisionStatus::Approved,
    ),
    (
        RevisionStatus::AwaitingApproval,
        RevisionTransition::ApprovalRejected,
        RevisionStatus::Rejected,
    ),
    (
        RevisionStatus::AwaitingApproval,
        RevisionTransition::ApprovalExpired,
        RevisionStatus::Rejected,
    ),
    (
        RevisionStatus::Approved,
        RevisionTransition::PreparationStarted,
        RevisionStatus::Preparing,
    ),
    (
        RevisionStatus::Preparing,
        RevisionTransition::PreparationSucceeded,
        RevisionStatus::Prepared,
    ),
    (
        RevisionStatus::Preparing,
        RevisionTransition::PreparationFailed,
        RevisionStatus::Failed,
    ),
    (
        RevisionStatus::Prepared,
        RevisionTransition::ActivationStarted,
        RevisionStatus::Activating,
    ),
    (
        RevisionStatus::Activating,
        RevisionTransition::ActivationSucceeded,
        RevisionStatus::Active,
    ),
    (
        RevisionStatus::Activating,
        RevisionTransition::ActivationOutcomeUnknown,
        RevisionStatus::Reconciling,
    ),
    (
        RevisionStatus::Reconciling,
        RevisionTransition::GatewayConfirmedRevision,
        RevisionStatus::Active,
    ),
    (
        RevisionStatus::Reconciling,
        RevisionTransition::GatewayConfirmedPreviousRevision,
        RevisionStatus::Failed,
    ),
    (
        RevisionStatus::Active,
        RevisionTransition::LaterRevisionActivated,
        RevisionStatus::Superseded,
    ),
];

#[test]
fn version_one_golden_documents_are_stable() {
    assert_eq!(STATUS_FIXTURES_V1.len(), RevisionStatus::KNOWN.len());
    for status in RevisionStatus::KNOWN.iter().copied() {
        assert_eq!(
            STATUS_FIXTURES_V1
                .iter()
                .filter(|(fixture_status, _)| *fixture_status == status)
                .count(),
            1,
            "{status:?} must have exactly one v1 fixture"
        );
    }

    for (status, fixture) in STATUS_FIXTURES_V1.iter().copied() {
        let state = RevisionState::rehydrate(status);
        assert_eq!(encode_revision_state(state).unwrap(), fixture);
        assert_eq!(decode_revision_state(fixture).unwrap(), state);
    }
}

#[test]
fn every_version_one_status_round_trips() {
    for status in RevisionStatus::KNOWN.iter().copied() {
        let state = RevisionState::rehydrate(status);
        assert_eq!(
            decode_revision_state(&encode_revision_state(state).unwrap()).unwrap(),
            state
        );
    }
}

#[test]
fn version_one_transition_outcomes_are_stable_and_domain_validated() {
    assert_eq!(TRANSITION_CASES_V1.len(), RevisionTransition::KNOWN.len());
    let mut encoded = Vec::new();
    for (from, transition, to) in TRANSITION_CASES_V1.iter().copied() {
        let outcome = RevisionState::rehydrate(from)
            .transition_with_outcome(transition)
            .unwrap();
        assert_eq!(outcome.to_status(), to);
        encoded.extend(encode_revision_transition_outcome(outcome).unwrap());
    }
    assert_eq!(encoded, TRANSITION_OUTCOMES_V1);

    for ((from, transition, to), document) in TRANSITION_CASES_V1
        .iter()
        .copied()
        .zip(TRANSITION_OUTCOMES_V1.split(|byte| *byte == b'\n'))
    {
        if document.is_empty() {
            continue;
        }
        let outcome = decode_revision_transition_outcome(document).unwrap();
        assert_eq!(outcome.from_status(), from);
        assert_eq!(outcome.applied_transition(), transition);
        assert_eq!(outcome.to_status(), to);
    }
}

#[test]
fn transition_decoder_rejects_illegal_and_inconsistent_tuples() {
    let illegal = decode_revision_transition_outcome(
        br#"{"schema":"io.github.eltavine.pingora-panel/revision-transition-outcome","version":1,"from":"draft","transition":"approval_granted","to":"approved"}"#,
    )
    .unwrap_err();
    assert_eq!(
        illegal.kind(),
        RevisionTransitionOutcomeDecodeErrorKind::InvalidTransition
    );
    assert_eq!(illegal.from_status(), Some(RevisionStatus::Draft));
    assert_eq!(
        illegal.transition(),
        Some(RevisionTransition::ApprovalGranted)
    );
    assert_eq!(illegal.expected_to_status(), None);

    let inconsistent = decode_revision_transition_outcome(
        br#"{"schema":"io.github.eltavine.pingora-panel/revision-transition-outcome","version":1,"from":"draft","transition":"validation_succeeded","to":"active"}"#,
    )
    .unwrap_err();
    assert_eq!(
        inconsistent.kind(),
        RevisionTransitionOutcomeDecodeErrorKind::InvalidTransition
    );
    assert_eq!(
        inconsistent.declared_to_status(),
        Some(RevisionStatus::Active)
    );
    assert_eq!(
        inconsistent.expected_to_status(),
        Some(RevisionStatus::Validated)
    );
}

#[test]
fn injected_document_limits_apply_before_json_interpretation() {
    let state_limit = RevisionCodecLimits::new(NonZeroUsize::new(DRAFT_V1.len() - 1).unwrap());
    let state_error = decode_revision_state_with_limits(DRAFT_V1, state_limit).unwrap_err();
    assert_eq!(
        state_error.kind(),
        RevisionStateDecodeErrorKind::ResourceLimitExceeded
    );
    assert_eq!(
        state_error.document_size_limit(),
        Some((DRAFT_V1.len(), DRAFT_V1.len() - 1))
    );

    let first_outcome = TRANSITION_OUTCOMES_V1
        .split(|byte| *byte == b'\n')
        .next()
        .unwrap();
    let outcome_limit =
        RevisionCodecLimits::new(NonZeroUsize::new(first_outcome.len() - 1).unwrap());
    let outcome_error =
        decode_revision_transition_outcome_with_limits(first_outcome, outcome_limit).unwrap_err();
    assert_eq!(
        outcome_error.kind(),
        RevisionTransitionOutcomeDecodeErrorKind::ResourceLimitExceeded
    );
    assert_eq!(
        outcome_error.document_size_limit(),
        Some((first_outcome.len(), first_outcome.len() - 1))
    );
}

#[test]
fn future_versions_fail_before_version_one_interpretation() {
    let error = decode_revision_state(
        br#"{"schema":"io.github.eltavine.pingora-panel/revision-state","version":2,"status":"draft"}"#,
    )
    .unwrap_err();
    assert_eq!(
        error.kind(),
        RevisionStateDecodeErrorKind::UnsupportedVersion
    );
    assert_eq!(error.unsupported_version(), Some(2));
}

#[test]
fn foreign_schemas_fail_before_version_one_interpretation() {
    let error = decode_revision_state(
        br#"{"schema":"example.invalid/revision-state","version":1,"status":"draft"}"#,
    )
    .unwrap_err();
    assert_eq!(
        error.kind(),
        RevisionStateDecodeErrorKind::UnsupportedSchema
    );
    assert_eq!(
        error.unsupported_schema(),
        Some("example.invalid/revision-state")
    );
}

#[test]
fn unsupported_schema_display_escapes_control_characters() {
    let error = decode_revision_state(
        br#"{"schema":"example.invalid/revision-state\nforged","version":1,"status":"draft"}"#,
    )
    .unwrap_err();

    assert_eq!(
        error.unsupported_schema(),
        Some("example.invalid/revision-state\nforged")
    );
    assert!(!error.to_string().contains('\n'));
    assert!(error.to_string().contains("\\nforged"));
}

#[test]
fn version_one_rejects_unknown_statuses_and_fields() {
    for document in [
        br#"{"schema":"io.github.eltavine.pingora-panel/revision-state","version":1,"status":"future"}"#.as_slice(),
        br#"{"schema":"io.github.eltavine.pingora-panel/revision-state","version":1,"status":"draft","extra":true}"#.as_slice(),
    ] {
        assert_eq!(
            decode_revision_state(document).unwrap_err().kind(),
            RevisionStateDecodeErrorKind::MalformedDocument
        );
    }
}
