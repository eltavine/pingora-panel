use crate::{
    limits::{enforce_document_limit, DocumentLimitExceeded},
    wire::{RevisionDocumentHeader, WireRevisionStatusV1, WireRevisionTransitionV1},
    RevisionCodecLimits,
};
use core::fmt;
use panel_config_domain::{
    RevisionState, RevisionStatus, RevisionTransition, RevisionTransitionOutcome,
};
use serde::{Deserialize, Serialize};
use std::error::Error;

/// Stable identity carried by every revision-transition outcome document.
pub const REVISION_TRANSITION_OUTCOME_SCHEMA: &str =
    "io.github.eltavine.pingora-panel/revision-transition-outcome";

/// Latest revision-transition outcome document version emitted by this codec.
pub const REVISION_TRANSITION_OUTCOME_VERSION: u64 = 1;

/// Encodes one validated transition outcome into the latest wire representation.
pub fn encode_revision_transition_outcome(
    outcome: RevisionTransitionOutcome,
) -> Result<Vec<u8>, RevisionTransitionOutcomeEncodeError> {
    let document = RevisionTransitionOutcomeDocumentV1 {
        schema: REVISION_TRANSITION_OUTCOME_SCHEMA.to_owned(),
        version: REVISION_TRANSITION_OUTCOME_VERSION,
        from: WireRevisionStatusV1::try_from(outcome.from_status())
            .map_err(RevisionTransitionOutcomeEncodeError::for_unsupported_status)?,
        transition: WireRevisionTransitionV1::try_from(outcome.applied_transition())
            .map_err(RevisionTransitionOutcomeEncodeError::for_unsupported_transition)?,
        to: WireRevisionStatusV1::try_from(outcome.to_status())
            .map_err(RevisionTransitionOutcomeEncodeError::for_unsupported_status)?,
    };
    let mut encoded = serde_json::to_vec(&document)
        .map_err(RevisionTransitionOutcomeEncodeError::serialization_failed)?;
    encoded.push(b'\n');
    Ok(encoded)
}

/// Decodes a transition outcome with the default resource policy.
pub fn decode_revision_transition_outcome(
    encoded: &[u8],
) -> Result<RevisionTransitionOutcome, RevisionTransitionOutcomeDecodeError> {
    decode_revision_transition_outcome_with_limits(encoded, RevisionCodecLimits::default())
}

/// Decodes a transition outcome with an explicitly injected resource policy.
pub fn decode_revision_transition_outcome_with_limits(
    encoded: &[u8],
    limits: RevisionCodecLimits,
) -> Result<RevisionTransitionOutcome, RevisionTransitionOutcomeDecodeError> {
    enforce_document_limit(encoded, limits)
        .map_err(RevisionTransitionOutcomeDecodeError::resource_limit_exceeded)?;
    let header: RevisionDocumentHeader = serde_json::from_slice(encoded)
        .map_err(RevisionTransitionOutcomeDecodeError::malformed_document)?;
    if header.schema != REVISION_TRANSITION_OUTCOME_SCHEMA {
        return Err(RevisionTransitionOutcomeDecodeError::for_unsupported_schema(header.schema));
    }
    if header.version != REVISION_TRANSITION_OUTCOME_VERSION {
        return Err(RevisionTransitionOutcomeDecodeError::for_unsupported_version(header.version));
    }

    let document: RevisionTransitionOutcomeDocumentV1 = serde_json::from_slice(encoded)
        .map_err(RevisionTransitionOutcomeDecodeError::malformed_document)?;
    let from = RevisionStatus::from(document.from);
    let transition = RevisionTransition::from(document.transition);
    let declared_to = RevisionStatus::from(document.to);
    let outcome = RevisionState::rehydrate(from)
        .transition_with_outcome(transition)
        .map_err(|_| {
            RevisionTransitionOutcomeDecodeError::invalid_transition(
                from,
                transition,
                declared_to,
                None,
            )
        })?;
    if outcome.to_status() != declared_to {
        return Err(RevisionTransitionOutcomeDecodeError::invalid_transition(
            from,
            transition,
            declared_to,
            Some(outcome.to_status()),
        ));
    }
    Ok(outcome)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RevisionTransitionOutcomeDocumentV1 {
    schema: String,
    version: u64,
    from: WireRevisionStatusV1,
    transition: WireRevisionTransitionV1,
    to: WireRevisionStatusV1,
}

/// Stable classification for a transition-outcome encoding failure.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum RevisionTransitionOutcomeEncodeErrorKind {
    /// A domain status cannot be represented by this codec version.
    UnsupportedStatus,
    /// A domain transition cannot be represented by this codec version.
    UnsupportedTransition,
    /// Serialization failed before a complete document was produced.
    SerializationFailed,
}

