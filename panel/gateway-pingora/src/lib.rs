#![forbid(unsafe_code)]

//! Pingora 0.9 adapter for the stable Panel engine port.
//!
//! The implementation modules are private so upstream Pingora types cannot
//! become part of this crate's public contract.

mod access_log;
mod acme;
mod adapter;
mod certificates;
mod dataplane;
mod file_checks;
mod forwarding;
mod head_deadline;
mod hosts;
mod hsts;
mod http_policy;
mod listeners;
mod log_files;
mod lua;
mod proxy;
mod request_identity;
mod resilience;
mod responses;
mod routing;
mod secrets;
mod security;
mod static_files;
mod subrequests;
mod telemetry;
mod template;
mod upstream;

pub use acme::ChallengeDirectory;
pub use adapter::{AdapterOptions, PingoraGatewayAdapter, PreparedPingoraSnapshot};
pub use dataplane::{DataPlane, DataPlaneOptions, DataPlaneStatus, ListenerStatus};
pub use file_checks::{
    EscapingLink, FileChecks, PrivateKeyCheck, StaticRootCheck, MAX_STATIC_ENTRIES,
};
pub use log_files::Logs;
pub use secrets::{DirectorySecrets, NoSecrets, SecretPermissions, SecretSource};
pub use telemetry::{register_configuration, GatewayMetrics};
pub use upstream::{EndpointHealth, PoolHealth};

pub const ADAPTER_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const PINGORA_PACKAGE_VERSION: &str = "0.9.0";
