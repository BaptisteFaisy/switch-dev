use crate::ios_wda::{
    IosWdaAction, IosWdaActionResult, IosWdaCapability, IosWdaError, IosWdaErrorKind, IosWdaProbe,
    IosWdaProvider,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    env, fmt, fs,
    io::Read,
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex, OnceLock},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Notify;
use uuid::Uuid;

pub const DEFAULT_CONNECTOR_TTL_SECONDS: i64 = 60;
pub const ACTION_CLAIM_LEASE_SECONDS: i64 = 300;
pub const MAX_ACTION_OUTPUT_BYTES: usize = 64 * 1024;
pub const MAX_SCREENSHOT_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const DEFAULT_DEVICE_ACTION_CONCURRENCY: usize = 8;
pub(crate) const MAX_DEVICE_ACTION_CONCURRENCY: usize = 128;

const MAX_DEVICES: usize = 128;
const MAX_METADATA_PROBES: usize = 16;
const MAX_QUEUED_ACTIONS: usize = 500;
const MAX_ACTIONS_PER_DEVICE: usize = 50;
const QUEUED_ACTION_TTL_SECONDS: i64 = 300;
const COMPLETED_ACTION_RETENTION_SECONDS: i64 = 24 * 60 * 60;
const TOOL_STATUS_CACHE_TTL: Duration = Duration::from_secs(5);
const REVOKED_ORIGIN_RETENTION_SECONDS: i64 = 24 * 60 * 60;
const ACTION_STORE_SCHEMA: u32 = 1;
const ACTION_STORE_DIRECTORY: &str = "device-fleet-actions";
const ACTION_STORE_FILE_PREFIX: &str = "actions-";
const ACTION_STORE_FILE_SUFFIX: &str = ".json";
const ACTION_STORE_GENERATIONS_TO_KEEP: usize = 2;

fn parse_device_action_concurrency(value: Option<&str>) -> usize {
    value
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|capacity| (1..=MAX_DEVICE_ACTION_CONCURRENCY).contains(capacity))
        .unwrap_or(DEFAULT_DEVICE_ACTION_CONCURRENCY)
}

pub(crate) fn configured_device_action_concurrency() -> usize {
    parse_device_action_concurrency(env::var("CST_DEVICE_ACTION_CONCURRENCY").ok().as_deref())
}

