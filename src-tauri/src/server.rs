use crate::{
    account_usage,
    auth::{self, AuthIdentity, AuthManager},
    autonomous::{
        AddAutonomousMemoryRequest, ApplyAutonomousReviewPolicyRequest, AutonomousAgentAction,
        AutonomousAgentManager, AutonomousAgentSnapshot, AutonomousAgentStatus,
        ControlAutonomousAgentRequest, CreateAutonomousAgentRequest,
        ReassignAutonomousAgentAccountRequest, ScheduleAutonomousAgentRequest,
        SendAutonomousAgentMessageRequest, UpdateAutonomousAgentRequest,
    },
    chat::{ChatTurnManager, StartChatTurnRequest, MAX_CHAT_TURN_REQUEST_BYTES},
    chat_model_tools::{
        self, ApplyAutonomousAgentPolicyToolArguments, AutonomousAgentToolContext,
        ChatModelToolServerConfig, ChatOpenRequestRegistry, ChatPostRequestRegistry,
        ChatToolCapabilityRegistry, ChatToolScope, CreateAutonomousAgentToolArguments,
        CreateAutonomousGoalToolArguments,        CreateChatToolArguments, CreateTerminalGoalToolArguments, SendChatMessageToolArguments,
        UpdateAutonomousAgentToolArguments, UpdateGoalToolArguments,
        ACTIVATE_SUPERVISOR_GENERAL_REPORT_TOOL_NAME, APPLY_AUTONOMOUS_AGENT_POLICY_TOOL_NAME,
        AUTONOMOUS_AGENT_TOOL_NAME, CONTROL_DEVICE_TOOL_NAME,
        CONTROL_PRIVATE_MESSAGE_CAMPAIGN_TOOL_NAME, CREATE_CALENDAR_EVENT_TOOL_NAME,
        CREATE_CHAT_TOOL_NAME, CREATE_GOAL_TOOL_NAME, CREATE_PRIVATE_MESSAGE_CAMPAIGN_TOOL_NAME,
        GET_CONTROL_DEVICE_ACTION_TOOL_NAME, GET_GOAL_TOOL_NAME, LIST_CALENDAR_EVENTS_TOOL_NAME,
        LIST_CONTROL_DEVICES_TOOL_NAME, LIST_OUTLOOK_MESSAGES_TOOL_NAME,
        LIST_PRIVATE_MESSAGE_CAMPAIGNS_TOOL_NAME, LIST_PRIVATE_MESSAGE_USERS_TOOL_NAME,
        LIST_TIKTOK_DM_CAMPAIGNS_TOOL_NAME, LIST_TIKTOK_FOLLOWER_EXTRACTIONS_TOOL_NAME,
        LIST_TIKTOK_SENDER_ACCOUNTS_TOOL_NAME, MANAGE_TIKTOK_SENDER_LOGIN_TOOL_NAME,
        PAUSE_AUTONOMOUS_AGENT_TOOL_NAME, PREPARE_TIKTOK_DM_CAMPAIGN_TOOL_NAME,
        QUEUE_TIKTOK_FOLLOWER_EXTRACTION_TOOL_NAME, SELECT_TIKTOK_SENDER_ACCOUNT_TOOL_NAME,
        SEND_CHAT_MESSAGE_TOOL_NAME, SEND_OUTLOOK_EMAIL_TOOL_NAME,
        SEND_TIKTOK_DM_CAMPAIGN_TOOL_NAME, UPDATE_AUTONOMOUS_AGENT_TOOL_NAME,
        UPDATE_CALENDAR_EVENT_TOOL_NAME, UPDATE_GOAL_TOOL_NAME,
    },
    creative_accounts::{self, ConnectCreativeAccountRequest, CreativeAccountIdRequest},
    freebuff_cloud::{self, AgentRunStreamQuery, ConnectFreebuffCloudRequest, ConnectRepoRequest, CreateBlankProjectRequest, DeleteProjectRequest},
    device_fleet::{
        self, DeviceActionKind, DeviceActionRecord, DeviceActionRequest, DeviceActionResult,
        DeviceActionStatus, DeviceConnectorClaimRequest, DeviceConnectorClaimResponse,
        DeviceConnectorHeartbeatRequest, DeviceConnectorReportRequest, DeviceFleetError,
        DeviceFleetManager,
    },
    discussions,
    doctolib_lab::{self, DoctolibLabManager, DoctolibLabSearchRequest},
    duello_bank::{
        CreditDuelloWalletRequest, DuelloBankClient, DuelloBankConfig, DuelloBankError,
        DuelloBankErrorKind,
    },
    forum::{ForumAuthor, ForumError, ForumManager},
    git_docker_environment::{self, CreateGitDockerEnvironmentRequest},
    image_generation::{self, ImageGenerationRequest, ImageGenerationStatusRequest},
    kombai::{KombaiManager, KombaiStatus},
    metrics,
    gmail::{self, GmailManager},
    microsoft::{
        self, CreateEventArguments, ListEventsArguments, ListMessagesArguments, MicrosoftManager,
        SendEmailArguments, UpdateEventArguments,
    },
    mobile_push::{self, ConfigureMobilePushRequest, RegisterMobilePushDeviceRequest},
    orchestration::{
        ControlOrchestrationRequest, CreateOrchestrationRequest, OrchestrationManager,
        PromoteAutonomousAgentRequest, ReassignOrchestrationAccountRequest,
    },
    pool::{self, AccountStatus, PoolManager},
    private_messages::{
        CreatePrivateMessageCampaignRequest, PrivateMessageCampaignAction, PrivateMessageError,
        PrivateMessageImageRequest, PrivateMessageManager, PrivateMessageUser,
        MAX_PRIVATE_MESSAGE_REQUEST_BYTES,
    },
    runtime_sync::{RuntimeSync, RuntimeSyncTopic},
    settings::{self, AccountProfile, AppSettings, Provider},
    telegram_notifications::{
        self, ConnectTelegramManagerRequest, ConnectTelegramRequest,
        PrepareManagedTelegramBotRequest,
    },
    terminal_goal::{terminal_goal_key, TerminalGoalManager},
    tiktok_messaging::{
        ConfirmTikTokDmCampaignRequest, PrepareTikTokDmCampaignRequest,
        QueueTikTokFollowerExtractionRequest, QueueTikTokSenderSetupRequest,
        TikTokConnectorClaimRequest, TikTokConnectorHeartbeatRequest, TikTokConnectorReportRequest,
        TikTokDmCampaign, TikTokDmCampaignStatus, TikTokDmError, TikTokDmManager,
        TikTokFollowerResultReportRequest, TikTokFollowerSubmissionReportRequest,
        TikTokSenderSetupActionKind, TikTokSenderSetupReportRequest,
    },
    referral::{CreateReferralCodeRequest, ReferralManager},
    tracking::{CreateTrackingLinkRequest, TrackingManager},
    video_generation::{self, VideoGenerationRequest, VideoGenerationStatusRequest},
    voice,
    vps_deploy::{StartVpsDeployRequest, VpsDeployError, VpsDeployManager},
    whatsapp_notifications::{self, ConnectWhatsAppRequest},
    work_time,
    workspace_access::{WorkspaceAccessError, WorkspaceAccessErrorKind, WorkspaceAccessManager},
};
use axum::{
    body::{Body, Bytes},
    extract::{
        rejection::JsonRejection,
        ws::{Message, WebSocket, WebSocketUpgrade},
        DefaultBodyLimit, Path as AxumPath, Query, Request, State,
    },
    http::{
        header::{CACHE_CONTROL, CONTENT_TYPE},
        HeaderMap, HeaderValue, StatusCode,
    },
    middleware::{self, Next},
    response::{IntoResponse, Redirect, Response},
    routing::{delete, get, post},
    Json, Router,
};
use futures_util::{SinkExt, StreamExt};
use portable_pty::{
    Child as PtyChild, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, SystemTime},
};
use tokio::sync::broadcast;
use tower_http::{compression::CompressionLayer, cors::CorsLayer, services::ServeDir};
use uuid::Uuid;

const WORKSPACE_RETENTION_SECS: u64 = 7 * 24 * 60 * 60;
const DEFAULT_DRAIN_LEASE_SECS: u64 = 20;
const MAX_DRAIN_LEASE_SECS: u64 = 60;
/// Ces secrets appartiennent uniquement au processus serveur. Les laisser dans
/// son environnement permettrait aux providers et terminaux qu'il lance de les
/// heriter, notamment le jeton capable d'activer `/api/admin/drain`.
const SERVER_CONTROL_SECRET_ENV_VARS: [&str; 3] = [
    "CST_ADMIN_TOKEN",
    "CST_GIT_PAT",
    "CST_DUELLO_BANK_ADMIN_TOKEN",
];
// Le receiver conserve les sorties produites entre le demarrage du PTY et
// l'ouverture du WebSocket par le navigateur. Une capacite genereuse evite de
// perdre l'ecran ANSI initial d'une TUI telle que Codex.
// 2048 blocs de 8 Kio couvrent environ 16 Mio de sortie en attente par
// terminal. La hausse reduit fortement les evenements jetes (`RecvError::Lagged`)
// pendant les rafales (builds, npm, TUI) qui se traduisaient par des « sauts »
// ou des gels d'affichage. La memoire n'est allouee que si le consommateur
// WebSocket est en retard, donc le cout au repos reste nul.
const TERMINAL_EVENT_BUFFER: usize = 2048;
// Regroupe uniquement les fragments PTY deja disponibles. Cette limite borne
// la taille d'une trame WebSocket sans ajouter de temporisation a l'echo du
// terminal. Un fragment qui ferait depasser la limite reste en attente pour la
// trame suivante.
const TERMINAL_WS_DATA_BATCH_BYTES: usize = 64 * 1024;

/// Decode un flux d'octets en chaines UTF-8 valides en conservant, d'un read a
/// l'autre, la fin d'une sequence multi-octets coupee par la taille du buffer
/// du PTY. Sans cela, `from_utf8_lossy` insere un U+FFFD a chaque sequence
/// coupee (accents/emojis corrompus).
fn incremental_utf8_decoder() -> impl FnMut(&[u8]) -> String {
    let mut carry: Vec<u8> = Vec::new();
    move |chunk: &[u8]| {
        let mut buf = std::mem::take(&mut carry);
        buf.extend_from_slice(chunk);
        let mut remaining = buf.as_slice();
        let mut output = String::new();

        loop {
            match std::str::from_utf8(remaining) {
                Ok(valid) => {
                    output.push_str(valid);
                    break;
                }
                Err(error) => {
                    let valid_up_to = error.valid_up_to();
                    // SAFETY: `valid_up_to` est garanti etre une frontiere UTF-8
                    // valide par `std::str::from_utf8`.
                    output.push_str(unsafe {
                        std::str::from_utf8_unchecked(&remaining[..valid_up_to])
                    });
                    match error.error_len() {
                        Some(invalid_len) => {
                            output.push('\u{FFFD}');
                            remaining = &remaining[valid_up_to + invalid_len..];
                        }
                        None => {
                            // Une sequence incomplete peut suivre un octet
                            // invalide : seule cette queue doit attendre le
                            // prochain read.
                            carry.extend_from_slice(&remaining[valid_up_to..]);
                            break;
                        }
                    }
                }
            }
        }

        output
    }
}

/// Version du binaire, exposee par `/healthz`, `/api/health` et `--version`.
/// Un tour lance avec le jeton administrateur n'a pas de compte nominatif :
/// aucune boite Microsoft ne peut alors etre choisie sans deviner, et deviner
/// reviendrait a envoyer un e-mail depuis la boite de quelqu'un d'autre.
const MICROSOFT_NO_NOMINAL_USER: &str = "Ce chat n'est rattache a aucun compte utilisateur nominatif : la boite Microsoft ne peut pas etre determinee. Demande a l'utilisateur de se connecter a l'application, puis de lier son compte Microsoft dans les parametres.";

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Commit git embarque au build (voir `build.rs`). "unknown" si indisponible.
pub const COMMIT: &str = match option_env!("CST_GIT_COMMIT") {
    Some(value) => value,
    None => "unknown",
};

#[derive(Debug, Clone)]
pub struct ServerConfig {
    bind: String,
    data_dir: PathBuf,
    static_dir: PathBuf,
    admin_token: String,
    git_pat: String,
    public_base_url: String,
    node_id: String,
    node_label: String,
    node_capacity: usize,
    terminal_capacity: usize,
    /// Racine autorisee pour le navigateur de dossiers et les workspaces
    /// pointant un dossier EXISTANT (`workspacePath`). Definie via
    /// `CST_WORKSPACES_ROOT` (defaut : dossier personnel). Toute navigation ou
    /// selection en dehors de cette racine est refusee. Stockee sous forme
    /// canonique pour une comparaison de prefixe fiable.
    workspaces_root: PathBuf,
    /// Endpoint de l'application Duello qui renvoie le nombre de personnes
    /// parrainees par code (`CST_DUELLO_REFERRAL_API_URL`). Vide = suivi inactif.
    duello_referral_api_url: Option<String>,
    /// Jeton optionnel envoye en `Authorization: Bearer` a l'endpoint Duello.
    duello_referral_api_key: Option<String>,
    /// Base publique de l'application Duello pour construire le lien
    /// `{appUrl}?ref={code}` (`CST_DUELLO_APP_URL`).
    duello_app_url: String,
    /// Acces serveur uniquement au grand livre Duello. Le jeton est lu avant le
    /// runtime puis retire de l'environnement des terminaux enfants.
    duello_bank: DuelloBankConfig,
}

#[derive(Clone)]
struct ServerState {
    config: ServerConfig,
    auth: AuthManager,
    terminals: RemoteTerminalManager,
    chat: ChatTurnManager,
    chat_tool_capabilities: ChatToolCapabilityRegistry,
    chat_open_requests: ChatOpenRequestRegistry,
    chat_post_requests: ChatPostRequestRegistry,
    microsoft: MicrosoftManager,
    gmail: GmailManager,
    autonomous: AutonomousAgentManager,
    terminal_goals: TerminalGoalManager,
    orchestration: OrchestrationManager,
    forum: ForumManager,
    private_messages: PrivateMessageManager,
    tiktok_messaging: TikTokDmManager,
    device_fleet: DeviceFleetManager,
    tracking: TrackingManager,
    referral: ReferralManager,
    duello_bank: DuelloBankClient,
    workspace_access: WorkspaceAccessManager,
    doctolib_lab: Arc<DoctolibLabManager>,
    kombai: Arc<KombaiManager>,
    kombai_owner: Arc<Mutex<Option<String>>>,
    vps_deploy: VpsDeployManager,
    started_at: i64,
    /// Echeance Unix de la courte lease de drain. Une lease bornee evite qu'un
    /// updater interrompu laisse le noeud ferme aux autres agents. Les sessions
    /// deja ouvertes continuent et un redemarrage repart toujours non draine.
    drain_until: Arc<AtomicI64>,
    /// Coordination du nettoyage declenche depuis l'interface web : le serveur
    /// ne fait que porter la demande et le dernier resultat ; le travail reel
    /// est execute par le gardien Windows qui surveille `/healthz`.
    cleanup: Arc<CleanupCoordinator>,
}

/// Demande de nettoyage en attente + dernier resultat publie par l'agent
/// Windows. Tout est volatile (en memoire) : apres un redemarrage, aucun
/// nettoyage n'est en attente et il n'y a plus de resultat a montrer.
#[derive(Default)]
struct CleanupCoordinator {
    requested_at: AtomicI64,
    last_result: std::sync::Mutex<Option<serde_json::Value>>,
}

#[derive(Debug, Clone)]
enum RequestActor {
    Administrator,
    User(AuthIdentity),
}

impl RequestActor {
    fn owner_id(&self) -> &str {
        match self {
            Self::Administrator => "server-admin",
            Self::User(identity) => &identity.id,
        }
    }

    fn user(&self) -> Option<&AuthIdentity> {
        match self {
            Self::Administrator => None,
            Self::User(identity) => Some(identity),
        }
    }

    fn is_administrator(&self) -> bool {
        matches!(self, Self::Administrator)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartTerminalRequest {
    id: Option<u64>,
    account_id: String,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    source_terminal_key: Option<String>,
    /// Depot Git a cloner normalement sur le serveur. Les clones inutilises
    /// depuis sept jours sont nettoyes. Ignore si `workspace_path` est fourni.
    #[serde(default)]
    repo_url: Option<String>,
    /// Dossier EXISTANT sur le serveur (dans la racine autorisee) a utiliser
    /// directement comme cwd. Prioritaire sur `repo_url` ; jamais clone ni purge.
    #[serde(default)]
    workspace_path: Option<String>,
    branch: Option<String>,
    cols: u16,
    rows: u16,
    command: Option<String>,
    #[serde(default)]
    login_only: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StartTerminalResponse {
    id: u64,
    workspace_id: String,
    workspace_path: String,
}

/// Les anciennes pages web peuvent rester ouvertes pendant un deploiement et
/// continuer d'envoyer le login OAuth classique. Sur un noeud serveur, son
/// callback localhost revient vers le navigateur de l'utilisateur et laisse le
/// terminal bloque. Le serveur corrige donc les deux commandes Codex integrees
/// historiques, sans toucher aux commandes personnalisees ni aux autres
/// providers.
fn normalize_remote_login_command(provider: Provider, command: Option<String>) -> Option<String> {
    let command = command?;
    if provider != Provider::Codex {
        return Some(command);
    }

    match command.trim() {
        "codex login" => Some("codex login --device-auth".to_string()),
        "codex logout; codex login" => Some("codex logout; codex login --device-auth".to_string()),
        _ => Some(command),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthResponse {
    ok: bool,
    node_id: String,
    node_label: String,
    public_base_url: String,
    version: &'static str,
    commit: &'static str,
    ready: bool,
    draining: bool,
    active_terminals: usize,
    active_chat_turns: usize,
    available_account_ids: Vec<String>,
    capacity: usize,
    terminal_capacity: usize,
    started_at: i64,
}

/// Liveness minimale NON authentifiee (`GET /healthz`), pour l'updater,
/// l'orchestrateur rolling et le routage client. Aucun secret : ni token, ni
/// compte, ni usage.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LivenessResponse {
    ok: bool,
    node_id: String,
    version: &'static str,
    commit: &'static str,
    ready: bool,
    draining: bool,
    active_terminals: usize,
    active_chat_turns: usize,
    capacity: usize,
    terminal_capacity: usize,
    /// Vrai tant qu'un nettoyage demande depuis l'interface web n'a pas encore
    /// ete execute par le gardien Windows. Lu sans authentification par le
    /// gardien, comme le reste de `/healthz` : aucun secret ici.
    cleanup_pending: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DrainRequest {
    draining: bool,
    /// Lease courte, renouvelee par l'appelant si necessaire. Bornee cote
    /// serveur pour qu'un agent mort ne puisse pas verrouiller le noeud.
    #[serde(default)]
    ttl_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ImportAccountRequest {
    content: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnsureAccountHomeRequest {
    codex_home: String,
    /// Absent => Codex (retro-compat des clients existants).
    #[serde(default)]
    provider: Option<settings::Provider>,
    #[serde(default = "default_true")]
    bypass: bool,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    reasoning_effort: Option<String>,
    #[serde(default)]
    fast_mode: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct WriteTerminalRequest {
    data: String,
}

#[derive(Debug, Deserialize)]
struct ResizeTerminalRequest {
    cols: u16,
    rows: u16,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KombaiStartRequest {
    project_dir: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DoctolibLabConfirmRequest {
    proposal_id: String,
    #[serde(default)]
    add_to_google_calendar: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CopyDiscussionRequest {
    session_id: String,
    source_account_id: String,
    target_account_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MoveDiscussionRequest {
    account_id: String,
    session_id: String,
    workspace_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenameDiscussionRequest {
    account_id: String,
    session_id: String,
    title: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaimSessionRequest {
    account_id: String,
    #[serde(default)]
    terminal_id: Option<u64>,
    after_unix: i64,
    #[serde(default)]
    exclude_session_ids: Vec<String>,
    #[serde(default)]
    match_session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteDiscussionRequest {
    account_id: String,
    session_id: String,
    #[serde(default)]
    archive: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompactChatSessionRequest {
    account_id: String,
    session_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportDiscussionRequest {
    account_id: String,
    session_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceView {
    id: String,
    path: String,
    modified_at: Option<i64>,
    retained_until: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FsEntry {
    name: String,
    path: String,
    is_dir: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FsListResponse {
    /// Racine autorisee (borne haute de navigation).
    root: String,
    /// Dossier courant liste.
    path: String,
    /// Dossier parent, ou `null` si `path` est deja la racine.
    parent: Option<String>,
    entries: Vec<FsEntry>,
}

#[derive(Debug, Deserialize)]
struct FsListQuery {
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateWorkspaceRequest {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestWorkspaceAccessRequest {
    share_code: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum ServerWsMessage {
    Data {
        id: u64,
        data: String,
    },
    Exit {
        id: u64,
    },
    Error {
        id: u64,
        message: String,
    },
    Status {
        id: u64,
        status: String,
        #[serde(rename = "workspaceId")]
        workspace_id: String,
        #[serde(rename = "workspacePath")]
        workspace_path: String,
    },
    Pong {
        id: u64,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum ClientWsMessage {
    Input { data: String },
    Resize { cols: u16, rows: u16 },
    Stop,
    Ping,
}

#[derive(Clone)]
struct RemoteTerminalManager {
    sessions: Arc<Mutex<HashMap<u64, Arc<RemoteTerminalSession>>>>,
    reservations: Arc<Mutex<HashSet<u64>>>,
    starting_freebuff_accounts: Arc<Mutex<HashSet<String>>>,
    device_terminal_tokens: Arc<Mutex<HashMap<String, DeviceTerminalCapability>>>,
    device_fleet: DeviceFleetManager,
    next_id: Arc<AtomicU64>,
    max_active: usize,
    runtime_sync: RuntimeSync,
    goal_tool_capabilities: Option<ChatToolCapabilityRegistry>,
    goal_tools_url: Option<String>,
}

impl Default for RemoteTerminalManager {
    fn default() -> Self {
        Self::with_max_active(crate::resource_profile::configured_terminal_capacity())
    }
}

struct TerminalEventCursor {
    receiver: broadcast::Receiver<ServerWsMessage>,
    prefetched: Option<ServerWsMessage>,
}

struct RemoteTerminalSession {
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    child: Mutex<Box<dyn portable_pty::Child + Send>>,
    events: broadcast::Sender<ServerWsMessage>,
    pending_events: Mutex<Option<TerminalEventCursor>>,
    socket_generation: AtomicU64,
    started_at: i64,
    owner_id: String,
    account_id: String,
    account_label: String,
    agent_id: Option<String>,
    source_terminal_key: Option<String>,
    workspace_id: String,
    workspace_path: PathBuf,
    login_only: bool,
    device_terminal_capability: Option<ActiveDeviceTerminalCapability>,
    device_terminal_tokens: Arc<Mutex<HashMap<String, DeviceTerminalCapability>>>,
    device_fleet: DeviceFleetManager,
    recorded_end: AtomicBool,
    /// Lease du bearer MCP Freebuff. La retirer revoque immediatement la
    /// capacite, meme si un WebSocket conserve encore un Arc de session.
    goal_tool_lease: Mutex<Option<TerminalGoalToolLease>>,
}

struct TerminalGoalToolLease {
    server: ChatModelToolServerConfig,
    registry: ChatToolCapabilityRegistry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeviceTerminalCapability {
    owner_id: String,
    origin_id: String,
    terminal_id: u64,
}

#[derive(Debug, Clone)]
struct ActiveDeviceTerminalCapability {
    token: String,
    context: DeviceTerminalCapability,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DeviceActionAccess {
    OwnerAll,
    OriginOnly(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeviceActionContext {
    owner_id: String,
    access: DeviceActionAccess,
}

impl DeviceTerminalCapability {
    fn action_context(&self) -> DeviceActionContext {
        DeviceActionContext {
            owner_id: self.owner_id.clone(),
            access: DeviceActionAccess::OriginOnly(self.origin_id.clone()),
        }
    }
}

impl DeviceActionContext {
    fn queue_action(
        &self,
        device_fleet: &DeviceFleetManager,
        request: DeviceActionRequest,
    ) -> Result<DeviceActionRecord, DeviceFleetError> {
        match &self.access {
            DeviceActionAccess::OwnerAll => device_fleet.queue_action(&self.owner_id, request),
            DeviceActionAccess::OriginOnly(origin_id) => {
                device_fleet.queue_ephemeral_action(&self.owner_id, origin_id, request)
            }
        }
    }

    fn action_status(
        &self,
        device_fleet: &DeviceFleetManager,
        action_id: &str,
    ) -> Result<DeviceActionRecord, DeviceFleetError> {
        match &self.access {
            DeviceActionAccess::OwnerAll => device_fleet.action_status(&self.owner_id, action_id),
            DeviceActionAccess::OriginOnly(origin_id) => {
                device_fleet.action_status_for_origin(&self.owner_id, origin_id, action_id)
            }
        }
    }

    async fn wait_action(
        &self,
        device_fleet: &DeviceFleetManager,
        action_id: &str,
        timeout: Duration,
    ) -> Result<DeviceActionRecord, DeviceFleetError> {
        match &self.access {
            DeviceActionAccess::OwnerAll => {
                device_fleet
                    .wait_action(&self.owner_id, action_id, timeout)
                    .await
            }
            DeviceActionAccess::OriginOnly(origin_id) => {
                device_fleet
                    .wait_action_for_origin(&self.owner_id, origin_id, action_id, timeout)
                    .await
            }
        }
    }
}

impl Drop for TerminalGoalToolLease {
    fn drop(&mut self) {
        self.registry.revoke(&self.server.bearer_token);
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteTerminalSummary {
    id: u64,
    account_id: String,
    account_label: String,
    agent_id: Option<String>,
    source_terminal_key: Option<String>,
    workspace_id: String,
    workspace_path: String,
    started_at: i64,
    login_only: bool,
    /// Vrai quand ce n'est pas un PTY lance par Switch mais une instance
    /// Freebuff Desktop tournee directement sur le poste (home occupe). La
    /// presence est synchronisee, mais Switch ne pilote pas son PTY.
    #[serde(default)]
    external: bool,
}

impl Drop for RemoteTerminalSession {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = terminate_remote_terminal_process_tree(child.as_mut());
        }
    }
}

fn terminate_remote_terminal_process_tree(child: &mut dyn PtyChild) -> std::io::Result<()> {
    let Some(pid) = child.process_id() else {
        return child.kill();
    };

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let status = Command::new("taskkill.exe")
            .args(["/PID", pid.to_string().as_str(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if status.is_ok_and(|status| status.success()) {
            return Ok(());
        }
    }

    #[cfg(not(windows))]
    {
        let process_group = format!("-{pid}");
        let status = Command::new("kill")
            .args(["-KILL", "--", process_group.as_str()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if status.is_ok_and(|status| status.success()) {
            return Ok(());
        }
    }

    child.kill()
}

struct RemoteTerminalIdReservation {
    reservations: Arc<Mutex<HashSet<u64>>>,
    starting_freebuff_accounts: Arc<Mutex<HashSet<String>>>,
    freebuff_account_id: Option<String>,
    id: u64,
}

struct DeviceTerminalTokenReservation {
    tokens: Arc<Mutex<HashMap<String, DeviceTerminalCapability>>>,
    device_fleet: DeviceFleetManager,
    token: String,
    capability: DeviceTerminalCapability,
    committed: bool,
}

impl DeviceTerminalTokenReservation {
    fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for DeviceTerminalTokenReservation {
    fn drop(&mut self) {
        if !self.committed {
            revoke_device_terminal_capability(
                &self.tokens,
                &self.device_fleet,
                &self.token,
                &self.capability,
            );
        }
    }
}

impl RemoteTerminalManager {
    fn with_max_active(max_active: usize) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            reservations: Arc::new(Mutex::new(HashSet::new())),
            starting_freebuff_accounts: Arc::new(Mutex::new(HashSet::new())),
            device_terminal_tokens: Arc::new(Mutex::new(HashMap::new())),
            device_fleet: DeviceFleetManager::default(),
            next_id: Arc::new(AtomicU64::new(0)),
            max_active,
            runtime_sync: RuntimeSync::default(),
            goal_tool_capabilities: None,
            goal_tools_url: None,
        }
    }

    fn with_runtime_sync(mut self, runtime_sync: RuntimeSync) -> Self {
        self.runtime_sync = runtime_sync;
        self
    }

    fn with_device_fleet(mut self, device_fleet: DeviceFleetManager) -> Self {
        self.device_fleet = device_fleet;
        self
    }

    fn with_goal_tools(mut self, capabilities: ChatToolCapabilityRegistry, url: String) -> Self {
        self.goal_tool_capabilities = Some(capabilities);
        self.goal_tools_url = Some(url);
        self
    }

    fn active_count(&self) -> usize {
        self.sessions
            .lock()
            .map(|sessions| sessions.len())
            .unwrap_or_default()
    }

    fn active_workspace_ids(&self) -> HashSet<String> {
        self.sessions
            .lock()
            .map(|sessions| {
                sessions
                    .values()
                    .map(|session| session.workspace_id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn active_for_actor(&self, actor: &RequestActor) -> Result<Vec<RemoteTerminalSummary>, String> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| "Etat terminal verrouille".to_string())?;
        let mut active = sessions
            .iter()
            .filter(|(_, session)| {
                !session.login_only
                    && (actor.is_administrator() || session.owner_id == actor.owner_id())
            })
            .map(|(id, session)| RemoteTerminalSummary {
                id: *id,
                account_id: session.account_id.clone(),
                account_label: session.account_label.clone(),
                agent_id: session.agent_id.clone(),
                source_terminal_key: session.source_terminal_key.clone(),
                workspace_id: session.workspace_id.clone(),
                workspace_path: session.workspace_path.to_string_lossy().to_string(),
                started_at: session.started_at,
                login_only: session.login_only,
                external: false,
            })
            .collect::<Vec<_>>();
        active.sort_by_key(|session| (session.started_at, session.id));
        Ok(active)
    }

    fn unavailable_account_ids(&self) -> HashSet<String> {
        let mut ids = self
            .sessions
            .lock()
            .map(|sessions| {
                sessions
                    .values()
                    .map(|session| session.account_id.clone())
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();
        if let Ok(starting) = self.starting_freebuff_accounts.lock() {
            ids.extend(starting.iter().cloned());
        }
        ids
    }

    fn reserve_device_terminal_token(
        &self,
        terminal_id: u64,
        owner_id: &str,
    ) -> Result<DeviceTerminalTokenReservation, String> {
        if owner_id.trim().is_empty() {
            return Err("Proprietaire du terminal Freebuff absent".to_string());
        }
        let token = format!(
            "cstd_{}{}",
            Uuid::new_v4().simple(),
            Uuid::new_v4().simple()
        );
        let capability = DeviceTerminalCapability {
            owner_id: owner_id.to_string(),
            origin_id: Uuid::new_v4().to_string(),
            terminal_id,
        };
        self.device_terminal_tokens
            .lock()
            .map_err(|_| "Capacites USB des terminaux verrouillees".to_string())?
            .insert(token.clone(), capability.clone());
        Ok(DeviceTerminalTokenReservation {
            tokens: self.device_terminal_tokens.clone(),
            device_fleet: self.device_fleet.clone(),
            token,
            capability,
            committed: false,
        })
    }

    fn device_terminal_capability(&self, provided: &str) -> Option<DeviceTerminalCapability> {
        if provided.is_empty() {
            return None;
        }
        let tokens = self.device_terminal_tokens.lock().ok()?;
        tokens.iter().find_map(|(token, capability)| {
            crate::security::constant_time_eq(provided.as_bytes(), token.as_bytes())
                .then(|| capability.clone())
        })
    }

    fn reserve_id(
        &self,
        requested: Option<u64>,
        freebuff_account_id: Option<&str>,
    ) -> Result<RemoteTerminalIdReservation, String> {
        let mut reservations = self
            .reservations
            .lock()
            .map_err(|_| "Reservations terminal verrouillees".to_string())?;
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| "Etat terminal verrouille".to_string())?;
        let active_or_starting = sessions.len().saturating_add(reservations.len());
        if self.max_active > 0 && active_or_starting >= self.max_active {
            return Err(format!(
                "capacite terminaux atteinte: {active_or_starting}/{} terminaux actifs",
                self.max_active
            ));
        }
        let id = if let Some(id) = requested {
            if reservations.contains(&id) || sessions.contains_key(&id) {
                return Err(format!("Identifiant terminal deja vivant: {id}"));
            }
            id
        } else {
            loop {
                let candidate = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
                if !reservations.contains(&candidate) && !sessions.contains_key(&candidate) {
                    break candidate;
                }
            }
        };
        let freebuff_account_id = freebuff_account_id.map(ToString::to_string);
        if let Some(account_id) = freebuff_account_id.as_deref() {
            if sessions
                .values()
                .any(|session| session.account_id == account_id)
            {
                return Err(
                    "Compte Freebuff indisponible : un terminal est deja ouvert".to_string()
                );
            }
            let mut starting = self
                .starting_freebuff_accounts
                .lock()
                .map_err(|_| "Reservations Freebuff verrouillees".to_string())?;
            if !starting.insert(account_id.to_string()) {
                return Err(
                    "Compte Freebuff indisponible : un terminal est deja en cours d'ouverture"
                        .to_string(),
                );
            }
        }
        drop(sessions);
        reservations.insert(id);
        drop(reservations);
        Ok(RemoteTerminalIdReservation {
            reservations: self.reservations.clone(),
            starting_freebuff_accounts: self.starting_freebuff_accounts.clone(),
            freebuff_account_id,
            id,
        })
    }

    fn active_agent_runs(&self) -> Vec<metrics::ActiveAgentRun> {
        let Ok(guard) = self.sessions.lock() else {
            return Vec::new();
        };

        guard
            .values()
            .filter(|session| !session.recorded_end.load(Ordering::Relaxed))
            .map(|session| metrics::ActiveAgentRun {
                started_at: session.started_at,
            })
            .collect()
    }

    fn start(
        &self,
        config: &ServerConfig,
        request: StartTerminalRequest,
        owner_id: String,
        authorized_workspace: Option<PathBuf>,
        generated_workspaces_root: PathBuf,
    ) -> Result<StartTerminalResponse, String> {
        let settings = settings::load_settings_for_terminal()?;
        let account = settings
            .accounts
            .iter()
            .find(|candidate| candidate.id == request.account_id)
            .cloned()
            .ok_or_else(|| "Compte introuvable".to_string())?;
        let provider = account.provider;
        let proxy = if settings.proxy_controls_enabled {
            account.proxy_id.as_ref().and_then(|id| {
                settings
                    .proxies
                    .iter()
                    .find(|candidate| candidate.id == *id)
            })
        } else {
            None
        };

        let canonical_home = settings::expand_home(&account.codex_home)?;
        fs::create_dir_all(&canonical_home).map_err(|error| error.to_string())?;
        if provider == Provider::Freebuff
            && crate::provider::freebuff_instance_busy(&canonical_home)
        {
            return Err(format!(
                "Compte Freebuff indisponible : {} possede deja un terminal ouvert",
                account.label
            ));
        }
        // Ferme la fenetre de course entre le controle du fichier owner et le
        // moment ou le processus Freebuff ecrit lui-meme ce fichier.
        let id_reservation = self.reserve_id(
            request.id,
            (provider == Provider::Freebuff).then_some(account.id.as_str()),
        )?;
        let id = id_reservation.id;

        // Un dossier existant est utilise directement. Un depot distant est
        // clone normalement avec sa branche et son upstream : les commandes Git
        // standard (pull, push, rebase) fonctionnent directement.
        let selected_workspace_requested = request
            .workspace_path
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty());

        let (repo_dir, workspace_id, repo_label) = if request.login_only {
            // Le login reste hors de tout projet et utilise uniquement le home
            // isole du compte. La racine des workspaces demeure obligatoire
            // pour chaque terminal de travail.
            let label = display_path(&canonical_home);
            let workspace_id = workspace_id_for_dir(&canonical_home);
            (canonical_home.clone(), workspace_id, label)
        } else if let Some(dir) = authorized_workspace {
            let label = display_path(&dir);
            let workspace_id = workspace_id_for_dir(&dir);
            (dir, workspace_id, label)
        } else {
            if selected_workspace_requested {
                return Err("Acces refuse a cet environnement".to_string());
            }
            let repo_url = request.repo_url.as_deref().unwrap_or("").trim();
            if repo_url.is_empty() {
                return Err("Environnement obligatoire avant d'ouvrir un terminal".to_string());
            } else {
                let workspace_id = format!("{id}-{}", Uuid::new_v4().simple());
                let repo_dir = generated_workspaces_root.join(&workspace_id).join("repo");
                let repo_label = prepare_workspace(
                    repo_url,
                    request.branch.as_deref(),
                    &repo_dir,
                    &config.git_pat,
                )
                .map_err(|error| redact_secrets(&error, config))?;
                (repo_dir, workspace_id, repo_label)
            }
        };

        let account_home = canonical_home;

        // Synchronise a chaque demarrage la config propre au compte, dans le
        // format du provider (Codex config.toml / Claude settings.json). Un echec
        // ne bloque pas le PTY.
        if let Err(error) = provider.write_account_config(
            &account_home,
            account.bypass,
            account.model.as_deref(),
            account.reasoning_effort.as_deref(),
            account.fast_mode,
        ) {
            eprintln!(
                "[config] config {} non ecrite pour {}: {error}",
                provider.as_str(),
                account.label
            );
        }

        let goal_tool_lease = if provider == Provider::Freebuff && !request.login_only {
            let capabilities = self.goal_tool_capabilities.as_ref().ok_or_else(|| {
                "Outils create_goal indisponibles pour le terminal Freebuff".to_string()
            })?;
            let url = self.goal_tools_url.as_deref().ok_or_else(|| {
                "URL des outils create_goal indisponible pour le terminal Freebuff".to_string()
            })?;
            crate::provider::ensure_freebuff_goal_mcp(&account_home, url).map_err(|error| {
                format!("Configuration de create_goal pour Freebuff impossible : {error}")
            })?;
            let goal_key = terminal_goal_key(&owner_id, &workspace_id)?;
            let token = capabilities.issue(AutonomousAgentToolContext {
                account_id: account.id.clone(),
                scope: ChatToolScope::GoalsOnly,
                user_id: (owner_id != "server-admin").then_some(owner_id.clone()),
                source_chat_key: request.source_terminal_key.clone(),
                project_dir: Some(repo_dir.to_string_lossy().to_string()),
                mode: crate::chat::ChatTurnMode::Build,
                model: account.model.clone(),
                reasoning_effort: account.reasoning_effort.clone(),
                goal_key: Some(goal_key),
            })?;
            Some(TerminalGoalToolLease {
                server: ChatModelToolServerConfig {
                    url: url.to_string(),
                    bearer_token: token,
                },
                registry: capabilities.clone(),
            })
        } else {
            None
        };

        let device_terminal_capability = (provider == Provider::Freebuff)
            .then(|| self.reserve_device_terminal_token(id, &owner_id))
            .transpose()?;

        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize {
                rows: request.rows.max(8),
                cols: request.cols.max(20),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| error.to_string())?;

        let mut builder = shell_command(&settings);
        builder.cwd(repo_dir.as_os_str());
        for (key, value) in provider.home_env(&account_home) {
            builder.env(key, value);
        }
        if let Some(lease) = goal_tool_lease.as_ref() {
            builder.env(
                crate::chat_model_tools::MCP_BEARER_ENV,
                lease.server.bearer_token.as_str(),
            );
        }
        builder.env("TERM", "xterm-256color");
        builder.env("COLORTERM", "truecolor");
        builder.env("PWD", repo_dir.to_string_lossy().to_string());
        crate::resource_profile::configure_terminal_resources(&mut builder);
        if provider == Provider::Freebuff {
            let server_bin = std::env::current_exe()
                .map_err(|error| format!("Binaire cst-server introuvable : {error}"))?;
            builder.env("CST_DEVICE_API_URL", config.device_terminal_api_url()?);
            builder.env(
                "CST_DEVICE_TOKEN",
                device_terminal_capability
                    .as_ref()
                    .map(|capability| capability.token.as_str())
                    .ok_or_else(|| "Capacite USB Freebuff absente".to_string())?,
            );
            builder.env("CST_DEVICE_HELPER", "cst-device");
            builder.env("CST_SERVER_BIN", server_bin);
            builder.env("PATH", path_with_device_terminal_helper(config)?);
        }

        if let Some(proxy) = proxy {
            for key in [
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "http_proxy",
                "https_proxy",
                "all_proxy",
            ] {
                builder.env(key, proxy.proxy_url.clone());
            }
        }

        let mut child = pair
            .slave
            .spawn_command(builder)
            .map_err(|error| error.to_string())?;
        drop(pair.slave);

        let mut reader = match pair.master.try_clone_reader() {
            Ok(reader) => reader,
            Err(error) => {
                let _ = terminate_remote_terminal_process_tree(child.as_mut());
                return Err(error.to_string());
            }
        };
        let writer = match pair.master.take_writer() {
            Ok(writer) => writer,
            Err(error) => {
                let _ = terminate_remote_terminal_process_tree(child.as_mut());
                return Err(error.to_string());
            }
        };
        // Garder le receiver initial est essentiel : le shell peut produire son
        // prompt (et Codex son premier ecran ANSI) avant que le POST /terminals
        // ait repondu et que le navigateur ait ouvert son WebSocket.
        let (events, initial_events) = broadcast::channel(TERMINAL_EVENT_BUFFER);

        let session = Arc::new(RemoteTerminalSession {
            writer: Mutex::new(writer),
            master: Mutex::new(pair.master),
            child: Mutex::new(child),
            events: events.clone(),
            pending_events: Mutex::new(Some(TerminalEventCursor {
                receiver: initial_events,
                prefetched: None,
            })),
            socket_generation: AtomicU64::new(0),
            started_at: metrics::now_ts(),
            owner_id,
            account_id: account.id.clone(),
            account_label: account.label.clone(),
            agent_id: request.agent_id.clone(),
            source_terminal_key: request.source_terminal_key.clone(),
            workspace_id: workspace_id.clone(),
            workspace_path: repo_dir.clone(),
            login_only: request.login_only,
            device_terminal_capability: device_terminal_capability.as_ref().map(|reservation| {
                ActiveDeviceTerminalCapability {
                    token: reservation.token.clone(),
                    context: reservation.capability.clone(),
                }
            }),
            device_terminal_tokens: self.device_terminal_tokens.clone(),
            device_fleet: self.device_fleet.clone(),
            recorded_end: AtomicBool::new(false),
            goal_tool_lease: Mutex::new(goal_tool_lease),
        });

        {
            let mut sessions = self
                .sessions
                .lock()
                .map_err(|_| "Etat terminal verrouille".to_string())?;
            if sessions.contains_key(&id) {
                drop(sessions);
                if let Ok(mut child) = session.child.lock() {
                    let _ = terminate_remote_terminal_process_tree(child.as_mut());
                }
                return Err(format!("Identifiant terminal deja vivant: {id}"));
            }
            sessions.insert(id, session.clone());
        }
        id_reservation.commit();
        // La session possede desormais le contexte de revocation. Tout echec
        // anterieur laisse la reservation retirer immediatement le jeton.
        if let Some(capability) = device_terminal_capability {
            capability.commit();
        }
        if !session.login_only {
            self.runtime_sync.notify(RuntimeSyncTopic::ActiveTerminals);
        }

        let sessions = self.sessions.clone();
        let runtime_sync = self.runtime_sync.clone();
        let reader_events = events.clone();
        let reader_thread = thread::Builder::new()
            .name(format!("cst-remote-terminal-reader-{id}"))
            .stack_size(crate::resource_profile::TERMINAL_READER_STACK_BYTES)
            .spawn(move || {
                let mut buffer = [0_u8; 8192];
                let mut decode = incremental_utf8_decoder();
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(size) => {
                            let data = decode(&buffer[..size]);
                            if !data.is_empty() {
                                let _ = reader_events.send(ServerWsMessage::Data { id, data });
                            }
                        }
                        Err(error) => {
                            let _ = reader_events.send(ServerWsMessage::Error {
                                id,
                                message: error.to_string(),
                            });
                            break;
                        }
                    }
                }

                let ended = sessions
                    .lock()
                    .ok()
                    .and_then(|mut sessions| sessions.remove(&id));
                if let Some(session) = ended {
                    finish_session(&session);
                    let _ = session.events.send(ServerWsMessage::Exit { id });
                    if !session.login_only {
                        runtime_sync.notify(RuntimeSyncTopic::ActiveTerminals);
                    }
                }
            });
        if let Err(error) = reader_thread {
            let ended = self
                .sessions
                .lock()
                .ok()
                .and_then(|mut sessions| sessions.remove(&id));
            if let Some(session) = ended {
                finish_session(&session);
                if !session.login_only {
                    self.runtime_sync.notify(RuntimeSyncTopic::ActiveTerminals);
                }
            }
            return Err(format!("Lecture du terminal impossible: {error}"));
        }

        let banner = format!(
            "\r\n[Codex Switch Terminal SaaS] session #{id} | compte: {} | repo: {} | dossier: {}\r\n\r\n",
            account.label,
            repo_label,
            repo_dir.to_string_lossy()
        );
        if events
            .send(ServerWsMessage::Data { id, data: banner })
            .is_err()
        {
            self.abort_started_session(id, &session);
            return Err("Diffusion du bandeau terminal impossible".to_string());
        }

        let command = if request.login_only {
            // Mode authentification strict : ne jamais retomber sur la commande
            // de demarrage du compte, qui pourrait ouvrir Codex normalement.
            normalize_remote_login_command(provider, request.command)
        } else {
            request.command.or_else(|| account.startup_command.clone())
        };
        if let Some(command) = command {
            let line = format!("{}\r", command.trim());
            let write_result = session
                .writer
                .lock()
                .map_err(|_| "Writer terminal verrouille".to_string())
                .and_then(|mut writer| {
                    writer
                        .write_all(line.as_bytes())
                        .map_err(|error| error.to_string())
                });
            if let Err(error) = write_result {
                self.abort_started_session(id, &session);
                return Err(error);
            }
        }

        Ok(StartTerminalResponse {
            id,
            workspace_id,
            workspace_path: repo_dir.to_string_lossy().to_string(),
        })
    }

    fn abort_started_session(&self, id: u64, session: &Arc<RemoteTerminalSession>) {
        let removed = self.sessions.lock().ok().and_then(|mut sessions| {
            let matches = sessions
                .get(&id)
                .is_some_and(|candidate| Arc::ptr_eq(candidate, session));
            matches.then(|| sessions.remove(&id)).flatten()
        });
        if let Some(session) = removed {
            finish_session(&session);
            if !session.login_only {
                self.runtime_sync.notify(RuntimeSyncTopic::ActiveTerminals);
            }
            if let Ok(mut child) = session.child.lock() {
                let _ = terminate_remote_terminal_process_tree(child.as_mut());
            }
        }
    }

    fn write(&self, id: u64, data: String) -> Result<(), String> {
        let session = self.get(id)?;
        let mut writer = session
            .writer
            .lock()
            .map_err(|_| "Writer terminal verrouille".to_string())?;
        writer
            .write_all(data.as_bytes())
            .and_then(|_| writer.flush())
            .map_err(|error| error.to_string())
    }

    fn write_for_actor(&self, id: u64, data: String, actor: &RequestActor) -> Result<(), String> {
        let session = self.get_for_actor(id, actor)?;
        let mut writer = session
            .writer
            .lock()
            .map_err(|_| "Writer terminal verrouille".to_string())?;
        writer
            .write_all(data.as_bytes())
            .and_then(|_| writer.flush())
            .map_err(|error| error.to_string())
    }

    fn resize(&self, id: u64, cols: u16, rows: u16) -> Result<(), String> {
        let session = self.get(id)?;
        let master = session
            .master
            .lock()
            .map_err(|_| "PTY verrouille".to_string())?;
        master
            .resize(PtySize {
                rows: rows.max(8),
                cols: cols.max(20),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| error.to_string())
    }

    fn resize_for_actor(
        &self,
        id: u64,
        cols: u16,
        rows: u16,
        actor: &RequestActor,
    ) -> Result<(), String> {
        let session = self.get_for_actor(id, actor)?;
        let master = session
            .master
            .lock()
            .map_err(|_| "PTY verrouille".to_string())?;
        master
            .resize(PtySize {
                rows: rows.max(8),
                cols: cols.max(20),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| error.to_string())
    }

    fn stop(&self, id: u64) -> Result<(), String> {
        let session = self
            .sessions
            .lock()
            .map_err(|_| "Etat terminal verrouille".to_string())?
            .get(&id)
            .cloned();
        let Some(session) = session else {
            return Ok(());
        };

        finish_session(&session);
        let result = session
            .child
            .lock()
            .map_err(|_| "Process terminal verrouille".to_string())
            .and_then(|mut child| {
                terminate_remote_terminal_process_tree(child.as_mut())
                    .map_err(|error| error.to_string())
            });
        if let Err(error) = result {
            // Conserver la session dans la table tant que l'arbre n'est pas
            // confirme comme termine : un DELETE en erreur ne doit pas rendre
            // le PTY invisible tout en laissant Freebuff vivant.
            return Err(error);
        }

        let removed = self.sessions.lock().ok().and_then(|mut sessions| {
            let matches = sessions
                .get(&id)
                .is_some_and(|candidate| Arc::ptr_eq(candidate, &session));
            matches.then(|| sessions.remove(&id)).flatten()
        });
        if let Some(removed) = removed {
            if !removed.login_only {
                self.runtime_sync.notify(RuntimeSyncTopic::ActiveTerminals);
            }
            let _ = removed.events.send(ServerWsMessage::Exit { id });
        }
        Ok(())
    }

    fn stop_for_actor(&self, id: u64, actor: &RequestActor) -> Result<(), String> {
        let session = {
            let sessions = self
                .sessions
                .lock()
                .map_err(|_| "Etat terminal verrouille".to_string())?;
            let Some(session) = sessions.get(&id).cloned() else {
                return Ok(());
            };
            if !actor.is_administrator() && session.owner_id != actor.owner_id() {
                return Err("Terminal introuvable ou inaccessible".to_string());
            }
            session
        };

        finish_session(&session);
        let result = session
            .child
            .lock()
            .map_err(|_| "Process terminal verrouille".to_string())
            .and_then(|mut child| {
                terminate_remote_terminal_process_tree(child.as_mut())
                    .map_err(|error| error.to_string())
            });
        if let Err(error) = result {
            // Ne pas retirer un PTY dont l'arbre n'a pas ete arrete. Le retry
            // client pourra alors retenter le DELETE sur une session encore
            // adressable au lieu de perdre definitivement son identifiant.
            return Err(error);
        }

        let removed = self.sessions.lock().ok().and_then(|mut sessions| {
            let matches = sessions
                .get(&id)
                .is_some_and(|candidate| Arc::ptr_eq(candidate, &session));
            matches.then(|| sessions.remove(&id)).flatten()
        });
        if let Some(removed) = removed {
            if !removed.login_only {
                self.runtime_sync.notify(RuntimeSyncTopic::ActiveTerminals);
            }
            let _ = removed.events.send(ServerWsMessage::Exit { id });
        }
        Ok(())
    }

    fn get(&self, id: u64) -> Result<Arc<RemoteTerminalSession>, String> {
        self.sessions
            .lock()
            .map_err(|_| "Etat terminal verrouille".to_string())?
            .get(&id)
            .cloned()
            .ok_or_else(|| "Session terminal introuvable".to_string())
    }

    fn contains(&self, id: u64) -> bool {
        self.sessions
            .lock()
            .map(|sessions| sessions.contains_key(&id))
            .unwrap_or(false)
    }

    fn get_for_actor(
        &self,
        id: u64,
        actor: &RequestActor,
    ) -> Result<Arc<RemoteTerminalSession>, String> {
        let session = self.get(id)?;
        if actor.is_administrator() || session.owner_id == actor.owner_id() {
            Ok(session)
        } else {
            Err("Terminal introuvable ou inaccessible".to_string())
        }
    }
}

impl RemoteTerminalIdReservation {
    fn commit(self) {
        // La session inseree porte maintenant l'identifiant vivant ; les
        // reservations transitoires doivent etre liberees apres le spawn.
        if let Ok(mut reservations) = self.reservations.lock() {
            reservations.remove(&self.id);
        }
        if let Some(account_id) = self.freebuff_account_id.as_deref() {
            if let Ok(mut starting) = self.starting_freebuff_accounts.lock() {
                starting.remove(account_id);
            }
        }
    }
}

impl Drop for RemoteTerminalIdReservation {
    fn drop(&mut self) {
        if let Ok(mut reservations) = self.reservations.lock() {
            reservations.remove(&self.id);
        }
        if let Some(account_id) = self.freebuff_account_id.as_deref() {
            if let Ok(mut starting) = self.starting_freebuff_accounts.lock() {
                starting.remove(account_id);
            }
        }
    }
}

fn revoke_device_terminal_capability(
    tokens: &Arc<Mutex<HashMap<String, DeviceTerminalCapability>>>,
    device_fleet: &DeviceFleetManager,
    token: &str,
    capability: &DeviceTerminalCapability,
) {
    match tokens.lock() {
        Ok(mut tokens) => {
            tokens.remove(token);
        }
        Err(_) => {
            // Le lookup echoue ferme si ce mutex est empoisonne : le jeton ne
            // peut donc plus autoriser de requete, meme si son retrait echoue.
            eprintln!(
                "[device-fleet] retrait du jeton du terminal {} impossible ; registre refuse par securite",
                capability.terminal_id
            );
        }
    }
    if let Err(error) =
        device_fleet.expire_origin_actions(&capability.owner_id, &capability.origin_id)
    {
        // `expire_origin_actions` place l'origin dans sa deny-list memoire avant
        // la persistance. Ne jamais journaliser le jeton ni l'identifiant origin.
        eprintln!(
            "[device-fleet] persistance de fin du terminal {} impossible ; origin refusee en memoire : {error}",
            capability.terminal_id
        );
    }
}

fn finish_session(session: &Arc<RemoteTerminalSession>) {
    if session.recorded_end.swap(true, Ordering::AcqRel) {
        return;
    }

    if let Some(capability) = session.device_terminal_capability.as_ref() {
        revoke_device_terminal_capability(
            &session.device_terminal_tokens,
            &session.device_fleet,
            &capability.token,
            &capability.context,
        );
    }

    if let Ok(mut lease) = session.goal_tool_lease.lock() {
        lease.take();
    }

    let _ = metrics::record_agent_run(
        &session.account_id,
        &session.account_label,
        session.started_at,
        metrics::now_ts(),
    );
}

fn frontend_cache_control(path: &str) -> Option<&'static str> {
    if path.starts_with("/api/")
        || path.starts_with("/ws/")
        || path.starts_with("/mcp")
        || path.starts_with("/t/")
        || path == "/healthz"
    {
        return None;
    }

    // Vite ajoute un hash de contenu aux fichiers sous /assets/. Ils peuvent
    // donc etre conserves tres longtemps sans jamais servir une ancienne
    // version : chaque build produit une nouvelle URL.
    if path.starts_with("/assets/") {
        return Some("public, max-age=31536000, immutable");
    }

    if path.starts_with("/icons/") || path == "/apple-touch-icon.png" {
        return Some("public, max-age=604800, stale-while-revalidate=2592000");
    }

    if path == "/manifest.webmanifest"
        || path.starts_with("/skills/")
        || path.starts_with("/impeccable/")
    {
        return Some("public, max-age=3600, stale-while-revalidate=86400");
    }

    // `no-cache` autorise un 304 conditionnel, contrairement a l'ancien
    // `no-store` qui retransmettait index.html et le service worker en entier.
    Some("no-cache, must-revalidate")
}

fn frontend_response_cache_control(path: &str, status: StatusCode) -> Option<&'static str> {
    if path.starts_with("/assets/") && !status.is_success() && status != StatusCode::NOT_MODIFIED {
        // Ne jamais rendre immuable un 404 de chunk : un proxy ou le cache HTTP
        // du navigateur ne doit pas memoriser pendant un an une publication
        // transitoirement incomplete.
        return Some("no-store");
    }
    frontend_cache_control(path)
}

async fn set_frontend_cache_control(request: Request, next: Next) -> Response {
    let path = request.uri().path().to_string();
    let mut response = next.run(request).await;
    if let Some(cache_control) = frontend_response_cache_control(&path, response.status()) {
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static(cache_control));
    }
    response
}

/// Charge puis retire les secrets de controle avant la creation du runtime
/// Tokio. L'environnement d'un processus ne doit pas etre modifie pendant que
/// d'autres threads peuvent le lire.
pub fn prepare_from_env_before_runtime() -> Result<ServerConfig, String> {
    let config = ServerConfig::from_env()?;
    isolate_server_control_secrets()?;
    Ok(config)
}

pub async fn run(config: ServerConfig) -> Result<(), String> {
    fs::create_dir_all(config.data_dir.join("workspaces")).map_err(|error| error.to_string())?;
    fs::create_dir_all(config.data_dir.join("codex-homes")).map_err(|error| error.to_string())?;
    fs::create_dir_all(config.data_dir.join("logs")).map_err(|error| error.to_string())?;
    install_device_terminal_helper(&config)?;

    let settings = settings::load_settings_for_terminal()?;
    let pool_manager = Arc::new(PoolManager::build(&settings)?);
    // La capacite annoncee par /health est aussi le plafond atomique reel du
    // moteur. Elle couvre les chats UI, autonomes et orchestres sans exception.
    let chat = ChatTurnManager::with_max_active(config.node_capacity);
    crate::chat::start_orphan_chat_image_sweeper();
    let user_auth = AuthManager::load(config.data_dir.clone(), &config.public_base_url)?
        .with_runtime_sync(chat.runtime_sync());
    // Le registre est cree ici, avant le moteur d'agents : chaque cycle d'agent
    // recoit sa propre capacite MCP, portee par le proprietaire de l'agent.
    let chat_tool_capabilities = ChatToolCapabilityRegistry::default();
    let agent_tools_url = config.chat_tools_mcp_url().ok();
    let agent_capabilities = chat_tool_capabilities.clone();
    let autonomous =
        AutonomousAgentManager::new(chat.clone(), config.data_dir.join("autonomous-agents.json"))?
            .with_model_tools(Arc::new(move |agent: &AutonomousAgentSnapshot| {
                // Agent sans proprietaire nominatif (cree avant cette version,
                // ou par le jeton administrateur) : aucun outil. Deviner une
                // boite reviendrait a ecrire depuis celle de quelqu'un d'autre.
                let owner_id = agent.owner_id.clone()?;
                let url = agent_tools_url.clone()?;
                let token = agent_capabilities
                    .issue(AutonomousAgentToolContext {
                        account_id: agent.account_id.clone(),
                        scope: ChatToolScope::PersonalDataOnly,
                        user_id: Some(owner_id),
                        source_chat_key: agent.source_chat_key.clone(),
                        project_dir: agent.project_dir.clone(),
                        mode: agent.mode,
                        model: agent.model.clone(),
                        reasoning_effort: agent.reasoning_effort.clone(),
                        goal_key: None,
                    })
                    .ok()?;
                Some(ChatModelToolServerConfig {
                    url,
                    bearer_token: token,
                })
            }));
    let orchestration =
        OrchestrationManager::new(chat.clone(), config.data_dir.join("orchestrated-runs.json"))?;
    let forum = ForumManager::new(config.data_dir.join("forum.json"))?;
    let private_messages =
        PrivateMessageManager::new(config.data_dir.join("private-messages.json"))?;
    let tiktok_messaging = TikTokDmManager::new(config.data_dir.join("tiktok-dm-campaigns.json"))?;
    let device_fleet = DeviceFleetManager::load(
        &config.data_dir,
        Duration::from_secs(device_fleet::DEFAULT_CONNECTOR_TTL_SECONDS as u64),
    )
    .map_err(|error| format!("Store des actions appareils illisible : {error}"))?;
    let tracking = TrackingManager::load(config.data_dir.join("tracking-links.json"))?;
    let referral = ReferralManager::load(
        config.data_dir.join("referral-codes.json"),
        config.duello_referral_api_url.clone(),
        config.duello_referral_api_key.clone(),
        config.duello_app_url.clone(),
    )?;
    let duello_bank = DuelloBankClient::new(config.duello_bank.clone())?;
    let workspace_access = WorkspaceAccessManager::load(config.data_dir.clone())?;
    // Les jetons Microsoft vivent dans `config.data_dir`, comme `user-auth.json`
    // et contrairement aux autres integrations qui passent par
    // `settings::runtime_data_path` : un `CST_ACCOUNTS_DIR` partage entre
    // noeuds rattacherait des jetons a des identifiants utilisateurs locaux.
    let microsoft = MicrosoftManager::load(
        config.data_dir.clone(),
        &config.public_base_url,
        user_auth.clone(),
    )?;
    // Meme principe : les jetons Gmail vivent dans `config.data_dir`, rattaches
    // aux identifiants utilisateurs locaux du noeud.
    let gmail = GmailManager::load(
        config.data_dir.clone(),
        &config.public_base_url,
        user_auth.clone(),
    )?;
    let terminal_goals = TerminalGoalManager::new(config.data_dir.join("terminal-goals.json"))?;
    let terminals = RemoteTerminalManager::with_max_active(config.terminal_capacity)
        .with_device_fleet(device_fleet.clone())
        .with_runtime_sync(chat.runtime_sync())
        .with_goal_tools(chat_tool_capabilities.clone(), config.chat_tools_mcp_url()?);
    let state = Arc::new(ServerState {
        config: config.clone(),
        auth: user_auth.clone(),
        terminals,
        chat,
        chat_tool_capabilities,
        chat_open_requests: ChatOpenRequestRegistry::default(),
        chat_post_requests: ChatPostRequestRegistry::default(),
        microsoft: microsoft.clone(),
        gmail: gmail.clone(),
        autonomous,
        terminal_goals,
        orchestration,
        forum,
        private_messages,
        tiktok_messaging,
        device_fleet: device_fleet.clone(),
        tracking,
        referral,
        duello_bank,
        workspace_access,
        doctolib_lab: Arc::new(DoctolibLabManager::default()),
        kombai: Arc::new(KombaiManager::default()),
        kombai_owner: Arc::new(Mutex::new(None)),
        vps_deploy: VpsDeployManager::default(),
        started_at: metrics::now_ts(),
        drain_until: Arc::new(AtomicI64::new(0)),
        cleanup: Arc::new(CleanupCoordinator::default()),
    });
    telegram_notifications::start_polling(state.autonomous.clone());
    state.microsoft.start_keepalive();
    #[cfg(target_os = "windows")]
    if embedded_device_connector_enabled() {
        tokio::spawn(run_embedded_device_connector(device_fleet));
    }
    tokio::spawn(
        crate::private_messages::run_private_message_campaign_worker(
            state.private_messages.clone(),
            state.chat.runtime_sync(),
        ),
    );

    spawn_workspace_cleanup(config.data_dir.clone(), state.terminals.clone());

    let api = Router::new()
        .route("/health", get(api_health))
        .route(
            "/tracking-links",
            get(api_tracking_links).post(api_create_tracking_link),
        )
        .route("/tracking-links/:slug", delete(api_delete_tracking_link))
        .route("/referral", get(api_referral_snapshot).post(api_create_referral))
        .route("/referral/:code", delete(api_delete_referral))
        .route("/duello-bank", get(api_duello_bank_snapshot))
        .route(
            "/duello-bank/credits",
            post(api_credit_duello_wallet).layer(DefaultBodyLimit::max(8 * 1024)),
        )
        .route("/admin/drain", post(api_admin_drain))
        .route("/cleanup/request", post(api_cleanup_request))
        .route("/cleanup/status", get(api_cleanup_status))
        .route("/cleanup/result", post(api_cleanup_result))
        .route("/vps/capabilities", get(api_vps_capabilities))
        .route(
            "/vps/deployments",
            get(api_vps_deployments).post(api_vps_start_deployment),
        )
        .route("/vps/deployments/:id", get(api_vps_deployment))
        .route("/settings", get(api_get_settings).put(api_put_settings))
        .route(
            "/accounts",
            get(api_get_accounts).post(api_add_shared_account),
        )
        .route("/accounts/import", post(api_import_account))
        .route("/accounts/home", post(api_ensure_account_home))
        .route("/accounts/:id", delete(api_remove_account))
        .route("/limits", get(api_limits))
        .route("/usage", get(api_usage))
        .route("/tokscale/submit", post(api_tokscale_submit))
        .route("/account-usage", get(api_account_usage))
        .route("/work-time", get(api_work_time))
        .route("/discussions", get(api_list_discussions))
        .route("/prompt-history", get(api_list_prompt_history))
        .route("/discussions/transcript", get(api_discussion_transcript))
        .route("/discussions/copy", post(api_copy_discussion))
        .route("/discussions/move", post(api_move_discussion))
        .route("/discussions/rename", post(api_rename_discussion))
        .route("/discussions/claim", post(api_claim_session))
        .route("/discussions/delete", post(api_delete_discussion))
        .route(
            "/discussions/export",
            post(api_export_discussion_transcript),
        )
        .route(
            "/discussions/import-codex-freebuff",
            post(api_import_codex_transcript_to_freebuff),
        )
        .route(
            "/forum/topics",
            get(api_list_forum_topics).post(api_create_forum_topic),
        )
        .route("/forum/topics/:id", get(api_get_forum_topic))
        .route("/forum/topics/:id/replies", post(api_reply_to_forum_topic))
        .route(
            "/private-messages/users",
            get(api_list_private_message_users),
        )
        .route(
            "/private-messages/conversations",
            get(api_list_private_message_conversations),
        )
        .route(
            "/private-messages/conversations/:user_id",
            get(api_get_private_message_conversation)
                .post(api_send_private_message)
                .layer(DefaultBodyLimit::max(MAX_PRIVATE_MESSAGE_REQUEST_BYTES)),
        )
        .route(
            "/private-messages/images/:image_id",
            get(api_get_private_message_image),
        )
        .route(
            "/private-messages/campaigns",
            get(api_list_private_message_campaigns).post(api_create_private_message_campaign),
        )
        .route(
            "/private-messages/campaigns/:campaign_id/control",
            post(api_control_private_message_campaign),
        )
        .route("/device-fleet", get(api_list_control_devices))
        .route(
            "/device-fleet/actions",
            post(api_control_device).layer(DefaultBodyLimit::max(16 * 1024)),
        )
        .route(
            "/device-fleet/actions/:action_id",
            get(api_device_action_status),
        )
        .route(
            "/device-fleet/connector/heartbeat",
            post(api_device_connector_heartbeat).layer(DefaultBodyLimit::max(256 * 1024)),
        )
        .route(
            "/device-fleet/connector/claim",
            post(api_device_connector_claim).layer(DefaultBodyLimit::max(4 * 1024)),
        )
        .route(
            "/device-fleet/connector/report",
            post(api_device_connector_report).layer(DefaultBodyLimit::max(12 * 1024 * 1024)),
        )
        .route(
            "/tiktok/dm-campaigns",
            get(api_list_tiktok_dm_campaigns)
                .post(api_prepare_tiktok_dm_campaign)
                .layer(DefaultBodyLimit::max(16 * 1024)),
        )
        .route(
            "/tiktok/dm-campaigns/:campaign_id/confirm",
            post(api_confirm_tiktok_dm_campaign).layer(DefaultBodyLimit::max(4 * 1024)),
        )
        .route(
            "/tiktok/sender-accounts",
            get(api_list_tiktok_sender_accounts),
        )
        .route(
            "/tiktok/sender-accounts/select",
            post(api_select_tiktok_sender_account).layer(DefaultBodyLimit::max(4 * 1024)),
        )
        .route(
            "/tiktok/sender-login",
            post(api_queue_tiktok_sender_setup).layer(DefaultBodyLimit::max(4 * 1024)),
        )
        .route(
            "/tiktok/follower-extractions",
            get(api_list_tiktok_follower_extractions)
                .post(api_queue_tiktok_follower_extraction)
                .layer(DefaultBodyLimit::max(8 * 1024)),
        )
        .route(
            "/tiktok/connector/heartbeat",
            post(api_tiktok_connector_heartbeat).layer(DefaultBodyLimit::max(8 * 1024)),
        )
        .route(
            "/tiktok/connector/jobs/claim",
            post(api_tiktok_connector_claim).layer(DefaultBodyLimit::max(4 * 1024)),
        )
        .route(
            "/tiktok/connector/jobs/report",
            post(api_tiktok_connector_report).layer(DefaultBodyLimit::max(8 * 1024)),
        )
        .route(
            "/tiktok/connector/sender-setup/claim",
            post(api_tiktok_sender_setup_claim).layer(DefaultBodyLimit::max(4 * 1024)),
        )
        .route(
            "/tiktok/connector/sender-setup/report",
            post(api_tiktok_sender_setup_report).layer(DefaultBodyLimit::max(8 * 1024)),
        )
        .route(
            "/tiktok/connector/follower-extractions/claim",
            post(api_tiktok_follower_connector_claim).layer(DefaultBodyLimit::max(4 * 1024)),
        )
        .route(
            "/tiktok/connector/follower-extractions/report",
            post(api_tiktok_follower_connector_report).layer(DefaultBodyLimit::max(8 * 1024)),
        )
        .route(
            "/tiktok/connector/follower-extractions/pending-results",
            post(api_tiktok_follower_connector_pending_results)
                .layer(DefaultBodyLimit::max(4 * 1024)),
        )
        .route(
            "/tiktok/connector/follower-extractions/report-result",
            post(api_tiktok_follower_connector_report_result)
                .layer(DefaultBodyLimit::max(64 * 1024)),
        )
        .route("/chat/models", get(api_chat_models))
        .route(
            "/chat/turns",
            post(api_start_chat_turn).layer(DefaultBodyLimit::max(MAX_CHAT_TURN_REQUEST_BYTES)),
        )
        .route("/chat/compact", post(api_compact_chat_session))
        .route("/chat/turns/active", get(api_list_active_chat_turns))
        .route(
            "/chat/open-requests/claim",
            post(api_claim_chat_open_requests),
        )
        .route(
            "/chat/post-requests/claim",
            post(api_claim_chat_post_requests),
        )
        .route(
            "/voice/process",
            post(api_process_voice).layer(DefaultBodyLimit::max(voice::MAX_REQUEST_BYTES)),
        )
        .route("/voice/status", get(api_voice_runtime_status))
        .route(
            "/transcriptions",
            post(api_transcribe_audio_file)
                .layer(DefaultBodyLimit::max(voice::MAX_AUDIO_FILE_BYTES)),
        )
        .route(
            "/creative/accounts",
            get(api_creative_accounts)
                .post(api_connect_creative_account)
                .layer(DefaultBodyLimit::max(16 * 1024)),
        )
        .route(
            "/creative/accounts/default",
            post(api_set_default_creative_account),
        )
        .route(
            "/creative/accounts/delete",
            post(api_delete_creative_account),
        )
        .route("/freebuff-cloud/status", get(api_freebuff_cloud_status))
        .route(
            "/freebuff-cloud/connect",
            post(api_freebuff_cloud_connect).layer(DefaultBodyLimit::max(16 * 1024)),
        )
        .route("/freebuff-cloud/disconnect", post(api_freebuff_cloud_disconnect))
        .route("/freebuff-cloud/projects", get(api_freebuff_cloud_projects))
        .route(
            "/freebuff-cloud/projects/blank",
            post(api_freebuff_cloud_create_blank_project)
                .layer(DefaultBodyLimit::max(8 * 1024)),
        )
        .route(
            "/freebuff-cloud/projects/repo",
            post(api_freebuff_cloud_connect_repo).layer(DefaultBodyLimit::max(8 * 1024)),
        )
        .route(
            "/freebuff-cloud/projects/delete",
            post(api_freebuff_cloud_delete_project).layer(DefaultBodyLimit::max(8 * 1024)),
        )
        .route("/freebuff-cloud/repos", get(api_freebuff_cloud_connectable_repos))
        .route("/freebuff-cloud/stream", get(api_freebuff_cloud_stream))
        .route(
            "/tasks",
            get(api_tasks_list)
                .post(api_tasks_add)
                .put(api_tasks_replace)
                .layer(DefaultBodyLimit::max(1024 * 1024)),
        )
        .route("/tasks/:id", delete(api_tasks_remove))
        .route(
            "/notifications/whatsapp",
            get(api_whatsapp_connection)
                .post(api_connect_whatsapp)
                .delete(api_disconnect_whatsapp)
                .layer(DefaultBodyLimit::max(16 * 1024)),
        )
        .route("/notifications/whatsapp/test", post(api_test_whatsapp))
        .route(
            "/notifications/telegram",
            get(api_telegram_connection)
                .post(api_connect_telegram)
                .delete(api_disconnect_telegram)
                .layer(DefaultBodyLimit::max(16 * 1024)),
        )
        .route(
            "/notifications/telegram/pairing",
            post(api_refresh_telegram_pairing),
        )
        .route("/notifications/telegram/test", post(api_test_telegram))
        .route(
            "/notifications/telegram/manager",
            get(api_telegram_manager)
                .post(api_connect_telegram_manager)
                .delete(api_disconnect_telegram_manager)
                .layer(DefaultBodyLimit::max(16 * 1024)),
        )
        .route(
            "/notifications/telegram/manager/prepare",
            post(api_prepare_managed_telegram_bot).layer(DefaultBodyLimit::max(8 * 1024)),
        )
        .route("/notifications/mobile-push", get(api_mobile_push_status))
        .route(
            "/notifications/mobile-push/config",
            get(api_mobile_push_configuration)
                .post(api_configure_mobile_push)
                .layer(DefaultBodyLimit::max(128 * 1024)),
        )
        .route(
            "/notifications/mobile-push/devices",
            post(api_register_mobile_push_device).layer(DefaultBodyLimit::max(8 * 1024)),
        )
        .route(
            "/notifications/mobile-push/devices/:device_id",
            delete(api_unregister_mobile_push_device),
        )
        .route(
            "/notifications/mobile-push/test",
            post(api_test_mobile_push),
        )
        .route(
            "/notifications/whatsapp/webhook",
            get(api_verify_whatsapp_webhook)
                .post(api_receive_whatsapp_webhook)
                .layer(DefaultBodyLimit::max(
                    whatsapp_notifications::MAX_WHATSAPP_WEBHOOK_BYTES,
                )),
        )
        .route(
            "/image/capabilities",
            get(api_image_generation_capabilities),
        )
        .route(
            "/image/generations",
            post(api_start_image_generation).layer(DefaultBodyLimit::max(
                image_generation::MAX_IMAGE_GENERATION_REQUEST_BYTES,
            )),
        )
        .route(
            "/image/generations/status",
            post(api_image_generation_status),
        )
        .route(
            "/image/generations/cancel",
            post(api_cancel_image_generation),
        )
        .route(
            "/video/capabilities",
            get(api_video_generation_capabilities),
        )
        .route(
            "/video/generations",
            post(api_start_video_generation).layer(DefaultBodyLimit::max(
                video_generation::MAX_VIDEO_REQUEST_BYTES,
            )),
        )
        .route(
            "/video/generations/status",
            post(api_video_generation_status),
        )
        .route(
            "/video/generations/cancel",
            post(api_cancel_video_generation),
        )
        .route(
            "/chat/turns/:id",
            get(api_chat_turn_status).delete(api_stop_chat_turn),
        )
        .route(
            "/autonomous-agents",
            get(api_list_autonomous_agents).post(api_create_autonomous_agent),
        )
        .route(
            "/autonomous-agents/:id/control",
            post(api_control_autonomous_agent),
        )
        .route(
            "/autonomous-agents/:id/schedule",
            post(api_schedule_autonomous_agent),
        )
        .route(
            "/autonomous-agents/:id/account",
            post(api_reassign_autonomous_agent_account),
        )
        .route(
            "/autonomous-agents/:id/messages",
            post(api_send_autonomous_agent_message),
        )
        .route(
            "/autonomous-agents/:id/review-policy",
            post(api_apply_autonomous_review_policy),
        )
        .route(
            "/autonomous-agents/:id/reviews/:review_id/evidence",
            get(api_read_autonomous_review_evidence),
        )
        .route(
            "/autonomous-agents/:id/memories",
            post(api_add_autonomous_agent_memory),
        )
        .route(
            "/autonomous-agents/:id/reports/:report_id/read",
            post(api_mark_autonomous_agent_report_read),
        )
        .route(
            "/autonomous-agents/:id/memories/:memory_id",
            delete(api_delete_autonomous_agent_memory),
        )
        .route(
            "/autonomous-agents/:id/orchestration",
            post(api_promote_autonomous_agent_to_orchestration),
        )
        .route(
            "/autonomous-agents/:id",
            post(api_update_autonomous_agent).delete(api_delete_autonomous_agent),
        )
        .route(
            "/orchestrations",
            get(api_list_orchestrations).post(api_create_orchestration),
        )
        .route(
            "/orchestrations/:id/control",
            post(api_control_orchestration),
        )
        .route(
            "/orchestrations/:id/account",
            post(api_reassign_orchestration_account),
        )
        .route("/orchestrations/:id", delete(api_delete_orchestration))
        .route("/doctolib-lab/status", get(api_doctolib_lab_status))
        .route("/doctolib-lab/connect", post(api_doctolib_lab_connect))
        .route(
            "/doctolib-lab/google-calendar/connect",
            post(api_doctolib_lab_google_calendar_connect),
        )
        .route("/doctolib-lab/search", post(api_doctolib_lab_search))
        .route("/doctolib-lab/confirm", post(api_doctolib_lab_confirm))
        .route("/pool/status", get(api_pool_status))
        .route("/pool/start", post(api_pool_status))
        .route("/pool/stop", post(api_pool_stop))
        .route("/terminals", post(api_start_terminal))
        .route("/terminals/active", get(api_list_active_terminals))
        .route("/terminals/:id/write", post(api_write_terminal))
        .route("/terminals/:id/resize", post(api_resize_terminal))
        .route("/terminals/:id", delete(api_stop_terminal))
        .route("/kombai/status", get(api_kombai_status))
        .route("/kombai/start", post(api_kombai_start))
        .route("/kombai/stop", post(api_kombai_stop))
        .route(
            "/kombai/install-extension",
            post(api_kombai_install_extension),
        )
        .route(
            "/workspaces",
            get(api_workspaces).post(api_create_workspace),
        )
        .route("/workspaces/access", get(api_workspace_access))
        .route(
            "/workspaces/access/request",
            post(api_request_workspace_access),
        )
        .route(
            "/workspaces/:id/access-requests/:user_id/accept",
            post(api_accept_workspace_access),
        )
        .route(
            "/workspaces/:id/access-requests/:user_id/reject",
            post(api_reject_workspace_access),
        )
        .route(
            "/workspaces/:id/members/:user_id",
            delete(api_revoke_workspace_access),
        )
        .route(
            "/workspaces/git-docker",
            post(api_create_git_docker_environment),
        )
        .route("/workspaces/:id", delete(api_delete_workspace))
        .route("/fs/list", get(api_fs_list))
        .with_state(state.clone());

    let ws = Router::new()
        .route("/terminals/:id", get(ws_terminal))
        .route("/discussions", get(ws_discussions))
        .route("/runtime", get(ws_runtime))
        .with_state(state.clone());

    // Liveness NON authentifiee, a la racine (`/healthz`) : l'auth de ce projet
    // est appliquee par handler, pas par couche de routeur, donc ce handler est
    // simplement lisible sans token.
    let health = Router::new()
        .route("/healthz", get(api_healthz))
        .with_state(state.clone());

    let public = Router::new()
        .route("/t/:slug", get(public_tracking_redirect))
        .with_state(state.clone());

    let mcp = Router::new()
        .route(
            "/mcp/chat-tools",
            post(mcp_chat_tools).layer(DefaultBodyLimit::max(64 * 1024)),
        )
        .with_state(state.clone());

    // Les builds web produisent les variantes .br et .gz. ServeDir les envoie
    // directement selon Accept-Encoding : le premier telephone qui ouvre une
    // nouvelle version ne paie plus la compression des gros bundles a chaud.
    let static_service = ServeDir::new(config.static_dir.clone())
        .precompressed_br()
        .precompressed_gzip()
        .not_found_service(
            ServeDir::new(config.static_dir.clone())
                .precompressed_br()
                .precompressed_gzip(),
        );

    let app = Router::new()
        .merge(health)
        .merge(public)
        .merge(mcp)
        .nest("/api/auth", auth::router(user_auth))
        .nest("/api/microsoft", microsoft::router(microsoft))
        .nest("/api/gmail", gmail::router(gmail))
        .nest("/api", api)
        .nest("/ws", ws)
        .merge(pool::router(pool_manager, Some(config.admin_token.clone())))
        .fallback_service(static_service)
        .layer(middleware::from_fn(set_frontend_cache_control))
        .layer(CompressionLayer::new())
        // Les clients mobiles peuvent joindre un noeud sur une autre origine.
        // Mettre en cache le preflight evite un OPTIONS avant chaque poll API.
        .layer(CorsLayer::very_permissive().max_age(Duration::from_secs(24 * 60 * 60)));

    let addr: SocketAddr = config
        .bind
        .parse()
        .map_err(|error| format!("CST_BIND invalide: {error}"))?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|error| format!("bind {addr} impossible: {error}"))?;

    println!(
        "Codex Switch Terminal SaaS listening on http://{} (node: {}, capacity: {}, data: {}, static: {})",
        addr,
        config.node_label,
        config.node_capacity,
        config.data_dir.display(),
        config.static_dir.display()
    );

    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|error| error.to_string())
}

/// Conserve les secrets dans `ServerConfig`, mais les retire de l'environnement
/// avant la creation du moindre provider ou terminal. Sous Linux, le processus
/// devient aussi non inspectable par les autres processus du meme UID : sans
/// cette seconde barriere, `/proc/<pid>/environ` peut encore exposer
/// l'environnement initial meme apres `remove_var`.
fn isolate_server_control_secrets() -> Result<(), String> {
    for name in SERVER_CONTROL_SECRET_ENV_VARS {
        std::env::remove_var(name);
    }
    prevent_same_user_process_inspection()
}

#[cfg(target_os = "linux")]
fn prevent_same_user_process_inspection() -> Result<(), String> {
    // SAFETY: `prctl(PR_SET_DUMPABLE, 0)` est une operation locale au
    // processus. Les arguments inutilises sont explicitement mis a zero comme
    // recommande par prctl(2), avec les types ABI Linux attendus.
    let result = unsafe {
        libc::prctl(
            libc::PR_SET_DUMPABLE,
            0 as libc::c_ulong,
            0 as libc::c_ulong,
            0 as libc::c_ulong,
            0 as libc::c_ulong,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(format!(
            "isolation des secrets du serveur impossible: {}",
            std::io::Error::last_os_error()
        ))
    }
}

#[cfg(not(target_os = "linux"))]
fn prevent_same_user_process_inspection() -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "windows")]
fn embedded_device_connector_enabled() -> bool {
    !std::env::var("CST_DEVICE_EMBEDDED_CONNECTOR")
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "off"
            )
        })
}

#[cfg(target_os = "windows")]
async fn run_embedded_device_connector(manager: DeviceFleetManager) {
    let first_snapshot = device_fleet::inspect_local_fleet().unwrap_or_default();
    let connector_id = first_snapshot
        .connector_id
        .clone()
        .unwrap_or_else(|| "usb-windows-server".to_string());
    let heartbeat_manager = manager.clone();
    let heartbeat_connector_id = connector_id.clone();
    let heartbeat = async move {
        loop {
            let inventory = tokio::task::spawn_blocking(device_fleet::inspect_local_fleet).await;
            let request = match inventory {
                Ok(Ok(snapshot)) => DeviceConnectorHeartbeatRequest {
                    connector_id: heartbeat_connector_id.clone(),
                    devices: snapshot.devices,
                    tools: snapshot.tools,
                    error: snapshot.error,
                },
                Ok(Err(error)) => DeviceConnectorHeartbeatRequest {
                    connector_id: heartbeat_connector_id.clone(),
                    devices: Vec::new(),
                    tools: Default::default(),
                    error: Some(error.to_string()),
                },
                Err(error) => DeviceConnectorHeartbeatRequest {
                    connector_id: heartbeat_connector_id.clone(),
                    devices: Vec::new(),
                    tools: Default::default(),
                    error: Some(format!("Inventaire USB interrompu : {error}")),
                },
            };
            let _ = heartbeat_manager.heartbeat(request);
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    };

    let workers = async move {
        let permits = Arc::new(tokio::sync::Semaphore::new(8));
        loop {
            let permit = match permits.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => return,
            };
            match manager.claim_next(&connector_id) {
                Ok(Some(job)) => {
                    let worker_manager = manager.clone();
                    let worker_connector_id = connector_id.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        let request = job.request.clone();
                        let action_id = job.action_id.clone();
                        let result = match tokio::task::spawn_blocking({
                            let worker_manager = worker_manager.clone();
                            let request = request.clone();
                            let action_id = action_id.clone();
                            move || worker_manager.execute_local_action(&action_id, request)
                        })
                        .await
                        {
                            Ok(Ok(result)) => result,
                            Ok(Err(error)) => {
                                embedded_device_failure(&action_id, &request, &error.to_string())
                            }
                            Err(error) => embedded_device_failure(
                                &action_id,
                                &request,
                                &format!("Execution USB interrompue : {error}"),
                            ),
                        };
                        let _ = worker_manager.report(DeviceConnectorReportRequest {
                            connector_id: worker_connector_id,
                            action_id: job.action_id,
                            claim_token: job.claim_token,
                            result,
                        });
                    });
                }
                Ok(None) | Err(_) => {
                    drop(permit);
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
        }
    };
    tokio::join!(heartbeat, workers);
}

#[cfg(target_os = "windows")]
fn embedded_device_failure(
    action_id: &str,
    request: &DeviceActionRequest,
    detail: &str,
) -> DeviceActionResult {
    let now = metrics::now_ts();
    DeviceActionResult {
        action_id: action_id.to_string(),
        device_id: request.device_id.clone(),
        action: request.action,
        success: false,
        detail: detail.chars().take(500).collect(),
        stdout: None,
        stderr: None,
        data_base64: None,
        mime_type: None,
        started_at: now,
        finished_at: now,
        truncated: detail.chars().count() > 500,
    }
}

/// Arret propre sur Ctrl-C (SIGINT) ET SIGTERM (envoye par `systemctl restart`
/// / `Stop-ScheduledTask`). Sans la branche SIGTERM, un redemarrage coupait
/// l'HTTP en vol au lieu de laisser Axum terminer les requetes en cours.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

impl ServerConfig {
    fn from_env() -> Result<Self, String> {
        let data_dir = std::env::var_os("CST_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/srv/cst"));
        // Loopback par defaut : un bind sur `0.0.0.0` declenche la fenetre
        // "Pare-feu Windows" (admin) a chaque demarrage tant qu'aucune regle
        // n'est acceptee. Pour exposer le serveur au LAN (telephone/tablette),
        // definir explicitement `CST_BIND=0.0.0.0:8080` (la fenetre pare-feu
        // n'apparait alors qu'une seule fois). Les noeuds de deploiement fixent
        // deja `CST_BIND` derriere un reverse-proxy, donc ce defaut ne les change
        // pas.
        let bind = std::env::var("CST_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
        let admin_token = std::env::var("CST_ADMIN_TOKEN")
            .map(|value| value.trim().to_string())
            .unwrap_or_default();
        if admin_token.is_empty() {
            return Err("CST_ADMIN_TOKEN est requis pour lancer le serveur SaaS".to_string());
        }

        let static_dir = std::env::var_os("CST_STATIC_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(default_static_dir);
        let public_base_url =
            std::env::var("CST_PUBLIC_BASE_URL").unwrap_or_else(|_| format!("http://{bind}"));
        let node_id =
            std::env::var("CST_NODE_ID").unwrap_or_else(|_| default_node_name(&public_base_url));
        let node_label = std::env::var("CST_NODE_LABEL").unwrap_or_else(|_| node_id.clone());
        let node_capacity = std::env::var("CST_NODE_CAPACITY")
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or_else(crate::chat::configured_max_active_chat_turns);
        let terminal_capacity = std::env::var("CST_TERMINAL_CAPACITY")
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or_else(crate::resource_profile::configured_terminal_capacity);
        let workspaces_root = resolve_workspaces_root(&data_dir);
        let duello_referral_api_url = std::env::var("CST_DUELLO_REFERRAL_API_URL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let duello_referral_api_key = std::env::var("CST_DUELLO_REFERRAL_API_KEY")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let duello_app_url = std::env::var("CST_DUELLO_APP_URL")
            .unwrap_or_else(|_| "https://app.duello.fr/".to_string());
        let duello_bank = DuelloBankConfig::from_env();
        Ok(ServerConfig {
            bind,
            data_dir,
            static_dir,
            admin_token,
            git_pat: std::env::var("CST_GIT_PAT").unwrap_or_default(),
            public_base_url,
            node_id,
            node_label,
            node_capacity,
            terminal_capacity,
            workspaces_root,
            duello_referral_api_url,
            duello_referral_api_key,
            duello_app_url,
            duello_bank,
        })
    }

    fn chat_tools_mcp_url(&self) -> Result<String, String> {
        let bound: SocketAddr = self
            .bind
            .parse()
            .map_err(|error| format!("CST_BIND invalide pour MCP : {error}"))?;
        let ip = if bound.ip().is_unspecified() {
            if bound.is_ipv4() {
                IpAddr::V4(Ipv4Addr::LOCALHOST)
            } else {
                IpAddr::V6(Ipv6Addr::LOCALHOST)
            }
        } else {
            bound.ip()
        };
        Ok(format!(
            "http://{}/mcp/chat-tools",
            SocketAddr::new(ip, bound.port())
        ))
    }

    /// URL locale remise aux terminaux Freebuff. Le jeton associe n'est
    /// accepte que par les routes device-fleet : il ne donne jamais acces aux
    /// comptes, discussions, workspaces ou operations d'administration.
    fn device_terminal_api_url(&self) -> Result<String, String> {
        let bound: SocketAddr = self
            .bind
            .parse()
            .map_err(|error| format!("CST_BIND invalide pour le controle USB : {error}"))?;
        let ip = if bound.ip().is_unspecified() {
            if bound.is_ipv4() {
                IpAddr::V4(Ipv4Addr::LOCALHOST)
            } else {
                IpAddr::V6(Ipv6Addr::LOCALHOST)
            }
        } else {
            bound.ip()
        };
        Ok(format!(
            "http://{}/api/device-fleet",
            SocketAddr::new(ip, bound.port())
        ))
    }

    fn device_terminal_helper_dir(&self) -> PathBuf {
        self.data_dir.join("bin")
    }
}

fn install_device_terminal_helper(config: &ServerConfig) -> Result<(), String> {
    let helper_dir = config.device_terminal_helper_dir();
    fs::create_dir_all(&helper_dir).map_err(|error| {
        format!(
            "Creation du dossier de l'aide device impossible ({}): {error}",
            helper_dir.display()
        )
    })?;

    #[cfg(windows)]
    {
        let wrapper = helper_dir.join("cst-device.cmd");
        fs::write(
            &wrapper,
            b"@echo off\r\nif \"%CST_SERVER_BIN%\"==\"\" (echo CST_SERVER_BIN est absent 1>&2 & exit /b 2)\r\n\"%CST_SERVER_BIN%\" device %*\r\n",
        )
        .map_err(|error| format!("Ecriture de {} impossible: {error}", wrapper.display()))?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let wrapper = helper_dir.join("cst-device");
        fs::write(
            &wrapper,
            b"#!/bin/sh\nif [ -z \"${CST_SERVER_BIN:-}\" ]; then echo 'CST_SERVER_BIN est absent' >&2; exit 2; fi\nexec \"$CST_SERVER_BIN\" device \"$@\"\n",
        )
        .map_err(|error| format!("Ecriture de {} impossible: {error}", wrapper.display()))?;
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).map_err(|error| {
            format!("Permissions de {} impossibles: {error}", wrapper.display())
        })?;
    }

    Ok(())
}

fn path_with_device_terminal_helper(config: &ServerConfig) -> Result<std::ffi::OsString, String> {
    let current = std::env::var_os("PATH").unwrap_or_default();
    std::env::join_paths(
        std::iter::once(config.device_terminal_helper_dir()).chain(std::env::split_paths(&current)),
    )
    .map_err(|error| format!("PATH du terminal USB invalide : {error}"))
}

/// Petit client borne utilise par le wrapper `cst-device` des terminaux
/// Freebuff. Toute l'autorisation reste cote serveur et le wrapper ne contient
/// aucun secret : il lit uniquement la capacite device-only de son environnement.
const DEVICE_TERMINAL_IDEMPOTENCY_KEY_MAX_BYTES: usize = 128;
const DEVICE_TERMINAL_ACTION_MAX_RETRIES: usize = 3;
const DEVICE_TERMINAL_ACTION_RETRY_DELAY: Duration = Duration::from_millis(150);

#[derive(Debug, Clone, PartialEq)]
struct DeviceTerminalActionCommand {
    device_id: String,
    action: String,
    args: Value,
    confirmed: bool,
    idempotency_key: String,
}

fn parse_device_terminal_action(
    arguments: &[String],
) -> Result<DeviceTerminalActionCommand, String> {
    if arguments.len() < 2 {
        return Err(device_terminal_cli_usage());
    }
    let device_id = arguments[0].trim().to_string();
    let action = arguments[1].trim().to_string();
    let supported = matches!(
        action.as_str(),
        "info"
            | "screenshot"
            | "open_screen"
            | "tap"
            | "swipe"
            | "type_text"
            | "key_event"
            | "open_app"
            | "shell"
    );
    if device_id.is_empty() || !supported {
        return Err(device_terminal_cli_usage());
    }

    let mut args = None;
    let mut confirmed = false;
    let mut idempotency_key = None;
    let mut index = 2;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--confirm" => {
                if confirmed {
                    return Err("Le flag --confirm ne peut apparaitre qu'une fois".to_string());
                }
                confirmed = true;
                index += 1;
            }
            "--idempotency-key" => {
                if idempotency_key.is_some() {
                    return Err(
                        "Le flag --idempotency-key ne peut apparaitre qu'une fois".to_string()
                    );
                }
                let key = arguments
                    .get(index + 1)
                    .ok_or_else(|| "Le flag --idempotency-key exige une cle ASCII".to_string())?;
                if key.starts_with("--") {
                    return Err("Le flag --idempotency-key exige une cle ASCII".to_string());
                }
                validate_device_terminal_idempotency_key(key)?;
                idempotency_key = Some(key.clone());
                index += 2;
            }
            option if option.starts_with("--") => {
                return Err(format!("Option cst-device action inconnue : {option}"));
            }
            raw => {
                if args.is_some() {
                    return Err("Un seul objet JSON args est accepte".to_string());
                }
                let value: Value = serde_json::from_str(raw)
                    .map_err(|error| format!("ARGS_JSON invalide : {error}"))?;
                if !value.is_object() {
                    return Err("ARGS_JSON doit etre un objet JSON".to_string());
                }
                args = Some(value);
                index += 1;
            }
        }
    }

    if !matches!(action.as_str(), "info" | "screenshot") && !confirmed {
        return Err(format!(
            "L'action {action} modifie ou ouvre l'appareil : confirmez la demande explicite de l'utilisateur avec --confirm"
        ));
    }

    Ok(DeviceTerminalActionCommand {
        device_id,
        action,
        args: args.unwrap_or_else(|| json!({})),
        confirmed,
        idempotency_key: idempotency_key.unwrap_or_else(|| Uuid::new_v4().to_string()),
    })
}

fn validate_device_terminal_idempotency_key(key: &str) -> Result<(), String> {
    if key.is_empty()
        || key.len() > DEVICE_TERMINAL_IDEMPOTENCY_KEY_MAX_BYTES
        || !key.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | ':')
        })
    {
        return Err(format!(
            "IDEMPOTENCY_KEY doit contenir entre 1 et {DEVICE_TERMINAL_IDEMPOTENCY_KEY_MAX_BYTES} caracteres ASCII parmi lettres, chiffres, '.', '_', '-' et ':'"
        ));
    }
    Ok(())
}

fn device_terminal_action_payload(command: &DeviceTerminalActionCommand) -> Value {
    json!({
        "deviceId": command.device_id,
        "action": command.action,
        "args": command.args,
        "confirmed": command.confirmed,
        "idempotencyKey": command.idempotency_key,
    })
}

async fn retry_ambiguous_device_terminal_action<T, E, F, Fut>(
    payload: Vec<u8>,
    mut send: F,
    retry_delay: Duration,
) -> Result<T, E>
where
    F: FnMut(Vec<u8>) -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    let mut retries = 0;
    loop {
        match send(payload.clone()).await {
            Ok(response) => return Ok(response),
            Err(error) => {
                if retries >= DEVICE_TERMINAL_ACTION_MAX_RETRIES {
                    return Err(error);
                }
                retries += 1;
                if !retry_delay.is_zero() {
                    tokio::time::sleep(retry_delay).await;
                }
            }
        }
    }
}

pub async fn run_device_terminal_cli(arguments: &[String]) -> Result<(), String> {
    let api_url = std::env::var("CST_DEVICE_API_URL")
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .map_err(|_| "CST_DEVICE_API_URL est absent de ce terminal".to_string())?;
    let parsed_url = url::Url::parse(&api_url)
        .map_err(|error| format!("CST_DEVICE_API_URL est invalide : {error}"))?;
    let local_host = parsed_url
        .host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("localhost"))
        || parsed_url
            .host_str()
            .and_then(|host| host.parse::<IpAddr>().ok())
            .is_some_and(|ip| ip.is_loopback());
    if !matches!(parsed_url.scheme(), "http" | "https") || !local_host {
        return Err("CST_DEVICE_API_URL doit cibler le serveur Switch local".to_string());
    }
    let token = std::env::var("CST_DEVICE_TOKEN")
        .map(|value| value.trim().to_string())
        .map_err(|_| "CST_DEVICE_TOKEN est absent de ce terminal".to_string())?;
    if token.is_empty() {
        return Err("CST_DEVICE_TOKEN est vide".to_string());
    }

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|error| format!("Client de controle USB indisponible : {error}"))?;
    let (status, bytes) = match arguments.first().map(String::as_str) {
        Some("list") if arguments.len() == 1 => {
            let response = client
                .get(&api_url)
                .bearer_auth(&token)
                .send()
                .await
                .map_err(|error| format!("Serveur de controle USB injoignable : {error}"))?;
            let status = response.status();
            let bytes = response
                .bytes()
                .await
                .map_err(|error| format!("Reponse du controle USB illisible : {error}"))?;
            (status, bytes)
        }
        Some("status") if arguments.len() == 2 => {
            let action_id = Uuid::parse_str(arguments[1].trim()).map_err(|_| {
                "ACTION_ID doit etre un UUID retourne par cst-device action".to_string()
            })?;
            let response = client
                .get(format!("{api_url}/actions/{action_id}"))
                .bearer_auth(&token)
                .send()
                .await
                .map_err(|error| format!("Serveur de controle USB injoignable : {error}"))?;
            let status = response.status();
            let bytes = response
                .bytes()
                .await
                .map_err(|error| format!("Reponse du controle USB illisible : {error}"))?;
            (status, bytes)
        }
        Some("action") => {
            let command = parse_device_terminal_action(&arguments[1..])?;
            let payload = serde_json::to_vec(&device_terminal_action_payload(&command))
                .map_err(|error| format!("Action USB non serialisable : {error}"))?;
            let action_url = format!("{api_url}/actions");
            retry_ambiguous_device_terminal_action(
                payload,
                |body| {
                    let request = client
                        .post(&action_url)
                        .bearer_auth(&token)
                        .header(CONTENT_TYPE, "application/json")
                        .body(body);
                    async move {
                        let response = request.send().await?;
                        let status = response.status();
                        let bytes = response.bytes().await?;
                        Ok::<_, reqwest::Error>((status, bytes))
                    }
                },
                DEVICE_TERMINAL_ACTION_RETRY_DELAY,
            )
            .await
            .map_err(|error| {
                format!(
                    "Serveur de controle USB injoignable ou reponse interrompue apres {} tentatives : {error}",
                    DEVICE_TERMINAL_ACTION_MAX_RETRIES + 1
                )
            })?
        }
        _ => return Err(device_terminal_cli_usage()),
    };
    if !status.is_success() {
        let detail = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|value| {
                value
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&bytes).chars().take(500).collect());
        return Err(format!("Controle USB refuse ({status}) : {detail}"));
    }
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) => println!(
            "{}",
            serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
        ),
        Err(_) => println!("{}", String::from_utf8_lossy(&bytes)),
    }
    Ok(())
}

fn device_terminal_cli_usage() -> String {
    "Usage: cst-device list | cst-device action DEVICE_ID ACTION [ARGS_JSON] [--confirm] [--idempotency-key KEY] | cst-device status ACTION_ID".to_string()
}

fn default_node_name(public_base_url: &str) -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .map(|value| value.trim().to_string())
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| public_base_url.replace([':', '/', '.'], "-"))
}

fn default_static_dir() -> PathBuf {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| cwd.parent().map(|parent| parent.join("dist")))
        .unwrap_or_else(|| PathBuf::from("dist"))
}

/// Racine du navigateur de dossiers / des workspaces "dossier existant".
/// Priorite : `CST_WORKSPACES_ROOT`, puis le dossier personnel
/// (`USERPROFILE`/`HOME`), puis le repertoire de donnees. Le dossier est cree si
/// besoin, puis canonicalise (best-effort) pour permettre une comparaison de
/// prefixe fiable lors de la validation des chemins.
fn resolve_workspaces_root(data_dir: &Path) -> PathBuf {
    let root = std::env::var_os("CST_WORKSPACES_ROOT")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| data_dir.to_path_buf());
    let _ = fs::create_dir_all(&root);
    fs::canonicalize(&root).unwrap_or(root)
}

/// Retire le prefixe Windows de chemin etendu (`\\?\`) pour un affichage propre
/// et un `cwd` utilisable. No-op sur les autres plateformes.
fn strip_extended_prefix(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let value = path.to_string_lossy();
        if let Some(rest) = value.strip_prefix(r"\\?\") {
            if let Some(unc) = rest.strip_prefix("UNC\\") {
                return PathBuf::from(format!(r"\\{unc}"));
            }
            return PathBuf::from(rest);
        }
    }
    path.to_path_buf()
}

fn display_path(path: &Path) -> String {
    strip_extended_prefix(path).to_string_lossy().to_string()
}

/// Valide qu'un chemin demande (absolu, ou relatif a la racine) existe, est un
/// dossier, et se situe A L'INTERIEUR de la racine autorisee. Empeche les
/// echappements par `..` et par lien symbolique (grace a la canonicalisation).
/// Renvoie le chemin canonique nettoye (sans prefixe `\\?\`).
fn resolve_within_root(root: &Path, requested: &str) -> Result<PathBuf, String> {
    let requested = requested.trim();
    if requested.is_empty() {
        return Ok(strip_extended_prefix(root));
    }
    let candidate = {
        // `settings::expand_home` traduit aussi les chemins absolus Windows
        // quand le serveur web tourne sous WSL (`C:\\...` -> `/mnt/c/...`).
        let expanded = settings::expand_home(requested)?;
        let path = expanded.as_path();
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        }
    };
    let canonical =
        fs::canonicalize(&candidate).map_err(|_| format!("dossier introuvable: {requested}"))?;
    // Comparaison sur les formes nettoyees pour eviter tout desaccord de prefixe
    // (`\\?\`) entre la racine et le candidat.
    let root_norm = strip_extended_prefix(root);
    let canonical_norm = strip_extended_prefix(&canonical);
    if !canonical_norm.starts_with(&root_norm) {
        return Err("dossier hors de la racine autorisee".to_string());
    }
    if !canonical_norm.is_dir() {
        return Err(format!("pas un dossier: {requested}"));
    }
    Ok(canonical_norm)
}

/// Liste UNIQUEMENT les sous-dossiers d'un repertoire (pour un selecteur de
/// dossier), tries par nom (insensible a la casse). Les fichiers sont ignores.
fn list_subdirs(dir: &Path) -> Result<Vec<FsEntry>, String> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let is_dir = entry
            .file_type()
            .map(|file_type| file_type.is_dir())
            .unwrap_or_else(|_| path.is_dir());
        if !is_dir {
            continue;
        }
        entries.push(FsEntry {
            name: entry.file_name().to_string_lossy().to_string(),
            path: display_path(&path),
            is_dir: true,
        });
    }
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(entries)
}

/// Identifiant stable (sans separateur de chemin) pour un workspace pointant un
/// dossier existant. Sert d'etiquette de routage ; n'est jamais utilise pour
/// supprimer quoi que ce soit (la purge ne touche que `data_dir/workspaces`).
fn workspace_id_for_dir(dir: &Path) -> String {
    let mut id = String::from("dir-");
    for character in dir.to_string_lossy().chars() {
        if character.is_ascii_alphanumeric() {
            id.push(character.to_ascii_lowercase());
        } else if !id.ends_with('-') {
            id.push('-');
        }
    }
    id.trim_end_matches('-').chars().take(96).collect()
}

async fn api_healthz(State(state): State<Arc<ServerState>>) -> Response {
    let draining = is_draining(&state);
    json_response(LivenessResponse {
        ok: true,
        node_id: state.config.node_id.clone(),
        version: VERSION,
        commit: COMMIT,
        // `ready` = pret a accepter de NOUVEAUX terminaux. Un noeud en drain se
        // declare non pret (semantique readiness type k8s) tout en restant
        // vivant pour ses sessions en cours.
        ready: !draining,
        draining,
        active_terminals: state.terminals.active_count(),
        active_chat_turns: state.chat.active_count(),
        capacity: state.config.node_capacity,
        terminal_capacity: state.config.terminal_capacity,
        cleanup_pending: cleanup_pending(&state),
    })
}

fn cleanup_pending(state: &ServerState) -> bool {
    state.cleanup.requested_at.load(Ordering::Acquire) > 0
}

/// L'utilisateur demande un nettoyage depuis l'interface web. Le serveur se
/// contente d'enregistrer la demande : le gardien Windows (`Protect-Switch-
/// PrepAppDisk.ps1 -Mode Watch`) la voit via `/healthz` et execute le travail
/// sur l'hote, puis publie le resultat via `POST /api/cleanup/result`.
async fn api_cleanup_request(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }
    state
        .cleanup
        .requested_at
        .store(metrics::now_ts(), Ordering::Release);
    json_response(serde_json::json!({
        "ok": true,
        "pending": true,
        "requestedAt": metrics::now_ts(),
    }))
}

async fn api_cleanup_status(State(state): State<Arc<ServerState>>, headers: HeaderMap) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }
    let requested_at = state.cleanup.requested_at.load(Ordering::Acquire);
    let last_result = state
        .cleanup
        .last_result
        .lock()
        .ok()
        .and_then(|guard| guard.clone());
    json_response(serde_json::json!({
        "pending": requested_at > 0,
        "requestedAt": requested_at,
        "lastResult": last_result,
    }))
}

async fn api_cleanup_result(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<serde_json::Value>,
) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }
    state.cleanup.requested_at.store(0, Ordering::Release);
    if let Ok(mut guard) = state.cleanup.last_result.lock() {
        *guard = Some(request);
    }
    json_response(serde_json::json!({ "ok": true, "pending": false }))
}

async fn api_tracking_links(State(state): State<Arc<ServerState>>, headers: HeaderMap) -> Response {
    if let Err(response) = request_actor(&state, &headers) {
        return response;
    }
    match state.tracking.snapshot() {
        Ok(snapshot) => json_response(snapshot),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    }
}

async fn api_create_tracking_link(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<CreateTrackingLinkRequest>,
) -> Response {
    if let Err(response) = request_actor(&state, &headers) {
        return response;
    }
    match state.tracking.create(request) {
        Ok(link) => (StatusCode::CREATED, Json(link)).into_response(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, &error, &state.config),
    }
}

async fn api_delete_tracking_link(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(slug): AxumPath<String>,
) -> Response {
    if let Err(response) = request_actor(&state, &headers) {
        return response;
    }
    match state.tracking.delete(&slug) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => api_error(
            StatusCode::NOT_FOUND,
            "Lien de tracking introuvable",
            &state.config,
        ),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    }
}

async fn api_duello_bank_snapshot(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = check_maintenance_header(&state, &headers) {
        return duello_bank_no_store(response);
    }
    match state.duello_bank.snapshot().await {
        Ok(snapshot) => duello_bank_no_store(json_response(snapshot)),
        Err(error) => duello_bank_error_response(&state, error),
    }
}

async fn api_credit_duello_wallet(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    request: Result<Json<CreditDuelloWalletRequest>, JsonRejection>,
) -> Response {
    if let Err(response) = check_maintenance_header(&state, &headers) {
        return duello_bank_no_store(response);
    }
    if !websocket_origin_allowed(&state.config, &headers) {
        return duello_bank_no_store(api_error(
            StatusCode::FORBIDDEN,
            "origine de requete refusee",
            &state.config,
        ));
    }
    let Json(request) = match request {
        Ok(request) => request,
        Err(_) => {
            return duello_bank_no_store(api_error(
                StatusCode::BAD_REQUEST,
                "requete JSON Banque Duello invalide",
                &state.config,
            ));
        }
    };
    match state.duello_bank.credit(request).await {
        Ok(result) => duello_bank_no_store(json_response(result)),
        Err(error) => duello_bank_error_response(&state, error),
    }
}

fn duello_bank_error_response(state: &Arc<ServerState>, error: DuelloBankError) -> Response {
    let status = match error.kind {
        DuelloBankErrorKind::BadRequest => StatusCode::BAD_REQUEST,
        DuelloBankErrorKind::NotFound => StatusCode::NOT_FOUND,
        DuelloBankErrorKind::Conflict => StatusCode::CONFLICT,
        DuelloBankErrorKind::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
        DuelloBankErrorKind::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    };
    duello_bank_no_store(api_error(status, &error.message, &state.config))
}

fn duello_bank_no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn public_tracking_redirect(
    State(state): State<Arc<ServerState>>,
    AxumPath(slug): AxumPath<String>,
) -> Response {
    match state.tracking.register_click(&slug) {
        Ok(Some(link)) => {
            let mut response = Redirect::temporary(&link.destination_url).into_response();
            response
                .headers_mut()
                .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Ok(None) => (StatusCode::NOT_FOUND, "Lien de tracking introuvable").into_response(),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    }
}

#[derive(Debug, Deserialize)]
struct ReferralQuery {
    #[serde(default)]
    refresh: bool,
}

async fn api_referral_snapshot(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Query(query): Query<ReferralQuery>,
) -> Response {
    if let Err(response) = request_actor(&state, &headers) {
        return response;
    }
    match state.referral.snapshot(query.refresh).await {
        Ok(snapshot) => json_response(snapshot),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    }
}

async fn api_create_referral(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<CreateReferralCodeRequest>,
) -> Response {
    if let Err(response) = request_actor(&state, &headers) {
        return response;
    }
    match state.referral.create(request) {
        Ok(code) => (StatusCode::CREATED, Json(code)).into_response(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, &error, &state.config),
    }
}

async fn api_delete_referral(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(code): AxumPath<String>,
) -> Response {
    if let Err(response) = request_actor(&state, &headers) {
        return response;
    }
    match state.referral.delete(&code) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => api_error(
            StatusCode::NOT_FOUND,
            "Code de parrainage introuvable",
            &state.config,
        ),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    }
}

fn is_draining(state: &ServerState) -> bool {
    drain_lease_active(&state.drain_until, metrics::now_ts())
}

fn drain_lease_active(drain_until: &AtomicI64, now: i64) -> bool {
    loop {
        let deadline = drain_until.load(Ordering::Acquire);
        if deadline == 0 {
            return false;
        }
        if deadline > now {
            return true;
        }
        if drain_until
            .compare_exchange(deadline, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return false;
        }
    }
}

async fn api_health(State(state): State<Arc<ServerState>>, headers: HeaderMap) -> Response {
    auth_or(&state, &headers, || {
        let draining = is_draining(&state);
        let available_account_ids = settings::load_settings_for_terminal()
            .map(|settings| {
                settings
                    .accounts
                    .into_iter()
                    .filter(settings::account_has_auth_tokens)
                    .map(|account| account.id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(json_response(HealthResponse {
            ok: true,
            node_id: state.config.node_id.clone(),
            node_label: state.config.node_label.clone(),
            public_base_url: state.config.public_base_url.clone(),
            version: VERSION,
            commit: COMMIT,
            ready: !draining,
            draining,
            active_terminals: state.terminals.active_count(),
            active_chat_turns: state.chat.active_count(),
            available_account_ids,
            capacity: state.config.node_capacity,
            terminal_capacity: state.config.terminal_capacity,
            started_at: state.started_at,
        }))
    })
}

async fn api_admin_drain(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<DrainRequest>,
) -> Response {
    if let Err(response) = check_maintenance_header(&state, &headers) {
        return response;
    }
    let drain_until = if request.draining {
        let ttl = request
            .ttl_seconds
            .unwrap_or(DEFAULT_DRAIN_LEASE_SECS)
            .clamp(1, MAX_DRAIN_LEASE_SECS);
        metrics::now_ts().saturating_add(ttl as i64)
    } else {
        0
    };
    state.drain_until.store(drain_until, Ordering::Release);
    json_response(json!({
        "draining": request.draining,
        "drainUntil": (drain_until > 0).then_some(drain_until),
        "activeTerminals": state.terminals.active_count(),
    }))
}

async fn api_vps_capabilities(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = check_maintenance_header(&state, &headers) {
        return response;
    }
    json_response(state.vps_deploy.capabilities())
}

async fn api_vps_deployments(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = check_maintenance_header(&state, &headers) {
        return response;
    }
    json_response(state.vps_deploy.jobs())
}

async fn api_vps_deployment(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Response {
    if let Err(response) = check_maintenance_header(&state, &headers) {
        return response;
    }
    match state.vps_deploy.job(&id) {
        Some(job) => json_response(job),
        None => api_error(
            StatusCode::NOT_FOUND,
            "Deploiement VPS introuvable",
            &state.config,
        ),
    }
}

async fn api_vps_start_deployment(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<StartVpsDeployRequest>,
) -> Response {
    if let Err(response) = check_maintenance_header(&state, &headers) {
        return response;
    }
    match state.vps_deploy.start(request) {
        Ok(job) => (StatusCode::CREATED, Json(job)).into_response(),
        Err(error) => vps_deploy_api_error(&state, error),
    }
}

fn vps_deploy_api_error(state: &Arc<ServerState>, error: VpsDeployError) -> Response {
    let status = match &error {
        VpsDeployError::Validation(_) => StatusCode::BAD_REQUEST,
        VpsDeployError::Unsupported(_) => StatusCode::SERVICE_UNAVAILABLE,
        VpsDeployError::Busy(_) => StatusCode::CONFLICT,
        VpsDeployError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    api_error(status, &error.to_string(), &state.config)
}

async fn api_get_settings(State(state): State<Arc<ServerState>>, headers: HeaderMap) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let mut value = match settings::load_settings() {
        Ok(value) => value,
        Err(error) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    };
    if let Some(identity) = actor.user() {
        let (workspaces, closed_workspace_ids) =
            match state.workspace_access.workspace_profiles_for(identity) {
                Ok(value) => value,
                Err(error) => return workspace_access_error(&state, error),
            };
        value.workspaces = workspaces;
        value.closed_workspace_ids = closed_workspace_ids;
    }
    json_response(value)
}

async fn api_put_settings(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(mut incoming): Json<AppSettings>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Some(identity) = actor.user() {
        if let Err(error) = state.workspace_access.sync_profiles(
            identity,
            &incoming.workspaces,
            &incoming.closed_workspace_ids,
        ) {
            return workspace_access_error(&state, error);
        }
        // Le registre global historique ne doit jamais etre remplace par la
        // vue filtree d'un utilisateur. Les environnements SaaS restent dans
        // le registre ACL prive gere ci-dessus.
        let current = match settings::load_settings() {
            Ok(value) => value,
            Err(error) => {
                return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config)
            }
        };
        incoming.workspaces = current.workspaces;
        incoming.closed_workspace_ids = current.closed_workspace_ids;
    }

    let mut saved = match settings::save_settings(incoming) {
        Ok(value) => value,
        Err(error) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    };
    if let Some(identity) = actor.user() {
        let (workspaces, closed_workspace_ids) =
            match state.workspace_access.workspace_profiles_for(identity) {
                Ok(value) => value,
                Err(error) => return workspace_access_error(&state, error),
            };
        saved.workspaces = workspaces;
        saved.closed_workspace_ids = closed_workspace_ids;
    }
    state
        .chat
        .runtime_sync()
        .notify(RuntimeSyncTopic::AccountCompletions);
    json_response(saved)
}

async fn api_get_accounts(State(state): State<Arc<ServerState>>, headers: HeaderMap) -> Response {
    auth_or(&state, &headers, || {
        settings::load_settings().map(|settings| json_response(settings.accounts))
    })
}

async fn api_add_shared_account(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(account): Json<AccountProfile>,
) -> Response {
    auth_or(&state, &headers, || {
        settings::add_shared_account(account).map(json_response)
    })
}

async fn api_import_account(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<ImportAccountRequest>,
) -> Response {
    auth_or(&state, &headers, || {
        settings::import_account_json(request.content).map(json_response)
    })
}

async fn api_ensure_account_home(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<EnsureAccountHomeRequest>,
) -> Response {
    auth_or(&state, &headers, || {
        settings::ensure_account_home(
            request.codex_home,
            request.provider,
            request.bypass,
            request.model,
            request.reasoning_effort,
            Some(request.fast_mode),
        )
        .map(|_| json_response(json!({ "ok": true })))
    })
}

#[derive(Debug, Deserialize)]
struct RemoveAccountQuery {
    #[serde(default, rename = "deleteFiles")]
    delete_files: Option<bool>,
}

async fn api_remove_account(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<RemoveAccountQuery>,
) -> Response {
    let delete_files = query.delete_files.unwrap_or(false);
    auth_or(&state, &headers, || {
        settings::remove_account(id, delete_files).map(json_response)
    })
}

#[derive(Debug, Default, Deserialize)]
struct AccountLimitQuery {
    #[serde(default)]
    force: bool,
}

async fn api_limits(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Query(query): Query<AccountLimitQuery>,
) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }

    match settings::account_limit_status(Some(query.force)).await {
        Ok(mut value) => {
            let unavailable = state.terminals.unavailable_account_ids();
            for row in &mut value {
                if row.provider == Provider::Freebuff && unavailable.contains(&row.id) {
                    row.session_busy = true;
                }
            }
            json_response(value)
        }
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    }
}

async fn api_tokscale_submit(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    auth_or(&state, &headers, || {
        metrics::tokscale_submit_usage().map(json_response)
    })
}

async fn api_usage(State(state): State<Arc<ServerState>>, headers: HeaderMap) -> Response {
    auth_or(&state, &headers, || {
        metrics::usage_dashboard_for_server(state.terminals.active_agent_runs()).map(json_response)
    })
}

async fn api_account_usage(State(state): State<Arc<ServerState>>, headers: HeaderMap) -> Response {
    auth_or(&state, &headers, || {
        account_usage::account_token_usage_dashboard().map(json_response)
    })
}

async fn api_work_time(State(state): State<Arc<ServerState>>, headers: HeaderMap) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }

    match tokio::task::spawn_blocking(work_time::work_time_dashboard_for_server).await {
        Ok(Ok(dashboard)) => json_response(dashboard),
        Ok(Err(error)) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("analyse du temps de travail interrompue: {error}"),
            &state.config,
        ),
    }
}

async fn api_list_discussions(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let dashboard = match tokio::task::spawn_blocking(discussions::list_discussions_dashboard).await
    {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config)
        }
        Err(error) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("analyse des discussions interrompue: {error}"),
                &state.config,
            )
        }
    };
    match actor {
        RequestActor::Administrator => json_response(dashboard),
        RequestActor::User(identity) => json_response(filter_discussions_for_identity(
            &state, &identity, dashboard,
        )),
    }
}

#[derive(Debug, Deserialize)]
struct PromptHistoryQuery {
    limit: Option<usize>,
}

/// Expose au client web le meme historique que la commande Tauri locale.
/// Le plafond evite qu'une requete distante puisse declencher un rendu ou une
/// reponse demesuree ; l'interface demande normalement 4 000 entrees.
async fn api_list_prompt_history(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Query(query): Query<PromptHistoryQuery>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let limit = query.limit.map(|value| value.clamp(1, 10_000));
    let history = match tokio::task::spawn_blocking(move || {
        discussions::list_prompt_history_dashboard(limit)
    })
    .await
    {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config)
        }
        Err(error) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("analyse de l'historique interrompue: {error}"),
                &state.config,
            )
        }
    };

    match actor {
        RequestActor::Administrator => json_response(history),
        RequestActor::User(identity) => json_response(filter_prompt_history_for_identity(
            &state, &identity, history,
        )),
    }
}

fn filter_prompt_history_for_identity(
    state: &Arc<ServerState>,
    identity: &AuthIdentity,
    mut history: discussions::PromptHistory,
) -> discussions::PromptHistory {
    let mut workspace_authorizations = HashMap::<String, bool>::new();
    history.prompts.retain(|prompt| {
        prompt.cwd.as_deref().is_some_and(|cwd| {
            *workspace_authorizations
                .entry(cwd.to_string())
                .or_insert_with(|| {
                    state
                        .workspace_access
                        .authorize_existing_environment(identity, cwd)
                        .is_ok()
                })
        })
    });
    // Ne jamais exposer, meme dans les compteurs, le volume des demandes
    // appartenant a des environnements auxquels cet utilisateur n'a pas acces.
    history.total_prompts = history.prompts.len() as u64;
    history.returned = history.prompts.len() as u64;
    history
}

fn identity_owns_account(
    state: &Arc<ServerState>,
    identity: &AuthIdentity,
    account_id: &str,
) -> bool {
    owned_account_ids_for_identity(state, identity).contains(account_id)
}

fn owned_account_ids_for_identity(
    state: &Arc<ServerState>,
    identity: &AuthIdentity,
) -> HashSet<String> {
    let Some(email) = state.auth.email_for_user_id(&identity.id).ok().flatten() else {
        return HashSet::new();
    };
    let Ok(settings) = settings::load_settings_for_terminal() else {
        return HashSet::new();
    };
    settings
        .accounts
        .iter()
        .filter(|account| account.label.trim().eq_ignore_ascii_case(email.trim()))
        .map(|account| account.id.clone())
        .collect()
}

fn filter_discussions_for_identity(
    state: &Arc<ServerState>,
    identity: &AuthIdentity,
    mut dashboard: discussions::DiscussionsDashboard,
) -> discussions::DiscussionsDashboard {
    let owned_account_ids = owned_account_ids_for_identity(state, identity);
    let mut workspace_authorizations = HashMap::<String, bool>::new();
    for account in &mut dashboard.accounts {
        let account_owner = owned_account_ids.contains(&account.account_id);
        account.discussions.retain(|discussion| {
            account_owner
                || discussion.cwd.as_deref().is_some_and(|cwd| {
                    *workspace_authorizations
                        .entry(cwd.to_string())
                        .or_insert_with(|| {
                            state
                                .workspace_access
                                .authorize_existing_environment(identity, cwd)
                                .is_ok()
                        })
                })
        });
        account.discussion_count = account.discussions.len() as u64;
    }
    dashboard
        .accounts
        .retain(|account| !account.discussions.is_empty());
    dashboard.total_discussions = dashboard
        .accounts
        .iter()
        .map(|account| account.discussion_count)
        .sum();
    dashboard
}

fn authorize_discussion_for_identity(
    state: &Arc<ServerState>,
    identity: &AuthIdentity,
    account_id: &str,
    session_id: &str,
) -> Result<(), Response> {
    let cwd = discussions::discussion_cwd_for_authorization(account_id, session_id)
        .map_err(|error| api_error(StatusCode::NOT_FOUND, &error, &state.config))?;
    if identity_owns_account(state, identity, account_id) {
        return Ok(());
    }
    let cwd = cwd.as_deref().ok_or_else(|| {
        api_error(
            StatusCode::FORBIDDEN,
            "Cette discussion n'est liee a aucun environnement autorise",
            &state.config,
        )
    })?;
    state
        .workspace_access
        .authorize_existing_environment(identity, cwd)
        .map(|_| ())
        .map_err(|_| {
            api_error(
                StatusCode::NOT_FOUND,
                "Discussion introuvable ou inaccessible",
                &state.config,
            )
        })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TranscriptQuery {
    account_id: String,
    session_id: String,
}

async fn api_discussion_transcript(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Query(query): Query<TranscriptQuery>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Some(identity) = actor.user() {
        if let Err(response) = authorize_discussion_for_identity(
            &state,
            identity,
            &query.account_id,
            &query.session_id,
        ) {
            return response;
        }
    }
    match discussions::transcript_for_account(query.account_id, query.session_id) {
        Ok(value) => json_response(value),
        Err(error) => api_error(resource_error_status(&error), &error, &state.config),
    }
}

async fn api_copy_discussion(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<CopyDiscussionRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Some(identity) = actor.user() {
        if let Err(response) = authorize_discussion_for_identity(
            &state,
            identity,
            &request.source_account_id,
            &request.session_id,
        ) {
            return response;
        }
    }
    match discussions::copy_discussion_between(
        request.session_id,
        request.source_account_id,
        request.target_account_id,
    ) {
        Ok(value) => json_response(value),
        Err(error) => api_error(resource_error_status(&error), &error, &state.config),
    }
}

async fn api_move_discussion(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<MoveDiscussionRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Some(identity) = actor.user() {
        if let Err(response) = authorize_discussion_for_identity(
            &state,
            identity,
            &request.account_id,
            &request.session_id,
        ) {
            return response;
        }
    }
    let workspace = match actor.user() {
        Some(identity) => match state.workspace_access.claim_or_authorize_environment(
            identity,
            &request.workspace_path,
            None,
        ) {
            Ok(path) => path,
            Err(error) => return workspace_access_error(&state, error),
        },
        None => match resolve_within_root(&state.config.workspaces_root, &request.workspace_path) {
            Ok(path) => path,
            Err(error) => return api_error(StatusCode::BAD_REQUEST, &error, &state.config),
        },
    };
    match discussions::move_discussion_for_account(
        request.account_id,
        request.session_id,
        display_path(&workspace),
    ) {
        Ok(value) => json_response(value),
        Err(error) => api_error(resource_error_status(&error), &error, &state.config),
    }
}

async fn api_rename_discussion(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<RenameDiscussionRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Some(identity) = actor.user() {
        if let Err(response) = authorize_discussion_for_identity(
            &state,
            identity,
            &request.account_id,
            &request.session_id,
        ) {
            return response;
        }
    }
    match discussions::rename_discussion_for_account(
        request.account_id,
        request.session_id,
        request.title,
    ) {
        Ok(value) => json_response(value),
        Err(error) => api_error(resource_error_status(&error), &error, &state.config),
    }
}

async fn api_claim_session(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<ClaimSessionRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if actor.user().is_some() {
        let Some(terminal_id) = request.terminal_id else {
            return api_error(
                StatusCode::BAD_REQUEST,
                "Le terminal proprietaire est obligatoire",
                &state.config,
            );
        };
        if let Err(error) = state.terminals.get_for_actor(terminal_id, &actor) {
            return api_error(StatusCode::NOT_FOUND, &error, &state.config);
        }
    }
    match discussions::claim_session_for_account(
        request.account_id,
        request.after_unix,
        request.exclude_session_ids,
        request.match_session_id,
    ) {
        Ok(value) => json_response(value),
        Err(error) => api_error(resource_error_status(&error), &error, &state.config),
    }
}

async fn api_delete_discussion(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<DeleteDiscussionRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Some(identity) = actor.user() {
        if let Err(response) = authorize_discussion_for_identity(
            &state,
            identity,
            &request.account_id,
            &request.session_id,
        ) {
            return response;
        }
    }
    let account_id = request.account_id;
    let session_id = request.session_id;
    let archive = request.archive;
    match tokio::task::spawn_blocking(move || {
        discussions::delete_discussion_for_account(account_id, session_id, archive)
    })
    .await
    {
        Ok(Ok(value)) => json_response(value),
        Ok(Err(error)) => api_error(resource_error_status(&error), &error, &state.config),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Suppression de discussion interrompue: {error}"),
            &state.config,
        ),
    }
}

/// Continuation INTER-PROVIDER (mode web) : renvoie le transcript semantique
/// (chaine JSON) d'une discussion Codex ou Claude, a injecter comme amorce dans
/// une session neuve du provider cible cote client.
async fn api_import_codex_transcript_to_freebuff(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<ImportCodexTranscriptRequest>,
) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }
    match discussions::import_codex_transcript_between(
        request.source_account_id,
        request.session_id,
        request.target_account_id,
        request.folder_path,
        request.transcript,
    ) {
        Ok(value) => json_response(value),
        Err(error) => api_error(resource_error_status(&error), &error, &state.config),
    }
}

async fn api_export_discussion_transcript(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<ExportDiscussionRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Some(identity) = actor.user() {
        if let Err(response) = authorize_discussion_for_identity(
            &state,
            identity,
            &request.account_id,
            &request.session_id,
        ) {
            return response;
        }
    }
    match discussions::export_transcript_for_account(request.account_id, request.session_id) {
        Ok(value) => json_response(value),
        Err(error) => api_error(resource_error_status(&error), &error, &state.config),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportCodexTranscriptRequest {
    source_account_id: String,
    session_id: String,
    target_account_id: String,
    folder_path: Option<String>,
    transcript: String,
}

#[derive(Debug, Deserialize)]
struct CreateForumTopicRequest {
    title: String,
    body: String,
}

#[derive(Debug, Deserialize)]
struct CreateForumReplyRequest {
    body: String,
}

async fn api_list_forum_topics(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }
    match state.forum.list_topics() {
        Ok(topics) => forum_no_store(json_response(topics)),
        Err(error) => forum_api_error(&state, error),
    }
}

async fn api_get_forum_topic(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }
    match state.forum.topic(&id) {
        Ok(topic) => forum_no_store(json_response(topic)),
        Err(error) => forum_api_error(&state, error),
    }
}

async fn api_create_forum_topic(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    request: Result<Json<CreateForumTopicRequest>, JsonRejection>,
) -> Response {
    let author = match forum_author(&state, &headers) {
        Ok(author) => author,
        Err(response) => return response,
    };
    let Json(request) = match request {
        Ok(request) => request,
        Err(error) => return forum_json_rejection(&state, error),
    };
    match state
        .forum
        .create_topic(author, request.title, request.body)
    {
        Ok(topic) => forum_no_store((StatusCode::CREATED, Json(topic)).into_response()),
        Err(error) => forum_api_error(&state, error),
    }
}

async fn api_reply_to_forum_topic(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    request: Result<Json<CreateForumReplyRequest>, JsonRejection>,
) -> Response {
    let author = match forum_author(&state, &headers) {
        Ok(author) => author,
        Err(response) => return response,
    };
    let Json(request) = match request {
        Ok(request) => request,
        Err(error) => return forum_json_rejection(&state, error),
    };
    match state.forum.add_reply(author, &id, request.body) {
        Ok(topic) => forum_no_store((StatusCode::CREATED, Json(topic)).into_response()),
        Err(error) => forum_api_error(&state, error),
    }
}

fn forum_author(state: &Arc<ServerState>, headers: &HeaderMap) -> Result<ForumAuthor, Response> {
    check_admin_header(state, headers)?;
    match state.auth.identity_from_headers(headers) {
        Ok(Some(identity)) => Ok(ForumAuthor::new(
            identity.id,
            identity.username,
            identity.avatar_url,
        )),
        // Les clients natifs peuvent utiliser le jeton administrateur sans
        // cookie de session. Leurs messages restent clairement identifies.
        Ok(None) => Ok(ForumAuthor::administrator()),
        Err(error) => Err(api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &error,
            &state.config,
        )),
    }
}

fn forum_api_error(state: &Arc<ServerState>, error: ForumError) -> Response {
    let status = match &error {
        ForumError::Validation(_) => StatusCode::BAD_REQUEST,
        ForumError::NotFound => StatusCode::NOT_FOUND,
        ForumError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    forum_no_store(api_error(status, &error.to_string(), &state.config))
}

fn forum_json_rejection(state: &Arc<ServerState>, _error: JsonRejection) -> Response {
    forum_no_store(api_error(
        StatusCode::BAD_REQUEST,
        "requete JSON du forum invalide",
        &state.config,
    ))
}

fn forum_no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[derive(Debug, Deserialize)]
struct SendPrivateMessageRequest {
    #[serde(default)]
    body: String,
    #[serde(default)]
    images: Vec<PrivateMessageImageRequest>,
}

async fn api_list_private_message_users(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    let actor = match private_message_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let identities = match state.auth.public_identities() {
        Ok(identities) => identities,
        Err(error) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    };
    let mut users = identities
        .into_iter()
        .map(private_message_user_from_identity)
        .filter(|user| user.id != actor.id)
        .collect::<Vec<_>>();
    if actor.id != "server-admin" {
        users.push(PrivateMessageUser::administrator());
    }
    users.sort_by(|left, right| {
        left.username
            .to_lowercase()
            .cmp(&right.username.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    private_message_no_store(json_response(users))
}

async fn api_list_private_message_conversations(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    let actor = match private_message_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    match state.private_messages.list_conversations(&actor.id) {
        Ok(conversations) => private_message_no_store(json_response(conversations)),
        Err(error) => private_message_api_error(&state, error),
    }
}

async fn api_get_private_message_conversation(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(user_id): AxumPath<String>,
) -> Response {
    let actor = match private_message_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let recipient = match private_message_recipient(&state, &actor, &user_id) {
        Ok(recipient) => recipient,
        Err(response) => return response,
    };
    match state
        .private_messages
        .conversation_with_read_status(&actor, &recipient)
    {
        Ok((conversation, marked_read)) => {
            if marked_read {
                notify_private_message_participants(&state, &actor, &recipient);
            }
            private_message_no_store(json_response(conversation))
        }
        Err(error) => private_message_api_error(&state, error),
    }
}

async fn api_send_private_message(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(user_id): AxumPath<String>,
    Json(request): Json<SendPrivateMessageRequest>,
) -> Response {
    let actor = match private_message_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let recipient = match private_message_recipient(&state, &actor, &user_id) {
        Ok(recipient) => recipient,
        Err(response) => return response,
    };
    match state.private_messages.send_with_images(
        actor.clone(),
        recipient.clone(),
        request.body,
        request.images,
    ) {
        Ok(message) => {
            notify_private_message_participants(&state, &actor, &recipient);
            private_message_no_store((StatusCode::CREATED, Json(message)).into_response())
        }
        Err(error) => private_message_api_error(&state, error),
    }
}

async fn api_get_private_message_image(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(image_id): AxumPath<String>,
) -> Response {
    let actor = match private_message_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    match state.private_messages.image_content(&actor.id, &image_id) {
        Ok(image) => private_message_no_store(json_response(image)),
        Err(error) => private_message_api_error(&state, error),
    }
}

async fn api_list_private_message_campaigns(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    let actor = match private_message_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    match state.private_messages.list_campaigns(&actor.id) {
        Ok(campaigns) => private_message_no_store(json_response(campaigns)),
        Err(error) => private_message_api_error(&state, error),
    }
}

async fn api_create_private_message_campaign(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<CreatePrivateMessageCampaignRequest>,
) -> Response {
    let actor = match private_message_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let mut recipients = Vec::with_capacity(request.recipient_ids.len());
    for user_id in &request.recipient_ids {
        match private_message_recipient(&state, &actor, user_id) {
            Ok(recipient) => recipients.push(recipient),
            Err(response) => return response,
        }
    }
    match state
        .private_messages
        .create_campaign(actor.clone(), recipients, request, "human")
    {
        Ok(campaign) => {
            state
                .chat
                .runtime_sync()
                .notify_private_messages([actor.id]);
            private_message_no_store((StatusCode::CREATED, Json(campaign)).into_response())
        }
        Err(error) => private_message_api_error(&state, error),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ControlPrivateMessageCampaignRequest {
    action: PrivateMessageCampaignAction,
    #[serde(default)]
    consent_confirmed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ControlPrivateMessageCampaignToolArguments {
    campaign_id: String,
    action: PrivateMessageCampaignAction,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GetControlDeviceActionToolArguments {
    action_id: String,
}

async fn api_control_private_message_campaign(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(campaign_id): AxumPath<String>,
    Json(request): Json<ControlPrivateMessageCampaignRequest>,
) -> Response {
    let actor = match private_message_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    match state.private_messages.control_campaign(
        &actor.id,
        &campaign_id,
        request.action,
        request.consent_confirmed,
    ) {
        Ok(campaign) => {
            state
                .chat
                .runtime_sync()
                .notify_private_messages([actor.id]);
            private_message_no_store(json_response(campaign))
        }
        Err(error) => private_message_api_error(&state, error),
    }
}

async fn api_list_control_devices(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = device_action_context_from_headers(&state, &headers) {
        return private_message_no_store(response);
    }
    match state.device_fleet.snapshot() {
        Ok(snapshot) => private_message_no_store(json_response(snapshot)),
        Err(error) => device_fleet_api_error(&state, error),
    }
}

async fn api_control_device(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<DeviceActionRequest>,
) -> Response {
    let context = match device_action_context_from_headers(&state, &headers) {
        Ok(context) => context,
        Err(response) => return private_message_no_store(response),
    };
    let queued = match context.queue_action(&state.device_fleet, request) {
        Ok(action) => action,
        Err(error) => return device_fleet_api_error(&state, error),
    };
    match context
        .wait_action(&state.device_fleet, &queued.id, Duration::from_secs(35))
        .await
    {
        Ok(action) => device_action_http_response(&state, action),
        Err(error) => device_fleet_api_error(&state, error),
    }
}

async fn api_device_action_status(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(action_id): AxumPath<String>,
) -> Response {
    let context = match device_action_context_from_headers(&state, &headers) {
        Ok(context) => context,
        Err(response) => return private_message_no_store(response),
    };
    match context.action_status(&state.device_fleet, &action_id) {
        Ok(action) => device_action_http_response(&state, action),
        Err(error) => device_fleet_api_error(&state, error),
    }
}

async fn api_device_connector_heartbeat(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<DeviceConnectorHeartbeatRequest>,
) -> Response {
    if let Err(response) = check_maintenance_header(&state, &headers) {
        return private_message_no_store(response);
    }
    match state.device_fleet.heartbeat(request) {
        Ok(snapshot) => private_message_no_store(json_response(snapshot)),
        Err(error) => device_fleet_api_error(&state, error),
    }
}

async fn api_device_connector_claim(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<DeviceConnectorClaimRequest>,
) -> Response {
    if let Err(response) = check_maintenance_header(&state, &headers) {
        return private_message_no_store(response);
    }
    match state.device_fleet.claim_next(&request.connector_id) {
        Ok(job) => private_message_no_store(json_response(DeviceConnectorClaimResponse { job })),
        Err(error) => device_fleet_api_error(&state, error),
    }
}

async fn api_device_connector_report(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<DeviceConnectorReportRequest>,
) -> Response {
    if let Err(response) = check_maintenance_header(&state, &headers) {
        return private_message_no_store(response);
    }
    match state.device_fleet.report(request) {
        Ok(action) => private_message_no_store(json_response(action)),
        Err(error) => device_fleet_api_error(&state, error),
    }
}

fn device_action_http_response(state: &Arc<ServerState>, action: DeviceActionRecord) -> Response {
    match action.status {
        DeviceActionStatus::Succeeded => {
            private_message_no_store(json_response(action.result.unwrap_or_else(|| {
                DeviceActionResult {
                    action_id: action.id,
                    device_id: action.device_id,
                    action: action.action,
                    success: true,
                    detail: "Action appareil terminee".to_string(),
                    stdout: None,
                    stderr: None,
                    data_base64: None,
                    mime_type: None,
                    started_at: action.created_at,
                    finished_at: action.finished_at.unwrap_or(action.updated_at),
                    truncated: false,
                }
            })))
        }
        DeviceActionStatus::Failed => {
            let detail = action
                .result
                .as_ref()
                .map(|result| result.detail.as_str())
                .unwrap_or("Action appareil echouee");
            private_message_no_store(api_error(
                StatusCode::BAD_GATEWAY,
                detail,
                &state.config,
            ))
        }
        DeviceActionStatus::Expired => private_message_no_store(api_error(
            StatusCode::GATEWAY_TIMEOUT,
            "Le resultat de l'action appareil est ambigu ou a expire ; elle n'a pas ete relancee",
            &state.config,
        )),
        DeviceActionStatus::Queued | DeviceActionStatus::Claimed => {
            private_message_no_store((StatusCode::ACCEPTED, Json(action)).into_response())
        }
    }
}

fn device_fleet_api_error(state: &Arc<ServerState>, error: DeviceFleetError) -> Response {
    let status = match &error {
        DeviceFleetError::Validation(_) => StatusCode::BAD_REQUEST,
        DeviceFleetError::NotFound => StatusCode::NOT_FOUND,
        DeviceFleetError::Conflict(_) => StatusCode::CONFLICT,
        DeviceFleetError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
        DeviceFleetError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    private_message_no_store(api_error(status, &error.to_string(), &state.config))
}

async fn api_list_tiktok_dm_campaigns(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let campaigns = match state.tiktok_messaging.list(actor.owner_id()) {
        Ok(campaigns) => campaigns,
        Err(error) => return tiktok_dm_api_error(&state, error),
    };
    let connector = match state.tiktok_messaging.connector_status() {
        Ok(connector) => connector,
        Err(error) => return tiktok_dm_api_error(&state, error),
    };
    let connector_online = connector
        .as_ref()
        .is_some_and(|status| status.is_online(metrics::now_ts()));
    private_message_no_store(json_response(json!({
        "campaigns": campaigns,
        "connector": connector,
        "connectorOnline": connector_online
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SelectTikTokSenderAccountRequest {
    username: String,
}

async fn api_list_tiktok_sender_accounts(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let accounts = match state.tiktok_messaging.sender_accounts(actor.owner_id()) {
        Ok(accounts) => accounts,
        Err(error) => return tiktok_dm_api_error(&state, error),
    };
    let connector = match state.tiktok_messaging.connector_status() {
        Ok(connector) => connector,
        Err(error) => return tiktok_dm_api_error(&state, error),
    };
    let connector_online = connector
        .as_ref()
        .is_some_and(|status| status.is_online(metrics::now_ts()) && status.agent_healthy);
    let bridge_online = connector
        .as_ref()
        .is_some_and(|status| status.is_online(metrics::now_ts()));
    let devices = connector
        .as_ref()
        .map(|status| status.device_serials.clone())
        .unwrap_or_default();
    let device_details = connector
        .as_ref()
        .map(|status| status.devices.clone())
        .unwrap_or_default();
    let scrcpy_available = connector
        .as_ref()
        .is_some_and(|status| status.scrcpy_available);
    let ws_scrcpy_available = connector
        .as_ref()
        .is_some_and(|status| status.ws_scrcpy_available);
    let ws_scrcpy_online = bridge_online
        && connector
            .as_ref()
            .is_some_and(|status| status.ws_scrcpy_online);
    let adb_error = connector
        .as_ref()
        .and_then(|status| status.adb_error.clone());
    let connector_error = connector.as_ref().and_then(|status| status.error.clone());
    let setup_required = accounts.is_empty();
    private_message_no_store(json_response(json!({
        "accounts": accounts,
        "devices": devices,
        "deviceDetails": device_details,
        "bridgeOnline": bridge_online,
        "connectorOnline": connector_online,
        "scrcpyAvailable": scrcpy_available,
        "wsScrcpyAvailable": ws_scrcpy_available,
        "wsScrcpyOnline": ws_scrcpy_online,
        "adbError": adb_error,
        "connectorError": connector_error,
        "setupRequired": setup_required
    })))
}

async fn api_select_tiktok_sender_account(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<SelectTikTokSenderAccountRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    match state
        .tiktok_messaging
        .select_sender_account(actor.owner_id(), &request.username)
    {
        Ok(account) => private_message_no_store(json_response(account)),
        Err(error) => tiktok_dm_api_error(&state, error),
    }
}

async fn api_queue_tiktok_sender_setup(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<QueueTikTokSenderSetupRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    match state
        .tiktok_messaging
        .queue_sender_setup(actor.owner_id(), request)
    {
        Ok(action) => {
            private_message_no_store((StatusCode::ACCEPTED, Json(action)).into_response())
        }
        Err(error) => tiktok_dm_api_error(&state, error),
    }
}

async fn api_prepare_tiktok_dm_campaign(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<PrepareTikTokDmCampaignRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    match state
        .tiktok_messaging
        .prepare(actor.owner_id(), request, "human_chat")
    {
        Ok(campaign) => {
            private_message_no_store((StatusCode::CREATED, Json(campaign)).into_response())
        }
        Err(error) => tiktok_dm_api_error(&state, error),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfirmTikTokDmHttpRequest {
    #[serde(default)]
    owned_accounts_confirmed: bool,
    #[serde(default)]
    send_confirmed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendTikTokDmDirectToolArguments {
    recipient: String,
    message: String,
    #[serde(default)]
    min_interval_minutes: Option<u8>,
    #[serde(default)]
    max_interval_minutes: Option<u8>,
    #[serde(default)]
    device_serial: Option<String>,
    #[serde(default)]
    sender_account: Option<String>,
    #[serde(default)]
    idempotency_key: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SendTikTokDmToolArguments {
    Direct(SendTikTokDmDirectToolArguments),
    Prepared(ConfirmTikTokDmCampaignRequest),
}

impl SendTikTokDmDirectToolArguments {
    fn into_prepare_request(self, capability_token: &str) -> PrepareTikTokDmCampaignRequest {
        let idempotency_key = self.idempotency_key.unwrap_or_else(|| {
            let mut digest = Sha256::new();
            digest.update(capability_token.as_bytes());
            digest.update([0]);
            digest.update(self.recipient.as_bytes());
            digest.update([0]);
            digest.update(self.message.as_bytes());
            digest.update([0]);
            digest.update([self.min_interval_minutes.unwrap_or(1)]);
            digest.update([self.max_interval_minutes.unwrap_or(2)]);
            if let Some(serial) = self.device_serial.as_deref() {
                digest.update([0]);
                digest.update(serial.as_bytes());
            }
            if let Some(sender) = self.sender_account.as_deref() {
                digest.update([0]);
                digest.update(sender.as_bytes());
            }
            format!("chat-direct:{:x}", digest.finalize())
        });
        PrepareTikTokDmCampaignRequest {
            recipients: vec![self.recipient],
            message: self.message,
            min_interval_minutes: self.min_interval_minutes.unwrap_or(1),
            max_interval_minutes: self.max_interval_minutes.unwrap_or(2),
            device_serial: self.device_serial,
            sender_account: self.sender_account,
            idempotency_key,
        }
    }
}

async fn api_confirm_tiktok_dm_campaign(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(campaign_id): AxumPath<String>,
    Json(request): Json<ConfirmTikTokDmHttpRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    match state.tiktok_messaging.confirm(
        actor.owner_id(),
        ConfirmTikTokDmCampaignRequest {
            campaign_id,
            owned_accounts_confirmed: request.owned_accounts_confirmed,
            send_confirmed: request.send_confirmed,
        },
    ) {
        Ok(campaign) => private_message_no_store(json_response(campaign)),
        Err(error) => tiktok_dm_api_error(&state, error),
    }
}

async fn api_list_tiktok_follower_extractions(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    match state
        .tiktok_messaging
        .list_follower_extractions(actor.owner_id())
    {
        Ok(extractions) => private_message_no_store(json_response(json!({
            "extractions": extractions
        }))),
        Err(error) => tiktok_dm_api_error(&state, error),
    }
}

async fn api_queue_tiktok_follower_extraction(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<QueueTikTokFollowerExtractionRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    match state
        .tiktok_messaging
        .queue_follower_extraction(actor.owner_id(), request, "human_chat")
    {
        Ok(extraction) => {
            private_message_no_store((StatusCode::CREATED, Json(extraction)).into_response())
        }
        Err(error) => tiktok_dm_api_error(&state, error),
    }
}

async fn api_tiktok_connector_heartbeat(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<TikTokConnectorHeartbeatRequest>,
) -> Response {
    if let Err(response) = check_maintenance_header(&state, &headers) {
        return response;
    }
    match state.tiktok_messaging.heartbeat(request) {
        Ok(status) => private_message_no_store(json_response(status)),
        Err(error) => tiktok_dm_api_error(&state, error),
    }
}

async fn api_tiktok_connector_claim(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<TikTokConnectorClaimRequest>,
) -> Response {
    if let Err(response) = check_maintenance_header(&state, &headers) {
        return response;
    }
    match state.tiktok_messaging.claim_next(&request.connector_id) {
        Ok(job) => private_message_no_store(json_response(json!({ "job": job }))),
        Err(error) => tiktok_dm_api_error(&state, error),
    }
}

async fn api_tiktok_connector_report(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<TikTokConnectorReportRequest>,
) -> Response {
    if let Err(response) = check_maintenance_header(&state, &headers) {
        return response;
    }
    match state.tiktok_messaging.report(request) {
        Ok(campaign) => private_message_no_store(json_response(campaign)),
        Err(error) => tiktok_dm_api_error(&state, error),
    }
}

async fn api_tiktok_sender_setup_claim(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<TikTokConnectorClaimRequest>,
) -> Response {
    if let Err(response) = check_maintenance_header(&state, &headers) {
        return response;
    }
    match state
        .tiktok_messaging
        .claim_next_sender_setup(&request.connector_id)
    {
        Ok(job) => private_message_no_store(json_response(json!({ "job": job }))),
        Err(error) => tiktok_dm_api_error(&state, error),
    }
}

async fn api_tiktok_sender_setup_report(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<TikTokSenderSetupReportRequest>,
) -> Response {
    if let Err(response) = check_maintenance_header(&state, &headers) {
        return response;
    }
    match state.tiktok_messaging.report_sender_setup(request) {
        Ok(action) => private_message_no_store(json_response(action)),
        Err(error) => tiktok_dm_api_error(&state, error),
    }
}

async fn api_tiktok_follower_connector_claim(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<TikTokConnectorClaimRequest>,
) -> Response {
    if let Err(response) = check_maintenance_header(&state, &headers) {
        return response;
    }
    match state
        .tiktok_messaging
        .claim_next_follower_extraction(&request.connector_id)
    {
        Ok(job) => private_message_no_store(json_response(json!({ "job": job }))),
        Err(error) => tiktok_dm_api_error(&state, error),
    }
}

async fn api_tiktok_follower_connector_report(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<TikTokFollowerSubmissionReportRequest>,
) -> Response {
    if let Err(response) = check_maintenance_header(&state, &headers) {
        return response;
    }
    match state.tiktok_messaging.report_follower_submission(request) {
        Ok(extraction) => private_message_no_store(json_response(extraction)),
        Err(error) => tiktok_dm_api_error(&state, error),
    }
}

async fn api_tiktok_follower_connector_pending_results(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<TikTokConnectorClaimRequest>,
) -> Response {
    if let Err(response) = check_maintenance_header(&state, &headers) {
        return response;
    }
    match state
        .tiktok_messaging
        .pending_follower_results(&request.connector_id)
    {
        Ok(extractions) => private_message_no_store(json_response(json!({
            "extractions": extractions
        }))),
        Err(error) => tiktok_dm_api_error(&state, error),
    }
}

async fn api_tiktok_follower_connector_report_result(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<TikTokFollowerResultReportRequest>,
) -> Response {
    if let Err(response) = check_maintenance_header(&state, &headers) {
        return response;
    }
    match state.tiktok_messaging.report_follower_result(request) {
        Ok(extraction) => private_message_no_store(json_response(extraction)),
        Err(error) => tiktok_dm_api_error(&state, error),
    }
}

fn tiktok_dm_api_error(state: &Arc<ServerState>, error: TikTokDmError) -> Response {
    let status = match &error {
        TikTokDmError::Validation(_) => StatusCode::BAD_REQUEST,
        TikTokDmError::NotFound => StatusCode::NOT_FOUND,
        TikTokDmError::Conflict(_) => StatusCode::CONFLICT,
        TikTokDmError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    api_error(status, &error.to_string(), &state.config)
}

fn notify_private_message_participants(
    state: &Arc<ServerState>,
    left: &PrivateMessageUser,
    right: &PrivateMessageUser,
) {
    state
        .chat
        .runtime_sync()
        .notify_private_messages([left.id.clone(), right.id.clone()]);
}

fn private_message_user_from_identity(identity: AuthIdentity) -> PrivateMessageUser {
    PrivateMessageUser::new(identity.id, identity.username, identity.avatar_url)
}

fn private_message_actor(
    state: &Arc<ServerState>,
    headers: &HeaderMap,
) -> Result<PrivateMessageUser, Response> {
    check_admin_header(state, headers)?;
    match state.auth.identity_from_headers(headers) {
        Ok(Some(identity)) => Ok(private_message_user_from_identity(identity)),
        Ok(None) => Ok(PrivateMessageUser::administrator()),
        Err(error) => Err(api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &error,
            &state.config,
        )),
    }
}

fn private_message_actor_for_tool(
    state: &Arc<ServerState>,
    context: &AutonomousAgentToolContext,
) -> Result<PrivateMessageUser, String> {
    let Some(user_id) = context.user_id.as_deref() else {
        return Ok(PrivateMessageUser::administrator());
    };
    state
        .auth
        .public_identity_by_id(user_id)?
        .map(private_message_user_from_identity)
        .ok_or_else(|| "Le proprietaire de cet agent n'existe plus".to_string())
}

fn private_message_recipient_for_tool(
    state: &Arc<ServerState>,
    actor: &PrivateMessageUser,
    user_id: &str,
) -> Result<PrivateMessageUser, String> {
    let user_id = user_id.trim();
    if user_id.is_empty() || user_id == actor.id {
        return Err("Destinataire interne invalide".to_string());
    }
    if user_id == "server-admin" {
        return Ok(PrivateMessageUser::administrator());
    }
    if let Some(identity) = state.auth.public_identity_by_id(user_id)? {
        return Ok(private_message_user_from_identity(identity));
    }
    state
        .private_messages
        .known_conversation_participant(&actor.id, user_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Utilisateur interne introuvable".to_string())
}

fn private_message_recipient(
    state: &Arc<ServerState>,
    actor: &PrivateMessageUser,
    user_id: &str,
) -> Result<PrivateMessageUser, Response> {
    let user_id = user_id.trim();
    if user_id.is_empty() || user_id == actor.id {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Destinataire invalide",
            &state.config,
        ));
    }
    if user_id == "server-admin" {
        return Ok(PrivateMessageUser::administrator());
    }
    match state.auth.public_identity_by_id(user_id) {
        Ok(Some(identity)) => return Ok(private_message_user_from_identity(identity)),
        Ok(None) => {}
        Err(error) => {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &error,
                &state.config,
            ))
        }
    }
    match state
        .private_messages
        .known_conversation_participant(&actor.id, user_id)
    {
        Ok(Some(user)) => Ok(user),
        Ok(None) => Err(api_error(
            StatusCode::NOT_FOUND,
            "Utilisateur introuvable",
            &state.config,
        )),
        Err(error) => Err(private_message_api_error(state, error)),
    }
}

fn private_message_api_error(state: &Arc<ServerState>, error: PrivateMessageError) -> Response {
    let status = match &error {
        PrivateMessageError::Validation(_) => StatusCode::BAD_REQUEST,
        PrivateMessageError::NotFound => StatusCode::NOT_FOUND,
        PrivateMessageError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    api_error(status, &error.to_string(), &state.config)
}

fn private_message_no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn api_pool_status(State(state): State<Arc<ServerState>>, headers: HeaderMap) -> Response {
    auth_or(&state, &headers, || {
        let settings = settings::load_settings_for_terminal()?;
        let manager = PoolManager::build(&settings)?;
        Ok(pool_status_response(
            &state.config,
            &settings,
            &manager,
            true,
        ))
    })
}

async fn api_pool_stop(State(state): State<Arc<ServerState>>, headers: HeaderMap) -> Response {
    auth_or(&state, &headers, || {
        Ok(json_response(json!({ "running": false })))
    })
}

async fn api_start_terminal(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(mut request): Json<StartTerminalRequest>,
) -> Response {
    // Auth d'abord (401 pour un appelant sans token), puis refus explicite en
    // 503 si le noeud est en drain. On NE passe PAS par auth_or ici : auth_or
    // mappe toute Err en 500, or on veut un 503 distinguable que le client
    // interprete comme "essaie un autre noeud".
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    request.source_terminal_key = match normalize_source_terminal_key(request.source_terminal_key) {
        Ok(value) => value,
        Err(error) => return api_error(StatusCode::BAD_REQUEST, &error, &state.config),
    };
    if is_draining(&state) {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "noeud en drain: nouveaux terminaux refuses",
            &state.config,
        );
    }
    let authorized_workspace = if request.login_only {
        None
    } else if let Some(raw) = request
        .workspace_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        match actor.user() {
            Some(identity) => match state
                .workspace_access
                .claim_or_authorize_environment(identity, raw, None)
            {
                Ok(path) => Some(path),
                Err(error) => return workspace_access_error(&state, error),
            },
            None => match resolve_within_root(&state.config.workspaces_root, raw) {
                Ok(path) => Some(path),
                Err(error) => return api_error(StatusCode::BAD_REQUEST, &error, &state.config),
            },
        }
    } else {
        None
    };
    // Une authentification de compte utilise exclusivement le home partage du
    // compte. Elle ne doit ni creer ni valider un espace personnel utilisateur.
    let generated_workspaces_root = if request.login_only {
        state.config.data_dir.join("workspaces")
    } else {
        match actor.user() {
            Some(identity) => match state.workspace_access.personal_root(identity) {
                Ok(path) => path,
                Err(error) => return workspace_access_error(&state, error),
            },
            None => state.config.data_dir.join("workspaces"),
        }
    };
    let owner_id = actor.owner_id().to_string();
    let identity = actor.user().cloned();
    let start_state = state.clone();
    let spawn = tokio::task::spawn_blocking(move || {
        let login_only = request.login_only;
        let value = start_state.terminals.start(
            &start_state.config,
            request,
            owner_id,
            authorized_workspace,
            generated_workspaces_root,
        )?;
        if !login_only {
            if let Some(identity) = identity.as_ref() {
                if let Err(error) = start_state.workspace_access.claim_or_authorize_environment(
                    identity,
                    &value.workspace_path,
                    None,
                ) {
                    // Le PTY est deja lance a ce stade. Une autorisation qui
                    // echoue ne doit pas retourner une erreur en laissant le
                    // shell et Freebuff vivants dans le noeud.
                    let _ = start_state.terminals.stop(value.id);
                    return Err(error.message);
                }
            }
        }
        Ok::<_, String>(value)
    });
    // Un noeud sature (CPU, threads bloquants, tests lancés à cote) ne doit
    // jamais laisser le client dans un « Preparation » indefini : au-dela de
    // 90 secondes, on repond 504 avec un message clair. La tache de fond
    // s'acheve seule ; un nouvel essai client verra alors « deja vivant ».
    match tokio::time::timeout(Duration::from_secs(90), spawn).await {
        Ok(Ok(Ok(value))) => json_response(value),
        Ok(Ok(Err(error))) => api_error(agent_start_status(&error), &error, &state.config),
        Ok(Err(error)) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("spawn terminal interrompu: {error}"),
            &state.config,
        ),
        Err(_elapsed) => api_error(
            StatusCode::GATEWAY_TIMEOUT,
            "demarrage du terminal trop long (90 s) : serveur surcharge, reessayez",
            &state.config,
        ),
    }
}

fn normalize_source_terminal_key(value: Option<String>) -> Result<Option<String>, String> {
    let value = value
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty());
    let Some(value) = value else {
        return Ok(None);
    };
    if value.chars().count() > 160 || value.chars().any(char::is_control) {
        return Err("Identifiant du terminal source invalide".to_string());
    }
    Ok(Some(value))
}

fn normalize_source_chat_key(value: Option<String>) -> Result<Option<String>, String> {
    let value = value
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty());
    let Some(value) = value else {
        return Ok(None);
    };
    if value.chars().count() > 160 || value.chars().any(char::is_control) {
        return Err("Identifiant du chat source invalide".to_string());
    }
    Ok(Some(value))
}

async fn api_start_chat_turn(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(mut request): Json<StartChatTurnRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if is_draining(&state) {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "noeud en drain: nouveaux messages refusés",
            &state.config,
        );
    }
    if let Some(raw) = request
        .project_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let resolved = match actor.user() {
            Some(identity) => match state
                .workspace_access
                .claim_or_authorize_environment(identity, raw, None)
            {
                Ok(path) => path,
                Err(error) => return workspace_access_error(&state, error),
            },
            None => match resolve_within_root(&state.config.workspaces_root, raw) {
                Ok(path) => path,
                Err(error) => return api_error(StatusCode::BAD_REQUEST, &error, &state.config),
            },
        };
        request.project_dir = Some(display_path(&resolved));
    } else if actor.user().is_some() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "Un environnement personnel ou partage est obligatoire",
            &state.config,
        );
    }
    if let (Some(identity), Some(session_id)) = (actor.user(), request.session_id.as_deref()) {
        if let Err(response) =
            authorize_discussion_for_identity(&state, identity, &request.account_id, session_id)
        {
            return response;
        }
    }
    request.source_chat_key = match normalize_source_chat_key(request.source_chat_key.take()) {
        Ok(value) => value,
        Err(error) => return api_error(StatusCode::BAD_REQUEST, &error, &state.config),
    };
    // L'identite nominative est calculee AVANT l'emission de la capacite : elle
    // est la seule cle qui permette a un outil de retrouver la boite Microsoft
    // du demandeur. `actor.owner_id()` ne conviendrait pas, il renvoie le
    // pseudo-compte "server-admin" pour un tour lance au jeton administrateur.
    let owner_id = actor.user().map(|identity| identity.id.clone());
    let token = match state
        .chat_tool_capabilities
        .issue(AutonomousAgentToolContext {
            account_id: request.account_id.clone(),
            scope: ChatToolScope::Full,
            user_id: owner_id.clone(),
            source_chat_key: request.source_chat_key.clone(),
            project_dir: request.project_dir.clone(),
            mode: request.mode,
            model: request.model.clone(),
            reasoning_effort: request.reasoning_effort.clone(),
            goal_key: None,
        }) {
        Ok(value) => value,
        Err(error) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    };
    let tool_server = match state.config.chat_tools_mcp_url() {
        Ok(url) => ChatModelToolServerConfig {
            url,
            bearer_token: token.clone(),
        },
        Err(error) => {
            state.chat_tool_capabilities.revoke(&token);
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config);
        }
    };
    let start_state = state.clone();
    let result = tokio::task::spawn_blocking(move || {
        let value = start_state
            .chat
            .start_with_model_tools(request, Some(tool_server))?;
        if let Some(owner_id) = owner_id.as_deref() {
            if let Err(error) = start_state.chat.assign_owner(value.id, owner_id) {
                let _ = start_state.chat.stop(value.id);
                return Err(error);
            }
        }
        Ok::<_, String>(value)
    })
    .await;
    match result {
        Ok(Ok(value)) => json_response(value),
        Ok(Err(error)) => {
            state.chat_tool_capabilities.revoke(&token);
            api_error(agent_start_status(&error), &error, &state.config)
        }
        Err(error) => {
            state.chat_tool_capabilities.revoke(&token);
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("spawn chat interrompu: {error}"),
                &state.config,
            )
        }
    }
}

fn mcp_origin_is_allowed(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get("origin").and_then(|value| value.to_str().ok()) else {
        return true;
    };
    let normalized = origin.trim().to_ascii_lowercase();
    normalized.starts_with("http://127.0.0.1:")
        || normalized.starts_with("http://localhost:")
        || normalized.starts_with("http://[::1]:")
}

fn bearer_from_headers(headers: &HeaderMap) -> &str {
    let value = headers
        .get("authorization")
        .and_then(|header| header.to_str().ok())
        .unwrap_or("");
    value.strip_prefix("Bearer ").unwrap_or(value).trim()
}

fn device_tool_result_response(id: Value, result: DeviceActionResult) -> Value {
    let text = format!(
        "Action {:?} terminee sur {} : {}",
        result.action, result.device_id, result.detail
    );
    let mut structured = serde_json::to_value(&result).unwrap_or_else(|_| {
        json!({
            "actionId": &result.action_id,
            "deviceId": &result.device_id,
            "success": result.success,
            "detail": &result.detail,
        })
    });
    let mut content = vec![json!({ "type": "text", "text": text })];
    if let (Some(data), Some(mime_type)) =
        (result.data_base64.as_deref(), result.mime_type.as_deref())
    {
        content.push(json!({
            "type": "image",
            "data": data,
            "mimeType": mime_type,
        }));
        if let Some(fields) = structured.as_object_mut() {
            fields.remove("dataBase64");
            fields.insert("hasImage".to_string(), Value::Bool(true));
        }
    }
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": content,
            "structuredContent": structured,
            "isError": false,
        }
    })
}

fn device_tool_action_record_response(id: Value, action: DeviceActionRecord) -> Value {
    match action.status {
        DeviceActionStatus::Succeeded => match action.result {
            Some(result) => device_tool_result_response(id, result),
            None => chat_model_tools::tool_error_response(
                id,
                "Le connecteur a termine l'action sans resultat exploitable",
            ),
        },
        DeviceActionStatus::Failed => {
            let detail = action
                .result
                .as_ref()
                .map(|result| result.detail.as_str())
                .unwrap_or("L'action appareil a echoue");
            chat_model_tools::tool_error_response(id, detail)
        }
        DeviceActionStatus::Expired => chat_model_tools::tool_error_response(
            id,
            "Le resultat de l'action est ambigu ou a expire. Par securite, une action mutante n'est jamais relancee automatiquement.",
        ),
        DeviceActionStatus::Queued | DeviceActionStatus::Claimed => {
            chat_model_tools::tool_microsoft_data_response(
                id,
                "L'action est encore en cours. Suis cet actionId avec get_control_device_action et ne la relance pas, afin d'eviter une double execution.",
                json!({ "action": action }),
            )
        }
    }
}

fn mcp_device_action_idempotency_key(
    token: &str,
    call_id: &Value,
    request: &DeviceActionRequest,
) -> Result<String, serde_json::Error> {
    let mut canonical_request = request.clone();
    canonical_request.idempotency_key = None;
    let call_id = serde_json::to_vec(call_id)?;
    let request = serde_json::to_vec(&canonical_request)?;
    let mut digest = Sha256::new();
    digest.update(b"cst-chat-device-action-v2\0");
    for component in [token.as_bytes(), call_id.as_slice(), request.as_slice()] {
        digest.update((component.len() as u64).to_be_bytes());
        digest.update(component);
    }
    Ok(format!("mcp:{:x}", digest.finalize()))
}

async fn wait_for_tiktok_submission(
    state: &Arc<ServerState>,
    owner_id: &str,
    mut campaign: TikTokDmCampaign,
) -> Result<TikTokDmCampaign, TikTokDmError> {
    for _ in 0..30 {
        if matches!(
            campaign.status,
            TikTokDmCampaignStatus::Submitted
                | TikTokDmCampaignStatus::Failed
                | TikTokDmCampaignStatus::Cancelled
        ) {
            return Ok(campaign);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        if let Some(current) = state
            .tiktok_messaging
            .list(owner_id)?
            .into_iter()
            .find(|current| current.id == campaign.id)
        {
            campaign = current;
        }
    }
    Ok(campaign)
}

async fn mcp_chat_tools(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> Response {
    if !mcp_origin_is_allowed(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let token = bearer_from_headers(&headers);
    let capability = match state.chat_tool_capabilities.authorize(token) {
        Ok(value) => value,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let id = payload
        .get("id")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let method = payload
        .get("method")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if payload.get("id").is_none() {
        return match method {
            "notifications/initialized" | "notifications/cancelled" => {
                StatusCode::ACCEPTED.into_response()
            }
            _ => StatusCode::ACCEPTED.into_response(),
        };
    }

    match method {
        "initialize" => {
            let requested_version = payload
                .pointer("/params/protocolVersion")
                .and_then(serde_json::Value::as_str);
            json_response(chat_model_tools::initialize_response(
                id,
                requested_version,
                capability.scope,
            ))
        }
        "ping" => json_response(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {}
        })),
        "tools/list" => json_response(chat_model_tools::tools_list_response(id, capability.scope)),
        "tools/call" => {
            let name = payload
                .pointer("/params/name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if name != AUTONOMOUS_AGENT_TOOL_NAME
                && name != CREATE_GOAL_TOOL_NAME
                && name != GET_GOAL_TOOL_NAME
                && name != UPDATE_GOAL_TOOL_NAME
                && name != UPDATE_AUTONOMOUS_AGENT_TOOL_NAME
                && name != PAUSE_AUTONOMOUS_AGENT_TOOL_NAME
                && name != ACTIVATE_SUPERVISOR_GENERAL_REPORT_TOOL_NAME
                && name != APPLY_AUTONOMOUS_AGENT_POLICY_TOOL_NAME
                && name != CREATE_CHAT_TOOL_NAME
                && name != SEND_CHAT_MESSAGE_TOOL_NAME
                && name != LIST_CONTROL_DEVICES_TOOL_NAME
                && name != CONTROL_DEVICE_TOOL_NAME
                && name != GET_CONTROL_DEVICE_ACTION_TOOL_NAME
                && name != LIST_OUTLOOK_MESSAGES_TOOL_NAME
                && name != LIST_CALENDAR_EVENTS_TOOL_NAME
                && name != SEND_OUTLOOK_EMAIL_TOOL_NAME
                && name != CREATE_CALENDAR_EVENT_TOOL_NAME
                && name != UPDATE_CALENDAR_EVENT_TOOL_NAME
                && name != LIST_PRIVATE_MESSAGE_USERS_TOOL_NAME
                && name != LIST_PRIVATE_MESSAGE_CAMPAIGNS_TOOL_NAME
                && name != CREATE_PRIVATE_MESSAGE_CAMPAIGN_TOOL_NAME
                && name != CONTROL_PRIVATE_MESSAGE_CAMPAIGN_TOOL_NAME
                && name != LIST_TIKTOK_DM_CAMPAIGNS_TOOL_NAME
                && name != LIST_TIKTOK_SENDER_ACCOUNTS_TOOL_NAME
                && name != MANAGE_TIKTOK_SENDER_LOGIN_TOOL_NAME
                && name != SELECT_TIKTOK_SENDER_ACCOUNT_TOOL_NAME
                && name != PREPARE_TIKTOK_DM_CAMPAIGN_TOOL_NAME
                && name != SEND_TIKTOK_DM_CAMPAIGN_TOOL_NAME
                && name != LIST_TIKTOK_FOLLOWER_EXTRACTIONS_TOOL_NAME
                && name != QUEUE_TIKTOK_FOLLOWER_EXTRACTION_TOOL_NAME
            {
                return json_response(chat_model_tools::protocol_error(
                    id,
                    -32602,
                    "Outil MCP inconnu",
                ));
            }
            // Meme reponse qu'un outil inexistant : un agent autonome ne doit
            // pas pouvoir deduire de la reponse qu'il existe des outils qu'on
            // lui refuse, ni les sonder un par un.
            if !capability.scope.allows(name) {
                return json_response(chat_model_tools::protocol_error(
                    id,
                    -32602,
                    "Outil MCP inconnu",
                ));
            }
            let context_result = match name {
                CREATE_CHAT_TOOL_NAME => state.chat_tool_capabilities.claim_chat_creation(token),
                // Les brouillons qui sortent de l'application consomment le
                // sous-quota d'actions externes en plus du budget global.
                SEND_OUTLOOK_EMAIL_TOOL_NAME
                | CREATE_CALENDAR_EVENT_TOOL_NAME
                | UPDATE_CALENDAR_EVENT_TOOL_NAME
                | CREATE_PRIVATE_MESSAGE_CAMPAIGN_TOOL_NAME
                | CONTROL_PRIVATE_MESSAGE_CAMPAIGN_TOOL_NAME
                | MANAGE_TIKTOK_SENDER_LOGIN_TOOL_NAME
                | PREPARE_TIKTOK_DM_CAMPAIGN_TOOL_NAME
                | SEND_TIKTOK_DM_CAMPAIGN_TOOL_NAME
                | CONTROL_DEVICE_TOOL_NAME
                | QUEUE_TIKTOK_FOLLOWER_EXTRACTION_TOOL_NAME => {
                    state.chat_tool_capabilities.claim_external_action(token)
                }
                _ => state.chat_tool_capabilities.claim_call(token),
            };
            let context = match context_result {
                Ok(value) => value,
                Err(error) => {
                    return json_response(chat_model_tools::tool_error_response(id, &error))
                }
            };
            if is_draining(&state) {
                return json_response(chat_model_tools::tool_error_response(
                    id,
                    "Le noeud est en drain ; aucune nouvelle action ne peut etre demarree.",
                ));
            }
            let arguments = payload
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match name {
                LIST_CONTROL_DEVICES_TOOL_NAME => {
                    if !arguments
                        .as_object()
                        .is_some_and(|arguments| arguments.is_empty())
                    {
                        return json_response(chat_model_tools::tool_error_response(
                            id,
                            "La liste des appareils ne prend aucun argument",
                        ));
                    }
                    match state.device_fleet.snapshot() {
                        Ok(snapshot) => {
                            let ready = snapshot
                                .devices
                                .iter()
                                .filter(|device| device.ready)
                                .count();
                            json_response(chat_model_tools::tool_microsoft_data_response(
                                id,
                                &format!(
                                    "{} appareil(s) USB detecte(s), dont {ready} pret(s). Connecteur {}.",
                                    snapshot.devices.len(),
                                    if snapshot.connector_online {
                                        "en ligne"
                                    } else {
                                        "hors ligne"
                                    }
                                ),
                                json!(snapshot),
                            ))
                        }
                        Err(error) => json_response(chat_model_tools::tool_error_response(
                            id,
                            &error.to_string(),
                        )),
                    }
                }
                GET_CONTROL_DEVICE_ACTION_TOOL_NAME => {
                    let request = match serde_json::from_value::<GetControlDeviceActionToolArguments>(
                        arguments,
                    ) {
                        Ok(request) => request,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(
                                id,
                                &format!("Arguments invalides pour le suivi USB : {error}"),
                            ))
                        }
                    };
                    let owner_id = context.user_id.as_deref().unwrap_or("server-admin");
                    match state
                        .device_fleet
                        .action_status(owner_id, &request.action_id)
                    {
                        Ok(action) => json_response(device_tool_action_record_response(id, action)),
                        Err(error) => json_response(chat_model_tools::tool_error_response(
                            id,
                            &error.to_string(),
                        )),
                    }
                }
                CONTROL_DEVICE_TOOL_NAME => {
                    let mut request = match serde_json::from_value::<DeviceActionRequest>(arguments)
                    {
                        Ok(request) => request,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(
                                id,
                                &format!("Arguments invalides pour le controle USB : {error}"),
                            ))
                        }
                    };
                    if !matches!(
                        request.action,
                        DeviceActionKind::Info | DeviceActionKind::Screenshot
                    ) && !request.confirmed
                    {
                        return json_response(chat_model_tools::tool_error_response(
                            id,
                            "Cette action modifie ou ouvre l'appareil. Demande d'abord une confirmation explicite a l'utilisateur, puis rappelle l'outil avec confirmed=true. Pour shell, la confirmation doit porter sur la commande exacte et celle-ci ne doit jamais etre modifiee.",
                        ));
                    }
                    request.idempotency_key =
                        match mcp_device_action_idempotency_key(token, &id, &request) {
                            Ok(key) => Some(key),
                            Err(error) => {
                                return json_response(chat_model_tools::tool_error_response(
                                    id,
                                    &format!(
                                        "Action USB impossible a dedupliquer proprement : {error}"
                                    ),
                                ))
                            }
                        };
                    let owner_id = context.user_id.as_deref().unwrap_or("server-admin");
                    let queued = match state.device_fleet.queue_action(owner_id, request) {
                        Ok(action) => action,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(
                                id,
                                &error.to_string(),
                            ))
                        }
                    };
                    let completed = match state
                        .device_fleet
                        .wait_action(owner_id, &queued.id, Duration::from_secs(75))
                        .await
                    {
                        Ok(action) => action,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(
                                id,
                                &error.to_string(),
                            ))
                        }
                    };
                    json_response(device_tool_action_record_response(id, completed))
                }
                CREATE_CHAT_TOOL_NAME => {
                    let arguments =
                        match serde_json::from_value::<CreateChatToolArguments>(arguments) {
                            Ok(value) => value,
                            Err(error) => {
                                return json_response(chat_model_tools::tool_error_response(
                                    id,
                                    &format!(
                                        "Arguments invalides pour l'ouverture du chat : {error}"
                                    ),
                                ))
                            }
                        };
                    let request = match arguments.into_request(&context) {
                        Ok(value) => value,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(id, &error))
                        }
                    };
                    match state.chat_open_requests.enqueue(request) {
                        Ok(request) => {
                            json_response(chat_model_tools::tool_chat_open_response(id, &request))
                        }
                        Err(error) => {
                            json_response(chat_model_tools::tool_error_response(id, &error))
                        }
                    }
                }
                SEND_CHAT_MESSAGE_TOOL_NAME => {
                    // Canal desactive par defaut : meme reponse qu'un outil
                    // inexistant pour ne pas le laisser sonder l'outil.
                    if !chat_model_tools::chat_post_tool_enabled() {
                        return json_response(chat_model_tools::protocol_error(
                            id,
                            -32602,
                            "Outil MCP inconnu",
                        ));
                    }
                    let arguments = match serde_json::from_value::<SendChatMessageToolArguments>(
                        arguments,
                    ) {
                        Ok(value) => value,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(
                                id,
                                &format!("Arguments invalides pour l'envoi du message : {error}"),
                            ))
                        }
                    };
                    let request = match arguments.into_request(&context) {
                        Ok(value) => value,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(id, &error))
                        }
                    };
                    match state.chat_post_requests.enqueue(request) {
                        Ok(request) => {
                            json_response(chat_model_tools::tool_chat_post_response(id, &request))
                        }
                        Err(error) => {
                            json_response(chat_model_tools::tool_error_response(id, &error))
                        }
                    }
                }
                CREATE_GOAL_TOOL_NAME if context.scope == ChatToolScope::GoalsOnly => {
                    let arguments = match serde_json::from_value::<CreateTerminalGoalToolArguments>(
                        arguments,
                    ) {
                        Ok(value) => value,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(
                                id,
                                &format!("Arguments invalides pour la creation du goal : {error}"),
                            ))
                        }
                    };
                    let key = match context.goal_key.clone() {
                        Some(value) => value,
                        None => {
                            return json_response(chat_model_tools::tool_error_response(
                                id,
                                "Portee persistante du goal absente",
                            ))
                        }
                    };
                    let manager = state.terminal_goals.clone();
                    match tokio::task::spawn_blocking(move || {
                        manager.create(&key, &arguments.objective, arguments.token_budget)
                    })
                    .await
                    {
                        Ok(Ok(goal)) => {
                            json_response(chat_model_tools::tool_terminal_goal_success_response(
                                id, "cree", &goal,
                            ))
                        }
                        Ok(Err(error)) => {
                            json_response(chat_model_tools::tool_error_response(id, &error))
                        }
                        Err(error) => json_response(chat_model_tools::tool_error_response(
                            id,
                            &format!("Creation du goal interrompue : {error}"),
                        )),
                    }
                }
                CREATE_GOAL_TOOL_NAME => {
                    let request = match serde_json::from_value::<CreateAutonomousGoalToolArguments>(
                        arguments,
                    ) {
                        Ok(value) => value.into_request(context),
                        Err(error) => Err(format!(
                            "Arguments invalides pour la creation du goal : {error}"
                        )),
                    };
                    let request = match request {
                        Ok(value) => value,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(id, &error))
                        }
                    };
                    let manager = state.autonomous.clone();
                    match tokio::task::spawn_blocking(move || manager.create_goal(request)).await {
                        Ok(Ok(agent)) => json_response(
                            chat_model_tools::tool_autonomous_goal_success_response(id, &agent),
                        ),
                        Ok(Err(error)) => {
                            json_response(chat_model_tools::tool_error_response(id, &error))
                        }
                        Err(error) => json_response(chat_model_tools::tool_error_response(
                            id,
                            &format!("Creation autonome interrompue : {error}"),
                        )),
                    }
                }
                GET_GOAL_TOOL_NAME => {
                    let key = match context.goal_key.as_deref() {
                        Some(value) => value,
                        None => {
                            return json_response(chat_model_tools::tool_error_response(
                                id,
                                "Portee persistante du goal absente",
                            ))
                        }
                    };
                    match state.terminal_goals.get(key) {
                        Ok(goal) => json_response(
                            chat_model_tools::tool_terminal_goal_get_response(id, goal.as_ref()),
                        ),
                        Err(error) => {
                            json_response(chat_model_tools::tool_error_response(id, &error))
                        }
                    }
                }
                UPDATE_GOAL_TOOL_NAME => {
                    let arguments =
                        match serde_json::from_value::<UpdateGoalToolArguments>(arguments) {
                            Ok(value) => value,
                            Err(error) => {
                                return json_response(chat_model_tools::tool_error_response(
                                    id,
                                    &format!(
                                        "Arguments invalides pour la mise a jour du goal : {error}"
                                    ),
                                ))
                            }
                        };
                    let key = match context.goal_key.clone() {
                        Some(value) => value,
                        None => {
                            return json_response(chat_model_tools::tool_error_response(
                                id,
                                "Portee persistante du goal absente",
                            ))
                        }
                    };
                    let manager = state.terminal_goals.clone();
                    match tokio::task::spawn_blocking(move || {
                        manager.update(&key, arguments.status)
                    })
                    .await
                    {
                        Ok(Ok(goal)) => {
                            json_response(chat_model_tools::tool_terminal_goal_success_response(
                                id,
                                "mis a jour",
                                &goal,
                            ))
                        }
                        Ok(Err(error)) => {
                            json_response(chat_model_tools::tool_error_response(id, &error))
                        }
                        Err(error) => json_response(chat_model_tools::tool_error_response(
                            id,
                            &format!("Mise a jour du goal interrompue : {error}"),
                        )),
                    }
                }
                AUTONOMOUS_AGENT_TOOL_NAME => {
                    let request = match serde_json::from_value::<CreateAutonomousAgentToolArguments>(
                        arguments,
                    ) {
                        Ok(value) => value.into_request(context),
                        Err(error) => Err(format!(
                            "Arguments invalides pour la creation autonome : {error}"
                        )),
                    };
                    let request = match request {
                        Ok(value) => value,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(id, &error))
                        }
                    };
                    let manager = state.autonomous.clone();
                    match tokio::task::spawn_blocking(move || manager.create(request)).await {
                        Ok(Ok(agent)) => {
                            json_response(chat_model_tools::tool_success_response(id, &agent))
                        }
                        Ok(Err(error)) => {
                            json_response(chat_model_tools::tool_error_response(id, &error))
                        }
                        Err(error) => json_response(chat_model_tools::tool_error_response(
                            id,
                            &format!("Creation autonome interrompue : {error}"),
                        )),
                    }
                }
                UPDATE_AUTONOMOUS_AGENT_TOOL_NAME => {
                    let arguments = match serde_json::from_value::<UpdateAutonomousAgentToolArguments>(
                        arguments,
                    ) {
                        Ok(value) => value,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(
                                id,
                                &format!(
                                    "Arguments invalides pour la modification autonome : {error}"
                                ),
                            ))
                        }
                    };
                    let manager = state.autonomous.clone();
                    let updated = tokio::task::spawn_blocking(move || {
                        let agents = manager.list()?;
                        let target = chat_model_tools::linked_agent_for_context(&agents, &context)?;
                        let was_active = target.status == AutonomousAgentStatus::Active;
                        let request = arguments.into_request(&target, was_active)?;
                        if was_active {
                            manager.control(&target.id, AutonomousAgentAction::Pause, None)?;
                        }
                        match manager.update(&target.id, request) {
                            Ok(agent) => Ok(agent),
                            Err(error) => {
                                if was_active {
                                    if let Err(resume_error) =
                                        manager.control(
                                            &target.id,
                                            AutonomousAgentAction::Resume,
                                            None,
                                        )
                                    {
                                        return Err(format!(
                                            "{error}. L'agent a ete securise en pause et sa reprise a echoue : {resume_error}"
                                        ));
                                    }
                                }
                                Err(error)
                            }
                        }
                    })
                    .await;
                    match updated {
                        Ok(Ok(agent)) => json_response(
                            chat_model_tools::tool_update_success_response(id, &agent),
                        ),
                        Ok(Err(error)) => {
                            json_response(chat_model_tools::tool_error_response(id, &error))
                        }
                        Err(error) => json_response(chat_model_tools::tool_error_response(
                            id,
                            &format!("Modification autonome interrompue : {error}"),
                        )),
                    }
                }
                PAUSE_AUTONOMOUS_AGENT_TOOL_NAME => {
                    if !arguments
                        .as_object()
                        .is_some_and(|arguments| arguments.is_empty())
                    {
                        return json_response(chat_model_tools::tool_error_response(
                            id,
                            "La mise en pause ne prend aucun argument ni identifiant d'agent",
                        ));
                    }
                    let manager = state.autonomous.clone();
                    let paused = tokio::task::spawn_blocking(move || -> Result<_, String> {
                        let agents = manager.list()?;
                        let target = chat_model_tools::linked_agent_for_context(&agents, &context)?;
                        if target.status == AutonomousAgentStatus::Paused {
                            Ok((target, true))
                        } else {
                            manager
                                .control(&target.id, AutonomousAgentAction::Pause, None)
                                .map(|agent| (agent, false))
                        }
                    })
                    .await;
                    match paused {
                        Ok(Ok((agent, already_paused))) => {
                            json_response(chat_model_tools::tool_pause_success_response(
                                id,
                                &agent,
                                already_paused,
                            ))
                        }
                        Ok(Err(error)) => {
                            json_response(chat_model_tools::tool_error_response(id, &error))
                        }
                        Err(error) => json_response(chat_model_tools::tool_error_response(
                            id,
                            &format!("Mise en pause autonome interrompue : {error}"),
                        )),
                    }
                }
                ACTIVATE_SUPERVISOR_GENERAL_REPORT_TOOL_NAME => {
                    if !arguments
                        .as_object()
                        .is_some_and(|arguments| arguments.is_empty())
                    {
                        return json_response(chat_model_tools::tool_error_response(
                            id,
                            "Le compte rendu general ne prend aucun argument ni identifiant d'agent",
                        ));
                    }
                    let manager = state.autonomous.clone();
                    let activated =
                        tokio::task::spawn_blocking(move || manager.activate_general_report())
                            .await;
                    match activated {
                        Ok(Ok((supervisor, pending, scheduled))) => {
                            json_response(chat_model_tools::tool_general_report_response(
                                id,
                                supervisor.as_ref(),
                                pending,
                                scheduled,
                            ))
                        }
                        Ok(Err(error)) => {
                            json_response(chat_model_tools::tool_error_response(id, &error))
                        }
                        Err(error) => json_response(chat_model_tools::tool_error_response(
                            id,
                            &format!("Activation du compte rendu general interrompue : {error}"),
                        )),
                    }
                }
                APPLY_AUTONOMOUS_AGENT_POLICY_TOOL_NAME => {
                    let arguments = match serde_json::from_value::<
                        ApplyAutonomousAgentPolicyToolArguments,
                    >(arguments)
                    {
                        Ok(value) => value,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(
                                id,
                                &format!(
                                    "Arguments invalides pour la politique autonome : {error}"
                                ),
                            ))
                        }
                    };
                    let scope = arguments.resolved_scope(&context);
                    let require_visual_evidence = arguments.require_visual_evidence;
                    let instruction = arguments.instruction.trim().to_string();
                    if instruction.is_empty() || instruction.chars().count() > 2_000 {
                        return json_response(chat_model_tools::tool_error_response(
                            id,
                            "La politique doit contenir entre 1 et 2000 caracteres",
                        ));
                    }
                    let manager = state.autonomous.clone();
                    let applied = tokio::task::spawn_blocking(move || -> Result<_, String> {
                        let agents = manager.list()?;
                        let targets =
                            chat_model_tools::agents_for_policy_context(&agents, &context, scope)?;
                        let mut updated = Vec::new();
                        let mut failures = Vec::new();
                        for target in targets {
                            let was_active = target.status == AutonomousAgentStatus::Active;
                            let activate = was_active || target.pending_review.is_some();
                            if was_active {
                                if let Err(error) =
                                    manager.control(
                                        &target.id,
                                        AutonomousAgentAction::Pause,
                                        None,
                                    )
                                {
                                    failures.push((target.id, target.name, error));
                                    continue;
                                }
                            }
                            match manager.apply_review_policy(
                                &target.id,
                                &instruction,
                                require_visual_evidence,
                                activate,
                            ) {
                                Ok(agent) => updated.push(agent),
                                Err(error) => {
                                    let error = if was_active {
                                        match manager
                                            .control(
                                                &target.id,
                                                AutonomousAgentAction::Resume,
                                                None,
                                            )
                                        {
                                            Ok(_) => error,
                                            Err(resume_error) => format!(
                                                "{error}. L'agent reste en pause car sa reprise a echoue : {resume_error}"
                                            ),
                                        }
                                    } else {
                                        error
                                    };
                                    failures.push((target.id, target.name, error));
                                }
                            }
                        }
                        Ok((updated, failures))
                    })
                    .await;
                    match applied {
                        Ok(Ok((updated, failures))) => json_response(
                            chat_model_tools::tool_policy_response(id, &updated, &failures),
                        ),
                        Ok(Err(error)) => {
                            json_response(chat_model_tools::tool_error_response(id, &error))
                        }
                        Err(error) => json_response(chat_model_tools::tool_error_response(
                            id,
                            &format!("Application de la politique interrompue : {error}"),
                        )),
                    }
                }
                LIST_PRIVATE_MESSAGE_USERS_TOOL_NAME => {
                    if !arguments
                        .as_object()
                        .is_some_and(|arguments| arguments.is_empty())
                    {
                        return json_response(chat_model_tools::tool_error_response(
                            id,
                            "La liste des destinataires ne prend aucun argument",
                        ));
                    }
                    let actor = match private_message_actor_for_tool(&state, &context) {
                        Ok(actor) => actor,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(id, &error))
                        }
                    };
                    let identities = match state.auth.public_identities() {
                        Ok(identities) => identities,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(id, &error))
                        }
                    };
                    let mut users = identities
                        .into_iter()
                        .map(private_message_user_from_identity)
                        .filter(|user| user.id != actor.id)
                        .collect::<Vec<_>>();
                    if actor.id != "server-admin" {
                        users.push(PrivateMessageUser::administrator());
                    }
                    users.sort_by(|left, right| {
                        left.username
                            .to_lowercase()
                            .cmp(&right.username.to_lowercase())
                            .then_with(|| left.id.cmp(&right.id))
                    });
                    let count = users.len();
                    json_response(chat_model_tools::tool_microsoft_data_response(
                        id,
                        &format!("{count} destinataire(s) interne(s) disponible(s)."),
                        json!({ "count": count, "users": users }),
                    ))
                }
                LIST_PRIVATE_MESSAGE_CAMPAIGNS_TOOL_NAME => {
                    if !arguments
                        .as_object()
                        .is_some_and(|arguments| arguments.is_empty())
                    {
                        return json_response(chat_model_tools::tool_error_response(
                            id,
                            "La liste des campagnes ne prend aucun argument",
                        ));
                    }
                    let actor = match private_message_actor_for_tool(&state, &context) {
                        Ok(actor) => actor,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(id, &error))
                        }
                    };
                    match state.private_messages.list_campaigns(&actor.id) {
                        Ok(campaigns) => {
                            let count = campaigns.len();
                            json_response(chat_model_tools::tool_microsoft_data_response(
                                id,
                                &format!("{count} campagne(s) interne(s) trouvee(s)."),
                                json!({ "count": count, "campaigns": campaigns }),
                            ))
                        }
                        Err(error) => json_response(chat_model_tools::tool_error_response(
                            id,
                            &error.to_string(),
                        )),
                    }
                }
                CREATE_PRIVATE_MESSAGE_CAMPAIGN_TOOL_NAME => {
                    let request = match serde_json::from_value::<CreatePrivateMessageCampaignRequest>(
                        arguments,
                    ) {
                        Ok(request) => request,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(
                                id,
                                &format!("Arguments invalides pour la campagne interne : {error}"),
                            ))
                        }
                    };
                    let actor = match private_message_actor_for_tool(&state, &context) {
                        Ok(actor) => actor,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(id, &error))
                        }
                    };
                    let recipients = request
                        .recipient_ids
                        .iter()
                        .map(|user_id| private_message_recipient_for_tool(&state, &actor, user_id))
                        .collect::<Result<Vec<_>, _>>();
                    let recipients = match recipients {
                        Ok(recipients) => recipients,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(id, &error))
                        }
                    };
                    match state.private_messages.create_campaign(
                        actor.clone(),
                        recipients,
                        request,
                        "autonomous_agent",
                    ) {
                        Ok(campaign) => {
                            state
                                .chat
                                .runtime_sync()
                                .notify_private_messages([actor.id]);
                            json_response(chat_model_tools::tool_microsoft_data_response(
                                id,
                                &format!(
                                    "Campagne interne « {} » creee avec le statut {:?}.",
                                    campaign.name, campaign.status
                                ),
                                json!({ "campaign": campaign }),
                            ))
                        }
                        Err(error) => json_response(chat_model_tools::tool_error_response(
                            id,
                            &error.to_string(),
                        )),
                    }
                }
                CONTROL_PRIVATE_MESSAGE_CAMPAIGN_TOOL_NAME => {
                    let arguments = match serde_json::from_value::<
                        ControlPrivateMessageCampaignToolArguments,
                    >(arguments)
                    {
                        Ok(arguments) => arguments,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(
                                id,
                                &format!(
                                    "Arguments invalides pour le pilotage de campagne : {error}"
                                ),
                            ))
                        }
                    };
                    let actor = match private_message_actor_for_tool(&state, &context) {
                        Ok(actor) => actor,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(id, &error))
                        }
                    };
                    match state.private_messages.control_campaign(
                        &actor.id,
                        &arguments.campaign_id,
                        arguments.action,
                        false,
                    ) {
                        Ok(campaign) => {
                            state
                                .chat
                                .runtime_sync()
                                .notify_private_messages([actor.id]);
                            json_response(chat_model_tools::tool_microsoft_data_response(
                                id,
                                &format!(
                                    "Campagne interne « {} » mise a jour : {:?}.",
                                    campaign.name, campaign.status
                                ),
                                json!({ "campaign": campaign }),
                            ))
                        }
                        Err(error) => json_response(chat_model_tools::tool_error_response(
                            id,
                            &error.to_string(),
                        )),
                    }
                }
                LIST_TIKTOK_DM_CAMPAIGNS_TOOL_NAME => {
                    if !arguments
                        .as_object()
                        .is_some_and(|arguments| arguments.is_empty())
                    {
                        return json_response(chat_model_tools::tool_error_response(
                            id,
                            "La liste des campagnes TikTok ne prend aucun argument",
                        ));
                    }
                    let owner_id = context.user_id.as_deref().unwrap_or("server-admin");
                    let campaigns = match state.tiktok_messaging.list(owner_id) {
                        Ok(campaigns) => campaigns,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(
                                id,
                                &error.to_string(),
                            ))
                        }
                    };
                    let connector = match state.tiktok_messaging.connector_status() {
                        Ok(connector) => connector,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(
                                id,
                                &error.to_string(),
                            ))
                        }
                    };
                    let connector_online = connector
                        .as_ref()
                        .is_some_and(|status| status.is_online(metrics::now_ts()));
                    json_response(chat_model_tools::tool_microsoft_data_response(
                        id,
                        &format!(
                            "{} campagne(s) TikTok ; connecteur Windows {}.",
                            campaigns.len(),
                            if connector_online {
                                "en ligne"
                            } else {
                                "hors ligne"
                            }
                        ),
                        json!({
                            "campaigns": campaigns,
                            "connector": connector,
                            "connectorOnline": connector_online
                        }),
                    ))
                }
                LIST_TIKTOK_SENDER_ACCOUNTS_TOOL_NAME => {
                    if !arguments
                        .as_object()
                        .is_some_and(|arguments| arguments.is_empty())
                    {
                        return json_response(chat_model_tools::tool_error_response(
                            id,
                            "La liste des comptes emetteurs TikTok ne prend aucun argument",
                        ));
                    }
                    let owner_id = context.user_id.as_deref().unwrap_or("server-admin");
                    let accounts = match state.tiktok_messaging.sender_accounts(owner_id) {
                        Ok(accounts) => accounts,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(
                                id,
                                &error.to_string(),
                            ))
                        }
                    };
                    let connector_online = state
                        .tiktok_messaging
                        .connector_status()
                        .ok()
                        .flatten()
                        .is_some_and(|status| {
                            status.is_online(metrics::now_ts()) && status.agent_healthy
                        });
                    let text = if !connector_online {
                        "Le connecteur Windows TikMatrix est hors ligne. Demarrez Codex Switch Terminal Cloud et TikMatrix sur Windows."
                    } else if accounts.is_empty() {
                        "Aucun compte emetteur n'est reconnu. Connectez-vous dans l'application TikTok de l'appareil Android, resolvez localement le code ou captcha si necessaire, puis lancez Match Accounts dans TikMatrix."
                    } else {
                        "Comptes emetteurs TikTok reconnus par TikMatrix. Selectionnez-en un avant l'envoi si aucun n'est deja selectionne."
                    };
                    json_response(chat_model_tools::tool_microsoft_data_response(
                        id,
                        text,
                        json!({
                            "accounts": accounts,
                            "connectorOnline": connector_online,
                            "secretsStoredOnVps": false
                        }),
                    ))
                }
                MANAGE_TIKTOK_SENDER_LOGIN_TOOL_NAME => {
                    let request =
                        match serde_json::from_value::<QueueTikTokSenderSetupRequest>(arguments) {
                            Ok(request) => request,
                            Err(error) => {
                                return json_response(chat_model_tools::tool_error_response(
                                    id,
                                    &format!(
                                        "Arguments invalides pour la connexion TikTok : {error}"
                                    ),
                                ))
                            }
                        };
                    let action_kind = request.action;
                    let owner_id = context.user_id.as_deref().unwrap_or("server-admin");
                    match state.tiktok_messaging.queue_sender_setup(owner_id, request) {
                        Ok(action) => {
                            let text = match action_kind {
                                TikTokSenderSetupActionKind::OpenLogin => format!(
                                    "Ouverture de TikTok demandee sur l'appareil {}. Saisissez vos identifiants et resolvez le captcha ou la verification uniquement dans la fenetre TikTok locale. Revenez ensuite dans ce chat et dites que la connexion est terminee.",
                                    action.device_serial
                                ),
                                TikTokSenderSetupActionKind::MatchAccounts => format!(
                                    "Synchronisation du compte demandee sur l'appareil {}. Attendez quelques secondes, puis verifiez le compte emetteur reconnu.",
                                    action.device_serial
                                ),
                                TikTokSenderSetupActionKind::OpenScrcpy => format!(
                                    "Ouverture de scrcpy demandee sur l'appareil {}. La fenetre apparait sur le poste Windows qui execute le connecteur.",
                                    action.device_serial
                                ),
                                TikTokSenderSetupActionKind::StartWsScrcpy =>
                                    "Demarrage de ws-scrcpy-web demande sur le poste Windows qui execute le connecteur."
                                        .to_string(),
                            };
                            json_response(chat_model_tools::tool_microsoft_data_response(
                                id,
                                &text,
                                json!({
                                    "action": action,
                                    "credentialsRequestedInChat": false,
                                    "tikTokTokenStoredOnVps": false,
                                    "sessionStorage": "TikTok/TikMatrix local"
                                }),
                            ))
                        }
                        Err(error) => json_response(chat_model_tools::tool_error_response(
                            id,
                            &error.to_string(),
                        )),
                    }
                }
                SELECT_TIKTOK_SENDER_ACCOUNT_TOOL_NAME => {
                    let request =
                        match serde_json::from_value::<SelectTikTokSenderAccountRequest>(arguments)
                        {
                            Ok(request) => request,
                            Err(error) => {
                                return json_response(chat_model_tools::tool_error_response(
                                    id,
                                    &format!(
                                    "Arguments invalides pour le compte emetteur TikTok : {error}"
                                ),
                                ))
                            }
                        };
                    let owner_id = context.user_id.as_deref().unwrap_or("server-admin");
                    match state
                        .tiktok_messaging
                        .select_sender_account(owner_id, &request.username)
                    {
                        Ok(account) => json_response(
                            chat_model_tools::tool_microsoft_data_response(
                                id,
                                &format!(
                                    "{} est maintenant le compte emetteur TikTok par defaut sur l'appareil {}.",
                                    account.username, account.device_serial
                                ),
                                json!({ "account": account }),
                            ),
                        ),
                        Err(error) => json_response(chat_model_tools::tool_error_response(
                            id,
                            &error.to_string(),
                        )),
                    }
                }
                PREPARE_TIKTOK_DM_CAMPAIGN_TOOL_NAME => {
                    let request =
                        match serde_json::from_value::<PrepareTikTokDmCampaignRequest>(arguments) {
                            Ok(request) => request,
                            Err(error) => {
                                return json_response(chat_model_tools::tool_error_response(
                                    id,
                                    &format!("Arguments invalides pour l'apercu TikTok : {error}"),
                                ))
                            }
                        };
                    let owner_id = context.user_id.as_deref().unwrap_or("server-admin");
                    let created_by = if context.scope == ChatToolScope::PersonalDataOnly {
                        "autonomous_agent"
                    } else {
                        "human_chat"
                    };
                    match state
                        .tiktok_messaging
                        .prepare(owner_id, request, created_by)
                    {
                        Ok(campaign) => json_response(
                            chat_model_tools::tool_microsoft_data_response(
                                id,
                                &format!(
                                    "Apercu TikTok prepare pour {} compte(s). Aucun message n'a ete envoye.",
                                    campaign.recipients.len()
                                ),
                                json!({ "campaign": campaign, "requiresConfirmation": true }),
                            ),
                        ),
                        Err(error) => json_response(chat_model_tools::tool_error_response(
                            id,
                            &error.to_string(),
                        )),
                    }
                }
                SEND_TIKTOK_DM_CAMPAIGN_TOOL_NAME => {
                    let request =
                        match serde_json::from_value::<SendTikTokDmToolArguments>(arguments) {
                            Ok(request) => request,
                            Err(error) => {
                                return json_response(chat_model_tools::tool_error_response(
                                    id,
                                    &format!("Arguments invalides pour l'envoi TikTok : {error}"),
                                ))
                            }
                        };
                    let owner_id = context.user_id.as_deref().unwrap_or("server-admin");
                    let queued = match request {
                        SendTikTokDmToolArguments::Prepared(request) => {
                            state.tiktok_messaging.confirm(owner_id, request)
                        }
                        SendTikTokDmToolArguments::Direct(mut request) => {
                            let connector = match state.tiktok_messaging.connector_status() {
                                Ok(connector) => connector,
                                Err(error) => {
                                    return json_response(chat_model_tools::tool_error_response(
                                        id,
                                        &error.to_string(),
                                    ))
                                }
                            };
                            if !connector.as_ref().is_some_and(|status| {
                                status.is_online(metrics::now_ts()) && status.agent_healthy
                            }) {
                                return json_response(chat_model_tools::tool_error_response(
                                    id,
                                    "Le connecteur Windows TikMatrix est hors ligne ou indisponible. Demarrez l'application Windows et TikMatrix, puis recommencez l'envoi.",
                                ));
                            }
                            let created_by = if context.scope == ChatToolScope::PersonalDataOnly {
                                "autonomous_agent"
                            } else {
                                "human_chat"
                            };
                            let sender = match state.tiktok_messaging.resolve_sender_account(
                                owner_id,
                                request.sender_account.as_deref(),
                                request.device_serial.as_deref(),
                            ) {
                                Ok(sender) => sender,
                                Err(error) => {
                                    return json_response(chat_model_tools::tool_error_response(
                                        id,
                                        &error.to_string(),
                                    ))
                                }
                            };
                            request.sender_account = Some(sender.username);
                            request.device_serial = Some(sender.device_serial);
                            state
                                .tiktok_messaging
                                .prepare(owner_id, request.into_prepare_request(token), created_by)
                                .and_then(|campaign| {
                                    state.tiktok_messaging.confirm(
                                        owner_id,
                                        ConfirmTikTokDmCampaignRequest {
                                            campaign_id: campaign.id,
                                            owned_accounts_confirmed: true,
                                            send_confirmed: true,
                                        },
                                    )
                                })
                        }
                    };
                    let campaign = match queued {
                        Ok(campaign) => campaign,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(
                                id,
                                &error.to_string(),
                            ))
                        }
                    };
                    let campaign =
                        match wait_for_tiktok_submission(&state, owner_id, campaign).await {
                            Ok(campaign) => campaign,
                            Err(error) => {
                                return json_response(chat_model_tools::tool_error_response(
                                    id,
                                    &error.to_string(),
                                ))
                            }
                        };
                    let text = match campaign.status {
                        TikTokDmCampaignStatus::Submitted => {
                            "TikMatrix a accepte la tache d'envoi TikTok. La livraison finale depend ensuite de TikTok."
                        }
                        TikTokDmCampaignStatus::Failed => {
                            "TikMatrix a refuse ou n'a pas pu lancer l'envoi TikTok."
                        }
                        TikTokDmCampaignStatus::Cancelled => {
                            "L'envoi TikTok a ete annule."
                        }
                        _ => {
                            "L'envoi TikTok est dans la file du connecteur Windows et reste en cours de traitement."
                        }
                    };
                    json_response(chat_model_tools::tool_microsoft_data_response(
                        id,
                        text,
                        json!({
                            "campaign": campaign,
                            "tikMatrixAccepted": campaign.status == TikTokDmCampaignStatus::Submitted,
                            "deliveryConfirmed": false
                        }),
                    ))
                }
                LIST_TIKTOK_FOLLOWER_EXTRACTIONS_TOOL_NAME => {
                    if !arguments
                        .as_object()
                        .is_some_and(|arguments| arguments.is_empty())
                    {
                        return json_response(chat_model_tools::tool_error_response(
                            id,
                            "La liste des collectes TikTok ne prend aucun argument",
                        ));
                    }
                    let owner_id = context.user_id.as_deref().unwrap_or("server-admin");
                    match state.tiktok_messaging.list_follower_extractions(owner_id) {
                        Ok(extractions) => {
                            let completed = extractions
                                .iter()
                                .filter(|extraction| {
                                    extraction.status
                                        == crate::tiktok_messaging::TikTokFollowerExtractionStatus::Completed
                                })
                                .count();
                            json_response(chat_model_tools::tool_microsoft_data_response(
                                id,
                                &format!(
                                    "{} collecte(s) TikTok, dont {completed} terminee(s).",
                                    extractions.len()
                                ),
                                json!({ "extractions": extractions }),
                            ))
                        }
                        Err(error) => json_response(chat_model_tools::tool_error_response(
                            id,
                            &error.to_string(),
                        )),
                    }
                }
                QUEUE_TIKTOK_FOLLOWER_EXTRACTION_TOOL_NAME => {
                    let request = match serde_json::from_value::<QueueTikTokFollowerExtractionRequest>(
                        arguments,
                    ) {
                        Ok(request) => request,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(
                                id,
                                &format!("Arguments invalides pour la collecte TikTok : {error}"),
                            ))
                        }
                    };
                    let owner_id = context.user_id.as_deref().unwrap_or("server-admin");
                    let created_by = if context.scope == ChatToolScope::PersonalDataOnly {
                        "autonomous_agent"
                    } else {
                        "human_chat"
                    };
                    match state
                        .tiktok_messaging
                        .queue_follower_extraction(owner_id, request, created_by)
                    {
                        Ok(extraction) => {
                            let pipeline_note = if extraction.dm_pipeline.is_some() {
                                " Un brouillon sera prepare uniquement pour l'intersection avec la liste de comptes secondaires controles ; aucun message ne sera envoye sans un nouvel apercu et une confirmation humaine."
                            } else {
                                ""
                            };
                            json_response(chat_model_tools::tool_microsoft_data_response(
                                id,
                                &format!(
                                    "Collecte TikTok placee dans la file pour {} (maximum {} noms).{}",
                                    extraction.target_username, extraction.max_count, pipeline_note
                                ),
                                json!({ "extraction": extraction }),
                            ))
                        }
                        Err(error) => json_response(chat_model_tools::tool_error_response(
                            id,
                            &error.to_string(),
                        )),
                    }
                }
                LIST_OUTLOOK_MESSAGES_TOOL_NAME => {
                    let Some(owner_id) = context.user_id.clone() else {
                        return json_response(chat_model_tools::tool_error_response(
                            id,
                            MICROSOFT_NO_NOMINAL_USER,
                        ));
                    };
                    let arguments = match serde_json::from_value::<ListMessagesArguments>(arguments)
                    {
                        Ok(value) => value,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(
                                id,
                                &format!(
                                    "Arguments invalides pour la lecture des e-mails : {error}"
                                ),
                            ))
                        }
                    };
                    match state.microsoft.list_messages(&owner_id, &arguments).await {
                        Ok(data) => {
                            let count = data.get("count").and_then(Value::as_u64).unwrap_or(0);
                            let mailbox = data
                                .get("mailbox")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            json_response(chat_model_tools::tool_microsoft_data_response(
                                id,
                                &format!("{count} e-mail(s) lu(s) dans la boite {mailbox}."),
                                data,
                            ))
                        }
                        Err(error) => {
                            json_response(chat_model_tools::tool_error_response(id, &error))
                        }
                    }
                }
                LIST_CALENDAR_EVENTS_TOOL_NAME => {
                    let Some(owner_id) = context.user_id.clone() else {
                        return json_response(chat_model_tools::tool_error_response(
                            id,
                            MICROSOFT_NO_NOMINAL_USER,
                        ));
                    };
                    let arguments = match serde_json::from_value::<ListEventsArguments>(arguments) {
                        Ok(value) => value,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(
                                id,
                                &format!(
                                    "Arguments invalides pour la lecture de l'agenda : {error}"
                                ),
                            ))
                        }
                    };
                    match state.microsoft.list_events(&owner_id, &arguments).await {
                        Ok(data) => {
                            let count = data.get("count").and_then(Value::as_u64).unwrap_or(0);
                            let mailbox = data
                                .get("mailbox")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            json_response(chat_model_tools::tool_microsoft_data_response(
                                id,
                                &format!(
                                    "{count} evenement(s) dans l'agenda de {mailbox}. Les horaires sont en UTC."
                                ),
                                data,
                            ))
                        }
                        Err(error) => {
                            json_response(chat_model_tools::tool_error_response(id, &error))
                        }
                    }
                }
                SEND_OUTLOOK_EMAIL_TOOL_NAME
                | CREATE_CALENDAR_EVENT_TOOL_NAME
                | UPDATE_CALENDAR_EVENT_TOOL_NAME => {
                    let Some(owner_id) = context.user_id.clone() else {
                        return json_response(chat_model_tools::tool_error_response(
                            id,
                            MICROSOFT_NO_NOMINAL_USER,
                        ));
                    };
                    // Chaque outil d'ecriture renvoie (brouillon, boite visee).
                    // La boite est None quand le modele ne precise rien : la
                    // boite par defaut de l'utilisateur sera alors employee.
                    let draft = match name {
                        SEND_OUTLOOK_EMAIL_TOOL_NAME => {
                            serde_json::from_value::<SendEmailArguments>(arguments)
                                .map_err(|error| {
                                    format!("Arguments invalides pour l'e-mail : {error}")
                                })
                                .and_then(SendEmailArguments::into_draft)
                        }
                        CREATE_CALENDAR_EVENT_TOOL_NAME => serde_json::from_value::<
                            CreateEventArguments,
                        >(arguments)
                        .map_err(|error| format!("Arguments invalides pour l'evenement : {error}"))
                        .and_then(CreateEventArguments::into_draft),
                        _ => serde_json::from_value::<UpdateEventArguments>(arguments)
                            .map_err(|error| {
                                format!("Arguments invalides pour la modification : {error}")
                            })
                            .and_then(UpdateEventArguments::into_draft),
                    };
                    let (draft, account) = match draft {
                        Ok(value) => value,
                        Err(error) => {
                            return json_response(chat_model_tools::tool_error_response(id, &error))
                        }
                    };
                    match state
                        .microsoft
                        .enqueue(
                            &owner_id,
                            draft,
                            account.as_deref(),
                            context.source_chat_key.clone(),
                        )
                        .await
                    {
                        Ok(action) => json_response(
                            chat_model_tools::tool_pending_action_response(id, &action),
                        ),
                        Err(error) => {
                            json_response(chat_model_tools::tool_error_response(id, &error))
                        }
                    }
                }
                _ => unreachable!("outil valide avant le dispatch"),
            }
        }
        _ => json_response(chat_model_tools::protocol_error(
            id,
            -32601,
            "Methode MCP inconnue",
        )),
    }
}

async fn api_process_voice(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<voice::VoiceProcessRequest>,
) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }
    match voice::process_voice_request(request).await {
        Ok(value) => json_response(value),
        Err(error) => api_error(StatusCode::BAD_GATEWAY, &error, &state.config),
    }
}

async fn api_voice_runtime_status(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }
    match voice::voice_runtime_status().await {
        Ok(value) => json_response(value),
        Err(error) => api_error(StatusCode::BAD_GATEWAY, &error, &state.config),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AudioTranscriptionQuery {
    #[serde(default)]
    file_name: String,
    #[serde(default = "default_transcription_language")]
    language: String,
    #[serde(default = "default_transcription_output_mode")]
    output_mode: String,
}

fn default_transcription_language() -> String {
    "auto".to_string()
}

fn default_transcription_output_mode() -> String {
    "clean".to_string()
}

async fn api_transcribe_audio_file(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Query(query): Query<AudioTranscriptionQuery>,
    body: Bytes,
) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }
    let mime_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let file_name = if query.file_name.trim().is_empty() {
        "audio.bin".to_string()
    } else {
        query.file_name
    };

    match voice::transcribe_audio_file_bytes(
        body.to_vec(),
        file_name,
        mime_type,
        query.language,
        query.output_mode,
    )
    .await
    {
        Ok(value) => json_response(value),
        Err(error) => api_error(StatusCode::BAD_GATEWAY, &error, &state.config),
    }
}

fn creative_owner_id(state: &Arc<ServerState>, headers: &HeaderMap) -> Result<String, Response> {
    let bearer = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let provided = bearer.strip_prefix("Bearer ").unwrap_or(bearer).trim();
    if crate::security::constant_time_eq(provided.as_bytes(), state.config.admin_token.as_bytes()) {
        return Ok("server-admin".to_string());
    }
    match state.auth.identity_from_headers(headers) {
        Ok(Some(identity)) => Ok(identity.id),
        Ok(None) => Err(api_error(
            StatusCode::UNAUTHORIZED,
            "authentification requise",
            &state.config,
        )),
        Err(error) => Err(api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &error,
            &state.config,
        )),
    }
}

fn creative_generation_error_status(error: &str) -> StatusCode {
    let normalized = error.to_lowercase();
    if normalized.contains("aucun compte") || normalized.contains("n’est plus configuré") {
        StatusCode::SERVICE_UNAVAILABLE
    } else if normalized.contains("lecture")
        || normalized.contains("écriture")
        || normalized.contains("fichier")
        || normalized.contains("verrou")
        || normalized.contains("client de génération")
        || normalized.contains("client fal.ai indisponible")
    {
        StatusCode::INTERNAL_SERVER_ERROR
    } else if normalized.contains("injoignable")
        || normalized.starts_with("validation fal.ai impossible")
        || normalized.starts_with("connexion fal.ai refusée")
        || normalized.starts_with("échec de ")
        || normalized.starts_with("réponse fal.ai")
        || normalized.starts_with("résultat fal.ai")
        || normalized.starts_with("fal.ai n’a pas renvoyé")
        || normalized.starts_with("fal.ai a renvoyé")
        || normalized.starts_with("annulation impossible")
    {
        StatusCode::BAD_GATEWAY
    } else {
        StatusCode::BAD_REQUEST
    }
}

fn creative_json_rejection(state: &Arc<ServerState>, _error: JsonRejection) -> Response {
    api_error(
        StatusCode::BAD_REQUEST,
        "requête JSON créative invalide",
        &state.config,
    )
}

async fn api_creative_accounts(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    let owner_id = match creative_owner_id(&state, &headers) {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    match creative_accounts::creative_accounts_for_owner(&owner_id) {
        Ok(value) => json_response(value),
        Err(error) => api_error(
            creative_generation_error_status(&error),
            &error,
            &state.config,
        ),
    }
}

async fn api_connect_creative_account(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    request: Result<Json<ConnectCreativeAccountRequest>, JsonRejection>,
) -> Response {
    let owner_id = match creative_owner_id(&state, &headers) {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    let Json(request) = match request {
        Ok(request) => request,
        Err(error) => return creative_json_rejection(&state, error),
    };
    match creative_accounts::connect_creative_account_for_owner(&owner_id, request).await {
        Ok(value) => json_response(value),
        Err(error) => api_error(
            creative_generation_error_status(&error),
            &error,
            &state.config,
        ),
    }
}

async fn api_delete_creative_account(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    request: Result<Json<CreativeAccountIdRequest>, JsonRejection>,
) -> Response {
    let owner_id = match creative_owner_id(&state, &headers) {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    let Json(request) = match request {
        Ok(request) => request,
        Err(error) => return creative_json_rejection(&state, error),
    };
    match creative_accounts::delete_creative_account_for_owner(&owner_id, request) {
        Ok(value) => json_response(value),
        Err(error) => api_error(
            creative_generation_error_status(&error),
            &error,
            &state.config,
        ),
    }
}

async fn api_set_default_creative_account(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    request: Result<Json<CreativeAccountIdRequest>, JsonRejection>,
) -> Response {
    let owner_id = match creative_owner_id(&state, &headers) {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    let Json(request) = match request {
        Ok(request) => request,
        Err(error) => return creative_json_rejection(&state, error),
    };
    match creative_accounts::set_default_creative_account_for_owner(&owner_id, request) {
        Ok(value) => json_response(value),
        Err(error) => api_error(
            creative_generation_error_status(&error),
            &error,
            &state.config,
        ),
    }
}

async fn api_freebuff_cloud_status(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }
    json_response(freebuff_cloud::status())
}

async fn api_freebuff_cloud_connect(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    request: Result<Json<ConnectFreebuffCloudRequest>, JsonRejection>,
) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }
    let Json(request) = match request {
        Ok(request) => request,
        Err(error) => return creative_json_rejection(&state, error),
    };
    match freebuff_cloud::connect(request.session_cookie).await {
        Ok(value) => json_response(value),
        Err(error) => api_error(
            creative_generation_error_status(&error),
            &error,
            &state.config,
        ),
    }
}

async fn api_freebuff_cloud_disconnect(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }
    match freebuff_cloud::disconnect() {
        Ok(()) => json_response(json!({ "ok": true })),
        Err(error) => api_error(
            creative_generation_error_status(&error),
            &error,
            &state.config,
        ),
    }
}

async fn api_freebuff_cloud_projects(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }
    match freebuff_cloud::projects().await {
        Ok(value) => json_response(value),
        Err(error) => api_error(
            creative_generation_error_status(&error),
            &error,
            &state.config,
        ),
    }
}

async fn api_freebuff_cloud_create_blank_project(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    request: Result<Json<CreateBlankProjectRequest>, JsonRejection>,
) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }
    let Json(request) = match request {
        Ok(request) => request,
        Err(error) => return creative_json_rejection(&state, error),
    };
    match freebuff_cloud::create_blank_project(request.name).await {
        Ok(value) => json_response(value),
        Err(error) => api_error(
            creative_generation_error_status(&error),
            &error,
            &state.config,
        ),
    }
}

async fn api_freebuff_cloud_connect_repo(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    request: Result<Json<ConnectRepoRequest>, JsonRejection>,
) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }
    let Json(request) = match request {
        Ok(request) => request,
        Err(error) => return creative_json_rejection(&state, error),
    };
    match freebuff_cloud::connect_repo(request.repo_full_name).await {
        Ok(value) => json_response(value),
        Err(error) => api_error(
            creative_generation_error_status(&error),
            &error,
            &state.config,
        ),
    }
}

async fn api_freebuff_cloud_delete_project(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    request: Result<Json<DeleteProjectRequest>, JsonRejection>,
) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }
    let Json(request) = match request {
        Ok(request) => request,
        Err(error) => return creative_json_rejection(&state, error),
    };
    match freebuff_cloud::delete_project(request.project_id).await {
        Ok(value) => json_response(value),
        Err(error) => api_error(
            creative_generation_error_status(&error),
            &error,
            &state.config,
        ),
    }
}

async fn api_freebuff_cloud_connectable_repos(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }
    match freebuff_cloud::connectable_repos().await {
        Ok(value) => json_response(value),
        Err(error) => api_error(
            creative_generation_error_status(&error),
            &error,
            &state.config,
        ),
    }
}

/// Proxy SSE : relaie `/api/agent-runs/stream` de freebuff.com (avec le cookie
/// de session) vers le client. Le cookie ne quitte jamais le serveur.
async fn api_freebuff_cloud_stream(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Query(query): Query<AgentRunStreamQuery>,
) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }
    let cookie = match freebuff_cloud::session_cookie() {
        Ok(cookie) => cookie,
        Err(error) => {
            return api_error(
                creative_generation_error_status(&error),
                &error,
                &state.config,
            )
        }
    };
    let url = freebuff_cloud::agent_run_stream_url(&query.message_id, query.run_id.as_deref());
    let upstream = match reqwest::Client::new()
        .get(&url)
        .header(reqwest::header::COOKIE, cookie)
        .header("accept", "text/event-stream")
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => {
            return api_error(
                StatusCode::BAD_GATEWAY,
                "Flux Freebuff Cloud injoignable",
                &state.config,
            )
        }
    };
    if !upstream.status().is_success() {
        return api_error(
            StatusCode::BAD_GATEWAY,
            &format!(
                "Flux Freebuff Cloud indisponible (HTTP {})",
                upstream.status().as_u16()
            ),
            &state.config,
        );
    }
    let stream = upstream
        .bytes_stream()
        .map(|chunk| chunk.map_err(|error| std::io::Error::other(error.to_string())));
    (
        StatusCode::OK,
        [
            (CONTENT_TYPE, HeaderValue::from_static("text/event-stream; charset=utf-8")),
            (CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (reqwest::header::HeaderName::from_static("X-Accel-Buffering"), HeaderValue::from_static("no")),
        ],
        Body::from_stream(stream),
    )
        .into_response()
}

#[derive(Deserialize)]
struct TasksQuery {
    #[serde(default)]
    account: Option<String>,
}

/// Propriétaire de la liste de tâches ciblée par la requête. Un utilisateur de
/// session n'accède qu'à sa propre liste ; le jeton administrateur peut viser
/// un compte précis via `?account=<id|nom-d-utilisateur>` (ex. baptiste.faisy)
/// ou, sans paramètre, la liste technique `server-admin`.
fn api_tasks_owner(
    state: &Arc<ServerState>,
    headers: &HeaderMap,
    query: &TasksQuery,
) -> Result<String, Response> {
    match request_actor(state, headers)? {
        RequestActor::User(identity) => {
            if let Some(account) = query.account.as_deref().filter(|value| !value.is_empty()) {
                if account != identity.id {
                    return Err(api_error(
                        StatusCode::FORBIDDEN,
                        "Un compte utilisateur n'accède qu'à ses propres tâches",
                        &state.config,
                    ));
                }
            }
            Ok(identity.id)
        }
        RequestActor::Administrator => {
            if let Some(account) = query.account.as_deref().filter(|value| !value.is_empty()) {
                match state.auth.user_id_by_username(account) {
                    Ok(Some(user_id)) => Ok(user_id),
                    Ok(None) => Err(api_error(
                        StatusCode::NOT_FOUND,
                        "Compte introuvable pour ses tâches",
                        &state.config,
                    )),
                    Err(error) => Err(api_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &error,
                        &state.config,
                    )),
                }
            } else {
                Ok("server-admin".to_string())
            }
        }
    }
}

async fn api_tasks_list(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Query(query): Query<TasksQuery>,
) -> Response {
    let owner = match api_tasks_owner(&state, &headers, &query) {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    match crate::tasks::list(&owner) {
        Ok(items) => json_response(items),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    }
}

async fn api_tasks_add(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Query(query): Query<TasksQuery>,
    body: Result<Json<serde_json::Value>, JsonRejection>,
) -> Response {
    let owner = match api_tasks_owner(&state, &headers, &query) {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Json(task) = match body {
        Ok(body) => body,
        Err(error) => return creative_json_rejection(&state, error),
    };
    match crate::tasks::add(&owner, task) {
        Ok(items) => json_response(items),
        Err(error) => api_error(StatusCode::BAD_REQUEST, &error, &state.config),
    }
}

async fn api_tasks_replace(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Query(query): Query<TasksQuery>,
    body: Result<Json<serde_json::Value>, JsonRejection>,
) -> Response {
    let owner = match api_tasks_owner(&state, &headers, &query) {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Json(items) = match body {
        Ok(body) => body,
        Err(error) => return creative_json_rejection(&state, error),
    };
    match crate::tasks::replace(&owner, items) {
        Ok(items) => json_response(items),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    }
}

async fn api_tasks_remove(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Query(query): Query<TasksQuery>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let owner = match api_tasks_owner(&state, &headers, &query) {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    match crate::tasks::remove(&owner, &id) {
        Ok(items) => json_response(items),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    }
}

fn telegram_error_status(error: &str) -> StatusCode {
    let normalized = error.to_lowercase();
    if normalized.contains("lecture")
        || normalized.contains("écriture")
        || normalized.contains("sérialisation")
        || normalized.contains("verrou")
    {
        StatusCode::INTERNAL_SERVER_ERROR
    } else if normalized.contains("telegram injoignable")
        || normalized.contains("telegram a refusé")
        || normalized.contains("réponse telegram")
    {
        StatusCode::BAD_GATEWAY
    } else {
        StatusCode::BAD_REQUEST
    }
}

async fn api_telegram_connection(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    let owner_id = match creative_owner_id(&state, &headers) {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    match telegram_notifications::telegram_connection_for_owner(&owner_id) {
        Ok(value) => json_response(value),
        Err(error) => api_error(telegram_error_status(&error), &error, &state.config),
    }
}

async fn api_connect_telegram(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    request: Result<Json<ConnectTelegramRequest>, JsonRejection>,
) -> Response {
    let owner_id = match creative_owner_id(&state, &headers) {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    let Json(request) = match request {
        Ok(request) => request,
        Err(_) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "requête JSON Telegram invalide",
                &state.config,
            )
        }
    };
    match telegram_notifications::connect_telegram_for_owner(&owner_id, request).await {
        Ok(value) => json_response(value),
        Err(error) => api_error(telegram_error_status(&error), &error, &state.config),
    }
}

async fn api_refresh_telegram_pairing(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    let owner_id = match creative_owner_id(&state, &headers) {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    match telegram_notifications::refresh_telegram_pairing_for_owner(&owner_id) {
        Ok(value) => json_response(value),
        Err(error) => api_error(telegram_error_status(&error), &error, &state.config),
    }
}

async fn api_disconnect_telegram(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    let owner_id = match creative_owner_id(&state, &headers) {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    match telegram_notifications::disconnect_telegram_for_owner(&owner_id) {
        Ok(value) => json_response(value),
        Err(error) => api_error(telegram_error_status(&error), &error, &state.config),
    }
}

async fn api_test_telegram(State(state): State<Arc<ServerState>>, headers: HeaderMap) -> Response {
    let owner_id = match creative_owner_id(&state, &headers) {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    match telegram_notifications::test_telegram_for_owner(&owner_id).await {
        Ok(value) => json_response(value),
        Err(error) => api_error(telegram_error_status(&error), &error, &state.config),
    }
}

async fn api_telegram_manager(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    let owner_id = match creative_owner_id(&state, &headers) {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    match telegram_notifications::telegram_manager_for_owner(&owner_id) {
        Ok(value) => json_response(value),
        Err(error) => api_error(telegram_error_status(&error), &error, &state.config),
    }
}

async fn api_connect_telegram_manager(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    request: Result<Json<ConnectTelegramManagerRequest>, JsonRejection>,
) -> Response {
    let owner_id = match creative_owner_id(&state, &headers) {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    let Json(request) = match request {
        Ok(request) => request,
        Err(_) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "requête JSON du bot gestionnaire Telegram invalide",
                &state.config,
            )
        }
    };
    match telegram_notifications::connect_telegram_manager_for_owner(&owner_id, request).await {
        Ok(value) => json_response(value),
        Err(error) => api_error(telegram_error_status(&error), &error, &state.config),
    }
}

async fn api_prepare_managed_telegram_bot(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    request: Result<Json<PrepareManagedTelegramBotRequest>, JsonRejection>,
) -> Response {
    let owner_id = match creative_owner_id(&state, &headers) {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    let Json(request) = match request {
        Ok(request) => request,
        Err(_) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "requête JSON de création Telegram invalide",
                &state.config,
            )
        }
    };
    match telegram_notifications::prepare_managed_telegram_bot_for_owner(&owner_id, request) {
        Ok(value) => json_response(value),
        Err(error) => api_error(telegram_error_status(&error), &error, &state.config),
    }
}

async fn api_disconnect_telegram_manager(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    let owner_id = match creative_owner_id(&state, &headers) {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    match telegram_notifications::disconnect_telegram_manager_for_owner(&owner_id) {
        Ok(value) => json_response(value),
        Err(error) => api_error(telegram_error_status(&error), &error, &state.config),
    }
}

async fn api_mobile_push_status(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = check_maintenance_header(&state, &headers) {
        return response;
    }
    match mobile_push::mobile_push_status() {
        Ok(value) => json_response(value),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    }
}

async fn api_mobile_push_configuration(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = check_maintenance_header(&state, &headers) {
        return response;
    }
    match mobile_push::mobile_push_configuration() {
        Ok(value) => json_response(value),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    }
}

async fn api_configure_mobile_push(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<ConfigureMobilePushRequest>,
) -> Response {
    if let Err(response) = check_maintenance_header(&state, &headers) {
        return response;
    }
    match mobile_push::configure_mobile_push(request) {
        Ok(value) => json_response(value),
        Err(error) => api_error(StatusCode::BAD_REQUEST, &error, &state.config),
    }
}

async fn api_register_mobile_push_device(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<RegisterMobilePushDeviceRequest>,
) -> Response {
    // Le client natif conserve le token administrateur dans Android Keystore.
    // Une session web ordinaire ne peut pas enregistrer silencieusement un
    // appareil qui recevrait les validations financieres de toute la flotte.
    if let Err(response) = check_maintenance_header(&state, &headers) {
        return response;
    }
    match mobile_push::register_mobile_push_device(request) {
        Ok(value) => json_response(value),
        Err(error) => api_error(StatusCode::BAD_REQUEST, &error, &state.config),
    }
}

async fn api_unregister_mobile_push_device(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(device_id): AxumPath<String>,
) -> Response {
    if let Err(response) = check_maintenance_header(&state, &headers) {
        return response;
    }
    match mobile_push::unregister_mobile_push_device(&device_id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, &error, &state.config),
    }
}

async fn api_test_mobile_push(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = check_maintenance_header(&state, &headers) {
        return response;
    }
    match mobile_push::test_mobile_push_configuration().await {
        Ok(value) => json_response(value),
        Err(error) => {
            let status = if error.contains("Aucun appareil") {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::BAD_GATEWAY
            };
            api_error(status, &error, &state.config)
        }
    }
}

fn whatsapp_error_status(error: &str) -> StatusCode {
    let normalized = error.to_lowercase();
    if normalized.contains("lecture")
        || normalized.contains("écriture")
        || normalized.contains("sérialisation")
        || normalized.contains("verrou")
    {
        StatusCode::INTERNAL_SERVER_ERROR
    } else if normalized.contains("meta")
        || normalized.contains("whatsapp impossible")
        || normalized.contains("whatsapp refusé")
        || normalized.contains("whatsapp refusée")
        || normalized.contains("réponse")
    {
        StatusCode::BAD_GATEWAY
    } else {
        StatusCode::BAD_REQUEST
    }
}

async fn api_whatsapp_connection(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    let owner_id = match creative_owner_id(&state, &headers) {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    match whatsapp_notifications::whatsapp_connection_for_owner(&owner_id) {
        Ok(value) => json_response(whatsapp_notifications::with_webhook_callback_url(
            value,
            &state.config.public_base_url,
        )),
        Err(error) => api_error(whatsapp_error_status(&error), &error, &state.config),
    }
}

async fn api_connect_whatsapp(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    request: Result<Json<ConnectWhatsAppRequest>, JsonRejection>,
) -> Response {
    let owner_id = match creative_owner_id(&state, &headers) {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    let Json(request) = match request {
        Ok(request) => request,
        Err(_) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "requête JSON WhatsApp invalide",
                &state.config,
            )
        }
    };
    match whatsapp_notifications::connect_whatsapp_for_owner(&owner_id, request).await {
        Ok(value) => json_response(whatsapp_notifications::with_webhook_callback_url(
            value,
            &state.config.public_base_url,
        )),
        Err(error) => api_error(whatsapp_error_status(&error), &error, &state.config),
    }
}

async fn api_disconnect_whatsapp(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    let owner_id = match creative_owner_id(&state, &headers) {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    match whatsapp_notifications::disconnect_whatsapp_for_owner(&owner_id) {
        Ok(value) => json_response(value),
        Err(error) => api_error(whatsapp_error_status(&error), &error, &state.config),
    }
}

async fn api_test_whatsapp(State(state): State<Arc<ServerState>>, headers: HeaderMap) -> Response {
    let owner_id = match creative_owner_id(&state, &headers) {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    match whatsapp_notifications::test_whatsapp_for_owner(&owner_id).await {
        Ok(value) => json_response(value),
        Err(error) => api_error(whatsapp_error_status(&error), &error, &state.config),
    }
}

#[derive(Debug, Deserialize)]
struct WhatsAppWebhookVerificationQuery {
    #[serde(rename = "hub.mode")]
    mode: Option<String>,
    #[serde(rename = "hub.verify_token")]
    verify_token: Option<String>,
    #[serde(rename = "hub.challenge")]
    challenge: Option<String>,
}

async fn api_verify_whatsapp_webhook(
    State(state): State<Arc<ServerState>>,
    Query(query): Query<WhatsAppWebhookVerificationQuery>,
) -> Response {
    let result = whatsapp_notifications::verify_webhook_challenge(
        query.mode.as_deref().unwrap_or_default(),
        query.verify_token.as_deref().unwrap_or_default(),
        query.challenge.as_deref().unwrap_or_default(),
    );
    match result {
        Ok(challenge) => (StatusCode::OK, challenge).into_response(),
        Err(error) => {
            let status = if error.is_internal() {
                StatusCode::INTERNAL_SERVER_ERROR
            } else if error.is_unauthorized() {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::BAD_REQUEST
            };
            api_error(status, error.message(), &state.config)
        }
    }
}

async fn api_receive_whatsapp_webhook(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let signature = headers
        .get("x-hub-signature-256")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let messages =
        match whatsapp_notifications::verify_and_extract_webhook_messages(signature, body.as_ref())
        {
            Ok(messages) => messages,
            Err(error) => {
                let status = if error.is_internal() {
                    StatusCode::INTERNAL_SERVER_ERROR
                } else if error.is_unauthorized() {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::BAD_REQUEST
                };
                return api_error(status, error.message(), &state.config);
            }
        };

    for message in messages {
        let manager = state.autonomous.clone();
        tokio::spawn(async move {
            let channel_id = message.channel_id.clone();
            let reply_to_message_id = message.reply_to_message_id.clone();
            let content = message.content.clone();
            let message_id = message.message_id.clone();
            let routed = tokio::task::spawn_blocking(move || {
                manager.receive_whatsapp_message(
                    &channel_id,
                    reply_to_message_id.as_deref(),
                    &content,
                )
            })
            .await;
            let (reply, agent_target) = match routed {
                Ok(Ok(dispatch)) => dispatch,
                Ok(Err(error)) => {
                    eprintln!(
                        "[whatsapp] message entrant {message_id} non transmis à l’agent : {error}"
                    );
                    (
                        "Je n’ai pas pu transmettre ce message à l’agent. Vérifie son état dans Codex Switch Terminal."
                            .to_string(),
                        None,
                    )
                }
                Err(error) => {
                    eprintln!(
                        "[whatsapp] traitement du message entrant {message_id} interrompu : {error}"
                    );
                    (
                        "Le traitement du message a été interrompu. Réessaie dans un instant."
                            .to_string(),
                        None,
                    )
                }
            };
            match whatsapp_notifications::send_conversation_reply(&message.channel_id, &reply).await
            {
                Ok(sent) => {
                    if let Some((agent_id, agent_name)) = agent_target {
                        if let Err(error) = whatsapp_notifications::record_conversation_reply_target(
                            &message.channel_id,
                            sent.message_id,
                            agent_id,
                            agent_name,
                        ) {
                            eprintln!(
                                "[whatsapp] continuité de conversation non persistée : {error}"
                            );
                        }
                    }
                }
                Err(error) => {
                    eprintln!(
                        "[whatsapp] réponse au message entrant {} non envoyée : {error}",
                        message.message_id
                    );
                }
            }
        });
    }

    (StatusCode::OK, "EVENT_RECEIVED").into_response()
}

async fn api_image_generation_capabilities(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    let owner_id = match creative_owner_id(&state, &headers) {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    match image_generation::image_generation_capabilities_for(&owner_id) {
        Ok(value) => json_response(value),
        Err(error) => api_error(
            creative_generation_error_status(&error),
            &error,
            &state.config,
        ),
    }
}

async fn api_start_image_generation(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<ImageGenerationRequest>,
) -> Response {
    let owner_id = match creative_owner_id(&state, &headers) {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    match image_generation::start_image_generation_for(&owner_id, request).await {
        Ok(value) => json_response(value),
        Err(error) => api_error(
            creative_generation_error_status(&error),
            &error,
            &state.config,
        ),
    }
}

async fn api_image_generation_status(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<ImageGenerationStatusRequest>,
) -> Response {
    let owner_id = match creative_owner_id(&state, &headers) {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    match image_generation::image_generation_status_for(&owner_id, request).await {
        Ok(value) => json_response(value),
        Err(error) => api_error(
            creative_generation_error_status(&error),
            &error,
            &state.config,
        ),
    }
}

async fn api_cancel_image_generation(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<ImageGenerationStatusRequest>,
) -> Response {
    let owner_id = match creative_owner_id(&state, &headers) {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    match image_generation::cancel_image_generation_for(&owner_id, request).await {
        Ok(value) => json_response(value),
        Err(error) => api_error(
            creative_generation_error_status(&error),
            &error,
            &state.config,
        ),
    }
}

async fn api_video_generation_capabilities(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    let owner_id = match creative_owner_id(&state, &headers) {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    match video_generation::video_generation_capabilities_for(&owner_id) {
        Ok(value) => json_response(value),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    }
}

async fn api_start_video_generation(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    request: Result<Json<VideoGenerationRequest>, JsonRejection>,
) -> Response {
    let owner_id = match creative_owner_id(&state, &headers) {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    let Json(request) = match request {
        Ok(request) => request,
        Err(error) => return creative_json_rejection(&state, error),
    };
    match video_generation::start_video_generation_for(&owner_id, request).await {
        Ok(value) => json_response(value),
        Err(error) => api_error(
            creative_generation_error_status(&error),
            &error,
            &state.config,
        ),
    }
}

async fn api_video_generation_status(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    request: Result<Json<VideoGenerationStatusRequest>, JsonRejection>,
) -> Response {
    let owner_id = match creative_owner_id(&state, &headers) {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    let Json(request) = match request {
        Ok(request) => request,
        Err(error) => return creative_json_rejection(&state, error),
    };
    match video_generation::video_generation_status_for(&owner_id, request).await {
        Ok(value) => json_response(value),
        Err(error) => api_error(
            creative_generation_error_status(&error),
            &error,
            &state.config,
        ),
    }
}

async fn api_cancel_video_generation(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    request: Result<Json<VideoGenerationStatusRequest>, JsonRejection>,
) -> Response {
    let owner_id = match creative_owner_id(&state, &headers) {
        Ok(owner_id) => owner_id,
        Err(response) => return response,
    };
    let Json(request) = match request {
        Ok(request) => request,
        Err(error) => return creative_json_rejection(&state, error),
    };
    match video_generation::cancel_video_generation_for(&owner_id, request).await {
        Ok(value) => json_response(value),
        Err(error) => api_error(
            creative_generation_error_status(&error),
            &error,
            &state.config,
        ),
    }
}

async fn api_doctolib_lab_status(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }
    match doctolib_lab::status().await {
        Ok(value) => json_response(value),
        Err(error) => api_error(StatusCode::BAD_GATEWAY, &error, &state.config),
    }
}

async fn api_doctolib_lab_connect(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }
    match doctolib_lab::connect().await {
        Ok(value) => json_response(value),
        Err(error) => api_error(StatusCode::BAD_GATEWAY, &error, &state.config),
    }
}

async fn api_doctolib_lab_google_calendar_connect(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }
    match doctolib_lab::connect_google_calendar().await {
        Ok(value) => json_response(value),
        Err(error) => api_error(StatusCode::BAD_GATEWAY, &error, &state.config),
    }
}

async fn api_doctolib_lab_search(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<DoctolibLabSearchRequest>,
) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }
    match doctolib_lab::search(state.doctolib_lab.as_ref(), request).await {
        Ok(value) => json_response(value),
        Err(error) => api_error(StatusCode::BAD_GATEWAY, &error, &state.config),
    }
}

async fn api_doctolib_lab_confirm(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<DoctolibLabConfirmRequest>,
) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }
    match doctolib_lab::confirm(
        state.doctolib_lab.as_ref(),
        request.proposal_id,
        request.add_to_google_calendar,
    )
    .await
    {
        Ok(value) => json_response(value),
        Err(error) => api_error(StatusCode::BAD_REQUEST, &error, &state.config),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatModelsQuery {
    account_id: String,
}

async fn api_chat_models(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Query(query): Query<ChatModelsQuery>,
) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }
    match settings::load_account_model_catalog(&query.account_id).await {
        Ok(value) => json_response(value),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    }
}

async fn api_chat_turn_status(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<u64>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Some(identity) = actor.user() {
        match state.chat.is_visible_to(id, &identity.id, |project_dir| {
            chat_workspace_is_visible(&state, identity, project_dir)
        }) {
            Ok(true) => {}
            Ok(false) => {
                return api_error(
                    StatusCode::NOT_FOUND,
                    "Tour de conversation introuvable ou inaccessible",
                    &state.config,
                )
            }
            Err(error) => {
                return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config)
            }
        }
    }
    match state.chat.status(id) {
        Ok(value) => json_response(value),
        Err(error) => api_error(resource_error_status(&error), &error, &state.config),
    }
}

/// Un chat reste visible tant que son environnement l'est. C'est ce qui donne
/// le meme statut d'execution sur tous les appareils d'un utilisateur, y compris
/// pour les tours lances au jeton administrateur, par un agent autonome ou par
/// une orchestration, qui n'ont aucun proprietaire nominatif.
fn chat_workspace_is_visible(
    state: &Arc<ServerState>,
    identity: &AuthIdentity,
    project_dir: &str,
) -> bool {
    state
        .workspace_access
        .authorize_existing_environment(identity, project_dir)
        .is_ok()
}

async fn api_list_active_chat_turns(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let result = match actor {
        RequestActor::Administrator => state.chat.active(),
        RequestActor::User(identity) => {
            // Plusieurs tours partagent le meme dossier et l'autorisation touche
            // le disque : on ne la resout qu'une fois par environnement.
            let mut authorized = HashMap::<String, bool>::new();
            state.chat.active_visible_to(&identity.id, |project_dir| {
                *authorized
                    .entry(project_dir.to_string())
                    .or_insert_with(|| chat_workspace_is_visible(&state, &identity, project_dir))
            })
        }
    };
    match result {
        Ok(value) => json_response(value),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    }
}

async fn api_claim_chat_open_requests(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    auth_or(&state, &headers, || {
        state.chat_open_requests.claim().map(json_response)
    })
}

async fn api_claim_chat_post_requests(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    auth_or(&state, &headers, || {
        state.chat_post_requests.claim().map(json_response)
    })
}

async fn api_compact_chat_session(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<CompactChatSessionRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if is_draining(&state) {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "noeud en drain: compaction refusee",
            &state.config,
        );
    }
    let manager = state.chat.clone();
    let account_id = request.account_id;
    let session_id = request.session_id;
    if let Some(identity) = actor.user() {
        if let Err(response) =
            authorize_discussion_for_identity(&state, identity, &account_id, &session_id)
        {
            return response;
        }
    }
    match tokio::task::spawn_blocking(move || manager.compact(account_id, session_id)).await {
        Ok(Ok(value)) => json_response(value),
        Ok(Err(error)) => api_error(resource_error_status(&error), &error, &state.config),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("compaction interrompue: {error}"),
            &state.config,
        ),
    }
}

async fn api_stop_chat_turn(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<u64>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Some(identity) = actor.user() {
        match state.chat.is_visible_to(id, &identity.id, |project_dir| {
            chat_workspace_is_visible(&state, identity, project_dir)
        }) {
            Ok(true) => {}
            Ok(false) => {
                return api_error(
                    StatusCode::NOT_FOUND,
                    "Tour de conversation introuvable ou inaccessible",
                    &state.config,
                )
            }
            Err(error) => {
                return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config)
            }
        }
    }
    match state.chat.stop(id) {
        Ok(value) => json_response(value),
        Err(error) => api_error(resource_error_status(&error), &error, &state.config),
    }
}

fn resolve_actor_environment(
    state: &Arc<ServerState>,
    actor: &RequestActor,
    raw: &str,
    claim_personal: bool,
) -> Result<PathBuf, Response> {
    match actor.user() {
        Some(identity) if claim_personal => state
            .workspace_access
            .claim_or_authorize_environment(identity, raw, None)
            .map_err(|error| workspace_access_error(state, error)),
        Some(identity) => state
            .workspace_access
            .authorize_existing_environment(identity, raw)
            .map_err(|error| workspace_access_error(state, error)),
        None => resolve_within_root(&state.config.workspaces_root, raw)
            .map_err(|error| api_error(StatusCode::BAD_REQUEST, &error, &state.config)),
    }
}

fn authorize_autonomous_resource(
    state: &Arc<ServerState>,
    actor: &RequestActor,
    id: &str,
) -> Result<(), Response> {
    let Some(identity) = actor.user() else {
        return Ok(());
    };
    let agents = state
        .autonomous
        .list()
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config))?;
    let agent = agents.iter().find(|agent| agent.id == id).ok_or_else(|| {
        api_error(
            StatusCode::NOT_FOUND,
            "Agent autonome introuvable ou inaccessible",
            &state.config,
        )
    })?;
    let project_dir = agent.project_dir.as_deref().ok_or_else(|| {
        api_error(
            StatusCode::NOT_FOUND,
            "Agent autonome introuvable ou inaccessible",
            &state.config,
        )
    })?;
    state
        .workspace_access
        .authorize_existing_environment(identity, project_dir)
        .map(|_| ())
        .map_err(|_| {
            api_error(
                StatusCode::NOT_FOUND,
                "Agent autonome introuvable ou inaccessible",
                &state.config,
            )
        })
}

fn authorize_orchestration_resource(
    state: &Arc<ServerState>,
    actor: &RequestActor,
    id: &str,
) -> Result<(), Response> {
    let Some(identity) = actor.user() else {
        return Ok(());
    };
    let runs = state
        .orchestration
        .list()
        .map_err(|error| api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config))?;
    let run = runs.iter().find(|run| run.id == id).ok_or_else(|| {
        api_error(
            StatusCode::NOT_FOUND,
            "Orchestration introuvable ou inaccessible",
            &state.config,
        )
    })?;
    state
        .workspace_access
        .authorize_existing_environment(identity, &run.project_dir)
        .map(|_| ())
        .map_err(|_| {
            api_error(
                StatusCode::NOT_FOUND,
                "Orchestration introuvable ou inaccessible",
                &state.config,
            )
        })
}

async fn api_list_autonomous_agents(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let mut agents = match state.autonomous.list() {
        Ok(value) => value,
        Err(error) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    };
    if let Some(identity) = actor.user() {
        agents.retain(|agent| {
            agent.project_dir.as_deref().is_some_and(|project_dir| {
                state
                    .workspace_access
                    .authorize_existing_environment(identity, project_dir)
                    .is_ok()
            })
        });
    }
    json_response(agents)
}

async fn api_create_autonomous_agent(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(mut request): Json<CreateAutonomousAgentRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if is_draining(&state) {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "noeud en drain: nouveaux agents autonomes refuses",
            &state.config,
        );
    }
    if request
        .whatsapp_notification_channel_id
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        let owner_id = match creative_owner_id(&state, &headers) {
            Ok(owner_id) => owner_id,
            Err(response) => return response,
        };
        request.whatsapp_notification_channel_id =
            match whatsapp_notifications::validate_channel_for_owner(
                &owner_id,
                request.whatsapp_notification_channel_id.as_deref(),
            ) {
                Ok(channel_id) => channel_id,
                Err(error) => return api_error(StatusCode::BAD_REQUEST, &error, &state.config),
            };
    } else {
        request.whatsapp_notification_channel_id = None;
    }
    if request
        .telegram_notification_channel_id
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        let owner_id = match creative_owner_id(&state, &headers) {
            Ok(owner_id) => owner_id,
            Err(response) => return response,
        };
        request.telegram_notification_channel_id =
            match telegram_notifications::validate_channel_for_owner(
                &owner_id,
                request.telegram_notification_channel_id.as_deref(),
            ) {
                Ok(channel_id) => channel_id,
                Err(error) => return api_error(StatusCode::BAD_REQUEST, &error, &state.config),
            };
    } else {
        request.telegram_notification_channel_id = None;
    }
    if let Some(raw) = request
        .project_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let resolved = match resolve_actor_environment(&state, &actor, raw, true) {
            Ok(path) => path,
            Err(response) => return response,
        };
        request.project_dir = Some(display_path(&resolved));
    } else if actor.user().is_some() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "Un environnement personnel ou partage est obligatoire pour un agent autonome",
            &state.config,
        );
    }

    // Impose depuis la session, jamais lu depuis le corps de la requete : c'est
    // ce qui autorisera plus tard l'agent a lire la boite mail de cette
    // personne, et d'aucune autre.
    request.owner_id = actor.user().map(|identity| identity.id.clone());

    let manager = state.autonomous.clone();
    match tokio::task::spawn_blocking(move || manager.create(request)).await {
        Ok(Ok(value)) => json_response(value),
        Ok(Err(error)) => api_error(agent_start_status(&error), &error, &state.config),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("creation de l'agent autonome interrompue: {error}"),
            &state.config,
        ),
    }
}

async fn api_update_autonomous_agent(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(mut request): Json<UpdateAutonomousAgentRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Err(response) = authorize_autonomous_resource(&state, &actor, &id) {
        return response;
    }
    if request
        .whatsapp_notification_channel_id
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        let owner_id = match creative_owner_id(&state, &headers) {
            Ok(owner_id) => owner_id,
            Err(response) => return response,
        };
        request.whatsapp_notification_channel_id =
            match whatsapp_notifications::validate_channel_for_owner(
                &owner_id,
                request.whatsapp_notification_channel_id.as_deref(),
            ) {
                Ok(channel_id) => channel_id,
                Err(error) => return api_error(StatusCode::BAD_REQUEST, &error, &state.config),
            };
    }
    if request
        .telegram_notification_channel_id
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        let owner_id = match creative_owner_id(&state, &headers) {
            Ok(owner_id) => owner_id,
            Err(response) => return response,
        };
        request.telegram_notification_channel_id =
            match telegram_notifications::validate_channel_for_owner(
                &owner_id,
                request.telegram_notification_channel_id.as_deref(),
            ) {
                Ok(channel_id) => channel_id,
                Err(error) => return api_error(StatusCode::BAD_REQUEST, &error, &state.config),
            };
    }
    if let Some(raw) = request
        .project_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let resolved = match resolve_actor_environment(&state, &actor, raw, true) {
            Ok(path) => path,
            Err(response) => return response,
        };
        request.project_dir = Some(display_path(&resolved));
    } else if actor.user().is_some() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "L'environnement autorise de l'agent ne peut pas etre retire",
            &state.config,
        );
    }

    let manager = state.autonomous.clone();
    match tokio::task::spawn_blocking(move || manager.update(&id, request)).await {
        Ok(Ok(value)) => json_response(value),
        Ok(Err(error)) => api_error(StatusCode::BAD_REQUEST, &error, &state.config),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("mise a jour de l'agent autonome interrompue: {error}"),
            &state.config,
        ),
    }
}

async fn api_control_autonomous_agent(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<ControlAutonomousAgentRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Err(response) = authorize_autonomous_resource(&state, &actor, &id) {
        return response;
    }
    let manager = state.autonomous.clone();
    match tokio::task::spawn_blocking(move || {
        manager.control(&id, request.action, request.payment_id.as_deref())
    })
    .await
    {
        Ok(Ok(value)) => json_response(value),
        Ok(Err(error)) => api_error(StatusCode::BAD_REQUEST, &error, &state.config),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("controle de l'agent autonome interrompu: {error}"),
            &state.config,
        ),
    }
}

async fn api_send_autonomous_agent_message(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<SendAutonomousAgentMessageRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Err(response) = authorize_autonomous_resource(&state, &actor, &id) {
        return response;
    }
    let manager = state.autonomous.clone();
    match tokio::task::spawn_blocking(move || manager.send_message(&id, request)).await {
        Ok(Ok(value)) => json_response(value),
        Ok(Err(error)) => api_error(StatusCode::BAD_REQUEST, &error, &state.config),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("envoi du message autonome interrompu: {error}"),
            &state.config,
        ),
    }
}

async fn api_apply_autonomous_review_policy(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<ApplyAutonomousReviewPolicyRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Err(response) = authorize_autonomous_resource(&state, &actor, &id) {
        return response;
    }
    let manager = state.autonomous.clone();
    match tokio::task::spawn_blocking(move || {
        manager.apply_review_policy(
            &id,
            &request.instruction,
            request.require_visual_evidence,
            request.activate,
        )
    })
    .await
    {
        Ok(Ok(value)) => json_response(value),
        Ok(Err(error)) => api_error(StatusCode::BAD_REQUEST, &error, &state.config),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("application de la politique de review interrompue: {error}"),
            &state.config,
        ),
    }
}

async fn api_read_autonomous_review_evidence(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath((id, review_id)): AxumPath<(String, String)>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Err(response) = authorize_autonomous_resource(&state, &actor, &id) {
        return response;
    }
    let manager = state.autonomous.clone();
    match tokio::task::spawn_blocking(move || manager.review_evidence(&id, &review_id)).await {
        Ok(Ok(value)) => json_response(value),
        Ok(Err(error)) => api_error(StatusCode::BAD_REQUEST, &error, &state.config),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("lecture de la preuve visuelle interrompue: {error}"),
            &state.config,
        ),
    }
}

async fn api_schedule_autonomous_agent(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<ScheduleAutonomousAgentRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Err(response) = authorize_autonomous_resource(&state, &actor, &id) {
        return response;
    }
    let manager = state.autonomous.clone();
    match tokio::task::spawn_blocking(move || {
        manager.schedule(&id, request.next_run_at, request.interval_seconds)
    })
    .await
    {
        Ok(Ok(value)) => json_response(value),
        Ok(Err(error)) => api_error(StatusCode::BAD_REQUEST, &error, &state.config),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("replanification de l'agent autonome interrompue: {error}"),
            &state.config,
        ),
    }
}

async fn api_reassign_autonomous_agent_account(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<ReassignAutonomousAgentAccountRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Err(response) = authorize_autonomous_resource(&state, &actor, &id) {
        return response;
    }
    let manager = state.autonomous.clone();
    match tokio::task::spawn_blocking(move || manager.reassign_account(&id, request)).await {
        Ok(Ok(value)) => json_response(value),
        Ok(Err(error)) => api_error(StatusCode::BAD_REQUEST, &error, &state.config),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("réaffectation du compte autonome interrompue: {error}"),
            &state.config,
        ),
    }
}

async fn api_add_autonomous_agent_memory(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<AddAutonomousMemoryRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Err(response) = authorize_autonomous_resource(&state, &actor, &id) {
        return response;
    }
    let manager = state.autonomous.clone();
    match tokio::task::spawn_blocking(move || manager.add_memory(&id, &request.content)).await {
        Ok(Ok(value)) => json_response(value),
        Ok(Err(error)) => api_error(StatusCode::BAD_REQUEST, &error, &state.config),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("ajout de memoire autonome interrompu: {error}"),
            &state.config,
        ),
    }
}

async fn api_mark_autonomous_agent_report_read(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath((id, report_id)): AxumPath<(String, String)>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Err(response) = authorize_autonomous_resource(&state, &actor, &id) {
        return response;
    }
    let manager = state.autonomous.clone();
    match tokio::task::spawn_blocking(move || manager.mark_report_read(&id, &report_id)).await {
        Ok(Ok(value)) => json_response(value),
        Ok(Err(error)) => api_error(StatusCode::BAD_REQUEST, &error, &state.config),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("lecture du compte rendu autonome interrompue: {error}"),
            &state.config,
        ),
    }
}

async fn api_delete_autonomous_agent_memory(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath((id, memory_id)): AxumPath<(String, String)>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Err(response) = authorize_autonomous_resource(&state, &actor, &id) {
        return response;
    }
    let manager = state.autonomous.clone();
    match tokio::task::spawn_blocking(move || manager.delete_memory(&id, &memory_id)).await {
        Ok(Ok(value)) => json_response(value),
        Ok(Err(error)) => api_error(StatusCode::BAD_REQUEST, &error, &state.config),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("suppression de memoire autonome interrompue: {error}"),
            &state.config,
        ),
    }
}

async fn api_delete_autonomous_agent(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Err(response) = authorize_autonomous_resource(&state, &actor, &id) {
        return response;
    }
    let manager = state.autonomous.clone();
    match tokio::task::spawn_blocking(move || manager.delete(&id)).await {
        Ok(Ok(())) => json_response(json!({ "ok": true })),
        Ok(Err(error)) => api_error(StatusCode::BAD_REQUEST, &error, &state.config),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("suppression de l'agent autonome interrompue: {error}"),
            &state.config,
        ),
    }
}

async fn api_promote_autonomous_agent_to_orchestration(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(mut request): Json<PromoteAutonomousAgentRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Err(response) = authorize_autonomous_resource(&state, &actor, &id) {
        return response;
    }
    if is_draining(&state) {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "noeud en drain: promotion vers une orchestration refusée",
            &state.config,
        );
    }
    let resolved = match resolve_actor_environment(&state, &actor, request.project_dir.trim(), true)
    {
        Ok(path) => path,
        Err(response) => return response,
    };
    request.project_dir = display_path(&resolved);
    let orchestration = state.orchestration.clone();
    let autonomous = state.autonomous.clone();
    match tokio::task::spawn_blocking(move || {
        orchestration.promote_autonomous_agent(&autonomous, &id, request)
    })
    .await
    {
        Ok(Ok(value)) => json_response(value),
        Ok(Err(error)) => api_error(agent_start_status(&error), &error, &state.config),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("promotion de l'agent autonome interrompue: {error}"),
            &state.config,
        ),
    }
}

async fn api_list_orchestrations(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let mut runs = match state.orchestration.list() {
        Ok(value) => value,
        Err(error) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    };
    if let Some(identity) = actor.user() {
        runs.retain(|run| {
            state
                .workspace_access
                .authorize_existing_environment(identity, &run.project_dir)
                .is_ok()
        });
    }
    json_response(runs)
}

async fn api_create_orchestration(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(mut request): Json<CreateOrchestrationRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if is_draining(&state) {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "noeud en drain: nouveaux chats orchestres refuses",
            &state.config,
        );
    }
    let resolved = match resolve_actor_environment(&state, &actor, request.project_dir.trim(), true)
    {
        Ok(path) => path,
        Err(response) => return response,
    };
    request.project_dir = display_path(&resolved);
    let manager = state.orchestration.clone();
    match tokio::task::spawn_blocking(move || manager.create(request)).await {
        Ok(Ok(value)) => json_response(value),
        Ok(Err(error)) => api_error(agent_start_status(&error), &error, &state.config),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("creation du chat orchestre interrompue: {error}"),
            &state.config,
        ),
    }
}

async fn api_control_orchestration(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<ControlOrchestrationRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Err(response) = authorize_orchestration_resource(&state, &actor, &id) {
        return response;
    }
    let manager = state.orchestration.clone();
    match tokio::task::spawn_blocking(move || manager.control(&id, request.action)).await {
        Ok(Ok(value)) => json_response(value),
        Ok(Err(error)) => api_error(StatusCode::BAD_REQUEST, &error, &state.config),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("controle du chat orchestre interrompu: {error}"),
            &state.config,
        ),
    }
}

async fn api_reassign_orchestration_account(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<ReassignOrchestrationAccountRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Err(response) = authorize_orchestration_resource(&state, &actor, &id) {
        return response;
    }
    let manager = state.orchestration.clone();
    match tokio::task::spawn_blocking(move || manager.reassign_account(&id, request)).await {
        Ok(Ok(value)) => json_response(value),
        Ok(Err(error)) => api_error(StatusCode::BAD_REQUEST, &error, &state.config),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("reprise du compte orchestre interrompue: {error}"),
            &state.config,
        ),
    }
}

async fn api_delete_orchestration(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Err(response) = authorize_orchestration_resource(&state, &actor, &id) {
        return response;
    }
    let manager = state.orchestration.clone();
    match tokio::task::spawn_blocking(move || manager.delete(&id)).await {
        Ok(Ok(())) => json_response(json!({ "ok": true })),
        Ok(Err(error)) => api_error(StatusCode::BAD_REQUEST, &error, &state.config),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("suppression du chat orchestre interrompue: {error}"),
            &state.config,
        ),
    }
}

async fn api_write_terminal(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<u64>,
    Json(request): Json<WriteTerminalRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    match state.terminals.write_for_actor(id, request.data, &actor) {
        Ok(()) => json_response(json!({ "ok": true })),
        Err(error) => api_error(resource_error_status(&error), &error, &state.config),
    }
}

/// Les terminaux Freebuff demarres directement par Freebuff Desktop (home occupe
/// sur le poste) n'appartiennent pas au `RemoteTerminalManager` de Switch.
/// On les synchronise ici en presence (pas de PTY pilotable) pour que la liste
/// des terminaux ouverts les affiche, avec l'historique et la reprise vers un
/// autre compte.
fn external_freebuff_terminal_summaries(
    switch_terminal_account_ids: &std::collections::HashSet<String>,
) -> Vec<RemoteTerminalSummary> {
    let Ok(settings) = settings::load_settings_for_terminal() else {
        return Vec::new();
    };
    let mut summaries = Vec::new();
    let now_unix = crate::metrics::now_ts();
    for account in &settings.accounts {
        if account.provider != Provider::Freebuff {
            continue;
        }
        if switch_terminal_account_ids.contains(&account.id) {
            continue;
        }
        let Ok(home) = settings::expand_home(&account.codex_home) else { continue };
        if !crate::provider::freebuff_instance_busy(&home) {
            continue;
        }
        summaries.push(RemoteTerminalSummary {
            id: external_freebuff_terminal_id(&account.id),
            account_id: account.id.clone(),
            account_label: account.label.clone(),
            agent_id: Some("freebuff".to_string()),
            source_terminal_key: Some(format!("freebuff-external-{}", account.id)),
            workspace_id: workspace_id_for_dir(&home),
            workspace_path: home.to_string_lossy().to_string(),
            started_at: now_unix,
            login_only: false,
            external: true,
        });
    }
    summaries
}

/// Identifiant stable, sans collision avec les PTY Switch (qui commencent a 1)
/// : la partie haute du mot et une valeur derivee du compte garantissent un id
/// unique et deterministe pour la synchronisation de presence.
fn external_freebuff_terminal_id(account_id: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    account_id.hash(&mut hasher);
    // Masque 32 bits conserve 63 bits disponibles d'un u64 non signe.
    1u64 << 62 | (hasher.finish() >> 32 & 0x0000_0000_F000_0000) | (hasher.finish() & 0x00FF_FFFF)
}

async fn api_list_active_terminals(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    match state.terminals.active_for_actor(&actor) {
        Ok(mut value) => {
            // Merge des instances Freebuff Desktop externes du poste. Chaque
            // compte occupe par Freebuff apparait comme un terminal externe
            // synchronise (pas un PTY Switch), sauf s'il est deja serveur par
            // un terminal que Switch pilote lui-meme.
            let switch_owned = state.terminals.unavailable_account_ids();
            value.extend(external_freebuff_terminal_summaries(&switch_owned));
            value.sort_by_key(|summary| (summary.started_at, summary.id));
            json_response(value)
        }
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    }
}

async fn api_resize_terminal(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<u64>,
    Json(request): Json<ResizeTerminalRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    match state
        .terminals
        .resize_for_actor(id, request.cols, request.rows, &actor)
    {
        Ok(()) => json_response(json!({ "ok": true })),
        Err(error) => api_error(resource_error_status(&error), &error, &state.config),
    }
}

async fn api_stop_terminal(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<u64>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    match state.terminals.stop_for_actor(id, &actor) {
        Ok(()) => json_response(json!({ "ok": true })),
        Err(error) => api_error(resource_error_status(&error), &error, &state.config),
    }
}

async fn api_kombai_status(State(state): State<Arc<ServerState>>, headers: HeaderMap) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let owner = match state.kombai_owner.lock() {
        Ok(owner) => owner.clone(),
        Err(_) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Proprietaire Kombai verrouille",
                &state.config,
            )
        }
    };
    if !actor.is_administrator() && owner.as_deref().is_some_and(|id| id != actor.owner_id()) {
        return api_error(
            StatusCode::NOT_FOUND,
            "Espace Kombai introuvable ou inaccessible",
            &state.config,
        );
    }
    match state.kombai.status() {
        Ok(status) => json_response(server_kombai_status(&state.config, status)),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    }
}

async fn api_kombai_start(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(mut request): Json<KombaiStartRequest>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if let Some(raw) = request
        .project_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let resolved = match resolve_actor_environment(&state, &actor, raw, true) {
            Ok(path) => path,
            Err(response) => return response,
        };
        request.project_dir = Some(display_path(&resolved));
    } else if actor.user().is_some() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "Un environnement autorise est obligatoire pour Kombai",
            &state.config,
        );
    }

    let owner_id = actor.owner_id().to_string();
    let newly_claimed = {
        let mut owner = match state.kombai_owner.lock() {
            Ok(owner) => owner,
            Err(_) => {
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Proprietaire Kombai verrouille",
                    &state.config,
                )
            }
        };
        if owner
            .as_deref()
            .is_some_and(|current| current != owner_id && !actor.is_administrator())
        {
            return api_error(
                StatusCode::CONFLICT,
                "Kombai est deja utilise dans l'espace d'un autre compte",
                &state.config,
            );
        }
        if owner.is_none() {
            *owner = Some(owner_id.clone());
            true
        } else {
            false
        }
    };

    match state.kombai.start(request.project_dir).await {
        Ok(status) => json_response(server_kombai_status(&state.config, status)),
        Err(error) => {
            if newly_claimed {
                if let Ok(mut owner) = state.kombai_owner.lock() {
                    if owner.as_deref() == Some(owner_id.as_str()) {
                        *owner = None;
                    }
                }
            }
            api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config)
        }
    }
}

async fn api_kombai_stop(State(state): State<Arc<ServerState>>, headers: HeaderMap) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let allowed = state
        .kombai_owner
        .lock()
        .map(|owner| {
            actor.is_administrator()
                || owner.is_none()
                || owner.as_deref() == Some(actor.owner_id())
        })
        .unwrap_or(false);
    if !allowed {
        return api_error(
            StatusCode::NOT_FOUND,
            "Espace Kombai introuvable ou inaccessible",
            &state.config,
        );
    }
    match state.kombai.stop() {
        Ok(status) => {
            if let Ok(mut owner) = state.kombai_owner.lock() {
                *owner = None;
            }
            json_response(server_kombai_status(&state.config, status))
        }
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    }
}

async fn api_kombai_install_extension(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = check_admin_header(&state, &headers) {
        return response;
    }

    match state.kombai.install_extension().await {
        Ok(status) => json_response(server_kombai_status(&state.config, status)),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    }
}

async fn api_workspaces(State(state): State<Arc<ServerState>>, headers: HeaderMap) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    match actor {
        RequestActor::Administrator => match list_workspaces(&state.config.data_dir) {
            Ok(value) => json_response(value),
            Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
        },
        RequestActor::User(identity) => match state.workspace_access.list_for(&identity) {
            Ok(value) => json_response(value),
            Err(error) => workspace_access_error(&state, error),
        },
    }
}

async fn api_create_workspace(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<CreateWorkspaceRequest>,
) -> Response {
    let identity = match require_user_actor(&state, &headers) {
        Ok(identity) => identity,
        Err(response) => return response,
    };
    match state
        .workspace_access
        .create_environment(&identity, &request.name)
    {
        Ok(value) => (StatusCode::CREATED, Json(value)).into_response(),
        Err(error) => workspace_access_error(&state, error),
    }
}

async fn api_workspace_access(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    let identity = match require_user_actor(&state, &headers) {
        Ok(identity) => identity,
        Err(response) => return response,
    };
    match state.workspace_access.list_for(&identity) {
        Ok(value) => json_response(value),
        Err(error) => workspace_access_error(&state, error),
    }
}

async fn api_request_workspace_access(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<RequestWorkspaceAccessRequest>,
) -> Response {
    let identity = match require_user_actor(&state, &headers) {
        Ok(identity) => identity,
        Err(response) => return response,
    };
    match state
        .workspace_access
        .request_access(&identity, &request.share_code)
    {
        Ok(()) => (StatusCode::ACCEPTED, Json(json!({ "requested": true }))).into_response(),
        Err(error) => workspace_access_error(&state, error),
    }
}

async fn api_accept_workspace_access(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath((id, user_id)): AxumPath<(String, String)>,
) -> Response {
    let identity = match require_user_actor(&state, &headers) {
        Ok(identity) => identity,
        Err(response) => return response,
    };
    match state
        .workspace_access
        .accept_request(&identity, &id, &user_id)
    {
        Ok(value) => json_response(value),
        Err(error) => workspace_access_error(&state, error),
    }
}

async fn api_reject_workspace_access(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath((id, user_id)): AxumPath<(String, String)>,
) -> Response {
    let identity = match require_user_actor(&state, &headers) {
        Ok(identity) => identity,
        Err(response) => return response,
    };
    match state
        .workspace_access
        .reject_request(&identity, &id, &user_id)
    {
        Ok(value) => json_response(value),
        Err(error) => workspace_access_error(&state, error),
    }
}

async fn api_revoke_workspace_access(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath((id, user_id)): AxumPath<(String, String)>,
) -> Response {
    let identity = match require_user_actor(&state, &headers) {
        Ok(identity) => identity,
        Err(response) => return response,
    };
    match state
        .workspace_access
        .revoke_member(&identity, &id, &user_id)
    {
        Ok(value) => json_response(value),
        Err(error) => workspace_access_error(&state, error),
    }
}

async fn api_create_git_docker_environment(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(request): Json<CreateGitDockerEnvironmentRequest>,
) -> Response {
    let identity = match require_user_actor(&state, &headers) {
        Ok(identity) => identity,
        Err(response) => return response,
    };
    if is_draining(&state) {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "noeud en drain: creation d'environnement refusee",
            &state.config,
        );
    }
    let projects_root = match state.workspace_access.personal_root(&identity) {
        Ok(root) => root,
        Err(error) => return workspace_access_error(&state, error),
    };
    let bundles_root = projects_root
        .parent()
        .unwrap_or(&projects_root)
        .join("docker-images");
    let access = state.workspace_access.clone();
    match tokio::task::spawn_blocking(move || {
        let result = git_docker_environment::create_git_docker_environment_in(
            &projects_root,
            &bundles_root,
            request,
        )?;
        access
            .claim_or_authorize_environment(&identity, &result.workspace_path, None)
            .map_err(|error| error.message)?;
        Ok::<_, String>(result)
    })
    .await
    {
        Ok(Ok(result)) => json_response(result),
        Ok(Err(error)) => api_error(StatusCode::BAD_REQUEST, &error, &state.config),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("creation de l'environnement interrompue: {error}"),
            &state.config,
        ),
    }
}

async fn api_delete_workspace(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    match actor {
        RequestActor::Administrator => match delete_workspace(
            &state.config.data_dir,
            &id,
            &state.terminals.active_workspace_ids(),
        ) {
            Ok(()) => json_response(json!({ "ok": true })),
            Err(error) => api_error(StatusCode::BAD_REQUEST, &error, &state.config),
        },
        RequestActor::User(identity) => {
            let view = match state.workspace_access.list_for(&identity) {
                Ok(environments) => environments
                    .into_iter()
                    .find(|environment| environment.id == id),
                Err(error) => return workspace_access_error(&state, error),
            };
            let Some(view) = view else {
                return api_error(
                    StatusCode::NOT_FOUND,
                    "Environnement introuvable ou inaccessible",
                    &state.config,
                );
            };
            let active_id = fs::canonicalize(&view.path)
                .ok()
                .map(|path| workspace_id_for_dir(&path));
            if active_id
                .as_ref()
                .is_some_and(|active_id| state.terminals.active_workspace_ids().contains(active_id))
            {
                return api_error(
                    StatusCode::CONFLICT,
                    "Environnement encore utilise par un terminal actif",
                    &state.config,
                );
            }
            match state
                .workspace_access
                .remove_owned_environment(&identity, &id)
            {
                Ok(()) => json_response(json!({ "ok": true })),
                Err(error) => workspace_access_error(&state, error),
            }
        }
    }
}

/// Navigateur de dossiers borne a la racine autorisee (`workspaces_root`).
/// Renvoie uniquement les sous-dossiers, plus le parent (sauf a la racine). Sert
/// au selecteur de workspace cote web.
async fn api_fs_list(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Query(query): Query<FsListQuery>,
) -> Response {
    let actor = match request_actor(&state, &headers) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let (dir, root) = match actor {
        RequestActor::Administrator => {
            let root = state.config.workspaces_root.clone();
            let dir = match query
                .path
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                Some(path) => match resolve_within_root(&root, path) {
                    Ok(dir) => dir,
                    Err(error) => return api_error(StatusCode::BAD_REQUEST, &error, &state.config),
                },
                None => strip_extended_prefix(&root),
            };
            (dir, root)
        }
        RequestActor::User(identity) => {
            match state
                .workspace_access
                .authorize_browse_path(&identity, query.path.as_deref())
            {
                Ok(value) => value,
                Err(error) => return workspace_access_error(&state, error),
            }
        }
    };
    let root_display = display_path(&root);
    let dir_display = display_path(&dir);
    let canonical_dir = fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
    let canonical_root = fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
    let parent = if canonical_dir == canonical_root {
        None
    } else {
        dir.parent()
            .filter(|parent| parent.starts_with(&canonical_root))
            .map(display_path)
    };
    let entries = match list_subdirs(&dir) {
        Ok(entries) => entries,
        Err(error) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    };
    json_response(FsListResponse {
        root: root_display,
        path: dir_display,
        parent,
        entries,
    })
}

async fn ws_terminal(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<u64>,
    Query(params): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> Response {
    let token = params.get("token").map(String::as_str).unwrap_or("");
    let actor =
        if crate::security::constant_time_eq(token.as_bytes(), state.config.admin_token.as_bytes())
        {
            RequestActor::Administrator
        } else {
            if !websocket_origin_allowed(&state.config, &headers) {
                return api_error(
                    StatusCode::FORBIDDEN,
                    "origine WebSocket non autorisee",
                    &state.config,
                );
            }
            match request_actor(&state, &headers) {
                Ok(actor) => actor,
                Err(response) => return response,
            }
        };

    let session = match state.terminals.get_for_actor(id, &actor) {
        Ok(session) => session,
        Err(error) => return api_error(StatusCode::NOT_FOUND, &error, &state.config),
    };

    ws.on_upgrade(move |socket| handle_terminal_socket(socket, state, id, session))
}

/// Signal partage des changements runtime. Le flux ne transporte aucune donnee
/// metier : le client relit le snapshot REST correspondant apres chaque
/// revision, avec son authentification habituelle. Les changements de
/// messagerie sont filtres selon l'identite de session avant tout envoi.
async fn ws_runtime(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> Response {
    let token = params.get("token").map(String::as_str).unwrap_or("");
    if !crate::security::constant_time_eq(token.as_bytes(), state.config.admin_token.as_bytes()) {
        if !websocket_origin_allowed(&state.config, &headers) {
            return api_error(
                StatusCode::FORBIDDEN,
                "origine WebSocket non autorisee",
                &state.config,
            );
        }
        if let Err(response) = check_admin_header(&state, &headers) {
            return response;
        }
    }

    let user_id = match state.auth.identity_from_headers(&headers) {
        Ok(Some(identity)) => identity.id,
        Ok(None) => PrivateMessageUser::administrator().id,
        Err(error) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    };
    let sync = state.chat.runtime_sync();
    ws.on_upgrade(move |socket| handle_runtime_socket(socket, sync, user_id))
}

async fn handle_runtime_socket(socket: WebSocket, sync: RuntimeSync, user_id: String) {
    let mut events = sync.subscribe();
    let (mut sender, mut receiver) = socket.split();
    if send_ws_value(
        &mut sender,
        &json!({ "type": "hello", "revision": sync.revision() }),
    )
    .await
    .is_err()
    {
        return;
    }

    loop {
        tokio::select! {
            event = events.recv() => {
                match event {
                    Ok(event) if event.is_visible_to(&user_id) => {
                        if send_ws_value(
                            &mut sender,
                            &json!({
                                "type": "change",
                                "topic": event.topic,
                                "revision": event.revision,
                            }),
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if send_ws_value(
                            &mut sender,
                            &json!({ "type": "resync", "revision": sync.revision() }),
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            incoming = receiver.next() => {
                let Some(Ok(message)) = incoming else {
                    break;
                };
                match message {
                    Message::Close(_) => break,
                    Message::Ping(data) => {
                        if sender.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    Message::Text(text) if text.contains("\"type\":\"ping\"") => {
                        if send_ws_value(&mut sender, &json!({ "type": "pong" })).await.is_err() {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Flux temps reel des discussions, utilise notamment par la WebView Android.
/// Sans `accountId`/`sessionId`, il pousse l'index complet lorsqu'un fichier de
/// session change. Avec les deux parametres, il pousse le transcript cible.
async fn ws_discussions(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> Response {
    let token = params.get("token").map(String::as_str).unwrap_or("");
    let actor =
        if crate::security::constant_time_eq(token.as_bytes(), state.config.admin_token.as_bytes())
        {
            RequestActor::Administrator
        } else {
            if !websocket_origin_allowed(&state.config, &headers) {
                return api_error(
                    StatusCode::FORBIDDEN,
                    "origine WebSocket non autorisee",
                    &state.config,
                );
            }
            match request_actor(&state, &headers) {
                Ok(actor) => actor,
                Err(response) => return response,
            }
        };

    let account_id = params.get("accountId").cloned();
    let session_id = params.get("sessionId").cloned();
    if account_id.is_some() != session_id.is_some() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "accountId et sessionId doivent etre fournis ensemble",
            &state.config,
        );
    }

    if let (Some(identity), Some(account_id), Some(session_id)) =
        (actor.user(), account_id.as_deref(), session_id.as_deref())
    {
        if let Err(response) =
            authorize_discussion_for_identity(&state, identity, account_id, session_id)
        {
            return response;
        }
    }

    let identity = actor.user().cloned();
    ws.on_upgrade(move |socket| {
        handle_discussions_socket(socket, state, identity, account_id, session_id)
    })
}

async fn handle_discussions_socket(
    socket: WebSocket,
    state: Arc<ServerState>,
    identity: Option<AuthIdentity>,
    account_id: Option<String>,
    session_id: Option<String>,
) {
    let (mut sender, mut receiver) = socket.split();
    let mut ticker = tokio::time::interval(Duration::from_millis(750));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_revision: Option<u64> = None;
    let mut last_payload: Option<String> = None;
    let mut last_error: Option<String> = None;

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let update = discussion_ws_update(
                    state.clone(),
                    identity.clone(),
                    account_id.clone(),
                    session_id.clone(),
                    last_revision,
                ).await;

                match update {
                    Ok(Some((revision, payload, signature))) => {
                        // La revision source peut changer pour un evenement outil
                        // qui n'ajoute aucun tour visible. Dans ce cas on memorise
                        // la revision sans renvoyer inutilement le meme transcript.
                        last_revision = Some(revision);
                        last_error = None;
                        if last_payload.as_deref() != Some(signature.as_str()) {
                            if send_ws_value(&mut sender, &payload).await.is_err() {
                                break;
                            }
                            last_payload = Some(signature);
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        if last_error.as_deref() != Some(error.as_str()) {
                            let payload = json!({ "type": "error", "message": error });
                            if send_ws_value(&mut sender, &payload).await.is_err() {
                                break;
                            }
                            last_error = payload
                                .get("message")
                                .and_then(|value| value.as_str())
                                .map(ToString::to_string);
                        }
                    }
                }
            }
            incoming = receiver.next() => {
                let Some(Ok(message)) = incoming else {
                    break;
                };
                match message {
                    Message::Close(_) => break,
                    Message::Ping(data) => {
                        if sender.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    Message::Text(text) if text.contains("\"type\":\"ping\"") => {
                        if send_ws_value(&mut sender, &json!({ "type": "pong" })).await.is_err() {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Renvoie un snapshot uniquement si l'empreinte disque a change depuis le
/// dernier tick. Le scan/parsing de JSONL reste dans le pool bloquant pour ne
/// jamais immobiliser les workers async d'Axum.
async fn discussion_ws_update(
    state: Arc<ServerState>,
    identity: Option<AuthIdentity>,
    account_id: Option<String>,
    session_id: Option<String>,
    last_revision: Option<u64>,
) -> Result<Option<(u64, serde_json::Value, String)>, String> {
    if let (Some(account_id), Some(session_id)) = (account_id, session_id) {
        if let Some(identity) = identity.as_ref() {
            authorize_discussion_for_identity(&state, identity, &account_id, &session_id)
                .map_err(|_| "Discussion introuvable ou inaccessible".to_string())?;
        }
        let revision_account = account_id.clone();
        let revision_session = session_id.clone();
        let revision = tokio::task::spawn_blocking(move || {
            discussions::transcript_revision_for_account(&revision_account, &revision_session)
        })
        .await
        .map_err(|error| error.to_string())??;
        if last_revision == Some(revision) {
            return Ok(None);
        }

        let transcript_account = account_id.clone();
        let transcript_session = session_id.clone();
        let transcript = tokio::task::spawn_blocking(move || {
            discussions::transcript_for_account(transcript_account, transcript_session)
        })
        .await
        .map_err(|error| error.to_string())??;
        let payload = json!({
            "type": "transcript",
            "accountId": account_id,
            "sessionId": session_id,
            "transcript": transcript,
        });
        let signature = payload.to_string();
        return Ok(Some((revision, payload, signature)));
    }

    let revision = tokio::task::spawn_blocking(discussions::discussions_revision)
        .await
        .map_err(|error| error.to_string())??;
    if last_revision == Some(revision) {
        return Ok(None);
    }
    let mut dashboard = tokio::task::spawn_blocking(move || {
        discussions::list_discussions_dashboard_at_revision(revision)
    })
    .await
    .map_err(|error| error.to_string())??;
    if let Some(identity) = identity.as_ref() {
        dashboard = filter_discussions_for_identity(&state, identity, dashboard);
    }
    let payload = json!({ "type": "dashboard", "dashboard": dashboard });
    // generatedAt est volontairement exclu de la signature fonctionnelle : sa
    // variation seule ne constitue pas une mise a jour visible.
    let mut stable = payload.clone();
    if let Some(dashboard) = stable
        .get_mut("dashboard")
        .and_then(|value| value.as_object_mut())
    {
        dashboard.remove("generatedAt");
    }
    let signature = stable.to_string();
    Ok(Some((revision, payload, signature)))
}

async fn handle_terminal_socket(
    socket: WebSocket,
    state: Arc<ServerState>,
    id: u64,
    session: Arc<RemoteTerminalSession>,
) {
    let (mut sender, mut receiver) = socket.split();
    let _ = send_ws(
        &mut sender,
        &ServerWsMessage::Status {
            id,
            status: "active".to_string(),
            workspace_id: session.workspace_id.clone(),
            workspace_path: session.workspace_path.to_string_lossy().to_string(),
        },
    )
    .await;
    // Le premier socket recupere le receiver cree avant le spawn du PTY. Les
    // sockets suivants reprennent le receiver remis en attente a la fermeture,
    // ce qui couvre aussi la courte fenetre d'une reconnexion.
    let socket_generation = session.socket_generation.fetch_add(1, Ordering::AcqRel) + 1;
    let TerminalEventCursor {
        receiver: mut events,
        prefetched: mut pending_event,
    } = take_terminal_event_cursor(&session.events, &session.pending_events);
    let mut terminal_ended = false;
    loop {
        if let Some(event) = pending_event.take() {
            let (event, next_pending) = coalesce_terminal_data(event, &mut events);
            pending_event = next_pending;
            terminal_ended = matches!(event, ServerWsMessage::Exit { .. });
            if send_ws(&mut sender, &event).await.is_err() || terminal_ended {
                break;
            }
            continue;
        }
        tokio::select! {
            event = events.recv() => {
                match event {
                    Ok(event) => {
                        let (event, next_pending) = coalesce_terminal_data(event, &mut events);
                        pending_event = next_pending;
                        terminal_ended = matches!(event, ServerWsMessage::Exit { .. });
                        if send_ws(&mut sender, &event).await.is_err() || terminal_ended {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            incoming = receiver.next() => {
                let Some(Ok(message)) = incoming else {
                    break;
                };
                if let Message::Ping(payload) = message {
            let _ = sender.send(Message::Pong(payload)).await;
            continue;
        }                if let Message::Text(text) = message {
                    match serde_json::from_str::<ClientWsMessage>(&text) {
                        Ok(ClientWsMessage::Input { data }) => {
                            if let Err(error) = state.terminals.write(id, data) {
                                let event = ServerWsMessage::Error { id, message: error };
                                let _ = send_ws(&mut sender, &event).await;
                                if !state.terminals.contains(id) {
                                    break;
                                }
                            }
                        }
                        Ok(ClientWsMessage::Resize { cols, rows }) => {
                            if let Err(error) = state.terminals.resize(id, cols, rows) {
                                let event = ServerWsMessage::Error { id, message: error };
                                let _ = send_ws(&mut sender, &event).await;
                                if !state.terminals.contains(id) {
                                    break;
                                }
                            }
                        }
                        Ok(ClientWsMessage::Stop) => {
                            if let Err(error) = state.terminals.stop(id) {
                                let event = ServerWsMessage::Error { id, message: error };
                                let _ = send_ws(&mut sender, &event).await;
                            }
                            break;
                        }
                        Ok(ClientWsMessage::Ping) => {
                            let _ = send_ws(&mut sender, &ServerWsMessage::Pong { id }).await;
                        }
                        Err(error) => {
                            let event = ServerWsMessage::Error {
                                id,
                                message: format!("message websocket invalide: {error}"),
                            };
                            let _ = send_ws(&mut sender, &event).await;
                        }
                    }
                }
            }
        }
    }

    if !terminal_ended
        && state.terminals.contains(id)
        && session.socket_generation.load(Ordering::Acquire) == socket_generation
    {
        restore_terminal_event_cursor(
            &session.pending_events,
            TerminalEventCursor {
                receiver: events,
                prefetched: pending_event,
            },
        );
    }
}

fn coalesce_terminal_data(
    first: ServerWsMessage,
    events: &mut broadcast::Receiver<ServerWsMessage>,
) -> (ServerWsMessage, Option<ServerWsMessage>) {
    let ServerWsMessage::Data { id, mut data } = first else {
        return (first, None);
    };

    if data.len() > TERMINAL_WS_DATA_BATCH_BYTES {
        let mut split_at = TERMINAL_WS_DATA_BATCH_BYTES;
        while !data.is_char_boundary(split_at) {
            split_at -= 1;
        }
        let remainder = data.split_off(split_at);
        return (
            ServerWsMessage::Data { id, data },
            Some(ServerWsMessage::Data {
                id,
                data: remainder,
            }),
        );
    }

    while data.len() < TERMINAL_WS_DATA_BATCH_BYTES {
        match events.try_recv() {
            Ok(ServerWsMessage::Data {
                id: next_id,
                data: next_data,
            }) => {
                if next_id != id
                    || data.len().saturating_add(next_data.len()) > TERMINAL_WS_DATA_BATCH_BYTES
                {
                    return (
                        ServerWsMessage::Data { id, data },
                        Some(ServerWsMessage::Data {
                            id: next_id,
                            data: next_data,
                        }),
                    );
                }
                data.push_str(&next_data);
            }
            Ok(event) => return (ServerWsMessage::Data { id, data }, Some(event)),
            Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed) => {
                break
            }
        }
    }

    (ServerWsMessage::Data { id, data }, None)
}

fn take_terminal_event_cursor(
    events: &broadcast::Sender<ServerWsMessage>,
    pending_events: &Mutex<Option<TerminalEventCursor>>,
) -> TerminalEventCursor {
    pending_events
        .lock()
        .ok()
        .and_then(|mut pending| pending.take())
        .unwrap_or_else(|| TerminalEventCursor {
            receiver: events.subscribe(),
            prefetched: None,
        })
}

fn restore_terminal_event_cursor(
    pending_events: &Mutex<Option<TerminalEventCursor>>,
    cursor: TerminalEventCursor,
) {
    if let Ok(mut pending) = pending_events.lock() {
        if pending.is_none() {
            *pending = Some(cursor);
        }
    }
}

async fn send_ws(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    event: &ServerWsMessage,
) -> Result<(), axum::Error> {
    let text = serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string());
    sender.send(Message::Text(text)).await
}

async fn send_ws_value(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    event: &serde_json::Value,
) -> Result<(), axum::Error> {
    sender.send(Message::Text(event.to_string())).await
}

fn auth_or(
    state: &Arc<ServerState>,
    headers: &HeaderMap,
    f: impl FnOnce() -> Result<Response, String>,
) -> Response {
    if let Err(response) = check_admin_header(state, headers) {
        return response;
    }

    match f() {
        Ok(response) => response,
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error, &state.config),
    }
}

fn request_actor(state: &Arc<ServerState>, headers: &HeaderMap) -> Result<RequestActor, Response> {
    // Le client distant joint le jeton administrateur technique a toutes les
    // requetes, y compris lorsqu'un cookie utilisateur valide est present.
    // Privilegier ce cookie conserve l'identite nominative necessaire aux
    // espaces partages et applique leurs controles d'acces utilisateur.
    match state.auth.identity_from_headers(headers) {
        Ok(Some(identity)) => return Ok(RequestActor::User(identity)),
        Ok(None) => {}
        Err(error) => {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &error,
                &state.config,
            ))
        }
    }

    let bearer = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let provided = bearer.strip_prefix("Bearer ").unwrap_or(bearer).trim();
    if crate::security::constant_time_eq(provided.as_bytes(), state.config.admin_token.as_bytes()) {
        return Ok(RequestActor::Administrator);
    }
    Err(api_error(
        StatusCode::UNAUTHORIZED,
        "authentification requise",
        &state.config,
    ))
}

fn device_terminal_capability_from_headers(
    state: &Arc<ServerState>,
    headers: &HeaderMap,
) -> Option<DeviceTerminalCapability> {
    let bearer = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let provided = bearer.strip_prefix("Bearer ").unwrap_or(bearer).trim();
    state.terminals.device_terminal_capability(provided)
}

fn device_action_context_from_headers(
    state: &Arc<ServerState>,
    headers: &HeaderMap,
) -> Result<DeviceActionContext, Response> {
    if let Some(capability) = device_terminal_capability_from_headers(state, headers) {
        Ok(capability.action_context())
    } else {
        request_actor(state, headers).map(|actor| DeviceActionContext {
            owner_id: actor.owner_id().to_string(),
            access: DeviceActionAccess::OwnerAll,
        })
    }
}

fn require_user_actor(
    state: &Arc<ServerState>,
    headers: &HeaderMap,
) -> Result<AuthIdentity, Response> {
    match request_actor(state, headers)? {
        RequestActor::User(identity) => Ok(identity),
        RequestActor::Administrator => Err(api_error(
            StatusCode::FORBIDDEN,
            "Cette action exige une session utilisateur nominative",
            &state.config,
        )),
    }
}

fn workspace_access_error(state: &Arc<ServerState>, error: WorkspaceAccessError) -> Response {
    let status = match error.kind {
        WorkspaceAccessErrorKind::Validation => StatusCode::BAD_REQUEST,
        WorkspaceAccessErrorKind::Unauthorized => StatusCode::UNAUTHORIZED,
        WorkspaceAccessErrorKind::Forbidden => StatusCode::FORBIDDEN,
        WorkspaceAccessErrorKind::NotFound => StatusCode::NOT_FOUND,
        WorkspaceAccessErrorKind::Conflict => StatusCode::CONFLICT,
        WorkspaceAccessErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    };
    api_error(status, &error.message, &state.config)
}

fn check_admin_header(state: &Arc<ServerState>, headers: &HeaderMap) -> Result<(), Response> {
    let bearer = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let provided = bearer.strip_prefix("Bearer ").unwrap_or(bearer).trim();
    if crate::security::constant_time_eq(provided.as_bytes(), state.config.admin_token.as_bytes())
        || state.auth.authorize_headers(headers)
    {
        Ok(())
    } else {
        Err(api_error(
            StatusCode::UNAUTHORIZED,
            "authentification requise",
            &state.config,
        ))
    }
}

fn websocket_origin_allowed(config: &ServerConfig, headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get("origin").and_then(|value| value.to_str().ok()) else {
        // Les clients non navigateur peuvent ne pas envoyer Origin. Ils doivent
        // tout de meme presenter une session ou un token valide juste apres.
        return true;
    };
    let (Ok(origin), Ok(expected)) = (
        url::Url::parse(origin.trim()),
        url::Url::parse(config.public_base_url.trim()),
    ) else {
        return false;
    };
    origin.scheme() == expected.scheme()
        && origin.host_str() == expected.host_str()
        && origin.port_or_known_default() == expected.port_or_known_default()
}

/// Les operations de maintenance automatisee (drain / mise a jour) restent
/// reservees au secret administrateur et ne sont jamais ouvertes aux comptes
/// utilisateurs ordinaires.
fn check_maintenance_header(state: &Arc<ServerState>, headers: &HeaderMap) -> Result<(), Response> {
    let bearer = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let provided = bearer.strip_prefix("Bearer ").unwrap_or(bearer).trim();
    if crate::security::constant_time_eq(provided.as_bytes(), state.config.admin_token.as_bytes()) {
        Ok(())
    } else {
        Err(api_error(
            StatusCode::UNAUTHORIZED,
            "token admin invalide",
            &state.config,
        ))
    }
}

fn json_response(value: impl Serialize) -> Response {
    Json(value).into_response()
}

fn server_kombai_status(_config: &ServerConfig, status: KombaiStatus) -> KombaiStatus {
    status
}

fn api_error(status: StatusCode, message: &str, config: &ServerConfig) -> Response {
    let message = redact_secrets(message, config);
    (
        status,
        Json(json!({
            "error": {
                "message": message,
                "code": status.as_u16()
            }
        })),
    )
        .into_response()
}

fn agent_start_status(error: &str) -> StatusCode {
    if error.starts_with("capacite agents atteinte")
        || error.starts_with("capacite chats atteinte")
        || error.starts_with("capacite terminaux atteinte")
    {
        StatusCode::TOO_MANY_REQUESTS
    } else if error.starts_with("memoire insuffisante") {
        StatusCode::SERVICE_UNAVAILABLE
    } else if error.contains("deja vivant")
        || error.contains("déjà en cours")
        || error.starts_with("Compte Freebuff indisponible")
    {
        StatusCode::CONFLICT
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

fn resource_error_status(error: &str) -> StatusCode {
    if error.to_lowercase().contains("introuvable") {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

fn pool_status_response(
    config: &ServerConfig,
    settings: &AppSettings,
    manager: &PoolManager,
    running: bool,
) -> Response {
    let views = manager.status_view();
    let total = views.len();
    let idle = views
        .iter()
        .filter(|account| account.status == AccountStatus::Idle)
        .count();
    json_response(json!({
        "running": running,
        "startedAt": manager.started_at(),
        "baseUrl": config.public_base_url,
        "model": settings.pool.default_model,
        "upstream": settings.pool.upstream,
        "total": total,
        "idle": idle,
        "accounts": views,
    }))
}

fn shell_command(settings: &AppSettings) -> CommandBuilder {
    let raw = settings.shell.trim();
    let shell = if raw.is_empty() || (cfg!(unix) && raw.to_ascii_lowercase().contains("powershell"))
    {
        "/bin/bash"
    } else {
        raw
    };
    let mut builder = CommandBuilder::new(shell);
    let lower = shell.to_ascii_lowercase();
    if lower.ends_with("powershell.exe")
        || lower.ends_with("pwsh.exe")
        || lower == "powershell"
        || lower == "pwsh"
    {
        builder.arg("-NoLogo");
    }
    builder
}

fn prepare_workspace(
    repo_url: &str,
    branch: Option<&str>,
    target: &Path,
    git_pat: &str,
) -> Result<String, String> {
    let repo_url = repo_url.trim();
    if repo_url.is_empty() {
        fs::create_dir_all(target).map_err(|error| error.to_string())?;
        return Ok("workspace vide".to_string());
    }

    clone_repo(repo_url, branch, target, git_pat)?;
    Ok(repo_url.to_string())
}

fn clone_repo(
    repo_url: &str,
    branch: Option<&str>,
    target: &Path,
    git_pat: &str,
) -> Result<(), String> {
    let repo_url = validate_repo_url(repo_url)?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    let authed_url = authenticated_repo_url(&repo_url, git_pat);
    let mut command = Command::new("git");
    command.arg("clone");
    if let Some(branch) = branch.map(str::trim).filter(|value| !value.is_empty()) {
        command.arg("--branch").arg(branch);
    }
    command
        .arg(&authed_url)
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = command
        .output()
        .map_err(|error| format!("git clone impossible: {error}"))?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    Err(format!(
        "git clone a echoue ({}): {}{}",
        output.status, stdout, stderr
    ))
}

fn validate_repo_url(repo_url: &str) -> Result<String, String> {
    let value = repo_url.trim();
    if value.is_empty() {
        return Err("repoUrl requis en mode SaaS".to_string());
    }
    if value.contains([' ', '\n', '\r', '\t']) {
        return Err("repoUrl contient un caractere invalide".to_string());
    }
    let allowed = [
        "https://github.com/",
        "https://gitlab.com/",
        "https://bitbucket.org/",
    ];
    if allowed.iter().any(|prefix| value.starts_with(prefix)) {
        Ok(value.to_string())
    } else {
        Err("repoUrl doit commencer par https://github.com/, https://gitlab.com/ ou https://bitbucket.org/".to_string())
    }
}

fn authenticated_repo_url(repo_url: &str, git_pat: &str) -> String {
    let token = git_pat.trim();
    if token.is_empty() || repo_url.contains('@') {
        return repo_url.to_string();
    }
    if let Some(path) = repo_url.strip_prefix("https://github.com/") {
        return format!("https://x-access-token:{token}@github.com/{path}");
    }
    if let Some(path) = repo_url.strip_prefix("https://gitlab.com/") {
        return format!("https://oauth2:{token}@gitlab.com/{path}");
    }
    if let Some(path) = repo_url.strip_prefix("https://bitbucket.org/") {
        return format!("https://x-token-auth:{token}@bitbucket.org/{path}");
    }
    repo_url.to_string()
}

fn redact_secrets(value: &str, config: &ServerConfig) -> String {
    let mut redacted = value.to_string();
    for secret in [&config.git_pat, &config.admin_token] {
        let secret = secret.trim();
        if !secret.is_empty() {
            redacted = redacted.replace(secret, "***");
        }
    }
    redacted
}

fn spawn_workspace_cleanup(data_dir: PathBuf, terminals: RemoteTerminalManager) {
    tokio::spawn(async move {
        loop {
            let _ = cleanup_old_workspaces(&data_dir, &terminals.active_workspace_ids());
            tokio::time::sleep(Duration::from_secs(60 * 60)).await;
        }
    });
}

fn cleanup_old_workspaces(data_dir: &Path, active: &HashSet<String>) -> Result<(), String> {
    let now = SystemTime::now();
    let root = data_dir.join("workspaces");
    if !root.is_dir() {
        return Ok(());
    }

    for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if active.contains(&entry.file_name().to_string_lossy().to_string()) {
            continue;
        }
        let metadata = entry.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_dir() {
            continue;
        }
        let modified = metadata.modified().unwrap_or(now);
        let age = now
            .duration_since(modified)
            .unwrap_or_else(|_| Duration::from_secs(0));
        if age.as_secs() >= WORKSPACE_RETENTION_SECS {
            fs::remove_dir_all(entry.path()).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn list_workspaces(data_dir: &Path) -> Result<Vec<WorkspaceView>, String> {
    let mut views = Vec::new();
    for root in [data_dir.join("workspaces")] {
        if !root.is_dir() {
            continue;
        }
        for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let metadata = entry.metadata().map_err(|error| error.to_string())?;
            if !metadata.is_dir() {
                continue;
            }
            let modified_at = metadata.modified().ok().and_then(system_time_to_unix);
            views.push(WorkspaceView {
                id: entry.file_name().to_string_lossy().to_string(),
                path: entry.path().to_string_lossy().to_string(),
                modified_at,
                retained_until: modified_at.map(|ts| ts + WORKSPACE_RETENTION_SECS as i64),
            });
        }
    }
    views.sort_by(|a, b| b.modified_at.cmp(&a.modified_at));
    Ok(views)
}

fn delete_workspace(data_dir: &Path, id: &str, active: &HashSet<String>) -> Result<(), String> {
    if id.contains(['/', '\\']) || id == "." || id == ".." {
        return Err("identifiant de dossier invalide".to_string());
    }
    if active.contains(id) {
        return Err("dossier encore utilise par un agent actif".to_string());
    }
    for target in [data_dir.join("workspaces").join(id)] {
        if target.is_dir() {
            let root = target
                .parent()
                .and_then(|parent| parent.canonicalize().ok())
                .ok_or_else(|| "racine des dossiers invalide".to_string())?;
            let resolved = target.canonicalize().map_err(|error| error.to_string())?;
            if resolved.starts_with(&root) && resolved != root {
                fs::remove_dir_all(&resolved).map_err(|error| error.to_string())?;
            }
        }
    }
    Ok(())
}

fn system_time_to_unix(value: SystemTime) -> Option<i64> {
    value
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device_test_config(node_id: &str, admin_token: &str) -> ServerConfig {
        let data_dir = std::env::temp_dir().join(format!("cst-device-test-{}", Uuid::new_v4()));
        ServerConfig {
            bind: "0.0.0.0:8080".to_string(),
            data_dir: data_dir.clone(),
            static_dir: data_dir.join("dist"),
            admin_token: admin_token.to_string(),
            git_pat: String::new(),
            public_base_url: "https://switch.test".to_string(),
            node_id: node_id.to_string(),
            node_label: node_id.to_string(),
            node_capacity: 2,
            terminal_capacity: 2,
            workspaces_root: data_dir,
            duello_referral_api_url: None,
            duello_referral_api_key: None,
            duello_app_url: "https://app.duello.fr/".to_string(),
            duello_bank: DuelloBankConfig::from_values(None, None, None),
        }
    }

    #[test]
    fn expired_drain_lease_reopens_the_node() {
        let drain_until = AtomicI64::new(120);
        assert!(drain_lease_active(&drain_until, 119));
        assert!(!drain_lease_active(&drain_until, 120));
        assert_eq!(drain_until.load(Ordering::Acquire), 0);
    }

    #[test]
    fn drain_request_accepts_a_camel_case_ttl() {
        let request: DrainRequest =
            serde_json::from_str(r#"{"draining":true,"ttlSeconds":12}"#).unwrap();
        assert!(request.draining);
        assert_eq!(request.ttl_seconds, Some(12));
    }

    #[test]
    fn direct_tiktok_tool_builds_one_idempotent_campaign_request() {
        let arguments = json!({
            "recipient": "@compte_test",
            "message": "Bonjour depuis le VPS"
        });
        let build = || {
            let request =
                serde_json::from_value::<SendTikTokDmToolArguments>(arguments.clone()).unwrap();
            let SendTikTokDmToolArguments::Direct(request) = request else {
                panic!("le format direct doit etre selectionne");
            };
            request.into_prepare_request("capability-token")
        };
        let first = build();
        let second = build();
        assert_eq!(first.recipients, vec!["@compte_test"]);
        assert_eq!(first.message, "Bonjour depuis le VPS");
        assert_eq!(first.min_interval_minutes, 1);
        assert_eq!(first.max_interval_minutes, 2);
        assert_eq!(first.idempotency_key, second.idempotency_key);
        assert!(first.idempotency_key.starts_with("chat-direct:"));
    }

    #[test]
    fn missing_resources_are_not_reported_as_server_failures() {
        assert_eq!(
            resource_error_status("Tour de conversation introuvable"),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            resource_error_status("Session terminal introuvable"),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            resource_error_status("Etat des conversations verrouille"),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn hashed_frontend_assets_are_cached_but_entrypoints_are_revalidated() {
        assert_eq!(
            frontend_cache_control("/assets/index-ABC123.js"),
            Some("public, max-age=31536000, immutable")
        );
        assert_eq!(
            frontend_cache_control("/"),
            Some("no-cache, must-revalidate")
        );
        assert_eq!(
            frontend_cache_control("/service-worker.js"),
            Some("no-cache, must-revalidate")
        );
        assert_eq!(frontend_cache_control("/api/settings"), None);
        assert_eq!(frontend_cache_control("/ws/discussions"), None);
        assert_eq!(
            frontend_response_cache_control(
                "/assets/prompts-view-ancien.js",
                StatusCode::NOT_FOUND
            ),
            Some("no-store")
        );
        assert_eq!(
            frontend_response_cache_control("/assets/prompts-view-courant.js", StatusCode::OK),
            Some("public, max-age=31536000, immutable")
        );
    }

    #[test]
    fn remote_live_terminal_id_is_reserved_atomically() {
        let manager = RemoteTerminalManager::default();
        let reservation = manager.reserve_id(Some(77), None).unwrap();
        assert!(manager.reserve_id(Some(77), None).is_err());
        drop(reservation);
        assert!(manager.reserve_id(Some(77), None).is_ok());
    }

    #[test]
    fn committed_remote_terminal_reservation_is_released_after_spawn() {
        let manager = RemoteTerminalManager::default();
        let reservation = manager.reserve_id(Some(1), Some("freebuff-a")).unwrap();
        reservation.commit();
        assert!(manager.reserve_id(Some(1), Some("freebuff-a")).is_ok());
    }

    #[test]
    fn remote_freebuff_account_start_is_reserved_atomically() {
        let manager = RemoteTerminalManager::default();
        let reservation = manager.reserve_id(Some(1), Some("freebuff-a")).unwrap();
        assert!(manager.reserve_id(Some(2), Some("freebuff-a")).is_err());
        assert!(manager.reserve_id(Some(3), Some("freebuff-b")).is_ok());
        drop(reservation);
        assert!(manager.reserve_id(Some(4), Some("freebuff-a")).is_ok());
    }

    #[test]
    fn freebuff_device_capability_is_per_terminal_and_uses_loopback() {
        let config = device_test_config("node-a", "admin-secret-a");
        let manager = RemoteTerminalManager::with_max_active(2);
        let first = manager
            .reserve_device_terminal_token(41, "human-owner")
            .unwrap();
        let second = manager
            .reserve_device_terminal_token(42, "human-owner")
            .unwrap();

        assert!(first.token.starts_with("cstd_"));
        assert_ne!(first.token, second.token);
        assert_ne!(first.capability.origin_id, second.capability.origin_id);
        assert!(Uuid::parse_str(&first.capability.origin_id).is_ok());
        assert_eq!(first.capability.owner_id, "human-owner");
        assert_eq!(first.capability.terminal_id, 41);
        assert_eq!(
            first.capability.action_context(),
            DeviceActionContext {
                owner_id: "human-owner".to_string(),
                access: DeviceActionAccess::OriginOnly(first.capability.origin_id.clone()),
            }
        );
        assert_eq!(
            manager.device_terminal_capability(&first.token),
            Some(first.capability.clone())
        );
        assert!(manager.device_terminal_capability("cstd_wrong").is_none());
        let first_token = first.token.clone();
        drop(first);
        assert!(manager.device_terminal_capability(&first_token).is_none());
        let second_token = second.token.clone();
        let second_capability = second.capability.clone();
        second.commit();
        assert_eq!(
            manager.device_terminal_capability(&second_token),
            Some(second_capability)
        );
        assert_eq!(
            config.device_terminal_api_url().unwrap(),
            "http://127.0.0.1:8080/api/device-fleet"
        );
    }

    #[test]
    fn freebuff_terminal_origins_isolate_dedupe_status_and_revoke_immediately() {
        let device_fleet = DeviceFleetManager::new(Duration::from_secs(300));
        device_fleet
            .heartbeat(DeviceConnectorHeartbeatRequest {
                connector_id: "usb-test".to_string(),
                devices: vec![device_fleet::ControlDevice {
                    id: "android:ABC123".to_string(),
                    platform: device_fleet::DevicePlatform::Android,
                    transport: device_fleet::DeviceTransport::Usb,
                    serial: "ABC123".to_string(),
                    name: Some("Test phone".to_string()),
                    model: None,
                    os_version: None,
                    state: device_fleet::DeviceConnectionState::Ready,
                    ready: true,
                    jailbreak_ready: false,
                    capabilities: vec![DeviceActionKind::Info],
                    connector_id: Some("usb-test".to_string()),
                    last_seen_at: metrics::now_ts(),
                    error: None,
                }],
                tools: device_fleet::DeviceFleetTools::default(),
                error: None,
            })
            .unwrap();
        let manager =
            RemoteTerminalManager::with_max_active(2).with_device_fleet(device_fleet.clone());
        let first = manager
            .reserve_device_terminal_token(41, "human-owner")
            .unwrap();
        let second = manager
            .reserve_device_terminal_token(42, "human-owner")
            .unwrap();
        let first_context = first.capability.action_context();
        let second_context = second.capability.action_context();
        let request: DeviceActionRequest = serde_json::from_value(json!({
            "deviceId": "android:ABC123",
            "action": "info",
            "idempotencyKey": "freebuff:same-retry-key",
        }))
        .unwrap();

        let first_action = first_context
            .queue_action(&device_fleet, request.clone())
            .unwrap();
        let second_action = second_context.queue_action(&device_fleet, request).unwrap();
        assert_ne!(first_action.id, second_action.id);
        assert!(matches!(
            second_context.action_status(&device_fleet, &first_action.id),
            Err(DeviceFleetError::NotFound)
        ));

        let first_token = first.token.clone();
        drop(first);
        assert!(manager.device_terminal_capability(&first_token).is_none());
        assert!(matches!(
            first_context.action_status(&device_fleet, &first_action.id),
            Err(DeviceFleetError::NotFound)
        ));
        let owner_context = DeviceActionContext {
            owner_id: "human-owner".to_string(),
            access: DeviceActionAccess::OwnerAll,
        };
        assert_eq!(
            owner_context
                .action_status(&device_fleet, &first_action.id)
                .unwrap()
                .status,
            DeviceActionStatus::Expired
        );
    }

    #[test]
    fn remote_terminal_capacity_accepts_twenty_and_rejects_the_twenty_first() {
        let manager = RemoteTerminalManager::with_max_active(20);
        let reservations = (1..=20)
            .map(|id| manager.reserve_id(Some(id), None).unwrap())
            .collect::<Vec<_>>();
        assert!(manager.reserve_id(Some(21), None).is_err());
        drop(reservations);
        assert!(manager.reserve_id(Some(21), None).is_ok());
    }

    #[test]
    fn zero_remote_terminal_capacity_has_no_numeric_limit() {
        let manager = RemoteTerminalManager::with_max_active(0);
        let reservations = (1..=64)
            .map(|id| manager.reserve_id(Some(id), None).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(reservations.len(), 64);
    }

    #[test]
    fn source_terminal_keys_are_trimmed_and_bounded() {
        assert_eq!(
            normalize_source_terminal_key(Some("  terminal-mobile-1  ".to_string())).unwrap(),
            Some("terminal-mobile-1".to_string())
        );
        assert_eq!(
            normalize_source_terminal_key(Some("  ".to_string())).unwrap(),
            None
        );
        assert!(normalize_source_terminal_key(Some("x".repeat(161))).is_err());
        assert!(normalize_source_terminal_key(Some("terminal\nmobile".to_string())).is_err());
    }

    #[test]
    fn stale_codex_login_commands_use_device_auth_on_the_server() {
        assert_eq!(
            normalize_remote_login_command(Provider::Codex, Some("codex login".to_string())),
            Some("codex login --device-auth".to_string())
        );
        assert_eq!(
            normalize_remote_login_command(
                Provider::Codex,
                Some("codex logout; codex login".to_string())
            ),
            Some("codex logout; codex login --device-auth".to_string())
        );
        assert_eq!(
            normalize_remote_login_command(Provider::Codex, Some("custom-codex-login".to_string())),
            Some("custom-codex-login".to_string())
        );
        assert_eq!(
            normalize_remote_login_command(Provider::Claude, Some("claude auth login".to_string())),
            Some("claude auth login".to_string())
        );
    }

    #[test]
    fn terminal_output_emitted_before_socket_is_replayed() {
        let (events, initial_receiver) = broadcast::channel(8);
        let pending = Mutex::new(Some(TerminalEventCursor {
            receiver: initial_receiver,
            prefetched: None,
        }));

        events
            .send(ServerWsMessage::Data {
                id: 7,
                data: "initial ANSI screen".to_string(),
            })
            .expect("the retained receiver must keep pre-connection output");

        let mut cursor = take_terminal_event_cursor(&events, &pending);
        match cursor
            .receiver
            .try_recv()
            .expect("pre-connection output must be replayed")
        {
            ServerWsMessage::Data { id, data } => {
                assert_eq!(id, 7);
                assert_eq!(data, "initial ANSI screen");
            }
            other => panic!("unexpected terminal event: {other:?}"),
        }
    }

    #[test]
    fn terminal_output_emitted_between_sockets_is_replayed() {
        let (events, initial_receiver) = broadcast::channel(8);
        let pending = Mutex::new(Some(TerminalEventCursor {
            receiver: initial_receiver,
            prefetched: None,
        }));

        let cursor = take_terminal_event_cursor(&events, &pending);
        restore_terminal_event_cursor(&pending, cursor);

        events
            .send(ServerWsMessage::Data {
                id: 9,
                data: "while disconnected".to_string(),
            })
            .expect("the restored receiver must keep reconnect output");

        let mut resumed = take_terminal_event_cursor(&events, &pending);
        match resumed
            .receiver
            .try_recv()
            .expect("disconnect output must be replayed")
        {
            ServerWsMessage::Data { id, data } => {
                assert_eq!(id, 9);
                assert_eq!(data, "while disconnected");
            }
            other => panic!("unexpected terminal event: {other:?}"),
        }
    }

    #[test]
    fn terminal_data_batch_prefetched_event_survives_reconnect_after_send_failure() {
        let (events, initial_receiver) = broadcast::channel(8);
        let pending = Mutex::new(Some(TerminalEventCursor {
            receiver: initial_receiver,
            prefetched: None,
        }));

        events
            .send(ServerWsMessage::Error {
                id: 10,
                message: "frontiere".to_string(),
            })
            .unwrap();
        events
            .send(ServerWsMessage::Data {
                id: 10,
                data: "apres".to_string(),
            })
            .unwrap();

        let mut cursor = take_terminal_event_cursor(&events, &pending);
        let (event, prefetched) = coalesce_terminal_data(
            ServerWsMessage::Data {
                id: 10,
                data: "avant".to_string(),
            },
            &mut cursor.receiver,
        );
        match event {
            ServerWsMessage::Data { id, data } => {
                assert_eq!(id, 10);
                assert_eq!(data, "avant");
            }
            other => panic!("unexpected terminal event: {other:?}"),
        }

        // Simule l'echec de `send_ws` du batch courant : la frontiere deja
        // prelevee doit rester liee au receiver pour le socket suivant.
        cursor.prefetched = prefetched;
        restore_terminal_event_cursor(&pending, cursor);

        let mut resumed = take_terminal_event_cursor(&events, &pending);
        match resumed
            .prefetched
            .take()
            .expect("the prefetched boundary must survive reconnect")
        {
            ServerWsMessage::Error { id, message } => {
                assert_eq!(id, 10);
                assert_eq!(message, "frontiere");
            }
            other => panic!("unexpected terminal event: {other:?}"),
        }
        match resumed.receiver.try_recv().unwrap() {
            ServerWsMessage::Data { id, data } => {
                assert_eq!(id, 10);
                assert_eq!(data, "apres");
            }
            other => panic!("unexpected terminal event: {other:?}"),
        }
    }

    #[test]
    fn terminal_data_batch_coalesces_immediately_queued_utf8_fragments() {
        let (events, mut receiver) = broadcast::channel(8);
        events
            .send(ServerWsMessage::Data {
                id: 11,
                data: "é".to_string(),
            })
            .unwrap();
        events
            .send(ServerWsMessage::Data {
                id: 11,
                data: " ☕".to_string(),
            })
            .unwrap();

        let (event, pending) = coalesce_terminal_data(
            ServerWsMessage::Data {
                id: 11,
                data: "caf".to_string(),
            },
            &mut receiver,
        );

        assert!(pending.is_none());
        match event {
            ServerWsMessage::Data { id, data } => {
                assert_eq!(id, 11);
                assert_eq!(data, "café ☕");
            }
            other => panic!("unexpected terminal event: {other:?}"),
        }
    }

    #[test]
    fn terminal_data_batch_stops_before_every_control_event() {
        let boundaries = [
            ServerWsMessage::Error {
                id: 12,
                message: "erreur".to_string(),
            },
            ServerWsMessage::Status {
                id: 12,
                status: "active".to_string(),
                workspace_id: "workspace".to_string(),
                workspace_path: "/workspace".to_string(),
            },
            ServerWsMessage::Exit { id: 12 },
        ];

        for boundary in boundaries {
            let expected_boundary = std::mem::discriminant(&boundary);
            let (events, mut receiver) = broadcast::channel(8);
            events.send(boundary).unwrap();
            events
                .send(ServerWsMessage::Data {
                    id: 12,
                    data: "apres".to_string(),
                })
                .unwrap();

            let (event, pending) = coalesce_terminal_data(
                ServerWsMessage::Data {
                    id: 12,
                    data: "avant".to_string(),
                },
                &mut receiver,
            );

            match event {
                ServerWsMessage::Data { id, data } => {
                    assert_eq!(id, 12);
                    assert_eq!(data, "avant");
                }
                other => panic!("unexpected terminal event: {other:?}"),
            }
            let pending = pending.expect("the control event must remain pending");
            assert_eq!(std::mem::discriminant(&pending), expected_boundary);
            match receiver.try_recv().unwrap() {
                ServerWsMessage::Data { id, data } => {
                    assert_eq!(id, 12);
                    assert_eq!(data, "apres");
                }
                other => panic!("unexpected terminal event: {other:?}"),
            }
        }
    }

    #[test]
    fn terminal_data_batch_keeps_overflow_for_the_next_frame() {
        let (events, mut receiver) = broadcast::channel(8);
        events
            .send(ServerWsMessage::Data {
                id: 13,
                data: "b".to_string(),
            })
            .unwrap();
        events
            .send(ServerWsMessage::Data {
                id: 13,
                data: "é".to_string(),
            })
            .unwrap();

        let (event, pending) = coalesce_terminal_data(
            ServerWsMessage::Data {
                id: 13,
                data: "a".repeat(TERMINAL_WS_DATA_BATCH_BYTES - 2),
            },
            &mut receiver,
        );

        match event {
            ServerWsMessage::Data { id, data } => {
                assert_eq!(id, 13);
                assert_eq!(data.len(), TERMINAL_WS_DATA_BATCH_BYTES - 1);
                assert!(data.ends_with('b'));
            }
            other => panic!("unexpected terminal event: {other:?}"),
        }
        match pending.expect("the overflowing UTF-8 fragment must remain pending") {
            ServerWsMessage::Data { id, data } => {
                assert_eq!(id, 13);
                assert_eq!(data, "é");
            }
            other => panic!("unexpected terminal event: {other:?}"),
        }
    }

    #[test]
    fn terminal_data_batch_splits_oversized_first_fragment_on_utf8_boundary() {
        let (_events, mut receiver) = broadcast::channel(1);
        let original = format!("{}éfin", "a".repeat(TERMINAL_WS_DATA_BATCH_BYTES - 1));

        let (event, pending) = coalesce_terminal_data(
            ServerWsMessage::Data {
                id: 14,
                data: original.clone(),
            },
            &mut receiver,
        );

        let ServerWsMessage::Data {
            id,
            data: first_data,
        } = event
        else {
            panic!("the first frame must remain a data event");
        };
        assert_eq!(id, 14);
        assert!(first_data.len() <= TERMINAL_WS_DATA_BATCH_BYTES);

        let Some(ServerWsMessage::Data {
            id,
            data: remaining_data,
        }) = pending
        else {
            panic!("the oversized fragment must keep a data remainder");
        };
        assert_eq!(id, 14);
        assert!(remaining_data.starts_with('é'));
        assert_eq!(format!("{first_data}{remaining_data}"), original);
    }

    #[test]
    fn incremental_utf8_decoder_keeps_split_multibyte_sequences() {
        let mut decode = incremental_utf8_decoder();
        // « é » = 0xC3 0xA9 coupe entre les deux octets.
        assert_eq!(decode(&[b'c', 0xC3]), "c");
        assert_eq!(decode(&[0xA9, b't']), "ét");
        // Emoji « 👍 » = 4 octets coupe apres le troisieme.
        let bytes = [0xF0, 0x9F, 0x91, 0x8D, b'x'];
        assert_eq!(decode(&bytes[..3]), "");
        assert_eq!(decode(&bytes[3..]), "👍x");
    }

    #[test]
    fn incremental_utf8_decoder_is_lossy_on_invalid_byte() {
        let mut decode = incremental_utf8_decoder();
        assert_eq!(decode(&[b'a', 0xFF, b'b']), "a\u{FFFD}b");
    }

    #[test]
    fn incremental_utf8_decoder_keeps_split_sequence_after_invalid_byte() {
        let mut decode = incremental_utf8_decoder();
        assert_eq!(decode(&[b'a', 0xFF, 0xC3]), "a\u{FFFD}");
        assert_eq!(decode(&[0xA9, b'x']), "éx");
    }

    #[test]
    fn resolve_within_root_confines_to_root() {
        // Racine reelle sous le repertoire temporaire, canonicalisee comme le
        // fait `resolve_workspaces_root` en production.
        let uid = format!("{}-{:p}", std::process::id(), &0u8 as *const u8);
        let base = std::env::temp_dir().join(format!("cst-ws-root-{uid}"));
        let child = base.join("projects").join("alpha");
        fs::create_dir_all(&child).expect("create child dir");
        let outside = std::env::temp_dir().join(format!("cst-ws-out-{uid}"));
        fs::create_dir_all(&outside).expect("create outside dir");
        let root = fs::canonicalize(&base).expect("canonicalize root");

        // Chemin absolu valide dans la racine.
        let resolved = resolve_within_root(&root, &child.to_string_lossy())
            .expect("child abs path must resolve");
        assert!(resolved.ends_with("alpha"));

        // Chemin relatif a la racine.
        let rel = resolve_within_root(&root, "projects/alpha").expect("relative path must resolve");
        assert!(rel.ends_with("alpha"));

        // Chemin vide -> racine elle-meme.
        let root_resolved = resolve_within_root(&root, "  ").expect("empty resolves to root");
        assert_eq!(root_resolved, strip_extended_prefix(&root));

        // Echappement par `..` vers un dossier hors racine : refuse.
        let escape = base.join("..").join(outside.file_name().unwrap());
        assert!(
            resolve_within_root(&root, &escape.to_string_lossy()).is_err(),
            "path traversal via .. must be rejected"
        );

        // Dossier existant mais hors de la racine : refuse.
        assert!(
            resolve_within_root(&root, &outside.to_string_lossy()).is_err(),
            "absolute path outside root must be rejected"
        );

        // Chemin inexistant : refuse.
        assert!(resolve_within_root(&root, "does/not/exist").is_err());

        let _ = fs::remove_dir_all(&base);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn workspace_id_for_dir_has_no_path_separators() {
        let id = workspace_id_for_dir(Path::new("/home/user/My Projects/app"));
        assert!(id.starts_with("dir-"));
        assert!(!id.contains('/'));
        assert!(!id.contains('\\'));
        assert!(!id.contains(' '));
    }
}
