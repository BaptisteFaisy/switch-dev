# Autorisation absolue obligatoire pour la production Switch

- Ne jamais accéder à Switch sur le VPS Microsoft Azure, l’inspecter, le modifier, le déployer, le redémarrer ni effectuer aucune autre action dessus sans une autorisation explicite de l’utilisateur dans la demande en cours pour l’action de production exacte.
- Ne jamais déduire cette autorisation du contexte, des mots `stable`, `production` ou `prod`, d’une demande précédente ou d’une autorisation donnée pour une autre action. Une autorisation ne se reporte jamais sur une demande ultérieure.
- Si la demande n’autorise pas explicitement l’action exacte sur Switch sur le VPS, ne pas toucher à la production et demander d’abord à l’utilisateur.

# Connexion au VPS Microsoft Azure

La production « Switch » (serveur `cst-server`) tourne sur une VM Microsoft
Azure nommée **duello-prod-vm**, identifiée aussi par le tailnet Tailscale
`azure-duello` :

- IP Tailscale : `100.103.224.119` (chemin utilisé par le PC fixe) ;
- IP publique : `172.182.240.65` ;
- nom d'hôte attendu sur la VM : `duello-prod-vm`.

Rappel : cette section décrit comment se connecter, elle ne vaut pas
autorisation. Toute action sur Switch (inspection, modification, déploiement,
redémarrage…) reste soumise à la règle d'autorisation absolue en tête de
fichier. Le guide complet de connexion et de diagnostic (profil par profil,
dépannage tunnel/SSH) est dans `docs/VPS_AZURE.md`.

## Accès SSH depuis le PC fixe

```sh
# Via Tailscale (recommandé) — profil VPS `azure`
ssh root@100.103.224.119

# Via le tailnet nommé — profil VPS `azure-duello`
ssh duelloedit@azure-duello.tail3a8bdf.ts.net -i ~/.ssh/id_ed25519

# Via l'alias du ~/.ssh/config (compte Azure par défaut, IP publique)
ssh duello-prod-vm        # → azureuser@172.182.240.65
```

Les profils VPS du client desktop sont dans
`%APPDATA%\codex-switch-terminal\vps\*.json` (`azure.json`,
`azure-duello.json`). Le conteneur Switch sur l'hôte s'appelle
`codex-switch-terminal-azure` ; on y entre avec
`sudo -n docker exec -it codex-switch-terminal-azure /bin/bash`.

## Ouvrir l'application Switch

Tunnel SSH brut vers l'interface web (`cst-server` sur `127.0.0.1:8080` de la
VM) :

```sh
ssh -N -L 127.0.0.1:8081:127.0.0.1:8080 root@100.103.224.119
```