/// Failure while encoding a transition outcome.
#[derive(Debug)]
pub struct RevisionTransitionOutcomeEncodeError {
    inner: RevisionTransitionOutcomeEncodeErrorInner,
}

#[derive(Debug)]
enum RevisionTransitionOutcomeEncodeErrorInner {
    UnsupportedStatus(RevisionStatus),
    UnsupportedTransition(RevisionTransition),
    SerializationFailed(serde_json::Error),
}

impl RevisionTransitionOutcomeEncodeError {
    const fn for_unsupported_status(status: RevisionStatus) -> Self {
        Self {
            inner: RevisionTransitionOutcomeEncodeErrorInner::UnsupportedStatus(status),
        }
    }

    const fn for_unsupported_transition(transition: RevisionTransition) -> Self {
        Self {
            inner: RevisionTransitionOutcomeEncodeErrorInner::UnsupportedTransition(transition),
        }
    }

    fn serialization_failed(error: serde_json::Error) -> Self {
        Self {
            inner: RevisionTransitionOutcomeEncodeErrorInner::SerializationFailed(error),
        }
    }

    /// Returns the stable category of this failure.
    pub const fn kind(&self) -> RevisionTransitionOutcomeEncodeErrorKind {
        match &self.inner {
            RevisionTransitionOutcomeEncodeErrorInner::UnsupportedStatus(_) => {
                RevisionTransitionOutcomeEncodeErrorKind::UnsupportedStatus
            }
            RevisionTransitionOutcomeEncodeErrorInner::UnsupportedTransition(_) => {
                RevisionTransitionOutcomeEncodeErrorKind::UnsupportedTransition
            }
            RevisionTransitionOutcomeEncodeErrorInner::SerializationFailed(_) => {
                RevisionTransitionOutcomeEncodeErrorKind::SerializationFailed
            }
        }
    }

    /// Returns the unsupported status, when present.
    pub const fn unsupported_status(&self) -> Option<RevisionStatus> {
        match &self.inner {
            RevisionTransitionOutcomeEncodeErrorInner::UnsupportedStatus(status) => Some(*status),
            RevisionTransitionOutcomeEncodeErrorInner::UnsupportedTransition(_)
            | RevisionTransitionOutcomeEncodeErrorInner::SerializationFailed(_) => None,
        }
    }

    /// Returns the unsupported transition, when present.
    pub const fn unsupported_transition(&self) -> Option<RevisionTransition> {
        match &self.inner {
            RevisionTransitionOutcomeEncodeErrorInner::UnsupportedTransition(transition) => {
                Some(*transition)
            }
            RevisionTransitionOutcomeEncodeErrorInner::UnsupportedStatus(_)
            | RevisionTransitionOutcomeEncodeErrorInner::SerializationFailed(_) => None,
        }
    }
}

