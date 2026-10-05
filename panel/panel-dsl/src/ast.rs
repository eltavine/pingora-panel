//! The syntax tree: directives with their arguments, blocks and the comments
//! around them.

use crate::span::Span;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Document {
    pub directives: Vec<Directive>,
    /// Comments and blank lines after the last directive.
    pub trailing: Vec<Trivia>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Directive {
    /// Comments and blank lines between the previous directive and this one.
    pub leading: Vec<Trivia>,
    pub name: Argument,
    pub args: Vec<Argument>,
    pub body: Body,
    /// A comment on the same line as the `;` or `{`.
    pub comment: Option<Comment>,
    /// From the name through the `;` or the closing `}`.
    pub span: Span,
}

impl Directive {
    pub fn block(&self) -> Option<&Block> {
        match &self.body {
            Body::Block(block) => Some(block),
            Body::Semicolon | Body::Missing | Body::Lua(_) => None,
        }
    }

    /// The code of a `*_by_lua_block` directive.
    pub fn lua(&self) -> Option<&LuaBlock> {
        match &self.body {
            Body::Lua(lua) => Some(lua),
            _ => None,
        }
    }

    /// The span including the comments that lead into it, so removing it
    /// also removes what describes it.
    pub fn span_with_leading(&self) -> Span {
        self.leading
            .iter()
            .filter_map(|trivia| match trivia {
                Trivia::Comment(comment) => Some(comment.span),
                Trivia::BlankLine => None,
            })
            .fold(self.span, Span::join)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Body {
    Semicolon,
    Block(Block),
    /// The Lua code of a `*_by_lua_block` directive.
    Lua(LuaBlock),
    /// Neither `;` nor a block followed; reported as an error.
    Missing,
}

/// Lua code between braces, kept as it is written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LuaBlock {
    /// The text between the braces.
    pub code: String,
    /// From the `{` through the `}`.
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Block {
    pub open: Span,
    pub directives: Vec<Directive>,
    /// Comments and blank lines before the closing `}`.
    pub trailing: Vec<Trivia>,
    /// The closing `}`, absent when the file ends first.
    pub close: Option<Span>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Argument {
    /// The value after unescaping.
    pub value: String,
    /// The quote it was written with, if any.
    pub quote: Option<char>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Comment {
    /// The text after `#`.
    pub text: String,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Trivia {
    Comment(Comment),
    BlankLine,
}
