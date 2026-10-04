# 0032: One control-plane process on SQLite

Status: accepted. Replaces the process-per-service composition of
[ADR 0007](0007-service-processes-health-and-discovery.md) and the
PostgreSQL storage of ADRs [0006](0006-jetstream-delivery-and-dead-letters.md),
[0008](0008-durable-jobs.md) and [0013](0013-audit-trail.md).

## Context

The product is a gateway control platform for one host: the specification
says so in its positioning and system context, and keeps multi-node
scheduling out of 1.0. Its control plane nevertheless ran as five service
processes and a bootstrap process around a PostgreSQL server, with the
machinery several instances of a service need: outbox relay leadership
held through advisory locks and woken by `LISTEN`/`NOTIFY`, job leases
claimed with `FOR UPDATE SKIP LOCKED`, alert evaluation guarded by an
advisory lock, one role, schema and SCRAM secret per service. A single host
never runs those instances, and pays for the server: another process to
run, credentials to rotate, a major-version upgrade that needs a dump or
`pg_upgrade`, and backups through `pg_dump`. The specification's own
backup list names an SQLite database.

SQLite's guidance for choosing an engine asks whether the data is separated
from the application by a network, whether many writers must write at the
same instant, and whether the data approaches a terabyte; when none holds,
it recommends SQLite. None holds for the control plane: its data lives on
the host it runs on, it writes a few rows per operator action, and its
records are small. In write-ahead-log mode readers proceed while one writer
commits, and separate database files for separate subdomains let their
writers proceed independently. A transaction that changes several attached
databases is atomic per file only.

## Decision

**Processes.** An installation runs three processes of its own: the
control plane, `gatewayd` and, where enabled, `ops-agent`. The control
plane composes the identity and API, configuration, automation,
observability and audit modules. Modules keep their gRPC contracts and
call each other over in-process channels, so nothing listens for them on a
network address. `gatewayd` stays apart so traffic outlives the control
plane, and `ops-agent` because it holds host privileges.

**Storage.** Each module owns one SQLite database file in the control
plane's data directory, named after the module: `identity.db`,
`config.db`, `automation.db`, `observability.db` and `audit.db`. Each
holds the tables the module's PostgreSQL schema held, as `STRICT` tables,
and its migrations, outbox and inbox. A module never writes another
module's file, so no transaction spans files. Files are opened with
`journal_mode=WAL`; `synchronous=FULL`, because a committed change, its
event and its audit record must survive a power loss and the control
plane commits rarely enough to afford it; `foreign_keys=ON`; and a busy
timeout. Transactions that write begin with `BEGIN IMMEDIATE`, so a writer
waits its turn for the file's lock instead of failing when a read would
have to become a write. Times are RFC 3339 UTC text in one form, written by
the module or by SQLite's clock as `strftime('%Y-%m-%dT%H:%M:%f+00:00')`, so
they compare in order. The data directory
and its files belong to the control plane's user alone.

**Outbox.** A module's relay publishes its outbox to JetStream as before.
One process relays each file, so relay leadership goes away. A module that
commits events wakes its relay, which otherwise looks every 250 ms.

**Kept.** NATS JetStream still carries events to their consumers, with
retries, dead letters and replay (ADR 0006); the installation's authority
and mutual TLS still protect the calls to `gatewayd` and `ops-agent`
(ADR 0009); Prometheus, Loki, the OpenTelemetry Collector and the node
exporter stay the telemetry backends (ADRs 0022, 0025 and 0028).

**Removed.** The PostgreSQL server; service roles, schemas and their
secrets; relay leadership and the alert evaluation lock; the database
provisioning in `panel-bootstrap`. Webhook notifications are attempted
outside any transaction, so no write waits on a receiver.

## Alternatives

- PostgreSQL behind the single process: a server to run, upgrade and back
  up for the data of one process.
- One SQLite file for every module: every module's writes would queue on
  one lock, and module ownership would no longer be visible in storage.
- One process per module, each with its own SQLite file: keeps the
  listeners, ports and credentials between modules that one host does not
  need.
- An embedded key-value store such as redb or fjall: gives up SQL, the
  migrations and every existing query.

## Consequences

- An installation's state is its data directory: SQLite's online backup
  copies each file consistently while the control plane runs.
- Tests open a temporary file instead of needing a database server.
- Should several hosts ever share a control plane, a server database comes
  back as another adapter behind the same stores.
