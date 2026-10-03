#!/usr/bin/env bash
# Disposable PostgreSQL, NATS JetStream and ACME servers for local
# integration tests. The ACME server is Pebble, Let's Encrypt's test CA, with
# its DNS test server resolving every name to 127.0.0.1.
#
#   panel/scripts/dev-services.sh up     start all servers
#   panel/scripts/dev-services.sh acme   start only the ACME servers
#   eval "$(panel/scripts/dev-services.sh env)"
#   panel/scripts/dev-services.sh down   stop them and delete all data
#
# Data lives under PANEL_DEV_SERVICES_DIR and is deleted by `down`. Durability
# settings are relaxed because the data is throwaway. Pebble's release
# binaries and test certificates are downloaded once into PANEL_DEV_TOOLS_DIR
# and checked against pinned SHA-256 digests.
set -euo pipefail

root="${PANEL_DEV_SERVICES_DIR:-${TMPDIR:-/tmp}/pingora-panel-dev-services}"
tools="${PANEL_DEV_TOOLS_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/pingora-panel/tools}"
pg_port="${PANEL_DEV_PG_PORT:-55432}"
nats_port="${PANEL_DEV_NATS_PORT:-54222}"
pg_password="${PANEL_DEV_PG_PASSWORD:-panel-dev-superuser}"
acme_port="${PANEL_DEV_ACME_PORT:-14000}"
acme_management_port="${PANEL_DEV_ACME_MANAGEMENT_PORT:-15000}"
# Pebble validates HTTP-01 challenges on this port of 127.0.0.1.
acme_http_port="${PANEL_DEV_ACME_HTTP_PORT:-5002}"
acme_dns_port="${PANEL_DEV_ACME_DNS_PORT:-8053}"
acme_dns_management_port="${PANEL_DEV_ACME_DNS_MANAGEMENT_PORT:-8055}"

pebble_version=v2.10.1
pebble_release="https://github.com/letsencrypt/pebble/releases/download/$pebble_version"
pebble_sources="https://raw.githubusercontent.com/letsencrypt/pebble/$pebble_version"
pebble_digest() {
  case "$1" in
    pebble-darwin-amd64) echo e670ff869886022637e077502a62e7f23be693c45a5a6727ebd76da8fdce64dc ;;
    pebble-darwin-arm64) echo 09a3a4e6ebed71e8d83294a26d361232262f45a7488f5de7bccb5887b395217f ;;
    pebble-linux-amd64) echo 4f2fcb5bca8c85c9cf73ad140fccfc0d2be40bd81ab99879c79b7b8a0b4f70ed ;;
    pebble-linux-arm64) echo b53fd072a69eb7692451de4e8b0667e0bdf5cccd7e36fc51b8eaf2fcc135ed9f ;;
    pebble-challtestsrv-darwin-amd64) echo 796bd923f2c595dd7bf15ae693096abfb1df962cb3673c7981ff306daa5c4a52 ;;
    pebble-challtestsrv-darwin-arm64) echo 59bf917fe39c96e2edca980fc2899f4f04aa1ce5485f28d419d18237b902cf82 ;;
    pebble-challtestsrv-linux-amd64) echo e93a5aa25ecdf3af2f9fbb2de32b0173e64a2eae81002a4ccfe35fa6f4f60b92 ;;
    pebble-challtestsrv-linux-arm64) echo db8e1a79ccdb2195c489fbe4f40fddb7f30e86f9cd8a07912566ee5025094d6c ;;
    cert.pem) echo c87fb918d9bac8db11aa54493b5f0cf6e0725a3fe73f617d1c64737e1ab4caf9 ;;
    key.pem) echo 0977979255b0e0721c17335c056b627f66a43cf56e5a52f3cda81fb111ecf00e ;;
    pebble.minica.pem) echo 0c502e52627ff7de972c7e9e065640103b4589b4e2d23d3fcf21bef1e3c8c67c ;;
    *) return 1 ;;
  esac
}
pebble_dir="$tools/pebble-$pebble_version"

