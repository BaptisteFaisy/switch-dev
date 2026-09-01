# Switch développement : plan pour porter une orchestration à 10 000 workers

Date de la baseline : 30 août 2026  
Périmètre : Switch développement uniquement, sur le SSD Samsung T7 et le serveur de développement prévu.  
Hors périmètre : Switch stable/production sur le VPS Azure, déploiement, redémarrage et modification fonctionnelle immédiate.

## 1. Décision d'architecture

Switch doit faire passer le champ actuel **`workerCount` de `1..5` à `1..10 000` dans une seule orchestration**. Il ne s'agit pas d'afficher 10 000 agents logiques ou de répartir artificiellement la demande dans dix orchestrations de 1 000 : le même `OrchestrationSnapshot` doit pouvoir contenir, piloter, interrompre et restaurer 10 000 workers réels.

Chaque worker possède son propre identifiant, sa tâche, son compte/provider, sa session, ses tours, ses preuves, ses messages, son état et ses artefacts. Le plafond de workers augmente, mais la valeur par défaut peut rester à 5 pour ne pas lancer accidentellement une campagne coûteuse.

Les trois paramètres exposés doivent accepter 10 000 :

- `workerCount` : nombre de workers du run, maximum 10 000 ;
- `maxTaskCount` : nombre maximal de tâches produites, maximum 10 000 ;
- `maxConcurrency` : nombre maximal de tours de workers API simultanément autorisés dans ce run, maximum 10 000.

`maxConcurrency: 10000` doit donc être un vrai contrat du scheduler, pas une valeur décorative. Le gouverneur peut réduire temporairement le débit lorsque le provider renvoie des `429`, que la mémoire baisse ou que les validations saturent le CPU, puis remonter automatiquement. Il ne doit pas exister de plafond architectural caché à 5, 64, 200 ou 1 000.

Dix mille workers simultanés ne doivent toutefois pas signifier 10 000 processus `codex.exe` ni 10 000 worktrees permanents. Pour atteindre cette concurrence avec les appels API demandés, les workers orchestrés utiliseront principalement un exécuteur HTTP asynchrone ; les processus CLI actuels resteront disponibles pour les chats interactifs et les providers concernés.

Le système actuel est conservé et étendu : serveur Rust, `ChatTurnManager`, providers/comptes, sessions, orchestration avec preuves/reviews/tests, isolation Git, WebSocket runtime et release gate. Il ne faut pas reconstruire Switch autour d'une pile différente.

## 2. Baseline réellement observée

### Paquet et runtime de développement

- dépôt : `E:\AppsData\SwitchDevelopment\app` ;
- branche observée à la vérification finale : `main`, commit `017feb13fdaed1fa2694c1ff5fcca5ae70d8e078` (`feat(dev): verrouiller les releases Switch`) ;
- état Git observé : branche en avance de 42 commits et en retard de 7 commits sur `origin/main`, avec des fichiers non suivis préexistants à préserver ;
- release locale acceptée : génération `202608301618` ;
- commit source enregistré pour la release active : `7b3a3a976cdc9a77a3957a78113090e5e8814377` ;
- build frontend : `dev-chat-queue-drain-v1-20260830` ;
- entrée frontend locale : `assets/index-BmdqKW3K.js` ;
- runtime : `C:\Users\jeanp\AppData\Local\SwitchDevelopmentRuntime\releases\012819DBBD62A68C-DE3ED962EBEC3123` ;
- serveur local : `127.0.0.1:18082` ;
- données et workspaces : `E:\AppsData\SwitchDevelopment\data` sur le T7 ;
- URL Switch développement : `https://pc-fixe-cst.tail3a8bdf.ts.net:10000/` ;
- `/healthz` local observé : prêt, non drainé, zéro chat et zéro terminal actif, capacité de nœud `2` ;
- version embarquée du binaire accepté : `cst-server 0.1.0 (c5a042f18f21b1da5766ec1bc4fbb37c37e79316)`.

Le HEAD du dépôt est donc plus récent que la release exécutée : le commit `017feb13f` n'est pas présenté ici comme déjà déployé. Le paquet accepté enregistre la source `7b3a3a9`, tandis que son binaire annonce `c5a042f`, ancêtre de cette source. Cela est acceptable uniquement si les commits intermédiaires ne changent pas le backend ; chaque futur benchmark backend devra enregistrer sans ambiguïté le commit du binaire réellement exécuté.

### Écart de routage à corriger avant toute mesure

Le 30 août 2026, le runtime local et `app\dist` servent `assets/index-BmdqKW3K.js`, mais la passerelle publique sur le port `10000` renvoie encore `assets/index-CCHd9qZq.js`. Aucun benchmark UI public n'est valide tant que la passerelle, le cache HTTP et le service worker ne servent pas la release acceptée. Ce contrôle doit devenir automatique : build ID, hash de l'index, hash de l'asset d'entrée et commit backend doivent être exposés par un endpoint de diagnostic puis comparés de bout en bout.

### Architecture actuelle à préserver

