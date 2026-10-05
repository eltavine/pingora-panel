//! Canonical formatting: four-space indentation, one directive per line,
//! single spaces between arguments, at most one blank line in a row and every
//! comment kept.

use crate::ast::{Body, Directive, Document, Trivia};
use std::borrow::Cow;

const INDENT: &str = "    ";

/// Formats a whole file.
pub fn format(document: &Document) -> String {
    let mut out = String::new();
    write_directives(&mut out, &document.directives, 0);
    write_trivia(
        &mut out,
        &document.trailing,
        0,
        document.directives.is_empty(),
    );
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

/// Formats one directive, its block and its leading comments, indented
/// `depth` levels, ending with a line break.
pub fn format_directive(directive: &Directive, depth: usize) -> String {
    let mut out = String::new();
    write_directive(&mut out, directive, depth, true);
    out
}

/// An argument as it is written: bare when that reads back the same value,
/// otherwise double-quoted.
pub fn quote(value: &str) -> Cow<'_, str> {
    if needs_quotes(value) {
        Cow::Owned(quoted(value))
    } else {
        Cow::Borrowed(value)
    }
}

fn needs_quotes(value: &str) -> bool {
    if value.is_empty() || value.starts_with('#') {
        return true;
    }
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '$' if chars.peek() == Some(&'{') => {
                for c in chars.by_ref() {
                    if c == '}' {
                        break;
                    }
                    if c.is_whitespace() || c == ';' {
                        return true;
                    }
                }
            }
            c if c.is_whitespace() => return true,
            ';' | '{' | '}' | '"' | '\'' | '\\' => return true,
            _ => {}
        }
    }
    false
}

fn quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            // A backslash reads back alone unless it would start an escape.
            '\\' => match chars.peek() {
                None | Some('"' | '\'' | '\\' | 't' | 'r' | 'n') => out.push_str("\\\\"),
                Some(_) => out.push('\\'),
            },
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str(INDENT);
    }
}

fn write_trivia(out: &mut String, trivia: &[Trivia], depth: usize, mut at_start: bool) {
    for item in trivia {
        match item {
            Trivia::BlankLine => {
                if !at_start && !out.ends_with("\n\n") {
                    out.push('\n');
                }
            }
            Trivia::Comment(comment) => {
                indent(out, depth);
                out.push('#');
                out.push_str(&comment.text);
                out.push('\n');
                at_start = false;
            }
        }
    }
}

fn write_directives(out: &mut String, directives: &[Directive], depth: usize) {
    for (index, directive) in directives.iter().enumerate() {
        write_directive(out, directive, depth, index == 0);
    }
}

fn write_directive(out: &mut String, directive: &Directive, depth: usize, first: bool) {
    write_trivia(out, &directive.leading, depth, first);
    indent(out, depth);
    out.push_str(&directive.name.value);
    for arg in &directive.args {
        out.push(' ');
        out.push_str(&quote(&arg.value));
    }
    match &directive.body {
        Body::Semicolon | Body::Missing => out.push(';'),
        Body::Block(_) => out.push_str(" {"),
        Body::Lua(lua) => {
            out.push_str(" {");
            out.push_str(&lua.code);
            out.push('}');
        }
    }
    if let Some(comment) = &directive.comment {
        out.push_str(" #");
        out.push_str(&comment.text);
    }
    out.push('\n');
    if let Body::Block(block) = &directive.body {
        write_directives(out, &block.directives, depth + 1);
        write_trivia(out, &block.trailing, depth + 1, block.directives.is_empty());
        while out.ends_with("\n\n") {
            out.pop();
        }
        indent(out, depth);
        out.push_str("}\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    fn reformat(text: &str) -> String {
        let parsed = parse("main.conf", text);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        format(&parsed.document)
    }

    #[test]
    fn layout_is_normalized_and_comments_survive() {
        let messy = "language_version   1 ;\n\n\n\nhttp{# all sites\n  server shop{\n\n server_name  a.example  'b c';   # names\n     # last\n\n\n }\n}\n\n\n# end\n";
        let expected = "\
language_version 1;

http { # all sites
    server shop {
        server_name a.example \"b c\"; # names
        # last
    }
}

# end
";
        assert_eq!(reformat(messy), expected);
        assert_eq!(reformat(expected), expected);
    }

    #[test]
    fn values_read_back_identically() {
        for value in [
            "plain",
            "with space",
            "semi;colon",
            "quote\"inside",
            r"regex\d+",
            "trailing\\",
            "literal\\n",
            "line\nbreak",
            "https://${host}$uri",
            "#hash",
            "a#b",
            "",
            "{brace",
        ] {
            let text = format!("x {};\n", quote(value));
            let parsed = parse("main.conf", &text);
            assert!(parsed.is_valid(), "{value:?}: {:?}", parsed.diagnostics);
            assert_eq!(parsed.document.directives[0].args[0].value, value, "{text}");
        }
        assert_eq!(quote("https://${host}$uri"), "https://${host}$uri");
        assert_eq!(quote(r"regex\d+"), r#""regex\d+""#);
    }
}
