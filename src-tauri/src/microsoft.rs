//! Liaison d'un compte Microsoft 365 (Entra ID) au compte utilisateur de
//! l'application, puis acces Microsoft Graph a la boite mail et a l'agenda.
//!
//! Trois invariants portent la securite de ce module :
//!
//! 1. la liaison exige une session utilisateur nominative au demarrage ET au
//!    retour du fournisseur. Contrairement au flux Google d'`auth.rs`, elle ne
//!    cree jamais de compte et ne fusionne jamais deux comptes par e-mail : une
//!    adresse Outlook homonyme ne permet donc aucune prise de controle ;
//! 2. les jetons ne quittent jamais le serveur. Aucune vue serialisee sous
//!    `/api/microsoft` ne contient `accessToken` ni `refreshToken` ;
//! 3. tout envoi d'e-mail et toute ecriture d'agenda demandes par un modele
//!    sont mis en file et n'atteignent Graph qu'apres une confirmation humaine
//!    explicite. La capacite MCP d'un tour de chat reste valable deux heures et
//!    transite par un fichier temporaire local : elle ne doit jamais suffire a
//!    envoyer un e-mail au nom de l'utilisateur.

use crate::{
    auth::{AuthIdentity, AuthManager},
    fs_util, metrics,
    security::constant_time_eq,
};
use axum::{
    extract::{Path, Query, State},
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
use serde_json::{json, Value};
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
const STORE_FILE: &str = "microsoft-connections.json";
/// Configuration Entra persistee (contient le secret client, 0600). Survit aux
/// redeploiements car il vit dans le dossier de donnees, contrairement aux
/// variables d'environnement que le playbook reecrit a chaque deploiement.
const PROVIDER_FILE: &str = "microsoft-provider.json";
const MAX_CLIENT_ID_CHARS: usize = 200;
const MAX_CLIENT_SECRET_CHARS: usize = 512;
/// Cookie distinct de `cst_oauth_state` : un flux Google ouvert dans un autre
/// onglet ecraserait sinon l'etat Microsoft, produisant des « Etat invalide »
/// intermittents et incomprehensibles.
const LINK_STATE_COOKIE: &str = "cst_ms_oauth_state";
const LINK_STATE_DURATION_SECS: i64 = 10 * 60;
/// Marge de renouvellement : un jeton qui expire dans moins de deux minutes est
/// rafraichi avant l'appel plutot que de faire echouer l'outil sur un 401.
const REFRESH_MARGIN_SECS: i64 = 120;
/// Duree de vie minimale retenue quand Microsoft n'annonce pas `expires_in`.
/// Sans ce plancher, la valeur par defaut 0 rendrait le jeton expire des sa
/// reception : chaque appel declencherait un echange, donc une rotation du
/// jeton de renouvellement, donc autant d'occasions de casser la chaine.
const MIN_ACCESS_TOKEN_LIFETIME_SECS: i64 = 300;
/// Un jeton de renouvellement Entra n'a pas de date de peremption absolue : il
/// meurt apres 90 jours SANS UTILISATION. Chaque echange le fait tourner et
/// repart pour 90 jours. Un renouvellement preventif regulier suffit donc a
/// rendre la liaison permanente, meme pour un utilisateur qui n'ouvre pas
/// l'application pendant des mois.
const KEEPALIVE_MAX_AGE_SECS: i64 = 14 * 24 * 60 * 60;
const KEEPALIVE_INTERVAL_SECS: u64 = 6 * 60 * 60;
/// Premiere passe peu apres le demarrage : un serveur redemarre plus souvent
/// que l'intervalle n'executerait jamais le renouvellement preventif.
const KEEPALIVE_STARTUP_DELAY_SECS: u64 = 5 * 60;
const ACTION_TTL_SECONDS: i64 = 6 * 60 * 60;
/// Duree pendant laquelle la conversation continue de proposer la liaison apres
/// qu'un outil l'a reclamee. Assez longue pour survivre a l'aller-retour vers
/// Microsoft, assez courte pour ne pas harceler quelqu'un qui a renonce.
const LINK_REQUEST_TTL_SECONDS: i64 = 60 * 60;
const MAX_PENDING_ACTIONS_PER_OWNER: usize = 20;
const DEFAULT_AUTHORITY: &str = "https://login.microsoftonline.com";
const DEFAULT_GRAPH: &str = "https://graph.microsoft.com/v1.0";
const DEFAULT_SCOPES: &str =
    "offline_access openid profile email User.Read Mail.Send Mail.Read Calendars.ReadWrite";

const MAX_RECIPIENTS: usize = 25;
const MAX_SUBJECT_CHARS: usize = 255;
const MAX_BODY_CHARS: usize = 20_000;
const MAX_QUERY_CHARS: usize = 200;
const MAX_LOCATION_CHARS: usize = 255;
const MAX_EVENT_DAYS: i64 = 62;

// ---------------------------------------------------------------------------
// Etat persistant
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MicrosoftStore {
    version: u32,
    #[serde(default)]
    links: Vec<StoredLink>,
}

impl Default for MicrosoftStore {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            links: Vec::new(),
        }
    }
}

/// Une liaison entre un compte de l'application (`owner_id`) et une boite
/// Microsoft. `oid` est l'identifiant immuable du compte Microsoft : c'est lui
/// qui porte l'unicite, jamais l'adresse e-mail qui peut etre reattribuee.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredLink {
    owner_id: String,
    oid: String,
    #[serde(default)]
    tenant_id: String,
    email: String,
    #[serde(default)]
    display_name: Option<String>,
    access_token: String,
    refresh_token: String,
    expires_at: i64,
    #[serde(default)]
    scopes: Vec<String>,
    linked_at: i64,
    /// Date du dernier echange reussi avec Entra. C'est elle, et non
    /// `linked_at`, qui gouverne la fenetre d'inactivite de 90 jours.
    updated_at: i64,
    /// Autorisation morte cote Microsoft (mot de passe change, consentement
    /// revoque, 90 jours d'inactivite). On garde l'identite pour que
    /// l'interface puisse proposer de relier CE compte, mais plus aucun jeton.
    #[serde(default)]
    needs_relink: bool,
    /// Boite utilisee quand un outil ne precise pas laquelle. Un seul compte par
    /// proprietaire peut l'etre ; la normalisation au chargement le garantit.
    #[serde(default)]
    is_default: bool,
}

/// Une boite liee, telle que vue par l'interface. Jamais de jeton ici.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MicrosoftAccountView {
    oid: String,
    email: String,
    display_name: Option<String>,
    is_default: bool,
    needs_relink: bool,
    scopes: Vec<String>,
    linked_at: i64,
}

/// Vue publique de la liaison. N'y ajoutez jamais de champ de jeton : elle est
/// serialisee sous `/api`, joignable depuis n'importe quelle origine.
///
/// Les champs de tete (`email`, `display_name`, `linked_at`, `scopes`) refletent
/// la boite par defaut : ils gardent l'interface d'avant le multi-comptes
/// fonctionnelle. `accounts` porte la liste complete.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MicrosoftConnectionView {
    configured: bool,
    /// Au moins une boite est utilisable.
    connected: bool,
    /// Aucune boite utilisable, mais au moins une a relier : l'interface propose
    /// de relier plutot que de connecter.
    needs_relink: bool,
    accounts: Vec<MicrosoftAccountView>,
    email: Option<String>,
    display_name: Option<String>,
    scopes: Vec<String>,
    linked_at: Option<i64>,
    tenant: Option<String>,
    redirect_uri: Option<String>,
    /// Point de depart de la liaison, sur l'origine publique du callback : un
    /// client ouvert derriere un tunnel local bascule ainsi sur la bonne
    /// origine avant de contacter Microsoft.
    login_url: Option<String>,
    /// Identifiant d'application Entra courant (jamais le secret). Sert a
    /// pre-remplir le formulaire de configuration dans l'application.
    client_id: Option<String>,
    /// URI de redirection a declarer dans Entra, calculee des l'origine publique
    /// meme quand rien n'est encore configure : l'utilisateur en a besoin pour
    /// remplir son inscription Entra AVANT de coller ses identifiants ici.
    default_redirect_uri: String,
    /// `env`, `app`, ou null quand rien n'est configure. Indique a l'interface
    /// si la configuration peut etre modifiee depuis l'application.
    provider_source: Option<String>,
}

struct PendingLink {
    code_verifier: String,
    owner_id: String,
    expires_at: i64,
}