- `cst-server` est un serveur Rust/Axum/Tokio ;
- le frontend TypeScript/Vite consomme les API `/api/orchestrations`, `/api/autonomous-agents`, chats, terminaux, workspaces et le flux WebSocket runtime ;
- `ChatTurnManager` lance aujourd'hui un processus provider par tour, notamment `codex exec`, et impose une seule utilisation simultanée d'une même session ;
- l'admission des chats utilise `CST_CHAT_MAX_ACTIVE` et des seuils de mémoire hôte/conteneur ; la valeur numérique par défaut est illimitée, mais la configuration locale annonce actuellement une capacité de nœud de `2` ;
- l'orchestration produit un worktree orchestrateur et un worktree par tâche matérialisée ;
- chaque worker rend une preuve structurée, puis l'orchestrateur revoit et valide réellement le patch ;
- un testeur est créé au minimum, puis un testeur par tranche de cinq workers ;
- la publication finale est refusée si le HEAD ou l'état du dépôt source a changé ;
- la release de développement est protégée par le mutex partagé, le drain et `switch-development-release-gate.ps1`.

### Limites actuelles confirmées dans le code

1. Le frontend fixe `MAX_ORCHESTRATION_WORKER_COUNT = 5`.
2. Le backend possède une limite absolue de 1 000 workers par orchestration et une concurrence configurée jusqu'à 1 000.
3. `max_concurrency` est validé et stocké, mais ne pilote pas plusieurs workers simultanés dans un même run.
4. Un run ne possède qu'un `current_turn_id`, un `current_start_id` et un `current_validation_id`. Il ne peut donc exécuter qu'un seul tour à la fois.
5. Le budget global compte les runs occupés, pas les workers réellement en vol.
6. Quatre drivers par défaut parcourent tous les runs toutes les 250 ms, puis prennent un verrou par run.
7. Chaque mutation verrouille le store global, clone tout le store et réécrit entièrement `orchestrated-runs.json` en JSON formaté.
8. Les API de liste renvoient de gros snapshots et l'UI n'est pas conçue pour rendre 10 000 cartes ou conversations.
9. Un worktree par tâche et un testeur persistant par cinq workers conduiraient à environ 12 001 identités pour 10 000 workers, avec des milliers de dossiers Git : ce modèle ne passe pas à l'échelle.
10. Les instructions des enfants interdisent actuellement toute sous-orchestration. Une hiérarchie devra être autorisée uniquement pour des rôles de chef de groupe explicitement créés par le scheduler.

Le changement visible devra bien relever les constantes, validations et contrôles UI jusqu'à 10 000, mais augmenter seulement `5`, `1 000` ou `8` sans corriger le scheduler et le store ne produirait pas de parallélisme réel.

## 3. Topologie cible

```text
Une orchestration (`workerCount = 10 000`)
  └─ orchestrateur racine
      ├─ 10 000 workers réels
      │   ├─ workers distribués par hash `(run_id, worker_id)`
      │   └─ 64 à 128 shards de scheduling, invisibles pour le contrat produit
      ├─ reviewers/testeurs réutilisables
      ├─ file globale priorisée
      ├─ gouverneur de capacité et de quotas
      ├─ pool d'exécuteurs API + compatibilité CLI
      ├─ pool borné de workspaces chauds
      └─ intégrateur Git par zone de code
```

Le sharding est un détail interne destiné à supprimer les verrous globaux. Il ne change pas le nombre demandé par l'utilisateur et ne crée pas dix runs indépendants. Les API de création, contrôle, progression et suppression continuent de manipuler une seule orchestration pouvant contenir 10 000 workers.

## 4. État et coordination : aucun fichier partagé en écriture

Un fichier dans lequel les 10 000 workers ajoutent librement ce qu'ils ont fait deviendrait un verrou global, créerait des conflits et serait difficile à reprendre après une panne.

Le remplacement proposé est :

- un journal d'événements append-only avec numéro de séquence et clé d'idempotence ;
- une projection d'état consultable par run, shard, worker et tâche ;
- un répertoire d'artefacts propre à chaque worker, par exemple `artifacts/<run>/<worker>/<turn>/` ;
- un rapport structuré par tour contenant objectif, fichiers lus, fichiers proposés, patch, commandes, résultats, risques et dépendances ;
- des messages de groupe stockés comme événements, pas dans un Markdown modifié par tous ;
- une synthèse lisible générée à la demande pour l'utilisateur.

Schéma minimal : `runs`, `worker_shards`, `workers`, `tasks`, `turns`, `leases`, `events`, `artifacts`, `workspace_slots`, `integration_queue` et `provider_budgets`.

Pour la machine unique, le premier choix est **SQLite en mode WAL**, avec transactions courtes, index ciblés, écritures groupées et snapshots périodiques. Cela évite d'ajouter immédiatement PostgreSQL, Redis et NATS. Une interface `OrchestrationStore` devra toutefois isoler le stockage afin de pouvoir passer plus tard à PostgreSQL si Switch devient multi-nœuds.

### Buffer d'écriture en RAM devant le SSD

L'état chaud doit effectivement vivre en RAM avant d'être écrit sur le T7. Le SSD ne doit jamais recevoir une petite écriture par token, message ou changement de statut de chacun des 10 000 workers.

```text
workers/API
   → file RAM bornée
   → agrégation et coalescence
   → transaction groupée
   → SQLite WAL sur SSD
   → checkpoint différé vers la base principale
```

Le composant cible est un `StoreWriter` unique, exécuté sur un thread bloquant dédié afin de ne jamais bloquer les tâches Tokio. Les producteurs lui envoient des événements par une `mpsc` bornée. Le writer regroupe ce qui arrive pendant 5 à 20 ms ou jusqu'à environ 4 Mio, puis effectue une seule transaction et un seul flush durable pour tout le lot. Les mises à jour répétées d'un même compteur ou d'une même progression sont fusionnées avant écriture.

