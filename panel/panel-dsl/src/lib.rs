#![forbid(unsafe_code)]

//! The NGINX-style syntax of the configuration language: a lexer and parser
//! that keep comments and exact byte ranges, recover from errors to report
//! all of them, and a canonical formatter.
//!
//! The syntax carries no meaning; which directives exist and what their
//! arguments mean is decided by the language crate built on it.

mod ast;
mod format;
mod lexer;
mod parse;
mod span;

pub use ast::{Argument, Block, Body, Comment, Directive, Document, LuaBlock, Trivia};
pub use format::{format, format_directive, quote};
pub use parse::{parse, Parsed};
pub use span::{LineIndex, Span};

/// Diagnostic code of every syntax error.
pub const SYNTAX_ERROR: &str = "DSL_SYNTAX";

impl Argument {
    /// An argument built rather than parsed, with an empty span.
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            quote: None,
            span: Span::default(),
        }
    }
}

impl Directive {
    /// A `name args;` directive built rather than parsed.
    pub fn simple<I, A>(name: &str, args: I) -> Self
    where
        I: IntoIterator<Item = A>,
        A: Into<String>,
    {
        Self {
            leading: Vec::new(),
            name: Argument::new(name),
            args: args.into_iter().map(Argument::new).collect(),
            body: Body::Semicolon,
            comment: None,
            span: Span::default(),
        }
    }

    /// A `name args { ... }` directive built rather than parsed.
    pub fn with_block<I, A>(name: &str, args: I, directives: Vec<Directive>) -> Self
    where
        I: IntoIterator<Item = A>,
        A: Into<String>,
    {
        Self {
            body: Body::Block(Block {
                open: Span::default(),
                directives,
                trailing: Vec::new(),
                close: None,
            }),
            ..Self::simple(name, args)
        }
    }
}
