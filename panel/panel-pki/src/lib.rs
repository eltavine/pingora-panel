#![forbid(unsafe_code)]

//! The internal certificate authority that gives every service a workload
//! identity for mutual TLS.
//!
//! The authority issues short-lived ECDSA P-256 certificates following the
//! RFC 5280 profile: each names its service by the DNS identity
//! `<service>.<trust domain>`, used for authentication, and by the SPIFFE ID
//! `spiffe://<trust domain>/service/<service>`. Credentials are renewed once
//! two thirds of their lifetime has passed and written with one atomic
//! rename, so a service never reads a key and certificate from different
//! issuances.

mod authority;
mod files;
mod identity;
mod pem;

pub use authority::{CertificateAuthority, IssuedCredentials, DEFAULT_AUTHORITY_VALIDITY};
pub use files::{CredentialFiles, Validity, IDENTITY_FILE, TRUST_FILE};
pub use identity::{TrustDomain, WorkloadIdentity, DEFAULT_TRUST_DOMAIN};
