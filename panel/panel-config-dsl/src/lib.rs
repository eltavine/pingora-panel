#![forbid(unsafe_code)]

//! The configuration language: what the NGINX-style syntax of `panel-dsl`
//! means. It reads a configuration's files into the configuration model,
//! reporting each problem at its line and column, and writes the model back
//! as canonical text.

mod checks;
mod edit;
pub mod explain;
mod inheritance;
mod lower;
pub mod nginx;
pub mod plan;
pub mod print;
pub mod schema;
mod scripts;
mod source;
pub mod syntax;
pub mod values;
pub mod variables;

pub use edit::{format_files, reconcile, write_identifiers};
pub use explain::{explain, Explanation};
pub use lower::{lower, Constant, Insertion, LowerOptions, Lowered, Origin, Written};
pub use nginx::{import_nginx, NginxImport};
pub use print::{print, print_sources};
pub use source::{is_lua, Sources, ENTRY};
pub use syntax::{syntax_tree, SyntaxNode, SyntaxTree};

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
    /// Quotes inside an argument are part of its value; a warning.
    pub const QUOTES: &str = "DSL_QUOTES";
    /// A setting has no effect where it is written: every block that would
    /// take it over sets its own, nothing uses it, or a constant is never
    /// used; a warning.
    pub const NO_EFFECT: &str = "DSL_NO_EFFECT";
    /// A Lua script does not compile, loads a module there is not, uses a
    /// function the gateway does not provide or its phase does not allow, or
    /// writes a global; only a script that does not compile is an error.
    pub const LUA: &str = "DSL_LUA";
    /// A setting works but exposes traffic: passwords asked for over plain
    /// HTTP, every peer trusted as a proxy, or TLS nodes whose certificates
    /// are not verified; a warning.
    pub const EXPOSURE: &str = "DSL_EXPOSURE";
}
