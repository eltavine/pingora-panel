#![forbid(unsafe_code)]

//! The configuration API as one contract for its callers and its service:
//! the port, and every read and change it serves as a typed operation, so
//! that the compiler checks what both sides agree on.
//!
//! The operation enums are exhaustive on purpose: a new operation is a new
//! contract version that every service has to handle.

mod command;
mod port;
mod query;

pub use command::{
    ApprovalChange, ConfigurationCommand, LanguageChange, ModelChange, RevisionChange,
};
pub use port::{
    ApplyOutcome, ApplyRequest, ApprovalBypass, ConfigurationChange, ConfigurationOutput,
    ConfigurationPort, DraftInfo,
};
pub use query::{
    ApprovalQuery, ConfigurationQuery, DiffBase, Files, LanguageQuery, ModelQuery, RevisionQuery,
};
