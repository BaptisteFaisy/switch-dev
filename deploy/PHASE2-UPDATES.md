# Phase 2 — CI signée + auto-updaters (serveur & desktop)

Cette phase transforme les mises à jour « build sur chaque machine » en
**artefacts CI signés** consommés par des updaters qui vérifient la signature
avant de basculer.

```
git tag vX.Y.Z ─► .github/workflows/release.yml
                    ├─ build cst-server (linux + windows) ─► .tar.gz/.zip + .sha256 + .minisig
                    └─ tauri-action (windows) ────────────► NSIS/MSI signés + latest.json
                    │
   GitHub Release ◄─┘
        │
        ├─► Nœuds serveur : update-node.sh --release / update-node.ps1 -ReleaseTag
        │       download → vérif SHA-256 + minisign (fail-closed) → bascule atomique → rollback
        │
        └─► App desktop : plugin updater Tauri
                check() → downloadAndInstall() → relaunch()  (vérif signature Ed25519 intégrée)
```

Deux chaînes de signature **indépendantes**, toutes deux Ed25519 :

| Cible | Outil | Clé privée (secret CI) | Clé publique (dans le repo) |
|---|---|---|---|
| Artefacts serveur | **minisign** | `MINISIGN_SECRET_KEY` (+ `MINISIGN_PASSWORD`) | `MINISIGN_PUBKEY` dans les scripts updater |
| App desktop | **tauri signer** | `TAURI_SIGNING_PRIVATE_KEY` (+ `..._PASSWORD`) | `plugins.updater.pubkey` dans `tauri.conf.json` |

> Les clés **publiques** ne sont pas secrètes : on les commite. Les clés
> **privées** ne quittent jamais ta machine / les *Actions secrets*.

---

## 0. Prérequis

- `gh` (GitHub CLI) authentifié sur `BaptisteFaisy/Software-multi-account`.
- L'arbre git **compile** et **toutes les dépendances vendored sont commitées**
  (`src-tauri/vendor/*`, `[patch.crates-io]` de `Cargo.toml`) — sinon le job
  `build-server` de la CI échoue.
- `minisign` installé localement pour générer la clé :
  - Debian/Ubuntu : `sudo apt-get install -y minisign`
  - macOS : `brew install minisign`
  - Windows : `scoop install minisign` **ou** `choco install minisign`

---

## 1. Générer les paires de clés (une seule fois)

### 1a. minisign (artefacts serveur)

```bash
minisign -G -p minisign.pub -s minisign.key      # demande un mot de passe — retiens-le
cat minisign.pub                                  # 2e ligne = la clé publique "RW..."
```

- `minisign.key` (privée) → secret CI `MINISIGN_SECRET_KEY`.
- La 2e ligne de `minisign.pub` (commence par `RW`) → à coller dans les scripts
  updater (voir §3).

### 1b. tauri signer (app desktop)

```bash
# Lancer DEPUIS le dossier du projet ; écrire la clé privée HORS du repo.
npx tauri signer generate -w "$HOME/cst-updater.key"   # demande un mot de passe
# -> écrit la clé privée dans ~/cst-updater.key et AFFICHE la clé publique (base64)
```

- `cst-updater.key` (privée) → secret CI `TAURI_SIGNING_PRIVATE_KEY`.
- La clé publique affichée → `plugins.updater.pubkey` dans `tauri.conf.json` (§4).

---

## 2. Enregistrer les secrets CI

Depuis le dossier du repo (ou avec `-R BaptisteFaisy/Software-multi-account`) :

```bash
gh secret set MINISIGN_SECRET_KEY < minisign.key
gh secret set MINISIGN_PASSWORD --body 'MOT_DE_PASSE_MINISIGN'
gh secret set TAURI_SIGNING_PRIVATE_KEY < cst-updater.key
gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --body 'MOT_DE_PASSE_TAURI'
```

> `GITHUB_TOKEN` est fourni automatiquement par Actions — rien à ajouter.
> Après enregistrement, **supprime** `minisign.key` / `cst-updater.key` de ton
> disque de travail (les secrets sont la source de vérité).

---

## 3. Câbler la clé publique minisign dans les nœuds serveur

La clé publique minisign doit être connue des nœuds pour vérifier les téléchargements.
Deux options (l'une OU l'autre) :

- **Chaîne continue (`deploy-web.yml`)** : commite la clé publique dans
  `deploy/minisign.pub` (une ligne `RW...`, non secrète). Le workflow la lit et
  la passe au nœud ; sans elle, le déploiement échoue si `MINISIGN_SECRET_KEY`
  est configuré. C'est aussi elle qui permet de changer de VPS sans toucher
  aux scripts.
