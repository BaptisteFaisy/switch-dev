#!/usr/bin/env bash
# Bascule de l'API Duello vers une URL directe (api.duello.fr) au lieu du
# relais tunnel Cloudflare. À exécuter SUR le VPS qui héberge l'API Duello,
# en root (sudo).
#
# Prérequis avant exécution :
#   1. Enregistrement DNS A : api.duello.fr → IP publique STATIQUE du VPS
#      (dashboard Cloudflare si le domaine y est, sinon chez le registrar).
#   2. Ports 80 et 443 joignables depuis l'extérieur (NSG Azure : règles
#      entrantes TCP 80 et TCP 443 autorisées — ce VPS ne laisse passer que
#      le SSH par défaut).
#   3. L'API répond déjà en local sur le port 8891 — le même port d'origine
#      que celui du tunnel duello-api (originPort 8891).
#
# Le tunnel Cloudflare n'est PAS touché : les deux chemins coexistent, donc
# les builds déjà installées continuent de fonctionner pendant la bascule.
#
# Pièges connus sur ce VPS, traités automatiquement par ce script :
#   - NAT Azure : l'IP publique n'existe PAS sur une interface de la VM
#     (bind → EADDRNOTAVAIL). Azure translate IP publique → IP privée, donc
#     nginx doit écouter sur l'IP privée (détectée automatiquement).
#   - tailscaled (`tailscale serve`, HTTPS du tailnet azure-duello) occupe
#     déjà 443 sur ses adresses → un bind wildcard `listen 443 ssl` échoue
#     (EADDRINUSE). Le script transforme le bind en IP privée spécifique.
#
# Usage :
#   sudo CERTBOT_EMAIL=toi@duello.fr bash apply-api-direct.sh
#   sudo CERTBOT_EMAIL=toi@duello.fr DUELLO_API_DOMAIN=api.duello.fr bash apply-api-direct.sh
#   sudo CERTBOT_EMAIL=toi@duello.fr DUELLO_API_BIND=172.16.0.4 bash apply-api-direct.sh
set -Eeuo pipefail

DOMAIN="${DUELLO_API_DOMAIN:-api.duello.fr}"
UPSTREAM="${DUELLO_API_UPSTREAM:-http://127.0.0.1:8891}"
CERTBOT_EMAIL="${CERTBOT_EMAIL:?CERTBOT_EMAIL doit être renseigné (ex: toi@duello.fr)}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONF_SRC="$SCRIPT_DIR/nginx-api.duello.fr.conf"
SITE_NAME="api.duello.fr"
CONF_DST="/etc/nginx/sites-available/$SITE_NAME"
CONF_LINK="/etc/nginx/sites-enabled/$SITE_NAME"
LOG="/tmp/duello-api-direct.log"

log() { printf '%s\n' "[$(date -u '+%Y-%m-%d %H:%M:%S')] $*" | tee -a "$LOG"; }

if [[ "$(id -u)" != "0" ]]; then
  log "ERREUR : exécuter en root (sudo)."
  exit 1
fi

log "=== bascule API directe domain=$DOMAIN upstream=$UPSTREAM ==="

# --- Préflight : origine locale ---
ORIGIN_CODE="$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 "$UPSTREAM/" || true)"
if [[ -z "$ORIGIN_CODE" || "$ORIGIN_CODE" == "000" ]]; then
  log "ERREUR : l'origine $UPSTREAM ne répond pas. Le tunnel s'appuie sur ce port ; vérifier que duello-server tourne."
  exit 1
fi
log "ok origine locale $UPSTREAM répond (code $ORIGIN_CODE)"

# --- Préflight : DNS ---
RESOLVED="$(getent ahostsv4 "$DOMAIN" | awk '{print $1}' | sort -u | head -1 || true)"
if [[ -z "$RESOLVED" ]]; then
  log "ERREUR : $DOMAIN ne résout pas. Créer l'enregistrement DNS A vers l'IP publique statique du VPS avant de continuer."
  exit 1
fi
log "ok DNS $DOMAIN -> $RESOLVED (vérifier que c'est bien l'IP statique du VPS)"

# --- Installation nginx + certbot si absents ---
if ! command -v nginx >/dev/null 2>&1; then
  log "installation nginx..."
  apt-get update -qq && apt-get install -y -qq nginx
fi
if ! command -v certbot >/dev/null 2>&1; then
  log "installation certbot + plugin nginx..."
  apt-get update -qq && apt-get install -y -qq certbot python3-certbot-nginx
