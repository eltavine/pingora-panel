#![forbid(unsafe_code)]

//! Request-scoped identity values shared by every surface, transport and
//! event producer.
//!
//! Commands, events and audit records carry the same identifiers, so the
//! validation rules live here once instead of being repeated per adapter.

mod command;
mod trace;

pub use command::CommandContext;
pub use trace::{TraceContext, TRACESTATE_PROPAGATION_LIMIT};

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

/// A service identity such as `config-service`: a lowercase DNS-label-like
/// name that maps onto broker subjects and durable names without escaping.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ServiceName(String);

impl ServiceName {
    pub const MAX_BYTES: usize = 63;

    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if !is_component_name(&value) {
            return Err(PanelError::invalid_argument(
                "component names must match [a-z][a-z0-9-]{0,62} and must not end with '-'",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// `[a-z][a-z0-9-]{0,62}` without a trailing `-`.
pub fn is_component_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=ServiceName::MAX_BYTES).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes[bytes.len() - 1] != b'-'
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

impl fmt::Display for ServiceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ServiceName {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// The identity of one request as it crosses surfaces, services and events.
///
/// The request ID names this hop; the correlation ID is shared by every
/// request and event of one logical operation and defaults to the request ID
/// at the system entry point. The trace context is the caller's W3C trace,
/// propagated unchanged by components that do not record spans.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RequestScope {
    request_id: RequestId,
    correlation_id: RequestId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    trace_context: Option<TraceContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    site_scope: Option<SiteScope>,
}

/// The sites a caller may act on with configuration permissions it holds
/// only for some of them (ADR 0021). Absent from a request when the caller
/// may act on every site.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SiteScope {
    /// Permissions held for every site, such as `config.read`.
    #[serde(default)]
    pub unrestricted: Vec<String>,
    #[serde(default)]
    pub limited: Vec<SiteAccess>,
}

/// A permission held for some site groups and sites.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SiteAccess {
    pub permission: String,
    #[serde(default)]
    pub groups: Vec<String>,
    /// Site IDs.
    #[serde(default)]
    pub sites: Vec<String>,
}

impl SiteScope {
    /// Whether `permission` is held for every site.
    pub fn everywhere(&self, permission: &str) -> bool {
        self.unrestricted.iter().any(|held| held == permission)
    }

    /// Whether `permission` is held for the site `id` in `group`.
    pub fn covers(&self, permission: &str, id: &str, group: Option<&str>) -> bool {
        self.everywhere(permission)
            || self.limited.iter().any(|access| {
                access.permission == permission
                    && (access.sites.iter().any(|site| site == id)
                        || group
                            .is_some_and(|group| access.groups.iter().any(|held| held == group)))
            })
    }
}

impl RequestScope {
    /// A scope that starts a new correlation at this request.
    pub fn new(request_id: RequestId) -> Self {
        Self {
            correlation_id: request_id.clone(),
            request_id,
            trace_context: None,
            site_scope: None,
        }
    }

    /// Limits the request to some sites.
    pub fn with_site_scope(mut self, site_scope: Option<SiteScope>) -> Self {
        self.site_scope = site_scope;
        self
    }

    pub fn site_scope(&self) -> Option<&SiteScope> {
        self.site_scope.as_ref()
    }

    pub fn with_correlation_id(mut self, correlation_id: RequestId) -> Self {
        self.correlation_id = correlation_id;
        self
    }

    pub fn with_trace_context(mut self, trace_context: Option<TraceContext>) -> Self {
        self.trace_context = trace_context;
        self
    }

    pub fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    pub fn correlation_id(&self) -> &RequestId {
        &self.correlation_id
    }

    pub fn trace_context(&self) -> Option<&TraceContext> {
        self.trace_context.as_ref()
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

    #[test]
    fn site_scopes_cover_groups_and_sites() {
        let scope = SiteScope {
            unrestricted: vec!["config.read".into()],
            limited: vec![SiteAccess {
                permission: "config.write".into(),
                groups: vec!["shop".into()],
                sites: vec!["s-1".into()],
            }],
        };
        assert!(scope.covers("config.read", "any", None));
        assert!(scope.covers("config.write", "s-2", Some("shop")));
        assert!(scope.covers("config.write", "s-1", None));
        assert!(!scope.covers("config.write", "s-3", Some("intranet")));
        assert!(!scope.covers("config.apply", "s-1", Some("shop")));
    }

    #[test]
    fn scopes_start_a_correlation_unless_one_is_inherited() {
        let request = RequestId::new("req-1").unwrap();
        let scope = RequestScope::new(request.clone());
        assert_eq!(scope.correlation_id(), &request);
        assert!(scope.trace_context().is_none());

        let trace = TraceContext::parse(
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            None,
        );
        let inherited = RequestScope::new(request)
            .with_correlation_id(RequestId::new("corr-9").unwrap())
            .with_trace_context(trace.clone());
        assert_eq!(inherited.correlation_id().as_str(), "corr-9");
        assert_eq!(inherited.trace_context(), trace.as_ref());
    }
}
