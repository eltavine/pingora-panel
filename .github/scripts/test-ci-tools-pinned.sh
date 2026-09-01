#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
checker="$repo_root/.github/scripts/check-ci-tools-pinned.sh"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/ci-tools-pinned.XXXXXX")"
trap 'rm -rf -- "$test_root"' EXIT

assert_accepted() {
  local scenario="$1"
  local command="$2"
  printf 'jobs:\n  check:\n    steps:\n      - run: %s\n' "$command" \
    >"$test_root/workflow.yml"
  if ! bash "$checker" "$test_root" >/dev/null 2>&1; then
    printf '%s was rejected\n' "$scenario" >&2
    exit 1
  fi
}

assert_rejected() {
  local scenario="$1"
  local command="$2"
  printf 'jobs:\n  check:\n    steps:\n      - run: %s\n' "$command" \
    >"$test_root/workflow.yml"
  if bash "$checker" "$test_root" >/dev/null 2>&1; then
    printf '%s was not rejected\n' "$scenario" >&2
    exit 1
  fi
}

assert_accepted \
  "exact Cargo tool version" \
  "cargo install --locked --version 1.2.3 cargo-example"
assert_accepted \
  "exact Go module version" \
  "go install example.com/tools/check@v1.2.3"
assert_accepted \
  "exact Go prerelease version" \
  "go install example.com/tools/check@v1.2.3-rc.1"

assert_rejected \
  "mutable Cargo tool version hidden by a comment" \
  "cargo install cargo-example # --locked --version 1.2.3"
assert_rejected \
  "mutable Go module version" \
  "go install example.com/tools/check@latest"
assert_rejected \
  "mutable Go module version hidden by a comment" \
  "go install example.com/tools/check@latest # example.com/other@v1.2.3"
assert_rejected \
  "ambiguous multi-module Go install" \
  "go install example.com/one@v1.2.3 example.com/two@v1.2.3"

printf 'CI tool pinning guard self-test passed.\n'
