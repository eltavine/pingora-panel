#![forbid(unsafe_code)]

//! Server certificates and their private keys.
//!
//! [`accept`] parses and checks a chain and key the way the gateway loads
//! them, [`describe`] reads a chain alone, [`CertificateDetails`] describes
//! what a certificate says and which
//! hosts it covers, and [`self_signed`] generates a certificate.
//! [`signing_request`], [`renewal_identifier`] and [`renewal_time`] serve
//! certificates a CA issues. Nothing here performs I/O or keeps key material
//! beyond the values it returns.

mod details;
mod generate;
mod intake;
mod inventory;
mod issuance;
mod pem;

pub use details::{CertificateDetails, CertificateStatus, KeyAlgorithm, EXPIRING_WITHIN};
pub use generate::{self_signed, MAX_SELF_SIGNED_DAYS};
pub use intake::{accept, describe, describe_der, Accepted};
pub use inventory::{Certificate, CertificateId, CertificateSource};
pub use issuance::{renewal_identifier, renewal_time, signing_request, SigningRequest};
pub use panel_domain::ACME_CHALLENGE_DIRECTORY;
