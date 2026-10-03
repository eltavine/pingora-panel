#![forbid(unsafe_code)]

//! Server certificates and their private keys.
//!
//! [`accept`] parses and checks a chain and key the way the gateway loads
//! them, [`describe`] reads a chain alone, [`CertificateDetails`] describes
//! what a certificate says and which
//! hosts it covers, and [`self_signed`] generates a certificate. Nothing here
//! performs I/O or keeps key material beyond the values it returns.

mod details;
mod generate;
mod intake;
mod inventory;
mod pem;

pub use details::{CertificateDetails, CertificateStatus, KeyAlgorithm, EXPIRING_WITHIN};
pub use generate::{self_signed, MAX_SELF_SIGNED_DAYS};
pub use intake::{accept, describe, Accepted};
pub use inventory::{Certificate, CertificateId, CertificateSource};
