# Controle d'ecran visible sur le PC

Cette integration donne aux terminaux et aux chats **Codex (ChatGPT dans
Switch)** et **Freebuff** un petit serveur MCP `switch_pc_screen`. Il pilote
l'ecran visible du poste Windows : capture d'ecran, deplacement de la souris,
clics, defilement, touches de navigation et saisie de texte non sensible.
Elle s'appuie sur la meme architecture que le controle navigateur
(`docs/PC_BROWSER_CONTROL.md`) mais agit sur le bureau entier, pas sur un
profil Chrome dedie.

## Perimetre

- Environnement vise : Switch developpement sur le PC uniquement.
- VPS : desactive par defaut. L'activation exige explicitement
  `CST_PC_SCREEN_CONTROL=1` **et** la cle, le known hosts et le client
  presents dans le conteneur.
- Actions : capturer l'ecran (tous les moniteurs, re-echantillonne en JPEG),
  deplacer la souris, clic gauche/droit, double-clic, defiler (molette),
  appuyer sur une touche de navigation sure, saisir un texte non sensible,
  lire la fenetre active, lister les fenetres, lancer une application sans
  focus. **Chaque action renvoie automatiquement une nouvelle capture** : le
  chat suit l'ecran a chaque etape sans action supplementaire.
- Touches autorisees : Enter, Escape, Tab, Shift+Tab, Backspace, Delete,
  fleches, Home, End, PageUp, PageDown, Space, F6 (cycler
  barre d'adresse/contenu dans les navigateurs), Ctrl+L (focaliser la barre
  d'adresse) et Win (menu Demarrer, puis saisie pour lancer une application).
- Interdits : presse-papiers, JavaScript arbitraire sur le poste, fichiers,
  uploads, downloads, mots de passe, PIN, OTP, carte/compte bancaire,
  commandes hote, Alt+Tab, Ctrl+Alt+Suppr, Ctrl+W, Ctrl+T et toute
  combinaison hors liste. Le JavaScript autorise dans une page web via
  `browser_eval` reste confine a CETTE page (profil isole, sans comptes).

## Frontiere reseau et cible de deploiement

La copie SSD/T7 ou `pc-fixe` reste exclusivement **Switch developpement**.
L'URL `https://pc-fixe-cst.tail3a8bdf.ts.net/` et son port `:10000` restent
prives : seul le VPS Azure est autorise a s'y connecter via le tailnet. Aucun
Funnel, port public ou bind applicatif sur `0.0.0.0` n'est autorise.

Cette autorisation de connexion ne vaut jamais autorisation de modifier ou de
deployer sur le VPS. Le VPS reste en lecture seule pour cette fonctionnalite,
sauf nouvelle demande utilisateur explicite et non ambigue qui le nomme.

Le broker ne renvoie jamais de contenu du presse-papiers ni la valeur d'un
champ de formulaire : la seule donnee lue sur l'ecran est l'image JPEG de la
capture (re-echantillonnee a 1600 px max, plafonnee a ~850 Ko en base64) et le
titre de la fenetre active. Aucune OCR n'est effectuee cote poste : le modele
lit l'image renvoyee par le MCP.

Les clics (gauche, droit, double), les saisies de texte et les touches
`Enter`/`Space` affichent une confirmation Windows locale : le modele ne peut
pas l'approuver lui-meme. Le deplacement, le defilement, la capture et les
autres touches de navigation passent sans confirmation.

**Mode session armee (`screen_arm` / `screen_disarm`)** : pour une mission
autonome bornee, le chat peut armer sa session (`minutes` 1 a 60, defaut 10).
Ensuite, toutes les actions de cette session (clics, saisies, touches)
passent sans popup jusqu'a expiration ou `screen_disarm`.

**Aucune popup Windows ne valide l'armement** : les fenetres de confirmation
lancees depuis un processus en arriere-plan ne s'affichent pas de facon
fiable (visible=False), et l'armement doit donc rester **exclusivement
autorise par une demande explicite et non ambigue de l'utilisateur dans le
chat** (ex. « fais-le tout seul », « ouvre Chrome et cherche X »), pour une
duree bornee. L'armement et le desarmement sont traces dans le journal du
broker (`agent-screen-broker.log`).

## Mode arriere-plan : cibler une fenetre sans prendre le focus

Le chat peut travailler **en arriere-plan** pendant que l'utilisateur utilise
son PC : la fenetre active de l'utilisateur ne change jamais.

- `screen_windows` liste les fenetres visibles (id, pid, titre, position).
  L'id sert de parametre `window` aux autres outils.
- Avec `window`, la capture utilise `PrintWindow` (la fenetre peut etre
  derriere d'autres) et les clics/saisie/touches sont envoyes a CETTE fenetre
  par messages (`SendMessage`), sans deplacement de souris ni activation.
- `screen_open` lance une application **sans focus** (`CreateProcess` avec
  `SW_SHOWNOACTIVATE`) : la fenetre active de l'utilisateur reste inchangee.
- `screen_locate` repere un texte par OCR Windows natif et renvoie ses
  coordonnees de clic pretes (`clickX`/`clickY` dans l'espace ecran sans
  `window`, CLIENT avec `window`) : navigation sans calcul manuel, meme quand
  l'interface change. Lecture seule, aucune confirmation.
- `browser_*` accepte un `cdpPort` explicite pour piloter un Chromium deja
  lance avec un port de debug — notamment le **WebView2 de l'app elle-meme** :
  lancer l'application avec `CST_WEBVIEW_CDP_PORT=9223` expose le protocole
  DevTools sur ce port loopback, et `browser_list`/`browser_screenshot`/
  `browser_eval`/`browser_click` naviguent alors le DOM de l'app sans aucun
  focus ni souris. Desactive par defaut (aucun port ouvert sans ce choix
  explicite).

**Limite honnete des messages fenetres** : les applications modernes
(Chrome/Edge, WebView2, applications WinUI) ignorent les messages clavier
(`WM_CHAR`) quand leur fenetre est inactive — verifie en test sur l'app :
le clic fantome passe (la fenetre reagit sans voler le focus) mais la saisie
clavier n'atteint pas le composeur. Les applications classiques (Boite de
dialogue, console, fenetres Win32 natives) repondent bien ; Chrome, Edge et
l'app elle-meme sont pilotes par CDP (ci-dessous), beaucoup plus fiable.

### Pilotage navigateur Chrome/Edge par CDP (`browser_*`)

Pour Chrome/Edge, `screen_open` ajoute automatiquement : un **profil isole**
(dossier temporaire, sans comptes ni mots de passe de l'utilisateur),
`--remote-debugging-port=0` (port aleatoire, lu dans `DevToolsActivePort`) et
`--start-minimized` (la fenetre demarre reduite : aucun focus vole, Chrome
active sinon sa premiere fenetre malgre `SW_SHOWNOACTIVATE`). La commande
renvoie `debugPort`.

Les outils `browser_list`, `browser_screenshot`, `browser_navigate`,
`browser_eval`, `browser_click`, `browser_type` et `browser_key` pilotent
alors la page via le Chrome DevTools Protocol, **independamment du focus et
meme si la fenetre est reduite ou masquee** : capture reelle de la page,
navigation, lecture de texte (`document.body.innerText`), clics par selecteur
CSS, saisie de formulaire, touches. La saisie de texte y applique les memes
refus (mots de passe, OTP, donnees bancaires).

Les methodes mutantes (`navigate`, `eval`, `click`, `type`, `key`) demandent
autorisation (confirmation ou session armee) ; `list` et `screenshot` sont en
lecture seule.

### Recherche Google sans interstitiel anti-bot

Google affiche sa page « trafic exceptionnel » sur un profil neuf sans
cookies de consentement. Le pilotage CDP neutralise les signaux
d'automatisation (un vrai Chrome fenetre ne fuit presque rien : pas de
`cdc_`, plugins et WebGL reels, en-tetes `sec-ch-ua` presents) :

- `--disable-blink-features=AutomationControlled` au lancement,
- surcharge de `navigator.webdriver` injectee par
  `Page.addScriptToEvaluateOnNewDocument` avant chaque navigation,
- **consentement Google accepte automatiquement** : si la page contient le
  bouton « Tout accepter » (`#L2AGLb`), `browser_navigate` clique dessus
  apres navigation, comme le ferait un utilisateur. Ceci ne concerne que le
  profil isole de l'agent (jamais le profil reel de l'utilisateur) et pose
  les cookies de consentement dans ce profil.

