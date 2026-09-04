#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
checker="$repo_root/.github/scripts/check-panel-boundaries.sh"
policy="$repo_root/.github/policies/panel-boundaries.json"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/pingora-panel-boundaries.XXXXXX")"
trap 'rm -rf -- "$test_root"' EXIT

expected_members=()
while IFS= read -r member; do
  expected_members+=("$member")
done < <(jq -r '.members | keys[]' "$policy")

create_fixture() {
  local root="$1"
  local mode="$2"
  local member

  mkdir -p "$root"
  if [[ "$mode" == custom-target ]]; then
    mkdir -p "$root/.cargo" "$root/artifacts/generated"
    printf '[build]\ntarget-dir = "artifacts"\n' >"$root/.cargo/config.toml"
    printf '[package]\nname = "generated-artifact"\nversion = "0.1.0"\n' \
      >"$root/artifacts/generated/Cargo.toml"
  fi
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
    elif [[ "$mode" == config-dependency ]]; then
      printf 'exclude = ["utility-fixture"]\n'
    elif [[ "$mode" == stray ]]; then
      printf 'exclude = ["stray-fixture"]\n'
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
        if [[ "$mode" == redirected-internal ]]; then
          printf \
            '\n[dependencies]\npanel-domain = { version = "=0.1.1", path = "../../counterfeit-panel-domain" }\n'
        elif [[ "$mode" == unpinned-internal ]]; then
          printf \
            '\n[dependencies]\npanel-domain = { version = "0.1", path = "../panel-domain" }\n'
        else
          printf \
            '\n[dependencies]\npanel-domain = { version = "=0.1.0", path = "../panel-domain" }\n'
        fi
      elif [[ "$member" == panel-config-state-codec ]]; then
        printf \
          '\n[dependencies]\npanel-config-domain = { version = "=0.1.0", path = "../panel-config-domain" }\n'
      elif [[ "$mode" == forbidden && "$member" == panel-domain ]]; then
        printf \
          '\n[dependencies]\ngateway-grpc = { version = "=0.1.0", path = "../gateway-grpc" }\n'
      elif [[ "$mode" == framework && "$member" == panel-domain ]]; then
        printf '\n[dependencies]\npingora = { path = "../pingora-fixture" }\n'
      elif [[ "$mode" == config-dependency && "$member" == panel-config-domain ]]; then
        printf '\n[dependencies]\nutility = { path = "../utility-fixture" }\n'
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

  if [[ "$mode" == config-dependency ]]; then
    mkdir -p "$root/utility-fixture/src"
    printf '[package]\nname = "utility"\nversion = "1.0.0"\nedition = "2021"\n' \
      >"$root/utility-fixture/Cargo.toml"
    printf '// external utility fixture\n' >"$root/utility-fixture/src/lib.rs"
  fi

  if [[ "$mode" == stray ]]; then
    mkdir -p "$root/stray-fixture/src"
    printf '[package]\nname = "stray-fixture"\nversion = "0.1.0"\nedition = "2021"\n' \
      >"$root/stray-fixture/Cargo.toml"
    printf '// unclassified package fixture\n' >"$root/stray-fixture/src/lib.rs"
  fi

  if [[ "$mode" == redirected-internal ]]; then
    mkdir -p "$root/../counterfeit-panel-domain/src"
    printf '[package]\nname = "panel-domain"\nversion = "0.1.1"\nedition = "2021"\n' \
      >"$root/../counterfeit-panel-domain/Cargo.toml"
    printf '// counterfeit workspace dependency\n' \
      >"$root/../counterfeit-panel-domain/src/lib.rs"
  fi

  (
    cd "$root"
    cargo generate-lockfile --manifest-path Cargo.toml --offline >/dev/null 2>&1
  )
}

assert_rejected() {
  local manifest="$1"
  local scenario="$2"
  local selected_policy="${3:-$policy}"
  if bash "$checker" "$manifest" "$selected_policy" >/dev/null 2>&1; then
    printf '%s was not rejected\n' "$scenario" >&2
    exit 1
  fi
}

valid_fixture="$test_root/valid"
create_fixture "$valid_fixture" valid
bash "$checker" "$valid_fixture/Cargo.toml" "$policy" >/dev/null

custom_target_fixture="$test_root/custom-target"
create_fixture "$custom_target_fixture" custom-target
bash "$checker" "$custom_target_fixture/Cargo.toml" "$policy" >/dev/null

invalid_policy="$test_root/invalid-policy.json"
printf '{"schema_version": 999, "members": {}, "rules": {}}\n' >"$invalid_policy"
assert_rejected \
  "$valid_fixture/Cargo.toml" \
  "unsupported boundary policy schema" \
  "$invalid_policy"

unknown_field_policy="$test_root/unknown-field-policy.json"
jq '.members["panel-domain"].unexpected = true' "$policy" >"$unknown_field_policy"
assert_rejected \
  "$valid_fixture/Cargo.toml" \
  "unknown boundary policy field" \
  "$unknown_field_policy"

duplicate_rule_policy="$test_root/duplicate-rule-policy.json"
jq '.rules.dependency_free_members += ["panel-config-domain"]' \
  "$policy" >"$duplicate_rule_policy"
assert_rejected \
  "$valid_fixture/Cargo.toml" \
  "duplicate boundary policy rule entry" \
  "$duplicate_rule_policy"

missing_fixture="$test_root/missing"
create_fixture "$missing_fixture" missing
assert_rejected "$missing_fixture/Cargo.toml" "missing workspace member"

extra_fixture="$test_root/extra"
create_fixture "$extra_fixture" extra
assert_rejected "$extra_fixture/Cargo.toml" "unclassified workspace member"

stray_fixture="$test_root/stray"
create_fixture "$stray_fixture" stray
assert_rejected "$stray_fixture/Cargo.toml" "unclassified package outside the workspace"

forbidden_fixture="$test_root/forbidden"
create_fixture "$forbidden_fixture" forbidden
assert_rejected "$forbidden_fixture/Cargo.toml" "forbidden internal dependency"

unpinned_internal_fixture="$test_root/unpinned-internal"
create_fixture "$unpinned_internal_fixture" unpinned-internal
assert_rejected \
  "$unpinned_internal_fixture/Cargo.toml" \
  "non-exact internal workspace dependency requirement"

redirected_internal_fixture="$test_root/redirected-internal"
create_fixture "$redirected_internal_fixture" redirected-internal
assert_rejected \
  "$redirected_internal_fixture/Cargo.toml" \
  "redirected internal workspace dependency"

framework_fixture="$test_root/framework"
create_fixture "$framework_fixture" framework
assert_rejected "$framework_fixture/Cargo.toml" "forbidden aggregate Pingora dependency"

config_dependency_fixture="$test_root/config-dependency"
create_fixture "$config_dependency_fixture" config-dependency
assert_rejected \
  "$config_dependency_fixture/Cargo.toml" \
  "dependency introduced into the pure configuration domain"

printf 'Panel dependency boundary self-test passed.\n'