// ---------------------------------------------------------------------------
// Actions en attente de confirmation humaine
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EmailDraft {
    pub to: Vec<String>,
    #[serde(default)]
    pub cc: Vec<String>,
    pub subject: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EventDraft {
    pub subject: String,
    /// Instants normalises en UTC, format Graph `AAAA-MM-JJTHH:MM:SS`.
    pub start: String,
    pub end: String,
    #[serde(default)]
    pub attendees: Vec<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub online_meeting: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EventUpdateDraft {
    pub event_id: String,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub start: Option<String>,
    #[serde(default)]
    pub end: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    /// Rappel de l'intitule courant, uniquement pour que l'humain sache quel
    /// evenement il confirme. Renseigne par le serveur, jamais par le modele.
    #[serde(default)]
    pub current_subject: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum MicrosoftDraft {
    SendEmail(EmailDraft),
    CreateEvent(EventDraft),
    UpdateEvent(EventUpdateDraft),
}

impl MicrosoftDraft {
    fn kind(&self) -> &'static str {
        match self {
            Self::SendEmail(_) => "sendEmail",
            Self::CreateEvent(_) => "createEvent",
            Self::UpdateEvent(_) => "updateEvent",
        }
    }

    fn summary(&self) -> String {
        match self {
            Self::SendEmail(draft) => {
                format!("E-mail « {} » a {}", draft.subject, draft.to.join(", "))
            }
            Self::CreateEvent(draft) => {
                format!("Nouvel evenement « {} » le {}", draft.subject, draft.start)
            }
            Self::UpdateEvent(draft) => format!(
                "Modification de l'evenement « {} »",
                draft
                    .current_subject
                    .clone()
                    .or_else(|| draft.subject.clone())
                    .unwrap_or_else(|| draft.event_id.clone())
            ),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingMicrosoftAction {
    pub id: String,
    #[serde(skip)]
    pub owner_id: String,
    pub kind: String,
    pub summary: String,
    pub draft: MicrosoftDraft,
    pub source_chat_key: Option<String>,
    pub created_at: i64,
    pub expires_at: i64,
    /// Brouillon prepare alors qu'aucune boite utilisable n'etait disponible. Le
    /// contenu est conserve : perdre un e-mail deja redige parce que la liaison
    /// manquait obligerait a tout refaire apres la connexion.
    pub requires_link: bool,
    /// Boite depuis laquelle l'action partira, resolue a la mise en file. Reste
    /// interne : la liste est deja filtree par session.
    #[serde(skip)]
    pub account_oid: Option<String>,
    /// Adresse de l'expediteur, affichee sur la carte de confirmation. Avec
    /// plusieurs boites liees, l'humain doit voir laquelle partira, et pouvoir
    /// en changer, avant de valider un envoi irreversible.
    pub account_email: Option<String>,
    /// Vrai quand `requires_link` vient d'une boite visee revoquee ALORS qu'une
    /// autre boite saine existe. Le message au modele doit alors parler de
    /// relier cette boite ou de changer d'expediteur, pas de « connecter un
    /// compte » — il y en a deja un qui marche.
    pub has_other_usable_account: bool,
}

/// Boite resolue pour une action : son identifiant immuable, son adresse et un
/// jeton d'acces frais.
struct ResolvedAccount {
    oid: String,
    email: String,
    token: String,
}

/// Trace d'un outil qui a reclame la boite Microsoft sans la trouver. Elle
/// remonte a l'interface pour que la conversation propose la liaison, au lieu
/// de laisser le modele annoncer un echec que l'utilisateur ne sait pas reparer.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MicrosoftLinkRequest {
    requested_at: i64,
    expires_at: i64,
    /// Vrai quand le serveur porte bien une configuration Entra : sinon la
    /// carte doit expliquer que rien ne peut etre lie ici, plutot que d'offrir
    /// un bouton qui ne menerait nulle part.
    configured: bool,
    /// Vrai quand un compte existe mais que son autorisation est morte.
    needs_relink: bool,
    login_url: Option<String>,
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration de l'application Entra (client_id/secret/tenant). Contient le
/// secret client : jamais serialisee dans une reponse `/api`, mais persistee
/// sur disque (0600) pour survivre a un redemarrage et a un redeploiement, sans
/// repasser par les variables d'environnement du serveur.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderConfig {
    client_id: String,
    client_secret: String,
    tenant: String,
    redirect_uri: String,
    login_url: String,
    scopes: String,
    /// `env` = venue des variables serveur au demarrage ; `app` = saisie dans
    /// l'application. Sert seulement a informer l'interface.
    #[serde(default = "provider_source_env")]
    source: String,
}

fn provider_source_env() -> String {
    "env".to_string()
}

/// Enveloppe persistee du provider, versionnee comme le store des liaisons.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderStore {
    #[serde(default = "store_version")]
    version: u32,
    provider: ProviderConfig,
}

fn store_version() -> u32 {
    STORE_VERSION
}

struct RuntimeConfig {
    store_path: PathBuf,
    provider_path: PathBuf,
    secure_cookie: bool,
    authority_base: String,
    graph_base: String,
    /// Origine publique du noeud, pour recalculer l'URI de redirection par
    /// defaut quand l'utilisateur configure Entra depuis l'application.
    public_base_url: String,
}

struct MicrosoftState {
    store: MicrosoftStore,
    /// Configuration Entra courante. Mutable a chaud : l'utilisateur peut la
    /// saisir dans l'application sans redemarrer ni redeployer le serveur.
    provider: Option<ProviderConfig>,
    pending_links: HashMap<String, PendingLink>,
    actions: Vec<PendingMicrosoftAction>,
    /// Derniere reclamation de liaison, par proprietaire.
    link_requests: HashMap<String, i64>,
    /// Un verrou asynchrone par boite (`oid`). Entra invalide l'ancien jeton de
    /// renouvellement a chaque echange : deux renouvellements simultanes de la
    /// meme boite en perdraient un, et la liaison mourrait sans que personne
    /// n'ait rien fait. Deux boites differentes se rafraichissent en parallele.
    refresh_locks: HashMap<String, Arc<tokio::sync::Mutex<()>>>,
}

#[derive(Clone)]
pub(crate) struct MicrosoftManager {
    inner: Arc<Mutex<MicrosoftState>>,
    config: Arc<RuntimeConfig>,
    http: reqwest::Client,
    auth: AuthManager,
}

// ---------------------------------------------------------------------------
// Erreurs
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct MicrosoftError {
    status: StatusCode,
    message: String,
}

impl MicrosoftError {
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

    fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, message)
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }
}

impl IntoResponse for MicrosoftError {
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

// ---------------------------------------------------------------------------
// Chargement
// ---------------------------------------------------------------------------

impl MicrosoftManager {
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
            let mut parsed: MicrosoftStore = serde_json::from_str(&content)
                .map_err(|error| format!("{} invalide: {error}", store_path.display()))?;
            if parsed.version == 0 {
                parsed.version = STORE_VERSION;
            }
            if parsed.version > STORE_VERSION {
                return Err(format!(
                    "{} utilise une version de connexions Microsoft plus recente ({})",
                    store_path.display(),
                    parsed.version
                ));
            }
            parsed
        } else {
            MicrosoftStore::default()
        };

        // Les fichiers ecrits avant le multi-comptes n'ont pas de defaut, et un
        // fichier trafique pourrait en marquer plusieurs : on garantit
        // exactement une boite par defaut et par proprietaire avant de servir.
        let mut store = store;
        normalize_defaults(&mut store.links);

        // Deux sources possibles pour la configuration Entra, dans cet ordre :
        // 1. le fichier persiste ecrit depuis l'application (il survit aux
        //    redeploiements, contrairement aux variables d'environnement que le
        //    playbook reecrit) ;
        // 2. a defaut, les variables d'environnement du serveur.
        // Une faute de frappe desactive l'integration mais n'empeche jamais le
        // noeud de demarrer : divergence assumee avec le flux Google d'auth.rs.
        let provider_path = data_dir.join(PROVIDER_FILE);
        // Repli sur les variables d'environnement, avec un message clair.
        let provider_from_env = || match build_provider_config(public_base_url) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("Microsoft 365 desactive : {error}");
                None
            }
        };
        let provider = match load_persisted_provider(&provider_path) {
            Ok(Some(provider)) => Some(provider),
            Ok(None) => provider_from_env(),
            // Fichier present mais illisible (tronque, schema incompatible, copie
            // d'un autre noeud) : on ne desactive PAS l'integration en silence,
            // on retombe sur les variables d'environnement — precisement le cas
            // ou ce repli sert. La corruption reste signalee a l'operateur.
            Err(error) => {
                eprintln!(
                    "Configuration Microsoft persistee illisible ({error}) — repli sur les variables d'environnement"
                );
                provider_from_env()
            }
        };

        let secure_cookie = public_base_url.trim().starts_with("https://");
        let authority_base = env_trimmed("CST_MICROSOFT_AUTHORITY_BASE_URL")
            .unwrap_or_else(|| DEFAULT_AUTHORITY.to_string());
        let graph_base = env_trimmed("CST_MICROSOFT_GRAPH_BASE_URL")
            .unwrap_or_else(|| DEFAULT_GRAPH.to_string());

        Ok(Self {
            inner: Arc::new(Mutex::new(MicrosoftState {
                store,
                provider,
                pending_links: HashMap::new(),
                actions: Vec::new(),
                link_requests: HashMap::new(),
                refresh_locks: HashMap::new(),
            })),
            config: Arc::new(RuntimeConfig {
                store_path,
                provider_path,
                secure_cookie,
                authority_base: authority_base.trim_end_matches('/').to_string(),
                graph_base: graph_base.trim_end_matches('/').to_string(),
                public_base_url: public_base_url.trim_end_matches('/').to_string(),
            }),
            // Timeout plus court que les 15 s d'`auth.rs` : un outil MCP doit
            // rendre une erreur lisible avant le `tool_timeout_sec=30` du CLI.
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .map_err(|error| format!("client Microsoft impossible: {error}"))?,
            auth,
        })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, MicrosoftState>, MicrosoftError> {
        self.inner
            .lock()
            .map_err(|_| MicrosoftError::internal("Verrou Microsoft indisponible"))
    }

    /// Renvoie une COPIE de la configuration Entra courante. La config vit
    /// desormais derriere le `Mutex` (elle est modifiable a chaud), donc on ne
    /// peut pas rendre une reference : chaque appelant travaillait deja sur un
    /// clone.
    fn provider(&self) -> Result<ProviderConfig, MicrosoftError> {
        self.provider_opt()?.ok_or_else(|| {
            MicrosoftError::new(
                StatusCode::NOT_FOUND,
                "Connexion Microsoft 365 non configuree sur ce serveur",
            )
        })
    }

    fn provider_opt(&self) -> Result<Option<ProviderConfig>, MicrosoftError> {
        Ok(self.lock()?.provider.clone())
    }

    /// Persiste la configuration Entra sur disque (0600). Ecrase le fichier ; un
    /// `provider` `None` le supprime.
    fn persist_provider(&self, provider: Option<&ProviderConfig>) -> Result<(), String> {
        match provider {
            Some(provider) => {
                let store = ProviderStore {
                    version: STORE_VERSION,
                    provider: provider.clone(),
                };
                let content =
                    serde_json::to_vec_pretty(&store).map_err(|error| error.to_string())?;
                fs_util::atomic_write(&self.config.provider_path, content).map_err(|error| {
                    format!(
                        "ecriture de {} impossible: {error}",
                        self.config.provider_path.display()
                    )
                })?;
                restrict_permissions(&self.config.provider_path)
            }
            None => {
                if self.config.provider_path.exists() {
                    fs::remove_file(&self.config.provider_path)
                        .map_err(|error| error.to_string())?;
                }
                Ok(())
            }
        }
    }

    /// Origine publique du noeud, pour l'URI de redirection par defaut.
    fn default_redirect_uri(&self) -> String {
        format!("{}/api/microsoft/callback", self.config.public_base_url)
    }

    /// Configure Entra depuis l'application : valide les entrees, construit la
    /// config, la met en service a chaud et la persiste (0600).
    fn set_provider(&self, input: ProviderInput) -> Result<(), MicrosoftError> {
        let client_id = input.client_id.trim().to_string();
        if client_id.is_empty() || client_id.chars().count() > MAX_CLIENT_ID_CHARS {
            return Err(MicrosoftError::bad_request(
                "L'identifiant d'application (client ID) est requis",
            ));
        }
        // Un secret vide en reconfiguration conserve le secret existant : on ne
        // le renvoie jamais a l'interface, l'utilisateur ne peut donc pas le
        // recopier. C'est ce que promet le libelle « laisser vide pour
        // conserver ». Sans config prealable, il reste obligatoire.
        let secret_input = input.client_secret.trim();
        let client_secret = if secret_input.is_empty() {
            self.lock()?
                .provider
                .as_ref()
                .map(|provider| provider.client_secret.clone())
                .ok_or_else(|| MicrosoftError::bad_request("Le secret client est requis"))?
        } else {
            if secret_input.chars().count() > MAX_CLIENT_SECRET_CHARS {
                return Err(MicrosoftError::bad_request(
                    "Le secret client est trop long",
                ));
            }
            secret_input.to_string()
        };
        let tenant = input
            .tenant
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("common")
            .to_string();
        if !tenant
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'))
        {
            return Err(MicrosoftError::bad_request(
                "Le tenant ne peut contenir que lettres, chiffres, tiret et point",
            ));
        }
        let redirect_uri = input
            .redirect_uri
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.default_redirect_uri());
        // Le point de depart de la liaison (login_url) derive de l'origine du
        // redirect_uri et est propose a TOUS les utilisateurs via la vue de
        // connexion. Comme la configuration est saisissable par toute session
        // (choix assume pour un reseau ferme), on VERROUILLE le redirect_uri sur
        // l'origine publique du noeud : sans ce garde, un utilisateur pourrait
        // pointer le bouton « Connecter » des autres vers un serveur tiers et
        // hameçonner leur consentement Microsoft. Entra n'accepte de toute façon
        // que le callback du noeud, il n'y a donc aucune raison legitime d'en
        // viser un autre.
        if !same_origin(&redirect_uri, &self.config.public_base_url) {
            return Err(MicrosoftError::bad_request(
                "L'URI de redirection doit rester sur l'origine publique de ce serveur",
            ));
        }
        // Meme validation que pour la config par variables d'environnement :
        // HTTPS hors boucle locale, sinon Entra refusera le retour.
        let login_url = microsoft_login_url(&redirect_uri).map_err(MicrosoftError::bad_request)?;

        let provider = ProviderConfig {
            client_id,
            client_secret,
            tenant,
            redirect_uri,
            login_url,
            scopes: input
                .scopes
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(DEFAULT_SCOPES)
                .to_string(),
            source: "app".to_string(),
        };

        self.persist_provider(Some(&provider))
            .map_err(MicrosoftError::internal)?;
        self.lock()?.provider = Some(provider);
        Ok(())
    }

    /// Retire la configuration Entra. Les boites liees sont conservees mais
    /// deviennent inutilisables jusqu'a une reconfiguration.
    fn clear_provider(&self) -> Result<(), MicrosoftError> {
        self.persist_provider(None)
            .map_err(MicrosoftError::internal)?;
        self.lock()?.provider = None;
        Ok(())
    }

    fn persist_locked(&self, state: &MicrosoftState) -> Result<(), String> {
        let content = serde_json::to_vec_pretty(&state.store).map_err(|error| error.to_string())?;
        fs_util::atomic_write(&self.config.store_path, content).map_err(|error| {
            format!(
                "ecriture de {} impossible: {error}",
                self.config.store_path.display()
            )
        })?;
        restrict_permissions(&self.config.store_path)
    }

    fn identity(&self, headers: &HeaderMap) -> Result<AuthIdentity, MicrosoftError> {
        self.auth
            .identity_from_headers(headers)
            .map_err(MicrosoftError::internal)?
            .ok_or_else(|| MicrosoftError::unauthorized("Session utilisateur requise"))
    }

    // -----------------------------------------------------------------------
    // Vue publique
    // -----------------------------------------------------------------------

    fn connection_view(&self, owner_id: &str) -> Result<MicrosoftConnectionView, MicrosoftError> {
        let state = self.lock()?;
        let provider = state.provider.as_ref();
        let mut accounts = state
            .store
            .links
            .iter()
            .filter(|link| link.owner_id == owner_id)
            .map(|link| MicrosoftAccountView {
                oid: link.oid.clone(),
                email: link.email.clone(),
                display_name: link.display_name.clone(),
                is_default: link.is_default,
                needs_relink: link.needs_relink,
                scopes: link.scopes.clone(),
                linked_at: link.linked_at,
            })
            .collect::<Vec<_>>();
        // Defaut d'abord, puis par anciennete : la liste est stable d'un
        // affichage a l'autre et l'expediteur par defaut saute aux yeux.
        accounts.sort_by(|a, b| {
            b.is_default
                .cmp(&a.is_default)
                .then(a.linked_at.cmp(&b.linked_at))
        });
        // Sans configuration Entra, aucune boite n'est utilisable meme si des
        // liaisons subsistent : elles ne peuvent plus ni se rafraichir ni
        // servir. L'interface affiche alors « a configurer », pas « connecte ».
        let configured = provider.is_some();
        let connected = configured && accounts.iter().any(|account| !account.needs_relink);
        let needs_relink = configured && !connected && !accounts.is_empty();
        let default = accounts
            .iter()
            .find(|account| account.is_default)
            .or_else(|| accounts.first());
        Ok(MicrosoftConnectionView {
            configured,
            connected,
            needs_relink,
            email: default.map(|account| account.email.clone()),
            display_name: default.and_then(|account| account.display_name.clone()),
            scopes: default
                .map(|account| account.scopes.clone())
                .unwrap_or_default(),
            linked_at: default.map(|account| account.linked_at),
            accounts,
            tenant: provider.map(|provider| provider.tenant.clone()),
            redirect_uri: provider.map(|provider| provider.redirect_uri.clone()),
            login_url: provider.map(|provider| provider.login_url.clone()),
            client_id: provider.map(|provider| provider.client_id.clone()),
            default_redirect_uri: self.default_redirect_uri(),
            provider_source: provider.map(|provider| provider.source.clone()),
        })
    }

    /// Retire toutes les boites du proprietaire.
    fn disconnect(&self, owner_id: &str) -> Result<(), MicrosoftError> {
        let mut state = self.lock()?;
        let previous = state.store.links.len();
        let removed_oids = state
            .store
            .links
            .iter()
            .filter(|link| link.owner_id == owner_id)
            .map(|link| link.oid.clone())
            .collect::<Vec<_>>();
        state.store.links.retain(|link| link.owner_id != owner_id);
        state.actions.retain(|action| action.owner_id != owner_id);
        state.link_requests.remove(owner_id);
        for oid in &removed_oids {
            state.refresh_locks.remove(oid);
        }
        if state.store.links.len() != previous {
            self.persist_locked(&state)
                .map_err(MicrosoftError::internal)?;
        }
        Ok(())
    }

    /// Retire une seule boite. Si c'etait la boite par defaut, la plus ancienne
    /// des restantes prend le relais pour qu'un envoi sans precision reste
    /// possible.
    fn disconnect_account(&self, owner_id: &str, oid: &str) -> Result<(), MicrosoftError> {
        let mut state = self.lock()?;
        let existed = state
            .store
            .links
            .iter()
            .any(|link| link.owner_id == owner_id && link.oid == oid);
        if !existed {
            return Err(MicrosoftError::not_found("Cette boite n'est pas liee"));
        }
        state
            .store
            .links
            .retain(|link| !(link.owner_id == owner_id && link.oid == oid));
        // Les brouillons qui partaient de cette boite n'ont plus d'expediteur :
        // les garder afficherait un envoi impossible.
        state
            .actions
            .retain(|action| action.account_oid.as_deref() != Some(oid));
        state.refresh_locks.remove(oid);
        normalize_defaults(&mut state.store.links);
        self.persist_locked(&state)
            .map_err(MicrosoftError::internal)?;
        Ok(())
    }

    /// Choisit la boite utilisee quand un outil ne precise rien.
    fn set_default_account(&self, owner_id: &str, oid: &str) -> Result<(), MicrosoftError> {
        let mut state = self.lock()?;
        if !state
            .store
            .links
            .iter()
            .any(|link| link.owner_id == owner_id && link.oid == oid)
        {
            return Err(MicrosoftError::not_found("Cette boite n'est pas liee"));
        }
        for link in state
            .store
            .links
            .iter_mut()
            .filter(|link| link.owner_id == owner_id)
        {
            link.is_default = link.oid == oid;
        }
        self.persist_locked(&state)
            .map_err(MicrosoftError::internal)?;
        Ok(())
    }

    /// Boites du proprietaire, defaut en tete.
    fn owner_links(&self, owner_id: &str) -> Vec<StoredLink> {
        let Ok(state) = self.inner.lock() else {
            return Vec::new();
        };
        let mut links = state
            .store
            .links
            .iter()
            .filter(|link| link.owner_id == owner_id)
            .cloned()
            .collect::<Vec<_>>();
        links.sort_by(|a, b| {
            b.is_default
                .cmp(&a.is_default)
                .then(a.linked_at.cmp(&b.linked_at))
        });
        links
    }

    fn owner_has_no_usable_account(&self, owner_id: &str) -> bool {
        !self
            .owner_links(owner_id)
            .iter()
            .any(|link| !link.needs_relink)
    }

    /// Resout la boite ciblee par un outil. Sans indice, la boite par defaut ;
    /// avec un indice, la boite du proprietaire dont l'adresse correspond. Un
    /// indice qui ne correspond a aucune de SES boites est une erreur explicite,
    /// jamais une invitation a lier un compte tiers.
    fn resolve_oid(
        &self,
        owner_id: &str,
        account_hint: Option<&str>,
    ) -> Result<String, MicrosoftError> {
        let links = self.owner_links(owner_id);
        if links.is_empty() {
            return Err(MicrosoftError::not_found(
                "Aucun compte Microsoft n'est lie a votre compte. Ouvrez Mon compte pour le connecter.",
            ));
        }
        match account_hint.map(str::trim).filter(|value| !value.is_empty()) {
            Some(hint) => links
                .iter()
                .find(|link| link.email.eq_ignore_ascii_case(hint))
                .map(|link| link.oid.clone())
                .ok_or_else(|| {
                    let available = links
                        .iter()
                        .map(|link| link.email.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    MicrosoftError::bad_request(format!(
                        "« {hint} » ne fait pas partie de vos boites Microsoft liees. Boites disponibles : {available}."
                    ))
                }),
            // Sans indice : la boite par defaut si elle est saine, sinon la plus
            // ancienne boite utilisable. `owner_links` trie defaut d'abord puis
            // par anciennete, donc `find(!needs_relink)` donne exactement cela.
            // On ne retombe sur une boite morte que si TOUTES le sont — cas ou
            // la carte « connecter » s'affiche de toute facon. Sans ce filtre,
            // un defaut revoque ferait echouer une lecture alors qu'une boite
            // saine existe, sans aucun recours pour l'utilisateur.
            None => Ok(links
                .iter()
                .find(|link| !link.needs_relink)
                .unwrap_or(&links[0])
                .oid
                .clone()),
        }
    }

    // -----------------------------------------------------------------------
    // Flux de liaison OAuth
    // -----------------------------------------------------------------------

    fn begin_link(&self, headers: &HeaderMap) -> Result<(String, String), MicrosoftError> {
        let identity = self.identity(headers)?;
        let provider = self.provider()?;
        let state_token = random_secret();
        let code_verifier = format!("{}{}", random_secret(), random_secret()).replace('-', "");
        let code_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()));
        let now = metrics::now_ts();

        let mut state = self.lock()?;
        state
            .pending_links
            .retain(|_, pending| pending.expires_at > now);
        state.pending_links.insert(
            hash_token(&state_token),
            PendingLink {
                code_verifier,
                owner_id: identity.id.clone(),
                expires_at: now + LINK_STATE_DURATION_SECS,
            },
        );
        drop(state);

        let mut url = Url::parse(&format!(
            "{}/{}/oauth2/v2.0/authorize",
            self.config.authority_base, provider.tenant
        ))
        .map_err(|error| MicrosoftError::internal(error.to_string()))?;
        url.query_pairs_mut()
            .append_pair("client_id", &provider.client_id)
            .append_pair("redirect_uri", &provider.redirect_uri)
            .append_pair("response_type", "code")
            .append_pair("response_mode", "query")
            .append_pair("scope", &provider.scopes)
            .append_pair("state", &state_token)
            .append_pair("code_challenge", &code_challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("prompt", "select_account");
        Ok((url.into(), state_token))
    }

    async fn finish_link(
        &self,
        headers: &HeaderMap,
        code: &str,
        state_token: &str,
    ) -> Result<(), MicrosoftError> {
        let identity = self.identity(headers)?;
        let provider = self.provider()?;

        let cookie_state = cookie_value(headers, LINK_STATE_COOKIE)
            .ok_or_else(|| MicrosoftError::bad_request("Session de liaison Microsoft absente"))?;
        if !constant_time_eq(cookie_state.as_bytes(), state_token.as_bytes()) {
            return Err(MicrosoftError::bad_request("Etat de liaison invalide"));
        }

        let pending = {
            let mut state = self.lock()?;
            state
                .pending_links
                .remove(&hash_token(state_token))
                .filter(|pending| pending.expires_at > metrics::now_ts())
                .ok_or_else(|| MicrosoftError::bad_request("Session de liaison expiree"))?
        };
        // La liaison appartient a la session qui l'a demarree : un utilisateur
        // ne peut pas terminer dans son navigateur la liaison ouverte par un
        // autre compte de la meme instance.
        if pending.owner_id != identity.id {
            return Err(MicrosoftError::forbidden(
                "La liaison a ete demarree depuis un autre compte",
            ));
        }

        let token = self
            .exchange_token(
                &provider,
                &[
                    ("client_id", provider.client_id.as_str()),
                    ("client_secret", provider.client_secret.as_str()),
                    ("code", code),
                    ("code_verifier", pending.code_verifier.as_str()),
                    ("grant_type", "authorization_code"),
                    ("redirect_uri", provider.redirect_uri.as_str()),
                    ("scope", provider.scopes.as_str()),
                ],
            )
            .await?;

        // Sans `offline_access` reellement accorde, la liaison cesserait de
        // fonctionner des la premiere heure : autant la refuser tout de suite.
        let refresh_token = token.refresh_token.clone().ok_or_else(|| {
            MicrosoftError::bad_request(
                "Microsoft n'a pas fourni d'autorisation durable (offline_access). Relancez la liaison en acceptant toutes les permissions demandees.",
            )
        })?;

        let profile = self.fetch_profile(&token.access_token).await?;
        let now = metrics::now_ts();
        let mut state = self.lock()?;

        // Unicite par identifiant Microsoft immuable : deux comptes de
        // l'application ne peuvent pas piloter la meme boite.
        if state
            .store
            .links
            .iter()
            .any(|link| link.oid == profile.oid && link.owner_id != identity.id)
        {
            return Err(MicrosoftError::conflict(
                "Ce compte Microsoft est deja lie a un autre utilisateur de l'application",
            ));
        }

        let scopes = token
            .scope
            .as_deref()
            .map(|scope| {
                scope
                    .split_whitespace()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let expires_at = access_token_expiry(now, token.expires_in);
        // Premiere boite de ce proprietaire : elle devient la boite par defaut.
        let is_first = !state
            .store
            .links
            .iter()
            .any(|link| link.owner_id == identity.id);

        if let Some(existing) = state
            .store
            .links
            .iter_mut()
            // Relier la MEME boite (meme oid) renouvelle ses jetons ; une boite
            // differente s'ajoute a cote. C'est ce qui permet plusieurs comptes.
            .find(|link| link.owner_id == identity.id && link.oid == profile.oid)
        {
            existing.tenant_id = profile.tenant_id;
            existing.email = profile.email;
            existing.display_name = profile.display_name;
            existing.access_token = token.access_token;
            existing.refresh_token = refresh_token;
            existing.expires_at = expires_at;
            existing.scopes = scopes;
            existing.updated_at = now;
            // Une nouvelle autorisation ressuscite une liaison morte.
            existing.needs_relink = false;
        } else {
            state.store.links.push(StoredLink {
                owner_id: identity.id.clone(),
                oid: profile.oid,
                tenant_id: profile.tenant_id,
                email: profile.email,
                display_name: profile.display_name,
                access_token: token.access_token,
                refresh_token,
                expires_at,
                scopes,
                linked_at: now,
                updated_at: now,
                needs_relink: false,
                is_default: is_first,
            });
        }
        // Relier une boite efface la demande de connexion en attente : la
        // conversation ne doit plus proposer ce que l'utilisateur vient de faire.
        state.link_requests.remove(&identity.id);
        normalize_defaults(&mut state.store.links);
        self.persist_locked(&state)
            .map_err(MicrosoftError::internal)?;
        Ok(())
    }

    async fn exchange_token(
        &self,
        provider: &ProviderConfig,
        form: &[(&str, &str)],
    ) -> Result<TokenResponse, MicrosoftError> {
        let url = format!(
            "{}/{}/oauth2/v2.0/token",
            self.config.authority_base, provider.tenant
        );
        let response =
            self.http.post(&url).form(form).send().await.map_err(|_| {
                MicrosoftError::new(StatusCode::BAD_GATEWAY, "Microsoft ne repond pas")
            })?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            let code = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|value| {
                    value
                        .get("error")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_default();
            if code == "invalid_grant" {
                return Err(MicrosoftError::unauthorized(
                    "L'autorisation Microsoft a expire ou a ete revoquee. Reliez le compte.",
                ));
            }
            return Err(MicrosoftError::new(
                StatusCode::BAD_GATEWAY,
                format!("Microsoft a refuse la demande d'autorisation ({status})"),
            ));
        }
        serde_json::from_str::<TokenResponse>(&body)
            .map_err(|_| MicrosoftError::new(StatusCode::BAD_GATEWAY, "Reponse Microsoft invalide"))
    }

    async fn fetch_profile(&self, access_token: &str) -> Result<GraphProfile, MicrosoftError> {
        let response = self
            .http
            .get(format!("{}/me", self.config.graph_base))
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|_| {
                MicrosoftError::new(StatusCode::BAD_GATEWAY, "Profil Microsoft inaccessible")
            })?;
        if !response.status().is_success() {
            return Err(MicrosoftError::new(
                StatusCode::BAD_GATEWAY,
                "Microsoft n'a pas retourne le profil demande",
            ));
        }
        let value: Value = response.json().await.map_err(|_| {
            MicrosoftError::new(StatusCode::BAD_GATEWAY, "Profil Microsoft invalide")
        })?;
        let oid = value
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                MicrosoftError::new(StatusCode::BAD_GATEWAY, "Profil Microsoft sans identifiant")
            })?;
        let email = value
            .get("mail")
            .and_then(Value::as_str)
            .or_else(|| value.get("userPrincipalName").and_then(Value::as_str))
            .map(str::to_string)
            .ok_or_else(|| {
                MicrosoftError::new(StatusCode::BAD_GATEWAY, "Profil Microsoft sans adresse")
            })?;
        Ok(GraphProfile {
            oid,
            tenant_id: value
                .get("@odata.context")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            email,
            display_name: value
                .get("displayName")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }

    // -----------------------------------------------------------------------
    // Jeton d'acces
    // -----------------------------------------------------------------------

    /// Resout la boite ciblee (defaut ou indice) puis renvoie un jeton frais et
    /// l'adresse correspondante. La conversation ne propose la liaison que si le
    /// proprietaire n'a AUCUNE boite utilisable : quand une seule boite parmi
    /// plusieurs est morte, c'est un probleme d'expediteur, traite sur la carte,
    /// pas une invitation a tout reconnecter.
    async fn resolve_account(
        &self,
        owner_id: &str,
        account_hint: Option<&str>,
    ) -> Result<ResolvedAccount, MicrosoftError> {
        // Sans configuration Entra (jamais posee, ou retiree apres avoir lie des
        // boites), rien n'est utilisable. On le signale comme une demande de
        // liaison pour que la conversation propose de (re)configurer plutot que
        // de renvoyer une erreur brute « non configure ».
        if self.provider_opt()?.is_none() {
            self.note_link_request(owner_id);
            return Err(MicrosoftError::not_found(
                "Microsoft 365 n'est pas configure. Ouvrez Mon compte pour saisir les identifiants de l'application.",
            ));
        }
        let oid = match self.resolve_oid(owner_id, account_hint) {
            Ok(oid) => oid,
            Err(error) => {
                if error.status == StatusCode::NOT_FOUND {
                    self.note_link_request(owner_id);
                }
                return Err(error);
            }
        };
        let email = self.email_for_oid(&oid).unwrap_or_default();
        match self.resolve_access_token(&oid).await {
            Ok(token) => Ok(ResolvedAccount { oid, email, token }),
            Err(error) => {
                if error.status == StatusCode::UNAUTHORIZED
                    && self.owner_has_no_usable_account(owner_id)
                {
                    self.note_link_request(owner_id);
                }
                Err(error)
            }
        }
    }

    fn email_for_oid(&self, oid: &str) -> Option<String> {
        let state = self.inner.lock().ok()?;
        state
            .store
            .links
            .iter()
            .find(|link| link.oid == oid)
            .map(|link| link.email.clone())
    }

    /// Marque que la boite Microsoft a ete reclamee sans etre disponible.
    fn note_link_request(&self, owner_id: &str) {
        let now = metrics::now_ts();
        if let Ok(mut state) = self.inner.lock() {
            state.link_requests.retain(|_, expires| *expires > now);
            state
                .link_requests
                .insert(owner_id.to_string(), now + LINK_REQUEST_TTL_SECONDS);
        }
    }

    fn clear_link_request(&self, owner_id: &str) {
        if let Ok(mut state) = self.inner.lock() {
            state.link_requests.remove(owner_id);
        }
    }

    fn link_request(&self, owner_id: &str) -> Result<Option<MicrosoftLinkRequest>, MicrosoftError> {
        let now = metrics::now_ts();
        let mut state = self.lock()?;
        state.link_requests.retain(|_, expires| *expires > now);
        let Some(expires_at) = state.link_requests.get(owner_id).copied() else {
            return Ok(None);
        };
        let configured = state.provider.is_some();
        let login_url = state
            .provider
            .as_ref()
            .map(|provider| provider.login_url.clone());
        let owner_links = state
            .store
            .links
            .iter()
            .filter(|link| link.owner_id == owner_id)
            .collect::<Vec<_>>();
        // Une boite utilisable existe : la demande n'a plus d'objet. Un envoi qui
        // echoue encore vise une boite precise et se regle sur sa carte, pas ici.
        let has_usable = owner_links.iter().any(|link| !link.needs_relink);
        let has_any = !owner_links.is_empty();
        if has_usable {
            state.link_requests.remove(owner_id);
            return Ok(None);
        }
        Ok(Some(MicrosoftLinkRequest {
            requested_at: expires_at - LINK_REQUEST_TTL_SECONDS,
            expires_at,
            configured,
            // Au moins une boite existe mais toutes sont a relier.
            needs_relink: has_any,
            login_url,
        }))
    }

    async fn resolve_access_token(&self, oid: &str) -> Result<String, MicrosoftError> {
        if let Some(token) = self.cached_access_token(oid)? {
            return Ok(token);
        }
        // Un seul renouvellement a la fois par boite. Sans ce verrou, deux
        // outils lances dans le meme tour echangeraient le meme jeton de
        // renouvellement : Entra invaliderait le premier resultat et la liaison
        // deviendrait irrecuperable sans intervention de l'utilisateur.
        let gate = self.refresh_gate(oid)?;
        let _guard = gate.lock().await;
        // Le detenteur precedent du verrou vient peut-etre de renouveler.
        if let Some(token) = self.cached_access_token(oid)? {
            return Ok(token);
        }
        self.refresh_now(oid).await
    }

    /// Jeton encore valide pour cette boite, s'il y en a un. `Err` distingue les
    /// deux situations que l'interface traite differemment : boite absente, ou
    /// boite dont l'autorisation est morte.
    fn cached_access_token(&self, oid: &str) -> Result<Option<String>, MicrosoftError> {
        let state = self.lock()?;
        let link = state
            .store
            .links
            .iter()
            .find(|link| link.oid == oid)
            .ok_or_else(|| MicrosoftError::not_found("Cette boite Microsoft n'est plus liee."))?;
        if link.needs_relink {
            return Err(relink_required());
        }
        Ok(
            (!link.access_token.is_empty() && link.expires_at > metrics::now_ts())
                .then(|| link.access_token.clone()),
        )
    }

    fn refresh_gate(&self, oid: &str) -> Result<Arc<tokio::sync::Mutex<()>>, MicrosoftError> {
        let mut state = self.lock()?;
        Ok(state
            .refresh_locks
            .entry(oid.to_string())
            .or_default()
            .clone())
    }

    /// Echange le jeton de renouvellement d'une boite contre un jeton frais.
    /// Appele aussi par le renouvellement preventif : c'est cet echange, et lui
    /// seul, qui repousse la fenetre d'inactivite de 90 jours d'Entra.
    async fn refresh_now(&self, oid: &str) -> Result<String, MicrosoftError> {
        let provider = self.provider()?;
        let refresh_token = {
            let state = self.lock()?;
            let link = state
                .store
                .links
                .iter()
                .find(|link| link.oid == oid)
                .ok_or_else(|| {
                    MicrosoftError::not_found("Cette boite Microsoft n'est plus liee.")
                })?;
            if link.needs_relink || link.refresh_token.is_empty() {
                return Err(relink_required());
            }
            link.refresh_token.clone()
        };

        let refreshed = match self
            .exchange_token(
                &provider,
                &[
                    ("client_id", provider.client_id.as_str()),
                    ("client_secret", provider.client_secret.as_str()),
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh_token.as_str()),
                    ("scope", provider.scopes.as_str()),
                ],
            )
            .await
        {
            Ok(value) => value,
            Err(error) if error.status == StatusCode::UNAUTHORIZED => {
                // Autorisation morte cote Microsoft. On efface les jetons mais
                // on conserve l'identite : l'utilisateur doit pouvoir relier
                // le meme compte en un clic, pas repartir d'une page vide.
                self.mark_needs_relink(oid)?;
                return Err(error);
            }
            // Panne reseau ou 5xx : surtout ne rien effacer, la liaison est
            // probablement intacte et le prochain appel reussira.
            Err(error) => return Err(error),
        };

        let now = metrics::now_ts();
        let expires_at = access_token_expiry(now, refreshed.expires_in);
        let access_token = refreshed.access_token.clone();
        let mut state = self.lock()?;
        if let Some(stored) = state
            .store
            .links
            .iter_mut()
            .find(|stored| stored.oid == oid)
        {
            stored.access_token = refreshed.access_token;
            // Entra fait tourner le jeton de renouvellement a chaque echange :
            // ne jamais supposer qu'il reste stable, ni l'ecraser par du vide.
            if let Some(rotated) = refreshed.refresh_token {
                stored.refresh_token = rotated;
            }
            stored.expires_at = expires_at;
            if let Some(scope) = refreshed.scope {
                stored.scopes = scope.split_whitespace().map(str::to_string).collect();
            }
            stored.updated_at = now;
            stored.needs_relink = false;
            self.persist_locked(&state)
                .map_err(MicrosoftError::internal)?;
        }
        Ok(access_token)
    }

    fn mark_needs_relink(&self, oid: &str) -> Result<(), MicrosoftError> {
        let mut state = self.lock()?;
        if let Some(link) = state.store.links.iter_mut().find(|link| link.oid == oid) {
            link.access_token.clear();
            link.refresh_token.clear();
            link.expires_at = 0;
            link.needs_relink = true;
            link.updated_at = metrics::now_ts();
            self.persist_locked(&state)
                .map_err(MicrosoftError::internal)?;
        }
        Ok(())
    }

    /// Boites dont le jeton de renouvellement n'a pas servi depuis assez
    /// longtemps pour que la fenetre d'inactivite d'Entra commence a compter.
    /// Chaque boite est suivie separement : deux comptes du meme utilisateur ne
    /// se rafraichissent pas au meme rythme.
    fn oids_to_keep_alive(&self) -> Vec<String> {
        let now = metrics::now_ts();
        let Ok(state) = self.inner.lock() else {
            return Vec::new();
        };
        state
            .store
            .links
            .iter()
            .filter(|link| needs_keepalive(link.updated_at, link.needs_relink, now))
            .map(|link| link.oid.clone())
            .collect()
    }

    /// Renouvellement preventif : sans lui, un compte lie puis inutilise perdrait
    /// son autorisation au bout de 90 jours et l'utilisateur devrait relier son
    /// compte sans comprendre pourquoi. Avec lui, la liaison est permanente tant
    /// que le serveur tourne et que l'utilisateur ne revoque rien cote Microsoft.
    pub(crate) fn start_keepalive(&self) {
        // On demarre toujours la boucle : la configuration Entra peut etre
        // saisie dans l'application apres le lancement du serveur. Sans provider,
        // `oids_to_keep_alive` reste vide (aucune boite ne peut etre liee) et la
        // boucle ne fait rien.
        let manager = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(KEEPALIVE_STARTUP_DELAY_SECS)).await;
            loop {
                for oid in manager.oids_to_keep_alive() {
                    let gate = match manager.refresh_gate(&oid) {
                        Ok(gate) => gate,
                        Err(_) => continue,
                    };
                    let _guard = gate.lock().await;
                    if let Err(error) = manager.refresh_now(&oid).await {
                        // Pas d'identifiant dans le journal : un fichier de logs
                        // n'a pas a dire qui a lie quelle boite.
                        eprintln!(
                            "Microsoft 365 : renouvellement preventif impossible ({})",
                            error.message
                        );
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(KEEPALIVE_INTERVAL_SECS)).await;
            }
        });
    }

    // -----------------------------------------------------------------------
    // Lectures Graph
    // -----------------------------------------------------------------------

    async fn graph_get(
        &self,
        owner_id: &str,
        account_hint: Option<&str>,
        url: Url,
    ) -> Result<(Value, String), MicrosoftError> {
        let account = self.resolve_account(owner_id, account_hint).await?;
        let response = self
            .http
            .get(url)
            .bearer_auth(&account.token)
            .header(
                "Prefer",
                "outlook.timezone=\"UTC\", outlook.body-content-type=\"text\"",
            )
            .send()
            .await
            .map_err(|_| {
                MicrosoftError::new(StatusCode::BAD_GATEWAY, "Microsoft Graph ne repond pas")
            })?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(graph_error(status, &body));
        }
        let value = serde_json::from_str(&body).map_err(|_| {
            MicrosoftError::new(StatusCode::BAD_GATEWAY, "Reponse Microsoft Graph invalide")
        })?;
        Ok((value, account.email))
    }

    pub(crate) async fn list_messages(
        &self,
        owner_id: &str,
        args: &ListMessagesArguments,
    ) -> Result<Value, String> {
        self.list_messages_inner(owner_id, args)
            .await
            .map_err(|error| error.message)
    }

    async fn list_messages_inner(
        &self,
        owner_id: &str,
        args: &ListMessagesArguments,
    ) -> Result<Value, MicrosoftError> {
        let folder = args.normalized_folder()?;
        let mut url = Url::parse(&format!(
            "{}/me/mailFolders/{folder}/messages",
            self.config.graph_base
        ))
        .map_err(|error| MicrosoftError::internal(error.to_string()))?;
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("$top", &args.normalized_limit().to_string())
                .append_pair(
                    "$select",
                    "id,subject,from,receivedDateTime,bodyPreview,isRead,hasAttachments,webLink",
                );
            match args.normalized_query()? {
                // `$search` et `$orderby` sont exclusifs cote Graph : quand une
                // recherche est demandee, c'est la pertinence qui ordonne.
                Some(search) => {
                    query.append_pair("$search", &format!("\"{search}\""));
                }
                None => {
                    query.append_pair("$orderby", "receivedDateTime desc");
                }
            }
        }

        let (payload, mailbox) = self
            .graph_get(owner_id, args.account.as_deref(), url)
            .await?;
        let messages = payload
            .get("value")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|message| {
                json!({
                    "id": message.get("id").and_then(Value::as_str).unwrap_or_default(),
                    "subject": message.get("subject").and_then(Value::as_str).unwrap_or("(sans objet)"),
                    "from": message
                        .pointer("/from/emailAddress/address")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    "fromName": message
                        .pointer("/from/emailAddress/name")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    "receivedAt": message.get("receivedDateTime").and_then(Value::as_str).unwrap_or_default(),
                    "preview": message.get("bodyPreview").and_then(Value::as_str).unwrap_or_default(),
                    "isRead": message.get("isRead").and_then(Value::as_bool).unwrap_or(true),
                    "hasAttachments": message.get("hasAttachments").and_then(Value::as_bool).unwrap_or(false),
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({ "messages": messages, "count": messages.len(), "mailbox": mailbox }))
    }

    pub(crate) async fn list_events(
        &self,
        owner_id: &str,
        args: &ListEventsArguments,
    ) -> Result<Value, String> {
        self.list_events_inner(owner_id, args)
            .await
            .map_err(|error| error.message)
    }

    async fn list_events_inner(
        &self,
        owner_id: &str,
        args: &ListEventsArguments,
    ) -> Result<Value, MicrosoftError> {
        let (start, end) = args.normalized_window()?;
        let mut url = Url::parse(&format!("{}/me/calendarView", self.config.graph_base))
            .map_err(|error| MicrosoftError::internal(error.to_string()))?;
        url.query_pairs_mut()
            .append_pair("startDateTime", &start)
            .append_pair("endDateTime", &end)
            .append_pair("$top", &args.normalized_limit().to_string())
            .append_pair("$orderby", "start/dateTime")
            .append_pair(
                "$select",
                "id,subject,start,end,location,organizer,attendees,isAllDay,isCancelled,onlineMeetingUrl",
            );

        let (payload, mailbox) = self
            .graph_get(owner_id, args.account.as_deref(), url)
            .await?;
        let events = payload
            .get("value")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|event| {
                json!({
                    "id": event.get("id").and_then(Value::as_str).unwrap_or_default(),
                    "subject": event.get("subject").and_then(Value::as_str).unwrap_or("(sans titre)"),
                    "start": event.pointer("/start/dateTime").and_then(Value::as_str).unwrap_or_default(),
                    "end": event.pointer("/end/dateTime").and_then(Value::as_str).unwrap_or_default(),
                    "timeZone": event.pointer("/start/timeZone").and_then(Value::as_str).unwrap_or("UTC"),
                    "location": event
                        .pointer("/location/displayName")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    "organizer": event
                        .pointer("/organizer/emailAddress/address")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    "attendees": event
                        .get("attendees")
                        .and_then(Value::as_array)
                        .map(|attendees| {
                            attendees
                                .iter()
                                .filter_map(|attendee| {
                                    attendee
                                        .pointer("/emailAddress/address")
                                        .and_then(Value::as_str)
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default(),
                    "isAllDay": event.get("isAllDay").and_then(Value::as_bool).unwrap_or(false),
                    "isCancelled": event.get("isCancelled").and_then(Value::as_bool).unwrap_or(false),
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "events": events,
            "count": events.len(),
            "mailbox": mailbox,
            "window": { "start": start, "end": end, "timeZone": "UTC" }
        }))
    }

    /// Intitule courant d'un evenement dans une boite donnee, pour que la carte
    /// de confirmation dise a l'humain ce qui va reellement changer.
    async fn event_subject(&self, oid: &str, event_id: &str) -> Option<String> {
        let url = Url::parse(&format!(
            "{}/me/events/{}",
            self.config.graph_base,
            urlencode(event_id)
        ))
        .ok()?;
        let token = self.resolve_access_token(oid).await.ok()?;
        let response = self
            .http
            .get(url)
            .bearer_auth(token)
            .header(
                "Prefer",
                "outlook.timezone=\"UTC\", outlook.body-content-type=\"text\"",
            )
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let payload: Value = response.json().await.ok()?;
        payload
            .get("subject")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    // -----------------------------------------------------------------------
    // File de confirmation
    // -----------------------------------------------------------------------

    pub(crate) async fn enqueue(
        &self,
        owner_id: &str,
        draft: MicrosoftDraft,
        account_hint: Option<&str>,
        source_chat_key: Option<String>,
    ) -> Result<PendingMicrosoftAction, String> {
        // Un compte manquant ou revoque ne fait pas perdre le brouillon : il est
        // conserve, son envoi bloque, et la carte propose de connecter ou de
        // changer d'expediteur. Redemander au modele de tout reecrire serait la
        // pire des reponses.
        let (requires_link, account_oid, account_email) =
            match self.resolve_account(owner_id, account_hint).await {
                Ok(account) => (false, Some(account.oid), Some(account.email)),
                Err(error)
                    if matches!(
                        error.status,
                        StatusCode::NOT_FOUND | StatusCode::UNAUTHORIZED
                    ) =>
                {
                    // On retient tout de meme quelle boite etait visee, si elle
                    // existe mais doit etre reliee, pour l'afficher sur la carte.
                    let oid = self.resolve_oid(owner_id, account_hint).ok();
                    let email = oid.as_deref().and_then(|oid| self.email_for_oid(oid));
                    (true, oid, email)
                }
                // Panne reseau, refus de Graph, ou indice invalide : la liaison
                // est probablement intacte, mieux vaut echouer que promettre un
                // envoi impossible.
                Err(error) => return Err(error.message),
            };

        let draft = match draft {
            MicrosoftDraft::UpdateEvent(mut update) => {
                if let Some(oid) = account_oid.as_deref().filter(|_| !requires_link) {
                    update.current_subject = self.event_subject(oid, &update.event_id).await;
                }
                MicrosoftDraft::UpdateEvent(update)
            }
            other => other,
        };

        // L'expediteur vise est mort mais une autre boite saine existe : le
        // modele doit proposer de relier ou de changer d'expediteur, pas de
        // connecter un compte inexistant.
        let has_other_usable_account =
            requires_link && account_oid.is_some() && !self.owner_has_no_usable_account(owner_id);

        let now = metrics::now_ts();
        let action = PendingMicrosoftAction {
            id: Uuid::new_v4().to_string(),
            owner_id: owner_id.to_string(),
            kind: draft.kind().to_string(),
            summary: draft.summary(),
            draft,
            source_chat_key,
            created_at: now,
            expires_at: now + ACTION_TTL_SECONDS,
            requires_link,
            account_oid,
            account_email,
            has_other_usable_account,
        };
        let mut state = self.lock().map_err(|error| error.message)?;
        state.actions.retain(|entry| entry.expires_at > now);
        let owned = state
            .actions
            .iter()
            .filter(|entry| entry.owner_id == owner_id)
            .count();
        if owned >= MAX_PENDING_ACTIONS_PER_OWNER {
            return Err(
                "Trop d'actions Microsoft attendent deja votre confirmation. Traitez-les avant d'en preparer une nouvelle."
                    .to_string(),
            );
        }
        state.actions.push(action.clone());
        Ok(action)
    }

    /// Actions du proprietaire uniquement, et sans consommation : une carte de
    /// confirmation doit survivre a un rechargement de page.
    fn pending_actions(
        &self,
        owner_id: &str,
    ) -> Result<Vec<PendingMicrosoftAction>, MicrosoftError> {
        let now = metrics::now_ts();
        let mut state = self.lock()?;
        state.actions.retain(|entry| entry.expires_at > now);
        Ok(state
            .actions
            .iter()
            .filter(|entry| entry.owner_id == owner_id)
            .cloned()
            .collect())
    }

    fn take_action(
        &self,
        owner_id: &str,
        action_id: &str,
    ) -> Result<PendingMicrosoftAction, MicrosoftError> {
        let now = metrics::now_ts();
        let mut state = self.lock()?;
        state.actions.retain(|entry| entry.expires_at > now);
        let index = state
            .actions
            .iter()
            .position(|entry| entry.id == action_id)
            .ok_or_else(|| {
                MicrosoftError::not_found("Cette action a expire ou a deja ete traitee")
            })?;
        if state.actions[index].owner_id != owner_id {
            // Meme reponse que pour une action absente : l'appelant n'apprend
            // pas l'existence d'un brouillon appartenant a quelqu'un d'autre.
            return Err(MicrosoftError::not_found(
                "Cette action a expire ou a deja ete traitee",
            ));
        }
        Ok(state.actions.remove(index))
    }

    /// Change la boite d'ou partira une action en attente. Le garde-fou central
    /// du multi-comptes : meme si le modele a choisi la mauvaise boite, l'humain
    /// corrige l'expediteur sur la carte avant de valider un envoi irreversible.
    fn set_action_account(
        &self,
        owner_id: &str,
        action_id: &str,
        oid: &str,
    ) -> Result<PendingMicrosoftAction, MicrosoftError> {
        let now = metrics::now_ts();
        let mut state = self.lock()?;
        state.actions.retain(|entry| entry.expires_at > now);
        // La boite visee doit appartenir a ce proprietaire. On lit aussi, tant
        // que le store est emprunte, s'il reste une autre boite saine.
        let (email, needs_relink, other_usable) = {
            let owned = state
                .store
                .links
                .iter()
                .filter(|link| link.owner_id == owner_id)
                .collect::<Vec<_>>();
            let chosen = owned
                .iter()
                .find(|link| link.oid == oid)
                .ok_or_else(|| MicrosoftError::not_found("Cette boite n'est pas liee"))?;
            let other_usable = owned
                .iter()
                .any(|link| link.oid != oid && !link.needs_relink);
            (chosen.email.clone(), chosen.needs_relink, other_usable)
        };
        let action = state
            .actions
            .iter_mut()
            .find(|entry| entry.id == action_id && entry.owner_id == owner_id)
            .ok_or_else(|| {
                MicrosoftError::not_found("Cette action a expire ou a deja ete traitee")
            })?;
        action.account_oid = Some(oid.to_string());
        action.account_email = Some(email);
        // Un envoi ne reste bloque que si la boite choisie doit etre reliee.
        action.requires_link = needs_relink;
        action.has_other_usable_account = needs_relink && other_usable;
        Ok(action.clone())
    }

    /// Remet une action dans la file. Utilise quand une confirmation echoue sur
    /// une boite a relier : l'echec survient AVANT tout envoi Graph, donc rien
    /// n'a ete envoye et remettre le brouillon ne risque aucun doublon.
    fn restore_action(&self, mut action: PendingMicrosoftAction) {
        let now = metrics::now_ts();
        if action.expires_at <= now {
            return;
        }
        action.requires_link = true;
        if let Ok(mut state) = self.inner.lock() {
            if !state.actions.iter().any(|entry| entry.id == action.id) {
                state.actions.push(action);
            }
        }
    }

    async fn execute(&self, action: &PendingMicrosoftAction) -> Result<String, MicrosoftError> {
        // Envoie depuis la boite fixee a la mise en file (ou choisie ensuite sur
        // la carte). Si cette boite a disparu entre-temps, on retombe sur la
        // boite par defaut plutot que d'echouer sans recours.
        let token = match action
            .account_oid
            .as_deref()
            .filter(|oid| self.email_for_oid(oid).is_some())
        {
            Some(oid) => self.resolve_access_token(oid).await?,
            None => self.resolve_account(&action.owner_id, None).await?.token,
        };
        match &action.draft {
            MicrosoftDraft::SendEmail(draft) => {
                let payload = json!({
                    "message": {
                        "subject": draft.subject,
                        "body": { "contentType": "Text", "content": draft.body },
                        "toRecipients": recipients(&draft.to),
                        "ccRecipients": recipients(&draft.cc),
                    },
                    "saveToSentItems": true
                });
                self.graph_post(
                    &token,
                    &format!("{}/me/sendMail", self.config.graph_base),
                    payload,
                )
                .await?;
                Ok(format!(
                    "E-mail « {} » envoye a {}.",
                    draft.subject,
                    draft.to.join(", ")
                ))
            }
            MicrosoftDraft::CreateEvent(draft) => {
                let mut payload = json!({
                    "subject": draft.subject,
                    "start": { "dateTime": draft.start, "timeZone": "UTC" },
                    "end": { "dateTime": draft.end, "timeZone": "UTC" },
                    "attendees": attendees(&draft.attendees),
                    "isOnlineMeeting": draft.online_meeting,
                });
                if let Some(location) = &draft.location {
                    payload["location"] = json!({ "displayName": location });
                }
                if let Some(body) = &draft.body {
                    payload["body"] = json!({ "contentType": "Text", "content": body });
                }
                self.graph_post(
                    &token,
                    &format!("{}/me/events", self.config.graph_base),
                    payload,
                )
                .await?;
                Ok(format!(
                    "Evenement « {} » cree du {} au {} (UTC).",
                    draft.subject, draft.start, draft.end
                ))
            }
            MicrosoftDraft::UpdateEvent(draft) => {
                let mut payload = json!({});
                if let Some(subject) = &draft.subject {
                    payload["subject"] = json!(subject);
                }
                if let Some(start) = &draft.start {
                    payload["start"] = json!({ "dateTime": start, "timeZone": "UTC" });
                }
                if let Some(end) = &draft.end {
                    payload["end"] = json!({ "dateTime": end, "timeZone": "UTC" });
                }
                if let Some(location) = &draft.location {
                    payload["location"] = json!({ "displayName": location });
                }
                if let Some(body) = &draft.body {
                    payload["body"] = json!({ "contentType": "Text", "content": body });
                }
                let url = format!(
                    "{}/me/events/{}",
                    self.config.graph_base,
                    urlencode(&draft.event_id)
                );
                let response = self
                    .http
                    .patch(&url)
                    .bearer_auth(&token)
                    .json(&payload)
                    .send()
                    .await
                    .map_err(|_| {
                        MicrosoftError::new(
                            StatusCode::BAD_GATEWAY,
                            "Microsoft Graph ne repond pas",
                        )
                    })?;
                let status = response.status();
                if !status.is_success() {
                    let body = response.text().await.unwrap_or_default();
                    return Err(graph_error(status, &body));
                }
                Ok("Evenement mis a jour.".to_string())
            }
        }
    }

    async fn graph_post(
        &self,
        token: &str,
        url: &str,
        payload: Value,
    ) -> Result<(), MicrosoftError> {
        let response = self
            .http
            .post(url)
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await
            .map_err(|_| {
                MicrosoftError::new(StatusCode::BAD_GATEWAY, "Microsoft Graph ne repond pas")
            })?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let body = response.text().await.unwrap_or_default();
        Err(graph_error(status, &body))
    }
}

// ---------------------------------------------------------------------------
// Arguments des outils de chat
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListMessagesArguments {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    /// Adresse de la boite a lire, parmi celles de l'utilisateur. Absente : sa
    /// boite par defaut.
    #[serde(default)]
    pub account: Option<String>,
}

impl ListMessagesArguments {
    fn normalized_limit(&self) -> u32 {
        self.limit.unwrap_or(10).clamp(1, 25)
    }

    fn normalized_query(&self) -> Result<Option<String>, MicrosoftError> {
        let Some(query) = self
            .query
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(None);
        };
        if query.chars().count() > MAX_QUERY_CHARS {
            return Err(MicrosoftError::bad_request(format!(
                "La recherche depasse {MAX_QUERY_CHARS} caracteres"
            )));
        }
        if query.contains('"') || query.chars().any(char::is_control) {
            return Err(MicrosoftError::bad_request(
                "La recherche ne peut pas contenir de guillemet",
            ));
        }
        Ok(Some(query.to_string()))
    }

    fn normalized_folder(&self) -> Result<&'static str, MicrosoftError> {
        match self
            .folder
            .as_deref()
            .map(str::trim)
            .unwrap_or("inbox")
            .to_ascii_lowercase()
            .as_str()
        {
            "" | "inbox" => Ok("inbox"),
            "sentitems" | "sent" => Ok("sentitems"),
            "drafts" => Ok("drafts"),
            "archive" => Ok("archive"),
            other => Err(MicrosoftError::bad_request(format!(
                "Dossier inconnu : {other}. Valeurs acceptees : inbox, sentitems, drafts, archive."
            ))),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListEventsArguments {
    #[serde(default)]
    pub start: Option<String>,
    #[serde(default)]
    pub end: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    /// Adresse de l'agenda a consulter, parmi les boites de l'utilisateur.
    /// Absente : sa boite par defaut.
    #[serde(default)]
    pub account: Option<String>,
}

impl ListEventsArguments {
    fn normalized_limit(&self) -> u32 {
        self.limit.unwrap_or(20).clamp(1, 50)
    }

    fn normalized_window(&self) -> Result<(String, String), MicrosoftError> {
        let now = chrono::Utc::now();
        let start = match self
            .start
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            Some(value) => parse_instant(value, "start")?,
            None => graph_instant(now),
        };
        let end = match self.end.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            Some(value) => parse_instant(value, "end")?,
            None => graph_instant(now + chrono::Duration::days(7)),
        };
        if end <= start {
            return Err(MicrosoftError::bad_request(
                "La fin de la periode doit suivre son debut",
            ));
        }
        Ok((start, end))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SendEmailArguments {
    pub to: Vec<String>,
    #[serde(default)]
    pub cc: Vec<String>,
    pub subject: String,
    pub body: String,
    /// Adresse de la boite expeditrice, parmi celles de l'utilisateur. Absente :
    /// sa boite par defaut.
    #[serde(default)]
    pub account: Option<String>,
}

impl SendEmailArguments {
    pub(crate) fn into_draft(self) -> Result<(MicrosoftDraft, Option<String>), String> {
        let to = normalize_recipients(&self.to, "destinataire")?;
        if to.is_empty() {
            return Err("Au moins un destinataire est requis".to_string());
        }
        let cc = normalize_recipients(&self.cc, "destinataire en copie")?;
        if to.len() + cc.len() > MAX_RECIPIENTS {
            return Err(format!("Au maximum {MAX_RECIPIENTS} destinataires"));
        }
        let subject = bounded_text(&self.subject, MAX_SUBJECT_CHARS, "L'objet")?;
        let body = bounded_text(&self.body, MAX_BODY_CHARS, "Le corps du message")?;
        Ok((
            MicrosoftDraft::SendEmail(EmailDraft {
                to,
                cc,
                subject,
                body,
            }),
            normalize_account(self.account),
        ))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateEventArguments {
    pub subject: String,
    pub start: String,
    pub end: String,
    #[serde(default)]
    pub attendees: Vec<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub online_meeting: Option<bool>,
    /// Agenda ou creer l'evenement, parmi les boites de l'utilisateur. Absent :
    /// sa boite par defaut.
    #[serde(default)]
    pub account: Option<String>,
}

impl CreateEventArguments {
    pub(crate) fn into_draft(self) -> Result<(MicrosoftDraft, Option<String>), String> {
        let subject = bounded_text(&self.subject, MAX_SUBJECT_CHARS, "Le titre")?;
        let start = parse_instant(&self.start, "start").map_err(|error| error.message)?;
        let end = parse_instant(&self.end, "end").map_err(|error| error.message)?;
        check_event_window(&start, &end)?;
        let attendees = normalize_recipients(&self.attendees, "participant")?;
        if attendees.len() > MAX_RECIPIENTS {
            return Err(format!("Au maximum {MAX_RECIPIENTS} participants"));
        }
        Ok((
            MicrosoftDraft::CreateEvent(EventDraft {
                subject,
                start,
                end,
                attendees,
                location: optional_text(self.location.as_deref(), MAX_LOCATION_CHARS, "Le lieu")?,
                body: optional_text(self.body.as_deref(), MAX_BODY_CHARS, "La description")?,
                online_meeting: self.online_meeting.unwrap_or(false),
            }),
            normalize_account(self.account),
        ))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateEventArguments {
    pub event_id: String,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub start: Option<String>,
    #[serde(default)]
    pub end: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    /// Agenda contenant l'evenement, parmi les boites de l'utilisateur. Absent :
    /// sa boite par defaut. L'evenement doit exister dans cette boite.
    #[serde(default)]
    pub account: Option<String>,
}

impl UpdateEventArguments {
    pub(crate) fn into_draft(self) -> Result<(MicrosoftDraft, Option<String>), String> {
        let event_id = self.event_id.trim().to_string();
        if event_id.is_empty() || event_id.chars().count() > 512 {
            return Err("L'identifiant de l'evenement est invalide".to_string());
        }
        let subject = optional_text(self.subject.as_deref(), MAX_SUBJECT_CHARS, "Le titre")?;
        let start = match self
            .start
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            Some(value) => Some(parse_instant(value, "start").map_err(|error| error.message)?),
            None => None,
        };
        let end = match self.end.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            Some(value) => Some(parse_instant(value, "end").map_err(|error| error.message)?),
            None => None,
        };
        if let (Some(start), Some(end)) = (start.as_deref(), end.as_deref()) {
            check_event_window(start, end)?;
        }
        let location = optional_text(self.location.as_deref(), MAX_LOCATION_CHARS, "Le lieu")?;
        let body = optional_text(self.body.as_deref(), MAX_BODY_CHARS, "La description")?;
        if subject.is_none()
            && start.is_none()
            && end.is_none()
            && location.is_none()
            && body.is_none()
        {
            return Err("Aucune modification demandee".to_string());
        }
        // Un seul cote de la plage deplacerait l'evenement sans que l'humain
        // voie la duree resultante : on exige les deux bornes ensemble.
        if start.is_some() != end.is_some() {
            return Err("Un deplacement d'horaire exige start et end ensemble".to_string());
        }
        Ok((
            MicrosoftDraft::UpdateEvent(EventUpdateDraft {
                event_id,
                subject,
                start,
                end,
                location,
                body,
                current_subject: None,
            }),
            normalize_account(self.account),
        ))
    }
}

// ---------------------------------------------------------------------------
// Routes HTTP
// ---------------------------------------------------------------------------

pub(crate) fn router(manager: MicrosoftManager) -> Router {
    Router::new()
        .route("/connection", get(api_connection).delete(api_disconnect))
        // Configuration Entra saisie dans l'application (client id/secret/tenant).
        .route(
            "/provider",
            put(api_set_provider).delete(api_clear_provider),
        )
        // Une boite precise : la retirer, ou en faire la boite par defaut.
        .route(
            "/connection/:oid",
            axum::routing::delete(api_disconnect_account),
        )
        .route("/connection/:oid/default", post(api_set_default))
        .route("/start", get(api_start))
        .route("/callback", get(api_callback))
        .route("/pending-actions", get(api_pending_actions))
        .route("/pending-actions/:id/confirm", post(api_confirm))
        .route("/pending-actions/:id/cancel", post(api_cancel))
        .route("/pending-actions/:id/account", post(api_set_action_account))
        .route(
            "/link-request",
            axum::routing::delete(api_dismiss_link_request),
        )
        .with_state(manager)
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

async fn api_connection(
    State(manager): State<MicrosoftManager>,
    headers: HeaderMap,
) -> Result<Response, MicrosoftError> {
    let identity = manager.identity(&headers)?;
    let view = manager.connection_view(&identity.id)?;
    Ok(no_store(Json(view).into_response()))
}

/// Entrees du formulaire de configuration Entra saisi dans l'application.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderInput {
    client_id: String,
    client_secret: String,
    #[serde(default)]
    tenant: Option<String>,
    #[serde(default)]
    redirect_uri: Option<String>,
    #[serde(default)]
    scopes: Option<String>,
}

async fn api_set_provider(
    State(manager): State<MicrosoftManager>,
    headers: HeaderMap,
    Json(input): Json<ProviderInput>,
) -> Result<Response, MicrosoftError> {
    // Configuration a l'echelle de l'instance : une session nominative suffit
    // (reseau ferme, choix explicite de l'utilisateur), pas le jeton admin.
    let identity = manager.identity(&headers)?;
    require_same_site(&headers)?;
    manager.set_provider(input)?;
    let view = manager.connection_view(&identity.id)?;
    Ok(no_store(Json(view).into_response()))
}

async fn api_clear_provider(
    State(manager): State<MicrosoftManager>,
    headers: HeaderMap,
) -> Result<Response, MicrosoftError> {
    let identity = manager.identity(&headers)?;
    require_same_site(&headers)?;
    manager.clear_provider()?;
    let view = manager.connection_view(&identity.id)?;
    Ok(no_store(Json(view).into_response()))
}

async fn api_disconnect(
    State(manager): State<MicrosoftManager>,
    headers: HeaderMap,
) -> Result<Response, MicrosoftError> {
    let identity = manager.identity(&headers)?;
    require_same_site(&headers)?;
    manager.disconnect(&identity.id)?;
    let view = manager.connection_view(&identity.id)?;
    Ok(no_store(Json(view).into_response()))
}

async fn api_disconnect_account(
    State(manager): State<MicrosoftManager>,
    headers: HeaderMap,
    Path(oid): Path<String>,
) -> Result<Response, MicrosoftError> {
    let identity = manager.identity(&headers)?;
    require_same_site(&headers)?;
    manager.disconnect_account(&identity.id, &oid)?;
    let view = manager.connection_view(&identity.id)?;
    Ok(no_store(Json(view).into_response()))
}

async fn api_set_default(
    State(manager): State<MicrosoftManager>,
    headers: HeaderMap,
    Path(oid): Path<String>,
) -> Result<Response, MicrosoftError> {
    let identity = manager.identity(&headers)?;
    require_same_site(&headers)?;
    manager.set_default_account(&identity.id, &oid)?;
    let view = manager.connection_view(&identity.id)?;
    Ok(no_store(Json(view).into_response()))
}

#[derive(Deserialize)]
struct SetAccountBody {
    oid: String,
}

async fn api_set_action_account(
    State(manager): State<MicrosoftManager>,
    headers: HeaderMap,
    Path(action_id): Path<String>,
    Json(body): Json<SetAccountBody>,
) -> Result<Response, MicrosoftError> {
    let identity = manager.identity(&headers)?;
    require_same_site(&headers)?;
    let action = manager.set_action_account(&identity.id, &action_id, &body.oid)?;
    Ok(no_store(Json(action).into_response()))
}

async fn api_start(
    State(manager): State<MicrosoftManager>,
    headers: HeaderMap,
) -> Result<Response, MicrosoftError> {
    let (location, state) = manager.begin_link(&headers)?;
    let mut response = StatusCode::SEE_OTHER.into_response();
    response.headers_mut().insert(
        LOCATION,
        HeaderValue::from_str(&location)
            .map_err(|_| MicrosoftError::internal("URL Microsoft invalide"))?,
    );
    set_cookie(
        &mut response,
        &cookie(
            LINK_STATE_COOKIE,
            &state,
            LINK_STATE_DURATION_SECS,
            manager.config.secure_cookie,
        ),
    );
    Ok(no_store(response))
}

async fn api_callback(
    State(manager): State<MicrosoftManager>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Response {
    if query.error.is_some() {
        return link_redirect(&manager, Err("cancelled"));
    }
    let (Some(code), Some(state)) = (query.code.as_deref(), query.state.as_deref()) else {
        return link_redirect(&manager, Err("invalid"));
    };
    match manager.finish_link(&headers, code, state).await {
        Ok(()) => link_redirect(&manager, Ok(())),
        // Le message brut d'Entra citerait le tenant et l'identifiant client
        // dans l'URL, donc dans l'historique du navigateur et les journaux de
        // proxy : on ne renvoie qu'un code stable interprete par l'interface.
        Err(error) if error.status == StatusCode::CONFLICT => {
            link_redirect(&manager, Err("conflict"))
        }
        Err(error) if error.status == StatusCode::UNAUTHORIZED => {
            link_redirect(&manager, Err("session"))
        }
        Err(_) => link_redirect(&manager, Err("failed")),
    }
}

async fn api_pending_actions(
    State(manager): State<MicrosoftManager>,
    headers: HeaderMap,
) -> Result<Response, MicrosoftError> {
    let identity = manager.identity(&headers)?;
    let actions = manager.pending_actions(&identity.id)?;
    let link_request = manager.link_request(&identity.id)?;
    Ok(no_store(
        Json(json!({ "actions": actions, "linkRequest": link_request })).into_response(),
    ))
}

async fn api_dismiss_link_request(
    State(manager): State<MicrosoftManager>,
    headers: HeaderMap,
) -> Result<Response, MicrosoftError> {
    let identity = manager.identity(&headers)?;
    require_same_site(&headers)?;
    manager.clear_link_request(&identity.id);
    Ok(no_store(
        Json(json!({ "status": "dismissed" })).into_response(),
    ))
}

async fn api_confirm(
    State(manager): State<MicrosoftManager>,
    headers: HeaderMap,
    Path(action_id): Path<String>,
) -> Result<Response, MicrosoftError> {
    let identity = manager.identity(&headers)?;
    require_same_site(&headers)?;
    let action = manager.take_action(&identity.id, &action_id)?;
    match manager.execute(&action).await {
        Ok(message) => Ok(no_store(
            Json(json!({ "status": "done", "message": message })).into_response(),
        )),
        // Boite a relier : l'echec est survenu AVANT tout envoi Graph, donc rien
        // n'est parti. On remet le brouillon en file pour ne pas perdre le
        // travail — l'utilisateur pourra relier la boite ou changer d'expediteur.
        Err(error) if error.status == StatusCode::UNAUTHORIZED => {
            manager.restore_action(action);
            Err(error)
        }
        // Tout autre echec : un envoi a pu partiellement aboutir. Remettre le
        // brouillon risquerait un doublon ; on prefere une erreur explicite.
        Err(error) => Err(error),
    }
}

async fn api_cancel(
    State(manager): State<MicrosoftManager>,
    headers: HeaderMap,
    Path(action_id): Path<String>,
) -> Result<Response, MicrosoftError> {
    let identity = manager.identity(&headers)?;
    require_same_site(&headers)?;
    manager.take_action(&identity.id, &action_id)?;
    Ok(no_store(
        Json(json!({ "status": "cancelled" })).into_response(),
    ))
}

fn link_redirect(manager: &MicrosoftManager, result: Result<(), &str>) -> Response {
    let location = match result {
        Ok(()) => "/?microsoft=linked".to_string(),
        Err(code) => format!("/?microsoft_error={code}"),
    };
    let mut response = StatusCode::SEE_OTHER.into_response();
    response.headers_mut().insert(
        LOCATION,
        HeaderValue::from_str(&location).unwrap_or_else(|_| HeaderValue::from_static("/")),
    );
    set_cookie(
        &mut response,
        &cookie(LINK_STATE_COOKIE, "", 0, manager.config.secure_cookie),
    );
    no_store(response)
}

/// Les mutations exigent un en-tete applicatif. Un formulaire tiers ne peut pas
/// le poser sans prevol CORS credite, que ce serveur n'accorde pas : une page
/// hostile ne peut donc pas declencher un envoi avec le cookie de session.
fn require_same_site(headers: &HeaderMap) -> Result<(), MicrosoftError> {
    let confirmed = headers
        .get("x-cst-confirm")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .is_some_and(|value| value == "1");
    if confirmed {
        Ok(())
    } else {
        Err(MicrosoftError::forbidden("En-tete de confirmation absent"))
    }
}

// ---------------------------------------------------------------------------
// Reponses Microsoft
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: i64,
    #[serde(default)]
    scope: Option<String>,
}

struct GraphProfile {
    oid: String,
    tenant_id: String,
    email: String,
    display_name: Option<String>,
}

fn graph_error(status: StatusCode, body: &str) -> MicrosoftError {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    let message = match status {
        StatusCode::UNAUTHORIZED => {
            "L'autorisation Microsoft a expire. Reliez le compte depuis les parametres.".to_string()
        }
        StatusCode::FORBIDDEN => format!(
            "Microsoft a refuse l'acces demande. Verifiez les permissions accordees. {detail}"
        ),
        StatusCode::NOT_FOUND => "L'element demande est introuvable dans Microsoft 365".to_string(),
        StatusCode::TOO_MANY_REQUESTS => {
            "Microsoft limite temporairement les appels. Reessayez dans un instant.".to_string()
        }
        _ => format!("Microsoft Graph a refuse la demande ({status}). {detail}"),
    };
    MicrosoftError::new(StatusCode::BAD_GATEWAY, message.trim().to_string())
}

// ---------------------------------------------------------------------------
// Aides
// ---------------------------------------------------------------------------

fn build_provider_config(public_base_url: &str) -> Result<Option<ProviderConfig>, String> {
    let client_id = env_trimmed("CST_MICROSOFT_CLIENT_ID");
    let client_secret = env_trimmed("CST_MICROSOFT_CLIENT_SECRET");
    let (client_id, client_secret) = match (client_id, client_secret) {
        (Some(client_id), Some(client_secret)) => (client_id, client_secret),
        (None, None) => return Ok(None),
        _ => return Err(
            "CST_MICROSOFT_CLIENT_ID et CST_MICROSOFT_CLIENT_SECRET doivent etre definis ensemble"
                .to_string(),
        ),
    };
    let tenant = env_trimmed("CST_MICROSOFT_TENANT_ID").unwrap_or_else(|| "common".to_string());
    if !tenant
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '.'))
    {
        return Err("CST_MICROSOFT_TENANT_ID invalide".to_string());
    }
    let redirect_uri = env_trimmed("CST_MICROSOFT_REDIRECT_URI").unwrap_or_else(|| {
        format!(
            "{}/api/microsoft/callback",
            public_base_url.trim_end_matches('/')
        )
    });
    let login_url = microsoft_login_url(&redirect_uri)?;
    Ok(Some(ProviderConfig {
        client_id,
        client_secret,
        tenant,
        redirect_uri,
        login_url,
        scopes: env_trimmed("CST_MICROSOFT_SCOPES").unwrap_or_else(|| DEFAULT_SCOPES.to_string()),
        source: "env".to_string(),
    }))
}

/// Relit la configuration Entra persistee par l'application. `Ok(None)` quand
/// le fichier n'existe pas encore ; on retombe alors sur les variables
/// d'environnement.
fn load_persisted_provider(path: &std::path::Path) -> Result<Option<ProviderConfig>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path)
        .map_err(|error| format!("lecture de {} impossible: {error}", path.display()))?;
    let store: ProviderStore = serde_json::from_str(&content)
        .map_err(|error| format!("{} invalide: {error}", path.display()))?;
    // On revalide l'URI de redirection : un fichier copie d'un autre noeud
    // pourrait pointer une origine devenue invalide.
    if microsoft_login_url(&store.provider.redirect_uri).is_err() {
        return Err(format!(
            "{} contient une URI de redirection invalide",
            path.display()
        ));
    }
    Ok(Some(store.provider))
}

/// Ramene le point de depart de la liaison sur l'origine du callback : un
/// client ouvert sur `127.0.0.1` via un tunnel bascule ainsi sur l'origine
/// publique avant de contacter Microsoft.
/// Deux URLs partagent-elles schema, hote et port ? Sert a interdire un
/// redirect_uri hors de l'origine du noeud. En cas d'URL illisible, `false` :
/// on refuse par defaut plutot que d'ouvrir une origine douteuse.
fn same_origin(candidate: &str, base: &str) -> bool {
    let (Ok(candidate), Ok(base)) = (Url::parse(candidate), Url::parse(base)) else {
        return false;
    };
    candidate.scheme() == base.scheme()
        && candidate.host_str() == base.host_str()
        && candidate.port_or_known_default() == base.port_or_known_default()
}

fn microsoft_login_url(redirect_uri: &str) -> Result<String, String> {
    let mut url = Url::parse(redirect_uri)
        .map_err(|error| format!("CST_MICROSOFT_REDIRECT_URI invalide: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("CST_MICROSOFT_REDIRECT_URI doit etre une URL HTTP(S) absolue".to_string());
    }
    // Entra ID refuse une URI de redirection en clair hors boucle locale : le
    // signaler ici evite un echec incomprehensible au moment du consentement.
    if url.scheme() == "http"
        && !matches!(
            url.host_str(),
            Some("localhost") | Some("127.0.0.1") | Some("[::1]") | Some("::1")
        )
    {
        return Err(
            "CST_MICROSOFT_REDIRECT_URI doit etre en HTTPS en dehors de la boucle locale"
                .to_string(),
        );
    }
    url.set_path("/api/microsoft/start");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.into())
}

/// Nettoie l'indice de compte fourni par un outil : une chaine vide vaut
/// « boite par defaut ».
fn normalize_account(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Garantit exactement une boite par defaut et par proprietaire. Appele au
/// chargement (fichiers d'avant le multi-comptes, ou fichier trafique) et apres
/// chaque suppression (l'ancien defaut a pu partir).
fn normalize_defaults(links: &mut [StoredLink]) {
    let mut owners = links
        .iter()
        .map(|link| link.owner_id.clone())
        .collect::<Vec<_>>();
    owners.sort();
    owners.dedup();
    for owner in owners {
        let mut indexes = links
            .iter()
            .enumerate()
            .filter(|(_, link)| link.owner_id == owner)
            .map(|(index, link)| (index, link.linked_at))
            .collect::<Vec<_>>();
        if indexes.is_empty() {
            continue;
        }
        let already_default = indexes
            .iter()
            .find(|(index, _)| links[*index].is_default)
            .map(|(index, _)| *index);
        // Un defaut existe : on garde le premier et on eteint les eventuels
        // doublons. Sinon, la boite la plus ancienne devient le defaut.
        let keep = already_default.unwrap_or_else(|| {
            indexes.sort_by_key(|(_, linked_at)| *linked_at);
            indexes[0].0
        });
        for (index, _) in &indexes {
            links[*index].is_default = *index == keep;
        }
    }
}

fn relink_required() -> MicrosoftError {
    MicrosoftError::unauthorized(
        "L'autorisation Microsoft a ete revoquee ou a expire. Reliez le compte depuis les parametres.",
    )
}

/// Echeance retenue pour un jeton d'acces : la duree annoncee, moins la marge de
/// renouvellement, avec un plancher qui protege d'un `expires_in` absent.
fn access_token_expiry(now: i64, expires_in: i64) -> i64 {
    now + expires_in.max(MIN_ACCESS_TOKEN_LIFETIME_SECS) - REFRESH_MARGIN_SECS
}

fn needs_keepalive(updated_at: i64, needs_relink: bool, now: i64) -> bool {
    !needs_relink && now.saturating_sub(updated_at) >= KEEPALIVE_MAX_AGE_SECS
}

fn recipients(addresses: &[String]) -> Vec<Value> {
    addresses
        .iter()
        .map(|address| json!({ "emailAddress": { "address": address } }))
        .collect()
}

fn attendees(addresses: &[String]) -> Vec<Value> {
    addresses
        .iter()
        .map(|address| {
            json!({
                "emailAddress": { "address": address },
                "type": "required"
            })
        })
        .collect()
}

fn normalize_recipients(values: &[String], label: &str) -> Result<Vec<String>, String> {
    let mut normalized = Vec::with_capacity(values.len());
    for value in values {
        let address = value.trim().to_string();
        if address.is_empty() {
            continue;
        }
        if address.len() > 254
            || address.chars().any(|c| c.is_whitespace() || c.is_control())
            || address.matches('@').count() != 1
            || address.starts_with('@')
            || address.ends_with('@')
            || !address.rsplit('@').next().is_some_and(|d| d.contains('.'))
        {
            return Err(format!("Adresse de {label} invalide : {address}"));
        }
        if !normalized
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(&address))
        {
            normalized.push(address);
        }
    }
    Ok(normalized)
}

fn bounded_text(value: &str, max: usize, label: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} est requis"));
    }
    if trimmed.chars().count() > max {
        return Err(format!("{label} depasse {max} caracteres"));
    }
    Ok(trimmed.to_string())
}

fn optional_text(value: Option<&str>, max: usize, label: &str) -> Result<Option<String>, String> {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => Ok(Some(bounded_text(value, max, label)?)),
        None => Ok(None),
    }
}

/// Normalise un instant en UTC au format attendu par Graph. Le decalage horaire
/// est obligatoire : sans lui, « 14h » serait interprete a l'aveugle et le
/// rendez-vous atterrirait a la mauvaise heure.
fn parse_instant(value: &str, field: &str) -> Result<String, MicrosoftError> {
    let trimmed = value.trim();
    let parsed = chrono::DateTime::parse_from_rfc3339(trimmed).map_err(|_| {
        MicrosoftError::bad_request(format!(
            "{field} doit etre une date ISO 8601 avec decalage horaire, par exemple 2026-07-25T14:00:00+02:00"
        ))
    })?;
    Ok(graph_instant(parsed.with_timezone(&chrono::Utc)))
}

fn graph_instant(value: chrono::DateTime<chrono::Utc>) -> String {
    value.format("%Y-%m-%dT%H:%M:%S").to_string()
}

fn check_event_window(start: &str, end: &str) -> Result<(), String> {
    if end <= start {
        return Err("La fin de l'evenement doit suivre son debut".to_string());
    }
    let parse = |value: &str| {
        chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S")
            .map_err(|_| "Date d'evenement invalide".to_string())
    };
    let duration = parse(end)? - parse(start)?;
    if duration.num_days() > MAX_EVENT_DAYS {
        return Err(format!(
            "Un evenement ne peut pas depasser {MAX_EVENT_DAYS} jours"
        ));
    }
    Ok(())
}

fn urlencode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
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

fn env_trimmed(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn cookie(name: &str, value: &str, max_age: i64, secure: bool) -> String {
    format!(
        "{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{}",
        if secure { "; Secure" } else { "" }
    )
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all("cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|part| {
            let (key, value) = part.trim().split_once('=')?;
            (key.trim() == name && !value.is_empty()).then(|| value.to_string())
        })
}

fn set_cookie(response: &mut Response, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        response.headers_mut().append(SET_COOKIE, value);
    }
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(unix)]
fn restrict_permissions(path: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &std::path::Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_starts_on_the_callback_origin() {
        assert_eq!(
            microsoft_login_url("https://cst.example.test/api/microsoft/callback?ignored=1")
                .unwrap(),
            "https://cst.example.test/api/microsoft/start"
        );
        assert_eq!(
            microsoft_login_url("http://localhost:8080/api/microsoft/callback").unwrap(),
            "http://localhost:8080/api/microsoft/start"
        );
        assert!(microsoft_login_url("javascript:alert(1)").is_err());
        // Entra refuse le HTTP en clair hors boucle locale : echouer ici plutot
        // qu'au moment du consentement.
        assert!(microsoft_login_url("http://cst.example.test/api/microsoft/callback").is_err());
    }

    #[test]
    fn recipients_are_validated_and_deduplicated() {
        let normalized =
            normalize_recipients(&["  a@b.fr ".into(), "A@B.FR".into(), "".into()], "test")
                .unwrap();
        assert_eq!(normalized, vec!["a@b.fr".to_string()]);
        assert!(normalize_recipients(&["pas-une-adresse".into()], "test").is_err());
        assert!(normalize_recipients(&["a@b".into()], "test").is_err());
        assert!(normalize_recipients(&["a b@c.fr".into()], "test").is_err());
    }

    #[test]
    fn instants_require_an_explicit_offset() {
        assert_eq!(
            parse_instant("2026-07-25T14:00:00+02:00", "start").unwrap(),
            "2026-07-25T12:00:00"
        );
        assert_eq!(
            parse_instant("2026-07-25T12:00:00Z", "start").unwrap(),
            "2026-07-25T12:00:00"
        );
        assert!(parse_instant("2026-07-25T14:00:00", "start").is_err());
        assert!(parse_instant("demain 14h", "start").is_err());
    }

    #[test]
    fn email_arguments_are_bounded() {
        let (draft, account) = SendEmailArguments {
            to: vec!["jean@example.fr".into()],
            cc: vec![],
            subject: " Point projet ".into(),
            body: "Bonjour".into(),
            account: Some("  ".into()),
        }
        .into_draft()
        .unwrap();
        // Un indice de compte vide vaut « boite par defaut ».
        assert_eq!(account, None);
        match draft {
            MicrosoftDraft::SendEmail(email) => {
                assert_eq!(email.subject, "Point projet");
                assert_eq!(email.to, vec!["jean@example.fr".to_string()]);
            }
            _ => panic!("brouillon inattendu"),
        }

        // Un indice de compte non vide est conserve tel quel (la validation
        // contre les boites de l'utilisateur a lieu au moment de la resolution).
        let (_, account) = SendEmailArguments {
            to: vec!["jean@example.fr".into()],
            cc: vec![],
            subject: "Objet".into(),
            body: "Corps".into(),
            account: Some(" pro@example.fr ".into()),
        }
        .into_draft()
        .unwrap();
        assert_eq!(account.as_deref(), Some("pro@example.fr"));

        assert!(SendEmailArguments {
            to: vec![],
            cc: vec![],
            subject: "x".into(),
            body: "y".into(),
            account: None,
        }
        .into_draft()
        .is_err());

        assert!(SendEmailArguments {
            to: vec!["jean@example.fr".into()],
            cc: vec![],
            subject: "x".repeat(MAX_SUBJECT_CHARS + 1),
            body: "y".into(),
            account: None,
        }
        .into_draft()
        .is_err());
    }

    #[test]
    fn event_updates_require_a_complete_time_range() {
        assert!(UpdateEventArguments {
            event_id: "AAA".into(),
            subject: None,
            start: Some("2026-07-25T10:00:00Z".into()),
            end: None,
            location: None,
            body: None,
            account: None,
        }
        .into_draft()
        .is_err());

        assert!(UpdateEventArguments {
            event_id: "AAA".into(),
            subject: None,
            start: None,
            end: None,
            location: None,
            body: None,
            account: None,
        }
        .into_draft()
        .is_err());

        assert!(UpdateEventArguments {
            event_id: "AAA".into(),
            subject: Some("Nouveau titre".into()),
            start: None,
            end: None,
            location: None,
            body: None,
            account: None,
        }
        .into_draft()
        .is_ok());
    }

    #[test]
    fn message_folders_are_restricted_to_known_names() {
        let arguments = ListMessagesArguments {
            folder: Some("Sent".into()),
            ..Default::default()
        };
        assert_eq!(arguments.normalized_folder().unwrap(), "sentitems");
        let arguments = ListMessagesArguments {
            folder: Some("../../users".into()),
            ..Default::default()
        };
        assert!(arguments.normalized_folder().is_err());
    }

    #[test]
    fn search_terms_cannot_break_out_of_the_graph_query() {
        let arguments = ListMessagesArguments {
            query: Some("facture\" OR from:admin".into()),
            ..Default::default()
        };
        assert!(arguments.normalized_query().is_err());
    }

    #[test]
    fn a_missing_expires_in_does_not_expire_the_token_on_arrival() {
        // Sans plancher, `expires_in` absent (donc 0) rendrait le jeton perime
        // des sa reception : chaque appel provoquerait une rotation du jeton de
        // renouvellement, et une seule rotation perdue tue la liaison.
        assert!(access_token_expiry(1_000, 0) > 1_000);
        assert_eq!(
            access_token_expiry(1_000, 3600),
            1_000 + 3600 - REFRESH_MARGIN_SECS
        );
    }

    #[test]
    fn keepalive_targets_only_links_that_drift_toward_the_inactivity_window() {
        let now = 100 * 24 * 60 * 60;
        let fresh = now - KEEPALIVE_MAX_AGE_SECS + 1;
        let stale = now - KEEPALIVE_MAX_AGE_SECS;
        assert!(!needs_keepalive(fresh, false, now));
        assert!(needs_keepalive(stale, false, now));
        // Une liaison morte ne doit pas etre reveillee en boucle : seule une
        // nouvelle autorisation de l'utilisateur peut la ressusciter.
        assert!(!needs_keepalive(stale, true, now));
        // Le seuil doit rester tres en deca des 90 jours d'Entra, pour laisser
        // le temps a plusieurs cycles de rattraper une panne reseau.
        assert!(KEEPALIVE_MAX_AGE_SECS * 3 < 90 * 24 * 60 * 60);
    }

    #[test]
    fn a_revoked_authorization_keeps_the_identity_but_no_token() {
        let mut link = StoredLink {
            owner_id: "user-1".into(),
            oid: "oid-1".into(),
            tenant_id: String::new(),
            email: "jean@example.fr".into(),
            display_name: Some("Jean".into()),
            access_token: "secret-acces".into(),
            refresh_token: "secret-renouvellement".into(),
            expires_at: 42,
            scopes: vec!["Mail.Send".into()],
            linked_at: 1,
            updated_at: 1,
            needs_relink: false,
            is_default: true,
        };
        // Meme transformation que `mark_needs_relink`, verifiee ici sans reseau.
        link.access_token.clear();
        link.refresh_token.clear();
        link.expires_at = 0;
        link.needs_relink = true;

        assert_eq!(link.email, "jean@example.fr");
        assert!(link.access_token.is_empty());
        assert!(link.refresh_token.is_empty());
        assert_eq!(relink_required().status, StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn a_draft_prepared_without_a_linked_account_is_kept_and_flagged() {
        let action = PendingMicrosoftAction {
            id: "action-1".into(),
            owner_id: "user-1".into(),
            kind: "sendEmail".into(),
            summary: "E-mail « Point » a jean@example.fr".into(),
            draft: SendEmailArguments {
                to: vec!["jean@example.fr".into()],
                cc: vec![],
                subject: "Point".into(),
                body: "Bonjour".into(),
                account: None,
            }
            .into_draft()
            .unwrap()
            .0,
            source_chat_key: Some("chat-1".into()),
            created_at: 1,
            expires_at: 2,
            requires_link: true,
            account_oid: None,
            account_email: None,
            has_other_usable_account: false,
        };
        let serialized = serde_json::to_value(&action).unwrap();
        // L'interface s'appuie sur ce drapeau pour desactiver l'envoi et
        // proposer la liaison plutot que de laisser cliquer dans le vide.
        assert_eq!(serialized["requiresLink"], true);
        // Le corps redige doit survivre : c'est tout l'interet de conserver le
        // brouillon au lieu de refuser l'appel de l'outil.
        assert_eq!(serialized["draft"]["body"], "Bonjour");
        assert_eq!(serialized["draft"]["kind"], "sendEmail");
        // Le proprietaire et l'oid ne sortent jamais : la liste est deja filtree
        // par session, les exposer ne servirait qu'a fuiter des identifiants.
        assert!(serialized.get("ownerId").is_none());
        assert!(serialized.get("accountOid").is_none());
    }

    #[test]
    fn a_link_request_says_whether_anything_can_be_linked_at_all() {
        let request = MicrosoftLinkRequest {
            requested_at: 10,
            expires_at: 10 + LINK_REQUEST_TTL_SECONDS,
            configured: false,
            needs_relink: false,
            login_url: None,
        };
        let serialized = serde_json::to_value(&request).unwrap();
        // Sans configuration serveur, la carte doit expliquer la situation au
        // lieu d'afficher un bouton qui ne menerait nulle part.
        assert_eq!(serialized["configured"], false);
        assert!(serialized["loginUrl"].is_null());
        assert!(LINK_REQUEST_TTL_SECONDS < ACTION_TTL_SECONDS);
    }

    #[test]
    fn the_public_view_never_carries_a_token() {
        let view = MicrosoftConnectionView {
            configured: true,
            connected: true,
            needs_relink: false,
            accounts: vec![MicrosoftAccountView {
                oid: "oid-1".into(),
                email: "jean@example.fr".into(),
                display_name: None,
                is_default: true,
                needs_relink: false,
                scopes: vec!["Mail.Send".into()],
                linked_at: 1,
            }],
            email: Some("jean@example.fr".into()),
            display_name: None,
            scopes: vec!["Mail.Send".into()],
            linked_at: Some(1),
            tenant: Some("common".into()),
            redirect_uri: Some("https://cst.example.test/api/microsoft/callback".into()),
            login_url: Some("https://cst.example.test/api/microsoft/start".into()),
            client_id: Some("00000000-0000-0000-0000-000000000000".into()),
            default_redirect_uri: "https://cst.example.test/api/microsoft/callback".into(),
            provider_source: Some("app".into()),
        };
        let serialized = serde_json::to_string(&view).unwrap();
        assert!(!serialized.contains("accessToken"));
        assert!(!serialized.contains("refreshToken"));
        // La vue expose l'identifiant client (public) mais JAMAIS le secret.
        assert!(!serialized.to_lowercase().contains("secret"));
        assert!(!serialized.contains("clientSecret"));
    }

    #[test]
    fn a_provider_config_persists_its_secret_but_the_view_never_does() {
        // Le secret client est persiste sur disque (0600) pour survivre a un
        // redeploiement, mais ne doit jamais transiter par une reponse /api.
        let provider = ProviderConfig {
            client_id: "app-1".into(),
            client_secret: "secret-entra".into(),
            tenant: "common".into(),
            redirect_uri: "https://cst.example.test/api/microsoft/callback".into(),
            login_url: "https://cst.example.test/api/microsoft/start".into(),
            scopes: DEFAULT_SCOPES.into(),
            source: "app".into(),
        };
        let store = ProviderStore {
            version: STORE_VERSION,
            provider: provider.clone(),
        };
        let persisted = serde_json::to_string(&store).unwrap();
        assert!(persisted.contains("secret-entra"));
        // Round-trip : ce qu'on ecrit se relit a l'identique.
        let back: ProviderStore = serde_json::from_str(&persisted).unwrap();
        assert_eq!(back.provider.client_id, "app-1");
        assert_eq!(back.provider.source, "app");
    }

    #[test]
    fn defaults_are_normalized_to_exactly_one_per_owner() {
        let mut links = vec![
            sample_link("user-1", "a", 10, false),
            sample_link("user-1", "b", 5, false),
            sample_link("user-2", "c", 1, true),
            sample_link("user-2", "d", 2, true),
        ];
        normalize_defaults(&mut links);
        // user-1 n'avait aucun defaut : la boite la plus ancienne (b, linked_at 5)
        // le devient.
        let user1 = links
            .iter()
            .filter(|l| l.owner_id == "user-1")
            .filter(|l| l.is_default)
            .collect::<Vec<_>>();
        assert_eq!(user1.len(), 1);
        assert_eq!(user1[0].oid, "b");
        // user-2 en avait deux : un seul survit.
        assert_eq!(
            links
                .iter()
                .filter(|l| l.owner_id == "user-2" && l.is_default)
                .count(),
            1
        );
    }

    fn sample_link(owner: &str, oid: &str, linked_at: i64, is_default: bool) -> StoredLink {
        StoredLink {
            owner_id: owner.into(),
            oid: oid.into(),
            tenant_id: String::new(),
            email: format!("{oid}@example.fr"),
            display_name: None,
            access_token: "t".into(),
            refresh_token: "r".into(),
            expires_at: i64::MAX,
            scopes: vec![],
            linked_at,
            updated_at: linked_at,
            needs_relink: false,
            is_default,
        }
    }
}
