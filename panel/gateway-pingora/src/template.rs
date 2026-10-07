//! Request-variable templates: parsed when a snapshot is prepared and
//! filled in for each request.

use crate::request_identity;
use bytes::Bytes;
use cookie::Cookie;
use http::HeaderMap;
use panel_ir::{
    logging::REDACTED,
    template::{parse_template, RequestVariable, TemplatePart},
};
use std::{
    collections::{HashMap, HashSet},
    net::IpAddr,
};

#[derive(Clone, Debug)]
pub(crate) enum Template {
    /// No variables: the text as written.
    Literal(Bytes),
    Parts(Vec<TemplatePart>),
}

/// What a template may refer to about the current request.
pub(crate) struct Facts<'a> {
    pub host: &'a str,
    pub uri: &'a str,
    /// The current query, without the `?`.
    pub query: Option<&'a str>,
    /// The client's request target, as rewrites leave it.
    pub request_uri: &'a str,
    pub method: &'a str,
    pub scheme: &'a str,
    pub client_ip: Option<IpAddr>,
    pub headers: &'a HeaderMap,
    /// The upstream node the request went to, as `address:port`.
    pub upstream: Option<&'a str>,
    /// The variables `set` and scripts gave the request.
    pub variables: &'a HashMap<String, String>,
}

impl Template {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        let parts = parse_template(value)?;
        let mut literal = String::new();
        for part in &parts {
            match part {
                TemplatePart::Text(text) => literal.push_str(text),
                _ => return Ok(Self::Parts(parts)),
            }
        }
        Ok(Self::Literal(Bytes::from(literal)))
    }

    pub(crate) fn render(&self, facts: &Facts<'_>) -> Bytes {
        let parts = match self {
            Self::Literal(text) => return Bytes::clone(text),
            Self::Parts(parts) => parts,
        };
        let mut out = String::new();
        for part in parts {
            match part {
                TemplatePart::Text(text) => out.push_str(text),
                TemplatePart::Variable(variable) => out.push_str(&value(variable, facts)),
                _ => {}
            }
        }
        Bytes::from(out)
    }

    /// The text for a log record, with sensitive headers and every cookie
    /// logged as `REDACTED` (ADR 0025).
    pub(crate) fn render_redacted(&self, facts: &Facts<'_>, redacted: &HashSet<String>) -> String {
        let parts = match self {
            Self::Literal(text) => return String::from_utf8_lossy(text).into_owned(),
            Self::Parts(parts) => parts,
        };
        let mut out = String::new();
        for part in parts {
            match part {
                TemplatePart::Text(text) => out.push_str(text),
                TemplatePart::Variable(RequestVariable::Cookie(_)) => out.push_str(REDACTED),
                TemplatePart::Variable(RequestVariable::Header(name))
                    if redacted.contains(name) =>
                {
                    out.push_str(REDACTED);
                }
                TemplatePart::Variable(variable) => out.push_str(&value(variable, facts)),
                _ => {}
            }
        }
        out
    }
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(http::header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(Cookie::split_parse)
        .filter_map(Result::ok)
        .find(|cookie| cookie.name() == name)
        .map(|cookie| cookie.value().to_owned())
}

/// The request's own ID when it is a short visible token, otherwise a new one.
fn request_id(headers: &HeaderMap) -> String {
    request_identity::request_id(headers)
        .map_or_else(|| uuid::Uuid::now_v7().to_string(), str::to_owned)
}

/// The value of the query parameter `name`, compared ignoring ASCII case as
/// nginx's `$arg_` does, as it is written in the query.
fn argument<'a>(query: Option<&'a str>, name: &str) -> &'a str {
    query
        .into_iter()
        .flat_map(|query| query.split('&'))
        .find_map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            key.eq_ignore_ascii_case(name).then_some(value)
        })
        .unwrap_or_default()
}

