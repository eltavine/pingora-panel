//! Request-variable templates: parsed when a snapshot is prepared and
//! filled in for each request.

use bytes::Bytes;
use http::HeaderMap;
use panel_ir::template::{parse_template, RequestVariable, TemplatePart};
use std::net::IpAddr;

const REQUEST_ID: &str = "x-request-id";
/// The longest `X-Request-Id` taken from a request.
const MAX_REQUEST_ID: usize = 128;

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
    pub method: &'a str,
    pub scheme: &'a str,
    pub client_ip: Option<IpAddr>,
    pub headers: &'a HeaderMap,
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
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(http::header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| {
            let (key, value) = pair.trim().split_once('=')?;
            (key == name).then(|| value.to_owned())
        })
}

/// The request's own ID when it is a short visible token, otherwise a new one.
fn request_id(headers: &HeaderMap) -> String {
    header(headers, REQUEST_ID)
        .filter(|id| {
            !id.is_empty()
                && id.len() <= MAX_REQUEST_ID
                && id.bytes().all(|byte| byte.is_ascii_graphic())
        })
        .map_or_else(|| uuid::Uuid::now_v7().to_string(), str::to_owned)
}

fn value(variable: &RequestVariable, facts: &Facts<'_>) -> String {
    match variable {
        RequestVariable::Host => facts.host.to_owned(),
        RequestVariable::Uri => facts.uri.to_owned(),
        RequestVariable::Method => facts.method.to_owned(),
        RequestVariable::Scheme => facts.scheme.to_owned(),
        RequestVariable::ClientIp => facts.client_ip.map(|ip| ip.to_string()).unwrap_or_default(),
        RequestVariable::RequestId => request_id(facts.headers),
        RequestVariable::Header(name) => header(facts.headers, name).unwrap_or_default().to_owned(),
        RequestVariable::Cookie(name) => cookie(facts.headers, name).unwrap_or_default(),
        // Local responses are not proxied, so there is no upstream address.
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(headers: &HeaderMap) -> Facts<'_> {
        Facts {
            host: "shop.example",
            uri: "/a/b",
            method: "GET",
            scheme: "http",
            client_ip: Some("192.0.2.7".parse().unwrap()),
            headers,
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
}
