#!/usr/bin/env bash
set -euo pipefail

# The vendored upstream workspace intentionally does not commit Cargo.lock.
# Keep every security lane on the same patched dependency floor before
# cargo-deny resolves or reports the graph.
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

if [[ "${1:-Cargo.toml}" != "Cargo.toml" ]]; then
  printf 'This helper only prepares the untracked upstream Cargo.lock.\n' >&2
  exit 2
fi

cargo generate-lockfile --manifest-path Cargo.toml --ignore-rust-version
if rg --quiet '^name = "time"$' Cargo.lock; then
  cargo update --manifest-path Cargo.toml -p time --precise 0.3.47 --ignore-rust-version
fi
