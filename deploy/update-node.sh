#!/usr/bin/env bash
set -euo pipefail

# Mise a jour SURE d'un noeud Linux deja installe.
#
# QUATRE modes pour peupler releases/<v> :
#   - build     (defaut) : compile sur l'hote depuis /opt/codex-switch-terminal-src.
#   - release   (--release <tag>) : TELECHARGE l'artefact signe de la GitHub
#                Release, verifie SHA-256 + signature minisign (fail-closed),
#                puis l'installe. C'est le mode Phase 2 (artefacts CI signes).
#   - prebuilt  (--prebuilt <archive>) : artefact CI PRECOMPILE (binaire + dist)
#                transfere par SSH, verifie SHA-256 + minisign puis installe
#                sans aucune compilation sur l'hote. Utilise par la chaine
#                continue (deploy-web.yml) pour les pushes qui touchent le Rust.
#   - frontend  (--frontend <archive>) : pousse frontend uniquement : le binaire
#                courant est conserve, seul dist/ est remplace (bascule rapide).
#
# Sequence commune ensuite : self-check `--version` -> attente NON BLOQUANTE
# activeTerminals==0 -> courte lease de drain -> bascule atomique de 'current'
# -> restart -> verification "vraiment revenu"
# -> rollback automatique si echec.
#
# Usage :
#   # build sur l'hote (Phase 1)
#   sudo bash update-node.sh [--source cst-source.tar.gz] [--commit <sha>]
#   # download d'une release signee (Phase 2)
#   sudo bash update-node.sh --release v0.1.0 [--repo owner/repo] \
#        [--minisign-pubkey 'RW...'] [--asset cst-server-linux-x86_64.tar.gz]
#   # artefact CI precompile signe, transfere par SSH (Phase 2 continue)
#   sudo bash update-node.sh --prebuilt /tmp/cst-server-linux-x86_64.tar.gz \
#        [--commit <sha>] [--minisign-pubkey 'RW...']
#   # push frontend seul : binaire conserve, dist/ remplace
#   sudo bash update-node.sh --frontend /tmp/cst-frontend.tar.gz \
#        [--commit <sha>] [--minisign-pubkey 'RW...']
#   # options communes : [--drain-timeout <sec>] [--drain-lease <sec>]
#   #                    [--force] [--allow-unsigned]

APP_DIR="${CST_APP_DIR:-/opt/codex-switch-terminal}"
SOURCE_DIR="${CST_SOURCE_DIR:-/opt/codex-switch-terminal-src}"
SOURCE_ARCHIVE="/tmp/cst-source.tar.gz"
RELEASES_DIR="$APP_DIR/releases"
BUILD_CACHE="$APP_DIR/build-cache"
CURRENT_LINK="$APP_DIR/current"
ENV_FILE="${CST_ENV_FILE:-/etc/codex-switch-terminal.env}"
SERVICE="codex-switch-terminal.service"
CST_GIT_COMMIT="${CST_GIT_COMMIT:-}"
DRAIN_TIMEOUT=300
DRAIN_LEASE=20
VERIFY_TIMEOUT="${CST_VERIFY_TIMEOUT:-60}"
FORCE=0
DRAIN_ARMED=0
DL=""
STAGE=""
RELEASE_DIR=""
RELEASE_MARKER=""

