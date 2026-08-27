#!/usr/bin/env bash
set -euo pipefail

RCLONE_CONFIG="/etc/rclone/cst-google-drive.conf"
RCLONE_REMOTE="cst-google-drive:"
MOUNT_POINT="/srv/cst/google-drive"
CACHE_DIR="/var/cache/cst-google-drive"
COMPOSE_DIR="/opt/codex-switch-terminal"
COMPOSE_FILE="$COMPOSE_DIR/compose.yaml"

if [[ "$(id -u)" -ne 0 ]]; then
  echo "Ce script doit etre execute en root." >&2
  exit 1
fi

command -v rclone >/dev/null
command -v fusermount3 >/dev/null
test -s "$RCLONE_CONFIG"
test -f "$COMPOSE_FILE"

rclone lsd "$RCLONE_REMOTE" --config "$RCLONE_CONFIG" --max-depth 1 >/dev/null
install -d -o root -g root -m 0755 "$MOUNT_POINT"
install -d -o root -g root -m 0700 "$CACHE_DIR"

cat >/usr/local/sbin/cst-google-drive-links <<'PYTHON'
#!/usr/bin/env python3
import os
import re
import tempfile
import tomllib
from pathlib import Path

DRIVE = "/srv/cst/google-drive"
ROOT = Path("/srv/cst")


def ensure_link(parent: Path) -> bool:
    if not parent.is_dir():
        return False
    link = parent / "GoogleDrive"
    if link.is_symlink():
        return False
    if link.exists():
        return False
    link.symlink_to(DRIVE, target_is_directory=True)
    return True


def ensure_writable_root(config: Path) -> bool:
    text = config.read_text(encoding="utf-8")
    if DRIVE in text:
        return False

    roots = re.search(r"(?m)^(\s*writable_roots\s*=\s*)\[([^\n]*)\](\s*(?:#.*)?)$", text)
    if roots:
        previous = roots.group(2).strip()
        value = f'{previous}, "{DRIVE}"' if previous else f'"{DRIVE}"'
        updated = text[: roots.start()] + roots.group(1) + "[" + value + "]" + roots.group(3) + text[roots.end() :]
    else:
        table = re.search(r"(?m)^\[sandbox_workspace_write\]\s*$", text)
        if table:
            insert_at = table.end()
            updated = text[:insert_at] + f'\nwritable_roots = ["{DRIVE}"]' + text[insert_at:]
        else:
            updated = text.rstrip() + f'\n\n[sandbox_workspace_write]\nwritable_roots = ["{DRIVE}"]\n'

    tomllib.loads(updated)
    metadata = config.stat()
    descriptor, temporary_name = tempfile.mkstemp(prefix=".config.toml.", dir=config.parent)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8", newline="") as output:
            output.write(updated)
        os.chmod(temporary_name, metadata.st_mode)
        os.chown(temporary_name, metadata.st_uid, metadata.st_gid)
        os.replace(temporary_name, config)
    finally:
        if os.path.exists(temporary_name):
            os.unlink(temporary_name)
    return True


linked = 0
configured = 0

homes = ROOT / "codex-homes"
if homes.is_dir():
    for home in homes.iterdir():
        if home.is_dir():
            linked += int(ensure_link(home))
            config = home / "config.toml"
            if config.is_file():
                configured += int(ensure_writable_root(config))

spaces = ROOT / "user-spaces"
if spaces.is_dir():
    for environments in spaces.glob("*/environments"):
        if environments.is_dir():
            for environment in environments.iterdir():
                if environment.is_dir():
                    linked += int(ensure_link(environment))

ensure_link(ROOT)
print(f"liens_crees={linked} configurations_mises_a_jour={configured}")
PYTHON
chmod 0755 /usr/local/sbin/cst-google-drive-links

cat >/etc/systemd/system/cst-google-drive.service <<'UNIT'
[Unit]
Description=Google Drive partage de Codex Switch Terminal
Documentation=https://rclone.org/drive/
Wants=network-online.target
After=network-online.target
Before=docker.service

