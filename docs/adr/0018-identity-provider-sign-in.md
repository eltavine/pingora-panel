# 0018: Identity provider sign-in and break-glass access

Status: accepted.

## Context

Teams keep their people in an identity provider and expect the panel to
use it: one sign-in, groups that decide what someone may do, and access that
ends when the provider removes them. Local passwords
([ADR 0014](0014-identity-and-access.md)) stay for the first Administrator
and for emergencies when the provider is unreachable. The design follows
OpenID Connect Core 1.0 and Discovery 1.0, OAuth 2.0 (RFC 6749) with PKCE
(RFC 7636), the OAuth 2.0 Security Best Current Practice (RFC 9700) and JSON
Web Signature and Keys (RFC 7515, RFC 7517, RFC 7518).

## Decision

- **Providers.** An Administrator configures named OpenID Connect
  providers: an issuer URL, a client ID and, for confidential clients, a
  client secret sealed with the deployment's master keys like other secret
  material ([ADR 0015](0015-certificates-and-secret-material.md)); the
  scopes requested, `openid profile email` by default; the claims that give
  the username, display name, email and groups; mappings from groups to
  roles; and whether unknown people get an account on first sign-in. The
  issuer and its endpoints must use HTTPS, except on loopback for tests.
  Changes are audited, and secrets are never returned.
- **Discovery and keys.** The panel reads the provider's discovery document
  from `<issuer>/.well-known/openid-configuration`, requires its `issuer` to
  equal the configured one exactly, and caches it and the JSON Web Key Set
  for an hour, fetching the keys again, at most once a minute, when a token
  names a key it does not know.
- **Sign-in.** The console starts the authorization code flow with PKCE
  (`S256`), a random `state` and a random `nonce`. The attempt is kept on the
  server for ten minutes under a hash of the state and used once, and the
  browser holds the state in a short-lived `__Host-` cookie, so a callback
  from another browser is refused. The redirect URI is the panel's first
  public origin followed by `/api/v1/auth/oidc/<provider>/callback`, and
  the page to return to must be a path of the panel itself.
- **Token validation.** The ID token's signature is verified with the
  provider's keys, found by `kid`, for RS256, RS384, RS512, PS256, PS384,
  PS512, ES256, ES384 and EdDSA; `none` and HMAC algorithms are refused. Its
  `iss` must equal the issuer, `aud` must contain the client ID, `azp` must
  be the client ID when present, `exp` must lie ahead and `iat` not ahead,
  with a minute of leeway, and `nonce` must match the attempt. Signatures are
  checked with `ring`, which the panel already uses for TLS, rather than a
  second cryptography stack: the JOSE layer is small, and it is tested
  against keys and tokens from independent tools.
- **Accounts.** A person is linked to an account by the provider and the
  token's `sub`, never by email or username alone. On first sign-in, an
  account is created when the provider allows it, named after the username
  claim; a name already taken by another account is refused rather than
  linked. Roles granted through group mappings are recalculated at every
  sign-in and kept apart from roles an Administrator granted by hand.
- **Ending access.** When the provider issues refresh tokens, they are
  sealed with the session, and a session older than fifteen minutes since
  its last check refreshes them before it is used again. When the provider
  refuses, the session ends; an account the provider created is disabled.
  Without refresh tokens, sessions end with their usual idle and absolute
  lifetimes.
- **Break-glass access.** Once a provider exists, Administrators may
  restrict password sign-in to accounts marked as break-glass. Those keep
  their password whatever happens to the provider, and every sign-in with
  one is recorded as a critical audit event so that it is noticed and
  reviewed.

## Consequences

- The panel becomes a relying party of each provider and needs its public
  origin configured; sign-in fails closed when the provider or its keys are
  unreachable, while break-glass accounts keep working.
- The JOSE verification code is the panel's own and must keep its tests
  against independently produced keys and tokens.
- Service accounts, workload identity, scoped bindings and approvals build
  on the same accounts, roles and audit events.