- **Éditer le défaut** dans `deploy/update-node.sh` et `scripts/update-node.ps1` :
  remplace `RWQPLACEHOLDER_REMPLACE_MOI` / la valeur par ta vraie clé publique.
- **Passer à l'exécution** (sans éditer) :
  ```bash
  # Linux
  sudo bash deploy/update-node.sh --release vX.Y.Z --minisign-pubkey 'RW...'
  # ou via l'environnement
  export CST_MINISIGN_PUBKEY='RW...'
  ```
  ```powershell
  # Windows
  scripts\update-node.ps1 -ReleaseTag vX.Y.Z -MinisignPubKey 'RW...'
  # ou:  $env:CST_MINISIGN_PUBKEY = 'RW...'
  ```

Sur Windows, la vérification de signature exige `minisign` sur le `PATH`
(`scoop install minisign`). Sans lui, la mise à jour **échoue** (fail-closed) ;
utilise `-AllowUnsigned` seulement en dépannage conscient.

---

## 4. Câbler l'auto-updater desktop Tauri (patch prêt à appliquer)

> ⚠️ **À appliquer sur un arbre propre** (pas au milieu du co-édit en cours) :
> ces changements touchent le graphe de build Rust/TS. Tant que ce n'est pas
> appliqué, la CI produit quand même des installeurs, mais **non updatables**
> (pas de `.sig` ni `latest.json`).

### 4.1 Ajouter les plugins (fait Cargo.toml + capabilities + package.json)

```bash
npx tauri add updater
npx tauri add process
```

### 4.2 Enregistrer les plugins — `src-tauri/src/lib.rs`

Dans `pub fn run()`, sur le `tauri::Builder`, juste après `tauri::Builder::default()` :

```rust
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(terminal::TerminalManager::default())
        // ... reste inchangé
```

### 4.3 `src-tauri/tauri.conf.json` — clé publique + endpoint + artefacts

```jsonc
{
  // ...
  "bundle": {
    "active": true,
    "targets": "all",
    "icon": ["icons/icon.ico"],
    "createUpdaterArtifacts": true          // <— AJOUT
  },
  "plugins": {                               // <— BLOC AJOUTÉ
    "updater": {
      "pubkey": "COLLE_ICI_LA_CLE_PUBLIQUE_TAURI",
      "endpoints": [
        "https://github.com/BaptisteFaisy/Software-multi-account/releases/latest/download/latest.json"
      ]
    }
  }
}
```

### 4.4 `src-tauri/capabilities/default.json` — permissions

`npm run tauri add` ajoute normalement `updater:default`. Ajoute aussi le process :

```json
  "permissions": [
    "core:default",
    "core:event:allow-listen",
    "core:window:allow-is-fullscreen",
    "core:window:allow-set-fullscreen",
    "updater:default",
    "process:default"
  ]
```

### 4.5 Nouveau module `src/updater.ts`