impl fmt::Display for RevisionTransitionOutcomeEncodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.inner {
            RevisionTransitionOutcomeEncodeErrorInner::UnsupportedStatus(status) => {
                write!(
                    formatter,
                    "revision status {status:?} has no outcome encoding"
                )
            }
            RevisionTransitionOutcomeEncodeErrorInner::UnsupportedTransition(transition) => {
                write!(
                    formatter,
                    "revision transition {transition:?} has no outcome encoding"
                )
            }
            RevisionTransitionOutcomeEncodeErrorInner::SerializationFailed(error) => {
                write!(
                    formatter,
                    "revision transition serialization failed: {error}"
                )
            }
        }
    }
}

impl Error for RevisionTransitionOutcomeEncodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.inner {
            RevisionTransitionOutcomeEncodeErrorInner::SerializationFailed(error) => Some(error),
            RevisionTransitionOutcomeEncodeErrorInner::UnsupportedStatus(_)
            | RevisionTransitionOutcomeEncodeErrorInner::UnsupportedTransition(_) => None,
        }
    }
}

/// Stable classification for a transition-outcome decoding failure.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum RevisionTransitionOutcomeDecodeErrorKind {
    /// The encoded document exceeds its injected resource policy.
    ResourceLimitExceeded,
    /// The input is not a structurally valid supported document.
    MalformedDocument,
    /// The input identifies a different wire schema.
    UnsupportedSchema,
    /// The input uses a version unknown to this codec.
    UnsupportedVersion,
    /// The declared before/event/after tuple violates the domain state machine.
    InvalidTransition,
}

/// Failure while decoding a transition-outcome document.
#[derive(Debug)]
pub struct RevisionTransitionOutcomeDecodeError {
    inner: RevisionTransitionOutcomeDecodeErrorInner,
}

#[derive(Debug)]
enum RevisionTransitionOutcomeDecodeErrorInner {
    ResourceLimitExceeded(DocumentLimitExceeded),
    MalformedDocument(serde_json::Error),
    UnsupportedSchema(String),
    UnsupportedVersion(u64),
    InvalidTransition {
        from: RevisionStatus,
        transition: RevisionTransition,
        declared_to: RevisionStatus,
        expected_to: Option<RevisionStatus>,
    },
}

impl RevisionTransitionOutcomeDecodeError {
    fn resource_limit_exceeded(error: DocumentLimitExceeded) -> Self {
        Self {
            inner: RevisionTransitionOutcomeDecodeErrorInner::ResourceLimitExceeded(error),
        }
    }

    fn malformed_document(error: serde_json::Error) -> Self {
        Self {
            inner: RevisionTransitionOutcomeDecodeErrorInner::MalformedDocument(error),
        }
    }

    fn for_unsupported_schema(schema: String) -> Self {
        Self {
            inner: RevisionTransitionOutcomeDecodeErrorInner::UnsupportedSchema(schema),
        }
    }

    const fn for_unsupported_version(version: u64) -> Self {
        Self {
            inner: RevisionTransitionOutcomeDecodeErrorInner::UnsupportedVersion(version),
        }
    }

    const fn invalid_transition(
        from: RevisionStatus,
        transition: RevisionTransition,
        declared_to: RevisionStatus,
        expected_to: Option<RevisionStatus>,
    ) -> Self {
        Self {
            inner: RevisionTransitionOutcomeDecodeErrorInner::InvalidTransition {
                from,
                transition,
                declared_to,
                expected_to,
            },
        }
    }

    /// Returns the stable category of this failure.
    pub const fn kind(&self) -> RevisionTransitionOutcomeDecodeErrorKind {
        match &self.inner {
            RevisionTransitionOutcomeDecodeErrorInner::ResourceLimitExceeded(_) => {
                RevisionTransitionOutcomeDecodeErrorKind::ResourceLimitExceeded
            }
            RevisionTransitionOutcomeDecodeErrorInner::MalformedDocument(_) => {
                RevisionTransitionOutcomeDecodeErrorKind::MalformedDocument
            }
            RevisionTransitionOutcomeDecodeErrorInner::UnsupportedSchema(_) => {
                RevisionTransitionOutcomeDecodeErrorKind::UnsupportedSchema
            }
            RevisionTransitionOutcomeDecodeErrorInner::UnsupportedVersion(_) => {
                RevisionTransitionOutcomeDecodeErrorKind::UnsupportedVersion
            }
            RevisionTransitionOutcomeDecodeErrorInner::InvalidTransition { .. } => {
                RevisionTransitionOutcomeDecodeErrorKind::InvalidTransition
            }
        }
    }

