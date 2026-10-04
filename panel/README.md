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
panel-metrics (no workspace dependencies)
panel-environment -> panel-errors
panel-jobs -> panel-context + panel-errors
panel-pki -> panel-context + panel-errors
panel-tls -> panel-pki + panel-context + panel-errors
panel-platform -> panel-context + panel-errors
panel-platform-codec -> panel-contracts + panel-platform
panel-service -> panel-environment + panel-health + panel-platform-codec + panel-contracts
panel-outbox -> panel-events + panel-errors
panel-postgres -> panel-outbox + panel-event-codec + panel-events + panel-health + panel-errors
panel-jetstream -> panel-event-codec + panel-events + panel-health + panel-errors
gateway-grpc -> gateway-proto-codec + panel-engine::GatewayEngine
gateway-grpc-client -> panel-application + gateway-proto-codec + panel-contracts + panel-health
config-proto-codec -> panel-application + gateway-proto-codec + panel-contracts
config-grpc-client -> panel-application + config-proto-codec + panel-service
observability-grpc-client -> panel-application + panel-contracts + panel-service
panel-control-runtime -> panel-postgres + panel-jetstream + panel-outbox + panel-service + panel-tls

gatewayd -> runtime + filesystem adapter + Pingora adapter + gRPC/Proto adapters + REST/compiler adapters
config-service -> panel-control-runtime + gateway-grpc-client + panel-config-json + panel-config-model + config-proto-codec
panel-api-server -> panel-control-runtime + panel-api + config-grpc-client + gateway-grpc-client
panel-cli (no workspace dependencies: a client of the public REST API)
automation-service -> panel-control-runtime + panel-jobs + panel-postgres + panel-events
observability-service -> panel-control-runtime + panel-contracts + panel-domain
audit-service -> panel-control-runtime + panel-jetstream + panel-events + panel-contracts
panel-bootstrap -> panel-postgres + panel-jetstream + panel-pki
```

箭头表示左侧 crate 依赖右侧 crate。

| Crate | Responsibility | Forbidden knowledge |
|---|---|---|
| `panel-errors` | Stable error codes and diagnostics | Domain, transport, storage, Pingora |
| `panel-context` | Request scope, W3C Trace Context, correlation, idempotency and actor identifiers shared by requests, commands and events | Domain, transport, storage, Pingora |
| `panel-domain` | Validated value objects | IR, transport, storage, Pingora |
| `panel-events` | CloudEvents-aligned event model, publisher/handler ports and idempotent consumption | Event formats, brokers, storage, Pingora |
| `panel-environment` | Process settings from the environment, and secrets from it or from files it names, through an injected lookup | Transports, storage, Pingora |
| `panel-event-contracts` | Event data generated from `proto/events`: one message per event type, written as proto3 JSON | Transports, storage, domain rules |
| `panel-event-codec` | CloudEvents Protobuf, JSON and binary-mode representations | Brokers, storage, application rules, Pingora |
| `panel-health` | Health checks, impact-based readiness aggregation, service mode and `application/health+json` documents | Transports, drivers, Pingora |
| `panel-metrics` | Prometheus metrics in the OpenMetrics text format: HTTP server and client metrics named by the OpenTelemetry semantic conventions, and scrape tokens | Transports, storage, Pingora |
| `panel-platform` | Service descriptors, protocol revision ranges and negotiation, capability directory and registration ports | Transports, registries, Pingora |
| `panel-platform-codec` | Protobuf form of service descriptors | Registries, transports, Pingora |
| `panel-service` | Liveness/readiness endpoints, gRPC health and `ServiceInfo`, peer negotiation, trace metadata, settings, signals and logging shared by service processes | Storage, brokers, application rules, Pingora |
| `panel-control-runtime` | Composition of control-plane processes: lazy dependencies, migrations, registration, outbox relay leadership, health and graceful shutdown | Application rules, Pingora |
| `panel-schedule` | RFC 5545 recurrences and the time windows they open, shared by schedules, maintenance windows, approval policies and grants | Storage, transports, Pingora |
| `panel-jobs` | Durable job model, leasing worker, retry policy, schedules and maintenance windows, and an in-memory store | Storage, transports, Pingora |
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
| `observability-service` | Traffic summaries and series from Prometheus over the gateway's metrics, and the `observability` schema | Pingora |
| `observability-grpc-client` | `TrafficPort` over `observability-service` | Storage, Pingora, metric backends |
| `audit-service` | The audit trail: every event appended once to a hash chain in the `audit` schema, with queries and verification | HTTP, Pingora |
| `panel-bootstrap` | Idempotent provisioning of service roles, schemas, streams and the service registry; issuance and rotation of service credentials | Application rules, Pingora |
| `gatewayd` | Dependency construction, REST/gRPC adapter composition, bind/readiness policies, environment configuration, process clock, worker executor, the data plane, its runtime API and standard gRPC Health | Business rules |
| `panel-cli` | The `ppanel` command line over the public REST API | Server crates, storage, Pingora |

`.github/scripts/check-panel-boundaries.sh` enforces these direct dependency rules in CI.
`gatewayd::build_gateway_transport` is the single composition factory used by both the production process and TCP black-box tests, preventing test-only dependency graphs from drifting away from production.

`gatewayd` serves the listeners of the active configuration through Pingora
([decision](../docs/adr/0010-pingora-data-plane.md)) next to its gRPC
management transport. TLS profiles name certificate and key files in
`PINGORA_PANEL_SECRET_DIR`, static sites live below
`PINGORA_PANEL_STATIC_ROOT` — a site's root must resolve to a directory
inside it, request paths are decoded and their dot segments resolved before
they are appended, and symbolic links are followed only when they lead to a
file inside the site's root; others are not served —
`PINGORA_PANEL_WORKERS` sets the initial worker
count and `PINGORA_PANEL_DRAIN_TIMEOUT_MS` bounds how long a replaced
generation finishes in-flight requests. Worker counts and node drains set at
runtime persist in the state directory and survive restarts. Its
operational listener answers `/livez` (and `/healthz`) and `/readyz`, which
follows the gateway's gRPC health, and Prometheus scrapes the gateway's
metrics
([decision](../docs/adr/0022-metrics-logs-and-traces.md)) from `/metrics` on
`PINGORA_PANEL_OPS_ADDR`, `127.0.0.1:9185` by default; off loopback, scrapes
must present `PINGORA_PANEL_METRICS_TOKEN` (or the file
`PINGORA_PANEL_METRICS_TOKEN_FILE` names) as a bearer token. With
`PINGORA_PANEL_LOG_DIR` it writes access and error logs
([decision](../docs/adr/0025-access-and-error-logs.md)) under that directory:
`sites/<site>.access.log`, `access.log` for requests no site took and
`error.log`. The Compose installation's `otel-collector` ships them to
`loki` with `deploy/otel-collector.yaml` and `deploy/loki.yaml`. See the
[gateway foundation runbook](../docs/gateway-foundation-runbook.md) for startup,
readiness, recovery and current limits.

## Service processes

`panel-api`, `config-service`, `automation-service`, `observability-service`
and `audit-service` are composed by `panel-control-runtime`
([decision](../docs/adr/0007-service-processes-health-and-discovery.md)). Each
binds an operational listener with `/livez` and `/readyz`
(`application/health+json`) and `/metrics`, where `panel-api` also counts
its requests by route template, and a gRPC listener with `grpc.health.v1.Health`
and `pingora.panel.platform.v1.ServiceInfo`, then migrates its schema, registers
in the service directory and relays its outbox in the background. Run a binary
with `healthcheck` to probe its own readiness, as container health checks do.

| Process | Schema | Operational | gRPC | Other |
|---|---|---|---|---|
| `panel-api` | `identity` | `127.0.0.1:9180` | `127.0.0.1:50060` | public HTTP `127.0.0.1:8080` |
| `config-service` | `config` | `127.0.0.1:9181` | `127.0.0.1:50061` | calls `gatewayd` at `127.0.0.1:50051` |
| `automation-service` | `automation` | `127.0.0.1:9182` | `127.0.0.1:50062` | |
| `observability-service` | `observability` | `127.0.0.1:9183` | `127.0.0.1:50063` | queries Prometheus at `127.0.0.1:9090` |
| `audit-service` | `audit` | `127.0.0.1:9184` | `127.0.0.1:50064` | consumes every event |

Every process reads `PINGORA_PANEL_DATABASE_URL` (its role, without password),
`PINGORA_PANEL_DATABASE_PASSWORD` or `PINGORA_PANEL_DATABASE_PASSWORD_FILE`,
`PINGORA_PANEL_NATS_URL`, and optionally `PINGORA_PANEL_OPS_ADDR`,
`PINGORA_PANEL_GRPC_ADDR` and `PINGORA_PANEL_HEALTH_INTERVAL_MS`.
`config-service` also reads `PINGORA_PANEL_GATEWAY_URL`;
`observability-service` reads `PINGORA_PANEL_PROMETHEUS_URL`; `panel-api` reads
`PINGORA_PANEL_HTTP_ADDR`, `PINGORA_PANEL_CONFIG_URL`,
`PINGORA_PANEL_GATEWAY_URL` for the gateway's runtime API,
`PINGORA_PANEL_OBSERVABILITY_URL` for its traffic,
`PINGORA_PANEL_WEB_ROOT`, the directory of the built console, and the
identity settings described under [Accounts and access](#accounts-and-access).
Plaintext listeners must stay on loopback until internal transports are
authenticated. `PINGORA_PANEL_HTTP_ADDR` may instead name a Unix domain
socket, `unix:/run/pingora-panel/api.sock`, for the reverse proxy in front
of the console: the socket is made readable and writable by its owner and
group only, replaces a stale socket but never another file, and is removed
at shutdown. As on loopback, the API takes the client address from the last
`X-Forwarded-For` entry the proxy appends.

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
from `--api` or `PPANEL_API` and authenticates with the session
`ppanel login` keeps, or with an API token from `--token` or `PPANEL_TOKEN`:

```sh
ppanel login --username admin
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