fi

# --- Installation de la config nginx ---
if [[ ! -f "$CONF_SRC" ]]; then
  log "ERREUR : config introuvable : $CONF_SRC (doit se trouver à côté du script)."
  exit 1
fi
install -m 0644 "$CONF_SRC" "$CONF_DST"
ln -sfn "$CONF_DST" "$CONF_LINK"
nginx -t
systemctl reload nginx
log "ok config nginx installée et rechargée"

# --- Certificat Let's Encrypt (émet si absent, installe sinon le bloc 443) ---
# Toujours exécuté : certbot --nginx est idempotent — avec un certificat valide
# existant il rejoue l'installateur (bloc 443 + redirection) sans ré-émettre
# (--keep-until-expiring). Ne pas remplacer ce test par `certbot certificates`
# en code de sortie : il sort en 0 même sans aucun certificat.
log "certificat Let's Encrypt pour $DOMAIN..."
certbot --nginx -d "$DOMAIN" \
  --non-interactive --agree-tos -m "$CERTBOT_EMAIL" \
  --redirect --keep-until-expiring

# --- Ajustement du bind 443 (NAT Azure + tailscaled) ---
# Certbot génère `listen 443 ssl;` (wildcard). Sur ce VPS ce bind échoue :
#   - l'IP publique n'est pas sur une interface (NAT Azure) → EADDRNOTAVAIL ;
#   - tailscaled (HTTPS du tailnet) occupe déjà 443 → EADDRINUSE.
# On lie donc 443 à l'IP privée de la VM ; Azure NAT fait la translation
# IP publique → IP privée (comme pour le port 80).
# NB : éditer $CONF_DST (le fichier réel), pas le lien symbolique — sed -i
# remplacerait le lien par un fichier régulier.
PRIV_IP="${DUELLO_API_BIND:-$(ip -4 route get 1.1.1.1 2>/dev/null | awk '{print $7; exit}')}"
if [[ -n "$PRIV_IP" ]]; then
  sed -i "/^    listen \[::\]:443 ssl ipv6only=on; # managed by Certbot$/d" "$CONF_DST"
  sed -i "s|^    listen 443 ssl; # managed by Certbot$|    listen $PRIV_IP:443 ssl; # bind IP privée (NAT Azure + 443 tailnet pris par tailscaled)|" "$CONF_DST"
  nginx -t
  systemctl reload nginx
  log "ok bind 443 sur l'IP privée $PRIV_IP"
else
  log "AVERTISSEMENT : IP privée non détectée, bind 443 laissé en wildcard"
fi

# --- Vérification (depuis la VM : impossible de se joindre soi-même via l'IP
# publique — hairpin Azure — donc on cible l'IP privée avec le SNI du domaine) ---
# Le reload nginx peut mettre un instant à être effectif : petit délai + retries.
HTTPS_CODE="000"
sleep 1
for _ in 1 2 3 4 5; do
  HTTPS_CODE="$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 \
    --resolve "$DOMAIN:443:${PRIV_IP:-127.0.0.1}" "https://$DOMAIN/" || true)"
  [[ -z "$HTTPS_CODE" || "$HTTPS_CODE" == "000" ]] || break
  sleep 2
done
if [[ -z "$HTTPS_CODE" || "$HTTPS_CODE" == "000" ]]; then
  log "ERREUR : https://$DOMAIN/ ne répond pas localement. Vérifier le DNS, le NSG (80/443), le bind 443 et la config."
  exit 1
fi
log "ok https://$DOMAIN/ répond localement (code $HTTPS_CODE)"
log "ok bascule serveur en place — vérifier depuis l'extérieur : curl -I https://$DOMAIN/api"

log ""
log "=== TERMINÉ — bascule serveur en place ==="
log "Le tunnel duello-api reste actif : les deux chemins coexistent (secours)."
log ""
log "Étapes restantes côté app (projet Duello sur le VPS) :"
log "  1. eas.json : EXPO_PUBLIC_DUELLO_API_URL=https://$DOMAIN/api dans TOUS les profils de build ;"
log "  2. config/cloudflare-tunnel.json : apiUrl=https://$DOMAIN/api (identique, invariant de build) ;"
log "  3. rebuild + republication de l'app (docs/METTRE_A_JOUR_LAPP.md) ;"
log "  4. une fois les anciennes builds sorties, le tunnel peut être arrêté proprement."
log ""
log "Rollback serveur : rm $CONF_LINK && nginx -t && systemctl reload nginx"
