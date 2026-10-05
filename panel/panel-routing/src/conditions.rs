//! Route conditions, compiled once per snapshot and evaluated per request
//! (ADR 0036).

use crate::{host_matches, Request};
use panel_domain::{IpNetwork, NormalizedHost};
use panel_engine::ROUTE_REGEX_SIZE_LIMIT;
use panel_errors::{PanelError, Result};
use panel_ir::{RouteCondition, ValueTest};
use regex::{Regex, RegexBuilder};
use std::{borrow::Cow, fmt};

pub(crate) enum Condition {
    Method(Vec<Box<str>>),
    Host(Vec<NormalizedHost>),
    Header { name: Box<str>, test: Test },
    Query { name: Box<str>, test: Test },
    Cookie { name: Box<str>, test: Test },
    Client(Vec<IpNetwork>),
    ContentType(Vec<MediaRange>),
    All(Vec<Condition>),
    Any(Vec<Condition>),
    Not(Box<Condition>),
}

pub(crate) enum Test {
    Present,
    Absent,
    Equals { value: Box<str>, fold: bool },
    Prefix { value: Box<str>, fold: bool },
    Suffix { value: Box<str>, fold: bool },
    Contains { value: Box<str>, fold: bool },
    Regex { regex: Regex, fold: bool },
}

pub(crate) struct MediaRange {
    kind: Box<str>,
    subtype: Box<str>,
}

fn invalid(detail: impl fmt::Display) -> PanelError {
    PanelError::validation_failed(format!("a route condition is invalid: {detail}"))
}

impl Condition {
    pub(crate) fn compile(condition: &RouteCondition) -> Result<Self> {
        let all = |conditions: &[RouteCondition]| -> Result<Vec<Condition>> {
            conditions.iter().map(Self::compile).collect()
        };
        Ok(match condition {
            RouteCondition::Method { methods } => Self::Method(
                methods
                    .iter()
                    .map(|method| method.as_str().into())
                    .collect(),
            ),
            RouteCondition::Host { hosts } => Self::Host(hosts.clone()),
            RouteCondition::Header { name, test } => Self::Header {
                name: name.to_ascii_lowercase().into(),
                test: Test::compile(test)?,
            },
            RouteCondition::Query { name, test } => Self::Query {
                name: name.as_str().into(),
                test: Test::compile(test)?,
            },
            RouteCondition::Cookie { name, test } => Self::Cookie {
                name: name.as_str().into(),
                test: Test::compile(test)?,
            },
            RouteCondition::Client { networks } => Self::Client(
                networks
                    .iter()
                    .map(|network| IpNetwork::new(network))
                    .collect::<std::result::Result<_, _>>()
                    .map_err(invalid)?,
            ),
            RouteCondition::UserAgent { test } => Self::Header {
                name: "user-agent".into(),
                test: Test::compile(test)?,
            },
            RouteCondition::Referer { test } => Self::Header {
                name: "referer".into(),
                test: Test::compile(test)?,
            },
            RouteCondition::ContentType { types } => Self::ContentType(
                types
                    .iter()
                    .map(|media| MediaRange::parse(media))
                    .collect::<Result<_>>()?,
            ),
            RouteCondition::All { conditions } => Self::All(all(conditions)?),
            RouteCondition::Any { conditions } => Self::Any(all(conditions)?),
            RouteCondition::Not { condition } => Self::Not(Box::new(Self::compile(condition)?)),
        })
    }

    pub(crate) fn holds(&self, request: &impl Request) -> bool {
        match self {
            Self::Method(methods) => methods.iter().any(|method| **method == *request.method()),
            Self::Host(hosts) => hosts.iter().any(|host| host_matches(host, request.host())),
            Self::Header { name, test } => test.holds(header(request, name).as_deref()),
            Self::Query { name, test } => test.holds_any(query(request, name)),
            Self::Cookie { name, test } => test.holds_any(cookies(request, name)),
            Self::Client(networks) => request
                .client()
                .map(|client| client.to_canonical())
                .is_some_and(|client| networks.iter().any(|network| network.contains(client))),
            Self::ContentType(ranges) => media_type(request).is_some_and(|(kind, subtype)| {
                ranges.iter().any(|range| range.covers(&kind, &subtype))
            }),
            Self::All(conditions) => conditions.iter().all(|condition| condition.holds(request)),
            Self::Any(conditions) => conditions.iter().any(|condition| condition.holds(request)),
            Self::Not(condition) => !condition.holds(request),
        }
    }

