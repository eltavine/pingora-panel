use panel_contracts::ops::v1::DirectoryKind;
use panel_environment::Environment;
use panel_errors::{PanelError, Result};
use panel_pki::TrustDomain;
use std::path::PathBuf;

/// The socket the agent serves on.
pub const SOCKET_ENV: &str = "PINGORA_PANEL_OPS_SOCKET";
/// The group the socket belongs to, a numeric ID the agent is a member of.
pub const SOCKET_GROUP_ENV: &str = "PINGORA_PANEL_OPS_SOCKET_GROUP";
/// Comma-separated numeric IDs of the users whose processes may connect.
pub const PEER_USERS_ENV: &str = "PINGORA_PANEL_OPS_PEER_USERS";
/// The agent's credentials; systemd's credentials directory when unset.
pub const TLS_DIR_ENV: &str = "PINGORA_PANEL_TLS_DIR";
pub const TRUST_DOMAIN_ENV: &str = "PINGORA_PANEL_TRUST_DOMAIN";
/// Where systemd places what `LoadCredential=` loads.
pub const CREDENTIALS_DIRECTORY_ENV: &str = "CREDENTIALS_DIRECTORY";
pub const CONFIGURATION_DIR_ENV: &str = "PINGORA_PANEL_OPS_CONFIGURATION_DIR";
pub const LOGS_DIR_ENV: &str = "PINGORA_PANEL_OPS_LOGS_DIR";
pub const CERTIFICATES_DIR_ENV: &str = "PINGORA_PANEL_OPS_CERTIFICATES_DIR";
/// `on` to name the processes listening on TCP ports.
pub const LISTENERS_ENV: &str = "PINGORA_PANEL_OPS_LISTENERS";
/// The gateway's systemd service, the one unit the agent may act on.
pub const GATEWAY_UNIT_ENV: &str = "PINGORA_PANEL_OPS_GATEWAY_UNIT";
/// Comma-separated `engine=socket` pairs, the engine `docker` or `podman`.
pub const ENGINES_ENV: &str = "PINGORA_PANEL_OPS_ENGINES";
/// Where the agent keeps what operators chose; systemd's state directory
/// when unset.
pub const STATE_DIR_ENV: &str = "PINGORA_PANEL_OPS_STATE_DIR";
/// Where systemd places a service's `StateDirectory=`.
pub const STATE_DIRECTORY_ENV: &str = "STATE_DIRECTORY";
/// The Compose project of the panel's own installation, whose containers
/// the agent only ever starts.
pub const INSTALLATION_PROJECT_ENV: &str = "PINGORA_PANEL_OPS_INSTALLATION_PROJECT";
/// The project `compose.yaml` names.
pub const DEFAULT_INSTALLATION_PROJECT: &str = "pingora-panel";
/// The installation's Compose service that runs the gateway, the one
/// container of the installation the agent may stop and restart.
pub const GATEWAY_SERVICE_ENV: &str = "PINGORA_PANEL_OPS_GATEWAY_SERVICE";
/// The service `compose.yaml` names.
pub const DEFAULT_GATEWAY_SERVICE: &str = "gatewayd";
/// The engines a socket may be named for.
pub const ENGINE_IDS: [&str; 2] = ["docker", "podman"];

pub const DEFAULT_SOCKET: &str = "/run/pingora-panel-ops/agent.sock";
/// The user and group every Panel container runs as.
pub const PANEL_USER: u32 = 65532;

/// What the agent serves on, whom it admits and what it may touch.
#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub socket: PathBuf,
    /// The group given the socket, so the processes allowed to connect can;
    /// `None` leaves the agent's own.
    pub socket_group: Option<u32>,
    pub peer_users: Vec<u32>,
    pub credentials: PathBuf,
    pub trust_domain: TrustDomain,
    /// The directories whose sizes the agent reports, by what they hold.
    pub directories: Vec<(DirectoryKind, PathBuf)>,
    pub listeners: bool,
    pub gateway_unit: Option<String>,
    /// The container engines' sockets, by engine.
    pub engines: Vec<(String, PathBuf)>,
    pub state: Option<PathBuf>,
    pub installation_project: String,
    pub gateway_service: String,
}

