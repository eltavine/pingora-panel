//! The CloudEvents JSON format (`application/cloudevents+json`).
//!
//! Attributes, including extensions, are top-level members. Data whose
//! `datacontenttype` declares JSON (`*/json` or `*/*+json`, ignoring
//! parameters and case) is embedded as a JSON value in `data`; all other
//! data is binary and travels Base64-encoded in `data_base64`. An absent
//! `datacontenttype` implies `application/json`, and `null` members are
//! treated as unset.

use crate::attributes::{context_attributes, ReadAttributes, DATACONTENTTYPE};
use base64::{engine::general_purpose::STANDARD, Engine};
use panel_errors::{PanelError, Result};
use panel_events::{EventEnvelope, EventPayload, JSON_MEDIA_TYPE, MAX_EVENT_BYTES};
use serde_json::{Map, Value};

const DATA: &str = "data";
const DATA_BASE64: &str = "data_base64";

pub fn encode(envelope: &EventEnvelope) -> Result<Vec<u8>> {
    let mut object = Map::new();
    for attribute in context_attributes(envelope) {
        object.insert(attribute.name.to_owned(), Value::String(attribute.value));
    }
    let payload = envelope.payload();
    if payload.is_json() {
        let value = serde_json::from_slice::<Value>(payload.data()).map_err(|error| {
            PanelError::invalid_argument(format!("JSON event data is not valid JSON: {error}"))
        })?;
        object.insert(DATA.to_owned(), value);
    } else {
        object.insert(
            DATA_BASE64.to_owned(),
            Value::String(STANDARD.encode(payload.data())),
        );
    }
    serde_json::to_vec(&Value::Object(object)).map_err(|error| {
        PanelError::internal(format!("CloudEvent could not be serialized: {error}"))
    })
}

pub fn decode(bytes: &[u8]) -> Result<EventEnvelope> {
    if bytes.len() > MAX_EVENT_BYTES {
        return Err(PanelError::resource_exhausted(format!(
            "CloudEvent JSON exceeds {MAX_EVENT_BYTES} bytes"
        )));
    }
    let Value::Object(object) = serde_json::from_slice::<Value>(bytes).map_err(|error| {
        PanelError::invalid_argument(format!("CloudEvent is not valid JSON: {error}"))
    })?
    else {
        return Err(PanelError::invalid_argument(
            "a JSON CloudEvent must be an object",
        ));
    };
    let mut attributes = ReadAttributes::default();
    let mut data = None;
    let mut data_base64 = None;
    for (name, value) in object {
        match (name.as_str(), value) {
            (_, Value::Null) => {}
            (DATA, value) => data = Some(value),
            (DATA_BASE64, Value::String(encoded)) => data_base64 = Some(encoded),
            (DATA_BASE64, _) => {
                return Err(PanelError::invalid_argument(
                    "data_base64 must be a Base64 string",
                ))
            }
            (_, Value::String(value)) => attributes.insert(&name, value)?,
            (_, Value::Bool(value)) => attributes.insert(&name, value.to_string())?,
            (_, Value::Number(number)) => {
                let integer = number
                    .as_i64()
                    .and_then(|value| i32::try_from(value).ok())
                    .ok_or_else(|| {
                        PanelError::invalid_argument(format!(
                            "attribute {name} is not a 32-bit integer"
                        ))
                    })?;
                attributes.insert(&name, integer.to_string())?
            }
            (_, Value::Array(_) | Value::Object(_)) => {
                return Err(PanelError::invalid_argument(format!(
                    "attribute {name} is not a CloudEvents attribute type"
                )))
            }
        }
    }
    let bytes = match (data, data_base64) {
        (Some(_), Some(_)) => {
            return Err(PanelError::invalid_argument(
                "data and data_base64 are mutually exclusive",
            ))
        }
        (None, Some(encoded)) => STANDARD.decode(encoded).map_err(|error| {
            PanelError::invalid_argument(format!("data_base64 is not Base64: {error}"))
        })?,
        (Some(value), None) => {
            let content_type = attributes.content_type().unwrap_or(JSON_MEDIA_TYPE);
            if EventPayload::new(content_type, Vec::new())?.is_json() {
                serde_json::to_vec(&value).map_err(|error| {
                    PanelError::invalid_argument(format!("data could not be re-encoded: {error}"))
                })?
            } else if let Value::String(text) = value {
                text.into_bytes()
            } else {
                return Err(PanelError::invalid_argument(format!(
                    "non-JSON {DATACONTENTTYPE} requires string data"
                )));
            }
        }
        (None, None) => Vec::new(),
    };
    attributes.into_envelope(bytes, Some(JSON_MEDIA_TYPE))
}
