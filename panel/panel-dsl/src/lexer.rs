//! Tokens as NGINX reads them: words, quoted strings, `;`, `{`, `}` and
//! comments, and the Lua code of `*_by_lua_block` directives, read with
//! Lua's lexical rules as lua-nginx-module reads it.

use crate::span::Span;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TokenKind {
    Word,
    Quoted,
    Semicolon,
    Open,
    Close,
    Comment,
    /// The code between the braces of a `*_by_lua_block` directive.
    Lua,
}

#[derive(Clone, Debug)]
pub(crate) struct Token {
    pub kind: TokenKind,
    pub span: Span,
    /// Line breaks between the previous token and this one.
    pub newlines_before: usize,
    /// The unescaped value of a word or string; a comment's text after `#`.
    pub value: String,
    pub quote: Option<char>,
}

#[derive(Debug, Default)]
pub(crate) struct Lexed {
    pub tokens: Vec<Token>,
    pub errors: Vec<(Span, String)>,
}

/// Applies the escapes NGINX recognizes; any other backslash stays, so
/// regular expressions such as `\d` keep their meaning.
fn push_escape(value: &mut String, escaped: char) {
    match escaped {
        '"' | '\'' | '\\' => value.push(escaped),
        't' => value.push('\t'),
        'r' => value.push('\r'),
        'n' => value.push('\n'),
        other => {
            value.push('\\');
            value.push(other);
        }
    }
}

fn is_delimiter(c: char) -> bool {
    c.is_whitespace() || matches!(c, ';' | '{' | '}')
}

/// Directives whose block is Lua code rather than directives.
pub(crate) fn takes_lua(name: &str) -> bool {
    name.ends_with("_by_lua_block")
}

/// The level of the long bracket (`[[`, `[=[`, ...) opening at `at`.
fn long_bracket(bytes: &[u8], at: usize) -> Option<usize> {
    if bytes.get(at) != Some(&b'[') {
        return None;
    }
    let level = bytes[at + 1..]
        .iter()
        .take_while(|&&byte| byte == b'=')
        .count();
    (bytes.get(at + 1 + level) == Some(&b'[')).then_some(level)
}

/// Just past the long bracket of `level` that closes the one opening at `at`.
fn after_long(bytes: &[u8], at: usize, level: usize) -> Option<usize> {
    let mut index = at + level + 2;
    while index < bytes.len() {
        if bytes[index] == b']'
            && bytes[index + 1..]
                .iter()
                .take(level)
                .all(|&byte| byte == b'=')
            && bytes.get(index + 1 + level) == Some(&b']')
        {
            return Some(index + level + 2);
        }
        index += 1;
    }
    None
}

/// The `}` closing the Lua code opened by the `{` at `open`: braces in
/// strings, long brackets and comments do not count.
fn lua_end(text: &str, open: usize) -> Result<usize, (usize, &'static str)> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut index = open;
    while index < bytes.len() {
        match bytes[index] {
            b'{' => {
                depth += 1;
                index += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(index);
                }
                index += 1;
            }
            b'-' if bytes.get(index + 1) == Some(&b'-') => {
                let start = index;
                index += 2;
                match long_bracket(bytes, index) {
                    Some(level) => {
                        index = after_long(bytes, index, level)
                            .ok_or((start, "the Lua comment is not closed"))?;
                    }
                    None => {
                        while index < bytes.len() && bytes[index] != b'\n' {
                            index += 1;
                        }
                    }
                }
            }
            b'[' => match long_bracket(bytes, index) {
                Some(level) => {
                    index = after_long(bytes, index, level)
                        .ok_or((index, "the Lua string is not closed"))?;
                }
                None => index += 1,
            },
            quote @ (b'"' | b'\'' | b'`') => {
                let start = index;
                index += 1;
                loop {
                    match bytes.get(index) {
                        None => return Err((start, "the Lua string is not closed")),
                        Some(b'\\') => index += 2,
                        Some(b'\n') if quote != b'`' => {
                            return Err((start, "the Lua string is not closed on its line"));
                        }
                        Some(&byte) if byte == quote => {
                            index += 1;
                            break;
                        }
                        Some(_) => index += 1,
                    }
                }
            }
            _ => index += 1,
        }
    }
    Err((open, "the Lua block is not closed with '}'"))
}

