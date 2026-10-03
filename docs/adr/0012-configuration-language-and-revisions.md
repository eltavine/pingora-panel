# 0012: Configuration language and revisions

Status: accepted.

## Context

The product specification makes a textual configuration language the
canonical source of what the gateway runs: forms and commands edit it, every
applied configuration is an immutable revision of it, and nothing builds a
runtime snapshot by another route. Operators know NGINX, so the language
should read like it without inheriting its implicit precedence. Diagnostics
must point at lines and columns, edits through forms must not destroy
comments, and the language must evolve without breaking stored revisions.

## Decision

- **Syntax.** The lexical rules are NGINX's: directives are a name and
  arguments ended by `;` or followed by a `{ }` block; arguments are bare
  words or single- or double-quoted strings with backslash escapes; `#`
  starts a comment. `panel-dsl` parses this into a syntax tree whose nodes
  know their exact byte ranges and the comments around them, reports errors
  with recovery at `;` and `}`, and formats a file canonically without
  losing a comment. Spans follow the GNU error format,
  `file:line.column-line.column`.
- **Language.** `panel-config-dsl` gives the syntax meaning. A file starts
  with `language_version 1;`. The `http` block holds `tls_profile`,
  `listener`, `upstream` and `server` blocks; servers hold `server_name`,
  `alias`, `domain`, `listen`, an action (`proxy`, `root`, `return` or
  `respond`) and `route` blocks with an explicit `match exact|prefix|glob|regex`.
  Values are typed when read: integers, `on|off` booleans, durations such as
  `1m30s`, sizes such as `10m`, IP addresses and CIDR blocks, lists and
  `key=value` parameters. A schema lists every directive with its contexts,
  arguments and deprecation; unknown directives and directives outside
  their context are errors, and deprecated ones name their replacement and
  the version that removes them. The schema is published for editor
  completion.
- **Variables.** `set $name value;` defines a constant for the enclosing
  block and the blocks inside it, and `${env:NAME}` reads an environment
  variable the configuration service exposes under the
  `PINGORA_PANEL_DSL_` prefix, both resolved at compile time. Request
  variables — `$host`, `$uri`, `$method`, `$scheme`, `$client_ip`,
  `$request_id` and `$upstream_addr` — are kept as templates and expanded by
  the gateway where a directive accepts them.
- **Identity.** `server`, `upstream`, `route` and upstream nodes carry their
  stable identifier as `id`, which the service adds when a block lacks one,
  so renaming keeps history and drained nodes keep their state. Listeners
  and TLS profiles are identified by their names.
- **Files.** A configuration is a set of files with `main.conf` as entry.
  `include` takes a path or glob within the set; cycles and missing files
  are errors. Nothing reads the service's own file system.
- **One draft, two forms.** The draft is stored as its files together with
  the model `panel-config-model` validates and compiles, and each is derived
  from the other. Editing a resource through the API reprints only that
  resource's block, keeping the comments around it, and leaves every other
  byte alone; saving text parses it and carries over what the language does
  not express — timestamps, favourites and the recycle bin — by identifier.
- **Revisions.** Applying records the draft's files as an immutable
  revision with its SHA-256 content hash and the IR hash, then compiles that
  revision, not the draft, so what runs is exactly what history shows.
  Revisions keep an author, a note and their outcome. Rolling back copies a
  revision's files into the draft, which is applied like any other change.
- **Review.** A plan lists the sites, upstreams, listeners and profiles a
  draft adds, changes or removes against the active revision, beside a
  line diff of the files. A dry run compiles the draft and prepares it on
  the gateway without activating it. Checks are reported by stage: syntax,
  schema, semantics and conflicts such as duplicate names or shadowed
  routes.
- **NGINX import.** A documented subset of NGINX — `server`, `listen`,
  `server_name`, `location` with its modifiers, `proxy_pass`, `root`,
  `return` and `upstream` — converts to the language, with a report that
  lists every directive it could not carry over. `location` is otherwise
  accepted only as an alias that formats as `route`.

## Consequences

- Text and forms are interchangeable views of one draft, and history is
  readable as text.
- New directives extend the schema; a stored revision written for an older
  `language_version` is upgraded by an explicit migration that keeps the
  original text.
- Request-variable templates make the gateway evaluate strings per request,
  which the adapter declares as capabilities like any other feature.
