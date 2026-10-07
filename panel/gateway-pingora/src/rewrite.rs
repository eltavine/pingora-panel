//! Rewrite rules of sites and routes and internal redirect targets
//! (ADR 0040), compiled when a snapshot is prepared and applied to each
//! request's path and query.

use crate::template::{self, Facts};
use panel_engine::ROUTE_REGEX_SIZE_LIMIT;
use panel_ir::{
    rewrite::{parse_replacement, redirects, split_target, ReplacementPart},
    template::{parse_template, TemplatePart},
    RewriteFlag, RewriteRule,
};
use regex::{Captures, Regex, RegexBuilder};
use std::borrow::Cow;

/// A site's or route's rules, in their order.
#[derive(Default)]
pub(crate) struct Rewrites(Vec<Rule>);

enum Rule {
    StripPrefix(String),
    AddPrefix(String),
    SetUri(Vec<TemplatePart>),
    Rewrite {
        pattern: Regex,
        replacement: Vec<ReplacementPart>,
        flag: RewriteFlag,
        redirects: bool,
    },
}

/// What a request's rules made of it.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Rewritten {
    /// The path, normalized, and the query the request goes on with.
    Path {
        path: String,
        query: Option<String>,
        changed: bool,
        /// A route is chosen again, as after nginx's `last`.
        reroute: bool,
    },
    Redirect {
        status: u16,
        location: String,
    },
}

/// Where an internal redirect sends a request.
pub(crate) enum InternalTarget {
    Named(String),
    Path(Vec<TemplatePart>),
}

impl InternalTarget {
    pub(crate) fn compile(target: &str) -> Result<Self, String> {
        Ok(match target.strip_prefix('@') {
            Some(name) => Self::Named(name.to_owned()),
            None => Self::Path(parse_template(target)?),
        })
    }

    /// The path and query a path target sends a request with, its own query
    /// taking the place of the request's as a rewrite's does.
    pub(crate) fn target(
        parts: &[TemplatePart],
        facts: &Facts<'_>,
    ) -> Result<(String, Option<String>), String> {
        absolute(split_target(
            &render(template_pieces(parts, facts)),
            facts.query,
        ))
    }
}

impl Rewrites {
    pub(crate) fn compile(rules: &[RewriteRule]) -> Result<Self, String> {
        rules
            .iter()
            .map(|rule| {
                Ok(match rule {
                    RewriteRule::StripPrefix { prefix } => Rule::StripPrefix(prefix.clone()),
                    RewriteRule::AddPrefix { prefix } => Rule::AddPrefix(prefix.clone()),
                    RewriteRule::SetUri { template } => Rule::SetUri(parse_template(template)?),
                    RewriteRule::Rewrite {
                        pattern,
                        replacement,
                        flag,
                    } => {
                        let pattern = RegexBuilder::new(pattern)
                            .size_limit(ROUTE_REGEX_SIZE_LIMIT)
                            .dfa_size_limit(ROUTE_REGEX_SIZE_LIMIT)
                            .build()
                            .map_err(|error| error.to_string())?;
                        let groups: Vec<&str> = pattern.capture_names().flatten().collect();
                        Rule::Rewrite {
                            replacement: parse_replacement(replacement, &groups)?,
                            redirects: redirects(replacement),
                            pattern,
                            flag: *flag,
                        }
                    }
                })
            })
            .collect::<Result<_, String>>()
            .map(Self)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Runs the rules on `path`, normalized, and `query`, with `facts` of
    /// the request for its variables.
    pub(crate) fn apply(
        &self,
        path: &str,
        query: Option<&str>,
        facts: &Facts<'_>,
    ) -> Result<Rewritten, String> {
        let mut path = path.to_owned();
        let mut query = query.filter(|query| !query.is_empty()).map(str::to_owned);
        let mut changed = false;
        let mut reroute = false;
        for rule in &self.0 {
            let current = Facts {
                uri: &path,
                query: query.as_deref(),
                ..*facts
            };
            let (next, next_query) = match rule {
                Rule::StripPrefix(prefix) => {
                    let rest = if path == *prefix {
                        Some("/")
                    } else {
                        path.strip_prefix(prefix.as_str())
                            .filter(|rest| rest.starts_with('/'))
                    };
                    let Some(rest) = rest else {
                        continue;
                    };
                    (rest.to_owned(), query.clone())
                }
                Rule::AddPrefix(prefix) => (format!("{prefix}{path}"), query.clone()),
                Rule::SetUri(parts) => absolute(split_target(
                    &render(template_pieces(parts, &current)),
                    query.as_deref(),
                ))?,
                Rule::Rewrite {
                    pattern,
                    replacement,
                    flag,
                    redirects,
                } => {
                    let Some(captures) = pattern.captures(&path) else {
                        continue;
                    };
                    let rendered = render(replacement_pieces(replacement, &captures, &current));
                    if *redirects || flag.redirect_status().is_some() {
                        let (target, target_query) = split_target(&rendered, query.as_deref());
                        let location = match target_query {
                            Some(target_query) => format!("{target}?{target_query}"),
                            None => target,
                        };
                        return Ok(Rewritten::Redirect {
                            status: flag.redirect_status().unwrap_or(302),
                            location,
                        });
                    }
                    let target = absolute(split_target(&rendered, query.as_deref()))?;
                    match flag {
                        RewriteFlag::Break => {
                            (path, query) = target;
                            changed = true;
                            reroute = false;
                            break;
                        }
                        RewriteFlag::Last => {
                            (path, query) = target;
                            changed = true;
                            reroute = true;
                            break;
                        }
                        _ => {
                            reroute = true;
                            target
                        }
                    }
                }
            };
            path = next;
            query = next_query;
            changed = true;
        }
        Ok(Rewritten::Path {
            path,
            query,
            changed,
            reroute,
        })
    }
}

/// `target` with its path normalized, or why it cannot be a request's.
fn absolute((path, query): (String, Option<String>)) -> Result<(String, Option<String>), String> {
    let normal = panel_routing::path::normalize(&path)
        .ok_or_else(|| format!("the rewritten target {path:?} is not an absolute path"))?
        .into_owned();
    Ok((normal, query.filter(|query| !query.is_empty())))
}

enum Piece<'a> {
    /// Text as written, a `?` in it starting the query.
    Literal(&'a str),
    /// A value, its `?` and `#` encoded.
    Value(Cow<'a, str>),
}

fn template_pieces<'a>(parts: &'a [TemplatePart], facts: &Facts<'_>) -> Vec<Piece<'a>> {
    parts
        .iter()
        .filter_map(|part| match part {
            TemplatePart::Text(text) => Some(Piece::Literal(text)),
            TemplatePart::Variable(variable) => {
                Some(Piece::Value(Cow::Owned(template::value(variable, facts))))
            }
            _ => None,
        })
        .collect()
}

fn replacement_pieces<'a>(
    parts: &'a [ReplacementPart],
    captures: &Captures<'a>,
    facts: &Facts<'_>,
) -> Vec<Piece<'a>> {
    parts
        .iter()
        .filter_map(|part| match part {
            ReplacementPart::Text(text) => Some(Piece::Literal(text)),
            ReplacementPart::Capture(group) => Some(Piece::Literal(
                captures.get(*group).map_or("", |found| found.as_str()),
            )),
            ReplacementPart::Group(name) => Some(Piece::Literal(
                captures.name(name).map_or("", |found| found.as_str()),
            )),
            ReplacementPart::Variable(variable) => {
                Some(Piece::Value(Cow::Owned(template::value(variable, facts))))
            }
            _ => None,
        })
        .collect()
}

