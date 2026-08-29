import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const access = readFileSync(
  new URL("../src-tauri/src/workspace_access.rs", import.meta.url),
  "utf8",
);
const server = readFileSync(new URL("../src-tauri/src/server.rs", import.meta.url), "utf8");
const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const platform = readFileSync(new URL("../src/platform.ts", import.meta.url), "utf8");
const style = readFileSync(new URL("../src/style.css", import.meta.url), "utf8");
const smoke = readFileSync(new URL("../scripts/smoke-site.mjs", import.meta.url), "utf8");

test("une réponse d’accès mal formée ne casse pas les interactions du menu", () => {
  const start = main.indexOf("const refreshWorkspaceAccess =");
  const end = main.indexOf("const submitWorkspaceAccessRequest", start);
  const implementation = main.slice(start, end);

  assert.match(implementation, /const response = await invoke<unknown>\("workspace_access"\)/);
  assert.match(implementation, /if \(!Array\.isArray\(response\)\)/);
  assert.match(smoke, /if \(path === "\/api\/workspaces\/access"\) return \[\]/);
});

test("le navigateur de dossiers reste strictement dans la racine personnelle", () => {
  const start = access.indexOf("pub(crate) fn authorize_browse_path");
  const end = access.indexOf("/// Valide un chemin de travail", start);
  const implementation = access.slice(start, end);

  assert.match(implementation, /let personal_root = self\.personal_root\(identity\)\?/);
  assert.match(implementation, /requested\.starts_with\(&personal_root\)/);
  assert.match(implementation, /Err\(WorkspaceAccessError::not_found\(\)\)/);
  assert.doesNotMatch(implementation, /user_can_access|members|matching_environment/);
  assert.match(main, /await loadWorkspaceDir\(null\)/);
  assert.match(main, /workspaceId === browseRootId \|\| workspaceId\.startsWith/);
});

test("un compte cree un environnement directement dans son espace", () => {
  assert.match(
    server,
    /\.route\(\s*"\/workspaces",\s*get\(api_workspaces\)\.post\(api_create_workspace\),?\s*\)/,
  );
  assert.match(server, /\.create_environment\(&identity, &request\.name\)/);
  assert.match(access, /let personal_root = self\.personal_root\(identity\)\?/);
  assert.match(access, /let directory = personal_root\.join\(new_environment_directory_name\(&label\)\)/);
  assert.match(platform, /case "create_workspace":\s*return api<T>\("POST", "\/api\/workspaces", \{ name: args\.name \}\)/);
  assert.match(main, /id="createPersonalWorkspaceForm"/);
  assert.match(main, /openWorkspacePicker\("create"\)/);
  assert.match(main, /Créer un autre environnement/);
  assert.match(style, /\.ws-create-environment-form/);
});

test("un import GitHub exige un compte et rejoint uniquement son espace personnel", () => {
  const start = server.indexOf("async fn api_create_git_docker_environment");
  const end = server.indexOf("async fn api_delete_workspace", start);
  const implementation = server.slice(start, end);

  // La route d'import GitHub reste protegee cote serveur meme si le bouton UI a
  // ete retire : compte requis, espace personnel uniquement, jamais admin.
  assert.match(implementation, /require_user_actor\(&state, &headers\)/);
  assert.match(implementation, /personal_root\(&identity\)/);
  assert.match(implementation, /claim_or_authorize_environment\(&identity/);
  assert.doesNotMatch(implementation, /RequestActor::Administrator|PROJECTS_DIRECTORY/);
});

test("un partage accepte ne devient pas une porte d'entree du navigateur", () => {
  assert.match(
    access,
    /authorize_existing_environment\(&guest,[\s\S]*?\.is_ok\(\)[\s\S]*?authorize_browse_path\(&guest,[\s\S]*?\.is_err\(\)/,
  );
});

test("un membre retrouve tout l'ecosysteme rattache a l'environnement partage", () => {
  const discussionFilter = server.slice(
    server.indexOf("fn filter_discussions_for_identity"),
    server.indexOf("fn authorize_discussion_for_identity"),
  );
  const autonomousList = server.slice(
    server.indexOf("async fn api_list_autonomous_agents"),
    server.indexOf("async fn api_create_autonomous_agent"),
  );
  const orchestrationList = server.slice(
    server.indexOf("async fn api_list_orchestrations"),
    server.indexOf("async fn api_create_orchestration"),
  );

  assert.match(discussionFilter, /authorize_existing_environment\(identity, cwd\)/);
  assert.match(autonomousList, /authorize_existing_environment\(identity, project_dir\)/);
  assert.match(orchestrationList, /authorize_existing_environment\(identity, &run\.project_dir\)/);
  assert.match(main, /fichiers, mémoire, historique des chats, agents autonomes et orchestrations sont communs/);
});

test("le partage reste limite a l'environnement explicitement autorise", () => {
  assert.match(access, /fn user_can_access[\s\S]*?environment\.owner_id == user_id[\s\S]*?members/);
  assert.match(
    server,
    /fn authorize_discussion_for_identity[\s\S]*?authorize_existing_environment\(identity, cwd\)/,
  );
  assert.match(main, /Vos préférences et vos autres environnements restent personnels/);
});
