<p align="center">
  <img src="panel/web/public/favicon.svg" alt="Pingora Panel logo" width="112">
</p>

<h1 align="center">Pingora Panel</h1>

<p align="center"><strong>A self-hosted control plane for websites served by a Pingora gateway: reviewed changes, atomic applies and a full audit trail.</strong></p>

<p align="center">
  <strong>
    <a href="#quick-start">Install</a> ·
    <a href="panel/README.md">Documentation</a> ·
    <a href="PRODUCT_SPEC.md">Product specification</a>
  </strong>
</p>

<p align="center">
  <a href="https://github.com/eltavine/pingora-panel/actions/workflows/panel.yml"><img src="https://img.shields.io/github/actions/workflow/status/eltavine/pingora-panel/panel.yml?branch=main&amp;style=flat-square&amp;label=build&amp;logo=githubactions&amp;logoColor=white" alt="Build workflow status"></a>
  <a href="https://github.com/eltavine/pingora-panel/actions/workflows/panel-deploy.yml"><img src="https://img.shields.io/github/actions/workflow/status/eltavine/pingora-panel/panel-deploy.yml?branch=main&amp;style=flat-square&amp;label=deploy&amp;logo=docker&amp;logoColor=white" alt="Deployment check status"></a>
  <a href="https://github.com/eltavine/pingora-panel/actions/workflows/audit.yml"><img src="https://img.shields.io/github/actions/workflow/status/eltavine/pingora-panel/audit.yml?branch=main&amp;style=flat-square&amp;label=audit&amp;logo=rust&amp;logoColor=white" alt="Security audit status"></a>
  <a href="#compatibility"><img src="https://img.shields.io/badge/Rust-1.94%2B-000000?style=flat-square&amp;logo=rust&amp;logoColor=white" alt="Rust 1.94 or later"></a>
  <a href="https://github.com/cloudflare/pingora"><img src="https://img.shields.io/badge/Pingora-0.9.0-F38020?style=flat-square&amp;logo=cloudflare&amp;logoColor=white" alt="Pingora 0.9.0"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/eltavine/pingora-panel?style=flat-square" alt="License"></a>
</p>

<p align="center">
  <a href="README.md">English</a> &nbsp; <a href="README_ZH.md">简体中文</a>
</p>

# Overview

Pingora Panel runs a gateway built on Cloudflare's [Pingora](https://github.com/cloudflare/pingora) on one host and manages it for a team. Sites and their domains, routes, upstreams, listeners, certificates, and security and HTTP policies are edited as a draft, checked, approved when a policy asks for it, and applied to the gateway atomically. Every apply is kept as a revision that can be compared and rolled back.

The web console, the `ppanel` command line and your own automation use the same REST API. Every request is authenticated and authorized, every change and refused attempt is recorded in a tamper-evident audit trail, and the gateway's traffic, logs and alerts sit next to its configuration. The draft is also a set of Nginx-style configuration files that can be checked, formatted, diffed and kept in version control.

# Quick start

1. **Prepare a Linux host** with Docker Engine 24 and Compose v2.20 or later, or Podman 4.4 or later with its socket enabled and the Compose CLI, and clone this repository. No release is published yet, so build the image from source:

   ```bash
   docker compose -f panel/deploy/compose.yaml build
   ```

2. **Check the host and install** with the host command. It generates the secrets into `panel/deploy/secrets/`, which is never committed: the one-time bootstrap token, the password pepper and the master key that seals certificate keys. `--agent` also installs the host agent, which manages the host and its containers.

   ```bash
   sudo panel/deploy/pingora-panel preflight
   sudo panel/deploy/pingora-panel install --agent directories,listeners
   ```

   Back the master key up together with the `control-data` volume; stored private keys cannot be opened without it.
3. **Open the console** at <http://127.0.0.1:8080>, from another machine through `ssh -L 8080:127.0.0.1:8080 <host>`. The first visit asks for the bootstrap token in `panel/deploy/secrets/bootstrap-token` and creates the first Administrator. Every container uses the host network and binds loopback addresses; the gateway listens where its listeners say.
4. **Serve a site** from the console, or from the command line with an API token created under your account in the console:

   ```bash
   export PPANEL_TOKEN=ppat_...
   ppanel() { docker compose -f panel/deploy/compose.yaml exec -T -e PPANEL_TOKEN control ppanel "$@"; }
   ppanel listener set public --address 0.0.0.0:80
   ppanel upstream create --name app --node 10.0.0.11:8080
   ppanel site create --name shop --domain shop.example --proxy <upstream-id>
   ppanel config apply --yes
   ```

