//! Rewrite rules of servers and routes (ADR 0040): nginx's `rewrite` and
//! `strip_prefix`, `add_prefix` and `set_uri`, kept in the order written.

use super::{Expansion, Lowerer};
use crate::codes;
use panel_dsl::{Argument, Directive};
use panel_ir::{RewriteFlag, RewriteRule};

/// The directives that write rewrite rules.
pub(super) const REWRITES: [&str; 4] = ["rewrite", "strip_prefix", "add_prefix", "set_uri"];

/// The end of the capture reference `value` holds at `index`, where it has a
/// `$`: `$1` to `$9`, or a named group of `groups` as `$name` or `${name}`.
pub(crate) fn capture_end(value: &str, index: usize, groups: &[String]) -> Option<usize> {
    let bytes = value.as_bytes();
    let named = |name: &str| groups.iter().any(|group| group == name);
    match bytes.get(index + 1)? {
        b'1'..=b'9' => Some(index + 2),
        b'{' => {
            let close = index + 2 + value[index + 2..].find('}')?;
            named(&value[index + 2..close]).then_some(close + 1)
        }
        next if next.is_ascii_alphabetic() || *next == b'_' => {
            let end = bytes[index + 1..]
                .iter()
                .position(|byte| !(byte.is_ascii_alphanumeric() || *byte == b'_'))
                .map_or(bytes.len(), |position| index + 1 + position);
            named(&value[index + 1..end]).then_some(end)
        }
        _ => None,
    }
}

/// The named groups of `pattern`, or why it does not compile.
pub(crate) fn groups(pattern: &str) -> Result<Vec<String>, String> {
    regex::Regex::new(pattern)
        .map(|compiled| {
            compiled
                .capture_names()
                .flatten()
                .map(str::to_owned)
                .collect()
        })
        .map_err(|error| {
            let text = error.to_string();
            text.lines()
                .last()
                .unwrap_or(&text)
                .trim_start_matches("error: ")
                .to_owned()
        })
}

impl Lowerer<'_> {
    pub(super) fn rewrite_rule(
        &mut self,
        file: &str,
        directive: &Directive,
        name: &str,
    ) -> Option<RewriteRule> {
        let first = directive.args.first()?;
        match name {
            "strip_prefix" => Some(RewriteRule::StripPrefix {
                prefix: self.value(file, first)?,
            }),
            "add_prefix" => Some(RewriteRule::AddPrefix {
                prefix: self.value(file, first)?,
            }),
            "set_uri" => Some(RewriteRule::SetUri {
                template: self.expand(file, first, &first.value.clone(), Expansion::Template)?,
            }),
            "rewrite" => {
                let [pattern, replacement, rest @ ..] = directive.args.as_slice() else {
                    self.error_with_help(
                        file,
                        directive.span,
                        codes::ARGUMENTS,
                        "a rewrite is an expression, a replacement and maybe a flag",
                        "write `rewrite ^/old/(.*)$ /new/$1 last;`",
                    );
                    return None;
                };
                let flag = match rest.first().map(|flag| (flag, flag.value.as_str())) {
                    None => RewriteFlag::None,
                    Some((_, "last")) => RewriteFlag::Last,
                    Some((_, "break")) => RewriteFlag::Break,
                    Some((_, "redirect")) => RewriteFlag::Redirect,
                    Some((_, "permanent")) => RewriteFlag::Permanent,
                    Some((flag, other)) => {
                        self.error(
                            file,
                            flag.span,
                            codes::TYPE,
                            format!("{other:?} is not last, break, redirect or permanent"),
                        );
                        return None;
                    }
                };
                let groups = match groups(&pattern.value) {
                    Ok(groups) => groups,
                    Err(error) => {
                        self.error(
                            file,
                            pattern.span,
                            codes::TYPE,
                            format!("the expression does not compile: {error}"),
                        );
                        return None;
                    }
                };
                Some(RewriteRule::Rewrite {
                    pattern: pattern.value.clone(),
                    replacement: self.replacement(file, replacement, &groups)?,
                    flag,
                })
            }
            _ => unreachable!("the schema has no other rewrites"),
        }
    }

    /// A replacement as the IR carries it: references to captures kept as
    /// written, the text between them expanded as a template.
    fn replacement(&mut self, file: &str, arg: &Argument, groups: &[String]) -> Option<String> {
        let value = arg.value.clone();
        let bytes = value.as_bytes();
        let mut out = String::with_capacity(value.len());
        let mut start = 0;
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] != b'$' {
                index += 1;
                continue;
            }
            if bytes.get(index + 1) == Some(&b'$') {
                index += 2;
                continue;
            }
            let Some(end) = capture_end(&value, index, groups) else {
                index += 1;
                continue;
            };
            out.push_str(&self.expand(file, arg, &value[start..index], Expansion::Template)?);
            out.push_str(&value[index..end]);
            start = end;
            index = end;
        }
        out.push_str(&self.expand(file, arg, &value[start..], Expansion::Template)?);
        Some(out)
    }
}
