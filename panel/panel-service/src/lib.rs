#![forbid(unsafe_code)]

//! Building blocks shared by service processes: operational HTTP endpoints,
//! standard gRPC health, service description and protocol negotiation,
//! process signals and logging.
//!
//! Each block is independent, so a composition root uses only what its
//! process exposes and keeps its own configuration and wiring.

mod grpc;
mod ops;
mod probe;
mod process;

pub use grpc::{describe_peer, negotiate_with_peer, publish_grpc_health, ServiceInfoService};
pub use ops::{health_response, ops_router, LIVENESS_PATH, READINESS_PATH};
pub use probe::probe_http;
pub use process::{init_logging, shutdown_signal};
