use crate::{binary, json, protobuf};
use chrono::{DateTime, Utc};
use panel_contracts::cloudevents::v1::cloud_event::Data;
use panel_events::{
    Actor, AggregateId, AggregateRef, AggregateType, EventDraft, EventEnvelope, EventEnvelopeParts,
    EventId, EventOrigin, EventPayload, EventType, EventVersion, IdempotencyKey, Principal,
    RequestId, ServiceName, TraceContext, MAX_DATA_BYTES,
};
use prost::Message;

const PROTOBUF_GOLDEN_V1: &str = include_str!("../tests/fixtures/v1/event.cloudevents.pb.hex");
const JSON_GOLDEN_V1: &str = include_str!("../tests/fixtures/v1/event.cloudevents.json");

fn fixed_envelope(payload: EventPayload) -> EventEnvelope {
    EventEnvelope::from_parts(EventEnvelopeParts {
        event_id: EventId::parse("01928f6c-4e3a-7b5c-8d9e-0f1a2b3c4d5e").unwrap(),
        event_type: EventType::new("config.revision.activated").unwrap(),
        event_version: EventVersion::V1,
        occurred_at: DateTime::from_timestamp(1_790_000_000, 123_456_789).unwrap(),
        producer: ServiceName::new("config-service").unwrap(),
        aggregate: AggregateRef::new(
            AggregateType::new("revision").unwrap(),
            AggregateId::new("42").unwrap(),
        ),
        correlation_id: RequestId::new("corr-1").unwrap(),
        causation_id: RequestId::new("req-1").unwrap(),
        principal: Principal::user(Actor::new("user-7").unwrap()),
        idempotency_key: Some(IdempotencyKey::new("apply-42").unwrap()),
        trace_context: TraceContext::parse(
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            Some("rojo=00f067aa0ba902b7, congo=t61rcWkgMzE"),
        ),
        payload,
    })
}

fn json_event() -> EventEnvelope {
    fixed_envelope(EventPayload::new("application/json", br#"{"revision":42}"#.to_vec()).unwrap())
}

fn protobuf_event() -> EventEnvelope {
    fixed_envelope(
        EventPayload::protobuf("pingora.panel.config.v1.RevisionActivated", vec![8, 42]).unwrap(),
    )
}

fn binary_event() -> EventEnvelope {
    fixed_envelope(EventPayload::new("application/octet-stream", vec![0, 159, 146, 150]).unwrap())
}

fn from_hex(value: &str) -> Vec<u8> {
    let value = value.trim();
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).unwrap())
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn every_representation_round_trips_every_payload_kind() {
    for event in [json_event(), protobuf_event(), binary_event()] {
        assert_eq!(protobuf::decode(&protobuf::encode(&event)).unwrap(), event);
        let message = binary::encode(&event);
        let headers = message
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()));
        assert_eq!(binary::decode(headers, &message.data).unwrap(), event);
        assert_eq!(json::decode(&json::encode(&event).unwrap()).unwrap(), event);
    }
}

#[test]
fn follow_up_events_keep_their_causal_chain_on_the_wire() {
    let parent = json_event();
    let child = EventEnvelope::new(
        EventDraft::new(
            EventType::new("audit.record.appended").unwrap(),
            EventVersion::new(2).unwrap(),
            AggregateRef::new(
                AggregateType::new("audit_record").unwrap(),
                AggregateId::new("9").unwrap(),
            ),
            EventPayload::json(&serde_json::json!({"sequence": 9})).unwrap(),
        ),
        EventOrigin::caused_by(ServiceName::new("audit-writer").unwrap(), &parent),
        Utc::now(),
    );
    let decoded = json::decode(&json::encode(&child).unwrap()).unwrap();
    assert_eq!(decoded.correlation_id().as_str(), "corr-1");
    assert_eq!(
        decoded.causation_id().as_str(),
        parent.event_id().to_string()
    );
    assert_eq!(decoded.trace_context(), parent.trace_context());
    assert_eq!(decoded.event_version().get(), 2);
}

#[test]
fn version_one_protobuf_bytes_are_stable() {
    let event = json_event();
    assert_eq!(hex(&protobuf::encode(&event)), PROTOBUF_GOLDEN_V1.trim());
    assert_eq!(
        protobuf::decode(&from_hex(PROTOBUF_GOLDEN_V1)).unwrap(),
        event
    );
}

#[test]
fn version_one_json_document_is_stable() {
    let event = json_event();
    assert_eq!(
        String::from_utf8(json::encode(&event).unwrap()).unwrap(),
        JSON_GOLDEN_V1.trim()
    );
    assert_eq!(json::decode(JSON_GOLDEN_V1.as_bytes()).unwrap(), event);
}

#[test]
fn protobuf_data_uses_the_format_specific_members() {
    let message = protobuf::to_message(&protobuf_event());
    let Some(Data::ProtoData(any)) = message.data else {
        panic!("protobuf data must use proto_data");
    };
    assert_eq!(
        any.type_url,
        "type.googleapis.com/pingora.panel.config.v1.RevisionActivated"
    );
    assert!(matches!(
        protobuf::to_message(&json_event()).data,
        Some(Data::TextData(_))
    ));
    assert!(matches!(
        protobuf::to_message(&binary_event()).data,
        Some(Data::BinaryData(_))
    ));
    assert_eq!(protobuf::to_message(&json_event()).spec_version, "1.0");
}

