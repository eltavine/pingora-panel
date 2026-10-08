#!/usr/bin/env bash
# Writes the supply-chain evidence of Pingora Panel (ADR 0046):
#
#   supply-chain.sh sbom rust <file>            CycloneDX SBOM of the crates in Cargo.lock
#   supply-chain.sh sbom web <file>             CycloneDX SBOM of the console's packages
#   supply-chain.sh sbom image <image> <file>   CycloneDX SBOM of an image
#   supply-chain.sh scan <sbom> <report>        Grype's JSON report of an SBOM, for
#                                                .github/scripts/check-vulnerability-leases.py
#   supply-chain.sh notices <file>              the licenses of the crates and the console's
#                                                packages, with their texts
#
# syft, grype and cargo-about are installed at fixed versions by
# .github/scripts/install-{syft,grype,cargo-about}.sh; the console's packages
# must be installed for `notices`. PINGORA_PANEL_VERSION names the release in
# the SBOMs; the commit does otherwise.
set -euo pipefail

panel=$(cd "$(dirname "$0")/.." && pwd)
repository=$(dirname "$panel")
version=${PINGORA_PANEL_VERSION:-$(git -C "$repository" rev-parse --short=12 HEAD 2>/dev/null || echo dev)}

usage() {
  sed -n '2,14p' "$0" >&2
  exit 2
}

sbom() {
  local subject=${1:-} source name
  case "$subject" in
    rust)
      [ $# -eq 2 ] || usage
      source="file:$panel/Cargo.lock"
      name=pingora-panel
      ;;
    web)
      [ $# -eq 2 ] || usage
      source="file:$panel/web/pnpm-lock.yaml"
      name=pingora-panel-web
      ;;
    image)
      [ $# -eq 3 ] || usage
      source=$2
      name=pingora-panel-image
      shift
      ;;
    *) usage ;;
  esac
  syft scan "$source" --quiet --source-name "$name" --source-version "$version" \
    --output "cyclonedx-json=$2"
}

scan() {
  [ $# -eq 2 ] || usage
  grype "sbom:$1" --quiet --output json --file "$2"
}

notices() {
  [ $# -eq 1 ] || usage
  local output=$1 web
  web=$(mktemp)
  trap 'rm -f "$web"' RETURN
  (cd "$panel/web" && node scripts/third-party-notices.ts >"$web")
  cargo-about generate --locked --offline --manifest-path "$panel/Cargo.toml" \
    --config "$panel/about.toml" --output-file "$output" "$panel/about.hbs"
  cat "$web" >>"$output"
}

command=${1:-}
[ $# -gt 0 ] && shift
case "$command" in
  sbom) sbom "$@" ;;
  scan) scan "$@" ;;
  notices) notices "$@" ;;
  *) usage ;;
esac
