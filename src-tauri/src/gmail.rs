//! Liaison d'une boite Gmail au compte utilisateur de l'application via OAuth
//! Google, puis lecture des messages via l'API Gmail (onglet Mail).
//!
//! Invariants de securite, miroirs de `microsoft.rs` :
//!
//! 1. la liaison exige une session utilisateur nominative au demarrage ET au
//!    retour du fournisseur. Elle ne cree jamais de compte et ne fusionne
//!    jamais deux comptes par adresse ;
//! 2. les jetons ne quittent jamais le serveur. Aucune vue serialisee sous
//!    `/api/gmail` ne contient `accessToken` ni `refreshToken` ;
//! 3. l'acces est en lecture seule (scope `gmail.readonly`) : l'onglet Mail
//!    affiche et actualise les messages, il ne peut ni envoyer ni supprimer.
//!
//! La configuration OAuth reutilise le client Google de l'application
//! (`CST_GOOGLE_CLIENT_ID` / `CST_GOOGLE_CLIENT_SECRET`), mais avec sa propre
//! URI de redirection (`/api/gmail/callback`) et ses propres scopes : le flux
//! de connexion de `auth.rs` reste limite a `openid email profile`.

use crate::{
    auth::{AuthIdentity, AuthManager},
    fs_util,
    security::constant_time_eq,
};
use axum::{
    extract::{Query, State},
    http::{
        header::{CACHE_CONTROL, LOCATION, SET_COOKIE},
        HeaderMap, HeaderValue, StatusCode,
    },
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use url::Url;
use uuid::Uuid;

const STORE_VERSION: u32 = 2;
const STORE_FILE: &str = "gmail-links.json";
/// Cookie distinct de `cst_oauth_state` (connexion) et de `cst_ms_oauth_state`
/// (Microsoft) : les flux ne s'ecrasent pas mutuellement.
const OAUTH_STATE_COOKIE: &str = "cst_gmail_oauth_state";
const OAUTH_STATE_DURATION_SECS: i64 = 10 * 60;
/// Marge de renouvellement : un jeton qui expire dans moins de deux minutes est
/// rafraichi avant l'appel plutot que de faire echouer l'onglet sur un 401.
const REFRESH_MARGIN_SECS: i64 = 120;
/// Duree de vie minimale retenue quand Google n'annonce pas `expires_in`.
const MIN_ACCESS_TOKEN_LIFETIME_SECS: i64 = 300;
const DEFAULT_SCOPES: &str = "https://www.googleapis.com/auth/gmail.readonly";
const GMAIL_API: &str = "https://gmail.googleapis.com/gmail/v1";
const OAUTH_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const OAUTH_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const DEFAULT_MAX_MESSAGES: usize = 30;
const MAX_MESSAGES: usize = 100;
const MAX_QUERY_CHARS: usize = 200;
/// Bornes du rafraichissement automatique : jamais plus d'un echange en vol par
/// proprietaire, pour ne pas faire tourner le refresh token a chaque onglet.
const REFRESH_LOCK_TTL_SECS: i64 = 60;

// ---------------------------------------------------------------------------
// Etat persistant
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GmailStore {
    version: u32,
    #[serde(default)]
    links: Vec<StoredLink>,
}

impl Default for GmailStore {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            links: Vec::new(),
        }
    }
}

/// Une boite Gmail liee a un compte de l'application (`owner_id`). Un
/// proprietaire peut lier autant de boites que souhaite : chaque liaison porte
/// son propre `link_id` (immuable, genere a la liaison), c'est lui qui
/// identifie la boite partout (messages, deconnexion), jamais l'adresse
/// e-mail qui peut etre reattribuee.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredLink {
    owner_id: String,
    /// Identifiant stable de CETTE liaison. Vide dans les anciens fichiers :
    /// la normalisation au chargement en genere un.
    #[serde(default)]
    link_id: String,
    email: String,
    access_token: String,
    refresh_token: String,
    expires_at: i64,
    #[serde(default)]
    scopes: Vec<String>,
    linked_at: i64,
    /// Date du dernier echange reussi avec Google.
    updated_at: i64,
    /// Autorisation morte cote Google (consentement revoque, refresh token
    /// rejete). On garde l'identite pour proposer de relier CE compte.
    #[serde(default)]
    needs_relink: bool,
}

/// Une boite liee, telle que vue par l'interface. Jamais de jeton ici.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GmailAccountView {
    /// Identifiant de la liaison, a repasser en parametre `link` des routes
    /// `messages` et `connection` (suppression).
    link_id: String,
    email: String,
    needs_relink: bool,
    scopes: Vec<String>,
    linked_at: i64,
    /// Premiere boite utilisable du proprietaire : cible des appels sans `link`.
    is_default: bool,
}

/// Liaison telle que vue par l'interface. Jamais de jeton ici.
///
/// Les champs de tete (`email`, `linked_at`, `scopes`) refletent la boite par
/// defaut : ils gardent l'interface d'avant le multi-comptes fonctionnelle.
/// `accounts` porte la liste complete des boites liees.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GmailConnectionView {
    configured: bool,
    connected: bool,
    email: Option<String>,
    needs_relink: bool,
    scopes: Vec<String>,
    linked_at: Option<i64>,
    accounts: Vec<GmailAccountView>,
    /// URI de redirection a declarer dans Google Cloud Console, connue meme
    /// avant configuration : l'utilisateur en a besoin pour autoriser le flux.
    redirect_uri: String,
    client_id: Option<String>,
    login_url: Option<String>,
}

