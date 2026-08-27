//! Intégration Freebuff Cloud (https://freebuff.com/cloud) : l'agent de
//! codage cloud gratuit de Freebuff (sandbox cloud + preview live).
//!
//! Freebuff Cloud s'appuie sur deux surfaces :
//! 1. un backend **Convex** (`harmless-tapir-303.convex.cloud`) : les projets,
//!    repos connectables et actions (créer un projet, connecter un repo) sont
//!    des fonctions Convex appelées par API HTTP (`/api/query`, `/api/action`)
//!    avec un token Convex en Bearer ;
//! 2. des routes de session sur freebuff.com (`/api/web/convex-token`,
//!    `/api/web/freebuff-session`) et le streaming des exécutions d'agent via
//!    SSE (`/api/agent-runs/stream?messageId=...&runId=...`).
//!
//! Authentification : Freebuff Cloud utilise la session freebuff.com (login
//! GitHub via NextAuth). Ce module conserve le cookie de session de
//! l'utilisateur dans le fichier de stockage (jamais renvoyé dans une vue), et
//! s'en sert pour (a) obtenir/rafraîchir le token Convex et (b) appeler les
//! routes de session. Le token Convex est mis en cache et rafraîchi à la
//! demande quand l'API Convex répond 401.
//!
//! Invariants de sécurité :
//! - le cookie de session n'apparaît dans aucune vue sérialisée : les vues
//!   n'exposent que l'e-mail, le nom et le login GitHub de l'utilisateur ;
//! - le stockage est écrit avec les permissions les plus restrictives possibles
//!   et ne contient rien d'autre que le cookie, le token Convex et le profil ;
//! - les appels sortants n'ajoutent jamais le cookie à une autre origine que
//!   freebuff.com.

use base64::Engine as _;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    io::ErrorKind,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const STORE_FILE: &str = "freebuff-cloud.json";
const STORE_VERSION: u32 = 1;

const FREEBUFF_BASE: &str = "https://freebuff.com";
const CONVEX_DEPLOYMENT: &str = "https://harmless-tapir-303.convex.cloud";
const SESSION_URL: &str = "https://freebuff.com/api/web/freebuff-session";
const CONVEX_TOKEN_URL: &str = "https://freebuff.com/api/web/convex-token";
const AGENT_RUNS_STREAM_PATH: &str = "/api/agent-runs/stream";
/// Nom du cookie de session NextAuth de freebuff.com (préfixe `__Secure-` car
/// le site est servi en HTTPS). La vue invite à copier seulement sa *valeur* ;
/// ce nom est réappliqué avant d'envoyer l'en-tête `Cookie`.
const SESSION_COOKIE_NAME: &str = "__Secure-next-auth.session-token";

/// Durée de vie maximale accordée au token Convex en cache (secondes). Le JWT
/// délivré expire après 10 minutes : on le rafraîchit bien avant (4 minutes)
/// pour éviter les 401 en vol. Un 401 éventuel déclenche quand même un
/// rafraîchissement.
const CONVEX_TOKEN_CACHE_SECS: u64 = 4 * 60;
const HTTP_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FreebuffCloudStore {
    version: u32,
    session_cookie: Option<String>,
    convex_token: Option<String>,
    convex_token_fetched_at: Option<i64>,
    user_email: Option<String>,
    user_name: Option<String>,
    github_login: Option<String>,
    instance_id: Option<String>,
}

