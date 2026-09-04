#![forbid(unsafe_code)]
#![forbid(missing_docs)]

//! Versioned JSON adapter for configuration revision state.
//!
//! Wire-format concerns live here rather than in `panel-config-domain`, keeping
//! the domain model independent from serde and any persistence representation.

mod limits;
mod outcome;
mod wire;

pub use limits::{RevisionCodecLimits, DEFAULT_MAX_REVISION_DOCUMENT_BYTES};
pub use outcome::{
    decode_revision_transition_outcome, decode_revision_transition_outcome_with_limits,
    encode_revision_transition_outcome, RevisionTransitionOutcomeDecodeError,
    RevisionTransitionOutcomeDecodeErrorKind, RevisionTransitionOutcomeEncodeError,
    RevisionTransitionOutcomeEncodeErrorKind, REVISION_TRANSITION_OUTCOME_SCHEMA,
    REVISION_TRANSITION_OUTCOME_VERSION,
};

use crate::{
    limits::{enforce_document_limit, DocumentLimitExceeded},
    wire::{RevisionDocumentHeader, WireRevisionStatusV1},
};
use core::fmt;
use panel_config_domain::{RevisionState, RevisionStatus};
use serde::{Deserialize, Serialize};
use std::error::Error;

/// Stable identity carried by every revision-state document.
pub const REVISION_STATE_SCHEMA: &str = "io.github.eltavine.pingora-panel/revision-state";

/// Latest revision-state document version emitted by this codec.
pub const REVISION_STATE_VERSION: u64 = 1;

/// Encodes a domain state into the latest canonical wire representation.
///
/// The returned document always ends in one newline, making golden files and
/// command-line inspection deterministic.
pub fn encode_revision_state(state: RevisionState) -> Result<Vec<u8>, RevisionStateEncodeError> {
    let status = WireRevisionStatusV1::try_from(state.status())
        .map_err(RevisionStateEncodeError::for_unsupported_status)?;
    let document = RevisionStateDocumentV1 {
        schema: REVISION_STATE_SCHEMA.to_owned(),
        version: REVISION_STATE_VERSION,
        status,
    };
    let mut encoded =
        serde_json::to_vec(&document).map_err(RevisionStateEncodeError::serialization_failed)?;
    encoded.push(b'\n');
    Ok(encoded)
}

/// Decodes one supported wire representation into the domain model.
///
/// The schema and version are inspected before the version-specific document,
/// so future versions fail with an actionable compatibility error rather than
/// being accidentally interpreted as version 1.
pub fn decode_revision_state(encoded: &[u8]) -> Result<RevisionState, RevisionStateDecodeError> {
    decode_revision_state_with_limits(encoded, RevisionCodecLimits::default())
}

/// Decodes one revision-state document with an explicitly injected resource policy.
pub fn decode_revision_state_with_limits(
    encoded: &[u8],
    limits: RevisionCodecLimits,
) -> Result<RevisionState, RevisionStateDecodeError> {
    enforce_document_limit(encoded, limits)
        .map_err(RevisionStateDecodeError::resource_limit_exceeded)?;
    let header: RevisionDocumentHeader =
        serde_json::from_slice(encoded).map_err(RevisionStateDecodeError::malformed_document)?;
    if header.schema != REVISION_STATE_SCHEMA {
        return Err(RevisionStateDecodeError::for_unsupported_schema(
            header.schema,
        ));
    }
    if header.version != REVISION_STATE_VERSION {
        return Err(RevisionStateDecodeError::for_unsupported_version(
            header.version,
        ));
    }

    let document: RevisionStateDocumentV1 =
        serde_json::from_slice(encoded).map_err(RevisionStateDecodeError::malformed_document)?;
    Ok(RevisionState::rehydrate(document.status.into()))
}

/// Stable classification for a revision-state encoding failure.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum RevisionStateEncodeErrorKind {
    /// The domain introduced a state which this codec version cannot represent.
    UnsupportedStatus,
    /// Serialization failed before a complete canonical document was produced.
    SerializationFailed,
}

/// Failure while encoding a domain state.
///
/// Implementation details remain private so replacing the JSON implementation
/// does not force callers to change their error handling.
#[derive(Debug)]
pub struct RevisionStateEncodeError {
    inner: RevisionStateEncodeErrorInner,
}

#[derive(Debug)]
enum RevisionStateEncodeErrorInner {
    UnsupportedStatus(RevisionStatus),
    SerializationFailed(serde_json::Error),
}

impl RevisionStateEncodeError {
    const fn for_unsupported_status(status: RevisionStatus) -> Self {
        Self {
            inner: RevisionStateEncodeErrorInner::UnsupportedStatus(status),
        }
    }

    fn serialization_failed(error: serde_json::Error) -> Self {
        Self {
            inner: RevisionStateEncodeErrorInner::SerializationFailed(error),
        }
    }

    /// Returns the stable category of this failure.
    pub const fn kind(&self) -> RevisionStateEncodeErrorKind {
        match &self.inner {
            RevisionStateEncodeErrorInner::UnsupportedStatus(_) => {
                RevisionStateEncodeErrorKind::UnsupportedStatus
            }
            RevisionStateEncodeErrorInner::SerializationFailed(_) => {
                RevisionStateEncodeErrorKind::SerializationFailed
            }
        }
    }

    /// Returns the unsupported domain status, when that caused the failure.
    pub const fn unsupported_status(&self) -> Option<RevisionStatus> {
        match &self.inner {
            RevisionStateEncodeErrorInner::UnsupportedStatus(status) => Some(*status),
            RevisionStateEncodeErrorInner::SerializationFailed(_) => None,
        }
    }
}

