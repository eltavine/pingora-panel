#![forbid(unsafe_code)]

//! Mutual TLS between services, with credentials that rotate without a
//! restart.
//!
//! Connections use TLS 1.3 only, with ALPN `h2` for gRPC. Servers require
//! a client certificate issued by the installation's authority; clients
//! verify the server's DNS identity in the trust domain rather than the
//! address they dial. Credentials are reloaded when their files change, so
//! new connections use the newest certificate while established ones finish
//! with the one they negotiated. A policy layer authorizes each gRPC service
//! for the peer identities allowed to call it.

mod client;
mod credentials;
mod policy;
mod server;

pub use client::{channel, MtlsConnector};
pub use credentials::TlsCredentials;
pub use policy::{PeerPolicy, PeerPolicyService};
pub use server::{incoming, PeerIdentity, TlsConnection};
