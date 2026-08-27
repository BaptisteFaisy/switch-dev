# Controle web visible sur le PC

Cette integration donne aux terminaux et aux chats **Codex (ChatGPT dans
Switch)** et **Freebuff** un petit serveur MCP `switch_pc_browser`. Il pilote
une fenetre Google Chrome visible, avec un profil dedie a Switch. Il ne se
connecte jamais au profil Chrome personnel.

## Perimetre

- Environnement vise : Switch developpement sur le PC uniquement.
- VPS : desactive par defaut. L'activation exige explicitement
  `CST_PC_BROWSER_CONTROL=1` **et** les trois fichiers locaux attendus.
- Actions : ouvrir, lire un instantane, cliquer, remplir un champ non sensible,
  choisir une option, appuyer sur une touche sure, revenir et fermer.
- Interdits : JavaScript arbitraire, fichiers, uploads, downloads, presse-papiers,
  mots de passe, PIN, OTP, carte/compte bancaire, URL `file:`, `data:`,
  `javascript:`, localhost et destinations DNS privees.

## Frontiere reseau et cible de deploiement

La copie SSD/T7 ou `pc-fixe` reste exclusivement **Switch developpement**.
L'URL `https://pc-fixe-cst.tail3a8bdf.ts.net/` et son port `:10000` restent
prives : seul le VPS Azure est autorise a s'y connecter via le tailnet. Aucun
Funnel, port public ou bind applicatif sur `0.0.0.0` n'est autorise.

Cette autorisation de connexion ne vaut jamais autorisation de modifier ou de
deployer sur le VPS. Le VPS reste en lecture seule pour cette fonctionnalite,
sauf nouvelle demande utilisateur explicite et non ambigue qui le nomme.

Le broker ne renvoie jamais la valeur courante d'un champ de formulaire. Les
references restent des `ElementHandle` internes au broker et ne sont pas
ecrites dans le DOM. Un proxy local resout chaque nouvelle connexion et refuse
toute reponse DNS contenant une adresse privee ; les service workers, QUIC et
le WebRTC hors proxy sont bloques. Les clics sur boutons/actions, les listes
deroulantes et `Enter`/`Space` affichent une confirmation Windows locale : la
page ou le modele ne peuvent pas l'approuver eux-memes.

Les references d'elements (`s12-e4`, par exemple) ne sont valides que pour le
dernier instantane de la page. Le chat Switch fournit une graine stable, hachee
par le MCP, afin de retrouver le meme onglet apres la relance du provider au
tour suivant. Un terminal conserve son onglet pendant la vie de son MCP. Une
session expire apres trente minutes d'inactivite et ne peut pas choisir le
profil Chrome, le pipe Windows ou une commande hote.

## Pont Windows borne

La cle historique `/srv/cst/ssh-keys/windows_pc_ed25519` ouvre un pont Windows
general. Elle n'est volontairement **pas** reutilisee ici.

`install-agent-browser-broker.ps1` cree une seconde cle
`windows_browser_ed25519` et installe sa cle publique avec une commande SSH
forcee vers `agent-browser-client.mjs`, avec `restrict`. Meme si un agent tente
de modifier la commande distante, Windows lance uniquement ce client. Le
client accepte seulement le protocole JSON valide par le broker Playwright.

## Installation apres restauration du conteneur

Avant chaque build ou deploiement, conserver la worktree vivante complete,
y compris les fichiers suivis, modifies, non suivis et supprimes. Fusionner
toutes les modifications intervenues entre-temps dans un nouveau snapshot
immuable sur `C:`, puis verifier son inventaire SHA-256 juste avant et juste
apres le build. Toute derive, provenance absente ou fusion non resolue bloque
l'operation. Ne jamais effacer les changements avec reset/clean/restore,
checkout, suppression de stash ou copie forcee. Une cible VPS exige en plus
une instruction utilisateur courante, explicite et non ambigue nommant le VPS.

1. Copier ces fichiers vers
   `C:\Users\jeanp\codex-switch-terminal\scripts` :

   - `agent-browser-core.mjs`
   - `agent-browser-broker.mjs`
   - `agent-browser-client.mjs`
   - `agent-browser-ssh-command.cmd`
   - `install-agent-browser-broker.ps1`

2. Verifier que `playwright-core` est installe a la racine du projet, puis
   executer `install-agent-browser-broker.ps1` dans un PowerShell **eleve**. Le
   script fusionne la cle forcee dans
   `C:\ProgramData\ssh\administrators_authorized_keys` en preservant ses ACL,
   epingle la cle hote locale avec `ssh-keyscan`, installe le demarrage
   automatique puis valide le pipe et le trajet SSH force. Il n'ouvre pas
   Chrome avant la premiere vraie demande.

3. Monter ou copier la cle privee produite vers
   `/srv/cst/ssh-keys/windows_browser_ed25519` dans **l'unique conteneur
   `codex-switch-terminal-prepapp`**, proprietaire `cst`, mode `0600`. Le fichier
   `windows_known_hosts` produit par l'installeur vers
   `/srv/cst/ssh-keys/windows_known_hosts`, proprietaire `cst`, mode `0600`.

4. Appliquer d'abord la porte de fraicheur de `AGENTS.md`, puis construire et
   deployer la source uniquement dans PrepApp/Switch developpement. Ne pas
   creer un second conteneur et ne rien deployer sur le VPS. L'image installe :

   - `/usr/local/bin/cst-agent-browser-mcp`
   - `/usr/local/bin/cst-connect-windows-browser`

5. Dans le runtime **PrepApp local seulement**, definir
   `CST_PC_BROWSER_CONTROL=1`. Ne pas definir cette variable dans le runtime
   VPS. Ouvrir ensuite un nouveau terminal ou un nouveau tour de chat. La configuration est
   ajoutee automatiquement aux homes Codex (`config.toml`) et Freebuff
   (`.agents/mcp.json`) uniquement si la cle dediee, `windows_known_hosts` et le
   client MCP sont tous presents.

L'absence de `CST_PC_BROWSER_CONTROL=1` desactive l'integration et retire
l'entree geree par Switch au prochain provisionnement du compte. Les tours de
preuve visuelle sans reseau desactivent aussi explicitement ce MCP.

## Plafond du profil Chrome

Passer `-ProfileDirectory` a l'installeur pour placer le profil dedie dans le
stockage hote reserve au clone. Le broker accepte aussi
`CST_AGENT_BROWSER_PROFILE_DIR`. Les caches disque et media de Chrome sont
bloques respectivement a 256 Mio et 64 Mio ; uploads et downloads restent
interdits. Le dossier par defaut de l'installeur est
`C:\Users\jeanp\Documents\Switch-PrepApp\data\agent-browser-profile` et doit
etre remplace par le chemin du stockage plafonne lors de l'installation finale.

## Verification

```text
node --test tests/agent-browser-control.test.mjs
cargo test --manifest-path src-tauri/Cargo.toml browser_mcp --lib
```

Test manuel attendu dans un nouveau terminal Freebuff ou Codex :

```text
Ouvre https://example.com sur mon PC, lis le titre, puis ferme la page.
```

Chrome doit apparaitre dans le profil **CodexSwitchTerminal / agent-browser**,
le titre doit revenir dans le terminal, puis l'onglet doit se fermer.
