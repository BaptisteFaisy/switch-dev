# Duello sidecar on the Google trial VPS

Duello is intentionally outside the Codex Switch Terminal Compose project. It
reuses the Node/Codex runtime captured in the stable local image tag
`duello-runtime:node22-codex`, while keeping its source, data, provider keys and
Codex account homes on the existing persistent disk.

Install or reconcile it without touching CST:

```bash
sudo install -d -m 0755 /opt/duello
sudo install -m 0644 deploy/duello/compose.yaml /opt/duello/compose.yaml
sudo install -m 0644 deploy/duello/google-trial.env /opt/duello/.env
sudo docker compose --file /opt/duello/compose.yaml up --detach --wait
```

The deployed container is labelled
`com.codex-switch-terminal.rebuild-policy=exclude`. The CST Ansible playbook
checks this label before and after every application rebuild and fails if a
protected workload is stopped, recreated, or attached to the CST Compose
project.

This protects Duello from a CST image/container rebuild. It cannot protect it
from deletion of the VM or its persistent disk. Before a provider reimage or VM
replacement, back up the project path from `google-trial.env`, `/opt/duello`,
and the Codex account homes it uses, or migrate Duello to another VPS.

## Parrainage : compteurs remontés dans duello.fr/dashboard

L'onglet **Parrainage** du dashboard (duello.fr/dashboard) affiche des codes de
parrainage gérés par le dashboard et, pour chaque code, le nombre de personnes
parrainées. Ce compteur vient de l'application Duello : le dashboard n'écrit
rien chez Duello, il interroge un endpoint que Duello doit exposer.

**Côté dashboard** (variables d'environnement du conteneur CST, voir
`deploy/cst-container.env.example`) :

- `CST_DUELLO_REFERRAL_API_URL` — endpoint Duello à interroger. Vide = les codes
  sont affichés avec le compteur « En attente ».
- `CST_DUELLO_REFERRAL_API_KEY` — optionnel, envoyé en `Authorization: Bearer`.
- `CST_DUELLO_APP_URL` — base publique de l'app Duello, sert à construire le
  lien `{appUrl}?ref={code}` partagé aux filleuls (défaut `https://app.duello.fr/`).

**Contrat attendu de l'application Duello** (à implémenter côté Duello, sans
rien changer côté dashboard) :

```
GET {CST_DUELLO_REFERRAL_API_URL}?codes=CODE1,CODE2
Authorization: Bearer {CST_DUELLO_REFERRAL_API_KEY}

200 OK
{ "referrals": { "CODE1": 3, "CODE2": 0 } }
```

Le dashboard accepte aussi la clé `codes` à la place de `referrals`, et pour un
code unique `referralCount`/`count` en nombre. Une réponse non-2xx ou injoignable
est montrée dans l'onglet (compteurs « En attente ») sans bloquer l'affichage
des codes.

## Exposer l'API en direct (api.duello.fr) au lieu du relais tunnel

L'app mobile parle à son backend via l'URL `EXPO_PUBLIC_DUELLO_API_URL`, gravée
au build. Aujourd'hui elle pointe vers le relais Cloudflare
`https://duello-api-relay.duello.workers.dev/api` (tunnel nommé `duello-api` →
port d'origine 8891). Avec une **IP publique statique** et un **domaine**, on peut
servir la même origine en direct : `https://api.duello.fr/api`, sans dépendre de
Cloudflare pour la route API. L'app suit le domaine (pas l'IP) : un futur
changement d'IP n'est qu'une mise à jour DNS, aucun rebuild.

Artefacts dans `deploy/duello/api-direct/` :

- `nginx-api.duello.fr.conf` — bloc nginx reverse proxy (port 80 au départ,
  `certbot --nginx` ajoute le 443) qui transmet à `http://127.0.0.1:8891`, la
  même origine que le tunnel (chemin complet préservé : `/api/auth/...` →
  `127.0.0.1:8891/api/auth/...`).
- `apply-api-direct.sh` — script idempotent à lancer **sur le VPS en root** :
  préflight (DNS + origine 8891), installation nginx/certbot si absents,
  installation de la config, émission Let's Encrypt, vérification publique.
- `STATUS-2026-08-27.md` — état réel de la bascule effectuée (serveur + app
  + publication + caveats + rollback) : à lire en priorité.

### Procédure complète

1. **DNS** — créer l'enregistrement `A  api.duello.fr  → <IP publique statique>`
   (dashboard Cloudflare si le domaine y est, sinon chez le registrar).
