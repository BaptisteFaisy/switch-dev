# Règles d’accès aux fichiers

Les opérations de lecture et d’écriture doivent rester dans le projet courant. Toute demande concernant un chemin situé hors du projet doit être signalée à l’utilisateur et nécessite une autorisation explicite ; cette autorisation ne permet pas de contourner les restrictions de l’outil ou de l’environnement.

# Tâches à faire

Quand l'utilisateur demande d'ajouter une tâche à faire, l'écrire dans
**Switch** (VPS) : onglet **Tâches**, sur le compte **baptiste.faisy**.
Depuis la mise en place de l'API serveur (`src-tauri/src/tasks.rs`), on peut
also l'ajouter à distance :

- `POST /api/tasks?account=baptiste.faisy` avec l'en-tête
  `Authorization: Bearer <jeton admin>` et un corps JSON
  `{ "id": "...", "title": "...", "priority": "normal", "dueDate": null }`
  (liste : `GET /api/tasks?account=...`, remplacement : `PUT /api/tasks`,
  suppression : `DELETE /api/tasks/<id>?account=...`).
- Le compte se résout par id ou nom d'utilisateur ; sans paramètre, le jeton
  admin vise la liste technique `server-admin`.
- Le client web synchronise l'onglet Tâches avec le serveur : le serveur est
  la source de vérité, le localStorage reste le cache hors-ligne.

Exemple de tâche enregistrée (en attente) :
- **Rebuild le serveur du VPS** pour appliquer le fix du cookie de session
  Freebuff Cloud (normalisation `__Secure-next-auth.session-token` côté
  serveur dans `freebuff_cloud.rs`) — le fix est côté serveur, redéployer
  le binaire suffit, le client web seul ne suffit pas.

# Terminaux et fenêtres console

- Dès qu'un terminal/console n'est plus nécessaire (script terminé, test fini), le fermer immédiatement ; ne jamais laisser de terminal interactif ouvert derrière soi.
- Les helpers et scripts lancés par l'agent doivent s'exécuter sans fenêtre visible : `powershell -WindowStyle Hidden`, ou action `open` du relais écran avec `hidden: true` (CREATE_NO_WINDOW). Une fenêtre ouverte explicitement pour un test (banc de saisie, navigateur de mission) doit être fermée en fin de tâche.
- Ne jamais fermer les terminaux de l'utilisateur (notamment les fenêtres « Administrateur : Windows PowerShell ») ni l'infrastructure de l'application (processus `launch_codex_app_tools_mcp`, runtimes codex, scripts de démarrage).

# Procédure obligatoire avant build et publication

- Avant chaque build, analyser l’état des modifications et le diff afin de distinguer les nouveaux changements des artefacts ou changements déjà traités.
- Avant chaque commit, isoler strictement les changements de la fonctionnalité demandée ; ne jamais inclure les modifications préexistantes ou sans rapport, même si elles touchent les mêmes fichiers.
- Ne jamais reconstruire ou publier un état non vérifié ; relancer les vérifications pertinentes après toute nouvelle modification.
- Après chaque modification fonctionnelle, préparer un commit Git dédié avec un message explicite et vérifier qu’il ne contient aucun changement sans rapport.
- Le push GitHub est obligatoire pour valider une modification destinée au dépôt distant, mais il doit être effectué uniquement après confirmation explicite de l’utilisateur juste avant `git push`.
- Avant un commit, vérifier le diff et l’historique récent ; ne jamais inclure de secrets, fichiers générés ou modifications sans rapport.
- Avant tout déploiement distant, analyser le diff depuis le dernier commit, valider le build et les tests, puis demander une confirmation explicite pour l’action réseau.

# Protection des chats de développement

- La copie locale/SSD reste exclusivement la version de développement. Son serveur HTTP doit être lancé via `scripts/start-switch-development-runtime.ps1` : le binaire et `dist` sont mis en cache sur le disque interne, tandis que les données et workspaces restent sur le Samsung T7.
- Pour appliquer un nouveau binaire au serveur de développement, utiliser `scripts/restart-switch-development.ps1`. Ce script attend que `activeChatTurns + activeTerminals` soit nul, arme une courte lease de drain, puis redémarre.
- Ne jamais arrêter directement `cst-server.exe`, utiliser `Stop-Process`, `taskkill` ou redémarrer la tâche planifiée tant qu’un chat ou un terminal est actif. L’option `-Force` du script de redémarrage n’est autorisée que sur demande explicite de l’utilisateur.
- Un build peut être préparé pendant un chat, mais sa bascule vers le runtime ne doit avoir lieu qu’après l’inactivité constatée par le script protégé.

