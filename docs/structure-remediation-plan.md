# Plan de remédiation progressive des infractions de structure

> Objectif : ramener la baseline du garde-fou (`scripts/structure-guard.baseline.json`) à **vide**, sans casser le travail des autres agents, en respectant les règles AGENTS.md (≤ 30 fonctions/fichier, ≤ 50 lignes/fonction, ≤ 500 lignes/fichier).
>
> Données : audit du 2026-08-30 (`tmp/audit-data.json`, snapshot de `scripts/code-structure-audit.mjs`). Les chiffres bougent au fil des commits des autres agents — le plan fixe l'ordre et la méthode, pas les valeurs exactes.

## 1. État des lieux (audit 2026-08-30)

- **122–135 fichiers en infraction** selon l'instant du scan (le travail des autres agents bouge en continu).
- Répartition par type d'infraction :
  - **30 fichiers** violent uniquement `fileLines > 500` → *correctifs mécaniques rapides*
  - **38 fichiers** n'ont que des fonctions > 50 lignes (dont 4 fonctions à extraire max) → *correctifs ciblés*
  - **~24 fichiers** ont seulement trop de fonctions → *découpage par domaine*
  - **30 fichiers** violent les 3 règles à la fois → *gros chantiers, à planifier*

### Top 10 des pires fichiers (sévérité cumulée)

| Fichier | Fns | Fns > 50 l. | Lignes |
|---|---|---|---|
| `src/main.ts` | 2594 | 53 | 40 176 |
| `src/platform.ts` | 139 | 18 | 3 031 |
| `src-tauri/src/microsoft.rs` | 108 | 9 | 3 405 |
| `src-tauri/src/tiktok_messaging.rs` | 82 | 17 | 4 419 |
| `src-tauri/src/orchestration.rs` | 79 | 15 | 4 867 |
| `src-tauri/src/account_usage.rs` | 80 | 5 | 2 293 |
| `scripts/skins-table-browser.mjs` | 73 | 7 | 1 336 |
| `src-tauri/src/chat_model_tools.rs` | 61 | 11 | 3 389 |
| `scripts/smoke-site.mjs` | 64 | 6 | 2 178 |
| `src/prompts.ts` | 69 | 4 | 1 043 |

`src/main.ts` concentre à lui seul ~65 % de l'excès total de fonctions (2 594 fonctions, 40 176 lignes) : c'est un chantier à part, traité en dernier car il est activement modifié par les autres agents.

## 2. Priorisation : 4 critères, dans l'ordre

1. **Risque de collision avec les autres agents** — on évite les fichiers activement modifiés (`src/main.ts`, `src/platform.ts`, `src-tauri/src/microsoft.rs` sont en flux). On les place en fin de plan.
2. **Ratio effort/impact** — un fichier à 1–2 infractions simples sort vite et fait tomber la baseline d'un cran ; on enchaîne ces « quick wins » d'abord.
3. **Stabilité du module** — un module déjà découpé en sous-fichiers (`src-tauri/src/autonomous/partNN.rs`, `src/chat/partNN.rs`) a un pattern de split éprouvé : reprendre le même schéma est peu risqué.
4. **Dépendances** — un fichier très importé (`src/platform.ts`, `src/main.ts`) casse beaucoup de choses quand on le touche : on le traite quand son entourage est stabilisé.

## 3. Découpage par modules (ordre de traitement)

### Vague 1 — Quick wins mécaniques (~2–3 jours, ~35 fichiers)

Fichiers à **1 seule** infraction simple. Aucun choix d'architecture, que de l'extraction.

- **30 fichiers > 500 lignes uniquement** (liste exacte dans `tmp/audit-data.json`, top : `src-tauri/src/device_fleet.rs` 4 886 l., `telegram_notifications.rs` 2 082 l., `src/chat/autonomous.ts` 965 l., `src/keyboard-shortcuts.ts` 563 l.).
  - Méthode : déplacer les blocs thématiques vers `partNN.rs` / `partNN.ts` dans le même dossier (le pattern `partNN` existe déjà partout : `autonomous/part01..14`, `chat/part03..14`, `discussions/part04..16`), puis `npm run guard:structure:update`.
- **38 fichiers à fonctions > 50 lignes uniquement** (top : `src/user-auth.ts` 4 fonctions trop longues, `src-tauri/src/device_terminal.rs` 3, `src/chat/voice.ts` 3).
  - Méthode : extraire les sous-blocs en helpers privés dans le même fichier ou un `*.helpers.ts` voisin ; ≤ 50 lignes par fonction extraite.

**Livrable vague 1** : baseline réduite d'environ 60 % (≈ 135 → ~55 fichiers).

### Vague 2 — Fichiers « moyens » multi-infractions (~1 semaine, ~25 fichiers)

Fichiers avec 2 types d'infractions (fonctions + lignes, ou fonctions + fonctions trop longues), hors top-10.

- `scripts/` : `smoke-admin-responsive.mjs`, `measure-open-ui-baseline.mjs`, `doctolib-lab-worker.mjs`, `agent-screen-core.mjs` — découpage par scénario/étape.
- `src-tauri/src/` fichiers moyens : `auth.rs`, `voice.rs`, `ios_wda.rs`, `private_messages.rs`, `terminal.rs`, `metrics.rs`, `video_generation.rs`, `work_time.rs`, `whatsapp_notifications.rs` — split par domaine métier (déjà amorcé via `partNN` pour autonomous/chat/discussions/settings).
- `tests/` et `social-analytics/` : extraire les fixtures/étapes partagées dans des helpers.

**Livrable vague 2** : baseline ≈ 25–30 fichiers restants.