5. **Upgrade, roll back and remove** with the same command and the API token in `PPANEL_TOKEN`: `upgrade --image <image>` keeps a backup on the host first and puts the previous release back when a step fails; `rollback`, `restore <archive>` on an empty host, `uninstall`, which keeps the data, and `uninstall --purge --yes`.

## How a change reaches the gateway

| Step | What happens |
| :--- | :--- |
| **Edit** | Changes go into a draft. A change that would leave the configuration invalid is refused as a whole. |
| **Check** | `ppanel config check` and the console show every problem where it was written; `ppanel config plan` shows what applying would change, resource by resource. |
| **Approve** | A change an approval policy covers waits until someone else approves it. |
| **Apply** | The draft compiles to a snapshot that the gateway prepares and then activates atomically, only if nothing else was applied meanwhile. A snapshot the gateway refuses changes nothing. |
| **Roll back** | Every apply is a revision you can compare, annotate and restore with `ppanel config rollback --to <revision>`. |

# Feature coverage

Current feature areas, each linked to the decision record that explains what it does, why, and what it leaves out:

[`Gateway`](./docs/adr/0010-pingora-data-plane.md) · [`Sites and apply`](./docs/adr/0011-configuration-model-and-apply.md) · [`Configuration language`](./docs/adr/0012-configuration-language-and-revisions.md) · [`Route conditions`](./docs/adr/0036-route-conditions.md) · [`HTTP policies`](./docs/adr/0037-http-policies.md) · [`Rewrites`](./docs/adr/0040-rewrites-and-internal-redirects.md) · [`Error pages`](./docs/adr/0041-error-pages-and-maintenance.md) · [`Static content`](./docs/adr/0042-directory-listings-media-types-and-cache-headers.md) · [`Proxy cache`](./docs/adr/0043-proxy-cache.md) · [`Plugins`](./docs/adr/0044-external-plugins-and-provider-ports.md) · [`Surface parity`](./docs/adr/0045-surface-parity.md) · [`Upstream resilience`](./docs/adr/0038-upstream-resilience-and-streams.md) · [`Security policies`](./docs/adr/0017-request-security-policies.md) · [`Certificates`](./docs/adr/0015-certificates-and-secret-material.md) · [`ACME`](./docs/adr/0016-acme-issuance-and-renewal.md) · [`Accounts and access`](./docs/adr/0014-identity-and-access.md) · [`Single sign-on`](./docs/adr/0018-identity-provider-sign-in.md) · [`Service accounts`](./docs/adr/0020-service-accounts-and-workload-identity.md) · [`Scoped grants`](./docs/adr/0021-scoped-and-conditional-grants.md) · [`Approvals`](./docs/adr/0019-change-approvals.md) · [`Audit trail`](./docs/adr/0013-audit-trail.md) · [`Metrics, logs and traces`](./docs/adr/0022-metrics-logs-and-traces.md) · [`Access logs`](./docs/adr/0025-access-and-error-logs.md) · [`Log search`](./docs/adr/0026-log-search-tail-and-deletion.md) · [`Alerts`](./docs/adr/0027-alerts.md) · [`Host and containers`](./docs/adr/0028-host-and-container-operations.md) · [`Host agent`](./docs/adr/0030-ops-agent.md) · [`Containers`](./docs/adr/0031-containers.md) · [`Sites for containers`](./docs/adr/0033-sites-in-front-of-containers.md) · [`Site files`](./docs/adr/0034-site-files.md) · [`Backups`](./docs/adr/0035-backups.md) · [`Supply chain`](./docs/adr/0046-supply-chain-evidence.md) · [`Installation lifecycle`](./docs/adr/0047-installation-lifecycle.md) · [`Benchmarks`](./docs/adr/0024-gateway-benchmarks.md)

[`panel/README.md`](./panel/README.md) documents each area's API, command line and configuration language, and [`PRODUCT_SPEC.md`](./PRODUCT_SPEC.md) lists every catalogued feature with its status.

