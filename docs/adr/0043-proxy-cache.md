# 0043: Proxy cache

Status: accepted. Builds on [ADR 0010](0010-pingora-data-plane.md),
[ADR 0036](0036-route-conditions.md),
[ADR 0037](0037-http-policies.md) and
[ADR 0042](0042-directory-listings-media-types-and-cache-headers.md).

## Context

Every request to a proxied site reaches its upstream, however often the same
response is asked for. Operators expect nginx's `proxy_cache`: responses
stored by a key, kept for what the origin or the operator says, bypassed for
some requests, purged on demand, bounded in size, and a way to see whether
it helps. The IR has carried an unused cache policy since the first
snapshot, which gateways refuse.

## Decision

**Policies.** Sites and routes name a cache policy; a route's replaces its
site's, and a disabled policy caches nothing. A policy says:

- the key, a template of request variables, `$scheme$host$request_uri`
  unless written (nginx's `proxy_cache_key`), and request fields whose
  values tell responses apart besides those the response's `Vary` names;
- how long responses are fresh when their origin does not say: a default
  for the statuses RFC 9110 §15.1 lets caches store by default, and one per
  status, `0` keeping that status out (nginx's `proxy_cache_valid`);
- whether the origin's `Cache-Control` and `Expires` decide (RFC 9111
  §5.2.2, §5.3), as by default, or the policy alone does (nginx's
  `proxy_ignore_headers`);
- conditions, as routes write them (ADR 0036), under which a request
  neither uses nor fills the cache (nginx's `proxy_cache_bypass` and
  `proxy_no_cache`);
- how long a stale response may be served while it is revalidated or when
  the upstream fails, unless the origin says (RFC 5861);
- the largest response it stores, and whether responses carry
  `Cache-Status` (RFC 9211).

**What is stored.** Only `GET` and `HEAD` requests use the cache. A response
with `Set-Cookie` is never stored, whatever its policy. A request with
`Authorization` uses a stored response only when the response allows it
(RFC 9111 §3.5). `Vary: *` is never stored, and other `Vary` fields keep a
response per variant (§4.1), so encodings never cross. `no-store` and
`private` keep a response out, `no-cache` stores it but revalidates it
first, and fields `private` or `no-cache` names are dropped (§5.2.2). A
stale response is revalidated with its validators and updated by a 304.
Ranges are answered from whole stored responses; partial responses are not
stored.

**Storage.** A gateway keeps one in-memory store bounded by the size the
configuration gives, 256 MiB unless written; entries are admitted and
evicted by TinyUFO (S3-FIFO with TinyLFU), the bounded cache Pingora's
in-memory caches use, by their size. Concurrent misses for one key are
collapsed by Pingora's cache lock, and keys found uncacheable are remembered
so their requests skip the lock. Changing the size empties the store.

**Seeing it work.** Responses of cached routes carry
`Cache-Status: pingora-panel; hit`, `; fwd=miss; stored`, `; fwd=bypass` or
the like (RFC 9211) unless their policy turns it off, and
`$upstream_cache_status` gives nginx's `HIT`, `MISS`, `BYPASS`, `EXPIRED`,
`STALE`, `UPDATING` or `REVALIDATED` to templates and access logs. The
gateway counts lookups per site and outcome, and reports them with the
store's size and entries since it started.

**Purging.** Operators purge everything, a site's responses, or URLs. Sites
and the whole store carry generations in their keys, so purging them is
immediate and what they held ages out; a purged URL drops every variant
stored for its key, the key rendered from the URL's scheme, host and
target. Purges are operations on the running gateway, recorded in the audit
trail, and need `gateway.operate`; statistics need `gateway.read`.

**Contracts.** The model, the configuration language, the IR, the API, the
CLI and the console carry cache policies and the store's size; a snapshot
with an enabled cache policy requires `proxy.cache`. The gateway runtime API
gains `GetCacheStats` and `PurgeCache`.

## Alternatives

- Pingora's `MemCache`: it is for tests, unbounded and without admission.
- A store on disk: worth it for large objects later; memory serves what
  sites ask for most and needs no space management on the host.
- Purging by scanning the store: TinyUFO keeps no order to scan, and a
  generation makes a site's purge immediate at any size.
- `X-Cache-Status`: RFC 9211 standardized `Cache-Status`; the variable
  keeps nginx's values for logs and headers operators already write.

## Consequences

- A gateway that does not cache refuses snapshots with enabled cache
  policies instead of serving them uncached.
- Sites without a cache policy reach their upstreams as before.
- A gateway restart empties the cache.
