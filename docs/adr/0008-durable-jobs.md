# 0008: Durable jobs, schedules and maintenance windows

Status: accepted. [ADR 0032](0032-one-control-plane-process-on-sqlite.md) keeps
jobs in the automation module's SQLite file; leases still fence work.

## Context

`automation-service` runs long and recurring work, such as certificate
renewal, backups, notifications and planned publications. Each task needs an
idempotency key, a lease, a retry policy and a way to stop it. Recurring work
must run once per occurrence even with several schedulers, and some work may
only start inside an agreed maintenance window. Work must survive restarts of
the service and of its workers, and its progress must reach the rest of the
system.

## Decision

- `panel-jobs` defines the model and a `JobStore` port. `automation-service`
  implements the port on PostgreSQL in its own schema, and an in-memory store
  with the same semantics backs the engine's tests.
- **Leases.** A worker claims jobs with `FOR UPDATE SKIP LOCKED` under a lease
  that it renews at a third of the lease duration. When a lease expires, the
  job's next attempt may run elsewhere. An expired job whose cancellation was
  requested becomes cancelled, and one on its final attempt becomes failed.
  Every write made with a lease is fenced by owner and attempt, so a worker
  that lost its lease changes nothing.
- **Idempotency.** Enqueuing the same kind and idempotency key returns the
  existing job.
- **Cancellation.** A queued job is cancelled at once. A running job is
  asked to stop through the lease renewal, and its handler observes a
  cancellation token.
- **Retries.** A retryable failure waits an exponential backoff with equal
  jitter. Attempt *n* waits between half of and the full
  `min(max, initial × 2^(n−1))`, until its attempts run out. A handler panic
  counts as a retryable failure.
- **Progress and events.** Handlers report progress, which is coalesced and
  written at most once per second. Every state change and progress write
  appends an `automation.job.<change>` CloudEvent to the transactional outbox
  in the same transaction.
- **Schedules.** A schedule enqueues one job for each occurrence of an
  RFC 5545 recurrence (`DTSTART` with an optional `TZID`, and `RRULE`). Its
  idempotency key names the schedule and the occurrence, and firing locks due
  schedules with `SKIP LOCKED`, so every occurrence is enqueued exactly once.
  Occurrences missed while no scheduler ran collapse into one.
- **Maintenance windows.** An RFC 5545 recurrence and a duration define a
  window. A job that requires a window is claimed only while an occurrence of
  it is open.

## Alternatives

Compared in October 2026:

- **`apalis`** with `apalis-sqlite` and `apalis-cron`, at 1.0 release
  candidates, keeps jobs in SQLite, retries through middleware and
  re-enqueues a job whose worker stopped sending heartbeats. It schedules
  with cron expressions rather than RFC 5545 recurrences, offers no
  maintenance windows, and keeps its jobs in tables and migrations of its
  own, so a job's state change could not append its event to the module's
  outbox in the same transaction.
- **A scheduler without durable jobs**, such as `tokio-cron-scheduler`,
  loses queued and running work when the process restarts.

`panel-jobs` keeps what the platform needs and leaves recurrence rules to
the `rrule` crate through `panel-schedule`.

## Consequences

- At-least-once execution with idempotent handlers. A worker crash costs
  at most one lease duration of delay.
- Recurrence rules with a time zone keep their wall-clock time across
  daylight-saving changes, which cron expressions evaluated in UTC cannot.
- Job state changes are observable through JetStream like any domain event,
  so the API and GUI can follow progress without polling the database.
