# Plan de remédiation progressive des infractions de structure

> Objectif : ramener la baseline du garde-fou (`scripts/structure-guard.baseline.json`) à **vide**, sans casser le travail des autres agents, en respectant les règles AGENTS.md (≤ 30 fonctions/fichier, ≤ 50 lignes/fonction, ≤ 500 lignes/fichier).
>
> Données : audit du 2026-08-31 (`scripts/code-structure-audit.mjs`, exécuté à la main). **Les chiffres bougent au fil des commits et des refactors en cours des autres agents** — le plan fixe l'ordre et la méthode, pas les valeurs exactes. Ce document n'est pas une mesure figée ; mettez-le à jour quand une vague s'achève.

## 1. État des lieux (audit 2026-08-31)

- **82 fichiers dans `scripts/structure-guard.baseline.json`** ; l'audit live momentané remonte **~47–50 fichiers en infraction** (le travail des autres agents fait bouger ce nombre en continu ; il était de 122–135 au 2026-08-30).
- Répartition par type d'infraction (audit live 2026-08-31) :
  - **fichiers > 500 lignes uniquement** : la majorité des `partNN.rs` récemment créés (`autonomous/part11–14`, `chat/part03/08/11/14`, `discussions/part04/07/12/16`, `settings/part16`) et plusieurs monolithes TS non encore touchés.
  - **trop de fonctions uniquement** : `src/android-control.ts` (42), `src-tauri/src/chat_model_tools.rs` (61), `src-tauri/src/pool.rs` (46)…
  - **multiples règles à la fois** : `src/main.ts`, `src/platform.ts`, `src-tauri/src/tiktok_messaging.rs`, `src-tauri/src/microsoft.rs`, `src/chat/autonomous.ts`…
- La refactorisation **Rust est très avancée** : la plupart des monolithes sont désormais des **dossiers** (`account_usage/`, `orchestration/`, `gmail/`, `auth/`, `autonomous/`, `chat/`, `settings/`, `work_time/`, `terminal/`, `video_generation/`, `metrics/`, `creative_accounts/`, `discussions/`, `microsoft/`, `chat_model_tools/`, `device_fleet/`, `freebuff_cloud/`, `doctolib_lab/`, `duello_bank/`, `forum/`, `git_docker_environment/`, `mobile_push/`, `provider/`, `referral/`, `server/`, `telegram_notifications/`, `whatsapp_notifications/`, `workspace_access/`). Les gros `.rs` de premier niveau **restant à découper** :
  - `src-tauri/src/tiktok_messaging.rs` — 4 418 l, ~82 fns dont 17 > 50 l
  - `src-tauri/src/voice.rs` — 2 093 l
  - `src-tauri/src/pool.rs` — 1 153 l, ~46 fns
  - `src-tauri/src/devices.rs` — 690 l
  - `src-tauri/src/image_generation.rs` — 681 l
  - `src-tauri/src/private_messages.rs` + `private_messages_part01…08.rs` — split partiel déjà en cours

### Top des fichiers en infraction (audit 2026-08-31)

| Fichier | Fns | Fns > 50 l. | Lignes |
|---|---|---|---|
| `src/main.ts` | 2075 | — | 31 887 |
| `src/platform.ts` | 158 | — | 2 997 |
| `src-tauri/src/tiktok_messaging.rs` | 82 | 17 | 4 418 |
| `src-tauri/src/pool.rs` | 46 | 3 | 1 153 |
| `src-tauri/src/microsoft.rs` | (→ dossier `microsoft/`) | 9 | — |
| `src-tauri/src/chat_model_tools.rs` | (→ dossier `chat_model_tools/`) | 11 | — |
| `src-tauri/src/voice.rs` | — | 2 | 2 093 |
| `src-tauri/src/devices.rs` | — | — | 690 |
| `src-tauri/src/image_generation.rs` | — | — | 681 |
| `src/android-control.ts` | 42 | — | >500 |
| `src/scheduled-chats.ts` | 43 | 2 | >500 |
| `src/chat/autonomous.ts` | — | — | >500 |
| `src-tauri/src/device_fleet.rs` | (→ dossier `device_fleet/`) | — | — |

