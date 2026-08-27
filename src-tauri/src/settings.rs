use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{
    collections::HashSet,
    env, fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use url::Url;

/// Fournisseur CLI gere par un compte / un agent.
///
/// `Codex` (ChatGPT/OpenAI) est le defaut historique : les comptes et agents
/// existants (champ `provider` absent du settings.json) ainsi que tout code qui
/// ne precise pas de provider restent Codex, ce qui preserve le comportement
/// actuel bit pour bit. `Claude` designe Claude Code (CLI `claude`) et
/// `OpenCode` sert de runtime commun aux fournisseurs API annexes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    #[default]
    Codex,
    Claude,
    OpenCode,
    Freebuff,
    /// Compte AIHubMix utilisant son endpoint OpenAI-compatible.
    Aihubmix,
}

impl Provider {
    /// Identifiant stable (utilise pour les logs et l'UI).
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Codex => "codex",
            Provider::Claude => "claude",
            Provider::OpenCode => "opencode",
            Provider::Freebuff => "freebuff",
            Provider::Aihubmix => "aihubmix",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountProfile {
    pub id: String,
    pub label: String,
    /// Date d'ajout du profil (secondes Unix). Les profils historiques sans
    /// cette date sont conserves : seuls les nouveaux comptes explicitement
    /// crees par l'interface sont soumis au delai de premiere connexion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    /// Fournisseur CLI de ce compte. `#[serde(default)]` => les comptes crees
    /// par une version anterieure (sans ce champ) sont interpretes comme Codex.
    #[serde(default)]
    pub provider: Provider,
    /// Fournisseur de modele selectionne dans OpenCode (`deepseek`, `minimax`,
    /// `zai`, `zai-coding-plan`, `openrouter`, ...). Ce champ reste vide pour
    /// les runtimes natifs Codex et Claude Code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inference_provider: Option<String>,
    /// Clé AIHubMix stockée côté runtime, jamais exposée au frontend.
    #[serde(default, skip_serializing)]
    pub api_key: Option<String>,
    /// Dossier "home" isole du compte. Pour Codex c'est `CODEX_HOME` ; pour
    /// Claude c'est `CLAUDE_CONFIG_DIR` (meme role : sessions + credentials +
    /// config propres au compte). Le nom de champ reste `codexHome` cote JSON
    /// pour la retro-compat des settings.json existants.
    pub codex_home: String,
    #[serde(default)]
    pub project_dir: Option<String>,
    pub proxy_id: Option<String>,
    pub startup_command: Option<String>,
    #[serde(default)]
    pub limits: AccountLimitTracking,
    /// Lance Codex en mode bypass (`--dangerously-bypass-approvals-and-sandbox`)
    /// pour CE compte. Actif par defaut ; les comptes existants (champ absent du
    /// settings.json) sont migres a `true` au chargement via ce default serde.
    #[serde(default = "default_true")]
    pub bypass: bool,
    /// Modele Codex par defaut de CE compte. `None` preserve le comportement et
    /// le `config.toml` des profils crees par une ancienne version de l'app.
    #[serde(default)]
    pub model: Option<String>,
    /// Intensite de raisonnement Codex (`model_reasoning_effort`) de CE compte.
    /// `None` laisse une eventuelle valeur existante du `config.toml` intacte.
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    /// Active le palier de service rapide propre a CE compte. Absent des
    /// anciennes configurations => mode normal, sans surconsommation.
    #[serde(default)]
    pub fast_mode: bool,
    /// Jour civil local (`YYYY-MM-DD`) ou le compte a ete marque termine.
    /// Le frontend ignore automatiquement une date anterieure au jour courant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_on: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountLimitTracking {
    pub connected_at: Option<i64>,
    pub session_anchor_at: Option<i64>,
    pub weekly_anchor_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountLimitView {
    pub id: String,
    pub label: String,
    pub provider: Provider,
    pub codex_home: String,
    pub has_tokens: bool,
    pub connected_at: Option<i64>,
    pub session_reset_at: Option<i64>,
    pub weekly_reset_at: Option<i64>,
    pub session_remaining_secs: Option<i64>,
    pub weekly_remaining_secs: Option<i64>,
    pub session_used_percent: Option<f64>,
    pub weekly_used_percent: Option<f64>,
    pub buckets: Vec<AccountRateLimitBucketView>,
    pub refreshed_at: Option<i64>,
    pub source: String,
    pub refreshing: bool,
    pub error: Option<String>,
    /// freebuff uniquement : une session occupe deja le home de ce compte.
    /// freebuff refusant toute seconde instance sur un meme home, l'interface
    /// s'appuie dessus pour diriger un nouveau terminal vers un compte libre
    /// plutot que d'y ouvrir une session condamnee a son ecran de blocage.
    pub session_busy: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountRateLimitBucketView {
    pub limit_id: String,
    pub limit_name: Option<String>,
    pub bucket: String,
    pub window_duration_mins: i64,
    pub resets_at: i64,
    pub used_percent: Option<f64>,
    pub rate_limit_reached_type: Option<String>,
    pub plan_type: Option<String>,
}

/// Capacites d'intensite exposees par le catalogue du CLI Codex pour un
/// modele precis. Le frontend ne doit pas inventer une liste globale : `max`
/// et `ultra`, par exemple, ne sont proposes que par certains modeles.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelReasoningEffortView {
    pub reasoning_effort: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AccountModelView {
    pub id: String,
    pub display_name: String,
    pub default_reasoning_effort: Option<String>,
    pub supported_reasoning_efforts: Vec<ModelReasoningEffortView>,
    pub supports_fast_mode: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyProfile {
    pub id: String,
    pub label: String,
    pub proxy_url: String,
    pub note: Option<String>,
}

/// Un agent CLI lancable dans un terminal (Codex, Kombai, ou tout autre outil).
/// `command` est la commande de base envoyee dans le shell ; les sous-commandes
/// optionnelles (`login`, `status`, `doctor`) ne sont proposees dans l'UI que
/// lorsqu'elles sont definies (Codex les fournit, un agent generique non).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProfile {
    pub id: String,
    pub label: String,
    pub command: String,
    /// Fournisseur CLI pilote par cet agent. `#[serde(default)]` => les agents
    /// existants (sans ce champ) sont Codex. L'agent Claude Code integre porte
    /// `Provider::Claude`.
    #[serde(default)]
    pub provider: Provider,
    /// "cli" : la commande est envoyee dans le terminal (Codex).
    /// "ide" : la commande est un lanceur d'editeur (code, cursor, windsurf,
    /// trae, antigravity, kiro) ouvert sur le dossier projet — c'est ainsi que
    /// Kombai (extension d'IDE) est utilise.
    #[serde(default = "default_agent_kind")]
    pub kind: String,
    #[serde(default)]
    pub builtin: bool,
    #[serde(default)]
    pub login_command: Option<String>,
    #[serde(default)]
    pub status_command: Option<String>,
    #[serde(default)]
    pub doctor_command: Option<String>,
}

/// Un **workspace** = un dossier projet ouvert par l'utilisateur, qui sert de
/// contexte a un ensemble de chats (comme OpenCode / Codex / Claude Code). Le
/// registre des workspaces est persiste dans `settings.json` afin de suivre
/// l'utilisateur sur ses appareils (desktop/web/Android), a la difference du
/// pointeur de workspace ACTIF qui reste local a l'appareil (localStorage).
///
/// L'appartenance d'un chat a un workspace n'est PAS stockee ici : elle est
/// derivee cote client en comparant le `cwd` de la discussion au `path` du
/// workspace. On ne conserve donc jamais de reference de chat fragile.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceProfile {
    /// Identite stable = chemin normalise (cf. `normalize_workspace_path`). Le
    /// backend le recalcule toujours afin de fusionner les anciens doublons.
    pub id: String,
    /// Libelle affiche (par defaut le nom du dossier), personnalisable.
    pub label: String,
    /// Chemin du dossier projet (tel que saisi/choisi par l'utilisateur).
    pub path: String,
    /// Contexte durable partage par tous les chats de cet environnement.
    /// Le champ absent des anciens settings est migre vers une memoire vide.
    #[serde(default)]
    pub memory: String,
    /// VPS utilise par defaut pour les nouveaux chats et terminaux de ce projet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_target_id: Option<String>,
}

pub const MAX_WORKSPACE_MEMORY_CHARS: usize = 8_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub accounts: Vec<AccountProfile>,
    pub proxies: Vec<ProxyProfile>,
    pub default_account_id: Option<String>,
    pub shell: String,
    pub codex_command: String,
    pub auto_run_codex: bool,
    #[serde(default = "default_proxy_controls_enabled")]
    pub proxy_controls_enabled: bool,
    #[serde(default)]
    pub pool: PoolConfig,
    #[serde(default)]
    pub agents: Vec<AgentProfile>,
    #[serde(default)]
    pub active_agent_id: Option<String>,
    #[serde(default)]
    pub kombai: KombaiConfig,
    /// Ajoute `--dangerously-bypass-approvals-and-sandbox` quand l'app lance
    /// Codex (bouton Run + auto-run). Actif par defaut.
    #[serde(default = "default_true")]
    pub codex_bypass: bool,
    /// Re-scan automatique de `~/.codex*` a chaque chargement pour ajouter les
    /// comptes trouves. Desactive par defaut : les comptes se gerent via le Pool
    /// (import + suppression).
    #[serde(default)]
    pub auto_discover_accounts: bool,
    /// Homes retires apres expiration de leur premiere connexion. Cette liste
    /// empeche l'auto-decouverte de recreer aussitot leur profil a cause d'un
    /// simple config.toml, sans jamais supprimer le dossier lui-meme.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expired_unconnected_account_homes: Vec<String>,
    /// Registre des workspaces (dossiers projets ouverts). `#[serde(default)]`
    /// => les settings.json anterieurs (sans ce champ) restent lisibles et
    /// demarrent avec une liste vide, ensuite peuplee par migration du
    /// localStorage cote client.
    #[serde(default)]
    pub workspaces: Vec<WorkspaceProfile>,
    /// Identites normalisees des workspaces explicitement fermes. Elles
    /// empechent une ancienne discussion ou le MRU d'un autre appareil de les
    /// recreer automatiquement ; une ouverture explicite retire le tombstone.
    #[serde(default)]
    pub closed_workspace_ids: Vec<String>,
    /// Maintient un process `claude` vivant par conversation (mode entree
    /// stream-json) au lieu d'un process one-shot par tour. Evite l'orphelinage
    /// des taches shell en arriere-plan entre deux tours (voir chat.rs, session
    /// Claude persistante). Desactive par defaut : opt-in tant que le mode n'est
    /// pas eprouve sur tous les parcours.
    #[serde(default)]
    pub claude_persistent_session: bool,
}

/// Reglages du VS Code embarque (code-server) qui heberge l'extension Kombai
/// dans l'onglet "Kombai". Kombai n'ayant pas de CLI/API, c'est la seule facon
/// de l'utiliser dans l'app.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KombaiConfig {
    #[serde(default = "default_code_server_command")]
    pub code_server_command: String,
    #[serde(default = "default_kombai_port")]
    pub port: u16,
    #[serde(default = "default_kombai_extension")]
    pub extension_id: String,
    #[serde(default = "default_auto_install_extension")]
    pub auto_install_extension: bool,
}

impl Default for KombaiConfig {
    fn default() -> Self {
        KombaiConfig {
            code_server_command: default_code_server_command(),
            port: default_kombai_port(),
            extension_id: default_kombai_extension(),
            auto_install_extension: default_auto_install_extension(),
        }
    }
}

fn default_code_server_command() -> String {
    "code-server".to_string()
}
fn default_kombai_port() -> u16 {
    8899
}
fn default_kombai_extension() -> String {
    "kombai.kombai".to_string()
}
fn default_auto_install_extension() -> bool {
    true
}

const CODEX_AGENT_ID: &str = "codex";
const CLAUDE_AGENT_ID: &str = "claude";
const OPENCODE_AGENT_ID: &str = "opencode";
/// Agent CLI freebuff. TUI interactif : il vit dans un onglet terminal et ne
/// peut pas alimenter le flux de chat, son CLI n'acceptant ni prompt en
/// argument ni sortie structuree (seulement `login` et `--continue`).
const FREEBUFF_AGENT_ID: &str = "freebuff";
/// Freebuff Desktop, lance comme app externe (`kind = "ide"`) sur le dossier
/// projet. C'est le seul canal ou le mode agent MAX est joignable : le CLI
/// freebuff est fige sur LITE par un flag compile dans son binaire.
const FREEBUFF_DESKTOP_AGENT_ID: &str = "freebuff-desktop";
/// Emplacement d'installation standard de Freebuff Desktop (electron-builder
/// par utilisateur). La variable est laissee litterale : cmd.exe la resout au
/// lancement, ce qui garde le reglage valable pour n'importe quel profil.
const FREEBUFF_DESKTOP_COMMAND: &str =
    "%LOCALAPPDATA%\\Programs\\@codebufffreebuff-desktop\\Freebuff.exe";
const KOMBAI_AGENT_ID: &str = "kombai";

