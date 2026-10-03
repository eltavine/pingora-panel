# Panel workspace architecture

`panel/` 使用 ports-and-adapters 边界。具体框架只允许出现在叶子适配器和最终组合根中；核心模型、用例编排和存储契约不依赖 Pingora、Tonic、SQL 或文件系统。

## Crate dependency direction

```text
panel-context -> panel-errors
panel-events -> panel-context + panel-errors
panel-ir -> panel-domain
panel-engine -> panel-errors + panel-domain + panel-ir
panel-application -> panel-context + panel-errors + panel-domain + panel-ir
panel-config-model -> panel-domain + panel-errors + panel-ir
panel-api -> panel-application + panel-config-model + panel-errors
panel-config-json -> panel-application + panel-errors + panel-ir

panel-gateway-runtime -> panel-engine ports
snapshot-store-fs -> panel-engine::SnapshotStore
gateway-pingora -> panel-engine::DataPlaneAdapter
gateway-proto-codec -> panel-contracts + panel-domain + panel-ir
panel-event-codec -> panel-contracts + panel-events
panel-health (no workspace dependencies)
panel-jobs -> panel-context + panel-errors
panel-pki -> panel-context + panel-errors
panel-tls -> panel-pki + panel-context + panel-errors
panel-platform -> panel-context + panel-errors
panel-platform-codec -> panel-contracts + panel-platform
panel-service -> panel-health + panel-platform-codec + panel-contracts
panel-outbox -> panel-events + panel-errors
panel-postgres -> panel-outbox + panel-event-codec + panel-events + panel-health + panel-errors
panel-jetstream -> panel-event-codec + panel-events + panel-health + panel-errors
gateway-grpc -> gateway-proto-codec + panel-engine::GatewayEngine
gateway-grpc-client -> panel-application + gateway-proto-codec + panel-contracts + panel-health
config-proto-codec -> panel-application + gateway-proto-codec + panel-contracts
config-grpc-client -> panel-application + config-proto-codec + panel-service
panel-control-runtime -> panel-postgres + panel-jetstream + panel-outbox + panel-service + panel-tls

gatewayd -> runtime + filesystem adapter + Pingora adapter + gRPC/Proto adapters + REST/compiler adapters
config-service -> panel-control-runtime + gateway-grpc-client + panel-config-json + panel-config-model + config-proto-codec
panel-api-server -> panel-control-runtime + panel-api + config-grpc-client + gateway-grpc-client
panel-cli (no workspace dependencies: a client of the public REST API)
automation-service -> panel-control-runtime + panel-jobs + panel-postgres + panel-events
observability-service -> panel-control-runtime
panel-bootstrap -> panel-postgres + panel-jetstream + panel-pki
```

箭头表示左侧 crate 依赖右侧 crate。

