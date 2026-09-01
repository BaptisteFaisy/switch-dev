---
name: publier-video-reseaux
description: Publier une vidéo (mp4) générée par le Studio IA ou fournie par l'utilisateur sur TikTok, Instagram (Reels/publication) ou YouTube, en pilotant l'application du téléphone Android branché en USB à Switch et en laissant l'utilisateur choisir le compte et l'audience dans l'app. Use when the user asks to post, publish, upload, partager, publier, poster ou mettre en ligne une vidéo sur les réseaux sociaux ("poste cette vidéo sur TikTok", "mets ça sur Insta", "upload sur YouTube"), including across multiple platforms, whether or not a phone is mentioned. C'est de la télécommande assistée : chaque action est proposée à partir d'une capture d'écran fraîche et confirmée par l'utilisateur ; l'agent ne choisit jamais le compte, l'audience ni la visibilité.
---

# Publier une vidéo sur TikTok, Instagram ou YouTube

Publier un contenu **appartient à l'utilisateur ou à une entité qu'il est autorisé à représenter**, depuis un téléphone Android qu'il possède, branché en USB à Switch, à travers le pont USB borné. C'est une télécommande assistée du téléphone : l'utilisateur garde le contrôle de ce qui part en ligne, compte par compte, étape par étape. Ce n'est ni un bot de publication, ni un outil d'engagement.

## Préconditions

- N'agir qu'après une demande explicite dans la conversation ou une action d'interface directe de l'utilisateur.
- Identifier **exactement une vidéo** avant toute action : soit le lien de téléchargement d'une vidéo terminée du Studio IA (l'utilisateur le colle dans la conversation), soit un chemin de fichier `.mp4` local, soit une URL `https://` pointant vers un fichier vidéo. En cas d'ambiguïté, demander ; ne jamais deviner entre plusieurs vidéos.
- Choisir **exactement un chemin d'exécution** : les outils MCP du chat Switch ou l'assistant terminal borné Freebuff. Ne jamais soumettre la même action par les deux chemins.
- Lister les appareils d'abord. N'utiliser que l'identifiant exact d'un appareil Android retourné par l'inventaire courant ; ne jamais deviner un numéro de série. Si plusieurs appareils éligibles sont retournés, exiger que l'utilisateur en choisisse un, ne jamais choisir le premier automatiquement.
- N'utiliser que les actions structurées documentées ci-dessous. Ne jamais activer un accès shell brut, ni appeler `adb` ou `scrcpy` directement, ni modifier les garde-fous du pont USB, ni contourner les vérifications de propriété, de bail, d'approbation ou de confirmation.
- Le **transfert du fichier vidéo sur le téléphone** utilise l'action `push_file` du pont : elle copie un fichier du poste (où tourne le pont) vers un chemin absolu de l'appareil (`adb push`). Si l'appareil n'annonce pas `push_file` dans ses capacités, ou si le transfert échoue, retomber sur le transfert humain (téléchargement du lien de la vidéo dans le navigateur du téléphone, câble USB ou Drive). Le skill vérifie ensuite par capture d'écran que la vidéo est visible dans le sélecteur de l'application cible.
- Ne pas deviner les noms de paquets. `open_app` ne reçoit qu'un `appId` exact confirmé par `info` ou accepté explicitement par l'utilisateur. Les identifiants ci-dessous sont des candidats usuels à confirmer, pas des valeurs sûres :
  - TikTok : `com.zhiliaoapp.musically`
  - Instagram : `com.instagram.android`
  - YouTube : `com.google.android.youtube`
  - YouTube Studio : `com.google.android.apps.youtube.creator`

### Chemin MCP du chat Switch

1. Appeler `list_control_devices` avant de cibler un Android.
2. Appeler `control_device` avec la forme exacte `{ "deviceId": "...", "action": "...", "args": { ... }, "confirmed": true|false }`. Omettre `args` quand l'action n'en a pas. Mettre `confirmed` à `true` pour une action qui change l'état uniquement après la confirmation explicite de l'utilisateur pour cette seule action.
3. Si le résultat est `queued` ou `claimed`, appeler `get_control_device_action` avec l'`actionId` retourné jusqu'à un état terminal. Ne pas resoumettre l'action en attendant.