fn default_agent_kind() -> String {
    "cli".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolConfig {
    #[serde(default = "default_pool_port")]
    pub port: u16,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_pool_model")]
    pub default_model: String,
    #[serde(default = "default_pool_effort")]
    pub reasoning_effort: String,
    #[serde(default = "default_pool_upstream")]
    pub upstream: String,
    #[serde(default = "default_request_timeout")]
    pub request_timeout_secs: u64,
    #[serde(default = "default_cooldown_429")]
    pub cooldown_secs_429: u64,
    #[serde(default = "default_pool_concurrency")]
    pub concurrency: usize,
    #[serde(default)]
    pub client_id_override: String,
}

const SESSION_LIMIT_MINS: i64 = 5 * 60;
const WEEKLY_LIMIT_MINS: i64 = 7 * 24 * 60;
// Un meme quota peut passer de `secondary` a `primary` entre deux versions du
// backend Codex. Son reset, lui, reste stable (a quelques secondes pres) tant
// que l'on parle bien de la meme fenetre et du meme compte.
const RATE_LIMIT_RESET_MATCH_TOLERANCE_SECS: u64 = 60;
const RATE_LIMIT_READ_TIMEOUT_SECS: u64 = 18;
const RATE_LIMIT_CACHE_TTL_SECS: u64 = 60;
const MODEL_CATALOG_TIMEOUT_SECS: u64 = 12;
const OPENROUTER_MODELS_USER_URL: &str = "https://openrouter.ai/api/v1/models/user";
const OPENROUTER_MODEL_CATALOG_TIMEOUT_SECS: u64 = 20;
const OPENROUTER_MODEL_PAGE_LIMIT: usize = 1000;
const OPENROUTER_MODEL_MAX_PAGES: usize = 100;
const OPENROUTER_MODEL_ID_MAX_CHARS: usize = 160;
const OPENROUTER_REASONING_EFFORTS: [&str; 7] =
    ["max", "xhigh", "high", "medium", "low", "minimal", "none"];

/// Endpoint OAuth interroge par la commande `/usage` de Claude Code : c'est la
/// seule source des fenetres de consommation d'un abonnement claude.ai.
const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const CLAUDE_OAUTH_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLAUDE_OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const CLAUDE_OAUTH_BETA: &str = "oauth-2025-04-20";
const CLAUDE_OAUTH_SCOPES: &str =
    "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";
const CLAUDE_USAGE_TIMEOUT_SECS: u64 = 10;
/// Le token d'acces Claude vit environ une journee. On le renouvelle un peu
/// avant l'echeance pour ne pas payer un aller-retour 401 a chaque lecture.
const CLAUDE_TOKEN_REFRESH_MARGIN_SECS: i64 = 120;

impl Default for PoolConfig {
    fn default() -> Self {
        PoolConfig {
            port: default_pool_port(),
            api_key: String::new(),
            default_model: default_pool_model(),
            reasoning_effort: default_pool_effort(),
            upstream: default_pool_upstream(),
            request_timeout_secs: default_request_timeout(),
            cooldown_secs_429: default_cooldown_429(),
            concurrency: default_pool_concurrency(),
            client_id_override: String::new(),
        }
    }
}

fn default_pool_port() -> u16 {
    8787
}
fn default_pool_model() -> String {
    "gpt-5-codex".to_string()
}
fn default_pool_effort() -> String {
    "medium".to_string()
}
fn default_pool_upstream() -> String {
    "https://chatgpt.com".to_string()
}
fn default_request_timeout() -> u64 {
    300
}
fn default_cooldown_429() -> u64 {
    60
}
fn default_pool_concurrency() -> usize {
    1
}
fn default_proxy_controls_enabled() -> bool {
    true
}
fn default_true() -> bool {
    true
}

const UNCONNECTED_ACCOUNT_EXPIRY_SECS: i64 = 10 * 60;
const DEFAULT_ACCOUNT_MODEL: &str = "gpt-5.6-sol";
const DEFAULT_ACCOUNT_REASONING_EFFORT: &str = "medium";

#[cfg_attr(feature = "desktop", tauri::command)]
pub fn load_settings() -> Result<AppSettings, String> {
    let path = settings_path()?;
    let mut settings = if path.exists() {
        let content = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        serde_json::from_str::<AppSettings>(&content).map_err(|error| error.to_string())?
    } else {
        let settings = discover_initial_settings()?;
        write_settings(&path, &settings)?;
        settings
    };

    let mut changed = false;
    if settings.auto_discover_accounts && merge_discovered_profiles(&mut settings)? {
        changed = true;
    }
    if deduplicate_accounts_by_home(&mut settings) {
        changed = true;
    }
    if ensure_special_bai_account(&mut settings)? {
        changed = true;
    }
    ensure_default_account(&mut settings);
    if ensure_agents(&mut settings) {
        changed = true;
    }
    if ensure_workspaces(&mut settings) {
        changed = true;
    }
    seed_missing_local_codex_credentials(&settings);
    if sync_account_limit_trackers(&mut settings) {
        changed = true;
    }
    if remove_expired_unconnected_accounts(&mut settings, now_unix()) {
        changed = true;
    }
    if clear_expired_home_tombstones_for_registered_accounts(&mut settings) {
        changed = true;
    }
    if changed {
        write_settings(&path, &settings)?;
    }
    Ok(settings)
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub fn save_settings(mut settings: AppSettings) -> Result<AppSettings, String> {
    let path = settings_path()?;
    let now = now_unix();
    merge_persisted_account_lifecycle(&path, &mut settings, now);
    deduplicate_accounts_by_home(&mut settings);
    ensure_special_bai_account(&mut settings)?;
    ensure_default_account(&mut settings);
    ensure_agents(&mut settings);
    ensure_workspaces(&mut settings);
    seed_missing_local_codex_credentials(&settings);
    sync_account_limit_trackers(&mut settings);
    remove_expired_unconnected_accounts(&mut settings, now);
    clear_expired_home_tombstones_for_registered_accounts(&mut settings);
    write_settings(&path, &settings)?;
    Ok(settings)
}

/// Fusionne l'etat de cycle de vie dont le serveur est l'autorite : dates des
/// profils deja connus et tombstones ajoutes apres une expiration. Un client
/// web obsolete ne peut ainsi ni repousser l'echeance, ni faire reapparaitre un
/// compte supprime. La lecture reste best-effort si le fichier n'existe pas.
fn merge_persisted_account_lifecycle(path: &Path, settings: &mut AppSettings, now: i64) {
    let persisted = fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str::<AppSettings>(&content).ok());

    for home in persisted
        .as_ref()
        .map(|value| value.expired_unconnected_account_homes.as_slice())
        .unwrap_or_default()
    {
        let normalized = account_home_key(home);
        if !settings
            .expired_unconnected_account_homes
            .iter()
            .any(|existing| account_home_key(existing) == normalized)
        {
            settings.expired_unconnected_account_homes.push(normalized);
        }
    }

    let tombstoned_homes = settings
        .expired_unconnected_account_homes
        .iter()
        .map(|home| account_home_key(home))
        .collect::<HashSet<_>>();
    for account in &mut settings.accounts {
        if let Some(existing) = persisted
            .as_ref()
            .and_then(|value| value.accounts.iter().find(|item| item.id == account.id))
        {
            // Un client ne peut ni vieillir ni rajeunir un profil existant.
            account.created_at = existing.created_at;
            // `connected_at` signifie "connecte au moins une fois". Une copie
            // cliente plus ancienne ne doit jamais pouvoir effacer cette preuve.
            if account.limits.connected_at.is_none() {
                account.limits.connected_at = existing.limits.connected_at;
            }
            if account.limits.session_anchor_at.is_none() {
                account.limits.session_anchor_at = existing.limits.session_anchor_at;
            }
            if account.limits.weekly_anchor_at.is_none() {
                account.limits.weekly_anchor_at = existing.limits.weekly_anchor_at;
            }
        } else if !tombstoned_homes.contains(&account_home_key(&account.codex_home)) {
            // Le serveur fait autorite sur l'instant de creation, notamment en
            // mode web ou l'horloge du navigateur peut etre decalee.
            account.created_at = Some(now);
        }
    }
}

/// Les settings peuvent etre partages entre plusieurs serveurs alors que leurs
/// runtimes restent isoles. Lorsqu'un profil Codex enregistre n'a pas encore de
/// credential dans l'instance courante, amorce son `auth.json` depuis une copie
/// soeur dont l'identite reelle est unique. Tout le reste du CODEX_HOME demeure
/// local (config, sessions, verrous et suppressions).
fn seed_missing_local_codex_credentials(settings: &AppSettings) -> usize {
    let Some(current_data_dir) = env::var_os("CST_DATA_DIR").map(PathBuf::from) else {
        return 0;
    };

    settings
        .accounts
        .iter()
        .filter(|account| account.provider == Provider::Codex)
        .filter_map(|account| {
            let stripped = cst_data_suffix(account.codex_home.trim())?;
            let relative_home = PathBuf::from(stripped.trim_start_matches(['\\', '/']));
            let components = relative_home.components().collect::<Vec<_>>();
            let is_account_home = components.len() >= 2
                && matches!(components.first(), Some(std::path::Component::Normal(value)) if value.to_string_lossy().eq_ignore_ascii_case("codex-homes"))
                && components
                    .iter()
                    .all(|component| matches!(component, std::path::Component::Normal(_)));
            if !is_account_home {
                return None;
            }

            let primary_home = current_data_dir.join(&relative_home);
            crate::account_usage::seed_local_data_dir_account_auth(
                &current_data_dir,
                &relative_home,
                &primary_home,
            )
            .then_some(())
        })
        .count()
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub fn ensure_account_home(
    codex_home: String,
    provider: Option<Provider>,
    bypass: bool,
    model: Option<String>,
    reasoning_effort: Option<String>,
    fast_mode: Option<bool>,
) -> Result<(), String> {
    // La creation/configuration reste propre a l'instance courante. La lecture
    // peut reutiliser un home authentifie d'une instance soeur, mais ne doit
    // jamais y ecrire implicitement.
    let home = expand_home_local(&codex_home)?;
    fs::create_dir_all(&home).map_err(|error| error.to_string())?;
    // `provider` absent (anciens appels front) => Codex : comportement inchange.
    provider
        .unwrap_or_default()
        .write_account_config(
            &home,
            bypass,
            model.as_deref(),
            reasoning_effort.as_deref(),
            fast_mode.unwrap_or(false),
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Ajoute un compte au registre partage sans transporter la vue personnelle
/// des workspaces du navigateur. Sur le serveur SaaS, tous les utilisateurs
/// authentifies voient le meme pool de comptes et les memes homes de provider.
// ---------------------------------------------------------------------------
// Compte special b.ai
// ---------------------------------------------------------------------------

/// Identifiant du compte special injecte depuis l'environnement. Comme le
/// compte fal.ai de l'image (`environment-fal`), il n'est jamais cree par
/// l'interface : sa cle API vient de l'env du noeud (`CST_BAI_API_KEY`) et le
/// compte apparait automatiquement sur chaque instance (SSD et VPS) qui la
/// possede. Le fournisseur est OpenCode/DeepSeek (modele DeepSeek V4 Flash,
/// intensite de raisonnement `max`) : c'est le seul provider chat/agents qui
/// accepte une cle `sk-` et sait piloter `deepseek/deepseek-v4-flash`.
pub(crate) const SPECIAL_BAI_ACCOUNT_ID: &str = "bai";
const BAI_API_KEY_ENV: &str = "CST_BAI_API_KEY";
const BAI_ACCOUNT_LABEL: &str = "b.ai";
const BAI_MODEL: &str = "deepseek/deepseek-v4-flash";
const BAI_REASONING_EFFORT: &str = "max";

pub(crate) fn bai_api_key_from_env() -> Option<String> {
    env::var(BAI_API_KEY_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Cree (ou met a jour) le compte special b.ai a partir de `CST_BAI_API_KEY`.
/// Retourne `true` si `settings.accounts` a change (le compte a ete ajoute),
/// `false` sinon (cle absente ou compte deja present). Idempotent.
pub(crate) fn ensure_special_bai_account(settings: &mut AppSettings) -> Result<bool, String> {
    ensure_special_bai_account_from(settings, bai_api_key_from_env().as_deref())
}

fn ensure_special_bai_account_from(
    settings: &mut AppSettings,
    key: Option<&str>,
) -> Result<bool, String> {
    let Some(key) = key.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(false);
    };

    // Home isole du compte, meme format distant que le frontend
    // (`%CST_DATA_DIR%\codex-homes/...`), resolu par `expand_home_local`.
    let codex_home = format!("%CST_DATA_DIR%\\codex-homes/opencode-b-ai");

    if let Some(account) = settings
        .accounts
        .iter()
        .find(|account| account.id == SPECIAL_BAI_ACCOUNT_ID)
    {
        seed_bai_account_auth(&account.codex_home, key)?;
        return Ok(false);
    }

    let account = AccountProfile {
        id: SPECIAL_BAI_ACCOUNT_ID.to_string(),
        label: BAI_ACCOUNT_LABEL.to_string(),
        created_at: Some(now_unix()),
        provider: Provider::OpenCode,
        inference_provider: Some("deepseek".to_string()),
        codex_home,
        project_dir: None,
        proxy_id: None,
        startup_command: None,
        limits: AccountLimitTracking::default(),
        bypass: true,
        model: Some(BAI_MODEL.to_string()),
        reasoning_effort: Some(BAI_REASONING_EFFORT.to_string()),
        fast_mode: false,
        completed_on: None,
        api_key: None,
    };
    seed_bai_account_auth(&account.codex_home, key)?;
    settings.accounts.push(account);
    Ok(true)
}

/// Ecrit la cle API dans le home isole du compte (auth opencode), sans
/// ecraser les autres fournisseurs eventuellement presents dans le fichier.
fn seed_bai_account_auth(codex_home: &str, key: &str) -> Result<(), String> {
    let home = expand_home_local(codex_home)?;
    let auth_dir = home.join("data").join("opencode");
    fs::create_dir_all(&auth_dir).map_err(|error| error.to_string())?;

    let path = auth_dir.join("auth.json");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let mut root: Value = serde_json::from_str(&existing).unwrap_or(Value::Object(Map::new()));
    if !root.is_object() {
        root = Value::Object(Map::new());
    }
    root.as_object_mut()
        .expect("racine JSON objet")
        .insert("deepseek".to_string(), json!({ "type": "api", "key": key }));
    let content = serde_json::to_string_pretty(&root).map_err(|error| error.to_string())?;
    crate::fs_util::atomic_write(&path, content).map_err(|error| error.to_string())
}

pub fn add_shared_account(account: AccountProfile) -> Result<AppSettings, String> {
    if account.id.trim().is_empty() {
        return Err("Identifiant de compte manquant".to_string());
    }
    if account.label.trim().is_empty() {
        return Err("Nom de compte manquant".to_string());
    }
    if account.codex_home.trim().is_empty() {
        return Err("Dossier du compte manquant".to_string());
    }

    let path = settings_path()?;
    let mut settings = load_settings()?;
    if settings
        .accounts
        .iter()
        .any(|existing| existing.id == account.id)
    {
        return Err("Ce compte existe deja".to_string());
    }

    let account_id = account.id.clone();
    settings.accounts.push(account);
    if settings.default_account_id.is_none() {
        settings.default_account_id = Some(account_id);
    }
    ensure_agents(&mut settings);
    sync_account_limit_trackers(&mut settings);
    write_settings(&path, &settings)?;
    Ok(settings)
}

/// Retire un compte du Pool / de la liste. Si `delete_files` est faux, ne touche
/// PAS au dossier CODEX_HOME sur le disque : seul l'enregistrement dans
/// `settings.json` est supprime (avec `auto_discover_accounts = false`, le compte
/// ne reapparait pas au prochain chargement).
///
/// Si `delete_files` est vrai, le dossier CODEX_HOME du compte est aussi efface
/// du disque (auth.json, sessions, config...). Garde-fous stricts :
/// - le dossier n'est efface que s'il ressemble a un CODEX_HOME (nom `.codex*`,
///   dossier sous `codex-homes`, ou presence d'un `auth.json`/`config.toml`) ;
/// - jamais le dossier utilisateur, un de ses ancetres, ni le dossier de
///   configuration de l'app ;
/// - si un AUTRE compte restant pointe vers le meme dossier, il est conserve.
/// En cas de dossier dangereux ou non supprimable, l'operation est annulee AVANT
/// d'ecrire `settings.json` (etat inchange) et l'erreur est remontee : le compte
/// reste alors present et l'utilisateur peut le retirer sans effacer les fichiers.
#[cfg_attr(feature = "desktop", tauri::command)]
pub fn remove_account(account_id: String, delete_files: bool) -> Result<AppSettings, String> {
    let path = settings_path()?;
    let mut settings = load_settings()?;

    let Some(target) = settings
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .cloned()
    else {
        return Err("Compte introuvable".to_string());
    };

    settings.accounts.retain(|account| account.id != account_id);

    if settings.default_account_id.as_deref() == Some(account_id.as_str()) {
        settings.default_account_id = settings.accounts.first().map(|account| account.id.clone());
    }

    ensure_agents(&mut settings);
    sync_account_limit_trackers(&mut settings);

    // Suppression du dossier AVANT l'ecriture de settings.json : si elle echoue
    // (chemin refuse par les garde-fous, verrou fichier...), on renvoie l'erreur
    // sans rien persister -> l'etat reste coherent (le compte est toujours la).
    if delete_files {
        let normalized_target = account_home_key(&target.codex_home);
        let still_referenced = settings
            .accounts
            .iter()
            .any(|account| account_home_key(&account.codex_home) == normalized_target);
        if !still_referenced {
            delete_codex_home_dir(&target.codex_home)?;
        }
    }

    write_settings(&path, &settings)?;
    Ok(settings)
}

/// Supprime le dossier CODEX_HOME d'un compte apres validation stricte. Un
/// dossier absent est traite comme un succes (rien a effacer).
fn delete_codex_home_dir(codex_home: &str) -> Result<(), String> {
    // Une suppression vise uniquement le chemin declare dans cette instance :
    // le fallback de lecture vers une instance soeur ne doit jamais devenir
    // une suppression distante implicite.
    let path = expand_home_local(codex_home)?;
    if !path.exists() {
        return Ok(());
    }
    if !path.is_dir() {
        return Err(format!(
            "Le CODEX_HOME du compte n'est pas un dossier : {}",
            path.display()
        ));
    }
    guard_deletable_codex_home(&path)?;
    fs::remove_dir_all(&path)
        .map_err(|error| format!("Suppression de {} impossible : {error}", path.display()))
}

/// Refuse d'effacer un dossier qui n'est pas manifestement le CODEX_HOME d'un
/// compte, ou qui est un chemin critique (racine/drive root, dossier
/// utilisateur et ses ancetres, dossier de configuration de l'app).
fn guard_deletable_codex_home(path: &Path) -> Result<(), String> {
    let path_key = guard_key(path);

    // Chemins trop courts (racine, drive root, chemin relatif ambigu) : refus.
    if path.components().count() < 3 || path.file_name().is_none() {
        return Err(format!(
            "Refus : chemin trop court ou critique ({}).",
            path.display()
        ));
    }

    // Dossier utilisateur lui-meme ou l'un de ses ancetres.
    if let Ok(home) = home_dir() {
        let home_key = guard_key(&home);
        if path_key == home_key || home_key.starts_with(&format!("{path_key}\\")) {
            return Err(format!(
                "Refus : {} est le dossier utilisateur (ou un parent).",
                path.display()
            ));
        }
    }

    // Dossier de configuration de l'app (settings.json, caches...).
    if let Ok(settings_file) = settings_path() {
        if let Some(app_dir) = settings_file.parent() {
            let app_key = guard_key(app_dir);
            if path_key == app_key || app_key.starts_with(&format!("{path_key}\\")) {
                return Err(format!(
                    "Refus : {} est le dossier de configuration de l'app.",
                    path.display()
                ));
            }
        }
    }

    // Le dossier doit ressembler au home d'un compte pris en charge.
    let looks_like_home = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| {
            is_codex_like_dir(name)
                || Provider::Codex.is_home_like_dir(name)
                || Provider::Claude.is_home_like_dir(name)
                || Provider::OpenCode.is_home_like_dir(name)
        })
        .unwrap_or(false);
    let under_homes = path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .map(|name| {
            name.eq_ignore_ascii_case("codex-homes")
                || name.eq_ignore_ascii_case("claude-homes")
                || name.eq_ignore_ascii_case("opencode-homes")
        })
        .unwrap_or(false);
    let has_marker = path.join("auth.json").is_file()      // Codex
        || path.join("config.toml").is_file()              // Codex
        || path.join(".credentials.json").is_file()        // Claude
        || path.join(".claude.json").is_file()             // Claude
        || path.join("data").join("opencode").join("auth.json").is_file(); // OpenCode

    if !(looks_like_home || under_homes || has_marker) {
        return Err(format!(
            "Refus : {} ne ressemble pas a un dossier de compte pris en charge.",
            path.display()
        ));
    }

    Ok(())
}

/// Cle normalisee pour comparer des chemins de facon robuste : minuscules,
/// separateurs `\`, sans separateur final.
fn guard_key(path: &Path) -> String {
    normalize_string_path(&path.to_string_lossy())
        .trim_end_matches('\\')
        .to_string()
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn account_limit_status(force: Option<bool>) -> Result<Vec<AccountLimitView>, String> {
    let settings = load_settings()?;
    tokio::task::spawn_blocking(move || {
        if force.unwrap_or(false) {
            account_limit_views(&settings)
        } else {
            account_limit_views_fast(&settings)
        }
    })
    .await
    .map_err(|error| error.to_string())
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn account_model_catalog(account_id: String) -> Result<Vec<AccountModelView>, String> {
    load_account_model_catalog(&account_id).await
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub fn import_account_docs(paths: Vec<String>) -> Result<AppSettings, String> {
    let settings_file = settings_path()?;
    let mut settings = load_settings()?;
    let files = expand_import_files(&paths)?;

    if files.is_empty() {
        return Err("Aucun fichier JSON a importer".to_string());
    }

    let mut imported = 0_usize;

    for file in files {
        let content =
            fs::read_to_string(&file).map_err(|error| format!("{}: {error}", file.display()))?;
        let value: Value = serde_json::from_str(&content)
            .map_err(|error| format!("JSON invalide {}: {error}", file.display()))?;
        let accounts = extract_import_accounts(&value);

        for account in accounts {
            import_single_account(&mut settings, account)?;
            imported += 1;
        }
    }

    if imported == 0 {
        return Err("Aucun token de compte trouve dans ces fichiers".to_string());
    }

    deduplicate_accounts_by_home(&mut settings);
    ensure_default_account(&mut settings);
    write_settings(&settings_file, &settings)?;
    Ok(settings)
}

/// Importe un ou plusieurs comptes a partir d'un contenu JSON colle
/// (blob de session ChatGPT, export `accounts[].credentials`, objet plat,
/// ou tableau de l'un de ces formats). Contrairement a `import_account_docs`,
/// aucune lecture fichier : le JSON est fourni directement par l'UI.
#[cfg_attr(feature = "desktop", tauri::command)]
pub fn import_account_json(content: String) -> Result<AppSettings, String> {
    let settings_file = settings_path()?;
    let mut settings = load_settings()?;

    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err("Contenu JSON vide".to_string());
    }

    let value = parse_import_json_content(trimmed)?;
    let accounts = extract_import_accounts(&value);

    if accounts.is_empty() {
        return Err("Aucun token de compte trouve dans ce JSON".to_string());
    }

    let mut imported = 0_usize;
    for account in accounts {
        import_single_account(&mut settings, account)?;
        imported += 1;
    }

    if imported == 0 {
        return Err("Aucun token de compte trouve dans ce JSON".to_string());
    }

    deduplicate_accounts_by_home(&mut settings);
    ensure_default_account(&mut settings);
    write_settings(&settings_file, &settings)?;
    Ok(settings)
}

fn parse_import_json_content(content: &str) -> Result<Value, String> {
    match serde_json::from_str::<Value>(content) {
        Ok(Value::String(inner)) => {
            let inner = inner.trim();
            if inner.starts_with('{') || inner.starts_with('[') {
                return parse_import_json_content(inner);
            }

            Err("JSON invalide: le contenu colle est une chaine, pas un objet".to_string())
        }
        Ok(value) => Ok(value),
        Err(error) => {
            if content.starts_with("{\\\"") || content.starts_with("[\\\"") {
                let unescaped = content.replace("\\\"", "\"");
                if let Ok(value) = serde_json::from_str::<Value>(&unescaped) {
                    return Ok(value);
                }
            }

            Err(format!("JSON invalide: {error}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_account_home(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        env::temp_dir().join(format!("cst-{prefix}-{unique}"))
    }

    fn account_for_home(home: &Path) -> AccountProfile {
        AccountProfile {
            id: "test-account".to_string(),
            label: "Test".to_string(),
            created_at: None,
            provider: Provider::Codex,
            inference_provider: None,
            codex_home: home.to_string_lossy().to_string(),
            project_dir: None,
            proxy_id: None,
            startup_command: None,
            limits: AccountLimitTracking::default(),
            bypass: true,
            model: None,
            reasoning_effort: None,
            fast_mode: false,
            completed_on: None,
            api_key: None,
        }
    }

    #[test]
    fn special_bai_account_is_created_from_env_and_writes_the_deepseek_key() {
        let data_dir = fresh_account_home("bai-account");
        env::set_var("CST_DATA_DIR", &data_dir);
        env::set_var("CST_BAI_API_KEY", "sk-test-bai-key");
        let result = (|| -> Result<(), String> {
            let mut settings = empty_settings("codex", Vec::new(), None);
            let changed = ensure_special_bai_account(&mut settings)?;
            assert!(changed, "le compte special doit etre ajoute au premier appel");

            let account = settings
                .accounts
                .iter()
                .find(|account| account.id == SPECIAL_BAI_ACCOUNT_ID)
                .expect("compte special present");
            assert_eq!(account.label, "b.ai");
            assert_eq!(account.provider, Provider::OpenCode);
            assert_eq!(account.inference_provider.as_deref(), Some("deepseek"));
            assert_eq!(account.model.as_deref(), Some("deepseek/deepseek-v4-flash"));
            assert_eq!(account.reasoning_effort.as_deref(), Some("max"));
            assert!(account.codex_home.starts_with("%CST_DATA_DIR%"));

            // La cle est ecrite dans le home isole, format auth opencode.
            let auth_path =
                data_dir.join("codex-homes/opencode-b-ai/data/opencode/auth.json");
            let auth: Value =
                serde_json::from_str(&fs::read_to_string(&auth_path).unwrap()).unwrap();
            assert_eq!(auth["deepseek"]["key"], "sk-test-bai-key");
            assert_eq!(auth["deepseek"]["type"], "api");

            // Idempotent : un second appel ne re-ajoute pas le compte.
            let second = ensure_special_bai_account(&mut settings)?;
            assert!(!second, "le compte ne doit pas etre duplique");
            assert_eq!(
                settings
                    .accounts
                    .iter()
                    .filter(|account| account.id == SPECIAL_BAI_ACCOUNT_ID)
                    .count(),
                1
            );
            Ok(())
        })();
        env::remove_var("CST_BAI_API_KEY");
        env::remove_var("CST_DATA_DIR");
        let _ = fs::remove_dir_all(&data_dir);
        result.expect("test reussi");
    }

    #[test]
    fn special_bai_account_is_absent_without_env_key() {
        env::remove_var("CST_BAI_API_KEY");
        let mut settings = empty_settings("codex", Vec::new(), None);
        let changed = ensure_special_bai_account(&mut settings).unwrap();
        assert!(!changed);
        assert!(
            !settings
                .accounts
                .iter()
                .any(|account| account.id == SPECIAL_BAI_ACCOUNT_ID)
        );
    }

    #[test]
    fn account_daily_completion_is_backward_compatible_and_persists_the_day() {
        let mut account: AccountProfile = serde_json::from_value(json!({
            "id": "daily-account",
            "label": "Compte quotidien",
            "codexHome": ".codex-daily"
        }))
        .unwrap();
        assert_eq!(account.completed_on, None);

        account.completed_on = Some("2026-08-20".to_string());
        let value = serde_json::to_value(account).unwrap();
        assert_eq!(value["completedOn"], "2026-08-20");
    }

    #[test]
    fn account_home_key_expands_userprofile_aliases() {
        let suffix = format!(".codex-account-dedupe-{}", std::process::id());
        let absolute = home_dir().unwrap().join(&suffix);
        let aliased = format!("%userprofile%/{suffix}/");

        assert_eq!(
            account_home_key(&absolute.to_string_lossy()),
            account_home_key(&aliased)
        );
    }

    #[test]
    fn duplicate_account_homes_keep_only_the_default_profile() {
        let suffix = format!(".codex-account-dedupe-default-{}", std::process::id());
        let absolute = home_dir().unwrap().join(&suffix);

        let mut discovered = account_for_home(&absolute);
        discovered.id = "auto-discovered".to_string();
        discovered.label = "Nom automatique".to_string();
        discovered.model = Some("modele-conserve".to_string());

        let mut selected = account_for_home(&absolute);
        selected.id = "selected-account".to_string();
        selected.label = "Compte choisi".to_string();
        selected.codex_home = format!("%USERPROFILE%\\{suffix}");
        selected.model = None;

        let mut other = account_for_home(&fresh_account_home("dedupe-other"));
        other.id = "other".to_string();

        let mut settings = empty_settings("codex", Vec::new(), None);
        settings.accounts = vec![discovered, selected, other];
        settings.default_account_id = Some("selected-account".to_string());

        assert!(deduplicate_accounts_by_home(&mut settings));
        assert_eq!(settings.accounts.len(), 2);
        assert_eq!(settings.accounts[0].id, "selected-account");
        assert_eq!(settings.accounts[0].label, "Compte choisi");
        assert_eq!(
            settings.accounts[0].model.as_deref(),
            Some("modele-conserve")
        );
        assert_eq!(settings.accounts[1].id, "other");
    }

    #[test]
    fn unconnected_new_account_expires_after_exactly_ten_minutes() {
        let mut pending = account_for_home(&fresh_account_home("pending-expiry"));
        pending.id = "pending".to_string();
        pending.created_at = Some(1_000);

        let mut connected = account_for_home(&fresh_account_home("connected-expiry"));
        connected.id = "connected".to_string();
        connected.created_at = Some(1_000);
        connected.limits.connected_at = Some(1_120);

        let mut legacy = account_for_home(&fresh_account_home("legacy-expiry"));
        legacy.id = "legacy".to_string();
        legacy.created_at = None;

        let mut settings = empty_settings("codex", Vec::new(), None);
        settings.accounts = vec![pending, connected, legacy];
        settings.default_account_id = Some("pending".to_string());

        assert!(!remove_expired_unconnected_accounts(&mut settings, 1_599));
        assert_eq!(settings.accounts.len(), 3);

        assert!(remove_expired_unconnected_accounts(&mut settings, 1_600));
        assert_eq!(
            settings
                .accounts
                .iter()
                .map(|account| account.id.as_str())
                .collect::<Vec<_>>(),
            vec!["connected", "legacy"]
        );
        assert_eq!(settings.default_account_id.as_deref(), Some("connected"));
        assert_eq!(settings.expired_unconnected_account_homes.len(), 1);
        assert!(settings.expired_unconnected_account_homes[0].contains("pending-expiry"));
    }

    #[test]
    fn credentials_preserve_an_expired_account_and_mark_it_connected() {
        let home = fresh_account_home("authenticated-expiry");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("auth.json"),
            r#"{"tokens":{"access_token":"connected"}}"#,
        )
        .unwrap();

        let mut account = account_for_home(&home);
        account.created_at = Some(1_000);
        let mut settings = empty_settings("codex", Vec::new(), None);
        settings.accounts.push(account);

        assert!(sync_account_limit_trackers(&mut settings));
        assert!(!remove_expired_unconnected_accounts(&mut settings, 1_600));
        assert!(settings.accounts[0].limits.connected_at.is_some());

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn stale_legacy_client_cannot_restore_an_already_expired_home() {
        let home = fresh_account_home("stale-expired-home");
        let mut stale_account = account_for_home(&home);
        stale_account.created_at = None;

        let mut settings = empty_settings("codex", Vec::new(), None);
        settings
            .expired_unconnected_account_homes
            .push(normalize_string_path(home.to_string_lossy().as_ref()));
        settings.accounts.push(stale_account);

        assert!(remove_expired_unconnected_accounts(&mut settings, 10_000));
        assert!(settings.accounts.is_empty());
        assert_eq!(settings.expired_unconnected_account_homes.len(), 1);
    }

    #[test]
    fn stale_save_preserves_expiration_tombstones_from_disk() {
        let dir = fresh_account_home("persisted-expired-home");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let mut persisted = empty_settings("codex", Vec::new(), None);
        persisted
            .expired_unconnected_account_homes
            .push("C:\\Users\\Test\\.codex-expired".to_string());
        write_settings(&path, &persisted).unwrap();

        let mut stale = empty_settings("codex", Vec::new(), None);
        merge_persisted_account_lifecycle(&path, &mut stale, 12_345);

        assert_eq!(stale.expired_unconnected_account_homes.len(), 1);
        assert_eq!(
            stale.expired_unconnected_account_homes[0],
            normalize_string_path("C:\\Users\\Test\\.codex-expired")
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn server_clock_stamps_only_new_account_profiles() {
        let dir = fresh_account_home("account-created-at");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let mut persisted = empty_settings("codex", Vec::new(), None);
        let mut legacy = account_for_home(&dir.join("legacy"));
        legacy.id = "legacy".to_string();
        legacy.created_at = None;
        legacy.limits.connected_at = Some(10_000);
        persisted.accounts.push(legacy.clone());
        write_settings(&path, &persisted).unwrap();

        // Le client tente de modifier la date du profil existant et envoie un
        // nouveau profil sans date (cas d'un ancien frontend).
        legacy.created_at = Some(1);
        legacy.limits = AccountLimitTracking::default();
        let mut new_account = account_for_home(&dir.join("new"));
        new_account.id = "new".to_string();
        let mut incoming = empty_settings("codex", Vec::new(), None);
        incoming.accounts = vec![legacy, new_account];

        merge_persisted_account_lifecycle(&path, &mut incoming, 12_345);

        assert_eq!(incoming.accounts[0].created_at, None);
        assert_eq!(incoming.accounts[0].limits.connected_at, Some(10_000));
        assert_eq!(incoming.accounts[1].created_at, Some(12_345));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn account_home_is_provisioned_with_all_account_defaults() {
        let home = fresh_account_home("account-config");

        ensure_account_home(
            home.to_string_lossy().to_string(),
            Some(Provider::Codex),
            true,
            Some("gpt-5.6-sol".to_string()),
            Some("medium".to_string()),
            Some(true),
        )
        .expect("account home should be created and provisioned");

        let config = fs::read_to_string(home.join("config.toml"))
            .expect("config.toml should exist immediately");
        assert!(config.contains("approval_policy = \"never\""));
        assert!(config.contains("sandbox_mode = \"danger-full-access\""));
        assert!(config.contains("model = \"gpt-5.6-sol\""));
        assert!(config.contains("model_reasoning_effort = \"medium\""));
        assert!(config.contains("service_tier = \"priority\""));

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn non_bypass_account_gets_safe_explicit_permissions() {
        let home = fresh_account_home("safe-account");

        ensure_account_home(
            home.to_string_lossy().to_string(),
            Some(Provider::Codex),
            false,
            None,
            None,
            Some(false),
        )
        .expect("non-bypass account should be provisioned");

        let config = fs::read_to_string(home.join("config.toml")).expect("config.toml");
        assert!(config.contains("approval_policy = \"on-request\""));
        assert!(config.contains("sandbox_mode = \"workspace-write\""));
        assert!(!config.contains("danger-full-access"));
        assert!(!config.contains("approval_policy = \"never\""));

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn disabling_bypass_replaces_previously_persisted_bypass() {
        let home = fresh_account_home("bypass-transition");
        ensure_codex_account_config(&home, true, Some("gpt-5.6-sol"), Some("high"), false)
            .expect("bypass config");
        ensure_codex_account_config(&home, false, Some("gpt-5.6-sol"), Some("high"), false)
            .expect("safe config");

        let config = fs::read_to_string(home.join("config.toml")).expect("config.toml");
        assert!(config.contains("approval_policy = \"on-request\""));
        assert!(config.contains("sandbox_mode = \"workspace-write\""));
        assert!(!config.contains("danger-full-access"));
        assert!(!config.contains("approval_policy = \"never\""));
        assert_eq!(config.matches("approval_policy =").count(), 1);
        assert_eq!(config.matches("sandbox_mode =").count(), 1);

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn disabling_fast_mode_removes_only_the_root_service_tier() {
        let home = fresh_account_home("fast-mode-transition");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("config.toml"),
            concat!(
                "# service_tier = \"commented\"\n",
                "[profiles.custom]\n",
                "service_tier = \"flex\"\n",
            ),
        )
        .unwrap();

        ensure_codex_account_config(&home, true, Some("gpt-5.6-sol"), Some("high"), true)
            .expect("fast config");
        let enabled = fs::read_to_string(home.join("config.toml")).unwrap();
        assert!(enabled.starts_with("service_tier = \"priority\"\n"));
        assert!(enabled.contains("[profiles.custom]\nservice_tier = \"flex\""));

        ensure_codex_account_config(&home, true, Some("gpt-5.6-sol"), Some("high"), false)
            .expect("normal config");
        let disabled = fs::read_to_string(home.join("config.toml")).unwrap();
        assert!(!disabled.contains("service_tier = \"priority\""));
        assert!(disabled.contains("# service_tier = \"commented\""));
        assert!(disabled.contains("[profiles.custom]\nservice_tier = \"flex\""));

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn account_config_preserves_sections_and_is_idempotent() {
        let home = fresh_account_home("config-idempotent");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("config.toml"),
            "[mcp_servers.example]\nurl = \"http://127.0.0.1:8123/mcp\"\n",
        )
        .unwrap();

        ensure_codex_account_config(&home, true, Some("gpt-5.6-sol"), Some("xhigh"), true)
            .expect("first provisioning");
        let once = fs::read_to_string(home.join("config.toml")).unwrap();
        ensure_codex_account_config(&home, true, Some("gpt-5.6-sol"), Some("xhigh"), true)
            .expect("second provisioning");
        let twice = fs::read_to_string(home.join("config.toml")).unwrap();

        assert_eq!(once, twice);
        assert!(twice.contains("[mcp_servers.example]"));
        assert!(twice.contains("url = \"http://127.0.0.1:8123/mcp\""));
        assert_eq!(twice.matches("service_tier =").count(), 1);
        assert!(twice.contains("goals = true"));

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn windows_sandbox_override_updates_only_the_windows_table() {
        let existing = concat!(
            "sandbox_mode = \"workspace-write\"\n",
            "\n",
            "[windows]\n",
            "sandbox = \"elevated\"\n",
            "private_desktop = true\n",
            "\n",
            "[mcp_servers.example]\n",
            "url = \"http://127.0.0.1:8123/mcp\"\n",
        );

        let updated = upsert_table_string(existing, "windows", "sandbox", "unelevated");

        assert!(updated.contains("[windows]\nsandbox = \"unelevated\""));
        assert!(updated.contains("private_desktop = true"));
        assert!(updated.contains("[mcp_servers.example]"));
        assert_eq!(updated.matches("sandbox =").count(), 1);
        assert_eq!(
            upsert_table_string(&updated, "windows", "sandbox", "unelevated"),
            updated
        );
    }

    #[test]
    fn windows_sandbox_override_adds_a_missing_table() {
        let existing = "model = \"gpt-test\"\n";
        let updated = upsert_table_string(existing, "windows", "sandbox", "unelevated");

        assert_eq!(
            updated,
            "model = \"gpt-test\"\n\n[windows]\nsandbox = \"unelevated\"\n"
        );
    }

    #[test]
    fn account_config_rejects_invalid_reasoning_effort() {
        let home = fresh_account_home("invalid-effort");
        let error = ensure_codex_account_config(&home, true, None, Some("ultra mode"), false)
            .expect_err("malformed effort must be rejected");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(!home.join("config.toml").exists());
    }

    #[test]
    fn account_config_accepts_max_and_ultra_reasoning_efforts() {
        let home = fresh_account_home("max-ultra-effort");
        ensure_codex_account_config(&home, true, Some("gpt-5.6-sol"), Some("max"), false)
            .expect("max effort");
        ensure_codex_account_config(&home, true, Some("gpt-5.6-sol"), Some("ultra"), false)
            .expect("ultra effort");

        let config = fs::read_to_string(home.join("config.toml")).unwrap();
        assert!(config.contains("model_reasoning_effort = \"ultra\""));
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn account_config_escapes_model_before_writing_toml() {
        let home = fresh_account_home("model-escape");
        let injected = "gpt-5.6-sol\"\nsandbox_mode = \"danger-full-access";
        ensure_codex_account_config(&home, false, Some(injected), Some("low"), false)
            .expect("escaped model should remain a TOML string");

        let config = fs::read_to_string(home.join("config.toml")).unwrap();
        ensure_codex_account_config(&home, false, Some(injected), Some("low"), false)
            .expect("escaped model should stay idempotent");
        let second = fs::read_to_string(home.join("config.toml")).unwrap();
        assert_eq!(config, second);
        assert!(
            config.contains("model = \"gpt-5.6-sol\\\"\\nsandbox_mode = \\\"danger-full-access\"")
        );
        assert_eq!(config.matches("sandbox_mode =").count(), 2);
        assert_eq!(config.matches("\nsandbox_mode =").count(), 1);
        assert!(config.contains("sandbox_mode = \"workspace-write\""));

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn legacy_account_json_keeps_optional_model_fields_unset() {
        let account: AccountProfile = serde_json::from_value(json!({
            "id": "legacy",
            "label": "Legacy",
            "codexHome": "~/.codex-legacy",
            "bypass": false
        }))
        .expect("legacy account should deserialize");

        assert_eq!(account.created_at, None);
        assert_eq!(account.model, None);
        assert_eq!(account.reasoning_effort, None);
        assert!(!account.fast_mode);
    }

    fn assert_session_blob_import(content: &str) {
        let value = parse_import_json_content(content).expect("session blob should parse");
        let accounts = extract_import_accounts(&value);

        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].access_token, "header.payload.signature");
        assert_eq!(accounts[0].refresh_token, "");
        assert_eq!(accounts[0].account_id, "account-1");
        assert_eq!(accounts[0].label, "Pool ava@example.com");
    }

    #[test]
    fn parses_pasted_chatgpt_session_blob_variants() {
        let raw = r#"{"accessToken":"header.payload.signature","user":{"id":"user-1","name":"","email":"ava@example.com"},"account":{"id":"account-1","planType":"plus"}}"#;
        let escaped_object = raw.replace('"', "\\\"");
        let quoted_json_string = serde_json::to_string(raw).expect("quoted string");

        assert_session_blob_import(raw);
        assert_session_blob_import(&escaped_object);
        assert_session_blob_import(&quoted_json_string);
    }

    #[test]
    fn upsert_inserts_key_into_empty_document() {
        let out = upsert_top_level_string("", "approval_policy", "never");
        assert_eq!(out, "approval_policy = \"never\"\n");
    }

    #[test]
    fn upsert_prefixes_key_before_existing_tables() {
        let existing = "[mcp_servers.example]\nurl = \"http://127.0.0.1:8123/mcp\"\n";
        let out = upsert_top_level_string(existing, "sandbox_mode", "danger-full-access");
        // La cle racine doit preceder la table pour rester du TOML valide.
        assert!(out.starts_with("sandbox_mode = \"danger-full-access\"\n"));
        assert!(out.contains("[mcp_servers.example]"));
    }

    #[test]
    fn upsert_replaces_existing_top_level_value() {
        let existing = "approval_policy = \"on-request\"\nmodel = \"gpt-5\"\n";
        let out = upsert_top_level_string(existing, "approval_policy", "never");
        assert!(out.contains("approval_policy = \"never\""));
        assert!(!out.contains("on-request"));
        assert!(out.contains("model = \"gpt-5\""));
        // Une seule occurrence de la cle (pas de doublon => TOML valide).
        assert_eq!(out.matches("approval_policy").count(), 1);
    }

    #[test]
    fn upsert_ignores_same_key_inside_a_table_and_comments() {
        let existing =
            "# approval_policy = \"never\"\n[profiles.x]\napproval_policy = \"untrusted\"\n";
        let out = upsert_top_level_string(existing, "approval_policy", "never");
        // La cle sous [profiles.x] et la ligne commentee ne sont pas touchees ;
        // la cle racine est prefixee.
        assert!(out.starts_with("approval_policy = \"never\"\n"));
        assert!(out.contains("[profiles.x]"));
        assert!(out.contains("approval_policy = \"untrusted\""));
        assert!(out.contains("# approval_policy = \"never\""));
    }

    #[test]
    fn upsert_does_not_match_key_prefix() {
        // `approval_policy_extra` ne doit pas etre confondu avec `approval_policy`.
        let existing = "approval_policy_extra = \"x\"\n";
        let out = upsert_top_level_string(existing, "approval_policy", "never");
        assert!(out.starts_with("approval_policy = \"never\"\n"));
        assert!(out.contains("approval_policy_extra = \"x\""));
    }

    #[test]
    fn upsert_is_idempotent() {
        let once = upsert_top_level_string("", "approval_policy", "never");
        let twice = upsert_top_level_string(&once, "approval_policy", "never");
        assert_eq!(once, twice);
    }

    #[test]
    fn upsert_does_not_misfire_inside_multiline_string() {
        // Une chaine multi-ligne contenant une ligne `[...]` ET une ligne
        // `approval_policy = ...` ne doit NI latcher in_table NI etre prise pour
        // la vraie cle top-level. La vraie cle (apres la chaine) est remplacee.
        let existing = "notify_tpl = \"\"\"\n[warn] hi\napproval_policy = \"x\"\n\"\"\"\napproval_policy = \"on-request\"\n";
        let out = upsert_top_level_string(existing, "approval_policy", "never");
        // Contenu de la chaine preserve tel quel.
        assert!(out.contains("[warn] hi"));
        assert!(out.contains("approval_policy = \"x\""));
        // Une seule occurrence EFFECTIVE remplacee, pas de doublon top-level.
        assert!(out.contains("approval_policy = \"never\""));
        assert!(!out.contains("on-request"));
        // Pas de prefixe : la cle a ete remplacee en place, donc le fichier
        // commence toujours par la chaine multi-ligne.
        assert!(out.starts_with("notify_tpl = \"\"\""));
        assert_eq!(out.matches("approval_policy = \"never\"").count(), 1);
    }

    #[test]
    fn upsert_does_not_latch_on_multiline_array_rows() {
        // Un tableau multi-ligne dont des lignes commencent par `[` ne doit pas
        // etre pris pour une table : la cle top-level qui suit reste remplacable.
        let existing = "matrix = [\n[1, 2],\n[3, 4],\n]\napproval_policy = \"on-request\"\n";
        let out = upsert_top_level_string(existing, "approval_policy", "never");
        assert!(out.contains("matrix = ["));
        assert!(out.contains("[1, 2],"));
        assert!(out.contains("approval_policy = \"never\""));
        assert!(!out.contains("on-request"));
        assert!(out.starts_with("matrix = ["));
    }

    #[test]
    fn upsert_is_utf8_safe() {
        // Ne doit pas paniquer sur des caracteres multi-octets (chemins accentues).
        let existing = "label = \"Café Références ☕\"\n[mcp_servers.x]\nurl = \"http://é\"\n";
        let out = upsert_top_level_string(existing, "approval_policy", "never");
        assert!(out.starts_with("approval_policy = \"never\"\n"));
        assert!(out.contains("Café Références ☕"));
    }

    fn empty_settings(
        codex_command: &str,
        agents: Vec<AgentProfile>,
        active: Option<&str>,
    ) -> AppSettings {
        AppSettings {
            accounts: Vec::new(),
            proxies: Vec::new(),
            default_account_id: None,
            shell: "sh".to_string(),
            codex_command: codex_command.to_string(),
            auto_run_codex: true,
            proxy_controls_enabled: true,
            pool: PoolConfig::default(),
            agents,
            active_agent_id: active.map(ToString::to_string),
            kombai: KombaiConfig::default(),
            codex_bypass: true,
            auto_discover_accounts: false,
            expired_unconnected_account_homes: Vec::new(),
            workspaces: Vec::new(),
            closed_workspace_ids: Vec::new(),
            claude_persistent_session: false,
        }
    }

    #[test]
    fn workspace_base_name_is_utf8_safe_and_strips_git() {
        // Regression: `base[len-4..]` panickait quand un caractere multi-octets
        // chevauchait la borne len-4 (ex. « Éire », « 😀x »).
        assert_eq!(workspace_base_name("C:\\Projects\\Éire"), "Éire");
        assert_eq!(workspace_base_name("/home/u/😀x"), "😀x");
        assert_eq!(workspace_base_name("/home/u/日本語"), "日本語");
        // Strip `.git` insensible a la casse, mais pas si c'est tout le segment.
        assert_eq!(workspace_base_name("/srv/myrepo.git"), "myrepo");
        assert_eq!(workspace_base_name("/srv/Repo.GIT"), "Repo");
        assert_eq!(workspace_base_name("C:/repos/.git"), ".git");
        // Slashes finaux et backslashes mixtes.
        assert_eq!(workspace_base_name("C:\\proj\\app\\"), "app");
        assert_eq!(workspace_base_name("/a/b/c/"), "c");
    }

    #[test]
    fn ensure_workspaces_dedups_and_fills_labels_without_panicking() {
        let mut settings = empty_settings("codex", Vec::new(), None);
        settings.workspaces = vec![
            WorkspaceProfile {
                id: "ancien-id-local".to_string(),
                label: String::new(),
                memory: String::new(),
                execution_target_id: None,
                path: "C:\\Projects\\Éire\\".to_string(),
            },
            // Meme chemin avec une autre casse, d'autres separateurs et un id
            // historique different : fusionne avec la premiere occurrence.
            WorkspaceProfile {
                id: "ancien-id-distant".to_string(),
                label: "dup".to_string(),
                memory: "Conserver cette decision".to_string(),
                execution_target_id: Some(" HTTP://127.0.0.1:18082/ ".to_string()),
                path: "c:/projects/éire".to_string(),
            },
            // Chemin vide : retire.
            WorkspaceProfile {
                id: "x".to_string(),
                label: "vide".to_string(),
                memory: String::new(),
                execution_target_id: None,
                path: "   ".to_string(),
            },
        ];

        let changed = ensure_workspaces(&mut settings);

        assert!(changed);
        assert_eq!(settings.workspaces.len(), 1);
        let ws = &settings.workspaces[0];
        assert_eq!(ws.id, "c:/projects/éire");
        // Label vide comble par le nom du dossier (UTF-8 safe).
        assert_eq!(ws.label, "Éire");
        assert_eq!(ws.memory, "Conserver cette decision");
        assert_eq!(
            ws.execution_target_id.as_deref(),
            Some("http://127.0.0.1:18082")
        );
    }

    #[test]
    fn ensure_workspaces_keeps_closed_workspaces_closed() {
        let mut settings = empty_settings("codex", Vec::new(), None);
        settings.workspaces = vec![WorkspaceProfile {
            id: "ancien-id".to_string(),
            label: "Projet".to_string(),
            memory: "Decision durable".to_string(),
            execution_target_id: None,
            path: "C:\\Projects\\Projet".to_string(),
        }];
        settings.closed_workspace_ids = vec![
            " C:\\Projects\\Projet\\ ".to_string(),
            "c:/projects/projet".to_string(),
        ];

        let changed = ensure_workspaces(&mut settings);

        assert!(changed);
        assert!(settings.workspaces.is_empty());
        assert_eq!(settings.closed_workspace_ids, vec!["c:/projects/projet"]);
    }

    #[test]
    fn workspace_memory_is_bounded_and_only_matches_its_environment() {
        let mut settings = empty_settings("codex", Vec::new(), None);
        settings.workspaces = vec![WorkspaceProfile {
            id: "ancien-id".to_string(),
            label: "Produit".to_string(),
            execution_target_id: None,
            path: "C:\\Projects\\Produit".to_string(),
            memory: format!("  {}  ", "é".repeat(MAX_WORKSPACE_MEMORY_CHARS + 20)),
        }];

        assert!(ensure_workspaces(&mut settings));
        assert_eq!(
            settings.workspaces[0].memory.chars().count(),
            MAX_WORKSPACE_MEMORY_CHARS
        );
        assert!(workspace_memory_for_path(&settings, Path::new("c:/projects/produit/")).is_some());
        assert_eq!(
            workspace_memory_for_path(&settings, Path::new("C:\\Projects\\Autre")),
            None
        );
    }

    #[test]
    fn ensure_agents_seeds_codex_and_claude_on_fresh_settings() {
        let mut settings = empty_settings("codex", Vec::new(), None);

        let changed = ensure_agents(&mut settings);

        assert!(changed);
        let codex = settings
            .agents
            .iter()
            .find(|agent| agent.id == CODEX_AGENT_ID)
            .expect("codex agent seeded");
        assert!(codex.builtin);
        assert_eq!(codex.command, "codex");
        assert_eq!(codex.kind, "cli");
        assert_eq!(codex.provider, Provider::Codex);
        assert_eq!(codex.status_command.as_deref(), Some("login status"));
        // L'agent Claude Code integre est seed a cote de Codex.
        let claude = settings
            .agents
            .iter()
            .find(|agent| agent.id == CLAUDE_AGENT_ID)
            .expect("claude agent seeded");
        assert!(claude.builtin);
        assert_eq!(claude.command, "claude");
        assert_eq!(claude.kind, "cli");
        assert_eq!(claude.provider, Provider::Claude);
        assert_eq!(claude.login_command.as_deref(), Some("auth login"));
        assert_eq!(claude.status_command.as_deref(), Some("auth status"));
        let opencode = settings
            .agents
            .iter()
            .find(|agent| agent.id == OPENCODE_AGENT_ID)
            .expect("opencode agent seeded");
        assert!(opencode.builtin);
        assert_eq!(opencode.command, "opencode");
        assert_eq!(opencode.provider, Provider::OpenCode);
        assert_eq!(opencode.login_command.as_deref(), Some("auth login"));
        assert_eq!(opencode.status_command.as_deref(), Some("auth list"));
        // Kombai n'est pas un agent terminal : il ne doit PAS etre seed ici.
        assert!(!settings
            .agents
            .iter()
            .any(|agent| agent.id == KOMBAI_AGENT_ID));
        // L'agent actif par defaut reste Codex (comportement historique).
        assert_eq!(settings.active_agent_id.as_deref(), Some(CODEX_AGENT_ID));
    }

    #[test]
    fn ensure_agents_seeds_both_freebuff_agents() {
        let mut settings = empty_settings("codex", Vec::new(), None);

        let changed = ensure_agents(&mut settings);

        assert!(changed);
        // Le CLI freebuff n'expose que `login` : ni status ni doctor n'existent,
        // les seeder produirait des commandes qui echouent a l'usage.
        let cli = settings
            .agents
            .iter()
            .find(|agent| agent.id == FREEBUFF_AGENT_ID)
            .expect("freebuff agent seeded");
        assert!(cli.builtin);
        assert_eq!(cli.command, "freebuff");
        assert_eq!(cli.kind, "cli");
        assert_eq!(cli.provider, Provider::Freebuff);
        assert_eq!(cli.login_command.as_deref(), Some("login"));
        assert_eq!(cli.status_command, None);
        assert_eq!(cli.doctor_command, None);

        // Freebuff Desktop est un lanceur d'application, pas un agent terminal.
        let desktop = settings
            .agents
            .iter()
            .find(|agent| agent.id == FREEBUFF_DESKTOP_AGENT_ID)
            .expect("freebuff desktop agent seeded");
        assert_eq!(desktop.kind, "ide");
        assert!(desktop.command.contains("%LOCALAPPDATA%"));
    }

    /// `command_for_provider` retient le premier agent integre du provider. Les
    /// deux agents freebuff le partagent : c'est le CLI qui doit gagner, sans
    /// quoi piloter le provider lancerait l'application de bureau.
    #[test]
    fn freebuff_provider_command_resolves_to_the_cli_not_the_desktop_app() {
        let mut settings = empty_settings("codex", Vec::new(), None);
        ensure_agents(&mut settings);

        assert_eq!(
            command_for_provider(&settings, Provider::Freebuff),
            "freebuff"
        );
    }

    /// L'intensite est exposee en lecture seule : le CLI freebuff ne persiste
    /// aucun reglage d'intensite, il applique la valeur propre au modele.
    #[test]
    fn freebuff_catalog_exposes_efforts_as_read_only() {
        let catalog = freebuff_model_catalog();

        assert!(catalog
            .iter()
            .any(|model| model.id == "deepseek/deepseek-v4-pro"));
        // Le seul modele illimite doit rester identifiable et arriver en
        // tete : c'est celui applique par defaut aux nouveaux comptes.
        assert_eq!(catalog[0].id, "deepseek/deepseek-v4-flash");
        assert!(catalog[0].display_name.contains("illimite"));
        assert!(catalog
            .iter()
            .filter(|model| model.id != "deepseek/deepseek-v4-flash")
            .all(|model| model.display_name.contains("premium")));
        assert!(catalog
            .iter()
            .all(|model| model.supported_reasoning_efforts.is_empty()));
        assert!(catalog.iter().all(|model| !model.supports_fast_mode));
    }

    #[test]
    fn ensure_agents_repairs_missing_builtin_claude_commands() {
        let claude = AgentProfile {
            id: CLAUDE_AGENT_ID.to_string(),
            label: "Claude Code".to_string(),
            command: "claude".to_string(),
            provider: Provider::Claude,
            kind: "cli".to_string(),
            builtin: true,
            login_command: None,
            status_command: Some(" ".to_string()),
            doctor_command: None,
        };
        let mut settings = empty_settings("codex", vec![claude], Some(CLAUDE_AGENT_ID));

        assert!(ensure_agents(&mut settings));

        let repaired = settings
            .agents
            .iter()
            .find(|agent| agent.id == CLAUDE_AGENT_ID)
            .expect("agent Claude repare");
        assert_eq!(repaired.login_command.as_deref(), Some("auth login"));
        assert_eq!(repaired.status_command.as_deref(), Some("auth status"));
        assert_eq!(repaired.doctor_command.as_deref(), Some("doctor"));
        assert!(!ensure_agents(&mut settings));
    }

    #[test]
    fn ensure_agents_removes_default_kombai_agent() {
        let legacy_kombai = AgentProfile {
            id: KOMBAI_AGENT_ID.to_string(),
            label: "Kombai".to_string(),
            command: "kombai".to_string(),
            provider: Provider::Codex,
            kind: "cli".to_string(),
            builtin: false,
            login_command: None,
            status_command: None,
            doctor_command: None,
        };
        let mut settings = empty_settings("codex", vec![legacy_kombai], Some(CODEX_AGENT_ID));

        let _ = ensure_agents(&mut settings);

        // L'agent Kombai seed par defaut est retire (Kombai = onglet dedie).
        assert!(!settings
            .agents
            .iter()
            .any(|agent| agent.id == KOMBAI_AGENT_ID));
    }

    #[test]
    fn ensure_agents_keeps_customized_kombai_agent() {
        let custom_kombai = AgentProfile {
            id: KOMBAI_AGENT_ID.to_string(),
            label: "Kombai".to_string(),
            command: "my-kombai-wrapper".to_string(),
            provider: Provider::Codex,
            kind: "cli".to_string(),
            builtin: false,
            login_command: None,
            status_command: None,
            doctor_command: None,
        };
        let mut settings = empty_settings("codex", vec![custom_kombai], Some(CODEX_AGENT_ID));

        let _ = ensure_agents(&mut settings);

        // Un agent Kombai personnalise (commande non-default) est conserve.
        assert!(settings
            .agents
            .iter()
            .any(|agent| agent.id == KOMBAI_AGENT_ID));
    }

    #[test]
    fn ensure_agents_syncs_codex_command_and_does_not_reseed_kombai() {
        let codex_agent = AgentProfile {
            id: CODEX_AGENT_ID.to_string(),
            label: "Codex".to_string(),
            command: "codex-custom".to_string(),
            provider: Provider::Codex,
            kind: "cli".to_string(),
            builtin: true,
            login_command: Some("login".to_string()),
            status_command: None,
            doctor_command: None,
        };
        let mut settings = empty_settings("codex", vec![codex_agent], Some(CODEX_AGENT_ID));

        let _ = ensure_agents(&mut settings);

        // Le champ historique suit la commande de l'agent Codex.
        assert_eq!(settings.codex_command, "codex-custom");
        // Kombai n'est PAS recree quand le registre etait deja renseigne.
        assert!(!settings
            .agents
            .iter()
            .any(|agent| agent.id == KOMBAI_AGENT_ID));
    }

    #[test]
    fn ensure_agents_repairs_dangling_active_agent_id() {
        let mut settings = empty_settings("codex", Vec::new(), Some("ghost-agent"));

        let _ = ensure_agents(&mut settings);

        assert_eq!(settings.active_agent_id.as_deref(), Some(CODEX_AGENT_ID));
    }

    /// Aucun test ne doit dependre du reseau. Seules les lectures de quotas
    /// distantes consultent cet interrupteur : l'armer une fois pour tout le
    /// binaire de test reste donc deterministe quel que soit l'ordre.
    fn disable_remote_account_limits() {
        REMOTE_ACCOUNT_LIMITS_DISABLED.store(true, Ordering::Relaxed);
    }

    /// Reponse reelle de `GET /api/oauth/usage` (compte Max), conservee telle
    /// quelle : c'est le contrat que le parseur doit respecter.
    const CLAUDE_USAGE_SAMPLE: &str = r#"{
      "five_hour": {"utilization": 61.0, "resets_at": "2026-07-26T17:20:00.499219+00:00",
        "limit_dollars": null, "used_dollars": null, "remaining_dollars": null},
      "seven_day": {"utilization": 66.0, "resets_at": "2026-07-29T20:00:00.499238+00:00",
        "limit_dollars": null, "used_dollars": null, "remaining_dollars": null},
      "seven_day_oauth_apps": null, "seven_day_opus": null, "seven_day_sonnet": null,
      "extra_usage": {"is_enabled": false, "monthly_limit": null},
      "limits": [
        {"kind": "session", "group": "session", "percent": 61, "severity": "normal",
         "resets_at": "2026-07-26T17:20:00.499219+00:00", "scope": null, "is_active": false},
        {"kind": "weekly_all", "group": "weekly", "percent": 66, "severity": "normal",
         "resets_at": "2026-07-29T20:00:00.499238+00:00", "scope": null, "is_active": true}
      ]
    }"#;

    #[test]
    fn claude_limit_view_stays_connected_without_calling_codex_rate_limits() {
        disable_remote_account_limits();
        let home = fresh_account_home("claude-limit-auth");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join(".credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"claude-token"}}"#,
        )
        .unwrap();
        let mut account = account_for_home(&home);
        account.provider = Provider::Claude;
        account.label = "Claude personnel".to_string();
        let mut settings = empty_settings("codex", Vec::new(), None);
        settings.accounts.push(account.clone());

        let view = account_limit_view(&account, &settings);

        assert_eq!(view.provider, Provider::Claude);
        assert!(view.has_tokens);
        // Lecture distante coupee : le compte reste connecte, sans erreur ni
        // mesure inventee, et surtout sans `codex app-server`.
        assert_eq!(view.source, "authenticated");
        assert!(!view.refreshing);
        assert!(view.error.is_none());
        assert!(view.buckets.is_empty());

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn claude_limit_placeholder_requests_a_background_refresh() {
        let home = fresh_account_home("claude-limit-placeholder");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join(".credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"claude-token"}}"#,
        )
        .unwrap();
        let mut account = account_for_home(&home);
        account.provider = Provider::Claude;

        let view = account_limit_placeholder(&account);

        assert!(view.has_tokens);
        assert!(view.refreshing);
        assert_eq!(view.source, "refreshing");
        assert!(account_limits_need_remote_refresh(&[view]));

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn opencode_limit_placeholder_stays_authenticated() {
        let home = fresh_account_home("opencode-limit-placeholder");
        fs::create_dir_all(home.join("data").join("opencode")).unwrap();
        fs::write(
            home.join("data").join("opencode").join("auth.json"),
            r#"{"anthropic":{"type":"api","key":"token"}}"#,
        )
        .unwrap();
        let mut account = account_for_home(&home);
        account.provider = Provider::OpenCode;
        account.inference_provider = Some("anthropic".to_string());

        let view = account_limit_placeholder(&account);

        assert!(view.has_tokens);
        assert!(!view.refreshing);
        assert_eq!(view.source, "authenticated");
        assert!(!account_limits_need_remote_refresh(&[view]));

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn claude_usage_payload_fills_session_and_weekly_windows() {
        let payload = serde_json::from_str::<Value>(CLAUDE_USAGE_SAMPLE).expect("payload");

        let buckets = claude_usage_buckets(&payload, Some("max"));

        let session = bucket_for_window(&buckets, SESSION_LIMIT_MINS).expect("fenetre 5 h");
        assert_eq!(session.used_percent, Some(61.0));
        assert_eq!(session.resets_at, 1_785_086_400);
        assert_eq!(session.plan_type.as_deref(), Some("max"));
        assert!(session.rate_limit_reached_type.is_none());

        let weekly = bucket_for_window(&buckets, WEEKLY_LIMIT_MINS).expect("fenetre hebdo");
        assert_eq!(weekly.used_percent, Some(66.0));
        assert_eq!(weekly.resets_at, 1_785_355_200);

        // Une seule fenetre par duree : la jauge hebdomadaire ne doit pas etre
        // disputee par les fenetres par modele.
        assert_eq!(buckets.len(), 2);
    }

    #[test]
    fn claude_usage_falls_back_to_the_limits_array() {
        let payload = json!({
            "five_hour": null,
            "seven_day": null,
            "limits": [
                {"kind": "session", "percent": 12, "resets_at": 1_785_086_400_i64},
                {"kind": "weekly_all", "percent": 34, "resets_at": "2026-07-29T20:00:00+00:00"}
            ]
        });

        let buckets = claude_usage_buckets(&payload, None);

        assert_eq!(
            bucket_for_window(&buckets, SESSION_LIMIT_MINS).and_then(|bucket| bucket.used_percent),
            Some(12.0)
        );
        let weekly = bucket_for_window(&buckets, WEEKLY_LIMIT_MINS).expect("fenetre hebdo");
        assert_eq!(weekly.used_percent, Some(34.0));
        assert_eq!(weekly.resets_at, 1_785_355_200);
    }

    #[test]
    fn claude_usage_skips_windows_without_measurement() {
        // Une fenetre absente ne doit pas produire une jauge a 0 %, qui ferait
        // croire a un quota intact.
        let payload = json!({ "five_hour": null, "seven_day": null, "limits": [] });

        assert!(claude_usage_buckets(&payload, None).is_empty());
    }

    #[test]
    fn claude_usage_marks_an_exhausted_window_as_reached() {
        let payload = json!({
            "five_hour": {"utilization": 100.0, "resets_at": "2026-07-26T17:20:00+00:00"}
        });

        let buckets = claude_usage_buckets(&payload, None);
        let session = bucket_for_window(&buckets, SESSION_LIMIT_MINS).expect("fenetre 5 h");

        assert_eq!(
            session.rate_limit_reached_type.as_deref(),
            Some("rate_limit_reached")
        );
    }

    #[test]
    fn claude_credentials_refresh_preserves_unknown_fields() {
        let home = fresh_account_home("claude-credentials-refresh");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join(".credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"old","refreshToken":"old-refresh",
                "expiresAt":1,"subscriptionType":"max","rateLimitTier":"default_claude"},
                "otherProvider":{"token":"keep-me"}}"#,
        )
        .unwrap();

        persist_claude_credentials(
            &home,
            &ClaudeRefreshedTokens {
                access_token: "new".to_string(),
                refresh_token: Some("new-refresh".to_string()),
                expires_at_ms: Some(42),
                refresh_token_expires_at_ms: None,
                scopes: None,
            },
        )
        .expect("ecriture credentials");

        let stored = read_claude_credentials(&home).expect("relecture");
        assert_eq!(stored.access_token, "new");
        assert_eq!(stored.refresh_token.as_deref(), Some("new-refresh"));
        assert_eq!(stored.expires_at_ms, Some(42));
        // Le plan et les cles etrangeres au renouvellement survivent.
        assert_eq!(stored.subscription_type.as_deref(), Some("max"));
        let raw =
            serde_json::from_slice::<Value>(&fs::read(home.join(".credentials.json")).unwrap())
                .unwrap();
        assert_eq!(
            raw.pointer("/otherProvider/token").and_then(Value::as_str),
            Some("keep-me")
        );
        assert_eq!(
            raw.pointer("/claudeAiOauth/rateLimitTier")
                .and_then(Value::as_str),
            Some("default_claude")
        );

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn claude_credentials_expiry_uses_the_refresh_margin() {
        let now = 1_785_000_000;
        let fresh = ClaudeOauthCredentials {
            access_token: "token".to_string(),
            refresh_token: None,
            expires_at_ms: Some((now + 3_600) * 1000),
            subscription_type: None,
        };
        let stale = ClaudeOauthCredentials {
            expires_at_ms: Some((now + 30) * 1000),
            ..fresh.clone()
        };
        let undated = ClaudeOauthCredentials {
            expires_at_ms: None,
            ..fresh.clone()
        };

        assert!(!fresh.is_expired(now));
        assert!(stale.is_expired(now));
        // Sans horodatage, on tente la lecture plutot que de bruler un refresh.
        assert!(!undated.is_expired(now));
    }

    #[test]
    fn codex_limit_placeholder_is_immediate_and_marks_background_refresh() {
        let home = fresh_account_home("codex-limit-placeholder");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("auth.json"),
            r#"{"tokens":{"account_id":"placeholder","access_token":"token"}}"#,
        )
        .unwrap();
        let account = account_for_home(&home);

        let view = account_limit_placeholder(&account);

        assert!(view.has_tokens);
        assert!(view.refreshing);
        assert_eq!(view.source, "refreshing");
        assert!(view.buckets.is_empty());
        assert!(view.error.is_none());

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn rollout_rate_limit_snapshot_parses_codex_snake_case() {
        let home = fresh_account_home("rate-limit-rollout");
        let archive = home
            .join("sessions-archive")
            .join("2026")
            .join("07")
            .join("12");
        fs::create_dir_all(&archive).unwrap();
        let rollout =
            archive.join("rollout-2026-07-12T20-00-00-019f5701-fb46-7503-9abb-004a5316894b.jsonl");
        fs::write(
            &rollout,
            concat!(
                "{\"timestamp\":\"2026-07-12T19:13:41.754Z\",",
                "\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",",
                "\"rate_limits\":{\"limit_id\":\"codex\",\"plan_type\":\"plus\",",
                "\"primary\":{\"used_percent\":54.0,\"window_minutes\":10080,",
                "\"resets_at\":4102444800},\"secondary\":null}}}\n"
            ),
        )
        .unwrap();

        let snapshot = scan_rollout_rate_limit_snapshot(&rollout).expect("quota snapshot");
        assert_eq!(snapshot.buckets.len(), 1);
        assert_eq!(snapshot.buckets[0].window_duration_mins, 10080);
        assert_eq!(snapshot.buckets[0].used_percent, Some(54.0));
        assert_eq!(snapshot.buckets[0].plan_type.as_deref(), Some("plus"));
        assert!(snapshot.observed_at > 0);

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn fallback_snapshot_must_be_newer_than_current_credentials() {
        let home = fresh_account_home("rate-limit-credentials");
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join("auth.json"), "{}").unwrap();
        let account = account_for_home(&home);
        let stale = LocalRateLimitSnapshot {
            buckets: Vec::new(),
            observed_at: 1,
        };
        let current = LocalRateLimitSnapshot {
            buckets: Vec::new(),
            observed_at: now_unix() + 1,
        };

        assert!(!local_snapshot_matches_current_credentials(
            &account, &stale
        ));
        assert!(local_snapshot_matches_current_credentials(
            &account, &current
        ));

        fs::remove_file(home.join("auth.json")).unwrap();
        assert!(local_snapshot_matches_current_credentials(&account, &stale));

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn local_rate_limit_snapshot_prevents_false_zero_regression() {
        let server = vec![AccountRateLimitBucketView {
            limit_id: "codex".to_string(),
            limit_name: None,
            bucket: "primary".to_string(),
            window_duration_mins: 10080,
            resets_at: 5000,
            used_percent: Some(0.0),
            rate_limit_reached_type: None,
            plan_type: Some("plus".to_string()),
        }];
        let local = vec![AccountRateLimitBucketView {
            limit_id: "codex".to_string(),
            limit_name: None,
            // Le backend utilisait auparavant `secondary` pour cette meme
            // fenetre hebdomadaire.
            bucket: "secondary".to_string(),
            window_duration_mins: 10080,
            resets_at: 5001,
            used_percent: Some(54.0),
            rate_limit_reached_type: None,
            plan_type: Some("plus".to_string()),
        }];

        let (merged, used_local) = merge_rate_limit_buckets(server, Some(&local), 1000);

        assert!(used_local);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].bucket, "primary");
        assert_eq!(merged[0].used_percent, Some(54.0));
        assert_eq!(merged[0].resets_at, 5000);
    }

    #[test]
    fn stale_secondary_weekly_bucket_does_not_duplicate_current_primary() {
        let server = vec![AccountRateLimitBucketView {
            limit_id: "codex".to_string(),
            limit_name: None,
            bucket: "primary".to_string(),
            window_duration_mins: 10080,
            resets_at: 1_784_573_804,
            used_percent: Some(100.0),
            rate_limit_reached_type: Some("rate_limit_reached".to_string()),
            plan_type: Some("plus".to_string()),
        }];
        let local = vec![AccountRateLimitBucketView {
            limit_id: "codex".to_string(),
            limit_name: None,
            bucket: "secondary".to_string(),
            window_duration_mins: 10080,
            resets_at: 1_784_462_078,
            used_percent: Some(31.0),
            rate_limit_reached_type: None,
            plan_type: Some("plus".to_string()),
        }];

        let (merged, used_local) = merge_rate_limit_buckets(server, Some(&local), 1_783_976_468);

        assert!(!used_local);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].bucket, "primary");
        assert_eq!(merged[0].used_percent, Some(100.0));
        assert_eq!(merged[0].resets_at, 1_784_573_804);
    }

    #[test]
    fn stale_higher_snapshot_from_another_window_does_not_override_server() {
        let server = vec![AccountRateLimitBucketView {
            limit_id: "codex".to_string(),
            limit_name: None,
            bucket: "primary".to_string(),
            window_duration_mins: 10080,
            resets_at: 700_000,
            used_percent: Some(12.0),
            rate_limit_reached_type: None,
            plan_type: Some("plus".to_string()),
        }];
        let local = vec![AccountRateLimitBucketView {
            limit_id: "codex".to_string(),
            limit_name: None,
            bucket: "primary".to_string(),
            window_duration_mins: 10080,
            resets_at: 600_000,
            used_percent: Some(94.0),
            rate_limit_reached_type: None,
            plan_type: Some("plus".to_string()),
        }];

        let (merged, used_local) = merge_rate_limit_buckets(server, Some(&local), 500_000);

        assert!(!used_local);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].used_percent, Some(12.0));
        assert_eq!(merged[0].resets_at, 700_000);
    }

    #[test]
    fn expired_local_rate_limit_snapshot_does_not_override_server() {
        let server = vec![AccountRateLimitBucketView {
            limit_id: "codex".to_string(),
            limit_name: None,
            bucket: "primary".to_string(),
            window_duration_mins: 10080,
            resets_at: 5000,
            used_percent: Some(0.0),
            rate_limit_reached_type: None,
            plan_type: None,
        }];
        let mut local = server.clone();
        local[0].resets_at = 900;
        local[0].used_percent = Some(99.0);

        let (merged, used_local) = merge_rate_limit_buckets(server, Some(&local), 1000);

        assert!(!used_local);
        assert_eq!(merged[0].used_percent, Some(0.0));
        assert_eq!(merged[0].resets_at, 5000);
    }

    #[test]
    fn model_catalog_preserves_efforts_and_fast_mode_capability() {
        let models = parse_account_model_catalog(&json!({
            "data": [
                {
                    "id": "gpt-5.6-sol",
                    "displayName": "GPT-5.6-Sol",
                    "defaultReasoningEffort": "medium",
                    "supportedReasoningEfforts": [
                        { "reasoningEffort": "low", "description": "Fast" },
                        { "reasoningEffort": "max", "description": "Maximum" },
                        { "reasoningEffort": "ultra", "description": "Delegation" }
                    ],
                    "serviceTiers": [
                        { "id": "priority", "name": "Fast" }
                    ]
                },
                {
                    "id": "gpt-5.4-mini",
                    "additional_speed_tiers": []
                }
            ]
        }));

        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gpt-5.6-sol");
        assert!(models[0].supports_fast_mode);
        assert!(!models[1].supports_fast_mode);
        assert_eq!(
            models[0]
                .supported_reasoning_efforts
                .iter()
                .map(|item| item.reasoning_effort.as_str())
                .collect::<Vec<_>>(),
            vec!["low", "max", "ultra"]
        );
    }

    #[test]
    fn openrouter_api_key_is_read_only_from_the_account_auth_store() {
        let home = fresh_account_home("openrouter-auth");
        let auth_dir = home.join("data").join("opencode");
        fs::create_dir_all(&auth_dir).unwrap();
        fs::write(
            auth_dir.join("auth.json"),
            r#"{"openrouter":{"type":"api","key":"sk-or-v1-backend-only"}}"#,
        )
        .unwrap();

        assert_eq!(
            read_opencode_provider_api_key(&home, "openrouter").unwrap(),
            "sk-or-v1-backend-only"
        );
        let missing = read_opencode_provider_api_key(&home, "another-provider").unwrap_err();
        assert!(!missing.contains("sk-or-v1-backend-only"));

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn openrouter_catalog_preserves_exact_reasoning_metadata_and_aliases() {
        let models = parse_openrouter_model_catalog(
            json!([
                {
                    "id": "vendor/optional-reasoning",
                    "name": "Optional",
                    "reasoning": {
                        "supported_efforts": ["high", "low", "none", "invalid"],
                        "default_effort": "high",
                        "mandatory": false
                    }
                },
                {
                    "id": "vendor/mandatory-reasoning",
                    "reasoning": {
                        "supported_efforts": ["max", "none"],
                        "default_effort": "none",
                        "mandatory": true
                    }
                },
                {
                    "id": "~z-ai/glm-latest",
                    "reasoning": {
                        "supported_efforts": ["medium"],
                        "default_effort": "medium",
                        "default_enabled": false
                    }
                },
                { "id": "vendor/model;whoami" }
            ])
            .as_array()
            .unwrap(),
        );

        assert_eq!(models.len(), 3);
        assert_eq!(models[0].id, "openrouter/vendor/optional-reasoning");
        assert_eq!(models[0].default_reasoning_effort.as_deref(), Some("high"));
        assert_eq!(
            models[0]
                .supported_reasoning_efforts
                .iter()
                .map(|effort| effort.reasoning_effort.as_str())
                .collect::<Vec<_>>(),
            vec!["high", "low", "none"]
        );
        assert_eq!(
            models[1]
                .supported_reasoning_efforts
                .iter()
                .map(|effort| effort.reasoning_effort.as_str())
                .collect::<Vec<_>>(),
            vec!["max"]
        );
        assert!(models[1].default_reasoning_effort.is_none());
        assert_eq!(models[2].id, "openrouter/~z-ai/glm-latest");
        assert!(models[2].default_reasoning_effort.is_none());
    }

    #[test]
    fn openrouter_alias_filter_rejects_shell_metacharacters() {
        assert!(safe_openrouter_model_slug("~z-ai/glm-latest"));
        assert!(safe_openrouter_model_slug("openai/gpt-5.6:online"));
        for hostile in [
            "~~z-ai/glm-latest",
            "z-ai/~glm-latest",
            "z-ai/glm$latest",
            "z-ai/glm`latest`",
            "z-ai/glm\\latest",
            "z-ai/glm\"latest",
            "z-ai/glm'latest",
            "z-ai/glm;latest",
            "z-ai/glm|latest",
            "z-ai/glm&latest",
            "z-ai/glm latest",
        ] {
            assert!(!safe_openrouter_model_slug(hostile), "accepte: {hostile}");
        }
    }

    #[test]
    fn openrouter_pagination_stays_on_the_catalog_endpoint() {
        let endpoint = Url::parse(OPENROUTER_MODELS_USER_URL).unwrap();
        let current = openrouter_page_url(&endpoint, 0);
        let next = openrouter_next_page_url(
            &endpoint,
            &current,
            &json!({ "data": [], "total_count": 2_000 }),
            None,
            1_000,
            1_000,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            next.query_pairs()
                .find(|(name, _)| name == "offset")
                .map(|(_, value)| value.into_owned()),
            Some("1000".to_string())
        );

        let hostile = json!({ "next": "https://attacker.invalid/api/v1/models/user?page=2" });
        assert!(
            openrouter_next_page_url(&endpoint, &current, &hostile, None, 1_000, 1_000,).is_err()
        );
    }
}

pub fn load_settings_for_terminal() -> Result<AppSettings, String> {
    load_settings()
}

fn write_settings(path: &Path, settings: &AppSettings) -> Result<(), String> {
    let content = serde_json::to_string_pretty(settings).map_err(|error| error.to_string())?;
    crate::fs_util::atomic_write(path, content).map_err(|error| error.to_string())
}

fn settings_path() -> Result<PathBuf, String> {
    // Une instance web portable peut partager les comptes/settings avec le
    // noeud principal tout en gardant son runtime (agents, locks, salons)
    // strictement isole dans CST_DATA_DIR.
    if let Some(value) = env::var_os("CST_ACCOUNTS_DIR") {
        return Ok(PathBuf::from(value).join("settings.json"));
    }

    if let Some(value) = env::var_os("CST_DATA_DIR") {
        return Ok(PathBuf::from(value).join("settings.json"));
    }

    let base = if let Some(value) = env::var_os("APPDATA") {
        PathBuf::from(value)
    } else if let Some(value) = env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(value)
    } else {
        home_dir()?.join(".config")
    };

    Ok(base.join("codex-switch-terminal").join("settings.json"))
}

/// Fichier d'etat persistant place a cote de `settings.json`.
///
/// Les runtimes desktop, serveur et portable suivent ainsi exactement les
/// memes variables `CST_ACCOUNTS_DIR` / `CST_DATA_DIR`, sans dupliquer la
/// resolution des repertoires dans chaque moteur de fond.
pub(crate) fn runtime_data_path(file_name: &str) -> Result<PathBuf, String> {
    let parent = settings_path()?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "Repertoire de donnees de l'application introuvable".to_string())?;
    Ok(parent.join(file_name))
}

fn discover_initial_settings() -> Result<AppSettings, String> {
    let mut settings = AppSettings {
        accounts: Vec::new(),
        proxies: Vec::new(),
        default_account_id: None,
        shell: default_shell(),
        codex_command: "codex".to_string(),
        auto_run_codex: true,
        proxy_controls_enabled: true,
        pool: PoolConfig::default(),
        agents: Vec::new(),
        active_agent_id: None,
        kombai: KombaiConfig::default(),
        codex_bypass: true,
        auto_discover_accounts: false,
        expired_unconnected_account_homes: Vec::new(),
        workspaces: Vec::new(),
        closed_workspace_ids: Vec::new(),
        claude_persistent_session: false,
    };

    if env::var_os("CST_DATA_DIR").is_none() {
        merge_discovered_profiles(&mut settings)?;
    }
    ensure_default_account(&mut settings);
    ensure_agents(&mut settings);
    sync_account_limit_trackers(&mut settings);
    Ok(settings)
}

/// Garantit la coherence du registre d'agents :
/// - l'agent Codex integre existe toujours (pilote la lecture des limites via
///   `codex app-server`) ;
/// - au tout premier passage (aucun agent enregistre) on ajoute aussi un agent
///   Kombai pret a l'emploi ;
/// - le champ historique `codex_command` reste synchronise avec la commande de
///   l'agent Codex ;
/// - `active_agent_id` pointe toujours vers un agent existant.
fn ensure_agents(settings: &mut AppSettings) -> bool {
    let mut changed = false;

    let codex_command = {
        let trimmed = settings.codex_command.trim();
        if trimmed.is_empty() {
            "codex".to_string()
        } else {
            trimmed.to_string()
        }
    };

    if !settings
        .agents
        .iter()
        .any(|agent| agent.id == CODEX_AGENT_ID)
    {
        settings.agents.insert(
            0,
            AgentProfile {
                id: CODEX_AGENT_ID.to_string(),
                label: "Codex".to_string(),
                command: codex_command,
                provider: Provider::Codex,
                kind: "cli".to_string(),
                builtin: true,
                login_command: Some("login".to_string()),
                status_command: Some("login status".to_string()),
                doctor_command: Some("doctor --summary --ascii".to_string()),
            },
        );
        changed = true;
    }

    // OpenCode porte tous les fournisseurs API annexes. Un seul agent CLI est
    // partage ; les credentials et le catalogue restent isoles par compte via
    // les variables XDG configurees dans `provider.rs`.
    if !settings
        .agents
        .iter()
        .any(|agent| agent.id == OPENCODE_AGENT_ID)
    {
        settings.agents.push(AgentProfile {
            id: OPENCODE_AGENT_ID.to_string(),
            label: "OpenCode".to_string(),
            command: "opencode".to_string(),
            provider: Provider::OpenCode,
            kind: "cli".to_string(),
            builtin: true,
            login_command: Some("auth login".to_string()),
            status_command: Some("auth list".to_string()),
            doctor_command: None,
        });
        changed = true;
    }

    // Agent CLI freebuff. Son binaire n'expose que deux entrees : `login` pour
    // s'authentifier et `--continue` pour reprendre une conversation. Ni
    // `status` ni `doctor` n'existent, d'ou les champs laisses vides.
    if !settings
        .agents
        .iter()
        .any(|agent| agent.id == FREEBUFF_AGENT_ID)
    {
        settings.agents.push(AgentProfile {
            id: FREEBUFF_AGENT_ID.to_string(),
            label: "Freebuff".to_string(),
            command: "freebuff".to_string(),
            provider: Provider::Freebuff,
            kind: "cli".to_string(),
            builtin: true,
            login_command: Some("login".to_string()),
            status_command: None,
            doctor_command: None,
        });
        changed = true;
    }

    // Freebuff Desktop : application externe ouverte sur le dossier projet
    // (`kind = "ide"`, comme Kombai). C'est le seul canal ou le mode agent MAX
    // est disponible, le CLI etant fige sur LITE par un flag compile dans son
    // binaire. `%LOCALAPPDATA%` est resolu au lancement par cmd.exe, que
    // `terminal::ide_command` utilise deja : le chemin reste donc portable.
    if !settings
        .agents
        .iter()
        .any(|agent| agent.id == FREEBUFF_DESKTOP_AGENT_ID)
    {
        settings.agents.push(AgentProfile {
            id: FREEBUFF_DESKTOP_AGENT_ID.to_string(),
            label: "Freebuff Desktop".to_string(),
            command: FREEBUFF_DESKTOP_COMMAND.to_string(),
            provider: Provider::Freebuff,
            kind: "ide".to_string(),
            builtin: true,
            login_command: None,
            status_command: None,
            doctor_command: None,
        });
        changed = true;
    }

    // Agent Claude Code integre. Comme l'agent Codex, il est (re)cree s'il
    // manque : Claude Code est un fournisseur pris en charge de premier rang.
    // Les versions recentes du CLI exposent l'authentification via
    // `claude auth login` et son controle via `claude auth status`.
    if !settings
        .agents
        .iter()
        .any(|agent| agent.id == CLAUDE_AGENT_ID)
    {
        settings.agents.push(AgentProfile {
            id: CLAUDE_AGENT_ID.to_string(),
            label: "Claude Code".to_string(),
            command: "claude".to_string(),
            provider: Provider::Claude,
            kind: "cli".to_string(),
            builtin: true,
            login_command: Some("auth login".to_string()),
            status_command: Some("auth status".to_string()),
            doctor_command: Some("doctor".to_string()),
        });
        changed = true;
    }

    // Les versions qui ont introduit Claude Code ont pu persister l'agent
    // integre avant d'ajouter ses sous-commandes d'authentification. Repare
    // uniquement les champs vides de l'agent builtin afin de conserver toute
    // personnalisation explicite d'un utilisateur.
    if let Some(agent) = settings
        .agents
        .iter_mut()
        .find(|agent| agent.id == CLAUDE_AGENT_ID && agent.builtin)
    {
        if agent
            .login_command
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
        {
            agent.login_command = Some("auth login".to_string());
            changed = true;
        }
        if agent
            .status_command
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
        {
            agent.status_command = Some("auth status".to_string());
            changed = true;
        }
        if agent
            .doctor_command
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
        {
            agent.doctor_command = Some("doctor".to_string());
            changed = true;
        }
    }

    // Kombai n'est PAS un agent terminal : il vit dans l'onglet "Kombai"
    // (VS Code embarque). On retire un agent Kombai seed par defaut lors de
    // versions precedentes, tant que l'utilisateur ne l'a pas personnalise.
    let before = settings.agents.len();
    settings.agents.retain(|agent| {
        !(!agent.builtin
            && agent.id == KOMBAI_AGENT_ID
            && matches!(agent.command.trim(), "" | "kombai" | "code"))
    });
    if settings.agents.len() != before {
        changed = true;
    }

    // Synchronise le champ historique `codex_command` avec l'agent Codex.
    let codex_agent_command = settings
        .agents
        .iter()
        .find(|agent| agent.id == CODEX_AGENT_ID)
        .map(|agent| agent.command.trim().to_string());
    if let Some(command) = codex_agent_command {
        let synced = if command.is_empty() {
            "codex".to_string()
        } else {
            command
        };
        if settings.codex_command != synced {
            settings.codex_command = synced;
            changed = true;
        }
    }

    // `active_agent_id` doit referencer un agent existant.
    let active_valid = match &settings.active_agent_id {
        Some(id) => settings.agents.iter().any(|agent| &agent.id == id),
        None => false,
    };
    if !active_valid {
        let fallback = settings
            .agents
            .iter()
            .find(|agent| agent.id == CODEX_AGENT_ID)
            .or_else(|| settings.agents.first())
            .map(|agent| agent.id.clone());
        if settings.active_agent_id != fallback {
            settings.active_agent_id = fallback;
            changed = true;
        }
    }

    changed
}

/// Normalise un chemin de workspace pour en faire une identite stable. Doit
/// rester alignee sur `normalizeWorkspacePath` cote front (src/main.ts) : trim,
/// retrait des slashes finaux, `\\` -> `/`, puis minuscule UNIQUEMENT pour les
/// chemins Windows (`X:/...`) et UNC (`//...`), insensibles a la casse.
pub(crate) fn normalize_workspace_path(path: &str) -> String {
    let trimmed = path.trim().trim_end_matches(['\\', '/']).replace('\\', "/");
    let bytes = trimmed.as_bytes();
    let is_windows_drive =
        bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/';
    if is_windows_drive || trimmed.starts_with("//") {
        trimmed.to_lowercase()
    } else {
        trimmed
    }
}

/// Dernier segment d'un chemin (nom du dossier, sans suffixe `.git`), pour un
/// libelle par defaut. Aligne sur `workspaceBaseName` cote front.
fn workspace_base_name(path: &str) -> String {
    let cleaned = path.trim_end_matches(['\\', '/']);
    let base = cleaned.rsplit(['\\', '/']).next().unwrap_or(cleaned);
    // Retire un suffixe `.git` (insensible a la casse) SANS indexer le `str` par
    // octet : `str::get` renvoie None si la borne n'est pas une frontiere de
    // caractere, ce qui evite tout panic sur un nom de dossier multi-octets
    // (ex. « Éire », « 😀x »). On ne retire pas `.git` s'il constitue tout le
    // segment (dossier litteralement nomme `.git`).
    let without_git = match base.len() {
        n if n > 4 => match base.get(n - 4..) {
            Some(tail) if tail.eq_ignore_ascii_case(".git") => &base[..n - 4],
            _ => base,
        },
        _ => base,
    };
    if without_git.is_empty() {
        path.trim().to_string()
    } else {
        without_git.to_string()
    }
}

fn normalize_workspace_memory(memory: &str) -> String {
    memory
        .trim()
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\r' | '\t'))
        .take(MAX_WORKSPACE_MEMORY_CHARS)
        .collect()
}

fn normalize_workspace_execution_target_id(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .map(|target| target.trim_end_matches('/'))
        .filter(|target| !target.is_empty())
        .map(str::to_ascii_lowercase)
}

/// Retourne uniquement la memoire du dossier exact. Une conversation ouverte
/// dans un autre environnement ne peut donc jamais recevoir ce contexte.
pub(crate) fn workspace_memory_for_path<'a>(
    settings: &'a AppSettings,
    path: &Path,
) -> Option<&'a str> {
    let id = normalize_workspace_path(path.to_string_lossy().as_ref());
    settings
        .workspaces
        .iter()
        .find(|workspace| normalize_workspace_path(&workspace.path) == id)
        .map(|workspace| workspace.memory.trim())
        .filter(|memory| !memory.is_empty())
}

/// Garantit la coherence du registre de workspaces :
/// - retire les entrees a chemin vide ;
/// - recalcule un `id` manquant/incoherent depuis le chemin normalise ;
/// - deduplique par `id` (premiere occurrence gardee) ;
/// - complete un `label` vide par le nom du dossier ;
/// - normalise les tombstones et leur donne priorite sur le registre ouvert.
fn ensure_workspaces(settings: &mut AppSettings) -> bool {
    let mut changed = false;
    let mut closed_seen: HashSet<String> = HashSet::new();
    let mut closed_ids: Vec<String> = Vec::with_capacity(settings.closed_workspace_ids.len());
    for raw_id in &settings.closed_workspace_ids {
        let id = normalize_workspace_path(raw_id);
        if id.is_empty() || !closed_seen.insert(id.clone()) {
            changed = true;
            continue;
        }
        if raw_id != &id {
            changed = true;
        }
        closed_ids.push(id);
    }
    if closed_ids != settings.closed_workspace_ids {
        changed = true;
    }
    settings.closed_workspace_ids = closed_ids;

    let mut seen: HashSet<String> = HashSet::new();
    let mut deduped: Vec<WorkspaceProfile> = Vec::with_capacity(settings.workspaces.len());

    for mut ws in std::mem::take(&mut settings.workspaces) {
        let path = ws.path.trim().to_string();
        if path.is_empty() {
            changed = true;
            continue;
        }
        // Le chemin est la source de verite. Un ancien id non vide mais
        // incoherent ne doit jamais permettre a deux chemins equivalents de
        // survivre comme deux workspaces distincts.
        let id = normalize_workspace_path(&path);
        let memory = normalize_workspace_memory(&ws.memory);
        let execution_target_id =
            normalize_workspace_execution_target_id(ws.execution_target_id.as_deref());
        if closed_seen.contains(&id) {
            changed = true;
            continue;
        }
        if !seen.insert(id.clone()) {
            if let Some(existing) = deduped.iter_mut().find(|workspace| workspace.id == id) {
                if existing.memory.is_empty() && !memory.is_empty() {
                    existing.memory = memory;
                }
                if existing.execution_target_id.is_none() && execution_target_id.is_some() {
                    existing.execution_target_id = execution_target_id;
                }
            }
            changed = true;
            continue;
        }
        let label = if ws.label.trim().is_empty() {
            workspace_base_name(&path)
        } else {
            ws.label.trim().to_string()
        };
        if ws.id != id
            || ws.label != label
            || ws.path != path
            || ws.memory != memory
            || ws.execution_target_id != execution_target_id
        {
            changed = true;
        }
        ws.id = id;
        ws.label = label;
        ws.path = path;
        ws.memory = memory;
        ws.execution_target_id = execution_target_id;
        deduped.push(ws);
    }

    settings.workspaces = deduped;

    changed
}

/// Un home d'authentification ne peut representer qu'un seul compte. Les
/// anciennes versions comparaient les chaines brutes et pouvaient donc garder
/// deux profils pour `%USERPROFILE%\\.codex-x` et
/// `C:\\Users\\...\\.codex-x`, alors qu'ils ciblent le meme dossier.
///
/// La premiere occurrence est conservee, sauf si une occurrence ulterieure est
/// le compte par defaut : dans ce cas son identifiant reste la reference
/// canonique. Les quelques metadonnees absentes sont completees depuis le
/// profil retire ; aucun fichier du home n'est supprime.
fn deduplicate_accounts_by_home(settings: &mut AppSettings) -> bool {
    if settings.accounts.len() < 2 {
        return false;
    }

    let default_id = settings.default_account_id.as_deref();
    let mut unique = Vec::<AccountProfile>::with_capacity(settings.accounts.len());
    let mut home_keys = Vec::<String>::with_capacity(settings.accounts.len());
    let mut changed = false;

    for account in std::mem::take(&mut settings.accounts) {
        let home_key = account_home_key(&account.codex_home);
        // Un home vide est invalide, mais ne doit pas provoquer la suppression
        // silencieuse de tous les profils invalides lors de leur migration.
        if home_key.is_empty() {
            home_keys.push(home_key);
            unique.push(account);
            continue;
        }

        let Some(existing_index) = home_keys.iter().position(|key| key == &home_key) else {
            home_keys.push(home_key);
            unique.push(account);
            continue;
        };

        changed = true;
        let current_is_default = default_id == Some(account.id.as_str());
        let existing_is_default = default_id == Some(unique[existing_index].id.as_str());
        if current_is_default && !existing_is_default {
            let mut replacement = account;
            merge_missing_account_metadata(&mut replacement, &unique[existing_index]);
            unique[existing_index] = replacement;
        } else {
            merge_missing_account_metadata(&mut unique[existing_index], &account);
        }
    }

    settings.accounts = unique;
    changed
}

fn merge_missing_account_metadata(target: &mut AccountProfile, source: &AccountProfile) {
    if target.label.trim().is_empty() && !source.label.trim().is_empty() {
        target.label = source.label.clone();
    }
    if target.created_at.is_none() {
        target.created_at = source.created_at;
    }
    if target.inference_provider.is_none() {
        target.inference_provider = source.inference_provider.clone();
    }
    if target.project_dir.is_none() {
        target.project_dir = source.project_dir.clone();
    }
    if target.proxy_id.is_none() {
        target.proxy_id = source.proxy_id.clone();
    }
    if target.startup_command.is_none() {
        target.startup_command = source.startup_command.clone();
    }
    if target.model.is_none() {
        target.model = source.model.clone();
    }
    if target.reasoning_effort.is_none() {
        target.reasoning_effort = source.reasoning_effort.clone();
    }
    target.fast_mode |= source.fast_mode;
    if target.completed_on.is_none() {
        target.completed_on = source.completed_on.clone();
    }
    if target.limits.connected_at.is_none() {
        target.limits.connected_at = source.limits.connected_at;
    }
    if target.limits.session_anchor_at.is_none() {
        target.limits.session_anchor_at = source.limits.session_anchor_at;
    }
    if target.limits.weekly_anchor_at.is_none() {
        target.limits.weekly_anchor_at = source.limits.weekly_anchor_at;
    }
}

fn merge_discovered_profiles(settings: &mut AppSettings) -> Result<bool, String> {
    let home = home_dir()?;
    if !home.exists() {
        return Ok(false);
    }
    let mut changed = false;

    let mut account_paths = settings
        .accounts
        .iter()
        .map(|account| account_home_key(&account.codex_home))
        .collect::<HashSet<_>>();
    let mut expired_homes = settings
        .expired_unconnected_account_homes
        .iter()
        .map(|home| account_home_key(home))
        .collect::<HashSet<_>>();
    let mut proxy_urls = settings
        .proxies
        .iter()
        .map(|proxy| proxy.proxy_url.trim().to_string())
        .collect::<HashSet<_>>();

    for entry in fs::read_dir(&home).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };

        if !is_codex_like_dir(name) {
            continue;
        }

        let path_string = path.to_string_lossy().to_string();
        let normalized = account_home_key(&path_string);
        let has_auth = path.join("auth.json").is_file();
        let has_config = path.join("config.toml").is_file();
        if expired_homes.contains(&normalized) {
            if Provider::Codex.has_auth(&path, None) {
                settings
                    .expired_unconnected_account_homes
                    .retain(|home| account_home_key(home) != normalized);
                expired_homes.remove(&normalized);
                changed = true;
            } else {
                continue;
            }
        }
        let proxy_file = path.join("proxy.txt");
        let proxy_id = if proxy_file.is_file() {
            let proxy_url = fs::read_to_string(&proxy_file)
                .map_err(|error| error.to_string())?
                .trim()
                .to_string();
            if proxy_url.is_empty() {
                None
            } else if let Some(existing) = settings
                .proxies
                .iter()
                .find(|proxy| proxy.proxy_url.trim() == proxy_url)
            {
                Some(existing.id.clone())
            } else {
                let id = stable_id("proxy", &path_string);
                if !proxy_urls.contains(&proxy_url) {
                    settings.proxies.push(ProxyProfile {
                        id: id.clone(),
                        label: format!("Proxy {}", name.trim_start_matches('.')),
                        proxy_url: proxy_url.clone(),
                        note: Some(path_string.clone()),
                    });
                    proxy_urls.insert(proxy_url);
                    changed = true;
                }
                Some(id)
            }
        } else {
            None
        };

        if (has_auth || has_config || proxy_id.is_some()) && !account_paths.contains(&normalized) {
            let id = stable_id("account", &path_string);
            // Compte auto-decouvert : herite du defaut global "Bypass defaut"
            // (comme la creation via l'UI), pas un `true` code en dur.
            let bypass_default = settings.codex_bypass;
            settings.accounts.push(AccountProfile {
                id,
                label: label_from_codex_dir(name),
                // Une decouverte nouvelle est bien un ajout de compte : si ce
                // home ne contient encore aucun credential valide, il dispose
                // du meme delai de dix minutes qu'une creation dans l'UI.
                created_at: Some(now_unix()),
                provider: Provider::Codex,
                inference_provider: None,
                codex_home: path_string.clone(),
                project_dir: None,
                proxy_id,
                startup_command: None,
                limits: AccountLimitTracking::default(),
                bypass: bypass_default,
                // Ce CODEX_HOME existait deja avant sa decouverte : ne pas
                // ecraser un modele/effort potentiellement defini dans son
                // config.toml. L'utilisateur pourra les choisir dans l'UI.
                model: None,
                reasoning_effort: None,
                fast_mode: false,
                completed_on: None,
                api_key: None,
            });
            account_paths.insert(normalized);
            changed = true;
        }
    }

    Ok(changed)
}

fn ensure_default_account(settings: &mut AppSettings) {
    if settings.default_account_id.is_none() {
        settings.default_account_id = settings.accounts.first().map(|account| account.id.clone());
    }
}

fn is_codex_like_dir(name: &str) -> bool {
    name == ".codex" || name.starts_with(".codex-") || name.starts_with(".codex_")
}

fn label_from_codex_dir(name: &str) -> String {
    match name {
        ".codex" => "Principal".to_string(),
        other => other
            .trim_start_matches('.')
            .replace(['-', '_'], " ")
            .split_whitespace()
            .map(capitalize)
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn stable_id(prefix: &str, input: &str) -> String {
    let mut value = String::with_capacity(prefix.len() + input.len() + 1);
    value.push_str(prefix);
    value.push('-');

    for character in input.chars() {
        if character.is_ascii_alphanumeric() {
            value.push(character.to_ascii_lowercase());
        } else if !value.ends_with('-') {
            value.push('-');
        }
    }

    value.trim_end_matches('-').chars().take(96).collect()
}

pub fn account_has_auth_tokens(account: &AccountProfile) -> bool {
    let Ok(home) = expand_home(&account.codex_home) else {
        return false;
    };
    // Detection des credentials propre au provider (Codex: auth.json ;
    // Claude: .credentials.json). Voir `provider::Provider::has_auth`.
    account
        .provider
        .has_auth(&home, account.inference_provider.as_deref())
}

/// Commande CLI a utiliser pour lancer/piloter un `provider` : commande de
/// l'agent integre correspondant (Codex ou Claude Code), avec repli sur la
/// commande par defaut du provider si l'agent a ete retire du registre.
pub fn command_for_provider(settings: &AppSettings, provider: Provider) -> String {
    settings
        .agents
        .iter()
        .find(|agent| agent.provider == provider && agent.builtin)
        .map(|agent| agent.command.trim().to_string())
        .filter(|command| !command.is_empty())
        .unwrap_or_else(|| match provider {
            Provider::Codex => "codex".to_string(),
            Provider::Claude => "claude".to_string(),
            Provider::OpenCode => "opencode".to_string(),
            Provider::Freebuff => "freebuff".to_string(),
            Provider::Aihubmix => "aihubmix".to_string(),
        })
}

/// Retire uniquement le profil des comptes crees depuis au moins dix minutes
/// qui n'ont encore jamais obtenu de credentials. Le dossier du compte reste
/// sur le disque : une expiration automatique ne doit jamais effacer des
/// fichiers utilisateur. Les profils historiques (`created_at` absent) et les
/// comptes qui se sont connectes au moins une fois sont toujours preserves.
fn remove_expired_unconnected_accounts(settings: &mut AppSettings, now: i64) -> bool {
    let tombstoned_homes = settings
        .expired_unconnected_account_homes
        .iter()
        .map(|home| account_home_key(home))
        .collect::<HashSet<_>>();
    let expired = settings
        .accounts
        .iter()
        .filter(|account| {
            if account.limits.connected_at.is_some() || account_has_auth_tokens(account) {
                return false;
            }
            match account.created_at {
                Some(created_at) => {
                    now.saturating_sub(created_at) >= UNCONNECTED_ACCOUNT_EXPIRY_SECS
                }
                // Un ancien frontend peut tenter de sauvegarder a nouveau un
                // profil deja expire sans connaitre `createdAt`. Le tombstone
                // du home doit alors primer sur cette copie obsolete.
                None => tombstoned_homes.contains(&account_home_key(&account.codex_home)),
            }
        })
        .map(|account| (account.id.clone(), account_home_key(&account.codex_home)))
        .collect::<Vec<_>>();

    if expired.is_empty() {
        return false;
    }

    let expired_ids = expired
        .iter()
        .map(|(id, _)| id.as_str())
        .collect::<HashSet<_>>();
    settings
        .accounts
        .retain(|account| !expired_ids.contains(account.id.as_str()));
    for (_, home) in expired {
        if !settings
            .expired_unconnected_account_homes
            .iter()
            .any(|existing| normalize_string_path(existing) == home)
        {
            settings.expired_unconnected_account_homes.push(home);
        }
    }

    let default_still_exists = settings
        .default_account_id
        .as_deref()
        .is_some_and(|id| settings.accounts.iter().any(|account| account.id == id));
    if !default_still_exists {
        settings.default_account_id = settings.accounts.first().map(|account| account.id.clone());
    }
    true
}

/// Une creation explicite reutilisant un ancien home reprend un nouveau delai
/// normal. On retire donc son exclusion, mais seulement apres avoir purge les
/// profils deja expires afin qu'une sauvegarde cliente obsolete ne les ressuscite
/// pas.
fn clear_expired_home_tombstones_for_registered_accounts(settings: &mut AppSettings) -> bool {
    if settings.expired_unconnected_account_homes.is_empty() {
        return false;
    }
    let registered_homes = settings
        .accounts
        .iter()
        .map(|account| account_home_key(&account.codex_home))
        .collect::<HashSet<_>>();
    let previous_len = settings.expired_unconnected_account_homes.len();
    settings
        .expired_unconnected_account_homes
        .retain(|home| !registered_homes.contains(&account_home_key(home)));
    settings.expired_unconnected_account_homes.len() != previous_len
}

fn sync_account_limit_trackers(settings: &mut AppSettings) -> bool {
    let now = now_unix();
    let mut changed = false;

    for account in &mut settings.accounts {
        if account_has_auth_tokens(account) && touch_account_limits(&mut account.limits, now) {
            changed = true;
        }
    }

    changed
}

fn touch_account_limits(limits: &mut AccountLimitTracking, now: i64) -> bool {
    let mut changed = false;

    if limits.connected_at.is_none() {
        limits.connected_at = Some(now);
        changed = true;
    }
    if limits.session_anchor_at.is_none() {
        limits.session_anchor_at = limits.connected_at.or(Some(now));
        changed = true;
    }
    if limits.weekly_anchor_at.is_none() {
        limits.weekly_anchor_at = limits.connected_at.or(Some(now));
        changed = true;
    }

    changed
}

fn new_connected_limits(now: i64) -> AccountLimitTracking {
    AccountLimitTracking {
        connected_at: Some(now),
        session_anchor_at: Some(now),
        weekly_anchor_at: Some(now),
    }
}

/// Cache stale-while-revalidate des quotas. Demarrer un app-server par compte
/// prend plusieurs secondes ; l'interface recoit donc immediatement le dernier
/// snapshot (ou un placeholder connecte au premier passage), tandis qu'un seul
/// rafraichissement reconstruit les lignes en arriere-plan.
#[derive(Clone)]
struct AccountLimitCache {
    signature: String,
    rows: Vec<AccountLimitView>,
    completed_at: Option<Instant>,
    refreshing: bool,
}

static ACCOUNT_LIMIT_CACHE: OnceLock<Mutex<Option<AccountLimitCache>>> = OnceLock::new();

fn account_limit_cache() -> &'static Mutex<Option<AccountLimitCache>> {
    ACCOUNT_LIMIT_CACHE.get_or_init(|| Mutex::new(None))
}

fn lock_account_limit_cache() -> std::sync::MutexGuard<'static, Option<AccountLimitCache>> {
    account_limit_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn account_limit_cache_signature(settings: &AppSettings) -> String {
    let mut signature = serde_json::to_string(&(
        &settings.accounts,
        &settings.proxies,
        settings.proxy_controls_enabled,
        &settings.codex_command,
    ))
    .unwrap_or_default();
    for account in &settings.accounts {
        signature.push('|');
        signature.push_str(&account.id);
        signature.push(':');
        signature.push(if account_has_auth_tokens(account) {
            '1'
        } else {
            '0'
        });
    }
    signature
}

/// Vrai si le CLI du compte occupe deja son home. Seul freebuff impose cette
/// exclusivite ; les autres providers acceptent plusieurs sessions par compte,
/// donc la reponse est constante pour eux.
fn account_session_busy(account: &AccountProfile) -> bool {
    if account.provider != Provider::Freebuff {
        return false;
    }
    expand_home(&account.codex_home)
        .map(|home| crate::provider::freebuff_instance_busy(&home))
        .unwrap_or(false)
}

fn account_limit_placeholder(account: &AccountProfile) -> AccountLimitView {
    let now = now_unix();
    let has_tokens = account_has_auth_tokens(account);
    let refreshing = has_tokens && provider_reads_remote_limits(account.provider);
    let authenticated_without_remote = has_tokens && !refreshing;

    AccountLimitView {
        id: account.id.clone(),
        label: account.label.clone(),
        provider: account.provider,
        codex_home: account.codex_home.clone(),
        has_tokens,
        connected_at: account.limits.connected_at,
        session_reset_at: None,
        weekly_reset_at: None,
        session_remaining_secs: None,
        weekly_remaining_secs: None,
        session_used_percent: None,
        weekly_used_percent: None,
        buckets: Vec::new(),
        refreshed_at: authenticated_without_remote.then_some(now),
        source: if authenticated_without_remote {
            "authenticated"
        } else if refreshing {
            "refreshing"
        } else {
            "none"
        }
        .to_string(),
        refreshing,
        error: None,
        session_busy: account_session_busy(account),
    }
}

fn prepare_cached_limit_rows(rows: &mut [AccountLimitView], refreshing: bool) {
    let now = now_unix();
    for row in rows {
        row.session_remaining_secs = row.session_reset_at.map(|value| (value - now).max(0));
        row.weekly_remaining_secs = row.weekly_reset_at.map(|value| (value - now).max(0));
        row.refreshing = refreshing && provider_reads_remote_limits(row.provider) && row.has_tokens;
        // Les quotas distants peuvent rester caches, mais l'occupation Freebuff
        // est un etat local et instantane. La recalculer a chaque lecture evite
        // qu'un selecteur affiche « disponible » alors qu'un terminal vient de
        // prendre le verrou de ce home.
        if row.provider == Provider::Freebuff {
            row.session_busy = expand_home(&row.codex_home)
                .map(|home| crate::provider::freebuff_instance_busy(&home))
                .unwrap_or(false);
        }
    }
}

fn account_limits_need_remote_refresh(rows: &[AccountLimitView]) -> bool {
    rows.iter()
        .any(|row| provider_reads_remote_limits(row.provider) && row.has_tokens)
}

fn store_account_limit_cache(signature: String, mut rows: Vec<AccountLimitView>) {
    prepare_cached_limit_rows(&mut rows, false);
    let mut cache = lock_account_limit_cache();
    *cache = Some(AccountLimitCache {
        signature,
        rows,
        completed_at: Some(Instant::now()),
        refreshing: false,
    });
}

fn spawn_account_limit_refresh(settings: AppSettings, signature: String) {
    thread::spawn(move || {
        let refreshed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            account_limit_views_uncached(&settings)
        }))
        .ok();
        let mut cache = lock_account_limit_cache();
        let Some(current) = cache
            .as_mut()
            .filter(|current| current.signature == signature)
        else {
            return;
        };
        if let Some(mut rows) = refreshed {
            prepare_cached_limit_rows(&mut rows, false);
            current.rows = rows;
            current.completed_at = Some(Instant::now());
        }
        current.refreshing = false;
    });
}

fn account_limit_views_fast(settings: &AppSettings) -> Vec<AccountLimitView> {
    let signature = account_limit_cache_signature(settings);
    let now = Instant::now();
    let (mut rows, refreshing, spawn_refresh) = {
        let mut cache = lock_account_limit_cache();
        match cache
            .as_mut()
            .filter(|current| current.signature == signature)
        {
            Some(current) => {
                let fresh = current.completed_at.is_some_and(|completed_at| {
                    now.duration_since(completed_at)
                        < Duration::from_secs(RATE_LIMIT_CACHE_TTL_SECS)
                });
                if fresh {
                    (current.rows.clone(), false, false)
                } else if current.refreshing {
                    (current.rows.clone(), true, false)
                } else {
                    let needs_refresh = account_limits_need_remote_refresh(&current.rows);
                    current.refreshing = needs_refresh;
                    if !needs_refresh {
                        current.completed_at = Some(now);
                    }
                    (current.rows.clone(), needs_refresh, needs_refresh)
                }
            }
            None => {
                let rows = settings
                    .accounts
                    .iter()
                    .map(account_limit_placeholder)
                    .collect::<Vec<_>>();
                let needs_refresh = account_limits_need_remote_refresh(&rows);
                *cache = Some(AccountLimitCache {
                    signature: signature.clone(),
                    rows: rows.clone(),
                    completed_at: (!needs_refresh).then_some(now),
                    refreshing: needs_refresh,
                });
                (rows, needs_refresh, needs_refresh)
            }
        }
    };

    if spawn_refresh {
        spawn_account_limit_refresh(settings.clone(), signature);
    }
    prepare_cached_limit_rows(&mut rows, refreshing);
    rows
}

/// Duree pendant laquelle un scan force reutilise le dernier scan complet.
///
/// Les declencheurs UI tirent plusieurs scans forces par minute (ouverture de
/// la modale nouveau chat, retours de connexion, vue Limites) alors qu'un scan
/// complet demarre un app-server par compte Codex et prend de 6 a 17 secondes
/// sur le poste local. Les quotas serveur bougent trop lentement pour justifier
/// ce cout : dans la fenetre TTL on sert le dernier instantane, l'occupation
/// Freebuff restant recalculee a chaque lecture (etat local instantane, voir
/// prepare_cached_limit_rows).
const FORCED_LIMITS_SCAN_TTL_SECS: u64 = 45;

/// Dernier scan complet s'il date de moins de FORCED_LIMITS_SCAN_TTL_SECS.
fn fresh_forced_limit_rows(signature: &str) -> Option<Vec<AccountLimitView>> {
    let mut cache = lock_account_limit_cache();
    let current = cache
        .as_mut()
        .filter(|current| current.signature == signature)?;
    current.completed_at.filter(|completed_at| {
        completed_at.elapsed() < Duration::from_secs(FORCED_LIMITS_SCAN_TTL_SECS)
    })?;
    let mut rows = current.rows.clone();
    drop(cache);
    prepare_cached_limit_rows(&mut rows, false);
    Some(rows)
}

pub(crate) fn account_limit_views(settings: &AppSettings) -> Vec<AccountLimitView> {
    let signature = account_limit_cache_signature(settings);
    if let Some(rows) = fresh_forced_limit_rows(&signature) {
        return rows;
    }
    let rows = account_limit_views_uncached(settings);
    store_account_limit_cache(signature, rows.clone());
    rows
}

fn account_limit_views_uncached(settings: &AppSettings) -> Vec<AccountLimitView> {
    let accounts = settings.accounts.clone();
    let handles = accounts
        .into_iter()
        .map(|account| {
            let settings = settings.clone();
            thread::spawn(move || account_limit_view(&account, &settings))
        })
        .collect::<Vec<_>>();

    handles
        .into_iter()
        .filter_map(|handle| handle.join().ok())
        .collect()
}

fn account_limit_view(account: &AccountProfile, settings: &AppSettings) -> AccountLimitView {
    let now = now_unix();
    let has_tokens = account_has_auth_tokens(account);
    let mut buckets = Vec::new();
    let mut refreshed_at = None;
    let mut error = None;
    let mut source = "none";

    if has_tokens && account.provider == Provider::Claude {
        // Claude Code ne fournit pas l'equivalent de `account/rateLimits/read` :
        // ses quotas viennent de l'endpoint OAuth `/api/oauth/usage`, jamais du
        // `codex app-server`, qui serait lance avec un home Claude.
        match read_claude_rate_limits(account, settings) {
            Ok(claude_buckets) if !claude_buckets.is_empty() => {
                buckets = claude_buckets;
                refreshed_at = Some(now);
                source = "claude-usage";
            }
            Ok(_) => {
                // Session valide mais aucune fenetre publiee (cle API, plan sans
                // quota d'abonnement, lecture reseau desactivee).
                refreshed_at = Some(now);
                source = "authenticated";
            }
            Err(message) => {
                // Une panne reseau ne doit pas faire passer un compte connecte
                // pour un compte deconnecte : seule la mesure manque. L'erreur
                // reste exposee pour que l'UI propose une reconnexion quand le
                // token est reellement revoque.
                refreshed_at = Some(now);
                source = "authenticated";
                error = Some(message);
            }
        }
    } else if has_tokens && account.provider == Provider::OpenCode {
        // Les fournisseurs pilotes par OpenCode n'exposent pas un format de
        // quotas commun. La presence du credential suffit a marquer le compte
        // connecte, sans appeler le serveur de quotas Codex.
        refreshed_at = Some(now);
        source = "authenticated";
    } else if has_tokens && account.provider == Provider::Freebuff {
        // freebuff n'annonce ses limites (sessions par jour ou par semaine,
        // plafond de depense) que dans son propre TUI, au demarrage d'une
        // session : son CLI n'expose aucune sous-commande de statut, donc
        // rien n'est interrogeable depuis Switch.
        //
        // Sans ce bras, un compte freebuff authentifie tombait dans le
        // chemin Codex ci-dessous, qui lance le `codex app-server` sur un
        // home etranger et faisait echouer la lecture d'un compte pourtant
        // valide.
        refreshed_at = Some(now);
        source = "authenticated";
    } else if has_tokens {
        let local_snapshot = read_local_rate_limit_snapshot(account).ok().flatten();
        match read_server_rate_limits(account, settings) {
            Ok(server_buckets) => {
                let (merged, used_local_snapshot) = merge_rate_limit_buckets(
                    server_buckets,
                    local_snapshot
                        .as_ref()
                        .map(|snapshot| snapshot.buckets.as_slice()),
                    now,
                );
                buckets = merged;
                if used_local_snapshot {
                    refreshed_at = local_snapshot.as_ref().map(|snapshot| snapshot.observed_at);
                    source = "session";
                } else {
                    refreshed_at = Some(now);
                    source = if buckets.is_empty() {
                        "server-empty"
                    } else {
                        "server"
                    };
                }
            }
            Err(message) => {
                if let Some(snapshot) = local_snapshot.filter(|snapshot| {
                    local_snapshot_matches_current_credentials(account, snapshot)
                }) {
                    buckets = valid_local_rate_limit_buckets(&snapshot.buckets, now);
                    if buckets.is_empty() {
                        error = Some(message);
                        source = "unavailable";
                    } else {
                        // Le snapshot local permet encore d'afficher les derniers
                        // quotas connus, mais ne doit pas masquer une session
                        // revoquee : l'UI a besoin de cette erreur pour proposer
                        // une vraie reconnexion.
                        error = Some(message);
                        refreshed_at = Some(snapshot.observed_at);
                        source = "session";
                    }
                } else {
                    error = Some(message);
                    source = "unavailable";
                }
            }
        }
    }

    let session_bucket = bucket_for_window(&buckets, SESSION_LIMIT_MINS);
    let weekly_bucket = bucket_for_window(&buckets, WEEKLY_LIMIT_MINS);
    let session_reset_at = session_bucket.map(|bucket| bucket.resets_at);
    let weekly_reset_at = weekly_bucket.map(|bucket| bucket.resets_at);
    AccountLimitView {
        id: account.id.clone(),
        label: account.label.clone(),
        provider: account.provider,
        codex_home: account.codex_home.clone(),
        has_tokens,
        connected_at: account.limits.connected_at,
        session_reset_at,
        weekly_reset_at,
        session_remaining_secs: session_reset_at.map(|value| (value - now).max(0)),
        weekly_remaining_secs: weekly_reset_at.map(|value| (value - now).max(0)),
        session_used_percent: session_bucket.and_then(|bucket| bucket.used_percent),
        weekly_used_percent: weekly_bucket.and_then(|bucket| bucket.used_percent),
        buckets,
        refreshed_at,
        source: source.to_string(),
        refreshing: false,
        error,
        session_busy: account_session_busy(account),
    }
}

fn bucket_for_window(
    buckets: &[AccountRateLimitBucketView],
    window_duration_mins: i64,
) -> Option<&AccountRateLimitBucketView> {
    buckets
        .iter()
        .filter(|bucket| bucket.window_duration_mins == window_duration_mins)
        .min_by_key(|bucket| bucket.resets_at)
}

/// Catalogue officiel pour le compte selectionne. Codex conserve son
/// `model/list` local ; OpenCode/OpenRouter interroge directement le catalogue
/// filtre par la cle du compte, sans jamais faire traverser cette cle au client.
pub async fn load_account_model_catalog(account_id: &str) -> Result<Vec<AccountModelView>, String> {
    let settings = load_settings_for_terminal()?;
    let account = settings
        .accounts
        .iter()
        .find(|candidate| candidate.id == account_id)
        .cloned()
        .ok_or_else(|| "Compte introuvable".to_string())?;

    match account.provider {
        Provider::Freebuff => Ok(freebuff_model_catalog()),
        Provider::OpenCode
            if account
                .inference_provider
                .as_deref()
                .is_some_and(|provider| provider.eq_ignore_ascii_case("openrouter")) =>
        {
            let home = expand_home(&account.codex_home)?;
            let api_key = read_opencode_provider_api_key(&home, "openrouter")?;
            let proxy_url = proxy_url_for_account(&account, &settings);
            load_openrouter_model_catalog(&api_key, proxy_url.as_deref()).await
        }
        Provider::Codex => tokio::task::spawn_blocking(move || {
            load_codex_account_model_catalog(&account, &settings)
        })
        .await
        .map_err(|_| "Lecture du catalogue Codex interrompue".to_string())?,            Provider::OpenCode | Provider::Claude | Provider::Aihubmix => Ok(Vec::new()),
    }
}

fn load_codex_account_model_catalog(
    account: &AccountProfile,
    settings: &AppSettings,
) -> Result<Vec<AccountModelView>, String> {
    let app_server_result = read_model_catalog_from_app_server(&account, &settings);
    if let Ok(result) = app_server_result.as_ref() {
        let models = parse_account_model_catalog(result);
        if !models.is_empty() {
            return Ok(models);
        }
    }

    let home = expand_home(&account.codex_home)?;
    let cache_path = home.join("models_cache.json");
    if let Ok(content) = fs::read_to_string(&cache_path) {
        if let Ok(value) = serde_json::from_str::<Value>(&content) {
            let models = parse_account_model_catalog(&value);
            if !models.is_empty() {
                return Ok(models);
            }
        }
    }

    Err(app_server_result
        .err()
        .unwrap_or_else(|| "Catalogue de modeles Codex indisponible".to_string()))
}

fn read_opencode_provider_api_key(home: &Path, provider: &str) -> Result<String, String> {
    let path = home.join("data").join("opencode").join("auth.json");
    let content = fs::read_to_string(path)
        .map_err(|_| "Identifiants OpenRouter introuvables pour ce compte".to_string())?;
    let root = serde_json::from_str::<Value>(&content)
        .map_err(|_| "Identifiants OpenRouter illisibles pour ce compte".to_string())?;
    let entry = root.as_object().and_then(|providers| {
        providers.get(provider).or_else(|| {
            providers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(provider))
                .map(|(_, value)| value)
        })
    });
    let api_key = match entry {
        Some(Value::String(value)) => Some(value.as_str()),
        Some(Value::Object(fields)) => ["key", "access", "token"].into_iter().find_map(|field| {
            fields
                .get(field)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        }),
        _ => None,
    }
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .filter(|value| !value.chars().any(char::is_control))
    .ok_or_else(|| "Cle OpenRouter absente pour ce compte".to_string())?;
    Ok(api_key.to_string())
}

fn openrouter_model_client(proxy_url: Option<&str>) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(OPENROUTER_MODEL_CATALOG_TIMEOUT_SECS))
        // Un redirect ne doit jamais pouvoir transporter le Bearer du compte
        // vers un autre hote. La pagination est validee separement ci-dessous.
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("codex-switch-terminal/", env!("CARGO_PKG_VERSION")));
    if let Some(proxy_url) = proxy_url {
        builder = builder.proxy(
            reqwest::Proxy::all(proxy_url).map_err(|_| "proxy OpenRouter invalide".to_string())?,
        );
    }
    builder
        .build()
        .map_err(|_| "client OpenRouter indisponible".to_string())
}