impl Default for FreebuffCloudStore {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            session_cookie: None,
            convex_token: None,
            convex_token_fetched_at: None,
            user_email: None,
            user_name: None,
            github_login: None,
            instance_id: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FreebuffCloudStatusView {
    pub connected: bool,
    pub email: Option<String>,
    pub name: Option<String>,
    pub github_login: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FreebuffCloudConnectView {
    pub connected: bool,
    pub email: Option<String>,
    pub name: Option<String>,
    pub github_login: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectFreebuffCloudRequest {
    pub session_cookie: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBlankProjectRequest {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectRepoRequest {
    pub repo_full_name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteProjectRequest {
    pub project_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunStreamQuery {
    pub message_id: String,
    pub run_id: Option<String>,
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn store_path() -> Result<PathBuf, String> {
    crate::settings::runtime_data_path(STORE_FILE)
}

fn load_store() -> Result<FreebuffCloudStore, String> {
    let path = store_path()?;
    match fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str::<FreebuffCloudStore>(&content)
            .map_err(|error| format!("Fichier Freebuff Cloud invalide : {error}")),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(FreebuffCloudStore::default()),
        Err(error) => Err(format!("Lecture du fichier Freebuff Cloud impossible : {error}")),
    }
}

fn save_store(store: &FreebuffCloudStore) -> Result<(), String> {
    let path = store_path()?;
    let content = serde_json::to_string_pretty(store)
        .map_err(|error| format!("Serialisation Freebuff Cloud impossible : {error}"))?;
    fs::write(&path, content)
        .map_err(|error| format!("Écriture du fichier Freebuff Cloud impossible : {error}"))?;
    // Restreindre les permissions : le cookie est un secret.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn http_client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|_| "Client HTTP Freebuff Cloud indisponible".to_string())
}

/// Lis une valeur texte parmi plusieurs chemins possibles (tolérant aux
/// variations de schéma du backend).
fn pick_string<'a>(value: &'a Value, paths: &[&str]) -> Option<String> {
    paths.iter().find_map(|path| {
        value
            .pointer(path)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|field| !field.is_empty())
            .map(ToString::to_string)
    })
}

/// Profil minimal porté par le JWT du token Convex (claims `email` et `name`).
#[derive(Debug, Clone, Default)]
struct ConvexTokenProfile {
    email: Option<String>,
    name: Option<String>,
}

/// `/api/web/freebuff-session` n'expose pas le profil utilisateur : l'e-mail et
/// le nom sont portés par le JWT du token Convex. On décode son payload sans
/// vérifier la signature — l'affichage seul en dépend, l'autorisation reste
/// portée par le cookie de session.
fn convex_token_profile(token: &str) -> ConvexTokenProfile {
    let Some(payload) = token.split('.').nth(1) else {
        return ConvexTokenProfile::default();
    };
    let Ok(decoded) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload) else {
        return ConvexTokenProfile::default();
    };
    let Ok(value) = serde_json::from_slice::<Value>(&decoded) else {
        return ConvexTokenProfile::default();
    };
    ConvexTokenProfile {
        email: pick_string(&value, &["/email"]),
        name: pick_string(&value, &["/name"]),
    }
}

fn extract_user_profile(session: &Value) -> (Option<String>, Option<String>, Option<String>, Option<String>) {
    let email = pick_string(
        session,
        &[
            "/user/email",
            "/user/email_address",
            "/email",
            "/profile/email",
            "/user/profile/email",
        ],
    );
    let name = pick_string(session, &["/user/name", "/name", "/user/full_name", "/user/displayName"]);
    let github = pick_string(
        session,
        &[
            "/user/github_login",
            "/user/githubLogin",
            "/github/login",
            "/github_login",
            "/user/github/login",
        ],
    );
    let instance_id = pick_string(session, &["/instanceId", "/instance_id"]);
    (email, name, github, instance_id)
}

/// Normalise le cookie de session collé par l'utilisateur : il peut contenir
/// soit la simple valeur du cookie (`eyJhbGci…`), soit la paire complète
/// `nom=valeur`, soit tout l'en-tête `Cookie` copié depuis DevTools. On garantit
/// un en-tête `Cookie: __Secure-next-auth.session-token=…` valide — sans le nom,
/// freebuff.com ne reconnaît aucune session et répond 401 Unauthorized.
fn normalize_session_cookie(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // Un JWT (base64url sans padding) ne contient jamais `=` : sa présence
    // signale une paire `nom=valeur` (ou un en-tête Cookie complet) déjà
    // exploitable telle quelle.
    if let Some((name, value)) = trimmed.split_once('=') {
        let name = name.trim();
        let value = value.trim();
        if !name.is_empty() && !value.is_empty() {
            return format!("{name}={value}");
        }
    }
    format!("{SESSION_COOKIE_NAME}={trimmed}")
}

/// Récupère la session freebuff.com (valide le cookie).
async fn fetch_session(cookie: &str) -> Result<Value, String> {
    let client = http_client()?;
    let response = client
        .get(SESSION_URL)
        .header("cookie", normalize_session_cookie(cookie))
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|_| "Freebuff Cloud est injoignable".to_string())?;
    let status = response.status();
    let payload: Value = response
        .json()
        .await
        .map_err(|_| "Réponse de session Freebuff Cloud illisible".to_string())?;
    if !status.is_success() {
        let message = pick_string(&payload, &["/error", "/message"])
            .unwrap_or_else(|| "Session Freebuff Cloud invalide ou expirée".to_string());
        return Err(message);
    }
    Ok(payload)
}

/// Obtient (ou rafraîchit) le token Convex à partir du cookie de session.
async fn fetch_convex_token(cookie: &str) -> Result<String, String> {
    let client = http_client()?;
    let response = client
        .get(CONVEX_TOKEN_URL)
        .header("cookie", normalize_session_cookie(cookie))
        .header("accept", "application/json")
        .header("x-fb-timezone", "UTC")
        .header("x-fb-tz-offset", "0")
        .header("x-fb-languages", "fr-FR,fr,en")
        .send()
        .await
        .map_err(|_| "Freebuff Cloud est injoignable".to_string())?;
    let status = response.status();
    let payload: Value = response
        .json()
        .await
        .map_err(|_| "Réponse du token Convex illisible".to_string())?;
    if !status.is_success() {
        return Err(pick_string(&payload, &["/error", "/message"])
            .unwrap_or_else(|| "Token Convex Freebuff Cloud refusé".to_string()));
    }
    let token = pick_string(
        &payload,
        &["/token", "/value", "/tokenValue", "/convexToken", "/convex_token"],
    )
    .or_else(|| payload.as_str().map(str::trim).map(ToString::to_string))
    .filter(|token| !token.is_empty())
    .ok_or_else(|| "Token Convex Freebuff Cloud absent de la réponse".to_string())?;
    Ok(token)
}

/// Résout le token Convex depuis le cache ou le rafraîchit via le cookie.
async fn resolve_convex_token(store: &FreebuffCloudStore) -> Result<String, String> {
    let cookie = store
        .session_cookie
        .as_deref()
        .ok_or_else(|| "Compte Freebuff Cloud non connecté".to_string())?;
    let fresh = store
        .convex_token_fetched_at
        .is_some_and(|fetched| now_unix() - fetched < CONVEX_TOKEN_CACHE_SECS as i64);
    if fresh {
        if let Some(token) = store.convex_token.as_deref().filter(|t| !t.is_empty()) {
            return Ok(token.to_string());
        }
    }
    let token = fetch_convex_token(cookie).await?;
    let mut updated = store.clone();
    updated.convex_token = Some(token.clone());
    updated.convex_token_fetched_at = Some(now_unix());
    save_store(&updated)?;
    Ok(token)
}

/// Appelle une fonction Convex (query ou action) avec rafraîchissement du
/// token en cas de 401.
async fn convex_call(kind: &str, path: &str, args: Value) -> Result<Value, String> {
    let store = load_store()?;
    let token = resolve_convex_token(&store).await?;
    let result = convex_call_with_token(kind, path, &args, &token).await;
    match result {
        Ok(value) => Ok(value),
        Err(error) if error.starts_with("token_convex_expire") => {
            // Token révoqué : rafraîchit une fois puis réessaie.
            let cookie = store
                .session_cookie
                .as_deref()
                .ok_or_else(|| "Compte Freebuff Cloud non connecté".to_string())?;
            let refreshed = fetch_convex_token(cookie).await?;
            let mut updated = store.clone();
            updated.convex_token = Some(refreshed.clone());
            updated.convex_token_fetched_at = Some(now_unix());
            save_store(&updated)?;
            convex_call_with_token(kind, path, &args, &refreshed).await
        }
        Err(error) => Err(error),
    }
}

async fn convex_call_with_token(
    kind: &str,
    path: &str,
    args: &Value,
    token: &str,
) -> Result<Value, String> {
    let client = http_client()?;
    let url = format!("{CONVEX_DEPLOYMENT}/api/{kind}");
    let body = json!({
        "path": path,
        "format": "json",
        "args": args,
        "formatVersion": 2,
    });
    let response = client
        .post(&url)
        .bearer_auth(token)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|_| "Backend Convex Freebuff Cloud injoignable".to_string())?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err("token_convex_expire".to_string());
    }
    if !response.status().is_success() {
        return Err(format!(
            "Backend Convex Freebuff Cloud indisponible (HTTP {})",
            response.status().as_u16()
        ));
    }
    let payload: Value = response
        .json()
        .await
        .map_err(|_| "Réponse Convex Freebuff Cloud illisible".to_string())?;
    match payload.get("status").and_then(Value::as_str) {
        Some("success") => Ok(payload.get("value").cloned().unwrap_or(Value::Null)),
        Some("error") => Err(pick_string(&payload, &["/errorMessage", "/message"])
            .unwrap_or_else(|| "Erreur Convex Freebuff Cloud".to_string())),
        _ => Err("Réponse Convex Freebuff Cloud invalide".to_string()),
    }
}

/// Connexion : valide le cookie, récupère le profil et le token Convex.
#[cfg_attr(feature = "desktop", tauri::command(rename_all = "camelCase"))]
pub async fn connect(session_cookie: String) -> Result<FreebuffCloudConnectView, String> {
    let cookie = normalize_session_cookie(&session_cookie);
    if cookie.is_empty() {
        return Err("Colle le cookie de session freebuff.com".to_string());
    }
    let session = fetch_session(&cookie).await?;
    let (mut email, mut name, github, instance_id) = extract_user_profile(&session);
    let token = fetch_convex_token(&cookie).await?;
    if email.is_none() || name.is_none() {
        let profile = convex_token_profile(&token);
        if email.is_none() {
            email = profile.email;
        }
        if name.is_none() {
            name = profile.name;
        }
    }
    let mut store = FreebuffCloudStore::default();
    store.session_cookie = Some(cookie);
    store.convex_token = Some(token);
    store.convex_token_fetched_at = Some(now_unix());
    store.user_email = email.clone();
    store.user_name = name.clone();
    store.github_login = github.clone();
    store.instance_id = instance_id;
    save_store(&store)?;
    Ok(FreebuffCloudConnectView {
        connected: true,
        email,
        name,
        github_login: github,
        message: "Compte Freebuff Cloud connecté".to_string(),
    })
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub fn disconnect() -> Result<(), String> {
    let path = store_path()?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Suppression du compte Freebuff Cloud impossible : {error}")),
    }
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub fn status() -> FreebuffCloudStatusView {
    let store = match load_store() {
        Ok(store) => store,
        Err(error) => {
            return FreebuffCloudStatusView {
                connected: false,
                email: None,
                name: None,
                github_login: None,
                message: Some(error),
            }
        }
    };
    let connected = store.session_cookie.as_deref().is_some_and(|c| !c.is_empty());
    FreebuffCloudStatusView {
        connected,
        email: store.user_email,
        name: store.user_name,
        github_login: store.github_login,
        message: None,
    }
}

/// Liste des projets de l'utilisateur (fonction Convex `project:getUserProjects`).
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn projects() -> Result<Vec<Value>, String> {
    // Mêmes arguments que l'application freebuff.com/cloud : la liste cloud,
    // sans les projets archivés.
    let value = convex_call(
        "query",
        "project:getUserProjects",
        json!({ "surface": "cloud", "archivedOnly": false }),
    )
    .await?;
    match value {
        Value::Array(projects) => Ok(projects),
        Value::Null => Ok(Vec::new()),
        _ => Err("Réponse de projets Freebuff Cloud invalide".to_string()),
    }
}

/// Crée un projet vierge.
#[cfg_attr(feature = "desktop", tauri::command(rename_all = "camelCase"))]
pub async fn create_blank_project(name: String) -> Result<Value, String> {
    convex_call("action", "cloud/blankProject:createBlankProject", json!({ "name": name })).await
}

/// Connecte un repo GitHub existant à un nouveau sandbox.
#[cfg_attr(feature = "desktop", tauri::command(rename_all = "camelCase"))]
pub async fn connect_repo(repo_full_name: String) -> Result<Value, String> {
    convex_call(
        "action",
        "cloud/connectRepo:connectRepo",
        json!({ "repoFullName": repo_full_name }),
    )
    .await
}

/// Supprime un projet.
#[cfg_attr(feature = "desktop", tauri::command(rename_all = "camelCase"))]
pub async fn delete_project(project_id: String) -> Result<Value, String> {
    convex_call(
        "action",
        "project:deleteProject",
        json!({ "projectId": project_id }),
    )
    .await
}

/// Repos GitHub connectables : rafraîchit puis lit le cache exposé.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn connectable_repos() -> Result<Value, String> {
    let refreshed = convex_call(
        "action",
        "github/cloudRepos:refreshConnectableRepositories",
        json!({}),
    )
    .await;
    if let Ok(value) = refreshed {
        if !matches!(value, Value::Null) {
            return Ok(value);
        }
    }
    convex_call(
        "action",
        "github/repoCacheStore:getCachedConnectableRepositories",
        json!({}),
    )
    .await
}

/// URL du flux SSE d'une exécution d'agent (proxy serveur).
pub fn agent_run_stream_url(message_id: &str, run_id: Option<&str>) -> String {
    let mut url = format!(
        "{FREEBUFF_BASE}{AGENT_RUNS_STREAM_PATH}?messageId={}",
        urlencode(message_id)
    );
    if let Some(run_id) = run_id.filter(|value| !value.is_empty()) {
        url.push_str(&format!("&runId={}", urlencode(run_id)));
    }
    url
}

/// Cookie de session pour le proxy SSE (jamais exposé dans une vue).
pub fn session_cookie() -> Result<String, String> {
    let store = load_store()?;
    let cookie = store
        .session_cookie
        .ok_or_else(|| "Compte Freebuff Cloud non connecté".to_string())?;
    // Protège aussi un fichier de stockage écrit par une ancienne version qui
    // aurait gardé la valeur seule sans le nom du cookie.
    Ok(normalize_session_cookie(&cookie))
}

fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
