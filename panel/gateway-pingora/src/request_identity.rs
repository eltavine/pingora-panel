//! The ID every request carries and the trace it belongs to (ADR 0025).

use http::HeaderMap;
use pingora_http::RequestHeader;

pub(crate) const REQUEST_ID: &str = "x-request-id";
/// The longest `X-Request-Id` kept from a request.
const MAX_REQUEST_ID: usize = 128;
const TRACEPARENT: &str = "traceparent";

/// Whether `id` is worth keeping as a request ID: a short visible token.
pub(crate) fn is_request_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_REQUEST_ID && id.bytes().all(|byte| byte.is_ascii_graphic())
}

/// The request's ID, once [`ensure_request_id`] has run.
pub(crate) fn request_id(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(REQUEST_ID)
        .and_then(|value| value.to_str().ok())
        .filter(|id| is_request_id(id))
}

/// Keeps the request's one `X-Request-Id` when it is a request ID, and
/// otherwise writes a new UUIDv7 in its place, so templates, the upstream
/// and the log see the same ID.
pub(crate) fn ensure_request_id(request: &mut RequestHeader) {
    if request.headers.get_all(REQUEST_ID).iter().count() == 1
        && request_id(&request.headers).is_some()
    {
        return;
    }
    let id = uuid::Uuid::now_v7().to_string();
    request
        .insert_header(REQUEST_ID, id)
        .expect("a UUID is a valid header value");
}

/// The trace ID of a valid W3C `traceparent`.
pub(crate) fn trace_id(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(TRACEPARENT)?.to_str().ok()?;
    let mut parts = value.split('-');
    let (version, trace, parent, flags) =
        (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    let hex = |text: &str, length: usize| {
        text.len() == length
            && text
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    };
    let nonzero = |text: &str| text.bytes().any(|byte| byte != b'0');
    (hex(version, 2)
        && version != "ff"
        && hex(trace, 32)
        && nonzero(trace)
        && hex(parent, 16)
        && nonzero(parent)
        && hex(flags, 2)
        && (version != "00" || parts.next().is_none()))
    .then_some(trace)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(ids: &[&str]) -> RequestHeader {
        let mut request = RequestHeader::build("GET", b"/", None).unwrap();
        for id in ids {
            request.append_header(REQUEST_ID, *id).unwrap();
        }
        request
    }

    #[test]
    fn a_valid_request_id_is_kept() {
        let mut kept = request(&["req-7"]);
        ensure_request_id(&mut kept);
        assert_eq!(request_id(&kept.headers), Some("req-7"));
    }

    #[test]
    fn missing_invalid_and_repeated_ids_are_replaced_by_one_new_id() {
        let long = "x".repeat(MAX_REQUEST_ID + 1);
        for ids in [&[][..], &["has space"], &[long.as_str()], &["a", "b"]] {
            let mut replaced = request(ids);
            ensure_request_id(&mut replaced);
            assert_eq!(replaced.headers.get_all(REQUEST_ID).iter().count(), 1);
            let id = request_id(&replaced.headers).unwrap();
            assert_eq!(uuid::Uuid::parse_str(id).unwrap().get_version_num(), 7);
        }
    }

    #[test]
    fn only_a_valid_traceparent_names_a_trace() {
        let headers = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(TRACEPARENT, value.parse().unwrap());
            headers
        };
        let valid = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        assert_eq!(
            trace_id(&headers(valid)),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );
        assert_eq!(
            trace_id(&headers(&format!("01{}-extra", &valid[2..]))),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );
        for invalid in [
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01",
            "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01",
            "ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-extra",
            "00-4bf92f3577b34da6a3ce929d0e0e4736",
        ] {
            assert_eq!(trace_id(&headers(invalid)), None, "{invalid}");
        }
        assert_eq!(trace_id(&HeaderMap::new()), None);
    }
}
