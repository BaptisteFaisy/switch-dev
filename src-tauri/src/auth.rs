use crate::{
    fs_util,
    runtime_sync::{RuntimeSync, RuntimeSyncTopic},
    security::constant_time_eq,
};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::{Query, State},
    http::{
        header::{CACHE_CONTROL, LOCATION, SET_COOKIE},
        HeaderMap, HeaderValue, StatusCode,
    },
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use url::Url;
use uuid::Uuid;

const STORE_VERSION: u32 = 1;
const SESSION_COOKIE: &str = "cst_session";
const OAUTH_STATE_COOKIE: &str = "cst_oauth_state";
const SESSION_DURATION_SECS: i64 = 30 * 24 * 60 * 60;
const OAUTH_STATE_DURATION_SECS: i64 = 10 * 60;
const MAX_SESSIONS_PER_USER: usize = 12;

#[derive(Clone)]
pub(crate) struct AuthManager {
    inner: Arc<Mutex<AuthState>>,
    config: Arc<AuthRuntimeConfig>,
    http: reqwest::Client,
    runtime_sync: Option<RuntimeSync>,
}

struct AuthRuntimeConfig {
    store_path: PathBuf,
    registration_enabled: bool,
    secure_cookie: bool,
    google: Option<GoogleConfig>,
    dummy_password_hash: String,
}

#[derive(Clone)]
struct GoogleConfig {
    client_id: String,
    client_secret: String,
    redirect_uri: String,
    login_url: String,
}

