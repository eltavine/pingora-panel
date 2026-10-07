# 0042: Directory listings, media types and cache headers of static content

Status: accepted. Builds on [ADR 0010](0010-pingora-data-plane.md),
[ADR 0034](0034-site-files.md) and
[ADR 0041](0041-error-pages-and-maintenance.md).

## Context

Static content serves the files below a directory of the gateway's static
root, with index files, conditional and range requests and a single-page
fallback. A directory without an index file answers 404, a file's media type
is guessed from its extension alone, and no response says how long it may be
reused, so browsers and shared caches revalidate or guess heuristically
(RFC 9111 §4.2.2). Operators moving from nginx expect its `autoindex`,
`types` and `default_type`, and `expires`.

## Decision

**Directory listings.** Static content may list a directory that has no
index file, as HTML or as JSON, as nginx's `autoindex` with
`autoindex_format html` or `json` does; index files still come first, and a
directory without either answers 404 as before. A listing names the
directory's entries other than those starting with `.` and those whose
symbolic links lead out of the root, directories first, each group sorted by
name, with sizes and modification times. HTML escapes names and links to
them percent-encoded as RFC 3986 §3.3 says; JSON entries are nginx's:
`name`, `type` (`directory` or `file`), `mtime` as an HTTP date (RFC 9110
§5.6.7) and a file's `size`. A listing stops at 10,000 entries so a huge
directory cannot exhaust memory; an HTML listing says so, and a JSON one
stays the array nginx writes.

**Media types.** Static content may map extensions to media types, ahead of
the built-in guesses, and name the type of files whose extension is unknown,
`application/octet-stream` unless written, as nginx's `types` and
`default_type`. Extensions are compared without case; a written type is sent
as written, while built-in text types keep `charset=utf-8`. File error pages
follow the built-in guesses.

**Cache headers.** Static content may list rules that set `Cache-Control`
(RFC 9111 §5.2.2) on the files it serves: each rule names extensions, or
none for every file, and either a `max-age` with `immutable` (RFC 8246)
when the file never changes at its URL, or `no-cache` so caches revalidate
it first. The first rule naming a file's extension applies, then the first
rule for every file; files no rule applies to get no `Cache-Control`. A 304
response carries the field its 200 would (RFC 9110 §15.4.5). `Expires` is not
sent: `max-age` overrides it (RFC 9111 §5.3) and every cache in use reads
`Cache-Control`. A `max-age` is at most 2^31 seconds (RFC 9111 §1.2.2).

**Contracts.** The model, the configuration language, the IR and the API
carry the three settings on the static action. A snapshot that lists
directories requires `static.listing`, one that maps media types requires
`static.media-types`, and one with cache rules requires
`static.cache-control`.

## Alternatives

- Listing directories that have an index file too, behind a query: nginx
  never does, and such sites would show files their index was meant to hide.
- `Expires` beside `Cache-Control`, as nginx's `expires` writes: it doubles
  every header for caches that no longer exist, and its clock-based value is
  wrong wherever clocks differ.
- Gateway-wide media types only: nginx's `types` applies per location, and
  sites serving foreign file layouts need their own.

## Consequences

- A gateway that does not know the settings refuses snapshots using them
  instead of ignoring them.
- Static content without the settings is served as before.
- Listings reveal file names and sizes, so they are off unless asked for and
  never show hidden files.
