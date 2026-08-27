#!/usr/bin/env bash
# tokscale-all — usage codex + freebuff consolidé de TOUS les comptes du conteneur Switch.
#
# Historique COMPLET (pas seulement les sessions actives) :
#   - Codex : fusionne sessions/ + sessions-archive/ + archived_sessions/ (là où
#     Switch range les chats avant qu'ils soient purgés sous pression mémoire).
#   - Freebuff : fusionne projects/ + projects-archive/ (chat-messages.json).
#
# Durabilité : chaque session est enregistrée par sessionId dans un snapshot
# cumulatif (/srv/cst/tokscale-history.json). Ce snapshot ne fait que croître :
# même quand les fichiers locaux sont supprimés, l'usage déjà tracé reste.
#
# Optimisé faible RAM : binaire local (pas de npx), cœur natif mono-thread,
# séquentiel, filtre par client, --json, stage léger par symlinks.
#
# Usage :
#   tokscale-all.sh                # rapport { window, history } (stdout)
#   tokscale-all.sh --submit       # + envoi au leaderboard (TOKSCALE_API_TOKEN requis)
#   TOKSCALE_RANGE="--month" tokscale-all.sh   # fenêtre (défaut --week)
set -euo pipefail

DATA_ROOT="${CST_DATA_DIR:-/srv/cst}/codex-homes"
RANGE="${TOKSCALE_RANGE:---week}"
HISTORY="${TOKSCALE_HISTORY:-/srv/cst/tokscale-history.json}"
STAGER="$(dirname "$(readlink -f "$0")")/freebuff-stage.mjs"
[ -f "$STAGER" ] || STAGER="/srv/cst/freebuff-stage.mjs"
DO_SUBMIT=0
[ "${1:-}" = "--submit" ] && DO_SUBMIT=1

# Cœur natif mono-thread : le scan parallèle est le principal pic mémoire.
export RAYON_NUM_THREADS="${TOKSCALE_THREADS:-1}"

command -v node >/dev/null 2>&1 || { echo "node requis" >&2; exit 1; }
[ -d "$DATA_ROOT" ] || { echo "codex-homes introuvable: $DATA_ROOT" >&2; exit 1; }

# Binaire local (faible RAM) ; bascule npx seulement s'il a disparu (recreate).
run_tokscale() {
  if command -v tokscale >/dev/null 2>&1; then
    tokscale "$@"
  else
    npx --yes tokscale@latest "$@"
  fi
}

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# --- Codex : sessions + sessions-archive + archived_sessions -----------------
for home in "$DATA_ROOT"/*/; do
  name="$(basename "$home")"
  has=0
  for d in sessions sessions-archive archived_sessions; do
    [ -d "$home/$d" ] && has=1
  done
  [ "$has" = 1 ] || continue

  stage="$TMP/codex-$name"
  mkdir -p "$stage/sessions"
  for d in sessions sessions-archive archived_sessions; do
    [ -d "$home/$d" ] && cp -rs "$home/$d/." "$stage/sessions/" 2>/dev/null || true
  done
  for f in config.toml auth.json; do
    [ -f "$home/$f" ] && cp -s "$home/$f" "$stage/$f" 2>/dev/null || true
  done

  CODEX_HOME="$stage" run_tokscale models --json --client codex --group-by session,model "$RANGE" \
    > "$TMP/codex-$name.json" 2>/dev/null || true
done

# --- Freebuff : stage léger (marqueur base2-free) puis scan ------------------
for home in "$DATA_ROOT"/*/; do
  name="$(basename "$home")"
  [ -d "$home/.config/manicode" ] || continue

  stage="$TMP/fb-$name"
  mkdir -p "$stage"
  node "$STAGER" "$home" "$stage" >/dev/null 2>&1 || true

  FREEBUFF_DATA_DIR="$stage" run_tokscale models --json --client freebuff --group-by session,model "$RANGE" \
    > "$TMP/freebuff-$name.json" 2>/dev/null || true
done

# --- Correction Freebuff : estimateur cumulatif (meme logique que account_usage.rs) ---
ESTIMATE_SCRIPT="$(dirname "$(readlink -f "$0")")/freebuff-estimate.mjs"
[ -f "$ESTIMATE_SCRIPT" ] || ESTIMATE_SCRIPT="/srv/cst/freebuff-estimate.mjs"
FREEBUFF_ESTIMATE="$TMP/freebuff-corrected.json"
if [ -f "$ESTIMATE_SCRIPT" ]; then
  node "$ESTIMATE_SCRIPT" "$DATA_ROOT" > "$FREEBUFF_ESTIMATE" 2>/dev/null || true
fi

# --- Fusion + snapshot durable (dedup par sessionId) -------------------------
node - "$TMP" "$HISTORY" "$FREEBUFF_ESTIMATE" <<'NODE'
const fs = require("fs");
const dir = process.argv[2];
const historyPath = process.argv[3];
const estimatePath = process.argv[4];

// 0) charge les estimations Freebuff corrigees (transcript cumulatif)
let corrected = new Map();
if (estimatePath) {
  try {
    const est = JSON.parse(fs.readFileSync(estimatePath, "utf8"));
    for (const e of est) {
      if (!e.sessionId) continue;
      corrected.set(`${e.account}\u0000${e.sessionId}`, { input: e.input, output: e.output, reasoning: e.reasoning, messageCount: e.messageCount, cacheRead: e.cacheRead ?? 0, cacheWrite: e.cacheWrite ?? 0, cacheConfidence: e.cacheConfidence ?? "unavailable" });
    }
  } catch {}
}

