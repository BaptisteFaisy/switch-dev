# Déploiement web continu de bout en bout

> **Classification des environnements :** cette chaîne de déploiement stable /
> production cible exclusivement le **VPS Microsoft Azure**. Le checkout sur
> SSD/T7 et le poste local **pc-fixe** restent l'environnement de
> **développement** et ne doivent jamais être utilisés comme cible stable.

> **Porte de provenance obligatoire :** avant chaque build de cette chaîne,
> inventorier la source canonique, fusionner toutes les modifications apparues
> entre-temps dans un snapshot immuable, puis exécuter
> `scripts/switch-build-source-guard.ps1` en `Capture` et `Verify`. Refaire le
> contrôle immédiatement après le build. Une worktree sale n'est jamais
> écrasée ou « nettoyée » ; conflit, provenance absente ou dérive rend
> l'artefact non déployable. Une étiquette `dirty` ne remplace pas cette preuve.

La chaîne active est la suivante :

1. un agent distant soumet ses commits avec `submit_for_merge` ;
2. la merge queue intègre les commits et pousse automatiquement la branche du
   miroir vers `origin` ;
3. un push sur `main` déclenche `.github/workflows/deploy-web.yml` ;
4. GitHub Actions teste le frontend, puis construit le site ; si `src-tauri/`
   (ou `rust-toolchain.toml`) a changé depuis le push précédent, la CI compile
   aussi `cst-server` en CI et **signe les artefacts** (minisign, fail-closed
   sur le nœud). Les artefacts sont transférés par SSH : binaire précompilé +
   `dist`, ou **`dist` seul** pour un push frontend-only (aucune compilation) ;
5. le nœud vérifie l'artefact (empreinte SHA-256 + signature minisign),
   draine les sessions, bascule vers un dossier propre au commit, redémarre et
   effectue sa sonde de santé avec rollback automatique — **sans jamais
   recompiler sur l'hôte** ;
6. les navigateurs voient le nouveau commit via `/healthz`, actualisent leur
   service worker et se rechargent automatiquement sous cinq secondes.

Il n'y a aucune approbation humaine dans ce flux. Les contrôles techniques
(conflits Git, build, signature, santé du serveur et rollback) restent actifs.

## Cadence : batching, précompilation et pushes frontend-only

Un seul déploiement à la fois (concurrency `cst-production-web`,
`cancel-in-progress: false`) : les pushes s'empilent et sont traités dans
l'ordre. La cadence réelle est donc **par push**, pas par modification : la
merge queue regroupe tes soumissions en un seul push, et la CI ne recompile le
binaire Rust que lorsque `src-tauri/` a réellement changé. Un push UI-only
bascule en quelques secondes (transfert de `dist` + drain court), sans
compilation sur l'hôte ni nouveau binaire.

## Configuration GitHub

Ajouter dans **Settings → Secrets and variables → Actions** :

- secret `CST_DEPLOY_TARGET` : `utilisateur@hote` ;
- secret `CST_DEPLOY_SSH_KEY` : clé privée SSH dédiée ;
- secret `CST_DEPLOY_KNOWN_HOSTS` : ligne produite par
  `ssh-keyscan -H <hote>` après vérification de son empreinte ;
- variable facultative `CST_DEPLOY_PORT` : `22` par défaut ;
- secret `CST_DEPLOY_ENV_FILE` : contenu complet de
  `deploy/cst-server.env.example`, requis uniquement pour le premier
  déploiement automatique.
- secret `MINISIGN_SECRET_KEY` (+ `MINISIGN_PASSWORD`) : clé privée minisign,
  **optionnel**. Absente → artefacts non signés et le nœud reçoit
  `--allow-unsigned` (intégrité SHA-256/SSH conservée, mais pas
  d'authenticité). Présente → la clé **publique** doit être commitée dans
  `deploy/minisign.pub` (ligne `RW...`), sinon le déploiement échoue en
  fail-closed. Voir `deploy/PHASE2-UPDATES.md` pour générer les clés.

L'utilisateur SSH doit pouvoir exécuter les deux installateurs avec
`sudo -n`. La clé de déploiement ne doit pas être réutilisée comme clé de
connexion personnelle.

## Portabilité : changer de VPS

Aucun chemin ni hôte n'est codé en dur dans le workflow : pour pointer la
chaîne vers **un autre VPS**, il suffit de remplacer les secrets
`CST_DEPLOY_TARGET`, `CST_DEPLOY_SSH_KEY`, `CST_DEPLOY_KNOWN_HOSTS` (et
éventuellement `CST_DEPLOY_PORT` / `CST_DEPLOY_ENV_FILE` pour un premier
déploiement). Le nœud doit disposer de `minisign` (signature activée) et des
mêmes droits `sudo -n` ; l'état (config, comptes) reste dans `/srv/cst` et
`/etc/codex-switch-terminal.env`, séparés du code.

## Publication des merges agents

Le comportement par défaut est `CST_MERGE_AUTO_PUSH=remote` : seuls les miroirs
SaaS sont publiés automatiquement. Les dossiers Git ouverts localement restent
locaux. Les valeurs disponibles sont :

- `remote` : miroirs distants uniquement ;
- `always` : miroirs distants et dépôts locaux ;
- `never` : aucune publication automatique.

`CST_MERGE_PUSH_REMOTE` choisit le remote, `origin` par défaut. Pour une URL
HTTPS GitHub, GitLab ou Bitbucket, le push réutilise `CST_GIT_PAT` en mémoire,
sans enregistrer le jeton dans le miroir ou le journal de merge.

La soumission `submit_for_merge` conserve `verify: false` par défaut. Pour
activer une commande de tests sur une soumission particulière, définir
`CST_MERGE_VERIFY_COMMAND` et envoyer `verify: true`.
