# 0002: Management HTTP contract boundaries

Status: accepted for the internal development adapter.

## Context and decision

The HTTP layer must express the same gateway operations as the application
port, while the deployed `gatewayd` currently binds only its gRPC management
listener. The REST router is therefore assembled by `gatewayd` for internal
integration tests and future authenticated composition; it is not exposed by
the production process today.

- `panel-api` maps HTTP headers, JSON envelopes, application results and stable
  errors. It owns routing, request limits, Problem Details and the Utoipa schema.
- `panel-config-json` compiles versioned JSON snapshots into the engine-neutral
  IR. Unknown configuration fields and invalid domain values fail at this
  boundary. A future DSL compiler can implement the same `ConfigCompiler` port.
- `panel-application` owns use cases, request context and replaceable gateway
  ports. Neither Tonic nor Axum types enter its interfaces.
- `gatewayd` wires these parts to the durable engine and enforces deadlines
  before an operation is dispatched. The HTTP adapter still requires an
  authenticated binding policy before it can be exposed in production.
- `gateway-proto-codec` owns Proto/IR conversion once for both gRPC adapters.
  The client depends on this narrow crate rather than on the server adapter;
  the dependency policy rejects a direct client-to-server dependency.

The existing v1 HTTP envelopes continue to ignore additional fields so a
client carrying extension metadata can still use older servers. This is
separate from the versioned configuration inside `snapshot`: that canonical
IR rejects unknown fields and validates value objects through Serde. Its
schema version and persisted hash must not silently change meaning. Future
IR formats need an explicit new schema reader/migrator, rather than weakening
v1 parsing or imposing new requirements on the transport envelope.

The codec exposes only snapshot and hash conversion functions. Private
snapshot, routing, upstream, policy and hash modules contain the wire details;
both adapters use the same hash codec. Existing `gateway_grpc` functions stay
available as thin forwarding functions with their original signatures, so
moving implementation does not break downstream Rust imports. Local CI showed
that cargo-semver-checks 0.50.0 treats cross-crate function re-exports as missing
APIs; retaining concrete forwarding functions also keeps that guard effective.
Cross-adapter status tests live in `gatewayd`, where both adapters are composed,
instead of adding a server dependency back to the client crate.

`POST /api/v1/gateway/abort` consumes the same mutation metadata as prepare
and activate; a successfully removed prepared token returns `{"aborted":true}`.
Missing or already removed tokens return stable `NOT_FOUND`. The engine runs
admitted mutations independently of request cancellation, so callers can query
status after a timeout. The application-level idempotency repository currently
covers activation receipts; it does not promise replayable abort responses.

## Contract review

Endpoint annotations generate the OpenAPI document. Regenerate the fixture
after intentional schema changes with:

```sh
cargo run --manifest-path panel/Cargo.toml --package panel-api --example export_openapi --locked > panel/panel-api/tests/fixtures/openapi.json
```

The fixture equality test detects unreviewed regeneration. The cross-commit
oasdiff guard in [0003](0003-openapi-compatibility.md) compares that reviewed
fixture with the PR target or preceding push commit. Public breaking changes
require an explicit new API version rather than an accidental v1 edit.
