#!/usr/bin/env bash
set -euo pipefail

if [[ $# -gt 1 ]]; then
  printf 'usage: %s [repository-root]\n' "${BASH_SOURCE[0]}" >&2
  exit 2
fi

script_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
if [[ $# -eq 1 ]]; then
  repo_root="$(cd "$1" && pwd -P)"
else
  repo_root="$script_root"
fi

scan_roots=("$repo_root/.github/workflows")
if [[ -d "$repo_root/.github/actions" ]]; then
  scan_roots+=("$repo_root/.github/actions")
fi

if [[ ! -d "${scan_roots[0]}" ]]; then
  printf 'workflow directory does not exist: %s\n' "${scan_roots[0]}" >&2
  exit 2
fi
if ! command -v rg >/dev/null 2>&1; then
  printf 'required command is unavailable: rg\n' >&2
  exit 2
fi
if ! command -v python3 >/dev/null 2>&1; then
  printf 'required command is unavailable: python3\n' >&2
  exit 2
fi

configuration_found=0
while IFS= read -r configuration; do
  if [[ -n "$configuration" ]]; then
    configuration_found=1
    break
  fi
done < <(rg --files --glob '*.yml' --glob '*.yaml' "${scan_roots[@]}")
if [[ "$configuration_found" -eq 0 ]]; then
  printf 'no workflow or composite Action YAML files were found\n' >&2
  exit 2
fi

failed=0
readonly uses_key_pattern="['\"]?\\buses\\b['\"]?[[:space:]]*:"
readonly uses_value_pattern="\"[^\"]*\"|'[^']*'|[^[:space:],}\\]]+"
readonly uses_declaration_pattern="${uses_key_pattern}[[:space:]]*(${uses_value_pattern})"

validate_action() {
  local file="$1"
  local line="$2"
  local action="$3"
  local digest
  local reference

  action="${action#"${action%%[![:space:]]*}"}"
  action="${action%"${action##*[![:space:]]}"}"
  if [[ "$action" == \"* || "$action" == *\" ]]; then
    if [[ "$action" != \"*\" ]]; then
      printf 'malformed quoted Action reference: %s:%s: %s\n' "$file" "$line" "$action" >&2
      failed=1
      return
    fi
    action="${action:1:${#action}-2}"
  elif [[ "$action" == \'* || "$action" == *\' ]]; then
    if [[ "$action" != \'*\' ]]; then
      printf 'malformed quoted Action reference: %s:%s: %s\n' "$file" "$line" "$action" >&2
      failed=1
      return
    fi
    action="${action:1:${#action}-2}"
  fi
  if [[ "$action" == ./* ]]; then
    return
  fi
  if [[ "$action" == docker://* ]]; then
    digest="${action##*@}"
    if [[ ! "$digest" =~ ^sha256:[0-9a-f]{64}$ ]]; then
      printf 'mutable Docker Action reference: %s:%s: %s\n' \
        "$file" "$line" "$action" >&2
      failed=1
    fi
    return
  fi
  reference="${action##*@}"
  if [[ "$action" != *@* || ! "$reference" =~ ^[0-9a-f]{40}$ ]]; then
    printf 'mutable GitHub Action reference: %s:%s: %s\n' "$file" "$line" "$action" >&2
    failed=1
  fi
}

strip_yaml_comment() {
  local input="$1"
  local output=""
  local quote=""
  local escaped=0
  local character
  local previous=""
  local index

  for ((index = 0; index < ${#input}; index++)); do
    character="${input:index:1}"
    if [[ "$escaped" -eq 1 ]]; then
      output+="$character"
      escaped=0
      previous="$character"
      continue
    fi
    if [[ "$character" == \\ && "$quote" == '"' ]]; then
      output+="$character"
      escaped=1
      previous="$character"
      continue
    fi
    if [[ "$character" == "'" && "$quote" != '"' ]]; then
      if [[ "$quote" == "'" ]]; then
        quote=""
      else
        quote="'"
      fi
    elif [[ "$character" == '"' && "$quote" != "'" ]]; then
      if [[ "$quote" == '"' ]]; then
        quote=""
      else
        quote='"'
      fi
    elif [[ "$character" == '#' && -z "$quote" \
      && ( "$index" -eq 0 || "$previous" =~ [[:space:]] ) ]]; then
      break
    fi
    output+="$character"
    previous="$character"
  done
  printf '%s' "$output"
}

extract_action() {
  local declaration="$1"
  local action

  action="${declaration#*uses}"
  action="${action#\"}"
  action="${action#\'}"
  action="${action#*:}"
  printf '%s' "$action"
}

validate_declarations() {
  local file="$1"
  local line="$2"
  local declaration
  local action_declaration

  declaration="$(strip_yaml_comment "$3")"

  while IFS= read -r action_declaration; do
    validate_action "$file" "$line" "$(extract_action "$action_declaration")"
  done < <(rg --only-matching "$uses_declaration_pattern" <<<"$declaration")
}

while IFS=: read -r file line declaration; do
  validate_declarations "$file" "$line" "$declaration"
done < <(
  rg \
    --no-heading \
    --line-number \
    --glob '*.yml' \
    --glob '*.yaml' \
    "^[[:space:]]*(-[[:space:]]+)?${uses_declaration_pattern}" \
    "${scan_roots[@]}" || true
)

while IFS=: read -r file line declaration; do
  validate_declarations "$file" "$line" "$declaration"
done < <(
  rg \
    --no-heading \
    --line-number \
    --glob '*.yml' \
    --glob '*.yaml' \
    "^[[:space:]]*(?:-[[:space:]]*|['\"]?[A-Za-z0-9_-]+['\"]?[[:space:]]*:[[:space:]]*(?:\\[[[:space:]]*)?)\\{.*${uses_declaration_pattern}" \
    "${scan_roots[@]}" || true
)

if ! python3 "$script_root/.github/scripts/check-local-actions.py" "$repo_root"; then
  failed=1
fi

if [[ "$failed" -ne 0 ]]; then
  exit 1
fi

printf 'All external GitHub Actions are pinned to immutable commit SHAs.\n'