/// Un message de la boite, tel que renvoye a l'onglet. Aucun jeton.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GmailMessage {
    id: String,
    thread_id: String,
    from: Option<String>,
    to: Vec<String>,
    subject: String,
    snippet: String,
    /// Horodatage RFC 3339 du message (date d'envoi).
    date: Option<String>,
    unread: bool,
    labels: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GmailMessagesView {
    email: String,
    fetched_at: i64,
    messages: Vec<GmailMessage>,
}

struct PendingLink {
    code_verifier: String,
    expires_at: i64,
}

#[derive(Debug, Clone)]
struct ProviderConfig {
    client_id: String,
    client_secret: String,
    redirect_uri: String,
    scopes: String,
    login_url: String,
}

struct GmailState {
    store: GmailStore,
    pending: HashMap<String, PendingLink>,
    /// Clef : owner_id. Empeche deux rafraichissements concurrents du meme lien.
    refresh_locks: HashMap<String, i64>,
}

struct RuntimeConfig {
    store_path: PathBuf,
    secure_cookie: bool,
}

#[derive(Clone)]
pub(crate) struct GmailManager {
    inner: Arc<Mutex<GmailState>>,
    config: Arc<RuntimeConfig>,
    provider: Arc<Option<ProviderConfig>>,
    http: reqwest::Client,
    auth: AuthManager,
}

struct GmailError {
    status: StatusCode,
    message: String,
}

impl GmailError {
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

    fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }
}

impl IntoResponse for GmailError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.message });
        (self.status, Json(body)).into_response()
    }
}

