# 0031: Containers

Status: accepted. Builds on [ADR 0028](0028-host-and-container-operations.md)
and [ADR 0030](0030-ops-agent.md).

## Context

Many operators run their upstreams as Docker or Podman containers and want
the panel to show them, act on them, read their logs and put a site in front
of a container's port. ADR 0028 gives that to `ops-agent` through the Docker
Engine API with `bollard`; ADR 0030 says how the agent is reached and that it
refuses any path, shell or API passthrough. Access to an engine's socket is
root on the host, so the agent, not a container, holds it.

The specification lists the minimum role of each container feature. A few
of those minimums disagree with what the data holds: logs are an operator's
but following them is a viewer's, and a Compose file, which often holds
passwords, is a viewer's.

## Decision

**Engines.** The agent's configuration names the candidate sockets: Docker's
and Podman's, which serves the same Engine API. Each candidate the agent can
reach is an engine, addressed as `docker` or `podman`, and the agent manages
every engine an operator has enabled. Enabling or disabling an engine is the
socket configuration the panel offers; the agent keeps the choice in its
state directory. No request names a socket.

**What the agent does.** Containers are listed, filtered by state and
searched by name and image, with their ports, labels and the sites that
point at them; they are started, stopped, restarted, killed and removed;
their logs are read or followed; their statistics are read once. Images are
listed, inspected, pulled and removed; networks and volumes are listed.
Compose projects are the ones Compose created, known by their containers'
labels: up starts their containers, down stops and removes them with their
networks and keeps their volumes, restart restarts them, and their logs
merge their containers'. Recreating a project from a changed file stays with
Compose. Disk use is read, and pruning shows what it would remove before it
does, then removes only what it showed.

**What it never returns.** A container's details give the names of its
environment variables, not their values. A Compose file is read only when a
label of the project names it, it lies in the project's working directory,
ends in `.yml` or `.yaml`, is a regular file and is at most 256 KiB.

**Permissions.**

| Permission | Holders | Allows |
|---|---|---|
| `containers.read` | Viewer, operator, auditor | Engines, containers, images, networks, volumes and projects; ports, labels, statistics, disk use, prune previews and site links |
| `containers.inspect` | Operator | Logs, followed or not, a container's details and Compose files |
| `containers.manage` | Operator | Every change: engines, containers, images, projects, pruning and sites from containers |

Following logs and reading Compose files take `containers.inspect` though
the specification asks only for a viewer; their content holds secrets as
much as logs do. CPU and memory statistics take `containers.read` though it
asks for an operator; they hold none.

**Audit.** `panel-api` records each change it relays, refused or not, as a
`container.*` event; this is AUDIT-006.

## Alternatives

- The `docker` command or the Compose CLI: text output that is not an
  interface, and a shell the agent must not have.
- One engine at a time, chosen by a socket path the operator types: a path
  from a request is what ADR 0030 refuses.
- Showing environment values in a container's details, as `docker inspect`
  does: they are where containers keep their secrets.

## Consequences

- A Compose project changed on disk is recreated with Compose, not from the
  panel; the console says so.
- An engine the agent cannot reach, such as rootless Podman under another
  user, is not offered.
- Granting the agent an engine's socket grants it root on the host; the
  containers drop-in says so, and the console where engines are enabled.
