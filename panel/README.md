# Panel workspace architecture

`panel/` 使用 ports-and-adapters 边界。具体框架只允许出现在叶子适配器和最终组合根中；核心模型、用例编排和存储契约不依赖 Pingora、Tonic、SQL 或文件系统。

## Crate dependency direction

```text
panel-ir -> panel-domain
panel-engine -> panel-errors + panel-domain + panel-ir
panel-application -> panel-errors + panel-domain + panel-ir
panel-api -> panel-application + panel-errors
panel-config-json -> panel-application + panel-errors + panel-ir

panel-gateway-runtime -> panel-engine ports
snapshot-store-fs -> panel-engine::SnapshotStore
gateway-pingora -> panel-engine::DataPlaneAdapter
gateway-proto-codec -> panel-contracts + panel-domain + panel-ir
gateway-grpc -> gateway-proto-codec + panel-engine::GatewayEngine
gateway-grpc-client -> panel-application + gateway-proto-codec + panel-contracts

gatewayd -> runtime + filesystem adapter + Pingora adapter + gRPC/Proto adapters + REST/compiler adapters
```

箭头表示左侧 crate 依赖右侧 crate。

| Crate | Responsibility | Forbidden knowledge |
|---|---|---|
| `panel-errors` | Stable error codes and diagnostics | Domain, transport, storage, Pingora |
| `panel-domain` | Validated value objects | IR, transport, storage, Pingora |
| `panel-ir` | Versioned canonical runtime snapshot | Proto, storage, Pingora |
| `panel-engine` | `GatewayEngine`, `DataPlaneAdapter`, `SnapshotStore`, runtime-info ports and Fake | Proto, storage implementation, Pingora |
| `panel-application` | Request context, format-neutral config document, use-case orchestration and persistence ports | HTTP, Proto, storage implementation, Pingora |
| `panel-api` | Axum HTTP mapping, body limits, request-ID propagation, RFC 9457 Problem Details and OpenAPI projection | Pingora, storage, identity implementation, generated Proto, use-case orchestration |
| `panel-config-json` | JSON `ConfigCompiler` adapter with schema and document limits | HTTP, Proto, storage, Pingora, application orchestration |
| `panel-gateway-runtime` | Prepare/Activate/CAS/LKG orchestration | Tonic, filesystem, Pingora |
| `snapshot-store-fs` | Versioned JSON records, fsync and atomic rename | Tonic, Pingora, runtime policy |
| `gateway-pingora` | Compile IR into private Pingora values and atomic `ArcSwap` publication | Proto, filesystem, control-plane policy |
| `gateway-proto-codec` | Shared Proto/IR conversion used by client and server | Engine, server, client, Pingora, filesystem |
| `gateway-grpc` | Runtime-info projection, request policy and Tonic service | Pingora, filesystem, environment |
| `gateway-grpc-client` | Tonic client adapter implementing `panel-application::GatewayPort` | HTTP, storage, identity, generated Proto outside this adapter |
| `gatewayd` | Dependency construction, REST/gRPC adapter composition, bind/readiness policies, environment configuration, process clock, worker executor and standard gRPC Health | Business rules |

`.github/scripts/check-panel-boundaries.sh` enforces these direct dependency rules in CI.
`gatewayd::build_gateway_transport` is the single composition factory used by both the production process and TCP black-box tests, preventing test-only dependency graphs from drifting away from production.

## Activation invariant

All fallible work required to build and durably publish the activation occurs
before the data-plane pointer swap:

```text
IR validation -> adapter prepare -> persist prepared record
-> compare active hash -> fsync active snapshot + receipt -> ArcSwap publish -> ACK
```

If durable commit fails before the atomic rename, the active pointer is unchanged. If the rename succeeds but directory synchronization is inconclusive, the store returns a typed `COMMIT_OUTCOME_UNKNOWN`; the runtime still aligns the data plane and in-memory state with the visible record, marks itself degraded, and requires recovery before further mutations. Prepare, activate, and abort run in request-independent tasks, so client cancellation cannot cancel an admitted durable transaction. A bounded semaphore applies fail-fast backpressure to running and queued mutations, an async mutex serializes them, and a `TaskTracker` drains admitted work during shutdown. `PINGORA_PANEL_MAX_PENDING_MUTATIONS` configures this bound and is validated before Tokio resources are constructed. If the process stops after durable commit but before publication or ACK, `DurableGatewayEngine::restore` recompiles and republishes the committed LKG. Retrying the same prepare token returns the stored activation receipt. Corrupt startup state keeps Status available in `NotReady` mode while all mutations fail closed.

The application idempotency decorator claims a key atomically before invoking
the gateway. A completed claim replays the stored receipt; a different request
hash conflicts; an in-flight claim is retryable. If gateway activation succeeds
but receipt persistence fails, the claim is deliberately retained and the
caller receives `COMMIT_OUTCOME_UNKNOWN` so a retry cannot execute a second
mutation before reconciliation.

