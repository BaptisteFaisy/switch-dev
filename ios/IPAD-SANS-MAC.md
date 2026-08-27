# Utiliser Codex Terminal avec seulement un iPad

Il existe deux resultats differents :

1. **Utiliser l'application sur l'iPad maintenant** : installe la web app depuis
   Safari. Aucun Mac, aucune licence et aucun fichier IPA ne sont necessaires.
2. **Verifier que le client natif compile** : lance GitHub Actions depuis l'iPad.
   GitHub loue automatiquement un Mac le temps du build Xcode.

## Solution recommandee : installer la web app

La web app utilise la meme interface et le meme serveur que les clients Android
et iOS. Elle dispose d'une icone, s'ouvre sans barre d'adresse et prend en compte
les encoches, le clavier et les rotations.

### Cote serveur

Le serveur doit etre allume et accessible depuis l'iPad. La configuration deja
utilisee par le client Android convient : Tailscale sur le serveur et sur l'iPad,
avec de preference une URL HTTPS `*.ts.net`.

Sur le PC serveur :

```powershell
& ".\Lancer Codex Switch Terminal.cmd" server
```

### Sur l'iPad

1. Installe et connecte **Tailscale** si le serveur est dans le tailnet prive.
2. Ouvre **Safari** et visite l'URL HTTPS de `cst-server`, par exemple
   `https://azure-duello.tail3a8bdf.ts.net`.
3. Connecte-toi avec le token admin.
4. Touche **Partager**, puis **Plus** si necessaire.
5. Choisis **Sur l'ecran d'accueil**.
6. Active **Ouvrir comme app web**, puis touche **Ajouter**.

La procedure officielle est egalement decrite dans le
[guide Apple pour iPad](https://support.apple.com/fr-fr/guide/ipad/ipad8f1f7a29/ipados).

L'icone **Codex Terminal** apparait sur l'ecran d'accueil. Le terminal reste une
application client : le PC ou le serveur distant doit rester allume pour executer
les commandes.

## Compiler le client natif avec GitHub Actions

Le fichier `.github/workflows/ios.yml` utilise un runner `macos-15` equipe de
Xcode. Il lance tous les tests, construit la web app, compile la cible iOS pour le
simulateur puis publie un ZIP pendant sept jours.

Une fois les fichiers commits et pousses sur GitHub :

1. Ouvre le depot GitHub dans Safari ou l'app GitHub sur l'iPad.
2. Ouvre l'onglet **Actions**.
3. Selectionne **iOS - Build distant**.
4. Touche **Run workflow**, choisis la branche puis confirme.
5. Ouvre l'execution ; lorsque tout est vert, l'artefact se trouve en bas de la
   page sous **Artifacts**.

Cet artefact est compile pour le simulateur. Il sert de preuve de compilation et
ne peut pas etre installe sur l'iPad.

## Limite Apple pour une vraie application native gratuite

Un iPad ne contient ni le SDK iOS ni Xcode et ne peut donc pas compiler lui-meme
le projet. Pour installer gratuitement une build native sur un appareil physique,
Apple demande une signature `Personal Team` geree par Xcode et un profil valable
sept jours. GitHub Actions ne peut pas transformer proprement cette signature
personnelle en installation sur ton iPad sans lui confier des identifiants et des
elements de provisioning.

Ne place jamais le mot de passe de ton compte Apple dans le depot ou dans un
workflow. Les services de signature IPA trouves au hasard sur Internet peuvent
recuperer tes identifiants ou utiliser des certificats revoques.

En pratique, sans Mac local :

- utilise la web app depuis l'ecran d'accueil pour travailler normalement ;
- utilise GitHub Actions pour verifier automatiquement le code natif ;
- le jour ou tu veux TestFlight/App Store, ajoute une signature de distribution
  Apple au workflow avec des secrets GitHub dedies.
