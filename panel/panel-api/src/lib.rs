#![forbid(unsafe_code)]

//! Public HTTP adapter for the control-plane application ports.
//!
//! HTTP contracts, rejection mapping, request metadata and routing live in
//! separate modules. Concrete compilers, persistence, identity and gateway
//! transports are injected through application-owned ports.

mod contract;
mod error;
mod request_context;
mod routes;

pub use contract::*;
pub use routes::{router, router_with_config, ApiConfig, ApiDoc, ApiState};

#[cfg(test)]
mod tests;
