#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

assert_equal() {
  local expected="$1"
  local actual="$2"
  local scenario="$3"
  if [[ "$actual" != "$expected" ]]; then
    printf '%s: expected %s, got %s\n' "$scenario" "$expected" "$actual" >&2
    exit 1
  fi
}

assert_status() {
  local expected="$1"
  local scenario="$2"
  shift 2

  set +e
  "$@" >/dev/null 2>&1
  local actual=$?
  set -e

  assert_equal "$expected" "$actual" "$scenario"
}

for resolver in \
  "$script_dir/resolve-panel-compat-baseline.sh" \
  "$script_dir/resolve-panel-proto-baseline.sh"; do
  resolver_name="$(basename "$resolver")"

  assert_equal \
    "origin/main" \
    "$(bash "$resolver" pull_request ignored main ignored)" \
    "$resolver_name pull request baseline"
  assert_equal \
    "origin/trunk" \
    "$(bash "$resolver" workflow_dispatch ignored ignored trunk)" \
    "$resolver_name manual baseline"
  assert_equal \
    "0123456789abcdef" \
    "$(bash "$resolver" push 0123456789abcdef ignored ignored)" \
    "$resolver_name existing ref baseline"
  assert_equal \
    "origin/trunk" \
    "$(bash "$resolver" \
      push 0000000000000000000000000000000000000000 ignored trunk)" \
    "$resolver_name new ref baseline"

  assert_status \
    2 \
    "$resolver_name pull request without base branch" \
    bash "$resolver" pull_request ignored "" ignored
  assert_status \
    2 \
    "$resolver_name manual run without default branch" \
    bash "$resolver" workflow_dispatch ignored ignored ""
  assert_status \
    2 \
    "$resolver_name push without before SHA" \
    bash "$resolver" push "" ignored main
  assert_status \
    2 \
    "$resolver_name new ref without default branch" \
    bash "$resolver" push 0000000000000000000000000000000000000000 ignored ""
  assert_status \
    2 \
    "$resolver_name unsupported event" \
    bash "$resolver" schedule ignored ignored ignored
done

printf 'Panel compatibility baseline resolver self-test passed.\n'
