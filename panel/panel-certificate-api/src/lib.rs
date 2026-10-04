#![forbid(unsafe_code)]

//! The certificate API as one contract for its callers and its service: the
//! port to the certificate inventory, ACME accounts, automatic certificates
//! and DNS providers, and every read and change it serves as a typed
//! operation, so that the compiler checks what both sides agree on.
//!
//! The operation enums are exhaustive on purpose: a new operation is a new
//! contract version that the service has to handle.

mod inputs;
mod operations;
mod port;
mod secret;

pub use inputs::{
    AccountId, Challenge, DnsProviderChange, DnsProviderKind, ExternalAccount, NewAccount,
    NewAutomaticCertificate, NewDnsProvider, Rfc2136Config, TsigAlgorithm,
};
pub use operations::{CertificateCommand, CertificateQuery};
pub use port::{CertificateChange, CertificateOutput, CertificatePort};
pub use secret::Secret;
