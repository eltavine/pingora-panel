#![forbid(unsafe_code)]

//! Aggregated service health in the IETF "Health Check Response Format for
//! HTTP APIs" (`application/health+json`) shape, independent of transports
//! and of the dependencies being checked.
//!
//! Dependency adapters implement [`HealthCheck`]; a composition root
//! registers them with the [`Impact`] a failure has on the service, and a
//! [`HealthMonitor`] evaluates them periodically so that health endpoints and
//! write admission read a published [`HealthReport`] instead of probing
//! dependencies per request.

mod check;
mod monitor;
mod registry;
mod report;

pub use check::{CheckOutcome, ComponentType, HealthCheck, HealthStatus, Impact};
pub use monitor::{HealthMonitor, HealthWatch};
pub use registry::{HealthRegistry, ServiceIdentity, DEFAULT_CHECK_TIMEOUT};
pub use report::{ComponentHealth, HealthReport, ServiceMode};

/// Media type of health documents.
pub const HEALTH_MEDIA_TYPE: &str = "application/health+json";