Trois classes d'écritures sont nécessaires :

- **critiques** : création du worker, lease, début/fin de tour, résultat final, hash du patch et décision de merge. Elles sont acquittées seulement après commit du WAL ; une requête API ne démarre pas avant que sa lease soit durable ;
- **progression** : tokens streamés, pourcentage, activité et métriques. Elles restent en RAM, sont coalescées et peuvent être écrites toutes les 100 à 250 ms ; une panne peut perdre les derniers indicateurs visuels, jamais le résultat du worker ;
- **artefacts** : sorties et patches volumineux. Ils sont compressés et écrits en blocs séquentiels de plusieurs Mio, puis leur hash et leur chemin sont commités dans SQLite.

Avec 384 Go de RAM, commencer avec un buffer borné de **8 Gio**, extensible après mesure jusqu'à 16 Gio. Il ne doit jamais être illimité. La pression de cette file entre dans le gouverneur : à 75 % il réduit les nouveaux départs, à 90 % il arrête temporairement de démarrer des tours, puis continue de drainer et persister. Si le T7 se fige, Switch ralentit donc les workers au lieu de saturer la RAM ou de perdre l'état.

SQLite doit démarrer en WAL avec les transactions critiques réellement durables. Le coût des synchronisations est amorti par le group commit. Les checkpoints sont déclenchés pendant une période calme ou lorsque le WAL atteint un seuil mesuré, pas sur chaque transaction. Le temps de commit, la profondeur de file, les octets en attente, le débit SSD et la durée des checkpoints doivent être exposés dans les métriques.

La RAM seule n'est jamais la source de vérité : après une coupure électrique, SQLite rejoue son WAL et Switch recrée les projections chaudes. Seules les progressions visuelles explicitement non critiques peuvent disparaître. Si le futur serveur possède un SSD/NVMe interne fiable, la variante la plus rapide est d'y placer la base active et son WAL, puis de sauvegarder vers le T7 ; déplacer ainsi les données hors du T7 nécessitera toutefois une décision explicite distincte pour l'environnement de développement actuel.

Migration depuis le store v10 actuel :

1. importer `orchestrated-runs.json` une seule fois dans une base temporaire ;
2. vérifier comptages, identifiants et hashes ;
3. renommer atomiquement la base validée ;
4. conserver un export JSON de récupération en lecture seule ;
5. interdire la double écriture permanente JSON + SQLite, trop lente et difficile à rendre cohérente.

## 5. Scheduler réellement concurrent

Le refactoring central est de remplacer les champs uniques d'un run par des collections :

- `active_starts: Map<AgentTurnKey, StartLease>` ;
- `active_turns: Map<AgentTurnKey, TurnLease>` ;
- `active_validations: Map<ValidationKey, ValidationLease>` ;
- `ready_queue` globale et files équitables par run/provider ;
- sémaphore global, sémaphore par run, sémaphore par provider et limite par compte.

Le champ `max_concurrency` deviendra alors un vrai sémaphore par run. La capacité globale restera l'autorité supérieure.

Les boucles qui scannent tout l'état toutes les 250 ms seront remplacées par une file réveillée par événement (`tokio::mpsc`/`Notify`) et un timer seulement pour les retries planifiés. Les drivers OS dédiés disparaîtront progressivement au profit de tâches Tokio légères.

### Modèle asynchrone Rust/Tokio

Le chemin réseau doit être asynchrone de bout en bout : Axum reçoit la commande, le scheduler envoie un message au shard propriétaire, puis `reqwest` streame la réponse API sans monopoliser un thread. Dix mille appels actifs correspondent à dix mille tâches Tokio légères, pas à dix mille threads système.

Le scheduler est divisé en 64 shards au premier benchmark, extensibles à 128. Le propriétaire est choisi par le hash de `(run_id, worker_id)`, afin qu'un run unique de 10 000 workers utilise tous les shards. Chaque shard possède sa file `mpsc` bornée et ses workers en mémoire ; il modifie son état séquentiellement et publie des événements, ce qui supprime le `Mutex<OrchestrationStore>` global du chemin chaud.

Primitives recommandées :

- `tokio::mpsc` borné pour les commandes de shards et le `StoreWriter` ;
- `tokio::Semaphore` pour les limites globales, par run, provider, compte et type de ressource ;
- `tokio::select!` pour réponse, timeout, annulation et arrêt propre ;
- `CancellationToken` ou équivalent pour propager l'annulation d'un run ;
- `JoinSet` pour suivre les appels actifs et garantir leur collecte ;
- `watch` pour les agrégats et `broadcast` seulement pour les notifications éphémères ;
- identifiants de tentative et clés d'idempotence pour ignorer les événements tardifs d'un retry annulé.

Les opérations bloquantes sont sorties du runtime async :

- SQLite est piloté par le thread `StoreWriter` dédié ;
- `git`, création/nettoyage de worktree et outils CLI passent dans un pool bloquant borné ;
- builds, tests et compression CPU utilisent un pool distinct, dimensionné sur les cœurs physiques ;
- aucune fonction bloquante ne doit rester sous un verrou ou dans une tâche réseau Tokio.

### Redis : pas sur le chemin critique du serveur unique

Pour le serveur bi-Xeon unique, Redis n'est pas nécessaire dans la première architecture. Les canaux Tokio et les shards en mémoire sont plus rapides, évitent un aller-retour réseau et suppriment un service supplémentaire à sauvegarder, surveiller et reprendre. SQLite WAL reste la vérité durable ; le buffer RAM et les shards sont reconstruits par replay au démarrage.

