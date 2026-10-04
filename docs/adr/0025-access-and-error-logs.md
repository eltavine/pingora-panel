# 0025: Access and error logs

Status: accepted.

## Context

[ADR 0022](0022-metrics-logs-and-traces.md) decided that the gateway writes
access and error logs to rotated files that an OpenTelemetry Collector
ships to Loki. Operators choose the format, the fields, which sites and
routes log, how files rotate and how long they stay. Every record has to
carry the request ID, the trace ID, the site, the route and the revision,
secrets must never reach a log, and the request path must not wait for a
disk or a collector.

[OpenTelemetry's HTTP and URL conventions](https://opentelemetry.io/docs/specs/semconv/registry/attributes/url/)
name the fields and say which query parameters to redact,
[OWASP's logging guidance](https://cheatsheetseries.owasp.org/cheatsheets/Logging_Cheat_Sheet.html)
lists what must not be logged and how to stop log injection, and the
[Combined Log Format](https://httpd.apache.org/docs/current/logs.html#combined)
is what log analysers read.

## Decision

**Request identity.** The gateway keeps a request's `X-Request-Id` when it
is at most 128 visible ASCII characters, and otherwise writes a new UUIDv7
into the request before anything reads it. Templates, the upstream and the
log therefore see the same ID; responses are unchanged. A valid W3C
`traceparent` gives the record its trace ID.

**Records.** A JSON record is one line whose keys are OpenTelemetry
attribute names: `http.request.method`, `url.scheme`, `url.path`,
`url.query`, `server.address`, `client.address`, `network.peer.address`,
`network.protocol.version`, `http.response.status_code`,
`http.request.body.size`, `http.response.body.size`,
`user_agent.original`, `error.type` and `http.server.request.duration` in
seconds, next to `timestamp` (RFC 3339, UTC, microseconds), `trace_id`,
`event.name` and the product's own `pingora_panel.*` fields: request ID,
listener, site, route, revision, upstream pool and node. Error records add
a severity and a message. Keys without a value are left out.

**Formats.** Access logs are JSON or the Combined Log Format, per site and
route; error logs are JSON. Combined lines escape quotes, backslashes and
control characters as `\xHH`, and JSON escapes them, so a request cannot
forge a line.

**Settings.** The configuration language sets them in `http` for every
site and in `server` and `route` for one:

- `access_log on|off [format=json|combined]`;
- `log_field <name> <template>`, extra fields from request variables, such
  as `log_field tenant $http_x_tenant`; a route's fields add to its
  server's, and its server's to those in `http`;
- `log_redact_query <key> ...` replaces OpenTelemetry's list of sensitive
  query parameters (`X-Amz-Signature`, `X-Amz-Credential`,
  `X-Amz-Security-Token`, `sig`, `X-Goog-Signature`), matched by case;
- `log_redact_headers <name> ...` adds headers to `authorization`,
  `proxy-authorization`, `cookie` and `set-cookie`.

The redaction lists and `log_files`, which sets rotation and retention for
every file, belong in `http` alone; for `access_log` the inner block wins.

**Redaction.** Sensitive query values become `REDACTED` with their keys
kept. Headers are logged only through `log_field`; a sensitive header and
every cookie are logged as `REDACTED`. URLs are logged as path and query,
so credentials in a URL never appear.

**Files.** Under the gateway's log directory, requests a site took go to
`sites/<site>.access.log` and the rest to `access.log`; errors go to
`error.log`. Naming the site in the file name rather than in a directory
keeps any site identifier inside the log directory, and lets the
collector read the site from the path whatever the format. A file rotates
when it would grow past `max_size` and, with `rotate=daily`, at midnight
UTC; the rotated file gets a UTC timestamp suffix. After a rotation and at
start the gateway deletes rotated files older than `keep` and beyond
`max_files`, so `max_size` times `max_files` bounds the disk a file uses.
No maintained Rust crate rotates on both size and time and prunes by both
age and count, so the gateway does it itself in a small module.

**Back-pressure.** A worker formats a record and queues it on a bounded
channel to the writer thread, which buffers writes and flushes at least
every 100 milliseconds. When the channel is full the record is dropped and
counted in `pingora_panel_log_records_dropped_total`; written records and
bytes are counted too.

**Shipping.** The Collector's `filelog` receiver follows the files across
rotations, parses JSON or Combined lines, takes the site from the file
name, and sends them over OTLP to Loki, which keeps the attributes as
structured metadata.

## Consequences

- Access logs are on by default once the gateway has a log directory; a
  site or route turns them off with `access_log off`.
- Adding a field to records is additive; renaming or dropping one is a
  breaking change for queries and dashboards.
- A full disk or a stalled writer costs records, not requests.
