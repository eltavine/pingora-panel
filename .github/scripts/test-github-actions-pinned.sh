#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
checker="$repo_root/.github/scripts/check-github-actions-pinned.sh"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/github-actions-pinned.XXXXXX")"
trap 'rm -rf -- "$test_root"' EXIT

readonly commit_sha="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
readonly image_digest="sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"

create_fixture() {
  local root="$1"
  local workflow_action="$2"
  local composite_action="$3"

  mkdir -p "$root/.github/workflows" "$root/.github/actions/example"
  printf \
    'jobs:\n  check:\n    steps:\n      - uses: %s\n      - uses: "./.github/actions/example"\n' \
    "$workflow_action" >"$root/.github/workflows/check.yml"
  printf \
    'name: Example\nruns:\n  using: composite\n  steps:\n    - uses: %s\n' \
    "$composite_action" >"$root/.github/actions/example/action.yml"
}

assert_accepted() {
  local root="$1"
  local scenario="$2"
  if ! bash "$checker" "$root" >/dev/null 2>&1; then
    printf '%s was rejected\n' "$scenario" >&2
    exit 1
  fi
}

assert_rejected() {
  local root="$1"
  local scenario="$2"
  if bash "$checker" "$root" >/dev/null 2>&1; then
    printf '%s was not rejected\n' "$scenario" >&2
    exit 1
  fi
}

immutable_fixture="$test_root/immutable"
create_fixture \
  "$immutable_fixture" \
  "actions/checkout@$commit_sha" \
  "docker://example/image@$image_digest"
assert_accepted "$immutable_fixture" "immutable workflow and composite Action references"
printf \
  '      # uses: actions/cache@main\n      - run: echo "{ uses: actions/cache@main }"\n' \
  >>"$immutable_fixture/.github/workflows/check.yml"
assert_accepted "$immutable_fixture" "commented and command-text Action references"

flow_fixture="$test_root/flow"
create_fixture \
  "$flow_fixture" \
  "actions/checkout@$commit_sha" \
  "actions/cache@$commit_sha"
printf \
  'jobs:\n  check:\n    steps: [{ uses: "actions/checkout@%s", name: Checkout }]\n' \
  "$commit_sha" >"$flow_fixture/.github/workflows/check.yml"
assert_accepted "$flow_fixture" "immutable flow-style Action reference"
printf \
  'jobs:\n  check:\n    steps:\n      - "uses" : "actions/checkout@%s"\n' \
  "$commit_sha" >"$flow_fixture/.github/workflows/check.yml"
assert_accepted "$flow_fixture" "immutable Action behind a quoted, spaced key"

mutable_workflow_fixture="$test_root/mutable-workflow"
create_fixture \
  "$mutable_workflow_fixture" \
  "actions/checkout@v4" \
  "actions/cache@$commit_sha"
assert_rejected "$mutable_workflow_fixture" "mutable workflow Action reference"

embedded_hash_fixture="$test_root/embedded-hash"
create_fixture \
  "$embedded_hash_fixture" \
  "actions/checkout@$commit_sha" \
  "actions/cache@$commit_sha"
printf \
  'jobs:\n  check:\n    steps:\n      - uses: "actions/checkout@%s#mutable"\n' \
  "$commit_sha" >"$embedded_hash_fixture/.github/workflows/check.yml"
assert_rejected "$embedded_hash_fixture" "embedded hash suffix in a quoted Action reference"

mismatched_quote_fixture="$test_root/mismatched-quote"
create_fixture \
  "$mismatched_quote_fixture" \
  "actions/checkout@$commit_sha" \
  "actions/cache@$commit_sha"
printf \
  'jobs:\n  check:\n    steps:\n      - uses: "actions/checkout@%s\n' \
  "$commit_sha" >"$mismatched_quote_fixture/.github/workflows/check.yml"
assert_rejected "$mismatched_quote_fixture" "mismatched Action reference quote"

mutable_composite_fixture="$test_root/mutable-composite"
create_fixture \
  "$mutable_composite_fixture" \
  "actions/checkout@$commit_sha" \
  "actions/cache@main"
assert_rejected "$mutable_composite_fixture" "mutable composite Action reference"

mutable_flow_fixture="$test_root/mutable-flow"
create_fixture \
  "$mutable_flow_fixture" \
  "actions/checkout@$commit_sha" \
  "actions/cache@$commit_sha"
printf \
  'jobs:\n  check:\n    steps: [{ name: Checkout, uses: actions/checkout@v4 }]\n' \
  >"$mutable_flow_fixture/.github/workflows/check.yml"
assert_rejected "$mutable_flow_fixture" "mutable flow-style Action reference"

multiple_flow_fixture="$test_root/multiple-flow"
create_fixture \
  "$multiple_flow_fixture" \
  "actions/checkout@$commit_sha" \
  "actions/cache@$commit_sha"
printf \
  'jobs:\n  check:\n    steps: [{ uses: actions/checkout@%s }, { uses: actions/cache@main }]\n' \
  "$commit_sha" >"$multiple_flow_fixture/.github/workflows/check.yml"
assert_rejected "$multiple_flow_fixture" "later mutable Action in a flow-style list"