2. **Serveur** — sur le VPS, s'assurer que l'origine 8891 répond (`duello-server`
   actif) puis :
   ```bash
   sudo CERTBOT_EMAIL=toi@duello.fr bash deploy/duello/api-direct/apply-api-direct.sh
   ```
   Vérifier ensuite `curl -I https://api.duello.fr/api` (même code que le relais).
3. **App** — dans le projet Duello (sur le VPS), mettre **les deux** à la même
   valeur (invariant de build, voir `work/ssh-bridge/AGENTS.active.md`) :
   - `eas.json` : `EXPO_PUBLIC_DUELLO_API_URL=https://api.duello.fr/api` dans
     tous les profils de build ;
   - `config/cloudflare-tunnel.json` : `apiUrl` identique.
   Puis rebuild + republication de l'app (`docs/METTRE_A_JOUR_LAPP.md`).
4. **Sortie** — le tunnel `duello-api` n'est **pas** coupé par cette bascule :
   les anciennes builds continuent de marcher via le relais. Une fois les
   nouvelles builds publiées et vérifiées, arrêter proprement le tunnel.

### Côté app (projet Duello, VPS Google — `env-prepapp-36934ccf`)

La source de vérité pour les builds **release** est `config/cloudflare-tunnel.json`
(`cloudflareTunnel.apiUrl`, lu par `src/config/platform.ts`) ; `eas.json` est un
miroir vérifié par la porte de build. La bascule consiste donc à mettre **les
deux** à la même valeur, en respectant l'invariant du projet
`apiUrl` ≡ `hostname` (validateur `validatePublicTunnelConfig`) :

```jsonc
// config/cloudflare-tunnel.json
{
  "mode": "relay",
  "durability": "permanent",
  "tunnelName": "duello-api",
  "hostname": "api.duello.fr",   // ← changé
  "apiUrl": "https://api.duello.fr/api", // ← changé
  "originPort": 8891             // inchangé
}
```

`eas.json` : `EXPO_PUBLIC_DUELLO_API_URL=https://api.duello.fr/api` dans les
3 profils (development, preview, production). Vérification (la porte de build
réelle, exécutée dans le conteneur `duello-server`) :

```bash
node scripts/tunnel.mjs check   # = npm run tunnel:check = npm run build:check
# → Configuration Cloudflare du build valide. URL fixe : https://api.duello.fr/api
```

⚠️ **Ne pas relancer `npm run tunnel:setup`** : il redéploie le Worker relay et
réécrit `config/cloudflare-tunnel.json` + `eas.json` avec l'URL `*.workers.dev`
(annulant la bascule). `tunnel:setup:named` créerait un tunnel cloudflared vers
l'origine du conteneur (pas l'API Azure) — à exclure aussi.