## Configuration language and revisions

The draft is also a set of files in the configuration language
([decision](../docs/adr/0012-configuration-language-and-revisions.md)), with
`main.conf` as the entry. `GET /api/v1/config/source` returns them with the
draft version and entity tag, and `PUT` replaces them once they check
cleanly; changes through the resource endpoints rewrite only the blocks they
touch, so comments and layout survive. `POST /api/v1/config/check`,
`/format` and `/ast` work on files without saving them, and diagnostics
carry `file:line.column` positions. `GET /api/v1/config/schema` describes
every directive for editors, `GET /api/v1/config/plan` lists the resources
and file lines the draft changes relative to the active revision,
`GET /api/v1/config/ir` returns the runtime snapshot it compiles to, and
`POST /api/v1/config/dry-run` prepares that snapshot on the gateway without
activating it. Values are inherited explicitly — a route takes its server's
listeners and redirects, a host's certificate falls back from its domain to
its server to the listener — and the schema states each rule.
`POST /api/v1/config/explain` lists, for the block at a position, every
value that applies there and whether it is written in the block, inherited
from where, or a default; settings that have no effect where they are
written are warnings. Redirect targets and response bodies are templates
the gateway fills in per request: `$host`, `$uri`, `$method`, `$scheme`,
`$client_ip`, `$request_id`, `$upstream_addr`, `$http_<name>` and
`$cookie_<name>`, with `$$` for a literal dollar; gateways that cannot
evaluate them refuse the snapshot. `POST /api/v1/config/import/nginx`
converts the documented NGINX subset — `server`, `listen`, `server_name`,
`location`, `proxy_pass`, `root`, `index`, `try_files`, `return` and
`upstream` — and reports every directive it did not carry over at its
position. Every apply records a revision with its files, author, note and
outcome under `/api/v1/revisions`; a revision can be compared with another,
the active one or the draft, annotated, and restored into the draft.

