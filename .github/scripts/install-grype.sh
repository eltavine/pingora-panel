#!/usr/bin/env bash
set -euo pipefail

readonly grype_version="0.120.1"

if [[ $# -gt 1 ]]; then
  printf 'usage: %s [install-directory]\n' "${BASH_SOURCE[0]}" >&2
  exit 2
fi

case "$(uname -s):$(uname -m)" in
  Linux:x86_64)
    platform="linux_amd64"
    sha256="0a9ee97ef5ae2ee953b0a80098105052e846cdbe319a57d808b519c33cd1343d"
    ;;
  Linux:aarch64|Linux:arm64)
    platform="linux_arm64"
    sha256="29f47391dc283aa79fcc38e65224cd61f64dec0ecfd0db7074128ebf8ff23514"
    ;;
  Darwin:arm64)
    platform="darwin_arm64"
    sha256="cf97957fa467d25575ec2cc3228289f391ea51cf88b9cffc5f83dc03d4cbc732"
    ;;
  Darwin:x86_64)
    platform="darwin_amd64"
    sha256="5313004ccbc524c8757521dc3913f1edf4309f56bef06a2fb2d0c0eeade7cc62"
    ;;
  *)
    printf 'unsupported Grype installer platform: %s %s\n' \
      "$(uname -s)" "$(uname -m)" >&2
    exit 2
    ;;
esac

archive_name="grype_${grype_version}_${platform}.tar.gz"
download_url="https://github.com/anchore/grype/releases/download/v${grype_version}/${archive_name}"
install_directory="${1:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}/grype-bin}"
temporary_directory="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/grype-install.XXXXXX")"
trap 'rm -rf -- "$temporary_directory"' EXIT

curl --fail --location --retry 3 --retry-all-errors --connect-timeout 30 \
  --proto '=https' --tlsv1.2 \
  --output "$temporary_directory/$archive_name" \
  "$download_url"

if [[ "$(uname -s)" == Linux ]] && command -v sha256sum >/dev/null 2>&1; then
  printf '%s  %s\n' "$sha256" "$temporary_directory/$archive_name" \
    | sha256sum -c >/dev/null
elif command -v shasum >/dev/null 2>&1; then
  printf '%s  %s\n' "$sha256" "$temporary_directory/$archive_name" \
    | shasum -a 256 -c >/dev/null
else
  printf 'neither sha256sum nor shasum is available\n' >&2
  exit 2
fi

tar --extract --gzip --file "$temporary_directory/$archive_name" \
  --directory "$temporary_directory"
mkdir -p "$install_directory"
install -m 0755 "$temporary_directory/grype" "$install_directory/grype"

installed_version="$("$install_directory/grype" --version)"
if [[ "$installed_version" != "grype 0.120.1" ]]; then
  printf 'installed Grype version mismatch: expected %s, got %s\n' \
    "grype 0.120.1" "${installed_version:-unknown}" >&2
  exit 1
fi
