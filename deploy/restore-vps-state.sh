#!/usr/bin/env bash
set -euo pipefail

ARCHIVE="${1:-}"
DATA_DIR="${CST_DATA_DIR:-/srv/cst}"
[[ -n "$ARCHIVE" && -f "$ARCHIVE" ]] || { echo "Usage: restore-vps-state.sh BACKUP/cst-data.tar.gz" >&2; exit 2; }
[[ -f "$ARCHIVE.sha256" ]] || { echo "Checksum file missing" >&2; exit 1; }
sha256sum -c "$ARCHIVE.sha256"
[[ "${CST_CONFIRM_RESTORE:-}" == "YES" ]] || {
  echo "Refusing restore. Set CST_CONFIRM_RESTORE=YES after draining all sessions." >&2
  exit 1
}
PARENT="$(dirname "$DATA_DIR")"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
mkdir -p "$PARENT"
if [[ -d "$DATA_DIR" ]]; then
  tar --xattrs --acls --numeric-owner -czf "$PARENT/cst-pre-restore-$STAMP.tar.gz" -C "$PARENT" "$(basename "$DATA_DIR")"
fi
tar --xattrs --acls --numeric-owner -xzf "$ARCHIVE" -C "$PARENT"
printf 'Restore completed. Pre-restore backup: %s\n' "$PARENT/cst-pre-restore-$STAMP.tar.gz"