async fn load_openrouter_model_catalog(
    api_key: &str,
    proxy_url: Option<&str>,
) -> Result<Vec<AccountModelView>, String> {
    let endpoint = Url::parse(OPENROUTER_MODELS_USER_URL)
        .map_err(|_| "Endpoint OpenRouter invalide".to_string())?;
    let client = openrouter_model_client(proxy_url)?;
    fetch_openrouter_model_catalog(&client, &endpoint, api_key).await
}

async fn fetch_openrouter_model_catalog(
    client: &reqwest::Client,
    endpoint: &Url,
    api_key: &str,
) -> Result<Vec<AccountModelView>, String> {
    let mut current = openrouter_page_url(endpoint, 0);
    let mut visited = HashSet::new();
    let mut entries = Vec::new();

    for _ in 0..OPENROUTER_MODEL_MAX_PAGES {
        if !visited.insert(current.as_str().to_string()) {
            return Err("Pagination OpenRouter cyclique".to_string());
        }
        let response = client
            .get(current.clone())
            .bearer_auth(api_key)
            .header("Accept", "application/json")
            .send()
            .await
            // Une erreur reqwest peut inclure l'URL du proxy et ses
            // identifiants. Le message public reste donc volontairement neutre.
            .map_err(|_| "Lecture du catalogue OpenRouter impossible".to_string())?;
        let status = response.status();
        if !status.is_success() {
            let message = match status.as_u16() {
                401 | 403 => "Cle OpenRouter refusee pour ce compte".to_string(),
                429 => "Catalogue OpenRouter temporairement limite".to_string(),
                code => format!("Catalogue OpenRouter indisponible (HTTP {code})"),
            };
            return Err(message);
        }

        let link_header = response
            .headers()
            .get(reqwest::header::LINK)
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        let payload = response
            .json::<Value>()
            .await
            .map_err(|_| "Reponse du catalogue OpenRouter illisible".to_string())?;
        let page = payload
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| "Reponse du catalogue OpenRouter invalide".to_string())?;
        let page_len = page.len();
        entries.extend(page.iter().cloned());

        let Some(next) = openrouter_next_page_url(
            endpoint,
            &current,
            &payload,
            link_header.as_deref(),
            entries.len(),
            page_len,
        )?
        else {
            return Ok(parse_openrouter_model_catalog(&entries));
        };
        current = next;
    }

    Err("Catalogue OpenRouter trop volumineux".to_string())
}

