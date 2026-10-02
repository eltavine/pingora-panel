#![forbid(unsafe_code)]

//! Stable, transport-neutral error values shared by Panel layers.

use serde::{ser::SerializeStruct, Deserialize, Serialize};
use std::{error::Error, fmt};

/// Extensible public error identifier. Unknown values must be preserved by transports.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ErrorCode(String);

impl ErrorCode {
    pub const INVALID_ARGUMENT: &'static str = "INVALID_ARGUMENT";
    pub const VALIDATION_FAILED: &'static str = "VALIDATION_FAILED";
    pub const CONFLICT: &'static str = "CONFLICT";
    pub const NOT_FOUND: &'static str = "NOT_FOUND";
    pub const PRECONDITION_FAILED: &'static str = "PRECONDITION_FAILED";
    pub const UNSUPPORTED_CAPABILITY: &'static str = "UNSUPPORTED_CAPABILITY";
    pub const PREPARE_FAILED: &'static str = "PREPARE_FAILED";
    pub const ACTIVATE_FAILED: &'static str = "ACTIVATE_FAILED";
    pub const COMMIT_OUTCOME_UNKNOWN: &'static str = "COMMIT_OUTCOME_UNKNOWN";
    pub const STORAGE_UNAVAILABLE: &'static str = "STORAGE_UNAVAILABLE";
    /// The service cannot accept the request now, for example while a
    /// dependency it needs is down; the same request can succeed later.
    pub const UNAVAILABLE: &'static str = "UNAVAILABLE";
    pub const CORRUPT_STATE: &'static str = "CORRUPT_STATE";
    pub const RESOURCE_EXHAUSTED: &'static str = "RESOURCE_EXHAUSTED";
    pub const DEADLINE_EXCEEDED: &'static str = "DEADLINE_EXCEEDED";
    pub const UNAUTHENTICATED: &'static str = "UNAUTHENTICATED";
    pub const PERMISSION_DENIED: &'static str = "PERMISSION_DENIED";
    pub const INTERNAL: &'static str = "INTERNAL";

    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for ErrorCode {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for ErrorCode {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: ErrorCode,
    pub severity: DiagnosticSeverity,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_span: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

impl Diagnostic {
    pub fn error(code: impl Into<ErrorCode>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            severity: DiagnosticSeverity::Error,
            message: message.into(),
            source_span: None,
            resource_id: None,
            help: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidationReport {
    pub valid: bool,
    pub diagnostics: Vec<Diagnostic>,
}

impl ValidationReport {
    pub fn valid() -> Self {
        Self {
            valid: true,
            diagnostics: Vec::new(),
        }
    }

    pub fn from_diagnostics(diagnostics: Vec<Diagnostic>) -> Self {
        let valid = !diagnostics
            .iter()
            .any(|item| item.severity == DiagnosticSeverity::Error);
        Self { valid, diagnostics }
    }
}

/// Public error envelope. `source` is intentionally omitted from serialization.
#[derive(Debug)]
pub struct PanelError {
    pub code: ErrorCode,
    pub message: String,
    pub diagnostics: Vec<Diagnostic>,
    pub retryable: bool,
    confirmed_precommit: bool,
    source: Option<Box<dyn Error + Send + Sync + 'static>>,
}

impl Serialize for PanelError {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("PanelError", 4)?;
        state.serialize_field("code", &self.code)?;
        state.serialize_field("message", &self.message)?;
        state.serialize_field("diagnostics", &self.diagnostics)?;
        state.serialize_field("retryable", &self.retryable)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for PanelError {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct WireError {
            code: ErrorCode,
            message: String,
            #[serde(default)]
            diagnostics: Vec<Diagnostic>,
            #[serde(default)]
            retryable: bool,
        }

        let wire = WireError::deserialize(deserializer)?;
        Ok(Self {
            code: wire.code,
            message: wire.message,
            diagnostics: wire.diagnostics,
            retryable: wire.retryable,
            confirmed_precommit: false,
            source: None,
        })
    }
}

impl Clone for PanelError {
    fn clone(&self) -> Self {
        Self {
            code: self.code.clone(),
            message: self.message.clone(),
            diagnostics: self.diagnostics.clone(),
            retryable: self.retryable,
            confirmed_precommit: self.confirmed_precommit,
            source: None,
        }
    }
}

impl PanelError {
    pub fn new(code: impl Into<ErrorCode>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            diagnostics: Vec::new(),
            retryable: false,
            confirmed_precommit: false,
            source: None,
        }
    }

    pub fn with_diagnostics(mut self, diagnostics: Vec<Diagnostic>) -> Self {
        self.diagnostics = diagnostics;
        self
    }

    pub fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    pub fn with_source(mut self, source: impl Error + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::INVALID_ARGUMENT, message)
    }

    pub fn validation_failed(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::VALIDATION_FAILED, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::CONFLICT, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NOT_FOUND, message)
    }

    pub fn precondition_failed(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::PRECONDITION_FAILED, message)
    }

    pub fn unsupported_capability(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::UNSUPPORTED_CAPABILITY, message)
    }