require() {
  if ! command -v "$1" >/dev/null 2>&1; then
    printf 'required command is unavailable: %s\n' "$1" >&2
    exit 2
  fi
}

start_postgres() {
  require initdb
  require pg_ctl
  if [[ ! -d "$root/pg" ]]; then
    mkdir -p "$root"
    printf '%s\n' "$pg_password" >"$root/pg-password"
    initdb --pgdata="$root/pg" --username=postgres --pwfile="$root/pg-password" \
      --auth-local=trust --auth-host=scram-sha-256 --encoding=UTF8 --locale=C \
      >"$root/initdb.log"
    rm -f "$root/pg-password"
  fi
  if ! pg_ctl --pgdata="$root/pg" status >/dev/null 2>&1; then
    pg_ctl --pgdata="$root/pg" --log="$root/postgres.log" --wait start \
      -o "-p $pg_port -k $root -c listen_addresses=127.0.0.1 -c fsync=off -c synchronous_commit=off -c full_page_writes=off -c max_connections=300" \
      >/dev/null
  fi
}

# Runs a command in a new session so it outlives the invoking shell's process group.
detach() {
  if command -v setsid >/dev/null 2>&1; then
    setsid "$@" </dev/null >/dev/null 2>&1 &
  else
    perl -MPOSIX -e 'POSIX::setsid(); exec @ARGV or die "exec: $!"' "$@" </dev/null >/dev/null 2>&1 &
  fi
}

# Downloads $2 to $3 unless it is there, and checks it against the digest of $1.
fetch() {
  local expected actual
  expected="$(pebble_digest "$1")"
  if [[ ! -f "$3" ]]; then
    curl --fail --silent --show-error --location --output "$3.part" "$2"
    mv "$3.part" "$3"
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$3" | cut -d' ' -f1)"
  else
    actual="$(shasum -a 256 "$3" | cut -d' ' -f1)"
  fi
  if [[ "$actual" != "$expected" ]]; then
    rm -f "$3"
    printf '%s does not have the pinned SHA-256 digest\n' "$2" >&2
    exit 1
  fi
}

install_pebble() {
  require curl
  require tar
  local os arch name
  os="$(uname -s | tr '[:upper:]' '[:lower:]')"
  case "$(uname -m)" in
    x86_64 | amd64) arch=amd64 ;;
    arm64 | aarch64) arch=arm64 ;;
    *) printf 'Pebble has no release for %s\n' "$(uname -m)" >&2; exit 2 ;;
  esac
  mkdir -p "$pebble_dir"
  for name in pebble pebble-challtestsrv; do
    if [[ ! -x "$pebble_dir/$name" ]]; then
      fetch "$name-$os-$arch" "$pebble_release/$name-$os-$arch.tar.gz" "$pebble_dir/$name.tar.gz"
      tar -xzf "$pebble_dir/$name.tar.gz" -C "$pebble_dir" --strip-components 3 \
        "$name-$os-$arch/$os/$arch/$name"
      chmod +x "$pebble_dir/$name"
      rm "$pebble_dir/$name.tar.gz"
    fi
  done
  fetch cert.pem "$pebble_sources/test/certs/localhost/cert.pem" "$pebble_dir/cert.pem"
  fetch key.pem "$pebble_sources/test/certs/localhost/key.pem" "$pebble_dir/key.pem"
  fetch pebble.minica.pem "$pebble_sources/test/certs/pebble.minica.pem" "$pebble_dir/pebble.minica.pem"
}

running() {
  [[ -f "$1" ]] && kill -0 "$(cat "$1")" 2>/dev/null
}

wait_for_port() {
  for _ in $(seq 1 100); do
    if nc -z 127.0.0.1 "$1" 2>/dev/null; then
      return
    fi
    sleep 0.1
  done
  printf '%s did not start; see %s\n' "$2" "$3" >&2
  exit 1
}