    /// Why the condition does not hold for `request`; `None` when it holds.
    pub(crate) fn mismatch(&self, request: &impl Request) -> Option<String> {
        if self.holds(request) {
            return None;
        }
        let found = match self {
            Self::All(conditions) => {
                return conditions
                    .iter()
                    .find_map(|condition| condition.mismatch(request))
            }
            Self::Any(_) => format!("{self} (none of them holds)"),
            Self::Not(_) => format!("{self} (the condition holds)"),
            Self::Method(_) => format!("{self} (the method is {})", request.method()),
            Self::Host(_) => format!("{self} (the host is {})", request.host()),
            Self::Header { name, .. } => match header(request, name) {
                Some(value) => format!("{self} (it is {value:?})"),
                None => format!("{self} (the request has no {name})"),
            },
            Self::Query { name, .. } => {
                let values: Vec<String> = query(request, name).map(Cow::into_owned).collect();
                match values.as_slice() {
                    [] => format!("{self} (the request has no {name} parameter)"),
                    values => format!("{self} (it is {})", quoted(values)),
                }
            }
            Self::Cookie { name, .. } => {
                let values: Vec<String> = cookies(request, name).map(Cow::into_owned).collect();
                match values.as_slice() {
                    [] => format!("{self} (the request has no {name} cookie)"),
                    values => format!("{self} (it is {})", quoted(values)),
                }
            }
            Self::Client(_) => match request.client() {
                Some(client) => format!("{self} (the client is {})", client.to_canonical()),
                None => format!("{self} (the client's address is unknown)"),
            },
            Self::ContentType(_) => match media_type(request) {
                Some((kind, subtype)) => format!("{self} (it is {kind}/{subtype})"),
                None => format!("{self} (the request has no content type)"),
            },
        };
        Some(found)
    }
}

fn quoted(values: &[String]) -> String {
    values
        .iter()
        .map(|value| format!("{value:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The combined value of a header's field lines (RFC 9110 §5.3).
fn header<'a>(request: &'a impl Request, name: &str) -> Option<Cow<'a, str>> {
    let mut lines = request.header_lines(name);
    let first = lines.next()?;
    let mut combined = String::from_utf8_lossy(first);
    for line in lines {
        let combined = combined.to_mut();
        combined.push_str(", ");
        combined.push_str(&String::from_utf8_lossy(line));
    }
    Some(combined)
}

/// The decoded values of the query parameters named `name`.
fn query<'a>(request: &'a impl Request, name: &'a str) -> impl Iterator<Item = Cow<'a, str>> + 'a {
    request
        .query()
        .map(|query| form_urlencoded::parse(query.as_bytes()))
        .into_iter()
        .flatten()
        .filter(move |(key, _)| key == name)
        .map(|(_, value)| value)
}

/// The values of the cookies named `name`, among every `Cookie` field;
/// cookie octets are ASCII (RFC 6265 §4.1.1), so other lines are skipped.
fn cookies<'a>(
    request: &'a impl Request,
    name: &'a str,
) -> impl Iterator<Item = Cow<'a, str>> + 'a {
    request
        .header_lines("cookie")
        .filter_map(|line| std::str::from_utf8(line).ok())
        .flat_map(|line| line.split(';'))
        .filter_map(move |pair| {
            let (key, value) = pair.split_once('=')?;
            (key.trim() == name).then(|| {
                let value = value.trim();
                Cow::Borrowed(
                    value
                        .strip_prefix('"')
                        .and_then(|value| value.strip_suffix('"'))
                        .unwrap_or(value),
                )
            })
        })
}

/// The request's media type, lowercase and without parameters.
fn media_type(request: &impl Request) -> Option<(String, String)> {
    let value = header(request, "content-type")?;
    let essence = value.split([';', ',']).next()?.trim();
    let (kind, subtype) = essence.split_once('/')?;
    let (kind, subtype) = (kind.trim(), subtype.trim());
    (!kind.is_empty() && !subtype.is_empty())
        .then(|| (kind.to_ascii_lowercase(), subtype.to_ascii_lowercase()))
}

impl MediaRange {
    fn parse(value: &str) -> Result<Self> {
        let (kind, subtype) = value
            .split_once('/')
            .filter(|(kind, subtype)| !kind.is_empty() && !subtype.is_empty())
            .ok_or_else(|| invalid(format!("{value:?} is not a media type")))?;
        Ok(Self {
            kind: kind.to_ascii_lowercase().into(),
            subtype: subtype.to_ascii_lowercase().into(),
        })
    }

    fn covers(&self, kind: &str, subtype: &str) -> bool {
        (&*self.kind == "*" || *self.kind == *kind)
            && (&*self.subtype == "*" || *self.subtype == *subtype)
    }
}

