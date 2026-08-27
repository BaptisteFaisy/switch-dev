# Codex Switch Terminal

Application Tauri + xterm.js pour utiliser plusieurs comptes Codex, Claude ou
des fournisseurs API pilotes par OpenCode, ouvrir des chats et lancer des
terminaux dans un projet choisi.

## Environnements Switch

- La version stockée sur le **SSD Samsung T7** ou exécutée sur le poste local
  **pc-fixe** est exclusivement la version de **développement**.
- La version hébergée sur le **VPS Microsoft Azure** est la version
  **stable / production**.
- Sans autre précision, les termes « stable », « production » et « prod »
  désignent toujours le VPS Azure, jamais la copie SSD/locale.
- Les deux environnements restent séparés : aucune synchronisation, promotion,
  mise en production ou relance croisée sans demande explicite de Jean-Pierre.

## Porte obligatoire avant chaque build

Toute compilation doit partir d'un nouveau snapshot immuable qui conserve et
fusionne les modifications faites entre-temps. La source canonique et le
snapshot sont inventoriés par SHA-256 avant et après le build ; une dérive,
un conflit ou une provenance absente invalide l'artefact. Il est interdit de
« nettoyer » une worktree avec reset/clean/restore/checkout, stash drop, une
copie forcée ou une synchronisation miroir.

`scripts/switch-build-source-guard.ps1` réalise la capture et la vérification.
Les commandes `npm run build`, `build:frontend`, `build:server` et
`build:android` appellent désormais cette porte et refusent de démarrer sans
manifeste frais. Aucun déploiement VPS ne peut être déduit d'un « go » générique
ou d'une commande visant l'environnement de développement.

## Principe

Le fonctionnement est volontairement direct :

- un chat ou un terminal s'execute dans le dossier projet selectionne ;
- le compte utilise son `CODEX_HOME` ou son `CLAUDE_CONFIG_DIR` habituel ;
- les fichiers et le depot Git ouverts sont ceux du dossier selectionne ;
- un terminal existant garde son dossier, meme si l'environnement actif change ;
- les discussions, terminaux et comptes restent regroupes dans l'interface.

Les commandes Git habituelles fonctionnent exactement comme dans un terminal
classique, sans protocole interne supplementaire.

## Prerequis

- Node.js 20+
- Rust et Cargo via rustup
- Codex CLI ou Claude CLI disponible dans le `PATH`
- OpenCode dans le `PATH` pour Z.ai, MiniMax, DeepSeek et OpenRouter :
  `npm install -g opencode-ai` (l'image serveur l'embarque deja)

## Demarrage desktop

```powershell
npm install
npm run dev
```

Sous Windows, tu peux aussi double-cliquer sur
`Lancer Codex Switch Terminal.cmd`. Le lanceur propose l'application locale,
l'application Cloud, le site web local et les commandes du serveur.

Depuis PowerShell :

```powershell
& ".\Lancer Codex Switch Terminal.cmd" app
& ".\Lancer Codex Switch Terminal.cmd" cloud
& ".\Lancer Codex Switch Terminal.cmd" web
& ".\Lancer Codex Switch Terminal.cmd" server
& ".\Lancer Codex Switch Terminal.cmd" stop
```

## Git : mise a jour et push

Pour mettre a jour un checkout propre :

```powershell
git pull --ff-only
```

Pour publier une modification :

```powershell
git add <fichiers>
git commit -m "description courte"
git pull --rebase
git push
```

Si le depot distant avance entre le rebase et le push, relance simplement
`git pull --rebase`, les controles utiles, puis `git push`. Aucun push force
n'est necessaire.

## Comptes et environnements

- Les comptes Codex sont des dossiers utilises comme `CODEX_HOME`.
- Les comptes Claude Code utilisent un `CLAUDE_CONFIG_DIR` isole.
- Z.ai, MiniMax, DeepSeek et OpenRouter passent par OpenCode. Chaque compte
  recoit ses propres repertoires XDG : les cles API, la configuration, le cache
  et les sessions ne sont pas partages entre comptes.
- L'application detecte les dossiers `~\.codex*` contenant `auth.json` ou
  `config.toml`.
- Les profils proxy sont lus depuis les fichiers `proxy.txt` de ces dossiers.
- Le terminal transmet `CODEX_HOME`, `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`
  et leurs variantes minuscules selon le compte selectionne.
- Un environnement projet doit etre choisi explicitement avant de lancer un
  chat ou un terminal.
- Les reglages de l'application sont stockes dans
  `%APPDATA%\codex-switch-terminal\settings.json`.

## Studio IA : images et videos generatives

L'onglet **Studio IA** genere des videos depuis un texte ou une image avec Wan
2.6, Veo 3.1 Fast, Kling 3.0 ou Luma Ray 2. Il genere aussi des images avec FLUX
2 Flash, Ideogram V3 ou Recraft V3. Les generations sont placees dans une file
d'attente, restent suivies lorsque tu changes d'onglet et apparaissent dans des
historiques locaux avec apercu et ouverture du resultat.