⚠️ **Redémarrage du conteneur `duello-server`** : en mode relay,
`node scripts/tunnel.mjs` (commande `auto`) exécute `quick()` qui redéploie le
Worker relay puis exige que l'URL déployée == `publicConfig.hostname`. Avec
`hostname=api.duello.fr`, cette vérification échoue → le conteneur sort en
erreur (crash-loop si `restart: unless-stopped`). **Observé lors de la bascule
(27/08/2026) : le conteneur n'a PAS été redémarré manuellement et tourne
toujours en mode relay avec son URL `*.workers.dev` d'origine — rien n'a en fait
été relancé ni crash-loope pendant l'opération, car on n'a pas touché au
process `tunnel.mjs`.** Le risque ne se matérialise que si le conteneur est
redémarré en l'état. Options si cela arrive : (a) corriger `scripts/tunnel.mjs`
(neutraliser la vérification `pointRelayAt`) ; (b) une fois les anciennes builds
écoulées, arrêter le conteneur (le relay n'est plus nécessaire).

### Publication OTA production (canal EAS production, Android)

La bascule est effective **côté serveur dès que la config nginx + le certificat
sont posés** ; côté app, il faut publier un nouvel update pour que les appareils
reçoivent l'URL `api.duello.fr`. Commandes dans le conteneur `duello-server`
(qui détient l'auth Expo/EAS) :

```bash
# Dans le conteneur, EN USER cst (jamais root) :
#   - EAS_NO_VCS=1 : désactive l'exigence git d'eas-cli (sinon il bloque sur
#     « It looks like you haven't initialized the git repository yet »).
#   - stdin /dev/null + nohup : eas update ne doit jamais attendre une saisie.
docker exec -u 10001 duello-server sh -lc 'cd <projet> && nohup env EAS_NO_VCS=1 \\'
  'npm run update:production:android -- --message "Bascule-API-vers-api.duello.fr" \\'
  '> /tmp/eas-prod.log 2>&1 < /dev/null &'
cat "$(docker exec duello-server sh -lc 'cat /tmp/eas-prod.log')"  # surveiller
```

Résultat type (confirmé le 27/08/2026, build `1.0.16`) :

```
Branch             production
Runtime version    1.0.16
Platform           android
Update group ID    35a9bbe0-6822-4713-b9e2-73c99123b6e1
Message            Bascule-API-vers-api.duello.fr
EAS Dashboard      https://expo.dev/accounts/liquidplus/projects/duello-mobile/updates/35a9
```

**Pièges rencontrés à la publication** (corrigés sur place, à connaître) :

- **Garde-fou mémoire (`eas-update-memory.mjs`)** : avant de lancer Metro/eas
  update, le projet attend d'avoir ~1,9 Gio bruts de marge mémoire, sinon il
  attend `DUELLO_EAS_UPDATE_MEMORY_WAIT_SECONDS` (30 min par défaut) puis
  échoue. Le conteneur `duello-server` était limité à 1 Go (`mem_limit: 1g`) →
  publication impossible. Tout a été relevé à 3 Go avec
  `docker update --memory 3g --memory-swap 3g duello-server` — le réglage est
  **effectif immédiatement** (le conteneur n'a pas été recréé pour cette
  publication), mais il n'est **pas persistant** : un prochain `up` du compose
  (`deploy/duello/compose.yaml`, `mem_limit: 1g`) reviendra à 1 Go — reporter la
  limite à ≥ 3 Go dans `deploy/duello/compose.yaml` si on veut pouvoir republier
  un update (ou régler `DUELLO_EAS_UPDATE_MEMORY_WAIT_SECONDS`).
- **Ownership des caches (`~/.npm`, `~/.expo`)** : exécuter eas en user cst (uid
  10001). Tout lancement en root pollue le cache npm (EACCES ensuite) et crée
  `~/.expo/state.json` en root (EACCES ensuite) → corriger par
  `chown -R 10001:10001 /home/cst/.npm /home/cst/.expo`.
- **Le garde-fou de build** (`npm run build:check` / `tunnel.mjs check`) passe
  une fois `eas.json` et `cloudflare-tunnel.json` alignés sur `api.duello.fr`.
- Les WebSockets temps réel et l'endpoint `/download` suivent `DUELLO_API_URL` :
  tout passe par la même origine directe.

### Rollback

- Serveur : `rm /etc/nginx/sites-enabled/api.duello.fr && nginx -t && systemctl reload nginx`
  (certificat conservé ; suppression éventuelle : `certbot delete --cert-name api.duello.fr`).
- App : restaurer les sauvegardes `eas.json.bak-api-direct-*` et
  `config/cloudflare-tunnel.json.bak-api-direct-*` (créées lors de la bascule)
  puis rebuilder — le relais étant resté actif, aucun utilisateur n'est impacté
  pendant la transition.

### Notes

- **NAT Azure** : l'IP publique n'existe pas sur une interface de la VM (bind
  dessus → `EADDRNOTAVAIL`). Azure translate IP publique → IP privée : nginx
  doit écouter sur l'IP **privée** (ex. `172.16.0.4:443`), détectée
automatiquement par le script.
- **Conflit 443 avec tailscaled** : `tailscale serve` (HTTPS du tailnet
  `azure-duello.tail3a8bdf.ts.net` → CST web) occupe 443 sur ses adresses ; un
  bind wildcard `listen 443 ssl` échoue (`EADDRINUSE`). Le script remplace donc
  la ligne générée par certbot par `listen <IP privée>:443 ssl;`.
- **NSG Azure** : ajouter les règles entrantes TCP 80 et TCP 443 (Source Any) —
  ce VPS ne laisse passer que le SSH par défaut.
- **Hairpin** : la VM ne peut pas se joindre via sa propre IP publique ; la
  vérification du script cible l'IP privée avec le SNI du domaine.
- Ne pas ajouter de slash final à `proxy_pass` (sinon nginx réécrit le chemin et
  casse `/api/*`).
- `client_max_body_size 200m` : les échanges de l'app peuvent porter de gros
  corps ; la valeur par défaut d'nginx (1 m) est trop basse.
- L'app contient une fonction de sanitisation d'override du relais qui laisse
  passer un domaine direct comme `api.duello.fr` tel quel (elle ne filtre que
  les hôtes `trycloudflare.com` et le relais par défaut) — pas de traitement
  spécial à prévoir côté app.

