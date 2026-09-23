#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if ! command -v oasdiff >/dev/null 2>&1; then
  printf 'oasdiff must be installed to self-test the OpenAPI guard\n' >&2
  exit 2
fi

test_root="$(mktemp -d "${TMPDIR:-/tmp}/pingora-panel-openapi-test.XXXXXX")"
trap 'rm -rf -- "$test_root"' EXIT
test_repo="$test_root/repository"
spec_dir="$test_repo/panel/panel-api/tests/fixtures"
mkdir -p "$test_repo/.github/scripts" "$spec_dir"
cp "$script_dir/check-panel-openapi-breaking.sh" "$test_repo/.github/scripts/"

cat >"$spec_dir/openapi.json" <<'EOF'
{
  "openapi": "3.1.0",
  "info": {"title": "Compatibility test", "version": "1"},
  "paths": {
    "/items": {
      "get": {
        "parameters": [{"name": "filter", "in": "query", "required": false, "schema": {"type": "string"}}],
        "responses": {
          "200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Item"}}}},
          "400": {"description": "invalid request"}
        }
      }
    }
  },
  "components": {"schemas": {"Item": {"type": "object", "properties": {"name": {"type": "string"}}}}}
}
EOF

git -C "$test_repo" init --quiet
git -C "$test_repo" config user.name "OpenAPI Compatibility Test"
git -C "$test_repo" config user.email "openapi-test@example.invalid"
git -C "$test_repo" commit --quiet --allow-empty -m 'test: before OpenAPI introduction'
bootstrap_ref="$(git -C "$test_repo" rev-parse HEAD)"
git -C "$test_repo" add panel
git -C "$test_repo" commit --quiet -m 'test: establish OpenAPI baseline'
baseline_ref="$(git -C "$test_repo" rev-parse HEAD)"

check() {
  bash "$test_repo/.github/scripts/check-panel-openapi-breaking.sh" "$1"
}

check 0000000000000000000000000000000000000000 >/dev/null
check "$bootstrap_ref" >/dev/null
check "$baseline_ref" >/dev/null
if check does-not-exist >"$test_root/result" 2>&1; then
  printf 'Invalid baseline was accepted\n' >&2
  exit 1
fi
if ! grep -q 'does not resolve' "$test_root/result"; then
  printf 'Invalid baseline did not produce the expected error\n' >&2
  exit 1
fi

mutate() {
  git -C "$test_repo" show "$baseline_ref:panel/panel-api/tests/fixtures/openapi.json" \
    >"$spec_dir/openapi.json"
  python3 - "$spec_dir/openapi.json" "$1" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
spec = json.loads(path.read_text())
operation = spec["paths"]["/items"]["get"]
case = sys.argv[2]
if case == "additive":
    spec["components"]["schemas"]["Item"]["properties"]["description"] = {"type": "string"}
elif case == "endpoint":
    del spec["paths"]["/items"]
elif case == "required":
    operation["parameters"][0]["required"] = True
elif case == "response":
    del operation["responses"]["200"]
elif case == "type":
    spec["components"]["schemas"]["Item"]["properties"]["name"]["type"] = "integer"
else:
    raise ValueError(case)
path.write_text(json.dumps(spec, indent=2) + "\n")
PY
}

mutate additive
check "$baseline_ref" >"$test_root/result"
for case in endpoint required response type; do
  mutate "$case"
  if check "$baseline_ref" >"$test_root/result" 2>&1; then
    printf 'Breaking OpenAPI change was accepted: %s\n' "$case" >&2
    exit 1
  fi
  if ! grep -q '^error' "$test_root/result"; then
    printf 'OpenAPI change failed for a reason other than a semantic break: %s\n' "$case" >&2
    cat "$test_root/result" >&2
    exit 1
  fi
done

# A missing current fixture is an error even on the first introduction.
mv "$spec_dir/openapi.json" "$test_root/saved.json"
for ref in "$baseline_ref" "$bootstrap_ref" 0000000000000000000000000000000000000000; do
  if check "$ref" >"$test_root/result" 2>&1; then
    printf 'Missing current OpenAPI fixture was accepted\n' >&2
    exit 1
  fi
  if ! grep -q 'Current OpenAPI fixture is missing' "$test_root/result"; then
    cat "$test_root/result" >&2
    exit 1
  fi
done
mv "$test_root/saved.json" "$spec_dir/openapi.json"

# Parser/reference errors must never look like a compatible contract.
printf '{invalid json' >"$spec_dir/openapi.json"
if check "$baseline_ref" >"$test_root/result" 2>&1; then
  printf 'Malformed OpenAPI fixture was accepted\n' >&2
  exit 1
fi
mutate additive
cp "$spec_dir/openapi.json" "$spec_dir/external.json"
python3 - "$spec_dir/openapi.json" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
spec = json.loads(path.read_text())
spec["paths"]["/items"]["get"]["responses"]["200"]["content"]["application/json"]["schema"] = {
    "$ref": "external.json#/components/schemas/Item"
}
path.write_text(json.dumps(spec) + "\n")
PY
if check "$baseline_ref" >"$test_root/result" 2>&1; then
  printf 'External OpenAPI reference was accepted\n' >&2
  exit 1
fi

printf 'Panel OpenAPI compatibility guard self-test passed.\n'
