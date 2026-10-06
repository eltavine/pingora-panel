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
mod capture;
mod exchange;
mod lint;
mod ocsp;
mod program;
mod runtime;
mod shared;
mod ssl;
mod stream;
mod timer;
mod tls;
mod upstreams;
mod vm;
mod worker;
mod x509;

pub use capture::{Capture, Captured, Share, MOST_CAPTURES, MOST_DEPTH};
pub use exchange::{
    Balancer, Changes, Chunk, Connection, Exchange, Failure, FailureKind, Limits, LogEntry,
    LogLevel, Outcome, Peer, PeerTimeouts, Permissions, Phase, Request, Response, Sockets,
};
pub use lint::{lint, Finding, FindingKind, Lint, Role};
pub use program::{compile, Diagnostic, HandlerId, Program, ProgramBuilder, SharedDict, Source};
pub use runtime::{Handler, Host, NoHost, Runtime, Scripts, Settings};
pub use shared::SharedStore;
pub use ssl::{ClientAuth, Handshake, UpstreamTls};
pub use stream::Output;
pub use timer::TimerRun;
pub use tls::{TlsId, TlsTerms};
pub use upstreams::{NoUpstreams, UpstreamPeer, UpstreamServer, Upstreams};

/// `ngx` functions lua-nginx-module has and this runtime does not provide.
pub fn unavailable_functions() -> &'static [&'static str] {
    &api::UNAVAILABLE
}

/// Modules `require` finds without a file in the configuration.
pub fn built_in_modules() -> &'static [&'static str] {
    static NAMES: std::sync::LazyLock<Vec<&'static str>> = std::sync::LazyLock::new(|| {
        api::BUILT_IN_MODULES
            .iter()
            .map(|(name, _)| *name)
            .collect()
    });
    &NAMES
}

/// The built-in modules, each with the library it stands in for: an
/// OpenResty library such as `lua-resty-redis`, LuaJIT's extensions
/// (`luajit`) or the gateway's own (`panel`).
pub fn module_catalog() -> &'static [(&'static str, &'static str)] {
    &api::BUILT_IN_MODULES
}

/// The OpenResty modules scripts may not load, and why.
pub fn refused_modules() -> &'static [(&'static str, &'static str)] {
    &api::REFUSED_MODULES
}
