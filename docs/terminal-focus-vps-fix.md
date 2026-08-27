# Correctif de saisie terminal après bascule Chat → Terminal

## Symptôme

Sur l'interface web hébergée sur le VPS, après avoir basculé d'un chat vers un
terminal, le terminal pouvait apparaître sélectionné mais ne plus accepter les
frappes clavier.

## Diagnostic

Le terminal utilise xterm.js et conserve son processus PTY côté serveur. Le
changement de vue reconstruit le DOM, puis lance une réconciliation des
terminaux actifs afin de rattacher le PTY distant. Pendant cette courte fenêtre
:

1. le textarea caché de xterm pouvait perdre le focus après le remontage ;
2. `ptyId` pouvait être temporairement nul ou la session marquée inactive ;
3. `onData` ignorait alors silencieusement les caractères saisis ;
4. une erreur d'écriture distante était également ignorée.

Le problème n'était donc pas nécessairement un terminal VPS arrêté : le PTY
pouvait rester vivant alors que le client avait perdu le focus ou la saisie.

## Correctif appliqué

Le correctif est dans `src/main.ts` :

- chaque terminal possède maintenant une file mémoire de saisie en attente ;
- les caractères reçus par xterm pendant le démarrage ou la reconnexion sont
  conservés dans l'ordre ;
- une erreur de `write_terminal` remet les caractères dans cette file ;
- la file est vidée dès que le PTY reçoit son identifiant effectif ;
- la file est supprimée lorsque le terminal est fermé ;
- après la sélection d'un terminal, le focus xterm est restauré après deux
  cycles de rendu, quand le nouvel élément est effectivement connecté au DOM.

Ce mécanisme ne recrée pas le PTY et ne modifie pas le contenu du terminal : il
protège uniquement la livraison des entrées et le focus côté navigateur.

## Vérifications réalisées

- `npx tsc --noEmit` : réussi.
- Tests ciblés :
  `node --test tests/terminal-transport.test.mjs tests/terminal-environments.test.mjs tests/keyboard-shortcuts.test.mjs`
  : 42 tests réussis.
- `npm run build:frontend` n'a pas pu démarrer dans l'environnement local car
  la garde de provenance exige `CST_CANONICAL_SOURCE_ROOT` et
  `CST_BUILD_SOURCE_MANIFEST`. Ce blocage est une protection du projet, pas une
  erreur de compilation du correctif.

## Verrou clavier (2026-08)

Le correctif initial a été complété par un verrou clavier en défense en profondeur,
toujours dans `src/main.ts` :

1. un chien de garde reprend le focus du terminal actif toutes les 500 ms (sans
   jamais voler le focus d'un champ réellement éditable) ;
2. un intercepteur global `keydown` re-délivre à xterm la première frappe tapée
   hors focus (alt-tab, remontage DOM) ; enregistré en dernier dans
   `setupEvents`, les raccourcis applicatifs restent prioritaires ;
3. un garde-fou périodique vide une file `terminalPendingInput` stagnante dès
   que la session est prête, et `attachRestoredTerminal` flush après
   rattachement ; `writeRemoteTerminal` re-tamponne une frappe dont le POST a
   échoué pendant une coupure ;
4. un badge par terminal (`expert-terminal-keyboard-badge`) montre l'état réel :
   focus, file d'attente, transport (`terminalTransportState` de
   `src/platform.ts`). Un clic sur le badge re-rattache ou refocusse.

Le test `tests/terminal-focus-keeper.test.mjs` protège ces couches.

## Règle de conservation

Ne pas supprimer la file `terminalPendingInput`, l'appel `flushTerminalInput`
ou la restauration différée du focus sans reproduire d'abord le scénario VPS
Chat → Terminal et ajouter un test de non-régression équivalent. Une correction
future du WebSocket ou du serveur ne doit pas retirer cette protection côté
client : la perte de focus et la reconnexion sont deux problèmes distincts.

Avant une publication, vérifier que ce fichier reste présent dans le snapshot
source et dans l'artefact publié.
