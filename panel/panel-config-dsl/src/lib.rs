#![forbid(unsafe_code)]

//! The configuration language: what the NGINX-style syntax of `panel-dsl`
//! means. It reads a configuration's files into the configuration model,
//! reporting each problem at its line and column, and writes the model back
//! as canonical text.

mod checks;
mod edit;
mod lower;
pub mod plan;
pub mod print;
pub mod schema;
mod source;
pub mod values;
pub mod variables;

pub use edit::{format_files, reconcile, write_identifiers};
pub use lower::{lower, Insertion, LowerOptions, Lowered, Origin};
pub use print::print;
pub use source::{Sources, ENTRY};

/// The language version this release reads and writes.
pub const LANGUAGE_VERSION: u32 = 1;

/// Diagnostic codes, by the stage that reports them.
pub mod codes {
    /// The file does not declare a language version this release reads.
    pub const VERSION: &str = "DSL_VERSION";
    /// No such directive exists.
    pub const UNKNOWN_DIRECTIVE: &str = "DSL_UNKNOWN_DIRECTIVE";
    /// The directive exists but not in this block.
    pub const CONTEXT: &str = "DSL_CONTEXT";
    /// Wrong number or kind of arguments, parameters or block.
    pub const ARGUMENTS: &str = "DSL_ARGUMENTS";
    /// A value is not of the type the directive expects.
    pub const TYPE: &str = "DSL_TYPE";
    /// A directive is deprecated; a warning.
    pub const DEPRECATED: &str = "DSL_DEPRECATED";
    /// An include names nothing, leaves the configuration or forms a cycle.
    pub const INCLUDE: &str = "DSL_INCLUDE";
    /// A variable is undefined or used where it cannot be.
    pub const VARIABLE: &str = "DSL_VARIABLE";
    /// A name is defined, or a directive set, twice.
    pub const DUPLICATE: &str = "DSL_DUPLICATE";
    /// A name refers to nothing.
    pub const REFERENCE: &str = "DSL_REFERENCE";
    /// A route is evaluated after another that takes all its requests; a warning.
    pub const SHADOWED_ROUTE: &str = "DSL_SHADOWED_ROUTE";
    /// A route can never match; a warning.
    pub const UNREACHABLE_ROUTE: &str = "DSL_UNREACHABLE_ROUTE";
}