Verifie en reel sur le poste : deux profils neufs successifs, recherche
Google OK des la premiere navigation (et les suivantes, cookies poses),
avec `navigator.webdriver` propre et focus utilisateur inchange. Si Google
renvoie quand meme l'interstitiel (rare, selon IP/reseau), l'agent peut
utiliser un autre moteur (DuckDuckGo, Bing) via `browser_navigate`.

## Pont Windows borne

Comme pour le navigateur agent, une cle dediee est obligatoire : la cle
historique `/srv/cst/ssh-keys/windows_pc_ed25519` ouvre un pont Windows
general et n'est volontairement **pas** reutilisee.

`install-agent-screen-broker.ps1` cree la cle `windows_screen_ed25519` et
installe sa cle publique avec une commande SSH forcee vers
`agent-screen-client.mjs`, avec `restrict`. Meme si un agent tente de modifier
la commande distante, Windows lance uniquement ce client. Le client accepte
seulement le protocole JSON valide par le broker ecran
(`agent-screen-broker.mjs`), qui parle au pipe nomme
`\\.\pipe\CodexSwitchAgentScreen`.

Le broker tourne au logon Windows (raccourci dans le dossier Demarrage) : il a
ainsi acces au bureau interactif pour les captures, l'injection de saisie
(`SendInput`) et les fenetres de confirmation locales. Il execute chaque
action dans un processus PowerShell isole (script genere, lance via `-File`
depuis le dossier temporaire) ; aucune dependance native n'est installee.

