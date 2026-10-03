#!/usr/bin/env bash
# Applied migrations are immutable. sqlx refuses to start against a database
# whose recorded checksum differs from the migration file, and a removed or
# renamed migration strands every database that applied it. Migrations are
# also applied in version order, so a new one must not sort before one that
# was already published in the same directory.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
baseline_ref="${1:-}"
pattern='^panel/[^/]+/migrations/[0-9]+_[^/]+\.sql$'

if [[ -z "$baseline_ref" ]]; then
  printf 'usage: %s <git-baseline-ref>\n' "${BASH_SOURCE[0]}" >&2
  exit 2
fi
if [[ "$baseline_ref" =~ ^0+$ ]]; then
  printf 'No parent commit exists; skipping the migration comparison.\n'
  exit 0
fi
if ! git -C "$repo_root" cat-file -e "${baseline_ref}^{commit}" 2>/dev/null; then
  printf 'Migration baseline ref does not resolve to a commit: %s\n' "$baseline_ref" >&2
  exit 2
fi

published="$(git -C "$repo_root" ls-tree -r --name-only "$baseline_ref" -- panel | grep -E "$pattern" || true)"
current="$(cd "$repo_root" && git ls-files --cached --others --exclude-standard -- panel | grep -E "$pattern" || true)"

failures=0
while IFS= read -r path; do
  [[ -z "$path" ]] && continue
  if [[ ! -f "$repo_root/$path" ]]; then
    printf 'Published migration was removed or renamed: %s\n' "$path" >&2
    failures=$((failures + 1))
  elif [[ "$(git -C "$repo_root" rev-parse "$baseline_ref:$path")" != "$(git -C "$repo_root" hash-object "$repo_root/$path")" ]]; then
    printf 'Published migration was changed: %s\n' "$path" >&2
    failures=$((failures + 1))
  fi
done <<<"$published"

version() {
  local name="${1##*/}"
  printf '%s' "$((10#${name%%_*}))"
}

while IFS= read -r path; do
  [[ -z "$path" ]] && continue
  if grep -qxF "$path" <<<"$published"; then
    continue
  fi
  directory="${path%/*}"
  newest=-1
  while IFS= read -r other; do
    [[ -z "$other" || "${other%/*}" != "$directory" ]] && continue
    candidate="$(version "$other")"
    ((candidate > newest)) && newest="$candidate"
  done <<<"$published"
  if (($(version "$path") <= newest)); then
    printf 'New migration %s sorts before or at published version %s\n' "$path" "$newest" >&2
    failures=$((failures + 1))
  fi
done <<<"$current"

if ((failures > 0)); then
  printf '%d migration change(s) would break databases that applied the baseline.\n' "$failures" >&2
  exit 1
fi
printf 'Published migrations are unchanged and new ones sort after them.\n'
