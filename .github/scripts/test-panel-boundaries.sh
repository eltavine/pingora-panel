#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
checker="$repo_root/.github/scripts/check-panel-boundaries.sh"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/pingora-panel-boundaries.XXXXXX")"
trap 'rm -rf -- "$test_root"' EXIT

readonly -a expected_members=(
  panel-contracts
  panel-errors
  panel-domain
  panel-ir
  panel-engine
  panel-gateway-runtime
  snapshot-store-fs
  gateway-pingora
  gateway-grpc
  gatewayd
)

create_fixture() {
  local root="$1"
  local mode="$2"
  local member

  mkdir -p "$root"
  {
    printf '[workspace]\nresolver = "2"\nmembers = [\n'
    for member in "${expected_members[@]}"; do
      if [[ "$mode" == missing && "$member" == gatewayd ]]; then
        continue
      fi
      printf '  "%s",\n' "$member"
    done
    if [[ "$mode" == extra ]]; then
      printf '  "unclassified-adapter",\n'
    fi
    printf ']\n'
    if [[ "$mode" == framework ]]; then
      printf 'exclude = ["pingora-fixture"]\n'
    fi
  } >"$root/Cargo.toml"

  for member in "${expected_members[@]}"; do
    if [[ "$mode" == missing && "$member" == gatewayd ]]; then
      continue
    fi
    mkdir -p "$root/$member/src"
    {
      printf '[package]\nname = "%s"\nversion = "0.1.0"\nedition = "2021"\n' "$member"
      if [[ "$member" == panel-ir ]]; then
        printf '\n[dependencies]\npanel-domain = { path = "../panel-domain" }\n'
      elif [[ "$mode" == forbidden && "$member" == panel-domain ]]; then
        printf '\n[dependencies]\ngateway-grpc = { path = "../gateway-grpc" }\n'
      elif [[ "$mode" == framework && "$member" == panel-domain ]]; then
        printf '\n[dependencies]\npingora = { path = "../pingora-fixture" }\n'
      fi
    } >"$root/$member/Cargo.toml"
    printf '// boundary fixture\n' >"$root/$member/src/lib.rs"
  done

  if [[ "$mode" == extra ]]; then
    mkdir -p "$root/unclassified-adapter/src"
    printf '[package]\nname = "unclassified-adapter"\nversion = "0.1.0"\nedition = "2021"\n' \
      >"$root/unclassified-adapter/Cargo.toml"
    printf '// unclassified fixture\n' >"$root/unclassified-adapter/src/lib.rs"
  fi

  if [[ "$mode" == framework ]]; then
    mkdir -p "$root/pingora-fixture/src"
    printf '[package]\nname = "pingora"\nversion = "0.8.0"\nedition = "2021"\n' \
      >"$root/pingora-fixture/Cargo.toml"
    printf '// external framework fixture\n' >"$root/pingora-fixture/src/lib.rs"
  fi

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

valid_fixture="$test_root/valid"
create_fixture "$valid_fixture" valid
bash "$checker" "$valid_fixture/Cargo.toml" >/dev/null

missing_fixture="$test_root/missing"
create_fixture "$missing_fixture" missing
assert_rejected "$missing_fixture/Cargo.toml" "missing workspace member"

extra_fixture="$test_root/extra"
create_fixture "$extra_fixture" extra
assert_rejected "$extra_fixture/Cargo.toml" "unclassified workspace member"

forbidden_fixture="$test_root/forbidden"
create_fixture "$forbidden_fixture" forbidden
assert_rejected "$forbidden_fixture/Cargo.toml" "forbidden internal dependency"

framework_fixture="$test_root/framework"
create_fixture "$framework_fixture" framework
assert_rejected "$framework_fixture/Cargo.toml" "forbidden aggregate Pingora dependency"

printf 'Panel dependency boundary self-test passed.\n'
