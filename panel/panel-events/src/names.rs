use panel_errors::{PanelError, Result};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

macro_rules! validated_name {
    ($(#[$meta:meta])* $name:ident, $validate:path) => {
        $(#[$meta])*
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self> {
                let value = value.into();
                $validate(&value)?;
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
            fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
            }
        }
    };
}

validated_name!(
    /// Dot-separated event name such as `config.revision.activated`.
    ///
    /// Each token is a broker subject token, so the type maps onto subjects
    /// and consumer filters without escaping.
    EventType,
    validate_event_type
);

validated_name!(
    /// A service identity such as `config-service`.
    ServiceName,
    validate_component_name
);

validated_name!(
    /// A durable consumer identity. Durable names cannot contain subject
    /// separators, so they follow the same rule as service names.
    ConsumerName,
    validate_component_name
);

validated_name!(
    /// The kind of aggregate an event belongs to, such as `revision`.
    AggregateType,
    validate_aggregate_type
);

validated_name!(
    /// The identity of one aggregate instance.
    AggregateId,
    validate_aggregate_id
);

/// The aggregate whose state change an event records.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct AggregateRef {
    pub aggregate_type: AggregateType,
    pub aggregate_id: AggregateId,
}

impl AggregateRef {
    pub fn new(aggregate_type: AggregateType, aggregate_id: AggregateId) -> Self {
        Self {
            aggregate_type,
            aggregate_id,
        }
    }

    /// The CloudEvents `subject`: `<aggregate type>/<aggregate id>`.
    pub fn subject(&self) -> String {
        format!("{}/{}", self.aggregate_type, self.aggregate_id)
    }

    pub fn parse_subject(subject: &str) -> Result<Self> {
        let (aggregate_type, aggregate_id) = subject.split_once('/').ok_or_else(|| {
            PanelError::invalid_argument("event subject must be <aggregate type>/<aggregate id>")
        })?;
        Ok(Self::new(
            AggregateType::new(aggregate_type)?,
            AggregateId::new(aggregate_id)?,
        ))
    }
}

impl EventType {
    pub const MAX_BYTES: usize = 128;
    pub const MAX_TOKENS: usize = 8;

    /// Subject tokens in order.
    pub fn tokens(&self) -> impl Iterator<Item = &str> {
        self.0.split('.')
    }
}

fn validate_event_type(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > EventType::MAX_BYTES {
        return Err(PanelError::invalid_argument(format!(
            "event type must contain 1..={} bytes",
            EventType::MAX_BYTES
        )));
    }
    let tokens = value.split('.').collect::<Vec<_>>();
    if tokens.len() < 2 || tokens.len() > EventType::MAX_TOKENS {
        return Err(PanelError::invalid_argument(format!(
            "event type must contain 2..={} dot-separated tokens",
            EventType::MAX_TOKENS
        )));
    }
    if tokens.iter().any(|token| {
        token.is_empty()
            || !token.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte)
            })
    }) {
        return Err(PanelError::invalid_argument(
            "event type tokens may contain only lowercase ASCII letters, digits, '_' and '-'",
        ));
    }
    Ok(())
}

fn validate_component_name(value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    let valid = (1..=63).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes[bytes.len() - 1] != b'-'
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-');
    if !valid {
        return Err(PanelError::invalid_argument(
            "component names must match [a-z][a-z0-9-]{0,62} and must not end with '-'",
        ));
    }
    Ok(())
}

fn validate_aggregate_type(value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    let valid = (1..=64).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_');
    if !valid {
        return Err(PanelError::invalid_argument(
            "aggregate types must match [a-z][a-z0-9_]{0,63}",
        ));
    }
    Ok(())
}

fn validate_aggregate_id(value: &str) -> Result<()> {
    let valid = (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:@/-".contains(&byte));
    if !valid {
        return Err(PanelError::invalid_argument(
            "aggregate ids must contain 1..=128 ASCII letters, digits or ._:@/-",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_types_are_subject_safe() {
        assert!(EventType::new("config.revision.activated").is_ok());
        assert!(EventType::new("gateway.snapshot_prepared").is_ok());
        assert!(EventType::new("single").is_err());
        assert!(EventType::new("config..activated").is_err());
        assert!(EventType::new("config.*").is_err());
        assert!(EventType::new("config.>").is_err());
        assert!(EventType::new("Config.Revision").is_err());
        assert!(EventType::new("a.b.c.d.e.f.g.h.i").is_err());
        assert_eq!(
            EventType::new("job.progress.updated")
                .unwrap()
                .tokens()
                .collect::<Vec<_>>(),
            ["job", "progress", "updated"]
        );
    }

    #[test]
    fn component_names_are_durable_safe() {
        assert!(ServiceName::new("config-service").is_ok());
        assert!(ConsumerName::new("automation-renewal-2").is_ok());
        assert!(ServiceName::new("Config").is_err());
        assert!(ServiceName::new("config.service").is_err());
        assert!(ServiceName::new("config-").is_err());
        assert!(ServiceName::new("1config").is_err());
        assert!(ConsumerName::new("x".repeat(64)).is_err());
    }

    #[test]
    fn aggregates_are_bounded() {
        assert!(AggregateType::new("revision").is_ok());
        assert!(AggregateType::new("Revision").is_err());
        assert!(AggregateId::new("018f6c4e-3a5b-7c1d-9e2f-0a1b2c3d4e5f").is_ok());
        assert!(AggregateId::new("site:example.com/42").is_ok());
        assert!(AggregateId::new("has space").is_err());
        assert!(serde_json::from_str::<AggregateId>("\"\"").is_err());
    }

    #[test]
    fn aggregates_map_onto_cloudevents_subjects() {
        let aggregate = AggregateRef::new(
            AggregateType::new("site").unwrap(),
            AggregateId::new("example.com/blog").unwrap(),
        );
        assert_eq!(aggregate.subject(), "site/example.com/blog");
        assert_eq!(
            AggregateRef::parse_subject(&aggregate.subject()).unwrap(),
            aggregate
        );
        assert!(AggregateRef::parse_subject("site").is_err());
        assert!(AggregateRef::parse_subject("Site/1").is_err());
    }
}