fn bounded_parallel_map_ordered<T, R, F>(items: Vec<T>, limit: usize, operation: F) -> Vec<R>
where
    T: Send,
    R: Send,
    F: Fn(T) -> R + Sync,
{
    if items.len() <= 1 || limit <= 1 {
        return items.into_iter().map(operation).collect();
    }
    let item_count = items.len();
    let worker_count = item_count.min(limit.max(1));
    let queue = Mutex::new(items.into_iter().enumerate().collect::<VecDeque<_>>());
    let results = Mutex::new(Vec::with_capacity(item_count));
    thread::scope(|scope| {
        for _ in 0..worker_count {
            let queue = &queue;
            let results = &results;
            let operation = &operation;
            scope.spawn(move || loop {
                let next = queue
                    .lock()
                    .expect("file des sondes metadata disponible")
                    .pop_front();
                let Some((index, item)) = next else {
                    break;
                };
                let result = operation(item);
                results
                    .lock()
                    .expect("resultats des sondes metadata disponibles")
                    .push((index, result));
            });
        }
    });
    let mut results = results
        .into_inner()
        .expect("resultats des sondes metadata disponibles");
    results.sort_unstable_by_key(|(index, _)| *index);
    debug_assert_eq!(results.len(), item_count);
    results.into_iter().map(|(_, result)| result).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DevicePlatform {
    Android,
    Ios,
}

impl DevicePlatform {
    fn id_prefix(self) -> &'static str {
        match self {
            Self::Android => "android",
            Self::Ios => "ios",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceTransport {
    Usb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceConnectionState {
    Ready,
    Unauthorized,
    Offline,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceActionKind {
    Info,
    Screenshot,
    OpenScreen,
    Tap,
    Swipe,
    TypeText,
    KeyEvent,
    OpenApp,
    Shell,
    PushFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeviceToolStatus {
    pub available: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeviceFleetTools {
    pub adb: DeviceToolStatus,
    pub scrcpy: DeviceToolStatus,
    pub idevice_id: DeviceToolStatus,
    pub idevice_info: DeviceToolStatus,
    pub idevice_screenshot: DeviceToolStatus,
    pub iproxy: DeviceToolStatus,
    pub ssh: DeviceToolStatus,
    #[serde(default)]
    pub ios_wda: DeviceToolStatus,
    pub ios_ssh_key_configured: bool,
    pub ios_ui_tool_configured: bool,
}

#[derive(Clone)]
struct IosWdaRuntime {
    provider: Option<IosWdaProvider>,
    status: DeviceToolStatus,
}

fn parse_ios_wda_opt_in(value: Option<&str>) -> Result<bool, String> {
    let Some(value) = value else {
        return Ok(false);
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(
            "CST_IOS_WDA_ENABLED invalide (valeurs acceptees: 1/true/yes/on ou 0/false/no/off)"
                .to_string(),
        ),
    }
}

fn build_ios_wda_runtime() -> IosWdaRuntime {
    match parse_ios_wda_opt_in(env::var("CST_IOS_WDA_ENABLED").ok().as_deref()) {
        Ok(false) => IosWdaRuntime {
            provider: None,
            status: DeviceToolStatus {
                available: false,
                detail: Some(
                    "WDA desactive; activez-le explicitement avec CST_IOS_WDA_ENABLED=1"
                        .to_string(),
                ),
            },
        },
        Err(error) => IosWdaRuntime {
            provider: None,
            status: DeviceToolStatus {
                available: false,
                detail: Some(format!("WDA refuse: {error}")),
            },
        },
        Ok(true) => match IosWdaProvider::from_env() {
            Ok(provider) => IosWdaRuntime {
                provider: Some(provider),
                status: DeviceToolStatus {
                    available: true,
                    detail: Some(
                        "WDA active; disponibilite et capacites verifiees par appareil".to_string(),
                    ),
                },
            },
            Err(error) => IosWdaRuntime {
                provider: None,
                status: DeviceToolStatus {
                    available: false,
                    detail: Some(format!(
                        "WDA active mais configuration invalide: {}",
                        error.message()
                    )),
                },
            },
        },
    }
}

fn ios_wda_runtime() -> &'static IosWdaRuntime {
    static RUNTIME: OnceLock<IosWdaRuntime> = OnceLock::new();
    RUNTIME.get_or_init(build_ios_wda_runtime)
}

fn ios_wda_provider() -> Option<&'static IosWdaProvider> {
    ios_wda_runtime().provider.as_ref()
}

fn effective_ios_wda_provider(tools: &DeviceFleetTools) -> Option<&'static IosWdaProvider> {
    tools.ios_wda.available.then(ios_wda_provider).flatten()
}

fn invalidate_disappeared_ios_wda_devices(
    provider: Option<&IosWdaProvider>,
    current_udids: &[String],
) {
    static LAST_SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let Some(provider) = provider else {
        return;
    };
    let current = current_udids.iter().cloned().collect::<HashSet<_>>();
    let Ok(mut last_seen) = LAST_SEEN.get_or_init(|| Mutex::new(HashSet::new())).lock() else {
        return;
    };
    for disappeared in last_seen.difference(&current) {
        provider.invalidate(disappeared);
    }
    *last_seen = current;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlDevice {
    pub id: String,
    pub platform: DevicePlatform,
    pub transport: DeviceTransport,
    pub serial: String,
    pub name: Option<String>,
    pub model: Option<String>,
    pub os_version: Option<String>,
    pub state: DeviceConnectionState,
    pub ready: bool,
    pub jailbreak_ready: bool,
    pub capabilities: Vec<DeviceActionKind>,
    pub connector_id: Option<String>,
    pub last_seen_at: i64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeviceFleetSnapshot {
    pub connector_online: bool,
    pub connector_id: Option<String>,
    pub last_seen_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub devices: Vec<ControlDevice>,
    pub tools: DeviceFleetTools,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceActionArgs {
    #[serde(default)]
    pub x: Option<i32>,
    #[serde(default)]
    pub y: Option<i32>,
    #[serde(default, alias = "fromX")]
    pub start_x: Option<i32>,
    #[serde(default, alias = "fromY")]
    pub start_y: Option<i32>,
    #[serde(default, alias = "toX")]
    pub end_x: Option<i32>,
    #[serde(default, alias = "toY")]
    pub end_y: Option<i32>,
    #[serde(default)]
    pub duration_ms: Option<u32>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub app_id: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub local_path: Option<String>,
    #[serde(default)]
    pub remote_path: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

impl Default for DeviceActionArgs {
    fn default() -> Self {
        Self {
            x: None,
            y: None,
            start_x: None,
            start_y: None,
            end_x: None,
            end_y: None,
            duration_ms: None,
            text: None,
            key: None,
            app_id: None,
            command: None,
            local_path: None,
            remote_path: None,
            timeout_ms: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceActionRequest {
    pub device_id: String,
    pub action: DeviceActionKind,
    #[serde(default)]
    pub args: DeviceActionArgs,
    #[serde(default)]
    pub confirmed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceActionResult {
    pub action_id: String,
    pub device_id: String,
    pub action: DeviceActionKind,
    pub success: bool,
    pub detail: String,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub data_base64: Option<String>,
    pub mime_type: Option<String>,
    pub started_at: i64,
    pub finished_at: i64,
    pub truncated: bool,
}

impl DeviceActionResult {
    fn failed(
        action_id: impl Into<String>,
        request: &DeviceActionRequest,
        detail: impl Into<String>,
    ) -> Self {
        let now = now_ts();
        Self {
            action_id: action_id.into(),
            device_id: request.device_id.clone(),
            action: request.action,
            success: false,
            detail: detail.into(),
            stdout: None,
            stderr: None,
            data_base64: None,
            mime_type: None,
            started_at: now,
            finished_at: now,
            truncated: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceActionStatus {
    Queued,
    Claimed,
    Succeeded,
    Failed,
    Expired,
}

impl DeviceActionStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Expired)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceActionRecord {
    pub id: String,
    pub owner_id: String,
    pub device_id: String,
    pub action: DeviceActionKind,
    pub status: DeviceActionStatus,
    pub created_at: i64,
    pub updated_at: i64,
    pub claimed_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub connector_id: Option<String>,
    pub result: Option<DeviceActionResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceConnectorHeartbeatRequest {
    pub connector_id: String,
    #[serde(default)]
    pub devices: Vec<ControlDevice>,
    #[serde(default)]
    pub tools: DeviceFleetTools,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceConnectorClaimRequest {
    pub connector_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceConnectorJob {
    pub action_id: String,
    pub claim_token: String,
    pub owner_id: String,
    pub device_id: String,
    pub request: DeviceActionRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeviceConnectorClaimResponse {
    pub job: Option<DeviceConnectorJob>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceConnectorReportRequest {
    pub connector_id: String,
    pub action_id: String,
    pub claim_token: String,
    pub result: DeviceActionResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceFleetError {
    Validation(String),
    NotFound,
    Conflict(String),
    Unavailable(String),
    Internal(String),
}

impl fmt::Display for DeviceFleetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(detail) => write!(formatter, "{detail}"),
            Self::NotFound => write!(formatter, "Action ou appareil introuvable"),
            Self::Conflict(detail) => write!(formatter, "{detail}"),
            Self::Unavailable(detail) => write!(formatter, "{detail}"),
            Self::Internal(detail) => write!(formatter, "{detail}"),
        }
    }
}

impl std::error::Error for DeviceFleetError {}

#[derive(Debug, Clone)]
struct StoredConnectorSnapshot {
    connector_id: String,
    last_seen_at: i64,
    devices: Vec<ControlDevice>,
    tools: DeviceFleetTools,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct QueuedAction {
    public: DeviceActionRecord,
    request: DeviceActionRequest,
    claim_token: Option<String>,
    origin_id: Option<String>,
    ephemeral_origin: bool,
}

fn gc_revoked_origins(
    revoked_origins: &mut HashMap<(String, String), i64>,
    actions: &[QueuedAction],
    now: i64,
) -> usize {
    let origins_with_actions = actions
        .iter()
        .filter_map(|action| {
            action.origin_id.as_deref().map(|origin_id| {
                (action.public.owner_id.as_str(), origin_id)
            })
        })
        .collect::<HashSet<_>>();
    let previous_len = revoked_origins.len();
    revoked_origins.retain(|(owner_id, origin_id), revoked_at| {
        revoked_at
            .saturating_add(REVOKED_ORIGIN_RETENTION_SECONDS)
            > now
            || origins_with_actions.contains(&(owner_id.as_str(), origin_id.as_str()))
    });
    previous_len.saturating_sub(revoked_origins.len())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedActionArgs {
    x: Option<i32>,
    y: Option<i32>,
    start_x: Option<i32>,
    start_y: Option<i32>,
    end_x: Option<i32>,
    end_y: Option<i32>,
    duration_ms: Option<u32>,
    text: Option<String>,
    key: Option<String>,
    app_id: Option<String>,
    command: Option<String>,
    local_path: Option<String>,
    remote_path: Option<String>,
    timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedActionRequest {
    device_id: String,
    action: DeviceActionKind,
    args: PersistedActionArgs,
    confirmed: bool,
    idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedActionResult {
    action_id: String,
    device_id: String,
    action: DeviceActionKind,
    success: bool,
    detail: String,
    stdout: Option<String>,
    stderr: Option<String>,
    data_omitted: bool,
    mime_type: Option<String>,
    started_at: i64,
    finished_at: i64,
    truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedActionRecord {
    id: String,
    owner_id: String,
    device_id: String,
    action: DeviceActionKind,
    status: DeviceActionStatus,
    created_at: i64,
    updated_at: i64,
    claimed_at: Option<i64>,
    finished_at: Option<i64>,
    connector_id: Option<String>,
    result: Option<PersistedActionResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedQueuedAction {
    public: PersistedActionRecord,
    request: PersistedActionRequest,
    origin_id: Option<String>,
    ephemeral_origin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedActionStore {
    schema: u32,
    generation: u64,
    checksum: String,
    actions: Vec<PersistedQueuedAction>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistedActionChecksum<'a> {
    schema: u32,
    generation: u64,
    actions: &'a [PersistedQueuedAction],
}

struct ActionPersistence {
    directory: PathBuf,
    generation: Mutex<u64>,
}

impl From<&DeviceActionArgs> for PersistedActionArgs {
    fn from(value: &DeviceActionArgs) -> Self {
        Self {
            x: value.x,
            y: value.y,
            start_x: value.start_x,
            start_y: value.start_y,
            end_x: value.end_x,
            end_y: value.end_y,
            duration_ms: value.duration_ms,
            text: value.text.clone(),
            key: value.key.clone(),
            app_id: value.app_id.clone(),
            command: value.command.clone(),
            local_path: value.local_path.clone(),
            remote_path: value.remote_path.clone(),
            timeout_ms: value.timeout_ms,
        }
    }
}

impl From<PersistedActionArgs> for DeviceActionArgs {
    fn from(value: PersistedActionArgs) -> Self {
        Self {
            x: value.x,
            y: value.y,
            start_x: value.start_x,
            start_y: value.start_y,
            end_x: value.end_x,
            end_y: value.end_y,
            duration_ms: value.duration_ms,
            text: value.text,
            key: value.key,
            app_id: value.app_id,
            command: value.command,
            local_path: value.local_path,
            remote_path: value.remote_path,
            timeout_ms: value.timeout_ms,
        }
    }
}

impl From<&DeviceActionRequest> for PersistedActionRequest {
    fn from(value: &DeviceActionRequest) -> Self {
        Self {
            device_id: value.device_id.clone(),
            action: value.action,
            args: PersistedActionArgs::from(&value.args),
            confirmed: value.confirmed,
            idempotency_key: value.idempotency_key.clone(),
        }
    }
}

impl From<PersistedActionRequest> for DeviceActionRequest {
    fn from(value: PersistedActionRequest) -> Self {
        Self {
            device_id: value.device_id,
            action: value.action,
            args: value.args.into(),
            confirmed: value.confirmed,
            idempotency_key: value.idempotency_key,
        }
    }
}

impl From<&DeviceActionResult> for PersistedActionResult {
    fn from(value: &DeviceActionResult) -> Self {
        Self {
            action_id: value.action_id.clone(),
            device_id: value.device_id.clone(),
            action: value.action,
            success: value.success,
            detail: value.detail.chars().take(1_000).collect(),
            stdout: sanitize_optional_bytes(value.stdout.clone(), MAX_ACTION_OUTPUT_BYTES),
            stderr: sanitize_optional_bytes(value.stderr.clone(), MAX_ACTION_OUTPUT_BYTES),
            data_omitted: value.data_base64.is_some(),
            mime_type: value.mime_type.clone(),
            started_at: value.started_at,
            finished_at: value.finished_at,
            truncated: value.truncated,
        }
    }
}

impl From<PersistedActionResult> for DeviceActionResult {
    fn from(value: PersistedActionResult) -> Self {
        let PersistedActionResult {
            action_id,
            device_id,
            action,
            success,
            detail,
            stdout,
            stderr,
            data_omitted,
            mime_type,
            started_at,
            finished_at,
            truncated,
        } = value;
        let detail = if data_omitted {
            format!(
                "{} [capture binaire omise du snapshot persistant]",
                detail.chars().take(940).collect::<String>()
            )
        } else {
            detail.chars().take(1_000).collect()
        };
        Self {
            action_id,
            device_id,
            action,
            success,
            detail,
            stdout: sanitize_optional_bytes(stdout, MAX_ACTION_OUTPUT_BYTES),
            stderr: sanitize_optional_bytes(stderr, MAX_ACTION_OUTPUT_BYTES),
            data_base64: None,
            mime_type: if data_omitted { None } else { mime_type },
            started_at,
            finished_at,
            truncated,
        }
    }
}

impl From<&DeviceActionRecord> for PersistedActionRecord {
    fn from(value: &DeviceActionRecord) -> Self {
        Self {
            id: value.id.clone(),
            owner_id: value.owner_id.clone(),
            device_id: value.device_id.clone(),
            action: value.action,
            status: value.status,
            created_at: value.created_at,
            updated_at: value.updated_at,
            claimed_at: value.claimed_at,
            finished_at: value.finished_at,
            connector_id: value.connector_id.clone(),
            result: value.result.as_ref().map(PersistedActionResult::from),
        }
    }
}

impl From<PersistedActionRecord> for DeviceActionRecord {
    fn from(value: PersistedActionRecord) -> Self {
        Self {
            id: value.id,
            owner_id: value.owner_id,
            device_id: value.device_id,
            action: value.action,
            status: value.status,
            created_at: value.created_at,
            updated_at: value.updated_at,
            claimed_at: value.claimed_at,
            finished_at: value.finished_at,
            connector_id: value.connector_id,
            result: value.result.map(DeviceActionResult::from),
        }
    }
}

impl From<&QueuedAction> for PersistedQueuedAction {
    fn from(value: &QueuedAction) -> Self {
        Self {
            public: PersistedActionRecord::from(&value.public),
            request: PersistedActionRequest::from(&value.request),
            origin_id: value.origin_id.clone(),
            ephemeral_origin: value.ephemeral_origin,
        }
    }
}

impl TryFrom<PersistedQueuedAction> for QueuedAction {
    type Error = DeviceFleetError;

    fn try_from(value: PersistedQueuedAction) -> Result<Self, Self::Error> {
        let public = DeviceActionRecord::from(value.public);
        let request = DeviceActionRequest::from(value.request);
        validate_action_id(&public.id)?;
        validate_owner_id(&public.owner_id)?;
        validate_action_request(&request)?;
        if public.device_id != request.device_id || public.action != request.action {
            return Err(DeviceFleetError::Internal(
                "Snapshot d'actions incoherent avec sa requete".to_string(),
            ));
        }
        if let Some(origin_id) = value.origin_id.as_deref() {
            validate_action_origin_id(origin_id)?;
        } else if value.ephemeral_origin {
            return Err(DeviceFleetError::Internal(
                "Snapshot d'action ephemere sans origine".to_string(),
            ));
        }
        match public.status {
            DeviceActionStatus::Queued => {
                if public.claimed_at.is_some()
                    || public.finished_at.is_some()
                    || public.result.is_some()
                {
                    return Err(DeviceFleetError::Internal(
                        "Snapshot d'action en attente incoherent".to_string(),
                    ));
                }
            }
            DeviceActionStatus::Claimed => {
                if public.claimed_at.is_none()
                    || public.connector_id.is_none()
                    || public.finished_at.is_some()
                    || public.result.is_some()
                {
                    return Err(DeviceFleetError::Internal(
                        "Snapshot d'action reclamee incoherent".to_string(),
                    ));
                }
            }
            DeviceActionStatus::Succeeded
            | DeviceActionStatus::Failed
            | DeviceActionStatus::Expired => {
                if public.finished_at.is_none() || public.result.is_none() {
                    return Err(DeviceFleetError::Internal(
                        "Snapshot d'action terminee incoherent".to_string(),
                    ));
                }
            }
        }
        if let Some(result) = public.result.as_ref() {
            if result.action_id != public.id
                || result.device_id != public.device_id
                || result.action != public.action
            {
                return Err(DeviceFleetError::Internal(
                    "Snapshot de resultat incoherent".to_string(),
                ));
            }
        }
        Ok(Self {
            public,
            request,
            claim_token: None,
            origin_id: value.origin_id,
            ephemeral_origin: value.ephemeral_origin,
        })
    }
}

impl ActionPersistence {
    fn load(data_dir: &Path) -> Result<(Self, Vec<QueuedAction>), DeviceFleetError> {
        let directory = data_dir.join(ACTION_STORE_DIRECTORY);
        fs::create_dir_all(&directory).map_err(|error| {
            action_persistence_error("creation du repertoire de persistance", error)
        })?;
        let mut final_snapshots = Vec::new();
        let mut has_final_snapshot = false;
        for entry in fs::read_dir(&directory).map_err(|error| {
            action_persistence_error("lecture du repertoire de persistance", error)
        })? {
            let entry = entry.map_err(|error| {
                action_persistence_error("lecture d'une entree de persistance", error)
            })?;
            let file_name = entry.file_name();
            let file_name = file_name.to_string_lossy();
            if !file_name.starts_with(ACTION_STORE_FILE_PREFIX)
                || !file_name.ends_with(ACTION_STORE_FILE_SUFFIX)
            {
                continue;
            }
            has_final_snapshot = true;
            let Some(generation) = action_store_generation(&file_name) else {
                continue;
            };
            final_snapshots.push((generation, entry.path()));
        }
        let highest_named_generation = final_snapshots
            .iter()
            .map(|(generation, _)| *generation)
            .max()
            .unwrap_or(0);
        let mut valid_snapshots = final_snapshots
            .iter()
            .filter_map(|(generation, path)| {
                decode_action_snapshot(path, *generation)
                    .ok()
                    .map(|actions| (*generation, path.clone(), actions))
            })
            .collect::<Vec<_>>();
        valid_snapshots
            .sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
        if has_final_snapshot && valid_snapshots.is_empty() {
            return Err(DeviceFleetError::Internal(
                "Tous les snapshots persistants de la flotte sont invalides".to_string(),
            ));
        }
        let actions = valid_snapshots
            .into_iter()
            .next()
            .map(|(_, _, actions)| actions)
            .unwrap_or_default();
        Ok((
            Self {
                directory,
                generation: Mutex::new(highest_named_generation),
            },
            actions,
        ))
    }

    fn persist(&self, actions: &[QueuedAction]) -> Result<(), DeviceFleetError> {
        let mut generation = self.generation.lock().map_err(|_| {
            DeviceFleetError::Internal("Generation de persistance verrouillee".to_string())
        })?;
        let next_generation = generation.checked_add(1).ok_or_else(|| {
            DeviceFleetError::Internal("Generation de persistance epuisee".to_string())
        })?;
        let persisted_actions = actions
            .iter()
            .map(PersistedQueuedAction::from)
            .collect::<Vec<_>>();
        let checksum =
            action_store_checksum(ACTION_STORE_SCHEMA, next_generation, &persisted_actions)?;
        let snapshot = PersistedActionStore {
            schema: ACTION_STORE_SCHEMA,
            generation: next_generation,
            checksum,
            actions: persisted_actions,
        };
        let bytes = serde_json::to_vec(&snapshot).map_err(|error| {
            action_persistence_error("serialisation du snapshot d'actions", error)
        })?;
        let unique = Uuid::new_v4().simple().to_string();
        let final_path = self.directory.join(format!(
            "{ACTION_STORE_FILE_PREFIX}{next_generation:020}-{unique}{ACTION_STORE_FILE_SUFFIX}"
        ));
        if final_path.exists() {
            return Err(DeviceFleetError::Internal(
                "Collision de nom de snapshot d'actions".to_string(),
            ));
        }
        crate::fs_util::atomic_write(&final_path, &bytes)
            .map_err(|error| action_persistence_error("commit du snapshot d'actions", error))?;
        *generation = next_generation;
        drop(generation);
        self.cleanup_old_generations();
        Ok(())
    }

    fn cleanup_old_generations(&self) {
        let Ok(entries) = fs::read_dir(&self.directory) else {
            return;
        };
        let mut valid = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let generation = action_store_generation(&entry.file_name().to_string_lossy())?;
                decode_action_snapshot(&entry.path(), generation)
                    .ok()
                    .map(|_| (generation, entry.path()))
            })
            .collect::<Vec<_>>();
        valid.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
        for (_, path) in valid.into_iter().skip(ACTION_STORE_GENERATIONS_TO_KEEP) {
            let _ = fs::remove_file(path);
        }
    }
}

fn action_persistence_error(context: &str, error: impl fmt::Display) -> DeviceFleetError {
    DeviceFleetError::Internal(format!("Echec de {context}: {error}"))
}

fn action_store_generation(file_name: &str) -> Option<u64> {
    let body = file_name
        .strip_prefix(ACTION_STORE_FILE_PREFIX)?
        .strip_suffix(ACTION_STORE_FILE_SUFFIX)?;
    let (generation, unique) = body.split_once('-')?;
    if unique.is_empty() {
        return None;
    }
    generation.parse().ok()
}

fn action_store_checksum(
    schema: u32,
    generation: u64,
    actions: &[PersistedQueuedAction],
) -> Result<String, DeviceFleetError> {
    let payload = PersistedActionChecksum {
        schema,
        generation,
        actions,
    };
    let bytes = serde_json::to_vec(&payload).map_err(|error| {
        action_persistence_error("calcul du checksum du snapshot d'actions", error)
    })?;
    let digest = Sha256::digest(bytes);
    Ok(digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>())
}

fn decode_action_snapshot(
    path: &Path,
    expected_generation: u64,
) -> Result<Vec<QueuedAction>, DeviceFleetError> {
    let bytes = fs::read(path)
        .map_err(|error| action_persistence_error("lecture d'un snapshot d'actions", error))?;
    let snapshot: PersistedActionStore = serde_json::from_slice(&bytes)
        .map_err(|error| action_persistence_error("decodage d'un snapshot d'actions", error))?;
    if snapshot.schema != ACTION_STORE_SCHEMA || snapshot.generation != expected_generation {
        return Err(DeviceFleetError::Internal(
            "Schema ou generation de snapshot invalide".to_string(),
        ));
    }
    let expected_checksum =
        action_store_checksum(snapshot.schema, snapshot.generation, &snapshot.actions)?;
    if snapshot.checksum != expected_checksum {
        return Err(DeviceFleetError::Internal(
            "Checksum de snapshot invalide".to_string(),
        ));
    }
    if snapshot.actions.len() > MAX_QUEUED_ACTIONS {
        return Err(DeviceFleetError::Internal(
            "Snapshot d'actions trop volumineux".to_string(),
        ));
    }
    let actions = snapshot
        .actions
        .into_iter()
        .map(QueuedAction::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    let mut action_ids = HashSet::new();
    let mut idempotency_keys = HashSet::new();
    for action in &actions {
        if !action_ids.insert(action.public.id.clone()) {
            return Err(DeviceFleetError::Internal(
                "Snapshot contenant des identifiants d'action dupliques".to_string(),
            ));
        }
        if let Some(key) = action.request.idempotency_key.as_deref() {
            let scope = (
                action.public.owner_id.clone(),
                action.origin_id.clone(),
                key.to_string(),
            );
            if !idempotency_keys.insert(scope) {
                return Err(DeviceFleetError::Internal(
                    "Snapshot contenant des cles d'idempotence dupliquees".to_string(),
                ));
            }
        }
    }
    Ok(actions)
}

struct DeviceFleetInner {
    connector: Mutex<Option<StoredConnectorSnapshot>>,
    revoked_origins: Mutex<HashMap<(String, String), i64>>,
    actions: Mutex<Vec<QueuedAction>>,
    action_persistence: Option<ActionPersistence>,
    local_device_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    changed: Notify,
    ttl_seconds: i64,
}

#[derive(Clone)]
pub struct DeviceFleetManager {
    inner: Arc<DeviceFleetInner>,
}

impl Default for DeviceFleetManager {
    fn default() -> Self {
        Self::new(Duration::from_secs(DEFAULT_CONNECTOR_TTL_SECONDS as u64))
    }
}

impl DeviceFleetManager {
    pub fn new(connector_ttl: Duration) -> Self {
        Self {
            inner: Arc::new(DeviceFleetInner {
                connector: Mutex::new(None),
                revoked_origins: Mutex::new(HashMap::new()),
                actions: Mutex::new(Vec::new()),
                action_persistence: None,
                local_device_locks: Mutex::new(HashMap::new()),
                changed: Notify::new(),
                ttl_seconds: connector_ttl.as_secs().max(1).min(i64::MAX as u64) as i64,
            }),
        }
    }

    pub fn load(data_dir: &Path, connector_ttl: Duration) -> Result<Self, DeviceFleetError> {
        let (action_persistence, actions) = ActionPersistence::load(data_dir)?;
        let mut recovered = actions.clone();
        if recover_actions(&mut recovered, now_ts()) {
            action_persistence.persist(&recovered)?;
        }
        Ok(Self {
            inner: Arc::new(DeviceFleetInner {
                connector: Mutex::new(None),
                revoked_origins: Mutex::new(HashMap::new()),
                actions: Mutex::new(recovered),
                action_persistence: Some(action_persistence),
                local_device_locks: Mutex::new(HashMap::new()),
                changed: Notify::new(),
                ttl_seconds: connector_ttl.as_secs().max(1).min(i64::MAX as u64) as i64,
            }),
        })
    }

    fn persist_actions(&self, actions: &[QueuedAction]) -> Result<(), DeviceFleetError> {
        if let Some(persistence) = self.inner.action_persistence.as_ref() {
            persistence.persist(actions)?;
        }
        Ok(())
    }

    pub fn heartbeat(
        &self,
        request: DeviceConnectorHeartbeatRequest,
    ) -> Result<DeviceFleetSnapshot, DeviceFleetError> {
        self.heartbeat_at(request, now_ts())
    }

    pub fn heartbeat_at(
        &self,
        request: DeviceConnectorHeartbeatRequest,
        now: i64,
    ) -> Result<DeviceFleetSnapshot, DeviceFleetError> {
        let connector_id = validate_connector_id(&request.connector_id)?;
        if request.devices.len() > MAX_DEVICES {
            return Err(DeviceFleetError::Validation(format!(
                "Un connecteur ne peut publier que {MAX_DEVICES} appareils"
            )));
        }

        let mut seen = HashSet::new();
        let mut devices = Vec::with_capacity(request.devices.len());
        for mut device in request.devices {
            validate_control_device(&device)?;
            if !seen.insert(device.id.clone()) {
                continue;
            }
            device.connector_id = Some(connector_id.clone());
            device.last_seen_at = now;
            device.error = sanitize_optional(device.error, 500);
            device.capabilities.sort_by_key(|value| *value as u8);
            device.capabilities.dedup();
            devices.push(device);
        }
        devices.sort_by(|left, right| left.id.cmp(&right.id));

        let stored = StoredConnectorSnapshot {
            connector_id,
            last_seen_at: now,
            devices,
            tools: sanitize_tools(request.tools),
            error: sanitize_optional(request.error, 500),
        };
        let mut connector = self
            .inner
            .connector
            .lock()
            .map_err(|_| DeviceFleetError::Internal("Etat du connecteur verrouille".into()))?;
        *connector = Some(stored);
        drop(connector);
        self.inner.changed.notify_waiters();
        self.snapshot_at(now)
    }

    pub fn snapshot(&self) -> Result<DeviceFleetSnapshot, DeviceFleetError> {
        self.snapshot_at(now_ts())
    }

    pub fn snapshot_at(&self, now: i64) -> Result<DeviceFleetSnapshot, DeviceFleetError> {
        let stored = self
            .inner
            .connector
            .lock()
            .map_err(|_| DeviceFleetError::Internal("Etat du connecteur verrouille".into()))?
            .clone();
        let Some(stored) = stored else {
            return Ok(DeviceFleetSnapshot::default());
        };
        let expires_at = stored.last_seen_at.saturating_add(self.inner.ttl_seconds);
        let connector_online = now <= expires_at;
        let mut devices = stored.devices;
        if !connector_online {
            for device in &mut devices {
                device.ready = false;
                device.state = DeviceConnectionState::Offline;
                device.error = Some("Connecteur USB hors ligne".to_string());
            }
        }
        Ok(DeviceFleetSnapshot {
            connector_online,
            connector_id: Some(stored.connector_id),
            last_seen_at: Some(stored.last_seen_at),
            expires_at: Some(expires_at),
            devices,
            tools: stored.tools,
            error: if connector_online {
                stored.error
            } else {
                Some("Le connecteur USB ne repond plus".to_string())
            },
        })
    }

    pub fn queue_action(
        &self,
        owner_id: &str,
        request: DeviceActionRequest,
    ) -> Result<DeviceActionRecord, DeviceFleetError> {
        self.queue_action_with_origin_at(owner_id, None, false, request, now_ts())
    }

    pub fn queue_action_at(
        &self,
        owner_id: &str,
        request: DeviceActionRequest,
        now: i64,
    ) -> Result<DeviceActionRecord, DeviceFleetError> {
        self.queue_action_with_origin_at(owner_id, None, false, request, now)
    }

    pub fn queue_ephemeral_action(
        &self,
        owner_id: &str,
        origin_id: &str,
        request: DeviceActionRequest,
    ) -> Result<DeviceActionRecord, DeviceFleetError> {
        self.queue_ephemeral_action_at(owner_id, origin_id, request, now_ts())
    }

    pub fn queue_ephemeral_action_at(
        &self,
        owner_id: &str,
        origin_id: &str,
        request: DeviceActionRequest,
        now: i64,
    ) -> Result<DeviceActionRecord, DeviceFleetError> {
        let origin_id = validate_action_origin_id(origin_id)?;
        self.queue_action_with_origin_at(owner_id, Some(origin_id), true, request, now)
    }

    fn queue_action_with_origin_at(
        &self,
        owner_id: &str,
        origin_id: Option<String>,
        ephemeral_origin: bool,
        request: DeviceActionRequest,
        now: i64,
    ) -> Result<DeviceActionRecord, DeviceFleetError> {
        let owner_id = validate_owner_id(owner_id)?;
        validate_action_request(&request)?;
        let mut revoked_origins = self.inner.revoked_origins.lock().map_err(|_| {
            DeviceFleetError::Internal("Liste des origines revoquees verrouillee".into())
        })?;
        let mut actions = self
            .inner
            .actions
            .lock()
            .map_err(|_| DeviceFleetError::Internal("File des actions verrouillee".into()))?;
        gc_revoked_origins(&mut revoked_origins, &actions, now);
        if origin_id.as_ref().is_some_and(|origin_id| {
            revoked_origins.contains_key(&(owner_id.clone(), origin_id.clone()))
        }) {
            return Err(DeviceFleetError::Conflict(
                "Cette origine d'action a ete revoquee".to_string(),
            ));
        }
        let mut pruned = actions.clone();
        if prune_actions(&mut pruned, now) {
            self.persist_actions(&pruned)?;
            *actions = pruned;
            gc_revoked_origins(&mut revoked_origins, &actions, now);
            self.inner.changed.notify_waiters();
        }
        if let Some(idempotency_key) = request.idempotency_key.as_deref() {
            if let Some(existing) = actions.iter().find(|action| {
                action.public.owner_id == owner_id
                    && action.origin_id == origin_id
                    && action.request.idempotency_key.as_deref() == Some(idempotency_key)
            }) {
                if existing.request == request {
                    return Ok(existing.public.clone());
                }
                return Err(DeviceFleetError::Conflict(
                    "La cle d'idempotence est deja liee a une autre action".to_string(),
                ));
            }
        }
        let snapshot = self.snapshot_at(now)?;
        if !snapshot.connector_online {
            return Err(DeviceFleetError::Unavailable(
                "Aucun connecteur USB actif".to_string(),
            ));
        }
        let device = snapshot
            .devices
            .iter()
            .find(|device| device.id == request.device_id)
            .ok_or(DeviceFleetError::NotFound)?;
        ensure_device_supports(device, request.action)?;
        if actions.len() >= MAX_QUEUED_ACTIONS {
            return Err(DeviceFleetError::Conflict(
                "La file de controle des appareils est pleine".to_string(),
            ));
        }
        let device_count = actions
            .iter()
            .filter(|action| {
                action.public.device_id == request.device_id && !action.public.status.is_terminal()
            })
            .count();
        if device_count >= MAX_ACTIONS_PER_DEVICE {
            return Err(DeviceFleetError::Conflict(format!(
                "Trop d'actions sont deja en attente pour {}",
                request.device_id
            )));
        }
        let public = DeviceActionRecord {
            id: Uuid::new_v4().to_string(),
            owner_id,
            device_id: request.device_id.clone(),
            action: request.action,
            status: DeviceActionStatus::Queued,
            created_at: now,
            updated_at: now,
            claimed_at: None,
            finished_at: None,
            connector_id: None,
            result: None,
        };
        let mut candidate = actions.clone();
        candidate.push(QueuedAction {
            public: public.clone(),
            request,
            claim_token: None,
            origin_id,
            ephemeral_origin,
        });
        self.persist_actions(&candidate)?;
        *actions = candidate;
        drop(actions);
        drop(revoked_origins);
        self.inner.changed.notify_waiters();
        Ok(public)
    }

    pub fn action_status(
        &self,
        owner_id: &str,
        action_id: &str,
    ) -> Result<DeviceActionRecord, DeviceFleetError> {
        self.action_status_scoped(owner_id, None, action_id)
    }

    pub fn action_status_for_origin(
        &self,
        owner_id: &str,
        origin_id: &str,
        action_id: &str,
    ) -> Result<DeviceActionRecord, DeviceFleetError> {
        let origin_id = validate_action_origin_id(origin_id)?;
        self.action_status_scoped(owner_id, Some(&origin_id), action_id)
    }

    fn action_status_scoped(
        &self,
        owner_id: &str,
        origin_id: Option<&str>,
        action_id: &str,
    ) -> Result<DeviceActionRecord, DeviceFleetError> {
        let owner_id = validate_owner_id(owner_id)?;
        let action_id = validate_action_id(action_id)?;
        let revoked_origins = self.inner.revoked_origins.lock().map_err(|_| {
            DeviceFleetError::Internal("Liste des origines revoquees verrouillee".into())
        })?;
        if origin_id.is_some_and(|origin_id| {
            revoked_origins.contains_key(&(owner_id.clone(), origin_id.to_string()))
        }) {
            return Err(DeviceFleetError::NotFound);
        }
        let actions = self
            .inner
            .actions
            .lock()
            .map_err(|_| DeviceFleetError::Internal("File des actions verrouillee".into()))?;
        actions
            .iter()
            .find(|action| {
                action.public.id == action_id
                    && action.public.owner_id == owner_id
                    && origin_id.map_or(true, |origin_id| {
                        action.origin_id.as_deref() == Some(origin_id)
                    })
            })
            .map(|action| action.public.clone())
            .ok_or(DeviceFleetError::NotFound)
    }

    pub async fn wait_action(
        &self,
        owner_id: &str,
        action_id: &str,
        timeout: Duration,
    ) -> Result<DeviceActionRecord, DeviceFleetError> {
        self.wait_action_scoped(owner_id, None, action_id, timeout)
            .await
    }

    pub async fn wait_action_for_origin(
        &self,
        owner_id: &str,
        origin_id: &str,
        action_id: &str,
        timeout: Duration,
    ) -> Result<DeviceActionRecord, DeviceFleetError> {
        let origin_id = validate_action_origin_id(origin_id)?;
        self.wait_action_scoped(owner_id, Some(&origin_id), action_id, timeout)
            .await
    }

    async fn wait_action_scoped(
        &self,
        owner_id: &str,
        origin_id: Option<&str>,
        action_id: &str,
        timeout: Duration,
    ) -> Result<DeviceActionRecord, DeviceFleetError> {
        let deadline = Instant::now() + timeout.min(Duration::from_secs(60));
        loop {
            let changed = self.inner.changed.notified();
            let current = self.action_status_scoped(owner_id, origin_id, action_id)?;
            if current.status.is_terminal() {
                return Ok(current);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(current);
            }
            if tokio::time::timeout(remaining, changed).await.is_err() {
                return self.action_status_scoped(owner_id, origin_id, action_id);
            }
        }
    }

    pub fn expire_origin_actions(
        &self,
        owner_id: &str,
        origin_id: &str,
    ) -> Result<usize, DeviceFleetError> {
        self.expire_origin_actions_at(owner_id, origin_id, now_ts())
    }

    pub fn expire_origin_actions_at(
        &self,
        owner_id: &str,
        origin_id: &str,
        now: i64,
    ) -> Result<usize, DeviceFleetError> {
        let owner_id = validate_owner_id(owner_id)?;
        let origin_id = validate_action_origin_id(origin_id)?;
        let mut revoked_origins = self.inner.revoked_origins.lock().map_err(|_| {
            DeviceFleetError::Internal("Liste des origines revoquees verrouillee".into())
        })?;
        revoked_origins.insert((owner_id.clone(), origin_id.clone()), now);
        let mut actions = self
            .inner
            .actions
            .lock()
            .map_err(|_| DeviceFleetError::Internal("File des actions verrouillee".into()))?;
        let mut candidate = actions.clone();
        let mut expired = 0;
        for action in &mut candidate {
            if action.public.owner_id == owner_id
                && action.origin_id.as_deref() == Some(&origin_id)
                && !action.public.status.is_terminal()
            {
                expire_action(
                    action,
                    now,
                    "Origine d'action revoquee ; execution interdite",
                );
                expired += 1;
            }
        }
        if expired == 0 {
            return Ok(0);
        }
        self.persist_actions(&candidate)?;
        *actions = candidate;
        drop(actions);
        drop(revoked_origins);
        self.inner.changed.notify_waiters();
        Ok(expired)
    }

    pub fn claim_next(
        &self,
        connector_id: &str,
    ) -> Result<Option<DeviceConnectorJob>, DeviceFleetError> {
        self.claim_next_at(connector_id, now_ts())
    }

    pub fn claim_next_at(
        &self,
        connector_id: &str,
        now: i64,
    ) -> Result<Option<DeviceConnectorJob>, DeviceFleetError> {
        let connector_id = validate_connector_id(connector_id)?;
        let snapshot = self.snapshot_at(now)?;
        if !snapshot.connector_online || snapshot.connector_id.as_deref() != Some(&connector_id) {
            return Err(DeviceFleetError::Conflict(
                "Ce connecteur USB n'est pas le connecteur actif".to_string(),
            ));
        }
        let ready_ids = snapshot
            .devices
            .iter()
            .filter(|device| device.ready)
            .map(|device| device.id.as_str())
            .collect::<HashSet<_>>();

        let revoked_origins = self.inner.revoked_origins.lock().map_err(|_| {
            DeviceFleetError::Internal("Liste des origines revoquees verrouillee".into())
        })?;
        let mut actions = self
            .inner
            .actions
            .lock()
            .map_err(|_| DeviceFleetError::Internal("File des actions verrouillee".into()))?;
        let mut candidate = actions.clone();
        let mut lease_changed = prune_actions(&mut candidate, now);
        for action in &mut candidate {
            if action.public.status == DeviceActionStatus::Queued
                && action
                    .public
                    .created_at
                    .saturating_add(QUEUED_ACTION_TTL_SECONDS)
                    <= now
            {
                lease_changed = true;
                expire_action(
                    action,
                    now,
                    "Action expiree avant sa prise en charge ; elle ne sera pas executee",
                );
            }
            if action.public.status == DeviceActionStatus::Claimed
                && action.public.claimed_at.is_some_and(|claimed| {
                    claimed.saturating_add(ACTION_CLAIM_LEASE_SECONDS) <= now
                })
            {
                lease_changed = true;
                let claimed_at = action.public.claimed_at.unwrap_or(now);
                let retry_is_safe = matches!(
                    action.public.action,
                    DeviceActionKind::Info | DeviceActionKind::Screenshot
                ) && action
                    .public
                    .created_at
                    .saturating_add(QUEUED_ACTION_TTL_SECONDS)
                    > now;
                action.public.status = if retry_is_safe {
                    DeviceActionStatus::Queued
                } else {
                    DeviceActionStatus::Expired
                };
                action.public.updated_at = now;
                action.public.claimed_at = None;
                action.public.connector_id = None;
                action.claim_token = None;
                if retry_is_safe {
                    action.public.finished_at = None;
                    action.public.result = None;
                } else {
                    let detail = if matches!(
                        action.public.action,
                        DeviceActionKind::Info | DeviceActionKind::Screenshot
                    ) {
                        "Lease expiree apres le TTL de file ; action de lecture abandonnee"
                    } else {
                        "Lease expiree : une action mutante n'est jamais relancee automatiquement"
                    };
                    expire_action_started_at(action, now, claimed_at, detail);
                }
            }
        }
        let busy_devices = candidate
            .iter()
            .filter(|action| action.public.status == DeviceActionStatus::Claimed)
            .map(|action| action.public.device_id.clone())
            .collect::<HashSet<_>>();
        let job = candidate
            .iter_mut()
            .find(|action| {
                action.public.status == DeviceActionStatus::Queued
                    && ready_ids.contains(action.public.device_id.as_str())
                    && !busy_devices.contains(&action.public.device_id)
                    && !action.origin_id.as_ref().is_some_and(|origin_id| {
                        revoked_origins
                            .contains_key(&(action.public.owner_id.clone(), origin_id.clone()))
                    })
            })
            .map(|action| {
                let claim_token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
                action.public.status = DeviceActionStatus::Claimed;
                action.public.updated_at = now;
                action.public.claimed_at = Some(now);
                action.public.connector_id = Some(connector_id.clone());
                action.claim_token = Some(claim_token.clone());
                DeviceConnectorJob {
                    action_id: action.public.id.clone(),
                    claim_token,
                    owner_id: action.public.owner_id.clone(),
                    device_id: action.public.device_id.clone(),
                    request: action.request.clone(),
                }
            });
        if lease_changed || job.is_some() {
            self.persist_actions(&candidate)?;
            *actions = candidate;
        }
        drop(actions);
        drop(revoked_origins);
        if lease_changed || job.is_some() {
            self.inner.changed.notify_waiters();
        }
        Ok(job)
    }

    pub fn report(
        &self,
        request: DeviceConnectorReportRequest,
    ) -> Result<DeviceActionRecord, DeviceFleetError> {
        self.report_at(request, now_ts())
    }

    pub fn report_at(
        &self,
        mut request: DeviceConnectorReportRequest,
        now: i64,
    ) -> Result<DeviceActionRecord, DeviceFleetError> {
        let connector_id = validate_connector_id(&request.connector_id)?;
        let action_id = validate_action_id(&request.action_id)?;
        if request.claim_token.len() != 64
            || !request
                .claim_token
                .chars()
                .all(|value| value.is_ascii_hexdigit())
        {
            return Err(DeviceFleetError::Validation(
                "Jeton de lease invalide".to_string(),
            ));
        }
        let revoked_origins = self.inner.revoked_origins.lock().map_err(|_| {
            DeviceFleetError::Internal("Liste des origines revoquees verrouillee".into())
        })?;
        let mut actions = self
            .inner
            .actions
            .lock()
            .map_err(|_| DeviceFleetError::Internal("File des actions verrouillee".into()))?;
        let mut candidate = actions.clone();
        let action = candidate
            .iter_mut()
            .find(|action| action.public.id == action_id)
            .ok_or(DeviceFleetError::NotFound)?;
        if action.origin_id.as_ref().is_some_and(|origin_id| {
            revoked_origins.contains_key(&(action.public.owner_id.clone(), origin_id.clone()))
        }) {
            return Err(DeviceFleetError::Conflict(
                "L'origine de cette action a ete revoquee".to_string(),
            ));
        }
        if action.public.status != DeviceActionStatus::Claimed
            || action.public.connector_id.as_deref() != Some(&connector_id)
            || action.claim_token.as_deref() != Some(request.claim_token.trim())
        {
            return Err(DeviceFleetError::Conflict(
                "Lease de controle absente ou expiree".to_string(),
            ));
        }
        if request.result.action_id != action.public.id
            || request.result.device_id != action.public.device_id
            || request.result.action != action.public.action
        {
            return Err(DeviceFleetError::Validation(
                "Le resultat ne correspond pas a l'action reclamee".to_string(),
            ));
        }
        sanitize_action_result(&mut request.result);
        action.public.status = if request.result.success {
            DeviceActionStatus::Succeeded
        } else {
            DeviceActionStatus::Failed
        };
        action.public.updated_at = now;
        action.public.finished_at = Some(now);
        action.public.result = Some(request.result);
        action.claim_token = None;
        let public = action.public.clone();
        self.persist_actions(&candidate)?;
        *actions = candidate;
        drop(actions);
        drop(revoked_origins);
        self.inner.changed.notify_waiters();
        Ok(public)
    }

    pub fn execute_local_action(
        &self,
        action_id: &str,
        request: DeviceActionRequest,
    ) -> Result<DeviceActionResult, DeviceFleetError> {
        validate_action_id(action_id)?;
        validate_action_request(&request)?;
        let tools = inspect_tools();
        let device = inspect_local_device(&tools, &request.device_id)?;
        ensure_device_supports(&device, request.action)?;
        let device_lock = {
            let mut locks = self.inner.local_device_locks.lock().map_err(|_| {
                DeviceFleetError::Internal("Verrou local des appareils indisponible".into())
            })?;
            locks
                .entry(device.id.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = device_lock
            .lock()
            .map_err(|_| DeviceFleetError::Internal("Verrou de l'appareil indisponible".into()))?;
        execute_action_on_device(action_id, &device, &request, &tools)
    }
}

pub fn inspect_local_fleet() -> Result<DeviceFleetSnapshot, DeviceFleetError> {
    let now = now_ts();
    let tools = inspect_tools();
    let mut devices = Vec::new();
    let mut errors = Vec::new();

    if tools.adb.available {
        match inspect_android_devices(&tools, now) {
            Ok(mut found) => devices.append(&mut found),
            Err(error) => errors.push(error.to_string()),
        }
    }
    if tools.idevice_id.available && devices.len() < MAX_DEVICES {
        match inspect_ios_devices(&tools, now, MAX_DEVICES - devices.len()) {
            Ok(mut found) => devices.append(&mut found),
            Err(error) => errors.push(error.to_string()),
        }
    }
    sort_and_cap_devices(&mut devices);
    let connector_id = local_connector_id();
    Ok(DeviceFleetSnapshot {
        connector_online: true,
        connector_id: Some(connector_id.clone()),
        last_seen_at: Some(now),
        expires_at: Some(now.saturating_add(DEFAULT_CONNECTOR_TTL_SECONDS)),
        devices: devices
            .into_iter()
            .map(|mut device| {
                device.connector_id = Some(connector_id.clone());
                device
            })
            .collect(),
        tools,
        error: (!errors.is_empty()).then(|| errors.join(" ")),
    })
}

fn sort_and_cap_devices(devices: &mut Vec<ControlDevice>) {
    devices.sort_by(|left, right| left.id.cmp(&right.id));
    devices.dedup_by(|left, right| left.id == right.id);
    devices.truncate(MAX_DEVICES);
}

#[cfg(feature = "desktop")]
fn desktop_device_fleet_manager() -> &'static DeviceFleetManager {
    static MANAGER: OnceLock<DeviceFleetManager> = OnceLock::new();
    MANAGER.get_or_init(DeviceFleetManager::default)
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn list_control_devices() -> Result<DeviceFleetSnapshot, String> {
    tokio::task::spawn_blocking(inspect_local_fleet)
        .await
        .map_err(|error| format!("Inventaire USB interrompu : {error}"))?
        .map_err(|error| error.to_string())
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn control_device(
    device_id: String,
    action: DeviceActionKind,
    args: Option<DeviceActionArgs>,
    confirmed: Option<bool>,
) -> Result<DeviceActionResult, String> {
    let request = DeviceActionRequest {
        device_id,
        action,
        args: args.unwrap_or_default(),
        confirmed: confirmed.unwrap_or(false),
        idempotency_key: None,
    };
    let action_id = Uuid::new_v4().to_string();
    let manager = desktop_device_fleet_manager().clone();
    tokio::task::spawn_blocking(move || manager.execute_local_action(&action_id, request))
        .await
        .map_err(|error| format!("Controle USB interrompu : {error}"))?
        .map_err(|error| error.to_string())
}

#[cfg(feature = "desktop")]
pub async fn run_device_fleet_connector() {
    let startup = crate::client_startup::client_startup_config();
    if !startup.remote_mode {
        return;
    }
    let Some(base_url) = startup
        .base_url
        .map(|value| value.trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    let Some(token) = startup.token.filter(|value| !value.trim().is_empty()) else {
        return;
    };
    let connector_id = local_connector_id();
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(20))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            eprintln!("Connecteur USB indisponible : {error}");
            return;
        }
    };

    let heartbeat_client = client.clone();
    let heartbeat_base = base_url.clone();
    let heartbeat_token = token.clone();
    let heartbeat_connector = connector_id.clone();
    let heartbeat_loop = async move {
        loop {
            let snapshot = tokio::task::spawn_blocking(inspect_local_fleet)
                .await
                .unwrap_or_else(|error| {
                    Err(DeviceFleetError::Internal(format!(
                        "Inventaire USB interrompu : {error}"
                    )))
                });
            let request = match snapshot {
                Ok(snapshot) => DeviceConnectorHeartbeatRequest {
                    connector_id: heartbeat_connector.clone(),
                    devices: snapshot.devices,
                    tools: snapshot.tools,
                    error: snapshot.error,
                },
                Err(error) => DeviceConnectorHeartbeatRequest {
                    connector_id: heartbeat_connector.clone(),
                    devices: Vec::new(),
                    tools: DeviceFleetTools::default(),
                    error: Some(error.to_string()),
                },
            };
            let _ = post_connector_json::<DeviceFleetSnapshot>(
                &heartbeat_client,
                &heartbeat_base,
                &heartbeat_token,
                "/api/device-fleet/connector/heartbeat",
                &request,
            )
            .await;
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    };

    let worker_loop = async move {
        let capacity = Arc::new(tokio::sync::Semaphore::new(
            configured_device_action_concurrency(),
        ));
        loop {
            let permit = match capacity.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => return,
            };
            let claim = post_connector_json::<DeviceConnectorClaimResponse>(
                &client,
                &base_url,
                &token,
                "/api/device-fleet/connector/claim",
                &DeviceConnectorClaimRequest {
                    connector_id: connector_id.clone(),
                },
            )
            .await;
            if let Ok(DeviceConnectorClaimResponse { job: Some(job) }) = claim {
                let job_client = client.clone();
                let job_base_url = base_url.clone();
                let job_token = token.clone();
                let job_connector_id = connector_id.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let manager = desktop_device_fleet_manager().clone();
                    let action_id = job.action_id.clone();
                    let request = job.request.clone();
                    let fallback_request = request.clone();
                    let result = tokio::task::spawn_blocking(move || {
                        manager.execute_local_action(&action_id, request)
                    })
                    .await
                    .map_err(|error| format!("Execution USB interrompue : {error}"))
                    .and_then(|value| value.map_err(|error| error.to_string()))
                    .unwrap_or_else(|error| {
                        DeviceActionResult::failed(&job.action_id, &fallback_request, error)
                    });
                    let report = DeviceConnectorReportRequest {
                        connector_id: job_connector_id,
                        action_id: job.action_id,
                        claim_token: job.claim_token,
                        result,
                    };
                    for attempt in 0..3 {
                        if post_connector_json::<DeviceActionRecord>(
                            &job_client,
                            &job_base_url,
                            &job_token,
                            "/api/device-fleet/connector/report",
                            &report,
                        )
                        .await
                        .is_ok()
                        {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(500 * (attempt + 1))).await;
                    }
                });
            } else {
                drop(permit);
                tokio::time::sleep(Duration::from_millis(750)).await;
            }
        }
    };

    tokio::join!(heartbeat_loop, worker_loop);
}

#[cfg(feature = "desktop")]
async fn post_connector_json<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    path: &str,
    body: &impl Serialize,
) -> Result<T, String> {
    let response = client
        .post(format!("{base_url}{path}"))
        .bearer_auth(token.trim())
        .json(body)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let bytes = response.bytes().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "Switch {} : {}",
            status.as_u16(),
            String::from_utf8_lossy(&bytes)
                .chars()
                .take(300)
                .collect::<String>()
        ));
    }
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

#[derive(Clone)]
struct CachedDeviceFleetTools {
    checked_at: Instant,
    tools: DeviceFleetTools,
}

fn inspect_tools() -> DeviceFleetTools {
    static CACHE: OnceLock<Mutex<Option<CachedDeviceFleetTools>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    let Ok(mut cached) = cache.lock() else {
        return inspect_tools_uncached();
    };
    if let Some(entry) = cached.as_ref() {
        if entry.checked_at.elapsed() < TOOL_STATUS_CACHE_TTL {
            return entry.tools.clone();
        }
    }
    let tools = inspect_tools_uncached();
    *cached = Some(CachedDeviceFleetTools {
        checked_at: Instant::now(),
        tools: tools.clone(),
    });
    tools
}

fn inspect_tools_uncached() -> DeviceFleetTools {
    let adb = tool_status(&tool_program("CST_ADB_PATH", "adb"), &["version"]);
    let scrcpy = tool_status(&tool_program("CST_SCRCPY_PATH", "scrcpy"), &["--version"]);
    let idevice_id = tool_status(
        &tool_program("CST_IDEVICE_ID_PATH", "idevice_id"),
        &["--version"],
    );
    let idevice_info = tool_status(
        &tool_program("CST_IDEVICEINFO_PATH", "ideviceinfo"),
        &["--version"],
    );
    let idevice_screenshot = tool_status(
        &tool_program("CST_IDEVICESCREENSHOT_PATH", "idevicescreenshot"),
        &["--help"],
    );
    let iproxy = tool_status(&tool_program("CST_IPROXY_PATH", "iproxy"), &["--version"]);
    let ssh = tool_status(&tool_program("CST_SSH_PATH", "ssh"), &["-V"]);
    let mut ios_wda = ios_wda_runtime().status.clone();
    if ios_wda.available && !iproxy.available {
        ios_wda.available = false;
        ios_wda.detail = Some(format!(
            "WDA active mais iproxy indisponible{}",
            iproxy
                .detail
                .as_deref()
                .map(|detail| format!(" : {detail}"))
                .unwrap_or_default()
        ));
    }
    let ios_ssh_key_configured = env::var("CST_IOS_SSH_KEY")
        .ok()
        .map(|value| Path::new(value.trim()).is_file())
        .unwrap_or(false);
    let ios_ui_tool_configured = ios_ui_tool().is_some();
    DeviceFleetTools {
        adb,
        scrcpy,
        idevice_id,
        idevice_info,
        idevice_screenshot,
        iproxy,
        ssh,
        ios_wda,
        ios_ssh_key_configured,
        ios_ui_tool_configured,
    }
}

fn tool_status(program: &str, version_args: &[&str]) -> DeviceToolStatus {
    let args = version_args.iter().map(|value| value.to_string()).collect();
    tool_status_from_probe(run_capped_command(
        program,
        args,
        Duration::from_secs(2),
        4 * 1024,
    ))
}

fn tool_status_from_probe(probe: Result<CappedCommandOutput, String>) -> DeviceToolStatus {
    match probe {
        Ok(output) if output.success => DeviceToolStatus {
            available: true,
            detail: None,
        },
        Ok(output) if output.timed_out => DeviceToolStatus {
            available: false,
            detail: Some("Le test de disponibilite a expire".to_string()),
        },
        Ok(output) => DeviceToolStatus {
            available: false,
            detail: Some(
                output
                    .status_code
                    .map(|code| format!("Le test de disponibilite a echoue (code {code})"))
                    .unwrap_or_else(|| "Le test de disponibilite a echoue".to_string()),
            ),
        },
        Err(_) => DeviceToolStatus {
            available: false,
            detail: Some("Executable introuvable".to_string()),
        },
    }
}

fn inspect_local_device(
    tools: &DeviceFleetTools,
    device_id: &str,
) -> Result<ControlDevice, DeviceFleetError> {
    let (platform, serial) = parse_local_device_target(device_id)?;
    let now = now_ts();
    match platform {
        DevicePlatform::Android => inspect_android_target_device(tools, &serial, now),
        DevicePlatform::Ios => inspect_ios_target_device(tools, &serial, now),
    }
}

fn inspect_android_devices(
    tools: &DeviceFleetTools,
    now: i64,
) -> Result<Vec<ControlDevice>, DeviceFleetError> {
    let adb = tool_program("CST_ADB_PATH", "adb");
    let output = run_capped_command(
        &adb,
        vec!["devices".into(), "-l".into()],
        Duration::from_secs(5),
        MAX_ACTION_OUTPUT_BYTES,
    )
    .map_err(DeviceFleetError::Unavailable)?;
    if !output.success {
        return Err(DeviceFleetError::Unavailable(format_command_error(
            "Inventaire ADB impossible",
            &output,
        )));
    }
    let devices = parse_adb_devices(&String::from_utf8_lossy(&output.stdout), now);
    Ok(bounded_parallel_map_ordered(
        devices,
        MAX_METADATA_PROBES,
        |mut device| {
            device.capabilities = android_capabilities(device.ready, tools.scrcpy.available);
            enrich_android_device_metadata(&adb, device)
        },
    ))
}

fn enrich_android_device_metadata(adb: &str, mut device: ControlDevice) -> ControlDevice {
    if device.ready {
        let properties = run_capped_command(
            adb,
            vec![
                "-s".into(),
                device.serial.clone(),
                "shell".into(),
                "getprop".into(),
            ],
            Duration::from_secs(5),
            MAX_ACTION_OUTPUT_BYTES,
        );
        if let Ok(properties) = properties {
            if properties.success {
                let parsed = parse_android_properties(&String::from_utf8_lossy(&properties.stdout));
                device.model = parsed
                    .get("ro.product.model")
                    .cloned()
                    .filter(|value| !value.is_empty())
                    .or_else(|| device.model.clone());
                device.name = parsed
                    .get("ro.product.device")
                    .cloned()
                    .filter(|value| !value.is_empty())
                    .or_else(|| device.name.clone());
                device.os_version = parsed
                    .get("ro.build.version.release")
                    .cloned()
                    .filter(|value| !value.is_empty());
            }
        }
    }
    device
}

fn inspect_android_target_device(
    tools: &DeviceFleetTools,
    serial: &str,
    now: i64,
) -> Result<ControlDevice, DeviceFleetError> {
    if !tools.adb.available {
        return Err(DeviceFleetError::Unavailable(
            "ADB n'est pas disponible".to_string(),
        ));
    }
    let adb = tool_program("CST_ADB_PATH", "adb");
    let output = run_capped_command(
        &adb,
        vec!["-s".into(), serial.to_string(), "get-state".into()],
        Duration::from_secs(5),
        MAX_ACTION_OUTPUT_BYTES,
    )
    .map_err(DeviceFleetError::Unavailable)?;
    if !output.success {
        return Err(DeviceFleetError::Unavailable(format_command_error(
            "Verification ADB ciblee impossible",
            &output,
        )));
    }
    let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if state != "device" {
        return Err(DeviceFleetError::Unavailable(format!(
            "L'appareil android:{serial} n'est pas pret ({})",
            state.chars().take(80).collect::<String>()
        )));
    }
    let device = ControlDevice {
        id: format!("android:{serial}"),
        platform: DevicePlatform::Android,
        transport: DeviceTransport::Usb,
        serial: serial.to_string(),
        name: None,
        model: None,
        os_version: None,
        state: DeviceConnectionState::Ready,
        ready: true,
        jailbreak_ready: false,
        capabilities: android_capabilities(true, tools.scrcpy.available),
        connector_id: None,
        last_seen_at: now,
        error: None,
    };
    Ok(enrich_android_device_metadata(&adb, device))
}

fn parse_adb_devices(output: &str, now: i64) -> Vec<ControlDevice> {
    let mut devices = Vec::new();
    for line in output.lines().map(str::trim) {
        if line.is_empty() || line.starts_with("List of devices attached") || line.starts_with('*')
        {
            continue;
        }
        let columns = line.split_whitespace().collect::<Vec<_>>();
        if columns.len() < 2 {
            continue;
        }
        let serial = columns[0];
        if !is_usb_android_serial(serial) || validate_serial(serial).is_err() {
            continue;
        }
        let state = match columns[1] {
            "device" => DeviceConnectionState::Ready,
            "unauthorized" => DeviceConnectionState::Unauthorized,
            "offline" => DeviceConnectionState::Offline,
            _ => DeviceConnectionState::Unavailable,
        };
        let metadata = columns[2..]
            .iter()
            .filter_map(|column| column.split_once(':'))
            .collect::<HashMap<_, _>>();
        let model = metadata.get("model").map(|value| value.replace('_', " "));
        let name = metadata
            .get("device")
            .or_else(|| metadata.get("product"))
            .map(|value| value.replace('_', " "));
        let ready = state == DeviceConnectionState::Ready;
        devices.push(ControlDevice {
            id: format!("android:{serial}"),
            platform: DevicePlatform::Android,
            transport: DeviceTransport::Usb,
            serial: serial.to_string(),
            name,
            model,
            os_version: None,
            state,
            ready,
            jailbreak_ready: false,
            capabilities: Vec::new(),
            connector_id: None,
            last_seen_at: now,
            error: match state {
                DeviceConnectionState::Unauthorized => {
                    Some("Validez l'autorisation ADB sur l'appareil".to_string())
                }
                DeviceConnectionState::Offline => Some("Appareil ADB hors ligne".to_string()),
                DeviceConnectionState::Unavailable => {
                    Some(format!("Etat ADB non pris en charge : {}", columns[1]))
                }
                DeviceConnectionState::Ready => None,
            },
        });
    }
    devices.sort_by(|left, right| left.id.cmp(&right.id));
    devices.dedup_by(|left, right| left.id == right.id);
    devices.truncate(MAX_DEVICES);
    devices
}

fn parse_android_properties(output: &str) -> HashMap<String, String> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let split = line.find("]: [")?;
            let key = line.strip_prefix('[')?.get(..split - 1)?;
            let value = line.get(split + 4..)?.strip_suffix(']')?;
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}

fn inspect_ios_devices(
    tools: &DeviceFleetTools,
    now: i64,
    limit: usize,
) -> Result<Vec<ControlDevice>, DeviceFleetError> {
    let idevice_id = tool_program("CST_IDEVICE_ID_PATH", "idevice_id");
    let output = run_capped_command(
        &idevice_id,
        vec!["-l".into()],
        Duration::from_secs(5),
        MAX_ACTION_OUTPUT_BYTES,
    )
    .map_err(DeviceFleetError::Unavailable)?;
    if !output.success {
        return Err(DeviceFleetError::Unavailable(format_command_error(
            "Inventaire iOS USB impossible",
            &output,
        )));
    }
    let mut ids = parse_idevice_ids(&String::from_utf8_lossy(&output.stdout));
    let wda = effective_ios_wda_provider(tools);
    invalidate_disappeared_ios_wda_devices(wda, &ids);
    ids.truncate(limit.min(MAX_DEVICES));
    let idevice_info = tool_program("CST_IDEVICEINFO_PATH", "ideviceinfo");
    let jailbreak_ready =
        tools.iproxy.available && tools.ssh.available && tools.ios_ssh_key_configured;
    Ok(bounded_parallel_map_ordered(
        ids,
        MAX_METADATA_PROBES,
        |udid| inspect_ios_device_metadata(tools, &idevice_info, udid, jailbreak_ready, wda, now),
    ))
}

fn inspect_ios_device_metadata(
    tools: &DeviceFleetTools,
    idevice_info: &str,
    udid: String,
    jailbreak_ready: bool,
    wda: Option<&IosWdaProvider>,
    now: i64,
) -> ControlDevice {
    let info = if tools.idevice_info.available {
        run_capped_command(
            idevice_info,
            vec!["-u".into(), udid.clone()],
            Duration::from_secs(6),
            MAX_ACTION_OUTPUT_BYTES,
        )
        .ok()
    } else {
        None
    };
    let paired = info.as_ref().is_some_and(|output| output.success);
    let properties = info
        .as_ref()
        .filter(|output| output.success)
        .map(|output| parse_ios_properties(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_default();
    let (wda_probe, wda_error) = if paired {
        match wda {
            Some(provider) => match provider.probe(&udid) {
                Ok(probe) if probe.ready => (Some(probe), None),
                Ok(_) => (
                    None,
                    Some("WDA n'a pas declare l'appareil pret".to_string()),
                ),
                Err(error) => (
                    None,
                    Some(
                        format!("WDA indisponible sur cet appareil : {}", error.message())
                            .chars()
                            .take(300)
                            .collect(),
                    ),
                ),
            },
            None => (None, None),
        }
    } else {
        (None, None)
    };
    let state = if paired {
        DeviceConnectionState::Ready
    } else {
        DeviceConnectionState::Unauthorized
    };
    ControlDevice {
        id: format!("ios:{udid}"),
        platform: DevicePlatform::Ios,
        transport: DeviceTransport::Usb,
        serial: udid,
        name: properties.get("DeviceName").cloned(),
        model: properties
            .get("ProductType")
            .cloned()
            .or_else(|| properties.get("HardwareModel").cloned()),
        os_version: properties.get("ProductVersion").cloned(),
        state,
        ready: paired,
        jailbreak_ready: paired && jailbreak_ready,
        capabilities: ios_capabilities(paired, jailbreak_ready, tools, wda_probe.as_ref()),
        connector_id: None,
        last_seen_at: now,
        error: if paired {
            wda_error
        } else {
            Some("Appareil iOS non jumele : validez la relation de confiance USB".to_string())
        },
    }
}

fn inspect_ios_target_device(
    tools: &DeviceFleetTools,
    udid: &str,
    now: i64,
) -> Result<ControlDevice, DeviceFleetError> {
    if !tools.idevice_info.available {
        return Err(DeviceFleetError::Unavailable(
            "ideviceinfo n'est pas disponible".to_string(),
        ));
    }
    let idevice_info = tool_program("CST_IDEVICEINFO_PATH", "ideviceinfo");
    let jailbreak_ready =
        tools.iproxy.available && tools.ssh.available && tools.ios_ssh_key_configured;
    let device = inspect_ios_device_metadata(
        tools,
        &idevice_info,
        udid.to_string(),
        jailbreak_ready,
        effective_ios_wda_provider(tools),
        now,
    );
    if !device.ready {
        return Err(DeviceFleetError::Unavailable(
            device
                .error
                .clone()
                .unwrap_or_else(|| format!("L'appareil ios:{udid} n'est pas pret")),
        ));
    }
    Ok(device)
}

fn parse_idevice_ids(output: &str) -> Vec<String> {
    let mut ids = output
        .lines()
        .map(str::trim)
        .filter(|value| !value.is_empty() && validate_serial(value).is_ok())
        .map(str::to_string)
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    ids.truncate(MAX_DEVICES);
    ids
}

fn parse_ios_properties(output: &str) -> HashMap<String, String> {
    output
        .lines()
        .filter_map(|line| line.split_once(": "))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .collect()
}

fn android_capabilities(ready: bool, scrcpy_available: bool) -> Vec<DeviceActionKind> {
    if !ready {
        return Vec::new();
    }
    let mut capabilities = vec![
        DeviceActionKind::Info,
        DeviceActionKind::Screenshot,
        DeviceActionKind::Tap,
        DeviceActionKind::Swipe,
        DeviceActionKind::TypeText,
        DeviceActionKind::KeyEvent,
        DeviceActionKind::OpenApp,
        DeviceActionKind::Shell,
        DeviceActionKind::PushFile,
    ];
    if scrcpy_available {
        capabilities.push(DeviceActionKind::OpenScreen);
    }
    capabilities
}

fn ios_capabilities(
    paired: bool,
    jailbreak_ready: bool,
    tools: &DeviceFleetTools,
    wda_probe: Option<&IosWdaProbe>,
) -> Vec<DeviceActionKind> {
    if !paired {
        return Vec::new();
    }
    let mut capabilities = vec![DeviceActionKind::Info];
    if tools.idevice_screenshot.available {
        capabilities.push(DeviceActionKind::Screenshot);
    }
    if jailbreak_ready {
        capabilities.push(DeviceActionKind::Shell);
        if tools.ios_ui_tool_configured {
            capabilities.extend([
                DeviceActionKind::Tap,
                DeviceActionKind::Swipe,
                DeviceActionKind::TypeText,
                DeviceActionKind::KeyEvent,
                DeviceActionKind::OpenApp,
            ]);
        }
    }
    if let Some(probe) = wda_probe.filter(|probe| probe.ready) {
        for capability in &probe.capabilities {
            let action = match capability {
                IosWdaCapability::Tap => DeviceActionKind::Tap,
                IosWdaCapability::Swipe => DeviceActionKind::Swipe,
                IosWdaCapability::TypeText => DeviceActionKind::TypeText,
                IosWdaCapability::KeyEvent => DeviceActionKind::KeyEvent,
                IosWdaCapability::OpenApp => DeviceActionKind::OpenApp,
                IosWdaCapability::Screenshot => DeviceActionKind::Screenshot,
            };
            if !capabilities.contains(&action) {
                capabilities.push(action);
            }
        }
    }
    capabilities
}

fn execute_action_on_device(
    action_id: &str,
    device: &ControlDevice,
    request: &DeviceActionRequest,
    tools: &DeviceFleetTools,
) -> Result<DeviceActionResult, DeviceFleetError> {
    let started_at = now_ts();
    let execution = match device.platform {
        DevicePlatform::Android => execute_android_action(device, request),
        DevicePlatform::Ios => execute_ios_action(device, request, tools),
    };
    let finished_at = now_ts();
    match execution {
        Ok(mut result) => {
            result.action_id = action_id.to_string();
            result.device_id = device.id.clone();
            result.action = request.action;
            result.started_at = started_at;
            result.finished_at = finished_at;
            Ok(result)
        }
        Err(error) => Ok(DeviceActionResult {
            action_id: action_id.to_string(),
            device_id: device.id.clone(),
            action: request.action,
            success: false,
            detail: error.to_string(),
            stdout: None,
            stderr: None,
            data_base64: None,
            mime_type: None,
            started_at,
            finished_at,
            truncated: false,
        }),
    }
}

fn execute_android_action(
    device: &ControlDevice,
    request: &DeviceActionRequest,
) -> Result<DeviceActionResult, DeviceFleetError> {
    let adb = tool_program("CST_ADB_PATH", "adb");
    let timeout = action_timeout(request);
    match request.action {
        DeviceActionKind::OpenScreen => {
            let scrcpy = tool_program("CST_SCRCPY_PATH", "scrcpy");
            Command::new(&scrcpy)
                .args(["--serial", device.serial.as_str()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| {
                    DeviceFleetError::Unavailable("Impossible de lancer scrcpy".to_string())
                })?;
            Ok(empty_success("Fenetre scrcpy ouverte"))
        }
        DeviceActionKind::Screenshot => {
            let output = run_capped_command(
                &adb,
                vec![
                    "-s".into(),
                    device.serial.clone(),
                    "exec-out".into(),
                    "screencap".into(),
                    "-p".into(),
                ],
                timeout,
                MAX_SCREENSHOT_BYTES,
            )
            .map_err(DeviceFleetError::Unavailable)?;
            if !output.success {
                return Err(DeviceFleetError::Unavailable(format_command_error(
                    "Capture Android impossible",
                    &output,
                )));
            }
            if output.stdout.is_empty() {
                return Err(DeviceFleetError::Unavailable(
                    "ADB n'a renvoye aucune image".to_string(),
                ));
            }
            Ok(DeviceActionResult {
                data_base64: Some(BASE64_STANDARD.encode(&output.stdout)),
                mime_type: Some("image/png".to_string()),
                detail: "Capture Android effectuee".to_string(),
                truncated: output.truncated,
                ..empty_success("")
            })
        }
        DeviceActionKind::Info => run_android_text_action(
            &adb,
            device,
            vec!["shell".into(), "getprop".into()],
            timeout,
            "Informations Android lues",
        ),
        DeviceActionKind::Tap => run_android_text_action(
            &adb,
            device,
            vec![
                "shell".into(),
                "input".into(),
                "tap".into(),
                request.args.x.expect("valide").to_string(),
                request.args.y.expect("valide").to_string(),
            ],
            timeout,
            "Appui envoye",
        ),
        DeviceActionKind::Swipe => run_android_text_action(
            &adb,
            device,
            vec![
                "shell".into(),
                "input".into(),
                "swipe".into(),
                request.args.start_x.expect("valide").to_string(),
                request.args.start_y.expect("valide").to_string(),
                request.args.end_x.expect("valide").to_string(),
                request.args.end_y.expect("valide").to_string(),
                request.args.duration_ms.unwrap_or(300).to_string(),
            ],
            timeout,
            "Glissement envoye",
        ),
        DeviceActionKind::TypeText => run_android_text_action(
            &adb,
            device,
            vec![
                "shell".into(),
                "input".into(),
                "text".into(),
                escape_android_input_text(request.args.text.as_deref().expect("valide"))?,
            ],
            timeout,
            "Texte saisi",
        ),
        DeviceActionKind::KeyEvent => run_android_text_action(
            &adb,
            device,
            vec![
                "shell".into(),
                "input".into(),
                "keyevent".into(),
                request.args.key.as_deref().expect("valide").to_string(),
            ],
            timeout,
            "Touche envoyee",
        ),
        DeviceActionKind::OpenApp => run_android_text_action(
            &adb,
            device,
            vec![
                "shell".into(),
                "monkey".into(),
                "-p".into(),
                request.args.app_id.as_deref().expect("valide").to_string(),
                "-c".into(),
                "android.intent.category.LAUNCHER".into(),
                "1".into(),
            ],
            timeout,
            "Application ouverte",
        ),
        DeviceActionKind::Shell => run_android_text_action(
            &adb,
            device,
            vec![
                "shell".into(),
                request.args.command.as_deref().expect("valide").to_string(),
            ],
            timeout,
            "Commande executee sur Android",
        ),
        DeviceActionKind::PushFile => run_android_push_file(&adb, device, request, timeout),
    }
}

fn run_android_push_file(
    adb: &str,
    device: &ControlDevice,
    request: &DeviceActionRequest,
    timeout: Duration,
) -> Result<DeviceActionResult, DeviceFleetError> {
    let local_path = request.args.local_path.as_deref().unwrap_or("");
    let remote_path = request.args.remote_path.as_deref().unwrap_or("");
    if local_path.is_empty() {
        return Err(DeviceFleetError::Validation(
            "localPath est requis pour pousser un fichier".to_string(),
        ));
    }
    if remote_path.is_empty() || !remote_path.starts_with('/') {
        return Err(DeviceFleetError::Validation(
            "remotePath doit etre un chemin absolu sur l'appareil (commencant par /)".to_string(),
        ));
    }
    if !Path::new(local_path).is_file() {
        return Err(DeviceFleetError::Validation(format!(
            "Le fichier local n'existe pas ou n'est pas lisible : {local_path}"
        )));
    }
    let output = run_capped_command(
        adb,
        vec![
            "-s".into(),
            device.serial.clone(),
            "push".into(),
            local_path.to_string(),
            remote_path.to_string(),
        ],
        timeout,
        MAX_ACTION_OUTPUT_BYTES,
    )
    .map_err(DeviceFleetError::Unavailable)?;
    command_result(output, "Fichier pousse sur l'appareil")
}

fn run_android_text_action(
    adb: &str,
    device: &ControlDevice,
    mut args: Vec<String>,
    timeout: Duration,
    success_detail: &str,
) -> Result<DeviceActionResult, DeviceFleetError> {
    let mut full_args = vec!["-s".into(), device.serial.clone()];
    full_args.append(&mut args);
    let output = run_capped_command(adb, full_args, timeout, MAX_ACTION_OUTPUT_BYTES)
        .map_err(DeviceFleetError::Unavailable)?;
    command_result(output, success_detail)
}

fn execute_ios_action(
    device: &ControlDevice,
    request: &DeviceActionRequest,
    tools: &DeviceFleetTools,
) -> Result<DeviceActionResult, DeviceFleetError> {
    let timeout = action_timeout(request);
    match request.action {
        DeviceActionKind::Info => {
            let program = tool_program("CST_IDEVICEINFO_PATH", "ideviceinfo");
            let output = run_capped_command(
                &program,
                vec!["-u".into(), device.serial.clone()],
                timeout,
                MAX_ACTION_OUTPUT_BYTES,
            )
            .map_err(DeviceFleetError::Unavailable)?;
            command_result(output, "Informations iOS lues")
        }
        DeviceActionKind::Screenshot => {
            execute_ios_screenshot_prefer_wda(device, timeout, effective_ios_wda_provider(tools))
        }
        DeviceActionKind::Shell => ios_ssh_action(
            device,
            request.args.command.as_deref().expect("valide").to_string(),
            timeout,
            "Commande executee sur iOS",
        ),
        DeviceActionKind::PushFile => Err(DeviceFleetError::Unavailable(
            "Le transfert de fichier n'est pas pris en charge sur iOS".to_string(),
        )),
        DeviceActionKind::OpenScreen => Err(DeviceFleetError::Unavailable(
            "Aucun outil de mirroring iOS jailbreak n'est configure".to_string(),
        )),
        DeviceActionKind::Tap
        | DeviceActionKind::Swipe
        | DeviceActionKind::TypeText
        | DeviceActionKind::KeyEvent
        | DeviceActionKind::OpenApp => {
            execute_ios_ui_action(device, request, timeout, effective_ios_wda_provider(tools))
        }
    }
}

fn ios_wda_capability_for_action(action: &IosWdaAction) -> IosWdaCapability {
    match action {
        IosWdaAction::Tap { .. } => IosWdaCapability::Tap,
        IosWdaAction::Swipe { .. } => IosWdaCapability::Swipe,
        IosWdaAction::TypeText { .. } => IosWdaCapability::TypeText,
        IosWdaAction::KeyEvent { .. } => IosWdaCapability::KeyEvent,
        IosWdaAction::OpenApp { .. } => IosWdaCapability::OpenApp,
        IosWdaAction::Screenshot => IosWdaCapability::Screenshot,
    }
}

fn ios_wda_coordinate(value: Option<i32>, name: &str) -> Result<u32, DeviceFleetError> {
    let value = value.ok_or_else(|| {
        DeviceFleetError::Validation(format!("{name} est requis pour l'action WDA"))
    })?;
    u32::try_from(value).map_err(|_| {
        DeviceFleetError::Validation(format!("{name} doit etre positif pour l'action WDA"))
    })
}

fn ios_wda_action_for_request(
    request: &DeviceActionRequest,
) -> Result<Option<IosWdaAction>, DeviceFleetError> {
    let action = match request.action {
        DeviceActionKind::Tap => Some(IosWdaAction::Tap {
            x: ios_wda_coordinate(request.args.x, "x")?,
            y: ios_wda_coordinate(request.args.y, "y")?,
        }),
        DeviceActionKind::Swipe => Some(IosWdaAction::Swipe {
            start_x: ios_wda_coordinate(request.args.start_x, "startX")?,
            start_y: ios_wda_coordinate(request.args.start_y, "startY")?,
            end_x: ios_wda_coordinate(request.args.end_x, "endX")?,
            end_y: ios_wda_coordinate(request.args.end_y, "endY")?,
            duration_ms: request.args.duration_ms.unwrap_or(300),
        }),
        DeviceActionKind::TypeText => Some(IosWdaAction::TypeText {
            text: request
                .args
                .text
                .clone()
                .ok_or_else(|| DeviceFleetError::Validation("text est requis".to_string()))?,
        }),
        DeviceActionKind::KeyEvent => {
            let key = request
                .args
                .key
                .as_deref()
                .ok_or_else(|| DeviceFleetError::Validation("key est requis".to_string()))?
                .to_ascii_uppercase();
            matches!(key.as_str(), "HOME" | "VOLUME_UP" | "VOLUME_DOWN")
                .then_some(IosWdaAction::KeyEvent { key })
        }
        DeviceActionKind::OpenApp => Some(IosWdaAction::OpenApp {
            bundle_id: request
                .args
                .app_id
                .clone()
                .ok_or_else(|| DeviceFleetError::Validation("appId est requis".to_string()))?,
        }),
        DeviceActionKind::Screenshot => Some(IosWdaAction::Screenshot),
        DeviceActionKind::Info
        | DeviceActionKind::OpenScreen
        | DeviceActionKind::Shell
        | DeviceActionKind::PushFile => None,
    };
    Ok(action)
}

fn map_ios_wda_error(error: IosWdaError, operation: &str) -> DeviceFleetError {
    let detail = format!("{operation} via WDA impossible : {}", error.message());
    match error.kind() {
        IosWdaErrorKind::InvalidInput => DeviceFleetError::Validation(detail),
        IosWdaErrorKind::AmbiguousMutation => DeviceFleetError::Conflict(format!(
            "{detail}; resultat potentiellement applique, aucune relance ni aucun repli SSH"
        )),
        IosWdaErrorKind::Unavailable
        | IosWdaErrorKind::Timeout
        | IosWdaErrorKind::Protocol
        | IosWdaErrorKind::InvalidSession
        | IosWdaErrorKind::ResponseTooLarge => DeviceFleetError::Unavailable(detail),
    }
}

fn execute_ios_ui_action(
    device: &ControlDevice,
    request: &DeviceActionRequest,
    timeout: Duration,
    wda: Option<&IosWdaProvider>,
) -> Result<DeviceActionResult, DeviceFleetError> {
    let wda_action = ios_wda_action_for_request(request)?;
    if let (Some(provider), Some(wda_action)) = (wda, wda_action) {
        let capability = ios_wda_capability_for_action(&wda_action);
        if provider
            .probe(&device.serial)
            .is_ok_and(|probe| probe.ready && probe.capabilities.contains(&capability))
        {
            // A partir de cet appel, une mutation a pu atteindre l'appareil. Tout
            // resultat, y compris une erreur ambigue, est terminal et ne doit
            // jamais etre rejoue via SSH.
            return provider
                .execute(&device.serial, wda_action)
                .map(|result| empty_success(&result.detail))
                .map_err(|error| map_ios_wda_error(error, "Action iOS"));
        }
    }
    execute_ios_jailbreak_ui_action(device, request, timeout)
}

fn execute_ios_jailbreak_ui_action(
    device: &ControlDevice,
    request: &DeviceActionRequest,
    timeout: Duration,
) -> Result<DeviceActionResult, DeviceFleetError> {
    let tool = ios_ui_tool().ok_or_else(|| {
        DeviceFleetError::Unavailable(
            "Configurez CST_IOS_UI_TOOL avec un outil distant jailbreak compatible".to_string(),
        )
    })?;
    let (subcommand, args, detail) = match request.action {
        DeviceActionKind::Tap => (
            "tap",
            vec![
                request.args.x.expect("valide").to_string(),
                request.args.y.expect("valide").to_string(),
            ],
            "Appui iOS envoye",
        ),
        DeviceActionKind::Swipe => (
            "swipe",
            vec![
                request.args.start_x.expect("valide").to_string(),
                request.args.start_y.expect("valide").to_string(),
                request.args.end_x.expect("valide").to_string(),
                request.args.end_y.expect("valide").to_string(),
                request.args.duration_ms.unwrap_or(300).to_string(),
            ],
            "Glissement iOS envoye",
        ),
        DeviceActionKind::TypeText => (
            "text",
            vec![request.args.text.as_deref().expect("valide").to_string()],
            "Texte iOS saisi",
        ),
        DeviceActionKind::KeyEvent => (
            "key",
            vec![request.args.key.as_deref().expect("valide").to_string()],
            "Touche iOS envoyee",
        ),
        DeviceActionKind::OpenApp => (
            "open_app",
            vec![request.args.app_id.as_deref().expect("valide").to_string()],
            "Application iOS ouverte",
        ),
        _ => unreachable!("seules les actions UI iOS mutantes arrivent ici"),
    };
    let mut remote_parts = vec![tool, subcommand.to_string()];
    remote_parts.extend(args);
    let remote_command = remote_parts
        .iter()
        .map(|value| posix_shell_quote(value))
        .collect::<Vec<_>>()
        .join(" ");
    ios_ssh_action(device, remote_command, timeout, detail)
}

fn execute_ios_screenshot_prefer_wda(
    device: &ControlDevice,
    timeout: Duration,
    wda: Option<&IosWdaProvider>,
) -> Result<DeviceActionResult, DeviceFleetError> {
    let mut wda_failure = None;
    if let Some(provider) = wda {
        match provider.probe(&device.serial) {
            Ok(probe)
                if probe.ready && probe.capabilities.contains(&IosWdaCapability::Screenshot) =>
            {
                match provider.execute(&device.serial, IosWdaAction::Screenshot) {
                    Ok(result) => match ios_wda_screenshot_result(result) {
                        Ok(result) => return Ok(result),
                        Err(error) => wda_failure = Some(error.to_string()),
                    },
                    Err(error) => {
                        wda_failure = Some(map_ios_wda_error(error, "Capture iOS").to_string())
                    }
                }
            }
            Ok(_) => wda_failure = Some("WDA ne publie pas la capacite de capture".to_string()),
            Err(error) => wda_failure = Some(map_ios_wda_error(error, "Sonde iOS").to_string()),
        }
    }

    match execute_ios_screenshot(device, timeout) {
        Ok(mut result) => {
            if wda_failure.is_some() {
                result.detail.push_str(" (repli local idevicescreenshot)");
            }
            Ok(result)
        }
        Err(local_error) => {
            if let Some(wda_failure) = wda_failure {
                Err(DeviceFleetError::Unavailable(format!(
                    "{wda_failure}; repli local impossible : {local_error}"
                )))
            } else {
                Err(local_error)
            }
        }
    }
}

fn ios_wda_screenshot_result(
    result: IosWdaActionResult,
) -> Result<DeviceActionResult, DeviceFleetError> {
    let screenshot = result.screenshot.ok_or_else(|| {
        DeviceFleetError::Unavailable("WDA n'a retourne aucune capture".to_string())
    })?;
    if screenshot.bytes.is_empty() || screenshot.bytes.len() > MAX_SCREENSHOT_BYTES {
        return Err(DeviceFleetError::Unavailable(
            "La capture WDA est vide ou depasse 8 Mio".to_string(),
        ));
    }
    let mime_type = screenshot.mime_type.trim().to_ascii_lowercase();
    if !screenshot_signature_matches(&mime_type, &screenshot.bytes) {
        return Err(DeviceFleetError::Unavailable(
            "Le type MIME ou la signature de la capture WDA est invalide".to_string(),
        ));
    }
    let encoded = BASE64_STANDARD.encode(&screenshot.bytes);
    let max_encoded_len = ((MAX_SCREENSHOT_BYTES + 2) / 3) * 4;
    if encoded.len() > max_encoded_len {
        return Err(DeviceFleetError::Unavailable(
            "La capture WDA encodee depasse la taille autorisee".to_string(),
        ));
    }
    Ok(DeviceActionResult {
        data_base64: Some(encoded),
        mime_type: Some(mime_type),
        detail: result.detail.chars().take(300).collect(),
        ..empty_success("")
    })
}

fn execute_ios_screenshot(
    device: &ControlDevice,
    timeout: Duration,
) -> Result<DeviceActionResult, DeviceFleetError> {
    let output_path = env::temp_dir().join(format!("cst-ios-{}.png", Uuid::new_v4()));
    let program = tool_program("CST_IDEVICESCREENSHOT_PATH", "idevicescreenshot");
    let output = run_capped_command(
        &program,
        vec![
            "-u".into(),
            device.serial.clone(),
            output_path.to_string_lossy().to_string(),
        ],
        timeout,
        MAX_ACTION_OUTPUT_BYTES,
    )
    .map_err(DeviceFleetError::Unavailable)?;
    if !output.success {
        let _ = fs::remove_file(&output_path);
        return Err(DeviceFleetError::Unavailable(format_command_error(
            "Capture iOS impossible",
            &output,
        )));
    }
    let image = fs::read(&output_path)
        .map_err(|error| DeviceFleetError::Unavailable(format!("Capture iOS illisible : {error}")));
    let _ = fs::remove_file(&output_path);
    let image = image?;
    if image.is_empty() || image.len() > MAX_SCREENSHOT_BYTES {
        return Err(DeviceFleetError::Unavailable(
            "La capture iOS est vide ou depasse 8 Mio".to_string(),
        ));
    }
    let mime_type = screenshot_mime_type(&image).ok_or_else(|| {
        DeviceFleetError::Unavailable(
            "La capture iOS locale n'a pas une signature d'image acceptee".to_string(),
        )
    })?;
    Ok(DeviceActionResult {
        data_base64: Some(BASE64_STANDARD.encode(image)),
        mime_type: Some(mime_type.to_string()),
        detail: "Capture iOS effectuee".to_string(),
        ..empty_success("")
    })
}

fn ios_ssh_action(
    device: &ControlDevice,
    remote_command: String,
    timeout: Duration,
    success_detail: &str,
) -> Result<DeviceActionResult, DeviceFleetError> {
    let key = env::var("CST_IOS_SSH_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && Path::new(value).is_file())
        .ok_or_else(|| {
            DeviceFleetError::Unavailable(
                "Configurez CST_IOS_SSH_KEY avec une cle privee existante".to_string(),
            )
        })?;
    let user = env::var("CST_IOS_SSH_USER").unwrap_or_else(|_| "root".to_string());
    validate_ssh_user(&user)?;
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| DeviceFleetError::Unavailable(format!("Port iproxy : {error}")))?;
    let port = listener
        .local_addr()
        .map_err(|error| DeviceFleetError::Unavailable(format!("Port iproxy : {error}")))?
        .port();
    drop(listener);

    let iproxy = tool_program("CST_IPROXY_PATH", "iproxy");
    let mut forward_command = Command::new(&iproxy);
    configure_silent_command(&mut forward_command);
    let mut forward = forward_command
        .args(["-u", device.serial.as_str(), &format!("{port}:22")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| DeviceFleetError::Unavailable("Impossible de lancer iproxy".to_string()))?;
    let ready_deadline = Instant::now() + Duration::from_secs(4);
    let mut tunnel_ready = false;
    while Instant::now() < ready_deadline {
        if TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}")
                .parse()
                .expect("adresse locale valide"),
            Duration::from_millis(150),
        )
        .is_ok()
        {
            tunnel_ready = true;
            break;
        }
        if forward.try_wait().ok().flatten().is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    if !tunnel_ready {
        let _ = forward.kill();
        let _ = forward.wait();
        return Err(DeviceFleetError::Unavailable(
            "iproxy n'a pas ouvert le tunnel USB vers SSH".to_string(),
        ));
    }

    let ssh = tool_program("CST_SSH_PATH", "ssh");
    let host_alias = format!("cst-ios-{}", device.serial);
    let args = vec![
        "-p".into(),
        port.to_string(),
        "-i".into(),
        key.clone(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "IdentitiesOnly=yes".into(),
        "-o".into(),
        "PasswordAuthentication=no".into(),
        "-o".into(),
        "KbdInteractiveAuthentication=no".into(),
        "-o".into(),
        "ConnectTimeout=5".into(),
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        "-o".into(),
        format!("HostKeyAlias={host_alias}"),
        format!("{user}@127.0.0.1"),
        "--".into(),
        remote_command,
    ];
    let output = run_capped_command(&ssh, args, timeout, MAX_ACTION_OUTPUT_BYTES);
    let _ = forward.kill();
    let _ = forward.wait();
    let mut output = output.map_err(DeviceFleetError::Unavailable)?;
    redact_bytes(&mut output.stderr, key.as_bytes());
    redact_bytes(&mut output.stdout, key.as_bytes());
    command_result(output, success_detail)
}

fn command_result(
    output: CappedCommandOutput,
    success_detail: &str,
) -> Result<DeviceActionResult, DeviceFleetError> {
    if !output.success {
        return Err(DeviceFleetError::Unavailable(format_command_error(
            "Action appareil impossible",
            &output,
        )));
    }
    Ok(DeviceActionResult {
        action_id: String::new(),
        device_id: String::new(),
        action: DeviceActionKind::Info,
        success: true,
        detail: success_detail.to_string(),
        stdout: bytes_to_optional_text(&output.stdout),
        stderr: bytes_to_optional_text(&output.stderr),
        data_base64: None,
        mime_type: None,
        started_at: 0,
        finished_at: 0,
        truncated: output.truncated,
    })
}

fn empty_success(detail: &str) -> DeviceActionResult {
    DeviceActionResult {
        action_id: String::new(),
        device_id: String::new(),
        action: DeviceActionKind::Info,
        success: true,
        detail: detail.to_string(),
        stdout: None,
        stderr: None,
        data_base64: None,
        mime_type: None,
        started_at: 0,
        finished_at: 0,
        truncated: false,
    }
}

#[derive(Debug)]
struct CappedCommandOutput {
    success: bool,
    status_code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    timed_out: bool,
    truncated: bool,
}

fn run_capped_command(
    program: &str,
    args: Vec<String>,
    timeout: Duration,
    max_bytes: usize,
) -> Result<CappedCommandOutput, String> {
    let mut command = Command::new(program);
    configure_silent_command(&mut command);
    let mut child = command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Impossible de lancer l'executable : {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Sortie standard indisponible".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Sortie d'erreur indisponible".to_string())?;
    let stdout_reader = thread::spawn(move || read_capped_stream(stdout, max_bytes));
    let stderr_reader = thread::spawn(move || read_capped_stream(stderr, max_bytes));
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            Ok(None) => {
                timed_out = true;
                let _ = child.kill();
                break child
                    .wait()
                    .map_err(|error| format!("Arret du processus impossible : {error}"))?;
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("Etat du processus illisible : {error}"));
            }
        }
    };
    let (stdout, stdout_truncated) = stdout_reader
        .join()
        .map_err(|_| "Lecture de la sortie standard interrompue".to_string())??;
    let (mut stderr, mut stderr_truncated) = stderr_reader
        .join()
        .map_err(|_| "Lecture de la sortie d'erreur interrompue".to_string())??;
    let remaining = max_bytes.saturating_sub(stdout.len());
    if stderr.len() > remaining {
        stderr.truncate(remaining);
        stderr_truncated = true;
    }
    Ok(CappedCommandOutput {
        success: status.success() && !timed_out,
        status_code: status.code(),
        stdout,
        stderr,
        timed_out,
        truncated: stdout_truncated || stderr_truncated,
    })
}

#[cfg(windows)]
fn configure_silent_command(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn configure_silent_command(_command: &mut Command) {}

fn read_capped_stream<R: Read>(mut stream: R, max_bytes: usize) -> Result<(Vec<u8>, bool), String> {
    let mut kept = Vec::with_capacity(max_bytes.min(16 * 1024));
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let read = stream
            .read(&mut buffer)
            .map_err(|error| format!("Lecture de sortie impossible : {error}"))?;
        if read == 0 {
            break;
        }
        let remaining = max_bytes.saturating_sub(kept.len());
        let copy = remaining.min(read);
        kept.extend_from_slice(&buffer[..copy]);
        truncated |= copy < read;
    }
    Ok((kept, truncated))
}

fn format_command_error(prefix: &str, output: &CappedCommandOutput) -> String {
    if output.timed_out {
        return format!("{prefix} : delai depasse");
    }
    let detail = bytes_to_optional_text(&output.stderr)
        .or_else(|| bytes_to_optional_text(&output.stdout))
        .unwrap_or_else(|| {
            output
                .status_code
                .map(|code| format!("code {code}"))
                .unwrap_or_else(|| "echec sans detail".to_string())
        });
    format!(
        "{prefix} : {}",
        detail.chars().take(500).collect::<String>()
    )
}

fn validate_action_request(request: &DeviceActionRequest) -> Result<(), DeviceFleetError> {
    validate_device_id(&request.device_id)?;
    if let Some(key) = request.idempotency_key.as_deref() {
        if key.is_empty()
            || key.len() > 128
            || !key.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | ':')
            })
        {
            return Err(DeviceFleetError::Validation(
                "idempotencyKey est invalide".to_string(),
            ));
        }
    }
    if !matches!(
        request.action,
        DeviceActionKind::Info | DeviceActionKind::Screenshot
    ) && !request.confirmed
    {
        return Err(DeviceFleetError::Validation(
            "Cette action modifie l'appareil et exige confirmed=true".to_string(),
        ));
    }
    if let Some(timeout) = request.args.timeout_ms {
        if !(250..=60_000).contains(&timeout) {
            return Err(DeviceFleetError::Validation(
                "timeoutMs doit etre compris entre 250 et 60000".to_string(),
            ));
        }
    }
    match request.action {
        DeviceActionKind::Tap => {
            validate_coordinate(request.args.x, "x")?;
            validate_coordinate(request.args.y, "y")?;
        }
        DeviceActionKind::Swipe => {
            validate_coordinate(request.args.start_x, "startX")?;
            validate_coordinate(request.args.start_y, "startY")?;
            validate_coordinate(request.args.end_x, "endX")?;
            validate_coordinate(request.args.end_y, "endY")?;
            let duration = request.args.duration_ms.unwrap_or(300);
            if !(50..=10_000).contains(&duration) {
                return Err(DeviceFleetError::Validation(
                    "durationMs doit etre compris entre 50 et 10000".to_string(),
                ));
            }
        }
        DeviceActionKind::TypeText => {
            let text = request
                .args
                .text
                .as_deref()
                .ok_or_else(|| DeviceFleetError::Validation("text est requis".to_string()))?;
            if text.is_empty() || text.chars().count() > 500 || text.chars().any(char::is_control) {
                return Err(DeviceFleetError::Validation(
                    "text doit contenir entre 1 et 500 caracteres sans controle".to_string(),
                ));
            }
        }
        DeviceActionKind::KeyEvent => {
            let key = request
                .args
                .key
                .as_deref()
                .ok_or_else(|| DeviceFleetError::Validation("key est requis".to_string()))?;
            if key.is_empty()
                || key.len() > 40
                || !key
                    .chars()
                    .all(|value| value.is_ascii_alphanumeric() || value == '_')
            {
                return Err(DeviceFleetError::Validation(
                    "key doit etre un code alphanumerique de 1 a 40 caracteres".to_string(),
                ));
            }
        }
        DeviceActionKind::OpenApp => {
            let app_id = request
                .args
                .app_id
                .as_deref()
                .ok_or_else(|| DeviceFleetError::Validation("appId est requis".to_string()))?;
            if app_id.is_empty()
                || app_id.len() > 200
                || !app_id
                    .chars()
                    .all(|value| value.is_ascii_alphanumeric() || matches!(value, '.' | '_' | '-'))
            {
                return Err(DeviceFleetError::Validation(
                    "appId contient des caracteres non autorises".to_string(),
                ));
            }
        }
        DeviceActionKind::Shell => {
            let command =
                request.args.command.as_deref().ok_or_else(|| {
                    DeviceFleetError::Validation("command est requis".to_string())
                })?;
            if command.trim().is_empty()
                || command.len() > 4096
                || command.chars().any(char::is_control)
            {
                return Err(DeviceFleetError::Validation(
                    "command doit contenir 1 a 4096 caracteres sans caractere de controle"
                        .to_string(),
                ));
            }
        }
        DeviceActionKind::PushFile => {
            let local_path = request.args.local_path.as_deref().unwrap_or("");
            let remote_path = request.args.remote_path.as_deref().unwrap_or("");
            if local_path.is_empty()
                || local_path.len() > 4096
                || local_path.chars().any(char::is_control)
            {
                return Err(DeviceFleetError::Validation(
                    "localPath doit contenir 1 a 4096 caracteres sans caractere de controle"
                        .to_string(),
                ));
            }
            if remote_path.is_empty()
                || !remote_path.starts_with('/')
                || remote_path.len() > 4096
                || remote_path.chars().any(char::is_control)
            {
                return Err(DeviceFleetError::Validation(
                    "remotePath doit etre un chemin absolu de 1 a 4096 caracteres sans controle"
                        .to_string(),
                ));
            }
        }
        DeviceActionKind::Info | DeviceActionKind::Screenshot | DeviceActionKind::OpenScreen => {}
    }
    Ok(())
}

fn validate_coordinate(value: Option<i32>, name: &str) -> Result<(), DeviceFleetError> {
    if !value.is_some_and(|coordinate| (0..=100_000).contains(&coordinate)) {
        return Err(DeviceFleetError::Validation(format!(
            "{name} doit etre compris entre 0 et 100000"
        )));
    }
    Ok(())
}

fn validate_control_device(device: &ControlDevice) -> Result<(), DeviceFleetError> {
    let serial = validate_serial(&device.serial)?;
    let expected = format!("{}:{serial}", device.platform.id_prefix());
    if device.id != expected {
        return Err(DeviceFleetError::Validation(format!(
            "Identifiant appareil invalide : {}",
            device.id
        )));
    }
    if device.transport != DeviceTransport::Usb {
        return Err(DeviceFleetError::Validation(
            "Seuls les appareils USB sont acceptes".to_string(),
        ));
    }
    if device.capabilities.len() > 16 {
        return Err(DeviceFleetError::Validation(
            "Trop de capacites publiees".to_string(),
        ));
    }
    if device.ready != (device.state == DeviceConnectionState::Ready) {
        return Err(DeviceFleetError::Validation(
            "Etat et disponibilite de l'appareil incoherents".to_string(),
        ));
    }
    if device.jailbreak_ready && device.platform != DevicePlatform::Ios {
        return Err(DeviceFleetError::Validation(
            "L'etat jailbreak n'est valable que pour iOS".to_string(),
        ));
    }
    Ok(())
}

fn ensure_device_supports(
    device: &ControlDevice,
    action: DeviceActionKind,
) -> Result<(), DeviceFleetError> {
    if !device.ready || device.state != DeviceConnectionState::Ready {
        return Err(DeviceFleetError::Unavailable(format!(
            "L'appareil {} n'est pas pret",
            device.id
        )));
    }
    if !device.capabilities.contains(&action) {
        return Err(DeviceFleetError::Unavailable(format!(
            "L'action {action:?} n'est pas disponible sur {}",
            device.id
        )));
    }
    Ok(())
}

fn validate_connector_id(value: &str) -> Result<String, DeviceFleetError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 80
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
    {
        return Err(DeviceFleetError::Validation(
            "Identifiant de connecteur invalide".to_string(),
        ));
    }
    Ok(value.to_string())
}

fn validate_owner_id(value: &str) -> Result<String, DeviceFleetError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 160
        || value.chars().any(|character| character.is_control())
    {
        return Err(DeviceFleetError::Validation(
            "Proprietaire d'action invalide".to_string(),
        ));
    }
    Ok(value.to_string())
}

fn validate_action_origin_id(value: &str) -> Result<String, DeviceFleetError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 200
        || value.chars().any(|character| character.is_control())
    {
        return Err(DeviceFleetError::Validation(
            "Origine d'action invalide".to_string(),
        ));
    }
    Ok(value.to_string())
}

fn validate_action_id(value: &str) -> Result<String, DeviceFleetError> {
    Uuid::parse_str(value.trim())
        .map(|uuid| uuid.to_string())
        .map_err(|_| DeviceFleetError::Validation("Identifiant d'action invalide".to_string()))
}

fn validate_device_id(value: &str) -> Result<(), DeviceFleetError> {
    let (platform, serial) = value.split_once(':').ok_or_else(|| {
        DeviceFleetError::Validation("Identifiant d'appareil invalide".to_string())
    })?;
    if !matches!(platform, "android" | "ios") {
        return Err(DeviceFleetError::Validation(
            "Plateforme d'appareil invalide".to_string(),
        ));
    }
    validate_serial(serial)?;
    Ok(())
}

fn parse_local_device_target(value: &str) -> Result<(DevicePlatform, String), DeviceFleetError> {
    validate_device_id(value)?;
    let (platform, serial) = value
        .split_once(':')
        .expect("identifiant appareil deja valide");
    let serial = validate_serial(serial)?;
    match platform {
        "android" if is_usb_android_serial(&serial) => Ok((DevicePlatform::Android, serial)),
        "android" => Err(DeviceFleetError::Validation(
            "La cible Android doit etre un appareil USB physique".to_string(),
        )),
        "ios" => Ok((DevicePlatform::Ios, serial)),
        _ => unreachable!("plateforme appareil deja validee"),
    }
}

fn validate_serial(value: &str) -> Result<String, DeviceFleetError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 160
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
    {
        return Err(DeviceFleetError::Validation(
            "Numero de serie USB invalide".to_string(),
        ));
    }
    Ok(value.to_string())
}

fn validate_ssh_user(value: &str) -> Result<(), DeviceFleetError> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return Err(DeviceFleetError::Validation(
            "CST_IOS_SSH_USER est invalide".to_string(),
        ));
    }
    Ok(())
}

fn is_usb_android_serial(serial: &str) -> bool {
    !serial.starts_with("emulator-")
        && !serial.contains(':')
        && !serial.contains("._adb-")
        && !serial.starts_with("localhost")
}

fn escape_android_input_text(value: &str) -> Result<String, DeviceFleetError> {
    let mut escaped = String::with_capacity(value.len() * 2);
    for character in value.chars() {
        if character == ' ' {
            escaped.push_str("%s");
        } else if character.is_ascii_alphanumeric()
            || matches!(
                character,
                '.' | ',' | '!' | '?' | '@' | '_' | '+' | '-' | '=' | ':' | '/'
            )
        {
            escaped.push(character);
        } else if matches!(
            character,
            '&' | '<' | '>' | ';' | '|' | '*' | '$' | '`' | '(' | ')' | '[' | ']' | '{' | '}'
        ) {
            escaped.push('\\');
            escaped.push(character);
        } else {
            return Err(DeviceFleetError::Validation(format!(
                "Le caractere {character:?} n'est pas pris en charge par la saisie ADB securisee"
            )));
        }
    }
    Ok(escaped)
}

fn ios_ui_tool() -> Option<String> {
    env::var("CST_IOS_UI_TOOL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 160
                && value.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '/' | '.' | '_' | '-')
                })
        })
}

fn posix_shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn action_timeout(request: &DeviceActionRequest) -> Duration {
    Duration::from_millis(request.args.timeout_ms.unwrap_or(match request.action {
        DeviceActionKind::Screenshot => 15_000,
        DeviceActionKind::OpenScreen => 5_000,
        DeviceActionKind::Shell => 30_000,
        DeviceActionKind::PushFile => 180_000,
        _ => 10_000,
    }))
}

fn tool_program(variable: &str, fallback: &str) -> String {
    env::var(variable)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn local_connector_id() -> String {
    let host = env::var("COMPUTERNAME")
        .or_else(|_| env::var("HOSTNAME"))
        .unwrap_or_else(|_| "desktop".to_string());
    let clean = host
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || "._-".contains(*character))
        .take(70)
        .collect::<String>();
    format!("usb-{}", if clean.is_empty() { "desktop" } else { &clean })
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}

fn sanitize_tools(mut tools: DeviceFleetTools) -> DeviceFleetTools {
    for tool in [
        &mut tools.adb,
        &mut tools.scrcpy,
        &mut tools.idevice_id,
        &mut tools.idevice_info,
        &mut tools.idevice_screenshot,
        &mut tools.iproxy,
        &mut tools.ssh,
        &mut tools.ios_wda,
    ] {
        tool.detail = sanitize_optional(tool.detail.take(), 300);
    }
    tools
}

fn sanitize_optional(value: Option<String>, max_chars: usize) -> Option<String> {
    value.and_then(|value| {
        let clean = value
            .chars()
            .filter(|character| !character.is_control() || *character == '\n' || *character == '\t')
            .take(max_chars)
            .collect::<String>();
        (!clean.trim().is_empty()).then_some(clean)
    })
}

fn sanitize_optional_bytes(value: Option<String>, max_bytes: usize) -> Option<String> {
    value.and_then(|value| {
        let mut clean = value
            .chars()
            .filter(|character| !character.is_control() || *character == '\n' || *character == '\t')
            .collect::<String>();
        if clean.len() > max_bytes {
            let mut boundary = max_bytes;
            while boundary > 0 && !clean.is_char_boundary(boundary) {
                boundary -= 1;
            }
            clean.truncate(boundary);
        }
        (!clean.trim().is_empty()).then_some(clean)
    })
}

fn sanitize_action_result(result: &mut DeviceActionResult) {
    result.detail = result.detail.chars().take(1_000).collect();
    result.stdout = sanitize_optional_bytes(result.stdout.take(), MAX_ACTION_OUTPUT_BYTES);
    result.stderr = sanitize_optional_bytes(result.stderr.take(), MAX_ACTION_OUTPUT_BYTES);
    if result.action != DeviceActionKind::Screenshot {
        result.data_base64 = None;
        result.mime_type = None;
        return;
    }
    if result.truncated {
        result.success = false;
        result.detail = "La capture est tronquee et a ete refusee".to_string();
        result.data_base64 = None;
        result.mime_type = None;
        return;
    }
    let (Some(encoded), Some(mime_type)) =
        (result.data_base64.as_deref(), result.mime_type.as_deref())
    else {
        result.success = false;
        result.detail = "La capture ne contient pas une image complete".to_string();
        result.data_base64 = None;
        result.mime_type = None;
        return;
    };
    let decoded = BASE64_STANDARD.decode(encoded).ok();
    let valid = decoded.as_deref().is_some_and(|bytes| {
        !bytes.is_empty()
            && bytes.len() <= MAX_SCREENSHOT_BYTES
            && screenshot_signature_matches(mime_type, bytes)
    });
    if !valid {
        result.success = false;
        result.detail = "La capture image est invalide ou depasse la taille autorisee".to_string();
        result.data_base64 = None;
        result.mime_type = None;
    }
}

fn screenshot_signature_matches(mime_type: &str, bytes: &[u8]) -> bool {
    screenshot_mime_type(bytes)
        .is_some_and(|detected| detected == mime_type.trim().to_ascii_lowercase())
}

fn screenshot_mime_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        Some("image/tiff")
    } else {
        None
    }
}

fn expire_action(action: &mut QueuedAction, now: i64, detail: &str) {
    expire_action_started_at(action, now, action.public.created_at, detail);
}

fn expire_action_started_at(action: &mut QueuedAction, now: i64, started_at: i64, detail: &str) {
    action.public.status = DeviceActionStatus::Expired;
    action.public.updated_at = now;
    action.public.claimed_at = None;
    action.public.finished_at = Some(now);
    action.public.connector_id = None;
    action.public.result = Some(DeviceActionResult {
        action_id: action.public.id.clone(),
        device_id: action.public.device_id.clone(),
        action: action.public.action,
        success: false,
        detail: detail.to_string(),
        stdout: None,
        stderr: None,
        data_base64: None,
        mime_type: None,
        started_at,
        finished_at: now,
        truncated: false,
    });
    action.claim_token = None;
}

fn recover_actions(actions: &mut Vec<QueuedAction>, now: i64) -> bool {
    let original_len = actions.len();
    actions.retain(|action| {
        !action.public.status.is_terminal()
            || action.public.finished_at.is_some_and(|finished| {
                finished.saturating_add(COMPLETED_ACTION_RETENTION_SECONDS) > now
            })
    });
    let mut changed = actions.len() != original_len;
    for action in actions {
        if action.public.status.is_terminal() {
            let had_claim = action.public.claimed_at.take().is_some();
            let had_connector = action.public.connector_id.take().is_some();
            let had_token = action.claim_token.take().is_some();
            if had_claim || had_connector || had_token {
                changed = true;
            }
            continue;
        }
        if action.ephemeral_origin {
            expire_action(
                action,
                now,
                "Origine non persistante perdue au redemarrage ; action abandonnee",
            );
            changed = true;
            continue;
        }
        match action.public.status {
            DeviceActionStatus::Queued => {
                if action
                    .public
                    .created_at
                    .saturating_add(QUEUED_ACTION_TTL_SECONDS)
                    <= now
                {
                    expire_action(
                        action,
                        now,
                        "Action expiree pendant l'arret ; elle ne sera pas executee",
                    );
                    changed = true;
                } else {
                    let had_claim = action.public.claimed_at.take().is_some();
                    let had_connector = action.public.connector_id.take().is_some();
                    let had_token = action.claim_token.take().is_some();
                    if had_claim || had_connector || had_token {
                        changed = true;
                    }
                }
            }
            DeviceActionStatus::Claimed => {
                let claimed_at = action.public.claimed_at.unwrap_or(action.public.created_at);
                let queue_is_live = action
                    .public
                    .created_at
                    .saturating_add(QUEUED_ACTION_TTL_SECONDS)
                    > now;
                let retry_is_safe = queue_is_live
                    && matches!(
                        action.public.action,
                        DeviceActionKind::Info | DeviceActionKind::Screenshot
                    );
                if retry_is_safe {
                    action.public.status = DeviceActionStatus::Queued;
                    action.public.updated_at = now;
                    action.public.claimed_at = None;
                    action.public.finished_at = None;
                    action.public.connector_id = None;
                    action.public.result = None;
                    action.claim_token = None;
                } else {
                    let detail = if matches!(
                        action.public.action,
                        DeviceActionKind::Info | DeviceActionKind::Screenshot
                    ) {
                        "Action de lecture reclamee mais son TTL a expire pendant l'arret"
                    } else {
                        "Etat d'execution ambigu apres redemarrage ; action mutante jamais rejouee"
                    };
                    expire_action_started_at(action, now, claimed_at, detail);
                }
                changed = true;
            }
            DeviceActionStatus::Succeeded
            | DeviceActionStatus::Failed
            | DeviceActionStatus::Expired => unreachable!("statut terminal traite plus haut"),
        }
    }
    changed
}

fn prune_actions(actions: &mut Vec<QueuedAction>, now: i64) -> bool {
    let original_len = actions.len();
    actions.retain(|action| {
        !action.public.status.is_terminal()
            || action.public.finished_at.is_some_and(|finished| {
                finished.saturating_add(COMPLETED_ACTION_RETENTION_SECONDS) > now
            })
    });
    actions.len() != original_len
}

fn bytes_to_optional_text(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        None
    } else {
        Some(String::from_utf8_lossy(bytes).trim().to_string()).filter(|value| !value.is_empty())
    }
}

fn redact_bytes(bytes: &mut Vec<u8>, secret: &[u8]) {
    if secret.is_empty() || bytes.len() < secret.len() {
        return;
    }
    let text = String::from_utf8_lossy(bytes)
        .replace(&String::from_utf8_lossy(secret).to_string(), "[cle privee]");
    *bytes = text.into_bytes();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    struct TestDataDir(PathBuf);

    impl TestDataDir {
        fn new(label: &str) -> Self {
            let path = env::temp_dir().join(format!(
                "switch-device-fleet-{label}-{}",
                Uuid::new_v4().simple()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn store_path(&self) -> PathBuf {
            self.0.join(ACTION_STORE_DIRECTORY)
        }
    }

    impl Drop for TestDataDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn device_action_concurrency_is_bounded_and_accepts_forty_workers() {
        assert_eq!(parse_device_action_concurrency(Some("40")), 40);
        assert_eq!(parse_device_action_concurrency(Some(" 40 ")), 40);
        assert_eq!(
            parse_device_action_concurrency(Some("0")),
            DEFAULT_DEVICE_ACTION_CONCURRENCY
        );
        assert_eq!(
            parse_device_action_concurrency(Some("129")),
            DEFAULT_DEVICE_ACTION_CONCURRENCY
        );
        assert_eq!(
            parse_device_action_concurrency(Some("invalid")),
            DEFAULT_DEVICE_ACTION_CONCURRENCY
        );
        assert_eq!(
            parse_device_action_concurrency(None),
            DEFAULT_DEVICE_ACTION_CONCURRENCY
        );
    }

    fn final_snapshot_paths(data_dir: &TestDataDir) -> Vec<(u64, PathBuf)> {
        let mut snapshots = fs::read_dir(data_dir.store_path())
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| {
                action_store_generation(&entry.file_name().to_string_lossy())
                    .map(|generation| (generation, entry.path()))
            })
            .collect::<Vec<_>>();
        snapshots.sort_by_key(|(generation, _)| *generation);
        snapshots
    }

    fn ready_android(serial: &str) -> ControlDevice {
        ControlDevice {
            id: format!("android:{serial}"),
            platform: DevicePlatform::Android,
            transport: DeviceTransport::Usb,
            serial: serial.to_string(),
            name: Some("Pixel".to_string()),
            model: Some("Pixel 9".to_string()),
            os_version: Some("16".to_string()),
            state: DeviceConnectionState::Ready,
            ready: true,
            jailbreak_ready: false,
            capabilities: android_capabilities(true, true),
            connector_id: None,
            last_seen_at: 0,
            error: None,
        }
    }

    fn info_request(serial: &str) -> DeviceActionRequest {
        DeviceActionRequest {
            device_id: format!("android:{serial}"),
            action: DeviceActionKind::Info,
            args: DeviceActionArgs::default(),
            confirmed: false,
            idempotency_key: None,
        }
    }

    fn heartbeat(serial: &str) -> DeviceConnectorHeartbeatRequest {
        DeviceConnectorHeartbeatRequest {
            connector_id: "usb-test".to_string(),
            devices: vec![ready_android(serial)],
            tools: DeviceFleetTools::default(),
            error: None,
        }
    }

    #[test]
    fn adb_parser_keeps_only_physical_usb_devices() {
        let output = "List of devices attached\nABC123 device usb:1-2 product:husky model:Pixel_8 device:husky transport_id:4\nNOAUTH unauthorized usb:1-3\nemulator-5554 device product:sdk model:sdk_gphone\n192.168.1.20:5555 device product:pixel\n";
        let devices = parse_adb_devices(output, 100);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].id, "android:ABC123");
        assert_eq!(devices[0].model.as_deref(), Some("Pixel 8"));
        assert!(devices[0].ready);
        assert_eq!(devices[1].state, DeviceConnectionState::Unauthorized);
    }

    #[test]
    fn adb_parser_caps_and_orders_large_usb_inventories() {
        let mut output = String::from("List of devices attached\n");
        for index in (0..MAX_DEVICES + 12).rev() {
            output.push_str(&format!(
                "USB{index:03} device usb:1-2 product:test model:Phone device:phone\n"
            ));
        }
        output.push_str("USB000 device usb:1-2 product:duplicate\n");
        let devices = parse_adb_devices(&output, 100);
        assert_eq!(devices.len(), MAX_DEVICES);
        assert_eq!(
            devices.first().map(|device| device.serial.as_str()),
            Some("USB000")
        );
        assert_eq!(
            devices.last().map(|device| device.serial.as_str()),
            Some("USB127")
        );
        assert!(devices.windows(2).all(|pair| pair[0].id < pair[1].id));
    }

    #[test]
    fn ios_and_property_parsers_are_bounded_and_deduplicate() {
        let mut ids = (0..MAX_DEVICES + 8)
            .rev()
            .map(|index| format!("00008110-{index:03}"))
            .collect::<Vec<_>>()
            .join("\n");
        ids.push_str("\n00008110-000\ninvalid id\n");
        let parsed_ids = parse_idevice_ids(&ids);
        assert_eq!(parsed_ids.len(), MAX_DEVICES);
        assert_eq!(parsed_ids.first().map(String::as_str), Some("00008110-000"));
        assert_eq!(parsed_ids.last().map(String::as_str), Some("00008110-127"));
        assert!(parsed_ids.windows(2).all(|pair| pair[0] < pair[1]));
        let properties = parse_ios_properties("DeviceName: Jean iPhone\nProductVersion: 18.2\n");
        assert_eq!(
            properties.get("DeviceName").map(String::as_str),
            Some("Jean iPhone")
        );
    }

    #[test]
    fn ios_wda_requires_an_explicit_valid_opt_in() {
        assert_eq!(parse_ios_wda_opt_in(None).unwrap(), false);
        for value in ["1", "true", "TRUE", " yes ", "on"] {
            assert!(parse_ios_wda_opt_in(Some(value)).unwrap(), "{value}");
        }
        for value in ["0", "false", "FALSE", " no ", "off"] {
            assert!(!parse_ios_wda_opt_in(Some(value)).unwrap(), "{value}");
        }
        for value in ["", "enabled", "2"] {
            assert!(parse_ios_wda_opt_in(Some(value)).is_err(), "{value}");
        }
    }

    #[test]
    fn ios_capabilities_merge_only_capabilities_reported_by_wda() {
        let tools = DeviceFleetTools::default();
        let probe = IosWdaProbe {
            ready: true,
            capabilities: vec![IosWdaCapability::Tap, IosWdaCapability::OpenApp],
            screen: None,
        };
        let capabilities = ios_capabilities(true, false, &tools, Some(&probe));
        assert_eq!(
            capabilities,
            vec![
                DeviceActionKind::Info,
                DeviceActionKind::Tap,
                DeviceActionKind::OpenApp
            ]
        );
        assert!(!capabilities.contains(&DeviceActionKind::Screenshot));
        assert!(!capabilities.contains(&DeviceActionKind::Swipe));
    }

    #[test]
    fn ios_capabilities_keep_local_and_jailbreak_fallbacks_without_duplicates() {
        let mut tools = DeviceFleetTools::default();
        tools.idevice_screenshot.available = true;
        tools.ios_ui_tool_configured = true;
        let probe = IosWdaProbe {
            ready: true,
            capabilities: vec![IosWdaCapability::Screenshot, IosWdaCapability::Tap],
            screen: None,
        };
        let capabilities = ios_capabilities(true, true, &tools, Some(&probe));
        for expected in [
            DeviceActionKind::Info,
            DeviceActionKind::Screenshot,
            DeviceActionKind::Shell,
            DeviceActionKind::Tap,
            DeviceActionKind::Swipe,
            DeviceActionKind::TypeText,
            DeviceActionKind::KeyEvent,
            DeviceActionKind::OpenApp,
        ] {
            assert!(capabilities.contains(&expected), "{expected:?}");
            assert_eq!(
                capabilities
                    .iter()
                    .filter(|capability| **capability == expected)
                    .count(),
                1
            );
        }
    }

    #[test]
    fn ios_wda_action_mapping_validates_coordinates_and_supported_keys() {
        let mut request = info_request("ABC123");
        request.device_id = "ios:ABC123".to_string();
        request.action = DeviceActionKind::Tap;
        request.args.x = Some(12);
        request.args.y = Some(34);
        assert_eq!(
            ios_wda_action_for_request(&request).unwrap(),
            Some(IosWdaAction::Tap { x: 12, y: 34 })
        );
        request.args.x = Some(-1);
        assert!(matches!(
            ios_wda_action_for_request(&request),
            Err(DeviceFleetError::Validation(_))
        ));

        request.action = DeviceActionKind::KeyEvent;
        request.args = DeviceActionArgs::default();
        request.args.key = Some("home".to_string());
        assert_eq!(
            ios_wda_action_for_request(&request).unwrap(),
            Some(IosWdaAction::KeyEvent {
                key: "HOME".to_string()
            })
        );
        for key in ["VOLUME_UP", "volume_down"] {
            request.args.key = Some(key.to_string());
            assert!(matches!(
                ios_wda_action_for_request(&request).unwrap(),
                Some(IosWdaAction::KeyEvent { .. })
            ));
        }
        request.args.key = Some("ENTER".to_string());
        assert_eq!(ios_wda_action_for_request(&request).unwrap(), None);
    }

    #[test]
    fn ios_wda_screenshot_adapter_bounds_and_validates_mime_and_signature() {
        let png = b"\x89PNG\r\n\x1a\nimage".to_vec();
        let valid = ios_wda_screenshot_result(IosWdaActionResult {
            detail: "capture WDA".to_string(),
            screenshot: Some(crate::ios_wda::IosWdaScreenshot {
                bytes: png.clone(),
                mime_type: "image/png".to_string(),
            }),
        })
        .unwrap();
        assert_eq!(valid.mime_type.as_deref(), Some("image/png"));
        assert_eq!(
            valid
                .data_base64
                .as_deref()
                .and_then(|value| BASE64_STANDARD.decode(value).ok()),
            Some(png)
        );

        let invalid_signature = ios_wda_screenshot_result(IosWdaActionResult {
            detail: "capture WDA".to_string(),
            screenshot: Some(crate::ios_wda::IosWdaScreenshot {
                bytes: b"not-an-image".to_vec(),
                mime_type: "image/png".to_string(),
            }),
        });
        assert!(matches!(
            invalid_signature,
            Err(DeviceFleetError::Unavailable(_))
        ));

        let oversized = ios_wda_screenshot_result(IosWdaActionResult {
            detail: "capture WDA".to_string(),
            screenshot: Some(crate::ios_wda::IosWdaScreenshot {
                bytes: vec![0; MAX_SCREENSHOT_BYTES + 1],
                mime_type: "image/png".to_string(),
            }),
        });
        assert!(matches!(oversized, Err(DeviceFleetError::Unavailable(_))));
    }

    #[test]
    fn local_target_parser_accepts_only_explicit_usb_device_ids() {
        assert_eq!(
            parse_local_device_target("android:ABC123").unwrap(),
            (DevicePlatform::Android, "ABC123".to_string())
        );
        assert_eq!(
            parse_local_device_target("ios:00008110-ABC").unwrap(),
            (DevicePlatform::Ios, "00008110-ABC".to_string())
        );
        for rejected in [
            "android:emulator-5554",
            "android:192.168.1.20:5555",
            "android:phone._adb-tls-connect._tcp",
            "unknown:ABC123",
        ] {
            assert!(parse_local_device_target(rejected).is_err(), "{rejected}");
        }
    }

    #[test]
    fn metadata_probes_are_bounded_and_keep_input_order() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let output =
            bounded_parallel_map_ordered((0..64).collect::<Vec<_>>(), MAX_METADATA_PROBES, {
                let active = active.clone();
                let peak = peak.clone();
                move |value| {
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(current, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(2));
                    active.fetch_sub(1, Ordering::SeqCst);
                    value * 2
                }
            });
        assert_eq!(output, (0..64).map(|value| value * 2).collect::<Vec<_>>());
        assert!((1..=MAX_METADATA_PROBES).contains(&peak.load(Ordering::SeqCst)));
    }

    #[test]
    fn total_fleet_is_sorted_deduplicated_and_capped() {
        let mut devices = Vec::new();
        for index in (0..80).rev() {
            devices.push(ready_android(&format!("A{index:03}")));
            let mut ios = ready_android(&format!("I{index:03}"));
            ios.platform = DevicePlatform::Ios;
            ios.id = format!("ios:{}", ios.serial);
            ios.jailbreak_ready = true;
            devices.push(ios);
        }
        let duplicate = devices[0].clone();
        devices.push(duplicate);
        sort_and_cap_devices(&mut devices);
        assert_eq!(devices.len(), MAX_DEVICES);
        assert!(devices.windows(2).all(|pair| pair[0].id < pair[1].id));
        assert_eq!(
            devices
                .iter()
                .filter(|device| device.platform == DevicePlatform::Android)
                .count(),
            80
        );
        assert_eq!(
            devices
                .iter()
                .filter(|device| device.platform == DevicePlatform::Ios)
                .count(),
            MAX_DEVICES - 80
        );
    }

    #[test]
    fn non_zero_tool_probe_is_not_available() {
        let failed = tool_status_from_probe(Ok(CappedCommandOutput {
            success: false,
            status_code: Some(7),
            stdout: Vec::new(),
            stderr: b"probe failed".to_vec(),
            timed_out: false,
            truncated: false,
        }));
        assert!(!failed.available);
        assert!(failed
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("code 7")));

        let successful = tool_status_from_probe(Ok(CappedCommandOutput {
            success: true,
            status_code: Some(0),
            stdout: Vec::new(),
            stderr: Vec::new(),
            timed_out: false,
            truncated: false,
        }));
        assert!(successful.available);
        assert!(successful.detail.is_none());
    }

    #[test]
    fn validation_requires_safe_coordinates_and_shell_confirmation() {
        let mut tap = info_request("ABC123");
        tap.action = DeviceActionKind::Tap;
        tap.args.x = Some(-1);
        tap.args.y = Some(10);
        tap.confirmed = true;
        assert!(matches!(
            validate_action_request(&tap),
            Err(DeviceFleetError::Validation(_))
        ));
        tap.args.x = Some(1);
        assert!(validate_action_request(&tap).is_ok());
        tap.confirmed = false;
        assert!(validate_action_request(&tap).is_err());

        let mut shell = info_request("ABC123");
        shell.action = DeviceActionKind::Shell;
        shell.args.command = Some("id".to_string());
        assert!(validate_action_request(&shell).is_err());
        shell.confirmed = true;
        assert!(validate_action_request(&shell).is_ok());
    }

    #[test]
    fn every_mutating_action_requires_explicit_confirmation() {
        for action in [
            DeviceActionKind::OpenScreen,
            DeviceActionKind::Tap,
            DeviceActionKind::Swipe,
            DeviceActionKind::TypeText,
            DeviceActionKind::KeyEvent,
            DeviceActionKind::OpenApp,
            DeviceActionKind::Shell,
        ] {
            let mut request = info_request("ABC123");
            request.action = action;
            assert!(matches!(
                validate_action_request(&request),
                Err(DeviceFleetError::Validation(detail)) if detail.contains("confirmed=true")
            ));
        }
        assert!(validate_action_request(&info_request("ABC123")).is_ok());
        let mut screenshot = info_request("ABC123");
        screenshot.action = DeviceActionKind::Screenshot;
        assert!(validate_action_request(&screenshot).is_ok());
    }

    #[test]
    fn manager_claims_forty_distinct_devices_concurrently() {
        let manager = DeviceFleetManager::new(Duration::from_secs(15));
        let serials = (1..=40)
            .map(|index| format!("PHONE-{index:02}"))
            .collect::<Vec<_>>();
        manager
            .heartbeat_at(
                DeviceConnectorHeartbeatRequest {
                    connector_id: "usb-test".to_string(),
                    devices: serials.iter().map(|serial| ready_android(serial)).collect(),
                    tools: DeviceFleetTools::default(),
                    error: None,
                },
                100,
            )
            .unwrap();
        for serial in &serials {
            manager
                .queue_action_at("owner-a", info_request(serial), 101)
                .unwrap();
        }
        manager
            .queue_action_at("owner-a", info_request(&serials[0]), 101)
            .unwrap();

        let mut claimed_devices = HashSet::new();
        for _ in 0..40 {
            let job = manager
                .claim_next_at("usb-test", 102)
                .unwrap()
                .expect("une action par telephone doit pouvoir etre reclamee");
            assert!(claimed_devices.insert(job.device_id));
        }
        assert_eq!(claimed_devices.len(), 40);
        assert!(manager.claim_next_at("usb-test", 102).unwrap().is_none());
    }

    #[test]
    fn manager_enforces_owner_and_connector_lease() {
        let manager = DeviceFleetManager::new(Duration::from_secs(15));
        manager.heartbeat_at(heartbeat("ABC123"), 100).unwrap();
        let queued = manager
            .queue_action_at("owner-a", info_request("ABC123"), 101)
            .unwrap();
        assert_eq!(queued.status, DeviceActionStatus::Queued);
        assert!(matches!(
            manager.action_status("owner-b", &queued.id),
            Err(DeviceFleetError::NotFound)
        ));
        assert!(manager.claim_next_at("usb-other", 102).is_err());
        let job = manager
            .claim_next_at("usb-test", 102)
            .unwrap()
            .expect("travail");
        let result = DeviceActionResult {
            action_id: job.action_id.clone(),
            device_id: job.device_id.clone(),
            action: job.request.action,
            success: true,
            detail: "ok".to_string(),
            stdout: None,
            stderr: None,
            data_base64: None,
            mime_type: None,
            started_at: 102,
            finished_at: 103,
            truncated: false,
        };
        let wrong = DeviceConnectorReportRequest {
            connector_id: "usb-test".to_string(),
            action_id: job.action_id.clone(),
            claim_token: "0".repeat(64),
            result: result.clone(),
        };
        assert!(matches!(
            manager.report_at(wrong, 103),
            Err(DeviceFleetError::Conflict(_))
        ));
        let completed = manager
            .report_at(
                DeviceConnectorReportRequest {
                    connector_id: "usb-test".to_string(),
                    action_id: job.action_id,
                    claim_token: job.claim_token,
                    result,
                },
                103,
            )
            .unwrap();
        assert_eq!(completed.status, DeviceActionStatus::Succeeded);
        assert_eq!(
            manager
                .action_status("owner-a", &completed.id)
                .unwrap()
                .status,
            DeviceActionStatus::Succeeded
        );
    }

    #[test]
    fn stale_snapshot_is_preserved_but_forced_offline() {
        let manager = DeviceFleetManager::new(Duration::from_secs(15));
        manager.heartbeat_at(heartbeat("ABC123"), 100).unwrap();
        assert!(manager.snapshot_at(115).unwrap().connector_online);
        let stale = manager.snapshot_at(116).unwrap();
        assert!(!stale.connector_online);
        assert_eq!(stale.devices.len(), 1);
        assert!(!stale.devices[0].ready);
        assert_eq!(stale.devices[0].state, DeviceConnectionState::Offline);
        assert!(matches!(
            manager.queue_action_at("owner", info_request("ABC123"), 116),
            Err(DeviceFleetError::Unavailable(_))
        ));
    }

    #[test]
    fn expired_claims_are_not_replayed_after_queue_ttl() {
        let manager = DeviceFleetManager::new(Duration::from_secs(15));
        manager.heartbeat_at(heartbeat("ABC123"), 100).unwrap();
        let mut tap = info_request("ABC123");
        tap.action = DeviceActionKind::Tap;
        tap.args.x = Some(10);
        tap.args.y = Some(20);
        tap.confirmed = true;
        let queued = manager.queue_action_at("owner", tap, 101).unwrap();
        manager
            .claim_next_at("usb-test", 102)
            .unwrap()
            .expect("premiere lease");
        let mutation_expired_at = 102 + ACTION_CLAIM_LEASE_SECONDS + 1;
        manager
            .heartbeat_at(heartbeat("ABC123"), mutation_expired_at)
            .unwrap();
        assert!(manager
            .claim_next_at("usb-test", mutation_expired_at)
            .unwrap()
            .is_none());
        assert_eq!(
            manager.action_status("owner", &queued.id).unwrap().status,
            DeviceActionStatus::Expired
        );

        let read_created_at = mutation_expired_at + 1;
        let read_claimed_at = read_created_at + 1;
        let read = manager
            .queue_action_at("owner", info_request("ABC123"), read_created_at)
            .unwrap();
        manager
            .claim_next_at("usb-test", read_claimed_at)
            .unwrap()
            .expect("premiere lecture");
        let read_expired_at = read_claimed_at + ACTION_CLAIM_LEASE_SECONDS + 1;
        manager
            .heartbeat_at(heartbeat("ABC123"), read_expired_at)
            .unwrap();
        assert!(manager
            .claim_next_at("usb-test", read_expired_at)
            .unwrap()
            .is_none());
        let expired_read = manager.action_status("owner", &read.id).unwrap();
        assert_eq!(expired_read.status, DeviceActionStatus::Expired);
        assert!(expired_read
            .result
            .as_ref()
            .is_some_and(|result| result.detail.contains("TTL de file")));
    }

    #[test]
    fn queued_actions_expire_before_a_late_device_reconnect() {
        let manager = DeviceFleetManager::new(Duration::from_secs(300));
        manager.heartbeat_at(heartbeat("ABC123"), 100).unwrap();
        let queued = manager
            .queue_action_at("owner", info_request("ABC123"), 101)
            .unwrap();
        let expired_at = 101 + QUEUED_ACTION_TTL_SECONDS + 1;
        manager
            .heartbeat_at(heartbeat("ABC123"), expired_at)
            .unwrap();
        assert!(manager
            .claim_next_at("usb-test", expired_at)
            .unwrap()
            .is_none());
        let expired = manager.action_status("owner", &queued.id).unwrap();
        assert_eq!(expired.status, DeviceActionStatus::Expired);
        assert!(expired
            .result
            .as_ref()
            .is_some_and(|result| result.detail.contains("avant sa prise en charge")));
    }

    #[test]
    fn idempotency_key_returns_the_original_action_without_duplicating_it() {
        let manager = DeviceFleetManager::new(Duration::from_secs(300));
        manager.heartbeat_at(heartbeat("ABC123"), 100).unwrap();
        let mut request = info_request("ABC123");
        request.idempotency_key = Some("mcp:turn-1-action-1".to_string());
        let first = manager
            .queue_action_at("owner", request.clone(), 101)
            .unwrap();
        let repeated = manager
            .queue_action_at("owner", request.clone(), 102)
            .unwrap();
        assert_eq!(repeated.id, first.id);

        request.action = DeviceActionKind::Screenshot;
        assert!(matches!(
            manager.queue_action_at("owner", request, 103),
            Err(DeviceFleetError::Conflict(_))
        ));
    }

    #[test]
    fn durable_round_trip_resolves_exact_idempotent_retry_without_connector() {
        let data_dir = TestDataDir::new("idempotence");
        let now = now_ts();
        let manager = DeviceFleetManager::load(data_dir.path(), Duration::from_secs(300)).unwrap();
        manager.heartbeat_at(heartbeat("ABC123"), now).unwrap();
        let mut request = info_request("ABC123");
        request.idempotency_key = Some("chat:durable-turn-1".to_string());
        let first = manager
            .queue_action_at("owner-durable", request.clone(), now)
            .unwrap();
        drop(manager);

        let recovered =
            DeviceFleetManager::load(data_dir.path(), Duration::from_secs(300)).unwrap();
        let repeated = recovered
            .queue_action_at("owner-durable", request.clone(), now.saturating_add(1))
            .unwrap();
        assert_eq!(repeated.id, first.id);
        assert_eq!(repeated.status, DeviceActionStatus::Queued);

        request.confirmed = true;
        assert!(matches!(
            recovered.queue_action_at("owner-durable", request, now.saturating_add(1)),
            Err(DeviceFleetError::Conflict(_))
        ));
    }

    #[test]
    fn snapshots_never_persist_claim_tokens_or_screenshot_base64() {
        let data_dir = TestDataDir::new("secrets");
        let now = now_ts();
        let manager = DeviceFleetManager::load(data_dir.path(), Duration::from_secs(300)).unwrap();
        manager.heartbeat_at(heartbeat("ABC123"), now).unwrap();
        let mut request = info_request("ABC123");
        request.action = DeviceActionKind::Screenshot;
        let queued = manager.queue_action_at("owner", request, now).unwrap();
        let job = manager
            .claim_next_at("usb-test", now.saturating_add(1))
            .unwrap()
            .expect("claim durable");
        let image = BASE64_STANDARD.encode(b"\x89PNG\r\n\x1a\nprivate-image-marker");
        let completed = manager
            .report_at(
                DeviceConnectorReportRequest {
                    connector_id: "usb-test".to_string(),
                    action_id: job.action_id.clone(),
                    claim_token: job.claim_token.clone(),
                    result: DeviceActionResult {
                        action_id: job.action_id,
                        device_id: job.device_id,
                        action: DeviceActionKind::Screenshot,
                        success: true,
                        detail: "capture".to_string(),
                        stdout: Some("stdout-borne".to_string()),
                        stderr: None,
                        data_base64: Some(image.clone()),
                        mime_type: Some("image/png".to_string()),
                        started_at: now.saturating_add(1),
                        finished_at: now.saturating_add(2),
                        truncated: false,
                    },
                },
                now.saturating_add(2),
            )
            .unwrap();
        assert!(completed
            .result
            .as_ref()
            .and_then(|result| result.data_base64.as_ref())
            .is_some());

        let snapshots = final_snapshot_paths(&data_dir);
        assert_eq!(snapshots.len(), ACTION_STORE_GENERATIONS_TO_KEEP);
        for (_, path) in snapshots {
            let disk = fs::read_to_string(path).unwrap();
            assert!(!disk.contains(&job.claim_token));
            assert!(!disk.contains("claimToken"));
            assert!(!disk.contains(&image));
        }
        drop(manager);

        let recovered =
            DeviceFleetManager::load(data_dir.path(), Duration::from_secs(300)).unwrap();
        let persisted = recovered.action_status("owner", &queued.id).unwrap();
        assert_eq!(persisted.status, DeviceActionStatus::Succeeded);
        assert!(persisted.claimed_at.is_none());
        assert!(persisted.connector_id.is_none());
        assert!(persisted
            .result
            .as_ref()
            .is_some_and(|result| result.data_base64.is_none()));
        assert!(persisted.result.as_ref().is_some_and(|result| {
            result.mime_type.is_none() && result.detail.contains("binaire omise")
        }));
    }

    #[test]
    fn recovery_requeues_safe_reads_and_expires_ambiguous_mutations() {
        let data_dir = TestDataDir::new("recovery");
        let now = now_ts();
        let manager = DeviceFleetManager::load(data_dir.path(), Duration::from_secs(300)).unwrap();
        let mut fleet = heartbeat("READ");
        fleet.devices.push(ready_android("WRITE"));
        manager.heartbeat_at(fleet, now.saturating_sub(10)).unwrap();
        let read = manager
            .queue_action_at("owner", info_request("READ"), now.saturating_sub(9))
            .unwrap();
        let mut tap = info_request("WRITE");
        tap.action = DeviceActionKind::Tap;
        tap.args.x = Some(10);
        tap.args.y = Some(20);
        tap.confirmed = true;
        let mutation = manager
            .queue_action_at("owner", tap, now.saturating_sub(8))
            .unwrap();
        assert!(manager
            .claim_next_at("usb-test", now.saturating_sub(7))
            .unwrap()
            .is_some());
        assert!(manager
            .claim_next_at("usb-test", now.saturating_sub(6))
            .unwrap()
            .is_some());
        drop(manager);

        let recovered =
            DeviceFleetManager::load(data_dir.path(), Duration::from_secs(300)).unwrap();
        let read = recovered.action_status("owner", &read.id).unwrap();
        assert_eq!(read.status, DeviceActionStatus::Queued);
        assert!(read.claimed_at.is_none());
        assert!(read.connector_id.is_none());
        let mutation = recovered.action_status("owner", &mutation.id).unwrap();
        assert_eq!(mutation.status, DeviceActionStatus::Expired);
        assert!(mutation.claimed_at.is_none());
        assert!(mutation.connector_id.is_none());
        assert!(mutation
            .result
            .as_ref()
            .is_some_and(|result| result.detail.contains("jamais rejouee")));
    }

    #[test]
    fn ephemeral_origin_is_scoped_and_expired_during_recovery() {
        let data_dir = TestDataDir::new("origin");
        let now = now_ts();
        let manager = DeviceFleetManager::load(data_dir.path(), Duration::from_secs(300)).unwrap();
        manager.heartbeat_at(heartbeat("ABC123"), now).unwrap();
        let mut request = info_request("ABC123");
        request.idempotency_key = Some("freebuff:action-1".to_string());
        let first = manager
            .queue_ephemeral_action_at("owner", "freebuff:terminal-a", request.clone(), now)
            .unwrap();
        let second = manager
            .queue_ephemeral_action_at("owner", "freebuff:terminal-b", request, now)
            .unwrap();
        assert_ne!(first.id, second.id);
        assert!(matches!(
            manager.action_status_for_origin("owner", "freebuff:terminal-b", &first.id),
            Err(DeviceFleetError::NotFound)
        ));
        drop(manager);

        let recovered =
            DeviceFleetManager::load(data_dir.path(), Duration::from_secs(300)).unwrap();
        assert_eq!(
            recovered
                .action_status_for_origin("owner", "freebuff:terminal-a", &first.id)
                .unwrap()
                .status,
            DeviceActionStatus::Expired
        );
        assert_eq!(
            recovered
                .action_status_for_origin("owner", "freebuff:terminal-b", &second.id)
                .unwrap()
                .status,
            DeviceActionStatus::Expired
        );
    }

    #[test]
    fn all_corrupt_final_snapshots_fail_closed_but_temps_are_ignored() {
        let data_dir = TestDataDir::new("corrupt");
        let store = data_dir.store_path();
        fs::create_dir_all(&store).unwrap();
        fs::write(
            store.join(".actions-00000000000000000001-left.tmp"),
            b"partial",
        )
        .unwrap();
        assert!(DeviceFleetManager::load(data_dir.path(), Duration::from_secs(300)).is_ok());
        fs::write(
            store.join("actions-00000000000000000001-corrupt.json"),
            b"not-json",
        )
        .unwrap();
        assert!(matches!(
            DeviceFleetManager::load(data_dir.path(), Duration::from_secs(300)),
            Err(DeviceFleetError::Internal(_))
        ));
    }

    #[test]
    fn load_falls_back_to_previous_valid_generation() {
        let data_dir = TestDataDir::new("fallback");
        let now = now_ts();
        let manager = DeviceFleetManager::load(data_dir.path(), Duration::from_secs(300)).unwrap();
        let mut fleet = heartbeat("FIRST");
        fleet.devices.push(ready_android("SECOND"));
        manager.heartbeat_at(fleet, now).unwrap();
        let first = manager
            .queue_action_at("owner", info_request("FIRST"), now)
            .unwrap();
        let second = manager
            .queue_action_at("owner", info_request("SECOND"), now)
            .unwrap();
        let snapshots = final_snapshot_paths(&data_dir);
        assert_eq!(snapshots.len(), 2);
        fs::write(&snapshots.last().unwrap().1, b"corrupted latest").unwrap();
        drop(manager);

        let recovered =
            DeviceFleetManager::load(data_dir.path(), Duration::from_secs(300)).unwrap();
        assert_eq!(
            recovered.action_status("owner", &first.id).unwrap().status,
            DeviceActionStatus::Queued
        );
        assert!(matches!(
            recovered.action_status("owner", &second.id),
            Err(DeviceFleetError::NotFound)
        ));
    }

    #[test]
    fn persistence_failure_leaves_memory_unchanged() {
        let data_dir = TestDataDir::new("write-failure");
        let now = now_ts();
        let manager = DeviceFleetManager::load(data_dir.path(), Duration::from_secs(300)).unwrap();
        manager.heartbeat_at(heartbeat("ABC123"), now).unwrap();
        fs::remove_dir(data_dir.store_path()).unwrap();
        fs::write(data_dir.store_path(), b"blocks snapshot directory").unwrap();

        let result = manager.queue_action_at("owner", info_request("ABC123"), now);
        assert!(matches!(result, Err(DeviceFleetError::Internal(_))));
        let actions = manager.inner.actions.lock().unwrap();
        assert!(actions.is_empty());
    }

    #[test]
    fn revoke_deny_list_survives_persistence_failure_and_blocks_claim() {
        let data_dir = TestDataDir::new("revoke-write-failure");
        let now = now_ts();
        let manager = DeviceFleetManager::load(data_dir.path(), Duration::from_secs(300)).unwrap();
        manager.heartbeat_at(heartbeat("ABC123"), now).unwrap();
        let queued = manager
            .queue_ephemeral_action_at("owner", "freebuff:revoked", info_request("ABC123"), now)
            .unwrap();
        let job = manager
            .claim_next_at("usb-test", now)
            .unwrap()
            .expect("claim avant revocation");
        fs::remove_dir_all(data_dir.store_path()).unwrap();
        fs::write(data_dir.store_path(), b"blocks snapshot directory").unwrap();

        assert!(matches!(
            manager.expire_origin_actions_at("owner", "freebuff:revoked", now),
            Err(DeviceFleetError::Internal(_))
        ));
        assert_eq!(
            manager.action_status("owner", &queued.id).unwrap().status,
            DeviceActionStatus::Claimed
        );
        assert!(matches!(
            manager.action_status_for_origin("owner", "freebuff:revoked", &queued.id),
            Err(DeviceFleetError::NotFound)
        ));
        assert!(manager.claim_next_at("usb-test", now).unwrap().is_none());
        assert!(matches!(
            manager.report_at(
                DeviceConnectorReportRequest {
                    connector_id: "usb-test".to_string(),
                    action_id: job.action_id.clone(),
                    claim_token: job.claim_token,
                    result: DeviceActionResult {
                        action_id: job.action_id,
                        device_id: job.device_id,
                        action: job.request.action,
                        success: true,
                        detail: "should be denied".to_string(),
                        stdout: None,
                        stderr: None,
                        data_base64: None,
                        mime_type: None,
                        started_at: now,
                        finished_at: now,
                        truncated: false,
                    },
                },
                now
            ),
            Err(DeviceFleetError::Conflict(_))
        ));
        assert_eq!(
            manager.action_status("owner", &queued.id).unwrap().status,
            DeviceActionStatus::Claimed
        );
        assert!(matches!(
            manager.queue_ephemeral_action_at(
                "owner",
                "freebuff:revoked",
                info_request("ABC123"),
                now
            ),
            Err(DeviceFleetError::Conflict(_))
        ));
    }

    #[test]
    fn screenshot_results_require_valid_base64_mime_and_signature() {
        let mut valid = DeviceActionResult {
            action_id: Uuid::new_v4().to_string(),
            device_id: "android:ABC123".to_string(),
            action: DeviceActionKind::Screenshot,
            success: true,
            detail: "ok".to_string(),
            stdout: None,
            stderr: None,
            data_base64: Some(BASE64_STANDARD.encode(b"\x89PNG\r\n\x1a\nimage")),
            mime_type: Some("image/png".to_string()),
            started_at: 1,
            finished_at: 2,
            truncated: false,
        };
        sanitize_action_result(&mut valid);
        assert!(valid.success);
        assert!(valid.data_base64.is_some());

        let mut invalid = valid.clone();
        invalid.data_base64 = Some(BASE64_STANDARD.encode(b"not-an-image"));
        sanitize_action_result(&mut invalid);
        assert!(!invalid.success);
        assert!(invalid.data_base64.is_none());
        assert!(invalid.mime_type.is_none());
    }

    #[test]
    fn persisted_output_limit_is_measured_in_utf8_bytes() {
        let bounded = sanitize_optional_bytes(
            Some("é".repeat(MAX_ACTION_OUTPUT_BYTES)),
            MAX_ACTION_OUTPUT_BYTES,
        )
        .unwrap();
        assert!(bounded.len() <= MAX_ACTION_OUTPUT_BYTES);
        assert!(bounded.is_char_boundary(bounded.len()));

        let mut result = DeviceActionResult {
            action_id: Uuid::new_v4().to_string(),
            device_id: "android:ABC123".to_string(),
            action: DeviceActionKind::Info,
            success: true,
            detail: "ok".to_string(),
            stdout: Some("é".repeat(MAX_ACTION_OUTPUT_BYTES)),
            stderr: Some("🙂".repeat(MAX_ACTION_OUTPUT_BYTES)),
            data_base64: None,
            mime_type: None,
            started_at: 1,
            finished_at: 2,
            truncated: false,
        };
        sanitize_action_result(&mut result);
        assert!(result
            .stdout
            .as_ref()
            .is_some_and(|stdout| stdout.len() <= MAX_ACTION_OUTPUT_BYTES));
        assert!(result
            .stderr
            .as_ref()
            .is_some_and(|stderr| stderr.len() <= MAX_ACTION_OUTPUT_BYTES));
    }

    #[test]
    fn serde_contract_uses_camel_case_and_stable_device_ids() {
        let value = serde_json::to_value(ready_android("ABC123")).unwrap();
        assert_eq!(value["id"], "android:ABC123");
        assert!(value.get("osVersion").is_some());
        assert!(value.get("jailbreakReady").is_some());
        assert!(value.get("os_version").is_none());

        let mut swipe = info_request("ABC123");
        swipe.action = DeviceActionKind::Swipe;
        swipe.args.start_x = Some(1);
        swipe.args.start_y = Some(2);
        swipe.args.end_x = Some(3);
        swipe.args.end_y = Some(4);
        let action = serde_json::to_value(swipe).unwrap();
        assert_eq!(action["args"]["startX"], 1);
        assert_eq!(action["args"]["endY"], 4);
        assert!(action["args"].get("toX").is_none());
    }

    #[test]
    fn push_file_requires_local_path_and_absolute_remote_path() {
        let mut request = info_request("ABC123");
        request.action = DeviceActionKind::PushFile;
        request.confirmed = true;
        request.args.local_path = Some("C:/Videos/ma-video.mp4".to_string());
        request.args.remote_path = Some("/sdcard/Pictures/ma-video.mp4".to_string());
        assert!(validate_action_request(&request).is_ok());

        let mut missing_local = request.clone();
        missing_local.args.local_path = None;
        assert!(validate_action_request(&missing_local).is_err());

        let mut relative_remote = request.clone();
        relative_remote.args.remote_path = Some("sdcard/ma-video.mp4".to_string());
        assert!(validate_action_request(&relative_remote).is_err());

        let mut control_char = request.clone();
        control_char.args.remote_path = Some("/sdcard/ma-video\u{0007}.mp4".to_string());
        assert!(validate_action_request(&control_char).is_err());

        let serialized = serde_json::to_value(&request).unwrap();
        assert_eq!(serialized["args"]["localPath"], "C:/Videos/ma-video.mp4");
        assert_eq!(serialized["args"]["remotePath"], "/sdcard/Pictures/ma-video.mp4");
        assert_eq!(serialized["action"], "push_file");
    }
}
