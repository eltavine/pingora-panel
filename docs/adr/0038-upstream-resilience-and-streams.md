# 0038: Upstream retries, circuit breaking, queueing and streamed protocols

Status: accepted. Builds on [ADR 0010](0010-pingora-data-plane.md), which
covers endpoint selection, failover and passive health.

## Context

The gateway sends a request to the next endpoint of its upstream when the
connection fails, since nothing reached the first one, and ejects an
endpoint for a while after consecutive failures. It refuses any retry
policy, has no limit on what one upstream receives at once beyond each
endpoint's connection limit, and has never been checked with WebSocket,
Server-Sent Events or gRPC traffic, which hold requests open, stream
responses or need HTTP/2 end to end.

## Decision

**Retries.** An upstream may retry failed requests a number of times, on
the next endpoint not yet tried. Failed connections are always retried.
Besides them an upstream lists what else is: timeouts waiting for the
upstream, connections reset or closed before a response, and response
statuses such as 502, 503 and 504. A request that may have reached the
upstream is retried only when its method is idempotent (RFC 9110 §9.2.2)
and its body is still held for a retry. Retries wait an exponentially
growing delay with full jitter from a base the upstream sets.

**Retry budget.** An upstream may cap retries at a share of its requests,
with a floor per second, counted over the last ten seconds in each gateway
process, so that retries cannot multiply load on a failing upstream.

**Circuit breaker.** An upstream may open its circuit when failures — failed
connections, timeouts, resets and 502, 503 and 504 responses — reach a
share of at least a minimum number of requests in the last ten seconds.
While open, its requests are answered at once with 503 and `Retry-After`;
after the open period a set number of trial requests go through, and the
circuit closes when they succeed and opens again when one fails.
Ejection of single endpoints stays as it is.

**Concurrency and queueing.** An upstream may limit the requests it
handles at once across its endpoints. Requests over the limit are answered
with 503, or wait in a first-in, first-out queue of a set length for at
most a set time when the upstream has one.

**Streams.** WebSocket upgrades (RFC 6455) over HTTP/1.1 are proxied as
they are, with no setting: the upgrade is sent to an HTTP/1.1 connection
upstream, the gateway does not close it, and request size limits stop
counting once the connection is upgraded. Server-Sent Events are streamed
as they arrive and never compressed, so events are not held back. gRPC
needs HTTP/2 on both sides: listeners already speak it, and upstreams may
speak it with prior knowledge (h2c) to plaintext endpoints as well as by
ALPN over TLS; trailers are forwarded.

**Contracts.** The model, the configuration language, the IR and the API
carry the settings; a snapshot using any of them requires the
`upstream.resilience` capability.

## Alternatives

- Retrying every method: a request that changed state upstream would run
  twice.
- A circuit per endpoint only: an upstream whose every endpoint fails
  would still take every request until each is ejected, and ejection
  already covers single endpoints.
- Queueing without a bound: a slow upstream would hold memory and
  connections for every waiting client.

## Consequences

- A gateway that does not know these settings refuses snapshots using
  them instead of ignoring them.
- Budgets, circuits and queues are per gateway process, as rate limits
  are.
