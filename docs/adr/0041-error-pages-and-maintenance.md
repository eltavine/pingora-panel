# 0041: Error pages, maintenance with an allowlist, robots.txt and favicons

Status: accepted. Builds on [ADR 0017](0017-request-security-policies.md),
[ADR 0034](0034-site-files.md),
[ADR 0036](0036-route-conditions.md) and
[ADR 0040](0040-rewrites-and-internal-redirects.md).

## Context

The gateway answers errors it makes itself — no route, a refusing security
policy, an upstream that cannot be reached or is shedding load — with a
short plain-text body, and passes upstreams' errors on as they are.
Operators want their own pages for the commonest of them, 404, 403, 502 and
503 above all, as nginx's `error_page` gives them. Maintenance so far means
giving a site the fixed-response action, which takes the site away from
everyone, its operators included, and loses the action it had. Two files
every site is asked for, `robots.txt` and `favicon.ico`, need a route each.

## Decision

**Error pages.** Sites and routes list error pages; a route that lists any
replaces its site's, as an nginx location's `error_page` replaces the
server's. A page names the statuses it answers, between 400 and 599, and
how: with a body, a template of request variables with its media type
(`text/html` unless written); with a file below the gateway's static root,
read when it is needed, whose media type follows its extension; or with a
redirect (RFC 9110 §15.4) to a URL template. A body or file page keeps the
error's status unless it names another, as nginx's `=code`. Pages answer
the errors the gateway makes once a site is known: no route, an internal
route reached from outside, a security policy's refusal without its own
body, a file that is missing or not allowed, an upstream that cannot be
reached, times out or sheds load, a rewrite cycle, and a fixed response
without a body, as nginx's error pages answer `return 503;`. Upstreams'
own error responses are answered too where the list says so, as nginx's
`proxy_intercept_errors` does; their body is dropped. A page never answers
a response that has started.

**Maintenance with an allowlist.** A site may be in maintenance: every
request to it, from a client outside the allowlist, gets the maintenance
response — 503 with `Retry-After` (RFC 9110 §10.2.3) unless another status
is written, and the site's 503 page unless it has its own body — while
clients in the listed networks or addresses, after trusted proxies, reach
the site as it is. Maintenance is checked once the site is known, after
its HTTPS and alias redirects and before its rewrites and routes, so the
site keeps its action and routes for when maintenance ends, and ACME
challenges, answered before any site, keep certificates renewing.

**robots.txt and favicons.** A site may answer `/robots.txt` itself:
allowing every crawler, disallowing every one (RFC 9309 §2.2) or with its
own text. It may answer `/favicon.ico` with 204 so browsers stop asking,
with a file below the gateway's static root, or with a redirect. Both are
written once on the site and compile to exact-path routes ahead of the
site's own, so they need nothing new from the gateway.

**Contracts.** The model, the configuration language, the IR and the API
carry error pages and maintenance; a snapshot with error pages requires the
`response.error-pages` capability and one with maintenance
`site.maintenance`. robots.txt and favicons compile to the routes and
rewrites gateways already know.

## Alternatives

- Error pages as internal redirects to a route, as nginx writes them: an
  error found while proxying cannot start a request over in Pingora, so
  such pages would answer some errors and not others.
- Maintenance as a route with a client condition: the route would compete
  with the site's own by priority, and rewrites would run before it.
- robots.txt and favicons as gateway features: routes and rewrites already
  express them, and a gateway gains nothing to keep in step.

## Consequences

- A gateway that does not know error pages or maintenance refuses
  snapshots using them instead of ignoring them.
- Requests that fail without a page, and sites without pages, are answered
  as before.
- A file page is read when an error needs it, so a changed file is served
  without a new revision, as static content is.