Redis ne doit pas contenir les prompts complets, réponses, patches ou l'état durable des workers. Il devient pertinent seulement si Switch utilise plusieurs processus `cst-server` actifs ou plusieurs machines. Dans cette phase distribuée :

- Redis Streams peut transporter la ready queue et les événements inter-nœuds ;
- des leases atomiques avec TTL peuvent attribuer un worker à un nœud ;
- un limiteur partagé peut coordonner quotas et concurrence par compte ;
- Pub/Sub ne sert qu'aux notifications perdables, jamais aux commandes durables ;
- PostgreSQL devient alors la source de vérité durable, Redis restant une couche de coordination/cache.

Le passage à Redis/PostgreSQL doit se faire derrière `OrchestrationStore` et `SchedulerBus`, sans changer le contrat `workerCount=10000`. Il n'apporte rien au premier déploiement mono-serveur tant que les mesures ne montrent pas que le processus Rust unique est le goulot.

Règles du gouverneur :

- un seul tour à la fois par session de worker, mais jusqu'à 10 000 sessions différentes en vol ;
- équité entre orchestrations pour qu'un run de 10 000 workers n'affame pas les chats ordinaires ;
- jetons distincts pour appels API, processus CLI, validations CPU et opérations Git ;
- réduction immédiate de la concurrence sur `429`, surcharge provider, hausse du p95 ou pression mémoire ;
- remontée additive prudente lorsque la file existe et que les métriques sont saines ;
- retry avec backoff exponentiel, jitter et clé d'idempotence ;
- circuit breaker par compte/modèle/provider ;
- réservation atomique avant lancement et libération garantie après succès, erreur, annulation ou crash.

## 6. Exécution API rapide sans casser les chats existants

Créer une interface Rust `AgentExecutor` :

- `CliExecutor` encapsule le chemin actuel `ChatTurnManager`/Codex CLI et reste utilisé pour les chats interactifs et les providers sans API native adaptée ;
- `ResponsesApiExecutor` utilise le client HTTP déjà présent (`reqwest`) pour les workers orchestrés configurés avec une clé API ;
- d'autres adapters gardent la même enveloppe de résultat structurée.

Le transport API doit partager un petit nombre de `reqwest::Client`, réutiliser les connexions HTTP/2, streamer les réponses, limiter la taille en mémoire et écrire les événements par lots. Il ne faut ni lancer Node ni créer un processus `codex.exe` pour chaque agent API.

Le choix de modèle doit rester piloté par le profil de compte de Switch. Une politique de coût/vitesse pourra envoyer la majorité des sous-tâches bornées vers le modèle rapide configuré, réserver le modèle le plus puissant à la planification, aux conflits et à la revue finale, puis réessayer sur un modèle de secours en cas de capacité. Les quotas réels du compte restent la limite : le scheduler les mesure, il ne les suppose pas.

## 7. Travail simultané sur le même code

Les 10 000 workers partagent le même commit de base, pas le même dossier modifiable.

Le moteur actuel de worktrees est conservé mais rendu paresseux :

- les 10 000 workers et leurs sessions existent bien dans l'orchestration dès sa création ;
- un worker qui effectue seulement un appel API de lecture/raisonnement n'a pas besoin d'un worktree ;
- un pool borné de 64 à 128 workspaces chauds est créé sur le stockage le plus rapide pour les workers qui doivent écrire ou tester ;
- un worker qui doit modifier le code obtient une lease sur un slot, vérifie le commit de base, travaille et produit un patch/commit ;
- le slot est nettoyé, vérifié puis réutilisé ;
- les artefacts durables et la base restent sur le stockage de développement autorisé ;
- sur le futur serveur Linux, un `tmpfs` peut héberger uniquement les workspaces jetables, jamais l'unique copie des résultats.

Avant de lancer deux tâches d'écriture, le planificateur calcule leurs zones de fichiers. Plusieurs workers peuvent lire les mêmes fichiers, mais les écritures qui se chevauchent passent par une lease de propriété ou par une branche de conflit explicitement revue. Les patches acceptés entrent dans une file d'intégration ordonnée ; l'intégrateur Git applique, teste et avance le commit intégré. Les workers suivants se rebasent sur ce commit au prochain tour.

Le protocole de preuve actuel reste obligatoire. Il faut ajouter au rapport :

- `base_commit` ;
- fichiers effectivement modifiés ;
- hash du patch ;
- commandes exécutées et codes de sortie ;
- dépendances sur d'autres tâches ;
- statut `ready`, `blocked`, `conflict`, `submitted` ou `accepted`.

### Invariants anti-perte de travail

La garantie cible est : **aucun patch acquitté ne peut être supprimé, remplacé ou oublié silencieusement**. Un conflit peut bloquer une intégration, mais les deux versions restent récupérables.

Invariants obligatoires :

1. aucun worker n'écrit directement dans la branche source ni dans le worktree d'un autre worker ;
2. un résultat n'obtient le statut `submitted` qu'après stockage durable du patch binaire, du rapport et de leurs hashes ;
3. un workspace ne peut pas être recyclé avant cette confirmation durable ;
4. un patch accepté reste immuable et adressé par `SHA-256` ; un retry crée une nouvelle tentative au lieu d'écraser l'ancienne ;
5. la référence Git intégrée avance uniquement par compare-and-swap sur l'ancien commit attendu ;
6. toute suppression ou modification hors du périmètre autorisé est rejetée par défaut ;
7. aucun nettoyage de patch, commit candidat ou artefact n'a lieu avant validation finale, sauvegarde et expiration d'une rétention configurée ;
8. chaque worker accepté apparaît exactement une fois dans le registre d'intégration final.

