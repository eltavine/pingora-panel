# 0014: Identity and access

Status: accepted.

## Context

`panel-api` is the only public management entry point and owns the
`identity` schema ([ADR 0007](0007-service-processes-health-and-discovery.md)).
Until now it trusted a caller-supplied `x-actor` header and therefore only
listened on loopback. Every later feature — approvals, scoped operators,
audit of logins — needs authenticated principals and a default-deny
authorization decision on every request. The design follows NIST SP
800-63B-4 for passwords, throttling and sessions, and the OWASP Password
Storage, Session Management and CSRF Prevention cheat sheets.

## Decision

- **Placement.** `panel-identity` holds the rules — accounts, password
  policy and hashing, sessions, API tokens, the permission catalog and
  authorization — behind an `IdentityStore` port. `identity-postgres`
  implements the port in the `identity` schema and records every change
  with its event through the outbox, so the audit trail covers logins,
  sessions, tokens and accounts ([ADR 0013](0013-audit-trail.md)). Requests
  are authenticated in process; no other service sees credentials.
- **Passwords.** Passwords are normalized to NFC, must be 15 to 1024
  characters counted as code points, and are refused when a strength
  estimate finds them common, predictable or derived from the account name,
  with the reason and a suggestion; no composition rules apply. They are
  stored as Argon2id PHC strings (19 MiB, 2 iterations, 1 lane), keyed with
  a pepper from the deployment's secrets when one is configured, and rehashed
  at login when the parameters change.
- **Throttling.** Each failed login adds a delay before the account accepts
  the next attempt, doubling from 30 seconds after the fifth consecutive
  failure up to an hour; the hundredth disables the password until an
  Administrator unlocks it. Logins are also limited per client address.
  Unknown accounts take the same time and give the same answer as wrong
  passwords.
- **Sessions.** A login creates a session whose secret is 256 random bits;
  only its SHA-256 hash is stored. Browsers receive it as a
  `__Host-ppanel_session` cookie that is `Secure`, `HttpOnly` and
  `SameSite=Strict`; the command line receives it once in the response body
  and sends it as a bearer token. A session ends after an hour without
  activity or a day after login, on logout, on revocation, and when the
  account is disabled or its password changes.
- **CSRF.** Unsafe requests authenticated by the cookie must carry the
  session's CSRF token in `x-csrf-token`, compared in constant time, must
  not come from another site according to `Sec-Fetch-Site`, and, without
  that header, must carry an `Origin` equal to the panel's own. The token
  is an HMAC of a fixed label keyed with the session secret: bound to the
  session, recomputed from the cookie and returned with the current
  session, and unknowable without the cookie. Bearer credentials are not
  sent by browsers on their own and skip these checks.
- **API tokens.** Tokens are `ppat_` followed by 256 random bits, shown once,
  stored as SHA-256 hashes, and have a name, an expiry of at most a year and
  a set of permissions that can only narrow their owner's. Revoking or
  disabling the owner ends them.
- **Authorization.** Permissions are named actions such as `config.read`
  or `config.apply`, listed in one catalog with their descriptions. Roles
  are sets of permissions stored as data; the built-in Administrator,
  Operator, Viewer and Auditor roles are seeded rows that code never refers
  to by name. A binding grants a role to an account globally. Every route
  of the API declares the permission it needs in one table; a route without
  an entry is refused, and a request is allowed only when the principal
  holds the permission.
- **Bootstrap.** While no account exists, the API accepts one setup request
  carrying the one-time bootstrap token from the deployment's secrets and
  creates the first Administrator; the token is then spent.
- **Actors.** Commands carry the authenticated account as their actor; the
  `x-actor` header no longer exists.

## Consequences

- The public listener can be exposed through the gateway, which
  terminates TLS, without trusting any header for identity.
- Adding an endpoint requires naming its permission, and the API's tests
  check that every documented route has one.
- Identity provider sign-in, scoped bindings, service accounts and
  approvals extend the same principals, permissions and store.
