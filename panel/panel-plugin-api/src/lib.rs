#![forbid(unsafe_code)]

//! The plugins API as one contract for its callers and its service (ADR
//! 0044): the port to the plugins module, the views it answers with, and
//! every read and change it serves as a typed operation, so that the
//! compiler checks what both sides agree on.
//!
//! The operation enums are exhaustive on purpose: a new operation is a new
//! contract version that the service has to handle.

mod operations;
mod port;
mod views;

pub use operations::{PluginCommand, PluginQuery};
pub use port::{PluginChange, PluginOutput, PluginsPort, Secret};
pub use views::{
    HealthStatus, NewTrustedKey, PluginHealth, PluginLimits, PluginList, PluginState, PluginView,
    SecretView, TrustedKeyView, VersionView,
};