puis ouvrir `http://127.0.0.1:8081` dans le navigateur. Le client desktop
pointé sur le nœud distant s'ouvre avec
`npm run connect:vps -- -Profile azure` (ou `-CheckOnly` pour vérifier le
tunnel et l'authentification sans lancer le client) ; ce script exige un jeton
administrateur protégé dans le profil. L'interface web est aussi servie en
HTTPS sur le tailnet : `https://azure-duello.tail3a8bdf.ts.net`.

Depuis un terminal du conteneur, le retour vers le PC fixe se fait par
`ssh pc-fixe` (pont SSH reverse via Tailscale, voir
`docs/SSH_TAILSCALE_PC.md`).

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

# Règles de structure du code

- Chaque fichier de code ne doit pas contenir plus de **30 fonctions**.
- Une fonction ne doit pas dépasser **50 lignes** (en incluant sa signature et ses accolades).
- Un fichier doit rester sous **~500 lignes** ; au-delà, il doit être scindé en plusieurs fichiers ou modules.
- Ne pas contourner ces limites en chaînant de longues méthodes privées ou en empilant des fonctions : tout fichier devenant trop dense doit être décomposé.

## Frontend TypeScript

- Les mêmes limites s'appliquent au frontend : **30 fonctions / 50 lignes / ~500 lignes** par fichier TypeScript, vérifiées par la même détection AST (`scripts/code-structure-audit.mjs`).
- `src/main.ts` (~40 000 lignes) est un monolithe historique en cours de découpe en modules : la règle y est déjà violée et ne doit plus jamais s'aggraver. Aucune nouvelle fonction n'ajoute au monolithe — elle va dans un module existant (`src/*.ts`, `src/chat/*`, …) ou dans un nouveau module cohérent (un domaine, pas un fourre-tout).
- Un module ne doit pas devenir un dépotoir de fonctions : regrouper par domaine (terminal, chat, comptes, appareils, vues secondaires, état, …) et extraire les sous-blocs > 50 lignes en helpers, comme pour le backend Rust.
- Les vues secondaires chargées à la demande passent par les loaders paresseux (`src/lazy-modules-a.ts`, `src/lazy-modules-b.ts`, `src/lazy-modules-c.ts`) et leurs chunks de binding DOM (`src/ui-bindings-a.ts` … `src/ui-bindings-k.ts`) ; on n'ajoute jamais de `import()` direct d'une vue dans `main.ts` sans passer par ce mécanisme (récupération de chunk obsolète incluse).

Vérification possible avec : `node scripts/code-structure-audit.mjs`.

# Standards approfondis de qualité du code et de revue

Toute modification de code doit faire l'objet d'un audit de qualité approfondi de l'état courant de la branche. Il faut repenser la structure et l'implémentation afin d'améliorer significativement la qualité sans modifier le comportement attendu. L'audit doit être ambitieux et rigoureux : améliorer les abstractions, la modularité, la lisibilité et la concision, réduire le spaghetti code, et restructurer le code lorsque cela apporte une amélioration claire. Mesurer deux fois avant de modifier.

## Principes non négociables

- Être ambitieux sur la simplification structurelle : rechercher le « code judo », c'est-à-dire une réorganisation qui supprime des concepts, branches, helpers, modes ou couches entières plutôt que de simplement déplacer la complexité.
- Ne pas s'arrêter à « cela pourrait être plus propre ». Si une reformulation rend le code nettement plus simple et plus inévitable, la privilégier.
- Ne pas laisser une PR pousser un fichier au-delà de **500 lignes** sans justification structurelle exceptionnelle. Si le diff franchit ce seuil, demander explicitement si le fichier doit d'abord être décomposé.
- Traiter toute croissance spaghetti, tout embranchement ad hoc ou toute logique spéciale dispersée comme un problème de conception, pas comme une simple remarque stylistique.
- Préférer une abstraction dédiée, une machine à états, une politique, un modèle typé ou un module séparé aux conditions ajoutées au hasard dans des chemins existants.
- Préférer du code direct, sobre et maintenable aux mécanismes magiques, aux wrappers minces et aux couches d'indirection qui ne clarifient pas réellement le domaine.
- Remettre en question les `Option`/nullable, `any`, `unknown`, casts et formes d'objets ad hoc lorsque la frontière peut être explicitement typée.
- Réutiliser les helpers et utilitaires canoniques existants ; ne pas créer de doublon local ni déplacer une logique vers le mauvais package, service ou module.
- Séparer orchestration et logique métier. Identifier les séquences asynchrones inutilement sérialisées et paralléliser le travail indépendant quand cela simplifie le flux. Rendre les mises à jour liées atomiques lorsqu'un état partiellement appliqué serait difficile à raisonner.
- Ne pas contourner les limites de taille en empilant des fonctions privées longues ou en répartissant artificiellement la même complexité ; le découpage doit suivre des responsabilités cohérentes.

## Questions obligatoires pour chaque changement significatif

- Existe-t-il un « code judo » qui rendrait l'implémentation radicalement plus simple ?
- Peut-on supprimer des concepts, branches ou couches plutôt que seulement les centraliser ?
- Le changement améliore-t-il l'architecture locale ou augmente-t-il le couplage et l'état implicite ?
- Des conditions répétées signalent-elles un modèle, une politique ou une abstraction manquante ?
- La logique est-elle dans le bon fichier, la bonne couche et le bon propriétaire canonique ?
- Le changement introduit-il une option, un booléen, un cast, un wrapper ou une forme d'objet qui obscurcit l'invariant réel ?
- Le flux est-il plus séquentiel, moins atomique ou plus difficile à vérifier qu'auparavant sans raison valable ?
- Les tests passent-ils sans que la structure soit devenue moins modulaire ou moins lisible ?

## Points à signaler et à traiter agressivement

- Implémentation compliquée alors qu'une reformulation supprimerait des catégories entières de complexité.
- Refactor qui déplace la complexité sans réduire le nombre de concepts à maintenir mentalement.
- Fichier franchissant **500 lignes**, en particulier à cause de la modification en cours.
- Conditions ad hoc, cas particuliers ou vérifications de fonctionnalité injectés dans un flux partagé ou déjà chargé.
- Booléens de mode, paramètres optionnels, fallbacks silencieux ou mécanismes génériques qui masquent une invariance floue.
- Feature logic qui fuit dans les chemins communs, détails d'implémentation exposés par l'API ou logique placée dans la mauvaise couche.
- Wrapper, helper d'identité ou abstraction générique qui ajoute de l'indirection sans acheter de clarté.
- Casts, `any`, `unknown`, optionalité inutile, contrats faiblement typés ou objets construits ad hoc.
- Copie de logique, helper bespoke lorsqu'un utilitaire canonique existe déjà, ou orchestration séquentielle évitable.
- Mise à jour partielle laissant un état incohérent ou plus difficile à raisonner.
- Refactor techniquement fonctionnel mais qui rend le code environnant plus spaghetti, plus couplé ou moins scannable.

## Remèdes préférés

- Supprimer une couche d'indirection ou un mode entier lorsque le modèle peut être simplifié.
- Repenser l'état pour faire disparaître les conditions plutôt que les déplacer.
- Déplacer la responsabilité vers l'abstraction, le module ou le package qui possède déjà le concept.
- Transformer les chaînes de conditions en modèle typé ou dispatcher explicite.
- Extraire une fonction pure, un helper cohérent, un sous-composant ou un module focalisé.
- Fusionner les branches dupliquées dans un flux unique et lisible.
- Remplacer un contrat faible par une frontière explicitement typée.
- Paralléliser les opérations indépendantes lorsque cela rend aussi l'orchestration plus claire.
- Regrouper les mises à jour liées dans une opération atomique.
- Ne pas se contenter d'un renommage ou d'un déplacement si une simplification structurelle plus profonde est visible.

## Barème d'approbation

Ne jamais approuver uniquement parce que le comportement semble correct. L'approbation exige l'absence de régression structurelle claire, de complexité de branchement évitable, d'explosion de taille injustifiée, d'abstraction magique ou inutile, de duplication canonique, de fuite de couche et d'opportunité évidente de décomposition.

Sont présumés bloquants, sauf justification explicite de l'auteur :

- conserver une complexité incidente importante alors qu'une simplification plausible la supprimerait ;
- faire passer un fichier sous 500 lignes à plus de 500 lignes ;
- ajouter des branches spéciales qui rendent un flux existant plus enchevêtré ;
- résoudre un problème local en dispersant des contrôles de fonctionnalité dans du code partagé ;
- ajouter une abstraction, un wrapper ou un contrat fondé sur des casts sans bénéfice clair ;
- dupliquer un helper existant ou placer la logique dans une couche non canonique ;
- laisser une décomposition évidente non réalisée alors qu'elle améliorerait matériellement la maintenabilité.

Le ton des retours doit être direct, sérieux et exigeant sans être irrespectueux. Prioriser les régressions structurelles, les occasions de simplification radicale, la complexité spaghetti, les problèmes de frontières et de contrats, la taille des fichiers, la modularité, puis la lisibilité. Préférer quelques remarques à forte conviction à une liste de micro-nits.

# Application automatique des règles de structure

- Un garde-fou « ratchet » bloque toute régression de structure : `scripts/structure-guard.mjs` compare l'état courant à l'instantané committé `scripts/structure-guard.baseline.json` et échoue si un fichier nouveau passe en infraction ou si un fichier déjà en infraction s'aggrave (plus de fonctions, fonction plus longue, fichier plus long). Les fichiers qui se corrigent sont acceptés — la règle se resserre, jamais l'inverse.
- Le hook pré-commit (`.githooks/pre-commit`, actif via `core.hooksPath = .githooks`) exécute automatiquement le garde-fou en mode `--staged` : seuls les fichiers stagés sont audités, les modifications en cours non stagées n'interfèrent jamais.
- Commandes : `npm run guard:structure` (contrôle complet), `npm run guard:structure:update` (régénérer l'instantané après une correction volontaire — uniquement quand un fichier a été refactoré conformément aux règles).
- Ne jamais contourner avec `--no-verify` sauf nécessité explicite validée par l'utilisateur ; ne jamais régénérer l'instantané pour masquer une régression.
- L'audit détaillé par fichier reste disponible : `node scripts/code-structure-audit.mjs` (le garde-fou et le CLI partagent la même détection via `auditAllFiles()` — une seule source de vérité).

# Procédure obligatoire avant build et publication

- Avant chaque build, analyser l’état des modifications et le diff afin de distinguer les nouveaux changements des artefacts ou changements déjà traités.
- Avant chaque commit, isoler strictement les changements de la fonctionnalité demandée ; ne jamais inclure les modifications préexistantes ou sans rapport, même si elles touchent les mêmes fichiers.
- Ne jamais reconstruire ou publier un état non vérifié ; relancer les vérifications pertinentes après toute nouvelle modification.
- Après chaque modification fonctionnelle, préparer un commit Git dédié avec un message explicite et vérifier qu’il ne contient aucun changement sans rapport.
- Le push GitHub est obligatoire pour valider une modification destinée au dépôt distant, mais il doit être effectué uniquement après confirmation explicite de l’utilisateur juste avant `git push`.
- Avant un commit, vérifier le diff et l’historique récent ; ne jamais inclure de secrets, fichiers générés ou modifications sans rapport.
- Avant tout déploiement distant, analyser le diff depuis le dernier commit, valider le build et les tests, puis demander une confirmation explicite pour l’action réseau.