fn openrouter_page_url(endpoint: &Url, offset: usize) -> Url {
    let mut url = endpoint.clone();
    url.set_query(None);
    url.query_pairs_mut()
        .append_pair("limit", &OPENROUTER_MODEL_PAGE_LIMIT.to_string())
        .append_pair("offset", &offset.to_string());
    url
}

fn openrouter_next_page_url(
    endpoint: &Url,
    current: &Url,
    payload: &Value,
    link_header: Option<&str>,
    next_offset: usize,
    page_len: usize,
) -> Result<Option<Url>, String> {
    let payload_next = [
        payload.get("next"),
        payload.pointer("/meta/next"),
        payload.pointer("/meta/next_page"),
        payload.pointer("/pagination/next"),
        payload.pointer("/links/next"),
    ]
    .into_iter()
    .flatten()
    .find_map(Value::as_str)
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .map(ToString::to_string);
    let link_next = link_header
        .and_then(openrouter_next_link)
        .map(ToString::to_string);

    if let Some(reference) = payload_next.or(link_next) {
        let candidate = Url::parse(&reference).or_else(|_| current.join(&reference));
        let candidate = candidate.map_err(|_| "Pagination OpenRouter invalide".to_string())?;
        if !same_openrouter_catalog_endpoint(endpoint, &candidate) {
            return Err("Pagination OpenRouter invalide".to_string());
        }
        return Ok(Some(candidate));
    }

    let has_more = payload
        .pointer("/meta/has_more")
        .or_else(|| payload.pointer("/pagination/has_more"))
        .or_else(|| payload.get("has_more"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let total = payload
        .pointer("/meta/total_count")
        .or_else(|| payload.pointer("/meta/total"))
        .or_else(|| payload.pointer("/pagination/total"))
        .or_else(|| payload.get("total_count"))
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok());
    let inferred_more =
        page_len == OPENROUTER_MODEL_PAGE_LIMIT || total.is_some_and(|total| next_offset < total);
    if page_len > 0 && (has_more || inferred_more) {
        Ok(Some(openrouter_page_url(endpoint, next_offset)))
    } else {
        Ok(None)
    }
}

fn openrouter_next_link(header: &str) -> Option<&str> {
    header.split(',').find_map(|part| {
        let mut fields = part.split(';');
        let target = fields.next()?.trim();
        let is_next = fields.any(|field| {
            let relation = field.trim();
            relation.eq_ignore_ascii_case("rel=next")
                || relation.eq_ignore_ascii_case("rel=\"next\"")
        });
        if !is_next {
            return None;
        }
        target.strip_prefix('<')?.strip_suffix('>')
    })
}

fn same_openrouter_catalog_endpoint(endpoint: &Url, candidate: &Url) -> bool {
    candidate.scheme() == endpoint.scheme()
        && candidate.host_str() == endpoint.host_str()
        && candidate.port_or_known_default() == endpoint.port_or_known_default()
        && candidate.path() == endpoint.path()
        && candidate.username().is_empty()
        && candidate.password().is_none()
        && candidate.fragment().is_none()
}

fn parse_openrouter_model_catalog(entries: &[Value]) -> Vec<AccountModelView> {
    let mut seen = HashSet::new();
    entries
        .iter()
        .filter_map(|entry| {
            let slug = entry.get("id").and_then(Value::as_str)?;
            if !safe_openrouter_model_slug(slug) {
                return None;
            }
            let id = format!("openrouter/{slug}");
            if !seen.insert(id.clone()) {
                return None;
            }
            let display_name = entry
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(slug)
                .to_string();
            let (default_reasoning_effort, supported_reasoning_efforts) =
                parse_openrouter_reasoning(entry.get("reasoning"));
            Some(AccountModelView {
                id,
                display_name,
                default_reasoning_effort,
                supported_reasoning_efforts,
                supports_fast_mode: false,
            })
        })
        .collect()
}

pub(crate) fn safe_openrouter_model_slug(slug: &str) -> bool {
    const PREFIX_LEN: usize = "openrouter/".len();
    if slug.is_empty()
        || slug != slug.trim()
        || PREFIX_LEN + slug.chars().count() > OPENROUTER_MODEL_ID_MAX_CHARS
        || !slug
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphanumeric() || character == '~')
    {
        return false;
    }
    let segments = slug.split('/').collect::<Vec<_>>();
    segments.len() >= 2
        && segments.iter().enumerate().all(|(segment_index, segment)| {
            !segment.is_empty()
                && *segment != "."
                && *segment != ".."
                && segment
                    .chars()
                    .enumerate()
                    .all(|(character_index, character)| {
                        character.is_ascii_alphanumeric()
                            || matches!(character, '-' | '_' | '.' | ':')
                            || (character == '~' && segment_index == 0 && character_index == 0)
                    })
        })
}