pub(crate) fn lex(text: &str) -> Lexed {
    let mut lexed = Lexed::default();
    let mut chars = text.char_indices().peekable();
    let mut newlines = 0;
    // Whether the next word names a directive.
    let mut statement = true;
    let push = |lexed: &mut Lexed,
                kind,
                span,
                newlines: &mut usize,
                value: String,
                quote: Option<char>| {
        lexed.tokens.push(Token {
            kind,
            span,
            newlines_before: std::mem::take(newlines),
            value,
            quote,
        });
    };
    while let Some((start, c)) = chars.next() {
        if !c.is_whitespace() && c != '#' {
            let starts = statement;
            statement = matches!(c, ';' | '{' | '}');
            if !starts && c == '{' {
                if let Some(name) = lexed
                    .tokens
                    .iter()
                    .rev()
                    .find(|token| token.kind != TokenKind::Comment)
                {
                    let is_name = name.kind == TokenKind::Word
                        && takes_lua(&name.value)
                        && lexed
                            .tokens
                            .iter()
                            .rev()
                            .filter(|token| token.kind != TokenKind::Comment)
                            .nth(1)
                            .is_none_or(|before| {
                                matches!(
                                    before.kind,
                                    TokenKind::Semicolon
                                        | TokenKind::Open
                                        | TokenKind::Close
                                        | TokenKind::Lua
                                )
                            });
                    if is_name {
                        match lua_end(text, start) {
                            Ok(close) => {
                                push(
                                    &mut lexed,
                                    TokenKind::Lua,
                                    Span::new(start, close + 1),
                                    &mut newlines,
                                    text[start + 1..close].to_owned(),
                                    None,
                                );
                                while chars.peek().is_some_and(|&(offset, _)| offset <= close) {
                                    chars.next();
                                }
                            }
                            Err((at, message)) => {
                                lexed
                                    .errors
                                    .push((Span::new(at, text.len()), message.into()));
                                push(
                                    &mut lexed,
                                    TokenKind::Lua,
                                    Span::new(start, text.len()),
                                    &mut newlines,
                                    text[start + 1..].to_owned(),
                                    None,
                                );
                                while chars.next().is_some() {}
                            }
                        }
                        statement = true;
                        continue;
                    }
                }
            }
        }
        match c {
            '\n' => newlines += 1,
            c if c.is_whitespace() => {}
            ';' => push(
                &mut lexed,
                TokenKind::Semicolon,
                Span::new(start, start + 1),
                &mut newlines,
                String::new(),
                None,
            ),
            '{' => push(
                &mut lexed,
                TokenKind::Open,
                Span::new(start, start + 1),
                &mut newlines,
                String::new(),
                None,
            ),
            '}' => push(
                &mut lexed,
                TokenKind::Close,
                Span::new(start, start + 1),
                &mut newlines,
                String::new(),
                None,
            ),
            '#' => {
                let mut end = start + 1;
                while let Some(&(offset, next)) = chars.peek() {
                    if next == '\n' {
                        break;
                    }
                    end = offset + next.len_utf8();
                    chars.next();
                }
                let comment = text[start + 1..end].trim_end_matches('\r').to_owned();
                push(
                    &mut lexed,
                    TokenKind::Comment,
                    Span::new(start, end),
                    &mut newlines,
                    comment,
                    None,
                );
            }
            '"' | '\'' => {
                let quote = c;
                let mut value = String::new();
                let mut end = None;
                while let Some((offset, next)) = chars.next() {
                    match next {
                        '\\' => match chars.next() {
                            Some((_, escaped)) => push_escape(&mut value, escaped),
                            None => value.push('\\'),
                        },
                        next if next == quote => {
                            end = Some(offset + 1);
                            break;
                        }
                        next => value.push(next),
                    }
                }
                let end = end.unwrap_or_else(|| {
                    lexed.errors.push((
                        Span::new(start, text.len()),
                        "the string is not closed".into(),
                    ));
                    text.len()
                });
                if let Some(&(offset, next)) = chars.peek() {
                    if !is_delimiter(next) {
                        lexed.errors.push((
                            Span::new(offset, offset + next.len_utf8()),
                            format!("expected a space, ';', '{{' or '}}' after the string, found {next:?}"),
                        ));
                    }
                }
                push(
                    &mut lexed,
                    TokenKind::Quoted,
                    Span::new(start, end),
                    &mut newlines,
                    value,
                    Some(quote),
                );
            }
            c => {
                let mut value = String::new();
                let mut end = start + c.len_utf8();
                let mut current = Some((start, c));
                while let Some((offset, c)) = current {
                    end = offset + c.len_utf8();
                    match c {
                        '\\' => match chars.next() {
                            Some((escaped_at, escaped)) => {
                                push_escape(&mut value, escaped);
                                end = escaped_at + escaped.len_utf8();
                            }
                            None => value.push('\\'),
                        },
                        '$' if matches!(chars.peek(), Some((_, '{'))) => {
                            value.push('$');
                            let mut closed = false;
                            for (offset, c) in chars.by_ref() {
                                value.push(c);
                                end = offset + c.len_utf8();
                                if c == '}' {
                                    closed = true;
                                    break;
                                }
                                if c.is_whitespace() || c == ';' {
                                    break;
                                }
                            }
                            if !closed {
                                lexed.errors.push((
                                    Span::new(offset, end),
                                    "the variable is not closed with '}'".into(),
                                ));
                            }
                        }
                        c => value.push(c),
                    }
                    current = match chars.peek() {
                        Some(&(_, next)) if !is_delimiter(next) => chars.next(),
                        _ => None,
                    };
                }
                push(
                    &mut lexed,
                    TokenKind::Word,
                    Span::new(start, end),
                    &mut newlines,
                    value,
                    None,
                );
            }
        }
    }
    lexed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(text: &str) -> Vec<(TokenKind, String)> {
        lex(text)
            .tokens
            .into_iter()
            .map(|token| (token.kind, token.value))
            .collect()
    }

    #[test]
    fn words_strings_and_punctuation() {
        use TokenKind::*;
        assert_eq!(
            values("listen 80;\nserver {\n  name \"a b\"; # note\n}"),
            vec![
                (Word, "listen".into()),
                (Word, "80".into()),
                (Semicolon, String::new()),
                (Word, "server".into()),
                (Open, String::new()),
                (Word, "name".into()),
                (Quoted, "a b".into()),
                (Semicolon, String::new()),
                (Comment, " note".into()),
                (Close, String::new()),
            ]
        );
    }

    #[test]
    fn escapes_follow_nginx() {
        let tokens = values(r#"a "q\"t\\\d\n" 'it\'s' w\;x"#);
        assert_eq!(tokens[1].1, "q\"t\\\\d\n");
        assert_eq!(tokens[2].1, "it's");
        assert_eq!(tokens[3].1, "w\\;x");
        assert_eq!(tokens.len(), 4);
    }

    #[test]
    fn braced_variables_stay_in_their_word() {
        let tokens = values("return 301 https://${host}$uri;");
        assert_eq!(tokens[2], (TokenKind::Word, "https://${host}$uri".into()));
        assert_eq!(tokens[3].0, TokenKind::Semicolon);
    }

    #[test]
    fn hash_inside_a_word_is_not_a_comment() {
        let tokens = values("path /a#b; #c");
        assert_eq!(tokens[1].1, "/a#b");
        assert_eq!(tokens[3], (TokenKind::Comment, "c".into()));
    }

    #[test]
    fn errors_are_reported_with_spans() {
        let lexed = lex("name \"open");
        assert_eq!(lexed.errors.len(), 1);
        assert_eq!(lexed.errors[0].0, Span::new(5, 10));
        let lexed = lex("name \"a\"b;");
        assert!(lexed.errors[0].1.contains("after the string"));
    }

    #[test]
    fn lua_blocks_end_at_their_own_closing_brace() {
        let text = "access_by_lua_block {\n    local t = { a = \"}\", b = [[}]] } -- }\n    --[==[ } ]==]\n    if x then ngx.say('{') end\n}\nlisten 80;";
        let lexed = lex(text);
        assert!(lexed.errors.is_empty(), "{:?}", lexed.errors);
        let kinds: Vec<_> = lexed.tokens.iter().map(|token| token.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TokenKind::Word,
                TokenKind::Lua,
                TokenKind::Word,
                TokenKind::Word,
                TokenKind::Semicolon
            ]
        );
        assert!(lexed.tokens[1].value.contains("ngx.say('{')"));
        assert!(text[lexed.tokens[1].span.range()].ends_with('}'));
    }

    #[test]
    fn lua_names_only_count_as_directive_names() {
        let lexed = lex("set $x content_by_lua_block { a; }");
        assert!(lexed
            .tokens
            .iter()
            .all(|token| token.kind != TokenKind::Lua));
        let lexed = lex("content_by_lua_block { ngx.say(\"open) }");
        assert_eq!(lexed.errors[0].1, "the Lua string is not closed");
        let lexed = lex("content_by_lua_block { ngx.say(\"open)\n}");
        assert_eq!(
            lexed.errors[0].1,
            "the Lua string is not closed on its line"
        );
    }

    #[test]
    fn line_breaks_are_counted_before_tokens() {
        let lexed = lex("a;\n\n# c\nb;");
        let breaks: Vec<_> = lexed
            .tokens
            .iter()
            .map(|token| token.newlines_before)
            .collect();
        assert_eq!(breaks, vec![0, 0, 2, 1, 0]);
    }
}