| Crate | Responsibility | Forbidden knowledge |
|---|---|---|
| `panel-errors` | Stable error codes and diagnostics | Domain, transport, storage, Pingora |
| `panel-context` | Request scope, W3C Trace Context, correlation, idempotency and actor identifiers shared by requests, commands and events | Domain, transport, storage, Pingora |
| `panel-domain` | Validated value objects | IR, transport, storage, Pingora |
| `panel-events` | CloudEvents-aligned event model, publisher/handler ports and idempotent consumption | Event formats, brokers, storage, Pingora |
| `panel-event-codec` | CloudEvents Protobuf, JSON and binary-mode representations | Brokers, storage, application rules, Pingora |
| `panel-health` | Health checks, impact-based readiness aggregation, service mode and `application/health+json` documents | Transports, drivers, Pingora |
| `panel-platform` | Service descriptors, protocol revision ranges and negotiation, capability directory and registration ports | Transports, registries, Pingora |
| `panel-platform-codec` | Protobuf form of service descriptors | Registries, transports, Pingora |
| `panel-service` | Liveness/readiness endpoints, gRPC health and `ServiceInfo`, peer negotiation, trace metadata, settings, signals and logging shared by service processes | Storage, brokers, application rules, Pingora |
| `panel-control-runtime` | Composition of control-plane processes: lazy dependencies, migrations, registration, outbox relay leadership, health and graceful shutdown | Application rules, Pingora |
| `panel-jobs` | Durable job model, leasing worker, retry policy, RFC 5545 schedules and maintenance windows, and an in-memory store | Storage, transports, Pingora |
| `panel-pki` | Internal certificate authority, workload identities, credential files and renewal | Transports, storage, Pingora |
| `panel-tls` | TLS 1.3 mutual authentication with reloadable credentials, tonic server and client integration, and per-service peer authorization | Storage, application rules, Pingora |
| `panel-outbox` | Ordered at-least-once outbox relay over `OutboxSource`, `OutboxWakeup` and `EventPublisher` ports | Storage, brokers, Pingora |
| `panel-postgres` | Service schema ownership, SCRAM role bootstrap, per-schema migrations, the transactional outbox, the idempotent-consumer inbox and the database health check | Application rules, transports, Pingora |
| `panel-jetstream` | Stream provisioning, deduplicated CloudEvents publication, durable consumers, dead letters, targeted replay and the broker health check | Storage, application rules, Pingora |
| `panel-ir` | Versioned canonical runtime snapshot | Proto, storage, Pingora |
| `panel-engine` | `GatewayEngine`, `DataPlaneAdapter`, `SnapshotStore`, runtime-info ports and Fake | Proto, storage implementation, Pingora |
| `panel-application` | Request context, format-neutral config document, use-case orchestration and persistence ports | HTTP, Proto, storage implementation, Pingora |
| `panel-api` | Axum HTTP mapping, body limits, request-ID propagation, RFC 9457 Problem Details and OpenAPI projection | Pingora, storage, identity implementation, generated Proto, use-case orchestration |
| `panel-config-json` | JSON `ConfigCompiler` adapter with schema and document limits | HTTP, Proto, storage, Pingora, application orchestration |
| `panel-config-model` | The editable configuration document: listeners, TLS profiles, upstreams and sites with domains and routes; validation, queries, checked edits and compilation into the IR | HTTP, Proto, storage, Pingora |
| `panel-gateway-runtime` | Prepare/Activate/CAS/LKG orchestration | Tonic, filesystem, Pingora |
| `snapshot-store-fs` | Versioned JSON records, fsync and atomic rename | Tonic, Pingora, runtime policy |
| `gateway-pingora` | Compile IR into private Pingora values with atomic `ArcSwap` publication, and run the data plane: listener generations, virtual hosts, routes, TLS, static files and upstream pools with health | Proto, control-plane policy |
| `gateway-proto-codec` | Shared Proto/IR conversion used by client and server | Engine, server, client, Pingora, filesystem |
| `gateway-grpc` | Runtime-info projection, request policy and Tonic service | Pingora, filesystem, environment |
| `config-proto-codec` | Protobuf form of the configuration publication contract | Storage, transports, Pingora |
| `config-grpc-client` | `GatewayUseCases` over `config-service`'s publication API | Storage, Pingora |
| `gateway-grpc-client` | Tonic client adapter implementing `panel-application::GatewayPort`, and the gateway health check | HTTP, storage, identity, generated Proto outside this adapter |
| `config-service` | Publication and configuration APIs, the draft configuration, PostgreSQL activation receipts and the `config` schema | HTTP, Pingora |
| `panel-api-server` | The `panel-api` process: public REST and web console, degraded admission and the service directory | Storage implementation, Pingora |
| `automation-service` | PostgreSQL job store with outbox events, worker and scheduler in the `automation` schema | HTTP, Pingora |
| `observability-service` | Service process owning the `observability` schema | Pingora |
| `panel-bootstrap` | Idempotent provisioning of service roles, schemas, streams and the service registry; issuance and rotation of service credentials | Application rules, Pingora |
| `gatewayd` | Dependency construction, REST/gRPC adapter composition, bind/readiness policies, environment configuration, process clock, worker executor, the data plane, its runtime API and standard gRPC Health | Business rules |
| `panel-cli` | The `ppanel` command line over the public REST API | Server crates, storage, Pingora |

`.github/scripts/check-panel-boundaries.sh` enforces these direct dependency rules in CI.
`gatewayd::build_gateway_transport` is the single composition factory used by both the production process and TCP black-box tests, preventing test-only dependency graphs from drifting away from production.