### Transaction de merge

Pour chaque worker :

1. figer `base_commit`, `allowed_paths`, hashes des blobs de base et autorisation éventuelle de suppression ;
2. produire un commit candidat et un `git diff --binary --full-index`, puis calculer `patch_sha256` ;
3. persister patch, rapport, inventaire des fichiers et événement `submitted` avant de libérer le worktree ;
4. placer le candidat dans une file d'intégration durable ;
5. acquérir une lease sur les zones de code concernées ;
6. créer un worktree candidat depuis le `integrated_head` courant, jamais depuis le dossier source ;
7. comparer les fichiers touchés depuis `base_commit`. Tout chevauchement avec un patch déjà intégré déclenche une nouvelle revue, même si Git sait faire un auto-merge ;
8. appliquer en trois voies dans le candidat. Un conflit place la tâche en `conflict` et conserve toutes les versions ; aucune option de résolution automatique ne doit choisir « ours » ou « theirs » globalement ;
9. vérifier l'inventaire avant/après, les suppressions, renommages, sous-modules, liens et chemins sortant du périmètre ;
10. exécuter les tests ciblés, puis la revue du résultat combiné ;
11. créer le commit intégré avec trailers `Switch-Worker-Id`, `Switch-Attempt-Id`, `Switch-Patch-SHA256` et `Switch-Base-Commit` ;
12. enregistrer une transaction `merge_pending`, puis avancer la ref avec `git update-ref <ref> <new> <old>` ;
13. si le compare-and-swap échoue parce qu'un autre lot a avancé la ref, ne rien supprimer : replacer le candidat sur le nouveau HEAD et recommencer ;
14. marquer `merge_committed` après succès. Au redémarrage, réconcilier les transactions `merge_pending` avec la ref et les trailers Git afin de couvrir un crash entre les deux écritures.

La préparation et les tests de patches qui touchent des zones disjointes peuvent se faire en parallèle. La publication sur la ref canonique reste atomique et ordonnée. Des lots de patches non chevauchants peuvent être publiés ensemble pour éviter 10 000 synchronisations, tout en gardant dans le registre la correspondance exacte entre chaque worker, son hash et le commit résultant.

### Équipe d'agents spécialisée dans les conflits

Oui, les conflits sémantiques doivent être confiés à des agents dédiés, mais aucun de ces agents ne reçoit le droit d'avancer la branche intégrée. Ils proposent et vérifient une résolution dans un worktree candidat ; seul l'intégrateur transactionnel peut publier après quorum.

Chaque collision crée un `ConflictCase` durable contenant :

- commit de base commun, `integrated_head` courant et commits/patches candidats ;
- hunks conflictuels et fichiers liés ;
- objectifs et rapports structurés de tous les workers concernés ;
- comportements, tests et invariants introduits par chaque côté ;
- suppressions, renommages et changements d'API détectés ;
- niveau de risque, tentatives de résolution et décisions signées par rôle.

Le traitement se fait dans cet ordre :

1. un moteur déterministe utilise Git, les hashes de blobs, l'analyse des hunks, le compilateur et les tests pour identifier exactement le conflit ;
2. un **agent de triage** construit la liste des intentions à préserver et classe le risque ;
3. un **agent résolveur A** produit un patch combiné et une matrice expliquant, intention par intention, où chaque travail est conservé ;
4. pour un conflit important, un **agent résolveur B**, isolé de la proposition A, produit une solution indépendante ;
5. un **agent arbitre** compare les propositions avec le code de base et choisit ou reconstruit la solution ;
6. un **agent vérificateur de préservation** confirme qu'aucun comportement, test ou hunk utile n'a disparu sans justification ;
7. un **agent testeur** exécute les tests des deux côtés, ajoute un test de régression propre au conflit et valide le résultat combiné ;
8. l'intégrateur applique ensuite le protocole `merge_pending` + compare-and-swap. Il refuse de publier si le quorum ou une preuve manque.

La matrice de préservation donne à chaque intention un identifiant et l'un des statuts `preserved`, `adapted`, `superseded` ou `rejected`. `superseded` et `rejected` exigent une justification explicite et l'accord de l'arbitre ; pour un fichier critique ou une suppression, ils exigent aussi une décision humaine. Ainsi, un agent ne peut pas résoudre un conflit simplement en choisissant la version la plus facile.

Niveaux de traitement :

- **niveau 0 — mécanique sans chevauchement** : Git applique, puis tests et vérification d'inventaire ;
- **niveau 1 — imports, formatage ou code généré** : un résolveur, un vérificateur et tests ;
- **niveau 2 — même symbole, même API, même schéma ou comportement concurrent** : deux résolveurs indépendants, un arbitre, un vérificateur et un testeur ;
- **niveau 3 — suppression, authentification, données, migration, sécurité, `AGENTS.md`, CI ou scripts de release** : même équipe que le niveau 2, plus validation humaine obligatoire.

Le pool initial recommandé est de 16 résolveurs, 4 arbitres et 8 vérificateurs/testeurs, avec montée adaptative selon la file de conflits et les quotas API. Des conflits portant sur des zones différentes avancent en parallèle ; un même `ConflictCase` conserve une lease unique. Après trois tentatives invalides, le cas passe en `needs_attention` au lieu de boucler ou de forcer une fusion.

