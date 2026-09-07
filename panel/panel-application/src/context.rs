use chrono::DateTime;
use panel_errors::{PanelError, Result};
use serde::{Deserialize, Deserializer, Serialize};

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

/// A caller supplied key that makes a mutating command safe to retry.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        validate_bounded_ascii(&value, "idempotency key", 256)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for IdempotencyKey {
    type Error = PanelError;

    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for IdempotencyKey {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::try_from(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// An application request identifier kept separate from idempotency identity.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct RequestId(String);

impl RequestId {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        validate_bounded_ascii(&value, "request id", 256)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RequestId {
    type Error = PanelError;

    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for RequestId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::try_from(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

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

/// Authenticated command metadata shared by every mutating surface.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommandContext {
    request_id: RequestId,
    correlation_id: RequestId,
    actor: String,
    deadline: RequestDeadline,
    idempotency_key: IdempotencyKey,
}

impl CommandContext {
    pub fn new(
        request_id: RequestId,
        correlation_id: RequestId,
        actor: impl Into<String>,
        deadline: RequestDeadline,
        idempotency_key: IdempotencyKey,
    ) -> Result<Self> {
        let actor = actor.into();
        validate_bounded_ascii(&actor, "actor", 256)?;
        Ok(Self {
            request_id,
            correlation_id,
            actor,
            deadline,
            idempotency_key,
        })
    }

    pub fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    pub fn correlation_id(&self) -> &RequestId {
        &self.correlation_id
    }

    pub fn actor(&self) -> &str {
        &self.actor
    }

    pub fn deadline(&self) -> &RequestDeadline {
        &self.deadline
    }

    pub fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
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
        assert!(serde_json::from_str::<IdempotencyKey>("\"\"").is_err());
    }
}
