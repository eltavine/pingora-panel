use panel_errors::Result;
use panel_postgres::RoleSecret;
use panel_service::{require_loopback, Environment};
use std::{net::SocketAddr, time::Duration};

pub const OPS_ADDRESS_ENV: &str = "PINGORA_PANEL_OPS_ADDR";
pub const GRPC_ADDRESS_ENV: &str = "PINGORA_PANEL_GRPC_ADDR";
pub const DATABASE_URL_ENV: &str = "PINGORA_PANEL_DATABASE_URL";
/// Also read from the file named by `PINGORA_PANEL_DATABASE_PASSWORD_FILE`.
pub const DATABASE_PASSWORD_ENV: &str = "PINGORA_PANEL_DATABASE_PASSWORD";
pub const NATS_URL_ENV: &str = "PINGORA_PANEL_NATS_URL";
pub const HEALTH_INTERVAL_MS_ENV: &str = "PINGORA_PANEL_HEALTH_INTERVAL_MS";

const DEFAULT_NATS_URL: &str = "nats://127.0.0.1:4222";
const DEFAULT_HEALTH_INTERVAL: Duration = Duration::from_secs(5);

/// Listener addresses a service uses when its environment names none.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DefaultAddresses {
    pub ops: SocketAddr,
    pub grpc: SocketAddr,
}

/// Settings shared by every control-plane process.
#[derive(Clone)]
pub struct ProcessSettings {
    ops_address: SocketAddr,
    grpc_address: SocketAddr,
    database_url: String,
    database_password: Option<RoleSecret>,
    nats_url: String,
    health_interval: Duration,
}

impl ProcessSettings {
    pub fn from_environment(defaults: DefaultAddresses) -> Result<Self> {
        Self::read(&mut Environment::process(), defaults)
    }

    /// Reads the settings; plaintext listeners must be loopback addresses.
    pub fn read(env: &mut Environment<'_>, defaults: DefaultAddresses) -> Result<Self> {
        Ok(Self {
            ops_address: require_loopback(
                OPS_ADDRESS_ENV,
                env.socket_addr(OPS_ADDRESS_ENV, defaults.ops)?,
            )?,
            grpc_address: require_loopback(
                GRPC_ADDRESS_ENV,
                env.socket_addr(GRPC_ADDRESS_ENV, defaults.grpc)?,
            )?,
            database_url: env.required(DATABASE_URL_ENV)?,
            database_password: env
                .secret(DATABASE_PASSWORD_ENV)?
                .map(RoleSecret::new)
                .transpose()?,
            nats_url: env
                .string(NATS_URL_ENV)?
                .unwrap_or_else(|| DEFAULT_NATS_URL.into()),
            health_interval: env.millis(HEALTH_INTERVAL_MS_ENV, DEFAULT_HEALTH_INTERVAL)?,
        })
    }

    pub fn ops_address(&self) -> SocketAddr {
        self.ops_address
    }

    pub fn grpc_address(&self) -> SocketAddr {
        self.grpc_address
    }

    pub fn database_url(&self) -> &str {
        &self.database_url
    }

    pub fn database_password(&self) -> Option<&RoleSecret> {
        self.database_password.as_ref()
    }

    pub fn nats_url(&self) -> &str {
        &self.nats_url
    }

    pub fn health_interval(&self) -> Duration {
        self.health_interval
    }

    pub fn with_listeners(mut self, ops: SocketAddr, grpc: SocketAddr) -> Self {
        self.ops_address = ops;
        self.grpc_address = grpc;
        self
    }

    pub fn with_health_interval(mut self, interval: Duration) -> Self {
        self.health_interval = interval;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn defaults() -> DefaultAddresses {
        DefaultAddresses {
            ops: "127.0.0.1:9181".parse().unwrap(),
            grpc: "127.0.0.1:50061".parse().unwrap(),
        }
    }

    fn read(values: &[(&str, &str)]) -> Result<ProcessSettings> {
        let values: HashMap<_, _> = values
            .iter()
            .map(|(key, value)| ((*key).to_owned(), std::ffi::OsString::from(value)))
            .collect();
        ProcessSettings::read(
            &mut Environment::from_lookup(move |name| values.get(name).cloned()),
            defaults(),
        )
    }

    #[test]
    fn settings_default_to_loopback_listeners_and_a_local_broker() {
        let settings = read(&[(DATABASE_URL_ENV, "postgres://config@db/panel")]).unwrap();
        assert_eq!(settings.ops_address(), defaults().ops);
        assert_eq!(settings.grpc_address(), defaults().grpc);
        assert_eq!(settings.nats_url(), DEFAULT_NATS_URL);
        assert!(settings.database_password().is_none());
        assert_eq!(settings.health_interval(), DEFAULT_HEALTH_INTERVAL);
    }

    #[test]
    fn a_database_and_loopback_listeners_are_required() {
        assert!(read(&[]).is_err());
        assert!(read(&[
            (DATABASE_URL_ENV, "postgres://config@db/panel"),
            (GRPC_ADDRESS_ENV, "0.0.0.0:50061"),
        ])
        .is_err());
        let settings = read(&[
            (DATABASE_URL_ENV, "postgres://config@db/panel"),
            (DATABASE_PASSWORD_ENV, "secret"),
            (OPS_ADDRESS_ENV, "[::1]:9999"),
        ])
        .unwrap();
        assert_eq!(settings.database_password().unwrap().expose(), "secret");
        assert_eq!(settings.ops_address().port(), 9999);
    }
}