### Chemin terminal Freebuff

- Le pont terminal fournit `CST_DEVICE_API_URL`, `CST_DEVICE_TOKEN`, `CST_DEVICE_HELPER=cst-device` et `CST_SERVER_BIN`. Exiger les quatre non vides et vérifier que le helper est appelable via `PATH`. L'URL API parsée doit être en HTTP(S), cibler exactement `localhost` ou une IP de boucle locale, utiliser le chemin exact `/api/device-fleet`, et ne contenir ni nom d'utilisateur, ni mot de passe, ni requête, ni fragment. Ne jamais afficher le jeton, le chemin du binaire serveur ni l'environnement complet. Si une vérification échoue, expliquer que le pont USB de Switch n'est pas prêt et s'arrêter.
- Lister l'inventaire courant avec `"$CST_DEVICE_HELPER" list` en shell POSIX ou `& $env:CST_DEVICE_HELPER list` en PowerShell.
- La seule grammaire valide est `cst-device list`, `cst-device action DEVICE_ID ACTION [ARGS_JSON] [--confirm] [--idempotency-key KEY]` et `cst-device status ACTION_ID`. `ARGS_JSON`, quand présent, est un seul objet JSON. Les deux drapeaux peuvent entourer cet objet, mais chaque drapeau ne peut apparaître qu'une fois.
- Avant la première soumission de chaque action Freebuff, créer une clé d'idempotence ASCII neuve, la passer avec `--idempotency-key` et la conserver jusqu'à l'état terminal. Une nouvelle tentative manuelle après un échec réseau ambigu doit réutiliser exactement la même clé, le même identifiant d'appareil, la même action, le même objet JSON et le même drapeau de confirmation. Ne jamais générer une nouvelle clé pour une nouvelle tentative de la même action.

## Modèle d'interaction

Une action utilisateur autorise au plus une action d'entrée qui change l'état correspondante. Les captures d'écran et l'inventaire en lecture seule ne consomment pas cette autorisation. Une demande telle que « ouvre l'écran d'upload » produit une navigation ; elle n'autorise pas une boucle, une file, un macro, un minuteur ou une session en arrière-plan.

1. Prendre une capture d'écran courante et décrire brièvement l'état visible.
2. Énoncer l'action unique proposée et sa cible. Une capture d'écran illisible, partielle, périmée ou visuellement ambiguë n'autorise aucune entrée ; demander à l'utilisateur de résoudre lui-même.
3. Exécuter uniquement l'action demandée par le chemin MCP ou terminal choisi, avec `confirmed: true` / `--confirm`.
4. Reprendre une capture d'écran pour vérifier le résultat. En cas de doute, s'arrêter plutôt que d'essayer des coordonnées voisines.

Le pont supporte exactement `info`, `screenshot`, `open_screen`, `tap`, `swipe`, `type_text`, `key_event`, `open_app`, `shell` et `push_file`. Seuls `info` et `screenshot` sont en lecture seule. Toute autre action exige `confirmed: true` en MCP ou `--confirm` avec le helper Freebuff. **Ce skill ne doit jamais invoquer `shell`** : le transfert de fichier se fait par l'action `push_file` du pont, jamais par un shell sur l'appareil.

Exemples Freebuff conformes au vrai contrat du helper (POSIX) :

```sh
"$CST_DEVICE_HELPER" action "$DEVICE_ID" info --idempotency-key "$IDEMPOTENCY_KEY"
"$CST_DEVICE_HELPER" action "$DEVICE_ID" screenshot --idempotency-key "$IDEMPOTENCY_KEY"
"$CST_DEVICE_HELPER" action "$DEVICE_ID" open_app '{"appId":"EXACT_PACKAGE"}' --confirm --idempotency-key "$IDEMPOTENCY_KEY"
"$CST_DEVICE_HELPER" action "$DEVICE_ID" tap '{"x":120,"y":340}' --confirm --idempotency-key "$IDEMPOTENCY_KEY"
"$CST_DEVICE_HELPER" action "$DEVICE_ID" swipe '{"startX":120,"startY":700,"endX":120,"endY":240,"durationMs":350}' --confirm --idempotency-key "$IDEMPOTENCY_KEY"
"$CST_DEVICE_HELPER" action "$DEVICE_ID" type_text '{"text":"EXACT_TEXT"}' --confirm --idempotency-key "$IDEMPOTENCY_KEY"
"$CST_DEVICE_HELPER" action "$DEVICE_ID" push_file '{"localPath":"C:\\Users\\moi\\Videos\\ma-video.mp4","remotePath":"/sdcard/Pictures/ma-video.mp4"}' --confirm --idempotency-key "$IDEMPOTENCY_KEY"
"$CST_DEVICE_HELPER" status "$ACTION_ID"
```

