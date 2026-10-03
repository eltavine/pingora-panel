//! Headers added to requests forwarded upstream.
//!
//! Pingora removes hop-by-hop fields before this runs. The gateway appends
//! `Forwarded` (RFC 7239), the de facto `X-Forwarded-*` fields and `Via`
//! (RFC 9110 §7.6.3, required of gateways for inbound requests).

use http::{header, HeaderName, HeaderValue, Version};
use pingora_http::RequestHeader;
use std::net::IpAddr;

const VIA_PSEUDONYM: &str = "pingora-panel";
static X_FORWARDED_FOR: HeaderName = HeaderName::from_static("x-forwarded-for");
static X_FORWARDED_HOST: HeaderName = HeaderName::from_static("x-forwarded-host");
static X_FORWARDED_PROTO: HeaderName = HeaderName::from_static("x-forwarded-proto");

pub(crate) struct Forwarding<'a> {
    pub client: Option<IpAddr>,
    pub tls: bool,
    pub host_override: Option<&'a str>,
    pub close: bool,
}

pub(crate) fn apply(
    request: &mut RequestHeader,
    forwarding: &Forwarding<'_>,
) -> pingora_core::Result<()> {
    let proto = if forwarding.tls { "https" } else { "http" };
    let original_host = request
        .uri
        .authority()
        .map(|authority| authority.as_str().to_owned())
        .or_else(|| {
            request
                .headers
                .get(header::HOST)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        })
        .filter(|host| !host.is_empty());

    if let Some(client) = forwarding.client {
        append(request, &X_FORWARDED_FOR, &client.to_string())?;
    }
    request.insert_header(X_FORWARDED_PROTO.clone(), proto)?;
    if let Some(host) = &original_host {
        request.insert_header(X_FORWARDED_HOST.clone(), host.as_str())?;
    }
    let mut element = Vec::with_capacity(3);
    if let Some(client) = forwarding.client {
        element.push(match client {
            IpAddr::V4(address) => format!("for={address}"),
            IpAddr::V6(address) => format!("for=\"[{address}]\""),
        });
    }
    if let Some(host) = &original_host {
        element.push(format!("host={}", quoted(host)));
    }
    element.push(format!("proto={proto}"));
    append(request, &header::FORWARDED, &element.join(";"))?;
    let version = match request.version {
        Version::HTTP_10 => "1.0",
        Version::HTTP_2 => "2",
        Version::HTTP_3 => "3",
        _ => "1.1",
    };
    append(request, &header::VIA, &format!("{version} {VIA_PSEUDONYM}"))?;
    if let Some(host) = forwarding.host_override {
        request.insert_header(header::HOST, host)?;
    }
    if forwarding.close {
        request.insert_header(header::CONNECTION, "close")?;
    }
    Ok(())
}

/// Appends to a list-valued field so earlier proxies' entries are kept.
fn append(request: &mut RequestHeader, name: &HeaderName, value: &str) -> pingora_core::Result<()> {
    let combined = match request
        .headers
        .get(name)
        .and_then(|existing| existing.to_str().ok())
        .filter(|existing| !existing.trim().is_empty())
    {
        Some(existing) => format!("{existing}, {value}"),
        None => value.to_owned(),
    };
    let value = HeaderValue::from_str(&combined).map_err(|error| {
        pingora_core::Error::because(
            pingora_core::ErrorType::InvalidHTTPHeader,
            "forwarding header",
            error,
        )
    })?;
    request.insert_header(name.clone(), value)
}

/// RFC 7239 §4: values outside the token grammar are quoted strings.
fn quoted(value: &str) -> String {
    let token = value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte));
    if token {
        value.to_owned()
    } else {
        format!("\"{}\"", value.replace(['\\', '"'], ""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwarding_fields_are_appended() {
        let mut request = RequestHeader::build("GET", b"/", None).unwrap();
        request
            .insert_header(header::HOST, "example.com:8080")
            .unwrap();
        request
            .insert_header(&X_FORWARDED_FOR, "198.51.100.7")
            .unwrap();
        request
            .insert_header(header::FORWARDED, "for=198.51.100.7")
            .unwrap();
        apply(
            &mut request,
            &Forwarding {
                client: Some("2001:db8::1".parse().unwrap()),
                tls: true,
                host_override: Some("origin.internal"),
                close: true,
            },
        )
        .unwrap();
        let field = |name: &HeaderName| request.headers.get(name).unwrap().to_str().unwrap();
        assert_eq!(field(&X_FORWARDED_FOR), "198.51.100.7, 2001:db8::1");
        assert_eq!(field(&X_FORWARDED_PROTO), "https");
        assert_eq!(field(&X_FORWARDED_HOST), "example.com:8080");
        assert_eq!(
            field(&header::FORWARDED),
            "for=198.51.100.7, for=\"[2001:db8::1]\";host=\"example.com:8080\";proto=https"
        );
        assert_eq!(field(&header::VIA), "1.1 pingora-panel");
        assert_eq!(field(&header::HOST), "origin.internal");
        assert_eq!(field(&header::CONNECTION), "close");
    }

    #[test]
    fn plain_requests_get_minimal_fields() {
        let mut request = RequestHeader::build("GET", b"/", None).unwrap();
        request.insert_header(header::HOST, "example.com").unwrap();
        apply(
            &mut request,
            &Forwarding {
                client: Some("192.0.2.4".parse().unwrap()),
                tls: false,
                host_override: None,
                close: false,
            },
        )
        .unwrap();
        assert_eq!(
            request.headers.get(header::FORWARDED).unwrap(),
            "for=192.0.2.4;host=example.com;proto=http"
        );
        assert_eq!(request.headers.get(header::HOST).unwrap(), "example.com");
        assert!(request.headers.get(header::CONNECTION).is_none());
    }
}
