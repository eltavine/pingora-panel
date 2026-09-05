#!/usr/bin/env bash
set -euo pipefail

readonly upstream_url="https://github.com/cloudflare/pingora.git"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
readonly repo_root
operation_root="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/pingora-panel-canary.XXXXXX")"
readonly operation_root
trap 'rm -rf -- "$operation_root"' EXIT

upstream_sha="$(git ls-remote "$upstream_url" refs/heads/main | awk '{print $1}')"
readonly upstream_sha
if [[ ! "$upstream_sha" =~ ^[0-9a-f]{40}$ ]]; then
  printf 'Unable to resolve Cloudflare Pingora main to an immutable commit.\n' >&2
  exit 2
fi

upstream_root="$operation_root/pingora"
workspace_root="$operation_root/workspace"
mkdir -p "$workspace_root"
git init --quiet "$upstream_root"
git -C "$upstream_root" remote add origin "$upstream_url"
git -C "$upstream_root" fetch --quiet --depth=1 origin "$upstream_sha"
git -C "$upstream_root" checkout --quiet --detach "$upstream_sha"

git -C "$repo_root" archive HEAD panel .cargo | tar -x -C "$workspace_root"
ln -s "$upstream_root/Cargo.toml" "$workspace_root/Cargo.toml"
for package_path in "$upstream_root"/pingora-* "$upstream_root/tinyufo"; do
  [[ -d "$package_path" ]] || continue
  ln -s "$package_path" "$workspace_root/$(basename "$package_path")"
done

# The canary intentionally tests the current upstream package version, while
# the product workspace keeps exact versions for its pinned production baseline.
# Removing only the path-dependency version constraints in this disposable copy
# preserves the real Panel manifest and all stable ports.
python3 - "$workspace_root/panel/Cargo.toml" <<'PY'
from pathlib import Path
import re
import sys

manifest = Path(sys.argv[1])
text = manifest.read_text()
text, replacements = re.subn(
    r'(pingora-(?:core|http|load-balancing)\s*= \{) version = "=[^"]+", ',
    r'\1',
    text,
)
if replacements != 3:
    raise SystemExit(f"expected three Panel Pingora path dependencies, found {replacements}")
manifest.write_text(text)
PY

printf 'Testing Panel adapter against Cloudflare Pingora main at %s\n' "$upstream_sha"
cargo check \
  --manifest-path "$workspace_root/panel/Cargo.toml" \
  --package gateway-pingora \
  --package gatewayd \
  --all-targets \
  --all-features
cargo test \
  --manifest-path "$workspace_root/panel/Cargo.toml" \
  --package gateway-pingora \
  --package gatewayd \
  --all-features