```sh
ppanel config export --dir conf
ppanel config check conf
ppanel config fmt conf --write
ppanel config explain sites/shop.conf:12 conf
ppanel config import conf --expected-version 3
ppanel config plan
ppanel config apply --dry-run
ppanel config apply --note "launch the shop"
ppanel revision list
ppanel revision diff 4 --against active
ppanel config rollback --to 3 --reason "errors after launch"
ppanel config import-nginx /etc/nginx/nginx.conf --dir conf
```

The console edits the same files with highlighting, completion for the
block being edited, problems checked as the text changes, an outline and
the effective values at the cursor, imports NGINX configuration for review,
and shows revisions with their differences, notes and rollback.

## Approvals

Approval policies decide which changes need someone other than the person
applying them to approve first
([decision](../docs/adr/0019-change-approvals.md)). A policy covers changes
to given resource kinds, to sites with given tags, of at least a given risk,
or applied in time windows, and asks for one to five approvals that
stay valid for a set time. A change is high-risk when it removes anything or
touches listeners, TLS profiles or security policies. Applying a covered
draft answers `202` with an approval request instead of publishing; the
request pins the draft's content and the policies' versions, and applying
again goes ahead once enough other people approved. Nobody approves their
own request, approvals can be revoked until the change is applied, the
requester can withdraw the request, and a different draft or an edited
policy needs a new request. In an emergency, Administrators can apply
without approvals by giving a reason and an incident, which is recorded as
`config.approval.bypassed` before anything is published. A time window is an
RFC 5545 recurrence, with a time zone or in UTC, and a length in minutes, so
it keeps local hours across daylight-saving changes and may cross midnight;
the console and `ppanel` write weekly ones from days and hours such as
`mon-fri 09:00-18:00 Europe/Berlin`.
`/api/v1/approval-policies` needs `approval.manage` to change policies, and
`/api/v1/approvals` needs `approval.decide` to approve, reject or revoke.

