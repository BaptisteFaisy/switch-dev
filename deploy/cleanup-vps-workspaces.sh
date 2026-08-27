#!/usr/bin/env bash
set -euo pipefail

DATA_DIR="${CST_DATA_DIR:-/srv/cst}"
WORKSPACES="${CST_WORKSPACES_ROOT:-$DATA_DIR/workspaces}"
MAX_AGE_DAYS="${CST_WORKSPACE_MAX_AGE_DAYS:-3}"
DRY_RUN=1
[[ "${1:-}" == "--apply" ]] && DRY_RUN=0
[[ "$MAX_AGE_DAYS" =~ ^[0-9]+$ ]] || { echo "Invalid age" >&2; exit 2; }

remove_path() {
  if (( DRY_RUN )); then printf '[dry-run] remove %s\n' "$1"; else rm -rf -- "$1"; fi
}
while IFS= read -r -d '' path; do remove_path "$path"; done < <(
  find "$WORKSPACES" -mindepth 1 -maxdepth 1 -type d -mtime "+$MAX_AGE_DAYS" -print0 2>/dev/null
)
while IFS= read -r -d '' path; do remove_path "$path"; done < <(
  find "$DATA_DIR" -type f \( -name '*.tmp' -o -name '*.partial' \) -mtime +1 -print0 2>/dev/null
)
if (( DRY_RUN )); then echo 'No files changed. Use --apply after reviewing the list.'; fi
