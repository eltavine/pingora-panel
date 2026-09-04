#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
if [[ $# -gt 2 ]]; then
  printf 'usage: %s [panel-workspace-manifest] [boundary-policy]\n' \
    "${BASH_SOURCE[0]}" >&2
  exit 2
fi

manifest="${1:-$repo_root/panel/Cargo.toml}"
policy="${2:-$repo_root/.github/policies/panel-boundaries.json}"
if [[ ! -f "$manifest" ]]; then
  printf 'Panel workspace manifest does not exist: %s\n' "$manifest" >&2
  exit 2
fi
if [[ ! -f "$policy" ]]; then
  printf 'Panel boundary policy does not exist: %s\n' "$policy" >&2
  exit 2
fi
manifest_directory="$(cd "$(dirname "$manifest")" && pwd -P)"
manifest="$manifest_directory/$(basename "$manifest")"
policy="$(cd "$(dirname "$policy")" && pwd -P)/$(basename "$policy")"

for command in cargo find jq; do
  if ! command -v "$command" >/dev/null 2>&1; then
    printf 'required command is unavailable: %s\n' "$command" >&2
    exit 2
  fi
done

if ! jq -e '
  def unique_nonempty_strings:
    type == "array"
    and all(.[]; type == "string" and length > 0)
    and length == (unique | length);

  . as $policy
  | (keys == ["members", "rules", "schema_version"])
    and .schema_version == 1
    and (.members | type == "object" and length > 0)
    and all(.members | to_entries[];
      (.key | length > 0)
      and (.value | keys == ["allowed_workspace_dependencies", "catalog"])
      and (.value.catalog | type == "boolean")
      and (.value.allowed_workspace_dependencies | unique_nonempty_strings)
      and (.key as $member
      | all(.value.allowed_workspace_dependencies[];
          . != $member and $policy.members[.] != null)))
    and (.rules | type == "object")
    and (.rules | keys == [
      "dependency_free_members",
      "engine_transport_storage_neutral_members",
      "forbidden_neutral_dependencies",
      "pingora_dependency_owner"
    ])
    and (.rules.dependency_free_members | unique_nonempty_strings)
    and all(.rules.dependency_free_members[]; $policy.members[.] != null)
    and all(.rules.dependency_free_members[];
      . as $member
      | ($policy.members[$member].allowed_workspace_dependencies | length == 0)
        and ($policy.rules.engine_transport_storage_neutral_members | index($member) != null))
    and (.rules.pingora_dependency_owner | type == "string" and length > 0)
    and ($policy.members[$policy.rules.pingora_dependency_owner] != null)
    and (.rules.engine_transport_storage_neutral_members | unique_nonempty_strings)
    and all(.rules.engine_transport_storage_neutral_members[];
      $policy.members[.] != null)
    and (.rules.forbidden_neutral_dependencies | unique_nonempty_strings)
' "$policy" >/dev/null; then
  printf 'Panel boundary policy is malformed or uses an unsupported schema: %s\n' \
    "$policy" >&2
  exit 2
fi

metadata="$(
  cd "$manifest_directory"
  cargo metadata \
      --manifest-path "$manifest" \
      --format-version 1 \
      --no-deps \
      --locked
)"
failed=0

expected_members=()
while IFS= read -r expected; do
  expected_members+=("$expected")
done < <(jq -r '.members | keys[]' "$policy")

declare -A expected_member_set=()
for expected in "${expected_members[@]}"; do
  expected_member_set["$expected"]=1
done

declare -A workspace_member_set=()
declare -A workspace_manifest_set=()
declare -A workspace_member_root=()
declare -A workspace_member_version=()
while IFS=$'\t' read -r member member_manifest member_version; do
  workspace_member_set["$member"]=1
  workspace_manifest_set["$member_manifest"]=1
  workspace_member_root["$member"]="$(cd "$(dirname "$member_manifest")" && pwd -P)"
  workspace_member_version["$member"]="$member_version"
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
    | [.name, .manifest_path, .version]
    | @tsv
  ' <<<"$metadata"
)

for expected in "${expected_members[@]}"; do
  if [[ -z "${workspace_member_set[$expected]-}" ]]; then
    printf 'workspace violation: expected member %s is missing\n' "$expected" >&2
    failed=1
  fi
done

workspace_root="$(jq -r '.workspace_root' <<<"$metadata")"
target_directory="$(jq -r '.target_directory' <<<"$metadata")"
if [[ -z "$target_directory" || "$target_directory" == null ]]; then
  printf 'Cargo metadata did not report a target directory\n' >&2
  exit 2
