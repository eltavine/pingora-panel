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
  with a minute of leeway, and `nonce` must match the attempt. Tokens are
  parsed and their registered claims validated by `jsonwebtoken`, whose
  pluggable crypto provider is filled with `ring`, which the panel already
  uses for TLS, rather than a second cryptography stack such as the RustCrypto
  `rsa` crate. Headers naming critical extensions are refused, and keys are
  chosen by `kid`, type, curve, `use`, `key_ops` and `alg` (RFC 7517 §4).
- **Accounts.** A person is linked to an account by the provider and the
  token's `sub`, never by email or username alone. On first sign-in, an
  account is created when the provider allows it, named after the username
  claim; a name already taken by another account is refused rather than
  linked. Roles granted through group mappings are recalculated at every
  sign-in and kept apart from roles an Administrator granted by hand.
- **Ending access.** When the provider issues refresh tokens, they are
  sealed with the session, and every fifteen minutes the panel refreshes
  them in the background, keeping the tokens the provider rotates to. Each
  check claims its sessions first, so that several API replicas never spend
  the same refresh token twice. When the provider refuses, the session
  ends and the person has to sign in again; the account stays as it is,
  because a refusal cannot tell a person who left from a provider session
  that simply ran out. Disabling or deleting a provider ends its sessions at
  once. Without refresh tokens, sessions end with their usual idle and
  absolute lifetimes.
- **Break-glass access.** Once a provider is enabled, Administrators may
  limit password sign-in to accounts marked as break-glass, provided an
  enabled one can manage accounts; while the limit holds, the last such
  account cannot lose either. Break-glass accounts keep their password
  whatever happens to the provider, and every sign-in with one is recorded
  as its own audit event, `identity.break_glass.used`, in the same
  transaction as the login, and shown on the console's sign-in providers
  page so that it is noticed and reviewed. Other accounts are refused even
  with the right password, which tells nobody without it anything.

## Alternatives

Compared in October 2026:

- **`openidconnect` 4.0.1**, the most used Rust relying-party library,
  covers discovery, the code flow with PKCE and ID token verification. It
  verifies signatures with RustCrypto's `rsa` 0.9, `p256`, `p384` and
  `ed25519-dalek`, a second cryptography stack beside `ring`. No `rsa`
  release fixes RUSTSEC-2023-0071, so the dependency policy would need a
  standing exception, and the crate brings older majors of `base64`,
  `rand`, `thiserror` and `itertools` beside the panel's. Its latest
  release is from July 2025.
- **`oauth2` 5 for the code exchange alone** would replace the token
  request and PKCE, a small part of the client, and still bring `rand` 0.8
  and RustCrypto's `sha2`.
- **The panel's client** keeps the protocol steps (discovery, the code
  flow, PKCE, `state`, `nonce`, key selection and refresh) in about 1,400
  lines with their unit tests, and leaves JWT parsing and claim checks to
  `jsonwebtoken` and signatures to `ring`.

The choice is revisited when `openidconnect` can verify signatures through
`ring` or another provider the panel already uses.

## Consequences

- The panel becomes a relying party of each provider and needs its public
  origin configured; sign-in fails closed when the provider or its keys are
  unreachable, while break-glass accounts keep working.
- The panel's own JOSE code is limited to the `ring` provider and key
  selection, and keeps its tests against the RFC 7515 and RFC 8037 examples
  and keys from independent tools.
- Service accounts, workload identity, scoped bindings and approvals build
  on the same accounts, roles and audit events.
