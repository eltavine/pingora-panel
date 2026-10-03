use iri_string::types::UriStr;
use mediatype::{MediaType, Name, ReadParams};
use panel_errors::{PanelError, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

/// Largest event data. CloudEvents intermediaries must forward events of up
/// to 64 KiB, so data stays below that with room for every bounded attribute.
pub const MAX_DATA_BYTES: usize = 60 * 1024;
/// Upper bound of a whole encoded event in any supported format.
pub const MAX_EVENT_BYTES: usize = 64 * 1024;
pub const JSON_MEDIA_TYPE: &str = "application/json";
/// RFC 9996 media type for binary protobuf serializations.
pub const PROTOBUF_MEDIA_TYPE: &str = "application/protobuf";
/// Conventional type URL authority for protobuf messages; with the `https`
/// scheme the type URL is also an absolute URI as `dataschema` requires.
pub const PROTOBUF_TYPE_URL_PREFIX: &str = "https://type.googleapis.com/";
const MAX_CONTENT_TYPE_BYTES: usize = 255;
const MAX_SCHEMA_BYTES: usize = 512;

/// The data of one event type: its name, and the schema its JSON follows.
pub trait EventData: Serialize {
    /// Such as `identity.account.created`.
    const TYPE: &'static str;

    /// The data's `dataschema`, an absolute URI, when it has one.
    fn schema() -> Option<String> {
        None
    }
}

/// Event data with its CloudEvents `datacontenttype` and `dataschema`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EventPayload {
    content_type: String,
    schema: Option<String>,
    data: Vec<u8>,
}

impl EventPayload {
    /// `content_type` must be an RFC 6838 media type.
    pub fn new(content_type: impl Into<String>, data: Vec<u8>) -> Result<Self> {
        let content_type = content_type.into();
        validate_media_type(&content_type)?;
        if data.len() > MAX_DATA_BYTES {
            return Err(PanelError::resource_exhausted(format!(
                "event data exceeds {MAX_DATA_BYTES} bytes"
            )));
        }
        Ok(Self {
            content_type,
            schema: None,
            data,
        })
    }

    /// Sets `dataschema`, which must be an absolute RFC 3986 URI.
    pub fn with_schema(mut self, schema: impl Into<String>) -> Result<Self> {
        let schema = schema.into();
        if schema.len() > MAX_SCHEMA_BYTES || UriStr::new(&schema).is_err() {
            return Err(PanelError::invalid_argument(format!(
                "data schema must be an absolute URI of at most {MAX_SCHEMA_BYTES} bytes"
            )));
        }
        self.schema = Some(schema);
        Ok(self)
    }

    pub fn json<T: Serialize>(value: &T) -> Result<Self> {
        let data = serde_json::to_vec(value).map_err(|error| {
            PanelError::invalid_argument(format!("event data is not serializable: {error}"))
        })?;
        Self::new(JSON_MEDIA_TYPE, data)
    }

    /// The JSON of `data`, with its `dataschema` when it has one.
    pub fn of<E: EventData>(data: &E) -> Result<Self> {
        let payload = Self::json(data)?;
        match E::schema() {
            Some(schema) => payload.with_schema(schema),
            None => Ok(payload),
        }
    }

    /// Wraps a binary protobuf message identified by its fully qualified name.
    pub fn protobuf(message_name: &str, data: Vec<u8>) -> Result<Self> {
        if !is_protobuf_full_name(message_name) {
            return Err(PanelError::invalid_argument(
                "protobuf message names must be fully qualified identifiers",
            ));
        }
        Self::new(PROTOBUF_MEDIA_TYPE, data)?
            .with_schema(format!("{PROTOBUF_TYPE_URL_PREFIX}{message_name}"))
    }

    pub fn content_type(&self) -> &str {
        &self.content_type
    }