pub(crate) fn value(variable: &RequestVariable, facts: &Facts<'_>) -> String {
    match variable {
        RequestVariable::Host => facts.host.to_owned(),
        RequestVariable::Uri => facts.uri.to_owned(),
        RequestVariable::RequestUri => facts.request_uri.to_owned(),
        RequestVariable::Args => facts.query.unwrap_or_default().to_owned(),
        RequestVariable::IsArgs => {
            if facts.query.is_some_and(|query| !query.is_empty()) {
                "?".to_owned()
            } else {
                String::new()
            }
        }
        RequestVariable::Arg(name) => argument(facts.query, name).to_owned(),
        RequestVariable::Method => facts.method.to_owned(),
        RequestVariable::Scheme => facts.scheme.to_owned(),
        RequestVariable::ClientIp => facts.client_ip.map(|ip| ip.to_string()).unwrap_or_default(),
        RequestVariable::RequestId => request_id(facts.headers),
        RequestVariable::Header(name) => header(facts.headers, name).unwrap_or_default().to_owned(),
        RequestVariable::Cookie(name) => cookie(facts.headers, name).unwrap_or_default(),
        RequestVariable::UpstreamAddr => facts.upstream.unwrap_or_default().to_owned(),
        RequestVariable::Lua(name) => facts.variables.get(name).cloned().unwrap_or_default(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static NONE: std::sync::LazyLock<HashMap<String, String>> =
        std::sync::LazyLock::new(HashMap::new);

    fn facts(headers: &HeaderMap) -> Facts<'_> {
        Facts {
            variables: &NONE,
            host: "shop.example",
            uri: "/a/b",
            query: Some("page=2&Sort=asc&flag"),
            request_uri: "/old/b?page=2",
            method: "GET",
            scheme: "http",
            client_ip: Some("192.0.2.7".parse().unwrap()),
            headers,
            upstream: None,
        }
    }

    #[test]
    fn literal_text_is_kept_without_rendering() {
        let template = Template::parse("costs $$5").unwrap();
        assert!(matches!(&template, Template::Literal(text) if text == "costs $5"));
        assert!(matches!(Template::parse("").unwrap(), Template::Literal(text) if text.is_empty()));
        assert!(Template::parse("$nope").is_err());
    }

    #[test]
    fn variables_are_filled_in_from_the_request() {
        let mut headers = HeaderMap::new();
        headers.insert("x-request-id", "req-7".parse().unwrap());
        headers.insert("x-tenant", "acme".parse().unwrap());
        headers.insert(http::header::COOKIE, "a=1; session=xyz".parse().unwrap());
        let template = Template::parse(
            "$scheme://$host$uri $method $client_ip $request_id $http_x_tenant $cookie_session [$upstream_addr] $http_missing",
        )
        .unwrap();
        assert_eq!(
            template.render(&facts(&headers)),
            "http://shop.example/a/b GET 192.0.2.7 req-7 acme xyz [] "
        );
        let variables = HashMap::from([("tenant".to_owned(), "t1".to_owned())]);
        let scripted = Facts {
            variables: &variables,
            ..facts(&headers)
        };
        assert_eq!(
            Template::parse("${lua:tenant}/${lua:unset}.")
                .unwrap()
                .render(&scripted),
            "t1/."
        );
    }

    #[test]
    fn query_variables_read_the_current_query_and_the_original_target() {
        let headers = HeaderMap::new();
        let template =
            Template::parse("$request_uri|$args|$is_args|$arg_sort|$arg_flag|$arg_none").unwrap();
        assert_eq!(
            template.render(&facts(&headers)),
            "/old/b?page=2|page=2&Sort=asc&flag|?|asc||"
        );
        let bare = Facts {
            query: None,
            ..facts(&headers)
        };
        assert_eq!(
            Template::parse("[$is_args$args]").unwrap().render(&bare),
            "[]"
        );
    }

    #[test]
    fn requests_without_a_usable_id_get_a_new_one() {
        let mut headers = HeaderMap::new();
        headers.insert("x-request-id", "has space".parse().unwrap());
        let rendered = Template::parse("$request_id")
            .unwrap()
            .render(&facts(&headers));
        assert_eq!(rendered.len(), 36);
        assert_ne!(rendered, "has space");
    }

    #[test]
    fn log_fields_redact_sensitive_headers_and_every_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer secret".parse().unwrap());
        headers.insert("x-api-key", "key".parse().unwrap());
        headers.insert("x-tenant", "acme".parse().unwrap());
        headers.insert(http::header::COOKIE, "session=xyz".parse().unwrap());
        let redacted = HashSet::from(["authorization".to_owned(), "x-api-key".to_owned()]);
        let mut facts = facts(&headers);
        facts.upstream = Some("10.0.0.7:8080");
        let template = Template::parse(
            "$http_authorization $http_x_api_key $cookie_session $http_x_tenant $upstream_addr",
        )
        .unwrap();
        assert_eq!(
            template.render_redacted(&facts, &redacted),
            "REDACTED REDACTED REDACTED acme 10.0.0.7:8080"
        );
        assert_eq!(
            Template::parse("fixed")
                .unwrap()
                .render_redacted(&facts, &redacted),
            "fixed"
        );
    }
}
