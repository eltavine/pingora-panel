#!/usr/bin/env bash
# Runs pingora-panel through install, upgrade, rollback, restore and both
# uninstalls against a fake engine that records what it is asked to do, so
# the order of each switch and the state left behind are checked without
# containers.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
work=$(mktemp -d)
server=
cleanup() {
  [[ -z $server ]] || kill "$server" 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT

mkdir -p "$work/bin" "$work/fake/images" "$work/fake/running" "$work/ready"
for release in old:0.8.0 new:0.9.0 contracting:0.9.1 broken:0.9.2; do
  printf '%s\n' "${release#*:}" >"$work/fake/images/${release%%:*}"
done
: >"$work/ready/readyz"

cat >"$work/bin/docker" <<'FAKE'
#!/usr/bin/env bash
# A Docker that records each call and answers as a healthy installation.
set -euo pipefail
fake=$FAKE_DIR
printf '%s\n' "$*" >>"$fake/calls"
image_name() { local image=${1##*/}; printf '%s\n' "${image##*:}"; }
case "$1 ${2:-}" in
  "info "* | "info") [[ ${2:-} == --format ]] && printf '%s\n' "$fake"; exit 0 ;;
  "version "*) printf '27.3.1\n'; exit 0 ;;
  "pull "*) exit 0 ;;
  "image inspect")
    shift 2
    if [[ $1 == --format ]]; then
      printf '%s\n' "$(cat "$fake/images/$(image_name "$3")")"
    else
      [[ -f $fake/images/$(image_name "$1") ]]
    fi
    exit
    ;;
  "compose version")
    [[ ${3:-} == --short ]] && printf '2.29.7\n' || printf 'Docker Compose version v2.29.7\n'
    exit 0
    ;;
  "compose "*)
    shift
    while [[ $1 == -p || $1 == -f ]]; do shift 2; done
    case $1 in
      up)
        shift
        [[ ${1:-} == -d ]] && shift
        [[ ${1:-} == --no-deps ]] && shift
        services=("$@")
        [[ ${#services[@]} -gt 0 ]] || services=(control gatewayd pki bootstrap pki-init nats)
        for service in "${services[@]}"; do
          printf '%s\n' "$PINGORA_PANEL_IMAGE" >"$fake/running/$service"
        done
        printf 'up %s %s\n' "$(image_name "$PINGORA_PANEL_IMAGE")" "${*:-all}" >>"$fake/switches"
        printf '%s\n' "$PINGORA_PANEL_DEPLOYMENT" >"$fake/deployment"
        ;;
      ps)
        if [[ ${2:-} == --format ]]; then
          [[ $(image_name "$(cat "$fake/running/control")") == broken ]] && echo unhealthy || echo healthy
        fi
        ;;
      config) printf '%s\n' nats bootstrap pki-init pki gatewayd control ;;
      stop) printf 'stop %s\n' "$2" >>"$fake/switches" ;;
      create | down) ;;
    esac
    exit 0
    ;;
  "run "*)
    image=
    for argument in "$@"; do
      [[ $argument == localhost/* ]] && image=$(image_name "$argument")
    done
    case "$*" in
      *"panel-control protocols"*)
        printf '[{"name":"pingora.panel.config.v1","min_revision":1,"max_revision":1}]\n'
        ;;
      *"panel-control preflight"*)
        cat >/dev/null
        printf 'preflight %s\n' "$image" >>"$fake/switches"
        [[ $image != contracting ]] || exit 3
        ;;
      *"panel-control restore"*)
        printf 'restore %s\n' "$image" >>"$fake/switches"
        printf 'installed the config database\ndesired_revision=7\ndesired_hash=sha256:aa\n'
        ;;
      *"ppanel backup create"*)
        printf '{\n  "id": "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b",\n  "state": "completed"\n}\n'
        ;;
      *"ppanel backup download"*) printf 'an archive' ;;
      *"ppanel system verify"*)
        printf 'verify %s %s\n' "$image" "$*" >>"$fake/switches"
        [[ $image != broken ]]
        ;;
      *"ppanel system"*) ;;
    esac
    exit
    ;;
esac
exit 0
FAKE
chmod +x "$work/bin/docker"

port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
python3 -m http.server "$port" --bind 127.0.0.1 --directory "$work/ready" >/dev/null 2>&1 &
server=$!
for _ in $(seq 50); do
  (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null && break
  sleep 0.1
done

export PATH="$work/bin:$PATH" FAKE_DIR="$work/fake"
export PINGORA_PANEL_STATE_DIR="$work/etc" PINGORA_PANEL_BACKUP_DIR="$work/backups"
export PINGORA_PANEL_SECRETS_DIR="$work/secrets" PINGORA_PANEL_GATEWAY_OPS="127.0.0.1:$port"
export PINGORA_PANEL_TIMEOUT=6 PINGORA_PANEL_AGENT_BIN="$work/ops-agent"
unset PPANEL_TOKEN
options=(--engine docker)
[[ $(uname -s) == Linux ]] || options+=(--ignore-preflight)

tool() { "$here/pingora-panel" "$@" "${options[@]}"; }
fail() {
  printf 'FAIL: %s\n' "$*" >&2
  exit 1
}
state() { sed -n "s/^$1=//p" "$PINGORA_PANEL_STATE_DIR/installation.env"; }
expect_state() { [[ $(state "$1") == "$2" ]] || fail "$1 is $(state "$1"), not $2"; }
switches() { cat "$work/fake/switches"; }
refused() {
  local expected=$1 output
  shift
  if output=$(tool "$@" 2>&1); then fail "$* was not refused"; fi
  [[ $output == *"$expected"* ]] || fail "$* said: $output"
}

tool install --image localhost/pingora-panel:old >/dev/null
expect_state IMAGE localhost/pingora-panel:old
expect_state RELEASE 0.8.0
expect_state ACTION install
for secret in bootstrap-token password-pepper master-keys; do
  [[ -s $work/secrets/$secret ]] || fail "install left no $secret"
done
grep -q '"action":"install","engine":"docker","project":"pingora-panel","images":\[{"service":"control"' \
  "$work/fake/deployment" || fail "the deployment record: $(cat "$work/fake/deployment")"
refused "runs here already" install --image localhost/pingora-panel:old

refused "PPANEL_TOKEN" upgrade --image localhost/pingora-panel:new
export PPANEL_TOKEN=ppat_test
: >"$work/fake/switches"
tool upgrade --image localhost/pingora-panel:new >/dev/null
expect_state IMAGE localhost/pingora-panel:new
expect_state PREVIOUS_IMAGE localhost/pingora-panel:old
expect_state RELEASE 0.9.0
expect_state PREVIOUS_RELEASE 0.8.0
expect_state CONTRACTED 0
backup=$(state BACKUP)
[[ -f $backup && $(cat "$backup") == "an archive" ]] || fail "no backup at $backup"
[[ $(stat -c %a "$backup" 2>/dev/null || stat -f %Lp "$backup") == 600 ]] || fail "the backup is readable by others"
[[ $(switches) == "preflight new
up new gatewayd
up new all
verify new"* ]] || fail "the upgrade switched: $(switches)"
grep -q '"previous":"0.8.0"' "$work/fake/deployment" || fail "the upgrade record names no previous release"

: >"$work/fake/switches"
tool rollback >/dev/null
expect_state IMAGE localhost/pingora-panel:old
expect_state PREVIOUS_IMAGE localhost/pingora-panel:new
[[ $(switches) == "up old nats bootstrap pki-init pki control
up old gatewayd" ]] || fail "the rollback switched: $(switches)"

tool upgrade --image localhost/pingora-panel:contracting >/dev/null
expect_state CONTRACTED 1
refused "roll back with --restore" rollback
: >"$work/fake/switches"
tool rollback --restore >/dev/null
[[ $(switches) == "stop control
restore old
up old nats bootstrap pki-init pki control
up old gatewayd" ]] || fail "the restoring rollback: $(switches)"
expect_state CONTRACTED 0

: >"$work/fake/switches"
refused "runs again" upgrade --image localhost/pingora-panel:broken
expect_state IMAGE localhost/pingora-panel:old
[[ $(switches) == *"up broken all"*"up old all" ]] || fail "the failed upgrade did not return: $(switches)"

refused "pass --yes as well" uninstall --purge
tool uninstall >/dev/null
[[ ! -f $PINGORA_PANEL_STATE_DIR/installation.env ]] || fail "uninstall left the state"
[[ -s $work/secrets/master-keys && -f $backup ]] || fail "uninstall removed data it keeps"
grep -q -- 'down --remove-orphans$' "$work/fake/calls" || fail "uninstall did not keep the volumes"

: >"$work/fake/switches"
tool restore "$backup" --image localhost/pingora-panel:new >/dev/null
expect_state ACTION restore
expect_state IMAGE localhost/pingora-panel:new
[[ $(switches) == "restore new
up new all
verify new"*"--revision 7 --hash sha256:aa"* ]] || fail "the restore: $(switches)"

tool uninstall --purge --yes >/dev/null
grep -q -- 'down --volumes --rmi all --remove-orphans' "$work/fake/calls" || fail "purge kept the volumes"
[[ ! -e $work/secrets && ! -e $PINGORA_PANEL_STATE_DIR && ! -e $work/backups ]] ||
  fail "purge left secrets, state or backups"
printf 'pingora-panel lifecycle verified against a fake engine\n'