impl AgentConfig {
    pub fn read(env: &mut Environment<'_>) -> Result<Self> {
        let socket = PathBuf::from(
            env.string(SOCKET_ENV)?
                .unwrap_or_else(|| DEFAULT_SOCKET.to_owned()),
        );
        let socket_group = match env.string(SOCKET_GROUP_ENV)? {
            None => PANEL_USER,
            Some(group) => id(SOCKET_GROUP_ENV, &group)?,
        };
        let peer_users = match env.string(PEER_USERS_ENV)? {
            None => vec![PANEL_USER],
            Some(users) => users
                .split(',')
                .filter(|user| !user.trim().is_empty())
                .map(|user| id(PEER_USERS_ENV, user))
                .collect::<Result<Vec<_>>>()?,
        };
        if peer_users.is_empty() {
            return Err(PanelError::invalid_argument(format!(
                "{PEER_USERS_ENV} names no user"
            )));
        }
        let credentials = match env.string(TLS_DIR_ENV)? {
            Some(directory) => directory,
            None => env.string(CREDENTIALS_DIRECTORY_ENV)?.ok_or_else(|| {
                PanelError::invalid_argument(format!(
                    "set {TLS_DIR_ENV}, or load the credentials with systemd"
                ))
            })?,
        };
        let trust_domain = env
            .string(TRUST_DOMAIN_ENV)?
            .map(TrustDomain::new)
            .transpose()?
            .unwrap_or_default();
        let mut directories = Vec::new();
        for (kind, name) in [
            (DirectoryKind::Configuration, CONFIGURATION_DIR_ENV),
            (DirectoryKind::Logs, LOGS_DIR_ENV),
            (DirectoryKind::Certificates, CERTIFICATES_DIR_ENV),
        ] {
            if let Some(path) = env.string(name)? {
                let path = PathBuf::from(path);
                if !path.is_absolute() {
                    return Err(PanelError::invalid_argument(format!(
                        "{name} must be an absolute path"
                    )));
                }
                directories.push((kind, path));
            }
        }
        let listeners = match env.string(LISTENERS_ENV)?.as_deref() {
            None | Some("off" | "false" | "0" | "no") => false,
            Some("on" | "true" | "1" | "yes") => true,
            Some(_) => {
                return Err(PanelError::invalid_argument(format!(
                    "{LISTENERS_ENV} is on or off"
                )))
            }
        };
        let gateway_unit = env
            .string(GATEWAY_UNIT_ENV)?
            .map(|unit| {
                service_unit(&unit).then_some(unit).ok_or_else(|| {
                    PanelError::invalid_argument(format!(
                        "{GATEWAY_UNIT_ENV} names a systemd service, such as pingora-panel-gatewayd.service"
                    ))
                })
            })
            .transpose()?;
        let mut engines: Vec<(String, PathBuf)> = Vec::new();
        for entry in env
            .string(ENGINES_ENV)?
            .unwrap_or_default()
            .split(',')
            .filter(|entry| !entry.trim().is_empty())
        {
            let (engine, socket) = entry.trim().split_once('=').ok_or_else(|| {
                PanelError::invalid_argument(format!("{ENGINES_ENV} entries are engine=socket"))
            })?;
            let socket = PathBuf::from(socket);
            if !ENGINE_IDS.contains(&engine) || !socket.is_absolute() {
                return Err(PanelError::invalid_argument(format!(
                    "{ENGINES_ENV} names docker or podman with an absolute socket path"
                )));
            }
            if engines.iter().any(|(known, _)| known == engine) {
                return Err(PanelError::invalid_argument(format!(
                    "{ENGINES_ENV} names {engine} twice"
                )));
            }
            engines.push((engine.to_owned(), socket));
        }
        let state = match env.string(STATE_DIR_ENV)? {
            Some(directory) => Some(directory),
            None => env.string(STATE_DIRECTORY_ENV)?,
        }
        .map(PathBuf::from);
        let installation_project = env
            .string(INSTALLATION_PROJECT_ENV)?
            .unwrap_or_else(|| DEFAULT_INSTALLATION_PROJECT.to_owned());
        let gateway_service = env
            .string(GATEWAY_SERVICE_ENV)?
            .unwrap_or_else(|| DEFAULT_GATEWAY_SERVICE.to_owned());
        if !compose_service(&gateway_service) {
            return Err(PanelError::invalid_argument(format!(
                "{GATEWAY_SERVICE_ENV} names a Compose service, such as {DEFAULT_GATEWAY_SERVICE}"
            )));
        }
        Ok(Self {
            socket,
            socket_group: Some(socket_group),
            peer_users,
            credentials: PathBuf::from(credentials),
            trust_domain,
            directories,
            listeners,
            gateway_unit,
            engines,
            state,
            installation_project,
            gateway_service,
        })
    }
}

