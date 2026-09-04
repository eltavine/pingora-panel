#![forbid(unsafe_code)]

//! Pingora 0.8 adapter for the stable Panel engine port.
//!
//! The implementation module is private so upstream Pingora types cannot become part
//! of this crate's public contract.

mod adapter;

pub use adapter::{PingoraGatewayAdapter, PreparedPingoraSnapshot};

pub const ADAPTER_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const PINGORA_PACKAGE_VERSION: &str = "0.8.0";
