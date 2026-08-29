import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const cloud = read("../src-tauri/src/freebuff_cloud.rs");
const server = read("../src-tauri/src/server.rs");
const lib = read("../src-tauri/src/lib.rs");
const frontend = read("../src/freebuff-cloud.ts");
const css = read("../src/freebuff-cloud.css");
const main = read("../src/main.ts");
const platform = read("../src/platform.ts");

const block = (source, start, end) => {
  const from = source.indexOf(start);
  const to = source.indexOf(end, from + start.length);
  assert.ok(from >= 0, `debut introuvable: ${start}`);
  assert.ok(to > from, `fin introuvable: ${end}`);
  return source.slice(from, to);
};

// ─────────────────────────── 1. Constantes d'infrastructure ───────────────────────────

test("les constantes Freebuff Cloud pointent vers les bons services", () => {
  assert.match(cloud, /const STORE_FILE: &str = "freebuff-cloud\.json"/);
  assert.match(cloud, /const STORE_VERSION: u32 = 1/);
  assert.match(cloud, /const FREEBUFF_BASE: &str = "https:\/\/freebuff\.com"/);
  assert.match(cloud, /const CONVEX_DEPLOYMENT: &str = "https:\/\/harmless-tapir-303\.convex\.cloud"/);
  assert.match(cloud, /const SESSION_URL: &str = "https:\/\/freebuff\.com\/api\/web\/freebuff-session"/);
  assert.match(cloud, /const CONVEX_TOKEN_URL: &str = "https:\/\/freebuff\.com\/api\/web\/convex-token"/);
  assert.match(cloud, /const AGENT_RUNS_STREAM_PATH: &str = "\/api\/agent-runs\/stream"/);
  assert.match(cloud, /const CONVEX_TOKEN_CACHE_SECS: u64 = 4 \* 60/);
  assert.match(cloud, /const HTTP_TIMEOUT_SECS: u64 = 30/);
  assert.match(cloud, /STORE_FILE/);
  assert.match(cloud, /CONVEX_DEPLOYMENT/);
  assert.match(cloud, /SESSION_URL/);
  assert.match(cloud, /CONVEX_TOKEN_URL/);
  assert.match(cloud, /AGENT_RUNS_STREAM_PATH/);
  assert.match(cloud, /CONVEX_TOKEN_CACHE_SECS/);
  assert.match(cloud, /HTTP_TIMEOUT_SECS/);
  assert.ok(cloud.indexOf("freebuff.com") > 0, "freebuff.com doit être la base");
  assert.ok(cloud.indexOf("convex.cloud") > 0, "le déploiement Convex doit être présent");
  assert.ok(cloud.indexOf("/api/web/") > 0, "routes /api/web présentes");
  assert.ok(cloud.indexOf("agent-runs") > 0, "chemin SSE présent");
});

// ─────────────────────────── 2. Stockage & sécurité ───────────────────────────