```sh
ppanel approval-policy set prod --site-tag prod --min-risk high \
  --window "mon-fri 09:00-18:00 Europe/Berlin" --approvals 1
ppanel config apply --note "raise the shop limits"
ppanel approval list
ppanel approval approve <request-id>
ppanel config apply --bypass-reason "checkout is down for everyone" --incident INC-7
```

The console's Approvals page lists requests with their changes and lets
people decide on them, manages policies, and shows recent emergency
bypasses.

## Audit trail

Every change, and every refused or failed attempt, is recorded once by
`audit-service` ([decision](../docs/adr/0013-audit-trail.md)). Services
publish them as events through their outbox: `config-service` for draft
changes, refusals, dry runs, applies, revision notes and snapshot
publication, and `panel-api` for gateway reloads, worker changes,
shutdowns, drains and restores. Each record keeps the actor, request,
correlation, idempotency key and trace context of its event, and records
form a SHA-256 hash chain that a trigger keeps append-only.

`GET /api/v1/audit-events` lists records newest first, filtered by `actor`,
`type` (an event type or a prefix such as `config.`), `subject`,
`correlation_id`, `since` and `until`; `/api/v1/audit-events/{sequence}`
returns one and `/api/v1/audit-events/verify` recomputes the chain.

```sh
ppanel audit list --type config. --since 2026-10-01T00:00:00Z
ppanel audit list --correlation-id <request-id>
ppanel audit show 42
ppanel audit verify
```

The console's audit log filters the same records, follows one request's
events and verifies the chain.

## Traffic

The gateway measures every request it serves and sends upstream, its
connections and its TLS handshakes, with the metric names of the
OpenTelemetry semantic conventions
([decision](../docs/adr/0022-metrics-logs-and-traces.md)). Prometheus
scrapes them, and `observability-service` answers for them with fixed
queries, so callers never write PromQL. `GET /api/v1/traffic` summarizes a
window — requests, rate, status classes, latency percentiles, traffic in
and out, open connections, handshakes, upstream error ratios, failed
upstream attempts by node and why, the busiest routes and the busiest
domains as configured, such as `*.shop.example` — for every site, one
`site` or one `route`, and
`/api/v1/traffic/series` charts the request rate, server errors and 95th
percentile latency. Both need the gateway read permission.

```sh
ppanel traffic summary --window 1h
ppanel traffic summary --site shop --route checkout --window 15m
ppanel traffic series --window 1d --step 15m
```

The console's traffic page shows the same figures for a chosen window and
site, charts the request rate, server errors and latency, and reads them
again every 30 seconds.

## Logs

`observability-service` reads the gateway's access and error logs from Loki
([decision](../docs/adr/0026-log-search-tail-and-deletion.md)) with typed
filters — kind, site, route, status or status class, client address or
CIDR block, path prefix, request ID and text — that it turns into LogQL
itself, so callers never write LogQL. `GET /api/v1/logs` returns matching
records newest first, up to 500 a page; `/api/v1/logs/download` streams the
original lines of up to 100,000 of them as a file; `/api/v1/logs/tail`
follows them over a WebSocket, and a tail that falls behind ends with a
cursor to resume from. `POST /api/v1/logs/deletions` asks Loki to delete
one site's or every site's records up to now; Loki applies the deletion
once it can no longer be cancelled, and the audit trail records who asked.
Reading needs `logs.read` and deleting `logs.delete`.

```sh
ppanel logs search --site shop --status 5xx --since 1h
ppanel logs tail --kind error
ppanel logs download --request-id 01J9Z8 --file request.log
ppanel logs delete --site shop --yes
```

## Accounts and access

Every request to `panel-api` is authenticated and authorized
([decision](../docs/adr/0014-identity-and-access.md)). While no account
exists, `POST /api/v1/setup` creates the first Administrator with the
one-time bootstrap token from `PINGORA_PANEL_BOOTSTRAP_TOKEN` (or `_FILE`);
the Compose installation generates it in `secrets/bootstrap-token`.
Passwords have at least 15 characters, are refused when common, predictable
or built from the account's names, and are stored as Argon2id hashes keyed
with the pepper in `PINGORA_PANEL_PASSWORD_PEPPER` (or `_FILE`). Failed
logins make the account wait, doubling from 30 seconds after the fifth to an
hour, and the hundredth locks the password until an Administrator unlocks
it; logins are also limited per client address.

