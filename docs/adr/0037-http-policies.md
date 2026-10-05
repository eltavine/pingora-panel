# 0037: HTTP policies: headers, CORS and compression

Status: accepted. Builds on [ADR 0010](0010-pingora-data-plane.md),
[ADR 0017](0017-request-security-policies.md) and
[ADR 0036](0036-route-conditions.md).

## Context

Operators change the headers requests carry upstream and responses carry
back, hide or replace the `Server` field, answer browsers' cross-origin
requests and compress responses. The gateway already forwards the
client's `Host` unless an upstream replaces it, appends `Forwarded`
(RFC 7239), `X-Forwarded-For`, `-Host` and `-Proto` and `Via`, keeps or
generates `X-Request-Id`, sends `X-Real-IP` with the client after trusted
proxies, and passes W3C Trace Context's `traceparent`
through while logging its trace; it refuses any other per-route HTTP
setting. Request security already lives in named policies that sites and
routes reference (ADR 0017), which keeps a setting written once and
shared.

## Decision

**Named policies.** An HTTP policy is a configuration resource that sites
and routes reference, as security policies are. A route's requests pass
the site's policy and then the route's: header changes of both apply, the
route's last, and the route's CORS and compression settings replace the
site's when it has them.

**Headers.** A policy adds, sets or removes request fields before a request
goes upstream and response fields before a response goes to the client.
Adding appends a field line, setting replaces every line of the field, and
removing drops them (RFC 9110 §5.2, §5.3). Values are templates of request
variables, such as `$remote_addr` or `$request_id`. Hop-by-hop fields,
framing fields (`Content-Length`, `Transfer-Encoding`), `Host` and the
forwarding fields the gateway writes, `X-Real-IP` among them, are not
theirs to change: an upstream names the `Host` it receives.

**Server.** A policy keeps the upstream's `Server` field, removes it, or
replaces it with its own value (RFC 9110 §10.2.4).

**CORS.** A policy may follow the Fetch Standard's CORS protocol for listed
origins — exact origins, `*`, or `https://*.example` patterns of one label
— with allowed methods and headers, exposed headers, whether credentials
are allowed and how long a preflight is cached. The gateway answers a
preflight (`OPTIONS` with `Origin` and `Access-Control-Request-Method`)
itself, with 204 and the allowed methods and headers when the origin,
method and headers are allowed and without them otherwise, and adds
`Access-Control-Allow-Origin` with `Vary: Origin` to other responses for
allowed origins. With credentials the origin is echoed, never `*`, as the
protocol requires.

**Compression.** A policy may compress responses with gzip, Brotli or
Zstandard, chosen from the client's `Accept-Encoding` by Pingora's
compression module, for listed media types and responses of at least a
size; responses already encoded, without a body, or to `Range` requests are
sent as they are, and compressed responses carry `Vary: Accept-Encoding`.

**Contracts.** The model, the configuration language, the IR and the API
carry policies; a snapshot whose sites or routes reference one requires the
`http.policies` capability.

## Alternatives

- Settings written separately on every site and route: the same CORS or
  header rules would be repeated and drift apart.
- Leaving preflights to upstreams: every upstream would implement the
  protocol, and static sites and fixed responses could not answer them.
- Compressing every response: images and archives would be compressed
  again for nothing.

## Consequences

- A gateway that does not know HTTP policies refuses snapshots using them
  instead of ignoring them.
- Header templates run per request; a policy with none costs nothing.
