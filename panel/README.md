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
panel-config-api -> panel-application + panel-config-model
panel-certificate-api -> panel-application + panel-domain
panel-api -> panel-application + panel-config-api + panel-certificate-api + panel-config-model + panel-errors
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
panel-sqlite -> panel-outbox + panel-event-codec + panel-events + panel-health + panel-errors
panel-jetstream -> panel-event-codec + panel-events + panel-health + panel-errors
gateway-grpc -> gateway-proto-codec + panel-engine::GatewayEngine
gateway-grpc-client -> panel-application + gateway-proto-codec + panel-contracts + panel-health
config-proto-codec -> panel-application + panel-config-api + gateway-proto-codec + panel-contracts
config-grpc-client -> panel-config-api + config-proto-codec + panel-service
observability-grpc-client -> panel-application + panel-contracts + panel-service
panel-control-runtime -> panel-sqlite + panel-jetstream + panel-outbox + panel-service + panel-tls

gatewayd -> runtime + filesystem adapter + Pingora adapter + gRPC/Proto adapters + REST/compiler adapters
config-service -> panel-control-runtime + gateway-grpc-client + panel-config-json + panel-config-api + panel-config-model + config-proto-codec
panel-api-server -> panel-control-runtime + panel-api + config-grpc-client + gateway-grpc-client
panel-cli (no workspace dependencies: a client of the public REST API)
automation-service -> panel-control-runtime + panel-certificate-api + panel-jobs + panel-sqlite + panel-events
observability-service -> panel-control-runtime + panel-contracts + panel-domain
audit-service -> panel-control-runtime + panel-jetstream + panel-events + panel-contracts
panel-control -> panel-control-runtime + panel-api-server + config-service + automation-service + observability-service + audit-service
panel-bootstrap -> panel-jetstream + panel-pki
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
| `panel-control-runtime` | Composition of control-plane modules: the module's database and its migrations, lazy broker connection, registration, the outbox relay, health and graceful shutdown, and hosting modules in one process that reach each other over in-memory gRPC | Application rules, Pingora |
| `panel-schedule` | RFC 5545 recurrences and the time windows they open, shared by schedules, maintenance windows, approval policies and grants | Storage, transports, Pingora |
| `panel-jobs` | Durable job model, leasing worker, retry policy, schedules and maintenance windows, and an in-memory store | Storage, transports, Pingora |
| `panel-pki` | Internal certificate authority, workload identities, credential files and renewal | Transports, storage, Pingora |
| `panel-tls` | TLS 1.3 mutual authentication with reloadable credentials, tonic server and client integration, and per-service peer authorization | Storage, application rules, Pingora |
| `panel-outbox` | Ordered at-least-once outbox relay over `OutboxSource`, `OutboxWakeup` and `EventPublisher` ports | Storage, brokers, Pingora |
| `panel-sqlite` | One private SQLite file per module in write-ahead-log mode, its migrations, the transactional outbox, the idempotent-consumer inbox and the database health check | Application rules, transports, Pingora |
| `panel-jetstream` | Stream provisioning, deduplicated CloudEvents publication, durable consumers, dead letters, targeted replay and the broker health check | Storage, application rules, Pingora |
| `panel-ir` | Versioned canonical runtime snapshot | Proto, storage, Pingora |
| `panel-engine` | `GatewayEngine`, `DataPlaneAdapter`, `SnapshotStore`, runtime-info ports and Fake | Proto, storage implementation, Pingora |
| `panel-application` | Request context, format-neutral config document, use-case orchestration and persistence ports | HTTP, Proto, storage implementation, Pingora |
| `panel-api` | Axum HTTP mapping, body limits, request-ID propagation, RFC 9457 Problem Details and OpenAPI projection | Pingora, storage, identity implementation, generated Proto, use-case orchestration |
| `panel-config-json` | JSON `ConfigCompiler` adapter with schema and document limits | HTTP, Proto, storage, Pingora, application orchestration |
| `panel-config-model` | The editable configuration document: listeners, TLS profiles, upstreams and sites with domains and routes; validation, queries, checked edits and compilation into the IR | HTTP, Proto, storage, Pingora |
| `panel-config-api` | The configuration API as one contract: its port and every read and change as a typed operation, shared by callers and the service | HTTP, Proto, storage, Pingora |
| `panel-certificate-api` | The certificate API as one contract: its port, every read and change of certificates, ACME accounts and DNS providers as a typed operation, and secrets that are never printed | HTTP, Proto, storage, Pingora |
| `panel-gateway-runtime` | Prepare/Activate/CAS/LKG orchestration | Tonic, filesystem, Pingora |
| `snapshot-store-fs` | Versioned JSON records, fsync and atomic rename | Tonic, Pingora, runtime policy |
| `gateway-pingora` | Compile IR into private Pingora values with atomic `ArcSwap` publication, and run the data plane: listener generations, virtual hosts, routes, TLS, static files and upstream pools with health | Proto, control-plane policy |
| `gateway-proto-codec` | Shared Proto/IR conversion used by client and server | Engine, server, client, Pingora, filesystem |
| `gateway-grpc` | Runtime-info projection, request policy and Tonic service | Pingora, filesystem, environment |
| `config-proto-codec` | Protobuf form of the configuration publication contract | Storage, transports, Pingora |
| `config-grpc-client` | `GatewayUseCases` over `config-service`'s publication API | Storage, Pingora |
| `gateway-grpc-client` | Tonic client adapter implementing `panel-application::GatewayPort`, and the gateway health check | HTTP, storage, identity, generated Proto outside this adapter |
| `config-service` | Publication and configuration APIs: the configuration use cases behind their port, storage ports with SQLite stores for the draft, revisions, approvals and activation receipts in `config.db`, and the gRPC adapter | HTTP, Pingora |
| `panel-api-server` | The `panel-api` module: public REST and web console, degraded admission and the service directory | Storage implementation, Pingora |
| `automation-service` | Job store with outbox events, worker, scheduler, and the certificate inventory behind its port with a gRPC adapter, kept in `automation.db` | HTTP, Pingora |
| `observability-service` | Traffic summaries and series from Prometheus over the gateway's metrics, and alerts kept in `observability.db` | Pingora |
| `observability-grpc-client` | `TrafficPort` over `observability-service` | Storage, Pingora, metric backends |
| `audit-service` | The audit trail: every event appended once to a hash chain in `audit.db`, with queries and verification | HTTP, Pingora |
| `panel-control` | The control-plane binary: the five modules in one process, its health check and graceful shutdown | Application rules, storage, Pingora |
| `panel-bootstrap` | Idempotent provisioning of the event streams and the service registry; issuance and rotation of service credentials | Application rules, Pingora |
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

## The control plane

`panel-control` runs the control plane as one process
([decision](../docs/adr/0032-one-control-plane-process-on-sqlite.md)): the
`audit-service`, `config-service`, `automation-service`,
`observability-service` and `panel-api` modules, which start in that order
and stop in reverse, so the API stops taking requests first. Each module is
composed by `panel-control-runtime`
([decision](../docs/adr/0007-service-processes-health-and-discovery.md)): it
binds an operational listener with `/livez` and `/readyz`
(`application/health+json`) and `/metrics`, where `panel-api` also counts
its requests by route template; serves `grpc.health.v1.Health`,
`pingora.panel.platform.v1.ServiceInfo` and its own gRPC services to the
other modules over in-memory streams rather than a network listener; then
migrates its schema, registers in the service directory and relays its
outbox in the background. Run `panel-control healthcheck` to probe every
module's readiness, as the container health check does.

