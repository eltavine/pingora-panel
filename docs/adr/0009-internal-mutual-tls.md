# 0009: Internal mutual TLS and certificate rotation

Status: accepted.

## Context

Services call each other over gRPC: `panel-api` calls `config-service`, and
`config-service` calls `gatewayd`. Until these calls are authenticated,
every plaintext listener is confined to loopback. That rules out separate
containers on a bridge network and lets any local process impersonate a
service. Each service needs an identity that its peers can verify, the
right to call only the APIs it uses, and credentials that rotate without
restarts or manual steps.

## Decision

- **Authority.** `panel-pki` creates one certificate authority per
  installation in a trust domain, `pingora-panel.internal` by default (the
  `.internal` TLD is reserved for private use). The authority is created on
  first use and reloaded afterwards. Loading refuses an authority that
  belongs to another trust domain or that exists only partially.
- **Certificates.** Service certificates follow the RFC 5280 profile. They
  use ECDSA P-256 keys, are not CA certificates, carry only the
  `digitalSignature` key usage, and allow both server and client
  authentication. Each names its service by the DNS identity
  `<service>.<trust domain>`, which peers authenticate, and by the SPIFFE ID
  `spiffe://<trust domain>/service/<service>`. They are valid for 24 hours by
  default and backdated five minutes for clock skew.
- **Credential files.** A service's private key and certificate are written
  together as PEM with one atomic rename, readable by the owner only; renewal
  reads the validity from the certificate itself, so a replaced file is never
  judged by stale metadata. A rotating service therefore never reads a key
  and certificate from different issuances.
- **Rotation.** `panel-bootstrap pki` renews any credentials past two thirds
  of their lifetime, so the default lifetime leaves eight hours to recover a
  failed renewal. Services poll their credential files and swap the TLS
  configuration in place: new connections use the new certificate, and
  established ones finish with the certificate they negotiated.
- **Transport.** `panel-tls` permits TLS 1.3 only, with ALPN `h2`. Servers
  require a client certificate from the installation's authority. Clients
  verify the server's DNS identity regardless of the address they dial, so
  services can sit behind any host name or IP address. Handshakes run in
  their own tasks under a timeout.
- **Authorization.** A policy layer opens `grpc.health.v1.Health` and
  `pingora.panel.platform.v1.ServiceInfo` to every authenticated peer.
  Every other service admits only the callers it lists: the gateway API
  admits `config-service`, and the publication API admits `panel-api`.
  Calls without a client certificate are unauthenticated, and calls from
  other identities are refused.
- **Binding.** A plaintext listener stays loopback-only. A service may bind
  beyond loopback only after its credentials load and its listener serves
  mutual TLS, as the security model requires. Operational HTTP endpoints
  stay on loopback.

## Consequences

- Services can run in separate containers and hosts with every internal
  call authenticated and authorized by identity.
- Revocation is by expiry: a compromised key is useless within its
  lifetime, and shortening the lifetime trades renewal frequency for
  exposure.
- Rotating the authority itself requires a trust bundle that holds both
  authorities during the change. The trust file already carries a bundle,
  so that rotation can be added without changing the file format.
