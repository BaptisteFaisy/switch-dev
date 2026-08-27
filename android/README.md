# Codex Terminal — coque Android (WebView)

Application Android native legere qui affiche l'interface web de **Codex Switch
Terminal** servie par `cst-server` (via Tailscale, en HTTPS tailnet). Toute la
logique metier reste dans le web (xterm.js + le SPA), tandis que la coque gere
les integrations et la recuperation propres a Android.

## Environnements Switch

La copie sur le **SSD Samsung T7** et l'instance locale **pc-fixe** sont la
version de **développement**. La version **stable / production** est celle
hébergée sur le **VPS Microsoft Azure**. Les mots « stable », « production »
et « prod » ne désignent jamais la copie SSD/locale.

## Regle d'acces (securite)

L'URL `https://pc-fixe-cst.tail3a8bdf.ts.net/` est reservee au **VPS Azure** :
c'est le seul tiers autorise a s'y connecter. Aucun acces public ne doit etre
ouvert sur cette instance (pas de Tailscale Funnel, pas de port publie sur
Internet) ; un acces externe passe par le tailnet prive avec le VPS Azure comme
seul tiers autorise.

## Ce que fait l'app

- Charge par défaut le serveur stable du VPS Microsoft Azure à l'URL
  `https://azure-duello.tail3a8bdf.ts.net/?cst-chunk-build=azure-20260826`
  (`res/values/strings.xml` → `server_url` et `server_start_url`).
- Lors de la première ouverture de cette version, une installation qui utilisait
  encore l'ancienne cible par défaut `pc-fixe-cst…` est migrée vers Azure. Une
  URL réellement personnalisée n'est pas remplacée.
- Active JavaScript + DOM storage, cookies first-party et WebSocket `wss://`.
- Clavier virtuel : `adjustResize` + `interactive-widget` → le terminal reste
  visible pendant la frappe.
- **Coder dans les terminaux sur mobile** : un script injecté par la coque
  (`assets/cst-mobile-terminal-helper.js`) ajoute une barre compacte sur une
  seule ligne au-dessus du clavier virtuel dès qu'un terminal est affiché —
  Échap, Tab, Ctrl/Alt one-shot (prochaine touche, désarmement automatique),
  les quatre flèches, Ctrl+V (collage natif) et Ctrl+C. Un
  toucher sur le terminal focalise le champ xterm et ouvre le clavier ; chaque
  touche est envoyée directement au xterm ciblé via son événement `input`, sans
  traverser les raccourcis globaux de l’interface ; un `KeyboardEvent`
  synthétique sert seulement de repli avec un ancien WebView. La barre absorbe
  aussi les gestes tactiles afin qu’un appui
  ne puisse pas atteindre l’onglet Chats placé dessous. La barre se cache
  automatiquement quand aucun terminal n'est visible ou qu'une modale est
  ouverte, et le pont natif `showKeyboard` / `hideKeyboard` pilote le clavier
  virtuel (chemin `WindowInsetsController` sur API 30+).
- Android 16/API 36, zones systeme/encoches, tablettes, rotation et retour
  predictif pris en charge.
- Bouton **Retour** = revenir dans l'historique de la WebView, puis quitter.
- Pont natif `CstAndroid` (`getBaseUrl` / `getToken` / `setConfig` /
  `openSettings` / `openPaymentSettings` / `openGooglePaySettings` /
  `openExternalHttpsUrl` / `consumePaymentHandoff` /
  `consumeAutonomousAgentHandoff`). Le token **n'est pas** embarque
  dans l'APK : il est chiffre par une cle AES-GCM non exportable d'Android
  Keystore.
- Notifications facultatives des agents autonomes via Firebase Cloud Messaging :
  les nouveaux comptes rendus et alertes ouvrent directement le bon agent ; les
  validations de paiement ouvrent la demande exacte sans transporter l'URL du
  checkout ni une donnee de carte.
- Ecran natif **Reessayer / Configurer** si le serveur ou Tailscale est
  indisponible ; la connexion reste aussi modifiable dans Parametres.
- Selection de fichiers, micro/camera WebRTC, telechargements HTTPS et exports
  `blob:` vers le selecteur de documents Android.
- HTTPS strict, contenu mixte et cookies tiers bloques, Safe Browsing active,
  sauvegardes cloud et transferts de donnees exclus.

## Prérequis (déjà présents sur ce poste)

- **Android SDK** : `%LOCALAPPDATA%\Android\Sdk` (build-tools 36.1.0, platform
  android-36, platform-tools).
- **JDK 21** : fourni par Android Studio (`…\Android Studio\jbr`).
- **Gradle 8.9** : le *wrapper* (`gradlew`) est déjà généré dans ce dossier.

## Activer les notifications mobiles FCM

L'app se construit et fonctionne sans Firebase. Pour recevoir les comptes
rendus d'agents, leurs alertes importantes et les handoffs Google Pay, 3D Secure
ou autre verification humaine :

1. Dans un projet Firebase, enregistre l'application Android release
   `com.codexswitch.terminal` et, pour l'APK de developpement,
   `com.codexswitch.terminal.debug`.
2. Telecharge le `google-services.json` de cette app, active Firebase Cloud
   Messaging HTTP v1, puis cree une cle JSON pour un compte de service autorise
   a envoyer les messages FCM.
3. Dans Codex Terminal Android, ouvre **Parametres > Notifications mobiles**.
   Colle le contenu de `google-services.json`, puis celui du compte
   de service, et touche **Verifier et enregistrer**.
4. Accepte l'autorisation Android de notifications. L'app extrait elle-meme le
   projet, l'App ID, la cle API publique et le Sender ID ; aucun rebuild de
   l'APK n'est necessaire pour une premiere configuration.