`gatewayd` serves the listeners of the active configuration through Pingora
([decision](../docs/adr/0010-pingora-data-plane.md)) next to its gRPC
management transport. TLS profiles name certificate and key files in
`PINGORA_PANEL_SECRET_DIR`, static sites live below
`PINGORA_PANEL_STATIC_ROOT`, `PINGORA_PANEL_WORKERS` sets the initial worker
count and `PINGORA_PANEL_DRAIN_TIMEOUT_MS` bounds how long a replaced
generation finishes in-flight requests. Worker counts and node drains set at
runtime persist in the state directory and survive restarts. See the
[gateway foundation runbook](../docs/gateway-foundation-runbook.md) for startup,
readiness, recovery and current limits.

## Service processes

`panel-api`, `config-service`, `automation-service` and `observability-service`
are composed by `panel-control-runtime`
([decision](../docs/adr/0007-service-processes-health-and-discovery.md)). Each
binds an operational listener with `/livez` and `/readyz`
(`application/health+json`) and a gRPC listener with `grpc.health.v1.Health`
and `pingora.panel.platform.v1.ServiceInfo`, then migrates its schema, registers
in the service directory and relays its outbox in the background. Run a binary
with `healthcheck` to probe its own readiness, as container health checks do.

| Process | Schema | Operational | gRPC | Other |
|---|---|---|---|---|
| `panel-api` | `identity` | `127.0.0.1:9180` | `127.0.0.1:50060` | public HTTP `127.0.0.1:8080` |
| `config-service` | `config` | `127.0.0.1:9181` | `127.0.0.1:50061` | calls `gatewayd` at `127.0.0.1:50051` |
| `automation-service` | `automation` | `127.0.0.1:9182` | `127.0.0.1:50062` | |
| `observability-service` | `observability` | `127.0.0.1:9183` | `127.0.0.1:50063` | |

Every process reads `PINGORA_PANEL_DATABASE_URL` (its role, without password),
`PINGORA_PANEL_DATABASE_PASSWORD` or `PINGORA_PANEL_DATABASE_PASSWORD_FILE`,
`PINGORA_PANEL_NATS_URL`, and optionally `PINGORA_PANEL_OPS_ADDR`,
`PINGORA_PANEL_GRPC_ADDR` and `PINGORA_PANEL_HEALTH_INTERVAL_MS`.
`config-service` also reads `PINGORA_PANEL_GATEWAY_URL`; `panel-api` reads
`PINGORA_PANEL_HTTP_ADDR`, `PINGORA_PANEL_CONFIG_URL`,
`PINGORA_PANEL_GATEWAY_URL` for the gateway's runtime API and
`PINGORA_PANEL_WEB_ROOT`, the directory of the built console. Plaintext
listeners must stay on loopback until internal transports are authenticated.

Internal gRPC runs over mutual TLS once a service has credentials
([decision](../docs/adr/0009-internal-mutual-tls.md)): set
`PINGORA_PANEL_TLS_DIR` to the directory holding its `identity.pem` and
`trust.pem`, and optionally `PINGORA_PANEL_TRUST_DOMAIN`. Its gRPC listener then
requires client certificates from the installation's authority and may bind
beyond loopback; clients verify each peer's identity
`<service>.<trust domain>` whatever address they dial, and the gateway and
publication APIs admit only `config-service` and `panel-api` respectively.
`gatewayd` reads the same variables. Credentials reload without a restart.
`panel-bootstrap pki` keeps them current: it creates the authority in
`PINGORA_PANEL_PKI_DIR` on first run and issues to every
`service=directory` pair in `PINGORA_PANEL_PKI_CREDENTIALS` whose credentials
are missing or past two thirds of their lifetime
(`PINGORA_PANEL_CERTIFICATE_LIFETIME_MS`, 24 hours by default), once with
`--once` or continuously.

`panel-bootstrap` runs once per installation and on every upgrade or password
rotation. It connects with `PINGORA_PANEL_ADMIN_DATABASE_URL` (the database
owner) and creates the `panel_<schema>` roles with the passwords in
`PINGORA_PANEL_<SCHEMA>_DATABASE_PASSWORD` (or `_FILE`), then provisions the
event streams and the service registry.