struct AuthState {
    store: AuthStore,
    pending_google: HashMap<String, PendingGoogleLogin>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthStore {
    version: u32,
    #[serde(default)]
    users: Vec<StoredUser>,
    #[serde(default)]
    sessions: Vec<StoredSession>,
}

impl Default for AuthStore {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            users: Vec::new(),
            sessions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredUser {
    id: String,
    username: String,
    email: String,
    #[serde(default)]
    password_hash: Option<String>,
    #[serde(default)]
    google_sub: Option<String>,
    #[serde(default)]
    avatar_url: Option<String>,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredSession {
    token_hash: String,
    user_id: String,
    created_at: i64,
    expires_at: i64,
}

struct PendingGoogleLogin {
    code_verifier: String,
    expires_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthUser {
    id: String,
    username: String,
    email: String,
    avatar_url: Option<String>,
    has_password: bool,
    google_linked: bool,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct AuthIdentity {
    pub id: String,
    pub username: String,
    pub avatar_url: Option<String>,
}

impl From<&StoredUser> for AuthUser {
    fn from(user: &StoredUser) -> Self {
        Self {
            id: user.id.clone(),
            username: user.username.clone(),
            email: user.email.clone(),
            avatar_url: user.avatar_url.clone(),
            has_password: user.password_hash.is_some(),
            google_linked: user.google_sub.is_some(),
            created_at: user.created_at,
            updated_at: user.updated_at,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicAuthConfig {
    enabled: bool,
    registration_enabled: bool,
    google_enabled: bool,
    google_login_url: Option<String>,
}

#[derive(Serialize)]
struct SessionResponse {
    user: AuthUser,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterRequest {
    username: String,
    email: String,
    password: String,
}

#[derive(Deserialize)]
struct LoginRequest {
    identifier: String,
    password: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateProfileRequest {
    username: String,
    email: String,
    #[serde(default)]
    current_password: Option<String>,
}

#[derive(Deserialize)]
struct GoogleCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct GoogleTokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct GoogleUserInfo {
    sub: String,
    email: String,
    #[serde(default)]
    email_verified: bool,
    #[serde(default)]
    picture: Option<String>,
}

#[derive(Debug)]
struct AuthError {
    status: StatusCode,
    message: String,
}

impl AuthError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, message)
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        no_store(
            (
                self.status,
                Json(json!({
                    "error": {
                        "message": self.message,
                        "code": self.status.as_u16()
                    }
                })),
            )
                .into_response(),
        )
    }
}

pub(crate) fn router(manager: AuthManager) -> Router {
    Router::new()
        .route("/config", get(api_config))
        .route("/register", post(api_register))
        .route("/login", post(api_login))
        .route("/logout", post(api_logout))
        .route("/me", get(api_me))
        .route("/profile", put(api_update_profile))
        .route("/google/start", get(api_google_start))
        .route("/google/callback", get(api_google_callback))
        .with_state(manager)
}

impl AuthManager {
    pub(crate) fn load(data_dir: PathBuf, public_base_url: &str) -> Result<Self, String> {
        let store_path = data_dir.join("user-auth.json");
        let store = if store_path.exists() {
            let content = fs::read_to_string(&store_path).map_err(|error| {
                format!("lecture de {} impossible: {error}", store_path.display())
            })?;
            let mut parsed: AuthStore = serde_json::from_str(&content)
                .map_err(|error| format!("{} invalide: {error}", store_path.display()))?;
            if parsed.version == 0 {
                parsed.version = STORE_VERSION;
            }
            if parsed.version > STORE_VERSION {
                return Err(format!(
                    "{} utilise une version d'authentification plus recente ({})",
                    store_path.display(),
                    parsed.version
                ));
            }
            parsed
        } else {
            AuthStore::default()
        };

        let client_id = env_trimmed("CST_GOOGLE_CLIENT_ID");
        let client_secret = env_trimmed("CST_GOOGLE_CLIENT_SECRET");
        let google = match (client_id, client_secret) {
            (Some(client_id), Some(client_secret)) => {
                let redirect_uri = env_trimmed("CST_GOOGLE_REDIRECT_URI").unwrap_or_else(|| {
                    format!(
                        "{}/api/auth/google/callback",
                        public_base_url.trim_end_matches('/')
                    )
                });
                let login_url = google_login_url(&redirect_uri)?;
                Some(GoogleConfig {
                    client_id,
                    client_secret,
                    redirect_uri,
                    login_url,
                })
            }
            (None, None) => None,
            _ => return Err(
                "CST_GOOGLE_CLIENT_ID et CST_GOOGLE_CLIENT_SECRET doivent etre definis ensemble"
                    .to_string(),
            ),
        };

        let secure_cookie = env_bool("CST_AUTH_SECURE_COOKIE")
            .unwrap_or_else(|| public_base_url.trim().starts_with("https://"));
        let registration_enabled = env_bool("CST_ALLOW_REGISTRATION").unwrap_or(true);
        let dummy_password_hash = hash_password(&random_secret())?;

        let manager = Self {
            inner: Arc::new(Mutex::new(AuthState {
                store,
                pending_google: HashMap::new(),
            })),
            config: Arc::new(AuthRuntimeConfig {
                store_path,
                registration_enabled,
                secure_cookie,
                google,
                dummy_password_hash,
            }),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .map_err(|error| format!("client OAuth impossible: {error}"))?,
            runtime_sync: None,
        };

        let mut state = manager
            .inner
            .lock()
            .map_err(|_| "verrou d'authentification empoisonne".to_string())?;
        if prune_expired_sessions(&mut state.store.sessions, now_ts()) {
            manager.persist_locked(&state)?;
        }
        drop(state);
        Ok(manager)
    }

    pub(crate) fn with_runtime_sync(mut self, runtime_sync: RuntimeSync) -> Self {
        self.runtime_sync = Some(runtime_sync);
        self
    }

    pub(crate) fn authorize_headers(&self, headers: &HeaderMap) -> bool {
        self.user_from_headers(headers).ok().flatten().is_some()
    }

    /// Identite publique associee au cookie de session. Les fonctionnalites
    /// collaboratives (comme le forum) n'ont ainsi jamais acces a l'e-mail ni
    /// aux secrets d'authentification de l'utilisateur.
    pub(crate) fn identity_from_headers(
        &self,
        headers: &HeaderMap,
    ) -> Result<Option<AuthIdentity>, String> {
        self.user_from_headers(headers)
            .map(|user| {
                user.map(|user| AuthIdentity {
                    id: user.id,
                    username: user.username,
                    avatar_url: user.avatar_url,
                })
            })
            .map_err(|error| error.message)
    }

    /// Catalogue public des utilisateurs pour les fonctions collaboratives.
    /// Les e-mails, fournisseurs OAuth et secrets restent dans le module auth.
    /// Retourne l'e-mail interne d'un utilisateur authentifie pour les controles
    /// d'appartenance cote serveur. Ce champ ne fait jamais partie d'une identite
    /// publique ni d'une reponse collaborative.
    pub(crate) fn email_for_user_id(&self, user_id: &str) -> Result<Option<String>, String> {
        let state = self.lock().map_err(|error| error.message)?;
        Ok(state
            .store
            .users
            .iter()
            .find(|user| user.id == user_id)
            .map(|user| user.email.clone()))
    }

    pub(crate) fn public_identities(&self) -> Result<Vec<AuthIdentity>, String> {
        let state = self.lock().map_err(|error| error.message)?;
        let mut identities = state
            .store
            .users
            .iter()
            .map(|user| AuthIdentity {
                id: user.id.clone(),
                username: user.username.clone(),
                avatar_url: user.avatar_url.clone(),
            })
            .collect::<Vec<_>>();
        identities.sort_by(|left, right| {
            left.username
                .to_lowercase()
                .cmp(&right.username.to_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(identities)
    }

    pub(crate) fn public_identity_by_id(
        &self,
        user_id: &str,
    ) -> Result<Option<AuthIdentity>, String> {
        let state = self.lock().map_err(|error| error.message)?;
        Ok(state
            .store
            .users
            .iter()
            .find(|user| user.id == user_id)
            .map(|user| AuthIdentity {
                id: user.id.clone(),
                username: user.username.clone(),
                avatar_url: user.avatar_url.clone(),
            }))
    }

    /// Résout un identifiant de compte (id exact, sinon nom d'utilisateur sans
    /// tenir compte de la casse) vers l'id de l'utilisateur. Permet au jeton
    /// administrateur de viser la liste de tâches d'un compte précis.
    pub(crate) fn user_id_by_username(&self, identifier: &str) -> Result<Option<String>, String> {
        let state = self.lock().map_err(|error| error.message)?;
        let identifier = identifier.trim();
        if identifier.is_empty() {
            return Ok(None);
        }
        Ok(state
            .store
            .users
            .iter()
            .find(|user| {
                user.id == identifier
                    || user.username.to_lowercase() == identifier.to_lowercase()
            })
            .map(|user| user.id.clone()))
    }

    fn public_config(&self) -> Result<PublicAuthConfig, AuthError> {
        let state = self.lock()?;
        Ok(PublicAuthConfig {
            enabled: true,
            registration_enabled: self.config.registration_enabled || state.store.users.is_empty(),
            google_enabled: self.config.google.is_some(),
            google_login_url: self
                .config
                .google
                .as_ref()
                .map(|google| google.login_url.clone()),
        })
    }

    fn register(&self, request: RegisterRequest) -> Result<(AuthUser, String), AuthError> {
        let username = normalize_username(&request.username)?;
        let email = normalize_email(&request.email)?;
        validate_password(&request.password)?;
        let password_hash = hash_password(&request.password).map_err(AuthError::internal)?;
        let now = now_ts();

        let mut state = self.lock()?;
        if !self.config.registration_enabled && !state.store.users.is_empty() {
            return Err(AuthError::new(
                StatusCode::FORBIDDEN,
                "La creation de nouveaux comptes est desactivee",
            ));
        }
        ensure_identity_available(&state.store.users, &username, &email, None)?;

        let stored = StoredUser {
            id: Uuid::new_v4().to_string(),
            username,
            email,
            password_hash: Some(password_hash),
            google_sub: None,
            avatar_url: None,
            created_at: now,
            updated_at: now,
        };
        let user = AuthUser::from(&stored);
        let user_id = stored.id.clone();
        state.store.users.push(stored);
        let token = issue_session(&mut state.store.sessions, &user_id, now);
        self.persist_locked(&state).map_err(AuthError::internal)?;
        drop(state);
        self.notify_public_identities_changed();
        Ok((user, token))
    }

    fn login(&self, request: LoginRequest) -> Result<(AuthUser, String), AuthError> {
        let identifier = request.identifier.trim().to_lowercase();
        if identifier.is_empty() || request.password.is_empty() {
            return Err(AuthError::unauthorized("Identifiants invalides"));
        }

        let (candidate, password_hash) = {
            let state = self.lock()?;
            let candidate = state
                .store
                .users
                .iter()
                .find(|user| {
                    user.email.eq_ignore_ascii_case(&identifier)
                        || user.username.eq_ignore_ascii_case(&identifier)
                })
                .cloned();
            let password_hash = candidate
                .as_ref()
                .and_then(|user| user.password_hash.clone())
                .unwrap_or_else(|| self.config.dummy_password_hash.clone());
            (candidate, password_hash)
        };

        let password_matches = verify_password(&password_hash, &request.password);
        let Some(candidate) = candidate else {
            return Err(AuthError::unauthorized("Identifiants invalides"));
        };
        if candidate.password_hash.is_none() {
            return Err(AuthError::unauthorized(
                "Ce compte utilise Google. Connectez-vous avec Google.",
            ));
        }
        if !password_matches {
            return Err(AuthError::unauthorized("Identifiants invalides"));
        }

        let mut state = self.lock()?;
        let stored = state
            .store
            .users
            .iter()
            .find(|user| user.id == candidate.id)
            .ok_or_else(|| AuthError::unauthorized("Compte introuvable"))?
            .clone();
        let token = issue_session(&mut state.store.sessions, &stored.id, now_ts());
        self.persist_locked(&state).map_err(AuthError::internal)?;
        Ok((AuthUser::from(&stored), token))
    }

    fn user_from_headers(&self, headers: &HeaderMap) -> Result<Option<AuthUser>, AuthError> {
        let tokens = session_tokens(headers);
        if tokens.is_empty() {
            return Ok(None);
        }

        let now = now_ts();
        let mut state = self.lock()?;
        let pruned = prune_expired_sessions(&mut state.store.sessions, now);
        let user_id = tokens.iter().find_map(|token| {
            let expected_hash = hash_token(token);
            state
                .store
                .sessions
                .iter()
                .find(|session| {
                    session.expires_at > now
                        && constant_time_eq(session.token_hash.as_bytes(), expected_hash.as_bytes())
                })
                .map(|session| session.user_id.clone())
        });
        if pruned {
            self.persist_locked(&state).map_err(AuthError::internal)?;
        }
        Ok(user_id.and_then(|id| {
            state
                .store
                .users
                .iter()
                .find(|user| user.id == id)
                .map(AuthUser::from)
        }))
    }

    fn update_profile(
        &self,
        headers: &HeaderMap,
        request: UpdateProfileRequest,
    ) -> Result<AuthUser, AuthError> {
        let authenticated = self
            .user_from_headers(headers)?
            .ok_or_else(|| AuthError::unauthorized("Session expiree"))?;
        let username = normalize_username(&request.username)?;
        let email = normalize_email(&request.email)?;

        let mut state = self.lock()?;
        ensure_identity_available(
            &state.store.users,
            &username,
            &email,
            Some(&authenticated.id),
        )?;
        let index = state
            .store
            .users
            .iter()
            .position(|user| user.id == authenticated.id)
            .ok_or_else(|| AuthError::unauthorized("Compte introuvable"))?;

        let email_changed = !state.store.users[index].email.eq_ignore_ascii_case(&email);
        if email_changed {
            if let Some(password_hash) = state.store.users[index].password_hash.as_deref() {
                let provided = request.current_password.as_deref().unwrap_or("");
                if !verify_password(password_hash, provided) {
                    return Err(AuthError::unauthorized(
                        "Le mot de passe actuel est requis pour modifier l'adresse e-mail",
                    ));
                }
            }
        }

        let user = &mut state.store.users[index];
        user.username = username;
        user.email = email;
        user.updated_at = now_ts();
        let public = AuthUser::from(&*user);
        self.persist_locked(&state).map_err(AuthError::internal)?;
        drop(state);
        self.notify_public_identities_changed();
        Ok(public)
    }

    fn logout(&self, headers: &HeaderMap) -> Result<(), AuthError> {
        let token_hashes = session_tokens(headers)
            .into_iter()
            .map(|token| hash_token(&token))
            .collect::<Vec<_>>();
        if token_hashes.is_empty() {
            return Ok(());
        }
        let mut state = self.lock()?;
        let previous_len = state.store.sessions.len();
        state.store.sessions.retain(|session| {
            !token_hashes
                .iter()
                .any(|hash| constant_time_eq(session.token_hash.as_bytes(), hash.as_bytes()))
        });
        if state.store.sessions.len() != previous_len {
            self.persist_locked(&state).map_err(AuthError::internal)?;
        }
        Ok(())
    }

    fn begin_google(&self) -> Result<(String, String), AuthError> {
        let google = self.config.google.as_ref().ok_or_else(|| {
            AuthError::new(StatusCode::NOT_FOUND, "Connexion Google non configuree")
        })?;
        let state_token = random_secret();
        let code_verifier = format!("{}{}", random_secret(), random_secret()).replace('-', "");
        let code_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()));
        let now = now_ts();

        let mut state = self.lock()?;
        state
            .pending_google
            .retain(|_, pending| pending.expires_at > now);
        state.pending_google.insert(
            hash_token(&state_token),
            PendingGoogleLogin {
                code_verifier,
                expires_at: now + OAUTH_STATE_DURATION_SECS,
            },
        );

        let mut url = Url::parse("https://accounts.google.com/o/oauth2/v2/auth")
            .map_err(|error| AuthError::internal(error.to_string()))?;
        url.query_pairs_mut()
            .append_pair("client_id", &google.client_id)
            .append_pair("redirect_uri", &google.redirect_uri)
            .append_pair("response_type", "code")
            .append_pair("scope", "openid email profile")
            .append_pair("state", &state_token)
            .append_pair("code_challenge", &code_challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("prompt", "select_account");
        Ok((url.into(), state_token))
    }

    async fn finish_google(
        &self,
        headers: &HeaderMap,
        code: &str,
        state_token: &str,
    ) -> Result<(AuthUser, String), AuthError> {
        let google = self
            .config
            .google
            .as_ref()
            .ok_or_else(|| {
                AuthError::new(StatusCode::NOT_FOUND, "Connexion Google non configuree")
            })?
            .clone();
        let cookie_state = cookie_value(headers, OAUTH_STATE_COOKIE)
            .ok_or_else(|| AuthError::bad_request("Session OAuth Google absente"))?;
        if !constant_time_eq(cookie_state.as_bytes(), state_token.as_bytes()) {
            return Err(AuthError::bad_request("Etat OAuth Google invalide"));
        }

        let pending = {
            let mut auth_state = self.lock()?;
            auth_state
                .pending_google
                .remove(&hash_token(state_token))
                .filter(|pending| pending.expires_at > now_ts())
                .ok_or_else(|| AuthError::bad_request("Session OAuth Google expiree"))?
        };

        let token_response = self
            .http
            .post("https://oauth2.googleapis.com/token")
            .form(&[
                ("client_id", google.client_id.as_str()),
                ("client_secret", google.client_secret.as_str()),
                ("code", code),
                ("code_verifier", pending.code_verifier.as_str()),
                ("grant_type", "authorization_code"),
                ("redirect_uri", google.redirect_uri.as_str()),
            ])
            .send()
            .await
            .map_err(|_| AuthError::new(StatusCode::BAD_GATEWAY, "Google ne repond pas"))?;
        if !token_response.status().is_success() {
            return Err(AuthError::new(
                StatusCode::BAD_GATEWAY,
                "Google a refuse le code d'autorisation",
            ));
        }
        let token: GoogleTokenResponse = token_response
            .json()
            .await
            .map_err(|_| AuthError::new(StatusCode::BAD_GATEWAY, "Reponse Google invalide"))?;

        let user_response = self
            .http
            .get("https://openidconnect.googleapis.com/v1/userinfo")
            .bearer_auth(&token.access_token)
            .send()
            .await
            .map_err(|_| AuthError::new(StatusCode::BAD_GATEWAY, "Profil Google inaccessible"))?;
        if !user_response.status().is_success() {
            return Err(AuthError::new(
                StatusCode::BAD_GATEWAY,
                "Google n'a pas retourne le profil demande",
            ));
        }
        let google_user: GoogleUserInfo = user_response
            .json()
            .await
            .map_err(|_| AuthError::new(StatusCode::BAD_GATEWAY, "Profil Google invalide"))?;
        if google_user.sub.trim().is_empty() || !google_user.email_verified {
            return Err(AuthError::new(
                StatusCode::FORBIDDEN,
                "L'adresse e-mail du compte Google doit etre verifiee",
            ));
        }
        let email = normalize_email(&google_user.email)?;
        let now = now_ts();
        let mut state = self.lock()?;

        let user_index = if let Some(index) = state
            .store
            .users
            .iter()
            .position(|user| user.google_sub.as_deref() == Some(google_user.sub.as_str()))
        {
            index
        } else if let Some(index) = state
            .store
            .users
            .iter()
            .position(|user| user.email.eq_ignore_ascii_case(&email))
        {
            if state.store.users[index]
                .google_sub
                .as_deref()
                .is_some_and(|sub| sub != google_user.sub)
            {
                return Err(AuthError::conflict(
                    "Cette adresse e-mail est deja liee a un autre compte Google",
                ));
            }
            state.store.users[index].google_sub = Some(google_user.sub.clone());
            index
        } else {
            if !self.config.registration_enabled && !state.store.users.is_empty() {
                return Err(AuthError::new(
                    StatusCode::FORBIDDEN,
                    "La creation de nouveaux comptes est desactivee",
                ));
            }
            let username = google_username(&email, &state.store.users);
            state.store.users.push(StoredUser {
                id: Uuid::new_v4().to_string(),
                username,
                email: email.clone(),
                password_hash: None,
                google_sub: Some(google_user.sub.clone()),
                avatar_url: google_user.picture.clone(),
                created_at: now,
                updated_at: now,
            });
            state.store.users.len() - 1
        };

        state.store.users[user_index].avatar_url = google_user.picture;
        state.store.users[user_index].updated_at = now;
        let stored = state.store.users[user_index].clone();
        let token = issue_session(&mut state.store.sessions, &stored.id, now);
        self.persist_locked(&state).map_err(AuthError::internal)?;
        drop(state);
        self.notify_public_identities_changed();
        Ok((AuthUser::from(&stored), token))
    }

    fn notify_public_identities_changed(&self) {
        if let Some(runtime_sync) = &self.runtime_sync {
            runtime_sync.notify(RuntimeSyncTopic::PrivateMessages);
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, AuthState>, AuthError> {
        self.inner
            .lock()
            .map_err(|_| AuthError::internal("Verrou d'authentification indisponible"))
    }

    fn persist_locked(&self, state: &AuthState) -> Result<(), String> {
        let content = serde_json::to_vec_pretty(&state.store).map_err(|error| error.to_string())?;
        fs_util::atomic_write(&self.config.store_path, content).map_err(|error| {
            format!(
                "ecriture de {} impossible: {error}",
                self.config.store_path.display()
            )
        })?;
        restrict_auth_file_permissions(&self.config.store_path)?;
        Ok(())
    }

    fn session_cookie(&self, token: &str) -> String {
        cookie(
            SESSION_COOKIE,
            token,
            SESSION_DURATION_SECS,
            self.config.secure_cookie,
        )
    }

    fn oauth_state_cookie(&self, state: &str) -> String {
        cookie(
            OAUTH_STATE_COOKIE,
            state,
            OAUTH_STATE_DURATION_SECS,
            self.config.secure_cookie,
        )
    }

    fn expired_cookie(&self, name: &str) -> String {
        cookie(name, "", 0, self.config.secure_cookie)
    }
}

async fn api_config(State(manager): State<AuthManager>) -> Result<Response, AuthError> {
    Ok(no_store(Json(manager.public_config()?).into_response()))
}

async fn api_register(
    State(manager): State<AuthManager>,
    Json(request): Json<RegisterRequest>,
) -> Result<Response, AuthError> {
    let worker = manager.clone();
    let (user, token) = tokio::task::spawn_blocking(move || worker.register(request))
        .await
        .map_err(|_| AuthError::internal("Creation du compte interrompue"))??;
    Ok(json_with_cookie(
        StatusCode::CREATED,
        &SessionResponse { user },
        &manager.session_cookie(&token),
    ))
}

async fn api_login(
    State(manager): State<AuthManager>,
    Json(request): Json<LoginRequest>,
) -> Result<Response, AuthError> {
    let worker = manager.clone();
    let (user, token) = tokio::task::spawn_blocking(move || worker.login(request))
        .await
        .map_err(|_| AuthError::internal("Connexion interrompue"))??;
    Ok(json_with_cookie(
        StatusCode::OK,
        &SessionResponse { user },
        &manager.session_cookie(&token),
    ))
}

async fn api_me(
    State(manager): State<AuthManager>,
    headers: HeaderMap,
) -> Result<Response, AuthError> {
    let user = manager
        .user_from_headers(&headers)?
        .ok_or_else(|| AuthError::unauthorized("Session absente ou expiree"))?;
    Ok(no_store(Json(SessionResponse { user }).into_response()))
}

async fn api_update_profile(
    State(manager): State<AuthManager>,
    headers: HeaderMap,
    Json(request): Json<UpdateProfileRequest>,
) -> Result<Response, AuthError> {
    let worker = manager.clone();
    let user = tokio::task::spawn_blocking(move || worker.update_profile(&headers, request))
        .await
        .map_err(|_| AuthError::internal("Mise a jour du profil interrompue"))??;
    Ok(no_store(Json(SessionResponse { user }).into_response()))
}

async fn api_logout(
    State(manager): State<AuthManager>,
    headers: HeaderMap,
) -> Result<Response, AuthError> {
    manager.logout(&headers)?;
    let mut response = StatusCode::NO_CONTENT.into_response();
    set_cookie(&mut response, &manager.expired_cookie(SESSION_COOKIE));
    Ok(no_store(response))
}

async fn api_google_start(State(manager): State<AuthManager>) -> Result<Response, AuthError> {
    let (location, state) = manager.begin_google()?;
    let mut response = StatusCode::SEE_OTHER.into_response();
    response.headers_mut().insert(
        LOCATION,
        HeaderValue::from_str(&location).map_err(|_| AuthError::internal("URL Google invalide"))?,
    );
    set_cookie(&mut response, &manager.oauth_state_cookie(&state));
    Ok(no_store(response))
}

async fn api_google_callback(
    State(manager): State<AuthManager>,
    headers: HeaderMap,
    Query(query): Query<GoogleCallbackQuery>,
) -> Response {
    if query.error.is_some() {
        return google_redirect(&manager, Err("Connexion Google annulee".to_string()), None);
    }
    let Some(code) = query.code.as_deref() else {
        return google_redirect(&manager, Err("Code Google absent".to_string()), None);
    };
    let Some(state) = query.state.as_deref() else {
        return google_redirect(&manager, Err("Etat Google absent".to_string()), None);
    };

    match manager.finish_google(&headers, code, state).await {
        Ok((_user, token)) => google_redirect(&manager, Ok(()), Some(&token)),
        Err(error) => google_redirect(&manager, Err(error.message), None),
    }
}

fn google_redirect(
    manager: &AuthManager,
    result: Result<(), String>,
    token: Option<&str>,
) -> Response {
    let location = match result {
        Ok(()) => "/?auth=google".to_string(),
        Err(message) => {
            let encoded =
                url::form_urlencoded::byte_serialize(message.as_bytes()).collect::<String>();
            format!("/?auth_error={encoded}")
        }
    };
    let mut response = StatusCode::SEE_OTHER.into_response();
    response.headers_mut().insert(
        LOCATION,
        HeaderValue::from_str(&location).unwrap_or_else(|_| HeaderValue::from_static("/")),
    );
    set_cookie(&mut response, &manager.expired_cookie(OAUTH_STATE_COOKIE));
    if let Some(token) = token {
        set_cookie(&mut response, &manager.session_cookie(token));
    }
    no_store(response)
}

fn json_with_cookie<T: Serialize>(status: StatusCode, value: &T, cookie_value: &str) -> Response {
    let mut response = (status, Json(value)).into_response();
    set_cookie(&mut response, cookie_value);
    no_store(response)
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn set_cookie(response: &mut Response, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        response.headers_mut().append(SET_COOKIE, value);
    }
}

fn cookie(name: &str, value: &str, max_age: i64, secure: bool) -> String {
    format!(
        "{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{}",
        if secure { "; Secure" } else { "" }
    )
}

fn session_tokens(headers: &HeaderMap) -> Vec<String> {
    let mut tokens = Vec::with_capacity(2);
    if let Some(cookie) = cookie_value(headers, SESSION_COOKIE) {
        tokens.push(cookie);
    }
    let bearer = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .strip_prefix("Bearer ")
        .unwrap_or("")
        .trim();
    if !bearer.is_empty() && !tokens.iter().any(|token| token == bearer) {
        tokens.push(bearer.to_string());
    }
    tokens
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all("cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|part| {
            let (key, value) = part.trim().split_once('=')?;
            (key == name && !value.is_empty()).then(|| value.to_string())
        })
}

fn issue_session(sessions: &mut Vec<StoredSession>, user_id: &str, now: i64) -> String {
    prune_expired_sessions(sessions, now);
    let mut user_sessions = sessions
        .iter()
        .enumerate()
        .filter(|(_, session)| session.user_id == user_id)
        .map(|(index, session)| (index, session.created_at))
        .collect::<Vec<_>>();
    if user_sessions.len() >= MAX_SESSIONS_PER_USER {
        user_sessions.sort_by_key(|(_, created_at)| *created_at);
        let remove_count = user_sessions.len() + 1 - MAX_SESSIONS_PER_USER;
        let mut indexes = user_sessions
            .into_iter()
            .take(remove_count)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        indexes.sort_unstable_by(|left, right| right.cmp(left));
        for index in indexes {
            sessions.remove(index);
        }
    }

    let token = format!("{}.{}", random_secret(), random_secret());
    sessions.push(StoredSession {
        token_hash: hash_token(&token),
        user_id: user_id.to_string(),
        created_at: now,
        expires_at: now + SESSION_DURATION_SECS,
    });
    token
}

fn prune_expired_sessions(sessions: &mut Vec<StoredSession>, now: i64) -> bool {
    let previous_len = sessions.len();
    sessions.retain(|session| session.expires_at > now);
    sessions.len() != previous_len
}

fn ensure_identity_available(
    users: &[StoredUser],
    username: &str,
    email: &str,
    except_user_id: Option<&str>,
) -> Result<(), AuthError> {
    if users.iter().any(|user| {
        Some(user.id.as_str()) != except_user_id && user.username.eq_ignore_ascii_case(username)
    }) {
        return Err(AuthError::conflict("Ce nom d'utilisateur est deja utilise"));
    }
    if users.iter().any(|user| {
        Some(user.id.as_str()) != except_user_id && user.email.eq_ignore_ascii_case(email)
    }) {
        return Err(AuthError::conflict(
            "Cette adresse e-mail est deja utilisee",
        ));
    }
    Ok(())
}

fn normalize_username(value: &str) -> Result<String, AuthError> {
    let value = value.trim();
    let length = value.chars().count();
    if !(3..=32).contains(&length) {
        return Err(AuthError::bad_request(
            "Le nom d'utilisateur doit contenir entre 3 et 32 caracteres",
        ));
    }
    if !value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.'))
    {
        return Err(AuthError::bad_request(
            "Le nom d'utilisateur accepte uniquement lettres, chiffres, point, tiret et underscore",
        ));
    }
    Ok(value.to_string())
}

fn normalize_email(value: &str) -> Result<String, AuthError> {
    let value = value.trim().to_lowercase();
    if value.is_empty()
        || value.len() > 254
        || value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(AuthError::bad_request("Adresse e-mail invalide"));
    }
    let Some((local, domain)) = value.rsplit_once('@') else {
        return Err(AuthError::bad_request("Adresse e-mail invalide"));
    };
    if local.is_empty()
        || local.len() > 64
        || domain.len() < 3
        || !domain.contains('.')
        || domain.split('.').any(|label| {
            label.is_empty()
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        })
    {
        return Err(AuthError::bad_request("Adresse e-mail invalide"));
    }
    Ok(value)
}

fn validate_password(value: &str) -> Result<(), AuthError> {
    let length = value.chars().count();
    if !(10..=128).contains(&length) {
        return Err(AuthError::bad_request(
            "Le mot de passe doit contenir entre 10 et 128 caracteres",
        ));
    }
    Ok(())
}

fn hash_password(value: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(value.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| format!("hachage du mot de passe impossible: {error}"))
}

fn verify_password(encoded: &str, value: &str) -> bool {
    PasswordHash::new(encoded).ok().is_some_and(|hash| {
        Argon2::default()
            .verify_password(value.as_bytes(), &hash)
            .is_ok()
    })
}

fn google_username(email: &str, users: &[StoredUser]) -> String {
    let mut base = email
        .split('@')
        .next()
        .unwrap_or("user")
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
        .take(24)
        .collect::<String>()
        .trim_matches(['.', '-', '_'])
        .to_string();
    if base.len() < 3 {
        base = "user".to_string();
    }
    if !users
        .iter()
        .any(|user| user.username.eq_ignore_ascii_case(&base))
    {
        return base;
    }
    loop {
        let suffix = Uuid::new_v4().simple().to_string();
        let candidate = format!("{}-{}", base, &suffix[..6]);
        if !users
            .iter()
            .any(|user| user.username.eq_ignore_ascii_case(&candidate))
        {
            return candidate;
        }
    }
}

fn hash_token(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn random_secret() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn google_login_url(redirect_uri: &str) -> Result<String, String> {
    let mut url = Url::parse(redirect_uri)
        .map_err(|error| format!("CST_GOOGLE_REDIRECT_URI invalide: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("CST_GOOGLE_REDIRECT_URI doit etre une URL HTTP(S) absolue".to_string());
    }
    url.set_path("/api/auth/google/start");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.into())
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

fn env_trimmed(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_bool(name: &str) -> Option<bool> {
    env_trimmed(name).and_then(|value| match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    })
}

#[cfg(unix)]
fn restrict_auth_file_permissions(path: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn restrict_auth_file_permissions(_path: &std::path::Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_and_normalizes_account_fields() {
        assert_eq!(
            normalize_username("  jean-pierre  ").unwrap(),
            "jean-pierre"
        );
        assert!(normalize_username("ab").is_err());
        assert!(normalize_username("jean pierre").is_err());
        assert_eq!(
            normalize_email(" Jean@Example.COM ").unwrap(),
            "jean@example.com"
        );
        assert!(normalize_email("jean@example").is_err());
        assert!(validate_password("mot-de-passe-solide").is_ok());
        assert!(validate_password("court").is_err());
    }

    #[test]
    fn session_tokens_are_hashed_and_cookie_is_http_only() {
        assert_ne!(hash_token("secret"), "secret");
        let cookie = cookie(SESSION_COOKIE, "secret", 60, true);
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Lax"));
        assert!(cookie.contains("Secure"));
    }

    #[test]
    fn google_username_is_unique() {
        let existing = StoredUser {
            id: "one".into(),
            username: "jean".into(),
            email: "jean@example.com".into(),
            password_hash: None,
            google_sub: None,
            avatar_url: None,
            created_at: 0,
            updated_at: 0,
        };
        let generated = google_username("jean@example.org", &[existing]);
        assert!(generated.starts_with("jean-"));
        assert_ne!(generated, "jean");
    }

    #[test]
    fn google_login_starts_on_the_callback_origin() {
        assert_eq!(
            google_login_url(
                "https://pc-fixe-cst.example.test/api/auth/google/callback?ignored=true"
            )
            .unwrap(),
            "https://pc-fixe-cst.example.test/api/auth/google/start"
        );
        assert!(google_login_url("javascript:alert(1)").is_err());
    }
}