start_acme() {
  install_pebble
  mkdir -p "$root/acme"
  if ! running "$root/acme/dns.pid"; then
    detach "$pebble_dir/pebble-challtestsrv" -defaultIPv4 127.0.0.1 -defaultIPv6 "" \
      -dnsserver "127.0.0.1:$acme_dns_port" -management "127.0.0.1:$acme_dns_management_port" \
      -http01 "" -https01 "" -tlsalpn01 "" -doh ""
    echo "$!" >"$root/acme/dns.pid"
    wait_for_port "$acme_dns_management_port" pebble-challtestsrv "$root/acme"
  fi
  if ! running "$root/acme/pebble.pid"; then
    cat >"$root/acme/pebble.json" <<EOF
{
  "pebble": {
    "listenAddress": "127.0.0.1:$acme_port",
    "managementListenAddress": "127.0.0.1:$acme_management_port",
    "certificate": "$pebble_dir/cert.pem",
    "privateKey": "$pebble_dir/key.pem",
    "httpPort": $acme_http_port,
    "tlsPort": 5001,
    "ocspResponderURL": "",
    "externalAccountBindingRequired": false
  }
}
EOF
    detach env PEBBLE_VA_NOSLEEP=1 PEBBLE_WFE_NONCEREJECT=0 "$pebble_dir/pebble" \
      -config "$root/acme/pebble.json" -dnsserver "127.0.0.1:$acme_dns_port"
    echo "$!" >"$root/acme/pebble.pid"
    wait_for_port "$acme_port" pebble "$root/acme"
  fi
}

start_nats() {
  require nats-server
  if [[ -f "$root/nats.pid" ]] && kill -0 "$(cat "$root/nats.pid")" 2>/dev/null; then
    return
  fi
  mkdir -p "$root/nats"
  detach nats-server --jetstream --store_dir "$root/nats" --addr 127.0.0.1 \
    --port "$nats_port" --pid "$root/nats.pid" --log "$root/nats.log"
  for _ in $(seq 1 50); do
    if [[ -f "$root/nats.pid" ]] && nc -z 127.0.0.1 "$nats_port" 2>/dev/null; then
      return
    fi
    sleep 0.1
  done
  printf 'nats-server did not start; see %s/nats.log\n' "$root" >&2
  exit 1
}

case "${1:-}" in
  up)
    start_postgres
    start_nats
    start_acme
    printf 'PostgreSQL on 127.0.0.1:%s, NATS on 127.0.0.1:%s and ACME on 127.0.0.1:%s (data in %s)\n' \
      "$pg_port" "$nats_port" "$acme_port" "$root"
    ;;
  acme)
    start_acme
    printf 'ACME on 127.0.0.1:%s (data in %s)\n' "$acme_port" "$root"
    ;;
  env)
    printf 'export PANEL_TEST_DATABASE_URL=%q\n' \
      "postgres://postgres:${pg_password}@127.0.0.1:${pg_port}/postgres"
    printf 'export PANEL_TEST_NATS_URL=%q\n' "nats://127.0.0.1:${nats_port}"
    printf 'export PANEL_TEST_ACME_DIRECTORY=%q\n' "https://127.0.0.1:${acme_port}/dir"
    printf 'export PANEL_TEST_ACME_CA=%q\n' "$pebble_dir/pebble.minica.pem"
    printf 'export PANEL_TEST_ACME_HTTP_PORT=%q\n' "$acme_http_port"
    printf 'export PANEL_TEST_ACME_DNS=%q\n' "http://127.0.0.1:${acme_dns_management_port}"
    ;;
  down)
    if [[ -d "$root/pg" ]]; then
      pg_ctl --pgdata="$root/pg" --mode=fast stop >/dev/null 2>&1 || true
    fi
    for pid in "$root/nats.pid" "$root/acme/pebble.pid" "$root/acme/dns.pid"; do
      if [[ -f "$pid" ]]; then
        kill "$(cat "$pid")" 2>/dev/null || true
      fi
    done
    rm -rf "$root"
    ;;
  *)
    printf 'usage: %s up|acme|env|down\n' "${BASH_SOURCE[0]}" >&2
    exit 2
    ;;
esac