test("le stockage garde le cookie en secret et limite les permissions", () => {
  assert.match(cloud, /struct FreebuffCloudStore \{/);
  assert.match(cloud, /session_cookie: Option<String>/);
  assert.match(cloud, /convex_token: Option<String>/);
  assert.match(cloud, /convex_token_fetched_at: Option<i64>/);
  assert.match(cloud, /user_email: Option<String>/);
  assert.match(cloud, /user_name: Option<String>/);
  assert.match(cloud, /github_login: Option<String>/);
  assert.match(cloud, /instance_id: Option<String>/);
  assert.match(cloud, /#\[serde\(rename_all = "camelCase"\)\]/);
  assert.match(cloud, /#\[cfg\(unix\)\]/);
  assert.match(cloud, /use std::os::unix::fs::PermissionsExt;/);
  assert.match(cloud, /set_permissions\(&path, fs::Permissions::from_mode\(0o600\)\)/);
  assert.match(cloud, /fs::write\(&path, content\)/);
  assert.match(cloud, /serde_json::to_string_pretty\(store\)/);
  assert.match(cloud, /ErrorKind::NotFound => Ok\(FreebuffCloudStore::default\(\)\)/);
  assert.match(cloud, /Fichier Freebuff Cloud invalide/);
  assert.match(cloud, /Écriture du fichier Freebuff Cloud impossible/);
  assert.match(cloud, /le cookie de session n'apparaît dans aucune vue sérialisée/i);
  assert.match(cloud, /n'ajoutent jamais le cookie à une autre origine que/i);
  assert.ok(cloud.includes("jamais renvoyé dans une vue"), "le cookie ne doit jamais être renvoyé");
});

test("les vues ne sérialisent jamais le cookie ni le token", () => {
  const views = block(cloud, "pub struct FreebuffCloudStatusView", "pub struct FreebuffCloudConnectView")
    + block(cloud, "pub struct FreebuffCloudConnectView", "pub struct ConnectFreebuffCloudRequest");
  assert.doesNotMatch(views, /session_cookie/);
  assert.doesNotMatch(views, /convex_token/);
  assert.match(views, /pub connected: bool/);
  assert.match(views, /pub email: Option<String>/);
  assert.match(views, /pub name: Option<String>/);
  assert.match(views, /pub github_login: Option<String>/);
  assert.match(views, /pub message: Option<String>/);
  assert.match(views, /pub message: String/);
  assert.match(views, /rename_all = "camelCase"/);
  assert.ok(views.indexOf("connected") >= 0, "champ connected exposé");
  assert.ok(views.indexOf("github_login") >= 0, "login GitHub exposé");
  assert.ok(views.indexOf("message") >= 0, "message exposé");
});

test("les requêtes entrantes sont bornées et désérialisées en camelCase", () => {
  assert.match(cloud, /pub struct ConnectFreebuffCloudRequest \{/);
  assert.match(cloud, /pub struct CreateBlankProjectRequest \{/);
  assert.match(cloud, /pub struct ConnectRepoRequest \{/);
  assert.match(cloud, /pub struct DeleteProjectRequest \{/);
  assert.match(cloud, /pub struct AgentRunStreamQuery \{/);
  assert.match(cloud, /session_cookie: String/);
  assert.match(cloud, /name: String/);
  assert.match(cloud, /repo_full_name: String/);
  assert.match(cloud, /project_id: String/);
  assert.match(cloud, /message_id: String/);
  assert.match(cloud, /run_id: Option<String>/);
  assert.match(cloud, /rename_all = "camelCase"/);
});

// ─────────────────────────── 3. Helpers texte & profil ───────────────────────────

test("pick_string tolère les variations de schéma et ignore les champs vides", () => {
  const helper = block(cloud, "fn pick_string<", "fn extract_user_profile");
  assert.match(helper, /pointer\(path\)/);
  assert.match(helper, /Value::as_str/);
  assert.match(helper, /map\(str::trim\)/);
  assert.match(helper, /filter\(\|field\| !field\.is_empty\(\)\)/);
  assert.match(helper, /find_map/);
  assert.match(helper, /ToString::to_string/);
  assert.ok(helper.indexOf("&[&str]") >= 0, "liste de chemins en entrée");
});

test("extract_user_profile couvre les schémas user/github connus", () => {
  const extract = block(cloud, "fn extract_user_profile(", "fn fetch_session");
  assert.match(extract, /\"\/user\/email\"/);
  assert.match(extract, /\"\/user\/email_address\"/);
  assert.match(extract, /\"\/email\"/);
  assert.match(extract, /\"\/profile\/email\"/);
  assert.match(extract, /\"\/user\/name\"/);
  assert.match(extract, /\"\/user\/full_name\"/);
  assert.match(extract, /\"\/user\/displayName\"/);
  assert.match(extract, /\"\/user\/github_login\"/);
  assert.match(extract, /\"\/user\/githubLogin\"/);
  assert.match(extract, /\"\/github\/login\"/);
  assert.match(extract, /\"\/instanceId\"/);
  assert.ok(extract.indexOf("/email") >= 0, "email");
  assert.ok(extract.indexOf("/name") >= 0, "nom");
  assert.ok(extract.indexOf("github") >= 0, "github");
  assert.ok(extract.indexOf("instance") >= 0, "instance");
  assert.ok(extract.indexOf("(Option<String>, Option<String>, Option<String>, Option<String>)") >= 0, "tuple 4 sorties");
});

// ─────────────────────────── 4. Session & token Convex ───────────────────────────

test("normalize_session_cookie réapplique le nom du cookie NextAuth", () => {
  assert.match(cloud, /const SESSION_COOKIE_NAME: &str = "__Secure-next-auth\.session-token"/);
  const normalize = block(cloud, "fn normalize_session_cookie(", "fn fetch_session");
  assert.match(normalize, /raw\.trim\(\)/);
  assert.match(normalize, /trimmed\.is_empty\(\)/);
  assert.ok(normalize.includes("split_once('=')"), "détection d'une paire nom=valeur");
  assert.ok(normalize.includes('format!("{SESSION_COOKIE_NAME}={trimmed}")'), "préfixe réappliqué");
  assert.ok(normalize.indexOf("SESSION_COOKIE_NAME") >= 0, "constante du nom du cookie référencée");
  assert.ok(normalize.indexOf("split_once('=')") >= 0, "détection d'une paire nom=valeur");
});

test("fetch_session valide le cookie sur freebuff.com seulement", () => {
  const fetch = block(cloud, "fn fetch_session(", "fn fetch_convex_token");
  assert.match(fetch, /\.get\(SESSION_URL\)/);
  assert.match(fetch, /\.header\("cookie", normalize_session_cookie\(cookie\)\)/);
  assert.match(fetch, /\.header\("accept", "application\/json"\)/);
  assert.match(fetch, /\.send\(\)\s*\.await/);
  assert.match(fetch, /Freebuff Cloud est injoignable/);
  assert.match(fetch, /Réponse de session Freebuff Cloud illisible/);
  assert.match(fetch, /Session Freebuff Cloud invalide ou expirée/);
  assert.match(fetch, /status\.is_success\(\)/);
  assert.doesNotMatch(fetch, /CONVEX_DEPLOYMENT/);
  assert.doesNotMatch(fetch, /\.bearer_auth/);
});

test("fetch_convex_token récupère le token avec les headers de localisation", () => {
  const fetch = block(cloud, "fn fetch_convex_token(", "fn resolve_convex_token");
  assert.match(fetch, /\.get\(CONVEX_TOKEN_URL\)/);
  assert.match(fetch, /\.header\("cookie", normalize_session_cookie\(cookie\)\)/);
  assert.match(fetch, /\.header\("x-fb-timezone", "UTC"\)/);
  assert.match(fetch, /\.header\("x-fb-tz-offset", "0"\)/);
  assert.match(fetch, /\.header\("x-fb-languages", "fr-FR,fr,en"\)/);
  assert.match(fetch, /\"\/token\"/);
  assert.match(fetch, /\"\/value\"/);
  assert.match(fetch, /\"\/tokenValue\"/);
  assert.match(fetch, /\"\/convexToken\"/);
  assert.match(fetch, /\"\/convex_token\"/);
  assert.match(fetch, /payload\.as_str\(\)/);
  assert.match(fetch, /Token Convex Freebuff Cloud absent de la réponse/);
  assert.match(fetch, /Token Convex Freebuff Cloud refusé/);
  assert.match(fetch, /\.filter\(\|token\| !token\.is_empty\(\)\)/);
  assert.doesNotMatch(fetch, /\.session_cookie/);
});

test("le cache du token est rafraîchi à expiration et persisté", () => {
  const resolve = block(cloud, "fn resolve_convex_token(", "fn convex_call(");
  assert.match(resolve, /Compte Freebuff Cloud non connecté/);
  assert.match(resolve, /convex_token_fetched_at/);
  assert.match(resolve, /now_unix\(\) - fetched < CONVEX_TOKEN_CACHE_SECS as i64/);
  assert.match(resolve, /is_some_and/);
  assert.match(resolve, /fetch_convex_token\(cookie\)\.await/);
  assert.match(resolve, /save_store\(&updated\)/);
  assert.match(resolve, /updated\.convex_token = Some\(token\.clone\(\)\)/);
  assert.match(resolve, /updated\.convex_token_fetched_at = Some\(now_unix\(\)\)/);
  assert.match(resolve, /\.filter\(\|t\| !t\.is_empty\(\)\)/);
  assert.ok(resolve.indexOf("50 * 60") >= 0 || resolve.indexOf("CONVEX_TOKEN_CACHE_SECS") >= 0, "durée de cache référencée");
});

test("un 401 Convex déclenche un seul rafraîchissement puis un retry", () => {
  const call = block(cloud, "fn convex_call(", "fn convex_call_with_token");
  assert.match(call, /error\.starts_with\("token_convex_expire"\)/);
  assert.match(call, /fetch_convex_token\(cookie\)\.await/);
  assert.match(call, /save_store\(&updated\)/);
  assert.match(call, /convex_call_with_token\(kind, path, &args, &refreshed\)\.await/);
  assert.ok((call.match(/convex_call_with_token/g) ?? []).length >= 2, "appel + retry");
  assert.ok((call.match(/fetch_convex_token/g) ?? []).length >= 1, "rafraîchissement présent");
  assert.ok(call.indexOf("token_convex_expire") >= 0, "code d'erreur token expiré");
});

// ─────────────────────────── 5. Protocole Convex ───────────────────────────

test("convex_call_with_token suit le protocole HTTP Convex", () => {
  const call = block(cloud, "fn convex_call_with_token(", "/// Connexion :");
  assert.match(call, /format!\("\{CONVEX_DEPLOYMENT\}\/api\/\{kind\}"\)/);
  assert.match(call, /\{CONVEX_DEPLOYMENT\}\/api/);
  assert.match(call, /\{kind\}/);
  assert.match(call, /\{CONVEX_DEPLOYMENT\}/);
  assert.match(call, /\.bearer_auth\(token\)/);
  assert.match(call, /\.header\("content-type", "application\/json"\)/);
  assert.match(call, /\.json\(&body\)/);
  assert.match(call, /"path": path/);
  assert.match(call, /"format": "json"/);
  assert.match(call, /"args": args/);
  assert.match(call, /"formatVersion": 2/);
  assert.match(call, /StatusCode::UNAUTHORIZED/);
  assert.match(call, /"token_convex_expire"/);
  assert.match(call, /Some\("success"\)/);
  assert.match(call, /Some\("error"\)/);
  assert.match(call, /\/errorMessage/);
  assert.match(call, /Backend Convex Freebuff Cloud injoignable/);
  assert.match(call, /Réponse Convex Freebuff Cloud invalide/);
  assert.match(call, /HTTP \{}/);
  assert.match(call, /payload\.get\("status"\)/);
  assert.ok(call.indexOf('"status"') >= 0, "statut Convex inspecté");
  assert.ok(call.indexOf("formatVersion") >= 0, "formatVersion 2 présent");
});

test("now_unix et http_client sont définis proprement", () => {
  assert.match(cloud, /fn now_unix\(\) -> i64/);
  assert.match(cloud, /duration_since\(UNIX_EPOCH\)/);
  assert.match(cloud, /unwrap_or\(0\)/);
  assert.match(cloud, /fn http_client\(\) -> Result<Client, String>/);
  assert.match(cloud, /Client::builder\(\)/);
  assert.match(cloud, /\.timeout\(Duration::from_secs\(HTTP_TIMEOUT_SECS\)\)/);
  assert.match(cloud, /Client HTTP Freebuff Cloud indisponible/);
});

// ─────────────────────────── 6. Connexion & déconnexion ───────────────────────────

test("connect valide, trim, profile et persiste en une passe", () => {
  const connect = block(cloud, "pub async fn connect(", "pub fn disconnect");
  assert.match(connect, /normalize_session_cookie\(&session_cookie\)/);
  assert.doesNotMatch(connect, /session_cookie\.trim\(\)/);
  assert.match(connect, /Colle le cookie de session freebuff\.com/);
  assert.match(connect, /fetch_session\(&cookie\)\.await/);
  assert.match(connect, /extract_user_profile\(&session\)/);
  assert.match(connect, /fetch_convex_token\(&cookie\)\.await/);
  assert.match(connect, /FreebuffCloudStore::default\(\)/);
  assert.match(connect, /store\.session_cookie = Some\(cookie\)/);
  assert.match(connect, /store\.convex_token = Some\(token\)/);
  assert.match(connect, /store\.user_email = email\.clone\(\)/);
  assert.match(connect, /store\.user_name = name\.clone\(\)/);
  assert.match(connect, /store\.github_login = github\.clone\(\)/);
  assert.match(connect, /save_store\(&store\)/);
  assert.match(connect, /connected: true/);
  assert.match(connect, /Compte Freebuff Cloud connecté/);
  assert.match(cloud, /#\[cfg_attr\(feature = "desktop", tauri::command\(rename_all = "camelCase"\)\)\]\npub async fn connect\(/);
});

test("disconnect supprime le stockage et tolère l'absence de fichier", () => {
  const disconnect = block(cloud, "pub fn disconnect(", "pub fn status");
  assert.match(disconnect, /fs::remove_file\(&path\)/);
  assert.match(disconnect, /ErrorKind::NotFound => Ok\(\(\)\)/);
  assert.match(disconnect, /Suppression du compte Freebuff Cloud impossible/);
  assert.match(disconnect, /store_path\(\)/);
});

test("status reflète la connexion sans fuir le cookie", () => {
  const status = block(cloud, "pub fn status(", "/// Liste des projets");
  assert.match(status, /load_store\(\)/);
  assert.match(status, /connected: false/);
  assert.match(status, /is_some_and\(\|c\| !c\.is_empty\(\)\)/);
  assert.match(status, /connected,/);
  assert.match(status, /email: store\.user_email/);
  assert.match(status, /name: store\.user_name/);
  assert.match(status, /github_login: store\.github_login/);
  assert.doesNotMatch(status, /FreebuffCloudStatusView \{[^}]*session_cookie/);
  assert.doesNotMatch(status, /FreebuffCloudStatusView \{[^}]*convex_token/);
});

// ─────────────────────────── 7. Fonctions projets & repos ───────────────────────────

test("les projets utilisent la fonction Convex getUserProjects", () => {
  const projects = block(cloud, "pub async fn projects(", "/// Crée un projet vierge");
  assert.match(projects, /"query",\s*"project:getUserProjects"/);
  assert.match(projects, /json!\(\{ "surface": "cloud", "archivedOnly": false \}\)/);
  assert.match(projects, /Value::Array\(projects\) => Ok\(projects\)/);
  assert.match(projects, /Value::Null => Ok\(Vec::new\(\)\)/);
  assert.match(projects, /Réponse de projets Freebuff Cloud invalide/);
});

test("les actions créent et suppriment avec les bons arguments", () => {
  const blank = block(cloud, "pub async fn create_blank_project(", "pub async fn connect_repo");
  assert.match(blank, /"action", "cloud\/blankProject:createBlankProject"/);
  assert.match(blank, /json!\(\{ "name": name \}\)/);
  assert.match(blank, /tauri::command\(rename_all = "camelCase"\)/);
  const repo = block(cloud, "pub async fn connect_repo(", "pub async fn delete_project");
  assert.match(repo, /"action",\s*"cloud\/connectRepo:connectRepo"/);
  assert.match(repo, /json!\(\{ "repoFullName": repo_full_name \}\)/);
  assert.match(repo, /rename_all = "camelCase"/);
  const del = block(cloud, "pub async fn delete_project(", "/// Repos GitHub connectables");
  assert.match(del, /"action",\s*"project:deleteProject"/);
  assert.match(del, /json!\(\{ "projectId": project_id \}\)/);
  assert.match(cloud, /tauri::command\(rename_all = "camelCase"\)\)\]\npub async fn delete_project\(/);
});

test("les repos connectables rafraîchissent puis lisent le cache", () => {
  const repos = block(cloud, "pub async fn connectable_repos(", "/// URL du flux SSE");
  assert.match(repos, /"action",\s*"github\/cloudRepos:refreshConnectableRepositories"/);
  assert.match(repos, /"action",\s*"github\/repoCacheStore:getCachedConnectableRepositories"/);
  assert.match(repos, /json!\(\{\}\)/);
  assert.match(repos, /if let Ok\(value\) = refreshed/);
  assert.match(repos, /matches!\(value, Value::Null\)/);
  assert.ok((repos.match(/convex_call/g) ?? []).length >= 2, "refresh + fallback");
});

// ─────────────────────────── 8. Flux SSE & urlencode ───────────────────────────

test("agent_run_stream_url construit l'URL SSE avec messageId et runId optionnel", () => {
  const url = block(cloud, "pub fn agent_run_stream_url(", "/// Cookie de session");
  assert.match(url, /FREEBUFF_BASE/);
  assert.match(url, /AGENT_RUNS_STREAM_PATH/);
  assert.match(url, /\?messageId=\{\}/);
  assert.match(url, /urlencode\(message_id\)/);
  assert.match(url, /run_id\.filter\(\|value\| !value\.is_empty\(\)\)/);
  assert.match(url, /&runId=\{\}/);
  assert.match(url, /urlencode\(run_id\)/);
});

test("urlencode encode tout sauf les caractères non réservés", () => {
  const encoder = cloud.slice(cloud.indexOf("fn urlencode("));
  assert.match(encoder, /b'A'\.\.=b'Z' \| b'a'\.\.=b'z'/);
  assert.match(encoder, /b'0'\.\.=b'9'/);
  assert.match(encoder, /\| b'-' \| b'_' \| b'\.' \| b'~'/);
  assert.match(encoder, /%\{byte:02X\}/);
  assert.match(encoder, /out\.push\(byte as char\)/);
  assert.ok(encoder.indexOf("match byte") >= 0, "encodage par octet");
});

test("session_cookie ne renvoie le cookie que si connecté", () => {
  const cookie = block(cloud, "pub fn session_cookie(", "fn urlencode");
  assert.match(cookie, /load_store\(\)/);
  assert.match(cookie, /\.session_cookie/);
  assert.match(cookie, /Compte Freebuff Cloud non connecté/);
  assert.match(cookie, /ok_or_else/);
});

// ─────────────────────────── 9. Routes serveur montées ───────────────────────────

test("les 9 routes /api/freebuff-cloud/* sont montées avec les bonnes méthodes", () => {
  assert.match(server, /\.route\("\/freebuff-cloud\/status", get\(api_freebuff_cloud_status\)\)/);
  assert.match(server, /\.route\(\n\s*"\/freebuff-cloud\/connect",\n\s*post\(api_freebuff_cloud_connect\)\.layer\(DefaultBodyLimit::max\(16 \* 1024\)\),/);
  assert.match(server, /\.route\("\/freebuff-cloud\/disconnect", post\(api_freebuff_cloud_disconnect\)\)/);
  assert.match(server, /\.route\("\/freebuff-cloud\/projects", get\(api_freebuff_cloud_projects\)\)/);
  assert.match(server, /\.route\(\n\s*"\/freebuff-cloud\/projects\/blank",\n\s*post\(api_freebuff_cloud_create_blank_project\)/);
  assert.match(server, /\.layer\(DefaultBodyLimit::max\(8 \* 1024\)\),\n\s*\)\n\s*\.route\(\n\s*"\/freebuff-cloud\/projects\/repo",/);
  assert.match(server, /"\/freebuff-cloud\/projects\/delete"/);
  assert.match(server, /\.route\("\/freebuff-cloud\/repos", get\(api_freebuff_cloud_connectable_repos\)\)/);
  assert.match(server, /\.route\("\/freebuff-cloud\/stream", get\(api_freebuff_cloud_stream\)\)/);
  const routes = server.match(/\/freebuff-cloud\/[a-z\/]+/g) ?? [];
  assert.equal(routes.length, 9, "9 routes freebuff-cloud attendues");
  assert.ok(routes.includes("/freebuff-cloud/status"), "status");
  assert.ok(routes.includes("/freebuff-cloud/connect"), "connect");
  assert.ok(routes.includes("/freebuff-cloud/disconnect"), "disconnect");
  assert.ok(routes.includes("/freebuff-cloud/projects"), "projects");
  assert.ok(routes.includes("/freebuff-cloud/projects/blank"), "blank");
  assert.ok(routes.includes("/freebuff-cloud/projects/repo"), "repo");
  assert.ok(routes.includes("/freebuff-cloud/projects/delete"), "delete");
  assert.ok(routes.includes("/freebuff-cloud/repos"), "repos");
  assert.ok(routes.includes("/freebuff-cloud/stream"), "stream");
});

test("chaque handler vérifie l'en-tête admin avant d'agir", () => {
  const handlers = block(server, "async fn api_freebuff_cloud_status(", "fn telegram_error_status");
  const checks = handlers.match(/check_admin_header\(&state, &headers\)/g) ?? [];
  assert.equal(checks.length, 9, "9 handlers protégés par l'en-tête admin");
  assert.match(handlers, /freebuff_cloud::status\(\)/);
  assert.match(handlers, /freebuff_cloud::connect\(request\.session_cookie\)/);
  assert.match(handlers, /freebuff_cloud::disconnect\(\)/);
  assert.match(handlers, /freebuff_cloud::projects\(\)\.await/);
  assert.match(handlers, /freebuff_cloud::create_blank_project\(request\.name\)/);
  assert.match(handlers, /freebuff_cloud::connect_repo\(request\.repo_full_name\)/);
  assert.match(handlers, /freebuff_cloud::delete_project\(request\.project_id\)/);
  assert.match(handlers, /freebuff_cloud::connectable_repos\(\)\.await/);
  assert.match(handlers, /creative_json_rejection\(&state, error\)/);
  assert.match(handlers, /creative_generation_error_status\(&error\)/);
  assert.match(handlers, /json_response\(value\)/);
  assert.match(handlers, /json!\(\{ "ok": true \}\)/);
  assert.match(handlers, /Json<ConnectFreebuffCloudRequest>/);
  assert.match(handlers, /Json<CreateBlankProjectRequest>/);
  assert.match(handlers, /Json<ConnectRepoRequest>/);
  assert.match(handlers, /Json<DeleteProjectRequest>/);
});

test("le proxy SSE relaie le flux sans exposer le cookie", () => {
  const stream = block(server, "async fn api_freebuff_cloud_stream(", "fn telegram_error_status");
  assert.match(stream, /Query<AgentRunStreamQuery>/);
  assert.match(stream, /freebuff_cloud::session_cookie\(\)/);
  assert.match(stream, /freebuff_cloud::agent_run_stream_url\(&query\.message_id, query\.run_id\.as_deref\(\)\)/);
  assert.match(stream, /reqwest::Client::new\(\)/);
  assert.match(stream, /reqwest::header::COOKIE, cookie/);
  assert.match(stream, /"accept", "text\/event-stream"/);
  assert.match(stream, /StatusCode::BAD_GATEWAY/);
  assert.match(stream, /Flux Freebuff Cloud injoignable/);
  assert.match(stream, /Flux Freebuff Cloud indisponible/);
  assert.match(stream, /text\/event-stream; charset=utf-8/);
  assert.match(stream, /no-store/);
  assert.match(stream, /X-Accel-Buffering/);
  assert.match(stream, /Body::from_stream\(stream\)/);
  assert.match(stream, /upstream\s*\.bytes_stream\(\)/);
  assert.ok(stream.indexOf("cookie") >= 0, "cookie utilisé en amont");
  assert.doesNotMatch(stream, /json_response\(.*cookie/);
});

// ─────────────────────────── 10. Enregistrement desktop (lib.rs) ───────────────────────────

test("les 8 commandes Tauri Freebuff Cloud sont enregistrées", () => {
  assert.match(lib, /freebuff_cloud::connect,/);
  assert.match(lib, /freebuff_cloud::disconnect,/);
  assert.match(lib, /freebuff_cloud::status,/);
  assert.match(lib, /freebuff_cloud::projects,/);
  assert.match(lib, /freebuff_cloud::create_blank_project,/);
  assert.match(lib, /freebuff_cloud::connect_repo,/);
  assert.match(lib, /freebuff_cloud::delete_project,/);
  assert.match(lib, /freebuff_cloud::connectable_repos,/);
  const registrations = lib.match(/freebuff_cloud::[a-z_]+,/g) ?? [];
  assert.equal(registrations.length, 8, "8 commandes enregistrées");
  assert.ok(registrations.includes("freebuff_cloud::connect,"), "connect");
  assert.ok(registrations.includes("freebuff_cloud::disconnect,"), "disconnect");
  assert.ok(registrations.includes("freebuff_cloud::status,"), "status");
  assert.ok(registrations.includes("freebuff_cloud::projects,"), "projects");
  assert.ok(registrations.includes("freebuff_cloud::create_blank_project,"), "blank");
  assert.ok(registrations.includes("freebuff_cloud::connect_repo,"), "repo");
  assert.ok(registrations.includes("freebuff_cloud::delete_project,"), "delete");
  assert.ok(registrations.includes("freebuff_cloud::connectable_repos,"), "repos");
  assert.match(lib, /mod freebuff_cloud;/);
});

// ─────────────────────────── 11. Mapping remoteInvoke (platform.ts) ───────────────────────────

test("chaque commande frontend a son équivalent HTTP dans platform.ts", () => {
  const mapping = block(platform, 'case "freebuff_cloud_status"', 'case "vps_deploy_capabilities"');
  assert.match(mapping, /"GET", "\/api\/freebuff-cloud\/status"/);
  assert.match(mapping, /"POST", "\/api\/freebuff-cloud\/connect", \{ sessionCookie: args\.sessionCookie \}\)/);
  assert.match(mapping, /"POST", "\/api\/freebuff-cloud\/disconnect"/);
  assert.match(mapping, /"GET", "\/api\/freebuff-cloud\/projects"/);
  assert.match(mapping, /"POST", "\/api\/freebuff-cloud\/projects\/blank", \{ name: args\.name \}\)/);
  assert.match(mapping, /"POST", "\/api\/freebuff-cloud\/projects\/repo", \{ repoFullName: args\.repoFullName \}\)/);
  assert.match(mapping, /"POST", "\/api\/freebuff-cloud\/projects\/delete", \{ projectId: args\.projectId \}\)/);
  assert.match(mapping, /"GET", "\/api\/freebuff-cloud\/repos"/);
  const cases = mapping.match(/case "freebuff_cloud_[a-z_]+":/g) ?? [];
  assert.equal(cases.length, 8, "8 cas de mapping");
  assert.ok(cases.includes('case "freebuff_cloud_status":'), "status");
  assert.ok(cases.includes('case "freebuff_cloud_connect":'), "connect");
  assert.ok(cases.includes('case "freebuff_cloud_disconnect":'), "disconnect");
  assert.ok(cases.includes('case "freebuff_cloud_projects":'), "projects");
  assert.ok(cases.includes('case "freebuff_cloud_create_blank":'), "create_blank");
  assert.ok(cases.includes('case "freebuff_cloud_connect_repo":'), "connect_repo");
  assert.ok(cases.includes('case "freebuff_cloud_delete_project":'), "delete_project");
  assert.ok(cases.includes('case "freebuff_cloud_repos":'), "repos");
});

// ─────────────────────────── 12. Helpers frontend ───────────────────────────

test("escapeHtml neutralise les 5 caractères dangereux", () => {
  const helper = block(frontend, "const escapeHtml =", "const safeHttpsUrl");
  assert.match(helper, /replaceAll\("&", "&amp;"\)/);
  assert.match(helper, /replaceAll\("<", "&lt;"\)/);
  assert.match(helper, /replaceAll\(">", "&gt;"\)/);
  assert.match(helper, /replaceAll\('"', "&quot;"\)/);
  assert.match(helper, /replaceAll\("'", "&#39;"\)/);
  assert.match(helper, /String\(value \?\? ""\)/);
  assert.ok(helper.indexOf("&amp;") >= 0, "amp");
  assert.ok(helper.indexOf("&lt;") >= 0, "lt");
  assert.ok(helper.indexOf("&gt;") >= 0, "gt");
  assert.ok(helper.indexOf("&quot;") >= 0, "quot");
  assert.ok(helper.indexOf("&#39;") >= 0, "apos");
});

test("safeHttpsUrl n'accepte que http/https et retombe sur l'hôte", () => {
  const helper = block(frontend, "const safeHttpsUrl =", "const showToast");
  assert.match(helper, /url\.protocol !== "https:" && url\.protocol !== "http:"/);
  assert.match(helper, /new URL\(raw\)/);
  assert.match(helper, /try \{/);
  assert.match(helper, /catch \{/);
  assert.ok(helper.includes("fallbackHost && /^[a-z0-9.-]+$/i.test(raw)"), "regex du fallback d'hôte");
  assert.match(helper, /`https:\/\/\$\{raw\}`/);
  assert.match(helper, /url\.toString\(\)/);
  assert.ok(helper.indexOf("https:") >= 0, "https accepté");
  assert.ok(helper.indexOf("http:") >= 0, "http accepté");
});

test("pick ignore les clés manquantes et les chaînes vides", () => {
  const pick = block(frontend, "const pick =", "const numberField");
  assert.match(pick, /for \(const key of keys\)/);
  assert.match(pick, /value !== undefined && value !== null/);
  assert.match(pick, /typeof value === "string"/);
  assert.match(pick, /\.trim\(\)/);
  assert.match(pick, /if \(trimmed\) return trimmed as T/);
  assert.ok(pick.indexOf("keys: string[]") >= 0, "clés multiples");
  assert.ok(pick.indexOf("return null") >= 0, "retour nul");
});

test("numberField accepte les nombres finis et les chaînes numériques", () => {
  const field = block(frontend, "const numberField =", "const normalizeProject");
  assert.match(field, /typeof value === "number" && Number\.isFinite\(value\)/);
  assert.match(field, /typeof value === "string"/);
  assert.match(field, /!Number\.isNaN\(Number\(value\)\)/);
  assert.match(field, /return Number\(value\)/);
  assert.match(field, /return null/);
});

// ─────────────────────────── 13. normalizeProject ───────────────────────────

test("normalizeProject couvre tous les alias de champs du backend", () => {
  const normalize = block(frontend, "const normalizeProject", "const normalizeRepos");
  assert.match(normalize, /\["_id", "id", "projectId", "project_id"\]/);
  assert.match(normalize, /\["semanticIdentifier", "semantic_identifier", "slug"\]/);
  assert.match(normalize, /\["name", "title"\]/);
  assert.match(normalize, /\["repoFullName", "repo_full_name", "repoName", "repo_url", "repoUrl"\]/);
  assert.match(normalize, /\["status", "buildStatus", "state"\]/);
  assert.match(normalize, /\["previewUrl", "preview_url", "previewURL", "preview"\]/);
  assert.match(normalize, /\["createdAt", "created_at", "created"\]/);
  assert.match(normalize, /\["updatedAt", "updated_at", "updated"\]/);
  assert.match(normalize, /\["messageId", "message_id", "agentRunId", "runMessageId"\]/);
  assert.match(normalize, /\["runId", "run_id", "agentRun"\]/);
  assert.match(normalize, /previewUrl: safeHttpsUrl\(preview\)/);
  assert.match(normalize, /createdAt: numberField\(record, \[/);
  assert.match(normalize, /updatedAt: numberField\(record, \[/);
  assert.match(normalize, /raw: record/);
  assert.ok(normalize.indexOf("if (!value || typeof value !== \"object\") return null;") >= 0, "rejette les non-objets");
});

test("normalizeRepos déduplique et explore les structures imbriquées", () => {
  const normalize = block(frontend, "const normalizeRepos", "const statusLabel");
  assert.match(normalize, /Array\.isArray\(value\)/);
  assert.match(normalize, /\["full_name", "fullName", "repo", "name", "repo_full_name"\]/);
  assert.match(normalize, /\[\.\.\.new Set\(items\)\]/);
  assert.match(normalize, /\(value as Record<string, unknown>\)\.repos/);
  assert.match(normalize, /\.repositories/);
  assert.match(normalize, /\.data/);
  assert.match(normalize, /\.items \?\? candidate/);
  assert.match(normalize, /if \(nested\.length\) return nested/);
  assert.match(normalize, /filter\(\(item\): item is string => Boolean\(item\)\)/);
  assert.ok(normalize.indexOf("return []") >= 0, "retour vide par défaut");
});

// ─────────────────────────── 14. Statuts & dates ───────────────────────────

test("statusLabel traduit tous les états connus en français", () => {
  const label = block(frontend, "const statusLabel", "const statusTone");
  assert.match(label, /case "ready":/);
  assert.match(label, /case "running":/);
  assert.match(label, /case "active":/);
  assert.match(label, /case "live":/);
  assert.match(label, /"En cours"/);
  assert.match(label, /case "completed":/);
  assert.match(label, /case "done":/);
  assert.match(label, /case "succeeded":/);
  assert.match(label, /"Terminé"/);
  assert.match(label, /case "failed":/);
  assert.match(label, /case "error":/);
  assert.match(label, /"Échec"/);
  assert.match(label, /case "provisioning":/);
  assert.match(label, /case "starting":/);
  assert.match(label, /case "building":/);
  assert.match(label, /"Démarrage"/);
  assert.match(label, /case "deleted":/);
  assert.match(label, /case "archived":/);
  assert.match(label, /"Archivé"/);
  assert.match(label, /default:/);
});

test("statusTone classe les états par tonalité visuelle", () => {
  const tone = block(frontend, "const statusTone", "const formatDate");
  assert.match(tone, /"active"/);
  assert.match(tone, /"done"/);
  assert.match(tone, /"error"/);
  assert.match(tone, /"muted"/);
  assert.match(tone, /case "ready":/);
  assert.match(tone, /case "failed":/);
  assert.match(tone, /case "deleted":/);
  assert.match(tone, /default:/);
  assert.ok(tone.indexOf("return \"active\"") >= 0, "tonalité active");
  assert.ok(tone.indexOf("return \"muted\"") >= 0, "tonalité muette");
});

test("formatDate formate en français et échoue proprement", () => {
  const format = block(frontend, "const formatDate", "export async function refreshFreebuffCloudPanel");
  assert.match(format, /new Intl\.DateTimeFormat\("fr-FR"/);
  assert.match(format, /day: "2-digit"/);
  assert.match(format, /month: "short"/);
  assert.match(format, /hour: "2-digit"/);
  assert.match(format, /minute: "2-digit"/);
  assert.match(format, /if \(!value\) return ""/);
  assert.match(format, /catch \{/);
});

// ─────────────────────────── 15. Panneau : connexion ───────────────────────────

test("le formulaire de connexion guide en 3 étapes et protège le cookie", () => {
  const form = block(frontend, "const renderConnectForm", "const renderCreateDialog");
  assert.match(form, /Connecter ton compte Freebuff Cloud/);
  assert.match(form, /freebuff\.com\/cloud/);
  assert.match(form, /Sign in with GitHub/);
  assert.match(form, /__Secure-next-auth\.session-token/);
  assert.match(form, /data-freebuff-cloud-connect-form/);
  assert.match(form, /data-freebuff-cloud-cookie-input/);
  assert.match(form, /placeholder="eyJhbGciOiJIUzI1NiIs…"/);
  assert.match(form, /spellcheck="false"/);
  assert.match(form, /autocomplete="off"/);
  assert.match(form, /Switch le garde sur le serveur et ne l'affiche plus jamais ensuite/);
  assert.match(form, /connectCookie\.trim\(\)/);
  assert.match(form, /data-close-freebuff-cloud-connect/);
  assert.ok((form.match(/<li><span>\d<\/span>/g) ?? []).length === 3, "3 étapes");
  assert.ok(form.indexOf("F12 → Application → Cookies") >= 0, "instructions navigateur");
});

// ─────────────────────────── 16. Panneau : création de projet ───────────────────────────

test("la boîte de dialogue de création gère les deux modes", () => {
  const dialog = block(frontend, "const renderCreateDialog", "const renderProjectCard");
  assert.match(dialog, /Nouveau projet cloud/);
  assert.match(dialog, /role="dialog"/);
  assert.match(dialog, /aria-modal="true"/);
  assert.match(dialog, /data-freebuff-cloud-mode="blank"/);
  assert.match(dialog, /data-freebuff-cloud-mode="repo"/);
  assert.match(dialog, /Projet vierge/);
  assert.match(dialog, /Depuis un repo GitHub/);
  assert.match(dialog, /data-freebuff-cloud-create-form/);
  assert.match(dialog, /data-freebuff-cloud-create-name/);
  assert.match(dialog, /data-freebuff-cloud-create-repo/);
  assert.match(dialog, /maxlength="80"/);
  assert.match(dialog, /Choisis un repo connectable…/);
  assert.match(dialog, /Aucun repo connectable trouvé/);
  assert.match(dialog, /data-close-freebuff-cloud-create/);
  assert.match(dialog, /repos\.map/);
  assert.ok(dialog.indexOf('${createMode === "blank"') >= 0, "mode conditionnel");
  assert.ok(dialog.indexOf("creating ? \"disabled\" : \"\"") >= 0, "désactivation pendant création");
});

// ─────────────────────────── 17. Carte projet & session ───────────────────────────

test("la carte projet affiche preview, session et suppression", () => {
  const card = block(frontend, "const renderProjectCard", "const renderSessionStream");
  assert.match(card, /data-freebuff-cloud-project/);
  assert.match(card, /freebuff-cloud-project-icon/);
  assert.match(card, /project\.repoFullName \? "github" : "layers"/);
  assert.match(card, /Projet sans nom/);
  assert.match(card, /Projet vierge/);
  assert.match(card, /freebuff-cloud-pill is-\$\{statusTone\(project\.status\)\}/);
  assert.match(card, /statusLabel\(project\.status\)/);
  assert.match(card, /data-freebuff-cloud-stream=/);
  assert.match(card, /data-message-id=/);
  assert.match(card, /data-run-id=/);
  assert.match(card, /data-freebuff-cloud-delete=/);
  assert.match(card, /project\.previewUrl/);
  assert.match(card, /external-link/);
  assert.match(card, /project\.messageId/);
  assert.ok(card.indexOf("Preview") >= 0, "bouton preview");
  assert.ok(card.indexOf("Session") >= 0, "bouton session");
  assert.ok(card.indexOf("Supprimer") >= 0, "bouton supprimer");
});

test("le panneau session est accessible et fermable", () => {
  const session = block(frontend, "const renderSessionStream", "export function renderFreebuffCloudPanel");
  assert.match(session, /data-freebuff-cloud-session/);
  assert.match(session, /Session en direct/);
  assert.match(session, /data-close-freebuff-cloud-session/);
  assert.match(session, /aria-live="polite"/);
  assert.match(session, /data-freebuff-cloud-session-output/);
  assert.match(session, /Connexion au flux de l'agent…/);
  assert.match(session, /is-spinning/);
  assert.ok(session.indexOf("Fermer") >= 0, "bouton fermer");
});

// ─────────────────────────── 18. Rendu du panneau ───────────────────────────

test("renderFreebuffCloudPanel gère loading, erreur, connecté et vide", () => {
  const panel = block(frontend, "export function renderFreebuffCloudPanel", "const openSessionStream");
  assert.match(panel, /renderLoading\(\)/);
  assert.match(panel, /renderLoadFailure\(\)/);
  assert.match(panel, /status\?\.connected === true/);
  assert.match(panel, /Freebuff Cloud/);
  assert.match(panel, /Sandbox cloud gratuit/);
  assert.match(panel, /data-refresh-freebuff-cloud/);
  assert.match(panel, /data-freebuff-cloud-disconnect/);
  assert.match(panel, /data-open-freebuff-cloud-create/);
  assert.match(panel, /renderConnectForm\(\)/);
  assert.match(panel, /Projets cloud/);
  assert.match(panel, /Aucun projet cloud/);
  assert.match(panel, /renderProjectCard/);
  assert.match(panel, /renderSessionStream\(streamingProject\)/);
  assert.match(panel, /renderCreateDialog\(\)/);
  assert.match(panel, /freebuff-cloud-toast/);
  assert.match(panel, /projectList\.map/);
  assert.ok(panel.indexOf("Connecté") >= 0, "badge connecté");
  assert.ok(panel.indexOf("Non connecté") >= 0, "badge déconnecté");
  assert.ok(panel.indexOf("Actualiser") >= 0, "bouton actualiser");
  assert.ok(panel.indexOf("Déconnecter") >= 0, "bouton déconnecter");
});

test("refreshFreebuffCloudPanel charge statut et projets en parallèle", () => {
  const refresh = block(frontend, "export async function refreshFreebuffCloudPanel", "const resetConnectDraft");
  assert.match(refresh, /Promise\.all\(\[/);
  assert.match(refresh, /invoke<FreebuffCloudStatus>\("freebuff_cloud_status"\)/);
  assert.match(refresh, /invoke<unknown>\("freebuff_cloud_projects"\)/);
  assert.match(refresh, /\.catch\(\(\) => null\)/);
  assert.match(refresh, /projectsResult\.map\(normalizeProject\)/);
  assert.match(refresh, /loadError = String\(/);
  assert.match(refresh, /loading = false/);
  assert.match(refresh, /if \(loading && silent\) return;/);
  assert.ok(refresh.indexOf("rerender()") >= 0, "re-rendu après chargement");
});

// ─────────────────────────── 19. Streaming SSE côté client ───────────────────────────

test("openSessionStream consomme le flux SSE du proxy serveur", () => {
  const stream = block(frontend, "const openSessionStream", "export function bindFreebuffCloudPanel");
  assert.match(stream, /\/api\/freebuff-cloud\/stream/);
  assert.match(stream, /new URLSearchParams\(\{ messageId: project\.messageId \|\| "" \}\)/);
  assert.match(stream, /params\.set\("runId", project\.runId\)/);
  assert.match(stream, /new AbortController\(\)/);
  assert.match(stream, /credentials: "same-origin"/);
  assert.match(stream, /response\.body\.getReader\(\)/);
  assert.match(stream, /new TextDecoder\(\)/);
  assert.match(stream, /buffer\.split\("\\n"\)/);
  assert.match(stream, /trimmed\.startsWith\("data:"\)/);
  assert.match(stream, /trimmed\.slice\(5\)\.trim\(\)/);
  assert.match(stream, /JSON\.parse\(data\)/);
  assert.match(stream, /item\.content/);
  assert.match(stream, /line\.textContent = content/);
  assert.match(stream, /statusTone\(statusValue\)/);
  assert.match(stream, /statusLabel\(statusValue\)/);
  assert.match(stream, /Ouvrir la preview/);
  assert.match(stream, /AbortError/);
  assert.match(stream, /Flux indisponible \(HTTP/);
  assert.ok(stream.indexOf("freebuff-cloud-session-line") >= 0, "ligne de session");
  assert.ok(stream.indexOf("freebuff-cloud-session-error") >= 0, "erreur de session");
  assert.ok(stream.indexOf("output.scrollTop = output.scrollHeight") >= 0, "auto-scroll");
});

// ─────────────────────────── 20. Bindings du panneau ───────────────────────────

test("bindFreebuffCloudPanel câble toutes les interactions", () => {
  const bind = frontend.slice(frontend.indexOf("export function bindFreebuffCloudPanel"));
  assert.match(bind, /freebuff-cloud-panel/);
  assert.match(bind, /refreshFreebuffCloudPanel\(rerender\)/);
  assert.match(bind, /data-freebuff-cloud-disconnect/);
  assert.match(bind, /window\.confirm/);
  assert.match(bind, /freebuff_cloud_disconnect/);
  assert.match(bind, /data-close-freebuff-cloud-connect/);
  assert.match(bind, /data-freebuff-cloud-connect-form/);
  assert.match(bind, /data-freebuff-cloud-cookie-input/);
  assert.match(bind, /freebuff_cloud_connect/);
  assert.match(bind, /sessionCookie: connectCookie\.trim\(\)/);
  assert.match(bind, /data-open-freebuff-cloud-create/);
  assert.match(bind, /freebuff_cloud_repos/);
  assert.match(bind, /data-close-freebuff-cloud-create/);
  assert.match(bind, /data-freebuff-cloud-mode/);
  assert.match(bind, /data-freebuff-cloud-create-form/);
  assert.match(bind, /data-freebuff-cloud-create-name/);
  assert.match(bind, /data-freebuff-cloud-create-repo/);
  assert.match(bind, /freebuff_cloud_create_blank/);
  assert.match(bind, /freebuff_cloud_connect_repo/);
  assert.match(bind, /data-freebuff-cloud-delete/);
  assert.match(bind, /freebuff_cloud_delete_project/);
  assert.match(bind, /data-freebuff-cloud-stream/);
  assert.match(bind, /openSessionStream\(project, rerender\)/);
  assert.match(bind, /renderIcons\(root\)/);
  assert.ok(bind.indexOf("showToast(") >= 0, "toasts après actions");
});

// ─────────────────────────── 21. Câblage main.ts ───────────────────────────

test("main.ts déclare, charge et rend la vue freebuff-cloud", () => {
  assert.match(main, /\| "freebuff-cloud"/);
  assert.match(main, /"freebuff-cloud",/);
  assert.match(main, /type FreebuffCloudModule = typeof import\("\.\/freebuff-cloud"\);/);
  assert.match(main, /const loadFreebuffCloudModule = \(\): Promise<FreebuffCloudModule>/);
  assert.match(main, /import\("\.\/freebuff-cloud"\)/);
  assert.match(main, /scheduleStaleChunkRecovery\(error\)/);
  assert.match(main, /"freebuff-cloud": "Freebuff Cloud"/);
  assert.match(main, /view === "freebuff-cloud" && !freebuffCloudModule/);
  assert.match(main, /loadFreebuffCloudModule\(\)/);
  assert.match(main, /Freebuff Cloud indisponible/);
  assert.match(main, /freebuffCloudModule\?\.renderFreebuffCloudPanel\(\) \?\? ""/);
  assert.match(main, /freebuffCloudModule\?\.bindFreebuffCloudPanel\(\{ rerender: render, renderIcons \}\)/);
  assert.match(main, /freebuffCloudModule\?\.refreshFreebuffCloudPanel\(render, true\)/);
  assert.match(main, /data-view="freebuff-cloud"/);
  assert.match(main, /<i data-lucide="cloud"><\/i><span>Freebuff Cloud<\/span>/);
  assert.match(main, /case "freebuff-cloud":/);
  const refreshes = main.match(/refreshFreebuffCloudPanel\(render, true\)/g) ?? [];
  assert.ok(refreshes.length >= 2, `rafraîchissements attendus: ${refreshes.length}`);
  const renders = main.match(/renderFreebuffCloudPanel\(\)/g) ?? [];
  assert.ok(renders.length >= 2, `rendus attendus: ${renders.length}`);
});

test("le panneau Freebuff Cloud n'apparaît qu'en mode distant", () => {
  const menu = block(main, "data-view=\"freebuff-cloud\"", "</button>");
  assert.match(menu, /<i data-lucide="cloud"><\/i><span>Freebuff Cloud<\/span>/);
  assert.match(menu, /data-view="freebuff-cloud">/);
  assert.match(main, /isRemoteMode\(\) \? `<button type="button" role="menuitem" data-view="freebuff-cloud">/);
});

// ─────────────────────────── 22. CSS du panneau ───────────────────────────

test("le CSS couvre le panneau, les états et les variantes", () => {
  assert.match(css, /\.freebuff-cloud-panel \{/);
  assert.match(css, /\.freebuff-cloud-loading/);
  assert.match(css, /\.freebuff-cloud-error/);
  assert.match(css, /\.freebuff-cloud-connect/);
  assert.match(css, /\.freebuff-cloud-steps/);
  assert.match(css, /\.freebuff-cloud-connect-form/);
  assert.match(css, /\.freebuff-cloud-modal/);
  assert.match(css, /\.freebuff-cloud-project/);
  assert.match(css, /\.freebuff-cloud-pill/);
  assert.match(css, /\.freebuff-cloud-session/);
  assert.match(css, /\.freebuff-cloud-toast/);
  assert.match(css, /\.freebuff-cloud-empty/);
  assert.match(css, /is-active/);
  assert.match(css, /\.freebuff-cloud-hero/);
  assert.match(css, /\.freebuff-cloud-project-main/);
  assert.match(css, /\.freebuff-cloud-button/);
  assert.match(css, /\.primary/);
  assert.match(css, /\.secondary/);
  assert.match(css, /\.danger/);
  assert.match(css, /\.ghost/);
  assert.ok(css.indexOf(".freebuff-cloud-panel {") < css.indexOf(".freebuff-cloud-loading"), "ordre du CSS");
});

// ─────────────────────────── 23. Parité bout en bout ───────────────────────────

test("les 8 invokes frontend ont chacun une route serveur et une commande Rust", () => {
  const invokeNames = [...frontend.matchAll(/invoke(?:<[^>]*>)?\("(freebuff_cloud_[a-z_]+)"/g)].map((m) => m[1]);
  const uniqueInvokes = [...new Set(invokeNames)];
  const rustFn = {
    freebuff_cloud_status: "status",
    freebuff_cloud_connect: "connect",
    freebuff_cloud_disconnect: "disconnect",
    freebuff_cloud_projects: "projects",
    freebuff_cloud_create_blank: "create_blank_project",
    freebuff_cloud_connect_repo: "connect_repo",
    freebuff_cloud_delete_project: "delete_project",
    freebuff_cloud_repos: "connectable_repos",
  };
  assert.ok(uniqueInvokes.length >= 8, `8 invokes attendus: ${uniqueInvokes.length}`);
  for (const name of uniqueInvokes) {
    assert.ok(platform.includes(`case "${name}":`), `mapping platform manquant: ${name}`);
    assert.ok(lib.includes(`freebuff_cloud::${rustFn[name]},`), `commande lib manquante: ${name}`);
  }
  for (const route of ["/api/freebuff-cloud/status", "/api/freebuff-cloud/connect", "/api/freebuff-cloud/projects", "/api/freebuff-cloud/repos"]) {
    assert.ok(platform.includes(route), `route plateforme manquante: ${route}`);
    assert.ok(server.includes(route.replace("/api", "")), `route serveur manquante: ${route}`);
  }
  assert.ok(server.includes("/freebuff-cloud/stream"), "route stream manquante côté serveur");
  assert.ok(frontend.includes("/api/freebuff-cloud/stream"), "fetch stream manquant côté frontend");
  assert.ok(frontend.includes("import \"./freebuff-cloud.css\";"), "CSS importé");
  assert.ok(frontend.includes("import { invoke } from \"./platform\";"), "platform importée");
  assert.ok(css.includes(".freebuff-cloud-panel"), "classe panneau dans le CSS");
  assert.ok(server.includes("freebuff_cloud::{self,"), "module importé dans server.rs");
});
