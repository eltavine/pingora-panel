use gateway_grpc::{
    DeadlineRequirement, GatewayTransportPolicy, DEFAULT_MAX_CONCURRENT_REQUESTS,
    DEFAULT_MAX_DECODING_MESSAGE_BYTES, DEFAULT_MAX_ENCODING_MESSAGE_BYTES,
    DEFAULT_REQUEST_TIMEOUT,
};
use panel_errors::{PanelError, Result};
use panel_gateway_runtime::{
    PreparedSnapshotBudget, DEFAULT_MAX_OUTSTANDING_PREPARES, DEFAULT_MAX_PREPARED_SNAPSHOT_BYTES,
    DEFAULT_MAX_TOTAL_PREPARED_BYTES,
};
use std::{ffi::OsStr, num::NonZeroUsize, time::Duration};

pub const GRPC_MAX_DECODING_MESSAGE_BYTES_ENV: &str =
    "PINGORA_PANEL_GRPC_MAX_DECODING_MESSAGE_BYTES";
pub const GRPC_MAX_ENCODING_MESSAGE_BYTES_ENV: &str =
    "PINGORA_PANEL_GRPC_MAX_ENCODING_MESSAGE_BYTES";
pub const GRPC_MAX_CONCURRENT_REQUESTS_ENV: &str = "PINGORA_PANEL_GRPC_MAX_CONCURRENT_REQUESTS";
pub const REQUEST_TIMEOUT_MILLIS_ENV: &str = "PINGORA_PANEL_REQUEST_TIMEOUT_MS";
pub const DEADLINE_REQUIREMENT_ENV: &str = "PINGORA_PANEL_DEADLINE_REQUIREMENT";
pub const MAX_PREPARED_SNAPSHOTS_ENV: &str = "PINGORA_PANEL_MAX_PREPARED_SNAPSHOTS";
pub const MAX_PREPARED_SNAPSHOT_BYTES_ENV: &str = "PINGORA_PANEL_MAX_PREPARED_SNAPSHOT_BYTES";
pub const MAX_TOTAL_PREPARED_BYTES_ENV: &str = "PINGORA_PANEL_MAX_TOTAL_PREPARED_BYTES";
pub const EVENT_BUFFER_CAPACITY_ENV: &str = "PINGORA_PANEL_EVENT_BUFFER_CAPACITY";

pub const DEFAULT_EVENT_BUFFER_CAPACITY: usize = 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GatewayResourceLimits {
    transport: GatewayTransportPolicy,
    prepared: PreparedSnapshotBudget,
    deadline_requirement: DeadlineRequirement,
    event_buffer_capacity: NonZeroUsize,
}

impl GatewayResourceLimits {
    pub fn new(
        transport: GatewayTransportPolicy,
        prepared: PreparedSnapshotBudget,
        deadline_requirement: DeadlineRequirement,
        event_buffer_capacity: usize,
    ) -> Result<Self> {
        let event_buffer_capacity = NonZeroUsize::new(event_buffer_capacity).ok_or_else(|| {
            PanelError::invalid_argument(format!(
                "{EVENT_BUFFER_CAPACITY_ENV} must be greater than zero"
            ))
        })?;
        Ok(Self {
            transport,
            prepared,
            deadline_requirement,
            event_buffer_capacity,
        })
    }

    pub fn from_lookup(mut lookup: impl FnMut(&str) -> Option<std::ffi::OsString>) -> Result<Self> {
        let decoding = parse_optional_usize(
            lookup(GRPC_MAX_DECODING_MESSAGE_BYTES_ENV).as_deref(),
            GRPC_MAX_DECODING_MESSAGE_BYTES_ENV,
        )?
        .unwrap_or(DEFAULT_MAX_DECODING_MESSAGE_BYTES);
        let encoding = parse_optional_usize(
            lookup(GRPC_MAX_ENCODING_MESSAGE_BYTES_ENV).as_deref(),
            GRPC_MAX_ENCODING_MESSAGE_BYTES_ENV,
        )?
        .unwrap_or(DEFAULT_MAX_ENCODING_MESSAGE_BYTES);
        let concurrency = parse_optional_usize(
            lookup(GRPC_MAX_CONCURRENT_REQUESTS_ENV).as_deref(),
            GRPC_MAX_CONCURRENT_REQUESTS_ENV,
        )?
        .unwrap_or(DEFAULT_MAX_CONCURRENT_REQUESTS);
        let timeout_millis = parse_optional_u64(
            lookup(REQUEST_TIMEOUT_MILLIS_ENV).as_deref(),
            REQUEST_TIMEOUT_MILLIS_ENV,
        )?;
        let timeout = timeout_millis
            .map(Duration::from_millis)
            .unwrap_or(DEFAULT_REQUEST_TIMEOUT);
        let transport = GatewayTransportPolicy::new(decoding, encoding, concurrency, timeout)?;

        let max_prepared = parse_optional_usize(
            lookup(MAX_PREPARED_SNAPSHOTS_ENV).as_deref(),
            MAX_PREPARED_SNAPSHOTS_ENV,
        )?
        .unwrap_or(DEFAULT_MAX_OUTSTANDING_PREPARES);
        let max_snapshot_bytes = parse_optional_usize(
            lookup(MAX_PREPARED_SNAPSHOT_BYTES_ENV).as_deref(),
            MAX_PREPARED_SNAPSHOT_BYTES_ENV,
        )?
        .unwrap_or(DEFAULT_MAX_PREPARED_SNAPSHOT_BYTES);
        let max_total_bytes = parse_optional_usize(
            lookup(MAX_TOTAL_PREPARED_BYTES_ENV).as_deref(),
            MAX_TOTAL_PREPARED_BYTES_ENV,
        )?
        .unwrap_or(DEFAULT_MAX_TOTAL_PREPARED_BYTES);
        let prepared =
            PreparedSnapshotBudget::with_limits(max_prepared, max_snapshot_bytes, max_total_bytes)?;

        let deadline_requirement = lookup(DEADLINE_REQUIREMENT_ENV)
            .as_deref()
            .map(parse_deadline_requirement)
            .transpose()?
            .unwrap_or(DeadlineRequirement::Optional);
        let event_buffer_capacity = parse_optional_usize(
            lookup(EVENT_BUFFER_CAPACITY_ENV).as_deref(),
            EVENT_BUFFER_CAPACITY_ENV,
        )?
        .unwrap_or(DEFAULT_EVENT_BUFFER_CAPACITY);

        Self::new(
            transport,
            prepared,
            deadline_requirement,
            event_buffer_capacity,
        )
    }