impl GmailManager {
    pub(crate) fn load(
        data_dir: PathBuf,
        public_base_url: &str,
        auth: AuthManager,
    ) -> Result<Self, String> {
        let store_path = data_dir.join(STORE_FILE);
        let store = if store_path.exists() {
            let content = fs::read_to_string(&store_path).map_err(|error| {
                format!("lecture de {} impossible: {error}", store_path.display())
            })?;
            let parsed: GmailStore = serde_json::from_str(&content)
                .map_err(|error| format!("{} invalide: {error}", store_path.display()))?;
            if parsed.version > STORE_VERSION {
                return Err(format!(
                    "{} utilise une version de liaisons Gmail plus recente ({})",
                    store_path.display(),
                    parsed.version
                ));
            }
            parsed
        } else {
            GmailStore::default()
        };
        // Multi-comptes : on conserve toutes les boites, en dedoublonnant par
        // `link_id` (les anciens fichiers sans identifiant en recoivent un).
        let mut store = store;
        let mut changed = false;
        for link in store.links.iter_mut() {
            if link.link_id.trim().is_empty() {
                link.link_id = random_secret();
                changed = true;
            }
        }
        store.links.sort_by_key(|link| std::cmp::Reverse(link.updated_at));
        let mut seen_links = std::collections::HashSet::new();
        let previous_len = store.links.len();
        store
            .links
            .retain(|link| seen_links.insert(link.link_id.clone()));
        if store.links.len() != previous_len {
            changed = true;
        }
        if changed {
            if let Err(error) = fs_util::atomic_write(&store_path, serde_json::to_vec_pretty(&store).map_err(|e| e.to_string())?) {
                return Err(format!(
                    "ecriture de {} impossible: {error}",
                    store_path.display()
                ));
            }
            let _ = fs_util::restrict_private_file(&store_path);
        }

        let provider = build_provider_config(public_base_url).map_err(|error| {
            // Une faute de frappe ne doit jamais empecher le noeud de demarrer :
            // divergence assumee avec le flux Google d'auth.rs.
            eprintln!("Onglet Gmail desactive : {error}");
            error
        })?;
        let secure_cookie = public_base_url.trim().starts_with("https://");

        Ok(Self {
            inner: Arc::new(Mutex::new(GmailState {
                store,
                pending: HashMap::new(),
                refresh_locks: HashMap::new(),
            })),
            config: Arc::new(RuntimeConfig {
                store_path,
                secure_cookie,
            }),
            provider: Arc::new(provider),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .map_err(|error| format!("client Gmail impossible: {error}"))?,
            auth,
        })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, GmailState>, GmailError> {
        self.inner
            .lock()
            .map_err(|_| GmailError::internal("Verrou Gmail indisponible"))
    }

    fn identity(&self, headers: &HeaderMap) -> Result<AuthIdentity, GmailError> {
        self.auth
            .identity_from_headers(headers)
            .map_err(GmailError::internal)?
            .ok_or_else(|| GmailError::unauthorized("Session utilisateur requise"))
    }

    fn provider(&self) -> Result<ProviderConfig, GmailError> {
        self.provider.as_ref().clone().ok_or_else(|| {
            GmailError::new(
                StatusCode::NOT_FOUND,
                "Integration Gmail non configuree sur ce serveur",
            )
        })
    }

    fn persist_locked(&self, state: &GmailState) -> Result<(), String> {
        let content = serde_json::to_vec_pretty(&state.store).map_err(|error| error.to_string())?;
        fs_util::atomic_write(&self.config.store_path, content).map_err(|error| {
            format!(
                "ecriture de {} impossible: {error}",
                self.config.store_path.display()
            )
        })?;
        fs_util::restrict_private_file(&self.config.store_path)
            .map_err(|error| format!("restriction de {} impossible: {error}", self.config.store_path.display()))
    }

    // -----------------------------------------------------------------------
    // Vue publique
    // -----------------------------------------------------------------------

    fn connection_view(&self, owner_id: &str) -> Result<GmailConnectionView, GmailError> {
        let state = self.lock()?;
        let provider: Option<&ProviderConfig> = self.provider.as_ref().as_ref();
        let links = state
            .store
            .links
            .iter()
            .filter(|link| link.owner_id == owner_id)
            .collect::<Vec<_>>();
        // Boite par defaut : la premiere utilisable, sinon la premiere liee.
        let default = links
            .iter()
            .find(|link| !link.needs_relink)
            .or_else(|| links.first())
            .copied();
        let default_link_id = default.map(|link| link.link_id.clone()).unwrap_or_default();
        let accounts = links
            .iter()
            .map(|link| GmailAccountView {
                link_id: link.link_id.clone(),
                email: link.email.clone(),
                needs_relink: link.needs_relink,
                scopes: link.scopes.clone(),
                linked_at: link.linked_at,
                is_default: link.link_id == default_link_id,
            })
            .collect::<Vec<_>>();
        Ok(GmailConnectionView {
            configured: provider.is_some(),
            connected: links.iter().any(|link| !link.needs_relink),
            email: default.map(|link| link.email.clone()),
            needs_relink: provider.is_some() && !links.iter().any(|link| !link.needs_relink) && !links.is_empty(),
            scopes: default
                .map(|link| link.scopes.clone())
                .unwrap_or_else(|| {
                    provider
                        .map(|p| split_scopes(&p.scopes))
                        .unwrap_or_default()
                }),
            linked_at: default.map(|link| link.linked_at),
            accounts,
            redirect_uri: provider
                .map(|p| p.redirect_uri.clone())
                .unwrap_or_else(|| "(non configure)".to_string()),
            client_id: provider.map(|p| p.client_id.clone()),
            login_url: provider.map(|p| p.login_url.clone()),
        })
    }

    /// Identifiant de la boite par defaut du proprietaire : premiere utilisable,
    /// sinon premiere liee. Cible des appels qui ne precisent pas `link`.
    fn default_link_id(&self, owner_id: &str) -> Result<String, GmailError> {
        let state = self.lock()?;
        state
            .store
            .links
            .iter()
            .filter(|link| link.owner_id == owner_id)
            .find(|link| !link.needs_relink)
            .or_else(|| {
                state
                    .store
                    .links
                    .iter()
                    .find(|link| link.owner_id == owner_id)
            })
            .map(|link| link.link_id.clone())
            .ok_or_else(|| GmailError::unauthorized("Aucune boite Gmail liee"))
    }

    fn find_link(&self, owner_id: &str, link_id: &str) -> Result<StoredLink, GmailError> {
        let state = self.lock()?;
        state
            .store
            .links
            .iter()
            .find(|link| link.owner_id == owner_id && link.link_id == link_id)
            .cloned()
            .ok_or_else(|| GmailError::not_found("Boite Gmail inconnue"))
    }

    // -----------------------------------------------------------------------
    // Liaison OAuth
    // -----------------------------------------------------------------------

    fn begin_link(&self, headers: &HeaderMap) -> Result<(String, String), GmailError> {
        let provider = self.provider()?;
        // La liaison exige une session nominative au depart, comme au retour :
        // on sait ainsi a QUI attacher la boite avant d'ouvrir le navigateur.
        let identity = self.identity(headers)?;
        let state_token = random_secret();
        let code_verifier = format!("{}{}", random_secret(), random_secret()).replace('-', "");
        let code_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()));
        let now = now_ts();

        let mut state = self.lock()?;
        state
            .pending
            .retain(|_, pending| pending.expires_at > now);
        state.pending.insert(
            hash_token(&state_token),
            PendingLink {
                code_verifier,
                expires_at: now + OAUTH_STATE_DURATION_SECS,
            },
        );
        let _ = identity;

        let mut url = Url::parse(OAUTH_AUTH_URL)
            .map_err(|error| GmailError::internal(error.to_string()))?;
        url.query_pairs_mut()
            .append_pair("client_id", &provider.client_id)
            .append_pair("redirect_uri", &provider.redirect_uri)
            .append_pair("response_type", "code")
            .append_pair("scope", &provider.scopes)
            .append_pair("state", &state_token)
            .append_pair("code_challenge", &code_challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("prompt", "select_account")
            // `access_type=offline` suffit pour obtenir un refresh token a la
            // premiere autorisation de chaque compte. Ne PAS ajouter l'ancien
            // parametre `approval_prompt` : combine a `prompt` il fait echouer
            // Google avec 400 invalid_request "Conflict params: approval_prompt
            // and prompt". Pour un nouveau compte, `prompt=select_account`
            // permet de choisir la boite a lier.
            .append_pair("access_type", "offline");
        Ok((url.into(), state_token))
    }

