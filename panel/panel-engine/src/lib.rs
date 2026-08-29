//! Stable gateway ports and an in-memory contract implementation.

pub mod fake;
pub mod ports;

pub use fake::FakeGatewayEngine;
pub use ports::*;
