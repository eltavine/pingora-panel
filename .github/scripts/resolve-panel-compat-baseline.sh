#!/usr/bin/env bash
set -euo pipefail

event_name="${1:-}"
push_before="${2:-}"
pull_request_base="${3:-}"
default_branch="${4:-}"

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
    if [[ "$push_before" =~ ^0+$ ]]; then
      if [[ -z "$default_branch" ]]; then
        printf 'a newly created ref requires the repository default branch\n' >&2
        exit 2
      fi
      printf 'origin/%s\n' "$default_branch"
    else
      printf '%s\n' "$push_before"
    fi
    ;;
  *)
    printf 'unsupported event for compatibility baseline resolution: %s\n' "$event_name" >&2
    exit 2
    ;;
esac
