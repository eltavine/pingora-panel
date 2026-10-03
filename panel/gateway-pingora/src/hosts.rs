//! Request target host (RFC 9110 §7.2, RFC 9112 §3.2).

use http::{header::HOST, uri::Authority, Version};
use pingora_http::RequestHeader;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RequestHost {
    /// Lowercase name without port or trailing dot; IPv6 literals keep brackets.
    pub name: String,
    pub port: Option<u16>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HostError {
    /// HTTP/1.1 requires exactly one `Host` field.
    Missing,
    Multiple,
    Invalid,
}

/// The URI authority wins over `Host` (RFC 9112 §3.2.2). `None` means the
/// request names no host, which only HTTP/1.0 or an empty `Host` may do.
pub(crate) fn request_host(request: &RequestHeader) -> Result<Option<RequestHost>, HostError> {
    let mut fields = request.headers.get_all(HOST).iter();
    let field = fields.next();
    if fields.next().is_some() {
        return Err(HostError::Multiple);
    }
    let target = request
        .uri
        .authority()
        .map(|authority| authority.as_str().as_bytes())
        .or_else(|| absolute_form_authority(request.raw_path()));
    let raw = match (target, field) {
        (Some(authority), _) => authority,
        (None, Some(field)) => field.as_bytes(),
        (None, None) if request.version == Version::HTTP_10 => return Ok(None),
        (None, None) if request.version == Version::HTTP_11 => return Err(HostError::Missing),
        (None, None) => return Ok(None),
    };
    if raw.is_empty() {
        return Ok(None);
    }
    if raw.contains(&b'@') {
        return Err(HostError::Invalid);
    }
    let authority = Authority::try_from(raw).map_err(|_| HostError::Invalid)?;
    let host = authority.host();
    // RFC 3986 §3.2.3: `port = *DIGIT`.
    let port_valid = authority.as_str()[host.len()..]
        .strip_prefix(':')
        .map_or(authority.as_str().len() == host.len(), |port| {
            port.bytes().all(|byte| byte.is_ascii_digit())
        });
    if !port_valid {
        return Err(HostError::Invalid);
    }
    let name = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    let valid = if let Some(literal) = name.strip_prefix('[') {
        literal
            .strip_suffix(']')
            .is_some_and(|address| address.parse::<std::net::Ipv6Addr>().is_ok())
    } else {
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-._".contains(&byte))
    };
    if !valid {
        return Err(HostError::Invalid);
    }
    Ok(Some(RequestHost {
        name,
        port: authority.port_u16(),
    }))
}

/// The authority of an HTTP/1 absolute-form target, which Pingora keeps in
/// the raw request target rather than the parsed URI.
fn absolute_form_authority(target: &[u8]) -> Option<&[u8]> {
    let scheme_end = target.windows(3).position(|window| window == b"://")?;
    let scheme = &target[..scheme_end];
    if !scheme.eq_ignore_ascii_case(b"http") && !scheme.eq_ignore_ascii_case(b"https") {
        return None;
    }
    let rest = &target[scheme_end + 3..];
    let end = rest
        .iter()
        .position(|byte| matches!(byte, b'/' | b'?' | b'#'))
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(version: Version, target: &str, hosts: &[&str]) -> RequestHeader {
        let mut request = RequestHeader::build("GET", target.as_bytes(), None).unwrap();
        request.set_version(version);
        for host in hosts {
            request.append_header(HOST, *host).unwrap();
        }
        request
    }

    #[test]
    fn host_field_is_normalized() {
        let host = request_host(&request(Version::HTTP_11, "/", &["Example.COM.:8443"]))
            .unwrap()
            .unwrap();
        assert_eq!(host.name, "example.com");
        assert_eq!(host.port, Some(8443));
        let literal = request_host(&request(Version::HTTP_11, "/", &["[::1]:80"]))
            .unwrap()
            .unwrap();
        assert_eq!(literal.name, "[::1]");
    }

    #[test]
    fn framing_rules_reject_ambiguous_hosts() {
        assert_eq!(
            request_host(&request(Version::HTTP_11, "/", &[])),
            Err(HostError::Missing)
        );
        assert_eq!(
            request_host(&request(Version::HTTP_11, "/", &["a.example", "b.example"])),
            Err(HostError::Multiple)
        );
        for invalid in [
            "user@example.com",
            "exa mple.com",
            "[not-ipv6]",
            "example.com:port",
        ] {
            assert_eq!(
                request_host(&request(Version::HTTP_11, "/", &[invalid])),
                Err(HostError::Invalid),
                "{invalid}"
            );
        }
        assert_eq!(request_host(&request(Version::HTTP_10, "/", &[])), Ok(None));
        assert_eq!(
            request_host(&request(Version::HTTP_11, "/", &[""])),
            Ok(None)
        );
    }

    #[test]
    fn absolute_form_authority_wins() {
        let host = request_host(&request(
            Version::HTTP_11,
            "http://api.example.com/path",
            &["other.example"],
        ))
        .unwrap()
        .unwrap();
        assert_eq!(host.name, "api.example.com");
    }
}
