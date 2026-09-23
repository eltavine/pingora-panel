# 0003: Pin oasdiff for HTTP compatibility checks

Status: accepted for the v1 OpenAPI fixture.

## Problem

Comparing the generated document with the checked-in fixture in the same
commit cannot catch a breaking change when both are updated together. CI must
also compare against the previous public contract. A raw JSON diff cannot
distinguish safe additive changes from removals or stronger requirements.

## Decision and evaluation

Use the maintained Apache-2.0 licensed `oasdiff` CLI, pinned to release 1.32.1
and a verified archive SHA-256 in CI. Its OpenAPI 3.1 support matches Utoipa's
fixture; `breaking --fail-on ERR` provides semantic checks for endpoints,
required parameters, responses and schema changes. The isolated check script
reads a Git baseline into a temporary file and disallows external references.
It reuses the existing Proto/Rust baseline resolver: a PR uses its base branch,
and a push uses the immutable preceding commit. A missing baseline fixture is
a one-time bootstrap; a bad commit reference fails closed. Negative self-tests
cover removal, newly required parameters, response removal and field type
changes, alongside an additive optional field.
The current fixture must exist even for a bootstrap comparison. Self-tests
also exercise an unchanged contract, first introduction, missing current
fixture, malformed JSON and forbidden external references.

Alternatives considered: a custom JSON comparator would duplicate OpenAPI
compatibility semantics and require long-term maintenance. A fixture equality
test remains useful for review but does not compare commits. The existing Buf
guard understands Protobuf rather than OpenAPI. Running a third-party CLI as a
CI-only tool keeps it out of Rust application modules and avoids exposing its
types through stable ports.

## Upgrade and limits

Pinning the release and digest makes upgrades reviewable and guards against
unverified binaries. Upgrade with the upstream release notes and the local
self-test, then confirm results against the actual fixture. Semantic tools
cannot prove every runtime behavior is compatible: review status semantics,
authentication and error behavior separately. This guard does not expose the
internal REST listener or assert that the full product API is implemented.

The review found a concrete blind spot in 1.32.1: changing the existing
request schemas from implicit `additionalProperties: true` to `false` was not
reported by `breaking`, even with `--fail-on WARN`. Keep real HTTP regression
coverage for extension fields and preserve the v1 envelope behavior. A green
schema comparison alone is not evidence of full runtime compatibility.

Upstream references: [oasdiff breaking checks](https://github.com/oasdiff/oasdiff/blob/v1.32.1/docs/BREAKING-CHANGES.md),
[Serde container attributes](https://serde.rs/container-attrs.html), and
[Cargo SemVer compatibility](https://doc.rust-lang.org/cargo/reference/semver.html).
