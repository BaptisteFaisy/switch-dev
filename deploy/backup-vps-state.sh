#!/usr/bin/env bash
set -euo pipefail

DATA_DIR="${CST_DATA_DIR:-/srv/cst}"
BACKUP_ROOT="${CST_BACKUP_ROOT:-/srv/cst-backups}"
RETENTION_DAYS="${CST_BACKUP_RETENTION_DAYS:-14}"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
DEST="$BACKUP_ROOT/$STAMP"

[[ -d "$DATA_DIR" ]] || { echo "Data directory not found: $DATA_DIR" >&2; exit 1; }
[[ "$RETENTION_DAYS" =~ ^[0-9]+$ ]] || { echo "Invalid retention" >&2; exit 2; }
install -d -m 0700 "$DEST"
tar --xattrs --acls --numeric-owner -czf "$DEST/cst-data.tar.gz" \
  --exclude='*/logs/*' \
  --exclude='*/.cache/*' \
  -C "$(dirname "$DATA_DIR")" "$(basename "$DATA_DIR")"
sha256sum "$DEST/cst-data.tar.gz" > "$DEST/cst-data.tar.gz.sha256"
if command -v pg_dump >/dev/null 2>&1 && [[ -n "${CST_DATABASE_URL:-}" ]]; then
  pg_dump --format=custom --file="$DEST/cst-postgres.dump" "$CST_DATABASE_URL"
  sha256sum "$DEST/cst-postgres.dump" > "$DEST/cst-postgres.dump.sha256"
fi
find "$BACKUP_ROOT" -mindepth 1 -maxdepth 1 -type d -mtime "+$RETENTION_DAYS" -exec rm -rf -- {} +
printf 'Backup created: %s\n' "$DEST"
