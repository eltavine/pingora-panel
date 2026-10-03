# 0015: Certificates and secret material

Status: accepted.

## Context

The gateway terminates TLS with certificates chosen by SNI from the active
snapshot; a TLS profile names a certificate chain and a private key in the
gateway's secret directory, and nothing in the panel puts them there
([ADR 0010](0010-pingora-data-plane.md)). Operators need to upload, inspect,
generate and automatically issue and renew certificates without handling
key files, and private keys must not appear in configuration documents,
events, logs or the snapshot store. The design follows RFC 5280 for
certificates, RFC 9525 for matching host names, RFC 8555 and RFC 9773 for
ACME issuance and renewal, and the CA/Browser Forum baseline requirements
for key sizes.

## Decision

- **Placement.** `panel-certificates` holds the rules — parsing and
  inspecting chains and keys, checking that they belong together, matching
  host names and generating self-signed certificates — without I/O.
  `panel-secrets` seals secret material behind a `SecretVault` port.
  `automation-service` owns the certificate inventory in its schema,
  records every change with its event through the outbox, and serves it
  over gRPC; `panel-api` reaches it through a port, like the audit trail.
- **Material at rest.** Certificate chains are public and stored as PEM.
  Private keys are sealed: each gets a random 256-bit data key, is
  encrypted with AES-256-GCM, and the data key is encrypted with a master
  key, with the owning record bound in as associated data so a sealed value
  cannot be moved to another record. Master keys come from the deployment's
  secrets, one per line; the first seals, the others still open values
  sealed with them, and each sealed value names its master key by a
  fingerprint so keys can rotate.
- **Intake.** A chain must be PEM X.509 certificates, leaf first, each
  issued by the next. A key may be PKCS#8, PKCS#1 or SEC1 and must be an
  RSA key of at least 2048 bits or an ECDSA P-256 or P-384 or Ed25519 key;
  it is loaded with the gateway's TLS provider and must match the leaf's
  public key, so what the panel accepts the gateway can serve. Expired
  certificates are refused. The inventory records names, issuer, serial,
  validity, the key's algorithm and the SHA-256 fingerprints of the leaf
  and of its public key.
- **Names.** A certificate covers a host when a DNS name or IP address in
  its subject alternative names matches; the subject's common name is
  ignored. A wildcard is only a whole left-most label and covers exactly one
  label. Names compare case-insensitively in their ASCII form.
- **Self-signed.** Generated certificates use ECDSA P-256 keys, list the
  requested names and are marked as self-signed.
- **Delivery.** `automation-service` writes each certificate's chain and
  key into the gateway's secret directory as `cert-<id>.pem` and
  `cert-<id>.key`, readable only by their owner, through a temporary file
  and a rename, after every change and for the whole inventory at start; it
  removes the files of deleted certificates and never touches other files.
  The gateway reloads certificate material that changes on disk without a
  new revision, and keeps serving the previous material when the new one
  fails to load.
- **References.** A TLS profile names either a certificate of the inventory
  or files that operators place themselves; the former compiles to the
  delivered file names.
- **Issuance.** ACME accounts belong to a directory such as Let's Encrypt
  or one with external account binding. HTTP-01 challenges are answered by
  the gateway from a challenge directory the automation service writes;
  DNS-01 challenges go through a DNS provider port, and wildcard names
  require them. Certificates renew in the window the CA suggests through
  renewal information, or when a third of their lifetime remains; failures
  retry with backoff and, like approaching expiry, are published as events.
- **Access.** Reading the inventory needs `certificate.read`; changing it
  needs `certificate.manage`. No interface returns a private key.

## Consequences

- Certificates renew and change without a configuration revision, and the
  configuration only names them.
- A gateway on another host needs the secret directory replicated until
  secret material is delivered over the internal API.
- Losing the master keys makes stored private keys unrecoverable; they are
  part of the backup like the database.
