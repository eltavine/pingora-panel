# 0004: CloudEvents as the domain event contract

Status: accepted.

## Context

Services exchange domain events through a transactional outbox and NATS
JetStream. Each event must identify its producer, type, aggregate,
occurrence time, accountable principal, correlation and causation, the
idempotency key of the command that caused it, and the starting trace. A
product-specific envelope would oblige every consumer, tool and future
integration to learn a private format, and its evolution would need its
own compatibility rules.

## Decision

Every domain event is a [CloudEvents 1.0](https://github.com/cloudevents/spec/blob/v1.0.2/cloudevents/spec.md)
event. `panel-events` models it independently of any format or broker:

- `id` is a UUIDv7. `source` is `/pingora-panel/<service>`. `subject` is
  `<aggregate type>/<aggregate id>`. `time` is the occurrence time.
- `type` is `io.github.eltavine.pingora-panel.<event type>.v<major>`. As the
  CloudEvents versioning guidance recommends, an incompatible data change
  creates a new type, while a compatible change keeps it.
- The documented Correlation, Auth Context and Distributed Tracing
  extensions carry `correlationid`/`causationid`, `authtype`/`authid` and
  `traceparent`/`tracestate`. One product extension, `idempotencykey`, carries
  the originating command's key.
- Data has an RFC 6838 media type. Protobuf data uses the RFC 9996
  `application/protobuf` type with an `https://type.googleapis.com/<message>`
  `dataschema`, which is both an absolute URI and a valid `Any` type URL.
- Trace context follows the W3C Trace Context Recommendation, including
  version handling and tracestate truncation. Its syntax is validated
  locally because the reference propagator belongs to the OpenTelemetry SDK,
  which must not enter a neutral crate.

`panel-event-codec` implements the Protobuf format for storage, the JSON
format required of every implementation, and binary content mode with
percent-encoded `ce-` headers for the NATS and HTTP bindings. All three use
one attribute mapping. Encoded events stay within the 64 KiB that
CloudEvents intermediaries must forward, and golden fixtures pin the v1
Protobuf bytes and JSON document. The official `io.cloudevents.v1` schema is
vendored unchanged apart from `buf format`.

Event data is defined in proto3 under `proto/events`, one message per event
type, and `panel-event-contracts` generates the Rust types with `prost` and
their JSON with `pbjson`. Data travels as JSON in the proto3 JSON mapping
with the definition's field names and every field written, so consumers read
plain JSON, and the message's type URL is the event's `dataschema`. Buf's
`FILE` breaking rules, which include JSON field names, guard the definitions
like every other contract. Producers build events from the generated types
through `EventData`, which names the event type, so a domain type never
becomes an event's shape by accident, and a store's adapters share one
construction of each event.

## Alternatives

- A private Protobuf envelope duplicates CloudEvents without its
  interoperability, bindings or tooling.
- `cloudevents-sdk` 0.9.0 was evaluated for the JSON format. Its JSON data
  detection uses string prefix and suffix tests, which ignore media type
  parameters and case. It turns `data_base64` into JSON data when the
  content type looks like JSON. Its NATS binding targets async-nats 0.42,
  and it has no Protobuf format. Its codec could be reintroduced behind
  the same functions if these gaps close; the local codec is small and
  tested against the specification rules.

## Consequences

- Consumers and operators can inspect events with generic CloudEvents
  tooling, and webhook delivery can reuse the same representation.
- Event types and payload schemas evolve under explicit, reviewable rules;
  Buf continues to guard Protobuf payload messages.
- Producers must keep events compact and reference large objects instead of
  embedding them.
