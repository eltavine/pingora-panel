# 0040: Request rewrites and internal redirects

Status: accepted. Builds on [ADR 0010](0010-pingora-data-plane.md),
[ADR 0036](0036-route-conditions.md),
[ADR 0037](0037-http-policies.md) and
[ADR 0039](0039-lua-scripts.md).

## Context

Operators change the path a request travels with: an application mounted
under `/api` expects `/`, another expects a version prefix, and old URLs
move to new ones by pattern. They also want one URL served with what
another route does, without a round trip to the client. nginx does this
with `rewrite` and its flags, `proxy_pass` with a URI part, internal
locations and internal redirects. The gateway so far offered it only to
Lua handlers (`ngx.req.set_uri`, `ngx.exec`), with nginx's limit of ten
URI changes a request may make.

## Decision

**Rewrite rules.** Sites and routes carry an ordered list of rules. A
site's rules run before a route is chosen, as nginx's rewrite directives
at the server level do; a route's run once it is chosen, before its
access phase and before its `rewrite_by_lua` handler. A rule is one of:

- `strip_prefix P`: removes `P` when the path is `P` or continues it with
  a `/` segment, so `/api` turns `/api/users` into `/users` and `/api`
  into `/`, and leaves `/apiary` alone;
- `add_prefix P`: puts `P` before the path;
- `set_uri T`: replaces the path with the template `T`;
- `rewrite R T [flag]`: nginx's rewrite. When the regular expression `R`
  matches the path, the path becomes `T`, where `$1` to `$9` and named
  groups take what `R` captured.

**Paths and queries.** Rules see the path as route matchers do: normalized
as RFC 3986 §6.2.2 says, with dot segments removed (§5.2.4) and
percent-encodings other than of unreserved characters kept, so an encoded
`/` never becomes a segment boundary. Captures and literal text go into
the new path as written; request variables are percent-encoded as a path
segment requires (§3.3). A new path that is not absolute is refused with
500. A replacement with a `?` sets the query and, as in nginx, appends
the request's own unless it ends with `?`; one without keeps the query.

**Flags.** `last` stops the rules and chooses the route again for the new
path; `break` stops them and keeps the route; `redirect` and `permanent`
answer 302 and 301 (RFC 9110 §15.4.3, §15.4.2) with the new URI as
`Location`, a relative reference unless the replacement starts with
`http://`, `https://` or `$scheme`, which redirects without a flag as
well. Without a flag the next rule runs, and a route whose rules changed
the path is chosen again after its last rule unless a `break` ended them,
as nginx repeats its location search. The other three rules never choose
a route again: they shape what the route sends on, as nginx's
`rewrite ... break` does. Each new choice counts against the ten URI
changes a request may make, with scripts' jumps and internal redirects;
the eleventh is answered with 500, as nginx answers it.

**What follows sees the change.** The new path and query are what an
upstream receives and what static content resolves. `$uri` is the
current path, `$args` (or `$query_string`) the current query, `$is_args`
a `?` when there is one, `$arg_NAME` a parameter, and `$request_uri` the
client's request target, unchanged.

**Internal redirects.** A route's action may be an internal redirect to a
path template or to a named route (`@name`): the request is served as if
it had asked for that target, with its method, headers and body, and the
client sees no redirect. A path target starts over with the site's rules,
a named route with its own, as `ngx.exec` does. A route may be internal:
it takes only requests that reached it through a rewrite, an internal
redirect or a script, and answers others 404, as nginx's `internal`
locations do.

**Expressions.** Patterns use Rust's `regex` syntax, matched in linear
time, so no pattern can make a request take exponential time; patterns
with back-references or look-around, which PCRE has, are refused when the
configuration is checked. A rule set has at most 64 rules.

**Contracts.** The model, the configuration language, the IR and the API
carry rules, internal routes and the internal redirect action; a snapshot
with any of them requires the `route.rewrite` capability.

## Alternatives

- Scripts only: every operator would write Lua for the commonest path
  changes, and the console could not show or check them.
- Changing only the path sent upstream, as `proxy_pass` with a URI does:
  static content and route choice would not see it, and nginx's rule of
  replacing the matched location prefix surprises even its users.
- PCRE: back-tracking patterns can be made to run for a very long time on
  a crafted path.
- Absolute `Location` values for relative replacements, as nginx builds by
  default: RFC 9110 §10.2.2 allows relative references, which keep the
  scheme and host the client used.

## Consequences

- A gateway that does not know rewrites refuses snapshots using them
  instead of ignoring them.
- Sites and routes without rules pay nothing; patterns compile once per
  snapshot.
- A request chooses a route at most eleven times.
