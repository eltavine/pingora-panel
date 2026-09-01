#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
if [[ $# -gt 1 ]]; then
  printf 'usage: %s [panel-workspace-manifest]\n' "${BASH_SOURCE[0]}" >&2
  exit 2
fi

manifest="${1:-$repo_root/panel/Cargo.toml}"
if [[ ! -f "$manifest" ]]; then
  printf 'Panel workspace manifest does not exist: %s\n' "$manifest" >&2
  exit 2
fi

for command in cargo jq; do
  if ! command -v "$command" >/dev/null 2>&1; then
    printf 'required command is unavailable: %s\n' "$command" >&2
    exit 2
  fi
done

metadata="$(
  cargo metadata \
    --manifest-path "$manifest" \
    --format-version 1 \
    --no-deps \
    --locked
)"
failed=0

expected_members=(
  panel-contracts \
  panel-errors \
  panel-domain \
  panel-ir \
  panel-engine \
  panel-gateway-runtime \
  snapshot-store-fs \
  gateway-pingora \
  gateway-grpc \
  gatewayd
)

declare -A expected_member_set=()
for expected in "${expected_members[@]}"; do
  expected_member_set["$expected"]=1
done

declare -A workspace_member_set=()
while IFS= read -r member; do
  workspace_member_set["$member"]=1
  if [[ -z "${expected_member_set[$member]-}" ]]; then
    printf 'workspace violation: unclassified member %s is not in the boundary policy\n' \
      "$member" >&2
    failed=1
  fi
done < <(
  jq -r '
    .workspace_members as $members
    | .packages[]
    | select(.id as $id | $members | index($id))
    | .name
  ' <<<"$metadata"
)

for expected in "${expected_members[@]}"; do
  if [[ -z "${workspace_member_set[$expected]-}" ]]; then
    printf 'workspace violation: expected member %s is missing\n' "$expected" >&2
    failed=1
  fi
done

declare -A direct_dependencies
while IFS=$'\t' read -r package dependency; do
  direct_dependencies["$package"]+=" $dependency"
done < <(
  jq -r '
    .workspace_members as $members
    | .packages[]
    | select(.id as $id | $members | index($id))
    | .name as $package
    | .dependencies[]
    | [$package, .name]
    | @tsv
  ' <<<"$metadata"
)

declare -A allowed_internal_dependencies=(
  [panel-contracts]=" panel-errors "
  [panel-errors]=" "
  [panel-domain]=" "
  [panel-ir]=" panel-domain "
  [panel-engine]=" panel-errors panel-domain panel-ir "
  [panel-gateway-runtime]=" panel-errors panel-domain panel-ir panel-engine "
  [snapshot-store-fs]=" panel-errors panel-domain panel-ir panel-engine "
  [gateway-pingora]=" panel-errors panel-domain panel-ir panel-engine "
  [gateway-grpc]=" panel-contracts panel-errors panel-domain panel-ir panel-engine "
  [gatewayd]=" panel-contracts panel-errors panel-domain panel-ir panel-engine panel-gateway-runtime snapshot-store-fs gateway-pingora gateway-grpc "
)

report_path() {
  local package="$1"
  local dependency="$2"
  printf '  reverse dependency path: %s -> %s\n' "$package" "$dependency" >&2
}

for package in "${!direct_dependencies[@]}"; do
  for dependency in ${direct_dependencies[$package]}; do
    if [[ -n "${workspace_member_set[$dependency]-}" \
      && "${allowed_internal_dependencies[$package]- }" != *" $dependency "* ]]; then
      printf 'boundary violation: %s is not allowed to depend on workspace member %s\n' \
        "$package" "$dependency" >&2
      report_path "$package" "$dependency"
      failed=1
    fi
    if [[ ("$dependency" == pingora || "$dependency" == pingora-*) \
      && "$package" != "gateway-pingora" ]]; then
      printf 'boundary violation: only gateway-pingora may depend on %s\n' "$dependency" >&2
      report_path "$package" "$dependency"
      failed=1
    fi
  done
done

protected=(panel-errors panel-domain panel-ir panel-engine panel-gateway-runtime)
for package in "${protected[@]}"; do
  for dependency in ${direct_dependencies[$package]-}; do
    case "$dependency" in
      pingora|pingora-*|tonic|tonic-prost|prost|axum|sqlx|async-nats|nats)
        printf 'boundary violation: %s must remain engine/transport/storage neutral\n' "$package" >&2
        report_path "$package" "$dependency"
        failed=1
        ;;
    esac
  done
done

if [[ "$failed" -ne 0 ]]; then
  exit 1
fi

printf 'Panel dependency boundaries verified.\n'
