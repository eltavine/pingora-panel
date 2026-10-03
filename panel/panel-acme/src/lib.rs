#![forbid(unsafe_code)]

//! ACME (RFC 8555) over `instant-acme` with the ring provider: registering
//! accounts, ordering certificates and asking CAs when to renew them
//! (RFC 9773). Challenges are answered by [`ChallengeSolver`]s: [`Http01`]
//! writes key authorizations for the gateway to serve, and [`Dns01`]
//! publishes TXT records through a [`DnsProvider`].

mod challenges;
mod client;
#[cfg(feature = "test-support")]
pub mod testing;

pub use challenges::{Challenge, ChallengeKind, ChallengeSolver, Dns01, DnsProvider, Http01};
pub use client::{
    AcmeClient, Directory, ExternalAccount, Issued, OrderRequest, Registered, Registration,
    RenewalWindow, LETS_ENCRYPT, LETS_ENCRYPT_STAGING,
};
