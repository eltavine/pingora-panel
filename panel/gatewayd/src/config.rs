use crate::{
    GatewayResourceLimits, LoopbackOnlyManagementBindPolicy, ManagementBindPolicy, ShutdownPolicy,
};
use panel_errors::{PanelError, Result};
use std::{
    ffi::{OsStr, OsString},
    net::SocketAddr,
    num::NonZeroU32,
    path::{Path, PathBuf},
    time::Duration,
};

pub const GATEWAY_ADDRESS_ENV: &str = "PINGORA_PANEL_GATEWAY_ADDR";
pub const STATE_DIRECTORY_ENV: &str = "PINGORA_PANEL_STATE_DIR";
pub const WORKER_COUNT_ENV: &str = "PINGORA_PANEL_WORKERS";
pub const DRAIN_TIMEOUT_MILLIS_ENV: &str = "PINGORA_PANEL_DRAIN_TIMEOUT_MS";

pub const MAX_GATEWAY_WORKERS: u32 = 256;

const DEFAULT_LISTEN_ADDRESS: &str = "127.0.0.1:50051";
const DEFAULT_STATE_DIRECTORY: &str = "/var/lib/pingora-panel/gateway";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GatewaydConfig {
    listen_address: SocketAddr,
    state_directory: PathBuf,
    worker_count: GatewayWorkerCount,
    shutdown_policy: ShutdownPolicy,
    resource_limits: GatewayResourceLimits,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GatewayWorkerCount(NonZeroU32);

impl GatewayWorkerCount {
    pub fn new(value: u32) -> Result<Self> {
        let value = NonZeroU32::new(value).ok_or_else(|| {
            PanelError::invalid_argument(format!("{WORKER_COUNT_ENV} must be greater than zero"))
        })?;
        if value.get() > MAX_GATEWAY_WORKERS {
            return Err(PanelError::invalid_argument(format!(
                "{WORKER_COUNT_ENV} must not exceed {MAX_GATEWAY_WORKERS}"
            )));
        }
        Ok(Self(value))
    }

    pub fn get(self) -> u32 {
        self.0.get()
    }

    pub fn as_non_zero(self) -> NonZeroU32 {
        self.0
    }
}

impl GatewaydConfig {
    pub fn from_environment() -> Result<Self> {
        Self::from_environment_with_policy(&LoopbackOnlyManagementBindPolicy)
    }

    pub fn from_environment_with_policy(bind_policy: &dyn ManagementBindPolicy) -> Result<Self> {
        Self::from_lookup_with_policy(|key| std::env::var_os(key), bind_policy)
    }

    /// Parse configuration through an injected lookup so tests and future config
    /// backends do not mutate process-global environment variables.
    pub fn from_lookup(mut lookup: impl FnMut(&str) -> Option<OsString>) -> Result<Self> {
        Self::from_lookup_with_policy(&mut lookup, &LoopbackOnlyManagementBindPolicy)
    }

    pub fn from_lookup_with_policy(
        mut lookup: impl FnMut(&str) -> Option<OsString>,
        bind_policy: &dyn ManagementBindPolicy,
    ) -> Result<Self> {
        let listen_address = lookup(GATEWAY_ADDRESS_ENV)
            .map(|value| parse_address(&value))
            .transpose()?
            .unwrap_or_else(|| {
                DEFAULT_LISTEN_ADDRESS
                    .parse()
                    .expect("default gateway address is valid")
            });
        bind_policy.validate(listen_address)?;
        let state_directory = lookup(STATE_DIRECTORY_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_STATE_DIRECTORY));
        let worker_count = lookup(WORKER_COUNT_ENV)
            .map(|value| parse_worker_count(&value))
            .transpose()?
            .unwrap_or_else(default_worker_count);
        let shutdown_policy = lookup(DRAIN_TIMEOUT_MILLIS_ENV)
            .map(|value| parse_shutdown_policy(&value))
            .transpose()?
            .unwrap_or_default();
        let resource_limits = GatewayResourceLimits::from_lookup(&mut lookup)?;

        Ok(Self {
            listen_address,
            state_directory,
            worker_count,
            shutdown_policy,
            resource_limits,
        })
    }

    pub fn listen_address(&self) -> SocketAddr {
        self.listen_address
    }

    pub fn state_directory(&self) -> &Path {
        &self.state_directory
    }

    pub fn worker_count(&self) -> GatewayWorkerCount {
        self.worker_count
    }

    pub fn shutdown_policy(&self) -> ShutdownPolicy {
        self.shutdown_policy
    }

    pub fn resource_limits(&self) -> GatewayResourceLimits {
        self.resource_limits
    }
}