# --- Mode release (Phase 2) ---
MODE="build"                                   # build | release | prebuilt | frontend
PREBUILT_ARCHIVE=""
FRONTEND_ARCHIVE=""
RELEASE_TAG=""
REPO="${CST_REPO:-BaptisteFaisy/Software-multi-account}"
ASSET="${CST_ASSET:-cst-server-linux-x86_64.tar.gz}"
# Cle publique minisign (NON secrete). Remplace le placeholder par ta vraie cle
# (voir deploy/PHASE2-UPDATES.md) ou passe --minisign-pubkey / CST_MINISIGN_PUBKEY.
MINISIGN_PUBKEY="${CST_MINISIGN_PUBKEY:-RWQPLACEHOLDER_REMPLACE_MOI}"
ALLOW_UNSIGNED=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --source) SOURCE_ARCHIVE="$2"; shift 2 ;;
    --commit) CST_GIT_COMMIT="$2"; shift 2 ;;
    --drain-timeout) DRAIN_TIMEOUT="$2"; shift 2 ;;
    --drain-lease) DRAIN_LEASE="$2"; shift 2 ;;
    --force) FORCE=1; shift ;;
    --release) MODE="release"; RELEASE_TAG="$2"; shift 2 ;;
    --prebuilt) MODE="prebuilt"; PREBUILT_ARCHIVE="$2"; shift 2 ;;
    --frontend) MODE="frontend"; FRONTEND_ARCHIVE="$2"; shift 2 ;;
    --repo) REPO="$2"; shift 2 ;;
    --asset) ASSET="$2"; shift 2 ;;
    --minisign-pubkey) MINISIGN_PUBKEY="$2"; shift 2 ;;
    --allow-unsigned) ALLOW_UNSIGNED=1; shift ;;
    *) echo "Argument inconnu: $1" >&2; exit 2 ;;
  esac
done

[[ "$DRAIN_TIMEOUT" =~ ^[0-9]+$ ]] || { echo "--drain-timeout doit etre un entier positif." >&2; exit 2; }
[[ "$DRAIN_LEASE" =~ ^[0-9]+$ && "$DRAIN_LEASE" -ge 5 && "$DRAIN_LEASE" -le 60 ]] || {
  echo "--drain-lease doit etre compris entre 5 et 60 secondes." >&2
  exit 2
}
[[ "$VERIFY_TIMEOUT" =~ ^[0-9]+$ && "$VERIFY_TIMEOUT" -ge 1 ]] || {
  echo "CST_VERIFY_TIMEOUT doit etre un entier strictement positif." >&2
  exit 2
}

[[ "$(id -u)" -eq 0 ]] || { echo "Lance ce script avec sudo." >&2; exit 1; }
[[ -f "$ENV_FILE" ]] || { echo "$ENV_FILE introuvable : noeud non installe ?" >&2; exit 1; }
REQUIRED_TOOLS=(curl jq awk sed tar flock)
if [[ "$MODE" == "release" || "$MODE" == "prebuilt" || "$MODE" == "frontend" ]]; then
  REQUIRED_TOOLS+=(sha256sum)
  [[ "$ALLOW_UNSIGNED" == "1" ]] || REQUIRED_TOOLS+=(minisign)
fi
for tool in "${REQUIRED_TOOLS[@]}"; do
  command -v "$tool" >/dev/null 2>&1 || { echo "Outil requis introuvable: $tool" >&2; exit 1; }
done

# Charge token + bind depuis l'EnvironmentFile (format KEY="value").
set -a; # shellcheck disable=SC1090
source "$ENV_FILE"; set +a
ADMIN_TOKEN="${CST_ADMIN_TOKEN:-}"
BIND="${CST_BIND:-127.0.0.1:8080}"
PORT="${BIND##*:}"
BASE="http://127.0.0.1:$PORT"
[[ -n "$ADMIN_TOKEN" ]] || { echo "CST_ADMIN_TOKEN absent de $ENV_FILE." >&2; exit 1; }

