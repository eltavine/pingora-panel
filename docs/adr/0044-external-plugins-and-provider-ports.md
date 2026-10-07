# 0044: External plugins and provider ports

Status: accepted. Builds on [ADR 0010](0010-pingora-data-plane.md),
[ADR 0013](0013-audit-trail.md),
[ADR 0015](0015-certificates-and-secret-material.md),
[ADR 0016](0016-acme-issuance-and-renewal.md),
[ADR 0027](0027-alerts.md), [ADR 0031](0031-containers.md),
[ADR 0032](0032-one-control-plane-process-on-sqlite.md) and
[ADR 0035](0035-backups.md).

## Context

Operators need DNS providers, secret stores, notification services, backup
storage, container engines and gateway engines the product does not ship.
The specification requires third-party plugins to run in processes of their
own and to register a manifest, capabilities, health and resource needs over
versioned gRPC; to hold no permission until an administrator grants it; to
receive a deadline with every call and never take the caller down with
them; and to go from discovered to validated to enabled, and to degraded or
disabled, with signatures, version checks, a configuration schema, health
checks, upgrades, rollbacks and an audit trail. Rust `cdylib` libraries are
ruled out: Rust has no stable ABI, and a crash in one would end the host.

## Decision

**Host.** A control-plane module, `plugins`, hosts every plugin (ADR 0032).
It owns `plugins.db`, runs each enabled plugin as a child process, and is
the only component that talks to plugins: other modules reach a plugin
through the host's gRPC services, so grants, deadlines, health and audit are
applied in one place.

**Protocol.** Plugins speak the process protocol of HashiCorp `go-plugin`,
so they can be written with `go-plugin` in Go or with any gRPC library. The
host starts the executable with a clean environment holding only the
protocol's variables: the magic cookie `PINGORA_PANEL_PLUGIN`, the
application protocol versions it accepts in `PLUGIN_PROTOCOL_VERSIONS`, and
in `PLUGIN_UNIX_SOCKET_DIR` a directory only the host's user can enter. The
plugin answers with one line on standard output,
`1|<version>|unix|<socket>|grpc`, and serves on that socket the gRPC Health
Checking Protocol for the service `plugin`, `pingora.panel.plugin.v1.Plugin`
and the services of the ports it provides. The host accepts only Unix
sockets, so nothing listens on the network; standard error goes to the
host's log. Application protocol version 1 is this decision; services are
versioned by package (`plugin.v1`) and extended only by adding to them.

**Packages and discovery.** A plugin version is a directory,
`<plugins>/<name>/<version>/`, holding `plugin.json`, its executable and
`plugin.json.minisig`. The manifest is `pingora.panel.plugin.v1.Manifest` in
the Proto3 JSON mapping: a lowercase name, a SemVer 2.0.0 version, the
publisher, a description, the executable's relative path and SHA-256, the
application protocol versions it speaks, the ports it provides, the
capabilities it asks for, its resource needs, its call timeout and the JSON
Schema of its configuration. Discovery reads the directory when the host
starts and whenever an administrator asks; each version found is recorded
as discovered. A version becomes validated when its manifest is well formed,
its executable has the named digest, a minisign signature (Ed25519) over
`plugin.json` verifies with a trusted publisher key, it shares an
application protocol version with the host, and every port it names is one
the host knows. Trusted keys are administered like any resource and can be
seeded at installation; unsigned plugins are refused. A version that fails
validation keeps its reasons and cannot be enabled.

**Capabilities.** Each port a plugin provides is a capability it asks for,
and so is `secret-references`, receiving the values its configuration
references.
Plugins are granted nothing until an administrator grants capabilities one
by one, among those the manifest asks for; the host routes a port's calls
only to a plugin granted that port, and resolves secrets only for a plugin
granted `secret-references`. A plugin cannot be enabled without a grant.

**Configuration.** A plugin's settings are a JSON document validated
against its manifest's JSON Schema (draft 2020-12) before they are stored. A
string whose schema says `"format": "secret-reference"` names a secret
instead of holding it: `vault:<name>` for one sealed in the host's store
(ADR 0015), or `<plugin>:<path>` for one a secret-provider plugin returns.
References are resolved when the plugin starts or is reconfigured and the
values go only to the plugin, over its socket; the API never returns them
and the audit trail never records them. A configuration the plugin refuses
is not kept.

