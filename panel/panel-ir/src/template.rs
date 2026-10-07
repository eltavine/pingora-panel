//! Strings evaluated per request, such as a redirect location or a response
//! body. `$name` and `${name}` name a request variable and `$$` is a
//! literal `$`; a `$` not followed by a name is literal too.

use std::fmt;

/// The capability a snapshot requires when an action uses a variable.
pub const TEMPLATE_CAPABILITY: &str = "action.template";

/// A value known only once a request arrives.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RequestVariable {
    /// The requested host, lowercase and without a port.
    Host,
    /// The normalized request path, without the query.
    Uri,
    Method,
    /// `http` or `https`.
    Scheme,
    ClientIp,
    /// The request's `X-Request-Id` when it is valid, otherwise a new one.
    RequestId,
    /// The upstream address the request went to; empty for local responses.
    UpstreamAddr,
    /// What the proxy cache did, as nginx's `$upstream_cache_status` says
    /// it: `HIT`, `MISS`, `BYPASS`, `EXPIRED`, `STALE`, `UPDATING` or
    /// `REVALIDATED`; empty when it had no part (ADR 0043).
    UpstreamCacheStatus,
    /// The client's request target, unchanged by rewrites.
    RequestUri,
    /// The current query, without the `?`; `$query_string` too.
    Args,
    /// `?` when the request has a query, empty otherwise.
    IsArgs,
    /// A query parameter, by its name, as it is written in the query.
    Arg(String),
    /// A request header, by its lowercase name with `_` for `-`.
    Header(String),
    Cookie(String),
    /// `${lua:NAME}`: what `set` or a script last gave a variable of the
    /// request.
    Lua(String),
}

impl RequestVariable {
    /// The variable a name refers to.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "host" => Self::Host,
            "uri" => Self::Uri,
            "method" => Self::Method,
            "scheme" => Self::Scheme,
            "client_ip" => Self::ClientIp,
            "request_id" => Self::RequestId,
            "upstream_addr" => Self::UpstreamAddr,
            "upstream_cache_status" => Self::UpstreamCacheStatus,
            "request_uri" => Self::RequestUri,
            "args" | "query_string" => Self::Args,
            "is_args" => Self::IsArgs,
            name => {
                let named = |prefix: &str| {
                    name.strip_prefix(prefix)
                        .filter(|rest| !rest.is_empty())
                        .map(str::to_owned)
                };
                if let Some(header) = named("http_") {
                    Self::Header(header.replace('_', "-"))
                } else if let Some(arg) = named("arg_") {
                    Self::Arg(arg)
                } else {
                    Self::Cookie(named("cookie_")?)
                }
            }
        })
    }
}

impl fmt::Display for RequestVariable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Host => formatter.write_str("host"),
            Self::Uri => formatter.write_str("uri"),
            Self::Method => formatter.write_str("method"),
            Self::Scheme => formatter.write_str("scheme"),
            Self::ClientIp => formatter.write_str("client_ip"),
            Self::RequestId => formatter.write_str("request_id"),
            Self::UpstreamAddr => formatter.write_str("upstream_addr"),
            Self::UpstreamCacheStatus => formatter.write_str("upstream_cache_status"),
            Self::RequestUri => formatter.write_str("request_uri"),
            Self::Args => formatter.write_str("args"),
            Self::IsArgs => formatter.write_str("is_args"),
            Self::Arg(name) => write!(formatter, "arg_{name}"),
            Self::Header(name) => write!(formatter, "http_{}", name.replace('-', "_")),
            Self::Cookie(name) => write!(formatter, "cookie_{name}"),
            Self::Lua(name) => write!(formatter, "lua:{name}"),
        }
    }
}

/// One piece of a template.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TemplatePart {
    Text(String),
    Variable(RequestVariable),
}

