# 0028: Host and container operations

Status: accepted. [ADR 0030](0030-ops-agent.md) refines how the agent is
reached.

## Context

Operators want to see the host the gateway runs on — CPU, memory, disks,
load, network traffic, the operating system, its clock — to be warned
before a disk fills, to find what holds ports 80 and 443, to start, stop
and restart the gateway's service, and to see how much space its
configuration, logs and certificates take. Many also run their upstreams
as Docker containers and want to see, start, stop and inspect them, read
their logs and put a site in front of a container's port.

Every control service runs in a container without capabilities and binds
loopback addresses. Reading another process's sockets, talking to systemd
or to the Docker socket takes privileges that would be dangerous in any
of them: the Docker socket alone is equivalent to root on the host.

The figures already have a widely deployed source:
[Prometheus's node exporter](https://github.com/prometheus/node_exporter)
publishes CPU, memory, filesystems, load, network devices, the kernel and
operating system release, the host name and the clock, and Prometheus
keeps their history. Docker's [Engine API](https://docs.docker.com/reference/api/engine/)
is the stable way to manage containers, and systemd exposes units over
[D-Bus](https://www.freedesktop.org/wiki/Software/systemd/dbus/).

## Decision

**Figures come from the node exporter.** The Compose installation runs
it bound to loopback with the host's root mounted read-only, and
Prometheus scrapes it like the gateway. `observability-service` answers
host queries with fixed PromQL over its metric names, as it does traffic
(ADR 0022): CPU use, memory, each real filesystem's size and free space,
load averages, each physical device's traffic, the kernel and operating
system release, the host name and the clock with its time zone. A
filesystem fuller than a threshold is a warning, and disk use is an alert
measure (ADR 0027).

**Actions and what exporters do not show come from `ops-agent`.** It is
installed on the host, outside the containers, and serves gRPC over the
same mutual TLS as the services, to `panel-api` only. Each capability is
enabled on its own, with only the privileges it needs: reading
`/proc` to name the processes listening on a port; the gateway's
container, through the engine, as the one container of the installation
it may stop or restart; the sizes
of the panel's configuration, log and certificate directories; and,
only where the operator enables it, the Docker socket. It reports which
capabilities it has, so the console offers only those. `panel-api`
records every action it relays in the audit trail.

**Containers use the Engine API** through `bollard`, a maintained Rust
client, never the `docker` command. Listing, inspecting, logs and
statistics read; starting, stopping, restarting, removing, pulling and
pruning change, and pruning shows what it would remove before it does.
A container's published port can become a site's upstream in one step.

## Alternatives

- An agent of our own for host figures would duplicate the node exporter
  without its coverage, tests or history in Prometheus.
- Giving a control service the Docker socket or host PID namespace would
  make every flaw in it a host compromise.
- Shelling out to `docker`, `systemctl` or `ss` would tie the panel to
  their text output, which is not an interface.

## Consequences

- Host figures work wherever the node exporter runs, including hosts the
  agent does not run on.
- Without `ops-agent`, the panel shows figures but offers no host or
  container actions.
- Enabling the Docker capability grants the agent root on the host; the
  console says so where it is enabled.