Les agents reçoivent uniquement le paquet de conflit et le worktree candidat. Ils ne peuvent ni supprimer les patches sources, ni nettoyer les workspaces d'origine, ni appeler `git update-ref`, ni toucher au projet source. Toutes leurs propositions restent hashées et auditables, y compris celles qui ne sont pas retenues.

### Protection spécifique contre les suppressions

- `allow_deletions` vaut `false` par défaut dans chaque tâche ;
- une suppression doit être annoncée dans le rapport et appartenir aux chemins autorisés ;
- supprimer un fichier modifié depuis `base_commit` constitue toujours un conflit ;
- les fichiers critiques (`AGENTS.md`, manifests, lockfiles, CI, scripts de release et configuration de déploiement) nécessitent une revue renforcée ;
- un seuil de suppression inhabituel bloque le lot et demande une décision humaine ;
- le manifeste `git ls-tree` avant/après et le résumé `git diff --name-status` sont conservés avec la preuve ;
- la publication finale vérifie que chaque patch accepté figure dans le registre et qu'aucun fichier n'a disparu sans autorisation associée.

Le projet source n'est modifié qu'après cette intégration complète, les tests finaux et le contrôle actuel qui exige que son HEAD et son état soient restés inchangés. Si le projet source a bougé, la publication s'arrête ; elle ne force jamais un reset et ne remplace jamais les changements présents.

### Protection contre la panne physique du SSD

Une seule copie sur le T7 ne peut pas fournir de garantie absolue : une panne matérielle pourrait perdre simultanément la base, le WAL et les patches. Pour qu'un travail accepté survive aussi à la perte du SSD, chaque patch et rapport doit avoir deux copies vérifiées sur deux supports indépendants.

- la copie primaire reste dans le store d'artefacts adressé par hash ;
- un réplicateur asynchrone copie le blob vers un second SSD/NVMe ou un stockage objet distant ;
- il relit la copie et vérifie `SHA-256` avant de marquer `replicated` ;
- le worktree peut être réutilisé après persistance locale, mais le blob local n'est jamais éligible au garbage collection avant `replicated` ;
- si la destination secondaire est indisponible, les nouveaux merges peuvent continuer dans une limite d'espace, mais le nettoyage durable s'arrête et une alerte est levée ;
- les snapshots SQLite et le registre d'intégration sont eux aussi copiés et testés par une restauration périodique.

Sur le serveur final, le placement recommandé est base/WAL actifs sur le disque interne le plus rapide et archive vérifiée sur le T7, ou l'inverse si les règles de l'environnement imposent que la copie primaire reste sur le T7. Ce déplacement ne doit pas être appliqué au runtime actuel sans décision explicite.

## 8. Tests et qualité à grande échelle

Le ratio actuel d'un testeur persistant pour cinq workers est conservé comme règle de couverture, mais les testeurs deviennent un **pool réutilisable par lots**. Dix mille workers ne doivent pas créer deux mille sessions de test permanentes.

Organisation cible :

- review locale de chaque patch par le chef de groupe ;
- testeurs mutualisés sur des lots de cinq tâches prêtes ;
- tests affectés selon les fichiers touchés ;
- validations CPU lourdes dans une file séparée ;
- tests d'intégration à chaque lot fusionné ;
- revue finale et suite complète sur le commit intégré ;
- publication toujours soumise aux contrôles actuels de HEAD, état Git et quorum.

Les commandes de test ne doivent jamais être lancées 10 000 fois si 500 tâches touchent le même composant. Un cache de résultat indexé par `commit + commande + empreinte environnement` et une fusion des demandes identiques réduiront fortement le temps total.

## 9. API, WebSocket et interface pour 10 000 workers

Les snapshots complets actuels doivent rester disponibles pour la compatibilité sur les petits runs, mais les grands runs utiliseront :

- listes paginées par curseur ;
- vues résumées (`counts`, progression, erreurs, coût, débit) ;
- récupération détaillée d'un shard ou d'un worker à la demande ;
- événements WebSocket différentiels avec `sequence` et reprise depuis `after_sequence` ;
- compression des lots d'événements ;
- virtualisation des listes dans le frontend ;
- recherche et filtres côté serveur ;
- agrégation par groupe par défaut, sans créer 10 000 composants DOM.

L'UI doit proposer directement un champ **Workers** de 1 à 10 000 et distinguer : `10 000 workers créés`, `N prêts`, `N actifs`, `N limités par quota`, `N terminés` et `N en erreur`. Un second champ **Concurrence maximale** accepte lui aussi 1 à 10 000. Le gouverneur peut descendre temporairement sous la demande pour protéger le débit, mais remonte vers la valeur demandée dès que les ressources et quotas le permettent.

## 10. Profil initial pour le serveur bi-Xeon / 384 Go

Le modèle exact des deux Xeon 2697 et la topologie NUMA doivent être relevés par `lscpu`, car les variantes n'ont pas le même nombre de cœurs. Le profil initial suivant est un point de départ à mesurer, pas une vérité fixe :

