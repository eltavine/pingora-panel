# 0010: Pingora data plane

Status: accepted.

## Context

`gatewayd` activates engine-neutral snapshots, and until now nothing served
them. The data plane must turn a snapshot into listeners, virtual hosts,
routes and upstream pools on Pingora 0.9 without forking it, survive
reloads and worker changes without refusing connections, and follow the
HTTP standards a public gateway is held to. Pingora's `Server` bootstrap is
unsuitable for embedding: it owns process signals and its descriptor
handover can exit the process.

## Decision

- **Embedding.** `gateway-pingora` uses Pingora's proxy, connector, TLS and
  load-balancing crates but not its `Server`. The data plane binds every
  listening socket itself (`SO_REUSEADDR`, optional `SO_REUSEPORT`,
  `IPV6_V6ONLY` when asked, backlog 65535) and starts each Pingora service
  with a descriptor table of duplicates on a runtime it owns.
- **Generations.** A generation serves one fixed listener set with one
  worker count. Activating a snapshot with the same listeners only swaps the
  routing state the next request reads. A listener or worker change, or an
  explicit reload, starts a new generation on the retained sockets, so the
  accept queues carry over; the previous generation stops accepting and
  finishes in-flight requests within the drain timeout. Only when a removed
  socket holds a port the new set needs does the old generation stop first.
  Bind conflicts are reported at prepare time, before activation.
- **Capabilities.** The adapter declares what it executes. Snapshots that
  need anything else — HTTP/3 listeners, Unix socket upstreams, retry
  policies — fail preparation with a diagnostic per field, so reserved
  configuration is stored but never half-applied.
- **Host routing.** A request names exactly one authority (RFC 9112
  §3.2): the absolute-form target wins, a missing or repeated Host is a
  400, and names are lowercased without the trailing dot. Exact names win
  over wildcards, which cover one label (RFC 6125 §6.4.3). A listener's
  default site serves unmatched names; without one the gateway answers 421
  (RFC 9110 §15.5.20), as it does when a TLS request names a host its
  certificate does not cover.
- **Redirects.** Alias domains, `www` canonicalization and HTTP-to-HTTPS
  combine into a single 308 to the final URL, preserving method, path and
  query.
- **Routes.** Paths are normalized (RFC 3986 §6.2.2) before matching.
  Lower priorities win; at equal priority exact paths beat globs, regular
  expressions and prefixes, longer patterns beat shorter ones, and the
  route identifier breaks remaining ties so evaluation is deterministic.
  A site's own action is its last route. Actions proxy to an upstream,
  redirect, answer with a fixed response or serve files.
- **Static files.** Files resolve below the configured root with
  containment checked after canonicalization. Responses carry validators
  and honour conditional and single-range requests (RFC 9110 §13, §14);
  directories without a trailing slash redirect.
- **Forwarding.** Requests carry `X-Forwarded-For/-Proto/-Host`, the
  `Forwarded` header (RFC 7239) and `Via` (RFC 9110 §7.6.3). Hop-by-hop
  fields are stripped by Pingora's request policy.
- **TLS.** Rustls selects the certificate by SNI from the active snapshot's
  TLS profiles, whose files live in the gateway's secret directory, and
  each profile's minimum version is enforced. A listener offers its enabled
  protocols through ALPN, narrowed by its own profile's list; profiles
  chosen by SNI cannot change the offer, because it is fixed per listener.
- **Upstreams.** Pools use Pingora's weighted round robin, random and
  consistent hashing (by client IP, URI, header or cookie), with backup
  nodes used only when no primary is available. Per-node limits skip
  saturated nodes, passive health ejects a node after consecutive
  failures, and active HTTP or TCP checks apply success and failure
  thresholds and expected statuses. A failed connection is retried on
  another node. Node state — load, failures, latency, ejection and manual
  drain — outlives snapshots and is published for operators; drains and the
  worker count persist across restarts.
- **Runtime control.** `gatewayd` exposes data plane state, reload, worker
  count, shutdown, upstream health and drain over
  `pingora.panel.gateway.v1.GatewayRuntime`, admitting only `panel-api`.
- **One change to Pingora.** Its rustls client keeps the server's
  certificate chain in the connection digest's extension, a slot upstream
  Pingora already has, so `proxy_ssl_verify_by_lua` (ADR 0039) can read
  what an upstream presented. The adapter reads it through upstream's API
  and builds against an unchanged Pingora, where scripts find no chain;
  the weekly canary against Pingora's main tests it that way.

## Consequences

- Configuration changes never restart the process, and listener changes
  drop no queued connection unless a port moves between sockets.
- The adapter is the only crate that knows Pingora; another engine
  implements the same adapter and capability set.
- Features beyond the declared capabilities, such as HTTP/3, arrive by
  extending the adapter and its capability list, not the configuration
  model.
