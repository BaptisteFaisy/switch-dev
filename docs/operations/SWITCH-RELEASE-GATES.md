# Verrous de release Switch

Switch développement et Switch production sont deux environnements séparés. Ils ont chacun un verrou monotone et aucun des deux états ne doit être copié, promu ou réinitialisé depuis l'autre.

## PC — développement

Le verrou est implémenté par :

`scripts/switch-development-release-gate.ps1`

État autoritaire local :

`E:\AppsData\SwitchDevelopment\.guard\release-gate\state.json`

Le verrou exige :

- un commit Git descendant du commit déjà accepté ;
- la présence des preuves fonctionnelles obligatoires dans les sources et les tests ;
- un paquet complet `cst-server.exe` + `dist` ;
- des empreintes exactes pour le serveur, l'index, le service worker et l'asset d'entrée ;
- un manifeste candidat lié à la génération et à la release parentes ;
- une génération `YYYYMMDDHHMM` strictement supérieure ;
- un build ID frontend terminé par cette génération exacte ;
- pour un changement backend, un `cst-server` dont `--version` contient le commit source candidat ;
- la correspondance entre le paquet, le processus actif, l'index réellement servi, le commit backend et `/healthz`.

Contrôles courants :

```powershell
npm run release-gate:status
npm run release-gate:audit
npm run release-gate:active
```

Après un build validé et un commit dédié :

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\switch-development-release-gate.ps1 `
  -Operation PrepareCandidate `
  -Generation 202608301900 `
  -ChangeKind frontend `
  -ConfirmAllConcurrentChangesMerged
```

Adapter la génération à l'heure UTC réelle et `ChangeKind` à `frontend`, `backend` ou `mixed`. Le démarrage protégé met le paquet en cache. Le redémarrage protégé attend l'inactivité, vérifie de nouveau le candidat puis l'enregistre seulement après le retour sain du runtime.

Le lanceur ne sélectionne jamais automatiquement `app.bak-*`. Un paquet incomplet échoue sans arrêter la release active.

## VPS Azure — production

Le verrou autoritaire est :

`/usr/local/sbin/switch-release-gate`

État et politique :

`/srv/switch/ui-runtime/deployment-gate/`

Séquence obligatoire après autorisation explicite de l'action de production :

1. `switch-release-gate audit-active`
2. assembler le candidat comme surcouche de la release acceptée ;
3. écrire `.switch-release.json` avec une génération strictement supérieure ;
4. figer le candidat en lecture seule ;
5. `switch-release-gate verify-candidate /srv/switch/releases/<candidate>` ;
6. vérifier encore l'état actif juste avant la bascule ;
7. drainer, basculer et vérifier le service ;
8. `switch-release-gate record-candidate /srv/switch/releases/<candidate>`.

Le remplacement complet de `dist`, le chargement d'une archive ancienne et le contournement du verrou sont interdits. La restauration de la baseline requiert toujours une autorisation de production explicite et utilise `verify-recovery` puis `record-recovery`.

## Séparation absolue

- Le PC reste le développement.
- Le VPS reste la production.
- Une réussite locale n'autorise pas un déploiement VPS.
- L'état local ne remplace jamais l'état production.
- Le snapshot `azure-vps-release-floor.json` sert uniquement à l'audit humain ; l'état lu sur le VPS au moment de l'opération est toujours autoritaire.
