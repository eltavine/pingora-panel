//! Rewrites of a request's path and query, and internal redirects
//! (ADR 0040).

use crate::template::{RequestVariable, TemplatePart};
use serde::{Deserialize, Serialize};

/// Required by snapshots whose sites or routes rewrite, redirect
/// internally or are internal.
pub const REWRITE_CAPABILITY: &str = "route.rewrite";

/// The most rules a site or route may have.
pub const MOST_REWRITE_RULES: usize = 64;

/// A change of a request's path, in the order a site or route lists them.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RewriteRule {
    /// Removes `prefix` from a path that is it or continues it with a `/`
    /// segment, leaving `/` at least.
    StripPrefix { prefix: String },
    /// Puts `prefix` before the path.
    AddPrefix { prefix: String },
    /// Replaces the path with a template of request variables; a `?` in it
    /// sets the query too.
    SetUri { template: String },
    /// nginx's `rewrite`: when `pattern` matches the path, it becomes
    /// `replacement`, where `$1`…`$9` and named groups take the captures.
    Rewrite {
        pattern: String,
        replacement: String,
        #[serde(default, skip_serializing_if = "RewriteFlag::is_none")]
        flag: RewriteFlag,
    },
}

/// What follows a [`RewriteRule::Rewrite`] that matched, as nginx's flags
/// say.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewriteFlag {
    /// The next rule runs; a route is chosen again after the last.
    #[default]
    None,
    /// The rules stop and a route is chosen again.
    Last,
    /// The rules stop and the route is kept.
    Break,
    /// 302 with the new URI.
    Redirect,
    /// 301 with the new URI.
    Permanent,
}

impl RewriteFlag {
    pub fn is_none(&self) -> bool {
        *self == Self::None
    }

    /// The status a redirecting flag answers with.
    pub fn redirect_status(self) -> Option<u16> {
        match self {
            Self::Redirect => Some(302),
            Self::Permanent => Some(301),
            Self::None | Self::Last | Self::Break => None,
        }
    }
}

/// One piece of a rewrite's replacement.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ReplacementPart {
    Text(String),
    /// `$1`…`$9`.
    Capture(usize),
    /// A named group of the pattern.
    Group(String),
    Variable(RequestVariable),
}

/// The pieces of `replacement`, whose `$name` names a group of `groups`
/// before a request variable, as nginx's named captures do.
pub fn parse_replacement(
    replacement: &str,
    groups: &[&str],
) -> Result<Vec<ReplacementPart>, String> {
    let bytes = replacement.as_bytes();
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut index = 0;
    while index < bytes.len() {
        let Some(offset) = replacement[index..].find('$') else {
            text.push_str(&replacement[index..]);
            break;
        };
        text.push_str(&replacement[index..index + offset]);
        index += offset;
        let (name, end) = match bytes.get(index + 1) {
            Some(b'$') => {
                text.push('$');
                index += 2;
                continue;
            }
            Some(digit @ b'1'..=b'9') => {
                if !text.is_empty() {
                    parts.push(ReplacementPart::Text(std::mem::take(&mut text)));
                }
                parts.push(ReplacementPart::Capture(usize::from(digit - b'0')));
                index += 2;
                continue;
            }
            Some(b'{') => {
                let Some(close) = replacement[index + 2..].find('}') else {
                    return Err("an unclosed ${ starts a variable".into());
                };
                (
                    &replacement[index + 2..index + 2 + close],
                    index + close + 3,
                )
            }
            Some(next) if next.is_ascii_alphabetic() || *next == b'_' => {
                let end = bytes[index + 1..]
                    .iter()
                    .position(|byte| !(byte.is_ascii_alphanumeric() || *byte == b'_'))
                    .map_or(bytes.len(), |position| index + 1 + position);
                (&replacement[index + 1..end], end)
            }
            _ => {
                text.push('$');
                index += 1;
                continue;
            }
        };
        let part = if groups.contains(&name) {
            ReplacementPart::Group(name.to_owned())
        } else {
            match crate::template::parse_template(&format!("${{{name}}}"))?.pop() {
                Some(TemplatePart::Variable(variable)) => ReplacementPart::Variable(variable),
                _ => return Err(format!("${name} is not a request variable")),
            }
        };
        if !text.is_empty() {
            parts.push(ReplacementPart::Text(std::mem::take(&mut text)));
        }
        parts.push(part);
        index = end;
    }
    if !text.is_empty() {
        parts.push(ReplacementPart::Text(text));
    }
    Ok(parts)
}

/// Whether a replacement redirects whatever its flag, as one starting with
/// `http://`, `https://` or `$scheme` does in nginx.
pub fn redirects(replacement: &str) -> bool {
    replacement.starts_with("http://")
        || replacement.starts_with("https://")
        || replacement.starts_with("$scheme")
        || replacement.starts_with("${scheme}")
}

/// The path and query a rendered replacement gives a request whose query is
/// `query`: a `?` sets the query and appends the request's own unless it
/// ends the replacement, as nginx's `rewrite` does.
pub fn split_target(rendered: &str, query: Option<&str>) -> (String, Option<String>) {
    let query = query.filter(|query| !query.is_empty());
    match rendered.split_once('?') {
        None => (rendered.to_owned(), query.map(str::to_owned)),
        Some((path, "")) => (path.to_owned(), None),
        Some((path, own)) => (
            path.to_owned(),
            Some(match query {
                Some(query) => format!("{own}&{query}"),
                None => own.to_owned(),
            }),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacements_tell_captures_groups_and_variables_apart() {
        assert_eq!(
            parse_replacement("/users/$1/${id}?from=$host&$$", &["id"]).unwrap(),
            [
                ReplacementPart::Text("/users/".into()),
                ReplacementPart::Capture(1),
                ReplacementPart::Text("/".into()),
                ReplacementPart::Group("id".into()),
                ReplacementPart::Text("?from=".into()),
                ReplacementPart::Variable(RequestVariable::Host),
                ReplacementPart::Text("&$".into()),
            ]
        );
        assert_eq!(
            parse_replacement("/$0/$9$args", &[]).unwrap(),
            [
                ReplacementPart::Text("/$0/".into()),
                ReplacementPart::Capture(9),
                ReplacementPart::Variable(RequestVariable::Args),
            ]
        );
        assert!(parse_replacement("/$nope", &[])
            .unwrap_err()
            .contains("$nope"));
        assert!(parse_replacement("/${uri", &[]).is_err());
    }

    #[test]
    fn queries_follow_nginx() {
        assert_eq!(
            split_target("/new", Some("a=1")),
            ("/new".into(), Some("a=1".into()))
        );
        assert_eq!(
            split_target("/new?b=2", Some("a=1")),
            ("/new".into(), Some("b=2&a=1".into()))
        );
        assert_eq!(split_target("/new?", Some("a=1")), ("/new".into(), None));
        assert_eq!(
            split_target("/new?b=2", None),
            ("/new".into(), Some("b=2".into()))
        );
        assert_eq!(split_target("/new", Some("")), ("/new".into(), None));
    }

    #[test]
    fn absolute_replacements_redirect() {
        assert!(redirects("https://example.com$uri"));
        assert!(redirects("$scheme://$host/new"));
        assert!(!redirects("/new"));
    }

    #[test]
    fn flags_redirect_with_nginx_statuses() {
        assert_eq!(RewriteFlag::Redirect.redirect_status(), Some(302));
        assert_eq!(RewriteFlag::Permanent.redirect_status(), Some(301));
        assert_eq!(RewriteFlag::Last.redirect_status(), None);
    }
}
