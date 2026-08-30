#!/usr/bin/env bash
set -euo pipefail

event_name="${1:-}"
push_before="${2:-}"
pull_request_base="${3:-}"

case "$event_name" in
  pull_request)
    if [[ -z "$pull_request_base" ]]; then
      printf 'pull_request requires a base branch\n' >&2
      exit 2
    fi
    printf 'origin/%s\n' "$pull_request_base"
    ;;
  push)
    if [[ -z "$push_before" ]]; then
      printf 'push requires the immutable before SHA\n' >&2
      exit 2
    fi
    printf '%s\n' "$push_before"
    ;;
  *)
    printf 'unsupported event for Proto baseline resolution: %s\n' "$event_name" >&2
    exit 2
    ;;
esac