    async fn finish_link(
        &self,
        headers: &HeaderMap,
        code: &str,
        state_token: &str,
    ) -> Result<(), GmailError> {
        // Session nominative exigee au retour : la boite est liee a la personne
        // qui a DEMARRE le flux, jamais a celle du navigateur seul.
        let identity = self.identity(headers)?;
        let provider = self.provider()?;
        let cookie_state = cookie_value(headers, OAUTH_STATE_COOKIE)
            .ok_or_else(|| GmailError::bad_request("Session OAuth Gmail absente"))?;
        if !constant_time_eq(cookie_state.as_bytes(), state_token.as_bytes()) {
            return Err(GmailError::bad_request("Etat OAuth Gmail invalide"));
        }
        let pending = {
            let mut state = self.lock()?;
            state
                .pending
                .remove(&hash_token(state_token))
                .filter(|pending| pending.expires_at > now_ts())
                .ok_or_else(|| GmailError::bad_request("Session OAuth Gmail expiree"))?
        };

        let token_response = self
            .http
            .post(OAUTH_TOKEN_URL)
            .form(&[
                ("client_id", provider.client_id.as_str()),
                ("client_secret", provider.client_secret.as_str()),
                ("code", code),
                ("code_verifier", pending.code_verifier.as_str()),
                ("grant_type", "authorization_code"),
                ("redirect_uri", provider.redirect_uri.as_str()),
            ])
            .send()
            .await
            .map_err(|_| GmailError::new(StatusCode::BAD_GATEWAY, "Google ne repond pas"))?;
        if !token_response.status().is_success() {
            return Err(GmailError::new(
                StatusCode::BAD_GATEWAY,
                "Google a refuse le code d'autorisation (URI de redirection non declaree ?)",
            ));
        }
        let token: TokenResponse = token_response
            .json()
            .await
            .map_err(|_| GmailError::new(StatusCode::BAD_GATEWAY, "Reponse Google invalide"))?;
        let Some(refresh_token) = token.refresh_token.as_deref() else {
            return Err(GmailError::new(
                StatusCode::BAD_GATEWAY,
                "Google n'a pas retourne de jeton de renouvellement",
            ));
        };

        let profile = self
            .http
            .get(format!("{GMAIL_API}/users/me/profile"))
            .bearer_auth(&token.access_token)
            .send()
            .await
            .map_err(|_| GmailError::new(StatusCode::BAD_GATEWAY, "Profil Gmail inaccessible"))?;
        if !profile.status().is_success() {
            return Err(GmailError::new(
                StatusCode::BAD_GATEWAY,
                "Google n'a pas retourne le profil demande",
            ));
        }
        let profile: Value = profile
            .json()
            .await
            .map_err(|_| GmailError::new(StatusCode::BAD_GATEWAY, "Profil Gmail invalide"))?;
        let email = profile
            .get("emailAddress")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| GmailError::new(StatusCode::BAD_GATEWAY, "Adresse Gmail absente"))?;