## Installation apres restauration du conteneur

1. Copier ces fichiers vers
   `C:\Users\jeanp\codex-switch-terminal\scripts` :

   - `agent-screen-core.mjs`
   - `agent-screen-broker.mjs`
   - `agent-screen-client.mjs`
   - `agent-screen-ssh-command.cmd`
   - `install-agent-screen-broker.ps1`

2. Executer `install-agent-screen-broker.ps1` dans un PowerShell **eleve**. Le
   script fusionne la cle forcee dans
   `C:\ProgramData\ssh\administrators_authorized_keys` en preservant ses ACL,
   epingle la cle hote locale avec `ssh-keyscan` (dans le `windows_known_hosts`
   partage avec le navigateur agent s'il existe, sinon dans le sien), installe
   le demarrage automatique puis valide le pipe et le trajet SSH force. Il ne
   touche ni a Chrome ni au profil navigateur.

3. Monter ou copier la cle privee produite vers
   `/srv/cst/ssh-keys/windows_screen_ed25519` dans **l'unique conteneur
   `codex-switch-terminal-prepapp`**, proprietaire `cst`, mode `0600`. Le
   fichier `windows_known_hosts` produit par l'installeur vers
   `/srv/cst/ssh-keys/windows_known_hosts`, proprietaire `cst`, mode `0600`
   (ce fichier est deja partage avec le controle navigateur).

4. Dans le runtime **PrepApp local seulement**, definir
   `CST_PC_SCREEN_CONTROL=1`. Ne pas definir cette variable dans le runtime
   VPS. Enregistrer le serveur MCP `switch-pc-screen` dans les homes Codex
   (`config.toml`) et Freebuff (`.agents/mcp.json`) avec la commande
   `/usr/local/bin/cst-agent-screen-mcp`, comme pour le serveur navigateur.
   Ouvrir ensuite un nouveau terminal ou un nouveau tour de chat.

L'absence de `CST_PC_SCREEN_CONTROL=1` desactive l'integration et retire
l'entree MCP au prochain provisionnement du compte.

## Serveur MCP cote conteneur

- `cst-agent-screen-mcp.mjs` expose les outils `screen_snapshot`,
  `screen_move`, `screen_click`, `screen_double_click`, `screen_right_click`,
  `screen_type`, `screen_press`, `screen_scroll`, `screen_windows`,
  `screen_open`, `screen_arm`, `screen_disarm`, `screen_health` et les outils
  navigateur `browser_list`, `browser_screenshot`, `browser_navigate`,
  `browser_eval`, `browser_click`, `browser_type`, `browser_key`. Il relaie
  chaque demande au pont `cst-connect-windows-screen`
  (`/usr/local/bin/cst-connect-windows-screen`), qui ouvre le SSH force vers
  le poste avec la cle `windows_screen_ed25519` et le known hosts partage.
- La capture est renvoyee en contenu image (`image/jpeg`) accompagnee d'un
  resume textuel avec la resolution reelle et l'echelle. Les coordonnees des
  outils de clic sont celles de l'ecran REEL : multiplier une position vue
  dans l'image par `screenWidth / width`.
- Variables d'environnement : `CST_WINDOWS_AGENT_SCREEN_BRIDGE` (chemin du
  pont, defaut `/usr/local/bin/cst-connect-windows-screen`),
  `CST_AGENT_SCREEN_SESSION_SEED` (graine stable de session par terminal) et,
  cote pont, `CST_WINDOWS_SCREEN_HOST/PORT/USER/KEY` et
  `CST_WINDOWS_KNOWN_HOSTS`.

