#![forbid(unsafe_code)]

//! Lua scripts in the gateway's request phases (ADR 0039): Luau VMs with
//! lua-nginx-module's directives' phases and `ngx` API, bounded in work,
//! time and memory.
//!
//! The crate knows neither Pingora nor the control plane. The gateway fills
//! an [`Exchange`] with the request, runs a [`Handler`] through [`Scripts`]
//! and applies what the run changed; the control plane checks scripts with
//! [`compile`] and [`lint`] and tries them with the same runtime.

mod api;
mod exchange;
mod lint;
mod program;
mod runtime;
mod shared;
mod timer;
mod tls;
mod vm;
mod worker;

pub use exchange::{
    Balancer, Changes, Chunk, Connection, Exchange, Failure, FailureKind, Limits, LogEntry,
    LogLevel, Outcome, Peer, PeerTimeouts, Permissions, Phase, Request, Response, Sockets,
};
pub use lint::{lint, Finding, FindingKind, Lint, Role};
pub use program::{compile, Diagnostic, HandlerId, Program, ProgramBuilder, SharedDict, Source};
pub use runtime::{Handler, Host, NoHost, Runtime, Scripts, Settings};
pub use shared::SharedStore;
pub use timer::TimerRun;
pub use tls::{TlsId, TlsTerms};

/// `ngx` functions lua-nginx-module has and this runtime does not provide.
pub fn unavailable_functions() -> &'static [&'static str] {
    &api::UNAVAILABLE
}

/// Modules `require` finds without a file in the configuration.
pub fn built_in_modules() -> &'static [&'static str] {
    &api::BUILT_IN_MODULES
}
