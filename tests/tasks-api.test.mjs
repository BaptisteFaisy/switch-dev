import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const tasks = read("../src-tauri/src/tasks.rs");
const server = read("../src-tauri/src/server.rs");
const auth = read("../src-tauri/src/auth.rs");
const lib = read("../src-tauri/src/lib.rs");
const platform = read("../src/platform.ts");
const sync = read("../src/tasks-sync.ts");
const taskModel = read("../src/tasks.ts");
const taskView = read("../src/tasks-view.ts");
const main = read("../src/main.ts");

const block = (source, start, end) => {
  const from = source.indexOf(start);
  const to = source.indexOf(end, from + start.length);
  assert.ok(from >= 0, `debut introuvable: ${start}`);
  assert.ok(to > from, `fin introuvable: ${end}`);
  return source.slice(from, to);
};

// ─────────────────────────── 1. Modèle serveur (tasks.rs) ───────────────────────────

test("le stockage serveur est scindé par propriétaire et sérialisé en camelCase", () => {
  assert.match(tasks, /const STORE_FILE: &str = "tasks\.json"/);
  assert.match(tasks, /const STORE_VERSION: u32 = 1/);
  assert.match(tasks, /struct TaskItem \{/);
  assert.match(tasks, /#\[serde\(rename_all = "camelCase"\)\]/);
  assert.match(tasks, /pub struct TaskItem/);
  assert.match(tasks, /pub id: String/);
  assert.match(tasks, /pub title: String/);
  assert.match(tasks, /pub completed: bool/);
  assert.match(tasks, /pub created_at: i64/);
  assert.match(tasks, /pub completed_at: Option<i64>/);
  assert.match(tasks, /pub priority: String/);
  assert.match(tasks, /pub due_date: Option<String>/);
  assert.match(tasks, /pub environment_path: Option<String>/);
  assert.match(tasks, /owners: HashMap<String, Vec<TaskItem>>/);
  assert.match(tasks, /crate::settings::runtime_data_path\(STORE_FILE\)/);
  assert.match(tasks, /ErrorKind::NotFound => Ok\(TaskStore::default\(\)\)/);
  assert.ok(tasks.indexOf("owners: HashMap<String, Vec<TaskItem>>") >= 0, "liste par propriétaire");
});

test("les opérations list, replace, add et remove existent", () => {
  assert.match(tasks, /pub fn list\(owner_id: &str\) -> Result<Vec<TaskItem>, String>/);
  assert.match(tasks, /pub fn replace\(owner_id: &str, items: serde_json::Value\)/);
  assert.match(tasks, /pub fn add\(owner_id: &str, item: serde_json::Value\)/);
  assert.match(tasks, /pub fn remove\(owner_id: &str, id: &str\)/);
  assert.match(tasks, /store\.owners\.get\(owner_id\)\.cloned\(\)\.unwrap_or_default\(\)/);
  assert.match(tasks, /tasks\.insert\(0, task\)/);
  assert.match(tasks, /tasks\.retain\(\|task\| task\.id != id\)/);
});

test("la normalisation sert le même format que le frontend", () => {
  const normalize = block(tasks, "fn normalize_task(", "fn store_mut");
  assert.match(tasks, /const TASK_TITLE_MAX_LENGTH: usize = 240/);
  assert.match(tasks, /TASK_PRIORITIES/);
  assert.ok(normalize.includes('get("title")'), "titre lu");
  assert.ok(normalize.includes('get("createdAt")'), "createdAt lu");
  assert.ok(normalize.includes('get("completedAt")'), "completedAt lu");
  assert.ok(normalize.includes('get("priority")'), "priorité lue");
  assert.ok(normalize.includes('get("dueDate")'), "échéance lue");
  assert.ok(normalize.includes('get("environmentPath")'), "chemin d'environnement lu");
  assert.ok(normalize.indexOf(".take(TASK_TITLE_MAX_LENGTH)") >= 0, "titre borné");
  assert.ok(normalize.indexOf("split_whitespace()") >= 0, "espaces normalisés");
  assert.match(tasks, /fn valid_due_date/);
  assert.match(tasks, /fn normalize_priority/);
  assert.match(tasks, /fn normalize_environment_path/);
});

test("les tâches de tests Rust couvrent les cas limites", () => {
  assert.match(tasks, /#\[cfg\(test\)\]/);
  assert.match(tasks, /fn normalize_task_rejects_empty_or_invalid/);
  assert.match(tasks, /fn replace_and_remove_keep_lists_isolated_per_owner/);
  assert.match(tasks, /fn normalize_task_fixes_priority_and_due_date/);
});

// ─────────────────────────── 2. Résolution du propriétaire ───────────────────────────

test("auth.rs sait résoudre un nom d'utilisateur vers son id", () => {
  const lookup = block(auth, "pub(crate) fn user_id_by_username(", "fn public_config");
  assert.match(lookup, /user\.id == identifier/);
  assert.match(lookup, /user\.username\.to_lowercase\(\) == identifier\.to_lowercase\(\)/);
  assert.match(lookup, /\.map\(\|user\| user\.id\.clone\(\)\)/);
  assert.match(lookup, /if identifier\.is_empty\(\)/);
  assert.ok(lookup.indexOf("Ok(None)") >= 0, "compte inconnu toléré");
});

test("server.rs résout le propriétaire via request_actor et le paramètre account", () => {
  const owner = block(server, "fn api_tasks_owner(", "async fn api_tasks_list");
  assert.match(owner, /request_actor\(state, headers\)\?/);
  assert.match(owner, /RequestActor::User\(identity\)/);
  assert.match(owner, /RequestActor::Administrator/);
  assert.match(owner, /user_id_by_username\(account\)/);
  assert.match(owner, /"server-admin"\.to_string\(\)/);
  assert.match(owner, /Un compte utilisateur n'accède qu'à ses propres tâches/);
  assert.match(owner, /Compte introuvable pour ses tâches/);
  assert.ok(owner.indexOf('query.account.as_deref()') >= 0, "paramètre account lu");
  assert.ok(owner.indexOf("StatusCode::FORBIDDEN") >= 0, "accès refusé à un autre compte");
  assert.ok(owner.indexOf("StatusCode::NOT_FOUND") >= 0, "compte inconnu");
});

// ─────────────────────────── 3. Routes montées ───────────────────────────

test("les routes /api/tasks sont montées avec les bonnes méthodes", () => {
  assert.match(server, /\.route\(\r?\n\s*"\/tasks",\r?\n\s*get\(api_tasks_list\)\r?\n\s*\.post\(api_tasks_add\)\r?\n\s*\.put\(api_tasks_replace\)\r?\n\s*\.layer\(DefaultBodyLimit::max\(1024 \* 1024\)\),\r?\n\s*\)/);
  assert.match(server, /\.route\("\/tasks\/:id", delete\(api_tasks_remove\)\)/);
});

test("les 4 handlers appellent le module tasks et répondent en JSON", () => {
  const handlers = block(server, "async fn api_tasks_list(", "fn telegram_error_status");
  assert.match(handlers, /crate::tasks::list\(&owner\)/);
  assert.match(handlers, /crate::tasks::add\(&owner, task\)/);
  assert.match(handlers, /crate::tasks::replace\(&owner, items\)/);
  assert.match(handlers, /crate::tasks::remove\(&owner, &id\)/);
  assert.match(handlers, /json_response\(items\)/);
  assert.match(handlers, /api_error\(StatusCode::INTERNAL_SERVER_ERROR/);
  assert.match(handlers, /api_error\(StatusCode::BAD_REQUEST/);
  assert.match(handlers, /creative_json_rejection\(&state, error\)/);
  assert.match(handlers, /AxumPath\(id\): AxumPath<String>/);
  const owners = handlers.match(/api_tasks_owner\(&state, &headers, &query\)/g) ?? [];
  assert.equal(owners.length, 4, "les 4 handlers résolvent le propriétaire");
});

// ─────────────────────────── 4. Enregistrement du module ───────────────────────────

test("lib.rs déclare le module tasks", () => {
  assert.match(lib, /mod tasks;/);
});

// ─────────────────────────── 5. Mapping platform.ts ───────────────────────────

test("platform.ts mappe les 4 commandes tâches vers les routes HTTP", () => {
  const mapping = block(platform, 'case "tasks_list"', 'case "vps_deploy_capabilities"');
  assert.match(mapping, /"GET",\r?\n\s*`\/api\/tasks\$\{args\.account/);
  assert.match(mapping, /"POST",\r?\n\s*`\/api\/tasks\$\{args\.account/);
  assert.match(mapping, /"PUT",\r?\n\s*`\/api\/tasks\$\{args\.account/);
  assert.match(mapping, /"DELETE",\r?\n\s*`\/api\/tasks\/\$\{encodeURIComponent\(String\(args\.id\)\)\}/);
  assert.match(mapping, /args\.task,/);
  assert.match(mapping, /args\.items,/);
  const cases = mapping.match(/case "tasks_[a-z_]+":/g) ?? [];
  assert.equal(cases.length, 4, "4 cas de mapping");
  assert.ok(cases.includes('case "tasks_list":'), "list");
  assert.ok(cases.includes('case "tasks_add":'), "add");
  assert.ok(cases.includes('case "tasks_replace":'), "replace");
  assert.ok(cases.includes('case "tasks_remove":'), "remove");
});

// ─────────────────────────── 6. Couche de synchronisation (tasks-sync.ts) ───────────────────────────

test("tasks-sync garde le serveur comme source et le localStorage en cache", () => {
  assert.match(sync, /import \{ invoke, isRemoteMode \} from "\.\/platform"/);
  assert.match(sync, /import \{\s*loadTaskItems,\s*persistTaskItems,/);
  assert.match(sync, /let localDirty = false/);
  assert.match(sync, /export const markTasksDirty/);
  assert.match(sync, /export const syncTasksFromServer/);
  assert.match(sync, /export const pushTasksToServer/);
  assert.match(sync, /export const addRemoteTask/);
  assert.match(sync, /export const removeRemoteTask/);
  assert.match(sync, /invoke<TaskItem\[\]>\("tasks_list"/);
  assert.match(sync, /invoke\("tasks_replace"/);
  assert.match(sync, /invoke\("tasks_add"/);
  assert.match(sync, /invoke\("tasks_remove"/);
  assert.match(sync, /if \(!isRemoteMode\(\) \|\| typeof window === "undefined"\) return;/);
  assert.ok(sync.indexOf('JSON.stringify(remote) !== JSON.stringify(local)') >= 0, "adoption seulement si différence");
  assert.ok(sync.indexOf("localDirty") >= 0, "reconciliation par drapeau sale");
  assert.ok(sync.indexOf("Première migration") >= 0, "migration du cache local documentée");
  assert.ok(sync.indexOf("accountId?.trim() || undefined") >= 0, "compte transmis au serveur");
});

test("tasks-view pousse chaque sauvegarde vers le serveur en arrière-plan", () => {
  assert.match(taskView, /import\("\.\/tasks-sync"\)\.then\(\(\{ markTasksDirty, pushTasksToServer \}\)/);
  assert.match(taskView, /markTasksDirty\(\);/);
  assert.match(taskView, /pushTasksToServer\(items, options\.accountId\)/);
});

test("main.ts synchronise à l'ouverture de l'onglet et à la coche rapide", () => {
  assert.match(main, /if \(activeView === "tasks"\) \{\s*\/\/ Le serveur est la source de vérité des tâches/);
  assert.match(main, /syncTasksFromServer\(accountScopedStorage, currentTaskAccountId\(\), render\)/);
  assert.match(main, /import\("\.\/tasks-sync"\)\.then\(\(\{ markTasksDirty, pushTasksToServer \}\)/);
  assert.match(main, /pushTasksToServer\(next, accountId\)/);
});

// ─────────────────────────── 7. Cohérence globale ───────────────────────────

test("les commandes frontend ont leur route serveur et le modèle reste pur", () => {
  for (const command of ["tasks_list", "tasks_add", "tasks_replace", "tasks_remove"]) {
    assert.ok(platform.includes(`case "${command}":`), `mapping platform manquant: ${command}`);
  }
  for (const route of ["/api/tasks", "/api/tasks/"]) {
    assert.ok(platform.includes(route), `route plateforme manquante: ${route}`);
    assert.ok(server.includes(route.replace("/api", "")), `route serveur manquante: ${route}`);
  }
  assert.ok(server.includes('"/tasks"'), "route /tasks montée");
  assert.ok(server.includes('"/tasks/:id"'), "route /tasks/:id montée");
  assert.doesNotMatch(taskModel, /renderTasksPanel|mountTasksPanel|document\.querySelector|from "\.\/platform"/);
  assert.ok(taskModel.includes('export const taskStorageKeyForAccount'), "clé par compte conservée");
});
