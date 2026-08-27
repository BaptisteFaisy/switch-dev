#!/usr/bin/env bash
# Monte un RAM-disk (tmpfs) pour les fichiers temporaires des agents orchestres
# (worktrees Git, clones, patches, handoffs) : ~10x plus rapide que le SSD SATA.
#
# Usage :
#   sudo ./deploy/ramdisk-agent-cache.sh [taille] [point-de-montage]
#   Ex. sudo ./deploy/ramdisk-agent-cache.sh 8g /mnt/cst-ramdisk
#
# Puis pointer l'environnement vers ce montage :
#   CST_ORCHESTRATION_SANDBOX_DIR=/mnt/cst-ramdisk/orchestrated-runs
#
# Pour rendre le montage permanent (fstab) :
#   tmpfs /mnt/cst-ramdisk tmpfs size=8g,mode=1777 0 0
#
# Notes :
# - En Docker (compose.yaml), inutile : le conteneur expose deja /dev/shm en
#   tmpfs, agrandi via CST_SHM_SIZE (defaut 4g). Pointer simplement
#   CST_ORCHESTRATION_SANDBOX_DIR=/dev/shm/cst-orchestrated-runs.
# - Windows : Docker Desktop couvre le cas conteneur. Hors Docker, utiliser
#   ImDisk Toolkit pour creer une lettre de lecteur RAM, puis pointer
#   CST_ORCHESTRATION_SANDBOX_DIR dessus.
# - Volatile par conception : le contenu disparait au redemarrage, sans
#   consequence car les worktrees sont regenerebles. Ne jamais y mettre l'etat
#   persiste (orchestrated-runs.json), qui doit rester sur le disque.

set -euo pipefail

SIZE="${1:-8g}"
MOUNTPOINT="${2:-/mnt/cst-ramdisk}"

if [ "$(id -u)" -ne 0 ]; then
    echo "Ce script doit tourner en root (sudo) pour monter un tmpfs." >&2
    exit 1
fi

if mountpoint -q "$MOUNTPOINT"; then
    echo "Deja monte : $MOUNTPOINT"
    df -h "$MOUNTPOINT"
    exit 0
fi

mkdir -p "$MOUNTPOINT"
mount -t tmpfs -o "size=${SIZE},mode=1777" tmpfs "$MOUNTPOINT"
echo "RAM-disk monte : ${MOUNTPOINT} (${SIZE})"
df -h "$MOUNTPOINT"
echo
echo "A activer pour les agents :"
echo "  CST_ORCHESTRATION_SANDBOX_DIR=${MOUNTPOINT}/orchestrated-runs"
