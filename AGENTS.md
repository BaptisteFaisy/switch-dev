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

# Procédure obligatoire avant build et publication

- Avant chaque build, analyser l’état des modifications et le diff afin de distinguer les nouveaux changements des artefacts ou changements déjà traités.
- Avant chaque commit, isoler strictement les changements de la fonctionnalité demandée ; ne jamais inclure les modifications préexistantes ou sans rapport, même si elles touchent les mêmes fichiers.
- Ne jamais reconstruire ou publier un état non vérifié ; relancer les vérifications pertinentes après toute nouvelle modification.
- Après chaque modification fonctionnelle, préparer un commit Git dédié avec un message explicite et vérifier qu’il ne contient aucun changement sans rapport.
- Le push GitHub est obligatoire pour valider une modification destinée au dépôt distant, mais il doit être effectué uniquement après confirmation explicite de l’utilisateur juste avant `git push`.
- Avant un commit, vérifier le diff et l’historique récent ; ne jamais inclure de secrets, fichiers générés ou modifications sans rapport.
- Avant tout déploiement distant, analyser le diff depuis le dernier commit, valider le build et les tests, puis demander une confirmation explicite pour l’action réseau.
