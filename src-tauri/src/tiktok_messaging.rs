//! Pont persistant entre les outils de chat du VPS et l'agent TikMatrix local.
//!
//! Le serveur ne connait jamais le port TikMatrix du poste Windows. Il conserve
//! seulement des campagnes confirmees. Le client desktop, deja authentifie au
//! VPS, reclame une campagne, prepare les fichiers locaux attendus par
//! TikMatrix, puis soumet la tache a son agent loopback.

use crate::{fs_util, metrics, tiktok_messaging_policy as policy};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fmt, fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use uuid::Uuid;

#[cfg(feature = "desktop")]
use serde_json::{json, Map, Value};
#[cfg(feature = "desktop")]
use std::{
    collections::HashMap,
    process::{Command, Stdio},
    time::Duration,
};

const STORE_VERSION: u32 = 1;
// Le bail doit couvrir la premiere installation de ws-scrcpy-web (telechargement
// du ZIP d'environ 80 Mo + extraction) : 10 minutes laissent une grosse marge
// meme sur une connexion lente, sans risquer de reclamer un job encore actif.
const CLAIM_LEASE_SECONDS: i64 = 600;
const CONNECTOR_STALE_SECONDS: i64 = 30;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TikTokDmCampaignStatus {
    Draft,
    Queued,
    Claimed,
    Submitted,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TikTokDmCampaign {
    pub id: String,
    pub owner_id: String,
    pub recipients: Vec<String>,
    pub message: String,
    pub status: TikTokDmCampaignStatus,
    pub min_interval_minutes: u8,
    pub max_interval_minutes: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_serial: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_account: Option<String>,
    pub owned_accounts_confirmed: bool,
    pub send_confirmed: bool,
    pub created_by: String,
    pub idempotency_key: String,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitted_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connector_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claim_token: Option<String>,
    #[serde(default)]
    pub tikmatrix_task_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareTikTokDmCampaignRequest {
    pub recipients: Vec<String>,
    pub message: String,
    #[serde(default = "default_min_interval_minutes")]
    pub min_interval_minutes: u8,
    #[serde(default = "default_max_interval_minutes")]
    pub max_interval_minutes: u8,
    #[serde(default)]
    pub device_serial: Option<String>,
    #[serde(default)]
    pub sender_account: Option<String>,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmTikTokDmCampaignRequest {
    pub campaign_id: String,
    #[serde(default)]
    pub owned_accounts_confirmed: bool,
    #[serde(default)]
    pub send_confirmed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TikTokConnectorStatus {
    pub connector_id: String,
    pub last_seen_at: i64,
    pub agent_healthy: bool,
    pub device_serials: Vec<String>,
    #[serde(default)]
    pub devices: Vec<TikTokAndroidDevice>,
    #[serde(default)]
    pub scrcpy_available: bool,
    #[serde(default)]
    pub ws_scrcpy_available: bool,
    #[serde(default)]
    pub ws_scrcpy_online: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adb_error: Option<String>,
    pub account_count: usize,
    #[serde(default)]
    pub accounts: Vec<TikTokSenderAccount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_campaign_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl TikTokConnectorStatus {
    pub fn is_online(&self, now: i64) -> bool {
        self.last_seen_at
            .saturating_add(CONNECTOR_STALE_SECONDS)
            .ge(&now)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TikTokConnectorHeartbeatRequest {
    pub connector_id: String,
    pub agent_healthy: bool,
    #[serde(default)]
    pub device_serials: Vec<String>,
    #[serde(default)]
    pub devices: Vec<TikTokAndroidDevice>,
    #[serde(default)]
    pub scrcpy_available: bool,
    #[serde(default)]
    pub ws_scrcpy_available: bool,
    #[serde(default)]
    pub ws_scrcpy_online: bool,
    #[serde(default)]
    pub adb_error: Option<String>,
    #[serde(default)]
    pub account_count: usize,
    #[serde(default)]
    pub accounts: Vec<TikTokSenderAccount>,
    #[serde(default)]
    pub current_campaign_id: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TikTokAndroidDeviceState {
    Device,
    Unauthorized,
    Offline,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TikTokAndroidDeviceTransport {
    Usb,
    Emulator,
    Network,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TikTokAndroidDevice {
    pub serial: String,
    #[serde(default)]
    pub state: TikTokAndroidDeviceState,
    #[serde(default)]
    pub transport: TikTokAndroidDeviceTransport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
    #[serde(default)]
    pub tikmatrix_managed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TikTokConnectorJob {
    pub campaign_id: String,
    pub claim_token: String,
    pub recipients: Vec<String>,
    pub message: String,
    pub min_interval_minutes: u8,
    pub max_interval_minutes: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_serial: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_account: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TikTokSenderAccount {
    pub username: String,
    pub device_serial: String,
    pub logged_in: bool,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TikTokSenderAccountView {
    pub username: String,
    pub device_serial: String,
    pub logged_in: bool,
    pub enabled: bool,
    pub connected: bool,
    pub selected: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TikTokSenderSetupActionKind {
    OpenLogin,
    MatchAccounts,
    OpenScrcpy,
    StartWsScrcpy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TikTokSenderSetupActionStatus {
    Queued,
    Claimed,
    Submitted,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TikTokSenderSetupAction {
    pub id: String,
    pub owner_id: String,
    pub action: TikTokSenderSetupActionKind,
    pub status: TikTokSenderSetupActionStatus,
    pub device_serial: String,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitted_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connector_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claim_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueueTikTokSenderSetupRequest {
    pub action: TikTokSenderSetupActionKind,
    #[serde(default)]
    pub device_serial: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TikTokSenderSetupJob {
    pub action_id: String,
    pub claim_token: String,
    pub action: TikTokSenderSetupActionKind,
    pub device_serial: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TikTokSenderSetupReportRequest {
    pub connector_id: String,
    pub action_id: String,
    pub claim_token: String,
    pub success: bool,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TikTokConnectorClaimRequest {
    pub connector_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TikTokConnectorReportRequest {
    pub connector_id: String,
    pub campaign_id: String,
    pub claim_token: String,
    pub success: bool,
    #[serde(default)]
    pub tikmatrix_task_count: u64,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TikTokFollowerExtractionStatus {
    Queued,
    Claimed,
    Submitted,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TikTokFollowerDmPipeline {
    pub owned_recipient_allowlist: Vec<String>,
    pub message: String,
    pub owned_accounts_confirmed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepared_campaign_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TikTokFollowerDmPipelineRequest {
    pub owned_recipient_allowlist: Vec<String>,
    pub message: String,
    #[serde(default)]
    pub owned_accounts_confirmed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TikTokFollowerExtraction {
    pub id: String,
    pub owner_id: String,
    pub target_username: String,
    pub max_count: u16,
    pub status: TikTokFollowerExtractionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_serial: Option<String>,
    pub authorized_account_confirmed: bool,
    pub created_by: String,
    pub idempotency_key: String,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitted_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connector_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claim_token: Option<String>,
    #[serde(default)]
    pub tikmatrix_task_count: u64,
    #[serde(default)]
    pub usernames: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dm_pipeline: Option<TikTokFollowerDmPipeline>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueTikTokFollowerExtractionRequest {
    pub target_username: String,
    #[serde(default = "default_follower_max_count")]
    pub max_count: u16,
    #[serde(default)]
    pub device_serial: Option<String>,
    #[serde(default)]
    pub authorized_account_confirmed: bool,
    #[serde(default)]
    pub dm_pipeline: Option<TikTokFollowerDmPipelineRequest>,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TikTokFollowerConnectorJob {
    pub extraction_id: String,
    pub claim_token: String,
    pub target_username: String,
    pub max_count: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_serial: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TikTokFollowerSubmissionReportRequest {
    pub connector_id: String,
    pub extraction_id: String,
    pub claim_token: String,
    pub success: bool,
    #[serde(default)]
    pub tikmatrix_task_count: u64,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TikTokFollowerPendingResult {
    pub extraction_id: String,
    pub target_username: String,
    pub max_count: u16,
    pub submitted_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TikTokFollowerResultReportRequest {
    pub connector_id: String,
    pub extraction_id: String,
    pub success: bool,
    #[serde(default)]
    pub usernames: Vec<String>,
    #[serde(default)]
    pub output_file: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct TikTokDmStore {
    version: u32,
    #[serde(default)]
    campaigns: Vec<TikTokDmCampaign>,
    #[serde(default)]
    follower_extractions: Vec<TikTokFollowerExtraction>,
    #[serde(default)]
    sender_bindings: Vec<TikTokSenderBinding>,
    #[serde(default)]
    sender_setup_actions: Vec<TikTokSenderSetupAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct TikTokSenderBinding {
    owner_id: String,
    username: String,
    device_serial: String,
    updated_at: i64,
}

impl Default for TikTokDmStore {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            campaigns: Vec::new(),
            follower_extractions: Vec::new(),
            sender_bindings: Vec::new(),
            sender_setup_actions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TikTokDmError {
    Validation(String),
    NotFound,
    Conflict(String),
    Internal(String),
}

impl fmt::Display for TikTokDmError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(message) | Self::Conflict(message) | Self::Internal(message) => {
                formatter.write_str(message)
            }
            Self::NotFound => formatter.write_str("Campagne TikTok introuvable"),
        }
    }
}

impl std::error::Error for TikTokDmError {}

#[derive(Clone)]
pub struct TikTokDmManager {
    inner: Arc<TikTokDmInner>,
}

struct TikTokDmInner {
    storage_path: PathBuf,
    store: Mutex<TikTokDmStore>,
    connector: Mutex<Option<TikTokConnectorStatus>>,
}

impl TikTokDmManager {
    pub fn new(storage_path: PathBuf) -> Result<Self, String> {
        let store = load_store(&storage_path).map_err(|error| error.to_string())?;
        Ok(Self {
            inner: Arc::new(TikTokDmInner {
                storage_path,
                store: Mutex::new(store),
                connector: Mutex::new(None),
            }),
        })
    }

    pub fn prepare(
        &self,
        owner_id: &str,
        request: PrepareTikTokDmCampaignRequest,
        created_by: &str,
    ) -> Result<TikTokDmCampaign, TikTokDmError> {
        let policy::ValidatedCampaignRequest {
            owner_id,
            recipients,
            message,
            min_interval_minutes,
            max_interval_minutes,
            device_serial,
            sender_account,
            idempotency_key,
        } = policy::validate_campaign_request(owner_id, request)?;

        let mut current = self.lock_store()?;
        if let Some(existing) = current.campaigns.iter().find(|campaign| {
            campaign.owner_id == owner_id && campaign.idempotency_key == idempotency_key
        }) {
            return Ok(existing.clone());
        }
        policy::ensure_campaign_capacity(current.campaigns.len())?;
        let now = metrics::now_ts();
        let campaign = TikTokDmCampaign {
            id: Uuid::new_v4().to_string(),
            owner_id,
            recipients,
            message,
            status: TikTokDmCampaignStatus::Draft,
            min_interval_minutes,
            max_interval_minutes,
            device_serial,
            sender_account,
            owned_accounts_confirmed: false,
            send_confirmed: false,
            created_by: normalize_creator(created_by),
            idempotency_key,
            created_at: now,
            updated_at: now,
            confirmed_at: None,
            claimed_at: None,
            submitted_at: None,
            connector_id: None,
            claim_token: None,
            tikmatrix_task_count: 0,
            error: None,
        };
        let mut next = current.clone();
        next.campaigns.push(campaign.clone());
        self.persist_and_replace(&mut current, next)?;
        Ok(campaign)
    }

    pub fn confirm(
        &self,
        owner_id: &str,
        request: ConfirmTikTokDmCampaignRequest,
    ) -> Result<TikTokDmCampaign, TikTokDmError> {
        if !policy::campaign_confirmation_guard(&request) {
            return Err(policy::campaign_confirmation_rejection());
        }
        let mut current = self.lock_store()?;
        let mut next = current.clone();
        let campaign = next
            .campaigns
            .iter_mut()
            .find(|campaign| campaign.id == request.campaign_id && campaign.owner_id == owner_id)
            .ok_or(TikTokDmError::NotFound)?;
        if campaign.status != TikTokDmCampaignStatus::Draft
            && campaign.owned_accounts_confirmed
            && campaign.send_confirmed
        {
            return Ok(campaign.clone());
        }
        if campaign.status != TikTokDmCampaignStatus::Draft {
            return Err(TikTokDmError::Conflict(
                "Seul un brouillon TikTok peut etre confirme".to_string(),
            ));
        }
        let now = metrics::now_ts();
        campaign.owned_accounts_confirmed = true;
        campaign.send_confirmed = true;
        campaign.confirmed_at = Some(now);
        campaign.updated_at = now;
        campaign.status = TikTokDmCampaignStatus::Queued;
        campaign.error = None;
        let result = campaign.clone();
        self.persist_and_replace(&mut current, next)?;
        Ok(result)
    }

    pub fn list(&self, owner_id: &str) -> Result<Vec<TikTokDmCampaign>, TikTokDmError> {
        let store = self.lock_store()?;
        let mut campaigns = store
            .campaigns
            .iter()
            .filter(|campaign| campaign.owner_id == owner_id)
            .cloned()
            .collect::<Vec<_>>();
        campaigns.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(campaigns)
    }

    pub fn connector_status(&self) -> Result<Option<TikTokConnectorStatus>, TikTokDmError> {
        let connector =
            self.inner.connector.lock().map_err(|_| {
                TikTokDmError::Internal("Etat du connecteur verrouille".to_string())
            })?;
        Ok(connector.clone())
    }

    pub fn sender_accounts(
        &self,
        owner_id: &str,
    ) -> Result<Vec<TikTokSenderAccountView>, TikTokDmError> {
        let owner_id = policy::validate_owner_id(owner_id)?;
        let connector = self.connector_status()?;
        let store = self.lock_store()?;
        let selected = store
            .sender_bindings
            .iter()
            .find(|binding| binding.owner_id == owner_id);
        let Some(connector) = connector else {
            return Ok(Vec::new());
        };
        let online = connector.is_online(metrics::now_ts()) && connector.agent_healthy;
        let mut accounts = connector
            .accounts
            .iter()
            .map(|account| TikTokSenderAccountView {
                username: account.username.clone(),
                device_serial: account.device_serial.clone(),
                logged_in: account.logged_in,
                enabled: account.enabled,
                connected: online
                    && connector
                        .device_serials
                        .iter()
                        .any(|serial| serial == &account.device_serial),
                selected: selected.is_some_and(|binding| {
                    binding.username == account.username
                        && binding.device_serial == account.device_serial
                }),
            })
            .collect::<Vec<_>>();
        accounts.sort_by(|left, right| {
            right
                .selected
                .cmp(&left.selected)
                .then_with(|| left.username.cmp(&right.username))
                .then_with(|| left.device_serial.cmp(&right.device_serial))
        });
        Ok(accounts)
    }

    pub fn select_sender_account(
        &self,
        owner_id: &str,
        username: &str,
    ) -> Result<TikTokSenderAccountView, TikTokDmError> {
        let owner_id = policy::validate_owner_id(owner_id)?;
        let username = policy::normalize_tiktok_username(username)?;
        let accounts = self.sender_accounts(&owner_id)?;
        let account = accounts
            .iter()
            .find(|account| account.username.eq_ignore_ascii_case(&username))
            .cloned()
            .ok_or_else(|| {
                TikTokDmError::Validation(format!(
                    "Le compte emetteur {username} n'est pas reconnu par TikMatrix. Connectez-le sur l'appareil puis lancez Match Accounts."
                ))
            })?;
        ensure_sender_account_ready(&account, &accounts)?;

        let mut current = self.lock_store()?;
        let mut next = current.clone();
        next.sender_bindings
            .retain(|binding| binding.owner_id != owner_id);
        next.sender_bindings.push(TikTokSenderBinding {
            owner_id,
            username: account.username.clone(),
            device_serial: account.device_serial.clone(),
            updated_at: metrics::now_ts(),
        });
        self.persist_and_replace(&mut current, next)?;
        Ok(TikTokSenderAccountView {
            selected: true,
            ..account
        })
    }

    pub fn resolve_sender_account(
        &self,
        owner_id: &str,
        requested_username: Option<&str>,
        requested_device_serial: Option<&str>,
    ) -> Result<TikTokSenderAccountView, TikTokDmError> {
        let accounts = self.sender_accounts(owner_id)?;
        let requested_username = requested_username
            .map(policy::normalize_tiktok_username)
            .transpose()?;
        let requested_device_serial = requested_device_serial
            .map(policy::validate_device_serial)
            .transpose()?;
        let mut candidates = accounts
            .iter()
            .filter(|account| {
                requested_username
                    .as_ref()
                    .is_none_or(|username| account.username.eq_ignore_ascii_case(username))
                    && requested_device_serial
                        .as_ref()
                        .is_none_or(|serial| account.device_serial == *serial)
            })
            .cloned()
            .collect::<Vec<_>>();

        if requested_username.is_none() && requested_device_serial.is_none() {
            if let Some(selected) = candidates.iter().find(|account| account.selected).cloned() {
                ensure_sender_account_ready(&selected, &accounts)?;
                return Ok(selected);
            }
            candidates.retain(|account| account.logged_in && account.enabled && account.connected);
            if candidates.len() == 1 {
                return Ok(candidates.remove(0));
            }
        }

        let account = match candidates.as_slice() {
            [account] => account.clone(),
            [] => {
                return Err(TikTokDmError::Validation(
                    "Aucun compte emetteur TikTok correspondant n'est disponible. Connectez le compte sur Windows, lancez Match Accounts, puis selectionnez-le depuis le chat."
                        .to_string(),
                ))
            }
            _ => {
                return Err(TikTokDmError::Validation(
                    "Plusieurs comptes emetteurs TikTok correspondent. Indiquez senderAccount ou selectionnez un compte emetteur par defaut."
                        .to_string(),
                ))
            }
        };
        ensure_sender_account_ready(&account, &accounts)?;
        Ok(account)
    }

    pub fn queue_sender_setup(
        &self,
        owner_id: &str,
        request: QueueTikTokSenderSetupRequest,
    ) -> Result<TikTokSenderSetupAction, TikTokDmError> {
        let owner_id = policy::validate_owner_id(owner_id)?;
        let requested_device = if request.action == TikTokSenderSetupActionKind::StartWsScrcpy {
            None
        } else {
            request
                .device_serial
                .as_deref()
                .map(policy::validate_device_serial)
                .transpose()?
        };
        let connector = self.connector_status()?.ok_or_else(|| {
            TikTokDmError::Conflict(
                "Le pont Android Windows est hors ligne. Demarrez l'application desktop."
                    .to_string(),
            )
        })?;
        if !connector.is_online(metrics::now_ts()) {
            return Err(TikTokDmError::Conflict(
                "Le pont Android Windows est hors ligne. Demarrez l'application desktop."
                    .to_string(),
            ));
        }
        if !matches!(
            request.action,
            TikTokSenderSetupActionKind::OpenScrcpy | TikTokSenderSetupActionKind::StartWsScrcpy
        ) && !connector.agent_healthy
        {
            return Err(TikTokDmError::Conflict(
                "TikMatrix est hors ligne ou son agent local ne repond pas.".to_string(),
            ));
        }
        if request.action == TikTokSenderSetupActionKind::OpenScrcpy && !connector.scrcpy_available
        {
            return Err(TikTokDmError::Conflict(
                "scrcpy est introuvable sur le poste Windows. Installez-le ou configurez CST_SCRCPY_PATH."
                    .to_string(),
            ));
        }
        // Note : contrairement a scrcpy, aucun garde-fou d'installation n'est
        // oppose ici a StartWsScrcpy : le connecteur Windows provisionne
        // ws-scrcpy-web automatiquement (telechargement verifie par SHA-256)
        // au premier demarrage quand aucun lanceur n'est configure.
        let device_serial = if request.action == TikTokSenderSetupActionKind::StartWsScrcpy {
            String::new()
        } else {
            match requested_device {
            Some(serial) => {
                if !connector
                    .device_serials
                    .iter()
                    .any(|connected| connected == &serial)
                {
                    return Err(TikTokDmError::Validation(format!(
                        "L'appareil Android {serial} n'est pas connecte ou n'est pas autorise par ADB"
                    )));
                }
                serial
            }
            None => match connector.device_serials.as_slice() {
                [serial] => serial.clone(),
                [] => {
                    return Err(TikTokDmError::Conflict(
                        "Aucun appareil Android autorise n'est connecte par ADB.".to_string(),
                    ))
                }
                _ => {
                    return Err(TikTokDmError::Validation(
                        "Plusieurs appareils Android sont connectes. Indiquez deviceSerial pour choisir la fenetre de connexion."
                            .to_string(),
                    ))
                }
            },
            }
        };

        let mut current = self.lock_store()?;
        if let Some(existing) = current.sender_setup_actions.iter().find(|action| {
            action.owner_id == owner_id
                && action.action == request.action
                && action.device_serial == device_serial
                && matches!(
                    action.status,
                    TikTokSenderSetupActionStatus::Queued | TikTokSenderSetupActionStatus::Claimed
                )
        }) {
            let mut public_action = existing.clone();
            public_action.claim_token = None;
            return Ok(public_action);
        }
        let now = metrics::now_ts();
        let mut next = current.clone();
        next.sender_setup_actions.retain(|action| {
            matches!(
                action.status,
                TikTokSenderSetupActionStatus::Queued | TikTokSenderSetupActionStatus::Claimed
            ) || action.updated_at.saturating_add(7 * 24 * 60 * 60) > now
        });
        if next.sender_setup_actions.len() >= 100 {
            return Err(TikTokDmError::Conflict(
                "Trop d'actions de connexion TikTok sont conservees".to_string(),
            ));
        }
        let action = TikTokSenderSetupAction {
            id: Uuid::new_v4().to_string(),
            owner_id,
            action: request.action,
            status: TikTokSenderSetupActionStatus::Queued,
            device_serial,
            created_at: now,
            updated_at: now,
            claimed_at: None,
            submitted_at: None,
            connector_id: None,
            claim_token: None,
            detail: None,
        };
        next.sender_setup_actions.push(action.clone());
        self.persist_and_replace(&mut current, next)?;
        Ok(action)
    }

    pub fn claim_next_sender_setup(
        &self,
        connector_id: &str,
    ) -> Result<Option<TikTokSenderSetupJob>, TikTokDmError> {
        let connector_id = policy::validate_connector_id(connector_id)?;
        let mut current = self.lock_store()?;
        let mut next = current.clone();
        let now = metrics::now_ts();
        for action in &mut next.sender_setup_actions {
            if action.status == TikTokSenderSetupActionStatus::Claimed
                && action
                    .claimed_at
                    .is_some_and(|claimed| claimed.saturating_add(CLAIM_LEASE_SECONDS) <= now)
            {
                action.status = TikTokSenderSetupActionStatus::Queued;
                action.claimed_at = None;
                action.connector_id = None;
                action.claim_token = None;
                action.updated_at = now;
                action.detail =
                    Some("Le connecteur precedent n'a pas confirme cette action".to_string());
            }
        }
        let job = next
            .sender_setup_actions
            .iter_mut()
            .find(|action| action.status == TikTokSenderSetupActionStatus::Queued)
            .map(|action| {
                let claim_token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
                action.status = TikTokSenderSetupActionStatus::Claimed;
                action.claimed_at = Some(now);
                action.updated_at = now;
                action.connector_id = Some(connector_id);
                action.claim_token = Some(claim_token.clone());
                action.detail = None;
                TikTokSenderSetupJob {
                    action_id: action.id.clone(),
                    claim_token,
                    action: action.action,
                    device_serial: action.device_serial.clone(),
                }
            });
        if next != *current {
            self.persist_and_replace(&mut current, next)?;
        }
        Ok(job)
    }

    pub fn report_sender_setup(
        &self,
        request: TikTokSenderSetupReportRequest,
    ) -> Result<TikTokSenderSetupAction, TikTokDmError> {
        let connector_id = policy::validate_connector_id(&request.connector_id)?;
        let action_id = policy::validate_campaign_id(&request.action_id)?;
        let mut current = self.lock_store()?;
        let mut next = current.clone();
        let action = next
            .sender_setup_actions
            .iter_mut()
            .find(|action| action.id == action_id)
            .ok_or(TikTokDmError::NotFound)?;
        if action.status == TikTokSenderSetupActionStatus::Submitted && request.success {
            return Ok(action.clone());
        }
        if action.status != TikTokSenderSetupActionStatus::Claimed
            || action.connector_id.as_deref() != Some(connector_id.as_str())
            || action.claim_token.as_deref() != Some(request.claim_token.trim())
        {
            return Err(TikTokDmError::Conflict(
                "Lease de connexion TikTok absente ou expiree".to_string(),
            ));
        }
        let now = metrics::now_ts();
        action.updated_at = now;
        action.claim_token = None;
        action.detail = sanitize_detail(request.detail);
        if request.success {
            action.status = TikTokSenderSetupActionStatus::Submitted;
            action.submitted_at = Some(now);
        } else {
            action.status = TikTokSenderSetupActionStatus::Failed;
            if action.detail.is_none() {
                action.detail = Some("Action de connexion TikMatrix impossible".to_string());
            }
        }
        let result = action.clone();
        self.persist_and_replace(&mut current, next)?;
        Ok(result)
    }

    pub fn heartbeat(
        &self,
        request: TikTokConnectorHeartbeatRequest,
    ) -> Result<TikTokConnectorStatus, TikTokDmError> {
        let connector_id = policy::validate_connector_id(&request.connector_id)?;
        let mut device_serials = request
            .device_serials
            .into_iter()
            .map(|serial| policy::validate_device_serial(&serial))
            .collect::<Result<Vec<_>, _>>()?;
        let mut devices = request
            .devices
            .into_iter()
            .map(validate_android_device)
            .collect::<Result<Vec<_>, _>>()?;
        devices.sort_by(|left, right| left.serial.cmp(&right.serial));
        devices.dedup_by(|left, right| left.serial == right.serial);
        devices.truncate(50);
        device_serials.extend(
            devices
                .iter()
                .filter(|device| device.state == TikTokAndroidDeviceState::Device)
                .map(|device| device.serial.clone()),
        );
        device_serials.sort();
        device_serials.dedup();
        device_serials.truncate(50);
        let mut accounts = request
            .accounts
            .into_iter()
            .map(validate_sender_account)
            .collect::<Result<Vec<_>, _>>()?;
        accounts.sort_by(|left, right| {
            left.username
                .cmp(&right.username)
                .then_with(|| left.device_serial.cmp(&right.device_serial))
        });
        accounts.dedup_by(|left, right| {
            left.username == right.username && left.device_serial == right.device_serial
        });
        let account_count = request.account_count.max(accounts.len()).min(100);
        let status = TikTokConnectorStatus {
            connector_id,
            last_seen_at: metrics::now_ts(),
            agent_healthy: request.agent_healthy,
            device_serials,
            devices,
            scrcpy_available: request.scrcpy_available,
            ws_scrcpy_available: request.ws_scrcpy_available,
            ws_scrcpy_online: request.ws_scrcpy_online,
            adb_error: sanitize_detail(request.adb_error),
            account_count,
            accounts,
            current_campaign_id: request
                .current_campaign_id
                .map(|value| policy::validate_campaign_id(&value))
                .transpose()?,
            error: sanitize_detail(request.error),
        };
        let mut connector =
            self.inner.connector.lock().map_err(|_| {
                TikTokDmError::Internal("Etat du connecteur verrouille".to_string())
            })?;
        *connector = Some(status.clone());
        Ok(status)
    }

    pub fn claim_next(
        &self,
        connector_id: &str,
    ) -> Result<Option<TikTokConnectorJob>, TikTokDmError> {
        let connector_id = policy::validate_connector_id(connector_id)?;
        let mut current = self.lock_store()?;
        let mut next = current.clone();
        let now = metrics::now_ts();
        for campaign in &mut next.campaigns {
            if campaign.status == TikTokDmCampaignStatus::Claimed
                && campaign
                    .claimed_at
                    .is_some_and(|claimed| claimed.saturating_add(CLAIM_LEASE_SECONDS) <= now)
            {
                campaign.status = TikTokDmCampaignStatus::Queued;
                campaign.claimed_at = None;
                campaign.connector_id = None;
                campaign.claim_token = None;
                campaign.updated_at = now;
                campaign.error = Some(
                    "Le connecteur precedent n'a pas confirme la soumission ; reprise securisee"
                        .to_string(),
                );
            }
        }
        let job = next
            .campaigns
            .iter_mut()
            .find(|campaign| campaign.status == TikTokDmCampaignStatus::Queued)
            .map(|campaign| {
                let claim_token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
                campaign.status = TikTokDmCampaignStatus::Claimed;
                campaign.claimed_at = Some(now);
                campaign.updated_at = now;
                campaign.connector_id = Some(connector_id);
                campaign.claim_token = Some(claim_token.clone());
                campaign.error = None;
                TikTokConnectorJob {
                    campaign_id: campaign.id.clone(),
                    claim_token,
                    recipients: campaign.recipients.clone(),
                    message: campaign.message.clone(),
                    min_interval_minutes: campaign.min_interval_minutes,
                    max_interval_minutes: campaign.max_interval_minutes,
                    device_serial: campaign.device_serial.clone(),
                    sender_account: campaign.sender_account.clone(),
                }
            });
        if next != *current {
            self.persist_and_replace(&mut current, next)?;
        }
        Ok(job)
    }

    pub fn report(
        &self,
        request: TikTokConnectorReportRequest,
    ) -> Result<TikTokDmCampaign, TikTokDmError> {
        let connector_id = policy::validate_connector_id(&request.connector_id)?;
        let campaign_id = policy::validate_campaign_id(&request.campaign_id)?;
        let mut current = self.lock_store()?;
        let mut next = current.clone();
        let campaign = next
            .campaigns
            .iter_mut()
            .find(|campaign| campaign.id == campaign_id)
            .ok_or(TikTokDmError::NotFound)?;

        // Une reponse identique est acceptee : le client conserve un recu
        // local apres l'appel TikMatrix et peut devoir le renvoyer apres une
        // coupure reseau.
        if campaign.status == TikTokDmCampaignStatus::Submitted && request.success {
            return Ok(campaign.clone());
        }
        if campaign.status != TikTokDmCampaignStatus::Claimed
            || campaign.connector_id.as_deref() != Some(connector_id.as_str())
            || campaign.claim_token.as_deref() != Some(request.claim_token.trim())
        {
            return Err(TikTokDmError::Conflict(
                "Lease du connecteur absente ou expiree".to_string(),
            ));
        }

        let now = metrics::now_ts();
        campaign.updated_at = now;
        campaign.claim_token = None;
        if request.success {
            campaign.status = TikTokDmCampaignStatus::Submitted;
            campaign.submitted_at = Some(now);
            campaign.tikmatrix_task_count = request.tikmatrix_task_count;
            campaign.error = None;
        } else {
            campaign.status = TikTokDmCampaignStatus::Failed;
            campaign.error = Some(
                sanitize_detail(request.detail)
                    .unwrap_or_else(|| "Soumission TikMatrix impossible".to_string()),
            );
        }
        let result = campaign.clone();
        self.persist_and_replace(&mut current, next)?;
        Ok(result)
    }

    pub fn queue_follower_extraction(
        &self,
        owner_id: &str,
        request: QueueTikTokFollowerExtractionRequest,
        created_by: &str,
    ) -> Result<TikTokFollowerExtraction, TikTokDmError> {
        if !policy::follower_extraction_guard(&request) {
            return Err(policy::follower_extraction_rejection(&request));
        }
        let policy::ValidatedFollowerExtractionRequest {
            owner_id,
            target_username,
            max_count,
            device_serial,
            authorized_account_confirmed,
            dm_pipeline,
            idempotency_key,
        } = policy::validate_follower_extraction_request(owner_id, request)?;

        let mut current = self.lock_store()?;
        if let Some(existing) = current.follower_extractions.iter().find(|extraction| {
            extraction.owner_id == owner_id && extraction.idempotency_key == idempotency_key
        }) {
            return Ok(existing.clone());
        }
        policy::ensure_follower_extraction_capacity(current.follower_extractions.len())?;
        let now = metrics::now_ts();
        let extraction = TikTokFollowerExtraction {
            id: Uuid::new_v4().to_string(),
            owner_id,
            target_username,
            max_count,
            status: TikTokFollowerExtractionStatus::Queued,
            device_serial,
            authorized_account_confirmed,
            created_by: normalize_creator(created_by),
            idempotency_key,
            created_at: now,
            updated_at: now,
            claimed_at: None,
            submitted_at: None,
            completed_at: None,
            connector_id: None,
            claim_token: None,
            tikmatrix_task_count: 0,
            usernames: Vec::new(),
            output_file: None,
            dm_pipeline,
            error: None,
        };
        let mut next = current.clone();
        next.follower_extractions.push(extraction.clone());
        self.persist_and_replace(&mut current, next)?;
        Ok(extraction)
    }

    pub fn list_follower_extractions(
        &self,
        owner_id: &str,
    ) -> Result<Vec<TikTokFollowerExtraction>, TikTokDmError> {
        let store = self.lock_store()?;
        let mut extractions = store
            .follower_extractions
            .iter()
            .filter(|extraction| extraction.owner_id == owner_id)
            .cloned()
            .collect::<Vec<_>>();
        extractions.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(extractions)
    }

    pub fn claim_next_follower_extraction(
        &self,
        connector_id: &str,
    ) -> Result<Option<TikTokFollowerConnectorJob>, TikTokDmError> {
        let connector_id = policy::validate_connector_id(connector_id)?;
        let mut current = self.lock_store()?;
        let mut next = current.clone();
        let now = metrics::now_ts();
        for extraction in &mut next.follower_extractions {
            if extraction.status == TikTokFollowerExtractionStatus::Claimed
                && extraction
                    .claimed_at
                    .is_some_and(|claimed| claimed.saturating_add(CLAIM_LEASE_SECONDS) <= now)
            {
                extraction.status = TikTokFollowerExtractionStatus::Queued;
                extraction.claimed_at = None;
                extraction.connector_id = None;
                extraction.claim_token = None;
                extraction.updated_at = now;
                extraction.error = Some(
                    "Le connecteur precedent n'a pas confirme la collecte ; reprise securisee"
                        .to_string(),
                );
            }
        }
        let job = next
            .follower_extractions
            .iter_mut()
            .find(|extraction| extraction.status == TikTokFollowerExtractionStatus::Queued)
            .map(|extraction| {
                let claim_token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
                extraction.status = TikTokFollowerExtractionStatus::Claimed;
                extraction.claimed_at = Some(now);
                extraction.updated_at = now;
                extraction.connector_id = Some(connector_id);
                extraction.claim_token = Some(claim_token.clone());
                extraction.error = None;
                TikTokFollowerConnectorJob {
                    extraction_id: extraction.id.clone(),
                    claim_token,
                    target_username: extraction.target_username.clone(),
                    max_count: extraction.max_count,
                    device_serial: extraction.device_serial.clone(),
                }
            });
        if next != *current {
            self.persist_and_replace(&mut current, next)?;
        }
        Ok(job)
    }

    pub fn report_follower_submission(
        &self,
        request: TikTokFollowerSubmissionReportRequest,
    ) -> Result<TikTokFollowerExtraction, TikTokDmError> {
        let connector_id = policy::validate_connector_id(&request.connector_id)?;
        let extraction_id = policy::validate_campaign_id(&request.extraction_id)?;
        let mut current = self.lock_store()?;
        let mut next = current.clone();
        let extraction = next
            .follower_extractions
            .iter_mut()
            .find(|extraction| extraction.id == extraction_id)
            .ok_or(TikTokDmError::NotFound)?;
        if matches!(
            extraction.status,
            TikTokFollowerExtractionStatus::Submitted | TikTokFollowerExtractionStatus::Completed
        ) && request.success
        {
            return Ok(extraction.clone());
        }
        if extraction.status != TikTokFollowerExtractionStatus::Claimed
            || extraction.connector_id.as_deref() != Some(connector_id.as_str())
            || extraction.claim_token.as_deref() != Some(request.claim_token.trim())
        {
            return Err(TikTokDmError::Conflict(
                "Lease de collecte TikTok absente ou expiree".to_string(),
            ));
        }
        let now = metrics::now_ts();
        extraction.updated_at = now;
        extraction.claim_token = None;
        if request.success {
            extraction.status = TikTokFollowerExtractionStatus::Submitted;
            extraction.submitted_at = Some(now);
            extraction.tikmatrix_task_count = request.tikmatrix_task_count;
            extraction.error = None;
        } else {
            extraction.status = TikTokFollowerExtractionStatus::Failed;
            extraction.error =
                Some(sanitize_detail(request.detail).unwrap_or_else(|| {
                    "Soumission de la collecte TikMatrix impossible".to_string()
                }));
        }
        let result = extraction.clone();
        self.persist_and_replace(&mut current, next)?;
        Ok(result)
    }

    pub fn pending_follower_results(
        &self,
        connector_id: &str,
    ) -> Result<Vec<TikTokFollowerPendingResult>, TikTokDmError> {
        let connector_id = policy::validate_connector_id(connector_id)?;
        let store = self.lock_store()?;
        Ok(store
            .follower_extractions
            .iter()
            .filter(|extraction| {
                extraction.status == TikTokFollowerExtractionStatus::Submitted
                    && extraction.connector_id.as_deref() == Some(connector_id.as_str())
            })
            .filter_map(|extraction| {
                Some(TikTokFollowerPendingResult {
                    extraction_id: extraction.id.clone(),
                    target_username: extraction.target_username.clone(),
                    max_count: extraction.max_count,
                    submitted_at: extraction.submitted_at?,
                })
            })
            .take(10)
            .collect())
    }

    pub fn report_follower_result(
        &self,
        request: TikTokFollowerResultReportRequest,
    ) -> Result<TikTokFollowerExtraction, TikTokDmError> {
        let connector_id = policy::validate_connector_id(&request.connector_id)?;
        let extraction_id = policy::validate_campaign_id(&request.extraction_id)?;
        let mut current = self.lock_store()?;
        let mut next = current.clone();
        let can_prepare_campaign = policy::campaign_capacity_available(next.campaigns.len());
        let mut prepared_campaign = None;
        let extraction = next
            .follower_extractions
            .iter_mut()
            .find(|extraction| extraction.id == extraction_id)
            .ok_or(TikTokDmError::NotFound)?;
        if extraction.status == TikTokFollowerExtractionStatus::Completed && request.success {
            return Ok(extraction.clone());
        }
        if extraction.status != TikTokFollowerExtractionStatus::Submitted
            || extraction.connector_id.as_deref() != Some(connector_id.as_str())
        {
            return Err(TikTokDmError::Conflict(
                "Collecte TikTok non soumise par ce connecteur".to_string(),
            ));
        }
        let now = metrics::now_ts();
        extraction.updated_at = now;
        if request.success {
            let mut seen = HashSet::new();
            let mut usernames = Vec::new();
            for value in request.usernames {
                let username = policy::normalize_tiktok_username(&value)?;
                if seen.insert(username.to_ascii_lowercase()) {
                    usernames.push(username);
                }
                if usernames.len() >= usize::from(extraction.max_count) {
                    break;
                }
            }
            extraction.status = TikTokFollowerExtractionStatus::Completed;
            extraction.completed_at = Some(now);
            extraction.usernames = usernames;
            extraction.output_file = sanitize_output_file(request.output_file);
            extraction.error = None;
            let available = extraction
                .usernames
                .iter()
                .map(|username| username.to_ascii_lowercase())
                .collect::<HashSet<_>>();
            let owner_id = extraction.owner_id.clone();
            let device_serial = extraction.device_serial.clone();
            let extraction_id = extraction.id.clone();
            if let Some(pipeline) = extraction.dm_pipeline.as_mut() {
                let recipients = pipeline
                    .owned_recipient_allowlist
                    .iter()
                    .filter(|username| available.contains(&username.to_ascii_lowercase()))
                    .cloned()
                    .collect::<Vec<_>>();
                if recipients.is_empty() {
                    pipeline.note = Some(
                        "Aucun compte secondaire controle de la liste autorisee n'apparait dans le resultat ; aucun brouillon de message n'a ete cree."
                            .to_string(),
                    );
                } else if !can_prepare_campaign {
                    pipeline.note = Some(policy::campaign_capacity_note());
                } else {
                    let campaign_id = Uuid::new_v4().to_string();
                    pipeline.prepared_campaign_id = Some(campaign_id.clone());
                    pipeline.note = Some(format!(
                        "Brouillon prepare pour {} compte(s) secondaire(s) controle(s). Affichez l'apercu puis demandez une confirmation explicite avant l'envoi.",
                        recipients.len()
                    ));
                    prepared_campaign = Some(TikTokDmCampaign {
                        id: campaign_id,
                        owner_id,
                        recipients,
                        message: pipeline.message.clone(),
                        status: TikTokDmCampaignStatus::Draft,
                        min_interval_minutes: default_min_interval_minutes(),
                        max_interval_minutes: default_max_interval_minutes(),
                        device_serial,
                        sender_account: None,
                        owned_accounts_confirmed: pipeline.owned_accounts_confirmed,
                        send_confirmed: false,
                        created_by: "scrape_to_dm_pipeline".to_string(),
                        idempotency_key: format!("scrape-dm:{extraction_id}"),
                        created_at: now,
                        updated_at: now,
                        confirmed_at: None,
                        claimed_at: None,
                        submitted_at: None,
                        connector_id: None,
                        claim_token: None,
                        tikmatrix_task_count: 0,
                        error: None,
                    });
                }
            }
        } else {
            extraction.status = TikTokFollowerExtractionStatus::Failed;
            extraction.error = Some(
                sanitize_detail(request.detail)
                    .unwrap_or_else(|| "Resultat de collecte TikMatrix introuvable".to_string()),
            );
        }
        let result = extraction.clone();
        if let Some(campaign) = prepared_campaign {
            next.campaigns.push(campaign);
        }
        self.persist_and_replace(&mut current, next)?;
        Ok(result)
    }

    fn lock_store(&self) -> Result<std::sync::MutexGuard<'_, TikTokDmStore>, TikTokDmError> {
        self.inner.store.lock().map_err(|_| {
            TikTokDmError::Internal("Etat des campagnes TikTok verrouille".to_string())
        })
    }

    fn persist_and_replace(
        &self,
        current: &mut std::sync::MutexGuard<'_, TikTokDmStore>,
        next: TikTokDmStore,
    ) -> Result<(), TikTokDmError> {
        persist_store(&self.inner.storage_path, &next)?;
        **current = next;
        Ok(())
    }
}

fn default_min_interval_minutes() -> u8 {
    1
}

fn default_max_interval_minutes() -> u8 {
    2
}

fn default_follower_max_count() -> u16 {
    policy::MAX_TIKTOK_FOLLOWER_RESULTS as u16
}

fn validate_sender_account(
    account: TikTokSenderAccount,
) -> Result<TikTokSenderAccount, TikTokDmError> {
    Ok(TikTokSenderAccount {
        username: policy::normalize_tiktok_username(&account.username)?,
        device_serial: policy::validate_device_serial(&account.device_serial)?,
        logged_in: account.logged_in,
        enabled: account.enabled,
        package_name: account.package_name.and_then(|value| {
            let value = value.trim();
            (!value.is_empty()
                && value.chars().count() <= 160
                && !value.chars().any(char::is_control))
            .then(|| value.to_string())
        }),
    })
}

fn validate_android_device(
    device: TikTokAndroidDevice,
) -> Result<TikTokAndroidDevice, TikTokDmError> {
    let metadata = |value: Option<String>| {
        value.and_then(|value| {
            let value = value.trim();
            (!value.is_empty()
                && value.chars().count() <= 160
                && !value.chars().any(char::is_control))
            .then(|| value.to_string())
        })
    };
    Ok(TikTokAndroidDevice {
        serial: policy::validate_device_serial(&device.serial)?,
        state: device.state,
        transport: device.transport,
        model: metadata(device.model),
        product: metadata(device.product),
        tikmatrix_managed: device.tikmatrix_managed,
    })
}

fn ensure_sender_account_ready(
    account: &TikTokSenderAccountView,
    accounts: &[TikTokSenderAccountView],
) -> Result<(), TikTokDmError> {
    if !account.connected {
        return Err(TikTokDmError::Conflict(
            "L'appareil du compte emetteur TikTok est hors ligne".to_string(),
        ));
    }
    if !account.logged_in {
        return Err(TikTokDmError::Conflict(format!(
            "Le compte emetteur {} n'est pas marque comme connecte dans TikMatrix",
            account.username
        )));
    }
    if !account.enabled {
        return Err(TikTokDmError::Conflict(format!(
            "Le compte emetteur {} est desactive dans TikMatrix",
            account.username
        )));
    }
    let active_on_device = accounts
        .iter()
        .filter(|candidate| {
            candidate.device_serial == account.device_serial
                && candidate.logged_in
                && candidate.enabled
                && candidate.connected
        })
        .count();
    if active_on_device > 1 {
        return Err(TikTokDmError::Conflict(
            "Plusieurs comptes TikTok actifs partagent cet appareil. Desactivez les autres comptes dans TikMatrix ou utilisez un appareil dedie afin de garantir l'identite de l'emetteur."
                .to_string(),
        ));
    }
    Ok(())
}

fn normalize_creator(value: &str) -> String {
    match value.trim() {
        "autonomous_agent" => "autonomous_agent".to_string(),
        _ => "human_chat".to_string(),
    }
}

fn sanitize_detail(value: Option<String>) -> Option<String> {
    let value = value?.trim().replace(['\r', '\n'], " ");
    (!value.is_empty()).then(|| value.chars().take(500).collect())
}

fn sanitize_output_file(value: Option<String>) -> Option<String> {
    let value = value?.trim().replace(['\r', '\n'], "");
    (!value.is_empty()).then(|| value.chars().take(260).collect())
}

fn load_store(path: &Path) -> Result<TikTokDmStore, TikTokDmError> {
    if !path.is_file() {
        return Ok(TikTokDmStore::default());
    }
    let bytes = fs::read(path).map_err(|error| {
        TikTokDmError::Internal(format!("Lecture des campagnes TikTok impossible : {error}"))
    })?;
    let store = serde_json::from_slice::<TikTokDmStore>(&bytes).map_err(|error| {
        TikTokDmError::Internal(format!("Campagnes TikTok illisibles : {error}"))
    })?;
    if store.version != STORE_VERSION {
        return Err(TikTokDmError::Internal(
            "Version du stockage TikTok non prise en charge".to_string(),
        ));
    }
    Ok(store)
}

fn persist_store(path: &Path, store: &TikTokDmStore) -> Result<(), TikTokDmError> {
    let bytes = serde_json::to_vec_pretty(store)
        .map_err(|error| TikTokDmError::Internal(error.to_string()))?;
    fs_util::atomic_write(path, bytes).map_err(|error| {
        TikTokDmError::Internal(format!(
            "Ecriture des campagnes TikTok impossible : {error}"
        ))
    })
}

#[cfg(feature = "desktop")]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct LocalReceiptStore {
    version: u32,
    #[serde(default)]
    receipts: HashMap<String, LocalReceipt>,
    #[serde(default)]
    follower_extractions: HashMap<String, LocalFollowerExtractionReceipt>,
    #[serde(default)]
    sender_setup_actions: HashMap<String, LocalSenderSetupReceipt>,
}

#[cfg(feature = "desktop")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalReceipt {
    task_count: u64,
    detail: String,
    submitted_at: i64,
}

#[cfg(feature = "desktop")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalFollowerExtractionReceipt {
    target_username: String,
    max_count: u16,
    task_count: u64,
    detail: String,
    submitted_at: i64,
    #[serde(default)]
    result_reported: bool,
}

#[cfg(feature = "desktop")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalSenderSetupReceipt {
    detail: String,
    submitted_at: i64,
}

#[cfg(feature = "desktop")]
#[derive(Debug, Clone, Default)]
struct TikMatrixSnapshot {
    agent_healthy: bool,
    device_serials: Vec<String>,
    devices: Vec<TikTokAndroidDevice>,
    account_count: usize,
    accounts: Vec<TikTokSenderAccount>,
    error: Option<String>,
}

#[cfg(feature = "desktop")]
#[derive(Debug, Clone, Default)]
struct LocalAndroidSnapshot {
    devices: Vec<TikTokAndroidDevice>,
    scrcpy_available: bool,
    ws_scrcpy_available: bool,
    ws_scrcpy_online: bool,
    error: Option<String>,
}

#[cfg(feature = "desktop")]
pub async fn run_tiktok_connector() {
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
    let connector_id = format!(
        "windows-{}",
        std::env::var("COMPUTERNAME")
            .unwrap_or_else(|_| "desktop".to_string())
            .chars()
            .filter(|character| character.is_ascii_alphanumeric() || "._-".contains(*character))
            .take(80)
            .collect::<String>()
    );
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(15))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            eprintln!("Connecteur TikMatrix indisponible : {error}");
            return;
        }
    };
    let receipts_path = match crate::settings::runtime_data_path("tiktok-connector-receipts.json") {
        Ok(path) => path,
        Err(error) => {
            eprintln!("Stockage du connecteur TikMatrix indisponible : {error}");
            return;
        }
    };
    let mut current_campaign_id: Option<String> = None;

    loop {
        let mut submitted_work = false;
        let mut snapshot = inspect_tikmatrix(&client).await;
        let mut local_android = tokio::task::spawn_blocking(inspect_local_android)
            .await
            .unwrap_or_else(|error| LocalAndroidSnapshot {
                error: Some(format!("Inventaire ADB interrompu : {error}")),
                ..LocalAndroidSnapshot::default()
            });
        local_android.ws_scrcpy_online = probe_ws_scrcpy().await;
        snapshot.devices = merge_android_devices(snapshot.devices, local_android.devices.clone());
        snapshot.device_serials = snapshot
            .devices
            .iter()
            .filter(|device| device.state == TikTokAndroidDeviceState::Device)
            .map(|device| device.serial.clone())
            .collect();
        let heartbeat = TikTokConnectorHeartbeatRequest {
            connector_id: connector_id.clone(),
            agent_healthy: snapshot.agent_healthy,
            device_serials: snapshot.device_serials.clone(),
            devices: snapshot.devices.clone(),
            scrcpy_available: local_android.scrcpy_available,
            ws_scrcpy_available: local_android.ws_scrcpy_available,
            ws_scrcpy_online: local_android.ws_scrcpy_online,
            adb_error: local_android.error.clone(),
            account_count: snapshot.account_count,
            accounts: snapshot.accounts.clone(),
            current_campaign_id: current_campaign_id.clone(),
            error: snapshot.error.clone(),
        };
        let _ = post_server_json::<Value>(
            &client,
            &base_url,
            &token,
            "/api/tiktok/connector/heartbeat",
            &heartbeat,
        )
        .await;

        let setup_claim = post_server_json::<Value>(
            &client,
            &base_url,
            &token,
            "/api/tiktok/connector/sender-setup/claim",
            &TikTokConnectorClaimRequest {
                connector_id: connector_id.clone(),
            },
        )
        .await;
        if let Ok(value) = setup_claim {
            if let Some(job_value) = value.get("job").filter(|value| !value.is_null()) {
                match serde_json::from_value::<TikTokSenderSetupJob>(job_value.clone()) {
                    Ok(job) => {
                        if job.action == TikTokSenderSetupActionKind::StartWsScrcpy {
                            // La premiere installation de ws-scrcpy-web telecharge
                            // environ 80 Mo : on traite ce job en arriere-plan pour
                            // que les heartbeats continuent et que le connecteur ne
                            // soit pas marque hors ligne pendant le telechargement.
                            let client = client.clone();
                            let base_url = base_url.clone();
                            let token = token.clone();
                            let receipts_path = receipts_path.clone();
                            let connector_id = connector_id.clone();
                            tokio::task::spawn(async move {
                                let report = process_tiktok_sender_setup(
                                    &client,
                                    &receipts_path,
                                    &connector_id,
                                    &job,
                                )
                                .await;
                                let _ = post_server_json::<Value>(
                                    &client,
                                    &base_url,
                                    &token,
                                    "/api/tiktok/connector/sender-setup/report",
                                    &report,
                                )
                                .await;
                            });
                        } else {
                            let report = process_tiktok_sender_setup(
                                &client,
                                &receipts_path,
                                &connector_id,
                                &job,
                            )
                            .await;
                            let _ = post_server_json::<Value>(
                                &client,
                                &base_url,
                                &token,
                                "/api/tiktok/connector/sender-setup/report",
                                &report,
                            )
                            .await;
                        }
                        submitted_work = true;
                    }
                    Err(error) => {
                        eprintln!("Action de connexion TikTok invalide recue du VPS : {error}");
                    }
                }
            }
        }

        let claim = if submitted_work {
            Ok(json!({ "job": null }))
        } else {
            post_server_json::<Value>(
                &client,
                &base_url,
                &token,
                "/api/tiktok/connector/jobs/claim",
                &TikTokConnectorClaimRequest {
                    connector_id: connector_id.clone(),
                },
            )
            .await
        };
        if let Ok(value) = claim {
            if let Some(job_value) = value.get("job").filter(|value| !value.is_null()) {
                match serde_json::from_value::<TikTokConnectorJob>(job_value.clone()) {
                    Ok(job) => {
                        current_campaign_id = Some(job.campaign_id.clone());
                        let _ = post_server_json::<Value>(
                            &client,
                            &base_url,
                            &token,
                            "/api/tiktok/connector/heartbeat",
                            &TikTokConnectorHeartbeatRequest {
                                connector_id: connector_id.clone(),
                                agent_healthy: snapshot.agent_healthy,
                                device_serials: snapshot.device_serials.clone(),
                                devices: snapshot.devices.clone(),
                                scrcpy_available: local_android.scrcpy_available,
                                ws_scrcpy_available: local_android.ws_scrcpy_available,
                                ws_scrcpy_online: local_android.ws_scrcpy_online,
                                adb_error: local_android.error.clone(),
                                account_count: snapshot.account_count,
                                accounts: snapshot.accounts.clone(),
                                current_campaign_id: current_campaign_id.clone(),
                                error: snapshot.error.clone(),
                            },
                        )
                        .await;
                        let report =
                            process_tiktok_job(&client, &receipts_path, &connector_id, &job).await;
                        let _ = post_server_json::<Value>(
                            &client,
                            &base_url,
                            &token,
                            "/api/tiktok/connector/jobs/report",
                            &report,
                        )
                        .await;
                        current_campaign_id = None;
                        submitted_work = true;
                    }
                    Err(error) => {
                        eprintln!("Travail TikTok invalide recu du VPS : {error}");
                    }
                }
            }
        }
        if !submitted_work {
            let claim = post_server_json::<Value>(
                &client,
                &base_url,
                &token,
                "/api/tiktok/connector/follower-extractions/claim",
                &TikTokConnectorClaimRequest {
                    connector_id: connector_id.clone(),
                },
            )
            .await;
            if let Ok(value) = claim {
                if let Some(job_value) = value.get("job").filter(|value| !value.is_null()) {
                    match serde_json::from_value::<TikTokFollowerConnectorJob>(job_value.clone()) {
                        Ok(job) => {
                            current_campaign_id = Some(job.extraction_id.clone());
                            let _ = post_server_json::<Value>(
                                &client,
                                &base_url,
                                &token,
                                "/api/tiktok/connector/heartbeat",
                                &TikTokConnectorHeartbeatRequest {
                                    connector_id: connector_id.clone(),
                                    agent_healthy: snapshot.agent_healthy,
                                    device_serials: snapshot.device_serials.clone(),
                                    devices: snapshot.devices.clone(),
                                    scrcpy_available: local_android.scrcpy_available,
                                    ws_scrcpy_available: local_android.ws_scrcpy_available,
                                    ws_scrcpy_online: local_android.ws_scrcpy_online,
                                    adb_error: local_android.error.clone(),
                                    account_count: snapshot.account_count,
                                    accounts: snapshot.accounts.clone(),
                                    current_campaign_id: current_campaign_id.clone(),
                                    error: snapshot.error.clone(),
                                },
                            )
                            .await;
                            let report = process_tiktok_follower_job(
                                &client,
                                &receipts_path,
                                &connector_id,
                                &job,
                            )
                            .await;
                            let _ = post_server_json::<Value>(
                                &client,
                                &base_url,
                                &token,
                                "/api/tiktok/connector/follower-extractions/report",
                                &report,
                            )
                            .await;
                            current_campaign_id = None;
                        }
                        Err(error) => {
                            eprintln!("Collecte TikTok invalide recue du VPS : {error}");
                        }
                    }
                }
            }
        }
        sync_follower_results(&client, &base_url, &token, &receipts_path, &connector_id).await;
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

#[cfg(feature = "desktop")]
async fn post_server_json<T: serde::de::DeserializeOwned>(
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
            "VPS {} : {}",
            status.as_u16(),
            String::from_utf8_lossy(&bytes)
                .chars()
                .take(300)
                .collect::<String>()
        ));
    }
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

#[cfg(feature = "desktop")]
async fn process_tiktok_sender_setup(
    client: &reqwest::Client,
    receipts_path: &Path,
    connector_id: &str,
    job: &TikTokSenderSetupJob,
) -> TikTokSenderSetupReportRequest {
    if let Some(receipt) = load_local_receipts(receipts_path)
        .sender_setup_actions
        .get(&job.action_id)
        .cloned()
    {
        let (success, detail) = if receipt.submitted_at > 0 {
            (
                true,
                format!(
                    "{} (recu local rejoue apres une coupure de connexion)",
                    receipt.detail
                ),
            )
        } else {
            (
                false,
                "Etat de l'action de connexion TikTok incertain apres une interruption ; elle n'a pas ete relancee automatiquement."
                    .to_string(),
            )
        };
        return TikTokSenderSetupReportRequest {
            connector_id: connector_id.to_string(),
            action_id: job.action_id.clone(),
            claim_token: job.claim_token.clone(),
            success,
            detail: Some(detail),
        };
    }

    let mut receipts = load_local_receipts(receipts_path);
    receipts.version = 1;
    receipts.sender_setup_actions.insert(
        job.action_id.clone(),
        LocalSenderSetupReceipt {
            detail: "Action de connexion TikTok demarree".to_string(),
            submitted_at: 0,
        },
    );
    if let Err(error) = persist_local_receipts(receipts_path, &receipts) {
        return TikTokSenderSetupReportRequest {
            connector_id: connector_id.to_string(),
            action_id: job.action_id.clone(),
            claim_token: job.claim_token.clone(),
            success: false,
            detail: Some(format!(
                "Impossible d'enregistrer la protection anti-doublon de la connexion : {error}"
            )),
        };
    }

    match submit_tiktok_sender_setup(client, job).await {
        Ok(detail) => {
            if let Some(receipt) = receipts.sender_setup_actions.get_mut(&job.action_id) {
                receipt.detail = detail.clone();
                receipt.submitted_at = metrics::now_ts();
            }
            let persisted = persist_local_receipts(receipts_path, &receipts);
            TikTokSenderSetupReportRequest {
                connector_id: connector_id.to_string(),
                action_id: job.action_id.clone(),
                claim_token: job.claim_token.clone(),
                success: persisted.is_ok(),
                detail: Some(match persisted {
                    Ok(()) => detail,
                    Err(error) => format!(
                        "TikMatrix a accepte l'action, mais son recu local n'a pas pu etre enregistre : {error}"
                    ),
                }),
            }
        }
        Err(error) => {
            receipts.sender_setup_actions.remove(&job.action_id);
            let detail = match persist_local_receipts(receipts_path, &receipts) {
                Ok(()) => error,
                Err(cleanup_error) => {
                    format!("{error} Le recu local n'a pas pu etre nettoye : {cleanup_error}")
                }
            };
            TikTokSenderSetupReportRequest {
                connector_id: connector_id.to_string(),
                action_id: job.action_id.clone(),
                claim_token: job.claim_token.clone(),
                success: false,
                detail: Some(detail),
            }
        }
    }
}

#[cfg(feature = "desktop")]
async fn submit_tiktok_sender_setup(
    client: &reqwest::Client,
    job: &TikTokSenderSetupJob,
) -> Result<String, String> {
    if job.action == TikTokSenderSetupActionKind::StartWsScrcpy {
        let url = ensure_ws_scrcpy_started().await?;
        return Ok(format!("ws-scrcpy-web est pret sur {url}."));
    }
    if job.action == TikTokSenderSetupActionKind::OpenScrcpy {
        let local = tokio::task::spawn_blocking(inspect_local_android)
            .await
            .map_err(|error| format!("Inventaire ADB interrompu : {error}"))?;
        let device = local
            .devices
            .iter()
            .find(|device| device.serial == job.device_serial)
            .ok_or_else(|| format!("L'appareil ADB {} n'est plus connecte", job.device_serial))?;
        if device.state != TikTokAndroidDeviceState::Device {
            return Err(format!(
                "L'appareil ADB {} n'est pas autorise ou est hors ligne",
                job.device_serial
            ));
        }
        let serial = job.device_serial.clone();
        tokio::task::spawn_blocking(move || launch_scrcpy(&serial))
            .await
            .map_err(|error| format!("Lancement de scrcpy interrompu : {error}"))??;
        return Ok(format!(
            "scrcpy est ouvert sur l'appareil {}.",
            job.device_serial
        ));
    }

    let snapshot = inspect_tikmatrix(client).await;
    if !snapshot.agent_healthy {
        return Err(snapshot
            .error
            .unwrap_or_else(|| "L'agent TikMatrix ne repond pas".to_string()));
    }
    let serial = select_tikmatrix_serial(&snapshot, Some(&job.device_serial))?;
    let port = tikmatrix_port()?;
    let base = format!("http://127.0.0.1:{port}");
    match job.action {
        TikTokSenderSetupActionKind::OpenLogin => {
            post_tikmatrix_json(
                client,
                &base,
                "/api/open_tiktok",
                &json!({
                    "serials": [serial]
                }),
            )
            .await?;
            Ok(format!(
                "TikTok est ouvert sur l'appareil {}. Saisissez les identifiants et resolvez le captcha ou la verification directement dans cette fenetre.",
                job.device_serial
            ))
        }
        TikTokSenderSetupActionKind::MatchAccounts => {
            let running = get_tikmatrix_json(client, &base, "/api/running_task").await?;
            if !data_array(&running).is_empty() {
                return Err(
                    "Une tache TikMatrix est deja active ; attendez sa fin avant Match Accounts"
                        .to_string(),
                );
            }
            post_tikmatrix_json(
                client,
                &base,
                "/api/task/run_now_by_account",
                &json!({
                    "script_name": "match_accounts",
                    "serials": [serial],
                    "script_args": "{\"enable_multi_account\":false,\"rotate_proxy\":false}"
                }),
            )
            .await?;
            Ok(format!(
                "La synchronisation Match Accounts a ete lancee sur l'appareil {}. Le compte apparaitra dans le chat apres sa detection par TikMatrix.",
                job.device_serial
            ))
        }
        TikTokSenderSetupActionKind::OpenScrcpy => unreachable!("traite avant TikMatrix"),
        TikTokSenderSetupActionKind::StartWsScrcpy => {
            unreachable!("traite avant TikMatrix")
        }
    }
}

#[cfg(feature = "desktop")]
async fn inspect_tikmatrix(client: &reqwest::Client) -> TikMatrixSnapshot {
    let port = match tikmatrix_port() {
        Ok(port) => port,
        Err(error) => {
            return TikMatrixSnapshot {
                error: Some(error),
                ..TikMatrixSnapshot::default()
            }
        }
    };
    let base = format!("http://127.0.0.1:{port}");
    let devices = match get_tikmatrix_json(client, &base, "/api/device").await {
        Ok(value) => value,
        Err(error) => {
            return TikMatrixSnapshot {
                error: Some(error),
                ..TikMatrixSnapshot::default()
            }
        }
    };
    let accounts = match get_tikmatrix_json(client, &base, "/api/account").await {
        Ok(value) => value,
        Err(error) => {
            return TikMatrixSnapshot {
                agent_healthy: true,
                error: Some(error),
                ..TikMatrixSnapshot::default()
            }
        }
    };
    let android_devices = data_array(&devices)
        .iter()
        .filter_map(tikmatrix_android_device)
        .collect::<Vec<_>>();
    let device_serials = android_devices
        .iter()
        .filter(|device| device.state == TikTokAndroidDeviceState::Device)
        .map(|device| device.serial.clone())
        .collect::<Vec<_>>();
    let sender_accounts = data_array(&accounts)
        .iter()
        .filter_map(tikmatrix_sender_account)
        .collect::<Vec<_>>();
    TikMatrixSnapshot {
        agent_healthy: true,
        device_serials,
        devices: android_devices,
        account_count: data_array(&accounts).len(),
        accounts: sender_accounts,
        error: None,
    }
}

#[cfg(feature = "desktop")]
fn tikmatrix_android_device(value: &Value) -> Option<TikTokAndroidDevice> {
    let serial = value
        .get("serial")
        .or_else(|| value.get("real_serial"))?
        .as_str()?
        .trim();
    if serial.is_empty() {
        return None;
    }
    let state = android_device_state(
        value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown"),
    );
    Some(TikTokAndroidDevice {
        serial: serial.to_string(),
        state,
        transport: android_device_transport(serial),
        model: value
            .get("model")
            .or_else(|| value.get("mode"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        product: value
            .get("product")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        tikmatrix_managed: true,
    })
}

#[cfg(feature = "desktop")]
fn inspect_local_android() -> LocalAndroidSnapshot {
    let scrcpy_available = resolve_scrcpy_path().is_some();
    // Le connecteur est considere comme capable de fournir ws-scrcpy-web des
    // qu'un lanceur est configure ou que l'installation geree est en place
    // (elle est provisionnee automatiquement au premier demarrage si besoin).
    let ws_scrcpy_available = resolve_ws_scrcpy_path().is_some()
        || managed_ws_scrcpy_root()
            .is_ok_and(|root| managed_ws_scrcpy_is_valid(&root));
    let candidates = adb_candidates();
    if candidates.is_empty() {
        return LocalAndroidSnapshot {
            scrcpy_available,
            ws_scrcpy_available,
            error: Some(
                "ADB est introuvable. Installez Android Platform Tools ou configurez CST_ADB_PATH."
                    .to_string(),
            ),
            ..LocalAndroidSnapshot::default()
        };
    }
    let mut last_error = None;
    for adb in candidates {
        let output = quiet_command(&adb).args(["devices", "-l"]).output();
        match output {
            Ok(output) if output.status.success() => {
                return LocalAndroidSnapshot {
                    devices: parse_adb_devices(&String::from_utf8_lossy(&output.stdout)),
                    scrcpy_available,
                    ws_scrcpy_available,
                    ws_scrcpy_online: false,
                    error: None,
                };
            }
            Ok(output) => {
                let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
                last_error = Some(if detail.is_empty() {
                    format!("ADB a quitte avec le statut {}", output.status)
                } else {
                    format!("ADB : {detail}")
                });
            }
            Err(error) => last_error = Some(format!("ADB impossible : {error}")),
        }
    }
    LocalAndroidSnapshot {
        scrcpy_available,
        ws_scrcpy_available,
        error: last_error,
        ..LocalAndroidSnapshot::default()
    }
}

#[cfg(feature = "desktop")]
fn parse_adb_devices(output: &str) -> Vec<TikTokAndroidDevice> {
    let mut devices = Vec::new();
    for line in output
        .lines()
        .skip_while(|line| !line.starts_with("List of devices"))
    {
        let line = line.trim();
        if line.is_empty() || line.starts_with("List of devices") || line.starts_with('*') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let Some(serial) = fields
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let state = android_device_state(fields.next().unwrap_or("unknown"));
        let mut model = None;
        let mut product = None;
        for field in fields {
            if let Some(value) = field.strip_prefix("model:") {
                model = Some(value.replace('_', " "));
            } else if let Some(value) = field.strip_prefix("product:") {
                product = Some(value.replace('_', " "));
            }
        }
        devices.push(TikTokAndroidDevice {
            serial: serial.to_string(),
            state,
            transport: android_device_transport(serial),
            model,
            product,
            tikmatrix_managed: false,
        });
    }
    devices.sort_by(|left, right| left.serial.cmp(&right.serial));
    devices.dedup_by(|left, right| left.serial == right.serial);
    devices
}

#[cfg(feature = "desktop")]
fn android_device_state(value: &str) -> TikTokAndroidDeviceState {
    match value.trim().to_ascii_lowercase().as_str() {
        "device" => TikTokAndroidDeviceState::Device,
        "unauthorized" => TikTokAndroidDeviceState::Unauthorized,
        "offline" => TikTokAndroidDeviceState::Offline,
        _ => TikTokAndroidDeviceState::Unknown,
    }
}

#[cfg(feature = "desktop")]
fn android_device_transport(serial: &str) -> TikTokAndroidDeviceTransport {
    let serial = serial.trim().to_ascii_lowercase();
    if serial.starts_with("emulator-") {
        TikTokAndroidDeviceTransport::Emulator
    } else if serial.contains(':') {
        TikTokAndroidDeviceTransport::Network
    } else if !serial.is_empty() {
        TikTokAndroidDeviceTransport::Usb
    } else {
        TikTokAndroidDeviceTransport::Unknown
    }
}

#[cfg(feature = "desktop")]
fn merge_android_devices(
    tikmatrix: Vec<TikTokAndroidDevice>,
    adb: Vec<TikTokAndroidDevice>,
) -> Vec<TikTokAndroidDevice> {
    let mut by_serial = tikmatrix
        .into_iter()
        .map(|device| (device.serial.clone(), device))
        .collect::<HashMap<_, _>>();
    for mut device in adb {
        if let Some(existing) = by_serial.get(&device.serial) {
            device.tikmatrix_managed |= existing.tikmatrix_managed;
            if device.model.is_none() {
                device.model = existing.model.clone();
            }
            if device.product.is_none() {
                device.product = existing.product.clone();
            }
        }
        by_serial.insert(device.serial.clone(), device);
    }
    let mut devices = by_serial.into_values().collect::<Vec<_>>();
    devices.sort_by(|left, right| left.serial.cmp(&right.serial));
    devices
}

#[cfg(feature = "desktop")]
fn adb_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    push_configured_executable(&mut candidates, "CST_ADB_PATH");
    if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
        candidates.push(
            appdata
                .join("com.tikmatrix")
                .join("platform-tools")
                .join(windows_executable("adb")),
        );
    }
    for variable in ["ANDROID_SDK_ROOT", "ANDROID_HOME"] {
        if let Some(root) = std::env::var_os(variable).map(PathBuf::from) {
            candidates.push(root.join("platform-tools").join(windows_executable("adb")));
        }
    }
    if let Some(localappdata) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        candidates.push(
            localappdata
                .join("Android")
                .join("Sdk")
                .join("platform-tools")
                .join(windows_executable("adb")),
        );
    }
    if let Some(path) = executable_on_path(windows_executable("adb")) {
        candidates.push(path);
    }
    candidates.retain(|path| path.is_file());
    candidates.sort();
    candidates.dedup();
    candidates
}

#[cfg(feature = "desktop")]
fn resolve_scrcpy_path() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    push_configured_executable(&mut candidates, "CST_SCRCPY_PATH");
    if let Some(path) = executable_on_path(windows_executable("scrcpy")) {
        candidates.push(path);
    }
    if let Some(profile) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
        candidates.push(
            profile
                .join("scoop")
                .join("apps")
                .join("scrcpy")
                .join("current")
                .join(windows_executable("scrcpy")),
        );
    }
    if let Some(programdata) = std::env::var_os("PROGRAMDATA").map(PathBuf::from) {
        candidates.push(
            programdata
                .join("chocolatey")
                .join("bin")
                .join(windows_executable("scrcpy")),
        );
    }
    if let Some(localappdata) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        candidates.push(
            localappdata
                .join("scrcpy")
                .join(windows_executable("scrcpy")),
        );
        let winget = localappdata
            .join("Microsoft")
            .join("WinGet")
            .join("Packages");
        if let Some(path) = find_named_file(&winget, windows_executable("scrcpy"), 3) {
            candidates.push(path);
        }
    }
    candidates.into_iter().find(|path| path.is_file())
}

#[cfg(feature = "desktop")]
const WS_SCRCPY_EMBED_URL: &str = "http://127.0.0.1:8000/embed.html";

// Version pinnee de ws-scrcpy-web provisionnee automatiquement par le
// connecteur Windows quand aucun lanceur n'est configure ni installe. Le ZIP
// portable embarque son propre runtime Node (current/seed/node) et son propre
// start.cmd relocatable ; on applique seulement le correctif de port 8000
// (cette beta ignore WS_SCRCPY_WEB_PORT et choisit parfois un port ephemere).
#[cfg(feature = "desktop")]
const WS_SCRCPY_RELEASE_TAG: &str = "v0.1.30-beta.82";
#[cfg(feature = "desktop")]
const WS_SCRCPY_ZIP_FILENAME: &str = "WsScrcpyWeb-beta-Portable.zip";
#[cfg(feature = "desktop")]
const WS_SCRCPY_DOWNLOAD_BASE: &str =
    "https://github.com/bilbospocketses/ws-scrcpy-web/releases/download";
#[cfg(feature = "desktop")]
const WS_SCRCPY_PORT_FIX_FROM: &str = "i.listen(r,()=>{Qr.printListeningMsg(n,r,ts)})";
#[cfg(feature = "desktop")]
const WS_SCRCPY_PORT_FIX_TO: &str = "i.listen(8000,()=>{Qr.printListeningMsg(n,8000,ts)})";

#[cfg(feature = "desktop")]
fn push_ws_scrcpy_directory_candidates(candidates: &mut Vec<PathBuf>, root: &Path) {
    candidates.push(root.join("ws-scrcpy-web.exe"));
    candidates.push(root.join("ws-scrcpy-web-launcher.exe"));
    candidates.push(root.join("current").join("ws-scrcpy-web-launcher.exe"));
    candidates.push(root.join("start.cmd"));
}

#[cfg(feature = "desktop")]
fn supported_ws_scrcpy_launcher(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let file_name = path
        .file_name()
        .map(|value| value.to_string_lossy())
        .unwrap_or_default();
    file_name.eq_ignore_ascii_case("start.cmd")
        || path
            .extension()
            .is_some_and(|value| value.to_string_lossy().eq_ignore_ascii_case("exe"))
}

#[cfg(feature = "desktop")]
fn resolve_ws_scrcpy_path() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(configured) = std::env::var_os("CST_WS_SCRCPY_PATH").map(PathBuf::from) {
        if configured.is_dir() {
            push_ws_scrcpy_directory_candidates(&mut candidates, &configured);
        } else {
            candidates.push(configured);
        }
    }
    if let Some(program_files) = std::env::var_os("ProgramFiles").map(PathBuf::from) {
        let root = program_files.join("WsScrcpyWeb");
        candidates.push(root.join("ws-scrcpy-web.exe"));
        candidates.push(root.join("current").join("ws-scrcpy-web-launcher.exe"));
        candidates.push(root.join("start.cmd"));
    }
    candidates
        .into_iter()
        .find(|path| supported_ws_scrcpy_launcher(path))
}

#[cfg(feature = "desktop")]
async fn probe_ws_scrcpy() -> bool {
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_millis(300))
        .timeout(Duration::from_millis(800))
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(client) => client,
        Err(_) => return false,
    };
    let response = match client.get(WS_SCRCPY_EMBED_URL).send().await {
        Ok(response) if response.status().is_success() => response,
        _ => return false,
    };
    let body = match response.text().await {
        Ok(body) => body,
        Err(_) => return false,
    };
    body.contains("ws-scrcpy-web stream") && body.contains("data-embed-entry")
}

#[cfg(feature = "desktop")]
fn managed_ws_scrcpy_root() -> Result<PathBuf, String> {
    crate::settings::runtime_data_path("ws-scrcpy-web")
}

#[cfg(feature = "desktop")]
fn managed_ws_scrcpy_is_valid(root: &Path) -> bool {
    let entry = root.join("dist").join("index.js");
    let node = root.join("seed").join("node").join("node.exe");
    let start = root.join("start.cmd");
    if !entry.is_file() || !node.is_file() || !start.is_file() {
        return false;
    }
    fs::read_to_string(&entry)
        .is_ok_and(|content| content.contains(WS_SCRCPY_PORT_FIX_TO))
}

#[cfg(feature = "desktop")]
fn sha256_hex(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let bytes = fs::read(path)
        .map_err(|error| format!("Lecture du fichier telecharge impossible : {error}"))?;
    let digest = Sha256::digest(&bytes);
    Ok(digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(feature = "desktop")]
fn ws_scrcpy_sha256_from_sums(sums: &str, file_name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        let (hash, name) = line.split_once(char::is_whitespace)?;
        let name = name.trim_start_matches('*').trim();
        (name == file_name).then(|| hash.to_ascii_lowercase())
    })
}

#[cfg(feature = "desktop")]
fn apply_ws_scrcpy_port_fix(root: &Path) -> Result<(), String> {
    let entry = root.join("dist").join("index.js");
    let content = fs::read_to_string(&entry)
        .map_err(|error| format!("Lecture de {} impossible : {error}", entry.display()))?;
    if content.contains(WS_SCRCPY_PORT_FIX_TO) {
        return Ok(());
    }
    let patched = content.replace(WS_SCRCPY_PORT_FIX_FROM, WS_SCRCPY_PORT_FIX_TO);
    if patched == content {
        return Err(format!(
            "La version telechargee de ws-scrcpy-web ne contient pas le point d'ecoute attendu. Version pinnee : {WS_SCRCPY_RELEASE_TAG}."
        ));
    }
    fs::write(&entry, patched)
        .map_err(|error| format!("Ecriture de {} impossible : {error}", entry.display()))
}

/// Provisionne ws-scrcpy-web dans le dossier gere de l'application quand aucun
/// lanceur n'est configure (CST_WS_SCRCPY_PATH) ni installe (Program Files).
/// Telecharge le ZIP portable officiel, verifie son SHA-256 contre SHA256SUMS,
/// extrait, applique le correctif de port 8000 puis cree le start.cmd.
#[cfg(feature = "desktop")]
async fn provision_ws_scrcpy(root: &Path) -> Result<(), String> {
    let parent = root
        .parent()
        .ok_or_else(|| "Dossier de donnees de l'application invalide".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Creation de {} impossible : {error}", parent.display()))?;
    let zip_path = parent.join(WS_SCRCPY_ZIP_FILENAME);
    let sums_path = parent.join("SHA256SUMS");
    let staging = parent.join("ws-scrcpy-web-staging");
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| format!("Client HTTP impossible : {error}"))?;
    let zip_url = format!(
        "{WS_SCRCPY_DOWNLOAD_BASE}/{WS_SCRCPY_RELEASE_TAG}/{WS_SCRCPY_ZIP_FILENAME}"
    );
    let sums_url = format!("{WS_SCRCPY_DOWNLOAD_BASE}/{WS_SCRCPY_RELEASE_TAG}/SHA256SUMS");
    let zip_bytes = client
        .get(&zip_url)
        .send()
        .await
        .map_err(|error| format!("Telechargement de ws-scrcpy-web impossible : {error}"))?
        .error_for_status()
        .map_err(|error| format!("Telechargement de ws-scrcpy-web refuse : {error}"))?
        .bytes()
        .await
        .map_err(|error| format!("Lecture du ZIP ws-scrcpy-web impossible : {error}"))?;
    let sums_text = client
        .get(&sums_url)
        .send()
        .await
        .map_err(|error| format!("Telechargement de SHA256SUMS impossible : {error}"))?
        .error_for_status()
        .map_err(|error| format!("Telechargement de SHA256SUMS refuse : {error}"))?
        .text()
        .await
        .map_err(|error| format!("Lecture de SHA256SUMS impossible : {error}"))?;
    let expected = ws_scrcpy_sha256_from_sums(&sums_text, WS_SCRCPY_ZIP_FILENAME)
        .ok_or_else(|| {
            format!(
                "SHA256SUMS ne contient aucune entree pour {WS_SCRCPY_ZIP_FILENAME}"
            )
        })?;
    fs::write(&zip_path, &zip_bytes).map_err(|error| {
        format!("Ecriture du ZIP ws-scrcpy-web impossible : {error}")
    })?;
    let actual = sha256_hex(&zip_path)?;
    if actual != expected {
        let _ = fs::remove_file(&zip_path);
        return Err(format!(
            "Echec de verification SHA-256 de ws-scrcpy-web : attendu {expected}, obtenu {actual}. Telechargement abandonne."
        ));
    }
    let _ = fs::write(&sums_path, &sums_text);

    if staging.exists() {
        fs::remove_dir_all(&staging)
            .map_err(|error| format!("Nettoyage de {} impossible : {error}", staging.display()))?;
    }
    fs::create_dir_all(&staging)
        .map_err(|error| format!("Creation de {} impossible : {error}", staging.display()))?;
    let extraction = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
                zip_path.display(),
                staging.display()
            ),
        ])
        .output()
        .map_err(|error| format!("Extraction de ws-scrcpy-web impossible : {error}"))?;
    if !extraction.status.success() {
        let detail = String::from_utf8_lossy(&extraction.stderr).trim().to_string();
        return Err(format!(
            "Extraction de ws-scrcpy-web impossible : {}",
            if detail.is_empty() {
                "Expand-Archive a echoue".to_string()
            } else {
                detail
            }
        ));
    }
    let extracted = staging.join("current");
    if !extracted.is_dir() {
        let _ = fs::remove_dir_all(&staging);
        return Err(
            "Le ZIP ws-scrcpy-web ne contient pas le dossier current attendu.".to_string(),
        );
    }
    if root.exists() {
        fs::remove_dir_all(root)
            .map_err(|error| format!("Remplacement de {} impossible : {error}", root.display()))?;
    }
    fs::rename(&extracted, root).map_err(|error| {
        format!("Installation de ws-scrcpy-web dans {} impossible : {error}", root.display())
    })?;
    let _ = fs::remove_dir_all(&staging);
    let _ = fs::remove_file(&zip_path);
    let _ = fs::remove_file(&sums_path);
    apply_ws_scrcpy_port_fix(root)?;
    Ok(())
}

/// Choisit le lanceur ws-scrcpy-web : configuration explicite ou installation
/// existante d'abord, sinon provisionnement automatique dans le dossier gere.
#[cfg(feature = "desktop")]
async fn resolve_or_provision_ws_scrcpy_launcher() -> Result<PathBuf, String> {
    if let Some(launcher) = resolve_ws_scrcpy_path() {
        return Ok(launcher);
    }
    let root = managed_ws_scrcpy_root()?;
    if managed_ws_scrcpy_is_valid(&root) {
        return Ok(root.join("start.cmd"));
    }
    provision_ws_scrcpy(&root).await?;
    Ok(root.join("start.cmd"))
}

#[cfg(feature = "desktop")]
fn launch_ws_scrcpy_at(launcher: &Path) -> Result<(), String> {
    #[cfg(not(target_os = "windows"))]
    {
        Err(
            "Le lancement automatique de ws-scrcpy-web est disponible uniquement sur Windows."
                .to_string(),
        )
    }

    #[cfg(target_os = "windows")]
    {
        let current_dir = launcher
            .parent()
            .ok_or_else(|| "Le dossier du lanceur ws-scrcpy-web est invalide".to_string())?;
        let is_cmd = launcher
            .file_name()
            .is_some_and(|value| value.to_string_lossy().eq_ignore_ascii_case("start.cmd"));
        let mut command = if is_cmd {
            let comspec = std::env::var_os("COMSPEC")
                .map(PathBuf::from)
                .filter(|path| {
                    path.is_file()
                        && path.file_name().is_some_and(|value| {
                            value.to_string_lossy().eq_ignore_ascii_case("cmd.exe")
                        })
                })
                .ok_or_else(|| "COMSPEC ne designe pas un cmd.exe Windows valide".to_string())?;
            let launcher_text = launcher.to_string_lossy();
            if launcher_text.contains('"') {
                return Err("Le chemin du lanceur ws-scrcpy-web est invalide".to_string());
            }
            let mut command = quiet_command(&comspec);
            command
                .args(["/D", "/S", "/C"])
                .arg(format!("call \"{launcher_text}\""));
            command
        } else {
            quiet_command(&launcher)
        };
        command
            .current_dir(current_dir)
            .env("WS_SCRCPY_WEB_PORT", "8000")
            .env("WS_SCRCPY_NO_BROWSER", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("Demarrage de ws-scrcpy-web impossible : {error}"))
    }
}

#[cfg(feature = "desktop")]
async fn ensure_ws_scrcpy_started() -> Result<String, String> {
    if probe_ws_scrcpy().await {
        return Ok(WS_SCRCPY_EMBED_URL.to_string());
    }
    let launcher = resolve_or_provision_ws_scrcpy_launcher().await?;
    let launcher_clone = launcher.clone();
    tokio::task::spawn_blocking(move || launch_ws_scrcpy_at(&launcher_clone))
        .await
        .map_err(|error| format!("Lancement de ws-scrcpy-web interrompu : {error}"))??;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }
        tokio::time::sleep((deadline - now).min(Duration::from_millis(250))).await;
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        if tokio::time::timeout(remaining, probe_ws_scrcpy())
            .await
            .unwrap_or(false)
        {
            return Ok(WS_SCRCPY_EMBED_URL.to_string());
        }
    }
    Err("ws-scrcpy-web a ete lance, mais son ecran integre n'est pas devenu disponible sur 127.0.0.1:8000 dans les 15 secondes."
        .to_string())
}

#[cfg(feature = "desktop")]
fn push_configured_executable(candidates: &mut Vec<PathBuf>, variable: &str) {
    if let Some(path) = std::env::var_os(variable).map(PathBuf::from) {
        candidates.push(path);
    }
}

#[cfg(feature = "desktop")]
fn executable_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(feature = "desktop")]
fn find_named_file(root: &Path, name: &str, depth: u8) -> Option<PathBuf> {
    if depth == 0 || !root.is_dir() {
        return None;
    }
    for entry in fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if path.is_file()
            && path
                .file_name()
                .is_some_and(|value| value.to_string_lossy().eq_ignore_ascii_case(name))
        {
            return Some(path);
        }
        if path.is_dir() {
            if let Some(found) = find_named_file(&path, name, depth - 1) {
                return Some(found);
            }
        }
    }
    None
}

#[cfg(feature = "desktop")]
fn windows_executable(name: &str) -> &str {
    #[cfg(target_os = "windows")]
    {
        return match name {
            "adb" => "adb.exe",
            "scrcpy" => "scrcpy.exe",
            value => value,
        };
    }
    #[cfg(not(target_os = "windows"))]
    name
}

#[cfg(feature = "desktop")]
fn quiet_command(executable: &Path) -> Command {
    let mut command = Command::new(executable);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
}

#[cfg(feature = "desktop")]
fn launch_scrcpy(serial: &str) -> Result<(), String> {
    let scrcpy = resolve_scrcpy_path().ok_or_else(|| {
        "scrcpy est introuvable. Installez-le ou configurez CST_SCRCPY_PATH.".to_string()
    })?;
    let mut command = quiet_command(&scrcpy);
    command
        .args(["--serial", serial, "--window-title"])
        .arg(format!("TikTok - {serial}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(adb) = adb_candidates().into_iter().next() {
        command.env("ADB", adb);
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Ouverture de scrcpy impossible : {error}"))
}

#[cfg(feature = "desktop")]
async fn process_tiktok_follower_job(
    client: &reqwest::Client,
    receipts_path: &Path,
    connector_id: &str,
    job: &TikTokFollowerConnectorJob,
) -> TikTokFollowerSubmissionReportRequest {
    if let Some(receipt) = load_local_receipts(receipts_path)
        .follower_extractions
        .get(&job.extraction_id)
        .cloned()
    {
        let (success, detail) = if receipt.submitted_at > 0 {
            (
                true,
                format!(
                    "{} (recu local rejoue apres une coupure de connexion)",
                    receipt.detail
                ),
            )
        } else {
            (
                false,
                "Etat de soumission de la collecte incertain apres une interruption. Aucun nouvel appel TikMatrix n'a ete effectue."
                    .to_string(),
            )
        };
        return TikTokFollowerSubmissionReportRequest {
            connector_id: connector_id.to_string(),
            extraction_id: job.extraction_id.clone(),
            claim_token: job.claim_token.clone(),
            success,
            tikmatrix_task_count: receipt.task_count,
            detail: Some(detail),
        };
    }

    let mut receipts = load_local_receipts(receipts_path);
    receipts.version = 1;
    receipts.follower_extractions.insert(
        job.extraction_id.clone(),
        LocalFollowerExtractionReceipt {
            target_username: job.target_username.clone(),
            max_count: job.max_count,
            task_count: 0,
            detail: "Soumission de la collecte TikMatrix demarree.".to_string(),
            submitted_at: 0,
            result_reported: false,
        },
    );
    if let Err(error) = persist_local_receipts(receipts_path, &receipts) {
        return TikTokFollowerSubmissionReportRequest {
            connector_id: connector_id.to_string(),
            extraction_id: job.extraction_id.clone(),
            claim_token: job.claim_token.clone(),
            success: false,
            tikmatrix_task_count: 0,
            detail: Some(format!(
                "Impossible d'enregistrer la protection anti-doublon de la collecte : {error}"
            )),
        };
    }

    let outcome = submit_tiktok_follower_job(client, job).await;
    match outcome {
        Ok((task_count, detail)) => {
            if let Some(receipt) = receipts.follower_extractions.get_mut(&job.extraction_id) {
                receipt.task_count = task_count;
                receipt.detail = detail.clone();
                receipt.submitted_at = metrics::now_ts();
            }
            if let Err(error) = persist_local_receipts(receipts_path, &receipts) {
                return TikTokFollowerSubmissionReportRequest {
                    connector_id: connector_id.to_string(),
                    extraction_id: job.extraction_id.clone(),
                    claim_token: job.claim_token.clone(),
                    success: false,
                    tikmatrix_task_count: 0,
                    detail: Some(format!(
                        "TikMatrix a accepte la collecte, mais son recu local n'a pas pu etre enregistre : {error}"
                    )),
                };
            }
            TikTokFollowerSubmissionReportRequest {
                connector_id: connector_id.to_string(),
                extraction_id: job.extraction_id.clone(),
                claim_token: job.claim_token.clone(),
                success: true,
                tikmatrix_task_count: task_count,
                detail: Some(detail),
            }
        }
        Err(error) => {
            receipts.follower_extractions.remove(&job.extraction_id);
            let detail = match persist_local_receipts(receipts_path, &receipts) {
                Ok(()) => error,
                Err(cleanup_error) => {
                    format!("{error} Le recu local n'a pas pu etre nettoye : {cleanup_error}")
                }
            };
            TikTokFollowerSubmissionReportRequest {
                connector_id: connector_id.to_string(),
                extraction_id: job.extraction_id.clone(),
                claim_token: job.claim_token.clone(),
                success: false,
                tikmatrix_task_count: 0,
                detail: Some(detail),
            }
        }
    }
}

#[cfg(feature = "desktop")]
async fn submit_tiktok_follower_job(
    client: &reqwest::Client,
    job: &TikTokFollowerConnectorJob,
) -> Result<(u64, String), String> {
    let snapshot = inspect_tikmatrix(client).await;
    if !snapshot.agent_healthy {
        return Err(snapshot
            .error
            .unwrap_or_else(|| "L'agent TikMatrix ne repond pas".to_string()));
    }
    if snapshot.account_count == 0 {
        return Err(
            "Aucun compte TikTok reconnu par TikMatrix ; connectez le compte puis lancez Match Accounts"
                .to_string(),
        );
    }
    let serial = select_tikmatrix_serial(&snapshot, job.device_serial.as_deref())?;
    let port = tikmatrix_port()?;
    let base = format!("http://127.0.0.1:{port}");
    let running = get_tikmatrix_json(client, &base, "/api/running_task").await?;
    if !data_array(&running).is_empty() {
        return Err(
            "Une tache TikMatrix est deja active ; attendez sa fin avant la collecte".to_string(),
        );
    }

    let data_dir = tikmatrix_data_dir()?;
    let settings_path = data_dir.join("scrape_users_settings.json");
    let mut settings = read_json_object(&settings_path);
    settings.insert("scrape_mode".to_string(), json!("followers"));
    settings.insert("target_username".to_string(), json!(job.target_username));
    settings.insert("search_keyword".to_string(), json!(""));
    settings.insert("max_scrape_count".to_string(), json!(job.max_count));
    let settings_bytes =
        serde_json::to_vec_pretty(&Value::Object(settings)).map_err(|error| error.to_string())?;
    fs_util::atomic_write(&settings_path, settings_bytes)
        .map_err(|error| format!("Reglages Scrape Users impossibles : {error}"))?;

    let response = client
        .post(format!("{base}/api/scrape_now"))
        .json(&json!({
            "serials": [serial],
            "mode": "followers",
            "target_username": job.target_username,
            "search_keyword": "",
            "max_scrape_count": job.max_count,
            "enable_multi_account": false,
            "rotate_proxy": false
        }))
        .send()
        .await
        .map_err(|error| format!("Appel Scrape Users impossible : {error}"))?;
    let status = response.status();
    let bytes = response.bytes().await.map_err(|error| error.to_string())?;
    let value = serde_json::from_slice::<Value>(&bytes).unwrap_or_else(
        |_| json!({ "raw": String::from_utf8_lossy(&bytes).chars().take(300).collect::<String>() }),
    );
    if !status.is_success()
        || value
            .get("code")
            .and_then(Value::as_i64)
            .is_some_and(|code| code != 0)
    {
        return Err(format!(
            "TikMatrix a refuse la collecte (HTTP {}) : {}",
            status.as_u16(),
            value
        ));
    }
    let task_count = value
        .get("data")
        .and_then(|data| {
            data.as_u64()
                .or_else(|| data.as_str().and_then(|value| value.parse::<u64>().ok()))
        })
        .unwrap_or(1);
    Ok((
        task_count,
        format!(
            "{task_count} tache(s) TikMatrix soumise(s) pour les followers de {}",
            job.target_username
        ),
    ))
}

#[cfg(feature = "desktop")]
async fn sync_follower_results(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    receipts_path: &Path,
    connector_id: &str,
) {
    let pending = post_server_json::<Value>(
        client,
        base_url,
        token,
        "/api/tiktok/connector/follower-extractions/pending-results",
        &TikTokConnectorClaimRequest {
            connector_id: connector_id.to_string(),
        },
    )
    .await;
    let Ok(value) = pending else {
        return;
    };
    let running = match tikmatrix_port() {
        Ok(port) => get_tikmatrix_json(
            client,
            &format!("http://127.0.0.1:{port}"),
            "/api/running_task",
        )
        .await
        .map(|value| !data_array(&value).is_empty())
        .unwrap_or(true),
        Err(_) => true,
    };
    if running {
        return;
    }
    let pending = value
        .get("extractions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for item in pending {
        let Ok(extraction) = serde_json::from_value::<TikTokFollowerPendingResult>(item) else {
            continue;
        };
        let outcome = find_follower_output_file(&extraction);
        let report = match outcome {
            Ok(Some(path)) => match read_follower_usernames(&path, extraction.max_count) {
                Ok(usernames) => TikTokFollowerResultReportRequest {
                    connector_id: connector_id.to_string(),
                    extraction_id: extraction.extraction_id.clone(),
                    success: true,
                    usernames,
                    output_file: path
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string()),
                    detail: None,
                },
                Err(error) => TikTokFollowerResultReportRequest {
                    connector_id: connector_id.to_string(),
                    extraction_id: extraction.extraction_id.clone(),
                    success: false,
                    usernames: Vec::new(),
                    output_file: None,
                    detail: Some(error),
                },
            },
            Ok(None)
                if metrics::now_ts().saturating_sub(extraction.submitted_at) > 30 * 60 =>
            {
                TikTokFollowerResultReportRequest {
                    connector_id: connector_id.to_string(),
                    extraction_id: extraction.extraction_id.clone(),
                    success: false,
                    usernames: Vec::new(),
                    output_file: None,
                    detail: Some(
                        "TikMatrix a termine sans produire de fichier de followers apres 30 minutes"
                            .to_string(),
                    ),
                }
            }
            Ok(None) => continue,
            Err(error) => TikTokFollowerResultReportRequest {
                connector_id: connector_id.to_string(),
                extraction_id: extraction.extraction_id.clone(),
                success: false,
                usernames: Vec::new(),
                output_file: None,
                detail: Some(error),
            },
        };
        if post_server_json::<Value>(
            client,
            base_url,
            token,
            "/api/tiktok/connector/follower-extractions/report-result",
            &report,
        )
        .await
        .is_ok()
        {
            let mut receipts = load_local_receipts(receipts_path);
            if let Some(receipt) = receipts
                .follower_extractions
                .get_mut(&extraction.extraction_id)
            {
                receipt.result_reported = true;
                let _ = persist_local_receipts(receipts_path, &receipts);
            }
        }
    }
}

#[cfg(feature = "desktop")]
fn find_follower_output_file(
    extraction: &TikTokFollowerPendingResult,
) -> Result<Option<PathBuf>, String> {
    let download_dir = tikmatrix_download_dir()?;
    let target = extraction
        .target_username
        .trim_start_matches('@')
        .to_ascii_lowercase();
    let earliest = extraction.submitted_at.saturating_sub(10);
    let mut candidates = Vec::new();
    let entries = fs::read_dir(&download_dir)
        .map_err(|error| format!("Dossier de resultats TikMatrix illisible : {error}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("txt") {
            continue;
        }
        let name = path
            .file_name()
            .map(|value| value.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if !name.starts_with("exported_users_")
            || !name.contains("_tiktok_")
            || !name.contains(&target)
        {
            continue;
        }
        let modified = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0);
        if modified >= earliest {
            candidates.push((modified, path));
        }
    }
    candidates.sort_by(|left, right| right.0.cmp(&left.0));
    Ok(candidates.into_iter().next().map(|(_, path)| path))
}

#[cfg(feature = "desktop")]
fn read_follower_usernames(path: &Path, max_count: u16) -> Result<Vec<String>, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("Lecture du resultat TikMatrix impossible : {error}"))?;
    let mut seen = HashSet::new();
    let mut usernames = Vec::new();
    for line in content.lines() {
        let value = line
            .trim()
            .trim_start_matches('\u{feff}')
            .split([',', ';', '\t'])
            .next()
            .unwrap_or("")
            .trim()
            .trim_matches(['"', '\'']);
        let Ok(username) = policy::normalize_tiktok_username(value) else {
            continue;
        };
        if seen.insert(username.to_ascii_lowercase()) {
            usernames.push(username);
        }
        if usernames.len() >= usize::from(max_count) {
            break;
        }
    }
    Ok(usernames)
}

#[cfg(feature = "desktop")]
async fn process_tiktok_job(
    client: &reqwest::Client,
    receipts_path: &Path,
    connector_id: &str,
    job: &TikTokConnectorJob,
) -> TikTokConnectorReportRequest {
    if let Some(receipt) = load_local_receipts(receipts_path)
        .receipts
        .get(&job.campaign_id)
        .cloned()
    {
        let (success, detail) = if receipt.submitted_at > 0 {
            (
                true,
                format!(
                    "{} (recu local rejoue apres une coupure de connexion)",
                    receipt.detail
                ),
            )
        } else {
            (
                false,
                "Etat de soumission TikMatrix incertain apres une interruption. Aucun renvoi automatique n'a ete effectue afin d'eviter un doublon. Verifiez TikMatrix avant de recreer la campagne."
                    .to_string(),
            )
        };
        return TikTokConnectorReportRequest {
            connector_id: connector_id.to_string(),
            campaign_id: job.campaign_id.clone(),
            claim_token: job.claim_token.clone(),
            success,
            tikmatrix_task_count: receipt.task_count,
            detail: Some(detail),
        };
    }

    let mut receipts = load_local_receipts(receipts_path);
    receipts.version = 1;
    receipts.receipts.insert(
        job.campaign_id.clone(),
        LocalReceipt {
            task_count: 0,
            detail: "Soumission TikMatrix demarree ; resultat final non confirme.".to_string(),
            submitted_at: 0,
        },
    );
    if let Err(error) = persist_local_receipts(receipts_path, &receipts) {
        return TikTokConnectorReportRequest {
            connector_id: connector_id.to_string(),
            campaign_id: job.campaign_id.clone(),
            claim_token: job.claim_token.clone(),
            success: false,
            tikmatrix_task_count: 0,
            detail: Some(format!(
                "Impossible d'enregistrer la protection anti-doublon avant l'envoi : {error}"
            )),
        };
    }

    let outcome = submit_tiktok_job(client, job).await;
    match outcome {
        Ok((task_count, detail)) => {
            if let Some(receipt) = receipts.receipts.get_mut(&job.campaign_id) {
                receipt.task_count = task_count;
                receipt.detail = detail.clone();
                receipt.submitted_at = metrics::now_ts();
            }
            if let Err(error) = persist_local_receipts(receipts_path, &receipts) {
                return TikTokConnectorReportRequest {
                    connector_id: connector_id.to_string(),
                    campaign_id: job.campaign_id.clone(),
                    claim_token: job.claim_token.clone(),
                    success: false,
                    tikmatrix_task_count: 0,
                    detail: Some(format!(
                        "TikMatrix a accepte la tache, mais le recu anti-doublon n'a pas pu etre enregistre : {error}"
                    )),
                };
            }
            TikTokConnectorReportRequest {
                connector_id: connector_id.to_string(),
                campaign_id: job.campaign_id.clone(),
                claim_token: job.claim_token.clone(),
                success: true,
                tikmatrix_task_count: task_count,
                detail: Some(detail),
            }
        }
        Err(error) => {
            receipts.receipts.remove(&job.campaign_id);
            let detail = match persist_local_receipts(receipts_path, &receipts) {
                Ok(()) => error,
                Err(cleanup_error) => format!(
                    "{error} La reservation anti-doublon locale n'a pas pu etre supprimee : {cleanup_error}"
                ),
            };
            TikTokConnectorReportRequest {
                connector_id: connector_id.to_string(),
                campaign_id: job.campaign_id.clone(),
                claim_token: job.claim_token.clone(),
                success: false,
                tikmatrix_task_count: 0,
                detail: Some(detail),
            }
        }
    }
}

#[cfg(feature = "desktop")]
async fn submit_tiktok_job(
    client: &reqwest::Client,
    job: &TikTokConnectorJob,
) -> Result<(u64, String), String> {
    let snapshot = inspect_tikmatrix(client).await;
    if !snapshot.agent_healthy {
        return Err(snapshot
            .error
            .unwrap_or_else(|| "L'agent TikMatrix ne repond pas".to_string()));
    }
    if snapshot.account_count == 0 {
        return Err(
            "Aucun compte TikTok reconnu par TikMatrix ; connectez le compte puis lancez Match Accounts"
                .to_string(),
        );
    }
    let serial = select_tikmatrix_sender_serial(
        &snapshot,
        job.sender_account.as_deref(),
        job.device_serial.as_deref(),
    )?;

    let port = tikmatrix_port()?;
    let base = format!("http://127.0.0.1:{port}");
    let running = get_tikmatrix_json(client, &base, "/api/running_task").await?;
    if !data_array(&running).is_empty() {
        return Err(
            "Une tache TikMatrix est deja active ; attendez sa fin avant cette campagne"
                .to_string(),
        );
    }

    let data_dir = tikmatrix_data_dir()?;
    let campaign_dir = data_dir.join("cst-dm-campaigns");
    let usernames_path = campaign_dir.join(format!("{}.txt", job.campaign_id));
    let usernames = job.recipients.join("\r\n") + "\r\n";
    fs_util::atomic_write(&usernames_path, usernames.as_bytes())
        .map_err(|error| format!("Ecriture de la liste TikTok impossible : {error}"))?;
    let portable_path = usernames_path.to_string_lossy().replace('\\', "/");

    let settings_path = data_dir.join("mass_dm_settings.json");
    let mut settings = read_json_object(&settings_path);
    settings.insert("message_contents".to_string(), json!(job.message));
    settings.insert("insert_emoji".to_string(), json!(false));
    settings.insert("target_username_path".to_string(), json!(portable_path));
    settings.insert("send_profile_card".to_string(), json!(""));
    settings.insert("open_user_method".to_string(), json!("direct"));
    settings.insert(
        "task_interval".to_string(),
        json!([job.min_interval_minutes, job.max_interval_minutes]),
    );
    let settings_bytes =
        serde_json::to_vec_pretty(&Value::Object(settings)).map_err(|error| error.to_string())?;
    fs_util::atomic_write(&settings_path, settings_bytes)
        .map_err(|error| format!("Reglages Mass DM impossibles : {error}"))?;

    let response = client
        .post(format!("{base}/api/message_now"))
        .json(&json!({
            "serials": [serial],
            "target_username_path": portable_path,
            "enable_multi_account": false,
            "rotate_proxy": false,
            "min_interval": job.min_interval_minutes,
            "max_interval": job.max_interval_minutes
        }))
        .send()
        .await
        .map_err(|error| format!("Appel TikMatrix impossible : {error}"))?;
    let status = response.status();
    let bytes = response.bytes().await.map_err(|error| error.to_string())?;
    let value = serde_json::from_slice::<Value>(&bytes).unwrap_or_else(
        |_| json!({ "raw": String::from_utf8_lossy(&bytes).chars().take(300).collect::<String>() }),
    );
    if !status.is_success()
        || value
            .get("code")
            .and_then(Value::as_i64)
            .is_some_and(|code| code != 0)
    {
        return Err(format!(
            "TikMatrix a refuse la campagne (HTTP {}) : {}",
            status.as_u16(),
            value
        ));
    }
    let task_count = value
        .get("data")
        .and_then(|data| {
            data.as_u64()
                .or_else(|| data.as_str().and_then(|value| value.parse::<u64>().ok()))
        })
        .unwrap_or(1);
    Ok((
        task_count,
        format!(
            "{task_count} tache(s) TikMatrix soumise(s) pour {} compte(s) de test",
            job.recipients.len()
        ),
    ))
}

#[cfg(feature = "desktop")]
fn tikmatrix_port() -> Result<u16, String> {
    let appdata = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| "APPDATA est indisponible".to_string())?;
    let path = appdata.join("com.tikmatrix").join("port.txt");
    let raw = fs::read_to_string(&path)
        .map_err(|_| "TikMatrix n'est pas demarre ou son agent n'est pas initialise".to_string())?;
    raw.trim()
        .parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .ok_or_else(|| "Port de l'agent TikMatrix invalide".to_string())
}

#[cfg(feature = "desktop")]
fn tikmatrix_data_dir() -> Result<PathBuf, String> {
    let appdata = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| "APPDATA est indisponible".to_string())?;
    let path = appdata.join("com.tikmatrix").join("data");
    fs::create_dir_all(&path)
        .map_err(|error| format!("Dossier de donnees TikMatrix inaccessible : {error}"))?;
    Ok(path)
}

#[cfg(feature = "desktop")]
fn tikmatrix_download_dir() -> Result<PathBuf, String> {
    let appdata = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| "APPDATA est indisponible".to_string())?;
    let path = appdata.join("com.tikmatrix").join("download");
    fs::create_dir_all(&path)
        .map_err(|error| format!("Dossier de resultats TikMatrix inaccessible : {error}"))?;
    Ok(path)
}

#[cfg(feature = "desktop")]
fn select_tikmatrix_serial(
    snapshot: &TikMatrixSnapshot,
    requested: Option<&str>,
) -> Result<String, String> {
    match requested {
        Some(serial) if snapshot.device_serials.iter().any(|value| value == serial) => {
            Ok(serial.to_string())
        }
        Some(serial) => Err(format!(
            "L'appareil TikMatrix demande n'est pas connecte : {serial}"
        )),
        None if snapshot.device_serials.len() == 1 => Ok(snapshot.device_serials[0].clone()),
        None if snapshot.device_serials.is_empty() => {
            Err("Aucun appareil Android n'est connecte a TikMatrix".to_string())
        }
        None => {
            Err("Plusieurs appareils TikMatrix sont connectes ; indiquez deviceSerial".to_string())
        }
    }
}

#[cfg(feature = "desktop")]
fn select_tikmatrix_sender_serial(
    snapshot: &TikMatrixSnapshot,
    sender_account: Option<&str>,
    requested_serial: Option<&str>,
) -> Result<String, String> {
    let Some(sender_account) = sender_account else {
        return select_tikmatrix_serial(snapshot, requested_serial);
    };
    let sender_account =
        policy::normalize_tiktok_username(sender_account).map_err(|error| error.to_string())?;
    let candidates = snapshot
        .accounts
        .iter()
        .filter(|account| {
            account.username.eq_ignore_ascii_case(&sender_account)
                && account.logged_in
                && account.enabled
                && requested_serial.is_none_or(|serial| account.device_serial == serial)
        })
        .collect::<Vec<_>>();
    let account = match candidates.as_slice() {
        [account] => *account,
        [] => {
            return Err(format!(
                "Le compte emetteur {sender_account} n'est plus connecte ou actif dans TikMatrix"
            ))
        }
        _ => {
            return Err(format!(
                "Le compte emetteur {sender_account} est associe a plusieurs appareils ; indiquez deviceSerial"
            ))
        }
    };
    if !snapshot
        .device_serials
        .iter()
        .any(|serial| serial == &account.device_serial)
    {
        return Err(format!(
            "L'appareil TikMatrix du compte emetteur {sender_account} n'est pas connecte"
        ));
    }
    let active_on_device = snapshot
        .accounts
        .iter()
        .filter(|candidate| {
            candidate.device_serial == account.device_serial
                && candidate.logged_in
                && candidate.enabled
        })
        .count();
    if active_on_device > 1 {
        return Err(
            "Plusieurs comptes TikTok actifs partagent l'appareil emetteur ; l'identite d'envoi ne peut pas etre garantie"
                .to_string(),
        );
    }
    Ok(account.device_serial.clone())
}

#[cfg(feature = "desktop")]
async fn get_tikmatrix_json(
    client: &reqwest::Client,
    base: &str,
    path: &str,
) -> Result<Value, String> {
    let response = client
        .get(format!("{base}{path}"))
        .send()
        .await
        .map_err(|error| format!("Agent TikMatrix indisponible : {error}"))?;
    let status = response.status();
    let bytes = response.bytes().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        return Err(format!("TikMatrix HTTP {}", status.as_u16()));
    }
    serde_json::from_slice(&bytes).map_err(|error| format!("Reponse TikMatrix invalide : {error}"))
}

#[cfg(feature = "desktop")]
async fn post_tikmatrix_json(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    body: &Value,
) -> Result<Value, String> {
    let response = client
        .post(format!("{base}{path}"))
        .json(body)
        .send()
        .await
        .map_err(|error| format!("Appel TikMatrix impossible : {error}"))?;
    let status = response.status();
    let bytes = response.bytes().await.map_err(|error| error.to_string())?;
    let value = serde_json::from_slice::<Value>(&bytes).unwrap_or_else(
        |_| json!({ "raw": String::from_utf8_lossy(&bytes).chars().take(300).collect::<String>() }),
    );
    if !status.is_success()
        || value
            .get("code")
            .and_then(Value::as_i64)
            .is_some_and(|code| code != 0)
    {
        return Err(format!(
            "TikMatrix a refuse l'action (HTTP {}) : {}",
            status.as_u16(),
            value
        ));
    }
    Ok(value)
}

#[cfg(feature = "desktop")]
fn data_array(value: &Value) -> &[Value] {
    value
        .get("data")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

#[cfg(feature = "desktop")]
fn tikmatrix_sender_account(value: &Value) -> Option<TikTokSenderAccount> {
    let username = value
        .get("username")
        .and_then(Value::as_str)
        .and_then(|username| policy::normalize_tiktok_username(username).ok())?;
    let device_serial = ["device", "serial", "real_serial"]
        .iter()
        .find_map(|field| value.get(*field).and_then(Value::as_str))
        .and_then(|serial| policy::validate_device_serial(serial).ok())?;
    let logged_in = value
        .get("logined")
        .or_else(|| value.get("logged_in"))
        .is_some_and(tikmatrix_truthy);
    let enabled = value
        .get("status")
        .map(|status| {
            status.as_i64() == Some(0) || status.as_u64() == Some(0) || status.as_str() == Some("0")
        })
        .unwrap_or(true);
    let package_name = value
        .get("packagename")
        .or_else(|| value.get("package_name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.chars().count() <= 160)
        .map(str::to_string);
    Some(TikTokSenderAccount {
        username,
        device_serial,
        logged_in,
        enabled,
        package_name,
    })
}

#[cfg(feature = "desktop")]
fn tikmatrix_truthy(value: &Value) -> bool {
    value.as_bool().unwrap_or(false)
        || value.as_i64().is_some_and(|value| value == 1)
        || value.as_u64().is_some_and(|value| value == 1)
        || value
            .as_str()
            .is_some_and(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true"))
}

#[cfg(feature = "desktop")]
fn read_json_object(path: &Path) -> Map<String, Value> {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

#[cfg(feature = "desktop")]
fn load_local_receipts(path: &Path) -> LocalReceiptStore {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

#[cfg(feature = "desktop")]
fn persist_local_receipts(path: &Path, store: &LocalReceiptStore) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(store).map_err(|error| error.to_string())?;
    fs_util::atomic_write(path, bytes).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager(name: &str) -> (PathBuf, TikTokDmManager) {
        let root = std::env::temp_dir().join(format!("cst-tiktok-{name}-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let manager = TikTokDmManager::new(root.join("campaigns.json")).unwrap();
        (root, manager)
    }

    fn request(key: &str) -> PrepareTikTokDmCampaignRequest {
        PrepareTikTokDmCampaignRequest {
            recipients: vec!["@secondaire_1".to_string(), "secondaire.2".to_string()],
            message: "Test controle".to_string(),
            min_interval_minutes: 1,
            max_interval_minutes: 2,
            device_serial: None,
            sender_account: None,
            idempotency_key: key.to_string(),
        }
    }

    #[test]
    fn prepare_normalizes_and_is_idempotent() {
        let (root, manager) = manager("prepare");
        let first = manager
            .prepare("owner-1", request("chat-1"), "human_chat")
            .unwrap();
        let second = manager
            .prepare("owner-1", request("chat-1"), "human_chat")
            .unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(first.recipients, vec!["@secondaire_1", "@secondaire.2"]);
        assert_eq!(first.status, TikTokDmCampaignStatus::Draft);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn five_recipients_and_confirmation_are_enforced() {
        let (root, manager) = manager("bounds");
        let mut too_many = request("too-many");
        too_many.recipients = (0..6).map(|index| format!("test_{index}")).collect();
        assert!(manager
            .prepare("owner-1", too_many, "human_chat")
            .unwrap_err()
            .to_string()
            .contains("entre 1 et 5"));

        let draft = manager
            .prepare("owner-1", request("ok"), "human_chat")
            .unwrap();
        let error = manager
            .confirm(
                "owner-1",
                ConfirmTikTokDmCampaignRequest {
                    campaign_id: draft.id.clone(),
                    owned_accounts_confirmed: true,
                    send_confirmed: false,
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("Confirmez"));
        let queued = manager
            .confirm(
                "owner-1",
                ConfirmTikTokDmCampaignRequest {
                    campaign_id: draft.id,
                    owned_accounts_confirmed: true,
                    send_confirmed: true,
                },
            )
            .unwrap();
        assert_eq!(queued.status, TikTokDmCampaignStatus::Queued);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn connector_claim_and_report_are_lease_bound() {
        let (root, manager) = manager("claim");
        let draft = manager
            .prepare("owner-1", request("job"), "autonomous_agent")
            .unwrap();
        manager
            .confirm(
                "owner-1",
                ConfirmTikTokDmCampaignRequest {
                    campaign_id: draft.id.clone(),
                    owned_accounts_confirmed: true,
                    send_confirmed: true,
                },
            )
            .unwrap();
        let job = manager.claim_next("windows-test").unwrap().unwrap();
        assert_eq!(job.campaign_id, draft.id);
        assert!(manager.claim_next("windows-test").unwrap().is_none());
        let submitted = manager
            .report(TikTokConnectorReportRequest {
                connector_id: "windows-test".to_string(),
                campaign_id: job.campaign_id,
                claim_token: job.claim_token,
                success: true,
                tikmatrix_task_count: 1,
                detail: None,
            })
            .unwrap();
        assert_eq!(submitted.status, TikTokDmCampaignStatus::Submitted);
        assert_eq!(submitted.tikmatrix_task_count, 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sender_account_is_selected_resolved_and_attached_to_the_job() {
        let (root, manager) = manager("sender-account");
        manager
            .heartbeat(TikTokConnectorHeartbeatRequest {
                connector_id: "windows-test".to_string(),
                agent_healthy: true,
                device_serials: vec!["emulator-5554".to_string()],
                devices: Vec::new(),
                scrcpy_available: true,
                ws_scrcpy_available: true,
                ws_scrcpy_online: false,
                adb_error: None,
                account_count: 1,
                accounts: vec![TikTokSenderAccount {
                    username: "emetteur_test".to_string(),
                    device_serial: "emulator-5554".to_string(),
                    logged_in: true,
                    enabled: true,
                    package_name: Some("com.zhiliaoapp.musically".to_string()),
                }],
                current_campaign_id: None,
                error: None,
            })
            .unwrap();
        let selected = manager
            .select_sender_account("owner-1", "@emetteur_test")
            .unwrap();
        assert!(selected.selected);
        assert!(selected.connected);
        let resolved = manager
            .resolve_sender_account("owner-1", None, None)
            .unwrap();
        assert_eq!(resolved.username, "@emetteur_test");
        assert_eq!(resolved.device_serial, "emulator-5554");

        let mut campaign_request = request("sender-job");
        campaign_request.sender_account = Some(resolved.username.clone());
        campaign_request.device_serial = Some(resolved.device_serial.clone());
        let draft = manager
            .prepare("owner-1", campaign_request, "human_chat")
            .unwrap();
        manager
            .confirm(
                "owner-1",
                ConfirmTikTokDmCampaignRequest {
                    campaign_id: draft.id,
                    owned_accounts_confirmed: true,
                    send_confirmed: true,
                },
            )
            .unwrap();
        let job = manager.claim_next("windows-test").unwrap().unwrap();
        assert_eq!(job.sender_account.as_deref(), Some("@emetteur_test"));
        assert_eq!(job.device_serial.as_deref(), Some("emulator-5554"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn start_ws_scrcpy_serializes_and_queues_without_device_or_tikmatrix() {
        assert_eq!(
            serde_json::to_value(TikTokSenderSetupActionKind::StartWsScrcpy).unwrap(),
            serde_json::json!("start_ws_scrcpy")
        );
        let (root, manager) = manager("start-ws-scrcpy");
        manager
            .heartbeat(TikTokConnectorHeartbeatRequest {
                connector_id: "windows-test".to_string(),
                agent_healthy: false,
                device_serials: Vec::new(),
                devices: Vec::new(),
                scrcpy_available: false,
                ws_scrcpy_available: true,
                ws_scrcpy_online: false,
                adb_error: None,
                account_count: 0,
                accounts: Vec::new(),
                current_campaign_id: None,
                error: Some("TikMatrix arrete".to_string()),
            })
            .unwrap();

        let first = manager
            .queue_sender_setup(
                "owner-1",
                QueueTikTokSenderSetupRequest {
                    action: TikTokSenderSetupActionKind::StartWsScrcpy,
                    device_serial: None,
                },
            )
            .unwrap();
        let duplicate = manager
            .queue_sender_setup(
                "owner-1",
                QueueTikTokSenderSetupRequest {
                    action: TikTokSenderSetupActionKind::StartWsScrcpy,
                    device_serial: None,
                },
            )
            .unwrap();
        assert_eq!(first.device_serial, "");
        assert_eq!(first.id, duplicate.id);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sender_login_actions_are_device_bound_idempotent_and_lease_bound() {
        let (root, manager) = manager("sender-login");
        assert!(
            serde_json::from_value::<QueueTikTokSenderSetupRequest>(serde_json::json!({
                "action": "open_login",
                "password": "ne-doit-jamais-etre-accepte"
            }))
            .is_err()
        );
        manager
            .heartbeat(TikTokConnectorHeartbeatRequest {
                connector_id: "windows-test".to_string(),
                agent_healthy: true,
                device_serials: vec!["emulator-5554".to_string()],
                devices: Vec::new(),
                scrcpy_available: true,
                ws_scrcpy_available: true,
                ws_scrcpy_online: false,
                adb_error: None,
                account_count: 0,
                accounts: Vec::new(),
                current_campaign_id: None,
                error: None,
            })
            .unwrap();
        let first = manager
            .queue_sender_setup(
                "owner-1",
                QueueTikTokSenderSetupRequest {
                    action: TikTokSenderSetupActionKind::OpenLogin,
                    device_serial: None,
                },
            )
            .unwrap();
        let duplicate = manager
            .queue_sender_setup(
                "owner-1",
                QueueTikTokSenderSetupRequest {
                    action: TikTokSenderSetupActionKind::OpenLogin,
                    device_serial: Some("emulator-5554".to_string()),
                },
            )
            .unwrap();
        assert_eq!(first.id, duplicate.id);
        assert_eq!(first.device_serial, "emulator-5554");

        let job = manager
            .claim_next_sender_setup("windows-test")
            .unwrap()
            .unwrap();
        assert_eq!(job.action, TikTokSenderSetupActionKind::OpenLogin);
        let claimed_duplicate = manager
            .queue_sender_setup(
                "owner-1",
                QueueTikTokSenderSetupRequest {
                    action: TikTokSenderSetupActionKind::OpenLogin,
                    device_serial: None,
                },
            )
            .unwrap();
        assert!(serde_json::to_value(claimed_duplicate)
            .unwrap()
            .get("claimToken")
            .is_none());
        assert!(manager
            .claim_next_sender_setup("windows-test")
            .unwrap()
            .is_none());
        let submitted = manager
            .report_sender_setup(TikTokSenderSetupReportRequest {
                connector_id: "windows-test".to_string(),
                action_id: job.action_id,
                claim_token: job.claim_token,
                success: true,
                detail: Some("TikTok ouvert".to_string()),
            })
            .unwrap();
        assert_eq!(submitted.status, TikTokSenderSetupActionStatus::Submitted);
        assert_eq!(submitted.detail.as_deref(), Some("TikTok ouvert"));

        let match_accounts = manager
            .queue_sender_setup(
                "owner-1",
                QueueTikTokSenderSetupRequest {
                    action: TikTokSenderSetupActionKind::MatchAccounts,
                    device_serial: None,
                },
            )
            .unwrap();
        assert_ne!(match_accounts.id, first.id);
        let scrcpy = manager
            .queue_sender_setup(
                "owner-1",
                QueueTikTokSenderSetupRequest {
                    action: TikTokSenderSetupActionKind::OpenScrcpy,
                    device_serial: Some("emulator-5554".to_string()),
                },
            )
            .unwrap();
        assert_eq!(scrcpy.action, TikTokSenderSetupActionKind::OpenScrcpy);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn adb_inventory_distinguishes_usb_emulator_network_and_authorization() {
        let devices = parse_adb_devices(
            "List of devices attached\n\
             R58M123ABC device product:dm3q model:SM_S918B device:dm3q transport_id:2\n\
             emulator-5554 device product:sdk model:sdk_gphone64 transport_id:1\n\
             192.168.1.20:5555 offline transport_id:3\n\
             USB-NOT-AUTHORIZED unauthorized transport_id:4\n",
        );
        assert_eq!(devices.len(), 4);
        assert_eq!(devices[0].transport, TikTokAndroidDeviceTransport::Network);
        assert_eq!(devices[0].state, TikTokAndroidDeviceState::Offline);
        let emulator = devices
            .iter()
            .find(|device| device.serial == "emulator-5554")
            .unwrap();
        assert_eq!(emulator.transport, TikTokAndroidDeviceTransport::Emulator);
        let phone = devices
            .iter()
            .find(|device| device.serial == "R58M123ABC")
            .unwrap();
        assert_eq!(phone.transport, TikTokAndroidDeviceTransport::Usb);
        assert_eq!(phone.model.as_deref(), Some("SM S918B"));
        let unauthorized = devices
            .iter()
            .find(|device| device.serial == "USB-NOT-AUTHORIZED")
            .unwrap();
        assert_eq!(unauthorized.state, TikTokAndroidDeviceState::Unauthorized);
    }

    #[test]
    fn usernames_and_multiline_messages_are_rejected() {
        assert!(policy::normalize_tiktok_username("@bad handle").is_err());
        assert!(policy::normalize_tiktok_username("@ok.name_1").is_ok());
        assert!(policy::validate_message("ligne 1\nligne 2".to_string()).is_err());
    }

    #[test]
    fn follower_extraction_is_authorized_bounded_and_completed() {
        let (root, manager) = manager("followers");
        let denied = QueueTikTokFollowerExtractionRequest {
            target_username: "@source_test".to_string(),
            max_count: 50,
            device_serial: None,
            authorized_account_confirmed: false,
            dm_pipeline: None,
            idempotency_key: "followers-denied".to_string(),
        };
        assert!(manager
            .queue_follower_extraction("owner-1", denied, "human_chat")
            .unwrap_err()
            .to_string()
            .contains("autorise"));

        let request = QueueTikTokFollowerExtractionRequest {
            target_username: "source_test".to_string(),
            max_count: 3,
            device_serial: None,
            authorized_account_confirmed: true,
            dm_pipeline: None,
            idempotency_key: "followers-ok".to_string(),
        };
        let queued = manager
            .queue_follower_extraction("owner-1", request, "autonomous_agent")
            .unwrap();
        assert_eq!(queued.target_username, "@source_test");
        assert_eq!(queued.status, TikTokFollowerExtractionStatus::Queued);
        let job = manager
            .claim_next_follower_extraction("windows-test")
            .unwrap()
            .unwrap();
        let submitted = manager
            .report_follower_submission(TikTokFollowerSubmissionReportRequest {
                connector_id: "windows-test".to_string(),
                extraction_id: job.extraction_id.clone(),
                claim_token: job.claim_token,
                success: true,
                tikmatrix_task_count: 1,
                detail: None,
            })
            .unwrap();
        assert_eq!(submitted.status, TikTokFollowerExtractionStatus::Submitted);
        let completed = manager
            .report_follower_result(TikTokFollowerResultReportRequest {
                connector_id: "windows-test".to_string(),
                extraction_id: job.extraction_id,
                success: true,
                usernames: vec![
                    "follower_one".to_string(),
                    "@follower_two".to_string(),
                    "follower_one".to_string(),
                    "follower_three".to_string(),
                    "ignored_four".to_string(),
                ],
                output_file: Some("exported_users_following_tiktok_source_test_3.txt".to_string()),
                detail: None,
            })
            .unwrap();
        assert_eq!(completed.status, TikTokFollowerExtractionStatus::Completed);
        assert_eq!(
            completed.usernames,
            vec!["@follower_one", "@follower_two", "@follower_three"]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn follower_extraction_accepts_one_pass_tikmatrix_maximum() {
        let (root, manager) = manager("followers-maximum");
        let accepted = manager
            .queue_follower_extraction(
                "owner-1",
                QueueTikTokFollowerExtractionRequest {
                    target_username: "@source_test".to_string(),
                    max_count: 1_000,
                    device_serial: None,
                    authorized_account_confirmed: true,
                    dm_pipeline: None,
                    idempotency_key: "followers-maximum".to_string(),
                },
                "human_chat",
            )
            .unwrap();
        assert_eq!(accepted.max_count, 1_000);

        let rejected = manager
            .queue_follower_extraction(
                "owner-1",
                QueueTikTokFollowerExtractionRequest {
                    target_username: "@source_test".to_string(),
                    max_count: 1_001,
                    device_serial: None,
                    authorized_account_confirmed: true,
                    dm_pipeline: None,
                    idempotency_key: "followers-too-many".to_string(),
                },
                "human_chat",
            )
            .unwrap_err();
        assert!(rejected.to_string().contains("1000"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn follower_pipeline_prepares_only_matching_accounts_as_a_draft() {
        let (root, manager) = manager("followers-to-dm");
        let queued = manager
            .queue_follower_extraction(
                "owner-1",
                QueueTikTokFollowerExtractionRequest {
                    target_username: "@source_test".to_string(),
                    max_count: 1_000,
                    device_serial: None,
                    authorized_account_confirmed: true,
                    dm_pipeline: Some(TikTokFollowerDmPipelineRequest {
                        owned_recipient_allowlist: vec![
                            "@secondaire_2".to_string(),
                            "@secondaire_absent".to_string(),
                        ],
                        message: "Message de test exact".to_string(),
                        owned_accounts_confirmed: false,
                    }),
                    idempotency_key: "followers-to-dm".to_string(),
                },
                "autonomous_agent",
            )
            .unwrap();
        let job = manager
            .claim_next_follower_extraction("windows-test")
            .unwrap()
            .unwrap();
        manager
            .report_follower_submission(TikTokFollowerSubmissionReportRequest {
                connector_id: "windows-test".to_string(),
                extraction_id: job.extraction_id.clone(),
                claim_token: job.claim_token,
                success: true,
                tikmatrix_task_count: 1,
                detail: None,
            })
            .unwrap();
        let completed = manager
            .report_follower_result(TikTokFollowerResultReportRequest {
                connector_id: "windows-test".to_string(),
                extraction_id: job.extraction_id,
                success: true,
                usernames: vec![
                    "@inconnu_1".to_string(),
                    "@secondaire_2".to_string(),
                    "@inconnu_2".to_string(),
                ],
                output_file: Some("exported_users_following_tiktok_source_test_3.txt".to_string()),
                detail: None,
            })
            .unwrap();
        let pipeline = completed.dm_pipeline.unwrap();
        let campaign_id = pipeline.prepared_campaign_id.unwrap();
        assert!(pipeline.note.unwrap().contains("Brouillon prepare"));

        let campaigns = manager.list("owner-1").unwrap();
        let campaign = campaigns
            .iter()
            .find(|campaign| campaign.id == campaign_id)
            .unwrap();
        assert_eq!(campaign.recipients, vec!["@secondaire_2"]);
        assert_eq!(campaign.message, "Message de test exact");
        assert_eq!(campaign.status, TikTokDmCampaignStatus::Draft);
        assert!(!campaign.owned_accounts_confirmed);
        assert!(!campaign.send_confirmed);
        assert!(!campaign.recipients.contains(&"@inconnu_1".to_string()));

        let sent = manager
            .confirm(
                "owner-1",
                ConfirmTikTokDmCampaignRequest {
                    campaign_id,
                    owned_accounts_confirmed: true,
                    send_confirmed: true,
                },
            )
            .unwrap();
        assert_eq!(sent.status, TikTokDmCampaignStatus::Queued);
        assert_eq!(queued.target_username, "@source_test");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn follower_text_results_are_normalized_and_limited() {
        let root = std::env::temp_dir().join(format!("cst-tiktok-file-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("followers.txt");
        fs::write(
            &path,
            "\u{feff}@alpha\nbeta\n\"gamma\"\ninvalid handle\n@alpha\n",
        )
        .unwrap();
        assert_eq!(
            read_follower_usernames(&path, 3).unwrap(),
            vec!["@alpha", "@beta", "@gamma"]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn tikmatrix_accounts_are_sanitized_without_credentials() {
        let value = json!({
            "username": "emetteur_test",
            "device": "emulator-5554",
            "logined": 1,
            "status": 0,
            "packagename": "com.zhiliaoapp.musically",
            "pwd": "secret-qui-ne-doit-jamais-sortir"
        });
        let account = tikmatrix_sender_account(&value).unwrap();
        assert_eq!(account.username, "@emetteur_test");
        assert_eq!(account.device_serial, "emulator-5554");
        assert!(account.logged_in);
        assert!(account.enabled);
        assert_eq!(
            serde_json::to_value(account).unwrap(),
            json!({
                "username": "@emetteur_test",
                "deviceSerial": "emulator-5554",
                "loggedIn": true,
                "enabled": true,
                "packageName": "com.zhiliaoapp.musically"
            })
        );
    }
}