later_flow_fixture="$test_root/later-flow"
create_fixture \
  "$later_flow_fixture" \
  "actions/checkout@$commit_sha" \
  "actions/cache@$commit_sha"
printf \
  'jobs:\n  check:\n    steps: [{ name: NoAction }, { uses: actions/cache@main }]\n' \
  >"$later_flow_fixture/.github/workflows/check.yml"
assert_rejected "$later_flow_fixture" "mutable Action after a non-Action flow mapping"

quoted_hash_fixture="$test_root/quoted-hash"
create_fixture \
  "$quoted_hash_fixture" \
  "actions/checkout@$commit_sha" \
  "actions/cache@$commit_sha"
printf \
  'jobs:\n  check:\n    steps: [{ name: "# retained" }, { uses: actions/cache@main }]\n' \
  >"$quoted_hash_fixture/.github/workflows/check.yml"
assert_rejected "$quoted_hash_fixture" "mutable Action after a quoted hash"
printf \
  'jobs:\n  check:\n    steps: [{ name: NoAction }] # { uses: actions/cache@main }\n' \
  >"$quoted_hash_fixture/.github/workflows/check.yml"
assert_accepted "$quoted_hash_fixture" "commented flow Action reference"

quoted_parent_fixture="$test_root/quoted-parent"
create_fixture \
  "$quoted_parent_fixture" \
  "actions/checkout@$commit_sha" \
  "actions/cache@$commit_sha"
printf \
  'jobs:\n  check:\n    "steps" : [{ "uses": actions/checkout@v4 }]\n' \
  >"$quoted_parent_fixture/.github/workflows/check.yml"
assert_rejected "$quoted_parent_fixture" "mutable flow Action behind a quoted parent key"
printf \
  'jobs:\n  check:\n    "steps" : [{ "uses": "actions/checkout@%s" }]\n' \
  "$commit_sha" >"$quoted_parent_fixture/.github/workflows/check.yml"
assert_accepted "$quoted_parent_fixture" "immutable flow Action behind a quoted parent key"

quoted_key_fixture="$test_root/quoted-key"
create_fixture \
  "$quoted_key_fixture" \
  "actions/checkout@$commit_sha" \
  "actions/cache@$commit_sha"
printf \
  'jobs:\n  check:\n    steps:\n      - "uses" : actions/checkout@v4\n' \
  >"$quoted_key_fixture/.github/workflows/check.yml"
assert_rejected "$quoted_key_fixture" "mutable Action behind a quoted, spaced key"

unpinned_fixture="$test_root/unpinned"
create_fixture \
  "$unpinned_fixture" \
  "actions/checkout" \
  "actions/cache@$commit_sha"
assert_rejected "$unpinned_fixture" "external Action reference without a revision"

tagged_image_fixture="$test_root/tagged-image"
create_fixture \
  "$tagged_image_fixture" \
  "actions/checkout@$commit_sha" \
  "docker://example/image:latest"
assert_rejected "$tagged_image_fixture" "mutable Docker Action image tag"

missing_local_fixture="$test_root/missing-local"
create_fixture \
  "$missing_local_fixture" \
  "actions/checkout@$commit_sha" \
  "actions/cache@$commit_sha"
sed -i.bak 's#./.github/actions/example#./.github/actions/missing#' \
  "$missing_local_fixture/.github/workflows/check.yml"
rm -f "$missing_local_fixture/.github/workflows/check.yml.bak"
assert_rejected "$missing_local_fixture" "missing local Action target"

traversal_local_fixture="$test_root/traversal-local"
create_fixture \
  "$traversal_local_fixture" \
  "actions/checkout@$commit_sha" \
  "actions/cache@$commit_sha"
sed -i.bak 's#./.github/actions/example#./.github/actions/../actions/example#' \
  "$traversal_local_fixture/.github/workflows/check.yml"
rm -f "$traversal_local_fixture/.github/workflows/check.yml.bak"
assert_rejected "$traversal_local_fixture" "non-canonical local Action traversal"

ambiguous_local_fixture="$test_root/ambiguous-local"
create_fixture \
  "$ambiguous_local_fixture" \
  "actions/checkout@$commit_sha" \
  "actions/cache@$commit_sha"
cp "$ambiguous_local_fixture/.github/actions/example/action.yml" \
  "$ambiguous_local_fixture/.github/actions/example/action.yaml"
assert_rejected "$ambiguous_local_fixture" "ambiguous local Action manifests"

escaped_local_fixture="$test_root/escaped-local"
create_fixture \
  "$escaped_local_fixture" \
  "actions/checkout@$commit_sha" \
  "actions/cache@$commit_sha"
mkdir -p "$test_root/outside-action"
cp "$escaped_local_fixture/.github/actions/example/action.yml" \
  "$test_root/outside-action/action.yml"
rm -rf "$escaped_local_fixture/.github/actions/example"
ln -s "$test_root/outside-action" "$escaped_local_fixture/.github/actions/example"
assert_rejected "$escaped_local_fixture" "local Action symlink escape"

empty_fixture="$test_root/empty"
mkdir -p "$empty_fixture/.github/workflows"
assert_rejected "$empty_fixture" "empty workflow tree"

printf 'GitHub Action pinning guard self-test passed.\n'
