# Plan des 10 000 workers de Switch développement

Le fichier explicatif de référence pour porter `workerCount` jusqu'à 10 000 workers dans une même orchestration Switch développement est :

`E:\AppsData\SwitchDevelopment\app\docs\architecture\PLAN-10000-AGENTS-SWITCH-DEV-ACTUEL.md`

Chemin relatif au dépôt :

`docs/architecture/PLAN-10000-AGENTS-SWITCH-DEV-ACTUEL.md`

Lire aussi `AGENTS.md` avant toute modification, construction, bascule ou publication. Ce plan ne donne aucune autorisation d'agir sur Switch stable/production sur le VPS Azure.

## API de données Instagram (Instagram Data API) — pour les agents d'exécution

Les agents Switch d'exécution peuvent consommer les statistiques Instagram (vues/likes par compte) via une API locale. **Contrat auto-découvert : `GET /api/v1/discovery`.**

- **Hôte/port de dev** : `http://127.0.0.1:8002` (`uvicorn app:app`, dossier `E:\instagram-likes-api`).
- **Auth (optionnelle)** : header `X-API-Key`, clé = variable `INSTAGRAM_API_KEY` dans `.env`. Sans clé configurée, accès local ouvert.
- **Endpoints** :
  - `GET  /api/v1/accounts` → comptes disponibles
  - `GET  /api/v1/accounts/{username}` → profil + vidéos + totaux
  - `GET  /api/v1/accounts/{username}/views` → total vues + détail par vidéo (trié desc)
  - `GET  /api/v1/accounts/{username}/likes` → total likes + détail par vidéo (trié desc)
  - `POST /api/v1/accounts/{username}/ingest` → un collecteur pousse un résultat JSON (payload de `parse_results.py` ou `harvest_views.py`)
- **Données** : non officielles, collectées localement (4G téléphone + session valide, via `web_profile_info` ou collecte écran ADB « comme un user »). Le champ vues privilégie `plays` (API), sinon `views` (écran).
- **Fraîcheur** : un compteur n'est fiable que si la collecte est allée **jusqu'au bout** du profil (`harvest_views.py <username>`). Après re-collecte complète, re-ingérer via `POST /ingest`.

Toile de fond : l'accès officiel (Meta Graph API) reste l'alternative conforme pour la prod.

## Warmup des feeds vidéo (Instagram / TikTok) — défilement « humain »

Pour chauffer un compte (rendre la session active) sans aucune requête API, les agents d'exécution peuvent déclencher un défilement humain du feed vidéo sur le téléphone USB (Instagram Reels / TikTok « Pour toi »). **Instructions complètes : `warmup.md`** (même dossier que ce fichier ; chemin absolu `E:\AppsData\SwitchDevelopment\app\warmup.md`).

Résumé du contrat : gestes aléatoires (pause 3–15 s avant et entre chaque geste, jamais identiques, volontairement de travers), durée maximale 30 min, via `python warmup_scroll.py` (dossier `E:\instagram-likes-api`, téléphone branché USB + écran déverrouillé). L'outil s'arrête proprement à la limite de durée ou si le téléphone se débranche.