[Service]
Type=notify
User=root
Group=root
ExecStartPre=-/bin/fusermount3 -uz /srv/cst/google-drive
ExecStart=/usr/bin/rclone mount cst-google-drive: /srv/cst/google-drive --config /etc/rclone/cst-google-drive.conf --cache-dir /var/cache/cst-google-drive --allow-other --umask 0022 --dir-cache-time 5m --poll-interval 1m --vfs-cache-mode writes --vfs-cache-max-size 1G --vfs-cache-max-age 1h --buffer-size 8M --log-level INFO
ExecStop=/bin/fusermount3 -uz /srv/cst/google-drive
Restart=on-failure
RestartSec=10s
TimeoutStopSec=30s
NoNewPrivileges=true

[Install]
WantedBy=multi-user.target
UNIT

cat >/etc/systemd/system/cst-google-drive-links.service <<'UNIT'
[Unit]
Description=Expose Google Drive aux comptes et environnements CST
Requires=cst-google-drive.service
After=cst-google-drive.service

[Service]
Type=oneshot
ExecStart=/usr/local/sbin/cst-google-drive-links
UNIT

cat >/etc/systemd/system/cst-google-drive-links.timer <<'UNIT'
[Unit]
Description=Expose Google Drive aux nouveaux comptes et environnements CST

[Timer]
OnBootSec=1min
OnUnitActiveSec=1min
Persistent=true
Unit=cst-google-drive-links.service

[Install]
WantedBy=timers.target
UNIT

systemctl daemon-reload
systemctl enable cst-google-drive.service
if mountpoint -q "$MOUNT_POINT" && ! systemctl is-active --quiet cst-google-drive.service; then
  fusermount3 -uz "$MOUNT_POINT" || umount -l "$MOUNT_POINT"
fi
systemctl restart cst-google-drive.service
systemctl enable --now cst-google-drive-links.timer
systemctl start cst-google-drive-links.service

if ! mountpoint -q "$MOUNT_POINT"; then
  systemctl status cst-google-drive.service --no-pager >&2 || true
  exit 1
fi

python3 - "$COMPOSE_FILE" <<'PYTHON'
import shutil
import sys
import time
from pathlib import Path

compose = Path(sys.argv[1])
text = compose.read_text(encoding="utf-8")
short = "    volumes:\n      - /srv/cst:/srv/cst\n"
long = """    volumes:
      - type: bind
        source: /srv/cst
        target: /srv/cst
        bind:
          propagation: rslave
"""

if "propagation: rslave" not in text:
    if short not in text:
        raise SystemExit("Le volume /srv/cst du compose n'a pas la forme attendue.")
    backup = compose.with_name(f"compose.yaml.pre-google-drive-{int(time.time())}")
    shutil.copy2(compose, backup)
    compose.write_text(text.replace(short, long, 1), encoding="utf-8")
    print(f"sauvegarde_compose={backup}")
PYTHON

cd "$COMPOSE_DIR"
docker compose config --quiet
docker compose up -d --force-recreate cst

healthy=0
for _ in $(seq 1 30); do
  if curl -fsS --max-time 3 http://127.0.0.1:8080/healthz >/dev/null 2>&1; then
    healthy=1
    break
  fi
  sleep 2
done
if [[ "$healthy" -ne 1 ]]; then
  docker compose ps
  docker compose logs --tail 80 cst
  exit 1
fi

docker exec codex-switch-terminal sh -eu -c '
  test -d /srv/cst/google-drive
  grep -q " /srv/cst/google-drive " /proc/self/mountinfo
  test -r /srv/cst/google-drive
  test -w /srv/cst/google-drive
'

probe="$MOUNT_POINT/.cst-drive-probe-$(date +%s)-$$"
printf 'Codex Switch Terminal Google Drive probe\n' >"$probe"
grep -q 'Google Drive probe' "$probe"
rm -f -- "$probe"

echo "Google Drive monte et accessible dans le conteneur."