# How it works

- **Gateway:** `gatewayd` runs Pingora 0.9 with an adapter that turns engine-neutral configuration snapshots into listeners, virtual hosts, routes, TLS, static content and upstream pools. A snapshot is prepared first and then activated atomically; the last good one is kept on disk and served again after a restart, and reloads and worker changes do not drop connections.
- **Contracts:** the control plane reaches the gateway over gRPC with proto3 contracts and mutual TLS, issued and renewed by an internal certificate authority. Snapshots name the capabilities they need, and settings a gateway does not know are refused rather than ignored.
- **Control plane:** `panel-control` runs the API, configuration, automation, observability, audit and plugins modules in one process. Each keeps its own SQLite database and publishes CloudEvents through a transactional outbox to NATS JetStream.
- **Plugins:** signed plugins run as child processes under resource limits and speak gRPC over Unix sockets with HashiCorp go-plugin's protocol. They are granted nothing until an administrator grants them DNS-01, secret, notification, backup target, container engine or gateway engine ports, and every call carries a deadline.
- **One API:** the REST API is described by OpenAPI and checked for breaking changes on every commit; the console's client is generated from it, and CI checks that `ppanel` and the console offer every operation it describes.
- **Configuration as text:** the draft is an Nginx-style configuration language rooted at `main.conf`, with checks, formatting, completion and a plan of what applying would change. Existing Nginx configuration can be converted into it.
- **Observability:** the gateway exposes Prometheus metrics and writes structured access logs, which the installation's collector ships to Loki; W3C Trace Context passes through to upstreams and its trace ID is logged with each request. The console charts traffic, searches and tails logs, and alerts notify signed webhooks.

# Compatibility

| Area | Details |
| :--- | :--- |
| **Host** | Linux with systemd and cgroup v2, and Docker Engine 24 with Compose v2.20, or Podman 4.4 through its socket. The image is built and every installation step, from install to recovery on an empty host, is checked in CI on Linux x86_64 with both engines. |
| **Protocols** | HTTP/1.1 and HTTP/2, over TLS by ALPN or as h2c on plaintext listeners, with WebSocket upgrades, Server-Sent Events and gRPC. HTTP/3 is reserved and refused. |
| **TLS** | rustls with TLS 1.2 and 1.3 and a certificate per host by SNI, plus HSTS. Certificates are uploaded or issued through ACME (RFC 8555) with HTTP-01, or DNS-01 through RFC 2136 or a plugin, external account binding included. |
| **Console** | Current desktop and mobile browsers; end-to-end tests run in Chromium and WebKit. English and Simplified Chinese, light and dark themes. |
| **Toolchains** | Rust 1.94 or later; Node.js 22.18 or later, or 24.12 or later, with pnpm 11 for the console. |
| **Pingora** | 0.9.0, kept in this repository. A scheduled job builds the adapter against Pingora's main branch to catch upcoming changes. |

# Architecture

```text
pingora-panel/
├─ pingora-*/, tinyufo/   # Pingora 0.9.0 crates, with any local change recorded
├─ panel/                 # The Pingora Panel workspace
│  ├─ proto/              # gRPC and event contracts, proto3, checked by Buf
│  ├─ panel-domain/       # Domain values; with panel-ir and panel-errors, the stable core
│  ├─ panel-engine/       # Gateway ports, IR validation and an in-memory engine
│  ├─ gateway-pingora/    # The Pingora data plane adapter
│  ├─ gatewayd/           # The gateway process
│  ├─ panel-control/      # The control plane process and its *-service modules
│  ├─ panel-api/          # The REST API and its OpenAPI document
│  ├─ panel-config-*/     # Configuration model, language and codecs
│  ├─ panel-cli/          # The ppanel command line
│  ├─ ops-agent/          # The optional host agent
│  ├─ web/                # The console: Vue, shadcn-vue and a generated API client
│  └─ deploy/             # Container image and Compose installation
├─ docs/adr/              # Architecture decision records
├─ docs/                  # Pingora's guides, the gateway runbook and local patches
├─ .github/               # Workflows, policies and repository guards
└─ PRODUCT_SPEC.md        # Product scope, feature catalogue, roadmap and quality gates
```

