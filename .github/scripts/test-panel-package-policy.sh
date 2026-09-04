#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
checker="$repo_root/.github/scripts/check-panel-package-policy.sh"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/panel-package-policy.XXXXXX")"
trap 'rm -rf -- "$test_root"' EXIT

create_fixture() {
  local root="$1"
  local publish_policy="$2"
  mkdir -p "$root/member/src"
  printf '[workspace]\nresolver = "2"\nmembers = ["member"]\n' >"$root/Cargo.toml"
  {
    printf '[package]\nname = "member"\nversion = "0.1.0"\nedition = "2021"\n'
    if [[ -n "$publish_policy" ]]; then
      printf 'publish = %s\n' "$publish_policy"
    fi
  } >"$root/member/Cargo.toml"
  printf '// package policy fixture\n' >"$root/member/src/lib.rs"
  cargo generate-lockfile --manifest-path "$root/Cargo.toml" --offline >/dev/null
}

assert_rejected() {
  local manifest="$1"
  local scenario="$2"
  if bash "$checker" "$manifest" >/dev/null 2>&1; then
    printf '%s was not rejected\n' "$scenario" >&2
    exit 1
  fi
}

private_fixture="$test_root/private"
create_fixture "$private_fixture" false
bash "$checker" "$private_fixture/Cargo.toml" >/dev/null

default_fixture="$test_root/default"
create_fixture "$default_fixture" ""
assert_rejected "$default_fixture/Cargo.toml" "default crates.io publication policy"

registry_fixture="$test_root/registry"
create_fixture "$registry_fixture" '["internal"]'
assert_rejected "$registry_fixture/Cargo.toml" "registry-scoped publication policy"

printf 'Panel package publication policy self-test passed.\n'
