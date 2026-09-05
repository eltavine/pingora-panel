# ADR 0001: Bound and supervise durable gateway mutations

- Status: Accepted
- Date: 2026-09-05

## Context

Gateway prepare, activate, and abort operations can outlive the initiating gRPC request. This is required because cancelling a client future must not cancel a storage transaction after it has been admitted. The previous executor serialized mutations and tracked their lifetime, but accepted an unbounded number of waiting tasks. Under overload, that converted transport pressure into process memory growth. Tokio's task tracker also allows tasks to be spawned after it is closed, so tracker closure alone is not an admission boundary.

The engine must remain independent of Tonic, filesystem storage, and process configuration. The composition root must validate resource limits before constructing asynchronous primitives. Status reads must remain independent of mutation storage I/O.

## Decision

`panel-gateway-runtime` owns one `GatewayMutationExecutor` composed from mature Tokio primitives:

1. A `Semaphore` bounds the total number of running and queued mutations and rejects excess work immediately with `RESOURCE_EXHAUSTED`.
2. An asynchronous `Mutex` serializes admitted durable transactions without blocking status reads.
3. A `TaskTracker` owns admitted task lifetimes and supports graceful drain.
4. A short synchronous lifecycle lock makes admission atomic with shutdown. Shutdown marks admission closed before closing the semaphore and task tracker, so no mutation can cross the close boundary.

Capacity is represented by the validated `GatewayMutationCapacity` value type. `gatewayd`, the composition root, reads `PINGORA_PANEL_MAX_PENDING_MUTATIONS`, validates it, and injects it into the runtime. Core engine ports remain unaware of environment variables and Tokio implementation details.

Recovery diagnostics use the canonical types declared by `panel-engine`. Because the project is still internal and has no released compatibility obligation, superseded duplicate runtime APIs are removed rather than retained as shims.

## Consequences

- Mutation memory pressure has an explicit, configurable upper bound.
- Overload and shutdown have stable, distinguishable error codes.
- Request cancellation cannot interrupt admitted durable work.
- Graceful shutdown drains exactly the work admitted before closure.
- Storage, transport, and adapter implementations remain replaceable through existing ports.
- Tests must cover saturation, close/admit ordering, cancellation on the real filesystem adapter, and unknown commit recovery after restart.