/// A Compose service's name, by the characters Compose allows.
fn compose_service(name: &str) -> bool {
    name.len() <= 63
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// A systemd service unit's name, by the characters systemd allows.
fn service_unit(name: &str) -> bool {
    name.len() <= 255
        && name.strip_suffix(".service").is_some_and(|stem| {
            !stem.is_empty()
                && stem.chars().all(|c| {
                    c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '.' | '-' | '@' | '\\')
                })
        })
}

fn id(name: &str, value: &str) -> Result<u32> {
    value
        .trim()
        .parse()
        .map_err(|_| PanelError::invalid_argument(format!("{name} takes numeric IDs")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, ffi::OsString};

    fn read(pairs: &[(&str, &str)]) -> Result<AgentConfig> {
        let values: HashMap<String, OsString> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), OsString::from(value)))
            .collect();
        AgentConfig::read(&mut Environment::from_lookup(move |name| {
            values.get(name).cloned()
        }))
    }

    #[test]
    fn defaults_admit_the_panel_user_on_the_standard_socket() {
        let config = read(&[(CREDENTIALS_DIRECTORY_ENV, "/run/credentials/agent")]).unwrap();
        assert_eq!(config.socket, PathBuf::from(DEFAULT_SOCKET));
        assert_eq!(config.socket_group, Some(PANEL_USER));
        assert_eq!(config.peer_users, vec![PANEL_USER]);
        assert_eq!(config.credentials, PathBuf::from("/run/credentials/agent"));
        assert!(config.directories.is_empty());
        assert!(!config.listeners);
        assert!(config.gateway_unit.is_none());
        assert!(config.engines.is_empty());
        assert!(config.state.is_none());
        assert_eq!(config.gateway_service, DEFAULT_GATEWAY_SERVICE);
    }

    #[test]
    fn settings_name_users_groups_and_directories() {
        let config = read(&[
            (TLS_DIR_ENV, "/etc/agent/tls"),
            (SOCKET_GROUP_ENV, "1000"),
            (PEER_USERS_ENV, "1000, 65532"),
            (LOGS_DIR_ENV, "/var/log/pingora-panel"),
            (LISTENERS_ENV, "on"),
            (GATEWAY_UNIT_ENV, "pingora-panel-gatewayd.service"),
            (
                ENGINES_ENV,
                "docker=/run/docker.sock, podman=/run/podman/podman.sock",
            ),
            (STATE_DIRECTORY_ENV, "/var/lib/pingora-panel-ops"),
        ])
        .unwrap();
        assert_eq!(
            config.engines,
            vec![
                ("docker".to_owned(), PathBuf::from("/run/docker.sock")),
                (
                    "podman".to_owned(),
                    PathBuf::from("/run/podman/podman.sock")
                ),
            ]
        );
        assert_eq!(
            config.state,
            Some(PathBuf::from("/var/lib/pingora-panel-ops"))
        );
        assert!(config.listeners);
        assert_eq!(
            config.gateway_unit.as_deref(),
            Some("pingora-panel-gatewayd.service")
        );
        assert_eq!(config.socket_group, Some(1000));
        assert_eq!(config.peer_users, vec![1000, 65532]);
        assert_eq!(config.credentials, PathBuf::from("/etc/agent/tls"));
        assert_eq!(
            config.directories,
            vec![(DirectoryKind::Logs, PathBuf::from("/var/log/pingora-panel"))]
        );
    }

    #[test]
    fn unusable_settings_are_refused() {
        assert!(read(&[]).is_err(), "credentials are required");
        for pairs in [
            [(PEER_USERS_ENV, "root")],
            [(PEER_USERS_ENV, " , ")],
            [(SOCKET_GROUP_ENV, "-1")],
            [(LOGS_DIR_ENV, "logs")],
            [(LISTENERS_ENV, "sometimes")],
            [(GATEWAY_UNIT_ENV, "gatewayd")],
            [(GATEWAY_UNIT_ENV, "../sshd.service")],
            [(GATEWAY_UNIT_ENV, ".service")],
            [(ENGINES_ENV, "containerd=/run/containerd.sock")],
            [(ENGINES_ENV, "docker=docker.sock")],
            [(ENGINES_ENV, "docker=/a.sock,docker=/b.sock")],
            [(GATEWAY_SERVICE_ENV, "-gateway")],
            [(GATEWAY_SERVICE_ENV, "gateway/1")],
        ] {
            let mut pairs = pairs.to_vec();
            pairs.push((TLS_DIR_ENV, "/etc/agent/tls"));
            assert!(read(&pairs).is_err(), "{pairs:?}");
        }
    }
}