log() { echo "[update-node] $*"; }
healthz() { curl -fsS --max-time 3 "$BASE/healthz" 2>/dev/null || true; }
# `// empty` perdrait les booleens JSON valides a `false` (jq considere
# `false` comme une valeur alternative). La verification doit distinguer un
# champ absent de `draining: false`, sinon chaque mise a jour saine rollbacke.
hfield() {
  jq -r --arg k "$2" \
    'if has($k) and .[$k] != null then .[$k] else empty end' \
    <<<"${1:-}" 2>/dev/null || true
}
active_chat_turn_count() {
  local payload count
  if ! payload="$(curl -fsS --max-time 3 "$BASE/api/chat/turns/active" \
    -H "Authorization: Bearer $ADMIN_TOKEN" 2>/dev/null)"; then
    echo "Impossible de verifier les tours de chat actifs ; mise a jour annulee sans redemarrage." >&2
    return 1
  fi
  if ! count="$(jq -er 'if type == "array" then length else error("reponse invalide") end' \
    <<<"$payload" 2>/dev/null)"; then
    echo "Reponse invalide pour les tours de chat actifs ; mise a jour annulee sans redemarrage." >&2
    return 1
  fi
  printf '%s\n' "$count"
}
active_workload_count() {
  local hz="${1:-}" terminals chats
  [[ -n "$hz" ]] || { printf '0\n'; return 0; }
  terminals="$(hfield "$hz" activeTerminals)"; terminals="${terminals:-0}"
  if [[ ! "$terminals" =~ ^[0-9]+$ ]]; then
    echo "Compteur de terminaux actifs invalide ; mise a jour annulee sans redemarrage." >&2
    return 1
  fi
  chats="$(active_chat_turn_count)" || return 1
  printf '%s\n' "$((terminals + chats))"
}
set_drain() {
  local draining="$1"
  curl -fsS --max-time 5 -X POST "$BASE/api/admin/drain" \
    -H "Authorization: Bearer $ADMIN_TOKEN" -H "Content-Type: application/json" \
    -d "{\"draining\":$draining,\"ttlSeconds\":$DRAIN_LEASE}" >/dev/null
}
release_update_in_progress() {
  local release_dir="$1" marker owner_pid owner_start current_start
  marker="$release_dir/.update-in-progress"
  [[ -f "$marker" ]] || return 1
  IFS='|' read -r owner_pid owner_start <"$marker" || true
  if [[ "$owner_pid" =~ ^[0-9]+$ && "$owner_start" =~ ^[0-9]+$ ]] &&
     kill -0 "$owner_pid" 2>/dev/null && [[ -r "/proc/$owner_pid/stat" ]]; then
    current_start="$(awk '{print $22}' "/proc/$owner_pid/stat" 2>/dev/null || true)"
    [[ "$current_start" == "$owner_start" ]] && return 0
  fi
  rm -f -- "$marker"
  return 1
}
prune_obsolete_releases() {
  local keep="$1" candidate removed=0
  for candidate in "$RELEASES_DIR"/*; do
    [[ -d "$candidate" ]] || continue
    [[ "$candidate" == "$keep" ]] && continue
    if release_update_in_progress "$candidate"; then
      log "Release conservee car une mise a jour l'utilise: ${candidate##*/}"
      continue
    fi
    if rm -rf -- "$candidate"; then
      removed=$((removed + 1))
    else
      log "ATTENTION: ancienne release impossible a supprimer: $candidate"
    fi
  done
  log "Nettoyage local : $removed ancienne(s) release(s) web/app supprimee(s)."
}
cleanup() {
  local status=$? current_real release_real
  trap - EXIT
  if [[ "$DRAIN_ARMED" == "1" ]]; then
    log "Mise a jour interrompue avant le redemarrage ; sortie automatique du drain."
    if set_drain false; then
      log "Noeud remis en service."
    else
      log "Sortie explicite impossible ; la lease expirera sous ${DRAIN_LEASE}s."
    fi
  fi
  if [[ -n "$RELEASE_MARKER" ]]; then
    rm -f -- "$RELEASE_MARKER"
    current_real="$(readlink -f "$CURRENT_LINK" 2>/dev/null || true)"
    release_real="$(readlink -f "$RELEASE_DIR" 2>/dev/null || true)"
    if [[ -n "$release_real" && "$current_real" != "$release_real" ]]; then
      rm -rf -- "$RELEASE_DIR"
    fi
  fi
  if [[ -n "$DL" ]]; then rm -rf "$DL"; fi
  if [[ -n "$STAGE" ]]; then rm -rf "$STAGE"; fi
  exit "$status"
}
trap cleanup EXIT

