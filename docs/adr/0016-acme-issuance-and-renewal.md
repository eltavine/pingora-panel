# 0016: ACME issuance and renewal

Status: accepted.

## Context

[ADR 0015](0015-certificates-and-secret-material.md) keeps certificates in
the inventory of `automation-service` and decides that ACME accounts belong
to a directory, that the gateway answers HTTP-01 challenges from a
directory the automation service writes, that DNS-01 goes through a DNS
provider port and that certificates renew in the window their CA suggests.
This record settles how: which ACME implementation, where accounts and
automatically managed certificates live, how issuance runs and is retried,
and what operators are told. It follows RFC 8555, RFC 8738 for IP
identifiers, RFC 9773 for renewal information and Let's Encrypt's guidance
for clients: renew when a third of the validity remains or when renewal
information says so, and back off after failures.

## Decision

- **Client.** `panel-acme` wraps `instant-acme` with the ring provider. It
  registers accounts, with external account binding (RFC 8555 §7.3.4),
  orders certificates and reads renewal windows. Public CAs are trusted
  through the platform's roots; a directory may instead name a CA bundle,
  for private CAs. Every certificate gets a new ECDSA P-256 key from
  `panel-certificates`, which also derives the RFC 9773 identifier of a
  certificate from its authority key identifier and serial.
- **Accounts.** An account belongs to one directory: Let's Encrypt, its
  staging directory or any other HTTPS URL. Registering requires agreeing to
  the CA's terms. The MAC key of an external account binding is used once
  and not kept. The account key travels in opaque credentials that are
  sealed like certificate keys.
- **Automatic certificates.** An automatic certificate names an account,
  its names and a challenge kind, and has the ID of the inventory
  certificate it produces: the first issuance creates or replaces that
  certificate and every renewal replaces it, so references never change and
  an uploaded certificate can be handed over to ACME. Removing the
  automation keeps the certificate.
- **Challenges.** HTTP-01 key authorizations are files in `acme-challenge/`
  inside the gateway's secret directory, which every listener serves before
  routing; requests for other tokens reach the site. Wildcard names need
  DNS-01, which publishes `_acme-challenge` TXT records through a
  `DnsProvider` and waits for them to propagate. Whatever was presented is
  removed after the order, whether it succeeded or not.
- **DNS providers.** DNS providers are kept by the automation service with
  their secrets sealed like keys, and a DNS-01 certificate names one. The
  first kind sends RFC 2136 dynamic updates to the zone's primary over TCP,
  signed with TSIG (RFC 8945, HMAC-SHA256 or HMAC-SHA512), and accepts only
  answers whose signature verifies, so BIND, Knot DNS, PowerDNS and most
  authoritative servers work without vendor APIs. Messages and signatures
  come from `hickory-proto` with its `ring` backend; the panel only frames
  them over TCP. Its signed updates are tested byte for byte against BIND's
  `nsupdate`, and against a BIND primary when one is configured.
- **Jobs.** Issuance is a `certificate.issue` job, so it survives restarts
  and runs once across replicas, and a certificate is issued by one job at a
  time. Creating an automatic certificate or asking to renew it enqueues the
  job at once; an hourly `certificate.renewal-check` schedule enqueues it
  for the others when they are due.
- **Renewal time.** After each issuance, and again whenever the CA's
  `Retry-After` allows, the renewal check asks for the CA's renewal window
  and picks a uniformly random moment within it. Without renewal
  information, a certificate renews when a third of its validity remains. A
  renewal order names the certificate it replaces.
- **Failures.** A failed attempt keeps the CA's problem with the automatic
  certificate and is published as `tls.acme.certificate.failed`, which
  alerting subscribes to; the next attempt waits one hour, doubling with
  every consecutive failure up to a day.
- **Reminders.** The renewal check publishes `tls.certificate.expiring` when
  any certificate of the inventory, automatic or not, comes within 30, 14,
  7, 3 and 1 days of its end and when it has expired, once per threshold
  and certificate version.
- **Access.** Reading accounts and automatic certificates needs
  `certificate.read`; changing them, and asking for a renewal, needs
  `certificate.manage`. No interface returns account keys.
- **Tests.** Issuance is tested against Pebble, Let's Encrypt's test CA,
  with its DNS test server resolving every name to the test host; the
  development services script and CI start both from pinned release
  binaries.

## Consequences

- HTTP-01 needs every name to reach a listener on port 80 of a gateway
  sharing the automation service's secret directory; DNS-01 needs a DNS
  provider that can update the names' zones.
- Accounts registered with one directory cannot order from another; moving
  CAs means a new account and new automatic certificates.
- Renewal windows that move earlier, such as during a mass revocation,
  are followed within the CA's `Retry-After`, at most a day.
