#![forbid(unsafe_code)]

//! Public HTTP adapter for the control-plane application ports.
//!
//! HTTP contracts, rejection mapping, request metadata and routing live in
//! separate modules. Concrete compilers, persistence, identity and gateway
//! transports are injected through application-owned ports.

mod access;
mod acme;
mod admission;
mod alerts;
mod approvals;
mod audit;
mod certificates;
mod compose;
mod conditional;
mod config;
mod configuration;
mod container_sites;
mod containers;
mod contract;
mod engine_resources;
mod error;
mod error_contract;
mod gateway_runtime;
mod grants;
mod host;
mod host_agent;
mod identity;
mod images;
mod language;
mod logs;
mod middleware;
mod openapi;
mod request_context;
mod router;
mod routes;
mod sign_in;
mod site_files;
mod state;
mod tail;
mod time;
mod tls_checks;
mod traffic;
mod workload;

pub use access::{AccessAudit, AccessSettings, Refusal};
pub use approvals::Rejection;
pub use config::ApiConfig;
pub use configuration::{
    ApplyBypass, ApplyRequest, ApplyResponse, CloneSiteRequest, DomainCheckRequest, DraftResponse,
    ImportResponse, RouteOrderRequest,
};
pub use contract::*;
pub use gateway_runtime::{
    DataPlaneListenerResponse, DataPlaneResponse, EndpointHealthResponse, ShutdownResponse,
    UpstreamHealthReportResponse, UpstreamHealthResponse, WorkerCountRequest,
};
pub use grants::{GrantConditionsBody, GrantScopeBody, GrantView, NewGrant};
pub use identity::{
    AccountPatch, AccountView, CreatedToken, CredentialKind, CurrentSession, EndedSessions,
    LoginRequest, LoginResponse, NewAccount, NewRole, NewToken, PasswordChange, PasswordReset,
    PermissionView, RoleChange, RoleView, SessionTransport, SessionView, SetupRequest, SetupStatus,
    TokenView,
};
pub use openapi::ApiDoc;
pub use router::{router, router_with_config};
pub use sign_in::{
    ClaimNamesBody, GroupRoleBody, IdentityProviderInput, IdentityProviderResponse,
    PasswordSignInMode, SignInOptionResponse, SignInPolicy,
};
pub use state::ApiState;
pub use workload::{
    WorkloadExchange, WorkloadIdentityInput, WorkloadIdentityResponse, WorkloadSession,
};

#[cfg(test)]
mod tests;
