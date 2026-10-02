//! Mapping between NATS messages and events, following the CloudEvents NATS
//! protocol binding.

use async_nats::HeaderMap;
use panel_errors::Result;
use panel_event_codec::{binary, json};
use panel_events::EventEnvelope;
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, CONTROLS};

const CONTENT_TYPE: &str = "Content-Type";
/// NATS reserves the `Nats-` prefix for system headers.
const SYSTEM_HEADER_PREFIX: &str = "nats-";
const HEADER_VALUE: &AsciiSet = &CONTROLS.add(b' ').add(b'"').add(b'%');

/// Binary content mode headers for an event: every context attribute as a
/// percent-encoded `ce-` header.
pub(crate) fn event_headers(envelope: &EventEnvelope) -> (HeaderMap, Vec<u8>) {
    let message = binary::encode(envelope);
    let mut headers = HeaderMap::new();
    for (name, value) in message.headers {
        headers.insert(name.as_str(), value.as_str());
    }
    (headers, message.data)
}

/// Decodes a message in either content mode. Structured mode is recognised
/// by an `application/cloudevents` content type and uses the JSON format, as
/// the binding requires; everything else is binary mode.
pub(crate) fn decode_event(headers: Option<&HeaderMap>, payload: &[u8]) -> Result<EventEnvelope> {
    let content_type = headers.and_then(|headers| header_value(headers, CONTENT_TYPE));
    if binary::is_structured(content_type) {
        return json::decode(payload);
    }
    let pairs = headers
        .into_iter()
        .flat_map(|headers| headers.iter())
        .flat_map(|(name, values)| {
            values
                .iter()
                .map(move |value| (name.as_ref(), value.as_str()))
        });
    binary::decode(pairs, payload)
}

/// First value of a header, matching the name case-insensitively.
pub(crate) fn header_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(candidate, _)| AsRef::<str>::as_ref(*candidate).eq_ignore_ascii_case(name))
        .and_then(|(_, values)| values.first())
        .map(|value| value.as_str())
}

/// Copies application headers, dropping broker system headers.
pub(crate) fn application_headers(headers: Option<&HeaderMap>, skip_prefix: &str) -> HeaderMap {
    let mut copied = HeaderMap::new();
    for (name, values) in headers.into_iter().flat_map(|headers| headers.iter()) {
        let lowered = AsRef::<str>::as_ref(name).to_ascii_lowercase();
        if lowered.starts_with(SYSTEM_HEADER_PREFIX) || lowered.starts_with(skip_prefix) {
            continue;
        }
        for value in values {
            copied.append(AsRef::<str>::as_ref(name), value.as_str());
        }
    }
    copied
}

pub(crate) fn encode_header_value(value: &str) -> String {
    utf8_percent_encode(value, HEADER_VALUE).to_string()
}

pub(crate) fn decode_header_value(value: &str) -> String {
    percent_decode_str(value).decode_utf8_lossy().into_owned()
}
