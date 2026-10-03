//! The canonical form of an audit record and the hash that chains records.

use base64::Engine as _;
use chrono::{DateTime, SecondsFormat, Utc};
use panel_events::EventEnvelope;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// The stored fields of one record, as hashed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    pub sequence: u64,
    pub event_id: String,
    pub source: String,
    pub event_type: String,
    pub event_version: u32,
    pub subject: String,
    pub occurred_at: DateTime<Utc>,
    pub recorded_at: DateTime<Utc>,
    pub actor_type: String,
    pub actor_id: String,
    pub correlation_id: String,
    pub causation_id: String,
    pub idempotency_key: String,
    pub traceparent: String,
    /// Canonical JSON text of the event data.
    pub data: String,
}

impl Entry {
    /// The record of `event` at `sequence`. Times keep microseconds, as
    /// PostgreSQL stores them, so a stored record hashes the same.
    pub fn from_event(sequence: u64, event: &EventEnvelope, recorded_at: DateTime<Utc>) -> Self {
        let principal = event.principal();
        Self {
            sequence,
            event_id: event.event_id().to_string(),
            source: event.source(),
            event_type: event.event_type().as_str().to_owned(),
            event_version: event.event_version().get(),
            subject: event.aggregate().subject(),
            occurred_at: microseconds(event.occurred_at()),
            recorded_at: microseconds(recorded_at),
            actor_type: principal.kind().as_str().to_owned(),
            actor_id: principal
                .id()
                .map(|id| id.as_str().to_owned())
                .unwrap_or_default(),
            correlation_id: event.correlation_id().as_str().to_owned(),
            causation_id: event.causation_id().as_str().to_owned(),
            idempotency_key: event
                .idempotency_key()
                .map(|key| key.as_str().to_owned())
                .unwrap_or_default(),
            traceparent: event
                .trace_context()
                .map(|trace| trace.traceparent().to_owned())
                .unwrap_or_default(),
            data: data_text(event),
        }
    }

    /// A JSON object of every field with sorted keys, the event data
    /// embedded as its canonical text.
    pub fn canonical(&self) -> Vec<u8> {
        let fields = [
            ("actor_id", Value::from(self.actor_id.as_str())),
            ("actor_type", Value::from(self.actor_type.as_str())),
            ("causation_id", Value::from(self.causation_id.as_str())),
            ("correlation_id", Value::from(self.correlation_id.as_str())),
            ("data", Value::from(self.data.as_str())),
            ("event_id", Value::from(self.event_id.as_str())),
            ("event_type", Value::from(self.event_type.as_str())),
            ("event_version", Value::from(self.event_version)),
            (
                "idempotency_key",
                Value::from(self.idempotency_key.as_str()),
            ),
            ("occurred_at", Value::from(timestamp(self.occurred_at))),
            ("recorded_at", Value::from(timestamp(self.recorded_at))),
            ("sequence", Value::from(self.sequence)),
            ("source", Value::from(self.source.as_str())),
            ("subject", Value::from(self.subject.as_str())),
            ("traceparent", Value::from(self.traceparent.as_str())),
        ];
        let object: Map<String, Value> = fields
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect();
        serde_json::to_vec(&Value::Object(object)).expect("JSON values serialize")
    }

    /// The record's hash given the previous record's, empty for the first.
    pub fn hash(&self, previous: &str) -> String {
        let mut digest = Sha256::new();
        digest.update(previous.as_bytes());
        digest.update(b"\n");
        digest.update(self.canonical());
        hex::encode(digest.finalize())
    }
}

fn microseconds(time: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(time.timestamp_micros()).unwrap_or(time)
}

fn timestamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// JSON data with its object keys sorted at every level; other content
/// types as an object of the type and base64 data.
fn data_text(event: &EventEnvelope) -> String {
    let payload = event.payload();
    let value = if payload.is_json() {
        serde_json::from_slice(payload.data()).unwrap_or(Value::Null)
    } else {
        serde_json::json!({
            "content_type": payload.content_type(),
            "data_base64": base64::engine::general_purpose::STANDARD.encode(payload.data()),
        })
    };
    serde_json::to_string(&sorted(value)).expect("JSON values serialize")
}

fn sorted(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries: Vec<_> = object.into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, sorted(value)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> Entry {
        Entry {
            sequence: 1,
            event_id: "0190b5b6-3f43-7a52-8a56-2f8b7a7d5a10".into(),
            source: "/pingora-panel/config-service".into(),
            event_type: "config.draft.changed".into(),
            event_version: 1,
            subject: "configuration/draft".into(),
            occurred_at: DateTime::from_timestamp_micros(1_800_000_000_123_456).unwrap(),
            recorded_at: DateTime::from_timestamp_micros(1_800_000_001_000_000).unwrap(),
            actor_type: "user".into(),
            actor_id: "ops".into(),
            correlation_id: "req-1".into(),
            causation_id: "req-1".into(),
            idempotency_key: "key-1".into(),
            traceparent: String::new(),
            data: r#"{"operation":"sites.create","version":2}"#.into(),
        }
    }

    #[test]
    fn the_canonical_form_has_sorted_keys_and_microsecond_times() {
        let text = String::from_utf8(entry().canonical()).unwrap();
        assert!(
            text.starts_with(r#"{"actor_id":"ops","actor_type":"user","#),
            "{text}"
        );
        assert!(
            text.contains(r#""occurred_at":"2027-01-15T08:00:00.123456Z""#),
            "{text}"
        );
        assert!(text.ends_with(r#""traceparent":""}"#), "{text}");
    }

    #[test]
    fn each_hash_depends_on_the_previous_one_and_every_field() {
        let first = entry();
        let genesis = first.hash("");
        assert_eq!(genesis.len(), 64);
        assert_eq!(genesis, first.hash(""));
        assert_ne!(genesis, first.hash("0".repeat(64).as_str()));
        let mut changed = first.clone();
        changed.actor_id = "mallory".into();
        assert_ne!(genesis, changed.hash(""));
    }

    #[test]
    fn data_objects_are_sorted_at_every_level() {
        let value = serde_json::json!({"b": {"y": 1, "x": [ {"d": 1, "c": 2} ]}, "a": true});
        assert_eq!(
            serde_json::to_string(&sorted(value)).unwrap(),
            r#"{"a":true,"b":{"x":[{"c":2,"d":1}],"y":1}}"#
        );
    }
}
