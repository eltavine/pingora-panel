# 0047: Installation lifecycle

Status: accepted. Builds on [ADR 0030](0030-ops-agent.md),
[ADR 0032](0032-one-control-plane-process-on-sqlite.md),
[ADR 0035](0035-backups.md) and [ADR 0046](0046-supply-chain-evidence.md).

## Context

An installation is started by hand: secrets are generated, an image is built
or pulled, the host agent is installed and Compose brings the services up.
Nothing checks the host first, upgrades and rollbacks are left to the
operator, which matters because the control plane, the gateway and the agent
speak versioned protocols and the modules migrate their databases, and
nothing removes an installation. The specification asks for Docker and Podman
Compose installations that behave alike, a preflight, a manifest of what is
deployed, migrations that expand before they contract, a backup before every
upgrade, an upgrade order that keeps every peer compatible, a fast return to
the previous image, a recovery drill on an empty host, a redacted diagnostic
bundle, and uninstalls that keep or remove the data.

## Decision

**One host command.** `panel/deploy/pingora-panel` installs, checks,
upgrades, rolls back, restores and uninstalls, on Docker or Podman, detected
or named with `--engine`. Podman runs the same Compose file through its
socket; CI runs every lifecycle step on both engines. What the command needs
to remember, the engine, the project, the images in use and the previous
ones, it keeps in `/etc/pingora-panel/installation.env`. Each run writes the
deployed images, with their digests, to a manifest the control plane mounts
read-only.

**Preflight.** `pingora-panel preflight` reads and changes nothing: a Linux
host with systemd and cgroup v2, an engine and Compose new enough, the ports
the listeners bind free, memory, space for the engine's images, the data and
a backup, and the installation's secrets present and well formed. Before an
upgrade it also runs the new image's `panel-control preflight` against the
live data, which reports the schema each module would migrate from and to,
refusing one newer than the release reaches, and whether the new release's
protocol revisions overlap those the running peers serve.
`GET /api/v1/system/preflight` reports the installation's own readiness to
upgrade: every module ready, no snapshot prepared or activation unsettled,
the age of the newest backup and the space left for another.

**Versions.** `GET /api/v1/system/versions` reports what runs: the release
and commit, each module's version and protocol revisions, each database's
schema, the IR and language versions, the gateway's and the engine's
versions, the agent's when it answers, and the deployed images with their
digests from the manifest, with when it was read.

**Migrations.** A migration may expand a schema at any time; removing or
renaming a table or column, narrowing a type, or requiring a value of
existing rows contracts it, and is allowed only once a previous release
stopped using what it removes, recorded in
`.github/policies/migration-contractions.json`. CI applies every module's
migrations to an empty database and compares the schema after each file with
the one before, so a table rebuilt with all its columns passes and a dropped
column without its record fails. One release back is therefore always able
to run on the newer schema.

**Upgrade.** `pingora-panel upgrade <version>` runs the preflight, takes a
backup of every database, the configuration and the sites through the API
and keeps it on the host after checking its digest, pulls and verifies the
new images, and switches servers before their clients: the agent, then the
gateway, then the control plane, each waiting for readiness and the next
peer's protocol revisions before going on. A step that fails puts back what
it switched.

**Rollback.** `pingora-panel rollback` returns to the images and agent
binary in use before the last upgrade, clients first. The databases stay as
they are, since one release back runs on the expanded schema; an upgrade
whose preflight reported a contraction can only be rolled back with its
backup, `rollback --restore`.

**Recovery.** `pingora-panel restore <archive>` installs on an empty host with
the installation's secrets, restores the databases with `panel-control
restore` before the first start, starts, and verifies the active revision and
its hash against the archive's manifest, the certificates the TLS profiles
name, the audit chain and the protocol revisions each module serves. CI runs
the drill: purge, install afresh, restore, verify.

**Diagnostics.** `GET /api/v1/system/diagnostics` answers one JSON document,
with when it was generated and the versions: readiness, the gateway's state,
recent failures of jobs, activations and alerts, the host when the agent
answers, the configuration's counts and the latest audit events. Every field
passes redaction first: values under names of secrets, tokens, passwords,
keys, cookies or authorization are replaced, and PEM blocks and token-shaped
values masked wherever they appear. It needs `platform.diagnose`, which only
Administrators hold.

**Uninstall.** `pingora-panel uninstall` removes the containers, networks
and the agent's service and keeps the volumes, secrets and host backups, so
installing again resumes. `uninstall --purge --yes` also removes the
volumes, images, the agent's binary, account and directories, the secrets
and `/etc/pingora-panel`.

## Alternatives

- An installer binary: it must be distributed before it can install
  anything, while bash is on every supported host.
- Kubernetes operators or Helm: the product is a single host.
- Ansible: another dependency on the operator's machine.
- Automatic image updaters: they neither order the switch nor take a backup.

## Consequences

- Upgrades and rollbacks are one command that leaves a backup on the host.
- A contraction needs two releases, which keeps rolling back one release
  possible without restoring data.
- Uninstalling never removes data unless asked to by name.
