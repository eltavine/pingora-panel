# 0021: Scoped and conditional grants

Status: accepted.

## Context

Roles grant their permissions everywhere and always
([ADR 0014](0014-identity-and-access.md)). Teams split work by site: the shop
team edits the shop's sites but not the intranet's, contractors may change
things only during a maintenance window, and an on-call engineer needs
access for a night, not forever. The authorization model is
`subject × action × resource scope × condition`, denying by default; sites
already belong to groups in the configuration model.

## Decision

- **Grants.** Besides the roles an account holds everywhere, an account
  manager can grant it a role with a scope and conditions. The scope is
  everything, one site group (the `group` of sites in the configuration) or
  one site. Conditions limit when the grant counts: an expiry, client
  networks the request must come from, and weekly windows in UTC. A grant
  counts for a request only while all of its conditions hold; grants are
  audited like role changes.
- **What scopes cover.** Only the configuration permissions,
  `config.read`, `config.write` and `config.apply`, can be scoped, since only
  sites have groups. A scoped grant of a role holding other permissions
  confers those only when its scope is everything.
- **Evaluation.** The API evaluates an account's grants on every request
  with the time and the client address it sees. A permission held through a
  role or a grant scoped to everything is held without restriction. A
  configuration permission held only through scoped grants lets the request
  through with a site scope attached to it: for each permission, the groups
  and sites it covers.
- **Enforcement where the data lives.** The scope travels in the request
  context to the configuration service, which knows each site's group.
  There, reads list and show only sites in scope, changes are refused unless
  the site is in scope both before and after the change, and applying is
  refused unless every change in the plan is a site in scope. Everything
  that is not a site, such as upstreams, listeners, TLS profiles and the
  configuration files as a whole, needs the permission without restriction.
  Refusals are audited like other refused changes.

## Consequences

- An account sees fewer sites than exist when it is scoped; summaries and
  plans count only what it may see.
- Conditions are evaluated per request, so an expired grant stops working
  without anyone revoking it, and a network condition follows the client
  address the API trusts (ADR 0014, trusted proxies).
- Other services can honour the same site scope later without changing how
  grants are kept.