5. Rouvre cette carte et touche **Tester** : le serveur authentifie vraiment le
   compte de service, appelle FCM HTTP v1 et envoie une notification privee a
   chaque appareil enregistre.

Pendant la saisie, Android bloque les captures d'ecran. Le compte de service
est envoye directement au serveur via la connexion HTTPS et le token admin,
sans passer par la WebView. Le serveur valide que les deux JSON ciblent le meme
projet, ne conserve que les champs Android utiles et stocke la cle dans
`mobile-push-config.json` avec des permissions `0600` sur Unix. L'API ne
reaffiche ensuite jamais la cle privee.

Pour une installation administree, la configuration par fichier reste
possible et prioritaire : place la cle hors du depot, puis utilise
`GOOGLE_APPLICATION_CREDENTIALS` et, facultativement,
`CST_FIREBASE_PROJECT_ID`. Le `google-services.json` de build reste egalement
accepte dans `app/src/debug/`, `app/src/release/` ou `app/`. Ces chemins sont
ignores par Git.

Dans la creation ou l'edition d'un agent, active **Notifications dans l'app
mobile**. Chaque nouveau compte rendu publie alors un push ; une suspension pour
review ou apres plusieurs echecs utilise une alerte distincte. Un toucher recharge
la liste depuis le serveur et ouvre le moniteur de l'agent identifie par UUID.
Le contenu complet reste masque sur l'ecran verrouille.

Le client enregistre son identifiant d'installation Firebase directement
aupres du serveur avec le token admin garde dans Android Keystore. Le serveur
stocke cet identifiant dans ses donnees d'execution et supprime les
installations devenues invalides. La notification de verrouillage reste privee
et ne contient que le nom de l'agent, le marchand, le montant et deux UUID de
handoff. Au toucher, le SPA recharge les agents depuis le serveur et exige que
l'UUID du paiement soit encore exactement celui de la demande en attente.

Google Pay n'est pas appele par l'agent : le checkout s'ouvre dans le navigateur
systeme et l'utilisateur choisit Google Pay seulement si le site marchand le
propose. Les validations Google Pay/banque restent donc dans le parcours du
marchand. Dans Codex Terminal, un seul bouton **Payer** verifie l'identifiant,
le montant, le domaine et l'etat encore en attente, journalise l'autorisation,
puis ouvre ce checkout. L'agent reprend apres 90 secondes pour rechercher le
recu ou l'etat de la commande ; le lancement du navigateur n'est jamais traite
comme la preuve que le debit a reussi.

## Configurer le compte Google Pay

Dans **Parametres > Compte Google Pay**, l'app interroge l'API Google Pay en
environnement de production avec `existingPaymentMethodRequired`. Le statut
indique si un moyen de paiement compatible est deja enregistre sur le compte
actif du telephone. **Gerer dans Google Wallet** ouvre l'app officielle ou, si
elle n'est pas installee, `https://wallet.google.com/` pour ajouter, verifier,
modifier ou retirer une carte. **Actualiser** relance ensuite le controle.

Codex Terminal ne lit et ne stocke ni adresse Google, ni mot de passe, ni numero
de carte, ni cryptogramme, ni jeton Google Pay. Il n'est pas possible de figer
un compte ou une carte pour tous les sites externes : chaque marchand definit
les cartes acceptees et la feuille Google Pay laisse l'utilisateur choisir le
compte et le moyen de paiement au moment du checkout.

## Construire l'APK

Le plus simple, depuis la racine du repo :

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-android-apk.ps1
```

Le script regle `JAVA_HOME`/`ANDROID_HOME`, lance les tests frontend, le lint et
`gradlew assembleDebug`, verifie la signature, puis copie l'APK a la racine du
repo **et** sur le Bureau
(`CodexTerminal-debug.apk`).

À la main :

```powershell
$env:JAVA_HOME = "C:\Program Files\Android\Android Studio\jbr"
$env:ANDROID_HOME = "$env:LOCALAPPDATA\Android\Sdk"
cd android
./gradlew.bat assembleDebug
# -> app/build/outputs/apk/debug/app-debug.apk
```

## Installer sur le téléphone

**Option A — transfert de fichier (le plus simple)**
1. Copie `CodexTerminal-debug.apk` sur le téléphone (câble, Drive, etc.).
2. Ouvre-le depuis le gestionnaire de fichiers ; autorise « installer des
   applis inconnues » pour cette source si demandé.

**Option B — ADB (débogage USB activé)**
```powershell
& "$env:LOCALAPPDATA\Android\Sdk\platform-tools\adb.exe" install -r CodexTerminal-debug.apk
```

## Première utilisation

1. Assure-toi que **Tailscale est actif** sur le téléphone (même hors du réseau
   du PC : 4G + Tailscale suffit).
2. Ouvre **Codex Terminal**. L'écran de connexion s'affiche, l'URL du serveur
   est pré-remplie.
3. Colle le **token admin** → *Se connecter*. Il est mémorisé pour les fois
   suivantes.

## Notes

- **APK debug** : signé avec la clé de debug d'Android, donc installable
  directement (pas destiné au Play Store). Pour une version signée de
  distribution, générer une keystore (`keytool`) et un build `assembleRelease`.
- **`applicationId`** en debug : `com.codexswitch.terminal.debug` (suffixe
  `.debug`), il peut donc cohabiter avec une future version release
  `com.codexswitch.terminal`.
- **HTTPS uniquement** (`usesCleartextTraffic=false`). Le certificat `*.ts.net`
  de Tailscale est reconnu par Android (chaîne Let's Encrypt), aucune config
  réseau supplémentaire n'est nécessaire.
