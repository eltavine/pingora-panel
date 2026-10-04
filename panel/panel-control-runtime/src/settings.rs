use crate::InProcessHub;
use panel_errors::Result;
use panel_pki::TrustDomain;
use panel_service::{require_loopback, Environment};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};

pub const OPS_ADDRESS_ENV: &str = "PINGORA_PANEL_OPS_ADDR";
pub const GRPC_ADDRESS_ENV: &str = "PINGORA_PANEL_GRPC_ADDR";
/// Where the control plane keeps its modules' SQLite files.
pub const DATA_DIR_ENV: &str = "PINGORA_PANEL_DATA_DIR";
pub const NATS_URL_ENV: &str = "PINGORA_PANEL_NATS_URL";
pub const HEALTH_INTERVAL_MS_ENV: &str = "PINGORA_PANEL_HEALTH_INTERVAL_MS";
/// Directory with the service's `identity.pem` and `trust.pem`; enables
/// mutual TLS on internal gRPC.
pub const TLS_DIR_ENV: &str = "PINGORA_PANEL_TLS_DIR";
pub const TRUST_DOMAIN_ENV: &str = "PINGORA_PANEL_TRUST_DOMAIN";

const DEFAULT_NATS_URL: &str = "nats://127.0.0.1:4222";
const DEFAULT_DATA_DIR: &str = "/var/lib/pingora-panel/control";
const DEFAULT_HEALTH_INTERVAL: Duration = Duration::from_secs(5);

/// Listener addresses a service uses when its environment names none.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DefaultAddresses {
    pub ops: SocketAddr,
    pub grpc: SocketAddr,
}

/// Where a service's mutual TLS credentials are and the trust domain they
/// belong to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TlsSettings {
    pub directory: PathBuf,
    pub trust_domain: TrustDomain,
}

impl TlsSettings {
    /// The TLS settings in `env`, if a credential directory is configured.
    pub fn read(env: &mut Environment<'_>) -> Result<Option<Self>> {
        let Some(directory) = env.string(TLS_DIR_ENV)? else {
            return Ok(None);
        };
        Ok(Some(Self {
            directory: PathBuf::from(directory),
            trust_domain: env
                .string(TRUST_DOMAIN_ENV)?
                .map(TrustDomain::new)
                .transpose()?
                .unwrap_or_default(),
        }))
    }
}

/// Settings shared by every control-plane process.
#[derive(Clone)]
pub struct ProcessSettings {
    ops_address: SocketAddr,
    grpc_address: SocketAddr,
    data_directory: PathBuf,
    nats_url: String,
    health_interval: Duration,
    tls: Option<TlsSettings>,
    hub: Option<InProcessHub>,
}

impl ProcessSettings {
    pub fn from_environment(defaults: DefaultAddresses) -> Result<Self> {
        Self::read(&mut Environment::process(), defaults)
    }

    /// Reads the settings. The operational listener stays on loopback; the
    /// gRPC listener may bind any address only with mutual TLS.
    pub fn read(env: &mut Environment<'_>, defaults: DefaultAddresses) -> Result<Self> {
        let tls = TlsSettings::read(env)?;
        let grpc_address = env.socket_addr(GRPC_ADDRESS_ENV, defaults.grpc)?;
        Ok(Self {
            ops_address: require_loopback(
                OPS_ADDRESS_ENV,
                env.socket_addr(OPS_ADDRESS_ENV, defaults.ops)?,
            )?,
            grpc_address: if tls.is_some() {
                grpc_address
            } else {
                require_loopback(GRPC_ADDRESS_ENV, grpc_address)?
            },
            tls,
            data_directory: PathBuf::from(
                env.string(DATA_DIR_ENV)?
                    .unwrap_or_else(|| DEFAULT_DATA_DIR.into()),
            ),
            nats_url: env
                .string(NATS_URL_ENV)?
                .unwrap_or_else(|| DEFAULT_NATS_URL.into()),
            health_interval: env.millis(HEALTH_INTERVAL_MS_ENV, DEFAULT_HEALTH_INTERVAL)?,
            hub: None,
        })
    }

    pub fn ops_address(&self) -> SocketAddr {
        self.ops_address
    }

    pub fn grpc_address(&self) -> SocketAddr {
        self.grpc_address
    }

    pub fn data_directory(&self) -> &Path {
        &self.data_directory
    }

    pub fn nats_url(&self) -> &str {
        &self.nats_url
    }

    pub fn health_interval(&self) -> Duration {
        self.health_interval
    }

    pub fn tls(&self) -> Option<&TlsSettings> {
        self.tls.as_ref()
    }

    pub fn hub(&self) -> Option<&InProcessHub> {
        self.hub.as_ref()
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

    pub fn with_data_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.data_directory = directory.into();
        self
    }

    /// Mutual TLS with the credentials `tls` names, or none.
    pub fn with_tls(mut self, tls: Option<TlsSettings>) -> Self {
        self.tls = tls;
        self
    }

    /// Serves gRPC to the other modules hosted on `hub` instead of on a
    /// network listener, and reaches those modules through it.
    pub fn in_process(mut self, hub: InProcessHub) -> Self {
        self.hub = Some(hub);
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
    fn settings_default_to_loopback_listeners_a_local_broker_and_the_data_directory() {
        let settings = read(&[]).unwrap();
        assert_eq!(settings.ops_address(), defaults().ops);
        assert_eq!(settings.grpc_address(), defaults().grpc);
        assert_eq!(settings.nats_url(), DEFAULT_NATS_URL);
        assert_eq!(settings.health_interval(), DEFAULT_HEALTH_INTERVAL);
        assert_eq!(
            settings.data_directory(),
            Path::new("/var/lib/pingora-panel/control")
        );
        let elsewhere = read(&[(DATA_DIR_ENV, "/srv/panel")]).unwrap();
        assert_eq!(elsewhere.data_directory(), Path::new("/srv/panel"));
    }

    #[test]
    fn listeners_stay_on_loopback() {
        assert!(read(&[(GRPC_ADDRESS_ENV, "0.0.0.0:50061")]).is_err());
        let settings = read(&[(OPS_ADDRESS_ENV, "[::1]:9999")]).unwrap();
        assert_eq!(settings.ops_address().port(), 9999);
    }

    #[test]
    fn mutual_tls_lets_the_grpc_listener_leave_loopback() {
        let settings = read(&[
            (GRPC_ADDRESS_ENV, "0.0.0.0:50061"),
            (TLS_DIR_ENV, "/run/pingora-panel/tls"),
        ])
        .unwrap();
        let tls = settings.tls().unwrap();
        assert_eq!(tls.trust_domain.as_str(), "pingora-panel.internal");
        assert!(
            read(&[
                (OPS_ADDRESS_ENV, "0.0.0.0:9181"),
                (TLS_DIR_ENV, "/run/pingora-panel/tls"),
            ])
            .is_err(),
            "operational endpoints stay on loopback"
        );
    }
}
