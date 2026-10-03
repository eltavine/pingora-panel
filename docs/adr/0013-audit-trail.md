# 0013: Audit trail

Status: accepted.

## Context

Every change, and every attempt that was refused or failed, must leave an
audit record an Auditor can query by actor, operation, resource, correlation
and time, and must be tamper-evident. Each control-plane service owns one
PostgreSQL schema and never writes another's ([ADR 0007](0007-service-processes-health-and-discovery.md)).
Domain events already leave every service through its transactional outbox
as CloudEvents carrying the actor, request, correlation, idempotency key and
trace context ([ADR 0004](0004-cloudevents-event-contract.md)), and reach
consumers at least once through JetStream ([ADR 0006](0006-jetstream-delivery-and-dead-letters.md)).

## Decision

- The audit trail is built from domain events. A service records a change
  and its event in one transaction, so a record exists exactly for the
  changes that committed. Attempts that change nothing, such as a refused
  change, a failed apply or a dry run that was not run, are recorded with an
  event of their own, written in its own transaction, whose data names the
  operation and the error code.
- `audit-service` owns the `audit` schema. It consumes every event type with
  one durable consumer and appends each event once, keyed by its source and
  ID, so redeliveries and replays do not duplicate records.
- Records form a SHA-256 hash chain: each record's hash covers the previous
  hash and the record's canonical form, a JSON object of its CloudEvent
  attributes and data with sorted keys. Appends hold the chain head's row
  lock, so the chain has one order. A checkpoint of the head's position and
  hash is written periodically. Rows cannot be updated or deleted: a trigger
  refuses it, and verification recomputes the chain over a range and checks
  it against the checkpoints, reporting the first record that differs.
- `audit-service` serves `pingora.panel.audit.v1.AuditQuery` to list records
  by actor, type, subject, correlation or request ID and time, newest first
  with a cursor, to fetch one record, and to verify the chain. `panel-api`
  exposes them as `/api/v1/audit-events`, `ppanel audit` and the console's
  audit page.

## Consequences

- Services stay unaware of the audit store; a new event type is audited
  without changing `audit-service`.
- Audit records appear after the outbox relay and consumer deliver the
  event, normally within a second; a record is never lost, since the outbox
  retains unpublished events and JetStream retains undelivered ones.
- Removing records requires the schema owner to drop the trigger, and is
  then detected by verification against the checkpoints; anchoring
  checkpoints outside the database can be added without changing records.