`POST /api/v1/session` logs in. Browsers receive a `__Host-ppanel_session`
cookie that is `Secure`, `HttpOnly` and `SameSite=Strict`, and send the
session's CSRF token in `x-csrf-token` with every unsafe request; requests
from another site are refused. The command line asks for a bearer session
instead. Sessions end after an hour without activity or a day after login
(`PINGORA_PANEL_SESSION_IDLE_MS`, `PINGORA_PANEL_SESSION_LIFETIME_MS`), on
logout, and when the password changes or the account is disabled. When the
console is reached through a proxy, `PINGORA_PANEL_PUBLIC_ORIGINS` lists its
origins for browsers that send no `Sec-Fetch-Site`.

Each route requires a permission from the catalog at
`GET /api/v1/permissions`. The built-in roles grant Administrator every
permission, Operator configuration changes, decisions on others' changes and
gateway operations, Viewer
reading, and Auditor reading with the audit trail and accounts; they cannot
change. Administrators compose further roles from the catalog and delete
those no account holds. `/api/v1/account` manages the caller's password,
sessions and API tokens; tokens start with `ppat_`, are shown once, expire
within a year and never hold permissions their owner lacks. Rotating a token
issues a new secret with the same permissions and lifetime and stops the old
one at once, and a user can end every other session of their account.
`/api/v1/accounts` and `/api/v1/roles` administer accounts, their roles,
sessions and tokens, and custom roles. Logins, failed logins, account, role
and token changes, and requests refused to a signed-in caller are recorded
in the audit trail.

```sh
ppanel setup --username admin --bootstrap-token-file secrets/bootstrap-token
ppanel login --username admin
ppanel whoami
ppanel account create ops --role operator --with-password
ppanel token create ci --permission config.read --permission config.apply --days 30
PPANEL_TOKEN=ppat_... ppanel config apply
ppanel token rotate <token-id>
ppanel role create deployer --name Deployer --permission config.read --permission config.apply
ppanel account end-sessions ops
ppanel logout --everywhere
```

The console asks for a login, or for the first administrator while none
exists, shows only the pages the account may use, and offers account
settings and the administration of accounts and roles.

People can also sign in through OpenID Connect identity providers
([decision](../docs/adr/0018-identity-provider-sign-in.md)).
`/api/v1/identity-providers` keeps each provider's issuer, client, scopes,
the claims to read and the roles its groups receive; client secrets are
sealed with the master keys in `PINGORA_PANEL_MASTER_KEYS` (or `_FILE`) and
never returned, and an enabled provider is checked against its discovery
document before it is saved. The sign-in page lists the enabled providers;
`/api/v1/auth/oidc/{id}/start` sends the browser to one with the
authorization code flow, PKCE, a nonce and a state bound to the browser by a
short-lived cookie, and the callback at
`{first public origin}/api/v1/auth/oidc/{id}/callback` verifies the ID token
(issuer, audience, signature, lifetime and nonce) before it opens a session.
People are linked to accounts by provider and subject; when the provider
allows it, an unknown person gets an account named after the username claim,
and a name already taken is refused. Roles from group mappings are
recalculated at every sign-in, while roles granted by hand stay. Every
fifteen minutes the provider is asked about each session through its
refresh token, and sessions it refuses end; disabling or deleting a provider
ends its sessions at once. Refused sign-ins are recorded as failed logins
with the provider and the reason.

