use chrono::{DateTime, Utc};
use panel_context::{IdempotencyKey, RequestId};
use panel_errors::{PanelError, Result};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};
use uuid::Uuid;

/// A job's identity: a UUIDv7, so identities sort by creation time.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct JobId(Uuid);

impl JobId {
    pub fn generate() -> Self {
        Self(Uuid::now_v7())
    }

    pub fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for JobId {
    type Err = PanelError;

    fn from_str(value: &str) -> Result<Self> {
        Uuid::parse_str(value)
            .map(Self)
            .map_err(|_| PanelError::invalid_argument("job IDs are UUIDs"))
    }
}

macro_rules! dotted_name {
    ($(#[$meta:meta])* $name:ident, $what:literal) => {
        $(#[$meta])*
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Dot-separated tokens of lowercase letters, digits, `-` and
            /// `_`, each starting with a letter; at most 128 bytes.
            pub fn new(value: impl Into<String>) -> Result<Self> {
                let value = value.into();
                let token = |token: &str| {
                    token.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
                        && token.bytes().all(|byte| {
                            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-_".contains(&byte)
                        })
                };
                if value.len() > 128 || !value.split('.').all(token) {
                    return Err(PanelError::invalid_argument(format!(
                        "{} `{value}` must be dot-separated lowercase tokens",
                        $what
                    )));
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
                Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
            }
        }
    };
}

dotted_name!(
    /// What a job does, such as `certificate.renew`; selects its handler.
    JobKind,
    "job kind"
);

dotted_name!(
    /// The name of a schedule or maintenance window, such as `nightly`.
    ScheduleName,
    "name"
);

/// Where a job is in its lifecycle.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Waiting to run for the first time.
    Queued,
    /// Leased to a worker.
    Running,
    /// Waiting to run again after a failed attempt.
    Retrying,
    Succeeded,
    /// Failed permanently or ran out of attempts.
    Failed,
    Cancelled,
}

impl JobState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Retrying => "retrying",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_final(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

impl FromStr for JobState {
    type Err = PanelError;

    fn from_str(value: &str) -> Result<Self> {
        Ok(match value {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "retrying" => Self::Retrying,
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => {
                return Err(PanelError::invalid_argument(format!(
                    "unknown job state `{value}`"
                )))
            }
        })
    }
}

/// How far a running job has got.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    percent: u8,
    message: String,
}

impl Progress {
    pub const MAX_MESSAGE_BYTES: usize = 256;

    pub fn new(percent: u8, message: impl Into<String>) -> Result<Self> {
        let message = message.into();
        if percent > 100 || message.len() > Self::MAX_MESSAGE_BYTES {
            return Err(PanelError::invalid_argument(
                "progress is 0..=100 percent with a message of at most 256 bytes",
            ));
        }
        Ok(Self { percent, message })
    }

    pub fn percent(&self) -> u8 {
        self.percent
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Why an attempt failed, in the stable error vocabulary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct JobError {
    pub code: String,
    pub message: String,
}

impl From<&PanelError> for JobError {
    fn from(error: &PanelError) -> Self {
        Self {
            code: error.code.to_string(),
            message: error.message.clone(),
        }
    }
}

/// The request that caused a job, for its events.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct JobOrigin {
    pub correlation_id: RequestId,
    pub causation_id: RequestId,
}

/// A job to enqueue.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobSpec {
    pub kind: JobKind,
    /// Enqueuing the same kind and key again returns the existing job.
    pub idempotency_key: IdempotencyKey,
    pub media_type: String,
    pub payload: Vec<u8>,
    pub max_attempts: u32,
    /// Higher runs first.
    pub priority: i16,
    pub not_before: Option<DateTime<Utc>>,
    /// The job only starts while this window is open.
    pub maintenance_window: Option<ScheduleName>,
    pub origin: JobOrigin,
}

impl JobSpec {
    /// A JSON job with three attempts and default priority.
    pub fn json(
        kind: JobKind,
        idempotency_key: IdempotencyKey,
        payload: &impl Serialize,
        origin: JobOrigin,
    ) -> Result<Self> {
        Ok(Self {
            kind,
            idempotency_key,
            media_type: "application/json".into(),
            payload: serde_json::to_vec(payload).map_err(|error| {
                PanelError::invalid_argument(format!("job payload cannot be encoded: {error}"))
            })?,
            max_attempts: 3,
            priority: 0,
            not_before: None,
            maintenance_window: None,
            origin,
        })
    }

    pub fn validate(&self) -> Result<()> {
        if self.max_attempts == 0 || self.max_attempts > 100 {
            return Err(PanelError::invalid_argument("jobs allow 1..=100 attempts"));
        }
        if self.payload.len() > 64 * 1024 {
            return Err(PanelError::invalid_argument(
                "job payloads are at most 64 KiB",
            ));
        }
        Ok(())
    }
}

/// A job as stored.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Job {
    pub id: JobId,
    pub kind: JobKind,
    pub idempotency_key: IdempotencyKey,
    pub state: JobState,
    pub attempts: u32,
    pub max_attempts: u32,
    pub media_type: String,
    pub payload: Vec<u8>,
    pub priority: i16,
    pub run_after: DateTime<Utc>,
    pub maintenance_window: Option<ScheduleName>,
    pub cancel_requested: bool,
    pub progress: Option<Progress>,
    pub last_error: Option<JobError>,
    pub origin: JobOrigin,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl Job {
    /// The payload decoded from JSON.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_slice(&self.payload).map_err(|error| {
            PanelError::invalid_argument(format!("job payload is not the expected JSON: {error}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_states_and_progress_are_validated() {
        assert!(JobKind::new("certificate.renew").is_ok());
        assert!(JobKind::new("backup.run-full_2").is_ok());
        for invalid in ["", "Certificate", "a..b", "1a", "a.", "a b"] {
            assert!(JobKind::new(invalid).is_err(), "{invalid}");
        }
        for state in [
            JobState::Queued,
            JobState::Running,
            JobState::Retrying,
            JobState::Succeeded,
            JobState::Failed,
            JobState::Cancelled,
        ] {
            assert_eq!(state.as_str().parse::<JobState>().unwrap(), state);
        }
        assert!(Progress::new(101, "").is_err());
        assert!(Progress::new(50, "x".repeat(257)).is_err());
        assert_eq!(Progress::new(50, "half").unwrap().percent(), 50);
    }
}