| Ressource | Départ prudent | Ajustement automatique |
|---|---:|---|
| Workers par orchestration | 10 000 | plafond produit fixe |
| Tours CLI actifs | 64 | 16 à 128 selon RSS et stabilité |
| Requêtes API actives | 256 au premier essai | montée par paliers jusqu'à 10 000 |
| Workspaces chauds | 96 | 32 à 128 selon I/O et espace |
| Validations CPU lourdes | nombre de cœurs physiques / 2 | selon load average et durée |
| Threads Tokio | 16 | benchmark 8, 16, 24 et 32 |
| Shards scheduler | 64 | benchmark 32, 64 et 128 |
| Buffer RAM d'écriture | 8 Gio | 4 à 16 Gio selon débit et pointes |
| Écritures SQLite | un writer, lots de 5 à 20 ms ou 4 Mio | adapter selon latence SSD et profondeur de file |

Le plafond logiciel doit autoriser 10 000 requêtes API de workers en vol. Cette capacité sera prouvée d'abord avec un provider simulé local, puis avec l'API réelle par paliers selon le quota accordé. La machine ne lancera pas 10 000 processus : un petit nombre de tâches Tokio et de connexions réutilisées multiplexera les flux HTTP.

Pour le serveur Linux de développement :

- réutiliser le `Dockerfile.server` et la composition `cst`/`social`/`social-gateway` existante ;
- conserver `cst-server` headless, sans Kubernetes sur un seul nœud ;
- monter le code et les données de manière explicite, sans toucher au VPS Azure ;
- utiliser un volume SSD/ext4 pour SQLite et les artefacts ;
- réserver un `tmpfs` borné aux sandboxes jetables ;
- laisser au moins 32 Go à l'OS, aux caches et aux pointes ;
- ne pas épingler aveuglément tous les threads sur un socket : tester l'entrelacement NUMA pour les tâches réseau et l'affinité par worker pour les validations ;
- séparer les limites mémoire du serveur, des CLIs et des validateurs afin qu'une vague de builds ne tue pas l'orchestrateur.

## 11. Étapes d'implémentation

### Étape 0 — baseline fiable

1. Résoudre l'écart d'asset entre le runtime local et la passerelle `:10000`.
2. Ajouter un endpoint de diagnostic de release sans secret.
3. Capturer les métriques actuelles avec 1 puis 5 workers.
4. Relever CPU exact, NUMA, disque, réseau, mémoire moyenne d'un tour CLI et débit API autorisé.
5. Ne changer aucune limite avant d'avoir ces chiffres.

### Étape 1 — stockage transactionnel compatible

1. Introduire `OrchestrationStore` derrière l'API actuelle.
2. Ajouter SQLite WAL et la migration du store v10.
3. Ajouter le `StoreWriter`, sa file RAM bornée et le group commit.
4. Séparer écritures critiques, progressions coalescées et artefacts volumineux.
5. Relier la profondeur de la file au backpressure du scheduler.
6. Écrire des événements idempotents et des projections paginées.
7. Garder les réponses existantes pour les runs de petite taille.
8. Tester reprise après crash au milieu d'une réservation, d'un turn et d'une fusion.

### Étape 2 — vraie concurrence dans un run

1. Remplacer les champs `current_*` uniques par des maps de leases.
2. Implémenter le sémaphore `run.max_concurrency`.
3. Introduire 64 shards propriétaires avec files `mpsc` bornées.
4. Remplacer le scan 250 ms par une ready queue événementielle.
5. Sortir SQLite, Git et validations CPU du runtime réseau Tokio.
6. Ajouter équité, backpressure, annulation et récupération de lease expirée.
7. Valider d'abord 5, 16, 32 puis 64 workers actifs sans modifier encore le plafond public.

### Étape 3 — executor API natif

1. Extraire `AgentExecutor` sans casser `ChatTurnManager`.
2. Ajouter le transport API streamé et les budgets par compte/modèle.
3. Normaliser les sorties dans le protocole de preuve existant.
4. Ajouter retries idempotents, circuit breaker et télémétrie coût/tokens.
5. Comparer débit et mémoire du CLI contre l'API à qualité égale.

### Étape 4 — pool de workspaces et intégration

1. Matérialiser les worktrees à la demande.
2. Ajouter leases de fichiers/zones et détection de chevauchement.
3. Produire des patches binaires hashés et durables avant de libérer un slot.
4. Ajouter le registre d'intégration et les transactions `merge_pending`/`merge_committed`.
5. Publier la ref intégrée par compare-and-swap avec `git update-ref`.
6. Ajouter `ConflictCase`, la matrice de préservation et les rôles résolveur/arbitre/vérificateur/testeur.
7. Bloquer par défaut suppressions, chemins hors périmètre et chevauchements non revus.
8. Tester la récupération après crash à chaque frontière de la transaction de merge.
9. Sérialiser la publication par zone, préparer les lots disjoints en parallèle et revalider le commit de base.
10. Mutualiser les validations identiques.

### Étape 5 — relever réellement `workerCount` à 10 000

1. Passer `MAX_WORKER_COUNT` de `1_000` à `10_000` dans `src-tauri/src/orchestration.rs`.
2. Faire suivre `MAX_TASK_COUNT` et `MAX_MAX_CONCURRENCY` jusqu'à `10_000`.
3. Autoriser `CST_ORCHESTRATION_MAX_WORKERS=10000` et `CST_ORCHESTRATION_CONCURRENCY=10000` dans les parseurs de configuration.
4. Passer `MAX_ORCHESTRATION_WORKER_COUNT` de `5` à `10_000` dans `src/chat/orchestration.ts`.
5. Conserver le nombre proposé par défaut à 5, mais accepter explicitement toute valeur entière entre 1 et 10 000.
6. Mettre à jour les DTO, formulaires, messages, tests frontend et tests Rust qui encodent aujourd'hui 5 ou 1 000.
7. Sharder en interne les collections et la ready queue, tout en conservant un seul run et une seule commande de contrôle.
8. Virtualiser et paginer l'interface, avec agrégats avant détails.
9. Monter par paliers : 200, 1 000, 5 000 puis 10 000 workers dans le même run.
10. Prouver avec un provider HTTP simulé que `maxConcurrency=10000` maintient 10 000 tours de workers simultanément en vol sans perte, duplication ou blocage global.