# Coordination multi-agents du déploiement

Le nœud de développement est partagé : plusieurs agents construisent et déploient en parallèle sur le même paquet `E:\AppsData\SwitchDevelopment\app`. Sans coordination, un agent peut écraser le binaire d'un autre entre sa construction et son redémarrage.

- Un **verrou de déploiement partagé** (`Local\SwitchDevelopmentDeploy-18082`) est tenu par `start-switch-development-runtime.ps1` pendant la mise en cache du paquet et par `restart-switch-development.ps1` pendant la bascule (arrêt du serveur). Il garantit qu'on ne sert jamais un paquet incomplet ni un mélange (binaire d'un agent + `dist` d'un autre).
- Tout agent qui doit déposer un nouveau `cst-server.exe` / `dist` dans `app\` doit d'abord acquérir le verrou `Local\SwitchDevelopmentDeploy-18082` (ou réutiliser un wrapper qui le fait), le temps de la copie, puis le relâcher avant d'invoquer `restart-switch-development.ps1`. Cela évite d'écraser le paquet pendant qu'un autre agent le lit ou le déploie. Exemple PowerShell :
  ```powershell
  $m = [Threading.Mutex]::new($false, 'Local\SwitchDevelopmentDeploy-18082')
  $held = $false
  try {
    $held = $m.WaitOne(300)
    if (-not $held) { throw 'Un autre déploiement de développement est en cours ; attendre.' }
    # copie de cst-server.exe et dist\ dans app\ ici
  } finally { if ($held) { $m.ReleaseMutex() }; $m.Dispose() }
  ```
- Ne jamais écraser `app\cst-server.exe` ou `app\dist\` pendant qu'un redémarrage est en cours (les scripts tiennent le verrou à ce moment ; respecter leur étiquette).
- Avant de redéployer, vérifier que l'on ne repart pas d'un état qui retirerait des changements déjà en production : `git log --oneline -3` (clone `app`) et comparer avec le paquet déployé (`SwitchDevelopmentRuntime\releases`).

# Obligation : inclure le travail des autres chats avant de déployer

Le nœud accueille plusieurs chats en parallèle (des agents `codex.exe` et le bureau tournent simultanément). **Avant toute bascule vers le runtime** (créer un build, le déposer dans `app\`, puis `restart-switch-development.ps1`), l'agent a l'obligation de **prendre connaissance de l'activité des autres chats et d'inclure leurs changements**, afin de ne jamais écraser ou retirer leur travail.

- Lancer systématiquement `scripts\check-other-agents.ps1 -Strict`. Ce script affiche, de façon objective :
  1. les **commits git récents** du clone `app` (activité des autres chats),
  2. les **changements non commités** (quelqu'un est en train de travailler),
  3. les **builds déployés récents** (qui peuvent être d'autres chats),
  4. les **fichiers cibles que l'agent s'apprête à remplacer** (`-OverlapPaths src/main.ts,src-tauri/src/gmail.rs`).
  En présence de changements non commités ou d'un chevauchement de fichiers, il renvoie le code de sortie **2** et interdit le déploiement. Les commits déjà dans `HEAD` et les builds récents sont signalés à titre informatif, puisqu'ils doivent être vérifiés mais sont déjà observables.
- `restart-switch-development.ps1 -EnforceCollaboration` appelle ce check au moment de la bascule et **refuse de redémarrer le serveur** si une concurrence active est détectée (commits récents, working tree sale, builds concurrents). Sans `-EnforceCollaboration`, le check est affiché à titre informatif.
- Si une concurrence est signalée : **intégrer** les changements des autres chats avant de déployer — récupérer leurs commits (`git log` / dernière release) et ne pas les retirer ; vérifier qu'aucun fichier qu'on remplace ne leur appartient (`-OverlapPaths`). Le déploiement doit « s'empiler au-dessus » de leur travail, pas le remplacer, sauf demande explicite de l'utilisateur.
- Le watchdog (relais automatique du serveur) n'est jamais bloqué par cette obligation : elle ne s'applique qu'aux bascules déclenchées par un agent ou l'utilisateur, pas au redémarrage d'entretien.