Ne pas se rabattre sur une autre commande quand le helper borné ou un outil MCP rejette une action.

## Choix de la plateforme, du compte et de l'audience

- Proposer à l'utilisateur de choisir **une plateforme à la fois** (TikTok, Instagram ou YouTube). S'il demande plusieurs plateformes, publier l'une après l'autre et reprendre une confirmation complète pour chacune.
- **Le compte est toujours choisi par l'utilisateur dans l'application.** Sur l'écran de publication, le compte actif s'affiche (avatar). L'agent peut proposer le tap sur ce sélecteur de compte, mais l'utilisateur choisit lui-même le compte dans la liste qui s'ouvre. L'agent ne sélectionne jamais un compte, ne crée jamais de compte et ne saisit jamais d'identifiants ni de mot de passe. Si le compte voulu n'est pas connecté dans l'app, le dire et demander à l'utilisateur de se connecter manuellement.
- **L'audience et la visibilité sont des choix de l'utilisateur.** TikTok (« tout le monde / amis / privé »), Instagram (public/abonnés) et YouTube (Publique/Non répertoriée/Privée/Planifiée) : l'agent peut proposer le tap sur chaque option affichée, mais c'est l'utilisateur qui choisit l'option. Ne jamais changer ces réglages par défaut sans confirmation.

## Transfert automatique de la vidéo (`push_file`)

Quand la vidéo n'est pas encore sur le téléphone :