Ports such as `GatewayEngine`, `DataPlaneAdapter` and `SnapshotStore` live in core crates. Pingora, storage, gRPC and HTTP live in leaf adapter crates, and `gatewayd` and `panel-control` only compose them; generated protobuf and Pingora types never cross a stable port. CI enforces the dependency direction, so another engine or store is a new leaf crate rather than a change to the core. See [crate dependency direction](./panel/README.md#crate-dependency-direction) and [extension rules](./panel/README.md#extension-rules).

Console features register their own routes and navigation and may not import each other; what they share lives in `src/lib` and `src/components`. See [`panel/web/README.md`](./panel/web/README.md).

# Build

## Requirements

- Rust 1.94 or later; CI also builds with 1.98 and nightly
- Node.js 22.18 or later, or 24.12 or later, and pnpm 11.22 for the console
- Docker or Podman for the image
- Optionally a NATS server with JetStream for the tests that start the whole control plane

`protoc` is vendored, and generated code is written at build time rather than committed.

## Commands

```bash
# Gateway, control plane and command line
cargo build --manifest-path panel/Cargo.toml --workspace --locked

# Console, served by panel-api from PINGORA_PANEL_WEB_ROOT
cd panel/web && pnpm install --frozen-lockfile && pnpm build

# Image
docker compose -f panel/deploy/compose.yaml build
```

For the standard local validation path:

```bash
cargo fmt --manifest-path panel/Cargo.toml --all --check
cargo clippy --manifest-path panel/Cargo.toml --workspace --all-targets -- -D warnings
PANEL_TEST_NATS_URL=nats://127.0.0.1:4222 cargo test --manifest-path panel/Cargo.toml --workspace --locked
for s in .github/scripts/check-panel-*.py; do python3 "$s"; done
bash .github/scripts/check-panel-boundaries.sh
(cd panel/web && pnpm type-check && pnpm lint && pnpm test:unit --run && pnpm test:e2e)
```

Without `PANEL_TEST_NATS_URL`, the tests that need the event broker are skipped. Before contributing, read the [extension rules](./panel/README.md#extension-rules) and the [decision records](./docs/adr/).

# Security and limitations

- Management interfaces bind loopback addresses. The console and the API require sign-in; the first Administrator is created with the one-time bootstrap token. To reach the console from elsewhere, serve it through the gateway as an HTTPS site or tunnel to it.
- Secrets are generated on the host, mounted as Compose secrets and never returned by the API. The master key seals stored private keys.
- The control plane connects only to what you configure: ACME directories, DNS servers for DNS-01, OpenID Connect providers and alert webhooks. It reports no usage anywhere.
- One installation runs one gateway. Rate limits, retry budgets, circuits and queues count per gateway process.
- HTTP/3 is not available yet.
- Lua scripts run with the time, work and memory limits and the permissions their configuration grants, and only Administrators may change them by default; the sandbox is a defense in depth, not a boundary between untrusted tenants.

# Project status

Pingora Panel is under active development and has not reached 1.0. [`PRODUCT_SPEC.md`](./PRODUCT_SPEC.md) catalogues every planned feature with its status, the roadmap and the quality gates for 1.0.

<p align="center">
  <a href="https://www.star-history.com/#eltavine/pingora-panel&amp;Date">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=eltavine/pingora-panel&amp;type=Date&amp;theme=dark" />
      <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=eltavine/pingora-panel&amp;type=Date" />
      <img alt="Star history chart" src="https://api.star-history.com/svg?repos=eltavine/pingora-panel&amp;type=Date" />
    </picture>
  </a>
</p>

# Disclaimer

This software is provided "as is", without warranty of any kind. Operators are responsible for what they expose, for their certificates, secrets and backups, and for testing changes before applying them to production traffic. The developers are not liable for damage, data loss or outages resulting from its use.

# License

Pingora Panel is licensed under the [Apache License 2.0](./LICENSE).

It is built on [Pingora](https://github.com/cloudflare/pingora) by Cloudflare, also licensed under the Apache License 2.0. The Pingora crates in this repository keep their original copyright and license notices, and any local change to them is recorded in [`docs/upstream-patches.md`](./docs/upstream-patches.md). Pingora Panel is not affiliated with or endorsed by Cloudflare.