fi
while IFS= read -r -d '' package_manifest; do
  if [[ -z "${workspace_manifest_set[$package_manifest]-}" ]]; then
    printf 'workspace violation: unclassified package manifest is outside the workspace: %s\n' \
      "$package_manifest" >&2
    failed=1
  fi
done < <(
  find "$workspace_root" \
    -path "$target_directory" -prune -o \
    -type f -name Cargo.toml \
    ! -path "$workspace_root/Cargo.toml" -print0
)

declare -A direct_dependencies
while IFS=$'\t' read -r package dependency requirement dependency_path; do
  direct_dependencies["$package"]+=" $dependency"
  if [[ -n "${workspace_member_set[$dependency]-}" ]]; then
    expected_requirement="=${workspace_member_version[$dependency]}"
    if [[ -z "$dependency_path" ]]; then
      printf 'workspace dependency violation: %s must use the local workspace member %s\n' \
        "$package" "$dependency" >&2
      failed=1
    else
      resolved_dependency_path="$(cd "$dependency_path" && pwd -P)"
      if [[ "$resolved_dependency_path" != "${workspace_member_root[$dependency]}" ]]; then
        printf 'workspace dependency violation: %s redirects %s to %s (expected %s)\n' \
          "$package" \
          "$dependency" \
          "$resolved_dependency_path" \
          "${workspace_member_root[$dependency]}" >&2
        failed=1
      fi
    fi
    if [[ "$requirement" != "$expected_requirement" ]]; then
      printf 'workspace dependency violation: %s requires %s with %s (expected %s)\n' \
        "$package" "$dependency" "$requirement" "$expected_requirement" >&2
      failed=1
    fi
  fi
done < <(
  jq -r '
    .workspace_members as $members
    | .packages[]
    | select(.id as $id | $members | index($id))
    | .name as $package
    | .dependencies[]
    | [$package, .name, .req, (.path // "")]
    | @tsv
  ' <<<"$metadata"
)

declare -A allowed_internal_dependencies=()
for package in "${expected_members[@]}"; do
  allowed_internal_dependencies["$package"]=" $(
    jq -r --arg package "$package" \
      '.members[$package].allowed_workspace_dependencies | join(" ")' "$policy"
  ) "
done

declare -A dependency_free_member_set=()
while IFS= read -r package; do
  dependency_free_member_set["$package"]=1
done < <(jq -r '.rules.dependency_free_members[]' "$policy")

pingora_dependency_owner="$(jq -r '.rules.pingora_dependency_owner' "$policy")"

report_path() {
  local package="$1"
  local dependency="$2"
  printf '  reverse dependency path: %s -> %s\n' "$package" "$dependency" >&2
}

for package in "${!direct_dependencies[@]}"; do
  for dependency in ${direct_dependencies[$package]}; do
    if [[ -n "${dependency_free_member_set[$package]-}" ]]; then
      printf 'boundary violation: %s must remain dependency-free (found %s)\n' \
        "$package" \
        "$dependency" >&2
      report_path "$package" "$dependency"
      failed=1
    fi
    if [[ -n "${workspace_member_set[$dependency]-}" \
      && "${allowed_internal_dependencies[$package]- }" != *" $dependency "* ]]; then
      printf 'boundary violation: %s is not allowed to depend on workspace member %s\n' \
        "$package" "$dependency" >&2
      report_path "$package" "$dependency"
      failed=1
    fi
    if [[ ("$dependency" == pingora || "$dependency" == pingora-*) \
      && "$package" != "$pingora_dependency_owner" ]]; then
      printf 'boundary violation: only %s may depend on %s\n' \
        "$pingora_dependency_owner" "$dependency" >&2
      report_path "$package" "$dependency"
      failed=1
    fi
  done
done

protected=()
while IFS= read -r package; do
  protected+=("$package")
done < <(jq -r '.rules.engine_transport_storage_neutral_members[]' "$policy")

forbidden_neutral_dependencies=()
while IFS= read -r dependency; do
  forbidden_neutral_dependencies+=("$dependency")
done < <(jq -r '.rules.forbidden_neutral_dependencies[]' "$policy")

for package in "${protected[@]}"; do
  for dependency in ${direct_dependencies[$package]-}; do
    for forbidden_dependency in "${forbidden_neutral_dependencies[@]}"; do
      # Policy entries intentionally support globs such as pingora-*.
      # shellcheck disable=SC2053
      if [[ "$dependency" == $forbidden_dependency ]]; then
        printf 'boundary violation: %s must remain engine/transport/storage neutral\n' "$package" >&2
        report_path "$package" "$dependency"
        failed=1
        break
      fi
    done
  done
done

if [[ "$failed" -ne 0 ]]; then
  exit 1
fi

printf 'Panel dependency boundaries verified.\n'
