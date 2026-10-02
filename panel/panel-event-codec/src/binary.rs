//! Binary content mode: context attributes as `ce-` prefixed headers and the
//! event data as the message body.
//!
//! Header values use the canonical string encoding, percent-encoded as the
//! CloudEvents NATS and HTTP protocol bindings require: space, double quote,
//! percent and every byte outside printable ASCII are encoded with
//! upper-case hexadecimal. Decoding first removes RFC 7230 quoted-string
//! escaping, then performs one round of percent-decoding and rejects byte
//! sequences that are not valid UTF-8.

use crate::attributes::{context_attributes, ReadAttributes};
use mediatype::MediaType;
use panel_errors::{PanelError, Result};
use panel_events::{EventEnvelope, MAX_EVENT_BYTES};
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, CONTROLS};

/// Header name prefix of every context attribute.
pub const HEADER_PREFIX: &str = "ce-";

/// Characters that must be percent-encoded in addition to non-ASCII bytes.
const HEADER_VALUE: &AsciiSet = &CONTROLS.add(b' ').add(b'"').add(b'%');

/// A binary content mode message ready for a protocol binding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryMessage {
    pub headers: Vec<(String, String)>,
    pub data: Vec<u8>,
}

pub fn encode(envelope: &EventEnvelope) -> BinaryMessage {
    BinaryMessage {
        headers: context_attributes(envelope)
            .into_iter()
            .map(|attribute| {
                (
                    format!("{HEADER_PREFIX}{}", attribute.name),
                    utf8_percent_encode(&attribute.value, HEADER_VALUE).to_string(),
                )
            })
            .collect(),
        data: envelope.payload().data().to_vec(),
    }
}

/// Decodes a binary content mode message. Header names are matched
/// case-insensitively; headers without the `ce-` prefix are not attributes.
pub fn decode<'a, I>(headers: I, data: &[u8]) -> Result<EventEnvelope>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut size = data.len();
    let mut attributes = ReadAttributes::default();
    for (name, value) in headers {
        size = size.saturating_add(name.len() + value.len());
        let Some(attribute) = name
            .get(..HEADER_PREFIX.len())
            .filter(|prefix| prefix.eq_ignore_ascii_case(HEADER_PREFIX))
            .map(|_| name[HEADER_PREFIX.len()..].to_ascii_lowercase())
        else {
            continue;
        };
        attributes.insert(&attribute, decode_value(value)?)?;
    }
    if size > MAX_EVENT_BYTES {
        return Err(PanelError::resource_exhausted(format!(
            "CloudEvent message exceeds {MAX_EVENT_BYTES} bytes"
        )));
    }
    attributes.into_envelope(data.to_vec(), None)
}

/// Whether a message is in structured content mode: its content type is an
/// `application/cloudevents` media type, matched case-insensitively.
pub fn is_structured(content_type: Option<&str>) -> bool {
    content_type
        .and_then(|value| MediaType::parse(value).ok())
        .is_some_and(|media| {
            media.ty.as_str().eq_ignore_ascii_case("application")
                && media.subty.as_str().eq_ignore_ascii_case("cloudevents")
        })
}

fn decode_value(value: &str) -> Result<String> {
    let value = value.trim_matches([' ', '\t']);
    let mut unquoted = String::with_capacity(value.len());
    let mut characters = value.chars();
    let mut quoted = false;
    while let Some(character) = characters.next() {
        match (quoted, character) {
            (_, '"') => quoted = !quoted,
            (true, '\\') => unquoted.push(characters.next().ok_or_else(|| {
                PanelError::invalid_argument("header value ends inside an escape")
            })?),
            (_, character) => unquoted.push(character),
        }
    }
    if quoted {
        return Err(PanelError::invalid_argument(
            "header value has an unterminated quoted string",
        ));
    }
    percent_decode_str(&unquoted)
        .decode_utf8()
        .map(|value| value.into_owned())
        .map_err(|_| PanelError::invalid_argument("header value is not percent-encoded UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_follow_the_binding_percent_encoding_rules() {
        assert_eq!(
            utf8_percent_encode("Euro \u{20AC} \u{1F600}", HEADER_VALUE).to_string(),
            "Euro%20%E2%82%AC%20%F0%9F%98%80"
        );
        assert_eq!(
            utf8_percent_encode("a\"b%c\td", HEADER_VALUE).to_string(),
            "a%22b%25c%09d"
        );
        assert_eq!(decode_value("Euro%20%e2%82%ac").unwrap(), "Euro \u{20AC}");
        assert_eq!(decode_value("%41BC").unwrap(), "ABC");
        assert_eq!(
            decode_value("\"quoted value\\\"x\"").unwrap(),
            "quoted value\"x"
        );
        assert!(decode_value("%C0%A0").is_err());
        assert!(decode_value("\"open").is_err());
    }

    #[test]
    fn structured_mode_is_detected_by_media_type() {
        assert!(is_structured(Some("application/cloudevents+json")));
        assert!(is_structured(Some(
            "Application/CloudEvents+JSON; charset=utf-8"
        )));
        assert!(!is_structured(Some("application/json")));
        assert!(!is_structured(None));
    }
}