// 1) collecte des sessions vues (une entrée par sessionId)
const seen = new Map();
for (const f of fs.readdirSync(dir)) {
  const m = f.match(/^(codex|freebuff)-(.*)\.json$/);
  if (!m) continue;
  const account = m[2];
  let j;
  try { j = JSON.parse(fs.readFileSync(`${dir}/${f}`, "utf8")); } catch { continue; }
  const entries = Array.isArray(j?.entries) ? j.entries : [];
  for (const e of entries) {
    if (!e.sessionId) continue;
    // Pour Freebuff : utiliser notre estimateur cumulatif (transcript
    // entier comme input, 4 chars/token) plutot que l'estimation naive
    // de tokscale (dernier message utilisateur seul).
    // tokscale ajoute un prefix (ex. "stage/env-abc123/") au sessionId;
    // notre estimateur utilise le nom brut du dossier de chat.
    const sessionSlug = String(e.sessionId || "").split("/").pop() || e.sessionId;
    const overridden = e.client === "freebuff" ? corrected.get(`${account}\u0000${sessionSlug}`) : null;
    const rec = { client: e.client, model: e.model, provider: e.provider || null, account,
      input: overridden ? overridden.input : (e.input ?? e.inputTokens ?? 0),
      output: overridden ? overridden.output : (e.output ?? e.outputTokens ?? 0),
      cacheRead: overridden ? overridden.cacheRead : (e.cacheRead ?? e.cachedInputTokens ?? 0),
      cacheWrite: overridden ? overridden.cacheWrite : (e.cacheWrite || 0),
      cacheConfidence: overridden ? overridden.cacheConfidence : (e.cacheRead !== undefined ? "native" : "unavailable"),
      reasoning: overridden ? overridden.reasoning : (e.reasoning || 0),
      messageCount: overridden ? overridden.messageCount : (e.messageCount || 0),
      cost: e.cost || 0 };
    const prev = seen.get(e.sessionId);
    if (prev) {
      // même session vue deux fois (chevauchement sessions/archive) : garder le max
      for (const k of ["input","output","cacheRead","cacheWrite","reasoning","messageCount","cost"]) {
        prev[k] = Math.max(prev[k], rec[k]);
      }
    } else {
      seen.set(e.sessionId, rec);
    }
  }
}

// 2) snapshot cumulatif : fusion par sessionId, jamais de suppression
let history = { updatedAt: null, sessions: {} };
try {
  const h = JSON.parse(fs.readFileSync(historyPath, "utf8"));
  if (h && h.sessions) history = h;
} catch {}
const now = new Date().toISOString();
for (const [sid, rec] of seen) {
  const old = history.sessions[sid];
  history.sessions[sid] = { ...rec, firstSeen: (old && old.firstSeen) || now, lastSeen: now };
}
history.updatedAt = now;
fs.writeFileSync(historyPath, JSON.stringify(history, null, 2));

// 3) agrégation par client+modèle
function aggregate(sessMap) {
  const acc = new Map();
  for (const [, e] of sessMap) {
    const key = `${e.client}\u0000${e.model}\u0000${e.provider}`;
    let t = acc.get(key);
    if (!t) t = { client: e.client, model: e.model, provider: e.provider,
                 accounts: new Set(), input: 0, output: 0, cacheRead: 0, cacheWrite: 0,
                 cacheConfidence: e.cacheConfidence || "unavailable", reasoning: 0, messageCount: 0, cost: 0 };
    acc.set(key, t);
    t.accounts.add(e.account);
    t.input += e.input; t.output += e.output; t.cacheRead += e.cacheRead;
    t.cacheConfidence = t.cacheConfidence === "native" || e.cacheConfidence === "native" ? "native" : "unavailable";
    t.cacheWrite += e.cacheWrite; t.reasoning += e.reasoning;
    t.messageCount += e.messageCount; t.cost += e.cost;
  }
  return [...acc.values()].map(t => ({ ...t, accounts: [...t.accounts].sort(),
    cost: Math.round(t.cost * 1e6) / 1e6 }));
}
function total(entries) {
  return entries.reduce((s, e) => ({
    input: s.input + e.input, output: s.output + e.output,
    cacheRead: s.cacheRead + e.cacheRead, cacheWrite: s.cacheWrite + e.cacheWrite,
    reasoning: s.reasoning + e.reasoning, messageCount: s.messageCount + e.messageCount,
    cost: Math.round((s.cost + e.cost) * 1e6) / 1e6,
  }), { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, reasoning: 0, messageCount: 0, cost: 0 });
}

const windowEntries = aggregate(seen);
const historyEntries = aggregate(new Map(Object.entries(history.sessions)));
console.log(JSON.stringify({
  window: { entries: windowEntries, total: total(windowEntries), sessions: seen.size },
  history: { entries: historyEntries, total: total(historyEntries), sessions: Object.keys(history.sessions).length },
}, null, 2));
NODE

# --- Leaderboard : submit par compte (staging = historique complet) ----------
if [ "$DO_SUBMIT" = "1" ]; then
  [ -n "${TOKSCALE_API_TOKEN:-}" ] || { echo "TOKSCALE_API_TOKEN manquant" >&2; exit 1; }
  for home in "$DATA_ROOT"/*/; do
    name="$(basename "$home")"
    if [ -d "$TMP/codex-$name" ]; then
      CODEX_HOME="$TMP/codex-$name" run_tokscale submit --client codex "$RANGE" || true
    fi
    if [ -d "$TMP/fb-$name" ]; then
      FREEBUFF_DATA_DIR="$TMP/fb-$name" run_tokscale submit --client freebuff "$RANGE" || true
    fi
  done
fi
