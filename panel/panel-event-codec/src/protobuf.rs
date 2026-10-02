//! The CloudEvents Protobuf format (`application/cloudevents+protobuf`).
//!
//! Required attributes are message fields; optional and extension attributes
//! use the typed attribute map. Protobuf data travels as `proto_data`, text
//! data (including JSON) as `text_data` and everything else as `binary_data`.

use crate::attributes::{self, context_attributes, Kind, ReadAttributes};
use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::DateTime;
use mediatype::MediaType;
use panel_contracts::cloudevents::v1::{
    cloud_event::{cloud_event_attribute_value::Attr, CloudEventAttributeValue, Data},
    CloudEvent,
};
use panel_errors::{PanelError, Result};
use panel_events::{EventEnvelope, MAX_EVENT_BYTES, PROTOBUF_MEDIA_TYPE, PROTOBUF_TYPE_URL_PREFIX};
use prost::Message;

/// Type URL authority used inside `google.protobuf.Any`.
const ANY_TYPE_URL_PREFIX: &str = "type.googleapis.com/";

pub fn encode(envelope: &EventEnvelope) -> Vec<u8> {
    to_message(envelope).encode_to_vec()
}

pub fn decode(bytes: &[u8]) -> Result<EventEnvelope> {
    if bytes.len() > MAX_EVENT_BYTES {
        return Err(PanelError::resource_exhausted(format!(
            "encoded CloudEvent exceeds {MAX_EVENT_BYTES} bytes"
        )));
    }
    let message = CloudEvent::decode(bytes).map_err(|error| {
        PanelError::invalid_argument(format!("CloudEvent is not valid protobuf: {error}"))
    })?;
    from_message(message)
}

pub fn to_message(envelope: &EventEnvelope) -> CloudEvent {
    let mut message = CloudEvent::default();
    for attribute in context_attributes(envelope) {
        match attribute.name {
            attributes::ID => message.id = attribute.value,
            attributes::SOURCE => message.source = attribute.value,
            attributes::SPECVERSION => message.spec_version = attribute.value,
            attributes::TYPE => message.r#type = attribute.value,
            name => {
                let attr = match attribute.kind {
                    Kind::String => Attr::CeString(attribute.value),
                    Kind::UriReference => Attr::CeUriRef(attribute.value),
                    Kind::Uri => Attr::CeUri(attribute.value),
                    Kind::Timestamp => Attr::CeTimestamp(timestamp(envelope)),
                };
                message.attributes.insert(
                    name.to_owned(),
                    CloudEventAttributeValue { attr: Some(attr) },
                );
            }
        }
    }
    message.data = Some(data(envelope));
    message
}

pub fn from_message(message: CloudEvent) -> Result<EventEnvelope> {
    let CloudEvent {
        id,
        source,
        spec_version,
        r#type,
        attributes: optional,
        data,
    } = message;
    let mut read = ReadAttributes::default();
    read.insert(attributes::ID, id)?;
    read.insert(attributes::SOURCE, source)?;
    read.insert(attributes::SPECVERSION, spec_version)?;
    read.insert(attributes::TYPE, r#type)?;
    for (name, value) in optional {
        read.insert(&name, canonical_string(value)?)?;
    }
    let data = match data {
        Some(Data::BinaryData(bytes)) => bytes,
        Some(Data::TextData(text)) => text.into_bytes(),
        Some(Data::ProtoData(any)) => {
            let name = any
                .type_url
                .rsplit_once('/')
                .map(|(_, name)| name)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| PanelError::invalid_argument("proto_data has no type URL"))?;
            if read.content_type().is_none() {
                read.insert(attributes::DATACONTENTTYPE, PROTOBUF_MEDIA_TYPE.to_owned())?;
            }
            match read.schema() {
                Some(schema) if !schema.ends_with(&format!("/{name}")) => {
                    return Err(PanelError::invalid_argument(
                        "dataschema does not name the proto_data message type",
                    ));
                }
                Some(_) => {}
                None => read.insert(
                    attributes::DATASCHEMA,
                    format!("{PROTOBUF_TYPE_URL_PREFIX}{name}"),
                )?,
            }
            any.value
        }
        None => Vec::new(),
    };
    read.into_envelope(data, None)
}

fn timestamp(envelope: &EventEnvelope) -> prost_types::Timestamp {
    let time = envelope.occurred_at();
    prost_types::Timestamp {
        seconds: time.timestamp(),
        nanos: i32::try_from(time.timestamp_subsec_nanos())
            .expect("sub-second nanoseconds fit in i32"),
    }
}

fn data(envelope: &EventEnvelope) -> Data {
    let payload = envelope.payload();
    if let Ok(name) = payload.protobuf_message_name() {
        return Data::ProtoData(prost_types::Any {
            type_url: format!("{ANY_TYPE_URL_PREFIX}{name}"),
            value: payload.data().to_vec(),
        });
    }
    let textual = payload.is_json()
        || MediaType::parse(payload.content_type())
            .is_ok_and(|media| media.ty.as_str().eq_ignore_ascii_case("text"));
    match std::str::from_utf8(payload.data()) {
        Ok(text) if textual => Data::TextData(text.to_owned()),
        _ => Data::BinaryData(payload.data().to_vec()),
    }
}

/// The canonical string encoding of any attribute value type.
fn canonical_string(value: CloudEventAttributeValue) -> Result<String> {
    Ok(match value.attr {
        Some(Attr::CeBoolean(value)) => value.to_string(),
        Some(Attr::CeInteger(value)) => value.to_string(),
        Some(Attr::CeString(value) | Attr::CeUri(value) | Attr::CeUriRef(value)) => value,
        Some(Attr::CeBytes(value)) => STANDARD.encode(value),
        Some(Attr::CeTimestamp(value)) => u32::try_from(value.nanos)
            .ok()
            .and_then(|nanos| DateTime::from_timestamp(value.seconds, nanos))
            .map(attributes::format_time)
            .ok_or_else(|| PanelError::invalid_argument("CloudEvent timestamp is out of range"))?,
        None => {
            return Err(PanelError::invalid_argument(
                "CloudEvent attribute has no value",
            ))
        }
    })
}
