# Gateway: startup and diagnosis

This page describes the gateway process for operators: how it starts, what
its readiness means and how it recovers.

## What starts today

`gatewayd` starts a gRPC management listener, loopback-only unless it serves
mutual TLS, and the Pingora data plane. The standard gRPC Health service, the
Gateway service and the GatewayRuntime service share a durable engine backed
by the state directory. A Health `SERVING` response means the engine can
accept mutations; whether websites receive traffic depends on the listeners
of the active configuration, which the GatewayRuntime service and
`ppanel gateway status` report.

The data plane serves the active snapshot's listeners, virtual hosts,
routes, TLS profiles, static content and upstream pools, as described in
[the data plane decision](adr/0010-pingora-data-plane.md). Snapshots that
need anything the adapter does not declare, such as HTTP/3 listeners or Unix
socket upstreams, fail capability validation before Prepare, and listener
addresses another process holds fail Prepare with a diagnostic. Lower route
priority values win; equal priorities prefer exact paths, then globs,
regular expressions and prefixes, longer patterns, an explicit Host matcher
and finally the stable route ID.

## Local startup

```sh
PINGORA_PANEL_STATE_DIR=/var/lib/pingora-panel/gateway \
PINGORA_PANEL_GATEWAY_ADDR=127.0.0.1:50051 \
PINGORA_PANEL_WORKERS=4 \
cargo run --manifest-path panel/Cargo.toml --package gatewayd
```

`PINGORA_PANEL_GATEWAY_ADDR` must be a numeric IPv4 or IPv6 loopback address
unless `PINGORA_PANEL_TLS_DIR` provides mutual TLS credentials.
`PINGORA_PANEL_WORKERS` must be in `1..=256`; a worker count set at runtime
replaces it and persists in the state directory, as do node drains.
`PINGORA_PANEL_SECRET_DIR` holds the certificate and key files TLS profiles
name, and `PINGORA_PANEL_STATIC_ROOT` the directories static sites serve.
The state directory needs durable storage and exclusive ownership by one
gateway process. A second process using the same directory fails to acquire
the lease.

## Reading readiness and recovery

- Standard gRPC Health reports `SERVING` only while the engine is ready.
  Corrupt or incompatible startup state reports `NOT_SERVING` while Status
  remains available for diagnosis.
- On shutdown, Health switches to `NOT_SERVING`, new mutations stop, and
  admitted mutations drain before the process exits. The drain window is set
  with `PINGORA_PANEL_DRAIN_TIMEOUT_MS` and is at most 300 seconds; it also
  bounds how long a replaced data plane generation finishes in-flight
  requests.
- The data plane state lists the generation, workers and served listeners.
  When the active configuration's listeners cannot be served, for example
  because another process took a port, the state carries the error and the
  gateway retries until it can.
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
