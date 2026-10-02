#!/usr/bin/env bash
# Disposable PostgreSQL and NATS JetStream servers for local integration tests.
#
#   panel/scripts/dev-services.sh up     start both servers
#   eval "$(panel/scripts/dev-services.sh env)"
#   panel/scripts/dev-services.sh down   stop them and delete all data
#
# Data lives under PANEL_DEV_SERVICES_DIR and is deleted by `down`. Durability
# settings are relaxed because the data is throwaway.
set -euo pipefail

root="${PANEL_DEV_SERVICES_DIR:-${TMPDIR:-/tmp}/pingora-panel-dev-services}"
pg_port="${PANEL_DEV_PG_PORT:-55432}"
nats_port="${PANEL_DEV_NATS_PORT:-54222}"
pg_password="${PANEL_DEV_PG_PASSWORD:-panel-dev-superuser}"

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

start_nats() {
  require nats-server
  if [[ -f "$root/nats.pid" ]] && kill -0 "$(cat "$root/nats.pid")" 2>/dev/null; then
    return
  fi
  mkdir -p "$root/nats"
  nohup nats-server --jetstream --store_dir "$root/nats" --addr 127.0.0.1 \
    --port "$nats_port" --pid "$root/nats.pid" --log "$root/nats.log" >/dev/null 2>&1 &
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
    printf 'PostgreSQL on 127.0.0.1:%s and NATS on 127.0.0.1:%s (data in %s)\n' \
      "$pg_port" "$nats_port" "$root"
    ;;
  env)
    printf 'export PANEL_TEST_DATABASE_URL=%q\n' \
      "postgres://postgres:${pg_password}@127.0.0.1:${pg_port}/postgres"
    printf 'export PANEL_TEST_NATS_URL=%q\n' "nats://127.0.0.1:${nats_port}"
    ;;
  down)
    if [[ -d "$root/pg" ]]; then
      pg_ctl --pgdata="$root/pg" --mode=fast stop >/dev/null 2>&1 || true
    fi
    if [[ -f "$root/nats.pid" ]]; then
      kill "$(cat "$root/nats.pid")" 2>/dev/null || true
    fi
    rm -rf "$root"
    ;;
  *)
    printf 'usage: %s up|env|down\n' "${BASH_SOURCE[0]}" >&2
    exit 2
    ;;
esac
