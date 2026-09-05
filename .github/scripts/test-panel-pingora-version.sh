#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
checker="$repo_root/.github/scripts/check-panel-pingora-version.sh"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/pingora-panel-version.XXXXXX")"
trap 'rm -rf -- "$test_root"' EXIT

create_package() {
  local root="$1"
  local name="$2"
  local version="$3"
  mkdir -p "$root/$name/src"
  printf '[package]\nname = "%s"\nversion = "%s"\nedition = "2021"\n' \
    "$name" "$version" >"$root/$name/Cargo.toml"
  printf '// version fixture\n' >"$root/$name/src/lib.rs"
}

create_fixture() {
  local root="$1"
  local mode="$2"
  local http_version="0.8.1"
  local http_requirement='=0.8.1'
  local reported_version="0.8.1"
  local core_path='../../dependencies/pingora-core'

  if [[ "$mode" == divergent ]]; then
    http_version="0.9.0"
    http_requirement='=0.9.0'
  elif [[ "$mode" == unpinned ]]; then
    http_requirement='0.8.1'
  elif [[ "$mode" == constant-drift ]]; then
    reported_version="0.9.0"
  elif [[ "$mode" == redirected ]]; then
    core_path='../../redirected/pingora-core'
  fi

  mkdir -p "$root/panel/gateway-pingora/src" "$root/dependencies"
  printf '[workspace]\nresolver = "2"\nmembers = ["gateway-pingora"]\n' \
    >"$root/panel/Cargo.toml"
  create_package "$root/dependencies" pingora 0.8.1
  create_package "$root/dependencies" pingora-core 0.8.1
  create_package "$root/dependencies" pingora-http "$http_version"
  create_package "$root/dependencies" pingora-load-balancing 0.8.1
  if [[ "$mode" == redirected ]]; then
    create_package "$root/redirected" pingora-core 0.8.1
  fi

  {
    printf '[package]\nname = "gateway-pingora"\nversion = "0.1.0"\nedition = "2021"\n'
    printf '\n[dependencies]\n'
    printf 'pingora = { version = "=0.8.1", path = "../../dependencies/pingora" }\n'
    printf 'pingora-core = { version = "=0.8.1", path = "%s" }\n' "$core_path"
    printf 'pingora-http = { version = "%s", path = "../../dependencies/pingora-http" }\n' \
      "$http_requirement"
    printf 'pingora-load-balancing = { version = "=0.8.1", path = "../../dependencies/pingora-load-balancing" }\n'
  } >"$root/panel/gateway-pingora/Cargo.toml"
  printf 'pub const PINGORA_PACKAGE_VERSION: &str = "%s";\n' \
    "$reported_version" >"$root/panel/gateway-pingora/src/lib.rs"
  cargo generate-lockfile --manifest-path "$root/panel/Cargo.toml" --offline >/dev/null
}

assert_rejected() {
  local root="$1"
  local scenario="$2"
  if bash "$checker" \
    "$root/panel/Cargo.toml" \
    "$root/panel/gateway-pingora/src/lib.rs" \
    "$root/dependencies" >/dev/null 2>&1; then
    printf '%s was not rejected\n' "$scenario" >&2
    exit 1
  fi
}

valid_fixture="$test_root/valid"
create_fixture "$valid_fixture" valid
bash "$checker" \
  "$valid_fixture/panel/Cargo.toml" \
  "$valid_fixture/panel/gateway-pingora/src/lib.rs" \
  "$valid_fixture/dependencies" >/dev/null

for mode in divergent unpinned constant-drift redirected; do
  fixture="$test_root/$mode"
  create_fixture "$fixture" "$mode"
  assert_rejected "$fixture" "$mode Pingora version contract"
done

printf 'Pingora adapter version coherence self-test passed.\n'