#[test]
fn binary_mode_headers_name_every_attribute() {
    let message = binary::encode(&json_event());
    let headers = message
        .headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}"))
        .collect::<Vec<_>>();
    assert_eq!(
        headers,
        [
            "ce-specversion: 1.0",
            "ce-id: 01928f6c-4e3a-7b5c-8d9e-0f1a2b3c4d5e",
            "ce-source: /pingora-panel/config-service",
            "ce-type: io.github.eltavine.pingora-panel.config.revision.activated.v1",
            "ce-subject: revision/42",
            "ce-time: 2026-09-21T14:13:20.123456789Z",
            "ce-datacontenttype: application/json",
            "ce-correlationid: corr-1",
            "ce-causationid: req-1",
            "ce-authtype: user",
            "ce-authid: user-7",
            "ce-idempotencykey: apply-42",
            "ce-traceparent: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "ce-tracestate: rojo=00f067aa0ba902b7,congo=t61rcWkgMzE",
        ]
    );
    assert_eq!(message.data, br#"{"revision":42}"#);
}

#[test]
fn binary_mode_accepts_any_header_case_and_ignores_foreign_headers() {
    let message = binary::encode(&json_event());
    let mut headers = message
        .headers
        .iter()
        .map(|(name, value)| (name.to_ascii_uppercase(), value.clone()))
        .collect::<Vec<_>>();
    headers.push(("Nats-Msg-Id".into(), "ignored".into()));
    headers.push((
        "traceparent".into(),
        "00-ffffffffffffffffffffffffffffffff-ffffffffffffffff-00".into(),
    ));
    let decoded = binary::decode(
        headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        &message.data,
    )
    .unwrap();
    assert_eq!(decoded, json_event());
}

#[test]
fn malformed_messages_fail_closed() {
    let message = binary::encode(&json_event());
    let mut duplicated = message.headers.clone();
    duplicated.push((
        "CE-ID".into(),
        "01928f6c-4e3a-7b5c-8d9e-0f1a2b3c4d5e".into(),
    ));
    assert!(binary::decode(
        duplicated
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        &message.data
    )
    .is_err());

    for (attribute, value) in [
        ("ce-specversion", "0.3"),
        ("ce-type", "com.example.revision.activated.v1"),
        ("ce-source", "https://example.com/config-service"),
        ("ce-time", "yesterday"),
        ("ce-id", ""),
        ("ce-authtype", "Not A Kind"),
    ] {
        let headers = message
            .headers
            .iter()
            .map(|(name, original)| {
                if name == attribute {
                    (name.as_str(), value)
                } else {
                    (name.as_str(), original.as_str())
                }
            })
            .collect::<Vec<_>>();
        assert!(
            binary::decode(headers, &message.data).is_err(),
            "{attribute}={value} must be rejected"
        );
    }

    assert!(protobuf::decode(&vec![0; panel_events::MAX_EVENT_BYTES + 1]).is_err());
    assert!(protobuf::decode(&[0xff, 0xff, 0xff]).is_err());
    assert!(json::decode(b"[]").is_err());
    assert!(json::decode(br#"{"specversion":"1.0","data":1,"data_base64":"AA=="}"#).is_err());
}

#[test]
fn invalid_trace_context_is_ignored_rather_than_rejected() {
    let message = binary::encode(&json_event());
    let headers = message
        .headers
        .iter()
        .map(|(name, value)| {
            if name == "ce-traceparent" {
                (
                    name.as_str(),
                    "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
                )
            } else {
                (name.as_str(), value.as_str())
            }
        })
        .collect::<Vec<_>>();
    let decoded = binary::decode(headers, &message.data).unwrap();
    assert!(decoded.trace_context().is_none());
}

#[test]
fn json_format_applies_the_specification_data_rules() {
    let mut document = serde_json::from_str::<serde_json::Value>(JSON_GOLDEN_V1).unwrap();
    let object = document.as_object_mut().unwrap();
    object.remove("datacontenttype");
    object.insert("subject".into(), "revision/42".into());
    object.insert("tracestate".into(), serde_json::Value::Null);
    object.insert("comexampleflag".into(), true.into());
    let decoded = json::decode(&serde_json::to_vec(&document).unwrap()).unwrap();
    assert_eq!(decoded.payload().content_type(), "application/json");
    assert!(decoded.trace_context().unwrap().tracestate().is_none());

    let binary = json::encode(&binary_event()).unwrap();
    let binary_document = serde_json::from_slice::<serde_json::Value>(&binary).unwrap();
    assert_eq!(binary_document["data_base64"], "AJ+Slg==");
    assert!(binary_document.get("data").is_none());

    let structured_json = fixed_envelope(
        EventPayload::new(
            "Application/Problem+JSON; charset=utf-8",
            br#"{"title":"x"}"#.to_vec(),
        )
        .unwrap(),
    );
    let encoded =
        serde_json::from_slice::<serde_json::Value>(&json::encode(&structured_json).unwrap())
            .unwrap();
    assert_eq!(encoded["data"]["title"], "x");
}

#[test]
fn encoded_events_respect_the_cloudevents_size_floor() {
    let largest = fixed_envelope(
        EventPayload::new("application/octet-stream", vec![0xab; MAX_DATA_BYTES]).unwrap(),
    );
    let encoded = protobuf::encode(&largest);
    assert!(encoded.len() <= panel_events::MAX_EVENT_BYTES);
    assert_eq!(protobuf::decode(&encoded).unwrap(), largest);
    let message = panel_contracts::cloudevents::v1::CloudEvent::decode(encoded.as_slice()).unwrap();
    assert_eq!(message.source, "/pingora-panel/config-service");
}
