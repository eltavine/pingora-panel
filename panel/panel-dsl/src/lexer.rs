//! Tokens as NGINX reads them: words, quoted strings, `;`, `{`, `}` and
//! comments.

use crate::span::Span;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TokenKind {
    Word,
    Quoted,
    Semicolon,
    Open,
    Close,
    Comment,
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

pub(crate) fn lex(text: &str) -> Lexed {
    let mut lexed = Lexed::default();
    let mut chars = text.char_indices().peekable();
    let mut newlines = 0;
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
