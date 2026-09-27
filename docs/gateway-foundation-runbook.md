# Gateway foundation: startup and diagnosis

This page describes the current internal gateway process. It is an operator
reference for the foundation stage, not a claim that the public control plane
or a production proxy listener is ready.

## What starts today

`gatewayd` starts one plaintext, loopback-only gRPC management listener. Its
standard gRPC Health service and Gateway service share a durable engine backed
by the state directory. The binary does not bind the Axum REST router or a
Pingora traffic listener. The REST router is available through
`gatewayd::management_router_with_config` for authenticated future composition
and for tests. A Health `SERVING` response means the management engine can
accept mutations; it does not mean a website is receiving traffic.

The adapter currently compiles supported HTTP/HTTPS upstream peers and an
immutable Host/PathPrefix route decision table scoped to each site's declared
domains. Routes without a site domain, or with a Host matcher outside that
site's domains, fail validation. Other IR nodes, including
listeners and TLS profiles, fail capability validation before Prepare.
Lower route priority values win; equal priorities choose the longer path
prefix, then an explicit Host matcher, then the stable route ID.

## Local startup

```sh
PINGORA_PANEL_STATE_DIR=/var/lib/pingora-panel/gateway \
PINGORA_PANEL_GATEWAY_ADDR=127.0.0.1:50051 \
PINGORA_PANEL_WORKERS=4 \
cargo run --manifest-path panel/Cargo.toml --package gatewayd
```

`PINGORA_PANEL_GATEWAY_ADDR` must be a numeric IPv4 or IPv6 loopback address
until an authenticated transport policy is composed. `PINGORA_PANEL_WORKERS`
must be in `1..=256`. The state directory needs durable storage and exclusive
ownership by one gateway process. A second process using the same directory
fails to acquire the lease.

## Reading readiness and recovery

- Standard gRPC Health reports `SERVING` only while the engine is ready.
  Corrupt or incompatible startup state reports `NOT_SERVING` while Status
  remains available for diagnosis.
- On shutdown, Health switches to `NOT_SERVING`, new mutations stop, and
  admitted mutations drain before the process exits. The drain window is set
  with `PINGORA_PANEL_DRAIN_TIMEOUT_MS` and is at most 300 seconds.
- Status includes active revision and hash, prepared count, adapter and schema
  versions, worker count, uptime, recovery count, degraded transitions, and
  unknown commit outcomes. Compare the active hash with the caller's expected
  hash before attempting another activation.
- Structured logs use events such as `recovery_completed`,
  `gateway_degraded`, `prepared_cleanup_deferred`, and
  `gateway_recovery_summary`. Preserve the state directory for diagnosis;
  do not hand-edit an active or prepared record.
- If an activation returns `COMMIT_OUTCOME_UNKNOWN`, check Status and the
  activation receipt before retrying. The gateway may already have published
  the new active snapshot. A rejected CAS attempt is a confirmed conflict;
  fetch the current active hash before preparing a new attempt.

The default filesystem store writes and reads v1 records. A v2 codec and
golden fixture exist, but v2 records require an explicitly configured reader;
the current `gatewayd` does not select it. A new disk format requires its own
fixture and explicit migration path. The JSON REST
adapter accepts up to 4 MiB of raw HTTP body by default; the JSON compiler
accepts up to 2 MiB of normalized snapshot document. Both limits are
configurable at their respective composition boundaries.

For a repeatable local verification of process startup, health, SIGTERM, and
same-port restart, run:

```sh
cargo test --manifest-path panel/Cargo.toml --package gatewayd --test process_lifecycle --locked
```

The in-memory idempotency repository is for ephemeral composition and tests.
It has a finite capacity and retains uncertain claims; restarting it loses
receipts. A persistent repository remains necessary before public management
traffic is enabled.
