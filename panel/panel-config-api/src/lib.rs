#![forbid(unsafe_code)]

//! The configuration API as one contract for its callers and its service:
//! the port, and every read and change it serves as a typed operation, so
//! that the compiler checks what both sides agree on.
//!
//! The operation enums are exhaustive on purpose: a new operation is a new
//! contract version that every service has to handle.

/// Declares operations, each serialized under the name audit records and
/// change receipts know it by.
macro_rules! operations {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $(
                $(#[$variant_meta:meta])*
                $operation:literal => $variant:ident
                $({ $($(#[$field_meta:meta])* $field:ident: $type:ty),* $(,)? })?
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        #[serde(tag = "operation", content = "parameters", deny_unknown_fields)]
        pub enum $name {
            $(
                $(#[$variant_meta])*
                #[serde(rename = $operation)]
                $variant $({ $($(#[$field_meta])* $field: $type),* })?,
            )*
        }

        impl $name {
            /// The operation's name.
            pub fn operation(&self) -> &'static str {
                match self {
                    $(Self::$variant { .. } => $operation,)*
                }
            }
        }
    };
}

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
