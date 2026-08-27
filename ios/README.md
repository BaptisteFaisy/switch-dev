# Codex Terminal pour iOS et iPadOS

Ce dossier contient une application UIKit native minimale pour iPhone et iPad.
Elle charge l'interface servie par `cst-server` dans une `WKWebView`, comme le
client Android existant, sans dupliquer le moteur Rust sur le telephone.

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

## Fonctionnalites

- iOS/iPadOS 15 ou version ulterieure.
- Interface plein ecran avec prise en charge des encoches, de la barre d'accueil,
  des rotations et du clavier logiciel.
- JavaScript, stockage DOM, cookies et WebSocket pour le terminal interactif.
- Pont `window.CstIOS` compatible avec `src/platform.ts`.
- URL conservee dans `UserDefaults` et token admin conserve dans le trousseau
  iOS avec l'accessibilite `WhenUnlockedThisDeviceOnly`.
- Manifeste de confidentialite Apple declarant l'usage local de `UserDefaults`
  (`CA92.1`), sans suivi ni collecte declaree par la coque native.
- Ecran natif de reconnexion/configuration si le serveur est inaccessible.
- Liens externes ouverts dans Safari ; seules les navigations du serveur restent
  dans l'application.
- HTTPS impose pour un serveur public. HTTP n'est accepte que pour une adresse
  locale, declaree via `NSAllowsLocalNetworking`.

Le serveur par defaut est configure dans
`CodexTerminal/Info.plist`, cle `CSTServerURL` :

```text
https://azure-duello.tail3a8bdf.ts.net
```

## Prerequis

- Un Mac avec Xcode 15 ou plus recent.
- Un compte Apple gratuit pour installer sur son propre iPhone, ou un abonnement
  Apple Developer pour TestFlight/App Store.
- Tailscale actif sur l'iPhone si l'URL utilise le domaine prive `*.ts.net`.
- `cst-server` accessible et son token admin.

Xcode et le SDK iOS n'existent pas sous Windows : le projet peut etre developpe
et valide statiquement ici, mais le binaire `.app`/`.ipa` doit etre compile et
signe sur macOS.

Si tu ne possedes qu'un iPad, la web app installable et le build Xcode distant
GitHub Actions sont detailles dans [`IPAD-SANS-MAC.md`](IPAD-SANS-MAC.md). La web
app ne demande ni Mac ni abonnement Apple.

## Tester dans le simulateur

Depuis la racine du depot, sur le Mac :

```bash
bash scripts/build-ios.sh simulator
```

Ou ouvre `ios/CodexTerminal.xcodeproj`, choisis un simulateur puis lance avec
`Cmd+R`.

## Installer sur un iPhone ou iPad

1. Ouvre `ios/CodexTerminal.xcodeproj` dans Xcode.
2. Selectionne la cible **CodexTerminal**, puis **Signing & Capabilities**.
3. Choisis ton equipe Apple et, si necessaire, remplace le Bundle Identifier
   `com.codexswitch.terminal` par une valeur unique.
4. Branche l'appareil, active le mode Developpeur iOS si Xcode le demande, puis
   selectionne-le comme destination.
5. Lance avec `Cmd+R`.
6. A la premiere ouverture, colle le token admin dans l'ecran web de connexion.

Pour creer une archive signee en ligne de commande :

```bash
DEVELOPMENT_TEAM=ABCDE12345 \
PRODUCT_BUNDLE_IDENTIFIER=com.tondomaine.codexterminal \
bash scripts/build-ios.sh archive
```

L'archive apparait dans `ios/build/CodexTerminal.xcarchive` et dans Xcode
Organizer, d'ou elle peut etre envoyee vers TestFlight/App Store Connect.

## Securite reseau

Utilise de preference `Tailscale Serve` ou un reverse proxy HTTPS. Le token admin
ne doit pas transiter en clair sur Internet. L'exception ATS du projet est limitee
au reseau local ; elle n'autorise pas globalement les connexions HTTP.

Documentation Apple :

- [WKWebView](https://developer.apple.com/documentation/webkit/wkwebview)
- [NSAllowsLocalNetworking](https://developer.apple.com/documentation/bundleresources/information-property-list/nsapptransportsecurity/nsallowslocalnetworking)
- [Configuration de l'icone](https://developer.apple.com/documentation/xcode/configuring-your-app-icon)