fn parse_openrouter_reasoning(
    reasoning: Option<&Value>,
) -> (Option<String>, Vec<ModelReasoningEffortView>) {
    let Some(reasoning) = reasoning.and_then(Value::as_object) else {
        return (None, Vec::new());
    };
    let mandatory = reasoning
        .get("mandatory")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut seen = HashSet::new();
    let mut supported = match reasoning.get("supported_efforts") {
        Some(Value::Null) => OPENROUTER_REASONING_EFFORTS
            .iter()
            .map(|effort| (*effort).to_string())
            .collect::<Vec<_>>(),
        Some(Value::Array(efforts)) => efforts
            .iter()
            .filter_map(Value::as_str)
            .filter(|effort| is_openrouter_reasoning_effort(effort))
            .filter(|effort| seen.insert((*effort).to_string()))
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    if mandatory {
        supported.retain(|effort| effort != "none");
    }
    let supported_reasoning_efforts = supported
        .into_iter()
        .map(|reasoning_effort| ModelReasoningEffortView {
            reasoning_effort,
            description: None,
        })
        .collect::<Vec<_>>();
    let default_enabled = reasoning
        .get("default_enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let default_reasoning_effort = default_enabled
        .then(|| reasoning.get("default_effort").and_then(Value::as_str))
        .flatten()
        .filter(|effort| is_openrouter_reasoning_effort(effort))
        .filter(|effort| {
            supported_reasoning_efforts
                .iter()
                .any(|supported| supported.reasoning_effort.as_str() == *effort)
        })
        .map(ToString::to_string);
    (default_reasoning_effort, supported_reasoning_efforts)
}

pub(crate) fn is_openrouter_reasoning_effort(effort: &str) -> bool {
    OPENROUTER_REASONING_EFFORTS.contains(&effort)
}

/// Catalogue des modeles proposes par freebuff.
///
/// Il est fige ici plutot qu'interroge : freebuff n'expose aucun point
/// d'acces au catalogue, la liste est compilee dans son binaire et son
/// validateur n'accepte que ces identifiants exacts. Un identifiant absent
/// de cette liste est silencieusement ignore a la lecture de sa config.
///
/// L'intensite de raisonnement est donnee en lecture seule : le CLI freebuff
/// ne persiste aucun reglage d'intensite et applique la valeur propre au
/// modele. Choisir le modele est le seul levier de profondeur disponible.
fn freebuff_model_catalog() -> Vec<AccountModelView> {
    // Le statut premium est porte par le libelle : c'est la seule
    // information qui distingue un modele facture en credits d'un modele
    // illimite, et elle n'apparait nulle part ailleurs cote Switch. Le
    // modele illimite vient en tete, c'est celui applique par defaut.
    const MODELS: [(&str, &str, Option<&str>); 4] = [
        (
            "deepseek/deepseek-v4-flash",
            "DeepSeek V4 Flash (illimite)",
            Some("high"),
        ),
        (
            "deepseek/deepseek-v4-pro",
            "DeepSeek V4 Pro (premium, le plus capable)",
            Some("high"),
        ),
        (
            "openai/gpt-5.6-luna",
            "GPT-5.6 Luna (premium, images)",
            Some("high"),
        ),
        (
            "minimax/minimax-m3",
            "MiniMax M3 (premium, le plus rapide)",
            None,
        ),
    ];

    MODELS
        .into_iter()
        .map(|(id, display_name, effort)| AccountModelView {
            id: id.to_string(),
            display_name: display_name.to_string(),
            default_reasoning_effort: effort.map(str::to_string),
            // Vide a dessein : aucune intensite n'est selectionnable.
            supported_reasoning_efforts: Vec::new(),
            supports_fast_mode: false,
        })
        .collect()
}

fn read_model_catalog_from_app_server(
    account: &AccountProfile,
    settings: &AppSettings,
) -> Result<Value, String> {
    let codex_home = expand_home(&account.codex_home)?;
    let mut command = codex_app_server_command(settings);
    command
        .env("CODEX_HOME", codex_home.to_string_lossy().to_string())
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    if let Some(proxy_url) = proxy_url_for_account(account, settings) {
        for key in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            command.env(key, proxy_url.clone());
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }

    let mut child = command
        .spawn()
        .map_err(|error| format!("codex app-server impossible: {error}"))?;
    let Some(mut stdin) = child.stdin.take() else {
        let _ = child.kill();
        return Err("stdin app-server indisponible".to_string());
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        return Err("stdout app-server indisponible".to_string());
    };

    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let requests = [
        json!({
            "method": "initialize",
            "id": 1,
            "params": {
                "clientInfo": {
                    "name": "codex_switch_terminal",
                    "title": "Codex Switch Terminal",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        }),
        json!({ "method": "initialized", "params": {} }),
        json!({
            "method": "model/list",
            "id": 2,
            "params": { "limit": 100, "includeHidden": false }
        }),
    ];
    for request in requests {
        if let Err(error) = writeln!(stdin, "{request}") {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("ecriture app-server impossible: {error}"));
        }
    }
    let _ = stdin.flush();

    let response = loop {
        match rx.recv_timeout(Duration::from_secs(MODEL_CATALOG_TIMEOUT_SECS)) {
            Ok(line) => {
                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if value.get("id").and_then(Value::as_i64) == Some(2) {
                    break value;
                }
            }
            Err(_) => {
                drop(stdin);
                let _ = child.kill();
                let _ = child.wait();
                return Err("timeout lecture catalogue de modeles".to_string());
            }
        }
    };

    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    if let Some(error) = response.get("error") {
        return Err(error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("erreur app-server")
            .to_string());
    }
    response
        .get("result")
        .cloned()
        .ok_or_else(|| "reponse app-server sans result".to_string())
}

fn parse_account_model_catalog(value: &Value) -> Vec<AccountModelView> {
    let entries = value
        .get("data")
        .or_else(|| value.get("models"))
        .and_then(Value::as_array);
    let Some(entries) = entries else {
        return Vec::new();
    };

    let mut seen = HashSet::new();
    entries
        .iter()
        .filter(|entry| entry.get("hidden").and_then(Value::as_bool) != Some(true))
        .filter(|entry| {
            entry
                .get("visibility")
                .and_then(Value::as_str)
                .is_none_or(|visibility| visibility == "list")
        })
        .filter_map(|entry| {
            let id = entry
                .get("id")
                .or_else(|| entry.get("model"))
                .or_else(|| entry.get("slug"))
                .and_then(Value::as_str)?
                .trim()
                .to_string();
            if id.is_empty() || !seen.insert(id.clone()) {
                return None;
            }
            let display_name = entry
                .get("displayName")
                .or_else(|| entry.get("display_name"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(&id)
                .to_string();
            let default_reasoning_effort = entry
                .get("defaultReasoningEffort")
                .or_else(|| entry.get("default_reasoning_level"))
                .and_then(Value::as_str)
                .map(ToString::to_string);
            let supported_reasoning_efforts = entry
                .get("supportedReasoningEfforts")
                .or_else(|| entry.get("supported_reasoning_levels"))
                .and_then(Value::as_array)
                .map(|efforts| {
                    efforts
                        .iter()
                        .filter_map(|effort| {
                            let reasoning_effort = effort
                                .get("reasoningEffort")
                                .or_else(|| effort.get("effort"))
                                .and_then(Value::as_str)?
                                .to_string();
                            let description = effort
                                .get("description")
                                .and_then(Value::as_str)
                                .map(ToString::to_string);
                            Some(ModelReasoningEffortView {
                                reasoning_effort,
                                description,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let supports_fast_mode = entry
                .get("serviceTiers")
                .or_else(|| entry.get("service_tiers"))
                .and_then(Value::as_array)
                .is_some_and(|tiers| {
                    tiers.iter().any(|tier| {
                        tier.get("id")
                            .and_then(Value::as_str)
                            .is_some_and(|id| id.eq_ignore_ascii_case("priority"))
                    })
                })
                || entry
                    .get("additionalSpeedTiers")
                    .or_else(|| entry.get("additional_speed_tiers"))
                    .and_then(Value::as_array)
                    .is_some_and(|tiers| {
                        tiers.iter().any(|tier| {
                            tier.as_str()
                                .is_some_and(|id| id.eq_ignore_ascii_case("fast"))
                        })
                    });
            Some(AccountModelView {
                id,
                display_name,
                default_reasoning_effort,
                supported_reasoning_efforts,
                supports_fast_mode,
            })
        })
        .collect()
}

#[derive(Debug)]
struct LocalRateLimitSnapshot {
    buckets: Vec<AccountRateLimitBucketView>,
    observed_at: i64,
}

/// Un home peut etre reconnecte a un autre compte sans supprimer ses anciens
/// rollouts. En cas d'echec reseau, ces mesures ne sont un fallback fiable que
/// si elles ont ete observees apres la derniere ecriture des credentials.
///
/// Cette verification reste volontairement limitee au chemin d'erreur : quand
/// le serveur repond, la fusion par fenetre + reset ci-dessous sait conserver
/// une vraie consommation locale tout en rejetant les anciennes fenetres.
fn local_snapshot_matches_current_credentials(
    account: &AccountProfile,
    snapshot: &LocalRateLimitSnapshot,
) -> bool {
    let Ok(home) = expand_home(&account.codex_home) else {
        return false;
    };
    let credentials = match account.provider {
        Provider::Codex => home.join("auth.json"),
        Provider::Claude => home.join(".credentials.json"),
        Provider::OpenCode => home.join("data").join("opencode").join("auth.json"),
        Provider::Freebuff => crate::provider::freebuff_config_dir(&home).join("credentials.json"),
        Provider::Aihubmix => home.join(".config").join("aihubmix").join("credentials.json"),
    };
    let Some(modified_at) = fs::metadata(credentials)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
    else {
        // Sans horodatage exploitable, on conserve le comportement historique
        // plutot que de supprimer un fallback potentiellement valide.
        return true;
    };

    snapshot.observed_at >= modified_at
}

/// Codex ecrit la mesure de quota effectivement appliquee a chaque tour dans
/// ses rollouts. Cette mesure est indispensable en secours : certaines
/// versions de `account/rateLimits/read` peuvent renvoyer momentanement une
/// nouvelle fenetre vide (`0 %`) alors qu'une session active recoit encore la
/// vraie consommation du compte.
fn read_local_rate_limit_snapshot(
    account: &AccountProfile,
) -> Result<Option<LocalRateLimitSnapshot>, String> {
    let codex_home = expand_home(&account.codex_home)?;
    let mut files = Vec::new();
    collect_rate_limit_rollouts(&codex_home.join("sessions"), &mut files);
    collect_rate_limit_rollouts(&codex_home.join("sessions-archive"), &mut files);

    let mut latest = None;
    for path in files {
        let Some(snapshot) = scan_rollout_rate_limit_snapshot(&path) else {
            continue;
        };
        if latest
            .as_ref()
            .map(|current: &LocalRateLimitSnapshot| snapshot.observed_at > current.observed_at)
            .unwrap_or(true)
        {
            latest = Some(snapshot);
        }
    }

    Ok(latest)
}

fn collect_rate_limit_rollouts(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rate_limit_rollouts(&path, files);
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
            .unwrap_or(false)
        {
            files.push(path);
        }
    }
}

fn scan_rollout_rate_limit_snapshot(path: &Path) -> Option<LocalRateLimitSnapshot> {
    let file = fs::File::open(path).ok()?;
    let fallback_timestamp = file
        .metadata()
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    let mut latest = None;

    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if !line.contains("\"rate_limits\"") && !line.contains("\"rateLimits\"") {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(limit) = value
            .pointer("/payload/rate_limits")
            .or_else(|| value.pointer("/payload/rateLimits"))
            .filter(|limit| !limit.is_null())
        else {
            continue;
        };

        let mut buckets = Vec::new();
        collect_rate_limit_object(limit, &mut buckets);
        normalize_rate_limit_buckets(&mut buckets);
        if buckets.is_empty() {
            continue;
        }

        let observed_at = value
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|timestamp| chrono::DateTime::parse_from_rfc3339(timestamp).ok())
            .map(|timestamp| timestamp.timestamp())
            .unwrap_or(fallback_timestamp);
        if latest
            .as_ref()
            .map(|current: &LocalRateLimitSnapshot| observed_at > current.observed_at)
            .unwrap_or(true)
        {
            latest = Some(LocalRateLimitSnapshot {
                buckets,
                observed_at,
            });
        }
    }

    latest
}

fn valid_local_rate_limit_buckets(
    buckets: &[AccountRateLimitBucketView],
    now: i64,
) -> Vec<AccountRateLimitBucketView> {
    let mut valid = buckets
        .iter()
        .filter(|bucket| bucket.resets_at > now)
        .cloned()
        .collect::<Vec<_>>();
    normalize_rate_limit_buckets(&mut valid);
    valid
}

/// Fusion monotone : tant que la fenetre locale est la meme que celle du
/// serveur, une lecture reseau a 0 % ne doit jamais effacer une consommation
/// positive observee par une vraie session Codex.
///
/// `primary`/`secondary` ne sont pas des identifiants stables : Codex a deja
/// deplace l'hebdomadaire de l'un vers l'autre. On rapproche donc les buckets
/// par limite + duree + reset. Un snapshot dont le reset differe appartient a
/// une ancienne session (souvent un compte reconnecte dans le meme home) et ne
/// doit ni remplacer ni doubler la valeur serveur. Les fenetres totalement
/// absentes de la reponse serveur, notamment la fenetre courte, restent en
/// revanche utilisables en fallback.
fn merge_rate_limit_buckets(
    mut server_buckets: Vec<AccountRateLimitBucketView>,
    local_buckets: Option<&[AccountRateLimitBucketView]>,
    now: i64,
) -> (Vec<AccountRateLimitBucketView>, bool) {
    let mut used_local_snapshot = false;

    for local in local_buckets
        .into_iter()
        .flatten()
        .filter(|bucket| bucket.resets_at > now)
    {
        let matching = server_buckets.iter().position(|server| {
            server.limit_id == local.limit_id
                && server.window_duration_mins == local.window_duration_mins
        });
        match matching {
            Some(index) => {
                let server = &mut server_buckets[index];
                let same_window = server.resets_at.abs_diff(local.resets_at)
                    <= RATE_LIMIT_RESET_MATCH_TOLERANCE_SECS;
                let server_used = server.used_percent.unwrap_or(-1.0);
                let local_used = local.used_percent.unwrap_or(-1.0);
                if same_window && local_used > server_used {
                    // La valeur serveur garde son slot et son reset canoniques;
                    // seul le compteur monotone vient du snapshot local.
                    server.used_percent = local.used_percent;
                    if server.limit_name.is_none() {
                        server.limit_name.clone_from(&local.limit_name);
                    }
                    if server.rate_limit_reached_type.is_none() {
                        server
                            .rate_limit_reached_type
                            .clone_from(&local.rate_limit_reached_type);
                    }
                    if server.plan_type.is_none() {
                        server.plan_type.clone_from(&local.plan_type);
                    }
                    used_local_snapshot = true;
                }
            }
            None => {
                server_buckets.push(local.clone());
                used_local_snapshot = true;
            }
        }
    }

    normalize_rate_limit_buckets(&mut server_buckets);
    (server_buckets, used_local_snapshot)
}

// ---------------------------------------------------------------------------
// Quotas Claude Code (abonnement claude.ai)
// ---------------------------------------------------------------------------

/// Interrupteur global des lectures reseau de quotas. Les tests unitaires et
/// les environnements volontairement hors ligne l'activent pour retomber sur la
/// vue « connecte » sans mesure, au lieu de tenter un aller-retour HTTP.
static REMOTE_ACCOUNT_LIMITS_DISABLED: AtomicBool = AtomicBool::new(false);

fn remote_account_limits_disabled() -> bool {
    REMOTE_ACCOUNT_LIMITS_DISABLED.load(Ordering::Relaxed)
        || env::var("CST_DISABLE_REMOTE_ACCOUNT_LIMITS")
            .is_ok_and(|value| !matches!(value.trim(), "" | "0"))
}

/// Fournisseurs dont les quotas se lisent a distance, et qui doivent donc
/// participer au rafraichissement en arriere-plan. OpenCode reste purement
/// local : la presence du credential suffit a le declarer connecte.
fn provider_reads_remote_limits(provider: Provider) -> bool {
    matches!(provider, Provider::Codex | Provider::Claude)
}

#[derive(Debug, Clone)]
struct ClaudeOauthCredentials {
    access_token: String,
    refresh_token: Option<String>,
    /// Epoch en millisecondes, tel qu'ecrit par Claude Code.
    expires_at_ms: Option<i64>,
    subscription_type: Option<String>,
}

impl ClaudeOauthCredentials {
    fn is_expired(&self, now: i64) -> bool {
        self.expires_at_ms
            .is_some_and(|expires_at| expires_at / 1000 - CLAUDE_TOKEN_REFRESH_MARGIN_SECS <= now)
    }
}

#[derive(Debug, Clone)]
struct ClaudeRefreshedTokens {
    access_token: String,
    refresh_token: Option<String>,
    expires_at_ms: Option<i64>,
    refresh_token_expires_at_ms: Option<i64>,
    scopes: Option<Vec<String>>,
}

fn claude_credentials_path(home: &Path) -> PathBuf {
    home.join(".credentials.json")
}

fn parse_claude_credentials(value: &Value) -> Option<ClaudeOauthCredentials> {
    let oauth = value.get("claudeAiOauth")?;
    let access_token = oauth
        .get("accessToken")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())?
        .to_string();

    Some(ClaudeOauthCredentials {
        access_token,
        refresh_token: oauth
            .get("refreshToken")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(ToString::to_string),
        expires_at_ms: oauth.get("expiresAt").and_then(Value::as_i64),
        subscription_type: oauth
            .get("subscriptionType")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|plan| !plan.is_empty())
            .map(ToString::to_string),
    })
}

fn read_claude_credentials(home: &Path) -> Result<ClaudeOauthCredentials, String> {
    let path = claude_credentials_path(home);
    let content =
        fs::read(&path).map_err(|error| format!("credentials Claude illisibles: {error}"))?;
    let value = serde_json::from_slice::<Value>(&content)
        .map_err(|error| format!("credentials Claude invalides: {error}"))?;
    parse_claude_credentials(&value)
        .ok_or_else(|| "credentials Claude sans accessToken: connexion requise".to_string())
}

static CLAUDE_CREDENTIALS_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Le refresh token de Claude Code tourne a chaque echange : oublier de
/// reecrire `.credentials.json` deconnecterait le CLI a l'expiration suivante.
/// On preserve donc les cles inconnues du fichier et on ecrit atomiquement.
fn persist_claude_credentials(
    home: &Path,
    refreshed: &ClaudeRefreshedTokens,
) -> Result<(), String> {
    let path = claude_credentials_path(home);
    let _guard = CLAUDE_CREDENTIALS_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let mut root = fs::read(&path)
        .ok()
        .and_then(|content| serde_json::from_slice::<Value>(&content).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let Some(entry) = root
        .as_object_mut()
        .map(|object| object.entry("claudeAiOauth").or_insert_with(|| json!({})))
    else {
        return Err("credentials Claude non modifiables".to_string());
    };
    if !entry.is_object() {
        *entry = json!({});
    }
    let Some(oauth) = entry.as_object_mut() else {
        return Err("credentials Claude non modifiables".to_string());
    };

    oauth.insert("accessToken".to_string(), json!(refreshed.access_token));
    if let Some(refresh_token) = &refreshed.refresh_token {
        oauth.insert("refreshToken".to_string(), json!(refresh_token));
    }
    if let Some(expires_at) = refreshed.expires_at_ms {
        oauth.insert("expiresAt".to_string(), json!(expires_at));
    }
    if let Some(expires_at) = refreshed.refresh_token_expires_at_ms {
        oauth.insert("refreshTokenExpiresAt".to_string(), json!(expires_at));
    }
    if let Some(scopes) = &refreshed.scopes {
        oauth.insert("scopes".to_string(), json!(scopes));
    }

    let serialized = serde_json::to_vec_pretty(&root)
        .map_err(|error| format!("credentials Claude non serialisables: {error}"))?;
    crate::fs_util::atomic_write(&path, serialized)
        .map_err(|error| format!("credentials Claude non enregistrables: {error}"))
}

fn claude_http_client(proxy_url: Option<&str>) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(CLAUDE_USAGE_TIMEOUT_SECS))
        .user_agent(concat!("codex-switch-terminal/", env!("CARGO_PKG_VERSION")));
    if let Some(proxy_url) = proxy_url {
        builder = builder.proxy(
            reqwest::Proxy::all(proxy_url).map_err(|error| format!("proxy invalide: {error}"))?,
        );
    }
    builder
        .build()
        .map_err(|error| format!("client HTTP indisponible: {error}"))
}

async fn request_claude_usage(
    client: &reqwest::Client,
    access_token: &str,
) -> Result<(u16, Option<Value>), String> {
    let response = client
        .get(CLAUDE_USAGE_URL)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Content-Type", "application/json")
        .header("anthropic-beta", CLAUDE_OAUTH_BETA)
        .send()
        .await
        .map_err(|error| format!("lecture des quotas Claude impossible: {error}"))?;
    let status = response.status().as_u16();
    Ok((status, response.json::<Value>().await.ok()))
}

async fn request_claude_token_refresh(
    client: &reqwest::Client,
    refresh_token: &str,
) -> Result<ClaudeRefreshedTokens, String> {
    let response = client
        .post(CLAUDE_OAUTH_TOKEN_URL)
        .header("Content-Type", "application/json")
        .json(&json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": CLAUDE_OAUTH_CLIENT_ID,
            "scope": CLAUDE_OAUTH_SCOPES,
        }))
        .send()
        .await
        .map_err(|error| format!("renouvellement du token Claude impossible: {error}"))?;

    let status = response.status().as_u16();
    let payload = response.json::<Value>().await.ok();
    if status != 200 {
        return Err(format!(
            "session Claude expiree ({status}) : reconnexion requise"
        ));
    }

    let payload = payload.ok_or_else(|| "reponse de renouvellement illisible".to_string())?;
    let access_token = payload
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| "renouvellement Claude sans access_token".to_string())?
        .to_string();
    let now_ms = now_unix() * 1000;

    Ok(ClaudeRefreshedTokens {
        access_token,
        refresh_token: payload
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(ToString::to_string),
        expires_at_ms: payload
            .get("expires_in")
            .and_then(Value::as_i64)
            .map(|expires_in| now_ms + expires_in * 1000),
        refresh_token_expires_at_ms: payload
            .get("refresh_token_expires_in")
            .and_then(Value::as_i64)
            .map(|expires_in| now_ms + expires_in * 1000),
        scopes: payload
            .get("scope")
            .and_then(Value::as_str)
            .map(|scope| scope.split_whitespace().map(ToString::to_string).collect()),
    })
}

/// Claude Code et nos propres chats rafraichissent le meme `.credentials.json`.
/// On relit donc le disque avant de bruler le refresh token : si un autre
/// processus vient d'en obtenir un neuf, le notre est deja invalide.
async fn claude_access_token_after_refresh(
    client: &reqwest::Client,
    home: &Path,
    previous_access_token: &str,
) -> Result<String, String> {
    let current = read_claude_credentials(home)?;
    if current.access_token != previous_access_token && !current.is_expired(now_unix()) {
        return Ok(current.access_token);
    }

    let refresh_token = current
        .refresh_token
        .ok_or_else(|| "session Claude expiree : reconnexion requise".to_string())?;
    let tokens = request_claude_token_refresh(client, &refresh_token).await?;
    persist_claude_credentials(home, &tokens)?;
    Ok(tokens.access_token)
}

fn claude_usage_payload(status: u16, payload: Option<Value>) -> Result<Value, String> {
    match status {
        200 => payload.ok_or_else(|| "reponse de quotas Claude illisible".to_string()),
        401 | 403 => Err(format!(
            "session Claude refusee ({status}) : reconnexion requise"
        )),
        429 => Err("quotas Claude temporairement indisponibles (429)".to_string()),
        _ => Err(format!("lecture des quotas Claude en echec ({status})")),
    }
}

/// Lit les fenetres de consommation d'un compte Claude Code.
///
/// Claude Code n'expose pas d'equivalent de `account/rateLimits/read` : lancer
/// `codex app-server` avec un home Claude ne renverrait rien. La mesure vient
/// de `/api/oauth/usage`, exactement comme la commande `/usage` du CLI.
fn read_claude_rate_limits(
    account: &AccountProfile,
    settings: &AppSettings,
) -> Result<Vec<AccountRateLimitBucketView>, String> {
    if remote_account_limits_disabled() {
        return Ok(Vec::new());
    }

    let home = expand_home(&account.codex_home)?;
    let credentials = read_claude_credentials(&home)?;
    let proxy_url = proxy_url_for_account(account, settings);
    let client = claude_http_client(proxy_url.as_deref())?;
    // `account_limit_view` s'execute toujours sur un thread OS dedie (voir
    // `account_limit_views_uncached`) : y demarrer un runtime est donc sur.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("runtime HTTP indisponible: {error}"))?;

    let payload = runtime.block_on(async {
        let mut access_token = credentials.access_token.clone();
        let mut refreshed = false;

        if credentials.is_expired(now_unix()) {
            access_token = claude_access_token_after_refresh(&client, &home, &access_token).await?;
            refreshed = true;
        }

        let (mut status, mut payload) = request_claude_usage(&client, &access_token).await?;
        if status == 401 && !refreshed {
            let access_token =
                claude_access_token_after_refresh(&client, &home, &access_token).await?;
            let retried = request_claude_usage(&client, &access_token).await?;
            status = retried.0;
            payload = retried.1;
        }

        claude_usage_payload(status, payload)
    })?;

    Ok(claude_usage_buckets(
        &payload,
        credentials.subscription_type.as_deref(),
    ))
}

/// Fenetres exposees par `/api/oauth/usage`, alignees sur les deux jauges de la
/// vue Limites. Le second nom est la cle equivalente du tableau `limits[]`, qui
/// sert de repli quand la fenetre racine est absente.
const CLAUDE_USAGE_WINDOWS: [(&str, &str, i64, &str); 2] = [
    ("five_hour", "session", SESSION_LIMIT_MINS, "primary"),
    ("seven_day", "weekly_all", WEEKLY_LIMIT_MINS, "secondary"),
];

fn claude_usage_buckets(
    payload: &Value,
    plan_type: Option<&str>,
) -> Vec<AccountRateLimitBucketView> {
    let mut buckets = Vec::new();

    for (key, limits_kind, window_duration_mins, bucket) in CLAUDE_USAGE_WINDOWS {
        let Some((used_percent, resets_at)) = claude_usage_window(payload, key)
            .or_else(|| claude_usage_limits_entry(payload, limits_kind))
        else {
            continue;
        };

        buckets.push(AccountRateLimitBucketView {
            limit_id: "claude".to_string(),
            limit_name: Some(key.to_string()),
            bucket: bucket.to_string(),
            window_duration_mins,
            resets_at,
            used_percent: Some(used_percent),
            rate_limit_reached_type: (used_percent >= 100.0)
                .then(|| "rate_limit_reached".to_string()),
            plan_type: plan_type.map(ToString::to_string),
        });
    }

    normalize_rate_limit_buckets(&mut buckets);
    buckets
}

/// `utilization` est un pourcentage 0-100 et `resets_at` un horodatage RFC 3339.
/// Une fenetre inactive est renvoyee a `null` : elle ne doit pas produire de
/// jauge a 0 %, qui ferait croire a un quota intact.
fn claude_usage_window(payload: &Value, key: &str) -> Option<(f64, i64)> {
    let window = payload.get(key).filter(|value| !value.is_null())?;
    let used_percent = window.get("utilization").and_then(json_f64)?;
    let resets_at = claude_reset_timestamp(window.get("resets_at")?)?;
    Some((used_percent.clamp(0.0, 100.0), resets_at))
}

fn claude_usage_limits_entry(payload: &Value, kind: &str) -> Option<(f64, i64)> {
    payload
        .get("limits")
        .and_then(Value::as_array)?
        .iter()
        .find(|entry| entry.get("kind").and_then(Value::as_str) == Some(kind))
        .and_then(|entry| {
            let used_percent = entry.get("percent").and_then(json_f64)?;
            let resets_at = claude_reset_timestamp(entry.get("resets_at")?)?;
            Some((used_percent.clamp(0.0, 100.0), resets_at))
        })
}

/// L'API renvoie du RFC 3339, mais le CLI sait aussi lire des epochs : on
/// accepte les deux plutot que de perdre la fenetre sur un changement de format.
fn claude_reset_timestamp(value: &Value) -> Option<i64> {
    if let Some(text) = value.as_str() {
        return chrono::DateTime::parse_from_rfc3339(text.trim())
            .ok()
            .map(|timestamp| timestamp.timestamp());
    }

    let number = json_f64(value)?;
    if !number.is_finite() || number <= 0.0 {
        return None;
    }
    Some(if number > 1e12 {
        (number / 1000.0) as i64
    } else {
        number as i64
    })
}

fn read_server_rate_limits(
    account: &AccountProfile,
    settings: &AppSettings,
) -> Result<Vec<AccountRateLimitBucketView>, String> {
    let codex_home = expand_home(&account.codex_home)?;
    let mut command = codex_app_server_command(settings);
    command
        .env("CODEX_HOME", codex_home.to_string_lossy().to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    if let Some(proxy_url) = proxy_url_for_account(account, settings) {
        for key in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            command.env(key, proxy_url.clone());
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }

    let mut child = command
        .spawn()
        .map_err(|error| format!("codex app-server impossible: {error}"))?;

    let Some(mut stdin) = child.stdin.take() else {
        let _ = child.kill();
        return Err("stdin app-server indisponible".to_string());
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        return Err("stdout app-server indisponible".to_string());
    };

    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let requests = [
        json!({
            "method": "initialize",
            "id": 1,
            "params": {
                "clientInfo": {
                    "name": "codex_switch_terminal",
                    "title": "Codex Switch Terminal",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        }),
        json!({ "method": "initialized", "params": {} }),
        json!({ "method": "account/rateLimits/read", "id": 2 }),
    ];

    for request in requests {
        writeln!(stdin, "{request}").map_err(|error| {
            let _ = child.kill();
            format!("ecriture app-server impossible: {error}")
        })?;
    }
    let _ = stdin.flush();
    // NE PAS fermer stdin ici. Depuis codex >= 0.144, `codex app-server`
    // interprete la fin de stdin (EOF) comme un signal d'arret et quitte
    // *avant* d'avoir termine la requete asynchrone `account/rateLimits/read`
    // (~1 a 2 s de reseau). Fermer stdin trop tot faisait quitter le serveur
    // sans reponse, ce qui remontait a tort en "timeout lecture limites
    // serveur". On garde donc `stdin` ouvert jusqu'a reception de la reponse.

    let timeout = Duration::from_secs(RATE_LIMIT_READ_TIMEOUT_SECS);
    let response = loop {
        match rx.recv_timeout(timeout) {
            Ok(line) => {
                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if value.get("id").and_then(Value::as_i64) == Some(2) {
                    break value;
                }
            }
            Err(_) => {
                drop(stdin);
                let _ = child.kill();
                let _ = child.wait();
                return Err("timeout lecture limites serveur".to_string());
            }
        }
    };

    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();

    if let Some(error) = response.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("erreur app-server");
        return Err(message.to_string());
    }

    let result = response
        .get("result")
        .ok_or_else(|| "reponse app-server sans result".to_string())?;
    let mut buckets = extract_rate_limit_buckets(result);
    normalize_rate_limit_buckets(&mut buckets);

    Ok(buckets)
}

fn normalize_rate_limit_buckets(buckets: &mut Vec<AccountRateLimitBucketView>) {
    buckets.sort_by(|a, b| {
        (
            a.limit_id.as_str(),
            a.bucket.as_str(),
            a.window_duration_mins,
            a.resets_at,
        )
            .cmp(&(
                b.limit_id.as_str(),
                b.bucket.as_str(),
                b.window_duration_mins,
                b.resets_at,
            ))
    });
    buckets.dedup_by(|a, b| {
        a.limit_id == b.limit_id
            && a.bucket == b.bucket
            && a.window_duration_mins == b.window_duration_mins
            && a.resets_at == b.resets_at
    });
}

pub(crate) fn codex_app_server_command(settings: &AppSettings) -> Command {
    let codex = settings.codex_command.trim();
    let codex = if codex.is_empty() { "codex" } else { codex };

    if cfg!(windows) {
        let mut command = Command::new("cmd.exe");
        command.arg("/S").arg("/C").arg(format!(
            "{} app-server --stdio",
            quote_windows_command(codex)
        ));
        command
    } else {
        let mut parts = codex.split_whitespace();
        let program = parts.next().unwrap_or("codex");
        let mut command = Command::new(program);
        command.args(parts).arg("app-server").arg("--stdio");
        command
    }
}

fn quote_windows_command(command: &str) -> String {
    if command.contains([' ', '\t']) && !command.starts_with('"') {
        format!("\"{}\"", command.replace('"', "\\\""))
    } else {
        command.to_string()
    }
}

pub(crate) fn proxy_url_for_account(
    account: &AccountProfile,
    settings: &AppSettings,
) -> Option<String> {
    if !settings.proxy_controls_enabled {
        return None;
    }

    account
        .proxy_id
        .as_ref()
        .and_then(|id| settings.proxies.iter().find(|proxy| proxy.id == *id))
        .map(|proxy| proxy.proxy_url.clone())
}

fn extract_rate_limit_buckets(result: &Value) -> Vec<AccountRateLimitBucketView> {
    let mut buckets = Vec::new();

    if let Some(limit) = result
        .get("rateLimits")
        .or_else(|| result.get("rate_limits"))
    {
        collect_rate_limit_object(limit, &mut buckets);
    }

    if let Some(map) = result
        .get("rateLimitsByLimitId")
        .or_else(|| result.get("rate_limits_by_limit_id"))
        .and_then(Value::as_object)
    {
        for limit in map.values() {
            collect_rate_limit_object(limit, &mut buckets);
        }
    }

    buckets
}

fn collect_rate_limit_object(limit: &Value, buckets: &mut Vec<AccountRateLimitBucketView>) {
    let limit_id = limit
        .get("limitId")
        .or_else(|| limit.get("limit_id"))
        .and_then(Value::as_str)
        .unwrap_or("codex")
        .to_string();
    let limit_name = limit
        .get("limitName")
        .or_else(|| limit.get("limit_name"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let reached_type = limit
        .get("rateLimitReachedType")
        .or_else(|| limit.get("rate_limit_reached_type"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let plan_type = limit
        .get("planType")
        .or_else(|| limit.get("plan_type"))
        .and_then(Value::as_str)
        .map(ToString::to_string);

    for bucket_name in ["primary", "secondary"] {
        let Some(bucket) = limit.get(bucket_name) else {
            continue;
        };
        if bucket.is_null() {
            continue;
        }

        let Some(window_duration_mins) = bucket
            .get("windowDurationMins")
            .or_else(|| bucket.get("window_minutes"))
            .and_then(Value::as_i64)
        else {
            continue;
        };
        let Some(resets_at) = bucket
            .get("resetsAt")
            .or_else(|| bucket.get("resets_at"))
            .and_then(Value::as_i64)
        else {
            continue;
        };

        buckets.push(AccountRateLimitBucketView {
            limit_id: limit_id.clone(),
            limit_name: limit_name.clone(),
            bucket: bucket_name.to_string(),
            window_duration_mins,
            resets_at,
            used_percent: bucket
                .get("usedPercent")
                .or_else(|| bucket.get("used_percent"))
                .and_then(json_f64),
            rate_limit_reached_type: reached_type.clone(),
            plan_type: plan_type.clone(),
        });
    }
}

fn json_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|number| number as f64))
}

#[derive(Debug, Clone)]
struct ImportedAccount {
    alias: String,
    label: String,
    id_token: String,
    access_token: String,
    refresh_token: String,
    account_id: String,
    last_refresh: Option<String>,
    proxy_url: Option<String>,
}

fn import_single_account(
    settings: &mut AppSettings,
    account: ImportedAccount,
) -> Result<(), String> {
    // On accepte un refresh_token vide (cas du blob de session ChatGPT qui ne
    // contient que l'accessToken). Le compte restera valide jusqu'a
    // l'expiration du JWT, sans renouvellement possible.
    if account.access_token.is_empty() {
        return Ok(());
    }

    let home = if let Some(value) = env::var_os("CST_DATA_DIR") {
        PathBuf::from(value)
            .join("codex-homes")
            .join(format!("pool-{}", account.alias))
    } else {
        home_dir()?.join(format!(".codex-pool-{}", account.alias))
    };
    fs::create_dir_all(&home).map_err(|error| error.to_string())?;

    // Codex CLI valide le format du `id_token` (JWT). Les blobs de session
    // ChatGPT n'en fournissent pas : on reutilise l'accessToken (JWT valide
    // contenant deja les memes claims d'identite) pour satisfaire le parseur.
    let id_token = if account.id_token.is_empty() {
        account.access_token.clone()
    } else {
        account.id_token.clone()
    };

    let auth = json!({
        "auth_mode": "chatgpt",
        "OPENAI_API_KEY": Value::Null,
        "tokens": {
            "id_token": id_token,
            "access_token": account.access_token,
            "refresh_token": account.refresh_token,
            "account_id": account.account_id,
        },
        "last_refresh": account.last_refresh.unwrap_or_else(now_iso8601),
    });

    fs::write(
        home.join("auth.json"),
        serde_json::to_string_pretty(&auth).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;

    let home_string = home.to_string_lossy().to_string();
    let id = stable_id("account", &home_string);
    let now = now_unix();
    let proxy_id = match account
        .proxy_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(proxy_url) => Some(upsert_proxy(settings, proxy_url, &account.label)),
        None => None,
    };

    // Compte importe : herite du defaut global "Bypass defaut" (comme la
    // creation via l'UI / newAccountProfile), pas un `true` code en dur.
    let bypass_default = settings.codex_bypass;
    let (bypass_enabled, model, reasoning_effort, fast_mode) = match settings
        .accounts
        .iter_mut()
        .find(|candidate| candidate.id == id)
    {
        Some(existing) => {
            existing.label = account.label;
            existing.codex_home = home_string;
            existing.proxy_id = proxy_id;
            touch_account_limits(&mut existing.limits, now);
            (
                existing.bypass,
                existing.model.clone(),
                existing.reasoning_effort.clone(),
                existing.fast_mode,
            )
        }
        None => {
            let model = Some(DEFAULT_ACCOUNT_MODEL.to_string());
            let reasoning_effort = Some(DEFAULT_ACCOUNT_REASONING_EFFORT.to_string());
            settings.accounts.push(AccountProfile {
                id,
                label: account.label,
                created_at: Some(now),
                provider: Provider::Codex,
                inference_provider: None,
                codex_home: home_string,
                project_dir: None,
                proxy_id,
                startup_command: None,
                limits: new_connected_limits(now),
                bypass: bypass_default,
                model: model.clone(),
                reasoning_effort: reasoning_effort.clone(),
                fast_mode: false,
                completed_on: None,
                api_key: None,
            });
            (bypass_default, model, reasoning_effort, false)
        }
    };

    ensure_codex_account_config(
        &home,
        bypass_enabled,
        model.as_deref(),
        reasoning_effort.as_deref(),
        fast_mode,
    )
    .map_err(|error| error.to_string())?;

    Ok(())
}

fn upsert_proxy(settings: &mut AppSettings, proxy_url: &str, label: &str) -> String {
    if let Some(existing) = settings
        .proxies
        .iter()
        .find(|proxy| proxy.proxy_url.trim() == proxy_url)
    {
        return existing.id.clone();
    }

    let id = stable_id("proxy", proxy_url);
    settings.proxies.push(ProxyProfile {
        id: id.clone(),
        label: format!("Proxy {label}"),
        proxy_url: proxy_url.to_string(),
        note: Some("imported account doc".to_string()),
    });
    id
}

fn extract_import_accounts(value: &Value) -> Vec<ImportedAccount> {
    if let Some(array) = value.as_array() {
        return array.iter().flat_map(extract_import_accounts).collect();
    }

    let Some(object) = value.as_object() else {
        return Vec::new();
    };

    if let Some(accounts) = object.get("accounts").and_then(Value::as_array) {
        return accounts
            .iter()
            .filter_map(import_from_nested_account)
            .collect();
    }

    if object
        .get("credentials")
        .and_then(Value::as_object)
        .is_some()
    {
        return import_from_nested_account(value).into_iter().collect();
    }

    // Blob de session web ChatGPT (chatgpt.com/api/auth/session) :
    // champ camelCase `accessToken`, pas de refresh_token. Compte a ~10 jours
    // de duree de vie (expiration du JWT), non renouvelable.
    if object.get("accessToken").and_then(Value::as_str).is_some() {
        return import_from_session_blob(value).into_iter().collect();
    }

    import_from_flat_account(value).into_iter().collect()
}

fn import_from_flat_account(value: &Value) -> Option<ImportedAccount> {
    let access_token = string_at(value, &["access_token"])?;
    let refresh_token = string_at(value, &["refresh_token"])?;
    let id_token = string_at(value, &["id_token"]).unwrap_or_default();
    let account_id = string_at(value, &["account_id"])
        .or_else(|| string_at(value, &["chatgpt_account_id"]))
        .unwrap_or_default();
    let email = string_at(value, &["email"]);
    let name = string_at(value, &["name"]);
    let label = best_label(name.as_deref(), email.as_deref(), account_id.as_str());
    let alias = make_alias(email.as_deref().or(name.as_deref()).unwrap_or(&account_id));

    Some(ImportedAccount {
        alias,
        label,
        id_token,
        access_token,
        refresh_token,
        account_id,
        last_refresh: string_at(value, &["last_refresh"]),
        proxy_url: proxy_url_from_value(value),
    })
}

fn import_from_nested_account(value: &Value) -> Option<ImportedAccount> {
    let credentials = value.get("credentials").unwrap_or(value);
    let extra = value.get("extra");
    let access_token = string_at(credentials, &["access_token"])?;
    let refresh_token = string_at(credentials, &["refresh_token"])?;
    let id_token = string_at(credentials, &["id_token"]).unwrap_or_default();
    let account_id = string_at(credentials, &["account_id"])
        .or_else(|| string_at(credentials, &["chatgpt_account_id"]))
        .unwrap_or_default();
    let email = string_at(credentials, &["email"])
        .or_else(|| extra.and_then(|extra| string_at(extra, &["email"])))
        .or_else(|| extra.and_then(|extra| string_at(extra, &["email_key"])));
    let name =
        string_at(value, &["name"]).or_else(|| extra.and_then(|extra| string_at(extra, &["name"])));
    let label = best_label(name.as_deref(), email.as_deref(), account_id.as_str());
    let alias = make_alias(email.as_deref().or(name.as_deref()).unwrap_or(&account_id));

    Some(ImportedAccount {
        alias,
        label,
        id_token,
        access_token,
        refresh_token,
        account_id,
        last_refresh: extra.and_then(|extra| string_at(extra, &["last_refresh"])),
        proxy_url: proxy_url_from_value(value),
    })
}

/// Parse un blob de session web ChatGPT (chatgpt.com/api/auth/session).
///
/// Format : `{ "accessToken": "<jwt>", "sessionToken": "<jwe>",
/// "user": { "id", "name", "email", "idp" }, "account": { "id", "planType" },
/// "expires": "<iso>" }`.
///
/// Ce blob ne contient PAS de `refresh_token` OAuth : le compte cree ici est
/// utilisable jusqu'a l'expiration du JWT d'acces (~10 jours), sans
/// renouvellement possible cote pool/Codex CLI.
fn import_from_session_blob(value: &Value) -> Option<ImportedAccount> {
    let access_token = string_at(value, &["accessToken"])?;
    let user_value = value.get("user").unwrap_or(value);
    let account_value = value.get("account").unwrap_or(value);

    let account_id = string_at(account_value, &["id"])
        .or_else(|| string_at(value, &["account_id"]))
        .or_else(|| string_at(user_value, &["id"]))
        .unwrap_or_default();
    let email = string_at(user_value, &["email"]);
    let name = string_at(user_value, &["name"]);
    let label = best_label(name.as_deref(), email.as_deref(), &account_id);
    let alias = make_alias(email.as_deref().or(name.as_deref()).unwrap_or(&account_id));

    Some(ImportedAccount {
        alias,
        label,
        id_token: String::new(),
        access_token,
        refresh_token: String::new(),
        account_id,
        last_refresh: None,
        proxy_url: proxy_url_from_value(value),
    })
}

fn proxy_url_from_value(value: &Value) -> Option<String> {
    string_at(value, &["proxy_url"])
        .or_else(|| string_at(value, &["proxyUrl"]))
        .or_else(|| {
            value
                .get("proxy")
                .and_then(|proxy| string_at(proxy, &["url"]))
        })
        .or_else(|| {
            value
                .get("proxy")
                .and_then(|proxy| string_at(proxy, &["proxy_url"]))
        })
        .or_else(|| {
            value
                .get("proxy")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
}

fn best_label(name: Option<&str>, email: Option<&str>, account_id: &str) -> String {
    let base = name
        .filter(|value| !value.trim().is_empty())
        .or(email)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(account_id);
    format!("Pool {base}")
}

fn string_at(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }

    match current {
        Value::String(text) if !text.is_empty() => Some(text.to_string()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn make_alias(value: &str) -> String {
    let mut alias = String::new();

    for character in value.trim().to_ascii_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            alias.push(character);
        } else if !alias.ends_with('-') {
            alias.push('-');
        }
    }

    let alias = alias.trim_matches('-').chars().take(60).collect::<String>();
    if alias.is_empty() {
        "account".to_string()
    } else {
        alias
    }
}

fn expand_import_files(paths: &[String]) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    let mut seen = HashSet::new();

    for raw in paths {
        for part in raw.split(['\n', '\r', ';', ',']) {
            let value = part.trim().trim_matches('"').trim_matches('\'');
            if value.is_empty() {
                continue;
            }

            for file in expand_import_entry(value)? {
                let key = normalize_string_path(&file.to_string_lossy());
                if seen.insert(key) {
                    files.push(file);
                }
            }
        }
    }

    Ok(files)
}

fn expand_import_entry(value: &str) -> Result<Vec<PathBuf>, String> {
    let path = expand_known_path(value)?;

    if path.is_dir() {
        let mut files = Vec::new();
        for entry in fs::read_dir(&path).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let candidate = entry.path();
            if candidate
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| extension.eq_ignore_ascii_case("json"))
                .unwrap_or(false)
            {
                files.push(candidate);
            }
        }
        return Ok(files);
    }

    if value.contains('*') || value.contains('?') {
        return expand_wildcard(value);
    }

    if path.is_file() {
        Ok(vec![path])
    } else {
        Err(format!("Fichier introuvable: {value}"))
    }
}

fn expand_known_path(value: &str) -> Result<PathBuf, String> {
    if value == "~"
        || value.starts_with("~/")
        || value.starts_with("~\\")
        || value.starts_with("%USERPROFILE%")
    {
        return expand_home(value);
    }

    if let Some(stripped) = value.strip_prefix("%APPDATA%") {
        if let Some(appdata) = env::var_os("APPDATA") {
            return Ok(PathBuf::from(appdata).join(stripped.trim_start_matches(['\\', '/'])));
        }
    }

    Ok(PathBuf::from(value))
}

fn expand_wildcard(value: &str) -> Result<Vec<PathBuf>, String> {
    let path = expand_known_path(value)?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let pattern = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Wildcard invalide: {value}"))?;
    let mut files = Vec::new();

    for entry in fs::read_dir(parent).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let candidate = entry.path();
        let Some(name) = candidate.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if wildcard_match(pattern, name) && candidate.is_file() {
            files.push(candidate);
        }
    }

    Ok(files)
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let (mut pi, mut ti) = (0_usize, 0_usize);
    let mut star = None;
    let mut match_i = 0_usize;

    while ti < text.len() {
        if pi < pattern.len()
            && (pattern[pi] == b'?' || pattern[pi].eq_ignore_ascii_case(&text[ti]))
        {
            pi += 1;
            ti += 1;
        } else if pi < pattern.len() && pattern[pi] == b'*' {
            star = Some(pi);
            match_i = ti;
            pi += 1;
        } else if let Some(star_i) = star {
            pi = star_i + 1;
            match_i += 1;
            ti = match_i;
        } else {
            return false;
        }
    }

    while pi < pattern.len() && pattern[pi] == b'*' {
        pi += 1;
    }

    pi == pattern.len()
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

/// Horodatage ISO 8601 / RFC 3339 attendu par Codex CLI pour `last_refresh`
/// dans `auth.json` (ex. `2026-07-07T22:02:21.440539Z`). Un timestamp unix
/// casse le parseur de Codex ("input contains invalid characters").
fn now_iso8601() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// Synchronise (idempotent) les reglages Codex propres a un compte.
///
/// Le choix bypass est toujours materialise explicitement : le desactiver remet
/// `approval_policy = "on-request"` et `sandbox_mode = "workspace-write"`, ce
/// qui neutralise un ancien bypass persiste dans le meme `config.toml`. Le
/// modele et l'intensite ne sont touches que lorsqu'une valeur non vide est
/// fournie, afin de préserver les profils des versions anterieures (`None`).
///
/// Les autres entrees du fichier (dont `[mcp_servers.*]`) sont preservees et
/// l'ecriture est atomique. Les valeurs sont echappees comme des chaines TOML :
/// un nom de modele fourni par l'UI ne peut donc pas injecter une nouvelle cle.
pub fn ensure_codex_account_config(
    home: &Path,
    bypass: bool,
    model: Option<&str>,
    reasoning_effort: Option<&str>,
    fast_mode: bool,
) -> std::io::Result<()> {
    let model = model.map(str::trim).filter(|value| !value.is_empty());
    let reasoning_effort = reasoning_effort
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if let Some(effort) = reasoning_effort {
        if !is_valid_reasoning_effort(effort) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Intensite de raisonnement invalide: {effort}"),
            ));
        }
    }

    fs::create_dir_all(home)?;
    let path = home.join("config.toml");
    let existing = fs::read_to_string(&path).unwrap_or_default();

    let approval_policy = if bypass { "never" } else { "on-request" };
    let sandbox_mode = if bypass {
        "danger-full-access"
    } else {
        "workspace-write"
    };
    let mut updated = upsert_top_level_string(&existing, "approval_policy", approval_policy);
    updated = upsert_top_level_string(&updated, "sandbox_mode", sandbox_mode);
    if let Some(model) = model {
        updated = upsert_top_level_string(&updated, "model", model);
    }
    if let Some(effort) = reasoning_effort {
        updated = upsert_top_level_string(&updated, "model_reasoning_effort", effort);
    }
    updated = if fast_mode {
        upsert_top_level_string(&updated, "service_tier", "priority")
    } else {
        remove_top_level_key(&updated, "service_tier")
    };
    // Epingler Goals et les deux generations du moteur multi-agent rend le
    // comportement identique dans les chats `codex exec` et dans les terminaux
    // interactifs. `agents.enabled` expose les outils ; v2 persiste les fils de
    // sous-agents.
    updated = upsert_table_bool(&updated, "features", "goals", true);
    updated = upsert_table_bool(&updated, "features", "multi_agent", true);
    updated = upsert_table_bool(&updated, "features", "multi_agent_v2", true);
    updated = upsert_table_bool(&updated, "agents", "enabled", true);
    if let Some(windows_sandbox) = codex_windows_sandbox_override()? {
        updated = upsert_table_string(&updated, "windows", "sandbox", &windows_sandbox);
    }

    if updated == existing {
        return Ok(());
    }

    crate::fs_util::atomic_write(&path, updated)
}

/// Sur un noeud Windows sans operateur local (serveur web/Tailscale), le
/// sandbox natif `elevated` relance son programme d'installation avec UAC pour
/// chaque nouveau workspace. Le demarreur du noeud fixe donc explicitement le
/// mode `unelevated`, qui conserve l'isolation sans demander un administrateur.
/// Le desktop local ne definit pas cette variable et garde le choix Codex de
/// l'utilisateur.
fn codex_windows_sandbox_override() -> std::io::Result<Option<String>> {
    let Some(raw) = env::var_os("CST_CODEX_WINDOWS_SANDBOX") else {
        return Ok(None);
    };
    let value = raw.to_string_lossy().trim().to_ascii_lowercase();
    if value.is_empty() {
        return Ok(None);
    }
    if matches!(value.as_str(), "elevated" | "unelevated") {
        return Ok(Some(value));
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        "CST_CODEX_WINDOWS_SANDBOX doit valoir elevated ou unelevated",
    ))
}

/// Le catalogue Codex est la source de verite des valeurs disponibles. Cette
/// validation ne maintient donc pas de whitelist fonctionnelle : elle bloque
/// uniquement les valeurs dangereuses/mal formees avant l'ecriture TOML ou le
/// passage au CLI, ce qui rend les futurs efforts compatibles sans mise a jour.
pub(crate) fn is_valid_reasoning_effort(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_lowercase())
        && value.chars().count() <= 32
        && chars.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '_' | '-')
        })
}