fn parse_address(value: &OsStr) -> Result<SocketAddr> {
    let value = value.to_str().ok_or_else(|| {
        PanelError::invalid_argument(format!("{GATEWAY_ADDRESS_ENV} must be valid UTF-8"))
    })?;
    value.parse().map_err(|error| {
        PanelError::invalid_argument(format!("invalid {GATEWAY_ADDRESS_ENV}: {error}"))
    })
}

fn parse_worker_count(value: &OsStr) -> Result<GatewayWorkerCount> {
    let value = value.to_str().ok_or_else(|| {
        PanelError::invalid_argument(format!("{WORKER_COUNT_ENV} must be valid UTF-8"))
    })?;
    let parsed = value.parse().map_err(|error| {
        PanelError::invalid_argument(format!(
            "{WORKER_COUNT_ENV} must be a positive 32-bit integer: {error}"
        ))
    })?;
    GatewayWorkerCount::new(parsed)
}

fn parse_shutdown_policy(value: &OsStr) -> Result<ShutdownPolicy> {
    let value = value.to_str().ok_or_else(|| {
        PanelError::invalid_argument(format!("{DRAIN_TIMEOUT_MILLIS_ENV} must be valid UTF-8"))
    })?;
    let milliseconds = value.parse().map_err(|error| {
        PanelError::invalid_argument(format!(
            "{DRAIN_TIMEOUT_MILLIS_ENV} must be an unsigned integer: {error}"
        ))
    })?;
    ShutdownPolicy::new(Duration::from_millis(milliseconds))
}

fn default_worker_count() -> GatewayWorkerCount {
    let available = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1);
    let available = u32::try_from(available)
        .unwrap_or(MAX_GATEWAY_WORKERS)
        .min(MAX_GATEWAY_WORKERS);
    GatewayWorkerCount::new(available).expect("available parallelism is non-zero and bounded")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct AllowAuthenticatedRemote;

    impl ManagementBindPolicy for AllowAuthenticatedRemote {
        fn validate(&self, _address: SocketAddr) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn injected_values_are_parsed_without_global_environment_mutation() {
        let values = HashMap::from([
            (GATEWAY_ADDRESS_ENV, OsString::from("127.0.0.1:51051")),
            (STATE_DIRECTORY_ENV, OsString::from("/tmp/gateway-state")),
            (WORKER_COUNT_ENV, OsString::from("3")),
            (DRAIN_TIMEOUT_MILLIS_ENV, OsString::from("250")),
        ]);
        let config = GatewaydConfig::from_lookup(|key| values.get(key).cloned()).unwrap();

        assert_eq!(config.listen_address().port(), 51051);
        assert_eq!(config.state_directory(), Path::new("/tmp/gateway-state"));
        assert_eq!(config.worker_count().get(), 3);
        assert_eq!(
            config.shutdown_policy().drain_timeout(),
            Duration::from_millis(250)
        );
    }

    #[test]
    fn zero_workers_are_rejected() {
        let error = GatewaydConfig::from_lookup(|key| {
            (key == WORKER_COUNT_ENV).then(|| OsString::from("0"))
        })
        .unwrap_err();

        assert_eq!(
            error.code.as_str(),
            panel_errors::ErrorCode::INVALID_ARGUMENT
        );
    }

    #[test]
    fn excessive_workers_are_rejected() {
        let error = GatewaydConfig::from_lookup(|key| {
            (key == WORKER_COUNT_ENV).then(|| OsString::from((MAX_GATEWAY_WORKERS + 1).to_string()))
        })
        .unwrap_err();

        assert_eq!(
            error.code.as_str(),
            panel_errors::ErrorCode::INVALID_ARGUMENT
        );
    }

    #[test]
    fn non_loopback_plaintext_listener_is_rejected() {
        let error = GatewaydConfig::from_lookup(|key| {
            (key == GATEWAY_ADDRESS_ENV).then(|| OsString::from("0.0.0.0:50051"))
        })
        .unwrap_err();

        assert_eq!(
            error.code.as_str(),
            panel_errors::ErrorCode::INVALID_ARGUMENT
        );
    }

    #[test]
    fn authenticated_transport_can_replace_the_default_bind_policy() {
        let config = GatewaydConfig::from_lookup_with_policy(
            |key| (key == GATEWAY_ADDRESS_ENV).then(|| OsString::from("192.0.2.10:50051")),
            &AllowAuthenticatedRemote,
        )
        .unwrap();

        assert_eq!(config.listen_address(), "192.0.2.10:50051".parse().unwrap());
    }
}
