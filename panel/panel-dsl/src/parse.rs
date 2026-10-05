//! Recursive descent over the tokens, recovering at `;` and `}` so one pass
//! reports every syntax error.

use crate::{
    ast::{Argument, Block, Body, Comment, Directive, Document, LuaBlock, Trivia},
    lexer::{lex, Token, TokenKind},
    span::{LineIndex, Span},
    SYNTAX_ERROR,
};
use panel_errors::Diagnostic;

/// A parsed file and every syntax error found in it.
#[derive(Clone, Debug)]
pub struct Parsed {
    pub document: Document,
    pub diagnostics: Vec<Diagnostic>,
}

impl Parsed {
    pub fn is_valid(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

/// Parses `text`, naming `file` in diagnostics.
pub fn parse(file: &str, text: &str) -> Parsed {
    let lexed = lex(text);
    let mut parser = Parser {
        tokens: lexed.tokens,
        position: 0,
        errors: lexed.errors,
    };
    let (directives, trailing, _) = parser.directives(None);
    let index = LineIndex::new(text);
    let mut errors = parser.errors;
    errors.sort_by_key(|(span, _)| span.start);
    let diagnostics = errors
        .into_iter()
        .map(|(span, message)| {
            let mut diagnostic = Diagnostic::error(SYNTAX_ERROR, message);
            diagnostic.source_span = Some(index.describe(file, text, span));
            diagnostic
        })
        .collect();
    Parsed {
        document: Document {
            directives,
            trailing,
        },
        diagnostics,
    }
}

struct Parser {
    tokens: Vec<Token>,
    position: usize,
    errors: Vec<(Span, String)>,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.position)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.position).cloned();
        self.position += usize::from(token.is_some());
        token
    }

    fn error(&mut self, span: Span, message: impl Into<String>) {
        self.errors.push((span, message.into()));
    }

    /// Comments and blank lines up to the next token that is not a comment.
    fn trivia(&mut self) -> Vec<Trivia> {
        let mut trivia = Vec::new();
        while let Some(token) = self.peek() {
            let blank = token.newlines_before > 1 && (!trivia.is_empty() || self.position > 0);
            if blank {
                trivia.push(Trivia::BlankLine);
            }
            if token.kind != TokenKind::Comment {
                break;
            }
            let token = self.next().expect("peeked");
            trivia.push(Trivia::Comment(Comment {
                text: token.value,
                span: token.span,
            }));
        }
        trivia
    }

    /// A comment on the same line as the token just consumed.
    fn same_line_comment(&mut self) -> Option<Comment> {
        let token = self.peek()?;
        if token.kind == TokenKind::Comment && token.newlines_before == 0 {
            let token = self.next().expect("peeked");
            return Some(Comment {
                text: token.value,
                span: token.span,
            });
        }
        None
    }

    /// Directives until the end of the file, or until `}` when inside the
    /// block opened at `open`; returns the closing brace if one was found.
    fn directives(&mut self, open: Option<Span>) -> (Vec<Directive>, Vec<Trivia>, Option<Span>) {
        let mut directives = Vec::new();
        loop {
            let leading = self.trivia();
            let Some(token) = self.peek().cloned() else {
                if let Some(open) = open {
                    self.error(open, "the block is not closed with '}'");
                }
                return (directives, leading, None);
            };
            match token.kind {
                TokenKind::Close => {
                    self.next();
                    if open.is_some() {
                        return (directives, leading, Some(token.span));
                    }
                    self.error(token.span, "unexpected '}' outside a block");
                }
                TokenKind::Semicolon => {
                    self.next();
                    self.error(token.span, "expected a directive name before ';'");
                }
                TokenKind::Open => {
                    self.next();
                    self.error(token.span, "expected a directive name before '{'");
                    let (_, _, close) = self.directives(Some(token.span));
                    if close.is_none() {
                        return (directives, leading, None);
                    }
                }
                TokenKind::Quoted => {
                    self.next();
                    self.error(token.span, "a directive name cannot be quoted");
                    self.recover();
                }
                TokenKind::Lua => {
                    self.next();
                    self.error(token.span, "expected a directive name before the Lua code");
                }
                TokenKind::Word => {
                    if let Some(directive) = self.directive(leading) {
                        directives.push(directive);
                    }
                }
                TokenKind::Comment => unreachable!("trivia consumes comments"),
            }
        }
    }

    /// Skips to just past the next `;` or before the next `}`.
    fn recover(&mut self) {
        while let Some(token) = self.peek() {
            match token.kind {
                TokenKind::Semicolon => {
                    self.next();
                    return;
                }
                TokenKind::Close => return,
                _ => {
                    self.next();
                }
            }
        }
    }

    fn directive(&mut self, leading: Vec<Trivia>) -> Option<Directive> {
        let name = self.next().expect("a word");
        let name = Argument {
            value: name.value,
            quote: None,
            span: name.span,
        };
        let mut args = Vec::new();
        loop {
            let Some(token) = self.peek().cloned() else {
                self.error(name.span, format!("'{}' is not ended with ';'", name.value));
                let span = args
                    .last()
                    .map_or(name.span, |arg: &Argument| name.span.join(arg.span));
                return Some(Directive {
                    leading,
                    name,
                    args,
                    body: Body::Missing,
                    comment: None,
                    span,
                });
            };
            match token.kind {
                TokenKind::Word | TokenKind::Quoted => {
                    self.next();
                    args.push(Argument {
                        value: token.value,
                        quote: token.quote,
                        span: token.span,
                    });
                }
                TokenKind::Comment => {
                    // A comment inside a directive's arguments does not end it.
                    self.next();
                }
                TokenKind::Lua => {
                    self.next();
                    let comment = self.same_line_comment();
                    return Some(Directive {
                        leading,
                        span: name.span.join(token.span),
                        name,
                        args,
                        body: Body::Lua(LuaBlock {
                            code: token.value,
                            span: token.span,
                        }),
                        comment,
                    });
                }
                TokenKind::Semicolon => {
                    self.next();
                    let comment = self.same_line_comment();
                    return Some(Directive {
                        leading,
                        span: name.span.join(token.span),
                        name,
                        args,
                        body: Body::Semicolon,
                        comment,
                    });
                }
                TokenKind::Open => {
                    self.next();
                    let comment = self.same_line_comment();
                    let (directives, trailing, close) = self.directives(Some(token.span));
                    let end = close.unwrap_or_else(|| {
                        directives
                            .last()
                            .map_or(token.span, |directive: &Directive| directive.span)
                    });
                    return Some(Directive {
                        leading,
                        span: name.span.join(end),
                        name,
                        args,
                        body: Body::Block(Block {
                            open: token.span,
                            directives,
                            trailing,
                            close,
                        }),
                        comment,
                    });
                }
                TokenKind::Close => {
                    self.error(name.span, format!("'{}' is not ended with ';'", name.value));
                    let span = args
                        .last()
                        .map_or(name.span, |arg| name.span.join(arg.span));
                    return Some(Directive {
                        leading,
                        name,
                        args,
                        body: Body::Missing,
                        comment: None,
                        span,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# Gateway configuration
language_version 1;

http {
    upstream app {
        server 10.0.0.1:8080 weight=2; # primary
    }

    # The shop
    server shop {
        server_name shop.example \"münchen.example\";
    }
}
";

    #[test]
    fn nested_blocks_keep_comments_and_spans() {
        let parsed = parse("main.conf", SAMPLE);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let document = parsed.document;
        assert_eq!(document.directives.len(), 2);
        let version = &document.directives[0];
        assert_eq!(version.name.value, "language_version");
        assert_eq!(version.args[0].value, "1");
        assert!(
            matches!(&version.leading[0], Trivia::Comment(comment) if comment.text == " Gateway configuration")
        );

        let http = document.directives[1].block().unwrap();
        assert_eq!(document.directives[1].leading, vec![Trivia::BlankLine]);
        let upstream = &http.directives[0];
        let server = &upstream.block().unwrap().directives[0];
        assert_eq!(server.args[1].value, "weight=2");
        assert_eq!(server.comment.as_ref().unwrap().text, " primary");

        let shop = &http.directives[1];
        assert_eq!(shop.leading.len(), 2);
        assert_eq!(
            &SAMPLE[shop.span.range()].lines().next().unwrap(),
            &"server shop {"
        );
        assert!(SAMPLE[shop.span.range()].ends_with('}'));
        assert!(SAMPLE[shop.span_with_leading().range()].starts_with("# The shop"));
        let names = &shop.block().unwrap().directives[0].args;
        assert_eq!(names[1].value, "münchen.example");
        assert_eq!(names[1].quote, Some('"'));
    }

    #[test]
    fn every_error_is_reported_and_parsing_recovers() {
        let parsed = parse(
            "main.conf",
            "http {\n    listen 80\n}\n}\n\"quoted\" name;\nserver {\n    root /srv;\n",
        );
        let messages: Vec<_> = parsed
            .diagnostics
            .iter()
            .map(|diagnostic| {
                (
                    diagnostic.source_span.clone().unwrap(),
                    diagnostic.message.clone(),
                )
            })
            .collect();
        assert_eq!(
            messages,
            vec![
                (
                    "main.conf:2.5-10".into(),
                    "'listen' is not ended with ';'".into()
                ),
                (
                    "main.conf:4.1".into(),
                    "unexpected '}' outside a block".into()
                ),
                (
                    "main.conf:5.1-8".into(),
                    "a directive name cannot be quoted".into()
                ),
                (
                    "main.conf:6.8".into(),
                    "the block is not closed with '}'".into()
                ),
            ]
        );
        let server = parsed.document.directives.last().unwrap();
        assert_eq!(server.name.value, "server");
        assert_eq!(server.block().unwrap().directives[0].name.value, "root");
    }

    #[test]
    fn lua_blocks_become_bodies_kept_as_written() {
        let text = "server s {\n  access_by_lua_block {\n        if ngx.var.arg_x then return ngx.exit(403) end\n  } # check\n  listen   80;\n}\n";
        let parsed = parse("main.conf", text);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let server = parsed.document.directives[0].block().unwrap();
        let access = &server.directives[0];
        let lua = access.lua().expect("a Lua body");
        assert_eq!(
            lua.code,
            "\n        if ngx.var.arg_x then return ngx.exit(403) end\n  "
        );
        assert_eq!(access.comment.as_ref().unwrap().text, " check");
        assert!(access.block().is_none());
        assert_eq!(server.directives[1].name.value, "listen");
        assert_eq!(
            crate::format(&parsed.document),
            "server s {\n    access_by_lua_block {\n        if ngx.var.arg_x then return ngx.exit(403) end\n  } # check\n    listen 80;\n}\n"
        );
    }

    #[test]
    fn diagnostics_carry_the_syntax_code() {
        let parsed = parse("x.conf", "a {");
        assert_eq!(parsed.diagnostics[0].code.as_str(), SYNTAX_ERROR);
    }
}