    pub fn schema(&self) -> Option<&str> {
        self.schema.as_deref()
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn into_data(self) -> Vec<u8> {
        self.data
    }

    /// Whether the data is JSON by the CloudEvents rule: a media subtype of
    /// `json` or a `+json` structured syntax suffix.
    pub fn is_json(&self) -> bool {
        MediaType::parse(&self.content_type).is_ok_and(|media| {
            media.subty.as_str().eq_ignore_ascii_case("json")
                || media
                    .suffix
                    .is_some_and(|suffix| suffix.as_str().eq_ignore_ascii_case("json"))
        })
    }

    pub fn decode_json<T: DeserializeOwned>(&self) -> Result<T> {
        if !self.is_json() {
            return Err(PanelError::invalid_argument(format!(
                "event data of type {} is not JSON",
                self.content_type
            )));
        }
        serde_json::from_slice(&self.data).map_err(|error| {
            PanelError::invalid_argument(format!("event data is not valid JSON: {error}"))
        })
    }

    /// Returns the protobuf message name when the data is an RFC 9996
    /// binary protobuf payload with a type URL schema. Unsupported
    /// `encoding` or `version` parameters are rejected as RFC 9996 requires.
    pub fn protobuf_message_name(&self) -> Result<&str> {
        let media = MediaType::parse(&self.content_type)
            .map_err(|_| PanelError::invalid_argument("event data has no media type"))?;
        if !media.ty.as_str().eq_ignore_ascii_case("application")
            || !media.subty.as_str().eq_ignore_ascii_case("protobuf")
            || media.suffix.is_some()
        {
            return Err(PanelError::invalid_argument(format!(
                "event data of type {} is not binary protobuf",
                self.content_type
            )));
        }
        for (name, expected) in [("encoding", "binary"), ("version", "1")] {
            let parameter = Name::new(name).expect("parameter names are valid tokens");
            if let Some(value) = media.get_param(parameter) {
                if !value.unquoted_str().eq_ignore_ascii_case(expected) {
                    return Err(PanelError::invalid_argument(format!(
                        "unsupported protobuf {name} parameter {value}"
                    )));
                }
            }
        }
        self.schema
            .as_deref()
            .and_then(|schema| schema.rsplit_once('/'))
            .map(|(_, name)| name)
            .filter(|name| is_protobuf_full_name(name))
            .ok_or_else(|| {
                PanelError::invalid_argument("protobuf event data has no type URL schema")
            })
    }
}

fn validate_media_type(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > MAX_CONTENT_TYPE_BYTES || MediaType::parse(value).is_err()
    {
        return Err(PanelError::invalid_argument(format!(
            "content type must be an RFC 6838 media type of at most {MAX_CONTENT_TYPE_BYTES} bytes"
        )));
    }
    Ok(())
}

fn is_protobuf_full_name(value: &str) -> bool {
    let mut segments = 0;
    let valid = value.split('.').all(|segment| {
        segments += 1;
        segment
            .bytes()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    });
    valid && segments >= 2 && value.len() <= 256
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_types_follow_rfc_6838() {
        assert!(EventPayload::new("application/json", Vec::new()).is_ok());
        assert!(EventPayload::new("text/plain; charset=utf-8", Vec::new()).is_ok());
        assert!(EventPayload::new("", Vec::new()).is_err());
        assert!(EventPayload::new("json", Vec::new()).is_err());
        assert!(EventPayload::new("application/", Vec::new()).is_err());
        assert!(EventPayload::new("text/plain", vec![0; MAX_DATA_BYTES + 1]).is_err());
    }

    #[test]
    fn json_detection_uses_subtype_or_suffix() {
        let json = EventPayload::json(&serde_json::json!({"a": 1})).unwrap();
        assert!(json.is_json());
        assert_eq!(json.decode_json::<serde_json::Value>().unwrap()["a"], 1);
        assert!(
            EventPayload::new("application/problem+json", b"{}".to_vec())
                .unwrap()
                .is_json()
        );
        assert!(
            EventPayload::new("Application/JSON; charset=utf-8", b"{}".to_vec())
                .unwrap()
                .is_json()
        );
        assert!(!EventPayload::new("text/plain", b"{}".to_vec())
            .unwrap()
            .is_json());
    }

    #[test]
    fn protobuf_payloads_use_rfc_9996_and_type_urls() {
        let payload = EventPayload::protobuf("pingora.panel.jobs.v1.Progress", vec![8, 1]).unwrap();
        assert_eq!(payload.content_type(), "application/protobuf");
        assert_eq!(
            payload.schema(),
            Some("https://type.googleapis.com/pingora.panel.jobs.v1.Progress")
        );
        assert_eq!(
            payload.protobuf_message_name().unwrap(),
            "pingora.panel.jobs.v1.Progress"
        );
        assert!(EventPayload::protobuf("Progress", Vec::new()).is_err());
        assert!(EventPayload::protobuf("pingora..Progress", Vec::new()).is_err());

        let versioned =
            EventPayload::new("application/protobuf; encoding=binary; version=1", vec![])
                .unwrap()
                .with_schema("https://type.googleapis.com/a.B")
                .unwrap();
        assert_eq!(versioned.protobuf_message_name().unwrap(), "a.B");
        for content_type in [
            "application/protobuf; version=2",
            "application/protobuf; encoding=json",
            "application/x-protobuf",
            "application/protobuf+json; charset=utf-8",
        ] {
            let payload = EventPayload::new(content_type, vec![])
                .unwrap()
                .with_schema("https://type.googleapis.com/a.B")
                .unwrap();
            assert!(payload.protobuf_message_name().is_err(), "{content_type}");
        }
    }

    #[test]
    fn data_schema_must_be_an_absolute_uri() {
        let payload = EventPayload::new("application/json", Vec::new()).unwrap();
        assert!(payload
            .clone()
            .with_schema("https://example.com/schema.json")
            .is_ok());
        assert!(payload
            .clone()
            .with_schema("urn:uuid:6e8bc430-9c3a-11d9-9669-0800200c9a66")
            .is_ok());
        assert!(payload.clone().with_schema("/relative/schema").is_err());
        assert!(payload.with_schema("type.googleapis.com/a.B").is_err());
    }
}