Besides the roles an account holds everywhere, account managers can grant it
a role for one site group (the sites' `group`), for one site, or for
everything, counting only before an expiry, from given client networks or
within time windows
([decision](../docs/adr/0021-scoped-and-conditional-grants.md)). The API
evaluates grants on every request; a configuration permission held only for
some sites lets the request through with a site scope that the
configuration service enforces: lists and summaries show only those sites,
changes must stay within them, a draft applies only when every change in its
plan is one of them, and shared resources can be read but not changed.
`/api/v1/accounts/{id}/grants` lists, creates and deletes grants.

```sh
ppanel account grant ops --role operator --site-group shop \
  --network 10.0.0.0/8 --window "mon-fri 09:00-18:00 Asia/Shanghai" --until 2026-12-31T00:00:00Z
ppanel account grants ops
```

Programs get service accounts
([decision](../docs/adr/0020-service-accounts-and-workload-identity.md)):
they have no password, never sign in and cannot be break-glass accounts, and
account managers issue their API tokens through
`POST /api/v1/accounts/{id}/tokens`, never with permissions the service
account lacks. Instead of a stored token, a CI job can prove what it is with
the short-lived OpenID Connect token its CI system issues: a workload
identity at `/api/v1/workload-identities` trusts an issuer's tokens for a
service account when they name an audience, match a subject exactly or by a
prefix ending in `*`, and carry given claims, and
`POST /api/v1/auth/workload` exchanges such a token, verified against the
issuer's published keys, for a bearer session of five to sixty minutes.
Exchanges and refusals are recorded like logins.

```sh
ppanel account create deployer --service --role operator
ppanel workload-identity set shop --account deployer \
  --issuer https://token.actions.githubusercontent.com --audience pingora-panel \
  --subject "repo:shop/site:ref:refs/heads/main"
# in the job, with its ID token for the audience pingora-panel in $ID_TOKEN:
export PPANEL_TOKEN=$(printf %s "$ID_TOKEN" | ppanel workload-identity exchange --token-file -)
ppanel config apply
```

Accounts can be marked as break-glass, and `/api/v1/sign-in-policy` can then
limit password sign-in to them so that everyone else goes through a provider.
The limit needs an enabled provider and an enabled break-glass account that
can manage accounts, and while it holds that account cannot lose either.
Every sign-in with a break-glass account is recorded as
`identity.break_glass.used` alongside the login, and the console's Sign-in
providers page lists the recent ones.

```sh
ppanel identity-provider set corp --name "Corporate SSO" \
  --issuer https://id.example.com/realms/main --client-id panel \
  --client-secret-file corp-client-secret --group-role ops=operator --create-accounts
ppanel identity-provider list
ppanel account update admin --break-glass
ppanel sign-in-policy set --password break-glass-only
```

## Certificates

`automation-service` keeps the certificate inventory
([decision](../docs/adr/0015-certificates-and-secret-material.md)). A
certificate is uploaded as a PEM chain, leaf first, with the leaf's
unencrypted private key — PKCS#8, PKCS#1 or SEC1; RSA of at least 2048 bits,
ECDSA P-256 or P-384, or Ed25519 — or generated with a new ECDSA P-256 key
and signed by it. Uploads are checked the way the gateway loads them: the key
must belong to the leaf, each certificate must be issued by the next, and
expired certificates are refused. Private keys are sealed with AES-256-GCM
under the master keys in `PINGORA_PANEL_MASTER_KEYS` (or `_FILE`), one
base64-encoded 256-bit key per line. The first seals new keys and the others
still open older ones, which are sealed again with the first at start, so a
master key is retired by adding a new first line, restarting, and then
removing the old line. Without master keys, certificates cannot be stored.

Every certificate is written to the gateway's secret directory,
`PINGORA_PANEL_GATEWAY_SECRET_DIR`, as `cert-<id>.pem` and `cert-<id>.key`,
readable only by their owner, and the directory is compared with the
inventory every minute. A TLS profile names one with `certificate_id` —
`certificate_id <id>;` in the configuration language — instead of naming
files placed by hand. Replacing a certificate, for example with a renewed
one, changes what the gateway serves within seconds without applying a new
revision; files that do not load leave the previous certificate in place.

`/api/v1/certificates` lists, uploads, generates, replaces and deletes
certificates and reports where each stands: `valid`, `expiring` within 30
days, `expired` or `not_yet_valid`. `/api/v1/certificates/{id}/coverage`
checks which hosts one covers by its subject alternative names, as RFC 9525
matches them, and `/api/v1/certificate-inspections` checks a chain and key
without storing them. Private keys are never returned. Reading needs
`certificate.read`, which every built-in role grants; changing needs
`certificate.manage`, which Operators and Administrators hold. Every change
and every refused change is recorded in the audit trail.

```sh
ppanel certificate generate intranet --name intranet.example --name '*.intranet.example'
ppanel certificate upload example.com --chain fullchain.pem --key privkey.pem
ppanel certificate check example.com --host www.example.com
ppanel tls-profile set edge --certificate-id example.com
ppanel certificate replace example.com --chain renewed.pem --key renewed.key
```

The console's Certificates page offers the same, with fingerprints, the
chain and a host check, and the TLS profile form picks a certificate of the
inventory.

A TLS profile also decides what its listeners negotiate. `min_protocol` and
`max_protocol` bound the TLS versions, `ciphers` picks cipher suites by their
IANA names — at least one for each version allowed — and `alpn` lists the
application protocols offered. Session resumption, through a session cache
and TLS 1.3 tickets, stays on unless `session_resumption off;`, and
`ocsp_stapling` is accepted but has no effect yet. A site's
`hsts max_age=365d include_subdomains preload;` sends Strict-Transport-Security
on HTTPS responses only; `preload` requires `include_subdomains` and a
`max_age` of at least a year, as browsers' preload lists do.

`/api/v1/gateway/file-checks`, `ppanel gateway files` and the console's
gateway overview report whether each private key the active TLS profiles
name may be read only by its owner, with its permission bits, and whether
each static root resolves inside the static content root, listing links
below a root that lead out of it — those are not served. The check looks at
up to 10,000 entries per root and needs `gateway.read`.

`/api/v1/tls-checks`, `ppanel listener check` and the console's listener page
connect to a configured TLS listener as a client and report the negotiated
version, cipher suite and ALPN, which versions it accepts on their own, the
certificate presented and whether it covers the host, and the
Strict-Transport-Security header. Checking needs `gateway.read`.

```sh
ppanel tls-profile set edge --certificate-id example.com --min-protocol TLSv1.2 \
  --cipher TLS13_AES_128_GCM_SHA256 --cipher TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256
ppanel site create --name shop --domain shop.example --proxy <upstream-id> \
  --tls-profile edge --hsts-max-age 31536000 --hsts-include-subdomains
ppanel listener check https --host shop.example
```

### Automatic certificates

Certificates can also come from an ACME CA
([decision](../docs/adr/0016-acme-issuance-and-renewal.md)). An ACME account
is registered with one directory — Let's Encrypt, ZeroSSL, Google Trust
Services or any other, with PEM roots for a private CA — after agreeing to
the CA's terms, and with an external account binding where the CA requires
one; the binding's MAC key is used once and not kept. The account key is
sealed with the master keys like certificate keys.

An automatic certificate names an account, its names and an inventory ID. A
job issues it into the inventory under that ID, replacing a certificate
already there, and renews it in place, so TLS profiles that name the ID
never change. HTTP-01 key authorizations are written to `acme-challenge/` in
the gateway's secret directory, and every listener answers
`/.well-known/acme-challenge/<token>` from there before routing, so each name
must reach a listener on port 80. Renewal happens when a third of the
validity remains or, when the CA publishes renewal information (RFC 9773),
at a random moment inside its suggested window. A failed attempt keeps the
CA's reason, is published as `tls.acme.certificate.failed` and is retried
after an hour, doubling up to a day. The hourly renewal check also publishes
`tls.certificate.expiring` when any certificate comes within 30, 14, 7, 3
and 1 days of its end and once it has expired.

Wildcard names, and names no gateway answers for on port 80, use DNS-01: a
DNS provider publishes `_acme-challenge` TXT records and the order waits its
propagation time before the CA looks. A provider sends RFC 2136 dynamic
updates to the zones' primary, signed with a TSIG key (HMAC-SHA256 or
HMAC-SHA512) whose base64 secret is sealed like keys, so BIND, Knot DNS and
PowerDNS work as they are. For BIND, a key and an `update-policy` such as
`grant acme-update. zonesub TXT;` limit the key to TXT records.

