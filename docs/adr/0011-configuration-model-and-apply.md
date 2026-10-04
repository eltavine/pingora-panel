# 0011: Configuration model, draft and apply

Status: accepted.

## Context

Operators manage listeners, TLS profiles, upstreams and sites with their
domains and routes through the API, the `ppanel` command line and the web
console. The gateway only runs engine-neutral snapshots. Edits need a
stable home between those two shapes: one that validates as it goes, keeps
concurrent and retried edits safe, and changes the running gateway only
when an operator applies a complete, valid configuration. New resource
kinds should not force new RPCs or wire changes in every layer.

## Decision

- **One draft document.** `config-service` keeps the draft as a single
  versioned document, `pingora.panel.config/v1`, in its own schema. Every
  change runs on a copy and commits by bumping the version, so readers
  never see half an edit. `panel-config-model` owns the document, its
  validation, queries and edits, and the compilation into the IR; it has
  no I/O.
- **Checked edits.** An edit is refused if it introduces a validation error
  the draft did not already have; existing problems do not block unrelated
  changes. Sites with errors, no enabled domain or no enabled upstream node
  report as `abnormal`, so problems stay visible instead of failing apply
  late.
- **Typed operations.** The gRPC service has three methods — `Read`,
  `Change` and `Apply` — over a closed set of operations that
  `panel-config-api` declares as enums, a variant for each read or change
  with its typed inputs: the REST API, the gRPC adapters and the
  configuration service share them, and each side matches them
  exhaustively, so an operation one side does not handle fails to compile.
  On the wire an operation travels as its JSON encoding, under the name
  audit records and change receipts know it by, such as `sites.create` or
  `routes.reorder`. Adding a resource adds operations, not RPCs.
- **Layers.** The configuration service's use cases implement the same
  port its gRPC client does; a thin adapter serves that port over gRPC.
  The draft, revisions and approvals are ports the use cases hold, SQLite
  being one implementation, and approval rules are pure functions every
  store applies within its own transaction.
- **Concurrency and retries.** Each resource has an entity tag. Replacing or
  deleting requires `If-Match` (428 without it, 412 when stale, RFC 9110
  §13.1.1 and RFC 6585). Every change carries an idempotency key; its
  receipt replays the original answer, and reusing the key for a different
  request is a conflict. Each committed change emits
  `config.draft.changed` through the outbox.
- **Apply.** Applying names the draft version it expects. The draft is
  compiled to an IR snapshot — a site's own action becomes its last route,
  and only upstreams some route uses are included — validated
  against the gateway's declared capabilities, prepared and activated with
  compare-and-swap on the active hash, then marked applied and announced
  with `config.draft.applied`. A rejected apply returns every diagnostic as
  Problem Details (RFC 9457) and changes nothing.
- **Lifecycle.** Deleting a site moves it to a recycle bin from which it can
  be restored or purged. Cloning copies settings and routes but not
  domains, which stay unique. Export writes sites with the upstreams and
  profiles they use; import gives everything fresh identities.
- **Queries.** Site lists filter by text, status, type, domain, tag, group
  and favourite, sort by a stable key, and page with an opaque cursor. Each
  response reports the draft version it reflects.
- **One contract, three surfaces.** The REST API documents the model's own
  types through OpenAPI. The console's client is generated from that
  document; the command line sends the same requests and maps outcomes to
  exit codes. Neither keeps a schema of its own.

## Consequences

- Editing and applying are separate steps, so several changes can be
  reviewed and validated together and applied atomically.
- The draft is a single document, so a textual DSL, diffs between versions
  and approval steps can operate on it without a storage migration.
- A failed apply leaves both the draft and the running gateway unchanged;
  the operator fixes the draft and applies again.
