#![forbid(unsafe_code)]

//! Stable gateway ports and an in-memory contract implementation.

mod conditions;
mod events;
pub mod fake;
mod logging;
pub mod ports;
mod traffic;
mod validation;

pub use conditions::{token, MOST_CONDITIONS, MOST_CONDITION_DEPTH};
pub use events::*;
pub use fake::FakeGatewayEngine;
pub use ports::*;
pub use traffic::{route_regex_error, ROUTE_REGEX_SIZE_LIMIT};
pub use validation::validate_engine_ir;