`/api/v1/acme-accounts`, `/api/v1/dns-providers` and
`/api/v1/acme-certificates`, with `/api/v1/acme-certificates/{id}/renewals`
to renew at once, need `certificate.read` to read and `certificate.manage`
to change, as does the console's Certificates page, which shows each
automatic certificate's state and last failure.

```sh
ppanel acme account register letsencrypt --directory letsencrypt \
  --email ops@example.com --agree-tos
ppanel acme certificate request example.com --account letsencrypt \
  --name example.com --name www.example.com
ppanel acme dns-provider add primary-ns --server ns1.example.com:53 \
  --zone example.com --key-name acme-update --secret-file acme-update.key
ppanel acme certificate request wildcard.example.com --account letsencrypt \
  --name '*.example.com' --challenge dns-01 --dns-provider primary-ns
ppanel tls-profile set edge --certificate-id example.com
```

## Security policies

Security policies restrict requests before sites and routes act on them
([decision](../docs/adr/0017-request-security-policies.md)). A policy is a
named resource of the configuration. A site's policy covers all of its
requests and a route's policy is checked after the site's, so a login route
can add a stricter rate limit to what the whole site allows.

```nginx
http {
    tls_profile edge {
        certificate_id example.com;
    }

    upstream app {
        server 10.0.0.11:8080;
    }

    security_policy office {
        allow 10.0.0.0/8 2001:db8::/32;          # other clients get 403
        deny 10.9.0.0/16;                          # wins over allow
        methods GET POST;                          # 405 with Allow; HEAD goes with GET
        deny_paths /.git /admin/internal;          # 403
        deny_user_agents "^sqlmap" "^curl/";       # case-insensitive, 403
        referers none *.example.com;               # hotlink protection, 403
        basic_auth staff.htpasswd realm=Staff;     # 401
        max_header_size 16k;                       # 431
        max_body_size 10m;                         # 413
        body_timeout 30s;                          # 408 when a body stalls
        rate_limit 10r/s burst=20;                 # per client address, 429
        rate_limit 300r/m key=$http_x_api_key;
        max_concurrent 20;                         # requests in progress per client
        limited_response 503 "body=Slow down" type=text/plain;
    }

    security_policy login {
        rate_limit 5r/m;
    }

    listener public {
        address 0.0.0.0:443;
        tls_profile edge;
        trusted_proxies 192.0.2.0/24;              # the load balancer
        real_ip_header x-forwarded-for;
        request_head_timeout 20s;                  # 30s when not written
    }

    server shop {
        server_name shop.example;
        security_policy office;
        proxy app;

        route sign-in {
            match exact /login;
            security_policy login;
            proxy app;
        }
    }
}
```

