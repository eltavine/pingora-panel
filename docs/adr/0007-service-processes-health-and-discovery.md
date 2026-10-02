# 0007: Service processes, health, degraded mode and discovery

Status: accepted.

## Context

The control plane runs as several processes (`panel-api`, `config-service`,
`automation-service`, `observability-service`) next to `gatewayd`, PostgreSQL
and NATS. Each process must start in any order, tell an orchestrator whether
it can serve, keep serving what it still can while a dependency is down, and
let its peers find it and agree on which protocol additions they can rely on.
Each concern needs one implementation rather than one per service.

## Decision

- **Composition.** `panel-control-runtime` composes every control-plane
  process. A service declares its name, schema, migrations, dependencies and
  gRPC services. The runtime binds the operational and gRPC listeners first,
  so a taken port fails the start. PostgreSQL and NATS are reached lazily,
  and schema migration, broker provisioning, registration and outbox relay
  run in the background with jittered exponential backoff. Settings come from
  `PINGORA_PANEL_*` environment variables. Secrets may instead be read from
  the file named by `<NAME>_FILE`, as container secrets are mounted.
  Plaintext listeners must be loopback addresses until internal transports
  are authenticated. `panel-bootstrap` provisions roles, schemas, streams and
  the registry idempotently.
- **Readiness.** `panel-health` evaluates checks concurrently under timeouts
  and isolates checks that stall or panic. Each check declares the impact of
  its failure: *required* makes the service unavailable, *degrading* keeps
  reads and suspends writes, *informational* is only reported. Documents use
  the `application/health+json` shape of the IETF health check response
  format draft (`pass`, `warn`, `fail`). `/livez` and `/readyz` answer from
  the last published report: 200 unless the service fails, otherwise 503.
  The same readiness is mirrored into the standard `grpc.health.v1.Health`
  service. Healthy services are re-evaluated at their interval, and failing
  or degraded ones every 500 ms. Endpoints never probe dependencies per
  request.
- **Degraded mode.** While degraded, the REST adapter serves the safe methods
  of RFC 9110 section 9.2.1 and refuses all others. While unavailable, it
  refuses every request. Refusals are `503 Service Unavailable` with
  `Retry-After` and a retryable `UNAVAILABLE` problem. `gatewayd` keeps
  serving its last known good snapshot whatever the control plane's state.
- **Description and discovery.** Every instance has a descriptor: service
  name, UUIDv7 instance ID, build and schema versions, the revision range it
  speaks of each protocol package, and its capabilities as `name@version`.
  `pingora.panel.platform.v1.ServiceInfo/Describe` returns the descriptor
  over gRPC. Instances register the Protobuf-encoded descriptor in a
  JetStream key-value bucket, one key per instance, refreshed well inside the
  bucket's expiry, so a stopped instance disappears without shared database
  tables. `GET /api/v1/platform/services` lists the directory with the time
  it was read.
- **Negotiation.** A protocol is a versioned Protobuf package. Within a major
  version, each revision only adds to the previous one. Both sides use the
  highest revision they share. A peer whose range does not overlap is
  refused with `UNSUPPORTED_CAPABILITY` before any call relies on it.
  `panel-contracts` records the range each build speaks for every package.

## Consequences

- Services start in any order and report themselves unavailable or degraded
  until their dependencies recover, so Compose and systemd can gate
  dependents on readiness alone.
- Liveness never restarts a process for a dependency failure.
- Adding a protocol revision is a deliberate, testable change to the range a
  build speaks, and a retired revision is visible to peers before it breaks
  them.
- Health documents name internal dependencies, so operational listeners stay
  on internal addresses.