**Running.** On Linux the host bounds the plugin's address space, CPU time
and open files and disables core dumps as the process starts (`prlimit`),
within what the manifest asks and the administrator allows; elsewhere the
limits are reported as not enforced. The host asks for the plugin's
manifest over gRPC and refuses a process whose name, version or ports differ
from the signed file. It checks health every 10 seconds; three failed
checks in a row, or the process exiting, make the plugin degraded, and the
host restarts it with a backoff from one second to a minute. Every call
through the host carries a deadline, the caller's or the plugin's call
timeout, whichever is sooner (10 seconds unless set, at most 60); a
streaming call, such as an archive's transfer, has the caller's deadline,
and each of its messages the call timeout to arrive. At most 16 calls are in
flight per plugin unless set; calls beyond them, and calls to a degraded
plugin, fail at once. Disabling a plugin stops it with
`SIGTERM` and, after five seconds, `SIGKILL`.

**Versions.** Installing a version beside another adds a directory, which
discovery finds. Upgrading starts the chosen validated version with the
current configuration and grants, and switches to it once it is healthy,
stopping the previous one; if it does not become healthy the previous
version keeps running and nothing changes. Rolling back does the same with
the version that ran before. The host records which version is active and
which ran before it.

**Ports.**

| Port | Service | Used by |
|---|---|---|
| DNS-01 | `plugin.v1.Dns01Provider`: add and remove TXT records | ACME DNS providers of kind `plugin` (ADR 0016) |
| Secrets | `plugin.v1.SecretProvider`: resolve a path | `<plugin>:<path>` secret references |
| Notifications | `plugin.v1.NotificationProvider`: deliver an alert | Alert channels of kind `plugin` (ADR 0027) |
| Backup targets | `plugin.v1.BackupTarget`: store, list, fetch and delete archives | Backups copied to a target and restored from it (ADR 0035) |
| Container engines | `ops.v1.Containers`, the agent's own contract | Engines named `plugin.<name>` beside the agent's (ADR 0031) |
| Gateway engines | `gateway.v1.GatewayEngine` and `gateway.v1.GatewayRuntime`, the gateway's own contracts | A gateway engine the configuration is applied to (ADR 0010) |

A consumer names the plugin and calls the port through the host; nothing
else in the consumer changes. Where the product already has a gRPC contract
for a port, the plugin implements that contract, so the consumer's client
is the same; a plugin answers `UNIMPLEMENTED` for operations it does not
offer, which the consumer reports as unsupported.

**Access and audit.** Viewing plugins needs `plugins.read` and changing them
`plugins.manage`, which only administrators hold. Plugins never reach the
control plane's databases, the container engines' sockets, systemd or the
gateway except through ports; they run as the control plane's user with
nothing but their own directory. Every change — discovery, validation,
grants, configuration, enabling, disabling, upgrades, rollbacks, trusted
keys — every refusal and every move between healthy and degraded is an event
of the `plugins` module in the audit trail.

**Surfaces.** The REST API, `ppanel plugin` and the console's plugin pages
list plugins with their versions, signatures, compatibility, grants,
configuration, health and limits, and perform every change; the Compose
installation mounts the plugins directory and seeds trusted keys.

## Alternatives

- WebAssembly components (WASI 0.2 under Wasmtime): the strongest sandbox
  and capability model, but DNS, secret, notification and storage plugins
  need sockets and TLS that WASI provides only partly today. An engine for
  components can host them later behind the same ports.
- Plugins as containers managed by `ops-agent`: heavier, and dependent on a
  container engine the product does not require.
- Sigstore signatures: keyless verification needs Fulcio and Rekor online,
  or a pinned trust root; minisign keys work offline. Sigstore bundles can
  be accepted beside them later.
- Each module calling its plugins directly: every module would repeat
  deadlines, grants, health and audit.

## Consequences

- A plugin that crashes or hangs fails only its own calls, and only until
  its deadline.
- Plugins are trusted code: the signature says whose, the grants say what
  they are called for, and the limits bound their resources, but they run
  with the control plane's user.
- Resource limits are enforced on Linux, the supported platform, only.