`src/main.ts` concentre l'essentiel de l'excès (2 075 fonctions, 31 887 lignes) : chantier à part, traité dans **Vague 4** et suivi précisément dans `docs/main-split-plan.md` (tranches 1–6 « faites » ; il ne doit plus **jamais** s'aggraver).

## 2. Priorisation : 4 critères, dans l'ordre

1. **Risque de collision avec les autres agents** — on évite les fichiers activement modifiés (`src/main.ts`, `src/platform.ts`). Ils restent en fin de plan.
2. **Ratio effort/impact** — un fichier à 1–2 infractions simples sort vite et fait tomber la baseline d'un cran ; on enchaîne les « quick wins » d'abord.
3. **Stabilité du module** — un module déjà découpé en sous-fichiers (`src-tauri/src/<dossier>/partNN.rs`, `src/<dossier>/`) a un pattern de split éprouvé : reprendre le même schéma est peu risqué.
4. **Dépendances** — un fichier très importé (`src/platform.ts`, `src/main.ts`) casse beaucoup de choses quand on le touche : on le traite quand son entourage est stabilisé.

## 3. Découpage par modules (ordre de traitement)

### ✅ Vague 1 — Quick wins mécaniques — **TERMINÉE**

Fichiers à **1 seule** infraction simple. Méthode : déplacer les blocs thématiques vers `partNN.rs` / `partNN.ts` dans le même dossier (pattern `partNN` en place partout), puis `npm run guard:structure:update`. **Livrable atteint** : baseline passée de ~135 à ~50 fichiers.

### ✅ Vague 2 — Fichiers « moyens » multi-infractions — **TERMINÉE**

Fichiers avec 2 types d'infractions, hors monolithes : `scripts/`, fichiers Rust moyens (`auth`, `voice`, terminaux, métriques…), `tests/` et `social-analytics/` — extractions des helpers/fixtures partagées. **Livrable atteint** : baseline ≈ 47–82 fichiers restants (selon l'instant de mesure).

### 🔶 Vague 3 — Monolithes Rust actifs — **PRESQUE TERMINÉE**

Statut au 2026-08-31 (la plupart sont devenus des **dossiers**) :

| Cible planifiée | Statut |
|---|---|
| `account_usage.rs` (80 fns) → `account_usage/` | ✅ dossier créé |
| `orchestration.rs` (79 fns) → `orchestration/` | ✅ dossier créé |
| `gmail.rs` → `gmail/` | ✅ dossier créé |
| `auth.rs` → `auth/` | ✅ dossier créé |
| `microsoft.rs` (108 fns) → `microsoft/` | 🔶 dossier créé, `microsoft/` reste lourd à vérifier |
| `chat_model_tools.rs` (61 fns) → `chat_model_tools/` | 🔶 dossier créé, reste lourd |
| `device_fleet.rs` → `device_fleet/` | ✅ dossier créé |
| `pool.rs` (46 fns) → **pas encore** | ❌ monolithe intact (1 153 l) |
| `tiktok_messaging.rs` (82 fns, 17 longues) → **pas encore** | ❌ monolithe intact (4 418 l) |
| `scripts/skins-table-browser.mjs` (73 fns), `scripts/smoke-site.mjs` (64 fns) | ❌ à découper par scénario |

**À faire pour clore la vague 3** : `tiktok_messaging.rs`, `pool.rs`, `voice.rs`, `devices.rs`, `image_generation.rs`, et les scripts `.mjs` énumérés ci-dessus.

### 🔶 Vague 4 — `src/main.ts` et `src/platform.ts` — **EN COURS**

`src/main.ts` = **2 075 fonctions / 31 887 lignes** aujourd'hui (était 2 594 / 40 176 en début de remédiation) : **impossible en un PR**, et activement modifié par les autres agents. Le découpage est piloté par **`docs/main-split-plan.md`** (cartographie des domaines + tranches, état des extractions : tranches 1–6 « faites », domaine stats entièrement sorti). `src/platform.ts` (158 fns / 2 997 l) suit la même méthode après `main.ts`.

**Livrable vague 4** : baseline vide (0).

## 4. Objectif « baseline vide » : jalons mesurables

Le garde-fou (`npm run guard:structure`) est un ratchet : la baseline ne peut que se resserrer. État au 2026-08-31 :

| Jalon | Contenu | Baseline | Statut |
|---|---|---|---|
| J0 (2026-08-30) | Plan rédigé, baseline initiale | ~122–135 | ✅ fait |
| J1 | Vague 1 terminée (quick wins) | → ~55 | ✅ fait |
| J2 | Vague 2 terminée (fichiers moyens) | → ~47–82 | ✅ fait |
| J3 | Vague 3 terminée (monolithes Rust/scripts) | → ≤ 10 | 🔶 en cours |
| J4 | `main.ts`/`platform.ts` extraits | **0 (vide)** | ⏳ |

