#![forbid(unsafe_code)]

//! Building blocks shared by service processes: operational HTTP endpoints,
//! standard gRPC health, service description and protocol negotiation,
//! process settings, signals and logging.
//!
//! Each block is independent, so a composition root uses only what its
//! process exposes and keeps its own configuration and wiring.

mod grpc;
mod grpc_client;
mod metrics;
mod ops;
mod probe;
mod process;
mod trace;

pub use grpc::{describe_peer, negotiate_with_peer, publish_grpc_health, ServiceInfoService};
pub use grpc_client::{
    loopback_channel, request_context, response_error, status_error, GrpcHealthCheck,
};
pub use metrics::{measured, register_readiness};
pub use ops::{health_response, ops_router, LIVENESS_PATH, READINESS_PATH};
pub use panel_environment::{require_loopback, Environment};
pub use probe::probe_http;
pub use process::{init_logging, shutdown_signal};
pub use trace::{propagate_trace, trace_context};