fn is_name_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_name(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The literal text and variables of `value`, or why it is not a template.
pub fn parse_template(value: &str) -> Result<Vec<TemplatePart>, String> {
    let bytes = value.as_bytes();
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut index = 0;
    let mut start = 0;
    let variable = |name: &str, text: &mut String, parts: &mut Vec<TemplatePart>| {
        let found = RequestVariable::parse(name)
            .ok_or_else(|| format!("${name} is not a request variable"))?;
        if !text.is_empty() {
            parts.push(TemplatePart::Text(std::mem::take(text)));
        }
        parts.push(TemplatePart::Variable(found));
        Ok::<_, String>(())
    };
    while index < bytes.len() {
        if bytes[index] != b'$' {
            index += 1;
            continue;
        }
        text.push_str(&value[start..index]);
        match bytes.get(index + 1) {
            Some(b'$') => {
                text.push('$');
                index += 2;
            }
            Some(b'{') => {
                let Some(end) = value[index + 2..].find('}') else {
                    return Err("an unclosed ${ starts a variable".into());
                };
                let name = &value[index + 2..index + 2 + end];
                if let Some(lua) = name.strip_prefix("lua:") {
                    if !lua.bytes().next().is_some_and(is_name_start) || !lua.bytes().all(is_name) {
                        return Err(format!("${{{name}}} is not a variable name"));
                    }
                    if !text.is_empty() {
                        parts.push(TemplatePart::Text(std::mem::take(&mut text)));
                    }
                    parts.push(TemplatePart::Variable(RequestVariable::Lua(lua.to_owned())));
                    index += end + 3;
                    start = index;
                    continue;
                }
                if name.is_empty() || !name.bytes().all(is_name) {
                    return Err(format!("${{{name}}} is not a variable name"));
                }
                variable(name, &mut text, &mut parts)?;
                index += end + 3;
            }
            Some(&next) if is_name_start(next) => {
                let end = bytes[index + 1..]
                    .iter()
                    .position(|byte| !is_name(*byte))
                    .map_or(bytes.len(), |position| index + 1 + position);
                variable(&value[index + 1..end], &mut text, &mut parts)?;
                index = end;
            }
            _ => {
                text.push('$');
                index += 1;
            }
        }
        start = index;
    }
    text.push_str(&value[start..]);
    if !text.is_empty() {
        parts.push(TemplatePart::Text(text));
    }
    Ok(parts)
}

/// Whether `value` uses a request variable.
pub fn uses_variables(value: &str) -> bool {
    parse_template(value).map_or(true, |parts| {
        parts
            .iter()
            .any(|part| matches!(part, TemplatePart::Variable(_)))
    })
}

/// The text of `value` with no variables, `$$` read as `$`.
pub fn literal(value: &str) -> Option<String> {
    let parts = parse_template(value).ok()?;
    parts
        .into_iter()
        .map(|part| match part {
            TemplatePart::Text(text) => Some(text),
            TemplatePart::Variable(_) => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variables_and_literal_dollars_are_told_apart() {
        assert_eq!(
            parse_template("https://$host${uri}?id=$request_id").unwrap(),
            [
                TemplatePart::Text("https://".into()),
                TemplatePart::Variable(RequestVariable::Host),
                TemplatePart::Variable(RequestVariable::Uri),
                TemplatePart::Text("?id=".into()),
                TemplatePart::Variable(RequestVariable::RequestId),
            ]
        );
        assert_eq!(
            parse_template("costs $$5, $5 and ^/a$").unwrap(),
            [TemplatePart::Text("costs $5, $5 and ^/a$".into())]
        );
        assert_eq!(
            parse_template("$http_x_forwarded_for $cookie_session").unwrap(),
            [
                TemplatePart::Variable(RequestVariable::Header("x-forwarded-for".into())),
                TemplatePart::Text(" ".into()),
                TemplatePart::Variable(RequestVariable::Cookie("session".into())),
            ]
        );
        assert_eq!(
            RequestVariable::Header("x-forwarded-for".into()).to_string(),
            "http_x_forwarded_for"
        );
    }

    #[test]
    fn query_variables_are_named_as_nginx_names_them() {
        assert_eq!(
            parse_template("$request_uri $args $query_string$is_args $arg_page").unwrap(),
            [
                TemplatePart::Variable(RequestVariable::RequestUri),
                TemplatePart::Text(" ".into()),
                TemplatePart::Variable(RequestVariable::Args),
                TemplatePart::Text(" ".into()),
                TemplatePart::Variable(RequestVariable::Args),
                TemplatePart::Variable(RequestVariable::IsArgs),
                TemplatePart::Text(" ".into()),
                TemplatePart::Variable(RequestVariable::Arg("page".into())),
            ]
        );
        assert_eq!(RequestVariable::Arg("page".into()).to_string(), "arg_page");
        assert!(parse_template("$arg_").is_err());
    }

    #[test]
    fn unknown_names_and_broken_references_are_refused() {
        assert!(parse_template("$nope").unwrap_err().contains("$nope"));
        assert!(parse_template("${host").is_err());
        assert!(parse_template("${a-b}").is_err());
        assert!(uses_variables("$scheme://x"));
        assert!(!uses_variables("plain $$text"));
        assert_eq!(literal("a $$ b").as_deref(), Some("a $ b"));
        assert_eq!(literal("$host"), None);
    }

    #[test]
    fn script_variables_are_braced_with_lua() {
        assert_eq!(
            parse_template("t=${lua:tenant}/$host").unwrap(),
            [
                TemplatePart::Text("t=".into()),
                TemplatePart::Variable(RequestVariable::Lua("tenant".into())),
                TemplatePart::Text("/".into()),
                TemplatePart::Variable(RequestVariable::Host),
            ]
        );
        assert_eq!(
            RequestVariable::Lua("tenant".into()).to_string(),
            "lua:tenant"
        );
        for broken in ["${lua:}", "${lua:1a}", "${lua:a-b}", "$lua:a"] {
            assert!(parse_template(broken).is_err(), "{broken}");
        }
    }
}