Commande de suivi : `node scripts/structure-guard.mjs 2>&1 | tail -1` ou, pour l'audit live complet, `node scripts/code-structure-audit.mjs`.

## 5. Règles de conduite pendant la remédiation

1. **Un PR = un fichier** (ou un groupe de fichiers du même module) ; jamais un méga-PR de refactor.
2. Après chaque fichier remis en conformité : `npm run guard:structure:update` **dans le même commit** (la baseline se resserre, le hook pré-commit reste vert pour les autres agents).
3. **Jamais de `--strict`** tant que la baseline n'est pas vide (il bloquerait tous les commits).
4. Les fichiers `partNN.rs` déjà créés restent la cible : on ne réinvente pas un nouveau schéma de découpage.
5. Avant de toucher un fichier signalé « en cours par un autre agent » (commits récents dessus), vérifier `git log --follow -5 -- <fichier>` et annoncer dans le canal partagé.
6. Tout refactor passe d'abord les tests du module (`npm test -- <module>` / `cargo test -p <crate>`) avant le `guard:structure:update`.

## 6. Risques et parades

| Risque | Parade |
|---|---|
| Collision avec un autre agent sur un fichier en cours | Vague 4 seulement après coordination ; `git log --follow -5` avant chaque fichier |
| Régression fonctionnelle lors d'un split Rust | `cargo test` + tests d'intégration du module avant commit |
| Baseline re-gonflée par un ajout des autres agents | C'est le rôle du hook pré-commit : il bloque, l'agent doit extraire avant de committer |
| `main.ts` ré-écrit entre 2 extractions | Ré-exports ES : les zones extraites restent importables depuis `main.ts` |
| Perte de vue de l'avancement | Tableau des jalons (section 4) mis à jour à chaque vague terminée |

## 7. Coordonnées des déplacements récents (tests de vue)

Depuis le début de la remédiation, plusieurs symboles référencés par les tests ont été déplacés de `src/main.ts` vers des modules ES dédiés. Les tests de vue (`.mjs`) doivent pointer vers la **localisation courante**, pas celle d'origine. Tableau de référence :

| Symbole | Avant (obsolète) | Maintenant | Assertions concernées |
|---|---|---|---|
| `AppView` (union des vues `\| "…view…"`) | `src/main.ts` (parfois inliné `type AppView = …`) | `src/types.ts` (`export type AppView = …`) | `assert.match(source, /\| "view"/)` et variantes `type AppView =[^;]*\| "view"/s` |
| Déclarations de modules lazy (`type XModule = typeof import("./x")`, `import("./x")`, le champ `…ModulePromise`) | `src/main.ts` | `src/lazy-modules-a.ts`, `src/lazy-modules-b.ts`, `src/lazy-modules-c.ts` | `type XModule = …`, `XModulePromise = import…`, `id="xToggle"`, `case "x":` |
| `\| "design"` + `DesignModule`/`designModulePromise` | `src/main.ts` | `src/lazy-modules-c.ts` | tests `design-tab` |
| Fonctions pures `-data`/`-ui` (formateurs, binding) | modules `-data`/`-ui` importés sans extension | mêmes modules, mais **imports sans extension à résoudre** pour `node --test` via le hook `tests/_resolve-ts.hook.mjs` | chaines d'imports `.ts` en direct |

Règles pour toute correction d'un test de vue :

1. **AppView** : l'appartenance d'une vue à l'union se vérifie sur `src/types.ts` (`const types = readFileSync(new URL("../src/types.ts", import.meta.url), "utf8")`), jamais sur `main.ts`.
2. **Lazy modules** : les déclarations `type XModule`/`XModulePromise`/maints `id="xToggle"` et `case "x":` vivent dans `src/lazy-modules-*.ts` — lire le module lazy du fichier pour ces assertions et garder `main.ts` pour le reste. Quand plusieurs fichiers lazy sont impliqués, concaténer les `readFileSync` (pattern du test `design-tab`).
3. **Contenus Rust** découpés en `partNN` : lire via `tests/_read-rust-module.mjs` (`readRustModule`/`rustModulePath`) plutôt qu'un `readFileSync(…src/<name>.rs…)`.
4. Après retarget d'un test, relancer le fichier seul (`node --import ./tests/_register-ts.mjs --test tests/<nom>.test.mjs`).