## Plafonds

- Capture : ecran virtuel complet (tous les moniteurs), re-echantillonne a
  1600 px au plus, JPEG avec qualite decroissante (72, 55, 40) jusqu'a tenir
  sous ~850 Ko en base64. Une capture placeholder signale un echec
  d'encodage.
- Saisie : 2000 caracteres max, sans caracteres de controle, sans contenu
  sensible (mot de passe, OTP, donnees bancaires, suites de 13 a 19 chiffres).
- Touches : uniquement la liste autorisee (navigation + F6, Ctrl+L, Win),
  chaque touche etant une sequence de codes virtuels relachee en sens inverse
  (modificateurs inclus). Dans une page (CDP), Win est exclu.
- Defilement : -20 a +20 crans, position optionnelle du curseur.
- Navigateur : selecteurs CSS <= 500 caracteres, expressions <= 4000, URL
  http/https <= 4000, texte de saisie <= 2000 sans contenu sensible.

## Verification

```text
node --test tests/agent-screen-control.test.mjs
```

Test manuel attendu dans un nouveau terminal Freebuff ou Codex :

```text
Prends une capture de mon ecran, deplace la souris au centre, puis dis-moi
quelle est la fenetre active.
```

La capture doit revenir dans le terminal avec la resolution reelle, la souris
doit se deplacer sans confirmation, et la fenetre active doit etre nommee.
Un clic ou une saisie doivent au contraire declencher la confirmation Windows
locale avant de s'executer.

Test manuel attendu du mode arriere-plan :

```text
arme 10 minutes, ouvre Chrome en arriere-plan et cherche « meteo Paris »,
puis rends-moi le titre du premier resultat, sans prendre le focus.
```

La fenetre active de l'utilisateur doit rester inchangee pendant toute la
mission ; Chrome demarre reduit avec un profil isole ; chaque etape
(navigation, lecture, clic) passe par CDP avec capture reelle de la page ;
la session est desarmee a la fin.