/// Insere ou remplace une cle scalaire chaine AU NIVEAU RACINE d'un document TOML
/// (avant toute table `[section]`). Ne touche jamais une cle de meme nom situee
/// dans une table, une chaine multi-ligne (`"""`/`'''`), un tableau multi-ligne,
/// ni une ligne commentee. Si la cle est absente, elle est prefixee (une cle
/// racine doit preceder toute table pour rester du TOML valide).
///
/// Ce n'est pas un parseur TOML complet, mais il suit l'etat des chaines
/// multi-lignes et la profondeur des crochets pour ne pas confondre le CONTENU
/// d'une valeur avec une structure top-level (ce qui pourrait dupliquer une cle
/// et corrompre le fichier).
fn upsert_top_level_string(content: &str, key: &str, value: &str) -> String {
    let desired = format!("{key} = \"{}\"", escape_toml_basic_string(value));
    let mut out = String::with_capacity(content.len() + desired.len() + 1);
    let mut replaced = false;
    let mut in_table = false;
    // Etat de lexing, evalue au DEBUT de chaque ligne.
    let mut ml: Option<&'static str> = None; // chaine multi-ligne ouverte
    let mut depth: i32 = 0; // profondeur de crochets (tableaux multi-lignes)

    for line in content.lines() {
        // Une ligne n'est "top-level" que hors chaine multi-ligne et hors tableau.
        let at_top_level = ml.is_none() && depth == 0;
        let trimmed = line.trim_start();

        let mut is_target = false;
        if at_top_level {
            if trimmed.starts_with('[') {
                in_table = true;
            } else if !replaced
                && !in_table
                && !trimmed.starts_with('#')
                && trimmed
                    .strip_prefix(key)
                    .map(|rest| rest.trim_start().starts_with('='))
                    .unwrap_or(false)
            {
                is_target = true;
            }
        }

        if is_target {
            out.push_str(&desired);
            out.push('\n');
            replaced = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }

        advance_toml_lex(line, &mut ml, &mut depth);
    }

    if replaced {
        out
    } else if content.is_empty() {
        format!("{desired}\n")
    } else {
        format!("{desired}\n{content}")
    }
}