    pub fn transport_policy(self) -> GatewayTransportPolicy {
        self.transport
    }

    pub fn prepared_snapshot_budget(self) -> PreparedSnapshotBudget {
        self.prepared
    }

    pub fn deadline_requirement(self) -> DeadlineRequirement {
        self.deadline_requirement
    }

    pub fn event_buffer_capacity(self) -> usize {
        self.event_buffer_capacity.get()
    }
}

impl Default for GatewayResourceLimits {
    fn default() -> Self {
        Self::new(
            GatewayTransportPolicy::default(),
            PreparedSnapshotBudget::default(),
            DeadlineRequirement::Optional,
            DEFAULT_EVENT_BUFFER_CAPACITY,
        )
        .expect("default gateway resource limits are valid")
    }
}

fn parse_optional_usize(value: Option<&OsStr>, name: &str) -> Result<Option<usize>> {
    value.map(|value| parse_number(value, name)).transpose()
}

fn parse_optional_u64(value: Option<&OsStr>, name: &str) -> Result<Option<u64>> {
    value.map(|value| parse_number(value, name)).transpose()
}

fn parse_number<T>(value: &OsStr, name: &str) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let value = value
        .to_str()
        .ok_or_else(|| PanelError::invalid_argument(format!("{name} must be valid UTF-8")))?;
    value.parse().map_err(|error| {
        PanelError::invalid_argument(format!("{name} must be an unsigned integer: {error}"))
    })
}

fn parse_deadline_requirement(value: &OsStr) -> Result<DeadlineRequirement> {
    let value = value.to_str().ok_or_else(|| {
        PanelError::invalid_argument(format!("{DEADLINE_REQUIREMENT_ENV} must be valid UTF-8"))
    })?;
    match value {
        "optional" => Ok(DeadlineRequirement::Optional),
        "mutations" => Ok(DeadlineRequirement::Mutations),
        "all" => Ok(DeadlineRequirement::All),
        _ => Err(PanelError::invalid_argument(format!(
            "{DEADLINE_REQUIREMENT_ENV} must be one of: optional, mutations, all"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, ffi::OsString};

    #[test]
    fn injected_resource_limits_are_composed_independently() {
        let values = HashMap::from([
            (GRPC_MAX_DECODING_MESSAGE_BYTES_ENV, "2048"),
            (GRPC_MAX_ENCODING_MESSAGE_BYTES_ENV, "1024"),
            (GRPC_MAX_CONCURRENT_REQUESTS_ENV, "3"),
            (REQUEST_TIMEOUT_MILLIS_ENV, "250"),
            (MAX_PREPARED_SNAPSHOTS_ENV, "7"),
            (MAX_PREPARED_SNAPSHOT_BYTES_ENV, "4096"),
            (MAX_TOTAL_PREPARED_BYTES_ENV, "8192"),
            (DEADLINE_REQUIREMENT_ENV, "mutations"),
            (EVENT_BUFFER_CAPACITY_ENV, "32"),
        ]);
        let limits = GatewayResourceLimits::from_lookup(|key| {
            values.get(key).map(|value| OsString::from(*value))
        })
        .unwrap();

        assert_eq!(limits.transport_policy().max_decoding_message_bytes(), 2048);
        assert_eq!(limits.transport_policy().max_encoding_message_bytes(), 1024);
        assert_eq!(limits.transport_policy().max_concurrent_requests(), 3);
        assert_eq!(
            limits.transport_policy().request_timeout(),
            Duration::from_millis(250)
        );
        assert_eq!(limits.prepared_snapshot_budget().max_outstanding(), 7);
        assert_eq!(limits.prepared_snapshot_budget().max_snapshot_bytes(), 4096);
        assert_eq!(limits.prepared_snapshot_budget().max_total_bytes(), 8192);
        assert_eq!(
            limits.deadline_requirement(),
            DeadlineRequirement::Mutations
        );
        assert_eq!(limits.event_buffer_capacity(), 32);
    }

    #[test]
    fn zero_and_unknown_values_are_rejected() {
        assert!(GatewayResourceLimits::from_lookup(|key| {
            (key == EVENT_BUFFER_CAPACITY_ENV).then(|| OsString::from("0"))
        })
        .is_err());
        assert!(GatewayResourceLimits::from_lookup(|key| {
            (key == DEADLINE_REQUIREMENT_ENV).then(|| OsString::from("sometimes"))
        })
        .is_err());
    }
}
