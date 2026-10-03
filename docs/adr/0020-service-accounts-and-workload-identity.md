# 0020: Service accounts and workload identity

Status: accepted.

## Context

Pipelines and other programs change configuration too. Giving them a
person's token ties them to that person and outlives their job; long-lived
secrets stored in a CI system leak. Most CI systems and clouds now issue
their jobs short-lived OpenID Connect tokens that name the repository,
branch or workload, so a job can prove what it is without any stored
secret. The design follows the JWT profile for OAuth 2.0 access tokens
(RFC 9068) for the claims it checks and the token exchange pattern of
RFC 8693, and reuses the panel's own JOSE verification
([ADR 0018](0018-identity-provider-sign-in.md)).

## Decision

- **Service accounts.** An account can be created as a service account. It
  has no password, never signs in with one or through an identity provider,
  and cannot be a break-glass account. Its roles decide what it may do like
  any account's. Account managers issue its API tokens, which never hold
  permissions the service account lacks, and revoke them as for any
  account; every token issued is audited with who issued it.
- **Workload identities.** An account manager trusts tokens from an issuer
  for a service account: the issuer URL, the audience the token must name,
  the subject, exactly or as a prefix ending in `*`, and further claims that
  must equal given values, such as a repository or branch. Each trust says
  how long the sessions it grants last, five to sixty minutes. Issuers must
  use HTTPS, except on loopback for tests.
- **Exchange.** A workload posts its token to
  `/api/v1/auth/workload`. The panel reads the issuer, refuses issuers no
  enabled trust names, fetches that issuer's discovery document and keys,
  verifies the signature with the same algorithms as sign-in (no `none`, no
  shared-secret algorithms) and checks the issuer, the expiry, not-before
  and issue time with a minute of leeway. It then takes the first enabled
  trust whose audience, subject and claims match and opens a bearer session
  for its service account that ends when the trust says. Exchanges and
  refusals are audited like sign-ins, with the trust they used; refusals
  are counted against the client address like failed logins.

## Consequences

- No secret has to be stored in a CI system to change configuration; what a
  job may do follows the service account's roles and the trust's
  conditions.
- Service accounts never create their own API tokens, so a workload's
  session cannot turn itself into a long-lived secret; only account managers
  issue them.
- The verification code is shared with identity provider sign-in, so both
  keep the same algorithm rules and tests.