impl Test {
    fn compile(test: &ValueTest) -> Result<Self> {
        let text = |value: &str| -> Box<str> { value.into() };
        Ok(match test {
            ValueTest::Present => Self::Present,
            ValueTest::Absent => Self::Absent,
            ValueTest::Equals { value, ignore_case } => Self::Equals {
                value: text(value),
                fold: *ignore_case,
            },
            ValueTest::Prefix { value, ignore_case } => Self::Prefix {
                value: text(value),
                fold: *ignore_case,
            },
            ValueTest::Suffix { value, ignore_case } => Self::Suffix {
                value: text(value),
                fold: *ignore_case,
            },
            ValueTest::Contains { value, ignore_case } => Self::Contains {
                value: text(value),
                fold: *ignore_case,
            },
            ValueTest::Regex {
                pattern,
                ignore_case,
            } => Self::Regex {
                regex: RegexBuilder::new(pattern)
                    .case_insensitive(*ignore_case)
                    .size_limit(ROUTE_REGEX_SIZE_LIMIT)
                    .dfa_size_limit(ROUTE_REGEX_SIZE_LIMIT)
                    .build()
                    .map_err(invalid)?,
                fold: *ignore_case,
            },
        })
    }

    fn holds(&self, value: Option<&str>) -> bool {
        let Some(value) = value else {
            return matches!(self, Self::Absent);
        };
        match self {
            Self::Present => true,
            Self::Absent => false,
            Self::Equals {
                value: expected,
                fold,
            } => {
                if *fold {
                    value.eq_ignore_ascii_case(expected)
                } else {
                    value == &**expected
                }
            }
            Self::Prefix {
                value: expected,
                fold,
            } => value
                .as_bytes()
                .get(..expected.len())
                .is_some_and(|start| same(start, expected.as_bytes(), *fold)),
            Self::Suffix {
                value: expected,
                fold,
            } => value
                .len()
                .checked_sub(expected.len())
                .is_some_and(|start| same(&value.as_bytes()[start..], expected.as_bytes(), *fold)),
            Self::Contains {
                value: expected,
                fold,
            } => {
                expected.is_empty()
                    || value
                        .as_bytes()
                        .windows(expected.len())
                        .any(|window| same(window, expected.as_bytes(), *fold))
            }
            Self::Regex { regex, .. } => regex.is_match(value),
        }
    }

    /// For repeated parameters and cookies: present when any is, absent
    /// when none is, and otherwise held when any value holds.
    fn holds_any<'a>(&self, mut values: impl Iterator<Item = Cow<'a, str>>) -> bool {
        match self {
            Self::Absent => values.next().is_none(),
            Self::Present => values.next().is_some(),
            test => values.any(|value| test.holds(Some(&value))),
        }
    }
}

fn same(left: &[u8], right: &[u8], fold: bool) -> bool {
    if fold {
        left.eq_ignore_ascii_case(right)
    } else {
        left == right
    }
}

impl fmt::Display for Condition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let list = |items: &mut dyn Iterator<Item = String>| items.collect::<Vec<_>>().join(" ");
        match self {
            Self::Method(methods) => {
                write!(
                    formatter,
                    "method {}",
                    list(&mut methods.iter().map(|m| m.to_string()))
                )
            }
            Self::Host(hosts) => write!(
                formatter,
                "host {}",
                list(&mut hosts.iter().map(|host| host.as_str().to_owned()))
            ),
            Self::Header { name, test } => write!(formatter, "header {name} {test}"),
            Self::Query { name, test } => write!(formatter, "query {name} {test}"),
            Self::Cookie { name, test } => write!(formatter, "cookie {name} {test}"),
            Self::Client(networks) => write!(
                formatter,
                "client {}",
                list(&mut networks.iter().map(ToString::to_string))
            ),
            Self::ContentType(ranges) => write!(
                formatter,
                "content_type {}",
                list(
                    &mut ranges
                        .iter()
                        .map(|range| format!("{}/{}", range.kind, range.subtype))
                )
            ),
            Self::All(conditions) | Self::Any(conditions) => {
                let group = if matches!(self, Self::All(_)) {
                    "all"
                } else {
                    "any"
                };
                write!(
                    formatter,
                    "{group} {{ {} }}",
                    conditions
                        .iter()
                        .map(|condition| format!("{condition};"))
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            }
            Self::Not(condition) => write!(formatter, "not {{ {condition}; }}"),
        }
    }
}

impl fmt::Display for Test {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let operator = |operator: &str, fold: bool| {
            if fold {
                format!("{operator}*")
            } else {
                operator.to_owned()
            }
        };
        match self {
            Self::Present => formatter.write_str("present"),
            Self::Absent => formatter.write_str("absent"),
            Self::Equals { value, fold } => write!(formatter, "{} {value:?}", operator("=", *fold)),
            Self::Prefix { value, fold } => {
                write!(formatter, "{} {value:?}", operator("^=", *fold))
            }
            Self::Suffix { value, fold } => {
                write!(formatter, "{} {value:?}", operator("$=", *fold))
            }
            Self::Contains { value, fold } => {
                write!(formatter, "{} {value:?}", operator("*=", *fold))
            }
            Self::Regex { regex, fold } => {
                write!(formatter, "{} {:?}", operator("~", *fold), regex.as_str())
            }
        }
    }
}