Le studio utilise [fal.ai](https://fal.ai/) comme passerelle. Dans **Gerer les
comptes**, ajoute une ou plusieurs cles fal personnelles ou d'equipe, choisis le
compte actif et, si besoin, definis le compte par defaut. Les mots de passe
Google, Kling, Luma ou Alibaba ne sont jamais demandes. Les cles sont validees
par le backend, stockees dans `creative-accounts.json` cote application/serveur
et ne sont jamais renvoyees au navigateur.

Pour un deploiement administre, une cle d'environnement peut aussi servir de
compte de secours :

```powershell
$env:CST_FAL_KEY = "identifiant:secret"
npm run dev
```

Sur un serveur Linux, ajoute `CST_FAL_KEY` au fichier d'environnement du
service. Chaque utilisateur web authentifie dispose de sa propre liste de
comptes creatifs. Chaque generation est facturee par fal.ai selon le moteur et
les options selectionnes.

### Memoire des chats

- Une conversation conserve son propre historique et reprend la meme session
  CLI apres sa fermeture ou un redemarrage.
- Chaque environnement possede aussi une **memoire partagee** visible et editable
  depuis le bouton **Memoire** de sa ligne dans le selecteur d'environnement.
  Elle est stockee dans `settings.json` et
  injectee comme contexte systeme dans tous les chats Codex et Claude de ce
  dossier, quel que soit le compte utilise.
- Les chats Codex activent en plus la memoire automatique locale du CLI. Ses
  souvenirs generes restent dans le `CODEX_HOME` du compte ; la memoire
  partagee de Switch est la couche qui garantit l'isolation par environnement.
- Une memoire est du contexte, pas un coffre-fort : n'y place aucun secret.

### Travail en equipe sur un VPS

Chaque personne se connecte avec son propre compte utilisateur. Le proprietaire
d'un environnement peut ensuite ouvrir le selecteur d'environnement, cliquer
sur le bouclier de l'espace concerne et copier son **code d'invitation**. Le
second utilisateur saisit ce code dans **Rejoindre un espace d'equipe** ; le
partage ne devient actif qu'apres acceptation par le proprietaire.

Les membres autorises retrouvent le meme projet, sa memoire, l'historique de ses
chats, ses agents autonomes et ses orchestrations. Les conversations sont
filtrees d'apres l'environnement auquel elles appartiennent : partager un
projet n'expose donc ni les autres environnements, ni les preferences locales,
ni les terminaux interactifs d'un autre utilisateur. Le proprietaire peut
revoquer un membre a tout moment depuis le meme panneau.

Si les inscriptions ont ete fermees apres la creation du compte proprietaire,
reactive temporairement `CST_ALLOW_REGISTRATION=true` pour creer le second
compte, puis repasse la variable a `false`.

Pour connecter un nouveau compte Codex, cree son dossier dans l'interface,
ouvre un terminal avec ce compte, puis lance :

```powershell
codex login
```

Pour un fournisseur annexe, choisis Z.ai, MiniMax, DeepSeek ou OpenRouter dans
la page **Comptes**. L'application ouvre automatiquement la commande OpenCode
`auth login --provider <fournisseur>` dans le home isole du nouveau compte. Ce
login n'ouvre aucun navigateur : OpenCode demande la cle API du fournisseur
directement dans le terminal. Un champ **Cle API** apparait alors sous le
terminal pour la coller sans passer par le presse-papiers, puis le terminal
temporaire se ferme des que la connexion est enregistree. Si OpenCode manque
dans le `PATH`, le message d'installation s'affiche au lieu d'une attente sans
fin. La cle API n'est jamais copiee dans `settings.json`.

Avant d'afficher cette invite, `opencode auth login` bootstrape son environnement :
il telecharge le catalogue models.dev (3,2 Mo) puis installe `@opencode-ai/plugin`
(~60 Mo de `node_modules`). Ce bootstrap est **mutualise entre tous les comptes**
et pre-chauffe dans l'image serveur ; sans cela il etait repaye par chaque nouveau
compte, soit environ 8 minutes de terminal muet sur un VPS 2 vCPU. Seuls les
identifiants et les sessions restent isoles par compte (`<home>/data/opencode/`).
`CST_OPENCODE_RUNTIME_DIR` deplace ce runtime partage ; par defaut il est cree a
cote des homes de comptes.

## Fonctionnalites principales

- grille paginee de chats independants ;
- choix du compte, du modele et de l'intensite de raisonnement ;
- reprise, deplacement et archivage des discussions ;
- terminaux PTY groupes par environnement ;
- import de comptes depuis des exports JSON ;
- suivi des quotas et selection d'un compte disponible ;
- onglet **Transcrire** pour envoyer WAV, MP3, M4A, FLAC, OGG, OPUS ou WebM au GPU du VPS ;
- lecture des e-mails et de l'agenda Microsoft 365 depuis un chat, envois confirmes a la main ;
- chats autonomes persistants avec reprise, pause et planification ;
- interface desktop, web et mobile.

## Transcription audio sur le GPU du VPS

L'onglet **Transcrire** accepte un fichier audio jusqu'a 100 Mo, l'envoie en
binaire a `cst-server`, puis affiche la transcription Whisper exacte. Il ne
lance pas Ollama et ne reformule donc pas le contenu. Le resultat peut etre
copie ou exporte en `.txt`; le fichier temporaire est supprime apres traitement.

Pour un deploiement portable sur un VPS NVIDIA dont le pilote est deja
installe, active le moteur CUDA integre :

```powershell
npm run deploy:vps:portable -- `
  -SshTarget "ubuntu@mon-vps" `
  -NodeId "gpu-vps" `
  -GpuTranscription
```

Ansible installe NVIDIA Container Toolkit, verifie CUDA depuis Docker, puis
lance Speaches/faster-whisper sur le reseau prive de Compose. Aucun port du
moteur n'est publie. Le modele par defaut est
`Systran/faster-whisper-small`; choisis-en un autre avec
`-TranscriptionModel "Systran/faster-whisper-large-v3"`. Le premier fichier
peut prendre plus de temps pendant le telechargement et le chargement du modele.

Avec Compose sans Ansible :

```bash
docker compose -f compose.yaml -f compose.gpu.yaml up -d --wait
```

Le client desktop connecte a un VPS envoie lui aussi les fichiers de cet onglet
au serveur. Ce comportement est volontairement distinct du bouton micro, qui
conserve sa priorite locale decrite ci-dessous.

## Dictee vocale locale (RTX 3060 Ti)

Le bouton micro du compositeur enregistre jusqu'a cinq minutes, transcrit la
voix avec `whisper.cpp`, puis reformule la dictee avec un petit modele Ollama.
Le resultat est insere dans le champ du chat pour relecture : il n'est jamais
envoye automatiquement.

Le profil par defaut est volontairement leger pour une carte de 8 Go :

- `ggml-small-q5_1` pour la transcription francaise (whisper.cpp CUDA) ;
- `qwen3:4b-instruct-2507-q4_K_M`, environ 2,5 Go, pour nettoyer la dictee sans perdre les chemins, commandes ou negations ;
- les deux moteurs tournent l'un apres l'autre pour limiter le pic de VRAM.

### Installation Windows

1. Installe [Ollama pour Windows](https://ollama.com/download/windows), puis
   rouvre PowerShell.
2. Depuis la racine du projet, lance :

```powershell
npm run voice:setup
```

Le script verifie le GPU, telecharge le binaire CUDA officiel de
[whisper.cpp](https://github.com/ggml-org/whisper.cpp), le runtime cuBLAS
officiel NVIDIA, le modele Whisper dans
`%APPDATA%\codex-switch-terminal\voice`, puis execute
`ollama pull qwen3:4b-instruct-2507-q4_K_M`. Redemarre ensuite l'application, ouvre un chat et
clique une fois sur le micro pour parler, puis une seconde fois pour terminer.

Windows ou le navigateur demandera l'autorisation du micro au premier usage.
En cas de refus, reactive **Microphone** pour l'application dans les parametres
de confidentialite Windows. Une interface web ouverte depuis un autre appareil
doit etre servie en HTTPS pour que le navigateur autorise le micro.

### Reglages optionnels

Les valeurs suivantes doivent etre definies avant de lancer l'application ou
`cst-server` :

```powershell
# Plus leger, avec une fidelite moindre sur les autocorrections orales
ollama pull qwen3:1.7b-q4_K_M
$env:CST_VOICE_OLLAMA_MODEL = "qwen3:1.7b-q4_K_M"

# Adresse de l'API Ollama locale si elle a ete personnalisee
$env:CST_VOICE_OLLAMA_URL = "http://127.0.0.1:11434"

# Dossier ou chemins personnalises
$env:CST_VOICE_HOME = "D:\modeles\cst-voice"
$env:CST_VOICE_WHISPER_BIN = "D:\whisper\whisper-cli.exe"
$env:CST_VOICE_WHISPER_MODEL = "D:\modeles\ggml-small-q5_1.bin"
```

Pour diagnostiquer l'acceleration apres une dictee :

```powershell
nvidia-smi
ollama ps
```

La page **Parametres > Voix et GPU** affiche aussi le statut en direct sans
charger de modele : calcul vocal en cours, modele Ollama charge en attente ou
inactif. Elle indique la topologie locale/distante, la VRAM allouee et
l'activite totale rapportee par `nvidia-smi`.

Quand le client desktop est connecte a un VPS, le micro et le moteur vocal du
PC restent prioritaires ; le VPS ne recoit alors que le texte deja transcrit.
Si le moteur local est absent, le client essaie automatiquement le moteur vocal
de `cst-server`. En mode navigateur ou mobile, la transcription et le resume
sont executes par `cst-server` : le serveur doit donc disposer de Whisper et
Ollama, ou etre configure vers les API GPU distantes decrites ci-dessous. Le
telephone ne charge aucun modele.

Le tunnel SSH ouvre l'interface sur `http://127.0.0.1`, origine autorisee pour
le micro par les navigateurs. Un acces direct au VPS depuis un autre appareil
doit obligatoirement utiliser HTTPS ; une page `http://IP_DU_VPS:8080` ne peut
pas obtenir la permission micro.

### GPU distant ou datacenter

Switch peut aussi conserver l'application sur le poste local et deporter les
deux calculs : transcription vers une API compatible OpenAI, reformulation vers
un Ollama distant. Les connexions non locales exigent HTTPS et les jetons Bearer
sont lus uniquement depuis les variables d'environnement.

Le configurateur n'installe aucun modele sur le poste client :

```powershell
npm run voice:remote -- `
  -TranscriptionUrl "https://gpu.example.net/stt/v1/audio/transcriptions" `
  -TranscriptionModel "Systran/faster-whisper-small" `
  -OllamaUrl "https://gpu.example.net/ollama"
```

Le guide [GPU vocal distant](docs/remote-voice-gpu.md) decrit l'architecture,
le deploiement Docker NVIDIA de reference, l'authentification, le repli local et
les variables de configuration.

## Agents autonomes 24/7

La vue **Autonomes** permet de donner un objectif durable, par exemple :

```text
Reduire et optimiser l'utilisation des ressources de la page web, avec des
mesures avant/apres et sans regression fonctionnelle.
```

Chaque agent peut etre cree a la volee avec un nom, un role, un objectif, un
compte, un dossier projet, un planning ou un declencheur, une memoire initiale et une commande de
test. Il conserve ensuite sa conversation, ses souvenirs structures et son
journal. L'agent peut proposer des souvenirs via `AUTONOMOUS_MEMORY:` ; ils sont
dedupliques, limites et reinjectes dans les tours suivants. L'utilisateur peut
aussi ajouter ou supprimer ces souvenirs depuis la carte de l'agent.

Chaque compte rendu public devient aussi un resultat durable dans un historique
borne aux 24 derniers tours : un nouveau passage ne peut donc plus ecraser une
proposition que l'utilisateur n'a pas encore lue. Les resultats non lus sont
prioritaires dans le moniteur, signales dans les navigations desktop et mobile,
et regroupes dans une boite de reception sur la page **Agents autonomes**. Le
bouton **Marquer comme lu** synchronise maintenant cet etat avec le moteur ; le
rapport reste conserve dans l'historique de l'agent. Lors de la migration,
l'ancien `lastSummary` est transforme en rapport afin de remettre en evidence
les resultats deja manques.

Ces comptes rendus sont rediges pour un lecteur humain : ils mettent en avant le
resultat, les changements importants, les blocages, les decisions attendues et
la prochaine action utile. Les identifiants internes, horodatages bruts et
autres metadonnees de pilotage restent dans le moteur, sauf detail indispensable
a une decision ou a une verification humaine.

Un agent peut aussi remettre zero ou plusieurs actions structurees avec
`AUTONOMOUS_PROPOSAL: titre | mission`. Elles sont dedupliquees et conservees
dans le nouvel onglet **Propositions**. Le bouton **Executer** cree directement
un agent dedie avec le meme compte et le meme projet ; la review humaine est
activee pour que la proposition n'autorise jamais, a elle seule, une
modification. Le lien avec l'agent lance est persistant et empeche une double
execution depuis un autre client. Le modele **Radar projet** publie ses idees
actionnables avec ce protocole, et ses anciennes lignes `IDEE:` sont migrees.

Un chat ordinaire peut aussi servir de point de depart : le bouton
**Autonomiser** de son bandeau ouvre une configuration pre-remplie avec le
compte, l'environnement, le dernier objectif et un extrait borne du contexte
recent. La conversation d'origine reste un chat normal et n'est ni effacee ni
verrouillee ; les cycles autonomes utilisent leur propre contexte pour eviter
deux executions concurrentes sur la meme session. Une fois l'agent cree, son
statut reste visible dans le bandeau et ce meme bouton permet de modifier son
nom, son role, son objectif, sa frequence, ses garde-fous, ses connecteurs et sa
commande de validation sans quitter le chat. Si un cycle est deja en cours,
l'editeur propose d'abord une mise en pause explicite.

Sur la version web, le chat dispose aussi nativement des outils MCP
`create_autonomous_agent`, `update_autonomous_agent`, `pause_autonomous_agent`,
`activate_supervisor_general_report` et `apply_autonomous_agent_policy`. Une demande explicite
comme « cree un agent autonome qui surveille les regressions toutes les heures »
conduit donc le modele a creer lui-meme l'agent, sans renvoyer vers le formulaire.
Dans le meme chat, « passe-le a un cycle toutes les deux heures et renomme-le
Sentinelle » modifie directement l'agent lie. « Mets-le en pause » interrompt
son cycle ou sa validation en cours et retire sa prochaine planification ;
l'agent reste en pause jusqu'a sa reprise explicite depuis l'interface. Une
demande repetee reussit sans modifier davantage son etat. Le modele ne recoit
aucun champ permettant de choisir un identifiant arbitraire : une capacite ephemere lie
chaque appel au compte, au dossier et au chat courants, puis le serveur retrouve
l'agent concerne. Les champs non cites conservent leur valeur, et un agent actif
est securise pendant l'ecriture avant de reprendre son activite. Le nouvel etat
apparait automatiquement dans le bandeau de la conversation. Les questions
purement explicatives ne declenchent aucune action.

Depuis n'importe quel chat, « active le compte rendu general du superviseur »
declenche directement le passage correspondant, sans identifiant d'agent. Le
superviseur reserve un lot de comptes rendus non lus, en publie une synthese
unique sous les priorites **critique**, **haute**, **moyenne** et **basse**, puis
marque les sources comme traitees seulement si cette synthese a bien ete
enregistree. L'accuse interne qui confirme les sources traitees est separe du
texte public et n'affiche donc aucun identifiant dans le compte rendu. Un echec conserve le lot non lu pour la tentative suivante ; un
retard superieur a la taille d'un lot est draine par des passages successifs.

Une regle commune peut aussi etre ajoutee depuis n'importe quel chat aux agents
deja actifs qui utilisent la review humaine, meme si ce chat n'a cree aucun
agent. Par defaut, la commande cible uniquement les agents non systeme du meme
compte et du projet courant ; la portee au compte entier doit etre demandee
explicitement. La regle est ajoutee a leur memoire durable sans remplacer leurs
objectifs, roles ou frequences, les anciennes autorisations sont invalidees et
les cycles en cours sont interrompus puis relances proprement. Une politique
visuelle peut ainsi imposer une capture ou maquette fidele avant autorisation,
puis une capture du rendu reel et une comparaison explicite avant de conclure.

Chaque boucle commence aussi par un cycle de pilotage : l'agent decide quelles
informations meritent la memoire durable, segmente l'objectif en domaines et en
taches bornees, reconcilie ce qui est fait avec les preuves disponibles, puis
choisit une seule prochaine tache. Ce carnet de travail structure est persiste
avec la strategie de memoire, les statuts `todo`, `in_progress`, `done`,
`blocked` ou `cancelled`, le domaine, la preuve et l'identifiant de la prochaine
tache. Pour un agent de recherche de bugs, il sert directement de matrice de
couverture : domaines deja testes avec leurs resultats et domaines restant a
tester. Le moteur refuse une conclusion `complete` tant que le carnet est vide
ou contient encore du travail ouvert ; un echec de la validation finale cree
automatiquement une tache de correction prioritaire.

Une premiere etape demarre immediatement, puis le moteur reprend la meme
conversation a l'intervalle choisi. L'etat est ecrit atomiquement dans
`autonomous-agents.json`. Apres un redemarrage, un agent actif est replanifie ;
apres trois echecs consecutifs il passe en **Attention requise** au lieu de
boucler indefiniment. La saturation temporaire du modele est traitee a part :
elle ne penalise pas l'agent et declenche des relances automatiques avec un
delai progressif, plafonne a une minute, jusqu'a ce que le modele reponde. Au
demarrage, un agent precedemment suspendu uniquement pour cette saturation est
egalement repris automatiquement.

Un agent peut aussi utiliser le declencheur **Modification du projet**. Dans ce
mode, aucune premiere etape n'est lancee : le moteur calcule l'empreinte des
fichiers et dossiers relatifs configures, puis l'agent dort sans consommer de
tour. Une nouvelle empreinte doit rester identique pendant le delai de
stabilisation choisi avant de produire un evenement. L'agent se reveille alors,
recoit la cause du reveil et les chemins surveilles, traite cette occurrence,
puis se rearme et se rendort apres `AUTONOMOUS_STATUS: complete` et la validation
eventuelle. Un redemarrage conserve cet etat de veille ; seul un travail
interrompu est relance immediatement.

Le modele suggere **Publieur du build** configure ce mode pour `src`, `public`
et les fichiers de configuration frontend. Il valide le projet, construit le
frontend, cree un commit, execute `git push origin HEAD`, publie atomiquement le
build sur le noeud web local avec `npm run deploy:web:local -- -SkipBuild`, puis
execute le smoke test du site `:8080`. Sa validation finale
`npm run verify:published-build` refuse de conclure tant que le depot n'est pas
propre, que `origin/<branche>` ne pointe pas sur le commit local, ou que
`dist/index.html` et ses assets ne sont pas ceux effectivement servis. La cible
peut etre remplacee avec `CST_PUBLISH_SITE_URL`.

Le publieur utilise une fenetre exclusive par projet : tant qu'un autre agent
demarre un tour, travaille ou valide dans le meme dossier, son reveil et son
demarrage restent en attente. Inversement, lorsqu'une publication est prete ou
en cours, le moteur ne demarre pas un nouveau tour concurrent dans ce projet.
Le modele attend aussi 120 secondes de stabilite des fichiers afin de ne pas
publier une sauvegarde intermediaire.

Pour eviter qu'un premier reveil publie des changements preexistants, cette
permission ne peut etre armee que depuis une branche propre possedant un remote
`origin`. La permission **Autoriser le push GitHub
et la publication du site** est distincte du mode Build et n'est activee que
pour ce modele ou lorsqu'elle est cochee explicitement. Elle n'autorise ni
`force push`, ni suppression de branche ou de donnees, ni changement de depot,
ni publication de secrets. Les chemins, le delai, l'objectif, la validation et
cette permission restent modifiables afin de creer d'autres agents
evenementiels.

Des qu'au moins un agent utilisateur est actif ou passe en **Attention
requise**, le moteur cree aussi un **Superviseur des agents autonomes** gere par
le systeme. Il recoit un etat borne de toute la flotte, lit la memoire durable,
le carnet, les preuves, le journal et l'activite directe de chaque agent, lance
un premier controle immediat puis revient toutes les heures. Il compile aussi
tous les nouveaux comptes rendus non lus dans un **compte rendu general** classe
par priorite, visible comme resultat propre du superviseur. Il repare
automatiquement les incoherences de planification, diagnostique les erreurs et
validations echouees, et peut corriger dans le dossier projet les bugs logiciels
confirmes. Apres un redemarrage, le superviseur garde la priorite et les agents
en retard sont repris avec un decalage de dix secondes entre eux, afin d'eviter
une relance simultanee de toute la flotte. S'il prouve qu'un agent tarde a agir, repete le meme travail ou se
perd dans un detail marginal, il inscrit une consigne **Superviseur** dans sa
memoire et planifie une reprise alignee sur l'objectif principal. Une
reorientation forte peut interrompre un tour enlise depuis au moins vingt
minutes, mais jamais un test en cours. Il ne
modifie jamais directement le fichier d'etat, ne contourne aucune review et ne
reprend pas un agent volontairement mis en pause ou termine. Sa configuration
est protegee dans l'interface ; il se met en veille quand aucun agent utilisateur
n'est encore actif. Une pause explicite de tous les agents utilisateur non
termines interrompt aussi son tour courant et le met immediatement en veille,
meme si des comptes rendus non lus restent en attente. Ces comptes rendus sont
conserves et seront compiles apres la reprise d'un agent.

Lorsqu'une commande de test est configuree (par exemple `npm test && npm run
build`), une declaration `complete` ne suffit pas : le moteur execute la
commande dans le dossier projet, capture stdout/stderr et applique le timeout
choisi. Un succes valide l'objectif. Un echec est memorise et provoque une
nouvelle etape de correction ; trois validations echouees suspendent l'agent.
Le bouton **Tester maintenant** permet de lancer la meme validation a la
demande. Les formes courantes de secrets presentes dans la sortie sont masquees
avant persistance ; une commande de test ne doit malgre tout jamais imprimer de
credentials.

Un agent utilisant un compte **Codex** peut recevoir un acces explicite a
**Gmail** et/ou **Google Agenda** depuis sa fiche de creation. Les connecteurs
ne sont jamais actifs par defaut : le moteur desactive toutes les apps Codex
pour le tour autonome, puis ne reactive que celles cochees. Les lectures et
recherches peuvent s'executer seules. Avant un envoi d'e-mail, une creation ou
une modification d'evenement, l'agent doit produire une demande structuree dans
le moniteur ; l'autorisation de l'utilisateur ne vaut que pour l'action decrite
et pour le tour suivant. Les outils marques destructifs restent desactives.

La connexion Google reste geree par Codex et n'est pas stockee dans Codex
Switch Terminal :

1. ouvre Codex avec le meme compte / `CODEX_HOME` que l'agent autonome ;
2. ouvre `/plugins`, installe **Gmail** et/ou **Google Calendar**, puis active le
   plugin ;
3. termine l'authentification Google demandee par le connecteur ;
4. cree l'agent autonome et coche uniquement les services necessaires.

Avec `cst-server`, plugins et authentification doivent etre configures sur
l'hote serveur dans le `CODEX_HOME` du compte selectionne, pas seulement dans
le navigateur client. Les comptes Claude ne chargent pas ces connecteurs
Codex.

### Paiements confirmes par l'utilisateur

Un agent autonome peut maintenant preparer une demande de paiement sans jamais
recevoir de carte, de compte bancaire, de wallet ou de secret. Lorsqu'une
depense est indispensable, il s'arrete et remet une ligne structuree :

```text
AUTONOMOUS_PAYMENT: order-42 | 1299 | EUR | Marchand | Abonnement mensuel | https://checkout.example/pay/order-42
```

Le montant est exprime dans la plus petite unite de la devise (`1299` =
`12,99 EUR`). Le moteur accepte uniquement un checkout HTTPS public, sans
identifiants dans l'URL, refuse les destinations locales ou privees et conserve
un journal borne des demandes. Une reference deja confirmee ne peut pas etre
representee comme un nouveau paiement avec des details differents.

Dans le moniteur, l'utilisateur voit separement le montant, le marchand, la
reference et le domaine, puis utilise un seul bouton **Payer**. L'action
`authorizePayment` est auditee et liee a l'identifiant exact de la demande ;
une vue perimee est refusee. Elle journalise seulement que le checkout a ete
lance, ouvre le navigateur et planifie une verification autonome du recu apres
90 secondes. Le clic n'est jamais transforme en preuve de debit, ne debloque
ni les ecritures Gmail/Agenda ni une review de code et ne permet pas de relancer
le meme paiement. Un refus reste disponible comme action de securite.

Sur Android, chaque agent peut aussi activer **Notifications dans l'app mobile**.
Firebase Cloud Messaging transmet alors ses nouveaux comptes rendus et ses
alertes importantes ; un toucher recharge l'etat du serveur et ouvre directement
le moniteur du bon agent. Le contenu reste masque sur l'ecran verrouille.

Un blocage de paiement peut produire un **handoff mobile** via le meme canal.
La notification affiche uniquement l'agent, le marchand et le
montant ; elle ne contient ni URL de checkout, ni carte, ni secret. Un toucher
ouvre l'app sur l'agent et l'identifiant exact de la demande est reverifie sur
le serveur avant d'afficher le bouton unique **Payer**. Ce clic autorise et
ouvre la page du marchand ; l'utilisateur choisit Google Pay si ce marchand le propose, puis termine
Google Pay, 3D Secure ou toute autre verification dans l'interface du marchand.
L'agent reprend ensuite pour verifier le recu sans supposer que le paiement a
reussi. La notification ne contourne donc jamais le geste utilisateur exige
par le wallet ou la banque. L'activation FCM est decrite dans `android/README.md`.
Sur l'app Android, tout se configure depuis la carte **Notifications mobiles**
des Parametres en collant les deux JSON Firebase ; aucun secret
n'est ensuite reaffiche dans l'interface.
La carte distincte **Compte Google Pay** verifie via l'API Google Pay si un
moyen compatible est deja present sur le telephone et ouvre la gestion
officielle Google Wallet. Codex Terminal ne collecte aucun compte Google ni
aucune donnee de carte ; le compte et le moyen de paiement restent choisis dans
la feuille securisee du marchand.

Ce premier socle est volontairement independant du prestataire et ne declenche
aucun debit cote serveur. Il pourra accueillir ensuite un adaptateur Agentic
Commerce Protocol ou Universal Commerce Protocol avec un gestionnaire de
paiement tokenise et borne, lorsqu'un marchand compatible sera configure, sans
changer le contrat de confirmation humaine ni exposer les donnees de paiement
au modele.

Sur le site web `:8080`, ouvre **Agents autonomes**, puis **Creer un agent
autonome** : les cases **Gmail** et **Google Agenda** sont affichees dans la
section **Services Google**. Le navigateur envoie uniquement la liste des
services autorises ; les outils et la session Google restent executes sur
l'hote `cst-server`.

- En desktop, le moteur reste actif tant que l'application est ouverte.
- Pour un vrai fonctionnement 24/7 independant du navigateur, lance
  `cst-server` avec le service systemd fourni dans
  `deploy/codex-switch-terminal.service` (ou la tache planifiee Windows du
  noeud local).
- Mettre un agent en pause arrete son tour courant. La reprise le replanifie
  immediatement. Depuis la liste, **Tout mettre en pause** suspend en une fois
  les agents actifs ou en attente d'intervention ; lorsque toute la flotte
  utilisateur est suspendue, le superviseur systeme est lui aussi interrompu et
  mis en veille.
- Le moteur n'autorise pas implicitement les actions externes irreversibles :
  l'agent doit demander une intervention humaine lorsqu'une autorisation, un
  secret ou une decision est indispensable.
- La commande de test est une commande shell arbitraire executee avec les
  droits du processus desktop ou `cst-server`. La creation d'un agent qui en
  contient une reste donc reservee aux appels admin et a des utilisateurs de
  confiance.

## Chats orchestres (beta)

La vue **Chats orchestres** est separee des conversations ordinaires et des
agents autonomes 24/7. Elle construit une feature avec une equipe de chats et
un protocole de validation impose :

Dans un chat ordinaire, l'interrupteur **Orchestration auto** du compositeur
active un routage par demande. Le modele traite directement les questions et
les travaux simples ; lorsqu'une realisation gagne vraiment a etre decomposee,
il rend une decision structuree et l'application lance aussitot le chat
orchestrateur et ses workers. Le choix est memorise separement pour chaque chat.
La validation automatique reprend la commande configuree dans la vue orchestree
ou, a defaut, `git diff --check`.

1. un chat orchestrateur inspecte le projet et produit un plan structure ;
2. chaque partie du plan devient un chat travailleur distinct ;
3. chaque travailleur modifie son propre worktree Git et doit soumettre une
   preuve structuree avec les fichiers touches et des tests reussis ;
4. le patch est applique dans le worktree prive de l'orchestrateur, qui le
   relit, le teste et l'accepte ou le renvoie au meme travailleur avec un retour
   actionnable ;
5. une commande de validation choisie par l'utilisateur est executee reellement
   apres chaque acceptation et apres l'audit final ;
6. une fois toutes les taches acceptees, le diff complet est applique au dossier
   source sans creer de commit utilisateur.

Le mode beta exige actuellement un depot Git propre et execute les travailleurs
sequentiellement. Cette contrainte donne a chaque agent une base deterministe et
rend les retours de revue reversibles. Si le commit ou les fichiers du depot
source changent pendant l'execution, la publication est bloquee : le rendu reste
dans le sandbox orchestrateur au lieu d'ecraser le travail intervenu entre-temps.
Les worktrees sont conserves pour inspection jusqu'a la suppression explicite
du chat orchestre.

Une preuve et des tests reduisent le risque de regression, mais ne constituent
pas une garantie mathematique d'absence de tout bug. La commande de validation
est une commande shell arbitraire executee avec les droits de l'application ou
du serveur ; cette fonctionnalite doit rester reservee a des utilisateurs de
confiance.

## RDV Lab (prototype separe)

La vue **RDV Lab** est un assistant conversationnel separe des chats principaux.
Ecris par exemple `Trouve-moi un medecin generaliste a Paris` : il interprete la
demande, lance lui-meme la recherche, affiche les propositions, comprend un
choix comme `le 2`, puis attend un `oui` explicite avant toute reservation.

Le mode **Vrai compte Doctolib** est actif par defaut. A la premiere demande,
l'assistant verifie le profil navigateur persistant et ouvre Chrome si une
connexion ou une double authentification est necessaire. Une fois la session
verifiee, il reprend automatiquement la recherche. Le mode **Bac a sable** reste
disponible pour verifier l'interface : lance `npm run dev`, ouvre
**RDV Lab**, demande un generaliste a Paris dans le chat, garde ou change la
proposition, puis ecris `oui` ou clique **Oui, prendre ce RDV**. Le backend
consomme un jeton a usage unique
et renvoie une reference `LAB-*`; aucun appel de reservation n'est envoye a
Doctolib.

Le mode **Vrai compte Doctolib** est volontairement experimental. Dans l'application
desktop, il s'execute sur le poste. Sur le site `:8080`, il s'execute sur la
machine qui heberge `cst-server` :

1. Node.js, `playwright-core` et Google Chrome doivent etre disponibles.
2. Clique **Connecter Doctolib** et termine toi-meme la connexion, la 2FA ou un
   eventuel CAPTCHA dans la fenetre Chrome visible.
3. Relance la recherche. Le prototype lit les premiers creneaux publics qui
   acceptent de nouveaux patients.
4. Le clic **Oui** autorise uniquement le creneau recapitule. Le worker le
   selectionne, confirme lorsque le parcours ne demande aucun nouveau choix,
   puis exige une page Doctolib confirmee avant d'annoncer un succes.

L'option **Ajouter ensuite a Google Calendar** est active par defaut sur une
proposition reelle et reste visible avant le clic **Oui**. Google Calendar
utilise un second profil Chrome persistant, distinct de Doctolib. Ecris
`Connecte Google Calendar` (ou utilise le bouton), puis termine toi-meme la
connexion et la 2FA dans la page Google officielle. Avant une reservation,
l'assistant impose cette connexion si la synchronisation est cochee. Apres le
clic **Oui**, il ne cree l'evenement qu'une fois la confirmation Doctolib
verifiee, clique **Enregistrer** dans Google Calendar et exige le message de
succes de Google. Le bac a sable ne cree jamais d'evenement reel.

Si Doctolib demande un choix de patient, une reponse medicale ou une nouvelle
authentification, Chrome reste visible sur la machine qui execute le moteur afin
que l'utilisateur intervienne. Le prototype ne contourne jamais ces etapes. Les
creneaux peuvent changer entre la recherche et le clic ; dans ce cas il faut
relancer la recherche. Le bac a sable fonctionne sur desktop, web et mobile. Le
mode reel du site `:8080` suppose que Node.js, Chrome, le worker et une session
graphique sont disponibles sur l'hote ; depuis un telephone, il faut donc avoir
acces a l'ecran de cette machine pour la connexion ou les questions manuelles.

## Serveur web local

### Dashboard Instagram et TikTok (développement)

La pile locale peut ajouter un dashboard **Réseaux sociaux** qui connecte les
comptes via les OAuth officiels Instagram et TikTok, puis trace les vues par jour
et par compte sur un graphique 2D. Le module reste masqué si son sidecar n'est pas
disponible et n'est câblé que dans `compose.yaml`, réservé au développement.

La configuration des secrets, des URI de retour et la sémantique des métriques
sont décrites dans [`social-analytics/README.md`](social-analytics/README.md).

### Règle d'accès (sécurité)

L'URL de développement `https://pc-fixe-cst.tail3a8bdf.ts.net/` (et `:10000`)
est réservée au **VPS Azure** : c'est le seul tiers autorisé à s'y connecter.
Tout accès public est interdit sur cette instance — pas de Tailscale Funnel,
pas de port applicatif publié sur Internet. Un accès externe éventuel doit
passer par le tailnet privé avec le VPS Azure comme seul tiers autorisé.

Pour utiliser le PC comme serveur :

```powershell
& ".\Lancer Codex Switch Terminal.cmd" server
```

Le serveur ecoute par defaut sur `127.0.0.1:8080`. Pour l'ouvrir aux appareils
du reseau local :

```powershell
$env:CST_BIND = "0.0.0.0:8080"
& ".\Lancer Codex Switch Terminal.cmd" server
```

Le token administrateur et le PAT Git local sont conserves dans :

```text
%APPDATA%\codex-switch-terminal-server\server.local.env.ps1
```

Sur un noeud Windows distant, le demarreur utilise par defaut le sandbox Codex
`unelevated`. Il conserve l'isolation native sans ouvrir de demandes UAC sur le
PC hote lorsque des chats sont lances via Tailscale. Ce choix ne modifie pas le
sandbox de l'application desktop locale.

Pour autoriser le port 8080 sur le profil reseau prive :

```powershell
& ".\Lancer Codex Switch Terminal.cmd" firewall
```

En mode serveur, une URL de depot est clonee normalement dans le repertoire de
donnees. Les chats et terminaux utilisent ensuite directement ce clone.

## Incident connu documenté : saisie après Chat → Terminal

Le correctif de la perte de clavier lors du passage d'un chat à un terminal web/VPS
est documenté dans [`docs/terminal-focus-vps-fix.md`](docs/terminal-focus-vps-fix.md).
Il protège à la fois le focus xterm après remontage du DOM et les caractères saisis
pendant une reconnexion du PTY. Toute modification future du transport ou du
rendu doit conserver ces deux protections et vérifier les tests ciblés avant
publication.

## Build et tests

La verification commune controle la coherence des versions, les tests frontend
et les tests Rust :

```powershell
npm run verify
```

`npm run build` et `npm run build:server` lancent automatiquement cette
verification avant de supprimer les anciens artefacts et de compiler. Le build
desktop local produit des installateurs non signes et ne demande donc aucune
cle privee. `npm run build:signed` conserve la generation des artefacts de mise
a jour signes lorsque `TAURI_SIGNING_PRIVATE_KEY` est configuree. Les builds
Android et iOS lancent au minimum la verification rapide des versions et du
frontend. Une release GitHub est egalement bloquee avant sa creation si la
verification ou le build frontend echoue ; elle continue d'utiliser la
configuration Tauri signee.

```powershell
npm run build:frontend
npm run build:server
npm run test:frontend
```

Pour construire l'application desktop :

```powershell
npm run build
```

Pour nettoyer tous les artefacts de build :

```powershell
npm run clean
```

## Deploiement serveur

### VPS prive via SSH

Pour choisir une offre gratuite encore valable et creer la VM avec une taille
realiste pour plusieurs chats, suis d'abord le guide
[VPS gratuits](docs/free-vps.md). Il recommande actuellement une instance
Oracle Cloud Ampere A1 Always Free de 2 OCPU et 12 Go, puis reutilise le flux
SSH ci-dessous sans publier le port applicatif.

Le flux recommande utilise Ansible et Docker Compose. Il copie le projet et les
comptes au premier passage, laisse le runtime lie a `127.0.0.1`, puis ouvre le
client desktop au travers d'un tunnel SSH :

```powershell
npm run deploy:vps:portable -- -SshTarget ubuntu@IP_DU_VPS -IdentityFile "$HOME\.ssh\id_ed25519" -NodeId vps-paris
npm run deploy:vps:pool -- -Config config/oracle-vps-pool.json -PreflightOnly
npm run connect:vps -- -Profile vps-paris
npm run connect:vps:pool -- -Profiles vps-paris,vps-secondaire -PrimaryProfile vps-paris
```

L'onglet web **VPS** propose aussi un assistant Google Cloud. Il ouvre la
connexion et l'activation de l'essai sur les pages officielles Google, puis cree
une VM europeenne et y deploie automatiquement la meme pile portable. CST ne
recoit jamais le mot de passe, le MFA ni les informations bancaires.

Le premier deploiement peut aussi enchainer directement la connexion avec
`-Connect`. La cible SSH accepte `root`, ou un utilisateur avec `sudo` sans mot
de passe. Les projets distants sont isoles dans `/srv/cst/workspaces`.

La modale **Nouveau chat** permet aussi d'imposer un noeud, par exemple
**Oracle Free**, ou de garder le choix automatique du VPS sain le moins charge.
La session reste ensuite sur ce VPS. Aucun port applicatif n'est publie sur
Internet. Le guide [Ansible + Docker Compose](docs/portable-vps.md) couvre le
chemin portable et l'image multi-architecture ; le guide
[VPS via SSH](docs/vps-ssh.md) conserve le deploiement natif historique et le
diagnostic du tunnel.

### Installation manuelle

Copie `deploy/cst-server.env.example` vers
`/etc/codex-switch-terminal.env`, puis configure au minimum :

- `CST_PUBLIC_BASE_URL` ;
- `CST_ADMIN_TOKEN` ;
- `CST_GIT_PAT` si des depots prives doivent etre clones.

### Comptes utilisateurs et connexion Google

Le serveur web demande maintenant un compte utilisateur. Au premier lancement,
cree le compte proprietaire depuis l'ecran d'inscription. Les mots de passe sont
haches avec Argon2 et le fichier persistant `user-auth.json` ne contient que des
hashes de mots de passe et de jetons de session. Le secret `CST_ADMIN_TOKEN`
reste reserve aux scripts de maintenance et aux anciens clients techniques.

Les inscriptions sont ouvertes par defaut. Le premier compte reste creatable
meme si elles sont fermees, afin d'initialiser une nouvelle installation. Apres
la creation du compte proprietaire, utilise :

```bash
CST_ALLOW_REGISTRATION=false
```

Cette option bloque aussi la creation d'un nouvel utilisateur via Google ; un
compte Google deja lie continue de fonctionner.

Pour activer Google, cree dans Google Cloud un client OAuth 2.0 de type
**Application Web**, puis ajoute exactement cette URI aux URI de redirection
autorisees :

```text
https://VOTRE_DOMAINE/api/auth/google/callback
```

Configure ensuite le serveur :

```bash
CST_GOOGLE_CLIENT_ID="...apps.googleusercontent.com"
CST_GOOGLE_CLIENT_SECRET="GOCSPX-..."
# Facultatif si CST_PUBLIC_BASE_URL est correcte :
CST_GOOGLE_REDIRECT_URI="https://VOTRE_DOMAINE/api/auth/google/callback"
```

Le bouton Google apparait uniquement lorsque l'identifiant et le secret sont
presents. Le serveur fournit aussi au frontend l'URL de demarrage OAuth associee
a l'origine du callback : un acces direct par `127.0.0.1:8080` bascule ainsi
proprement vers l'URL publique HTTPS avant de contacter Google. Le callback
utilise le flux serveur avec `state`, PKCE et les scopes
`openid email profile`; seules les adresses Google verifiees sont acceptees.
Avec une URL publique HTTPS, les cookies de session recoivent automatiquement
l'attribut `Secure`.

### Messages TikTok de test via TikMatrix

Un chat ou un agent du VPS peut préparer un message pour cinq comptes TikTok
secondaires contrôlés, afficher l'aperçu exact puis placer la campagne confirmée
dans une file persistante. Le client desktop Cloud récupère cette file par sa
connexion authentifiée existante et appelle TikMatrix uniquement sur le
loopback Windows. Aucun port TikMatrix ni identifiant TikTok n'est exposé au
VPS. Le guide [Messages TikTok depuis un chat VPS](docs/tiktok-vps-messaging.md)
décrit les prérequis, les outils et la protection anti-doublon.

Le même connecteur peut demander en une seule opération jusqu'à 1 000
`@username` disponibles dans les followers d'un compte appartenant à
l'utilisateur ou explicitement autorisé. TikTok peut toutefois ne rendre
qu'environ 50 profils visibles : l'application déduplique et retourne le
nombre réellement obtenu, sans présenter le résultat comme exhaustif. Cette
liste n'est jamais transformée automatiquement en campagne de messages.

Pour relier les deux fonctions sans contacter les autres profils collectés, le
chat peut recevoir une liste séparée d'au plus cinq comptes secondaires
contrôlés et un message exact. Après la collecte, il intersecte uniquement
cette liste autorisée avec les résultats et crée un brouillon TikTok. Le chat
affiche alors les destinataires retenus et le message ; une nouvelle
confirmation humaine reste obligatoire avant l'envoi.

### Connexion Microsoft 365 (mail et agenda)

Un utilisateur connecte peut lier une ou plusieurs boites Microsoft 365 depuis
**Mon compte** (la liaison est nominative ; **Parametres** n'affiche qu'un
raccourci). Un chat normal, comme un agent autonome, lit alors ses boites
Outlook et ses agendas, et prepare e-mails et rendez-vous sous forme de cartes :
rien ne part sans un clic de confirmation. Avec plusieurs boites, une boite par
defaut est utilisee sauf indication contraire, et la carte de confirmation
permet de choisir l'expediteur. Un agent autonome ne recoit que ces cinq outils
Microsoft, et seulement s'il a un proprietaire nominatif.

Cote Entra ID, enregistre une application, ajoute une URI de redirection de type
**Web** identique a celle du serveur, cree un secret client et accorde les
permissions Microsoft Graph deleguees `User.Read`, `Mail.Read`, `Mail.Send`,
`Calendars.ReadWrite` et `offline_access`. Configure ensuite le serveur :

```bash
CST_MICROSOFT_CLIENT_ID="00000000-0000-0000-0000-000000000000"
CST_MICROSOFT_CLIENT_SECRET="..."
# Facultatifs : locataire "common" par defaut, redirection deduite de
# CST_PUBLIC_BASE_URL. Hors boucle locale elle doit etre en HTTPS.
CST_MICROSOFT_TENANT_ID="common"
CST_MICROSOFT_REDIRECT_URI="https://VOTRE_DOMAINE/api/microsoft/callback"
```

L'identifiant et le secret vont ensemble : sans les deux, l'integration reste
eteinte et l'interface annonce qu'elle n'est pas configuree sur ce serveur. Les
jetons restent cote serveur dans le `CST_DATA_DIR` du noeud, une liaison ne vaut
donc que pour ce noeud. Le guide [Microsoft 365](docs/microsoft-365.md) detaille
l'inscription Entra, la liaison, la securite du stockage des jetons et les
limites connues.

Lancement manuel :

```bash
export CST_ADMIN_TOKEN="change-me"
export CST_GIT_PAT="ghp_xxx"
export CST_PUBLIC_BASE_URL="http://IP_DE_LA_VM:8080"
./cst-server
```

Le frontend compile doit etre place dans `dist/` a cote du binaire serveur.

### Dockeriser n'importe quel dépôt Git

Un lien Git suffit pour analyser un projet, réutiliser ou générer son
`Dockerfile`, construire une image exportable et, si souhaité, la lancer par
SSH sur un VPS `amd64` ou `arm64` :

```powershell
npm run dockerize:git -- https://github.com/organisation/projet.git
```

Le même flux est sélectionnable dans **Choisir un environnement → Depuis Git / Docker** pour créer et activer directement un nouvel environnement depuis l'URL.

Le déploiement direct utilise `--deploy utilisateur@hote`. Le guide
[Dockerize Git](docs/dockerize-git.md) décrit le paquet portable, les stacks
détectées, les secrets, les monorepos et la construction native sur le VPS.

### Goal persistant dans le TUI Freebuff

Switch installe automatiquement le skill `switch-create-goal` dans chaque home
Freebuff. Dans le TUI, lance-le avec
`/skill:switch-create-goal <objectif>` : le skill utilise les outils MCP
`cst_goal__create_goal`, `cst_goal__get_goal` et `cst_goal__update_goal`.

Un seul goal inacheve peut exister par proprietaire et environnement. Cette
portee reste stable quand Freebuff recree son terminal ou transfere la discussion
vers un autre compte. Le goal est conserve dans `terminal-goals.json` apres la
fermeture du PTY ou le redemarrage du serveur. Le statut `blocked` reste
inacheve : le meme goal doit reprendre puis passer a `complete` avant qu'un
remplacement soit autorise. Le bearer MCP est injecte uniquement dans
l'environnement du processus Freebuff et n'est jamais enregistre dans
`mcp.json`.

Pour un travail logiciel, le skill impose aussi une preuve temporaire couvrant
les cas normaux, limites et d'erreur — jusqu'a quelques milliers de scenarios
generes lorsqu'ils sont deterministes et peu couteux. Tant qu'un test requis
echoue, le goal reste inacheve et le travail continue. Une fois tout valide, le
skill supprime seulement ces tests temporaires puis relance les tests permanents.

## Mobile

Le client web reste installable en PWA. Le depot contient aussi deux coques
natives WebView qui se connectent au meme `cst-server` sans embarquer le moteur
Rust sur le telephone :

- `android/` : Android 7.0+ (API 24), cible Android 16/API 36 ;
- `ios/` : iOS et iPadOS 15+.

Sous Windows, Android Studio et son SDK suffisent pour produire un APK debug
signe et directement installable :

```powershell
npm run test:android
npm run build:android
# -> CodexTerminal-debug.apk
```

Le build Android execute les tests frontend, le lint natif, Gradle et la
verification de signature. La coque gere le clavier, les zones sures, les
fichiers, le micro/camera, les telechargements et un ecran natif de reconnexion.
Le token du pont natif est chiffre avec Android Keystore et les sauvegardes de
l'application sont exclues. Voir [android/README.md](android/README.md).

iOS necessite macOS et Xcode ; voir [ios/README.md](ios/README.md).
