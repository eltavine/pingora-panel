#![forbid(unsafe_code)]

//! Stable gateway ports and an in-memory contract implementation.

mod events;
pub mod fake;
pub mod ports;
mod traffic;
mod validation;

pub use events::*;
pub use fake::FakeGatewayEngine;
pub use ports::*;
pub use traffic::{route_regex_error, ROUTE_REGEX_SIZE_LIMIT};
pub use validation::validate_engine_ir;