/// Whether `byte` may stand in a request target's path, or its query, as
/// RFC 3986 §3.3 and §3.4 allow, a `%` kept as an encoding's start.
fn allowed(byte: u8, query: bool) -> bool {
    byte.is_ascii_alphanumeric()
        || b"-._~!$&'()*+,;=:@/%".contains(&byte)
        || (query && byte == b'?')
}

fn push_encoded(out: &mut String, text: &str, query: bool) {
    for byte in text.bytes() {
        if allowed(byte, query) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
}

/// The request target `pieces` spell, encoded where they hold what a
/// target may not.
fn render(pieces: Vec<Piece<'_>>) -> String {
    let mut out = String::new();
    let mut query = false;
    for piece in pieces {
        match piece {
            Piece::Literal(text) if !query => match text.split_once('?') {
                Some((path, rest)) => {
                    push_encoded(&mut out, path, false);
                    out.push('?');
                    query = true;
                    push_encoded(&mut out, rest, true);
                }
                None => push_encoded(&mut out, text, false),
            },
            Piece::Literal(text) => push_encoded(&mut out, text, true),
            Piece::Value(value) => {
                if query {
                    push_encoded(&mut out, &value, true);
                } else {
                    push_encoded(&mut out, &value.replace('?', "%3F"), false);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderMap;
    use std::collections::HashMap;

    fn rules(rules: Vec<RewriteRule>) -> Rewrites {
        Rewrites::compile(&rules).unwrap()
    }

    fn rewrite(pattern: &str, replacement: &str, flag: RewriteFlag) -> RewriteRule {
        RewriteRule::Rewrite {
            pattern: pattern.into(),
            replacement: replacement.into(),
            flag,
        }
    }

    fn apply(rewrites: &Rewrites, path: &str, query: Option<&str>) -> Rewritten {
        let headers = HeaderMap::from_iter([(
            http::header::HeaderName::from_static("x-tenant"),
            http::HeaderValue::from_static("a b?c"),
        )]);
        let variables = HashMap::new();
        let facts = Facts {
            host: "shop.example",
            uri: path,
            query,
            request_uri: "/original?x=1",
            method: "GET",
            scheme: "https",
            client_ip: None,
            headers: &headers,
            upstream: None,
            variables: &variables,
        };
        rewrites.apply(path, query, &facts).unwrap()
    }

    fn path(path: &str, query: Option<&str>, changed: bool, reroute: bool) -> Rewritten {
        Rewritten::Path {
            path: path.into(),
            query: query.map(str::to_owned),
            changed,
            reroute,
        }
    }

    #[test]
    fn prefixes_follow_segments() {
        let strip = rules(vec![RewriteRule::StripPrefix {
            prefix: "/api".into(),
        }]);
        assert_eq!(
            apply(&strip, "/api/users", Some("a=1")),
            path("/users", Some("a=1"), true, false)
        );
        assert_eq!(apply(&strip, "/api", None), path("/", None, true, false));
        assert_eq!(
            apply(&strip, "/apiary", None),
            path("/apiary", None, false, false)
        );
        let add = rules(vec![
            RewriteRule::StripPrefix {
                prefix: "/api".into(),
            },
            RewriteRule::AddPrefix {
                prefix: "/v2".into(),
            },
        ]);
        assert_eq!(
            apply(&add, "/api/x", None),
            path("/v2/x", None, true, false)
        );
    }

    #[test]
    fn set_uri_renders_variables_and_follows_nginx_queries() {
        let set = rules(vec![RewriteRule::SetUri {
            template: "/index.php?q=$uri&t=$http_x_tenant".into(),
        }]);
        assert_eq!(
            apply(&set, "/a/b", Some("page=2")),
            path("/index.php", Some("q=/a/b&t=a%20b?c&page=2"), true, false)
        );
        let dropped = rules(vec![RewriteRule::SetUri {
            template: "/$http_x_tenant/?".into(),
        }]);
        assert_eq!(
            apply(&dropped, "/", Some("page=2")),
            path("/a%20b%3Fc/", None, true, false)
        );
    }

    #[test]
    fn regex_rules_capture_and_choose_again_unless_broken() {
        let flagless = rules(vec![
            rewrite("^/old/(.*)$", "/new/$1", RewriteFlag::None),
            rewrite("^/new/(?<rest>.*)$", "/newer/$rest", RewriteFlag::None),
        ]);
        assert_eq!(
            apply(&flagless, "/old/a%2Fb", None),
            path("/newer/a%2Fb", None, true, true)
        );
        let broken = rules(vec![
            rewrite("^/a$", "/b", RewriteFlag::None),
            rewrite("^/b$", "/c", RewriteFlag::Break),
            rewrite("^/c$", "/d", RewriteFlag::None),
        ]);
        assert_eq!(apply(&broken, "/a", None), path("/c", None, true, false));
        let last = rules(vec![
            rewrite("^/a$", "/b", RewriteFlag::Last),
            rewrite("^/b$", "/c", RewriteFlag::None),
        ]);
        assert_eq!(apply(&last, "/a", None), path("/b", None, true, true));
        assert_eq!(apply(&last, "/z", None), path("/z", None, false, false));
        let dots = rules(vec![rewrite("^/a/(.*)$", "/b/../$1", RewriteFlag::Break)]);
        assert_eq!(apply(&dots, "/a/c", None), path("/c", None, true, false));
    }

    #[test]
    fn redirects_carry_the_query_and_flag_statuses() {
        let permanent = rules(vec![rewrite("^/old$", "/new", RewriteFlag::Permanent)]);
        assert_eq!(
            apply(&permanent, "/old", Some("a=1")),
            Rewritten::Redirect {
                status: 301,
                location: "/new?a=1".into()
            }
        );
        let absolute = rules(vec![rewrite(
            "^/(.*)$",
            "$scheme://$host/moved/$1?",
            RewriteFlag::None,
        )]);
        assert_eq!(
            apply(&absolute, "/x", Some("a=1")),
            Rewritten::Redirect {
                status: 302,
                location: "https://shop.example/moved/x".into()
            }
        );
    }

    #[test]
    fn targets_that_are_not_paths_are_refused() {
        let relative = rules(vec![RewriteRule::SetUri {
            template: "$http_x_tenant".into(),
        }]);
        let headers = HeaderMap::new();
        let variables = HashMap::new();
        let facts = Facts {
            host: "",
            uri: "/",
            query: None,
            request_uri: "/",
            method: "GET",
            scheme: "http",
            client_ip: None,
            headers: &headers,
            upstream: None,
            variables: &variables,
        };
        assert!(relative
            .apply("/", None, &facts)
            .unwrap_err()
            .contains("not an absolute path"));
        let InternalTarget::Path(parts) = InternalTarget::compile("/errors$uri?").unwrap() else {
            panic!("a path target");
        };
        let facts = Facts {
            uri: "/a",
            query: Some("b=1"),
            ..facts
        };
        assert_eq!(
            InternalTarget::target(&parts, &facts).unwrap(),
            ("/errors/a".into(), None)
        );
        assert!(matches!(
            InternalTarget::compile("@fallback").unwrap(),
            InternalTarget::Named(name) if name == "fallback"
        ));
    }
}
