# 0026: Log search, tail and deletion

Status: accepted.

## Context

[ADR 0025](0025-access-and-error-logs.md) puts the gateway's access and
error records in Loki, with their fields as structured metadata. Operators
need to find records by keyword, status, client, path and request ID, to
watch them arrive, to download them and to delete a site's records. Per
[ADR 0022](0022-metrics-logs-and-traces.md), the console and the CLI reach
Loki only through `observability-service`. The product specification asks
for bounded buffers, the disconnection of slow consumers and resumable
cursors on every tail.

## Decision

**Queries are built, never passed through.** Clients send typed filters;
`observability-service` writes the LogQL. Values go into quoted LogQL
strings with backslashes and quotes escaped, and text that becomes part of a
regular expression is escaped first, so no filter can change the query.
The filters are the kind of record (access or error), the site, the route,
a status code or class such as `5xx`, a client address or CIDR block
(Loki's `ip()` filter), a path prefix, a request ID and text the line
contains, ignoring case.

**Search.** Records come newest first, at most 500 a page, each with its
time, kind, original line and fields. The page's cursor is the time of its
last record; the next page continues before it.

**Tail.** A server-streaming call asks Loki every second for records after
its cursor, oldest first, and sends each with the cursor to resume from. A
tail buffers at most 1,000 records; a client that falls further behind is
disconnected and resumes from the last cursor it saw. `panel-api` relays
tails to browsers over a WebSocket (RFC 6455) that it accepts only from the
console's origins, and pings idle ones.

**Download.** `panel-api` streams the original lines of up to 100,000
matching records as plain text, newest first, so a download is a log file.

**Deletion.** Deleting is a Loki delete request for a site's records, or
every record, over a time range that ends now. Loki applies it once its
cancel period, five minutes in the Compose installation, has passed;
pending requests are listed with the records that remain until then. Every
request is audited.

**Permissions.** `logs.read` searches, tails and downloads; viewers,
auditors and operators hold it. `logs.delete` deletes; operators hold it.

## Consequences

- Loki can be replaced by another store that answers these filters without
  changing the API, the CLI or the console.
- A deletion is not immediate, and the console says so.
- A tail costs a Loki query a second while it is open.