| Module | Storage | Operational | Other |
|---|---|---|---|
| `panel-api` | `identity.db` | `127.0.0.1:9180` | public HTTP `127.0.0.1:8080` |
| `config-service` | `config.db` | `127.0.0.1:9181` | calls `gatewayd` at `127.0.0.1:50051` |
| `automation-service` | `automation.db` | `127.0.0.1:9182` | |
| `observability-service` | `observability.db` | `127.0.0.1:9183` | queries Prometheus at `127.0.0.1:9090` |
| `audit-service` | `audit.db` | `127.0.0.1:9184` | consumes every event |

Each module keeps its SQLite file in `PINGORA_PANEL_DATA_DIR`
(`/var/lib/pingora-panel/control` by default). The process reads
`PINGORA_PANEL_NATS_URL`, and optionally `PINGORA_PANEL_HEALTH_INTERVAL_MS`.
`config-service` also reads `PINGORA_PANEL_GATEWAY_URL`;
`observability-service` reads `PINGORA_PANEL_PROMETHEUS_URL`; `panel-api` reads
`PINGORA_PANEL_HTTP_ADDR`, `PINGORA_PANEL_GATEWAY_URL` for the gateway's
runtime API,
`PINGORA_PANEL_WEB_ROOT`, the directory of the built console, and the
identity settings described under [Accounts and access](#accounts-and-access).
Plaintext listeners must stay on loopback until internal transports are
authenticated. `PINGORA_PANEL_HTTP_ADDR` may instead name a Unix domain
socket, `unix:/run/pingora-panel/api.sock`, for the reverse proxy in front
of the console: the socket is made readable and writable by its owner and
group only, replaces a stale socket but never another file, and is removed
at shutdown. As on loopback, the API takes the client address from the last
`X-Forwarded-For` entry the proxy appends.

The control plane reaches `gatewayd` and the host agent over mutual TLS
([decision](../docs/adr/0009-internal-mutual-tls.md)) once
`PINGORA_PANEL_CREDENTIALS_DIR` names the directory with a subdirectory per
module, named after its service, holding its `identity.pem` and `trust.pem`;
`PINGORA_PANEL_TRUST_DOMAIN` is optional. Clients verify each peer's identity
`<service>.<trust domain>` whatever address they dial, and the gateway's API
admits only `config-service` and `panel-api`. `gatewayd` reads its own
credentials from `PINGORA_PANEL_TLS_DIR`; its gRPC listener then requires
client certificates from the installation's authority and may bind beyond
loopback. Credentials reload without a restart.
`panel-bootstrap pki` keeps them current: it creates the authority in
`PINGORA_PANEL_PKI_DIR` on first run and issues to every
`service=directory` pair in `PINGORA_PANEL_PKI_CREDENTIALS` whose credentials
are missing or past two thirds of their lifetime
(`PINGORA_PANEL_CERTIFICATE_LIFETIME_MS`, 24 hours by default), once with
`--once` or continuously.

`panel-bootstrap` runs once per installation and on every upgrade. It
provisions the event streams and the service registry.

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
replacing or deleting one requires `If-Match`. A change may carry an
`Idempotency-Key`, so a retry returns the first outcome instead of acting
twice, and an `x-deadline`, which is 150 seconds after the request when
absent. A change that would introduce a validation error is
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

A route takes requests by path and, optionally, by conditions that must
all hold ([decision](../docs/adr/0036-route-conditions.md)): methods,
hosts, header fields, query parameters and cookies that are present,
absent, equal to, start or end with or contain a value or match a regular
expression, client networks after trusted proxies, user agents, referers
and media types, grouped with `any`, `all` and `not`. The gateway and the
route tester share one matcher, so `POST /api/v1/config/route-test`
explains which route of the draft a request takes, and why each route
before it does not, exactly as the gateway would decide:

```sh
ppanel route add <site-id> --match prefix:/api --proxy <canary-upstream-id> \
  --method GET --header x-canary=1
ppanel route test --host shop.example --target /api/items -H 'X-Canary: 1'
```

In the configuration language the same route reads:

```nginx
route canary {
    match prefix /api;
    method GET;
    header x-canary = 1;
    not { client 192.0.2.0/24; }
    proxy canary;
}
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
`$client_ip`, `$request_id`, `$upstream_addr`, `$http_<name>`,
`$cookie_<name>` and `${lua:name}`, what `set` or a script last gave a
variable, with `$$` for a literal dollar; gateways that cannot
evaluate them refuse the snapshot. `POST /api/v1/config/import/nginx`
converts the documented NGINX subset — `server`, `listen`, `server_name`,
`location`, named ones included, `proxy_pass`, `root`, `index`,
`try_files`, `return` and `upstream` — and reports every directive it did
not carry over at its
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

The whole configuration also travels as one bundle: a JSON file naming its
format and language version with every file of the draft, so sites,
upstreams, listeners, TLS profiles and security policies move between
installations together; certificates stay behind, named by the TLS profiles
that use them. `GET /api/v1/config/bundle` exports the draft as an
attachment and `PUT` imports a bundle as a change of the draft, validated
and audited like any other; a file that is not a bundle, or one written in a
newer language version, is refused before anything changes
([decision](../docs/adr/0035-backups.md)). The configuration page's import
and export menu does the same.

```sh
ppanel config export --bundle configuration.json
ppanel config import configuration.json --expected-version 7
ppanel config export --bundle - | ssh standby ppanel config import -
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
and out, open connections, handshakes, upstream error ratios and the share
of their connections reused from the gateway's pool, failed upstream
attempts by node and why, the busiest routes and the busiest
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
again every 30 seconds. The gateway overview gathers what needs attention:
the latest requests answered with a server error, the upstream nodes whose
attempts failed in the last 15 minutes and why, and certificates that have
expired, expire soon or failed to renew, each leading to its page.

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

## Host

The Compose installation runs Prometheus's node exporter on loopback,
reading the host's root read-only without capabilities
([decision](../docs/adr/0028-host-and-container-operations.md)), and
Prometheus scrapes it. `observability-service` reads the host name,
operating system, kernel, architecture, clock and time zone, uptime, CPU
use, load, memory, each real filesystem once and each physical network
device's traffic with fixed queries. `GET /api/v1/host` serves them with
each filesystem's share used and a warning from 85% and a critical level
from 95%; it needs `host.read`.

```sh
ppanel host
```

The console's Host page shows the same figures and warnings.

### Host agent

What no container may do on the host, `ops-agent` does
([decision](../docs/adr/0030-ops-agent.md)). It runs on the host as a
systemd service, as a dynamic user in a sandbox, and serves gRPC over
mutual TLS on a Unix socket that only the control plane mounts; it admits
the containers' user only, and among their identities `panel-api` only. Each
capability is enabled on its own with the privilege it needs, and the
agent reports which ones it has.

For the Compose installation, build the image, install the agent with the
capabilities wanted, then start the installation with the agent's override:

```sh
docker compose -f panel/deploy/compose.yaml build
sudo panel/deploy/ops-agent/install.sh directories listeners containers
docker compose -f panel/deploy/compose.yaml \
  -f panel/deploy/compose.ops-agent.yaml up -d
```

The installation's authority issues the agent's credentials into
`/var/lib/pingora-panel/ops-agent/credentials`; systemd starts the agent
once they exist and restarts it with each renewal. Its settings are in
`/etc/pingora-panel/ops-agent.env`.

The directory sizes capability measures the gateway's configuration, log
and certificate directories, which the settings name: in the Compose
installation, the gateway's volumes, whose place on the host `install.sh`
asks the engine for, creating them as Compose would when the installation
has not started yet. Reading them takes `CAP_DAC_READ_SEARCH`, which
`directories.conf` grants while hiding everything else under `/var` and the
host's credentials from the agent.

The port diagnostics capability, on Linux, names the processes listening
on TCP ports from `/proc`: their name, ID, executable and user, never
their command line. Reading other processes' descriptors takes
`CAP_DAC_READ_SEARCH` and `CAP_SYS_PTRACE`, which `listeners.conf` grants;
the agent still cannot call `ptrace`.

The gateway service capability manages what runs the gateway: its
container in the installation's Compose project, which the agent finds by
the project's and the service's labels (`pingora-panel` and `gatewayd`,
or `PINGORA_PANEL_OPS_INSTALLATION_PROJECT` and
`PINGORA_PANEL_OPS_GATEWAY_SERVICE`) on an enabled engine. It reports the
container's state and health, when it last started and stopped, its exit
code and how often the engine restarted it, and starts, stops or restarts
it, answering once the engine has finished. It needs the containers
capability below, which reaches the engine; that container is the one in
the installation the agent may stop or restart.

```sh
ppanel host agent
ppanel host directories
ppanel host listeners --port 80 --port 443
ppanel host gateway-service
ppanel host gateway-service restart --yes
```

`GET /api/v1/host/agent` says whether an agent is configured and answers
and which capabilities it has; `GET /api/v1/host/directories` reports each
directory's size and file count, and whether the count is partial;
`GET /api/v1/host/listeners?ports=80,443` lists what listens on those
ports; `GET /api/v1/host/gateway-service` reads the gateway's service.
They need `host.read`. `POST /api/v1/host/gateway-service/{start,stop,restart}`
needs `host.manage`, which only administrators hold, and the audit trail
records each change, refused or not. The console's Host page shows the
agent and, when it has those capabilities, the gateway service with its
actions, the directories and what holds ports 80 and 443, saying whether
the gateway or another process does.

## Containers

The host agent's containers capability reaches Docker and Podman through
their sockets ([decision](../docs/adr/0031-containers.md)).
`containers.conf` points the agent at Docker's socket and makes it a
member of the group that owns it, which makes the agent root on the host
through the engine; the file says how to add Podman's socket. Install it
with the other capabilities:

```sh
sudo panel/deploy/ops-agent/install.sh directories listeners containers
```

Every engine the agent is pointed at starts enabled. An operator can
disable one, and the panel then leaves what runs on it alone; the agent
keeps that choice in its state directory.

```sh
ppanel container engines
ppanel container engine disable podman
ppanel container list --search nginx --state running
ppanel container inspect shop-web-1
ppanel container logs shop-web-1 --lines 500 --since 1h --timestamps
ppanel container logs shop-web-1 --follow
ppanel container stats
ppanel image list --search nginx
ppanel image remove ghcr.io/example/app:2.3 --yes
ppanel image pull nginx:1.27 --platform linux/arm64
ppanel image pull ghcr.io/example/app:2.3 --username ci --password-stdin < token
ppanel network list
ppanel volume list
ppanel container engine df docker
ppanel container engine prune docker
ppanel container engine prune docker --named-volumes --yes
ppanel compose list
ppanel compose logs shop --lines 500 --since 1h
ppanel compose config shop
ppanel compose restart shop --yes
ppanel container links
ppanel container proxy shop-web-1 --domain shop.example
ppanel container restart shop-web-1 --yes
ppanel container remove nightly-report --volumes --yes
```

`GET /api/v1/container-engines` reports each engine: whether it is enabled
and answers, its version, and how many containers and images it has.
`GET /api/v1/container-engines/{engine}/containers` lists an engine's
containers, searched by name or image and filtered by state, with their
published ports and Compose project. Both need `containers.read`.
`GET /api/v1/container-engines/{engine}/containers/{container}` needs
`containers.inspect` and adds a container's start and stop times, exit
code, restarts, health, restart policy, host name, user, working
directory, platform, labels, mounts and networks; never its environment
or command line, which carry secrets.
`GET /api/v1/container-engines/{engine}/containers/{container}/logs`
needs `containers.inspect` too, since what a container prints can hold
secrets. It returns the container's last lines as its engine's log driver
kept them, 200 by default and at most 5000, optionally from a time on,
oldest first. Each line has the time its engine recorded, to the
nanosecond, and whether it came from standard output or standard error;
everything a container with a terminal prints is standard output. Lines
are cut at 16 KiB, and an answer keeps to the newest 2 MiB and says when
older lines were left out. `.../logs/tail` follows the lines a container
prints over a WebSocket, after up to 1000 of its last lines or after the
line a client last received (`after`, a message's `cursor`), and closes
normally once the container stops. The agent follows at most 64 logs at
once. `ppanel container logs` writes standard error to standard error,
as the container did.
`GET /api/v1/container-engines/{engine}/stats` reports what every running
container uses, and `.../containers/{container}/stats` what one does;
both need `containers.read`, since statistics hold no secrets. They are
read as `docker stats --no-stream` reads them: CPU as a share of one CPU
between the engine's two samples a second apart, memory without the page
cache the kernel can reclaim against its limit, network traffic summed
over the container's interfaces with errors and drops, block I/O and
processes, each with the time the engine read them. The agent reads at
most 16 containers at once, and leaves out one that stops meanwhile.
`GET /api/v1/container-engines/{engine}/images` lists an engine's images
with their tags, digests, size and the containers made from each,
searched by tag, digest or ID, and `.../images/{image}` shows one's platform,
author, user, working directory, exposed ports, volumes, stop signal,
layers and labels, never its environment or command line; both need
`containers.read`. `DELETE .../images/{image}` needs `containers.manage`
and removes the reference it names, and the image once nothing else
names it; `force` removes one stopped containers use, though the engine
still refuses one a running container uses. An image is named by its ID,
a prefix of it, or a reference with its slashes percent-encoded, and the
agent refuses a reference that would leave the engine's image paths. An
image the installation's own containers use is never removed, and each
removal is audited as `container.image.removed`, refused or not.
`POST /api/v1/container-engines/{engine}/image-pulls` needs
`containers.manage` and pulls a reference, checked against the
distribution grammar; a name alone is its `latest` tag, never every tag.
A platform can be named, and a registry that wants a sign-in gets a
username and password or token for that pull only, never kept, recorded
or printed. The API answers with server-sent events, each a JSON
message: how far each layer got, at most four times a second and at
least every ten seconds, then what was pulled, its digest and whether a
newer image was downloaded, or why the pull failed. A refusal before
anything is pulled answers as a problem like any other, so a registry's
refusal never reads as the session ending. The agent pulls at most four
images at once, each for at most 30 minutes; a pull goes on to its end
when its client leaves, and is audited then as `container.image.pulled`,
refused or not. `ppanel image pull` writes each layer's change to
standard error as `docker pull` does and the reference pulled to
standard output, and reads a password only from standard input.
`GET /api/v1/container-engines/{engine}/networks` lists an engine's
networks with their driver, scope, subnets and gateways, and
`.../volumes` its volumes with where their data lives on the host; each
says how many containers use it and which Compose project created it.
Both need `containers.read`, as does `.../disk-usage`, which reports how
much disk the engine's images, containers, local volumes and build cache
take, how many of each are in use, and what removing the rest would
free, as `docker system df` does. The agent reads the engine's own
figures: Engine API 1.52 and later sum them up, and the agent sums an
earlier version's lists as the Docker CLI does.
`GET .../prune-preview` lists what pruning would remove, each with what
removing it frees: containers that are not running, images no container
uses, anonymous volumes nothing mounts, networks no container is on and
build cache not in use; `tagged_images` and `named_volumes` add images a
tag names and named volumes. What the installation made, and the
engine's own networks, are never listed. `POST .../prune` needs
`containers.manage` and takes up to 1000 items of a preview with its
choices; the agent lists again and removes, one at a time, the requested
items still listed, so whatever came into use meanwhile stays, and
starts no removal after 130 seconds. Each prune is audited as
`container.engine.pruned`, refused or not.
`GET /api/v1/container-engines/{engine}/compose-projects` needs
`containers.read` and lists the Compose projects the engine's containers
are labelled with, running or not, each with its working directory,
Compose files and services and how many of its containers run, marking
the panel's own installation.
`POST .../compose-projects/{project}/{up,down,restart}` needs
`containers.manage`: up starts the project's containers that are not
running, down stops and removes its containers and networks and keeps
its volumes, and restart restarts its containers, at most 8 at once. The
answer says how many containers changed and which containers or networks
the engine refused. Recreating a project from a changed file stays with
Compose, and the installation is only ever brought up. Each action is
audited as `container.compose.up`, `.down` or `.restarted`, refused or
not. `.../logs` merges the last lines of all the project's containers by
time, each with its service and container, and `.../files` reads the
Compose files its labels name; both need `containers.inspect`, since
either can hold secrets. A file is read only when it lies in the
project's working directory, ends in `.yml` or `.yaml`, is a regular
file and is at most 256 KiB, after following links; the agent's sandbox
hides home directories and `/tmp`, and a file that cannot be read says
why.
Sites go in front of containers
([decision](../docs/adr/0033-sites-in-front-of-containers.md)) through
their endpoints, where the gateway can reach a running container's TCP
ports: each published port, a wildcard address read as `127.0.0.1`, then
each of its ports on its addresses on networks other than the host's.
Listed containers carry their addresses, endpoints and the site their
labels declare: `pingora-panel.site.domains`, comma-separated hosts,
`pingora-panel.site.port` and `pingora-panel.site.name`; nothing is created
from labels. `GET /api/v1/container-engines/{engine}/site-links` needs
`containers.read` and `config.read` and lists the sites of the draft
whose upstream nodes, the site's own or its routes', have the address and
port of a container's endpoint, `localhost` and the loopback addresses
being one host, naming only sites the caller may read, and the declared
hosts no site serves.
`POST .../containers/{container}/sites` needs `config.write` and
`containers.read` and adds an upstream with one of the container's
endpoints as its node and a reverse-proxy site for it to the draft as one
import, taking what the request leaves out from the labels, or the
container's name and only port, published first; applying the draft is a
step of its own.
`POST /api/v1/container-engines/{engine}/{enable,disable}` needs
`containers.manage`, as do
`POST /api/v1/container-engines/{engine}/containers/{container}/{start,stop,restart,kill}`
and `DELETE /api/v1/container-engines/{engine}/containers/{container}`,
which removes a running container only with `force` and its anonymous
volumes with `volumes`. A container is named by its ID, a unique prefix
of its ID or its name, and the installation's own containers are only
ever started. The audit trail records each change and action, refused or
not. The console's Containers page shows the engines and their
containers, opens a container's details from its name and its logs from
the details or its row, and has a menu on each container that asks
before anything but a start. Its logs can be filtered by output and
text, followed from the last line read and saved as a file. The table
shows each running container's CPU and memory, and the details show
everything it uses. An Images tab lists the engine's images, opens one's
configuration, removes one after confirming and pulls one, showing each
layer's state and how far it got; Networks and Volumes tabs list those.
The table links each container to the sites in front of it, says which
declared hosts no site serves, and puts a site in front of a running
container from its row; an upstream's node form offers running
containers' endpoints, and a site's page lists the containers it points
at. A Projects tab lists the engine's Compose
projects, brings one up, or restarts or takes it down after confirming,
and opens its merged logs, filtered by service, and its Compose files. A
reachable engine's card opens its disk use, where operators see what
pruning would remove and remove it after confirming.

## Site files

The control plane manages the files below the directory the gateway serves
static sites from ([decision](../docs/adr/0034-site-files.md)), which
`PINGORA_PANEL_SITES_ROOT` names; the Compose installation mounts its
`sites` volume there read-write. Every operation opens that directory as a
capability and resolves paths beneath it, so neither a path nor a symbolic
link in the tree leads out of it. Paths are names separated by `/`; empty
parts, `.`, `..`, backslashes and control characters are refused. As in
NGINX, a static site whose directory is not there answers 404 until it is,
so removing a site's files does not stop other changes from applying.

```sh
ppanel files ls shop
ppanel files cat shop/index.html
ppanel files get shop/logo.png --to logo.png
ppanel files put index.html shop/index.html --if-match '"<sha256>"'
printf 'User-agent: *\n' | ppanel files put - shop/robots.txt --new
ppanel files mkdir blog/2027
ppanel files rm shop/old --recursive --yes
```

`GET /api/v1/site-files?path=` lists a directory, directories first, and
`DELETE` removes an entry, a directory with what it holds only with
`recursive=true`. `GET /api/v1/site-files/content?path=` returns a file of up
to 64 MiB as an `application/octet-stream` attachment with `nosniff`, so
nothing uploaded runs as a page of the console's origin, with its entity tag
in `ETag`; `PUT` writes one atomically, through a temporary file renamed
over it, creating its directories, `If-Match` replacing only the file of
that tag and `If-None-Match: *` only creating one.
`POST /api/v1/site-files/directories?path=` creates a directory.
`files.read`, held by viewers and operators, lists and reads; `files.write`,
held by operators, changes. Each change is audited as
`files.file.written`, with its size and digest but never its content,
`files.directory.created` or `files.entry.removed`, refused or not.
The console's Site files page browses the directory, edits text files of up
to 1 MiB on the entity tag they were read with, so a file changed meanwhile
is not overwritten, and uploads files picked or dropped on it. The
configuration's own files stay in the configuration language's editor.

## Backups

A backup is one archive, a Zstandard-compressed tar archive whose first
member, `manifest.json`, lists every directory and each file with its size
and SHA-256 ([decision](../docs/adr/0035-backups.md)). It holds any of the
configuration's database, with the draft and the active revision as
configuration bundles; the certificates, ACME accounts and DNS providers,
their keys sealed; every module's database; and the sites' directory or one
directory below it. Databases are copied with SQLite's `VACUUM INTO`, a
consistent snapshot taken while the control plane keeps running. The master
keys, the password pepper and the bootstrap token are never in an archive,
so restoring sealed values needs the installation's master keys.

`POST /api/v1/backups` lists a backup as pending and takes it in the
background; archives are kept in the control plane's data directory, owner
only, and `PINGORA_PANEL_BACKUPS_KEPT` (10 by default) finished backups are
kept. `GET /api/v1/backups/{id}/archive` downloads one as
`application/zstd` with its SHA-256 as `ETag` and `Repr-Digest`
(RFC 9530). `POST /api/v1/backups/{id}/restores` replaces a directory of the
sites with the archive's copy, unpacked beside it and renamed into place, or
saves the archive's active revision as the draft to review and apply. Every
archive is checked whole against its manifest before anything is restored
from it. `backups.read` lists backups and `backups.manage` does the rest;
each request, removal and restore is audited, refused or not. The console's
Backups page does the same.

```sh
ppanel backup create --with configuration,certificates,sites --wait
ppanel backup download 6f1c7a52-2b8e-4d6b-9a33-0d3c58f1e2a4
ppanel backup restore 6f1c7a52-2b8e-4d6b-9a33-0d3c58f1e2a4 --sites shop
ppanel backup restore 6f1c7a52-2b8e-4d6b-9a33-0d3c58f1e2a4 --configuration
```

Every database is restored with the control plane stopped, from an archive
in its data directory or anywhere else: `panel-control restore` refuses to
run while the control plane answers, checks the archive and each database's
integrity, refuses databases at a schema newer than the release reaches, and
keeps the files it replaces beside the new ones. The next start migrates
them forward.

```sh
docker compose stop control
docker compose run --rm --no-deps control /usr/local/bin/panel-control restore \
  /var/lib/pingora-panel/control/backups/6f1c7a52-2b8e-4d6b-9a33-0d3c58f1e2a4.tar.zst
docker compose up --detach --wait control
```

## Alerts

`observability-service` evaluates alert rules
([decision](../docs/adr/0027-alerts.md)) every 30 seconds. A rule compares one measure over the last five
minutes — the share of 5xx responses, P95 latency, the request rate, the
share of failed upstream attempts or open connections — with a threshold,
for every site, one site, one route or one upstream, and fires once the
condition has held for its pending period. Firing and resolving queue a
notification for each of the rule's channels, which a sender posts in
Alertmanager's webhook payload, signed as Standard Webhooks specify, and
retries for a day. A channel's URL and signing secret are sealed with the
master keys; only the URL's origin is shown, and the secret only when the
channel is created or rotated. Set `PINGORA_PANEL_PUBLIC_ORIGINS` on the
control plane so notifications link to the console. Email
channels are reserved and refused as unsupported.

`/api/v1/alert-rules`, `/api/v1/alert-channels` and
`/api/v1/alert-notifications` serve them; reading needs `alerts.read` and
changing `alerts.manage`. The console's Alerts page manages rules and
channels and lists notifications, and the overview leads with firing
alerts.

```sh
ppanel alert channel create ops --url-file hook-url
ppanel alert rule set shop-errors --measure server-error-ratio --above 0.05 \
  --pending 5m --site shop --severity critical --channel ops
ppanel alert rule list
ppanel alert notifications --rule shop-errors
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

## HTTP policies

HTTP policies change request and response fields, the `Server` field,
cross-origin access and compression
([decision](../docs/adr/0037-http-policies.md)). Like security policies they
are named resources that sites and routes use: a site's policy applies to
all of its requests, a route's after it, and a route's `cors` and
`compress` replace the site's.

```nginx
http {
    http_policy site {
        response_header set Strict-Transport-Security "max-age=63072000; includeSubDomains";
        response_header set X-Frame-Options DENY;
        response_header remove X-Powered-By;
        server_header remove;                      # or keep, or replace <value>
    }

    http_policy api {
        request_header remove X-Debug;
        request_header set X-Tenant $host;         # values are templates
        request_header add X-Client $client_ip;
        cors {
            origins https://shop.example https://*.shop.example;
            methods PUT DELETE;
            headers X-Api-Key;
            expose X-Request-Id;
            credentials on;                        # the origin is echoed, never *
            max_age 10m;                           # at most a day
        }
        compress gzip br zstd types=text/*,application/json min_size=1k;
    }

    server shop {
        server_name shop.example;
        http_policy site;
        proxy app;

        route api {
            match prefix /api;
            http_policy api;
            proxy app;
        }
    }
}
```

Field changes apply as removals, then replacements, then additions, to
requests before they go upstream and to every response, proxied or
generated by the gateway. Values may use `$host`, `$uri`, `$method`,
`$scheme`, `$client_ip`, `$request_id`, `$http_<name>` and `$cookie_<name>`,
and `$$` is a `$`. Hop-by-hop and framing fields, `Host` and the forwarding
fields the gateway writes cannot be changed, and `Content-Encoding` is left
to compression.

For CORS the gateway follows the Fetch Standard. It answers preflights
itself with 204: for an allowed origin with the allowed methods, the
requested fields when they are allowed, credentials and the preflight age,
and for any other origin with `Vary` only. Responses to allowed origins
carry the origin, `Vary: Origin` and the exposed fields. Origins are written
as browsers send them, such as `https://shop.example:8443`, with `*.` for
one label of subdomains or `*` for every origin; `*` with credentials is
refused.

Compression uses the listed coding the client weighs highest in
`Accept-Encoding` (RFC 9110 §12.5.3: `q=0` refuses a coding, `*` covers
unnamed ones, and br, zstd and gzip go in that order when weighed the
same), for proxied and static responses of the listed media types and at
least the minimum size. Responses that are already encoded, partial, to HEAD, without
a body or marked `Cache-Control: no-transform` are sent as they are, and
compressed ones carry `Vary: Accept-Encoding`.

Every proxied request, with or without a policy, keeps the client's `Host`
unless the upstream sets `host_header`, gains `X-Forwarded-For`,
`X-Forwarded-Host`, `X-Forwarded-Proto`, `Forwarded` (RFC 7239), `Via` and
`X-Real-IP`, keeps a valid `X-Request-Id` or gets a new one, and passes its
`traceparent` on.

`/api/v1/http-policies` lists, reads, sets and deletes policies; sites and
routes name one with `http_policy_id`. A policy that a site or route uses
cannot be deleted, and one the gateway would refuse is answered with its
problems. The console's HTTP policies page edits the same, and the site and
route forms pick a policy.

```sh
ppanel http-policy set site --response-set 'X-Frame-Options: DENY' --remove-server
ppanel http-policy set api --request-set 'X-Tenant: $host' \
  --cors-origin https://shop.example --cors-method PUT --cors-credentials \
  --compress gzip --compress br --compress-type 'text/*' --compress-min-size 1k
ppanel site create --name shop --domain shop.example --proxy <upstream-id> --http-policy site
ppanel route add <site-id> --match prefix:/api --proxy <upstream-id> --http-policy api
ppanel http-policy list
```

## Rewrites and internal redirects

Sites and routes change the path a request goes on with, and routes serve
one URL with what another does without telling the client
([decision](../docs/adr/0040-rewrites-and-internal-redirects.md)). A site's
rules run before a route is chosen, a route's once it is, in the order they
are written.

```nginx
server shop {
    server_name shop.example;
    rewrite ^/old/(.*)$ /new/$1 permanent;     # 301 to /new/...
    proxy app;

    route api {
        match prefix /api;
        strip_prefix /api;                     # /api/users goes on as /users
        add_prefix /v2;                        # ... and reaches app as /v2/users
        proxy app;
    }

    route blog {
        match prefix /blog;
        set_uri /index.php?q=$uri;             # the query follows the request's own
        proxy app;
    }

    route legacy {
        match prefix /legacy;
        rewrite "^/legacy/(?<id>\d+)$" /posts/$id last;   # choose the route again
        respond 410;
    }

    route errors {
        match prefix /errors;
        internal;                              # 404 unless sent here from inside
        root errors;
    }

    route gone {
        match prefix /gone;
        internal_redirect /errors/gone.html;   # or @name for a named route
    }
}
```

`rewrite` follows NGINX: `$1` to `$9` and named groups take what the
expression captured; `last` chooses the route again, `break` keeps it,
`redirect` and `permanent` answer 302 and 301, and a replacement starting
with `http://`, `https://` or `$scheme` redirects as well. Without a flag
the next rule runs, and a route whose rules changed the path is chosen again
after its last rule; `strip_prefix`, `add_prefix` and `set_uri` keep the
route. A `?` in a replacement sets the query, followed by the request's own
unless the replacement ends with it. Expressions are matched in linear time,
so patterns with back-references or look-around are refused, and a request
changes its URI at most ten times, by rules, internal redirects and scripts
together, before it is answered 500.

Rules see the path as routes match it, normalized as RFC 3986 says with
percent-encodings other than of unreserved characters kept, so an encoded `/`
never becomes a segment boundary. Upstreams receive the changed path and
query, and static content resolves it; `$uri` is the current path, `$args`
(`$query_string`), `$is_args` and `$arg_<name>` read the current query,
`$request_uri` keeps what the client asked for, and access records of a
rewritten request add it as `url.original`. NGINX configurations imported
with `ppanel config import-nginx` carry `rewrite` and `internal` over.

Sites and routes take the rules as `rewrites` through the API, and routes
take `internal` and the `internal_redirect` action; a client that leaves the
fields out keeps the current ones. The site and route forms list the rules
to add, reorder and remove, and the route list marks internal routes and
rule counts.

```sh
ppanel site create --name shop --domain shop.example --proxy <upstream-id> \
  --rewrite 'rewrite ^/old/(.*)$ /new/$1 permanent'
ppanel route add <site-id> --match prefix:/api --proxy <upstream-id> \
  --rewrite 'strip_prefix /api' --rewrite 'add_prefix /v2'
ppanel route add <site-id> --match prefix:/errors --static errors --internal
ppanel route add <site-id> --match prefix:/gone --internal-redirect /errors/gone.html
```

## Error pages and maintenance

Sites and routes answer errors with their own pages, a site may be in
maintenance for everyone outside an allowlist, and a site answers
`/robots.txt` and `/favicon.ico` itself
([decision](../docs/adr/0041-error-pages-and-maintenance.md)).

```nginx
server shop {
    server_name shop.example;
    error_page 404 410 "body=<h1>$host has no $uri</h1>";
    error_page 502 503 file=errors/50x.html status=503;   # below the static root
    error_page 403 =301 https://shop.example/denied;      # nginx's form reads the same
    intercept_errors on;                                   # upstreams' errors too
    maintenance on allow=10.0.0.0/8,2001:db8::/32 retry_after=10m;
    robots disallow_all;
    favicon file=shop/favicon.ico;
    proxy app;

    route api {
        match prefix /api;
        error_page 404 "body={\"error\":\"not found\"}" type=application/json;
        proxy app;
    }

    route raw {
        match prefix /raw;
        error_page off;                                    # answered without pages
        proxy app;
    }
}
```

A page names the statuses it answers, from 400 to 599, and answers with a
body, a template of request variables that is `text/html` unless a type is
written; with a file below the gateway's static root, read when an error
needs it; or with a redirect, 302 unless another status is written. A body or
file keeps the error's status unless `status=` or nginx's `=code` names
another. Pages answer the errors the gateway makes once a site is known — no
route, a refused request, a missing file, an upstream that cannot be reached,
times out or sheds load, a rewrite cycle — and fixed responses without a
body, keeping fields such as `Retry-After`. With `intercept_errors on`,
upstreams' error responses with a page's status get the page instead; only
`Date`, `WWW-Authenticate`, `Proxy-Authenticate`, `Retry-After` and `Allow`
of theirs are kept. A route that writes any page answers with its own, and a
route that writes only `intercept_errors` keeps its server's pages, as nginx's
locations do.

In maintenance, clients outside the allowlist — after trusted proxies — get
503 with `Retry-After` and the site's 503 page unless `body=` is written,
while the rest reach the site as it is. `maintenance off` keeps the settings
for next time, and ACME challenges are answered throughout. `robots` allows or
disallows every crawler, or writes its own rules with `body=`; `favicon`
answers 204 with `no_content`, a file in a directory below the static root, or
a redirect. NGINX configurations imported with `ppanel config import-nginx`
carry `error_page` and `proxy_intercept_errors` over.

Through the API, sites take `error_pages`, `maintenance`, `robots` and
`favicon`, and routes `error_pages`; a client that leaves them out keeps the
current ones and `null` removes them. The site form edits pages with 404, 403,
502 and 503 to start from, the maintenance allowlist, robots.txt and the
favicon, and the sites list marks sites in maintenance.

```sh
ppanel site error-pages <site-id> --page '404 file=errors/404.html' \
  --page '502 503 "body=<h1>Back soon</h1>"' --intercept on
ppanel site maintenance <site-id> on --allow 10.0.0.0/8 --retry-after 600
ppanel site maintenance <site-id> off
ppanel site robots <site-id> disallow-all
ppanel site favicon <site-id> file shop/favicon.ico
ppanel route add <site-id> --match prefix:/api --proxy <upstream-id> \
  --error-page '404 body=missing' --intercept-errors
ppanel route error-pages <route-id> --inherit
```

## Directory listings, media types and cache headers

Static content lists directories that have no index file, maps media types
and sets `Cache-Control` on the files it serves
([decision](../docs/adr/0042-directory-listings-media-types-and-cache-headers.md)).

```nginx
server files {
    server_name files.example;
    root files autoindex=html;                       # or autoindex=json
    media_type application/wasm wasm;
    media_type text/markdown md markdown;
    default_type text/plain;                         # extensions nothing knows
    cache_control max_age=1y immutable for=css,js,woff2;
    cache_control no_cache for=html;
    cache_control max_age=1h;                        # every other file
}
```

A listing names a directory's entries other than hidden ones and links
leading out of the root, directories first, with sizes and modification
times; JSON listings are nginx's `autoindex_format json`. Index files still
come first, and a directory without either answers 404. A written media type
is sent as written ahead of the built-in guesses, which keep
`charset=utf-8` for text. For `Cache-Control`, the first rule naming a
file's extension applies, then the first rule for every file; 304 responses
carry the field too, and `Expires` is not sent. NGINX configurations
imported with `ppanel config import-nginx` carry `autoindex`,
`autoindex_format`, `types` and `default_type` of servers and locations, and
`expires` as a rule for every file.

Static actions take `listing`, `media_types`, `default_type` and `cache`
through the API; the site and route forms choose the listing, edit media
types and build cache rules from presets for hashed assets, pages and every
file.

```sh
ppanel site create --name files --domain files.example --static files \
  --autoindex json --media-type wasm=application/wasm --default-type text/plain \
  --cache-control 'max_age=1y immutable for=css,js' --cache-control 'no_cache for=html'
```

## Lua scripts

Sites, routes and upstreams run Lua in the gateway's request phases with
lua-nginx-module's directives and `ngx` API
([decision](../docs/adr/0039-lua-scripts.md)), on Luau VMs bounded in time,
work and memory.

```nginx
http {
    lua_shared_dict hits 10m;
    lua_time_limit 50ms;                   # wall-clock, waits included
    lua_allow upstream;                    # what scripts may do beyond reading and changing requests
    init_by_lua_block {
        LIMITS = { per_minute = 60 }       # read-only to requests
    }
    access_by_lua_file lua/auth.lua;       # inherited by every server and route

    upstream app {
        server 10.0.0.11:8080;
        server 10.0.0.12:8080;
        balancer_by_lua_file lua/pick.lua;
    }

    server shop {
        server_name shop.example;
        lua_on_error 503;                  # or fail (500), or continue
        header_filter_by_lua_block {
            ngx.header["X-Served-By"] = "shop"
        }
        proxy app;

        route hello {
            match exact /hello;
            lua_allow body network;        # bodies and TCP cosockets
            content_by_lua_block {
                local panel = require("panel.v1")
                panel.resp.send(200, "hello " .. (panel.req.query_value("who") or "world"))
            }
        }
    }
}
```

Handlers run in `server_rewrite`, `rewrite`, `access`, `precontent`,
`content` (the action of a server or route), `balancer`, `header_filter`, `body_filter`
and `log`; a route's handler replaces its server's and a server's the one
in `http`, as NGINX inherits them. `init_by_lua*` and `init_worker_by_lua*`
run once in each VM, and `exit_worker_by_lua*` once in each when a new
configuration replaces it. `ssl_client_hello_by_lua*` and
`ssl_certificate_by_lua*` in `http` or a server run as a TLS handshake
begins, for the server its server name selects: `ngx.ssl.clienthello`
reads the client's hello, `ngx.ssl` presents a certificate of the script's
in place of the TLS profile's or asks the client for one with
`verify_client`, whose result requests read as `$ssl_client_verify`,
`ngx.ocsp` checks and staples OCSP responses, and `ngx.exit(ngx.ERROR)`
ends the handshake. `ssl_session_fetch_by_lua*` and `ssl_session_store_by_lua*` in
`http` keep TLS sessions where scripts choose, with `ngx.ssl.session`, so
any listener, or another gateway, resumes them, and
`proxy_ssl_certificate_by_lua*` chooses the certificate a route's TLS
connections to its upstream present, and `proxy_ssl_verify_by_lua*`
accepts or refuses each new one by the certificate the upstream presented. The code of a
`*_by_lua_block` is read with Lua's lexical rules and kept as it is
written; `access_by_lua '...'` and the other forms that take code as a
string read as their blocks, with a warning. `lua off;` keeps every
script checked without running any.

`set_by_lua_block $name [argument ...] { ... }` and `set_by_lua_file $name
lua/<file>.lua [argument ...]` in a server or route set `$name` to what
their code returns before the block's rewrite handler, with the arguments
as `ngx.arg`. With Lua handlers in the configuration, `set` gives such a
variable too: scripts read and change it with `ngx.var`, and headers, log
fields, redirects and answers use the value it has when they are filled
in, written `${lua:name}`.

Scripts are files of the configuration: `*_by_lua_file` names a file under
`lua/`, and `require("a.b")` loads a built-in module (`cjson`, `bit`,
`resty.string`, `resty.sha256`, `resty.aes`, `resty.lrucache`, `ngx.re`, `ngx.balancer`,
`ngx.ssl`, `resty.core.base`, `resty.websocket.server`,
`resty.websocket.client`, `resty.lock`, `resty.limit.req`,
`resty.limit.conn`, `resty.limit.count`, `resty.limit.traffic`,
`tablepool`, `resty.http`, `resty.redis`, `resty.memcached`, `resty.mysql`,
`resty.dns.resolver`, `resty.upload`, `ngx.upstream`,
`resty.upstream.healthcheck` and others) or `lua/a/b.lua`;
`ngx.pipe`, `resty.shell`, `resty.signal` and `ffi` are refused, since they
would start or signal processes or run native code outside the sandbox. The WebSocket modules keep
lua-resty-websocket's API without its FFI, answer with a single
subprotocol and check the server's `Sec-WebSocket-Accept`, as RFC 6455
asks; a `resty.lock` lock that expired is never released or extended for
whoever took it since.
They are saved, compared and rolled back with every other
file, and each has the SHA-256 of its code as its version. `panel.v1` is
the gateway's own API next to `ngx`, with `req`, `resp`, `ctx`,
`upstream`, `log`, `json`, `re`, `time`, `random` and `crypto` functions
that mean one thing in every phase.

| Term | Default | Effect |
| :--- | :--- | :--- |
| `lua_time_limit` | `100ms` | wall-clock time of a run |
| `lua_work_limit` | `10000000` | function calls and loop iterations of a run |
| `lua_memory_limit` | `64m` | memory of each VM (`http` only) |
| `access_by_lua_no_postpone` | `off` | access handlers run before the security policies instead of after them (`http` only) |
| `lua_max_pending_timers` | `1024` | timers each VM holds waiting (`http` only) |
| `lua_max_running_timers` | `256` | timers each VM runs at once (`http` only) |
| `lua_regex_cache_max_entries` | `1024` | compiled `ngx.re` expressions each VM keeps, none at `0` (`http` only) |
| `lua_regex_match_limit` | PCRE2's | backtracking a match may do before it fails (`http` only) |
| `lua_worker_thread_vm_pool_size` | `10` | VMs `ngx.run_worker_thread` runs module functions on at once (`http` only) |
| `lua_capture_error_log` | none | what each VM's scripts log, kept up to this size for `ngx.errlog.get_logs` (`http` only) |
| `lua_allow` | `none` | `body`, `upstream` (balancers), `network` (cosockets) |
| `lua_on_error` | `fail` | what a failed run does |
| `lua_log_level` | `notice` | the least severe `ngx.log` messages kept |
| `lua_slow_threshold` | `10ms` | runs longer than this are logged and counted as slow |
| `lua_debug` | `off` | logs every run's start, end, duration and outcome |
| `lua_socket_connect_timeout`, `lua_socket_send_timeout`, `lua_socket_read_timeout` | `60s` | how long cosockets wait, unless a script sets its own |
| `lua_socket_buffer_size` | `16k` | how much a cosocket reads at a time |
| `lua_socket_pool_size`, `lua_socket_keepalive_timeout` | `30`, `60s` | the idle connections `setkeepalive` keeps, and for how long |
| `lua_socket_log_errors` | `on` | cosocket failures in the error log |
| `lua_transform_underscores_in_response_headers` | `on` | `ngx.header.x_trace_id` sets `X-Trace-Id` |
| `lua_use_default_type` | `on` | answers without a `Content-Type` get `text/plain; charset=utf-8` |
| `lua_need_request_body` | `off` | the request body is read before rewrite, access and content handlers run |
| `lua_check_client_abort` | `off` | rewrite, access and content handlers learn that the client left: `ngx.on_abort`'s function runs, or the run stops and the request is logged as 499 |
| `lua_ssl_trusted_certificate` | `system` | the authorities `sslhandshake` checks servers against: a secret of PEM certificates, or the system's trusted roots |
| `lua_ssl_crl` | none | a secret of PEM revocation lists of those authorities |
| `lua_ssl_certificate`, `lua_ssl_certificate_key` | none | secrets of the certificate chain cosockets present and its key |
| `lua_ssl_verify_depth` | not limited | the most intermediate certificates a server's chain may have |
| `lua_ssl_protocols` | `TLSv1.2 TLSv1.3` | the versions cosockets offer |
| `lua_ssl_ciphers` | every suite | the TLS 1.2 suites offered, by OpenSSL name; TLS 1.3's are always offered |

The sandbox has no `io`, `os.execute`, native libraries, FFI or bytecode
loading; globals are read-only and a request's writes stay with it. A run
that keeps the CPU for more than a millisecond yields its thread. A failed
run leaves the request as it was before it. TCP cosockets
(`ngx.socket.tcp`) verify certificates with the system's trusted roots,
or the authorities `lua_ssl_trusted_certificate` names, unless a script
passes `ssl_verify` false. Light threads
(`ngx.thread.spawn`) share the budget of the run that spawned them, which
ends once they have all ended. Timers (`ngx.timer.at`, `ngx.timer.every`)
run later under the limits and permissions of the run that created them;
when a new configuration takes over, pending ones run at once with
`premature` true. `ngx.socket.udp` sends datagrams under the same
permission, and `ngx.semaphore` coordinates the threads, timers and
requests of a VM. Output goes to the client as it is flushed, through the
header and body filters: `ngx.flush`, `ngx.eof` and every 64 KiB printed
send what is kept, so server-sent events and long responses reach clients
as handlers make them. Once the header went out, `ngx.req.socket(true)`
gives the client's connection to the script, for WebSocket and other
protocols a 101 response switches to. `ngx.req.socket()` streams the request body to a script,
chunked bodies included. `ngx.exec` handles the request again with a new URI, up
to ten URI changes as in nginx. `ngx.run_worker_thread` runs a module's
function on a thread and VM of its own, copying its arguments and results.
`ngx.location.capture` and `capture_multi` send subrequests through the
gateway to routes of the same site, skipping the access phase as nginx
does, and give their status, header and body back.

Reading the configuration compiles every script with the gateway's
compiler and reports, at their lines, scripts that do not compile, modules
`require` cannot load, `ngx` functions the gateway does not provide or the
phase does not allow, globals scripts write and Lua files nothing uses.
OpenResty directives that have nothing to tune here, such as
`lua_load_resty_core` or `lua_malloc_trim`, are read with a warning that
says why.

```sh
ppanel lua check conf/                      # the Lua diagnostics of local files
ppanel lua scripts --revision 12            # every script, where it runs and its version
ppanel lua modules                          # built-in modules, their libraries and the scripts that load them
ppanel lua test --host shop.example --target '/hello?who=lua' -H 'X-Key: k'
ppanel lua test --host shop.example --script lua/auth.lua --phase access --allow body
```

`GET /api/v1/config/lua` lists the scripts of the draft or of a revision,
and `POST /api/v1/config/lua/test` runs the handlers a described request
reaches, or one script, with the gateway's runtime and limits without
proxying anything, opening connections or running timers; tests are
recorded in the audit trail. Changing or
applying Lua takes the `config.lua` permission, which only Administrators
hold by default and which can be granted for site groups and sites;
approval policies can cover `lua` as a kind of their own. The gateway
exports `pingora_panel_gateway_lua_runs_total`,
`pingora_panel_gateway_lua_run_duration_seconds`,
`pingora_panel_gateway_lua_slow_runs_total` and
`pingora_panel_gateway_lua_memory_bytes`, which the traffic summary turns
into runs, failures by why, slow runs, run times and the handlers that
fail most. The console's Lua page lists, edits and tests scripts.

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

## Module-owned SQLite databases

Each module owns one SQLite file in the data directory
([decision](../docs/adr/0032-one-control-plane-process-on-sqlite.md)). The
directory and the files are created for the control plane's user alone, and
the write-ahead log takes the file's permissions. Files are opened with
`journal_mode=WAL`, `synchronous=FULL`, foreign keys and a busy timeout;
tables are `STRICT`. `ServiceDatabase::begin` starts a transaction that
writes with `BEGIN IMMEDIATE`, so a writer waits its turn for the file's lock
instead of failing when a read would have to become a write. No transaction
spans files, and no module writes another's.

`ServiceDatabase::migrate` applies platform migrations (versions below 10000)
and the module's own migrations (10000 and above) in one ordered history
stored in the file, so modules migrate independently. A published migration
is never edited, renamed or removed, and a new one sorts after every
published version in its directory: CI compares them with the baseline
commit, because a database that applied the old file would otherwise refuse
to start or apply changes out of order.

Producers call `SqliteOutbox::append` inside the transaction that changes
their state, so an event exists exactly when that change commits. Each row
stores the CloudEvents Protobuf form. One relay per file publishes in append
order; a module wakes it after a commit, and otherwise it looks every 250 ms.

`panel-jetstream` publishes relayed events to NATS JetStream and drives durable consumers.
Events a consumer cannot process are parked in a dead-letter stream, either by the handler's
decision, after the final failed attempt, or from the max-deliveries advisory. Replay
returns them to that consumer alone. See
[the delivery decision](../docs/adr/0006-jetstream-delivery-and-dead-letters.md).

Database tests open temporary files. Integration tests use disposable
servers named by `PANEL_TEST_NATS_URL` and `PANEL_TEST_ACME_*` and skip
without them; CI sets
`PANEL_REQUIRE_INTEGRATION_SERVICES` so a missing server fails instead. The
ACME server is Pebble, Let's Encrypt's test CA, with its DNS test server
resolving every name to 127.0.0.1; its release binaries are downloaded once
and checked against pinned digests. Locally:

```sh
panel/scripts/dev-services.sh up
eval "$(panel/scripts/dev-services.sh env)"
cargo test --manifest-path panel/Cargo.toml --package panel-jetstream \
  --package panel-control-runtime --package panel-acme --package automation-service \
  --package config-service --package audit-service --package panel-api-server --all-features
panel/scripts/dev-services.sh down
```

`PANEL_TEST_RFC2136_SERVER` additionally runs the RFC 2136 provider against a
real primary for `example.com`, as described in
`panel/dns-rfc2136/tests/primary.rs`.

## Compose installation

`deploy/compose.yaml` installs Pingora Panel on one Linux host with Docker
Compose or Podman Compose, from one image built by `deploy/Containerfile`
(distroless, non-root) and digest-pinned third-party images:

```bash
panel/deploy/generate-secrets.sh
docker compose -f panel/deploy/compose.yaml up -d
```

Every container uses the host network and binds loopback addresses, so the
console at <http://127.0.0.1:8080> and every internal port stay local until
the API authenticates callers. `pki-init` creates the certificate authority
and the credentials of every module and of `gatewayd`, `pki` renews them,
and `bootstrap` provisions the event streams and the service registry before
the `control` service starts `panel-control`; it reaches `gatewayd` over
mutual TLS. The control plane's SQLite files live in the `control-data`
volume. `control` mounts the five module credential volumes and `gatewayd`
its own, read-only, and every container runs with a read-only root file
system, no capabilities and `no-new-privileges`. The bootstrap token, the
password pepper and the master key that seals certificate keys are
generated into `deploy/secrets/` (never committed) and mounted as Compose
secrets; back the master key up with the `control-data` volume, since stored
private keys cannot be opened without it. The automation module writes
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