    pub fn prepare_failed(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::PREPARE_FAILED, message)
    }

    pub fn activate_failed(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ACTIVATE_FAILED, message)
    }

    /// Reports that publication reached the atomic namespace replacement, but
    /// the containing directory could not be synchronized.
    pub fn commit_outcome_unknown(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::COMMIT_OUTCOME_UNKNOWN, message).retryable(true)
    }

    pub fn storage_unavailable(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::STORAGE_UNAVAILABLE, message).retryable(true)
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::UNAVAILABLE, message).retryable(true)
    }

    pub fn corrupt_state(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::CORRUPT_STATE, message)
    }

    pub fn resource_exhausted(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::RESOURCE_EXHAUSTED, message).retryable(true)
    }

    pub fn deadline_exceeded(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::DEADLINE_EXCEEDED, message).retryable(true)
    }

    /// Marks a local deadline rejection proven to occur before dispatch.
    /// The marker is deliberately omitted from the wire form: a remote error
    /// cannot establish commit certainty for this process.
    pub fn deadline_exceeded_before_dispatch(message: impl Into<String>) -> Self {
        let mut error = Self::deadline_exceeded(message);
        error.confirmed_precommit = true;
        error
    }

    pub fn is_confirmed_precommit(&self) -> bool {
        self.confirmed_precommit
    }

    pub fn unauthenticated(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::UNAUTHENTICATED, message)
    }

    pub fn permission_denied(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::PERMISSION_DENIED, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::INTERNAL, message)
    }
}

impl fmt::Display for PanelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl Error for PanelError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|error| error as &(dyn Error + 'static))
    }
}

pub type Result<T> = std::result::Result<T, PanelError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_codes_round_trip() {
        let error = PanelError::new(ErrorCode::new("FUTURE_CODE"), "future");
        let json = serde_json::to_string(&error).unwrap();
        assert!(json.contains("FUTURE_CODE"));
        let decoded: PanelError = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.code.as_str(), "FUTURE_CODE");
        assert!(decoded.source().is_none());
    }

    #[test]
    fn precommit_certainty_is_local_only() {
        let error = PanelError::deadline_exceeded_before_dispatch("expired");
        assert!(error.is_confirmed_precommit());
        assert!(error.clone().is_confirmed_precommit());
        let wire = serde_json::to_string(&error).unwrap();
        let decoded: PanelError = serde_json::from_str(&wire).unwrap();
        assert!(!decoded.is_confirmed_precommit());
        assert!(!PanelError::deadline_exceeded("unknown outcome").is_confirmed_precommit());
    }

    #[test]
    fn validation_report_tracks_error_diagnostic() {
        let report = ValidationReport::from_diagnostics(vec![Diagnostic::error(
            ErrorCode::VALIDATION_FAILED,
            "bad host",
        )]);
        assert!(!report.valid);
    }
}
