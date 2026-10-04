# 0006: JetStream delivery, dead letters and replay

Status: accepted. [ADR 0032](0032-one-control-plane-process-on-sqlite.md) keeps
the inbox of each module in its SQLite file.

## Context

Domain events leave each service through its transactional outbox and must
reach consumers at least once, in order per aggregate, without being lost
when a consumer keeps failing. JetStream deduplicates by message ID within a
window and redelivers until acknowledged, but once a message reaches a
consumer's delivery limit it is silently dropped from that consumer: JetStream
has no dead-letter queue and only publishes a max-deliveries advisory.

## Decision

`panel-jetstream` provides the broker side:

- Streams are provisioned idempotently: `<PREFIX>_EVENTS` holds
  `<prefix>.events.>` and `<prefix>.replay.>`, and `<PREFIX>_DLQ` holds
  `<prefix>.dlq.>`. Both use file storage, limits retention and direct get.
  Mutable settings are updated in place. A stream whose storage or retention
  differs is never recreated implicitly, because that would delete its
  messages.
- Events are published in the CloudEvents NATS binding's binary content mode
  to `<prefix>.events.<event type>.v<major>`. The event ID is the
  `Nats-Msg-Id`, so a relay that republishes after a crash inside the
  duplicate window stores nothing twice. Consumers also accept structured
  JSON messages, as the binding requires.
- Consumers are durable pull consumers with explicit, server-confirmed
  acknowledgements and a finite delivery limit. A handler's retry is a
  delayed negative acknowledgement. Long handlers extend the ack deadline
  with progress acknowledgements. A panic counts as a transient failure.
- An event is parked in the dead-letter stream, with its original headers
  and payload, when its handler asks for it, when retries run out on the
  final attempt, or when the message is not a valid CloudEvent. The delivery
  is then terminated. The max-deliveries advisory parks deliveries that run
  out without any handler decision, such as a worker that crashes. Parking
  uses `dlq.<consumer>.<stream sequence>` as its message ID, so both paths
  park an event once.
- Replay republishes a parked event to `<prefix>.replay.<consumer>`, which
  only that consumer subscribes to, and then removes it from the queue.
  Consumers that already processed the event do not see it again.
- Idempotent consumption uses the `(consumer, event_id)` record of the
  idempotent consumer pattern. Handlers whose effects live in PostgreSQL
  record the event in their own transaction. Other handlers claim it with a
  lease through `IdempotentEventHandler` and the PostgreSQL inbox.

## Consequences

- Delivery is at-least-once and effectively-once per consumer, and no event
  is dropped without a trace in the dead-letter stream.
- Operators can inspect, replay or discard parked events per consumer;
  exposing these operations through the API, CLI and GUI builds on this
  adapter.
- Integration tests run against a real JetStream server locally through
  `panel/scripts/dev-services.sh` and in CI through a digest-pinned NATS
  container.