### Vague 3 — Les « monstres » Rust/TS actifs (~2 semaines, 10 fichiers)

Les 7 gros fichiers Rust + leurs homologues TS, **coordonnés avec les autres agents** (les annoncer dans le canal partagé avant de commencer) :

1. `src-tauri/src/account_usage.rs` (80 fns) → modules `account_usage/{usage,quota,reporting}.rs`
2. `src-tauri/src/tiktok_messaging.rs` (82 fns, 17 fns trop longues) → `tiktok_messaging/{session,send,parse,media}.rs`
3. `src-tauri/src/orchestration.rs` (79 fns, 15 tl) → `orchestration/{plan,execute,schedule}.rs`
4. `src-tauri/src/microsoft.rs` (108 fns) → `microsoft/{auth,mail,calendar,graph}.rs`
5. `src-tauri/src/chat_model_tools.rs` (61 fns, 11 tl) → `chat_model_tools/{registry,dispatch,schemas}.rs`
6. `src-tauri/src/gmail.rs`, `pool.rs` → split par responsabilité
7. `scripts/skins-table-browser.mjs` (73 fns), `scripts/smoke-site.mjs` (64 fns) → un module par scénario

**Livrable vague 3** : baseline ≈ 3–5 fichiers.

### Vague 4 — `src/main.ts` et `src/platform.ts` (chantier dédié, ~2–3 semaines)

`src/main.ts` = 2 594 fonctions / 40 176 lignes : **impossible en un PR**, et activement modifié par les autres agents.

Stratégie :
1. **Cartographier** les 2 594 fonctions par domaine (grep des sections, exports, appelants) → produire `docs/main-ts-map.md`.
2. **Geler par zones** : convenir avec les autres agents d'une zone à la fois « en refonte » (ils s'abstiennent, on s'abstient du reste).
3. **Extraire zone par zone** en modules ES (`src/main/{chat,devices,settings,...}.ts`) avec ré-exports depuis `main.ts` pour ne rien casser ; un PR par zone, guard `--update` après chaque.
4. `src/platform.ts` (139 fns) suit la même méthode, après `main.ts` (beaucoup de ses appelants vivent dans main).

**Livrable vague 4** : baseline vide.

## 4. Objectif « baseline vide » : jalons mesurables

Le garde-fou (`npm run guard:structure`) est un ratchet : la baseline ne peut que se resserrer. Jalons de suivi (nombre de fichiers dans `scripts/structure-guard.baseline.json`) :

| Jalon | Contenu | Baseline restante | Échéance indicative |
|---|---|---|---|
| J0 (aujourd'hui) | Plan rédigé, baseline actuelle | ~122–135 | 2026-08-30 |
| J1 | Vague 1 terminée (quick wins) | ≤ 55 | +1 semaine |
| J2 | Vague 2 terminée (fichiers moyens) | ≤ 30 | +2 semaines |
| J3 | Vague 3 terminée (monstres Rust/scripts) | ≤ 5 | +1 mois |
| J4 | `main.ts`/`platform.ts` extraits | **0 (vide)** | +6–8 semaines |

Commande de suivi : `node scripts/structure-guard.mjs 2>&1 | tail -1` ou compter les clés de `scripts/structure-guard.baseline.json`.

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

Depuis l'audit du 2026-08-30, plusieurs symboles référencés par les tests ont été déplacés de `src/main.ts` vers des modules ES dédiés. Les tests de vue (`.mjs`) doivent pointer vers la **localisation courante**, pas celle d'origine. Tableau de référence :

| Symbole | Avant (obsolète) | Maintenant | Assertions concernées |
|---|---|---|---|
| `AppView` (union des vues `\| "…view…"`) | `src/main.ts` (parfois inliné `type AppView = …`) | `src/types.ts` (`export type AppView = …`) | `assert.match(source, /\| "view"/)` et variantes `type AppView =[^;]*\| "view"/s` |
| Déclarations de modules lazy (`type XModule = typeof import("./x")`, `import("./x")`, le champ `…ModulePromise`) | `src/main.ts` | `src/lazy-modules-a.ts`, `src/lazy-modules-c.ts` (et variantes `-b`, `-d…`) | `type XModule = …`, `XModulePromise = import…`, `id="xToggle"`, `case "x":` |
| `| "design"` + `DesignModule`/`designModulePromise` | `src/main.ts` | `src/lazy-modules-c.ts` | tests `design-tab` |
| Fonctions pures `-data`/`-ui` (formateurs, binding) | modules `-data`/`-ui` importés sans extension | mêmes modules, mais **imports sans extension à résoudre** pour `node --test` via le hook `tests/_resolve-ts.hook.mjs` | chaines d'imports `.ts` en direct |

Règles pour toute correction d'un test de vue :

1. **AppView** : l'appartenance d'une vue à l'union se vérifie sur `src/types.ts` (`const types = readFileSync(new URL("../src/types.ts", import.meta.url), "utf8")`), jamais sur `main.ts`.
2. **Lazy modules** : les déclarations `type XModule`/`XModulePromise`/maints `id="xToggle"` et `case "x":` vivent dans `src/lazy-modules-*.ts` — lire le module lazy du fichier pour ces assertions et garder `main.ts` pour le reste. Quand plusieurs fichiers lazy sont impliqués, concaténer les `readFileSync` (pattern du test `design-tab`).
3. **Contenus Rust** découpés en `partNN` : lire via `tests/_read-rust-module.mjs` (`readRustModule`/`rustModulePath`) plutôt qu'un `readFileSync(…src/<name>.rs…)`.
4. Après retarget d'un test, relancer le fichier seul (`node --import ./tests/_register-ts.mjs --test tests/<nom>.test.mjs`).