1. **Obtenir le fichier sur le poste.** Si la vidéo est une URL `https://` (lien Studio IA), la télécharger dans un fichier temporaire du poste avec `curl -L -o` (chemin local absolu, nom `.mp4`). Vérifier que le fichier existe et a une taille plausible avant tout transfert.
2. **Vérifier que l'appareil annonce `push_file`** dans les capacités retournées par `list`/`info`. Sinon, dire à l'utilisateur que le transfert automatique n'est pas disponible sur cet appareil et passer au transfert humain.
3. **Pousser le fichier** : `push_file` avec `localPath` (chemin absolu du fichier sur le poste) et `remotePath` (chemin absolu sur l'appareil commençant par `/`). Choisir la destination selon l'écran où la vidéo doit apparaître : `/sdcard/Pictures/<nom>.mp4` pour la galerie (Instagram, TikTok, YouTube), `/sdcard/Download/<nom>.mp4` pour les sélecteurs de fichiers (YouTube Studio). Proposer la commande exacte `adb push <localPath> <remotePath>`, attendre la confirmation explicite de l'utilisateur, puis exécuter avec `--confirm` / `confirmed: true`.
4. **Vérifier** par capture d'écran dans le sélecteur de l'app cible que la vignette apparaît (attendre quelques secondes après le push, le temps que la galerie scanne le dossier). Si la vidéo n'apparaît pas, proposer le repli humain (téléchargement du lien dans le navigateur du téléphone) ou une autre destination (`/sdcard/Download/`), jamais un `shell`.

## Flux général de publication

Préparer puis vérifier chaque étape par capture d'écran ; les interfaces changent souvent, donc **se laisser guider par les captures, jamais par des coordonnées mémorisées**.

1. **Avoir la vidéo sur le téléphone.** Soit elle y est déjà (l'utilisateur le confirme), soit la transférer via `push_file` (section ci-dessus), soit la transférer à la main (lien Studio IA à télécharger dans le navigateur du téléphone, câble, Drive). Vérifier ensuite, par `open_app` + captures, que la vidéo est visible dans le sélecteur de l'app cible.
2. **Ouvrir l'application** : `open_app` avec le paquet confirmé, puis capture.
3. **Rejoindre l'écran de publication** selon la plateforme (voir ci-dessous), une action à la fois.
4. **Sélectionner la vidéo** dans le sélecteur (galerie) : tap unique proposé puis confirmé ; la capture doit montrer la vignette et la durée. Si la durée dépasse les limites de la plateforme ou que le format est manifestement inadéquat (par ex. très long pour TikTok/Reels), prévenir l'utilisateur avant de continuer.
5. **Rédiger la légende / le titre** : `type_text` avec le texte exact fourni ou validé par l'utilisateur (légende du Studio IA, accroche du carrousel, ou texte que l'utilisateur dicte/colle). Garder des textes courts (légende ≤ 2 000 caractères, titre YouTube ≤ 90 caractères) et vérifier par capture que le champ a bien reçu le texte.
6. **Vérifier le compte affiché** sur l'écran de publication : proposer l'ouverture du sélecteur de compte, laisser l'utilisateur choisir, puis reprendre une capture.
7. **Publier** : décrire l'écran final (aperçu, légende, compte, audience), proposer le tap sur le bouton de publication (« Publier », « Partager », « Téléverser »), attendre la confirmation explicite de l'utilisateur pour **cette** action, puis exécuter.
8. **Vérifier** par une dernière capture : message de succès ou présence de la vidéo dans le profil. Si le résultat est incertain, s'arrêter et demander. Ne jamais republier sans une nouvelle instruction explicite.

### TikTok

- Bouton « + » central (créer) → choisir « Importer » ou « Envoyer » → sélecteur de la galerie.
- Après sélection : écran d'édition avec le champ légende ; vérifier l'avatar du compte en haut (sélecteur de compte le cas échéant) ; bouton « Publier » en bas à droite.
- Ne jamais toucher aux réglages « engagements », « remarque », ni programmer une publication sans demande explicite.

### Instagram

- Bouton « + » en haut (ou au centre selon version) → onglet « Reels » pour une vidéo courte ou « Publication » pour une vidéo de flux → galerie.
- Après sélection : « Suivant » (filtres/cover) → « Suivant » (partage) → champ légende, vérification du compte en haut (« Votre compte » / avatar), bouton « Partager ».
- Une vidéo longue n'est pas acceptée en Reels : le signaler à l'utilisateur si l'app refuse le fichier.

### YouTube

- Préférer **YouTube Studio** (`com.google.android.apps.youtube.creator`) avec son bouton « Téléverser », sinon l'app YouTube : « + » (créer) → « Téléverser une vidéo ».
- Sélecteur de fichier (galerie ou « Parcourir ») → écran de détails : titre (type_text), description (type_text si demandé), **visibilité laissée à l'utilisateur**, chaîne vérifiée en haut (« La vidéo sera visible sur … ») → « Publier ».

## Limites d'engagement

- Publier uniquement la vidéo explicitement désignée par l'utilisateur. Ne jamais choisir du contenu à aimer, commenter, suivre, partager, enregistrer ni supprimer de son propre chef.
- Ne jamais simuler un comportement humain : pas de délais aléatoires, pas de motifs de gestes, pas de durées de visionnage ou de rythmes d'action destinés à ressembler à un être humain.
- Ne jamais affirmer que l'activité est indétectable, sûre vis-à-vis de l'application des règles, ni conforme du simple fait qu'une personne a lancé la session.
- La publication assistée reste équivalente à une télécommande manuelle : pas de session sans surveillance, pas de publication en boucle, pas de file d'attente multi-comptes.

## Conditions d'arrêt

S'arrêter immédiatement quand l'utilisateur le demande, que l'appareil se déconnecte, que l'appareil sélectionné change de propriétaire, ou que l'interface montre un CAPTCHA, une limite de débit, un défi de sécurité, un mur de connexion, une invite de paiement, une restriction de compte, une boîte de dialogue inattendue ou une cible incertaine. Ne pas résoudre ni contourner un défi.

Tenir uniquement un journal d'actions minimal en mémoire contenant l'horodatage, un alias aléatoire opaque créé pour l'appareil dans la session courante, l'autorisation de l'utilisateur, l'action demandée et le succès ou l'échec. Ne jamais persister le journal ni enregistrer de mots de passe, de jetons, d'identifiants d'appareil, de numéros de série bruts, de légendes ou de captures d'écran. Ne pas envoyer de captures d'écran ni de journaux.