```ts
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

// Vérifie une mise à jour au démarrage de l'app DESKTOP (no-op en mode
// navigateur/remote). Signature Ed25519 vérifiée par le plugin avant install.
export async function initDesktopUpdater(): Promise<void> {
  if (!("__TAURI_INTERNALS__" in window)) return; // pas dans la webview Tauri
  try {
    const update = await check();
    if (!update) return;
    const ok = window.confirm(
      `Mise à jour ${update.version} disponible (actuelle ${update.currentVersion}).\n\n` +
        `${update.body ?? ""}\n\nInstaller maintenant ? L'application redémarrera.`,
    );
    if (!ok) return;
    await update.downloadAndInstall();
    await relaunch();
  } catch (err) {
    console.error("[updater] échec de la vérification/installation :", err);
  }
}
```

### 4.6 Appeler au démarrage — `src/main.ts`

Ajoute l'import en tête et l'appel après l'initialisation de la plateforme :

```ts
import { initDesktopUpdater } from "./updater";
// ... après initializePlatform()/le bootstrap de l'app :
void initDesktopUpdater();
```

### 4.7 Vérifier localement (arbre propre)

```bash
npm ci
npm run build:frontend        # tsc doit passer (nouveaux imports résolus)
npx tauri build               # doit générer les bundles + les artefacts updater (.sig)
```

---

## 5. Couper une release

1. **Bumper la version aux 3 endroits** (doivent être identiques, = tag sans `v`) :
   - `src-tauri/Cargo.toml` → `version = "X.Y.Z"`
   - `src-tauri/tauri.conf.json` → `"version": "X.Y.Z"`
   - `package.json` → `"version": "X.Y.Z"`
2. Commit, puis tag + push :
   ```bash
   git commit -am "release: vX.Y.Z"
   git tag vX.Y.Z
   git push origin main --tags
   ```
3. La CI crée une Release **brouillon**, y attache les artefacts serveur signés +
   les installeurs desktop + `latest.json`, puis la **publie** (`publish-release`).

---

## 6. Déployer / mettre à jour

### Nœuds serveur (rolling, capacité jamais nulle)

```powershell
# Oracle d'abord puis PC — chacun draine, bascule, vérifie, rollback si besoin.
scripts\rolling-update.ps1 -OracleSshTarget ubuntu@oracle -Port 8080
```

**Chaîne continue (push sur `main`)** : la CI compile et signe en amont ; le
nœud ne compile plus. Deux modes selon le diff du push :

- `--prebuilt /tmp/cst-server-linux-x86_64.tar.gz` : `src-tauri/` a changé,
  binaire CI précompilé + `dist`, vérifié SHA-256 + minisign ;
- `--frontend /tmp/cst-frontend.tar.gz` : push frontend-only, le binaire
  courant est conservé et seul `dist/` est remplacé (bascule quasi
  instantanée).

Les deux modes partagent la même séquence sûre (attente sans drain → lease de
drain → bascule atomique → sonde de santé → rollback).

Ou nœud par nœud, en mode release signée :

```bash
# Oracle
sudo bash deploy/update-node.sh --release vX.Y.Z --minisign-pubkey 'RW...'
```
```powershell
# PC
scripts\update-node.ps1 -ReleaseTag vX.Y.Z -MinisignPubKey 'RW...'
```

> `rolling-update.ps1` et `deploy-oracle-node.ps1` sont aujourd'hui en mode
> *build-on-host* (Phase 1). Pour les faire pointer sur les artefacts signés,
> remplace leur étape de build par un appel `--release vX.Y.Z` (voir §7).

### App desktop

Rien à faire : au prochain lancement, `initDesktopUpdater()` détecte la nouvelle
version via `latest.json`, propose l'installation, vérifie la signature, installe
et redémarre.

---

## 7. Vérification bout-en-bout

- **Release** : `gh release view vX.Y.Z` liste `cst-server-linux-x86_64.tar.gz`
  (+`.sha256`,`.minisig`), `cst-server-windows-x86_64.zip` (+`.sha256`,`.minisig`),
  les installeurs `.exe/.msi` (+`.sig`) et `latest.json`.
- **Vérif manuelle d'un artefact serveur** :
  ```bash
  minisign -Vm cst-server-linux-x86_64.tar.gz -P 'RW...'
  sha256sum -c cst-server-linux-x86_64.tar.gz.sha256
  ```
- **Fail-closed** : altère 1 octet d'un artefact ⇒ `update-node.sh --release`
  doit **refuser** de basculer (SHA-256 ou minisign en échec), le nœud reste sur
  la release courante.
- **Desktop** : installe la N-1, publie la N, relance ⇒ pop-up de mise à jour.

---

## 8. Référence — noms d'artefacts (source de vérité)

Ces noms sont partagés entre la CI (producteur) et les updaters (consommateurs) —
ne les change qu'aux deux endroits à la fois (`.github/workflows/release.yml`,
`env: ASSET_*` ; `update-node.sh` / `update-node.ps1`, `--asset`) :

| Plateforme | Archive | Contenu | Sidecars |
|---|---|---|---|
| Linux x86_64 | `cst-server-linux-x86_64.tar.gz` | `cst-server` + `dist/` | `.sha256`, `.minisig` |
| Windows x86_64 | `cst-server-windows-x86_64.zip` | `cst-server.exe` + `dist/` | `.sha256`, `.minisig` |
| Desktop | `*-setup.exe`, `*.msi` (+ `.sig`), `latest.json` | installeur Tauri | signature Tauri intégrée |

---

## 9. Notes / pièges

- **`latest.json` = release "latest"** : GitHub marque comme *latest* la dernière
  release **non-draft, non-prerelease**. Le job `publish-release` retire le
  brouillon ; ne coche pas *prerelease* pour une vraie diffusion.
- **Cohérence tag ↔ version** : les updaters serveur refusent si
  `tag (sans v) != cst-server --version` (garde-fou contre un mauvais artefact).
- **Le drain est une lease courte en mémoire** (20 s par défaut, 60 s maximum) :
  un updater interrompu ne peut pas laisser le nœud verrouillé. Le restart
  rouvre également le nœud immédiatement.
- **CST_GIT_COMMIT** : injecté depuis `${{ github.sha }}` en CI (le `.git` n'est
  pas présent au runtime des nœuds).
