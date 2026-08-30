//! Stable gateway ports and an in-memory contract implementation.

mod events;
pub mod fake;
pub mod ports;
mod validation;

pub use events::*;
pub use fake::FakeGatewayEngine;
pub use ports::*;
pub use validation::validate_engine_ir;