impl fmt::Display for RevisionStateEncodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.inner {
            RevisionStateEncodeErrorInner::UnsupportedStatus(status) => {
                write!(
                    formatter,
                    "revision status {status:?} has no version 1 encoding"
                )
            }
            RevisionStateEncodeErrorInner::SerializationFailed(error) => {
                write!(formatter, "revision state serialization failed: {error}")
            }
        }
    }
}

impl Error for RevisionStateEncodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.inner {
            RevisionStateEncodeErrorInner::UnsupportedStatus(_) => None,
            RevisionStateEncodeErrorInner::SerializationFailed(error) => Some(error),
        }
    }
}

/// Stable classification for a revision-state decoding failure.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum RevisionStateDecodeErrorKind {
    /// The encoded document exceeds its injected resource policy.
    ResourceLimitExceeded,
    /// The input is not a structurally valid supported document.
    MalformedDocument,
    /// The input identifies a different wire schema.
    UnsupportedSchema,
    /// The input uses a revision-state version unknown to this codec.
    UnsupportedVersion,
}

/// Failure while decoding a revision-state document.
///
/// The stable kind and detail getters let callers branch without depending on
/// serde or on the codec's private parser implementation.
#[derive(Debug)]
pub struct RevisionStateDecodeError {
    inner: RevisionStateDecodeErrorInner,
}

#[derive(Debug)]
enum RevisionStateDecodeErrorInner {
    ResourceLimitExceeded(DocumentLimitExceeded),
    MalformedDocument(serde_json::Error),
    UnsupportedSchema(String),
    UnsupportedVersion(u64),
}

impl RevisionStateDecodeError {
    fn resource_limit_exceeded(error: DocumentLimitExceeded) -> Self {
        Self {
            inner: RevisionStateDecodeErrorInner::ResourceLimitExceeded(error),
        }
    }

    fn malformed_document(error: serde_json::Error) -> Self {
        Self {
            inner: RevisionStateDecodeErrorInner::MalformedDocument(error),
        }
    }

    fn for_unsupported_schema(schema: String) -> Self {
        Self {
            inner: RevisionStateDecodeErrorInner::UnsupportedSchema(schema),
        }
    }

    const fn for_unsupported_version(version: u64) -> Self {
        Self {
            inner: RevisionStateDecodeErrorInner::UnsupportedVersion(version),
        }
    }

    /// Returns the stable category of this failure.
    pub const fn kind(&self) -> RevisionStateDecodeErrorKind {
        match &self.inner {
            RevisionStateDecodeErrorInner::ResourceLimitExceeded(_) => {
                RevisionStateDecodeErrorKind::ResourceLimitExceeded
            }
            RevisionStateDecodeErrorInner::MalformedDocument(_) => {
                RevisionStateDecodeErrorKind::MalformedDocument
            }
            RevisionStateDecodeErrorInner::UnsupportedSchema(_) => {
                RevisionStateDecodeErrorKind::UnsupportedSchema
            }
            RevisionStateDecodeErrorInner::UnsupportedVersion(_) => {
                RevisionStateDecodeErrorKind::UnsupportedVersion
            }
        }
    }

    /// Returns the unsupported schema identity, when present.
    pub fn unsupported_schema(&self) -> Option<&str> {
        match &self.inner {
            RevisionStateDecodeErrorInner::UnsupportedSchema(schema) => Some(schema),
            RevisionStateDecodeErrorInner::ResourceLimitExceeded(_)
            | RevisionStateDecodeErrorInner::MalformedDocument(_)
            | RevisionStateDecodeErrorInner::UnsupportedVersion(_) => None,
        }
    }

    /// Returns the unsupported format version, when present.
    pub const fn unsupported_version(&self) -> Option<u64> {
        match &self.inner {
            RevisionStateDecodeErrorInner::UnsupportedVersion(version) => Some(*version),
            RevisionStateDecodeErrorInner::ResourceLimitExceeded(_)
            | RevisionStateDecodeErrorInner::MalformedDocument(_)
            | RevisionStateDecodeErrorInner::UnsupportedSchema(_) => None,
        }
    }

    /// Returns `(actual_bytes, max_bytes)` for a resource-limit failure.
    pub const fn document_size_limit(&self) -> Option<(usize, usize)> {
        match &self.inner {
            RevisionStateDecodeErrorInner::ResourceLimitExceeded(error) => {
                Some((error.actual_bytes, error.max_bytes))
            }
            RevisionStateDecodeErrorInner::MalformedDocument(_)
            | RevisionStateDecodeErrorInner::UnsupportedSchema(_)
            | RevisionStateDecodeErrorInner::UnsupportedVersion(_) => None,
        }
    }
}

impl fmt::Display for RevisionStateDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.inner {
            RevisionStateDecodeErrorInner::ResourceLimitExceeded(error) => error.fmt(formatter),
            RevisionStateDecodeErrorInner::MalformedDocument(error) => {
                write!(formatter, "malformed revision state document: {error}")
            }
            RevisionStateDecodeErrorInner::UnsupportedSchema(schema) => {
                write!(formatter, "unsupported revision state schema: {schema:?}")
            }
            RevisionStateDecodeErrorInner::UnsupportedVersion(version) => {
                write!(formatter, "unsupported revision state version: {version}")
            }
        }
    }
}

impl Error for RevisionStateDecodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.inner {
            RevisionStateDecodeErrorInner::MalformedDocument(error) => Some(error),
            RevisionStateDecodeErrorInner::ResourceLimitExceeded(_)
            | RevisionStateDecodeErrorInner::UnsupportedSchema(_)
            | RevisionStateDecodeErrorInner::UnsupportedVersion(_) => None,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RevisionStateDocumentV1 {
    schema: String,
    version: u64,
    status: WireRevisionStatusV1,
}
