# 0045: Surface parity

Status: accepted. Builds on [ADR 0002](0002-management-http-contracts.md),
[ADR 0005](0005-web-console-stack.md),
[ADR 0011](0011-configuration-model-and-apply.md),
[ADR 0012](0012-configuration-language-and-revisions.md),
[ADR 0019](0019-change-approvals.md) and
[ADR 0023](0023-adaptive-layout-touch-and-motion.md).

## Context

Every business query and change is a public API operation, and the
command line and the console are meant to offer each with the same
meaning. Nothing checks that they do: an operation can ship with a command
and no page, or a page can stop calling it, and only a reader comparing
the three would notice. Applying the draft also asks for less than review
promises: it names the draft version it expects, so a plan reviewed
against one active revision can still be applied after another revision
became active, changing more than the reviewer saw. The console has no
picture of how sites reach upstreams and their nodes, and the
configuration editor does not say what an empty file or a failed load
means.

## Decision

**Surfaces are declared.** `panel/surfaces.json` names, for every
operation of the OpenAPI document, the `ppanel` command and the console
route that offer it. An operation whose meaning belongs to one form says
so with `specific`, a reason, and the surfaces it has, as the API's own
description, a browser's sign-in redirect or a workload's token exchange
do. A console route may name the operation it reads instead with
`console_via`, when a page shows what the operation returns from its
list, as resource pages do for single resources.

**Parity is checked.** Three checks keep the declaration true:

- every operation of the OpenAPI document is declared exactly once, every
  declared operation exists, every declaration names both surfaces or a
  `specific` reason, and the share of non-specific operations offered on
  all three surfaces is 100%;
- every declared command exists in `ppanel`'s command tree;
- every declared console route is a route of the console, and the console
  calls the declared operation through its generated client or its path.

**Applying names the plan.** A plan is identified by a digest of the
content hashes of the active revision and the draft, so it changes when
either does and not when a save leaves the files as they were. The plan
carries its digest, the draft version and the active revision it compares.
`expected_plan` makes apply refuse, with `409 Conflict` and nothing
changed, when the plan computed at apply is not the one named; the check
happens where apply reads the draft and the active revision, and
activation compares the active hash as before (ADR 0011). A refusal is
recorded with `config.apply.failed`, which names the plan expected, and
`config.draft.applied` names the plan applied.

- `ppanel config plan` prints the plan and its digest.
  `ppanel config apply` prints the plan and asks before applying it on a
  terminal; `--yes` confirms the plan it printed, and `--plan <digest>`
  applies only a plan reviewed earlier. Without a terminal, apply needs
  one of them.
- The console's review shows the plan; applying asks for confirmation
  with what changes, against which revision, and sends the plan's digest.
  A plan that changed meanwhile is shown again to review.
- Dry runs check without activating and need no confirmation; approvals
  (ADR 0019) bind the draft's content as before.

**Upstream topology.** The upstreams page shows sites, the routes that
proxy, the pools they reach and the pools' nodes with live health, from
the operations that list sites, upstreams and upstream health. It is a
presentation (`specific` in the catalog's sense): columns joined by
connectors on wide screens, each item naming what it reaches, so the
picture reads as nested lists without the drawing, on narrow screens and
to assistive technology. Health refreshes as the upstreams page does.

**Configuration editor states.** The editor shows a skeleton while the
files load, a failure with retry when they cannot be read, and an empty
file as a prompt for directives, with the import of NGINX configuration at
hand. Each state is covered by browser tests on desktop and mobile, as is
reflow at 320 CSS pixels (WCAG 2.2 SC 1.4.10).

## Alternatives

- OpenAPI extensions on each operation (`x-cli`, `x-console`): the
  contract would carry the console's routes and change with the command
  line's names; one declaration beside the three keeps them independent.
- Measuring parity from test traffic: tests cover flows, not surfaces, and
  would make a missing test look like a missing command.
- The draft version alone, as before: it misses a change of the active
  revision.
- `If-Match` with the plan's entity tag: apply already names what it
  expects in its body; an entity tag would describe the plan resource,
  not the command.
- A graph library (Vue Flow with dagre or ELK) for the topology: the graph
  is always four layers, and pan, zoom and automatic layout would add a
  dependency and an interaction model the console has nowhere else.

## Consequences

- A new operation fails the checks until its command and console route
  exist or its reason is written down.
- Scripts that apply without a terminal pass `--yes`, or `--plan` with a
  digest they reviewed.
- A plan reviewed before someone else applied, or before a rollback, can
  no longer be applied unseen.