`config-service` reconciles the gateway at startup and then every
`PINGORA_PANEL_RECONCILE_INTERVAL_MS` (30 s by default). It records the
document of every prepared deployment, each activation's intent before the
activation claims its idempotency key, and the newest activated
configuration as the desired one. Reconciliation re-issues activations that
claimed their key without recording a receipt, so the gateway either replays
the receipt of one that committed or runs it now, and an activation that can
no longer commit is released for its caller to retry. A gateway without an
active configuration receives the desired one, and a newer configuration
prepared here and confirmed by the gateway becomes the desired one. Any other
configuration is quarantined: publication answers `UNAVAILABLE` and
readiness reports the service degraded until an operator resolves it.

`automation-service` runs durable jobs
([decision](../docs/adr/0008-durable-jobs.md)): a worker leases jobs of the
kinds it has handlers for, renews the lease while a job runs, stops a job
cooperatively when it is cancelled, retries retryable failures with
exponential backoff and records progress; a scheduler enqueues one job per
occurrence of each RFC 5545 schedule, and jobs that require a maintenance
window wait until an occurrence of it is open. Every job change is published
as an `automation.job.<change>` CloudEvent through the outbox.

A service whose degrading dependency is down keeps serving reads and refuses
changes with `503 Service Unavailable`, `Retry-After` and a retryable
`UNAVAILABLE` problem; a failing required dependency makes it unavailable.
`GET /api/v1/platform/services` lists live instances with their versions,
protocol revisions and capabilities.

## Configuration, command line and gateway operations

Operators edit one draft configuration and apply it as a whole
([decision](../docs/adr/0011-configuration-model-and-apply.md)). Under
`/api/v1`, `sites` (with `domains` and `routes`), `upstreams` (with
`nodes`), `listeners` and `tls-profiles` are resources with entity tags:
replacing or deleting one requires `If-Match`, and every change requires an
`Idempotency-Key`. A change that would introduce a validation error is
refused with its diagnostics. `GET /api/v1/config/draft` reports the draft
version and whether the gateway runs it, `GET /api/v1/config/validation`
checks the draft or chosen sites, and `POST /api/v1/config/apply` compiles
the expected draft version and activates it with compare-and-swap.
`/api/v1/gateway/data-plane`, `/reload`, `/workers` and `/shutdown`, together
with `/api/v1/upstreams/health` and node `drain`, operate the running
gateway.

`ppanel` covers the same operations from a shell. It reads the API address
from `--api` or `PPANEL_API` and records `--actor` or `PPANEL_ACTOR` with each
change:

```sh
ppanel upstream create --name app --node 10.0.0.11:8080,weight=2
ppanel listener set http --address 0.0.0.0:80
ppanel site create --name shop --domain shop.example --proxy <upstream-id>
ppanel route add <site-id> --match exact:/healthz --respond 204
ppanel config apply
ppanel upstream health
```

`-o json` prints machine-readable output and `completion <shell>` prints a
completion script. Exit codes distinguish usage errors (2), missing resources
(3), conflicts and failed preconditions (4), rejected changes (5), an
unavailable service (6) and denied requests (7) from other failures (1).

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

## Request identity

Every request has one identity from the first surface to the last event.
The REST adapter accepts a valid `x-request-id` or generates one, echoes it on
every response and Problem Details body, and records it with `correlation_id`
and the W3C `trace_id` on the request span. `x-correlation-id` is optional and
defaults to the request ID. `traceparent` and `tracestate` follow the W3C Trace
Context receiver rules: an invalid or repeated `traceparent` is ignored rather
than rejected, and repeated `tracestate` fields are combined in order.

Queries carry this identity as a `RequestScope`; commands carry it in
`CommandContext`. The gRPC client maps both into `RequestContext` and sends the
trace as `traceparent`/`tracestate` metadata, so gateway request events and logs
report the caller's request, correlation and trace IDs. Requests without a
caller start their own correlation. `EventOrigin::scoped` and
`EventOrigin::caused_by` place the same identity in CloudEvents
`correlationid`, `causationid`, `traceparent` and `tracestate`.

## Conditional requests

