# 0030: ops-agent

Status: accepted. Refines how [ADR 0028](0028-host-and-container-operations.md)
reaches the agent.

## Context

ADR 0028 puts host actions and container management in `ops-agent`, a
process on the host outside the containers. The product specification
adds how it is reached and what it may do: a native systemd service that
exposes no TCP port by default, takes versioned gRPC over a Unix domain
socket with restricted permissions, checks each caller's identity, the
operation, its resource scope and the request's signature, refuses any
shell, any path and any Docker API passthrough, and is the only process
that sees the Docker or Podman socket. An unknown operation or a path out
of scope is refused and recorded as a security event.

Every Panel container runs as user 65532, so the peer credentials of a
socket cannot tell `panel-api` from another service.

## Decision

**Process.** `ops-agent` is a native binary that systemd runs as
`pingora-panel-ops-agent.service`, as a dynamic user named
`pingora-panel-ops`, in a sandbox: a read-only system, no home
directories, a private `/tmp`, no new privileges and Unix sockets only. A
capability brings its own privileges through a drop-in that the operator
installs to enable it:

| Capability | Privilege |
|---|---|
| Directory sizes | Read access to the configured directories |
| Port diagnostics | `CAP_DAC_READ_SEARCH` and `CAP_SYS_PTRACE`, to read other processes' descriptors |
| Containers | Membership of the group that owns the Docker or Podman socket |
| Gateway service | The containers capability's engine; of the installation's containers, the agent stops and restarts the gateway's alone |

The agent reports which capabilities it has and why any is missing, so the
API and the console offer only those.

**Transport.** The agent serves gRPC over mutual TLS on a Unix domain
socket, `/run/pingora-panel-ops/agent.sock`, mode 0660, group 65532, and
opens no TCP listener. Only the control plane's container mounts the
socket's directory. Before a handshake the agent checks the peer's user against
an allow list, 65532 by default. The handshake then authenticates the
caller's workload identity from the installation's authority, and each
gRPC service admits named identities only: `panel-api` for host and
container operations. TLS 1.3's `CertificateVerify` is the caller's
signature over the handshake, and each request travels under keys bound
to it; that is how requests are signed, without a second scheme.

**Credentials.** The installation's authority issues the agent's
credentials into a host directory as it does for the services. systemd
hands both files to the dynamic user with `LoadCredential=`, and a path
unit restarts the agent when they are renewed. Compose issues them only
with the agent's override file, so installations without the agent are
unchanged.

**Operations.** The gRPC API is the allow list. Each method is one
operation with typed arguments: no command, no file path, no Engine API
passthrough. Resources are bounded by the agent's configuration, which
covers the gateway's Compose service, the configured directories and the
engine's socket; containers, images, networks and volumes are named by identifier.
Unknown methods fail as unimplemented. Refused peers, identities and
arguments go to the journal as security events. `panel-api` records every
operation it relays, refused or not, in the audit trail.

**Libraries.**

| Need | Choice | Instead of |
|---|---|---|
| Listening sockets and their processes | `procfs`: typed parsers for `/proc/net/tcp*` and process descriptors, MIT or Apache-2.0 | Parsing `/proc` by hand; `ss` (text output) |
| Directory sizes | `walkdir`, staying on one file system and not following links, MIT or Unlicense | A recursive walk of our own |
| Docker and Podman | `bollard`, as ADR 0028 decided; Podman serves the same Engine API | The `docker` command |

All three are maintained, widely used, and contained behind the agent's own
gRPC types, so replacing one changes no contract.

**Gateway service.** The installation runs the gateway as a container, so
the gateway's service is that container: the one in the installation's
Compose project labelled with the gateway's Compose service, found on an
enabled engine. The agent reports its state, health, start and stop times,
exit code and restarts, and starts, stops or restarts it through the
Engine API. The contract names what supervises the gateway, so a native
installation's systemd unit could be added beside the container without
changing what callers already read.

**Tests.** Port diagnostics are tested against `/proc` fixtures, and
containers and the gateway's service against a fake Engine API served on
a Unix socket. No test needs host privileges.

## Alternatives

- Mutual TLS over loopback TCP, as for the services: the specification
  exposes no TCP by default.
- Peer credentials alone: every container shares one user.
- A request signature of our own on top of TLS would add keys and a replay
  window and prove nothing the session does not.
- Running the agent as root would turn any flaw in it into a host
  compromise.
- Managing the gateway as a systemd unit over D-Bus, with a polkit rule for
  the agent: the shipped installation runs the gateway as a container, so
  there is no unit to manage, and the agent would carry a D-Bus client for
  nothing.

## Consequences

- Port diagnostics are Linux only; directory sizes, containers and the
  gateway's service work on any Unix.
- Renewing the credentials restarts the agent, which keeps no state.
- The agent checks again what `panel-api` already authorized; it is a
  second barrier, not the first.