# Verifie un artefact en echec-ferme : empreinte SHA-256 obligatoire, puis
# signature minisign (sauf --allow-unsigned explicite). `$1` = chemin de
# l'archive ; les sidecars `.sha256` / `.minisig` doivent etre a cote (le
# .sha256 contient le seul basename, comme produit par `sha256sum f > f.sha256`).
verify_artifact() {
  local asset="$1"
  [[ -f "$asset.sha256" ]] || { echo "Empreinte $asset.sha256 absente (fail-closed)." >&2; exit 1; }
  log "Verification SHA-256"
  ( cd "$(dirname "$asset")" && sha256sum -c "$(basename "$asset").sha256" )
  if [[ "$ALLOW_UNSIGNED" == "1" ]]; then
    log "ATTENTION: verification de signature IGNOREE (--allow-unsigned)."
  else
    [[ -f "$asset.minisig" ]] || { echo "Signature $asset.minisig absente (fail-closed)." >&2; exit 1; }
    if [[ "$MINISIGN_PUBKEY" == RWQPLACEHOLDER* ]]; then
      echo "Cle publique minisign non configuree (voir deploy/PHASE2-UPDATES.md" >&2
      echo "ou passe --minisign-pubkey / CST_MINISIGN_PUBKEY)." >&2
      exit 1
    fi
    log "Verification signature minisign"
    minisign -Vm "$asset" -x "$asset.minisig" -P "$MINISIGN_PUBKEY"
  fi
}