    /// Returns the unsupported schema identity, when present.
    pub fn unsupported_schema(&self) -> Option<&str> {
        match &self.inner {
            RevisionTransitionOutcomeDecodeErrorInner::UnsupportedSchema(schema) => Some(schema),
            _ => None,
        }
    }

    /// Returns the unsupported version, when present.
    pub const fn unsupported_version(&self) -> Option<u64> {
        match &self.inner {
            RevisionTransitionOutcomeDecodeErrorInner::UnsupportedVersion(version) => {
                Some(*version)
            }
            _ => None,
        }
    }

    /// Returns `(actual_bytes, max_bytes)` for a resource-limit failure.
    pub const fn document_size_limit(&self) -> Option<(usize, usize)> {
        match &self.inner {
            RevisionTransitionOutcomeDecodeErrorInner::ResourceLimitExceeded(error) => {
                Some((error.actual_bytes, error.max_bytes))
            }
            _ => None,
        }
    }

    /// Returns the declared source status for an invalid transition tuple.
    pub const fn from_status(&self) -> Option<RevisionStatus> {
        match &self.inner {
            RevisionTransitionOutcomeDecodeErrorInner::InvalidTransition { from, .. } => {
                Some(*from)
            }
            _ => None,
        }
    }

    /// Returns the declared transition for an invalid transition tuple.
    pub const fn transition(&self) -> Option<RevisionTransition> {
        match &self.inner {
            RevisionTransitionOutcomeDecodeErrorInner::InvalidTransition { transition, .. } => {
                Some(*transition)
            }
            _ => None,
        }
    }

    /// Returns the declared target status for an invalid transition tuple.
    pub const fn declared_to_status(&self) -> Option<RevisionStatus> {
        match &self.inner {
            RevisionTransitionOutcomeDecodeErrorInner::InvalidTransition {
                declared_to, ..
            } => Some(*declared_to),
            _ => None,
        }
    }

    /// Returns the legal target, when the transition itself was legal.
    pub const fn expected_to_status(&self) -> Option<RevisionStatus> {
        match &self.inner {
            RevisionTransitionOutcomeDecodeErrorInner::InvalidTransition {
                expected_to, ..
            } => *expected_to,
            _ => None,
        }
    }
}

impl fmt::Display for RevisionTransitionOutcomeDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.inner {
            RevisionTransitionOutcomeDecodeErrorInner::ResourceLimitExceeded(error) => {
                error.fmt(formatter)
            }
            RevisionTransitionOutcomeDecodeErrorInner::MalformedDocument(error) => {
                write!(formatter, "malformed revision transition document: {error}")
            }
            RevisionTransitionOutcomeDecodeErrorInner::UnsupportedSchema(schema) => {
                write!(formatter, "unsupported revision transition schema: {schema:?}")
            }
            RevisionTransitionOutcomeDecodeErrorInner::UnsupportedVersion(version) => {
                write!(formatter, "unsupported revision transition version: {version}")
            }
            RevisionTransitionOutcomeDecodeErrorInner::InvalidTransition {
                from,
                transition,
                declared_to,
                expected_to,
            } => write!(
                formatter,
                "invalid revision transition tuple: {from:?} + {transition:?} -> {declared_to:?} (expected {expected_to:?})"
            ),
        }
    }
}

impl Error for RevisionTransitionOutcomeDecodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.inner {
            RevisionTransitionOutcomeDecodeErrorInner::MalformedDocument(error) => Some(error),
            _ => None,
        }
    }
}