        let now = now_ts();
        // Multi-comptes : on AJOUTE la nouvelle boite, on ne remplace pas une
        // existante. L'utilisateur peut lier autant de boites que souhaite.
        let mut state = self.lock()?;
        state.store.links.push(StoredLink {
            owner_id: identity.id.clone(),
            link_id: random_secret(),
            email,
            access_token: token.access_token,
            refresh_token: refresh_token.to_string(),
            expires_at: now + lifetime(token.expires_in),
            scopes: split_scopes(token.scope.as_deref().unwrap_or(&provider.scopes)),
            linked_at: now,
            updated_at: now,
            needs_relink: false,
        });
        self.persist_locked(&state)
            .map_err(GmailError::internal)?;
        Ok(())
    }

    /// Retire la boite designee par `link_id` (et seulement celle-la).
    fn disconnect(&self, owner_id: &str, link_id: &str) -> Result<(), GmailError> {
        let mut state = self.lock()?;
        let previous_len = state.store.links.len();
        state
            .store
            .links
            .retain(|link| !(link.owner_id == owner_id && link.link_id == link_id));
        if state.store.links.len() != previous_len {
            self.persist_locked(&state).map_err(GmailError::internal)?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Lecture de la boite
    // -----------------------------------------------------------------------

    /// Jeton d'acces valide pour la boite designee par `link_id`, en
    /// rafraichissant si besoin. Renvoie (access_token, email) sans jamais
    /// exposer le refresh token.
    async fn valid_tokens(&self, owner_id: &str, link_id: &str) -> Result<(String, String), GmailError> {
        let provider = self.provider()?;
        let snapshot = {
            let state = self.lock()?;
            let link = state
                .store
                .links
                .iter()
                .find(|link| link.owner_id == owner_id && link.link_id == link_id)
                .cloned()
                .ok_or_else(|| GmailError::unauthorized("Aucune boite Gmail liee"))?;
            if link.needs_relink {
                return Err(GmailError::unauthorized(
                    "L'autorisation Gmail a expire. Relie le compte depuis l'onglet Mail.",
                ));
            }
            link
        };
        if snapshot.expires_at - REFRESH_MARGIN_SECS > now_ts() {
            return Ok((snapshot.access_token, snapshot.email));
        }
        if !self.try_acquire_refresh_lock(link_id)? {
            // Un autre appel est en train de rafraichir : on attend un instant
            // puis on relit l'etat plutot que de doubler l'echange.
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            let state = self.lock()?;
            let link = state
                .store
                .links
                .iter()
                .find(|link| link.owner_id == owner_id && link.link_id == link_id)
                .cloned()
                .ok_or_else(|| GmailError::unauthorized("Aucune boite Gmail liee"))?;
            return Ok((link.access_token, link.email));
        }

        let refreshed = self
            .refresh_access_token(&provider, &snapshot.refresh_token)
            .await;
        self.release_refresh_lock(link_id);
        let (access_token, expires_at) = refreshed?;

        let email = {
            let mut state = self.lock()?;
            let Some(link) = state
                .store
                .links
                .iter_mut()
                .find(|link| link.owner_id == owner_id && link.link_id == link_id)
            else {
                return Err(GmailError::unauthorized("Aucune boite Gmail liee"));
            };
            link.access_token = access_token.clone();
            link.expires_at = expires_at;
            link.updated_at = now_ts();
            let email = link.email.clone();
            self.persist_locked(&state).map_err(GmailError::internal)?;
            email
        };
        Ok((access_token, email))
    }

    fn try_acquire_refresh_lock(&self, link_id: &str) -> Result<bool, GmailError> {
        let mut state = self.lock()?;
        let now = now_ts();
        state.refresh_locks.retain(|_, expires_at| *expires_at > now);
        if state.refresh_locks.contains_key(link_id) {
            return Ok(false);
        }
        state
            .refresh_locks
            .insert(link_id.to_string(), now + REFRESH_LOCK_TTL_SECS);
        Ok(true)
    }

    fn release_refresh_lock(&self, link_id: &str) {
        if let Ok(mut state) = self.lock() {
            state.refresh_locks.remove(link_id);
        }
    }

    async fn refresh_access_token(
        &self,
        provider: &ProviderConfig,
        refresh_token: &str,
    ) -> Result<(String, i64), GmailError> {
        let response = self
            .http
            .post(OAUTH_TOKEN_URL)
            .form(&[
                ("client_id", provider.client_id.as_str()),
                ("client_secret", provider.client_secret.as_str()),
                ("refresh_token", refresh_token),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .map_err(|_| GmailError::new(StatusCode::BAD_GATEWAY, "Google ne repond pas"))?;
        if !response.status().is_success() {
            // Refresh token rejete (consentement revoque) : on marque la liaison
            // a refaire plutot que de boucler sur chaque appel.
            let mut state = self.lock()?;
            if let Some(link) = state
                .store
                .links
                .iter_mut()
                .find(|link| link.refresh_token == refresh_token)
            {
                link.needs_relink = true;
                let _ = self.persist_locked(&state);
            }
            return Err(GmailError::unauthorized(
                "L'autorisation Gmail a expire (refresh token rejete). Relie le compte.",
            ));
        }
        let token: TokenResponse = response
            .json()
            .await
            .map_err(|_| GmailError::new(StatusCode::BAD_GATEWAY, "Reponse Google invalide"))?;
        Ok((token.access_token, now_ts() + lifetime(token.expires_in)))
    }

    async fn list_messages(
        &self,
        owner_id: &str,
        link_id: &str,
        max: usize,
        query: Option<String>,
    ) -> Result<GmailMessagesView, GmailError> {
        let (access_token, email) = self.valid_tokens(owner_id, link_id).await?;
        let max = max.clamp(1, MAX_MESSAGES);
        let query = query
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(|value| {
                value
                    .chars()
                    .take(MAX_QUERY_CHARS)
                    .collect::<String>()
            });

        let mut list_url = Url::parse(&format!("{GMAIL_API}/users/me/messages"))
            .map_err(|error| GmailError::internal(error.to_string()))?;
        list_url
            .query_pairs_mut()
            .append_pair("maxResults", &max.to_string());
        if let Some(query) = query.as_deref() {
            list_url.query_pairs_mut().append_pair("q", query);
        }
        let list_response = self
            .http
            .get(list_url)
            .bearer_auth(&access_token)
            .send()
            .await
            .map_err(|_| GmailError::new(StatusCode::BAD_GATEWAY, "Gmail ne repond pas"))?;
        if !list_response.status().is_success() {
            return Err(gmail_error(list_response.status(), "Gmail a refuse la liste"));
        }
        let list: Value = list_response
            .json()
            .await
            .map_err(|_| GmailError::new(StatusCode::BAD_GATEWAY, "Liste Gmail invalide"))?;
        let ids = list
            .get("messages")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("id").and_then(Value::as_str))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let mut messages = Vec::with_capacity(ids.len());
        for id in ids {
            let mut meta_url = Url::parse(&format!("{GMAIL_API}/users/me/messages/{id}"))
                .map_err(|error| GmailError::internal(error.to_string()))?;
            meta_url
                .query_pairs_mut()
                .append_pair("format", "metadata");
            for header in ["From", "To", "Subject", "Date"] {
                meta_url.query_pairs_mut().append_pair("metadataHeaders", header);
            }
            let response = self
                .http
                .get(meta_url)
                .bearer_auth(&access_token)
                .send()
                .await
                .map_err(|_| GmailError::new(StatusCode::BAD_GATEWAY, "Gmail ne repond pas"))?;
            if !response.status().is_success() {
                continue;
            }
            if let Ok(value) = response.json::<Value>().await {
                if let Some(message) = parse_message(&value) {
                    messages.push(message);
                }
            }
        }

        Ok(GmailMessagesView {
            email,
            fetched_at: now_ts(),
            messages,
        })
    }
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

pub(crate) fn router(manager: GmailManager) -> Router {
    Router::new()
        .route("/connection", get(api_connection).delete(api_disconnect))
        .route("/start", get(api_start))
        .route("/callback", get(api_callback))
        .route("/messages", get(api_messages))
        .with_state(manager)
}

async fn api_connection(
    State(manager): State<GmailManager>,
    headers: HeaderMap,
) -> Result<Response, GmailError> {
    let identity = manager.identity(&headers)?;
    let view = manager.connection_view(&identity.id)?;
    Ok(no_store(Json(view).into_response()))
}

#[derive(Deserialize)]
struct DisconnectQuery {
    /// `link` de la boite a delier ; sans lui, la boite par defaut.
    #[serde(default)]
    link: Option<String>,
}

async fn api_disconnect(
    State(manager): State<GmailManager>,
    headers: HeaderMap,
    Query(query): Query<DisconnectQuery>,
) -> Result<Response, GmailError> {
    let identity = manager.identity(&headers)?;
    require_same_site(&headers)?;
    let link_id = match query.link {
        Some(link) if !link.trim().is_empty() => link,
        _ => manager.default_link_id(&identity.id)?,
    };
    manager.disconnect(&identity.id, &link_id)?;
    let view = manager.connection_view(&identity.id)?;
    Ok(no_store(Json(view).into_response()))
}

async fn api_start(
    State(manager): State<GmailManager>,
    headers: HeaderMap,
) -> Result<Response, GmailError> {
    let (location, state) = manager.begin_link(&headers)?;
    let mut response = StatusCode::SEE_OTHER.into_response();
    response.headers_mut().insert(
        LOCATION,
        HeaderValue::from_str(&location)
            .map_err(|_| GmailError::internal("URL Google invalide"))?,
    );
    set_cookie(
        &mut response,
        &cookie(
            OAUTH_STATE_COOKIE,
            &state,
            OAUTH_STATE_DURATION_SECS,
            manager.config.secure_cookie,
        ),
    );
    Ok(no_store(response))
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

async fn api_callback(
    State(manager): State<GmailManager>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let result = (|| async {
        if query.error.is_some() {
            return Err(GmailError::bad_request("Liaison Gmail annulee"));
        }
        let Some(code) = query.code.as_deref() else {
            return Err(GmailError::bad_request("Code Google absent"));
        };
        let Some(state) = query.state.as_deref() else {
            return Err(GmailError::bad_request("Etat Google absent"));
        };
        manager.finish_link(&headers, code, state).await
    })()
    .await;
    gmail_redirect(result)
}

fn gmail_redirect(result: Result<(), GmailError>) -> Response {
    let location = match result {
        Ok(()) => "/?gmail=linked".to_string(),
        Err(error) => {
            let encoded =
                url::form_urlencoded::byte_serialize(error.message.as_bytes()).collect::<String>();
            format!("/?gmail_error={encoded}")
        }
    };
    let mut response = StatusCode::SEE_OTHER.into_response();
    response.headers_mut().insert(
        LOCATION,
        HeaderValue::from_str(&location).unwrap_or_else(|_| HeaderValue::from_static("/")),
    );
    no_store(response)
}

#[derive(Deserialize)]
struct MessagesQuery {
    #[serde(default)]
    max: Option<usize>,
    #[serde(default)]
    q: Option<String>,
    /// `link` de la boite a lire ; sans lui, la boite par defaut.
    #[serde(default)]
    link: Option<String>,
}

async fn api_messages(
    State(manager): State<GmailManager>,
    headers: HeaderMap,
    Query(query): Query<MessagesQuery>,
) -> Result<Response, GmailError> {
    let identity = manager.identity(&headers)?;
    let link_id = match query.link {
        Some(link) if !link.trim().is_empty() => link,
        _ => manager.default_link_id(&identity.id)?,
    };
    let view = manager
        .list_messages(&identity.id, &link_id, query.max.unwrap_or(DEFAULT_MAX_MESSAGES), query.q)
        .await?;
    Ok(no_store(Json(view).into_response()))
}

// ---------------------------------------------------------------------------
// Aides
// ---------------------------------------------------------------------------

fn build_provider_config(public_base_url: &str) -> Result<Option<ProviderConfig>, String> {
    let client_id = env_trimmed("CST_GOOGLE_CLIENT_ID");
    let client_secret = env_trimmed("CST_GOOGLE_CLIENT_SECRET");
    let (client_id, client_secret) = match (client_id, client_secret) {
        (Some(client_id), Some(client_secret)) => (client_id, client_secret),
        (None, None) => return Ok(None),
        _ => return Err(
            "CST_GOOGLE_CLIENT_ID et CST_GOOGLE_CLIENT_SECRET doivent etre definis ensemble"
                .to_string(),
        ),
    };
    let redirect_uri = env_trimmed("CST_GOOGLE_MAIL_REDIRECT_URI").unwrap_or_else(|| {
        format!(
            "{}/api/gmail/callback",
            public_base_url.trim_end_matches('/')
        )
    });
    let scopes = env_trimmed("CST_GOOGLE_MAIL_SCOPES").unwrap_or_else(|| DEFAULT_SCOPES.to_string());
    let login_url = gmail_login_url(&redirect_uri)?;
    Ok(Some(ProviderConfig {
        client_id,
        client_secret,
        redirect_uri,
        scopes,
        login_url,
    }))
}

/// La page de configuration de l'application pointe vers l'onglet Mail plutot
/// que vers le flux de connexion.
fn gmail_login_url(redirect_uri: &str) -> Result<String, String> {
    let mut url = Url::parse(redirect_uri)
        .map_err(|error| format!("CST_GOOGLE_MAIL_REDIRECT_URI invalide: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("CST_GOOGLE_MAIL_REDIRECT_URI doit etre une URL HTTP(S) absolue".to_string());
    }
    url.set_path("/api/gmail/start");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.into())
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: i64,
    #[serde(default)]
    scope: Option<String>,
}

/// `expires_in` absent ou nul => plancher de cinq minutes (voir constantes).
fn lifetime(expires_in: i64) -> i64 {
    expires_in.max(MIN_ACCESS_TOKEN_LIFETIME_SECS)
}

fn split_scopes(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .map(str::to_string)
        .filter(|scope| !scope.is_empty())
        .collect()
}

/// Construit la vue d'un message depuis la reponse `format=metadata` de Gmail.
fn parse_message(value: &Value) -> Option<GmailMessage> {
    let id = value.get("id")?.as_str()?.to_string();
    let thread_id = value
        .get("threadId")
        .and_then(Value::as_str)
        .unwrap_or(&id)
        .to_string();
    let headers = value
        .get("payload")
        .and_then(|payload| payload.get("headers"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let header = |name: &str| -> Option<String> {
        headers.iter().find_map(|entry| {
            let header_name = entry.get("name").and_then(Value::as_str)?;
            if header_name.eq_ignore_ascii_case(name) {
                entry
                    .get("value")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            } else {
                None
            }
        })
    };
    let labels = value
        .get("labelIds")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let unread = labels.iter().any(|label| label == "UNREAD");
    let to = header("To")
        .map(|value| {
            value
                .split(',')
                .map(|part| part.trim().to_string())
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some(GmailMessage {
        id,
        thread_id,
        from: header("From"),
        to,
        subject: header("Subject").unwrap_or_else(|| "(sans objet)".to_string()),
        snippet: value
            .get("snippet")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        date: header("Date"),
        unread,
        labels,
    })
}

fn gmail_error(status: StatusCode, fallback: &str) -> GmailError {
    let message = match status {
        StatusCode::UNAUTHORIZED => {
            "L'autorisation Gmail a expire. Relie le compte depuis l'onglet Mail.".to_string()
        }
        StatusCode::FORBIDDEN => {
            "Google a refuse l'acces demande. Verifiez les permissions accordees.".to_string()
        }
        StatusCode::TOO_MANY_REQUESTS => {
            "Gmail limite temporairement les appels. Reessayez dans un instant.".to_string()
        }
        _ => format!("{fallback} ({status})"),
    };
    GmailError::new(StatusCode::BAD_GATEWAY, message)
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

fn require_same_site(headers: &HeaderMap) -> Result<(), GmailError> {
    let origin = headers
        .get("origin")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if origin.is_empty() {
        return Err(GmailError::bad_request(
            "En-tete Origin requis pour cette mutation",
        ));
    }
    Ok(())
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

fn random_secret() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn hash_token(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_bytes()))
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn link(owner_id: &str, link_id: &str, updated_at: i64) -> StoredLink {
        StoredLink {
            owner_id: owner_id.to_string(),
            link_id: link_id.to_string(),
            email: "moi@gmail.com".to_string(),
            access_token: "at".to_string(),
            refresh_token: "rt".to_string(),
            expires_at: now_ts() + 3600,
            scopes: vec![DEFAULT_SCOPES.to_string()],
            linked_at: updated_at - 100,
            updated_at,
            needs_relink: false,
        }
    }

    #[test]
    fn store_keeps_every_linked_mailbox_and_dedupes_by_link_id() {
        // Un proprietaire peut lier plusieurs boites ; seul un doublon du meme
        // `link_id` (fichier corrompu) est retire, en gardant le plus recent.
        let mut store = GmailStore {
            version: STORE_VERSION,
            links: vec![
                link("user-1", "a", 100),
                link("user-2", "b", 200),
                link("user-1", "c", 300),
                link("user-1", "a", 400),
            ],
        };
        store.links.sort_by_key(|link| std::cmp::Reverse(link.updated_at));
        let mut seen_links = std::collections::HashSet::new();
        store
            .links
            .retain(|link| seen_links.insert(link.link_id.clone()));
        assert_eq!(store.links.len(), 3);
        let user1 = store
            .links
            .iter()
            .filter(|link| link.owner_id == "user-1")
            .collect::<Vec<_>>();
        assert_eq!(user1.len(), 2);
        let duplicate = user1
            .iter()
            .find(|link| link.link_id == "a")
            .expect("doublon a");
        assert_eq!(duplicate.updated_at, 400);
    }

    #[test]
    fn legacy_links_without_id_get_one_on_load() {
        let mut store = GmailStore {
            version: 1,
            links: vec![StoredLink {
                owner_id: "user-1".to_string(),
                link_id: String::new(),
                email: "moi@gmail.com".to_string(),
                access_token: "at".to_string(),
                refresh_token: "rt".to_string(),
                expires_at: now_ts() + 3600,
                scopes: vec![DEFAULT_SCOPES.to_string()],
                linked_at: now_ts(),
                updated_at: now_ts(),
                needs_relink: false,
            }],
        };
        for link in store.links.iter_mut() {
            if link.link_id.trim().is_empty() {
                link.link_id = random_secret();
            }
        }
        assert!(!store.links[0].link_id.is_empty());
    }

    #[test]
    fn connection_view_exposes_every_account_without_tokens() {
        let view = GmailConnectionView {
            configured: true,
            connected: true,
            email: Some("moi@gmail.com".to_string()),
            needs_relink: false,
            scopes: vec![DEFAULT_SCOPES.to_string()],
            linked_at: Some(100),
            accounts: vec![
                GmailAccountView {
                    link_id: "a".to_string(),
                    email: "moi@gmail.com".to_string(),
                    needs_relink: false,
                    scopes: vec![DEFAULT_SCOPES.to_string()],
                    linked_at: 100,
                    is_default: true,
                },
                GmailAccountView {
                    link_id: "b".to_string(),
                    email: "autre@gmail.com".to_string(),
                    needs_relink: true,
                    scopes: vec![DEFAULT_SCOPES.to_string()],
                    linked_at: 50,
                    is_default: false,
                },
            ],
            redirect_uri: "https://switch.test/api/gmail/callback".to_string(),
            client_id: Some("id".to_string()),
            login_url: Some("https://switch.test/api/gmail/start".to_string()),
        };
        let serialized = serde_json::to_string(&view).unwrap();
        assert!(serialized.contains("autre@gmail.com"));
        assert!(serialized.contains("\"linkId\":\"b\""));
        assert!(!serialized.contains("accessToken"));
        assert!(!serialized.contains("refreshToken"));
        assert!(!serialized.contains("clientSecret"));
    }

    #[test]
    fn login_url_reuses_the_callback_origin() {
        let url = gmail_login_url("https://switch.test/api/gmail/callback").unwrap();
        assert_eq!(url, "https://switch.test/api/gmail/start");
        assert!(gmail_login_url("not-a-url").is_err());
    }

    #[test]
    fn provider_config_requires_both_credentials() {
        std::env::remove_var("CST_GOOGLE_CLIENT_ID");
        std::env::remove_var("CST_GOOGLE_CLIENT_SECRET");
        assert!(build_provider_config("https://switch.test").unwrap().is_none());

        std::env::set_var("CST_GOOGLE_CLIENT_ID", "id");
        std::env::remove_var("CST_GOOGLE_CLIENT_SECRET");
        assert!(build_provider_config("https://switch.test").is_err());

        std::env::set_var("CST_GOOGLE_CLIENT_SECRET", "secret");
        let provider = build_provider_config("https://switch.test").unwrap().unwrap();
        assert_eq!(
            provider.redirect_uri,
            "https://switch.test/api/gmail/callback"
        );
        assert!(provider.scopes.contains("gmail.readonly"));
        std::env::remove_var("CST_GOOGLE_CLIENT_ID");
        std::env::remove_var("CST_GOOGLE_CLIENT_SECRET");
    }

    #[test]
    fn parse_message_extracts_headers_and_unread_state() {
        let value = serde_json::json!({
            "id": "abc123",
            "threadId": "thread-1",
            "labelIds": ["INBOX", "UNREAD"],
            "snippet": "Bonjour, voici le devis.",
            "payload": {
                "headers": [
                    { "name": "From", "value": "Alice <alice@example.com>" },
                    { "name": "To", "value": "moi@gmail.com, autre@example.com" },
                    { "name": "Subject", "value": "Devis projet" },
                    { "name": "Date", "value": "Wed, 27 Aug 2026 10:00:00 +0200" }
                ]
            }
        });
        let message = parse_message(&value).expect("message parse");
        assert_eq!(message.id, "abc123");
        assert_eq!(message.thread_id, "thread-1");
        assert_eq!(message.from.as_deref(), Some("Alice <alice@example.com>"));
        assert_eq!(message.to.len(), 2);
        assert_eq!(message.subject, "Devis projet");
        assert!(message.unread);
        assert!(message.labels.contains(&"INBOX".to_string()));
        assert_eq!(
            message.date.as_deref(),
            Some("Wed, 27 Aug 2026 10:00:00 +0200")
        );
    }

    #[test]
    fn parse_message_defaults_subject_and_read_state() {
        let value = serde_json::json!({
            "id": "id2",
            "labelIds": ["INBOX"],
            "payload": { "headers": [] }
        });
        let message = parse_message(&value).expect("message parse");
        assert_eq!(message.subject, "(sans objet)");
        assert!(!message.unread);
        assert!(message.to.is_empty());
    }

    #[test]
    fn lifetime_has_a_floor() {
        assert_eq!(lifetime(0), MIN_ACCESS_TOKEN_LIFETIME_SECS);
        assert_eq!(lifetime(3600), 3600);
    }

    #[test]
    fn scopes_are_split_on_whitespace() {
        assert_eq!(
            split_scopes("a b  c"),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert!(split_scopes("").is_empty());
    }
}