Activation errors release a claim only for the precommit rejection codes
documented by `GatewayPort`. Timeouts, resource/storage failures and unknown
codes retain the claim. Malformed gRPC success receipts also preserve commit
uncertainty. Automatic reconciliation remains separate from this protection.

The HTTP adapter keeps router construction, configuration, state, metadata,
error mapping, middleware and OpenAPI conventions in private modules behind
its public exports. Tower HTTP owns request-ID generation and propagation;
one renderer attaches the validated ID to Problem Details. Utoipa derives
mutation headers from the parsed metadata type and shares error statuses with
runtime mapping. The reviewed OpenAPI fixture and real HTTP publication tests
cover these contracts. See [the HTTP contract decision](../docs/adr/0002-management-http-contracts.md)
for module boundaries and the contract regeneration command.

## Compatibility fixtures and readiness

`snapshot-store-fs/tests/fixtures/v1` and `snapshot-store-fs/tests/fixtures/v2` are committed storage ABI fixtures. Tests must continue reading supported versions after implementation changes and must reject unknown format versions, truncated JSON, hash mismatches, and unsafe downgrades without rewriting the source record. A new storage format requires a new fixture directory and explicit migration path; existing fixtures are immutable.

`.github/scripts/check-panel-proto-breaking.sh` owns Protobuf compatibility enforcement. Pull requests compare against their target branch; default-branch pushes compare against the event's immutable `before` commit rather than the already-updated branch head. A missing predecessor module is treated only as the one-time bootstrap case; an invalid baseline fails closed. `resolve-panel-proto-baseline.sh` isolates event mapping, while `test-panel-proto-breaking.sh` uses a temporary Git repository and real Buf to verify bootstrap, additive evolution, deleted fields, changed types and reused field numbers.

The OpenAPI fixture is also compared across commits by `check-panel-openapi-breaking.sh` using pinned `oasdiff`. The guard allows additive optional fields and rejects endpoint, required-parameter, response, and schema breaks. Both guards keep transport-specific compatibility policy out of the application and engine crates.

`gatewayd` exposes the standard `grpc.health.v1.Health` service for both the overall server name (`""`) and the generated Gateway service name. Readiness comes from `GatewayEngine::status`: healthy or restored LKG state is `SERVING`; corrupt or incompatible startup state is `NOT_SERVING`. On shutdown, `ShutdownCoordinator` calls an abstract `ReadinessGate`, closes mutation admission atomically, waits the bounded drain window, and only then resolves Tonic's graceful-shutdown future. The custom Status RPC remains available for diagnostics and additively projects gateway/data-plane/adapter versions, process start time, monotonic uptime, configured worker count, completed recoveries, degraded transitions, and unknown commit outcomes through stable engine ports.

`GatewaydServices` retains its original public fields for source compatibility with existing integrations. New code should use `gateway()`, `health()`, and `health_reporter()`; these accessors are the supported extension boundary for future transports. A future major release can make the collection fully opaque without changing the transport composition model.

Until an authenticated transport is composed, `LoopbackOnlyManagementBindPolicy` rejects every non-loopback plaintext address. Bind validation is a policy port rather than an address-parser special case, so a future mTLS adapter can replace the policy explicitly. `GatewayWorkerCount` and `ShutdownPolicy` keep resource and drain limits valid before executor or server construction.

## Extension rules

1. Add a new engine without changing the runtime by implementing `DataPlaneAdapter` in a new leaf crate.
2. Add a new persistence backend by implementing `SnapshotStore`; backend format versions remain private to that adapter.
3. Add a new transport in its own crate and convert generated values only at that boundary.
4. Add process metadata through `GatewayRuntimeInfoProvider`; engines must not read clocks or environment variables.
5. Add authenticated management transports by implementing `ManagementBindPolicy`; never weaken the plaintext loopback default implicitly.
6. Add a health protocol by adapting `ReadinessGate`; shutdown sequencing must not depend on a concrete health implementation.
7. Extend Proto additively. Never expose generated Proto or Pingora structs from stable ports.
8. Evolve IR through a new schema version and explicit migrator; do not silently reinterpret persisted snapshots.
9. Keep `gatewayd` as a composition root. It may wire dependencies but must not acquire domain rules.
10. Prefer one canonical port or value type per concept. When moving implementations, keep thin re-exports at established public paths until an explicit breaking release; do not duplicate the implementation or make clients depend on server adapters.

## Revision lifecycle policy

`panel-config-domain` models one immutable revision attempt. `Failed`,
`Rejected`, and `Superseded` are terminal states by design: a transient gateway
or operator failure ends that revision attempt rather than reopening it. A retry
creates a new revision, preserving an auditable one-attempt-to-one-lifecycle
history. If a future product needs retries for the same revision identity, add a
separate attempt aggregate instead of introducing a `Failed -> Preparing`
transition to this state machine.