/// Retire toutes les occurrences effectives d'une cle scalaire racine sans
/// toucher aux commentaires, aux tables ni au contenu des valeurs multi-lignes.
fn remove_top_level_key(content: &str, key: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut removed = false;
    let mut in_table = false;
    let mut ml: Option<&'static str> = None;
    let mut depth: i32 = 0;

    for line in content.lines() {
        let at_top_level = ml.is_none() && depth == 0;
        let trimmed = line.trim_start();
        let is_target = if at_top_level {
            if trimmed.starts_with('[') {
                in_table = true;
                false
            } else {
                !in_table
                    && !trimmed.starts_with('#')
                    && trimmed
                        .strip_prefix(key)
                        .map(|rest| rest.trim_start().starts_with('='))
                        .unwrap_or(false)
            }
        } else {
            false
        };

        if is_target {
            removed = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
        advance_toml_lex(line, &mut ml, &mut depth);
    }

    if removed {
        out
    } else {
        content.to_string()
    }
}

/// Insere ou remplace une chaine dans une table TOML simple sans reserialiser
/// le document entier (les commentaires et les tables MCP restent intacts).
fn upsert_table_string(content: &str, table: &str, key: &str, value: &str) -> String {
    let desired = format!("{key} = \"{}\"", escape_toml_basic_string(value));
    upsert_table_value(content, table, key, &desired)
}

/// Insere ou remplace un booleen dans une table TOML simple en conservant les
/// commentaires et les autres sections du fichier.
fn upsert_table_bool(content: &str, table: &str, key: &str, value: bool) -> String {
    let desired = format!("{key} = {value}");
    upsert_table_value(content, table, key, &desired)
}

fn upsert_table_value(content: &str, table: &str, key: &str, desired: &str) -> String {
    let header = format!("[{table}]");
    let mut out = String::with_capacity(content.len() + header.len() + desired.len() + 4);
    let mut found_table = false;
    let mut in_target_table = false;
    let mut replaced = false;
    let mut ml: Option<&'static str> = None;
    let mut depth: i32 = 0;

    for line in content.lines() {
        let at_structural_level = ml.is_none() && depth == 0;
        let trimmed = line.trim_start();

        if at_structural_level && trimmed.starts_with('[') {
            if in_target_table && !replaced {
                out.push_str(desired);
                out.push('\n');
                replaced = true;
            }
            in_target_table = trimmed.trim_end() == header;
            found_table |= in_target_table;
        }

        let is_target = at_structural_level
            && in_target_table
            && !replaced
            && !trimmed.starts_with('#')
            && trimmed
                .strip_prefix(key)
                .map(|rest| rest.trim_start().starts_with('='))
                .unwrap_or(false);

        if is_target {
            out.push_str(desired);
            out.push('\n');
            replaced = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }

        advance_toml_lex(line, &mut ml, &mut depth);
    }

    if found_table && !replaced {
        out.push_str(desired);
        out.push('\n');
    } else if !found_table {
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        out.push_str(&header);
        out.push('\n');
        out.push_str(desired);
        out.push('\n');
    }

    out
}

/// Echappe une valeur pour une chaine TOML basique delimitee par `"`. Les
/// retours a la ligne restent ainsi dans la valeur et ne peuvent pas devenir
/// des cles TOML au niveau racine.
fn escape_toml_basic_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\u{0008}' => escaped.push_str("\\b"),
            '\t' => escaped.push_str("\\t"),
            '\n' => escaped.push_str("\\n"),
            '\u{000C}' => escaped.push_str("\\f"),
            '\r' => escaped.push_str("\\r"),
            control if control.is_control() => {
                let codepoint = control as u32;
                if codepoint <= 0xFFFF {
                    escaped.push_str(&format!("\\u{codepoint:04X}"));
                } else {
                    escaped.push_str(&format!("\\U{codepoint:08X}"));
                }
            }
            other => escaped.push(other),
        }
    }
    escaped
}

