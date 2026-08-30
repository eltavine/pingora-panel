#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
baseline_ref="${1:-}"

if [[ -z "$baseline_ref" ]]; then
  printf 'usage: %s <git-baseline-ref>\n' "${BASH_SOURCE[0]}" >&2
  exit 2
fi

# A zero before-SHA is GitHub's representation of a branch without a parent.
# Contract enforcement starts after the bootstrap commit introduces the module.
if [[ "$baseline_ref" =~ ^0+$ ]]; then
  printf 'No parent commit exists; skipping the Protobuf bootstrap comparison.\n'
  exit 0
fi

if ! git -C "$repo_root" cat-file -e "${baseline_ref}^{commit}" 2>/dev/null; then
  printf 'Proto baseline ref does not resolve to a commit: %s\n' "$baseline_ref" >&2
  exit 2
fi

if ! git -C "$repo_root" cat-file -e \
  "${baseline_ref}:panel/proto/common/v1/common.proto" 2>/dev/null; then
  printf 'No Proto module exists at %s; treating this as the bootstrap comparison.\n' \
    "$baseline_ref"
  exit 0
fi

cd "$repo_root/panel"
exec buf breaking \
  --against "$repo_root/.git#ref=$baseline_ref,subdir=panel"
