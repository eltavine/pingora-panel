#![forbid(unsafe_code)]

//! Public HTTP adapter for the control-plane application ports.
//!
//! HTTP contracts, rejection mapping, request metadata and routing live in
//! separate modules. Concrete compilers, persistence, identity and gateway
//! transports are injected through application-owned ports.

mod config;
mod contract;
mod error;
mod error_contract;
mod middleware;
mod openapi;
mod request_context;
mod router;
mod routes;
mod state;

pub use config::ApiConfig;
pub use contract::*;
pub use openapi::ApiDoc;
pub use router::{router, router_with_config};
pub use state::ApiState;

#[cfg(test)]
mod tests;
