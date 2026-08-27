#!/usr/bin/env bash
# Surveille la bande passante d'un noeud Switch afin de detecter une
# saturation (ex. connexion limitee a 10 Mbit/s) quand la flotte d'agents
# autonomes ou les chats tournent.
#
# Deux modes :
#   1. iftop      (defaut) : interface interactive en temps reel par
#                            connexion (installez-le : apt install iftop).
#   2. --nload             : releve brut du debit global (installez nload).
#   3. --once              : releve ponctuel non interactif du debit effectif
#                            (le plus simple a chronometrer / journaliser).
#
# Usage :
#   sudo scripts/bandwidth-monitor.sh [--iface eth0] [--nload|--once] [--seconds N]
#
# Regle de base (10 Mbit/s = 1,25 Mo/s) :
#   - keep-alive ~0,33 Mo/s ; API LLM actif ~0,5-2 Mo/s par tour ;
#   - si le debit soutenu approche 1,25 Mo/s en permanence, reduisez la
#     cadence/un lancement simultane.


set -euo pipefail

IFACE=""
MODE="iftop"
SECONDS=10

usage() {
  sed -n '2,30p' "$0" | sed '/^#/!q;s/^# \{0,1\}//'
  exit 2
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --iface) IFACE="$2"; shift 2 ;;
    --nload) MODE="nload"; shift ;;
    --once)  MODE="once"; shift ;;
    --seconds) SECONDS="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "Argument inconnu: $1" >&2; usage ;;
  esac
done

[[ "$SECONDS" =~ ^[1-9][0-9]*$ ]] || { echo "--seconds doit etre un entier positif." >&2; exit 1; }

default_iface() {
  # Prefere l'interface de routage par defaut (la plus pertinente pour sortir).
  ip route 2>/dev/null | awk '/^default/{print $5; exit}' || true
}

run_iftop() {
  local iface_args=()
  if [[ -n "$IFACE" ]]; then
    iface_args+=(-i "$IFACE")
  else
    local d; d="$(default_iface)"
    [[ -n "$d" ]] && iface_args+=(-i "$d")
  fi
  command -v iftop >/dev/null 2>&1 || {
    echo "iftop introuvable. Installez-le : sudo apt-get install -y iftop" >&2
    exit 1
  }
  echo "iftop : appuyez sur 't' pour basculer en debit total, 'q' pour quitter."
  exec iftop "${iface_args[@]}" -B   # -B : affichage en octets
}

run_nload() {
  command -v nload >/dev/null 2>&1 || {
    echo "nload introuvable. Installez-le : sudo apt-get install -y nload" >&2
    exit 1
  }
  local iface_args=()
  [[ -n "$IFACE" ]] && iface_args+=(-i "$IFACE")
  # -m : graphique agrege (un seul paquet d'interfaces) ; -u G : octets.
  exec nload "${iface_args[@]}" -m -u G
}

run_once() {
  # Releve ponctuel du debit total en Mo/s pendant `$SECONDS` secondes, sans
  # outil exterieur : lit les compteurs /proc/net/dev avant/apres.
  local iface d rx0 tx0 rx1 tx1
  if [[ -n "$IFACE" ]]; then
    iface="$IFACE"
  else
    d="$(default_iface)"
    iface="${d:-}"
    [[ -n "$iface" ]] || { echo "Interface par defaut introuvable." >&2; exit 1; }
  fi

  read -r rx0 tx0 < <(awk -v I="$iface" '$1 ~ I ":"{gsub(":","",$1); print $2, $10}' /proc/net/dev)
  sleep "$SECONDS"
  read -r rx1 tx1 < <(awk -v I="$iface" '$1 ~ I ":"{gsub(":","",$1); print $2, $10}' /proc/net/dev)

  local down up
  down=$(awk -v a="$rx0" -v b="$rx1" -v s="$SECONDS" 'BEGIN{printf "%.2f", (b-a)/1048576/s}')
  up=$(awk   -v a="$tx0" -v b="$tx1" -v s="$SECONDS" 'BEGIN{printf "%.2f", (b-a)/1048576/s}')

  printf 'Interface : %s\n' "$iface"
  printf 'Descendant : %s Mo/s (max 10 Mbit/s = 1,25 Mo/s)\n' "$down"
  printf 'Montant    : %s Mo/s\n' "$up"
  echo
  echo "Interpretation :"
  echo "  - debit soutenu proche de 1,25 Mo/s  -> connexion saturee."
  echo "  - sinon, laissez la flotte tourner (CPU/RAM OK)."
}

case "$MODE" in
  iftop)  run_iftop ;;
  nload)  run_nload ;;
  once)   run_once ;;
  *)      usage ;;
esac