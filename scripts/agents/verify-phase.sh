#!/usr/bin/env sh
set -eu

if [ "${SWITCH_ENV:-}" != "development" ]; then
  echo "SWITCH_ENV=development is mandatory." >&2
  exit 1
fi

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/../.." && pwd)
verifier="$repo_root/sidecar/mass-subagents/scripts/verify-phase.mjs"

phase=${1:-}
if [ "$phase" != "0" ] && [ "$phase" != "A" ]; then
  echo "usage: verify-phase.sh 0|A [--seed N] [--scale N] [--artifacts-root PATH] [--foundation-only]" >&2
  exit 1
fi
shift

exec node "$verifier" --phase "$phase" "$@"
