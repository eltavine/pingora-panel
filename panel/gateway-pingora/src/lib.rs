#![forbid(unsafe_code)]

//! Pingora 0.9 adapter for the stable Panel engine port.
//!
//! The implementation modules are private so upstream Pingora types cannot
//! become part of this crate's public contract.

mod acme;
mod adapter;
mod certificates;
mod dataplane;
mod forwarding;
mod hosts;
mod hsts;
mod listeners;
mod path;
mod proxy;
mod responses;
mod routing;
mod secrets;
mod security;
mod static_files;
mod template;
mod upstream;

pub use acme::ChallengeDirectory;
pub use adapter::{AdapterOptions, PingoraGatewayAdapter, PreparedPingoraSnapshot};
pub use dataplane::{DataPlane, DataPlaneOptions, DataPlaneStatus, ListenerStatus};
pub use secrets::{DirectorySecrets, NoSecrets, SecretSource};
pub use upstream::{EndpointHealth, PoolHealth};

pub const ADAPTER_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const PINGORA_PACKAGE_VERSION: &str = "0.9.0";
