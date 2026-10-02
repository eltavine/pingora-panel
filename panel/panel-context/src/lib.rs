#![forbid(unsafe_code)]

//! Request-scoped identity values shared by every surface, transport and
//! event producer.
//!
//! Commands, events and audit records carry the same identifiers, so the
//! validation rules live here once instead of being repeated per adapter.

use chrono::DateTime;
use panel_errors::{PanelError, Result};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

fn validate_bounded_ascii(value: &str, label: &str, max_bytes: usize) -> Result<()> {
    if value.is_empty() || value.len() > max_bytes || !value.is_ascii() {
        return Err(PanelError::invalid_argument(format!(
            "{label} must contain 1..={max_bytes} visible ASCII bytes"
        )));
    }
    if value.bytes().any(|byte| !(0x20..=0x7e).contains(&byte)) {
        return Err(PanelError::invalid_argument(format!(
            "{label} must contain visible ASCII bytes"
        )));
    }
    Ok(())
}

macro_rules! bounded_ascii {
    ($(#[$meta:meta])* $name:ident, $label:literal, $max:literal) => {
        $(#[$meta])*
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub const MAX_BYTES: usize = $max;

            pub fn new(value: impl Into<String>) -> Result<Self> {
                let value = value.into();
                validate_bounded_ascii(&value, $label, $max)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = PanelError;

            fn try_from(value: String) -> Result<Self> {
                Self::new(value)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                Self::try_from(String::deserialize(deserializer)?)
                    .map_err(serde::de::Error::custom)
            }
        }
    };
}

bounded_ascii!(
    /// A caller supplied key that makes a mutating command safe to retry.
    IdempotencyKey,
    "idempotency key",
    256
);

bounded_ascii!(
    /// A message identifier: a request ID, a correlation ID, or the ID of the
    /// command or event that caused another message.
    RequestId,
    "request id",
    256
);

bounded_ascii!(
    /// The authenticated principal, or a `system:<component>` identity, that
    /// is accountable for a command or event.
    Actor,
    "actor",
    256
);

/// An absolute RFC 3339 deadline shared by all mutating command surfaces.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct RequestDeadline(String);

impl RequestDeadline {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        validate_bounded_ascii(&value, "deadline", 64)?;
        DateTime::parse_from_rfc3339(&value).map_err(|error| {
            PanelError::invalid_argument(format!("deadline must be RFC 3339: {error}"))
        })?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RequestDeadline {
    type Error = PanelError;

    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for RequestDeadline {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::try_from(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_values_have_bounded_wire_shapes() {
        assert!(IdempotencyKey::new("deploy-1").is_ok());
        assert!(IdempotencyKey::new("").is_err());
        assert!(IdempotencyKey::new("x".repeat(257)).is_err());
        assert!(RequestId::new("request-1").is_ok());
        assert!(RequestDeadline::new("2026-09-06T12:00:00Z").is_ok());
        assert!(RequestDeadline::new("tomorrow").is_err());
        assert!(RequestId::new("request\n1").is_err());
        assert!(IdempotencyKey::new("部署-1").is_err());
        assert!(Actor::new("system:automation-service").is_ok());
        assert!(Actor::new("").is_err());
        assert!(serde_json::from_str::<IdempotencyKey>("\"\"").is_err());
        assert!(serde_json::from_str::<Actor>("\"operator@example.com\"").is_ok());
    }
}
