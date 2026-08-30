use panel_errors::{PanelError, Result};
use std::{
    ffi::{OsStr, OsString},
    net::SocketAddr,
    num::NonZeroU32,
    path::{Path, PathBuf},
};

pub const GATEWAY_ADDRESS_ENV: &str = "PINGORA_PANEL_GATEWAY_ADDR";
pub const STATE_DIRECTORY_ENV: &str = "PINGORA_PANEL_STATE_DIR";
pub const WORKER_COUNT_ENV: &str = "PINGORA_PANEL_WORKERS";

const DEFAULT_LISTEN_ADDRESS: &str = "127.0.0.1:50051";
const DEFAULT_STATE_DIRECTORY: &str = "/var/lib/pingora-panel/gateway";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GatewaydConfig {
    listen_address: SocketAddr,
    state_directory: PathBuf,
    worker_count: NonZeroU32,
}

impl GatewaydConfig {
    pub fn from_environment() -> Result<Self> {
        Self::from_lookup(|key| std::env::var_os(key))
    }

    /// Parse configuration through an injected lookup so tests and future config
    /// backends do not mutate process-global environment variables.
    pub fn from_lookup(mut lookup: impl FnMut(&str) -> Option<OsString>) -> Result<Self> {
        let listen_address = lookup(GATEWAY_ADDRESS_ENV)
            .map(|value| parse_address(&value))
            .transpose()?
            .unwrap_or_else(|| {
                DEFAULT_LISTEN_ADDRESS
                    .parse()
                    .expect("default gateway address is valid")
            });
        let state_directory = lookup(STATE_DIRECTORY_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_STATE_DIRECTORY));
        let worker_count = lookup(WORKER_COUNT_ENV)
            .map(|value| parse_worker_count(&value))
            .transpose()?
            .unwrap_or_else(default_worker_count);

        Ok(Self {
            listen_address,
            state_directory,
            worker_count,
        })
    }

    pub fn listen_address(&self) -> SocketAddr {
        self.listen_address
    }

    pub fn state_directory(&self) -> &Path {
        &self.state_directory
    }

    pub fn worker_count(&self) -> NonZeroU32 {
        self.worker_count
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

fn parse_worker_count(value: &OsStr) -> Result<NonZeroU32> {
    let value = value.to_str().ok_or_else(|| {
        PanelError::invalid_argument(format!("{WORKER_COUNT_ENV} must be valid UTF-8"))
    })?;
    value.parse().map_err(|error| {
        PanelError::invalid_argument(format!(
            "{WORKER_COUNT_ENV} must be a positive 32-bit integer: {error}"
        ))
    })
}

fn default_worker_count() -> NonZeroU32 {
    let available = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1);
    NonZeroU32::new(u32::try_from(available).unwrap_or(u32::MAX))
        .expect("available parallelism is non-zero")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn injected_values_are_parsed_without_global_environment_mutation() {
        let values = HashMap::from([
            (GATEWAY_ADDRESS_ENV, OsString::from("127.0.0.1:51051")),
            (STATE_DIRECTORY_ENV, OsString::from("/tmp/gateway-state")),
            (WORKER_COUNT_ENV, OsString::from("3")),
        ]);
        let config = GatewaydConfig::from_lookup(|key| values.get(key).cloned()).unwrap();

        assert_eq!(config.listen_address().port(), 51051);
        assert_eq!(config.state_directory(), Path::new("/tmp/gateway-state"));
        assert_eq!(config.worker_count().get(), 3);
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
}