/// Fait avancer l'etat de lexing TOML (chaine multi-ligne ouverte, profondeur de
/// crochets) au fil d'une ligne. UTF-8 safe : toutes les decoupes se font sur des
/// frontieres de caracteres. Ne vise pas la conformite TOML complete, juste de
/// quoi distinguer le niveau racine du contenu des valeurs.
fn advance_toml_lex(line: &str, ml: &mut Option<&'static str>, depth: &mut i32) {
    let mut rest = line;
    loop {
        // Dans une chaine multi-ligne : on cherche sa fermeture.
        if let Some(delim) = *ml {
            match rest.find(delim) {
                Some(pos) => {
                    *ml = None;
                    rest = &rest[pos + delim.len()..];
                }
                None => return,
            }
            continue;
        }

        let Some(c) = rest.chars().next() else {
            return;
        };

        // Ouverture d'une chaine multi-ligne ?
        if rest.starts_with("\"\"\"") || rest.starts_with("'''") {
            let delim = if rest.starts_with("\"\"\"") {
                "\"\"\""
            } else {
                "'''"
            };
            let after_open = &rest[3..];
            match after_open.find(delim) {
                Some(pos) => rest = &after_open[pos + 3..], // ouverte + fermee ici
                None => {
                    *ml = Some(delim);
                    return;
                }
            }
            continue;
        }

        match c {
            // Commentaire : le reste de la ligne est ignore.
            '#' => return,
            // Chaine simple ligne : consommee jusqu'a sa fermeture (ou fin de ligne).
            '"' | '\'' => {
                let after = &rest[c.len_utf8()..];
                match after.find(c) {
                    Some(pos) => rest = &after[pos + c.len_utf8()..],
                    None => return,
                }
            }
            '[' => {
                *depth += 1;
                rest = &rest[1..];
            }
            ']' => {
                if *depth > 0 {
                    *depth -= 1;
                }
                rest = &rest[1..];
            }
            _ => rest = &rest[c.len_utf8()..],
        }
    }
}

fn normalize_string_path(value: &str) -> String {
    value.trim().replace('/', "\\").to_ascii_lowercase()
}

/// Identite physique d'un home de compte. Les alias utilisateur et data sont
/// developpes avant comparaison, puis un lien symbolique/jonction existant est
/// resolu afin que deux syntaxes du meme dossier produisent la meme cle.
fn account_home_key(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let expanded = expand_home_local(trimmed).unwrap_or_else(|_| PathBuf::from(trimmed));
    let resolved = fs::canonicalize(&expanded).unwrap_or(expanded);
    let normalized = resolved
        .to_string_lossy()
        .trim()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_string();
    if cfg!(windows) {
        normalized.to_ascii_lowercase()
    } else {
        normalized
    }
}

fn strip_ascii_case_prefix<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    value
        .get(..prefix.len())
        .filter(|candidate| candidate.eq_ignore_ascii_case(prefix))
        .map(|_| &value[prefix.len()..])
}

fn cst_data_suffix(value: &str) -> Option<&str> {
    ["%CST_DATA_DIR%", "${CST_DATA_DIR}", "$CST_DATA_DIR"]
        .into_iter()
        .find_map(|prefix| strip_ascii_case_prefix(value, prefix))
}

fn expand_cst_data_path(stripped: &str) -> Result<PathBuf, String> {
    let base = env::var_os("CST_DATA_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| "CST_DATA_DIR n'est pas defini".to_string())?;
    Ok(base.join(stripped.trim_start_matches(['\\', '/'])))
}

/// Developpe un chemin sans jamais le rediriger vers une autre instance.
/// Reserve aux operations d'ecriture/destruction et aux cles de deduplication.
fn expand_home_local(value: &str) -> Result<PathBuf, String> {
    if value == "~" {
        return home_dir();
    }

    if let Some(stripped) = cst_data_suffix(value) {
        return expand_cst_data_path(stripped);
    }

    if let Some(stripped) = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
    {
        return Ok(home_dir()?.join(stripped));
    }

    if let Some(stripped) = strip_ascii_case_prefix(value, "%USERPROFILE%") {
        return Ok(home_dir()?.join(stripped.trim_start_matches(['\\', '/'])));
    }

    #[cfg(target_os = "linux")]
    if is_wsl() {
        if let Some(path) = windows_drive_path_in_wsl(value) {
            return Ok(path);
        }
    }

    Ok(PathBuf::from(value))
}

/// Developpe un home de lecture/execution. Pour les comptes serveur ranges sous
/// `CST_DATA_DIR/codex-homes`, une copie authentifiee d'une instance locale
/// soeur est acceptee lorsque la copie locale est absente et que l'identite du
/// compte est sans ambiguite. Tous les autres chemins restent strictement
/// locaux.
pub fn expand_home(value: &str) -> Result<PathBuf, String> {
    let primary = expand_home_local(value)?;
    let Some(stripped) = cst_data_suffix(value) else {
        return Ok(primary);
    };
    let base = env::var_os("CST_DATA_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| "CST_DATA_DIR n'est pas defini".to_string())?;
    let relative = PathBuf::from(stripped.trim_start_matches(['\\', '/']));
    Ok(crate::account_usage::resolve_data_dir_account_home(
        &base, &relative, primary,
    ))
}

#[cfg(target_os = "linux")]
fn is_wsl() -> bool {
    env::var_os("WSL_DISTRO_NAME").is_some()
        || fs::read_to_string("/proc/sys/kernel/osrelease")
            .map(|value| value.to_ascii_lowercase().contains("microsoft"))
            .unwrap_or(false)
}

/// Convertit `C:\\Users\\...` ou `C:/Users/...` en `/mnt/c/Users/...`.
/// La fonction reste pure pour pouvoir tester la conversion hors d'un montage
/// WSL reel ; l'appelant verifie lui-meme qu'il tourne bien sous WSL.
#[cfg(target_os = "linux")]
fn windows_drive_path_in_wsl(value: &str) -> Option<PathBuf> {
    let value = value.trim().trim_matches('"');
    let bytes = value.as_bytes();
    if bytes.len() < 3
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || !matches!(bytes[2], b'\\' | b'/')
    {
        return None;
    }
    let drive = (bytes[0] as char).to_ascii_lowercase().to_string();
    let remainder = value[3..].replace('\\', "/");
    Some(PathBuf::from("/mnt").join(drive).join(remainder))
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn pick_project_dir(current_dir: Option<String>) -> Result<Option<String>, String> {
    let start_dir = current_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| expand_home(value).ok())
        .filter(|path| path.is_dir());

    let selected = tokio::task::spawn_blocking(move || {
        let mut dialog = rfd::FileDialog::new();
        if let Some(dir) = start_dir {
            dialog = dialog.set_directory(dir);
        }
        dialog
            .pick_folder()
            .map(|path| path.to_string_lossy().to_string())
    })
    .await
    .map_err(|error| error.to_string())?;

    Ok(selected)
}

fn home_dir() -> Result<PathBuf, String> {
    env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
        .ok_or_else(|| "Impossible de trouver le dossier utilisateur".to_string())
}

fn default_shell() -> String {
    if cfg!(windows) {
        "powershell.exe".to_string()
    } else {
        env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    }
}

#[cfg(test)]
mod delete_home_tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn windows_drive_paths_map_to_their_wsl_mount() {
        assert_eq!(
            windows_drive_path_in_wsl(r"C:\Users\JeanP\Project"),
            Some(PathBuf::from("/mnt/c/Users/JeanP/Project"))
        );
        assert_eq!(
            windows_drive_path_in_wsl("d:/work/repo"),
            Some(PathBuf::from("/mnt/d/work/repo"))
        );
        assert_eq!(windows_drive_path_in_wsl("relative/repo"), None);
    }

    /// Base temporaire unique par test (pas de `Date`/`rand` : nom fixe + nettoyage).
    fn scratch(tag: &str) -> PathBuf {
        let base = env::temp_dir()
            .join("cst-remove-account-tests")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).expect("create scratch dir");
        base
    }

    #[test]
    fn guard_accepts_codex_like_dir_name() {
        let base = scratch("codex-like");
        let dir = base.join(".codex-pool-alpha");
        fs::create_dir_all(&dir).unwrap();
        assert!(guard_deletable_codex_home(&dir).is_ok());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn guard_accepts_dir_with_auth_marker() {
        let base = scratch("marker");
        // Nom quelconque, mais contient un auth.json => reconnu comme CODEX_HOME.
        let dir = base.join("some-random-name");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("auth.json"), "{}").unwrap();
        assert!(guard_deletable_codex_home(&dir).is_ok());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn guard_accepts_dir_under_codex_homes() {
        let base = scratch("codex-homes");
        let dir = base.join("codex-homes").join("pool-beta");
        fs::create_dir_all(&dir).unwrap();
        assert!(guard_deletable_codex_home(&dir).is_ok());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn guard_rejects_non_codex_dir() {
        let base = scratch("non-codex");
        let dir = base.join("my-documents");
        fs::create_dir_all(&dir).unwrap();
        assert!(guard_deletable_codex_home(&dir).is_err());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn guard_rejects_user_home() {
        let home = home_dir().expect("home dir");
        assert!(guard_deletable_codex_home(&home).is_err());
    }

    #[test]
    fn guard_rejects_ancestor_of_home() {
        let home = home_dir().expect("home dir");
        if let Some(parent) = home.parent() {
            assert!(guard_deletable_codex_home(parent).is_err());
        }
    }

    #[test]
    fn delete_missing_dir_is_ok() {
        let base = scratch("missing");
        let missing = base.join(".codex-does-not-exist");
        assert!(delete_codex_home_dir(&missing.to_string_lossy()).is_ok());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn delete_removes_codex_home_dir() {
        let base = scratch("delete-ok");
        let dir = base.join(".codex-pool-gamma");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("auth.json"), "{}").unwrap();
        fs::create_dir_all(dir.join("sessions")).unwrap();

        delete_codex_home_dir(&dir.to_string_lossy()).expect("delete should succeed");
        assert!(!dir.exists());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn delete_refuses_and_keeps_non_codex_dir() {
        let base = scratch("delete-refuse");
        let dir = base.join("important-stuff");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("data.txt"), "keep me").unwrap();

        assert!(delete_codex_home_dir(&dir.to_string_lossy()).is_err());
        assert!(dir.exists(), "un dossier refuse ne doit pas etre efface");
        let _ = fs::remove_dir_all(&base);
    }
}
