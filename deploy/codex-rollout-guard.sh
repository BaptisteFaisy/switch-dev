#!/bin/sh
set -u

real_codex=${CST_CODEX_REAL_BIN:-/home/cst/.local/bin/codex-real}
codex_home=${CODEX_HOME:-${HOME:-/home/cst}/.codex}
session_id=

for argument in "$@"; do
  if printf '%s\n' "$argument" | grep -Eq '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'; then
    session_id=$argument
    break
  fi
done

if [ -n "$session_id" ] && [ -d "$codex_home" ]; then
  lock_file="$codex_home/.cst-ordinal-repair.lock"
  exec 9>"$lock_file"
  if flock -w 10 9; then
    for directory in sessions sessions-archive archived_sessions; do
      root="$codex_home/$directory"
      [ -d "$root" ] || continue
      find "$root" -type f -name "*-$session_id.jsonl" -print 2>/dev/null |
        while IFS= read -r rollout; do
          /usr/local/bin/codex-rollout-ordinal-repair "$rollout" || true
        done
    done
    flock -u 9
  else
    printf '%s\n' "cst-codex: ordinal repair lock timed out for $session_id" >&2
  fi
  exec 9>&-
fi

exec "$real_codex" "$@"
