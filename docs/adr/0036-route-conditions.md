# 0036: Route conditions

Status: accepted. Builds on [ADR 0010](0010-pingora-data-plane.md),
[ADR 0011](0011-configuration-model-and-apply.md) and
[ADR 0012](0012-configuration-language-and-revisions.md).

## Context

A route takes requests by path, and a prefix route optionally by one host.
Operators also route by method, host, header, query parameter, cookie,
client address, user agent, referer and content type, combine conditions
with and, or and not, and need to see which route a request would take
before applying a change. The gateway's matcher lives inside the data plane
crate, where the control plane cannot reuse it; a second implementation for
a tester would disagree with the gateway sooner or later.

## Decision

**One matcher.** Matching moves into `panel-routing`, a crate that knows the
IR and nothing of Pingora or the control plane: it finds the site of a host,
ranks a site's routes and decides whether a route takes a request, saying
why not when asked. The gateway serves requests with it and the control
plane's route tester explains requests with it, so both agree by
construction. A request reaches it as a borrowed view: method, host, path,
query, header lookup and the client address after trusted proxies.

**Conditions.** A route's match keeps its path and adds conditions, all of
which must hold:

- `method`: one of the listed methods, compared case-sensitively as
  RFC 9110 §9.1 defines them.
- `host`: one of the listed hosts or `*.parent` wildcards of one label,
  compared with the request's host after normalization.
- `header`, `query`, `cookie`: a named field, parameter or cookie that is
  present, absent, equal to, starts with, ends with or contains a value, or
  matches a regular expression, optionally ignoring case. Header names
  ignore case (RFC 9110 §5.1) and repeated field lines are tested as their
  combined value (§5.3). Query parameters are decoded as
  `application/x-www-form-urlencoded` (WHATWG URL Standard), and a
  repeated parameter holds when any of its values does. Cookies are the
  pairs of every `Cookie` field (RFC 6265 §5.4; RFC 9113 §8.2.3 for HTTP/2's
  split fields).
- `client`: the client's address, after trusted proxies, in one of the
  listed networks or addresses; an IPv4-mapped IPv6 address is compared as
  IPv4.
- `user_agent`, `referer`: tests on those fields.
- `content_type`: the request's media type, compared without parameters and
  ignoring case (RFC 9110 §8.3.1), against types such as `application/json`
  or ranges such as `text/*`.
- `any`, `all` and `not` group conditions, so routes express or, and and
  negation. The model follows the Gateway API's `HTTPRoute` matches, where
  conditions of one match are and-ed, generalized to nested groups.

Conditions are checked after the path, cheapest first; regular expressions
are compiled once per snapshot under the same size limit as path patterns.
Ranking is unchanged: priority, then the more specific path, then a
concrete host constraint, then the route's ID; a route whose conditions do
not hold is skipped for the next.

**Contracts.** The model, the configuration language, the IR and the API
carry conditions as one tree. A snapshot whose routes have conditions
requires the `route.conditions` capability, so a gateway that does not
evaluate them refuses the snapshot instead of ignoring them.

**Testing routes.** The route tester takes a request as method, URL,
headers and client address, compiles the draft as an apply would, and
answers the site and route that would take it, with, for every route ranked
before it, the first part that did not hold.

## Alternatives

- Conditions only in a scripting extension: every operator would write code
  for what is a declarative match, and the tester could not explain it.
- A tester in the gateway, reached over its API: it would only see the
  active snapshot, not the draft about to be applied.
- Or-ing several matches per route, as `HTTPRoute` does: nested groups say
  the same and also allow negation.

## Consequences

- The gateway's routing table keeps only what serving needs beyond
  matching: targets, security policies, access logs and site settings.
- Adding a kind of condition changes `panel-routing`, the IR and the
  language together, and a new capability version guards the gateway.