Rate limits are token buckets as NGINX's `limit_req` keeps them: a bucket
holds one request plus the burst and refills at the rate, written as
`<n>r/s`, `r/m`, `r/h`, `r/d` or `<n>r/<seconds>s`. They count by `$client_ip`
(the default), `$host`, `$route` or a request header such as `$http_x_api_key`,
and answer with 429 and `Retry-After` unless `limited_response` says
otherwise. Buckets live in each gateway process.

Without trusted proxies, the client is the TCP peer, and `X-Forwarded-For`,
`X-Real-IP` and `Forwarded` it sends are dropped before the request is
forwarded. With them, the gateway reads the header `real_ip_header` names
from the right, skipping trusted networks; the first address outside them is
the client that rules and limits see, and upstreams receive it as
`X-Real-IP`.

Every listener also gives clients a deadline for each request head, 30
seconds unless `request_head_timeout` says otherwise: counted from the
connection's start for its first request and from the end of the previous
request for later ones. A client that sends its request a byte at a time
(Slowloris) gets 408 for a late first head, and a late later head, or a
kept-alive connection left idle that long, is closed. HTTP/2 connections are
watched until their preface.

Password files are htpasswd files in the gateway's secret directory, made
with `htpasswd -B` or an Argon2 tool; a file holding weaker hashes is refused
when the snapshot is prepared. The gateway remembers verified credentials for
a few minutes, so the hashes' cost is paid once per user, and does not
forward `Authorization` upstream.

`/api/v1/security-policies` lists, reads, sets and deletes policies; sites
and routes name one with `security_policy_id`, and listeners take
`trusted_proxies` and `real_ip_header`. A policy that a site or route uses
cannot be deleted. Reading needs `config.read` and changing `config.write`,
and changes become active when the configuration is applied. The console's
Security policies page and the site, route and listener forms offer the same.

```sh
ppanel security-policy set office --allow 10.0.0.0/8 --method GET --method POST \
  --basic-auth staff.htpasswd --max-body-size 10m --rate-limit "10r/s burst=20"
ppanel security-policy set login --rate-limit 5r/m
ppanel site create --name shop --domain shop.example --proxy <upstream-id> --security-policy office
ppanel route add <site-id> --match exact:/login --proxy <upstream-id> --security-policy login
ppanel listener set public --address 0.0.0.0:443 --tls-profile edge --trusted-proxy 192.0.2.0/24
ppanel security-policy list
```

`ppanel config check` and the console's configuration editor warn when a
server asks for passwords on a plain HTTP listener without `https_redirect`,
when a listener trusts every address as a proxy, and when an upstream does
not verify its TLS nodes.

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
service schema, so services migrate independently. A published migration is never
edited, renamed or removed, and a new one sorts after every published version in its
directory: CI compares them with the baseline commit, because a database that applied
the old file would otherwise refuse to start or apply changes out of order.

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

Integration tests use disposable servers named by `PANEL_TEST_DATABASE_URL`,
`PANEL_TEST_NATS_URL` and `PANEL_TEST_ACME_*` and skip without them; CI sets
`PANEL_REQUIRE_INTEGRATION_SERVICES` so a missing server fails instead. The
ACME server is Pebble, Let's Encrypt's test CA, with its DNS test server
resolving every name to 127.0.0.1; its release binaries are downloaded once
and checked against pinned digests. Locally:

```sh
panel/scripts/dev-services.sh up
eval "$(panel/scripts/dev-services.sh env)"
cargo test --manifest-path panel/Cargo.toml --package panel-postgres --package panel-jetstream \
  --package panel-acme --package automation-service --all-features
panel/scripts/dev-services.sh down
```

`PANEL_TEST_RFC2136_SERVER` additionally runs the RFC 2136 provider against a
real primary for `example.com`, as described in
`panel/dns-rfc2136/tests/primary.rs`.

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
system, no capabilities and `no-new-privileges`. Passwords, the bootstrap
token, the password pepper and the master key that seals certificate keys
are generated into `deploy/secrets/` (never committed) and mounted as
Compose secrets; back the master key up with the database, since stored
private keys cannot be opened without it. `automation-service` writes
certificates into the `gateway-secrets` volume, which the gateway mounts
read-only.
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