Responses that clients poll or replay carry strong entity tags (RFC 9110
section 8.8.3) and `Cache-Control: no-cache`, so a client revalidates with
`If-None-Match` and receives `304 Not Modified` while nothing changed. An
activation may state the configuration it replaces as `If-Match` with the
active content hash instead of `expected_active_hash`; the precondition is
evaluated before the activation and fails with `412 Precondition Failed`, and
the gateway's compare-and-swap still refuses a change made since. The
response's `ETag` is the new active hash, ready for the next `If-Match`.

## Service-owned PostgreSQL schemas

Each service connects as a login role that owns exactly one schema. `DatabaseBootstrap`
applies PostgreSQL's secure schema usage pattern idempotently: it revokes `PUBLIC`
privileges on the database and on `public`, creates each role with a client-computed
SCRAM-SHA-256 verifier so the server never sees a plaintext password, makes the role
own its schema, and pins the role's `search_path` to that schema. Unqualified platform
SQL therefore resolves to the caller's own tables and cannot reach another service's
schema. The administrator, never a service role, must own the database.

`ServiceDatabase::migrate` applies platform migrations (versions below 10000) and the
service's own migrations (10000 and above) in one ordered history stored in the
service schema, so services migrate independently.

Producers call `PgOutbox::append` inside the transaction that changes their state, so an
event exists exactly when that change commits. Each row stores the CloudEvents Protobuf
form. A statement trigger issues `NOTIFY` (delivered only after commit) to wake the
relay, and polling bounds the delay when a notification is lost. One relay per schema
holds a PostgreSQL advisory lock and publishes in append order; a producer must lock the
aggregate before appending so append order matches the aggregate's commit order.

`panel-jetstream` publishes relayed events to NATS JetStream and drives durable consumers.
Events a consumer cannot process are parked in a dead-letter stream, either by the handler's
decision, after the final failed attempt, or from the max-deliveries advisory. Replay
returns them to that consumer alone. See
[the delivery decision](../docs/adr/0006-jetstream-delivery-and-dead-letters.md).

Integration tests use disposable servers named by `PANEL_TEST_DATABASE_URL` and
`PANEL_TEST_NATS_URL` and skip without them; CI sets `PANEL_REQUIRE_INTEGRATION_SERVICES` so a missing server fails
instead. Locally:

```sh
panel/scripts/dev-services.sh up
eval "$(panel/scripts/dev-services.sh env)"
cargo test --manifest-path panel/Cargo.toml --package panel-postgres --package panel-jetstream --all-features
panel/scripts/dev-services.sh down
```

## Compose installation

`deploy/compose.yaml` installs Pingora Panel on one Linux host with Docker
Compose or Podman Compose, from one image built by `deploy/Containerfile`
(distroless, non-root) and digest-pinned PostgreSQL and NATS images:

```bash
panel/deploy/generate-secrets.sh
docker compose -f panel/deploy/compose.yaml up -d
```

Every container uses the host network and binds loopback addresses, so the
console at <http://127.0.0.1:8080> and every internal port stay local until
the API authenticates callers. `pki-init` creates the certificate authority
and every service's credentials, `pki` renews them, and `bootstrap`
provisions roles, schemas, streams and the service registry before the
services start; internal gRPC runs over mutual TLS. Each service mounts only
its own credential volume, read-only, and runs with a read-only root file
system, no capabilities and `no-new-privileges`. Passwords are generated
into `deploy/secrets/` (never committed) and mounted as Compose secrets.
The `panel-deploy` workflow builds the image and checks the running
installation through its API.

## Web console

`panel/web` is the Vue console served by `panel-api`. It is generated from the official
`create-vue` and shadcn-vue tooling, renders feature modules that register their own
routes and navigation, and calls the API through a client generated from the reviewed
OpenAPI fixture. See [`web/README.md`](web/README.md) and
[the console decision](../docs/adr/0005-web-console-stack.md).

`panel-api` sends the security headers in
[`web/security-headers.json`](web/security-headers.json). The preview server used by
end-to-end tests sends the same headers, and the tests fail on any policy violation.
The only inline style the policy admits is the scrollbar rule reka-ui's select viewport
renders, by its SHA-256 hash; the end-to-end tests open a select, so a dependency update
that changes the rule fails them.

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