# --- Peupler la nouvelle release : build hote, artefact precompile signe, ---
# --- frontend seul OU release signee telechargee (verif SHA-256 + minisign) ---
case "$MODE" in
  release)
    DL="$(mktemp -d)"; STAGE="$(mktemp -d)"
    base_url="https://github.com/$REPO/releases/download/$RELEASE_TAG"
    log "Telechargement de $ASSET depuis $REPO@$RELEASE_TAG"
    for suffix in "" ".sha256" ".minisig"; do
      curl -fSL --retry 3 --retry-delay 2 --max-time 180 \
        -o "$DL/$ASSET$suffix" "$base_url/$ASSET$suffix"
    done
    verify_artifact "$DL/$ASSET"
    tar -xzf "$DL/$ASSET" -C "$STAGE"
    BUILT_BIN="$STAGE/cst-server"
    DIST_SRC="$STAGE/dist"
    [[ -f "$BUILT_BIN" ]] || { echo "Archive invalide: cst-server introuvable." >&2; exit 1; }
    [[ -d "$DIST_SRC" ]] || { echo "Archive invalide: dist/ introuvable." >&2; exit 1; }
    chmod 0755 "$BUILT_BIN"
    ;;
  prebuilt)
    # Artefact CI PRECOMPILE (binaire + dist) transfere par SSH : aucune
    # compilation sur l'hote, verification SHA-256 + minisign avant bascule.
    STAGE="$(mktemp -d)"
    [[ -f "$PREBUILT_ARCHIVE" ]] || { echo "Artefact precompile introuvable: $PREBUILT_ARCHIVE" >&2; exit 1; }
    verify_artifact "$PREBUILT_ARCHIVE"
    tar -xzf "$PREBUILT_ARCHIVE" -C "$STAGE"
    BUILT_BIN="$STAGE/cst-server"
    DIST_SRC="$STAGE/dist"
    [[ -f "$BUILT_BIN" ]] || { echo "Archive invalide: cst-server introuvable." >&2; exit 1; }
    [[ -d "$DIST_SRC" ]] || { echo "Archive invalide: dist/ introuvable." >&2; exit 1; }
    chmod 0755 "$BUILT_BIN"
    log "Artefact precompile verifie : ${BUILT_BIN}"
    ;;
  frontend)
    # Pousse frontend uniquement : le binaire courant est conserve, seul
    # dist/ est remplace -> bascule quasi instantanee, aucune compilation.
    STAGE="$(mktemp -d)"
    [[ -f "$FRONTEND_ARCHIVE" ]] || { echo "Artefact frontend introuvable: $FRONTEND_ARCHIVE" >&2; exit 1; }
    verify_artifact "$FRONTEND_ARCHIVE"
    tar -xzf "$FRONTEND_ARCHIVE" -C "$STAGE"
    DIST_SRC="$STAGE/dist"
    [[ -d "$DIST_SRC" ]] || { echo "Archive invalide: dist/ introuvable." >&2; exit 1; }
    CURRENT_REAL="$(readlink -f "$CURRENT_LINK" 2>/dev/null || true)"
    BUILT_BIN="$CURRENT_REAL/cst-server"
    [[ -x "$BUILT_BIN" ]] || { echo "Binaire courant introuvable (noeud non installe ?)." >&2; exit 1; }
    log "Mode frontend : binaire conserve ($BUILT_BIN), seul dist/ est remplace."
    ;;
  build)
  # Mode build (Phase 1) : nouvelle source (push deploy) puis compilation hote.
  if [[ -f "$SOURCE_ARCHIVE" ]]; then
    log "Extraction de la nouvelle source dans $SOURCE_DIR"
    install -d -o cst -g cst "$SOURCE_DIR"
    rm -rf "${SOURCE_DIR:?}"/*
    tar -xzf "$SOURCE_ARCHIVE" -C "$SOURCE_DIR"
    chown -R cst:cst "$SOURCE_DIR"
  fi
  log "Build de la nouvelle release depuis $SOURCE_DIR"
  install -d -o cst -g cst "$BUILD_CACHE"
  ln -sfn "$BUILD_CACHE" "$SOURCE_DIR/src-tauri/target"
  chown -h cst:cst "$SOURCE_DIR/src-tauri/target"
  runuser -u cst -- env \
    HOME=/home/cst \
    PATH=/home/cst/.cargo/bin:/usr/local/bin:/usr/bin:/bin \
    CARGO_TARGET_DIR="$BUILD_CACHE" \
    CST_GIT_COMMIT="$CST_GIT_COMMIT" \
    bash -c "cd '$SOURCE_DIR' && cargo +1.88.0 build --manifest-path src-tauri/Cargo.toml --profile server --bin cst-server"
    BUILT_BIN="$BUILD_CACHE/server/cst-server"
    DIST_SRC="$SOURCE_DIR/dist"
    ;;
esac

# --- Self-check : le binaire repond a --version (commun aux deux modes) ---
VLINE="$("$BUILT_BIN" --version)"
VERSION="$(awk '{print $2}' <<<"$VLINE")"
COMMIT="$(sed -n 's/^cst-server [^ ]* (\(.*\))$/\1/p' <<<"$VLINE")"
[[ -n "$VERSION" && -n "$COMMIT" ]] || {
  echo "Version/commit illisibles via 'cst-server --version': $VLINE" >&2
  exit 1
}
if [[ "$MODE" == "build" || "$MODE" == "prebuilt" ]] \
   && [[ -n "$CST_GIT_COMMIT" && "$COMMIT" != "$CST_GIT_COMMIT" ]]; then
  echo "Incoherence: commit demande $CST_GIT_COMMIT mais binaire en $COMMIT." >&2
  exit 1
fi
if [[ "$MODE" == "release" && "${RELEASE_TAG#v}" != "$VERSION" ]]; then
  echo "Incoherence: tag $RELEASE_TAG mais binaire en version $VERSION." >&2
  exit 1
fi
if [[ "$MODE" == "frontend" && -n "$CST_GIT_COMMIT" ]]; then
  # Binaire inchange : le commit embarque reste celui d'avant ; seul dist/ avance.
  log "Frontend uniquement : dist du commit $CST_GIT_COMMIT, binaire en $COMMIT."
fi
log "Nouvelle release : $VLINE"

# --- Installer dans releases/<version-commit> (commun) ---
# La version Cargo change peu en developpement. Le commit rend chaque dossier
# immutable et evite de reecrire le binaire actuellement execute.
SAFE_COMMIT="$(sed 's/[^A-Za-z0-9._-]/-/g' <<<"$COMMIT")"
RELEASE_ID="$VERSION-$SAFE_COMMIT"
if [[ "$MODE" == "frontend" && -n "$CST_GIT_COMMIT" ]]; then
  # Deux pousses frontend successives partagent le meme binaire : suffixer par
  # le nouveau commit distingue les releases et preserve le rollback.
  RELEASE_ID="$RELEASE_ID-ui-$(sed 's/[^A-Za-z0-9._-]/-/g' <<<"$CST_GIT_COMMIT")"
fi
RELEASE_DIR="$RELEASES_DIR/$RELEASE_ID"
if [[ -e "$RELEASE_DIR" ]]; then
  RELEASE_ID="$RELEASE_ID-$(date +%s)-$$"
  RELEASE_DIR="$RELEASES_DIR/$RELEASE_ID"
fi
install -d -o cst -g cst "$RELEASES_DIR" "$RELEASE_DIR"
RELEASE_MARKER="$RELEASE_DIR/.update-in-progress"
PROCESS_START="$(awk '{print $22}' "/proc/$$/stat")"
printf '%s|%s\n' "$$" "$PROCESS_START" >"$RELEASE_MARKER"
if ! install -m 0755 "$BUILT_BIN" "$RELEASE_DIR/cst-server" ||
   ! cp -a "$DIST_SRC" "$RELEASE_DIR/dist"; then
  rm -rf -- "$RELEASE_DIR"
  RELEASE_MARKER=""
  echo "Installation de la release locale impossible." >&2
  exit 1
fi
chown -R cst:cst "$RELEASE_DIR"

# --- Attente NON BLOQUANTE, puis mutex court de bascule ---
log "Attente d'un instant libre sans drainer le noeud (timeout ${DRAIN_TIMEOUT}s)"
deadline=$(( $(date +%s) + DRAIN_TIMEOUT ))
exec 9>"$APP_DIR/.update.lock"
LOCK_HELD=0
RUNNING=0
while :; do
  hz="$(healthz)"
  if [[ -z "$hz" ]]; then
    active=0; draining=false; RUNNING=0
  else
    active="$(active_workload_count "$hz")" || exit 1
    draining="$(hfield "$hz" draining)"; draining="${draining:-false}"
    RUNNING=1
  fi

  if { [[ "$active" == "0" && "$draining" != "true" ]] ||
       [[ "$(date +%s)" -ge "$deadline" && "$FORCE" == "1" ]]; }; then
    if ! flock -n 9; then
      log "Une autre mise a jour effectue deja la courte bascule ; retente apres sa verification."
      exit 4
    fi
    LOCK_HELD=1
    # Ferme la course entre la sonde et la prise du mutex.
    hz="$(healthz)"
    if [[ -z "$hz" ]]; then
      RUNNING=0; active=0; draining=false
    else
      RUNNING=1
      active="$(active_workload_count "$hz")" || exit 1
      draining="$(hfield "$hz" draining)"; draining="${draining:-false}"
    fi
    if [[ "$draining" != "true" && ( "$active" == "0" || "$FORCE" == "1" ) ]]; then
      break
    fi
    flock -u 9
    LOCK_HELD=0
  fi

  if [[ "$(date +%s)" -ge "$deadline" ]]; then
    log "Timeout : $active session(s) encore actives. MAJ abandonnee sans avoir draine ni bloque le noeud."
    exit 3
  fi
  log "Noeud occupe ($active session(s)) : attente sans drain..."
  sleep 1
done

# Capture la vraie cible de rollback sous flock, apres le build et l'attente :
# un autre agent a pu deployer entre-temps.
PREV_TARGET=""
[[ -L "$CURRENT_LINK" ]] && PREV_TARGET="$(readlink "$CURRENT_LINK")"
PREV_HEALTH="$(healthz)"

# --- Courte lease de drain, uniquement pendant la bascule ---
if [[ "$RUNNING" == "1" ]]; then
  log "Fenetre de bascule : drain borne a ${DRAIN_LEASE}s"
  set_drain true || { echo "Drain impossible (serveur injoignable ?)." >&2; exit 1; }
  DRAIN_ARMED=1
  sleep 0.25
  hz="$(healthz)"
  active="$(active_workload_count "$hz")" || exit 1
  if [[ "$active" != "0" && "$FORCE" != "1" ]]; then
    set_drain false || true
    DRAIN_ARMED=0
    log "Course de bascule : $active session(s) viennent de demarrer. Noeud rouvert ; retente."
    exit 3
  fi
fi

# --- Bascule atomique de 'current' ---
log "Bascule current -> releases/$RELEASE_ID"
ln -sfnT "releases/$RELEASE_ID" "$APP_DIR/current.tmp"
mv -Tf "$APP_DIR/current.tmp" "$CURRENT_LINK"
chown -h cst:cst "$CURRENT_LINK"

# --- Redemarrage (efface aussi le drain, etat en memoire) ---
log "Redemarrage de $SERVICE"
systemctl restart "$SERVICE"
# Le nouveau processus repart non draine (etat uniquement en memoire).
DRAIN_ARMED=0

# --- Verification "vraiment revenu" ---
verify() {
  local want_version="$1" want_commit="$2" hz ver commit rdy drn code
  local vdeadline=$(( $(date +%s) + VERIFY_TIMEOUT ))
  while :; do
    hz="$(healthz)"
    ver="$(hfield "$hz" version)"; commit="$(hfield "$hz" commit)"
    rdy="$(hfield "$hz" ready)"; drn="$(hfield "$hz" draining)"
    if [[ "$ver" == "$want_version" && "$commit" == "$want_commit" &&
          "$rdy" == "true" && "$drn" == "false" ]]; then
      # Sonde d'acceptation : POST /api/terminals avec un compte bidon. Un noeud
      # sain repond 400/500 (compte introuvable) ; un noeud draine repondrait
      # 503, un token invalide 401. On accepte donc tout sauf 503/401/000.
      code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 8 -X POST "$BASE/api/terminals" \
        -H "Authorization: Bearer $ADMIN_TOKEN" -H "Content-Type: application/json" \
        -d '{"accountId":"__cst_update_probe__","repoUrl":"","cols":80,"rows":24}' 2>/dev/null || echo 000)"
      case "$code" in
        503|401|000) : ;;    # pas encore pret / probleme -> on continue d'attendre
        *) return 0 ;;
      esac
    fi
    [[ "$(date +%s)" -ge "$vdeadline" ]] && return 1
    sleep 2
  done
}

if verify "$VERSION" "$COMMIT"; then
  rm -f -- "$RELEASE_MARKER"
  RELEASE_MARKER=""
  prune_obsolete_releases "$RELEASE_DIR"
  log "OK : noeud en $VERSION ($COMMIT), pret et non draine."
  exit 0
fi

# --- Rollback ---
log "ECHEC de la verification en $VERSION ($COMMIT)."
if [[ -n "$PREV_TARGET" && "$PREV_TARGET" != "releases/$RELEASE_ID" ]]; then
  PREV_VERSION="$(hfield "$PREV_HEALTH" version)"
  PREV_COMMIT="$(hfield "$PREV_HEALTH" commit)"
  log "Rollback -> $PREV_TARGET"
  ln -sfnT "$PREV_TARGET" "$APP_DIR/current.tmp"
  mv -Tf "$APP_DIR/current.tmp" "$CURRENT_LINK"
  chown -h cst:cst "$CURRENT_LINK"
  systemctl restart "$SERVICE"
  if [[ -n "$PREV_VERSION" && -n "$PREV_COMMIT" ]] && verify "$PREV_VERSION" "$PREV_COMMIT"; then
    log "Rollback OK : noeud restaure en $PREV_VERSION ($PREV_COMMIT)."
  else
    log "ALERTE : rollback n'a pas restaure un etat sain, intervention manuelle requise."
  fi
else
  log "ALERTE : pas de release precedente pour le rollback."
fi
exit 1