### Étape 6 — profil serveur et endurance

1. Déployer uniquement sur le serveur de développement explicitement désigné.
2. Calibrer automatiquement API, CLI, workspaces et validations.
3. Effectuer un soak test de 24 heures avec pannes injectées.
4. Vérifier récupération après restart, perte réseau, `429`, disque plein et conflit Git.
5. Figer le profil seulement après résultats reproductibles.

## 12. Fichiers actuels concernés lors de l'implémentation

- `src-tauri/src/orchestration.rs` : état, scheduler, worktrees, intégration et validations ;
- `src-tauri/src/chat.rs` : admission, sessions et exécuteur CLI ;
- `src-tauri/src/server.rs` : API paginée, diagnostics et contrôle ;
- `src-tauri/src/runtime_sync.rs` : événements différentiels et reprise ;
- `src-tauri/Cargo.toml` : stockage SQLite et éventuelles primitives supplémentaires ;
- `src/chat/orchestration.ts` : modèles de données, pagination et plafond `workerCount = 10_000` ;
- `src/main.ts` : écran agrégé, virtualisation et contrôles ;
- `compose.yaml` et `Dockerfile.server` : profil du serveur de développement ;
- `tests/orchestration-chat.test.mjs` : limite frontend actuelle et nouveaux contrats ;
- nouveaux tests Rust : concurrence dans un run, leases, migration, reprise et idempotence.

Il est préférable de scinder `orchestration.rs` en modules ciblés (`store`, `scheduler`, `executor`, `workspace`, `integration`, `protocol`) après la mise en place de tests de caractérisation, afin d'éviter une réécriture risquée du comportement actuel.

## 13. Critères d'acceptation

Le travail n'est pas terminé lorsque l'UI accepte la valeur 10 000. Il est terminé lorsque :

- `POST /api/orchestrations` accepte dans un même run `workerCount=10000`, `maxTaskCount=10000` et `maxConcurrency=10000` ;
- ce run contient bien 10 000 workers distincts, paginables et restaurables après restart ;
- un provider simulé maintient 10 000 appels de workers simultanément en vol pendant au moins 60 secondes ;
- avec le provider réel, Switch monte jusqu'au plafond de concurrence effectivement accordé au compte et affiche clairement la part limitée par quota ;
- aucun tour n'est perdu ou exécuté deux fois après crash ;
- plusieurs workers d'un même run avancent réellement en parallèle ;
- les chats ordinaires restent utilisables pendant une grande campagne ;
- le frontend n'affiche ni freeze ni payload complet de 10 000 workers ;
- aucun worker ne modifie un fichier de journal partagé ;
- les conflits de code passent par les leases et la revue ;
- chaque worker accepté possède un `patch_sha256` récupérable et une entrée unique dans le registre d'intégration ;
- une suppression non déclarée ou hors périmètre est systématiquement rejetée ;
- un conflit de niveau 2 ne peut être publié sans deux propositions indépendantes, arbitrage, matrice de préservation et test de régression ;
- un conflit de niveau 3 reste bloqué jusqu'à la validation humaine ;
- chaque intention des patches en conflit possède un statut explicite et aucune proposition d'agent ne peut avancer directement la ref Git ;
- un échec de compare-and-swap replace le patch dans la file sans perdre son artefact ;
- des crashes injectés avant/après `merge_pending`, `git update-ref` et `merge_committed` restaurent exactement le même `integrated_head` et le même registre ;
- la perte simulée du disque primaire permet de reconstruire chaque patch accepté depuis la copie secondaire vérifiée ;
- le nombre de workspaces reste borné ;
- une pause simulée du SSD pendant 30 secondes déclenche la backpressure sans OOM, sans perte d'écriture critique et sans blocage du serveur ;
- après un crash forcé, le WAL restaure toutes les leases et fins de tours acquittées ; seules des progressions visuelles non critiques peuvent manquer ;
- le débit se stabilise au maximum autorisé par le provider sans tempête de `429` ;
- les preuves, reviews, tests, vérifications Git et release gates actuels restent obligatoires ;
- une campagne peut reprendre après un arrêt propre ou brutal sans perdre ses artefacts.

Objectifs de performance à valider sur le matériel final : dispatch p95 inférieur à 50 ms quand une place est disponible, page résumée p95 inférieure à 250 ms, premier affichage UI inférieur à 2 s sur un run de 10 000 workers, et absence de croissance mémoire non bornée pendant 24 heures.

## 14. Ordre recommandé

L'ordre optimal est : **baseline/routage → SQLite WAL → concurrence réelle par run → executor API → workspaces chauds → relèvement des limites à 10 000 → UI virtualisée → calibration serveur**.

La valeur finale sera bien `10 000 workers`. Le relèvement des constantes doit simplement arriver après le store et le scheduler, afin que cette valeur déclenche 10 000 workers utilisables plutôt qu'un blocage du JSON global et des milliers de worktrees inutiles.
