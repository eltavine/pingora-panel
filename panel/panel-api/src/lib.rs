#![forbid(unsafe_code)]

//! Public HTTP adapter for the control-plane application ports.
//!
//! HTTP contracts, rejection mapping, request metadata and routing live in
//! separate modules. Concrete compilers, persistence, identity and gateway
//! transports are injected through application-owned ports.

mod access;
mod admission;
mod audit;
mod conditional;
mod config;
mod configuration;
mod contract;
mod error;
mod error_contract;
mod gateway_runtime;
mod identity;
mod language;
mod middleware;
mod openapi;
mod request_context;
mod router;
mod routes;
mod state;

pub use access::AccessSettings;
pub use config::ApiConfig;
pub use configuration::{
    ApplyRequest, ApplyResponse, CloneSiteRequest, DomainCheckRequest, DraftResponse,
    ImportResponse, RouteOrderRequest,
};
pub use contract::*;
pub use gateway_runtime::{
    DataPlaneListenerResponse, DataPlaneResponse, EndpointHealthResponse, ShutdownResponse,
    UpstreamHealthReportResponse, UpstreamHealthResponse, WorkerCountRequest,
};
pub use identity::{
    AccountPatch, AccountView, CreatedToken, CredentialKind, CurrentSession, LoginRequest,
    LoginResponse, NewAccount, NewToken, PasswordChange, PasswordReset, PermissionView, RoleView,
    SessionTransport, SessionView, SetupRequest, SetupStatus, TokenView,
};
pub use openapi::ApiDoc;
pub use router::{router, router_with_config};
pub use state::ApiState;

#[cfg(test)]
mod tests;
