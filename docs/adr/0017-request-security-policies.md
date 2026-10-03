# 0017: Request security policies

Status: accepted.

## Context

The gateway proxies and serves requests for every site, and operators need
to restrict who may send them and how: by client address, method, path,
user agent and referring page, behind a password, within size and time
limits, and at bounded rates. Behind load balancers and CDNs the client is
not the TCP peer, and forwarding headers from untrusted clients must not be
believed. The snapshot IR has carried a placeholder `SecurityPolicy` that
the Pingora adapter refuses. The design follows RFC 9110 (methods, 405 and
`Allow`, 413), RFC 6585 (429 and 431), RFC 7617 and RFC 7235 (Basic
authentication, 401 and `WWW-Authenticate`), RFC 7239 and the de facto
`X-Forwarded-For` for client addresses, and Apache's htpasswd format for
password files.

## Decision

- **Policies.** A security policy is a named object of the configuration,
  like a TLS profile, so it is written once and attached to many sites and
  routes. A site's policy applies to all its requests and a route's policy
  to that route's; a request must pass both. Policies are compiled into the
  snapshot IR, replacing the placeholder, and the gateway advertises the
  `request.security` capability.
- **Access rules.** A policy may allow and deny client networks (CIDR, IPv4
  and IPv6, IPv4-mapped addresses compared as IPv4); a client is refused
  when it is in a denied network, or when allowed networks are listed and it
  is in none. It may list allowed methods, answered otherwise with 405 and
  an `Allow` that names HEAD wherever GET is allowed, since HEAD goes with
  GET; denied path prefixes; denied user agents as case-insensitive
  regular expressions; and allowed referring hosts, with wildcards and a
  choice for requests without `Referer`. Refused requests get 403.
- **Basic authentication.** A policy may require a user from an htpasswd
  file in the gateway's secret directory, named like other secret files.
  Entries must use bcrypt (`$2a$`, `$2b$`, `$2y$`) or Argon2 (`$argon2id$`,
  `$argon2i$`); a file with weaker hashes is refused when the snapshot is
  prepared. Checks run off the request threads, and successful ones are
  remembered by a keyed digest of the credentials for a few minutes, so the
  deliberate cost of the hashes is paid once per user and not per request.
  Missing or wrong credentials get 401 with `WWW-Authenticate: Basic`, the
  policy's realm and `charset="UTF-8"`.
- **Limits.** A policy may cap the bytes of request headers (431), the
  request body (413, from `Content-Length` before anything is forwarded and
  while streaming otherwise) and the time between body reads (408, as RFC
  9110 has it for a request that does not arrive in time).
- **Rate limits.** A policy may hold token buckets keyed by client address,
  host, route or a request header, each with a rate and a burst, and a cap
  on concurrent requests per client address. Requests over a limit get 429
  with `Retry-After`, or the policy's own status and body. Buckets live in
  the gateway's memory, sharded and pruned when idle; they are per gateway
  process and start full after a restart.
- **Client addresses.** A listener may name trusted proxy networks and
  where they put the client address: `X-Forwarded-For`, `X-Real-IP` or
  `Forwarded`. The client is the rightmost address not in a trusted network,
  walking from the TCP peer; rules, limits and forwarding headers use it.
  Forwarding headers from peers that are not trusted are dropped before the
  gateway adds its own, so upstreams cannot be told a forged address.
- **Checks.** Validation warns about dangerous settings: Basic
  authentication on listeners without TLS, every address trusted as a proxy
  and upstreams that do not verify their TLS nodes. The gateway refuses
  private keys that others than their owner may read.

## Consequences

- A request passes through at most two policies, compiled into lookup
  tables, so policies cost little when they are not used.
- Rate limits are not shared between gateways; a cluster-wide limit needs
  a shared store, which this design leaves open.
- Upstreams that relied on client-supplied `X-Forwarded-For` behind a load
  balancer need that load balancer listed as a trusted proxy.
