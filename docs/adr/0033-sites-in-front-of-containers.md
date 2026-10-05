# 0033: Sites in front of containers

Status: accepted. Builds on [ADR 0011](0011-configuration-model-and-apply.md),
[ADR 0021](0021-scoped-and-conditional-grants.md) and
[ADR 0031](0031-containers.md).

## Context

An operator who runs an upstream as a container wants to put a site in front
of one of its ports without copying addresses by hand, to see which sites
point at which containers, and to let a container say which site it backs.
Proxies built for containers, such as Traefik, Caddy's Docker plugin and
nginx-proxy, watch the engine and change their routes as containers start and
stop. The panel changes what the gateway serves only through the draft and
applying it (ADR 0011): validated, reviewed when a policy asks (ADR 0019),
recorded as a revision and limited to the sites a grant names (ADR 0021).

## Decision

**Endpoints.** A running container's endpoints are where the gateway can
reach its TCP ports: each published port on the host address it is published
on, with a wildcard address read as `127.0.0.1`, and each port on each address
the container has on a network other than the host's. The agent lists a
container's network addresses with it; the API derives endpoints from them and
the ports. Published ports come first: they stay put when a container is
recreated, while network addresses change.

**Links.** A site points at a container when a node of an upstream that the
site, or one of its routes, proxies to has the address and port of one of the
container's endpoints; `localhost`, `127.0.0.1` and `::1` are one host. Links
are read against the draft, as the sites list shows it, and only for the sites
the caller may read. Listing them needs `containers.read` and `config.read`.

**Labels.** A container declares the site it backs with labels:
`pingora-panel.site.domains`, a comma-separated list of hosts;
`pingora-panel.site.port`, its port, needed when it has several; and
`pingora-panel.site.name`. Nothing is created from labels. The console and the
command line offer what a container declares when creating a site for it, and
show containers that declare a domain no site serves.

**Creating.** Creating a site for a container adds an upstream with one of the
container's endpoints as its node and a reverse-proxy site for it to the draft,
as one change: both or neither. It needs `config.write` for the new site and
`containers.read`, and the endpoint must be one of the container's. Applying
stays a step of its own.

**Discovery.** Where an upstream's nodes are edited, the console offers the
endpoints of the running containers on every enabled engine.

## Alternatives

- Changing routes as containers start and stop: what the gateway serves would
  change without a draft, an approval or a revision.
- Reading Traefik's or Caddy's labels: they carry rule languages of their own
  that the gateway does not share, and half-read rules would route wrongly.
- nginx-proxy's `VIRTUAL_HOST`: an environment variable, and the panel never
  reads a container's environment.
- Container names as upstream hosts: the gateway runs on the host's network,
  where they do not resolve.

## Consequences

- A site that points at a container's network address follows it only while
  the address stays; recreating the container breaks the link and the site.
  The console prefers published ports for this reason.
- Links are found by address, so an upstream that reaches a container through
  another name, such as a DNS record, is not linked.
- A container on the host's network has no endpoints the engine reports, so it
  is offered for neither discovery nor creation.
